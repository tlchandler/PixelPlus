//! Sensor nodes (F20, ARCHITECTURE §12.16).
//!
//! * `GET /sensor-nodes` → `SensorNode[]`; `GET /sensor-nodes/:id`
//! * `GET /sensor-nodes/discovered` → nodes announcing themselves
//! * `POST /sensor-nodes/adopt {id}` → the adopted `SensorNode`
//! * `PUT /sensor-nodes/:id` (merge patch: name, location, inputs)
//! * `POST /sensor-nodes/:id/release`, `DELETE /sensor-nodes/:id` → `{ok, message?}`
//! * `POST /sensor-nodes/:id/identify` → blink its LED
//! * `GET /sensor-nodes/:id/live`, `GET /sensor-nodes/live` → live inputs,
//!   signal, current readings
//! * `POST /surprises/test {action}` → run a trigger action now (F20 surprise)
//! * `GET /cluster/sensor-config/:id` → the node's configuration, signed by
//!   the node (`X-PixelPlus-Auth`), answered with `X-PixelPlus-Reply`
//!
//! The logic lives in `services/sensornodes.rs`.

use super::crud::merge_patch;
use super::{ApiError, ApiResult};
use crate::cluster::sig;
use crate::services::sensornodes::{self as svc, DiscoveredSensorNode, Live};
use crate::state::AppState;
use axum::extract::{OriginalUri, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use pixelplus_core::model::{SensorNode, TriggerAction};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

async fn list(State(state): State<AppState>) -> Json<Vec<SensorNode>> {
    Json(state.store.get().sensor_nodes.clone())
}

async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<SensorNode>> {
    state
        .store
        .get()
        .sensor_nodes
        .iter()
        .find(|n| n.id == id)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::not_found("That sensor"))
}

async fn discovered(State(state): State<AppState>) -> Json<Vec<DiscoveredSensorNode>> {
    Json(svc::discovered(&state))
}

#[derive(Deserialize)]
struct AdoptBody {
    id: String,
}

async fn adopt(
    State(state): State<AppState>,
    Json(body): Json<AdoptBody>,
) -> ApiResult<Json<SensorNode>> {
    Ok(Json(svc::adopt(&state, &body.id).await?))
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(patch): Json<Value>,
) -> ApiResult<Json<SensorNode>> {
    let (node, _) = state
        .store
        .update(move |s| {
            let n = s
                .sensor_nodes
                .iter_mut()
                .find(|n| n.id == id)
                .ok_or_else(|| ApiError::not_found("That sensor"))?;
            let mut value = serde_json::to_value(&*n).map_err(ApiError::internal)?;
            merge_patch(&mut value, &patch);
            let mut new: SensorNode = serde_json::from_value(value)
                .map_err(|e| ApiError::bad_request(format!("Those settings aren't valid: {e}")))?;
            // Identity and adoption are not edited here.
            new.id = n.id.clone();
            new.adopted = n.adopted;
            new.hw = n.hw.clone();
            new.name = new.name.trim().to_string();
            new.location = new
                .location
                .map(|l| l.trim().chars().take(80).collect::<String>())
                .filter(|l| !l.is_empty());
            svc::validate_node(&new)?;
            *n = new.clone();
            Ok(new)
        })
        .await?;
    Ok(Json(node))
}

async fn release(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let note = svc::release(&state, &id).await?;
    Ok(Json(match note {
        Some(m) => json!({ "ok": true, "message": m }),
        None => json!({ "ok": true }),
    }))
}

async fn identify(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    svc::command(&state, &id, "identify").await?;
    Ok(Json(json!({ "ok": true })))
}

