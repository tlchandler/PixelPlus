//! Auto light show generator (F2, ARCHITECTURE §12.2).
//!
//! A pure function of (props, song analysis, style, seed) → an ordinary FSEQ
//! v2 file (zstd, 25 ms frames) in the show's channel space. Because the
//! result is a normal sequence, it is sliced for followers, previewed (F3),
//! power-estimated and requestable like any xLights sequence.
//!
//! Choreography:
//! * Each analysed section picks a look by its energy level: low →
//!   colour washes / twinkle / breathing, mid → chases and waves moving with
//!   the tempo, high → meteors, fast chases, candy stripes and sparkles.
//! * Downbeats rotate the style's palettes (a snap on the bar, not a fade);
//!   section changes cross-fade over [`SECTION_FADE_MS`].
//! * Beats pulse the "beat group" (the big props: trees, matrices, arches and
//!   the largest others) with `env(t) = exp(−(t − b)/τ)`, τ = [`BEAT_TAU_MS`];
//!   other props follow the song's loudness.
//! * The strongest low-band hits (top 5 %) in loud sections flash every prop
//!   warm white for [`FLASH_MS`] — never more than 3 per second and never
//!   saturated red (photosensitivity guidance).
//! * High-band hits (hi-hats, bells) sparkle on the small props.
//! * Style `voice` ("speak with lights", DJ clips) is an energy follower:
//!   every prop glows with the voice's loudness.
//!
//! Output is deterministic: the same inputs and seed produce identical bytes.

use crate::audio_analysis::{Analysis, Level};
use crate::effects::EffectRenderer;
use crate::fseq::{FseqWriter, FseqWriterOptions};
use crate::model::{EffectKind, EffectPreset, Prop, PropKind, Show};
use crate::smartlist::{fnv1a, SplitMix};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Digest;
use std::path::Path;

/// Frame length of generated sequences.
pub const FRAME_MS: u32 = 25;
/// Beat envelope decay.
pub const BEAT_TAU_MS: f32 = 120.0;
/// Length of an accent flash.
pub const FLASH_MS: u64 = 60;
/// Shortest time between flashes (at most 3 per second).
pub const MIN_FLASH_GAP_MS: u64 = 340;
/// Cross-fade between section looks.
pub const SECTION_FADE_MS: u64 = 400;
/// Songs longer than this are refused (matches the analysis cap).
pub const MAX_DURATION_MS: u64 = crate::audio_analysis::MAX_DURATION_MS;

#[derive(Debug, thiserror::Error)]
pub enum AutoShowError {
    #[error("unknown style \"{0}\"")]
    UnknownStyle(String),
    #[error("no props to light")]
    NoProps,
    #[error("the song is empty")]
    Empty,
    #[error("writing the sequence: {0}")]
    Write(#[from] crate::fseq::FseqError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("cancelled")]
    Cancelled,
}

/// A choreography style (`GET /autoshow/styles`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StyleInfo {
    pub id: String,
    pub name: String,
    pub description: String,
}

