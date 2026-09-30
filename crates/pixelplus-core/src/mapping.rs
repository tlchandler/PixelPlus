//! The frame router: turns an fseq frame (absolute channel space) into per-output pixel
//! buffers for one node, following the props' segments (ARCHITECTURE §4.1).
//!
//! * [`NodeMap`] is built once per (show version, node) and holds a precomputed copy
//!   list. [`NodeMap::render`] is allocation-free and essentially a sequence of
//!   `memcpy`s, so it keeps up with 60 outputs × 1600 pixels at 40 fps on a Pi.
//! * [`OutputFrame`] is the node's pixel buffer: one flat RGB byte buffer laid out
//!   *output-major, pixel-order* — exactly the frame layout of a `.ppseq` slice
//!   (§7.3), so a follower can hand slice frames straight to the output encoder.
//! * [`PropMap`] answers "where is pixel *i* of prop *P* on this node?" and writes live
//!   content (effects, overlays, test patterns) into an [`OutputFrame`].
//!
//! Colour order, brightness and gamma are *not* applied here; that is the output
//! stage's job.

use std::collections::HashMap;

use crate::model::{Node, Prop, Show};

/// Errors building a map.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MappingError {
    /// The node id is not in the show.
    #[error("unknown node '{0}'")]
    UnknownNode(String),
}

/// Bytes per pixel handled by the router (RGB).
pub const BYTES_PER_PIXEL: usize = 3;

/// Sanity bound on the length of one output (pixels). Far beyond anything a WS281x
/// output can refresh (≈1600 px at 20 fps); segments reaching past it come from corrupt
/// or hostile show data and are dropped with a warning instead of allocating gigabytes.
pub const MAX_OUTPUT_PIXELS: u32 = 1 << 17;

// ---------------------------------------------------------------------------
// OutputFrame
// ---------------------------------------------------------------------------

/// Per-output RGB pixel buffers of one node, stored contiguously.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputFrame {
    data: Vec<u8>,
    /// Byte offset of each output; `offsets[i+1] - offsets[i]` = bytes of output `i`.
    offsets: Vec<usize>,
}

impl OutputFrame {
    /// Allocate a zeroed frame with the given pixel count per output (index 0 = output 1).
    pub fn new(pixels_per_output: &[u32]) -> Self {
        let mut offsets = Vec::with_capacity(pixels_per_output.len() + 1);
        let mut acc = 0usize;
        offsets.push(0);
        for &p in pixels_per_output {
            acc += p as usize * BYTES_PER_PIXEL;
            offsets.push(acc);
        }
        OutputFrame {
            data: vec![0; acc],
            offsets,
        }
    }

    /// Number of outputs.
    pub fn output_count(&self) -> usize {
        self.offsets.len() - 1
    }

    /// Pixel count of output `index0` (0-based); 0 if out of range.
    pub fn pixels(&self, index0: usize) -> usize {
        if index0 + 1 >= self.offsets.len() {
            return 0;
        }
        (self.offsets[index0 + 1] - self.offsets[index0]) / BYTES_PER_PIXEL
    }

    /// Pixel counts of all outputs.
    pub fn pixels_per_output(&self) -> Vec<u32> {
        (0..self.output_count())
            .map(|i| self.pixels(i) as u32)
            .collect()
    }

    /// RGB bytes of output `index0` (0-based). Empty if out of range.
    pub fn output(&self, index0: usize) -> &[u8] {
        if index0 + 1 >= self.offsets.len() {
            return &[];
        }
        &self.data[self.offsets[index0]..self.offsets[index0 + 1]]
    }

    /// Mutable RGB bytes of output `index0` (0-based). Empty if out of range.
    pub fn output_mut(&mut self, index0: usize) -> &mut [u8] {
        if index0 + 1 >= self.offsets.len() {
            return &mut [];
        }
        let (a, b) = (self.offsets[index0], self.offsets[index0 + 1]);
        &mut self.data[a..b]
    }

    /// Byte offset of output `index0` in [`OutputFrame::as_bytes`].
    pub fn output_offset(&self, index0: usize) -> Option<usize> {
        (index0 < self.output_count()).then(|| self.offsets[index0])
    }

