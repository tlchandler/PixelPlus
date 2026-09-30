//! WS281x bit timing and DPI display geometry.
//!
//! PixelPlus clocks the Raspberry Pi DPI peripheral at [`PIXEL_CLOCK_HZ`]
//! (38.4 MHz). One WS281x bit (1.25 µs at 800 kbit/s) is then exactly
//! [`BitTiming::STANDARD`]`.px_per_bit` = 48 framebuffer pixels of 26.04 ns,
//! and one LED (24 bits) is exactly one 1152-pixel framebuffer line. See
//! `crates/pixelplus-output/DESIGN.md` for the reasoning behind every number.

use crate::error::{OutputError, Result};
use serde::{Deserialize, Serialize};

/// The DPI pixel clock PixelPlus uses on every Raspberry Pi model.
pub const PIXEL_CLOCK_HZ: u32 = 38_400_000;

/// WS281x "high speed" data rate.
pub const WS281X_BIT_RATE_HZ: u32 = 800_000;

/// Bits per RGB LED on the wire.
pub const BITS_PER_LED: u32 = 24;

/// Minimum low time that every supported chip (including WS2812B-V5 and
/// WS2815, the strictest) accepts as a latch/reset.
pub const RESET_MIN_NS: f64 = 280_000.0;

/// Low time PixelPlus aims for between frames (a margin above [`RESET_MIN_NS`]).
pub const RESET_TARGET_NS: f64 = 300_000.0;

/// Framebuffer pixels per latch time slot: data set-up, LE high, LE high, data hold.
pub const LATCH_SLOT_PX: u32 = 4;

/// Default horizontal blanking (front porch, sync, back porch) in pixels.
///
/// Blanking only ever extends the *low* part of the 24th bit of an LED, by
/// `24 × 26.04 ns = 625 ns`, which WS281x chips ignore (they only react to
/// lows longer than several microseconds).
pub const DEFAULT_H_BLANK: (u32, u32, u32) = (8, 8, 8);

/// Default vertical blanking (front porch, sync, back porch) in lines.
pub const DEFAULT_V_BLANK: (u32, u32, u32) = (1, 1, 1);

/// Acceptance windows of a WS281x-family chip, in nanoseconds (inclusive).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ws281xSpec {
    /// Allowed high time for a `0` bit.
    pub t0h_ns: (f64, f64),
    /// Allowed high time for a `1` bit.
    pub t1h_ns: (f64, f64),
    /// Shortest low time between two bits.
    pub min_low_ns: f64,
    /// Longest low time between two bits of the same frame (anything longer
    /// risks being taken as a reset by some chips).
    pub max_low_ns: f64,
    /// Minimum reset (latch) low time.
    pub reset_ns: f64,
}

impl Ws281xSpec {
    /// The intersection of the WS2811 (high-speed mode), WS2812/WS2812B and
    /// WS2815 datasheet windows: a waveform inside it drives all of them.
    ///
    /// * T0H: WS2811 100–400, WS2812B 250–550, WS2815 220–380 → **250–380 ns**
    /// * T1H: WS2811 450–750, WS2812B 650–950, WS2815 580–1000 → **650–750 ns**
    pub const COMMON: Ws281xSpec = Ws281xSpec {
        t0h_ns: (250.0, 380.0),
        t1h_ns: (650.0, 750.0),
        min_low_ns: 400.0,
        max_low_ns: 5_000.0,
        reset_ns: RESET_MIN_NS,
    };

    /// High time that separates a decoded `0` from a `1` (midpoint of the windows).
    pub fn threshold_ns(&self) -> f64 {
        (self.t0h_ns.1 + self.t1h_ns.0) / 2.0
    }
}

/// How one WS281x bit is laid out in framebuffer pixels.
///
/// Every bit starts high; a `0` falls after `t0h_px` pixels and a `1` after
/// `t1h_px` pixels; the rest of the `px_per_bit` pixels are low.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BitTiming {
    /// Framebuffer pixels per WS281x bit.
    pub px_per_bit: u32,
    /// High pixels of a `0` bit.
    pub t0h_px: u32,
    /// High pixels of a `1` bit.
    pub t1h_px: u32,
}

