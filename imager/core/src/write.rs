//! Streaming image writer: decompress (.img.xz) -> write to the target -> verify by
//! reading back -> inject pixelplus.txt. Progress is reported through a callback.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::disk::SECTOR;

pub const CHUNK: usize = 4 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Prepare,
    Write,
    Verify,
    Customize,
    Done,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub phase: Phase,
    pub bytes: u64,
    /// Total bytes of this phase if known (extracted image size).
    pub total: Option<u64>,
    pub message: Option<String>,
}

impl Progress {
    pub fn new(phase: Phase, bytes: u64, total: Option<u64>) -> Self {
        Progress { phase, bytes, total, message: None }
    }
    pub fn msg(phase: Phase, m: impl Into<String>) -> Self {
        Progress { phase, bytes: 0, total: None, message: Some(m.into()) }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("the image is corrupt: {0}")]
    Decompress(String),
    #[error("the downloaded image does not match its checksum (expected {expected}, got {actual})")]
    ImageChecksum { expected: String, actual: String },
    #[error("verification failed: the card returned different data at around {offset} bytes. The card may be faulty or counterfeit.")]
    Verify { offset: u64 },
    #[error("the image ({image} bytes) is larger than the card ({device} bytes)")]
    TooSmall { image: u64, device: u64 },
    #[error("cancelled")]
    Cancelled,
}

/// What kind of image file this is (by magic bytes, not by name).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None,
    Xz,
}

pub fn detect_compression(path: &Path) -> io::Result<Compression> {
    let mut f = File::open(path)?;
    let mut magic = [0u8; 6];
    let n = f.read(&mut magic)?;
    if n == 6 && magic == [0xFD, b'7', b'z', b'X', b'Z', 0x00] {
        Ok(Compression::Xz)
    } else {
        Ok(Compression::None)
    }
}

/// Sink that forwards decompressed bytes to the target in CHUNK-sized, sector-aligned
/// writes, hashing them and reporting progress.
struct DeviceSink<'a, W: Write> {
    dev: &'a mut W,
    buf: Vec<u8>,
    written: u64,
    hash: Sha256,
    total: Option<u64>,
    limit: Option<u64>,
    progress: &'a mut dyn FnMut(Progress),
    cancel: &'a AtomicBool,
}

impl<W: Write> DeviceSink<'_, W> {
    fn flush_buf(&mut self, final_: bool) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        if self.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        if let Some(limit) = self.limit {
            if self.written + self.buf.len() as u64 > limit {
                return Err(io::Error::new(io::ErrorKind::WriteZero, "image larger than device"));
            }
        }
        self.hash.update(&self.buf);
        let len = self.buf.len();
        if final_ && len as u64 % SECTOR != 0 {
            // raw devices need whole sectors; pad the tail with zeros (not hashed)
            let pad = (SECTOR - len as u64 % SECTOR) as usize;
            self.buf.resize(len + pad, 0);
        }
        self.dev.write_all(&self.buf)?;
        self.written += len as u64;
        self.buf.clear();
        (self.progress)(Progress::new(Phase::Write, self.written, self.total));
        Ok(())
    }
}

impl<W: Write> Write for DeviceSink<'_, W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let mut rest = data;
        while !rest.is_empty() {
            let room = CHUNK - self.buf.len();
            let n = room.min(rest.len());
            self.buf.extend_from_slice(&rest[..n]);
            rest = &rest[n..];
            if self.buf.len() == CHUNK {
                self.flush_buf(false)?;
            }
        }
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct WriteOutcome {
    pub bytes: u64,
    pub sha256: String,
}

