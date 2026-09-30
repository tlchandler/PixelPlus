//! Raspberry Pi boot configuration for DPI pixel output.
//!
//! Generates the `/boot/firmware/config.txt` fragment for a board and Pi
//! model, and ships the device-tree overlay sources that define the DPI mode.
//!
//! PixelPlus uses the **KMS** display stack (`vc4-kms-v3d` on Pi 0–4, the RP1
//! DPI driver on Pi 5), which is the only stack Raspberry Pi OS Bookworm and
//! Trixie support. The legacy firmware settings `enable_dpi_lcd`,
//! `dpi_group`, `dpi_mode`, `dpi_output_format` and `dpi_timings` belong to
//! the retired firmware display driver and are ignored under KMS, so they are
//! not generated; the overlay's `panel-dpi` node carries the timings instead.

use crate::error::{OutputError, Result};
use crate::layout::OutputLayout;
use crate::timing::DpiGeometry;
use pixelplus_core::model::BoardKind;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

/// Overlay source for Pi Zero 2 W / 3 / 4 / 400 / CM3 / CM4.
pub const OVERLAY_SOURCE: &str = include_str!("../overlays/pixelplus-dpi.dts");

/// Overlay source for Pi 5 / 500 / CM5 (RP1).
pub const OVERLAY_SOURCE_PI5: &str = include_str!("../overlays/pixelplus-dpi-pi5.dts");

/// The SoC family, which decides DPI block, pin function and limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DpiSoc {
    /// BCM2835/6/7 (Pi 1, 2, 3, Zero, Zero 2 W, CM3): VideoCore IV, HVS max 2048 lines.
    Bcm283x,
    /// BCM2711 (Pi 4, 400, CM4): VideoCore VI.
    Bcm2711,
    /// BCM2712 + RP1 (Pi 5, 500, CM5): DPI and GPIO live on the RP1 southbridge.
    Bcm2712,
}

impl DpiSoc {
    /// Classify a `/proc/device-tree/model` string
    /// (e.g. `"Raspberry Pi 4 Model B Rev 1.5"`). `None` if it is not a Raspberry Pi.
    pub fn from_model(model: &str) -> Option<DpiSoc> {
        let m = model.trim_end_matches('\0').trim();
        if !m.starts_with("Raspberry Pi") {
            return None;
        }
        let rest = &m["Raspberry Pi".len()..];
        let tok = rest.split_whitespace().collect::<Vec<_>>();
        let first = tok.first().copied().unwrap_or("");
        let second = tok.get(1).copied().unwrap_or("");
        Some(match (first, second) {
            ("5", _) | ("500", _) | ("500+", _) => DpiSoc::Bcm2712,
            ("Compute", "Module") => match tok.get(2).copied().unwrap_or("") {
                s if s.starts_with('5') => DpiSoc::Bcm2712,
                s if s.starts_with('4') => DpiSoc::Bcm2711,
                _ => DpiSoc::Bcm283x,
            },
            ("4", _) | ("400", _) => DpiSoc::Bcm2711,
            _ => DpiSoc::Bcm283x,
        })
    }

    /// Tallest display mode the display engine accepts (lines).
    pub fn max_lines(self) -> u32 {
        match self {
            DpiSoc::Bcm283x => 2048,
            DpiSoc::Bcm2711 => 7680,
            // RP1 DPI: conservative until measured on hardware.
            DpiSoc::Bcm2712 => 4096,
        }
    }

    /// Longest string (LEDs per output) this SoC can carry in one frame.
    pub fn max_pixels_per_output(self) -> u32 {
        // Reset lines are constant (10) for the default geometry.
        let reset = DpiGeometry::default().reset_lines;
        self.max_lines().saturating_sub(reset)
    }

    /// Alternate function that routes a GPIO to the DPI block (`pinctrl` syntax).
    pub fn dpi_function(self) -> &'static str {
        match self {
            DpiSoc::Bcm283x | DpiSoc::Bcm2711 => "a2",
            DpiSoc::Bcm2712 => "a1",
        }
    }

    /// Name of the overlay to load (`dtoverlay=` value, without parameters).
    pub fn overlay_name(self) -> &'static str {
        match self {
            DpiSoc::Bcm283x | DpiSoc::Bcm2711 => "pixelplus-dpi",
            DpiSoc::Bcm2712 => "pixelplus-dpi-pi5",
        }
    }

    /// The overlay's device-tree source.
    pub fn overlay_source(self) -> &'static str {
        match self {
            DpiSoc::Bcm283x | DpiSoc::Bcm2711 => OVERLAY_SOURCE,
            DpiSoc::Bcm2712 => OVERLAY_SOURCE_PI5,
        }
    }

    /// Short human description.
    pub fn describe(self) -> &'static str {
        match self {
            DpiSoc::Bcm283x => "Pi Zero 2 W / 3 (BCM283x)",
            DpiSoc::Bcm2711 => "Pi 4 / 400 (BCM2711)",
            DpiSoc::Bcm2712 => "Pi 5 (BCM2712 + RP1)",
        }
    }
}

