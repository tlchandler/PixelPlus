//! Secret trigger links (ARCHITECTURE §12.18, `services::hooks`).
//!
//! **For automation** (no session, no CSRF header — the token is the
//! credential; `api::security::guard` and `auth::require_auth` let
//! `/api/v1/hooks/*` through):
//!
//! `POST /hooks/trigger/:id` with `Authorization: Bearer <token>` (preferred)
//! or `?token=<token>`; `GET` only when the trigger allows simple GET links.
//! Answers `{ok, fired, reason?, message?, error?}`:
//!
//! | Status | When |
//! |---|---|
//! | 202 | fired (`message` says what happened) |
//! | 401 `bad_token` / `token_required` | wrong or missing token, or the trigger has no link (never made, or revoked); wrong ones back off like sign-in |
//! | 404 `not_found` / `feature_disabled` | no such web-link trigger, from the internet while the trigger is home-only, or *Buttons & triggers* is off |
//! | 405 `get_not_allowed` | `GET` while simple GET links are off (`HEAD` never fires) |
//! | 409 `blocked` / `feature_disabled` | a gate said no (cooling down, outside its hours, wrong moment, hourly cap) or the action's feature is off |
//! | 429 `throttled` / `rate_limited` | too many wrong tokens from this address, or more than 10 calls a minute |
//!
//! **Admin** (session + CSRF as usual): `POST /triggers/:id/token` (make or
//! rotate; the only time the token is shown), `DELETE /triggers/:id/token`
//! (revoke), `GET /triggers/links` (addresses to use and each link's last use).

use super::{ApiError, ApiResult, Peer};
use crate::services::hooks::{self, LinkUse, Origin};
use crate::state::AppState;
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use pixelplus_core::model::{FeatureId, TriggerKind};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

fn answer(status: StatusCode, code: &str, message: &str, fired: bool) -> Response {
    let ok = status.is_success();
    let mut body = json!({ "ok": ok, "fired": fired, "message": message });
    if !ok {
        body["reason"] = json!(message);
        body["error"] = json!({ "code": code, "message": message });
    }
    (status, Json(body)).into_response()
}

fn wait_answer(status: StatusCode, code: &str, message: String, wait: Duration) -> Response {
    let mut r = answer(status, code, &message, false);
    if let Ok(v) = HeaderValue::from_str(&wait.as_secs().max(1).to_string()) {
        r.headers_mut().insert(header::RETRY_AFTER, v);
    }
    r
}

fn no_such_link() -> Response {
    answer(
        StatusCode::NOT_FOUND,
        "not_found",
        "There is no trigger link here (it may have been revoked).",
        false,
    )
}

/// Where the call came from: the public listener, a proxy on this machine
/// or a public client address all count as the internet.
pub fn origin(
    via_public: bool,
    peer: Option<std::net::SocketAddr>,
    headers: &HeaderMap,
    client: Option<std::net::IpAddr>,
) -> Origin {
    let internet = via_public
        || super::security::tunnel_request(peer, headers)
        || !client.is_some_and(super::security::lan_peer);
    if internet {
        Origin::Internet
    } else {
        Origin::Home
    }
}

