//! xLights "FPP Connect" compatible upload subset (F16, ARCHITECTURE §12.14),
//! so xLights (Tools → FPP Connect) can send sequences and songs straight to
//! PixelPlus. Matched against xLights `src-core/controllers/FPP.cpp` and
//! `src-ui-wx/controllers/FPPConnectDialog.cpp` (master, 2026-09):
//!
//! 1. **Detection**: `GET /config.php` is parsed line by line for
//!    `settings['Title'] = "…";` and the target counts as an FPP only if the
//!    title contains "Falcon Player" (a nominative compatibility statement).
//!    `GET /api/system/info` must carry a `uuid`; "Add FPP" by address also
//!    reads `GET /api/fppd/multiSyncSystems` for the `typeId` (1 = "FPP,
//!    undetermined hardware"; anything below 0x80 selects the FPP ≥ 7 upload
//!    path). No `channelRanges`, so xLights sends whole files.
//! 2. **Skip unchanged**: `GET /api/sequence/<name>/meta` (compares `Version`,
//!    `CompressionType`, `ID`, `NumFrames`, `StepTime`, `MaxChannel`,
//!    `ChannelCount`, `Ranges`) and `GET /api/media/<name>/meta`
//!    (`format.size`). 404 = not here yet.
//! 3. **Upload**: `PATCH /api/file/<dir>` in 16 MiB chunks with
//!    `Upload-Offset`, `Upload-Length`, `Upload-Name`; any non-200 makes
//!    xLights restart the file from offset 0 (3 tries). Offset 0 starts over;
//!    another offset must equal the bytes received so far (else 409). The last
//!    chunk hands the file to WS2's `import_sequence_file` (`sequences`) or
//!    `import_media_file` (`music`), which replace a same-named upload in
//!    place (same id, so playlists stay valid). `videos` and `effects` get 415
//!    (PixelPlus plays neither); `virtualdisplay_assets` is accepted and
//!    dropped. Legacy (FPP < 7) `POST /api/file/uploads/<name>` + `GET
//!    /api/file/move/<name>` work too.
//! 4. **Playlist**: `GET/POST /api/playlist/<name>` map to the PixelPlus
//!    playlist of that name; a POST adds missing sequences (never removes).
//! 5. **Controller configuration** (outputs, models, proxies, settings,
//!    restart) is answered with 200 and ignored: PixelPlus manages its own
//!    wiring.
//!
//! **Security** (`api/security.rs` `fpp_compat_authorize`, WS5): the routes
//! are mounted at the root, outside `/api/v1` auth and CSRF, and only answer
//! when `settings.xlights.fppConnect` is on, from the local network, without
//! proxy headers, for an allowed Host. Writes need HTTP Basic with the
//! xLights upload password (any user name); without one, only non-CORS-simple
//! writes pass. Additionally (this module): while the show has a sign-in
//! password, writes are refused until an upload password is set.
//!
//! Admin endpoints (`/api/v1/xlights/*`, merged through `api/profiles.rs`):
//! `GET /xlights/status`, `PUT /xlights/password {password}`,
//! `DELETE /xlights/uploads`. The optional **watch folder**
//! (`settings.xlights.watchFolder`) imports `.fseq` and audio files copied
//! into it (checked every 10 s, once a file stopped growing); imported files
//! move to `imported/`, refused ones to `failed/`.

use super::{ApiError, ApiResult};
use crate::api::content::{self, ImportedMedia, ImportedSequence, MediaImport, SequenceImport};
use crate::state::AppState;
use axum::body::Body;
use axum::extract::{FromRequestParts, Path, Request, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use futures::StreamExt;
use parking_lot::Mutex;
use pixelplus_core::fseq::{Compression, FseqFile};
use pixelplus_core::model::{new_id, MediaKind, Playlist, PlaylistItem, Show};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashSet, VecDeque};
use std::path::{Path as FsPath, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};
use tokio::io::AsyncWriteExt;

/// FPP version we present (≥ 7.1 for the chunked upload path; < 9.3 / < 10
/// so xLights uses the plain FPP ≥ 7 behaviour).
pub const FPP_VERSION: &str = "9.0";
/// FPP multisync `typeId` 0x01: "FPP (undetermined hardware)".
pub const FPP_TYPE_ID: u32 = 1;
/// Title xLights looks for ("Falcon Player").
pub const TITLE: &str = "PixelPlus (Falcon Player compatible upload)";
/// Entries kept in the uploads log.
const LOG_KEEP: usize = 50;
/// Free space kept when accepting an upload.
const KEEP_FREE: u64 = 256 * 1024 * 1024;
/// Largest single chunk accepted (xLights sends 16 MiB).
const MAX_CHUNK: u64 = 64 * 1024 * 1024;
/// Watch folder scan interval.
const WATCH_EVERY: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// Authorization extractor
// ---------------------------------------------------------------------------

/// Runs WS5's `fpp_compat_authorize` (feature on, LAN only, Host, upload
/// password / CSRF) plus the sign-in-password rule, before any handler.
pub struct FppAuth;

impl FromRequestParts<AppState> for FppAuth {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let peer = parts
            .extensions
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|c| c.0);
        super::security::fpp_compat_authorize(state, peer, &parts.method, &parts.headers).await?;
        // The legacy `GET /api/file/move/<name>` imports a file: a write,
        // although a GET (reads pass without the upload password).
        let legacy_move = parts.uri.path().starts_with("/api/file/move/");
        let write = legacy_move
            || !matches!(
                parts.method,
                axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS
            );
        if legacy_move && has_upload_password(state) {
            // Same rule as the upload itself: HTTP Basic with the upload password.
            super::security::fpp_compat_authorize(
                state,
                peer,
                &axum::http::Method::PATCH,
                &parts.headers,
            )
            .await?;
        }
        if write {
            let s = state.store.get();
            let has_upload_pw = s
                .settings
                .xlights
                .password_hash
                .as_deref()
                .is_some_and(|h| !h.is_empty());
            if s.settings.security.password_hash.is_some() && !has_upload_pw {
                return Err(ApiError::forbidden(
                    "Set an xLights upload password in PixelPlus (Settings → xLights) first.",
                )
                .into_response());
            }
        }
        Ok(FppAuth)
    }
}

