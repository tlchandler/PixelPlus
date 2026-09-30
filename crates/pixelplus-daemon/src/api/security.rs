//! Browser-facing HTTP hardening (ARCHITECTURE §8, BUILDING "Security model"):
//!
//! * **Host allow-list** for `/api/v1/*` (except `/public/*`): IP literals,
//!   `localhost`, this controller's host name (`<name>` / `<name>.local`) and
//!   the names in Settings → Security → "Other names for this controller"
//!   (`settings.security.allowedHosts`, e.g. a tunnel domain) or
//!   `PIXELPLUS_ALLOWED_HOSTS`. Anything else gets `421` — this defeats DNS
//!   rebinding (a web page whose host name suddenly points at the Pi).
//! * **CSRF**: every state-changing API request (not GET/HEAD/OPTIONS) must carry
//!   `X-PixelPlus-Request: 1`. Browsers only send custom headers cross-site
//!   after a CORS preflight, which is never granted, so a foreign page can't
//!   post forms, text/plain or multipart bodies to the API.
//! * **WebSocket**: an `Origin` (browsers always send one) must match `Host`.
//! * **Response headers** on everything: CSP (script hashes of the static UI's
//!   inline bootstrap scripts), `nosniff`, `frame-ancestors 'none'` /
//!   `X-Frame-Options: DENY`, `Referrer-Policy`.
//! * **Client address** behind a local reverse proxy ([`client_ip`]).
//! * **Local sidecar token** (`/run/pixelplus/local-token`, [`init_local_token`]).

use crate::state::AppState;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Response};
use std::net::{IpAddr, SocketAddr};
use std::path::Path;

/// Header the web UI (and sidecars, and cluster calls) send on every request.
pub const REQUEST_HEADER: &str = "x-pixelplus-request";
/// Group shared with sidecars (overlay shared memory, local token).
pub const SIDECAR_GROUP: &str = "pixelplus-overlay";

// ---------------------------------------------------------------------------
// Client address
// ---------------------------------------------------------------------------

fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    }
}

/// `ip` matches `a.b.c.d`, `a.b.c.d/nn` or an IPv6 address / prefix.
fn in_net(ip: IpAddr, spec: &str) -> bool {
    let spec = spec.trim();
    let (addr, bits) = match spec.split_once('/') {
        Some((a, b)) => (a, b.parse::<u32>().ok()),
        None => (spec, None),
    };
    let Ok(net) = addr.parse::<IpAddr>() else {
        return false;
    };
    match (canonical(ip), canonical(net)) {
        (IpAddr::V4(a), IpAddr::V4(n)) => {
            let bits = bits.unwrap_or(32).min(32);
            let mask = if bits == 0 {
                0
            } else {
                u32::MAX << (32 - bits)
            };
            u32::from(a) & mask == u32::from(n) & mask
        }
        (IpAddr::V6(a), IpAddr::V6(n)) => {
            let bits = bits.unwrap_or(128).min(128);
            let mask = if bits == 0 {
                0
            } else {
                u128::MAX << (128 - bits)
            };
            u128::from(a) & mask == u128::from(n) & mask
        }
        _ => false,
    }
}

fn trusted_proxy(ip: IpAddr, trusted: &[String]) -> bool {
    ip.is_loopback() || trusted.iter().any(|t| in_net(ip, t))
}

/// The visitor's address. Forwarding headers are believed only from a trusted
/// proxy (this machine, or `settings.security.trustedProxies`):
/// `CF-Connecting-IP` from a local cloudflared, otherwise the right-most
/// `X-Forwarded-For` entry that is not itself a trusted proxy (proxies append
/// the address they saw; everything left of it is what the client claimed).
pub fn client_ip(
    peer: Option<SocketAddr>,
    headers: &HeaderMap,
    trusted: &[String],
) -> Option<IpAddr> {
    let direct = canonical(peer?.ip());
    if !trusted_proxy(direct, trusted) {
        return Some(direct);
    }
    if direct.is_loopback() {
        if let Some(ip) = headers
            .get("cf-connecting-ip")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<IpAddr>().ok())
        {
            return Some(canonical(ip));
        }
    }
    let hops: Vec<IpAddr> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|v| v.trim().parse::<IpAddr>().ok())
        .map(canonical)
        .collect();
    for ip in hops.iter().rev() {
        if !trusted_proxy(*ip, trusted) {
            return Some(*ip);
        }
    }
    Some(hops.first().copied().unwrap_or(direct))
}

