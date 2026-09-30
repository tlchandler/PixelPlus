//! The PixelPlus show model.
//!
//! This is the single source of truth for everything a user configures. It is
//! serialized as camelCase JSON (`show.json`) and exchanged verbatim with the
//! web UI. See `docs/ARCHITECTURE.md` §4 for the contract.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Generate a short random id (10 chars, `[a-z0-9]`).
pub fn new_id() -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..10)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}

// ---------------------------------------------------------------------------
// Show
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Show {
    /// Monotonically increasing revision, bumped on every change.
    pub version: u64,
    pub name: String,
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub receivers: Vec<Receiver>,
    #[serde(default)]
    pub props: Vec<Prop>,
    #[serde(default)]
    pub prop_groups: Vec<PropGroup>,
    #[serde(default)]
    pub sequences: Vec<Sequence>,
    #[serde(default)]
    pub media: Vec<Media>,
    #[serde(default)]
    pub dj_clips: Vec<DjClip>,
    #[serde(default)]
    pub dj_voices: Vec<DjVoice>,
    #[serde(default)]
    pub pronunciations: Vec<Pronunciation>,
    #[serde(default)]
    pub effects: Vec<EffectPreset>,
    #[serde(default)]
    pub playlists: Vec<Playlist>,
    #[serde(default)]
    pub schedule: Schedule,
    #[serde(default)]
    pub settings: ShowSettings,
    /// Show file format (F15). Bumped only by a migration older versions
    /// cannot read; an update rollback restores the pre-update snapshot then.
    #[serde(default = "default_format_version")]
    pub format_version: u32,
    /// Season profiles (F8): named copies of the season-specific settings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<ShowProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile_id: Option<String>,
    /// Switch profiles by their date ranges (daily at 12:00 and at boot).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub profile_auto_switch: bool,
    /// Power supplies feeding receivers / direct outputs (F12).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub power_supplies: Vec<PowerSupply>,
    /// Tag colours / descriptions for the library (F18). Tags themselves live
    /// on sequences and media.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tag_defs: Vec<TagDef>,
    /// ESP32 sensor nodes (F20).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sensor_nodes: Vec<SensorNode>,
}

/// Current [`Show::format_version`].
pub const SHOW_FORMAT_VERSION: u32 = 1;

fn default_format_version() -> u32 {
    SHOW_FORMAT_VERSION
}

fn yes() -> bool {
    true
}

impl Default for Show {
    fn default() -> Self {
        Show {
            version: 1,
            name: "My Show".into(),
            nodes: vec![],
            receivers: vec![],
            props: vec![],
            prop_groups: vec![],
            sequences: vec![],
            media: vec![],
            dj_clips: vec![],
            dj_voices: vec![],
            pronunciations: vec![],
            effects: vec![],
            playlists: vec![],
            schedule: Schedule::default(),
            settings: ShowSettings::default(),
            format_version: SHOW_FORMAT_VERSION,
            profiles: vec![],
            active_profile_id: None,
            profile_auto_switch: false,
            power_supplies: vec![],
            tag_defs: vec![],
            sensor_nodes: vec![],
        }
    }
}

impl Show {
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }
    pub fn prop(&self, id: &str) -> Option<&Prop> {
        self.props.iter().find(|p| p.id == id)
    }
    pub fn sequence(&self, id: &str) -> Option<&Sequence> {
        self.sequences.iter().find(|s| s.id == id)
    }
    pub fn media_item(&self, id: &str) -> Option<&Media> {
        self.media.iter().find(|m| m.id == id)
    }
    pub fn playlist(&self, id: &str) -> Option<&Playlist> {
        self.playlists.iter().find(|p| p.id == id)
    }
    pub fn effect(&self, id: &str) -> Option<&EffectPreset> {
        self.effects.iter().find(|e| e.id == id)
    }
    pub fn dj_clip(&self, id: &str) -> Option<&DjClip> {
        self.dj_clips.iter().find(|d| d.id == id)
    }
    pub fn leader(&self) -> Option<&Node> {
        self.nodes.iter().find(|n| n.role == NodeRole::Leader)
    }
}

// ---------------------------------------------------------------------------
// Hardware
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum BoardKind {
    Difftx,
    Difftxlarge,
    Diffsmart,
    BarePi,
    Virtual,
}

impl BoardKind {
    pub const ALL: [BoardKind; 5] = [
        BoardKind::Difftx,
        BoardKind::Difftxlarge,
        BoardKind::Diffsmart,
        BoardKind::BarePi,
        BoardKind::Virtual,
    ];

    pub fn output_count(self) -> usize {
        match self {
            BoardKind::Difftx | BoardKind::Diffsmart => 4,
            BoardKind::Difftxlarge => 60,
            BoardKind::BarePi | BoardKind::Virtual => 0,
        }
    }

    /// Number of RJ45 jacks (4 outputs each). diffsmart has direct terminals (0 jacks).
    pub fn jack_count(self) -> usize {
        match self {
            BoardKind::Difftx => 1,
            BoardKind::Difftxlarge => 15,
            _ => 0,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            BoardKind::Difftx => "PixelPlus pHAT (difftx)",
            BoardKind::Difftxlarge => "60-Port Transmitter (difftxlarge)",
            BoardKind::Diffsmart => "Smart Receiver, standalone (diffsmart)",
            BoardKind::BarePi => "Raspberry Pi (no board)",
            BoardKind::Virtual => "Virtual (Docker / PC)",
        }
    }

    /// Human label for 1-based output `index`.
    pub fn output_label(self, index: usize) -> String {
        match self {
            BoardKind::Difftxlarge => {
                let k = index - 1;
                format!("J{}-{}", k / 4 + 1, k % 4 + 1)
            }
            BoardKind::Diffsmart => format!("Out {index}"),
            _ => format!("Port {index}"),
        }
    }

