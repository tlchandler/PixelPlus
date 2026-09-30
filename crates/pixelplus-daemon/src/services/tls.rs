//! HTTPS for phones (F1, ARCHITECTURE §12.1 "Secure connection").
//!
//! Browsers only give web pages the camera and microphone over a secure
//! connection. The leader therefore runs its own small certificate authority:
//!
//! * **Local CA** — "PixelPlus Local CA – <show> – <id>", EC P-256, 10 years,
//!   `CA:true pathlen:0` and critical **name constraints**: it can only vouch
//!   for `.local`, `.lan`, `.home.arpa`, `.internal`, `localhost`, this
//!   controller's single-label host name, and private / link-local / CGNAT
//!   (Tailscale) IP ranges. Even a stolen CA key cannot impersonate a public
//!   web site. Phones install it once (`/trust`).
//! * **Leaf** — 397 days, SANs = host-name variants + every current LAN IP +
//!   `settings.https.extraNames` (only names the CA may sign). It is re-issued
//!   automatically when an address or the host name changes, 30 days before
//!   it expires, or when the clock proves it not yet/no longer valid. Phones
//!   trust the CA, so a re-issue never needs a re-install.
//! * **Storage** — `<data>/tls/{ca.key (0600), ca.crt, ca.json, leaf.key
//!   (0600), leaf.crt, leaf.json}`. Excluded from normal snapshots; the F10
//!   controller transfer carries the CA via [`export_ca`] / [`import_ca`] so
//!   phones keep trusting a replacement leader.
//!
//! The listener itself lives in `listeners.rs`; it reads the current leaf
//! through [`TlsState::server_config`] (hot-swapped, no restart needed).
//!
//! Followers do not run HTTPS: phones measure and map through the leader.

use crate::state::AppState;
use anyhow::{anyhow, Context};
use chrono::{DateTime, Datelike, Duration as ChronoDuration, Utc};
use parking_lot::{Mutex, RwLock};
use rcgen::{
    BasicConstraints, CertificateParams, CidrSubnet, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, GeneralSubtree, IsCa, KeyPair, KeyUsagePurpose, NameConstraints,
    SanType, SerialNumber,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_rustls::rustls;

/// Leaf validity in days (Chrome refuses more than 398).
pub const LEAF_DAYS: i64 = 397;
/// CA validity in years.
pub const CA_YEARS: i32 = 10;
/// Re-issue the leaf this long before it expires.
pub const RENEW_BEFORE_DAYS: i64 = 30;
/// How often the service checks addresses / host name / expiry.
const CHECK_EVERY: Duration = Duration::from_secs(60);
/// A Pi without a real-time clock may boot with a date far in the past;
/// certificates are never dated before this (phones have the right date).
const CLOCK_FLOOR: (i32, u32, u32) = (2026, 1, 1);
/// Show the CA fingerprint on the OLED for this long after the trust page
/// or the HTTPS settings asked for it.
const OLED_FINGERPRINT_FOR: Duration = Duration::from_secs(10 * 60);

/// DNS subtrees every PixelPlus CA may sign (plus the host name it was
/// created with). Multi-label names under these (e.g. `pixelplus.local`) match.
pub const PERMITTED_DNS: &[&str] = &["local", "lan", "home.arpa", "internal", "localhost"];

/// IP ranges every PixelPlus CA may sign: RFC 1918, link-local, CGNAT
/// (Tailscale), loopback, IPv6 ULA / link-local / loopback.
pub fn permitted_ip_ranges() -> Vec<(IpAddr, u8)> {
    [
        ("10.0.0.0", 8),
        ("172.16.0.0", 12),
        ("192.168.0.0", 16),
        ("169.254.0.0", 16),
        ("100.64.0.0", 10),
        ("127.0.0.0", 8),
        ("fc00::", 7),
        ("fe80::", 10),
        ("::1", 128),
    ]
    .iter()
    .map(|(a, p)| (a.parse().expect("static address"), *p))
    .collect()
}

fn ip_in(ip: IpAddr, net: IpAddr, prefix: u8) -> bool {
    match (ip, net) {
        (IpAddr::V4(a), IpAddr::V4(n)) => {
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - prefix.min(32) as u32)
            };
            u32::from(a) & mask == u32::from(n) & mask
        }
        (IpAddr::V6(a), IpAddr::V6(n)) => {
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - prefix.min(128) as u32)
            };
            u128::from(a) & mask == u128::from(n) & mask
        }
        _ => false,
    }
}

