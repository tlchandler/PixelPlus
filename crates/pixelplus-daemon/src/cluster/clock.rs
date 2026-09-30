//! Leader clock estimation on followers (NTP-style ping/pong, ARCHITECTURE §7.4).
//!
//! Every second the follower sends `ping{t0}` (its own clock); the leader answers
//! `pong{t0, t1}` (leader clock at reception); the follower notes `t2` on arrival.
//!
//! ```text
//! rtt    = t2 − t0
//! offset = t1 − (t0 + t2) / 2          (leader clock − follower clock)
//! ```
//!
//! Queueing delay only ever *adds* to the round trip, so the sample with the
//! smallest RTT in a sliding window is the most trustworthy one ("minimum-RTT
//! filter"); its error is bounded by `rtt / 2`.

use std::collections::VecDeque;

/// Samples kept in the sliding window.
pub const WINDOW: usize = 8;
/// Round trips longer than this are discarded outright.
pub const MAX_RTT_MS: f64 = 1_000.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub rtt_ms: f64,
    pub offset_ms: f64,
}

#[derive(Debug, Clone)]
pub struct ClockSync {
    window: usize,
    samples: VecDeque<Sample>,
    boot: Option<String>,
}

impl Default for ClockSync {
    fn default() -> Self {
        Self::new(WINDOW)
    }
}

impl ClockSync {
    pub fn new(window: usize) -> Self {
        ClockSync {
            window: window.max(1),
            samples: VecDeque::new(),
            boot: None,
        }
    }

    /// Forget everything (new leader, leader restarted, clock jumped).
    pub fn reset(&mut self) {
        self.samples.clear();
        self.boot = None;
    }

    /// Record the leader's boot id; a different one resets the filter.
    pub fn observe_boot(&mut self, boot: &str) {
        if boot.is_empty() {
            return;
        }
        if self.boot.as_deref() != Some(boot) {
            if self.boot.is_some() {
                self.samples.clear();
            }
            self.boot = Some(boot.to_string());
        }
    }

    /// Add a ping/pong exchange. Returns the accepted sample, if any.
    pub fn add(&mut self, t0: f64, t1: f64, t2: f64) -> Option<Sample> {
        let rtt = t2 - t0;
        if !(0.0..=MAX_RTT_MS).contains(&rtt) || !t1.is_finite() {
            return None;
        }
        let sample = Sample {
            rtt_ms: rtt,
            offset_ms: t1 - (t0 + t2) / 2.0,
        };
        // A jump far larger than the uncertainty means the leader restarted
        // (its clock starts at 0 again) or a clock was stepped: start over.
        if let Some(best) = self.best() {
            let tolerance = 50.0 + best.rtt_ms + sample.rtt_ms;
            if (sample.offset_ms - best.offset_ms).abs() > tolerance {
                self.samples.clear();
            }
        }
        self.samples.push_back(sample);
        while self.samples.len() > self.window {
            self.samples.pop_front();
        }
        Some(sample)
    }

    /// The minimum-RTT sample of the window.
    pub fn best(&self) -> Option<Sample> {
        self.samples
            .iter()
            .copied()
            .min_by(|a, b| a.rtt_ms.total_cmp(&b.rtt_ms))
    }

    /// Estimated `leader − local` clock offset (ms).
    pub fn offset_ms(&self) -> Option<f64> {
        self.best().map(|s| s.offset_ms)
    }

    /// Error bound of the estimate (half the best RTT, ms).
    pub fn accuracy_ms(&self) -> Option<f64> {
        self.best().map(|s| s.rtt_ms / 2.0)
    }

    /// Convert a leader timestamp to the local clock.
    pub fn to_local(&self, leader_ms: f64) -> Option<f64> {
        self.offset_ms().map(|o| leader_ms - o)
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simulate an exchange with true offset `off`, one-way delays `up`/`down`.
    fn exchange(c: &mut ClockSync, local_t0: f64, off: f64, up: f64, down: f64) -> Option<Sample> {
        let t1 = local_t0 + up + off;
        let t2 = local_t0 + up + down;
        c.add(local_t0, t1, t2)
    }

    #[test]
    fn symmetric_delay_is_exact() {
        let mut c = ClockSync::default();
        let s = exchange(&mut c, 1000.0, 5_000.0, 2.0, 2.0).unwrap();
        assert_eq!(s.rtt_ms, 4.0);
        assert_eq!(s.offset_ms, 5_000.0);
        assert_eq!(c.offset_ms(), Some(5_000.0));
        assert_eq!(c.accuracy_ms(), Some(2.0));
        assert_eq!(c.to_local(6_000.0), Some(1_000.0));
    }

    #[test]
    fn minimum_rtt_sample_wins() {
        let mut c = ClockSync::default();
        // Asymmetric queueing delays distort the estimate…
        exchange(&mut c, 0.0, -300.0, 40.0, 2.0); // error +19
        exchange(&mut c, 1000.0, -300.0, 1.0, 1.0); // exact, rtt 2
        exchange(&mut c, 2000.0, -300.0, 2.0, 30.0); // error −14
        assert_eq!(c.offset_ms(), Some(-300.0));
        assert_eq!(c.best().unwrap().rtt_ms, 2.0);
    }

    #[test]
    fn window_slides() {
        let mut c = ClockSync::new(3);
        exchange(&mut c, 0.0, 10.0, 0.5, 0.5); // best, but will age out
        for i in 1..=3 {
            exchange(&mut c, i as f64 * 1000.0, 10.0, 3.0, 5.0);
        }
        assert_eq!(c.len(), 3);
        assert_eq!(c.best().unwrap().rtt_ms, 8.0);
        assert_eq!(c.offset_ms(), Some(9.0));
    }

    #[test]
    fn rejects_nonsense_round_trips() {
        let mut c = ClockSync::default();
        assert!(c.add(100.0, 5.0, 50.0).is_none()); // negative rtt
        assert!(c.add(0.0, 5.0, 5_000.0).is_none()); // way too slow
        assert!(c.add(0.0, f64::NAN, 5.0).is_none());
        assert_eq!(c.offset_ms(), None);
    }

    #[test]
    fn leader_restart_resets_the_filter() {
        let mut c = ClockSync::default();
        for i in 0..5 {
            exchange(&mut c, i as f64 * 1000.0, 90_000.0, 1.0, 1.0);
        }
        // Leader restarted: its clock is ~0 now. A single slower sample must win
        // immediately instead of being out-voted by stale low-RTT samples.
        exchange(&mut c, 6000.0, -5_990.0, 3.0, 3.0);
        assert_eq!(c.len(), 1);
        assert_eq!(c.offset_ms(), Some(-5_990.0));
    }

    #[test]
    fn boot_change_resets() {
        let mut c = ClockSync::default();
        c.observe_boot("a");
        exchange(&mut c, 0.0, 10.0, 1.0, 1.0);
        c.observe_boot("a");
        assert_eq!(c.len(), 1);
        c.observe_boot("b");
        assert_eq!(c.len(), 0);
    }
}
