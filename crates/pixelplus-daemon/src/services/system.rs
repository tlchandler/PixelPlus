//! Host information and host control: board detection, CPU/memory/disk,
//! addresses, Wi-Fi status, time zone, reboot/shutdown, and small helpers for
//! running system tools (`nmcli`, `systemctl`, `timedatectl`, ...).
//!
//! Everything here degrades gracefully on a development machine or in Docker:
//! missing tools or files simply leave fields empty.

use crate::api::{ApiError, ApiResult};
use crate::node::LocalRole;
use crate::state::AppState;
use parking_lot::Mutex;
use pixelplus_core::model::BoardKind;
use pixelplus_hw::BoardDetection;
use serde::Serialize;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Running tools
// ---------------------------------------------------------------------------

/// Output of a finished command.
#[derive(Debug, Clone)]
pub struct CmdOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Run `program args...` with a timeout. `Err` when the program is missing,
/// could not start, or timed out (message is user-presentable).
pub async fn run(program: &str, args: &[&str], timeout: Duration) -> Result<CmdOutput, String> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .env("LC_ALL", "C");
    let child = cmd.output();
    match tokio::time::timeout(timeout, child).await {
        Err(_) => Err(format!("{program} did not answer within {} s", timeout.as_secs())),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => Err(format!("{program} is not installed")),
        Ok(Err(e)) => Err(format!("could not run {program}: {e}")),
        Ok(Ok(out)) => Ok(CmdOutput {
            success: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }),
    }
}

/// Whether `program` is found on `$PATH`.
pub fn have(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|d| d.join(program).is_file())
}

pub fn is_root() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no preconditions.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Running inside a container (Docker / Podman).
pub fn in_docker() -> bool {
    std::env::var_os("PIXELPLUS_DOCKER").is_some()
        || Path::new("/.dockerenv").exists()
        || Path::new("/run/.containerenv").exists()
}

/// systemd is PID 1.
pub fn has_systemd() -> bool {
    Path::new("/run/systemd/system").is_dir()
}

/// Running as a systemd service (journald has our logs).
pub fn under_systemd_service() -> bool {
    std::env::var_os("INVOCATION_ID").is_some() && has_systemd()
}

// ---------------------------------------------------------------------------
// Board detection
// ---------------------------------------------------------------------------

static DETECTION: Mutex<Option<(BoardDetection, Option<pixelplus_hw::PiInfo>)>> = Mutex::new(None);

/// Probe the board (EEPROM + I2C) and the Pi model. Cached; call
/// [`redetect_board`] after writing the EEPROM.
pub fn detection() -> (BoardDetection, Option<pixelplus_hw::PiInfo>) {
    if let Some(d) = DETECTION.lock().clone() {
        return d;
    }
    let d = probe_board();
    *DETECTION.lock() = Some(d.clone());
    d
}

pub fn redetect_board() -> (BoardDetection, Option<pixelplus_hw::PiInfo>) {
    let d = probe_board();
    *DETECTION.lock() = Some(d.clone());
    d
}

fn probe_board() -> (BoardDetection, Option<pixelplus_hw::PiInfo>) {
    let pi = pixelplus_hw::board::read_pi_info();
    #[cfg(target_os = "linux")]
    {
        if pi.is_some() {
            if let Ok(mut bus) = pixelplus_hw::LinuxI2c::open(pixelplus_hw::i2c::DEFAULT_BUS) {
                let mut eeprom = pixelplus_hw::eeprom::SysfsEeprom::open(1, pixelplus_hw::eeprom::EEPROM_ADDR).ok();
                let det = pixelplus_hw::board::detect(
                    &mut bus,
                    eeprom.as_mut().map(|e| e as &mut dyn pixelplus_hw::EepromStore),
                );
                return (det, pi);
            }
        }
    }
    (pixelplus_hw::board::classify(None, &[]), pi)
}

/// The board this node runs: wizard override, else EEPROM, else a Pi without
/// a board, else virtual (PC / Docker).
pub fn effective_board(state: &AppState) -> (BoardKind, Option<String>) {
    let id = state.identity();
    if let Some(b) = id.board {
        return (b, id.board_rev.clone());
    }
    let (det, pi) = detection();
    match det.board {
        Some(b) => (b, det.rev.clone()),
        None if pi.is_some() => (BoardKind::BarePi, None),
        None => (BoardKind::Virtual, None),
    }
}