/// May a CA with DNS constraints `dns` sign this name / address?
pub fn permitted(name: &str, dns: &[String]) -> bool {
    if let Ok(ip) = name.parse::<IpAddr>() {
        let ip = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
            v4 => v4,
        };
        return permitted_ip_ranges()
            .into_iter()
            .any(|(n, p)| ip_in(ip, n, p));
    }
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    if !valid_dns_name(&name) {
        return false;
    }
    dns.iter()
        .any(|c| name == *c || name.ends_with(&format!(".{c}")))
}

/// A plain host name: labels of letters, digits and hyphens.
pub fn valid_dns_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// Could `label` be a top-level domain? A dNSName name constraint also
/// permits every name *below* it (RFC 5280 4.2.1.10), so a CA constrained to
/// the bare host name `christmas` (or `shop`, `app`, `lighting`…, all real
/// TLDs) could sign `anything.christmas` on the internet. ICANN TLDs are
/// letters only (or `xn--` IDNs), so a label with a digit or a hyphen, not
/// starting `xn--`, can never be one.
pub fn could_be_tld(label: &str) -> bool {
    let l = label.to_ascii_lowercase();
    l.starts_with("xn--") || !l.chars().any(|c| c.is_ascii_digit() || c == '-')
}

/// The DNS constraints for a new CA on a host called `hostname`.
pub fn ca_dns_constraints(hostname: &str) -> Vec<String> {
    let mut v: Vec<String> = PERMITTED_DNS.iter().map(|s| s.to_string()).collect();
    let h = hostname.trim().to_ascii_lowercase();
    // A single-label host name ("garage-pi") lets phones use the bare name on
    // networks whose DNS resolves it. Dotted names are not added: they could
    // be a public domain; nor are labels that could be a top-level domain
    // ([`could_be_tld`]): those phones use `<name>.local` / `<name>.lan`.
    if !h.is_empty()
        && !h.contains('.')
        && valid_dns_name(&h)
        && !could_be_tld(&h)
        && !v.contains(&h)
    {
        v.push(h);
    }
    v
}

/// Every name the leaf should carry, in a stable order: host-name variants,
/// extra names, then addresses — filtered to what the CA may sign.
pub fn desired_names(
    hostname: &str,
    ips: &[IpAddr],
    extra: &[String],
    dns: &[String],
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |n: String| {
        if permitted(&n, dns) && !out.contains(&n) {
            out.push(n);
        }
    };
    let h = hostname.trim().trim_end_matches('.').to_ascii_lowercase();
    if !h.is_empty() {
        if h.ends_with(".local") {
            push(h.clone());
        } else {
            push(format!("{h}.local"));
            push(format!("{h}.lan"));
            push(h.clone());
        }
    }
    push("localhost".into());
    for e in extra {
        push(e.trim().trim_end_matches('.').to_ascii_lowercase());
    }
    let mut ips: Vec<IpAddr> = ips.to_vec();
    ips.sort();
    ips.dedup();
    for ip in ips {
        push(ip.to_string());
    }
    out
}

/// This host's addresses for the certificate (no IPv6 link-local: browsers
/// cannot use zone ids in URLs).
pub fn local_addresses() -> Vec<IpAddr> {
    let mut v: Vec<IpAddr> = vec![IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)];
    if let Ok(list) = if_addrs::get_if_addrs() {
        for iface in list {
            let ip = iface.ip();
            if let IpAddr::V6(v6) = ip {
                if (v6.segments()[0] & 0xffc0) == 0xfe80 {
                    continue;
                }
            }
            if !v.contains(&ip) {
                v.push(ip);
            }
        }
    }
    v
}

fn floor_now(now: DateTime<Utc>) -> DateTime<Utc> {
    let floor = chrono::NaiveDate::from_ymd_opt(CLOCK_FLOOR.0, CLOCK_FLOOR.1, CLOCK_FLOOR.2)
        .expect("valid floor date")
        .and_hms_opt(0, 0, 0)
        .expect("midnight")
        .and_utc();
    now.max(floor)
}