impl BitTiming {
    /// 48 / 12 / 28 pixels at 38.4 MHz: 1250 ns bit, 312.5 ns T0H, 729.2 ns T1H.
    pub const STANDARD: BitTiming = BitTiming {
        px_per_bit: 48,
        t0h_px: 12,
        t1h_px: 28,
    };

    /// Derive the closest bit timing for an arbitrary pixel clock.
    ///
    /// This exists so a Pi whose DPI clock does not land exactly on 38.4 MHz
    /// still gets correct WS281x timing. The result is checked against
    /// [`Ws281xSpec::COMMON`].
    pub fn for_clock(pixel_clock_hz: u32) -> Result<BitTiming> {
        if pixel_clock_hz == PIXEL_CLOCK_HZ {
            return Ok(Self::STANDARD);
        }
        if pixel_clock_hz < 8_000_000 {
            return Err(OutputError::InvalidTiming(format!(
                "pixel clock {pixel_clock_hz} Hz is too slow for WS281x (need at least 8 MHz)"
            )));
        }
        let px_ns = 1e9 / f64::from(pixel_clock_hz);
        let round = |ns: f64| (ns / px_ns).round() as u32;
        let timing = BitTiming {
            px_per_bit: round(1_250.0),
            t0h_px: round(312.5),
            t1h_px: round(729.2),
        };
        timing.validate(pixel_clock_hz, 0)?;
        Ok(timing)
    }

    /// Check the timing is internally consistent, meets [`Ws281xSpec::COMMON`]
    /// at `pixel_clock_hz`, and leaves room for `latch_banks` latch time slots
    /// per edge (pass `0` for direct mode).
    pub fn validate(&self, pixel_clock_hz: u32, latch_banks: u32) -> Result<()> {
        let BitTiming {
            px_per_bit,
            t0h_px,
            t1h_px,
        } = *self;
        if !(0 < t0h_px && t0h_px < t1h_px && t1h_px < px_per_bit) {
            return Err(OutputError::InvalidTiming(format!(
                "need 0 < T0H ({t0h_px}) < T1H ({t1h_px}) < bit ({px_per_bit}) pixels"
            )));
        }
        if pixel_clock_hz == 0 {
            return Err(OutputError::InvalidTiming("pixel clock is 0 Hz".into()));
        }
        let px_ns = 1e9 / f64::from(pixel_clock_hz);
        let spec = Ws281xSpec::COMMON;
        let in_window = |px: u32, (lo, hi): (f64, f64)| {
            let ns = f64::from(px) * px_ns;
            ns >= lo && ns <= hi
        };
        if !in_window(t0h_px, spec.t0h_ns) {
            return Err(OutputError::InvalidTiming(format!(
                "T0H {:.0} ns is outside {:?} ns",
                f64::from(t0h_px) * px_ns,
                spec.t0h_ns
            )));
        }
        if !in_window(t1h_px, spec.t1h_ns) {
            return Err(OutputError::InvalidTiming(format!(
                "T1H {:.0} ns is outside {:?} ns",
                f64::from(t1h_px) * px_ns,
                spec.t1h_ns
            )));
        }
        let bit_ns = f64::from(px_per_bit) * px_ns;
        if !(1_100.0..=1_400.0).contains(&bit_ns) {
            return Err(OutputError::InvalidTiming(format!(
                "bit period {bit_ns:.0} ns is outside 1100-1400 ns"
            )));
        }
        if f64::from(px_per_bit - t1h_px) * px_ns < spec.min_low_ns {
            return Err(OutputError::InvalidTiming(format!(
                "T1L {:.0} ns is shorter than {} ns",
                f64::from(px_per_bit - t1h_px) * px_ns,
                spec.min_low_ns
            )));
        }
        if latch_banks > 0 {
            let window = latch_banks * LATCH_SLOT_PX;
            if t0h_px < window || t1h_px - t0h_px < window || px_per_bit - t1h_px < window {
                return Err(OutputError::InvalidTiming(format!(
                    "{latch_banks} latch banks need {window} pixels between bit edges \
                     (T0H {t0h_px}, T1H {t1h_px}, bit {px_per_bit})"
                )));
            }
        }
        Ok(())
    }
}

impl Default for BitTiming {
    fn default() -> Self {
        Self::STANDARD
    }
}

