//! Privileged operations for the unprivileged `pixelplus` service user.
//!
//! On a PixelPlus Pi, `pixelplusd` runs as the system user `pixelplus`
//! (packaging/systemd/pixelplusd.service). What it may do beyond its own data is
//! fixed by packaging/polkit/50-pixelplus.rules:
//!
//! | operation                         | route                                             |
//! |-----------------------------------|---------------------------------------------------|
//! | Wi-Fi, hotspot, Ethernet          | NetworkManager (`nmcli`)                          |
//! | hostname                          | hostnamed (`hostnamectl --static --transient`)    |
//! | time zone                         | timedated (`timedatectl set-timezone`)            |
//! | reboot / power off                | logind (`systemctl reboot|poweroff`)              |
//! | restart pixelplusd                | `systemctl restart pixelplusd.service`            |
//! | boot config, apt update, SSH, ... | root helper `pixelplus-helper@<verb>.service`     |
//!
//! The helper (packaging/bin/pixelplus-helper) is started with
//! `systemctl start --no-block pixelplus-helper@<verb>.service` and reports
//! progress in `/run/pixelplus/helper-<verb>.json`
//! (`{verb, state: running|ok|failed, message, updatedAt}`), which we poll and
//! forward to the UI as `helper` WebSocket messages and toasts.
//!
//! Running as root (development machine, a hand-started daemon) without the
//! helper installed, the same operations are done directly where that is
//! safe; in Docker or on a PC without the PixelPlus package the API answers
//! with a friendly explanation instead.

use super::system::{has_systemd, have, in_docker, is_root, run};
use crate::api::{ApiError, ApiResult};
use crate::events::ToastKind;
use crate::state::AppState;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The root helper script (installed by the pixelplus package).
pub const HELPER_BIN: &str = "/usr/lib/pixelplus/pixelplus-helper";
/// pixelplus.txt applier / board-config writer (installed by the pixelplus package).
pub const FIRSTBOOT: &str = "/usr/lib/pixelplus/firstboot/firstboot.py";
/// Where the polkit rules may live (package: /usr/share, admin override: /etc).
const POLKIT_RULES: [&str; 2] = [
    "/usr/share/polkit-1/rules.d/50-pixelplus.rules",
    "/etc/polkit-1/rules.d/50-pixelplus.rules",
];

/// Shared runtime directory (tmpfiles.d: `/run/pixelplus`), overridable for tests.
pub fn run_dir() -> PathBuf {
    std::env::var_os("PIXELPLUS_RUN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/run/pixelplus"))
}

/// Record the board we run in `/run/pixelplus/board`, so `pixelplus pins release`
/// (pixelplusd.service `ExecStopPost=`) parks the right pins even when the board
/// was chosen in the setup wizard and its EEPROM is blank. Best effort.
pub fn publish_board(state: &AppState) {
    let (board, _) = super::system::effective_board(state);
    let Some(id) = serde_json::to_value(board)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
    else {
        return;
    };
    let dir = run_dir();
    if !dir.is_dir() {
        return;
    }
    let tmp = dir.join(format!(".board.{}", std::process::id()));
    let res = std::fs::write(&tmp, format!("{id}\n"))
        .and_then(|_| std::fs::rename(&tmp, dir.join("board")));
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        tracing::debug!("couldn't write {}/board: {e}", dir.display());
    }
}

/// The pixelplus polkit rules are installed (so the service user may reboot, set
/// the hostname/time zone and start the helper). The rules directories are often
/// unreadable for other users (Debian: 0700 polkitd), so the package's helper
/// script - installed by the same package - counts as proof too.
pub fn polkit_rules_installed() -> bool {
    Path::new(HELPER_BIN).is_file() || POLKIT_RULES.iter().any(|p| Path::new(p).is_file())
}

/// `pixelplus-helper@.service` can be started from here.
pub fn helper_installed() -> bool {
    cfg!(target_os = "linux")
        && !in_docker()
        && has_systemd()
        && have("systemctl")
        && Path::new(HELPER_BIN).is_file()
}

/// systemd/logind can reboot, power off and restart us from here.
pub fn can_control_power() -> bool {
    cfg!(target_os = "linux")
        && !in_docker()
        && has_systemd()
        && have("systemctl")
        && (is_root() || polkit_rules_installed())
}

// ---------------------------------------------------------------------------
// Helper verbs
// ---------------------------------------------------------------------------

