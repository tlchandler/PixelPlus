//! `.ppseq` node slices (ARCHITECTURE §7.3).
//!
//! A slice holds only the pixels of one node, already routed into that node's output
//! layout (output-major, pixel order, RGB, colour order not applied). Followers play
//! slices; the leader produces them from the full `.fseq` with the same [`NodeMap`] it
//! uses for its own outputs.
//!
//! ```text
//! "PPSQ" | u16 version=1 | u32 frameCount | u32 frameUs | u32 frameBytes |
//! u16 outputCount | outputCount × u32 pixelsPerOutput | 32-byte sha256 of source fseq |
//! frames: zstd blocks of up to 64 frames |
//! index: [u64 offset, u32 compressedLen, u32 firstFrame] × nBlocks | u32 nBlocks | "PPSQ"
//! ```
//!
//! All integers are little-endian. `frameUs` is the frame duration in microseconds
//! (`frameMs × 1000`).

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::fseq::{FseqError, FseqFile};
use crate::mapping::{NodeMap, OutputFrame};

/// Magic at both ends of the file.
pub const MAGIC: &[u8; 4] = b"PPSQ";
/// Current format version.
pub const VERSION: u16 = 1;
/// Frames per compressed block.
pub const FRAMES_PER_BLOCK: u32 = 64;

const INDEX_ENTRY: usize = 16;

/// Errors reading or writing slices.
#[derive(Debug, thiserror::Error)]
pub enum PpseqError {
    /// I/O failure.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// Not a valid slice file.
    #[error("invalid .ppseq file: {0}")]
    Format(String),
    /// Source sequence could not be read.
    #[error(transparent)]
    Fseq(#[from] FseqError),
    /// Block decode failure.
    #[error("failed to decompress .ppseq block {block}: {message}")]
    Decompress {
        /// Failing block.
        block: usize,
        /// Decoder message.
        message: String,
    },
    /// Frame index out of range.
    #[error("frame {frame} is out of range (slice has {frame_count} frames)")]
    FrameOutOfRange {
        /// Requested frame.
        frame: u32,
        /// Frames in the slice.
        frame_count: u32,
    },
}

/// Result alias for this module.
pub type Result<T> = std::result::Result<T, PpseqError>;

/// Fixed metadata of a slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpseqHeader {
    /// Format version.
    pub version: u16,
    /// Number of frames.
    pub frame_count: u32,
    /// Frame duration in microseconds.
    pub frame_us: u32,
    /// Bytes per frame (= sum of pixels × 3).
    pub frame_bytes: u32,
    /// Pixel count per output (index 0 = output 1).
    pub pixels_per_output: Vec<u32>,
    /// SHA-256 of the source `.fseq`.
    pub source_sha256: [u8; 32],
}

impl PpseqHeader {
    /// Frame duration in whole milliseconds.
    pub fn frame_ms(&self) -> u32 {
        self.frame_us / 1000
    }

    /// Duration in milliseconds.
    pub fn duration_ms(&self) -> u64 {
        self.frame_count as u64 * self.frame_us as u64 / 1000
    }

    /// Hex SHA-256 of the source sequence.
    pub fn source_sha256_hex(&self) -> String {
        crate::fseq::to_hex(&self.source_sha256)
    }

    fn encode(&self) -> Vec<u8> {
        let mut h = Vec::with_capacity(22 + self.pixels_per_output.len() * 4 + 32);
        h.extend_from_slice(MAGIC);
        h.extend_from_slice(&self.version.to_le_bytes());
        h.extend_from_slice(&self.frame_count.to_le_bytes());
        h.extend_from_slice(&self.frame_us.to_le_bytes());
        h.extend_from_slice(&self.frame_bytes.to_le_bytes());
        h.extend_from_slice(&(self.pixels_per_output.len() as u16).to_le_bytes());
        for p in &self.pixels_per_output {
            h.extend_from_slice(&p.to_le_bytes());
        }
        h.extend_from_slice(&self.source_sha256);
        h
    }
}

/// Streaming slice writer (frames are compressed block by block).
pub struct PpseqWriter<W: Write + Seek> {
    out: W,
    header: PpseqHeader,
    header_len: u64,
    pending: Vec<u8>,
    pending_frames: u32,
    frames: u32,
    first_frame: u32,
    pos: u64,
    index: Vec<(u64, u32, u32)>,
    level: i32,
}

