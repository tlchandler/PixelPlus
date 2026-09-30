//! Browser preview file format `PPPV` v1 (F3, ARCHITECTURE §12.3).
//!
//! A compact, downloadable rendering of a sequence per prop, so a phone can
//! play the whole display in time with the song without touching the lights.
//! The server builds it once per sequence and layout; any number of phones
//! then download it.
//!
//! ```text
//! "PPPV" | u32 LE header length | header JSON (space padded) | block 0 | block 1 | …
//! ```
//!
//! * Header ([`PreviewHeader`], camelCase JSON): `{v, seqId, frameMs,
//!   frameCount, frameBytes, props:[{id, n, idx?}], blockFrames, blocks:[{offset,
//!   len}], mappingHash}`. `offset` is absolute in the file, so a browser can
//!   fetch any block with one HTTP Range request.
//! * A frame is, for each prop in header order, `n` RGB triplets (the prop's
//!   sampled pixels in `idx` order; all pixels in order when `idx` is absent).
//!   Colours are channel data as sequenced (before colour order, gamma and
//!   brightness), the same as the live WebSocket preview.
//! * Each block holds `blockFrames` (64) frames (the last may hold fewer) and
//!   is **gzip** compressed on its own: browsers inflate it with the native
//!   `DecompressionStream('gzip')`.
//! * `frameMs` ≥ 50 (≤ 20 fps): faster sequences keep every k-th frame.
//! * Props over [`MAX_PROP_SAMPLES`] pixels keep a uniform subsample
//!   (matrices a grid), and the whole preview at most [`MAX_TOTAL_SAMPLES`].

use crate::fseq::FseqFile;
use crate::model::Prop;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

