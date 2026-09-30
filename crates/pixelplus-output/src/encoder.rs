//! WS281x → DPI framebuffer encoder.
//!
//! The framebuffer is **XRGB8888**: one little-endian `u32` per DPI pixel whose
//! bit *n* (0..=23) appears on DPI_D*n* = GPIO *n + 4*; bits 24..=31 are unused.
//!
//! ## Line layout
//!
//! Line *y* carries LED *y* of every output: 24 WS281x bits, MSB of the first
//! wire byte first, each `px_per_bit` pixels wide (see [`BitTiming`]).
//!
//! **Direct mode** (difftx, diffsmart), per bit:
//!
//! ```text
//! px 0 .. T0H        active mask   (every output that still has data goes high)
//! px T0H .. T1H      data word     (only outputs sending a 1 stay high)
//! px T1H .. bit end  0             (static: written once per buffer)
//! ```
//!
//! **Latched mode** (difftxlarge), per bit and per bank *b*, three latch time
//! slots of [`LATCH_SLOT_PX`] pixels at the three bit edges, offset `4 b`:
//!
//! ```text
//! edge 0    (px 4b ..)        mask | LE_b on the middle two pixels
//! edge T0H  (px T0H + 4b ..)  data | LE_b on the middle two pixels
//! edge T1H  (px T1H + 4b ..)  0    | LE_b on the middle two pixels  (static)
//! ```
//!
//! A 74AHCT573 is transparent while LE is high, so bank *b*'s outputs change
//! at pixel `edge + 4b + 1`. Every edge of a bank moves by the same offset, so
//! high times are exactly T0H / T1H on every bank; banks are merely skewed by
//! 104 ns from each other, which WS281x strings do not care about.
//!
//! ## Latch alignment ("bottom-aligned" strings, experimental)
//!
//! By default every output's data starts at line 0, so a string of *L* LEDs
//! latches (shows its new colours) about `L × line + reset` after scan-out
//! starts: strings of different lengths change at different times (up to
//! 49 ms apart at 1600 LEDs). With [`WsEncoder::set_bottom_align`] output *k*
//! starts at line `N − L_k` instead, where *N* is the longest output of the
//! frame; the leading low lines are just a longer reset, and every string
//! latches at `N × line + reset`. Must be validated on real pixels (see
//! `DESIGN.md`, bring-up checklist).
//!
//! ## Speed
//!
//! Eight outputs' bytes are packed into a `u64` and bit-transposed with three
//! delta swaps, giving eight "one bit of every output" bytes at once; a
//! per-group 256-entry table then scatters those bytes onto the board's DPI
//! bits. Static regions are written only when a buffer is first used
//! ([`BufferState`]), so a steady-state frame writes 28 of 48 pixels per bit in
//! direct mode and 24 of 48 in latched mode.

use crate::error::{OutputError, Result};
use crate::frame::OutputFrameRef;
use crate::layout::{OutputLayout, OutputMode};
use crate::timing::{BitTiming, DpiGeometry, BITS_PER_LED, LATCH_SLOT_PX};
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ENCODER_ID: AtomicU64 = AtomicU64::new(1);

/// Transpose an 8×8 bit matrix stored row-major in a `u64`
/// (row *r* = byte *r*, column *c* = bit *c* of that byte).
///
/// After the call, bit *i* of byte *j* equals bit *j* of input byte *i*.
#[inline(always)]
pub fn transpose8(mut x: u64) -> u64 {
    let t = (x ^ (x >> 7)) & 0x00AA_00AA_00AA_00AA;
    x ^= t ^ (t << 7);
    let t = (x ^ (x >> 14)) & 0x0000_CCCC_0000_CCCC;
    x ^= t ^ (t << 14);
    let t = (x ^ (x >> 28)) & 0x0000_0000_F0F0_F0F0;
    x ^= t ^ (t << 28);
    x
}