impl<W: Write + Seek> PpseqWriter<W> {
    /// Start a slice. `frame_count` in `header` is ignored and filled in by
    /// [`PpseqWriter::finish`].
    pub fn new(mut out: W, mut header: PpseqHeader) -> Result<Self> {
        if header.pixels_per_output.len() > u16::MAX as usize {
            return Err(PpseqError::Format("too many outputs".into()));
        }
        let bytes: u64 = header.pixels_per_output.iter().map(|&p| p as u64 * 3).sum();
        header.frame_bytes = u32::try_from(bytes)
            .map_err(|_| PpseqError::Format("frame larger than 4 GiB".into()))?;
        header.version = VERSION;
        header.frame_count = 0;
        let enc = header.encode();
        out.write_all(&enc)?;
        Ok(PpseqWriter {
            out,
            header_len: enc.len() as u64,
            pos: enc.len() as u64,
            header,
            pending: Vec::new(),
            pending_frames: 0,
            frames: 0,
            first_frame: 0,
            index: Vec::new(),
            level: 3,
        })
    }

    /// Append one frame of exactly `frame_bytes` bytes.
    pub fn write_frame(&mut self, frame: &[u8]) -> Result<()> {
        if frame.len() != self.header.frame_bytes as usize {
            return Err(PpseqError::Format(format!(
                "frame has {} bytes, expected {}",
                frame.len(),
                self.header.frame_bytes
            )));
        }
        if self.pending_frames == 0 {
            self.first_frame = self.frames;
        }
        self.pending.extend_from_slice(frame);
        self.pending_frames += 1;
        self.frames += 1;
        if self.pending_frames >= FRAMES_PER_BLOCK {
            self.flush_block()?;
        }
        Ok(())
    }

    fn flush_block(&mut self) -> Result<()> {
        if self.pending_frames == 0 {
            return Ok(());
        }
        let c = zstd::bulk::compress(&self.pending, self.level)?;
        self.out.write_all(&c)?;
        let len = u32::try_from(c.len())
            .map_err(|_| PpseqError::Format("compressed block exceeds 4 GiB".into()))?;
        self.index.push((self.pos, len, self.first_frame));
        self.pos += c.len() as u64;
        self.pending.clear();
        self.pending_frames = 0;
        Ok(())
    }

    /// Write the index and trailer, patch the frame count and return the sink.
    pub fn finish(mut self) -> Result<W> {
        self.flush_block()?;
        for &(off, len, first) in &self.index {
            self.out.write_all(&off.to_le_bytes())?;
            self.out.write_all(&len.to_le_bytes())?;
            self.out.write_all(&first.to_le_bytes())?;
        }
        self.out
            .write_all(&(self.index.len() as u32).to_le_bytes())?;
        self.out.write_all(MAGIC)?;
        let end = self.out.stream_position()?;
        self.out.seek(SeekFrom::Start(6))?;
        self.out.write_all(&self.frames.to_le_bytes())?;
        self.out.seek(SeekFrom::Start(end))?;
        self.out.flush()?;
        debug_assert!(self.header_len <= end);
        Ok(self.out)
    }
}

/// Summary returned by [`write_slice`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliceInfo {
    /// Header written.
    pub header: PpseqHeader,
    /// Size of the slice file in bytes.
    pub file_bytes: u64,
}

/// Render every frame of `fseq` through `node_map` and write the node's slice to
/// `out_path` (atomically: written to a temporary file, then renamed).
pub fn write_slice<R: Read + Seek>(
    fseq: &mut FseqFile<R>,
    source_sha256: [u8; 32],
    node_map: &NodeMap,
    out_path: impl AsRef<Path>,
) -> Result<SliceInfo> {
    let out_path = out_path.as_ref();
    let tmp = tmp_path(out_path);
    let res = (|| {
        let file = File::create(&tmp)?;
        let w = write_slice_to(fseq, source_sha256, node_map, BufWriter::new(file))?;
        let file = w.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        Ok::<_, PpseqError>(())
    })();
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, out_path)?;
    let file_bytes = std::fs::metadata(out_path)?.len();
    let header = PpseqFile::open(out_path)?.header().clone();
    Ok(SliceInfo { header, file_bytes })
}

