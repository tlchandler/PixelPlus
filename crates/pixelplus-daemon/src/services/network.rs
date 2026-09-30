//! Network settings through NetworkManager (`nmcli`) and hostnamed
//! (`hostnamectl`), both allowed for the `pixelplus` service user by polkit;
//! the Wi-Fi country (regulatory domain) is set by the root helper.
//!
//! Changes are validated first, then applied in the background so the HTTP
//! response reaches the browser before Wi-Fi drops. Progress and failures
//! are reported as toasts.
//!
//! The setup-hotspot watchdog (image/netwatch, root) publishes its state in
//! `/run/pixelplus/netwatch.json`; it is returned as `netwatch`.

use super::platform::{self, HelperOpts, HelperState, HelperVerb};
use super::system::{have, hostname, in_docker, nmcli_fields, quality_to_dbm, run};
use crate::api::{ApiError, ApiResult};
use crate::events::ToastKind;
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct WifiConfig {
    #[serde(default)]
    pub ssid: String,
    /// Write-only: never returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub psk: Option<String>,
    #[serde(default)]
    pub country: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EthernetConfig {
    #[serde(default = "yes")]
    pub dhcp: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns: Option<String>,
}

impl Default for EthernetConfig {
    fn default() -> Self {
        EthernetConfig {
            dhcp: true,
            address: None,
            gateway: None,
            dns: None,
        }
    }
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct NetworkConfig {
    pub hostname: String,
    #[serde(default)]
    pub wifi: WifiConfig,
    #[serde(default)]
    pub ethernet: EthernetConfig,
    /// Read-only: false when this machine's network can't be managed (Docker, PC).
    #[serde(default = "yes")]
    pub managed: bool,
    /// Read-only: setup-hotspot watchdog status (None when netwatch isn't running here).
    #[serde(default, skip_deserializing)]
    pub netwatch: Option<NetwatchStatus>,
}

/// Last Wi-Fi network netwatch joined from the setup portal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct NetwatchJoined {
    #[serde(default)]
    pub ssid: String,
    #[serde(default)]
    pub ips: Vec<String>,
    /// Unix seconds.
    #[serde(default)]
    pub at: Option<i64>,
}

/// `/run/pixelplus/netwatch.json`, written by image/netwatch/netwatch.py.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct NetwatchStatus {
    /// waiting | online | hotspot | connecting
    pub state: String,
    #[serde(default)]
    pub hotspot_ssid: Option<String>,
    #[serde(default)]
    pub hotspot_secured: bool,
    /// The hotspot's current Wi-Fi password (signed-in owner only; the file is
    /// readable by root and the pixelplus group).
    #[serde(default)]
    pub hotspot_password: Option<String>,
    #[serde(default)]
    pub portal_url: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub last_joined: Option<NetwatchJoined>,
    /// Unix seconds.
    #[serde(default)]
    pub updated_at: Option<i64>,
}

/// Path of the netwatch status file (`PIXELPLUS_NETWATCH_STATUS` overrides, as in netwatch.py).
pub fn netwatch_path() -> std::path::PathBuf {
    std::env::var_os("PIXELPLUS_NETWATCH_STATUS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| platform::run_dir().join("netwatch.json"))
}

/// Read the netwatch status (None when missing or unreadable).
pub fn read_netwatch(path: &Path) -> Option<NetwatchStatus> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut st: NetwatchStatus = serde_json::from_str(&text).ok()?;
    st.state = st.state.to_ascii_lowercase();
    Some(st)
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WifiNetwork {
    pub ssid: String,
    /// Approximate dBm.
    pub signal: i32,
    /// 0–100 %.
    pub quality: u8,
    pub secure: bool,
}

pub fn managed() -> bool {
    cfg!(target_os = "linux") && !in_docker() && have("nmcli")
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

pub fn validate_hostname(h: &str) -> ApiResult<()> {
    let ok = !h.is_empty()
        && h.len() <= 63
        && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !h.starts_with('-')
        && !h.ends_with('-');
    if ok {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "A controller name can only use letters, numbers and dashes (up to 63), and can't start or end with a dash.",
        ))
    }
}

