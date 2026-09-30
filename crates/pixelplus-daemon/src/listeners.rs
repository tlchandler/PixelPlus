//! Extra HTTP listeners besides the main `:80` one (ARCHITECTURE §12.1):
//!
//! * **HTTPS** on `Config.https_port` (443; Docker 8443; `0` = off): the
//!   *same* router as `:80`, over TLS with the leaf certificate from
//!   `services::tls` (hot-swapped on re-issue). Runs while
//!   `settings.https.enabled` and this node is not a follower. Responses get
//!   `Secure` on their cookies; handlers can tell HTTPS requests by the
//!   [`ViaHttps`] request extension. `:80` is never redirected: followers,
//!   sidecars and old bookmarks use it.
//! * **Public-only** on `127.0.0.1:Config.public_port` (8081; `0` = off) for
//!   tunnels (F14): [`public_router`] = the full router behind
//!   `api::security::public_only` (WS5's allow-list), plus the games phone
//!   controller proxied under `/play/` (HTTP and WebSocket). Serves only
//!   while `settings.remote.publicListener` is on.
//!
//! Both bind lazily and retry, so a busy or privileged port never stops the
//! daemon; `/tls/status` reports what happened.

use crate::state::AppState;
use axum::extract::Request;
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::serve::{Listener, ListenerExt};
use axum::Router;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};

/// Request extension: this request arrived over the HTTPS listener.
#[derive(Debug, Clone, Copy)]
pub struct ViaHttps;

/// Time allowed for a TLS handshake / a request head on the public listener.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Concurrent TLS handshakes (a flood of half-open connections can't starve the rest).
const MAX_HANDSHAKES: usize = 64;
/// Largest request head the public listener peeks at.
const MAX_HEAD: usize = 16 * 1024;
/// Open connections the public listener accepts at once (page requests and
/// games bridges together). A visitor on the internet can open many slow
/// connections through a tunnel; past this they get a quick 503 instead of
/// using up the daemon's file descriptors.
pub const MAX_PUBLIC_CONNS: usize = 256;
/// A public page request (head and body) must have arrived within this long
/// of connecting; every such connection serves one request, so a visitor
/// trickling a request body can't hold a slot for ever.
pub const PUBLIC_READ_DEADLINE: Duration = Duration::from_secs(30);
/// Retry a failed bind this often.
const REBIND_EVERY: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// HTTPS
// ---------------------------------------------------------------------------

/// Accepts TCP connections, completes TLS handshakes in parallel and hands
/// finished streams to axum.
pub struct TlsListener {
    rx: mpsc::Receiver<(tokio_rustls::server::TlsStream<TcpStream>, SocketAddr)>,
    local: SocketAddr,
}

impl TlsListener {
    /// Serve TLS on `tcp` with `config`; connections are accepted only
    /// while `gate()` is true (otherwise closed straight away).
    pub fn new(
        tcp: TcpListener,
        config: Arc<tokio_rustls::rustls::ServerConfig>,
        gate: impl Fn() -> bool + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        let local = tcp.local_addr()?;
        let (tx, rx) = mpsc::channel(64);
        let acceptor = tokio_rustls::TlsAcceptor::from(config);
        let permits = Arc::new(tokio::sync::Semaphore::new(MAX_HANDSHAKES));
        tokio::spawn(async move {
            loop {
                let (tcp, peer) = match tcp.accept().await {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::debug!("HTTPS accept failed: {e}");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                if tx.is_closed() {
                    return;
                }
                if !gate() {
                    drop(tcp);
                    continue;
                }
                let Ok(permit) = permits.clone().try_acquire_owned() else {
                    drop(tcp);
                    continue;
                };
                let acceptor = acceptor.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let _ = tcp.set_nodelay(true);
                    match tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await {
                        Ok(Ok(tls)) => {
                            let _ = tx.send((tls, peer)).await;
                        }
                        // Phones that don't trust the certificate yet abort the
                        // handshake: that's normal, not worth a warning.
                        Ok(Err(e)) => tracing::debug!("HTTPS handshake with {peer} failed: {e}"),
                        Err(_) => tracing::debug!("HTTPS handshake with {peer} timed out"),
                    }
                });
            }
        });
        Ok(TlsListener { rx, local })
    }
}

