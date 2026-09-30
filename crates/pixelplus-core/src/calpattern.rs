//! Calibration pattern v2 (F1, "Measure with my phone").
//!
//! A pseudo-random, non-periodic train of *events*. At every event the leader
//! plays a short chirp ("click") on its audio output **and** every prop, on
//! every controller, flashes full white. A phone at the street records both
//! the sound and the lights and correlates each stream against this known
//! schedule; the difference between the two offsets is how far the sound is
//! behind the lights (see docs/ARCHITECTURE.md §12.1).
//!
//! Everything here is a pure function of `(seed, position)`, so followers
//! render exactly the same flashes as the leader from the seed carried in the
//! sync packet (`TestRequest.cal = CalPattern { seed, v: 2 }`).
//!
//! # Timeline (one cycle of [`RUN_MS`], repeated if the engine loops it)
//!
//! ```text
//! pos 0 ........ 2000 ms dark ........ | ev0 | gap | ev1 | gap | ... | evN |  dark tail  | (wrap)
//!                                        ^ chirp starts exactly at the event time,
//!                                          and the flash turns on at the same instant
//! ```
//!
//! * Gaps come from a 32-bit Galois LFSR: `gap = 450 + (bits % 8) × 60 ms`
//!   (450…870 ms), so 32 events take about 21 s and no two stretches of the
//!   train look alike.
//! * Flash length is `max(80 ms, 2 presentation slots)` ([`flash_len_ms`]), so
//!   a 15 fps phone camera still sees at least one fully lit frame.
//! * Audio: an 8 ms linear chirp 2 → 4 kHz ([`chirp`]) with a fast raised
//!   cosine attack, written as a mono 16-bit 24 kHz WAV ([`wav`]); the phone
//!   uses the same chirp as its matched filter (`web/src/lib/sensing/`).
//!
//! # Engine wiring (WS3)
//!
//! ```ignore
//! use pixelplus_core::calpattern;
//! let seed = calpattern::seed_from(rand_u64);          // never 0
//! let plan = calpattern::schedule(seed, slot_ms);      // cache for the run
//! let wav  = calpattern::wav(seed);                    // -> cache/<calpattern::wav_file_name(seed)>
//! // every frame (leader and followers):
//! let lit = plan.flash_on(pos_ms);                     // or flash_on_v2(pos_ms, seed, slot_ms)
//! // POST /player/calibration {on:true, pattern:"v2"} replies with `plan`
//! // (serialises as {seed, eventsMs, flashMs, windowMs, leadInMs, chirp}) plus `startsInMs`.
//! ```
//!
//! [`schedule`]'s `events_ms` are relative to pattern position 0 (the start of
//! the calibration item's timeline, i.e. the first sample of the WAV).

use serde::{Deserialize, Serialize};

/// Pattern version this module implements (`CalPattern.v`).
pub const VERSION: u8 = 2;
/// Length of one cycle of the pattern and of the WAV (the engine's run length).
pub const RUN_MS: u64 = 60_000;
/// All-dark, silent lead-in before the first event of each cycle.
pub const LEAD_IN_MS: u32 = 2_000;
/// Quiet tail kept free of events at the end of a cycle (so a loop restart
/// never cuts a flash or chirp short).
pub const TAIL_MS: u32 = 1_000;
/// Shortest gap between events.
pub const GAP_MIN_MS: u32 = 450;
/// Gap step: `gap = GAP_MIN_MS + k × GAP_STEP_MS`, `k ∈ 0..GAP_STEPS`.
pub const GAP_STEP_MS: u32 = 60;
/// Number of distinct gap lengths.
pub const GAP_STEPS: u32 = 8;
/// Events the phone needs for one measurement (about 21 s).
pub const MEASURE_EVENTS: usize = 32;
/// Minimum flash length.
pub const FLASH_MIN_MS: f64 = 80.0;
/// Chirp length.
pub const CHIRP_MS: f64 = 8.0;
/// Chirp start frequency.
pub const CHIRP_F0_HZ: f64 = 2_000.0;
/// Chirp end frequency.
pub const CHIRP_F1_HZ: f64 = 4_000.0;
/// Chirp attack/release (raised cosine) length.
pub const CHIRP_RAMP_MS: f64 = 0.5;
/// Chirp peak amplitude (full scale = 1.0).
pub const CHIRP_LEVEL: f64 = 0.85;
/// Sample rate of [`wav`].
pub const WAV_RATE: u32 = 24_000;

