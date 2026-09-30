//! `/calibration/result` (F1; `/player/calibration` v2 stays in playerapi).
//!
//! Stub created by WS0 (foundation); owned by WS1. Already merged into the
//! `/api/v1` router in `api/mod.rs` (behind auth and the security guard).

use crate::state::AppState;
use axum::Router;

pub fn routes() -> Router<AppState> {
    Router::new()
}