impl Listener for TlsListener {
    type Io = tokio_rustls::server::TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.rx.recv().await {
            Some(c) => c,
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.local)
    }
}

/// Marks HTTPS requests ([`ViaHttps`]) and makes their cookies `Secure`.
pub async fn https_layer(mut req: Request, next: Next) -> Response {
    req.extensions_mut().insert(ViaHttps);
    let mut resp = next.run(req).await;
    secure_cookies(resp.headers_mut());
    resp
}

/// Add `; Secure` to every `Set-Cookie` that lacks it.
pub fn secure_cookies(headers: &mut axum::http::HeaderMap) {
    let cookies: Vec<HeaderValue> = headers
        .get_all(header::SET_COOKIE)
        .iter()
        .cloned()
        .collect();
    if cookies.is_empty() {
        return;
    }
    headers.remove(header::SET_COOKIE);
    for c in cookies {
        let v = match c.to_str() {
            Ok(s) if !s.to_ascii_lowercase().contains("; secure") => {
                HeaderValue::from_str(&format!("{s}; Secure")).unwrap_or(c)
            }
            _ => c,
        };
        headers.append(header::SET_COOKIE, v);
    }
}

/// Bind `addr`, retrying every [`REBIND_EVERY`] (the port may be busy or
/// need privileges). Reports each outcome through `report`.
async fn bind_retrying(
    addr: SocketAddr,
    what: &str,
    mut shutdown: watch::Receiver<bool>,
    report: impl Fn(Result<(), String>),
) -> Option<TcpListener> {
    let mut warned = false;
    loop {
        match TcpListener::bind(addr).await {
            Ok(l) => {
                report(Ok(()));
                return Some(l);
            }
            Err(e) => {
                let msg = if e.kind() == std::io::ErrorKind::PermissionDenied {
                    format!("port {} needs permission to listen (run as the pixelplus service, or set a port above 1024)", addr.port())
                } else if e.kind() == std::io::ErrorKind::AddrInUse {
                    format!("port {} is already used by another program", addr.port())
                } else {
                    format!("can't listen on {addr}: {e}")
                };
                if !warned {
                    tracing::warn!("{what}: {msg}; retrying every {} s", REBIND_EVERY.as_secs());
                    warned = true;
                }
                report(Err(msg));
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(REBIND_EVERY) => {}
            _ = shutdown.changed() => return None,
        }
    }
}

/// Run the HTTPS listener until `shutdown` (no-op when `https_port` is 0).
pub async fn serve_https(state: AppState, app: Router, shutdown: watch::Receiver<bool>) {
    let port = state.config.https_port;
    if port == 0 {
        return;
    }
    let addr = SocketAddr::new(state.config.http_addr.ip(), port);
    let report = {
        let state = state.clone();
        move |r: Result<(), String>| {
            let (listening, error) = match r {
                Ok(()) => (true, None),
                Err(e) => (false, Some(e)),
            };
            state
                .services
                .tls
                .set_listener(crate::services::tls::ListenerInfo { listening, error });
        }
    };
    let Some(tcp) = bind_retrying(addr, "HTTPS", shutdown.clone(), report).await else {
        return;
    };
    let gate = {
        let state = state.clone();
        move || crate::services::tls::active(&state) && state.services.tls.material().is_some()
    };
    let listener = match TlsListener::new(tcp, state.services.tls.server_config(), gate) {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!("HTTPS listener failed: {e}");
            return;
        }
    };
    tracing::info!("Secure web interface on https://{addr}");
    let app = app.layer(axum::middleware::from_fn(https_layer));
    let mut sd = shutdown;
    let res = axum::serve(
        listener.tap_io(|_| {}),
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let _ = sd.wait_for(|v| *v).await;
    })
    .await;
    if let Err(e) = res {
        tracing::warn!("HTTPS listener stopped: {e}");
    }
}

