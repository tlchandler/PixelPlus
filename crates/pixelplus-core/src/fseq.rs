//! Reader and writer for FSEQ sequence files (the format xLights and FPP use).
//!
//! Supported:
//!
//! * **v1** (`PSEQ` major 1): uncompressed, variable headers.
//! * **v2.0 / v2.1 / v2.2**: uncompressed, zstd- or zlib-compressed channel data split in
//!   independently decodable *blocks* (the v2.2 extended 12-bit block count is honoured),
//!   sparse channel ranges and variable headers (`mf` media filename, `sp` producer,
//!   and `ED` extended-data indirection).
//!
//! # Channel space
//!
//! A frame is delivered in *absolute channel space*: byte `n` of the output buffer is
//! channel `n` (0-based) as xLights numbered it. For ordinary files that is simply the
//! stored frame. For sparse files ([`FseqHeader::sparse_ranges`] non-empty) only the
//! listed ranges are stored; [`FseqFile::frame`] scatters them to their absolute offsets
//! and zeroes everything else. [`FseqFile::frame_size`] is the buffer size that holds a
//! full frame in absolute channel space.
//!
//! # Performance
//!
//! Random access is O(1) for uncompressed files (one positioned read). For compressed
//! files the reader keeps exactly one decompressed block in memory and decompresses the
//! next block lazily when playback crosses a block boundary; allocations and the
//! decompression context are reused, so steady-state playback performs no heap
//! allocation. This comfortably sustains 40 fps on a Raspberry Pi Zero 2 W for typical
//! show sizes.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

/// Errors produced while reading or writing FSEQ files.
#[derive(Debug, thiserror::Error)]
pub enum FseqError {
    /// Underlying I/O failure.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// The file is not a (supported) FSEQ file or its header is inconsistent.
    #[error("invalid FSEQ file: {0}")]
    Format(String),
    /// A compressed block could not be decoded.
    #[error("failed to decompress FSEQ block {block}: {message}")]
    Decompress {
        /// Index of the failing block.
        block: usize,
        /// Decoder error message.
        message: String,
    },
    /// The requested frame does not exist.
    #[error("frame {frame} is out of range (sequence has {frame_count} frames)")]
    FrameOutOfRange {
        /// Requested frame index.
        frame: u32,
        /// Number of frames in the sequence.
        frame_count: u32,
    },
    /// A writer argument is invalid (wrong frame size, too many blocks, ...).
    #[error("invalid FSEQ writer usage: {0}")]
    Writer(String),
}

/// Result alias for this module.
pub type Result<T> = std::result::Result<T, FseqError>;

/// Channel-data compression used by an FSEQ v2 file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compression {
    /// Raw channel data.
    None,
    /// Zstandard (the xLights default).
    #[default]
    Zstd,
    /// zlib (deflate with zlib wrapper).
    Zlib,
}

impl Compression {
    fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Compression::None),
            1 => Some(Compression::Zstd),
            2 => Some(Compression::Zlib),
            _ => None,
        }
    }

    fn code(self) -> u8 {
        match self {
            Compression::None => 0,
            Compression::Zstd => 1,
            Compression::Zlib => 2,
        }
    }
}

/// A contiguous run of channels stored in a sparse FSEQ file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SparseRange {
    /// First channel (0-based absolute channel number).
    pub start: u32,
    /// Number of channels.
    pub len: u32,
}

/// A variable-length header entry (`mf`, `sp`, `FC`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariableHeader {
    /// Two-character code, e.g. `*b"mf"`.
    pub code: [u8; 2],
    /// Raw payload (strings are NUL terminated in the file; the terminator is kept).
    pub data: Vec<u8>,
}

impl VariableHeader {
    /// The payload interpreted as a string (up to the first NUL, lossily decoded).
    pub fn as_str(&self) -> String {
        let end = self
            .data
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.data.len());
        String::from_utf8_lossy(&self.data[..end]).into_owned()
    }
}

/// Parsed FSEQ header.
#[derive(Debug, Clone, PartialEq)]
pub struct FseqHeader {
    /// Major format version (1 or 2).
    pub version_major: u8,
    /// Minor format version.
    pub version_minor: u8,
    /// Byte offset of the first channel data byte.
    pub channel_data_offset: u64,
    /// Channels stored per frame (for sparse files: the sum of the sparse range lengths).
    pub channel_count: u32,
    /// Number of frames.
    pub frame_count: u32,
    /// Frame duration in milliseconds (typically 25 or 50).
    pub step_time_ms: u8,
    /// Channel data compression.
    pub compression: Compression,
    /// Sparse ranges (empty for a dense file).
    pub sparse_ranges: Vec<SparseRange>,
    /// Timestamp-based unique id written by the producer (v2 only; 0 for v1).
    pub unique_id: u64,
    /// All variable headers in file order.
    pub variable_headers: Vec<VariableHeader>,
}

impl FseqHeader {
    /// Value of the variable header `code` interpreted as a string.
    pub fn variable_str(&self, code: &[u8; 2]) -> Option<String> {
        self.variable_headers
            .iter()
            .find(|h| &h.code == code)
            .map(VariableHeader::as_str)
            .filter(|s| !s.is_empty())
    }

    /// Media (audio) filename from the `mf` header, as written by xLights (often an
    /// absolute path from the sequencing PC).
    pub fn media_filename(&self) -> Option<String> {
        self.variable_str(b"mf")
    }

    /// Just the file-name part of [`FseqHeader::media_filename`] (handles both `/` and
    /// `\` separators), handy for auto-linking uploaded audio.
    pub fn media_basename(&self) -> Option<String> {
        self.media_filename().map(|m| {
            m.rsplit(['/', '\\'])
                .next()
                .unwrap_or(m.as_str())
                .to_string()
        })
    }

    /// Producer string from the `sp` header (e.g. `xLights macOS 2024.10`).
    pub fn producer(&self) -> Option<String> {
        self.variable_str(b"sp")
    }

