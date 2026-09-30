//! Secure connection (F1, ARCHITECTURE §12.1):
//!
//! * `GET /tls/status` — certificate authority, leaf, listener, URLs.
//! * `POST /tls/rotate {ca}` — new leaf, or (`ca: true`) a new CA: every phone
//!   must install the certificate again.
//! * `GET /public/tls` — what the `/trust` page needs (no sign-in).
//! * `GET /public/ca.crt` — the CA certificate (DER) for phones (no sign-in:
//!   it is public data; phones compare the fingerprint with the OLED /
//!   Settings before trusting it).
//! * `GET /public/ca.mobileconfig` — the same for iPhone / iPad.

use super::{ApiError, ApiResult, Peer};
use crate::listeners::ViaHttps;
use crate::services::tls::{self, Material};
use crate::state::AppState;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

/// Did this request reach us over HTTPS (our listener, or a local
/// `tailscale serve` / tunnel that terminates TLS)?
fn secure_now(via: Option<Extension<ViaHttps>>, peer: Peer, headers: &HeaderMap) -> bool {
    if via.is_some() {
        return true;
    }
    peer.0.is_some_and(|p| p.ip().is_loopback())
        && headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("https"))
}

fn https_url(host: &str, port: u16) -> String {
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    if port == 443 {
        format!("https://{host}")
    } else {
        format!("https://{host}:{port}")
    }
}

/// `https://…` addresses phones on the LAN can use, best first: IPv4, the
/// `.local` name, then the rest (no localhost).
fn lan_urls(m: &Material, port: u16) -> Vec<String> {
    let names = &m.leaf.meta.names;
    let mut v4: Vec<&String> = names
        .iter()
        .filter(|n| {
            n.parse::<std::net::Ipv4Addr>()
                .is_ok_and(|ip| !ip.is_loopback())
        })
        .collect();
    // Home networks first, then Tailscale / link-local.
    v4.sort_by_key(|n| {
        let ip: std::net::Ipv4Addr = n.parse().expect("filtered");
        let o = ip.octets();
        match o[0] {
            192 | 10 | 172 => 0,
            100 => 2,
            169 => 3,
            _ => 1,
        }
    });
    let mut out: Vec<String> = v4.into_iter().map(|n| https_url(n, port)).collect();
    for n in names {
        if n.ends_with(".local") {
            out.push(https_url(n, port));
        }
    }
    out
}

fn remote_urls(state: &AppState) -> (Option<String>, Option<String>) {
    let remote = &state.store.get().settings.remote;
    let tailscale = remote
        .tailscale
        .as_ref()
        .filter(|t| t.enabled && t.serve_admin)
        .and_then(|t| t.dns_name.as_deref())
        .filter(|n| !n.is_empty())
        .map(|n| format!("https://{}", n.trim_end_matches('.')));
    let tunnel = remote
        .cloudflare
        .as_ref()
        .and_then(|c| c.admin_host.as_deref())
        .filter(|h| !h.is_empty())
        .map(|h| format!("https://{h}"));
    (tailscale, tunnel)
}

fn role(state: &AppState) -> &'static str {
    match state.identity().role {
        crate::node::LocalRole::Unconfigured => "unconfigured",
        crate::node::LocalRole::Leader => "leader",
        crate::node::LocalRole::Follower => "follower",
    }
}