/// Carries any header a reverse proxy adds (such requests never get the
/// sidecar's local-token trust).
pub fn forwarded(headers: &HeaderMap) -> bool {
    [
        "x-forwarded-for",
        "forwarded",
        "cf-connecting-ip",
        "x-real-ip",
        "x-forwarded-host",
    ]
    .iter()
    .any(|h| headers.contains_key(*h))
}

/// A peer on this machine's networks: loopback, private/link-local/CGNAT
/// ranges, IPv6 ULA / link-local, or a directly attached subnet.
pub fn lan_peer(ip: IpAddr) -> bool {
    match canonical(ip) {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64)
                || crate::cluster::net::on_local_subnet(ip)
        }
        IpAddr::V6(v6) => {
            let seg = v6.segments()[0];
            v6.is_loopback()
                || (seg & 0xfe00) == 0xfc00
                || (seg & 0xffc0) == 0xfe80
                || crate::cluster::net::on_local_subnet(ip)
        }
    }
}

// ---------------------------------------------------------------------------
// Host allow-list, CSRF header, WebSocket origin
// ---------------------------------------------------------------------------

/// Host name part of a `Host` header value (no port, lower case).
fn host_name(host: &str) -> String {
    let host = host.trim();
    let name = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or_default()
    } else if host.matches(':').count() == 1 {
        host.split(':').next().unwrap_or_default()
    } else {
        host
    };
    name.trim_end_matches('.').to_ascii_lowercase()
}

fn name_matches(name: &str, pattern: &str) -> bool {
    let pattern = pattern.trim().trim_end_matches('.').to_ascii_lowercase();
    if pattern.is_empty() {
        return false;
    }
    if pattern == "*" {
        return true;
    }
    match pattern.strip_prefix("*.") {
        Some(suffix) => name.len() > suffix.len() && name.ends_with(&format!(".{suffix}")),
        None => name == pattern,
    }
}

/// May the API be used under this `Host`?
pub fn host_allowed(host: &str, hostname: &str, node_id: &str, extra: &[String]) -> bool {
    let name = host_name(host);
    if name.is_empty() || name.parse::<IpAddr>().is_ok() {
        return true;
    }
    if name == "localhost" || name.ends_with(".localhost") {
        return true;
    }
    let me = hostname.trim().to_ascii_lowercase();
    if !me.is_empty() && (name == me || name == format!("{me}.local")) {
        return true;
    }
    // The name the built-in mDNS responder uses in Docker (cluster::discovery).
    let short: String = node_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect::<String>()
        .to_ascii_lowercase();
    if !short.is_empty() && name == format!("pixelplus-{short}.local") {
        return true;
    }
    let env = std::env::var("PIXELPLUS_ALLOWED_HOSTS").unwrap_or_default();
    extra
        .iter()
        .map(String::as_str)
        .chain(env.split([',', ' ']))
        .any(|p| name_matches(&name, p))
}

/// How long the old host name keeps working after the controller was renamed:
/// open pages (and the browser's mDNS cache) still use it for a while.
const PREVIOUS_NAME_GRACE: std::time::Duration = std::time::Duration::from_secs(30 * 60);

static PREVIOUS_HOSTNAME: parking_lot::Mutex<Option<(String, std::time::Instant)>> =
    parking_lot::const_mutex(None);

/// The controller was renamed (Settings → Network): keep answering to `old`
/// for [`PREVIOUS_NAME_GRACE`], so the page that made the change doesn't
/// suddenly get "unknown address" for every request.
pub fn remember_previous_hostname(old: &str) {
    let old = old.trim().to_ascii_lowercase();
    if !old.is_empty() {
        *PREVIOUS_HOSTNAME.lock() = Some((old, std::time::Instant::now()));
    }
}

