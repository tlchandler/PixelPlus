//! Client for the games sidecar's control socket (JSON line request →
//! JSON line response, one per connection; see `games/README.md`), plus
//! ROM file management.

use crate::state::AppState;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(4);
/// Largest ROM accepted (NES ROMs are well under 1 MB).
pub const ROM_MAX: usize = 4 * 1024 * 1024;

/// Send one command; `Err` (friendly) when the service isn't running.
pub async fn command(state: &AppState, cmd: Value) -> Result<Value, String> {
    #[cfg(unix)]
    {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let path = state.config.games_socket.clone();
        let fut = async move {
            let mut stream = tokio::net::UnixStream::connect(&path).await.map_err(|_| ())?;
            let mut line = serde_json::to_vec(&cmd).map_err(|_| ())?;
            line.push(b'\n');
            stream.write_all(&line).await.map_err(|_| ())?;
            let mut reader = BufReader::new(stream);
            let mut resp = String::new();
            reader.read_line(&mut resp).await.map_err(|_| ())?;
            serde_json::from_str::<Value>(resp.trim()).map_err(|_| ())
        };
        match tokio::time::timeout(TIMEOUT, fut).await {
            Ok(Ok(v)) => Ok(v),
            _ => Err("The games service isn't running. Install it with games/install.sh, or check it in Settings → Logs.".into()),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (state, cmd);
        Err("Games are only available on Linux.".into())
    }
}

pub async fn available(state: &AppState) -> bool {
    state.config.games_socket.exists() && command(state, json!({"cmd": "status"})).await.is_ok()
}

pub fn roms_dir(state: &AppState) -> PathBuf {
    state.config.games_dir().join("roms")
}

/// `[{name, sizeBytes}]`, sorted by name.
pub fn list_roms(state: &AppState) -> Vec<Value> {
    let mut v: Vec<(String, u64)> = std::fs::read_dir(roms_dir(state))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            (name.to_ascii_lowercase().ends_with(".nes") && e.path().is_file())
                .then(|| (name, e.metadata().map(|m| m.len()).unwrap_or(0)))
        })
        .collect();
    v.sort();
    v.into_iter().map(|(name, size)| json!({ "name": name, "sizeBytes": size })).collect()
}

/// Safe ROM file name: keeps letters, digits, space, `-_.()`; must end in `.nes`.
pub fn sanitize_rom_name(name: &str) -> Option<String> {
    let base = std::path::Path::new(name).file_name()?.to_str()?;
    let clean: String = base
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || " -_.()".contains(*c))
        .collect();
    let clean = clean.trim().trim_start_matches('.').to_string();
    (clean.to_ascii_lowercase().ends_with(".nes") && clean.len() > 4 && clean.len() <= 100).then_some(clean)
}

/// iNES / NES 2.0 header check.
pub fn looks_like_nes(bytes: &[u8]) -> bool {
    bytes.len() > 16 && bytes.starts_with(b"NES\x1a")
}

/// `GET /games/status`: the sidecar's status, or a synthesized one.
pub async fn status(state: &AppState) -> Value {
    let settings = state.store.get().settings.games.clone();
    let roms = list_roms(state);
    match command(state, json!({"cmd": "status"})).await {
        Ok(mut v) => {
            if let Some(o) = v.as_object_mut() {
                o.insert("available".into(), json!(true));
                o.insert("roms".into(), json!(roms));
                o.entry("enabled").or_insert(json!(settings.enabled));
                o.entry("arcade").or_insert(json!(settings.arcade_mode));
            }
            v
        }
        Err(e) => json!({
            "enabled": settings.enabled,
            "running": false,
            "arcade": settings.arcade_mode,
            "queueLength": 0,
            "cooldownS": 0,
            "available": false,
            "lastError": if settings.enabled { Some(e) } else { None },
            "roms": roms,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rom_names() {
        assert_eq!(sanitize_rom_name("Super Mario Bros (W).nes").as_deref(), Some("Super Mario Bros (W).nes"));
        assert_eq!(sanitize_rom_name("../../etc/smb.NES").as_deref(), Some("smb.NES"));
        assert_eq!(sanitize_rom_name("evil.sh"), None);
        assert_eq!(sanitize_rom_name(".nes"), None);
        assert!(looks_like_nes(b"NES\x1a0123456789abcdef"));
        assert!(!looks_like_nes(b"PK\x03\x04"));
    }
}
