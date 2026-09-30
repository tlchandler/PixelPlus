//! The PixelPlus board EEPROM (`PPX1`, `docs/ARCHITECTURE.md` §3.3).
//!
//! ```text
//! offset 0   magic   "PPX1"      4 bytes
//! offset 4   length  u16 LE      JSON payload length
//! offset 6   crc32   u32 LE      CRC-32 (IEEE) of the JSON payload
//! offset 10  json    UTF-8       {"board":"difftx","rev":"E","serial":"PPX-…","made":"2026-10-01","notes":"…"}
//! ```
//!
//! Boards carry an AT24C256 (32 KiB) at i2c-1 0x50. The kernel `at24` driver
//! exposes it as `/sys/bus/i2c/devices/1-0050/eeprom` ([`SysfsEeprom`], which
//! registers the device via `new_device` if needed); [`I2cEeprom`] talks to
//! the chip directly and [`MemoryEeprom`] is for tests.
//!
//! Boards programmed for FPP carry an `FPP02` cape image instead; it is
//! recognised ([`EepromContents::Fpp`]) so detection still knows the board.

use crate::error::{HwError, Result};
use crate::i2c::I2cBus;
use pixelplus_core::model::BoardKind;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// PPX1 magic.
pub const MAGIC: &[u8; 4] = b"PPX1";
/// Header length before the JSON payload.
pub const HEADER_LEN: usize = 10;
/// AT24C256 capacity.
pub const AT24C256_SIZE: usize = 32 * 1024;
/// AT24C256 page size.
pub const AT24C256_PAGE: usize = 64;
/// I²C address of the board EEPROM.
pub const EEPROM_ADDR: u8 = 0x50;

/// The JSON record stored in a PPX1 EEPROM.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ppx1Record {
    /// Board id (`BoardKind` kebab-case, e.g. `difftx`). Kept as a string so a
    /// record written by a newer PixelPlus still parses.
    pub board: String,
    /// Board revision letter, e.g. `E`.
    pub rev: String,
    /// Serial number, e.g. `PPX-7K2M9Q4D`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    /// Manufacturing date, `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made: Option<String>,
    /// Free text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl Ppx1Record {
    /// A record for `board` rev `rev`, with a fresh serial and today's date.
    pub fn new(board: BoardKind, rev: &str) -> Self {
        Ppx1Record {
            board: board_id(board).to_string(),
            rev: rev.trim().to_ascii_uppercase(),
            serial: Some(generate_serial()),
            made: Some(chrono::Local::now().format("%Y-%m-%d").to_string()),
            notes: None,
        }
    }

    /// The board, if this PixelPlus knows it.
    pub fn board_kind(&self) -> Option<BoardKind> {
        parse_board(&self.board)
    }

    /// Check the record can be written: known board, sane revision, fits.
    pub fn validate(&self) -> Result<()> {
        if self.board_kind().is_none() {
            return Err(HwError::InvalidArgument(format!(
                "unknown board `{}` (expected one of {})",
                self.board,
                BoardKind::ALL.map(board_id).join(", ")
            )));
        }
        let rev_ok = !self.rev.is_empty()
            && self.rev.len() <= 8
            && self.rev.chars().all(|c| c.is_ascii_alphanumeric() || c == '.');
        if !rev_ok {
            return Err(HwError::InvalidArgument(format!(
                "revision `{}` must be 1-8 letters/digits (e.g. E)",
                self.rev
            )));
        }
        if let Some(made) = &self.made {
            if chrono::NaiveDate::parse_from_str(made, "%Y-%m-%d").is_err() {
                return Err(HwError::InvalidArgument(format!(
                    "manufacturing date `{made}` must be YYYY-MM-DD"
                )));
            }
        }
        Ok(())
    }
}

/// The kebab-case id of a board (`BoardKind`'s JSON form).
pub fn board_id(board: BoardKind) -> &'static str {
    match board {
        BoardKind::Difftx => "difftx",
        BoardKind::Difftxlarge => "difftxlarge",
        BoardKind::Diffsmart => "diffsmart",
        BoardKind::BarePi => "bare-pi",
        BoardKind::Virtual => "virtual",
    }
}