/// The `/tls/status` payload.
pub fn status(state: &AppState, secure: bool) -> Value {
    let settings = state.store.get().settings.https.clone();
    let tls_state = &state.services.tls;
    let listener = tls_state.listener();
    let port = state.config.https_port;
    let m = tls_state.material();
    let (tailscale, tunnel) = remote_urls(state);
    let mut urls = json!({ "lan": m.as_ref().map(|m| lan_urls(m, port)).unwrap_or_default() });
    if let Some(t) = tailscale {
        urls["tailscale"] = json!(t);
    }
    if let Some(t) = tunnel {
        urls["tunnel"] = json!(t);
    }
    // Extra names the CA can't vouch for (shown next to the setting).
    let rejected: Vec<&String> = m
        .as_ref()
        .map(|m| {
            settings
                .extra_names
                .iter()
                .filter(|n| {
                    !tls::permitted(&n.trim().to_ascii_lowercase(), &m.ca_meta.permitted_dns)
                })
                .collect()
        })
        .unwrap_or_default();
    let active = tls::active(state);
    json!({
        "enabled": settings.enabled,
        "port": port,
        "active": active,
        "listening": listener.listening && active,
        "error": listener.error.or_else(|| tls_state.last_error()),
        "role": role(state),
        "caFingerprint": m.as_ref().map(|m| m.ca_fingerprint.clone()).unwrap_or_default(),
        "caSubject": m.as_ref().map(|m| m.ca_meta.common_name.clone()).unwrap_or_default(),
        "caCreatedAt": m.as_ref().map(|m| m.ca_meta.created_at.clone()),
        "caNotAfter": m.as_ref().map(|m| m.ca_meta.not_after.clone()),
        "leafNames": m.as_ref().map(|m| m.leaf.meta.names.clone()).unwrap_or_default(),
        "leafNotAfter": m.as_ref().map(|m| m.leaf.meta.not_after.clone()).unwrap_or_default(),
        "leafIssuedAt": m.as_ref().map(|m| m.leaf.meta.issued_at.clone()),
        "rejectedNames": rejected,
        "urls": urls,
        "secureNow": secure,
    })
}

async fn get_status(
    State(state): State<AppState>,
    via: Option<Extension<ViaHttps>>,
    peer: Peer,
    headers: HeaderMap,
) -> Json<Value> {
    state.services.tls.show_fingerprint();
    Json(status(&state, secure_now(via, peer, &headers)))
}

#[derive(Deserialize)]
struct RotateBody {
    #[serde(default)]
    ca: bool,
}

async fn rotate(
    State(state): State<AppState>,
    via: Option<Extension<ViaHttps>>,
    peer: Peer,
    headers: HeaderMap,
    Json(b): Json<RotateBody>,
) -> ApiResult<Json<Value>> {
    if state.identity().role == crate::node::LocalRole::Follower {
        return Err(ApiError::bad_request(
            "This controller follows its show leader; manage the secure connection on the leader.",
        ));
    }
    tls::ensure(&state, b.ca, true)
        .await
        .map_err(|e| ApiError::internal(format!("Couldn't make new certificates: {e:#}")))?;
    if b.ca {
        tracing::warn!(
            "The secure-connection certificate authority was replaced; phones must trust it again"
        );
        state.events.toast(
            crate::events::ToastKind::Warning,
            "New secure-connection certificate: phones need to install it again (open /trust on each phone).",
        );
    }
    Ok(Json(status(&state, secure_now(via, peer, &headers))))
}

/// `GET /public/tls`: for the `/trust` page (no sign-in). LAN addresses only
/// for visitors on the local network (never through a tunnel).
async fn public_status(
    State(state): State<AppState>,
    via: Option<Extension<ViaHttps>>,
    peer: Peer,
    headers: HeaderMap,
) -> Json<Value> {
    state.services.tls.show_fingerprint();
    let m = state.services.tls.material();
    let port = state.config.https_port;
    let lan = peer.0.is_some_and(|p| super::security::lan_peer(p.ip()))
        && !super::security::forwarded(&headers);
    let urls = match (&m, lan) {
        (Some(m), true) => lan_urls(m, port),
        _ => vec![],
    };
    Json(json!({
        "available": m.is_some() && tls::active(&state),
        "role": role(&state),
        "port": port,
        "caFingerprint": m.as_ref().map(|m| m.ca_fingerprint.clone()),
        "caSubject": m.as_ref().map(|m| m.ca_meta.common_name.clone()),
        "leafNames": if lan { m.as_ref().map(|m| m.leaf.meta.names.clone()).unwrap_or_default() } else { vec![] },
        "urls": urls,
        "secureNow": secure_now(via, peer, &headers),
    }))
}

fn material(state: &AppState) -> ApiResult<Arc<Material>> {
    state.services.tls.material().ok_or_else(|| {
        let why = if state.identity().role == crate::node::LocalRole::Follower {
            "This controller follows its show leader: open this page on the leader."
        } else {
            "The secure connection isn't set up on this controller yet. Turn it on in Settings → Secure connection."
        };
        ApiError::new(StatusCode::NOT_FOUND, "no_certificate", why)
    })
}

fn download(body: Vec<u8>, content_type: &'static str, filename: &str) -> Response {
    let mut resp = (StatusCode::OK, body).into_response();
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    resp
}