/// The complete DPI video mode PixelPlus runs, and how WS281x data sits in it.
///
/// * One framebuffer **line** carries one LED (24 bits) for every output at once.
/// * Line `n` (0-based) carries LED `n` of every string.
/// * The last `reset_lines` active lines are always zero: together with the
///   vertical blanking they form the ≥ 280 µs reset between frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DpiGeometry {
    /// DPI pixel clock.
    pub pixel_clock_hz: u32,
    /// Bit layout within a line.
    pub bit: BitTiming,
    /// LEDs per output that fit in one frame (= active lines carrying data).
    pub pixels_per_output: u32,
    /// Always-zero active lines at the bottom of the frame (reset time).
    pub reset_lines: u32,
    /// Horizontal front porch in pixels.
    pub h_front_porch: u32,
    /// Horizontal sync width in pixels.
    pub h_sync: u32,
    /// Horizontal back porch in pixels.
    pub h_back_porch: u32,
    /// Vertical front porch in lines.
    pub v_front_porch: u32,
    /// Vertical sync width in lines.
    pub v_sync: u32,
    /// Vertical back porch in lines.
    pub v_back_porch: u32,
}

impl DpiGeometry {
    /// Geometry for strings of up to `pixels_per_output` LEDs at 38.4 MHz.
    pub fn for_pixels(pixels_per_output: u32) -> Result<DpiGeometry> {
        let (hfp, hs, hbp) = DEFAULT_H_BLANK;
        let (vfp, vs, vbp) = DEFAULT_V_BLANK;
        let mut g = DpiGeometry {
            pixel_clock_hz: PIXEL_CLOCK_HZ,
            bit: BitTiming::STANDARD,
            pixels_per_output,
            reset_lines: 0,
            h_front_porch: hfp,
            h_sync: hs,
            h_back_porch: hbp,
            v_front_porch: vfp,
            v_sync: vs,
            v_back_porch: vbp,
        };
        g.reset_lines = g.reset_lines_needed();
        g.validate()?;
        Ok(g)
    }

    /// The longest strings that still refresh at `fps` frames per second.
    pub fn for_refresh(fps: f64) -> Result<DpiGeometry> {
        if !(fps.is_finite() && fps > 0.0) {
            return Err(OutputError::InvalidTiming(format!(
                "refresh rate {fps} is not a positive number"
            )));
        }
        let probe = Self::for_pixels(1)?;
        let lines_per_frame = (1e9 / fps / probe.line_ns()).floor() as u32;
        let overhead = probe.reset_lines + probe.v_blank_lines();
        if lines_per_frame <= overhead {
            return Err(OutputError::InvalidTiming(format!(
                "{fps} fps leaves no time for pixel data"
            )));
        }
        Self::for_pixels(lines_per_frame - overhead)
    }

    /// Reconstruct the geometry from a DRM/KMS video mode (the mode the
    /// overlay configured), so the encoder always matches what is on the wire.
    #[allow(clippy::too_many_arguments)]
    pub fn from_mode(
        pixel_clock_hz: u32,
        hdisplay: u32,
        hsync_start: u32,
        hsync_end: u32,
        htotal: u32,
        vdisplay: u32,
        vsync_start: u32,
        vsync_end: u32,
        vtotal: u32,
    ) -> Result<DpiGeometry> {
        let ordered = hdisplay <= hsync_start
            && hsync_start <= hsync_end
            && hsync_end <= htotal
            && vdisplay <= vsync_start
            && vsync_start <= vsync_end
            && vsync_end <= vtotal;
        if !ordered {
            return Err(OutputError::InvalidTiming(format!(
                "malformed video mode h {hdisplay}/{hsync_start}/{hsync_end}/{htotal} \
                 v {vdisplay}/{vsync_start}/{vsync_end}/{vtotal}"
            )));
        }
        let bit = BitTiming::for_clock(pixel_clock_hz)?;
        if hdisplay != BITS_PER_LED * bit.px_per_bit {
            return Err(OutputError::InvalidTiming(format!(
                "display is {hdisplay} pixels wide; PixelPlus needs exactly {} \
                 (24 bits × {} pixels) — is the pixelplus-dpi overlay loaded?",
                BITS_PER_LED * bit.px_per_bit,
                bit.px_per_bit
            )));
        }
        let mut g = DpiGeometry {
            pixel_clock_hz,
            bit,
            pixels_per_output: 0,
            reset_lines: 0,
            h_front_porch: hsync_start - hdisplay,
            h_sync: hsync_end - hsync_start,
            h_back_porch: htotal - hsync_end,
            v_front_porch: vsync_start - vdisplay,
            v_sync: vsync_end - vsync_start,
            v_back_porch: vtotal - vsync_end,
        };
        g.reset_lines = g.reset_lines_needed();
        if vdisplay <= g.reset_lines {
            return Err(OutputError::InvalidTiming(format!(
                "display is only {vdisplay} lines tall; need more than {} for the reset",
                g.reset_lines
            )));
        }
        g.pixels_per_output = vdisplay - g.reset_lines;
        g.validate()?;
        Ok(g)
    }