/// Write `image` (raw or .xz) to `dev` from offset 0. `device_size` guards against
/// images larger than the card. `extract_sha256` (from the release metadata) is checked
/// after writing, before verification.
pub fn write_image<W: Write + Seek>(
    image: &Path,
    dev: &mut W,
    device_size: Option<u64>,
    extract_size: Option<u64>,
    extract_sha256: Option<&str>,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<WriteOutcome, WriteError> {
    let comp = detect_compression(image)?;
    let file = File::open(image)?;
    let raw_len = file.metadata()?.len();
    let total = match comp {
        Compression::None => Some(raw_len),
        Compression::Xz => extract_size,
    };
    if let (Some(t), Some(d)) = (total, device_size) {
        if t > d {
            return Err(WriteError::TooSmall { image: t, device: d });
        }
    }
    dev.seek(SeekFrom::Start(0))?;
    let mut sink = DeviceSink {
        dev,
        buf: Vec::with_capacity(CHUNK),
        written: 0,
        hash: Sha256::new(),
        total,
        limit: device_size,
        progress,
        cancel,
    };
    let mut reader = BufReader::with_capacity(1 << 20, file);
    let res: io::Result<()> = match comp {
        Compression::None => io::copy(&mut reader, &mut sink).map(|_| ()),
        Compression::Xz => lzma_rs::xz_decompress(&mut reader, &mut sink).map_err(|e| match e {
            lzma_rs::error::Error::IoError(io) => io,
            other => io::Error::new(io::ErrorKind::InvalidData, format!("{other:?}")),
        }),
    };
    match res {
        Ok(()) => {}
        Err(e) if cancel.load(Ordering::Relaxed) => {
            let _ = e;
            return Err(WriteError::Cancelled);
        }
        Err(e) if e.kind() == io::ErrorKind::InvalidData => return Err(WriteError::Decompress(e.to_string())),
        Err(e) if e.kind() == io::ErrorKind::WriteZero && device_size.is_some() => {
            return Err(WriteError::TooSmall { image: sink.written, device: device_size.unwrap_or(0) })
        }
        Err(e) => return Err(e.into()),
    }
    sink.flush_buf(true)?;
    let bytes = sink.written;
    let sha = hex(&sink.hash.finalize());
    sink.dev.flush()?;
    if let Some(exp) = extract_sha256 {
        if !exp.eq_ignore_ascii_case(&sha) {
            return Err(WriteError::ImageChecksum { expected: exp.to_string(), actual: sha });
        }
    }
    Ok(WriteOutcome { bytes, sha256: sha })
}

/// Read `len` bytes back from `dev` and compare their SHA-256 with `sha256`.
pub fn verify<R: Read + Seek>(
    dev: &mut R,
    len: u64,
    sha256: &str,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<(), WriteError> {
    dev.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    while done < len {
        if cancel.load(Ordering::Relaxed) {
            return Err(WriteError::Cancelled);
        }
        let want = (len - done).min(CHUNK as u64) as usize;
        // read whole sectors, hash only the image bytes
        let aligned = want.div_ceil(SECTOR as usize) * SECTOR as usize;
        let mut got = 0;
        while got < aligned {
            match dev.read(&mut buf[got..aligned])? {
                0 => break,
                n => got += n,
            }
        }
        if got < want {
            return Err(WriteError::Verify { offset: done + got as u64 });
        }
        hash.update(&buf[..want]);
        done += want as u64;
        progress(Progress::new(Phase::Verify, done, Some(len)));
    }
    let actual = hex(&hash.finalize());
    if actual.eq_ignore_ascii_case(sha256) {
        Ok(())
    } else {
        Err(WriteError::Verify { offset: 0 })
    }
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// SHA-256 of a file (used to check downloads against `image_download_sha256`).
pub fn sha256_file(path: &Path, progress: &mut dyn FnMut(u64)) -> io::Result<String> {
    let mut f = BufReader::with_capacity(1 << 20, File::open(path)?);
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut n_total = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        n_total += n as u64;
        progress(n_total);
    }
    Ok(hex(&h.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn xz_of(data: &[u8]) -> Vec<u8> {
        // Prefer the real xz tool (multi-threaded, multi-block output like pi-gen's);
        // fall back to lzma-rs' encoder.
        if let Ok(mut child) = std::process::Command::new("xz")
            .args(["-T2", "--block-size=1MiB", "-c", "-1"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
        {
            let mut stdin = child.stdin.take().unwrap();
            let owned = data.to_vec();
            let t = std::thread::spawn(move || stdin.write_all(&owned));
            let out = child.wait_with_output().unwrap();
            t.join().unwrap().unwrap();
            if out.status.success() {
                return out.stdout;
            }
        }
        let mut out = Vec::new();
        lzma_rs::xz_compress(&mut Cursor::new(data), &mut out).unwrap();
        out
    }

    fn sample(len: usize) -> Vec<u8> {
        (0..len).map(|i| ((i * 7 + i / 4093) % 251) as u8).collect()
    }

    fn run(data: &[u8], compressed: bool, dev_size: Option<u64>) -> (Result<WriteOutcome, WriteError>, Vec<u8>, Vec<Progress>) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(if compressed { "x.img.xz" } else { "x.img" });
        std::fs::write(&p, if compressed { xz_of(data) } else { data.to_vec() }).unwrap();
        let mut dev = Cursor::new(vec![0u8; dev_size.unwrap_or(data.len() as u64 + 8192) as usize]);
        let mut events = Vec::new();
        let cancel = AtomicBool::new(false);
        let r = write_image(&p, &mut dev, dev_size, Some(data.len() as u64), None, &mut |e| events.push(e), &cancel);
        (r, dev.into_inner(), events)
    }

    #[test]
    fn raw_and_xz_roundtrip_with_verify() {
        let data = sample(9 * 1024 * 1024 + 512);
        for compressed in [false, true] {
            let (r, dev, events) = run(&data, compressed, None);
            let out = r.unwrap();
            assert_eq!(out.bytes, data.len() as u64);
            assert_eq!(&dev[..data.len()], &data[..]);
            assert_eq!(events.last().unwrap().bytes, data.len() as u64);
            let cancel = AtomicBool::new(false);
            verify(&mut Cursor::new(&dev), out.bytes, &out.sha256, &mut |_| {}, &cancel).unwrap();
        }
    }

    #[test]
    fn unaligned_tail_is_padded() {
        let data = sample(CHUNK + 100);
        let (r, dev, _) = run(&data, false, None);
        let out = r.unwrap();
        assert_eq!(out.bytes, data.len() as u64);
        assert_eq!(&dev[..data.len()], &data[..]);
        assert!(dev[data.len()..data.len() + 412].iter().all(|b| *b == 0));
        let cancel = AtomicBool::new(false);
        verify(&mut Cursor::new(&dev), out.bytes, &out.sha256, &mut |_| {}, &cancel).unwrap();
    }

    #[test]
    fn detects_bad_card() {
        let data = sample(2 * 1024 * 1024);
        let (r, mut dev, _) = run(&data, false, None);
        let out = r.unwrap();
        dev[1_000_000] ^= 0xFF;
        let cancel = AtomicBool::new(false);
        let e = verify(&mut Cursor::new(&dev), out.bytes, &out.sha256, &mut |_| {}, &cancel).unwrap_err();
        assert!(matches!(e, WriteError::Verify { .. }));
    }

    #[test]
    fn too_small_device() {
        let data = sample(3 * 1024 * 1024);
        let (r, _, _) = run(&data, false, Some(1024 * 1024));
        assert!(matches!(r, Err(WriteError::TooSmall { .. })));
        // compressed: size unknown up front (no extract_size) -> caught while writing
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.img.xz");
        std::fs::write(&p, xz_of(&data)).unwrap();
        let mut dev = Cursor::new(vec![0u8; 1024 * 1024]);
        let cancel = AtomicBool::new(false);
        let r = write_image(&p, &mut dev, Some(1024 * 1024), None, None, &mut |_| {}, &cancel);
        assert!(matches!(r, Err(WriteError::TooSmall { .. })), "{:?}", r.err());
    }

    #[test]
    fn checksum_mismatch() {
        let data = sample(1024 * 1024);
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.img");
        std::fs::write(&p, &data).unwrap();
        let mut dev = Cursor::new(vec![0u8; data.len()]);
        let cancel = AtomicBool::new(false);
        let r = write_image(&p, &mut dev, None, None, Some(&"0".repeat(64)), &mut |_| {}, &cancel);
        assert!(matches!(r, Err(WriteError::ImageChecksum { .. })));
    }

    #[test]
    fn cancel() {
        let data = sample(9 * 1024 * 1024);
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.img");
        std::fs::write(&p, &data).unwrap();
        let mut dev = Cursor::new(vec![0u8; data.len()]);
        let cancel = AtomicBool::new(false);
        let r = write_image(&p, &mut dev, None, None, None, &mut |_| cancel.store(true, Ordering::Relaxed), &cancel);
        assert!(matches!(r, Err(WriteError::Cancelled)));
    }

    #[test]
    fn corrupt_xz() {
        let data = sample(2 * 1024 * 1024);
        let mut xz = xz_of(&data);
        let n = xz.len();
        xz[n / 2] ^= 0x55;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.img.xz");
        std::fs::write(&p, xz).unwrap();
        let mut dev = Cursor::new(vec![0u8; data.len() + 4096]);
        let cancel = AtomicBool::new(false);
        let r = write_image(&p, &mut dev, None, None, None, &mut |_| {}, &cancel);
        assert!(r.is_err());
    }
}
