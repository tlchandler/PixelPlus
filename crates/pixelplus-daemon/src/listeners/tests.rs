//! End-to-end tests of the HTTPS and public-only listeners over real sockets.

use super::*;
use crate::api::testkit::TestApp;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Send raw bytes, read until the peer closes (or `limit` bytes).
async fn roundtrip<S: AsyncRead + AsyncWrite + Unpin>(s: &mut S, req: &str) -> String {
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await;
    String::from_utf8_lossy(&out).into_owned()
}

#[tokio::test]
async fn https_listener_serves_the_app_with_secure_cookies() {
    let app = TestApp::new();
    crate::services::tls::ensure(&app.state, false, false)
        .await
        .unwrap();
    let m = app.state.services.tls.material().unwrap();
    let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = tcp.local_addr().unwrap();
    let listener = TlsListener::new(tcp, app.state.services.tls.server_config(), || true).unwrap();
    let router = app
        .router
        .clone()
        .layer(axum::middleware::from_fn(https_layer));
    let server = tokio::spawn(async move {
        axum::serve(
            listener.tap_io(|_| {}),
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });

    let mut roots = tokio_rustls::rustls::RootCertStore::empty();
    roots.add(m.ca_der.clone().into()).unwrap();
    let client = tokio_rustls::rustls::ClientConfig::builder_with_provider(Arc::new(
        tokio_rustls::rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(client));

    let fetch = |path: &'static str, name: &'static str| {
        let connector = connector.clone();
        async move {
            let tcp = TcpStream::connect(addr).await.unwrap();
            let sni = tokio_rustls::rustls::pki_types::ServerName::try_from(name).unwrap();
            let mut s = connector.connect(sni, tcp).await.unwrap();
            roundtrip(
                &mut s,
                &format!("GET {path} HTTP/1.1\r\nHost: {name}\r\nConnection: close\r\n\r\n"),
            )
            .await
        }
    };
    let health = fetch("/api/v1/public/health", "localhost").await;
    assert!(health.starts_with("HTTP/1.1 200"), "{health}");
    assert!(health.contains("\"ok\":true"));
    // Handlers see the request as secure.
    let status = fetch("/api/v1/tls/status", "localhost").await;
    assert!(status.contains("\"secureNow\":true"), "{status}");
    // Addressed by IP (the phone's usual way in).
    let by_ip = fetch("/api/v1/public/health", "127.0.0.1").await;
    assert!(by_ip.starts_with("HTTP/1.1 200"), "{by_ip}");

    // Cookies set over HTTPS are Secure.
    let mut h = axum::http::HeaderMap::new();
    h.append(
        header::SET_COOKIE,
        HeaderValue::from_static("pp_session=a; Path=/; HttpOnly"),
    );
    h.append(header::SET_COOKIE, HeaderValue::from_static("x=1; Secure"));
    secure_cookies(&mut h);
    let v: Vec<&str> = h
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap())
        .collect();
    assert_eq!(
        v,
        vec!["pp_session=a; Path=/; HttpOnly; Secure", "x=1; Secure"]
    );
    server.abort();
}

#[tokio::test]
async fn https_gate_refuses_connections_when_off() {
    let app = TestApp::new();
    crate::services::tls::ensure(&app.state, false, false)
        .await
        .unwrap();
    let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = tcp.local_addr().unwrap();
    let _listener =
        TlsListener::new(tcp, app.state.services.tls.server_config(), || false).unwrap();
    let mut s = TcpStream::connect(addr).await.unwrap();
    let mut b = [0u8; 1];
    let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut b))
        .await
        .unwrap();
    assert!(matches!(n, Ok(0) | Err(_)), "closed without a handshake");
}

#[test]
fn public_head_routing() {
    let peer: IpAddr = "127.0.0.1".parse().unwrap();
    assert_eq!(
        route_public_head(b"GET /request HTTP/1.1\r\nHost: x\r\n\r\n", peer),
        PublicRoute::App
    );
    assert_eq!(
        route_public_head(b"GET /play?x=1 HTTP/1.1\r\nHost: x\r\n\r\n", peer),
        PublicRoute::PlayRedirect("/play/?x=1".into())
    );
    assert_eq!(
        route_public_head(b"GET /playground HTTP/1.1\r\n\r\n", peer),
        PublicRoute::App
    );
    let PublicRoute::Games(head) = route_public_head(
        b"GET /play/ws?room=1 HTTP/1.1\r\nHost: lights.example.com\r\nUpgrade: websocket\r\nX-Forwarded-For: 203.0.113.9\r\n\r\n",
        peer,
    ) else {
        panic!("games")
    };
    let head = String::from_utf8(head).unwrap();
    assert!(head.starts_with("GET /ws?room=1 HTTP/1.1\r\n"), "{head}");
    assert!(head.contains("Upgrade: websocket\r\n"));
    assert!(
        head.ends_with("X-Forwarded-For: 203.0.113.9, 127.0.0.1\r\n\r\n"),
        "{head}"
    );
    let PublicRoute::Games(head) = route_public_head(b"GET /play/ HTTP/1.1\r\n\r\n", peer) else {
        panic!("games")
    };
    assert_eq!(
        String::from_utf8(head).unwrap(),
        "GET / HTTP/1.1\r\nX-Forwarded-For: 127.0.0.1\r\n\r\n"
    );
    assert_eq!(route_public_head(b"\xff\xfe", peer), PublicRoute::App);
}

