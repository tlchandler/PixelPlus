//! Cluster-internal endpoints (`/api/v1/cluster/*`, ARCHITECTURE §7.5), and
//! "Join another show" (`/system/join-show`, a normal signed-in endpoint).
//!
//! The cluster endpoints bypass the session check in `auth::require_auth` and
//! authenticate signed requests instead (`cluster::sig`): the leader signs
//! with the key of the follower it calls, a follower with its own key. A
//! follower key never authenticates anything else. `POST /cluster/adopt` is
//! also accepted unsigned in the cases listed in `follower::handle_adopt`.

use super::{ApiError, ApiResult, Peer};
use crate::cluster::follower::AdoptAuth;
use crate::cluster::leader::{AdoptCall, AdoptReply};
use crate::cluster::manifest::{self, ManifestError};
use crate::cluster::sig;
use crate::cluster::slices::{self, SliceError};
use crate::cluster::{follower, ClusterCommand, ClusterHandle, JoinWindow, JOIN_WINDOW};
use crate::node::LocalRole;
use crate::state::AppState;
use axum::body::{Body, Bytes};
use axum::extract::{OriginalUri, Path, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use pixelplus_core::model::NodeRole;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/cluster/manifest/{node_id}", get(get_manifest))
        .route("/cluster/slice/{node_id}/{seq_id}", get(get_slice))
        .route("/cluster/adopt", post(adopt))
        .route("/cluster/release", post(release))
        .route("/cluster/command", post(command))
        .route("/cluster/status", get(status))
        .route(
            "/system/join-show",
            get(join_status).post(join_open).delete(join_close),
        )
}

pub(crate) fn handle(state: &AppState) -> ApiResult<ClusterHandle> {
    state
        .services
        .cluster
        .get()
        .cloned()
        .ok_or_else(|| ApiError::unavailable("The cluster service is not running."))
}

fn path_of(uri: &OriginalUri) -> String {
    uri.0
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| uri.0.path().to_string())
}

/// 401 for a refused signed request (with our clock when only the time was off).
fn refusal(r: sig::Refusal, hint: Option<(String, String)>) -> Response {
    let (code, msg) = match r {
        sig::Refusal::Unauthenticated => ("cluster_auth", "Missing or wrong cluster signature."),
        sig::Refusal::Skew { .. } => ("clock_skew", "The clocks of the two controllers differ too much."),
        sig::Refusal::Replay => ("replay", "That request was already used."),
    };
    let mut resp = ApiError::new(StatusCode::UNAUTHORIZED, code, msg).into_response();
    if let (sig::Refusal::Skew { now }, Some((key, nonce))) = (r, hint) {
        if let Ok(v) = HeaderValue::from_str(&sig::time_proof(&key, now, &nonce)) {
            resp.headers_mut().insert(sig::TIME_HEADER, v);
        }
    }
    resp
}

fn auth_header(headers: &HeaderMap) -> Option<&str> {
    headers.get(sig::AUTH_HEADER).and_then(|v| v.to_str().ok())
}

/// Follower side: the request is signed by our leader with our key.
fn verify_from_leader(
    state: &AppState,
    cluster: &ClusterHandle,
    headers: &HeaderMap,
    method: &Method,
    path: &str,
    body: &[u8],
) -> Result<sig::Signed, Response> {
    let identity = state.identity();
    let key_for = |sender: &str| {
        (identity.role == LocalRole::Follower && identity.leader_id.as_deref() == Some(sender))
            .then(|| identity.cluster_key.clone())
            .flatten()
    };
    match sig::check(
        auth_header(headers),
        method.as_str(),
        path,
        body,
        &cluster.shared.nonces,
        sig::now_s(),
        key_for,
    ) {
        Ok((signed, _)) => {
            follower::leader_key_confirmed(&cluster.shared);
            Ok(signed)
        }
        Err((r, hint)) => Err(refusal(r, hint)),
    }
}

/// Leader side: the request is signed by follower `node_id` with its key.
/// Returns the key (to sign the reply).
fn verify_from_follower(
    state: &AppState,
    cluster: &ClusterHandle,
    headers: &HeaderMap,
    path: &str,
    node_id: &str,
) -> Result<(sig::Signed, String), Response> {
    let sh = &cluster.shared;
    let key_for = |sender: &str| {
        (sender == node_id)
            .then(|| sh.follower_key(state, sender))
            .flatten()
    };
    sig::check(
        auth_header(headers),
        "GET",
        path,
        b"",
        &sh.nonces,
        sig::now_s(),
        key_for,
    )
    .map_err(|(r, hint)| refusal(r, hint))
}

