//! Camera mapping runs (F6/F7): `mapping/<runId>.json` storage.
//!
//! Stub created by WS0 (foundation); owned by WS4. `services/mod.rs`
//! already registers [`MappingState`] in `Services::mapping` and calls [`start`]
//! from `start_all`, so the owner never needs to edit shared files.

use crate::state::AppState;

/// Runtime state of this service (`state.services.mapping`).
#[derive(Default)]
pub struct MappingState {}

/// Start the service (called once from `services::start_all`).
pub fn start(_state: &AppState) {}
