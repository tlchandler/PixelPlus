//! Public types of the playback engine, shared by the API, the scheduler,
//! the cluster layer and other services. (Contract: ARCHITECTURE §7.4, §8, §8.1.)

use pixelplus_core::mapcode::MapPlan;
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
    /// | "countdown" (F4) | "surprise" (F20)
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
    /// Power limiter state (F12), while the limiter is not off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<PowerStatus>,
    /// Engine-internal (not serialized): the lights timeline, stamped with
    /// the engine's own clock, for the leader's sync packets.
    #[serde(skip)]
    pub anchor: Option<super::Anchor>,
    /// Engine-internal: follower's smoothed error following the leader (ms).
    #[serde(skip)]
    pub sync_error_ms: Option<f64>,
    /// Engine-internal: refresh rate of the pixel output (Hz) when scanned
    /// out on a vblank grid (DPI).
    #[serde(skip)]
    pub refresh_hz: Option<f64>,
    /// Engine-internal (leader): the running surprise for sync packets, its
    /// `start_pos` relative to `anchor.at_ms` (F20).
    #[serde(skip)]
    pub surprise: Option<SurpriseAnchor>,
    /// Engine-internal (leader): the master brightness the lights actually
    /// use (`brightness` capped by late-night dimming, F12); sync packets
    /// carry this one.
    #[serde(skip)]
    pub light_brightness: Option<u8>,
}

/// Power limiter summary in [`PlayerStatus::power`] (F12).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct PowerStatus {
    /// Some output is being scaled down right now.
    pub limiting: bool,
    /// Lowest scale applied (1 = none).
    pub min_scale: f32,
}

/// What to play. Body of `POST /player/play`.
///
/// Manual play of a playlist (ARCHITECTURE §8 "Manual playback"): started
/// inside a schedule window it ends with that window (the entry's end
/// behaviour applies); outside windows it plays once (its `repeat` is
/// ignored) unless `loop_until_stopped` is set.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
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
    /// "Loop until I stop": repeat the playlist until stopped, and keep
    /// playing past the end of a schedule window.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub loop_until_stopped: bool,
}

/// Body of `POST /test/start`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct TestRequest {
    /// "solid" | "chase" | "rgbCycle" | "countPixels" | "walk" | "effect"
    /// | "mapCode" (F6/F7, needs `map` or `mapRunId`) | "identify" (F9, needs
    /// `identify`) | "calibration" (F1 v2 pattern, needs `cal`)
    pub mode: String,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub speed: Option<f32>,
    #[serde(default)]
    pub target: TestTarget,
    #[serde(default)]
    pub effect: Option<EffectPreset>,
    /// `mapCode`: the full plan (sent once by `/cluster/command`; too large
    /// for sync packets).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<MapPlan>,
    /// `mapCode`: the run whose plan a follower already has (sync packets
    /// carry this instead of `map`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_run_id: Option<String>,
    /// `identify`: lights to show (receiver wizard, F9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identify: Option<Vec<IdentifyLight>>,
    /// `calibration`: the v2 pattern (F1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cal: Option<CalPattern>,
}

/// One identification light (F9): port 1 of a free jack blinks `color`
/// `blinks` times per cycle. Colours are post-colour-order.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IdentifyLight {
    pub node_id: String,
    /// 1-based output.
    pub output: u32,
    /// "#rrggbb"
    pub color: String,
    pub blinks: u8,
}

/// Calibration pattern parameters (F1): followers render
/// `calpattern::flash_on_v2(pos, seed)`. `v: 1` = the classic click each second.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CalPattern {
    pub seed: u32,
    #[serde(default = "cal_v2")]
    pub v: u8,
}

fn cal_v2() -> u8 {
    2
}

/// A surprise layered over the playing item (F20), carried in sync packets
/// while active so followers render the same layer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SurpriseAnchor {
    pub id: String,
    /// "sequence" | "effect"
    pub kind: String,
    /// Sequence or effect id.
    pub r#ref: String,
    /// Prop ids it draws on.
    #[serde(default)]
    pub targets: Vec<String>,
    /// Leader timeline position (ms) where it starts.
    pub start_pos: f64,
    pub duration_ms: u64,
    pub epoch: u64,
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