// ---------------------------------------------------------------------------
// Host metrics
// ---------------------------------------------------------------------------

struct CpuSample {
    at: Instant,
    total: u64,
    idle: u64,
    pct: f32,
}

static CPU: Mutex<Option<CpuSample>> = Mutex::new(None);

fn read_cpu_times() -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let line = stat.lines().next()?;
    let nums: Vec<u64> = line.split_whitespace().skip(1).filter_map(|v| v.parse().ok()).collect();
    if nums.len() < 4 {
        return None;
    }
    let idle = nums[3] + nums.get(4).copied().unwrap_or(0);
    Some((nums.iter().sum(), idle))
}

/// CPU usage (%) since the previous call (sampled over ≥150 ms the first time).
pub async fn cpu_pct() -> Option<f32> {
    let (total, idle) = read_cpu_times()?;
    let prev = CPU.lock().as_ref().map(|s| (s.at, s.total, s.idle, s.pct));
    let (t0, i0) = match prev {
        Some((at, _, _, pct)) if at.elapsed() < Duration::from_millis(500) => return Some(pct),
        Some((_, t, i, _)) => (t, i),
        None => {
            tokio::time::sleep(Duration::from_millis(150)).await;
            let (t1, i1) = read_cpu_times()?;
            let pct = pct_of(total, idle, t1, i1);
            *CPU.lock() = Some(CpuSample { at: Instant::now(), total: t1, idle: i1, pct });
            return Some(pct);
        }
    };
    let pct = pct_of(t0, i0, total, idle);
    *CPU.lock() = Some(CpuSample { at: Instant::now(), total, idle, pct });
    Some(pct)
}

fn pct_of(t0: u64, i0: u64, t1: u64, i1: u64) -> f32 {
    let dt = t1.saturating_sub(t0);
    if dt == 0 {
        return 0.0;
    }
    let busy = dt.saturating_sub(i1.saturating_sub(i0));
    ((busy as f64 / dt as f64) * 1000.0).round() as f32 / 10.0
}

/// Memory in use (%), from `MemTotal` and `MemAvailable`.
pub fn mem_pct() -> Option<f32> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    parse_mem_pct(&text)
}

fn parse_mem_pct(text: &str) -> Option<f32> {
    let field = |name: &str| -> Option<f64> {
        text.lines()
            .find(|l| l.starts_with(name))?
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()
    };
    let total = field("MemTotal:")?;
    let avail = field("MemAvailable:")?;
    if total <= 0.0 {
        return None;
    }
    Some((((total - avail) / total) * 1000.0).round() as f32 / 10.0)
}

/// Free (available to us) and total space of the filesystem holding `path`, in bytes.
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: `c` is a valid NUL-terminated path, `s` a valid out pointer.
        if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
            return None;
        }
        let frsize = s.f_frsize as u64;
        Some((s.f_bavail as u64 * frsize, s.f_blocks as u64 * frsize))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// SoC temperature in °C.
pub fn soc_temp() -> Option<f32> {
    let v: f32 = std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp")
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some((v / 100.0).round() / 10.0)
}

/// Seconds since boot.
pub fn uptime_s(state: &AppState) -> u64 {
    std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())
        .map(|v| v as u64)
        .unwrap_or_else(|| state.started.elapsed().as_secs())
}

pub fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .or_else(|_| std::fs::read_to_string("/etc/hostname"))
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "pixelplus".into())
}