pub const MAGIC: &[u8; 4] = b"PPPV";
pub const VERSION: u32 = 1;
/// Frames per gzip block.
pub const BLOCK_FRAMES: u32 = 64;
/// Shortest preview frame (20 fps).
pub const MIN_FRAME_MS: u32 = 50;
/// Props above this many pixels are subsampled.
pub const MAX_PROP_SAMPLES: u32 = 300;
/// Cap on sampled pixels over all props.
pub const MAX_TOTAL_SAMPLES: u32 = 8192;
/// Refuse to read a header larger than this.
const MAX_HEADER: u32 = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum PreviewError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("sequence: {0}")]
    Fseq(#[from] crate::fseq::FseqError),
    #[error("not a preview file: {0}")]
    Format(String),
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, PreviewError>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewHeader {
    pub v: u32,
    pub seq_id: String,
    pub frame_ms: u32,
    pub frame_count: u32,
    /// Bytes per frame (3 × total samples).
    pub frame_bytes: u32,
    pub props: Vec<PreviewProp>,
    pub block_frames: u32,
    pub blocks: Vec<BlockRef>,
    /// Identifies the sequence file + layout it was built from (the ETag).
    pub mapping_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewProp {
    pub id: String,
    /// Sampled pixel count.
    pub n: u32,
    /// Prop pixel index of each sample; absent = all pixels `0..n`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idx: Option<Vec<u32>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockRef {
    pub offset: u64,
    pub len: u32,
}

impl PreviewHeader {
    /// Byte offset of each prop's first sample within a frame.
    pub fn prop_offsets(&self) -> Vec<usize> {
        let mut o = 0usize;
        self.props
            .iter()
            .map(|p| {
                let here = o;
                o += p.n as usize * 3;
                here
            })
            .collect()
    }

    /// Frame shown at `t_ms` (clamped to the last frame).
    pub fn frame_at(&self, t_ms: u64) -> u32 {
        let f = t_ms / u64::from(self.frame_ms.max(1));
        (f.min(u64::from(self.frame_count.saturating_sub(1)))) as u32
    }
}

/// Which prop pixels a preview keeps (see the module docs).
pub fn sample_plan(props: &[Prop]) -> Vec<PreviewProp> {
    let want: Vec<u32> = props
        .iter()
        .map(|p| p.pixel_count.min(MAX_PROP_SAMPLES))
        .collect();
    let total: u64 = want.iter().map(|&n| u64::from(n)).sum();
    let scale = if total > u64::from(MAX_TOTAL_SAMPLES) {
        f64::from(MAX_TOTAL_SAMPLES) / total as f64
    } else {
        1.0
    };
    props
        .iter()
        .zip(want)
        .map(|(p, w)| {
            let n = if scale < 1.0 {
                ((f64::from(w) * scale).floor() as u32).max(u32::from(p.pixel_count > 0))
            } else {
                w
            };
            let idx = if n >= p.pixel_count {
                None
            } else {
                Some(subsample(p, n))
            };
            PreviewProp {
                id: p.id.clone(),
                n: idx.as_ref().map_or(p.pixel_count, |v| v.len() as u32),
                idx,
            }
        })
        .collect()
}

/// About `n` pixel indices spread over the prop: a grid for matrices, else uniform.
fn subsample(p: &Prop, n: u32) -> Vec<u32> {
    if n == 0 || p.pixel_count == 0 {
        return vec![];
    }
    if let Some(m) = &p.matrix {
        let cells = m.width.max(1) * m.height.max(1);
        let stride = ((f64::from(cells) / f64::from(n)).sqrt().ceil() as u32).max(1);
        let mut out: Vec<u32> = Vec::new();
        for y in (0..m.height).step_by(stride as usize) {
            for x in (0..m.width).step_by(stride as usize) {
                let i = (y * m.width + x) as usize;
                if let Some(&px) = m.pixel_map.get(i) {
                    if px >= 0 && (px as u32) < p.pixel_count {
                        out.push(px as u32);
                    }
                }
            }
        }
        if !out.is_empty() {
            out.sort_unstable();
            out.dedup();
            return out;
        }
    }
    if n == 1 {
        return vec![p.pixel_count / 2];
    }
    let last = f64::from(p.pixel_count - 1);
    let mut v: Vec<u32> = (0..n)
        .map(|i| (f64::from(i) * last / f64::from(n - 1)).round() as u32)
        .collect();
    v.dedup();
    v
}

/// Source byte offset in a channel frame of every sample (None = no channel).
fn source_offsets(props: &[Prop], plan: &[PreviewProp]) -> Vec<Option<usize>> {
    let mut out = Vec::new();
    for (p, s) in props.iter().zip(plan) {
        let runs: Vec<_> = p.channel_ranges().collect();
        let at = |px: u32| {
            runs.iter()
                .find(|r| px >= r.prop_offset && px < r.prop_offset + r.pixel_count)
                .map(|r| r.channel_start as usize + (px - r.prop_offset) as usize * 3)
        };
        match &s.idx {
            Some(idx) => out.extend(idx.iter().map(|&i| at(i))),
            None => out.extend((0..s.n).map(at)),
        }
    }
    out
}

/// Preview frame length for a sequence frame length.
pub fn preview_frame_ms(seq_frame_ms: u32) -> u32 {
    let f = seq_frame_ms.max(1);
    f * MIN_FRAME_MS.div_ceil(f)
}

/// Hash of everything a preview depends on: the sequence file (its sha256),
/// the frame rate and each prop's id, pixel count, channel runs and samples.
pub fn mapping_hash(seq_hash: &str, seq_frame_ms: u32, props: &[Prop]) -> String {
    let plan = sample_plan(props);
    let mut h = sha2::Sha256::new();
    h.update(format!("pppv{VERSION}|{seq_hash}|{seq_frame_ms}|").as_bytes());
    for (p, s) in props.iter().zip(&plan) {
        h.update(format!("{}|{}|", p.id, p.pixel_count).as_bytes());
        for r in p.channel_ranges() {
            h.update(
                format!("{},{},{};", r.prop_offset, r.channel_start, r.pixel_count).as_bytes(),
            );
        }
        if let Some(idx) = &s.idx {
            h.update(
                format!(
                    "{}#{}",
                    idx.len(),
                    idx.iter().map(|&i| u64::from(i)).sum::<u64>()
                )
                .as_bytes(),
            );
        }
    }
    crate::fseq::to_hex(&h.finalize())[..16].to_string()
}

/// Build a preview of `fseq` for `props` into `out` (written to a temp file
/// then renamed). `progress(0..1)` returns false to cancel.
pub fn build(
    fseq: &Path,
    seq_id: &str,
    seq_hash: &str,
    props: &[Prop],
    out: &Path,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<PreviewHeader> {
    let mut f = FseqFile::open(fseq)?;
    let seq_frame_ms = f.frame_ms().max(1);
    let frame_ms = preview_frame_ms(seq_frame_ms);
    let step = (frame_ms / seq_frame_ms).max(1);
    let src_frames = f.frame_count();
    let frame_count = src_frames.div_ceil(step);
    let plan = sample_plan(props);
    let src = source_offsets(props, &plan);
    let frame_bytes = src.len() * 3;
    let tmp = out.with_extension("pppv.tmp");
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    let res = (|| -> Result<PreviewHeader> {
        let body_tmp = out.with_extension("pppv.body");
        let mut body = std::io::BufWriter::new(std::fs::File::create(&body_tmp)?);
        let cleanup = || {
            let _ = std::fs::remove_file(&body_tmp);
        };
        let mut blocks: Vec<(u64, u32)> = Vec::new();
        let mut pos = 0u64;
        let mut raw: Vec<u8> = Vec::with_capacity(frame_bytes * BLOCK_FRAMES as usize);
        let mut in_block = 0u32;
        let mut written = 0u32;
        let mut flush =
            |raw: &mut Vec<u8>, body: &mut std::io::BufWriter<std::fs::File>| -> Result<()> {
                let mut enc = flate2::write::GzEncoder::new(
                    Vec::with_capacity(raw.len() / 4),
                    flate2::Compression::new(5),
                );
                enc.write_all(raw)?;
                let gz = enc.finish()?;
                body.write_all(&gz)?;
                blocks.push((pos, gz.len() as u32));
                pos += gz.len() as u64;
                raw.clear();
                Ok(())
            };
        let mut reader = f.frames();
        while let Some(fr) = reader.next_frame() {
            let (idx, data) = match fr {
                Ok(x) => x,
                Err(e) => {
                    cleanup();
                    return Err(e.into());
                }
            };
            if idx % step != 0 {
                continue;
            }
            for o in &src {
                match o {
                    Some(o) if o + 3 <= data.len() => raw.extend_from_slice(&data[*o..o + 3]),
                    _ => raw.extend_from_slice(&[0, 0, 0]),
                }
            }
            in_block += 1;
            written += 1;
            if in_block == BLOCK_FRAMES {
                if let Err(e) = flush(&mut raw, &mut body) {
                    cleanup();
                    return Err(e);
                }
                in_block = 0;
                if !progress(written as f32 / frame_count.max(1) as f32) {
                    cleanup();
                    return Err(PreviewError::Cancelled);
                }
            }
        }
        if in_block > 0 {
            if let Err(e) = flush(&mut raw, &mut body) {
                cleanup();
                return Err(e);
            }
        }
        body.flush()?;
        drop(body);
        let mut header = PreviewHeader {
            v: VERSION,
            seq_id: seq_id.to_string(),
            frame_ms,
            frame_count: written,
            frame_bytes: frame_bytes as u32,
            props: plan.clone(),
            block_frames: BLOCK_FRAMES,
            blocks: vec![],
            mapping_hash: mapping_hash(seq_hash, seq_frame_ms, props),
        };
        // Offsets depend on the header length: size it with slack, then pad.
        let mut base = 0u64;
        let json = loop {
            header.blocks = blocks
                .iter()
                .map(|&(o, l)| BlockRef {
                    offset: base + o,
                    len: l,
                })
                .collect();
            let j = serde_json::to_vec(&header).map_err(|e| PreviewError::Format(e.to_string()))?;
            let need = 8 + j.len() as u64;
            if base >= need {
                let mut j = j;
                j.resize((base - 8) as usize, b' ');
                break j;
            }
            base = need + 32;
        };
        let mut file = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        file.write_all(MAGIC)?;
        file.write_all(&(json.len() as u32).to_le_bytes())?;
        file.write_all(&json)?;
        let mut b = std::fs::File::open(&body_tmp)?;
        std::io::copy(&mut b, &mut file)?;
        file.flush()?;
        file.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        cleanup();
        std::fs::rename(&tmp, out)?;
        Ok(header)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    progress(1.0);
    res
}

/// Read the header of a preview file.
pub fn read_header(path: &Path) -> Result<PreviewHeader> {
    let mut f = std::fs::File::open(path)?;
    read_header_from(&mut f)
}

pub fn read_header_from(r: &mut impl Read) -> Result<PreviewHeader> {
    let mut m = [0u8; 8];
    r.read_exact(&mut m)?;
    if &m[..4] != MAGIC {
        return Err(PreviewError::Format("bad magic".into()));
    }
    let len = u32::from_le_bytes([m[4], m[5], m[6], m[7]]);
    if len > MAX_HEADER {
        return Err(PreviewError::Format("header too large".into()));
    }
    let mut j = vec![0u8; len as usize];
    r.read_exact(&mut j)?;
    let h: PreviewHeader =
        serde_json::from_slice(&j).map_err(|e| PreviewError::Format(e.to_string()))?;
    if h.v != VERSION {
        return Err(PreviewError::Format(format!("version {}", h.v)));
    }
    Ok(h)
}

/// Decompressed frames of block `n` (tests, tools).
pub fn read_block(path: &Path, header: &PreviewHeader, n: usize) -> Result<Vec<u8>> {
    let b = header
        .blocks
        .get(n)
        .ok_or_else(|| PreviewError::Format(format!("no block {n}")))?;
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::Start(b.offset))?;
    let mut gz = vec![0u8; b.len as usize];
    f.read_exact(&mut gz)?;
    inflate_block(&gz)
}

/// Decompress one gzip block (as fetched by a Range request).
pub fn inflate_block(gz: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(gz).read_to_end(&mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fseq::{FseqWriter, FseqWriterOptions};
    use crate::model::{ChannelRun, MatrixInfo};

    pub(crate) fn prop(id: &str, n: u32, start: u32) -> Prop {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "kind": "line", "pixelCount": n,
            "channelStart": start, "channelsPerPixel": 3, "segments": [], "groupIds": []
        }))
        .unwrap()
    }

    fn tmp() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("pp-pppv-{}", crate::model::new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Channel value of channel `c` in frame `f`.
    fn val(f: u32, c: usize) -> u8 {
        ((f as usize * 7 + c * 13) % 251) as u8
    }

    fn write_fseq(path: &Path, channels: u32, frames: u32, frame_ms: u8) {
        let mut w = FseqWriter::create(path, FseqWriterOptions::new(channels, frame_ms)).unwrap();
        let mut buf = vec![0u8; channels as usize];
        for f in 0..frames {
            for (c, b) in buf.iter_mut().enumerate() {
                *b = val(f, c);
            }
            w.write_frame(&buf).unwrap();
        }
        w.finish().unwrap();
    }

    #[test]
    fn round_trip_matches_the_live_preview_mapping() {
        let d = tmp();
        let fseq = d.join("s.fseq");
        // 25 ms sequence -> 50 ms preview (every 2nd frame).
        write_fseq(&fseq, 3 * 500, 300, 25);
        let mut split = prop("split", 40, 0);
        split.channel_runs = Some(vec![
            ChannelRun {
                prop_offset: 0,
                channel_start: 900,
                pixel_count: 20,
            },
            ChannelRun {
                prop_offset: 20,
                channel_start: 60,
                pixel_count: 20,
            },
        ]);
        let props = vec![
            prop("a", 50, 0),
            split,
            prop("big", 400, 150 * 3),
            prop("off", 10, 3 * 499),
        ];
        let out = d.join("p.pppv");
        let mut calls = 0;
        let h = build(&fseq, "seq1", "abc", &props, &out, &mut |_| {
            calls += 1;
            true
        })
        .unwrap();
        assert!(calls >= 2);
        assert_eq!(h.frame_ms, 50);
        assert_eq!(h.frame_count, 150);
        assert_eq!(h.blocks.len(), 3);
        assert_eq!(read_header(&out).unwrap(), h);
        assert_eq!(h.props[2].n, 300);
        assert!(h.props[0].idx.is_none() && h.props[2].idx.is_some());
        let offs = h.prop_offsets();
        // Full-resolution props equal `mapping::read_prop_channels` of the source frame.
        for block in 0..h.blocks.len() {
            let raw = read_block(&out, &h, block).unwrap();
            let frames = raw.len() / h.frame_bytes as usize;
            for fi in 0..frames {
                let pf = (block * 64 + fi) as u32;
                let src_frame = pf * 2;
                let chan: Vec<u8> = (0..1500).map(|c| val(src_frame, c)).collect();
                let frame = &raw[fi * h.frame_bytes as usize..(fi + 1) * h.frame_bytes as usize];
                for (pi, p) in props.iter().enumerate() {
                    let mut full = vec![0u8; p.pixel_count as usize * 3];
                    crate::mapping::read_prop_channels(p, &chan, &mut full);
                    let got = &frame[offs[pi]..offs[pi] + h.props[pi].n as usize * 3];
                    match &h.props[pi].idx {
                        None => assert_eq!(got, &full[..], "prop {} frame {pf}", p.id),
                        Some(idx) => {
                            for (k, &i) in idx.iter().enumerate() {
                                assert_eq!(
                                    &got[k * 3..k * 3 + 3],
                                    &full[i as usize * 3..i as usize * 3 + 3]
                                );
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(h.frame_at(0), 0);
        assert_eq!(h.frame_at(125), 2);
        assert_eq!(h.frame_at(10_000_000), 149);
        std::fs::remove_dir_all(d).ok();
    }

    /// `web/src/lib/preview/fixtures/sample.pppv` is decoded by the browser
    /// reader's tests (`pppv.test.ts`) and checked against [`val`]: it is
    /// (re)written when missing or when `PP_UPDATE_FIXTURES` is set, and must
    /// always decode to the expected frames here.
    #[test]
    fn web_fixture_round_trip() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/src/lib/preview/fixtures/sample.pppv");
        let props = vec![prop("a", 10, 0), prop("b", 350, 30)];
        if !fixture.exists() || std::env::var_os("PP_UPDATE_FIXTURES").is_some() {
            let d = tmp();
            let fseq = d.join("s.fseq");
            write_fseq(&fseq, 30 + 350 * 3, 150, 25);
            build(
                &fseq,
                "fixture",
                "fixture-hash",
                &props,
                &fixture,
                &mut |_| true,
            )
            .unwrap();
            std::fs::remove_dir_all(d).ok();
        }
        let h = read_header(&fixture).unwrap();
        assert_eq!((h.frame_ms, h.frame_count, h.blocks.len()), (50, 75, 2));
        assert_eq!(h.props[1].n, 300);
        let idx = h.props[1].idx.clone().unwrap();
        for b in 0..2 {
            let raw = read_block(&fixture, &h, b).unwrap();
            for (fi, frame) in raw.chunks(h.frame_bytes as usize).enumerate() {
                let src = (b * 64 + fi) as u32 * 2;
                assert_eq!(frame[0], val(src, 0));
                // Prop b sample k = pixel idx[k] = channel 30 + 3·idx[k].
                let k = 123;
                assert_eq!(frame[30 + k * 3], val(src, 30 + idx[k] as usize * 3));
            }
        }
    }

    #[test]
    fn sampling_caps_and_grids() {
        let mut m = prop("m", 64 * 32, 0);
        m.matrix = Some(MatrixInfo {
            width: 64,
            height: 32,
            pixel_map: (0..64 * 32).collect(),
        });
        let plan = sample_plan(&[m.clone(), prop("s", 100, 0)]);
        let n = plan[0].n;
        assert!((100..=300).contains(&n), "{n}");
        // A grid: every sample is on the same column stride.
        let idx = plan[0].idx.as_ref().unwrap();
        let stride = idx[1] - idx[0];
        assert!(idx.iter().all(|i| (i % 64) % stride == 0));
        assert_eq!(plan[1].n, 100);
        assert!(plan[1].idx.is_none());
        // 40 props x 300 = 12000 > 8192.
        let many: Vec<Prop> = (0..40).map(|i| prop(&format!("p{i}"), 1000, 0)).collect();
        let plan = sample_plan(&many);
        let total: u32 = plan.iter().map(|p| p.n).sum();
        assert!(total <= MAX_TOTAL_SAMPLES && total > 7000, "{total}");
        // Uniform subsample keeps both ends.
        let v = subsample(&prop("x", 1000, 0), 10);
        assert_eq!((v[0], *v.last().unwrap(), v.len()), (0, 999, 10));
        assert_eq!(sample_plan(&[prop("z", 0, 0)])[0].n, 0);
    }

    #[test]
    fn mapping_hash_tracks_what_matters() {
        let a = vec![prop("a", 50, 0)];
        let h = mapping_hash("x", 25, &a);
        assert_eq!(h, mapping_hash("x", 25, &a));
        assert_ne!(h, mapping_hash("y", 25, &a));
        assert_ne!(h, mapping_hash("x", 50, &a));
        assert_ne!(h, mapping_hash("x", 25, &[prop("a", 51, 0)]));
        assert_ne!(h, mapping_hash("x", 25, &[prop("a", 50, 3)]));
        let mut renamed = prop("a", 50, 0);
        renamed.name = "Other".into();
        assert_eq!(h, mapping_hash("x", 25, &[renamed]), "names don't matter");
        assert_eq!(h.len(), 16);
    }

    #[test]
    fn rejects_garbage_and_cancels() {
        let d = tmp();
        std::fs::write(d.join("bad.pppv"), b"NOPE0000").unwrap();
        assert!(read_header(&d.join("bad.pppv")).is_err());
        let fseq = d.join("s.fseq");
        write_fseq(&fseq, 30, 400, 50);
        let out = d.join("c.pppv");
        let r = build(&fseq, "s", "h", &[prop("a", 10, 0)], &out, &mut |p| p < 0.5);
        assert!(matches!(r, Err(PreviewError::Cancelled)));
        assert!(!out.exists());
        let leftovers = std::fs::read_dir(&d).unwrap().count();
        assert_eq!(leftovers, 2, "no temp files left");
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn preview_frame_rate() {
        assert_eq!(preview_frame_ms(25), 50);
        assert_eq!(preview_frame_ms(50), 50);
        assert_eq!(preview_frame_ms(20), 60);
        assert_eq!(preview_frame_ms(100), 100);
        assert_eq!(preview_frame_ms(0), 50);
    }
}
