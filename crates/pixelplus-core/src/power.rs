//! Power (current) estimation for sequences, compared against receiver fuse ratings.
//!
//! # Model and assumptions
//!
//! * Pixels are **12 V WS2811-style** pixels: each colour channel is a constant-current
//!   sink (≈20 mA) driving LEDs in series, so a pixel draws roughly
//!   `maxMilliampsPerPixel` (default 60 mA) at full white and the current scales
//!   linearly with the PWM duty of each channel:
//!   `I = (r' + g' + b') / 765 × maxMilliampsPerPixel`.
//!   5 V pixels (WS2812B) behave the same way per pixel, but draw more current per watt;
//!   set `maxMilliampsPerPixel` on those props accordingly.
//! * `r', g', b'` are the values *actually sent*: output brightness and gamma are
//!   applied (colour order does not matter for current). Quiescent current of the pixel
//!   ICs (~1 mA/pixel) is ignored.
//! * Sequences are sampled every *N*th frame for speed (see [`PowerOptions`]), so very
//!   short peaks between samples can be missed. Peaks are the maximum over sampled frames.
//! * Receiver ports are protected by resettable PPTC fuses (Chandler diffrx: Bourns
//!   MF-R600, **6 A hold at 23 °C**). PPTC hold current falls with temperature
//!   (≈4.1 A at 60 °C, a factor of [`PPTC_DERATE_60C`]); a fuse only trips on *sustained*
//!   over-current, so the *average* current is compared to the hold rating for "will
//!   trip" and the *peak* for warnings.

use serde::{Deserialize, Serialize};

use crate::fseq::{FseqError, FseqFile};
use crate::model::{LimiterMode, Node, NodePowerBudget, PowerGroup, PowerGroupKind, Prop, Show};
use std::collections::BTreeMap;
use std::io::{Read, Seek};

/// Ratio of PPTC hold current at 60 °C to the 23 °C rating (MF-R600: 4.1 A / 6 A).
pub const PPTC_DERATE_60C: f32 = 4.1 / 6.0;

/// Estimation options.
#[derive(Debug, Clone, PartialEq)]
pub struct PowerOptions {
    /// Sample every Nth frame. `None` picks a stride giving at most
    /// [`PowerOptions::target_samples`] samples.
    pub sample_every: Option<u32>,
    /// Target number of sampled frames when `sample_every` is `None`.
    pub target_samples: u32,
}

impl Default for PowerOptions {
    fn default() -> Self {
        PowerOptions {
            sample_every: None,
            target_samples: 1200,
        }
    }
}

/// Current on one node output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputPower {
    /// Node id.
    pub node_id: String,
    /// 1-based output.
    pub output: u32,
    /// Output label (e.g. `J3-2`).
    pub label: String,
    /// Wired pixels on the output.
    pub pixels: u32,
    /// Highest sampled current (A).
    pub peak_amps: f32,
    /// Mean current over sampled frames (A).
    pub avg_amps: f32,
    /// Current if every pixel showed full white at the output's brightness/gamma (A).
    pub max_amps: f32,
}

/// Status of a receiver port relative to its fuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PowerStatus {
    /// Comfortably within rating.
    Ok,
    /// Peaks exceed the derated (hot) hold current, or the 23 °C hold current briefly.
    Warn,
    /// Average current exceeds the hold rating: the fuse will trip.
    Over,
}

/// Current on one receiver port.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiverPortPower {
    /// Receiver id.
    pub receiver_id: String,
    /// Receiver name.
    pub receiver_name: String,
    /// 1-based receiver port.
    pub port: u32,
    /// Node feeding the receiver.
    pub node_id: String,
    /// 1-based node output feeding this port.
    pub output: u32,
    /// Highest sampled current (A).
    pub peak_amps: f32,
    /// Mean current (A).
    pub avg_amps: f32,
    /// Fuse hold rating at 23 °C, if known (A).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fuse_amps: Option<f32>,
    /// Fuse hold rating derated to 60 °C, if known (A).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derated_fuse_amps: Option<f32>,
    /// Verdict.
    pub status: PowerStatus,
}

/// Current of one prop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PropPower {
    /// Prop id.
    pub prop_id: String,
    /// Prop name.
    pub name: String,
    /// Highest sampled current (A).
    pub peak_amps: f32,
    /// Mean current (A).
    pub avg_amps: f32,
    /// Full-white current at 100 % brightness (A).
    pub max_amps: f32,
}

/// Total current of one node (supply sizing).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodePower {
    /// Node id.
    pub node_id: String,
    /// Highest sampled total current (A).
    pub peak_amps: f32,
    /// Mean total current (A).
    pub avg_amps: f32,
}

/// Result of [`estimate_power`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PowerEstimate {
    /// Frames sampled.
    pub frames_sampled: u32,
    /// Stride between sampled frames.
    pub sample_every: u32,
    /// Per node output.
    pub per_output: Vec<OutputPower>,
    /// Per receiver port.
    pub per_receiver_port: Vec<ReceiverPortPower>,
    /// Per prop.
    pub per_prop: Vec<PropPower>,
    /// Per node.
    pub per_node: Vec<NodePower>,
    /// Per power supply (F12 supply view).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub per_supply: Vec<SupplyPower>,
    /// Budget groups the power limiter would scale down while this plays
    /// (F12, simulated with the same [`Limiter`] the output thread runs).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limited: Vec<LimitedGroup>,
    /// Human-readable warnings.
    pub warnings: Vec<String>,
}

/// One accumulation rule: bytes `src..src+len` of the frame feed output `out` and prop
/// `prop`, using `lut` (per-output brightness/gamma) and scale `ma_per_unit`.
struct Tap {
    src: usize,
    len: usize,
    out: usize,
    prop: usize,
    ma_per_unit: f64,
}

struct OutputSlot {
    node: usize,
    output: u32,
    label: String,
    pixels: u32,
    lut: [u16; 256],
    max_ma: f64,
    /// Σ (mA per pixel × pixels): the output's mean mA/pixel is `ma_px / pixels`.
    ma_px: f64,
}

struct Plan {
    taps: Vec<Tap>,
    outputs: Vec<OutputSlot>,
    /// Indices into `show.props` of props with at least one tap.
    props: Vec<usize>,
}

fn lut_for(brightness: u8, gamma: f32) -> [u16; 256] {
    let b = brightness.min(100) as f32 / 100.0;
    let g = if gamma.is_finite() && gamma > 0.0 {
        gamma
    } else {
        1.0
    };
    let mut lut = [0u16; 256];
    for (v, slot) in lut.iter_mut().enumerate() {
        // Scaled by 256 to keep precision in integer sums.
        let x = (v as f32 / 255.0).powf(g) * 255.0 * b;
        *slot = (x * 256.0).round() as u16;
    }
    lut
}

