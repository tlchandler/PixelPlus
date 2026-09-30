//! Controller transfer file (F10, ARCHITECTURE §12.9): everything that makes
//! a controller *this* show leader, in one passphrase-encrypted `.ppxfer`
//! file, so a dead leader can be replaced by a freshly flashed Pi.
//!
//! **Contents** (a zstd-compressed tar inside the encryption):
//! `transfer.json` (what and from where), `show.json` (complete, secrets
//! included), `node.json` (identity: id, name; the cluster keys make the
//! followers trust the new hardware), `cluster/keys.json` (one key per
//! follower), `tls/ca.{key,crt,json}` (the local CA of F1, so phones keep
//! trusting the replacement) and every sequence, audio file and thumbnail the
//! show references. Tunnel tokens are *not* included (they belong to the old
//! hardware's tailnet / cloudflared service and are set up again).
//!
//! **Container** (`.ppxfer`, version 1):
//!
//! ```text
//! "PPXFER\0\x01" | u32 BE header length | header JSON
//! record*: u32 BE length | AES-256-GCM ciphertext (+16-byte tag)
//! ```
//!
//! * key = Argon2id(passphrase, salt, m/t/p from the header), 32 bytes;
//!   the passphrase has at least [`MIN_PASSPHRASE`] characters.
//! * record `i` carries up to 1 MiB of plaintext; its nonce is the header's
//!   7-byte prefix ‖ `i` (u32 BE) ‖ a final-record flag (0/1), so records
//!   can't be reordered, dropped or appended, and a truncated file is
//!   detected (the last record must carry the flag) — the STREAM
//!   construction.
//! * every record authenticates the magic and the whole header as associated
//!   data, so the KDF parameters and the metadata can't be altered either.
//! * the header's `check` is the tag of an empty message under a reserved
//!   nonce: a wrong passphrase is told apart from a damaged file up front.

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

/// File magic (8 bytes, includes the container version).
pub const MAGIC: &[u8; 8] = b"PPXFER\0\x01";
/// Shortest accepted passphrase.
pub const MIN_PASSPHRASE: usize = 10;
/// Plaintext bytes per record.
pub const CHUNK: usize = 1 << 20;
/// Longest accepted header.
const MAX_HEADER: u32 = 64 * 1024;
/// Argon2id cost when writing (64 MiB, 3 passes: about a second on a Pi 4,
/// fits a 512 MB Zero 2 W).
/// (Debug-build tests use a cheap cost: unoptimized Argon2 is very slow.)
const KDF_M_KIB: u32 = if cfg!(test) { 256 } else { 64 * 1024 };
const KDF_T: u32 = if cfg!(test) { 1 } else { 3 };
const KDF_P: u32 = 1;
/// Refuse files asking for more (a crafted header must not exhaust memory).
const KDF_MAX_M_KIB: u32 = 256 * 1024;
const KDF_MAX_T: u32 = 10;
const CHECK_COUNTER: u32 = u32::MAX;

/// Key derivation parameters (stored in the header).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Kdf {
    /// Always `argon2id`.
    pub alg: String,
    /// Memory in KiB.
    pub m: u32,
    pub t: u32,
    pub p: u32,
    /// 16 random bytes, hex.
    pub salt: String,
}

/// Unencrypted (but authenticated) file header.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Header {
    pub v: u32,
    pub kdf: Kdf,
    /// Plaintext bytes per record.
    pub chunk: u32,
    /// 7-byte nonce prefix, hex.
    pub nonce: String,
    /// Tag of an empty message under the reserved nonce (passphrase check), hex.
    pub check: String,
    /// RFC 3339.
    pub created_at: String,
    #[serde(default)]
    pub show_name: String,
    /// Node id of the leader it was made on.
    #[serde(default)]
    pub leader_id: String,
    #[serde(default)]
    pub hostname: String,
    /// PixelPlus version that wrote it.
    #[serde(default)]
    pub version: String,
}

/// What the header says about the file (shown before restoring).
#[derive(Debug, Clone, Default)]
pub struct Meta {
    pub show_name: String,
    pub leader_id: String,
    pub hostname: String,
    pub version: String,
}

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// The passphrase is wrong (as opposed to a damaged file).
pub fn wrong_passphrase(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::PermissionDenied
}

/// Marker for "the show doesn't fit on this SD card" (see [`no_space`]).
#[derive(Debug)]
struct NoSpace;

