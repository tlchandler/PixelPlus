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
