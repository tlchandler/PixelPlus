//! Tauri commands for the PixelPlus Imager UI (see ../src/lib/api.ts).

mod elevate;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use futures_util::StreamExt;
use pixelplus_imager_core::drives::{self, Drive};
use pixelplus_imager_core::helper::cancel_path;
use pixelplus_imager_core::job::WriteJob;
use pixelplus_imager_core::release::{self, OsImage};
use pixelplus_imager_core::settings::{FieldError, ImagerSettings};
use pixelplus_imager_core::write::{Phase, Progress};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, State};
use tokio::io::AsyncWriteExt;

const USER_AGENT: &str = concat!("PixelPlus-Imager/", env!("CARGO_PKG_VERSION"));

#[derive(Default)]
struct AppState {
    /// Progress file of the running write (its `.cancel` sibling cancels it).
    running: Mutex<Option<PathBuf>>,
    download_cancel: std::sync::atomic::AtomicBool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Defaults {
    timezone: String,
    country: String,
    timezones: Vec<String>,
    platform: &'static str,
}

#[tauri::command]
fn defaults() -> Defaults {
    let locale = sys_locale::get_locale().unwrap_or_default();
    let country = locale
        .split(['-', '_', '.'])
        .nth(1)
        .filter(|c| c.len() == 2 && c.chars().all(|x| x.is_ascii_alphabetic()))
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or_default();
    Defaults {
        timezone: iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into()),
        country,
        timezones: vec![], // the UI fills this from Intl.supportedValuesOf('timeZone')
        platform: if cfg!(target_os = "linux") {
            "linux"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(windows) {
            "windows"
        } else {
            "unknown"
        },
    }
}

fn http() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn releases() -> Result<Vec<OsImage>, String> {
    let client = http()?;
    let body = client
        .get(release::GITHUB_RELEASES)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("GitHub: {e}"))?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    let rels = release::parse_github_releases(&body).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    // newest stable release first; older stable ones and one newer beta follow
    let mut stable = 0;
    let mut beta = false;
    for r in rels {
        if r.prerelease {
            if beta || stable > 0 {
                continue;
            }
            beta = true;
        } else {
            stable += 1;
            if stable > 3 {
                break;
            }
        }
        let mut imgs = r.images.clone();
        if let Some(url) = &r.repo_json_url {
            if let Ok(resp) = client
                .get(url)
                .send()
                .await
                .and_then(|r| r.error_for_status())
            {
                if let Ok(text) = resp.text().await {
                    if let Ok(parsed) = release::parse_repo_json(&text, &r.version, r.prerelease) {
                        imgs = parsed;
                    }
                }
            }
        }
        out.extend(imgs);
    }
    // stable before beta, recommended first within a version
    out.sort_by_key(|i| {
        (
            i.prerelease,
            std::cmp::Reverse(i.release_date.clone()),
            !i.recommended,
        )
    });
    Ok(out)
}

#[derive(Serialize)]
struct LocalImage {
    path: String,
    name: String,
    size: u64,
}

#[tauri::command]
fn inspect_image(path: String) -> Result<LocalImage, String> {
    let p = Path::new(&path);
    let meta = std::fs::metadata(p).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a file".into());
    }
    Ok(LocalImage {
        name: p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        size: meta.len(),
        path,
    })
}

#[tauri::command]
async fn list_drives() -> Result<Vec<Drive>, String> {
    tauri::async_runtime::spawn_blocking(drives::list)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn validate_settings(settings: ImagerSettings) -> Vec<FieldError> {
    settings.validate()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WriteRequest {
    image: Option<OsImage>,
    local_path: Option<String>,
    device: String,
    settings: ImagerSettings,
}

fn emit(app: &AppHandle, p: Progress) {
    let _ = app.emit("write-progress", p);
}

fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("pixelplus-imager")
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    pixelplus_imager_core::write::sha256_file(path, &mut |_| {})
}

/// Download a release image into the cache (resumable by re-download; verified by SHA-256).
async fn download(app: &AppHandle, state: &AppState, img: &OsImage) -> Result<PathBuf, String> {
    let name = img
        .url
        .rsplit('/')
        .next()
        .filter(|n| !n.is_empty())
        .unwrap_or("pixelplus.img.xz");
    let dir = cache_dir();
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| e.to_string())?;
    let dest = dir.join(name);
    if dest.exists() {
        if let Some(exp) = &img.download_sha256 {
            emit(
                app,
                Progress::msg(Phase::Download, "Checking the downloaded image"),
            );
            let d = dest.clone();
            let ok = tauri::async_runtime::spawn_blocking(move || sha256_file(&d))
                .await
                .map_err(|e| e.to_string())?
                .map(|h| h.eq_ignore_ascii_case(exp))
                .unwrap_or(false);
            if ok {
                return Ok(dest);
            }
        }
    }
    let part = dest.with_extension("part");
    let resp = http()?
        .get(&img.url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("download failed: {e}"))?;
    let total = resp.content_length().or(img.download_size);
    let mut f = tokio::fs::File::create(&part)
        .await
        .map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut got = 0u64;
    let mut stream = resp.bytes_stream();
    let mut last_emit = std::time::Instant::now();
    while let Some(chunk) = stream.next().await {
        if state
            .download_cancel
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            drop(f);
            let _ = tokio::fs::remove_file(&part).await;
            return Err("cancelled".into());
        }
        let chunk = chunk.map_err(|e| format!("download interrupted: {e}"))?;
        hash.update(&chunk);
        f.write_all(&chunk).await.map_err(|e| e.to_string())?;
        got += chunk.len() as u64;
        if last_emit.elapsed() > Duration::from_millis(200) {
            last_emit = std::time::Instant::now();
            emit(app, Progress::new(Phase::Download, got, total));
        }
    }
    f.flush().await.map_err(|e| e.to_string())?;
    drop(f);
    let actual = pixelplus_imager_core::write::hex(&hash.finalize());
    if let Some(exp) = &img.download_sha256 {
        if !exp.eq_ignore_ascii_case(&actual) {
            let _ = tokio::fs::remove_file(&part).await;
            return Err("the download is corrupt (checksum mismatch) - please try again".into());
        }
    }
    tokio::fs::rename(&part, &dest)
        .await
        .map_err(|e| e.to_string())?;
    Ok(dest)
}

