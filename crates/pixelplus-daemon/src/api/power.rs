//! Power (F12): CRUD `/power-supplies`, `/power/live` (`/power/estimate` stays in tools).
//!
//! Stub created by WS0 (foundation); owned by WS3. Already merged into the
//! `/api/v1` router in `api/mod.rs` (behind auth and the security guard).

use crate::state::AppState;
use axum::Router;

pub fn routes() -> Router<AppState> {
    Router::new()
}
