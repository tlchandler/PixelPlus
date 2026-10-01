//! HTTP tests of the secret trigger links (`api::hooks`): the auth matrix
//! (no session needed, wrong tokens throttled, header vs query, GET on/off,
//! home network vs tunnel / public listener), feature toggles, gates,
//! rotate / revoke, and secrets never leaving the controller.

use super::testkit::TestApp;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{header, Request, StatusCode};
use serde_json::{json, Value};
use std::net::SocketAddr;
use tower::ServiceExt;

const LAN: &str = "192.168.1.40:50000";

async fn signed_in_leader(id: &str, extra: Value) -> TestApp {
    let mut app = TestApp::new();
    let body = json!({ "role": "leader", "showName": "Test", "timezone": "America/Chicago",
        "board": "difftx", "boardRev": "E", "password": "sleigh-bells" });
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
    assert!(app.cookie.is_some(), "signed in");
    let mut t =
        json!({ "id": id, "name": "Doorbell", "kind": "http", "action": { "type": "stop" } });
    if let (Some(t), Some(extra)) = (t.as_object_mut(), extra.as_object()) {
        for (k, v) in extra {
            t.insert(k.clone(), v.clone());
        }
    }
    let (s, v) = app
        .json("PUT", "/show/settings", Some(json!({ "triggers": [t] })))
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    app
}

async fn make_token(app: &TestApp, id: &str) -> String {
    let (s, v) = app
        .json("POST", &format!("/triggers/{id}/token"), None)
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v["token"].as_str().unwrap().to_string()
}

/// A request as a doorbell / Home Assistant sends it: no cookie, no CSRF header.
struct Call<'a> {
    method: &'a str,
    uri: String,
    headers: Vec<(&'a str, String)>,
    peer: &'a str,
}

impl<'a> Call<'a> {
    fn post(id: &str) -> Self {
        Call {
            method: "POST",
            uri: format!("/api/v1/hooks/trigger/{id}"),
            headers: vec![],
            peer: LAN,
        }
    }
    fn bearer(mut self, token: &str) -> Self {
        self.headers
            .push(("authorization", format!("Bearer {token}")));
        self
    }
    fn header(mut self, k: &'a str, v: &str) -> Self {
        self.headers.push((k, v.to_string()));
        self
    }
    fn method(mut self, m: &'a str) -> Self {
        self.method = m;
        self
    }
    fn query(mut self, q: &str) -> Self {
        self.uri = format!("{}?{q}", self.uri);
        self
    }
    fn from(mut self, peer: &'a str) -> Self {
        self.peer = peer;
        self
    }
    fn build(&self) -> Request<Body> {
        let mut b = Request::builder().method(self.method).uri(&self.uri);
        if !self.headers.iter().any(|(k, _)| *k == "host") {
            b = b.header("host", "192.168.1.10");
        }
        for (k, v) in &self.headers {
            b = b.header(*k, v);
        }
        let mut r = b.body(Body::empty()).unwrap();
        r.extensions_mut()
            .insert(ConnectInfo::<SocketAddr>(self.peer.parse().unwrap()));
        r
    }
    async fn send_to(&self, router: &axum::Router) -> (StatusCode, Value) {
        let resp = router.clone().oneshot(self.build()).await.unwrap();
        let s = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (s, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
    async fn send(&self, app: &TestApp) -> (StatusCode, Value) {
        self.send_to(&app.router).await
    }
}

fn public_router(app: &TestApp) -> axum::Router {
    super::router(app.state.clone()).layer(axum::middleware::from_fn(super::security::public_only))
}

#[tokio::test]
async fn a_token_replaces_the_session_and_the_csrf_header() {
    let app = signed_in_leader("hk_auth", json!({})).await;
    // No link until the owner makes one.
    let (s, v) = Call::post("hk_auth").bearer("ppt_x").send(&app).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "{v}");
    let token = make_token(&app, "hk_auth").await;
    assert!(token.starts_with("ppt_"));

    // Header: fires without a session or CSRF header.
    let (s, v) = Call::post("hk_auth").bearer(&token).send(&app).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    assert_eq!(
        (v["ok"].as_bool(), v["fired"].as_bool()),
        (Some(true), Some(true))
    );
    assert!(app.commands_matching("Stop").await >= 1, "the action ran");
    // Query works too.
    let (s, _) = Call::post("hk_auth")
        .query(&format!("token={token}"))
        .send(&app)
        .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    // Missing / wrong token.
    let (s, v) = Call::post("hk_auth").send(&app).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(v["error"]["code"], "token_required");
    assert_eq!(v["fired"], false);
    let (s, v) = Call::post("hk_auth").bearer("ppt_wrong").send(&app).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(v["error"]["code"], "bad_token");
    // Unknown trigger.
    let (s, _) = Call::post("nope").bearer(&token).send(&app).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // The token opens nothing else: the admin API still wants a session.
    let mut r = Call::post("x").bearer(&token).build();
    *r.uri_mut() = "/api/v1/show".parse().unwrap();
    *r.method_mut() = axum::http::Method::GET;
    let resp = app.router.clone().oneshot(r).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    // DNS rebinding: the Host allow-list still applies on the home network.
    let (s, _) = Call::post("hk_auth")
        .bearer(&token)
        .header("host", "evil.example.com")
        .send(&app)
        .await;
    assert_eq!(s, StatusCode::MISDIRECTED_REQUEST);
}

#[tokio::test]
async fn wrong_tokens_back_off_per_address() {
    let app = signed_in_leader("hk_throttle", json!({})).await;
    let token = make_token(&app, "hk_throttle").await;
    let attacker = "192.168.1.66:4000";
    for _ in 0..5 {
        let (s, _) = Call::post("hk_throttle")
            .bearer("ppt_guess")
            .from(attacker)
            .send(&app)
            .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }
    // Locked out, even with the right token.
    let (s, v) = Call::post("hk_throttle")
        .bearer(&token)
        .from(attacker)
        .send(&app)
        .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "{v}");
    assert_eq!(v["error"]["code"], "throttled");
    // Others (the real doorbell) aren't affected, nor is signing in.
    let (s, _) = Call::post("hk_throttle").bearer(&token).send(&app).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    assert!(app
        .state
        .sessions
        .throttle
        .lock()
        .check(
            Some("192.168.1.66".parse().unwrap()),
            std::time::Instant::now()
        )
        .is_ok());
}