fn has_upload_password(state: &AppState) -> bool {
    state
        .store
        .get()
        .settings
        .xlights
        .password_hash
        .as_deref()
        .is_some_and(|h| !h.is_empty())
}

// ---------------------------------------------------------------------------
// Uploads log (Settings → xLights)
// ---------------------------------------------------------------------------

/// One finished (or refused) upload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UploadEntry {
    /// RFC 3339.
    pub at: String,
    pub name: String,
    /// "sequence" | "song" | "ignored".
    pub kind: String,
    pub ok: bool,
    pub message: String,
    pub bytes: u64,
    /// "xlights" (FPP Connect) or "folder" (watch folder).
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_id: Option<String>,
    #[serde(default)]
    pub replaced: bool,
}

fn log_path(state: &AppState) -> PathBuf {
    upload_dir(state).join("log.json")
}

static LOG_LOCK: Mutex<()> = parking_lot::const_mutex(());

fn read_log(state: &AppState) -> VecDeque<UploadEntry> {
    std::fs::read(log_path(state))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn add_log(state: &AppState, entry: UploadEntry) {
    if entry.ok {
        tracing::info!("xLights upload \"{}\": {}", entry.name, entry.message);
    } else {
        tracing::warn!(
            "xLights upload \"{}\" refused: {}",
            entry.name,
            entry.message
        );
    }
    let _g = LOG_LOCK.lock();
    let mut log = read_log(state);
    log.push_front(entry);
    log.truncate(LOG_KEEP);
    let _ = std::fs::create_dir_all(upload_dir(state));
    if let Ok(b) = serde_json::to_vec_pretty(&log) {
        let tmp = log_path(state).with_extension("json.tmp");
        if std::fs::write(&tmp, b).is_ok() {
            let _ = std::fs::rename(tmp, log_path(state));
        }
    }
}

fn now_rfc() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

// ---------------------------------------------------------------------------
// Identity responses
// ---------------------------------------------------------------------------

/// A value safe inside `settings['X'] = "…";` (xLights cuts at the first `;`).
fn config_value(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '"' | ';' | '\\' | '\'' | '\r' | '\n' | '<' | '>'))
        .take(100)
        .collect()
}

fn hostname() -> String {
    crate::services::system::hostname()
}

fn uuid(state: &AppState) -> String {
    format!("PixelPlus-{}", state.identity().id)
}

fn variant() -> String {
    crate::services::system::detection()
        .1
        .map(|p| p.model)
        .unwrap_or_else(|| "PixelPlus".into())
}

/// `GET /config.php`: what FPP's settings page script looks like, enough for
/// xLights' `parseConfig`.
async fn config_php(_: FppAuth, State(state): State<AppState>) -> Response {
    let show = state.store.get();
    let body = format!(
        "var settings = new Array();\n\
         settings['Title'] = \"{}\";\n\
         settings['HostName'] = \"{}\";\n\
         settings['HostDescription'] = \"{}\";\n\
         settings['fppMode'] = \"player\";\n\
         settings['Platform'] = \"PixelPlus\";\n",
        config_value(TITLE),
        config_value(&hostname()),
        config_value(&show.name),
    );
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        body,
    )
        .into_response()
}

fn version_parts() -> (u32, u32) {
    let (a, b) = FPP_VERSION.split_once('.').unwrap_or(("9", "0"));
    (a.parse().unwrap_or(9), b.parse().unwrap_or(0))
}

/// `GET /api/system/info`.
async fn system_info(_: FppAuth, State(state): State<AppState>) -> Json<Value> {
    let show = state.store.get();
    let (major, minor) = version_parts();
    Json(json!({
        "HostName": hostname(),
        "HostDescription": show.name,
        "Platform": "PixelPlus",
        "Variant": variant(),
        "Mode": "player",
        "Version": FPP_VERSION,
        "majorVersion": major,
        "minorVersion": minor,
        "typeId": FPP_TYPE_ID,
        // xLights' sysinfo parser checks for this (misspelled) key before
        // reading "typeId".
        "typId": FPP_TYPE_ID,
        "uuid": uuid(&state),
        "multisync": false,
        "IPs": crate::services::system::ip_addresses(),
        "PixelPlus": { "version": env!("CARGO_PKG_VERSION") },
    }))
}

/// The address xLights reached us at (the Host header when it is an IPv4
/// literal; xLights ignores longer "addresses"), else our first LAN address.
fn reached_address(headers: &HeaderMap) -> String {
    let from_host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(|h| h.rsplit_once(':').map(|(a, _)| a).unwrap_or(h).to_string())
        .filter(|h| h.parse::<std::net::Ipv4Addr>().is_ok());
    from_host
        .or_else(|| {
            crate::services::system::ip_addresses()
                .into_iter()
                .find(|a| a.parse::<std::net::Ipv4Addr>().is_ok())
        })
        .unwrap_or_default()
}

