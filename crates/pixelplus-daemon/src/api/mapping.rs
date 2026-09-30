//! Camera mapping (F6): `/mapping/runs…`. Owned by WS4.
//!
//! The phone starts a run, records the display while the engine renders the
//! `mapCode` test pattern (`pixelplus_core::mapcode`), decodes it in the
//! browser (`web/src/lib/cv`) and posts the detected lights and proposals
//! back. Applying proposals takes an automatic snapshot first (undo =
//! restore it). Runs live in `<data>/mapping/` (services::mapping).

use super::content::player;
use super::{ApiError, ApiResult};
use crate::player::{PlayerCmd, PlayerState, TestRequest};
use crate::services::mapping::{
    self as svc, ActivePattern, MapScope, MappingRun, PlanOptions, RunResults,
};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use pixelplus_core::mapcode::{self, MapPlan};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Pattern control (shared with pixelcount.rs and wizard.rs)
// ---------------------------------------------------------------------------

/// Refuse to take over the lights during a scheduled show unless `force`.
pub(crate) fn check_idle(state: &AppState, force: bool) -> ApiResult<()> {
    let st = player(state)?.status();
    if !force && st.state == PlayerState::Playing && st.schedule_entry.is_some() {
        return Err(ApiError::conflict(
            "The show is playing. Stop it first, or choose \"Use the lights anyway\".",
        ));
    }
    Ok(())
}

/// Map "unknown test mode" from an engine without the feature to a clear 503.
fn engine_error(e: ApiError, what: &str) -> ApiError {
    if e.message.contains("unknown test mode") {
        ApiError::unavailable(format!(
            "{what} needs a newer player on this controller (test mode not available yet)."
        ))
    } else {
        e
    }
}

/// Start a test on behalf of pattern `id`, replacing any earlier one of ours.
pub(crate) async fn start_test(
    state: &AppState,
    id: &str,
    req: TestRequest,
    auto_stop: Option<Duration>,
    what: &str,
) -> ApiResult<()> {
    let p = player(state)?.clone();
    p.test_start(req).await.map_err(|e| engine_error(e, what))?;
    let stopper = auto_stop.map(|d| {
        let st = state.clone();
        let id = id.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(d).await;
            let mine = st
                .services
                .mapping
                .active
                .lock()
                .as_ref()
                .is_some_and(|a| a.id == id);
            if mine {
                if let Some(p) = st.services.player.get() {
                    let _ = p.send(PlayerCmd::TestStop).await;
                }
                // Not `take()` under the lock with this task inside: the drop
                // would abort ourselves before the lock is released.
                let old = st.services.mapping.active.lock().take();
                if let Some(mut a) = old {
                    a.stopper = None;
                }
            }
        })
    });
    let old = state.services.mapping.active.lock().replace(ActivePattern {
        id: id.to_string(),
        stopper,
    });
    drop(old);
    Ok(())
}

/// A `mapCode` test for `plan`.
pub(crate) fn map_test(plan: MapPlan, run_id: &str) -> TestRequest {
    TestRequest {
        mode: "mapCode".into(),
        color: None,
        speed: None,
        target: Default::default(),
        effect: None,
        map: Some(plan),
        map_run_id: Some(run_id.to_string()),
        identify: None,
        cal: None,
    }
}

