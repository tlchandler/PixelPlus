//! Software updates from the PixelPlus apt repository (`pixelplus` package).
//! In Docker the image is updated instead.
//!
//! Checking (`apt-cache policy`) works as the unprivileged service user, using
//! the package lists apt refreshes daily. Installing goes through the root
//! helper (`pixelplus-helper@update.service`: apt-get update + upgrade), whose
//! progress is reported as `helper` WebSocket messages and toasts.

use super::platform::{self, HelperOpts, HelperStatus, HelperVerb};
use super::system::{have, in_docker, is_root, run};
use crate::api::{ApiError, ApiResult};
use crate::state::AppState;
use pixelplus_core::model::UpdateChannel;
use serde::Serialize;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub available: bool,
    /// Whether `POST /system/update` can install it here.
    pub can_apply: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The latest install run (progress of `POST /system/update`), if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job: Option<HelperStatus>,
    /// Signed over-the-air updates are used (F15) rather than apt.
    #[serde(default)]
    pub ota: bool,
    /// Every controller of the show and its version (F15; leader).
    #[serde(default)]
    pub nodes: Vec<NodeVersion>,
    /// What keeps "Update everything" from starting now.
    #[serde(default)]
    pub problems: Vec<String>,
    /// The current / last cluster update job.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<super::updates_orch::UpdateJob>,
    #[serde(default)]
    pub history: Vec<super::updates_orch::HistoryEntry>,
    /// "Roll back to …" is possible (the version before the last update).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<String>,
}

/// A controller's version (`GET /system/update` `nodes`).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NodeVersion {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proto: Option<u32>,
    pub can_apply: bool,
    pub online: bool,
    pub arch: String,
    /// Its update phase (`idle`, `staging`, `staged`, `failed`…).
    pub phase: String,
}

const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// Parse `apt-cache policy pixelplus` → (installed, candidate, channel).
pub fn parse_policy(text: &str) -> (Option<String>, Option<String>, Option<String>) {
    let field = |name: &str| {
        text.lines()
            .find_map(|l| l.trim().strip_prefix(name).map(|v| v.trim().to_string()))
            .filter(|v| !v.is_empty() && v != "(none)")
    };
    let channel = text
        .lines()
        .skip_while(|l| !l.contains("***"))
        .nth(1)
        .and_then(|l| {
            l.split_whitespace()
                .nth(2)
                .map(|s| s.split('/').next().unwrap_or(s).to_string())
        });
    (field("Installed:"), field("Candidate:"), channel)
}

/// Whether this machine can install the update itself.
fn can_install() -> bool {
    platform::helper_installed() || (is_root() && have("systemd-run") && have("apt-get"))
}

/// `GET /system/update` (`refresh`: check the release index now).
pub async fn check_all(state: &AppState, refresh: bool) -> UpdateInfo {
    use super::updates_orch as orch;
    let mut info = if !trusted_keys().is_empty() && !in_docker() {
        check_signed(state, refresh).await
    } else {
        check(state).await
    };
    info.run = orch::current_job(state);
    info.history = orch::history(state);
    info.previous = info
        .history
        .iter()
        .find(|h| h.ok && h.to == CURRENT)
        .map(|h| h.from.clone());
    let fleet = orch::ClusterFleet {
        state: state.clone(),
        scope: orch::Scope::Cluster,
    };
    let status = state
        .services
        .cluster
        .get()
        .map(|c| c.nodes_status())
        .unwrap_or_default();
    use orch::Fleet;
    info.nodes = fleet
        .nodes()
        .into_iter()
        .map(|n| NodeVersion {
            proto: status.iter().find(|s| s.id == n.id).map(|s| s.protocol),
            id: n.id,
            name: n.name,
            version: n.version,
            can_apply: n.can_apply,
            online: n.online,
            arch: n.arch,
            phase: n.phase,
        })
        .collect();
    info
}

async fn check_signed(state: &AppState, refresh: bool) -> UpdateInfo {
    use super::updates_orch as orch;
    let channel = match state.store.get().settings.updates.channel {
        UpdateChannel::Stable => "stable",
        UpdateChannel::Beta => "beta",
    };
    let mut info = UpdateInfo {
        current: CURRENT.into(),
        latest: CURRENT.into(),
        available: false,
        can_apply: false,
        notes: None,
        channel: Some(channel.into()),
        message: None,
        job: None,
        ota: true,
        nodes: vec![],
        problems: vec![],
        run: None,
        history: vec![],
        previous: None,
    };
    match orch::latest(state, refresh).await {
        Ok(idx) => {
            info.available = compare_versions(&idx.version, CURRENT) == std::cmp::Ordering::Greater;
            info.latest = idx.version.clone();
            info.notes = idx.notes.clone();
            info.can_apply = info.available && orch::ota_available();
            if info.available {
                info.problems = orch::plan(state, orch::Scope::Cluster, &idx).await;
            }
            if info.available && !orch::ota_available() {
                info.message = Some(
                    "An update is available, but this PixelPlus can't install it by itself (not a PixelPlus Pi image or package).".into(),
                );
            }
        }
        Err(e) => info.message = Some(e),
    }
    info
}

