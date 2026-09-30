//! Season profiles (F8): switching and the daily auto-switch.
//!
//! Stub created by WS0 (foundation); owned by WS6. `services/mod.rs`
//! already registers [`ProfilesState`] in `Services::profiles` and calls [`start`]
//! from `start_all`, so the owner never needs to edit shared files.

use crate::state::AppState;

/// Runtime state of this service (`state.services.profiles`).
#[derive(Default)]
pub struct ProfilesState {}

/// Start the service (called once from `services::start_all`).
pub fn start(_state: &AppState) {}
