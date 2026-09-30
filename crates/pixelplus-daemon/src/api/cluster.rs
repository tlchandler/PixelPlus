//! Cluster-internal endpoints (`/api/v1/cluster/*`, ARCHITECTURE §7).
//!
//! These bypass the session check in `auth::require_auth` and authenticate
//! with the shared cluster key (`X-PixelPlus-Key`) instead — except
//! `POST /cluster/adopt`, which an unadopted node must accept from a leader
//! it has never met (see `cluster::follower::handle_adopt` for the rules).

use super::{ApiError, ApiResult, Peer};
use crate::cluster::leader::{AdoptCall, AdoptReply};
use crate::cluster::manifest::{self, ManifestError, NodeManifest};
use crate::cluster::slices::{self, SliceError};
use crate::cluster::{follower, ClusterCommand, ClusterHandle};
use crate::node::LocalRole;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use pixelplus_core::model::NodeRole;
use serde_json::{json, Value};
use std::time::Duration;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/cluster/manifest/{node_id}", get(get_manifest))
        .route("/cluster/slice/{node_id}/{seq_id}", get(get_slice))
        .route("/cluster/adopt", post(adopt))
        .route("/cluster/release", post(release))
        .route("/cluster/command", post(command))
        .route("/cluster/status", get(status))
}

pub(crate) fn handle(state: &AppState) -> ApiResult<ClusterHandle> {
    state
        .services
        .cluster
        .get()
        .cloned()
        .ok_or_else(|| ApiError::unavailable("The cluster service is not running."))
}

fn require_key(state: &AppState, headers: &HeaderMap) -> ApiResult<()> {
    if super::auth::has_cluster_key(state, headers) {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "cluster_key",
            "Missing or wrong cluster key.",
        ))
    }
}

fn require_leader(state: &AppState) -> ApiResult<()> {
    if state.identity().role == LocalRole::Leader {
        Ok(())
    } else {
        Err(ApiError::conflict("This controller is not the show leader."))
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
    headers: HeaderMap,
    Path(node_id): Path<String>,
) -> ApiResult<Json<NodeManifest>> {
    require_key(&state, &headers)?;
    require_leader(&state)?;
    require_member(&state, &node_id)?;
    let show = state.store.get();
    let leader_id = state.identity().id;
    let data_dir = state.config.data_dir.clone();
    let m = tokio::task::spawn_blocking(move || manifest::build(&show, &leader_id, &node_id, &data_dir))
        .await
        .map_err(ApiError::internal)?
        .map_err(|e| match e {
            ManifestError::UnknownNode(_) => ApiError::not_found("That controller"),
            ManifestError::NotFollower(n) => ApiError::bad_request(format!("{n} is the show leader")),
        })?;
    Ok(Json(m))
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
        let end = if b.is_empty() { len.saturating_sub(1) } else { b.parse::<u64>().ok()?.min(len.saturating_sub(1)) };
        if start >= len || end < start {
            return Some(Err(()));
        }
        (start, end)
    };
    Some(Ok(range))
}

async fn get_slice(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((node_id, seq_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    require_key(&state, &headers)?;
    require_leader(&state)?;
    require_member(&state, &node_id)?;
    let cluster = handle(&state)?;
    let show = state.store.get();
    let job = slices::job(&show, &state.config.data_dir, &node_id, &seq_id).map_err(|e| match e {
        SliceError::UnknownNode(_) => ApiError::not_found("That controller"),
        SliceError::UnknownSequence(_) => ApiError::not_found("That sequence"),
        SliceError::MissingFile(f) => ApiError::new(StatusCode::GONE, "missing_file", format!("The sequence file {f} is missing on the leader.")),
        SliceError::Generate(m) => ApiError::internal(m),
    })?;
    let etag = format!("\"{}\"", job.key);
    let meta = match cluster.shared.slices.ensure_within(job, Duration::from_secs(10)).await {
        Ok(Some(m)) => m,
        Ok(None) => {
            let mut r = ApiError::unavailable("The leader is still preparing this sequence.").into_response();
            r.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("3"));
            return Ok(r);
        }
        Err(e) => return Err(ApiError::internal(e)),
    };

    let common = |b: axum::http::response::Builder| {
        b.header(header::ETAG, &etag)
            .header(header::ACCEPT_RANGES, "bytes")
            .header("x-pixelplus-sha256", &meta.sha256)
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
    b.body(Body::from_stream(stream)).map_err(ApiError::internal)
}

async fn adopt(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(call): Json<AdoptCall>,
) -> ApiResult<Json<AdoptReply>> {
    let cluster = handle(&state)?;
    Ok(Json(follower::handle_adopt(&state, &cluster.shared, &headers, call).await?))
}

async fn release(State(state): State<AppState>, peer: Peer, headers: HeaderMap) -> ApiResult<Json<Value>> {
    // The leader (with the key) or someone signed in to this controller's own
    // UI ("Forget leader") may release it.
    if !super::auth::is_authenticated(&state, &headers, peer.0) {
        require_key(&state, &headers)?;
    }
    let cluster = handle(&state)?;
    follower::handle_release(&state, &cluster.shared).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn command(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(cmd): Json<ClusterCommand>,
) -> ApiResult<Json<Value>> {
    require_key(&state, &headers)?;
    if state.identity().role != LocalRole::Follower {
        return Err(ApiError::conflict("This controller is not a follower."));
    }
    let cluster = handle(&state)?;
    follower::handle_command(&state, &cluster.shared, cmd).await?;
    Ok(Json(json!({ "ok": true })))
}

/// Diagnostics: this node's cluster view (key or signed-in UI).
async fn status(State(state): State<AppState>, peer: Peer, headers: HeaderMap) -> ApiResult<Json<Value>> {
    if !super::auth::is_authenticated(&state, &headers, peer.0) {
        return Err(ApiError::unauthorized());
    }
    let cluster = handle(&state)?;
    let identity = state.identity();
    let report = (identity.role == LocalRole::Follower).then(|| follower::report(&state, &cluster.shared));
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