/// A whitelisted helper verb (see packaging/bin/pixelplus-helper).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperVerb {
    /// Regenerate `/boot/firmware/pixelplus.conf` for a board (+ string length).
    ConfigTxt {
        board: String,
        pixels: Option<u32>,
    },
    /// apt-get update + upgrade the pixelplus package.
    Update,
    /// apt-get update only: refresh the package lists the update check reads
    /// (PixelPlus images turn apt's daily timers off).
    RefreshIndex,
    SshOn,
    SshOff,
    /// Re-apply `/boot/firmware/pixelplus.txt` now.
    Reapply,
    /// Set the Wi-Fi regulatory country (two letters).
    WifiCountry(String),
    /// Sync the `127.0.1.1` line of /etc/hosts with the current hostname.
    Hosts,
}

fn valid_token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

impl HelperVerb {
    /// The verb name: also the status file name (`helper-<name>.json`).
    pub fn name(&self) -> &'static str {
        match self {
            HelperVerb::ConfigTxt { .. } => "config-txt",
            HelperVerb::Update => "update",
            HelperVerb::RefreshIndex => "refresh-index",
            HelperVerb::SshOn => "ssh-on",
            HelperVerb::SshOff => "ssh-off",
            HelperVerb::Reapply => "reapply",
            HelperVerb::WifiCountry(_) => "wifi-country",
            HelperVerb::Hosts => "hosts",
        }
    }

    /// The systemd instance string (`<verb>[:arg1[:arg2]]`), validated.
    pub fn instance(&self) -> ApiResult<String> {
        let s = match self {
            HelperVerb::ConfigTxt { board, pixels } => {
                if !valid_token(board) {
                    return Err(ApiError::bad_request(format!(
                        "\"{board}\" isn't a board PixelPlus knows."
                    )));
                }
                match pixels {
                    Some(p) if *p == 0 || *p > 99_999 => {
                        return Err(ApiError::bad_request(
                            "The string length must be between 1 and 99999 pixels.",
                        ))
                    }
                    Some(p) => format!("config-txt:{board}:{p}"),
                    None => format!("config-txt:{board}"),
                }
            }
            HelperVerb::WifiCountry(cc) => {
                if cc.len() != 2 || !cc.chars().all(|c| c.is_ascii_alphabetic()) {
                    return Err(ApiError::bad_request(
                        "Pick your Wi-Fi country (a two-letter code like US or GB).",
                    ));
                }
                format!("wifi-country:{}", cc.to_ascii_uppercase())
            }
            other => other.name().to_string(),
        };
        Ok(s)
    }

    /// systemd unit name for this instance.
    pub fn unit(&self) -> ApiResult<String> {
        Ok(format!("pixelplus-helper@{}.service", self.instance()?))
    }

    /// Human description used in progress messages.
    fn describe(&self) -> String {
        match self {
            HelperVerb::ConfigTxt {
                board,
                pixels: Some(p),
            } => {
                format!("Writing the boot settings for {board} ({p} pixels per output)")
            }
            HelperVerb::ConfigTxt { board, .. } => format!("Writing the boot settings for {board}"),
            HelperVerb::Update => "Installing the update".into(),
            HelperVerb::RefreshIndex => "Checking for updates".into(),
            HelperVerb::SshOn => "Turning SSH on".into(),
            HelperVerb::SshOff => "Turning SSH off".into(),
            HelperVerb::Reapply => "Applying pixelplus.txt".into(),
            HelperVerb::WifiCountry(cc) => format!("Setting the Wi-Fi country to {cc}"),
            HelperVerb::Hosts => "Updating /etc/hosts".into(),
        }
    }

    /// How long the helper may take.
    fn timeout(&self) -> Duration {
        match self {
            HelperVerb::Update => Duration::from_secs(30 * 60),
            HelperVerb::Reapply => Duration::from_secs(5 * 60),
            _ => Duration::from_secs(120),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HelperState {
    Running,
    Ok,
    Failed,
}

/// `/run/pixelplus/helper-<verb>.json`, also sent as the `helper` WS message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HelperStatus {
    pub verb: String,
    pub state: HelperState,
    #[serde(default)]
    pub message: String,
    /// Unix seconds.
    #[serde(default)]
    pub updated_at: i64,
}

impl HelperStatus {
    fn new(verb: &str, state: HelperState, message: impl Into<String>) -> Self {
        HelperStatus {
            verb: verb.to_string(),
            state,
            message: message.into(),
            updated_at: chrono::Utc::now().timestamp(),
        }
    }
    pub fn done(&self) -> bool {
        self.state != HelperState::Running
    }
}

/// Status file of `verb` in `dir`.
pub fn status_path(dir: &Path, verb: &str) -> PathBuf {
    dir.join(format!("helper-{verb}.json"))
}

/// Read a helper status file (None when missing or unreadable).
pub fn read_status(dir: &Path, verb: &str) -> Option<HelperStatus> {
    let text = std::fs::read_to_string(status_path(dir, verb)).ok()?;
    serde_json::from_str(&text).ok()
}

/// In-memory view of helper jobs started by this daemon.
#[derive(Default)]
pub struct HelperJobs {
    jobs: Mutex<BTreeMap<String, (Instant, HelperStatus)>>,
}

impl HelperJobs {
    /// Latest status of every verb run since startup.
    pub fn all(&self) -> Vec<HelperStatus> {
        self.jobs.lock().values().map(|(_, s)| s.clone()).collect()
    }
    pub fn get(&self, verb: &str) -> Option<HelperStatus> {
        self.jobs.lock().get(verb).map(|(_, s)| s.clone())
    }
    fn set(&self, s: &HelperStatus) {
        let mut jobs = self.jobs.lock();
        let started = match jobs.get(&s.verb) {
            Some((at, old)) if !old.done() => *at,
            _ => Instant::now(),
        };
        jobs.insert(s.verb.clone(), (started, s.clone()));
    }
    /// Claim `verb` for a new run; false when one is still running.
    fn claim(&self, verb: &HelperVerb, initial: &HelperStatus) -> bool {
        let mut jobs = self.jobs.lock();
        if let Some((at, s)) = jobs.get(verb.name()) {
            if !s.done() && at.elapsed() < verb.timeout() {
                return false;
            }
        }
        jobs.insert(verb.name().to_string(), (Instant::now(), initial.clone()));
        true
    }
}

/// Options for [`run_helper`].
#[derive(Debug, Clone, Copy, Default)]
pub struct HelperOpts {
    /// No toasts (the caller reports the result itself).
    pub quiet: bool,
}

/// A started helper job: the first status, and the final one when it finishes.
pub struct HelperJob {
    pub status: HelperStatus,
    pub done: tokio::sync::oneshot::Receiver<HelperStatus>,
}

impl HelperJob {
    /// Wait for the final status (at most `timeout`).
    pub async fn wait(self, timeout: Duration) -> HelperStatus {
        let verb = self.status.verb.clone();
        match tokio::time::timeout(timeout, self.done).await {
            Ok(Ok(s)) => s,
            _ => HelperStatus::new(
                &verb,
                HelperState::Failed,
                "Timed out waiting for the helper",
            ),
        }
    }
}

fn publish(state: &AppState, s: &HelperStatus) {
    state.services.helpers.set(s);
    state.events.publish("helper", s);
}

fn finish(state: &AppState, s: &HelperStatus, opts: HelperOpts) {
    publish(state, s);
    match s.state {
        HelperState::Ok => {
            tracing::info!("helper {}: {}", s.verb, s.message);
            if !opts.quiet {
                state.events.toast(ToastKind::Success, s.message.clone());
            }
        }
        HelperState::Failed => {
            tracing::warn!("helper {} failed: {}", s.verb, s.message);
            if !opts.quiet {
                state.events.toast(ToastKind::Error, s.message.clone());
            }
        }
        HelperState::Running => {}
    }
}

/// Why a privileged operation can't be done here (user-presentable).
pub fn not_possible_here(what: &str) -> ApiError {
    if in_docker() {
        return ApiError::forbidden(format!(
            "PixelPlus is running in Docker, so it can't {what}. Do it on the host computer instead."
        ));
    }
    if !cfg!(target_os = "linux") || !has_systemd() {
        return ApiError::forbidden(format!(
            "PixelPlus can only {what} on a PixelPlus Pi. Do it in this computer's own settings."
        ));
    }
    ApiError::forbidden(format!(
        "PixelPlus isn't allowed to {what} here: the PixelPlus system package (helper and permissions) isn't installed. Reinstall the pixelplus package, or do it yourself with sudo."
    ))
}

fn friendly_start_error(unit: &str, stderr: &str) -> ApiError {
    let e = stderr.trim();
    if e.contains("Access denied")
        || e.contains("authentication")
        || e.contains("Permission denied")
    {
        return ApiError::forbidden(
            "PixelPlus wasn't allowed to start its system helper. The PixelPlus permissions (polkit rules) are missing; reinstall the pixelplus package.",
        );
    }
    if e.contains("not found") || e.contains("not loaded") {
        return ApiError::unavailable(format!(
            "The PixelPlus system helper ({unit}) isn't installed. Reinstall the pixelplus package."
        ));
    }
    ApiError::unavailable(format!("The PixelPlus system helper couldn't start: {e}"))
}

/// Start a helper verb and follow its progress in the background. Errors when
/// the helper can't be used here (see [`not_possible_here`]) or is already
/// running that verb.
pub async fn run_helper(
    state: &AppState,
    verb: HelperVerb,
    opts: HelperOpts,
) -> ApiResult<HelperJob> {
    verb.instance()?; // validate before anything else
    let name = verb.name();
    let initial = HelperStatus::new(name, HelperState::Running, format!("{}…", verb.describe()));
    let (tx, rx) = tokio::sync::oneshot::channel();

    if helper_installed() {
        if !state.services.helpers.claim(&verb, &initial) {
            return Err(ApiError::conflict(format!(
                "{} is already in progress.",
                verb.describe()
            )));
        }
        let unit = verb.unit()?;
        let since = chrono::Utc::now().timestamp() - 1;
        // The result of an earlier run (possibly within the same second) isn't ours.
        let baseline = read_status(&run_dir(), name);
        let out = run(
            "systemctl",
            &["--no-ask-password", "start", "--no-block", &unit],
            Duration::from_secs(15),
        )
        .await;
        let started = match out {
            Ok(o) if o.success => Ok(()),
            Ok(o) => Err(friendly_start_error(&unit, &o.stderr)),
            Err(e) => Err(ApiError::unavailable(e)),
        };
        if let Err(e) = started {
            let failed = HelperStatus::new(name, HelperState::Failed, e.message.clone());
            publish(state, &failed);
            return Err(e);
        }
        tracing::info!("started {unit}");
        publish(state, &initial);
        let st = state.clone();
        let dir = run_dir();
        let v = verb.clone();
        tokio::spawn(async move {
            let last = follow(
                &st,
                &dir,
                &v,
                &unit,
                since,
                baseline,
                Duration::from_secs(1),
            )
            .await;
            finish(&st, &last, opts);
            let _ = tx.send(last);
        });
        return Ok(HelperJob {
            status: initial,
            done: rx,
        });
    }

    if is_root() && !in_docker() && cfg!(target_os = "linux") {
        direct_precheck(&verb)?;
        if !state.services.helpers.claim(&verb, &initial) {
            return Err(ApiError::conflict(format!(
                "{} is already in progress.",
                verb.describe()
            )));
        }
        publish(state, &initial);
        let st = state.clone();
        let v = verb.clone();
        tokio::spawn(async move {
            let last = match direct(&v).await {
                Ok(msg) => HelperStatus::new(v.name(), HelperState::Ok, msg),
                Err(msg) => HelperStatus::new(v.name(), HelperState::Failed, msg),
            };
            finish(&st, &last, opts);
            let _ = tx.send(last);
        });
        return Ok(HelperJob {
            status: initial,
            done: rx,
        });
    }

    Err(not_possible_here(match verb {
        HelperVerb::ConfigTxt { .. } => "change the boot settings",
        HelperVerb::Update => "install updates",
        HelperVerb::RefreshIndex => "check for updates",
        HelperVerb::SshOn | HelperVerb::SshOff => "change SSH",
        HelperVerb::Reapply => "apply pixelplus.txt",
        HelperVerb::WifiCountry(_) => "set the Wi-Fi country",
        HelperVerb::Hosts => "update /etc/hosts",
    }))
}

/// Poll the helper's status file until it reports ok/failed (or times out).
/// Only files written after `since` (unix s) count: an old result from an
/// earlier run must not be mistaken for this one.
pub(crate) async fn follow(
    state: &AppState,
    dir: &Path,
    verb: &HelperVerb,
    unit: &str,
    since: i64,
    baseline: Option<HelperStatus>,
    every: Duration,
) -> HelperStatus {
    let name = verb.name();
    let deadline = Instant::now() + verb.timeout();
    let started = Instant::now();
    let mut last_seen: Option<HelperStatus> = None;
    let mut next_unit_check = Instant::now() + Duration::from_secs(10);
    loop {
        let current = read_status(dir, name).filter(|s| s.updated_at >= since);
        if let Some(s) = current.clone().filter(|s| Some(s) != baseline.as_ref()) {
            if s.done() {
                return s;
            }
            if last_seen.as_ref() != Some(&s) {
                publish(state, &s);
                last_seen = Some(s);
            }
        }
        if Instant::now() >= next_unit_check && !unit.is_empty() {
            // No result yet: did the unit die without writing one, or finish with a
            // result identical to the previous run's (same message, same second)?
            next_unit_check = Instant::now() + Duration::from_secs(5);
            let failed = run(
                "systemctl",
                &["is-failed", "--quiet", unit],
                Duration::from_secs(5),
            )
            .await
            .is_ok_and(|o| o.success);
            if failed {
                return HelperStatus::new(
                    name,
                    HelperState::Failed,
                    format!("{} failed. Details: journalctl -u {unit}", verb.describe()),
                );
            }
            // oneshot: "activating" while running, "inactive" once done ("failed" handled above)
            let finished = run(
                "systemctl",
                &["show", "--property=ActiveState", "--value", unit],
                Duration::from_secs(5),
            )
            .await
            .is_ok_and(|o| o.success && o.stdout.trim() == "inactive");
            if finished {
                if let Some(s) = current.filter(|s| s.done()) {
                    return s;
                }
            }
        }
        if Instant::now() >= deadline {
            return HelperStatus::new(
                name,
                HelperState::Failed,
                format!(
                    "{} didn't finish within {} minutes. Details: journalctl -u {unit}",
                    verb.describe(),
                    started.elapsed().as_secs().div_ceil(60)
                ),
            );
        }
        tokio::time::sleep(every).await;
    }
}

// ---------------------------------------------------------------------------
// Root fallback (no helper installed: development machines)
// ---------------------------------------------------------------------------

fn direct_precheck(verb: &HelperVerb) -> ApiResult<()> {
    match verb {
        HelperVerb::ConfigTxt { .. } | HelperVerb::Reapply if !Path::new(FIRSTBOOT).is_file() => {
            Err(ApiError::unavailable(
                "Boot settings can only be changed on a PixelPlus Pi image (the PixelPlus boot tools aren't installed).",
            ))
        }
        HelperVerb::RefreshIndex if !have("apt-get") => Err(not_possible_here("check for updates")),
        HelperVerb::Update if !(have("apt-get") && has_systemd() && have("systemd-run")) => {
            Err(ApiError::forbidden(
                "This PixelPlus can't update itself. Run: sudo apt install --only-upgrade pixelplus",
            ))
        }
        HelperVerb::SshOn | HelperVerb::SshOff
            if !(have("raspi-config") || (has_systemd() && have("systemctl"))) =>
        {
            Err(not_possible_here("change SSH"))
        }
        HelperVerb::WifiCountry(_) if !(have("raspi-config") || have("iw")) => {
            Err(not_possible_here("set the Wi-Fi country"))
        }
        _ => Ok(()),
    }
}

async fn ok_or(program: &str, args: &[&str], timeout: Duration, what: &str) -> Result<(), String> {
    match run(program, args, timeout).await {
        Ok(o) if o.success => Ok(()),
        Ok(o) => Err(format!("{what} failed: {}", o.stderr.trim())),
        Err(e) => Err(format!("{what} failed: {e}")),
    }
}

/// Do a helper verb directly (we are root and the helper isn't installed).
async fn direct(verb: &HelperVerb) -> Result<String, String> {
    match verb {
        HelperVerb::ConfigTxt { board, pixels } => {
            let mut args = vec![
                FIRSTBOOT.to_string(),
                "board-config".into(),
                "--board".into(),
                board.clone(),
            ];
            if let Some(p) = pixels {
                args.extend(["--pixels".into(), p.to_string()]);
            }
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            ok_or(
                "python3",
                &refs,
                Duration::from_secs(60),
                "Writing the board settings",
            )
            .await?;
            Ok(format!(
                "Board settings for {board} saved; restart the controller to use them"
            ))
        }
        HelperVerb::Reapply => {
            ok_or(
                "python3",
                &[FIRSTBOOT, "apply", "--force", "--no-reboot"],
                Duration::from_secs(300),
                "Applying pixelplus.txt",
            )
            .await?;
            Ok("pixelplus.txt applied".into())
        }
        HelperVerb::RefreshIndex => {
            ok_or(
                "apt-get",
                &["update", "-q"],
                Duration::from_secs(120),
                "Checking for updates",
            )
            .await?;
            Ok("Package lists refreshed".into())
        }
        HelperVerb::Update => {
            ok_or(
                "systemd-run",
                &[
                    "--unit=pixelplus-update",
                    "--collect",
                    "--setenv=DEBIAN_FRONTEND=noninteractive",
                    "sh",
                    "-c",
                    "apt-get update -q && apt-get install -y -q -o Dpkg::Options::=--force-confold --only-upgrade pixelplus",
                ],
                Duration::from_secs(20),
                "Starting the update",
            )
            .await?;
            Ok(
                "Installing the update. PixelPlus will restart by itself in a minute or two."
                    .into(),
            )
        }
        HelperVerb::SshOn | HelperVerb::SshOff => {
            let on = *verb == HelperVerb::SshOn;
            if have("raspi-config") {
                ok_or(
                    "raspi-config",
                    &["nonint", "do_ssh", if on { "0" } else { "1" }],
                    Duration::from_secs(60),
                    "Changing SSH",
                )
                .await?;
            } else {
                ok_or(
                    "systemctl",
                    &[
                        if on { "enable" } else { "disable" },
                        "--now",
                        "ssh.service",
                    ],
                    Duration::from_secs(60),
                    "Changing SSH",
                )
                .await?;
            }
            Ok(format!("SSH {}", if on { "on" } else { "off" }))
        }
        HelperVerb::WifiCountry(cc) => {
            let cc = cc.to_ascii_uppercase();
            if have("raspi-config") {
                ok_or(
                    "raspi-config",
                    &["nonint", "do_wifi_country", &cc],
                    Duration::from_secs(30),
                    "Setting the Wi-Fi country",
                )
                .await?;
            } else {
                ok_or(
                    "iw",
                    &["reg", "set", &cc],
                    Duration::from_secs(10),
                    "Setting the Wi-Fi country",
                )
                .await?;
            }
            Ok(format!("Wi-Fi country set to {cc}"))
        }
        HelperVerb::Hosts => {
            let name = super::system::hostname();
            let hosts = std::fs::read_to_string("/etc/hosts").unwrap_or_default();
            std::fs::write("/etc/hosts", sync_hosts(&hosts, &name))
                .map_err(|e| format!("Updating /etc/hosts failed: {e}"))?;
            Ok("/etc/hosts updated".into())
        }
    }
}

/// /etc/hosts with its `127.0.1.1` line pointing at `name` (Debian convention).
pub fn sync_hosts(hosts: &str, name: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut done = false;
    for l in hosts.lines() {
        if l.trim_start().starts_with("127.0.1.1")
            && l.trim_start()[9..].starts_with(char::is_whitespace)
        {
            if !done {
                out.push(format!("127.0.1.1\t{name}"));
                done = true;
            }
            continue;
        }
        out.push(l.to_string());
    }
    if !done {
        out.push(format!("127.0.1.1\t{name}"));
    }
    out.join("\n") + "\n"
}

// ---------------------------------------------------------------------------
// Power, time zone, hostname
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    Reboot,
    Shutdown,
    RestartService,
}

