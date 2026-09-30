//! Cluster update orchestration (F15): stage → commit → verify → rollback across nodes.
//!
//! Stub created by WS0 (foundation); owned by WS5. `services/mod.rs`
//! already registers [`UpdatesOrchState`] in `Services::updates_orch` and calls [`start`]
//! from `start_all`, so the owner never needs to edit shared files.

use crate::state::AppState;

/// Runtime state of this service (`state.services.updates_orch`).
#[derive(Default)]
pub struct UpdatesOrchState {}

/// Start the service (called once from `services::start_all`).
pub fn start(_state: &AppState) {}