// ---------------------------------------------------------------------------
// Public-only listener (F14) and the games proxy
// ---------------------------------------------------------------------------

/// A stream that first replays bytes already read from it (and holds its
/// connection slot of the public listener until it is dropped).
pub struct Prefixed<S> {
    prefix: Vec<u8>,
    pos: usize,
    inner: S,
    _slot: Option<tokio::sync::OwnedSemaphorePermit>,
    read_deadline: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl<S> Prefixed<S> {
    pub fn new(prefix: Vec<u8>, inner: S) -> Self {
        Prefixed {
            prefix,
            pos: 0,
            inner,
            _slot: None,
            read_deadline: None,
        }
    }

    /// Hold a public-listener connection slot, and stop reading (error) once
    /// `deadline` has passed.
    fn with_slot(mut self, slot: tokio::sync::OwnedSemaphorePermit, deadline: Duration) -> Self {
        self._slot = Some(slot);
        self.read_deadline = Some(Box::pin(tokio::time::sleep(deadline)));
        self
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Prefixed<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if let Some(d) = self.read_deadline.as_mut() {
            if std::future::Future::poll(d.as_mut(), cx).is_ready() {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "request not received in time",
                )));
            }
        }
        if self.pos < self.prefix.len() {
            let n = (self.prefix.len() - self.pos).min(buf.remaining());
            let start = self.pos;
            buf.put_slice(&self.prefix[start..start + n]);
            self.pos += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Prefixed<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

/// Where the first request on a public-listener connection goes.
#[derive(Debug, PartialEq, Eq)]
pub enum PublicRoute {
    /// To the PixelPlus router.
    App,
    /// `/play` without a slash: redirect to `/play/` (relative URLs).
    PlayRedirect(String),
    /// To the games controller, with this request head (path rewritten,
    /// `X-Forwarded-For` appended).
    Games(Vec<u8>),
}

/// Decide where a request head goes; rewrites `/play/<rest>` → `/<rest>`
/// for the games controller and records the client in `X-Forwarded-For`
/// (appended, so the sidecar's trusted-proxy logic sees the real client).
///
/// `trust_cf`: a Cloudflare tunnel managed by PixelPlus is the only thing
/// that may set `CF-Connecting-IP` (the games controller believes it from a
/// loopback peer, which every bridged connection is). Otherwise, e.g. through
/// Tailscale Funnel, which passes unknown headers on, a visitor could pick
/// their own address and dodge the per-visitor limits, so it is dropped.
pub fn route_public_head(head: &[u8], peer: IpAddr, trust_cf: bool) -> PublicRoute {
    let Ok(text) = std::str::from_utf8(head) else {
        return PublicRoute::App;
    };
    let mut lines = text.split("\r\n");
    let Some(request_line) = lines.next() else {
        return PublicRoute::App;
    };
    let mut parts = request_line.split(' ');
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        return PublicRoute::App;
    };
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (target, None),
    };
    if path == "/play" {
        let q = query.map(|q| format!("?{q}")).unwrap_or_default();
        return PublicRoute::PlayRedirect(format!("/play/{q}"));
    }
    let Some(rest) = path.strip_prefix("/play/") else {
        return PublicRoute::App;
    };
    let mut new_target = format!("/{rest}");
    if let Some(q) = query {
        new_target.push('?');
        new_target.push_str(q);
    }
    let mut out = format!("{method} {new_target} {version}\r\n");
    let mut xff: Option<String> = None;
    let lines: Vec<&str> = lines.filter(|l| !l.is_empty()).collect();
    // The connection is bridged as a whole, so a kept-alive connection would
    // carry the visitor's *next* request (e.g. `/request`) to the games
    // controller too: plain requests ask it to close after answering;
    // WebSocket upgrades keep their `Connection: Upgrade`.
    let upgrade = lines.iter().any(|l| {
        l.to_ascii_lowercase()
            .strip_prefix("upgrade:")
            .is_some_and(|v| v.trim() == "websocket")
    });
    for line in lines {
        let lower = line.to_ascii_lowercase();
        if !trust_cf && lower.starts_with("cf-connecting-ip:") {
            continue;
        }
        if !upgrade
            && ["connection:", "keep-alive:", "proxy-connection:"]
                .iter()
                .any(|h| lower.starts_with(h))
        {
            continue;
        }
        if lower.starts_with("x-forwarded-for:") {
            let v = line["x-forwarded-for:".len()..].trim();
            xff = Some(match xff {
                Some(prev) => format!("{prev}, {v}"),
                None => v.to_string(),
            });
            continue;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    let client = peer.to_string();
    let xff = match xff {
        Some(prev) if !prev.is_empty() => format!("{prev}, {client}"),
        _ => client,
    };
    if !upgrade {
        out.push_str("Connection: close\r\n");
    }
    out.push_str(&format!("X-Forwarded-For: {xff}\r\n\r\n"));
    PublicRoute::Games(out.into_bytes())
}

/// Read until the end of the request head (`\r\n\r\n`), at most [`MAX_HEAD`].
async fn read_head(s: &mut TcpStream) -> std::io::Result<(Vec<u8>, Option<usize>)> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 2048];
    loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            return Ok((buf, Some(i + 4)));
        }
        if buf.len() >= MAX_HEAD {
            return Ok((buf, None));
        }
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            return Ok((buf, None));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// Pipe one connection to the games controller.
async fn bridge_games(mut client: TcpStream, head: Vec<u8>, body_start: Vec<u8>, port: u16) {
    let upstream = tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        TcpStream::connect(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)),
    )
    .await;
    let mut upstream = match upstream {
        Ok(Ok(u)) => u,
        _ => {
            let body = "Games aren't running right now. Please try again later.";
            let resp = format!(
                "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = client.write_all(resp.as_bytes()).await;
            return;
        }
    };
    if upstream.write_all(&head).await.is_err() || upstream.write_all(&body_start).await.is_err() {
        return;
    }
    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
}

