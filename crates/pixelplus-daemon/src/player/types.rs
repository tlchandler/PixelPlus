//! Public types of the playback engine, shared by the API, the scheduler,
//! the cluster layer and other services. (Contract: ARCHITECTURE §7.4, §8, §8.1.)

use pixelplus_core::model::{EffectPreset, Target};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PlayerState {
    #[default]
    Idle,
    Playing,
    Paused,
    Testing,
    Effect,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ItemRef {
    /// "sequence" | "dj" | "effect" | "media" | "pause" | "command" | "request"
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistRef {
    pub id: String,
    pub name: String,
    /// 0-based index of the current item.
    pub index: u32,
    pub count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleRef {
    pub id: String,
    pub name: String,
    /// RFC 3339.
    pub ends_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NextShowRef {
    pub name: String,
    /// RFC 3339.
    pub starts_at: String,
}

/// Sent on the WebSocket as `status` and returned by `GET /player`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlayerStatus {
    pub state: PlayerState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playlist: Option<PlaylistRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<ItemRef>,
    pub pos_ms: u64,
    pub duration_ms: u64,
    pub volume: u8,
    pub brightness: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_item: Option<ItemRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule_entry: Option<ScheduleRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_show: Option<NextShowRef>,
    /// Frames per second actually output over the last second.
    pub fps: f32,
    /// True when the lights are forced dark (blackout).
    #[serde(default)]
    pub blackout: bool,
    /// Pending visitor song requests.
    #[serde(default)]
    pub request_queue: u32,
    /// Human readable problem that stopped playback, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What to play. Body of `POST /player/play`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlayRequest {
    #[serde(default)]
    pub playlist_id: Option<String>,
    #[serde(default)]
    pub sequence_id: Option<String>,
    #[serde(default)]
    pub dj_clip_id: Option<String>,
    #[serde(default)]
    pub effect_id: Option<String>,
    #[serde(default)]
    pub media_id: Option<String>,
    /// Start at this playlist item index.
    #[serde(default)]
    pub start_index: Option<u32>,
}

/// Body of `POST /test/start`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TestRequest {
    /// "solid" | "chase" | "rgbCycle" | "countPixels" | "walk" | "effect"
    pub mode: String,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub speed: Option<f32>,
    #[serde(default)]
    pub target: TestTarget,
    #[serde(default)]
    pub effect: Option<EffectPreset>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct TestTarget {
    /// Raw output test: every pixel of this node output (1-based).
    #[serde(default)]
    pub node_id: Option<String>,
    #[serde(default)]
    pub output: Option<u32>,
    #[serde(flatten)]
    pub props: Target,
}
