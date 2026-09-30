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
            match serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
            {
                Ok(me) => return Ok(me),
                Err(e) => {
                    // A torn write (power cut) must not keep the daemon from
                    // starting: use the previous version, else start over (the
                    // controller then has to be set up / adopted again).
                    let aside = path.with_extension(format!(
                        "corrupt-{}.json",
                        chrono::Utc::now().format("%Y%m%d%H%M%S")
                    ));
                    std::fs::copy(path, &aside).ok();
                    let bak = path.with_extension("json.bak");
                    if let Some(me) = std::fs::read_to_string(&bak)
                        .ok()
                        .and_then(|t| serde_json::from_str::<NodeIdentity>(&t).ok())
                    {
                        tracing::error!(
                            "{e:#}; kept a copy in {} and used the previous version",
                            aside.display()
                        );
                        me.save(path)?;
                        return Ok(me);
                    }
                    tracing::error!(
                        "{e:#}; kept a copy in {} and started as a new controller",
                        aside.display()
                    );
                }
            }
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

    /// Write atomically (temp file + fsync + rename), keeping one `.bak`.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        use std::io::Write;
        let tmp: PathBuf = path.with_extension("json.tmp");
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&serde_json::to_vec_pretty(self)?)?;
            f.sync_all()?;
        }
        if path.exists() {
            std::fs::copy(path, path.with_extension("json.bak")).ok();
        }
        std::fs::rename(tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrupt_identity_uses_the_backup_or_starts_over() {
        let dir =
            std::env::temp_dir().join(format!("pp-node-{}", pixelplus_core::model::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("node.json");
        let mut me = NodeIdentity::load_or_create(&path).unwrap();
        me.role = LocalRole::Follower;
        me.leader_id = Some("lead".into());
        me.save(&path).unwrap();
        me.name = Some("Garage".into());
        me.save(&path).unwrap();
        std::fs::write(&path, b"").unwrap(); // torn write
        let back = NodeIdentity::load_or_create(&path).unwrap();
        assert_eq!(back.id, me.id);
        assert_eq!(back.leader_id.as_deref(), Some("lead"));
        // Both unreadable: a new identity instead of a daemon that never starts.
        std::fs::write(&path, b"{").unwrap();
        std::fs::write(path.with_extension("json.bak"), b"{").unwrap();
        let fresh = NodeIdentity::load_or_create(&path).unwrap();
        assert_eq!(fresh.role, LocalRole::Unconfigured);
        std::fs::remove_dir_all(dir).ok();
    }
}