    /// Size in bytes of a frame in absolute channel space.
    pub fn frame_size(&self) -> usize {
        if self.sparse_ranges.is_empty() {
            self.channel_count as usize
        } else {
            self.sparse_ranges
                .iter()
                .map(|r| r.start as usize + r.len as usize)
                .max()
                .unwrap_or(0)
        }
    }

    /// Total duration in milliseconds.
    pub fn duration_ms(&self) -> u64 {
        self.frame_count as u64 * self.step_time_ms as u64
    }
}

/// One compressed block of frames.
#[derive(Debug, Clone, Copy)]
struct Block {
    first_frame: u32,
    /// Exclusive.
    end_frame: u32,
    offset: u64,
    len: u64,
}

/// Decompressed block cache (one block kept at a time).
#[derive(Default)]
struct BlockCache {
    block: Option<usize>,
    data: Vec<u8>,
    compressed: Vec<u8>,
    zstd: Option<zstd::bulk::Decompressor<'static>>,
    zlib: Option<flate2::Decompress>,
}

/// An open FSEQ file with random frame access.
pub struct FseqFile<R = BufReader<File>> {
    reader: R,
    header: FseqHeader,
    blocks: Vec<Block>,
    cache: BlockCache,
    /// Scratch buffer holding one stored (compact) frame of a sparse uncompressed file.
    scratch: Vec<u8>,
    /// Reader position when known, so sequential reads skip the seek (and keep any
    /// read-ahead buffer of the underlying reader).
    pos: Option<u64>,
}

impl<R> std::fmt::Debug for FseqFile<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FseqFile")
            .field("header", &self.header)
            .field("blocks", &self.blocks.len())
            .finish()
    }
}

const FIXED_V1_HEADER: usize = 28;
const FIXED_V2_HEADER: usize = 32;

fn u16le(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn u24le(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], 0])
}
fn u32le(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
fn u64le(b: &[u8], at: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(a)
}

impl FseqFile<BufReader<File>> {
    /// Open and parse an FSEQ file from disk.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::open(path.as_ref())?;
        Self::from_reader(BufReader::with_capacity(64 * 1024, file))
    }
}

impl<R: Read + Seek> FseqFile<R> {
    /// Parse an FSEQ file from any seekable reader (e.g. an in-memory `Cursor`).
    pub fn from_reader(mut reader: R) -> Result<Self> {
        let file_len = reader.seek(SeekFrom::End(0))?;
        reader.seek(SeekFrom::Start(0))?;
        let mut fixed = [0u8; 10];
        read_exact_or_format(&mut reader, &mut fixed, "file shorter than FSEQ header")?;
        if &fixed[0..4] != b"PSEQ" && &fixed[0..4] != b"FSEQ" {
            if &fixed[0..4] == b"ESEQ" {
                return Err(FseqError::Format(
                    "ESEQ effect sequences are not supported".into(),
                ));
            }
            return Err(FseqError::Format("missing PSEQ magic".into()));
        }
        let data_offset = u16le(&fixed, 4) as usize;
        let minor = fixed[6];
        let major = fixed[7];
        let min_header = match major {
            1 => FIXED_V1_HEADER,
            2 => FIXED_V2_HEADER,
            _ => {
                return Err(FseqError::Format(format!(
                    "unsupported FSEQ version {major}.{minor}"
                )))
            }
        };
        if data_offset < min_header {
            return Err(FseqError::Format(format!(
                "channel data offset {data_offset} is inside the fixed header"
            )));
        }
        if data_offset as u64 > file_len {
            return Err(FseqError::Format(
                "channel data offset beyond end of file".into(),
            ));
        }
        let mut raw = vec![0u8; data_offset];
        reader.seek(SeekFrom::Start(0))?;
        read_exact_or_format(&mut reader, &mut raw, "truncated FSEQ header")?;

        let channel_count = u32le(&raw, 10);
        let frame_count = u32le(&raw, 14);
        let step_time_ms = raw[18];
        if step_time_ms == 0 {
            return Err(FseqError::Format("step time of 0 ms".into()));
        }

        let mut header = FseqHeader {
            version_major: major,
            version_minor: minor,
            channel_data_offset: data_offset as u64,
            channel_count,
            frame_count,
            step_time_ms,
            compression: Compression::None,
            sparse_ranges: Vec::new(),
            unique_id: 0,
            variable_headers: Vec::new(),
        };
        let mut blocks = Vec::new();

        if major == 1 {
            let var_start = (u16le(&raw, 8) as usize).clamp(FIXED_V1_HEADER, data_offset);
            header.variable_headers =
                parse_variable_headers(&raw, var_start, &mut reader, file_len)?;
        } else {
            let compression = Compression::from_code(raw[20] & 0x0F).ok_or_else(|| {
                FseqError::Format(format!("unknown compression type {}", raw[20] & 0x0F))
            })?;
            header.compression = compression;
            let block_count = (((raw[20] & 0xF0) as usize) << 4) | raw[21] as usize;
            let sparse_count = raw[22] as usize;
            header.unique_id = u64le(&raw, 24);
            let mut pos = FIXED_V2_HEADER;
            let table_end = pos + block_count * 8 + sparse_count * 6;
            if table_end > data_offset {
                return Err(FseqError::Format(
                    "block/sparse index extends past channel data offset".into(),
                ));
            }
            let mut offset = data_offset as u64;
            for _ in 0..block_count {
                let first = u32le(&raw, pos);
                let len = u32le(&raw, pos + 4) as u64;
                pos += 8;
                if len == 0 {
                    continue;
                }
                if let Some(prev) = blocks.last() {
                    let prev: &Block = prev;
                    if first < prev.first_frame {
                        return Err(FseqError::Format(format!(
                            "block table out of order (block starting at frame {first} follows {})",
                            prev.first_frame
                        )));
                    }
                } else if first != 0 {
                    return Err(FseqError::Format(
                        "first compression block does not start at frame 0".into(),
                    ));
                }
                blocks.push(Block {
                    first_frame: first,
                    end_frame: 0,
                    offset,
                    len,
                });
                offset += len;
            }
            for _ in 0..sparse_count {
                let start = u24le(&raw, pos);
                let len = u24le(&raw, pos + 3);
                pos += 6;
                header.sparse_ranges.push(SparseRange { start, len });
            }
            let var_start = (u16le(&raw, 8) as usize).clamp(pos, data_offset);
            header.variable_headers =
                parse_variable_headers(&raw, var_start, &mut reader, file_len)?;

            if compression != Compression::None {
                if blocks.is_empty() {
                    // Tolerate writers that never filled the table: treat all channel
                    // data as a single block (mirrors xLights' recovery behaviour).
                    blocks.push(Block {
                        first_frame: 0,
                        end_frame: 0,
                        offset: data_offset as u64,
                        len: file_len - data_offset as u64,
                    });
                }
                if blocks.last().map(|b| b.offset + b.len).unwrap_or(0) > file_len {
                    return Err(FseqError::Format(
                        "compressed blocks extend past end of file".into(),
                    ));
                }
                let n = blocks.len();
                for i in 0..n {
                    let end = if i + 1 < n {
                        blocks[i + 1].first_frame
                    } else {
                        frame_count
                    };
                    blocks[i].end_frame = end.min(frame_count).max(blocks[i].first_frame);
                }
            } else {
                blocks.clear();
            }
        }

        if !header.sparse_ranges.is_empty() {
            let sum: u64 = header.sparse_ranges.iter().map(|r| r.len as u64).sum();
            if sum != channel_count as u64 {
                return Err(FseqError::Format(format!(
                    "sparse ranges cover {sum} channels but header declares {channel_count}"
                )));
            }
        }
        if header.compression == Compression::None {
            let needed = data_offset as u64 + frame_count as u64 * channel_count as u64;
            if needed > file_len {
                return Err(FseqError::Format(format!(
                    "file is truncated: {frame_count} frames of {channel_count} channels need {needed} bytes, file has {file_len}"
                )));
            }
        }

        let scratch = if header.sparse_ranges.is_empty() {
            Vec::new()
        } else {
            vec![0u8; channel_count as usize]
        };
        Ok(FseqFile {
            reader,
            header,
            blocks,
            cache: BlockCache::default(),
            scratch,
            pos: None,
        })
    }