async fn hook(
    State(state): State<AppState>,
    peer: Peer,
    Path(id): Path<String>,
    req: Request,
) -> Response {
    let method = req.method().clone();
    // Link checkers and previews use HEAD: never fire on it.
    if method == Method::HEAD {
        return answer(
            StatusCode::METHOD_NOT_ALLOWED,
            "get_not_allowed",
            "Use POST to fire this trigger.",
            false,
        );
    }
    let headers = req.headers().clone();
    let via_public = req
        .extensions()
        .get::<super::security::ViaPublicListener>()
        .is_some();
    let query = req.uri().query().map(str::to_string);
    drop(req);

    let show = state.store.get();
    if !show.feature(FeatureId::Triggers) {
        return super::features::disabled_error(FeatureId::Triggers, true).into_response();
    }
    let client = super::security::client_ip(
        peer.0,
        &headers,
        &show.settings.security.trusted_proxies,
        super::security::cf_trusted(&show.settings),
    );
    let from = origin(via_public, peer.0, &headers, client);
    let Some(t) = show
        .settings
        .triggers
        .iter()
        .find(|t| t.id == id && t.kind == TriggerKind::Http)
        .cloned()
    else {
        return no_such_link();
    };
    // Home-only links don't exist as far as the internet can tell.
    if from == Origin::Internet && !t.allow_internet {
        return no_such_link();
    }
    drop(show);

    // Wrong tokens back off per address (like sign-in).
    let now = Instant::now();
    if let Err(wait) = state.sessions.hook_throttle.lock().check_ip(client, now) {
        return wait_answer(
            StatusCode::TOO_MANY_REQUESTS,
            "throttled",
            format!(
                "Too many wrong tokens from this address. Try again in {} s.",
                wait.as_secs().max(1)
            ),
            wait,
        );
    }
    let token = hooks::token_from(&headers, query.as_deref());
    let good = token
        .as_deref()
        .is_some_and(|tok| hooks::verify(t.token_hash.as_deref().unwrap_or(""), tok));
    if !good {
        let mut th = state.sessions.hook_throttle.lock();
        th.failure(client, now);
        if let Err(wait) = th.check_global(now) {
            return wait_answer(
                StatusCode::TOO_MANY_REQUESTS,
                "throttled",
                "Too many wrong tokens. Try again in a minute.".into(),
                wait,
            );
        }
        drop(th);
        tracing::info!(
            "Trigger link \"{}\": {} token from {}",
            t.name,
            if token.is_some() { "wrong" } else { "no" },
            client.map_or_else(|| "?".into(), |c| c.to_string())
        );
        return match token {
            None => answer(
                StatusCode::UNAUTHORIZED,
                "token_required",
                "Send the trigger's token as `Authorization: Bearer <token>` (or ?token=).",
                false,
            ),
            Some(_) => answer(
                StatusCode::UNAUTHORIZED,
                "bad_token",
                "That token isn't right (the link may have been renewed or turned off).",
                false,
            ),
        };
    }
    state.sessions.hook_throttle.lock().success(client);
    if method == Method::GET && !t.allow_get {
        let mut r = answer(
            StatusCode::METHOD_NOT_ALLOWED,
            "get_not_allowed",
            "Simple GET links are off for this trigger: use POST, or turn on “Allow simple GET links” in Settings → Triggers.",
            false,
        );
        r.headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("POST"));
        return r;
    }
    if let Err(wait) = state.services.hooks.admit(&t.id, client, now) {
        return wait_answer(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            format!(
                "This link was used too often. Try again in {} s.",
                wait.as_secs().max(1)
            ),
            wait,
        );
    }

    let from_text = client.map_or_else(|| "unknown".to_string(), |c| c.to_string());
    let result =
        crate::services::triggers::fire_trigger_from(&state, &t, "link", Some(from_text.clone()))
            .await;
    let (fired, message) = match &result {
        Ok(m) => (true, m.clone()),
        Err(e) => (false, e.message.clone()),
    };
    let new_address = state
        .services
        .hooks
        .record(
            &state,
            &t.id,
            LinkUse {
                at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                from: from_text.clone(),
                origin: Some(from),
                fired,
                message: message.clone(),
            },
        )
        .await;
    if new_address {
        tracing::info!(
            "Trigger link \"{}\" used from a new address: {from_text} ({})",
            t.name,
            if from == Origin::Internet {
                "internet"
            } else {
                "home network"
            }
        );
        if from == Origin::Internet {
            hooks::new_internet_address(&state, &t.name, &from_text);
        }
    }
    match result {
        Ok(m) => answer(StatusCode::ACCEPTED, "fired", &m, true),
        Err(e) => {
            let code = if e.code == "conflict" {
                "blocked"
            } else {
                e.code
            };
            answer(e.status, code, &e.message, false)
        }
    }
}

// ---------------------------------------------------------------------------
// Admin
// ---------------------------------------------------------------------------

async fn make_token(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let token = hooks::new_token();
    let hash = hooks::hash_token(&token);
    let hint = hooks::hint(&token);
    let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let (name, rotated) = {
        let (hint, at) = (hint.clone(), at.clone());
        let id = id.clone();
        state
            .store
            .update(move |s| {
                let t = s
                    .settings
                    .triggers
                    .iter_mut()
                    .find(|t| t.id == id)
                    .ok_or_else(|| ApiError::not_found("That trigger"))?;
                if t.kind != TriggerKind::Http {
                    return Err(ApiError::bad_request(
                        "Only web-link triggers have a secret link.",
                    ));
                }
                let rotated = t.token_hash.is_some();
                t.token_hash = Some(hash);
                t.token_hint = Some(hint);
                t.token_created_at = Some(at);
                Ok((t.name.clone(), rotated))
            })
            .await?
            .0
    };
    state.services.hooks.forget(&state, &id).await;
    tracing::info!(
        "Trigger link \"{name}\" {}",
        if rotated { "rotated" } else { "created" }
    );
    Ok(Json(json!({
        "token": token,
        "tokenHint": hint,
        "tokenCreatedAt": at,
        "path": format!("/api/v1/hooks/trigger/{id}"),
        "rotated": rotated,
    })))
}

async fn revoke_token(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let id2 = id.clone();
    let (name, _) = state
        .store
        .update(move |s| {
            let t = s
                .settings
                .triggers
                .iter_mut()
                .find(|t| t.id == id2)
                .ok_or_else(|| ApiError::not_found("That trigger"))?;
            t.token_hash = None;
            t.token_hint = None;
            t.token_created_at = None;
            Ok(t.name.clone())
        })
        .await?;
    state.services.hooks.forget(&state, &id).await;
    tracing::info!("Trigger link \"{name}\" revoked");
    Ok(Json(json!({ "ok": true })))
}