pub async fn check(state: &AppState) -> UpdateInfo {
    let mut info = UpdateInfo {
        current: CURRENT.into(),
        latest: CURRENT.into(),
        available: false,
        can_apply: false,
        notes: None,
        channel: None,
        message: None,
        job: state.services.helpers.get("update"),
        ota: false,
        nodes: vec![],
        problems: vec![],
        run: None,
        history: vec![],
        previous: None,
    };
    if in_docker() {
        info.message = Some(
            "PixelPlus runs in Docker here. To update, pull the new image and recreate the container (docker compose pull && docker compose up -d).".into(),
        );
        return info;
    }
    if !have("apt-cache") {
        info.message = Some("Updates are installed with the PixelPlus Imager or your package manager on this computer.".into());
        return info;
    }
    if platform::helper_installed() {
        // PixelPlus images turn apt's daily timers off, so nothing else refreshes
        // the package lists `apt-cache policy` reads: without this an update would
        // never show up. At most every few hours, through the root helper, waiting
        // a little for it (offline: the check just uses the lists it has).
        refresh_index_via_helper(state).await;
    } else if is_root() {
        // Development machine running as root: refresh the index in the background
        // (at most every few hours; ignore failures: offline) so opening Settings
        // never waits for `apt-get update`.
        refresh_index_in_background();
    }
    match run(
        "apt-cache",
        &["policy", "pixelplus"],
        Duration::from_secs(20),
    )
    .await
    {
        Ok(o) if o.success => {
            let (installed, candidate, channel) = parse_policy(&o.stdout);
            let installed = installed.unwrap_or_else(|| CURRENT.into());
            info.current = installed.clone();
            info.channel = channel;
            if let Some(c) = candidate {
                info.available = c != installed;
                info.latest = c;
            } else {
                info.latest = installed;
                info.message =
                    Some("The PixelPlus package repository isn't set up on this computer.".into());
            }
            info.can_apply = info.available && can_install();
            if info.available && !info.can_apply {
                info.message = Some("An update is available. Install it with: sudo apt install --only-upgrade pixelplus".into());
            }
        }
        _ => {
            info.message =
                Some("Couldn't check for updates right now. Is the internet connected?".into())
        }
    }
    info
}

/// How often a root development install refreshes the apt index.
const INDEX_REFRESH: Duration = Duration::from_secs(6 * 3600);

/// Whether the index refresh is due (`last` = previous refresh of this process).
fn index_refresh_due(last: Option<std::time::Instant>, now: std::time::Instant) -> bool {
    last.map_or(true, |t| now.duration_since(t) >= INDEX_REFRESH)
}

/// Last package-list refresh started through the helper (this daemon run).
static HELPER_REFRESH: parking_lot::Mutex<Option<std::time::Instant>> =
    parking_lot::Mutex::new(None);

async fn refresh_index_via_helper(state: &AppState) {
    let now = std::time::Instant::now();
    {
        let mut last = HELPER_REFRESH.lock();
        if !index_refresh_due(*last, now) {
            return;
        }
        *last = Some(now);
    }
    let quiet = HelperOpts { quiet: true };
    match platform::run_helper(state, HelperVerb::RefreshIndex, quiet).await {
        Ok(job) => {
            let done = job.wait(Duration::from_secs(45)).await;
            if done.state != platform::HelperState::Ok {
                tracing::info!("update check: {}", done.message);
            }
        }
        Err(e) => tracing::info!("update check: {}", e.message),
    }
}

fn refresh_index_in_background() {
    static LAST: parking_lot::Mutex<Option<std::time::Instant>> = parking_lot::Mutex::new(None);
    let now = std::time::Instant::now();
    {
        let mut last = LAST.lock();
        if !index_refresh_due(*last, now) {
            return;
        }
        *last = Some(now);
    }
    tokio::spawn(async {
        let _ = run("apt-get", &["update", "-qq"], Duration::from_secs(90)).await;
    });
}

