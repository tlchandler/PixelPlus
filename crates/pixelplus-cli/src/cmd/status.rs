//! `pixelplus status`: what the local daemon is doing.

use crate::http::{self, BaseUrl};
use crate::style::{self, paint, Level};
use anyhow::{bail, Context, Result};
use clap::Args;
use serde_json::Value;
use std::time::Duration;

/// Arguments of `status`.
#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Daemon URL [default: http://127.0.0.1:$PIXELPLUS_HTTP_PORT, port 80].
    #[arg(long)]
    pub url: Option<String>,
}

/// The daemon's base URL.
pub fn daemon_url(explicit: Option<&str>) -> String {
    match explicit {
        Some(u) => u.to_string(),
        None => {
            let port = std::env::var("PIXELPLUS_HTTP_PORT")
                .ok()
                .and_then(|p| p.trim().parse::<u16>().ok())
                .unwrap_or(80);
            format!("http://127.0.0.1:{port}")
        }
    }
}

/// GET a JSON document from the daemon's API.
pub fn fetch(base: &BaseUrl, path: &str) -> Result<Value> {
    let resp = http::get(base, path, Duration::from_secs(3))
        .with_context(|| format!("is pixelplusd running? (GET {path})"))?;
    match resp.status {
        200 => resp.json(),
        401 => bail!("the daemon has a password set; `status` needs an unauthenticated API (log in via the web UI)"),
        code => {
            let msg = resp
                .json()
                .ok()
                .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
                .unwrap_or_else(|| String::from_utf8_lossy(&resp.body).chars().take(200).collect());
            bail!("GET {path} returned HTTP {code}: {msg}")
        }
    }
}

fn s<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn f(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64)
}

/// `83000` → `1:23`.
pub fn clock(ms: f64) -> String {
    let total = (ms.max(0.0) / 1000.0).round() as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

/// `11520` → `3 h 12 min`.
pub fn uptime(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    match (s / 86_400, (s % 86_400) / 3600, (s % 3600) / 60) {
        (0, 0, m) => format!("{m} min"),
        (0, h, m) => format!("{h} h {m} min"),
        (d, h, _) => format!("{d} d {h} h"),
    }
}

/// Run `status`.
pub fn run(args: StatusArgs, json: bool) -> Result<()> {
    let base = BaseUrl::parse(&daemon_url(args.url.as_deref()))?;
    let system = fetch(&base, "/api/v1/system")?;
    let player = fetch(&base, "/api/v1/player")?;
    if json {
        let out = serde_json::json!({ "system": system, "player": player });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    let w = 11;
    let host = s(&system, "hostname").unwrap_or("?");
    let role = s(&system, "role").unwrap_or("?");
    anstream::println!(
        "{} {} {}",
        paint(
            style::HEADING,
            format!("pixelplusd {}", s(&system, "version").unwrap_or(""))
        ),
        host,
        paint(style::DIM, format!("({role})"))
    );
    if system.get("needsSetup").and_then(Value::as_bool) == Some(true) {
        anstream::println!(
            "  {} first-run setup not completed: open http://{host}.local/",
            Level::Warn.badge()
        );
    }
    if let Some(board) = s(&system, "board") {
        let rev = s(&system, "boardRev")
            .map(|r| format!(" rev {r}"))
            .unwrap_or_default();
        style::kv("Board", format!("{board}{rev}"), w);
    }
    if let Some(pi) = s(&system, "piModel") {
        style::kv("Pi", pi, w);
    }
    if let Some(up) = f(&system, "uptimeS") {
        style::kv("Uptime", uptime(up), w);
    }
    if let (Some(cpu), Some(mem)) = (f(&system, "cpuPct"), f(&system, "memPct")) {
        style::kv("CPU / mem", format!("{cpu:.0}% / {mem:.0}%"), w);
    }
    if let Some(t) = f(&system, "tempC") {
        style::kv("Temperature", format!("{t:.1} °C"), w);
    }
    if let Some(ips) = system.get("ips").and_then(Value::as_array) {
        let list: Vec<&str> = ips.iter().filter_map(Value::as_str).collect();
        if !list.is_empty() {
            style::kv("Addresses", list.join(", "), w);
        }
    }
    if let Some(ssid) = system.get("wifi").and_then(|wi| s(wi, "ssid")) {
        let signal = system["wifi"]
            .get("signal")
            .and_then(Value::as_f64)
            .map(|v| format!(" ({v:.0}%)"))
            .unwrap_or_default();
        style::kv("Wi-Fi", format!("{ssid}{signal}"), w);
    }

    anstream::println!();
    style::heading("Player");
    let state = s(&player, "state").unwrap_or("unknown");
    let badge = match state {
        "playing" => paint(style::OK, "▶ playing"),
        "paused" => paint(style::WARN, "❚❚ paused"),
        "testing" => paint(style::WARN, "testing"),
        "effect" => paint(style::OK, "effect"),
        other => paint(style::DIM, other),
    };
    let mut line = badge;
    if let Some(name) = player.get("item").and_then(|i| s(i, "name")) {
        line.push_str(&format!("  {name}"));
        if let (Some(pos), Some(dur)) = (f(&player, "posMs"), f(&player, "durationMs")) {
            line.push_str(&format!("  {} / {}", clock(pos), clock(dur)));
        }
    }
    style::kv("State", line, w);
    if let Some(pl) = player.get("playlist").filter(|p| p.is_object()) {
        let pos = match (f(pl, "index"), f(pl, "count")) {
            (Some(i), Some(c)) => format!(" ({}/{})", i as u64 + 1, c as u64),
            _ => String::new(),
        };
        style::kv(
            "Playlist",
            format!("{}{pos}", s(pl, "name").unwrap_or("?")),
            w,
        );
    }
    if let Some(next) = player.get("nextItem").and_then(|n| s(n, "name")) {
        style::kv("Up next", next, w);
    }
    let vol = f(&player, "volume").map(|v| format!("volume {v:.0}%"));
    let bri = f(&player, "brightness").map(|v| format!("brightness {v:.0}%"));
    let fps = f(&player, "fps").map(|v| format!("{v:.0} fps"));
    let levels: Vec<String> = [vol, bri, fps].into_iter().flatten().collect();
    if !levels.is_empty() {
        style::kv("Output", levels.join(" · "), w);
    }
    if let Some(entry) = player.get("scheduleEntry").and_then(|e| s(e, "name")) {
        let ends = player["scheduleEntry"]
            .get("endsAt")
            .and_then(Value::as_str)
            .map(|e| format!(" until {e}"))
            .unwrap_or_default();
        style::kv("Schedule", format!("{entry}{ends}"), w);
    }
    if let Some(show) = player.get("nextShow").and_then(|n| s(n, "name")) {
        let at = player["nextShow"]
            .get("startsAt")
            .and_then(Value::as_str)
            .unwrap_or("");
        style::kv("Next show", format!("{show} {at}"), w);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting() {
        assert_eq!(clock(83_000.0), "1:23");
        assert_eq!(clock(-5.0), "0:00");
        assert_eq!(uptime(59.0), "0 min");
        assert_eq!(uptime(11_520.0), "3 h 12 min");
        assert_eq!(uptime(200_000.0), "2 d 7 h");
        assert_eq!(daemon_url(Some("http://x:1")), "http://x:1");
    }
}
