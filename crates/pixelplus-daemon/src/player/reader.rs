//! Sequence frame reading on a background thread with read-ahead, so an SD
//! card hiccup never stalls the output thread.
//!
//! The reader opens the file itself (opening can block too), then keeps the
//! next few hundred milliseconds of frames decoded in memory. The output
//! thread asks for frame *n* with [`FrameReader::get`]; if the frame is not
//! there yet it keeps showing the previous one, and a request outside the
//! read-ahead window (seek, the output fell behind) restarts reading there.

use parking_lot::{Condvar, Mutex};
use pixelplus_core::fseq::FseqFile;
use pixelplus_core::ppseq::PpseqFile;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Frame layout of a sequence file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameLayout {
    /// `.fseq`: absolute channel space.
    Channels,
    /// `.ppseq`: this node's output-major layout with these pixels per output.
    Outputs(Vec<u32>),
}

/// Header facts, known once the file is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeqMeta {
    pub frame_count: u32,
    pub frame_ms: u32,
    pub frame_len: usize,
    pub layout: FrameLayout,
}

impl SeqMeta {
    pub fn duration_ms(&self) -> u64 {
        self.frame_count as u64 * self.frame_ms as u64
    }
}

enum SeqFile {
    Fseq(FseqFile),
    Ppseq(PpseqFile),
}

impl SeqFile {
    fn open(path: &Path) -> Result<SeqFile, String> {
        let is_ppseq = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("ppseq"));
        if is_ppseq {
            PpseqFile::open(path).map(SeqFile::Ppseq).map_err(|e| e.to_string())
        } else {
            FseqFile::open(path).map(SeqFile::Fseq).map_err(|e| e.to_string())
        }
    }

    fn meta(&self) -> SeqMeta {
        match self {
            SeqFile::Fseq(f) => SeqMeta {
                frame_count: f.frame_count(),
                frame_ms: f.frame_ms().max(1),
                frame_len: f.frame_size(),
                layout: FrameLayout::Channels,
            },
            SeqFile::Ppseq(f) => SeqMeta {
                frame_count: f.frame_count(),
                frame_ms: f.frame_ms().max(1),
                frame_len: f.frame_bytes(),
                layout: FrameLayout::Outputs(f.pixels_per_output().to_vec()),
            },
        }
    }

    fn read(&mut self, idx: u32, buf: &mut [u8]) -> Result<(), String> {
        match self {
            SeqFile::Fseq(f) => f.frame(idx, buf).map_err(|e| e.to_string()),
            SeqFile::Ppseq(f) => f.frame(idx, buf).map_err(|e| e.to_string()),
        }
    }
}

#[derive(Default)]
struct State {
    meta: Option<SeqMeta>,
    error: Option<String>,
    /// Consecutive frames `(idx, bytes)`, ascending.
    cache: VecDeque<(u32, Vec<u8>)>,
    /// Next frame the reader thread will read.
    next: u32,
    pool: Vec<Vec<u8>>,
    lookahead: usize,
    stop: bool,
}

struct Shared {
    state: Mutex<State>,
    cond: Condvar,
}

/// Background reader of one sequence file.
pub struct FrameReader {
    shared: Arc<Shared>,
    path: PathBuf,
}

/// Memory budget for read-ahead frames.
const READAHEAD_BYTES: usize = 8 * 1024 * 1024;

