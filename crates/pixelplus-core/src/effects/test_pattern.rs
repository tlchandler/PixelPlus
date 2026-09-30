//! Installation test patterns.
//!
//! Test patterns work on a raw run of pixels (a prop, or a whole controller
//! output) in wiring order, so they help verify wiring, colour order and pixel
//! counts independently of any layout.

use super::color::Rgb;
use serde::{Deserialize, Serialize};

/// Default colour for single-colour patterns: 50 % white, bright enough to
/// judge colour but safe for power supplies at full length.
pub const DEFAULT_TEST_COLOR: Rgb = Rgb::new(128, 128, 128);

/// How fast the chase and walk patterns step, in pixels per second.
pub const DEFAULT_STEP_RATE: f32 = 5.0;

/// A test pattern. JSON: `{"mode":"solid","color":"#ff0000"}` (the same
/// shape as the `POST /test/start` body, whose other fields are ignored).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase")]
pub enum TestPattern {
    /// Every pixel one colour (default 50 % white).
    Solid {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<Rgb>,
    },
    /// Every third pixel lit, stepping forward. Without a colour, pixels show
    /// red, green, blue in turn, which makes a wrong colour order obvious
    /// (the sequence should read red → green → blue from the start).
    Chase {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<Rgb>,
    },
    /// The whole run shows red, green, blue, then white, one second each.
    RgbCycle,
    /// Counting aid: pixel 1 yellow, every 10th pixel green, every 50th red,
    /// every 100th white, the rest dim blue.
    CountPixels,
    /// A single pixel walks along the run.
    Walk {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<Rgb>,
        /// Pixels per second (default [`DEFAULT_STEP_RATE`]).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        speed: Option<f32>,
    },
}

impl TestPattern {
    /// Render at `t_ms` since the test started into `out` (RGB, 3 bytes per
    /// pixel; the pixel count is `out.len() / 3`).
    pub fn render(&self, t_ms: u64, out: &mut [u8]) {
        let n = out.len() / 3;
        let px = out.chunks_exact_mut(3);
        match self {
            TestPattern::Solid { color } => {
                let c = color.unwrap_or(DEFAULT_TEST_COLOR).to_array();
                px.for_each(|p| p.copy_from_slice(&c));
            }
            TestPattern::Chase { color } => {
                let step = step_at(t_ms, DEFAULT_STEP_RATE) % 3;
                const RGB: [Rgb; 3] = [Rgb::new(255, 0, 0), Rgb::new(0, 255, 0), Rgb::new(0, 0, 255)];
                for (i, p) in px.enumerate() {
                    // Phase so that the pattern moves toward higher indices.
                    let slot = ((i as u64 + 3 - step) % 3) as usize;
                    let c = match color {
                        Some(c) if slot == 0 => *c,
                        Some(_) => Rgb::BLACK,
                        None => RGB[slot],
                    };
                    p.copy_from_slice(&c.to_array());
                }
            }
            TestPattern::RgbCycle => {
                let c = match (t_ms / 1000) % 4 {
                    0 => Rgb::new(255, 0, 0),
                    1 => Rgb::new(0, 255, 0),
                    2 => Rgb::new(0, 0, 255),
                    _ => DEFAULT_TEST_COLOR,
                };
                px.for_each(|p| p.copy_from_slice(&c.to_array()));
            }
            TestPattern::CountPixels => {
                for (i, p) in px.enumerate() {
                    let k = i + 1;
                    let c = if k == 1 {
                        Rgb::new(200, 160, 0)
                    } else if k % 100 == 0 {
                        Rgb::new(200, 200, 200)
                    } else if k % 50 == 0 {
                        Rgb::new(255, 0, 0)
                    } else if k % 10 == 0 {
                        Rgb::new(0, 255, 0)
                    } else {
                        Rgb::new(0, 0, 24)
                    };
                    p.copy_from_slice(&c.to_array());
                }
            }
            TestPattern::Walk { color, speed } => {
                let rate = speed
                    .filter(|s| s.is_finite() && *s > 0.0)
                    .unwrap_or(DEFAULT_STEP_RATE);
                let pos = if n == 0 {
                    0
                } else {
                    (step_at(t_ms, rate) % n as u64) as usize
                };
                let c = color.unwrap_or(Rgb::WHITE).to_array();
                for (i, p) in px.enumerate() {
                    p.copy_from_slice(if i == pos { &c } else { &[0, 0, 0] });
                }
            }
        }
    }
}

