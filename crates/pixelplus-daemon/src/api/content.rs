//! Content: sequences (`.fseq`), audio media, DJ clip rendering/upload and
//! TTS proxy endpoints. Also hosts small helpers shared by the other
//! system/content API modules (multipart streaming, player access).
//!
//! # Import functions for other modules (owned by WS2)
//!
//! Anything that receives sequence or audio files outside the multipart upload
//! routes (xLights FPP Connect in `api/fppcompat.rs`, a future watch folder)
//! must go through these two functions, so every file gets the same checks,
//! thumbnail, song auto-linking, loudness measurement and beat analysis (F2):
//!
//! ```ignore
//! // `src` is a complete file anywhere under the data directory (e.g.
//! // `uploads/<name>.part`). It is always consumed: moved into place on
//! // success, deleted on failure.
//! pub async fn import_sequence_file(
//!     state: &AppState,
//!     src: PathBuf,
//!     original_name: &str,          // "Wizards in Winter.fseq"
//!     opts: SequenceImport,         // { name: Option<String>, audio: Option<(PathBuf, String)> }
//! ) -> ApiResult<ImportedSequence>; // { sequence, warnings, replaced }
//!
//! pub async fn import_media_file(
//!     state: &AppState,
//!     src: PathBuf,
//!     original_name: &str,          // "Wizards in Winter.mp3"
//!     opts: MediaImport,            // { kind, name, replace_same_name }
//! ) -> ApiResult<ImportedMedia>;    // { media, replaced, linked_sequence_ids }
//! ```
//!
//! * A sequence whose `xlightsName` equals `original_name` is **replaced in
//!   place** (same id, tags and playlist entries). Its song is linked from the
//!   fseq header's media file name or a matching name.
//! * With `replace_same_name`, audio whose original file name matches is
//!   replaced in place too (same id and tags; the analysis is redone).
//! * Errors are friendly [`ApiError`]s (400 for unreadable files).
//! * Both functions write `show.json` once and queue the beat analysis.

use super::crud::merge_patch;
use super::{ApiError, ApiResult};
use crate::player::PlayerHandle;
use crate::services::media::{self as media_svc, MediaMeta};
use crate::services::paths;
use crate::state::AppState;
use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use pixelplus_core::fseq::FseqFile;
use pixelplus_core::model::{new_id, DjClip, Media, MediaKind, PlaylistItem, Prop, Sequence, Show};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Digest;
use std::path::{Path as FsPath, PathBuf};
use tower::ServiceExt;

/// Largest sequence upload (xLights sequences of big displays get large).
pub const FSEQ_MAX: u64 = 4 * 1024 * 1024 * 1024;
/// Largest audio upload.
pub const AUDIO_MAX: u64 = 512 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// The playback engine, or a friendly 503.
pub(crate) fn player(state: &AppState) -> ApiResult<&PlayerHandle> {
    state.services.player.get().ok_or_else(|| {
        ApiError::unavailable("The player is still starting. Try again in a moment.")
    })
}

pub(crate) fn multipart_error(e: axum::extract::multipart::MultipartError) -> ApiError {
    let status = e.status();
    if status == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::new(status, "too_large", "That file is too large.")
    } else {
        ApiError::bad_request(format!(
            "The upload didn't come through completely ({}). Please try again.",
            e.body_text()
        ))
    }
}

/// Stream a multipart field to `path` (via a temp file), enforcing `limit`.
/// Returns (bytes written, sha256 hex).
pub(crate) async fn save_field(
    mut field: axum::extract::multipart::Field<'_>,
    path: &FsPath,
    limit: u64,
) -> ApiResult<(u64, String)> {
    use tokio::io::AsyncWriteExt;
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let mut file = tokio::fs::File::create(path).await?;
    let mut hasher = sha2::Sha256::new();
    let mut total: u64 = 0;
    let res: ApiResult<()> = async {
        while let Some(chunk) = field.chunk().await.map_err(multipart_error)? {
            total += chunk.len() as u64;
            if total > limit {
                return Err(ApiError::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "too_large",
                    format!(
                        "That file is too large (the limit is {} MB).",
                        limit / (1024 * 1024)
                    ),
                ));
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        file.sync_all().await?;
        Ok(())
    }
    .await;
    if let Err(e) = res {
        drop(file);
        let _ = tokio::fs::remove_file(path).await;
        return Err(e);
    }
    Ok((total, pixelplus_core::fseq::to_hex(&hasher.finalize())))
}

/// The show without the password hash (as `GET /show` returns it).
pub(crate) fn public_show(show: &Show) -> Show {
    let mut s = show.clone();
    super::show::redact_settings(&mut s.settings);
    s
}

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"))
}

// ---------------------------------------------------------------------------
// Media ingest
// ---------------------------------------------------------------------------

/// Analyze an uploaded audio file at `tmp` and move it to `media/<id>.<ext>`.
/// The returned `Media` is not yet in the show.
pub(crate) async fn ingest_audio(
    state: &AppState,
    tmp: PathBuf,
    original: &str,
    kind: MediaKind,
    name: Option<String>,
) -> ApiResult<Media> {
    let Some(ext) = media_svc::audio_ext(original) else {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(ApiError::bad_request(format!(
            "\"{original}\" isn't a supported audio file. Use MP3, OGG, M4A, WAV or FLAC."
        )));
    };
    let t = tmp.clone();
    let analysis = tokio::task::spawn_blocking(move || media_svc::analyze(&t))
        .await
        .map_err(ApiError::internal)?;
    let mut meta = match analysis {
        Ok(m) => m,
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(ApiError::bad_request(format!(
                "We couldn't read \"{original}\" as audio ({e}). Try exporting it again as MP3."
            )));
        }
    };
    meta.original_name = original.to_string();
    let size = tokio::fs::metadata(&tmp).await.ok().map(|m| m.len());
    let id = new_id();
    let rel = format!("media/{id}.{ext}");
    media_svc::move_file(&tmp, &state.config.data_dir.join(&rel)).await?;
    let _ = media_svc::write_meta(&state.config.media_dir(), &id, &meta);
    let target = state.store.get().settings.audio.target_lufs;
    Ok(Media {
        tags: Default::default(),
        analysis: Default::default(),
        original_name: Some(original.to_string()),
        original_size: size,
        id,
        name: name
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| media_svc::display_name(original)),
        kind,
        file: rel,
        duration_ms: meta.duration_ms,
        loudness_lufs: meta.loudness_lufs,
        gain_db: media_svc::gain_for(target, meta.loudness_lufs),
    })
}

/// Original filename of a media item (meta file), else its name.
fn media_original(state: &AppState, m: &Media) -> String {
    media_svc::read_meta(&state.config.media_dir(), &m.id)
        .map(|meta| meta.original_name)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| m.name.clone())
}