fn parse_cidr(s: &str) -> Option<(Ipv4Addr, u8)> {
    let s = s.trim();
    let (ip, prefix) = match s.split_once('/') {
        Some((ip, p)) => (ip, p.parse::<u8>().ok()?),
        None => (s, 24),
    };
    let ip: Ipv4Addr = ip.trim().parse().ok()?;
    (1..=32).contains(&prefix).then_some((ip, prefix))
}

fn split_dns(s: &str) -> Vec<&str> {
    s.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|x| !x.is_empty())
        .collect()
}

pub fn validate(cfg: &NetworkConfig) -> ApiResult<()> {
    validate_hostname(&cfg.hostname)?;
    let w = &cfg.wifi;
    if w.ssid.len() > 32 {
        return Err(ApiError::bad_request(
            "A Wi-Fi network name can be at most 32 characters.",
        ));
    }
    if let Some(psk) = w.psk.as_deref().filter(|p| !p.is_empty()) {
        let hex64 = psk.len() == 64 && psk.chars().all(|c| c.is_ascii_hexdigit());
        if !(8..=63).contains(&psk.chars().count()) && !hex64 {
            return Err(ApiError::bad_request(
                "A Wi-Fi password needs 8 to 63 characters.",
            ));
        }
    }
    let country_ok = w.country.is_empty()
        || (w.country.len() == 2 && w.country.chars().all(|c| c.is_ascii_alphabetic()));
    if !country_ok {
        return Err(ApiError::bad_request(
            "Pick your Wi-Fi country (a two-letter code like US or GB).",
        ));
    }
    let e = &cfg.ethernet;
    if !e.dhcp {
        let addr = e.address.as_deref().unwrap_or("");
        let Some((ip, prefix)) = parse_cidr(addr) else {
            return Err(ApiError::bad_request(
                "Enter the wired address like 192.168.1.50 (or 192.168.1.50/24).",
            ));
        };
        if let Some(gw) = e.gateway.as_deref().filter(|g| !g.trim().is_empty()) {
            let gw: Ipv4Addr = gw.trim().parse().map_err(|_| {
                ApiError::bad_request("The router (gateway) address should look like 192.168.1.1.")
            })?;
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - prefix)
            };
            if u32::from(gw) & mask != u32::from(ip) & mask {
                return Err(ApiError::bad_request(format!(
                    "The router address {gw} isn't on the same network as {ip}/{prefix}."
                )));
            }
        }
        if let Some(dns) = e.dns.as_deref() {
            for d in split_dns(dns) {
                if d.parse::<std::net::IpAddr>().is_err() {
                    return Err(ApiError::bad_request(format!(
                        "\"{d}\" isn't a valid DNS server address."
                    )));
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

async fn nm(args: &[&str]) -> Result<String, String> {
    let o = run("nmcli", args, Duration::from_secs(15)).await?;
    if o.success {
        Ok(o.stdout)
    } else {
        Err(o.stderr.trim().trim_start_matches("Error: ").to_string())
    }
}

/// `nmcli` with `input` on its stdin (secrets).
async fn nm_stdin(args: &[&str], input: Option<&str>) -> Result<String, String> {
    use tokio::io::AsyncWriteExt;
    let Some(input) = input else {
        return nm(args).await;
    };
    let mut child = tokio::process::Command::new("nmcli")
        .args(args)
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("could not run nmcli: {e}"))?;
    if let Some(mut w) = child.stdin.take() {
        let _ = w.write_all(input.as_bytes()).await;
    }
    let out = tokio::time::timeout(Duration::from_secs(60), child.wait_with_output())
        .await
        .map_err(|_| "nmcli did not answer within 60 s".to_string())?
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr)
            .trim()
            .trim_start_matches("Error: ")
            .to_string())
    }
}

/// `nmcli` arguments to join `ssid`; the password (if any) is read from stdin.
pub fn wifi_connect_args(ssid: &str, with_password: bool) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    if with_password {
        a.push("--ask".into());
    }
    a.extend(
        [
            "--wait",
            "45",
            "dev",
            "wifi",
            "connect",
            ssid,
            "name",
            "pixelplus-wifi",
        ]
        .map(String::from),
    );
    a
}