fn random_serial() -> SerialNumber {
    let mut b: [u8; 16] = rand::random();
    b[0] &= 0x7f; // positive
    b[0] |= 0x01; // no leading zero byte
    SerialNumber::from_slice(&b)
}

/// SHA-256 of DER, as `AB:CD:…`.
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// What `ca.json` records (everything needed to sign with the CA again).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CaMeta {
    pub common_name: String,
    pub permitted_dns: Vec<String>,
    pub created_at: String,
    pub not_after: String,
}

/// What `leaf.json` records.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LeafMeta {
    pub names: Vec<String>,
    pub not_before: String,
    pub not_after: String,
    pub issued_at: String,
    pub ca_fingerprint: String,
}

/// A CA: key + certificate + metadata.
pub struct Ca {
    pub key: KeyPair,
    pub cert_der: Vec<u8>,
    pub cert_pem: String,
    pub meta: CaMeta,
}

/// A leaf certificate and its key (PKCS#8 DER).
#[derive(Clone)]
pub struct Leaf {
    pub key_der: Vec<u8>,
    pub cert_der: Vec<u8>,
    pub cert_pem: String,
    pub key_pem: String,
    pub meta: LeafMeta,
}

fn ca_params(meta: &CaMeta) -> anyhow::Result<CertificateParams> {
    let mut p = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, meta.common_name.clone());
    dn.push(DnType::OrganizationName, "PixelPlus");
    p.distinguished_name = dn;
    p.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    p.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let mut permitted: Vec<GeneralSubtree> = meta
        .permitted_dns
        .iter()
        .map(|d| GeneralSubtree::DnsName(d.clone()))
        .collect();
    permitted.extend(
        permitted_ip_ranges().into_iter().map(|(ip, prefix)| {
            GeneralSubtree::IpAddress(CidrSubnet::from_addr_prefix(ip, prefix))
        }),
    );
    p.name_constraints = Some(NameConstraints {
        permitted_subtrees: permitted,
        excluded_subtrees: vec![],
    });
    Ok(p)
}

/// The CA's common name: "PixelPlus Local CA – <show> – <id>".
pub fn ca_common_name(show_name: &str, node_id: &str) -> String {
    let show: String = show_name
        .chars()
        .filter(|c| !c.is_control())
        .take(40)
        .collect::<String>()
        .trim()
        .to_string();
    let short: String = node_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(4)
        .collect::<String>()
        .to_ascii_lowercase();
    let show = if show.is_empty() {
        "My show".into()
    } else {
        show
    };
    if short.is_empty() {
        format!("PixelPlus Local CA – {show}")
    } else {
        format!("PixelPlus Local CA – {show} – {short}")
    }
}

/// Create a new CA.
pub fn create_ca(common_name: &str, hostname: &str, now: DateTime<Utc>) -> anyhow::Result<Ca> {
    let now = floor_now(now);
    let not_before = now - ChronoDuration::days(2);
    let not_after = not_before
        .with_year(not_before.year() + CA_YEARS)
        .unwrap_or(not_before + ChronoDuration::days(365 * CA_YEARS as i64));
    let meta = CaMeta {
        common_name: common_name.to_string(),
        permitted_dns: ca_dns_constraints(hostname),
        created_at: now.to_rfc3339(),
        not_after: not_after.to_rfc3339(),
    };
    let mut params = ca_params(&meta)?;
    params.not_before = rcgen::date_time_ymd(
        not_before.year(),
        not_before.month() as u8,
        not_before.day() as u8,
    );
    params.not_after = rcgen::date_time_ymd(
        not_after.year(),
        not_after.month() as u8,
        not_after.day() as u8,
    );
    params.serial_number = Some(random_serial());
    let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let cert = params.self_signed(&key)?;
    Ok(Ca {
        cert_der: cert.der().to_vec(),
        cert_pem: cert.pem(),
        key,
        meta,
    })
}