/// Parse a board id (case-insensitive).
pub fn parse_board(id: &str) -> Option<BoardKind> {
    let id = id.trim().to_ascii_lowercase();
    BoardKind::ALL.into_iter().find(|b| board_id(*b) == id)
}

/// `PPX-` followed by 8 characters from an unambiguous alphabet.
pub fn generate_serial() -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
    let mut rng = rand::thread_rng();
    let tail: String = (0..8)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect();
    format!("PPX-{tail}")
}

/// Serialise a record into the PPX1 byte image.
pub fn encode(record: &Ppx1Record, capacity: usize) -> Result<Vec<u8>> {
    record.validate()?;
    let json = serde_json::to_vec(record)
        .map_err(|e| HwError::InvalidData(format!("serialising EEPROM record: {e}")))?;
    let max = capacity.saturating_sub(HEADER_LEN).min(usize::from(u16::MAX));
    if json.len() > max {
        return Err(HwError::InvalidArgument(format!(
            "EEPROM record is {} bytes; at most {max} fit",
            json.len()
        )));
    }
    let mut out = Vec::with_capacity(HEADER_LEN + json.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(json.len() as u16).to_le_bytes());
    out.extend_from_slice(&crc32fast::hash(&json).to_le_bytes());
    out.extend_from_slice(&json);
    Ok(out)
}

/// What an EEPROM holds.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum EepromContents {
    /// A valid PixelPlus record.
    Ppx1 {
        /// The record.
        record: Ppx1Record,
    },
    /// Erased (all 0xFF, or all 0x00): the setup wizard should ask for the board.
    Blank,
    /// An FPP cape image (`FPP02`), as shipped by the board's own `make_eeprom.py`.
    Fpp {
        /// Cape id, e.g. `difftx`.
        cape: String,
        /// Cape version, e.g. `1.1`.
        version: String,
        /// Cape serial.
        serial: String,
    },
    /// Something else.
    Unknown {
        /// The first bytes, for diagnostics.
        head: Vec<u8>,
    },
    /// Starts with PPX1 but fails validation.
    Corrupt {
        /// Why.
        reason: String,
    },
}

impl EepromContents {
    /// Short state name for display (`ppx1`, `blank`, `fpp`, `unknown`, `corrupt`).
    pub fn state_name(&self) -> &'static str {
        match self {
            EepromContents::Ppx1 { .. } => "ppx1",
            EepromContents::Blank => "blank",
            EepromContents::Fpp { .. } => "fpp",
            EepromContents::Unknown { .. } => "unknown",
            EepromContents::Corrupt { .. } => "corrupt",
        }
    }
}

fn c_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

/// Decode an EEPROM image (at least [`HEADER_LEN`] bytes; the payload is
/// taken from `bytes` too, so pass enough of the image).
pub fn decode(bytes: &[u8]) -> EepromContents {
    let head_len = bytes.len().min(64);
    let head = &bytes[..head_len];
    if head.is_empty() || head.iter().all(|&b| b == 0xFF) || head.iter().all(|&b| b == 0x00) {
        return EepromContents::Blank;
    }
    if bytes.starts_with(b"FPP02") && bytes.len() >= 58 {
        return EepromContents::Fpp {
            cape: c_string(&bytes[6..32]),
            version: c_string(&bytes[32..42]),
            serial: c_string(&bytes[42..58]),
        };
    }
    if !bytes.starts_with(MAGIC) {
        return EepromContents::Unknown {
            head: bytes[..bytes.len().min(16)].to_vec(),
        };
    }
    if bytes.len() < HEADER_LEN {
        return EepromContents::Corrupt {
            reason: "truncated header".into(),
        };
    }
    let len = usize::from(u16::from_le_bytes([bytes[4], bytes[5]]));
    let crc = u32::from_le_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]);
    let Some(json) = bytes.get(HEADER_LEN..HEADER_LEN + len) else {
        return EepromContents::Corrupt {
            reason: format!("payload length {len} exceeds the image"),
        };
    };
    let actual = crc32fast::hash(json);
    if actual != crc {
        return EepromContents::Corrupt {
            reason: format!("CRC mismatch (stored {crc:08x}, computed {actual:08x})"),
        };
    }
    match serde_json::from_slice::<Ppx1Record>(json) {
        Ok(record) => EepromContents::Ppx1 { record },
        Err(e) => EepromContents::Corrupt {
            reason: format!("invalid JSON: {e}"),
        },
    }
}

