//! ESP32 sensor nodes (F20): discovery / events on UDP `Config::sensor_port` (32422), adoption, live inputs.
//!
//! Stub created by WS0 (foundation); owned by WS6. `services/mod.rs`
//! already registers [`SensorNodesState`] in `Services::sensornodes` and calls [`start`]
//! from `start_all`, so the owner never needs to edit shared files.

use crate::state::AppState;

/// Runtime state of this service (`state.services.sensornodes`).
#[derive(Default)]
pub struct SensorNodesState {}

/// Start the service (called once from `services::start_all`).
pub fn start(_state: &AppState) {}
