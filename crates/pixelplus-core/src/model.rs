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
    pub channel_start: u32,
    #[serde(default = "default_cpp")]
    pub channels_per_pixel: u8,
    #[serde(default)]
    pub segments: Vec<PropSegment>,
    #[serde(default)]
    pub group_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<PropLayout>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_milliamps_per_pixel: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
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

    /// Byte length of this prop inside an fseq frame.
    pub fn channel_len(&self) -> u32 {
        self.pixel_count * self.channels_per_pixel as u32
    }

    /// Pixels not covered by any segment (unwired).
    pub fn unwired_pixels(&self) -> u32 {
        let wired: u32 = self.segments.iter().map(|s| s.pixel_count).sum();
        self.pixel_count.saturating_sub(wired)
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
}

impl PlaylistItem {
    pub fn id(&self) -> &str {
        match self {
            PlaylistItem::Sequence { id, .. }
            | PlaylistItem::Dj { id, .. }
            | PlaylistItem::Effect { id, .. }
            | PlaylistItem::Media { id, .. }
            | PlaylistItem::Pause { id, .. } => id,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AudioSettings {
    pub device: String,
    pub volume: u8,
    pub normalize: bool,
    pub target_lufs: f32,
}

impl Default for AudioSettings {
    fn default() -> Self {
        AudioSettings {
            device: "default".into(),
            volume: 80,
            normalize: true,
            target_lufs: -16.0,
        }
    }
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
}

impl Default for RequestSettings {
    fn default() -> Self {
        RequestSettings {
            enabled: false,
            max_queue: 5,
            playlist_id: None,
            title: "Request a song".into(),
            message: "Pick a song and it will play next. Merry Christmas!".into(),
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
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TriggerKind {
    Gpio,
    Http,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TriggerActionType {
    PlayPlaylist,
    PlaySequence,
    Stop,
    Effect,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TriggerAction {
    #[serde(rename = "type")]
    pub kind: TriggerActionType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_roundtrips_camel_case() {
        let mut show = Show::default();
        show.playlists.push(Playlist {
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
    fn board_kind_json() {
        assert_eq!(
            serde_json::to_string(&BoardKind::BarePi).unwrap(),
            "\"bare-pi\""
        );
    }
}