/// Issue a leaf for `names` (already filtered with [`desired_names`]).
pub fn issue_leaf(ca: &Ca, names: &[String], now: DateTime<Utc>) -> anyhow::Result<Leaf> {
    if names.is_empty() {
        return Err(anyhow!("no names for the certificate"));
    }
    let now = floor_now(now);
    let not_before = now - ChronoDuration::days(1);
    let not_after = not_before + ChronoDuration::days(LEAF_DAYS);
    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, names[0].clone());
    dn.push(DnType::OrganizationName, "PixelPlus");
    params.distinguished_name = dn;
    params.subject_alt_names = names
        .iter()
        .map(|n| {
            Ok(match n.parse::<IpAddr>() {
                Ok(ip) => SanType::IpAddress(ip),
                Err(_) => SanType::DnsName(n.clone().try_into()?),
            })
        })
        .collect::<Result<Vec<_>, rcgen::Error>>()?;
    params.is_ca = IsCa::ExplicitNoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.use_authority_key_identifier_extension = true;
    params.serial_number = Some(random_serial());
    params.not_before = rcgen::date_time_ymd(
        not_before.year(),
        not_before.month() as u8,
        not_before.day() as u8,
    );
    params.not_after = rcgen::date_time_ymd(
        not_after.year(),
        not_after.month() as u8,
        not_after.day() as u8,
    );
    let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let issuer_params = ca_params(&ca.meta)?;
    let issuer = rcgen::Issuer::from_params(&issuer_params, &ca.key);
    let cert = params.signed_by(&key, &issuer)?;
    Ok(Leaf {
        key_der: key.serialize_der(),
        key_pem: key.serialize_pem(),
        cert_der: cert.der().to_vec(),
        cert_pem: cert.pem(),
        meta: LeafMeta {
            names: names.to_vec(),
            not_before: not_before.to_rfc3339(),
            not_after: not_after.to_rfc3339(),
            issued_at: now.to_rfc3339(),
            ca_fingerprint: fingerprint(&ca.cert_der),
        },
    })
}

/// Why the leaf must be re-issued now (`None` = it is fine).
pub fn reissue_reason(
    leaf: &LeafMeta,
    ca_fp: &str,
    desired: &[String],
    now: DateTime<Utc>,
) -> Option<&'static str> {
    if leaf.ca_fingerprint != ca_fp {
        return Some("the certificate authority changed");
    }
    if desired.iter().any(|n| !leaf.names.contains(n)) {
        return Some("an address or the host name changed");
    }
    let parse = |s: &str| {
        DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.with_timezone(&Utc))
    };
    let (Some(nb), Some(na)) = (parse(&leaf.not_before), parse(&leaf.not_after)) else {
        return Some("its dates are unreadable");
    };
    // The clock only matters once it is plausible (no RTC before NTP sync).
    if now >= floor_now(now) {
        if now < nb - ChronoDuration::days(1) {
            return Some("the clock moved back before its start date");
        }
        if now + ChronoDuration::days(RENEW_BEFORE_DAYS) > na {
            return Some("it expires soon");
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

/// `<data>/tls`.
pub fn tls_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("tls")
}

fn write_atomic(path: &Path, data: &[u8], secret: bool) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    // A leftover temp file (crash) may have other permissions: start afresh,
    // so a private key is never written into a readable file.
    let _ = std::fs::remove_file(&tmp);
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(if secret { 0o600 } else { 0o644 });
        }
        #[cfg(not(unix))]
        let _ = secret;
        let mut f = opts.open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    #[cfg(unix)]
    if secret {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)
}

fn pem_to_der(pem: &str) -> anyhow::Result<Vec<u8>> {
    let body: String = pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .map(str::trim)
        .collect();
    b64_decode(&body).ok_or_else(|| anyhow!("not a PEM certificate"))
}

/// Save a CA into `dir` (ca.key, ca.crt, ca.json).
pub fn save_ca(dir: &Path, ca: &Ca) -> anyhow::Result<()> {
    write_atomic(&dir.join("ca.key"), ca.key.serialize_pem().as_bytes(), true)?;
    write_atomic(&dir.join("ca.crt"), ca.cert_pem.as_bytes(), false)?;
    write_atomic(
        &dir.join("ca.json"),
        &serde_json::to_vec_pretty(&ca.meta)?,
        false,
    )?;
    Ok(())
}

