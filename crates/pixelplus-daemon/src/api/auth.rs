//! Optional password protection.
//!
//! * No password set → everything is open (the UI nudges the user to set one).
//! * Password set → a session cookie (`pp_session`) is required, except for
//!   `/api/v1/auth/*`, `/api/v1/public/*`, `/api/v1/system` (so the UI can show
//!   the sign-in screen), `/api/v1/hooks/*` (trigger links: their own token,
//!   `api::hooks`) and `/api/v1/cluster/*` (signed with a per-follower
//!   key, checked in `api::cluster`; a cluster key never opens anything else).
//! * Local sidecars (games) send `X-PixelPlus-Local: <token>`, the random
//!   token this daemon writes to `/run/pixelplus/local-token` at startup. It
//!   is accepted only from loopback, only without proxy headers, and only for
//!   the few routes in [`super::security::sidecar_route`].
//! * Sign-in is throttled per client address and globally, and password
//!   hashing runs on at most two blocking threads.
//! * A sign-in from outside the home network (Cloudflare Tunnel, Tailscale,
//!   another tunnel or proxy, or a public address; F14) raises an alert
//!   ([`remote_via`], `services::alerts::remote_sign_in`).

use super::{ApiError, ApiResult};
use crate::state::AppState;
use argon2::password_hash::{
    rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
};
use argon2::Argon2;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{post, put};
use axum::{Json, Router};
use parking_lot::Mutex;
use serde::Deserialize;
use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const COOKIE: &str = "pp_session";
const SESSION_TTL: Duration = Duration::from_secs(60 * 60 * 24 * 30);
/// Header carrying the local sidecar token.
pub const LOCAL_HEADER: &str = "x-pixelplus-local";
/// Shortest accepted new password (the UI says the same).
pub const MIN_PASSWORD: usize = 6;

/// Wrong passwords allowed per address before it has to wait.
const FREE_TRIES: u32 = 5;
const FIRST_LOCKOUT: Duration = Duration::from_secs(60);
const MAX_LOCKOUT: Duration = Duration::from_secs(15 * 60);
/// Wrong passwords from everywhere within [`GLOBAL_WINDOW`] before every
/// sign-in pauses for [`GLOBAL_LOCKOUT`] (attacks from many addresses).
const GLOBAL_FAILURES: usize = 50;
const GLOBAL_WINDOW: Duration = Duration::from_secs(10 * 60);
const GLOBAL_LOCKOUT: Duration = Duration::from_secs(60);

/// Argon2 uses ~19 MiB and tens of ms per hash: never more than two at once.
static HASHING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

#[derive(Default)]
struct IpFailures {
    failures: u32,
    until: Option<Instant>,
    last: Option<Instant>,
}

/// Sign-in throttle (per address, exponential; plus a global brake).
#[derive(Default)]
pub struct Throttle {
    per_ip: HashMap<IpAddr, IpFailures>,
    recent: VecDeque<Instant>,
    global_until: Option<Instant>,
}

impl Throttle {
    /// `Err(wait)` while `ip` (or everyone) must wait.
    pub fn check(&mut self, ip: Option<IpAddr>, now: Instant) -> Result<(), Duration> {
        if let Some(t) = self.global_until.filter(|t| *t > now) {
            return Err(t - now);
        }
        if let Some(until) = ip
            .and_then(|ip| self.per_ip.get(&ip))
            .and_then(|f| f.until)
            .filter(|t| *t > now)
        {
            return Err(until - now);
        }
        Ok(())
    }

    /// Like [`Throttle::check`] but only the per-address back-off (the global
    /// brake is checked separately by callers that let a correct secret
    /// through it, e.g. trigger links: a flood of wrong guesses from
    /// elsewhere must not lock out a doorbell that knows its token).
    pub fn check_ip(&self, ip: Option<IpAddr>, now: Instant) -> Result<(), Duration> {
        match ip
            .and_then(|ip| self.per_ip.get(&ip))
            .and_then(|f| f.until)
            .filter(|t| *t > now)
        {
            Some(until) => Err(until - now),
            None => Ok(()),
        }
    }