/// Stop our pattern (only pattern `id`, when given). Returns whether one was stopped.
pub(crate) async fn stop_pattern(state: &AppState, id: Option<&str>) -> bool {
    let old = {
        let mut a = state.services.mapping.active.lock();
        match (a.as_ref(), id) {
            (Some(x), Some(id)) if x.id != id => None,
            _ => a.take(),
        }
    };
    if old.is_some() {
        if let Some(p) = state.services.player.get() {
            let _ = p.send(PlayerCmd::TestStop).await;
        }
        true
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct StartBody {
    #[serde(default)]
    scope: Option<MapScope>,
    #[serde(flatten)]
    opts: PlanOptions,
    #[serde(default)]
    force: bool,
}

pub(crate) fn start_response(run: &MappingRun) -> Value {
    json!({
        "runId": run.id,
        "kind": run.kind,
        "startedAt": run.started_at,
        "plan": run.plan,
        "schedule": mapcode::schedule(&run.plan),
        "codebook": svc::codebook_bits(&run.plan),
        "targets": run.targets,
    })
}

async fn start_run(State(state): State<AppState>, body: Bytes) -> ApiResult<Json<Value>> {
    let b: StartBody = if body.is_empty() {
        StartBody::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| ApiError::bad_request(format!("Bad request: {e}")))?
    };
    check_idle(&state, b.force)?;
    let show = state.store.get();
    let mut scope = b.scope.unwrap_or_default();
    if scope.node_id.is_none() && scope.prop_ids.is_empty() {
        scope.all = true;
    }
    if let Some(n) = &scope.node_id {
        show.node(n)
            .ok_or_else(|| ApiError::not_found("That controller"))?;
    }
    let outs = svc::scope_targets(&show, &scope);
    if outs.is_empty() {
        return Err(ApiError::bad_request(
            "No props are wired to controller outputs here yet. Set up wiring first.",
        ));
    }
    let (plan, targets) = svc::build_plan(
        &show,
        &outs,
        &b.opts,
        mapcode::PHASE_A | mapcode::PHASE_B,
        |_| None,
    )
    .map_err(ApiError::bad_request)?;
    let run = MappingRun {
        id: pixelplus_core::model::new_id(),
        kind: "map".into(),
        started_at: chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false),
        scope,
        plan,
        targets,
        results: None,
        applied_snapshot_id: None,
        applied_proposal_ids: vec![],
        photo: false,
    };
    svc::save(&state, &run).await?;
    let total = mapcode::schedule(&run.plan).total_ms;
    start_test(
        &state,
        &run.id,
        map_test(run.plan.clone(), &run.id),
        Some(Duration::from_millis(total + 1500)),
        "Camera mapping",
    )
    .await?;
    svc::prune(&state).await;
    Ok(Json(start_response(&run)))
}

async fn stop_run(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let stopped = stop_pattern(&state, Some(&id)).await;
    Ok(Json(json!({ "ok": true, "stopped": stopped })))
}

/// Light the display dim white so the user can frame the shot ("Frame it").
async fn frame_lights(State(state): State<AppState>, body: Bytes) -> ApiResult<Json<Value>> {
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase")]
    struct B {
        #[serde(default)]
        on: Option<bool>,
        #[serde(default)]
        force: bool,
    }
    let b: B = if body.is_empty() {
        B::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| ApiError::bad_request(format!("Bad request: {e}")))?
    };
    if b.on == Some(false) {
        stop_pattern(&state, Some("frame")).await;
        return Ok(Json(json!({ "ok": true })));
    }
    check_idle(&state, b.force)?;
    let req = TestRequest {
        mode: "solid".into(),
        color: Some("#262626".into()),
        speed: None,
        target: crate::player::TestTarget {
            props: pixelplus_core::model::Target {
                all: true,
                ..Default::default()
            },
            ..Default::default()
        },
        effect: None,
        map: None,
        map_run_id: None,
        identify: None,
        cal: None,
    };
    // Framing ends by itself after 5 minutes.
    start_test(
        &state,
        "frame",
        req,
        Some(Duration::from_secs(300)),
        "Framing",
    )
    .await?;
    Ok(Json(json!({ "ok": true })))
}

async fn list_runs(State(state): State<AppState>) -> Json<Vec<MappingRun>> {
    let mut runs = svc::list(&state).await;
    // The list stays light: detected pixels only in GET /mapping/runs/:id.
    for r in &mut runs {
        if let Some(res) = r.results.as_mut() {
            res.detected.clear();
        }
    }
    Json(runs)
}

async fn get_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<MappingRun>> {
    svc::load(&state, &id).await.map(Json)
}

async fn delete_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    stop_pattern(&state, Some(&id)).await;
    svc::delete(&state, &id).await?;
    Ok(Json(json!({ "ok": true })))
}

/// Most decoded lights stored per run.
const MAX_DETECTED: usize = 200_000;
const MAX_PROPOSALS: usize = 5_000;

async fn post_results(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<RunResults>,
) -> ApiResult<Json<MappingRun>> {
    let mut run = svc::load(&state, &id).await?;
    let n: usize = b.detected.iter().map(|d| d.pixels.len()).sum();
    if n > MAX_DETECTED || b.proposals.len() > MAX_PROPOSALS {
        return Err(ApiError::bad_request("Too many results for one run."));
    }
    if b.detected.iter().any(|d| d.k as usize >= run.targets.len()) {
        return Err(ApiError::bad_request(
            "A result refers to an unknown output.",
        ));
    }
    let mut ids = std::collections::HashSet::new();
    if b.proposals.iter().any(|p| !ids.insert(p.id.clone())) {
        return Err(ApiError::bad_request("Proposal ids must be unique."));
    }
    run.results = Some(b);
    svc::save(&state, &run).await?;
    Ok(Json(run))
}