/// A mutable view of a mapped framebuffer, in 32-bit pixels.
#[derive(Debug)]
pub struct FrameBufferMut<'a> {
    words: &'a mut [u32],
    width: usize,
    height: usize,
    stride: usize,
}

impl<'a> FrameBufferMut<'a> {
    /// Wrap `words`; `stride` is the distance between lines in pixels (≥ `width`).
    pub fn new(words: &'a mut [u32], width: usize, height: usize, stride: usize) -> Result<Self> {
        check_shape(words.len(), width, height, stride)?;
        Ok(FrameBufferMut {
            words,
            width,
            height,
            stride,
        })
    }

    /// Wrap a byte mapping (e.g. an mmapped DRM dumb buffer). `pitch` is in bytes.
    pub fn from_bytes(
        bytes: &'a mut [u8],
        width: usize,
        height: usize,
        pitch: usize,
    ) -> Result<Self> {
        if pitch % 4 != 0 {
            return Err(OutputError::InvalidFrame(format!(
                "framebuffer pitch {pitch} is not a multiple of 4 bytes"
            )));
        }
        // SAFETY: every bit pattern is a valid u32; align_to_mut only yields a
        // correctly aligned middle slice.
        let (head, words, _) = unsafe { bytes.align_to_mut::<u32>() };
        if !head.is_empty() {
            return Err(OutputError::InvalidFrame(
                "framebuffer mapping is not 4-byte aligned".into(),
            ));
        }
        Self::new(words, width, height, pitch / 4)
    }

    /// Width in pixels.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Height in lines.
    pub fn height(&self) -> usize {
        self.height
    }

    /// Read-only view of the same memory.
    pub fn as_ref(&self) -> FrameBufferRef<'_> {
        FrameBufferRef {
            words: self.words,
            width: self.width,
            height: self.height,
            stride: self.stride,
        }
    }

    fn line_mut(&mut self, y: usize) -> &mut [u32] {
        let start = y * self.stride;
        &mut self.words[start..start + self.width]
    }
}

/// A read-only framebuffer view (used by the decoder).
#[derive(Debug, Clone, Copy)]
pub struct FrameBufferRef<'a> {
    words: &'a [u32],
    width: usize,
    height: usize,
    stride: usize,
}

impl<'a> FrameBufferRef<'a> {
    /// Wrap `words`; `stride` is the distance between lines in pixels (≥ `width`).
    pub fn new(words: &'a [u32], width: usize, height: usize, stride: usize) -> Result<Self> {
        check_shape(words.len(), width, height, stride)?;
        Ok(FrameBufferRef {
            words,
            width,
            height,
            stride,
        })
    }

    /// Width in pixels.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Height in lines.
    pub fn height(&self) -> usize {
        self.height
    }

    /// Pixels of line `y` (empty if out of range).
    pub fn line(&self, y: usize) -> &'a [u32] {
        if y >= self.height {
            return &[];
        }
        let start = y * self.stride;
        &self.words[start..start + self.width]
    }
}

fn check_shape(len: usize, width: usize, height: usize, stride: usize) -> Result<()> {
    if stride < width {
        return Err(OutputError::InvalidFrame(format!(
            "stride {stride} is smaller than width {width}"
        )));
    }
    let needed = match height {
        0 => 0,
        h => (h - 1)
            .checked_mul(stride)
            .and_then(|n| n.checked_add(width))
            .ok_or_else(|| OutputError::InvalidFrame("framebuffer size overflows".into()))?,
    };
    if len < needed {
        return Err(OutputError::InvalidFrame(format!(
            "framebuffer has {len} pixels, {width}×{height} (stride {stride}) needs {needed}"
        )));
    }
    Ok(())
}

/// What an encoder last wrote into one particular framebuffer.
///
/// Keep one per physical buffer (e.g. per DRM dumb buffer) so the encoder
/// can skip regions that are already correct.
#[derive(Debug, Clone, Default)]
pub struct BufferState {
    encoder_id: u64,
    dirty_lines: u32,
}

