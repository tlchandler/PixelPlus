//! Games sidecar control (proxied to its unix socket) and ROM management.
//! Game settings themselves live in `show.settings.games` (`PUT /show/settings`).

use super::content::multipart_error;
use super::playerapi::body_or_default;
use super::{ApiError, ApiResult};
use crate::services::games;
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

async fn status(State(state): State<AppState>) -> Json<Value> {
    Json(games::status(&state).await)
}

/// Send a command; map "not running" to 503 and `{ok:false}` to 409.
async fn proxy(state: &AppState, cmd: Value) -> ApiResult<Json<Value>> {
    let resp = games::command(state, cmd)
        .await
        .map_err(ApiError::unavailable)?;
    if resp["ok"].as_bool() == Some(false) {
        let msg = resp["error"]
            .as_str()
            .unwrap_or("The games service couldn't do that right now.")
            .to_string();
        return Err(ApiError::new(StatusCode::CONFLICT, "games_busy", msg));
    }
    Ok(Json(resp))
}

async fn invite(State(state): State<AppState>, body: Bytes) -> ApiResult<Json<Value>> {
    let args: Value = body_or_default(&body)?;
    let mut cmd = json!({ "cmd": "invite" });
    for k in ["flashes", "style", "url", "force"] {
        if let Some(v) = args.get(k).filter(|v| !v.is_null()) {
            cmd[k] = v.clone();
        }
    }
    proxy(&state, cmd).await
}

async fn stop(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    proxy(&state, json!({ "cmd": "stop" })).await
}

async fn test(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    proxy(&state, json!({ "cmd": "test" })).await
}

/// Grid used to check a matrix's size and orientation: blue border, red
/// top-left, green top-right, white bottom-left corner pixel.
pub(crate) fn test_pattern_grid(w: u32, h: u32) -> pixelplus_core::text::RgbGrid {
    use pixelplus_core::effects::Rgb;
    let mut g = pixelplus_core::text::RgbGrid::new(w, h);
    let blue = Rgb::new(0, 0, 160);
    for x in 0..w as i64 {
        g.set(x, 0, blue);
        g.set(x, h as i64 - 1, blue);
    }
    for y in 0..h as i64 {
        g.set(0, y, blue);
        g.set(w as i64 - 1, y, blue);
    }
    g.set(0, 0, Rgb::new(255, 0, 0));
    g.set(w as i64 - 1, 0, Rgb::new(0, 255, 0));
    g.set(0, h as i64 - 1, Rgb::new(255, 255, 255));
    g
}

/// `POST /games/test-pattern {propId}`: show the orientation pattern on a
/// matrix prop for 8 seconds (drawn by the daemon; the games service isn't needed).
async fn test_pattern(State(state): State<AppState>, body: Bytes) -> ApiResult<Json<Value>> {
    let args: Value = body_or_default(&body)?;
    let show = state.store.get();
    let pid = args["propId"]
        .as_str()
        .map(str::to_string)
        .or_else(|| show.settings.games.matrix_prop_id.clone())
        .ok_or_else(|| ApiError::bad_request("Choose the matrix prop first."))?;
    let p = show
        .prop(&pid)
        .ok_or_else(|| ApiError::not_found("That prop"))?
        .clone();
    let m = p.matrix.clone().ok_or_else(|| {
        ApiError::bad_request(format!(
            "\"{}\" isn't set up as a matrix yet. Set its width and height first.",
            p.name
        ))
    })?;
    let rgb = pixelplus_core::text::grid_to_prop_pixels(
        &test_pattern_grid(m.width, m.height),
        &m,
        p.pixel_count as usize,
    );
    let player = super::content::player(&state)?.clone();
    use crate::player::{OverlayCmd, PlayerCmd};
    player
        .send(PlayerCmd::Overlay(OverlayCmd::Enable {
            prop_id: pid.clone(),
            enabled: true,
        }))
        .await?;
    player
        .send(PlayerCmd::Overlay(OverlayCmd::PropPixels {
            prop_id: pid.clone(),
            rgb: rgb.clone(),
        }))
        .await?;
    tokio::spawn(async move {
        // Refresh a few times (in case the player drops idle overlays), then clear.
        for _ in 0..8 {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let _ = player
                .send(PlayerCmd::Overlay(OverlayCmd::PropPixels {
                    prop_id: pid.clone(),
                    rgb: rgb.clone(),
                }))
                .await;
        }
        let _ = player
            .send(PlayerCmd::Overlay(OverlayCmd::Enable {
                prop_id: pid,
                enabled: false,
            }))
            .await;
    });
    Ok(Json(
        json!({ "ok": true, "message": format!("Test pattern on {} ({}×{}): blue border, red top-left, green top-right, white bottom-left.", p.name, m.width, m.height) }),
    ))
}

async fn reload(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    proxy(&state, json!({ "cmd": "reload" })).await
}

async fn roms(State(state): State<AppState>) -> Json<Vec<Value>> {
    Json(games::list_roms(&state))
}

async fn upload_rom(
    State(state): State<AppState>,
    mut mp: Multipart,
) -> ApiResult<Json<Vec<Value>>> {
    let mut saved = 0;
    while let Some(field) = mp.next_field().await.map_err(multipart_error)? {
        let Some(fname) = field.file_name().map(str::to_string) else {
            continue;
        };
        let name = games::sanitize_rom_name(&fname)
            .ok_or_else(|| ApiError::bad_request("Please choose a .nes ROM file."))?;
        let bytes = field.bytes().await.map_err(multipart_error)?;
        if bytes.len() > games::ROM_MAX {
            return Err(ApiError::bad_request(
                "That file is too large to be an NES ROM.",
            ));
        }
        if !games::looks_like_nes(&bytes) {
            return Err(ApiError::bad_request(format!(
                "\"{fname}\" isn't an NES ROM (.nes files start with an iNES header)."
            )));
        }
        let dir = games::roms_dir(&state);
        tokio::fs::create_dir_all(&dir).await?;
        let tmp = dir.join(format!(".{name}.tmp"));
        tokio::fs::write(&tmp, &bytes).await?;
        tokio::fs::rename(&tmp, dir.join(&name)).await?;
        saved += 1;
    }
    if saved == 0 {
        return Err(ApiError::bad_request("Please choose a .nes ROM file."));
    }
    let _ = games::command(&state, json!({ "cmd": "reload" })).await;
    Ok(Json(games::list_roms(&state)))
}

async fn delete_rom(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<Vec<Value>>> {
    let clean = games::sanitize_rom_name(&name)
        .filter(|c| *c == name)
        .ok_or_else(|| ApiError::not_found("That ROM"))?;
    let path = games::roms_dir(&state).join(&clean);
    if !path.is_file() {
        return Err(ApiError::not_found("That ROM"));
    }
    tokio::fs::remove_file(&path).await?;
    let _ = games::command(&state, json!({ "cmd": "reload" })).await;
    Ok(Json(games::list_roms(&state)))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/games/status", get(status))
        .route("/games/invite", post(invite))
        .route("/games/stop", post(stop))
        .route("/games/test", post(test))
        .route("/games/test-pattern", post(test_pattern))
        .route("/games/reload", post(reload))
        .route(
            "/games/roms",
            get(roms)
                .post(upload_rom)
                .layer(DefaultBodyLimit::max(games::ROM_MAX + 64 * 1024)),
        )
        .route("/games/roms/{name}", delete(delete_rom))
}