/// The port in a `Host` header (`None`: default port or no port).
fn host_port(host: &str) -> Option<u16> {
    let host = host.trim();
    let rest = match host.strip_prefix('[') {
        Some(r) => r.split_once(']').map(|(_, p)| p)?,
        None if host.matches(':').count() == 1 => &host[host.find(':')?..],
        None => return None,
    };
    rest.strip_prefix(':')?.parse().ok()
}

fn with_port(scheme: &str, host: &str, port: u16) -> String {
    let default = (scheme == "http" && port == 80) || (scheme == "https" && port == 443);
    if default || port == 0 {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}:{port}")
    }
}

/// The internet address of the public listener (Cloudflare public name,
/// Tailscale Funnel, or the song-request page's public address), while
/// public access is on.
fn internet_base(show: &pixelplus_core::model::Show) -> Option<String> {
    let s = &show.settings;
    if !show.feature(FeatureId::Remote) || !s.remote.public_listener {
        return None;
    }
    if let Some(h) = s
        .remote
        .cloudflare
        .as_ref()
        .and_then(|c| c.public_host.as_deref())
        .filter(|h| !h.is_empty())
    {
        return Some(format!("https://{}", h.trim_end_matches('.')));
    }
    if let Some(n) = s
        .remote
        .tailscale
        .as_ref()
        .filter(|t| t.enabled && t.funnel_public)
        .and_then(|t| t.dns_name.as_deref())
        .filter(|n| !n.is_empty())
    {
        return Some(format!("https://{}:8443", n.trim_end_matches('.')));
    }
    s.requests
        .public_url
        .as_deref()
        .and_then(|u| u.strip_suffix("/request"))
        .filter(|u| u.starts_with("https://"))
        .map(str::to_string)
}

/// `{addresses: [{kind, label, base}], links: {triggerId: LinkUse}}`.
async fn links(State(state): State<AppState>, headers: HeaderMap, req: Request) -> Json<Value> {
    let show = state.store.get();
    let via_https = req
        .extensions()
        .get::<crate::listeners::ViaHttps>()
        .is_some();
    let http_port = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .filter(|_| !via_https)
        .and_then(host_port)
        .unwrap_or_else(|| state.config.http_addr.port());
    let hostname = crate::cluster::net::hostname();
    let mut addresses = Vec::new();
    if !hostname.is_empty() {
        addresses.push(json!({
            "kind": "name",
            "label": format!("{hostname}.local"),
            "base": with_port("http", &format!("{hostname}.local"), http_port),
        }));
    }
    for ip in crate::cluster::net::interfaces().ips {
        addresses.push(json!({
            "kind": "ip",
            "label": ip.to_string(),
            "base": with_port("http", &ip.to_string(), http_port),
        }));
    }
    if crate::services::tls::active(&state) && !hostname.is_empty() {
        addresses.push(json!({
            "kind": "https",
            "label": format!("{hostname}.local (HTTPS)"),
            "base": with_port("https", &format!("{hostname}.local"), state.config.https_port),
        }));
    }
    if let Some(base) = internet_base(&show) {
        addresses.push(json!({ "kind": "internet", "label": "From the internet", "base": base }));
    }
    Json(json!({
        "addresses": addresses,
        "links": state.services.hooks.last_uses(&state),
    }))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/hooks/trigger/{id}", post(hook).get(hook))
        .route(
            "/triggers/{id}/token",
            post(make_token).delete(revoke_token),
        )
        .route("/triggers/links", get(links))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_and_bases() {
        assert_eq!(host_port("192.168.1.2:8080"), Some(8080));
        assert_eq!(host_port("pp.local"), None);
        assert_eq!(host_port("[fe80::1]:81"), Some(81));
        assert_eq!(host_port("[fe80::1]"), None);
        assert_eq!(with_port("http", "pp.local", 80), "http://pp.local");
        assert_eq!(with_port("http", "pp.local", 8080), "http://pp.local:8080");
        assert_eq!(with_port("https", "pp.local", 443), "https://pp.local");
    }

    #[test]
    fn origins() {
        let lan: std::net::SocketAddr = "192.168.1.9:1".parse().unwrap();
        let lo: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
        let none = HeaderMap::new();
        let mut fwd = HeaderMap::new();
        fwd.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.9"));
        let ip = |s: &str| Some(s.parse().unwrap());
        assert_eq!(
            origin(false, Some(lan), &none, ip("192.168.1.9")),
            Origin::Home
        );
        assert_eq!(
            origin(false, Some(lo), &none, ip("127.0.0.1")),
            Origin::Home
        );
        assert_eq!(
            origin(true, Some(lo), &none, ip("127.0.0.1")),
            Origin::Internet,
            "the public listener"
        );
        assert_eq!(
            origin(false, Some(lo), &fwd, ip("203.0.113.9")),
            Origin::Internet,
            "a proxy on this machine"
        );
        let public: std::net::SocketAddr = "203.0.113.9:1".parse().unwrap();
        assert_eq!(
            origin(false, Some(public), &none, ip("203.0.113.9")),
            Origin::Internet,
            "a port forward"
        );
        assert_eq!(origin(false, None, &none, None), Origin::Internet);
    }
}
