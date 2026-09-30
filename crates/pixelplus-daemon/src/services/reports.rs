//! Nightly health report (F11): aggregates the journal into `reports/<date>.json`, sends email / push.
//!
//! Stub created by WS0 (foundation); owned by WS6. `services/mod.rs`
//! already registers [`ReportsState`] in `Services::reports` and calls [`start`]
//! from `start_all`, so the owner never needs to edit shared files.

use crate::state::AppState;

/// Runtime state of this service (`state.services.reports`).
#[derive(Default)]
pub struct ReportsState {}

/// Start the service (called once from `services::start_all`).
pub fn start(_state: &AppState) {}