struct Style {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    palettes: &'static [&'static [&'static str]],
    low: &'static [EffectKind],
    mid: &'static [EffectKind],
    high: &'static [EffectKind],
    /// Beat pulse depth multiplier (calm styles pulse less).
    punch: f32,
    /// Allow accent flashes.
    flashes: bool,
}

use EffectKind as K;

const STYLES: &[Style] = &[
    Style {
        id: "classic",
        name: "Classic Christmas",
        description: "Warm reds, greens and gold. Gentle in the verses, big on the chorus.",
        palettes: &[
            &["#ff1a0a", "#12b02a"],
            &["#ffb46b", "#ff1a0a"],
            &["#12b02a", "#ffc861"],
            &["#ffffff", "#ff1a0a", "#12b02a"],
        ],
        low: &[K::Twinkle, K::Colorwash, K::Breathe],
        mid: &[K::Wave, K::Chase, K::Candycane],
        high: &[K::Meteor, K::Chase, K::Sparkle],
        punch: 1.0,
        flashes: true,
    },
    Style {
        id: "candy",
        name: "Candy",
        description: "Red and white stripes that bounce on the beat.",
        palettes: &[
            &["#ff0014", "#ffffff"],
            &["#ffffff", "#ff3c64"],
            &["#ff0014", "#ffe0e6", "#12b02a"],
        ],
        low: &[K::Candycane, K::Twinkle],
        mid: &[K::Candycane, K::Chase],
        high: &[K::Candycane, K::Meteor, K::Chase],
        punch: 1.0,
        flashes: true,
    },
    Style {
        id: "rock",
        name: "Rock",
        description: "Bold colors, fast chases and a hit on every strong beat.",
        palettes: &[
            &["#ff1400", "#ffffff"],
            &["#0050ff", "#ffffff"],
            &["#ff7800", "#ff1400"],
            &["#b400ff", "#00b4ff"],
        ],
        low: &[K::Breathe, K::Colorwash],
        mid: &[K::Chase, K::Wave],
        high: &[K::Meteor, K::Chase, K::Strobe],
        punch: 1.25,
        flashes: true,
    },
    Style {
        id: "calm",
        name: "Calm",
        description: "Slow color washes that swell with the music. No flashes.",
        palettes: &[
            &["#1e3cff", "#8c50ff"],
            &["#ffffff", "#50b4ff"],
            &["#ffb46b", "#ff8c5a"],
            &["#00c8a0", "#1e3cff"],
        ],
        low: &[K::Colorwash, K::Breathe],
        mid: &[K::Wave, K::Twinkle],
        high: &[K::Wave, K::Sparkle, K::Colorwash],
        punch: 0.5,
        flashes: false,
    },
    Style {
        id: "party",
        name: "Party",
        description: "Rainbow everything, sparkles on the high notes.",
        palettes: &[
            &["#ff0000", "#ffb400", "#00ff3c", "#00b4ff", "#b400ff"],
            &["#ff00b4", "#00ffff"],
            &["#ffff00", "#ff00ff", "#00ffff"],
        ],
        low: &[K::Rainbow, K::Colorwash],
        mid: &[K::Chase, K::Rainbow, K::Wave],
        high: &[K::Rainbow, K::Meteor, K::Sparkle],
        punch: 1.1,
        flashes: true,
    },
    Style {
        id: "voice",
        name: "Speak with lights",
        description: "For DJ talk: every prop glows with the voice.",
        palettes: &[&["#ffb46b"], &["#ffd9a0"]],
        low: &[K::Solid],
        mid: &[K::Solid],
        high: &[K::Solid],
        punch: 0.0,
        flashes: false,
    },
];

/// The available styles, in menu order.
pub fn styles() -> Vec<StyleInfo> {
    STYLES
        .iter()
        .map(|s| StyleInfo {
            id: s.id.into(),
            name: s.name.into(),
            description: s.description.into(),
        })
        .collect()
}

/// Is `id` a known style?
pub fn style_exists(id: &str) -> bool {
    STYLES.iter().any(|s| s.id == id)
}

/// The props a generated show uses: `prop_ids` in show order, or all.
pub fn selected_props<'a>(show: &'a Show, prop_ids: &[String]) -> Vec<&'a Prop> {
    show.props
        .iter()
        .filter(|p| prop_ids.is_empty() || prop_ids.contains(&p.id))
        .filter(|p| p.pixel_count > 0)
        .collect()
}

/// Everything about the layout a generated show depends on (for
/// `GeneratedInfo.propsHash`): all props' channel runs (the channel space)
/// plus the used props' kind, size, shape and position.
pub fn props_hash(show: &Show, prop_ids: &[String]) -> String {
    let mut h = sha2::Sha256::new();
    h.update(b"autoshow1|");
    for p in &show.props {
        let used = prop_ids.is_empty() || prop_ids.contains(&p.id);
        h.update(format!("{}|{}|{}|", p.id, p.pixel_count, p.channels_per_pixel).as_bytes());
        for r in p.channel_ranges() {
            h.update(
                format!("{},{},{};", r.prop_offset, r.channel_start, r.pixel_count).as_bytes(),
            );
        }
        if used {
            let v = json!({"k": p.kind, "l": p.layout, "m": p.matrix.as_ref().map(|m| (m.width, m.height))});
            h.update(v.to_string().as_bytes());
        }
    }
    crate::fseq::to_hex(&h.finalize())[..16].to_string()
}

/// What to generate.
#[derive(Debug, Clone)]
pub struct AutoShowRequest<'a> {
    pub show: &'a Show,
    /// Props to light (empty = all).
    pub prop_ids: &'a [String],
    pub analysis: &'a Analysis,
    pub style: &'a str,
    pub seed: u32,
    /// Song length (the sequence length); 0 = the analysis length.
    pub duration_ms: u64,
    /// Written into the fseq header (`mf`), like xLights does.
    pub media_filename: Option<String>,
}

/// Result of [`render`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoShowOutput {
    pub frame_ms: u32,
    pub frame_count: u32,
    pub channel_count: u32,
    pub duration_ms: u64,
}