/// Find the song that belongs to a sequence: the fseq header's media file name,
/// else the sequence's own file/display name.
fn find_audio_for(
    state: &AppState,
    show: &Show,
    media_basename: Option<&str>,
    seq_names: &[&str],
) -> Option<String> {
    let songs: Vec<&Media> = show
        .media
        .iter()
        .filter(|m| m.kind == MediaKind::Song)
        .collect();
    let keys: Vec<(String, &Media)> = songs
        .iter()
        .flat_map(|m| {
            let orig = media_original(state, m);
            [
                (media_svc::match_key(&orig), *m),
                (media_svc::match_key(&m.name), *m),
            ]
        })
        .filter(|(k, _)| !k.is_empty())
        .collect();
    let mut wanted: Vec<String> = Vec::new();
    if let Some(b) = media_basename {
        wanted.push(media_svc::match_key(b));
    }
    wanted.extend(seq_names.iter().map(|n| media_svc::match_key(n)));
    for w in wanted.iter().filter(|w| !w.is_empty()) {
        if let Some((_, m)) = keys.iter().find(|(k, _)| k == w) {
            return Some(m.id.clone());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Sequences
// ---------------------------------------------------------------------------

struct FseqInfo {
    duration_ms: u64,
    frame_ms: u32,
    channel_count: u32,
    media_basename: Option<String>,
}

fn read_fseq_info(path: &FsPath) -> Result<FseqInfo, String> {
    let f = FseqFile::open(path).map_err(|e| e.to_string())?;
    if f.frame_count() == 0 {
        return Err("it has no frames".into());
    }
    Ok(FseqInfo {
        duration_ms: f.duration_ms(),
        frame_ms: f.frame_ms(),
        channel_count: f.channel_count(),
        media_basename: f.header().media_basename(),
    })
}

/// Props that need channels beyond what the sequence contains.
pub(crate) fn channel_warnings(props: &[Prop], channel_count: u32) -> Vec<String> {
    let beyond: Vec<&str> = props
        .iter()
        .filter(|p| p.channel_end() > channel_count as u64)
        .map(|p| p.name.as_str())
        .collect();
    if beyond.is_empty() {
        return vec![];
    }
    let names = if beyond.len() <= 4 {
        beyond.join(", ")
    } else {
        format!("{} and {} more", beyond[..4].join(", "), beyond.len() - 4)
    };
    vec![format!(
        "This sequence doesn't include data for {names}, so {} stay dark while it plays. Re-export it from xLights after updating your layout.",
        if beyond.len() == 1 { "that prop will" } else { "those props will" }
    )]
}

/// Render a preview strip: x = time, one band per prop (average colour).
pub fn generate_thumbnail(fseq: &FsPath, props: &[Prop], out: &FsPath) -> Result<(), String> {
    const MAX_COLS: u32 = 400;
    const HEIGHT: u32 = 64;
    let started = std::time::Instant::now();
    let mut f = FseqFile::open(fseq).map_err(|e| e.to_string())?;
    let frames = f.frame_count().max(1);
    let cols = frames.min(MAX_COLS);
    let frame_size = f.frame_size();
    // Rows: groups of props (or channel bands when there are no props yet).
    let ranges: Vec<Vec<(usize, usize)>> = if props.is_empty() {
        let bands = 32usize.min((frame_size / 3).max(1));
        let per = frame_size.div_ceil(bands).max(3);
        (0..bands)
            .map(|b| vec![(b * per, ((b + 1) * per).min(frame_size))])
            .filter(|r| r[0].0 < r[0].1)
            .collect()
    } else {
        let rows = props.len().min(HEIGHT as usize);
        let mut groups: Vec<Vec<(usize, usize)>> = vec![vec![]; rows];
        for (i, p) in props.iter().enumerate() {
            for r in p.channel_ranges() {
                let s = r.channel_start as usize;
                let e = (s + r.pixel_count as usize * 3).min(frame_size);
                if s < e {
                    groups[i * rows / props.len()].push((s, e));
                }
            }
        }
        groups
    };
    let rows = ranges.len().max(1) as u32;
    let row_h = (HEIGHT / rows).max(1);
    let height = rows * row_h;
    let mut img = vec![0u8; (cols * height * 3) as usize];
    let mut buf = vec![0u8; frame_size];
    for c in 0..cols {
        if started.elapsed() > std::time::Duration::from_secs(20) {
            break;
        }
        let idx = (c as u64 * frames as u64 / cols as u64) as u32;
        f.frame(idx, &mut buf).map_err(|e| e.to_string())?;
        for (r, group) in ranges.iter().enumerate() {
            let mut sum = [0u64; 3];
            let mut n = 0u64;
            for &(s, e) in group {
                for px in buf[s..e].chunks_exact(3) {
                    sum[0] += px[0] as u64;
                    sum[1] += px[1] as u64;
                    sum[2] += px[2] as u64;
                    n += 1;
                }
            }
            if n == 0 {
                continue;
            }
            // Averages of mostly-dark props are dim: lift them for legibility.
            let rgb =
                sum.map(|v| (255.0 * ((v as f64 / n as f64) / 255.0).powf(0.55)).round() as u8);
            for y in 0..row_h {
                let o = (((r as u32 * row_h + y) * cols + c) * 3) as usize;
                img[o..o + 3].copy_from_slice(&rgb);
            }
        }
    }
    let tmp = out.with_extension("png.tmp");
    {
        let file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), cols, height);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        w.write_image_data(&img).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, out).map_err(|e| e.to_string())?;
    Ok(())
}

async fn list_sequences(State(state): State<AppState>) -> Json<Vec<Sequence>> {
    Json(state.store.get().sequences.clone())
}

async fn get_sequence(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Sequence>> {
    state
        .store
        .get()
        .sequence(&id)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::not_found("That sequence"))
}

async fn create_sequence(State(state): State<AppState>, req: Request) -> ApiResult<Json<Value>> {
    if is_json(req.headers()) {
        let Json(seq): Json<Sequence> = Json::from_request_json(req).await?;
        return restore_sequence(&state, seq).await;
    }
    let mp = Multipart::from_request_multipart(req).await?;
    upload_sequence(state, mp).await.map(Json)
}

/// Small extraction helpers so one route can accept JSON (undo) and multipart (upload).
trait FromReq: Sized {
    async fn from_request_json(req: Request) -> ApiResult<Self>;
}
impl<T: serde::de::DeserializeOwned> FromReq for Json<T> {
    async fn from_request_json(req: Request) -> ApiResult<Self> {
        let bytes = axum::body::to_bytes(req.into_body(), 16 * 1024 * 1024)
            .await
            .map_err(|_| ApiError::bad_request("The request is too large."))?;
        serde_json::from_slice(&bytes)
            .map(Json)
            .map_err(|e| ApiError::bad_request(format!("That isn't valid: {e}")))
    }
}
trait FromReqMp: Sized {
    async fn from_request_multipart(req: Request) -> ApiResult<Self>;
}
impl FromReqMp for Multipart {
    async fn from_request_multipart(req: Request) -> ApiResult<Self> {
        use axum::extract::FromRequest;
        Multipart::from_request(req, &())
            .await
            .map_err(|_| ApiError::bad_request("Send the file as a multipart upload."))
    }
}

/// Undo of a delete: re-create the entity and bring its files back.
/// What an edit (`PUT /sequences/:id`) can't change: identity, files and what was
/// measured from the file (a patch naming another `file` would make the leader read,
/// slice and serve an arbitrary path).
fn keep_sequence_facts(old: &Sequence, new: &mut Sequence) {
    new.id = old.id.clone();
    new.file = old.file.clone();
    new.thumbnail = old.thumbnail.clone();
    new.hash = old.hash.clone();
    new.duration_ms = old.duration_ms;
    new.frame_ms = old.frame_ms;
    new.channel_count = old.channel_count;
}

/// Same for `PUT /media/:id` (`GET /media/:id/file` serves `file`).
fn keep_media_facts(old: &Media, new: &mut Media) {
    new.id = old.id.clone();
    new.file = old.file.clone();
    new.duration_ms = old.duration_ms;
    new.loudness_lufs = old.loudness_lufs;
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A re-created (undone) media item may only point at `media/<id>.<ext>`.
fn restored_media_path_ok(m: &Media) -> bool {
    safe_id(&m.id)
        && m.file
            .strip_prefix(&format!("media/{}.", m.id))
            .is_some_and(|ext| {
                !ext.is_empty() && ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric())
            })
}

/// A re-created (undone) sequence may only point at `sequences/<id>.fseq` and
/// `thumbnails/<id>.png`.
fn check_restored_paths(seq: &Sequence) -> ApiResult<()> {
    let id_ok = safe_id(&seq.id);
    let file_ok = seq.file == format!("sequences/{}.fseq", seq.id);
    let thumb_ok = seq
        .thumbnail
        .as_deref()
        .map_or(true, |t| t == format!("thumbnails/{}.png", seq.id));
    if id_ok && file_ok && thumb_ok {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "That isn't a sequence PixelPlus made.",
        ))
    }
}