    /// The parsed header.
    pub fn header(&self) -> &FseqHeader {
        &self.header
    }

    /// Number of frames.
    pub fn frame_count(&self) -> u32 {
        self.header.frame_count
    }

    /// Frame duration in milliseconds.
    pub fn frame_ms(&self) -> u32 {
        self.header.step_time_ms as u32
    }

    /// Stored channels per frame (see [`FseqHeader::channel_count`]).
    pub fn channel_count(&self) -> u32 {
        self.header.channel_count
    }

    /// Bytes needed to hold one frame in absolute channel space.
    pub fn frame_size(&self) -> usize {
        self.header.frame_size()
    }

    /// Total duration in milliseconds.
    pub fn duration_ms(&self) -> u64 {
        self.header.duration_ms()
    }

    /// Media filename from the `mf` variable header.
    pub fn media_filename(&self) -> Option<String> {
        self.header.media_filename()
    }

    /// Number of compression blocks (0 for uncompressed files).
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// Frame index that is playing at `pos_ms` (clamped to the last frame).
    pub fn frame_at_ms(&self, pos_ms: u64) -> u32 {
        let f = pos_ms / self.header.step_time_ms as u64;
        f.min(self.header.frame_count.saturating_sub(1) as u64) as u32
    }

    /// Read frame `idx` into `buf` in absolute channel space.
    ///
    /// Up to `buf.len()` bytes are written. If `buf` is longer than
    /// [`FseqFile::frame_size`] the excess is zeroed; if it is shorter, the frame is
    /// truncated. Channels not stored in a sparse file are zeroed.
    pub fn frame(&mut self, idx: u32, buf: &mut [u8]) -> Result<()> {
        if idx >= self.header.frame_count {
            return Err(FseqError::FrameOutOfRange {
                frame: idx,
                frame_count: self.header.frame_count,
            });
        }
        let cc = self.header.channel_count as usize;
        let sparse = !self.header.sparse_ranges.is_empty();

        if self.header.compression == Compression::None {
            let at = self.header.channel_data_offset + idx as u64 * cc as u64;
            if sparse {
                read_at(&mut self.reader, &mut self.pos, at, &mut self.scratch)?;
                scatter_sparse(&self.header.sparse_ranges, &self.scratch, buf);
            } else {
                let n = cc.min(buf.len());
                read_at(&mut self.reader, &mut self.pos, at, &mut buf[..n])?;
                buf[n..].fill(0);
            }
            return Ok(());
        }

        let block = self.block_for_frame(idx)?;
        self.load_block(block)?;
        let b = self.blocks[block];
        let start = (idx - b.first_frame) as usize * cc;
        let end = start + cc;
        if end > self.cache.data.len() {
            return Err(FseqError::Decompress {
                block,
                message: format!(
                    "block decompressed to {} bytes, frame {idx} needs {end}",
                    self.cache.data.len()
                ),
            });
        }
        let stored = &self.cache.data[start..end];
        if sparse {
            scatter_sparse(&self.header.sparse_ranges, stored, buf);
        } else {
            let n = cc.min(buf.len());
            buf[..n].copy_from_slice(&stored[..n]);
            buf[n..].fill(0);
        }
        Ok(())
    }

