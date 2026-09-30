//! `/system/*`: host info, first-run setup, power actions, logs, network,
//! sensors, board EEPROM, updates, audio devices.

use super::content::player;
use super::playerapi::body_or_default;
use super::{ApiError, ApiResult, Peer};
use crate::node::LocalRole;
use crate::services::{geometry, network, platform, setup, system as sys};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use pixelplus_core::model::{BoardKind, Location};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

async fn info(State(state): State<AppState>, peer: Peer, headers: HeaderMap) -> Json<Value> {
    let authed = super::auth::is_authenticated(&state, &headers, peer.0);
    Json(sys::system_info(&state, authed).await)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetupBody {
    role: String,
    #[serde(default)]
    show_name: Option<String>,
    /// This controller's friendly name.
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    board: Option<BoardKind>,
    #[serde(default)]
    board_rev: Option<String>,
    #[serde(default)]
    location: Option<Location>,
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    write_eeprom: bool,
}

async fn setup(State(state): State<AppState>, Json(b): Json<SetupBody>) -> ApiResult<Response> {
    let Some(role) = setup::parse_role(&b.role) else {
        return Err(ApiError::bad_request(
            "Choose whether this controller runs the show (leader) or joins one (follower).",
        ));
    };
    let req = setup::SetupRequest {
        role: Some(role),
        show_name: b.show_name,
        name: b.name,
        board: b.board,
        board_rev: b.board_rev,
        location: b.location,
        timezone: b.timezone,
        password: b.password,
        write_eeprom: b.write_eeprom,
    };
    let out = setup::apply(&state, req).await?;
    for n in &out.notes {
        state
            .events
            .toast(crate::events::ToastKind::Warning, n.clone());
    }
    let mut body = sys::system_info(&state, true).await;
    body["notes"] = json!(out.notes);
    let mut resp = Json(body).into_response();
    // Keep the person who just chose the password signed in.
    if out.password_set {
        let token = state.sessions.create();
        let cookie = format!(
            "pp_session={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
            60 * 60 * 24 * 30
        );
        if let Ok(v) = HeaderValue::from_str(&cookie) {
            resp.headers_mut().insert(header::SET_COOKIE, v);
        }
    }
    Ok(resp)
}

async fn power(state: AppState, action: sys::PowerAction) -> ApiResult<Json<Value>> {
    let msg = platform::power_action(&state, action)?;
    if action != sys::PowerAction::RestartService {
        // Lights off before the computer goes away.
        if let Ok(p) = player(&state) {
            let _ = p.send(crate::player::PlayerCmd::Stop { fade: false }).await;
        }
    }
    state.events.toast(crate::events::ToastKind::Info, msg);
    Ok(Json(json!({ "ok": true, "message": msg })))
}

async fn reboot(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    power(state, sys::PowerAction::Reboot).await
}
async fn shutdown(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    power(state, sys::PowerAction::Shutdown).await
}
async fn restart_service(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    power(state, sys::PowerAction::RestartService).await
}

#[derive(Deserialize)]
struct LinesQuery {
    #[serde(default)]
    lines: Option<usize>,
}

async fn logs(Query(q): Query<LinesQuery>) -> Response {
    let text = sys::logs_text(q.lines.unwrap_or(500)).await;
    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response()
}

async fn get_network() -> Json<network::NetworkConfig> {
    Json(network::read_config().await)
}

async fn put_network(
    State(state): State<AppState>,
    Json(cfg): Json<network::NetworkConfig>,
) -> ApiResult<Json<network::NetworkConfig>> {
    network::apply(cfg, state).await.map(Json)
}

async fn scan() -> ApiResult<Json<Vec<network::WifiNetwork>>> {
    network::scan().await.map(Json)
}

async fn sensors(State(state): State<AppState>) -> Json<Vec<crate::services::sensors::Reading>> {
    Json(state.services.sensors.latest())
}

#[derive(Deserialize)]
struct MinutesQuery {
    #[serde(default)]
    minutes: Option<u32>,
}

async fn sensor_history(
    State(state): State<AppState>,
    Query(q): Query<MinutesQuery>,
) -> Json<Value> {
    Json(json!({ "series": state.services.sensors.history(q.minutes.unwrap_or(60)) }))
}

#[derive(Deserialize)]
struct EepromBody {
    board: BoardKind,
    rev: String,
}

async fn eeprom(
    State(state): State<AppState>,
    Json(b): Json<EepromBody>,
) -> ApiResult<Json<Value>> {
    if !matches!(
        b.board,
        BoardKind::Difftx | BoardKind::Difftxlarge | BoardKind::Diffsmart
    ) {
        return Err(ApiError::bad_request(
            "Only PixelPlus boards have an EEPROM to write.",
        ));
    }
    let rev = b.rev.trim().to_ascii_uppercase();
    if !setup::valid_rev(&rev) {
        return Err(ApiError::bad_request(
            "The board revision is a letter printed on the board, like E.",
        ));
    }
    let (_, pi) = sys::detection();
    if pi.is_none() {
        return Err(ApiError::bad_request(
            "The board EEPROM can only be written on a Raspberry Pi with the board fitted.",
        ));
    }
    setup::write_eeprom(b.board, rev.clone()).await.map_err(|e| {
        ApiError::bad_request(format!(
            "Couldn't write the board EEPROM: {e}. Check the board is seated and its write-protect jumper is closed."
        ))
    })?;
    // The EEPROM is now authoritative: drop any wizard override.
    state
        .set_identity(|id| {
            id.board = None;
            id.board_rev = None;
        })
        .map_err(ApiError::internal)?;
    if state.identity().role == LocalRole::Leader {
        let _ = crate::cluster::ensure_self_node(&state).await;
    }
    platform::publish_board(&state);
    let msg = format!("EEPROM written: {} rev {rev}", b.board.display_name());
    state
        .events
        .toast(crate::events::ToastKind::Success, msg.clone());
    Ok(Json(json!({ "ok": true, "message": msg })))
}

async fn update_check(State(state): State<AppState>) -> Json<crate::services::updates::UpdateInfo> {
    Json(crate::services::updates::check(&state).await)
}

async fn update_apply(State(state): State<AppState>, _body: Bytes) -> ApiResult<Json<Value>> {
    let msg = crate::services::updates::apply(&state).await?;
    state
        .events
        .toast(crate::events::ToastKind::Info, msg.clone());
    let job = state.services.helpers.get("update");
    Ok(Json(json!({ "ok": true, "message": msg, "job": job })))
}

/// Progress of the root helper jobs started since the daemon started.
async fn helpers(State(state): State<AppState>) -> Json<Vec<platform::HelperStatus>> {
    Json(state.services.helpers.all())
}

async fn get_ssh(State(state): State<AppState>) -> Json<Value> {
    Json(ssh_state(&state).await)
}

async fn ssh_state(state: &AppState) -> Value {
    let enabled = if sys::has_systemd() && sys::have("systemctl") && !sys::in_docker() {
        match sys::run(
            "systemctl",
            &["is-enabled", "ssh.service"],
            Duration::from_secs(5),
        )
        .await
        {
            Ok(o) => match o.stdout.trim() {
                "enabled" | "enabled-runtime" | "alias" => Some(true),
                "disabled" | "masked" | "static" | "indirect" => Some(false),
                _ => None,
            },
            Err(_) => None,
        }
    } else {
        None
    };
    let can_change = enabled.is_some() && (platform::helper_installed() || sys::is_root());
    json!({
        "enabled": enabled,
        "canChange": can_change,
        "job": state.services.helpers.get("ssh-on").into_iter()
            .chain(state.services.helpers.get("ssh-off"))
            .max_by_key(|s| s.updated_at),
    })
}

#[derive(Deserialize)]
struct SshBody {
    enabled: bool,
}

async fn put_ssh(State(state): State<AppState>, Json(b): Json<SshBody>) -> ApiResult<Json<Value>> {
    let verb = if b.enabled {
        platform::HelperVerb::SshOn
    } else {
        platform::HelperVerb::SshOff
    };
    let job = platform::run_helper(&state, verb, platform::HelperOpts::default()).await?;
    Ok(Json(json!({ "ok": true, "job": job.status })))
}

/// Re-apply /boot/firmware/pixelplus.txt now (root helper `reapply`).
async fn reapply(State(state): State<AppState>, _body: Bytes) -> ApiResult<Json<Value>> {
    let job = platform::run_helper(
        &state,
        platform::HelperVerb::Reapply,
        platform::HelperOpts::default(),
    )
    .await?;
    Ok(Json(json!({ "ok": true, "job": job.status })))
}

async fn get_geometry(State(state): State<AppState>) -> Json<geometry::OutputGeometry> {
    Json(geometry::status(&state))
}

#[derive(Deserialize, Default)]
struct GeometryApplyBody {
    #[serde(default = "yes")]
    reboot: bool,
}

fn yes() -> bool {
    true
}

async fn apply_geometry(State(state): State<AppState>, body: Bytes) -> ApiResult<Json<Value>> {
    let b: GeometryApplyBody = if body.is_empty() {
        GeometryApplyBody { reboot: true }
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| ApiError::bad_request(format!("Invalid request: {e}")))?
    };
    let job = geometry::apply(&state, b.reboot).await?;
    Ok(Json(
        json!({ "ok": true, "job": job, "geometry": geometry::status(&state) }),
    ))
}