    /// The whole frame, output-major (the `.ppseq` frame layout).
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Mutable access to the whole frame.
    pub fn as_bytes_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// Total bytes.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// True if the frame has no pixels at all.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Set every pixel to black.
    pub fn clear(&mut self) {
        self.data.fill(0);
    }

    /// Replace the frame contents with `bytes` (e.g. a `.ppseq` frame). Copies
    /// `min(len)` bytes and zeroes any remainder.
    pub fn load(&mut self, bytes: &[u8]) {
        let n = bytes.len().min(self.data.len());
        self.data[..n].copy_from_slice(&bytes[..n]);
        self.data[n..].fill(0);
    }
}

// ---------------------------------------------------------------------------
// NodeMap
// ---------------------------------------------------------------------------

/// One precomputed copy: `pixels` pixels from `src` (byte offset into the source frame)
/// to `dst` (byte offset into [`OutputFrame::as_bytes`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyRun {
    /// Byte offset in the source frame.
    pub src: u32,
    /// Byte offset in the output frame.
    pub dst: u32,
    /// Number of pixels.
    pub pixels: u32,
    /// Copy pixels in reverse order (last source pixel lands on `dst`).
    pub reverse: bool,
    /// 0-based output index (informational, used for inspection and tests).
    pub output: u16,
}

/// Precomputed router for one node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeMap {
    /// Node this map belongs to.
    pub node_id: String,
    pixels_per_output: Vec<u32>,
    runs: Vec<CopyRun>,
    /// Smallest source frame length that covers every run.
    source_len: usize,
    /// Problems found while building (segments on missing outputs, ...).
    pub warnings: Vec<String>,
}

impl NodeMap {
    /// Build the map for `node_id` from the show's props and segments.
    ///
    /// Each output's pixel count is the furthest segment end on it. Segments pointing
    /// at outputs the node does not have, or past the end of their prop, are clipped and
    /// reported in [`NodeMap::warnings`]. Props with `channelsPerPixel != 3` are skipped.
    pub fn build(show: &Show, node_id: &str) -> Result<NodeMap, MappingError> {
        let node = show
            .node(node_id)
            .ok_or_else(|| MappingError::UnknownNode(node_id.to_string()))?;
        Ok(Self::build_for_node(node, &show.props))
    }

    /// Like [`NodeMap::build`] but from a node and prop list directly (followers build
    /// from their manifest).
    pub fn build_for_node(node: &Node, props: &[Prop]) -> NodeMap {
        let output_count = node_output_count(node);
        let mut warnings = Vec::new();
        let mut pixels = vec![0u32; output_count];

        // Pass 1: output sizes.
        let mut pieces: Vec<(usize, u32, u32, u32, bool)> = Vec::new(); // (out, src, dstpix, count, rev)
        for prop in props {
            if prop.channels_per_pixel as usize != BYTES_PER_PIXEL {
                if prop.segments.iter().any(|s| s.node_id == node.id) {
                    warnings.push(format!(
                        "prop '{}' uses {} channels per pixel; only RGB is supported",
                        prop.name, prop.channels_per_pixel
                    ));
                }
                continue;
            }
            for seg in prop.segments.iter().filter(|s| s.node_id == node.id) {
                if seg.output == 0 || seg.output as usize > output_count {
                    warnings.push(format!(
                        "prop '{}' is wired to output {} but node '{}' has {} outputs",
                        prop.name, seg.output, node.name, output_count
                    ));
                    continue;
                }
                let avail = prop.pixel_count.saturating_sub(seg.prop_offset);
                let count = seg.pixel_count.min(avail);
                if count < seg.pixel_count {
                    warnings.push(format!(
                        "prop '{}' segment on output {} extends past the prop's {} pixels",
                        prop.name, seg.output, prop.pixel_count
                    ));
                }
                if count == 0 {
                    continue;
                }
                let out = seg.output as usize - 1;
                // A reversed segment keeps its physical extent even when clipped, so the
                // first prop pixel stays on the segment's last physical pixel.
                let (dst_pix, end) = if seg.reverse {
                    (
                        seg.start_pixel.saturating_add(seg.pixel_count - count),
                        seg.start_pixel.saturating_add(seg.pixel_count),
                    )
                } else {
                    (seg.start_pixel, seg.start_pixel.saturating_add(count))
                };
                if end > MAX_OUTPUT_PIXELS {
                    warnings.push(format!(
                        "prop '{}' segment on output {} ends at pixel {end}, beyond the {MAX_OUTPUT_PIXELS}-pixel limit; ignored",
                        prop.name, seg.output
                    ));
                    continue;
                }
                pixels[out] = pixels[out].max(end);
                let src = prop
                    .channel_start
                    .saturating_add(seg.prop_offset.saturating_mul(3));
                pieces.push((out, src, dst_pix, count, seg.reverse));
            }
        }

        let frame = OutputFrame::new(&pixels);
        let mut runs: Vec<CopyRun> = pieces
            .into_iter()
            .map(|(out, src, dst_pix, count, reverse)| CopyRun {
                src,
                dst: (frame.offsets[out] + dst_pix as usize * BYTES_PER_PIXEL) as u32,
                pixels: count,
                reverse,
                output: out as u16,
            })
            .collect();
        runs.sort_by_key(|r| (r.dst, r.src));
        let runs = merge_runs(runs);
        let source_len = runs
            .iter()
            .map(|r| (r.src as usize).saturating_add(r.pixels as usize * BYTES_PER_PIXEL))
            .max()
            .unwrap_or(0);
        NodeMap {
            node_id: node.id.clone(),
            pixels_per_output: pixels,
            runs,
            source_len,
            warnings,
        }
    }

