//! Remote access (F14): `/remote/status`, `/remote/tailscale/*`,
//! `/remote/cloudflare/*`, `/remote/test` (behind auth and the security
//! guard like every admin route). See `services::remote`.

use super::playerapi::body_or_default;
use super::{ApiError, ApiResult};
use crate::services::platform::HelperStatus;
use crate::services::remote::{self, CloudflareToken, RemoteStatus, TailscaleUp, Toggle};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/remote/status", get(status))
        .route("/remote/tailscale/{action}", post(tailscale))
        .route("/remote/cloudflare/{action}", post(cloudflare))
        .route("/remote/test", post(test))
}

#[derive(Deserialize, Default)]
struct StatusQuery {
    #[serde(default)]
    fresh: bool,
}

async fn status(State(state): State<AppState>, Query(q): Query<StatusQuery>) -> Json<RemoteStatus> {
    Json(remote::status(&state, q.fresh).await)
}

fn reply(job: HelperStatus) -> Json<Value> {
    Json(json!({ "ok": true, "job": job }))
}

/// `POST /remote/tailscale/{install|up|serve|funnel|down}`.
async fn tailscale(
    State(state): State<AppState>,
    Path(action): Path<String>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let job = match action.as_str() {
        "install" => remote::tailscale_install(&state).await?,
        "up" => remote::tailscale_up(&state, body_or_default::<TailscaleUp>(&body)?).await?,
        "serve" => remote::tailscale_serve(&state, body_or_default::<Toggle>(&body)?.on).await?,
        "funnel" => remote::tailscale_funnel(&state, body_or_default::<Toggle>(&body)?.on).await?,
        "down" => remote::tailscale_down(&state).await?,
        _ => return Err(ApiError::not_found("That remote access action")),
    };
    Ok(reply(job))
}

/// `POST /remote/cloudflare/{install|quick|token|hosts|stop}`.
async fn cloudflare(
    State(state): State<AppState>,
    Path(action): Path<String>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let job = match action.as_str() {
        "install" => remote::cloudflare_install(&state).await?,
        "quick" => remote::cloudflare_quick(&state, body_or_default::<Toggle>(&body)?.on).await?,
        "token" => {
            remote::cloudflare_token(&state, body_or_default::<CloudflareToken>(&body)?).await?
        }
        "hosts" => {
            remote::cloudflare_hosts(&state, &body_or_default::<CloudflareToken>(&body)?).await?;
            return Ok(Json(json!({ "ok": true })));
        }
        "stop" => remote::cloudflare_stop(&state).await?,
        _ => return Err(ApiError::not_found("That remote access action")),
    };
    Ok(reply(job))
}

#[derive(Deserialize)]
struct TestBody {
    url: String,
}

async fn test(State(state): State<AppState>, Json(b): Json<TestBody>) -> ApiResult<Json<Value>> {
    Ok(Json(remote::test(&state, &b.url).await?))
}

#[cfg(test)]
mod tests {
    use crate::api::testkit::TestApp;
    use axum::http::StatusCode;
    use serde_json::json;

    #[tokio::test]
    async fn status_and_validation_without_a_helper() {
        let app = TestApp::new();
        let (s, v) = app.json("GET", "/remote/status", None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["canManage"], false);
        assert!(v["tailscale"]["state"].is_string());
        assert_eq!(v["cloudflare"]["urls"], json!([]));
        // Bad input is refused before anything runs.
        let (s, _) = app
            .json(
                "POST",
                "/remote/tailscale/up",
                Some(json!({"authKey": "not-a-key"})),
            )
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, _) = app
            .json(
                "POST",
                "/remote/cloudflare/token",
                Some(json!({"token": "short"})),
            )
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        // The admin can't be exposed without a password.
        let (s, v) = app
            .json(
                "POST",
                "/remote/cloudflare/hosts",
                Some(json!({"publicHost": "lights.example.com", "adminHost": "admin.example.com"})),
            )
            .await;
        assert_eq!(s, StatusCode::CONFLICT);
        assert_eq!(v["error"]["code"], "password_required");
        let (s, v) = app
            .json("POST", "/remote/tailscale/serve", Some(json!({"on": true})))
            .await;
        assert_eq!(s, StatusCode::CONFLICT);
        assert_eq!(v["error"]["code"], "password_required");
        // Public host only is fine, and becomes the request page's address.
        let (s, _) = app
            .json(
                "POST",
                "/remote/cloudflare/hosts",
                Some(json!({"publicHost": "https://Lights.Example.com/"})),
            )
            .await;
        assert_eq!(s, StatusCode::OK);
        let show = app.state.store.get();
        let cf = show.settings.remote.cloudflare.as_ref().unwrap();
        assert_eq!(cf.public_host.as_deref(), Some("lights.example.com"));
        assert_eq!(
            show.settings.requests.public_url.as_deref(),
            Some("https://lights.example.com/request")
        );
        // Helper-driven actions say why they can't run here.
        let (s, _) = app.json("POST", "/remote/tailscale/install", None).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        let (s, _) = app.json("POST", "/remote/tailscale/nope", None).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        // Only our own addresses can be tested (no open proxy).
        let (s, _) = app
            .json(
                "POST",
                "/remote/test",
                Some(json!({"url": "https://169.254.169.254/"})),
            )
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, _) = app
            .json(
                "POST",
                "/remote/test",
                Some(json!({"url": "http://lights.example.com/"})),
            )
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }
}