/// Convenience: open the fseq at `fseq_path`, hash it, and write the slice.
pub fn write_slice_from_path(
    fseq_path: impl AsRef<Path>,
    node_map: &NodeMap,
    out_path: impl AsRef<Path>,
) -> Result<SliceInfo> {
    let sha = crate::fseq::sha256_file_bytes(fseq_path.as_ref())?;
    let mut fseq = FseqFile::open(fseq_path)?;
    write_slice(&mut fseq, sha, node_map, out_path)
}

/// Write a slice to any seekable sink; returns the sink.
pub fn write_slice_to<R: Read + Seek, W: Write + Seek>(
    fseq: &mut FseqFile<R>,
    source_sha256: [u8; 32],
    node_map: &NodeMap,
    out: W,
) -> Result<W> {
    let header = PpseqHeader {
        version: VERSION,
        frame_count: 0,
        frame_us: fseq.frame_ms() * 1000,
        frame_bytes: 0,
        pixels_per_output: node_map.pixels_per_output().to_vec(),
        source_sha256,
    };
    let mut w = PpseqWriter::new(out, header)?;
    let mut src = vec![0u8; fseq.frame_size()];
    let mut frame: OutputFrame = node_map.new_frame();
    for i in 0..fseq.frame_count() {
        fseq.frame(i, &mut src)?;
        node_map.render(&src, &mut frame);
        w.write_frame(frame.as_bytes())?;
    }
    w.finish()
}

fn tmp_path(p: &Path) -> std::path::PathBuf {
    let mut name = p.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".tmp");
    p.with_file_name(name)
}

#[derive(Debug, Clone, Copy)]
struct Block {
    offset: u64,
    len: u32,
    first: u32,
    end: u32,
}

/// An open `.ppseq` slice with random frame access (one decompressed block cached).
pub struct PpseqFile<R = BufReader<File>> {
    reader: R,
    header: PpseqHeader,
    blocks: Vec<Block>,
    cur: Option<usize>,
    data: Vec<u8>,
    compressed: Vec<u8>,
    dec: Option<zstd::stream::raw::Decoder<'static>>,
}

impl<R> std::fmt::Debug for PpseqFile<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PpseqFile")
            .field("header", &self.header)
            .field("blocks", &self.blocks.len())
            .finish()
    }
}

impl PpseqFile<BufReader<File>> {
    /// Open a slice from disk.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_reader(BufReader::new(File::open(path.as_ref())?))
    }
}