fn build_plan(show: &Show) -> (Plan, Vec<String>) {
    let mut warnings = Vec::new();
    let mut outputs: Vec<OutputSlot> = Vec::new();
    let mut slot_of = std::collections::HashMap::<(usize, u32), usize>::new();
    let mut taps = Vec::new();
    let mut prop_slot: Vec<Option<usize>> = vec![None; show.props.len()];
    let mut props = Vec::new();

    for (pi, prop) in show.props.iter().enumerate() {
        if prop.channels_per_pixel != 3 {
            continue;
        }
        let ma = prop.ma_per_pixel();
        if !(ma.is_finite() && ma >= 0.0) {
            warnings.push(format!(
                "prop '{}' has an invalid milliamps-per-pixel value",
                prop.name
            ));
            continue;
        }
        for seg in &prop.segments {
            let Some(ni) = show.nodes.iter().position(|n| n.id == seg.node_id) else {
                continue;
            };
            let node: &Node = &show.nodes[ni];
            let avail = prop.pixel_count.saturating_sub(seg.prop_offset);
            let count = seg.pixel_count.min(avail);
            if count == 0 || seg.output == 0 {
                continue;
            }
            let slot = *slot_of.entry((ni, seg.output)).or_insert_with(|| {
                let cfg = node.outputs.iter().find(|o| o.index == seg.output);
                let (brightness, gamma, label) = match cfg {
                    Some(o) if !o.enabled => (0, 1.0, o.label.clone()),
                    Some(o) => (o.brightness, o.gamma, o.label.clone()),
                    None => (100, 1.0, node.board.output_label(seg.output as usize)),
                };
                outputs.push(OutputSlot {
                    node: ni,
                    output: seg.output,
                    label,
                    pixels: 0,
                    lut: lut_for(brightness, gamma),
                    max_ma: 0.0,
                    ma_px: 0.0,
                });
                outputs.len() - 1
            });
            let s = &mut outputs[slot];
            s.pixels = s.pixels.saturating_add(count);
            let full: f64 = s.lut[255] as f64 * 3.0 / 256.0 / 765.0 * ma as f64 * count as f64;
            s.max_ma += full;
            s.ma_px += ma as f64 * count as f64;
            if prop_slot[pi].is_none() {
                prop_slot[pi] = Some(props.len());
                props.push(pi);
            }
            // One tap per contiguous channel run (xLights individual start channels).
            for piece in prop.channel_pieces(seg.prop_offset, count) {
                taps.push(Tap {
                    src: piece.channel_start as usize,
                    len: piece.pixel_count as usize * 3,
                    out: slot,
                    prop: prop_slot[pi].expect("set above"),
                    ma_per_unit: ma as f64 / 765.0 / 256.0,
                });
            }
        }
    }
    outputs.sort_by_key(|o| (o.node, o.output));
    // Re-point taps after sorting.
    let mut remap = vec![0usize; outputs.len()];
    let mut keys: Vec<(usize, u32)> = slot_of.keys().copied().collect();
    keys.sort();
    for (new_idx, key) in keys.iter().enumerate() {
        remap[slot_of[key]] = new_idx;
    }
    for t in &mut taps {
        t.out = remap[t.out];
    }
    (
        Plan {
            taps,
            outputs,
            props,
        },
        warnings,
    )
}

struct Accum {
    out_peak: Vec<f64>,
    out_sum: Vec<f64>,
    prop_peak: Vec<f64>,
    prop_sum: Vec<f64>,
    node_peak: Vec<f64>,
    node_sum: Vec<f64>,
    frame_out: Vec<f64>,
    frame_prop: Vec<f64>,
    frame_node: Vec<f64>,
    samples: u32,
}

impl Accum {
    fn new(plan: &Plan, nodes: usize) -> Self {
        let o = plan.outputs.len();
        let p = plan.props.len();
        Accum {
            out_peak: vec![0.0; o],
            out_sum: vec![0.0; o],
            prop_peak: vec![0.0; p],
            prop_sum: vec![0.0; p],
            node_peak: vec![0.0; nodes],
            node_sum: vec![0.0; nodes],
            frame_out: vec![0.0; o],
            frame_prop: vec![0.0; p],
            frame_node: vec![0.0; nodes],
            samples: 0,
        }
    }

    fn add_frame(&mut self, plan: &Plan, frame: &[u8]) {
        self.frame_out.fill(0.0);
        self.frame_prop.fill(0.0);
        self.frame_node.fill(0.0);
        for t in &plan.taps {
            if t.src >= frame.len() {
                continue;
            }
            let end = (t.src + t.len).min(frame.len());
            let lut = &plan.outputs[t.out].lut;
            let units: u64 = frame[t.src..end]
                .iter()
                .map(|&b| lut[b as usize] as u64)
                .sum();
            let ma = units as f64 * t.ma_per_unit;
            self.frame_out[t.out] += ma;
            self.frame_prop[t.prop] += ma;
        }
        for (i, &ma) in self.frame_out.iter().enumerate() {
            self.out_sum[i] += ma;
            self.out_peak[i] = self.out_peak[i].max(ma);
            self.frame_node[plan.outputs[i].node] += ma;
        }
        for (i, &ma) in self.frame_prop.iter().enumerate() {
            self.prop_sum[i] += ma;
            self.prop_peak[i] = self.prop_peak[i].max(ma);
        }
        for (i, &ma) in self.frame_node.iter().enumerate() {
            self.node_sum[i] += ma;
            self.node_peak[i] = self.node_peak[i].max(ma);
        }
        self.samples += 1;
    }
}

/// Estimate current draw of a sequence for every output, receiver port and prop.
pub fn estimate_power<R: Read + Seek>(
    show: &Show,
    fseq: &mut FseqFile<R>,
    opts: &PowerOptions,
) -> Result<PowerEstimate, FseqError> {
    let frames = fseq.frame_count();
    let stride = match opts.sample_every {
        Some(n) => n.max(1),
        None => frames.div_ceil(opts.target_samples.max(1)).max(1),
    };
    let mut buf = vec![0u8; fseq.frame_size()];
    let dt = f64::from(stride) * f64::from(fseq.frame_ms().max(1));
    estimate_timed(show, stride, Some(dt), |f| {
        let mut i = 0u32;
        while i < frames {
            fseq.frame(i, &mut buf)?;
            f(&buf);
            i = i.saturating_add(stride);
        }
        Ok(())
    })
}

/// Worst case: every prop at full white (useful before any sequence is uploaded).
pub fn estimate_full_white(show: &Show) -> PowerEstimate {
    // Bounded like a real fseq frame: a prop with a corrupt channel range must not
    // make us allocate gigabytes of "white".
    let len = show
        .props
        .iter()
        .map(|p| p.channel_end())
        .max()
        .unwrap_or(0)
        .min(crate::fseq::MAX_FRAME_BYTES) as usize;
    let frame = vec![255u8; len];
    estimate_from_frames(show, 1, |f| {
        f(&frame);
        Ok::<_, FseqError>(())
    })
    .unwrap_or_default()
}

/// Core estimator over an arbitrary frame source. `feed` must call the supplied closure
/// once per sampled frame (absolute channel space).
pub fn estimate_from_frames<E>(
    show: &Show,
    sample_every: u32,
    feed: impl FnOnce(&mut dyn FnMut(&[u8])) -> Result<(), E>,
) -> Result<PowerEstimate, E> {
    estimate_timed(show, sample_every, None, feed)
}

