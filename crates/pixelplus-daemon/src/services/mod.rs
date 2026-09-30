//! Long-running services (playback engine, cluster, scheduler, sensors, ...).
//!
//! Each service is started from [`start_all`] and registers a handle in
//! [`Services`] so API handlers can reach it. Handles are set once at startup.

// System, content & integrations workstream.
pub mod alerts;
pub mod games;
pub mod geometry;
pub mod health;
pub mod logs;
pub mod media;
pub mod mqtt;
pub mod network;
pub mod oled;
pub mod paths;
pub mod platform;
pub mod provision;
pub mod requests;
pub mod seed;
pub mod sensors;
pub mod setup;
pub mod snapshots;
pub mod system;
pub mod triggers;
pub mod tts;
pub mod updates;

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
    pub cluster: OnceLock<crate::cluster::ClusterHandle>,
    // System, content & integrations workstream (always present).
    pub sensors: sensors::SensorState,
    pub alerts: alerts::AlertState,
    pub requests: requests::RequestQueue,
    pub health: health::HealthState,
    pub mqtt: mqtt::MqttState,
    pub faults: crate::api::test::FaultState,
    pub tools: crate::api::tools::ToolsState,
    /// Root helper jobs (`pixelplus-helper@<verb>.service`) and their progress.
    pub helpers: platform::HelperJobs,
    /// Output tap for `GET /debug/output` (only with `PIXELPLUS_DEV` or the sim output).
    pub debug_output: OnceLock<std::sync::Arc<crate::player::debugtap::OutputTap>>,
}

impl Services {
    /// The playback engine handle.
    ///
    /// # Panics
    /// Only if called before startup finished (a programming error).
    #[allow(dead_code)] // handlers use `api::content::player`, which errors instead of panicking
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
    // The playback engine first: the cluster and other services talk to it.
    crate::player::engine::start(_state).await?;
    crate::cluster::start(_state).await?;
    crate::services::system::start(_state).await;
    Ok(())
}