impl std::fmt::Display for NoSpace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("There isn't enough free space on this controller for that show.")
    }
}

impl std::error::Error for NoSpace {}

/// The unpacked show would not fit.
pub fn no_space(e: &io::Error) -> bool {
    e.get_ref().is_some_and(|x| x.is::<NoSpace>())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Check a passphrase before using it.
pub fn validate_passphrase(p: &str) -> Result<(), String> {
    if p.chars().count() < MIN_PASSPHRASE {
        return Err(format!(
            "Use a passphrase of at least {MIN_PASSPHRASE} characters."
        ));
    }
    if p.len() > 1024 {
        return Err("That passphrase is too long.".into());
    }
    Ok(())
}

/// Derive the file key from a passphrase.
pub fn derive_key(passphrase: &str, kdf: &Kdf) -> io::Result<[u8; 32]> {
    use argon2::{Algorithm, Argon2, Params, Version};
    if kdf.alg != "argon2id"
        || kdf.m > KDF_MAX_M_KIB
        || kdf.t > KDF_MAX_T
        || kdf.p == 0
        || kdf.p > 4
    {
        return Err(bad("unsupported key derivation in the transfer file"));
    }
    let salt = unhex(&kdf.salt)
        .filter(|s| s.len() >= 16)
        .ok_or_else(|| bad("bad salt"))?;
    let params = Params::new(kdf.m, kdf.t, kdf.p, Some(32)).map_err(|e| bad(e.to_string()))?;
    let mut key = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), &salt, &mut key)
        .map_err(|e| bad(e.to_string()))?;
    Ok(key)
}

fn nonce(prefix: &[u8; 7], counter: u32, last: bool) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[..7].copy_from_slice(prefix);
    n[7..11].copy_from_slice(&counter.to_be_bytes());
    n[11] = if last { 1 } else { 2 };
    n
}

struct Sealer {
    cipher: aes_gcm::Aes256Gcm,
    prefix: [u8; 7],
    aad: Vec<u8>,
}

impl Sealer {
    fn new(key: &[u8; 32], prefix: [u8; 7], aad: Vec<u8>) -> Self {
        use aes_gcm::KeyInit;
        Sealer {
            cipher: aes_gcm::Aes256Gcm::new(key.into()),
            prefix,
            aad,
        }
    }

    fn seal(&self, counter: u32, last: bool, msg: &[u8]) -> io::Result<Vec<u8>> {
        use aes_gcm::aead::{Aead, Payload};
        let n = nonce(&self.prefix, counter, last);
        self.cipher
            .encrypt(
                (&n).into(),
                Payload {
                    msg,
                    aad: &self.aad,
                },
            )
            .map_err(|_| bad("encryption failed"))
    }

    fn open(&self, counter: u32, last: bool, ct: &[u8]) -> Option<Vec<u8>> {
        use aes_gcm::aead::{Aead, Payload};
        let n = nonce(&self.prefix, counter, last);
        self.cipher
            .decrypt(
                (&n).into(),
                Payload {
                    msg: ct,
                    aad: &self.aad,
                },
            )
            .ok()
    }
}

fn aad_for(header_json: &[u8]) -> Vec<u8> {
    let mut aad = Vec::with_capacity(12 + header_json.len());
    aad.extend_from_slice(MAGIC);
    aad.extend_from_slice(&(header_json.len() as u32).to_be_bytes());
    aad.extend_from_slice(header_json);
    aad
}

/// Encrypting writer: plaintext in, `.ppxfer` out. Call [`Encryptor::finish`].
pub struct Encryptor<W: Write> {
    out: W,
    sealer: Sealer,
    counter: u32,
    buf: Vec<u8>,
    chunk: usize,
}

impl<W: Write> Encryptor<W> {
    /// Write the header and return the writer. `meta` fills the header's
    /// informational fields.
    pub fn new(out: W, passphrase: &str, meta: &Meta) -> io::Result<Self> {
        Self::with_params(out, passphrase, meta, KDF_M_KIB, KDF_T, CHUNK)
    }

