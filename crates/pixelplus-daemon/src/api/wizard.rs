//! New receiver wizard (F9): `/wizard/receiver/…`. Owned by WS4.
//!
//! 1. `identify-jack`: port 1 of every free jack blinks a signal (white or
//!    blue, 1–4 blinks; test mode `identify`). The user taps what they see;
//!    with more than 8 free jacks a second round narrows it down. An engine
//!    without `identify` falls back to lighting one jack at a time
//!    (`method: "sequential"`).
//! 2. `port/:n/light`: light one port (solid, chase for direction, or pure
//!    red/green for colour-order detection → `color-order`).
//! 3. `finish`: create the receiver and wire the chosen props (auto snapshot).

use super::mapping::{check_idle, start_test, stop_pattern};
use super::{ApiError, ApiResult};
use crate::player::{IdentifyLight, TestRequest, TestTarget};
use crate::services::mapping::{self as svc, WizardSession};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use pixelplus_core::model::{ColorOrder, PropSegment, Receiver, ReceiverKind, Show};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// Lights stay on at most this long without a new request.
const LIGHT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

fn no_session() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::CONFLICT,
        "no_session",
        "This wizard has ended. Start \"Add receiver\" again.",
    )
}

fn raw_test(node_id: &str, output: u32, mode: &str, color: Option<&str>) -> TestRequest {
    TestRequest {
        mode: mode.into(),
        color: color.map(str::to_string),
        speed: None,
        target: TestTarget {
            node_id: Some(node_id.into()),
            output: Some(output),
            props: Default::default(),
        },
        effect: None,
        map: None,
        map_run_id: None,
        identify: None,
        cal: None,
    }
}

fn session_key(id: &str) -> String {
    format!("wiz-{id}")
}

/// Light the identify signals for the session's candidates, or (sequential
/// method) the first candidate. Returns the JSON answer.
async fn light_candidates(state: &AppState, id: &str, node_id: &str, cands: &[u32], method: &str) -> ApiResult<Value> {
    let signals = svc::assign_signals(cands);
    let mut method = method.to_string();
    if method == "identify" {
        let lights = signals
            .iter()
            .map(|(jack, color, blinks)| IdentifyLight {
                node_id: node_id.into(),
                output: (jack - 1) * 4 + 1,
                color: (*color).into(),
                blinks: *blinks,
            })
            .collect();
        let req = TestRequest {
            mode: "identify".into(),
            identify: Some(lights),
            ..raw_test(node_id, 1, "identify", None)
        };
        let req = TestRequest {
            target: TestTarget::default(),
            ..req
        };
        match start_test(state, &session_key(id), req, Some(LIGHT_TIMEOUT), "Jack identification").await {
            Ok(()) => {}
            Err(e) if e.status == axum::http::StatusCode::SERVICE_UNAVAILABLE => {
                method = "sequential".into();
            }
            Err(e) => return Err(e),
        }
    }
    if method == "sequential" {
        light_jack(state, id, node_id, cands[0]).await?;
    }
    Ok(json!({
        "sessionId": id,
        "method": method,
        "round": if cands.len() > svc::SIGNALS.len() { "first" } else { "final" },
        "candidates": signals
            .iter()
            .map(|(jack, color, blinks)| json!({ "jack": jack, "color": color, "blinks": blinks }))
            .collect::<Vec<_>>(),
        "probeJack": (method == "sequential").then(|| cands[0]),
    }))
}

