//! Leader clock estimation on followers (ARCHITECTURE §7.4).
//!
//! ## Exchange (4 timestamps)
//!
//! ```text
//! ping : follower → leader  { t0 }            t0 = follower clock just before sending
//! pong : leader → follower  { t0, t1, t2 }    t1 = leader kernel receive stamp of the ping
//!                                             t2 = leader clock just before sending the pong
//!                                             t3 = follower kernel receive stamp of the pong
//! delay  = (t3 − t0) − (t2 − t1)              network round trip, leader processing excluded
//! offset = ((t1 − t0) + (t2 − t3)) / 2        leader clock − follower clock
//! ```
//!
//! Pings go out in bursts (5 pings 20 ms apart every 2 s; 16 pings 10 ms apart
//! right after joining, a leader restart or a detected step), which raises the
//! chance of catching an uncontended moment on Wi-Fi.
//!
//! ## Model
//!
//! [`ClockModel`] keeps the last [`WINDOW_MS`] of samples and fits
//! `offset(t) = a + b·(t − t_ref)` — offset *and* drift (`b`, the relative
//! frequency of the two monotonic clocks) — by weighted least squares over
//! the good samples only:
//!
//! 1. from every burst, and then from every 4 s bin, only its lowest-delay
//!    sample (so the fit sees the whole window evenly);
//! 2. only samples whose delay is within `3 · max(0.3 ms, ½(p30 − min))` of
//!    the window's minimum delay (queueing only ever *adds* delay, and Wi-Fi
//!    delay is heavy-tailed: the low quantile is stable, the mean is not);
//! 3. weights `1 / (delay − min + 0.1 ms)²`; one robust pass drops residual
//!    outliers (> 3·MAD) and refits.
//!
//! The drift estimate is regularised towards the previous one (0 at first,
//! σ = 200 ppm), so it starts cautiously from the first seconds and is
//! data-driven once the samples span ~10 s; it is clamped to ±1000 ppm. When pongs stop
//! arriving the fit simply extrapolates ("holdover": at 20 ppm residual drift
//! a minute costs 1.2 ms).
//!
//! A leader restart (new boot id, or an offset jump of more than 50 ms) resets
//! the model at once; smaller steps (a clock slewed by NTP) are detected when
//! good samples disagree with the fit by more than `5·rms + 1 ms` in three
//! consecutive bursts, and the fit restarts from those samples.
//!
//! The error bound reported to the UI is `min_delay/2 + rms`: half the best
//! round trip bounds the (unmeasurable) path asymmetry, the residual RMS is
//! the noise of the fit.

use std::collections::VecDeque;

/// Samples older than this (relative to the newest) are dropped.
pub const WINDOW_MS: f64 = 90_000.0;
/// Round trips longer than this are discarded outright.
pub const MAX_DELAY_MS: f64 = 1_000.0;
/// An offset jump larger than this (plus the delays) means the leader
/// restarted or a clock was stepped: start over immediately.
pub const JUMP_MS: f64 = 50.0;
/// Largest believable drift between two monotonic clocks.
pub const MAX_SKEW: f64 = 1e-3;
/// Prior uncertainty of the drift (the regression is regularised towards the
/// previous estimate, 0 at first): with samples spanning only a second or two
/// the prior dominates, after ~10 s the data does.
const SKEW_PRIOR_SIGMA: f64 = 200e-6;
/// Good samples needed after a reset before normal (slow) bursts resume.
const FAST_START_SAMPLES: usize = 5;
/// Bursts in a row whose good samples disagree with the fit before it restarts.
const STEP_BURSTS: usize = 3;
/// The fit uses the best sample of each bin of this length.
const BIN_MS: f64 = 4_000.0;
/// Samples up to this many "good" margins above the minimum delay enter the
/// fit (with steeply falling weights); the step detector uses one margin.
const FIT_LIMIT_FACTOR: f64 = 3.0;
/// Keep at most this many samples (bursts every 2 s over the window ≈ 250).
const MAX_SAMPLES: usize = 1024;

