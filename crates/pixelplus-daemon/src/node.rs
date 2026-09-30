//! This node's identity (`node.json`): who am I, and who is my leader.

use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LocalRole {
    /// Fresh install; the setup wizard has not run yet.
    Unconfigured,
    Leader,
    Follower,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NodeIdentity {
    pub id: String,
    pub role: LocalRole,
    /// Leader base URL (followers only), e.g. `http://192.168.1.20`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leader_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leader_id: Option<String>,
    /// Shared secret for leader <-> follower calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_key: Option<String>,
    /// Board chosen in the wizard when it could not be detected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board: Option<pixelplus_core::model::BoardKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_rev: Option<String>,
    /// Friendly name shown before adoption.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl NodeIdentity {
    pub fn load_or_create(path: &Path) -> anyhow::Result<Self> {
        if path.exists() {
            let text = std::fs::read_to_string(path)?;
            return serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()));
        }
        let id = pixelplus_core::model::new_id();
        let me = NodeIdentity {
            id,
            role: LocalRole::Unconfigured,
            leader_url: None,
            leader_id: None,
            cluster_key: None,
            board: None,
            board_rev: None,
            name: None,
        };
        me.save(path)?;
        Ok(me)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let tmp: PathBuf = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
}