impl FrameReader {
    /// Open `path` on a new thread and start reading at `start_frame`.
    pub fn open(path: PathBuf, start_frame: u32) -> FrameReader {
        let shared = Arc::new(Shared {
            state: Mutex::new(State { next: start_frame, ..Default::default() }),
            cond: Condvar::new(),
        });
        let s2 = shared.clone();
        let p2 = path.clone();
        let spawned = std::thread::Builder::new()
            .name("pp-seqread".into())
            .spawn(move || reader_thread(s2, p2));
        if let Err(e) = spawned {
            shared.state.lock().error = Some(format!("could not start the frame reader: {e}"));
        }
        FrameReader { shared, path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Header facts (None while still opening).
    pub fn meta(&self) -> Option<SeqMeta> {
        self.shared.state.lock().meta.clone()
    }

    /// Open/read error, if any (the item should be skipped).
    pub fn error(&self) -> Option<String> {
        self.shared.state.lock().error.clone()
    }

    /// Is frame `idx` available right now?
    pub fn ready(&self, idx: u32) -> bool {
        let st = self.shared.state.lock();
        st.cache.iter().any(|(i, _)| *i == idx)
    }

    /// Wait up to `timeout` until frame `idx` is available (tests, preloading).
    pub fn wait_ready(&self, idx: u32, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        let mut st = self.shared.state.lock();
        loop {
            if st.cache.iter().any(|(i, _)| *i == idx) {
                return true;
            }
            if st.error.is_some() {
                return false;
            }
            if self.shared.cond.wait_until(&mut st, deadline).timed_out() {
                return st.cache.iter().any(|(i, _)| *i == idx);
            }
        }
    }

    /// Copy frame `idx` into `out` (up to `out.len()` bytes). Returns false if the
    /// frame is not read yet (the caller keeps showing its previous frame).
    pub fn get(&self, idx: u32, out: &mut [u8]) -> bool {
        let mut st = self.shared.state.lock();
        let Some(meta) = st.meta.clone() else { return false };
        let idx = idx.min(meta.frame_count.saturating_sub(1));
        // Drop frames we are past.
        let mut dropped = false;
        while st.cache.front().is_some_and(|(i, _)| *i < idx) {
            if let Some((_, buf)) = st.cache.pop_front() {
                st.pool.push(buf);
                dropped = true;
            }
        }
        if let Some((i, buf)) = st.cache.front() {
            if *i == idx {
                let n = out.len().min(buf.len());
                out[..n].copy_from_slice(&buf[..n]);
                out[n..].fill(0);
                drop(st);
                if dropped {
                    self.shared.cond.notify_all();
                }
                return true;
            }
        }
        // Not cached: if the reader is not about to produce it, restart there.
        let in_flight = st.cache.is_empty() && st.next == idx;
        if !in_flight {
            let stale: Vec<_> = st.cache.drain(..).map(|(_, b)| b).collect();
            st.pool.extend(stale);
            st.next = idx;
        }
        drop(st);
        self.shared.cond.notify_all();
        false
    }
}

impl Drop for FrameReader {
    fn drop(&mut self) {
        self.shared.state.lock().stop = true;
        self.shared.cond.notify_all();
    }
}

fn reader_thread(shared: Arc<Shared>, path: PathBuf) {
    let mut file = match SeqFile::open(&path) {
        Ok(f) => f,
        Err(e) => {
            shared.state.lock().error = Some(e);
            shared.cond.notify_all();
            return;
        }
    };
    let meta = file.meta();
    {
        let mut st = shared.state.lock();
        st.lookahead = (READAHEAD_BYTES / meta.frame_len.max(1)).clamp(4, 64);
        st.meta = Some(meta.clone());
    }
    shared.cond.notify_all();
    let mut failures = 0u32;
    loop {
        // Decide what to read.
        let (idx, mut buf) = {
            let mut st = shared.state.lock();
            loop {
                if st.stop {
                    return;
                }
                if st.cache.len() < st.lookahead && st.next < meta.frame_count {
                    break;
                }
                shared.cond.wait_for(&mut st, Duration::from_millis(200));
            }
            let buf = st.pool.pop().unwrap_or_default();
            (st.next, buf)
        };
        buf.resize(meta.frame_len, 0);
        let result = file.read(idx, &mut buf);
        let mut st = shared.state.lock();
        match result {
            Ok(()) => {
                failures = 0;
                // Only keep it if nobody restarted the reader meanwhile.
                if st.next == idx {
                    st.cache.push_back((idx, buf));
                    st.next = idx + 1;
                } else {
                    st.pool.push(buf);
                }
            }
            Err(e) => {
                failures += 1;
                tracing::warn!("reading frame {idx} of {}: {e}", path.display());
                if failures >= 3 {
                    st.error = Some(format!("the sequence file is damaged ({e})"));
                    drop(st);
                    shared.cond.notify_all();
                    return;
                }
                // Skip the bad frame; the output holds the previous one.
                if st.next == idx {
                    st.next = idx + 1;
                }
                st.pool.push(buf);
            }
        }
        drop(st);
        shared.cond.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixelplus_core::fseq::{FseqWriter, FseqWriterOptions};

    pub(crate) fn write_fseq(path: &Path, frames: u32, channels: u32, frame_ms: u8) {
        let mut w = FseqWriter::create(path, FseqWriterOptions::new(channels, frame_ms)).unwrap();
        for f in 0..frames {
            let frame: Vec<u8> = (0..channels).map(|c| (f as u8).wrapping_add(c as u8)).collect();
            w.write_frame(&frame).unwrap();
        }
        w.finish().unwrap();
    }

    #[test]
    fn reads_ahead_and_seeks() {
        let dir = std::env::temp_dir().join(format!("pp-reader-{}", pixelplus_core::model::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.fseq");
        write_fseq(&path, 200, 9, 25);
        let r = FrameReader::open(path, 0);
        assert!(r.wait_ready(0, Duration::from_secs(5)));
        let meta = r.meta().unwrap();
        assert_eq!(meta.frame_count, 200);
        assert_eq!(meta.frame_ms, 25);
        assert_eq!(meta.duration_ms(), 5000);
        let mut buf = vec![0u8; 9];
        assert!(r.get(0, &mut buf));
        assert_eq!(buf[0], 0);
        assert!(r.wait_ready(3, Duration::from_secs(5)));
        assert!(r.get(3, &mut buf));
        assert_eq!(buf[0], 3);
        assert_eq!(buf[2], 5);
        // Seek far ahead: first miss, then available.
        assert!(!r.get(150, &mut buf) || buf[0] == 150);
        assert!(r.wait_ready(150, Duration::from_secs(5)));
        assert!(r.get(150, &mut buf));
        assert_eq!(buf[0], 150);
        // Seek back.
        r.get(10, &mut buf);
        assert!(r.wait_ready(10, Duration::from_secs(5)));
        assert!(r.get(10, &mut buf));
        assert_eq!(buf[0], 10);
        // Beyond the end clamps to the last frame.
        r.get(199, &mut buf);
        assert!(r.wait_ready(199, Duration::from_secs(5)));
        assert!(r.get(5000, &mut buf));
        assert_eq!(buf[0], 199);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn missing_and_corrupt_files_report_errors() {
        let dir = std::env::temp_dir().join(format!("pp-reader-{}", pixelplus_core::model::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let r = FrameReader::open(dir.join("missing.fseq"), 0);
        assert!(!r.wait_ready(0, Duration::from_secs(5)));
        assert!(r.error().is_some());
        let bad = dir.join("bad.fseq");
        std::fs::write(&bad, b"this is not an fseq file at all").unwrap();
        let r = FrameReader::open(bad, 0);
        assert!(!r.wait_ready(0, Duration::from_secs(5)));
        assert!(r.error().is_some());
        std::fs::remove_dir_all(dir).ok();
    }
}