/// One ping/pong exchange.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// Follower clock at the middle of the exchange (ms).
    pub t: f64,
    /// Leader − follower clock (ms).
    pub offset: f64,
    /// Round trip without the leader's processing time (ms).
    pub delay: f64,
    /// Burst the ping belonged to.
    pub burst: u32,
    /// Inconsistent with the fit by more than its delay can explain: kept
    /// out of the fit (an outlier, or evidence of a clock step).
    pub quarantined: bool,
}

/// The current fit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    /// Offset at `t_ref` (ms).
    pub a: f64,
    /// Drift (leader ms per follower ms − 1).
    pub b: f64,
    pub t_ref: f64,
    /// Weighted RMS of the residuals of the samples used (ms).
    pub rms: f64,
    /// Smallest delay in the window (ms).
    pub min_delay: f64,
    /// Samples used.
    pub used: usize,
    /// Standard deviation of the drift estimate.
    pub b_sigma: f64,
}

impl Fit {
    pub fn offset_at(&self, t: f64) -> f64 {
        self.a + self.b * (t - self.t_ref)
    }
}

/// Round-trip statistics over the window.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DelayStats {
    pub min: f64,
    pub p50: f64,
    pub p95: f64,
    pub samples: usize,
}

#[derive(Debug, Clone)]
pub struct ClockModel {
    samples: VecDeque<Sample>,
    fit: Option<Fit>,
    boot: Option<String>,
    /// Bursts whose good samples disagreed with the fit (step detection).
    suspects: Vec<u32>,
    /// Good samples accepted since the last reset.
    since_reset: usize,
    /// Drift of the previous fit (kept across restarts of the fit).
    last_skew: f64,
    /// Local time of the newest sample.
    newest: Option<f64>,
}

impl Default for ClockModel {
    fn default() -> Self {
        ClockModel {
            samples: VecDeque::new(),
            fit: None,
            boot: None,
            suspects: Vec::new(),
            since_reset: 0,
            last_skew: 0.0,
            newest: None,
        }
    }
}

impl ClockModel {
    /// Forget everything (new leader, leader restarted, clock jumped).
    pub fn reset(&mut self) {
        self.samples.clear();
        self.fit = None;
        self.boot = None;
        self.suspects.clear();
        self.since_reset = 0;
        self.newest = None;
    }

    /// Drop the samples (a restart of the fit) but keep the boot id and the
    /// last drift estimate.
    fn restart_fit(&mut self) {
        self.samples.clear();
        self.fit = None;
        self.suspects.clear();
        self.since_reset = 0;
    }

    /// Record the leader's boot id; a different one resets the model.
    /// Returns `true` when the leader restarted.
    pub fn observe_boot(&mut self, boot: &str) -> bool {
        if boot.is_empty() || self.boot.as_deref() == Some(boot) {
            return false;
        }
        let restarted = self.boot.is_some();
        if restarted {
            self.restart_fit();
            self.last_skew = 0.0;
        }
        self.boot = Some(boot.to_string());
        restarted
    }

