//! Remote access without port forwarding (F14, ARCHITECTURE §12.12):
//! Tailscale and Cloudflare Tunnel, driven through root helper verbs
//! (`packaging/bin/pixelplus-helper`: `tailscale-*`, `cloudflared-*`).
//!
//! **Public pages only by default.** Funnels and tunnels point at the
//! public-only listener (`127.0.0.1:8081`, `api::security::public_only`),
//! which serves the song request page, its API and the games controller and
//! answers 404 to everything else. The admin UI is exposed only on explicit
//! opt-in, and only with a password set:
//!
//! * Tailscale `serve` → `https://<host>.<tailnet>.ts.net` for the owner's own
//!   devices (tailnet members only): the safest remote admin.
//! * a Cloudflare *admin hostname* → the admin UI on the internet (sign-in
//!   page exposed): the UI insists on a password and recommends Cloudflare
//!   Access (email one-time PIN) in front of it.
//!
//! Admin hostnames are allowed by the Host allow-list only while exposed
//! (`security::remote_admin_hosts`), and `security::guard` refuses admin API
//! calls through any tunnel while no password is set.
//!
//! Secrets (Tailscale auth keys, Cloudflare tunnel tokens) are write-only:
//! written to a 0600 file the helper consumes and deletes, never passed on a
//! command line and never stored in `show.json` (only `tokenSet`).

use super::platform::{self, ExtVerb, HelperOpts, HelperStatus, HelperVerb};
use super::system::{have, in_docker, run};
use crate::api::{ApiError, ApiResult};
use crate::state::AppState;
use parking_lot::Mutex;
use pixelplus_core::model::{CloudflareState, TailscaleState};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// cloudflared's metrics server for the quick tunnel (`--metrics`), which
/// reports the random `*.trycloudflare.com` name at `/quicktunnel`.
pub const QUICK_METRICS: &str = "127.0.0.1:20241";

/// Runtime state of this service (`state.services.remote`).
#[derive(Default)]
pub struct RemoteState {
    cache: Mutex<Option<(Instant, RemoteStatus)>>,
}

/// Start the service (called once from `services::start_all`).
pub fn start(_state: &AppState) {}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TailscaleStatus {
    pub installed: bool,
    /// tailscaled's BackendState: `NotInstalled`, `NoState`, `NeedsLogin`,
    /// `NeedsMachineAuth`, `Stopped`, `Starting`, `Running`.
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns_name: Option<String>,
    /// HTTPS certificates are enabled in the tailnet (MagicDNS + HTTPS).
    pub https_ok: bool,
    /// `tailscale serve` publishes the admin UI to the tailnet.
    pub serve: bool,
    /// `tailscale funnel` publishes the public pages to the internet.
    pub funnel: bool,
    /// Open this to connect the controller to a tailnet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login_url: Option<String>,
    /// Tailnet addresses (100.x).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ips: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareStatus {
    pub installed: bool,
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Public addresses of the request page.
    pub urls: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub admin_host: Option<String>,
    pub token_set: bool,
}

/// `GET /remote/status`.
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatus {
    pub tailscale: TailscaleStatus,
    pub cloudflare: CloudflareStatus,
    /// The public-only listener is on (settings) and has a port.
    pub public_listener: bool,
    pub public_port: u16,
    /// A web UI password is set (required to expose the admin).
    pub password_set: bool,
    /// Remote access can be set up here (packaged helper, not Docker).
    pub can_manage: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

// ---------------------------------------------------------------------------
// Parsing (unit-tested)
// ---------------------------------------------------------------------------

/// Parse `tailscale status --json`.
pub fn parse_tailscale_status(json: &str) -> TailscaleStatus {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let state = v["BackendState"].as_str().unwrap_or("NoState").to_string();
    let dns_name = v["Self"]["DNSName"]
        .as_str()
        .map(|s| s.trim_end_matches('.').to_string())
        .filter(|s| !s.is_empty());
    let https_ok = v["CertDomains"].as_array().is_some_and(|a| !a.is_empty());
    let login_url = v["AuthURL"]
        .as_str()
        .filter(|u| u.starts_with("https://"))
        .map(String::from);
    let ips = v["Self"]["TailscaleIPs"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    TailscaleStatus {
        installed: true,
        state,
        dns_name,
        https_ok,
        login_url,
        ips,
        ..Default::default()
    }
}

/// Parse `tailscale serve status --json` → (admin served on 443, funnel on).
pub fn parse_serve_status(json: &str) -> (bool, bool) {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let web = v["Web"].as_object();
    let serve = web.is_some_and(|w| {
        w.iter().any(|(host, cfg)| {
            host.ends_with(":443")
                && cfg["Handlers"].as_object().is_some_and(|h| {
                    h.values()
                        .any(|x| x["Proxy"].as_str().is_some_and(|p| p.ends_with(":80")))
                })
        })
    });
    let funnel = v["AllowFunnel"]
        .as_object()
        .is_some_and(|f| f.values().any(|x| x.as_bool() == Some(true)));
    (serve, funnel)
}

/// A Cloudflare tunnel token (base64 JSON from the Zero Trust dashboard).
pub fn valid_tunnel_token(t: &str) -> bool {
    (40..=4096).contains(&t.len())
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '=' | '+' | '/' | '-' | '_' | '.'))
}

