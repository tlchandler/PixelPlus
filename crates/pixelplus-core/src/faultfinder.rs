//! Fault finder: locate the first bad pixel on a prop by binary search.
//!
//! When a string of pixels misbehaves (everything after some point is dark,
//! flickers or shows wrong colours), the fault is almost always at the first
//! misbehaving pixel or the connection just before it. The fault finder lights
//! the first *k* pixels and asks the user a yes/no question; each answer halves
//! the search space, so a 1,000-pixel prop is diagnosed in at most 10 questions.
//!
//! The search distinguishes `n + 1` outcomes (the first bad pixel is 1…n, or
//! every pixel is fine), so it takes at most ⌈log₂(n + 1)⌉ answers.
//!
//! ```
//! use pixelplus_core::faultfinder::FaultFinder;
//!
//! let mut ff = FaultFinder::new(100);
//! let bad = 37; // 0-based index of the first broken pixel
//! while let Some(step) = ff.current_step() {
//!     // The user answers "yes" if every lit pixel looks right.
//!     ff.answer(step.lit_range.end <= bad);
//! }
//! assert_eq!(ff.result().unwrap().first_bad_pixel, Some(bad));
//! ```

use serde::Serialize;
use std::ops::Range;

/// Colour of pixels lit during a question (a soft warm white, bright enough
/// to judge colour but kind to power supplies).
pub const LIT_COLOR: [u8; 3] = [150, 140, 120];
/// Colour of the last lit pixel, so the user can see where the lit run ends.
pub const BOUNDARY_COLOR: [u8; 3] = [0, 180, 255];
/// Colour of the pixel identified as faulty.
pub const FAULT_COLOR: [u8; 3] = [255, 0, 0];
/// Colour of the known-good pixels once the search is done.
pub const GOOD_COLOR: [u8; 3] = [0, 60, 0];

/// One question for the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    /// 1-based step number.
    pub number: u32,
    /// Upper bound on the number of remaining questions including this one.
    pub max_remaining: u32,
    /// 0-based prop pixel indices that are lit (always starts at 0).
    pub lit_range: Range<u32>,
    /// Plain-language question to show the user.
    pub question: String,
}

/// The outcome of a completed search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FaultResult {
    /// 0-based index of the first faulty pixel, or `None` if every pixel was
    /// reported as working.
    pub first_bad_pixel: Option<u32>,
    /// Plain-language explanation and advice.
    pub message: String,
}

/// Binary-search state machine for finding the first bad pixel.
///
/// Invariant: the first bad pixel `f` (with `f == pixel_count` meaning "none")
/// lies in `lo..=hi`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultFinder {
    pixel_count: u32,
    lo: u32,
    hi: u32,
    /// Previous `(lo, hi)` states, for [`FaultFinder::undo`].
    history: Vec<(u32, u32)>,
}

impl FaultFinder {
    /// Start a search over a prop with `pixel_count` pixels.
    pub fn new(pixel_count: u32) -> Self {
        FaultFinder {
            pixel_count,
            lo: 0,
            hi: pixel_count,
            history: Vec::new(),
        }
    }

    /// Number of pixels on the prop.
    pub fn pixel_count(&self) -> u32 {
        self.pixel_count
    }

    /// Whether the search has finished.
    pub fn is_done(&self) -> bool {
        self.lo >= self.hi
    }

    /// Number of answers given so far (after undos).
    pub fn answers_given(&self) -> u32 {
        self.history.len() as u32
    }

    /// Number of pixels lit for the current question, if one is pending.
    fn lit_count(&self) -> Option<u32> {
        // Midpoint rounded up, so that lit_count is in lo+1..=hi.
        (!self.is_done()).then(|| self.lo + (self.hi - self.lo).div_ceil(2))
    }

    /// The current question, or `None` once the search is complete.
    pub fn current_step(&self) -> Option<Step> {
        let lit = self.lit_count()?;
        let remaining = self.hi - self.lo + 1;
        let question = if lit == 1 {
            "Only pixel 1 is lit. Does it light correctly (steady, warm white, \
             the last lit pixel shows blue)?"
                .to_string()
        } else {
            format!(
                "Pixels 1\u{2013}{lit} are lit. Do they ALL light correctly? \
                 (Steady warm white; the last lit pixel, number {lit}, shows blue. \
                 Anything dark, flickering or the wrong colour means \"no\".)"
            )
        };
        Some(Step {
            number: self.answers_given() + 1,
            max_remaining: ceil_log2(remaining),
            lit_range: 0..lit,
            question,
        })
    }