fn require_leader(state: &AppState) -> ApiResult<()> {
    if state.identity().role == LocalRole::Leader {
        Ok(())
    } else {
        Err(ApiError::conflict(
            "This controller is not the show leader.",
        ))
    }
}

/// The follower (by id) must be an adopted follower of this show.
fn require_member(state: &AppState, node_id: &str) -> ApiResult<()> {
    let show = state.store.get();
    match show.node(node_id) {
        Some(n) if n.role == NodeRole::Follower && n.adopted => Ok(()),
        _ => Err(ApiError::not_found("That controller")),
    }
}

async fn get_manifest(
    State(state): State<AppState>,
    uri: OriginalUri,
    headers: HeaderMap,
    Path(node_id): Path<String>,
) -> ApiResult<Response> {
    let cluster = handle(&state)?;
    let (signed, key) = match verify_from_follower(&state, &cluster, &headers, &path_of(&uri), &node_id) {
        Ok(v) => v,
        Err(r) => return Ok(r),
    };
    require_leader(&state)?;
    require_member(&state, &node_id)?;
    let show = state.store.get();
    let leader_id = state.identity().id;
    let data_dir = state.config.data_dir.clone();
    let m = tokio::task::spawn_blocking(move || {
        manifest::build(&show, &leader_id, &node_id, &data_dir)
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(|e| match e {
        ManifestError::UnknownNode(_) => ApiError::not_found("That controller"),
        ManifestError::NotFollower(n) => ApiError::bad_request(format!("{n} is the show leader")),
    })?;
    let body = serde_json::to_vec(&m).map_err(ApiError::internal)?;
    let mac = sig::reply_mac(&key, &signed.nonce, &format!("manifest {}", sig::sha256_hex(&body)));
    Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .header(sig::REPLY_HEADER, mac)
        .body(Body::from(body))
        .map_err(ApiError::internal)
}

/// Parse a single `Range: bytes=…` against a file of `len` bytes.
/// `None` = no usable range (serve the whole file); `Some(Err)` = unsatisfiable.
pub fn parse_range(value: &str, len: u64) -> Option<Result<(u64, u64), ()>> {
    let spec = value.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None; // multi-range: not supported, send everything
    }
    let (a, b) = spec.split_once('-')?;
    let (a, b) = (a.trim(), b.trim());
    let range = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        if n == 0 {
            return Some(Err(()));
        }
        (len.saturating_sub(n), len.checked_sub(1)?)
    } else {
        let start: u64 = a.parse().ok()?;
        let end = if b.is_empty() {
            len.saturating_sub(1)
        } else {
            b.parse::<u64>().ok()?.min(len.saturating_sub(1))
        };
        if start >= len || end < start {
            return Some(Err(()));
        }
        (start, end)
    };
    Some(Ok(range))
}