    /// Iterate over all frames from `start`.
    pub fn frames_from(&mut self, start: u32) -> FrameReader<'_, R> {
        let size = self.frame_size();
        FrameReader {
            file: self,
            next: start,
            buf: vec![0u8; size],
        }
    }

    /// Iterate over all frames.
    pub fn frames(&mut self) -> FrameReader<'_, R> {
        self.frames_from(0)
    }

    fn block_for_frame(&self, idx: u32) -> Result<usize> {
        if let Some(cur) = self.cache.block {
            let b = &self.blocks[cur];
            if idx >= b.first_frame && idx < b.end_frame {
                return Ok(cur);
            }
            // Fast path for sequential playback: the next block.
            if let Some(n) = self.blocks.get(cur + 1) {
                if idx >= n.first_frame && idx < n.end_frame {
                    return Ok(cur + 1);
                }
            }
        }
        let i = self.blocks.partition_point(|b| b.first_frame <= idx);
        if i == 0 {
            return Err(FseqError::Format(format!("no block contains frame {idx}")));
        }
        let i = i - 1;
        if idx >= self.blocks[i].end_frame {
            return Err(FseqError::Format(format!("no block contains frame {idx}")));
        }
        Ok(i)
    }

    fn load_block(&mut self, block: usize) -> Result<()> {
        if self.cache.block == Some(block) {
            return Ok(());
        }
        self.cache.block = None;
        let b = self.blocks[block];
        let cc = self.header.channel_count as usize;
        let expected = (b.end_frame - b.first_frame) as usize * cc;

        let len = usize::try_from(b.len)
            .map_err(|_| FseqError::Format("compressed block too large".into()))?;
        self.cache.compressed.resize(len, 0);
        read_at(
            &mut self.reader,
            &mut self.pos,
            b.offset,
            &mut self.cache.compressed,
        )?;

        self.cache.data.clear();
        self.cache.data.reserve(expected);
        let decomp_err = |message: String| FseqError::Decompress { block, message };
        match self.header.compression {
            Compression::Zstd => {
                if self.cache.zstd.is_none() {
                    self.cache.zstd = Some(
                        zstd::bulk::Decompressor::new().map_err(|e| decomp_err(e.to_string()))?,
                    );
                }
                let dec = self.cache.zstd.as_mut().expect("initialised above");
                // Frames without a content-size field need a generous upper bound; the
                // block table tells us exactly how much to expect.
                let cap = expected.max(1);
                if self.cache.data.capacity() < cap {
                    self.cache.data.reserve(cap);
                }
                dec.decompress_to_buffer(&self.cache.compressed, &mut self.cache.data)
                    .map_err(|e| decomp_err(e.to_string()))?;
            }
            Compression::Zlib => {
                let dec = self
                    .cache
                    .zlib
                    .get_or_insert_with(|| flate2::Decompress::new(true));
                dec.reset(true);
                loop {
                    if self.cache.data.len() == self.cache.data.capacity() {
                        self.cache.data.reserve(expected.max(4096));
                    }
                    let consumed = dec.total_in() as usize;
                    let produced = self.cache.data.len();
                    let status = dec
                        .decompress_vec(
                            &self.cache.compressed[consumed..],
                            &mut self.cache.data,
                            flate2::FlushDecompress::Finish,
                        )
                        .map_err(|e| decomp_err(e.to_string()))?;
                    match status {
                        flate2::Status::StreamEnd => break,
                        flate2::Status::Ok | flate2::Status::BufError => {
                            let stalled = dec.total_in() as usize == consumed
                                && self.cache.data.len() == produced
                                && self.cache.data.len() < self.cache.data.capacity();
                            if stalled
                                || (dec.total_in() as usize >= self.cache.compressed.len()
                                    && self.cache.data.len() < self.cache.data.capacity())
                            {
                                // Input exhausted without a stream end: use what we have.
                                break;
                            }
                        }
                    }
                }
            }
            Compression::None => unreachable!("uncompressed files have no blocks"),
        }
        self.cache.block = Some(block);
        Ok(())
    }
}

/// Read exactly `buf.len()` bytes at `at`, seeking only when the tracked position
/// differs. On failure the position becomes unknown.
fn read_at<R: Read + Seek>(
    reader: &mut R,
    pos: &mut Option<u64>,
    at: u64,
    buf: &mut [u8],
) -> io::Result<()> {
    let res = (|| {
        if *pos != Some(at) {
            reader.seek(SeekFrom::Start(at))?;
        }
        reader.read_exact(buf)
    })();
    *pos = match res {
        Ok(()) => Some(at + buf.len() as u64),
        Err(_) => None,
    };
    res
}

/// Copy a compact sparse frame into absolute channel space.
fn scatter_sparse(ranges: &[SparseRange], stored: &[u8], buf: &mut [u8]) {
    buf.fill(0);
    let mut src = 0usize;
    for r in ranges {
        let len = r.len as usize;
        let start = r.start as usize;
        if start < buf.len() {
            let n = len.min(buf.len() - start);
            buf[start..start + n].copy_from_slice(&stored[src..src + n]);
        }
        src += len;
    }
}

fn read_exact_or_format<R: Read>(r: &mut R, buf: &mut [u8], what: &str) -> Result<()> {
    r.read_exact(buf).map_err(|e| {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            FseqError::Format(what.to_string())
        } else {
            FseqError::Io(e)
        }
    })
}

fn parse_variable_headers<R: Read + Seek>(
    raw: &[u8],
    mut pos: usize,
    reader: &mut R,
    file_len: u64,
) -> Result<Vec<VariableHeader>> {
    let mut out = Vec::new();
    while pos + 4 <= raw.len() {
        let len = u16le(raw, pos) as usize;
        let code = [raw[pos + 2], raw[pos + 3]];
        if len == 0 && code == [0, 0] {
            break; // padding
        }
        if len < 4 {
            // Empty header: skip just the length+code.
            pos += 4;
            continue;
        }
        if pos + len > raw.len() {
            // Corrupt length: stop parsing variable headers (data is still readable).
            break;
        }
        let payload = &raw[pos + 4..pos + len];
        if code == *b"ED" && payload.len() >= 14 {
            let real_code = [payload[0], payload[1]];
            let offset = u64le(payload, 2);
            let dlen = u32le(payload, 10) as u64;
            if offset + dlen <= file_len {
                let here = reader.stream_position()?;
                reader.seek(SeekFrom::Start(offset))?;
                let mut data = vec![0u8; dlen as usize];
                reader.read_exact(&mut data)?;
                reader.seek(SeekFrom::Start(here))?;
                out.push(VariableHeader {
                    code: real_code,
                    data,
                });
            }
        } else {
            out.push(VariableHeader {
                code,
                data: payload.to_vec(),
            });
        }
        pos += len;
    }
    Ok(out)
}

