//! Shared application state handed to every request handler and service.

use crate::config::Config;
use crate::events::EventBus;
use crate::node::NodeIdentity;
use crate::store::ShowStore;
use parking_lot::RwLock;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState(pub Arc<AppInner>);

pub struct AppInner {
    pub config: Config,
    pub events: EventBus,
    pub store: ShowStore,
    pub identity: RwLock<NodeIdentity>,
    pub sessions: crate::api::auth::Sessions,
    pub started: std::time::Instant,
    /// Registered services (playback engine, cluster, sensors, ...). Each
    /// service stores its handle here during startup.
    pub services: crate::services::Services,
}

impl std::ops::Deref for AppState {
    type Target = AppInner;
    fn deref(&self) -> &AppInner {
        &self.0
    }
}

impl AppState {
    pub fn identity(&self) -> NodeIdentity {
        self.identity.read().clone()
    }

    /// Update and persist this node's identity.
    pub fn set_identity(&self, f: impl FnOnce(&mut NodeIdentity)) -> anyhow::Result<NodeIdentity> {
        let mut id = self.identity.write();
        f(&mut id);
        id.save(&self.config.node_path())?;
        Ok(id.clone())
    }
}