    /// Add an exchange: `t0`/`t3` follower clock, `t1`/`t2` leader clock (ms).
    /// Old leaders answer without `t2`: pass `t2 = t1`. Returns the sample
    /// if it was plausible.
    pub fn add(&mut self, t0: f64, t1: f64, t2: f64, t3: f64, burst: u32) -> Option<Sample> {
        if ![t0, t1, t2, t3].iter().all(|v| v.is_finite()) {
            return None;
        }
        let processing = (t2 - t1).max(0.0);
        let delay = (t3 - t0) - processing;
        // A tiny negative delay is timestamp resolution; more is nonsense.
        if !(-0.05..=MAX_DELAY_MS).contains(&delay) || t3 < t0 {
            return None;
        }
        let delay = delay.max(0.0);
        let mut sample = Sample {
            t: (t0 + t3) / 2.0,
            offset: ((t1 - t0) + (t2 - t3)) / 2.0,
            delay,
            burst,
            quarantined: false,
        };
        if let Some(fit) = self.fit {
            let dev = sample.offset - fit.offset_at(sample.t);
            if dev.abs() > JUMP_MS + fit.min_delay + sample.delay {
                // The leader restarted (its clock starts at 0 again) or a clock
                // was stepped: a single sample must win at once.
                self.restart_fit();
            } else {
                sample.quarantined = self.check_step(&sample, dev, &fit);
            }
        }
        self.samples.push_back(sample);
        self.newest = Some(self.newest.map_or(sample.t, |n| n.max(sample.t)));
        let newest = self.newest.unwrap_or(sample.t);
        while self.samples.len() > MAX_SAMPLES
            || self
                .samples
                .front()
                .is_some_and(|s| newest - s.t > WINDOW_MS)
        {
            self.samples.pop_front();
        }
        self.refit();
        if self.fit.is_some_and(|f| {
            sample.delay <= f.min_delay + threshold_extra(&self.samples, f.min_delay)
        }) {
            self.since_reset += 1;
        }
        Some(sample)
    }

    /// Is the sample consistent with the fit? Queueing can shift a sample's
    /// offset by at most half its excess delay, so a sample further off than
    /// that (plus the fit's own noise and drift uncertainty) is quarantined.
    /// Low-delay samples off the fit in [`STEP_BURSTS`] bursts in a row mean
    /// the clock stepped: the fit restarts from them. Returns "quarantine".
    fn check_step(&mut self, s: &Sample, dev: f64, fit: &Fit) -> bool {
        // A new lowest-delay sample may legitimately correct the fit by half
        // the delay it saves.
        let excess = (s.delay - fit.min_delay).abs();
        let allowed =
            excess / 2.0 + 5.0 * fit.rms + 0.5 + 3.0 * fit.b_sigma * (s.t - fit.t_ref).abs();
        let good = s.delay - fit.min_delay <= threshold_extra(&self.samples, fit.min_delay);
        if dev.abs() <= allowed {
            if good && !self.suspects.is_empty() && !self.suspects.contains(&s.burst) {
                self.suspects.clear();
            }
            return false;
        }
        if good && !self.suspects.contains(&s.burst) {
            self.suspects.push(s.burst);
        }
        if self.suspects.len() >= STEP_BURSTS {
            // Keep the evidence (every sample since the first suspect burst
            // that disagrees with the old fit), drop the rest.
            let keep = std::mem::take(&mut self.suspects);
            let since = self
                .samples
                .iter()
                .find(|x| keep.contains(&x.burst))
                .map_or(s.t, |x| x.t);
            self.samples.retain(|x| x.quarantined && x.t >= since);
            for x in self.samples.iter_mut() {
                x.quarantined = false;
            }
            self.fit = None;
            self.since_reset = 0;
            return false;
        }
        true
    }

    fn refit(&mut self) {
        if let Some(f) = fit(&self.samples, self.last_skew) {
            if f.used >= 3 {
                self.last_skew = f.b;
            }
            self.fit = Some(f);
        } else {
            self.fit = None;
        }
    }

    /// The current fit, if any sample is known.
    pub fn fit(&self) -> Option<Fit> {
        self.fit
    }

    /// Estimated `leader − local` clock offset at local time `t` (ms).
    pub fn offset_at(&self, t: f64) -> Option<f64> {
        self.fit.map(|f| f.offset_at(t))
    }

    /// Leader clock at local time `t`.
    #[cfg(test)]
    pub fn to_leader(&self, t: f64) -> Option<f64> {
        self.offset_at(t).map(|o| t + o)
    }

