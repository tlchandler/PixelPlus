//! "Measure with my phone" results (F1, ARCHITECTURE §12.1).
//!
//! The phone measures everything itself (camera and microphone never leave
//! it) and posts only numbers:
//!
//! * `POST /calibration/result {residualMs, spreadMs, matches, device?, apply}`
//!   → `{outputDelayMs, previousDelayMs, calibration, clamped}`. With
//!   `apply`, `settings.audio.outputDelayMs += round(residualMs)` (clamped to
//!   −500…2000) and the result is kept as `settings.audio.lastCalibration`;
//!   this goes through the show store, so followers follow at once.
//! * `POST /calibration/undo {outputDelayMs, lastCalibration?}` puts back what
//!   the page saw before it applied a result.
//!
//! Starting the flash/click pattern itself is `POST /player/calibration
//! {on:true, pattern:"v2"}` (player API; see `pixelplus_core::calpattern`).

use super::{ApiError, ApiResult};
use crate::state::AppState;
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use pixelplus_core::model::{AudioCalibration, CalibrationMethod, OUTPUT_DELAY_RANGE_MS};
use serde::Deserialize;
use serde_json::{json, Value};

/// Largest correction the phone may report (the whole delay range plus slack).
const MAX_RESIDUAL_MS: f32 = 3000.0;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResultBody {
    residual_ms: f32,
    #[serde(default)]
    spread_ms: f32,
    #[serde(default)]
    matches: u32,
    #[serde(default)]
    device: Option<String>,
    #[serde(default)]
    apply: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UndoBody {
    output_delay_ms: i32,
    #[serde(default)]
    last_calibration: Option<AudioCalibration>,
}

fn not_on_follower(state: &AppState) -> ApiResult<()> {
    if state.identity().role == crate::node::LocalRole::Follower {
        return Err(ApiError::bad_request(
            "This controller follows its show leader; measure the sound delay on the leader.",
        ));
    }
    Ok(())
}

/// A readable phone model: printable, at most 80 characters.
fn clean_device(d: Option<String>) -> Option<String> {
    let d: String = d?
        .chars()
        .filter(|c| !c.is_control())
        .take(80)
        .collect::<String>()
        .trim()
        .to_string();
    (!d.is_empty()).then_some(d)
}

/// The delay after applying `residual` to `current`, and whether it hit a limit.
pub fn corrected_delay(current: i32, residual_ms: f32) -> (i32, bool) {
    let want = current as i64 + residual_ms.round() as i64;
    let lo = *OUTPUT_DELAY_RANGE_MS.start() as i64;
    let hi = *OUTPUT_DELAY_RANGE_MS.end() as i64;
    let clamped = want.clamp(lo, hi);
    (clamped as i32, clamped != want)
}

async fn result(
    State(state): State<AppState>,
    Json(b): Json<ResultBody>,
) -> ApiResult<Json<Value>> {
    not_on_follower(&state)?;
    if !b.residual_ms.is_finite() || b.residual_ms.abs() > MAX_RESIDUAL_MS {
        return Err(ApiError::bad_request(
            "That measurement is out of range; please measure again.",
        ));
    }
    if !b.spread_ms.is_finite() || !(0.0..=1000.0).contains(&b.spread_ms) {
        return Err(ApiError::bad_request(
            "spreadMs must be between 0 and 1000.",
        ));
    }
    let matches = b.matches.min(10_000);
    let device = clean_device(b.device);
    let previous = state.store.get().settings.audio.clone();
    let (next, clamped) = corrected_delay(previous.output_delay_ms, b.residual_ms);
    let calibration = AudioCalibration {
        measured_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        method: CalibrationMethod::Phone,
        residual_ms: (b.residual_ms * 10.0).round() / 10.0,
        spread_ms: (b.spread_ms * 10.0).round() / 10.0,
        matches,
        applied_delay_ms: if b.apply {
            next
        } else {
            previous.output_delay_ms
        },
        device,
    };
    let output_delay_ms = if b.apply {
        let cal = calibration.clone();
        state
            .store
            .update(move |s| {
                s.settings.audio.output_delay_ms = next;
                s.settings.audio.last_calibration = Some(cal);
                Ok(())
            })
            .await?;
        tracing::info!(
            "Sound delay measured with a phone: {:+.1} ms (spread {:.1} ms, {} matches); now {} ms",
            b.residual_ms,
            b.spread_ms,
            matches,
            next
        );
        next
    } else {
        previous.output_delay_ms
    };
    Ok(Json(json!({
        "outputDelayMs": output_delay_ms,
        "previousDelayMs": previous.output_delay_ms,
        "previousCalibration": previous.last_calibration,
        "calibration": calibration,
        "clamped": clamped,
    })))
}

async fn undo(State(state): State<AppState>, Json(b): Json<UndoBody>) -> ApiResult<Json<Value>> {
    not_on_follower(&state)?;
    if !OUTPUT_DELAY_RANGE_MS.contains(&b.output_delay_ms) {
        return Err(ApiError::bad_request(format!(
            "The sound delay must be between {} and {} ms.",
            OUTPUT_DELAY_RANGE_MS.start(),
            OUTPUT_DELAY_RANGE_MS.end()
        )));
    }
    let delay = b.output_delay_ms;
    let last = b.last_calibration;
    state
        .store
        .update(move |s| {
            s.settings.audio.output_delay_ms = delay;
            s.settings.audio.last_calibration = last;
            Ok(())
        })
        .await?;
    Ok(Json(json!({ "outputDelayMs": delay })))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/calibration/result", post(result))
        .route("/calibration/undo", post(undo))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::testkit::TestApp;
    use axum::http::StatusCode;

    #[test]
    fn delay_is_clamped() {
        assert_eq!(corrected_delay(100, 112.4), (212, false));
        assert_eq!(corrected_delay(100, -12.6), (87, false));
        assert_eq!(corrected_delay(1900, 400.0), (2000, true));
        assert_eq!(corrected_delay(-400, -300.0), (-500, true));
    }

    #[tokio::test]
    async fn apply_then_undo() {
        let app = TestApp::new();
        let body = json!({"residualMs": 212.3, "spreadMs": 3.2, "matches": 30, "device": "Pixel 8\u{7}", "apply": true});
        let (st, v) = app.json("POST", "/calibration/result", Some(body)).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(v["outputDelayMs"], 212);
        assert_eq!(v["previousDelayMs"], 0);
        assert_eq!(v["clamped"], false);
        assert_eq!(v["calibration"]["method"], "phone");
        assert_eq!(v["calibration"]["device"], "Pixel 8");
        let audio = app.state.store.get().settings.audio.clone();
        assert_eq!(audio.output_delay_ms, 212);
        let cal = audio.last_calibration.unwrap();
        assert_eq!(cal.applied_delay_ms, 212);
        assert_eq!(cal.matches, 30);

        // A second measurement adds to the current delay.
        let (_, v) = app
            .json(
                "POST",
                "/calibration/result",
                Some(json!({"residualMs": -4.6, "spreadMs": 2.0, "matches": 31, "apply": true})),
            )
            .await;
        assert_eq!(v["outputDelayMs"], 207);
        assert_eq!(v["previousCalibration"]["appliedDelayMs"], 212);

        // Undo restores what was there.
        let prev = v["previousCalibration"].clone();
        let (st, _) = app
            .json(
                "POST",
                "/calibration/undo",
                Some(json!({"outputDelayMs": 212, "lastCalibration": prev})),
            )
            .await;
        assert_eq!(st, StatusCode::OK);
        let audio = app.state.store.get().settings.audio.clone();
        assert_eq!(audio.output_delay_ms, 212);
        assert_eq!(audio.last_calibration.unwrap().residual_ms, 212.3);
    }

    #[tokio::test]
    async fn dry_run_and_bad_input() {
        let app = TestApp::new();
        let (st, v) = app
            .json(
                "POST",
                "/calibration/result",
                Some(json!({"residualMs": 50.0, "spreadMs": 4.0, "matches": 25, "apply": false})),
            )
            .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["outputDelayMs"], 0);
        assert_eq!(v["calibration"]["appliedDelayMs"], 0);
        assert_eq!(app.state.store.get().settings.audio.output_delay_ms, 0);
        assert!(app
            .state
            .store
            .get()
            .settings
            .audio
            .last_calibration
            .is_none());

        for bad in [
            json!({"residualMs": 5000.0, "apply": true}),
            json!({"residualMs": 5.0, "spreadMs": -1.0, "apply": true}),
            json!({"spreadMs": 1.0}),
        ] {
            let (st, _) = app
                .json("POST", "/calibration/result", Some(bad.clone()))
                .await;
            assert!(st.is_client_error(), "{bad}: {st}");
        }
        let (st, _) = app
            .json(
                "POST",
                "/calibration/undo",
                Some(json!({"outputDelayMs": 9000})),
            )
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }
}