    /// Check every invariant the encoder relies on.
    pub fn validate(&self) -> Result<()> {
        self.bit.validate(self.pixel_clock_hz, 0)?;
        if self.pixels_per_output == 0 {
            return Err(OutputError::InvalidTiming(
                "pixels per output must be at least 1".into(),
            ));
        }
        if self.pixels_per_output > 16_384 {
            return Err(OutputError::InvalidTiming(format!(
                "{} pixels per output is beyond any supported display height",
                self.pixels_per_output
            )));
        }
        let reset = self.reset_ns();
        if reset < RESET_MIN_NS {
            return Err(OutputError::InvalidTiming(format!(
                "reset time {:.0} µs is below the {:.0} µs WS281x minimum",
                reset / 1000.0,
                RESET_MIN_NS / 1000.0
            )));
        }
        Ok(())
    }

    /// Active pixels per line (24 bits).
    pub fn hactive(&self) -> u32 {
        BITS_PER_LED * self.bit.px_per_bit
    }

    /// Pixels per line including blanking.
    pub fn htotal(&self) -> u32 {
        self.hactive() + self.h_blank_px()
    }

    /// Horizontal blanking in pixels.
    pub fn h_blank_px(&self) -> u32 {
        self.h_front_porch + self.h_sync + self.h_back_porch
    }

    /// Active lines (data lines plus reset lines).
    pub fn vactive(&self) -> u32 {
        self.pixels_per_output + self.reset_lines
    }

    /// Vertical blanking in lines.
    pub fn v_blank_lines(&self) -> u32 {
        self.v_front_porch + self.v_sync + self.v_back_porch
    }

    /// Lines per frame including blanking.
    pub fn vtotal(&self) -> u32 {
        self.vactive() + self.v_blank_lines()
    }

    /// Duration of one framebuffer pixel.
    pub fn pixel_ns(&self) -> f64 {
        1e9 / f64::from(self.pixel_clock_hz.max(1))
    }

    /// Duration of one line (= one LED on every output).
    pub fn line_ns(&self) -> f64 {
        f64::from(self.htotal()) * self.pixel_ns()
    }

    /// Duration of one full frame.
    pub fn frame_ns(&self) -> f64 {
        f64::from(self.vtotal()) * self.line_ns()
    }

    /// Display refresh rate: the maximum pixel frame rate.
    pub fn refresh_hz(&self) -> f64 {
        1e9 / self.frame_ns()
    }

    /// Guaranteed low time between the last data bit of a full-length string
    /// and the first bit of the next frame.
    pub fn reset_ns(&self) -> f64 {
        f64::from(self.reset_lines + self.v_blank_lines()) * self.line_ns()
    }

    /// Bytes of one XRGB8888 frame without row padding.
    pub fn frame_bytes(&self) -> usize {
        self.hactive() as usize * self.vactive() as usize * 4
    }

    /// Zero lines needed at the bottom of the active area so that, together
    /// with the vertical blanking, the reset reaches [`RESET_TARGET_NS`].
    /// At least one is always kept inside the active area in case a display
    /// engine holds the last line's value during blanking.
    fn reset_lines_needed(&self) -> u32 {
        let total = (RESET_TARGET_NS / self.line_ns()).ceil() as u32;
        total.saturating_sub(self.v_blank_lines()).max(1)
    }
}