/// Non-loopback IPv4 addresses (then global IPv6), interface order.
pub fn ip_addresses() -> Vec<String> {
    let mut v4 = Vec::new();
    let mut v6 = Vec::new();
    #[cfg(unix)]
    unsafe {
        // SAFETY: standard getifaddrs/freeifaddrs usage; we only read the list.
        let mut addrs: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut addrs) == 0 {
            let mut cur = addrs;
            while !cur.is_null() {
                let ifa = &*cur;
                if !ifa.ifa_addr.is_null() {
                    let family = (*ifa.ifa_addr).sa_family as i32;
                    if family == libc::AF_INET {
                        let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                        let ip = std::net::Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                        if !ip.is_loopback() && !ip.is_link_local() {
                            v4.push(ip.to_string());
                        }
                    } else if family == libc::AF_INET6 {
                        let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in6);
                        let ip = std::net::Ipv6Addr::from(sin.sin6_addr.s6_addr);
                        let seg0 = ip.segments()[0];
                        let global = !ip.is_loopback() && (seg0 & 0xffc0) != 0xfe80 && (seg0 & 0xfe00) != 0xfc00;
                        if global {
                            v6.push(ip.to_string());
                        }
                    }
                }
                cur = ifa.ifa_next;
            }
            libc::freeifaddrs(addrs);
        }
    }
    v4.dedup();
    v6.dedup();
    v4.extend(v6);
    v4
}

/// The host's IANA time zone.
pub fn system_timezone() -> Option<String> {
    if let Ok(tz) = std::env::var("TZ") {
        let tz = tz.trim_start_matches(':').to_string();
        if tz.parse::<chrono_tz::Tz>().is_ok() {
            return Some(tz);
        }
    }
    if let Ok(target) = std::fs::read_link("/etc/localtime") {
        let s = target.to_string_lossy();
        if let Some(idx) = s.find("zoneinfo/") {
            return Some(s[idx + "zoneinfo/".len()..].to_string());
        }
    }
    std::fs::read_to_string("/etc/timezone")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Set the host time zone (root on Linux only; failures are logged, not fatal).
pub async fn set_system_timezone(tz: &str) {
    if tz.parse::<chrono_tz::Tz>().is_err() || !is_root() || in_docker() || !have("timedatectl") {
        return;
    }
    match run("timedatectl", &["set-timezone", tz], Duration::from_secs(10)).await {
        Ok(o) if o.success => tracing::info!("System time zone set to {tz}"),
        Ok(o) => tracing::warn!("Could not set the system time zone to {tz}: {}", o.stderr.trim()),
        Err(e) => tracing::warn!("Could not set the system time zone to {tz}: {e}"),
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WifiStatus {
    pub ssid: String,
    /// Approximate RSSI in dBm (converted from NetworkManager's 0–100 %).
    pub signal: i32,
    /// NetworkManager signal quality, 0–100 %.
    pub quality: u8,
}

/// Convert NetworkManager's 0–100 % quality to an approximate dBm value.
pub fn quality_to_dbm(q: u8) -> i32 {
    i32::from(q.min(100)) / 2 - 100
}

static WIFI: Mutex<Option<(Instant, Option<WifiStatus>)>> = Mutex::new(None);

/// Active Wi-Fi network (cached 15 s; `None` on Ethernet or without nmcli).
pub async fn wifi_status() -> Option<WifiStatus> {
    if let Some((at, v)) = WIFI.lock().clone() {
        if at.elapsed() < Duration::from_secs(15) {
            return v;
        }
    }
    let v = if have("nmcli") {
        run("nmcli", &["-t", "-f", "ACTIVE,SSID,SIGNAL", "dev", "wifi"], Duration::from_secs(5))
            .await
            .ok()
            .filter(|o| o.success)
            .and_then(|o| parse_active_wifi(&o.stdout))
    } else {
        None
    };
    *WIFI.lock() = Some((Instant::now(), v.clone()));
    v
}

/// Split one `nmcli -t` line into fields (handles `\:` escapes).
pub fn nmcli_fields(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(n) = chars.next() {
                    out.last_mut().unwrap().push(n);
                }
            }
            ':' => out.push(String::new()),
            c => out.last_mut().unwrap().push(c),
        }
    }
    out
}

fn parse_active_wifi(text: &str) -> Option<WifiStatus> {
    text.lines().find_map(|l| {
        let f = nmcli_fields(l);
        if f.len() >= 3 && f[0] == "yes" && !f[1].is_empty() {
            let q: u8 = f[2].parse().unwrap_or(0);
            Some(WifiStatus { ssid: f[1].clone(), signal: quality_to_dbm(q), quality: q })
        } else {
            None
        }
    })
}

// ---------------------------------------------------------------------------
// SystemInfo
// ---------------------------------------------------------------------------