async fn get_slice(
    State(state): State<AppState>,
    uri: OriginalUri,
    headers: HeaderMap,
    Path((node_id, seq_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    let cluster = handle(&state)?;
    let (signed, key) = match verify_from_follower(&state, &cluster, &headers, &path_of(&uri), &node_id) {
        Ok(v) => v,
        Err(r) => return Ok(r),
    };
    require_leader(&state)?;
    require_member(&state, &node_id)?;
    let show = state.store.get();
    let job =
        slices::job(&show, &state.config.data_dir, &node_id, &seq_id).map_err(|e| match e {
            SliceError::UnknownNode(_) => ApiError::not_found("That controller"),
            SliceError::UnknownSequence(_) => ApiError::not_found("That sequence"),
            SliceError::MissingFile(f) => ApiError::new(
                StatusCode::GONE,
                "missing_file",
                format!("The sequence file {f} is missing on the leader."),
            ),
            SliceError::Generate(m) => ApiError::internal(m),
        })?;
    let etag = format!("\"{}\"", job.key);
    let meta = match cluster
        .shared
        .slices
        .ensure_within(job, Duration::from_secs(10))
        .await
    {
        Ok(Some(m)) => m,
        Ok(None) => {
            let mut r = ApiError::unavailable("The leader is still preparing this sequence.")
                .into_response();
            r.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("3"));
            return Ok(r);
        }
        Err(e) => return Err(ApiError::internal(e)),
    };

    let reply = sig::reply_mac(&key, &signed.nonce, &format!("slice {etag} {}", meta.sha256));
    let common = |b: axum::http::response::Builder| {
        b.header(header::ETAG, &etag)
            .header(header::ACCEPT_RANGES, "bytes")
            .header("x-pixelplus-sha256", &meta.sha256)
            .header(sig::REPLY_HEADER, &reply)
            // Never let the compression layer touch (and break) byte ranges.
            .header(header::CONTENT_ENCODING, "identity")
    };
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag))
    {
        return common(Response::builder().status(StatusCode::NOT_MODIFIED))
            .body(Body::empty())
            .map_err(ApiError::internal);
    }
    let len = meta.bytes;
    let if_range_ok = headers
        .get(header::IF_RANGE)
        .and_then(|v| v.to_str().ok())
        .map_or(true, |v| v.trim() == etag);
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .filter(|_| if_range_ok)
        .and_then(|v| parse_range(v, len));
    let (status, start, end) = match range {
        None => (StatusCode::OK, 0, len.saturating_sub(1)),
        Some(Ok((s, e))) => (StatusCode::PARTIAL_CONTENT, s, e),
        Some(Err(())) => {
            return common(Response::builder().status(StatusCode::RANGE_NOT_SATISFIABLE))
                .header(header::CONTENT_RANGE, format!("bytes */{len}"))
                .body(Body::empty())
                .map_err(ApiError::internal);
        }
    };
    let count = if len == 0 { 0 } else { end - start + 1 };
    let mut file = tokio::fs::File::open(&meta.path).await?;
    if start > 0 {
        use tokio::io::AsyncSeekExt;
        file.seek(std::io::SeekFrom::Start(start)).await?;
    }
    use tokio::io::AsyncReadExt;
    let stream = tokio_util::io::ReaderStream::with_capacity(file.take(count), 64 * 1024);
    let mut b = common(Response::builder().status(status))
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, count);
    if status == StatusCode::PARTIAL_CONTENT {
        b = b.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"));
    }
    b.body(Body::from_stream(stream))
        .map_err(ApiError::internal)
}

async fn adopt(
    State(state): State<AppState>,
    peer: Peer,
    uri: OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let cluster = handle(&state)?;
    // Signed by our current leader? (An unsigned call may still be allowed.)
    let signed_by_leader = if auth_header(&headers).is_some() {
        match verify_from_leader(&state, &cluster, &headers, &Method::POST, &path_of(&uri), &body) {
            Ok(_) => true,
            Err(r) => return Ok(r),
        }
    } else {
        false
    };
    let call: AdoptCall = serde_json::from_slice(&body)
        .map_err(|e| ApiError::bad_request(format!("Invalid adoption request: {e}")))?;
    let auth = AdoptAuth {
        signed_by_leader,
        peer: peer.0.map(|p| p.ip()),
    };
    let reply: AdoptReply = follower::handle_adopt(&state, &cluster.shared, call, auth).await?;
    Ok(Json(reply).into_response())
}

async fn release(
    State(state): State<AppState>,
    peer: Peer,
    uri: OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let cluster = handle(&state)?;
    // Our leader (signed) or someone signed in to this controller's own UI
    // ("Forget leader") may release it.
    if !super::auth::is_authenticated(&state, &headers, peer.0) {
        if let Err(r) = verify_from_leader(&state, &cluster, &headers, &Method::POST, &path_of(&uri), &body) {
            return Ok(r);
        }
    }
    follower::handle_release(&state, &cluster.shared).await?;
    Ok(Json(json!({ "ok": true })).into_response())
}

async fn command(
    State(state): State<AppState>,
    uri: OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let cluster = handle(&state)?;
    if let Err(r) = verify_from_leader(&state, &cluster, &headers, &Method::POST, &path_of(&uri), &body) {
        return Ok(r);
    }
    let cmd: ClusterCommand = serde_json::from_slice(&body)
        .map_err(|e| ApiError::bad_request(format!("Invalid command: {e}")))?;
    if state.identity().role != LocalRole::Follower {
        return Err(ApiError::conflict("This controller is not a follower."));
    }
    follower::handle_command(&state, &cluster.shared, cmd).await?;
    Ok(Json(json!({ "ok": true })).into_response())
}