impl<R: Read + Seek> PpseqFile<R> {
    /// Parse a slice from any seekable reader.
    pub fn from_reader(mut reader: R) -> Result<Self> {
        let file_len = reader.seek(SeekFrom::End(0))?;
        reader.seek(SeekFrom::Start(0))?;
        let fmt = |m: &str| PpseqError::Format(m.to_string());
        let mut fixed = [0u8; 20];
        reader
            .read_exact(&mut fixed)
            .map_err(|_| fmt("file shorter than header"))?;
        if &fixed[0..4] != MAGIC {
            return Err(fmt("missing PPSQ magic"));
        }
        let version = u16::from_le_bytes([fixed[4], fixed[5]]);
        if version != VERSION {
            return Err(PpseqError::Format(format!("unsupported version {version}")));
        }
        let rd32 =
            |b: &[u8], at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
        let frame_count = rd32(&fixed, 6);
        let frame_us = rd32(&fixed, 10);
        let frame_bytes = rd32(&fixed, 14);
        let outputs = u16::from_le_bytes([fixed[18], fixed[19]]) as usize;
        let header_len = 20 + outputs as u64 * 4 + 32;
        if header_len + 8 > file_len {
            return Err(fmt("truncated header"));
        }
        let mut rest = vec![0u8; outputs * 4 + 32];
        reader.read_exact(&mut rest)?;
        let pixels_per_output: Vec<u32> = (0..outputs).map(|i| rd32(&rest, i * 4)).collect();
        let mut source_sha256 = [0u8; 32];
        source_sha256.copy_from_slice(&rest[outputs * 4..]);
        let sum: u64 = pixels_per_output.iter().map(|&p| p as u64 * 3).sum();
        if sum != frame_bytes as u64 {
            return Err(fmt("frame size does not match output pixel counts"));
        }
        if frame_us == 0 {
            return Err(fmt("frame duration of 0"));
        }
        if frame_bytes as u64 > crate::fseq::MAX_FRAME_BYTES {
            return Err(fmt("frame size exceeds the supported maximum"));
        }

        // Trailer.
        reader.seek(SeekFrom::Start(file_len - 8))?;
        let mut trailer = [0u8; 8];
        reader.read_exact(&mut trailer)?;
        if &trailer[4..8] != MAGIC {
            return Err(fmt("missing trailer (incomplete file?)"));
        }
        let n_blocks = rd32(&trailer, 0) as u64;
        let index_len = n_blocks
            .checked_mul(INDEX_ENTRY as u64)
            .ok_or_else(|| fmt("bad block count"))?;
        if header_len + index_len + 8 > file_len {
            return Err(fmt("block index larger than file"));
        }
        let index_start = file_len - 8 - index_len;
        reader.seek(SeekFrom::Start(index_start))?;
        let mut idx = vec![0u8; index_len as usize];
        reader.read_exact(&mut idx)?;
        let mut blocks: Vec<Block> = Vec::with_capacity(n_blocks as usize);
        for i in 0..n_blocks as usize {
            let e = &idx[i * INDEX_ENTRY..(i + 1) * INDEX_ENTRY];
            let mut o = [0u8; 8];
            o.copy_from_slice(&e[0..8]);
            let offset = u64::from_le_bytes(o);
            let len = rd32(e, 8);
            let first = rd32(e, 12);
            let in_data = offset
                .checked_add(len as u64)
                .is_some_and(|end| end <= index_start);
            if offset < header_len || !in_data {
                return Err(PpseqError::Format(format!(
                    "block {i} lies outside the data area"
                )));
            }
            if let Some(prev) = blocks.last() {
                if first <= prev.first {
                    return Err(fmt("block index out of order"));
                }
            } else if first != 0 {
                return Err(fmt("first block does not start at frame 0"));
            }
            blocks.push(Block {
                offset,
                len,
                first,
                end: 0,
            });
        }
        for i in 0..blocks.len() {
            let end = blocks.get(i + 1).map(|b| b.first).unwrap_or(frame_count);
            if end < blocks[i].first || end > frame_count {
                return Err(fmt("block index inconsistent with frame count"));
            }
            blocks[i].end = end;
            if (end - blocks[i].first) as u64 * frame_bytes as u64 > crate::fseq::MAX_BLOCK_BYTES {
                return Err(fmt("block decompresses to more than the supported maximum"));
            }
        }
        if frame_count > 0 && blocks.is_empty() {
            return Err(fmt("frames present but no blocks"));
        }
        Ok(PpseqFile {
            reader,
            header: PpseqHeader {
                version,
                frame_count,
                frame_us,
                frame_bytes,
                pixels_per_output,
                source_sha256,
            },
            blocks,
            cur: None,
            data: Vec::new(),
            compressed: Vec::new(),
            dec: None,
        })
    }

    /// Header.
    pub fn header(&self) -> &PpseqHeader {
        &self.header
    }

    /// Number of frames.
    pub fn frame_count(&self) -> u32 {
        self.header.frame_count
    }

    /// Frame duration in ms.
    pub fn frame_ms(&self) -> u32 {
        self.header.frame_ms()
    }

    /// Bytes per frame.
    pub fn frame_bytes(&self) -> usize {
        self.header.frame_bytes as usize
    }

    /// Output layout.
    pub fn pixels_per_output(&self) -> &[u32] {
        &self.header.pixels_per_output
    }

    /// A zeroed [`OutputFrame`] with this slice's layout.
    pub fn new_frame(&self) -> OutputFrame {
        OutputFrame::new(&self.header.pixels_per_output)
    }

    /// Read frame `idx` into `buf` (copies `min(len)` bytes, zeroes the rest).
    pub fn frame(&mut self, idx: u32, buf: &mut [u8]) -> Result<()> {
        if idx >= self.header.frame_count {
            return Err(PpseqError::FrameOutOfRange {
                frame: idx,
                frame_count: self.header.frame_count,
            });
        }
        let block = match self.cur {
            Some(c) if idx >= self.blocks[c].first && idx < self.blocks[c].end => c,
            _ => self.blocks.partition_point(|b| b.first <= idx) - 1,
        };
        self.load(block)?;
        let fb = self.header.frame_bytes as usize;
        let start = (idx - self.blocks[block].first) as usize * fb;
        if start + fb > self.data.len() {
            return Err(PpseqError::Decompress {
                block,
                message: "block shorter than expected".into(),
            });
        }
        let n = fb.min(buf.len());
        buf[..n].copy_from_slice(&self.data[start..start + n]);
        buf[n..].fill(0);
        Ok(())
    }