/// The previous host name while its grace period lasts.
fn previous_hostname(now: std::time::Instant) -> Option<String> {
    PREVIOUS_HOSTNAME
        .lock()
        .as_ref()
        .filter(|(_, at)| now.duration_since(*at) < PREVIOUS_NAME_GRACE)
        .map(|(n, _)| n.clone())
}

fn misdirected(host: &str, wants_html: bool) -> Response {
    let name = host_name(host);
    if wants_html {
        let safe: String = name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
            .take(253)
            .collect();
        let page = format!(
            "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width\"><title>PixelPlus</title>\
             <body style=\"font-family:system-ui,sans-serif;max-width:36em;margin:3em auto;padding:0 1em;line-height:1.5\">\
             <h1>Unknown address</h1><p>This PixelPlus controller doesn't answer to <b>{safe}</b>.</p>\
             <p>Open it by its IP address or <code>&lt;name&gt;.local</code>. If you reach it through a tunnel or your own domain, \
             add that name under <b>Settings → Security → Other names for this controller</b>.</p></body>"
        );
        return (StatusCode::MISDIRECTED_REQUEST, Html(page)).into_response();
    }
    super::ApiError::new(
        StatusCode::MISDIRECTED_REQUEST,
        "unknown_host",
        format!("This controller doesn't answer to “{name}”. Add it under Settings → Security → Other names for this controller."),
    )
    .into_response()
}

/// Is the WebSocket `Origin` (if any) the page this server served?
pub fn origin_matches(origin: Option<&str>, host: Option<&str>) -> bool {
    let Some(origin) = origin else {
        return true; // not a browser
    };
    let Some(host) = host else {
        return false;
    };
    let Ok(url) = reqwest::Url::parse(origin) else {
        return false;
    };
    let Some(oh) = url.host_str() else {
        return false;
    };
    let origin_host = match url.port() {
        Some(p) => format!("{oh}:{p}"),
        None => oh.to_string(),
    };
    let strip_default = |h: &str| {
        h.trim()
            .trim_end_matches(":80")
            .trim_end_matches(":443")
            .to_ascii_lowercase()
    };
    strip_default(&origin_host) == strip_default(host)
}

/// Middleware in front of `/api/v1`: Host allow-list, CSRF header,
/// WebSocket origin. (Authentication follows in `auth::require_auth`.)
pub async fn guard(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let path = req
        .extensions()
        .get::<axum::extract::OriginalUri>()
        .map(|u| u.0.path().to_string())
        .unwrap_or_else(|| req.uri().path().to_string());
    let public = path.starts_with("/api/v1/public/");
    let headers = req.headers();
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    if !public {
        if let Some(h) = &host {
            let mut extra = state.store.get().settings.security.allowed_hosts.clone();
            if let Some(old) = previous_hostname(std::time::Instant::now()) {
                extra.push(format!("{old}.local"));
                extra.push(old);
            }
            let hostname = crate::cluster::net::hostname();
            if !host_allowed(h, &hostname, &state.identity().id, &extra) {
                let wants_html = headers
                    .get(header::ACCEPT)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|a| a.contains("text/html"));
                return misdirected(h, wants_html);
            }
        }
        let safe = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
        if !safe && !headers.get(REQUEST_HEADER).is_some_and(|v| v == "1") {
            return super::ApiError::new(
                StatusCode::FORBIDDEN,
                "csrf",
                "This request didn't come from the PixelPlus app (missing X-PixelPlus-Request header).",
            )
            .into_response();
        }
    }
    if path == "/api/v1/ws" {
        let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
        if !origin_matches(origin, host.as_deref()) {
            return super::ApiError::forbidden("WebSocket from another site.").into_response();
        }
    }
    next.run(req).await
}

// ---------------------------------------------------------------------------
// CSRF exemption for the root-mounted xLights FPP Connect API (F16, WS6)
// ---------------------------------------------------------------------------