/// Load the CA from `dir`, if all its files are there and consistent.
pub fn load_ca(dir: &Path) -> anyhow::Result<Option<Ca>> {
    let key_path = dir.join("ca.key");
    if !key_path.exists() {
        return Ok(None);
    }
    let key = KeyPair::from_pem(&std::fs::read_to_string(&key_path)?).context("reading ca.key")?;
    let cert_pem = std::fs::read_to_string(dir.join("ca.crt")).context("reading ca.crt")?;
    let meta: CaMeta =
        serde_json::from_slice(&std::fs::read(dir.join("ca.json"))?).context("reading ca.json")?;
    let cert_der = pem_to_der(&cert_pem)?;
    Ok(Some(Ca {
        key,
        cert_der,
        cert_pem,
        meta,
    }))
}

/// Save a leaf into `dir` (leaf.key, leaf.crt, leaf.json).
pub fn save_leaf(dir: &Path, leaf: &Leaf) -> anyhow::Result<()> {
    write_atomic(&dir.join("leaf.key"), leaf.key_pem.as_bytes(), true)?;
    write_atomic(&dir.join("leaf.crt"), leaf.cert_pem.as_bytes(), false)?;
    write_atomic(
        &dir.join("leaf.json"),
        &serde_json::to_vec_pretty(&leaf.meta)?,
        false,
    )?;
    Ok(())
}

/// Load the leaf from `dir`, if present.
pub fn load_leaf(dir: &Path) -> anyhow::Result<Option<Leaf>> {
    let key_path = dir.join("leaf.key");
    if !key_path.exists() {
        return Ok(None);
    }
    let key_pem = std::fs::read_to_string(&key_path)?;
    let key = KeyPair::from_pem(&key_pem).context("reading leaf.key")?;
    let cert_pem = std::fs::read_to_string(dir.join("leaf.crt"))?;
    let meta: LeafMeta = serde_json::from_slice(&std::fs::read(dir.join("leaf.json"))?)?;
    Ok(Some(Leaf {
        key_der: key.serialize_der(),
        key_pem,
        cert_der: pem_to_der(&cert_pem)?,
        cert_pem,
        meta,
    }))
}

// ---------------------------------------------------------------------------
// Runtime state
// ---------------------------------------------------------------------------

/// The CA + leaf in use.
pub struct Material {
    pub ca_der: Vec<u8>,
    pub ca_meta: CaMeta,
    pub ca_fingerprint: String,
    pub leaf: Leaf,
}

/// Hands rustls the current leaf (swapped on re-issue without a restart).
#[derive(Debug, Default)]
pub struct LeafResolver {
    current: RwLock<Option<Arc<rustls::sign::CertifiedKey>>>,
}

impl rustls::server::ResolvesServerCert for LeafResolver {
    fn resolve(
        &self,
        _hello: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        self.current.read().clone()
    }
}

/// Listener facts for `/tls/status`.
#[derive(Debug, Clone, Default)]
pub struct ListenerInfo {
    pub listening: bool,
    pub error: Option<String>,
}

/// Runtime state (`state.services.tls`).
pub struct TlsState {
    material: RwLock<Option<Arc<Material>>>,
    resolver: Arc<LeafResolver>,
    config: Arc<rustls::ServerConfig>,
    listener: Mutex<ListenerInfo>,
    last_error: Mutex<Option<String>>,
    fingerprint_shown: Mutex<Option<Instant>>,
    /// Serialises CA/leaf changes.
    busy: tokio::sync::Mutex<()>,
    wake: tokio::sync::Notify,
    started: AtomicBool,
}

impl Default for TlsState {
    fn default() -> Self {
        let resolver = Arc::new(LeafResolver::default());
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("ring supports the default TLS versions")
            .with_no_client_auth()
            .with_cert_resolver(resolver.clone());
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        TlsState {
            material: RwLock::new(None),
            resolver,
            config: Arc::new(config),
            listener: Mutex::new(ListenerInfo::default()),
            last_error: Mutex::new(None),
            fingerprint_shown: Mutex::new(None),
            busy: tokio::sync::Mutex::new(()),
            wake: tokio::sync::Notify::new(),
            started: AtomicBool::new(false),
        }
    }
}

impl TlsState {
    /// The rustls configuration for the HTTPS listener (serves the current leaf).
    pub fn server_config(&self) -> Arc<rustls::ServerConfig> {
        self.config.clone()
    }

    /// The CA + leaf in use, once created.
    pub fn material(&self) -> Option<Arc<Material>> {
        self.material.read().clone()
    }

    pub fn set_listener(&self, info: ListenerInfo) {
        *self.listener.lock() = info;
    }