/// Body of `GET /system`.
pub async fn system_info(state: &AppState, authed: bool) -> serde_json::Value {
    let id = state.identity();
    let show = state.store.get();
    let (det, pi) = detection();
    let (board, board_rev) = effective_board(state);
    let role = match id.role {
        LocalRole::Unconfigured => "unconfigured",
        LocalRole::Leader => "leader",
        LocalRole::Follower => "follower",
    };
    let timezone = system_timezone().unwrap_or_else(|| show.schedule.location.timezone.clone());
    let mut info = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "nodeId": id.id,
        "name": id.name.clone().unwrap_or_else(hostname),
        "role": role,
        "hostname": hostname(),
        "board": board,
        "boardRev": board_rev,
        "boardName": board.display_name(),
        "detectedBoard": det.board,
        "suggestedBoard": det.suggested,
        "eeprom": det.eeprom.as_ref().map(|e| e.state_name()),
        "boardWarnings": det.warnings,
        "piModel": pi.as_ref().map(|p| p.model.clone()),
        "needsSetup": id.role == LocalRole::Unconfigured,
        "passwordSet": show.settings.security.password_hash.is_some(),
        "time": chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        "timezone": timezone,
        "showName": show.name,
        "docker": in_docker(),
    });
    if !authed {
        return info;
    }
    let (free, _) = disk_space(&state.config.data_dir).unwrap_or((0, 0));
    let leader_name = id.leader_url.clone().map(|u| u.trim_start_matches("http://").to_string());
    let extra = serde_json::json!({
        "uptimeS": uptime_s(state),
        "cpuPct": cpu_pct().await.unwrap_or(0.0),
        "memPct": mem_pct().unwrap_or(0.0),
        "diskFreeMb": free / (1024 * 1024),
        "tempC": soc_temp(),
        "ips": ip_addresses(),
        "wifi": wifi_status().await,
        "leaderName": leader_name,
        "ttsAvailable": crate::services::tts::device_available(state).await,
        "gamesAvailable": crate::services::games::available(state).await,
        "outputs": board.output_count(),
    });
    if let (serde_json::Value::Object(a), serde_json::Value::Object(b)) = (&mut info, extra) {
        a.extend(b);
    }
    info
}

// ---------------------------------------------------------------------------
// Power actions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    Reboot,
    Shutdown,
    RestartService,
}

/// Check that we may reboot/shut down/restart, then do it after a short delay
/// (so the HTTP response reaches the browser first).
pub fn power_action(action: PowerAction) -> ApiResult<&'static str> {
    if in_docker() {
        return Err(ApiError::forbidden(match action {
            PowerAction::RestartService => {
                "PixelPlus is running in Docker. Restart the container from your Docker or NAS dashboard."
            }
            _ => "PixelPlus is running in Docker, so it can't restart or turn off the computer. Use your Docker or NAS dashboard.",
        }));
    }
    if !cfg!(target_os = "linux") || !has_systemd() || !have("systemctl") {
        return Err(ApiError::forbidden(
            "This computer can't be restarted from PixelPlus (it isn't a PixelPlus Pi). Restart it yourself.",
        ));
    }
    if !is_root() {
        return Err(ApiError::forbidden(
            "PixelPlus isn't running as a system service, so it isn't allowed to do that. Restart it yourself.",
        ));
    }
    let (args, msg): (&[&str], &str) = match action {
        PowerAction::Reboot => (&["reboot"], "Restarting. PixelPlus will be back in about a minute."),
        PowerAction::Shutdown => (&["poweroff"], "Shutting down. Wait for the green light to stop blinking before unplugging."),
        PowerAction::RestartService => (&["restart", "pixelplusd"], "Restarting PixelPlus. This takes a few seconds."),
    };
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(800)).await;
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        match run("systemctl", &refs, Duration::from_secs(30)).await {
            Ok(o) if o.success => {}
            Ok(o) => tracing::error!("systemctl {} failed: {}", refs.join(" "), o.stderr.trim()),
            Err(e) => tracing::error!("systemctl {} failed: {e}", refs.join(" ")),
        }
    });
    Ok(msg)
}

