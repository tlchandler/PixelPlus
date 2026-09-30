//! TLS certificates for the HTTPS listener (F1): local name-constrained CA, leaf re-issue on IP/hostname change.
//!
//! Stub created by WS0 (foundation); owned by WS1. `services/mod.rs`
//! already registers [`TlsState`] in `Services::tls` and calls [`start`]
//! from `start_all`, so the owner never needs to edit shared files.

use crate::state::AppState;

/// Runtime state of this service (`state.services.tls`).
#[derive(Default)]
pub struct TlsState {}

/// Start the service (called once from `services::start_all`).
pub fn start(_state: &AppState) {}
