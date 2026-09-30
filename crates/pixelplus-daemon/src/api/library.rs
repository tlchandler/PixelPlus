//! Library tools (F18): `/sequences/tags`, `/playlists/:id/preview`, `/library/history`.
//!
//! Stub created by WS0 (foundation); owned by WS2. Already merged into the
//! `/api/v1` router in `api/mod.rs` (behind auth and the security guard).

use crate::state::AppState;
use axum::Router;

pub fn routes() -> Router<AppState> {
    Router::new()
}