async fn live_one(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Live>> {
    if !state.store.get().sensor_nodes.iter().any(|n| n.id == id) {
        return Err(ApiError::not_found("That sensor"));
    }
    Ok(Json(svc::live(&state, &id)))
}

async fn live_all(State(state): State<AppState>) -> Json<BTreeMap<String, Live>> {
    let show = state.store.get();
    Json(
        show.sensor_nodes
            .iter()
            .map(|n| (n.id.clone(), svc::live(&state, &n.id)))
            .collect(),
    )
}

#[derive(Deserialize)]
struct SurpriseBody {
    action: TriggerAction,
}

async fn surprise_test(
    State(state): State<AppState>,
    Json(body): Json<SurpriseBody>,
) -> ApiResult<Json<Value>> {
    let msg = crate::services::triggers::run_action(&state, &body.action).await?;
    Ok(Json(json!({ "ok": true, "message": msg })))
}

/// 401 for a refused signed request (with our clock when only the time was off).
fn refusal(r: sig::Refusal, hint: Option<(String, String)>) -> Response {
    let (code, msg) = match r {
        sig::Refusal::Unauthenticated => ("sensor_auth", "Missing or wrong sensor signature."),
        sig::Refusal::Skew { .. } => ("clock_skew", "The sensor's clock is off."),
        sig::Refusal::Replay => ("replay", "That request was already used."),
    };
    let mut resp = ApiError::new(StatusCode::UNAUTHORIZED, code, msg).into_response();
    if let (sig::Refusal::Skew { now }, Some((key, nonce))) = (r, hint) {
        if let Ok(v) = HeaderValue::from_str(&sig::time_proof(&key, now, &nonce)) {
            resp.headers_mut().insert(sig::TIME_HEADER, v);
        }
    }
    resp
}

async fn sensor_config(
    State(state): State<AppState>,
    Path(id): Path<String>,
    uri: OriginalUri,
    headers: HeaderMap,
) -> Response {
    let path = uri
        .0
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| uri.0.path().to_string());
    let auth = headers.get(sig::AUTH_HEADER).and_then(|v| v.to_str().ok());
    match svc::signed_config(&state, &id, auth, &path) {
        Ok((cfg, key, nonce)) => {
            let body = serde_json::to_vec(&cfg).unwrap_or_default();
            let mac = sig::reply_mac(&key, &nonce, &sig::sha256_hex(&body));
            let mut resp = (
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                body,
            )
                .into_response();
            if let Ok(v) = HeaderValue::from_str(&mac) {
                resp.headers_mut().insert(sig::REPLY_HEADER, v);
            }
            resp
        }
        Err((r, hint)) => refusal(r, hint),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/sensor-nodes", get(list))
        .route("/sensor-nodes/discovered", get(discovered))
        .route("/sensor-nodes/live", get(live_all))
        .route("/sensor-nodes/adopt", post(adopt))
        .route(
            "/sensor-nodes/{id}",
            get(get_one).put(update).patch(update).delete(release),
        )
        .route("/sensor-nodes/{id}/release", post(release))
        .route("/sensor-nodes/{id}/identify", post(identify))
        .route("/sensor-nodes/{id}/live", get(live_one))
        .route("/surprises/test", post(surprise_test))
        .route("/cluster/sensor-config/{id}", get(sensor_config))
}

#[cfg(test)]
mod tests {
    use crate::api::testkit::TestApp;
    use crate::cluster::sig;
    use crate::services::sensornodes as svc;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::json;

    const KEY: &str = "7b8101026207edce5fd1255f3fa781b80b6c560cc0908416d605f3f47e81b32d";
    const ID: &str = "sn9c1e2a00";

    /// Pretend the node was adopted (the HTTP key exchange needs a device).
    async fn adopted(app: &TestApp) {
        std::fs::write(
            app.dir.join("sensor-keys.json"),
            json!({ ID: KEY }).to_string(),
        )
        .unwrap();
        app.state
            .store
            .update(|s| {
                s.sensor_nodes.push(
                    serde_json::from_value(json!({
                        "id": ID, "name": "Sidewalk", "hw": "esp32c3", "adopted": true,
                        "inputs": [{"id":"pir1","name":"Motion 1","pin":4,"kind":"motion"}]
                    }))
                    .unwrap(),
                );
                s.settings.triggers.push(
                    serde_json::from_value(json!({
                        "id": "trig000001", "name": "Sidewalk sparkle", "kind": "sensor",
                        "sensor": {"sensorNodeId": ID, "input": "pir1"},
                        "action": {"type": "stop"}
                    }))
                    .unwrap(),
                );
                Ok(())
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn crud_live_and_events_over_udp_path() {
        let app = TestApp::new();
        adopted(&app).await;
        let (st, list) = app.json("GET", "/sensor-nodes", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(list[0]["id"], ID);

        // Edit inputs: invalid pin rejected, valid accepted, id kept.
        let (st, _) = app
            .json(
                "PUT",
                &format!("/sensor-nodes/{ID}"),
                Some(json!({"inputs": [{"id":"pir1","name":"x","pin":99,"kind":"motion"}]})),
            )
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let (st, n) = app
            .json(
                "PUT",
                &format!("/sensor-nodes/{ID}"),
                Some(json!({"id": "hijack", "name": " Porch ", "location": "Front door"})),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{n}");
        assert_eq!(
            (n["id"].as_str(), n["name"].as_str()),
            (Some(ID), Some("Porch"))
        );

        // A beacon shows up in discovery (not adopted by us → listed).
        let from: std::net::SocketAddr = "192.168.1.81:32422".parse().unwrap();
        let beacon = json!({"t":"sbeacon","id":"sn00000001","name":"PixelPlus-Sensor-0001","hw":"esp32c3","ver":"0.1.0","http":80,"adoptedBy":null,"inputs":["pir1"],"proto":1});
        assert!(
            svc::on_datagram(&app.state, beacon.to_string().as_bytes(), from)
                .await
                .is_none()
        );
        let (_, d) = app.json("GET", "/sensor-nodes/discovered", None).await;
        assert_eq!(d[0]["id"], "sn00000001");
        assert_eq!(d[0]["ip"], "192.168.1.81");

        // Heartbeat → live; an event → trigger fires (a "stop" reaches the player).
        let mut seq = 0u64;
        let mut stamp = |v: serde_json::Value| {
            seq += 1;
            svc::seal(v.to_string().as_bytes(), KEY, "e5f00d01", seq)
        };
        let reply = svc::on_datagram(
            &app.state,
            &stamp(json!({"t":"sstatus","id":ID,"rssi":-58,"uptime":12,"ver":"0.1.0","inputs":{"pir1":0},"amps":{"ina1":1.25}})),
            from,
        )
        .await
        .unwrap();
        let ack: serde_json::Value = serde_json::from_slice(&reply).unwrap();
        let lb = ack["lb"].as_str().unwrap().to_string();
        let (_, live) = app
            .json("GET", &format!("/sensor-nodes/{ID}/live"), None)
            .await;
        assert_eq!(live["online"], true);
        assert_eq!(live["rssi"], -58);
        assert_eq!(
            svc::amps(
                &app.state,
                &pixelplus_core::model::SensorRef {
                    sensor_node_id: ID.into(),
                    input: "ina1".into()
                }
            ),
            Some(1.25)
        );
        let mut events = app.state.events.subscribe();
        svc::on_datagram(
            &app.state,
            &stamp(json!({"t":"sevent","id":ID,"input":"pir1","state":1,"ms":5,"lb":lb})),
            from,
        )
        .await
        .unwrap();
        let mut got_ws = false;
        while let Ok(Ok(ev)) =
            tokio::time::timeout(std::time::Duration::from_millis(200), events.recv()).await
        {
            if let crate::events::Event::Json {
                kind: "sensorInput",
                data,
            } = ev
            {
                assert_eq!(data["input"], "pir1");
                got_ws = true;
                break;
            }
        }
        assert!(got_ws, "sensorInput WebSocket message");
        assert_eq!(
            app.commands_matching("Stop").await,
            1,
            "the sensor trigger ran"
        );
        let (_, all) = app.json("GET", "/sensor-nodes/live", None).await;
        assert_eq!(all[ID]["inputs"]["pir1"], 1);
        assert_eq!(all[ID]["events"], 1);

        // Release: device unreachable → still released locally, with a note.
        let (st, r) = app
            .json("POST", &format!("/sensor-nodes/{ID}/release"), None)
            .await;
        assert_eq!(st, StatusCode::OK);
        assert!(r["message"].as_str().unwrap().contains("BOOT"));
        let (_, list) = app.json("GET", "/sensor-nodes", None).await;
        assert!(list.as_array().unwrap().is_empty());
        let keys = std::fs::read_to_string(app.dir.join("sensor-keys.json")).unwrap();
        assert!(!keys.contains(KEY));
    }

    #[tokio::test]
    async fn sensor_config_is_signed_both_ways() {
        let app = TestApp::new();
        adopted(&app).await;
        let path = format!("/api/v1/cluster/sensor-config/{ID}");
        let get = |auth: Option<String>| {
            let mut b = Request::builder().uri(path.clone());
            if let Some(a) = auth {
                b = b.header(sig::AUTH_HEADER, a);
            }
            b.body(Body::empty()).unwrap()
        };
        // Unsigned: refused (the path is outside the login check).
        let (st, _, _) = app.send(get(None)).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        // Signed by the node.
        let (auth, nonce) = sig::sign(KEY, ID, "GET", &path, b"", sig::now_s());
        let (st, headers, body) = app.send(get(Some(auth.clone()))).await;
        assert_eq!(st, StatusCode::OK);
        let cfg: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(cfg["inputs"][0]["pin"], 4);
        assert_eq!(cfg["statusEverySec"], 10);
        assert!(sig::verify_reply(
            KEY,
            &nonce,
            &sig::sha256_hex(&body),
            headers.get(sig::REPLY_HEADER).and_then(|v| v.to_str().ok())
        ));
        // Replayed nonce refused.
        let (st, _, _) = app.send(get(Some(auth))).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        // Another node's key can't read this config.
        let (auth, _) = sig::sign(&"1".repeat(64), ID, "GET", &path, b"", sig::now_s());
        let (st, _, _) = app.send(get(Some(auth))).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        // A clock 10 minutes off gets our time, MACed for that nonce.
        let (auth, nonce) = sig::sign(KEY, ID, "GET", &path, b"", sig::now_s() - 600);
        let (st, headers, _) = app.send(get(Some(auth))).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        let hint = headers.get(sig::TIME_HEADER).unwrap().to_str().unwrap();
        assert!(sig::verify_time_proof(KEY, hint, &nonce).is_some());
    }

    #[tokio::test]
    async fn adopt_and_identify_need_a_live_device() {
        let app = TestApp::new();
        let (st, _) = app
            .json(
                "POST",
                "/sensor-nodes/adopt",
                Some(json!({"id": "sn00000009"})),
            )
            .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        adopted(&app).await;
        let (st, _) = app
            .json("POST", &format!("/sensor-nodes/{ID}/identify"), None)
            .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
    }
}