/// (name, type, device) of connections, active ones first.
async fn connections() -> Vec<(String, String, String, bool)> {
    let Ok(out) = nm(&["-t", "-f", "NAME,TYPE,DEVICE,ACTIVE", "con", "show"]).await else {
        return vec![];
    };
    let mut v: Vec<_> = out
        .lines()
        .map(nmcli_fields)
        .filter(|f| f.len() >= 4)
        .map(|f| (f[0].clone(), f[1].clone(), f[2].clone(), f[3] == "yes"))
        .collect();
    v.sort_by_key(|c| !c.3);
    v
}

async fn wifi_country() -> String {
    if let Ok(o) = run("iw", &["reg", "get"], Duration::from_secs(5)).await {
        for l in o.stdout.lines() {
            if let Some(rest) = l.trim().strip_prefix("country ") {
                let c = rest.split(':').next().unwrap_or("").trim();
                if c.len() == 2 && c != "00" {
                    return c.to_string();
                }
            }
        }
    }
    String::new()
}

pub async fn read_config() -> NetworkConfig {
    let mut cfg = NetworkConfig {
        hostname: hostname(),
        managed: managed(),
        netwatch: read_netwatch(&netwatch_path()),
        ..Default::default()
    };
    if !cfg.managed {
        return cfg;
    }
    let conns = connections().await;
    if let Some((name, _, _, _)) = conns.iter().find(|c| c.1 == "802-11-wireless") {
        if let Ok(out) = nm(&["-t", "-f", "802-11-wireless.ssid", "con", "show", name]).await {
            cfg.wifi.ssid = out
                .lines()
                .next()
                .and_then(|l| l.split_once(':'))
                .map(|(_, v)| v.to_string())
                .unwrap_or_default();
        }
    }
    cfg.wifi.country = wifi_country().await;
    if let Some((name, _, _, _)) = conns.iter().find(|c| c.1 == "802-3-ethernet") {
        if let Ok(out) = nm(&[
            "-t",
            "-f",
            "ipv4.method,ipv4.addresses,ipv4.gateway,ipv4.dns",
            "con",
            "show",
            name,
        ])
        .await
        {
            for l in out.lines() {
                let Some((k, v)) = l.split_once(':') else {
                    continue;
                };
                let v = v.trim();
                match k {
                    "ipv4.method" => cfg.ethernet.dhcp = v != "manual",
                    "ipv4.addresses" if !v.is_empty() => cfg.ethernet.address = Some(v.to_string()),
                    "ipv4.gateway" if !v.is_empty() && v != "--" => {
                        cfg.ethernet.gateway = Some(v.to_string())
                    }
                    "ipv4.dns" if !v.is_empty() => cfg.ethernet.dns = Some(v.to_string()),
                    _ => {}
                }
            }
        }
    }
    cfg
}

pub fn parse_scan(text: &str) -> Vec<WifiNetwork> {
    let mut nets: Vec<WifiNetwork> = Vec::new();
    for line in text.lines() {
        let f = nmcli_fields(line);
        if f.len() < 3 || f[0].trim().is_empty() {
            continue;
        }
        let quality: u8 = f[1].parse().unwrap_or(0);
        let secure = !f[2].trim().is_empty() && f[2].trim() != "--";
        let n = WifiNetwork {
            ssid: f[0].clone(),
            signal: quality_to_dbm(quality),
            quality,
            secure,
        };
        match nets.iter_mut().find(|x| x.ssid == n.ssid) {
            Some(x) if x.quality < n.quality => *x = n,
            Some(_) => {}
            None => nets.push(n),
        }
    }
    nets.sort_by(|a, b| b.quality.cmp(&a.quality));
    nets
}

pub async fn scan() -> ApiResult<Vec<WifiNetwork>> {
    if !managed() {
        return Err(ApiError::unavailable(
            "Wi-Fi scanning only works on a PixelPlus Pi (NetworkManager was not found).",
        ));
    }
    let out = nm(&[
        "-t",
        "-f",
        "SSID,SIGNAL,SECURITY",
        "dev",
        "wifi",
        "list",
        "--rescan",
        "yes",
    ])
    .await
    .map_err(|e| ApiError::unavailable(format!("Couldn't scan for Wi-Fi networks: {e}")))?;
    Ok(parse_scan(&out))
}

