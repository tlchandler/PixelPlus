//! Camera mapping codes (F6/F7/F9, owned by WS4): the deterministic light
//! pattern a phone camera decodes to find which output / pixel is where.
//!
//! # Contract for the engine (WS3 wiring of test modes `mapCode` / `identify`)
//!
//! Everything here is a **pure function of the plan and the timeline
//! position**, so the leader and every follower light exactly the same bits
//! without talking to each other during the run.
//!
//! * `mapCode`: for every output of this node call
//!   [`render_output`]`(plan, node_id, output_1based, pos_ms, rgb)`. It fills
//!   the output's RGB (pre-colour-order) bytes and returns `true` when the
//!   output is a target; non-target outputs of nodes in the plan should be
//!   dark (`false` → the engine clears them). `pos_ms` is the test's elapsed
//!   time on the shared timeline (the same `t` `TestLayer` uses today);
//!   [`MapPlan::start_pos_ms`] is subtracted, so `0` is the plan start.
//!   Once [`frame_for`] returns [`MapSlot::Done`] the pattern is dark and the
//!   engine may end the test (the daemon also stops it via `/test/stop`).
//! * `identify` (F9 receiver wizard): [`render_identify`]`(rgb_color,
//!   blinks, elapsed_ms, out)` fills a whole output. Colours are
//!   **post-colour-order** in the spec sense: the wizard only uses white for
//!   jack identification (blink counts), so no reorder is needed.
//!
//! # Timeline of one run (all durations are multiples of `bit_ms`, one *slot*)
//!
//! ```text
//! lead-in  : LEAD_IN_SLOTS dark                       (phone sees a clean start)
//! pass × passes:
//!   preamble : PREAMBLE (1 1 1 0 0 0 1 1 1 0 0 0)     all target pixels at `level`
//!   phase A  : code_bits slots (if phases & PHASE_A)  bit i of the target's codeword
//!   phase B  : 2 × pixel_bits slots (if PHASE_B)      Gray-code bit j of the pixel
//!                                                     index, then its complement
//!   gap      : GAP_SLOTS dark
//! done     : dark forever
//! ```
//!
//! * **Phase A codewords** are constant-weight words with pairwise Hamming
//!   distance ≥ 4 ([`codebook`]): 12 bits weight 6 (the 132 hexads of the
//!   Steiner system S(5,6,12), sorted ascending) for up to 132 targets, else
//!   16 bits weight 8 (greedy ascending, 870 words). Target `k` (its index
//!   in [`MapPlan::targets`]) uses `codebook(bits)[k]`; slot `i` of phase A
//!   shows bit `bits-1-i` (most significant bit first). Constant weight lets
//!   the decoder take the top-`w` soft values with no global threshold.
//! * **Phase B**: slot `2j` shows bit `pixel_bits-1-j` of `gray(p)` for pixel
//!   `p` (MSB first) and slot `2j+1` shows its complement, so each bit is
//!   self-thresholding (compare the two slots).
//! * **Count probe** (F7 manual search, [`MapPlan::count_probe`] = `Some(k)`):
//!   no timeline; pixels `0..k` are dim green and pixel `k` is red while
//!   `k < max_pixels`. Used to ask "is there a red pixel at the end?".
//! * `seed` identifies the run; the pattern itself does not depend on it (a
//!   stale camera recording decodes to the same labels, which is harmless
//!   because the phone only decodes its own recording).
//!
//! Test vectors shared with the browser decoder live in
//! `crates/pixelplus-core/tests/fixtures/mapcode/vectors.json` (regenerate
//! with `MAPCODE_BLESS=1 cargo test -p pixelplus-core mapcode`); the TypeScript
//! mirror is `web/src/lib/cv/mapcode.ts`.

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// What the leader shows for one mapping run. Every node renders its own
/// outputs from this plan and the timeline position, so all nodes agree bit
/// for bit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MapPlan {
    pub seed: u32,
    /// Duration of one code bit (ms).
    #[serde(default = "default_bit_ms")]
    pub bit_ms: u32,
    /// "On" level, 0..255 (≤ 50 % enforced by the leader).
    #[serde(default = "default_level")]
    pub level: u8,
    #[serde(default = "default_passes")]
    pub passes: u8,
    /// bit 0 = phase A (identify targets), bit 1 = phase B (pixel positions).
    #[serde(default = "default_phases")]
    pub phases: u8,
    /// Ordered targets; the index is the code index.
    #[serde(default)]
    pub targets: Vec<MapTarget>,
    /// ceil(log2(max pixels)).
    #[serde(default)]
    pub pixel_bits: u8,
    /// Timeline-relative start (ms).
    #[serde(default)]
    pub start_pos_ms: u64,
    /// F7 manual pixel-count probe: a static frame instead of the code
    /// timeline (pixels `0..k` dim green, pixel `k` red).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count_probe: Option<u32>,
}

