//! Late-bound connections from the engine to other services, so the engine
//! does not depend on whether those services are compiled in or started.
//!
//! Wire them once at startup (see `services::start_all`):
//!
//! ```ignore
//! player::hooks::set_overlay_forwarder(move |prop, rgb| cluster.forward_overlay(prop, rgb));
//! player::hooks::set_sync_extras(move |effect, test| { cluster.set_sync_effect(effect); cluster.set_sync_test(test); });
//! player::hooks::set_dynamic_clip_renderer(move |clip_id, ctx| Box::pin(render(clip_id, ctx)));
//! ```

use super::types::TestRequest;
use futures::future::BoxFuture;
use std::sync::RwLock;
use pixelplus_core::model::EffectPreset;
use std::path::PathBuf;
use std::sync::Arc;

/// Live values for a dynamic DJ clip rendered at showtime.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DjContext {
    pub next_song: Option<String>,
    pub prev_song: Option<String>,
    pub request_name: Option<String>,
}

type OverlayForwarder = Arc<dyn Fn(&str, &[u8]) -> usize + Send + Sync>;
type SyncExtras = Arc<dyn Fn(Option<EffectPreset>, Option<TestRequest>) + Send + Sync>;
type DjRenderer = Arc<dyn Fn(String, DjContext) -> BoxFuture<'static, anyhow::Result<PathBuf>> + Send + Sync>;

static OVERLAY: RwLock<Option<OverlayForwarder>> = RwLock::new(None);
static EXTRAS: RwLock<Option<SyncExtras>> = RwLock::new(None);
static DJ: RwLock<Option<DjRenderer>> = RwLock::new(None);

/// Leader: send overlay pixels (prop order) of a prop that has segments on
/// other nodes to those nodes. Called from the output thread; must not block.
pub fn set_overlay_forwarder(f: impl Fn(&str, &[u8]) -> usize + Send + Sync + 'static) {
    *OVERLAY.write().unwrap_or_else(|p| p.into_inner()) = Some(Arc::new(f));
}

/// Leader: the look / test currently shown, for sync packets. Called on change.
pub fn set_sync_extras(f: impl Fn(Option<EffectPreset>, Option<TestRequest>) + Send + Sync + 'static) {
    *EXTRAS.write().unwrap_or_else(|p| p.into_inner()) = Some(Arc::new(f));
}

/// Render a dynamic DJ clip with live placeholder values; returns the audio file.
pub fn set_dynamic_clip_renderer(
    f: impl Fn(String, DjContext) -> BoxFuture<'static, anyhow::Result<PathBuf>> + Send + Sync + 'static,
) {
    *DJ.write().unwrap_or_else(|p| p.into_inner()) = Some(Arc::new(f));
}

pub(crate) fn forward_overlay(prop_id: &str, rgb: &[u8]) -> bool {
    let f = OVERLAY.read().unwrap_or_else(|p| p.into_inner()).clone();
    match f {
        Some(f) => {
            f(prop_id, rgb);
            true
        }
        None => false,
    }
}

pub(crate) fn sync_extras(effect: Option<EffectPreset>, test: Option<TestRequest>) {
    let f = EXTRAS.read().unwrap_or_else(|p| p.into_inner()).clone();
    if let Some(f) = f {
        f(effect, test);
    }
}

pub(crate) fn dynamic_clip_renderer() -> Option<DjRenderer> {
    DJ.read().unwrap_or_else(|p| p.into_inner()).clone()
}