/// Render `pattern` into `out`; see [`TestPattern::render`].
pub fn render_test_pattern(pattern: &TestPattern, t_ms: u64, out: &mut [u8]) {
    pattern.render(t_ms, out);
}

fn step_at(t_ms: u64, rate: f32) -> u64 {
    (t_ms as f64 * f64::from(rate) / 1000.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(buf: &[u8], i: usize) -> [u8; 3] {
        [buf[i * 3], buf[i * 3 + 1], buf[i * 3 + 2]]
    }

    #[test]
    fn solid_default_and_custom() {
        let mut b = vec![0u8; 9];
        TestPattern::Solid { color: None }.render(0, &mut b);
        assert_eq!(px(&b, 2), [128, 128, 128]);
        TestPattern::Solid {
            color: Some(Rgb::new(1, 2, 3)),
        }
        .render(0, &mut b);
        assert_eq!(b, [1, 2, 3, 1, 2, 3, 1, 2, 3]);
    }

    #[test]
    fn rgb_chase_moves_forward() {
        let mut a = vec![0u8; 9];
        let mut b = vec![0u8; 9];
        let p = TestPattern::Chase { color: None };
        p.render(0, &mut a);
        p.render(200, &mut b); // one step at 5 px/s
        assert_eq!(px(&a, 0), [255, 0, 0]);
        assert_eq!(px(&a, 1), [0, 255, 0]);
        assert_eq!(px(&b, 1), [255, 0, 0]);
    }

    #[test]
    fn count_pixels_markers() {
        let mut b = vec![0u8; 100 * 3];
        TestPattern::CountPixels.render(0, &mut b);
        assert_eq!(px(&b, 0), [200, 160, 0]);
        assert_eq!(px(&b, 9), [0, 255, 0]);
        assert_eq!(px(&b, 49), [255, 0, 0]);
        assert_eq!(px(&b, 99), [200, 200, 200]);
        assert_eq!(px(&b, 5), [0, 0, 24]);
    }

    #[test]
    fn walk_wraps_and_handles_empty() {
        let mut b = vec![0u8; 4 * 3];
        let p = TestPattern::Walk {
            color: None,
            speed: Some(1.0),
        };
        p.render(5_000, &mut b);
        assert_eq!(px(&b, 1), [255, 255, 255]);
        assert_eq!(b.iter().filter(|&&x| x > 0).count(), 3);
        p.render(0, &mut []);
        TestPattern::Walk {
            color: None,
            speed: Some(f32::NAN),
        }
        .render(1000, &mut b);
    }

    #[test]
    fn rgb_cycle_and_serde() {
        let mut b = vec![0u8; 3];
        TestPattern::RgbCycle.render(2_500, &mut b);
        assert_eq!(b, [0, 0, 255]);
        let p: TestPattern =
            serde_json::from_str(r##"{"mode":"walk","color":"#00ff00","target":{"all":true}}"##)
                .unwrap();
        assert_eq!(
            p,
            TestPattern::Walk {
                color: Some(Rgb::new(0, 255, 0)),
                speed: None
            }
        );
        let p: TestPattern = serde_json::from_str(r#"{"mode":"countPixels"}"#).unwrap();
        assert_eq!(p, TestPattern::CountPixels);
        assert_eq!(
            serde_json::to_string(&TestPattern::RgbCycle).unwrap(),
            r#"{"mode":"rgbCycle"}"#
        );
    }
}