    /// Identity map for playing a `.ppseq` slice whose frames already have this node's
    /// output layout (source offsets are relative to the slice frame).
    pub fn identity(node_id: &str, pixels_per_output: &[u32]) -> NodeMap {
        let frame = OutputFrame::new(pixels_per_output);
        let runs = (0..frame.output_count())
            .filter(|&i| frame.pixels(i) > 0)
            .map(|i| CopyRun {
                src: frame.offsets[i] as u32,
                dst: frame.offsets[i] as u32,
                pixels: frame.pixels(i) as u32,
                reverse: false,
                output: i as u16,
            })
            .collect::<Vec<_>>();
        NodeMap {
            node_id: node_id.to_string(),
            pixels_per_output: pixels_per_output.to_vec(),
            source_len: frame.len(),
            runs: merge_runs(runs),
            warnings: Vec::new(),
        }
    }

    /// The map a follower uses for slices produced from this map.
    pub fn slice_map(&self) -> NodeMap {
        NodeMap::identity(&self.node_id, &self.pixels_per_output)
    }

    /// Pixel count per output (index 0 = output 1).
    pub fn pixels_per_output(&self) -> &[u32] {
        &self.pixels_per_output
    }

    /// Total pixels on the node.
    pub fn total_pixels(&self) -> u64 {
        self.pixels_per_output.iter().map(|&p| p as u64).sum()
    }

    /// Precomputed copy runs.
    pub fn runs(&self) -> &[CopyRun] {
        &self.runs
    }

    /// Minimum source frame length for which no run is truncated.
    pub fn source_len(&self) -> usize {
        self.source_len
    }

    /// A zeroed [`OutputFrame`] with this map's layout.
    pub fn new_frame(&self) -> OutputFrame {
        OutputFrame::new(&self.pixels_per_output)
    }

    /// Route `frame` into `out`. Pixels not covered by any prop are set to black; source
    /// bytes beyond `frame.len()` read as black. `out` must have been created by
    /// [`NodeMap::new_frame`] (or have the same layout); runs that do not fit are
    /// clipped. Never allocates.
    pub fn render(&self, frame: &[u8], out: &mut OutputFrame) {
        let dst_all = &mut out.data;
        dst_all.fill(0);
        let dst_len = dst_all.len();
        for r in &self.runs {
            let src = r.src as usize;
            let dst = r.dst as usize;
            if dst >= dst_len || src >= frame.len() {
                continue;
            }
            let bytes = r.pixels as usize * BYTES_PER_PIXEL;
            let bytes = bytes.min(dst_len - dst);
            if !r.reverse {
                let n = bytes.min(frame.len() - src);
                dst_all[dst..dst + n].copy_from_slice(&frame[src..src + n]);
            } else {
                // Source pixel k lands on destination pixel (count-1-k).
                let count = bytes / BYTES_PER_PIXEL;
                let total = r.pixels as usize;
                let src_pixels = ((frame.len() - src) / BYTES_PER_PIXEL).min(total);
                for k in 0..src_pixels {
                    let d = total - 1 - k;
                    if d >= count {
                        continue;
                    }
                    let s = src + k * BYTES_PER_PIXEL;
                    let d = dst + d * BYTES_PER_PIXEL;
                    dst_all[d..d + BYTES_PER_PIXEL].copy_from_slice(&frame[s..s + BYTES_PER_PIXEL]);
                }
            }
        }
    }
}

