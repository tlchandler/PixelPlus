//! Tools: power estimate, health checks, snapshots (time machine), bulk prop
//! edits, schedule preview, triggers, and alert/MQTT tests.

use super::content::{multipart_error, save_field};
use super::crud::{merge_patch, Entity};
use super::playerapi::body_or_default;
use super::{ApiError, ApiResult};
use crate::services::snapshots;
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use parking_lot::Mutex;
use pixelplus_core::model::{Prop, Show};
use pixelplus_core::power::{estimate_full_white, estimate_power, PowerEstimate, PowerOptions};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// Cached power estimates keyed by (sequence hash, show version).
#[derive(Default)]
pub struct ToolsState {
    power: Mutex<HashMap<(String, u64), Arc<PowerEstimate>>>,
}

// ---------------------------------------------------------------------------
// Power
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PowerQuery {
    #[serde(default)]
    sequence_id: Option<String>,
}

async fn power(State(state): State<AppState>, Query(q): Query<PowerQuery>) -> ApiResult<Json<Arc<PowerEstimate>>> {
    let show = state.store.get();
    let Some(seq_id) = q.sequence_id.filter(|s| !s.is_empty()) else {
        let s = show.clone();
        let est = tokio::task::spawn_blocking(move || estimate_full_white(&s)).await.map_err(ApiError::internal)?;
        return Ok(Json(Arc::new(est)));
    };
    let seq = show.sequence(&seq_id).ok_or_else(|| ApiError::not_found("That sequence"))?.clone();
    let key = (if seq.hash.is_empty() { seq.id.clone() } else { seq.hash.clone() }, show.version);
    if let Some(hit) = state.services.tools.power.lock().get(&key).cloned() {
        return Ok(Json(hit));
    }
    let path = state.config.data_dir.join(&seq.file);
    let s = show.clone();
    let est = tokio::task::spawn_blocking(move || -> Result<PowerEstimate, String> {
        let mut f = pixelplus_core::fseq::FseqFile::open(&path).map_err(|e| e.to_string())?;
        estimate_power(&s, &mut f, &PowerOptions::default()).map_err(|e| e.to_string())
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(|e| ApiError::bad_request(format!("Couldn't read \"{}\" to estimate power: {e}", seq.name)))?;
    let est = Arc::new(est);
    let mut cache = state.services.tools.power.lock();
    cache.retain(|(_, v), _| *v == show.version);
    if cache.len() > 64 {
        cache.clear();
    }
    cache.insert(key, est.clone());
    Ok(Json(est))
}

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

async fn health(State(state): State<AppState>) -> Json<crate::services::health::HealthReport> {
    match state.services.health.last() {
        Some(r) => Json(r),
        None => Json(crate::services::health::run_checks(&state, false).await),
    }
}

async fn health_run(State(state): State<AppState>) -> Json<crate::services::health::HealthReport> {
    Json(crate::services::health::run_checks(&state, false).await)
}

// ---------------------------------------------------------------------------
// Snapshots
// ---------------------------------------------------------------------------

async fn snapshot_list(State(state): State<AppState>) -> Json<Vec<snapshots::Snapshot>> {
    Json(snapshots::list(&state).await)
}

#[derive(Deserialize, Default)]
struct CreateSnapshot {
    #[serde(default)]
    label: String,
    #[serde(default)]
    full: bool,
}

#[derive(Deserialize, Default)]
struct FullQuery {
    #[serde(default)]
    full: Option<String>,
}

async fn snapshot_create(State(state): State<AppState>, Query(q): Query<FullQuery>, body: Bytes) -> ApiResult<Json<snapshots::Snapshot>> {
    let b: CreateSnapshot = body_or_default(&body)?;
    let full = b.full || q.full.as_deref().is_some_and(|f| f == "1" || f == "true");
    snapshots::create(&state, &b.label, false, full).await.map(Json)
}

async fn snapshot_restore(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let show = snapshots::restore(&state, &id).await?;
    state.events.toast(crate::events::ToastKind::Success, "Snapshot restored.");
    Ok(Json(json!({ "ok": true, "version": show.version })))
}

async fn snapshot_delete(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    snapshots::delete(&state, &id).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn snapshot_download(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Response> {
    let path = snapshots::archive_file(&state, &id).ok_or_else(|| ApiError::not_found("That snapshot"))?;
    let file = tokio::fs::File::open(&path).await?;
    let len = file.metadata().await?.len();
    let body = axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file));
    let mut resp = body.into_response();
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/zstd"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"pixelplus-{id}.tar.zst\"")) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(resp)
}

async fn snapshot_import(State(state): State<AppState>, mut mp: Multipart) -> ApiResult<Json<snapshots::Snapshot>> {
    while let Some(field) = mp.next_field().await.map_err(multipart_error)? {
        let Some(name) = field.file_name().map(str::to_string) else { continue };
        let tmp = snapshots::dir(&state).join(format!(".import-{}.tmp", pixelplus_core::model::new_id()));
        save_field(field, &tmp, 8 * 1024 * 1024 * 1024).await?;
        return snapshots::import(&state, tmp, &name).await.map(Json);
    }
    Err(ApiError::bad_request("Choose a PixelPlus snapshot file (.tar.zst) to import."))
}

// ---------------------------------------------------------------------------
// Props: bulk edit & reorder
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct BulkOp {
    op: String,
    id: String,
    #[serde(default)]
    patch: Option<Value>,
}

#[derive(Deserialize)]
struct BulkBody {
    ops: Vec<BulkOp>,
}