impl BufferState {
    /// A state that forces a full rewrite on next use.
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget everything (e.g. after something else drew into the buffer).
    pub fn invalidate(&mut self) {
        *self = Self::default();
    }
}

/// Statistics of one [`WsEncoder::encode`] call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncodeReport {
    /// Lines (LEDs) carrying data this frame.
    pub data_lines: u32,
    /// Lines rewritten (data lines plus lines cleared from a longer previous frame).
    pub lines_written: u32,
    /// Outputs that were longer than the frame geometry and got cut short.
    pub truncated_outputs: u32,
    /// The whole buffer (template) was rewritten.
    pub full_rewrite: bool,
}

#[derive(Debug, Clone)]
struct Group {
    outputs: [usize; 8],
    len: usize,
    lut: Box<[u32; 256]>,
}

#[derive(Debug, Clone)]
struct LaneTables {
    le: u32,
    slot: usize,
    groups: Vec<Group>,
}

/// Per-lane values of one line.
#[derive(Clone, Copy)]
struct LaneLine {
    mask: u32,
    words: [u32; BITS_PER_LED as usize],
}

const EMPTY_LANE_LINE: LaneLine = LaneLine {
    mask: 0,
    words: [0; BITS_PER_LED as usize],
};

/// Maximum number of lanes (latch banks) supported by one encoder.
pub const MAX_LANES: usize = 6;

/// Encodes per-output WS281x byte streams into DPI framebuffer pixels.
#[derive(Debug, Clone)]
pub struct WsEncoder {
    id: u64,
    layout: OutputLayout,
    geometry: DpiGeometry,
    lanes: Vec<LaneTables>,
    bottom_align: bool,
}

impl WsEncoder {
    /// Build an encoder; fails if the layout's latch banks do not fit between
    /// the geometry's bit edges.
    pub fn new(layout: OutputLayout, geometry: DpiGeometry) -> Result<WsEncoder> {
        geometry.validate()?;
        geometry
            .bit
            .validate(geometry.pixel_clock_hz, layout.latch_banks())?;
        if layout.lanes().len() > MAX_LANES {
            return Err(OutputError::InvalidLayout(format!(
                "{} lanes exceed the maximum of {MAX_LANES}",
                layout.lanes().len()
            )));
        }
        let latched = layout.mode() == OutputMode::Latched;
        let lanes = layout
            .lanes()
            .iter()
            .enumerate()
            .map(|(li, lane)| LaneTables {
                le: lane.le_bit.map_or(0, |b| 1 << b),
                slot: if latched {
                    li * LATCH_SLOT_PX as usize
                } else {
                    0
                },
                groups: lane
                    .outputs
                    .chunks(8)
                    .map(|chunk| {
                        let mut outputs = [0usize; 8];
                        let mut bits = [0u32; 8];
                        for (i, &(o, b)) in chunk.iter().enumerate() {
                            outputs[i] = o;
                            bits[i] = 1 << b;
                        }
                        let mut lut = Box::new([0u32; 256]);
                        for (v, entry) in lut.iter_mut().enumerate() {
                            *entry = (0..chunk.len())
                                .filter(|i| v & (1 << i) != 0)
                                .fold(0, |acc, i| acc | bits[i]);
                        }
                        Group {
                            outputs,
                            len: chunk.len(),
                            lut,
                        }
                    })
                    .collect(),
            })
            .collect();
        Ok(WsEncoder {
            id: NEXT_ENCODER_ID.fetch_add(1, Ordering::Relaxed),
            layout,
            geometry,
            lanes,
            bottom_align: false,
        })
    }

    /// Start every output's data so that all outputs end on the same line
    /// (the longest output's last line): every string then latches at the
    /// same moment. See the module docs. Off by default.
    pub fn set_bottom_align(&mut self, on: bool) {
        self.bottom_align = on;
    }

