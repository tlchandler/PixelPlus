//! Overlays on matrix props (ARCHITECTURE §10): shared-memory fast path for
//! local processes, raw frames over HTTP, scrolling text and QR codes.

use super::content::player;
use super::{ApiError, ApiResult};
use crate::player::{OverlayCmd, OverlayInfo, PlayerCmd};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::routing::{post, put};
use axum::{Json, Router};
use pixelplus_core::model::{MatrixInfo, Prop};
use serde::Deserialize;
use serde_json::{json, Value};

fn prop(state: &AppState, id: &str) -> ApiResult<Prop> {
    state.store.get().prop(id).cloned().ok_or_else(|| ApiError::not_found("That prop"))
}

fn matrix(p: &Prop) -> ApiResult<MatrixInfo> {
    p.matrix.clone().ok_or_else(|| {
        ApiError::bad_request(format!(
            "\"{}\" isn't set up as a matrix. Set its width and height in the prop's settings first.",
            p.name
        ))
    })
}

async fn open(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<OverlayInfo>> {
    let p = prop(&state, &id)?;
    matrix(&p)?;
    player(&state)?.overlay_open(id).await.map(Json)
}

async fn frame(State(state): State<AppState>, Path(id): Path<String>, body: Bytes) -> ApiResult<Json<Value>> {
    let p = prop(&state, &id)?;
    let m = matrix(&p)?;
    let want = m.width as usize * m.height as usize * 3;
    if body.len() != want {
        return Err(ApiError::bad_request(format!(
            "A frame for \"{}\" must be {}×{}×3 = {want} bytes (got {}).",
            p.name,
            m.width,
            m.height,
            body.len()
        )));
    }
    player(&state)?.send(PlayerCmd::Overlay(OverlayCmd::Frame { prop_id: id, rgb: body })).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct EnableBody {
    enabled: bool,
}

async fn enable(State(state): State<AppState>, Path(id): Path<String>, Json(b): Json<EnableBody>) -> ApiResult<Json<Value>> {
    prop(&state, &id)?;
    player(&state)?
        .send(PlayerCmd::Overlay(OverlayCmd::Enable { prop_id: id, enabled: b.enabled }))
        .await?;
    Ok(Json(json!({ "ok": true, "enabled": b.enabled })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TextBody {
    text: String,
    #[serde(default)]
    color: Option<String>,
    #[serde(default = "yes")]
    scroll: bool,
    #[serde(default)]
    duration_ms: Option<u64>,
}

fn yes() -> bool {
    true
}

async fn text(State(state): State<AppState>, Path(id): Path<String>, Json(b): Json<TextBody>) -> ApiResult<Json<Value>> {
    let p = prop(&state, &id)?;
    matrix(&p)?;
    let text = b.text.trim().to_string();
    if text.is_empty() {
        return Err(ApiError::bad_request("Type some text to show."));
    }
    if text.chars().count() > 500 {
        return Err(ApiError::bad_request("That text is too long (500 characters max)."));
    }
    let color = b.color.unwrap_or_else(|| "#ffffff".into());
    if pixelplus_core::effects::Rgb::from_hex(&color).is_none() {
        return Err(ApiError::bad_request(format!("\"{color}\" isn't a colour (use #rrggbb).")));
    }
    let duration_ms = b.duration_ms.unwrap_or(15_000).clamp(1_000, 3_600_000);
    player(&state)?
        .send(PlayerCmd::Overlay(OverlayCmd::Text { prop_id: id, text, color, scroll: b.scroll, duration_ms }))
        .await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QrBody {
    url: String,
    #[serde(default)]
    duration_ms: Option<u64>,
}

async fn qr(State(state): State<AppState>, Path(id): Path<String>, Json(b): Json<QrBody>) -> ApiResult<Json<Value>> {
    let p = prop(&state, &id)?;
    let m = matrix(&p)?;
    let url = b.url.trim().to_string();
    if url.is_empty() {
        return Err(ApiError::bad_request("Enter the link the QR code should open."));
    }
    // Check it fits before asking the player to draw it.
    pixelplus_core::text::render_qr(&url, m.width, m.height, Default::default())
        .map_err(|e| ApiError::bad_request(capitalize(&e.to_string())))?;
    let duration_ms = b.duration_ms.unwrap_or(30_000).clamp(1_000, 3_600_000);
    player(&state)?
        .send(PlayerCmd::Overlay(OverlayCmd::Qr { prop_id: id, url, duration_ms }))
        .await?;
    Ok(Json(json!({ "ok": true })))
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str() + ".").unwrap_or_default()
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/overlay/{prop_id}", post(enable))
        .route("/overlay/{prop_id}/open", post(open))
        .route("/overlay/{prop_id}/frame", put(frame).post(frame).layer(DefaultBodyLimit::max(64 * 1024 * 1024)))
        .route("/overlay/{prop_id}/text", post(text))
        .route("/overlay/{prop_id}/qr", post(qr))
}
