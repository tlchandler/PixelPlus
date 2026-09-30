//! The playback engine.
//!
//! Everything talks to the engine through a cloneable [`PlayerHandle`] that
//! sends [`PlayerCmd`]s to the engine task and watches its [`PlayerStatus`].
//! The engine (in `engine.rs`) owns the output backend, the frame clock, audio,
//! overlays and test patterns.

pub mod types;

pub use types::*;

use crate::api::{ApiError, ApiResult};
use pixelplus_core::model::EffectPreset;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, watch};

/// Leader → follower playback sync (ARCHITECTURE §7.4).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SyncPacket {
    pub leader: String,
    pub show_version: u64,
    pub state: PlayerState,
    /// What is playing: sequence id / effect id etc.
    #[serde(default)]
    pub item: Option<ItemRef>,
    /// Position in the current item (ms) at `sent_at_ms` (leader clock).
    pub pos_ms: u64,
    /// Leader monotonic clock (ms) when the packet was sent.
    pub sent_at_ms: u64,
    /// Effect being shown (already passed through `stamp_world_bounds`).
    #[serde(default)]
    pub effect: Option<EffectPreset>,
    #[serde(default)]
    pub test: Option<TestRequest>,
    pub brightness: u8,
    #[serde(default)]
    pub blackout: bool,
}

/// Command sent to the engine task.
#[derive(Debug)]
pub enum PlayerCmd {
    Play(PlayRequest, oneshot::Sender<ApiResult<()>>),
    Stop { fade: bool },
    Pause,
    Resume,
    Next,
    Previous,
    Seek(u64),
    SetVolume(u8),
    SetBrightness(u8),
    Blackout(bool),
    TestStart(TestRequest, oneshot::Sender<ApiResult<()>>),
    TestStop,
    /// Visitor song request: play `sequence_id` next.
    Enqueue { sequence_id: String, name: Option<String> },
    /// Follower: follow the leader's playback.
    Sync(SyncPacket),
    /// Overlay control for a prop (games, text, QR, fault finder).
    Overlay(OverlayCmd),
    /// Show changed (new version): rebuild maps, reload files.
    Reload,
}

#[derive(Debug)]
pub enum OverlayCmd {
    /// Open (or reuse) the shared-memory buffer for a matrix prop.
    Open { prop_id: String, reply: oneshot::Sender<ApiResult<OverlayInfo>> },
    /// Enable/disable compositing of the prop's overlay.
    Enable { prop_id: String, enabled: bool },
    /// Replace the overlay content with a full grid frame (row-major RGB, width×height×3).
    Frame { prop_id: String, rgb: bytes::Bytes },
    /// Replace the overlay content with pixels in prop order (pixelCount×3).
    PropPixels { prop_id: String, rgb: Vec<u8> },
    /// Scrolling text / QR helper overlays.
    Text { prop_id: String, text: String, color: String, scroll: bool, duration_ms: u64 },
    Qr { prop_id: String, url: String, duration_ms: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OverlayInfo {
    pub shm: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone)]
pub struct PlayerHandle {
    tx: mpsc::Sender<PlayerCmd>,
    status: watch::Receiver<PlayerStatus>,
}

impl PlayerHandle {
    pub fn new(tx: mpsc::Sender<PlayerCmd>, status: watch::Receiver<PlayerStatus>) -> Self {
        PlayerHandle { tx, status }
    }

    /// Latest status snapshot.
    pub fn status(&self) -> PlayerStatus {
        self.status.borrow().clone()
    }

    /// Watch status changes.
    pub fn watch(&self) -> watch::Receiver<PlayerStatus> {
        self.status.clone()
    }

    pub async fn send(&self, cmd: PlayerCmd) -> ApiResult<()> {
        self.tx
            .send(cmd)
            .await
            .map_err(|_| ApiError::unavailable("The player is not running."))
    }

    async fn request<T>(&self, f: impl FnOnce(oneshot::Sender<ApiResult<T>>) -> PlayerCmd) -> ApiResult<T> {
        let (tx, rx) = oneshot::channel();
        self.send(f(tx)).await?;
        rx.await
            .map_err(|_| ApiError::unavailable("The player stopped unexpectedly."))?
    }

    pub async fn play(&self, req: PlayRequest) -> ApiResult<()> {
        self.request(|tx| PlayerCmd::Play(req, tx)).await
    }
    pub async fn test_start(&self, req: TestRequest) -> ApiResult<()> {
        self.request(|tx| PlayerCmd::TestStart(req, tx)).await
    }
    pub async fn overlay_open(&self, prop_id: String) -> ApiResult<OverlayInfo> {
        self.request(|reply| PlayerCmd::Overlay(OverlayCmd::Open { prop_id, reply }))
            .await
    }
}
