//! Which board and which Raspberry Pi PixelPlus is running on.
//!
//! The EEPROM is authoritative: a valid `PPX1` record names the board (and an
//! `FPP02` cape image from the board's FPP days is understood too). A blank or
//! unreadable EEPROM leaves the board undetected, so the setup wizard asks the
//! user; an I²C probe still offers a *suggestion*:
//!
//! | devices present | suggestion |
//! |---|---|
//! | 0x40 (INA226) and 0x68 (DS3231) | difftxlarge |
//! | 0x48 and 0x49 (two LM75B), no 0x40 | diffsmart |
//! | 0x50 (EEPROM) only | difftx |

use crate::eeprom::{self, EepromContents, EepromStore};
use crate::i2c::I2cBus;
use pixelplus_core::model::BoardKind;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// I²C addresses PixelPlus boards use.
pub const KNOWN_ADDRESSES: [u8; 6] = [0x3C, 0x40, 0x48, 0x49, 0x50, 0x68];

/// Paths that hold the Raspberry Pi model string.
pub const MODEL_PATHS: [&str; 2] = ["/proc/device-tree/model", "/sys/firmware/devicetree/base/model"];

/// Raspberry Pi product family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PiFamily {
    /// Pi 1 (A+/B+).
    Pi1,
    /// Pi 2.
    Pi2,
    /// Pi Zero / Zero W.
    Zero,
    /// Pi Zero 2 W.
    Zero2W,
    /// Pi 3 (A+/B/B+).
    Pi3,
    /// Pi 4 B.
    Pi4,
    /// Pi 400.
    Pi400,
    /// Pi 5.
    Pi5,
    /// Pi 500.
    Pi500,
    /// Compute Module 3.
    Cm3,
    /// Compute Module 4.
    Cm4,
    /// Compute Module 5.
    Cm5,
    /// A Raspberry Pi this version does not know.
    Unknown,
}

impl PiFamily {
    /// Classify a device-tree model string.
    pub fn from_model(model: &str) -> PiFamily {
        let m = model.trim_end_matches('\0').trim();
        let Some(rest) = m.strip_prefix("Raspberry Pi") else {
            return PiFamily::Unknown;
        };
        let words: Vec<&str> = rest.split_whitespace().collect();
        match words.as_slice() {
            ["Zero", "2", ..] => PiFamily::Zero2W,
            ["Zero", ..] => PiFamily::Zero,
            ["5", ..] => PiFamily::Pi5,
            ["500", ..] | ["500+", ..] => PiFamily::Pi500,
            ["4", ..] => PiFamily::Pi4,
            ["400", ..] => PiFamily::Pi400,
            ["3", ..] => PiFamily::Pi3,
            ["2", ..] => PiFamily::Pi2,
            ["Model", ..] => PiFamily::Pi1,
            ["Compute", "Module", n, ..] if n.starts_with('5') => PiFamily::Cm5,
            ["Compute", "Module", n, ..] if n.starts_with('4') => PiFamily::Cm4,
            ["Compute", "Module", n, ..] if n.starts_with('3') => PiFamily::Cm3,
            _ => PiFamily::Unknown,
        }
    }

    /// `true` for models with the RP1 southbridge (different GPIO/DPI blocks).
    pub fn is_rp1(self) -> bool {
        matches!(self, PiFamily::Pi5 | PiFamily::Pi500 | PiFamily::Cm5)
    }

    /// `true` if the model has the 3.5 mm analog audio jack.
    pub fn has_analog_audio(self) -> bool {
        matches!(self, PiFamily::Pi1 | PiFamily::Pi2 | PiFamily::Pi3 | PiFamily::Pi4)
    }

    /// `true` for models PixelPlus supports as pixel controllers.
    pub fn is_supported(self) -> bool {
        matches!(
            self,
            PiFamily::Zero2W
                | PiFamily::Pi3
                | PiFamily::Pi4
                | PiFamily::Pi400
                | PiFamily::Pi5
                | PiFamily::Pi500
                | PiFamily::Cm4
                | PiFamily::Cm5
        )
    }
}

/// The Raspberry Pi this is running on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiInfo {
    /// Full model string, e.g. `Raspberry Pi 4 Model B Rev 1.5`.
    pub model: String,
    /// Product family.
    pub family: PiFamily,
}

impl PiInfo {
    /// Build from a model string.
    pub fn from_model(model: &str) -> PiInfo {
        PiInfo {
            model: model.trim_end_matches('\0').trim().to_string(),
            family: PiFamily::from_model(model),
        }
    }
}