async fn restore_sequence(state: &AppState, mut seq: Sequence) -> ApiResult<Json<Value>> {
    let data = state.config.data_dir.clone();
    // Only this sequence's own files: the body comes from the client.
    check_restored_paths(&seq)?;
    media_svc::untrash(&data, &seq.file);
    if let Some(t) = &seq.thumbnail {
        media_svc::untrash(&data, t);
    }
    if !data.join(&seq.file).is_file() {
        return Err(ApiError::bad_request(
            "The sequence file is gone. Please upload it again.",
        ));
    }
    // Followers cache slices by this hash: never trust the one in the request.
    let path = data.join(&seq.file);
    seq.hash = tokio::task::spawn_blocking(move || pixelplus_core::fseq::sha256_file(path))
        .await
        .map_err(ApiError::internal)??;
    let (s, _) = state
        .store
        .update(move |show| {
            if show.sequence(&seq.id).is_some() {
                return Err(ApiError::conflict("That sequence already exists."));
            }
            let mut seq = seq;
            if seq
                .media_id
                .as_deref()
                .is_some_and(|m| show.media_item(m).is_none())
            {
                seq.media_id = None;
            }
            show.sequences.push(seq.clone());
            Ok(seq)
        })
        .await?;
    Ok(Json(serde_json::to_value(s).map_err(ApiError::internal)?))
}

async fn upload_sequence(state: AppState, mut mp: Multipart) -> ApiResult<Value> {
    let upload_id = new_id();
    let seq_tmp = state
        .config
        .sequences_dir()
        .join(format!(".upload-{upload_id}.fseq"));
    let mut fseq_name: Option<String> = None;
    let mut audio: Option<(PathBuf, String)> = None;
    let mut display: Option<String> = None;
    let mut hash = String::new();
    let cleanup = |seq: &PathBuf, audio: &Option<(PathBuf, String)>| {
        let _ = std::fs::remove_file(seq);
        if let Some((a, _)) = audio {
            let _ = std::fs::remove_file(a);
        }
    };
    loop {
        let field = match mp.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                cleanup(&seq_tmp, &audio);
                return Err(multipart_error(e));
            }
        };
        let name = field.name().unwrap_or_default().to_string();
        let file_name = field.file_name().map(str::to_string);
        match (name.as_str(), file_name) {
            ("name", None) => {
                display = field
                    .text()
                    .await
                    .ok()
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
            }
            (_, Some(fname)) if fname.to_ascii_lowercase().ends_with(".fseq") || name == "fseq" => {
                if fseq_name.is_some() {
                    continue;
                }
                match save_field(field, &seq_tmp, FSEQ_MAX).await {
                    Ok((_, h)) => {
                        hash = h;
                        fseq_name = Some(fname);
                    }
                    Err(e) => {
                        cleanup(&seq_tmp, &audio);
                        return Err(e);
                    }
                }
            }
            (_, Some(fname)) if name == "audio" || media_svc::audio_ext(&fname).is_some() => {
                if audio.is_some() {
                    continue;
                }
                let ext = media_svc::audio_ext(&fname).unwrap_or_else(|| "bin".into());
                let tmp = state
                    .config
                    .media_dir()
                    .join(format!(".upload-{upload_id}.{ext}"));
                if let Err(e) = save_field(field, &tmp, AUDIO_MAX).await {
                    cleanup(&seq_tmp, &audio);
                    return Err(e);
                }
                audio = Some((tmp, fname));
            }
            _ => {}
        }
    }
    let Some(fseq_name) = fseq_name else {
        cleanup(&seq_tmp, &audio);
        return Err(ApiError::bad_request(
            "Choose an .fseq file exported from xLights (File → Export → FSEQ).",
        ));
    };
    let done = import_sequence_file(
        &state,
        seq_tmp,
        &fseq_name,
        SequenceImport {
            name: display,
            audio,
            hash: Some(hash),
        },
    )
    .await?;
    let mut v = serde_json::to_value(&done.sequence).map_err(ApiError::internal)?;
    v["warnings"] = json!(done.warnings);
    v["replaced"] = json!(done.replaced);
    Ok(v)
}