async fn put_photo(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let mut run = svc::load(&state, &id).await?;
    if body.len() > svc::MAX_PHOTO {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_large",
            "The photo is too large (2 MB at most).",
        ));
    }
    if !body.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Err(ApiError::bad_request("The photo must be a JPEG."));
    }
    let path =
        svc::photo_path(&state, &id).ok_or_else(|| ApiError::not_found("That mapping run"))?;
    tokio::fs::write(&path, &body)
        .await
        .map_err(|e| ApiError::internal(format!("couldn't save the photo: {e}")))?;
    run.photo = true;
    svc::save(&state, &run).await?;
    Ok(Json(json!({ "ok": true })))
}

fn jpeg(bytes: Vec<u8>) -> Response {
    let mut h = HeaderMap::new();
    h.insert(header::CONTENT_TYPE, "image/jpeg".parse().unwrap());
    (h, bytes).into_response()
}

async fn get_photo(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Response> {
    let path = svc::photo_path(&state, &id).ok_or_else(|| ApiError::not_found("That photo"))?;
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|_| ApiError::not_found("That photo"))?;
    Ok(jpeg(bytes))
}

fn background_path(state: &AppState) -> std::path::PathBuf {
    state.config.data_dir.join("layout").join("background.jpg")
}

/// "Save photo as layout background" → `<data>/layout/background.jpg`.
async fn photo_as_background(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let src = svc::photo_path(&state, &id).ok_or_else(|| ApiError::not_found("That photo"))?;
    if !src.exists() {
        return Err(ApiError::not_found("That photo"));
    }
    let dst = background_path(&state);
    if let Some(d) = dst.parent() {
        tokio::fs::create_dir_all(d)
            .await
            .map_err(ApiError::internal)?;
    }
    tokio::fs::copy(src, &dst)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(json!({ "ok": true, "path": "layout/background.jpg" })))
}