/// Byte-addressed storage holding an EEPROM image.
pub trait EepromStore {
    /// Capacity in bytes.
    fn size(&self) -> usize;
    /// Read `buf.len()` bytes at `offset`.
    fn read(&mut self, offset: usize, buf: &mut [u8]) -> Result<()>;
    /// Write `data` at `offset`.
    fn write(&mut self, offset: usize, data: &[u8]) -> Result<()>;
}

fn check_range(size: usize, offset: usize, len: usize) -> Result<()> {
    match offset.checked_add(len) {
        Some(end) if end <= size => Ok(()),
        _ => Err(HwError::InvalidArgument(format!(
            "EEPROM access {offset}+{len} beyond {size} bytes"
        ))),
    }
}

/// Read and decode the EEPROM.
pub fn read_contents(store: &mut dyn EepromStore) -> Result<EepromContents> {
    let mut header = [0u8; 64];
    let n = header.len().min(store.size());
    store.read(0, &mut header[..n])?;
    let header = &header[..n];
    if !header.starts_with(MAGIC) || n < HEADER_LEN {
        return Ok(decode(header));
    }
    let len = usize::from(u16::from_le_bytes([header[4], header[5]]));
    if HEADER_LEN + len > store.size() {
        return Ok(EepromContents::Corrupt {
            reason: format!("payload length {len} exceeds the {}-byte EEPROM", store.size()),
        });
    }
    let mut image = vec![0u8; HEADER_LEN + len];
    store.read(0, &mut image)?;
    Ok(decode(&image))
}

/// Write `record` and read it back to verify.
pub fn write_record(store: &mut dyn EepromStore, record: &Ppx1Record) -> Result<()> {
    let image = encode(record, store.size())?;
    store.write(0, &image)?;
    let mut check = vec![0u8; image.len()];
    store.read(0, &mut check)?;
    if check != image {
        return Err(HwError::InvalidData(
            "EEPROM read-back differs from what was written (is write-protect JP1 closed?)".into(),
        ));
    }
    Ok(())
}

/// Erase to 0xFF: the header and any PPX1 payload, or the whole chip if `full`.
pub fn erase(store: &mut dyn EepromStore, full: bool) -> Result<()> {
    let len = if full {
        store.size()
    } else {
        let mut header = [0u8; HEADER_LEN];
        let n = HEADER_LEN.min(store.size());
        store.read(0, &mut header[..n])?;
        let payload = if header.starts_with(MAGIC) {
            usize::from(u16::from_le_bytes([header[4], header[5]]))
        } else {
            0
        };
        // Also clears FPP headers (58 bytes) and anything short.
        (HEADER_LEN + payload).max(256).min(store.size())
    };
    store.write(0, &vec![0xFF; len])?;
    let mut check = vec![0u8; len.min(256)];
    store.read(0, &mut check)?;
    if check.iter().any(|&b| b != 0xFF) {
        return Err(HwError::InvalidData(
            "EEPROM did not erase (is write-protect JP1 closed?)".into(),
        ));
    }
    Ok(())
}

/// An in-memory EEPROM (tests, simulation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryEeprom {
    /// Contents.
    pub data: Vec<u8>,
}

impl MemoryEeprom {
    /// A blank (0xFF) AT24C256.
    pub fn blank() -> Self {
        MemoryEeprom {
            data: vec![0xFF; AT24C256_SIZE],
        }
    }

    /// An EEPROM holding `record`.
    pub fn with_record(record: &Ppx1Record) -> Result<Self> {
        let mut e = Self::blank();
        write_record(&mut e, record)?;
        Ok(e)
    }
}

impl EepromStore for MemoryEeprom {
    fn size(&self) -> usize {
        self.data.len()
    }

    fn read(&mut self, offset: usize, buf: &mut [u8]) -> Result<()> {
        check_range(self.data.len(), offset, buf.len())?;
        buf.copy_from_slice(&self.data[offset..offset + buf.len()]);
        Ok(())
    }