    /// [`Encryptor::new`] with explicit cost and record size (tests).
    pub fn with_params(
        mut out: W,
        passphrase: &str,
        meta: &Meta,
        m_kib: u32,
        t: u32,
        chunk: usize,
    ) -> io::Result<Self> {
        use rand::RngCore;
        let mut rng = rand::rngs::OsRng;
        let mut salt = [0u8; 16];
        let mut prefix = [0u8; 7];
        rng.fill_bytes(&mut salt);
        rng.fill_bytes(&mut prefix);
        let kdf = Kdf {
            alg: "argon2id".into(),
            m: m_kib,
            t,
            p: KDF_P,
            salt: hex(&salt),
        };
        let key = derive_key(passphrase, &kdf)?;
        // The passphrase check does not depend on the header (it is part of it).
        let check = Sealer::new(&key, prefix, Vec::new()).seal(CHECK_COUNTER, true, b"")?;
        let header = Header {
            v: 1,
            kdf,
            chunk: chunk as u32,
            nonce: hex(&prefix),
            check: hex(&check),
            created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            show_name: meta.show_name.clone(),
            leader_id: meta.leader_id.clone(),
            hostname: meta.hostname.clone(),
            version: meta.version.clone(),
        };
        let json = serde_json::to_vec(&header).map_err(|e| bad(e.to_string()))?;
        out.write_all(MAGIC)?;
        out.write_all(&(json.len() as u32).to_be_bytes())?;
        out.write_all(&json)?;
        Ok(Encryptor {
            out,
            sealer: Sealer::new(&key, prefix, aad_for(&json)),
            counter: 0,
            buf: Vec::with_capacity(chunk),
            chunk,
        })
    }

    fn emit(&mut self, last: bool) -> io::Result<()> {
        let n = if last { self.buf.len() } else { self.chunk };
        let ct = self.sealer.seal(self.counter, last, &self.buf[..n])?;
        self.out.write_all(&(ct.len() as u32).to_be_bytes())?;
        self.out.write_all(&ct)?;
        self.buf.drain(..n);
        self.counter = self
            .counter
            .checked_add(1)
            .filter(|c| *c != CHECK_COUNTER)
            .ok_or_else(|| bad("transfer file too large"))?;
        Ok(())
    }

    /// Seal the last record and return the inner writer.
    pub fn finish(mut self) -> io::Result<W> {
        while self.buf.len() > self.chunk {
            self.emit(false)?;
        }
        self.emit(true)?;
        self.out.flush()?;
        Ok(self.out)
    }
}

impl<W: Write> Write for Encryptor<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        // Keep at least one byte back: the final record must not be empty
        // just because the data happened to fill whole records.
        while self.buf.len() > self.chunk {
            self.emit(false)?;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

/// Read and check a `.ppxfer` header (no passphrase needed).
pub fn read_header<R: Read>(r: &mut R) -> io::Result<(Header, Vec<u8>)> {
    let mut magic = [0u8; 8];
    r.read_exact(&mut magic)
        .map_err(|_| bad("That isn't a PixelPlus transfer file."))?;
    if &magic != MAGIC {
        if &magic[..6] == b"PPXFER" {
            return Err(bad(
                "This transfer file was made by a newer PixelPlus. Update this controller first.",
            ));
        }
        return Err(bad("That isn't a PixelPlus transfer file."));
    }
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len);
    if len == 0 || len > MAX_HEADER {
        return Err(bad("The transfer file is damaged (header)."));
    }
    let mut json = vec![0u8; len as usize];
    r.read_exact(&mut json)?;
    let header: Header =
        serde_json::from_slice(&json).map_err(|_| bad("The transfer file is damaged (header)."))?;
    if header.v != 1 {
        return Err(bad(
            "This transfer file was made by a newer PixelPlus. Update this controller first.",
        ));
    }
    Ok((header, json))
}

/// Decrypting reader over a `.ppxfer` stream. Every byte it returns has been
/// authenticated; a damaged, tampered or truncated file is an error
/// ([`io::ErrorKind::InvalidData`]) at the point it is detected, a wrong
/// passphrase fails in [`Decryptor::new`] ([`wrong_passphrase`]).
pub struct Decryptor<R: Read> {
    inner: R,
    sealer: Sealer,
    header: Header,
    counter: u32,
    out: Vec<u8>,
    pos: usize,
    done: bool,
}