/// A Tailscale auth key (`tskey-auth-…`).
pub fn valid_auth_key(k: &str) -> bool {
    k.starts_with("tskey-")
        && (16..=200).contains(&k.len())
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A public DNS host name (`lights.example.com`).
pub fn valid_host(h: &str) -> bool {
    let h = h.trim_end_matches('.');
    h.len() <= 253
        && h.contains('.')
        && h.parse::<std::net::IpAddr>().is_err()
        && h.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

fn norm_host(h: &str) -> String {
    let h = h.trim();
    let h = h
        .strip_prefix("https://")
        .or_else(|| h.strip_prefix("http://"))
        .unwrap_or(h);
    h.split('/')
        .next()
        .unwrap_or_default()
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

/// Host names `POST /remote/test` may call (ours only: no SSRF).
pub fn known_hosts(status: &RemoteStatus) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(n) = &status.tailscale.dns_name {
        out.push(n.to_ascii_lowercase());
    }
    for u in &status.cloudflare.urls {
        out.push(norm_host(u));
    }
    for h in [
        &status.cloudflare.public_host,
        &status.cloudflare.admin_host,
    ]
    .into_iter()
    .flatten()
    {
        out.push(norm_host(h));
    }
    out.retain(|h| !h.is_empty());
    out
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

fn can_manage() -> bool {
    platform::helper_installed() && !in_docker()
}

async fn tailscale_status() -> TailscaleStatus {
    if !have("tailscale") {
        return TailscaleStatus {
            state: "NotInstalled".into(),
            ..Default::default()
        };
    }
    let mut st = match run("tailscale", &["status", "--json"], Duration::from_secs(8)).await {
        Ok(o) if !o.stdout.trim().is_empty() => parse_tailscale_status(&o.stdout),
        _ => TailscaleStatus {
            installed: true,
            state: "Stopped".into(),
            ..Default::default()
        },
    };
    if st.state == "Running" {
        if let Ok(o) = run(
            "tailscale",
            &["serve", "status", "--json"],
            Duration::from_secs(8),
        )
        .await
        {
            let (serve, funnel) = parse_serve_status(&o.stdout);
            st.serve = serve;
            st.funnel = funnel;
        }
    }
    st
}

async fn unit_active(unit: &str) -> bool {
    run(
        "systemctl",
        &["is-active", "--quiet", unit],
        Duration::from_secs(5),
    )
    .await
    .is_ok_and(|o| o.success)
}

/// The quick tunnel's `https://….trycloudflare.com` address, if it runs.
async fn quick_url() -> Option<String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .ok()?;
    let v: serde_json::Value = client
        .get(format!("http://{QUICK_METRICS}/quicktunnel"))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    v["hostname"]
        .as_str()
        .filter(|h| valid_host(h))
        .map(|h| format!("https://{h}"))
}

/// `GET /remote/status` (cached 3 s; `fresh` skips the cache).
pub async fn status(state: &AppState, fresh: bool) -> RemoteStatus {
    if !fresh {
        if let Some((at, s)) = state.services.remote.cache.lock().clone() {
            if at.elapsed() < Duration::from_secs(3) {
                return s;
            }
        }
    }
    let show = state.store.get();
    let remote = show.settings.remote.clone();
    let mut st = RemoteStatus {
        public_listener: remote.public_listener && state.config.public_port != 0,
        public_port: state.config.public_port,
        password_set: show.settings.security.password_hash.is_some(),
        can_manage: can_manage(),
        ..Default::default()
    };
    drop(show);
    if in_docker() {
        st.message = Some(
            "In Docker, run Tailscale or cloudflared as their own containers next to PixelPlus (see docker/README.md) and point them at the public port.".into(),
        );
    }
    st.tailscale = tailscale_status().await;
    let cf = remote.cloudflare.clone();
    st.cloudflare = CloudflareStatus {
        installed: have("cloudflared"),
        mode: cf.as_ref().map(|c| c.mode.clone()),
        public_host: cf.as_ref().and_then(|c| c.public_host.clone()),
        admin_host: cf.as_ref().and_then(|c| c.admin_host.clone()),
        token_set: cf.as_ref().is_some_and(|c| c.token_set),
        ..Default::default()
    };
    if st.cloudflare.installed {
        let quick = unit_active("pixelplus-cloudflared-quick.service").await;
        let named = unit_active("pixelplus-cloudflared.service").await;
        st.cloudflare.running = quick || named;
        if quick {
            st.cloudflare.mode = Some("quick".into());
            st.cloudflare.urls.extend(quick_url().await);
        }
        if named {
            st.cloudflare.mode = Some("token".into());
            if let Some(h) = &st.cloudflare.public_host {
                st.cloudflare.urls.push(format!("https://{h}"));
            }
        }
    }
    *state.services.remote.cache.lock() = Some((Instant::now(), st.clone()));
    st
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

fn remote_dir(state: &AppState) -> PathBuf {
    state.config.data_dir.join("remote")
}

/// Write a secret for the helper (0600, fresh file; the helper deletes it).
fn write_secret(state: &AppState, name: &str, value: &str) -> ApiResult<()> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let dir = remote_dir(state);
    std::fs::create_dir_all(&dir).map_err(ApiError::internal)?;
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    o.mode(0o600);
    let mut f = o.open(&path).map_err(ApiError::internal)?;
    f.write_all(value.as_bytes()).map_err(ApiError::internal)?;
    f.sync_all().map_err(ApiError::internal)?;
    Ok(())
}

fn verb(name: &'static str, arg: Option<&str>, describe: &str, minutes: u64) -> HelperVerb {
    HelperVerb::Ext(ExtVerb::new(
        name,
        arg,
        describe,
        Duration::from_secs(minutes * 60),
        "set up remote access",
    ))
}

async fn run_verb(state: &AppState, v: HelperVerb) -> ApiResult<HelperStatus> {
    if !can_manage() {
        return Err(platform::not_possible_here("set up remote access"));
    }
    let job = platform::run_helper(state, v, HelperOpts { quiet: false }).await?;
    *state.services.remote.cache.lock() = None;
    Ok(job.status)
}

async fn run_verb_wait(
    state: &AppState,
    v: HelperVerb,
    timeout: Duration,
) -> ApiResult<HelperStatus> {
    if !can_manage() {
        return Err(platform::not_possible_here("set up remote access"));
    }
    let job = platform::run_helper(state, v, HelperOpts { quiet: true }).await?;
    let s = job.wait(timeout).await;
    *state.services.remote.cache.lock() = None;
    if s.state == platform::HelperState::Failed {
        return Err(ApiError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "helper_failed",
            s.message,
        ));
    }
    Ok(s)
}

fn require_password(state: &AppState, what: &str) -> ApiResult<()> {
    if state.store.get().settings.security.password_hash.is_none() {
        return Err(ApiError::new(
            axum::http::StatusCode::CONFLICT,
            "password_required",
            format!("Set a password (Settings → Security) before {what}: the admin pages would be reachable from outside your home."),
        ));
    }
    Ok(())
}

/// Point the request page / games QR codes at a new public address, unless
/// the owner entered their own.
fn public_urls_update(
    show: &mut pixelplus_core::model::Show,
    base: Option<&str>,
    old_base: Option<&str>,
) {
    let ours = |u: &str| {
        u.is_empty()
            || u.contains(".ts.net")
            || u.contains(".trycloudflare.com")
            || old_base.is_some_and(|b| u.starts_with(b))
    };
    let req = show
        .settings
        .requests
        .public_url
        .clone()
        .unwrap_or_default();
    if ours(&req) {
        show.settings.requests.public_url = base.map(|b| format!("{b}/request"));
    }
    if ours(&show.settings.games.public_url) {
        show.settings.games.public_url = base.map(|b| format!("{b}/play")).unwrap_or_default();
    }
}

async fn save_remote(
    state: &AppState,
    f: impl FnOnce(&mut pixelplus_core::model::Show) + Send + 'static,
) -> ApiResult<()> {
    state
        .store
        .update(move |s| {
            f(s);
            Ok(())
        })
        .await?;
    *state.services.remote.cache.lock() = None;
    Ok(())
}

/// Body of the `on/off` actions (no body = on).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Toggle {
    #[serde(default = "yes")]
    pub on: bool,
}