impl Default for DpiGeometry {
    /// 800 LEDs per output at about 40 fps (1152 × 807 active, 40.3 Hz).
    fn default() -> Self {
        let (hfp, hs, hbp) = DEFAULT_H_BLANK;
        let (vfp, vs, vbp) = DEFAULT_V_BLANK;
        DpiGeometry {
            pixel_clock_hz: PIXEL_CLOCK_HZ,
            bit: BitTiming::STANDARD,
            pixels_per_output: 800,
            reset_lines: 7,
            h_front_porch: hfp,
            h_sync: hs,
            h_back_porch: hbp,
            v_front_porch: vfp,
            v_sync: vs,
            v_back_porch: vbp,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_timing_meets_every_chip() {
        BitTiming::STANDARD.validate(PIXEL_CLOCK_HZ, 0).unwrap();
        // Three latch banks fit; four do not with 12/28 edges.
        BitTiming::STANDARD.validate(PIXEL_CLOCK_HZ, 3).unwrap();
        assert!(BitTiming::STANDARD.validate(PIXEL_CLOCK_HZ, 4).is_err());
    }

    #[test]
    fn clock_derivation() {
        assert_eq!(
            BitTiming::for_clock(PIXEL_CLOCK_HZ).unwrap(),
            BitTiming::STANDARD
        );
        let t = BitTiming::for_clock(40_000_000).unwrap();
        assert_eq!(t.px_per_bit, 50);
        assert!(BitTiming::for_clock(1_000_000).is_err());
    }

    #[test]
    fn default_geometry_numbers() {
        let g = DpiGeometry::for_pixels(800).unwrap();
        assert_eq!(g, DpiGeometry::default());
        assert_eq!(g.hactive(), 1152);
        assert_eq!(g.htotal(), 1176);
        assert!((g.line_ns() - 30_625.0).abs() < 0.01);
        assert!(g.reset_ns() >= RESET_MIN_NS);
        assert_eq!(g.vactive(), 807);
        assert!(
            g.refresh_hz() > 40.0 && g.refresh_hz() < 40.5,
            "{}",
            g.refresh_hz()
        );
        let big = DpiGeometry::for_pixels(1600).unwrap();
        assert!(big.refresh_hz() > 20.0 && big.refresh_hz() < 20.5);
    }

    #[test]
    fn refresh_round_trip() {
        let g = DpiGeometry::for_refresh(40.0).unwrap();
        assert!(g.refresh_hz() >= 40.0);
        let one_more = DpiGeometry::for_pixels(g.pixels_per_output + 1).unwrap();
        assert!(one_more.refresh_hz() < 40.0);
        assert!(DpiGeometry::for_refresh(0.0).is_err());
        assert!(DpiGeometry::for_refresh(f64::NAN).is_err());
        assert!(DpiGeometry::for_refresh(1e6).is_err());
    }

    #[test]
    fn from_mode_matches_generated() {
        let g = DpiGeometry::for_pixels(500).unwrap();
        let h = g.hactive();
        let v = g.vactive();
        let m = DpiGeometry::from_mode(
            g.pixel_clock_hz,
            h,
            h + g.h_front_porch,
            h + g.h_front_porch + g.h_sync,
            g.htotal(),
            v,
            v + g.v_front_porch,
            v + g.v_front_porch + g.v_sync,
            g.vtotal(),
        )
        .unwrap();
        assert_eq!(m, g, "a generated mode must round-trip exactly");
    }

    #[test]
    fn from_mode_rejects_wrong_width_and_garbage() {
        assert!(
            DpiGeometry::from_mode(PIXEL_CLOCK_HZ, 800, 824, 896, 992, 480, 483, 493, 500).is_err()
        );
        assert!(
            DpiGeometry::from_mode(PIXEL_CLOCK_HZ, 1152, 1100, 1160, 1176, 20, 21, 22, 23).is_err()
        );
        assert!(
            DpiGeometry::from_mode(PIXEL_CLOCK_HZ, 1152, 1160, 1168, 1176, 5, 6, 7, 8).is_err()
        );
    }

    #[test]
    fn zero_pixels_rejected() {
        assert!(DpiGeometry::for_pixels(0).is_err());
    }
}