impl<R: Read> Decryptor<R> {
    pub fn new(mut inner: R, passphrase: &str) -> io::Result<Self> {
        let (header, json) = read_header(&mut inner)?;
        let key = derive_key(passphrase, &header.kdf)?;
        let prefix: [u8; 7] = unhex(&header.nonce)
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| bad("The transfer file is damaged (header)."))?;
        let check = unhex(&header.check).unwrap_or_default();
        if Sealer::new(&key, prefix, Vec::new())
            .open(CHECK_COUNTER, true, &check)
            .is_none()
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "That passphrase doesn't open this transfer file.",
            ));
        }
        if header.chunk == 0 || header.chunk as usize > 16 * CHUNK {
            return Err(bad("The transfer file is damaged (header)."));
        }
        Ok(Decryptor {
            inner,
            sealer: Sealer::new(&key, prefix, aad_for(&json)),
            header,
            counter: 0,
            out: Vec::new(),
            pos: 0,
            done: false,
        })
    }

    fn next_record(&mut self) -> io::Result<()> {
        let mut len = [0u8; 4];
        if let Err(e) = self.inner.read_exact(&mut len) {
            return Err(if e.kind() == io::ErrorKind::UnexpectedEof {
                bad("The transfer file is incomplete (cut off). Copy it again.")
            } else {
                e
            });
        }
        let len = u32::from_be_bytes(len) as usize;
        if len < 16 || len > self.header.chunk as usize + 16 {
            return Err(bad("The transfer file is damaged."));
        }
        let mut ct = vec![0u8; len];
        self.inner.read_exact(&mut ct).map_err(|e| {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                bad("The transfer file is incomplete (cut off). Copy it again.")
            } else {
                e
            }
        })?;
        let (plain, last) = match self.sealer.open(self.counter, false, &ct) {
            Some(p) => (p, false),
            None => match self.sealer.open(self.counter, true, &ct) {
                Some(p) => (p, true),
                None => {
                    return Err(bad(
                        "The transfer file is damaged or was changed after it was made.",
                    ))
                }
            },
        };
        self.counter = self.counter.wrapping_add(1);
        if last {
            self.done = true;
            // Nothing may follow the final record.
            let mut probe = [0u8; 1];
            if self.inner.read(&mut probe)? != 0 {
                return Err(bad(
                    "The transfer file is damaged or was changed after it was made.",
                ));
            }
        }
        self.out = plain;
        self.pos = 0;
        Ok(())
    }
}

impl<R: Read> Read for Decryptor<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.pos >= self.out.len() {
            if self.done {
                return Ok(0);
            }
            self.next_record()?;
        }
        let n = buf.len().min(self.out.len() - self.pos);
        buf[..n].copy_from_slice(&self.out[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

// ---------------------------------------------------------------------------
// Bundle contents
// ---------------------------------------------------------------------------

/// `transfer.json` inside the bundle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub v: u32,
    pub created_at: String,
    pub version: String,
    pub show_name: String,
    pub leader_id: String,
    pub hostname: String,
    /// Data files (relative paths) in the bundle.
    #[serde(default)]
    pub files: Vec<String>,
}

/// What goes into a bundle.
pub struct Source<'a> {
    pub data_dir: &'a std::path::Path,
    pub manifest: Manifest,
    pub show_json: Vec<u8>,
    pub node_json: Vec<u8>,
    /// `cluster/keys.json`, when there is one.
    pub keys_json: Option<Vec<u8>>,
    /// `(ca.key, ca.crt, ca.json)` from WS1's `tls::export_ca`.
    pub ca: Option<(String, String, String)>,
}

fn tar_bytes<W: Write>(
    tar: &mut tar::Builder<W>,
    name: &str,
    data: &[u8],
    mode: u32,
) -> io::Result<()> {
    let mut h = tar::Header::new_gnu();
    h.set_size(data.len() as u64);
    h.set_mode(mode);
    h.set_mtime(chrono::Utc::now().timestamp().max(0) as u64);
    h.set_cksum();
    tar.append_data(&mut h, name, data)
}

/// Write the bundle (tar + zstd) into `out` (normally an [`Encryptor`]).
pub fn write_bundle<W: Write>(out: W, src: &Source) -> io::Result<W> {
    let enc = zstd::Encoder::new(out, 3)?;
    let mut tar = tar::Builder::new(enc);
    tar_bytes(
        &mut tar,
        "transfer.json",
        &serde_json::to_vec_pretty(&src.manifest).map_err(|e| bad(e.to_string()))?,
        0o600,
    )?;
    tar_bytes(&mut tar, "show.json", &src.show_json, 0o600)?;
    tar_bytes(&mut tar, "node.json", &src.node_json, 0o600)?;
    if let Some(k) = &src.keys_json {
        tar_bytes(&mut tar, "cluster/keys.json", k, 0o600)?;
    }
    if let Some((key, crt, meta)) = &src.ca {
        tar_bytes(&mut tar, "tls/ca.key", key.as_bytes(), 0o600)?;
        tar_bytes(&mut tar, "tls/ca.crt", crt.as_bytes(), 0o644)?;
        tar_bytes(&mut tar, "tls/ca.json", meta.as_bytes(), 0o644)?;
    }
    for rel in &src.manifest.files {
        tar.append_path_with_name(src.data_dir.join(rel), rel)?;
    }
    let enc = tar.into_inner()?;
    enc.finish()
}