#[tokio::test]
async fn get_links_only_when_allowed_and_head_never_fires() {
    let app = signed_in_leader("hk_get", json!({})).await;
    let token = make_token(&app, "hk_get").await;
    let get = || {
        Call::post("hk_get")
            .method("GET")
            .query(&format!("token={token}"))
    };
    let (s, v) = get().send(&app).await;
    assert_eq!(s, StatusCode::METHOD_NOT_ALLOWED, "{v}");
    assert_eq!(v["error"]["code"], "get_not_allowed");
    let (s, _) = Call::post("hk_get")
        .method("HEAD")
        .bearer(&token)
        .send(&app)
        .await;
    assert_eq!(s, StatusCode::METHOD_NOT_ALLOWED);
    let show = app.state.store.get();
    let mut t = serde_json::to_value(&show.settings.triggers[0]).unwrap();
    t["allowGet"] = json!(true);
    app.json("PUT", "/show/settings", Some(json!({ "triggers": [t] })))
        .await;
    let before = app.commands_matching("Stop").await;
    let (s, _) = get().send(&app).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    assert!(app.commands_matching("Stop").await > before);
    let (s, _) = Call::post("hk_get")
        .method("HEAD")
        .bearer(&token)
        .send(&app)
        .await;
    assert_eq!(
        s,
        StatusCode::METHOD_NOT_ALLOWED,
        "HEAD (link previews) never fires"
    );
}