/// One node output in a mapping run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct MapTarget {
    pub node_id: String,
    /// 1-based.
    pub output: u32,
    /// Pixels to light (may exceed the configured count when probing, F7).
    #[serde(default)]
    pub max_pixels: u32,
}

pub const PHASE_A: u8 = 1;
pub const PHASE_B: u8 = 2;

/// Dark slots before the first pass.
pub const LEAD_IN_SLOTS: u32 = 5;
/// Dark slots after each pass.
pub const GAP_SLOTS: u32 = 3;
/// Preamble slot values (1 = all target pixels on).
pub const PREAMBLE: [bool; 12] = [
    true, true, true, false, false, false, true, true, true, false, false, false,
];
/// Highest `level` the leader accepts (50 %; spec F6 "Security").
pub const MAX_LEVEL: u8 = 127;
/// Shortest bit the decoder can handle at 15 fps (≥ 3 frames per bit).
pub const MIN_BIT_MS: u32 = 120;
pub const MAX_BIT_MS: u32 = 1000;
/// Most pixels one target can have (Gray code of 12 bits).
pub const MAX_PIXEL_BITS: u8 = 12;
/// Colours of the count probe (F7 manual search).
pub const PROBE_RUN: [u8; 3] = [0, 40, 0];
pub const PROBE_END: [u8; 3] = [200, 0, 0];

fn default_bit_ms() -> u32 {
    200
}
fn default_level() -> u8 {
    77
}
fn default_passes() -> u8 {
    3
}
fn default_phases() -> u8 {
    PHASE_A | PHASE_B
}