/// Sequential frame iterator returned by [`FseqFile::frames`].
///
/// Use [`FrameReader::next_frame`] for allocation-free iteration (the returned slice
/// is valid until the next call); the [`Iterator`] implementation yields owned copies.
pub struct FrameReader<'a, R> {
    file: &'a mut FseqFile<R>,
    next: u32,
    buf: Vec<u8>,
}

impl<R: Read + Seek> FrameReader<'_, R> {
    /// Next frame index and a borrowed frame, or `None` at the end.
    pub fn next_frame(&mut self) -> Option<Result<(u32, &[u8])>> {
        if self.next >= self.file.frame_count() {
            return None;
        }
        let idx = self.next;
        self.next += 1;
        match self.file.frame(idx, &mut self.buf) {
            Ok(()) => Some(Ok((idx, &self.buf))),
            Err(e) => {
                self.next = u32::MAX;
                Some(Err(e))
            }
        }
    }

    /// Skip ahead so that the next frame returned is `idx`.
    pub fn seek(&mut self, idx: u32) {
        self.next = idx;
    }
}

impl<R: Read + Seek> Iterator for FrameReader<'_, R> {
    type Item = Result<Vec<u8>>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_frame().map(|r| r.map(|(_, f)| f.to_vec()))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.file.frame_count().saturating_sub(self.next) as usize;
        (left, Some(left))
    }
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

/// Options for [`FseqWriter`].
#[derive(Debug, Clone)]
pub struct FseqWriterOptions {
    /// Frame size in absolute channel space (bytes passed to [`FseqWriter::write_frame`]).
    pub channel_count: u32,
    /// Frame duration in ms (1..=255).
    pub frame_ms: u8,
    /// Channel data compression.
    pub compression: Compression,
    /// zstd level (1..=22) or zlib level (0..=9, clamped).
    pub level: i32,
    /// Frames per compression block.
    pub frames_per_block: u32,
    /// Block index entries reserved in the header. More than 255 produces a v2.2 file.
    pub max_blocks: u32,
    /// Only store these channel ranges (sparse file). Empty = all channels.
    pub sparse_ranges: Vec<SparseRange>,
    /// Written as the `mf` variable header.
    pub media_filename: Option<String>,
    /// Written as the `sp` variable header.
    pub producer: Option<String>,
}

impl FseqWriterOptions {
    /// Defaults: zstd level 3, 64 frames per block, up to 255 blocks, producer
    /// `PixelPlus`.
    pub fn new(channel_count: u32, frame_ms: u8) -> Self {
        FseqWriterOptions {
            channel_count,
            frame_ms,
            compression: Compression::Zstd,
            level: 3,
            frames_per_block: 64,
            max_blocks: 255,
            sparse_ranges: Vec::new(),
            media_filename: None,
            producer: Some("PixelPlus".into()),
        }
    }

    /// Adjust `frames_per_block` so that `frame_count` frames fit into `max_blocks`.
    pub fn fit_frame_count(mut self, frame_count: u32) -> Self {
        let max = self.max_blocks.max(1);
        let needed = frame_count.div_ceil(max);
        self.frames_per_block = self.frames_per_block.max(needed).max(1);
        self
    }
}

/// Streaming FSEQ v2 writer.
///
/// The header is reserved up front and patched by [`FseqWriter::finish`], so frames are
/// written straight to the output and memory use stays bounded by one block.
pub struct FseqWriter<W: Write + Seek = BufWriter<File>> {
    out: W,
    opts: FseqWriterOptions,
    stored_channels: u32,
    data_offset: u64,
    header_size: usize,
    frames: u32,
    /// (first frame, compressed length)
    blocks: Vec<(u32, u32)>,
    pending: Vec<u8>,
    pending_frames: u32,
    pos: u64,
}

impl FseqWriter<BufWriter<File>> {
    /// Create `path` and prepare to write frames.
    pub fn create(path: impl AsRef<Path>, opts: FseqWriterOptions) -> Result<Self> {
        let f = File::create(path.as_ref())?;
        Self::new(BufWriter::with_capacity(256 * 1024, f), opts)
    }
}

impl<W: Write + Seek> FseqWriter<W> {
    /// Write to any seekable sink.
    pub fn new(mut out: W, opts: FseqWriterOptions) -> Result<Self> {
        if opts.frame_ms == 0 {
            return Err(FseqError::Writer("frame_ms must be at least 1".into()));
        }
        if opts.frames_per_block == 0 {
            return Err(FseqError::Writer(
                "frames_per_block must be at least 1".into(),
            ));
        }
        if opts.max_blocks == 0 || opts.max_blocks > 4095 {
            return Err(FseqError::Writer("max_blocks must be 1..=4095".into()));
        }
        if opts.sparse_ranges.len() > 255 {
            return Err(FseqError::Writer("at most 255 sparse ranges".into()));
        }
        for r in &opts.sparse_ranges {
            if r.start > 0xFF_FFFF || r.len > 0xFF_FFFF {
                return Err(FseqError::Writer("sparse range exceeds 24 bits".into()));
            }
            if r.start as u64 + r.len as u64 > opts.channel_count as u64 {
                return Err(FseqError::Writer(format!(
                    "sparse range {}+{} exceeds channel count {}",
                    r.start, r.len, opts.channel_count
                )));
            }
        }
        let stored_channels = if opts.sparse_ranges.is_empty() {
            opts.channel_count
        } else {
            opts.sparse_ranges.iter().map(|r| r.len).sum()
        };
        let reserved_blocks = if opts.compression == Compression::None {
            0
        } else {
            opts.max_blocks as usize
        };
        let header_size = FIXED_V2_HEADER + reserved_blocks * 8 + opts.sparse_ranges.len() * 6;
        let var_len: usize = Self::variable_headers(&opts)
            .iter()
            .map(|(_, d)| 4 + d.len())
            .sum();
        let data_offset = (header_size + var_len).div_ceil(4) * 4;
        if data_offset > u16::MAX as usize {
            return Err(FseqError::Writer("header exceeds 64 KiB".into()));
        }
        out.write_all(&vec![0u8; data_offset])?;
        Ok(FseqWriter {
            out,
            stored_channels,
            data_offset: data_offset as u64,
            header_size,
            frames: 0,
            blocks: Vec::new(),
            pending: Vec::new(),
            pending_frames: 0,
            pos: data_offset as u64,
            opts,
        })
    }