/// Compress sorted GPIO numbers into `config.txt`/`pinctrl` ranges: `4-23,25-27`.
pub fn gpio_ranges(pins: &[u8]) -> String {
    let mut sorted = pins.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut out = String::new();
    let mut i = 0;
    while i < sorted.len() {
        let start = sorted[i];
        let mut end = start;
        while i + 1 < sorted.len() && sorted[i + 1] == end + 1 {
            i += 1;
            end = sorted[i];
        }
        if !out.is_empty() {
            out.push(',');
        }
        if start == end {
            let _ = write!(out, "{start}");
        } else {
            let _ = write!(out, "{start}-{end}");
        }
        i += 1;
    }
    out
}

/// The `dtoverlay=` line for `soc` and `geometry` (only non-default parameters).
pub fn overlay_line(soc: DpiSoc, geometry: &DpiGeometry) -> String {
    let d = DpiGeometry::default();
    let mut line = format!("dtoverlay={}", soc.overlay_name());
    let params: [(&str, u32, u32); 9] = [
        ("clock-frequency", geometry.pixel_clock_hz, d.pixel_clock_hz),
        ("hactive", geometry.hactive(), d.hactive()),
        ("hfp", geometry.h_front_porch, d.h_front_porch),
        ("hsync", geometry.h_sync, d.h_sync),
        ("hbp", geometry.h_back_porch, d.h_back_porch),
        // vactive is always written: it is what changes with string length.
        ("vactive", geometry.vactive(), u32::MAX),
        ("vfp", geometry.v_front_porch, d.v_front_porch),
        ("vsync", geometry.v_sync, d.v_sync),
        ("vbp", geometry.v_back_porch, d.v_back_porch),
    ];
    for (name, value, default) in params {
        if value != default {
            let _ = write!(line, ",{name}={value}");
        }
    }
    line
}