/// Read the Pi model from the device tree; `None` when not on a Raspberry Pi.
pub fn read_pi_info() -> Option<PiInfo> {
    read_pi_info_in(Path::new("/"))
}

/// [`read_pi_info`] with an alternative filesystem root (tests).
pub fn read_pi_info_in(root: &Path) -> Option<PiInfo> {
    MODEL_PATHS.iter().find_map(|p| {
        let s = std::fs::read_to_string(root.join(p.trim_start_matches('/'))).ok()?;
        let info = PiInfo::from_model(&s);
        info.model.starts_with("Raspberry Pi").then_some(info)
    })
}

/// Where a board identification came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DetectionSource {
    /// A valid PPX1 EEPROM record.
    Eeprom,
    /// An FPP cape EEPROM image naming a PixelPlus board.
    FppEeprom,
    /// Not identified (see `suggested`).
    None,
}

/// The result of board detection.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardDetection {
    /// The identified board (EEPROM-based), if any.
    pub board: Option<BoardKind>,
    /// Board revision.
    pub rev: Option<String>,
    /// Board serial number.
    pub serial: Option<String>,
    /// How `board` was identified.
    pub source: DetectionSource,
    /// Best guess from the I²C devices present, to preselect in the wizard.
    pub suggested: Option<BoardKind>,
    /// What the EEPROM holds (`None` if it could not be read).
    pub eeprom: Option<EepromContents>,
    /// I²C addresses that acknowledged (of [`KNOWN_ADDRESSES`]).
    pub i2c_devices: Vec<u8>,
    /// Things the user must know (e.g. the difftx rev D port 3 lead).
    pub warnings: Vec<String>,
}

/// Guess the board from the I²C devices present.
pub fn suggest_from_devices(present: &[u8]) -> Option<BoardKind> {
    let has = |a: u8| present.contains(&a);
    if has(0x40) && has(0x68) {
        Some(BoardKind::Difftxlarge)
    } else if has(0x48) && has(0x49) && !has(0x40) {
        Some(BoardKind::Diffsmart)
    } else if has(0x50) && !has(0x40) && !has(0x48) && !has(0x49) && !has(0x68) {
        Some(BoardKind::Difftx)
    } else {
        None
    }
}

/// Warnings that apply to a board revision regardless of how it was found.
pub fn board_warnings(board: BoardKind, rev: Option<&str>) -> Vec<String> {
    let mut w = Vec::new();
    match board {
        BoardKind::Difftx if rev.is_some_and(|r| r.eq_ignore_ascii_case("D")) => w.push(
            "difftx rev D: port 3 has + and − swapped on RJ45 pins 4/5. Port 3 needs a \
             patch lead with pins 4 and 5 swapped at one end (a 4/5-swapped patch lead)."
                .to_string(),
        ),
        BoardKind::Diffsmart => w.push(
            "diffsmart: set switch SW1 to PI so the Pi drives the four outputs.".to_string(),
        ),
        _ => {}
    }
    w
}

/// Map an FPP cape image to a PixelPlus board and revision.
fn from_fpp(cape: &str, version: &str, present: &[u8]) -> Option<(BoardKind, Option<String>)> {
    match cape.to_ascii_lowercase().as_str() {
        // diffsmart boards are flashed with difftx's image; their two LM75s
        // give them away.
        "difftx" if present.contains(&0x48) && present.contains(&0x49) => {
            Some((BoardKind::Diffsmart, None))
        }
        "difftx" => Some((
            BoardKind::Difftx,
            Some(match version {
                "1.0" => "D".to_string(),
                "1.1" => "E".to_string(),
                other => other.to_string(),
            }),
        )),
        "difftxlarge" => Some((
            BoardKind::Difftxlarge,
            Some(if version == "1.0" { "A".into() } else { version.to_string() }),
        )),
        "diffsmart" => Some((BoardKind::Diffsmart, None)),
        _ => None,
    }
}