/// [`estimate_from_frames`] with the time between sampled frames (`dt_ms`):
/// with it, the power limiter is simulated over the frames (`limited`).
pub fn estimate_timed<E>(
    show: &Show,
    sample_every: u32,
    dt_ms: Option<f64>,
    feed: impl FnOnce(&mut dyn FnMut(&[u8])) -> Result<(), E>,
) -> Result<PowerEstimate, E> {
    let (plan, mut warnings) = build_plan(show);
    let mut acc = Accum::new(&plan, show.nodes.len());
    let mut supplies = SupplyAccum::new(show, &plan);
    let mut sim = dt_ms
        .filter(|d| d.is_finite() && *d > 0.0)
        .map(|dt| LimiterSim::new(show, &plan, dt));
    feed(&mut |frame: &[u8]| {
        acc.add_frame(&plan, frame);
        supplies.add(&acc.frame_out);
        if let Some(sim) = sim.as_mut() {
            sim.add(&acc.frame_out);
        }
    })?;
    let n = acc.samples.max(1) as f64;
    let amps = |ma: f64| (ma / 1000.0) as f32;

    let per_output: Vec<OutputPower> = plan
        .outputs
        .iter()
        .enumerate()
        .map(|(i, o)| OutputPower {
            node_id: show.nodes[o.node].id.clone(),
            output: o.output,
            label: o.label.clone(),
            pixels: o.pixels,
            peak_amps: amps(acc.out_peak[i]),
            avg_amps: amps(acc.out_sum[i] / n),
            max_amps: amps(o.max_ma),
        })
        .collect();

    let per_prop = plan
        .props
        .iter()
        .enumerate()
        .map(|(i, &pi)| {
            let p: &Prop = &show.props[pi];
            PropPower {
                prop_id: p.id.clone(),
                name: p.name.clone(),
                peak_amps: amps(acc.prop_peak[i]),
                avg_amps: amps(acc.prop_sum[i] / n),
                max_amps: amps(p.pixel_count as f64 * p.ma_per_pixel() as f64),
            }
        })
        .collect();

    let per_node = show
        .nodes
        .iter()
        .enumerate()
        .filter(|(i, _)| plan.outputs.iter().any(|o| o.node == *i))
        .map(|(i, node)| NodePower {
            node_id: node.id.clone(),
            peak_amps: amps(acc.node_peak[i]),
            avg_amps: amps(acc.node_sum[i] / n),
        })
        .collect();

    let mut per_receiver_port = Vec::new();
    for r in &show.receivers {
        let fuse = r.fuse_amps.or_else(|| r.kind.default_fuse_amps());
        let derated = fuse.map(|f| f * PPTC_DERATE_60C);
        for port in 1..=r.kind.port_count() as u32 {
            if r.jack == 0 {
                continue;
            }
            let output = r.output_for_port(port);
            let Some(op) = per_output
                .iter()
                .find(|o| o.node_id == r.node_id && o.output == output)
            else {
                continue;
            };
            // 1 mA tolerance so "exactly at the rating" is not reported as over it.
            const EPS: f32 = 0.001;
            let status = match (fuse, derated) {
                (Some(f), _) if op.avg_amps > f + EPS => PowerStatus::Over,
                (Some(f), Some(d)) if op.peak_amps > f + EPS || op.peak_amps > d + EPS => {
                    PowerStatus::Warn
                }
                _ => PowerStatus::Ok,
            };
            let where_ = match &r.location {
                Some(l) => format!("{} ({l})", r.name),
                None => r.name.clone(),
            };
            match (status, fuse) {
                (PowerStatus::Over, Some(f)) => warnings.push(format!(
                    "{where_} port {port}: average {:.1} A exceeds the {f:.1} A fuse; it will trip. Lower brightness or move props to another port.",
                    op.avg_amps
                )),
                (PowerStatus::Warn, Some(f)) if op.peak_amps > f + EPS => warnings.push(format!(
                    "{where_} port {port}: peaks of {:.1} A exceed the {f:.1} A fuse hold current; sustained peaks may trip it.",
                    op.peak_amps
                )),
                (PowerStatus::Warn, Some(f)) => warnings.push(format!(
                    "{where_} port {port}: peaks of {:.1} A are above the fuse's hot rating ({:.1} A at 60 °C, {f:.1} A at 23 °C).",
                    op.peak_amps,
                    f * PPTC_DERATE_60C
                )),
                _ => {}
            }
            per_receiver_port.push(ReceiverPortPower {
                receiver_id: r.id.clone(),
                receiver_name: r.name.clone(),
                port,
                node_id: r.node_id.clone(),
                output,
                peak_amps: op.peak_amps,
                avg_amps: op.avg_amps,
                fuse_amps: fuse,
                derated_fuse_amps: derated,
                status,
            });
        }
    }

    let per_supply = supplies.finish(acc.samples, &mut warnings);
    let limited = sim.map(|s| s.finish(show)).unwrap_or_default();
    if let Some(worst) = limited
        .iter()
        .min_by(|a, b| a.min_scale.total_cmp(&b.min_scale))
    {
        warnings.push(format!(
            "The power limiter would dim {} for {:.0} s (down to {:.0} %).",
            worst.label,
            worst.seconds,
            worst.min_scale * 100.0
        ));
    }

    Ok(PowerEstimate {
        frames_sampled: acc.samples,
        sample_every: sample_every.max(1),
        per_output,
        per_receiver_port,
        per_prop,
        per_node,
        per_supply,
        limited,
        warnings,
    })
}

// ---------------------------------------------------------------------------
// F12: budgets, the limiter, supply view
// ---------------------------------------------------------------------------

/// Thermal time constant of a receiver port's PPTC fuse (it trips on I²t over seconds).
pub const PORT_TAU_MS: u32 = 8_000;
/// Averaging time constant of a receiver's main fuse / bus.
pub const BUS_TAU_MS: u32 = 1_000;
/// Supplies: over-current protection is fast, so the limit is per frame.
pub const SUPPLY_TAU_MS: u32 = 0;
/// Global cap (household circuit breaker): 1 s average.
pub const GLOBAL_TAU_MS: u32 = 1_000;
/// Averaged groups may draw up to this multiple of their budget while cold.
pub const HEADROOM: f32 = 4.0;
/// Time for a scale to recover from 0 to 1 (slow release: no pumping).
pub const RELEASE_MS: f32 = 1_500.0;
/// PPTC hold derating used when the enclosure temperature is unknown.
pub const PPTC_DERATE_UNKNOWN: f32 = 0.8;
/// Pixel supply voltage assumed for `globalWatts` when no supply says otherwise.
pub const DEFAULT_PIXEL_VOLTS: f32 = 12.0;
/// Efficiency of the pixel supplies when converting a wall-power cap (W) into amps.
pub const SUPPLY_EFFICIENCY: f32 = 0.85;

/// PPTC hold-current derating at `temp_c` (1.0 at 23 °C, [`PPTC_DERATE_60C`] at
/// 60 °C, linear in between and beyond, clamped to 0.4..1.1); `None` (no
/// temperature known) gives [`PPTC_DERATE_UNKNOWN`].
pub fn pptc_derate(temp_c: Option<f32>) -> f32 {
    match temp_c.filter(|t| t.is_finite()) {
        None => PPTC_DERATE_UNKNOWN,
        Some(t) => {
            let per_deg = (1.0 - PPTC_DERATE_60C) / (60.0 - 23.0);
            (1.0 - (t - 23.0) * per_deg).clamp(0.4, 1.1)
        }
    }
}

/// Per-output facts every budget is computed from: `(node index, output)` →
/// (pixels, full-white mA at the output's brightness/gamma, mean mA/pixel).
struct OutputFacts {
    node: usize,
    output: u32,
    max_ma: f64,
    ma_pp: f32,
}

fn output_facts(plan: &Plan) -> Vec<OutputFacts> {
    plan.outputs
        .iter()
        .map(|o| OutputFacts {
            node: o.node,
            output: o.output,
            max_ma: o.max_ma,
            ma_pp: if o.pixels > 0 {
                (o.ma_px / o.pixels as f64) as f32
            } else {
                Prop::DEFAULT_MA_PER_PIXEL
            },
        })
        .collect()
}

/// The outputs `(node id, output)` a power supply feeds: every port of its
/// receivers plus its direct outputs.
pub fn supply_outputs(show: &Show, supply: &crate::model::PowerSupply) -> Vec<(String, u32)> {
    let mut v: Vec<(String, u32)> = Vec::new();
    for rid in &supply.receiver_ids {
        if let Some(r) = show.receivers.iter().find(|r| &r.id == rid) {
            if r.jack == 0 {
                continue;
            }
            for port in 1..=r.kind.port_count() as u32 {
                v.push((r.node_id.clone(), r.output_for_port(port)));
            }
        }
    }
    for d in &supply.direct_outputs {
        v.push((d.node_id.clone(), d.output));
    }
    v.sort();
    v.dedup();
    v
}