/// Light port 1 of `jack` steady (sequential method).
async fn light_jack(state: &AppState, id: &str, node_id: &str, jack: u32) -> ApiResult<()> {
    start_test(
        state,
        &session_key(id),
        raw_test(node_id, (jack - 1) * 4 + 1, "solid", Some("#707070")),
        Some(LIGHT_TIMEOUT),
        "Jack identification",
    )
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentifyBody {
    node_id: String,
    #[serde(default)]
    force: bool,
}

async fn identify_jack(State(state): State<AppState>, Json(b): Json<IdentifyBody>) -> ApiResult<Json<Value>> {
    svc::expire_sessions(&state);
    let show = state.store.get();
    let node = show
        .node(&b.node_id)
        .ok_or_else(|| ApiError::not_found("That controller"))?;
    if node.board.jack_count() == 0 && node.outputs.len() < 4 {
        return Err(ApiError::bad_request(format!(
            "{} has no receiver jacks; wire props to its outputs directly.",
            node.name
        )));
    }
    let free = svc::free_jacks(&show, &b.node_id);
    if free.is_empty() {
        return Err(ApiError::bad_request(format!(
            "Every jack on {} already has a receiver.",
            node.name
        )));
    }
    check_idle(&state, b.force)?;
    let id = pixelplus_core::model::new_id();
    let v = light_candidates(&state, &id, &b.node_id, &free, "identify").await?;
    state.services.mapping.wizards.lock().insert(
        id,
        WizardSession {
            node_id: b.node_id.clone(),
            candidates: free,
            jack: None,
            method: v["method"].as_str().unwrap_or("identify").into(),
            last_used: Instant::now(),
        },
    );
    Ok(Json(v))
}

fn with_session<R>(state: &AppState, id: &str, f: impl FnOnce(&mut WizardSession) -> ApiResult<R>) -> ApiResult<R> {
    let mut map = state.services.mapping.wizards.lock();
    let s = map.get_mut(id).ok_or_else(no_session)?;
    s.last_used = Instant::now();
    f(s)
}

#[derive(Deserialize)]
struct PickBody {
    color: String,
    blinks: u8,
}

/// The user saw signal `{color, blinks}` on the new receiver.
async fn pick(State(state): State<AppState>, Path(id): Path<String>, Json(b): Json<PickBody>) -> ApiResult<Json<Value>> {
    let (node, left, method) = with_session(&state, &id, |s| {
        let left: Vec<u32> = svc::assign_signals(&s.candidates)
            .into_iter()
            .filter(|(_, c, n)| c.eq_ignore_ascii_case(&b.color) && *n == b.blinks)
            .map(|(j, _, _)| j)
            .collect();
        if left.is_empty() {
            return Err(ApiError::bad_request("None of the jacks shows that signal."));
        }
        s.candidates = left.clone();
        if left.len() == 1 {
            s.jack = Some(left[0]);
        }
        Ok((s.node_id.clone(), left, s.method.clone()))
    })?;
    if left.len() == 1 {
        stop_pattern(&state, Some(&session_key(&id))).await;
        return Ok(Json(json!({ "done": true, "jack": left[0] })));
    }
    let mut v = light_candidates(&state, &id, &node, &left, &method).await?;
    v["done"] = json!(false);
    Ok(Json(v))
}

#[derive(Deserialize)]
struct JackBody {
    jack: u32,
}

/// Sequential method: light port 1 of `jack` ("Is it lit now?").
async fn probe(State(state): State<AppState>, Path(id): Path<String>, Json(b): Json<JackBody>) -> ApiResult<Json<Value>> {
    let node = with_session(&state, &id, |s| {
        if !s.candidates.contains(&b.jack) {
            return Err(ApiError::bad_request("That jack isn't free."));
        }
        Ok(s.node_id.clone())
    })?;
    light_jack(&state, &id, &node, b.jack).await?;
    Ok(Json(json!({ "ok": true, "jack": b.jack })))
}

/// Choose the jack directly (the user knows it, or the sequential probe lit).
async fn set_jack(State(state): State<AppState>, Path(id): Path<String>, Json(b): Json<JackBody>) -> ApiResult<Json<Value>> {
    let show = state.store.get();
    with_session(&state, &id, |s| {
        if !svc::free_jacks(&show, &s.node_id).contains(&b.jack) {
            return Err(ApiError::bad_request(format!("Jack {} already has a receiver.", b.jack)));
        }
        s.jack = Some(b.jack);
        Ok(())
    })?;
    stop_pattern(&state, Some(&session_key(&id))).await;
    Ok(Json(json!({ "ok": true, "jack": b.jack })))
}

#[derive(Deserialize, Default)]
struct LightBody {
    /// "solid" (default) | "chase" | "red" | "green" | "blue" | "off"
    #[serde(default)]
    pattern: Option<String>,
}

async fn light_port(
    State(state): State<AppState>,
    Path((id, port)): Path<(String, u32)>,
    body: axum::body::Bytes,
) -> ApiResult<Json<Value>> {
    let b: LightBody = if body.is_empty() {
        LightBody::default()
    } else {
        serde_json::from_slice(&body).map_err(|e| ApiError::bad_request(format!("Bad request: {e}")))?
    };
    let (node, jack) = with_session(&state, &id, |s| {
        Ok((s.node_id.clone(), s.jack.ok_or_else(|| ApiError::bad_request("Find the jack first."))?))
    })?;
    if !(1..=4).contains(&port) {
        return Err(ApiError::bad_request("Ports are 1–4."));
    }
    let output = (jack - 1) * 4 + port;
    let pattern = b.pattern.as_deref().unwrap_or("solid");
    let (mode, color) = match pattern {
        "solid" => ("solid", Some("#707070")),
        "chase" => ("chase", Some("#909090")),
        "red" => ("solid", Some("#a00000")),
        "green" => ("solid", Some("#00a000")),
        "blue" => ("solid", Some("#0000a0")),
        "off" => {
            stop_pattern(&state, Some(&session_key(&id))).await;
            return Ok(Json(json!({ "ok": true })));
        }
        p => return Err(ApiError::bad_request(format!("Unknown pattern \"{p}\"."))),
    };
    start_test(&state, &session_key(&id), raw_test(&node, output, mode, color), Some(LIGHT_TIMEOUT), "Port lighting").await?;
    Ok(Json(json!({ "ok": true, "output": output })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ColorBody {
    port: u32,
    /// What pure red looked like: "red" | "green" | "blue".
    red: String,
    green: String,
}

fn color_index(c: &str) -> Option<usize> {
    match c {
        "red" => Some(0),
        "green" => Some(1),
        "blue" => Some(2),
        _ => None,
    }
}

async fn color_order(State(state): State<AppState>, Path(id): Path<String>, Json(b): Json<ColorBody>) -> ApiResult<Json<Value>> {
    let (node, jack) = with_session(&state, &id, |s| {
        Ok((s.node_id.clone(), s.jack.ok_or_else(|| ApiError::bad_request("Find the jack first."))?))
    })?;
    let show = state.store.get();
    let output = (jack - 1) * 4 + b.port;
    let configured = show
        .node(&node)
        .and_then(|n| n.outputs.iter().find(|o| o.index == output))
        .map(|o| o.color_order)
        .unwrap_or_default();
    let (Some(r), Some(g)) = (color_index(&b.red), color_index(&b.green)) else {
        return Err(ApiError::bad_request("Answer red, green or blue."));
    };
    let order = svc::detect_color_order(configured, r, g)
        .ok_or_else(|| ApiError::bad_request("Red and green can't look the same; try again."))?;
    Ok(Json(json!({ "colorOrder": order, "configured": configured, "changed": order != configured })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewReceiver {
    name: String,
    #[serde(default = "diffrx")]
    kind: ReceiverKind,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    fuse_amps: Option<f32>,
    #[serde(default)]
    main_fuse_amps: Option<f32>,
}

fn diffrx() -> ReceiverKind {
    ReceiverKind::Diffrx
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PortPlan {
    port: u32,
    #[serde(default)]
    prop_ids: Vec<String>,
    #[serde(default)]
    reverse: bool,
    #[serde(default)]
    color_order: Option<ColorOrder>,
}

#[derive(Deserialize)]
struct FinishBody {
    receiver: NewReceiver,
    #[serde(default)]
    ports: Vec<PortPlan>,
}

/// Create the receiver and wire its ports (pure; tested).
fn finish_show(show: &mut Show, node_id: &str, jack: u32, b: &FinishBody) -> ApiResult<Receiver> {
    let name = b.receiver.name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return Err(ApiError::bad_request("Give the receiver a name (up to 60 characters)."));
    }
    if !svc::free_jacks(show, node_id).contains(&jack) {
        return Err(ApiError::bad_request(format!("Jack {jack} already has a receiver.")));
    }
    let kind = b.receiver.kind;
    let rx = Receiver {
        id: pixelplus_core::model::new_id(),
        name: name.to_string(),
        kind,
        node_id: node_id.into(),
        jack,
        location: b.receiver.location.clone().filter(|l| !l.trim().is_empty()),
        fuse_amps: b.receiver.fuse_amps.or(kind.default_fuse_amps()),
        notes: None,
        main_fuse_amps: b.receiver.main_fuse_amps,
    };
    let mut used = std::collections::HashSet::new();
    for p in &b.ports {
        if p.port == 0 || p.port as usize > kind.port_count() {
            return Err(ApiError::bad_request(format!("Port {} doesn't exist.", p.port)));
        }
        let output = rx.output_for_port(p.port);
        let node = show
            .nodes
            .iter_mut()
            .find(|n| n.id == node_id)
            .ok_or_else(|| ApiError::not_found("That controller"))?;
        let out = node
            .outputs
            .iter_mut()
            .find(|o| o.index == output)
            .ok_or_else(|| ApiError::bad_request(format!("Output {output} doesn't exist.")))?;
        if let Some(order) = p.color_order {
            out.color_order = order;
        }
        let mut start = 0;
        for pid in &p.prop_ids {
            if !used.insert(pid.clone()) {
                return Err(ApiError::bad_request("A prop can only be on one port."));
            }
            let prop = show
                .props
                .iter_mut()
                .find(|x| &x.id == pid)
                .ok_or_else(|| ApiError::not_found("A chosen prop"))?;
            prop.segments = vec![PropSegment {
                node_id: node_id.into(),
                output,
                start_pixel: start,
                pixel_count: prop.pixel_count,
                prop_offset: 0,
                reverse: p.reverse,
                null_pixels: 0,
            }];
            start += prop.pixel_count;
        }
    }
    show.receivers.push(rx.clone());
    svc::check_overlaps(show)?;
    Ok(rx)
}

async fn finish(State(state): State<AppState>, Path(id): Path<String>, Json(b): Json<FinishBody>) -> ApiResult<Json<Value>> {
    let (node, jack) = with_session(&state, &id, |s| {
        Ok((s.node_id.clone(), s.jack.ok_or_else(|| ApiError::bad_request("Find the jack first."))?))
    })?;
    finish_show(&mut (*state.store.get()).clone(), &node, jack, &b)?;
    let snap = crate::services::snapshots::create(&state, "Before adding a receiver", true, false).await?;
    let (rx, show) = state.store.update(move |s| finish_show(s, &node, jack, &b)).await?;
    state.services.mapping.wizards.lock().remove(&id);
    stop_pattern(&state, Some(&session_key(&id))).await;
    Ok(Json(json!({ "show": &*show, "receiver": rx, "snapshotId": snap.id })))
}

async fn cancel(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    state.services.mapping.wizards.lock().remove(&id);
    stop_pattern(&state, Some(&session_key(&id))).await;
    Ok(Json(json!({ "ok": true })))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/wizard/receiver/identify-jack", post(identify_jack))
        .route("/wizard/receiver/{id}/pick", post(pick))
        .route("/wizard/receiver/{id}/probe", post(probe))
        .route("/wizard/receiver/{id}/jack", post(set_jack))
        .route("/wizard/receiver/{id}/port/{n}/light", post(light_port))
        .route("/wizard/receiver/{id}/color-order", post(color_order))
        .route("/wizard/receiver/{id}/finish", post(finish))
        .route("/wizard/receiver/{id}/cancel", post(cancel))
}

#[cfg(test)]
mod tests {
    use crate::api::testkit::TestApp;
    use axum::http::StatusCode;
    use serde_json::json;

    #[tokio::test]
    async fn wizard_flow() {
        let app = TestApp::new();
        let mut show = crate::services::mapping::tests_support::show();
        // An unwired prop to place on the new receiver.
        let mut d = show.props[2].clone();
        d.id = "d".into();
        d.segments.clear();
        show.props.push(d);
        app.state.store.replace(show).await.unwrap();

        let (s, v) = app
            .json("POST", "/wizard/receiver/identify-jack", Some(json!({"nodeId": "lead"})))
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let id = v["sessionId"].as_str().unwrap().to_string();
        assert_eq!(v["method"], "identify");
        assert_eq!(v["round"], "first");
        assert_eq!(v["candidates"].as_array().unwrap().len(), 14);
        assert!(app.commands_matching("identify: Some(").await >= 1);
        // Jack 10 (9th free: shares the first signal with jack 2).
        let c = &v["candidates"][8];
        assert_eq!(c["jack"], 10);
        let (s, v) = app
            .json("POST", &format!("/wizard/receiver/{id}/pick"), Some(json!({"color": c["color"], "blinks": c["blinks"]})))
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["done"], false);
        let c = v["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["jack"] == 10)
            .unwrap()
            .clone();
        let (_, v) = app
            .json("POST", &format!("/wizard/receiver/{id}/pick"), Some(json!({"color": c["color"], "blinks": c["blinks"]})))
            .await;
        assert_eq!(v["done"], true);
        assert_eq!(v["jack"], 10);

        let (s, v) = app.json("POST", &format!("/wizard/receiver/{id}/port/2/light"), Some(json!({"pattern": "chase"}))).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["output"], 38);
        let (_, v) = app
            .json("POST", &format!("/wizard/receiver/{id}/color-order"), Some(json!({"port": 2, "red": "green", "green": "red"})))
            .await;
        assert_eq!(v["colorOrder"], "GRB");

        let body = json!({
            "receiver": {"name": "Porch", "kind": "diffrx"},
            "ports": [{"port": 2, "propIds": ["d"], "reverse": true, "colorOrder": "GRB"}]
        });
        let (s, v) = app.json("POST", &format!("/wizard/receiver/{id}/finish"), Some(body)).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let show = app.state.store.get();
        let rx = show.receivers.iter().find(|r| r.name == "Porch").unwrap();
        assert_eq!((rx.jack, rx.fuse_amps), (10, Some(6.0)));
        let seg = &show.prop("d").unwrap().segments[0];
        assert_eq!((seg.output, seg.start_pixel, seg.reverse), (38, 0, true));
        assert_eq!(
            show.node("lead").unwrap().outputs[37].color_order,
            pixelplus_core::model::ColorOrder::GRB
        );
        let (s, _) = app.json("POST", &format!("/wizard/receiver/{id}/cancel"), None).await;
        assert_eq!(s, StatusCode::OK);
        let (s, _) = app.json("POST", &format!("/wizard/receiver/{id}/port/1/light"), None).await;
        assert_eq!(s, StatusCode::CONFLICT);
    }
}
