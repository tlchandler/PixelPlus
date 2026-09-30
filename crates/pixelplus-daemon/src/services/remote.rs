//! Remote access (F14): Tailscale / Cloudflare state via root helper verbs; public listener.
//!
//! Stub created by WS0 (foundation); owned by WS5. `services/mod.rs`
//! already registers [`RemoteState`] in `Services::remote` and calls [`start`]
//! from `start_all`, so the owner never needs to edit shared files.

use crate::state::AppState;

/// Runtime state of this service (`state.services.remote`).
#[derive(Default)]
pub struct RemoteState {}

/// Start the service (called once from `services::start_all`).
pub fn start(_state: &AppState) {}