/// How [`import_sequence_file`] should import a sequence.
#[derive(Debug, Default)]
pub struct SequenceImport {
    /// Display name; default: the existing sequence's name, else the file name.
    pub name: Option<String>,
    /// The song uploaded with it: (file anywhere under the data dir, original name).
    /// Consumed like the sequence file.
    pub audio: Option<(PathBuf, String)>,
    /// sha256 hex of `src` when the caller already computed it while receiving.
    pub hash: Option<String>,
}

/// Result of [`import_sequence_file`].
#[derive(Debug, Clone)]
pub struct ImportedSequence {
    pub sequence: Sequence,
    /// Friendly notes, e.g. props the sequence has no channels for.
    pub warnings: Vec<String>,
    /// An existing sequence with the same xLights file name was replaced in place.
    pub replaced: bool,
}

/// How [`import_media_file`] should import an audio file.
#[derive(Debug, Clone)]
pub struct MediaImport {
    pub kind: MediaKind,
    /// Display name; default: made from the file name.
    pub name: Option<String>,
    /// Replace an existing item whose original file name is the same (xLights
    /// re-uploads), keeping its id, name and tags.
    pub replace_same_name: bool,
}

impl Default for MediaImport {
    fn default() -> Self {
        MediaImport {
            kind: MediaKind::Song,
            name: None,
            replace_same_name: false,
        }
    }
}

/// Result of [`import_media_file`].
#[derive(Debug, Clone)]
pub struct ImportedMedia {
    pub media: Media,
    pub replaced: bool,
    /// Sequences that were waiting for this song and now play it.
    pub linked_sequence_ids: Vec<String>,
}

/// Import a complete `.fseq` file (see the module docs). `src` is consumed.
pub async fn import_sequence_file(
    state: &AppState,
    src: PathBuf,
    original_name: &str,
    opts: SequenceImport,
) -> ApiResult<ImportedSequence> {
    let fseq_name = sanitize_original(original_name);
    let SequenceImport {
        name: display,
        audio,
        hash,
    } = opts;
    let drop_all = |src: &PathBuf, audio: &Option<(PathBuf, String)>| {
        let _ = std::fs::remove_file(src);
        if let Some((a, _)) = audio {
            let _ = std::fs::remove_file(a);
        }
    };
    let t = src.clone();
    let info = match tokio::task::spawn_blocking(move || read_fseq_info(&t))
        .await
        .map_err(ApiError::internal)?
    {
        Ok(i) => i,
        Err(e) => {
            drop_all(&src, &audio);
            return Err(ApiError::bad_request(format!(
                "\"{fseq_name}\" isn't a sequence PixelPlus can play ({e}). Export it again from xLights as an .fseq file."
            )));
        }
    };
    let hash = match hash.filter(|h| h.len() == 64) {
        Some(h) => h,
        None => {
            let p = src.clone();
            match tokio::task::spawn_blocking(move || pixelplus_core::fseq::sha256_file(p))
                .await
                .map_err(ApiError::internal)?
            {
                Ok(h) => h,
                Err(e) => {
                    drop_all(&src, &audio);
                    return Err(e.into());
                }
            }
        }
    };
    let show = state.store.get();
    // Re-uploading the same xLights file replaces it (keeps playlists intact).
    let existing = show
        .sequences
        .iter()
        .find(|s| s.xlights_name.as_deref() == Some(fseq_name.as_str()))
        .cloned();
    let id = existing
        .as_ref()
        .map(|s| s.id.clone())
        .unwrap_or_else(new_id);
    // Audio first: if it is refused, the sequence (maybe one being replaced,
    // whose hash, duration and follower slices describe the old file) is untouched.
    let mut new_media: Option<Media> = None;
    if let Some((tmp, fname)) = audio {
        match ingest_audio(state, tmp, &fname, MediaKind::Song, None).await {
            Ok(m) => new_media = Some(m),
            Err(e) => {
                let _ = tokio::fs::remove_file(&src).await;
                return Err(e);
            }
        }
    }
    let rel = format!("sequences/{id}.fseq");
    if let Err(e) = media_svc::move_file(&src, &state.config.data_dir.join(&rel)).await {
        let _ = tokio::fs::remove_file(&src).await;
        return Err(e.into());
    }
    let name = display
        .or_else(|| existing.as_ref().map(|s| s.name.clone()))
        .unwrap_or_else(|| media_svc::display_name(&fseq_name));
    let media_id = match &new_media {
        Some(m) => Some(m.id.clone()),
        None => existing
            .as_ref()
            .and_then(|s| s.media_id.clone())
            .or_else(|| {
                find_audio_for(
                    state,
                    &show,
                    info.media_basename.as_deref(),
                    &[&fseq_name, &name],
                )
            }),
    };
    let new_media_id = new_media.as_ref().map(|m| m.id.clone());
    // Thumbnail.
    let thumb_rel = format!("thumbnails/{id}.png");
    let (src, dst, props) = (
        state.config.data_dir.join(&rel),
        state.config.data_dir.join(&thumb_rel),
        show.props.clone(),
    );
    let _ = tokio::fs::create_dir_all(state.config.thumbnails_dir()).await;
    let thumbnail =
        match tokio::task::spawn_blocking(move || generate_thumbnail(&src, &props, &dst)).await {
            Ok(Ok(())) => Some(thumb_rel),
            Ok(Err(e)) => {
                tracing::warn!("Couldn't draw a preview for \"{name}\": {e}");
                None
            }
            Err(_) => None,
        };
    let warnings = channel_warnings(&show.props, info.channel_count);
    let seq = Sequence {
        // An xLights upload replaces a generated show's file with a real one.
        generated: None,
        tags: existing
            .as_ref()
            .map(|s| s.tags.clone())
            .unwrap_or_default(),
        id: id.clone(),
        name,
        file: rel,
        duration_ms: info.duration_ms,
        frame_ms: info.frame_ms,
        channel_count: info.channel_count,
        media_id,
        xlights_name: Some(fseq_name),
        thumbnail,
        hash,
    };
    let (seq, _) = state
        .store
        .update({
            let seq = seq.clone();
            move |s| {
                if let Some(m) = new_media {
                    s.media.push(m);
                }
                match s.sequences.iter_mut().find(|x| x.id == seq.id) {
                    Some(x) => *x = seq.clone(),
                    None => s.sequences.push(seq.clone()),
                }
                Ok(seq)
            }
        })
        .await?;
    if let Some(m) = new_media_id {
        crate::services::analysis::enqueue_analysis(state, &m);
    }
    Ok(ImportedSequence {
        sequence: seq,
        warnings,
        replaced: existing.is_some(),
    })
}

/// A file name as the user knows it: no directories, no control characters.
fn sanitize_original(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let s: String = base.chars().filter(|c| !c.is_control()).take(200).collect();
    let s = s.trim().to_string();
    if s.is_empty() {
        "upload".into()
    } else {
        s
    }
}