/// Is this request "CORS-simple", i.e. one a foreign web page could send
/// cross-site **without** a preflight? (GET/HEAD/POST with a form, multipart or
/// text/plain body.) Anything else (PATCH/PUT/DELETE, or a POST with another
/// content type) needs a preflight, which this server never grants, so a
/// browser can't forge it.
pub fn cors_simple(method: &Method, headers: &HeaderMap) -> bool {
    match *method {
        Method::GET | Method::HEAD => true,
        Method::POST => {
            let ct = headers
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            ct.is_empty()
                || ct == "application/x-www-form-urlencoded"
                || ct == "multipart/form-data"
                || ct == "text/plain"
        }
        _ => false,
    }
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim().trim_end_matches('=');
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The password of an `Authorization: Basic …` header (any user name; xLights
/// sends `admin`).
pub fn basic_auth_password(headers: &HeaderMap) -> Option<String> {
    let v = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, rest) = v.trim().split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let raw = String::from_utf8(base64_decode(rest)?).ok()?;
    raw.split_once(':').map(|(_, p)| p.to_string())
}

/// Recently verified upload passwords: `(sha256(hash ‖ 0 ‖ password), until)`.
/// xLights uploads in 16 MiB chunks; an Argon2 check per chunk would cost
/// ~50 ms each, so a verified password is remembered for 10 minutes (only a
/// digest, bound to the stored hash, so changing the password invalidates it).
static FPP_VERIFIED: parking_lot::Mutex<Vec<([u8; 32], std::time::Instant)>> =
    parking_lot::const_mutex(Vec::new());
const FPP_VERIFIED_TTL: std::time::Duration = std::time::Duration::from_secs(600);

fn fpp_digest(hash: &str, password: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(hash.as_bytes());
    h.update([0u8]);
    h.update(password.as_bytes());
    h.finalize().into()
}

/// **CSRF-exemption hook for WS6's root-mounted FPP Connect routes**
/// (`api/fppcompat.rs`, ARCHITECTURE §12.14). Those routes live outside
/// `/api/v1`, so neither [`guard`] (Host allow-list, `X-PixelPlus-Request`)
/// nor `auth::require_auth` runs for them, and xLights can't send the CSRF
/// header. Call this first in every such handler (or as a `from_fn_with_state`
/// layer on the fppcompat router) and return the `Err` response as is.
///
/// Rules, in order:
/// 1. `settings.xlights.fppConnect` off → `404` (the feature is invisible).
/// 2. Only from this network: the peer must be a LAN address ([`lan_peer`])
///    and the request must not carry proxy headers ([`forwarded`]) — so a
///    tunnel, `tailscale serve` or the public listener can never reach it →
///    `404`.
/// 3. Host allow-list as for `/api/v1` ([`host_allowed`]; DNS rebinding) →
///    `421`.
/// 4. Reads (GET/HEAD) pass (xLights probes `/config.php`, meta files).
/// 5. Writes: when an upload password is set (`settings.xlights.passwordHash`)
///    it must come as HTTP Basic auth (any user name; the sign-in throttle
///    applies) → else `401` with `WWW-Authenticate: Basic`. Without a
///    password, writes are accepted only when they are not CORS-simple
///    ([`cors_simple`]: xLights uses `PATCH` and JSON `POST`s), which a foreign
///    web page can't send → else `403 csrf`.
#[allow(dead_code)] // called by api/fppcompat.rs (WS6)
pub async fn fpp_compat_authorize(
    state: &AppState,
    peer: Option<SocketAddr>,
    method: &Method,
    headers: &HeaderMap,
) -> Result<(), Response> {
    let settings = state.store.get().settings.clone();
    let not_found = || super::ApiError::not_found("That page").into_response();
    if !settings.xlights.fpp_connect {
        return Err(not_found());
    }
    let Some(peer) = peer else {
        return Err(not_found());
    };
    if forwarded(headers) || !lan_peer(peer.ip()) {
        return Err(not_found());
    }
    if let Some(h) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) {
        let hostname = crate::cluster::net::hostname();
        if !host_allowed(
            h,
            &hostname,
            &state.identity().id,
            &settings.security.allowed_hosts,
        ) {
            return Err(misdirected(h, false));
        }
    }
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return Ok(());
    }
    let hash = settings
        .xlights
        .password_hash
        .as_deref()
        .filter(|h| !h.is_empty());
    let Some(hash) = hash else {
        if cors_simple(method, headers) {
            return Err(super::ApiError::new(
                StatusCode::FORBIDDEN,
                "csrf",
                "Uploads must use PATCH or a JSON body (or set an xLights upload password).",
            )
            .into_response());
        }
        return Ok(());
    };
    let unauthorized = |msg: &str| {
        let mut r = super::ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized", msg)
            .into_response();
        r.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"PixelPlus xLights upload\""),
        );
        r
    };
    let Some(password) = basic_auth_password(headers) else {
        return Err(unauthorized(
            "Enter the PixelPlus xLights upload password in xLights (FPP Connect).",
        ));
    };
    let digest = fpp_digest(hash, &password);
    let now = std::time::Instant::now();
    {
        let mut cache = FPP_VERIFIED.lock();
        cache.retain(|(_, until)| *until > now);
        if cache.iter().any(|(d, _)| *d == digest) {
            return Ok(());
        }
    }
    let ip = Some(peer.ip());
    if let Err(wait) = state.sessions.throttle.lock().check(ip, now) {
        return Err(super::ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "throttled",
            format!("Too many wrong passwords. Try again in {} s.", wait.as_secs().max(1)),
        )
        .into_response());
    }
    if super::auth::verify_password_async(hash, &password).await {
        state.sessions.throttle.lock().success(ip);
        let mut cache = FPP_VERIFIED.lock();
        if cache.len() >= 16 {
            cache.remove(0);
        }
        cache.push((digest, now + FPP_VERIFIED_TTL));
        Ok(())
    } else {
        state.sessions.throttle.lock().failure(ip, now);
        Err(unauthorized("Wrong xLights upload password."))
    }
}