    pub fn listener(&self) -> ListenerInfo {
        self.listener.lock().clone()
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().clone()
    }

    /// Someone is comparing fingerprints: show it on the OLED for a while.
    pub fn show_fingerprint(&self) {
        *self.fingerprint_shown.lock() = Some(Instant::now());
    }

    /// Check addresses / host name / expiry now instead of within a minute.
    pub fn poke(&self) {
        self.wake.notify_one();
    }

    fn install(&self, m: Material) -> anyhow::Result<()> {
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(
            rustls::pki_types::PrivatePkcs8KeyDer::from(m.leaf.key_der.clone()),
        );
        let signer = rustls::crypto::ring::sign::any_supported_type(&key)
            .map_err(|e| anyhow!("unusable certificate key: {e}"))?;
        let chain = vec![
            rustls::pki_types::CertificateDer::from(m.leaf.cert_der.clone()),
            rustls::pki_types::CertificateDer::from(m.ca_der.clone()),
        ];
        *self.resolver.current.write() =
            Some(Arc::new(rustls::sign::CertifiedKey::new(chain, signer)));
        *self.material.write() = Some(Arc::new(m));
        Ok(())
    }
}

/// Should this node serve HTTPS now? (Leader or not yet set up, and enabled.)
pub fn active(state: &AppState) -> bool {
    state.config.https_port != 0
        && state.store.get().settings.https.enabled
        && state.identity().role != crate::node::LocalRole::Follower
}

/// The first 6 bytes of the CA fingerprint while someone is comparing it
/// (trust page / HTTPS settings opened in the last 10 minutes), for the
/// OLED status screen: `CA 1A:2B:3C:4D:5E:6F`.
pub fn oled_line(state: &AppState) -> Option<String> {
    let t = state
        .services
        .tls
        .fingerprint_shown
        .lock()
        .as_ref()
        .copied()?;
    if t.elapsed() > OLED_FINGERPRINT_FOR {
        return None;
    }
    let m = state.services.tls.material()?;
    Some(format!("CA {}", m.ca_fingerprint.get(..17)?))
}

/// Make sure a CA and a current leaf exist (creating / re-issuing as needed).
/// `rotate_ca` replaces the CA (phones must install the new one);
/// `rotate_leaf` forces a new leaf.
pub async fn ensure(state: &AppState, rotate_ca: bool, rotate_leaf: bool) -> anyhow::Result<()> {
    let tls = &state.services.tls;
    let _busy = tls.busy.lock().await;
    let dir = tls_dir(&state.config.data_dir);
    let show_name = state.store.get().name.clone();
    let node_id = state.identity().id.clone();
    let hostname = crate::cluster::net::hostname();
    let extra = state.store.get().settings.https.extra_names.clone();
    let addrs = local_addresses();
    let res = tokio::task::spawn_blocking(move || -> anyhow::Result<(Material, bool)> {
        let now = Utc::now();
        let mut ca = if rotate_ca { None } else { load_ca(&dir).unwrap_or_else(|e| {
            tracing::warn!("The secure-connection certificate authority can't be read ({e:#}); making a new one");
            None
        }) };
        let mut changed = false;
        if ca.is_none() {
            let new = create_ca(&ca_common_name(&show_name, &node_id), &hostname, now)?;
            save_ca(&dir, &new)?;
            tracing::info!("Created the secure-connection certificate authority ({})", new.meta.common_name);
            ca = Some(new);
            changed = true;
        }
        let ca = ca.expect("set above");
        if let Some(bad) = ca
            .meta
            .permitted_dns
            .iter()
            .find(|d| !d.contains('.') && !PERMITTED_DNS.contains(&d.as_str()) && could_be_tld(d))
        {
            tracing::warn!(
                "The secure-connection certificate authority may also vouch for names under \".{bad}\" (a possible internet top-level domain); make a new one under Settings → Secure connection"
            );
        }
        let ca_fp = fingerprint(&ca.cert_der);
        let desired = desired_names(&hostname, &addrs, &extra, &ca.meta.permitted_dns);
        let current = if rotate_leaf || changed { None } else { load_leaf(&dir).unwrap_or(None) };
        let leaf = match current {
            Some(l) => match reissue_reason(&l.meta, &ca_fp, &desired, now) {
                None => l,
                Some(why) => {
                    tracing::info!("Renewing the HTTPS certificate: {why}");
                    changed = true;
                    let l = issue_leaf(&ca, &desired, now)?;
                    save_leaf(&dir, &l)?;
                    l
                }
            },
            None => {
                changed = true;
                let l = issue_leaf(&ca, &desired, now)?;
                save_leaf(&dir, &l)?;
                l
            }
        };
        Ok((
            Material {
                ca_der: ca.cert_der.clone(),
                ca_meta: ca.meta.clone(),
                ca_fingerprint: ca_fp,
                leaf,
            },
            changed,
        ))
    })
    .await
    .map_err(|e| anyhow!("certificate task failed: {e}"))?;
    match res {
        Ok((m, changed)) => {
            let first = tls.material().is_none();
            if changed || first {
                tls.install(m)?;
            }
            *tls.last_error.lock() = None;
            Ok(())
        }
        Err(e) => {
            *tls.last_error.lock() = Some(format!("{e:#}"));
            Err(e)
        }
    }
}