// ---------------------------------------------------------------------------
// Applying
// ---------------------------------------------------------------------------

/// Validate, then apply in the background. Returns the config as it will be.
pub async fn apply(new: NetworkConfig, state: AppState) -> ApiResult<NetworkConfig> {
    let events = state.events.clone();
    validate(&new)?;
    if !managed() {
        return Err(ApiError::unavailable(
            "Network settings can only be changed on a PixelPlus Pi. On a PC or in Docker, use the computer's own network settings.",
        ));
    }
    let current = read_config().await;
    let mut result = new.clone();
    result.wifi.psk = None;
    result.managed = true;
    result.netwatch = current.netwatch.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(700)).await;
        let mut problems: Vec<String> = Vec::new();
        if new.hostname != current.hostname {
            if let Err(e) = platform::set_hostname(&state, &new.hostname).await {
                problems.push(format!("the name couldn't be changed ({e})"));
            }
        }
        if !new.wifi.country.is_empty()
            && !new.wifi.country.eq_ignore_ascii_case(&current.wifi.country)
        {
            let cc = new.wifi.country.to_ascii_uppercase();
            let res = match platform::run_helper(
                &state,
                HelperVerb::WifiCountry(cc.clone()),
                HelperOpts { quiet: true },
            )
            .await
            {
                Ok(job) => {
                    let s = job.wait(Duration::from_secs(60)).await;
                    (s.state == HelperState::Ok).then_some(()).ok_or(s.message)
                }
                Err(e) => Err(e.message),
            };
            if let Err(e) = res {
                problems.push(format!("the Wi-Fi country couldn't be set to {cc} ({e})"));
            }
        }
        // Ethernet before Wi-Fi: Wi-Fi changes may cut us off.
        if new.ethernet != current.ethernet {
            let conns = connections().await;
            if let Some((name, _, _, _)) = conns.iter().find(|c| c.1 == "802-3-ethernet") {
                let e = &new.ethernet;
                let args: Vec<String> = if e.dhcp {
                    [
                        "con",
                        "mod",
                        name,
                        "ipv4.method",
                        "auto",
                        "ipv4.addresses",
                        "",
                        "ipv4.gateway",
                        "",
                        "ipv4.dns",
                        "",
                    ]
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
                } else {
                    let (ip, prefix) = parse_cidr(e.address.as_deref().unwrap_or(""))
                        .unwrap_or((Ipv4Addr::UNSPECIFIED, 24));
                    let dns = e
                        .dns
                        .as_deref()
                        .map(|d| split_dns(d).join(","))
                        .unwrap_or_default();
                    vec![
                        "con".into(),
                        "mod".into(),
                        name.clone(),
                        "ipv4.method".into(),
                        "manual".into(),
                        "ipv4.addresses".into(),
                        format!("{ip}/{prefix}"),
                        "ipv4.gateway".into(),
                        e.gateway.clone().unwrap_or_default(),
                        "ipv4.dns".into(),
                        dns,
                    ]
                };
                let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                match nm(&refs).await {
                    Ok(_) => {
                        if let Err(e) = nm(&["con", "up", name]).await {
                            problems
                                .push(format!("the wired connection didn't come back up ({e})"));
                        }
                    }
                    Err(e) => problems.push(format!("the wired settings couldn't be saved ({e})")),
                }
            } else {
                problems.push("there's no wired connection to configure".into());
            }
        }
        let wifi_changed = !new.wifi.ssid.is_empty()
            && (new.wifi.ssid != current.wifi.ssid
                || new.wifi.psk.as_deref().is_some_and(|p| !p.is_empty()));
        if wifi_changed {
            let _ = nm(&["con", "delete", "pixelplus-wifi"]).await;
            let psk = new.wifi.psk.as_deref().filter(|p| !p.is_empty());
            // The password goes to nmcli on stdin (`--ask`), never on its command
            // line where every local user could read it (/proc/<pid>/cmdline).
            let args = wifi_connect_args(&new.wifi.ssid, psk.is_some());
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let stdin = psk.map(|p| format!("{p}\n"));
            if let Err(e) = nm_stdin(&args, stdin.as_deref()).await {
                problems.push(format!("couldn't join \"{}\" ({e})", new.wifi.ssid));
            }
        }
        if problems.is_empty() {
            events.toast(ToastKind::Success, "Network settings applied.");
            tracing::info!("Network settings applied");
        } else {
            let msg = format!("Some network changes didn't work: {}.", problems.join("; "));
            tracing::warn!("{msg}");
            events.toast(ToastKind::Error, msg);
        }
    });
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> NetworkConfig {
        NetworkConfig {
            hostname: "pixelplus-main".into(),
            managed: true,
            ..Default::default()
        }
    }

    #[test]
    fn wifi_password_never_on_the_command_line() {
        let a = wifi_connect_args("Home", true);
        assert_eq!(a[0], "--ask");
        assert!(!a.iter().any(|x| x == "password"));
        assert!(!wifi_connect_args("Open", false).contains(&"--ask".to_string()));
    }

    #[test]
    fn netwatch_status() {
        let dir = std::env::temp_dir().join(format!("pp-nw-{}", pixelplus_core::model::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("netwatch.json");
        assert!(read_netwatch(&p).is_none());
        // Exactly what image/netwatch/netwatch.py writes.
        std::fs::write(
            &p,
            r#"{"state": "hotspot", "hotspotSsid": "PixelPlus-1A2B", "hotspotSecured": false, "portalUrl": "http://10.42.0.1/", "lastError": "Wrong password for \"Home\"", "lastJoined": null, "updatedAt": 1790000000}"#,
        )
        .unwrap();
        let s = read_netwatch(&p).unwrap();
        assert_eq!(s.state, "hotspot");
        assert_eq!(s.hotspot_ssid.as_deref(), Some("PixelPlus-1A2B"));
        assert_eq!(s.portal_url.as_deref(), Some("http://10.42.0.1/"));
        assert!(s.last_error.unwrap().contains("Home"));
        std::fs::write(
            &p,
            r#"{"state": "online", "hotspotSsid": null, "hotspotSecured": true, "portalUrl": null, "lastError": null, "lastJoined": {"ssid": "Home", "ips": ["192.168.1.5"], "at": 1790000000}, "updatedAt": 1790000001}"#,
        )
        .unwrap();
        let s = read_netwatch(&p).unwrap();
        assert_eq!(s.last_joined.unwrap().ips, vec!["192.168.1.5"]);
        // The UI's PUT echoes the config back; netwatch is read-only.
        let c: NetworkConfig =
            serde_json::from_str(r#"{"hostname":"x","netwatch":{"state":"hotspot"}}"#).unwrap();
        assert!(c.netwatch.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validation() {
        assert!(validate(&cfg()).is_ok());
        let mut c = cfg();
        c.hostname = "bad name".into();
        assert!(validate(&c).is_err());
        c = cfg();
        c.wifi.psk = Some("short".into());
        assert!(validate(&c).unwrap_err().message.contains("8 to 63"));
        c = cfg();
        c.ethernet = EthernetConfig {
            dhcp: false,
            address: Some("192.168.1.50".into()),
            gateway: Some("192.168.2.1".into()),
            dns: None,
        };
        assert!(validate(&c).unwrap_err().message.contains("same network"));
        c.ethernet.gateway = Some("192.168.1.1".into());
        c.ethernet.dns = Some("1.1.1.1, 8.8.8.8".into());
        assert!(validate(&c).is_ok());
        c.ethernet.dns = Some("dns.google".into());
        assert!(validate(&c).is_err());
    }

    #[test]
    fn scan_parsing() {
        let t = "Home:90:WPA2\nHome:40:WPA2\nCafe:60:\n:30:WPA2\nMy\\:Net:70:WPA1 WPA2\n";
        let n = parse_scan(t);
        assert_eq!(n.len(), 3);
        assert_eq!(n[0].ssid, "Home");
        assert_eq!(n[0].quality, 90);
        assert_eq!(n[1].ssid, "My:Net");
        assert!(!n[2].secure);
    }
}