/// [`fpp_compat_authorize`] as a middleware, for
/// `Router::layer(axum::middleware::from_fn_with_state(state, security::fpp_compat_guard))`
/// on the root-mounted fppcompat router.
#[allow(dead_code)] // used by api/fppcompat.rs (WS6)
pub async fn fpp_compat_guard(
    State(state): State<AppState>,
    peer: super::Peer,
    req: Request,
    next: Next,
) -> Response {
    if let Err(r) = fpp_compat_authorize(&state, peer.0, req.method(), req.headers()).await {
        return r;
    }
    next.run(req).await
}

// ---------------------------------------------------------------------------
// Response headers
// ---------------------------------------------------------------------------

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// `'sha256-…'` sources for every inline `<script>` in `html`.
pub fn inline_script_hashes(html: &str) -> Vec<String> {
    use sha2::{Digest, Sha256};
    let mut out = Vec::new();
    let lower = html.to_ascii_lowercase();
    let mut pos = 0;
    while let Some(start) = lower[pos..].find("<script") {
        let tag_start = pos + start;
        let Some(tag_end) = lower[tag_start..].find('>').map(|e| tag_start + e) else {
            break;
        };
        let Some(close) = lower[tag_end..].find("</script").map(|e| tag_end + e) else {
            break;
        };
        let attrs = &lower[tag_start + 7..tag_end];
        if !attrs.contains("src=") {
            let body = &html[tag_end + 1..close];
            out.push(format!(
                "'sha256-{}'",
                base64(&Sha256::digest(body.as_bytes()))
            ));
        }
        pos = close;
    }
    out
}

fn collect_html(dir: &Path, depth: u32, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() && depth > 0 {
            collect_html(&p, depth - 1, out);
        } else if p.extension().is_some_and(|x| x == "html") {
            if let Ok(text) = std::fs::read_to_string(&p) {
                for h in inline_script_hashes(&text) {
                    if !out.contains(&h) {
                        out.push(h);
                    }
                }
            }
        }
    }
}