    fn write(&mut self, offset: usize, data: &[u8]) -> Result<()> {
        check_range(self.data.len(), offset, data.len())?;
        self.data[offset..offset + data.len()].copy_from_slice(data);
        Ok(())
    }
}

/// The kernel `at24` driver's sysfs file.
#[derive(Debug, Clone)]
pub struct SysfsEeprom {
    path: PathBuf,
    size: usize,
}

impl SysfsEeprom {
    /// Default path of the board EEPROM on bus 1.
    pub const DEFAULT_PATH: &'static str = "/sys/bus/i2c/devices/1-0050/eeprom";

    /// Open `bus`/`addr`, registering a `24c256` device with the kernel via
    /// `new_device` if it is not bound yet (needs root).
    pub fn open(bus: u8, addr: u8) -> Result<SysfsEeprom> {
        Self::open_in(Path::new("/"), bus, addr)
    }

    /// As [`SysfsEeprom::open`] with an alternative filesystem root (tests).
    pub fn open_in(root: &Path, bus: u8, addr: u8) -> Result<SysfsEeprom> {
        let dev = root.join(format!("sys/bus/i2c/devices/{bus}-{addr:04x}/eeprom"));
        if !dev.exists() {
            let new_device = root.join(format!("sys/bus/i2c/devices/i2c-{bus}/new_device"));
            if !new_device.exists() {
                return Err(HwError::NotFound(format!(
                    "I2C bus {bus} ({}); enable it with dtparam=i2c_arm=on",
                    new_device.display()
                )));
            }
            std::fs::write(&new_device, format!("24c256 0x{addr:02x}\n")).map_err(|e| {
                HwError::io(
                    format!("registering the EEPROM via {} (run as root)", new_device.display()),
                    e,
                )
            })?;
            let deadline = Instant::now() + Duration::from_secs(2);
            while !dev.exists() {
                if Instant::now() > deadline {
                    return Err(HwError::NotFound(format!(
                        "{} did not appear after registering 24c256 at 0x{addr:02x} \
                         (no EEPROM fitted, or the at24 driver is missing)",
                        dev.display()
                    )));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        Self::open_path(dev)
    }

    /// Use an explicit sysfs `eeprom` file.
    pub fn open_path(path: impl Into<PathBuf>) -> Result<SysfsEeprom> {
        let path = path.into();
        let size = std::fs::metadata(&path)
            .map_err(|e| HwError::io(format!("reading {}", path.display()), e))?
            .len() as usize;
        // at24 reports the real size; fall back to the AT24C256 if it reports 0.
        let size = if size == 0 { AT24C256_SIZE } else { size };
        Ok(SysfsEeprom { path, size })
    }

    /// The sysfs file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl EepromStore for SysfsEeprom {
    fn size(&self) -> usize {
        self.size
    }

    fn read(&mut self, offset: usize, buf: &mut [u8]) -> Result<()> {
        use std::io::{Read, Seek, SeekFrom};
        check_range(self.size, offset, buf.len())?;
        let mut f = std::fs::File::open(&self.path)
            .map_err(|e| HwError::io(format!("opening {}", self.path.display()), e))?;
        f.seek(SeekFrom::Start(offset as u64))
            .and_then(|_| f.read_exact(buf))
            .map_err(|e| HwError::io(format!("reading {}", self.path.display()), e))
    }

    fn write(&mut self, offset: usize, data: &[u8]) -> Result<()> {
        use std::io::{Seek, SeekFrom, Write};
        check_range(self.size, offset, data.len())?;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .open(&self.path)
            .map_err(|e| HwError::io(format!("opening {} for writing (run as root)", self.path.display()), e))?;
        f.seek(SeekFrom::Start(offset as u64))
            .and_then(|_| f.write_all(data))
            .and_then(|_| f.flush())
            .map_err(|e| HwError::io(format!("writing {}", self.path.display()), e))
    }
}

/// Direct AT24C256 access over I²C (when no kernel driver is bound).
pub struct I2cEeprom<B: I2cBus> {
    bus: B,
    addr: u8,
    size: usize,
}

impl<B: I2cBus> I2cEeprom<B> {
    /// An AT24C256 at `addr`.
    pub fn new(bus: B, addr: u8) -> Self {
        I2cEeprom {
            bus,
            addr,
            size: AT24C256_SIZE,
        }
    }

    /// Give the bus back.
    pub fn into_inner(self) -> B {
        self.bus
    }

    /// Wait for the internal write cycle (≤ 5 ms) by polling for an ACK.
    /// The poll is a bare address-pointer write (some I²C controllers,
    /// including the Pi's, reject zero-length messages).
    fn wait_ready(&mut self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_millis(25);
        loop {
            if self.bus.write(self.addr, &[0, 0]).is_ok() {
                return Ok(());
            }
            if Instant::now() > deadline {
                return Err(HwError::i2c(self.addr, "EEPROM write cycle did not finish"));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

impl<B: I2cBus> EepromStore for I2cEeprom<B> {
    fn size(&self) -> usize {
        self.size
    }

    fn read(&mut self, offset: usize, buf: &mut [u8]) -> Result<()> {
        check_range(self.size, offset, buf.len())?;
        for (i, chunk) in buf.chunks_mut(4096).enumerate() {
            let at = (offset + i * 4096) as u16;
            self.bus.write_read(self.addr, &at.to_be_bytes(), chunk)?;
        }
        Ok(())
    }

    fn write(&mut self, offset: usize, data: &[u8]) -> Result<()> {
        check_range(self.size, offset, data.len())?;
        let mut pos = offset;
        let mut rest = data;
        while !rest.is_empty() {
            // Never cross a page boundary: the chip would wrap within the page.
            let room = AT24C256_PAGE - pos % AT24C256_PAGE;
            let (now, later) = rest.split_at(room.min(rest.len()));
            let mut msg = Vec::with_capacity(2 + now.len());
            msg.extend_from_slice(&(pos as u16).to_be_bytes());
            msg.extend_from_slice(now);
            self.bus.write(self.addr, &msg)?;
            self.wait_ready()?;
            pos += now.len();
            rest = later;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i2c::{MockByteRegisters, MockI2c};

    fn record() -> Ppx1Record {
        Ppx1Record {
            board: "difftx".into(),
            rev: "E".into(),
            serial: Some("PPX-TEST0001".into()),
            made: Some("2026-10-01".into()),
            notes: Some("unit test".into()),
        }
    }

    #[test]
    fn encode_layout_matches_spec() {
        let img = encode(&record(), AT24C256_SIZE).unwrap();
        assert_eq!(&img[..4], b"PPX1");
        let len = u16::from_le_bytes([img[4], img[5]]) as usize;
        assert_eq!(img.len(), HEADER_LEN + len);
        let crc = u32::from_le_bytes([img[6], img[7], img[8], img[9]]);
        assert_eq!(crc, crc32fast::hash(&img[10..]));
        let json: serde_json::Value = serde_json::from_slice(&img[10..]).unwrap();
        assert_eq!(json["board"], "difftx");
        assert_eq!(json["serial"], "PPX-TEST0001");
    }

    #[test]
    fn round_trip_memory() {
        let mut e = MemoryEeprom::blank();
        assert_eq!(read_contents(&mut e).unwrap(), EepromContents::Blank);
        write_record(&mut e, &record()).unwrap();
        match read_contents(&mut e).unwrap() {
            EepromContents::Ppx1 { record: r } => {
                assert_eq!(r, record());
                assert_eq!(r.board_kind(), Some(BoardKind::Difftx));
            }
            other => panic!("{other:?}"),
        }
        erase(&mut e, false).unwrap();
        assert_eq!(read_contents(&mut e).unwrap(), EepromContents::Blank);
        write_record(&mut e, &record()).unwrap();
        erase(&mut e, true).unwrap();
        assert!(e.data.iter().all(|&b| b == 0xFF));
    }

    #[test]
    fn corruption_detected() {
        let mut e = MemoryEeprom::with_record(&record()).unwrap();
        e.data[12] ^= 0x20;
        assert!(matches!(read_contents(&mut e).unwrap(), EepromContents::Corrupt { .. }));
        let mut e = MemoryEeprom::blank();
        e.data[..6].copy_from_slice(b"PPX1\xff\xff");
        assert!(matches!(read_contents(&mut e).unwrap(), EepromContents::Corrupt { .. }));
        // Valid CRC over invalid JSON.
        let json = b"{not json";
        let mut img = b"PPX1".to_vec();
        img.extend_from_slice(&(json.len() as u16).to_le_bytes());
        img.extend_from_slice(&crc32fast::hash(json).to_le_bytes());
        img.extend_from_slice(json);
        assert!(matches!(decode(&img), EepromContents::Corrupt { .. }));
        assert!(matches!(decode(b"PPX1"), EepromContents::Corrupt { .. }));
    }

    #[test]
    fn fpp_cape_header_recognised() {
        // Same layout as boardtempinfo/difftx/make_eeprom.py.
        fn field(s: &str, n: usize) -> Vec<u8> {
            let mut v = s.as_bytes().to_vec();
            v.resize(n, 0);
            v
        }
        let mut img = field("FPP02", 6);
        img.extend(field("difftx", 26));
        img.extend(field("1.0", 10));
        img.extend(field("20260929121046", 16));
        img.extend(field("2", 6));
        match decode(&img) {
            EepromContents::Fpp { cape, version, serial } => {
                assert_eq!((cape.as_str(), version.as_str()), ("difftx", "1.0"));
                assert_eq!(serial, "20260929121046");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(decode(b"garbage!garbage!"), EepromContents::Unknown { .. }));
    }

    #[test]
    fn validation() {
        let mut r = record();
        r.board = "toaster".into();
        assert!(encode(&r, AT24C256_SIZE).is_err());
        let mut r = record();
        r.rev = "".into();
        assert!(r.validate().is_err());
        let mut r = record();
        r.made = Some("yesterday".into());
        assert!(r.validate().is_err());
        let mut r = record();
        r.notes = Some("x".repeat(100));
        assert!(encode(&r, 64).is_err());
        let fresh = Ppx1Record::new(BoardKind::Difftxlarge, " a ");
        assert_eq!(fresh.rev, "A");
        assert!(fresh.serial.as_deref().unwrap().starts_with("PPX-"));
        fresh.validate().unwrap();
        assert_eq!(parse_board("DIFFTXLARGE"), Some(BoardKind::Difftxlarge));
        assert_eq!(parse_board("bare-pi"), Some(BoardKind::BarePi));
    }

    #[test]
    fn i2c_eeprom_pages_and_round_trip() {
        let bus = MockI2c::new().with(EEPROM_ADDR, MockByteRegisters::new(AT24C256_SIZE, 2, 0xFF));
        let mut e = I2cEeprom::new(bus, EEPROM_ADDR);
        write_record(&mut e, &record()).unwrap();
        assert!(matches!(read_contents(&mut e).unwrap(), EepromContents::Ppx1 { .. }));
        let bus = e.into_inner();
        let dev = bus.device::<MockByteRegisters>(EEPROM_ADDR).unwrap();
        let len = encode(&record(), AT24C256_SIZE).unwrap().len();
        assert_eq!(dev.data_writes, len.div_ceil(AT24C256_PAGE));
    }

    #[test]
    fn out_of_range_access_is_an_error() {
        let mut e = MemoryEeprom::blank();
        let mut buf = [0u8; 4];
        assert!(e.read(AT24C256_SIZE - 2, &mut buf).is_err());
        assert!(e.write(usize::MAX, &buf).is_err());
    }

    #[test]
    fn sysfs_store_on_a_fake_tree() {
        let root = std::env::temp_dir().join(format!("pixelplus-eeprom-{}", std::process::id()));
        let dir = root.join("sys/bus/i2c/devices/1-0050");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("eeprom"), vec![0xFFu8; 4096]).unwrap();
        let mut e = SysfsEeprom::open_in(&root, 1, 0x50).unwrap();
        assert_eq!(e.size(), 4096);
        write_record(&mut e, &record()).unwrap();
        assert!(matches!(read_contents(&mut e).unwrap(), EepromContents::Ppx1 { .. }));
        assert!(SysfsEeprom::open_in(&root, 3, 0x50).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