/// Start the upgrade through the root helper (it restarts pixelplusd when done).
pub async fn apply(state: &AppState) -> ApiResult<String> {
    if in_docker() {
        return Err(ApiError::forbidden(
            "PixelPlus runs in Docker here. Update by pulling the new container image.",
        ));
    }
    if !can_install() {
        return Err(ApiError::forbidden(
            "This PixelPlus can't update itself. Run: sudo apt install --only-upgrade pixelplus",
        ));
    }
    platform::run_helper(state, HelperVerb::Update, HelperOpts::default()).await?;
    Ok(
        "Installing the update. PixelPlus will restart by itself when it's done (a minute or two)."
            .into(),
    )
}

// ===========================================================================
// F15: signed over-the-air updates (ARCHITECTURE §12.13)
// ===========================================================================
//
// A release is described by a signed index per channel,
// `<base>/pixelplus-<channel>.json` + `.minisig`, listing one `.deb` per
// architecture with its size and SHA-256; every `.deb` also has its own
// `.minisig`. Signatures are minisign (Ed25519) by a key listed in
// `packaging/keys/pixelplus-release.pub` (compiled in, and installed to
// `/usr/share/pixelplus/keys/`). The root helper verifies again with the
// installed keys before `dpkg -i`, so a compromised daemon user can't install
// unsigned code; followers verify what their leader serves them the same way.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Keys compiled into this build.
const BUILTIN_KEYS: &str = include_str!("../../../../packaging/keys/pixelplus-release.pub");
/// Keys installed by the package (root-owned).
pub const KEYS_DIR: &str = "/usr/share/pixelplus/keys";
/// Default location of the release indexes (`PIXELPLUS_UPDATE_URL` overrides).
pub const DEFAULT_INDEX_BASE: &str = "https://tlchandler.github.io/PixelPlus/ota";
/// Largest package accepted.
pub const MAX_PACKAGE: u64 = 512 * 1024 * 1024;
const MAX_INDEX: usize = 256 * 1024;

/// One package of a release.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseFile {
    /// Debian architecture: `arm64` | `amd64`.
    pub arch: String,
    /// `pixelplus_<version>_<arch>.deb`.
    pub name: String,
    pub size: u64,
    /// Hex SHA-256 of the `.deb`.
    pub sha256: String,
    /// Download URL of the `.deb` (its signature is at `<url>.minisig`).
    pub url: String,
}

/// A channel's release index (`pixelplus-<channel>.json`).
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseIndex {
    pub v: u32,
    pub channel: String,
    pub version: String,
    #[serde(default)]
    pub date: Option<String>,
    /// Release notes (plain text / light markdown).
    #[serde(default)]
    pub notes: Option<String>,
    /// Cluster protocol range of this release (beacon `protoMin/protoMax`).
    #[serde(default = "proto_default")]
    pub proto_min: u32,
    #[serde(default = "proto_default")]
    pub proto_max: u32,
    /// `Show.formatVersion` this release writes.
    #[serde(default = "one")]
    pub format_version: u32,
    pub files: Vec<ReleaseFile>,
}

fn proto_default() -> u32 {
    crate::cluster::proto::PROTOCOL_VERSION
}
fn one() -> u32 {
    1
}

impl ReleaseIndex {
    pub fn file_for(&self, arch: &str) -> Option<&ReleaseFile> {
        self.files.iter().find(|f| f.arch == arch)
    }

    /// Structural checks (the signature was checked before parsing).
    pub fn validate(&self) -> Result<(), String> {
        if self.v != 1 {
            return Err(
                "The update index has a newer format; update PixelPlus by hand once.".into(),
            );
        }
        if !valid_version(&self.version) {
            return Err("The update index names an invalid version.".into());
        }
        for f in &self.files {
            let expected = format!("pixelplus_{}_{}.deb", self.version, f.arch);
            if f.name != expected
                || !valid_package_name(&f.name)
                || f.sha256.len() != 64
                || !f.sha256.chars().all(|c| c.is_ascii_hexdigit())
                || f.size == 0
                || f.size > MAX_PACKAGE
                || !(f.url.starts_with("https://") || f.url.starts_with("http://127.0.0.1:"))
            {
                return Err(format!("The update index entry for {} is invalid.", f.arch));
            }
        }
        Ok(())
    }
}

/// This build's Debian architecture.
pub fn deb_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86_64") {
        "amd64"
    } else {
        "unsupported"
    }
}