    /// Local time at which the leader clock reads `t_leader`.
    pub fn to_local(&self, t_leader: f64) -> Option<f64> {
        let f = self.fit?;
        // t + a + b (t − t_ref) = L  ⇒  t = (L − a + b·t_ref) / (1 + b)
        Some((t_leader - f.a + f.b * f.t_ref) / (1.0 + f.b))
    }

    /// Leader ms per local ms − 1, in ppm.
    pub fn skew_ppm(&self) -> Option<f64> {
        self.fit.map(|f| f.b * 1e6)
    }

    /// Error bound of the estimate: half the best round trip (path
    /// asymmetry) plus the residual RMS (ms).
    pub fn uncertainty_ms(&self) -> Option<f64> {
        self.fit.map(|f| f.min_delay / 2.0 + f.rms)
    }

    /// Residual RMS of the fit (ms).
    pub fn jitter_ms(&self) -> Option<f64> {
        self.fit.map(|f| f.rms)
    }

    /// Round-trip statistics over the window.
    pub fn delay_stats(&self) -> DelayStats {
        let mut d: Vec<f64> = self.samples.iter().map(|s| s.delay).collect();
        if d.is_empty() {
            return DelayStats::default();
        }
        d.sort_by(f64::total_cmp);
        let q = |p: f64| d[((d.len() - 1) as f64 * p).round() as usize];
        DelayStats {
            min: d[0],
            p50: q(0.5),
            p95: q(0.95),
            samples: d.len(),
        }
    }

    /// Too few good samples since the last reset: ping in a fast burst.
    pub fn needs_burst(&self) -> bool {
        self.since_reset < FAST_START_SAMPLES
    }

    /// Enough good samples to trust the estimate.
    pub fn converged(&self) -> bool {
        self.fit.is_some() && self.since_reset >= 3
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.samples.len()
    }
}

/// How much above the minimum delay a sample may be and still count as good.
fn threshold_extra(samples: &VecDeque<Sample>, min: f64) -> f64 {
    let mut d: Vec<f64> = samples.iter().map(|s| s.delay).collect();
    if d.is_empty() {
        return 0.3;
    }
    d.sort_by(f64::total_cmp);
    let p30 = d[((d.len() - 1) as f64 * 0.3).round() as usize];
    (0.5 * (p30 - min)).max(0.3)
}

/// Fit offset + drift to the good samples (see the module docs).
fn fit(samples: &VecDeque<Sample>, prior_skew: f64) -> Option<Fit> {
    // Best sample of every burst.
    let mut best: Vec<Sample> = Vec::new();
    for s in samples.iter().filter(|s| !s.quarantined) {
        match best.iter_mut().find(|b| b.burst == s.burst) {
            Some(b) if s.delay < b.delay => *b = *s,
            Some(_) => {}
            None => best.push(*s),
        }
    }
    // …and of every time bin, so the fit sees the whole window evenly (the
    // drift is only as good as the time span of the samples it rests on).
    let bin_ms = BIN_MS;
    let mut bins: Vec<Sample> = Vec::new();
    for s in best {
        let bin = (s.t / bin_ms).floor();
        match bins.iter_mut().find(|b| (b.t / bin_ms).floor() == bin) {
            Some(b) if s.delay < b.delay => *b = s,
            Some(_) => {}
            None => bins.push(s),
        }
    }
    let best = bins;
    let min_delay = best.iter().map(|s| s.delay).fold(f64::INFINITY, f64::min);
    if !min_delay.is_finite() {
        return None;
    }
    let limit = min_delay + threshold_extra(samples, min_delay) * FIT_LIMIT_FACTOR;
    let mut chosen: Vec<(Sample, f64)> = best
        .into_iter()
        .filter(|s| s.delay <= limit)
        .map(|s| {
            let w = 1.0 / (s.delay - min_delay + 0.1).powi(2);
            (s, w)
        })
        .collect();
    let mut result = wls(&chosen, prior_skew, min_delay)?;
    // One robust pass: drop residual outliers, refit.
    if chosen.len() >= 5 {
        let mut res: Vec<f64> = chosen
            .iter()
            .map(|(s, _)| (s.offset - result.offset_at(s.t)).abs())
            .collect();
        res.sort_by(f64::total_cmp);
        let mad = res[res.len() / 2];
        let cut = (3.0 * 1.4826 * mad).max(0.2);
        let before = chosen.len();
        chosen.retain(|(s, _)| (s.offset - result.offset_at(s.t)).abs() <= cut);
        if chosen.len() != before && !chosen.is_empty() {
            result = wls(&chosen, prior_skew, min_delay)?;
        }
    }
    Some(result)
}

