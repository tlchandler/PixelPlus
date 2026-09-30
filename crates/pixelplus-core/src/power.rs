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
use crate::model::{Node, Prop, Show};
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
                });
                outputs.len() - 1
            });
            let s = &mut outputs[slot];
            s.pixels = s.pixels.saturating_add(count);
            let full: f64 = s.lut[255] as f64 * 3.0 / 256.0 / 765.0 * ma as f64 * count as f64;
            s.max_ma += full;
            if prop_slot[pi].is_none() {
                prop_slot[pi] = Some(props.len());
                props.push(pi);
            }
            taps.push(Tap {
                src: prop.channel_start as usize + seg.prop_offset as usize * 3,
                len: count as usize * 3,
                out: slot,
                prop: prop_slot[pi].expect("set above"),
                ma_per_unit: ma as f64 / 765.0 / 256.0,
            });
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
    estimate_from_frames(show, stride, |f| {
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
        .map(|p| p.channel_start as usize + p.channel_len() as usize)
        .max()
        .unwrap_or(0)
        .min(crate::fseq::MAX_FRAME_BYTES as usize);
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
    let (plan, mut warnings) = build_plan(show);
    let mut acc = Accum::new(&plan, show.nodes.len());
    feed(&mut |frame: &[u8]| acc.add_frame(&plan, frame))?;
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

    Ok(PowerEstimate {
        frames_sampled: acc.samples,
        sample_every: sample_every.max(1),
        per_output,
        per_receiver_port,
        per_prop,
        per_node,
        warnings,
    })
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
            id: id.into(),
            name: id.to_uppercase(),
            kind: PropKind::Line,
            pixel_count: px,
            xlights_model: None,
            channel_start: start,
            channels_per_pixel: 3,
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
}