/// Import a complete audio file (see the module docs). `src` is consumed.
pub async fn import_media_file(
    state: &AppState,
    src: PathBuf,
    original_name: &str,
    opts: MediaImport,
) -> ApiResult<ImportedMedia> {
    let fname = sanitize_original(original_name);
    let existing = if opts.replace_same_name {
        let show = state.store.get();
        show.media
            .iter()
            .find(|m| {
                m.kind == opts.kind
                    && m.original_name
                        .clone()
                        .unwrap_or_else(|| media_original(state, m))
                        == fname
            })
            .cloned()
    } else {
        None
    };
    let mut media = ingest_audio(state, src, &fname, opts.kind, opts.name).await?;
    let replaced = existing.is_some();
    if let Some(old) = &existing {
        // Keep the identity: move the new file to the old id's name.
        let ext = media.file.rsplit_once('.').map(|x| x.1).unwrap_or("mp3");
        let rel = format!("media/{}.{ext}", old.id);
        let data = &state.config.data_dir;
        if rel != old.file {
            media_svc::trash(data, &old.file);
        }
        media_svc::move_file(&data.join(&media.file), &data.join(&rel)).await?;
        let dir = state.config.media_dir();
        if let Some(meta) = media_svc::read_meta(&dir, &media.id) {
            let _ = media_svc::write_meta(&dir, &old.id, &meta);
        }
        let _ = std::fs::remove_file(media_svc::meta_path(&dir, &media.id));
        crate::services::analysis::forget(state, &old.id);
        media = Media {
            id: old.id.clone(),
            name: old.name.clone(),
            tags: old.tags.clone(),
            file: rel,
            analysis: None,
            ..media
        };
    }
    // Link sequences that were waiting for this song.
    let show = state.store.get();
    let mut link: Vec<String> = Vec::new();
    if opts.kind == MediaKind::Song {
        let key_orig = media_svc::match_key(&fname);
        let key_name = media_svc::match_key(&media.name);
        for s in show.sequences.iter().filter(|s| s.media_id.is_none()) {
            let path = state.config.data_dir.join(&s.file);
            let basename = tokio::task::spawn_blocking(move || {
                FseqFile::open(&path)
                    .ok()
                    .and_then(|f| f.header().media_basename())
            })
            .await
            .ok()
            .flatten();
            let keys = [
                basename.as_deref().map(media_svc::match_key),
                Some(media_svc::match_key(&s.name)),
                s.xlights_name.as_deref().map(media_svc::match_key),
            ];
            if keys
                .iter()
                .flatten()
                .any(|k| !k.is_empty() && (*k == key_orig || *k == key_name))
            {
                link.push(s.id.clone());
            }
        }
    }
    let linked = link.clone();
    let (media, _) = state
        .store
        .update(move |s| {
            match s.media.iter_mut().find(|m| m.id == media.id) {
                Some(m) => *m = media.clone(),
                None => s.media.push(media.clone()),
            }
            for seq in s
                .sequences
                .iter_mut()
                .filter(|x| link.contains(&x.id) && x.media_id.is_none())
            {
                seq.media_id = Some(media.id.clone());
            }
            Ok(media)
        })
        .await?;
    crate::services::analysis::enqueue_analysis(state, &media.id);
    Ok(ImportedMedia {
        media,
        replaced,
        linked_sequence_ids: linked,
    })
}

async fn update_sequence(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(mut patch): Json<Value>,
) -> ApiResult<Json<Sequence>> {
    // Only user-editable fields.
    if let Value::Object(p) = &mut patch {
        p.retain(|k, _| matches!(k.as_str(), "name" | "mediaId" | "xlightsName" | "tags"));
    }
    let (seq, _) = state
        .store
        .update(move |show| {
            let idx = show
                .sequences
                .iter()
                .position(|s| s.id == id)
                .ok_or_else(|| ApiError::not_found("That sequence"))?;
            let mut v = serde_json::to_value(&show.sequences[idx]).map_err(ApiError::internal)?;
            merge_patch(&mut v, &patch);
            let mut seq: Sequence = serde_json::from_value(v)
                .map_err(|e| ApiError::bad_request(format!("That change isn't valid: {e}")))?;
            keep_sequence_facts(&show.sequences[idx], &mut seq);
            seq.tags = pixelplus_core::smartlist::normalize_tags(&seq.tags);
            if seq.name.trim().is_empty() {
                return Err(ApiError::bad_request("Please give it a name."));
            }
            if let Some(m) = &seq.media_id {
                if show.media_item(m).is_none() {
                    return Err(ApiError::bad_request("That audio file no longer exists."));
                }
            }
            show.sequences[idx] = seq.clone();
            Ok(seq)
        })
        .await?;
    Ok(Json(seq))
}

async fn delete_sequence(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    if state.store.get().sequence(&id).is_none() {
        return Err(ApiError::not_found("That sequence"));
    }
    crate::services::snapshots::auto(&state, "Before deleting a sequence").await;
    let (seq, _) = state
        .store
        .update(|show| {
            let idx = show.sequences.iter().position(|s| s.id == id).ok_or_else(|| ApiError::not_found("That sequence"))?;
            let seq = show.sequences.remove(idx);
            for p in &mut show.playlists {
                for list in [&mut p.items, &mut p.intro, &mut p.outro] {
                    list.retain(|i| !matches!(i, PlaylistItem::Sequence { sequence_id, .. } if *sequence_id == id));
                }
                if let Some(r) = &mut p.smart {
                    for list in [&mut r.pinned_first, &mut r.pinned_last, &mut r.interleave] {
                        list.retain(|i| !matches!(i, PlaylistItem::Sequence { sequence_id, .. } if *sequence_id == id));
                    }
                }
            }
            Ok(seq)
        })
        .await?;
    for r in state
        .services
        .requests
        .list()
        .into_iter()
        .filter(|r| r.sequence_id == seq.id)
    {
        state.services.requests.remove(&r.id);
    }
    media_svc::trash(&state.config.data_dir, &seq.file);
    if let Some(t) = &seq.thumbnail {
        media_svc::trash(&state.config.data_dir, t);
    }
    crate::services::analysis::forget_sequence(&state, &seq.id);
    Ok(Json(json!({ "ok": true })))
}