/// Number of outputs a node exposes: its configured outputs, or the board's count if
/// the output list has not been populated yet.
pub fn node_output_count(node: &Node) -> usize {
    node.outputs.len().max(node.board.output_count())
}

fn merge_runs(runs: Vec<CopyRun>) -> Vec<CopyRun> {
    let mut merged: Vec<CopyRun> = Vec::with_capacity(runs.len());
    for r in runs {
        if let Some(last) = merged.last_mut() {
            let last_bytes = last.pixels * BYTES_PER_PIXEL as u32;
            if !last.reverse
                && !r.reverse
                && last.output == r.output
                && last.dst.checked_add(last_bytes) == Some(r.dst)
                && last.src.checked_add(last_bytes) == Some(r.src)
            {
                last.pixels += r.pixels;
                continue;
            }
        }
        merged.push(r);
    }
    merged
}

// ---------------------------------------------------------------------------
// Prop lookups
// ---------------------------------------------------------------------------

/// Physical location of one prop pixel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelLocation {
    /// Node id.
    pub node_id: String,
    /// 1-based output index.
    pub output: u32,
    /// 0-based pixel position on the output.
    pub pixel: u32,
}

/// Where pixel `i` (0-based, xLights model order) of `prop` physically is, per §4.1.
/// `None` if the pixel is not wired.
pub fn prop_pixel_location(prop: &Prop, i: u32) -> Option<PixelLocation> {
    prop.segments.iter().find_map(|s| {
        if i < s.prop_offset || i - s.prop_offset >= s.pixel_count {
            return None;
        }
        let k = i - s.prop_offset;
        let pixel = if s.reverse {
            s.start_pixel.saturating_add(s.pixel_count - 1 - k)
        } else {
            s.start_pixel.saturating_add(k)
        };
        Some(PixelLocation {
            node_id: s.node_id.clone(),
            output: s.output,
            pixel,
        })
    })
}

/// The RGB bytes of `prop` inside an fseq frame (absolute channel space). Returns the
/// available part only (shorter if the frame ends early; empty if it starts beyond).
pub fn prop_channels<'a>(prop: &Prop, frame: &'a [u8]) -> &'a [u8] {
    let start = (prop.channel_start as usize).min(frame.len());
    let end = start
        .saturating_add(prop.pixel_count as usize * BYTES_PER_PIXEL)
        .min(frame.len());
    &frame[start..end]
}

const NOT_HERE: u32 = u32::MAX;

#[derive(Debug, Clone)]
struct PropEntry {
    pixel_count: u32,
    /// Byte offset in the output frame per prop pixel, or `NOT_HERE`.
    dst: Vec<u32>,
    /// Number of pixels on this node.
    local: u32,
}

/// Prop-pixel → output-buffer lookup for one node.
#[derive(Debug, Clone)]
pub struct PropMap {
    /// Node this map belongs to.
    pub node_id: String,
    pixels_per_output: Vec<u32>,
    index: HashMap<String, usize>,
    entries: Vec<PropEntry>,
}

impl PropMap {
    /// Build the map for `node_id`. Use the same show as the node's [`NodeMap`] so the
    /// output layouts agree.
    pub fn build(show: &Show, node_id: &str) -> Result<PropMap, MappingError> {
        let node = show
            .node(node_id)
            .ok_or_else(|| MappingError::UnknownNode(node_id.to_string()))?;
        let layout = NodeMap::build_for_node(node, &show.props);
        Ok(Self::build_with_layout(
            node,
            &show.props,
            layout.pixels_per_output(),
        ))
    }

