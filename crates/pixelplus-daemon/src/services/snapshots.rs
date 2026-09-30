//! Time machine: versioned backups of the show.
//!
//! A snapshot is `snapshots/<id>.tar.zst` holding `snapshot.json` (metadata),
//! `show.json` and the small files the show references (DJ/sfx audio and
//! their meta files, thumbnails). With `full` it also holds the sequences and
//! all audio. A `snapshots/<id>.json` sidecar keeps listing fast.
//!
//! Automatic snapshots are taken before destructive operations and daily;
//! the newest [`KEEP_AUTO`] automatic snapshots are kept, manual ones forever.

use crate::api::{ApiError, ApiResult};
use crate::state::AppState;
use pixelplus_core::model::{MediaKind, Show};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const KEEP_AUTO: usize = 30;
const META_NAME: &str = "snapshot.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub id: String,
    pub label: String,
    /// RFC 3339.
    pub created_at: String,
    #[serde(default)]
    pub size_bytes: u64,
    #[serde(default)]
    pub show_version: Option<u64>,
    #[serde(default)]
    pub auto: bool,
    #[serde(default)]
    pub full: bool,
    #[serde(default)]
    pub show_name: Option<String>,
}

pub fn dir(state: &AppState) -> PathBuf {
    state.config.snapshots_dir()
}

fn slug(label: &str) -> String {
    let s: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let s = s
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let s: String = s.chars().take(40).collect();
    if s.is_empty() {
        "snapshot".into()
    } else {
        s
    }
}

/// Valid snapshot ids are file-name safe.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 100
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn archive_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.tar.zst"))
}

/// Files (relative to the data dir) to include.
fn referenced_files(show: &Show, data_dir: &Path, full: bool) -> Vec<String> {
    let mut files = Vec::new();
    for m in &show.media {
        if full || m.kind != MediaKind::Song {
            files.push(m.file.clone());
            files.push(format!("media/{}.meta.json", m.id));
        }
    }
    for s in &show.sequences {
        if let Some(t) = &s.thumbnail {
            files.push(t.clone());
        }
        if full {
            files.push(s.file.clone());
        }
    }
    files.retain(|f| safe_rel(f) && data_dir.join(f).is_file());
    files.sort();
    files.dedup();
    files
}

/// Only well-formed data files (`media/<id>.<audio|meta.json>`,
/// `sequences/<id>.fseq|ppseq`, `thumbnails/<id>.png`; see services::paths).
fn safe_rel(rel: &str) -> bool {
    super::paths::check(rel).is_some()
}

/// Largest `show.json` inside a snapshot.
const MAX_SHOW_JSON: u64 = 64 * 1024 * 1024;
/// Most bytes a restore may unpack (and never more than the free space
/// minus [`KEEP_FREE`]).
const MAX_RESTORE_BYTES: u64 = 16 * 1024 * 1024 * 1024;
const KEEP_FREE: u64 = 256 * 1024 * 1024;

