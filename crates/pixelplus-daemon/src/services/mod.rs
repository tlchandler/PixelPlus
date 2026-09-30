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
    /// Playback engine (set by `player::engine::start`).
    pub player: OnceLock<crate::player::PlayerHandle>,
    // Other services register their handles here (one field per service).
}

impl Services {
    /// The playback engine handle.
    ///
    /// # Panics
    /// Only if called before startup finished (a programming error).
    pub fn player(&self) -> &crate::player::PlayerHandle {
        self.player.get().expect("player service not started")
    }

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