/// A Debian version PixelPlus produces (`1.2.3`, `1.3.0~beta1`, `0.1.0~git20260930.abc123`).
pub fn valid_version(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 64
        && v.starts_with(|c: char| c.is_ascii_digit())
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '~' | '-'))
}

/// `pixelplus_<version>_<arm64|amd64>.deb` (optionally `.minisig`).
pub fn valid_package_name(name: &str) -> bool {
    let base = name.strip_suffix(".minisig").unwrap_or(name);
    let Some(rest) = base
        .strip_prefix("pixelplus_")
        .and_then(|r| r.strip_suffix(".deb"))
    else {
        return false;
    };
    let Some((ver, arch)) = rest.rsplit_once('_') else {
        return false;
    };
    valid_version(ver) && matches!(arch, "arm64" | "amd64")
}

/// Compare Debian versions (dpkg's algorithm, without epochs: PixelPlus
/// versions have none). `~` sorts before everything, even the end.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    fn order(c: Option<char>) -> i32 {
        match c {
            None => 0,
            Some('~') => -1,
            Some(c) if c.is_ascii_digit() => 0,
            Some(c) if c.is_ascii_alphabetic() => c as i32,
            Some(c) => c as i32 + 256,
        }
    }
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        // Non-digit prefix.
        loop {
            let (ca, cb) = (a.peek().copied(), b.peek().copied());
            let da = ca.map_or(true, |c| c.is_ascii_digit());
            let db = cb.map_or(true, |c| c.is_ascii_digit());
            if da && db {
                break;
            }
            let (oa, ob) = (
                if da { 0 } else { order(ca) },
                if db { 0 } else { order(cb) },
            );
            if oa != ob {
                return oa.cmp(&ob);
            }
            if !da {
                a.next();
            }
            if !db {
                b.next();
            }
        }
        // Digit run.
        let num = |it: &mut std::iter::Peekable<std::str::Chars>| {
            let mut n: u128 = 0;
            while let Some(c) = it.peek().copied().filter(|c| c.is_ascii_digit()) {
                n = n.saturating_mul(10).saturating_add(c as u128 - '0' as u128);
                it.next();
            }
            n
        };
        let (na, nb) = (num(&mut a), num(&mut b));
        if na != nb {
            return na.cmp(&nb);
        }
        if a.peek().is_none() && b.peek().is_none() {
            return Ordering::Equal;
        }
    }
}

/// Public keys from a `.pub`-style text (lines starting with `RW`; comments
/// and the `untrusted comment:` line are skipped).
pub fn parse_keys(text: &str) -> Vec<minisign_verify::PublicKey> {
    text.lines()
        .map(str::trim)
        .filter(|l| l.starts_with("RW"))
        .filter_map(|l| minisign_verify::PublicKey::from_base64(l).ok())
        .collect()
}

/// Every key this daemon trusts for updates: the compiled-in list plus the
/// keys installed in [`KEYS_DIR`] (root-owned).
pub fn trusted_keys() -> Vec<minisign_verify::PublicKey> {
    let mut keys = parse_keys(BUILTIN_KEYS);
    if let Ok(entries) = std::fs::read_dir(KEYS_DIR) {
        for e in entries.flatten() {
            if e.path().extension().is_some_and(|x| x == "pub") {
                if let Ok(t) = std::fs::read_to_string(e.path()) {
                    for k in parse_keys(&t) {
                        if !keys.contains(&k) {
                            keys.push(k);
                        }
                    }
                }
            }
        }
    }
    keys
}

/// Check a minisign signature over `data` with any of `keys`.
pub fn verify_bytes(
    keys: &[minisign_verify::PublicKey],
    data: &[u8],
    sig_text: &str,
) -> Result<(), String> {
    let sig = minisign_verify::Signature::decode(sig_text)
        .map_err(|_| "The signature file is damaged.".to_string())?;
    if keys.is_empty() {
        return Err("This PixelPlus build has no update signing key.".into());
    }
    if keys.iter().any(|k| k.verify(data, &sig, true).is_ok()) {
        Ok(())
    } else {
        Err("The signature doesn't match (not signed by a PixelPlus release key).".into())
    }
}