// ---------------------------------------------------------------------------
// Join another show / allow a new leader
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct JoinBody {
    /// Address (or host name) of the leader that may adopt this controller.
    #[serde(default)]
    leader_url: Option<String>,
}

fn join_json(w: Option<JoinWindow>) -> Value {
    match w {
        Some(w) => json!({
            "open": true,
            "secondsLeft": w.until.saturating_duration_since(Instant::now()).as_secs(),
            "leaderAddress": w.leader_ip.map(|ip| ip.to_string()),
        }),
        None => json!({ "open": false, "secondsLeft": 0, "leaderAddress": null }),
    }
}

async fn join_status(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(join_json(handle(&state)?.shared.join_window())))
}

/// `POST /system/join-show {leaderUrl?}` (signed-in admin): for the next
/// 15 minutes a leader (only that one, when named) may adopt this controller,
/// even though it is a leader itself or already follows another leader.
async fn join_open(State(state): State<AppState>, body: Bytes) -> ApiResult<Json<Value>> {
    let cluster = handle(&state)?;
    let b: JoinBody = super::playerapi::body_or_default(&body)?;
    let leader_ip = match b.leader_url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
        None => None,
        Some(u) => {
            let with_scheme = if u.contains("://") { u.to_string() } else { format!("http://{u}") };
            let url = reqwest::Url::parse(&with_scheme)
                .map_err(|_| ApiError::bad_request("That isn't a valid address."))?;
            let host = url
                .host_str()
                .ok_or_else(|| ApiError::bad_request("That isn't a valid address."))?
                .trim_matches(['[', ']'])
                .to_string();
            let ip = match host.parse::<std::net::IpAddr>() {
                Ok(ip) => ip,
                Err(_) => tokio::net::lookup_host((host.as_str(), 80))
                    .await
                    .ok()
                    .and_then(|mut a| a.next())
                    .map(|a| a.ip())
                    .ok_or_else(|| {
                        ApiError::bad_request(format!("Couldn't find “{host}” on the network. Use its IP address."))
                    })?,
            };
            Some(ip)
        }
    };
    let w = JoinWindow {
        until: Instant::now() + JOIN_WINDOW,
        leader_ip,
    };
    *cluster.shared.join.lock() = Some(w);
    let who = leader_ip.map(|ip| format!("the leader at {ip}")).unwrap_or_else(|| "a show leader".into());
    crate::cluster::log_warning(
        &state,
        format!("For the next 15 minutes {who} may adopt this controller."),
    );
    Ok(Json(join_json(Some(w))))
}

async fn join_close(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let cluster = handle(&state)?;
    *cluster.shared.join.lock() = None;
    Ok(Json(join_json(None)))
}

/// Diagnostics: this node's cluster view (signed-in UI).
async fn status(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    if !super::auth::is_authenticated(&state, &headers, peer.0) {
        return Err(ApiError::unauthorized());
    }
    let cluster = handle(&state)?;
    let identity = state.identity();
    let report =
        (identity.role == LocalRole::Follower).then(|| follower::report(&state, &cluster.shared));
    Ok(Json(json!({
        "id": identity.id,
        "role": identity.role,
        "leaderId": identity.leader_id,
        "leaderUrl": identity.leader_url,
        "clusterPort": cluster.settings().port,
        "overlayPort": cluster.settings().overlay_port,
        "report": report,
        "nodes": cluster.nodes_status(),
    })))
}

#[cfg(test)]
mod tests {
    use super::parse_range;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-", 100), Some(Ok((0, 99))));
        assert_eq!(parse_range("bytes=10-19", 100), Some(Ok((10, 19))));
        assert_eq!(parse_range("bytes=90-200", 100), Some(Ok((90, 99))));
        assert_eq!(parse_range("bytes=-10", 100), Some(Ok((90, 99))));
        assert_eq!(parse_range("bytes=100-", 100), Some(Err(())));
        assert_eq!(parse_range("bytes=5-2", 100), Some(Err(())));
        assert_eq!(parse_range("bytes=0-1,5-6", 100), None);
        assert_eq!(parse_range("items=0-1", 100), None);
        assert_eq!(parse_range("bytes=x-", 100), None);
    }
}
