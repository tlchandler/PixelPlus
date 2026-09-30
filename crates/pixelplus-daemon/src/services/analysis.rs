//! Audio analysis / auto-show / preview job queue (F2, F3): one job at a time, `nice 10`, WS `job` progress.
//!
//! Stub created by WS0 (foundation); owned by WS2. `services/mod.rs`
//! already registers [`AnalysisState`] in `Services::analysis` and calls [`start`]
//! from `start_all`, so the owner never needs to edit shared files.

use crate::state::AppState;

/// Runtime state of this service (`state.services.analysis`).
#[derive(Default)]
pub struct AnalysisState {}

/// Start the service (called once from `services::start_all`).
pub fn start(_state: &AppState) {}