/// Combine EEPROM contents and the devices present into a detection result.
pub fn classify(eeprom: Option<&EepromContents>, present: &[u8]) -> BoardDetection {
    let mut d = BoardDetection {
        board: None,
        rev: None,
        serial: None,
        source: DetectionSource::None,
        suggested: suggest_from_devices(present),
        eeprom: eeprom.cloned(),
        i2c_devices: present.to_vec(),
        warnings: Vec::new(),
    };
    match eeprom {
        Some(EepromContents::Ppx1 { record }) => match record.board_kind() {
            Some(board) => {
                d.board = Some(board);
                d.rev = Some(record.rev.clone());
                d.serial = record.serial.clone();
                d.source = DetectionSource::Eeprom;
            }
            None => d.warnings.push(format!(
                "EEPROM names board `{}`, which this PixelPlus version does not know; update PixelPlus",
                record.board
            )),
        },
        Some(EepromContents::Fpp {
            cape,
            version,
            serial,
        }) => match from_fpp(cape, version, present) {
            Some((board, rev)) => {
                d.board = Some(board);
                d.rev = rev;
                d.serial = (!serial.is_empty()).then(|| serial.clone());
                d.source = DetectionSource::FppEeprom;
                d.warnings.push(
                    "The EEPROM holds an FPP cape image. PixelPlus understands it; \
                     `pixelplus eeprom write` would replace it with a PixelPlus record \
                     (FPP would then no longer recognise the board)."
                        .into(),
                );
            }
            None => d.warnings.push(format!(
                "The EEPROM holds FPP cape `{cape}`, which is not a PixelPlus board"
            )),
        },
        Some(EepromContents::Corrupt { reason }) => d
            .warnings
            .push(format!("The board EEPROM is corrupt ({reason}); rewrite it")),
        Some(EepromContents::Unknown { .. }) => d
            .warnings
            .push("The board EEPROM holds data PixelPlus does not recognise".into()),
        Some(EepromContents::Blank) | None => {}
    }
    if let Some(board) = d.board {
        d.warnings.extend(board_warnings(board, d.rev.as_deref()));
        let expected: &[(u8, &str)] = match board {
            BoardKind::Difftxlarge => &[(0x40, "INA226 power monitor"), (0x48, "LM75B"), (0x49, "LM75B"), (0x68, "DS3231 RTC")],
            BoardKind::Diffsmart => &[(0x48, "LM75B"), (0x49, "LM75B")],
            _ => &[],
        };
        for (addr, what) in expected {
            if !present.contains(addr) {
                d.warnings.push(format!(
                    "Expected the {what} at 0x{addr:02x} on this board, but it did not answer"
                ));
            }
        }
    }
    d
}

/// Addresses among [`KNOWN_ADDRESSES`] that acknowledge on `bus`.
pub fn probe_known(bus: &mut dyn I2cBus) -> Vec<u8> {
    KNOWN_ADDRESSES
        .iter()
        .copied()
        .filter(|&a| bus.probe(a))
        .collect()
}

