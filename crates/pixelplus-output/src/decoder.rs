//! Framebuffer → WS281x decoder: an independent model of the wire.
//!
//! The decoder replays a framebuffer exactly as the DPI peripheral scans it
//! out (active pixels, then horizontal blanking at level 0, then vertical
//! blanking), models the difftxlarge's transparent latches (outputs follow
//! the data lines while LE is high and hold otherwise), and then decodes each
//! output the way a WS281x chip does: measure every high pulse, classify it as
//! a `0` or `1`, and treat a long low as reset.
//!
//! Every pulse is checked against a [`Ws281xSpec`], so a round-trip test
//! verifies both the *data* and the *timing* the encoder produces.

use crate::encoder::FrameBufferRef;
use crate::error::{OutputError, Result};
use crate::frame::OutputFrameRef;
use crate::layout::{OutputLayout, OutputMode};
use crate::timing::{DpiGeometry, Ws281xSpec};
use serde::Serialize;

/// Violations kept per output (the total is always counted).
const MAX_VIOLATIONS_KEPT: usize = 8;

/// Smallest and largest observed value.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MinMax {
    /// Smallest observation.
    pub min: f64,
    /// Largest observation.
    pub max: f64,
}

impl MinMax {
    fn add(slot: &mut Option<MinMax>, v: f64) {
        match slot {
            Some(m) => {
                m.min = m.min.min(v);
                m.max = m.max.max(v);
            }
            None => *slot = Some(MinMax { min: v, max: v }),
        }
    }
}

/// What one output put on the wire during one frame period.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodedOutput {
    /// Decoded bytes, in wire order.
    pub bytes: Vec<u8>,
    /// High time of `0` bits.
    pub t0h_ns: Option<MinMax>,
    /// High time of `1` bits.
    pub t1h_ns: Option<MinMax>,
    /// Rising edge to rising edge within the frame.
    pub bit_period_ns: Option<MinMax>,
    /// Low time between bits within the frame.
    pub low_ns: Option<MinMax>,
    /// Low time from the last bit to the first bit of the next frame.
    pub reset_ns: f64,
    /// Human-readable timing problems (at most a few are kept).
    pub violations: Vec<String>,
    /// Total number of timing problems.
    pub violation_count: usize,
}

/// All outputs of one decoded frame.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodedFrame {
    /// One entry per layout output, in output order.
    pub outputs: Vec<DecodedOutput>,
    /// Latch-enable timing problems (latched layouts only): data not stable
    /// for at least one pixel before LE rises, while LE is high and one pixel
    /// after LE falls, or two banks' LE high at once. At most a few are kept.
    pub latch_violations: Vec<String>,
    /// Total number of latch timing problems.
    pub latch_violation_count: usize,
}

impl DecodedFrame {
    /// Total timing violations across all outputs (including latch timing).
    pub fn violation_count(&self) -> usize {
        self.outputs
            .iter()
            .map(|o| o.violation_count)
            .sum::<usize>()
            + self.latch_violation_count
    }

