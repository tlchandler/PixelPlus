//! Settings collected by the Imager and their rendering into `pixelplus.txt`.
//!
//! The file format and validation rules mirror `image/firstboot/pptxt.py` (the parser
//! that runs on the Pi). A golden file shared by both test suites
//! (`image/tests/fixtures/imager-rendered.txt`) keeps them in step.

use serde::{Deserialize, Serialize};

/// The commented template shipped on every image (`/boot/firmware/pixelplus.txt`).
/// Used when the image being written has no `pixelplus.txt` of its own.
pub const TEMPLATE: &str = include_str!("../../../image/boot/pixelplus.txt");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Leader,
    Follower,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ImagerSettings {
    pub wifi_ssid: String,
    pub wifi_password: String,
    pub wifi_country: String,
    pub wifi_hidden: bool,
    pub hostname: String,
    pub role: Option<Role>,
    pub timezone: String,
    pub ui_password: String,
    pub ssh: bool,
    pub ssh_password: String,
    pub ssh_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldError {
    pub field: &'static str,
    pub message: String,
}

fn fe(field: &'static str, message: impl Into<String>) -> FieldError {
    FieldError { field, message: message.into() }
}

pub fn valid_hostname(h: &str) -> bool {
    let b = h.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        && b[0] != b'-'
        && b[b.len() - 1] != b'-'
}

fn valid_psk(p: &str) -> bool {
    (p.len() == 64 && p.bytes().all(|c| c.is_ascii_hexdigit()))
        || ((8..=63).contains(&p.len()) && !p.chars().any(|c| (c as u32) < 32))
}

impl ImagerSettings {
    /// Normalise user input (trim, lower-case the hostname, upper-case the country).
    pub fn normalized(&self) -> ImagerSettings {
        let mut s = self.clone();
        s.hostname = s.hostname.trim().trim_end_matches(".local").to_ascii_lowercase();
        s.wifi_country = s.wifi_country.trim().to_ascii_uppercase();
        s.timezone = s.timezone.trim().to_string();
        s.ssh_key = s.ssh_key.trim().to_string();
        s
    }

    /// Validate like the on-device parser does. Empty fields are fine ("leave as is").
    pub fn validate(&self) -> Vec<FieldError> {
        let s = self.normalized();
        let mut errs = Vec::new();
        if s.wifi_ssid.len() > 32 {
            errs.push(fe("wifiSsid", "A Wi-Fi name can be at most 32 bytes."));
        }
        if s.wifi_ssid.chars().any(|c| (c as u32) < 32) {
            errs.push(fe("wifiSsid", "The Wi-Fi name contains invalid characters."));
        }
        if !s.wifi_password.is_empty() {
            if s.wifi_ssid.is_empty() {
                errs.push(fe("wifiPassword", "Enter the Wi-Fi name first."));
            } else if !valid_psk(&s.wifi_password) {
                errs.push(fe("wifiPassword", "Wi-Fi passwords are 8 to 63 characters."));
            }
        }
        if !s.wifi_country.is_empty()
            && !(s.wifi_country.len() == 2 && s.wifi_country.bytes().all(|c| c.is_ascii_uppercase()))
        {
            errs.push(fe("wifiCountry", "Choose a country."));
        }
        if !s.wifi_ssid.is_empty() && s.wifi_country.is_empty() {
            errs.push(fe("wifiCountry", "Wi-Fi needs the country it is used in."));
        }
        if !s.hostname.is_empty() && !valid_hostname(&s.hostname) {
            errs.push(fe("hostname", "Use letters, numbers and dashes (not at the start or end)."));
        }
        if !s.timezone.is_empty()
            && (s.timezone.contains("..")
                || !s.timezone.chars().all(|c| c.is_ascii_alphanumeric() || "/_+-".contains(c)))
        {
            errs.push(fe("timezone", "Not a valid time zone."));
        }
        if !s.ui_password.is_empty() && s.ui_password.chars().count() < 4 {
            errs.push(fe("uiPassword", "Use at least 4 characters."));
        }
        if !s.ssh_password.is_empty() && (s.ssh_password.len() < 8 || s.ssh_password.contains(':')) {
            errs.push(fe("sshPassword", "Use at least 8 characters (no ':')."));
        }
        if s.ssh && s.ssh_password.is_empty() && s.ssh_key.is_empty() {
            errs.push(fe("sshPassword", "Set a password or a key, or turn SSH off."));
        }
        if !s.ssh_key.is_empty() {
            let ok = ["ssh-ed25519 ", "ssh-rsa ", "ecdsa-sha2-", "sk-ssh-ed25519@openssh.com ", "sk-ecdsa-sha2-"]
                .iter()
                .any(|p| s.ssh_key.starts_with(p));
            if !ok || s.ssh_key.contains('\n') {
                errs.push(fe("sshKey", "Paste a public key (ssh-ed25519 AAAA...)."));
            }
        }
        errs
    }

    /// `(key, value)` pairs written into pixelplus.txt, in file order.
    pub fn values(&self) -> Vec<(&'static str, String)> {
        let s = self.normalized();
        let yn = |b: bool| if b { "yes" } else { "no" }.to_string();
        vec![
            ("wifi_ssid", s.wifi_ssid.clone()),
            ("wifi_password", s.wifi_password.clone()),
            ("wifi_country", s.wifi_country.clone()),
            ("wifi_hidden", yn(s.wifi_hidden)),
            ("hostname", s.hostname.clone()),
            (
                "role",
                match s.role {
                    Some(Role::Leader) => "leader".into(),
                    Some(Role::Follower) => "follower".into(),
                    None => String::new(),
                },
            ),
            ("timezone", s.timezone.clone()),
            ("ui_password", s.ui_password.clone()),
            ("ssh", if s.ssh { "on".into() } else { String::new() }),
            ("ssh_password", s.ssh_password.clone()),
            ("ssh_key", s.ssh_key.clone()),
        ]
    }
}

/// Quote a value so the on-device parser reads it back unchanged
/// (same rules as `pptxt.quote_if_needed`).
pub fn quote_if_needed(v: &str) -> String {
    if v.is_empty() {
        return String::new();
    }
    let needs = v != v.trim() || (v.len() >= 2 && v.starts_with('"') && v.ends_with('"'));
    if needs {
        format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        v.to_string()
    }
}

fn line_key(line: &str) -> Option<String> {
    let t = line.trim_start();
    if t.is_empty() || t.starts_with('#') || t.starts_with(';') {
        return None;
    }
    let (k, _) = t.split_once('=')?;
    Some(k.trim().to_ascii_lowercase().replace('-', "_"))
}

/// Fill `settings` into an existing pixelplus.txt text (keeping every comment and the
/// file's line endings). Keys missing from the template are appended at the end.
pub fn render_into(template: &str, settings: &ImagerSettings) -> String {
    let template = template.strip_prefix('\u{feff}').unwrap_or(template);
    let nl = if template.contains("\r\n") { "\r\n" } else { "\n" };
    let values = settings.values();
    let mut done = vec![false; values.len()];
    let mut out: Vec<String> = Vec::new();
    for line in template.lines() {
        if let Some(k) = line_key(line) {
            if let Some(i) = values.iter().position(|(key, _)| *key == k) {
                if done[i] {
                    // duplicate key in the template: drop it so ours is the only value
                    continue;
                }
                done[i] = true;
                let prefix = &line[..line.find('=').unwrap()];
                out.push(format!("{}={}", prefix.trim_end(), quote_if_needed(&values[i].1)));
                continue;
            }
        }
        out.push(line.to_string());
    }
    let missing: Vec<_> = values.iter().zip(done.iter()).filter(|(_, d)| !**d).map(|(v, _)| v).collect();
    if !missing.is_empty() {
        out.push(String::new());
        out.push("# Added by PixelPlus Imager".to_string());
        for (k, v) in missing {
            out.push(format!("{k}={}", quote_if_needed(v)));
        }
    }
    let mut s = out.join(nl);
    s.push_str(nl);
    s
}

/// Parse `key=value` lines (for tests and for reading back what was written).
pub fn parse_values(text: &str) -> Vec<(String, String)> {
    let mut v = Vec::new();
    for line in text.lines() {
        if let Some(k) = line_key(line) {
            let raw = line.split_once('=').map(|x| x.1).unwrap_or("").trim();
            let val = if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
                let inner = &raw[1..raw.len() - 1];
                let mut o = String::new();
                let mut it = inner.chars().peekable();
                while let Some(c) = it.next() {
                    if c == '\\' {
                        if let Some(&n) = it.peek() {
                            if n == '"' || n == '\\' {
                                o.push(n);
                                it.next();
                                continue;
                            }
                        }
                    }
                    o.push(c);
                }
                o
            } else {
                raw.to_string()
            };
            v.push((k, val));
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn golden_settings() -> ImagerSettings {
        ImagerSettings {
            wifi_ssid: "Chandler Home".into(),
            wifi_password: "  pa ss#word  ".into(),
            wifi_country: "us".into(),
            wifi_hidden: false,
            hostname: "PixelPlus-Garage".into(),
            role: Some(Role::Follower),
            timezone: "America/Chicago".into(),
            ui_password: "letmein1".into(),
            ssh: true,
            ssh_password: "sshpassword".into(),
            ssh_key: String::new(),
        }
    }

    #[test]
    fn golden_file_matches() {
        let rendered = render_into(TEMPLATE, &golden_settings());
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../image/tests/fixtures/imager-rendered.txt");
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
            std::fs::write(path, &rendered).unwrap();
        }
        let golden = std::fs::read_to_string(path).expect("golden file (run with UPDATE_GOLDEN=1)");
        assert_eq!(rendered, golden, "re-run with UPDATE_GOLDEN=1 if the template changed on purpose");
    }

    #[test]
    fn render_keeps_comments_and_roundtrips() {
        let out = render_into(TEMPLATE, &golden_settings());
        assert_eq!(out.lines().count(), TEMPLATE.lines().count());
        let vals = parse_values(&out);
        let get = |k: &str| vals.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.clone()).unwrap();
        assert_eq!(get("wifi_password"), "  pa ss#word  ");
        assert_eq!(get("hostname"), "pixelplus-garage");
        assert_eq!(get("wifi_country"), "US");
        assert_eq!(get("hotspot_password"), "pixelplus");
        assert_eq!(get("board"), "auto");
    }

    #[test]
    fn crlf_and_missing_keys() {
        let t = "# hi\r\nwifi_ssid=\r\n";
        let s = ImagerSettings { wifi_ssid: "A".into(), hostname: "x".into(), ..Default::default() };
        let out = render_into(t, &s);
        assert!(out.starts_with("# hi\r\nwifi_ssid=A\r\n"));
        assert!(out.contains("\r\nhostname=x\r\n"));
        assert!(out.ends_with("\r\n"));
    }

    #[test]
    fn validation() {
        assert!(golden_settings().validate().is_empty());
        let bad = ImagerSettings {
            wifi_ssid: "x".into(),
            wifi_password: "short".into(),
            hostname: "-no".into(),
            ssh: true,
            ..Default::default()
        };
        let fields: Vec<_> = bad.validate().iter().map(|e| e.field).collect();
        assert_eq!(fields, vec!["wifiPassword", "wifiCountry", "hostname", "sshPassword"]);
        assert!(valid_hostname("pixelplus-1"));
        assert!(!valid_hostname("Pixel"));
    }

    #[test]
    fn quoting() {
        for v in ["plain", " lead", "trail ", "\"q\"", "a\"b ", "b\\s "] {
            let line = format!("k={}", quote_if_needed(v));
            assert_eq!(parse_values(&line)[0].1, v);
        }
    }
}
