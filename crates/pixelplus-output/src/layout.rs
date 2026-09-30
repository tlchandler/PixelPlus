//! Which DPI data bit (and latch bank) each board output is wired to.
//!
//! DPI data bit *n* is driven on BCM GPIO *n + 4* (see `docs/ARCHITECTURE.md` §3.1).

use crate::error::{OutputError, Result};
use crate::timing::LATCH_SLOT_PX;
use pixelplus_core::model::BoardKind;
use serde::{Deserialize, Serialize};

/// Number of DPI data bits in 24-bit mode (DPI_D0..DPI_D23 = GPIO4..GPIO27).
pub const DPI_BITS: u8 = 24;

/// BCM GPIO number of DPI data bit 0.
pub const DPI_GPIO_BASE: u8 = 4;

/// difftx / diffsmart: output 1..4 → DPI bit (Port 1 = GPIO5, Port 2 = GPIO6,
/// Port 3 = GPIO7, Port 4 = GPIO4).
pub const DIFFTX_BITS: [u8; 4] = [1, 2, 3, 0];

/// difftxlarge: data lines per latch bank (D0..D19 = GPIO4..23).
pub const DIFFTXLARGE_DATA_BITS: u8 = 20;

/// difftxlarge: latch-enable DPI bit of bank 0, 1, 2 (LE0 = GPIO27, LE1 = GPIO26, LE2 = GPIO25).
pub const DIFFTXLARGE_LE_BITS: [u8; 3] = [23, 22, 21];

/// How outputs reach the pins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OutputMode {
    /// Each output is one DPI data bit, straight to a line driver.
    Direct,
    /// Outputs are multiplexed onto shared data lines and captured by
    /// transparent latches, one latch-enable bit per bank.
    Latched,
}

/// One group of outputs written simultaneously: the whole board in direct
/// mode, or one latch bank.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lane {
    /// DPI bit of this bank's latch enable (`None` in direct mode).
    pub le_bit: Option<u8>,
    /// `(output index, DPI data bit)` pairs, output indices 0-based.
    pub outputs: Vec<(usize, u8)>,
}

/// The complete output-to-pin map of a board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputLayout {
    mode: OutputMode,
    lanes: Vec<Lane>,
    output_count: usize,
}

impl OutputLayout {
    /// The layout of a PixelPlus board. Boards without pixel outputs
    /// (`bare-pi`, `virtual`) get an empty direct layout.
    pub fn for_board(board: BoardKind) -> OutputLayout {
        let built = match board {
            BoardKind::Difftx | BoardKind::Diffsmart => Self::direct(&DIFFTX_BITS),
            BoardKind::Difftxlarge => Self::latched(DIFFTXLARGE_DATA_BITS, &DIFFTXLARGE_LE_BITS),
            BoardKind::BarePi | BoardKind::Virtual => Self::direct(&[]),
        };
        // The constant tables above are valid by construction (unit-tested);
        // fall back to an empty layout rather than panicking regardless.
        built.unwrap_or_else(|_| OutputLayout {
            mode: OutputMode::Direct,
            lanes: vec![Lane {
                le_bit: None,
                outputs: vec![],
            }],
            output_count: 0,
        })
    }

    /// Direct mode: output *i* is DPI bit `bits[i]`.
    pub fn direct(bits: &[u8]) -> Result<OutputLayout> {
        let mut seen = 0u32;
        for &b in bits {
            if b >= DPI_BITS {
                return Err(OutputError::InvalidLayout(format!(
                    "DPI bit {b} out of range 0..{DPI_BITS}"
                )));
            }
            if seen & (1 << b) != 0 {
                return Err(OutputError::InvalidLayout(format!(
                    "DPI bit {b} used twice"
                )));
            }
            seen |= 1 << b;
        }
        Ok(OutputLayout {
            mode: OutputMode::Direct,
            lanes: vec![Lane {
                le_bit: None,
                outputs: bits.iter().copied().enumerate().collect(),
            }],
            output_count: bits.len(),
        })
    }

    /// Latched mode: `data_bits` shared data lines on DPI bits `0..data_bits`,
    /// and one bank per entry of `le_bits`. Output *k* is bank `k / data_bits`,
    /// data bit `k % data_bits`.
    pub fn latched(data_bits: u8, le_bits: &[u8]) -> Result<OutputLayout> {
        if data_bits == 0 || le_bits.is_empty() {
            return Err(OutputError::InvalidLayout(
                "latched mode needs at least one data bit and one bank".into(),
            ));
        }
        if u32::from(data_bits) + le_bits.len() as u32 > u32::from(DPI_BITS) {
            return Err(OutputError::InvalidLayout(format!(
                "{data_bits} data bits + {} latch enables exceed {DPI_BITS} DPI bits",
                le_bits.len()
            )));
        }
        let data_mask = (1u32 << data_bits) - 1;
        let mut seen = data_mask;
        for &le in le_bits {
            if le >= DPI_BITS || seen & (1 << le) != 0 {
                return Err(OutputError::InvalidLayout(format!(
                    "latch enable bit {le} is out of range or collides with another line"
                )));
            }
            seen |= 1 << le;
        }
        let lanes = le_bits
            .iter()
            .enumerate()
            .map(|(bank, &le)| Lane {
                le_bit: Some(le),
                outputs: (0..data_bits)
                    .map(|bit| (bank * usize::from(data_bits) + usize::from(bit), bit))
                    .collect(),
            })
            .collect();
        Ok(OutputLayout {
            mode: OutputMode::Latched,
            lanes,
            output_count: usize::from(data_bits) * le_bits.len(),
        })
    }