async fn audio_devices() -> Json<Vec<Value>> {
    Json(sys::audio_devices().await)
}

async fn identify_self(State(state): State<AppState>, body: Bytes) -> ApiResult<Json<Value>> {
    // Flash all props briefly so the user can see which controller this is.
    let v: Value = body_or_default(&body)?;
    let secs = v["seconds"].as_u64().unwrap_or(5).clamp(1, 30);
    let p = player(&state)?.clone();
    let req = crate::player::TestRequest {
        map_run_id: Default::default(),
        cal: Default::default(),
        identify: Default::default(),
        map: Default::default(),
        mode: "rgbCycle".into(),
        color: None,
        speed: None,
        target: crate::player::TestTarget {
            props: pixelplus_core::model::Target {
                all: true,
                ..Default::default()
            },
            ..Default::default()
        },
        effect: None,
    };
    p.test_start(req).await?;
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(secs)).await;
        let _ = p.send(crate::player::PlayerCmd::TestStop).await;
    });
    Ok(Json(json!({ "ok": true })))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/system", get(info))
        .route("/system/setup", post(setup))
        .route("/system/reboot", post(reboot))
        .route("/system/shutdown", post(shutdown))
        .route("/system/restart-service", post(restart_service))
        .route("/system/logs", get(logs))
        .route("/system/network", get(get_network).put(put_network))
        .route("/system/network/scan", get(scan))
        .route("/system/sensors", get(sensors))
        .route("/system/sensors/history", get(sensor_history))
        .route("/system/eeprom", post(eeprom))
        .route("/system/update", get(update_check).post(update_apply))
        .route("/system/audio/devices", get(audio_devices))
        .route("/system/helpers", get(helpers))
        .route("/system/ssh", get(get_ssh).put(put_ssh))
        .route("/system/reapply", post(reapply))
        .route("/system/output-geometry", get(get_geometry))
        .route("/system/output-geometry/apply", post(apply_geometry))
        .route("/system/identify", post(identify_self))
}