/// Recent log lines as text: journald when running as a service, else the
/// in-memory ring buffer.
pub async fn logs_text(lines: usize) -> String {
    let lines = lines.clamp(1, 5000);
    if under_systemd_service() && have("journalctl") {
        let n = lines.to_string();
        if let Ok(o) = run(
            "journalctl",
            &["-u", "pixelplusd", "-n", &n, "--no-pager", "-o", "short-iso"],
            Duration::from_secs(10),
        )
        .await
        {
            if o.success && !o.stdout.trim().is_empty() && !o.stdout.contains("No entries") {
                return o.stdout;
            }
        }
    }
    let recent = crate::services::logs::ring().recent(lines);
    if recent.is_empty() {
        return "No log messages yet.\n".into();
    }
    crate::services::logs::format_lines(&recent)
}

/// Audio output devices (`aplay -L`), always starting with the system default.
pub async fn audio_devices() -> Vec<serde_json::Value> {
    let mut out = vec![serde_json::json!({"id": "default", "name": "System default"})];
    if let Ok(o) = run("aplay", &["-L"], Duration::from_secs(5)).await {
        if o.success {
            for (id, name) in parse_aplay_l(&o.stdout) {
                out.push(serde_json::json!({ "id": id, "name": name }));
            }
        }
    }
    out
}

/// Parse `aplay -L`: keep hardware-ish PCMs (`hw:`/`plughw:`/`sysdefault:` of each card).
pub fn parse_aplay_l(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if line.starts_with(char::is_whitespace) || line.trim().is_empty() {
            continue;
        }
        let id = line.trim().to_string();
        let desc = lines
            .peek()
            .filter(|l| l.starts_with(char::is_whitespace))
            .map(|l| l.trim().to_string())
            .unwrap_or_default();
        if !(id.starts_with("plughw:") || id.starts_with("hw:")) {
            continue;
        }
        // Only the first device of each card is interesting for most users; keep all though.
        let card_label = desc.split(',').next().unwrap_or(&desc).trim().to_string();
        let name = friendly_audio_name(&id, &card_label);
        if !out.iter().any(|(i, _)| i == &id) {
            out.push((id, name));
        }
    }
    // Prefer plughw (format conversion) over hw when both exist for the same card.
    let plug: Vec<String> = out
        .iter()
        .filter(|(i, _)| i.starts_with("plughw:"))
        .map(|(i, _)| i.trim_start_matches("plughw:").to_string())
        .collect();
    out.retain(|(i, _)| !(i.starts_with("hw:") && plug.contains(&i.trim_start_matches("hw:").to_string())));
    out
}

fn friendly_audio_name(id: &str, label: &str) -> String {
    let lower = id.to_ascii_lowercase();
    if lower.contains("headphones") {
        "Headphone jack (line out)".into()
    } else if lower.contains("hdmi") {
        let n = if lower.contains("hdmi1") { "2" } else { "1" };
        format!("HDMI {n}")
    } else if label.is_empty() {
        id.to_string()
    } else {
        label.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo() {
        let t = "MemTotal:        1000 kB\nMemFree: 100 kB\nMemAvailable:     250 kB\n";
        assert_eq!(parse_mem_pct(t), Some(75.0));
    }

    #[test]
    fn nmcli_escapes() {
        assert_eq!(nmcli_fields(r"yes:My\:Net:77"), vec!["yes", "My:Net", "77"]);
        let w = parse_active_wifi("no:Other:40\nyes:Chandler-Home:92\n").unwrap();
        assert_eq!(w.ssid, "Chandler-Home");
        assert_eq!(w.quality, 92);
        assert_eq!(w.signal, -54);
    }

    #[test]
    fn aplay_parsing() {
        let t = "null\n    Discard all samples\nhw:CARD=Headphones,DEV=0\n    bcm2835 Headphones, bcm2835 Headphones\n    Direct hardware device\nplughw:CARD=Headphones,DEV=0\n    bcm2835 Headphones, bcm2835 Headphones\nhw:CARD=Device,DEV=0\n    USB Audio Device, USB Audio\n";
        let d = parse_aplay_l(t);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].0, "plughw:CARD=Headphones,DEV=0");
        assert_eq!(d[0].1, "Headphone jack (line out)");
        assert_eq!(d[1].1, "USB Audio Device");
    }

    #[test]
    fn cpu_pct_math() {
        assert_eq!(pct_of(0, 0, 100, 25), 75.0);
        assert_eq!(pct_of(5, 5, 5, 5), 0.0);
    }
}
