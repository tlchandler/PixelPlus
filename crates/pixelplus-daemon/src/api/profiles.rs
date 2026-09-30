//! Season profiles (F8, ARCHITECTURE §12.7).
//!
//! * `GET /profiles` → `ShowProfile[]`; `GET /profiles/:id`
//! * `POST /profiles {name, …}` → a new profile (empty schedule unless one is
//!   given); `POST /profiles/capture {name}` → a copy of the live season
//! * `PUT /profiles/:id` (JSON merge patch; `id` is kept), `DELETE /profiles/:id`
//! * `POST /profiles/:id/activate {saveCurrent = true}` → the new `Show`
//! * `GET /profiles/preview-switch/:id` → `{lines}` (what a switch changes)
//! * `GET /profiles/active` → the dashboard chip ([`ActiveSeason`])
//! * `PUT /profiles/auto-switch {enabled}` → `show.profileAutoSwitch`
//!
//! The logic lives in `services/profiles.rs`.

use super::crud::merge_patch;
use super::{ApiError, ApiResult};
use crate::services::profiles::{self as svc, ActiveSeason, SwitchReason};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use pixelplus_core::model::{Show, ShowProfile};
use serde::Deserialize;
use serde_json::{json, Value};

async fn list(State(state): State<AppState>) -> Json<Vec<ShowProfile>> {
    Json(state.store.get().profiles.clone())
}

async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ShowProfile>> {
    state
        .store
        .get()
        .profiles
        .iter()
        .find(|p| p.id == id)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::not_found("That season"))
}

fn room_for_one_more(show: &Show) -> ApiResult<()> {
    if show.profiles.len() >= svc::MAX_PROFILES {
        return Err(ApiError::bad_request(format!(
            "A show can keep {} seasons at most. Delete one first.",
            svc::MAX_PROFILES
        )));
    }
    Ok(())
}

/// Referenced playlists / looks must exist (dates and names are checked by
/// [`svc::validate`]).
fn check_new(show: &Show, p: &ShowProfile) -> ApiResult<()> {
    svc::validate(p)?;
    svc::check_references(show, p)
}