    /// Record the user's answer to the current question: `ok = true` if every
    /// lit pixel lights correctly. Ignored once the search is complete.
    pub fn answer(&mut self, ok: bool) {
        let Some(lit) = self.lit_count() else {
            return;
        };
        self.history.push((self.lo, self.hi));
        if ok {
            // Pixels 0..lit are fine: the first bad pixel is at least `lit`.
            self.lo = lit;
        } else {
            // Something in 0..lit is bad.
            self.hi = lit - 1;
        }
    }

    /// Take back the most recent answer. Returns `false` if there was none.
    pub fn undo(&mut self) -> bool {
        match self.history.pop() {
            Some((lo, hi)) => {
                self.lo = lo;
                self.hi = hi;
                true
            }
            None => false,
        }
    }

    /// The result, once the search is complete.
    pub fn result(&self) -> Option<FaultResult> {
        if !self.is_done() {
            return None;
        }
        let f = self.lo;
        let n = self.pixel_count;
        let (first_bad_pixel, message) = if f >= n {
            (
                None,
                if n == 0 {
                    "This prop has no pixels to test.".to_string()
                } else {
                    format!(
                        "All {n} pixels light correctly. If the prop still misbehaves during \
                         the show, check its pixel count and wiring settings, or whether the \
                         problem comes and goes with temperature or moisture."
                    )
                },
            )
        } else if f == 0 {
            (
                Some(0),
                "Even pixel 1 doesn't light correctly, so the problem is before the prop: \
                 check the power and data connection into the first pixel, the receiver \
                 port and fuse feeding it, and the port's colour order setting."
                    .to_string(),
            )
        } else {
            let px = f + 1;
            (
                Some(f),
                format!(
                    "Pixel {px} is the first one that doesn't light correctly. The fault is \
                     most likely pixel {px} itself or the wire between pixels {f} and {px} \
                     (data line damage, a loose joint or water). Pixel {f} and everything \
                     before it work."
                ),
            )
        };
        Some(FaultResult {
            first_bad_pixel,
            message,
        })
    }

    /// Render the current state into `out` (prop pixel order, RGB, 3 bytes
    /// per pixel). Pixels beyond `out`'s length are skipped; bytes beyond the
    /// prop's pixels are left untouched.
    ///
    /// * While asking: lit pixels [`LIT_COLOR`], the last lit pixel
    ///   [`BOUNDARY_COLOR`], the rest off.
    /// * When done: good pixels dim [`GOOD_COLOR`], the faulty pixel blinking
    ///   [`FAULT_COLOR`] (2 Hz, driven by `t_ms`), the rest off.
    pub fn render(&self, t_ms: u64, out: &mut [u8]) {
        let n = self.pixel_count as usize;
        let pixels = out.chunks_exact_mut(3).take(n);
        match self.lit_count() {
            Some(lit) => {
                let lit = lit as usize;
                for (i, px) in pixels.enumerate() {
                    let c = if i + 1 == lit {
                        BOUNDARY_COLOR
                    } else if i < lit {
                        LIT_COLOR
                    } else {
                        [0, 0, 0]
                    };
                    px.copy_from_slice(&c);
                }
            }
            None => {
                let f = self.lo as usize;
                let blink_on = (t_ms / 250) % 2 == 0;
                for (i, px) in pixels.enumerate() {
                    let c = if i < f {
                        GOOD_COLOR
                    } else if i == f && blink_on {
                        FAULT_COLOR
                    } else {
                        [0, 0, 0]
                    };
                    px.copy_from_slice(&c);
                }
            }
        }
    }
}