/// The Content-Security-Policy for the UI in `web_dir` (`PIXELPLUS_CSP`
/// overrides it; `off` disables it).
pub fn content_security_policy(web_dir: &Path) -> Option<String> {
    if let Ok(v) = std::env::var("PIXELPLUS_CSP") {
        let v = v.trim().to_string();
        return (!v.is_empty() && v != "off").then_some(v);
    }
    let mut hashes = Vec::new();
    collect_html(web_dir, 3, &mut hashes);
    // Browser TTS (kokoro-js / transformers.js) loads its model from Hugging
    // Face and the onnxruntime loader from jsDelivr (@huggingface scope only).
    Some(format!(
        "default-src 'self'; \
         script-src 'self' {} 'wasm-unsafe-eval' https://cdn.jsdelivr.net/npm/@huggingface/; \
         style-src 'self' 'unsafe-inline'; \
         img-src 'self' data: blob:; media-src 'self' data: blob:; font-src 'self' data:; \
         connect-src 'self' ws: wss: https://huggingface.co https://*.huggingface.co https://*.hf.co https://cdn.jsdelivr.net/npm/@huggingface/; \
         worker-src 'self' blob:; object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'",
        hashes.join(" ")
    ))
}

/// The CSP for the UI in `web_dir`, recomputed whenever `index.html` changes
/// (a rebuilt UI has new inline-script hashes; dev servers and Docker volume
/// setups rebuild while the daemon runs).
pub struct CspCache {
    web_dir: std::path::PathBuf,
    cached: parking_lot::Mutex<Option<(Option<FileStamp>, Option<HeaderValue>)>>,
}

type FileStamp = (std::time::SystemTime, u64);

impl CspCache {
    pub fn new(web_dir: &Path) -> Self {
        CspCache {
            web_dir: web_dir.to_path_buf(),
            cached: Default::default(),
        }
    }

    fn stamp(&self) -> Option<FileStamp> {
        let m = std::fs::metadata(self.web_dir.join("index.html")).ok()?;
        Some((m.modified().ok()?, m.len()))
    }

    pub fn get(&self) -> Option<HeaderValue> {
        let stamp = self.stamp();
        let mut cached = self.cached.lock();
        if let Some((s, v)) = cached.as_ref() {
            if *s == stamp {
                return v.clone();
            }
        }
        let v = content_security_policy(&self.web_dir).and_then(|v| HeaderValue::from_str(&v).ok());
        *cached = Some((stamp, v.clone()));
        v
    }
}

/// Add the security headers to every response.
pub async fn headers(
    State(csp): State<std::sync::Arc<CspCache>>,
    req: Request,
    next: Next,
) -> Response {
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    if !h.contains_key(header::CONTENT_SECURITY_POLICY) {
        if let Some(v) = csp.get() {
            h.insert(header::CONTENT_SECURITY_POLICY, v);
        }
    }
    resp
}

// ---------------------------------------------------------------------------
// Local sidecar token
// ---------------------------------------------------------------------------

/// Group id of [`SIDECAR_GROUP`], if that group exists.
pub fn sidecar_gid() -> Option<u32> {
    #[cfg(unix)]
    {
        let name = std::ffi::CString::new(SIDECAR_GROUP).ok()?;
        // SAFETY: getgrnam returns a pointer into static storage or null; we
        // only read gr_gid immediately.
        let gr = unsafe { libc::getgrnam(name.as_ptr()) };
        if gr.is_null() {
            return None;
        }
        Some(unsafe { (*gr).gr_gid })
    }
    #[cfg(not(unix))]
    None
}

/// Where the local token is written: `PIXELPLUS_LOCAL_TOKEN_FILE`, else
/// `/run/pixelplus/local-token` when `/run/pixelplus` exists.
pub fn local_token_path() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("PIXELPLUS_LOCAL_TOKEN_FILE") {
        return (!p.trim().is_empty()).then(|| p.trim().into());
    }
    let run = Path::new("/run/pixelplus");
    run.is_dir().then(|| run.join("local-token"))
}

/// Create this run's sidecar token and write it where sidecars read it
/// (0640, group [`SIDECAR_GROUP`] when it exists).
pub fn init_local_token(state: &AppState) {
    let token = crate::cluster::sig::random_hex(32);
    if let Some(path) = local_token_path() {
        if let Err(e) = write_token(&path, &token) {
            tracing::warn!(
                "could not write {} ({e}); local sidecars need a session",
                path.display()
            );
        }
    }
    state.sessions.set_local_token(token);
}

