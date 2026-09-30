//! Network settings through NetworkManager (`nmcli`) and `hostnamectl`.
//!
//! Changes are validated first, then applied in the background so the HTTP
//! response reaches the browser before Wi-Fi drops. Progress and failures
//! are reported as toasts.

use super::system::{have, hostname, in_docker, nmcli_fields, quality_to_dbm, run};
use crate::api::{ApiError, ApiResult};
use crate::events::{EventBus, ToastKind};
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;
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
        EthernetConfig { dhcp: true, address: None, gateway: None, dns: None }
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
    s.split(|c: char| c == ',' || c.is_whitespace()).filter(|x| !x.is_empty()).collect()
}

pub fn validate(cfg: &NetworkConfig) -> ApiResult<()> {
    validate_hostname(&cfg.hostname)?;
    let w = &cfg.wifi;
    if w.ssid.len() > 32 {
        return Err(ApiError::bad_request("A Wi-Fi network name can be at most 32 characters."));
    }
    if let Some(psk) = w.psk.as_deref().filter(|p| !p.is_empty()) {
        let hex64 = psk.len() == 64 && psk.chars().all(|c| c.is_ascii_hexdigit());
        if !(8..=63).contains(&psk.chars().count()) && !hex64 {
            return Err(ApiError::bad_request("A Wi-Fi password needs 8 to 63 characters."));
        }
    }
    if !w.country.is_empty() && !(w.country.len() == 2 && w.country.chars().all(|c| c.is_ascii_alphabetic())) {
        return Err(ApiError::bad_request("Pick your Wi-Fi country (a two-letter code like US or GB)."));
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
            let gw: Ipv4Addr = gw
                .trim()
                .parse()
                .map_err(|_| ApiError::bad_request("The router (gateway) address should look like 192.168.1.1."))?;
            let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix) };
            if u32::from(gw) & mask != u32::from(ip) & mask {
                return Err(ApiError::bad_request(format!(
                    "The router address {gw} isn't on the same network as {ip}/{prefix}."
                )));
            }
        }
        if let Some(dns) = e.dns.as_deref() {
            for d in split_dns(dns) {
                if d.parse::<std::net::IpAddr>().is_err() {
                    return Err(ApiError::bad_request(format!("\"{d}\" isn't a valid DNS server address.")));
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
    let mut cfg = NetworkConfig { hostname: hostname(), managed: managed(), ..Default::default() };
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
        if let Ok(out) = nm(&["-t", "-f", "ipv4.method,ipv4.addresses,ipv4.gateway,ipv4.dns", "con", "show", name]).await {
            for l in out.lines() {
                let Some((k, v)) = l.split_once(':') else { continue };
                let v = v.trim();
                match k {
                    "ipv4.method" => cfg.ethernet.dhcp = v != "manual",
                    "ipv4.addresses" if !v.is_empty() => cfg.ethernet.address = Some(v.to_string()),
                    "ipv4.gateway" if !v.is_empty() && v != "--" => cfg.ethernet.gateway = Some(v.to_string()),
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
        let n = WifiNetwork { ssid: f[0].clone(), signal: quality_to_dbm(quality), quality, secure };
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
    let out = nm(&["-t", "-f", "SSID,SIGNAL,SECURITY", "dev", "wifi", "list", "--rescan", "yes"])
        .await
        .map_err(|e| ApiError::unavailable(format!("Couldn't scan for Wi-Fi networks: {e}")))?;
    Ok(parse_scan(&out))
}

// ---------------------------------------------------------------------------
// Applying
// ---------------------------------------------------------------------------

/// Validate, then apply in the background. Returns the config as it will be.
pub async fn apply(new: NetworkConfig, events: EventBus) -> ApiResult<NetworkConfig> {
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
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(700)).await;
        let mut problems: Vec<String> = Vec::new();
        if new.hostname != current.hostname {
            match run("hostnamectl", &["set-hostname", &new.hostname], Duration::from_secs(10)).await {
                Ok(o) if o.success => {
                    // Keep /etc/hosts resolving the new name (sudo warnings otherwise).
                    if let Ok(hosts) = std::fs::read_to_string("/etc/hosts") {
                        let old = &current.hostname;
                        let updated: String = hosts
                            .lines()
                            .map(|l| {
                                if l.starts_with("127.0.1.1") {
                                    format!("127.0.1.1\t{}", new.hostname)
                                } else if !old.is_empty() && l.split_whitespace().skip(1).any(|w| w == old) {
                                    l.replace(old.as_str(), &new.hostname)
                                } else {
                                    l.to_string()
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        let _ = std::fs::write("/etc/hosts", updated + "\n");
                    }
                    let _ = run("systemctl", &["try-restart", "avahi-daemon"], Duration::from_secs(10)).await;
                }
                Ok(o) => problems.push(format!("the name couldn't be changed ({})", o.stderr.trim())),
                Err(e) => problems.push(format!("the name couldn't be changed ({e})")),
            }
        }
        if !new.wifi.country.is_empty() && !new.wifi.country.eq_ignore_ascii_case(&current.wifi.country) {
            let cc = new.wifi.country.to_ascii_uppercase();
            let ok = if have("raspi-config") {
                run("raspi-config", &["nonint", "do_wifi_country", &cc], Duration::from_secs(20)).await
            } else {
                run("iw", &["reg", "set", &cc], Duration::from_secs(10)).await
            };
            if !matches!(ok, Ok(ref o) if o.success) {
                problems.push("the Wi-Fi country couldn't be set".into());
            }
        }
        // Ethernet before Wi-Fi: Wi-Fi changes may cut us off.
        if new.ethernet != current.ethernet {
            let conns = connections().await;
            if let Some((name, _, _, _)) = conns.iter().find(|c| c.1 == "802-3-ethernet") {
                let e = &new.ethernet;
                let args: Vec<String> = if e.dhcp {
                    ["con", "mod", name, "ipv4.method", "auto", "ipv4.addresses", "", "ipv4.gateway", "", "ipv4.dns", ""]
                        .iter()
                        .map(|s| s.to_string())
                        .collect()
                } else {
                    let (ip, prefix) = parse_cidr(e.address.as_deref().unwrap_or("")).unwrap_or((Ipv4Addr::UNSPECIFIED, 24));
                    let dns = e.dns.as_deref().map(|d| split_dns(d).join(",")).unwrap_or_default();
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
                            problems.push(format!("the wired connection didn't come back up ({e})"));
                        }
                    }
                    Err(e) => problems.push(format!("the wired settings couldn't be saved ({e})")),
                }
            } else {
                problems.push("there's no wired connection to configure".into());
            }
        }
        let wifi_changed = !new.wifi.ssid.is_empty()
            && (new.wifi.ssid != current.wifi.ssid || new.wifi.psk.as_deref().is_some_and(|p| !p.is_empty()));
        if wifi_changed {
            let _ = nm(&["con", "delete", "pixelplus-wifi"]).await;
            let mut args = vec!["--wait", "45", "dev", "wifi", "connect", new.wifi.ssid.as_str()];
            if let Some(psk) = new.wifi.psk.as_deref().filter(|p| !p.is_empty()) {
                args.extend(["password", psk]);
            }
            args.extend(["name", "pixelplus-wifi"]);
            if let Err(e) = nm(&args).await {
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
        NetworkConfig { hostname: "pixelplus-main".into(), managed: true, ..Default::default() }
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
        c.ethernet = EthernetConfig { dhcp: false, address: Some("192.168.1.50".into()), gateway: Some("192.168.2.1".into()), dns: None };
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