async fn ca_crt(State(state): State<AppState>) -> ApiResult<Response> {
    let m = material(&state)?;
    state.services.tls.show_fingerprint();
    Ok(download(
        m.ca_der.clone(),
        "application/x-x509-ca-cert",
        "PixelPlus-CA.crt",
    ))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A UUID derived from `seed` (profiles must keep the same UUID so iOS
/// replaces, rather than duplicates, an installed profile).
fn uuid_from(seed: &str) -> String {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(seed.as_bytes());
    let h: String = d.iter().take(16).map(|b| format!("{b:02X}")).collect();
    format!(
        "{}-{}-4{}-8{}-{}",
        &h[0..8],
        &h[8..12],
        &h[13..16],
        &h[17..20],
        &h[20..32]
    )
}

/// The iOS configuration profile installing `m`'s CA.
pub fn mobileconfig(m: &Material) -> String {
    let short: String = m.ca_fingerprint.replace(':', "").chars().take(12).collect();
    let name = xml_escape(&m.ca_meta.common_name);
    let data = tls::b64_encode(&m.ca_der);
    let wrapped: Vec<String> = data
        .as_bytes()
        .chunks(64)
        .map(|c| format!("\t\t\t{}", String::from_utf8_lossy(c)))
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>PayloadContent</key>
	<array>
		<dict>
			<key>PayloadCertificateFileName</key>
			<string>PixelPlus-CA.crt</string>
			<key>PayloadContent</key>
			<data>
{data}
			</data>
			<key>PayloadDescription</key>
			<string>Lets this device open your light show's pages securely.</string>
			<key>PayloadDisplayName</key>
			<string>{name}</string>
			<key>PayloadIdentifier</key>
			<string>app.pixelplus.ca.{short}.cert</string>
			<key>PayloadType</key>
			<string>com.apple.security.root</string>
			<key>PayloadUUID</key>
			<string>{cert_uuid}</string>
			<key>PayloadVersion</key>
			<integer>1</integer>
		</dict>
	</array>
	<key>PayloadDescription</key>
	<string>Trust your PixelPlus light show so its camera and microphone pages work. It can only vouch for devices on your home network.</string>
	<key>PayloadDisplayName</key>
	<string>PixelPlus secure connection</string>
	<key>PayloadIdentifier</key>
	<string>app.pixelplus.ca.{short}</string>
	<key>PayloadRemovalDisallowed</key>
	<false/>
	<key>PayloadType</key>
	<string>Configuration</string>
	<key>PayloadUUID</key>
	<string>{profile_uuid}</string>
	<key>PayloadVersion</key>
	<integer>1</integer>
</dict>
</plist>
"#,
        data = wrapped.join("\n"),
        cert_uuid = uuid_from(&format!("cert:{}", m.ca_fingerprint)),
        profile_uuid = uuid_from(&format!("profile:{}", m.ca_fingerprint)),
    )
}

async fn ca_mobileconfig(State(state): State<AppState>) -> ApiResult<Response> {
    let m = material(&state)?;
    Ok(download(
        mobileconfig(&m).into_bytes(),
        "application/x-apple-aspen-config",
        "PixelPlus.mobileconfig",
    ))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/tls/status", get(get_status))
        .route("/tls/rotate", post(rotate))
        .route("/public/tls", get(public_status))
        .route("/public/ca.crt", get(ca_crt))
        .route("/public/ca.mobileconfig", get(ca_mobileconfig))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::testkit::TestApp;
    use axum::body::Body;
    use axum::http::Request;

    #[tokio::test]
    async fn status_before_and_after_certificates() {
        let app = TestApp::new();
        let (st, v) = app.json("GET", "/tls/status", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["enabled"], true);
        assert_eq!(v["caFingerprint"], "");
        assert_eq!(v["secureNow"], false);
        assert!(v["urls"]["lan"].as_array().unwrap().is_empty());
        // No CA yet: a friendly 404.
        let (st, v) = app.json("GET", "/public/ca.crt", None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        assert_eq!(v["error"]["code"], "no_certificate");

        tls::ensure(&app.state, false, false).await.unwrap();
        let (_, v) = app.json("GET", "/tls/status", None).await;
        let fp = v["caFingerprint"].as_str().unwrap().to_string();
        assert_eq!(fp.len(), 95);
        assert!(v["caSubject"]
            .as_str()
            .unwrap()
            .starts_with("PixelPlus Local CA"));
        assert!(v["leafNames"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n == "localhost"));

        // Download: DER with the advertised fingerprint.
        let req = Request::get("/api/v1/public/ca.crt")
            .body(Body::empty())
            .unwrap();
        let (st, h, body) = app.send(req).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(h[header::CONTENT_TYPE], "application/x-x509-ca-cert");
        assert!(h[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .contains("PixelPlus-CA.crt"));
        assert_eq!(body[0], 0x30, "DER SEQUENCE");
        assert_eq!(tls::fingerprint(&body), fp);

        // iOS profile embeds the same certificate.
        let req = Request::get("/api/v1/public/ca.mobileconfig")
            .body(Body::empty())
            .unwrap();
        let (st, h, body) = app.send(req).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(h[header::CONTENT_TYPE], "application/x-apple-aspen-config");
        let text = String::from_utf8(body).unwrap();
        assert!(text.contains("com.apple.security.root"));
        let der = app.state.services.tls.material().unwrap().ca_der.clone();
        assert!(text.contains(&tls::b64_encode(&der)[..64]));

        // Public summary (LAN peer in the test kit) has the addresses.
        let (st, v) = app.json("GET", "/public/tls", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["caFingerprint"], fp);
        assert!(v["leafNames"].as_array().is_some());
    }

    #[tokio::test]
    async fn rotate_leaf_keeps_ca_rotate_ca_changes_it() {
        let app = TestApp::new();
        tls::ensure(&app.state, false, false).await.unwrap();
        let before = app.state.services.tls.material().unwrap();
        let (st, v) = app
            .json("POST", "/tls/rotate", Some(json!({"ca": false})))
            .await;
        assert_eq!(st, StatusCode::OK, "{v}");
        let after = app.state.services.tls.material().unwrap();
        assert_eq!(before.ca_fingerprint, after.ca_fingerprint);
        assert_ne!(before.leaf.cert_der, after.leaf.cert_der);
        let (st, v) = app
            .json("POST", "/tls/rotate", Some(json!({"ca": true})))
            .await;
        assert_eq!(st, StatusCode::OK);
        assert_ne!(v["caFingerprint"], json!(before.ca_fingerprint));
        // Idempotent re-check keeps the same leaf.
        let leaf = app
            .state
            .services
            .tls
            .material()
            .unwrap()
            .leaf
            .cert_der
            .clone();
        tls::ensure(&app.state, false, false).await.unwrap();
        assert_eq!(
            app.state.services.tls.material().unwrap().leaf.cert_der,
            leaf
        );
    }

    #[tokio::test]
    async fn secure_now_and_rejected_names() {
        let app = TestApp::new();
        app.state
            .store
            .update(|s| {
                s.settings.https.extra_names =
                    vec!["lights.home.arpa".into(), "lights.example.com".into()];
                Ok(())
            })
            .await
            .unwrap();
        tls::ensure(&app.state, false, false).await.unwrap();
        let mut req = Request::get("/api/v1/tls/status")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(ViaHttps);
        let (_, _, body) = app.send(req).await;
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["secureNow"], true);
        assert_eq!(v["rejectedNames"], json!(["lights.example.com"]));
        assert!(v["leafNames"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n == "lights.home.arpa"));
        // A local TLS-terminating proxy (tailscale serve) counts too.
        let mut req = Request::get("/api/v1/tls/status")
            .header("x-forwarded-proto", "https")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo::<std::net::SocketAddr>(
                "127.0.0.1:5000".parse().unwrap(),
            ));
        let (_, _, body) = app.send(req).await;
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["secureNow"], true);
        // ... but not a LAN client claiming it.
        let req = Request::get("/api/v1/tls/status")
            .header("x-forwarded-proto", "https")
            .body(Body::empty())
            .unwrap();
        let (_, _, body) = app.send(req).await;
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["secureNow"], false);
    }

    #[test]
    fn urls_and_uuids() {
        assert_eq!(https_url("192.168.1.5", 443), "https://192.168.1.5");
        assert_eq!(https_url("fd00::1", 8443), "https://[fd00::1]:8443");
        let u = uuid_from("x");
        assert_eq!(u.len(), 36);
        assert_eq!(u, uuid_from("x"));
        assert_ne!(u, uuid_from("y"));
        assert_eq!(xml_escape("a<b&\"c\">"), "a&lt;b&amp;&quot;c&quot;&gt;");
    }
}