fn private_dir() -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("pixelplus-imager-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(data)
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, data)
    }
}

#[tauri::command]
async fn write_card(
    app: AppHandle,
    state: State<'_, AppState>,
    req: WriteRequest,
) -> Result<(), String> {
    if state.running.lock().map_err(|e| e.to_string())?.is_some() {
        return Err("already writing a card".into());
    }
    let errs = req.settings.validate();
    if let Some(e) = errs.first() {
        return Err(e.message.clone());
    }
    state
        .download_cancel
        .store(false, std::sync::atomic::Ordering::Relaxed);

    let (image_path, extract_size, extract_sha) = match (&req.image, &req.local_path) {
        (Some(img), _) => (
            download(&app, &state, img).await?,
            img.extract_size,
            img.extract_sha256.clone(),
        ),
        (None, Some(p)) => (PathBuf::from(p), None, None),
        _ => return Err("no image selected".into()),
    };

    let dir = private_dir().map_err(|e| e.to_string())?;
    let job_path = dir.join("job.json");
    let progress_path = dir.join("progress.jsonl");
    let _ = std::fs::remove_file(&job_path);
    let _ = std::fs::remove_file(&progress_path);
    let job = WriteJob {
        image: image_path,
        device: req.device.clone(),
        settings: req.settings.clone(),
        extract_size,
        extract_sha256: extract_sha,
        verify: true,
    };
    write_private(
        &job_path,
        &serde_json::to_vec(&job).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    write_private(&progress_path, b"").map_err(|e| e.to_string())?;
    *state.running.lock().map_err(|e| e.to_string())? = Some(progress_path.clone());

    emit(
        &app,
        Progress::msg(Phase::Prepare, "Waiting for permission to write the card…"),
    );
    let result = run_helper(&app, &job_path, &progress_path).await;

    *state.running.lock().map_err(|e| e.to_string())? = None;
    let _ = std::fs::remove_file(&job_path); // normally already deleted by the helper
    let _ = std::fs::remove_dir_all(&dir);
    result
}

async fn run_helper(app: &AppHandle, job: &Path, progress: &Path) -> Result<(), String> {
    let mut child =
        elevate::spawn(job, progress).map_err(|e| format!("could not start the writer: {e}"))?;
    let mut offset = 0usize;
    let mut last: Option<Progress> = None;
    let forward = |last: &mut Option<Progress>, offset: &mut usize| {
        if let Ok(text) = std::fs::read_to_string(progress) {
            if text.len() > *offset {
                let new = &text[*offset..];
                // only complete lines
                if let Some(end) = new.rfind('\n') {
                    for line in new[..end].lines() {
                        if let Ok(p) = serde_json::from_str::<Progress>(line) {
                            emit(app, p.clone());
                            *last = Some(p);
                        }
                    }
                    *offset += end + 1;
                }
            }
        }
    };
    let status = loop {
        forward(&mut last, &mut offset);
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => tokio::time::sleep(Duration::from_millis(250)).await,
            Err(e) => return Err(e.to_string()),
        }
    };
    forward(&mut last, &mut offset);
    match last {
        Some(p) if p.phase == Phase::Done => Ok(()),
        Some(p) if p.phase == Phase::Error => {
            Err(p.message.unwrap_or_else(|| "writing failed".into()))
        }
        _ if !status.success() => Err(match status.code() {
            // pkexec: 126 = dialog dismissed, 127 = not authorised; osascript: 1 (user cancelled)
            Some(126) | Some(127) | Some(1) => {
                "Permission was not granted, so nothing was written.".into()
            }
            c => format!("the writer stopped unexpectedly (exit code {c:?})"),
        }),
        _ => Err("the writer finished without reporting success".into()),
    }
}

#[tauri::command]
fn cancel_write(state: State<'_, AppState>) -> Result<(), String> {
    state
        .download_cancel
        .store(true, std::sync::atomic::Ordering::Relaxed);
    if let Some(p) = state.running.lock().map_err(|e| e.to_string())?.as_ref() {
        std::fs::write(cancel_path(p), b"cancel").map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            defaults,
            releases,
            inspect_image,
            list_drives,
            validate_settings,
            write_card,
            cancel_write
        ])
        .run(tauri::generate_context!())
        .expect("error while running PixelPlus Imager");
}
