//! Playback clocks and timing math.
//!
//! * [`MonoClock`]: a pausable, seekable monotonic position clock (used when no
//!   audio plays, and as the fallback when the audio device fails).
//! * [`SlewClock`]: follower position tracking with slew/jump correction
//!   (ARCHITECTURE §7.4: drift > 1 frame → slew, > 250 ms → jump).
//! * Crossfade / fade helpers shared by lights and audio.
//!
//! Everything takes explicit `now` values so it is tested with a fake clock.

/// Jump instead of slewing beyond this error (ms).
pub const JUMP_THRESHOLD_MS: f64 = 250.0;
/// Maximum slew rate deviation (±10 %).
pub const MAX_SLEW: f64 = 0.10;

/// Pausable monotonic position clock. Times are in ms on any monotonic base.
#[derive(Debug, Clone)]
pub struct MonoClock {
    base_pos: f64,
    base_at: f64,
    paused: bool,
}

impl MonoClock {
    pub fn new(pos_ms: f64, now_ms: f64) -> Self {
        MonoClock { base_pos: pos_ms, base_at: now_ms, paused: false }
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

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn seek(&mut self, pos_ms: f64, now_ms: f64) {
        self.base_pos = pos_ms;
        self.base_at = now_ms;
    }
}

/// What a sync update did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SyncAction {
    /// Within one frame: nothing to do.
    InSync,
    /// Correcting gradually with this rate (1.0 = real time).
    Slew(f64),
    /// Error too large: jumped to the target.
    Jump,
}

/// Follower playback clock that converges on the leader's position.
#[derive(Debug, Clone)]
pub struct SlewClock {
    pos: f64,
    last: f64,
    rate: f64,
    running: bool,
}

impl SlewClock {
    pub fn new(pos_ms: f64, now_ms: f64) -> Self {
        SlewClock { pos: pos_ms, last: now_ms, rate: 1.0, running: true }
    }

    /// Position now (advances the internal state).
    pub fn advance(&mut self, now_ms: f64) -> f64 {
        let dt = (now_ms - self.last).max(0.0);
        if self.running {
            self.pos += dt * self.rate;
        }
        self.last = now_ms;
        self.pos
    }

    pub fn pos(&self) -> f64 {
        self.pos
    }

    pub fn rate(&self) -> f64 {
        self.rate
    }

    pub fn set_running(&mut self, running: bool, now_ms: f64) {
        self.advance(now_ms);
        self.running = running;
        if !running {
            self.rate = 1.0;
        }
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Feed the leader's position `target_ms` valid at local time `now_ms`.
    /// `frame_ms` is the tolerance (one frame).
    pub fn update(&mut self, target_ms: f64, now_ms: f64, frame_ms: f64) -> SyncAction {
        let pos = self.advance(now_ms);
        let err = target_ms - pos;
        let tolerance = frame_ms.max(1.0);
        if err.abs() > JUMP_THRESHOLD_MS || !self.running {
            self.pos = target_ms;
            self.rate = 1.0;
            if err.abs() > JUMP_THRESHOLD_MS {
                return SyncAction::Jump;
            }
            return SyncAction::InSync;
        }
        if err.abs() > tolerance {
            // Remove the error over about one second, bounded to ±10 %.
            self.rate = 1.0 + (err / 1000.0).clamp(-MAX_SLEW, MAX_SLEW);
            SyncAction::Slew(self.rate)
        } else {
            self.rate = 1.0;
            SyncAction::InSync
        }
    }
}

/// Equal-power gain for a fade level in 0..=1 (sin curve). A fade-in at level
/// `t` and a fade-out at level `1 - t` always sum to constant power.
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
        Fade { start_ms, len_ms, from, to }
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
    fn slew_small_errors_jump_large() {
        let mut c = SlewClock::new(0.0, 0.0);
        // Perfect: in sync.
        assert_eq!(c.update(100.0, 100.0, 25.0), SyncAction::InSync);
        // 20 ms behind with a 25 ms frame: still in sync.
        assert_eq!(c.update(220.0, 200.0, 25.0), SyncAction::InSync);
        // 100 ms behind: slew faster.
        match c.update(400.0, 300.0, 25.0) {
            SyncAction::Slew(r) => assert!(r > 1.0 && r <= 1.0 + MAX_SLEW),
            other => panic!("expected slew, got {other:?}"),
        }
        // Converges: after a few seconds of updates the error is within a frame.
        let mut now = 300.0;
        let mut target = 400.0;
        for _ in 0..40 {
            now += 250.0;
            target += 250.0;
            c.update(target, now, 25.0);
        }
        assert!((c.advance(now) - target).abs() <= 25.0, "converged");
        // 1 s ahead of target: jump.
        assert_eq!(c.update(target, now, 25.0 ), SyncAction::InSync);
        assert_eq!(c.update(target + 1000.0, now, 25.0), SyncAction::Jump);
        assert_eq!(c.pos(), target + 1000.0);
        // Ahead by 100 ms: slew slower.
        match c.update(target + 900.0, now, 25.0) {
            SyncAction::Slew(r) => assert!(r < 1.0 && r >= 1.0 - MAX_SLEW),
            other => panic!("expected slew, got {other:?}"),
        }
    }

    #[test]
    fn slew_clock_hold() {
        let mut c = SlewClock::new(1000.0, 0.0);
        c.set_running(false, 100.0);
        assert_eq!(c.advance(5000.0), 1100.0);
        c.set_running(true, 5000.0);
        assert_eq!(c.advance(5100.0), 1200.0);
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