/// Limiter budgets for every node of the show (F12), keyed by node id. Empty
/// when the limiter is off. See [`node_budget`].
pub fn show_budgets(show: &Show) -> BTreeMap<String, NodePowerBudget> {
    let settings = &show.settings.power;
    let mut out: BTreeMap<String, NodePowerBudget> = BTreeMap::new();
    if settings.mode == LimiterMode::Off {
        return out;
    }
    let safety = if settings.safety.is_finite() {
        settings.safety.clamp(0.1, 1.0)
    } else {
        0.9
    };
    let (plan, _) = build_plan(show);
    let facts = output_facts(&plan);
    let node_idx = |id: &str| show.nodes.iter().position(|n| n.id == id);
    let fact =
        |node: usize, output: u32| facts.iter().find(|f| f.node == node && f.output == output);

    for n in &show.nodes {
        out.insert(
            n.id.clone(),
            NodePowerBudget {
                mode: settings.mode,
                safety,
                groups: vec![],
                ma_pp: BTreeMap::new(),
            },
        );
    }
    for f in &facts {
        if let Some(b) = out.get_mut(&show.nodes[f.node].id) {
            b.ma_pp.insert(f.output, (f.ma_pp * 100.0).round() / 100.0);
        }
    }
    let push = |out: &mut BTreeMap<String, NodePowerBudget>, node: &str, g: PowerGroup| {
        if g.members.is_empty() || !(g.budget_a.is_finite() && g.budget_a > 0.0) {
            return;
        }
        if let Some(b) = out.get_mut(node) {
            b.groups.push(g);
        }
    };
    let round = |a: f32| (a * 1000.0).round() / 1000.0;

    // 1. Receiver port fuses (PPTC, thermal) and 2. main fuse / bus.
    for r in &show.receivers {
        let Some(ni) = node_idx(&r.node_id) else {
            continue;
        };
        if r.jack == 0 {
            continue;
        }
        let ports: Vec<(u32, u32)> = (1..=r.kind.port_count() as u32)
            .map(|p| (p, r.output_for_port(p)))
            .filter(|&(_, o)| fact(ni, o).is_some())
            .collect();
        if let Some(fuse) = r.fuse_amps.or_else(|| r.kind.default_fuse_amps()) {
            for &(port, output) in &ports {
                push(
                    &mut out,
                    &r.node_id,
                    PowerGroup {
                        id: format!("port:{}:{port}", r.id),
                        kind: PowerGroupKind::Port,
                        budget_a: round(fuse * pptc_derate(None) * safety),
                        tau_ms: PORT_TAU_MS,
                        members: vec![output],
                    },
                );
            }
        }
        if let Some(main) = r.main_fuse_amps {
            push(
                &mut out,
                &r.node_id,
                PowerGroup {
                    id: format!("bus:{}", r.id),
                    kind: PowerGroupKind::Bus,
                    budget_a: round(main * safety),
                    tau_ms: BUS_TAU_MS,
                    members: ports.iter().map(|&(_, o)| o).collect(),
                },
            );
        }
    }

    // 3. Power supplies (instantaneous), split pro rata by possible current
    //    when they feed several nodes.
    for sup in &show.power_supplies {
        let members: Vec<(usize, u32, f64)> = supply_outputs(show, sup)
            .into_iter()
            .filter_map(|(node, o)| {
                let ni = node_idx(&node)?;
                fact(ni, o).map(|f| (ni, o, f.max_ma))
            })
            .collect();
        split_group(&members, sup.amps * safety, |ni, budget, members| {
            push(
                &mut out,
                &show.nodes[ni].id,
                PowerGroup {
                    id: format!("supply:{}", sup.id),
                    kind: PowerGroupKind::Supply,
                    budget_a: round(budget),
                    tau_ms: SUPPLY_TAU_MS,
                    members,
                },
            )
        });
    }

    // 4. Global cap: pixel-side amps, or wall watts at the pixel voltage.
    let volts = show
        .power_supplies
        .iter()
        .map(|s| s.volts)
        .find(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(DEFAULT_PIXEL_VOLTS);
    let from_watts = settings
        .global_watts
        .filter(|w| w.is_finite() && *w > 0.0)
        .map(|w| w * SUPPLY_EFFICIENCY / volts);
    let global = match (
        settings.global_amps.filter(|a| a.is_finite() && *a > 0.0),
        from_watts,
    ) {
        (Some(a), Some(w)) => Some(a.min(w)),
        (a, w) => a.or(w),
    };
    if let Some(cap) = global {
        let members: Vec<(usize, u32, f64)> =
            facts.iter().map(|f| (f.node, f.output, f.max_ma)).collect();
        split_group(&members, cap * safety, |ni, budget, members| {
            push(
                &mut out,
                &show.nodes[ni].id,
                PowerGroup {
                    id: "global".into(),
                    kind: PowerGroupKind::Global,
                    budget_a: round(budget),
                    tau_ms: GLOBAL_TAU_MS,
                    members,
                },
            )
        });
    }
    for b in out.values_mut() {
        for g in &mut b.groups {
            g.members.sort_unstable();
            g.members.dedup();
        }
    }
    out
}

/// Split a budget over the nodes of `members` (node index, output, max mA)
/// pro rata by each node's possible current (equal shares when nothing can
/// light), calling `f(node, budget, outputs)` once per node.
fn split_group(
    members: &[(usize, u32, f64)],
    budget: f32,
    mut f: impl FnMut(usize, f32, Vec<u32>),
) {
    let mut by_node: BTreeMap<usize, (f64, Vec<u32>)> = BTreeMap::new();
    for &(ni, o, ma) in members {
        let e = by_node.entry(ni).or_default();
        e.0 += ma;
        e.1.push(o);
    }
    let total: f64 = by_node.values().map(|v| v.0).sum();
    let count = by_node.len().max(1) as f64;
    for (ni, (ma, outs)) in by_node {
        let share = if total > 0.0 { ma / total } else { 1.0 / count };
        f(ni, (f64::from(budget) * share) as f32, outs);
    }
}

/// The power limiter budget of one node (F12), for its manifest
/// (`NodeManifest.power`) and for the leader's own output thread.
///
/// `None` when the limiter is off or the node is not in the show. Groups:
/// receiver port fuses (`port:<receiverId>:<port>`, hold × 0.8 × safety, τ 8 s),
/// receiver main fuses (`bus:<receiverId>`, τ 1 s), power supplies
/// (`supply:<id>`, instantaneous, split pro rata by possible current when a
/// supply feeds several nodes) and the global cap (`global`, τ 1 s). `mApp`
/// is each output's mean mA per pixel at full white.
pub fn node_budget(show: &Show, node_id: &str) -> Option<NodePowerBudget> {
    show.node(node_id)?;
    show_budgets(show).remove(node_id)
}

/// One budget group of a node as the limiter sees it right now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupLive {
    pub id: String,
    /// Current (A) the group's budget is compared against: the thermal / 1 s
    /// average for fuses and the global cap, the frame's current for supplies.
    pub amps: f32,
    /// Budget (A).
    pub budget: f32,
    /// Scale the group asks for (1 = none).
    pub scale: f32,
}

#[derive(Debug, Clone)]
struct GroupState {
    id: String,
    budget: f32,
    tau_ms: f32,
    members: Vec<usize>,
    /// Average of the current actually drawn (after scaling).
    ema_act: f32,
    /// Current drawn in the last frame.
    now_act: f32,
    need: f32,
    /// Slow (5 s) average of the estimated current drawn, for measured feedback.
    est_slow: f32,
    /// Measured / estimated current (F20 sensor feedback; 1 = trust the estimate).
    correction: f32,
}

/// The per-node power limiter (F12): per-output scale factors from the
/// budgets, one [`Limiter::step`] per output frame.
///
/// For each group `g` (outputs `p ∈ g`, raw current `I = ΣI_p` this frame,
/// budget `B`):
/// * supplies (τ = 0): `s = min(1, B / I)`, per frame;
/// * fuses and caps (τ > 0), with `EMA` the average of the current actually
///   drawn (`EMA ← EMA + (s·I − EMA)·Δt/τ`): the allowed current tapers
///   linearly from `m·B` while the average is cold to `B` at the limit,
///   `s = min(1, (m·B − (m−1)·EMA) / I)` with `m` = [`HEADROOM`] (at most
///   `τ/Δt`). The fuse's thermal headroom is used first, dimming then starts
///   gradually, and the average converges to `B` without ever exceeding it.
///
/// An output's target is the lowest need of its groups; scales drop at once
/// (attack) and recover by at most `Δt / 1.5 s` per frame (release), so a
/// strobe is dimmed evenly instead of pumping. Current is linear in the PWM
/// duty, so scaling the post-gamma bytes by `s` scales the current by `s`
/// and keeps the hue.
#[derive(Debug, Clone)]
pub struct Limiter {
    mode: LimiterMode,
    /// mA per pixel at full white, per output (0-based).
    ma_pp: Vec<f32>,
    groups: Vec<GroupState>,
    scale: Vec<f32>,
    raw: Vec<f32>,
    /// Seconds any group asked for less than 1.
    pub seconds_limited: f64,
}

impl Limiter {
    /// A limiter for `budget` (members and `mApp` keys are 1-based outputs).
    pub fn new(budget: &NodePowerBudget) -> Self {
        let max_out = budget
            .groups
            .iter()
            .flat_map(|g| g.members.iter().copied())
            .chain(budget.ma_pp.keys().copied())
            .max()
            .unwrap_or(0) as usize;
        let mut ma_pp = vec![Prop::DEFAULT_MA_PER_PIXEL; max_out];
        for (&o, &ma) in &budget.ma_pp {
            if o >= 1 && ma.is_finite() && ma >= 0.0 {
                ma_pp[o as usize - 1] = ma;
            }
        }
        let groups = budget
            .groups
            .iter()
            .filter(|g| g.budget_a.is_finite() && g.budget_a > 0.0)
            .map(|g| GroupState {
                id: g.id.clone(),
                budget: g.budget_a,
                tau_ms: g.tau_ms as f32,
                members: g
                    .members
                    .iter()
                    .filter(|&&o| o >= 1)
                    .map(|&o| o as usize - 1)
                    .collect(),
                ema_act: 0.0,
                now_act: 0.0,
                need: 1.0,
                est_slow: 0.0,
                correction: 1.0,
            })
            .collect();
        Limiter {
            mode: budget.mode,
            scale: vec![1.0; max_out],
            raw: vec![0.0; max_out],
            ma_pp,
            groups,
            seconds_limited: 0.0,
        }
    }

