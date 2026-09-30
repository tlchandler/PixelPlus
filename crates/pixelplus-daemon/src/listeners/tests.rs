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
        route_public_head(b"GET /request HTTP/1.1\r\nHost: x\r\n\r\n", peer, false),
        PublicRoute::App
    );
    assert_eq!(
        route_public_head(b"GET /play?x=1 HTTP/1.1\r\nHost: x\r\n\r\n", peer, false),
        PublicRoute::PlayRedirect("/play/?x=1".into())
    );
    assert_eq!(
        route_public_head(b"GET /playground HTTP/1.1\r\n\r\n", peer, false),
        PublicRoute::App
    );
    let PublicRoute::Games(head) = route_public_head(
        b"GET /play/ws?room=1 HTTP/1.1\r\nHost: lights.example.com\r\nUpgrade: websocket\r\nX-Forwarded-For: 203.0.113.9\r\n\r\n",
        peer,
        false,
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
    assert!(
        !head.contains("Connection: close"),
        "upgrades stay open: {head}"
    );
    let PublicRoute::Games(head) = route_public_head(b"GET /play/ HTTP/1.1\r\n\r\n", peer, false)
    else {
        panic!("games")
    };
    assert_eq!(
        String::from_utf8(head).unwrap(),
        "GET / HTTP/1.1\r\nConnection: close\r\nX-Forwarded-For: 127.0.0.1\r\n\r\n"
    );
    // A kept-alive page request: the games controller closes after answering,
    // so the visitor's next request on that connection can't reach it.
    let PublicRoute::Games(head) = route_public_head(
        b"GET /play/app.js HTTP/1.1\r\nHost: x\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\n",
        peer,
        false,
    ) else {
        panic!("games")
    };
    assert_eq!(
        String::from_utf8(head).unwrap(),
        "GET /app.js HTTP/1.1\r\nHost: x\r\nConnection: close\r\nX-Forwarded-For: 127.0.0.1\r\n\r\n"
    );
    assert_eq!(
        route_public_head(b"\xff\xfe", peer, false),
        PublicRoute::App
    );
}

/// Security audit 2: a visitor can't choose the address the games
/// controller counts (per-visitor limits) with `CF-Connecting-IP` unless a
/// PixelPlus-managed Cloudflare tunnel is the proxy; a bogus `Upgrade`
/// header doesn't keep a page request's connection open.
#[test]
fn public_head_drops_untrusted_cf_connecting_ip() {
    let peer: IpAddr = "127.0.0.1".parse().unwrap();
    let req = b"GET /play/ HTTP/1.1\r\nHost: x\r\nCF-Connecting-IP: 198.51.100.77\r\nUpgrade: h2c\r\n\r\n";
    let PublicRoute::Games(head) = route_public_head(req, peer, false) else {
        panic!("games")
    };
    let head = String::from_utf8(head).unwrap();
    assert!(
        !head.to_ascii_lowercase().contains("cf-connecting-ip"),
        "{head}"
    );
    assert!(head.contains("Connection: close\r\n"), "{head}");
    let PublicRoute::Games(head) = route_public_head(req, peer, true) else {
        panic!("games")
    };
    assert!(String::from_utf8(head)
        .unwrap()
        .contains("CF-Connecting-IP: 198.51.100.77\r\n"));
}