async fn props_bulk(State(state): State<AppState>, Json(body): Json<BulkBody>) -> ApiResult<Json<Vec<Prop>>> {
    if body.ops.iter().any(|o| o.op == "delete") {
        crate::services::snapshots::auto(&state, "Before deleting props").await;
    }
    let (props, _) = state
        .store
        .update(move |show| {
            for op in &body.ops {
                match op.op.as_str() {
                    "delete" => {
                        let before = show.props.len();
                        show.props.retain(|p| p.id != op.id);
                        if show.props.len() != before {
                            <Prop as Entity>::on_delete(&op.id, show);
                        }
                    }
                    "update" => {
                        let idx = show
                            .props
                            .iter()
                            .position(|p| p.id == op.id)
                            .ok_or_else(|| ApiError::not_found("One of those props"))?;
                        let mut v = serde_json::to_value(&show.props[idx]).map_err(ApiError::internal)?;
                        if let Some(patch) = &op.patch {
                            merge_patch(&mut v, patch);
                        }
                        let mut p: Prop = serde_json::from_value(v)
                            .map_err(|e| ApiError::bad_request(format!("That change isn't valid: {e}")))?;
                        p.id = op.id.clone();
                        p.validate(show)?;
                        show.props[idx] = p;
                    }
                    other => return Err(ApiError::bad_request(format!("Unknown operation \"{other}\"."))),
                }
            }
            Ok(show.props.clone())
        })
        .await?;
    Ok(Json(props))
}

#[derive(Deserialize)]
struct ReorderBody {
    ids: Vec<String>,
}

async fn props_reorder(State(state): State<AppState>, Json(body): Json<ReorderBody>) -> ApiResult<Json<Value>> {
    state
        .store
        .update(move |show| {
            let order: HashMap<&str, usize> = body.ids.iter().enumerate().map(|(i, id)| (id.as_str(), i)).collect();
            // Stable: unlisted props keep their relative order after the listed ones.
            show.props.sort_by_key(|p| order.get(p.id.as_str()).copied().unwrap_or(usize::MAX));
            Ok(())
        })
        .await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Schedule preview
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct DaysQuery {
    #[serde(default)]
    days: Option<u32>,
}

pub(crate) fn schedule_preview_json(show: &Show, days: u32) -> ApiResult<Value> {
    let tz = pixelplus_core::schedule::schedule_timezone(&show.schedule)
        .map_err(|_| ApiError::bad_request(format!("\"{}\" isn't a time zone PixelPlus knows. Pick your city again in the schedule.", show.schedule.location.timezone)))?;
    let now = chrono::Utc::now().with_timezone(&tz);
    let occ = pixelplus_core::schedule::occurrences(&show.schedule, now, days.clamp(1, 366));
    Ok(Value::Array(
        occ.into_iter()
            .map(|o| {
                json!({
                    "date": o.date.to_string(),
                    "start": o.start.to_rfc3339(),
                    "end": o.end.to_rfc3339(),
                    "entryId": o.entry_id,
                    "playlistId": o.playlist_id,
                    "name": o.name,
                    "priority": o.priority,
                    "preempted": o.preempted,
                })
            })
            .collect(),
    ))
}

async fn schedule_preview(State(state): State<AppState>, Query(q): Query<DaysQuery>) -> ApiResult<Json<Value>> {
    schedule_preview_json(&state.store.get(), q.days.unwrap_or(14)).map(Json)
}

// ---------------------------------------------------------------------------
// Triggers, alerts, MQTT
// ---------------------------------------------------------------------------

async fn trigger_fire(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let msg = crate::services::triggers::fire(&state, &id).await?;
    Ok(Json(json!({ "ok": true, "message": msg })))
}

#[derive(Deserialize)]
struct ChannelBody {
    channel: String,
}

async fn alert_test(State(state): State<AppState>, Json(b): Json<ChannelBody>) -> Json<Value> {
    match crate::services::alerts::send_test(&state, &b.channel).await {
        Ok(m) => Json(json!({ "ok": true, "message": m })),
        Err(m) => Json(json!({ "ok": false, "message": m })),
    }
}

async fn mqtt_test(State(state): State<AppState>, body: Bytes) -> Json<Value> {
    // Optionally test unsaved settings from the form.
    let mut settings = state.store.get().settings.mqtt.clone();
    if let Ok(v) = body_or_default::<Value>(&body) {
        if v.is_object() && !v.as_object().is_some_and(|o| o.is_empty()) {
            let mut cur = serde_json::to_value(&settings).unwrap_or_default();
            merge_patch(&mut cur, &v);
            if let Ok(s) = serde_json::from_value(cur) {
                settings = s;
            }
        }
    }
    let node = state.identity().id;
    match crate::services::mqtt::test_connection(&settings, &node).await {
        Ok(m) => Json(json!({ "ok": true, "message": m })),
        Err(m) => Json(json!({ "ok": false, "message": m })),
    }
}

async fn mqtt_status(State(state): State<AppState>) -> Json<Value> {
    let (connected, error) = state.services.mqtt.status();
    Json(json!({ "enabled": state.store.get().settings.mqtt.enabled, "connected": connected, "error": error }))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/power/estimate", get(power))
        .route("/health", get(health))
        .route("/health/run", post(health_run))
        .route("/snapshots", get(snapshot_list).post(snapshot_create))
        .route("/snapshots/import", post(snapshot_import).layer(DefaultBodyLimit::disable()))
        .route("/snapshots/{id}", delete(snapshot_delete))
        .route("/snapshots/{id}/restore", post(snapshot_restore))
        .route("/snapshots/{id}/download", get(snapshot_download))
        .route("/props/bulk", post(props_bulk))
        .route("/props/reorder", post(props_reorder))
        .route("/schedule/preview", get(schedule_preview))
        .route("/triggers/{id}/fire", post(trigger_fire))
        .route("/alerts/test", post(alert_test))
        .route("/mqtt/test", post(mqtt_test))
        .route("/mqtt/status", get(mqtt_status))
}
