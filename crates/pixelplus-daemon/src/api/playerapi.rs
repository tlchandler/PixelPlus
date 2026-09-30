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
        // Smart playlists (F18) get their songs when they start.
        if pl.items.is_empty() && pl.intro.is_empty() && pl.smart.is_none() {
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

/// `POST /player/calibration` `{on: bool, pattern?: "v1"|"v2", seed?}`
/// (default on, v1): play or stop the calibration pattern.
///
/// * v1 ("Sync lights to sound"): a click every second and every prop on
///   every controller flashing white with it. While it runs, change
///   `settings.audio.outputDelayMs` until flash and click coincide.
/// * v2 (F1 "Measure with my phone"): the seeded pseudo-random train of
///   `core::calpattern`; answers with the plan
///   `{seed, v, eventsMs, flashMs, windowMs, leadInMs, chirp, startsInMs}`.
async fn calibration(State(state): State<AppState>, body: Bytes) -> ApiResult<Json<Value>> {
    let v: Value = body_or_default(&body)?;
    let on = v["on"].as_bool().unwrap_or(true);
    if on && state.identity().role == crate::node::LocalRole::Follower {
        return Err(ApiError::bad_request(
            "This controller follows its show leader; calibrate on the leader.",
        ));
    }
    let v2 = on && v["pattern"].as_str() == Some("v2");
    if !v2 {
        player(&state)?.send(PlayerCmd::Calibrate(on)).await?;
        return Ok(Json(json!({ "ok": true, "on": on })));
    }
    let seed = match v.get("seed") {
        None | Some(Value::Null) => pixelplus_core::calpattern::seed_from(rand::random()),
        Some(s) => s
            .as_u64()
            .filter(|s| (1..=u64::from(u32::MAX)).contains(s))
            .map(|s| s as u32)
            .ok_or_else(|| ApiError::bad_request("The seed must be a whole number from 1."))?,
    };
    let p = player(&state)?;
    p.send(PlayerCmd::CalibrateV2(seed)).await?;
    // Flash length follows this controller's refresh (2 slots at least).
    let slot_ms = p.status().refresh_hz.map_or(0.0, |hz| 1000.0 / hz.max(1.0));
    let plan = pixelplus_core::calpattern::schedule(seed, slot_ms);
    let mut out = serde_json::to_value(&plan).map_err(ApiError::internal)?;
    out["ok"] = json!(true);
    out["on"] = json!(true);
    // The pattern (and its sound) start with the next output frame.
    out["startsInMs"] = json!(0);
    Ok(Json(out))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SurpriseBody {
    /// "sequence" | "effect" (guessed from `ref` when missing).
    #[serde(default)]
    source: Option<String>,
    r#ref: String,
    #[serde(default)]
    target: Option<pixelplus_core::model::Target>,
    #[serde(default)]
    duration_ms: Option<u64>,
}

/// `POST /player/surprise {ref, source?, target?, durationMs?}`: show a
/// surprise now, without a trigger's gates (the trigger editor's "Try it").
async fn surprise(
    State(state): State<AppState>,
    Json(b): Json<SurpriseBody>,
) -> ApiResult<Json<Value>> {
    let show = state.store.get();
    let action = pixelplus_core::model::TriggerAction {
        kind: pixelplus_core::model::TriggerActionType::Surprise,
        r#ref: Some(b.r#ref),
        target: b.target,
        duration_ms: b.duration_ms,
        source: b.source,
    };
    let req = crate::services::triggers::surprise_request(&show, "test", &action)?;
    let started = player(&state)?.surprise(req).await?;
    Ok(Json(json!({ "ok": true, "surprise": started })))
}

/// `POST /player/surprise/stop`: end the running surprise at once.
async fn surprise_stop(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    player(&state)?.send(PlayerCmd::SurpriseStop).await?;
    Ok(ok())
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
        .route("/player/surprise", post(surprise))
        .route("/player/surprise/stop", post(surprise_stop))
}