/// ⌈log₂ x⌉ for x ≥ 1 (0 for x ≤ 1).
fn ceil_log2(x: u32) -> u32 {
    if x <= 1 {
        0
    } else {
        u32::BITS - (x - 1).leading_zeros()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simulate a user with the first bad pixel at `bad` (or none).
    fn run(n: u32, bad: Option<u32>) -> (FaultResult, u32) {
        let mut ff = FaultFinder::new(n);
        let mut steps = 0;
        while let Some(step) = ff.current_step() {
            assert_eq!(step.lit_range.start, 0);
            assert!(step.lit_range.end >= 1 && step.lit_range.end <= n);
            let ok = match bad {
                Some(b) => step.lit_range.end <= b,
                None => true,
            };
            ff.answer(ok);
            steps += 1;
            assert!(steps <= 40, "search does not terminate");
        }
        (ff.result().unwrap(), steps)
    }

    #[test]
    fn finds_every_position_within_bound() {
        for n in [1u32, 2, 3, 7, 8, 100, 120, 1000] {
            let bound = ceil_log2(n + 1);
            for bad in (0..n).map(Some).chain([None]) {
                let (res, steps) = run(n, bad);
                assert_eq!(res.first_bad_pixel, bad, "n={n} bad={bad:?}");
                assert!(steps <= bound, "n={n} bad={bad:?} steps={steps}");
            }
        }
    }

    #[test]
    fn first_step_lights_half() {
        let ff = FaultFinder::new(240);
        let step = ff.current_step().unwrap();
        assert_eq!(step.lit_range, 0..120);
        assert!(step.question.starts_with("Pixels 1\u{2013}120 are lit."));
        assert_eq!(step.number, 1);
        assert_eq!(step.max_remaining, 8);
    }

    #[test]
    fn zero_pixels_is_done_immediately() {
        let ff = FaultFinder::new(0);
        assert!(ff.current_step().is_none());
        assert_eq!(ff.result().unwrap().first_bad_pixel, None);
    }

    #[test]
    fn undo_restores_previous_question() {
        let mut ff = FaultFinder::new(50);
        let first = ff.current_step().unwrap();
        ff.answer(false);
        assert_ne!(ff.current_step().unwrap().lit_range, first.lit_range);
        assert!(ff.undo());
        assert_eq!(ff.current_step().unwrap(), first);
        assert!(!ff.undo());
    }

    #[test]
    fn answers_after_done_are_ignored() {
        let mut ff = FaultFinder::new(1);
        ff.answer(false);
        let r = ff.result().unwrap();
        ff.answer(true);
        assert_eq!(ff.result().unwrap(), r);
        assert!(r.message.contains("before the prop"));
    }

    #[test]
    fn messages_are_one_based() {
        let (res, _) = run(100, Some(56));
        assert!(res.message.starts_with("Pixel 57 is the first"));
        assert!(res.message.contains("pixels 56 and 57"));
        let (res, _) = run(100, None);
        assert!(res.message.starts_with("All 100 pixels"));
    }

    #[test]
    fn render_marks_lit_range_and_boundary() {
        let ff = FaultFinder::new(6);
        let mut buf = vec![9u8; 6 * 3 + 3];
        ff.render(0, &mut buf);
        assert_eq!(&buf[0..3], &LIT_COLOR);
        assert_eq!(&buf[3..6], &LIT_COLOR);
        assert_eq!(&buf[6..9], &BOUNDARY_COLOR);
        assert_eq!(&buf[9..18], &[0; 9]);
        // Beyond the prop: untouched.
        assert_eq!(&buf[18..], &[9, 9, 9]);
        // Short buffers are fine.
        ff.render(0, &mut [0u8; 4]);
    }

    #[test]
    fn render_result_blinks_fault() {
        let mut ff = FaultFinder::new(4);
        while let Some(step) = ff.current_step() {
            ff.answer(step.lit_range.end <= 2);
        }
        let mut on = vec![0u8; 12];
        let mut off = vec![0u8; 12];
        ff.render(0, &mut on);
        ff.render(250, &mut off);
        assert_eq!(&on[0..6], &[GOOD_COLOR, GOOD_COLOR].concat()[..]);
        assert_eq!(&on[6..9], &FAULT_COLOR);
        assert_eq!(&off[6..9], &[0, 0, 0]);
        assert_eq!(&on[9..12], &[0, 0, 0]);
    }

    #[test]
    fn serializes_camel_case() {
        let step = FaultFinder::new(10).current_step().unwrap();
        let v = serde_json::to_value(&step).unwrap();
        assert_eq!(v["litRange"]["start"], 0);
        assert_eq!(v["litRange"]["end"], 5);
        assert_eq!(v["maxRemaining"], 4);
    }
}