/// `GET /api/fppd/multiSyncSystems` (FPP ≥ 6 format).
async fn multisync_systems(
    _: FppAuth,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Json<Value> {
    let show = state.store.get();
    let (major, minor) = version_parts();
    Json(json!({
        "systems": [{
            "hostname": hostname(),
            "address": reached_address(&headers),
            "type": "PixelPlus",
            "model": variant(),
            "version": FPP_VERSION,
            "majorVersion": major,
            "minorVersion": minor,
            "typeId": FPP_TYPE_ID,
            "uuid": uuid(&state),
            "fppModeString": "player",
            "channelRanges": "",
            "HostDescription": show.name,
            "local": 1,
        }]
    }))
}

// ---------------------------------------------------------------------------
// Meta (skip unchanged files)
// ---------------------------------------------------------------------------

/// FPP's `/api/sequence/<name>/meta` for an fseq header.
pub fn sequence_meta<R: std::io::Read + std::io::Seek>(name: &str, f: &FseqFile<R>) -> Value {
    let h = f.header();
    let ranges: Vec<Value> = h
        .sparse_ranges
        .iter()
        .map(|r| json!({"Start": r.start, "Length": r.len}))
        .collect();
    let max_channel = if h.sparse_ranges.is_empty() {
        h.channel_count
    } else {
        h.sparse_ranges
            .iter()
            .map(|r| r.start + r.len)
            .max()
            .unwrap_or(0)
    };
    let compression = match h.compression {
        Compression::None => 0,
        Compression::Zstd => 1,
        Compression::Zlib => 2,
    };
    let mut v = json!({
        "Name": name,
        "Version": format!("{}.{}", h.version_major, h.version_minor),
        "ID": h.unique_id.to_string(),
        "StepTime": h.step_time_ms,
        "NumFrames": h.frame_count,
        "MaxChannel": max_channel,
        "ChannelCount": h.channel_count,
        "CompressionType": compression,
        "variableHeaders": {},
    });
    if !ranges.is_empty() {
        v["Ranges"] = Value::Array(ranges);
    }
    if let Some(mf) = h.media_filename() {
        v["variableHeaders"]["mf"] = json!(mf);
    }
    v
}

async fn seq_meta(
    _: FppAuth,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    let show = state.store.get();
    let seq = show
        .sequences
        .iter()
        .find(|s| s.xlights_name.as_deref() == Some(name.as_str()))
        .ok_or_else(|| ApiError::not_found("That sequence"))?;
    let path = state.config.data_dir.join(&seq.file);
    let name2 = name.clone();
    let meta = tokio::task::spawn_blocking(move || {
        FseqFile::open(&path)
            .ok()
            .map(|f| sequence_meta(&name2, &f))
    })
    .await
    .map_err(ApiError::internal)?
    .ok_or_else(|| ApiError::not_found("That sequence"))?;
    Ok(Json(meta))
}

async fn media_meta(
    _: FppAuth,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    let show = state.store.get();
    let m = show
        .media
        .iter()
        .find(|m| m.kind == MediaKind::Song && m.original_name.as_deref() == Some(name.as_str()))
        .ok_or_else(|| ApiError::not_found("That song"))?;
    let mut format = json!({
        "filename": name,
        "duration": format!("{:.6}", m.duration_ms as f64 / 1000.0),
    });
    // Only the original upload's size lets xLights skip it (a converted
    // file's size never matches).
    if let Some(size) = m.original_size {
        format["size"] = json!(size.to_string());
    }
    Ok(Json(json!({ "format": format })))
}

// ---------------------------------------------------------------------------
// Chunked upload (PATCH /api/file/<dir>)
// ---------------------------------------------------------------------------

/// What an upload directory means here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Sequences,
    Music,
    /// Accepted and dropped (virtual display assets).
    Discard,
    /// Refused with a reason (videos, effect sequences).
    Unsupported(&'static str),
}

pub fn classify_dir(dir: &str) -> Option<Dir> {
    Some(match dir {
        "sequences" => Dir::Sequences,
        "music" => Dir::Music,
        "virtualdisplay_assets" => Dir::Discard,
        "videos" => Dir::Unsupported("PixelPlus doesn't play videos. Untick \"Media\" for video sequences or add the song instead."),
        "effects" => Dir::Unsupported("PixelPlus doesn't use effect sequences (.eseq)."),
        _ => return None,
    })
}

/// A file name as xLights sends it, safe to keep: no directories, no control
/// characters, 1–200 characters.
pub fn clean_name(raw: &str) -> Option<String> {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw).trim();
    let s: String = base.chars().filter(|c| !c.is_control()).collect();
    (!s.is_empty() && s.chars().count() <= 200 && s != "." && s != "..").then_some(s)
}

fn upload_dir(state: &AppState) -> PathBuf {
    state.config.data_dir.join("uploads").join("xlights")
}

fn part_paths(state: &AppState, dir: &str, name: &str) -> (PathBuf, PathBuf) {
    let tag = &crate::cluster::sig::sha256_hex(format!("{dir}\n{name}").as_bytes())[..20];
    let base = upload_dir(state);
    (
        base.join(format!("{tag}.part")),
        base.join(format!("{tag}.json")),
    )
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct PartMeta {
    name: String,
    dir: String,
    length: u64,
}

/// Files being received now (`dir\nname`): one request per file at a time.
fn busy() -> &'static Mutex<HashSet<String>> {
    static B: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    B.get_or_init(Default::default)
}

struct BusyGuard(String);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        busy().lock().remove(&self.0);
    }
}

fn header_u64(h: &HeaderMap, name: &str) -> Option<u64> {
    h.get(name)?.to_str().ok()?.trim().parse().ok()
}

fn fpp_error(status: StatusCode, msg: impl Into<String>) -> Response {
    let msg = msg.into();
    (status, Json(json!({ "status": "ERROR", "message": msg }))).into_response()
}

fn cap_for(dir: Dir) -> u64 {
    match dir {
        Dir::Sequences => content::FSEQ_MAX,
        Dir::Music => content::AUDIO_MAX,
        _ => 256 * 1024 * 1024,
    }
}

/// Is there room for `bytes` more (keeping [`KEEP_FREE`] free)?
fn room_for(state: &AppState, bytes: u64) -> bool {
    crate::services::system::disk_space(&state.config.data_dir)
        .map(|(f, _)| f)
        .unwrap_or(u64::MAX)
        >= bytes.saturating_add(KEEP_FREE)
}

/// Unfinished uploads are dropped after this long (xLights restarts a
/// failed file from offset 0 at once).
const STALE_PART: Duration = Duration::from_secs(24 * 3600);