/// Render the show into an FSEQ file at `out` (via a temp file). `progress`
/// (0..1) returns false to cancel.
pub fn render(
    req: &AutoShowRequest,
    out: &Path,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<AutoShowOutput, AutoShowError> {
    let mut c = Choreographer::new(req)?;
    let tmp = out.with_extension("fseq.tmp");
    let mut opts =
        FseqWriterOptions::new(c.channel_count, FRAME_MS as u8).fit_frame_count(c.frame_count);
    opts.media_filename = req.media_filename.clone();
    opts.producer = Some("PixelPlus auto light show".into());
    let res = (|| {
        let mut w = FseqWriter::create(&tmp, opts)?;
        let mut frame = vec![0u8; c.channel_count as usize];
        for i in 0..c.frame_count {
            c.frame(u64::from(i) * u64::from(FRAME_MS), &mut frame);
            w.write_frame(&frame)?;
            if i % 200 == 0 && !progress(i as f32 / c.frame_count as f32) {
                return Err(AutoShowError::Cancelled);
            }
        }
        w.finish()?;
        stamp_unique_id(&tmp, req)?;
        std::fs::rename(&tmp, out)?;
        Ok(())
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res?;
    progress(1.0);
    Ok(AutoShowOutput {
        frame_ms: FRAME_MS,
        frame_count: c.frame_count,
        channel_count: c.channel_count,
        duration_ms: c.duration_ms,
    })
}

/// The fseq header's unique id (bytes 24..32, a timestamp by default) made
/// deterministic, so identical inputs give byte-identical files (and the same
/// sha256, so followers keep their cached slices).
fn stamp_unique_id(path: &Path, req: &AutoShowRequest) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom, Write};
    let key = format!(
        "{}|{}|{}|{}|{}",
        props_hash(req.show, req.prop_ids),
        req.style,
        req.seed,
        req.duration_ms,
        serde_json::to_string(&req.analysis.summary()).unwrap_or_default()
    );
    let uid = fnv1a(key.as_bytes()) >> 1;
    let mut f = std::fs::OpenOptions::new().write(true).open(path)?;
    f.seek(SeekFrom::Start(24))?;
    f.write_all(&uid.to_le_bytes())?;
    f.sync_all()
}

/// A look: one effect with one palette on the used props.
struct Look {
    key: (usize, usize),
    renderer: EffectRenderer,
    /// Scales the look to its level's mean brightness, so a sparse twinkle
    /// and a full colour wash of the same level look equally bright.
    norm: f32,
}

/// Mean output brightness (share of full scale) a look is scaled to, by level.
fn level_target(l: Level) -> f32 {
    match l {
        Level::Low => 0.3,
        Level::Mid => 0.45,
        Level::High => 0.6,
    }
}

/// Renders frames of a generated show (channel space). Exposed for tests
/// and tools; [`render`] writes them to a file.
pub struct Choreographer<'a> {
    style: &'static Style,
    analysis: &'a Analysis,
    props: Vec<&'a Prop>,
    /// Byte range of each used prop in the look renderer's frame.
    ranges: Vec<std::ops::Range<usize>>,
    beat_group: Vec<bool>,
    small: Vec<bool>,
    /// (section index) → effect kind.
    section_kind: Vec<EffectKind>,
    /// Beats used for pulses (≥ 1/3 s apart).
    pulse_beats: Vec<u32>,
    flashes: Vec<u32>,
    /// High-band hits for sparkles: (ms, strength).
    sparkles: Vec<(u32, f32)>,
    seed: u32,
    looks: Vec<Look>,
    buf: Vec<u8>,
    prev: Vec<u8>,
    pub channel_count: u32,
    pub frame_count: u32,
    pub duration_ms: u64,
}

impl<'a> Choreographer<'a> {
    pub fn new(req: &AutoShowRequest<'a>) -> Result<Self, AutoShowError> {
        let style = STYLES
            .iter()
            .find(|s| s.id == req.style)
            .ok_or_else(|| AutoShowError::UnknownStyle(req.style.to_string()))?;
        let props = selected_props(req.show, req.prop_ids);
        if props.is_empty() {
            return Err(AutoShowError::NoProps);
        }
        let a = req.analysis;
        let duration_ms = if req.duration_ms > 0 {
            req.duration_ms
        } else {
            a.duration_ms
        }
        .min(MAX_DURATION_MS);
        if duration_ms < u64::from(FRAME_MS) {
            return Err(AutoShowError::Empty);
        }
        let channel_count = req
            .show
            .props
            .iter()
            .map(|p| p.channel_end())
            .max()
            .unwrap_or(0)
            .clamp(3, u64::from(u32::MAX)) as u32;
        let frame_count = duration_ms.div_ceil(u64::from(FRAME_MS)) as u32;

        // Beat group: big props by kind, else the largest third.
        let mut sizes: Vec<u32> = props.iter().map(|p| p.pixel_count).collect();
        sizes.sort_unstable();
        let median = sizes[sizes.len() / 2];
        let big_kind =
            |p: &Prop| matches!(p.kind, PropKind::Tree | PropKind::Matrix | PropKind::Arch);
        let mut beat_group: Vec<bool> = props
            .iter()
            .map(|p| big_kind(p) || p.pixel_count as f32 >= median as f32 * 1.5)
            .collect();
        if !beat_group.iter().any(|&b| b) {
            let cut = sizes[(sizes.len() * 2) / 3];
            beat_group = props.iter().map(|p| p.pixel_count >= cut).collect();
        }
        let small: Vec<bool> = beat_group.iter().map(|b| !b).collect();

        // One effect per section, drawn from the style's list for its level.
        let mut rng = SplitMix(u64::from(req.seed) ^ fnv1a(style.id.as_bytes()));
        let sections = if a.sections.is_empty() {
            vec![crate::audio_analysis::Section {
                start_ms: 0,
                end_ms: duration_ms as u32,
                level: Level::Mid,
            }]
        } else {
            a.sections.clone()
        };
        let mut last: Option<EffectKind> = None;
        let section_kind = sections
            .iter()
            .map(|s| {
                let list = match s.level {
                    Level::Low => style.low,
                    Level::Mid => style.mid,
                    Level::High => style.high,
                };
                let mut k = list[(rng.next() % list.len() as u64) as usize];
                if Some(k) == last && list.len() > 1 {
                    let i = list.iter().position(|&x| x == k).unwrap_or(0);
                    k = list[(i + 1) % list.len()];
                }
                last = Some(k);
                k
            })
            .collect();

        let mut pulse_beats = Vec::new();
        for &b in &a.beats {
            if pulse_beats.last().map_or(true, |&l: &u32| b - l >= 330) {
                pulse_beats.push(b);
            }
        }
        // Flashes: top 5 % low-band onsets in loud sections, ≥ 340 ms apart.
        let mut flashes = Vec::new();
        if style.flashes {
            let mut strengths: Vec<f32> = a
                .onsets
                .iter()
                .filter(|o| o.band == 0)
                .map(|o| o.strength)
                .collect();
            strengths.sort_by(|x, y| x.total_cmp(y));
            if !strengths.is_empty() {
                let th =
                    strengths[((strengths.len() as f32 * 0.95) as usize).min(strengths.len() - 1)];
                for o in a.onsets.iter().filter(|o| o.band == 0 && o.strength >= th) {
                    let loud = a
                        .section_at(u64::from(o.ms))
                        .is_some_and(|s| s.level == Level::High);
                    let ok = flashes
                        .last()
                        .map_or(true, |&l: &u32| u64::from(o.ms - l) >= MIN_FLASH_GAP_MS);
                    if loud && ok {
                        flashes.push(o.ms);
                    }
                }
            }
        }
        let sparkles = a
            .onsets
            .iter()
            .filter(|o| o.band == 2 && o.strength >= 0.25)
            .map(|o| (o.ms, o.strength))
            .collect();

        // Byte ranges in the look renderer's frame (same for every look).
        let probe = EffectRenderer::new(&preset(K::Solid, &["#000000"], 0, 120.0, "p"), &props);
        let ranges = probe.prop_ranges().map(|(_, r)| r).collect();
        let len = probe.frame_len();
        Ok(Choreographer {
            style,
            analysis: a,
            props,
            ranges,
            beat_group,
            small,
            section_kind,
            pulse_beats,
            flashes,
            sparkles,
            seed: req.seed,
            looks: Vec::new(),
            buf: vec![0; len],
            prev: vec![0; len],
            channel_count,
            frame_count,
            duration_ms,
        })
    }