async fn sequence_thumbnail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let show = state.store.get();
    let seq = show
        .sequence(&id)
        .ok_or_else(|| ApiError::not_found("That sequence"))?;
    // Paths come from show.json: only well-formed data paths (services::paths).
    let data_dir = &state.config.data_dir;
    let rel = seq
        .thumbnail
        .clone()
        .unwrap_or_else(|| format!("thumbnails/{id}.png"));
    let path = paths::resolve(data_dir, &rel, paths::Kind::Thumbnail)
        .ok_or_else(|| ApiError::not_found("A preview for that sequence"))?;
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(_) => {
            // Draw it now (older uploads, or a restored show).
            let src = paths::resolve(data_dir, &seq.file, paths::Kind::Sequence)
                .ok_or_else(|| ApiError::not_found("A preview for that sequence"))?;
            let (dst, props) = (path.clone(), show.props.clone());
            let _ = tokio::fs::create_dir_all(state.config.thumbnails_dir()).await;
            tokio::task::spawn_blocking(move || generate_thumbnail(&src, &props, &dst))
                .await
                .map_err(ApiError::internal)?
                .map_err(|_| ApiError::not_found("A preview for that sequence"))?;
            tokio::fs::read(&path).await?
        }
    };
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "max-age=60"),
        ],
        bytes,
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Media
// ---------------------------------------------------------------------------

async fn list_media(State(state): State<AppState>) -> Json<Vec<Media>> {
    Json(state.store.get().media.clone())
}

async fn get_media(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Media>> {
    state
        .store
        .get()
        .media_item(&id)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::not_found("That audio file"))
}

async fn create_media(State(state): State<AppState>, req: Request) -> ApiResult<Json<Value>> {
    if is_json(req.headers()) {
        let Json(m): Json<Media> = Json::from_request_json(req).await?;
        // Undo of a delete: only this item's own file (the body comes from the client,
        // and /media/:id/file serves whatever `file` names).
        if !restored_media_path_ok(&m) {
            return Err(ApiError::bad_request(
                "That isn't an audio file PixelPlus stored.",
            ));
        }
        let data = state.config.data_dir.clone();
        media_svc::untrash(&data, &m.file);
        media_svc::untrash(&data, &format!("media/{}.meta.json", m.id));
        if !data.join(&m.file).is_file() {
            return Err(ApiError::bad_request(
                "The audio file is gone. Please upload it again.",
            ));
        }
        let (m, _) = state
            .store
            .update(move |show| {
                if show.media_item(&m.id).is_some() {
                    return Err(ApiError::conflict("That audio file already exists."));
                }
                show.media.push(m.clone());
                Ok(m)
            })
            .await?;
        return Ok(Json(serde_json::to_value(m).map_err(ApiError::internal)?));
    }
    let mut mp = Multipart::from_request_multipart(req).await?;
    let upload_id = new_id();
    let mut kind = MediaKind::Song;
    let mut name: Option<String> = None;
    let mut file: Option<(PathBuf, String)> = None;
    while let Some(field) = mp.next_field().await.map_err(multipart_error)? {
        let fname = field.file_name().map(str::to_string);
        match (field.name().unwrap_or_default(), fname) {
            ("kind", None) => {
                let k = field.text().await.unwrap_or_default();
                kind = serde_json::from_value(json!(k.trim()))
                    .map_err(|_| ApiError::bad_request("Kind must be song, dj or sfx."))?;
            }
            ("name", None) => name = field.text().await.ok(),
            (_, Some(fname)) if file.is_none() => {
                let ext = media_svc::audio_ext(&fname).ok_or_else(|| {
                    ApiError::bad_request(format!(
                        "\"{fname}\" isn't a supported audio file. Use MP3, OGG, M4A, WAV or FLAC."
                    ))
                })?;
                let tmp = state
                    .config
                    .media_dir()
                    .join(format!(".upload-{upload_id}.{ext}"));
                save_field(field, &tmp, AUDIO_MAX).await?;
                file = Some((tmp, fname));
            }
            _ => {}
        }
    }
    let (tmp, fname) =
        file.ok_or_else(|| ApiError::bad_request("Choose an audio file to upload."))?;
    let done = import_media_file(
        &state,
        tmp,
        &fname,
        MediaImport {
            kind,
            name,
            replace_same_name: false,
        },
    )
    .await?;
    Ok(Json(
        serde_json::to_value(done.media).map_err(ApiError::internal)?,
    ))
}

async fn update_media(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(mut patch): Json<Value>,
) -> ApiResult<Json<Media>> {
    if let Value::Object(p) = &mut patch {
        p.retain(|k, _| matches!(k.as_str(), "name" | "kind" | "gainDb" | "tags"));
    }
    let (m, _) = state
        .store
        .update(move |show| {
            let idx = show
                .media
                .iter()
                .position(|m| m.id == id)
                .ok_or_else(|| ApiError::not_found("That audio file"))?;
            let mut v = serde_json::to_value(&show.media[idx]).map_err(ApiError::internal)?;
            merge_patch(&mut v, &patch);
            let mut m: Media = serde_json::from_value(v)
                .map_err(|e| ApiError::bad_request(format!("That change isn't valid: {e}")))?;
            keep_media_facts(&show.media[idx], &mut m);
            m.tags = pixelplus_core::smartlist::normalize_tags(&m.tags);
            if m.name.trim().is_empty() {
                return Err(ApiError::bad_request("Please give it a name."));
            }
            if let Some(g) = m.gain_db {
                m.gain_db = Some(g.clamp(-30.0, 20.0));
            }
            show.media[idx] = m.clone();
            Ok(m)
        })
        .await?;
    Ok(Json(m))
}

async fn delete_media(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    if state.store.get().media_item(&id).is_none() {
        return Err(ApiError::not_found("That audio file"));
    }
    crate::services::snapshots::auto(&state, "Before deleting audio").await;
    let (m, _) = state
        .store
        .update(|show| {
            let idx = show
                .media
                .iter()
                .position(|m| m.id == id)
                .ok_or_else(|| ApiError::not_found("That audio file"))?;
            let m = show.media.remove(idx);
            for s in show
                .sequences
                .iter_mut()
                .filter(|s| s.media_id.as_deref() == Some(id.as_str()))
            {
                s.media_id = None;
            }
            for c in &mut show.dj_clips {
                if c.media_id.as_deref() == Some(id.as_str()) {
                    c.media_id = None;
                }
                if c.music_bed_media_id.as_deref() == Some(id.as_str()) {
                    c.music_bed_media_id = None;
                }
            }
            for p in &mut show.playlists {
                for list in [&mut p.items, &mut p.intro, &mut p.outro] {
                    list.retain(
                        |i| !matches!(i, PlaylistItem::Media { media_id, .. } if *media_id == id),
                    );
                }
                if let Some(r) = &mut p.smart {
                    for list in [&mut r.pinned_first, &mut r.pinned_last, &mut r.interleave] {
                        list.retain(
                            |i| !matches!(i, PlaylistItem::Media { media_id, .. } if *media_id == id),
                        );
                    }
                }
            }
            Ok(m)
        })
        .await?;
    media_svc::trash(&state.config.data_dir, &m.file);
    media_svc::trash(&state.config.data_dir, &format!("media/{}.meta.json", m.id));
    crate::services::analysis::forget(&state, &m.id);
    Ok(Json(json!({ "ok": true })))
}

