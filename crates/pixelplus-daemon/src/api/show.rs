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

/// Stands in for a stored secret in API responses. Sending it back unchanged
/// keeps the stored value.
pub const SECRET_PLACEHOLDER: &str = "********";

/// Remove secrets from settings sent to clients: the password hash becomes
/// `""` (so the UI knows one is set), SMTP / MQTT passwords become
/// [`SECRET_PLACEHOLDER`].
pub fn redact_settings(s: &mut ShowSettings) {
    if s.security.password_hash.is_some() {
        s.security.password_hash = Some(String::new());
    }
    if let Some(e) = s.alerts.email.as_mut() {
        if !e.password.is_empty() {
            e.password = SECRET_PLACEHOLDER.into();
        }
    }
    if s.mqtt.password.as_deref().is_some_and(|p| !p.is_empty()) {
        s.mqtt.password = Some(SECRET_PLACEHOLDER.into());
    }
}

/// In a settings patch, replace placeholders with the stored secrets.
pub fn restore_secrets(patch: &mut Value, current: &ShowSettings) {
    let placeholder = |v: Option<&Value>| v.and_then(Value::as_str) == Some(SECRET_PLACEHOLDER);
    if let Some(email) = patch.pointer_mut("/alerts/email").and_then(Value::as_object_mut) {
        if placeholder(email.get("password")) {
            let stored = current.alerts.email.as_ref().map(|e| e.password.clone()).unwrap_or_default();
            email.insert("password".into(), Value::String(stored));
        }
    }
    if let Some(mqtt) = patch.pointer_mut("/mqtt").and_then(Value::as_object_mut) {
        if placeholder(mqtt.get("password")) {
            let stored = current.mqtt.password.clone().map(Value::String).unwrap_or(Value::Null);
            mqtt.insert("password".into(), stored);
        }
    }
}

/// Allowed host names and trusted proxies must be well-formed.
fn validate_security(sec: &pixelplus_core::model::SecuritySettings) -> ApiResult<()> {
    if sec.allowed_hosts.len() > 32 || sec.trusted_proxies.len() > 32 {
        return Err(ApiError::bad_request("That's too many entries (32 at most)."));
    }
    for h in &sec.allowed_hosts {
        let name = h.trim().strip_prefix("*.").unwrap_or(h.trim());
        let ok = h.trim() == "*"
            || (!name.is_empty()
                && name.len() <= 253
                && name
                    .split('.')
                    .all(|l| !l.is_empty() && l.len() <= 63 && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')));
        if !ok {
            return Err(ApiError::bad_request(format!("“{h}” isn't a valid host name.")));
        }
    }
    for p in &sec.trusted_proxies {
        let (ip, bits) = p.trim().split_once('/').unwrap_or((p.trim(), "0"));
        if ip.parse::<std::net::IpAddr>().is_err() || bits.parse::<u8>().is_err() {
            return Err(ApiError::bad_request(format!(
                "“{p}” isn't an IP address or network (like 192.168.1.10 or 192.168.1.0/24)."
            )));
        }
    }
    Ok(())
}

async fn get_show(State(state): State<AppState>) -> Json<Arc<Show>> {
    // Never ship the password hash or other secrets to the browser.
    Json(Arc::new(super::content::public_show(&state.store.get())))
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
            // The UI password is changed through /auth/password only; the
            // other security settings (allowed hosts, trusted proxies) here.
            if let Some(sec) = patch.get_mut("security").and_then(Value::as_object_mut) {
                sec.remove("passwordHash");
            }
            restore_secrets(&mut patch, &s.settings);
            merge_patch(&mut value, &patch);
            let new: ShowSettings = serde_json::from_value(value)
                .map_err(|e| ApiError::bad_request(format!("Those settings aren't valid: {e}")))?;
            validate_security(&new.security)?;
            s.settings = new.clone();
            Ok(new)
        })
        .await?;
    let mut settings = settings;
    redact_settings(&mut settings);
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