    fn section_index(&self, t: u64) -> usize {
        let s = &self.analysis.sections;
        if s.is_empty() {
            return 0;
        }
        s.iter()
            .rposition(|x| u64::from(x.start_ms) <= t)
            .unwrap_or(0)
            .min(self.section_kind.len() - 1)
    }

    fn look(&mut self, section: usize, palette: usize) -> usize {
        if let Some(i) = self.looks.iter().position(|l| l.key == (section, palette)) {
            return i;
        }
        let kind = self.section_kind[section];
        let pal = self.style.palettes[palette % self.style.palettes.len()];
        let level = self
            .analysis
            .sections
            .get(section)
            .map_or(Level::Mid, |s| s.level);
        let bpm = if self.analysis.bpm > 0.0 {
            self.analysis.bpm
        } else {
            110.0
        };
        let id = format!("auto-{}-{section}", self.seed);
        let p = preset(kind, pal, level_num(level), bpm, &id);
        let renderer = EffectRenderer::new(&p, &self.props);
        // Measure the look's mean brightness over a few seconds.
        let mut probe = vec![0u8; renderer.frame_len()];
        let mut sum = 0u64;
        let mut n = 0u64;
        for k in 0..8u64 {
            renderer.render(k * 450, &mut probe);
            sum += probe.iter().map(|&v| u64::from(v)).sum::<u64>();
            n += probe.len() as u64;
        }
        let mean = sum as f32 / (n.max(1) as f32 * 255.0);
        let norm = if mean > 0.01 {
            (level_target(level) / mean).clamp(0.3, 2.5)
        } else {
            1.0
        };
        // Keep at most 3 looks (memory on a Pi Zero).
        if self.looks.len() >= 3 {
            self.looks.remove(0);
        }
        self.looks.push(Look {
            key: (section, palette),
            renderer,
            norm,
        });
        self.looks.len() - 1
    }

