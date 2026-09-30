//! Playback clocks and timing math.
//!
//! * [`MonoClock`]: a pausable, seekable monotonic position clock (used when no
//!   audio plays, and as the fallback when the audio device fails).
//! * [`Servo`]: a position clock that follows a target timeline continuously
//!   (ARCHITECTURE §7.4): followers track the leader's timeline anchor, the
//!   leader tracks its audio clock. Small errors are removed by a bounded rate
//!   slew (no deadband), large ones (> [`JUMP_THRESHOLD_MS`]) or a new epoch
//!   (seek, new item) by a jump.
//! * Presentation-time frame selection ([`frame_for_slot`], [`next_update`]).
//! * Crossfade / fade helpers shared by lights and audio.
//!
//! Everything takes explicit `now` values so it is tested with a fake clock.

/// Jump instead of slewing beyond this error (ms).
pub const JUMP_THRESHOLD_MS: f64 = 100.0;
/// Maximum slew rate deviation (±2 %, invisible for lights).
pub const MAX_SLEW: f64 = 0.02;
/// Largest believable rate deviation of a tracked timeline (±0.5 %).
pub const MAX_FREQ: f64 = 0.005;

/// Pausable monotonic position clock. Times are in ms on any monotonic base.
#[derive(Debug, Clone)]
pub struct MonoClock {
    base_pos: f64,
    base_at: f64,
    paused: bool,
}

impl MonoClock {
    pub fn new(pos_ms: f64, now_ms: f64) -> Self {
        MonoClock {
            base_pos: pos_ms,
            base_at: now_ms,
            paused: false,
        }
    }

    pub fn pos(&self, now_ms: f64) -> f64 {
        if self.paused {
            self.base_pos
        } else {
            self.base_pos + (now_ms - self.base_at).max(0.0)
        }
    }

    pub fn set_paused(&mut self, paused: bool, now_ms: f64) {
        if paused != self.paused {
            self.base_pos = self.pos(now_ms);
            self.base_at = now_ms;
            self.paused = paused;
        }
    }

    pub fn seek(&mut self, pos_ms: f64, now_ms: f64) {
        self.base_pos = pos_ms;
        self.base_at = now_ms;
    }
}

/// What a servo update did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SyncAction {
    /// Following: slewing with this rate (1.0 = real time).
    Slew(f64),
    /// Error too large or a new epoch: jumped to the target.
    Jump,
    /// Not running (paused / holding): set to the target.
    Hold,
}

/// Loop gains of a [`Servo`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServoGains {
    /// Proportional gain (1/ms): the error is removed with time constant 1/kp.
    pub kp: f64,
    /// Integral gain (1/ms²): learns a rate the feed-forward does not know.
    pub ki: f64,
}

impl ServoGains {
    /// Followers: the target's rate is known (feed-forward), so a fast P loop
    /// (τ = 0.5 s) suffices; model updates of a fraction of a millisecond are
    /// smoothed out over about a second.
    pub const FOLLOWER: ServoGains = ServoGains {
        kp: 1.0 / 500.0,
        ki: 0.0,
    };
    /// Leader tracking its audio clock: the audio clock is noisy (ALSA delay
    /// jitter, 10 ms steps on the Pi headphone jack) and its rate relative to
    /// the monotonic clock is unknown (sound-card crystal): a slow, critically
    /// damped PI loop (τ ≈ 2 s) filters the jitter and learns the rate.
    pub const AUDIO: ServoGains = ServoGains {
        kp: 1.0 / 2000.0,
        ki: 1.0 / 2000.0 / 2000.0 / 4.0,
    };
}

/// A position clock that follows a target timeline (see the module docs).
///
/// `update` is called every output frame with the target position *now*
/// and, when known, the target's rate (feed-forward). Between updates the
/// position advances at the current rate, so it can be evaluated at any time
/// ([`Servo::pos_at`]), e.g. at the moment a frame will light up.
#[derive(Debug, Clone)]
pub struct Servo {
    pos: f64,
    last: f64,
    /// Rate applied until the next update.
    rate: f64,
    /// Learnt rate of the target (integrator) when no feed-forward is given.
    freq: f64,
    running: bool,
    epoch: Option<u64>,
    gains: ServoGains,
    /// Smoothed |error| (ms) for diagnostics.
    err_avg: f64,
    jumps: u64,
}