impl Default for MapPlan {
    fn default() -> Self {
        MapPlan {
            seed: 0,
            bit_ms: default_bit_ms(),
            level: default_level(),
            passes: default_passes(),
            phases: default_phases(),
            targets: vec![],
            pixel_bits: 0,
            start_pos_ms: 0,
            count_probe: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Codes
// ---------------------------------------------------------------------------

/// Binary-reflected Gray code.
pub fn gray(n: u32) -> u32 {
    n ^ (n >> 1)
}

/// Inverse of [`gray`].
pub fn gray_inverse(mut g: u32) -> u32 {
    let mut n = g;
    while g > 1 {
        g >>= 1;
        n ^= g;
    }
    n
}

/// Bits needed to address `max_pixels` pixels (`ceil(log2(n))`, at least 1).
pub fn pixel_bits_for(max_pixels: u32) -> u8 {
    if max_pixels <= 2 {
        1
    } else {
        (u32::BITS - (max_pixels - 1).leading_zeros()) as u8
    }
}

/// Codeword length for `n` targets: 12 (weight 6) while the 12-bit book is big
/// enough, else 16 (weight 8). `0` when there are more targets than codes.
pub fn code_bits_for(n_targets: usize) -> u8 {
    if n_targets <= codebook(12).len() {
        12
    } else if n_targets <= codebook(16).len() {
        16
    } else {
        0
    }
}

/// Weight of the words in the `bits`-bit book.
pub fn code_weight(bits: u8) -> u32 {
    bits as u32 / 2
}

/// Generator of the extended ternary Golay code `[I₆ | A]` (rows of `A`).
const GOLAY_A: [[u8; 6]; 6] = [
    [0, 1, 1, 1, 1, 1],
    [1, 0, 1, 2, 2, 1],
    [1, 1, 0, 1, 2, 2],
    [1, 2, 1, 0, 1, 2],
    [1, 2, 2, 1, 0, 1],
    [1, 1, 2, 2, 1, 0],
];

/// 12 bits: the 132 hexads of the Steiner system S(5,6,12) (supports of the
/// weight-6 words of the extended ternary Golay code; any two share at most
/// 4 points, so distance ≥ 4 — the optimum A(12,4,6) = 132). Sorted ascending;
/// bit `11 - i` of a word is coordinate `i`.
fn hexads() -> Vec<u16> {
    let mut out = Vec::new();
    for m in 0..729u32 {
        let msg: Vec<u8> = (0..6).map(|i| ((m / 3u32.pow(i)) % 3) as u8).collect();
        let mut word = [0u8; 12];
        word[..6].copy_from_slice(&msg);
        for (j, w) in word[6..].iter_mut().enumerate() {
            *w = ((0..6)
                .map(|i| msg[i] as u32 * GOLAY_A[i][j] as u32)
                .sum::<u32>()
                % 3) as u8;
        }
        if word.iter().filter(|&&x| x != 0).count() == 6 {
            let bits = word
                .iter()
                .enumerate()
                .fold(0u16, |acc, (i, &x)| acc | ((x != 0) as u16) << (11 - i));
            out.push(bits);
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// 16 bits: greedy (ascending) weight-8 words with pairwise distance ≥ 4.
fn greedy_book(bits: u32) -> Vec<u16> {
    let w = bits / 2;
    let mut book: Vec<u16> = Vec::new();
    for x in 0u32..(1 << bits) {
        if x.count_ones() == w && book.iter().all(|&c| (c as u32 ^ x).count_ones() >= 4) {
            book.push(x as u16);
        }
    }
    book
}

/// The constant-weight codebook for `bits` = 12 or 16 (other values: empty).
/// Deterministic and mirrored exactly in TypeScript (`web/src/lib/cv/mapcode.ts`).
pub fn codebook(bits: u8) -> &'static [u16] {
    static B12: OnceLock<Vec<u16>> = OnceLock::new();
    static B16: OnceLock<Vec<u16>> = OnceLock::new();
    match bits {
        12 => B12.get_or_init(hexads),
        16 => B16.get_or_init(|| greedy_book(16)),
        _ => &[],
    }
}

/// Most targets one run can identify.
pub fn max_targets() -> usize {
    codebook(16).len()
}

// ---------------------------------------------------------------------------
// Schedule
// ---------------------------------------------------------------------------

/// Durations of a run (ms), as sent to the phone.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    pub bit_ms: u32,
    pub lead_in_ms: u64,
    pub preamble_ms: u64,
    /// JSON `phaseAms` (the web contract's spelling).
    #[serde(rename = "phaseAms")]
    pub phase_a_ms: u64,
    #[serde(rename = "phaseBms")]
    pub phase_b_ms: u64,
    pub gap_ms: u64,
    pub pass_ms: u64,
    pub total_ms: u64,
    /// Phase A codeword length (0 without phase A).
    pub code_bits: u8,
    pub pixel_bits: u8,
    pub passes: u8,
}

/// Slot counts of one pass: (preamble, phase A, phase B, gap).
fn pass_slots(plan: &MapPlan) -> (u32, u32, u32, u32) {
    let a = if plan.phases & PHASE_A != 0 {
        code_bits_for(plan.targets.len()) as u32
    } else {
        0
    };
    let b = if plan.phases & PHASE_B != 0 {
        2 * plan.pixel_bits as u32
    } else {
        0
    };
    (PREAMBLE.len() as u32, a, b, GAP_SLOTS)
}

/// The run's durations.
pub fn schedule(plan: &MapPlan) -> Schedule {
    let bit = plan.bit_ms.max(1) as u64;
    let (p, a, b, g) = pass_slots(plan);
    let pass = (p + a + b + g) as u64 * bit;
    Schedule {
        bit_ms: plan.bit_ms,
        lead_in_ms: LEAD_IN_SLOTS as u64 * bit,
        preamble_ms: p as u64 * bit,
        phase_a_ms: a as u64 * bit,
        phase_b_ms: b as u64 * bit,
        gap_ms: g as u64 * bit,
        pass_ms: pass,
        total_ms: LEAD_IN_SLOTS as u64 * bit + pass * plan.passes as u64,
        code_bits: if a > 0 { a as u8 } else { 0 },
        pixel_bits: if b > 0 { plan.pixel_bits } else { 0 },
        passes: plan.passes,
    }
}

/// What a slot shows.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum MapSlot {
    /// Dark before the first pass.
    LeadIn,
    /// All target pixels `on` or dark.
    Preamble { index: u8, on: bool },
    /// Codeword bit `index` (0 = first sent = MSB).
    PhaseA { index: u8 },
    /// Gray bit `index` (0 = first sent = MSB); `inverted` = the complement slot.
    PhaseB { index: u8, inverted: bool },
    /// Dark between passes.
    Gap,
    /// The run is over (dark).
    Done,
    /// Count probe (static, [`MapPlan::count_probe`]).
    Probe,
}

/// The frame at one timeline position.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MapFrame {
    /// 0-based pass (0 during lead-in and after the end).
    pub pass: u8,
    pub slot: MapSlot,
}

/// The deterministic frame function: what the plan shows at `pos_ms`.
pub fn frame_for(plan: &MapPlan, pos_ms: u64) -> MapFrame {
    if plan.count_probe.is_some() {
        return MapFrame {
            pass: 0,
            slot: MapSlot::Probe,
        };
    }
    let bit = plan.bit_ms.max(1) as u64;
    let slot = pos_ms.saturating_sub(plan.start_pos_ms) / bit;
    if slot < LEAD_IN_SLOTS as u64 {
        return MapFrame {
            pass: 0,
            slot: MapSlot::LeadIn,
        };
    }
    let (p, a, b, g) = pass_slots(plan);
    let per_pass = (p + a + b + g) as u64;
    let s = slot - LEAD_IN_SLOTS as u64;
    let pass = s / per_pass;
    if pass >= plan.passes as u64 {
        return MapFrame {
            pass: 0,
            slot: MapSlot::Done,
        };
    }
    let mut i = (s % per_pass) as u32;
    let kind = if i < p {
        MapSlot::Preamble {
            index: i as u8,
            on: PREAMBLE[i as usize],
        }
    } else {
        i -= p;
        if i < a {
            MapSlot::PhaseA { index: i as u8 }
        } else {
            i -= a;
            if i < b {
                MapSlot::PhaseB {
                    index: (i / 2) as u8,
                    inverted: i % 2 == 1,
                }
            } else {
                MapSlot::Gap
            }
        }
    };
    MapFrame {
        pass: pass as u8,
        slot: kind,
    }
}

/// Codeword of target `k` (0 without phase A or out of range).
pub fn codeword(plan: &MapPlan, k: usize) -> u16 {
    let bits = code_bits_for(plan.targets.len());
    codebook(bits).get(k).copied().unwrap_or(0)
}

/// Whether pixel `pixel` of target `k` is on in `frame` (ignores the count
/// probe; see [`render_target`]).
pub fn pixel_on(plan: &MapPlan, frame: &MapFrame, k: usize, pixel: u32) -> bool {
    let Some(t) = plan.targets.get(k) else {
        return false;
    };
    if pixel >= t.max_pixels {
        return false;
    }
    match frame.slot {
        MapSlot::Preamble { on, .. } => on,
        MapSlot::PhaseA { index } => {
            let bits = code_bits_for(plan.targets.len());
            let word = codeword(plan, k);
            bits > index && (word >> (bits - 1 - index)) & 1 == 1
        }
        MapSlot::PhaseB { index, inverted } => {
            if index >= plan.pixel_bits {
                return false;
            }
            let bit = (gray(pixel) >> (plan.pixel_bits - 1 - index)) & 1 == 1;
            bit != inverted
        }
        MapSlot::LeadIn | MapSlot::Gap | MapSlot::Done | MapSlot::Probe => false,
    }
}

/// Level (0 or `plan.level`) of pixel `pixel` of target `k` at `pos_ms`.
pub fn level_at(plan: &MapPlan, k: usize, pixel: u32, pos_ms: u64) -> u8 {
    if pixel_on(plan, &frame_for(plan, pos_ms), k, pixel) {
        plan.level
    } else {
        0
    }
}

/// Index of the target driven by `node_id` output `output` (1-based).
pub fn target_index(plan: &MapPlan, node_id: &str, output: u32) -> Option<usize> {
    plan.targets
        .iter()
        .position(|t| t.node_id == node_id && t.output == output)
}

/// Paint target `k` into `out` (RGB triples, pixel order along the output).
/// The whole slice is written: pixels outside the pattern are dark.
pub fn render_target(plan: &MapPlan, k: usize, pos_ms: u64, out: &mut [u8]) {
    out.fill(0);
    let Some(t) = plan.targets.get(k) else {
        return;
    };
    let n = (t.max_pixels as usize).min(out.len() / 3);
    if let Some(end) = plan.count_probe {
        for (i, px) in out.chunks_exact_mut(3).take(n).enumerate() {
            let i = i as u32;
            if i < end {
                px.copy_from_slice(&PROBE_RUN);
            } else if i == end {
                px.copy_from_slice(&PROBE_END);
                break;
            }
        }
        return;
    }
    let frame = frame_for(plan, pos_ms);
    let l = plan.level;
    for (i, px) in out.chunks_exact_mut(3).take(n).enumerate() {
        if pixel_on(plan, &frame, k, i as u32) {
            px.copy_from_slice(&[l, l, l]);
        }
    }
}

/// Paint `node_id`'s output `output` (1-based) into `out`. Returns `false`
/// (and leaves `out` alone) when that output is not a target.
pub fn render_output(
    plan: &MapPlan,
    node_id: &str,
    output: u32,
    pos_ms: u64,
    out: &mut [u8],
) -> bool {
    match target_index(plan, node_id, output) {
        Some(k) => {
            render_target(plan, k, pos_ms, out);
            true
        }
        None => false,
    }
}

/// Whether the plan's pattern is over at `pos_ms` (never for a count probe).
pub fn is_done(plan: &MapPlan, pos_ms: u64) -> bool {
    frame_for(plan, pos_ms).slot == MapSlot::Done
}

/// Check a plan before running it.
pub fn validate(plan: &MapPlan) -> Result<(), String> {
    if plan.targets.is_empty() {
        return Err("Nothing to map: pick at least one output.".into());
    }
    if !(MIN_BIT_MS..=MAX_BIT_MS).contains(&plan.bit_ms) {
        return Err(format!(
            "Bit length must be {MIN_BIT_MS}–{MAX_BIT_MS} ms (got {}).",
            plan.bit_ms
        ));
    }
    if plan.level == 0 || plan.level > MAX_LEVEL {
        return Err(format!("Level must be 1–{MAX_LEVEL} (50 %)."));
    }
    if plan.count_probe.is_none() {
        if plan.passes == 0 || plan.passes > 5 {
            return Err("Passes must be 1–5.".into());
        }
        if plan.phases & (PHASE_A | PHASE_B) == 0 {
            return Err("Pick phase A, phase B or both.".into());
        }
        if plan.phases & PHASE_A != 0 && code_bits_for(plan.targets.len()) == 0 {
            return Err(format!(
                "Too many outputs for one run ({}; at most {}).",
                plan.targets.len(),
                max_targets()
            ));
        }
        if plan.phases & PHASE_B != 0 {
            if plan.pixel_bits == 0 || plan.pixel_bits > MAX_PIXEL_BITS {
                return Err(format!("pixelBits must be 1–{MAX_PIXEL_BITS}."));
            }
            let most = plan.targets.iter().map(|t| t.max_pixels).max().unwrap_or(0);
            if most > 1 << plan.pixel_bits {
                return Err(format!(
                    "pixelBits {} can't address {most} pixels.",
                    plan.pixel_bits
                ));
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    for t in &plan.targets {
        if t.output == 0 {
            return Err("Outputs are numbered from 1.".into());
        }
        if !seen.insert((&t.node_id, t.output)) {
            return Err(format!(
                "Output {} of {} is listed twice.",
                t.output, t.node_id
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Identify (F9 receiver wizard)
// ---------------------------------------------------------------------------

/// On time of one identify blink (ms).
pub const IDENTIFY_ON_MS: u64 = 350;
/// Off time between blinks (ms).
pub const IDENTIFY_OFF_MS: u64 = 350;
/// Dark pause after a blink group (ms).
pub const IDENTIFY_PAUSE_MS: u64 = 1400;
/// Most blinks in a group the wizard uses.
pub const IDENTIFY_MAX_BLINKS: u8 = 4;

/// Whether an identify light with `blinks` blinks per cycle is on at
/// `elapsed_ms` (0 blinks = steady on). All lights start their cycle together.
pub fn identify_on(blinks: u8, elapsed_ms: u64) -> bool {
    if blinks == 0 {
        return true;
    }
    let group = blinks as u64 * (IDENTIFY_ON_MS + IDENTIFY_OFF_MS);
    let t = elapsed_ms % (group + IDENTIFY_PAUSE_MS);
    t < group && t % (IDENTIFY_ON_MS + IDENTIFY_OFF_MS) < IDENTIFY_ON_MS
}

/// Length of one identify cycle for `blinks` (ms).
pub fn identify_cycle_ms(blinks: u8) -> u64 {
    if blinks == 0 {
        IDENTIFY_ON_MS + IDENTIFY_OFF_MS
    } else {
        blinks as u64 * (IDENTIFY_ON_MS + IDENTIFY_OFF_MS) + IDENTIFY_PAUSE_MS
    }
}

/// Fill a whole output with `rgb` when [`identify_on`], else dark.
pub fn render_identify(rgb: [u8; 3], blinks: u8, elapsed_ms: u64, out: &mut [u8]) {
    let c = if identify_on(blinks, elapsed_ms) {
        rgb
    } else {
        [0, 0, 0]
    };
    for px in out.chunks_exact_mut(3) {
        px.copy_from_slice(&c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn plan(n_targets: usize, max_pixels: u32) -> MapPlan {
        MapPlan {
            seed: 42,
            targets: (0..n_targets)
                .map(|i| MapTarget {
                    node_id: format!("n{}", i / 4),
                    output: (i % 4) as u32 + 1,
                    max_pixels,
                })
                .collect(),
            pixel_bits: pixel_bits_for(max_pixels),
            ..MapPlan::default()
        }
    }

    #[test]
    fn plan_defaults_from_minimal_json() {
        let p: MapPlan = serde_json::from_str(r#"{"seed":7}"#).unwrap();
        assert_eq!(p.bit_ms, 200);
        assert_eq!(p.level, 77);
        assert_eq!(p.phases, PHASE_A | PHASE_B);
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["bitMs"], 200);
        assert_eq!(v["startPosMs"], 0);
        assert!(v.get("countProbe").is_none());
    }

    #[test]
    fn codebooks_have_weight_and_distance() {
        for bits in [12u8, 16] {
            let book = codebook(bits);
            let w = code_weight(bits);
            assert!(book.iter().all(|c| c.count_ones() == w));
            assert!(book.iter().all(|&c| (c as u32) < (1 << bits)));
            for (i, a) in book.iter().enumerate() {
                for b in &book[i + 1..] {
                    assert!((a ^ b).count_ones() >= 4, "{a:b} vs {b:b}");
                }
            }
        }
        assert_eq!(codebook(12).len(), 132);
        assert_eq!(codebook(16).len(), 870);
        assert!(codebook(7).is_empty());
        assert_eq!(code_bits_for(1), 12);
        assert_eq!(code_bits_for(codebook(12).len()), 12);
        assert_eq!(code_bits_for(codebook(12).len() + 1), 16);
        assert_eq!(code_bits_for(max_targets() + 1), 0);
    }

    #[test]
    fn gray_roundtrip_and_adjacency() {
        for n in 0..5000u32 {
            assert_eq!(gray_inverse(gray(n)), n);
            assert_eq!((gray(n) ^ gray(n + 1)).count_ones(), 1);
        }
        assert_eq!(pixel_bits_for(1), 1);
        assert_eq!(pixel_bits_for(2), 1);
        assert_eq!(pixel_bits_for(3), 2);
        assert_eq!(pixel_bits_for(2048), 11);
        assert_eq!(pixel_bits_for(2049), 12);
    }

    #[test]
    fn schedule_matches_spec_example() {
        let p = plan(25, 2048);
        let s = schedule(&p);
        assert_eq!(s.preamble_ms, 2400);
        assert_eq!(s.phase_a_ms, 2400);
        assert_eq!(s.phase_b_ms, 4400);
        assert_eq!(s.pass_ms, 2400 + 2400 + 4400 + 600);
        assert_eq!(s.total_ms, 1000 + 3 * s.pass_ms);
        assert!(s.total_ms < 32_000);
        assert!(is_done(&p, s.total_ms));
        assert!(!is_done(&p, s.total_ms - 1));
    }

    #[test]
    fn frames_walk_the_timeline() {
        let p = plan(3, 10);
        assert_eq!(frame_for(&p, 0).slot, MapSlot::LeadIn);
        let t0 = 5 * 200;
        assert_eq!(
            frame_for(&p, t0).slot,
            MapSlot::Preamble { index: 0, on: true }
        );
        assert_eq!(
            frame_for(&p, t0 + 3 * 200).slot,
            MapSlot::Preamble {
                index: 3,
                on: false
            }
        );
        assert_eq!(
            frame_for(&p, t0 + 12 * 200).slot,
            MapSlot::PhaseA { index: 0 }
        );
        assert_eq!(
            frame_for(&p, t0 + 24 * 200 + 199).slot,
            MapSlot::PhaseB {
                index: 0,
                inverted: false
            }
        );
        assert_eq!(
            frame_for(&p, t0 + 25 * 200).slot,
            MapSlot::PhaseB {
                index: 0,
                inverted: true
            }
        );
        let per_pass = 12 + 12 + 8 + 3;
        let f = frame_for(&p, t0 + per_pass * 200);
        assert_eq!(f.pass, 1);
        assert_eq!(f.slot, MapSlot::Preamble { index: 0, on: true });
        // start_pos_ms shifts everything.
        let mut q = p.clone();
        q.start_pos_ms = 10_000;
        assert_eq!(frame_for(&q, 10_000 + t0), frame_for(&p, t0));
        assert_eq!(frame_for(&q, 500).slot, MapSlot::LeadIn);
    }

    #[test]
    fn phase_a_reproduces_codeword_and_phase_b_the_pixel() {
        let p = plan(7, 100);
        let s = schedule(&p);
        let a0 = s.lead_in_ms + s.preamble_ms;
        let b0 = a0 + s.phase_a_ms;
        for k in 0..7 {
            let mut word = 0u16;
            for i in 0..12u64 {
                let on = level_at(&p, k, 5, a0 + i * 200 + 100) > 0;
                word = (word << 1) | on as u16;
            }
            assert_eq!(word, codebook(12)[k]);
            for px in [0u32, 1, 50, 99] {
                let mut g = 0u32;
                for j in 0..p.pixel_bits as u64 {
                    let a = level_at(&p, k, px, b0 + 2 * j * 200 + 100) > 0;
                    let b = level_at(&p, k, px, b0 + (2 * j + 1) * 200 + 100) > 0;
                    assert_ne!(a, b, "pair must be complementary");
                    g = (g << 1) | a as u32;
                }
                assert_eq!(gray_inverse(g), px);
            }
            // Beyond max_pixels: always dark.
            assert_eq!(level_at(&p, k, 100, a0 - 1000), 0);
        }
    }

    #[test]
    fn render_output_paints_targets_only() {
        let p = plan(2, 4);
        let t = 5 * 200 + 50; // preamble on
        let mut buf = vec![9u8; 6 * 3];
        assert!(render_output(&p, "n0", 2, t, &mut buf));
        assert_eq!(&buf[..12], &[77; 12]);
        assert_eq!(&buf[12..], &[0; 6], "beyond max_pixels is dark");
        let mut other = vec![9u8; 6];
        assert!(!render_output(&p, "n0", 3, t, &mut other));
        assert_eq!(other, vec![9u8; 6]);
        // lead-in: dark
        assert!(render_output(&p, "n0", 1, 0, &mut buf));
        assert!(buf.iter().all(|&b| b == 0));
    }

    #[test]
    fn count_probe_renders_static_frame() {
        let mut p = plan(1, 10);
        p.count_probe = Some(3);
        let mut buf = vec![9u8; 12 * 3];
        render_target(&p, 0, 123_456, &mut buf);
        assert_eq!(&buf[..9], &[PROBE_RUN, PROBE_RUN, PROBE_RUN].concat()[..]);
        assert_eq!(&buf[9..12], &PROBE_END);
        assert!(buf[12..].iter().all(|&b| b == 0));
        assert!(!is_done(&p, u64::MAX / 2));
        // Probe beyond max_pixels: only the green run.
        p.count_probe = Some(20);
        render_target(&p, 0, 0, &mut buf);
        assert_eq!(&buf[27..30], &PROBE_RUN);
        assert!(buf[30..].iter().all(|&b| b == 0));
    }

    #[test]
    fn validate_catches_bad_plans() {
        assert!(validate(&plan(3, 50)).is_ok());
        assert!(validate(&plan(0, 50)).is_err());
        let mut p = plan(3, 50);
        p.level = 200;
        assert!(validate(&p).is_err());
        let mut p = plan(3, 50);
        p.bit_ms = 50;
        assert!(validate(&p).is_err());
        let mut p = plan(3, 50);
        p.pixel_bits = 3;
        assert!(validate(&p).is_err());
        let mut p = plan(2, 50);
        p.targets[1] = p.targets[0].clone();
        assert!(validate(&p).is_err());
        assert!(validate(&plan(max_targets() + 1, 4)).is_err());
    }

    #[test]
    fn identify_blinks() {
        assert!(identify_on(0, 12345));
        let ons = |b: u8| {
            let mut n = 0;
            let mut prev = false;
            for t in (0..identify_cycle_ms(b)).step_by(10) {
                let on = identify_on(b, t);
                if on && !prev {
                    n += 1;
                }
                prev = on;
            }
            n
        };
        for b in 1..=IDENTIFY_MAX_BLINKS {
            assert_eq!(ons(b), b);
        }
        let mut out = vec![0u8; 9];
        render_identify([1, 2, 3], 2, 0, &mut out);
        assert_eq!(out, vec![1, 2, 3, 1, 2, 3, 1, 2, 3]);
        render_identify([1, 2, 3], 2, 400, &mut out);
        assert_eq!(out, vec![0; 9]);
    }

    // -----------------------------------------------------------------------
    // Shared test vectors (consumed by web/src/lib/cv/mapcode.test.ts)
    // -----------------------------------------------------------------------

    fn slot_json(f: &MapFrame) -> Value {
        serde_json::to_value(f).unwrap()
    }

    fn vectors() -> Value {
        let gray: Vec<[u32; 2]> = [0u32, 1, 2, 3, 7, 8, 100, 255, 1023, 2047]
            .iter()
            .map(|&n| [n, gray(n)])
            .collect();
        let book = |bits: u8| {
            let b = codebook(bits);
            let sum: u64 = b
                .iter()
                .enumerate()
                .map(|(i, &c)| (i as u64 + 1) * c as u64)
                .sum();
            json!({ "bits": bits, "len": b.len(), "first": &b[..32], "last": b[b.len() - 1], "weightedSum": sum })
        };
        let mut plans = Vec::new();
        let mut cases = vec![plan(3, 12), plan(200, 5)];
        let mut b_only = plan(1, 70);
        b_only.phases = PHASE_B;
        b_only.passes = 2;
        b_only.bit_ms = 150;
        b_only.start_pos_ms = 777;
        cases.push(b_only);
        for p in cases {
            let s = schedule(&p);
            let mut samples = Vec::new();
            let bit = p.bit_ms as u64;
            let slots = s.total_ms.div_ceil(bit) + 2;
            let shown_targets: Vec<usize> = [0usize, 1, 2, 150, 199]
                .into_iter()
                .filter(|&k| k < p.targets.len())
                .collect();
            for slot in 0..slots {
                let t = p.start_pos_ms + slot * bit + bit / 2;
                let f = frame_for(&p, t);
                let targets: Vec<String> = shown_targets
                    .iter()
                    .map(|&k| {
                        (0..p.targets[k].max_pixels.min(16) + 1)
                            .map(|px| if level_at(&p, k, px, t) > 0 { '1' } else { '0' })
                            .collect()
                    })
                    .collect();
                samples.push(json!({ "t": t, "frame": slot_json(&f), "targets": targets }));
            }
            plans.push(json!({
                "plan": p,
                "schedule": s,
                "shownTargets": shown_targets,
                "codewords": shown_targets.iter().map(|&k| codeword(&p, k)).collect::<Vec<_>>(),
                "samples": samples,
            }));
        }
        json!({
            "note": "Generated by pixelplus-core mapcode tests; regenerate with MAPCODE_BLESS=1.",
            "constants": { "leadInSlots": LEAD_IN_SLOTS, "gapSlots": GAP_SLOTS, "preamble": PREAMBLE },
            "gray": gray,
            "codebooks": [book(12), book(16)],
            "plans": plans,
        })
    }

    #[test]
    fn fixture_vectors_are_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mapcode/vectors.json");
        let want = vectors();
        if std::env::var_os("MAPCODE_BLESS").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, serde_json::to_string(&want).unwrap() + "\n").unwrap();
        }
        let have: Value = serde_json::from_str(
            &std::fs::read_to_string(&path).expect("fixture missing: run with MAPCODE_BLESS=1"),
        )
        .unwrap();
        assert_eq!(
            have, want,
            "mapcode vectors changed: regenerate with MAPCODE_BLESS=1"
        );
    }
}
