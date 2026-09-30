//! Pixel-count check (F7): `/pixelcount/…`. Owned by WS4.
//!
//! * `camera`: a phase-B-only mapping run on one output that also lights
//!   pixels past the configured end (`max(configured × 1.25, configured +
//!   64)`, clamped to the DPI geometry limit on this node); the phone decodes
//!   the highest responding pixel index.
//! * `manual`: a binary search (`faultfinder::CountSearch`): pixels `0..k`
//!   dim green and pixel `k` red; "can you see the red pixel?" (≤ 12 taps
//!   for 2048 pixels).
//! * `current`: needs a receiver-side current sensor (F20), not available on
//!   difftxlarge's INA226 (it only sees the transmitter's own input).

use super::mapping::{check_idle, map_test, start_response, start_test, stop_pattern};
use super::{ApiError, ApiResult};
use crate::services::mapping::{self as svc, CountSession, MapScope, MappingRun, PlanOptions};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use pixelplus_core::faultfinder::CountSearch;
use pixelplus_core::mapcode::{self, MapPlan, MapTarget};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartBody {
    node_id: String,
    output: u32,
    #[serde(default = "manual")]
    method: String,
    #[serde(default)]
    force: bool,
}

fn manual() -> String {
    "manual".into()
}

/// DPI geometry limit when the output is on this node.
fn local_limit(state: &AppState, node_id: &str) -> Option<u32> {
    (state.identity().id == node_id)
        .then(|| crate::player::geometry_status().max_pixels)
        .flatten()
}

fn probe_plan(node_id: &str, output: u32, max_probe: u32, probe: u32) -> MapPlan {
    MapPlan {
        seed: 0,
        level: 60,
        passes: 1,
        phases: 0,
        targets: vec![MapTarget {
            node_id: node_id.into(),
            output,
            max_pixels: max_probe,
        }],
        pixel_bits: mapcode::pixel_bits_for(max_probe),
        count_probe: Some(probe),
        ..MapPlan::default()
    }
}

fn step_json(id: &str, s: &CountSession) -> Value {
    let base = json!({
        "session": id,
        "nodeId": s.node_id,
        "output": s.output,
        "configured": s.configured,
        "maxProbe": s.search.max_probe(),
        "canUndo": s.search.answers_given() > 0,
    });
    let mut v = base;
    match (s.search.current_step(), s.search.result()) {
        (Some(step), _) => {
            v["step"] = json!({
                "litUntil": step.probe,
                "ask": step.question,
                "number": step.number,
                "maxRemaining": step.max_remaining,
            });
        }
        (None, Some(count)) => v["count"] = json!(count),
        _ => {}
    }
    v
}

async fn light_probe(state: &AppState, id: &str, s: &CountSession) -> ApiResult<()> {
    match s.search.current_step() {
        Some(step) => {
            let plan = probe_plan(&s.node_id, s.output, s.search.max_probe(), step.probe);
            // A forgotten check must not stay lit all night.
            start_test(
                state,
                id,
                map_test(plan, id),
                Some(Duration::from_secs(svc::SESSION_TTL_S)),
                "The pixel-count check",
            )
            .await
        }
        None => {
            stop_pattern(state, Some(id)).await;
            Ok(())
        }
    }
}

async fn start(State(state): State<AppState>, Json(b): Json<StartBody>) -> ApiResult<Json<Value>> {
    svc::expire_sessions(&state);
    let show = state.store.get();
    let node = show
        .node(&b.node_id)
        .ok_or_else(|| ApiError::not_found("That controller"))?;
    if b.output == 0 || !node.outputs.iter().any(|o| o.index == b.output) {
        return Err(ApiError::bad_request(format!(
            "{} doesn't have output {}.",
            node.name, b.output
        )));
    }
    let configured = svc::output_length(&show, &b.node_id, b.output);
    let limit = local_limit(&state, &b.node_id);
    let max_probe = svc::probe_len(configured.max(1), limit);
    let limited = limit.is_some_and(|l| max_probe >= l);
    match b.method.as_str() {
        "camera" => {
            check_idle(&state, b.force)?;
            let outs = vec![(b.node_id.clone(), b.output)];
            let opts = PlanOptions {
                probe_extra: true,
                ..Default::default()
            };
            let (plan, targets) = svc::build_plan(&show, &outs, &opts, mapcode::PHASE_B, |_| limit)
                .map_err(ApiError::bad_request)?;
            let run = MappingRun {
                id: pixelplus_core::model::new_id(),
                kind: "pixelCount".into(),
                started_at: chrono::Local::now()
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, false),
                scope: MapScope {
                    node_id: Some(b.node_id.clone()),
                    ..Default::default()
                },
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
                "The camera pixel-count check",
            )
            .await?;
            let mut v = start_response(&run);
            v["configured"] = json!(configured);
            v["maxProbe"] = json!(run.plan.targets[0].max_pixels);
            v["limited"] = json!(limited);
            Ok(Json(v))
        }
        "manual" => {
            check_idle(&state, b.force)?;
            let id = pixelplus_core::model::new_id();
            let s = CountSession {
                node_id: b.node_id.clone(),
                output: b.output,
                configured,
                search: CountSearch::new(max_probe),
                last_used: Instant::now(),
            };
            light_probe(&state, &id, &s).await?;
            let mut v = step_json(&id, &s);
            v["limited"] = json!(limited);
            state.services.mapping.counts.lock().insert(id, s);
            Ok(Json(v))
        }
        "current" => Err(ApiError::bad_request(
            "Measuring by current needs a current sensor on the receiver's power feed \
             (an ESP32 sensor node with an INA226). Use the camera or the manual check.",
        )),
        m => Err(ApiError::bad_request(format!("Unknown method \"{m}\"."))),
    }
}

