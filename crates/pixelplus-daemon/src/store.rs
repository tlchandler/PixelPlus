//! The show store: owns the in-memory [`Show`], persists it atomically to
//! `show.json`, bumps `version` on every change and notifies listeners.
//!
//! All mutations go through [`ShowStore::update`], which gives callers a
//! mutable copy, validates the result, persists it, and only then publishes
//! it. A failed update leaves the stored show untouched.

use crate::events::EventBus;
use anyhow::Context;
use parking_lot::RwLock;
use pixelplus_core::model::Show;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::watch;

#[derive(Clone)]
pub struct ShowStore {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    show: RwLock<Arc<Show>>,
    /// Serializes writers so read-modify-write cycles never interleave.
    write_lock: tokio::sync::Mutex<()>,
    changed: watch::Sender<u64>,
    events: EventBus,
}

impl ShowStore {
    /// Load `show.json` (or start from an empty show).
    pub fn load(path: &Path, events: EventBus) -> anyhow::Result<Self> {
        let show = if path.exists() {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            match serde_json::from_str::<Show>(&text) {
                Ok(mut show) => {
                    crate::services::paths::sanitize_show(&mut show);
                    show
                }
                Err(e) => {
                    // Never lose a user's show: keep the unreadable file aside.
                    let backup = path.with_extension(format!(
                        "corrupt-{}.json",
                        chrono::Utc::now().format("%Y%m%d%H%M%S")
                    ));
                    std::fs::copy(path, &backup).ok();
                    tracing::error!(
                        "show.json could not be parsed ({e}); saved a copy to {} and started fresh",
                        backup.display()
                    );
                    Show::default()
                }
            }
        } else {
            Show::default()
        };
        let (changed, _) = watch::channel(show.version);
        Ok(ShowStore {
            inner: Arc::new(Inner {
                path: path.to_path_buf(),
                show: RwLock::new(Arc::new(show)),
                write_lock: tokio::sync::Mutex::new(()),
                changed,
                events,
            }),
        })
    }

    /// Cheap snapshot of the current show.
    pub fn get(&self) -> Arc<Show> {
        self.inner.show.read().clone()
    }

    pub fn version(&self) -> u64 {
        self.inner.show.read().version
    }

    /// Watch channel that yields the new version after every change.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.inner.changed.subscribe()
    }

    /// Apply a mutation. The closure may return an error to abort.
    pub async fn update<R>(
        &self,
        f: impl FnOnce(&mut Show) -> Result<R, crate::api::ApiError>,
    ) -> Result<(R, Arc<Show>), crate::api::ApiError> {
        let _guard = self.inner.write_lock.lock().await;
        let mut show = (*self.get()).clone();
        let result = f(&mut show)?;
        // File paths in the show are never trusted (services::paths).
        crate::services::paths::sanitize_show(&mut show);
        show.version = show.version.wrapping_add(1).max(1);
        let show = Arc::new(show);
        persist(&self.inner.path, &show)
            .await
            .map_err(crate::api::ApiError::internal)?;
        *self.inner.show.write() = show.clone();
        let _ = self.inner.changed.send(show.version);
        self.inner
            .events
            .publish("show", &serde_json::json!({ "version": show.version }));
        Ok((result, show))
    }

    /// Replace the whole show (restore/import). Version continues to increase.
    pub async fn replace(&self, mut new_show: Show) -> anyhow::Result<Arc<Show>> {
        let current = self.version();
        new_show.version = current;
        let (_, show) = self
            .update(move |s| {
                *s = new_show;
                Ok(())
            })
            .await
            .map_err(|e| anyhow::anyhow!("{}", e.message))?;
        Ok(show)
    }
}

/// Write atomically: temp file + fsync + rename, keeping one `.bak`.
async fn persist(path: &Path, show: &Show) -> anyhow::Result<()> {
    let json = serde_json::to_vec_pretty(show)?;
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        use std::io::Write;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&json)?;
            f.sync_all()?;
        }
        if path.exists() {
            std::fs::copy(&path, path.with_extension("json.bak")).ok();
        }
        std::fs::rename(&tmp, &path)?;
        Ok(())
    })
    .await??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn update_bumps_version_and_persists() {
        let dir = tempdir();
        let path = dir.join("show.json");
        let store = ShowStore::load(&path, EventBus::new()).unwrap();
        let v0 = store.version();
        store
            .update(|s| {
                s.name = "Chandler Lights".into();
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(store.version(), v0 + 1);
        let reloaded = ShowStore::load(&path, EventBus::new()).unwrap();
        assert_eq!(reloaded.get().name, "Chandler Lights");
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn failed_update_changes_nothing() {
        let dir = tempdir();
        let store = ShowStore::load(&dir.join("show.json"), EventBus::new()).unwrap();
        let v0 = store.version();
        let r = store
            .update::<()>(|s| {
                s.name = "nope".into();
                Err(crate::api::ApiError::bad_request("no"))
            })
            .await;
        assert!(r.is_err());
        assert_eq!(store.version(), v0);
        assert_ne!(store.get().name, "nope");
        std::fs::remove_dir_all(dir).ok();
    }

    fn tempdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("pp-store-{}", pixelplus_core::model::new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}
