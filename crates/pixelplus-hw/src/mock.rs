//! Complete simulated boards, for development machines, Docker and demos
//! (`pixelplus --mock …`).

use crate::eeprom::{self, Ppx1Record, AT24C256_SIZE, EEPROM_ADDR};
use crate::i2c::{MockByteRegisters, MockI2c, MockRecorder, MockWordRegisters};
use crate::oled::OLED_ADDR;
use crate::rtc::{encode_time, DS3231_ADDR};
use crate::sensors::INA226_ADDR;
use pixelplus_core::model::BoardKind;

/// A simulated AT24C256 holding `record` (or blank when `None`).
pub fn mock_eeprom(record: Option<&Ppx1Record>) -> MockByteRegisters {
    let mut dev = MockByteRegisters::new(AT24C256_SIZE, 2, 0xFF);
    if let Some(image) = record.and_then(|r| eeprom::encode(r, AT24C256_SIZE).ok()) {
        dev.mem[..image.len()].copy_from_slice(&image);
    }
    dev
}

/// The I²C bus of `board` with plausible readings. With `programmed`, the
/// EEPROM holds a PPX1 record for `board` rev `rev`; otherwise it is blank.
pub fn mock_board_bus(board: BoardKind, rev: &str, programmed: bool) -> MockI2c {
    let mut bus = MockI2c::new();
    let has_eeprom = matches!(
        board,
        BoardKind::Difftx | BoardKind::Difftxlarge | BoardKind::Diffsmart
    );
    if has_eeprom {
        let record = programmed.then(|| {
            let mut r = Ppx1Record::new(board, rev);
            r.serial = Some("PPX-SIMULATE".into());
            r.notes = Some("simulated board".into());
            r
        });
        bus.add(EEPROM_ADDR, mock_eeprom(record.as_ref()));
    }
    match board {
        BoardKind::Difftxlarge => {
            bus.add(INA226_ADDR, MockWordRegisters::ina226(12.08, 2.35));
            bus.add(0x48, MockWordRegisters::lm75(38.5));
            bus.add(0x49, MockWordRegisters::lm75(44.125));
            let mut rtc = MockByteRegisters::new(0x13, 1, 0);
            if let Ok(regs) = encode_time(&chrono::Utc::now().naive_utc()) {
                rtc.mem[..7].copy_from_slice(&regs);
            }
            rtc.mem[0x11] = 0x1A; // 26.0 °C
            bus.add(DS3231_ADDR, rtc);
            bus.add(OLED_ADDR, MockRecorder::default());
        }
        BoardKind::Diffsmart => {
            bus.add(0x48, MockWordRegisters::lm75(41.0));
            bus.add(0x49, MockWordRegisters::lm75(22.5));
        }
        _ => {}
    }
    bus
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::detect;
    use crate::eeprom::I2cEeprom;
    use crate::sensors::SensorHub;

    #[test]
    fn simulated_boards_detect_as_themselves() {
        for board in [BoardKind::Difftx, BoardKind::Diffsmart, BoardKind::Difftxlarge] {
            let bus = mock_board_bus(board, "A", true);
            let mut eeprom = I2cEeprom::new(bus, EEPROM_ADDR);
            let contents = eeprom::read_contents(&mut eeprom).unwrap();
            let mut bus = eeprom.into_inner();
            let d = crate::board::classify(Some(&contents), &crate::board::probe_known(&mut bus));
            assert_eq!(d.board, Some(board));
            assert!(d.warnings.iter().all(|w| !w.contains("did not answer")), "{:?}", d.warnings);
            // Unprogrammed: suggestion only.
            let mut blank = mock_board_bus(board, "A", false);
            let d = detect(&mut blank, None);
            assert_eq!((d.board, d.suggested), (None, Some(board)));
        }
    }

    #[test]
    fn simulated_difftxlarge_sensors() {
        let bus = mock_board_bus(BoardKind::Difftxlarge, "A", true);
        let mut hub = SensorHub::new(BoardKind::Difftxlarge, Some(Box::new(bus)))
            .with_root(std::env::temp_dir().join("pixelplus-no-sysfs"));
        let ids: Vec<String> = hub.read_all().into_iter().map(|s| s.id).collect();
        assert_eq!(ids, ["driverTemp", "powerTemp", "inputVoltage", "inputCurrent", "inputPower"]);
    }
}