    /// Build against an explicit output layout (e.g. a node map's pixel counts).
    pub fn build_with_layout(node: &Node, props: &[Prop], pixels_per_output: &[u32]) -> PropMap {
        let frame = OutputFrame::new(pixels_per_output);
        let mut index = HashMap::new();
        let mut entries = Vec::new();
        for prop in props {
            if !prop.segments.iter().any(|s| s.node_id == node.id) {
                continue;
            }
            let mut dst = vec![NOT_HERE; prop.pixel_count as usize];
            let mut local = 0;
            for seg in prop.segments.iter().filter(|s| s.node_id == node.id) {
                let out = seg.output as usize;
                if out == 0 || out > frame.output_count() {
                    continue;
                }
                let out_pixels = frame.pixels(out - 1) as u32;
                let base = frame.offsets[out - 1];
                for k in 0..seg.pixel_count {
                    let i = seg.prop_offset as u64 + k as u64;
                    if i >= prop.pixel_count as u64 {
                        break;
                    }
                    let p = if seg.reverse {
                        seg.start_pixel as u64 + (seg.pixel_count - 1 - k) as u64
                    } else {
                        seg.start_pixel as u64 + k as u64
                    };
                    if p >= out_pixels as u64 {
                        continue;
                    }
                    let slot = &mut dst[i as usize];
                    if *slot == NOT_HERE {
                        local += 1;
                    }
                    *slot = (base + p as usize * BYTES_PER_PIXEL) as u32;
                }
            }
            if local > 0 {
                index.insert(prop.id.clone(), entries.len());
                entries.push(PropEntry {
                    pixel_count: prop.pixel_count,
                    dst,
                    local,
                });
            }
        }
        PropMap {
            node_id: node.id.clone(),
            pixels_per_output: pixels_per_output.to_vec(),
            index,
            entries,
        }
    }

    /// Does any pixel of `prop_id` live on this node?
    pub fn contains(&self, prop_id: &str) -> bool {
        self.index.contains_key(prop_id)
    }

    /// Number of `prop_id`'s pixels wired to this node.
    pub fn local_pixels(&self, prop_id: &str) -> u32 {
        self.index
            .get(prop_id)
            .map(|&i| self.entries[i].local)
            .unwrap_or(0)
    }

    /// Output layout the map was built against.
    pub fn pixels_per_output(&self) -> &[u32] {
        &self.pixels_per_output
    }

    /// `(output index0, pixel)` of prop pixel `i` on this node.
    pub fn locate(&self, prop_id: &str, i: u32) -> Option<(usize, u32)> {
        let e = &self.entries[*self.index.get(prop_id)?];
        let dst = *e.dst.get(i as usize)?;
        if dst == NOT_HERE {
            return None;
        }
        let dst = dst as usize;
        let mut acc = 0usize;
        for (o, &p) in self.pixels_per_output.iter().enumerate() {
            let bytes = p as usize * BYTES_PER_PIXEL;
            if dst < acc + bytes {
                return Some((o, ((dst - acc) / BYTES_PER_PIXEL) as u32));
            }
            acc += bytes;
        }
        None
    }

    /// Write `rgb` (prop pixel order, 3 bytes per pixel) into `out`, replacing whatever
    /// the sequence put there. Extra bytes are ignored; missing pixels are left alone.
    /// Returns the number of pixels written. Never allocates.
    pub fn apply_overlay(&self, prop_id: &str, rgb: &[u8], out: &mut OutputFrame) -> usize {
        let Some(&idx) = self.index.get(prop_id) else {
            return 0;
        };
        let e = &self.entries[idx];
        let n = (rgb.len() / BYTES_PER_PIXEL).min(e.pixel_count as usize);
        let data = &mut out.data;
        let mut written = 0;
        for (i, &dst) in e.dst[..n].iter().enumerate() {
            if dst == NOT_HERE {
                continue;
            }
            let d = dst as usize;
            if d + BYTES_PER_PIXEL > data.len() {
                continue;
            }
            data[d..d + BYTES_PER_PIXEL].copy_from_slice(&rgb[i * 3..i * 3 + 3]);
            written += 1;
        }
        written
    }