    pub fn mode(&self) -> LimiterMode {
        self.mode
    }

    /// Outputs (0-based) the limiter knows about.
    pub fn outputs(&self) -> usize {
        self.scale.len()
    }

    /// Current (A) of output `index` (0-based) for a byte sum of its wire data
    /// (`I = Σbytes / 765 × mApp`).
    pub fn amps_for(&self, index: usize, byte_sum: u64) -> f32 {
        let ma = self
            .ma_pp
            .get(index)
            .copied()
            .unwrap_or(Prop::DEFAULT_MA_PER_PIXEL);
        (byte_sum as f64 / 765.0 * f64::from(ma) / 1000.0) as f32
    }

    /// Advance by one frame: `amps[i]` is output `i`'s unscaled current (A),
    /// `dt_ms` the time since the previous frame. Returns the per-output scale
    /// to apply (all 1.0 unless the mode is `limit`).
    pub fn step(&mut self, amps: &[f32], dt_ms: f32) -> &[f32] {
        let dt = if dt_ms.is_finite() {
            dt_ms.clamp(0.0, 250.0)
        } else {
            0.0
        };
        for (i, r) in self.raw.iter_mut().enumerate() {
            *r = amps
                .get(i)
                .copied()
                .filter(|a| a.is_finite() && *a > 0.0)
                .unwrap_or(0.0);
        }
        let mut target = vec![1.0f32; self.scale.len()];
        let mut limiting = false;
        for g in &mut self.groups {
            let i_raw: f32 = g
                .members
                .iter()
                .map(|&m| self.raw.get(m).copied().unwrap_or(0.0))
                .sum::<f32>()
                * g.correction;
            g.need = if g.tau_ms <= 0.0 {
                // Supplies: this frame's current must fit.
                (g.budget / i_raw.max(1e-4)).min(1.0)
            } else {
                // Fuses / caps: the allowed current tapers from HEADROOM × B
                // while the average is cold to exactly B at the limit, so the
                // average approaches B smoothly and never exceeds it.
                let k = (dt / g.tau_ms).min(1.0);
                let m = HEADROOM.min(1.0 / k.max(1e-6)).max(1.0);
                let allowed = (m * g.budget - (m - 1.0) * g.ema_act).max(0.0);
                (allowed / i_raw.max(1e-4)).min(1.0)
            };
            if g.need < 0.999 {
                limiting = true;
            }
            for &m in &g.members {
                if let Some(t) = target.get_mut(m) {
                    *t = t.min(g.need);
                }
            }
        }
        let release = dt / RELEASE_MS;
        for (s, t) in self.scale.iter_mut().zip(&target) {
            *s = if *t < *s { *t } else { t.min(*s + release) };
        }
        // What actually flows: scaled when limiting, the raw current otherwise.
        let apply = self.mode == LimiterMode::Limit;
        for g in &mut self.groups {
            let k = if g.tau_ms <= 0.0 {
                1.0
            } else {
                (dt / g.tau_ms).min(1.0)
            };
            let act: f32 = g
                .members
                .iter()
                .map(|&m| {
                    let a = self.raw.get(m).copied().unwrap_or(0.0);
                    if apply {
                        a * self.scale.get(m).copied().unwrap_or(1.0)
                    } else {
                        a
                    }
                })
                .sum();
            g.est_slow += (act - g.est_slow) * (dt / 5_000.0).min(1.0);
            let act = act * g.correction;
            g.now_act = act;
            g.ema_act += (act - g.ema_act) * k;
        }
        if limiting {
            self.seconds_limited += f64::from(dt) / 1000.0;
        }
        &self.scale
    }

    /// Measured current of group `group_id` (a sensor on a supply, F20): the
    /// ratio to the estimate (averaged over 5 s) slowly corrects the estimate
    /// of that group, within ×0.5…×2 (the mA/pixel model is ±20 %). Ignored
    /// while little is lit.
    pub fn feedback(&mut self, group_id: &str, measured_a: f32) {
        let Some(g) = self.groups.iter_mut().find(|g| g.id == group_id) else {
            return;
        };
        if !(measured_a.is_finite() && measured_a >= 0.0) || g.est_slow < 0.3 {
            return;
        }
        let ratio = (measured_a / g.est_slow).clamp(0.5, 2.0);
        g.correction += (ratio - g.correction) * 0.3;
    }

    /// Scale output `index` (0-based) is (or, in `warn` mode, would be) drawn at.
    pub fn scale(&self, index: usize) -> f32 {
        self.scale.get(index).copied().unwrap_or(1.0)
    }

    /// Lowest output scale right now (1 = not limiting).
    pub fn min_scale(&self) -> f32 {
        self.scale.iter().copied().fold(1.0, f32::min)
    }

    /// Is any output scaled below 1?
    pub fn limiting(&self) -> bool {
        self.min_scale() < 0.995
    }

    /// Groups currently asking for less than full brightness.
    pub fn active_groups(&self) -> Vec<String> {
        self.groups
            .iter()
            .filter(|g| g.need < 0.995)
            .map(|g| g.id.clone())
            .collect()
    }

    /// Every group with its current, budget and requested scale.
    pub fn groups(&self) -> Vec<GroupLive> {
        self.groups
            .iter()
            .map(|g| GroupLive {
                id: g.id.clone(),
                amps: if g.tau_ms > 0.0 { g.ema_act } else { g.now_act },
                budget: g.budget,
                scale: g.need,
            })
            .collect()
    }
}

/// Current drawn from one power supply while a sequence plays (F12 supply view).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupplyPower {
    pub supply_id: String,
    pub name: String,
    pub volts: f32,
    /// Rating (A).
    pub rated_amps: f32,
    /// Highest sampled current (A).
    pub peak_amps: f32,
    /// Mean current (A).
    pub avg_amps: f32,
    /// `peakAmps × volts`.
    pub peak_watts: f32,
    /// `ok` below 90 % of the rating, `warn` up to it, `over` beyond it.
    pub status: PowerStatus,
}

/// A limiter group that would scale down (F12 planning view).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitedGroup {
    pub group_id: String,
    pub node_id: String,
    /// What the group is, for people ("Garage PSU", "Porch receiver port 2").
    pub label: String,
    /// Seconds of the sequence the group would be scaled down.
    pub seconds: f32,
    /// Lowest scale it would apply.
    pub min_scale: f32,
}

struct SupplyAccum {
    rows: Vec<(String, String, f32, f32, Vec<usize>)>,
    peak: Vec<f64>,
    sum: Vec<f64>,
}

impl SupplyAccum {
    fn new(show: &Show, plan: &Plan) -> Self {
        let rows: Vec<_> = show
            .power_supplies
            .iter()
            .map(|s| {
                let slots = supply_outputs(show, s)
                    .into_iter()
                    .filter_map(|(node, o)| {
                        plan.outputs
                            .iter()
                            .position(|p| show.nodes[p.node].id == node && p.output == o)
                    })
                    .collect();
                (s.id.clone(), s.name.clone(), s.volts, s.amps, slots)
            })
            .collect();
        let n = rows.len();
        SupplyAccum {
            rows,
            peak: vec![0.0; n],
            sum: vec![0.0; n],
        }
    }

    fn add(&mut self, frame_out_ma: &[f64]) {
        for (i, row) in self.rows.iter().enumerate() {
            let ma: f64 = row.4.iter().filter_map(|&s| frame_out_ma.get(s)).sum();
            self.sum[i] += ma;
            self.peak[i] = self.peak[i].max(ma);
        }
    }

