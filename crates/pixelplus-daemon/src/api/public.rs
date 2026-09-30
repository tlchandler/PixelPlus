//! Visitor song requests: public page (`/public/requests`, no sign-in, no
//! private data) and the admin queue (`/requests`).

use super::{ApiError, ApiResult, Peer};
use crate::services::requests::SongRequest;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::{delete, get};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::IpAddr;
use std::time::Instant;

/// `GET /public/health`: liveness for Docker / monitoring. Unauthenticated and
/// cheap (no I/O): `{ok, version, role}`.
async fn health(State(state): State<AppState>) -> Json<Value> {
    let role = match state.identity.read().role {
        crate::node::LocalRole::Unconfigured => "unconfigured",
        crate::node::LocalRole::Leader => "leader",
        crate::node::LocalRole::Follower => "follower",
    };
    Json(json!({ "ok": true, "version": env!("CARGO_PKG_VERSION"), "role": role }))
}

async fn public_list(State(state): State<AppState>) -> Json<Value> {
    Json(crate::services::requests::public_view(&state))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmitBody {
    sequence_id: String,
    #[serde(default)]
    name: Option<String>,
}

/// Client address; behind a local reverse proxy (tunnel), trust X-Forwarded-For.
fn client_ip(peer: Peer, headers: &HeaderMap) -> Option<IpAddr> {
    let direct = peer.0.map(|a| a.ip());
    if direct.is_some_and(|ip| ip.is_loopback()) {
        if let Some(fwd) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .and_then(|v| v.trim().parse().ok())
        {
            return Some(fwd);
        }
    }
    direct
}

async fn submit(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(b): Json<SubmitBody>,
) -> ApiResult<Json<Value>> {
    let show = state.store.get();
    let (req, position) = state.services.requests.submit(
        &show,
        &b.sequence_id,
        b.name.as_deref(),
        client_ip(peer, &headers),
        Instant::now(),
    )?;
    let who = req
        .requested_by
        .as_deref()
        .map(|n| format!(" (from {n})"))
        .unwrap_or_default();
    state.events.toast(
        crate::events::ToastKind::Info,
        format!("New song request: {}{who}", req.name),
    );
    state
        .events
        .publish("requests", &state.services.requests.list());
    Ok(Json(
        json!({ "ok": true, "id": req.id, "position": position }),
    ))
}

async fn admin_list(State(state): State<AppState>) -> Json<Vec<SongRequest>> {
    Json(state.services.requests.list())
}

async fn admin_delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    if !state.services.requests.remove(&id) {
        return Err(ApiError::not_found("That request"));
    }
    state
        .events
        .publish("requests", &state.services.requests.list());
    Ok(Json(json!({ "ok": true })))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/public/health", get(health))
        .route("/public/requests", get(public_list).post(submit))
        .route("/requests", get(admin_list))
        .route("/requests/{id}", delete(admin_delete))
}