/// A fake games controller: records each request head, answers `/ws` with a
/// 101 and then echoes, anything else with a page, closing afterwards (like
/// the real one).
async fn fake_games() -> (u16, Arc<parking_lot::Mutex<Vec<String>>>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = l.accept().await.unwrap();
            let log = log.clone();
            tokio::spawn(async move {
                let (buf, end) = read_head(&mut s).await.unwrap();
                let head = String::from_utf8_lossy(&buf[..end.unwrap()]).into_owned();
                log.lock().push(head.clone());
                if head.starts_with("GET /ws") {
                    s.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n")
                        .await
                        .unwrap();
                    let mut b = [0u8; 5];
                    s.read_exact(&mut b).await.unwrap();
                    s.write_all(&b).await.unwrap();
                } else {
                    s.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\ngames",
                    )
                    .await
                    .unwrap();
                }
            });
        }
    });
    (port, seen)
}

#[tokio::test]
async fn public_listener_serves_public_pages_and_proxies_games() {
    let app = TestApp::new();
    let (games_port, seen) = fake_games().await;
    let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = tcp.local_addr().unwrap();
    let listener = PublicListener::new(tcp, move || games_port).unwrap();
    let router = public_router(app.state.clone());
    let server = tokio::spawn(async move {
        axum::serve(
            listener.tap_io(|_| {}),
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let get = |path: &'static str| async move {
        let mut s = TcpStream::connect(addr).await.unwrap();
        roundtrip(
            &mut s,
            &format!("GET {path} HTTP/1.1\r\nHost: lights.example.com\r\n\r\n"),
        )
        .await
    };

    // Off by default: nothing is served.
    let off = get("/api/v1/public/health").await;
    assert!(off.starts_with("HTTP/1.1 503"), "{off}");
    app.state
        .store
        .update(|s| {
            s.settings.remote.public_listener = true;
            Ok(())
        })
        .await
        .unwrap();

    // Public API works; every response closes the connection (read_to_end returned).
    let health = get("/api/v1/public/health").await;
    assert!(health.starts_with("HTTP/1.1 200"), "{health}");
    assert!(health.to_ascii_lowercase().contains("connection: close"));
    // Admin API is not there.
    let show = get("/api/v1/show").await;
    assert!(show.starts_with("HTTP/1.1 404"), "{show}");

    // Games page, path rewritten, client recorded.
    let page = get("/play/").await;
    assert!(page.ends_with("games"), "{page}");
    let redirect = get("/play").await;
    assert!(redirect.starts_with("HTTP/1.1 308"), "{redirect}");
    assert!(redirect.contains("Location: /play/"));
    // WebSocket upgrade passes straight through.
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(
        b"GET /play/ws HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
    )
    .await
    .unwrap();
    let mut buf = vec![0u8; 256];
    let n = s.read(&mut buf).await.unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 101"));
    s.write_all(b"hello").await.unwrap();
    let mut echo = [0u8; 5];
    s.read_exact(&mut echo).await.unwrap();
    assert_eq!(&echo, b"hello");

    let heads = seen.lock().clone();
    assert!(heads[0].starts_with("GET / HTTP/1.1"), "{heads:?}");
    assert!(heads[0].contains("X-Forwarded-For: 127.0.0.1"));
    assert!(heads.iter().any(|h| h.starts_with("GET /ws HTTP/1.1")));
    server.abort();
}

#[tokio::test]
async fn games_down_gives_a_friendly_502() {
    let app = TestApp::new();
    app.state
        .store
        .update(|s| {
            s.settings.remote.public_listener = true;
            Ok(())
        })
        .await
        .unwrap();
    // A port nothing listens on.
    let dead = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = tcp.local_addr().unwrap();
    let listener = PublicListener::new(tcp, move || dead).unwrap();
    let router = public_router(app.state.clone());
    let server = tokio::spawn(async move {
        axum::serve(
            listener.tap_io(|_| {}),
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let mut s = TcpStream::connect(addr).await.unwrap();
    let r = roundtrip(&mut s, "GET /play/ HTTP/1.1\r\nHost: x\r\n\r\n").await;
    assert!(r.starts_with("HTTP/1.1 502"), "{r}");
    assert!(r.contains("Games aren't running"));
    server.abort();
}