    fn finish(self, samples: u32, warnings: &mut Vec<String>) -> Vec<SupplyPower> {
        let n = f64::from(samples.max(1));
        self.rows
            .into_iter()
            .enumerate()
            .map(|(i, (id, name, volts, rated, _))| {
                let peak = (self.peak[i] / 1000.0) as f32;
                let avg = (self.sum[i] / n / 1000.0) as f32;
                let status = if rated > 0.0 && peak > rated + 0.001 {
                    warnings.push(format!(
                        "{name}: peaks of {peak:.1} A exceed its {rated:.0} A rating; it may shut down. Turn on the power limiter or lower brightness."
                    ));
                    PowerStatus::Over
                } else if rated > 0.0 && peak > rated * 0.9 {
                    warnings.push(format!(
                        "{name}: peaks of {peak:.1} A are close to its {rated:.0} A rating."
                    ));
                    PowerStatus::Warn
                } else {
                    PowerStatus::Ok
                };
                SupplyPower {
                    supply_id: id,
                    name,
                    volts,
                    rated_amps: rated,
                    peak_amps: peak,
                    avg_amps: avg,
                    peak_watts: peak * volts,
                    status,
                }
            })
            .collect()
    }
}

/// Runs each node's [`Limiter`] over the sampled frames.
struct LimiterSim {
    dt_ms: f64,
    nodes: Vec<SimNode>,
}

struct SimNode {
    node_id: String,
    limiter: Limiter,
    /// Plan output slot → limiter output index (0-based).
    slots: Vec<(usize, usize)>,
    amps: Vec<f32>,
    seconds: BTreeMap<String, f64>,
    min: BTreeMap<String, f32>,
}

impl LimiterSim {
    fn new(show: &Show, plan: &Plan, dt_ms: f64) -> Self {
        let budgets = show_budgets(show);
        let nodes = budgets
            .into_iter()
            .filter(|(_, b)| !b.groups.is_empty())
            .map(|(node_id, b)| {
                let mut b = b;
                // Planning shows what *would* happen: simulate as if limiting.
                b.mode = LimiterMode::Limit;
                let limiter = Limiter::new(&b);
                let slots = plan
                    .outputs
                    .iter()
                    .enumerate()
                    .filter(|(_, o)| show.nodes[o.node].id == node_id && o.output >= 1)
                    .map(|(i, o)| (i, o.output as usize - 1))
                    .filter(|&(_, o)| o < limiter.outputs())
                    .collect();
                let amps = vec![0.0; limiter.outputs()];
                SimNode {
                    node_id,
                    limiter,
                    slots,
                    amps,
                    seconds: BTreeMap::new(),
                    min: BTreeMap::new(),
                }
            })
            .collect();
        LimiterSim { dt_ms, nodes }
    }

    fn add(&mut self, frame_out_ma: &[f64]) {
        for n in &mut self.nodes {
            n.amps.fill(0.0);
            for &(slot, o) in &n.slots {
                n.amps[o] = (frame_out_ma.get(slot).copied().unwrap_or(0.0) / 1000.0) as f32;
            }
            n.limiter.step(&n.amps, self.dt_ms as f32);
            for g in n.limiter.groups() {
                if g.scale < 0.995 {
                    *n.seconds.entry(g.id.clone()).or_default() += self.dt_ms / 1000.0;
                    let m = n.min.entry(g.id).or_insert(1.0);
                    *m = m.min(g.scale);
                }
            }
        }
    }

    fn finish(self, show: &Show) -> Vec<LimitedGroup> {
        let mut v = Vec::new();
        for n in self.nodes {
            for (id, secs) in n.seconds {
                v.push(LimitedGroup {
                    label: group_label(show, &id),
                    min_scale: (n.min.get(&id).copied().unwrap_or(1.0) * 1000.0).round() / 1000.0,
                    group_id: id,
                    node_id: n.node_id.clone(),
                    seconds: ((secs * 10.0).round() / 10.0) as f32,
                });
            }
        }
        v
    }
}