/// Delete unfinished uploads (`*.part` and their `*.json` resume records,
/// staged legacy files, watch-folder copies) older than [`STALE_PART`], so
/// abandoned uploads can't pile up on the SD card.
async fn cleanup_stale_parts(state: &AppState) {
    let base = upload_dir(state);
    for dir in [base.clone(), base.join("legacy"), base.join("watch")] {
        let Ok(mut rd) = tokio::fs::read_dir(&dir).await else {
            continue;
        };
        while let Ok(Some(e)) = rd.next_entry().await {
            let name = e.file_name().to_string_lossy().to_string();
            if !(name.ends_with(".part") || (name.ends_with(".json") && name != "log.json")) {
                continue;
            }
            let old = e
                .metadata()
                .await
                .ok()
                .filter(|m| m.is_file())
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > STALE_PART);
            if old {
                let _ = tokio::fs::remove_file(e.path()).await;
            }
        }
    }
}

/// Stream `body` into `file`; returns the bytes written or an error text.
async fn write_body(file: &mut tokio::fs::File, body: Body, max: u64) -> Result<u64, String> {
    let mut stream = body.into_data_stream();
    let mut n = 0u64;
    let mut w = tokio::io::BufWriter::with_capacity(1 << 20, file);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("the upload was interrupted ({e})"))?;
        n += chunk.len() as u64;
        if n > max {
            return Err("more data than announced".into());
        }
        w.write_all(&chunk)
            .await
            .map_err(|e| format!("couldn't store it ({e})"))?;
    }
    w.flush()
        .await
        .map_err(|e| format!("couldn't store it ({e})"))?;
    Ok(n)
}

async fn upload_patch(
    _: FppAuth,
    State(state): State<AppState>,
    Path(dir): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let Some(kind) = classify_dir(&dir) else {
        return fpp_error(StatusCode::NOT_FOUND, "Unknown upload folder.");
    };
    let Some(name) = headers
        .get("upload-name")
        .and_then(|v| v.to_str().ok())
        .and_then(clean_name)
    else {
        return fpp_error(StatusCode::BAD_REQUEST, "Missing Upload-Name.");
    };
    let (Some(offset), Some(length)) = (
        header_u64(&headers, "upload-offset"),
        header_u64(&headers, "upload-length"),
    ) else {
        return fpp_error(
            StatusCode::BAD_REQUEST,
            "Missing Upload-Offset / Upload-Length.",
        );
    };
    if let Dir::Unsupported(why) = kind {
        if offset == 0 {
            add_log(
                &state,
                UploadEntry {
                    at: now_rfc(),
                    name: name.clone(),
                    kind: "ignored".into(),
                    ok: false,
                    message: why.into(),
                    bytes: length,
                    source: "xlights".into(),
                    sequence_id: None,
                    media_id: None,
                    replaced: false,
                },
            );
        }
        return fpp_error(StatusCode::UNSUPPORTED_MEDIA_TYPE, why);
    }
    if length > cap_for(kind) {
        return fpp_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("\"{name}\" is too large ({} MB).", length / 1_000_000),
        );
    }
    if offset > length {
        return fpp_error(StatusCode::CONFLICT, "Upload-Offset is past Upload-Length.");
    }
    let key = format!("{dir}\n{name}");
    if !busy().lock().insert(key.clone()) {
        return fpp_error(StatusCode::CONFLICT, "That file is already being uploaded.");
    }
    let _busy = BusyGuard(key);
    let (part, meta_path) = part_paths(&state, &dir, &name);
    if let Err(e) = tokio::fs::create_dir_all(upload_dir(&state)).await {
        return fpp_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Storage error: {e}"),
        );
    }
    // Resume check: offset 0 starts over; any other offset must continue the
    // same file exactly where it stopped.
    let meta = PartMeta {
        name: name.clone(),
        dir: dir.clone(),
        length,
    };
    if offset == 0 {
        cleanup_stale_parts(&state).await;
        if !room_for(&state, length - offset) {
            return fpp_error(
                StatusCode::INSUFFICIENT_STORAGE,
                "PixelPlus is out of storage space for that file.",
            );
        }
        let _ = tokio::fs::remove_file(&part).await;
        if let Err(e) =
            tokio::fs::write(&meta_path, serde_json::to_vec(&meta).unwrap_or_default()).await
        {
            return fpp_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Storage error: {e}"),
            );
        }
    } else {
        let stored: Option<PartMeta> = tokio::fs::read(&meta_path)
            .await
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        let have = tokio::fs::metadata(&part).await.map(|m| m.len()).ok();
        if stored.as_ref() != Some(&meta) || have != Some(offset) {
            return fpp_error(
                StatusCode::CONFLICT,
                format!(
                    "Upload-Offset {offset} doesn't continue this file (have {}).",
                    have.unwrap_or(0)
                ),
            );
        }
    }
    let announced = header_u64(&headers, header::CONTENT_LENGTH.as_str());
    let room = (length - offset).min(MAX_CHUNK);
    if announced.is_some_and(|c| c > room) {
        return fpp_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "That chunk goes past Upload-Length.",
        );
    }
    // Several uploads may run at once: check before every chunk, not only
    // at the start, so together they can't fill the SD card.
    if !room_for(&state, room) {
        return fpp_error(
            StatusCode::INSUFFICIENT_STORAGE,
            "PixelPlus is out of storage space for that file.",
        );
    }
    let mut file = match tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&part)
        .await
    {
        Ok(f) => f,
        Err(e) => {
            return fpp_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Storage error: {e}"),
            )
        }
    };
    let got = match write_body(&mut file, body, room).await {
        Ok(n) => n,
        Err(e) => {
            // Keep the file at the last good offset so a retry can continue.
            let _ = file.set_len(offset).await;
            return fpp_error(StatusCode::BAD_REQUEST, format!("\"{name}\": {e}."));
        }
    };
    if announced.is_some_and(|c| c != got) {
        let _ = file.set_len(offset).await;
        return fpp_error(StatusCode::BAD_REQUEST, "The chunk was cut short.");
    }
    let _ = file.sync_data().await;
    drop(file);
    let now_at = offset + got;
    if now_at < length {
        let mut r = Json(json!({ "status": "OK", "offset": now_at })).into_response();
        if let Ok(v) = HeaderValue::from_str(&now_at.to_string()) {
            r.headers_mut().insert("upload-offset", v);
        }
        return r;
    }
    // Complete.
    let _ = tokio::fs::remove_file(&meta_path).await;
    let entry = finish(&state, kind, part, &name, length, "xlights").await;
    let ok = entry.ok;
    let msg = entry.message.clone();
    add_log(&state, entry);
    if ok {
        let mut r =
            Json(json!({ "status": "OK", "offset": length, "message": msg })).into_response();
        if let Ok(v) = HeaderValue::from_str(&length.to_string()) {
            r.headers_mut().insert("upload-offset", v);
        }
        r
    } else {
        fpp_error(StatusCode::UNPROCESSABLE_ENTITY, msg)
    }
}