/// Galois LFSR taps for a maximal-length 32-bit register
/// (x^32 + x^22 + x^2 + x + 1).
const LFSR_TAPS: u32 = 0x8020_0003;
/// Stand-in for the forbidden all-zero LFSR state.
const ZERO_SEED: u32 = 0x5EED_CA11;

/// A usable seed from any entropy (never 0, which would stall the LFSR).
pub fn seed_from(entropy: u64) -> u32 {
    let s = (entropy ^ (entropy >> 32)) as u32;
    if s == 0 {
        ZERO_SEED
    } else {
        s
    }
}

/// One Galois LFSR clock.
pub fn lfsr_step(state: u32) -> u32 {
    let lsb = state & 1;
    let s = state >> 1;
    if lsb != 0 {
        s ^ LFSR_TAPS
    } else {
        s
    }
}

/// The endless gap generator for `seed` (clocked 8 times per gap so
/// consecutive gaps share no register bits).
fn gaps(seed: u32) -> impl Iterator<Item = u32> {
    let mut state = if seed == 0 { ZERO_SEED } else { seed };
    std::iter::from_fn(move || {
        for _ in 0..8 {
            state = lfsr_step(state);
        }
        Some(GAP_MIN_MS + (state % GAP_STEPS) * GAP_STEP_MS)
    })
}

/// Event times (ms from pattern position 0) of one cycle of [`RUN_MS`].
/// The first event is at [`LEAD_IN_MS`]; no event starts within
/// [`TAIL_MS`] of the cycle end. Deterministic in `seed`.
pub fn events_ms(seed: u32) -> Vec<u32> {
    let end = RUN_MS as u32 - TAIL_MS;
    let mut out = Vec::with_capacity(96);
    let mut t = LEAD_IN_MS;
    let mut g = gaps(seed);
    while t < end {
        out.push(t);
        t += g.next().unwrap_or(GAP_MIN_MS);
    }
    out
}

/// Flash length for a presentation slot of `slot_ms` (0 = unknown):
/// `max(80 ms, 2 slots)`.
pub fn flash_len_ms(slot_ms: f64) -> f64 {
    let slot = if slot_ms.is_finite() && slot_ms > 0.0 {
        slot_ms
    } else {
        0.0
    };
    FLASH_MIN_MS.max(2.0 * slot)
}

/// Whether the v2 pattern lights everything at `pos_ms` (position in the
/// calibration item; wraps every [`RUN_MS`]). Stateless convenience; for
/// per-frame use prefer a cached [`CalSchedule::flash_on`].
pub fn flash_on_v2(pos_ms: f64, seed: u32, slot_ms: f64) -> bool {
    if !pos_ms.is_finite() || pos_ms < 0.0 {
        return false;
    }
    let pos = pos_ms % RUN_MS as f64;
    let len = flash_len_ms(slot_ms);
    let mut t = LEAD_IN_MS as f64;
    let end = (RUN_MS as u32 - TAIL_MS) as f64;
    let mut g = gaps(seed);
    while t < end && t <= pos {
        if pos < t + len {
            return true;
        }
        t += g.next().unwrap_or(GAP_MIN_MS) as f64;
    }
    false
}

/// Chirp parameters, sent to the phone for its matched filter.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChirpSpec {
    pub ms: f64,
    pub f0_hz: f64,
    pub f1_hz: f64,
    pub ramp_ms: f64,
}