/// Build the `config.txt` fragment for `board` on `soc` with `geometry`.
///
/// Fails if the geometry does not fit the SoC's display engine.
pub fn config_txt(board: BoardKind, soc: DpiSoc, geometry: &DpiGeometry) -> Result<String> {
    geometry.validate()?;
    if geometry.vactive() > soc.max_lines() {
        return Err(OutputError::InvalidTiming(format!(
            "{} pixels per output needs {} display lines; the {} supports at most {} \
             ({} pixels per output)",
            geometry.pixels_per_output,
            geometry.vactive(),
            soc.describe(),
            soc.max_lines(),
            soc.max_pixels_per_output()
        )));
    }
    let layout = OutputLayout::for_board(board);
    let pins = layout.gpio_pins();
    let mut s = String::new();
    let _ = writeln!(
        s,
        "# --- PixelPlus: {} on {} (generated by `pixelplus config-txt`) ---",
        board.display_name(),
        soc.describe()
    );
    s.push_str("[all]\n");
    s.push_str("# I2C-1 on GPIO2/3: board EEPROM, sensors, RTC, OLED.\n");
    s.push_str("dtparam=i2c_arm=on\n");
    if soc != DpiSoc::Bcm2712 {
        s.push_str("# Analog audio uses internal GPIO40/41, no conflict with DPI.\n");
        s.push_str("dtparam=audio=on\n");
    }

    if !pins.is_empty() {
        let _ = writeln!(
            s,
            "# WS281x pixel engine: {} outputs, up to {} pixels per output at {:.1} fps.",
            layout.output_count(),
            geometry.pixels_per_output,
            geometry.refresh_hz()
        );
        if soc == DpiSoc::Bcm2712 {
            s.push_str("# Uses the RP1 DPI block; keep the default dtoverlay=vc4-kms-v3d for HDMI.\n");
        } else {
            s.push_str("# Needs the KMS display stack: keep dtoverlay=vc4-kms-v3d (Raspberry Pi OS default).\n");
        }
        let _ = writeln!(s, "{}", overlay_line(soc, geometry));
        let _ = writeln!(
            s,
            "# Hold the pixel lines low from power-on until pixelplusd switches them to DPI ({}).",
            soc.dpi_function()
        );
        let _ = writeln!(s, "gpio={}=op,dl", gpio_ranges(&pins));
    }

    if board == BoardKind::Difftxlarge {
        s.push_str("# DS3231 battery-backed RTC at 0x68.\n");
        s.push_str("dtoverlay=i2c-rtc,ds3231\n");
        if soc == DpiSoc::Bcm2712 {
            s.push_str("# The board's USB-C output advertises 3 A; let the Pi 5 use it fully.\n");
            s.push_str("usb_max_current_enable=1\n");
        }
    }
    s.push_str("# --- end PixelPlus ---\n");
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_strings() {
        let cases = [
            ("Raspberry Pi 4 Model B Rev 1.5", Some(DpiSoc::Bcm2711)),
            ("Raspberry Pi 400 Rev 1.0\0", Some(DpiSoc::Bcm2711)),
            ("Raspberry Pi 5 Model B Rev 1.0", Some(DpiSoc::Bcm2712)),
            ("Raspberry Pi 500 Rev 1.0", Some(DpiSoc::Bcm2712)),
            ("Raspberry Pi Compute Module 5 Rev 1.0", Some(DpiSoc::Bcm2712)),
            ("Raspberry Pi Compute Module 4 Rev 1.1", Some(DpiSoc::Bcm2711)),
            ("Raspberry Pi Zero 2 W Rev 1.0", Some(DpiSoc::Bcm283x)),
            ("Raspberry Pi 3 Model B Plus Rev 1.3", Some(DpiSoc::Bcm283x)),
            ("Generic x86 PC", None),
            ("", None),
        ];
        for (model, want) in cases {
            assert_eq!(DpiSoc::from_model(model), want, "{model}");
        }
    }

    #[test]
    fn ranges() {
        assert_eq!(gpio_ranges(&[4, 5, 6, 7]), "4-7");
        let large: Vec<u8> = (4..=23).chain(25..=27).collect();
        assert_eq!(gpio_ranges(&large), "4-23,25-27");
        assert_eq!(gpio_ranges(&[9, 3, 3]), "3,9");
        assert_eq!(gpio_ranges(&[]), "");
    }

    #[test]
    fn difftx_fragment() {
        let g = DpiGeometry::for_pixels(800).unwrap();
        let s = config_txt(BoardKind::Difftx, DpiSoc::Bcm283x, &g).unwrap();
        assert!(s.contains("dtoverlay=pixelplus-dpi,vactive=810\n"), "{s}");
        assert!(s.contains("gpio=4-7=op,dl\n"));
        assert!(s.contains("dtparam=i2c_arm=on"));
        assert!(!s.contains("ds3231"));
        assert!(!s.contains("enable_dpi_lcd"));
    }

    #[test]
    fn difftxlarge_pi5_fragment() {
        let g = DpiGeometry::for_pixels(1600).unwrap();
        let s = config_txt(BoardKind::Difftxlarge, DpiSoc::Bcm2712, &g).unwrap();
        assert!(s.contains("dtoverlay=pixelplus-dpi-pi5,vactive=1610"), "{s}");
        assert!(s.contains("gpio=4-23,25-27=op,dl"));
        assert!(s.contains("dtoverlay=i2c-rtc,ds3231"));
        assert!(s.contains("usb_max_current_enable=1"));
        assert!(!s.contains("dtparam=audio=on"));
    }

    #[test]
    fn bare_pi_has_no_dpi() {
        let g = DpiGeometry::default();
        let s = config_txt(BoardKind::BarePi, DpiSoc::Bcm2711, &g).unwrap();
        assert!(!s.contains("pixelplus-dpi"));
        assert!(!s.contains("gpio="));
    }

    #[test]
    fn too_tall_for_pi3() {
        let g = DpiGeometry::for_pixels(3000).unwrap();
        assert!(config_txt(BoardKind::Difftx, DpiSoc::Bcm283x, &g).is_err());
        assert!(config_txt(BoardKind::Difftx, DpiSoc::Bcm2711, &g).is_ok());
        assert_eq!(DpiSoc::Bcm283x.max_pixels_per_output(), 2038);
    }

    #[test]
    fn overlay_parameters_only_when_changed() {
        let mut g = DpiGeometry::for_pixels(100).unwrap();
        assert_eq!(overlay_line(DpiSoc::Bcm2711, &g), "dtoverlay=pixelplus-dpi,vactive=110");
        g.h_front_porch = 16;
        assert_eq!(
            overlay_line(DpiSoc::Bcm2711, &g),
            "dtoverlay=pixelplus-dpi,hfp=16,vactive=110"
        );
        assert!(DpiSoc::Bcm2712.overlay_source().contains("rp1_dpi"));
        assert!(DpiSoc::Bcm2711.overlay_source().contains("target = <&dpi>"));
    }
}