/// People-facing name of a limiter group id (`port:<rid>:<n>`, `bus:<rid>`,
/// `supply:<id>`, `global`).
pub fn group_label(show: &Show, group_id: &str) -> String {
    let mut parts = group_id.splitn(3, ':');
    let kind = parts.next().unwrap_or("");
    let a = parts.next().unwrap_or("");
    let b = parts.next().unwrap_or("");
    let receiver = |id: &str| {
        show.receivers
            .iter()
            .find(|r| r.id == id)
            .map_or_else(|| "a receiver".to_string(), |r| r.name.clone())
    };
    match kind {
        "port" => format!("{} port {b}", receiver(a)),
        "bus" => format!("{} (main fuse)", receiver(a)),
        "supply" => show
            .power_supplies
            .iter()
            .find(|s| s.id == a)
            .map_or_else(|| "a power supply".to_string(), |s| s.name.clone()),
        "global" => "the whole display (power cap)".to_string(),
        _ => group_id.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fseq::{FseqWriter, FseqWriterOptions};
    use crate::model::*;
    use std::io::Cursor;

    fn show() -> Show {
        let mut s = Show::default();
        s.nodes.push(Node {
            hardware_history: Default::default(),
            serial: Default::default(),
            id: "n1".into(),
            name: "Leader".into(),
            hostname: "pp".into(),
            role: NodeRole::Leader,
            board: BoardKind::Difftx,
            board_rev: None,
            pi_model: None,
            outputs: BoardKind::Difftx.default_outputs(),
            adopted: true,
            last_seen: None,
            notes: None,
        });
        let mk = |id: &str, px: u32, start: u32, out: u32| Prop {
            suspect_pixels: Default::default(),
            id: id.into(),
            name: id.to_uppercase(),
            kind: PropKind::Line,
            pixel_count: px,
            xlights_model: None,
            channel_start: start,
            channels_per_pixel: 3,
            channel_runs: None,
            segments: vec![PropSegment {
                node_id: "n1".into(),
                output: out,
                start_pixel: 0,
                pixel_count: px,
                prop_offset: 0,
                reverse: false,
                null_pixels: 0,
            }],
            group_ids: vec![],
            layout: None,
            matrix: None,
            color: None,
            max_milliamps_per_pixel: None,
            notes: None,
        };
        s.props.push(mk("a", 100, 0, 1)); // 6 A at full white
        s.props.push(mk("b", 50, 300, 2)); // 3 A at full white
        s.receivers.push(Receiver {
            main_fuse_amps: Default::default(),
            id: "r1".into(),
            name: "Garage".into(),
            kind: ReceiverKind::Diffrx,
            node_id: "n1".into(),
            jack: 1,
            location: Some("Garage".into()),
            fuse_amps: None,
            notes: None,
        });
        s
    }

    #[test]
    fn full_white_matches_hand_calculation() {
        let e = estimate_full_white(&show());
        assert_eq!(e.per_output.len(), 2);
        let o1 = &e.per_output[0];
        assert_eq!((o1.output, o1.pixels), (1, 100));
        assert!((o1.peak_amps - 6.0).abs() < 0.01, "{}", o1.peak_amps);
        assert!((o1.max_amps - 6.0).abs() < 0.01);
        assert!((e.per_output[1].peak_amps - 3.0).abs() < 0.01);
        assert!((e.per_node[0].peak_amps - 9.0).abs() < 0.02);
        // Port 1 averages exactly the 6 A hold current: not "over", but a warning.
        let p1 = &e.per_receiver_port[0];
        assert_eq!(p1.port, 1);
        assert_eq!(p1.fuse_amps, Some(6.0));
        assert_eq!(p1.status, PowerStatus::Warn);
        assert_eq!(e.per_receiver_port[1].status, PowerStatus::Ok); // 3 A < 4.1 A hot rating
    }

    #[test]
    fn sequence_sampling_brightness_and_fuse_verdicts() {
        let mut s = show();
        s.props[0].max_milliamps_per_pixel = Some(80.0); // 8 A full white on port 1
        s.nodes[0].outputs[1].brightness = 50;
        let mut w =
            FseqWriter::new(Cursor::new(Vec::new()), FseqWriterOptions::new(450, 25)).unwrap();
        for f in 0..100u32 {
            // Odd frames full white, even frames black.
            let v = if f % 2 == 1 { 255 } else { 0 };
            w.write_frame(&vec![v; 450]).unwrap();
        }
        let bytes = w.finish().unwrap().into_inner();
        let mut fseq = FseqFile::from_reader(Cursor::new(bytes)).unwrap();
        let e = estimate_power(
            &s,
            &mut fseq,
            &PowerOptions {
                sample_every: Some(1),
                target_samples: 0,
            },
        )
        .unwrap();
        assert_eq!(e.frames_sampled, 100);
        let o1 = &e.per_output[0];
        assert!((o1.peak_amps - 8.0).abs() < 0.01);
        assert!((o1.avg_amps - 4.0).abs() < 0.01);
        let o2 = &e.per_output[1];
        assert!(
            (o2.peak_amps - 1.5).abs() < 0.02,
            "brightness 50% halves 3 A: {}",
            o2.peak_amps
        );
        let ports = &e.per_receiver_port;
        assert_eq!(ports[0].status, PowerStatus::Warn); // peak 8 A > 6 A, avg 4 A < 6 A
        assert_eq!(ports[1].status, PowerStatus::Ok);
        assert!(e
            .warnings
            .iter()
            .any(|w| w.contains("Garage") && w.contains("port 1")));

        // Sampling every 2nd frame from frame 0 sees only black frames.
        let e2 = estimate_power(
            &s,
            &mut fseq,
            &PowerOptions {
                sample_every: Some(2),
                target_samples: 0,
            },
        )
        .unwrap();
        assert_eq!(e2.frames_sampled, 50);
        assert_eq!(e2.per_output[0].peak_amps, 0.0);

        // Sustained overload is "over".
        s.props[0].max_milliamps_per_pixel = Some(150.0); // 15 A at white, avg 7.5 A
        let e3 = estimate_power(&s, &mut fseq, &PowerOptions::default()).unwrap();
        assert_eq!(e3.per_receiver_port[0].status, PowerStatus::Over);
        let json = serde_json::to_string(&e3).unwrap();
        assert!(json.contains("\"perReceiverPort\"") && json.contains("\"peakAmps\""));
        assert!(json.contains("\"status\":\"over\""));
    }

    #[test]
    fn gamma_reduces_current() {
        let mut s = show();
        s.nodes[0].outputs[0].gamma = 2.2;
        let frame = vec![128u8; 450];
        let e = estimate_from_frames(&s, 1, |f| {
            f(&frame);
            Ok::<_, ()>(())
        })
        .unwrap();
        let linear = 6.0 * 128.0 / 255.0;
        assert!(e.per_output[0].peak_amps < linear * 0.5);
        assert!((e.per_output[1].peak_amps - 3.0 * 128.0 / 255.0).abs() < 0.02);
    }

    #[test]
    fn channel_runs_are_sampled_where_the_data_is() {
        // Prop "a": first 50 pixels at byte 0, last 50 at byte 600 (beyond prop "b").
        let mut s = show();
        s.props[0].channel_runs = Some(vec![
            ChannelRun {
                prop_offset: 0,
                channel_start: 0,
                pixel_count: 50,
            },
            ChannelRun {
                prop_offset: 50,
                channel_start: 600,
                pixel_count: 50,
            },
        ]);
        // Full white only where the second run lives: half the prop lights.
        let mut frame = vec![0u8; 750];
        frame[600..750].fill(255);
        let e = estimate_from_frames(&s, 1, |f| {
            f(&frame);
            Ok::<_, ()>(())
        })
        .unwrap();
        assert!(
            (e.per_output[0].peak_amps - 3.0).abs() < 0.01,
            "{}",
            e.per_output[0].peak_amps
        );
        // Bytes 150..300 (contiguous layout's second half) are ignored.
        let mut frame = vec![0u8; 750];
        frame[150..300].fill(255);
        let e = estimate_from_frames(&s, 1, |f| {
            f(&frame);
            Ok::<_, ()>(())
        })
        .unwrap();
        assert_eq!(e.per_output[0].peak_amps, 0.0);
        // Full-white estimate covers the far run.
        let e = estimate_full_white(&s);
        assert!((e.per_output[0].peak_amps - 6.0).abs() < 0.01);
    }

    #[test]
    fn empty_show_and_short_frames() {
        let e = estimate_full_white(&Show::default());
        assert!(e.per_output.is_empty());
        let s = show();
        let e = estimate_from_frames(&s, 1, |f| {
            f(&[255u8; 30]);
            Ok::<_, ()>(())
        })
        .unwrap();
        assert!((e.per_output[0].peak_amps - 0.6).abs() < 0.01);
        assert_eq!(e.per_output[1].peak_amps, 0.0);
    }

    // ----- F12 budgets and limiter ---------------------------------------

    fn budget(groups: Vec<PowerGroup>, mode: LimiterMode) -> NodePowerBudget {
        NodePowerBudget {
            mode,
            safety: 0.9,
            groups,
            ma_pp: [(1u32, 60.0f32), (2, 60.0)].into_iter().collect(),
        }
    }

    fn group(id: &str, budget: f32, tau: u32, members: Vec<u32>) -> PowerGroup {
        PowerGroup {
            id: id.into(),
            kind: PowerGroupKind::Supply,
            budget_a: budget,
            tau_ms: tau,
            members,
        }
    }

    #[test]
    fn pptc_derating() {
        assert_eq!(pptc_derate(None), PPTC_DERATE_UNKNOWN);
        assert!((pptc_derate(Some(23.0)) - 1.0).abs() < 1e-4);
        assert!((pptc_derate(Some(60.0)) - PPTC_DERATE_60C).abs() < 1e-4);
        assert!(pptc_derate(Some(200.0)) >= 0.4);
    }

    #[test]
    fn node_budget_groups_ports_bus_supply_global() {
        let mut s = show();
        s.settings.power.mode = LimiterMode::Limit;
        s.receivers[0].main_fuse_amps = Some(20.0);
        s.power_supplies.push(PowerSupply {
            id: "psu".into(),
            name: "Garage PSU".into(),
            volts: 12.0,
            amps: 10.0,
            receiver_ids: vec!["r1".into()],
            direct_outputs: vec![],
            sensor: None,
        });
        s.settings.power.global_watts = Some(120.0);
        let b = node_budget(&s, "n1").unwrap();
        assert_eq!(b.mode, LimiterMode::Limit);
        assert_eq!(b.ma_pp.get(&1), Some(&60.0));
        let g = |id: &str| {
            b.groups
                .iter()
                .find(|g| g.id == id)
                .unwrap_or_else(|| panic!("{id}"))
        };
        // 6 A diffrx fuse × 0.8 (temperature unknown) × 0.9 safety.
        let p1 = g("port:r1:1");
        assert_eq!(
            (p1.kind, p1.tau_ms, p1.members.clone()),
            (PowerGroupKind::Port, PORT_TAU_MS, vec![1])
        );
        assert!((p1.budget_a - 4.32).abs() < 1e-3, "{}", p1.budget_a);
        assert!(
            b.groups.iter().all(|g| g.id != "port:r1:3"),
            "unused ports have no group"
        );
        let bus = g("bus:r1");
        assert_eq!(bus.members, vec![1, 2]);
        assert!((bus.budget_a - 18.0).abs() < 1e-3);
        let psu = g("supply:psu");
        assert_eq!((psu.tau_ms, psu.members.clone()), (0, vec![1, 2]));
        assert!((psu.budget_a - 9.0).abs() < 1e-3);
        // 120 W at the wall × 0.85 / 12 V × 0.9.
        assert!((g("global").budget_a - 7.65).abs() < 1e-3);
        let json = serde_json::to_value(&b).unwrap();
        assert!(json["groups"][0]["budgetA"].is_number() && json["mApp"]["1"].is_number());
        s.settings.power.mode = LimiterMode::Off;
        assert!(node_budget(&s, "n1").is_none());
        assert!(node_budget(&show(), "nope").is_none());
    }

    #[test]
    fn supply_spanning_nodes_is_split_by_possible_current() {
        let mut s = show();
        s.settings.power.mode = LimiterMode::Limit;
        let mut n2 = s.nodes[0].clone();
        n2.id = "n2".into();
        n2.role = NodeRole::Follower;
        s.nodes.push(n2);
        s.props[1].segments[0].node_id = "n2".into(); // 3 A of the 9 A lives on n2
        s.power_supplies.push(PowerSupply {
            id: "psu".into(),
            name: "PSU".into(),
            volts: 12.0,
            amps: 10.0,
            receiver_ids: vec![],
            direct_outputs: vec![
                NodeOutputRef {
                    node_id: "n1".into(),
                    output: 1,
                },
                NodeOutputRef {
                    node_id: "n2".into(),
                    output: 2,
                },
            ],
            sensor: None,
        });
        let all = show_budgets(&s);
        let a = all["n1"]
            .groups
            .iter()
            .find(|g| g.id == "supply:psu")
            .unwrap();
        let b = all["n2"]
            .groups
            .iter()
            .find(|g| g.id == "supply:psu")
            .unwrap();
        assert!(
            (a.budget_a - 6.0).abs() < 0.01 && (b.budget_a - 3.0).abs() < 0.01,
            "{} {}",
            a.budget_a,
            b.budget_a
        );
        assert_eq!(b.members, vec![2]);
    }

    #[test]
    fn limiter_is_linear_and_instant_for_supplies() {
        let mut l = Limiter::new(&budget(
            vec![group("s", 3.0, 0, vec![1])],
            LimiterMode::Limit,
        ));
        // 100 white pixels at 60 mA = 6 A; 255 × 300 bytes.
        let a = l.amps_for(0, 255 * 300);
        assert!((a - 6.0).abs() < 1e-3);
        let s = l.step(&[a, 1.0], 25.0).to_vec();
        assert!((s[0] - 0.5).abs() < 1e-3, "attack is instant: {}", s[0]);
        assert_eq!(s[1], 1.0, "output 2 has no group");
        assert!(l.limiting() && l.active_groups() == vec!["s".to_string()]);
        // Under budget: releases at 1/1.5 s per second, not at once.
        l.step(&[1.0], 25.0);
        assert!((l.scale(0) - (0.5 + 25.0 / RELEASE_MS)).abs() < 1e-4);
        for _ in 0..80 {
            l.step(&[1.0], 25.0);
        }
        assert_eq!(l.scale(0), 1.0);
        assert!(l.seconds_limited > 0.0);
    }

    #[test]
    fn strobe_does_not_pump() {
        // 2 Hz strobe, 6 A white vs a 3 A supply: every white frame gets the
        // same scale (no visible brightness wobble between flashes).
        let mut l = Limiter::new(&budget(
            vec![group("s", 3.0, 0, vec![1])],
            LimiterMode::Limit,
        ));
        let mut white = vec![];
        for f in 0..400 {
            let on = (f * 25 / 250) % 2 == 0;
            let s = l.step(&[if on { 6.0 } else { 0.0 }], 25.0)[0];
            if on && f > 40 {
                white.push(s);
            }
        }
        let (lo, hi) = white
            .iter()
            .fold((1.0f32, 0.0f32), |(a, b), &s| (a.min(s), b.max(s)));
        assert!(
            (lo - 0.5).abs() < 1e-3 && (hi - 0.5).abs() < 1e-3,
            "{lo}..{hi}"
        );
    }

    #[test]
    fn thermal_ema_converges_without_overshoot() {
        // Constant 2× overload of a PPTC port at 40 Hz for 60 s.
        let mut l = Limiter::new(&budget(
            vec![group("p", 4.0, PORT_TAU_MS, vec![1])],
            LimiterMode::Limit,
        ));
        let mut ema = 0.0f32;
        let mut prev = 1.0f32;
        let mut max_step = 0.0f32;
        for _ in 0..(60 * 40) {
            let s = l.step(&[8.0], 25.0)[0];
            ema += (8.0 * s - ema) * (25.0 / PORT_TAU_MS as f32);
            assert!(ema <= 4.0 + 1e-3, "thermal average overshoots: {ema}");
            max_step = max_step.max((prev - s).abs());
            prev = s;
        }
        assert!((ema - 4.0).abs() < 0.05, "converges to the budget: {ema}");
        assert!((l.scale(0) - 0.5).abs() < 0.01);
        // Gradual: no single frame changes brightness by more than 2 %.
        assert!(max_step < 0.02, "dimming is gradual: {max_step}");
        // The first seconds are not limited at all (the fuse is still cool).
        let mut l = Limiter::new(&budget(
            vec![group("p", 4.0, PORT_TAU_MS, vec![1])],
            LimiterMode::Limit,
        ));
        for _ in 0..40 {
            l.step(&[8.0], 25.0);
        }
        assert_eq!(l.scale(0), 1.0);
    }

    #[test]
    fn warn_mode_reports_but_draws_full_current() {
        let mut l = Limiter::new(&budget(
            vec![group("s", 3.0, 0, vec![1])],
            LimiterMode::Warn,
        ));
        let s = l.step(&[6.0], 25.0)[0];
        assert!((s - 0.5).abs() < 1e-3, "reports what it would do");
        assert_eq!(l.mode(), LimiterMode::Warn);
        let g = &l.groups()[0];
        assert!(
            (g.amps - 6.0).abs() < 1e-3,
            "the full current flows: {}",
            g.amps
        );
        // Garbage in: nothing panics, nothing limits.
        let mut l = Limiter::new(&NodePowerBudget::default());
        assert!(l.step(&[f32::NAN, -1.0], f32::INFINITY).is_empty());
    }

    #[test]
    fn estimate_has_supply_view_and_simulated_limiting() {
        let mut s = show();
        s.settings.power.mode = LimiterMode::Warn;
        s.power_supplies.push(PowerSupply {
            id: "psu".into(),
            name: "Garage PSU".into(),
            volts: 12.0,
            amps: 5.0,
            receiver_ids: vec!["r1".into()],
            direct_outputs: vec![],
            sensor: None,
        });
        let mut w =
            FseqWriter::new(Cursor::new(Vec::new()), FseqWriterOptions::new(450, 25)).unwrap();
        for _ in 0..400 {
            w.write_frame(&[255u8; 450]).unwrap(); // 9 A for 10 s
        }
        let bytes = w.finish().unwrap().into_inner();
        let mut fseq = FseqFile::from_reader(Cursor::new(bytes)).unwrap();
        let e = estimate_power(
            &s,
            &mut fseq,
            &PowerOptions {
                sample_every: Some(1),
                target_samples: 0,
            },
        )
        .unwrap();
        let sp = &e.per_supply[0];
        assert!((sp.peak_amps - 9.0).abs() < 0.02 && sp.status == PowerStatus::Over);
        assert!((sp.peak_watts - 108.0).abs() < 0.5);
        let lim = e
            .limited
            .iter()
            .find(|l| l.group_id == "supply:psu")
            .unwrap();
        assert!((lim.seconds - 10.0).abs() < 0.1, "{}", lim.seconds);
        assert!((lim.min_scale - 0.5).abs() < 0.01, "{}", lim.min_scale);
        assert_eq!(lim.label, "Garage PSU");
        assert!(e
            .warnings
            .iter()
            .any(|w| w.contains("power limiter would dim")));
        let json = serde_json::to_value(&e).unwrap();
        assert!(json["perSupply"][0]["peakWatts"].is_number());
        assert!(json["limited"][0]["minScale"].is_number());
        // No supplies, limiter off: the old shape.
        let json = serde_json::to_value(estimate_full_white(&show())).unwrap();
        assert!(json.get("perSupply").is_none() && json.get("limited").is_none());
        assert_eq!(group_label(&s, "port:r1:2"), "Garage port 2");
        assert_eq!(group_label(&s, "bus:zz"), "a receiver (main fuse)");
    }

    #[test]
    fn measured_current_corrects_the_estimate() {
        // The estimate says 6 A against a 7 A supply (no limiting), but the
        // sensor measures 30 % more: after a few readings the limiter dims.
        let mut l = Limiter::new(&budget(vec![group("s", 7.0, 0, vec![1])], LimiterMode::Limit));
        for _ in 0..(6 * 40) {
            l.step(&[6.0], 25.0);
        }
        assert_eq!(l.scale(0), 1.0);
        for _ in 0..40 {
            // The real current is 1.3 × what the estimate says is drawn
            // (a sensor node reports about every second here).
            let drawn = 6.0 * l.scale(0);
            l.feedback("s", 1.3 * drawn);
            for _ in 0..40 {
                l.step(&[6.0], 25.0);
            }
        }
        let s = l.scale(0);
        assert!((s - 7.0 / 7.8).abs() < 0.03, "{s}");
        // Readings with little lit, unknown groups and garbage change nothing.
        l.feedback("nope", 1.0);
        l.feedback("s", f32::NAN);
        let mut quiet = Limiter::new(&budget(vec![group("s", 7.0, 0, vec![1])], LimiterMode::Limit));
        quiet.step(&[0.1], 25.0);
        quiet.feedback("s", 50.0);
        quiet.step(&[6.0], 25.0);
        assert_eq!(quiet.scale(0), 1.0);
    }
}
