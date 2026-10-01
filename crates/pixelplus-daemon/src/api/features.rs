//! Feature toggles (Settings → Features, ARCHITECTURE §12.17).
//!
//! * `GET /features` → the catalogue with each feature's state.
//! * `PUT /features {disabled: [...]}` (a whole preset) or
//!   `PUT /features {id, enabled}` (one switch; what it needs / what needs it
//!   follows) → the new state plus `changed`.
//! * [`guard`]: every API endpoint of a feature that is off answers
//!   `feature_disabled` — 409 with a friendly message for the admin UI, 404
//!   for public pages (song requests, the CA certificate) so visitors see
//!   nothing there. The xLights (FPP Connect) root paths use [`root_guard`].
//!
//! Services react to the change through the show store (see
//! `services::features`).

use super::{ApiError, ApiResult};
use crate::state::AppState;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use pixelplus_core::features::{FeatureId, FeatureSettings};
use serde::Deserialize;
use serde_json::{json, Value};

/// The `feature_disabled` answer for `id`.
pub fn disabled_error(id: FeatureId, public: bool) -> ApiError {
    if public {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "feature_disabled",
            "That page isn't available.",
        )
    } else {
        ApiError::new(StatusCode::CONFLICT, "feature_disabled", id.off_message())
    }
}

/// `Err(feature_disabled)` unless `id` is on (for handlers and services).
pub fn require(state: &AppState, id: FeatureId) -> ApiResult<()> {
    if state.store.get().feature(id) {
        Ok(())
    } else {
        Err(disabled_error(id, false))
    }
}

/// Which feature an API path (relative to `/api/v1`) belongs to. `None`:
/// always available (core pages, settings, updates, backups…).
pub fn feature_for(method: &Method, path: &str) -> Option<FeatureId> {
    use FeatureId::*;
    let path = path.strip_prefix("/api/v1").unwrap_or(path);
    let seg: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let first = seg.first().copied().unwrap_or("");
    let second = seg.get(1).copied();
    let third = seg.get(2).copied();
    let read = method == Method::GET || method == Method::HEAD;
    Some(match (first, second, third) {
        ("dj-clips" | "dj-voices" | "tts" | "pronunciations", _, _) => Dj,
        // Custom looks: the list stays readable (idle looks, playlists), the
        // editor's writes and live tries are the feature.
        ("effects", Some("preview-apply"), _) => Effects,
        ("effects", None | Some(_), _)
            if !read && !matches!(second, Some("builtin" | "catalog" | "schema")) =>
        {
            Effects
        }
        ("autoshow" | "jobs", _, _) => AutoShows,
        ("media", Some(_), Some("analyze" | "analysis")) => AutoShows,
        ("sequences", Some(_), Some("regenerate")) => AutoShows,
        ("sequences", Some("tags"), None) => SmartPlaylists,
        ("library", _, _) => SmartPlaylists,
        ("playlists", Some(_), Some("preview")) => SmartPlaylists,
        ("sequences", Some(_), Some("preview")) => Layout,
        ("profiles", _, _) => Seasons,
        ("requests", _, _) | ("public", Some("requests"), _) => Requests,
        ("games", _, _) => Games,
        ("faultfinder", _, _) => FaultFinder,
        ("pixelcount", _, _) => PixelCount,
        ("wizard", Some("receiver"), _) => ReceiverWizard,
        ("mapping", _, _) => MapYard,
        ("calibration", _, _) | ("player", Some("calibration"), _) => SoundSync,
        ("tls", _, _) => PhoneTrust,
        ("public", Some("ca.crt" | "ca.mobileconfig" | "tls"), _) => PhoneTrust,
        ("reports", _, _) => Reports,
        ("alerts", _, _) => Alerts,
        ("power", Some("budget" | "live"), _) => Power,
        ("triggers", Some(_), _) | ("hooks", _, _) => Triggers,
        ("sensor-nodes", _, _) | ("cluster", Some("sensor-config"), _) => Sensors,
        ("surprises", _, _) => Surprises,
        ("player", Some("surprise"), None) => Surprises,
        ("mqtt", _, _) => Mqtt,
        ("remote", _, _) => Remote,
        ("xlights", _, _) => XlightsUpload,
        _ => return None,
    })
}