impl Default for Toggle {
    fn default() -> Self {
        Toggle { on: true }
    }
}

fn yes() -> bool {
    true
}

/// Body of `POST /remote/tailscale/up`.
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TailscaleUp {
    /// Optional auth key (write-only); otherwise a login link is shown.
    #[serde(default)]
    pub auth_key: Option<String>,
}

pub async fn tailscale_install(state: &AppState) -> ApiResult<HelperStatus> {
    run_verb(
        state,
        verb("tailscale-install", None, "Installing Tailscale", 15),
    )
    .await
}

pub async fn tailscale_up(state: &AppState, body: TailscaleUp) -> ApiResult<HelperStatus> {
    if let Some(k) = body
        .auth_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
    {
        if !valid_auth_key(k) {
            return Err(ApiError::bad_request(
                "That doesn't look like a Tailscale auth key (tskey-auth-…).",
            ));
        }
        write_secret(state, "tailscale.authkey", k)?;
    }
    let s = run_verb_wait(
        state,
        verb("tailscale-up", None, "Connecting to Tailscale", 2),
        Duration::from_secs(60),
    )
    .await?;
    save_remote(state, |show| {
        let ts = show
            .settings
            .remote
            .tailscale
            .get_or_insert(TailscaleState {
                enabled: true,
                serve_admin: false,
                funnel_public: false,
                dns_name: None,
            });
        ts.enabled = true;
    })
    .await?;
    Ok(s)
}