/// Import a complete file (`src` is consumed).
async fn finish(
    state: &AppState,
    kind: Dir,
    src: PathBuf,
    name: &str,
    bytes: u64,
    source: &str,
) -> UploadEntry {
    let mut e = UploadEntry {
        at: now_rfc(),
        name: name.to_string(),
        kind: "ignored".into(),
        ok: false,
        message: String::new(),
        bytes,
        source: source.into(),
        sequence_id: None,
        media_id: None,
        replaced: false,
    };
    match kind {
        Dir::Sequences => {
            e.kind = "sequence".into();
            match content::import_sequence_file(state, src, name, SequenceImport::default()).await {
                Ok(ImportedSequence {
                    sequence,
                    warnings,
                    replaced,
                }) => {
                    e.ok = true;
                    e.replaced = replaced;
                    e.sequence_id = Some(sequence.id.clone());
                    e.message = format!(
                        "{} \"{}\"{}{}",
                        if replaced { "Updated" } else { "Added" },
                        sequence.name,
                        if sequence.media_id.is_some() {
                            " with its song"
                        } else {
                            ""
                        },
                        warnings
                            .first()
                            .map(|w| format!(". {w}"))
                            .unwrap_or_default()
                    );
                }
                Err(err) => e.message = err.message,
            }
        }
        Dir::Music => {
            e.kind = "song".into();
            let opts = MediaImport {
                kind: MediaKind::Song,
                name: None,
                replace_same_name: true,
            };
            match content::import_media_file(state, src, name, opts).await {
                Ok(ImportedMedia {
                    media,
                    replaced,
                    linked_sequence_ids,
                }) => {
                    e.ok = true;
                    e.replaced = replaced;
                    e.media_id = Some(media.id.clone());
                    e.message = format!(
                        "{} song \"{}\"{}",
                        if replaced { "Updated" } else { "Added" },
                        media.name,
                        match linked_sequence_ids.len() {
                            0 => String::new(),
                            1 => ", linked to 1 sequence".into(),
                            n => format!(", linked to {n} sequences"),
                        }
                    );
                }
                Err(err) => e.message = err.message,
            }
        }
        Dir::Discard | Dir::Unsupported(_) => {
            let _ = tokio::fs::remove_file(&src).await;
            e.ok = true;
            e.message = "Not needed by PixelPlus (ignored).".into();
        }
    }
    e
}

/// Legacy (FPP < 7): `POST /api/file/uploads/<name>` with the whole file.
async fn legacy_upload(
    _: FppAuth,
    State(state): State<AppState>,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let Some(name) = clean_name(&name) else {
        return fpp_error(StatusCode::BAD_REQUEST, "Bad file name.");
    };
    let dir = upload_dir(&state).join("legacy");
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return fpp_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Storage error: {e}"),
        );
    }
    let tag = &crate::cluster::sig::sha256_hex(name.as_bytes())[..20];
    let path = dir.join(format!("{tag}.part"));
    let max = content::FSEQ_MAX;
    let announced = header_u64(&headers, header::CONTENT_LENGTH.as_str());
    if announced.is_some_and(|c| c > max) {
        return fpp_error(StatusCode::PAYLOAD_TOO_LARGE, "That file is too large.");
    }
    cleanup_stale_parts(&state).await;
    if !room_for(&state, announced.unwrap_or(max)) {
        return fpp_error(
            StatusCode::INSUFFICIENT_STORAGE,
            "PixelPlus is out of storage space for that file.",
        );
    }
    let mut f = match tokio::fs::File::create(&path).await {
        Ok(f) => f,
        Err(e) => {
            return fpp_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Storage error: {e}"),
            )
        }
    };
    if let Err(e) = write_body(&mut f, body, max).await {
        let _ = tokio::fs::remove_file(&path).await;
        return fpp_error(StatusCode::BAD_REQUEST, e);
    }
    Json(json!({ "status": "OK" })).into_response()
}

fn kind_for_name(name: &str) -> Dir {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".fseq") {
        Dir::Sequences
    } else if [".mp3", ".ogg", ".wav", ".flac", ".m4a", ".aac", ".mp4a"]
        .iter()
        .any(|e| lower.ends_with(e))
    {
        Dir::Music
    } else if lower.ends_with(".eseq") {
        Dir::Unsupported("PixelPlus doesn't use effect sequences (.eseq).")
    } else if [".mp4", ".mkv", ".avi", ".mov", ".mpg", ".mpeg"]
        .iter()
        .any(|e| lower.ends_with(e))
    {
        Dir::Unsupported("PixelPlus doesn't play videos.")
    } else {
        Dir::Discard
    }
}

