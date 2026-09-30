//! Auto light show and analysis (F2): `/autoshow/styles`, `/autoshow`, `/autoshow/preview`, `/media/:id/analysis`, `/media/:id/analyze`, `/sequences/:id/regenerate`.
//!
//! Stub created by WS0 (foundation); owned by WS2. Already merged into the
//! `/api/v1` router in `api/mod.rs` (behind auth and the security guard).

use crate::state::AppState;
use axum::Router;

pub fn routes() -> Router<AppState> {
    Router::new()
}