impl Servo {
    pub fn new(pos_ms: f64, now_ms: f64, gains: ServoGains) -> Self {
        Servo {
            pos: pos_ms,
            last: now_ms,
            rate: 1.0,
            freq: 1.0,
            running: true,
            epoch: None,
            gains,
            err_avg: 0.0,
            jumps: 0,
        }
    }

    /// Jumps so far (a discontinuity of the followed timeline).
    pub fn jumps(&self) -> u64 {
        self.jumps
    }

    /// Advance to `now` and return the position.
    pub fn advance(&mut self, now_ms: f64) -> f64 {
        let dt = now_ms - self.last;
        if dt > 0.0 {
            if self.running {
                self.pos += dt * self.rate;
            }
            self.last = now_ms;
        }
        self.pos
    }

    /// Position at `t` (usually slightly in the future: when the next frame
    /// lights up), extrapolated with the current rate. Does not change state.
    pub fn pos_at(&self, t_ms: f64) -> f64 {
        if self.running {
            self.pos + (t_ms - self.last) * self.rate
        } else {
            self.pos
        }
    }

    /// Position at the last update.
    pub fn pos(&self) -> f64 {
        self.pos
    }

    /// Rate applied until the next update (timeline ms per clock ms; 0 when
    /// not running).
    pub fn rate(&self) -> f64 {
        if self.running {
            self.rate
        } else {
            0.0
        }
    }

    pub fn running(&self) -> bool {
        self.running
    }

    /// The followed timeline's own rate as learnt (or given as
    /// feed-forward), without the error correction; 0 when not running.
    /// Smooth, so it is what sync anchors carry.
    pub fn freq(&self) -> f64 {
        if self.running {
            self.freq
        } else {
            0.0
        }
    }

    /// Smoothed absolute tracking error (ms).
    pub fn error_ms(&self) -> f64 {
        self.err_avg
    }

    pub fn set_running(&mut self, running: bool, now_ms: f64) {
        self.advance(now_ms);
        self.running = running;
    }

    /// Follow `target_ms` (the target position at `now_ms`). `target_rate`
    /// is the target's own rate if known (feed-forward); `epoch` changes on
    /// seeks and new items and forces a jump.
    pub fn update(
        &mut self,
        target_ms: f64,
        target_rate: Option<f64>,
        epoch: Option<u64>,
        now_ms: f64,
    ) -> SyncAction {
        let dt = (now_ms - self.last).max(0.0);
        let pos = self.advance(now_ms);
        let err = target_ms - pos;
        let new_epoch = epoch.is_some() && epoch != self.epoch;
        if epoch.is_some() {
            self.epoch = epoch;
        }
        if !self.running {
            self.pos = target_ms;
            return SyncAction::Hold;
        }
        if new_epoch || err.abs() > JUMP_THRESHOLD_MS {
            self.pos = target_ms;
            let r = target_rate.unwrap_or(self.freq);
            self.freq = r.clamp(1.0 - MAX_FREQ, 1.0 + MAX_FREQ);
            self.rate = if target_rate == Some(0.0) {
                0.0
            } else {
                self.freq
            };
            self.err_avg = 0.0;
            self.jumps += 1;
            return SyncAction::Jump;
        }
        let a = (dt / 1000.0).min(1.0);
        self.err_avg += a * (err.abs() - self.err_avg);
        let ff = match target_rate {
            Some(r) => {
                self.freq = r;
                r
            }
            None => {
                self.freq =
                    (self.freq + self.gains.ki * err * dt).clamp(1.0 - MAX_FREQ, 1.0 + MAX_FREQ);
                self.freq
            }
        };
        let correction = (self.gains.kp * err).clamp(-MAX_SLEW, MAX_SLEW);
        self.rate = (ff + correction).max(0.0);
        SyncAction::Slew(self.rate)
    }
}

// ---------------------------------------------------------------------------
// Presentation-time frame selection
// ---------------------------------------------------------------------------

