//! Access to real or simulated hardware for the CLI.
//!
//! With `--simulate <board>` every command runs against
//! [`pixelplus_hw::mock`]'s simulated board. The simulated EEPROM is kept in
//! a file in the temp directory so `eeprom write` followed by `eeprom read`
//! behaves like the real thing across invocations.

use anyhow::{Context, Result};
use pixelplus_core::model::BoardKind;
use pixelplus_hw::board::{read_pi_info, BoardDetection, PiInfo};
use pixelplus_hw::eeprom::{self, EepromStore, I2cEeprom, Ppx1Record, SysfsEeprom, EEPROM_ADDR};
use pixelplus_hw::i2c::{I2cBus, DEFAULT_BUS};
use pixelplus_hw::mock::mock_board_bus;
use std::path::PathBuf;

/// Real hardware, or a simulated board.
#[derive(Debug, Clone, Copy)]
pub struct HwContext {
    /// The simulated board, if simulating.
    pub simulate: Option<BoardKind>,
}

/// Default revision used for simulated boards.
pub fn default_rev(board: BoardKind) -> &'static str {
    match board {
        BoardKind::Difftx => "E",
        BoardKind::Difftxlarge => "A",
        _ => "1",
    }
}

impl HwContext {
    /// `true` when simulating.
    pub fn is_simulated(&self) -> bool {
        self.simulate.is_some()
    }

    /// The header I²C bus.
    pub fn bus(&self) -> Result<Box<dyn I2cBus>> {
        match self.simulate {
            Some(board) => Ok(Box::new(mock_board_bus(board, default_rev(board), false))),
            None => open_linux_bus(),
        }
    }

    fn sim_eeprom_path(board: BoardKind) -> PathBuf {
        std::env::temp_dir().join(format!(
            "pixelplus-simulated-{}.eeprom",
            eeprom::board_id(board)
        ))
    }

    /// The board EEPROM.
    pub fn eeprom(&self) -> Result<Box<dyn EepromStore>> {
        if let Some(board) = self.simulate {
            let path = Self::sim_eeprom_path(board);
            if !path.exists() {
                let mut image = vec![0xFFu8; eeprom::AT24C256_SIZE];
                let mut record = Ppx1Record::new(board, default_rev(board));
                record.serial = Some("PPX-SIMULATE".into());
                record.notes = Some("simulated board".into());
                if let Ok(bytes) = eeprom::encode(&record, image.len()) {
                    image[..bytes.len()].copy_from_slice(&bytes);
                }
                std::fs::write(&path, &image)
                    .with_context(|| format!("creating {}", path.display()))?;
            }
            return Ok(Box::new(SysfsEeprom::open_path(path)?));
        }
        match SysfsEeprom::open(DEFAULT_BUS, EEPROM_ADDR) {
            Ok(store) => Ok(Box::new(store)),
            Err(sysfs_err) => {
                // No at24 driver: talk to the chip directly.
                let bus = open_linux_bus().with_context(|| format!("{sysfs_err}"))?;
                Ok(Box::new(I2cEeprom::new(bus, EEPROM_ADDR)))
            }
        }
    }

    /// The Raspberry Pi model.
    pub fn pi_info(&self) -> Option<PiInfo> {
        match self.simulate {
            Some(_) => Some(PiInfo::from_model("Raspberry Pi 4 Model B Rev 1.5")),
            None => read_pi_info(),
        }
    }

    /// Detect the board (EEPROM + I²C probe).
    pub fn detect(&self) -> Result<BoardDetection> {
        let mut bus = self.bus()?;
        let mut store = self.eeprom().ok();
        Ok(pixelplus_hw::board::detect(
            bus.as_mut(),
            store.as_mut().map(|s| s.as_mut() as &mut dyn EepromStore),
        ))
    }

    /// The board to use: `explicit`, else the detected one.
    pub fn board_or_detect(&self, explicit: Option<BoardKind>) -> Result<BoardKind> {
        if let Some(b) = explicit {
            return Ok(b);
        }
        let d = self.detect()?;
        d.board.ok_or_else(|| {
            let hint = d
                .suggested
                .map(|s| format!(" (it looks like a {})", eeprom::board_id(s)))
                .unwrap_or_default();
            anyhow::anyhow!("could not identify the board{hint}; pass --board")
        })
    }
}

#[cfg(target_os = "linux")]
fn open_linux_bus() -> Result<Box<dyn I2cBus>> {
    Ok(Box::new(pixelplus_hw::LinuxI2c::open(DEFAULT_BUS)?))
}

#[cfg(not(target_os = "linux"))]
fn open_linux_bus() -> Result<Box<dyn I2cBus>> {
    anyhow::bail!("I2C access needs Linux; use --simulate <board> on this machine")
}