async fn create(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> ApiResult<Json<ShowProfile>> {
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let (p, _) = state
        .store
        .update(move |s| {
            room_for_one_more(s)?;
            let mut value = serde_json::to_value(svc::empty(&name)).map_err(ApiError::internal)?;
            let mut patch = body;
            if let Some(o) = patch.as_object_mut() {
                o.remove("id");
            }
            merge_patch(&mut value, &patch);
            let mut p: ShowProfile = serde_json::from_value(value)
                .map_err(|e| ApiError::bad_request(format!("That season isn't valid: {e}")))?;
            p.name = p.name.trim().to_string();
            check_new(s, &p)?;
            s.profiles.push(p.clone());
            Ok(p)
        })
        .await?;
    Ok(Json(p))
}

#[derive(Deserialize)]
struct CaptureBody {
    #[serde(default)]
    name: String,
}

async fn capture(
    State(state): State<AppState>,
    Json(body): Json<CaptureBody>,
) -> ApiResult<Json<ShowProfile>> {
    let (p, _) = state
        .store
        .update(move |s| {
            room_for_one_more(s)?;
            let name = if body.name.trim().is_empty() {
                "New season".to_string()
            } else {
                body.name
            };
            let p = svc::capture(s, &name);
            svc::validate(&p)?;
            s.profiles.push(p.clone());
            // The first season describes what's live: make it the active one.
            if s.active_profile_id.is_none() && s.profiles.len() == 1 {
                s.active_profile_id = Some(p.id.clone());
            }
            Ok(p)
        })
        .await?;
    Ok(Json(p))
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(patch): Json<Value>,
) -> ApiResult<Json<ShowProfile>> {
    let (p, _) = state
        .store
        .update(move |s| {
            let idx = s
                .profiles
                .iter()
                .position(|p| p.id == id)
                .ok_or_else(|| ApiError::not_found("That season"))?;
            let is_active = s.active_profile_id.as_deref() == Some(id.as_str());
            if is_active {
                // Keep edits made on the normal pages (schedule, requests, …).
                svc::save_live_into_active(s);
            }
            let mut value = serde_json::to_value(&s.profiles[idx]).map_err(ApiError::internal)?;
            merge_patch(&mut value, &patch);
            let mut p: ShowProfile = serde_json::from_value(value)
                .map_err(|e| ApiError::bad_request(format!("That season isn't valid: {e}")))?;
            p.id = id.clone();
            p.name = p.name.trim().to_string();
            svc::validate(&p)?;
            if is_active {
                svc::check_references(s, &p)?;
            }
            s.profiles[idx] = p.clone();
            if is_active {
                // Editing the live season edits the live show.
                svc::switch(s, &id, false)?;
            }
            Ok(p)
        })
        .await?;
    Ok(Json(p))
}

async fn remove(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    state
        .store
        .update(move |s| {
            let before = s.profiles.len();
            s.profiles.retain(|p| p.id != id);
            if s.profiles.len() == before {
                return Err(ApiError::not_found("That season"));
            }
            // The live settings stay as they are; they just aren't a season anymore.
            if s.active_profile_id.as_deref() == Some(id.as_str()) {
                s.active_profile_id = None;
            }
            Ok(())
        })
        .await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActivateBody {
    #[serde(default = "yes")]
    save_current: bool,
}

fn yes() -> bool {
    true
}

async fn activate(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<ActivateBody>>,
) -> ApiResult<Json<Show>> {
    let save = body.map(|b| b.save_current).unwrap_or(true);
    let show = svc::activate(&state, &id, save, SwitchReason::Manual).await?;
    Ok(Json(super::content::public_show(&show)))
}

async fn preview_switch(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let show = state.store.get();
    let p = show
        .profiles
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| ApiError::not_found("That season"))?;
    let mut lines = svc::diff(&show, p);
    if let Err(e) = svc::check_references(&show, p) {
        lines.insert(0, format!("Can't switch yet: {}", e.message));
    }
    Ok(Json(json!({ "lines": lines })))
}

async fn active(State(state): State<AppState>) -> Json<ActiveSeason> {
    let show = state.store.get();
    let today = chrono::Utc::now()
        .with_timezone(&svc::show_tz(&show))
        .date_naive();
    Json(svc::active_season(&show, today))
}

#[derive(Deserialize)]
struct AutoSwitchBody {
    enabled: bool,
}

async fn auto_switch(
    State(state): State<AppState>,
    Json(body): Json<AutoSwitchBody>,
) -> ApiResult<Json<Value>> {
    state
        .store
        .update(move |s| {
            if body.enabled && !s.profiles.iter().any(|p| p.date_range.is_some()) {
                return Err(ApiError::bad_request(
                    "Give at least one season its dates first.",
                ));
            }
            s.profile_auto_switch = body.enabled;
            Ok(())
        })
        .await?;
    Ok(Json(json!({ "enabled": body.enabled })))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        // xLights FPP Connect admin endpoints (F16, also WS6): merged here so
        // the shared `api/mod.rs` stays untouched.
        .merge(super::fppcompat::admin_routes())
        .route("/profiles", get(list).post(create))
        .route("/profiles/capture", post(capture))
        .route("/profiles/active", get(active))
        .route("/profiles/auto-switch", put(auto_switch))
        .route("/profiles/preview-switch/{id}", get(preview_switch))
        .route("/profiles/{id}/activate", post(activate))
        .route(
            "/profiles/{id}",
            get(get_one).put(update).patch(update).delete(remove),
        )
}

#[cfg(test)]
mod tests {
    use crate::api::testkit::TestApp;
    use axum::http::StatusCode;
    use serde_json::json;

    async fn with_playlists(app: &TestApp) -> (String, String) {
        let (_, a) = app
            .json("POST", "/playlists", Some(json!({"name": "Christmas Mix"})))
            .await;
        let (_, b) = app
            .json("POST", "/playlists", Some(json!({"name": "Halloween Mix"})))
            .await;
        (
            a["id"].as_str().unwrap().to_string(),
            b["id"].as_str().unwrap().to_string(),
        )
    }

    fn entry(pl: &str) -> serde_json::Value {
        json!({
            "id": "e1", "name": "Nightly", "enabled": true, "playlistId": pl,
            "days": ["mon","tue","wed","thu","fri","sat","sun"],
            "start": {"kind": "clock", "time": "17:00"},
            "end": {"kind": "clock", "time": "22:00"}
        })
    }

    #[tokio::test]
    async fn capture_create_switch_and_delete() {
        let app = TestApp::new();
        let (xmas_pl, hw_pl) = with_playlists(&app).await;
        let (st, _) = app
            .json(
                "PUT",
                "/schedule",
                Some(json!({"enabled": true, "entries": [entry(&xmas_pl)]})),
            )
            .await;
        assert_eq!(st, StatusCode::OK);
        let (st, xmas) = app
            .json(
                "POST",
                "/profiles/capture",
                Some(json!({"name": "Christmas"})),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{xmas}");
        assert_eq!(xmas["schedule"]["entries"].as_array().unwrap().len(), 1);
        let xmas_id = xmas["id"].as_str().unwrap().to_string();
        // The first captured season becomes the active one.
        let (_, show) = app.json("GET", "/show", None).await;
        assert_eq!(show["activeProfileId"], xmas_id.as_str());

        let (st, hw) = app
            .json(
                "POST",
                "/profiles",
                Some(json!({
                    "name": "Halloween", "icon": "🎃",
                    "dateRange": {"start": "10-01", "end": "10-31"},
                    "schedule": {"enabled": true, "entries": [entry(&hw_pl)]},
                    "requestsPlaylistId": hw_pl
                })),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{hw}");
        let hw_id = hw["id"].as_str().unwrap().to_string();

        let (st, v) = app
            .json("GET", &format!("/profiles/preview-switch/{hw_id}"), None)
            .await;
        assert_eq!(st, StatusCode::OK);
        assert!(v["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l.as_str().unwrap().starts_with("Song requests")));

        let (st, show) = app
            .json(
                "POST",
                &format!("/profiles/{hw_id}/activate"),
                Some(json!({})),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{show}");
        assert_eq!(show["activeProfileId"], hw_id.as_str());
        assert_eq!(show["schedule"]["entries"][0]["playlistId"], hw_pl.as_str());
        assert_eq!(show["settings"]["requests"]["playlistId"], hw_pl.as_str());

        let (_, chip) = app.json("GET", "/profiles/active", None).await;
        assert_eq!(chip["name"], "Halloween");
        assert_eq!(chip["icon"], "🎃");

        // Bad input.
        let (st, _) = app
            .json(
                "PUT",
                &format!("/profiles/{hw_id}"),
                Some(json!({"dateRange": {"start": "10-40", "end": "10-31"}})),
            )
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let (st, _) = app
            .json("POST", "/profiles/nope/activate", Some(json!({})))
            .await;
        assert_eq!(st, StatusCode::NOT_FOUND);

        // Editing the live season's schedule edits the live show.
        let (st, _) = app
            .json(
                "PUT",
                &format!("/profiles/{hw_id}"),
                Some(json!({"schedule": {"entries": []}})),
            )
            .await;
        assert_eq!(st, StatusCode::OK);
        let (_, sched) = app.json("GET", "/schedule", None).await;
        assert_eq!(sched["entries"].as_array().unwrap().len(), 0);

        // Auto-switch toggle.
        let (st, _) = app
            .json(
                "PUT",
                "/profiles/auto-switch",
                Some(json!({"enabled": true})),
            )
            .await;
        assert_eq!(st, StatusCode::OK);
        let (_, show) = app.json("GET", "/show", None).await;
        assert_eq!(show["profileAutoSwitch"], true);

        let (st, _) = app
            .json("DELETE", &format!("/profiles/{hw_id}"), None)
            .await;
        assert_eq!(st, StatusCode::OK);
        let (_, show) = app.json("GET", "/show", None).await;
        assert!(show.get("activeProfileId").is_none());
        let (_, list) = app.json("GET", "/profiles", None).await;
        assert_eq!(list.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn switch_is_journaled() {
        let app = TestApp::new();
        let (xmas_pl, _) = with_playlists(&app).await;
        let (_, a) = app
            .json("POST", "/profiles/capture", Some(json!({"name": "A"})))
            .await;
        let (_, b) = app
            .json(
                "POST",
                "/profiles",
                Some(json!({"name": "B", "requestsPlaylistId": xmas_pl})),
            )
            .await;
        crate::services::journal::start(&app.state);
        let (st, _) = app
            .json(
                "POST",
                &format!("/profiles/{}/activate", b["id"].as_str().unwrap()),
                Some(json!({"saveCurrent": true})),
            )
            .await;
        assert_eq!(st, StatusCode::OK);
        app.state.services.journal.flush().await;
        let (_, recs) = app.json("GET", "/journal?types=profileSwitch", None).await;
        assert_eq!(recs[0]["to"], "B");
        assert_eq!(recs[0]["from"], "A");
        let _ = a;
    }
}