/// A bundle unpacked into a staging directory (nothing applied yet).
#[derive(Debug)]
pub struct Staged {
    pub dir: std::path::PathBuf,
    pub manifest: Manifest,
    pub show: pixelplus_core::model::Show,
    pub node: crate::node::NodeIdentity,
    pub keys_json: Option<Vec<u8>>,
    pub ca: Option<(String, String, String)>,
    /// Data files unpacked under `dir` (relative paths, checked).
    pub files: Vec<String>,
}

const MAX_SMALL: u64 = 64 * 1024 * 1024;

fn read_small<R: Read>(entry: &mut tar::Entry<R>) -> io::Result<Vec<u8>> {
    if entry.header().size()? > MAX_SMALL {
        return Err(bad("The transfer file holds an oversized entry."));
    }
    let mut buf = Vec::new();
    entry.take(MAX_SMALL).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Unpack a decrypted bundle stream into `dir` (created), unpacking at most
/// `budget` bytes of data files. Unknown or unsafe entries are skipped. The
/// stream is read to its end, so a damaged / truncated file is always
/// reported before anything is used.
pub fn unpack<R: Read>(plain: R, dir: &std::path::Path, budget: u64) -> io::Result<Staged> {
    std::fs::create_dir_all(dir)?;
    let dec = zstd::Decoder::new(plain)?;
    let mut ar = tar::Archive::new(dec);
    let (mut manifest, mut show, mut node, mut keys, mut files) = (None, None, None, None, vec![]);
    let (mut ca_key, mut ca_crt, mut ca_json) = (None, None, None);
    let mut used: u64 = 0;
    for entry in ar.entries()? {
        let mut entry = entry?;
        if entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }
        let rel = entry.path()?.to_string_lossy().to_string();
        match rel.as_str() {
            "transfer.json" => {
                manifest = Some(
                    serde_json::from_slice::<Manifest>(&read_small(&mut entry)?)
                        .map_err(|_| bad("The transfer file is damaged (transfer.json)."))?,
                )
            }
            "show.json" => {
                show = Some(
                    serde_json::from_slice::<pixelplus_core::model::Show>(&read_small(&mut entry)?)
                        .map_err(|e| {
                            bad(format!("The show in the transfer file can't be read: {e}"))
                        })?,
                )
            }
            "node.json" => {
                node = Some(
                    serde_json::from_slice::<crate::node::NodeIdentity>(&read_small(&mut entry)?)
                        .map_err(|_| bad("The transfer file is damaged (node.json)."))?,
                )
            }
            "cluster/keys.json" => keys = Some(read_small(&mut entry)?),
            "tls/ca.key" => ca_key = Some(read_small(&mut entry)?),
            "tls/ca.crt" => ca_crt = Some(read_small(&mut entry)?),
            "tls/ca.json" => ca_json = Some(read_small(&mut entry)?),
            _ if crate::services::paths::check(&rel).is_some() => {
                used = used.saturating_add(entry.header().size()?);
                if used > budget {
                    return Err(io::Error::other(NoSpace));
                }
                let dst = dir.join(&rel);
                if let Some(p) = dst.parent() {
                    std::fs::create_dir_all(p)?;
                }
                entry.unpack(&dst)?;
                files.push(rel);
            }
            _ => {}
        }
    }
    let text = |v: Option<Vec<u8>>| v.and_then(|b| String::from_utf8(b).ok());
    let ca = match (text(ca_key), text(ca_crt), text(ca_json)) {
        (Some(k), Some(c), Some(j)) => Some((k, c, j)),
        _ => None,
    };
    let manifest = manifest.ok_or_else(|| bad("That isn't a controller transfer file."))?;
    let show = show.ok_or_else(|| bad("The transfer file holds no show."))?;
    let node = node.ok_or_else(|| bad("The transfer file holds no controller identity."))?;
    if !crate::cluster::slices::safe_id(&node.id) {
        return Err(bad("The transfer file is damaged (node.json)."));
    }
    Ok(Staged {
        dir: dir.to_path_buf(),
        manifest,
        show,
        node,
        keys_json: keys,
        ca,
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> Meta {
        Meta {
            show_name: "Oak Street".into(),
            leader_id: "lead123456".into(),
            hostname: "pixelplus".into(),
            version: "1.0.0".into(),
        }
    }

    /// Cheap KDF and tiny records so tests are fast and span many records.
    fn seal(data: &[u8], pass: &str, chunk: usize) -> Vec<u8> {
        let mut e = Encryptor::with_params(Vec::new(), pass, &meta(), 64, 1, chunk).unwrap();
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn open(file: &[u8], pass: &str) -> io::Result<Vec<u8>> {
        let mut d = Decryptor::new(file, pass)?;
        let mut out = Vec::new();
        d.read_to_end(&mut out)?;
        Ok(out)
    }

    fn records_start(file: &[u8]) -> usize {
        12 + u32::from_be_bytes(file[8..12].try_into().unwrap()) as usize
    }

    #[test]
    fn round_trip_across_record_boundaries() {
        for len in [0usize, 1, 99, 100, 101, 1000, 4096] {
            let data: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
            let file = seal(&data, "correct horse battery", 100);
            assert_eq!(open(&file, "correct horse battery").unwrap(), data, "{len}");
        }
        let file = seal(b"x", "correct horse battery", 100);
        let (h, _) = read_header(&mut &file[..]).unwrap();
        assert_eq!(h.show_name, "Oak Street");
        assert_eq!(h.kdf.alg, "argon2id");
    }

    #[test]
    fn wrong_passphrase_is_told_apart_from_damage() {
        let file = seal(b"secret show", "correct horse battery", 100);
        let e = open(&file, "wrong horse battery").unwrap_err();
        assert!(wrong_passphrase(&e), "{e}");
    }

    #[test]
    fn tampering_truncation_and_reordering_are_detected() {
        let data: Vec<u8> = (0..1000u32).map(|i| i as u8).collect();
        let file = seal(&data, "correct horse battery", 100);
        let start = records_start(&file);
        // A flipped bit anywhere in the records.
        for pos in [start + 5, start + 200, file.len() - 1] {
            let mut t = file.clone();
            t[pos] ^= 1;
            let e = open(&t, "correct horse battery").unwrap_err();
            assert!(!wrong_passphrase(&e));
        }
        // An altered header (e.g. the show name) is caught by the AAD.
        let mut t = file.clone();
        let hs = String::from_utf8(t[12..start].to_vec()).unwrap();
        let changed = hs.replace("Oak Street", "Elm Street");
        t.splice(12..start, changed.bytes());
        assert!(open(&t, "correct horse battery").is_err());
        // Truncated at a record boundary: the final-record flag is missing.
        let rec = 4 + 100 + 16;
        let cut = &file[..start + 3 * rec];
        let e = open(cut, "correct horse battery").unwrap_err();
        assert!(e.to_string().contains("cut off"), "{e}");
        // Truncated inside a record.
        assert!(open(&file[..file.len() - 3], "correct horse battery").is_err());
        // Two records swapped.
        let mut t = file.clone();
        let (a, b) = (start, start + rec);
        let first: Vec<u8> = t[a..a + rec].to_vec();
        let second: Vec<u8> = t[b..b + rec].to_vec();
        t[a..a + rec].copy_from_slice(&second);
        t[b..b + rec].copy_from_slice(&first);
        assert!(open(&t, "correct horse battery").is_err());
        // Trailing garbage after the final record.
        let mut t = file.clone();
        t.extend_from_slice(b"extra");
        assert!(open(&t, "correct horse battery").is_err());
        // Not a transfer file / a future version.
        assert!(open(b"hello world, not a file", "correct horse battery").is_err());
        let mut t = file.clone();
        t[7] = 2;
        let e = open(&t, "correct horse battery").unwrap_err();
        assert!(e.to_string().contains("newer PixelPlus"));
    }

    #[test]
    fn hostile_kdf_parameters_are_refused() {
        let kdf = Kdf {
            alg: "argon2id".into(),
            m: 4 * 1024 * 1024,
            t: 1,
            p: 1,
            salt: "00".repeat(16),
        };
        assert!(derive_key("correct horse battery", &kdf).is_err());
        let kdf = Kdf {
            alg: "scrypt".into(),
            m: 64,
            t: 1,
            p: 1,
            salt: "00".repeat(16),
        };
        assert!(derive_key("correct horse battery", &kdf).is_err());
    }

    #[test]
    fn passphrase_rules() {
        assert!(validate_passphrase("short").is_err());
        assert!(validate_passphrase("ten chars!").is_ok());
        assert!(validate_passphrase(&"x".repeat(2000)).is_err());
    }

    #[test]
    fn bundle_round_trip_and_unsafe_entries() {
        let dir = std::env::temp_dir().join(format!("pp-xfer-{}", pixelplus_core::model::new_id()));
        let data = dir.join("data");
        std::fs::create_dir_all(data.join("sequences")).unwrap();
        std::fs::write(data.join("sequences/s1.fseq"), b"FSEQ-bytes").unwrap();
        let show = pixelplus_core::model::Show {
            name: "Oak Street".into(),
            ..Default::default()
        };
        let node = crate::node::NodeIdentity {
            id: "lead123456".into(),
            role: crate::node::LocalRole::Leader,
            leader_url: None,
            leader_id: None,
            cluster_key: None,
            board: None,
            board_rev: None,
            name: Some("Garage".into()),
        };
        let src = Source {
            data_dir: &data,
            manifest: Manifest {
                v: 1,
                created_at: "2026-09-30T00:00:00Z".into(),
                version: "1.0.0".into(),
                show_name: "Oak Street".into(),
                leader_id: "lead123456".into(),
                hostname: "pixelplus".into(),
                files: vec!["sequences/s1.fseq".into()],
            },
            show_json: serde_json::to_vec(&show).unwrap(),
            node_json: serde_json::to_vec(&node).unwrap(),
            keys_json: Some(br#"{"followers":{"f1":"k"}}"#.to_vec()),
            ca: Some(("KEY".into(), "CRT".into(), "{}".into())),
        };
        let enc = Encryptor::with_params(Vec::new(), "correct horse battery", &meta(), 64, 1, 256)
            .unwrap();
        let file = write_bundle(enc, &src).unwrap().finish().unwrap();
        let d = Decryptor::new(&file[..], "correct horse battery").unwrap();
        let staged = unpack(d, &dir.join("stage"), u64::MAX).unwrap();
        assert_eq!(staged.show.name, "Oak Street");
        assert_eq!(staged.node.id, "lead123456");
        assert_eq!(staged.files, ["sequences/s1.fseq"]);
        assert_eq!(
            std::fs::read(staged.dir.join("sequences/s1.fseq")).unwrap(),
            b"FSEQ-bytes"
        );
        assert_eq!(staged.ca.as_ref().unwrap().1, "CRT");
        assert!(staged.keys_json.is_some());
        // Too little space.
        let d = Decryptor::new(&file[..], "correct horse battery").unwrap();
        let e = unpack(d, &dir.join("stage2"), 3).unwrap_err();
        assert!(no_space(&e), "{e}");

        // A plaintext bundle with path tricks: only safe entries are unpacked.
        let mut raw = Vec::new();
        {
            let enc = zstd::Encoder::new(&mut raw, 1).unwrap();
            let mut tar = tar::Builder::new(enc);
            tar_bytes(
                &mut tar,
                "transfer.json",
                &serde_json::to_vec(&src.manifest).unwrap(),
                0o600,
            )
            .unwrap();
            tar_bytes(&mut tar, "show.json", &src.show_json, 0o600).unwrap();
            tar_bytes(&mut tar, "node.json", &src.node_json, 0o600).unwrap();
            tar_bytes(&mut tar, "etc/passwd", b"x", 0o600).unwrap();
            tar_bytes(&mut tar, "sequences/evil.sh", b"x", 0o600).unwrap();
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_size(0);
            h.set_cksum();
            tar.append_link(&mut h, "media/a.mp3", "/etc/shadow")
                .unwrap();
            tar.into_inner().unwrap().finish().unwrap();
        }
        let staged = unpack(&raw[..], &dir.join("stage3"), u64::MAX).unwrap();
        assert!(staged.files.is_empty());
        assert!(!dir.join("stage3/etc").exists());
        assert!(!dir.join("stage3/media/a.mp3").exists());
        std::fs::remove_dir_all(dir).ok();
    }
}