/// Legacy: `GET /api/file/move/<name>` after the POST.
async fn legacy_move(
    _: FppAuth,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Response {
    let Some(name) = clean_name(&name) else {
        return fpp_error(StatusCode::BAD_REQUEST, "Bad file name.");
    };
    let tag = &crate::cluster::sig::sha256_hex(name.as_bytes())[..20];
    let path = upload_dir(&state)
        .join("legacy")
        .join(format!("{tag}.part"));
    let Ok(meta) = tokio::fs::metadata(&path).await else {
        return fpp_error(StatusCode::NOT_FOUND, "Upload the file first.");
    };
    let entry = finish(
        &state,
        kind_for_name(&name),
        path,
        &name,
        meta.len(),
        "xlights",
    )
    .await;
    let (ok, msg) = (entry.ok, entry.message.clone());
    add_log(&state, entry);
    if ok {
        Json(json!({ "status": "OK", "message": msg })).into_response()
    } else {
        fpp_error(StatusCode::UNPROCESSABLE_ENTITY, msg)
    }
}

// ---------------------------------------------------------------------------
// Playlists
// ---------------------------------------------------------------------------

fn seq_file_name(show: &Show, id: &str) -> Option<(String, Option<String>, f64)> {
    let s = show.sequence(id)?;
    let name = s
        .xlights_name
        .clone()
        .unwrap_or_else(|| format!("{}.fseq", s.name));
    let media = s
        .media_id
        .as_deref()
        .and_then(|m| show.media_item(m))
        .map(|m| m.original_name.clone().unwrap_or_else(|| m.name.clone()));
    Some((name, media, s.duration_ms as f64 / 1000.0))
}

/// FPP playlist JSON for a PixelPlus playlist (its sequences).
pub fn fpp_playlist(show: &Show, pl: &Playlist) -> Value {
    let entries: Vec<Value> = pl
        .items
        .iter()
        .filter_map(|it| match it {
            PlaylistItem::Sequence { sequence_id, .. } => seq_file_name(show, sequence_id),
            _ => None,
        })
        .map(|(seq, media, dur)| match media {
            Some(m) => json!({
                "type": "both", "enabled": 1, "playOnce": 0,
                "sequenceName": seq, "mediaName": m, "videoOut": "--Default--", "duration": dur
            }),
            None => json!({
                "type": "sequence", "enabled": 1, "playOnce": 0,
                "sequenceName": seq, "duration": dur
            }),
        })
        .collect();
    let total: f64 = entries.iter().filter_map(|e| e["duration"].as_f64()).sum();
    json!({
        "name": pl.name,
        "version": 3,
        "repeat": if pl.repeat { 1 } else { 0 },
        "loopCount": 0,
        "random": if pl.shuffle { 1 } else { 0 },
        "desc": "",
        "leadIn": [],
        "mainPlaylist": entries,
        "leadOut": [],
        "playlistInfo": { "total_items": entries.len(), "total_duration": total },
    })
}

async fn playlists(_: FppAuth, State(state): State<AppState>) -> Json<Vec<String>> {
    Json(
        state
            .store
            .get()
            .playlists
            .iter()
            .filter(|p| p.smart.is_none())
            .map(|p| p.name.clone())
            .collect(),
    )
}

async fn playlist_get(
    _: FppAuth,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    let show = state.store.get();
    let pl = show
        .playlists
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(name.trim()))
        .ok_or_else(|| ApiError::not_found("That playlist"))?;
    Ok(Json(fpp_playlist(&show, pl)))
}

/// Add the sequences named in an FPP playlist to the PixelPlus playlist
/// `name` (created when missing). Never removes. Returns (added, unknown).
pub fn merge_playlist(
    show: &mut Show,
    name: &str,
    body: &Value,
) -> ApiResult<(usize, Vec<String>)> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(ApiError::bad_request("Bad playlist name."));
    }
    let wanted: Vec<String> = ["leadIn", "mainPlaylist", "leadOut"]
        .iter()
        .filter_map(|k| body.get(*k).and_then(Value::as_array))
        .flatten()
        .filter_map(|e| e.get("sequenceName").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    let mut ids = vec![];
    let mut unknown = vec![];
    for w in wanted {
        match show
            .sequences
            .iter()
            .find(|s| s.xlights_name.as_deref() == Some(w.as_str()))
        {
            Some(s) => ids.push(s.id.clone()),
            None => unknown.push(w),
        }
    }
    let idx = match show
        .playlists
        .iter()
        .position(|p| p.name.eq_ignore_ascii_case(name))
    {
        Some(i) => i,
        None => {
            if ids.is_empty() {
                return Ok((0, unknown));
            }
            show.playlists.push(Playlist {
                id: new_id(),
                name: name.to_string(),
                items: vec![],
                intro: vec![],
                outro: vec![],
                shuffle: false,
                repeat: true,
                crossfade_ms: 0,
                smart: None,
            });
            show.playlists.len() - 1
        }
    };
    let pl = &mut show.playlists[idx];
    if pl.smart.is_some() {
        return Err(ApiError::bad_request(format!(
            "\"{}\" is a smart playlist in PixelPlus; its songs are picked by rules.",
            pl.name
        )));
    }
    let mut added = 0;
    for id in ids {
        let present = pl.items.iter().any(
            |it| matches!(it, PlaylistItem::Sequence { sequence_id, .. } if *sequence_id == id),
        );
        if !present {
            pl.items.push(PlaylistItem::Sequence {
                id: new_id(),
                sequence_id: id,
            });
            added += 1;
        }
    }
    Ok((added, unknown))
}