/// Create a snapshot (blocking work runs on a worker thread).
pub async fn create(state: &AppState, label: &str, auto: bool, full: bool) -> ApiResult<Snapshot> {
    let show = state.store.get();
    let dir = dir(state);
    let data_dir = state.config.data_dir.clone();
    let label = if label.trim().is_empty() {
        "Snapshot".to_string()
    } else {
        label.trim().chars().take(100).collect()
    };
    let now = chrono::Local::now();
    let mut id = format!("{}-{}", now.format("%Y%m%d-%H%M%S"), slug(&label));
    if archive_path(&dir, &id).exists() {
        id = format!("{id}-{}", &pixelplus_core::model::new_id()[..4]);
    }
    let meta = Snapshot {
        id: id.clone(),
        label,
        created_at: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        size_bytes: 0,
        show_version: Some(show.version),
        auto,
        full,
        show_name: Some(show.name.clone()),
    };
    let snap = tokio::task::spawn_blocking(move || -> anyhow::Result<Snapshot> {
        std::fs::create_dir_all(&dir)?;
        let files = referenced_files(&show, &data_dir, full);
        let tmp = dir.join(format!(".{id}.tmp"));
        // A failed write (full SD card) must not leave a partial archive behind:
        // the daily snapshot would add another one every day.
        struct RemoveOnDrop(Option<PathBuf>);
        impl Drop for RemoveOnDrop {
            fn drop(&mut self) {
                if let Some(p) = self.0.take() {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
        let mut guard = RemoveOnDrop(Some(tmp.clone()));
        {
            let f = std::fs::File::create(&tmp)?;
            let level = if full { 1 } else { 9 };
            let enc = zstd::Encoder::new(f, level)?;
            let mut tar = tar::Builder::new(enc);
            append_bytes(&mut tar, META_NAME, &serde_json::to_vec_pretty(&meta)?)?;
            let mut show_clean = (*show).clone();
            show_clean.nodes.iter_mut().for_each(|n| n.last_seen = None);
            append_bytes(
                &mut tar,
                "show.json",
                &serde_json::to_vec_pretty(&show_clean)?,
            )?;
            for rel in &files {
                tar.append_path_with_name(data_dir.join(rel), rel)?;
            }
            let enc = tar.into_inner()?;
            let f = enc.finish()?;
            f.sync_all()?;
        }
        let path = archive_path(&dir, &id);
        std::fs::rename(&tmp, &path)?;
        guard.0 = None;
        let mut meta = meta;
        meta.size_bytes = std::fs::metadata(&path)?.len();
        std::fs::write(
            dir.join(format!("{id}.json")),
            serde_json::to_vec_pretty(&meta)?,
        )?;
        Ok(meta)
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(|e| ApiError::internal(format!("couldn't save the snapshot: {e:#}")));
    // The compressor's working memory (tens of MB) is freed, but glibc keeps it
    // in the blocking thread's arena: without this the daemon grows ~10 MB per
    // snapshot (up to the arena limit) on a 512 MB Pi Zero 2.
    release_free_memory();
    let snap = snap?;
    if auto {
        prune(state).await;
    }
    Ok(snap)
}

/// Hand memory freed by large, short-lived work back to the OS.
pub fn release_free_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: malloc_trim only releases free heap memory; no pointers are involved.
    unsafe {
        libc::malloc_trim(0);
    }
}

fn append_bytes<W: std::io::Write>(
    tar: &mut tar::Builder<W>,
    name: &str,
    data: &[u8],
) -> std::io::Result<()> {
    let mut h = tar::Header::new_gnu();
    h.set_size(data.len() as u64);
    h.set_mode(0o644);
    h.set_mtime(chrono::Utc::now().timestamp().max(0) as u64);
    h.set_cksum();
    tar.append_data(&mut h, name, data)
}

/// Automatic snapshot before a destructive operation (skipped if the newest
/// snapshot already has this show version). Never fails the caller.
pub async fn auto(state: &AppState, label: &str) {
    let version = state.store.version();
    if list(state)
        .await
        .first()
        .is_some_and(|s| s.show_version == Some(version))
    {
        return;
    }
    if let Err(e) = create(state, label, true, false).await {
        tracing::warn!("Automatic snapshot \"{label}\" failed: {}", e.message);
    }
}

/// All snapshots, newest first.
pub async fn list(state: &AppState) -> Vec<Snapshot> {
    let dir = dir(state);
    tokio::task::spawn_blocking(move || list_blocking(&dir))
        .await
        .unwrap_or_default()
}

fn list_blocking(dir: &Path) -> Vec<Snapshot> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(id) = name.strip_suffix(".tar.zst") else {
            continue;
        };
        if !valid_id(id) {
            continue;
        }
        let path = e.path();
        let size = e.metadata().map(|m| m.len()).unwrap_or(0);
        let sidecar = dir.join(format!("{id}.json"));
        let meta = std::fs::read(&sidecar)
            .ok()
            .and_then(|b| serde_json::from_slice::<Snapshot>(&b).ok())
            .or_else(|| {
                let m = read_archive_meta(&path).ok()?;
                let _ = std::fs::write(&sidecar, serde_json::to_vec_pretty(&m).ok()?);
                Some(m)
            });
        let mut meta = meta.unwrap_or_else(|| Snapshot {
            id: id.to_string(),
            label: id.to_string(),
            created_at: e
                .metadata()
                .and_then(|m| m.modified())
                .map(|t| chrono::DateTime::<chrono::Local>::from(t).to_rfc3339())
                .unwrap_or_default(),
            size_bytes: size,
            show_version: None,
            auto: false,
            full: false,
            show_name: None,
        });
        meta.id = id.to_string();
        meta.size_bytes = size;
        out.push(meta);
    }
    sort_newest_first(&mut out);
    out
}

/// By creation instant: the RFC 3339 strings carry the local UTC offset, so
/// comparing them as text misorders snapshots across a DST change or a time
/// zone change (which decides what "newest" and pruning keep).
fn sort_newest_first(list: &mut [Snapshot]) {
    let at = |s: &Snapshot| {
        chrono::DateTime::parse_from_rfc3339(&s.created_at)
            .map(|t| t.timestamp())
            .ok()
    };
    list.sort_by(|a, b| {
        at(b)
            .cmp(&at(a))
            .then_with(|| b.created_at.cmp(&a.created_at))
            .then(b.id.cmp(&a.id))
    });
}

fn read_archive_meta(path: &Path) -> anyhow::Result<Snapshot> {
    let f = std::fs::File::open(path)?;
    let mut ar = tar::Archive::new(zstd::Decoder::new(f)?);
    for entry in ar.entries()? {
        let mut entry = entry?;
        if entry.path()?.to_string_lossy() == META_NAME {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            return Ok(serde_json::from_slice(&buf)?);
        }
    }
    anyhow::bail!("no snapshot.json")
}

/// Read `show.json` out of an archive (validates it is a PixelPlus snapshot).
pub fn read_archive_show(path: &Path) -> Result<Show, String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let dec =
        zstd::Decoder::new(f).map_err(|_| "That isn't a PixelPlus snapshot file.".to_string())?;
    let mut ar = tar::Archive::new(dec);
    let entries = ar
        .entries()
        .map_err(|_| "That isn't a PixelPlus snapshot file.".to_string())?;
    for entry in entries {
        let mut entry = entry.map_err(|_| "The snapshot file is damaged.".to_string())?;
        let is_show = entry
            .path()
            .map(|p| p.to_string_lossy() == "show.json")
            .unwrap_or(false);
        if is_show {
            if entry.header().size().unwrap_or(u64::MAX) > MAX_SHOW_JSON {
                return Err("The show in that snapshot is too large.".into());
            }
            let mut buf = Vec::new();
            (&mut entry)
                .take(MAX_SHOW_JSON)
                .read_to_end(&mut buf)
                .map_err(|_| "The snapshot file is damaged.".to_string())?;
            return serde_json::from_slice(&buf)
                .map_err(|e| format!("The show in that snapshot can't be read: {e}"));
        }
    }
    Err("That isn't a PixelPlus snapshot (it has no show.json).".into())
}

/// Restore: auto-snapshot the current show, unpack files, replace the show.
pub async fn restore(state: &AppState, id: &str) -> ApiResult<std::sync::Arc<Show>> {
    if !valid_id(id) {
        return Err(ApiError::not_found("That snapshot"));
    }
    let path = archive_path(&dir(state), id);
    if !path.exists() {
        return Err(ApiError::not_found("That snapshot"));
    }
    auto(state, "Before restore").await;
    let data_dir = state.config.data_dir.clone();
    let show = tokio::task::spawn_blocking(move || -> Result<Show, String> {
        let show = read_archive_show(&path)?;
        let free = super::system::disk_space(&data_dir)
            .map(|(f, _)| f)
            .unwrap_or(u64::MAX);
        let budget = MAX_RESTORE_BYTES.min(free.saturating_sub(KEEP_FREE));
        let mut unpacked: u64 = 0;
        let f = std::fs::File::open(&path).map_err(|e| e.to_string())?;
        let mut ar = tar::Archive::new(zstd::Decoder::new(f).map_err(|e| e.to_string())?);
        for entry in ar.entries().map_err(|e| e.to_string())? {
            let mut entry = entry.map_err(|e| e.to_string())?;
            let rel = entry
                .path()
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .to_string();
            if !safe_rel(&rel) || entry.header().entry_type() != tar::EntryType::Regular {
                continue;
            }
            // The size is declared up front: refuse decompression bombs and
            // archives that would fill the SD card.
            unpacked = unpacked.saturating_add(entry.header().size().unwrap_or(u64::MAX));
            if unpacked > budget {
                return Err(
                    "That snapshot is too large to restore here (not enough free space).".into(),
                );
            }
            let dst = data_dir.join(&rel);
            if let Some(p) = dst.parent() {
                std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            let tmp = dst.with_extension("restore.tmp");
            entry
                .unpack(&tmp)
                .map_err(|e| format!("couldn't unpack {rel}: {e}"))?;
            std::fs::rename(&tmp, &dst).map_err(|e| e.to_string())?;
        }
        Ok(show)
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(ApiError::bad_request);
    release_free_memory();
    let show = show?;
    // Keep this device's security settings and its own leader node identity.
    // File paths come from the archive: rebuild them from ids (services::paths;
    // the store does it too).
    let current = state.store.get();
    let mut show = show;
    super::paths::sanitize_show(&mut show);
    show.settings.security = current.settings.security.clone();
    let restored = state.store.replace(show).await.map_err(ApiError::from)?;
    Ok(restored)
}

pub async fn delete(state: &AppState, id: &str) -> ApiResult<()> {
    if !valid_id(id) {
        return Err(ApiError::not_found("That snapshot"));
    }
    let d = dir(state);
    let path = archive_path(&d, id);
    if !path.exists() {
        return Err(ApiError::not_found("That snapshot"));
    }
    tokio::fs::remove_file(&path).await?;
    let _ = tokio::fs::remove_file(d.join(format!("{id}.json"))).await;
    Ok(())
}

pub fn archive_file(state: &AppState, id: &str) -> Option<PathBuf> {
    let p = archive_path(&dir(state), id);
    (valid_id(id) && p.exists()).then_some(p)
}

/// Adopt an uploaded archive (already written to `tmp`) as a snapshot.
pub async fn import(state: &AppState, tmp: PathBuf, original_name: &str) -> ApiResult<Snapshot> {
    let d = dir(state);
    let name = original_name.to_string();
    tokio::task::spawn_blocking(move || -> Result<Snapshot, String> {
        let res = (|| {
            let show = read_archive_show(&tmp)?;
            let meta = read_archive_meta(&tmp).ok();
            let now = chrono::Local::now();
            let label = format!(
                "Imported: {}",
                if name.is_empty() { "backup" } else { &name }
            );
            let id = format!(
                "{}-{}",
                now.format("%Y%m%d-%H%M%S"),
                slug(&format!("imported {}", name.trim_end_matches(".tar.zst")))
            );
            let dst = archive_path(&d, &id);
            std::fs::rename(&tmp, &dst).map_err(|e| e.to_string())?;
            let snap = Snapshot {
                id: id.clone(),
                label,
                created_at: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
                size_bytes: std::fs::metadata(&dst).map(|m| m.len()).unwrap_or(0),
                show_version: Some(show.version),
                auto: false,
                full: meta.map(|m| m.full).unwrap_or(false),
                show_name: Some(show.name),
            };
            let _ = std::fs::write(
                d.join(format!("{id}.json")),
                serde_json::to_vec_pretty(&snap).unwrap_or_default(),
            );
            Ok(snap)
        })();
        if res.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        res
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(ApiError::bad_request)
}

/// Keep the newest [`KEEP_AUTO`] automatic snapshots.
pub async fn prune(state: &AppState) {
    let all = list(state).await;
    let d = dir(state);
    for s in all.iter().filter(|s| s.auto).skip(KEEP_AUTO) {
        let _ = tokio::fs::remove_file(archive_path(&d, &s.id)).await;
        let _ = tokio::fs::remove_file(d.join(format!("{}.json", s.id))).await;
    }
}

/// Daily automatic snapshot (when the show changed since the last one).
pub fn start(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(120)).await;
        let mut tick = tokio::time::interval(Duration::from_secs(3600));
        loop {
            tick.tick().await;
            let all = list(&state).await;
            let version = state.store.version();
            let latest_version = all.first().and_then(|s| s.show_version);
            let last_auto_age = all
                .iter()
                .find(|s| s.auto)
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s.created_at).ok())
                .map(|t| chrono::Utc::now().signed_duration_since(t));
            let due = last_auto_age.map_or(true, |a| a > chrono::Duration::hours(24));
            if due && latest_version != Some(version) {
                if let Err(e) = create(&state, "Daily", true, false).await {
                    tracing::warn!("Daily snapshot failed: {}", e.message);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_and_ids() {
        assert_eq!(slug("Before xLights import!"), "before-xlights-import");
        assert_eq!(slug("   "), "snapshot");
        assert!(valid_id("20261201-183000-daily"));
        assert!(!valid_id("../etc"));
        assert!(safe_rel("media/abc.mp3"));
        assert!(!safe_rel("media/../../etc/passwd"));
        assert!(!safe_rel("show.json"));
        assert!(!safe_rel("/etc/passwd"));
    }

    #[test]
    fn newest_first_across_a_dst_change() {
        let snap = |id: &str, at: &str| Snapshot {
            id: id.into(),
            label: id.into(),
            created_at: at.into(),
            size_bytes: 0,
            show_version: None,
            auto: true,
            full: false,
            show_name: None,
        };
        // Clocks fall back at 02:00 CDT: 01:10 CST is 40 minutes after 01:30 CDT.
        let mut list = vec![
            snap("before", "2026-11-01T01:30:00-05:00"),
            snap("after", "2026-11-01T01:10:00-06:00"),
            snap("older", "2026-10-31T23:00:00-05:00"),
        ];
        sort_newest_first(&mut list);
        let ids: Vec<_> = list.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["after", "before", "older"]);
    }
}