    /// Render the channel frame at `t_ms` into `out` (`channel_count` bytes).
    pub fn frame(&mut self, t_ms: u64, out: &mut [u8]) {
        out.fill(0);
        let a = self.analysis;
        let sec = self.section_index(t_ms);
        let bars = a.downbeats.partition_point(|&d| u64::from(d) <= t_ms);
        let palette = (bars + self.seed as usize) % self.style.palettes.len();
        let li = self.look(sec, palette);
        let mut buf = std::mem::take(&mut self.buf);
        self.looks[li].renderer.render(t_ms, &mut buf);
        let norm = self.looks[li].norm;
        // Cross-fade from the previous section's look.
        let start = a.sections.get(sec).map_or(0, |s| u64::from(s.start_ms));
        if sec > 0 && t_ms < start + SECTION_FADE_MS {
            let bars_prev = a.downbeats.partition_point(|&d| u64::from(d) < start);
            let pal_prev =
                (bars_prev.saturating_sub(1) + self.seed as usize) % self.style.palettes.len();
            let pi = self.look(sec - 1, pal_prev);
            // `look` may have evicted the current look: find it again.
            let li = self.look(sec, palette);
            let pnorm = self.looks[pi].norm;
            let mut prev = std::mem::take(&mut self.prev);
            self.looks[pi].renderer.render(t_ms, &mut prev);
            let f = (t_ms - start) as f32 / SECTION_FADE_MS as f32;
            let cur = self.looks[li].norm;
            for (b, p) in buf.iter_mut().zip(prev.iter()) {
                let v = *b as f32 * cur * f + *p as f32 * pnorm * (1.0 - f);
                *b = (v / norm).round().min(255.0) as u8;
            }
            self.prev = prev;
        }

        let level = a.section_at(t_ms).map_or(Level::Mid, |s| s.level);
        let energy = if a.energy_10hz.rms.is_empty() {
            0.7
        } else {
            a.energy_at(t_ms)
        };
        let voice = self.style.id == "voice";
        let env = crate::audio_analysis::envelope(&self.pulse_beats, t_ms, BEAT_TAU_MS);
        let (base, steady) = match level {
            Level::Low => (0.55, 0.55),
            Level::Mid => (0.4, 0.75),
            Level::High => (0.3, 0.9),
        };
        let depth = ((1.0 - base) * self.style.punch).clamp(0.0, 0.85);
        let flash = self
            .flashes
            .binary_search_by(|&f| {
                if u64::from(f) > t_ms {
                    std::cmp::Ordering::Greater
                } else if t_ms - u64::from(f) < FLASH_MS {
                    std::cmp::Ordering::Equal
                } else {
                    std::cmp::Ordering::Less
                }
            })
            .is_ok();
        for (i, r) in self.ranges.iter().enumerate() {
            let px = &mut buf[r.clone()];
            let gain = if voice {
                // Voice: brightness follows the loudness closely.
                0.08 + 0.92 * energy.powf(1.4)
            } else if self.beat_group[i] {
                norm * ((1.0 - depth) + depth * env) * (0.55 + 0.45 * energy)
            } else {
                norm * steady * (0.6 + 0.4 * energy)
            };
            for v in px.iter_mut() {
                *v = (*v as f32 * gain).round().min(255.0) as u8;
            }
            if self.small[i] && !voice {
                self.sparkle(i, t_ms, px);
            }
            if flash {
                for c in px.chunks_exact_mut(3) {
                    c.copy_from_slice(&[255, 226, 190]);
                }
            }
        }
        for (i, p) in self.props.iter().enumerate() {
            crate::mapping::write_prop_channels(p, &buf[self.ranges[i].clone()], out);
        }
        self.buf = buf;
    }