fn no_session() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::CONFLICT,
        "no_session",
        "That pixel-count check has ended. Start it again.",
    )
}

#[derive(Deserialize)]
struct AnswerBody {
    seen: bool,
}

async fn answer_or_undo(state: &AppState, id: &str, answer: Option<bool>) -> ApiResult<Json<Value>> {
    let (v, relight) = {
        let mut map = state.services.mapping.counts.lock();
        let s = map.get_mut(id).ok_or_else(no_session)?;
        s.last_used = Instant::now();
        let before = s.search.clone();
        match answer {
            Some(seen) => s.search.answer(seen),
            None => {
                s.search.undo();
            }
        }
        let relight = before != s.search;
        (step_json(id, s), relight.then(|| CountSession {
            node_id: s.node_id.clone(),
            output: s.output,
            configured: s.configured,
            search: s.search.clone(),
            last_used: s.last_used,
        }))
    };
    if let Some(s) = relight {
        light_probe(state, id, &s).await?;
    }
    Ok(Json(v))
}

async fn answer(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<AnswerBody>,
) -> ApiResult<Json<Value>> {
    answer_or_undo(&state, &id, Some(b.seen)).await
}

async fn undo(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    answer_or_undo(&state, &id, None).await
}

async fn stop(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    state.services.mapping.counts.lock().remove(&id);
    stop_pattern(&state, Some(&id)).await;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResultBody {
    count: u32,
    #[serde(default)]
    dead: Vec<u32>,
    #[serde(default)]
    confidence: Option<f32>,
}

/// The camera result for run `id` (stored with the run).
async fn result(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<ResultBody>,
) -> ApiResult<Json<Value>> {
    let mut run = svc::load(&state, &id).await?;
    let t = run
        .targets
        .first()
        .cloned()
        .ok_or_else(|| ApiError::bad_request("This run has no output."))?;
    let res = run.results.get_or_insert_with(Default::default);
    res.stats = Some(json!({ "count": b.count, "dead": b.dead, "confidence": b.confidence }));
    svc::save(&state, &run).await?;
    Ok(Json(json!({
        "ok": true,
        "count": b.count,
        "configured": t.configured,
        "nodeId": t.node_id,
        "output": t.output,
    })))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ApplyBody {
    #[serde(default)]
    update_prop_count: bool,
    #[serde(default)]
    count: Option<u32>,
    #[serde(default)]
    dead: Option<Vec<u32>>,
}

/// Store the measured count (and optionally resize the last prop) for a
/// manual session or a camera run `id`.
async fn apply(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<ApplyBody>,
) -> ApiResult<Json<Value>> {
    let from_session = {
        let map = state.services.mapping.counts.lock();
        map.get(&id)
            .map(|s| (s.node_id.clone(), s.output, s.search.result()))
    };
    let (node_id, output, count, dead, method) = match from_session {
        Some((n, o, c)) => {
            let count = b
                .count
                .or(c)
                .ok_or_else(|| ApiError::bad_request("Finish the check first."))?;
            (n, o, count, b.dead.clone().unwrap_or_default(), "manual")
        }
        None => {
            let run = svc::load(&state, &id).await.map_err(|_| no_session())?;
            let t = run
                .targets
                .first()
                .cloned()
                .ok_or_else(|| ApiError::bad_request("This run has no output."))?;
            let stats = run.results.and_then(|r| r.stats).unwrap_or(Value::Null);
            let count = b
                .count
                .or_else(|| stats["count"].as_u64().map(|c| c as u32))
                .ok_or_else(|| ApiError::bad_request("No count was measured yet."))?;
            let dead = b.dead.clone().unwrap_or_else(|| {
                serde_json::from_value(stats["dead"].clone()).unwrap_or_default()
            });
            (t.node_id, t.output, count, dead, "camera")
        }
    };
    // Dry run so a refusal doesn't leave a snapshot behind.
    svc::apply_count(
        &mut (*state.store.get()).clone(),
        &node_id,
        output,
        count,
        &dead,
        method,
        b.update_prop_count,
    )?;
    let snap = crate::services::snapshots::create(&state, "Before pixel count fix", true, false).await?;
    let upd = b.update_prop_count;
    let (message, show) = state
        .store
        .update(move |s| svc::apply_count(s, &node_id, output, count, &dead, method, upd))
        .await?;
    state.services.mapping.counts.lock().remove(&id);
    stop_pattern(&state, Some(&id)).await;
    Ok(Json(json!({ "show": &*show, "snapshotId": snap.id, "message": message })))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/pixelcount/start", post(start))
        .route("/pixelcount/{id}/answer", post(answer))
        .route("/pixelcount/{id}/undo", post(undo))
        .route("/pixelcount/{id}/stop", post(stop))
        .route("/pixelcount/{id}/result", post(result))
        .route("/pixelcount/{id}/apply", post(apply))
}

#[cfg(test)]
mod tests {
    use crate::api::testkit::TestApp;
    use axum::http::StatusCode;
    use serde_json::json;

    async fn app() -> TestApp {
        let app = TestApp::new();
        let show = crate::services::mapping::tests_support::show();
        app.state.store.replace(show).await.unwrap();
        app
    }

    #[tokio::test]
    async fn manual_search_finds_count_and_applies() {
        let app = app().await;
        let (s, mut v) = app
            .json("POST", "/pixelcount/start", Some(json!({"nodeId": "lead", "output": 2, "method": "manual"})))
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["configured"], 100);
        assert_eq!(v["maxProbe"], 164);
        let id = v["session"].as_str().unwrap().to_string();
        let real = 97u64;
        let mut steps = 0;
        while let Some(k) = v["step"]["litUntil"].as_u64() {
            let (s, nv) = app
                .json("POST", &format!("/pixelcount/{id}/answer"), Some(json!({"seen": k < real})))
                .await;
            assert_eq!(s, StatusCode::OK, "{nv}");
            v = nv;
            steps += 1;
            assert!(steps <= 12);
        }
        assert_eq!(v["count"], real);
        assert!(app.commands_matching("countProbe: Some(").await >= 2);
        let (s, v) = app
            .json("POST", &format!("/pixelcount/{id}/apply"), Some(json!({"updatePropCount": true})))
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(app.state.store.get().prop("c").unwrap().pixel_count, 97);
        let (s, _) = app.json("POST", &format!("/pixelcount/{id}/answer"), Some(json!({"seen": true}))).await;
        assert_eq!(s, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn camera_and_current_methods() {
        let app = app().await;
        let (s, v) = app
            .json("POST", "/pixelcount/start", Some(json!({"nodeId": "lead", "output": 1, "method": "camera"})))
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["plan"]["phases"], 2);
        assert_eq!(v["plan"]["targets"][0]["maxPixels"], 144);
        assert_eq!(v["codebook"].as_array().unwrap().len(), 0);
        let id = v["runId"].as_str().unwrap().to_string();
        let (s, v) = app
            .json("POST", &format!("/pixelcount/{id}/result"), Some(json!({"count": 80, "dead": [3]})))
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["configured"], 80);
        let (s, v) = app.json("POST", &format!("/pixelcount/{id}/apply"), Some(json!({}))).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let show = app.state.store.get();
        let m = show.node("lead").unwrap().outputs[0].measured_pixels.clone().unwrap();
        assert_eq!((m.count, m.method.as_str(), m.dead.clone()), (80, "camera", vec![3]));
        assert_eq!(show.prop("a").unwrap().suspect_pixels, vec![3]);
        let (s, _) = app
            .json("POST", "/pixelcount/start", Some(json!({"nodeId": "lead", "output": 1, "method": "current"})))
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, _) = app
            .json("POST", "/pixelcount/start", Some(json!({"nodeId": "lead", "output": 99})))
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }
}
