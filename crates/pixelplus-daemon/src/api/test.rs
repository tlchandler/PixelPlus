//! Test patterns (`/test/*`) and the fault finder (`/faultfinder/*`).
//!
//! The fault finder drives the prop's pixels through the overlay path: every
//! 50 ms it renders `FaultFinder::render` and sends it as a
//! `PlayerCmd::Overlay(PropPixels)` until the session is stopped.

use super::content::player;
use super::playerapi::body_or_default;
use super::{ApiError, ApiResult};
use crate::player::{OverlayCmd, PlayerCmd, TestRequest};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use parking_lot::Mutex;
use pixelplus_core::faultfinder::FaultFinder;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

const TEST_MODES: &[&str] = &["solid", "chase", "rgbCycle", "countPixels", "walk", "effect"];

async fn test_start(State(state): State<AppState>, Json(req): Json<TestRequest>) -> ApiResult<Json<Value>> {
    if !TEST_MODES.contains(&req.mode.as_str()) {
        return Err(ApiError::bad_request(format!("Unknown test pattern \"{}\".", req.mode)));
    }
    let show = state.store.get();
    let t = &req.target;
    if let Some(node) = &t.node_id {
        let n = show.node(node).ok_or_else(|| ApiError::not_found("That controller"))?;
        if let Some(o) = t.output {
            if o == 0 || o as usize > n.outputs.len() {
                return Err(ApiError::bad_request(format!("{} doesn't have output {o}.", n.name)));
            }
        }
    } else {
        for id in &t.props.prop_ids {
            show.prop(id).ok_or_else(|| ApiError::not_found("That prop"))?;
        }
        if !t.props.all && t.props.prop_ids.is_empty() && t.props.group_ids.is_empty() {
            return Err(ApiError::bad_request("Pick which props (or which controller output) to test."));
        }
    }
    if let Some(c) = &req.color {
        if pixelplus_core::effects::Rgb::from_hex(c).is_none() {
            return Err(ApiError::bad_request(format!("\"{c}\" isn't a colour (use #rrggbb).")));
        }
    }
    if req.mode == "effect" && req.effect.is_none() {
        return Err(ApiError::bad_request("Pick a look to test."));
    }
    player(&state)?.test_start(req).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn test_stop(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    stop_fault_session(&state).await;
    player(&state)?.send(PlayerCmd::TestStop).await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Fault finder
// ---------------------------------------------------------------------------

pub struct FaultSession {
    pub id: String,
    pub prop_id: String,
    finder: Arc<Mutex<FaultFinder>>,
    task: tokio::task::JoinHandle<()>,
}

/// One running fault-finder session (per daemon).
#[derive(Default)]
pub struct FaultState {
    session: Mutex<Option<FaultSession>>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FaultStep {
    pub session: String,
    pub prop_id: String,
    pub pixel_count: u32,
    /// Lit range, 0-based [litFrom, litTo).
    pub lit_from: u32,
    pub lit_to: u32,
    pub step: u32,
    pub total_steps: u32,
    pub question: String,
    pub done: bool,
    pub can_undo: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}

fn step_of(session: &str, prop_id: &str, ff: &FaultFinder) -> FaultStep {
    let given = ff.answers_given();
    match (ff.current_step(), ff.result()) {
        (Some(s), _) => FaultStep {
            session: session.into(),
            prop_id: prop_id.into(),
            pixel_count: ff.pixel_count(),
            lit_from: s.lit_range.start,
            lit_to: s.lit_range.end,
            step: s.number,
            total_steps: given + s.max_remaining,
            question: s.question,
            done: false,
            can_undo: given > 0,
            result: None,
        },
        (None, Some(r)) => FaultStep {
            session: session.into(),
            prop_id: prop_id.into(),
            pixel_count: ff.pixel_count(),
            lit_from: 0,
            lit_to: 0,
            step: given,
            total_steps: given,
            question: String::new(),
            done: true,
            can_undo: given > 0,
            result: Some(json!({ "pixelIndex": r.first_bad_pixel, "message": r.message })),
        },
        (None, None) => unreachable!("fault finder is either asking or done"),
    }
}

pub async fn stop_fault_session(state: &AppState) {
    let old = state.services.faults.session.lock().take();
    if let Some(s) = old {
        s.task.abort();
        if let Some(p) = state.services.player.get() {
            let _ = p.send(PlayerCmd::Overlay(OverlayCmd::Enable { prop_id: s.prop_id.clone(), enabled: false })).await;
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartBody {
    prop_id: String,
}

async fn fault_start(State(state): State<AppState>, Json(b): Json<StartBody>) -> ApiResult<Json<FaultStep>> {
    let show = state.store.get();
    let prop = show.prop(&b.prop_id).ok_or_else(|| ApiError::not_found("That prop"))?.clone();
    if prop.pixel_count == 0 {
        return Err(ApiError::bad_request("This prop has no pixels to test."));
    }
    if prop.segments.is_empty() {
        return Err(ApiError::bad_request(format!(
            "\"{}\" isn't wired to a controller port yet. Set its wiring first.",
            prop.name
        )));
    }
    let p = player(&state)?.clone();
    stop_fault_session(&state).await;
    p.send(PlayerCmd::Overlay(OverlayCmd::Enable { prop_id: prop.id.clone(), enabled: true })).await?;
    let finder = Arc::new(Mutex::new(FaultFinder::new(prop.pixel_count)));
    let id = format!("ff{}", pixelplus_core::model::new_id());
    let task = {
        let finder = finder.clone();
        let prop_id = prop.id.clone();
        let n = prop.pixel_count as usize * 3;
        tokio::spawn(async move {
            let started = std::time::Instant::now();
            let mut tick = tokio::time::interval(Duration::from_millis(50));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                let mut rgb = vec![0u8; n];
                finder.lock().render(started.elapsed().as_millis() as u64, &mut rgb);
                if p.send(PlayerCmd::Overlay(OverlayCmd::PropPixels { prop_id: prop_id.clone(), rgb })).await.is_err() {
                    break;
                }
            }
        })
    };
    let step = step_of(&id, &prop.id, &finder.lock());
    *state.services.faults.session.lock() = Some(FaultSession { id, prop_id: prop.id.clone(), finder, task });
    Ok(Json(step))
}

fn with_session<R>(state: &AppState, session: &str, f: impl FnOnce(&FaultSession) -> R) -> ApiResult<R> {
    let guard = state.services.faults.session.lock();
    match guard.as_ref() {
        Some(s) if s.id == session => Ok(f(s)),
        _ => Err(ApiError::new(
            axum::http::StatusCode::CONFLICT,
            "no_session",
            "That fault finder session has ended. Start it again from the prop.",
        )),
    }
}

#[derive(Deserialize, Default)]
struct AnswerBody {
    #[serde(default)]
    ok: Option<bool>,
    #[serde(default)]
    lit: Option<bool>,
}

async fn fault_answer(State(state): State<AppState>, Path(session): Path<String>, Json(b): Json<AnswerBody>) -> ApiResult<Json<FaultStep>> {
    let ok = b.ok.or(b.lit).ok_or_else(|| ApiError::bad_request("Answer with {\"ok\": true} or {\"ok\": false}."))?;
    with_session(&state, &session, |s| {
        let mut ff = s.finder.lock();
        ff.answer(ok);
        step_of(&s.id, &s.prop_id, &ff)
    })
    .map(Json)
}

async fn fault_undo(State(state): State<AppState>, Path(session): Path<String>) -> ApiResult<Json<FaultStep>> {
    with_session(&state, &session, |s| {
        let mut ff = s.finder.lock();
        ff.undo();
        step_of(&s.id, &s.prop_id, &ff)
    })
    .map(Json)
}

async fn fault_stop(State(state): State<AppState>, _body: Bytes) -> ApiResult<Json<Value>> {
    stop_fault_session(&state).await;
    Ok(Json(json!({ "ok": true })))
}

async fn fault_stop_session(State(state): State<AppState>, Path(_session): Path<String>, body: Bytes) -> ApiResult<Json<Value>> {
    let _: Value = body_or_default(&body)?;
    stop_fault_session(&state).await;
    Ok(Json(json!({ "ok": true })))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/test/start", post(test_start))
        .route("/test/stop", post(test_stop))
        .route("/faultfinder/start", post(fault_start))
        .route("/faultfinder/stop", post(fault_stop))
        .route("/faultfinder/{session}/answer", post(fault_answer))
        .route("/faultfinder/{session}/undo", post(fault_undo))
        .route("/faultfinder/{session}/stop", post(fault_stop_session))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_until_done() {
        let mut ff = FaultFinder::new(10);
        let s = step_of("x", "p", &ff);
        assert!(!s.done);
        assert_eq!(s.lit_from, 0);
        assert!(s.lit_to > 0 && s.lit_to <= 10);
        assert!(!s.can_undo);
        for _ in 0..10 {
            if ff.is_done() {
                break;
            }
            ff.answer(true);
        }
        let s = step_of("x", "p", &ff);
        assert!(s.done);
        assert_eq!(s.result.unwrap()["pixelIndex"], Value::Null);
    }
}
