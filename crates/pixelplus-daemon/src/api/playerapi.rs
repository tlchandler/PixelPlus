//! Transport controls: `/player/*` → the playback engine's `PlayerHandle`.

use super::content::player;
use super::{ApiError, ApiResult};
use crate::player::{PlayRequest, PlayerCmd, PlayerStatus, TestRequest, TestTarget};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::State;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use pixelplus_core::model::EffectPreset;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

/// Parse an optional JSON body (empty body → `T::default()`).
pub(crate) fn body_or_default<T: DeserializeOwned + Default>(bytes: &Bytes) -> ApiResult<T> {
    if bytes.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(T::default());
    }
    serde_json::from_slice(bytes)
        .map_err(|e| ApiError::bad_request(format!("That request isn't valid: {e}")))
}

fn ok() -> Json<Value> {
    Json(json!({ "ok": true }))
}

async fn status(State(state): State<AppState>) -> ApiResult<Json<PlayerStatus>> {
    Ok(Json(player(&state)?.status()))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PlayBody {
    #[serde(default)]
    playlist_id: Option<String>,
    #[serde(default)]
    sequence_id: Option<String>,
    #[serde(default)]
    dj_clip_id: Option<String>,
    #[serde(default)]
    effect_id: Option<String>,
    #[serde(default)]
    media_id: Option<String>,
    #[serde(default)]
    start_index: Option<u32>,
}

async fn play(State(state): State<AppState>, body: Bytes) -> ApiResult<Json<Value>> {
    let b: PlayBody = body_or_default(&body)?;
    let show = state.store.get();
    let p = player(&state)?;
    let nothing = b.playlist_id.is_none()
        && b.sequence_id.is_none()
        && b.dj_clip_id.is_none()
        && b.effect_id.is_none()
        && b.media_id.is_none();
    if nothing {
        // "Play": resume when paused, otherwise the scheduled/first playlist.
        if p.status().state == crate::player::PlayerState::Paused {
            p.send(PlayerCmd::Resume).await?;
            return Ok(ok());
        }
    }
    let mut req = PlayRequest {
        loop_until_stopped: Default::default(),
        playlist_id: b.playlist_id,
        sequence_id: b.sequence_id,
        dj_clip_id: b.dj_clip_id,
        effect_id: b.effect_id,
        media_id: b.media_id,
        start_index: b.start_index,
    };
    if nothing {
        req.playlist_id = show
            .schedule
            .entries
            .iter()
            .find(|e| e.enabled && show.playlist(&e.playlist_id).is_some())
            .map(|e| e.playlist_id.clone())
            .or_else(|| {
                show.playlists
                    .iter()
                    .find(|p| !p.items.is_empty())
                    .map(|p| p.id.clone())
            });
        if req.playlist_id.is_none() {
            return Err(ApiError::bad_request(
                "There's nothing to play yet. Add sequences to a playlist first.",
            ));
        }
    }
    if let Some(id) = &req.playlist_id {
        let pl = show
            .playlist(id)
            .ok_or_else(|| ApiError::not_found("That playlist"))?;
        if pl.items.is_empty() && pl.intro.is_empty() {
            return Err(ApiError::bad_request(format!(
                "\"{}\" is empty. Add some sequences to it first.",
                pl.name
            )));
        }
    }
    if let Some(id) = &req.sequence_id {
        show.sequence(id)
            .ok_or_else(|| ApiError::not_found("That sequence"))?;
    }
    if let Some(id) = &req.dj_clip_id {
        show.dj_clip(id)
            .ok_or_else(|| ApiError::not_found("That DJ clip"))?;
    }
    if let Some(id) = &req.effect_id {
        show.effect(id)
            .ok_or_else(|| ApiError::not_found("That look"))?;
    }
    if let Some(id) = &req.media_id {
        show.media_item(id)
            .ok_or_else(|| ApiError::not_found("That audio file"))?;
    }
    p.play(req).await?;
    Ok(ok())
}

#[derive(Deserialize, Default)]
struct StopBody {
    #[serde(default)]
    fade: bool,
}

async fn stop(State(state): State<AppState>, body: Bytes) -> ApiResult<Json<Value>> {
    let b: StopBody = body_or_default(&body)?;
    player(&state)?
        .send(PlayerCmd::Stop { fade: b.fade })
        .await?;
    Ok(ok())
}

async fn pause(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    player(&state)?.send(PlayerCmd::Pause).await?;
    Ok(ok())
}
async fn resume(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    player(&state)?.send(PlayerCmd::Resume).await?;
    Ok(ok())
}
async fn next(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    player(&state)?.send(PlayerCmd::Next).await?;
    Ok(ok())
}
async fn previous(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    player(&state)?.send(PlayerCmd::Previous).await?;
    Ok(ok())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeekBody {
    pos_ms: f64,
}

async fn seek(State(state): State<AppState>, Json(b): Json<SeekBody>) -> ApiResult<Json<Value>> {
    if !b.pos_ms.is_finite() || b.pos_ms < 0.0 {
        return Err(ApiError::bad_request("Pick a position within the song."));
    }
    player(&state)?
        .send(PlayerCmd::Seek(b.pos_ms as u64))
        .await?;
    Ok(ok())
}

fn percent(v: &Value, key: &str) -> ApiResult<u8> {
    let n = v[key]
        .as_f64()
        .ok_or_else(|| ApiError::bad_request(format!("Send {{\"{key}\": 0-100}}.")))?;
    if !(0.0..=100.0).contains(&n) {
        return Err(ApiError::bad_request(format!(
            "{} must be between 0 and 100.",
            capitalize(key)
        )));
    }
    Ok(n.round() as u8)
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

async fn volume(State(state): State<AppState>, Json(v): Json<Value>) -> ApiResult<Json<Value>> {
    let vol = percent(&v, "volume")?;
    player(&state)?.send(PlayerCmd::SetVolume(vol)).await?;
    Ok(Json(json!({ "ok": true, "volume": vol })))
}

async fn brightness(State(state): State<AppState>, Json(v): Json<Value>) -> ApiResult<Json<Value>> {
    let b = percent(&v, "brightness")?;
    player(&state)?.send(PlayerCmd::SetBrightness(b)).await?;
    Ok(Json(json!({ "ok": true, "brightness": b })))
}

async fn blackout(State(state): State<AppState>, body: Bytes) -> ApiResult<Json<Value>> {
    let v: Value = body_or_default(&body)?;
    // UI sends {enabled}; ARCHITECTURE/automation may send {on}. No body = on.
    let on = v["enabled"]
        .as_bool()
        .or_else(|| v["on"].as_bool())
        .unwrap_or(true);
    player(&state)?.send(PlayerCmd::Blackout(on)).await?;
    Ok(Json(json!({ "ok": true, "blackout": on })))
}

/// `POST /player/calibration` `{on: bool}` (default on): play or stop the
/// "Sync lights to sound" pattern: a click every second and every prop on
/// every controller flashing white with it. While it runs, change
/// `settings.audio.outputDelayMs` until flash and click coincide.
async fn calibration(State(state): State<AppState>, body: Bytes) -> ApiResult<Json<Value>> {
    let v: Value = body_or_default(&body)?;
    let on = v["on"].as_bool().unwrap_or(true);
    if on && state.identity().role == crate::node::LocalRole::Follower {
        return Err(ApiError::bad_request(
            "This controller follows its show leader; calibrate on the leader.",
        ));
    }
    player(&state)?.send(PlayerCmd::Calibrate(on)).await?;
    Ok(Json(json!({ "ok": true, "on": on })))
}

/// Show a look live (`{effect: EffectPreset}`), or stop it (`{effect: null}`).
pub(crate) async fn apply_effect(state: &AppState, effect: Option<EffectPreset>) -> ApiResult<()> {
    let p = player(state)?;
    match effect {
        None => {
            if matches!(
                p.status().state,
                crate::player::PlayerState::Effect | crate::player::PlayerState::Testing
            ) {
                p.send(PlayerCmd::TestStop).await?;
            }
            Ok(())
        }
        Some(mut e) => {
            let show = state.store.get();
            if e.target.resolve(&show).is_empty() {
                if show.props.is_empty() {
                    return Err(ApiError::bad_request(
                        "Add some props first, then try the look on them.",
                    ));
                }
                e.target.all = true;
            }
            if e.id.is_empty() {
                e.id = "live".into();
            }
            let target = TestTarget {
                node_id: None,
                output: None,
                props: e.target.clone(),
            };
            p.test_start(TestRequest {
                map_run_id: Default::default(),
                cal: Default::default(),
                identify: Default::default(),
                map: Default::default(),
                mode: "effect".into(),
                color: None,
                speed: None,
                target,
                effect: Some(e),
            })
            .await
        }
    }
}

#[derive(Deserialize, Default)]
struct EffectBody {
    #[serde(default)]
    effect: Option<EffectPreset>,
    #[serde(default, rename = "effectId")]
    effect_id: Option<String>,
}

async fn effect(State(state): State<AppState>, body: Bytes) -> ApiResult<Json<Value>> {
    let b: EffectBody = body_or_default(&body)?;
    let preset = match (b.effect, b.effect_id) {
        (Some(e), _) => Some(e),
        (None, Some(id)) => Some(
            state
                .store
                .get()
                .effect(&id)
                .cloned()
                .ok_or_else(|| ApiError::not_found("That look"))?,
        ),
        (None, None) => None,
    };
    apply_effect(&state, preset).await?;
    Ok(ok())
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/player", get(status))
        .route("/player/play", post(play))
        .route("/player/stop", post(stop))
        .route("/player/pause", post(pause))
        .route("/player/resume", post(resume))
        .route("/player/next", post(next))
        .route("/player/previous", post(previous))
        .route("/player/seek", post(seek))
        .route("/player/volume", put(volume).post(volume))
        .route("/player/brightness", put(brightness).post(brightness))
        .route("/player/blackout", post(blackout))
        .route("/player/effect", post(effect))
        .route("/player/calibration", post(calibration))
}