/// Serve the audio file (supports Range requests for seeking).
async fn media_file(
    State(state): State<AppState>,
    Path(id): Path<String>,
    req: Request,
) -> ApiResult<Response> {
    let show = state.store.get();
    let m = show
        .media_item(&id)
        .ok_or_else(|| ApiError::not_found("That audio file"))?;
    // Never a path from show.json as such (services::paths), and only ever
    // served as audio: a whitelisted type, `nosniff` (global) and a download
    // disposition, so nothing uploaded can run as a page on this origin.
    let path = paths::resolve(&state.config.data_dir, &m.file, paths::Kind::Media)
        .filter(|p| p.is_file())
        .ok_or_else(|| ApiError::not_found("The audio file on disk"))?;
    let mime = audio_mime(&m.file).ok_or_else(|| ApiError::not_found("The audio file on disk"))?;
    let resp = tower_http::services::ServeFile::new_with_mime(
        path,
        &mime.parse().map_err(ApiError::internal)?,
    )
    .oneshot(req)
    .await
    .map_err(ApiError::internal)?;
    let mut resp = resp.map(axum::body::Body::new);
    let name: String = media_original(&state, m)
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '"' | '\\' | ';'))
        .collect();
    let ascii: String = name
        .chars()
        .map(|c| if c.is_ascii() { c } else { '_' })
        .collect();
    let disp = format!("attachment; filename=\"{ascii}\"");
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disp).unwrap_or(HeaderValue::from_static("attachment")),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(resp)
}

/// The only content types audio files are served with.
pub(crate) fn audio_mime(rel: &str) -> Option<&'static str> {
    let ext = rel.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "mp3" => "audio/mpeg",
        "ogg" | "oga" => "audio/ogg",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        _ => return None,
    })
}

#[derive(Deserialize)]
struct PeaksQuery {
    #[serde(default)]
    n: Option<usize>,
}

async fn media_peaks(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<PeaksQuery>,
) -> ApiResult<Json<Vec<f32>>> {
    let show = state.store.get();
    let m = show
        .media_item(&id)
        .ok_or_else(|| ApiError::not_found("That audio file"))?
        .clone();
    let n = q.n.unwrap_or(160).clamp(8, 4000);
    let dir = state.config.media_dir();
    let meta = match media_svc::read_meta(&dir, &id).filter(|m| !m.peaks.is_empty()) {
        Some(meta) => meta,
        None => {
            let path = state.config.data_dir.join(&m.file);
            let meta: MediaMeta = tokio::task::spawn_blocking(move || media_svc::analyze(&path))
                .await
                .map_err(ApiError::internal)?
                .map_err(|e| {
                    ApiError::bad_request(format!("Couldn't read that audio file: {e}"))
                })?;
            let _ = media_svc::write_meta(&dir, &id, &meta);
            meta
        }
    };
    Ok(Json(media_svc::resample_peaks(&meta.peaks, n)))
}

// ---------------------------------------------------------------------------
// DJ clips & TTS
// ---------------------------------------------------------------------------