#[tokio::test]
async fn home_network_only_unless_allowed_from_the_internet() {
    let app = signed_in_leader("hk_net", json!({})).await;
    let token = make_token(&app, "hk_net").await;
    let public = public_router(&app);
    let tunnel_to = |host: &'static str| {
        Call::post("hk_net")
            .bearer(&token)
            .from("127.0.0.1:40000")
            .header("x-forwarded-for", "203.0.113.9")
            .header("host", host)
    };
    let tunnel = || tunnel_to("lights.example.com");
    // Through the public listener (tunnels) and a proxy on this machine: invisible.
    let (s, v) = tunnel().send_to(&public).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{v}");
    let (s, _) = tunnel_to("127.0.0.1").send(&app).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // Straight from a public address (a port forward): invisible too.
    let (s, _) = Call::post("hk_net")
        .bearer(&token)
        .from("203.0.113.9:1")
        .send(&app)
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // Other admin paths stay hidden on the public listener.
    let mut r = tunnel().build();
    *r.uri_mut() = "/api/v1/triggers/hk_net/token".parse().unwrap();
    let resp = public.clone().oneshot(r).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let show = app.state.store.get();
    let mut t = serde_json::to_value(&show.settings.triggers[0]).unwrap();
    t["allowInternet"] = json!(true);
    app.json("PUT", "/show/settings", Some(json!({ "triggers": [t] })))
        .await;
    let (s, v) = tunnel().send_to(&public).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    // Still needs the token from the internet.
    let (s, _) = Call::post("hk_net")
        .bearer("ppt_nope")
        .from("127.0.0.1:40000")
        .header("x-forwarded-for", "203.0.113.10")
        .send_to(&public)
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // The owner sees where it came from.
    let (s, v) = app.json("GET", "/triggers/links", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["links"]["hk_net"]["from"], "203.0.113.9", "{v}");
    assert_eq!(v["links"]["hk_net"]["origin"], "internet");
    assert_eq!(v["links"]["hk_net"]["fired"], true);
    assert!(v["addresses"].as_array().is_some());
}

#[tokio::test]
async fn feature_toggles_and_gates() {
    let app = signed_in_leader("hk_gate", json!({ "cooldownS": 60 })).await;
    let token = make_token(&app, "hk_gate").await;
    let (s, _) = Call::post("hk_gate").bearer(&token).send(&app).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let (s, v) = Call::post("hk_gate").bearer(&token).send(&app).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["error"]["code"], "blocked");
    assert!(
        v["reason"].as_str().unwrap().contains("cooling down"),
        "{v}"
    );
    assert_eq!(v["fired"], false);

    // A surprise while Surprises is off: 409 with the feature's message.
    let show = app.state.store.get();
    let mut t = serde_json::to_value(&show.settings.triggers[0]).unwrap();
    t["cooldownS"] = json!(0);
    t["action"] = json!({ "type": "surprise", "ref": "x", "source": "effect" });
    app.json("PUT", "/show/settings", Some(json!({ "triggers": [t] })))
        .await;
    app.json(
        "PUT",
        "/features",
        Some(json!({ "id": "surprises", "enabled": false })),
    )
    .await;
    let (s, v) = Call::post("hk_gate").bearer(&token).send(&app).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["error"]["code"], "feature_disabled");
    assert!(v["reason"]
        .as_str()
        .unwrap()
        .contains("Settings → Features"));
    // Buttons & triggers off: the link is gone (404).
    app.json(
        "PUT",
        "/features",
        Some(json!({ "id": "triggers", "enabled": false })),
    )
    .await;
    let (s, v) = Call::post("hk_gate").bearer(&token).send(&app).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{v}");
    assert_eq!(v["error"]["code"], "feature_disabled");
}

#[tokio::test]
async fn calls_are_rate_limited() {
    let app = signed_in_leader("hk_rate", json!({})).await;
    let token = make_token(&app, "hk_rate").await;
    for i in 0..crate::services::hooks::PER_ADDRESS_PER_MINUTE {
        let (s, v) = Call::post("hk_rate").bearer(&token).send(&app).await;
        assert_eq!(s, StatusCode::ACCEPTED, "call {i}: {v}");
    }
    let (s, v) = Call::post("hk_rate").bearer(&token).send(&app).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "{v}");
    assert_eq!(v["error"]["code"], "rate_limited");
}