/// The chirp this module plays.
pub const CHIRP: ChirpSpec = ChirpSpec {
    ms: CHIRP_MS,
    f0_hz: CHIRP_F0_HZ,
    f1_hz: CHIRP_F1_HZ,
    ramp_ms: CHIRP_RAMP_MS,
};

/// A calibration run's plan: what `POST /player/calibration {pattern:"v2"}`
/// returns (plus the engine's `startsInMs`). JSON:
/// `{seed, v, eventsMs, flashMs, windowMs, leadInMs, chirp:{ms,f0Hz,f1Hz,rampMs}}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CalSchedule {
    pub seed: u32,
    pub v: u8,
    /// Event (flash + chirp start) times from pattern position 0, one cycle.
    pub events_ms: Vec<u32>,
    /// Flash length.
    pub flash_ms: f64,
    /// Cycle length: the pattern (and WAV) repeats after this.
    pub window_ms: u64,
    pub lead_in_ms: u32,
    pub chirp: ChirpSpec,
}

/// The plan for `seed` at presentation slot `slot_ms` (0 if unknown).
pub fn schedule(seed: u32, slot_ms: f64) -> CalSchedule {
    CalSchedule {
        seed,
        v: VERSION,
        events_ms: events_ms(seed),
        flash_ms: flash_len_ms(slot_ms),
        window_ms: RUN_MS,
        lead_in_ms: LEAD_IN_MS,
        chirp: CHIRP,
    }
}

impl CalSchedule {
    /// Whether everything is lit at `pos_ms` (wraps every `window_ms`);
    /// O(log n). Identical to [`flash_on_v2`] for the same seed and slot.
    pub fn flash_on(&self, pos_ms: f64) -> bool {
        if !pos_ms.is_finite() || pos_ms < 0.0 || self.window_ms == 0 {
            return false;
        }
        let pos = pos_ms % self.window_ms as f64;
        // Last event at or before `pos`.
        let idx = self.events_ms.partition_point(|&e| e as f64 <= pos);
        idx > 0 && pos < self.events_ms[idx - 1] as f64 + self.flash_ms
    }
}

/// The chirp as float samples at `sample_rate` (peak [`CHIRP_LEVEL`]).
/// Linear sweep `f0 → f1` over [`CHIRP_MS`], raised-cosine ramps of
/// [`CHIRP_RAMP_MS`] at both ends. Sample 0 is the event instant.
pub fn chirp(sample_rate: u32) -> Vec<f32> {
    let rate = sample_rate.max(1) as f64;
    let len = ((CHIRP_MS / 1000.0) * rate).round() as usize;
    let dur = CHIRP_MS / 1000.0;
    let ramp = ((CHIRP_RAMP_MS / 1000.0) * rate).max(1.0);
    let k = (CHIRP_F1_HZ - CHIRP_F0_HZ) / dur;
    (0..len)
        .map(|i| {
            let t = i as f64 / rate;
            let phase = 2.0 * std::f64::consts::PI * (CHIRP_F0_HZ * t + 0.5 * k * t * t);
            let fi = i as f64;
            let from_end = (len - 1 - i) as f64;
            let env = if fi < ramp {
                0.5 - 0.5 * (std::f64::consts::PI * fi / ramp).cos()
            } else if from_end < ramp {
                0.5 - 0.5 * (std::f64::consts::PI * from_end / ramp).cos()
            } else {
                1.0
            };
            (phase.sin() * env * CHIRP_LEVEL) as f32
        })
        .collect()
}

/// Cache file name for the seed's WAV (`cal-<seed>.wav`).
pub fn wav_file_name(seed: u32) -> String {
    format!("cal-{seed}.wav")
}