/// Check a minisign signature over a file (streamed for pre-hashed
/// signatures, the minisign default).
pub fn verify_file(
    keys: &[minisign_verify::PublicKey],
    path: &Path,
    sig_text: &str,
) -> Result<(), String> {
    use std::io::Read;
    let sig = minisign_verify::Signature::decode(sig_text)
        .map_err(|_| "The signature file is damaged.".to_string())?;
    if keys.is_empty() {
        return Err("This PixelPlus build has no update signing key.".into());
    }
    for k in keys {
        match k.verify_stream(&sig) {
            Ok(mut v) => {
                let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
                let mut buf = vec![0u8; 256 * 1024];
                loop {
                    let n = f.read(&mut buf).map_err(|e| e.to_string())?;
                    if n == 0 {
                        break;
                    }
                    v.update(&buf[..n]);
                }
                if v.finalize().is_ok() {
                    return Ok(());
                }
            }
            Err(minisign_verify::Error::UnsupportedLegacyMode) => {
                let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
                if len > MAX_PACKAGE {
                    return Err("That package is too large.".into());
                }
                let data = std::fs::read(path).map_err(|e| e.to_string())?;
                if k.verify(&data, &sig, true).is_ok() {
                    return Ok(());
                }
            }
            Err(_) => {} // another key id
        }
    }
    Err("The package signature doesn't match (not signed by a PixelPlus release key).".into())
}

/// SHA-256 of a file, hex.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Base URL of the release indexes.
pub fn index_base() -> String {
    std::env::var("PIXELPLUS_UPDATE_URL")
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| v.starts_with("https://") || v.starts_with("http://127.0.0.1"))
        .unwrap_or_else(|| DEFAULT_INDEX_BASE.to_string())
}

/// Where downloaded / leader-served packages live (daemon-owned).
pub fn packages_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("updates/packages")
}