    /// `Err(wait)` while the global brake is on.
    pub fn check_global(&self, now: Instant) -> Result<(), Duration> {
        match self.global_until.filter(|t| *t > now) {
            Some(t) => Err(t - now),
            None => Ok(()),
        }
    }

    pub fn failure(&mut self, ip: Option<IpAddr>, now: Instant) {
        while self
            .recent
            .front()
            .is_some_and(|t| now.duration_since(*t) > GLOBAL_WINDOW)
        {
            self.recent.pop_front();
        }
        self.recent.push_back(now);
        if self.recent.len() >= GLOBAL_FAILURES {
            self.global_until = Some(now + GLOBAL_LOCKOUT);
        }
        let Some(ip) = ip else { return };
        if self.per_ip.len() > 10_000 {
            self.per_ip
                .retain(|_, f| f.last.is_some_and(|l| now.duration_since(l) < MAX_LOCKOUT));
        }
        let f = self.per_ip.entry(ip).or_default();
        // A day without mistakes forgives old ones.
        if f.last
            .is_some_and(|l| now.duration_since(l) > Duration::from_secs(24 * 3600))
        {
            f.failures = 0;
        }
        f.failures += 1;
        f.last = Some(now);
        if f.failures >= FREE_TRIES {
            let doublings = (f.failures - FREE_TRIES).min(10);
            let wait = (FIRST_LOCKOUT * 2u32.pow(doublings)).min(MAX_LOCKOUT);
            f.until = Some(now + wait);
        }
    }

    pub fn success(&mut self, ip: Option<IpAddr>) {
        if let Some(ip) = ip {
            self.per_ip.remove(&ip);
        }
    }
}

#[derive(Default)]
pub struct Sessions {
    tokens: Mutex<HashMap<String, Instant>>,
    /// Token for local sidecars (`security::init_local_token`).
    local_token: OnceLock<String>,
    pub(crate) throttle: Mutex<Throttle>,
    /// Wrong trigger-link tokens (`services::hooks`): the same back-off as
    /// sign-in, kept apart so a misconfigured doorbell can't lock the owner
    /// out of the web UI.
    pub(crate) hook_throttle: Mutex<Throttle>,
}

impl Sessions {
    pub(crate) fn set_local_token(&self, token: String) {
        let _ = self.local_token.set(token);
    }

    pub(crate) fn create(&self) -> String {
        use rand::RngCore;
        let mut raw = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut raw);
        let token: String = raw.iter().map(|b| format!("{b:02x}")).collect();
        let mut map = self.tokens.lock();
        let now = Instant::now();
        map.retain(|_, exp| *exp > now);
        map.insert(token.clone(), now + SESSION_TTL);
        token
    }
    fn valid(&self, token: &str) -> bool {
        self.tokens
            .lock()
            .get(token)
            .is_some_and(|exp| *exp > Instant::now())
    }
    fn remove(&self, token: &str) {
        self.tokens.lock().remove(token);
    }
    pub fn clear(&self) {
        self.tokens.lock().clear();
    }
}

pub fn hash_password(password: &str) -> ApiResult<String> {
    if password.chars().count() < MIN_PASSWORD {
        return Err(ApiError::bad_request(format!(
            "Use at least {MIN_PASSWORD} characters."
        )));
    }
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(ApiError::internal)
}

pub fn verify_password(hash: &str, password: &str) -> bool {
    PasswordHash::new(hash)
        .map(|h| {
            Argon2::default()
                .verify_password(password.as_bytes(), &h)
                .is_ok()
        })
        .unwrap_or(false)
}

/// [`verify_password`] on a blocking thread, at most two at a time.
pub async fn verify_password_async(hash: &str, password: &str) -> bool {
    let Ok(_permit) = HASHING.acquire().await else {
        return false;
    };
    let (hash, password) = (hash.to_string(), password.to_string());
    tokio::task::spawn_blocking(move || verify_password(&hash, &password))
        .await
        .unwrap_or(false)
}

fn session_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_string())
}

pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// True when the request is authenticated (or no password is set): a valid
/// session. (Cluster keys and the local token are *not* enough.)
pub fn is_authenticated(state: &AppState, headers: &HeaderMap, peer: Option<SocketAddr>) -> bool {
    let _ = peer;
    if state.store.get().settings.security.password_hash.is_none() {
        return true;
    }
    session_token(headers).is_some_and(|t| state.sessions.valid(&t))
}

/// The request comes from a local sidecar: loopback, no proxy headers, and
/// the token of this daemon run.
pub fn is_local_sidecar(state: &AppState, headers: &HeaderMap, peer: Option<SocketAddr>) -> bool {
    let Some(token) = state.sessions.local_token.get() else {
        return false;
    };
    peer.is_some_and(|p| p.ip().is_loopback())
        && !super::security::forwarded(headers)
        && headers
            .get(LOCAL_HEADER)
            .is_some_and(|v| constant_time_eq(v.as_bytes(), token.as_bytes()))
}

/// [`is_authenticated`], or a local sidecar on one of its routes.
pub fn is_authenticated_for(
    state: &AppState,
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
    method: &axum::http::Method,
    path: &str,
) -> bool {
    is_authenticated(state, headers, peer)
        || (super::security::sidecar_route(method, path) && is_local_sidecar(state, headers, peer))
}

/// Middleware guarding `/api/v1/*`.
pub async fn require_auth(
    State(state): State<AppState>,
    peer: super::Peer,
    req: Request,
    next: Next,
) -> Response {
    // Inside `nest("/api/v1", ..)` the URI has the prefix stripped; use the
    // original URI so the open paths below actually match.
    let path = req
        .extensions()
        .get::<axum::extract::OriginalUri>()
        .map(|u| u.0.path().to_string())
        .unwrap_or_else(|| req.uri().path().to_string());
    let path = if path.starts_with("/api/v1/") || path == "/api/v1" {
        path
    } else {
        format!("/api/v1{path}")
    };
    let path = path.as_str();
    let unconfigured = state.identity().role == crate::node::LocalRole::Unconfigured;
    if path == "/api/v1/system/setup"
        && unconfigured
        && !peer.0.is_some_and(|p| super::security::lan_peer(p.ip()))
    {
        // Claiming a new controller only from the local network.
        return ApiError::forbidden("Set up this controller from your local network.")
            .into_response();
    }
    let open = path.starts_with("/api/v1/auth/")
        || path.starts_with("/api/v1/public/")
        // Trigger links carry their own secret (checked by `api::hooks`).
        || path.starts_with("/api/v1/hooks/")
        || path == "/api/v1/system"
        || path == "/api/v1/system/setup" && unconfigured
        // Cluster endpoints check their signatures themselves.
        || path.starts_with("/api/v1/cluster/");
    if open || is_authenticated_for(&state, req.headers(), peer.0, req.method(), path) {
        next.run(req).await
    } else {
        ApiError::unauthorized().into_response()
    }
}

#[derive(Deserialize)]
struct LoginBody {
    password: String,
}

fn too_many(wait: Duration) -> ApiError {
    let secs = wait.as_secs().max(1);
    let when = if secs >= 90 {
        format!("{} minutes", secs.div_ceil(60))
    } else {
        format!("{secs} seconds")
    };
    ApiError::new(
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        "too_many_attempts",
        format!("Too many wrong passwords. Try again in {when}."),
    )
}