pub async fn tailscale_serve(state: &AppState, on: bool) -> ApiResult<HelperStatus> {
    if on {
        require_password(state, "managing PixelPlus over Tailscale")?;
        let st = tailscale_status().await;
        if st.state != "Running" {
            return Err(ApiError::conflict(
                "Connect this controller to Tailscale first.",
            ));
        }
    }
    let arg = if on { "on" } else { "off" };
    let s = run_verb_wait(
        state,
        verb(
            "tailscale-serve",
            Some(arg),
            "Setting up Tailscale HTTPS",
            2,
        ),
        Duration::from_secs(90),
    )
    .await?;
    let dns = tailscale_status().await.dns_name;
    save_remote(state, move |show| {
        let ts = show
            .settings
            .remote
            .tailscale
            .get_or_insert(TailscaleState {
                enabled: true,
                serve_admin: false,
                funnel_public: false,
                dns_name: None,
            });
        ts.serve_admin = on;
        if dns.is_some() {
            ts.dns_name = dns;
        }
    })
    .await?;
    Ok(s)
}

pub async fn tailscale_funnel(state: &AppState, on: bool) -> ApiResult<HelperStatus> {
    if on && state.config.public_port == 0 {
        return Err(ApiError::conflict(
            "The public-only port is turned off (PIXELPLUS_PUBLIC_PORT=0).",
        ));
    }
    let arg = if on { "on" } else { "off" };
    let s = run_verb_wait(
        state,
        verb(
            "tailscale-funnel",
            Some(arg),
            "Publishing the song request page",
            2,
        ),
        Duration::from_secs(90),
    )
    .await?;
    let dns = tailscale_status().await.dns_name;
    save_remote(state, move |show| {
        let old = show
            .settings
            .remote
            .tailscale
            .as_ref()
            .and_then(|t| t.dns_name.clone())
            .map(|d| format!("https://{d}:8443"));
        let ts = show
            .settings
            .remote
            .tailscale
            .get_or_insert(TailscaleState {
                enabled: true,
                serve_admin: false,
                funnel_public: false,
                dns_name: None,
            });
        ts.funnel_public = on;
        if dns.is_some() {
            ts.dns_name = dns.clone();
        }
        if on {
            show.settings.remote.public_listener = true;
        }
        let base = dns.filter(|_| on).map(|d| format!("https://{d}:8443"));
        public_urls_update(show, base.as_deref(), old.as_deref());
    })
    .await?;
    Ok(s)
}