/// Where a package waits for the root helper (`update-stage`).
pub fn incoming_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("updates/incoming")
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(60))
        .user_agent(concat!("pixelplusd/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

async fn get_limited(client: &reqwest::Client, url: &str, max: usize) -> Result<Vec<u8>, String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("couldn't reach {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("{url}: HTTP {}", resp.status()));
    }
    if resp.content_length().is_some_and(|l| l > max as u64) {
        return Err(format!("{url} is too large"));
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    if bytes.len() > max {
        return Err(format!("{url} is too large"));
    }
    Ok(bytes.to_vec())
}

/// Fetch and verify the release index of `channel` from `base`.
pub async fn fetch_index(
    base: &str,
    channel: &str,
    keys: &[minisign_verify::PublicKey],
) -> Result<ReleaseIndex, String> {
    let client = http_client()?;
    let url = format!("{base}/pixelplus-{channel}.json");
    let body = get_limited(&client, &url, MAX_INDEX).await?;
    let sig = get_limited(&client, &format!("{url}.minisig"), 4096).await?;
    verify_bytes(keys, &body, &String::from_utf8_lossy(&sig))
        .map_err(|e| format!("The update index isn't genuine: {e}"))?;
    let index: ReleaseIndex = serde_json::from_slice(&body)
        .map_err(|e| format!("The update index can't be read: {e}"))?;
    if index.channel != channel {
        return Err("The update index is for another channel.".into());
    }
    index.validate()?;
    Ok(index)
}

/// Make sure `file` (from a verified index) is in `dir`, downloaded,
/// size- and hash-checked and its own signature verified. Returns its path.
pub async fn ensure_package(
    dir: &Path,
    file: &ReleaseFile,
    keys: &[minisign_verify::PublicKey],
) -> Result<PathBuf, String> {
    use tokio::io::AsyncWriteExt;
    if !valid_package_name(&file.name) {
        return Err("invalid package name".into());
    }
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| e.to_string())?;
    let path = dir.join(&file.name);
    let sig_path = dir.join(format!("{}.minisig", file.name));
    let ok = |p: PathBuf| {
        let (p2, sigp, want, keys) = (
            p.clone(),
            sig_path.clone(),
            file.sha256.clone(),
            keys.to_vec(),
        );
        async move {
            tokio::task::spawn_blocking(move || -> Result<(), String> {
                let sig = std::fs::read_to_string(&sigp).map_err(|e| e.to_string())?;
                let got = sha256_file(&p2).map_err(|e| e.to_string())?;
                if !got.eq_ignore_ascii_case(&want) {
                    return Err("The downloaded package is damaged (checksum).".into());
                }
                verify_file(&keys, &p2, &sig)
            })
            .await
            .map_err(|e| e.to_string())?
        }
    };
    if path.exists() && sig_path.exists() && ok(path.clone()).await.is_ok() {
        return Ok(path);
    }
    if let Some((free, _)) = super::system::disk_space(dir) {
        if free < file.size.saturating_mul(3) + 256 * 1024 * 1024 {
            return Err(format!(
                "Not enough free space for the update ({} MB needed).",
                (file.size * 3) / 1_000_000 + 256
            ));
        }
    }
    let client = http_client()?;
    let sig = get_limited(&client, &format!("{}.minisig", file.url), 4096).await?;
    tokio::fs::write(&sig_path, &sig)
        .await
        .map_err(|e| e.to_string())?;
    let part = dir.join(format!("{}.part", file.name));
    let mut resp = client
        .get(&file.url)
        .send()
        .await
        .map_err(|e| format!("download failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("download failed: HTTP {}", resp.status()));
    }
    let mut out = tokio::fs::File::create(&part)
        .await
        .map_err(|e| e.to_string())?;
    let mut total: u64 = 0;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("download failed: {e}"))?
    {
        total += chunk.len() as u64;
        if total > file.size {
            let _ = tokio::fs::remove_file(&part).await;
            return Err("The package is larger than announced.".into());
        }
        out.write_all(&chunk).await.map_err(|e| e.to_string())?;
    }
    out.sync_all().await.map_err(|e| e.to_string())?;
    drop(out);
    if total != file.size {
        let _ = tokio::fs::remove_file(&part).await;
        return Err("The download was incomplete.".into());
    }
    if let Err(e) = ok(part.clone()).await {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }
    tokio::fs::rename(&part, &path)
        .await
        .map_err(|e| e.to_string())?;
    Ok(path)
}

/// Keep only the packages of `keep` versions in `dir`.
pub fn prune_packages(dir: &Path, keep: &[String]) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let base = name.strip_suffix(".minisig").unwrap_or(&name);
        let base = base.strip_suffix(".part").unwrap_or(base);
        let ver = base
            .strip_prefix("pixelplus_")
            .and_then(|r| r.rsplit_once('_'))
            .map(|(v, _)| v.to_string());
        if ver.map_or(true, |v| !keep.contains(&v)) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    #[test]
    fn index_refresh_is_throttled() {
        use super::*;
        let now = std::time::Instant::now();
        assert!(index_refresh_due(None, now));
        assert!(!index_refresh_due(Some(now), now + Duration::from_secs(60)));
        assert!(index_refresh_due(Some(now), now + INDEX_REFRESH));
    }

    use super::*;

    #[test]
    fn policy_parsing() {
        let t = "pixelplus:\n  Installed: 0.9.0\n  Candidate: 0.9.2\n  Version table:\n     0.9.2 500\n        500 https://apt.pixelplus.dev stable/main arm64 Packages\n *** 0.9.0 100\n        100 /var/lib/dpkg/status\n";
        let (i, c, _) = parse_policy(t);
        assert_eq!(i.as_deref(), Some("0.9.0"));
        assert_eq!(c.as_deref(), Some("0.9.2"));
        let (i, c, _) = parse_policy("pixelplus:\n  Installed: (none)\n  Candidate: 1.0\n");
        assert_eq!(i, None);
        assert_eq!(c.as_deref(), Some("1.0"));
    }

    #[test]
    fn debian_version_order() {
        use std::cmp::Ordering::*;
        for (a, b, o) in [
            ("1.0.0", "1.0.0", Equal),
            ("1.0.0", "1.0.1", Less),
            ("1.10.0", "1.9.9", Greater),
            ("1.3.0~beta1", "1.3.0", Less),
            ("1.3.0~beta1", "1.3.0~beta2", Less),
            ("1.3.0", "1.3.0+local1", Less),
            ("0.1.0~git20260930.abc", "0.1.0", Less),
            ("2.0", "10.0", Less),
        ] {
            assert_eq!(compare_versions(a, b), o, "{a} vs {b}");
            assert_eq!(compare_versions(b, a), o.reverse(), "{b} vs {a}");
        }
    }

    #[test]
    fn package_names() {
        assert!(valid_package_name("pixelplus_1.2.3_arm64.deb"));
        assert!(valid_package_name(
            "pixelplus_1.3.0~beta1_amd64.deb.minisig"
        ));
        assert!(!valid_package_name("pixelplus_1.2.3_armhf.deb"));
        assert!(!valid_package_name("pixelplus_../x_arm64.deb"));
        assert!(!valid_package_name("other_1.0_arm64.deb"));
        assert!(!valid_package_name("pixelplus_1.0_arm64.deb.sh"));
    }

    /// Minisign test signer (legacy Ed25519 mode; the verifier also accepts
    /// the pre-hashed mode `minisign` uses by default, see the fixture test).
    pub struct TestSigner {
        pair: ring::signature::Ed25519KeyPair,
        pub key_id: [u8; 8],
    }

    fn b64(data: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for c in data.chunks(3) {
            let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
            let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
            for i in 0..4 {
                out.push(if i <= c.len() {
                    T[(n >> (18 - 6 * i) & 63) as usize] as char
                } else {
                    '='
                });
            }
        }
        out
    }

    impl TestSigner {
        pub fn new(seed: u8) -> Self {
            let pair = ring::signature::Ed25519KeyPair::from_seed_unchecked(&[seed; 32]).unwrap();
            TestSigner {
                pair,
                key_id: [seed; 8],
            }
        }
        pub fn public(&self) -> String {
            use ring::signature::KeyPair;
            let mut bin = b"Ed".to_vec();
            bin.extend_from_slice(&self.key_id);
            bin.extend_from_slice(self.pair.public_key().as_ref());
            b64(&bin)
        }
        pub fn keys(&self) -> Vec<minisign_verify::PublicKey> {
            parse_keys(&self.public())
        }
        pub fn sign(&self, data: &[u8]) -> String {
            let sig = self.pair.sign(data);
            let mut bin = b"Ed".to_vec();
            bin.extend_from_slice(&self.key_id);
            bin.extend_from_slice(sig.as_ref());
            let trusted = "timestamp:0\tfile:test";
            let mut global = sig.as_ref().to_vec();
            global.extend_from_slice(trusted.as_bytes());
            let gsig = self.pair.sign(&global);
            format!(
                "untrusted comment: signature from test key\n{}\ntrusted comment: {trusted}\n{}\n",
                b64(&bin),
                b64(gsig.as_ref())
            )
        }
    }

    #[test]
    fn signatures_good_bad_and_wrong_key() {
        let signer = TestSigner::new(7);
        let other = TestSigner::new(9);
        let data = b"pixelplus 1.2.3";
        let sig = signer.sign(data);
        assert!(verify_bytes(&signer.keys(), data, &sig).is_ok());
        assert!(verify_bytes(&signer.keys(), b"pixelplus 6.6.6", &sig).is_err());
        assert!(verify_bytes(&other.keys(), data, &sig).is_err());
        assert!(verify_bytes(&[], data, &sig).is_err());
        assert!(verify_bytes(&signer.keys(), data, "garbage").is_err());
        // Several trusted keys (rotation): any one will do.
        let mut both = other.keys();
        both.extend(signer.keys());
        assert!(verify_bytes(&both, data, &sig).is_ok());
        // Files (legacy mode reads them whole).
        let dir = std::env::temp_dir().join(format!("pp-sig-{}", pixelplus_core::model::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("pixelplus_1.2.3_arm64.deb");
        std::fs::write(&f, data).unwrap();
        assert!(verify_file(&signer.keys(), &f, &sig).is_ok());
        std::fs::write(&f, b"tampered").unwrap();
        assert!(verify_file(&signer.keys(), &f, &sig).is_err());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn prehashed_minisign_fixture_verifies() {
        // Made with the reference format (BLAKE2b-512 pre-hash, "ED"), see
        // packaging/tests/fixtures/README.md.
        let fx = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../packaging/tests/fixtures/minisign"
        );
        let keys = parse_keys(&std::fs::read_to_string(format!("{fx}/test.pub")).unwrap());
        assert_eq!(keys.len(), 1);
        let file = std::path::PathBuf::from(format!("{fx}/pixelplus_9.9.9_arm64.deb"));
        let sig =
            std::fs::read_to_string(format!("{fx}/pixelplus_9.9.9_arm64.deb.minisig")).unwrap();
        assert!(verify_file(&keys, &file, &sig).is_ok());
        assert!(verify_bytes(&keys, &std::fs::read(&file).unwrap(), &sig).is_ok());
        assert!(verify_bytes(&keys, b"other bytes", &sig).is_err());
    }

    #[test]
    fn builtin_keys_parse() {
        // The placeholder file (no key yet) or real keys: never a panic, and
        // every "RW" line must be a valid key.
        let lines = BUILTIN_KEYS
            .lines()
            .filter(|l| l.trim().starts_with("RW"))
            .count();
        assert_eq!(parse_keys(BUILTIN_KEYS).len(), lines);
    }

    #[test]
    fn index_validation() {
        let good = ReleaseIndex {
            v: 1,
            channel: "stable".into(),
            version: "1.2.3".into(),
            date: None,
            notes: None,
            proto_min: 2,
            proto_max: 2,
            format_version: 1,
            files: vec![ReleaseFile {
                arch: "arm64".into(),
                name: "pixelplus_1.2.3_arm64.deb".into(),
                size: 10,
                sha256: "a".repeat(64),
                url: "https://example.com/pixelplus_1.2.3_arm64.deb".into(),
            }],
        };
        assert!(good.validate().is_ok());
        let mut bad = good.clone();
        bad.files[0].name = "pixelplus_1.2.4_arm64.deb".into();
        assert!(bad.validate().is_err());
        let mut bad = good.clone();
        bad.files[0].url = "http://example.com/x.deb".into();
        assert!(bad.validate().is_err());
        let mut bad = good.clone();
        bad.version = "1.2.3; rm -rf".into();
        assert!(bad.validate().is_err());
        assert_eq!(good.file_for("arm64").unwrap().size, 10);
        assert!(good.file_for("amd64").is_none());
    }

    #[tokio::test]
    async fn fetch_verify_and_download_from_a_release_server() {
        use axum::routing::get;
        let fx = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../packaging/tests/fixtures"
        );
        let deb = std::fs::read(format!("{fx}/minisign/pixelplus_9.9.9_arm64.deb")).unwrap();
        let deb_sig =
            std::fs::read_to_string(format!("{fx}/minisign/pixelplus_9.9.9_arm64.deb.minisig"))
                .unwrap();
        // The committed index (made by packaging/release-index.py) parses and validates.
        let committed: ReleaseIndex =
            serde_json::from_slice(&std::fs::read(format!("{fx}/pixelplus-stable.json")).unwrap())
                .unwrap();
        committed.validate().unwrap();
        assert_eq!(committed.files[0].size, deb.len() as u64);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let signer = TestSigner::new(3);
        let mut index = committed.clone();
        index.files[0].url = format!("http://127.0.0.1:{port}/files/pixelplus_9.9.9_arm64.deb");
        let index_json = serde_json::to_vec(&index).unwrap();
        let index_sig = signer.sign(&index_json);
        let bad_sig = TestSigner::new(4).sign(&index_json);
        let evil = Arc::new(parking_lot::Mutex::new(false));
        let e2 = evil.clone();
        let (ij, deb2) = (index_json.clone(), deb.clone());
        let app = axum::Router::new()
            .route(
                "/ota/pixelplus-stable.json",
                get(move || {
                    let v = ij.clone();
                    async move { v }
                }),
            )
            .route(
                "/ota/pixelplus-stable.json.minisig",
                get(move || {
                    let s = if *e2.lock() {
                        bad_sig.clone()
                    } else {
                        index_sig.clone()
                    };
                    async move { s }
                }),
            )
            .route(
                "/files/pixelplus_9.9.9_arm64.deb",
                get(move || {
                    let v = deb2.clone();
                    async move { v }
                }),
            )
            .route(
                "/files/pixelplus_9.9.9_arm64.deb.minisig",
                get(move || {
                    let v = deb_sig.clone();
                    async move { v }
                }),
            );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let base = format!("http://127.0.0.1:{port}/ota");
        let mut keys = signer.keys();
        keys.extend(parse_keys(
            &std::fs::read_to_string(format!("{fx}/minisign/test.pub")).unwrap(),
        ));

        let got = fetch_index(&base, "stable", &keys).await.unwrap();
        assert_eq!(got.version, "9.9.9");
        assert!(
            fetch_index(&base, "beta", &keys).await.is_err(),
            "no such channel"
        );
        // An index signed by anyone else is refused.
        *evil.lock() = true;
        let e = fetch_index(&base, "stable", &keys).await.unwrap_err();
        assert!(e.contains("isn't genuine"), "{e}");
        *evil.lock() = false;

        let dir = std::env::temp_dir().join(format!("pp-pkg-{}", pixelplus_core::model::new_id()));
        let p = ensure_package(&dir, &got.files[0], &keys).await.unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), deb);
        // Cached next time; a wrong hash in the index is caught.
        assert_eq!(ensure_package(&dir, &got.files[0], &keys).await.unwrap(), p);
        let mut wrong = got.files[0].clone();
        wrong.sha256 = "0".repeat(64);
        std::fs::remove_file(&p).unwrap();
        assert!(ensure_package(&dir, &wrong, &keys).await.is_err());
        assert!(!p.exists());
        // Signed by a key we don't trust: refused.
        assert!(ensure_package(&dir, &got.files[0], &signer.keys())
            .await
            .is_err());
        std::fs::remove_dir_all(dir).ok();
    }
}
