//! Remote access (F14): `/remote/status`, `/remote/tailscale/*`, `/remote/cloudflare/*`, `/remote/test`.
//!
//! Stub created by WS0 (foundation); owned by WS5. Already merged into the
//! `/api/v1` router in `api/mod.rs` (behind auth and the security guard).

use crate::state::AppState;
use axum::Router;

pub fn routes() -> Router<AppState> {
    Router::new()
}