impl PowerAction {
    /// `systemctl` arguments: logind (reboot/power-off) and the unit restart that
    /// the polkit rules allow for the `pixelplus` user.
    pub fn systemctl_args(self) -> &'static [&'static str] {
        match self {
            PowerAction::Reboot => &["--no-ask-password", "reboot"],
            PowerAction::Shutdown => &["--no-ask-password", "poweroff"],
            PowerAction::RestartService => &[
                "--no-ask-password",
                "--no-block",
                "restart",
                "pixelplusd.service",
            ],
        }
    }
}

/// Check that we may reboot / shut down / restart. Returns the message to show.
pub fn check_power(action: PowerAction) -> ApiResult<&'static str> {
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
    if !can_control_power() {
        return Err(ApiError::forbidden(
            "PixelPlus isn't allowed to do that here (its system permissions aren't installed). Restart it yourself, or reinstall the pixelplus package.",
        ));
    }
    Ok(match action {
        PowerAction::Reboot => "Restarting. PixelPlus will be back in about a minute.",
        PowerAction::Shutdown => {
            "Shutting down. Wait for the green light to stop blinking before unplugging."
        }
        PowerAction::RestartService => "Restarting PixelPlus. This takes a few seconds.",
    })
}

/// Check, then do the power action after a short delay (so the HTTP response
/// reaches the browser first). Failures are reported as a toast.
pub fn power_action(state: &AppState, action: PowerAction) -> ApiResult<&'static str> {
    let msg = check_power(action)?;
    let events = state.events.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(800)).await;
        let args = action.systemctl_args();
        let err = match run("systemctl", args, Duration::from_secs(30)).await {
            Ok(o) if o.success => return,
            Ok(o) => o.stderr.trim().to_string(),
            Err(e) => e,
        };
        tracing::error!("systemctl {} failed: {err}", args.join(" "));
        events.toast(ToastKind::Error, format!("That didn't work: {err}"));
    });
    Ok(msg)
}

