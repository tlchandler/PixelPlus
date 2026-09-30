//! HTTP API (`/api/v1`) and static UI serving.

pub mod auth;
pub mod cluster; pub mod nodes;
pub mod crud;
mod error;
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
pub mod system;
pub mod test;
pub mod tools;
#[cfg(test)]
pub mod testkit;
#[cfg(test)]
mod security_tests;

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
        .merge(cluster::routes()).merge(nodes::routes())
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
        .route("/ws", get(ws::handler))
        .fallback(|| async { ApiError::not_found("That API endpoint") })
        .layer(axum::middleware::from_fn_with_state(state.clone(), auth::require_auth))
        // Host allow-list, CSRF header, WebSocket origin (before auth).
        .layer(axum::middleware::from_fn_with_state(state.clone(), security::guard))
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
        .fallback_service(spa)
        .layer(axum::middleware::from_fn_with_state(csp, security::headers))
        .layer(tower_http::compression::CompressionLayer::new())
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}