    /// Whether outputs are bottom-aligned (see [`WsEncoder::set_bottom_align`]).
    pub fn bottom_align(&self) -> bool {
        self.bottom_align
    }

    /// The layout this encoder drives.
    pub fn layout(&self) -> &OutputLayout {
        &self.layout
    }

    /// The geometry this encoder writes.
    pub fn geometry(&self) -> &DpiGeometry {
        &self.geometry
    }

    /// Encode `frame` into `fb`.
    ///
    /// Outputs longer than [`DpiGeometry::pixels_per_output`] are truncated
    /// (reported in [`EncodeReport::truncated_outputs`]); a frame with more
    /// outputs than the layout is an error.
    pub fn encode(
        &self,
        frame: &OutputFrameRef<'_>,
        fb: &mut FrameBufferMut<'_>,
        state: &mut BufferState,
    ) -> Result<EncodeReport> {
        if frame.len() > self.layout.output_count() {
            return Err(OutputError::TooManyOutputs {
                got: frame.len(),
                max: self.layout.output_count(),
            });
        }
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
        let mut report = EncodeReport::default();
        if state.encoder_id != self.id {
            self.write_template(fb);
            state.encoder_id = self.id;
            state.dirty_lines = 0;
            report.full_rewrite = true;
        }

        let capacity = g.pixels_per_output as usize;
        let longest = frame.max_pixels();
        report.truncated_outputs = frame
            .iter()
            .filter(|o| o.len().div_ceil(3) > capacity)
            .count() as u32;
        let data_lines = longest.min(capacity);
        let lines = data_lines.max(state.dirty_lines as usize);
        // Bottom alignment: output `o` starts `shift[o]` lines late.
        let shifts: Vec<usize> = if self.bottom_align {
            frame
                .iter()
                .map(|o| data_lines - o.len().div_ceil(3).min(capacity))
                .collect()
        } else {
            Vec::new()
        };

        let mut lane_lines = [EMPTY_LANE_LINE; MAX_LANES];
        for y in 0..lines {
            if y < data_lines {
                for (tables, out) in self.lanes.iter().zip(lane_lines.iter_mut()) {
                    *out = Self::compute_lane_line(tables, frame, y, &shifts);
                }
            } else {
                lane_lines = [EMPTY_LANE_LINE; MAX_LANES];
            }
            self.write_line(fb.line_mut(y), &lane_lines[..self.lanes.len()], false);
        }
        state.dirty_lines = data_lines as u32;
        report.data_lines = data_lines as u32;
        report.lines_written = lines as u32;
        Ok(report)
    }

    /// Encode into a freshly allocated, tightly packed buffer of
    /// `hactive × vactive` pixels (for simulation and tests).
    pub fn encode_to_vec(&self, frame: &OutputFrameRef<'_>) -> Result<Vec<u32>> {
        let w = self.geometry.hactive() as usize;
        let h = self.geometry.vactive() as usize;
        let mut words = vec![0u32; w * h];
        let mut fb = FrameBufferMut::new(&mut words, w, h, w)?;
        self.encode(frame, &mut fb, &mut BufferState::new())?;
        Ok(words)
    }

    /// Write everything that never changes, and zero everything else.
    fn write_template(&self, fb: &mut FrameBufferMut<'_>) {
        let data_lines = self.geometry.pixels_per_output as usize;
        let empty = [EMPTY_LANE_LINE; MAX_LANES];
        for y in 0..fb.height() {
            let line = fb.line_mut(y);
            if y < data_lines {
                line.fill(0);
                self.write_line(line, &empty[..self.lanes.len()], true);
            } else {
                line.fill(0);
            }
        }
    }