    /// Direct or latched.
    pub fn mode(&self) -> OutputMode {
        self.mode
    }

    /// Output lanes (one per latch bank, or a single lane in direct mode).
    pub fn lanes(&self) -> &[Lane] {
        &self.lanes
    }

    /// Number of latch banks (0 in direct mode).
    pub fn latch_banks(&self) -> u32 {
        match self.mode {
            OutputMode::Direct => 0,
            OutputMode::Latched => self.lanes.len() as u32,
        }
    }

    /// Framebuffer pixels needed after each bit edge before the next edge
    /// may be written (latched mode only).
    pub fn edge_window_px(&self) -> u32 {
        self.latch_banks() * LATCH_SLOT_PX
    }

    /// Number of outputs this layout drives.
    pub fn output_count(&self) -> usize {
        self.output_count
    }

    /// Bit mask of every DPI bit the layout drives (data and latch enables).
    pub fn used_bits(&self) -> u32 {
        self.lanes.iter().fold(0, |acc, lane| {
            let data = lane.outputs.iter().fold(0u32, |m, &(_, b)| m | (1 << b));
            acc | data | lane.le_bit.map_or(0, |le| 1 << le)
        })
    }

    /// BCM GPIO numbers that must be switched to the DPI function, ascending.
    pub fn gpio_pins(&self) -> Vec<u8> {
        let used = self.used_bits();
        (0..DPI_BITS)
            .filter(|b| used & (1 << b) != 0)
            .map(|b| b + DPI_GPIO_BASE)
            .collect()
    }

    /// `(lane index, DPI data bit)` of output `index` (0-based).
    pub fn locate(&self, index: usize) -> Option<(usize, u8)> {
        self.lanes.iter().enumerate().find_map(|(li, lane)| {
            lane.outputs
                .iter()
                .find(|&&(o, _)| o == index)
                .map(|&(_, bit)| (li, bit))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn difftx_map() {
        let l = OutputLayout::for_board(BoardKind::Difftx);
        assert_eq!(l.mode(), OutputMode::Direct);
        assert_eq!(l.output_count(), 4);
        assert_eq!(l.locate(0), Some((0, 1))); // Port 1 = GPIO5 = bit 1
        assert_eq!(l.locate(3), Some((0, 0))); // Port 4 = GPIO4 = bit 0
        assert_eq!(l.gpio_pins(), vec![4, 5, 6, 7]);
        assert_eq!(l.latch_banks(), 0);
    }

    #[test]
    fn difftxlarge_map() {
        let l = OutputLayout::for_board(BoardKind::Difftxlarge);
        assert_eq!(l.mode(), OutputMode::Latched);
        assert_eq!(l.output_count(), 60);
        assert_eq!(l.latch_banks(), 3);
        assert_eq!(l.locate(0), Some((0, 0)));
        assert_eq!(l.locate(19), Some((0, 19)));
        assert_eq!(l.locate(20), Some((1, 0)));
        assert_eq!(l.locate(59), Some((2, 19)));
        assert_eq!(l.locate(60), None);
        assert_eq!(l.lanes()[0].le_bit, Some(23));
        assert_eq!(l.lanes()[2].le_bit, Some(21));
        let pins = l.gpio_pins();
        assert_eq!(pins.len(), 23);
        assert!(!pins.contains(&24), "GPIO24 stays free for a button");
        assert!(pins.contains(&25) && pins.contains(&27));
        assert!(pins.iter().all(|&p| p >= 4), "GPIO0-3 (I2C) never claimed");
    }

    #[test]
    fn boards_without_outputs() {
        assert_eq!(OutputLayout::for_board(BoardKind::BarePi).output_count(), 0);
        assert!(OutputLayout::for_board(BoardKind::Virtual)
            .gpio_pins()
            .is_empty());
    }

    #[test]
    fn invalid_layouts() {
        assert!(OutputLayout::direct(&[0, 0]).is_err());
        assert!(OutputLayout::direct(&[24]).is_err());
        assert!(OutputLayout::latched(20, &[5]).is_err());
        assert!(OutputLayout::latched(22, &[22, 23, 21]).is_err());
        assert!(OutputLayout::latched(0, &[23]).is_err());
        assert!(OutputLayout::latched(20, &[]).is_err());
        assert!(OutputLayout::latched(20, &[23, 23]).is_err());
    }
}