/// The public-only listener: routes `/play/…` to the games controller at the
/// TCP level (so WebSocket upgrades just work) and everything else to axum.
/// Every axum response closes its connection, so each request is routed
/// afresh even through a tunnel's connection pool.
pub struct PublicListener {
    rx: mpsc::Receiver<(Prefixed<TcpStream>, SocketAddr)>,
    local: SocketAddr,
}

impl PublicListener {
    /// `games_port()` gives the games controller port at connection time.
    /// `CF-Connecting-IP` is not passed to the games controller.
    #[cfg(test)]
    pub fn new(
        tcp: TcpListener,
        games_port: impl Fn() -> u16 + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        Self::with_options(
            tcp,
            games_port,
            || false,
            MAX_PUBLIC_CONNS,
            PUBLIC_READ_DEADLINE,
        )
    }

    /// [`PublicListener::new`]; `trust_cf()` says (at connection time) whether
    /// `CF-Connecting-IP` may reach the games controller (see
    /// [`route_public_head`]); at most `max_conns` connections at once; a
    /// page request must be read within `read_deadline`.
    pub fn with_options(
        tcp: TcpListener,
        games_port: impl Fn() -> u16 + Send + Sync + 'static,
        trust_cf: impl Fn() -> bool + Send + Sync + 'static,
        max_conns: usize,
        read_deadline: Duration,
    ) -> std::io::Result<Self> {
        let local = tcp.local_addr()?;
        let (tx, rx) = mpsc::channel(64);
        let games_port = Arc::new(games_port);
        let trust_cf = Arc::new(trust_cf);
        let slots = Arc::new(tokio::sync::Semaphore::new(max_conns.max(1)));
        tokio::spawn(async move {
            loop {
                let (mut s, peer) = match tcp.accept().await {
                    Ok(c) => c,
                    Err(_) => {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                if tx.is_closed() {
                    return;
                }
                let Ok(slot) = slots.clone().try_acquire_owned() else {
                    // Full: answer at once (never wait for the request) and close.
                    tokio::spawn(async move {
                        let _ = tokio::time::timeout(
                            Duration::from_secs(2),
                            s.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Type: text/plain\r\nContent-Length: 38\r\nRetry-After: 10\r\nConnection: close\r\n\r\nToo many visitors. Please try again.\r\n"),
                        )
                        .await;
                    });
                    continue;
                };
                let tx = tx.clone();
                let games_port = games_port.clone();
                let trust_cf = trust_cf.clone();
                tokio::spawn(async move {
                    let Ok(Ok((buf, end))) =
                        tokio::time::timeout(HANDSHAKE_TIMEOUT, read_head(&mut s)).await
                    else {
                        return;
                    };
                    let route = match end {
                        Some(e) => route_public_head(&buf[..e], peer.ip(), trust_cf()),
                        None => PublicRoute::App,
                    };
                    match route {
                        PublicRoute::App => {
                            let conn = Prefixed::new(buf, s).with_slot(slot, read_deadline);
                            let _ = tx.send((conn, peer)).await;
                        }
                        PublicRoute::PlayRedirect(to) => {
                            let resp = format!(
                                "HTTP/1.1 308 Permanent Redirect\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            );
                            let _ = s.write_all(resp.as_bytes()).await;
                        }
                        PublicRoute::Games(head) => {
                            let e = end.unwrap_or(buf.len());
                            bridge_games(s, head, buf[e..].to_vec(), games_port()).await;
                            drop(slot);
                        }
                    }
                });
            }
        });
        Ok(PublicListener { rx, local })
    }
}

impl Listener for PublicListener {
    type Io = Prefixed<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.rx.recv().await {
            Some(c) => c,
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.local)
    }
}

/// While `settings.remote.publicListener` is off, nothing is served; every
/// response closes its connection (see [`PublicListener`]).
async fn public_gate(
    axum::extract::State(state): axum::extract::State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let mut resp = if state.store.get().settings.remote.public_listener {
        next.run(req).await
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Public access to this light show is turned off.",
        )
            .into_response()
    };
    resp.headers_mut()
        .insert(header::CONNECTION, HeaderValue::from_static("close"));
    resp
}

/// The router for the public-only listener: the full app behind WS5's
/// allow-list (`api::security::public_only`) and the on/off switch.
/// `/play/…` never reaches it (bridged in [`PublicListener`]); a `/play`
/// request that still does (HTTP/1.0 clients) gets a friendly 502.
pub fn public_router(state: AppState) -> Router {
    let play = || async {
        (
            StatusCode::BAD_GATEWAY,
            [(header::CONNECTION, HeaderValue::from_static("close"))],
            "Please reload the page.",
        )
    };
    Router::new()
        .route("/play", axum::routing::any(play))
        .route("/play/{*rest}", axum::routing::any(play))
        .merge(crate::api::router(state.clone()))
        .layer(axum::middleware::from_fn(crate::api::security::public_only))
        .layer(axum::middleware::from_fn_with_state(state, public_gate))
}

/// Run the public-only listener on `127.0.0.1:public_port` until `shutdown`.
pub async fn serve_public(state: AppState, shutdown: watch::Receiver<bool>) {
    let port = state.config.public_port;
    if port == 0 {
        return;
    }
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let Some(tcp) = bind_retrying(addr, "Public listener", shutdown.clone(), |_| {}).await else {
        return;
    };
    let games_port = {
        let state = state.clone();
        move || match state.store.get().settings.games.port {
            0 => 8088,
            p => p,
        }
    };
    let trust_cf = {
        let state = state.clone();
        move || crate::api::security::cf_trusted(&state.store.get().settings)
    };
    let listener = match PublicListener::with_options(
        tcp,
        games_port,
        trust_cf,
        MAX_PUBLIC_CONNS,
        PUBLIC_READ_DEADLINE,
    ) {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!("Public listener failed: {e}");
            return;
        }
    };
    tracing::info!("Public pages for tunnels on http://{addr}");
    let mut sd = shutdown;
    let res = axum::serve(
        listener.tap_io(|_| {}),
        public_router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let _ = sd.wait_for(|v| *v).await;
    })
    .await;
    if let Err(e) = res {
        tracing::warn!("Public listener stopped: {e}");
    }
}

#[cfg(test)]
mod tests;
