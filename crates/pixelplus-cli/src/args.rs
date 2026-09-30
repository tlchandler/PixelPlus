//! Shared command-line value types.

use clap::ValueEnum;
use pixelplus_core::model::{BoardKind, ColorOrder};
use pixelplus_output::DpiSoc;

/// A PixelPlus board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BoardArg {
    /// PixelPlus pHAT: 4 outputs on one RJ45.
    Difftx,
    /// 60-port transmitter: 15 RJ45, 3 latch banks.
    Difftxlarge,
    /// Smart Receiver in standalone (PI) mode: 4 outputs.
    Diffsmart,
    /// A Raspberry Pi without a PixelPlus board.
    BarePi,
    /// A virtual node (Docker / PC).
    Virtual,
}

impl From<BoardArg> for BoardKind {
    fn from(b: BoardArg) -> Self {
        match b {
            BoardArg::Difftx => BoardKind::Difftx,
            BoardArg::Difftxlarge => BoardKind::Difftxlarge,
            BoardArg::Diffsmart => BoardKind::Diffsmart,
            BoardArg::BarePi => BoardKind::BarePi,
            BoardArg::Virtual => BoardKind::Virtual,
        }
    }
}

/// Raspberry Pi generation for `config-txt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PiArg {
    /// Detect from /proc/device-tree/model.
    Auto,
    /// Pi Zero 2 W / 3 / CM3 (BCM283x).
    Pi3,
    /// Pi 4 / 400 / CM4 (BCM2711).
    Pi4,
    /// Pi 5 / 500 / CM5 (BCM2712 + RP1).
    Pi5,
}

impl PiArg {
    /// The SoC family, or `None` for `auto`.
    pub fn soc(self) -> Option<DpiSoc> {
        match self {
            PiArg::Auto => None,
            PiArg::Pi3 => Some(DpiSoc::Bcm283x),
            PiArg::Pi4 => Some(DpiSoc::Bcm2711),
            PiArg::Pi5 => Some(DpiSoc::Bcm2712),
        }
    }
}

/// Colour order of the pixels under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "UPPER")]
pub enum OrderArg {
    /// Red, green, blue.
    Rgb,
    /// Red, blue, green.
    Rbg,
    /// Green, red, blue (most WS2812B).
    Grb,
    /// Green, blue, red.
    Gbr,
    /// Blue, red, green.
    Brg,
    /// Blue, green, red.
    Bgr,
}

impl From<OrderArg> for ColorOrder {
    fn from(o: OrderArg) -> Self {
        match o {
            OrderArg::Rgb => ColorOrder::RGB,
            OrderArg::Rbg => ColorOrder::RBG,
            OrderArg::Grb => ColorOrder::GRB,
            OrderArg::Gbr => ColorOrder::GBR,
            OrderArg::Brg => ColorOrder::BRG,
            OrderArg::Bgr => ColorOrder::BGR,
        }
    }
}

/// Parse `#rrggbb`, `rrggbb` or a colour name.
pub fn parse_color(s: &str) -> Result<[u8; 3], String> {
    let named = match s.to_ascii_lowercase().as_str() {
        "red" => Some([255, 0, 0]),
        "green" => Some([0, 255, 0]),
        "blue" => Some([0, 0, 255]),
        "white" => Some([255, 255, 255]),
        "warm" | "warmwhite" => Some([255, 160, 60]),
        "amber" => Some([245, 165, 36]),
        "off" | "black" => Some([0, 0, 0]),
        _ => None,
    };
    if let Some(c) = named {
        return Ok(c);
    }
    let hex = s.trim_start_matches('#');
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "`{s}` is not a colour (use e.g. ff0000, #00ff00 or red)"
        ));
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| e.to_string());
    Ok([byte(0)?, byte(2)?, byte(4)?])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours() {
        assert_eq!(parse_color("#FF8000").unwrap(), [255, 128, 0]);
        assert_eq!(parse_color("00ff00").unwrap(), [0, 255, 0]);
        assert_eq!(parse_color("Red").unwrap(), [255, 0, 0]);
        assert!(parse_color("ff00").is_err());
        assert!(parse_color("gg0000").is_err());
        assert!(parse_color("é12345").is_err());
    }

    #[test]
    fn conversions() {
        assert_eq!(BoardKind::from(BoardArg::BarePi), BoardKind::BarePi);
        assert_eq!(ColorOrder::from(OrderArg::Grb), ColorOrder::GRB);
        assert_eq!(PiArg::Pi5.soc(), Some(DpiSoc::Bcm2712));
        assert_eq!(PiArg::Auto.soc(), None);
    }
}