fn wls(chosen: &[(Sample, f64)], prior_skew: f64, min_delay: f64) -> Option<Fit> {
    let sw: f64 = chosen.iter().map(|(_, w)| w).sum();
    if chosen.is_empty() || sw <= 0.0 {
        return None;
    }
    let t_ref = chosen.iter().map(|(s, w)| s.t * w).sum::<f64>() / sw;
    let o_mean = chosen.iter().map(|(s, w)| s.offset * w).sum::<f64>() / sw;
    let sxx: f64 = chosen.iter().map(|(s, w)| w * (s.t - t_ref).powi(2)).sum();
    let sxy: f64 = chosen
        .iter()
        .map(|(s, w)| w * (s.t - t_ref) * (s.offset - o_mean))
        .sum();
    let lambda = 1.0 / (SKEW_PRIOR_SIGMA * SKEW_PRIOR_SIGMA);
    let b = ((sxy + lambda * prior_skew) / (sxx + lambda)).clamp(-MAX_SKEW, MAX_SKEW);
    let b_sigma = (1.0 / (sxx + lambda)).sqrt();
    // The weighted mean offset is the fit's value at t_ref.
    let a = o_mean;
    let f = Fit {
        a,
        b,
        t_ref,
        rms: 0.0,
        min_delay,
        used: chosen.len(),
        b_sigma,
    };
    let rms = (chosen
        .iter()
        .map(|(s, w)| w * (s.offset - f.offset_at(s.t)).powi(2))
        .sum::<f64>()
        / sw)
        .sqrt();
    Some(Fit { rms, ..f })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Deterministic xorshift PRNG for the simulations.
    pub struct Rng(pub u64);
    impl Rng {
        pub fn uniform(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
        /// Exponentially distributed with this mean.
        pub fn exp(&mut self, mean: f64) -> f64 {
            -mean * (1.0 - self.uniform()).ln()
        }
    }

    /// A network path: minimum one-way delays plus heavy-tailed queueing.
    #[derive(Clone, Copy)]
    pub struct Path {
        pub up_min: f64,
        pub down_min: f64,
        pub up_jitter: f64,
        pub down_jitter: f64,
        /// Probability of a 20–200 ms spike (power save, retries).
        pub spike: f64,
        pub loss: f64,
    }

    impl Path {
        pub const LOOPBACK: Path = Path {
            up_min: 0.03,
            down_min: 0.03,
            up_jitter: 0.02,
            down_jitter: 0.02,
            spike: 0.0,
            loss: 0.0,
        };
        pub const WIFI: Path = Path {
            up_min: 1.2,
            down_min: 1.2,
            up_jitter: 2.0,
            down_jitter: 1.0,
            spike: 0.03,
            loss: 0.05,
        };
        /// Busy 2.4 GHz: more jitter, asymmetric queueing, 15 % loss.
        pub const BAD_WIFI: Path = Path {
            up_min: 2.0,
            down_min: 2.0,
            up_jitter: 6.0,
            down_jitter: 2.5,
            spike: 0.08,
            loss: 0.15,
        };

        /// One-way delays of one exchange, or `None` when lost.
        pub fn sample(&self, rng: &mut Rng) -> Option<(f64, f64)> {
            if rng.uniform() < self.loss {
                return None;
            }
            let mut up = self.up_min + rng.exp(self.up_jitter);
            let mut down = self.down_min + rng.exp(self.down_jitter);
            if rng.uniform() < self.spike {
                up += 20.0 + rng.uniform() * 180.0;
            }
            if rng.uniform() < self.spike {
                down += 20.0 + rng.uniform() * 180.0;
            }
            Some((up, down))
        }
    }

    /// Two clocks: the leader's is "true" time `T`; the follower reads
    /// `(T − start) · (1 + drift) + 100 000` plus a step at `step_at`.
    #[derive(Clone, Copy)]
    pub struct Clocks {
        pub start: f64,
        pub drift_ppm: f64,
        pub step: Option<(f64, f64)>,
    }

    impl Clocks {
        pub fn local(&self, t: f64) -> f64 {
            let mut l = (t - self.start) * (1.0 + self.drift_ppm * 1e-6) + 100_000.0;
            if let Some((at, by)) = self.step {
                if t >= at {
                    l += by;
                }
            }
            l
        }
        /// The true leader − local offset at local time `l` (inverse of `local`).
        pub fn true_offset_at_local(&self, l: f64) -> f64 {
            let mut x = l;
            if let Some((at, by)) = self.step {
                if l >= self.local(at) {
                    x -= by;
                }
            }
            let t = (x - 100_000.0) / (1.0 + self.drift_ppm * 1e-6) + self.start;
            t - l
        }
    }

    /// Run the ping schedule (bursts of 5 every 2 s, 16 at 10 ms while
    /// `needs_burst`) for `secs`, calling `check(model, true_time)` every 100 ms.
    pub fn simulate(
        model: &mut ClockModel,
        clocks: Clocks,
        path: Path,
        rng: &mut Rng,
        from: f64,
        secs: f64,
        mut check: impl FnMut(&ClockModel, f64),
    ) {
        let mut t = from;
        let end = from + secs * 1000.0;
        let mut burst = (from / 1000.0) as u32 * 7;
        let mut next_check = t;
        while t < end {
            let fast = model.needs_burst();
            let (n, gap) = if fast { (16, 10.0) } else { (5, 20.0) };
            burst += 1;
            for i in 0..n {
                let ts = t + i as f64 * gap;
                // Leader processing between receive and reply: 0.1–3 ms.
                let proc = 0.1 + rng.uniform() * 3.0;
                if let Some((up, down)) = path.sample(rng) {
                    let t0 = clocks.local(ts);
                    let t1 = ts + up;
                    let t2 = t1 + proc;
                    let t3 = clocks.local(t2 + down);
                    model.add(t0, t1, t2, t3, burst);
                }
            }
            let period = if fast { 200.0 } else { 2000.0 };
            while next_check < t + period && next_check < end {
                check(model, next_check);
                next_check += 100.0;
            }
            t += period;
        }
    }

    fn err(model: &ClockModel, clocks: &Clocks, t: f64) -> f64 {
        let l = clocks.local(t);
        model.offset_at(l).expect("estimate") - clocks.true_offset_at_local(l)
    }

    /// Convergence and 30-minute tracking for a range of clocks and networks.
    #[test]
    fn converges_and_tracks_for_30_minutes() {
        // (name, network, drift, worst error allowed over 30 minutes)
        let cases = [
            ("loopback", Path::LOOPBACK, 0.0, 0.1),
            ("wifi +45 ppm", Path::WIFI, 45.0, 0.5),
            ("wifi −80 ppm", Path::WIFI, -80.0, 0.5),
            // Busy 2.4 GHz: asymmetric queueing, 8 % spikes, 15 % loss.
            ("bad wifi +20 ppm", Path::BAD_WIFI, 20.0, 1.0),
        ];
        for (name, path, drift, bound) in cases {
            let clocks = Clocks {
                start: 3_000.0,
                drift_ppm: drift,
                step: None,
            };
            let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ drift.to_bits());
            let mut m = ClockModel::default();
            // Within 0.5 ms 3 s after joining (fast-start burst + first bursts).
            let mut first_good = None;
            simulate(&mut m, clocks, path, &mut rng, 5_000.0, 3.0, |m, t| {
                if first_good.is_none() && err(m, &clocks, t).abs() < 0.5 {
                    first_good = Some(t);
                }
            });
            let first_good = first_good.unwrap_or_else(|| panic!("{name}: no convergence in 3 s"));
            // Then 30 minutes of tracking: never worse than the bound.
            let mut worst = 0.0f64;
            let mut sum = 0.0;
            let mut n = 0.0;
            simulate(&mut m, clocks, path, &mut rng, 8_000.0, 1_800.0, |m, t| {
                let e = err(m, &clocks, t).abs();
                if e > 0.4 && std::env::var("PP_DEBUG_CLOCK").is_ok() {
                    eprintln!("{name} t={t:.0} err={e:.3} fit={:?} n={}", m.fit(), m.len());
                }
                worst = worst.max(e);
                sum += e;
                n += 1.0;
            });
            // A fast follower clock (+drift) sees the leader run slow.
            let skew_err = m.skew_ppm().unwrap() + drift / (1.0 + drift * 1e-6);
            eprintln!(
                "{name}: converged after {:.0} ms, 30 min worst {worst:.3} ms, mean {:.3} ms, \
                 drift error {skew_err:.2} ppm, bound ±{:.2} ms",
                first_good - 5_000.0,
                sum / n,
                m.uncertainty_ms().unwrap()
            );
            assert!(worst < bound, "{name}: worst {worst} ms");
            assert!(skew_err.abs() < 5.0, "{name}: drift error {skew_err} ppm");
            // The reported bound covers the actual error.
            assert!(m.uncertainty_ms().unwrap() >= sum / n, "{name}");
        }
    }

    /// Holdover: pongs stop for 60 s; the drift estimate keeps the error small.
    #[test]
    fn holdover_extrapolates_the_drift() {
        let clocks = Clocks {
            start: 0.0,
            drift_ppm: 60.0,
            step: None,
        };
        let mut rng = Rng(42);
        let mut m = ClockModel::default();
        simulate(
            &mut m,
            clocks,
            Path::WIFI,
            &mut rng,
            1_000.0,
            120.0,
            |_, _| {},
        );
        let at_loss = 121_000.0;
        let e0 = err(&m, &clocks, at_loss).abs();
        let e60 = err(&m, &clocks, at_loss + 60_000.0).abs();
        eprintln!("holdover: {e0:.3} ms at loss, {e60:.3} ms after 60 s");
        assert!(e60 < 1.5, "60 s holdover error {e60} ms");
        // Without a drift model the error would be 60 ppm × 60 s = 3.6 ms.
        assert!(e60 < 3.6 / 2.0);
    }

    /// An NTP-style step of the follower clock is detected and re-learnt.
    #[test]
    fn small_steps_are_detected() {
        let clocks = Clocks {
            start: 0.0,
            drift_ppm: 10.0,
            step: Some((40_000.0, 8.0)),
        };
        let mut rng = Rng(7);
        let mut m = ClockModel::default();
        simulate(
            &mut m,
            clocks,
            Path::WIFI,
            &mut rng,
            1_000.0,
            70.0,
            |m, t| {
                if std::env::var("PP_DEBUG_CLOCK").is_ok() && (t as u64) % 2000 == 0 {
                    eprintln!(
                        "step t={t} err={:.3} n={} fit={:?}",
                        err(m, &clocks, t),
                        m.len(),
                        m.fit()
                    );
                }
            },
        );
        let e = err(&m, &clocks, 71_000.0).abs();
        assert!(e < 0.5, "error {e} ms 30 s after an 8 ms step");
    }

    #[test]
    fn four_timestamps_exclude_leader_processing() {
        let mut m = ClockModel::default();
        // Local = leader − 500. Up 2 ms, leader holds the ping 30 ms, down 2 ms.
        let s = m.add(1000.0, 1502.0, 1532.0, 1034.0, 1).unwrap();
        assert_eq!(s.delay, 4.0);
        assert_eq!(s.offset, 500.0);
        assert_eq!(m.offset_at(2000.0), Some(500.0));
        assert_eq!(m.uncertainty_ms(), Some(2.0));
        // The 3-timestamp formula would have been off by 15 ms.
        let three = 1502.0 - (1000.0 + 1034.0) / 2.0;
        assert_eq!(three, 485.0);
        // Old leaders (t2 missing → t2 = t1) still work.
        let mut old = ClockModel::default();
        let s = old.add(0.0, 502.0, 502.0, 4.0, 1).unwrap();
        assert_eq!((s.delay, s.offset), (4.0, 500.0));
    }

    #[test]
    fn minimum_delay_samples_win() {
        let mut m = ClockModel::default();
        // Asymmetric queueing delays distort the estimate…
        m.add(0.0, -300.0 + 40.0, -300.0 + 40.0, 42.0, 1); // error +19
        m.add(1000.0, 701.0, 701.0, 1002.0, 2); // exact, delay 2
        m.add(2000.0, 1702.0, 1702.0, 2032.0, 3); // error −14
        assert_eq!(m.offset_at(2000.0), Some(-300.0));
        assert_eq!(m.fit().unwrap().min_delay, 2.0);
    }

    #[test]
    fn to_local_inverts_to_leader() {
        let clocks = Clocks {
            start: 0.0,
            drift_ppm: -250.0,
            step: None,
        };
        let mut rng = Rng(99);
        let mut m = ClockModel::default();
        simulate(
            &mut m,
            clocks,
            Path::LOOPBACK,
            &mut rng,
            1_000.0,
            40.0,
            |_, _| {},
        );
        for l in [50_000.0, 140_000.0, 1e6] {
            let back = m.to_local(m.to_leader(l).unwrap()).unwrap();
            assert!((back - l).abs() < 1e-6, "{l} → {back}");
        }
        assert!((m.skew_ppm().unwrap() - 250.0).abs() < 1.0);
    }

    #[test]
    fn rejects_nonsense_exchanges() {
        let mut m = ClockModel::default();
        assert!(m.add(100.0, 5.0, 5.0, 50.0, 1).is_none()); // negative delay
        assert!(m.add(0.0, 5.0, 5.0, 5_000.0, 1).is_none()); // way too slow
        assert!(m.add(0.0, f64::NAN, 1.0, 5.0, 1).is_none());
        // Leader processing longer than the round trip (clock nonsense).
        assert!(m.add(0.0, 1.0, 50.0, 5.0, 1).is_none());
        assert_eq!(m.offset_at(0.0), None);
        assert!(m.needs_burst());
    }

    #[test]
    fn leader_restart_resets_the_model() {
        let mut m = ClockModel::default();
        for i in 0..5 {
            let t = i as f64 * 1000.0;
            m.add(t, t + 90_001.0, t + 90_001.0, t + 2.0, i);
        }
        assert!(!m.needs_burst());
        // Leader restarted: its clock is ~0 now. A single slower sample must win
        // immediately instead of being out-voted by stale low-delay samples.
        m.add(6000.0, 13.0, 13.0, 6006.0, 9);
        assert_eq!(m.len(), 1);
        assert_eq!(m.offset_at(6003.0), Some(-5_990.0));
        assert!(m.needs_burst(), "fast burst after a restart");
    }

    #[test]
    fn boot_change_resets() {
        let mut m = ClockModel::default();
        assert!(!m.observe_boot("a"));
        m.add(0.0, 11.0, 11.0, 2.0, 1);
        assert!(!m.observe_boot("a"));
        assert_eq!(m.len(), 1);
        assert!(m.observe_boot("b"));
        assert_eq!(m.len(), 0);
        assert!(m.needs_burst());
    }
}