/// The frame to show in a presentation slot that lights up at timeline
/// position `pos_ms` and stays until the next update, `slot_ms` later: the
/// frame whose interval covers the middle of the slot. Every node then
/// changes frames within ±slot/2 of the ideal instant instead of 0..slot late.
pub fn frame_for_slot(pos_ms: f64, slot_ms: f64, frame_ms: f64) -> u32 {
    if frame_ms <= 0.0 {
        return 0;
    }
    ((pos_ms + slot_ms.max(0.0) / 2.0) / frame_ms)
        .floor()
        .clamp(0.0, u32::MAX as f64) as u32
}

/// When (clock ms) the next update should light up so that the frame after
/// `shown` appears as close as possible to its ideal instant.
///
/// The timeline advances at `rate` from `pos_ms` at `now_ms`. With a
/// presentation slot of `slot_ms` (refresh period, or the software-timer
/// granularity) [`frame_for_slot`] switches to the next frame as soon as the
/// slot starts less than half a slot before the boundary, so that is the
/// target. `None` when the timeline is not moving.
pub fn next_update(
    pos_ms: f64,
    rate: f64,
    now_ms: f64,
    shown: u32,
    frame_ms: f64,
    slot_ms: f64,
) -> Option<f64> {
    if rate <= 0.0 || frame_ms <= 0.0 {
        return None;
    }
    let boundary = (shown as f64 + 1.0) * frame_ms - slot_ms / 2.0;
    Some(now_ms + (boundary - pos_ms) / rate)
}

/// Equal-power gain for a fade level in 0..=1 (sin curve). A fade-in at level
/// `t` and a fade-out at level `1 - t` always sum to constant power.
#[cfg_attr(not(feature = "audio"), allow(dead_code))]
pub fn equal_power(level: f32) -> f32 {
    (level.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2).sin()
}

/// Crossfade progress 0..=1 at `pos_ms` into a crossfade of `len_ms` that
/// started at `start_ms`. A zero-length crossfade is complete immediately.
pub fn crossfade_progress(start_ms: f64, len_ms: f64, pos_ms: f64) -> f32 {
    if len_ms <= 0.0 {
        return 1.0;
    }
    ((pos_ms - start_ms) / len_ms).clamp(0.0, 1.0) as f32
}

/// When the next item should start so its start overlaps the last
/// `crossfade_ms` of an item lasting `duration_ms` (never before the start).
pub fn crossfade_start(duration_ms: u64, crossfade_ms: u64) -> u64 {
    duration_ms.saturating_sub(crossfade_ms.min(duration_ms / 2))
}

/// Linear blend of two RGB buffers: `out = a·(1-t) + b·t` (lengths may differ;
/// missing bytes count as black).
pub fn blend(a: &[u8], b: &[u8], t: f32, out: &mut [u8]) {
    let t = (t.clamp(0.0, 1.0) * 256.0) as u32;
    let s = 256 - t;
    for (i, o) in out.iter_mut().enumerate() {
        let x = a.get(i).copied().unwrap_or(0) as u32;
        let y = b.get(i).copied().unwrap_or(0) as u32;
        *o = ((x * s + y * t) >> 8) as u8;
    }
}

/// Scale every byte by `level` (0..=1) in place.
pub fn scale(buf: &mut [u8], level: f32) {
    let l = (level.clamp(0.0, 1.0) * 256.0) as u32;
    if l >= 256 {
        return;
    }
    for b in buf {
        *b = ((*b as u32 * l) >> 8) as u8;
    }
}

/// A linear fade over `len_ms` from `from` to `to`.
#[derive(Debug, Clone, Copy)]
pub struct Fade {
    pub start_ms: f64,
    pub len_ms: f64,
    pub from: f32,
    pub to: f32,
}

impl Fade {
    pub fn new(start_ms: f64, len_ms: f64, from: f32, to: f32) -> Self {
        Fade {
            start_ms,
            len_ms,
            from,
            to,
        }
    }

    pub fn level(&self, now_ms: f64) -> f32 {
        let p = crossfade_progress(self.start_ms, self.len_ms, now_ms);
        self.from + (self.to - self.from) * p
    }

