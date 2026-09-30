//! Built-in test patterns for bring-up and oscilloscope timing checks.
//!
//! [`TestPattern::Solid`], [`TestPattern::Chase`] and [`TestPattern::RgbCycle`]
//! produce show-order RGB and should go through a [`crate::PixelPipeline`].
//! [`TestPattern::Scope`] patterns are raw **wire** bytes chosen for what
//! they look like on an oscilloscope, and must bypass the pipeline.

use crate::frame::OutputFrame;
use serde::{Deserialize, Serialize};

/// Wire-level patterns for measuring the waveform with an oscilloscope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScopePattern {
    /// Every byte `0x00`: a train of `0` bits — measure T0H and the bit period.
    Zeros,
    /// Every byte `0xFF`: a train of `1` bits — measure T1H.
    Ones,
    /// Every byte `0xAA`: alternating `1`/`0` — both pulse widths side by side.
    Alternating,
    /// LEDs alternate `00 00 00` / `FF FF FF` — checks LED boundaries and h-blank.
    Checker,
    /// First LED of output *k* is `[k, 0x00, 0xFF]` (k 1-based), the rest
    /// zeros — the first byte on each pin names its port in binary, which
    /// verifies the pin/latch-bank map.
    Identify,
}

impl ScopePattern {
    /// Every scope pattern.
    pub const ALL: [ScopePattern; 5] = [
        ScopePattern::Zeros,
        ScopePattern::Ones,
        ScopePattern::Alternating,
        ScopePattern::Checker,
        ScopePattern::Identify,
    ];

    /// Short name used on the command line.
    pub fn name(self) -> &'static str {
        match self {
            ScopePattern::Zeros => "zeros",
            ScopePattern::Ones => "ones",
            ScopePattern::Alternating => "alternating",
            ScopePattern::Checker => "checker",
            ScopePattern::Identify => "identify",
        }
    }

    /// Parse a [`ScopePattern::name`].
    pub fn from_name(name: &str) -> Option<ScopePattern> {
        Self::ALL.into_iter().find(|p| p.name() == name)
    }

    /// What to look for on the scope.
    pub fn describe(self) -> &'static str {
        match self {
            ScopePattern::Zeros => "0x00 bytes: 312 ns pulses every 1.25 µs",
            ScopePattern::Ones => "0xFF bytes: 729 ns pulses every 1.25 µs",
            ScopePattern::Alternating => "0xAA bytes: alternating 729 ns / 312 ns pulses",
            ScopePattern::Checker => "LEDs alternate all-0 / all-1 (24 short, 24 long pulses)",
            ScopePattern::Identify => "first byte on each pin = its output number, MSB first",
        }
    }

    fn fill(self, output: usize, bytes: &mut [u8]) {
        match self {
            ScopePattern::Zeros => bytes.fill(0x00),
            ScopePattern::Ones => bytes.fill(0xFF),
            ScopePattern::Alternating => bytes.fill(0xAA),
            ScopePattern::Checker => {
                for (i, led) in bytes.chunks_mut(3).enumerate() {
                    led.fill(if i % 2 == 0 { 0x00 } else { 0xFF });
                }
            }
            ScopePattern::Identify => {
                bytes.fill(0);
                let id = [((output + 1) & 0xFF) as u8, 0x00, 0xFF];
                for (b, v) in bytes.iter_mut().zip(id) {
                    *b = v;
                }
            }
        }
    }
}

/// A test pattern for every output of a board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum TestPattern {
    /// Every LED one colour (RGB).
    Solid {
        /// Colour, show-order RGB.
        color: [u8; 3],
    },
    /// Every `spacing`-th LED lit, moving one LED per step.
    Chase {
        /// Colour, show-order RGB.
        color: [u8; 3],
        /// Distance between lit LEDs (at least 1).
        spacing: usize,
    },
    /// Whole strings step red → green → blue → white.
    RgbCycle,
    /// Raw wire pattern for the oscilloscope.
    Scope {
        /// Which waveform.
        pattern: ScopePattern,
    },
}

impl TestPattern {
    /// `true` for patterns that are already wire bytes (skip the pipeline).
    pub fn is_wire_level(&self) -> bool {
        matches!(self, TestPattern::Scope { .. })
    }

    /// Render step `step` (callers advance it with time, e.g. every 100 ms
    /// for a chase) for `outputs` outputs of `pixels` LEDs into `frame`.
    pub fn render(&self, step: u64, outputs: usize, pixels: usize, frame: &mut OutputFrame) {
        frame.reshape(outputs, pixels);
        for (o, bytes) in frame.outputs.iter_mut().enumerate() {
            match *self {
                TestPattern::Solid { color } => fill_color(bytes, color),
                TestPattern::Chase { color, spacing } => {
                    let spacing = spacing.max(1);
                    let phase = (step % spacing as u64) as usize;
                    for (i, led) in bytes.chunks_mut(3).enumerate() {
                        let lit = (i + spacing - phase) % spacing == 0;
                        led.copy_from_slice(if lit { &color } else { &[0, 0, 0] });
                    }
                }
                TestPattern::RgbCycle => {
                    const STEPS: [[u8; 3]; 4] =
                        [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255]];
                    fill_color(bytes, STEPS[(step % 4) as usize]);
                }
                TestPattern::Scope { pattern } => pattern.fill(o, bytes),
            }
        }
    }
}

fn fill_color(bytes: &mut [u8], color: [u8; 3]) {
    for led in bytes.chunks_mut(3) {
        let n = led.len();
        led.copy_from_slice(&color[..n]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_and_cycle() {
        let mut f = OutputFrame::default();
        TestPattern::Solid { color: [1, 2, 3] }.render(0, 2, 2, &mut f);
        assert_eq!(f.outputs, vec![vec![1, 2, 3, 1, 2, 3]; 2]);
        TestPattern::RgbCycle.render(5, 1, 1, &mut f);
        assert_eq!(f.outputs[0], vec![0, 255, 0]);
    }

    #[test]
    fn chase_moves() {
        let mut f = OutputFrame::default();
        let p = TestPattern::Chase {
            color: [9, 9, 9],
            spacing: 3,
        };
        p.render(0, 1, 6, &mut f);
        assert_eq!(f.outputs[0][0], 9);
        assert_eq!(f.outputs[0][3], 0);
        assert_eq!(f.outputs[0][9], 9);
        p.render(1, 1, 6, &mut f);
        assert_eq!(f.outputs[0][0], 0);
        assert_eq!(f.outputs[0][3], 9);
        // spacing 0 never divides by zero
        TestPattern::Chase {
            color: [1, 1, 1],
            spacing: 0,
        }
        .render(7, 1, 2, &mut f);
        assert_eq!(f.outputs[0], vec![1; 6]);
    }

    #[test]
    fn scope_patterns() {
        let mut f = OutputFrame::default();
        TestPattern::Scope {
            pattern: ScopePattern::Identify,
        }
        .render(0, 3, 2, &mut f);
        assert_eq!(f.outputs[2], vec![3, 0, 0xFF, 0, 0, 0]);
        TestPattern::Scope {
            pattern: ScopePattern::Checker,
        }
        .render(0, 1, 2, &mut f);
        assert_eq!(f.outputs[0], vec![0, 0, 0, 0xFF, 0xFF, 0xFF]);
        for p in ScopePattern::ALL {
            assert_eq!(ScopePattern::from_name(p.name()), Some(p));
            assert!(!p.describe().is_empty());
        }
        assert!(TestPattern::Scope {
            pattern: ScopePattern::Ones
        }
        .is_wire_level());
        assert!(!TestPattern::RgbCycle.is_wire_level());
    }
}