    fn variable_headers(opts: &FseqWriterOptions) -> Vec<([u8; 2], Vec<u8>)> {
        let mut v = Vec::new();
        for (code, val) in [(*b"mf", &opts.media_filename), (*b"sp", &opts.producer)] {
            if let Some(s) = val {
                let mut d = s.as_bytes().to_vec();
                d.push(0);
                v.push((code, d));
            }
        }
        v
    }

    /// Append one frame (exactly `channel_count` bytes, absolute channel space).
    pub fn write_frame(&mut self, frame: &[u8]) -> Result<()> {
        if frame.len() != self.opts.channel_count as usize {
            return Err(FseqError::Writer(format!(
                "frame has {} bytes, expected {}",
                frame.len(),
                self.opts.channel_count
            )));
        }
        if self.frames == u32::MAX {
            return Err(FseqError::Writer("too many frames".into()));
        }
        if self.opts.sparse_ranges.is_empty() {
            self.pending.extend_from_slice(frame);
        } else {
            for r in &self.opts.sparse_ranges {
                self.pending
                    .extend_from_slice(&frame[r.start as usize..(r.start + r.len) as usize]);
            }
        }
        self.pending_frames += 1;
        self.frames += 1;
        if self.opts.compression == Compression::None
            || self.pending_frames >= self.opts.frames_per_block
        {
            self.flush_block()?;
        }
        Ok(())
    }

    fn flush_block(&mut self) -> Result<()> {
        if self.pending_frames == 0 {
            return Ok(());
        }
        let first = self.frames - self.pending_frames;
        let bytes = match self.opts.compression {
            Compression::None => {
                self.out.write_all(&self.pending)?;
                self.pending.len()
            }
            Compression::Zstd => {
                let c = zstd::bulk::compress(&self.pending, self.opts.level.clamp(1, 22))?;
                self.out.write_all(&c)?;
                c.len()
            }
            Compression::Zlib => {
                let level = flate2::Compression::new(self.opts.level.clamp(0, 9) as u32);
                let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), level);
                enc.write_all(&self.pending)?;
                let c = enc.finish()?;
                self.out.write_all(&c)?;
                c.len()
            }
        };
        if self.opts.compression != Compression::None {
            if self.blocks.len() >= self.opts.max_blocks as usize {
                return Err(FseqError::Writer(format!(
                    "more than {} blocks needed; raise frames_per_block or max_blocks",
                    self.opts.max_blocks
                )));
            }
            let len = u32::try_from(bytes)
                .map_err(|_| FseqError::Writer("compressed block exceeds 4 GiB".into()))?;
            self.blocks.push((first, len));
        }
        self.pos += bytes as u64;
        self.pending.clear();
        self.pending_frames = 0;
        Ok(())
    }

    /// Number of frames written so far.
    pub fn frames_written(&self) -> u32 {
        self.frames
    }

    /// Flush the last block, write the header and return the sink.
    pub fn finish(mut self) -> Result<W> {
        self.flush_block()?;
        let mut h = vec![0u8; self.data_offset as usize];
        h[0..4].copy_from_slice(b"PSEQ");
        h[4..6].copy_from_slice(&(self.data_offset as u16).to_le_bytes());
        let reserved = if self.opts.compression == Compression::None {
            0
        } else {
            self.opts.max_blocks
        };
        h[6] = if reserved > 255 { 2 } else { 0 };
        h[7] = 2;
        h[8..10].copy_from_slice(&(self.header_size as u16).to_le_bytes());
        h[10..14].copy_from_slice(&self.stored_channels.to_le_bytes());
        h[14..18].copy_from_slice(&self.frames.to_le_bytes());
        h[18] = self.opts.frame_ms;
        h[19] = 0;
        h[20] = (((reserved >> 4) & 0xF0) as u8) | self.opts.compression.code();
        h[21] = (reserved & 0xFF) as u8;
        h[22] = self.opts.sparse_ranges.len() as u8;
        h[23] = 0;
        let uid = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros() as u64)
            .unwrap_or(0);
        h[24..32].copy_from_slice(&uid.to_le_bytes());
        let mut p = FIXED_V2_HEADER;
        for i in 0..reserved as usize {
            if let Some(&(first, len)) = self.blocks.get(i) {
                h[p..p + 4].copy_from_slice(&first.to_le_bytes());
                h[p + 4..p + 8].copy_from_slice(&len.to_le_bytes());
            }
            p += 8;
        }
        for r in &self.opts.sparse_ranges {
            h[p..p + 3].copy_from_slice(&r.start.to_le_bytes()[..3]);
            h[p + 3..p + 6].copy_from_slice(&r.len.to_le_bytes()[..3]);
            p += 6;
        }
        for (code, data) in Self::variable_headers(&self.opts) {
            let len = (4 + data.len()) as u16;
            h[p..p + 2].copy_from_slice(&len.to_le_bytes());
            h[p + 2..p + 4].copy_from_slice(&code);
            h[p + 4..p + 4 + data.len()].copy_from_slice(&data);
            p += 4 + data.len();
        }
        self.out.seek(SeekFrom::Start(0))?;
        self.out.write_all(&h)?;
        self.out.seek(SeekFrom::Start(self.pos))?;
        self.out.flush()?;
        Ok(self.out)
    }
}

