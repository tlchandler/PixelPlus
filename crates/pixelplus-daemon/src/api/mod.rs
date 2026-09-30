//! HTTP API (`/api/v1`) and static UI serving.

pub mod auth;
pub mod cluster;
pub mod crud;
mod error;
pub mod nodes;
pub mod show;
pub mod ws;
// System, content & integrations workstream.
pub mod content;
pub mod debug;
pub mod effectsapi;
pub mod games;
pub mod import;
pub mod overlay;
pub mod playerapi;
pub mod public;
pub mod security;
#[cfg(test)]
mod security_tests;
pub mod system;
pub mod test;
#[cfg(test)]
pub mod testkit;
pub mod tools;
// Feature wave (ARCHITECTURE §12). Stubs created by WS0; one owner per file.
pub mod autoshow; // WS2 (F2)
pub mod calibration; // WS1 (F1)
pub mod fppcompat; // WS6 (F16), root-mounted
pub mod journal; // WS0 (F11)
pub mod library; // WS2 (F18)
pub mod mapping; // WS4 (F6)
pub mod pixelcount; // WS4 (F7)
pub mod power; // WS3 (F12)
pub mod preview; // WS2 (F3)
pub mod profiles; // WS6 (F8)
pub mod remote; // WS5 (F14)
pub mod reports; // WS6 (F11)
pub mod sensornodes; // WS6 (F20)
pub mod tls; // WS1 (F1)
pub mod wizard; // WS4 (F9)

pub use error::{ApiError, ApiResult};

/// The remote address of the request, if known.
#[derive(Debug, Clone, Copy)]
pub struct Peer(pub Option<std::net::SocketAddr>);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for Peer {
    type Rejection = std::convert::Infallible;
    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(Peer(
            parts
                .extensions
                .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                .map(|c| c.0),
        ))
    }
}

use crate::state::AppState;
use axum::http::{header, HeaderValue};
use axum::routing::get;
use axum::Router;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

/// Build the complete application router.
pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .merge(auth::routes())
        .merge(crud::all_routes())
        .merge(show::routes())
        .merge(cluster::routes())
        .merge(nodes::routes())
        .merge(system::routes())
        .merge(content::routes())
        .merge(import::routes())
        .merge(playerapi::routes())
        .merge(test::routes())
        .merge(tools::routes())
        .merge(effectsapi::routes())
        .merge(overlay::routes())
        .merge(games::routes())
        .merge(public::routes())
        .merge(debug::routes())
        // Feature wave (ARCHITECTURE §12).
        .merge(tls::routes())
        .merge(calibration::routes())
        .merge(library::routes())
        .merge(autoshow::routes())
        .merge(preview::routes())
        .merge(mapping::routes())
        .merge(wizard::routes())
        .merge(pixelcount::routes())
        .merge(profiles::routes())
        .merge(reports::routes())
        .merge(remote::routes())
        .merge(power::routes())
        .merge(sensornodes::routes())
        .merge(journal::routes())
        .route("/ws", get(ws::handler))
        .fallback(|| async { ApiError::not_found("That API endpoint") })
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ))
        // Host allow-list, CSRF header, WebSocket origin (before auth).
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            security::guard,
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ));

    // SPA: serve static files, fall back to index.html for client-side routes.
    let web = &state.config.web_dir;
    let spa = ServeDir::new(web)
        .precompressed_gzip()
        .fallback(ServeFile::new(web.join("index.html")));

    let csp = std::sync::Arc::new(security::CspCache::new(web));
    Router::new()
        .nest("/api/v1", api)
        // xLights FPP Connect paths live at the root (F16; checks its own auth).
        .merge(fppcompat::routes())
        .fallback_service(spa)
        .layer(axum::middleware::from_fn_with_state(csp, security::headers))
        .layer(tower_http::compression::CompressionLayer::new())
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}
