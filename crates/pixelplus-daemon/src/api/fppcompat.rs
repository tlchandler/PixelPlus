//! xLights "FPP Connect" compatible upload subset (F16): `/config.php`,
//! `/api/system/info`, `/api/sequence/:name/meta`, `/api/media/:name/meta`,
//! `PATCH /api/file/:dir`, playlists, and no-op controller-config endpoints.
//!
//! Stub created by WS0 (foundation); owned by WS6. Unlike every other module
//! these routes are mounted at the ROOT (not under `/api/v1`), outside the
//! normal auth / CSRF layers: handlers must check `settings.xlights.fppConnect`
//! and the upload password themselves (and use WS5's CSRF-exemption hook in
//! `api/security.rs`). The public listener (F14) never mounts them.

use crate::state::AppState;
use axum::Router;

pub fn routes() -> Router<AppState> {
    Router::new()
}
