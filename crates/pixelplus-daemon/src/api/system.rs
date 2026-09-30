//! `/system/*`: host info, first-run setup, power actions, logs, network,
//! sensors, board EEPROM, updates, audio devices.

use super::content::player;
use super::playerapi::body_or_default;
use super::{ApiError, ApiResult, Peer};
use crate::node::LocalRole;
use crate::services::{network, system as sys};
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

fn valid_rev(rev: &str) -> bool {
    !rev.is_empty() && rev.len() <= 8 && rev.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
}

async fn setup(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(b): Json<SetupBody>,
) -> ApiResult<Response> {
    let role =
        match b.role.as_str() {
            "leader" => LocalRole::Leader,
            "follower" => LocalRole::Follower,
            _ => return Err(ApiError::bad_request(
                "Choose whether this controller runs the show (leader) or joins one (follower).",
            )),
        };
    let current = state.identity();
    if current.role == LocalRole::Follower
        && current.leader_id.is_some()
        && role == LocalRole::Leader
    {
        return Err(ApiError::conflict(
            "This controller belongs to another show's leader. Release it from that leader first.",
        ));
    }
    if let Some(name) = b.show_name.as_deref() {
        if name.trim().is_empty() || name.chars().count() > 120 {
            return Err(ApiError::bad_request(
                "Give your show a name (up to 120 characters).",
            ));
        }
    }
    let tz = b
        .timezone
        .clone()
        .or_else(|| b.location.as_ref().map(|l| l.timezone.clone()))
        .filter(|t| !t.trim().is_empty());
    if let Some(tz) = &tz {
        if tz.parse::<chrono_tz::Tz>().is_err() {
            return Err(ApiError::bad_request(format!(
                "\"{tz}\" isn't a time zone PixelPlus knows. Pick your city again."
            )));
        }
    }
    if let Some(l) = &b.location {
        if !(-90.0..=90.0).contains(&l.lat) || !(-180.0..=180.0).contains(&l.lon) {
            return Err(ApiError::bad_request(
                "That location doesn't look right. Pick your city again.",
            ));
        }
    }
    if let Some(rev) = &b.board_rev {
        if !valid_rev(rev) {
            return Err(ApiError::bad_request(
                "The board revision is a letter printed on the board, like E.",
            ));
        }
    }
    let password_hash = match b.password.as_deref().filter(|p| !p.is_empty()) {
        Some(p) => Some(super::auth::hash_password(p)?),
        None => None,
    };
    let mut notes: Vec<String> = Vec::new();

    // Board: keep the wizard's choice when it differs from (or replaces) detection.
    let (det, _) = tokio::task::spawn_blocking(sys::detection)
        .await
        .map_err(ApiError::internal)?;
    if let Some(board) = b.board {
        if b.write_eeprom
            && det.board.is_none()
            && matches!(
                board,
                BoardKind::Difftx | BoardKind::Difftxlarge | BoardKind::Diffsmart
            )
        {
            let rev = b.board_rev.clone().unwrap_or_else(|| "A".into());
            match write_eeprom_blocking(board, rev).await {
                Ok(()) => {}
                Err(e) => notes.push(format!(
                    "The board EEPROM couldn't be written ({e}); your choice is saved anyway."
                )),
            }
        }
    }
    let name = b
        .name
        .clone()
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty());
    state
        .set_identity(|id| {
            id.role = role;
            if let Some(board) = b.board {
                let detected_same =
                    det.board == Some(board) && b.board_rev.as_ref().matches_rev(det.rev.as_ref());
                id.board = if detected_same { None } else { Some(board) };
                id.board_rev = if detected_same {
                    None
                } else {
                    b.board_rev.clone()
                };
            }
            if name.is_some() {
                id.name = name.clone();
            }
        })
        .map_err(ApiError::internal)?;

    if role == LocalRole::Leader {
        let show_name = b.show_name.clone().map(|n| n.trim().to_string());
        let location = b.location.clone();
        let tz2 = tz.clone();
        let hash = password_hash.clone();
        state
            .store
            .update(move |s| {
                if let Some(n) = show_name {
                    s.name = n;
                }
                if let Some(mut l) = location {
                    if let Some(tz) = &tz2 {
                        l.timezone = tz.clone();
                    }
                    s.schedule.location = l;
                } else if let Some(tz) = tz2 {
                    s.schedule.location.timezone = tz;
                }
                if hash.is_some() {
                    s.settings.security.password_hash = hash;
                }
                crate::services::seed::seed_defaults(s);
                Ok(())
            })
            .await?;
        if let Err(e) = crate::cluster::ensure_self_node(&state).await {
            tracing::warn!("Couldn't add this controller to the show: {e:#}");
        }
    }
    if let Some(tz) = tz.clone() {
        tokio::spawn(async move { sys::set_system_timezone(&tz).await });
    }
    for n in &notes {
        state
            .events
            .toast(crate::events::ToastKind::Warning, n.clone());
    }
    let mut body = sys::system_info(&state, true).await;
    body["notes"] = json!(notes);
    let mut resp = Json(body).into_response();
    // Keep the person who just chose the password signed in.
    if password_hash.is_some() {
        let token = state.sessions.create();
        let cookie = format!(
            "pp_session={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
            60 * 60 * 24 * 30
        );
        if let Ok(v) = HeaderValue::from_str(&cookie) {
            resp.headers_mut().insert(header::SET_COOKIE, v);
        }
    }
    let _ = (peer, headers);
    Ok(resp)
}

trait OptEq {
    fn matches_rev(self, other: Option<&String>) -> bool;
}
impl OptEq for Option<&String> {
    fn matches_rev(self, other: Option<&String>) -> bool {
        match self {
            None => true,
            Some(a) => other.is_some_and(|b| a.eq_ignore_ascii_case(b)),
        }
    }
}

async fn power(state: AppState, action: sys::PowerAction) -> ApiResult<Json<Value>> {
    let msg = sys::power_action(action)?;
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
    network::apply(cfg, state.events.clone()).await.map(Json)
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

async fn write_eeprom_blocking(board: BoardKind, rev: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        #[cfg(target_os = "linux")]
        {
            let mut store =
                pixelplus_hw::eeprom::SysfsEeprom::open(1, pixelplus_hw::eeprom::EEPROM_ADDR)
                    .map_err(|e| e.to_string())?;
            let record = pixelplus_hw::Ppx1Record::new(board, &rev);
            pixelplus_hw::eeprom::write_record(&mut store, &record).map_err(|e| e.to_string())?;
            sys::redetect_board();
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (board, rev);
            Err("only possible on a Raspberry Pi".into())
        }
    })
    .await
    .map_err(|e| e.to_string())?
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
    if !valid_rev(&rev) {
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
    write_eeprom_blocking(b.board, rev.clone()).await.map_err(|e| {
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
    let msg = format!("EEPROM written: {} rev {rev}", b.board.display_name());
    state
        .events
        .toast(crate::events::ToastKind::Success, msg.clone());
    Ok(Json(json!({ "ok": true, "message": msg })))
}

async fn update_check() -> Json<crate::services::updates::UpdateInfo> {
    Json(crate::services::updates::check().await)
}

async fn update_apply(State(state): State<AppState>, _body: Bytes) -> ApiResult<Json<Value>> {
    let msg = crate::services::updates::apply().await?;
    state
        .events
        .toast(crate::events::ToastKind::Info, msg.clone());
    Ok(Json(json!({ "ok": true, "message": msg })))
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
        .route("/system/identify", post(identify_self))
}