/// Set the host time zone through timedated (polkit allows it for the service
/// user). `Ok(false)` when this machine's time zone isn't ours to change.
pub async fn set_timezone(tz: &str) -> Result<bool, String> {
    if tz.parse::<chrono_tz::Tz>().is_err() {
        return Err(format!("\"{tz}\" isn't a time zone"));
    }
    if in_docker() || !cfg!(target_os = "linux") || !has_systemd() || !have("timedatectl") {
        return Ok(false);
    }
    if super::system::system_timezone().as_deref() == Some(tz) {
        return Ok(true);
    }
    match run(
        "timedatectl",
        &["--no-ask-password", "set-timezone", tz],
        Duration::from_secs(15),
    )
    .await
    {
        Ok(o) if o.success => {
            tracing::info!("System time zone set to {tz}");
            Ok(true)
        }
        Ok(o) => Err(o.stderr.trim().to_string()),
        Err(e) => Err(e),
    }
}

/// Change the hostname through hostnamed (static + transient; polkit allows
/// both for the service user), tell avahi, and sync /etc/hosts via the helper.
pub async fn set_hostname(state: &AppState, name: &str) -> Result<(), String> {
    let o = run(
        "hostnamectl",
        &[
            "--no-ask-password",
            "--static",
            "--transient",
            "set-hostname",
            name,
        ],
        Duration::from_secs(15),
    )
    .await?;
    if !o.success {
        return Err(o.stderr.trim().to_string());
    }
    // avahi does not follow hostnamed; members of "netdev" may rename it over D-Bus.
    let renamed = have("avahi-set-host-name")
        && run("avahi-set-host-name", &[name], Duration::from_secs(10))
            .await
            .is_ok_and(|o| o.success);
    if !renamed && is_root() {
        let _ = run(
            "systemctl",
            &["try-reload-or-restart", "avahi-daemon.service"],
            Duration::from_secs(15),
        )
        .await;
    }
    // /etc/hosts (sudo warns "unable to resolve host" otherwise): root-only file.
    if helper_installed() || is_root() {
        if let Ok(job) = run_helper(state, HelperVerb::Hosts, HelperOpts { quiet: true }).await {
            let s = job.wait(Duration::from_secs(30)).await;
            if s.state != HelperState::Ok {
                tracing::warn!("/etc/hosts not updated: {}", s.message);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::testkit::TestApp;

    #[test]
    fn verbs_and_instances() {
        let v = HelperVerb::ConfigTxt {
            board: "difftxlarge".into(),
            pixels: Some(1600),
        };
        assert_eq!(v.instance().unwrap(), "config-txt:difftxlarge:1600");
        assert_eq!(
            v.unit().unwrap(),
            "pixelplus-helper@config-txt:difftxlarge:1600.service"
        );
        assert_eq!(v.name(), "config-txt");
        let v = HelperVerb::ConfigTxt {
            board: "bare-pi".into(),
            pixels: None,
        };
        assert_eq!(v.instance().unwrap(), "config-txt:bare-pi");
        assert!(HelperVerb::ConfigTxt {
            board: "x;rm -rf".into(),
            pixels: None
        }
        .instance()
        .is_err());
        assert!(HelperVerb::ConfigTxt {
            board: "difftx".into(),
            pixels: Some(0)
        }
        .instance()
        .is_err());
        assert_eq!(HelperVerb::Update.instance().unwrap(), "update");
        // The helper script's verb (packaging/bin/pixelplus-helper).
        assert_eq!(
            HelperVerb::RefreshIndex.unit().unwrap(),
            "pixelplus-helper@refresh-index.service"
        );
        assert_eq!(
            HelperVerb::SshOff.unit().unwrap(),
            "pixelplus-helper@ssh-off.service"
        );
        assert_eq!(
            HelperVerb::WifiCountry("gb".into()).instance().unwrap(),
            "wifi-country:GB"
        );
        assert!(HelperVerb::WifiCountry("G1".into()).instance().is_err());
        // Unit names must satisfy the polkit rule: ^pixelplus-helper@[A-Za-z0-9:_.\-]+\.service$
        for v in [
            HelperVerb::ConfigTxt {
                board: "difftx".into(),
                pixels: Some(800),
            },
            HelperVerb::Update,
            HelperVerb::RefreshIndex,
            HelperVerb::SshOn,
            HelperVerb::Reapply,
            HelperVerb::Hosts,
            HelperVerb::WifiCountry("US".into()),
        ] {
            let u = v.unit().unwrap();
            let inst = u
                .strip_prefix("pixelplus-helper@")
                .unwrap()
                .strip_suffix(".service")
                .unwrap();
            assert!(
                inst.chars()
                    .all(|c| c.is_ascii_alphanumeric() || ":_.-".contains(c)),
                "{u}"
            );
        }
    }

    #[test]
    fn status_file_parsing() {
        let dir =
            std::env::temp_dir().join(format!("pp-helper-{}", pixelplus_core::model::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(read_status(&dir, "update").is_none());
        std::fs::write(
            status_path(&dir, "update"),
            r#"{"verb":"update","state":"running","message":"Installing update","updatedAt":1700000000}"#,
        )
        .unwrap();
        let s = read_status(&dir, "update").unwrap();
        assert_eq!(s.state, HelperState::Running);
        assert_eq!(s.updated_at, 1_700_000_000);
        assert!(!s.done());
        std::fs::write(status_path(&dir, "update"), "{not json").unwrap();
        assert!(read_status(&dir, "update").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn follow_reports_progress_then_result() {
        let app = TestApp::new();
        let mut rx = app.state.events.subscribe();
        let dir = app.dir.join("run");
        std::fs::create_dir_all(&dir).unwrap();
        let now = chrono::Utc::now().timestamp();
        // A stale result from an earlier run must be ignored.
        std::fs::write(
            status_path(&dir, "ssh-on"),
            format!(
                r#"{{"verb":"ssh-on","state":"ok","message":"old","updatedAt":{}}}"#,
                now - 3600
            ),
        )
        .unwrap();
        let d2 = dir.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(60)).await;
            std::fs::write(
                status_path(&d2, "ssh-on"),
                format!(
                    r#"{{"verb":"ssh-on","state":"running","message":"working","updatedAt":{now}}}"#
                ),
            )
            .unwrap();
            tokio::time::sleep(Duration::from_millis(60)).await;
            std::fs::write(
                status_path(&d2, "ssh-on"),
                format!(r#"{{"verb":"ssh-on","state":"ok","message":"SSH on","updatedAt":{now}}}"#),
            )
            .unwrap();
        });
        let s = follow(
            &app.state,
            &dir,
            &HelperVerb::SshOn,
            "",
            now - 1,
            read_status(&dir, "ssh-on"),
            Duration::from_millis(10),
        )
        .await;
        assert_eq!(s.state, HelperState::Ok);
        assert_eq!(s.message, "SSH on");
        // The running state was published on the way.
        let mut saw_running = false;
        while let Ok(ev) = rx.try_recv() {
            if format!("{ev:?}").contains("working") {
                saw_running = true;
            }
        }
        assert!(saw_running);
        assert_eq!(
            app.state.services.helpers.get("ssh-on").unwrap().message,
            "working"
        );
    }

    #[test]
    fn hosts_sync() {
        let h = "127.0.0.1\tlocalhost\n127.0.1.1\told-name\n::1 localhost\n";
        assert_eq!(
            sync_hosts(h, "garage"),
            "127.0.0.1\tlocalhost\n127.0.1.1\tgarage\n::1 localhost\n"
        );
        assert_eq!(
            sync_hosts("127.0.0.1 localhost\n", "garage"),
            "127.0.0.1 localhost\n127.0.1.1\tgarage\n"
        );
        // 127.0.1.10 is a different address.
        assert!(sync_hosts("127.0.1.10 other\n", "g").contains("127.0.1.10 other"));
    }

    #[test]
    fn power_args_match_polkit_rules() {
        // polkit allows restart/try-restart of exactly "pixelplusd.service".
        assert!(PowerAction::RestartService
            .systemctl_args()
            .contains(&"pixelplusd.service"));
        assert!(PowerAction::Reboot.systemctl_args().contains(&"reboot"));
        assert!(PowerAction::Shutdown.systemctl_args().contains(&"poweroff"));
    }
}