    #[inline]
    fn compute_lane_line(
        tables: &LaneTables,
        frame: &OutputFrameRef<'_>,
        y: usize,
        shifts: &[usize],
    ) -> LaneLine {
        let mut out = EMPTY_LANE_LINE;
        for group in &tables.groups {
            let mut active = 0usize;
            let mut rows = [0u64; 3];
            for (i, &o) in group.outputs[..group.len].iter().enumerate() {
                let shift = shifts.get(o).copied().unwrap_or(0);
                if y < shift {
                    continue; // bottom-aligned: this output has not started yet
                }
                let base = (y - shift) * 3;
                let bytes = frame.output(o);
                if let Some(px) = bytes.get(base..base + 3) {
                    active |= 1 << i;
                    rows[0] |= u64::from(px[0]) << (8 * i);
                    rows[1] |= u64::from(px[1]) << (8 * i);
                    rows[2] |= u64::from(px[2]) << (8 * i);
                } else if base < bytes.len() {
                    // Trailing partial LED: pad the missing bytes with zero.
                    active |= 1 << i;
                    for (c, row) in rows.iter_mut().enumerate() {
                        *row |= u64::from(bytes.get(base + c).copied().unwrap_or(0)) << (8 * i);
                    }
                }
            }
            if active == 0 {
                continue;
            }
            out.mask |= group.lut[active];
            for (c, &row) in rows.iter().enumerate() {
                if row == 0 {
                    continue;
                }
                let t = transpose8(row);
                // Wire bit j of a byte is its bit (7 - j): MSB first.
                for j in 0..8 {
                    let column = ((t >> (8 * (7 - j))) & 0xFF) as usize;
                    out.words[c * 8 + j] |= group.lut[column];
                }
            }
        }
        out
    }

    /// Write one line. With `full`, static regions are written too.
    #[inline]
    fn write_line(&self, line: &mut [u32], lanes: &[LaneLine], full: bool) {
        let BitTiming {
            px_per_bit,
            t0h_px,
            t1h_px,
        } = self.geometry.bit;
        let (ppb, t0h, t1h) = (px_per_bit as usize, t0h_px as usize, t1h_px as usize);
        match self.layout.mode() {
            OutputMode::Direct => {
                let lane = lanes.first().copied().unwrap_or(EMPTY_LANE_LINE);
                for (bit, px) in line
                    .chunks_exact_mut(ppb)
                    .enumerate()
                    .take(BITS_PER_LED as usize)
                {
                    px[..t0h].fill(lane.mask);
                    px[t0h..t1h].fill(lane.words[bit]);
                    if full {
                        px[t1h..].fill(0);
                    }
                }
            }
            OutputMode::Latched => {
                // Edge by edge, bank by bank: the slots of one edge are
                // adjacent, so the (write-combined) buffer is written
                // strictly in ascending address order.
                for (bit, px) in line
                    .chunks_exact_mut(ppb)
                    .enumerate()
                    .take(BITS_PER_LED as usize)
                {
                    for (tables, lane) in self.lanes.iter().zip(lanes) {
                        let s = tables.slot;
                        write_slot(&mut px[s..s + 4], lane.mask, tables.le);
                    }
                    for (tables, lane) in self.lanes.iter().zip(lanes) {
                        let s = t0h + tables.slot;
                        write_slot(&mut px[s..s + 4], lane.words[bit], tables.le);
                    }
                    if full {
                        for tables in &self.lanes {
                            let s = t1h + tables.slot;
                            write_slot(&mut px[s..s + 4], 0, tables.le);
                        }
                    }
                }
            }
        }
    }
}