async fn get_background(State(state): State<AppState>) -> ApiResult<Response> {
    let bytes = tokio::fs::read(background_path(&state))
        .await
        .map_err(|_| ApiError::not_found("A layout background"))?;
    Ok(jpeg(bytes))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApplyBody {
    proposal_ids: Vec<String>,
}

async fn apply_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<ApplyBody>,
) -> ApiResult<Json<Value>> {
    let mut run = svc::load(&state, &id).await?;
    if b.proposal_ids.is_empty() {
        return Err(ApiError::bad_request("Pick at least one change to apply."));
    }
    // Dry run first: a bad selection must not leave a pointless snapshot.
    svc::apply_proposals(&mut (*state.store.get()).clone(), &run, &b.proposal_ids)?;
    let snap =
        crate::services::snapshots::create(&state, "Before camera mapping", true, false).await?;
    let r = run.clone();
    let ids = b.proposal_ids.clone();
    let (applied, show) = state
        .store
        .update(move |s| svc::apply_proposals(s, &r, &ids))
        .await?;
    run.applied_snapshot_id = Some(snap.id.clone());
    for id in b.proposal_ids {
        if !run.applied_proposal_ids.contains(&id) {
            run.applied_proposal_ids.push(id);
        }
    }
    svc::save(&state, &run).await?;
    Ok(Json(
        json!({ "show": &*show, "snapshotId": snap.id, "applied": applied }),
    ))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mapping/runs", get(list_runs).post(start_run))
        .route("/mapping/frame", post(frame_lights))
        .route("/mapping/background", get(get_background))
        .route("/mapping/runs/{id}", get(get_run).delete(delete_run))
        .route("/mapping/runs/{id}/stop", post(stop_run))
        .route(
            "/mapping/runs/{id}/results",
            post(post_results).layer(DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
        .route(
            "/mapping/runs/{id}/photo",
            get(get_photo)
                .put(put_photo)
                .layer(DefaultBodyLimit::max(svc::MAX_PHOTO + 1024)),
        )
        .route(
            "/mapping/runs/{id}/photo/background",
            post(photo_as_background),
        )
        .route("/mapping/runs/{id}/apply", post(apply_run))
}

#[cfg(test)]
mod tests {
    use crate::api::testkit::TestApp;
    use axum::http::StatusCode;
    use serde_json::json;

    pub(crate) async fn app_with_show() -> TestApp {
        let app = TestApp::new();
        let show = crate::services::mapping::tests_support::show();
        app.state.store.replace(show).await.unwrap();
        app
    }

    #[tokio::test]
    async fn run_lifecycle() {
        let app = app_with_show().await;
        let (s, v) = app.json("POST", "/mapping/runs", Some(json!({}))).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let id = v["runId"].as_str().unwrap().to_string();
        assert_eq!(v["plan"]["targets"].as_array().unwrap().len(), 2);
        assert_eq!(v["codebook"][0].as_array().unwrap().len(), 12);
        assert!(v["schedule"]["totalMs"].as_u64().unwrap() > 20_000);
        assert_eq!(v["schedule"]["phaseAms"], 2400);
        assert_eq!(app.commands_matching("mapCode").await, 1);

        let (s, list) = app.json("GET", "/mapping/runs", None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(list[0]["id"], id.as_str());

        let res = json!({
            "detected": [{"k": 0, "pixels": [[0, 0.1, 0.2, 0.9]]}],
            "proposals": [
                {"id": "r", "kind": "reverse", "message": "Reverse Prop a", "data": {"propId": "a", "segment": 0}},
                {"id": "n", "kind": "notSeen", "message": "Not seen"}
            ]
        });
        let (s, v) = app
            .json("POST", &format!("/mapping/runs/{id}/results"), Some(res))
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let (s, v) = app
            .json(
                "POST",
                &format!("/mapping/runs/{id}/apply"),
                Some(json!({"proposalIds": ["r"]})),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert!(v["snapshotId"].as_str().is_some());
        assert!(app.state.store.get().prop("a").unwrap().segments[0].reverse);
        let (_, v) = app.json("GET", &format!("/mapping/runs/{id}"), None).await;
        assert_eq!(v["appliedProposalIds"][0], "r");
        // Unknown proposal: 404 and nothing changes.
        let (s, _) = app
            .json(
                "POST",
                &format!("/mapping/runs/{id}/apply"),
                Some(json!({"proposalIds": ["zz"]})),
            )
            .await;
        assert_eq!(s, StatusCode::NOT_FOUND);

        let (s, _) = app
            .json("POST", &format!("/mapping/runs/{id}/stop"), None)
            .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(app.commands_matching("TestStop").await, 1);
        let (s, _) = app
            .json("DELETE", &format!("/mapping/runs/{id}"), None)
            .await;
        assert_eq!(s, StatusCode::OK);
        let (s, _) = app.json("GET", &format!("/mapping/runs/{id}"), None).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        let (s, _) = app.json("GET", "/mapping/runs/..%2Fshow", None).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn empty_scope_and_bad_options_are_rejected() {
        let app = app_with_show().await;
        let (s, _) = app
            .json(
                "POST",
                "/mapping/runs",
                Some(json!({"scope": {"nodeId": "nope"}})),
            )
            .await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        let (s, _) = app
            .json("POST", "/mapping/runs", Some(json!({"level": 250})))
            .await;
        // Level is clamped to 50 %, not rejected.
        assert_eq!(s, StatusCode::OK);
        let (s, v) = app
            .json("POST", "/mapping/runs", Some(json!({"bitMs": 20})))
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    }

    #[tokio::test]
    async fn photo_roundtrip() {
        let app = app_with_show().await;
        let (_, v) = app.json("POST", "/mapping/runs", None).await;
        let id = v["runId"].as_str().unwrap().to_string();
        let req = axum::http::Request::builder()
            .method("PUT")
            .uri(format!("/api/v1/mapping/runs/{id}/photo"))
            .header("content-type", "image/jpeg")
            .body(axum::body::Body::from(vec![
                0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3,
            ]))
            .unwrap();
        let (s, _, _) = app.send(req).await;
        assert_eq!(s, StatusCode::OK);
        let req = axum::http::Request::builder()
            .uri(format!("/api/v1/mapping/runs/{id}/photo"))
            .body(axum::body::Body::empty())
            .unwrap();
        let (s, h, body) = app.send(req).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(h["content-type"], "image/jpeg");
        assert_eq!(body.len(), 7);
        let (s, _) = app
            .json(
                "POST",
                &format!("/mapping/runs/{id}/photo/background"),
                None,
            )
            .await;
        assert_eq!(s, StatusCode::OK);
        let req = axum::http::Request::builder()
            .method("PUT")
            .uri(format!("/api/v1/mapping/runs/{id}/photo"))
            .body(axum::body::Body::from("not a jpeg"))
            .unwrap();
        assert_eq!(app.send(req).await.0, StatusCode::BAD_REQUEST);
    }
}