async fn login(
    State(state): State<AppState>,
    peer: super::Peer,
    headers: HeaderMap,
    Json(body): Json<LoginBody>,
) -> ApiResult<Response> {
    let show = state.store.get();
    let Some(hash) = show.settings.security.password_hash.as_deref() else {
        return Ok(Json(serde_json::json!({ "ok": true })).into_response());
    };
    let ip = super::security::client_ip(
        peer.0,
        &headers,
        &show.settings.security.trusted_proxies,
        super::security::cf_trusted(&show.settings),
    );
    if let Err(wait) = state.sessions.throttle.lock().check(ip, Instant::now()) {
        return Err(too_many(wait));
    }
    if body.password.len() > 1024 || !verify_password_async(hash, &body.password).await {
        state.sessions.throttle.lock().failure(ip, Instant::now());
        // Slow down guessing.
        tokio::time::sleep(Duration::from_millis(600)).await;
        return Err(ApiError::new(
            axum::http::StatusCode::UNAUTHORIZED,
            "wrong_password",
            "That password isn't right.",
        ));
    }
    state.sessions.throttle.lock().success(ip);
    if let Some(via) = remote_via(peer.0, &headers, ip) {
        crate::services::alerts::remote_sign_in(&state, ip, via);
    }
    let token = state.sessions.create();
    let cookie = format!(
        "{COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
        SESSION_TTL.as_secs()
    );
    let mut resp = Json(serde_json::json!({ "ok": true })).into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(ApiError::internal)?,
    );
    Ok(resp)
}

/// How a sign-in reached this controller from outside the home network, or
/// `None` for one from the LAN. `client` is the visitor's address as
/// `security::client_ip` sees it (forwarding headers only from trusted proxies).
pub fn remote_via(
    peer: Option<SocketAddr>,
    headers: &HeaderMap,
    client: Option<IpAddr>,
) -> Option<&'static str> {
    let tunnel = super::security::tunnel_request(peer, headers);
    let tailscale_headers =
        headers.contains_key("tailscale-user-login") || headers.contains_key("tailscale-user-name");
    let tailnet = client.is_some_and(tailnet_addr) || peer.is_some_and(|p| tailnet_addr(p.ip()));
    if tunnel && headers.contains_key("cf-connecting-ip") {
        Some("Cloudflare Tunnel")
    } else if tailnet || (tunnel && tailscale_headers) {
        Some("Tailscale")
    } else if tunnel {
        Some("a tunnel or reverse proxy")
    } else if client.is_some_and(|ip| !super::security::lan_peer(ip)) {
        Some("the internet")
    } else {
        None
    }
}

/// A Tailscale address (`100.64.0.0/10`, `fd7a:115c:a1e0::/48`).
fn tailnet_addr(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => tailnet_addr(IpAddr::V4(v4)),
            None => v6.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
        },
    }
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(t) = session_token(&headers) {
        state.sessions.remove(&t);
    }
    let mut resp = Json(serde_json::json!({ "ok": true })).into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static("pp_session=; Path=/; HttpOnly; Max-Age=0"),
    );
    resp
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PasswordBody {
    /// Required when a password is already set.
    #[serde(default)]
    current: Option<String>,
    /// `None` or empty removes the password. (The web UI sends `password`.)
    #[serde(default, alias = "password")]
    new_password: Option<String>,
}