    /// White sparkles for 150 ms after a high-band hit on small props.
    fn sparkle(&self, prop: usize, t_ms: u64, px: &mut [u8]) {
        let i = self
            .sparkles
            .partition_point(|&(ms, _)| u64::from(ms) <= t_ms);
        let Some(&(ms, strength)) = i.checked_sub(1).and_then(|i| self.sparkles.get(i)) else {
            return;
        };
        let dt = t_ms - u64::from(ms);
        if dt >= 150 {
            return;
        }
        let fade = 1.0 - dt as f32 / 150.0;
        let density = 0.12 * strength;
        let key = fnv1a(format!("{}:{ms}:{prop}", self.seed).as_bytes());
        for (k, c) in px.chunks_exact_mut(3).enumerate() {
            let mut r = SplitMix(key ^ (k as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
            if r.unit() < density {
                for v in c.iter_mut() {
                    *v = (*v as f32 + (255.0 - *v as f32) * fade).round() as u8;
                }
            }
        }
    }
}

fn level_num(l: Level) -> u8 {
    match l {
        Level::Low => 0,
        Level::Mid => 1,
        Level::High => 2,
    }
}

/// An effect preset for a look, speeds scaled to the tempo and energy level.
fn preset(kind: EffectKind, pal: &[&str], level: u8, bpm: f32, id: &str) -> EffectPreset {
    let beat_hz = f64::from(bpm) / 60.0;
    let lv = f64::from(level);
    let colors: Vec<Value> = pal.iter().map(|c| json!(c)).collect();
    let first = pal.first().copied().unwrap_or("#ffffff");
    let params = match kind {
        K::Solid => json!({ "color": first }),
        K::Twinkle => {
            json!({ "colors": colors, "density": 0.35 + 0.1 * lv, "speed": 0.6 + 0.3 * lv, "glow": 0.25 })
        }
        K::Colorwash => json!({ "colors": colors, "speed": 0.03 + 0.03 * lv, "spread": 0.6 }),
        K::Breathe => {
            json!({ "colors": colors, "period": (4.0 / beat_hz).clamp(1.5, 8.0), "minBrightness": 0.15 })
        }
        K::Wave => {
            json!({ "colors": colors, "speed": (beat_hz / (4.0 - lv)).clamp(0.05, 2.0), "wavelength": 0.6, "direction": "right", "mode": "across" })
        }
        K::Chase => {
            json!({ "colors": colors, "speed": (beat_hz * (3.0 + 3.0 * lv)).clamp(1.0, 40.0), "size": 3 + level as u32, "gap": 3, "fade": true })
        }
        K::Candycane => {
            json!({ "colors": colors, "stripeWidth": 4, "speed": (beat_hz * (1.5 + 2.0 * lv)).clamp(0.5, 25.0) })
        }
        K::Meteor => {
            json!({ "colors": colors, "speed": (beat_hz * 20.0).clamp(10.0, 120.0), "tailLength": 12, "count": 2, "sparkleTail": true })
        }
        K::Sparkle => {
            json!({ "colors": [pal.get(1).copied().unwrap_or("#0a1a4a")], "sparkleColor": first, "density": 0.12, "speed": 1.5 })
        }
        K::Rainbow => json!({ "speed": 0.1 + 0.2 * lv, "spread": 1.0, "mode": "across" }),
        // A gentle, beat-locked strobe: at most 3 Hz, short duty, never red.
        K::Strobe => {
            json!({ "color": "#ffffff", "rate": (beat_hz).clamp(0.5, 3.0), "duty": 0.12, "pattern": "alternate" })
        }
        _ => json!({ "colors": colors }),
    };
    let params: crate::model::EffectParams = match params {
        Value::Object(m) => m
            .into_iter()
            .chain([("brightness".to_string(), json!(100))])
            .collect(),
        _ => Default::default(),
    };
    EffectPreset {
        id: id.into(),
        name: "Auto".into(),
        effect: kind,
        params,
        target: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_analysis::{analyze, tests::click_track};
    use crate::model::PropLayout;

    fn prop(id: &str, kind: PropKind, n: u32, start: u32, x: f32) -> Prop {
        let mut p: Prop = serde_json::from_value(json!({
            "id": id, "name": id, "kind": kind, "pixelCount": n,
            "channelStart": start, "channelsPerPixel": 3, "segments": [], "groupIds": []
        }))
        .unwrap();
        p.layout = Some(PropLayout {
            x,
            y: 0.0,
            w: 60.0,
            h: 40.0,
            rotation: 0.0,
            points: None,
            source: None,
        });
        p
    }

    fn show() -> Show {
        Show {
            props: vec![
                prop("tree", PropKind::Tree, 200, 0, 0.0),
                prop("arch", PropKind::Arch, 50, 600, 80.0),
                prop("cane1", PropKind::Candycane, 20, 750, 160.0),
                prop("cane2", PropKind::Candycane, 20, 810, 200.0),
                prop("win", PropKind::Window, 30, 870, 240.0),
            ],
            ..Show::default()
        }
    }

    fn song() -> Analysis {
        // Quiet 12 s, loud 12 s (sections), 128 BPM with kicks.
        let rate = 22_050;
        let (mut s, _) = click_track(128.0, 36.0, rate, 0.0, 0.01, 0, 5);
        for (i, v) in s.iter_mut().enumerate() {
            let t = i as f32 / rate as f32;
            if (12.0..24.0).contains(&t) {
                *v *= 4.0;
            } else {
                *v *= 0.4;
            }
        }
        analyze(&s, rate)
    }

    fn hash_frames(req: &AutoShowRequest, every: u32) -> (String, Vec<Vec<u8>>) {
        let mut c = Choreographer::new(req).unwrap();
        let mut h = sha2::Sha256::new();
        let mut kept = vec![];
        let mut f = vec![0u8; c.channel_count as usize];
        for i in 0..c.frame_count {
            c.frame(u64::from(i) * 25, &mut f);
            h.update(&f);
            if i % every == 0 {
                kept.push(f.clone());
            }
        }
        (crate::fseq::to_hex(&h.finalize()), kept)
    }

    #[test]
    fn deterministic_and_seed_dependent() {
        let s = show();
        let a = song();
        let req = |style: &'static str, seed: u32| AutoShowRequest {
            show: &s,
            prop_ids: &[],
            analysis: &a,
            style,
            seed,
            duration_ms: 0,
            media_filename: None,
        };
        let (h1, _) = hash_frames(&req("classic", 7), 1000);
        let (h2, _) = hash_frames(&req("classic", 7), 1000);
        assert_eq!(h1, h2);
        let (h3, _) = hash_frames(&req("classic", 8), 1000);
        assert_ne!(h1, h3);
        for st in styles() {
            let (h, frames) = hash_frames(&req(Box::leak(st.id.clone().into_boxed_str()), 1), 40);
            assert_eq!(h.len(), 64);
            assert!(
                frames.iter().any(|f| f.iter().any(|&b| b > 0)),
                "{} lights something",
                st.id
            );
        }
        assert!(matches!(
            Choreographer::new(&req("nope", 1)),
            Err(AutoShowError::UnknownStyle(_))
        ));
    }

    #[test]
    fn writes_a_playable_fseq_and_is_byte_identical() {
        let s = show();
        let a = song();
        let dir = std::env::temp_dir().join(format!("pp-auto-{}", crate::model::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let req = AutoShowRequest {
            show: &s,
            prop_ids: &[],
            analysis: &a,
            style: "party",
            seed: 3,
            duration_ms: 36_000,
            media_filename: Some("Song.mp3".into()),
        };
        let o1 = render(&req, &dir.join("a.fseq"), &mut |_| true).unwrap();
        let o2 = render(&req, &dir.join("b.fseq"), &mut |_| true).unwrap();
        assert_eq!(o1, o2);
        assert_eq!(o1.frame_count, 1440);
        assert_eq!(o1.channel_count, 960);
        let h1 = crate::fseq::sha256_file(dir.join("a.fseq")).unwrap();
        assert_eq!(h1, crate::fseq::sha256_file(dir.join("b.fseq")).unwrap());
        let f = crate::fseq::FseqFile::open(dir.join("a.fseq")).unwrap();
        assert_eq!(f.frame_ms(), 25);
        assert_eq!(f.frame_count(), 1440);
        assert_eq!(f.media_filename().as_deref(), Some("Song.mp3"));
        // Cancel leaves nothing behind.
        let r = render(&req, &dir.join("c.fseq"), &mut |p| p < 0.2);
        assert!(matches!(r, Err(AutoShowError::Cancelled)));
        assert!(!dir.join("c.fseq").exists() && !dir.join("c.fseq.tmp").exists());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn at_most_three_full_flashes_per_second_and_never_red() {
        let s = show();
        let a = song();
        for style in ["classic", "rock", "party", "candy"] {
            let req = AutoShowRequest {
                show: &s,
                prop_ids: &[],
                analysis: &a,
                style,
                seed: 1,
                duration_ms: 0,
                media_filename: None,
            };
            let mut c = Choreographer::new(&req).unwrap();
            let mut f = vec![0u8; c.channel_count as usize];
            // A "full flash" frame: every lit prop pixel bright (≥ 200 on all
            // channels) across the whole display.
            let mut starts: Vec<u64> = vec![];
            let mut was = false;
            for i in 0..c.frame_count {
                let t = u64::from(i) * 25;
                c.frame(t, &mut f);
                let pixels: Vec<&[u8]> = s
                    .props
                    .iter()
                    .flat_map(|p| {
                        let a = p.channel_start as usize;
                        f[a..a + p.pixel_count as usize * 3].chunks_exact(3)
                    })
                    .collect();
                let full = pixels.iter().all(|c| c.iter().all(|&v| v >= 200));
                let red = pixels
                    .iter()
                    .filter(|c| c[0] > 200 && c[1] < 40 && c[2] < 40)
                    .count();
                assert!(
                    !(red == pixels.len() && full),
                    "{style}: full-field red at {t}"
                );
                if full && !was {
                    starts.push(t);
                }
                was = full;
            }
            for (k, &t) in starts.iter().enumerate() {
                let in_sec = starts[k..].iter().take_while(|&&x| x < t + 1000).count();
                assert!(
                    in_sec <= 3,
                    "{style}: {in_sec} flashes in the second after {t}"
                );
            }
        }
    }

    #[test]
    fn dense_kicks_still_flash_at_most_three_times_a_second() {
        use crate::audio_analysis::{Energy, Onset, Section};
        let s = show();
        // 10 s, a loud section, very strong kicks every 100 ms (10 Hz).
        let onsets: Vec<Onset> = (0..100)
            .map(|i| Onset {
                ms: i * 100,
                strength: 1.0,
                band: 0,
            })
            .collect();
        let a = Analysis {
            v: 1,
            sr: 22_050,
            hop_ms: 11.61,
            duration_ms: 10_000,
            bpm: 150.0,
            bpm_confidence: 1.0,
            tempo_curve: vec![150.0],
            beats: (0..25).map(|i| i * 400).collect(),
            downbeats: (0..7).map(|i| i * 1600).collect(),
            downbeat_confidence: 1.0,
            onsets,
            energy_10hz: Energy {
                rms: vec![255; 100],
                low: vec![255; 100],
                mid: vec![200; 100],
                high: vec![100; 100],
            },
            sections: vec![Section {
                start_ms: 0,
                end_ms: 10_000,
                level: Level::High,
            }],
        };
        let req = AutoShowRequest {
            show: &s,
            prop_ids: &[],
            analysis: &a,
            style: "rock",
            seed: 1,
            duration_ms: 0,
            media_filename: None,
        };
        let mut c = Choreographer::new(&req).unwrap();
        assert!(c.flashes.len() >= 20, "{:?}", c.flashes);
        for w in c.flashes.windows(4) {
            assert!(w[3] - w[0] >= 1000, "4 flashes within a second: {w:?}");
        }
        // And the rendered frames agree: count white-out frame runs.
        let mut f = vec![0u8; c.channel_count as usize];
        let mut starts = vec![];
        let mut was = false;
        for i in 0..c.frame_count {
            c.frame(u64::from(i) * 25, &mut f);
            let full = s.props.iter().all(|p| {
                let a = p.channel_start as usize;
                f[a..a + p.pixel_count as usize * 3]
                    == [255, 226, 190].repeat(p.pixel_count as usize)[..]
            });
            if full && !was {
                starts.push(u64::from(i) * 25);
            }
            was = full;
        }
        assert!(starts.len() >= 20);
        for w in starts.windows(4) {
            assert!(w[3] - w[0] >= 1000, "{w:?}");
        }
    }

    #[test]
    fn beats_pulse_the_big_props_and_loud_parts_are_brighter() {
        let s = show();
        let a = song();
        let req = AutoShowRequest {
            show: &s,
            prop_ids: &[],
            analysis: &a,
            style: "calm",
            seed: 2,
            duration_ms: 0,
            media_filename: None,
        };
        let mut c = Choreographer::new(&req).unwrap();
        assert!(c.beat_group[0] && c.beat_group[1] && !c.beat_group[2]);
        let mut f = vec![0u8; c.channel_count as usize];
        let tree_sum = |f: &[u8]| f[..600].iter().map(|&v| u64::from(v)).sum::<u64>();
        let mut on = 0u64;
        let mut off = 0u64;
        let mut quiet = 0u64;
        let mut loud = 0u64;
        for &b in a.beats.iter().filter(|&&b| b > 13_000 && b < 23_000) {
            c.frame(u64::from(b), &mut f);
            on += tree_sum(&f);
            c.frame(u64::from(b) + 200, &mut f);
            off += tree_sum(&f);
        }
        assert!(on > off, "beats pulse the tree: {on} vs {off}");
        let all = |f: &[u8]| f.iter().map(|&v| u64::from(v)).sum::<u64>();
        for t in (1000..11_000).step_by(250) {
            c.frame(t, &mut f);
            quiet += all(&f);
            c.frame(t + 12_500, &mut f);
            loud += all(&f);
        }
        assert!(loud > quiet, "{loud} vs {quiet}");
    }

    #[test]
    fn only_selected_props_light_and_hash_tracks_layout() {
        let s = show();
        let a = song();
        let ids = vec!["arch".to_string()];
        let req = AutoShowRequest {
            show: &s,
            prop_ids: &ids,
            analysis: &a,
            style: "rock",
            seed: 1,
            duration_ms: 5_000,
            media_filename: None,
        };
        let mut c = Choreographer::new(&req).unwrap();
        let mut f = vec![0u8; c.channel_count as usize];
        let mut lit_arch = false;
        for i in 0..200 {
            c.frame(i * 25, &mut f);
            assert!(f[..600].iter().all(|&v| v == 0), "tree stays dark");
            assert!(f[750..].iter().all(|&v| v == 0), "others stay dark");
            lit_arch |= f[600..750].iter().any(|&v| v > 0);
        }
        assert!(lit_arch);
        let h = props_hash(&s, &ids);
        assert_eq!(h, props_hash(&s, &ids));
        let mut s2 = s.clone();
        s2.props[1].pixel_count = 51;
        assert_ne!(h, props_hash(&s2, &ids));
        let mut s3 = s.clone();
        s3.props[0].name = "Big tree".into();
        assert_eq!(h, props_hash(&s3, &ids), "names don't matter");
        let empty = Show::default();
        let req = AutoShowRequest {
            show: &empty,
            ..req
        };
        assert!(matches!(
            Choreographer::new(&req),
            Err(AutoShowError::NoProps)
        ));
    }

    /// `cargo test --release -p pixelplus-core perf_budget -- --ignored --nocapture`
    /// Times the F2/F3 CPU work for a 4-minute song on a ~2,000-pixel layout.
    #[test]
    #[ignore]
    fn perf_budget() {
        use std::time::Instant;
        let rate = 44_100;
        let (s, _) = click_track(124.0, 240.0, rate, 0.01, 0.05, 0, 3);
        let t = Instant::now();
        let a = analyze(&s, rate);
        let t_an = t.elapsed();
        let mut show = Show::default();
        let mut start = 0;
        for i in 0..20 {
            let kind = [
                PropKind::Tree,
                PropKind::Arch,
                PropKind::Line,
                PropKind::Candycane,
            ][i % 4];
            show.props
                .push(prop(&format!("p{i}"), kind, 100, start, i as f32 * 70.0));
            start += 300;
        }
        let dir = std::env::temp_dir().join(format!("pp-perf-{}", crate::model::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let req = AutoShowRequest {
            show: &show,
            prop_ids: &[],
            analysis: &a,
            style: "classic",
            seed: 1,
            duration_ms: 240_000,
            media_filename: None,
        };
        let t = Instant::now();
        render(&req, &dir.join("a.fseq"), &mut |_| true).unwrap();
        let t_auto = t.elapsed();
        let t = Instant::now();
        let h = crate::preview::build(
            &dir.join("a.fseq"),
            "a",
            "h",
            &show.props,
            &dir.join("a.pppv"),
            &mut |_| true,
        )
        .unwrap();
        let t_pv = t.elapsed();
        let size = std::fs::metadata(dir.join("a.pppv")).unwrap().len();
        let json = serde_json::to_string(&a).unwrap().len();
        eprintln!(
            "analysis {:?} ({} beats, {} B json) | autoshow {:?} | preview {:?} ({} frames, {} KB)",
            t_an,
            a.beats.len(),
            json,
            t_auto,
            t_pv,
            h.frame_count,
            size / 1024
        );
        std::fs::remove_dir_all(dir).ok();
    }
}