#[tokio::test]
async fn rotate_and_revoke_invalidate_the_old_token_and_secrets_stay_put() {
    let app = signed_in_leader("hk_rot", json!({})).await;
    let first = make_token(&app, "hk_rot").await;
    let stored = app.state.store.get().settings.triggers[0]
        .token_hash
        .clone()
        .unwrap();
    assert_eq!(stored, crate::services::hooks::hash_token(&first));
    assert!(!stored.contains(&first[4..]));

    // GET /show: hint and date, never the hash or the token.
    let (_, _, body) = app
        .send(
            Request::builder()
                .uri("/api/v1/show")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    let text = String::from_utf8(body).unwrap();
    assert!(!text.contains(&stored) && !text.contains(&first));
    assert!(!text.contains("tokenHash"));
    let show: Value = serde_json::from_str(&text).unwrap();
    let t = &show["settings"]["triggers"][0];
    assert_eq!(t["tokenHint"], first[first.len() - 4..]);
    assert!(t["tokenCreatedAt"].is_string());

    // A settings save can't set or clear it (whatever it sends).
    let mut forged = t.clone();
    forged["tokenHash"] = json!(crate::services::hooks::hash_token("ppt_mine"));
    forged["tokenHint"] = json!("mine");
    let (s, v) = app
        .json(
            "PUT",
            "/show/settings",
            Some(json!({ "triggers": [forged] })),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert!(v["triggers"][0].get("tokenHash").is_none(), "{v}");
    let (s, _) = Call::post("hk_rot").bearer("ppt_mine").send(&app).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let mut stripped = t.clone();
    stripped.as_object_mut().unwrap().remove("tokenHint");
    app.json(
        "PUT",
        "/show/settings",
        Some(json!({ "triggers": [stripped] })),
    )
    .await;
    let (s, _) = Call::post("hk_rot").bearer(&first).send(&app).await;
    assert_eq!(s, StatusCode::ACCEPTED, "the token survives a save");

    // Rotate: the old token stops working at once.
    let (s, v) = app.json("POST", "/triggers/hk_rot/token", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["rotated"], true);
    let second = v["token"].as_str().unwrap().to_string();
    assert_ne!(first, second);
    let (s, _) = Call::post("hk_rot").bearer(&first).send(&app).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = Call::post("hk_rot").bearer(&second).send(&app).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    // Revoke: no token works any more.
    let (s, _) = app.json("DELETE", "/triggers/hk_rot/token", None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = Call::post("hk_rot")
        .bearer(&second)
        .from("192.168.1.41:1")
        .send(&app)
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert!(app.state.store.get().settings.triggers[0]
        .token_hint
        .is_none());

    // The admin endpoints need a session (and the CSRF header).
    let mut r = Call::post("x").build();
    *r.uri_mut() = "/api/v1/triggers/hk_rot/token".parse().unwrap();
    r.headers_mut()
        .insert("x-pixelplus-request", "1".parse().unwrap());
    assert_eq!(
        app.router.clone().oneshot(r).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    // Only web-link triggers get one; a trigger that stops being one loses it.
    let token = make_token(&app, "hk_rot").await;
    let mut t = serde_json::to_value(&app.state.store.get().settings.triggers[0]).unwrap();
    t["kind"] = json!("gpio");
    t["gpio"] = json!(17);
    app.json(
        "PUT",
        "/show/settings",
        Some(json!({ "triggers": [t.clone()] })),
    )
    .await;
    assert!(app.state.store.get().settings.triggers[0]
        .token_hash
        .is_none());
    let (s, _) = app.json("POST", "/triggers/hk_rot/token", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    t["kind"] = json!("http");
    app.json("PUT", "/show/settings", Some(json!({ "triggers": [t] })))
        .await;
    let (s, _) = Call::post("hk_rot").bearer(&token).send(&app).await;
    assert_eq!(
        s,
        StatusCode::UNAUTHORIZED,
        "switching back doesn't revive it"
    );
}

#[tokio::test]
async fn old_shows_without_tokens_load() {
    let t: pixelplus_core::model::Trigger = serde_json::from_value(json!({
        "id": "t", "name": "Old", "kind": "http", "action": {"type": "stop"}
    }))
    .unwrap();
    assert!(t.token_hash.is_none() && !t.allow_get && !t.allow_internet);
    let out = serde_json::to_value(&t).unwrap();
    assert!(out.get("allowGet").is_none() && out.get("tokenHash").is_none());
}
