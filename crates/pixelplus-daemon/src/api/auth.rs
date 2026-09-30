//! Optional password protection.
//!
//! * No password set → everything is open (the UI nudges the user to set one).
//! * Password set → a session cookie (`pp_session`) is required, except for
//!   `/api/v1/auth/*`, `/api/v1/public/*`, `/api/v1/system` (so the UI can show
//!   the sign-in screen), cluster calls carrying a valid `X-PixelPlus-Key`, and
//!   local sidecars (loopback + `X-PixelPlus-Local: 1`).

use super::{ApiError, ApiResult};
use crate::state::AppState;
use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{post, put};
use axum::{Json, Router};
use parking_lot::Mutex;
use serde::Deserialize;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

const COOKIE: &str = "pp_session";
const SESSION_TTL: Duration = Duration::from_secs(60 * 60 * 24 * 30);

#[derive(Default)]
pub struct Sessions {
    tokens: Mutex<HashMap<String, Instant>>,
}

impl Sessions {
    fn create(&self) -> String {
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
    if password.chars().count() < 4 {
        return Err(ApiError::bad_request("Use at least 4 characters."));
    }
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(ApiError::internal)
}

pub fn verify_password(hash: &str, password: &str) -> bool {
    PasswordHash::new(hash)
        .map(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
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

/// True when the request carries a valid cluster key.
pub fn has_cluster_key(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(key) = state.identity().cluster_key else {
        return false;
    };
    headers
        .get("x-pixelplus-key")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| constant_time_eq(v.as_bytes(), key.as_bytes()))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// True when the request is authenticated (or no password is set).
pub fn is_authenticated(state: &AppState, headers: &HeaderMap, peer: Option<SocketAddr>) -> bool {
    if state.store.get().settings.security.password_hash.is_none() {
        return true;
    }
    if peer.is_some_and(|p| p.ip().is_loopback())
        && headers.get("x-pixelplus-local").is_some_and(|v| v == "1")
    {
        return true;
    }
    if has_cluster_key(state, headers) {
        return true;
    }
    session_token(headers).is_some_and(|t| state.sessions.valid(&t))
}

/// Middleware guarding `/api/v1/*`.
pub async fn require_auth(
    State(state): State<AppState>,
    peer: super::Peer,
    req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path();
    let open = path.starts_with("/api/v1/auth/")
        || path.starts_with("/api/v1/public/")
        || path == "/api/v1/system"
        || path == "/api/v1/system/setup" && state.identity().role == crate::node::LocalRole::Unconfigured
        // Cluster endpoints authenticate with the cluster key themselves.
        || path.starts_with("/api/v1/cluster/");
    if open || is_authenticated(&state, req.headers(), peer.0) {
        next.run(req).await
    } else {
        ApiError::unauthorized().into_response()
    }
}

#[derive(Deserialize)]
struct LoginBody {
    password: String,
}

async fn login(State(state): State<AppState>, Json(body): Json<LoginBody>) -> ApiResult<Response> {
    let show = state.store.get();
    let Some(hash) = show.settings.security.password_hash.as_deref() else {
        return Ok(Json(serde_json::json!({ "ok": true })).into_response());
    };
    if !verify_password(hash, &body.password) {
        // Slow down guessing.
        tokio::time::sleep(Duration::from_millis(600)).await;
        return Err(ApiError::new(
            axum::http::StatusCode::UNAUTHORIZED,
            "wrong_password",
            "That password isn't right.",
        ));
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
    /// `None` or empty removes the password.
    #[serde(default)]
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
        let current_ok = body.current.as_deref().is_some_and(|c| verify_password(hash, c));
        if !(authed && current_ok) {
            return Err(ApiError::forbidden("Enter your current password to change it."));
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
    fn cookie_parsing() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("a=1; pp_session=abc; b=2"));
        assert_eq!(session_token(&h).as_deref(), Some("abc"));
    }
}
