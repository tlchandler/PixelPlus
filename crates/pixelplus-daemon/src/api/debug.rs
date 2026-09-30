//! Development diagnostics (`/debug/*`), available only with `PIXELPLUS_DEV=1`
//! or the simulated output (`PIXELPLUS_OUTPUT=sim`). Requires sign-in like the
//! rest of the API when a password is set.

use super::{ApiError, ApiResult};
use crate::player::debugtap;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize, Default)]
struct OutputQuery {
    /// Comma-separated 1-based outputs to include (default: all).
    #[serde(default)]
    outputs: Option<String>,
}

/// `GET /debug/output`: the last frame written to this controller's outputs
/// `{frameNo, atMs, wallMs, sequence: {id, frame} | null, posMs, lightWallMs, master, player, outputs: [{index, pixels, rgb, wire}]}`
/// (`posMs`: the unquantised timeline position the frame was chosen for, valid at
/// `lightWallMs`, the wall-clock time the frame lights up; compare nodes with
/// `posMs − lightWallMs`)
/// with `rgb` (rendered, colour order not applied) and `wire` (what the output
/// backend received) base64-encoded.
async fn output(
    State(state): State<AppState>,
    Query(q): Query<OutputQuery>,
) -> ApiResult<Json<Value>> {
    let Some(tap) = state.services.debug_output.get() else {
        return Err(ApiError::not_found(
            "The output tap (only with PIXELPLUS_DEV=1 or PIXELPLUS_OUTPUT=sim)",
        ));
    };
    let frame = tap.snapshot();
    let want: Option<Vec<usize>> = q
        .outputs
        .as_deref()
        .map(|s| s.split(',').filter_map(|v| v.trim().parse().ok()).collect());
    let outputs: Vec<_> = debugtap::outputs(&frame)
        .into_iter()
        .filter(|o| want.as_ref().map_or(true, |w| w.contains(&o.index)))
        .collect();
    let player = state.services.player.get().map(|p| p.status());
    Ok(Json(json!({
        "nodeId": state.identity().id,
        "frameNo": frame.frame_no,
        "atMs": frame.at_ms,
        "wallMs": frame.wall_ms,
        "sequence": frame.sequence.as_ref().map(|(id, f)| json!({"id": id, "frame": f})),
        "posMs": frame.pos_ms,
        "lightWallMs": frame.light_wall_ms,
        "master": frame.master,
        "player": player,
        "outputs": outputs,
    })))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/debug/output", get(output))
}