/// One cycle ([`RUN_MS`]) of the pattern's sound: mono 16-bit PCM WAV at
/// [`WAV_RATE`], silence except a [`chirp`] starting exactly at each event.
pub fn wav(seed: u32) -> Vec<u8> {
    let rate = WAV_RATE;
    let samples = (RUN_MS * rate as u64 / 1000) as usize;
    let mut pcm = vec![0i16; samples];
    let c = chirp(rate);
    for e in events_ms(seed) {
        let start = (e as u64 * rate as u64 / 1000) as usize;
        for (i, v) in c.iter().enumerate() {
            if let Some(s) = pcm.get_mut(start + i) {
                *s = (*v as f64 * i16::MAX as f64).round() as i16;
            }
        }
    }
    let data_len = (samples * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cross-language test vector: `web/src/lib/sensing/schedule.test.ts`
    /// asserts the same numbers for its TypeScript mirror.
    #[test]
    fn test_vector_seed_1() {
        let ev = events_ms(1);
        assert_eq!(&ev[..8], &TEST_VECTOR_SEED_1);
    }

    const TEST_VECTOR_SEED_1: [u32; 8] = [2000, 2570, 3200, 4010, 4760, 5630, 6500, 6950];

    #[test]
    fn deterministic_and_seed_dependent() {
        assert_eq!(events_ms(42), events_ms(42));
        assert_ne!(events_ms(42), events_ms(43));
        assert_eq!(schedule(7, 25.0), schedule(7, 25.0));
        assert_eq!(wav(9), wav(9));
    }

    #[test]
    fn zero_seed_is_usable() {
        assert_eq!(events_ms(0), events_ms(ZERO_SEED));
        assert_ne!(seed_from(0), 0);
        assert_ne!(seed_from(u64::MAX), 0);
        assert_ne!(seed_from(0xFFFF_FFFF_0000_0000 | 0xFFFF_FFFF), 0);
        // Gaps still vary for the zero stand-in.
        let ev = events_ms(0);
        let g: std::collections::BTreeSet<u32> = ev.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(g.len() > 4);
    }

    #[test]
    fn schedule_shape() {
        for seed in [1, 2, 3, 0xDEAD_BEEF, 12345] {
            let ev = events_ms(seed);
            assert_eq!(ev[0], LEAD_IN_MS);
            assert!(ev.len() >= MEASURE_EVENTS * 2, "{}", ev.len());
            assert!(*ev.last().unwrap() < RUN_MS as u32 - TAIL_MS);
            for w in ev.windows(2) {
                let gap = w[1] - w[0];
                assert!((GAP_MIN_MS..GAP_MIN_MS + GAP_STEPS * GAP_STEP_MS).contains(&gap));
                assert_eq!((gap - GAP_MIN_MS) % GAP_STEP_MS, 0);
            }
            // 32 events take roughly 21 s.
            let span = ev[MEASURE_EVENTS - 1] - ev[0];
            assert!((14_000..=27_000).contains(&span), "span {span}");
        }
    }

    #[test]
    fn gaps_use_every_length_and_are_not_periodic() {
        let ev = events_ms(0xC0FFEE);
        let gaps: Vec<u32> = ev.windows(2).map(|w| w[1] - w[0]).collect();
        let distinct: std::collections::BTreeSet<_> = gaps.iter().collect();
        assert_eq!(distinct.len(), GAP_STEPS as usize);
        // No period up to half the train.
        for p in 1..gaps.len() / 2 {
            assert!(
                gaps.iter().zip(&gaps[p..]).any(|(a, b)| a != b),
                "gap train repeats with period {p}"
            );
        }
    }

    /// The pairwise-difference histogram of the schedule against itself has a
    /// single clear peak (what the phone relies on): any non-zero shift within
    /// ±5 s aligns far fewer events (within ±25 ms) than the true one.
    #[test]
    fn autocorrelation_is_unambiguous() {
        for seed in [1u32, 99, 0xABCD_1234] {
            let ev: Vec<i64> = events_ms(seed)[..MEASURE_EVENTS]
                .iter()
                .map(|&e| e as i64)
                .collect();
            let all: Vec<i64> = events_ms(seed).iter().map(|&e| e as i64).collect();
            let score = |shift: i64| {
                ev.iter()
                    .filter(|&&x| all.iter().any(|&s| (x - (s + shift)).abs() < 25))
                    .count()
            };
            assert_eq!(score(0), MEASURE_EVENTS);
            let worst = (-5000..=5000)
                .step_by(5)
                .filter(|s: &i64| s.abs() >= 50)
                .map(score)
                .max()
                .unwrap();
            assert!(worst <= MEASURE_EVENTS / 2, "seed {seed}: worst {worst}");
        }
    }

    #[test]
    fn flash_matches_schedule() {
        for slot in [0.0, 25.0, 50.0, 66.7] {
            let plan = schedule(77, slot);
            let len = flash_len_ms(slot);
            assert!(len >= FLASH_MIN_MS && len >= 2.0 * slot);
            let mut pos = -10.0;
            while pos < RUN_MS as f64 * 2.0 {
                let a = flash_on_v2(pos, 77, slot);
                let b = plan.flash_on(pos);
                assert_eq!(a, b, "pos {pos} slot {slot}");
                pos += 3.7;
            }
            // Dark in the lead-in, lit at each event, dark just after.
            assert!(!plan.flash_on(0.0));
            assert!(!plan.flash_on(LEAD_IN_MS as f64 - 0.1));
            for &e in &plan.events_ms {
                assert!(plan.flash_on(e as f64));
                assert!(plan.flash_on(e as f64 + len - 0.1));
                assert!(!plan.flash_on(e as f64 + len));
                assert!(!plan.flash_on(e as f64 - 0.1));
            }
            // Wraps.
            let e = plan.events_ms[3] as f64;
            assert!(plan.flash_on(e + RUN_MS as f64));
        }
        assert!(!flash_on_v2(f64::NAN, 1, 25.0));
        assert!(!flash_on_v2(-1.0, 1, 25.0));
        assert_eq!(flash_len_ms(f64::NAN), FLASH_MIN_MS);
    }

    #[test]
    fn chirp_shape() {
        let c = chirp(48_000);
        assert_eq!(c.len(), 384);
        let peak = c.iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(peak <= CHIRP_LEVEL as f32 + 1e-6 && peak > 0.8 * CHIRP_LEVEL as f32);
        assert!(c[0].abs() < 1e-6);
        assert!(c[c.len() - 1].abs() < 0.05);
        // Zero crossings grow faster: 2 kHz at the start, 4 kHz at the end.
        let crossings = |s: &[f32]| s.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        let first = crossings(&c[..96]);
        let last = crossings(&c[c.len() - 96..]);
        assert!(last > first, "{first} {last}");
    }

    #[test]
    fn wav_places_chirps_at_events() {
        let bytes = wav(5);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        let rate = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
        assert_eq!(rate, WAV_RATE);
        let data_len = u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as usize;
        assert_eq!(bytes.len(), 44 + data_len);
        assert_eq!(data_len, (RUN_MS as usize * WAV_RATE as usize / 1000) * 2);
        let pcm: Vec<i16> = bytes[44..]
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        let at = |ms: f64| (ms * WAV_RATE as f64 / 1000.0) as usize;
        // Silent lead-in.
        assert!(pcm[..at(LEAD_IN_MS as f64)].iter().all(|&s| s == 0));
        for &e in &events_ms(5) {
            let s = at(e as f64);
            let energy: i64 = pcm[s..s + at(CHIRP_MS)].iter().map(|&v| (v as i64).abs()).sum();
            assert!(energy > 0);
            // Silence just before and well after.
            assert_eq!(pcm[s - 1], 0);
            assert!(pcm[s + at(CHIRP_MS) + 2..s + at(100.0)].iter().all(|&v| v == 0));
        }
        assert_eq!(wav_file_name(5), "cal-5.wav");
    }

    #[test]
    fn schedule_json_is_camel_case() {
        let j = serde_json::to_value(schedule(3, 20.0)).unwrap();
        for k in ["seed", "v", "eventsMs", "flashMs", "windowMs", "leadInMs", "chirp"] {
            assert!(j.get(k).is_some(), "{k}");
        }
        assert_eq!(j["chirp"]["f0Hz"], 2000.0);
        assert_eq!(j["v"], 2);
    }
}