pub async fn tailscale_down(state: &AppState) -> ApiResult<HelperStatus> {
    let s = run_verb_wait(
        state,
        verb("tailscale-down", None, "Disconnecting from Tailscale", 2),
        Duration::from_secs(60),
    )
    .await?;
    save_remote(state, |show| {
        let old = show
            .settings
            .remote
            .tailscale
            .as_ref()
            .and_then(|t| t.dns_name.clone())
            .map(|d| format!("https://{d}:8443"));
        show.settings.remote.tailscale = None;
        public_urls_update(show, None, old.as_deref());
    })
    .await?;
    Ok(s)
}

pub async fn cloudflare_install(state: &AppState) -> ApiResult<HelperStatus> {
    run_verb(
        state,
        verb("cloudflared-install", None, "Installing cloudflared", 15),
    )
    .await
}

pub async fn cloudflare_quick(state: &AppState, on: bool) -> ApiResult<HelperStatus> {
    if on && state.config.public_port == 0 {
        return Err(ApiError::conflict(
            "The public-only port is turned off (PIXELPLUS_PUBLIC_PORT=0).",
        ));
    }
    let arg = if on { "on" } else { "off" };
    let s = run_verb_wait(
        state,
        verb(
            "cloudflared-quick",
            Some(arg),
            "Starting a temporary public link",
            2,
        ),
        Duration::from_secs(60),
    )
    .await?;
    // The random name shows up once cloudflared registered (a few seconds).
    let mut url = None;
    if on {
        for _ in 0..20 {
            url = quick_url().await;
            if url.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    save_remote(state, move |show| {
        if on {
            show.settings.remote.public_listener = true;
            let token_set = show
                .settings
                .remote
                .cloudflare
                .as_ref()
                .is_some_and(|c| c.token_set);
            let prev = show.settings.remote.cloudflare.take();
            show.settings.remote.cloudflare = Some(CloudflareState {
                mode: "quick".into(),
                public_host: prev.as_ref().and_then(|c| c.public_host.clone()),
                admin_host: prev.and_then(|c| c.admin_host),
                token_set,
            });
        } else if let Some(c) = show.settings.remote.cloudflare.as_mut() {
            if c.mode == "quick" {
                c.mode = if c.token_set {
                    "token".into()
                } else {
                    "quick".into()
                };
            }
        }
        public_urls_update(show, url.as_deref(), None);
    })
    .await?;
    Ok(s)
}

/// Body of `POST /remote/cloudflare/token` (and `/hosts`).
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareToken {
    /// The tunnel token from the Zero Trust dashboard (write-only).
    #[serde(default)]
    pub token: Option<String>,
    /// Public hostname of the request page (→ `http://localhost:8081`).
    #[serde(default)]
    pub public_host: Option<String>,
    /// Optional admin hostname (→ `http://localhost:80`); needs a password.
    #[serde(default)]
    pub admin_host: Option<String>,
}

fn clean_host(h: Option<&str>) -> ApiResult<Option<String>> {
    match h.map(norm_host).filter(|h| !h.is_empty()) {
        None => Ok(None),
        Some(h) if valid_host(&h) => Ok(Some(h)),
        Some(h) => Err(ApiError::bad_request(format!(
            "“{h}” isn't a host name like lights.example.com."
        ))),
    }
}

/// Save the Cloudflare host names (and allow the admin one).
pub async fn cloudflare_hosts(state: &AppState, body: &CloudflareToken) -> ApiResult<()> {
    let public = clean_host(body.public_host.as_deref())?;
    let admin = clean_host(body.admin_host.as_deref())?;
    if admin.is_some() {
        require_password(state, "publishing the admin pages")?;
    }
    if admin.is_some() && admin == public {
        return Err(ApiError::bad_request(
            "Use different host names for the public page and the admin pages.",
        ));
    }
    save_remote(state, move |show| {
        let old = show
            .settings
            .remote
            .cloudflare
            .as_ref()
            .and_then(|c| c.public_host.clone())
            .map(|h| format!("https://{h}"));
        let c = show
            .settings
            .remote
            .cloudflare
            .get_or_insert(CloudflareState {
                mode: "token".into(),
                public_host: None,
                admin_host: None,
                token_set: false,
            });
        c.public_host = public.clone();
        c.admin_host = admin;
        let base = public.map(|h| format!("https://{h}"));
        public_urls_update(show, base.as_deref(), old.as_deref());
    })
    .await
}

pub async fn cloudflare_token(state: &AppState, body: CloudflareToken) -> ApiResult<HelperStatus> {
    let token = body
        .token
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| ApiError::bad_request("Paste the tunnel token."))?;
    if !valid_tunnel_token(token) {
        return Err(ApiError::bad_request(
            "That doesn't look like a Cloudflare tunnel token (a long code from the Zero Trust dashboard).",
        ));
    }
    if state.config.public_port == 0 {
        return Err(ApiError::conflict(
            "The public-only port is turned off (PIXELPLUS_PUBLIC_PORT=0).",
        ));
    }
    cloudflare_hosts(state, &body).await?;
    write_secret(state, "cloudflared.token", token)?;
    let s = run_verb_wait(
        state,
        verb(
            "cloudflared-token",
            None,
            "Starting the Cloudflare tunnel",
            3,
        ),
        Duration::from_secs(120),
    )
    .await?;
    save_remote(state, |show| {
        show.settings.remote.public_listener = true;
        let c = show
            .settings
            .remote
            .cloudflare
            .get_or_insert(CloudflareState {
                mode: "token".into(),
                public_host: None,
                admin_host: None,
                token_set: true,
            });
        c.mode = "token".into();
        c.token_set = true;
    })
    .await?;
    Ok(s)
}

pub async fn cloudflare_stop(state: &AppState) -> ApiResult<HelperStatus> {
    let s = run_verb_wait(
        state,
        verb(
            "cloudflared-stop",
            None,
            "Stopping the Cloudflare tunnel",
            2,
        ),
        Duration::from_secs(60),
    )
    .await?;
    save_remote(state, |show| {
        let old = show
            .settings
            .remote
            .cloudflare
            .as_ref()
            .and_then(|c| c.public_host.clone())
            .map(|h| format!("https://{h}"));
        show.settings.remote.cloudflare = None;
        public_urls_update(show, None, old.as_deref());
    })
    .await?;
    Ok(s)
}

/// `POST /remote/test {url}`: can the public page be reached through the
/// tunnel? Only our own remote addresses may be tested (no open proxy).
pub async fn test(state: &AppState, url: &str) -> ApiResult<serde_json::Value> {
    let st = status(state, true).await;
    let parsed = reqwest::Url::parse(url.trim())
        .map_err(|_| ApiError::bad_request("That isn't a web address."))?;
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    if parsed.scheme() != "https" || !known_hosts(&st).contains(&host) {
        return Err(ApiError::bad_request(
            "Only this controller's own Tailscale / Cloudflare addresses can be tested.",
        ));
    }
    let mut probe = parsed.clone();
    probe.set_path("/api/v1/public/health");
    probe.set_query(None);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(ApiError::internal)?;
    let t0 = Instant::now();
    let r = client.get(probe.clone()).send().await;
    let ms = t0.elapsed().as_millis() as u64;
    Ok(match r {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let ok = status == 200 && body["ok"].as_bool() == Some(true);
            serde_json::json!({
                "ok": ok,
                "url": probe.as_str(),
                "status": status,
                "ms": ms,
                "error": (!ok).then(|| format!("It answered HTTP {status}, not the PixelPlus public page.")),
            })
        }
        Err(e) => serde_json::json!({
            "ok": false,
            "url": probe.as_str(),
            "ms": ms,
            "error": if e.is_timeout() { "No answer (timed out).".to_string() } else { format!("Couldn't reach it: {e}") },
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tailscale_status_fixtures() {
        let running = r#"{
          "Version": "1.76.1", "BackendState": "Running", "AuthURL": "",
          "Self": {"DNSName": "pixelplus.tail1234.ts.net.", "TailscaleIPs": ["100.101.102.103", "fd7a::1"]},
          "CertDomains": ["pixelplus.tail1234.ts.net"]
        }"#;
        let s = parse_tailscale_status(running);
        assert_eq!(s.state, "Running");
        assert_eq!(s.dns_name.as_deref(), Some("pixelplus.tail1234.ts.net"));
        assert!(s.https_ok);
        assert_eq!(s.login_url, None);
        assert_eq!(s.ips[0], "100.101.102.103");
        let login = r#"{"BackendState": "NeedsLogin", "AuthURL": "https://login.tailscale.com/a/1a2b3c",
                        "Self": {"DNSName": ""}, "CertDomains": null}"#;
        let s = parse_tailscale_status(login);
        assert_eq!(s.state, "NeedsLogin");
        assert_eq!(
            s.login_url.as_deref(),
            Some("https://login.tailscale.com/a/1a2b3c")
        );
        assert!(!s.https_ok);
        assert_eq!(s.dns_name, None);
        // A hostile AuthURL is not offered as a link.
        let s = parse_tailscale_status(
            r#"{"BackendState":"NeedsLogin","AuthURL":"javascript:alert(1)"}"#,
        );
        assert_eq!(s.login_url, None);
        assert_eq!(parse_tailscale_status("not json").state, "NoState");
    }

    #[test]
    fn serve_status_fixtures() {
        let both = r#"{
          "TCP": {"443": {"HTTPS": true}, "8443": {"HTTPS": true}},
          "Web": {
            "pixelplus.tail1234.ts.net:443": {"Handlers": {"/": {"Proxy": "http://127.0.0.1:80"}}},
            "pixelplus.tail1234.ts.net:8443": {"Handlers": {"/": {"Proxy": "http://127.0.0.1:8081"}}}
          },
          "AllowFunnel": {"pixelplus.tail1234.ts.net:8443": true}
        }"#;
        assert_eq!(parse_serve_status(both), (true, true));
        let funnel_only = r#"{"Web": {"x.ts.net:8443": {"Handlers": {"/": {"Proxy": "http://127.0.0.1:8081"}}}},
                             "AllowFunnel": {"x.ts.net:8443": true}}"#;
        assert_eq!(parse_serve_status(funnel_only), (false, true));
        assert_eq!(parse_serve_status("{}"), (false, false));
    }

    #[test]
    fn secrets_and_hosts_validation() {
        assert!(valid_tunnel_token(
            &"eyJhIjoiYWJjIiwidCI6Inh5eiJ9".repeat(3)
        ));
        assert!(!valid_tunnel_token("short"));
        assert!(!valid_tunnel_token(&format!(
            "{} --url http://evil",
            "a".repeat(50)
        )));
        assert!(valid_auth_key("tskey-auth-kAbC123CNTRL-xyz789"));
        assert!(!valid_auth_key("tskey-auth x"));
        assert!(!valid_auth_key("something-else-entirely"));
        assert!(valid_host("lights.example.com"));
        assert!(valid_host("a-b.example.co.uk."));
        assert!(!valid_host("localhost"));
        assert!(!valid_host("192.168.1.2"));
        assert!(!valid_host("-bad.example.com"));
        assert!(!valid_host("sp ace.example.com"));
        assert_eq!(
            norm_host("https://Lights.Example.com/request"),
            "lights.example.com"
        );
    }

    #[test]
    fn only_our_own_addresses_can_be_tested() {
        let st = RemoteStatus {
            tailscale: TailscaleStatus {
                dns_name: Some("pp.tail1.ts.net".into()),
                ..Default::default()
            },
            cloudflare: CloudflareStatus {
                urls: vec!["https://calm-otter.trycloudflare.com".into()],
                public_host: Some("lights.example.com".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let hosts = known_hosts(&st);
        assert!(hosts.contains(&"pp.tail1.ts.net".to_string()));
        assert!(hosts.contains(&"calm-otter.trycloudflare.com".to_string()));
        assert!(hosts.contains(&"lights.example.com".to_string()));
        assert!(!hosts.contains(&"169.254.169.254".to_string()));
    }

    #[test]
    fn public_urls_follow_the_tunnel_unless_set_by_hand() {
        let mut show = pixelplus_core::model::Show::default();
        public_urls_update(&mut show, Some("https://pp.tail1.ts.net:8443"), None);
        assert_eq!(
            show.settings.requests.public_url.as_deref(),
            Some("https://pp.tail1.ts.net:8443/request")
        );
        assert_eq!(
            show.settings.games.public_url,
            "https://pp.tail1.ts.net:8443/play"
        );
        public_urls_update(&mut show, None, Some("https://pp.tail1.ts.net:8443"));
        assert_eq!(show.settings.requests.public_url, None);
        // The owner's own address stays.
        show.settings.requests.public_url = Some("https://lights.mydomain.org/request".into());
        public_urls_update(&mut show, Some("https://x.trycloudflare.com"), None);
        assert_eq!(
            show.settings.requests.public_url.as_deref(),
            Some("https://lights.mydomain.org/request")
        );
    }
}