    /// Read frame `idx` directly into an [`OutputFrame`].
    pub fn frame_into(&mut self, idx: u32, out: &mut OutputFrame) -> Result<()> {
        self.frame(idx, out.as_bytes_mut())
    }

    fn load(&mut self, block: usize) -> Result<()> {
        if self.cur == Some(block) {
            return Ok(());
        }
        self.cur = None;
        let b = self.blocks[block];
        self.compressed.resize(b.len as usize, 0);
        self.reader.seek(SeekFrom::Start(b.offset))?;
        self.reader.read_exact(&mut self.compressed)?;
        let expected = (b.end - b.first) as usize * self.header.frame_bytes as usize;
        let err = |e: io::Error| PpseqError::Decompress {
            block,
            message: e.to_string(),
        };
        if self.dec.is_none() {
            self.dec = Some(zstd::stream::raw::Decoder::new().map_err(err)?);
        }
        let dec = self.dec.as_mut().expect("initialised above");
        crate::fseq::zstd_decompress_capped(dec, &self.compressed, &mut self.data, expected)
            .map_err(err)?;
        self.cur = Some(block);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fseq::{FseqWriter, FseqWriterOptions};
    use crate::model::{BoardKind, Node, NodeRole, Prop, PropKind, PropSegment, Show};
    use std::io::Cursor;

    fn node(id: &str) -> Node {
        Node {
            id: id.into(),
            name: id.into(),
            hostname: id.into(),
            role: NodeRole::Follower,
            board: BoardKind::Difftx,
            board_rev: None,
            pi_model: None,
            outputs: BoardKind::Difftx.default_outputs(),
            adopted: true,
            last_seen: None,
            notes: None,
        }
    }

    fn prop(id: &str, pixels: u32, start: u32, segs: Vec<PropSegment>) -> Prop {
        Prop {
            id: id.into(),
            name: id.into(),
            kind: PropKind::Line,
            pixel_count: pixels,
            xlights_model: None,
            channel_start: start,
            channels_per_pixel: 3,
            channel_runs: None,
            segments: segs,
            group_ids: vec![],
            layout: None,
            matrix: None,
            color: None,
            max_milliamps_per_pixel: None,
            notes: None,
        }
    }

    fn seg(node: &str, output: u32, start: u32, count: u32, off: u32, rev: bool) -> PropSegment {
        PropSegment {
            node_id: node.into(),
            output,
            start_pixel: start,
            pixel_count: count,
            prop_offset: off,
            reverse: rev,
            null_pixels: 0,
        }
    }

    fn show() -> Show {
        let mut s = Show::default();
        s.nodes.push(node("f1"));
        s.props.push(prop(
            "a",
            50,
            0,
            vec![seg("f1", 1, 0, 30, 0, false), seg("f1", 2, 5, 20, 30, true)],
        ));
        s.props
            .push(prop("b", 40, 150, vec![seg("f1", 4, 0, 40, 0, false)]));
        s
    }

    fn fseq_bytes(frames: u32, channels: u32) -> Vec<u8> {
        let mut w = FseqWriter::new(
            Cursor::new(Vec::new()),
            FseqWriterOptions::new(channels, 25),
        )
        .unwrap();
        for f in 0..frames {
            let fr: Vec<u8> = (0..channels).map(|c| (c * 3 + f * 11) as u8).collect();
            w.write_frame(&fr).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn roundtrip_matches_direct_render() {
        let show = show();
        let map = NodeMap::build(&show, "f1").unwrap();
        let frames = 150; // 3 blocks: 64 + 64 + 22
        let bytes = fseq_bytes(frames, 400);
        let mut fseq = FseqFile::from_reader(Cursor::new(bytes.clone())).unwrap();
        let sha = [7u8; 32];
        let slice = write_slice_to(&mut fseq, sha, &map, Cursor::new(Vec::new()))
            .unwrap()
            .into_inner();

        let mut pp = PpseqFile::from_reader(Cursor::new(slice)).unwrap();
        assert_eq!(pp.frame_count(), frames);
        assert_eq!(pp.frame_ms(), 25);
        assert_eq!(pp.header().source_sha256, sha);
        assert_eq!(pp.pixels_per_output(), map.pixels_per_output());
        assert_eq!(pp.blocks.len(), 3);

        let mut fseq = FseqFile::from_reader(Cursor::new(bytes)).unwrap();
        let mut src = vec![0u8; fseq.frame_size()];
        let mut direct = map.new_frame();
        let mut from_slice = pp.new_frame();
        for &i in &[0u32, 63, 64, 149, 1, 100, 128, 127] {
            fseq.frame(i, &mut src).unwrap();
            map.render(&src, &mut direct);
            pp.frame_into(i, &mut from_slice).unwrap();
            assert_eq!(direct, from_slice, "frame {i}");
        }
        assert!(matches!(
            pp.frame(frames, &mut [0u8; 3]),
            Err(PpseqError::FrameOutOfRange { .. })
        ));
    }

    #[test]
    fn slice_follows_channel_runs() {
        use crate::model::ChannelRun;
        // Prop "a" is two xLights strings: pixels 0..30 at byte 300, 30..50 at byte 0.
        let mut show = show();
        show.props[0].channel_runs = Some(vec![
            ChannelRun {
                prop_offset: 0,
                channel_start: 300,
                pixel_count: 30,
            },
            ChannelRun {
                prop_offset: 30,
                channel_start: 0,
                pixel_count: 20,
            },
        ]);
        show.props[0].channel_start = 0;
        let map = NodeMap::build(&show, "f1").unwrap();
        let bytes = fseq_bytes(3, 400);
        let mut fseq = FseqFile::from_reader(Cursor::new(bytes)).unwrap();
        let slice = write_slice_to(&mut fseq, [1; 32], &map, Cursor::new(Vec::new()))
            .unwrap()
            .into_inner();
        let mut pp = PpseqFile::from_reader(Cursor::new(slice)).unwrap();
        let mut f = pp.new_frame();
        for frame in 0..3u32 {
            pp.frame_into(frame, &mut f).unwrap();
            let byte = |c: u32| (c * 3 + frame * 11) as u8;
            let rgb = |c: u32| vec![byte(c), byte(c + 1), byte(c + 2)];
            // Output 1: prop pixels 0..30 from byte 300.
            for k in 0..30u32 {
                let o = &f.output(0)[k as usize * 3..k as usize * 3 + 3];
                assert_eq!(o, rgb(300 + 3 * k), "frame {frame} out 1 px {k}");
            }
            // Output 2 (reversed, from pixel 5): prop pixels 30..50 from byte 0.
            for j in 0..20u32 {
                let p = 5 + 19 - j;
                let o = &f.output(1)[p as usize * 3..p as usize * 3 + 3];
                assert_eq!(o, rgb(3 * j), "frame {frame} out 2 px {p}");
            }
        }
    }

    #[test]
    fn write_slice_on_disk_is_atomic_and_hashes_source() {
        let dir = std::env::temp_dir().join(format!("ppx-ppseq-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fseq_path = dir.join("song.fseq");
        std::fs::write(&fseq_path, fseq_bytes(10, 300)).unwrap();
        let map = NodeMap::build(&show(), "f1").unwrap();
        let out = dir.join("song.ppseq");
        let info = write_slice_from_path(&fseq_path, &map, &out).unwrap();
        assert_eq!(info.header.frame_count, 10);
        assert_eq!(
            info.header.source_sha256_hex(),
            crate::fseq::sha256_file(&fseq_path).unwrap()
        );
        assert!(!tmp_path(&out).exists());
        assert_eq!(info.file_bytes, std::fs::metadata(&out).unwrap().len());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn empty_sequence() {
        let map = NodeMap::build(&show(), "f1").unwrap();
        let mut fseq = FseqFile::from_reader(Cursor::new(fseq_bytes(0, 30))).unwrap();
        let slice = write_slice_to(&mut fseq, [0; 32], &map, Cursor::new(Vec::new()))
            .unwrap()
            .into_inner();
        let pp = PpseqFile::from_reader(Cursor::new(slice)).unwrap();
        assert_eq!(pp.frame_count(), 0);
    }

    #[test]
    fn rejects_corrupt_files() {
        let map = NodeMap::build(&show(), "f1").unwrap();
        let mut fseq = FseqFile::from_reader(Cursor::new(fseq_bytes(70, 400))).unwrap();
        let good = write_slice_to(&mut fseq, [0; 32], &map, Cursor::new(Vec::new()))
            .unwrap()
            .into_inner();
        // Truncated (no trailer).
        let mut t = good.clone();
        t.truncate(t.len() - 3);
        assert!(PpseqFile::from_reader(Cursor::new(t)).is_err());
        // Bad magic.
        let mut t = good.clone();
        t[0] = b'X';
        assert!(PpseqFile::from_reader(Cursor::new(t)).is_err());
        // Absurd block count.
        let mut t = good.clone();
        let n = t.len();
        t[n - 8..n - 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(PpseqFile::from_reader(Cursor::new(t)).is_err());
        // Corrupt block payload: error on read, not a panic.
        let mut t = good.clone();
        let hdr = 20 + 4 * 4 + 32;
        for b in &mut t[hdr..hdr + 8] {
            *b ^= 0xFF;
        }
        let mut pp = PpseqFile::from_reader(Cursor::new(t)).unwrap();
        let mut buf = vec![0u8; pp.frame_bytes()];
        assert!(pp.frame(0, &mut buf).is_err());
        // Garbage.
        assert!(PpseqFile::from_reader(Cursor::new(vec![0u8; 5])).is_err());
    }

    /// Minimal slice: one output of `pixels` pixels, `frame_count` frames and the
    /// given block index entries (offset, len, first) over `data`.
    fn raw_slice(pixels: u32, frame_count: u32, data: &[u8], index: &[(u64, u32, u32)]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(MAGIC);
        v.extend_from_slice(&VERSION.to_le_bytes());
        v.extend_from_slice(&frame_count.to_le_bytes());
        v.extend_from_slice(&25_000u32.to_le_bytes());
        v.extend_from_slice(&(pixels * 3).to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&pixels.to_le_bytes());
        v.extend_from_slice(&[0u8; 32]);
        v.extend_from_slice(data);
        for &(o, l, f) in index {
            v.extend_from_slice(&o.to_le_bytes());
            v.extend_from_slice(&l.to_le_bytes());
            v.extend_from_slice(&f.to_le_bytes());
        }
        v.extend_from_slice(&(index.len() as u32).to_le_bytes());
        v.extend_from_slice(MAGIC);
        v
    }

    #[test]
    fn hostile_index_and_sizes_are_errors_not_panics() {
        let block = zstd::bulk::compress(&[1u8; 30], 3).unwrap();
        let hdr = 20 + 4 + 32;
        // Sane file reads.
        let ok = raw_slice(10, 1, &block, &[(hdr, block.len() as u32, 0)]);
        let mut pp = PpseqFile::from_reader(Cursor::new(ok)).unwrap();
        let mut buf = [0u8; 30];
        pp.frame(0, &mut buf).unwrap();
        assert_eq!(buf, [1u8; 30]);
        // offset + len overflowing u64 used to panic.
        let t = raw_slice(10, 1, &block, &[(u64::MAX - 1, 16, 0)]);
        assert!(PpseqFile::from_reader(Cursor::new(t)).is_err());
        // One block claiming 4 billion frames of 3 KB each.
        let t = raw_slice(1000, u32::MAX, &block, &[(hdr, block.len() as u32, 0)]);
        assert!(PpseqFile::from_reader(Cursor::new(t)).is_err());
        // A frame of billions of pixels.
        let t = raw_slice(0x4000_0000, 1, &block, &[(hdr, block.len() as u32, 0)]);
        assert!(PpseqFile::from_reader(Cursor::new(t)).is_err());
        // Decompression bomb: the block inflates far past its frames.
        let bomb = zstd::bulk::compress(&vec![5u8; 16 << 20], 3).unwrap();
        let t = raw_slice(10, 2, &bomb, &[(hdr, bomb.len() as u32, 0)]);
        let mut pp = PpseqFile::from_reader(Cursor::new(t)).unwrap();
        pp.frame(1, &mut buf).unwrap();
        assert_eq!(buf, [5u8; 30]);
        assert!(pp.data.capacity() < 1 << 20);
    }
}
