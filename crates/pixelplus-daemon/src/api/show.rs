//! Whole-show endpoints: `GET /show`, settings, schedule, pronunciations.

use super::crud::merge_patch;
use super::{ApiError, ApiResult};
use crate::state::AppState;
use axum::extract::State;
use axum::routing::{get, put};
use axum::{Json, Router};
use pixelplus_core::model::{Pronunciation, Schedule, Show, ShowSettings};
use serde_json::Value;
use std::sync::Arc;

async fn get_show(State(state): State<AppState>) -> Json<Arc<Show>> {
    let mut show = state.store.get();
    // Never ship the password hash to the browser.
    if show.settings.security.password_hash.is_some() {
        let mut s = (*show).clone();
        s.settings.security.password_hash = Some(String::new());
        show = Arc::new(s);
    }
    Json(show)
}

#[derive(serde::Deserialize)]
struct NameBody {
    name: String,
}

async fn put_name(State(state): State<AppState>, Json(body): Json<NameBody>) -> ApiResult<Json<Show>> {
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::bad_request("Please give your show a name."));
    }
    if name.chars().count() > 120 {
        return Err(ApiError::bad_request("That name is too long (120 characters max)."));
    }
    let (_, show) = state
        .store
        .update(|s| {
            s.name = name;
            Ok(())
        })
        .await?;
    // The UI expects the updated show (without the password hash).
    Ok(Json(super::content::public_show(&show)))
}

async fn put_settings(State(state): State<AppState>, Json(patch): Json<Value>) -> ApiResult<Json<ShowSettings>> {
    let (settings, _) = state
        .store
        .update(|s| {
            let mut value = serde_json::to_value(&s.settings).map_err(ApiError::internal)?;
            let mut patch = patch;
            // The password is changed through /auth/password only.
            if let Value::Object(p) = &mut patch {
                p.remove("security");
            }
            merge_patch(&mut value, &patch);
            let new: ShowSettings = serde_json::from_value(value)
                .map_err(|e| ApiError::bad_request(format!("Those settings aren't valid: {e}")))?;
            s.settings = new.clone();
            Ok(new)
        })
        .await?;
    let mut settings = settings;
    if settings.security.password_hash.is_some() {
        settings.security.password_hash = Some(String::new());
    }
    Ok(Json(settings))
}

async fn get_schedule(State(state): State<AppState>) -> Json<Schedule> {
    Json(state.store.get().schedule.clone())
}

async fn put_schedule(State(state): State<AppState>, Json(patch): Json<Value>) -> ApiResult<Json<Schedule>> {
    let (schedule, _) = state
        .store
        .update(|s| {
            let mut value = serde_json::to_value(&s.schedule).map_err(ApiError::internal)?;
            merge_patch(&mut value, &patch);
            let new: Schedule = serde_json::from_value(value)
                .map_err(|e| ApiError::bad_request(format!("That schedule isn't valid: {e}")))?;
            for e in &new.entries {
                if s.playlist(&e.playlist_id).is_none() {
                    return Err(ApiError::bad_request(format!(
                        "\"{}\" needs a playlist to play.",
                        e.name
                    )));
                }
            }
            s.schedule = new.clone();
            Ok(new)
        })
        .await?;
    Ok(Json(schedule))
}

async fn get_pronunciations(State(state): State<AppState>) -> Json<Vec<Pronunciation>> {
    Json(state.store.get().pronunciations.clone())
}

async fn put_pronunciations(
    State(state): State<AppState>,
    Json(list): Json<Vec<Pronunciation>>,
) -> ApiResult<Json<Vec<Pronunciation>>> {
    let list: Vec<Pronunciation> = list
        .into_iter()
        .filter(|p| !p.word.trim().is_empty() && !p.say.trim().is_empty())
        .collect();
    state
        .store
        .update(|s| {
            s.pronunciations = list.clone();
            Ok(())
        })
        .await?;
    Ok(Json(list))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/show", get(get_show))
        .route("/show/name", put(put_name))
        .route("/show/settings", put(put_settings).patch(put_settings))
        .route("/schedule", get(get_schedule).put(put_schedule))
        .route("/pronunciations", get(get_pronunciations).put(put_pronunciations))
}