/// Lower-case hex SHA-256 of a file's contents (streamed; constant memory).
pub fn sha256_file(path: impl AsRef<Path>) -> io::Result<String> {
    let mut f = File::open(path.as_ref())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(to_hex(&hasher.finalize()))
}

/// Raw 32-byte SHA-256 of a file's contents.
pub fn sha256_file_bytes(path: impl AsRef<Path>) -> io::Result<[u8; 32]> {
    let mut f = File::open(path.as_ref())?;
    let mut hasher = Sha256::new();
    io::copy(&mut f, &mut hasher)?;
    Ok(hasher.finalize().into())
}

/// Lower-case hex encoding.
pub fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xF) as usize] as char);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Deterministic, frame-dependent test pattern.
    fn pattern(frame: u32, channels: usize) -> Vec<u8> {
        (0..channels)
            .map(|c| ((c as u32).wrapping_mul(31) ^ frame.wrapping_mul(7)) as u8)
            .collect()
    }

    fn write_mem(opts: FseqWriterOptions, frames: u32) -> Vec<u8> {
        let cc = opts.channel_count as usize;
        let mut w = FseqWriter::new(Cursor::new(Vec::new()), opts).unwrap();
        for f in 0..frames {
            w.write_frame(&pattern(f, cc)).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn roundtrip(compression: Compression) {
        let mut opts = FseqWriterOptions::new(300, 25);
        opts.compression = compression;
        opts.frames_per_block = 7;
        opts.media_filename = Some("C:\\Show\\Audio\\Jingle Bells.mp3".into());
        let bytes = write_mem(opts, 50);
        let mut f = FseqFile::from_reader(Cursor::new(bytes)).unwrap();
        assert_eq!(f.frame_count(), 50);
        assert_eq!(f.frame_ms(), 25);
        assert_eq!(f.channel_count(), 300);
        assert_eq!(f.duration_ms(), 1250);
        assert_eq!(f.header().compression, compression);
        assert_eq!(
            f.media_filename().as_deref(),
            Some("C:\\Show\\Audio\\Jingle Bells.mp3")
        );
        assert_eq!(
            f.header().media_basename().as_deref(),
            Some("Jingle Bells.mp3")
        );
        assert_eq!(f.header().producer().as_deref(), Some("PixelPlus"));
        if compression != Compression::None {
            assert_eq!(f.block_count(), 8);
        }
        let mut buf = vec![0u8; 300];
        // Random access, including backwards jumps across blocks.
        for &i in &[0u32, 49, 13, 14, 6, 7, 42, 0, 35] {
            f.frame(i, &mut buf).unwrap();
            assert_eq!(buf, pattern(i, 300), "frame {i}");
        }
        let all: Vec<_> = f.frames().collect::<Result<_>>().unwrap();
        assert_eq!(all.len(), 50);
        for (i, fr) in all.iter().enumerate() {
            assert_eq!(fr, &pattern(i as u32, 300));
        }
    }

    #[test]
    fn roundtrip_zstd() {
        roundtrip(Compression::Zstd);
    }

    #[test]
    fn roundtrip_zlib() {
        roundtrip(Compression::Zlib);
    }

    #[test]
    fn roundtrip_uncompressed() {
        roundtrip(Compression::None);
    }

    #[test]
    fn sparse_ranges_scatter_to_absolute_channels() {
        let mut opts = FseqWriterOptions::new(1000, 50);
        opts.sparse_ranges = vec![
            SparseRange { start: 10, len: 30 },
            SparseRange {
                start: 600,
                len: 90,
            },
        ];
        opts.frames_per_block = 4;
        let bytes = write_mem(opts, 10);
        let mut f = FseqFile::from_reader(Cursor::new(bytes)).unwrap();
        assert_eq!(f.channel_count(), 120);
        assert_eq!(f.frame_size(), 690);
        let mut buf = vec![0xAAu8; 1000];
        f.frame(5, &mut buf).unwrap();
        let expect = pattern(5, 1000);
        for (c, &b) in buf.iter().enumerate() {
            let stored = (10..40).contains(&c) || (600..690).contains(&c);
            assert_eq!(b, if stored { expect[c] } else { 0 }, "channel {c}");
        }
    }

    #[test]
    fn sparse_uncompressed() {
        let mut opts = FseqWriterOptions::new(64, 25);
        opts.compression = Compression::None;
        opts.sparse_ranges = vec![SparseRange { start: 32, len: 32 }];
        let bytes = write_mem(opts, 3);
        let mut f = FseqFile::from_reader(Cursor::new(bytes)).unwrap();
        let mut buf = vec![0u8; 64];
        f.frame(2, &mut buf).unwrap();
        assert_eq!(&buf[..32], &[0u8; 32]);
        assert_eq!(&buf[32..], &pattern(2, 64)[32..]);
    }

    #[test]
    fn extended_block_count_v22() {
        let mut opts = FseqWriterOptions::new(9, 25);
        opts.frames_per_block = 1;
        opts.max_blocks = 300;
        let bytes = write_mem(opts, 280);
        assert_eq!(bytes[6], 2, "minor version 2 for > 255 blocks");
        let mut f = FseqFile::from_reader(Cursor::new(bytes)).unwrap();
        assert_eq!(f.block_count(), 280);
        let mut buf = vec![0u8; 9];
        f.frame(279, &mut buf).unwrap();
        assert_eq!(buf, pattern(279, 9));
    }

    #[test]
    fn too_many_blocks_is_an_error() {
        let mut opts = FseqWriterOptions::new(3, 25);
        opts.frames_per_block = 1;
        opts.max_blocks = 2;
        let mut w = FseqWriter::new(Cursor::new(Vec::new()), opts).unwrap();
        w.write_frame(&[1, 2, 3]).unwrap();
        w.write_frame(&[1, 2, 3]).unwrap();
        assert!(matches!(
            w.write_frame(&[1, 2, 3]),
            Err(FseqError::Writer(_))
        ));
    }

    #[test]
    fn fit_frame_count_adjusts_block_size() {
        let o = FseqWriterOptions::new(3, 25).fit_frame_count(100_000);
        assert!(o.frames_per_block * o.max_blocks >= 100_000);
    }

    #[test]
    fn short_and_long_buffers() {
        let bytes = write_mem(FseqWriterOptions::new(12, 25), 2);
        let mut f = FseqFile::from_reader(Cursor::new(bytes)).unwrap();
        let mut short = vec![0u8; 5];
        f.frame(1, &mut short).unwrap();
        assert_eq!(short, pattern(1, 12)[..5]);
        let mut long = vec![0xFFu8; 20];
        f.frame(1, &mut long).unwrap();
        assert_eq!(&long[..12], &pattern(1, 12)[..]);
        assert_eq!(&long[12..], &[0u8; 8]);
    }

    #[test]
    fn out_of_range_frame() {
        let bytes = write_mem(FseqWriterOptions::new(3, 25), 2);
        let mut f = FseqFile::from_reader(Cursor::new(bytes)).unwrap();
        let mut buf = [0u8; 3];
        assert!(matches!(
            f.frame(2, &mut buf),
            Err(FseqError::FrameOutOfRange {
                frame: 2,
                frame_count: 2
            })
        ));
    }

    /// Build a v1 file by hand.
    fn v1_file(channels: u32, frames: u32) -> Vec<u8> {
        let mf = b"song.mp3\0";
        let var_len = 4 + mf.len();
        let offset = (FIXED_V1_HEADER + var_len).div_ceil(4) * 4;
        let mut v = vec![0u8; offset];
        v[0..4].copy_from_slice(b"PSEQ");
        v[4..6].copy_from_slice(&(offset as u16).to_le_bytes());
        v[6] = 0;
        v[7] = 1;
        v[8..10].copy_from_slice(&(FIXED_V1_HEADER as u16).to_le_bytes());
        v[10..14].copy_from_slice(&channels.to_le_bytes());
        v[14..18].copy_from_slice(&frames.to_le_bytes());
        v[18] = 50;
        v[24] = 1;
        v[25] = 2;
        let p = FIXED_V1_HEADER;
        v[p..p + 2].copy_from_slice(&(var_len as u16).to_le_bytes());
        v[p + 2..p + 4].copy_from_slice(b"mf");
        v[p + 4..p + 4 + mf.len()].copy_from_slice(mf);
        for f in 0..frames {
            v.extend(pattern(f, channels as usize));
        }
        v
    }

    #[test]
    fn reads_v1() {
        let mut f = FseqFile::from_reader(Cursor::new(v1_file(30, 4))).unwrap();
        assert_eq!(f.header().version_major, 1);
        assert_eq!(f.frame_ms(), 50);
        assert_eq!(f.media_filename().as_deref(), Some("song.mp3"));
        let mut buf = vec![0u8; 30];
        f.frame(3, &mut buf).unwrap();
        assert_eq!(buf, pattern(3, 30));
    }

    #[test]
    fn rejects_garbage_without_panicking() {
        assert!(FseqFile::from_reader(Cursor::new(Vec::<u8>::new())).is_err());
        assert!(FseqFile::from_reader(Cursor::new(b"hello world, not fseq".to_vec())).is_err());
        let mut v = v1_file(30, 4);
        v.truncate(v.len() - 10);
        assert!(matches!(
            FseqFile::from_reader(Cursor::new(v)),
            Err(FseqError::Format(_))
        ));
        // Header claims a huge data offset.
        let mut v = v1_file(30, 4);
        v[4..6].copy_from_slice(&0xFFFFu16.to_le_bytes());
        assert!(FseqFile::from_reader(Cursor::new(v)).is_err());
        // Corrupted compressed data is reported, not panicked on.
        let mut bytes = write_mem(FseqWriterOptions::new(300, 25), 10);
        let off = u16le(&bytes, 4) as usize;
        for b in &mut bytes[off..off + 16] {
            *b ^= 0x5A;
        }
        let mut f = FseqFile::from_reader(Cursor::new(bytes)).unwrap();
        let mut buf = vec![0u8; 300];
        assert!(f.frame(0, &mut buf).is_err());
        // Random bytes after a valid magic.
        for seed in 0..64u32 {
            let mut v: Vec<u8> = (0..200u32)
                .map(|i| (i.wrapping_mul(2654435761).wrapping_add(seed * 97) >> 13) as u8)
                .collect();
            v[0..4].copy_from_slice(b"PSEQ");
            v[7] = 2;
            if let Ok(mut f) = FseqFile::from_reader(Cursor::new(v)) {
                let mut buf = vec![0u8; f.frame_size().min(1 << 16)];
                let _ = f.frame(0, &mut buf);
            }
        }
    }

    #[test]
    fn sha256_known_value() {
        let dir = std::env::temp_dir().join(format!("ppx-sha-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("abc.txt");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(
            sha256_file(&p).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            to_hex(&sha256_file_bytes(&p).unwrap()),
            sha256_file(&p).unwrap()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn file_roundtrip_on_disk() {
        let dir = std::env::temp_dir().join(format!("ppx-fseq-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.fseq");
        let mut w = FseqWriter::create(&p, FseqWriterOptions::new(600, 25)).unwrap();
        for f in 0..200 {
            w.write_frame(&pattern(f, 600)).unwrap();
        }
        w.finish().unwrap();
        let mut f = FseqFile::open(&p).unwrap();
        let mut reader = f.frames_from(150);
        let (idx, fr) = reader.next_frame().unwrap().unwrap();
        assert_eq!(idx, 150);
        assert_eq!(fr, &pattern(150, 600)[..]);
        assert_eq!(f.frame_at_ms(10_000_000), 199);
        std::fs::remove_dir_all(&dir).ok();
    }
}