    /// Set all of `prop_id`'s pixels on this node to one colour.
    pub fn fill(&self, prop_id: &str, rgb: [u8; 3], out: &mut OutputFrame) -> usize {
        let Some(&idx) = self.index.get(prop_id) else {
            return 0;
        };
        let data = &mut out.data;
        let mut written = 0;
        for &dst in &self.entries[idx].dst {
            let d = dst as usize;
            if dst != NOT_HERE && d + 3 <= data.len() {
                data[d..d + 3].copy_from_slice(&rgb);
                written += 1;
            }
        }
        written
    }

    /// Read back `prop_id`'s pixels from `out` into `rgb` (prop order; pixels not on this
    /// node are left untouched).
    pub fn read_prop(&self, prop_id: &str, out: &OutputFrame, rgb: &mut [u8]) {
        let Some(&idx) = self.index.get(prop_id) else {
            return;
        };
        let e = &self.entries[idx];
        let n = (rgb.len() / 3).min(e.dst.len());
        for i in 0..n {
            let d = e.dst[i] as usize;
            if e.dst[i] != NOT_HERE && d + 3 <= out.data.len() {
                rgb[i * 3..i * 3 + 3].copy_from_slice(&out.data[d..d + 3]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BoardKind, NodeRole, PropKind, PropSegment};

    pub(crate) fn node(id: &str, board: BoardKind) -> Node {
        Node {
            id: id.into(),
            name: id.into(),
            hostname: id.into(),
            role: NodeRole::Leader,
            board,
            board_rev: None,
            pi_model: None,
            outputs: board.default_outputs(),
            adopted: true,
            last_seen: None,
            notes: None,
        }
    }

    pub(crate) fn prop(id: &str, pixels: u32, channel_start: u32, segs: Vec<PropSegment>) -> Prop {
        Prop {
            id: id.into(),
            name: id.into(),
            kind: PropKind::Line,
            pixel_count: pixels,
            xlights_model: None,
            channel_start,
            channels_per_pixel: 3,
            segments: segs,
            group_ids: vec![],
            layout: None,
            matrix: None,
            color: None,
            max_milliamps_per_pixel: None,
            notes: None,
        }
    }

    pub(crate) fn seg(
        node: &str,
        output: u32,
        start: u32,
        count: u32,
        off: u32,
        reverse: bool,
    ) -> PropSegment {
        PropSegment {
            node_id: node.into(),
            output,
            start_pixel: start,
            pixel_count: count,
            prop_offset: off,
            reverse,
            null_pixels: 0,
        }
    }

    #[test]
    fn hostile_segments_do_not_allocate_or_panic() {
        let mut show = Show::default();
        show.nodes.push(node("n1", BoardKind::Difftx));
        // A segment starting four billion pixels into the output.
        show.props.push(prop("far", 10, 0, vec![seg("n1", 1, u32::MAX - 20, 10, 0, false)]));
        // Channel start at the very end of the channel space, two adjacent segments.
        show.props.push(prop(
            "end",
            4,
            u32::MAX - 2,
            vec![seg("n1", 2, 0, 2, 0, false), seg("n1", 2, 2, 2, 2, false)],
        ));
        // A huge prop that is not on this node at all.
        show.props.push(prop("elsewhere", u32::MAX, 0, vec![seg("n2", 1, 0, 10, 0, false)]));
        let map = NodeMap::build(&show, "n1").unwrap();
        assert!(map.total_pixels() <= 4 * MAX_OUTPUT_PIXELS as u64);
        assert!(map.warnings.iter().any(|w| w.contains("far")));
        let mut out = map.new_frame();
        map.render(&[1, 2, 3], &mut out);
        let pm = PropMap::build(&show, "n1").unwrap();
        assert!(!pm.contains("elsewhere"));
        assert_eq!(
            prop_pixel_location(&show.props[0], 9).map(|l| l.pixel),
            Some(u32::MAX - 11)
        );
    }

    fn frame_with_pixel_ids(pixels: usize) -> Vec<u8> {
        // Pixel n = (n, n+1, n+2) mod 256 — easy to recognise.
        (0..pixels * 3).map(|b| (b / 3 + b % 3) as u8).collect()
    }

    #[test]
    fn routes_segments_reverse_and_nulls() {
        let mut show = Show::default();
        show.nodes.push(node("n1", BoardKind::Difftx));
        show.nodes.push(node("n2", BoardKind::Difftx));
        // Prop A: 10 px at channel 0: 6 on output 1 (after 2 nulls), 4 reversed on output 2.
        show.props.push(prop(
            "a",
            10,
            0,
            vec![seg("n1", 1, 2, 6, 0, false), seg("n1", 2, 0, 4, 6, true)],
        ));
        // Prop B: 5 px at channel 30, chained after A on output 1.
        show.props
            .push(prop("b", 5, 30, vec![seg("n1", 1, 8, 5, 0, false)]));
        // Prop C lives on another node.
        show.props
            .push(prop("c", 5, 45, vec![seg("n2", 1, 0, 5, 0, false)]));

        let map = NodeMap::build(&show, "n1").unwrap();
        assert_eq!(map.pixels_per_output(), &[13, 4, 0, 0]);
        assert!(map.warnings.is_empty());
        let frame = frame_with_pixel_ids(20);
        let mut out = map.new_frame();
        map.render(&frame, &mut out);

        let o1 = out.output(0);
        assert_eq!(&o1[0..6], &[0; 6], "null pixels are black");
        for k in 0..6 {
            assert_eq!(&o1[(2 + k) * 3..(3 + k) * 3], &frame[k * 3..k * 3 + 3]);
        }
        for k in 0..5 {
            assert_eq!(
                &o1[(8 + k) * 3..(9 + k) * 3],
                &frame[(10 + k) * 3..(11 + k) * 3]
            );
        }
        let o2 = out.output(1);
        for k in 0..4 {
            // prop pixel 6+k -> output pixel 3-k
            assert_eq!(
                &o2[(3 - k) * 3..(4 - k) * 3],
                &frame[(6 + k) * 3..(7 + k) * 3]
            );
        }

        // PropMap agrees with NodeMap and with prop_pixel_location.
        let pm = PropMap::build(&show, "n1").unwrap();
        assert!(pm.contains("a") && pm.contains("b") && !pm.contains("c"));
        assert_eq!(pm.locate("a", 0), Some((0, 2)));
        assert_eq!(pm.locate("a", 6), Some((1, 3)));
        assert_eq!(pm.locate("a", 9), Some((1, 0)));
        let loc = prop_pixel_location(&show.props[0], 9).unwrap();
        assert_eq!((loc.output, loc.pixel), (2, 0));
        assert!(prop_pixel_location(&show.props[0], 10).is_none());

        let mut back = vec![0u8; 30];
        pm.read_prop("a", &out, &mut back);
        assert_eq!(&back[..], &frame[..30]);

        let overlay: Vec<u8> = (0..30).map(|i| 200 + (i % 50) as u8).collect();
        assert_eq!(pm.apply_overlay("a", &overlay, &mut out), 10);
        pm.read_prop("a", &out, &mut back);
        assert_eq!(back, overlay);
        assert_eq!(pm.fill("b", [1, 2, 3], &mut out), 5);
        assert_eq!(&out.output(0)[8 * 3..9 * 3], &[1, 2, 3]);
        assert_eq!(pm.apply_overlay("missing", &overlay, &mut out), 0);
    }

    #[test]
    fn short_frames_and_bad_segments_do_not_panic() {
        let mut show = Show::default();
        show.nodes.push(node("n1", BoardKind::Difftx));
        show.props.push(prop(
            "a",
            10,
            0,
            vec![
                seg("n1", 1, 0, 10, 0, true),
                seg("n1", 9, 0, 10, 0, false), // missing output
                seg("n1", 2, 0, 50, 5, false), // past the prop's end
                seg("n1", 0, 0, 5, 0, false),  // output 0 is invalid
            ],
        ));
        let map = NodeMap::build(&show, "n1").unwrap();
        assert_eq!(map.warnings.len(), 3);
        assert_eq!(map.pixels_per_output(), &[10, 5, 0, 0]);
        let mut out = map.new_frame();
        map.render(&[9, 9, 9, 8, 8, 8], &mut out); // only 2 pixels available
        let o1 = out.output(0);
        assert_eq!(&o1[27..30], &[9, 9, 9]);
        assert_eq!(&o1[24..27], &[8, 8, 8]);
        assert_eq!(&o1[..24], &[0; 24]);
        map.render(&[], &mut out);
        assert!(out.as_bytes().iter().all(|&b| b == 0));
        // Rendering into a mismatched (smaller) frame is clipped, not a panic.
        let mut small = OutputFrame::new(&[2]);
        map.render(&[1; 90], &mut small);
        assert_eq!(
            NodeMap::build(&show, "zz").unwrap_err(),
            MappingError::UnknownNode("zz".into())
        );
    }

    #[test]
    fn adjacent_runs_are_merged() {
        let mut show = Show::default();
        show.nodes.push(node("n1", BoardKind::Difftx));
        show.props
            .push(prop("a", 5, 0, vec![seg("n1", 1, 0, 5, 0, false)]));
        show.props
            .push(prop("b", 5, 15, vec![seg("n1", 1, 5, 5, 0, false)]));
        let map = NodeMap::build(&show, "n1").unwrap();
        assert_eq!(map.runs().len(), 1);
        assert_eq!(map.runs()[0].pixels, 10);
        assert_eq!(map.source_len(), 30);
    }

    #[test]
    fn identity_slice_map_reproduces_frame() {
        let mut show = Show::default();
        show.nodes.push(node("n1", BoardKind::Difftx));
        show.props.push(prop(
            "a",
            8,
            12,
            vec![seg("n1", 1, 1, 4, 0, true), seg("n1", 3, 0, 4, 4, false)],
        ));
        let map = NodeMap::build(&show, "n1").unwrap();
        let fseq_frame = frame_with_pixel_ids(20);
        let mut direct = map.new_frame();
        map.render(&fseq_frame, &mut direct);
        let slice = map.slice_map();
        let mut via_slice = slice.new_frame();
        slice.render(direct.as_bytes(), &mut via_slice);
        assert_eq!(direct, via_slice);
        let mut loaded = map.new_frame();
        loaded.load(direct.as_bytes());
        assert_eq!(loaded, direct);
    }

    #[test]
    fn prop_channels_clips() {
        let p = prop("a", 4, 6, vec![]);
        assert_eq!(prop_channels(&p, &[0u8; 30]).len(), 12);
        assert_eq!(prop_channels(&p, &[0u8; 10]).len(), 4);
        assert!(prop_channels(&p, &[0u8; 3]).is_empty());
    }

    /// 60 outputs × 1600 px, 16 props of 100 px per output (some reversed).
    #[test]
    fn benchmark_60_outputs_1600_px() {
        let mut show = Show::default();
        show.nodes.push(node("big", BoardKind::Difftxlarge));
        let mut ch = 0u32;
        for o in 1..=60u32 {
            for k in 0..16u32 {
                show.props.push(prop(
                    &format!("p{o}_{k}"),
                    100,
                    ch,
                    vec![seg("big", o, k * 100, 100, 0, k % 4 == 3)],
                ));
                ch += 300;
            }
        }
        let map = NodeMap::build(&show, "big").unwrap();
        assert_eq!(map.total_pixels(), 60 * 1600);
        let frame: Vec<u8> = (0..ch as usize).map(|i| i as u8).collect();
        let mut out = map.new_frame();
        let iterations = 200;
        let t = std::time::Instant::now();
        for _ in 0..iterations {
            map.render(&frame, &mut out);
        }
        let per_frame = t.elapsed() / iterations;
        eprintln!("NodeMap::render 60×1600 px: {per_frame:?} per frame");
        // Generous bound so unoptimised debug builds on slow CI pass; release builds
        // run in well under a millisecond.
        assert!(
            per_frame < std::time::Duration::from_millis(25),
            "{per_frame:?}"
        );
        // Spot-check one normal and one reversed prop.
        assert_eq!(&out.output(0)[0..3], &frame[0..3]);
        let rev_start = 3 * 300; // prop p1_3
        assert_eq!(
            &out.output(0)[399 * 3..400 * 3],
            &frame[rev_start..rev_start + 3]
        );
    }
}
