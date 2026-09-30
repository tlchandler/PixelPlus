//! Long-running services (playback engine, cluster, scheduler, sensors, ...).
//!
//! Each service is started from [`start_all`] and registers a handle in
//! [`Services`] so API handlers can reach it. Handles are set once at startup.

use crate::state::AppState;
use serde_json::Value;
use std::sync::OnceLock;

/// Registry of service handles. Every field is set exactly once during startup.
#[derive(Default)]
pub struct Services {
    /// Latest JSON payload per WebSocket message type (`status`, `nodes`,
    /// `sensors`), replayed to newly connected clients.
    last: parking_lot::Mutex<std::collections::BTreeMap<&'static str, Value>>,
    /// Placeholder so the struct is never empty; services add fields below.
    _private: OnceLock<()>,
}

impl Services {
    /// Remember the latest payload of a periodically published message.
    pub fn remember(&self, kind: &'static str, data: Value) {
        self.last.lock().insert(kind, data);
    }

    pub fn snapshot_for_new_client(&self) -> Vec<(&'static str, Value)> {
        self.last
            .lock()
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect()
    }
}

/// Start every background service.
pub async fn start_all(_state: &AppState) -> anyhow::Result<()> {
    Ok(())
}