/// Full detection: read the EEPROM (if available) and probe the bus.
pub fn detect(bus: &mut dyn I2cBus, eeprom: Option<&mut dyn EepromStore>) -> BoardDetection {
    let present = probe_known(bus);
    let contents = eeprom.and_then(|store| match eeprom::read_contents(store) {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::debug!("board EEPROM unreadable: {e}");
            None
        }
    });
    classify(contents.as_ref(), &present)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eeprom::{MemoryEeprom, Ppx1Record};
    use crate::i2c::{MockByteRegisters, MockI2c, MockWordRegisters};

    #[test]
    fn pi_models() {
        let cases = [
            ("Raspberry Pi Zero 2 W Rev 1.0\0", PiFamily::Zero2W),
            ("Raspberry Pi Zero W Rev 1.1", PiFamily::Zero),
            ("Raspberry Pi 3 Model B Plus Rev 1.3", PiFamily::Pi3),
            ("Raspberry Pi 4 Model B Rev 1.5", PiFamily::Pi4),
            ("Raspberry Pi 400 Rev 1.0", PiFamily::Pi400),
            ("Raspberry Pi 5 Model B Rev 1.0", PiFamily::Pi5),
            ("Raspberry Pi 500 Rev 1.0", PiFamily::Pi500),
            ("Raspberry Pi Compute Module 4 Rev 1.1", PiFamily::Cm4),
            ("Raspberry Pi Compute Module 5 Rev 1.0", PiFamily::Cm5),
            ("Raspberry Pi Model B Plus Rev 1.2", PiFamily::Pi1),
            ("Raspberry Pi 2 Model B Rev 1.1", PiFamily::Pi2),
            ("Something Else", PiFamily::Unknown),
        ];
        for (m, f) in cases {
            assert_eq!(PiFamily::from_model(m), f, "{m}");
        }
        assert!(PiFamily::Pi5.is_rp1() && !PiFamily::Pi4.is_rp1());
        assert!(!PiFamily::Pi5.has_analog_audio());
        assert!(PiFamily::Zero2W.is_supported() && !PiFamily::Zero.is_supported());
        assert_eq!(PiInfo::from_model("Raspberry Pi 4 Model B Rev 1.5\0").model, "Raspberry Pi 4 Model B Rev 1.5");
    }

    #[test]
    fn pi_info_from_fake_root() {
        let root = std::env::temp_dir().join(format!("pixelplus-model-{}", std::process::id()));
        std::fs::create_dir_all(root.join("proc/device-tree")).unwrap();
        std::fs::write(root.join("proc/device-tree/model"), b"Raspberry Pi 5 Model B Rev 1.0\0").unwrap();
        let info = read_pi_info_in(&root).unwrap();
        assert_eq!(info.family, PiFamily::Pi5);
        let empty = std::env::temp_dir().join(format!("pixelplus-nomodel-{}", std::process::id()));
        assert!(read_pi_info_in(&empty).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn heuristics() {
        assert_eq!(suggest_from_devices(&[0x40, 0x48, 0x49, 0x50, 0x68]), Some(BoardKind::Difftxlarge));
        assert_eq!(suggest_from_devices(&[0x48, 0x49, 0x50]), Some(BoardKind::Diffsmart));
        assert_eq!(suggest_from_devices(&[0x48, 0x49]), Some(BoardKind::Diffsmart));
        assert_eq!(suggest_from_devices(&[0x50]), Some(BoardKind::Difftx));
        assert_eq!(suggest_from_devices(&[]), None);
        assert_eq!(suggest_from_devices(&[0x50, 0x3C, 0x68]), None);
    }

    #[test]
    fn eeprom_is_authoritative() {
        let rec = Ppx1Record {
            board: "difftx".into(),
            rev: "D".into(),
            serial: Some("PPX-1".into()),
            made: None,
            notes: None,
        };
        let d = classify(Some(&EepromContents::Ppx1 { record: rec }), &[0x50]);
        assert_eq!(d.board, Some(BoardKind::Difftx));
        assert_eq!(d.source, DetectionSource::Eeprom);
        assert!(d.warnings.iter().any(|w| w.contains("4/5-swapped")), "{:?}", d.warnings);
    }

    #[test]
    fn blank_eeprom_gives_suggestion_only() {
        let d = classify(Some(&EepromContents::Blank), &[0x40, 0x48, 0x49, 0x50, 0x68]);
        assert_eq!(d.board, None);
        assert_eq!(d.source, DetectionSource::None);
        assert_eq!(d.suggested, Some(BoardKind::Difftxlarge));
    }

    #[test]
    fn fpp_images() {
        let fpp = |cape: &str, version: &str| EepromContents::Fpp {
            cape: cape.into(),
            version: version.into(),
            serial: "2026".into(),
        };
        let d = classify(Some(&fpp("difftx", "1.1")), &[0x50]);
        assert_eq!((d.board, d.rev.as_deref()), (Some(BoardKind::Difftx), Some("E")));
        assert_eq!(d.source, DetectionSource::FppEeprom);
        let d = classify(Some(&fpp("difftx", "1.0")), &[0x50]);
        assert!(d.warnings.iter().any(|w| w.contains("rev D")));
        let d = classify(Some(&fpp("difftx", "1.1")), &[0x48, 0x49, 0x50]);
        assert_eq!(d.board, Some(BoardKind::Diffsmart));
        assert!(d.warnings.iter().any(|w| w.contains("SW1")));
        let d = classify(Some(&fpp("difftxlarge", "1.0")), &[0x50]);
        assert_eq!((d.board, d.rev.as_deref()), (Some(BoardKind::Difftxlarge), Some("A")));
        assert!(d.warnings.iter().any(|w| w.contains("0x40")), "missing INA226 flagged");
        let d = classify(Some(&fpp("k8-pi", "1.0")), &[0x50]);
        assert_eq!(d.board, None);
    }

    #[test]
    fn end_to_end_with_mocks() {
        let mut bus = MockI2c::new()
            .with(0x50, MockByteRegisters::new(256, 2, 0xFF))
            .with(0x48, MockWordRegisters::lm75(30.0))
            .with(0x49, MockWordRegisters::lm75(31.0));
        let rec = Ppx1Record::new(BoardKind::Diffsmart, "1");
        let mut e = MemoryEeprom::with_record(&rec).unwrap();
        let d = detect(&mut bus, Some(&mut e));
        assert_eq!(d.board, Some(BoardKind::Diffsmart));
        assert_eq!(d.i2c_devices, vec![0x48, 0x49, 0x50]);
        let d = detect(&mut bus, None);
        assert_eq!((d.board, d.suggested), (None, Some(BoardKind::Diffsmart)));
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(json["suggested"], "diffsmart");
        assert!(json["i2cDevices"].is_array());
    }
}