async fn playlist_post(
    _: FppAuth,
    State(state): State<AppState>,
    Path(name): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    if !state.store.get().settings.xlights.add_to_playlists {
        return Json(json!({ "Status": "OK", "Message": "Playlists are managed in PixelPlus." }))
            .into_response();
    }
    let Ok(v) = serde_json::from_slice::<Value>(&body) else {
        return fpp_error(StatusCode::BAD_REQUEST, "That isn't playlist JSON.");
    };
    let res = state
        .store
        .update(move |s| merge_playlist(s, &name, &v))
        .await;
    match res {
        Ok(((added, unknown), _)) => {
            if added > 0 {
                tracing::info!("xLights playlist: added {added} sequence(s)");
            }
            Json(json!({
                "Status": "OK",
                "Message": if unknown.is_empty() { String::new() } else {
                    format!("Not in PixelPlus yet: {}", unknown.join(", "))
                }
            }))
            .into_response()
        }
        Err(e) => fpp_error(e.status, e.message),
    }
}

// ---------------------------------------------------------------------------
// Controller configuration: accepted and ignored
// ---------------------------------------------------------------------------

async fn ok_json(_: FppAuth) -> Json<Value> {
    Json(json!({ "Status": "OK", "Message": "" }))
}

async fn ignored_config(_: FppAuth, req: Request) -> Json<Value> {
    tracing::info!(
        "xLights sent controller configuration ({} {}) — ignored: PixelPlus manages its own wiring.",
        req.method(),
        req.uri().path()
    );
    Json(json!({ "Status": "OK", "Message": "" }))
}

async fn empty_array(_: FppAuth) -> Json<Value> {
    Json(json!([]))
}

async fn no_outputs(_: FppAuth) -> Json<Value> {
    Json(json!({ "channelOutputs": [] }))
}

async fn not_here(_: FppAuth) -> Response {
    ApiError::not_found("That page").into_response()
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/config.php", get(config_php))
        .route("/api/system/info", get(system_info))
        .route("/api/fppd/multiSyncSystems", get(multisync_systems))
        .route("/api/sequence/{name}/meta", get(seq_meta))
        .route("/api/media/{name}/meta", get(media_meta))
        .route("/api/file/uploads/{name}", post(legacy_upload))
        .route("/api/file/move/{name}", get(legacy_move))
        .route("/api/file/{dir}", patch(upload_patch))
        .route("/api/playlists", get(playlists))
        .route(
            "/api/playlist/{name}",
            get(playlist_get).post(playlist_post),
        )
        .route("/api/proxies", get(empty_array).post(ignored_config))
        .route("/api/proxies/{*rest}", post(ignored_config))
        .route("/api/models", get(empty_array).post(ignored_config))
        .route(
            "/api/channel/output/{*rest}",
            get(no_outputs).post(ignored_config),
        )
        .route(
            "/api/configfile/{*rest}",
            get(not_here).post(ignored_config),
        )
        .route("/api/cape", get(not_here))
        .route(
            "/api/settings/{*rest}",
            put(ignored_config).post(ignored_config),
        )
        .route("/api/system/fppd/restart", get(ok_json))
}

// ---------------------------------------------------------------------------
// Admin: /api/v1/xlights/*
// ---------------------------------------------------------------------------

/// `GET /api/v1/xlights/status`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct XlightsStatus {
    pub enabled: bool,
    pub password_set: bool,
    /// The show has a sign-in password (then an upload password is required).
    pub admin_password_set: bool,
    /// Uploads would be accepted now.
    pub ready: bool,
    /// Why not, when not ready.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Addresses to type into xLights' "Add FPP".
    pub addresses: Vec<String>,
    pub hostname: String,
    pub uploads: Vec<UploadEntry>,
    pub watch: WatchStatus,
}

#[derive(Debug, Serialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct WatchStatus {
    pub folder: Option<String>,
    pub exists: bool,
    /// Suggested folder (under the data directory).
    pub suggested: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_scan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn watch_state() -> &'static Mutex<(Option<String>, Option<String>)> {
    static W: OnceLock<Mutex<(Option<String>, Option<String>)>> = OnceLock::new();
    W.get_or_init(Default::default)
}

fn suggested_folder(state: &AppState) -> PathBuf {
    state.config.data_dir.join("xlights-drop")
}

async fn admin_status(State(state): State<AppState>) -> Json<XlightsStatus> {
    let show = state.store.get();
    let x = &show.settings.xlights;
    let password_set = x.password_hash.as_deref().is_some_and(|h| !h.is_empty());
    let admin_password_set = show.settings.security.password_hash.is_some();
    let reason = if !x.fpp_connect {
        Some("Turned off.".to_string())
    } else if admin_password_set && !password_set {
        Some("Set an upload password: this show has a sign-in password.".into())
    } else {
        None
    };
    let folder = x.watch_folder.clone().filter(|f| !f.trim().is_empty());
    let (last_scan, error) = watch_state().lock().clone();
    Json(XlightsStatus {
        enabled: x.fpp_connect,
        password_set,
        admin_password_set,
        ready: reason.is_none(),
        reason,
        addresses: crate::services::system::ip_addresses(),
        hostname: hostname(),
        uploads: read_log(&state).into_iter().collect(),
        watch: WatchStatus {
            exists: folder.as_deref().is_some_and(|f| FsPath::new(f).is_dir()),
            folder,
            suggested: suggested_folder(&state).display().to_string(),
            last_scan,
            error,
        },
    })
}

#[derive(Deserialize)]
struct PasswordBody {
    #[serde(default)]
    password: String,
}

async fn admin_password(
    State(state): State<AppState>,
    Json(body): Json<PasswordBody>,
) -> ApiResult<Json<Value>> {
    let pw = body.password;
    let hash = if pw.is_empty() {
        None
    } else {
        if pw.chars().count() < 6 {
            return Err(ApiError::bad_request(
                "Use at least 6 characters for the upload password.",
            ));
        }
        if pw.len() > 200 || pw.contains(':') {
            return Err(ApiError::bad_request(
                "That password can't be used (no colons, 200 characters at most).",
            ));
        }
        Some(super::auth::hash_password(&pw)?)
    };
    let set = hash.is_some();
    state
        .store
        .update(move |s| {
            s.settings.xlights.password_hash = hash;
            Ok(())
        })
        .await?;
    Ok(Json(json!({ "passwordSet": set })))
}