async fn render_clip(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<DjClip>> {
    crate::services::tts::render_clip(&state, &id)
        .await
        .map(Json)
}

async fn upload_clip(
    State(state): State<AppState>,
    Path(id): Path<String>,
    mut mp: Multipart,
) -> ApiResult<Json<DjClip>> {
    let clip = state
        .store
        .get()
        .dj_clip(&id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("That DJ clip"))?;
    let upload_id = new_id();
    let mut saved: Option<(PathBuf, String)> = None;
    while let Some(field) = mp.next_field().await.map_err(multipart_error)? {
        let Some(fname) = field.file_name().map(str::to_string) else {
            continue;
        };
        if saved.is_some() {
            continue;
        }
        let ext = media_svc::audio_ext(&fname).unwrap_or_else(|| "wav".into());
        let tmp = state
            .config
            .media_dir()
            .join(format!(".upload-{upload_id}.{ext}"));
        save_field(field, &tmp, 100 * 1024 * 1024).await?;
        saved = Some((tmp, ext));
    }
    let (tmp, ext) = saved.ok_or_else(|| ApiError::bad_request("No audio was uploaded."))?;
    // Browser renders are WAV: store MP3 when ffmpeg is available.
    let (bytes, ext) = if ext == "wav" {
        let mp3 = tmp.with_extension("mp3");
        match media_svc::transcode_mp3(&tmp, &mp3).await {
            Ok(Some(())) => {
                let b = tokio::fs::read(&mp3).await?;
                let _ = tokio::fs::remove_file(&mp3).await;
                (b, "mp3".to_string())
            }
            Ok(None) => (tokio::fs::read(&tmp).await?, ext),
            Err(e) => {
                tracing::warn!("{e}; keeping the WAV");
                let _ = tokio::fs::remove_file(&mp3).await;
                (tokio::fs::read(&tmp).await?, ext)
            }
        }
    } else {
        (tokio::fs::read(&tmp).await?, ext)
    };
    let _ = tokio::fs::remove_file(&tmp).await;
    // Validate it is audio before storing.
    let probe = state
        .config
        .media_dir()
        .join(format!(".probe-{upload_id}.{ext}"));
    tokio::fs::write(&probe, &bytes).await?;
    let p = probe.clone();
    let ok = tokio::task::spawn_blocking(move || media_svc::analyze(&p))
        .await
        .map_err(ApiError::internal)?;
    let _ = tokio::fs::remove_file(&probe).await;
    if let Err(e) = ok {
        return Err(ApiError::bad_request(format!(
            "That recording couldn't be read ({e}). Please render it again."
        )));
    }
    crate::services::tts::save_clip_audio(&state, &clip, &bytes, &ext, None, None)
        .await
        .map(Json)
}

async fn tts_status(State(state): State<AppState>) -> Json<Value> {
    Json(crate::services::tts::status(&state).await)
}

async fn tts_voices(State(state): State<AppState>) -> Json<Value> {
    Json(crate::services::tts::voices(&state).await)
}

fn audio_response(r: crate::services::tts::Rendered) -> Response {
    let mut resp = (StatusCode::OK, r.bytes).into_response();
    let h = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&r.content_type) {
        h.insert(header::CONTENT_TYPE, v);
    }
    if let Some(d) = r.duration_ms {
        h.insert("x-duration-ms", HeaderValue::from(d));
    }
    if let Some(l) = r
        .loudness_lufs
        .and_then(|l| HeaderValue::from_str(&l.to_string()).ok())
    {
        h.insert("x-loudness-lufs", l);
    }
    if let Some(w) = r.warnings.and_then(|w| HeaderValue::from_str(&w).ok()) {
        h.insert("x-warnings", w);
    }
    resp
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TtsRenderBody {
    lines: Vec<Value>,
    #[serde(default)]
    speed: Option<f32>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    context: Option<crate::services::tts::DynamicContext>,
}

async fn tts_render(
    State(state): State<AppState>,
    Json(body): Json<TtsRenderBody>,
) -> ApiResult<Response> {
    if body.lines.is_empty() {
        return Err(ApiError::bad_request(
            "Write something for the DJ to say first.",
        ));
    }
    if crate::services::tts::resolved_mode(&state).await != "device" {
        return Err(ApiError::unavailable(
            "On-device voices aren't available here; your browser will render instead.",
        ));
    }
    let show = state.store.get();
    let ctx = body
        .context
        .unwrap_or_else(|| crate::services::tts::context_from_player(&state));
    let fmt = match body.format.as_deref() {
        Some("wav") => "wav",
        Some("ogg") => "ogg",
        _ => "mp3",
    };
    let req =
        crate::services::tts::render_body(&show, &body.lines, body.speed.unwrap_or(1.0), &ctx, fmt);
    Ok(audio_response(
        crate::services::tts::post_audio(&state, "/render", &req).await?,
    ))
}

async fn tts_audition(
    State(state): State<AppState>,
    Json(mut body): Json<Value>,
) -> ApiResult<Response> {
    let show = state.store.get();
    if let Some(v) = body.get("voice").and_then(Value::as_str) {
        if let Some(dv) = show.dj_voices.iter().find(|d| d.id == v) {
            body["voice"] = serde_json::to_value(dv).map_err(ApiError::internal)?;
        }
    }
    if body.get("voice").is_none() {
        return Err(ApiError::bad_request("Pick a voice to hear."));
    }
    Ok(audio_response(
        crate::services::tts::post_audio(&state, "/audition", &body).await?,
    ))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/sequences",
            // Size limits are enforced while streaming (save_field); a body limit
            // here would also overflow on 32-bit Pis.
            get(list_sequences)
                .post(create_sequence)
                .layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/sequences/{id}",
            get(get_sequence)
                .put(update_sequence)
                .patch(update_sequence)
                .delete(delete_sequence),
        )
        .route("/sequences/{id}/thumbnail", get(sequence_thumbnail))
        .route(
            "/media",
            get(list_media)
                .post(create_media)
                .layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/media/{id}",
            get(get_media)
                .put(update_media)
                .patch(update_media)
                .delete(delete_media),
        )
        .route("/media/{id}/file", get(media_file))
        .route("/media/{id}/peaks", get(media_peaks))
        .route("/dj-clips/{id}/render", post(render_clip))
        .route(
            "/dj-clips/{id}/upload",
            post(upload_clip).layer(DefaultBodyLimit::max(101 * 1024 * 1024)),
        )
        .route("/tts/status", get(tts_status))
        .route("/tts/voices", get(tts_voices))
        .route("/tts/render", post(tts_render))
        .route("/tts/audition", post(tts_audition))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_can_only_restore_its_own_files() {
        let seq = |id: &str, file: &str, thumb: Option<&str>| Sequence {
            generated: Default::default(),
            tags: Default::default(),
            id: id.into(),
            name: "S".into(),
            file: file.into(),
            duration_ms: 1,
            frame_ms: 50,
            channel_count: 3,
            media_id: None,
            xlights_name: None,
            thumbnail: thumb.map(String::from),
            hash: String::new(),
        };
        assert!(check_restored_paths(&seq(
            "abc",
            "sequences/abc.fseq",
            Some("thumbnails/abc.png")
        ))
        .is_ok());
        assert!(check_restored_paths(&seq("abc", "sequences/abc.fseq", None)).is_ok());
        assert!(check_restored_paths(&seq("abc", "../../etc/passwd", None)).is_err());
        assert!(check_restored_paths(&seq("abc", "sequences/other.fseq", None)).is_err());
        assert!(
            check_restored_paths(&seq("abc", "sequences/abc.fseq", Some("/etc/shadow"))).is_err()
        );
        assert!(check_restored_paths(&seq("../x", "sequences/../x.fseq", None)).is_err());
    }

    #[test]
    fn edits_cannot_repoint_files() {
        let old = Media {
            tags: Default::default(),
            analysis: Default::default(),
            original_name: Default::default(),
            original_size: Default::default(),
            id: "abc".into(),
            name: "Song".into(),
            kind: MediaKind::Song,
            file: "media/abc.mp3".into(),
            duration_ms: 1000,
            loudness_lufs: Some(-14.0),
            gain_db: None,
        };
        let mut new = Media {
            id: "zzz".into(),
            name: "Renamed".into(),
            file: "/etc/shadow".into(),
            duration_ms: 5,
            ..old.clone()
        };
        keep_media_facts(&old, &mut new);
        assert_eq!(
            (new.id.as_str(), new.file.as_str(), new.duration_ms),
            ("abc", "media/abc.mp3", 1000)
        );
        assert_eq!(new.name, "Renamed");
    }

    #[test]
    fn undo_can_only_restore_its_own_media_file() {
        let m = |id: &str, file: &str| Media {
            tags: Default::default(),
            analysis: Default::default(),
            original_name: Default::default(),
            original_size: Default::default(),
            id: id.into(),
            name: "Song".into(),
            kind: MediaKind::Song,
            file: file.into(),
            duration_ms: 1,
            loudness_lufs: None,
            gain_db: None,
        };
        assert!(restored_media_path_ok(&m("abc", "media/abc.mp3")));
        assert!(!restored_media_path_ok(&m("abc", "/etc/shadow")));
        assert!(!restored_media_path_ok(&m(
            "abc",
            "media/abc.mp3/../../../etc/shadow"
        )));
        assert!(!restored_media_path_ok(&m("abc", "media/other.mp3")));
        assert!(!restored_media_path_ok(&m("..", "media/...mp3")));
    }

    #[test]
    fn channel_warning_text() {
        let prop = |name: &str, start: u32, px: u32| Prop {
            suspect_pixels: Default::default(),
            id: name.into(),
            name: name.into(),
            kind: Default::default(),
            pixel_count: px,
            xlights_model: None,
            channel_start: start,
            channels_per_pixel: 3,
            channel_runs: None,
            segments: vec![],
            group_ids: vec![],
            layout: None,
            matrix: None,
            color: None,
            max_milliamps_per_pixel: None,
            notes: None,
        };
        let props = vec![prop("Arch", 0, 50), prop("Tree", 150, 100)];
        assert!(channel_warnings(&props, 450).is_empty());
        let w = channel_warnings(&props, 300);
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("Tree") && !w[0].contains("Arch"), "{}", w[0]);
    }
}