fn write_token(path: &Path, token: &str) -> std::io::Result<()> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension(format!("tmp-{}", crate::cluster::sig::random_hex(4)));
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    o.mode(0o640).custom_flags(libc::O_NOFOLLOW);
    let mut f = o.open(&tmp)?;
    #[cfg(unix)]
    if let Some(gid) = sidecar_gid() {
        use std::os::fd::AsRawFd;
        // SAFETY: plain fchown on our own open file descriptor.
        let _ = unsafe { libc::fchown(f.as_raw_fd(), u32::MAX, gid) };
    }
    f.write_all(token.as_bytes())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)
}

/// Routes a local sidecar (games) may use with the local token: read the show
/// (secrets are redacted) and player state, pause/resume/stop, overlays,
/// the event WebSocket. Nothing else (no settings, network, SSH, updates…).
pub fn sidecar_route(method: &Method, path: &str) -> bool {
    let get = *method == Method::GET || *method == Method::HEAD;
    match path {
        "/api/v1/show" | "/api/v1/player" | "/api/v1/system" | "/api/v1/ws" => get,
        "/api/v1/player/pause" | "/api/v1/player/resume" | "/api/v1/player/stop" => {
            *method == Method::POST
        }
        p => p.starts_with("/api/v1/overlay/") && !p.contains(".."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.append(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        m
    }

    #[test]
    fn client_ip_trusts_only_local_proxies() {
        let lan: SocketAddr = "192.168.1.9:5000".parse().unwrap();
        let lo: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let spoof = h(&[("x-forwarded-for", "1.2.3.4")]);
        // A LAN client can't pick its address.
        assert_eq!(
            client_ip(Some(lan), &spoof, &[]),
            Some("192.168.1.9".parse().unwrap())
        );
        // Behind a local proxy the right-most hop (what the proxy saw) counts,
        // not the left-most value the client sent.
        let appended = h(&[("x-forwarded-for", "6.6.6.6, 203.0.113.7")]);
        assert_eq!(
            client_ip(Some(lo), &appended, &[]),
            Some("203.0.113.7".parse().unwrap())
        );
        let cf = h(&[
            ("cf-connecting-ip", "198.51.100.2"),
            ("x-forwarded-for", "6.6.6.6"),
        ]);
        assert_eq!(
            client_ip(Some(lo), &cf, &[]),
            Some("198.51.100.2".parse().unwrap())
        );
        // A configured LAN proxy (e.g. a NAS) is trusted too, but not for CF-Connecting-IP.
        let nas = vec!["192.168.1.0/24".to_string()];
        assert_eq!(
            client_ip(Some(lan), &appended, &nas),
            Some("203.0.113.7".parse().unwrap())
        );
        assert_eq!(
            client_ip(Some(lan), &cf, &nas),
            Some("6.6.6.6".parse().unwrap())
        );
        let chain = h(&[("x-forwarded-for", "203.0.113.7, 192.168.1.2")]);
        assert_eq!(
            client_ip(Some(lo), &chain, &nas),
            Some("203.0.113.7".parse().unwrap())
        );
        assert_eq!(
            client_ip(Some(lo), &h(&[]), &[]),
            Some("127.0.0.1".parse().unwrap())
        );
        assert_eq!(client_ip(None, &spoof, &[]), None);
    }

    #[test]
    fn hosts() {
        let ok = |host: &str, extra: &[&str]| {
            let extra: Vec<String> = extra.iter().map(|s| s.to_string()).collect();
            host_allowed(host, "pixelplus-garage", "01j9zqxyab", &extra)
        };
        assert!(ok("192.168.1.20", &[]));
        assert!(ok("192.168.1.20:8080", &[]));
        assert!(ok("[fe80::1]:80", &[]));
        assert!(ok("localhost:5173", &[]));
        assert!(ok("PixelPlus-Garage.local", &[]));
        assert!(ok("pixelplus-garage.local.", &[]));
        assert!(ok("pixelplus-garage", &[]));
        assert!(ok("pixelplus-01j9zqxy.local", &[]));
        assert!(!ok("evil.example.com", &[]));
        assert!(!ok("pixelplus-garage.local.evil.com", &[]));
        assert!(!ok("other.local", &[]));
        assert!(ok("lights.example.com", &["lights.example.com"]));
        assert!(ok("a.b.tunnel.dev", &["*.tunnel.dev"]));
        assert!(!ok("tunnel.dev", &["*.tunnel.dev"]));
        assert!(ok("anything.example", &["*"]));
    }

    #[test]
    fn a_renamed_controller_answers_to_its_old_name_for_a_while() {
        let t0 = std::time::Instant::now();
        remember_previous_hostname("PixelPlus-Old");
        assert_eq!(previous_hostname(t0).as_deref(), Some("pixelplus-old"));
        assert_eq!(
            previous_hostname(t0 + PREVIOUS_NAME_GRACE + std::time::Duration::from_secs(1)),
            None
        );
    }

    #[test]
    fn websocket_origin() {
        assert!(origin_matches(None, Some("x")));
        assert!(origin_matches(
            Some("http://192.168.1.2"),
            Some("192.168.1.2")
        ));
        assert!(origin_matches(
            Some("http://192.168.1.2:8080"),
            Some("192.168.1.2:8080")
        ));
        assert!(origin_matches(Some("http://pp.local"), Some("pp.local:80")));
        assert!(!origin_matches(
            Some("http://evil.com"),
            Some("192.168.1.2")
        ));
        assert!(!origin_matches(
            Some("http://192.168.1.2:8088"),
            Some("192.168.1.2")
        ));
        assert!(!origin_matches(Some("null"), Some("192.168.1.2")));
        assert!(!origin_matches(Some("http://a"), None));
    }

    #[test]
    fn csp_hashes_inline_scripts() {
        let html = "<html><script>alert(1)</script><script type=module src=\"/x.js\"></script><SCRIPT>b()</SCRIPT></html>";
        let hashes = inline_script_hashes(html);
        assert_eq!(hashes.len(), 2);
        assert_eq!(
            hashes[0],
            "'sha256-bhHHL3z2vDgxUt0W3dWQOrprscmda2Y5pLsLg4GF+pI='"
        );
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
    }

    #[test]
    fn csp_follows_rebuilds_of_the_ui() {
        let dir = std::env::temp_dir().join(format!("pp-csp-{}", pixelplus_core::model::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let index = dir.join("index.html");
        std::fs::write(&index, "<script>one()</script>").unwrap();
        let cache = CspCache::new(&dir);
        let first = cache.get().unwrap();
        let hash = |js: &str| inline_script_hashes(&format!("<script>{js}</script>")).remove(0);
        assert!(first.to_str().unwrap().contains(&hash("one()")));
        assert_eq!(cache.get(), Some(first.clone()), "cached while unchanged");
        // A rebuild while the daemon runs.
        std::fs::write(&index, "<script>two(2)</script>").unwrap();
        let f = std::fs::File::options().append(true).open(&index).unwrap();
        f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
            .unwrap();
        let second = cache.get().unwrap();
        assert!(second.to_str().unwrap().contains(&hash("two(2)")));
        assert!(!second.to_str().unwrap().contains(&hash("one()")));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn sidecar_scope() {
        assert!(sidecar_route(&Method::GET, "/api/v1/show"));
        assert!(sidecar_route(&Method::POST, "/api/v1/player/pause"));
        assert!(sidecar_route(&Method::PUT, "/api/v1/overlay/m1/frame"));
        assert!(sidecar_route(&Method::GET, "/api/v1/ws"));
        assert!(!sidecar_route(&Method::PUT, "/api/v1/show/settings"));
        assert!(!sidecar_route(&Method::PUT, "/api/v1/system/ssh"));
        assert!(!sidecar_route(&Method::POST, "/api/v1/system/update"));
        assert!(!sidecar_route(&Method::POST, "/api/v1/player/play"));
        assert!(!sidecar_route(&Method::POST, "/api/v1/show"));
    }

    #[test]
    fn lan_peers() {
        assert!(lan_peer("192.168.0.5".parse().unwrap()));
        assert!(lan_peer("10.1.2.3".parse().unwrap()));
        assert!(lan_peer("::1".parse().unwrap()));
        assert!(lan_peer("fd00::5".parse().unwrap()));
        assert!(!lan_peer("8.8.8.8".parse().unwrap()));
    }
}