/// Start the service (called once from `services::start_all`): create or
/// load the CA and leaf, then re-check every minute (addresses, host name,
/// settings, expiry).
pub fn start(state: &AppState) {
    let tls = &state.services.tls;
    if tls.started.swap(true, Ordering::SeqCst) || state.config.https_port == 0 {
        return;
    }
    let state = state.clone();
    tokio::spawn(async move {
        let mut show = state.store.subscribe();
        let mut failures = 0u32;
        loop {
            if active(&state) {
                match ensure(&state, false, false).await {
                    Ok(()) => failures = 0,
                    Err(e) => {
                        failures += 1;
                        if failures == 1 {
                            tracing::warn!("The secure connection (HTTPS) isn't available: {e:#}");
                        }
                    }
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(CHECK_EVERY) => {}
                _ = state.services.tls.wake.notified() => {}
                r = show.changed() => { if r.is_err() { return; } }
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Controller transfer (F10, WS5): the CA travels in the encrypted bundle.
// ---------------------------------------------------------------------------

/// The CA files for the passphrase-encrypted controller transfer bundle:
/// `(ca.key PEM, ca.crt PEM, ca.json)`. `None` if no CA exists yet.
pub fn export_ca(data_dir: &Path) -> anyhow::Result<Option<(String, String, String)>> {
    let dir = tls_dir(data_dir);
    if !dir.join("ca.key").exists() {
        return Ok(None);
    }
    Ok(Some((
        std::fs::read_to_string(dir.join("ca.key"))?,
        std::fs::read_to_string(dir.join("ca.crt"))?,
        std::fs::read_to_string(dir.join("ca.json"))?,
    )))
}

/// Restore a CA from a transfer bundle (validated first); the leaf is
/// re-issued on the next check. Call [`TlsState::poke`] afterwards.
pub fn import_ca(
    data_dir: &Path,
    key_pem: &str,
    cert_pem: &str,
    meta_json: &str,
) -> anyhow::Result<()> {
    let key = KeyPair::from_pem(key_pem).context("the certificate authority key is damaged")?;
    let meta: CaMeta = serde_json::from_str(meta_json).context("ca.json is damaged")?;
    let ca = Ca {
        key,
        cert_der: pem_to_der(cert_pem)?,
        cert_pem: cert_pem.to_string(),
        meta,
    };
    // Key and certificate must belong together: the certificate carries the
    // key's public point (signing a throwaway leaf alone proves only that the
    // key works, not that phones' trusted certificate is its).
    let public = ca.key.public_key_raw();
    if public.is_empty() || !ca.cert_der.windows(public.len()).any(|w| w == public) {
        return Err(anyhow!(
            "the certificate authority's key doesn't match its certificate"
        ));
    }
    issue_leaf(&ca, &["localhost".to_string()], Utc::now())?;
    let dir = tls_dir(data_dir);
    save_ca(&dir, &ca)?;
    let _ = std::fs::remove_file(dir.join("leaf.json"));
    Ok(())
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 (for the iOS profile).
pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        if c == b'=' {
            break;
        }
        if c.is_ascii_whitespace() {
            continue;
        }
        let v = B64.iter().position(|&x| x == c)? as u32;
        buf = buf << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests;