    pub fn default_outputs(self) -> Vec<OutputConfig> {
        (1..=self.output_count())
            .map(|i| OutputConfig {
                index: i as u32,
                label: self.output_label(i),
                ..OutputConfig::default()
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum NodeRole {
    Leader,
    Follower,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub id: String,
    pub name: String,
    pub hostname: String,
    pub role: NodeRole,
    pub board: BoardKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_rev: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pi_model: Option<String>,
    #[serde(default)]
    pub outputs: Vec<OutputConfig>,
    #[serde(default)]
    pub adopted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// EEPROM serial of the current hardware (F10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    /// Earlier hardware of this controller (replacements, F10).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hardware_history: Vec<HardwareRecord>,
}

/// One piece of hardware a controller ran on (F10 "Replace with…").
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HardwareRecord {
    /// RFC 3339.
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    pub board: BoardKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pi_model: Option<String>,
    /// e.g. "replaced", "adopted".
    pub reason: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "lowercase")]
pub enum PixelType {
    /// WS281x family at 800 kHz (WS2811, WS2812B, WS2815).
    #[default]
    Ws2811,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub enum ColorOrder {
    #[default]
    RGB,
    RBG,
    GRB,
    GBR,
    BRG,
    BGR,
}

impl ColorOrder {
    /// For each output byte position, the source RGB index (0=R,1=G,2=B).
    pub fn source_indices(self) -> [usize; 3] {
        match self {
            ColorOrder::RGB => [0, 1, 2],
            ColorOrder::RBG => [0, 2, 1],
            ColorOrder::GRB => [1, 0, 2],
            ColorOrder::GBR => [1, 2, 0],
            ColorOrder::BRG => [2, 0, 1],
            ColorOrder::BGR => [2, 1, 0],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OutputConfig {
    /// 1-based.
    pub index: u32,
    pub label: String,
    #[serde(default)]
    pub pixel_type: PixelType,
    #[serde(default)]
    pub color_order: ColorOrder,
    /// 0..=100 %.
    pub brightness: u8,
    pub gamma: f32,
    pub enabled: bool,
    /// Pixels found on this output by a pixel-count check (F7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measured_pixels: Option<MeasuredCount>,
}

/// Result of a pixel-count check (F7).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MeasuredCount {
    pub count: u32,
    /// "camera" | "manual" | "current"
    pub method: String,
    /// RFC 3339.
    pub at: String,
    /// Output pixel indices that did not respond.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dead: Vec<u32>,
}

impl Default for OutputConfig {
    fn default() -> Self {
        OutputConfig {
            index: 1,
            label: String::new(),
            pixel_type: PixelType::Ws2811,
            color_order: ColorOrder::RGB,
            brightness: 100,
            gamma: 1.0,
            enabled: true,
            measured_pixels: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum ReceiverKind {
    Diffrx,
    DiffsmartRx,
    Generic4,
    Direct,
}

impl ReceiverKind {
    pub fn port_count(self) -> usize {
        match self {
            ReceiverKind::Direct => 1,
            _ => 4,
        }
    }
    pub fn display_name(self) -> &'static str {
        match self {
            ReceiverKind::Diffrx => "Chandler 4D/8P Differential Receiver",
            ReceiverKind::DiffsmartRx => "Chandler Smart Receiver (RX mode)",
            ReceiverKind::Generic4 => "Generic 4-port differential receiver",
            ReceiverKind::Direct => "Direct connection",
        }
    }
    /// Default per-port fuse rating (A) used for power warnings.
    pub fn default_fuse_amps(self) -> Option<f32> {
        match self {
            // Bourns MF-R600: 6 A hold at 23 °C (derates to ~4.1 A at 60 °C).
            ReceiverKind::Diffrx | ReceiverKind::DiffsmartRx => Some(6.0),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Receiver {
    pub id: String,
    pub name: String,
    pub kind: ReceiverKind,
    pub node_id: String,
    /// 1-based transmitter jack.
    pub jack: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fuse_amps: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Main fuse / bus rating (A), e.g. 30 on a diffrx rev C (F12).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_fuse_amps: Option<f32>,
}

impl Receiver {
    /// Transmitter output (1-based) that receiver port `port` (1-based) is fed from.
    pub fn output_for_port(&self, port: u32) -> u32 {
        (self.jack - 1) * 4 + port
    }
}

// ---------------------------------------------------------------------------
// Props
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "lowercase")]
pub enum PropKind {
    Arch,
    Candycane,
    Tree,
    Matrix,
    Line,
    Circle,
    Star,
    Spinner,
    Window,
    Icicles,
    Custom,
    #[default]
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Prop {
    pub id: String,
    pub name: String,
    pub kind: PropKind,
    pub pixel_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xlights_model: Option<String>,
    /// 0-based byte offset into the fseq frame. Internal; never shown as a "channel".
    /// With [`Prop::channel_runs`] it is the lowest byte offset of any run.
    pub channel_start: u32,
    #[serde(default = "default_cpp")]
    pub channels_per_pixel: u8,
    /// Non-contiguous channel layout (xLights "individual start channels"): pixel `i`
    /// reads from the run containing it. Absent → contiguous from `channel_start`.
    /// Pixels not covered by any run have no channel data (they stay dark).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_runs: Option<Vec<ChannelRun>>,
    #[serde(default)]
    pub segments: Vec<PropSegment>,
    #[serde(default)]
    pub group_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<PropLayout>,
    /// Present for matrix-like props: maps a width×height grid onto prop pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix: Option<MatrixInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_milliamps_per_pixel: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Prop pixel indices flagged as dead / suspect (F6, F7, fault finder).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suspect_pixels: Vec<u32>,
}

fn default_cpp() -> u8 {
    3
}

impl Prop {
    pub const DEFAULT_MA_PER_PIXEL: f32 = 60.0;

    pub fn ma_per_pixel(&self) -> f32 {
        self.max_milliamps_per_pixel
            .unwrap_or(Self::DEFAULT_MA_PER_PIXEL)
    }

    /// Byte length of this prop's data inside an fseq frame (`pixelCount ×
    /// channelsPerPixel`; with [`Prop::channel_runs`] the bytes are spread over runs).
    pub fn channel_len(&self) -> u32 {
        self.pixel_count
            .saturating_mul(self.channels_per_pixel as u32)
    }

    /// Where this prop's pixels live in the fseq frame: its [`ChannelRun`]s (clipped to
    /// `pixelCount`, empty runs dropped), or one run covering the whole prop from
    /// `channelStart` when it has none. Never allocates.
    pub fn channel_ranges(&self) -> ChannelRanges<'_> {
        match self.channel_runs.as_deref() {
            Some(runs) => ChannelRanges {
                single: None,
                runs: runs.iter(),
                pixel_count: self.pixel_count,
            },
            None => ChannelRanges {
                single: Some(ChannelRun {
                    prop_offset: 0,
                    channel_start: self.channel_start,
                    pixel_count: self.pixel_count,
                }),
                runs: [].iter(),
                pixel_count: self.pixel_count,
            },
        }
    }

    /// The parts of prop pixels `first .. first + count` that have channel data, as runs
    /// (`prop_offset` = first prop pixel of the piece, `channel_start` = its byte offset).
    pub fn channel_pieces(&self, first: u32, count: u32) -> impl Iterator<Item = ChannelRun> + '_ {
        let cpp = self.channels_per_pixel as u32;
        let end = first.saturating_add(count);
        self.channel_ranges().filter_map(move |r| {
            let a = r.prop_offset.max(first);
            let b = r.prop_offset.saturating_add(r.pixel_count).min(end);
            (a < b).then(|| ChannelRun {
                prop_offset: a,
                channel_start: r
                    .channel_start
                    .saturating_add((a - r.prop_offset).saturating_mul(cpp)),
                pixel_count: b - a,
            })
        })
    }

    /// Byte offset of pixel `i` in the fseq frame; `None` if the prop has no channel
    /// data for it.
    pub fn channel_of_pixel(&self, i: u32) -> Option<u32> {
        self.channel_pieces(i, 1).next().map(|r| r.channel_start)
    }

    /// One past the highest byte this prop reads from an fseq frame (0 for an empty prop).
    pub fn channel_end(&self) -> u64 {
        let cpp = self.channels_per_pixel as u64;
        self.channel_ranges()
            .map(|r| r.channel_start as u64 + r.pixel_count as u64 * cpp)
            .max()
            .unwrap_or(0)
    }

    /// Pixels not covered by any segment (unwired).
    pub fn unwired_pixels(&self) -> u32 {
        let wired: u32 = self.segments.iter().map(|s| s.pixel_count).sum();
        self.pixel_count.saturating_sub(wired)
    }
}

/// A run of consecutive prop pixels whose channels are contiguous in the fseq frame
/// (one xLights string with its own start channel).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChannelRun {
    /// 0-based index of the first prop pixel in the run.
    pub prop_offset: u32,
    /// 0-based byte offset of that pixel in the fseq frame.
    pub channel_start: u32,
    /// Number of pixels in the run.
    pub pixel_count: u32,
}

/// Iterator returned by [`Prop::channel_ranges`].
#[derive(Debug, Clone)]
pub struct ChannelRanges<'a> {
    single: Option<ChannelRun>,
    runs: std::slice::Iter<'a, ChannelRun>,
    pixel_count: u32,
}

impl Iterator for ChannelRanges<'_> {
    type Item = ChannelRun;
    fn next(&mut self) -> Option<ChannelRun> {
        if let Some(r) = self.single.take() {
            return (r.pixel_count > 0).then_some(r);
        }
        for r in self.runs.by_ref() {
            let avail = self.pixel_count.saturating_sub(r.prop_offset);
            let n = r.pixel_count.min(avail);
            if n > 0 {
                return Some(ChannelRun {
                    pixel_count: n,
                    ..*r
                });
            }
        }
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PropSegment {
    pub node_id: String,
    /// 1-based node output index.
    pub output: u32,
    /// 0-based pixel position on that output (null pixels already counted).
    pub start_pixel: u32,
    pub pixel_count: u32,
    /// 0-based index into the prop's pixels where this segment starts.
    pub prop_offset: u32,
    #[serde(default)]
    pub reverse: bool,
    #[serde(default)]
    pub null_pixels: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PropLayout {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    #[serde(default)]
    pub rotation: f32,
    /// Normalized (0..1) per-pixel positions inside the w×h box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub points: Option<Vec<[f32; 2]>>,
    /// Where the layout came from (absent = unknown / older show).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<LayoutSource>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum LayoutSource {
    Xlights,
    Manual,
    /// Camera mapping (F6).
    Camera,
}

/// Grid geometry of a matrix prop, used by overlays (games, text, QR codes).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MatrixInfo {
    pub width: u32,
    pub height: u32,
    /// Row-major from the top-left, `width*height` entries: prop pixel index, or -1 if no pixel.
    pub pixel_map: Vec<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PropGroup {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub prop_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

// ---------------------------------------------------------------------------
// Content
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Sequence {
    pub id: String,
    pub name: String,
    pub file: String,
    pub duration_ms: u64,
    pub frame_ms: u32,
    pub channel_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xlights_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    /// sha256 of the fseq file (hex).
    #[serde(default)]
    pub hash: String,
    /// Made by PixelPlus (auto light show, "speak with lights"), F2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated: Option<GeneratedInfo>,
    /// Library tags (F18), e.g. "kids", "season:halloween".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// How a generated sequence was made (F2); regenerated when `props_hash` changes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedInfo {
    pub kind: GeneratedKind,
    pub media_id: String,
    /// Style id (`GET /autoshow/styles`).
    pub style: String,
    /// Props used; empty = all.
    #[serde(default)]
    pub prop_ids: Vec<String>,
    pub seed: u32,
    pub analysis_version: u32,
    pub props_hash: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum GeneratedKind {
    AutoShow,
    Voice,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Song,
    Dj,
    Sfx,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Media {
    pub id: String,
    pub name: String,
    pub kind: MediaKind,
    pub file: String,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loudness_lufs: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain_db: Option<f32>,
    /// Beat / tempo / energy summary (F2); the full analysis is
    /// `media/<id>.analysis.json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analysis: Option<AudioAnalysisSummary>,
    /// Library tags (F18).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// File name as uploaded (xLights FPP Connect compares it, F16).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_name: Option<String>,
    /// Size in bytes of the original upload (F16 `…/meta`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_size: Option<u64>,
}

/// Summary of a song's audio analysis (F2).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AudioAnalysisSummary {
    pub version: u32,
    pub bpm: f32,
    pub bpm_confidence: f32,
    pub beat_count: u32,
    pub first_beat_ms: u32,
    /// 0..1 mean normalized energy.
    pub energy: f32,
    /// Number of detected sections.
    #[serde(default)]
    pub sections: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DjLine {
    pub voice: String,
    pub text: String,
    #[serde(default)]
    pub pause_ms: u32,
    /// 0 = calm, 0.4 = normal DJ (default), 1 = hype, 1.5 = extra hype.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub energy: Option<f32>,
}

/// A DJ voice: a named blend of Kokoro base voices plus delivery tuning.
/// Mirrors the `voices.json` format from tlchandler/fpp-voices.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DjVoice {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Kokoro base voice id -> weight (weights are normalized).
    pub blend: BTreeMap<String, f32>,
    #[serde(default = "one")]
    pub speed: f32,
    #[serde(default = "default_lang")]
    pub lang: String,
    /// Optional ffmpeg EQ filter chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eq: Option<String>,
    #[serde(default = "default_energy")]
    pub default_energy: f32,
    /// Energy tuning (pitch, range, speed, stretch, boost, lift, ceiling, maxLift).
    #[serde(default)]
    pub energy: BTreeMap<String, f32>,
}

fn default_lang() -> String {
    "en-us".into()
}

fn default_energy() -> f32 {
    0.4
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DjClip {
    pub id: String,
    pub name: String,
    pub lines: Vec<DjLine>,
    #[serde(default)]
    pub dynamic: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_id: Option<String>,
    #[serde(default = "one")]
    pub speed: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub music_bed_media_id: Option<String>,
}

fn one() -> f32 {
    1.0
}

/// Pronunciation fix: whole-word `word` is spoken as `say`
/// (sound-alike spelling, or IPA between slashes, e.g. `/noʊˈɛl/`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Pronunciation {
    pub word: String,
    pub say: String,
}

pub const DJ_PLACEHOLDERS: &[&str] = &[
    "time",
    "date",
    "day",
    "daysUntilChristmas",
    "nextSong",
    "prevSong",
    "showName",
    "temperature",
    "sunset",
    "requestName",
];

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum EffectKind {
    Solid,
    Chase,
    Twinkle,
    Rainbow,
    Colorwash,
    Candycane,
    Fire,
    Snow,
    Sparkle,
    Wave,
    Meteor,
    Strobe,
    Breathe,
    /// Show-start countdown (F4): params `durationMs, text, color, others,
    /// finale, matrixPropId`.
    Countdown,
}

pub type EffectParams = BTreeMap<String, serde_json::Value>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    #[serde(default)]
    pub all: bool,
    #[serde(default)]
    pub prop_ids: Vec<String>,
    #[serde(default)]
    pub group_ids: Vec<String>,
}

impl Target {
    /// Resolve to prop ids (deduplicated, show order).
    pub fn resolve<'a>(&self, show: &'a Show) -> Vec<&'a Prop> {
        show.props
            .iter()
            .filter(|p| {
                self.all
                    || self.prop_ids.contains(&p.id)
                    || p.group_ids.iter().any(|g| self.group_ids.contains(g))
                    || show
                        .prop_groups
                        .iter()
                        .any(|g| self.group_ids.contains(&g.id) && g.prop_ids.contains(&p.id))
            })
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EffectPreset {
    pub id: String,
    pub name: String,
    pub effect: EffectKind,
    #[serde(default)]
    pub params: EffectParams,
    #[serde(default)]
    pub target: Target,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum PlaylistItem {
    #[serde(rename_all = "camelCase")]
    Sequence { id: String, sequence_id: String },
    #[serde(rename_all = "camelCase")]
    Dj { id: String, dj_clip_id: String },
    #[serde(rename_all = "camelCase")]
    Effect {
        id: String,
        effect_id: String,
        duration_ms: u64,
    },
    #[serde(rename_all = "camelCase")]
    Media { id: String, media_id: String },
    #[serde(rename_all = "camelCase")]
    Pause { id: String, duration_ms: u64 },
    /// Run a command, e.g. "games.invite", "games.stop", "overlay.text".
    #[serde(rename_all = "camelCase")]
    Command {
        id: String,
        command: String,
        #[serde(default)]
        args: serde_json::Value,
    },
    /// Show-start countdown (F4), usually the last intro item.
    #[serde(rename_all = "camelCase")]
    Countdown {
        id: String,
        duration_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        matrix_prop_id: Option<String>,
        /// `{s}` seconds left, `{mm}`/`{ss}`, or custom text around them.
        #[serde(default = "default_countdown_text")]
        text: String,
        /// "#rrggbb"
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<String>,
        #[serde(default)]
        others: CountdownOthers,
        #[serde(default)]
        finale: CountdownFinale,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dj_clip_id: Option<String>,
        /// 0 = the clip ends at zero; positive = later.
        #[serde(default)]
        dj_offset_ms: i32,
        /// Tick sound each second (when there is no DJ clip).
        #[serde(default)]
        tick: bool,
    },
}

fn default_countdown_text() -> String {
    "{s}".into()
}

/// What props other than the countdown matrix do (F4).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum CountdownOthers {
    #[default]
    Fill,
    Pulse,
    Dark,
}

/// What happens at zero (F4).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum CountdownFinale {
    #[default]
    Flash,
    None,
}

impl PlaylistItem {
    pub fn id(&self) -> &str {
        match self {
            PlaylistItem::Sequence { id, .. }
            | PlaylistItem::Dj { id, .. }
            | PlaylistItem::Effect { id, .. }
            | PlaylistItem::Media { id, .. }
            | PlaylistItem::Pause { id, .. }
            | PlaylistItem::Command { id, .. }
            | PlaylistItem::Countdown { id, .. } => id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub items: Vec<PlaylistItem>,
    #[serde(default)]
    pub intro: Vec<PlaylistItem>,
    #[serde(default)]
    pub outro: Vec<PlaylistItem>,
    #[serde(default)]
    pub shuffle: bool,
    #[serde(default)]
    pub repeat: bool,
    #[serde(default)]
    pub crossfade_ms: u32,
    /// Smart playlist rules (F18): `items` is then generated at play time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smart: Option<SmartRules>,
}

/// Rules of a smart playlist (F18, `core::smartlist::expand`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SmartRules {
    #[serde(default)]
    pub include_tags: Vec<String>,
    #[serde(default)]
    pub include_mode: TagMatch,
    #[serde(default)]
    pub exclude_tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_item_ms: Option<u64>,
    #[serde(default)]
    pub no_repeat_nights: u32,
    #[serde(default)]
    pub time_rules: Vec<SmartTimeRule>,
    #[serde(default)]
    pub order: SmartOrder,
    #[serde(default)]
    pub pinned_first: Vec<PlaylistItem>,
    #[serde(default)]
    pub pinned_last: Vec<PlaylistItem>,
    /// Inserted every `interleave_every` songs (e.g. a DJ clip).
    #[serde(default)]
    pub interleave: Vec<PlaylistItem>,
    #[serde(default)]
    pub interleave_every: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum TagMatch {
    #[default]
    Any,
    All,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum SmartOrder {
    #[default]
    LeastRecent,
    Shuffle,
    Rotation,
    Fixed,
}

/// Before `before`, only songs carrying all of `require_tags` are placed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SmartTimeRule {
    pub before: TimeSpec,
    #[serde(default)]
    pub require_tags: Vec<String>,
}

/// Colour / description of a library tag (F18).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TagDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

// ---------------------------------------------------------------------------
// Schedule
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub lat: f64,
    pub lon: f64,
    pub timezone: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl Default for Location {
    fn default() -> Self {
        Location {
            lat: 39.8283,
            lon: -98.5795,
            timezone: "America/Chicago".into(),
            label: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum TimeSpec {
    /// "HH:MM" local time.
    Clock { time: String },
    #[serde(rename_all = "camelCase")]
    Sunset { offset_min: i32 },
    #[serde(rename_all = "camelCase")]
    Sunrise { offset_min: i32 },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Weekday {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DateRange {
    /// "MM-DD"
    pub start: String,
    /// "MM-DD" (may be before start: wraps the year end)
    pub end: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum EndBehavior {
    #[default]
    FinishSong,
    StopNow,
    FadeOut,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleEntry {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub playlist_id: String,
    pub days: Vec<Weekday>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_range: Option<DateRange>,
    pub start: TimeSpec,
    pub end: TimeSpec,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub end_behavior: EndBehavior,
    /// Start the playlist's intro early so its first song begins exactly at
    /// `start` (F4 countdown).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub start_exact: bool,
}

/// A daily time window `from`..`to` (wraps midnight when `to` < `from`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TimeWindow {
    pub from: TimeSpec,
    pub to: TimeSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VolumeCurfew {
    pub time: TimeSpec,
    pub volume: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub location: Location,
    #[serde(default)]
    pub entries: Vec<ScheduleEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_effect_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub off_effect_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_curfew: Option<VolumeCurfew>,
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ShowSettings {
    #[serde(default)]
    pub audio: AudioSettings,
    #[serde(default)]
    pub alerts: AlertSettings,
    #[serde(default)]
    pub mqtt: MqttSettings,
    #[serde(default)]
    pub requests: RequestSettings,
    #[serde(default)]
    pub tts: TtsSettings,
    #[serde(default)]
    pub oled: OledSettings,
    #[serde(default)]
    pub security: SecuritySettings,
    #[serde(default)]
    pub triggers: Vec<Trigger>,
    #[serde(default)]
    pub games: GameSettings,
    /// Display units for the UI. `None` = pick from the viewer's locale (US → °F).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub units: Option<UnitSettings>,
    #[serde(default)]
    pub output: OutputSettings,
    /// HTTPS listener preferences (F1); key material stays in `tls/`.
    #[serde(default)]
    pub https: HttpsSettings,
    /// Nightly health report (F11).
    #[serde(default)]
    pub reports: ReportSettings,
    /// Power limiter and late-night dimming (F12).
    #[serde(default)]
    pub power: PowerSettings,
    /// Remote access: public listener, Tailscale, Cloudflare (F14).
    #[serde(default)]
    pub remote: RemoteSettings,
    /// Software update channel and automation (F15).
    #[serde(default)]
    pub updates: UpdateSettings,
    /// xLights FPP Connect upload (F16).
    #[serde(default)]
    pub xlights: XlightsSettings,
}

// --- F1 HTTPS -------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HttpsSettings {
    /// Listen on :443 (`PIXELPLUS_HTTPS_PORT`) with the local CA's certificate.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Extra host names for the certificate (besides hostname variants and IPs).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_names: Vec<String>,
}

impl Default for HttpsSettings {
    fn default() -> Self {
        HttpsSettings {
            enabled: true,
            extra_names: vec![],
        }
    }
}

/// Last "Measure with my phone" result (F1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AudioCalibration {
    /// RFC 3339.
    pub measured_at: String,
    pub method: CalibrationMethod,
    pub residual_ms: f32,
    pub spread_ms: f32,
    pub matches: u32,
    pub applied_delay_ms: i32,
    /// Phone model (user agent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CalibrationMethod {
    Phone,
    Manual,
}

// --- F11 reports ----------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReportSettings {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// "HH:MM" local time the report for the past show night is made.
    #[serde(default = "default_report_time")]
    pub time: String,
    /// Send by email (uses `alerts.email`).
    #[serde(default = "yes")]
    pub email: bool,
    /// Send by push (uses `alerts.ntfy`).
    #[serde(default = "yes")]
    pub push: bool,
    #[serde(default)]
    pub only_when_problems: bool,
    #[serde(default = "default_report_keep")]
    pub keep_days: u32,
}

fn default_report_time() -> String {
    "07:00".into()
}

fn default_report_keep() -> u32 {
    90
}

impl Default for ReportSettings {
    fn default() -> Self {
        ReportSettings {
            enabled: true,
            time: default_report_time(),
            email: true,
            push: true,
            only_when_problems: false,
            keep_days: default_report_keep(),
        }
    }
}

// --- F12 power ------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum LimiterMode {
    Off,
    /// Compute and report, never scale.
    #[default]
    Warn,
    Limit,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PowerSettings {
    #[serde(default)]
    pub mode: LimiterMode,
    /// Fraction of each budget actually used (0..1).
    #[serde(default = "default_safety")]
    pub safety: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_amps: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_watts: Option<f32>,
    /// Late-night dimming: master-brightness ceilings by time of day.
    #[serde(default)]
    pub dim: Vec<DimWindow>,
    /// Master-brightness ceiling, percent.
    #[serde(default = "hundred")]
    pub max_brightness: u8,
}

fn default_safety() -> f32 {
    0.9
}

fn hundred() -> u8 {
    100
}

impl Default for PowerSettings {
    fn default() -> Self {
        PowerSettings {
            mode: LimiterMode::Warn,
            safety: default_safety(),
            global_amps: None,
            global_watts: None,
            dim: vec![],
            max_brightness: 100,
        }
    }
}

/// Brightness ceiling (percent) between `from` and `to`; `days` empty = every day.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DimWindow {
    pub from: TimeSpec,
    pub to: TimeSpec,
    pub brightness: u8,
    #[serde(default)]
    pub days: Vec<Weekday>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PowerSupply {
    pub id: String,
    pub name: String,
    pub volts: f32,
    pub amps: f32,
    #[serde(default)]
    pub receiver_ids: Vec<String>,
    /// Outputs fed without a receiver (diffsmart terminals).
    #[serde(default)]
    pub direct_outputs: Vec<NodeOutputRef>,
    /// Measured current from a sensor node input (F20).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensor: Option<SensorRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct NodeOutputRef {
    pub node_id: String,
    /// 1-based.
    pub output: u32,
}

/// The per-node limiter budget the leader sends in the manifest (F12,
/// computed by `power::node_budget`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct NodePowerBudget {
    pub mode: LimiterMode,
    pub safety: f32,
    #[serde(default)]
    pub groups: Vec<PowerGroup>,
    /// Estimated mA per pixel at full white, per output (1-based index → mA).
    #[serde(default, rename = "mApp")]
    pub ma_pp: BTreeMap<u32, f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PowerGroup {
    pub id: String,
    pub kind: PowerGroupKind,
    #[serde(rename = "budgetA")]
    pub budget_a: f32,
    /// Averaging time constant (0 = instantaneous).
    pub tau_ms: u32,
    /// Outputs (1-based) in the group.
    pub members: Vec<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PowerGroupKind {
    Port,
    Bus,
    Supply,
    Global,
}

// --- F8 season profiles ---------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ShowProfile {
    pub id: String,
    pub name: String,
    /// Emoji or lucide icon id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Auto-switch window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_range: Option<DateRange>,
    /// Tie-break when date ranges overlap (higher wins).
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub schedule: Schedule,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requests_playlist_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requests_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_dj_voice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub games_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<PowerProfilePart>,
    /// Props kept dark (and out of health checks) in this season.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_prop_ids: Vec<String>,
    /// Library filter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// The season-specific part of [`PowerSettings`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct PowerProfilePart {
    #[serde(default)]
    pub dim: Vec<DimWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_brightness: Option<u8>,
}

// --- F14 remote access ----------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSettings {
    /// Serve the public-only listener on 127.0.0.1:8081 (for tunnels).
    #[serde(default)]
    pub public_listener: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tailscale: Option<TailscaleState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloudflare: Option<CloudflareState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TailscaleState {
    pub enabled: bool,
    #[serde(default)]
    pub serve_admin: bool,
    #[serde(default)]
    pub funnel_public: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareState {
    /// "quick" | "token"
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_host: Option<String>,
    /// A tunnel token is stored (the token itself is never in show.json).
    #[serde(default)]
    pub token_set: bool,
}

// --- F15 updates ----------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    #[default]
    Stable,
    Beta,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AutoUpdate {
    Off,
    #[default]
    Notify,
    Install,
}

/// Daily window for automatic installs ("HH:MM"; `days` empty = every day).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateWindow {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub days: Vec<Weekday>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSettings {
    #[serde(default)]
    pub channel: UpdateChannel,
    #[serde(default)]
    pub auto: AutoUpdate,
    #[serde(default = "default_update_window")]
    pub window: UpdateWindow,
    /// Never install within this many hours before a show window.
    #[serde(default = "default_quiet_hours")]
    pub avoid_show_hours: u32,
}

fn default_update_window() -> UpdateWindow {
    UpdateWindow {
        from: "10:00".into(),
        to: "14:00".into(),
        days: vec![],
    }
}

fn default_quiet_hours() -> u32 {
    2
}

impl Default for UpdateSettings {
    fn default() -> Self {
        UpdateSettings {
            channel: UpdateChannel::Stable,
            auto: AutoUpdate::Notify,
            window: default_update_window(),
            avoid_show_hours: default_quiet_hours(),
        }
    }
}

// --- F16 xLights ----------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct XlightsSettings {
    /// Answer xLights "FPP Connect" uploads (root-mounted `/api/...` subset).
    #[serde(default)]
    pub fpp_connect: bool,
    /// Argon2 hash of the upload password. Write-only: the API returns `""`
    /// when one is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_hash: Option<String>,
    /// Add uploaded sequences to the playlist xLights names.
    #[serde(default = "yes")]
    pub add_to_playlists: bool,
    /// Phase 2: SMB drop folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_folder: Option<String>,
}

impl Default for XlightsSettings {
    fn default() -> Self {
        XlightsSettings {
            fpp_connect: false,
            password_hash: None,
            add_to_playlists: true,
            watch_folder: None,
        }
    }
}

// --- F20 sensor nodes -----------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SensorNode {
    pub id: String,
    pub name: String,
    /// e.g. "esp32c3".
    #[serde(default)]
    pub hw: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default)]
    pub inputs: Vec<SensorInput>,
    #[serde(default)]
    pub adopted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SensorInput {
    /// e.g. "pir1".
    pub id: String,
    pub name: String,
    pub pin: u8,
    pub kind: SensorInputKind,
    #[serde(default)]
    pub active_low: bool,
    #[serde(default = "default_debounce")]
    pub debounce_ms: u32,
    #[serde(default)]
    pub hold_ms: u32,
    /// `kind: current` (INA219/INA226 on the node's I²C bus, `pin` = its
    /// 7-bit address): shunt resistance in milliohms (WS6 addition).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shunt_milliohms: Option<f32>,
}

fn default_debounce() -> u32 {
    30
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum SensorInputKind {
    Motion,
    Button,
    Beam,
    Contact,
    Current,
}

/// One input of a sensor node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct SensorRef {
    pub sensor_node_id: String,
    pub input: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum TemperatureUnit {
    #[default]
    C,
    F,
}

/// How the UI shows measurements. Values are always stored metric (°C).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct UnitSettings {
    #[serde(default)]
    pub temperature: TemperatureUnit,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum GamePlayWindow {
    #[default]
    DuringShow,
    Anytime,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum InviteStyle {
    #[default]
    Text,
    Qr,
    Alternate,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ScaleMode {
    #[default]
    Fit,
    Stretch,
}

/// Visitor-playable games on a matrix prop (port of tlchandler/fpp-mariobros).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GameSettings {
    pub enabled: bool,
    /// Matrix prop the game is shown on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix_prop_id: Option<String>,
    pub port: u16,
    pub game_seconds: u32,
    pub cooldown_minutes: u32,
    /// e.g. "1-1,1-2,4-1"; empty = all levels.
    #[serde(default)]
    pub levels: String,
    pub play_window: GamePlayWindow,
    pub pause_show: bool,
    pub santa_hat: bool,
    pub arcade_mode: bool,
    pub arcade_minutes: u32,
    pub arcade_idle_seconds: u32,
    #[serde(default)]
    pub public_url: String,
    pub invite_every_minutes: u32,
    pub invite_style: InviteStyle,
    pub invite_flashes: u32,
    pub invite_color: String,
    pub scale_mode: ScaleMode,
    pub output_fps: u32,
    pub brightness: u8,
    pub volume: u8,
    /// left, top, right, bottom of the NES screen shown in Mario mode.
    pub crop: [u32; 4],
    /// Most phones from one visitor address (a household, or a mobile
    /// network's shared address) that may wait in line or play at once
    /// (0 = no limit).
    #[serde(default = "default_games_queue_per_visitor")]
    pub max_queue_per_visitor: u32,
}

fn default_games_queue_per_visitor() -> u32 {
    3
}

impl Default for GameSettings {
    fn default() -> Self {
        GameSettings {
            enabled: false,
            matrix_prop_id: None,
            port: 8088,
            game_seconds: 60,
            cooldown_minutes: 5,
            levels: String::new(),
            play_window: GamePlayWindow::DuringShow,
            pause_show: true,
            santa_hat: true,
            arcade_mode: false,
            arcade_minutes: 0,
            arcade_idle_seconds: 600,
            public_url: String::new(),
            invite_every_minutes: 5,
            invite_style: InviteStyle::Text,
            invite_flashes: 3,
            invite_color: "#ff0000".into(),
            scale_mode: ScaleMode::Fit,
            output_fps: 40,
            brightness: 100,
            volume: 80,
            crop: [8, 32, 256, 224],
            max_queue_per_visitor: default_games_queue_per_visitor(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AudioSettings {
    pub device: String,
    pub volume: u8,
    pub normalize: bool,
    pub target_lufs: f32,
    /// How much later the audience hears the sound than it leaves this
    /// controller's audio output (FM transmitter, HDMI TV, Bluetooth,
    /// distance: ~3 ms per metre), in ms. Every controller's lights are
    /// delayed by this much so they match what is heard. Negative values
    /// make the lights earlier. Set with Settings → Audio → "Sync lights to
    /// sound". Range [`OUTPUT_DELAY_RANGE_MS`].
    #[serde(default)]
    pub output_delay_ms: i32,
    /// Last automatic / manual calibration (F1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_calibration: Option<AudioCalibration>,
}

/// Allowed range of [`AudioSettings::output_delay_ms`].
pub const OUTPUT_DELAY_RANGE_MS: std::ops::RangeInclusive<i32> = -500..=2000;

impl Default for AudioSettings {
    fn default() -> Self {
        AudioSettings {
            device: "default".into(),
            volume: 80,
            normalize: true,
            target_lufs: -16.0,
            output_delay_ms: 0,
            last_calibration: None,
        }
    }
}

/// Pixel output options (show-wide, sent to followers).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct OutputSettings {
    /// Experimental: start every string's data so that all strings on a
    /// controller end together ("bottom-aligned"), so strings of different
    /// lengths show a new frame at the same moment instead of up to 49 ms
    /// apart. Off by default; validate on real pixels before a show.
    #[serde(default)]
    pub latch_align: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EmailSettings {
    pub smtp_host: String,
    pub smtp_port: u16,
    pub username: String,
    pub password: String,
    pub from: String,
    pub to: String,
    pub tls: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NtfySettings {
    pub server: String,
    pub topic: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AlertRules {
    pub temp_c: f32,
    pub voltage_min: f32,
    pub follower_offline: bool,
    pub show_failure: bool,
}

impl Default for AlertRules {
    fn default() -> Self {
        AlertRules {
            temp_c: 60.0,
            voltage_min: 11.0,
            follower_offline: true,
            show_failure: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct AlertSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<EmailSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ntfy: Option<NtfySettings>,
    #[serde(default)]
    pub rules: AlertRules,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MqttSettings {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    pub base_topic: String,
    pub home_assistant_discovery: bool,
}

impl Default for MqttSettings {
    fn default() -> Self {
        MqttSettings {
            enabled: false,
            host: "homeassistant.local".into(),
            port: 1883,
            username: None,
            password: None,
            base_topic: "pixelplus".into(),
            home_assistant_discovery: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RequestSettings {
    pub enabled: bool,
    pub max_queue: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playlist_id: Option<String>,
    pub title: String,
    pub message: String,
    /// FM station visitors tune to, e.g. "88.7 FM". Shown on the request page and yard sign.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radio_frequency: Option<String>,
    /// Internet address of the request page (e.g. through a tunnel), used for QR codes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    /// Most requests one visitor (address) may make per hour (0 = no limit;
    /// a burst limit of 3 per 10 minutes always applies).
    #[serde(default = "default_requests_per_visitor_per_hour")]
    pub per_visitor_per_hour: u32,
    /// Most requests from everyone together per hour (0 = no limit).
    #[serde(default = "default_requests_max_per_hour")]
    pub max_per_hour: u32,
}

fn default_requests_per_visitor_per_hour() -> u32 {
    6
}

fn default_requests_max_per_hour() -> u32 {
    60
}

impl Default for RequestSettings {
    fn default() -> Self {
        RequestSettings {
            enabled: false,
            max_queue: 5,
            playlist_id: None,
            title: "Request a song".into(),
            message: "Pick a song and it will play next. Merry Christmas!".into(),
            radio_frequency: None,
            public_url: None,
            per_visitor_per_hour: default_requests_per_visitor_per_hour(),
            max_per_hour: default_requests_max_per_hour(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum TtsMode {
    #[default]
    Auto,
    Device,
    Browser,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct TtsSettings {
    #[serde(default)]
    pub mode: TtsMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct OledSettings {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SecuritySettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_hash: Option<String>,
    /// Extra host names the web UI / API answers to (tunnel or own domain);
    /// `*.example.com` wildcards allowed. IP addresses, `localhost` and this
    /// controller's `<hostname>(.local)` always work.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_hosts: Vec<String>,
    /// Reverse proxies on the network (IPs or CIDRs) whose `X-Forwarded-For`
    /// is believed. Proxies on this machine are always trusted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trusted_proxies: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TriggerKind {
    Gpio,
    Http,
    /// An input of an ESP32 sensor node (F20), see [`Trigger::sensor`].
    Sensor,
}

/// When a trigger may fire (F20).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum TriggerWhen {
    #[default]
    Always,
    /// Only while a show window plays.
    ShowOnly,
    /// Only while the idle look runs.
    IdleOnly,
    /// Only outside show and idle times.
    OffOnly,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TriggerActionType {
    PlayPlaylist,
    PlaySequence,
    Stop,
    Effect,
    /// Layer a short sequence / effect over whatever plays (F20).
    Surprise,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TriggerAction {
    #[serde(rename = "type")]
    pub kind: TriggerActionType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
    /// Surprise: props to draw on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Target>,
    /// Surprise: how long (ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Surprise: "sequence" | "effect" (what `ref` names).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Trigger {
    pub id: String,
    pub name: String,
    pub kind: TriggerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpio: Option<u8>,
    pub action: TriggerAction,
    /// `kind: sensor`: which sensor input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensor: Option<SensorRef>,
    /// Minimum seconds between firings (0 = none).
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub cooldown_s: u32,
    #[serde(default, skip_serializing_if = "is_default")]
    pub when: TriggerWhen,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_window: Option<TimeWindow>,
    /// 0 = unlimited.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub max_per_hour: u32,
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_roundtrips_camel_case() {
        let mut show = Show::default();
        show.playlists.push(Playlist {
            smart: Default::default(),
            id: "p1".into(),
            name: "Main".into(),
            items: vec![PlaylistItem::Effect {
                id: "i1".into(),
                effect_id: "e1".into(),
                duration_ms: 5000,
            }],
            intro: vec![],
            outro: vec![],
            shuffle: false,
            repeat: true,
            crossfade_ms: 0,
        });
        let json = serde_json::to_string(&show).unwrap();
        assert!(json.contains("\"propGroups\""));
        assert!(json.contains("\"type\":\"effect\""));
        assert!(json.contains("\"effectId\":\"e1\""));
        assert!(json.contains("\"durationMs\":5000"));
        let back: Show = serde_json::from_str(&json).unwrap();
        assert_eq!(back, show);
    }

    #[test]
    fn difftxlarge_labels() {
        assert_eq!(BoardKind::Difftxlarge.output_label(1), "J1-1");
        assert_eq!(BoardKind::Difftxlarge.output_label(60), "J15-4");
        assert_eq!(BoardKind::Difftx.output_label(3), "Port 3");
    }

    #[test]
    fn units_and_visitor_settings_are_optional() {
        // Older show files have neither field: they load with the defaults and stay absent on save.
        let s: ShowSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.units, None);
        assert_eq!(s.requests.radio_frequency, None);
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("units"));
        assert!(!json.contains("radioFrequency"));

        let s: ShowSettings = serde_json::from_value(serde_json::json!({
            "units": { "temperature": "f" },
            "requests": {
                "enabled": true, "maxQueue": 5, "title": "t", "message": "m",
                "radioFrequency": "88.7 FM", "publicUrl": "lights.example.com/request"
            }
        }))
        .unwrap();
        assert_eq!(s.units.as_ref().unwrap().temperature, TemperatureUnit::F);
        assert_eq!(s.requests.radio_frequency.as_deref(), Some("88.7 FM"));
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["units"]["temperature"], "f");
        assert_eq!(v["requests"]["publicUrl"], "lights.example.com/request");
    }

    /// Every optional field added by the feature wave (F1–F20) stays out of
    /// `show.json` while empty, so shows only grow when a feature is used.
    #[test]
    fn feature_wave_fields_are_omitted_when_empty() {
        let v: serde_json::Value = serde_json::json!({
            "version": 1, "name": "S",
            "nodes": [{"id": "n", "name": "N", "hostname": "h", "role": "leader", "board": "difftx",
                       "outputs": [{"index": 1, "label": "Port 1", "brightness": 100, "gamma": 1.0, "enabled": true}]}],
            "receivers": [{"id": "r", "name": "R", "kind": "diffrx", "nodeId": "n", "jack": 1}],
            "props": [{"id": "p", "name": "P", "kind": "arch", "pixelCount": 10, "channelStart": 0,
                       "layout": {"x": 0, "y": 0, "w": 1, "h": 1}}],
            "sequences": [{"id": "s", "name": "S", "file": "s.fseq", "durationMs": 1, "frameMs": 50, "channelCount": 3}],
            "media": [{"id": "m", "name": "M", "kind": "song", "file": "m.mp3", "durationMs": 1}],
            "playlists": [{"id": "pl", "name": "PL", "items": [{"type": "pause", "id": "i", "durationMs": 5}]}],
            "schedule": {"entries": [{"id": "e", "name": "E", "enabled": true, "playlistId": "pl", "days": [],
                          "start": {"kind": "clock", "time": "17:00"}, "end": {"kind": "clock", "time": "22:00"}}]},
            "settings": {"triggers": [{"id": "t", "name": "T", "kind": "gpio", "gpio": 17,
                                       "action": {"type": "stop"}}]}
        });
        let show: Show = serde_json::from_value(v).unwrap();
        let json = serde_json::to_string(&show).unwrap();
        for key in [
            "profiles",
            "activeProfileId",
            "profileAutoSwitch",
            "powerSupplies",
            "tagDefs",
            "sensorNodes",
            "serial",
            "hardwareHistory",
            "measuredPixels",
            "mainFuseAmps",
            "suspectPixels",
            "source",
            "generated",
            "tags",
            "analysis",
            "originalName",
            "originalSize",
            "smart",
            "startExact",
            "lastCalibration",
            "sensor",
            "cooldownS",
            "\"when\"",
            "activeWindow",
            "target",
            "extraNames",
            "passwordHash",
            "watchFolder",
            "tailscale",
            "cloudflare",
            "globalAmps",
        ] {
            let key = if key.starts_with('"') {
                key.to_string()
            } else {
                format!("\"{key}\"")
            };
            assert!(!json.contains(&key), "{key} should be omitted: {json}");
        }
        // Always-present settings carry their documented defaults.
        let v = serde_json::to_value(&show).unwrap();
        // `maxPerHour` is also a (always present) song-request setting.
        assert!(v["settings"]["triggers"][0].get("maxPerHour").is_none());
        assert_eq!(v["settings"]["requests"]["maxPerHour"], 60);
        assert_eq!(v["settings"]["requests"]["perVisitorPerHour"], 6);
        assert_eq!(v["settings"]["games"]["maxQueuePerVisitor"], 3);
        assert_eq!(v["formatVersion"], SHOW_FORMAT_VERSION);
        assert_eq!(v["settings"]["https"]["enabled"], true);
        assert_eq!(v["settings"]["reports"]["time"], "07:00");
        assert_eq!(v["settings"]["reports"]["keepDays"], 90);
        assert_eq!(v["settings"]["power"]["mode"], "warn");
        assert_eq!(v["settings"]["power"]["maxBrightness"], 100);
        assert_eq!(v["settings"]["updates"]["channel"], "stable");
        assert_eq!(v["settings"]["updates"]["auto"], "notify");
        assert_eq!(v["settings"]["updates"]["window"]["from"], "10:00");
        assert_eq!(v["settings"]["xlights"]["fppConnect"], false);
        assert_eq!(v["settings"]["xlights"]["addToPlaylists"], true);
        assert_eq!(v["settings"]["remote"]["publicListener"], false);
    }

    /// A show using every new feature round-trips with the documented JSON names.
    #[test]
    fn feature_wave_show_roundtrips() {
        let v: serde_json::Value = serde_json::from_str(r##"{
            "version": 3, "name": "Full", "formatVersion": 1,
            "nodes": [{"id": "n", "name": "N", "hostname": "h", "role": "follower", "board": "difftx",
                "serial": "PPX-1", "hardwareHistory": [{"at": "2026-01-01T00:00:00Z", "board": "difftx", "reason": "replaced"}],
                "outputs": [{"index": 1, "label": "Port 1", "brightness": 100, "gamma": 1.0, "enabled": true,
                    "measuredPixels": {"count": 48, "method": "camera", "at": "2026-01-01T00:00:00Z", "dead": [48, 49]}}]}],
            "receivers": [{"id": "r", "name": "R", "kind": "diffrx", "nodeId": "n", "jack": 1, "mainFuseAmps": 30.0}],
            "props": [{"id": "p", "name": "P", "kind": "arch", "pixelCount": 10, "channelStart": 0, "suspectPixels": [3],
                "layout": {"x": 0.0, "y": 0.0, "w": 1.0, "h": 1.0, "source": "camera"}}],
            "sequences": [{"id": "s", "name": "S", "file": "s.fseq", "durationMs": 1, "frameMs": 25, "channelCount": 3,
                "tags": ["kids"], "generated": {"kind": "autoShow", "mediaId": "m", "style": "classic", "seed": 7,
                "analysisVersion": 1, "propsHash": "abc"}}],
            "media": [{"id": "m", "name": "M", "kind": "song", "file": "m.mp3", "durationMs": 1, "tags": ["kids"],
                "originalName": "Song.mp3", "originalSize": 1234,
                "analysis": {"version": 1, "bpm": 120.0, "bpmConfidence": 0.8, "beatCount": 400, "firstBeatMs": 250, "energy": 0.5, "sections": 6}}],
            "playlists": [{"id": "pl", "name": "PL",
                "intro": [{"type": "countdown", "id": "c", "durationMs": 10000, "matrixPropId": "p", "color": "#ffffff",
                    "others": "pulse", "finale": "none", "djClipId": "d", "djOffsetMs": -200, "tick": true}],
                "smart": {"includeTags": ["kids"], "includeMode": "all", "excludeTags": ["halloween"],
                    "targetDurationMs": 2700000, "noRepeatNights": 1, "order": "rotation",
                    "timeRules": [{"before": {"kind": "clock", "time": "19:00"}, "requireTags": ["kids"]}],
                    "pinnedLast": [{"type": "sequence", "id": "i2", "sequenceId": "s"}], "interleaveEvery": 3}}],
            "schedule": {"entries": [{"id": "e", "name": "E", "enabled": true, "playlistId": "pl", "days": [],
                "start": {"kind": "clock", "time": "17:30"}, "end": {"kind": "clock", "time": "22:00"}, "startExact": true}]},
            "profiles": [{"id": "x", "name": "Christmas", "icon": "🎄", "dateRange": {"start": "11-01", "end": "01-06"},
                "priority": 1, "schedule": {}, "disabledPropIds": ["p"], "tags": ["season:christmas"],
                "power": {"dim": [], "maxBrightness": 80}}],
            "activeProfileId": "x", "profileAutoSwitch": true,
            "powerSupplies": [{"id": "ps", "name": "Garage PSU", "volts": 12.0, "amps": 29.0, "receiverIds": ["r"],
                "directOutputs": [{"nodeId": "n", "output": 2}], "sensor": {"sensorNodeId": "sn", "input": "ch1"}}],
            "tagDefs": [{"name": "kids", "color": "#00ff00"}],
            "sensorNodes": [{"id": "sn", "name": "Sidewalk", "hw": "esp32c3", "adopted": true,
                "inputs": [{"id": "pir1", "name": "PIR", "pin": 4, "kind": "motion"}]}],
            "settings": {
                "audio": {"device": "default", "volume": 80, "normalize": true, "targetLufs": -16.0, "outputDelayMs": 212,
                    "lastCalibration": {"measuredAt": "2026-01-01T00:00:00Z", "method": "phone", "residualMs": 3.5,
                        "spreadMs": 4.0, "matches": 30, "appliedDelayMs": 212, "device": "Pixel 8"}},
                "https": {"enabled": false, "extraNames": ["lights.lan"]},
                "reports": {"enabled": true, "time": "06:30", "email": false, "push": true, "onlyWhenProblems": true, "keepDays": 30},
                "power": {"mode": "limit", "safety": 0.8, "globalAmps": 15.0,
                    "dim": [{"from": {"kind": "clock", "time": "22:00"}, "to": {"kind": "clock", "time": "23:00"}, "brightness": 40, "days": ["fri"]}],
                    "maxBrightness": 90},
                "remote": {"publicListener": true, "tailscale": {"enabled": true, "serveAdmin": true, "funnelPublic": false, "dnsName": "pp.ts.net"},
                    "cloudflare": {"mode": "token", "publicHost": "lights.example.com", "tokenSet": true}},
                "updates": {"channel": "beta", "auto": "install", "window": {"from": "09:00", "to": "12:00", "days": ["sat"]}, "avoidShowHours": 3},
                "xlights": {"fppConnect": true, "passwordHash": "$argon2id$x", "addToPlaylists": false},
                "triggers": [{"id": "t", "name": "Doorbell", "kind": "sensor", "sensor": {"sensorNodeId": "sn", "input": "pir1"},
                    "cooldownS": 60, "when": "showOnly", "maxPerHour": 10,
                    "activeWindow": {"from": {"kind": "clock", "time": "17:00"}, "to": {"kind": "clock", "time": "21:00"}},
                    "action": {"type": "surprise", "ref": "e1", "source": "effect", "durationMs": 5000, "target": {"propIds": ["p"]}}}]
            }
        }"##).unwrap();
        let show: Show = serde_json::from_value(v.clone()).unwrap();
        assert!(matches!(
            show.playlists[0].intro[0],
            PlaylistItem::Countdown {
                others: CountdownOthers::Pulse,
                finale: CountdownFinale::None,
                ..
            }
        ));
        assert_eq!(show.settings.triggers[0].kind, TriggerKind::Sensor);
        assert_eq!(
            show.settings.triggers[0].action.kind,
            TriggerActionType::Surprise
        );
        assert_eq!(show.settings.power.mode, LimiterMode::Limit);
        let back: Show = serde_json::from_str(&serde_json::to_string(&show).unwrap()).unwrap();
        assert_eq!(back, show);
        let out = serde_json::to_value(&show).unwrap();
        assert_eq!(out["playlists"][0]["intro"][0]["type"], "countdown");
        assert_eq!(out["playlists"][0]["intro"][0]["djOffsetMs"], -200);
        assert_eq!(out["sequences"][0]["generated"]["kind"], "autoShow");
        assert_eq!(out["settings"]["triggers"][0]["when"], "showOnly");
        // A minimal countdown gets its defaults.
        let c: PlaylistItem =
            serde_json::from_str(r#"{"type":"countdown","id":"c","durationMs":10000}"#).unwrap();
        let PlaylistItem::Countdown {
            text,
            others,
            finale,
            tick,
            ..
        } = c
        else {
            panic!()
        };
        assert_eq!(
            (text.as_str(), others, finale, tick),
            ("{s}", CountdownOthers::Fill, CountdownFinale::Flash, false)
        );
    }

    #[test]
    fn board_kind_json() {
        assert_eq!(
            serde_json::to_string(&BoardKind::BarePi).unwrap(),
            "\"bare-pi\""
        );
    }
}