async fn set_password(
    State(state): State<AppState>,
    peer: super::Peer,
    headers: HeaderMap,
    Json(body): Json<PasswordBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let show = state.store.get();
    if let Some(hash) = show.settings.security.password_hash.as_deref() {
        let authed = is_authenticated(&state, &headers, peer.0);
        let ip = super::security::client_ip(
            peer.0,
            &headers,
            &show.settings.security.trusted_proxies,
            super::security::cf_trusted(&show.settings),
        );
        if let Err(wait) = state.sessions.throttle.lock().check(ip, Instant::now()) {
            return Err(too_many(wait));
        }
        let current_ok = match body.current.as_deref() {
            Some(c) if authed => verify_password_async(hash, c).await,
            _ => false,
        };
        if !(authed && current_ok) {
            if authed {
                state.sessions.throttle.lock().failure(ip, Instant::now());
            }
            return Err(ApiError::forbidden(
                "Enter your current password to change it.",
            ));
        }
    }
    let new_hash = match body.new_password.as_deref().filter(|p| !p.is_empty()) {
        Some(p) => Some(hash_password(p)?),
        None => None,
    };
    state
        .store
        .update(|s| {
            s.settings.security.password_hash = new_hash;
            Ok(())
        })
        .await?;
    state.sessions.clear();
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/password", put(set_password))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrip() {
        let h = hash_password("jingle").unwrap();
        assert!(verify_password(&h, "jingle"));
        assert!(!verify_password(&h, "bells"));
    }

    #[test]
    fn short_passwords_are_refused() {
        assert!(hash_password("12345").is_err());
        assert!(hash_password("123456").is_ok());
    }

    #[test]
    fn throttle_backs_off_per_address_and_globally() {
        let mut t = Throttle::default();
        let now = Instant::now();
        let a: IpAddr = "192.168.1.5".parse().unwrap();
        let b: IpAddr = "192.168.1.6".parse().unwrap();
        for _ in 0..FREE_TRIES - 1 {
            t.failure(Some(a), now);
            assert!(t.check(Some(a), now).is_ok());
        }
        t.failure(Some(a), now);
        assert_eq!(t.check(Some(a), now), Err(FIRST_LOCKOUT));
        assert!(
            t.check(Some(b), now).is_ok(),
            "other addresses are not affected"
        );
        assert!(t.check(Some(a), now + FIRST_LOCKOUT).is_ok());
        t.failure(Some(a), now + FIRST_LOCKOUT);
        assert_eq!(
            t.check(Some(a), now + FIRST_LOCKOUT),
            Err(FIRST_LOCKOUT * 2),
            "doubles"
        );
        t.success(Some(a));
        assert!(t.check(Some(a), now + FIRST_LOCKOUT).is_ok());
        // Many addresses: everyone waits a minute.
        let mut t = Throttle::default();
        for i in 0..GLOBAL_FAILURES {
            t.failure(
                Some(IpAddr::from([10, 0, (i / 250) as u8, (i % 250) as u8])),
                now,
            );
        }
        assert!(t.check(Some(b), now).is_err());
        assert!(t.check(Some(b), now + GLOBAL_LOCKOUT).is_ok());
    }

    #[test]
    fn remote_sign_ins_are_recognised() {
        let loopback: Option<SocketAddr> = Some("127.0.0.1:50000".parse().unwrap());
        let lan: Option<SocketAddr> = Some("192.168.1.20:50000".parse().unwrap());
        let ip = |s: &str| Some(s.parse::<IpAddr>().unwrap());
        let mut h = HeaderMap::new();
        // From the LAN: no alert.
        assert_eq!(remote_via(lan, &h, ip("192.168.1.20")), None);
        // Direct from a public address (port forward): alert.
        let public: Option<SocketAddr> = Some("203.0.113.9:4000".parse().unwrap());
        assert_eq!(
            remote_via(public, &h, ip("203.0.113.9")),
            Some("the internet")
        );
        // Over the tailnet directly.
        let ts: Option<SocketAddr> = Some("100.101.102.103:4000".parse().unwrap());
        assert_eq!(remote_via(ts, &h, ip("100.101.102.103")), Some("Tailscale"));
        // `tailscale serve` (loopback + forwarding + identity headers).
        h.insert("x-forwarded-for", HeaderValue::from_static("100.90.1.2"));
        h.insert(
            "tailscale-user-login",
            HeaderValue::from_static("me@example.com"),
        );
        assert_eq!(
            remote_via(loopback, &h, ip("100.90.1.2")),
            Some("Tailscale")
        );
        // cloudflared.
        let mut h = HeaderMap::new();
        h.insert("cf-connecting-ip", HeaderValue::from_static("198.51.100.4"));
        assert_eq!(
            remote_via(loopback, &h, ip("198.51.100.4")),
            Some("Cloudflare Tunnel")
        );
        // Some other local proxy.
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", HeaderValue::from_static("192.168.1.30"));
        assert_eq!(
            remote_via(loopback, &h, ip("192.168.1.30")),
            Some("a tunnel or reverse proxy")
        );
        // Local sidecars and the UI on this machine: no proxy headers, no alert.
        assert_eq!(
            remote_via(loopback, &HeaderMap::new(), ip("127.0.0.1")),
            None
        );
    }

    #[test]
    fn cookie_parsing() {
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            HeaderValue::from_static("a=1; pp_session=abc; b=2"),
        );
        assert_eq!(session_token(&h).as_deref(), Some("abc"));
    }
}