/// Middleware on `/api/v1`: a feature that is off answers `feature_disabled`.
pub async fn guard(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if let Some(id) = feature_for(req.method(), req.uri().path()) {
        if !state.store.get().feature(id) {
            let rel = req.uri().path().trim_start_matches("/api/v1");
            // Public pages and trigger links: nothing to see (404).
            let public = rel.starts_with("/public/") || rel.starts_with("/hooks/");
            return disabled_error(id, public).into_response();
        }
    }
    next.run(req).await
}

/// Middleware on the root-mounted xLights (FPP Connect) paths.
pub async fn root_guard(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if !state.store.get().feature(FeatureId::XlightsUpload) {
        return disabled_error(FeatureId::XlightsUpload, true).into_response();
    }
    next.run(req).await
}

/// The catalogue with each feature's state.
pub fn describe(settings: &FeatureSettings) -> Value {
    let features: Vec<Value> = FeatureId::ALL
        .into_iter()
        .map(|f| {
            json!({
                "id": f,
                "name": f.name(),
                "group": f.group(),
                "requires": f.requires(),
                "enabled": settings.is_enabled(f),
            })
        })
        .collect();
    json!({ "features": features, "disabled": settings.disabled })
}

async fn get_features(State(state): State<AppState>) -> Json<Value> {
    Json(describe(&state.store.get().settings.features))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PutBody {
    /// A whole set (a preset or a restore).
    #[serde(default)]
    disabled: Option<Vec<String>>,
    /// One switch.
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
}

async fn put_features(
    State(state): State<AppState>,
    Json(body): Json<PutBody>,
) -> ApiResult<Json<Value>> {
    let one = match (&body.id, body.enabled) {
        (Some(id), Some(on)) => Some((
            FeatureId::parse(id).ok_or_else(|| {
                ApiError::bad_request(format!("There is no feature called “{id}”."))
            })?,
            on,
        )),
        (None, None) => None,
        _ => {
            return Err(ApiError::bad_request(
                "Send either {disabled: [...]} or {id, enabled}.",
            ))
        }
    };
    if one.is_none() && body.disabled.is_none() {
        return Err(ApiError::bad_request(
            "Send either {disabled: [...]} or {id, enabled}.",
        ));
    }
    let (changed, show) = state
        .store
        .update(move |s| {
            let before = s.settings.features.disabled_ids();
            let f = &mut s.settings.features;
            match one {
                Some((id, on)) => {
                    f.set(id, on);
                }
                None => {
                    f.disabled = body.disabled.unwrap_or_default();
                    f.normalize();
                }
            }
            let after = f.disabled_ids();
            Ok(FeatureId::ALL
                .into_iter()
                .filter(|x| before.contains(x) != after.contains(x))
                .collect::<Vec<_>>())
        })
        .await?;
    let mut out = describe(&show.settings.features);
    out["changed"] = json!(changed);
    Ok(Json(out))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/features", get(get_features).put(put_features))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_map_to_their_feature() {
        let g = Method::GET;
        let p = Method::POST;
        assert_eq!(feature_for(&g, "/games/status"), Some(FeatureId::Games));
        assert_eq!(
            feature_for(&p, "/hooks/trigger/t1"),
            Some(FeatureId::Triggers)
        );
        assert_eq!(
            feature_for(&p, "/triggers/t1/token"),
            Some(FeatureId::Triggers)
        );
        assert_eq!(
            feature_for(&g, "/api/v1/public/requests"),
            Some(FeatureId::Requests)
        );
        assert_eq!(feature_for(&p, "/requests/abc"), Some(FeatureId::Requests));
        assert_eq!(feature_for(&g, "/public/health"), None);
        assert_eq!(
            feature_for(&g, "/public/ca.crt"),
            Some(FeatureId::PhoneTrust)
        );
        assert_eq!(feature_for(&g, "/dj-clips"), Some(FeatureId::Dj));
        assert_eq!(feature_for(&p, "/dj-clips/x/render"), Some(FeatureId::Dj));
        assert_eq!(feature_for(&g, "/effects"), None);
        assert_eq!(feature_for(&g, "/effects/schema"), None);
        assert_eq!(feature_for(&p, "/effects"), Some(FeatureId::Effects));
        assert_eq!(
            feature_for(&Method::PUT, "/effects/abc"),
            Some(FeatureId::Effects)
        );
        assert_eq!(
            feature_for(&p, "/media/m/analyze"),
            Some(FeatureId::AutoShows)
        );
        assert_eq!(feature_for(&g, "/media/m/file"), None);
        assert_eq!(
            feature_for(&g, "/sequences/s/preview"),
            Some(FeatureId::Layout)
        );
        assert_eq!(feature_for(&g, "/sequences/s/thumbnail"), None);
        assert_eq!(
            feature_for(&p, "/sequences/tags"),
            Some(FeatureId::SmartPlaylists)
        );
        assert_eq!(
            feature_for(&g, "/playlists/p/preview"),
            Some(FeatureId::SmartPlaylists)
        );
        assert_eq!(feature_for(&g, "/playlists"), None);
        assert_eq!(
            feature_for(&p, "/player/surprise"),
            Some(FeatureId::Surprises)
        );
        assert_eq!(feature_for(&p, "/player/surprise/stop"), None);
        assert_eq!(feature_for(&p, "/player/play"), None);
        assert_eq!(
            feature_for(&p, "/player/calibration"),
            Some(FeatureId::SoundSync)
        );
        assert_eq!(
            feature_for(&p, "/triggers/t/fire"),
            Some(FeatureId::Triggers)
        );
        assert_eq!(feature_for(&g, "/power/estimate"), None);
        assert_eq!(feature_for(&g, "/power/live"), Some(FeatureId::Power));
        assert_eq!(
            feature_for(&g, "/cluster/sensor-config/n"),
            Some(FeatureId::Sensors)
        );
        assert_eq!(feature_for(&g, "/cluster/status"), None);
        assert_eq!(feature_for(&g, "/features"), None);
        assert_eq!(feature_for(&g, "/show"), None);
        assert_eq!(feature_for(&g, "/system/update"), None);
        assert_eq!(feature_for(&g, "/snapshots"), None);
        assert_eq!(
            feature_for(&g, "/xlights/status"),
            Some(FeatureId::XlightsUpload)
        );
    }

    #[test]
    fn messages_are_friendly() {
        let e = disabled_error(FeatureId::Games, false);
        assert_eq!(e.status, StatusCode::CONFLICT);
        assert_eq!(e.code, "feature_disabled");
        assert!(e.message.contains("Games is turned off"));
        assert!(e.message.contains("Settings → Features"));
        assert_eq!(
            disabled_error(FeatureId::Requests, true).status,
            StatusCode::NOT_FOUND
        );
    }

    use crate::api::testkit::TestApp;
    use axum::http::StatusCode as S;

    #[tokio::test]
    async fn get_and_put_features() {
        let app = TestApp::new();
        let (st, v) = app.json("GET", "/features", None).await;
        assert_eq!(st, S::OK);
        assert_eq!(
            v["features"].as_array().unwrap().len(),
            FeatureId::ALL.len()
        );
        assert!(v["features"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["enabled"] == true));
        assert_eq!(v["features"][12]["requires"], json!(["phoneTrust"]));

        // One switch: what needs it follows.
        let (st, v) = app
            .json(
                "PUT",
                "/features",
                Some(json!({"id": "phoneTrust", "enabled": false})),
            )
            .await;
        assert_eq!(st, S::OK, "{v}");
        assert_eq!(v["changed"], json!(["mapYard", "soundSync", "phoneTrust"]));
        assert!(!app.state.store.get().feature(FeatureId::MapYard));
        // Turning one back on brings what it needs.
        let (_, v) = app
            .json(
                "PUT",
                "/features",
                Some(json!({"id": "soundSync", "enabled": true})),
            )
            .await;
        assert_eq!(v["changed"], json!(["soundSync", "phoneTrust"]));
        // A whole set (preset), normalized.
        let (_, v) = app
            .json(
                "PUT",
                "/features",
                Some(json!({"disabled": ["triggers", "games"]})),
            )
            .await;
        assert_eq!(
            v["disabled"],
            json!(["games", "sensors", "surprises", "triggers"])
        );
        // Also visible in GET /show.
        let (_, show) = app.json("GET", "/show", None).await;
        assert_eq!(show["settings"]["features"]["disabled"], v["disabled"]);
        let (st, _) = app
            .json(
                "PUT",
                "/features",
                Some(json!({"id": "warpDrive", "enabled": true})),
            )
            .await;
        assert_eq!(st, S::BAD_REQUEST);
        let (st, _) = app.json("PUT", "/features", Some(json!({}))).await;
        assert_eq!(st, S::BAD_REQUEST);
    }

    #[tokio::test]
    async fn disabled_features_answer_feature_disabled() {
        let app = TestApp::new();
        app.state
            .store
            .update(|s| {
                s.settings.requests.enabled = true;
                Ok(())
            })
            .await
            .unwrap();
        let (st, _) = app.json("GET", "/public/requests", None).await;
        assert_eq!(st, S::OK);
        let (st, _) = app.json("GET", "/profiles", None).await;
        assert_eq!(st, S::OK);
        app.json(
            "PUT",
            "/features",
            Some(json!({"disabled": ["requests", "seasons", "games", "effects"]})),
        )
        .await;
        let (st, v) = app.json("GET", "/public/requests", None).await;
        assert_eq!(st, S::NOT_FOUND);
        assert_eq!(v["error"]["code"], "feature_disabled");
        let (st, v) = app.json("GET", "/profiles", None).await;
        assert_eq!(st, S::CONFLICT);
        assert_eq!(v["error"]["code"], "feature_disabled");
        assert!(v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Seasons is turned off"));
        let (st, _) = app.json("GET", "/games/status", None).await;
        assert_eq!(st, S::CONFLICT);
        // Custom looks: readable, not writable.
        let (st, _) = app.json("GET", "/effects", None).await;
        assert_eq!(st, S::OK);
        let (st, _) = app
            .json(
                "POST",
                "/effects",
                Some(json!({"name": "x", "effect": "solid"})),
            )
            .await;
        assert_eq!(st, S::CONFLICT);
        // Core endpoints keep working.
        let (st, _) = app.json("GET", "/show", None).await;
        assert_eq!(st, S::OK);
        let (st, _) = app.json("GET", "/playlists", None).await;
        assert_eq!(st, S::OK);
        // Back on: works again.
        app.json("PUT", "/features", Some(json!({"disabled": []})))
            .await;
        let (st, _) = app.json("GET", "/public/requests", None).await;
        assert_eq!(st, S::OK);
    }

    #[tokio::test]
    async fn xlights_root_paths_follow_the_toggle() {
        let app = TestApp::new();
        async fn get(app: &TestApp) -> S {
            let req = axum::http::Request::builder()
                .uri("/api/fppd/multiSyncSystems")
                .body(axum::body::Body::empty())
                .unwrap();
            app.send(req).await.0
        }
        app.state
            .store
            .update(|s| {
                s.settings.xlights.fpp_connect = true;
                Ok(())
            })
            .await
            .unwrap();
        assert_ne!(get(&app).await, S::NOT_FOUND);
        app.json(
            "PUT",
            "/features",
            Some(json!({"id": "xlightsUpload", "enabled": false})),
        )
        .await;
        assert_eq!(get(&app).await, S::NOT_FOUND);
    }

    #[tokio::test]
    async fn triggers_and_surprises_do_not_fire_while_off() {
        let app = TestApp::new();
        app.state
            .store
            .update(|s| {
                s.settings.triggers.push(
                    serde_json::from_value(json!({
                        "id": "t1", "name": "Button", "kind": "http",
                        "action": {"type": "stop"}
                    }))
                    .unwrap(),
                );
                Ok(())
            })
            .await
            .unwrap();
        let (st, _) = app.json("POST", "/triggers/t1/fire", None).await;
        assert_eq!(st, S::OK);
        app.json(
            "PUT",
            "/features",
            Some(json!({"id": "triggers", "enabled": false})),
        )
        .await;
        let (st, v) = app.json("POST", "/triggers/t1/fire", None).await;
        assert_eq!(st, S::CONFLICT, "{v}");
        let t = app.state.store.get().settings.triggers[0].clone();
        let r = crate::services::triggers::fire_trigger(&app.state, &t, "gpio").await;
        assert_eq!(r.unwrap_err().code, "feature_disabled");
    }
}
