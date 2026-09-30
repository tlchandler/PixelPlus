//! Season profiles (F8): CRUD `/profiles`, `/profiles/:id/activate`, `/profiles/capture`, `/profiles/preview-switch/:id`.
//!
//! Stub created by WS0 (foundation); owned by WS6. Already merged into the
//! `/api/v1` router in `api/mod.rs` (behind auth and the security guard).

use crate::state::AppState;
use axum::Router;

pub fn routes() -> Router<AppState> {
    Router::new()
}