/// One latch time slot: data set-up, LE high ×2, data hold.
#[inline(always)]
fn write_slot(px: &mut [u32], value: u32, le: u32) {
    px[0] = value;
    px[1] = value | le;
    px[2] = value | le;
    px[3] = value;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::OutputLayout;
    use pixelplus_core::model::BoardKind;

    fn naive_transpose(x: u64) -> u64 {
        let mut out = 0u64;
        for i in 0..8 {
            for j in 0..8 {
                if x & (1 << (8 * i + j)) != 0 {
                    out |= 1 << (8 * j + i);
                }
            }
        }
        out
    }

    #[test]
    fn transpose_matches_naive() {
        let mut x = 0x0123_4567_89AB_CDEFu64;
        for _ in 0..10_000 {
            assert_eq!(transpose8(x), naive_transpose(x));
            // xorshift
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
        }
        assert_eq!(transpose8(0), 0);
        assert_eq!(transpose8(u64::MAX), u64::MAX);
    }

    fn direct_encoder(pixels: u32) -> WsEncoder {
        WsEncoder::new(
            OutputLayout::for_board(BoardKind::Difftx),
            DpiGeometry::for_pixels(pixels).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn direct_single_bit_layout() {
        let enc = direct_encoder(4);
        // Port 1 (DPI bit 1) sends 0x80 0x00 0x01, Port 4 (bit 0) sends nothing.
        let p1 = [0x80u8, 0x00, 0x01];
        let frame = OutputFrameRef::new(vec![&p1]);
        let fb = enc.encode_to_vec(&frame).unwrap();
        let line0 = &fb[..1152];
        // Bit 0 (MSB of 0x80) is a 1: high for 27 px.
        assert!(line0[..12].iter().all(|&w| w == 0b10));
        assert!(line0[12..27].iter().all(|&w| w == 0b10));
        assert!(line0[27..48].iter().all(|&w| w == 0));
        // Bit 1 is a 0: high for 12 px only.
        assert!(line0[48..60].iter().all(|&w| w == 0b10));
        assert!(line0[60..96].iter().all(|&w| w == 0));
        // Bit 23 (LSB of 0x01) is a 1.
        let last = &line0[23 * 48..24 * 48];
        assert!(last[..27].iter().all(|&w| w == 0b10));
        assert!(last[27..].iter().all(|&w| w == 0));
        // Line 1: no data at all.
        assert!(fb[1152..2304].iter().all(|&w| w == 0));
    }

    #[test]
    fn latched_slots_and_le() {
        let enc = WsEncoder::new(
            OutputLayout::for_board(BoardKind::Difftxlarge),
            DpiGeometry::for_pixels(2).unwrap(),
        )
        .unwrap();
        // Output 21 = bank 1, data bit 1, first wire bit = 1.
        let mut outs: Vec<&[u8]> = vec![&[]; 22];
        let px = [0xFFu8, 0, 0];
        outs[21] = &px;
        let fb = enc.encode_to_vec(&OutputFrameRef::new(outs)).unwrap();
        let le1 = 1 << 22;
        let bit0 = &fb[..48];
        // Bank 1's edge-0 slot at px 4..8 carries mask bit 1 with LE1 in the middle.
        assert_eq!(&bit0[4..8], &[0b10, 0b10 | le1, 0b10 | le1, 0b10]);
        // Its T0H slot at px 16..20 carries the data bit.
        assert_eq!(&bit0[16..20], &[0b10, 0b10 | le1, 0b10 | le1, 0b10]);
        // Its T1H slot at 31..35 releases (zero data, LE pulses).
        assert_eq!(&bit0[31..35], &[0, le1, le1, 0]);
        // Bank 0 and 2 slots carry zero data but still latch.
        assert_eq!(&bit0[0..4], &[0, 1 << 23, 1 << 23, 0]);
        assert_eq!(&bit0[8..12], &[0, 1 << 21, 1 << 21, 0]);
        // Reset lines are all zero, no LE pulses.
        let reset = &fb[2 * 1152..];
        assert!(reset.iter().all(|&w| w == 0));
    }

    #[test]
    fn incremental_encode_clears_shrinking_frames() {
        let enc = direct_encoder(8);
        let w = 1152;
        let h = enc.geometry().vactive() as usize;
        let mut words = vec![0xDEAD_BEEFu32; w * h];
        let mut fb = FrameBufferMut::new(&mut words, w, h, w).unwrap();
        let mut state = BufferState::new();
        let long = [0xFFu8; 3 * 8];
        let r = enc
            .encode(&OutputFrameRef::new(vec![&long]), &mut fb, &mut state)
            .unwrap();
        assert!(r.full_rewrite);
        assert_eq!(r.data_lines, 8);
        let short = [0xFFu8; 3];
        let r = enc
            .encode(&OutputFrameRef::new(vec![&short]), &mut fb, &mut state)
            .unwrap();
        assert!(!r.full_rewrite);
        assert_eq!((r.data_lines, r.lines_written), (1, 8));
        let fresh = enc
            .encode_to_vec(&OutputFrameRef::new(vec![&short]))
            .unwrap();
        assert_eq!(words, fresh, "incremental result must equal a fresh encode");
    }

    #[test]
    fn bottom_align_shifts_short_outputs_down() {
        let mut enc = direct_encoder(8);
        enc.set_bottom_align(true);
        assert!(enc.bottom_align());
        // Port 1 (bit 1): 4 LEDs; port 2 (bit 2): 1 LED (all ones).
        let p1 = [0xFFu8; 12];
        let p2 = [0xFFu8; 3];
        let fb = enc
            .encode_to_vec(&OutputFrameRef::new(vec![&p1, &p2]))
            .unwrap();
        let line = |y: usize| &fb[y * 1152..(y + 1) * 1152];
        // Lines 0..3 carry only port 1; port 2 starts on line 3 (= 4 − 1).
        for y in 0..3 {
            assert_eq!(line(y)[0], 0b10, "line {y}");
        }
        assert_eq!(line(3)[0], 0b110);
        assert!(line(4).iter().all(|&w| w == 0));
        // Top-aligned (default): port 2 is on line 0.
        enc.set_bottom_align(false);
        let fb = enc
            .encode_to_vec(&OutputFrameRef::new(vec![&p1, &p2]))
            .unwrap();
        assert_eq!(fb[0], 0b110);
        assert_eq!(fb[1152], 0b10);
    }

    #[test]
    fn rejects_bad_input() {
        let enc = direct_encoder(4);
        let a = [0u8; 3];
        let too_many = OutputFrameRef::new(vec![&a; 5]);
        assert!(matches!(
            enc.encode_to_vec(&too_many),
            Err(OutputError::TooManyOutputs { got: 5, max: 4 })
        ));
        let mut small = vec![0u32; 10];
        let mut fb = FrameBufferMut::new(&mut small, 5, 2, 5).unwrap();
        assert!(enc
            .encode(
                &OutputFrameRef::new(vec![&a]),
                &mut fb,
                &mut BufferState::new()
            )
            .is_err());
        assert!(FrameBufferMut::new(&mut small, 5, 3, 5).is_err());
        assert!(FrameBufferMut::new(&mut small, 6, 1, 5).is_err());
    }

    #[test]
    fn truncation_is_reported() {
        let enc = direct_encoder(2);
        let long = [1u8; 3 * 5];
        let frame = OutputFrameRef::new(vec![&long]);
        let w = 1152;
        let h = enc.geometry().vactive() as usize;
        let mut words = vec![0u32; w * h];
        let mut fb = FrameBufferMut::new(&mut words, w, h, w).unwrap();
        let r = enc
            .encode(&frame, &mut fb, &mut BufferState::new())
            .unwrap();
        assert_eq!(r.truncated_outputs, 1);
        assert_eq!(r.data_lines, 2);
    }

    #[test]
    fn from_bytes_checks_alignment_and_pitch() {
        let mut bytes = vec![0u8; 4 * 1152 * 2 + 4];
        assert!(FrameBufferMut::from_bytes(&mut bytes, 1152, 2, 1151 * 4 + 2).is_err());
        // Whichever way the allocation is aligned, a misaligned view is refused.
        let aligned_at = bytes.as_ptr().align_offset(4);
        let misaligned = &mut bytes[aligned_at + 1..];
        assert!(FrameBufferMut::from_bytes(misaligned, 16, 1, 64).is_err());
    }
}
