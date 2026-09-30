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
    let resp = games::command(state, cmd).await.map_err(ApiError::unavailable)?;
    if resp["ok"].as_bool() == Some(false) {
        let msg = resp["error"].as_str().unwrap_or("The games service couldn't do that right now.").to_string();
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

async fn test(State(state): State<AppState>, body: Bytes) -> ApiResult<Json<Value>> {
    let args: Value = body_or_default(&body)?;
    if let Some(pid) = args["propId"].as_str() {
        let show = state.store.get();
        let p = show.prop(pid).ok_or_else(|| ApiError::not_found("That prop"))?;
        if p.matrix.is_none() {
            return Err(ApiError::bad_request(format!("\"{}\" isn't set up as a matrix yet.", p.name)));
        }
        if show.settings.games.matrix_prop_id.as_deref() != Some(pid) {
            return Err(ApiError::bad_request(format!(
                "Choose \"{}\" as the game matrix in the games settings first, then run the test pattern.",
                p.name
            )));
        }
    }
    proxy(&state, json!({ "cmd": "test" })).await
}

async fn reload(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    proxy(&state, json!({ "cmd": "reload" })).await
}

async fn roms(State(state): State<AppState>) -> Json<Vec<Value>> {
    Json(games::list_roms(&state))
}

async fn upload_rom(State(state): State<AppState>, mut mp: Multipart) -> ApiResult<Json<Vec<Value>>> {
    let mut saved = 0;
    while let Some(field) = mp.next_field().await.map_err(multipart_error)? {
        let Some(fname) = field.file_name().map(str::to_string) else { continue };
        let name = games::sanitize_rom_name(&fname).ok_or_else(|| ApiError::bad_request("Please choose a .nes ROM file."))?;
        let bytes = field.bytes().await.map_err(multipart_error)?;
        if bytes.len() > games::ROM_MAX {
            return Err(ApiError::bad_request("That file is too large to be an NES ROM."));
        }
        if !games::looks_like_nes(&bytes) {
            return Err(ApiError::bad_request(format!("\"{fname}\" isn't an NES ROM (.nes files start with an iNES header).")));
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

async fn delete_rom(State(state): State<AppState>, Path(name): Path<String>) -> ApiResult<Json<Vec<Value>>> {
    let clean = games::sanitize_rom_name(&name).filter(|c| *c == name).ok_or_else(|| ApiError::not_found("That ROM"))?;
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
        .route("/games/test-pattern", post(test))
        .route("/games/reload", post(reload))
        .route("/games/roms", get(roms).post(upload_rom).layer(DefaultBodyLimit::max(games::ROM_MAX + 64 * 1024)))
        .route("/games/roms/{name}", delete(delete_rom))
}