/// Security audit 2: the public listener caps concurrent connections; past
/// the cap a visitor gets a quick 503 (slow connections through a tunnel
/// can't exhaust the daemon's file descriptors).
#[tokio::test]
async fn public_listener_caps_connections() {
    let app = TestApp::new();
    app.state
        .store
        .update(|s| {
            s.settings.remote.public_listener = true;
            Ok(())
        })
        .await
        .unwrap();
    let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = tcp.local_addr().unwrap();
    let listener =
        PublicListener::with_options(tcp, || Some(1), || false, 2, Duration::from_millis(1500))
            .unwrap();
    let router = public_router(app.state.clone());
    let server = tokio::spawn(async move {
        axum::serve(
            listener.tap_io(|_| {}),
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    // Two slow visitors that never finish their request head.
    let mut slow = vec![];
    for _ in 0..2 {
        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_all(b"GET /api/v1/public/health HTTP/1.1\r\n")
            .await
            .unwrap();
        slow.push(s);
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut s = TcpStream::connect(addr).await.unwrap();
    let full = roundtrip(
        &mut s,
        "GET /api/v1/public/health HTTP/1.1\r\nHost: x\r\n\r\n",
    )
    .await;
    assert!(full.starts_with("HTTP/1.1 503"), "{full}");
    // A slot frees up when a visitor leaves.
    drop(slow.pop());
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut s = TcpStream::connect(addr).await.unwrap();
    let ok = roundtrip(
        &mut s,
        "GET /api/v1/public/health HTTP/1.1\r\nHost: x\r\n\r\n",
    )
    .await;
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    // A visitor trickling a request body loses the connection at the read
    // deadline (and its slot with it).
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(b"POST /api/v1/public/requests HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: 100000\r\n\r\n{")
        .await
        .unwrap();
    let t0 = std::time::Instant::now();
    let mut sink = Vec::new();
    let r = tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            if s.write_all(b" ").await.is_err() {
                break;
            }
            let mut b = [0u8; 512];
            match tokio::time::timeout(Duration::from_millis(100), s.read(&mut b)).await {
                Ok(Ok(0)) | Ok(Err(_)) => break,
                Ok(Ok(n)) => sink.extend_from_slice(&b[..n]),
                Err(_) => {}
            }
        }
    })
    .await;
    assert!(r.is_ok(), "the trickling connection was closed");
    assert!(t0.elapsed() < Duration::from_secs(5));
    server.abort();
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
    let listener = PublicListener::new(tcp, move || Some(games_port)).unwrap();
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
    let listener = PublicListener::new(tcp, move || Some(dead)).unwrap();
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

/// Settings → Features: games off → `/play/` is 404; song requests off →
/// the public request API is 404; remote access off → nothing is served.
#[tokio::test]
async fn public_pages_follow_feature_toggles() {
    use pixelplus_core::model::FeatureId;
    let app = TestApp::new();
    let (games_port, _seen) = fake_games().await;
    app.state
        .store
        .update(|s| {
            s.settings.remote.public_listener = true;
            s.settings.requests.enabled = true;
            Ok(())
        })
        .await
        .unwrap();
    let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = tcp.local_addr().unwrap();
    let st = app.state.clone();
    let listener = PublicListener::new(tcp, move || {
        st.store
            .get()
            .feature(FeatureId::Games)
            .then_some(games_port)
    })
    .unwrap();
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
    let set = |id: FeatureId, on: bool| {
        let state = app.state.clone();
        async move {
            state
                .store
                .update(move |s| {
                    s.settings.features.set(id, on);
                    Ok(())
                })
                .await
                .unwrap();
        }
    };
    assert!(get("/play/").await.ends_with("games"));
    assert!(get("/api/v1/public/requests")
        .await
        .starts_with("HTTP/1.1 200"));

    set(FeatureId::Games, false).await;
    set(FeatureId::Requests, false).await;
    let play = get("/play/").await;
    assert!(play.starts_with("HTTP/1.1 404"), "{play}");
    let req = get("/api/v1/public/requests").await;
    assert!(req.starts_with("HTTP/1.1 404"), "{req}");
    assert!(req.contains("feature_disabled"), "{req}");
    assert!(get("/api/v1/public/health")
        .await
        .starts_with("HTTP/1.1 200"));

    set(FeatureId::Remote, false).await;
    assert!(get("/api/v1/public/health")
        .await
        .starts_with("HTTP/1.1 503"));

    set(FeatureId::Remote, true).await;
    set(FeatureId::Games, true).await;
    set(FeatureId::Requests, true).await;
    assert!(get("/play/").await.ends_with("games"));
    assert!(get("/api/v1/public/requests")
        .await
        .starts_with("HTTP/1.1 200"));
    server.abort();
}
