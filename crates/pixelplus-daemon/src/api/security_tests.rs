//! HTTP integration tests of the security hardening (api::security, auth,
//! secret redaction, media serving). Uses the [`super::testkit`] harness.

use super::testkit::TestApp;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{header, Request, StatusCode};
use serde_json::json;
use std::net::SocketAddr;

async fn leader(app: &mut TestApp, password: Option<&str>) {
    let mut body = json!({ "role": "leader", "showName": "Test", "timezone": "America/Chicago", "board": "difftx", "boardRev": "E" });
    if let Some(p) = password {
        body["password"] = json!(p);
    }
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/system/setup")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, headers, _) = app.send(req).await;
    assert_eq!(status, StatusCode::OK);
    app.cookie = headers
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .map(|c| c.split(';').next().unwrap().to_string());
}

// ---------------------------------------------------------------------
// Browser hardening (api::security) and secrets (M4)
// ---------------------------------------------------------------------

fn req(method: &str, path: &str, headers: &[(&str, &str)]) -> Request<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(format!("/api/v1{path}"));
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    b.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn host_allow_list_csrf_header_and_security_headers() {
    let app = TestApp::new();
    // DNS rebinding: a foreign host name is refused (friendly page for browsers).
    let (s, h, body) = app
        .send(req(
            "GET",
            "/show",
            &[("host", "evil.example.com"), ("accept", "text/html")],
        ))
        .await;
    assert_eq!(s, StatusCode::MISDIRECTED_REQUEST);
    assert!(String::from_utf8_lossy(&body).contains("Other names for this controller"));
    assert_eq!(h["x-content-type-options"], "nosniff");
    // IP literal, localhost and <hostname>.local work.
    for host in [
        "192.168.1.20",
        "localhost:8080",
        &format!("{}.local", crate::cluster::net::hostname()),
    ] {
        let (s, _, _) = app.send(req("GET", "/show", &[("host", host)])).await;
        assert_eq!(s, StatusCode::OK, "{host}");
    }
    // The public page works under any name (tunnels).
    let (s, _, _) = app
        .send(req(
            "GET",
            "/public/health",
            &[("host", "lights.example.com")],
        ))
        .await;
    assert_eq!(s, StatusCode::OK);
    // Configured extra names.
    app.state
        .store
        .update(|s| {
            s.settings.security.allowed_hosts = vec!["lights.example.com".into()];
            Ok(())
        })
        .await
        .unwrap();
    let (s, _, _) = app
        .send(req("GET", "/show", &[("host", "lights.example.com")]))
        .await;
    assert_eq!(s, StatusCode::OK);

    // CSRF: state-changing calls need the app's header (a cross-site form can't send it).
    let (s, _, _) = app
        .send(req("POST", "/player/stop", &[("x-pixelplus-request", "0")]))
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let mut r = req("POST", "/player/stop", &[]);
    r.headers_mut()
        .insert("x-pixelplus-request", "1".parse().unwrap());
    let (s, _, _) = app.send(r).await;
    assert_ne!(s, StatusCode::FORBIDDEN);
    // Security headers on everything, CSP included.
    let (_, h, _) = app.send(req("GET", "/show", &[])).await;
    assert_eq!(h["x-frame-options"], "DENY");
    assert!(h["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("frame-ancestors 'none'"));

    // WebSocket from another site.
    let (s, _, _) = app
        .send(req(
            "GET",
            "/ws",
            &[
                ("host", "192.168.1.20"),
                ("origin", "http://evil.example.com"),
            ],
        ))
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn local_token_is_scoped_and_proxy_proof() {
    let mut app = TestApp::new();
    leader(&mut app, Some("jingle")).await;
    app.cookie = None;
    app.state.sessions.set_local_token("t0ken".repeat(8));
    let token = "t0ken".repeat(8);
    let lo = |mut r: Request<Body>| {
        r.extensions_mut()
            .insert(ConnectInfo::<SocketAddr>("127.0.0.1:4000".parse().unwrap()));
        r
    };
    // The old constant header is worthless.
    let (s, _, _) = app
        .send(lo(req("GET", "/show", &[("x-pixelplus-local", "1")])))
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _, _) = app
        .send(lo(req("GET", "/show", &[("x-pixelplus-local", &token)])))
        .await;
    assert_eq!(s, StatusCode::OK);
    // Not through a reverse proxy on the same machine…
    let (s, _, _) = app
        .send(lo(req(
            "GET",
            "/show",
            &[
                ("x-pixelplus-local", &token),
                ("x-forwarded-for", "6.6.6.6"),
            ],
        )))
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // …not from the network…
    let (s, _, _) = app
        .send(req("GET", "/show", &[("x-pixelplus-local", &token)]))
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // …and only for the sidecar's routes.
    for (m, p) in [
        ("GET", "/system/ssh"),
        ("PUT", "/show/settings"),
        ("POST", "/system/update"),
    ] {
        let (s, _, _) = app
            .send(lo(req(m, p, &[("x-pixelplus-local", &token)])))
            .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{m} {p}");
    }
    let (s, _, _) = app
        .send(lo(req(
            "POST",
            "/player/pause",
            &[("x-pixelplus-local", &token)],
        )))
        .await;
    assert_ne!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_is_throttled() {
    let mut app = TestApp::new();
    leader(&mut app, Some("jingle")).await;
    app.cookie = None;
    let mut last = StatusCode::OK;
    for _ in 0..6 {
        last = app
            .json("POST", "/auth/login", Some(json!({"password": "nope"})))
            .await
            .0;
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
    // Even the right password waits now (from that address).
    let (s, v) = app
        .json("POST", "/auth/login", Some(json!({"password": "jingle"})))
        .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "{v}");
}

#[tokio::test]
async fn secrets_are_write_only() {
    let app = TestApp::new();
    let (s, _) = app
        .json(
            "PUT",
            "/show/settings",
            Some(json!({
                "mqtt": {"enabled": false, "host": "ha.local", "port": 1883, "username": "u", "password": "mqtt-secret",
                         "baseTopic": "pixelplus", "homeAssistantDiscovery": true},
                "alerts": {"email": {"smtpHost": "smtp.x", "smtpPort": 587, "username": "me", "password": "smtp-secret",
                                     "from": "a@x", "to": "b@x", "tls": true}}
            })),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let (_, show) = app.json("GET", "/show", None).await;
    let text = show.to_string();
    assert!(
        !text.contains("mqtt-secret") && !text.contains("smtp-secret"),
        "{text}"
    );
    assert_eq!(show["settings"]["mqtt"]["password"], "********");
    assert_eq!(show["settings"]["alerts"]["email"]["password"], "********");
    // Sending the settings back unchanged keeps the stored secrets.
    let (s, back) = app
        .json("PUT", "/show/settings", Some(show["settings"].clone()))
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(back["mqtt"]["password"], "********");
    let stored = app.state.store.get();
    assert_eq!(
        stored.settings.mqtt.password.as_deref(),
        Some("mqtt-secret")
    );
    assert_eq!(
        stored.settings.alerts.email.as_ref().unwrap().password,
        "smtp-secret"
    );
    // A new value replaces it.
    app.json(
        "PUT",
        "/show/settings",
        Some(json!({"mqtt": {"password": "new"}})),
    )
    .await;
    assert_eq!(
        app.state.store.get().settings.mqtt.password.as_deref(),
        Some("new")
    );
    // Bad allowed-host entries are refused.
    let (s, _) = app
        .json(
            "PUT",
            "/show/settings",
            Some(json!({"security": {"allowedHosts": ["a b"]}})),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn hostile_media_paths_are_never_served() {
    let app = TestApp::new();
    std::fs::write(app.dir.join("node.json.mp3"), b"x").unwrap();
    std::fs::write(
        app.dir.join("media/evil.html"),
        b"<script>alert(1)</script>",
    )
    .unwrap();
    std::fs::write(app.dir.join("media/good.mp3"), b"ID3").unwrap();
    let mk = |id: &str, file: &str| pixelplus_core::model::Media {
        tags: Default::default(),
        analysis: Default::default(),
        original_name: Default::default(),
        original_size: Default::default(),
        id: id.into(),
        name: id.into(),
        kind: pixelplus_core::model::MediaKind::Song,
        file: file.into(),
        duration_ms: 1,
        loudness_lufs: None,
        gain_db: None,
    };
    // As a restored snapshot would bring them.
    let mut show = (*app.state.store.get()).clone();
    show.media = vec![
        mk("a", "node.json.mp3"),
        mk("evil", "media/evil.html"),
        mk("good", "media/good.mp3"),
    ];
    app.state.store.replace(show).await.unwrap();
    for id in ["a", "evil"] {
        let (s, _, _) = app
            .send(req("GET", &format!("/media/{id}/file"), &[]))
            .await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{id}");
    }
    let (s, h, _) = app.send(req("GET", "/media/good/file", &[])).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h["content-type"], "audio/mpeg");
    assert!(h["content-disposition"]
        .to_str()
        .unwrap()
        .starts_with("attachment"));
    assert_eq!(h["x-content-type-options"], "nosniff");
}

/// The xLights upload password hash (F16) is write-only like the UI password:
/// shown as "" when set, never settable through `PUT /show/settings`. (WS0)
#[tokio::test]
async fn xlights_upload_password_is_write_only() {
    let app = TestApp::new();
    app.state
        .store
        .update(|s| {
            s.settings.xlights.password_hash = Some("$argon2id$secret".into());
            Ok(())
        })
        .await
        .unwrap();
    let (_, show) = app.json("GET", "/show", None).await;
    assert!(!show.to_string().contains("argon2id"));
    assert_eq!(show["settings"]["xlights"]["passwordHash"], "");
    let (s, back) = app
        .json(
            "PUT",
            "/show/settings",
            Some(json!({"xlights": {"passwordHash": "attacker", "fppConnect": true}})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(back["xlights"]["passwordHash"], "");
    let stored = app.state.store.get();
    assert_eq!(
        stored.settings.xlights.password_hash.as_deref(),
        Some("$argon2id$secret")
    );
    assert!(stored.settings.xlights.fpp_connect);
}

// ---------------------------------------------------------------------
// F16 hook: root-mounted FPP Connect routes (security::fpp_compat_guard)
// ---------------------------------------------------------------------

async fn fpp_send(
    app: &TestApp,
    method: &str,
    peer: &str,
    headers: &[(&str, &str)],
) -> StatusCode {
    use tower::ServiceExt;
    let router = axum::Router::new()
        .route(
            "/api/file/{dir}",
            axum::routing::any(|| async { "ok" }),
        )
        .layer(axum::middleware::from_fn_with_state(
            app.state.clone(),
            super::security::fpp_compat_guard,
        ))
        .with_state(app.state.clone());
    let mut b = Request::builder().method(method).uri("/api/file/sequences");
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let mut r = b.body(Body::empty()).unwrap();
    r.extensions_mut()
        .insert(ConnectInfo::<SocketAddr>(peer.parse().unwrap()));
    router.oneshot(r).await.unwrap().status()
}

fn basic(password: &str) -> String {
    // "admin:<password>" in base64, the way xLights sends it.
    let raw = format!("admin:{password}");
    let t = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in raw.as_bytes().chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(t[(n >> 18) as usize & 63] as char);
        out.push(t[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { t[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { t[n as usize & 63] as char } else { '=' });
    }
    format!("Basic {out}")
}

#[tokio::test]
async fn fpp_connect_hook_replaces_csrf_with_lan_and_upload_password() {
    let app = TestApp::new();
    let lan = "192.168.1.50:40000";
    // Off by default: invisible.
    assert_eq!(fpp_send(&app, "GET", lan, &[]).await, StatusCode::NOT_FOUND);
    app.state
        .store
        .update(|s| {
            s.settings.xlights.fpp_connect = true;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(fpp_send(&app, "GET", lan, &[]).await, StatusCode::OK);
    // Never from the internet, a tunnel or a proxy.
    assert_eq!(
        fpp_send(&app, "GET", "203.0.113.9:1", &[]).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fpp_send(&app, "PATCH", "127.0.0.1:1", &[("cf-connecting-ip", "203.0.113.9")]).await,
        StatusCode::NOT_FOUND
    );
    // DNS rebinding.
    assert_eq!(
        fpp_send(&app, "GET", lan, &[("host", "evil.example.com")]).await,
        StatusCode::MISDIRECTED_REQUEST
    );
    // No password: only non-CORS-simple writes (a web page can't forge them).
    assert_eq!(fpp_send(&app, "PATCH", lan, &[]).await, StatusCode::OK);
    assert_eq!(
        fpp_send(&app, "POST", lan, &[("content-type", "application/json")]).await,
        StatusCode::OK
    );
    for ct in ["text/plain", "multipart/form-data; boundary=x"] {
        assert_eq!(
            fpp_send(&app, "POST", lan, &[("content-type", ct)]).await,
            StatusCode::FORBIDDEN,
            "{ct}"
        );
    }
    // With an upload password: Basic auth required for every write.
    let hash = super::auth::hash_password("xlights-secret").unwrap();
    app.state
        .store
        .update(|s| {
            s.settings.xlights.password_hash = Some(hash.clone());
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(fpp_send(&app, "PATCH", lan, &[]).await, StatusCode::UNAUTHORIZED);
    let wrong = basic("nope-nope");
    assert_eq!(
        fpp_send(&app, "PATCH", lan, &[("authorization", &wrong)]).await,
        StatusCode::UNAUTHORIZED
    );
    let good = basic("xlights-secret");
    assert_eq!(
        fpp_send(&app, "PATCH", lan, &[("authorization", &good)]).await,
        StatusCode::OK
    );
    // Remembered (no Argon2 per chunk), and a form POST with the password is fine.
    assert_eq!(
        fpp_send(
            &app,
            "POST",
            lan,
            &[("authorization", &good), ("content-type", "multipart/form-data; boundary=x")]
        )
        .await,
        StatusCode::OK
    );
    // Reads stay open (xLights probes /config.php before it knows the password).
    assert_eq!(fpp_send(&app, "GET", lan, &[]).await, StatusCode::OK);
}

#[test]
fn basic_auth_header_parsing() {
    let mut h = axum::http::HeaderMap::new();
    h.insert(header::AUTHORIZATION, basic("pa:ss wörd").parse().unwrap());
    assert_eq!(
        super::security::basic_auth_password(&h).as_deref(),
        Some("pa:ss wörd")
    );
    h.insert(header::AUTHORIZATION, "Bearer x".parse().unwrap());
    assert_eq!(super::security::basic_auth_password(&h), None);
    h.insert(header::AUTHORIZATION, "Basic !!!".parse().unwrap());
    assert_eq!(super::security::basic_auth_password(&h), None);
}

// ---------------------------------------------------------------------
// F14: public-only listener allow-list and admin through tunnels
// ---------------------------------------------------------------------

/// The full app router behind the public-only policy, as the public
/// listener serves it (WS1 mounts `security::public_only` the same way).
fn public_app(app: &TestApp) -> axum::Router {
    std::fs::create_dir_all(&app.state.config.web_dir).unwrap();
    std::fs::write(
        app.state.config.web_dir.join("index.html"),
        "<!doctype html><title>PixelPlus</title>",
    )
    .unwrap();
    super::router(app.state.clone()).layer(axum::middleware::from_fn(super::security::public_only))
}

async fn public_status(router: &axum::Router, method: &str, path: &str) -> StatusCode {
    use tower::ServiceExt;
    let mut r = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "lights.example.com")
        .header("x-pixelplus-request", "1")
        .header("cf-connecting-ip", "203.0.113.5")
        .body(Body::empty())
        .unwrap();
    r.extensions_mut()
        .insert(ConnectInfo::<SocketAddr>("127.0.0.1:40000".parse().unwrap()));
    router.clone().oneshot(r).await.unwrap().status()
}

#[tokio::test]
async fn public_listener_serves_only_the_public_pages() {
    let mut app = TestApp::new();
    leader(&mut app, None).await; // no password: the worst case
    let router = public_app(&app);
    // Every admin API route and page is invisible.
    let admin = [
        ("GET", "/api/v1/show"),
        ("GET", "/api/v1/system"),
        ("POST", "/api/v1/system/setup"),
        ("POST", "/api/v1/system/reboot"),
        ("GET", "/api/v1/system/logs"),
        ("PUT", "/api/v1/show/settings"),
        ("POST", "/api/v1/auth/login"),
        ("PUT", "/api/v1/auth/password"),
        ("GET", "/api/v1/nodes"),
        ("POST", "/api/v1/nodes/adopt"),
        ("POST", "/api/v1/nodes/x/replace"),
        ("GET", "/api/v1/cluster/manifest/x"),
        ("POST", "/api/v1/cluster/adopt"),
        ("POST", "/api/v1/cluster/command"),
        ("GET", "/api/v1/cluster/update/pixelplus_1.0_arm64.deb"),
        ("POST", "/api/v1/system/transfer/export"),
        ("POST", "/api/v1/system/update"),
        ("GET", "/api/v1/remote/status"),
        ("POST", "/api/v1/remote/tailscale/up"),
        ("GET", "/api/v1/requests"),
        ("GET", "/api/v1/snapshots"),
        ("GET", "/api/v1/ws"),
        ("GET", "/api/v1/journal"),
        ("POST", "/api/v1/player/play"),
        ("GET", "/config.php"),
        ("PATCH", "/api/file/sequences"),
        ("GET", "/settings"),
        ("GET", "/settings/remote"),
        ("GET", "/setup"),
        ("GET", "/controllers"),
        ("GET", "/trust"),
        ("GET", "/api/v1/public/../show"),
        ("GET", "/api/v1/public/%2e%2e/show"),
        ("GET", "//api/v1/show"),
        ("POST", "/request"),
    ];
    for (m, p) in admin {
        assert_eq!(public_status(&router, m, p).await, StatusCode::NOT_FOUND, "{m} {p}");
    }
    // The public page, its assets and API work (under any host name).
    assert_eq!(public_status(&router, "GET", "/request").await, StatusCode::OK);
    assert_eq!(
        public_status(&router, "GET", "/api/v1/public/health").await,
        StatusCode::OK
    );
    assert_eq!(
        public_status(&router, "GET", "/api/v1/public/requests").await,
        StatusCode::OK
    );
    assert_eq!(
        public_status(&router, "GET", "/").await,
        StatusCode::TEMPORARY_REDIRECT
    );
}

#[test]
fn public_path_policy() {
    use axum::http::Method;
    use super::security::public_path_allowed as ok;
    assert!(ok(&Method::GET, "/request"));
    assert!(ok(&Method::GET, "/request/thanks"));
    assert!(!ok(&Method::GET, "/requests"));
    assert!(ok(&Method::GET, "/_app/immutable/entry/app.js"));
    assert!(ok(&Method::POST, "/api/v1/public/requests"));
    assert!(ok(&Method::GET, "/api/v1/public/ca.crt"));
    assert!(ok(&Method::GET, "/play/controller"));
    assert!(ok(&Method::POST, "/play/api/join"));
    assert!(!ok(&Method::GET, "/player"));
    assert!(!ok(&Method::POST, "/_app/x.js"));
    assert!(!ok(&Method::GET, "/api/v1/publicx"));
    assert!(!ok(&Method::GET, "/_app/../api/v1/show"));
    assert!(!ok(&Method::GET, "/api/v1/public/%2F..%2Fshow"));
}

#[tokio::test]
async fn admin_through_a_tunnel_needs_a_password() {
    let mut app = TestApp::new();
    leader(&mut app, None).await;
    let tunnel = |path: &str| {
        let mut r = req(
            "GET",
            path,
            &[("host", "127.0.0.1"), ("x-forwarded-for", "203.0.113.9")],
        );
        r.extensions_mut()
            .insert(ConnectInfo::<SocketAddr>("127.0.0.1:1".parse().unwrap()));
        r
    };
    let (s, _, body) = app.send(tunnel("/show")).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(String::from_utf8_lossy(&body).contains("password_required"));
    // Public pages stay available through the tunnel.
    let (s, _, _) = app.send(tunnel("/public/health")).await;
    assert_eq!(s, StatusCode::OK);
    // On the home network (no proxy) nothing changes.
    let (s, _, _) = app.send(req("GET", "/show", &[])).await;
    assert_eq!(s, StatusCode::OK);
    // With a password the tunnel reaches the sign-in check instead.
    let hash = super::auth::hash_password("sleigh-bells").unwrap();
    app.state
        .store
        .update(|s| {
            s.settings.security.password_hash = Some(hash.clone());
            Ok(())
        })
        .await
        .unwrap();
    app.cookie = None;
    let (s, _, _) = app.send(tunnel("/show")).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn remote_admin_hostnames_are_allowed_only_while_exposed() {
    use pixelplus_core::model::{CloudflareState, TailscaleState};
    let app = TestApp::new();
    let host = |h: &'static str| req("GET", "/show", &[("host", h)]);
    let (s, _, _) = app.send(host("pp.tail1234.ts.net")).await;
    assert_eq!(s, StatusCode::MISDIRECTED_REQUEST);
    app.state
        .store
        .update(|s| {
            s.settings.remote.tailscale = Some(TailscaleState {
                enabled: true,
                serve_admin: true,
                funnel_public: false,
                dns_name: Some("pp.tail1234.ts.net.".into()),
            });
            s.settings.remote.cloudflare = Some(CloudflareState {
                mode: "token".into(),
                public_host: Some("lights.example.com".into()),
                admin_host: Some("admin.example.com".into()),
                token_set: true,
            });
            Ok(())
        })
        .await
        .unwrap();
    for h in ["pp.tail1234.ts.net", "admin.example.com"] {
        let (s, _, _) = app.send(host(h)).await;
        assert_eq!(s, StatusCode::OK, "{h}");
    }
    // The public hostname never reaches the admin API.
    let (s, _, _) = app.send(host("lights.example.com")).await;
    assert_eq!(s, StatusCode::MISDIRECTED_REQUEST);
}