    /// Compare with the frame that was encoded. `capacity` is the geometry's
    /// pixels per output (longer outputs are expected to be truncated).
    /// Returns a description of the first difference.
    pub fn verify(&self, expected: &OutputFrameRef<'_>, capacity: usize) -> Result<(), String> {
        if self.latch_violation_count > 0 {
            return Err(format!(
                "{} latch timing violation(s), first: {}",
                self.latch_violation_count,
                self.latch_violations
                    .first()
                    .map(String::as_str)
                    .unwrap_or("?")
            ));
        }
        for (i, got) in self.outputs.iter().enumerate() {
            if got.violation_count > 0 {
                return Err(format!(
                    "output {}: {} timing violation(s), first: {}",
                    i + 1,
                    got.violation_count,
                    got.violations.first().map(String::as_str).unwrap_or("?")
                ));
            }
            let src = expected.output(i);
            let leds = src.len().div_ceil(3).min(capacity);
            let mut want = src[..src.len().min(leds * 3)].to_vec();
            want.resize(leds * 3, 0);
            if got.bytes != want {
                let at = got
                    .bytes
                    .iter()
                    .zip(&want)
                    .position(|(a, b)| a != b)
                    .unwrap_or(got.bytes.len().min(want.len()));
                return Err(format!(
                    "output {}: decoded {} bytes, expected {}; first difference at byte {at}",
                    i + 1,
                    got.bytes.len(),
                    want.len()
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
struct Track {
    high: bool,
    rise: u64,
    last_fall: u64,
    first_rise: Option<u64>,
    prev_rise: Option<u64>,
    ended: bool,
    acc: u8,
    nbits: u8,
    out: DecodedOutput,
}

impl Track {
    fn violation(&mut self, msg: String) {
        self.out.violation_count += 1;
        if self.out.violations.len() < MAX_VIOLATIONS_KEPT {
            self.out.violations.push(msg);
        }
    }

    fn rise(&mut self, t: u64, px_ns: f64, spec: &Ws281xSpec, reset_px: u64) {
        self.high = true;
        self.rise = t;
        if self.ended {
            return;
        }
        if let Some(prev) = self.prev_rise {
            let low = t - self.last_fall;
            if low >= reset_px {
                // A reset in the middle of the period: a second frame would
                // start here. The encoder never does this.
                self.violation(format!(
                    "reset ({:.1} µs low) inside the frame at {:.1} µs",
                    low as f64 * px_ns / 1000.0,
                    t as f64 * px_ns / 1000.0
                ));
                self.ended = true;
                return;
            }
            let low_ns = low as f64 * px_ns;
            MinMax::add(&mut self.out.low_ns, low_ns);
            if low_ns < spec.min_low_ns || low_ns > spec.max_low_ns {
                self.violation(format!(
                    "low time {low_ns:.0} ns outside {:.0}-{:.0} ns",
                    spec.min_low_ns, spec.max_low_ns
                ));
            }
            MinMax::add(&mut self.out.bit_period_ns, (t - prev) as f64 * px_ns);
        } else {
            self.first_rise = Some(t);
        }
        self.prev_rise = Some(t);
    }

    fn fall(&mut self, t: u64, px_ns: f64, spec: &Ws281xSpec) {
        self.high = false;
        self.last_fall = t;
        if self.ended {
            return;
        }
        let high_ns = (t - self.rise) as f64 * px_ns;
        let in_window = |(lo, hi): (f64, f64)| high_ns >= lo && high_ns <= hi;
        let bit = if in_window(spec.t0h_ns) {
            MinMax::add(&mut self.out.t0h_ns, high_ns);
            0
        } else if in_window(spec.t1h_ns) {
            MinMax::add(&mut self.out.t1h_ns, high_ns);
            1
        } else {
            self.violation(format!(
                "high time {high_ns:.0} ns fits neither T0H {:?} nor T1H {:?}",
                spec.t0h_ns, spec.t1h_ns
            ));
            u8::from(high_ns >= spec.threshold_ns())
        };
        self.acc = (self.acc << 1) | bit;
        self.nbits += 1;
        if self.nbits == 8 {
            self.out.bytes.push(self.acc);
            self.acc = 0;
            self.nbits = 0;
        }
    }

    fn finish(mut self, period: u64, px_ns: f64, spec: &Ws281xSpec) -> DecodedOutput {
        if self.high {
            self.violation("line is still high at the end of the frame".into());
        }
        match self.first_rise {
            None => self.out.reset_ns = period as f64 * px_ns,
            Some(first) => {
                let trailing = period.saturating_sub(self.last_fall);
                self.out.reset_ns = (trailing + first) as f64 * px_ns;
            }
        }
        if self.first_rise.is_some() && self.out.reset_ns < spec.reset_ns {
            let reset_ns = self.out.reset_ns;
            self.violation(format!(
                "reset {:.1} µs is shorter than {:.1} µs",
                reset_ns / 1000.0,
                spec.reset_ns / 1000.0
            ));
        }
        if self.nbits != 0 {
            let n = self.nbits;
            self.violation(format!("{n} stray bit(s) after the last whole byte"));
        }
        self.out
    }
}

/// Decodes a framebuffer produced for `layout` and `geometry`.
#[derive(Debug, Clone)]
pub struct WsDecoder {
    layout: OutputLayout,
    geometry: DpiGeometry,
    spec: Ws281xSpec,
}

impl WsDecoder {
    /// A decoder checking against [`Ws281xSpec::COMMON`].
    pub fn new(layout: OutputLayout, geometry: DpiGeometry) -> Self {
        WsDecoder {
            layout,
            geometry,
            spec: Ws281xSpec::COMMON,
        }
    }

    /// Check against a different chip specification.
    pub fn with_spec(mut self, spec: Ws281xSpec) -> Self {
        self.spec = spec;
        self
    }

    /// Replay one frame period and decode every output.
    pub fn decode(&self, fb: &FrameBufferRef<'_>) -> Result<DecodedFrame> {
        let g = &self.geometry;
        let width = g.hactive() as usize;
        let vactive = g.vactive() as usize;
        if fb.width() < width || fb.height() < vactive {
            return Err(OutputError::InvalidFrame(format!(
                "framebuffer is {}×{}, geometry needs {width}×{vactive}",
                fb.width(),
                fb.height()
            )));
        }
        let px_ns = g.pixel_ns();
        let reset_px = (self.spec.reset_ns / px_ns).ceil() as u64;
        let lanes = self.layout.lanes();
        let latched = self.layout.mode() == OutputMode::Latched;
        let data_masks: Vec<u32> = lanes
            .iter()
            .map(|l| l.outputs.iter().fold(0, |m, &(_, b)| m | (1 << b)))
            .collect();
        let le_masks: Vec<u32> = lanes
            .iter()
            .map(|l| l.le_bit.map_or(0, |b| 1 << b))
            .collect();

        let mut tracks: Vec<Track> = (0..self.layout.output_count())
            .map(|_| Track::default())
            .collect();
        let mut state = vec![0u32; lanes.len()];
        let mut t: u64 = 0;

        let mut feed = |word: u32, count: u64, t: &mut u64, tracks: &mut [Track]| {
            for (li, lane) in lanes.iter().enumerate() {
                let next = if latched {
                    if word & le_masks[li] != 0 {
                        word & data_masks[li]
                    } else {
                        state[li]
                    }
                } else {
                    word & data_masks[li]
                };
                let changed = next ^ state[li];
                if changed != 0 {
                    // Latch outputs change one pixel into the slot (when LE
                    // rises); direct outputs change with the pixel. Either way
                    // the change is at the start of this run.
                    for &(o, b) in &lane.outputs {
                        if changed & (1 << b) != 0 {
                            if let Some(track) = tracks.get_mut(o) {
                                if next & (1 << b) != 0 {
                                    track.rise(*t, px_ns, &self.spec, reset_px);
                                } else {
                                    track.fall(*t, px_ns, &self.spec);
                                }
                            }
                        }
                    }
                    state[li] = next;
                }
            }
            *t += count;
        };

        let h_blank = u64::from(g.h_blank_px());
        for y in 0..vactive {
            let line = &fb.line(y)[..width];
            let mut x = 0;
            while x < line.len() {
                let word = line[x];
                let run = line[x..].iter().take_while(|&&w| w == word).count();
                feed(word, run as u64, &mut t, &mut tracks);
                x += run;
            }
            feed(0, h_blank, &mut t, &mut tracks);
        }
        feed(
            0,
            u64::from(g.v_blank_lines()) * u64::from(g.htotal()),
            &mut t,
            &mut tracks,
        );
        let period = t;
        let (latch_violations, latch_violation_count) = if latched {
            self.check_latch_timing(fb, &data_masks, &le_masks)
        } else {
            (Vec::new(), 0)
        };
        Ok(DecodedFrame {
            outputs: tracks
                .into_iter()
                .map(|tr| tr.finish(period, px_ns, &self.spec))
                .collect(),
            latch_violations,
            latch_violation_count,
        })
    }

    /// Check every latch-enable pulse pixel by pixel (blanking included):
    /// the bank's data lines must hold one value from the pixel before LE
    /// rises (set-up) until the pixel after LE falls (hold), and no two LE
    /// lines may be high together. With the 4-pixel slot this guarantees
    /// ≥ 26 ns set-up and hold at the SN74AHCT573 at 38.4 MHz, and that a
    /// bank never latches data meant for another bank.
    fn check_latch_timing(
        &self,
        fb: &FrameBufferRef<'_>,
        data_masks: &[u32],
        le_masks: &[u32],
    ) -> (Vec<String>, usize) {
        let g = &self.geometry;
        let width = g.hactive() as usize;
        let htotal = g.htotal() as usize;
        let any_le = le_masks.iter().fold(0, |m, &l| m | l);
        let mut kept = Vec::new();
        let mut count = 0usize;
        let mut report = |y: usize, x: usize, msg: String| {
            count += 1;
            if kept.len() < MAX_VIOLATIONS_KEPT {
                kept.push(format!("line {y} pixel {x}: {msg}"));
            }
        };
        let mut prev = 0u32;
        let vactive = g.vactive() as usize;
        for y in 0..=vactive {
            // The last "line" is vertical blanking: all zero.
            let line: &[u32] = if y < vactive {
                &fb.line(y)[..width]
            } else {
                &[]
            };
            let span = if y < vactive { htotal } else { 1 };
            for x in 0..span {
                let w = line.get(x).copied().unwrap_or(0);
                if (w & any_le).count_ones() > 1 {
                    report(
                        y,
                        x,
                        format!("{} latch enables high at once", (w & any_le).count_ones()),
                    );
                }
                for (&le, &dm) in le_masks.iter().zip(data_masks) {
                    let (was, is) = (prev & le != 0, w & le != 0);
                    if (was || is) && (prev & dm) != (w & dm) {
                        let what = match (was, is) {
                            (false, true) => "changes as LE rises (no set-up pixel)",
                            (true, false) => "changes as LE falls (no hold pixel)",
                            _ => "changes while LE is high",
                        };
                        report(y, x, format!("data {what}"));
                    }
                }
                prev = w;
            }
        }
        (kept, count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder::{FrameBufferRef, WsEncoder};
    use pixelplus_core::model::BoardKind;

    fn round_trip(board: BoardKind, frame: &OutputFrameRef<'_>, pixels: u32) -> DecodedFrame {
        let layout = OutputLayout::for_board(board);
        let geometry = DpiGeometry::for_pixels(pixels).unwrap();
        let enc = WsEncoder::new(layout.clone(), geometry).unwrap();
        let words = enc.encode_to_vec(frame).unwrap();
        let w = geometry.hactive() as usize;
        let h = geometry.vactive() as usize;
        let fb = FrameBufferRef::new(&words, w, h, w).unwrap();
        let decoded = WsDecoder::new(layout, geometry).decode(&fb).unwrap();
        decoded.verify(frame, pixels as usize).unwrap();
        decoded
    }

    #[test]
    fn direct_round_trip_with_timing() {
        let a: Vec<u8> = (0..30).map(|i| (i * 37) as u8).collect();
        let b = [0xAAu8; 6];
        let c: [u8; 0] = [];
        let d = [0xFFu8, 0x00, 0x5A];
        let frame = OutputFrameRef::new(vec![&a, &b, &c, &d]);
        let dec = round_trip(BoardKind::Difftx, &frame, 12);
        let t0 = dec.outputs[0].t0h_ns.unwrap();
        let t1 = dec.outputs[0].t1h_ns.unwrap();
        assert!((t0.min - 312.5).abs() < 0.1 && (t0.max - 312.5).abs() < 0.1);
        assert!((t1.min - 729.2).abs() < 0.1);
        let period = dec.outputs[0].bit_period_ns.unwrap();
        assert!((period.min - 1250.0).abs() < 0.1);
        // The 24th bit of every LED is stretched by the 24-pixel h-blank.
        assert!((period.max - 1875.0).abs() < 0.1, "{period:?}");
        assert!(dec.outputs[2].bytes.is_empty());
        assert!(dec.outputs[0].reset_ns >= 280_000.0);
    }

    #[test]
    fn latched_round_trip_all_60() {
        let data: Vec<Vec<u8>> = (0..60)
            .map(|o| {
                (0..(o % 7 + 1) * 3)
                    .map(|i| (o * 13 + i * 29) as u8)
                    .collect()
            })
            .collect();
        let frame = OutputFrameRef::new(data.iter().map(Vec::as_slice).collect());
        let dec = round_trip(BoardKind::Difftxlarge, &frame, 8);
        assert_eq!(dec.violation_count(), 0);
        for out in &dec.outputs {
            let t0 = out.t0h_ns.unwrap();
            assert!((t0.min - 312.5).abs() < 0.1 && (t0.max - 312.5).abs() < 0.1);
        }
    }

    #[test]
    fn latch_hold_violation_is_detected() {
        let layout = OutputLayout::for_board(BoardKind::Difftxlarge);
        let geometry = DpiGeometry::for_pixels(2).unwrap();
        let enc = WsEncoder::new(layout.clone(), geometry).unwrap();
        let px = [0xFFu8; 3];
        let frame = OutputFrameRef::new(vec![&px]);
        let mut words = enc.encode_to_vec(&frame).unwrap();
        let fb = FrameBufferRef::new(&words, 1152, geometry.vactive() as usize, 1152).unwrap();
        let dec = WsDecoder::new(layout.clone(), geometry)
            .decode(&fb)
            .unwrap();
        assert_eq!(dec.latch_violation_count, 0, "{:?}", dec.latch_violations);
        // Bank 0's edge-0 slot is px 0..4 (data, data+LE, data+LE, data).
        // Drop the hold pixel: data changes in the same pixel LE falls.
        words[3] = 0;
        let fb = FrameBufferRef::new(&words, 1152, geometry.vactive() as usize, 1152).unwrap();
        let dec = WsDecoder::new(layout, geometry).decode(&fb).unwrap();
        assert!(dec.latch_violation_count >= 1);
        assert!(
            dec.latch_violations[0].contains("no hold"),
            "{:?}",
            dec.latch_violations
        );
        assert!(dec.verify(&frame, 2).is_err());
    }

    #[test]
    fn corrupted_framebuffer_is_detected() {
        let layout = OutputLayout::for_board(BoardKind::Difftx);
        let geometry = DpiGeometry::for_pixels(2).unwrap();
        let enc = WsEncoder::new(layout.clone(), geometry).unwrap();
        let px = [0u8; 3];
        let frame = OutputFrameRef::new(vec![&px]);
        let mut words = enc.encode_to_vec(&frame).unwrap();
        // Stretch the first high pulse of port 1 into the forbidden gap.
        for w in &mut words[12..20] {
            *w |= 0b10;
        }
        let fb = FrameBufferRef::new(&words, 1152, geometry.vactive() as usize, 1152).unwrap();
        let dec = WsDecoder::new(layout, geometry).decode(&fb).unwrap();
        assert_eq!(dec.outputs[0].violation_count, 1);
        assert!(dec.verify(&frame, 2).is_err());
    }

    #[test]
    fn undersized_framebuffer_rejected() {
        let layout = OutputLayout::for_board(BoardKind::Difftx);
        let geometry = DpiGeometry::for_pixels(2).unwrap();
        let words = vec![0u32; 1152];
        let fb = FrameBufferRef::new(&words, 1152, 1, 1152).unwrap();
        assert!(WsDecoder::new(layout, geometry).decode(&fb).is_err());
    }
}