    pub fn done(&self, now_ms: f64) -> bool {
        now_ms >= self.start_ms + self.len_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_clock_pause_seek() {
        let mut c = MonoClock::new(0.0, 1000.0);
        assert_eq!(c.pos(1500.0), 500.0);
        c.set_paused(true, 1500.0);
        assert_eq!(c.pos(9000.0), 500.0);
        c.set_paused(false, 9000.0);
        assert_eq!(c.pos(9100.0), 600.0);
        c.seek(20_000.0, 9100.0);
        assert_eq!(c.pos(9200.0), 20_100.0);
        // Time going backwards never moves the clock backwards.
        assert_eq!(c.pos(9000.0), 20_000.0);
    }

    #[test]
    fn servo_follows_without_deadband() {
        // Target runs 40 ppm fast and starts 3 ms ahead: the servo converges
        // to well under 0.5 ms and stays there (no ±1 frame sawtooth).
        let mut c = Servo::new(0.0, 0.0, ServoGains::FOLLOWER);
        let r = 1.0 + 40e-6;
        let target = |t: f64| 3.0 + t * r;
        let mut worst_late = 0.0f64;
        let mut t = 0.0;
        while t < 600_000.0 {
            let a = c.update(target(t), Some(r), Some(1), t);
            if t > 0.0 {
                assert!(matches!(a, SyncAction::Slew(_)), "{a:?} at {t}");
            }
            if t > 3_000.0 {
                worst_late = worst_late.max((c.pos_at(t + 12.0) - target(t + 12.0)).abs());
            }
            t += 25.0;
        }
        assert!(worst_late < 0.05, "tracking error {worst_late} ms");
        // Slewing is bounded.
        let mut c = Servo::new(0.0, 0.0, ServoGains::FOLLOWER);
        match c.update(90.0, Some(1.0), None, 25.0) {
            SyncAction::Slew(r) => assert!((r - (1.0 + MAX_SLEW)).abs() < 1e-12),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn servo_jumps_on_large_errors_and_new_epochs() {
        let mut c = Servo::new(0.0, 0.0, ServoGains::FOLLOWER);
        assert_eq!(c.update(100.0, Some(1.0), Some(1), 100.0), SyncAction::Jump);
        assert!(matches!(
            c.update(200.5, Some(1.0), Some(1), 200.0),
            SyncAction::Slew(_)
        ));
        // Seek: small error but a new epoch.
        assert_eq!(c.update(260.0, Some(1.0), Some(2), 250.0), SyncAction::Jump);
        assert_eq!(c.pos(), 260.0);
        // 150 ms off: jump.
        assert_eq!(c.update(450.0, Some(1.0), Some(2), 300.0), SyncAction::Jump);
        // Paused: held on the target.
        c.set_running(false, 300.0);
        assert_eq!(
            c.update(451.0, Some(0.0), Some(2), 1000.0),
            SyncAction::Hold
        );
        assert_eq!(c.pos_at(5_000.0), 451.0);
        assert_eq!(c.rate(), 0.0);
        c.set_running(true, 1000.0);
        c.update(451.0, Some(1.0), Some(2), 1000.0);
        assert!((c.pos_at(1100.0) - 551.0).abs() < 0.1);
    }

    #[test]
    fn audio_servo_learns_the_rate_and_filters_jitter() {
        // A sound card 120 ppm slow whose reported position is off by up to
        // ±5 ms (the Pi headphone jack's delay moves in 10 ms steps) plus
        // ±2 ms of scheduling noise.
        let mut c = Servo::new(0.0, 0.0, ServoGains::AUDIO);
        let rate = 1.0 - 120e-6;
        let mut seed = 1u64;
        let mut uniform = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
        };
        let (mut worst, mut sum, mut n) = (0.0f64, 0.0, 0.0);
        let mut t = 0.0;
        while t < 300_000.0 {
            let truth: f64 = t * rate;
            let reported = truth + 5.0 * uniform() + 2.0 * uniform();
            c.update(reported, None, Some(1), t);
            if t > 60_000.0 {
                let e = (c.pos() - truth).abs();
                worst = worst.max(e);
                sum += e;
                n += 1.0;
            }
            t += 25.0;
        }
        assert!(worst < 1.5, "worst {worst} ms against ±7 ms of noise");
        assert!(sum / n < 0.4, "mean {} ms", sum / n);
        assert!((c.freq() - rate).abs() < 30e-6, "rate {}", c.freq());
    }

    #[test]
    fn slot_centred_frame_choice() {
        // 25 ms frames, a 10 ms refresh slot starting at 99 ms: the slot's
        // middle (104 ms) is in frame 4.
        assert_eq!(frame_for_slot(99.0, 10.0, 25.0), 4);
        assert_eq!(frame_for_slot(94.0, 10.0, 25.0), 3);
        assert_eq!(frame_for_slot(95.0, 10.0, 25.0), 4);
        assert_eq!(frame_for_slot(-30.0, 10.0, 25.0), 0);
        assert_eq!(frame_for_slot(10.0, 0.0, 0.0), 0);
        // Long slots (R > F) show the frame at the middle of the slot.
        assert_eq!(frame_for_slot(100.0, 50.0, 25.0), 5);
    }

    #[test]
    fn next_update_lands_on_frame_boundaries() {
        // Frame 3 shown at pos 80 (now 1000); frame 4 starts at 100 ms. With
        // a 10 ms slot the update should light up at pos 95 → 15 ms from now.
        let t = next_update(80.0, 1.0, 1000.0, 3, 25.0, 10.0).unwrap();
        assert!((t - 1015.0).abs() < 1e-9);
        assert_eq!(frame_for_slot(95.0, 10.0, 25.0), 4);
        // At a slower rate it takes longer.
        let t = next_update(80.0, 0.5, 1000.0, 3, 25.0, 10.0).unwrap();
        assert!((t - 1030.0).abs() < 1e-9);
        assert_eq!(next_update(80.0, 0.0, 1000.0, 3, 25.0, 10.0), None);
        // Error of a node presenting on a free-running 12.6 ms vblank grid:
        // every frame change lands within ±R/2 of its ideal instant.
        let r = 12.6;
        let frame = 25.0;
        let mut worst = 0.0f64;
        let mut shown = 0;
        let mut v = 3.3; // first vblank
        let mut next = next_update(0.0, 1.0, 0.0, shown, frame, r).unwrap();
        while v < 60_000.0 {
            if v + 1e-9 >= next {
                let idx = frame_for_slot(v, r, frame);
                if idx != shown {
                    // Frame idx should appear at idx·F; it appears at v.
                    assert_eq!(idx, shown + 1, "no frame skipped");
                    worst = worst.max((v - idx as f64 * frame).abs());
                    shown = idx;
                }
                next = next_update(v, 1.0, v, shown, frame, r).unwrap();
            }
            v += r;
        }
        assert!(worst <= r / 2.0 + 1e-9);
    }

    #[test]
    fn crossfade_math() {
        assert_eq!(crossfade_start(10_000, 2_000), 8_000);
        // Crossfade never longer than half the item.
        assert_eq!(crossfade_start(3_000, 2_000), 1_500);
        assert_eq!(crossfade_start(10_000, 0), 10_000);
        assert_eq!(crossfade_progress(8_000.0, 2_000.0, 9_000.0), 0.5);
        assert_eq!(crossfade_progress(8_000.0, 2_000.0, 7_000.0), 0.0);
        assert_eq!(crossfade_progress(8_000.0, 2_000.0, 11_000.0), 1.0);
        assert_eq!(crossfade_progress(8_000.0, 0.0, 0.0), 1.0);
        // Equal power: fade-in² + fade-out² = 1 along the whole fade.
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let p = equal_power(t).powi(2) + equal_power(1.0 - t).powi(2);
            assert!((p - 1.0).abs() < 1e-5);
        }
        assert_eq!(equal_power(0.0), 0.0);
        assert!((equal_power(1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn blend_and_scale() {
        let a = [200u8, 0, 100];
        let b = [0u8, 200, 100];
        let mut out = [0u8; 3];
        blend(&a, &b, 0.0, &mut out);
        assert_eq!(out, a);
        blend(&a, &b, 1.0, &mut out);
        assert_eq!(out, b);
        blend(&a, &b, 0.5, &mut out);
        assert_eq!(out, [100, 100, 100]);
        let mut buf = [255u8, 128, 0];
        scale(&mut buf, 0.5);
        assert_eq!(buf, [127, 64, 0]);
        scale(&mut buf, 0.0);
        assert_eq!(buf, [0, 0, 0]);
        let f = Fade::new(1000.0, 1000.0, 1.0, 0.0);
        assert_eq!(f.level(1500.0), 0.5);
        assert!(f.done(2000.0) && !f.done(1999.0));
    }
}