async fn admin_clear_log(State(state): State<AppState>) -> Json<Value> {
    let _g = LOG_LOCK.lock();
    let _ = std::fs::remove_file(log_path(&state));
    Json(json!({ "ok": true }))
}

/// Admin routes, merged into `/api/v1` through `api/profiles.rs` (so no
/// shared router file changes).
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/xlights/status", get(admin_status))
        .route("/xlights/password", put(admin_password))
        .route("/xlights/uploads", axum::routing::delete(admin_clear_log))
}

// ---------------------------------------------------------------------------
// Watch folder
// ---------------------------------------------------------------------------

/// A watch folder the daemon may read: absolute, not a system directory.
pub fn watch_folder_ok(p: &str) -> bool {
    let path = FsPath::new(p.trim());
    if !path.is_absolute() || path.components().count() < 2 {
        return false;
    }
    let s = path.to_string_lossy();
    ![
        "/proc", "/sys", "/dev", "/etc", "/boot", "/run", "/usr", "/bin", "/sbin", "/lib",
    ]
    .iter()
    .any(|bad| s == *bad || s.starts_with(&format!("{bad}/")))
}

/// Sizes seen at the previous scan: a file is imported once it stopped
/// growing between two scans and is at least 5 s old.
type Seen = std::collections::HashMap<PathBuf, (u64, SystemTime)>;

async fn scan_once(state: &AppState, folder: &FsPath, seen: &mut Seen) -> Result<(), String> {
    let mut rd = tokio::fs::read_dir(folder)
        .await
        .map_err(|e| format!("Can't read {}: {e}", folder.display()))?;
    let mut now_seen: Seen = Default::default();
    while let Ok(Some(e)) = rd.next_entry().await {
        let path = e.path();
        let Ok(meta) = e.metadata().await else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        if name.starts_with('.') || name.ends_with(".part") || name.ends_with(".tmp") {
            continue;
        }
        let kind = kind_for_name(&name);
        if !matches!(kind, Dir::Sequences | Dir::Music) {
            continue;
        }
        let size = meta.len();
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let stable = seen.get(&path) == Some(&(size, mtime))
            && mtime.elapsed().unwrap_or_default() >= Duration::from_secs(5);
        now_seen.insert(path.clone(), (size, mtime));
        if !stable || size == 0 {
            continue;
        }
        // Copy into the data directory (the import consumes its source).
        let tmp_dir = upload_dir(state).join("watch");
        let _ = tokio::fs::create_dir_all(&tmp_dir).await;
        let tmp = tmp_dir.join(format!("{}.part", new_id()));
        let entry = match copy_nofollow(&path, &tmp).await {
            Ok(_) => finish(state, kind, tmp, &name, size, "folder").await,
            Err(err) => {
                let _ = tokio::fs::remove_file(&tmp).await;
                UploadEntry {
                    at: now_rfc(),
                    name: name.clone(),
                    kind: if kind == Dir::Sequences {
                        "sequence"
                    } else {
                        "song"
                    }
                    .into(),
                    ok: false,
                    message: format!("Couldn't copy it: {err}"),
                    bytes: size,
                    source: "folder".into(),
                    sequence_id: None,
                    media_id: None,
                    replaced: false,
                }
            }
        };
        let sub = folder.join(if entry.ok { "imported" } else { "failed" });
        let _ = tokio::fs::create_dir_all(&sub).await;
        if tokio::fs::rename(&path, sub.join(&name)).await.is_err() {
            // Read-only share: remember it so it isn't imported again.
            now_seen.insert(path.clone(), (u64::MAX, mtime));
        }
        now_seen.remove(&path);
        add_log(state, entry);
    }
    // Files kept in place (read-only) keep their "done" marker.
    for (p, v) in seen.iter() {
        if v.0 == u64::MAX && p.exists() {
            now_seen.insert(p.clone(), *v);
        }
    }
    *seen = now_seen;
    Ok(())
}

/// Copy a regular file, never through a symlink: the watch folder may be a
/// network share others can write to, and a file swapped for a link to a
/// daemon-only file (keys, the show) between the scan and the copy must not
/// be imported. Returns the bytes copied.
pub async fn copy_nofollow(src: &FsPath, dst: &FsPath) -> std::io::Result<u64> {
    let (src, dst) = (src.to_path_buf(), dst.to_path_buf());
    tokio::task::spawn_blocking(move || {
        let mut opts = std::fs::OpenOptions::new();
        opts.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let mut f = opts.open(&src)?;
        if !f.metadata()?.is_file() {
            return Err(std::io::Error::other("not a regular file"));
        }
        let mut out = std::fs::File::create(&dst)?;
        std::io::copy(&mut f, &mut out)
    })
    .await
    .map_err(std::io::Error::other)?
}

/// Start the watch-folder task (called from `services::profiles::start`).
pub fn start(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let mut seen: Seen = Default::default();
        let mut tick = tokio::time::interval(WATCH_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let show = state.store.get();
            // Off in Settings → Features: the drop folder isn't watched.
            let folder = show
                .settings
                .xlights
                .watch_folder
                .clone()
                .filter(|f| !f.trim().is_empty())
                .filter(|_| show.feature(pixelplus_core::model::FeatureId::XlightsUpload));
            let Some(folder) = folder else {
                seen.clear();
                continue;
            };
            let err = if !watch_folder_ok(&folder) {
                Some(
                    "That folder can't be used (use an absolute path outside system folders)."
                        .to_string(),
                )
            } else {
                let p = PathBuf::from(folder.trim());
                if !p.is_dir() && p.starts_with(&state.config.data_dir) {
                    let _ = tokio::fs::create_dir_all(&p).await;
                }
                scan_once(&state, &p, &mut seen).await.err()
            };
            *watch_state().lock() = (Some(now_rfc()), err);
        }
    });
}

#[cfg(test)]
mod tests;
