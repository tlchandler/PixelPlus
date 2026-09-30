//! Beat, tempo and energy analysis (F2, ARCHITECTURE §12.2).
//!
//! Pure DSP on mono samples: the daemon decodes the upload (symphonia),
//! downmixes and streams the samples into an [`Analyzer`]; [`Analyzer::finish`]
//! returns the versioned [`Analysis`] that is stored as
//! `media/<id>.analysis.json` and served by `GET /media/:id/analysis`.
//!
//! Pipeline (all O(n log n) or O(n · lags), bounded memory: only per-frame
//! scalars are kept, never the spectrogram):
//! 1. Input at 32 kHz or more is halved with a windowed-sinc low-pass, so the
//!    analysis rate is 16–24 kHz. STFT: Hann window 1024, hop 256 (≈11.6 ms
//!    at 22.05 kHz), centred frames.
//! 2. Onset strength: log magnitude `ln(1 + 100·|X|)` averaged in 8
//!    log-spaced bands (30 Hz – 11 kHz), half-wave rectified frame difference
//!    (spectral flux) summed over the bands; a 1 s moving mean is subtracted
//!    and the result scaled to unit standard deviation.
//! 3. Tempo: autocorrelation of the onset strength for 60–200 BPM weighted by a
//!    log-Gaussian prior (µ = 120 BPM, σ = 1 octave); octave check against
//!    half / double tempo (prefer 80–160 BPM unless the other scores 1.5×
//!    higher; a pulse whose "off-beats" are as strong as its beats is taken at
//!    the faster rate). Local tempo in 20 s windows gives `tempoCurve`.
//! 4. Beats: Ellis dynamic programming, `score(t) = O(t) + max_τ [score(τ) −
//!    100·ln((t−τ)/P)²]` for τ ∈ [t−2P, t−P/2] with the windowed period P,
//!    backtraced from the last strong peak; weak leading / trailing beats are
//!    trimmed. The final BPM is the least-squares slope of the beat times.
//! 5. Downbeats (4/4 assumed): of the 4 bar phases, the one with the most
//!    low-band (30–130 Hz) onset plus chroma change at its beats.
//! 6. Energy at 10 Hz: RMS and low / mid / high band energy, each mapped to
//!    0–255 between the song's 5th and 95th percentile (in dB). Sections: a
//!    Gaussian-tapered checkerboard kernel (±4 s) over the self-similarity of
//!    those energy vectors, peaks at least 8 s apart, each section labelled
//!    low / mid / high by its mean energy.
//!
//! Beat-reactive effects (F2, rendered by `effects*`, WS3) use
//! [`beat_pulse`] (a fixed BPM, no audio needed) or
//! [`Analysis::beat_envelope`] (real beats of a song); both are pure functions
//! of the timeline position, so followers render identically.

use crate::model::AudioAnalysisSummary;
use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Format version of [`Analysis`] (`v`), bumped when results change.
pub const VERSION: u32 = 1;
/// Audio beyond this is ignored (hostile / accidental hour-long uploads).
pub const MAX_DURATION_MS: u64 = 20 * 60 * 1000;
/// STFT window and hop (samples at the analysis rate).
pub const WIN: usize = 1024;
pub const HOP: usize = 256;
/// Tempo search range.
pub const MIN_BPM: f32 = 60.0;
pub const MAX_BPM: f32 = 200.0;

const BANDS: usize = 8;
const BAND_LO_HZ: f32 = 30.0;
const BAND_HI_HZ: f32 = 11_000.0;
/// DP transition tightness (Ellis 2007 / librosa).
const TIGHTNESS: f32 = 100.0;

/// Full analysis of one song, `media/<id>.analysis.json` (JSON camelCase).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Analysis {
    /// Format version ([`VERSION`]).
    pub v: u32,
    /// Analysis sample rate (Hz).
    pub sr: u32,
    /// Onset frame hop in ms.
    pub hop_ms: f32,
    /// Analysed length (ms); at most [`MAX_DURATION_MS`].
    #[serde(default)]
    pub duration_ms: u64,
    /// Main tempo; 0 when no steady beat was found (speech, ambient).
    pub bpm: f32,
    /// 0..1: how clearly the tempo stands out.
    pub bpm_confidence: f32,
    /// Local tempo per 20 s window.
    pub tempo_curve: Vec<f32>,
    /// Beat times (ms).
    pub beats: Vec<u32>,
    /// Bar starts (ms), a subset of `beats` (4/4 assumed).
    pub downbeats: Vec<u32>,
    /// 0..1: how clearly one bar phase stood out.
    #[serde(default)]
    pub downbeat_confidence: f32,
    /// Note / drum hits.
    pub onsets: Vec<Onset>,
    /// Energy curves at 10 Hz, 0..255 each.
    #[serde(rename = "energy10Hz")]
    pub energy_10hz: Energy,
    pub sections: Vec<Section>,
}

/// One detected hit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Onset {
    pub ms: u32,
    /// 0..1 relative to the song's strongest hits.
    pub strength: f32,
    /// Dominant band: 0 low (bass, kick), 1 mid, 2 high (hats, sparkle).
    pub band: u8,
}

/// Energy curves at 10 Hz (index = time / 100 ms), each 0..255 between the
/// song's 5th and 95th percentile.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Energy {
    pub rms: Vec<u8>,
    /// 30–250 Hz.
    pub low: Vec<u8>,
    /// 250–2000 Hz.
    pub mid: Vec<u8>,
    /// 2–11 kHz.
    pub high: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    Low,
    Mid,
    High,
}

/// A part of the song (intro, verse, chorus…) by energy.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    pub start_ms: u32,
    pub end_ms: u32,
    pub level: Level,
}

impl Analysis {
    /// The stored summary (`Media.analysis`).
    pub fn summary(&self) -> AudioAnalysisSummary {
        let rms = &self.energy_10hz.rms;
        let energy = if rms.is_empty() {
            0.0
        } else {
            rms.iter().map(|&v| v as f32).sum::<f32>() / rms.len() as f32 / 255.0
        };
        AudioAnalysisSummary {
            version: self.v,
            bpm: (self.bpm * 10.0).round() / 10.0,
            bpm_confidence: (self.bpm_confidence * 100.0).round() / 100.0,
            beat_count: self.beats.len() as u32,
            first_beat_ms: self.beats.first().copied().unwrap_or(0),
            energy: (energy * 100.0).round() / 100.0,
            sections: self.sections.len() as u32,
        }
    }

    /// Beat envelope at `t_ms`: 1.0 on a beat, decaying as `exp(−Δt/τ)`
    /// until the next one (0 before the first beat).
    pub fn beat_envelope(&self, t_ms: u64, tau_ms: f32) -> f32 {
        envelope(&self.beats, t_ms, tau_ms)
    }

    /// Same on bar starts.
    pub fn downbeat_envelope(&self, t_ms: u64, tau_ms: f32) -> f32 {
        envelope(&self.downbeats, t_ms, tau_ms)
    }

    /// Section playing at `t_ms`.
    pub fn section_at(&self, t_ms: u64) -> Option<&Section> {
        self.sections
            .iter()
            .find(|s| t_ms >= u64::from(s.start_ms) && t_ms < u64::from(s.end_ms))
            .or(self.sections.last().filter(|s| t_ms >= u64::from(s.end_ms)))
    }

    /// Normalized loudness 0..1 at `t_ms` (linear between the 10 Hz points).
    pub fn energy_at(&self, t_ms: u64) -> f32 {
        curve_at(&self.energy_10hz.rms, t_ms)
    }

    /// Index of the last beat at or before `t_ms`.
    pub fn beat_index(&self, t_ms: u64) -> Option<usize> {
        let p = self.beats.partition_point(|&b| u64::from(b) <= t_ms);
        p.checked_sub(1)
    }
}

/// Linear interpolation into a 10 Hz 0..255 curve, as 0..1.
pub fn curve_at(curve: &[u8], t_ms: u64) -> f32 {
    if curve.is_empty() {
        return 0.0;
    }
    let x = t_ms as f32 / 100.0;
    let i = (x.floor() as usize).min(curve.len() - 1);
    let j = (i + 1).min(curve.len() - 1);
    let f = (x - i as f32).clamp(0.0, 1.0);
    (curve[i] as f32 * (1.0 - f) + curve[j] as f32 * f) / 255.0
}

/// `exp(−(t − b)/τ)` for the last `b` ≤ `t` in sorted `beats` (0 before the first).
pub fn envelope(beats: &[u32], t_ms: u64, tau_ms: f32) -> f32 {
    let p = beats.partition_point(|&b| u64::from(b) <= t_ms);
    match p.checked_sub(1) {
        Some(i) => {
            let dt = (t_ms - u64::from(beats[i])) as f32;
            (-dt / tau_ms.max(1.0)).exp()
        }
        None => 0.0,
    }
}

/// Brightness multiplier for beat-reactive looks without audio (effect params
/// `beatBpm`, `beatPhaseMs`, `beatDepth`, `beatDecayMs`): `1 − depth·(1 − e)`
/// with `e = exp(−Δt/decay)` since the last beat of a `bpm` grid through
/// `phase_ms`. Returns 1.0 when `bpm` ≤ 0 or `depth` ≤ 0. Pure function of
/// `t_ms`, so every node renders the same pulse.
pub fn beat_pulse(t_ms: u64, bpm: f32, phase_ms: i64, depth: f32, decay_ms: f32) -> f32 {
    if !(bpm > 0.0) || !(depth > 0.0) {
        return 1.0;
    }
    let period = 60_000.0 / f64::from(bpm.clamp(1.0, 1000.0));
    let dt = (t_ms as f64 - phase_ms as f64).rem_euclid(period) as f32;
    let e = (-dt / decay_ms.max(1.0)).exp();
    1.0 - depth.min(1.0) * (1.0 - e)
}

// ---------------------------------------------------------------------------
// Streaming analyzer
// ---------------------------------------------------------------------------

/// Streams mono samples in, produces an [`Analysis`] at the end.
pub struct Analyzer {
    rate: u32,
    decim: Option<Decimator>,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    win_norm: f32,
    buf: Vec<f32>,
    spec: Vec<Complex32>,
    scratch: Vec<Complex32>,
    /// Inclusive bin range of each onset band.
    bands: [(usize, usize); BANDS],
    /// Bin ranges for the low / mid / high energy curves.
    energy_bins: [(usize, usize); 3],
    /// Pitch class per bin (-1 = not used for chroma).
    chroma_of_bin: Vec<i8>,
    prev_log: [f32; BANDS],
    have_prev: bool,
    // Per frame.
    onset: Vec<f32>,
    onset_low: Vec<f32>,
    onset_band: Vec<u8>,
    chroma: Vec<[u8; 12]>,
    // Per 100 ms block.
    block: usize,
    rms_acc: f64,
    rms_n: usize,
    rms: Vec<f32>,
    band_e: [Vec<f32>; 3],
    band_n: Vec<u16>,
    samples: u64,
    max_samples: u64,
}

impl Analyzer {
    /// `sample_rate` of the mono samples that will be pushed.
    pub fn new(sample_rate: u32) -> Self {
        let in_rate = sample_rate.max(1000);
        let (rate, decim) = if in_rate >= 32_000 {
            (in_rate / 2, Some(Decimator::new()))
        } else {
            (in_rate, None)
        };
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(WIN);
        let scratch = vec![Complex32::default(); fft.get_inplace_scratch_len()];
        let window: Vec<f32> = (0..WIN)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / WIN as f32).cos())
            .collect();
        let win_norm = window.iter().sum::<f32>() / 2.0;
        let df = rate as f32 / WIN as f32;
        let nyq_bin = WIN / 2;
        let bin = |hz: f32| ((hz / df).round() as usize).clamp(1, nyq_bin);
        let mut bands = [(0usize, 0usize); BANDS];
        let ratio = (BAND_HI_HZ.min(rate as f32 / 2.0 * 0.98) / BAND_LO_HZ).powf(1.0 / BANDS as f32);
        let mut lo_hz = BAND_LO_HZ;
        let mut next_lo = bin(lo_hz);
        for b in bands.iter_mut() {
            let hi_hz = lo_hz * ratio;
            let lo = next_lo.min(nyq_bin);
            let hi = bin(hi_hz).max(lo);
            *b = (lo, hi);
            next_lo = (hi + 1).min(nyq_bin);
            lo_hz = hi_hz;
        }
        let energy_bins = [
            (bin(30.0), bin(250.0)),
            (bin(250.0) + 1, bin(2000.0)),
            (bin(2000.0) + 1, bin(11_000.0).max(bin(2000.0) + 1)),
        ];
        let chroma_of_bin = (0..=nyq_bin)
            .map(|k| {
                let f = k as f32 * df;
                if !(200.0..=4000.0).contains(&f) {
                    return -1;
                }
                let midi = 69.0 + 12.0 * (f / 440.0).log2();
                (midi.round() as i32).rem_euclid(12) as i8
            })
            .collect();
        let block = (rate / 10) as usize;
        Analyzer {
            rate,
            decim,
            fft,
            window,
            win_norm,
            // Centred frames: frame i is centred on sample i·HOP.
            buf: vec![0.0; WIN / 2],
            spec: vec![Complex32::default(); WIN],
            scratch,
            bands,
            energy_bins,
            chroma_of_bin,
            prev_log: [0.0; BANDS],
            have_prev: false,
            onset: Vec::new(),
            onset_low: Vec::new(),
            onset_band: Vec::new(),
            chroma: Vec::new(),
            block,
            rms_acc: 0.0,
            rms_n: 0,
            rms: Vec::new(),
            band_e: [Vec::new(), Vec::new(), Vec::new()],
            band_n: Vec::new(),
            samples: 0,
            max_samples: MAX_DURATION_MS * u64::from(rate) / 1000,
        }
    }

    /// Analysis rate (after the optional 2× decimation).
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Audio analysed so far (ms).
    pub fn duration_ms(&self) -> u64 {
        self.samples * 1000 / u64::from(self.rate)
    }

    /// Reached [`MAX_DURATION_MS`]: further samples are ignored.
    pub fn is_full(&self) -> bool {
        self.samples >= self.max_samples
    }

    /// Feed mono samples (any chunk size, nominally −1..1).
    pub fn push(&mut self, mono: &[f32]) {
        if self.is_full() {
            return;
        }
        match self.decim.as_mut() {
            Some(d) => {
                let mut out = Vec::with_capacity(mono.len() / 2 + 1);
                d.process(mono, &mut out);
                self.push_rate(&out);
            }
            None => self.push_rate(mono),
        }
    }

    fn push_rate(&mut self, s: &[f32]) {
        let room = (self.max_samples - self.samples) as usize;
        let s = &s[..s.len().min(room)];
        for &x in s {
            let x = if x.is_finite() { x } else { 0.0 };
            self.rms_acc += f64::from(x * x);
            self.rms_n += 1;
            if self.rms_n == self.block {
                self.rms.push((self.rms_acc / self.rms_n as f64).sqrt() as f32);
                self.rms_acc = 0.0;
                self.rms_n = 0;
            }
        }
        self.samples += s.len() as u64;
        self.buf.extend(s.iter().map(|x| if x.is_finite() { *x } else { 0.0 }));
        let mut start = 0;
        while self.buf.len() - start >= WIN {
            self.frame(start);
            start += HOP;
        }
        self.buf.drain(..start);
    }

    fn frame(&mut self, start: usize) {
        for (k, c) in self.spec.iter_mut().enumerate() {
            *c = Complex32::new(self.buf[start + k] * self.window[k], 0.0);
        }
        self.fft
            .process_with_scratch(&mut self.spec, &mut self.scratch);
        let nyq = WIN / 2;
        let inv = 1.0 / self.win_norm;
        // Magnitudes (reuse the real parts).
        for c in self.spec[..=nyq].iter_mut() {
            c.re = c.norm() * inv;
        }
        let mag = |k: usize| self.spec[k].re;
        // Onset bands.
        let mut cur = [0f32; BANDS];
        for (b, &(lo, hi)) in self.bands.iter().enumerate() {
            let mut sum = 0.0;
            for k in lo..=hi {
                sum += (1.0 + 100.0 * mag(k)).ln();
            }
            cur[b] = sum / (hi - lo + 1) as f32;
        }
        let mut flux = [0f32; BANDS];
        if self.have_prev {
            for b in 0..BANDS {
                flux[b] = (cur[b] - self.prev_log[b]).max(0.0);
            }
        }
        self.prev_log = cur;
        self.have_prev = true;
        let total: f32 = flux.iter().sum();
        self.onset.push(total);
        self.onset_low.push(flux[0] + flux[1]);
        let groups = [
            flux[0] + flux[1] + flux[2],
            flux[3] + flux[4] + flux[5],
            flux[6] + flux[7],
        ];
        let band = (0..3)
            .max_by(|&a, &b| groups[a].total_cmp(&groups[b]))
            .unwrap_or(1) as u8;
        self.onset_band.push(band);
        // Chroma.
        let mut ch = [0f32; 12];
        for (k, &pc) in self.chroma_of_bin.iter().enumerate() {
            if pc >= 0 {
                ch[pc as usize] += mag(k);
            }
        }
        let mx = ch.iter().fold(0f32, |a, &b| a.max(b));
        let q = if mx > 1e-9 {
            ch.map(|v| (v / mx * 255.0).round() as u8)
        } else {
            [0; 12]
        };
        self.chroma.push(q);
        // Band energy into the frame's 100 ms block.
        let frame = self.onset.len() - 1;
        let blk = frame * HOP / self.block;
        while self.band_n.len() <= blk {
            self.band_n.push(0);
            for e in self.band_e.iter_mut() {
                e.push(0.0);
            }
        }
        for (i, &(lo, hi)) in self.energy_bins.iter().enumerate() {
            let mut p = 0.0;
            for k in lo..=hi.min(nyq) {
                p += mag(k) * mag(k);
            }
            self.band_e[i][blk] += p;
        }
        self.band_n[blk] = self.band_n[blk].saturating_add(1);
    }

    /// Finish and analyse.
    pub fn finish(mut self) -> Analysis {
        // Flush the tail (centred frames: pad half a window).
        let pad = vec![0.0; WIN / 2 + HOP];
        let mut start = 0;
        self.buf.extend_from_slice(&pad);
        while self.buf.len() - start >= WIN {
            self.frame(start);
            start += HOP;
        }
        if self.rms_n > self.block / 2 {
            self.rms
                .push((self.rms_acc / self.rms_n as f64).sqrt() as f32);
        }
        analyze_frames(self)
    }
}

/// Analyse a whole buffer of mono samples (tests, short clips).
pub fn analyze(mono: &[f32], sample_rate: u32) -> Analysis {
    let mut a = Analyzer::new(sample_rate);
    for chunk in mono.chunks(4096) {
        a.push(chunk);
    }
    a.finish()
}

/// 2× decimation with a 31-tap windowed-sinc low-pass (cutoff 0.23·fs).
struct Decimator {
    taps: [f32; 31],
    hist: Vec<f32>,
    phase: bool,
}

impl Decimator {
    fn new() -> Self {
        let mut taps = [0f32; 31];
        let fc = 0.23f32;
        let m = 30.0;
        for (i, t) in taps.iter_mut().enumerate() {
            let n = i as f32 - m / 2.0;
            let sinc = if n == 0.0 {
                2.0 * fc
            } else {
                (std::f32::consts::TAU * fc * n).sin() / (std::f32::consts::PI * n)
            };
            let w = 0.42 - 0.5 * (std::f32::consts::TAU * i as f32 / m).cos()
                + 0.08 * (2.0 * std::f32::consts::TAU * i as f32 / m).cos();
            *t = sinc * w;
        }
        let sum: f32 = taps.iter().sum();
        for t in taps.iter_mut() {
            *t /= sum;
        }
        Decimator {
            taps,
            hist: vec![0.0; 30],
            phase: false,
        }
    }

    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        for &x in input {
            self.hist.push(x);
            if self.phase {
                let n = self.hist.len();
                let w = &self.hist[n - 31..];
                out.push(w.iter().zip(self.taps.iter()).map(|(a, b)| a * b).sum());
            }
            self.phase = !self.phase;
            if self.hist.len() > 4096 {
                let n = self.hist.len();
                self.hist.drain(..n - 30);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Offline part
// ---------------------------------------------------------------------------

fn analyze_frames(a: Analyzer) -> Analysis {
    let rate = a.rate;
    let fps = rate as f32 / HOP as f32;
    let hop_ms = 1000.0 / fps;
    let n = a.onset.len();
    let duration_ms = a.samples * 1000 / u64::from(rate);
    let energy = energy_curves(&a.rms, &a.band_e, &a.band_n);
    let sections = sections(&energy, duration_ms);
    let mut out = Analysis {
        v: VERSION,
        sr: rate,
        hop_ms: (hop_ms * 100.0).round() / 100.0,
        duration_ms,
        bpm: 0.0,
        bpm_confidence: 0.0,
        tempo_curve: vec![],
        beats: vec![],
        downbeats: vec![],
        downbeat_confidence: 0.0,
        onsets: vec![],
        energy_10hz: energy,
        sections,
    };
    let o = normalize_onset(&a.onset, fps);
    if n < (fps * 4.0) as usize || o.iter().all(|&v| v == 0.0) {
        return out; // too short or silent: no beat
    }
    let frame_ms = |f: f32| (f * hop_ms).max(0.0).round() as u32;
    out.onsets = pick_onsets(&o, &a.onset_band, fps)
        .into_iter()
        .map(|(f, s, b)| Onset {
            ms: frame_ms(f as f32),
            strength: s,
            band: b,
        })
        .collect();

    // Tempo.
    let Some(tempo) = global_tempo(&o, fps) else {
        return out;
    };
    let curve = tempo_curve(&o, fps, tempo.lag);
    out.tempo_curve = curve
        .iter()
        .map(|&lag| ((60.0 * fps / lag) * 10.0).round() / 10.0)
        .collect();
    out.bpm_confidence = tempo.confidence;

    // Beats.
    let win = (20.0 * fps) as usize;
    let period_at = |i: usize| curve.get(i / win.max(1)).copied().unwrap_or(tempo.lag);
    let beats = track_beats(&o, &period_at, tempo.lag);
    if beats.len() < 4 {
        out.bpm = 60.0 * fps / tempo.lag;
        return out;
    }
    // Refine: least-squares slope of beat frames vs index.
    let bpm_acf = 60.0 * fps / tempo.lag;
    let slope = ls_slope(&beats);
    out.bpm = match slope {
        Some(p) if (60.0 * fps / p / bpm_acf - 1.0).abs() < 0.06 => 60.0 * fps / p,
        _ => bpm_acf,
    };
    out.bpm = (out.bpm * 100.0).round() / 100.0;
    out.beats = beats.iter().map(|&f| frame_ms(f as f32)).collect();

    // Downbeats.
    let (phase, conf) = downbeat_phase(&beats, &a.onset_low, &a.chroma);
    out.downbeats = out.beats.iter().skip(phase).step_by(4).copied().collect();
    out.downbeat_confidence = (conf * 100.0).round() / 100.0;
    out
}

/// High-pass (1 s moving mean removed), rectified, unit standard deviation.
fn normalize_onset(raw: &[f32], fps: f32) -> Vec<f32> {
    let n = raw.len();
    if n == 0 {
        return vec![];
    }
    let half = (fps / 2.0) as usize;
    let mut prefix = vec![0f64; n + 1];
    for i in 0..n {
        prefix[i + 1] = prefix[i] + f64::from(raw[i]);
    }
    let mut o: Vec<f32> = (0..n)
        .map(|i| {
            let lo = i.saturating_sub(half);
            let hi = (i + half + 1).min(n);
            let mean = (prefix[hi] - prefix[lo]) / (hi - lo) as f64;
            (raw[i] - mean as f32).max(0.0)
        })
        .collect();
    let mean = o.iter().sum::<f32>() / n as f32;
    let var = o.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n as f32;
    let std = var.sqrt();
    if std < 1e-6 {
        return vec![0.0; n];
    }
    for v in o.iter_mut() {
        *v /= std;
    }
    o
}

/// Local maxima (±3 frames), above 1.2 σ, at least 50 ms apart; strongest
/// ≤ 4/s kept. Returns (frame, strength 0..1, band).
fn pick_onsets(o: &[f32], band: &[u8], fps: f32) -> Vec<(usize, f32, u8)> {
    let n = o.len();
    let mut peaks: Vec<(usize, f32)> = Vec::new();
    let min_gap = (0.05 * fps).ceil() as usize;
    for i in 0..n {
        let v = o[i];
        if v < 1.2 {
            continue;
        }
        let lo = i.saturating_sub(3);
        let hi = (i + 4).min(n);
        if o[lo..hi].iter().any(|&x| x > v) || (lo..i).any(|j| o[j] == v) {
            continue;
        }
        if let Some(&(p, pv)) = peaks.last() {
            if i - p < min_gap {
                if v > pv {
                    peaks.pop();
                } else {
                    continue;
                }
            }
        }
        peaks.push((i, v));
    }
    let mut sorted: Vec<f32> = peaks.iter().map(|p| p.1).collect();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let top = sorted
        .get(((sorted.len() as f32) * 0.95) as usize)
        .copied()
        .unwrap_or(1.0)
        .max(1e-6);
    let max_n = ((n as f32 / fps) * 4.0) as usize;
    if peaks.len() > max_n {
        let mut by: Vec<(usize, f32)> = peaks.clone();
        by.sort_by(|a, b| b.1.total_cmp(&a.1));
        by.truncate(max_n);
        by.sort_by_key(|p| p.0);
        peaks = by;
    }
    peaks
        .into_iter()
        .map(|(i, v)| {
            (
                i,
                ((v / top).min(1.0) * 100.0).round() / 100.0,
                band.get(i).copied().unwrap_or(1),
            )
        })
        .collect()
}

struct Tempo {
    /// Beat period in frames (fractional).
    lag: f32,
    confidence: f32,
}

fn acf(o: &[f32], lag: usize) -> f32 {
    if lag >= o.len() {
        return 0.0;
    }
    let n = o.len() - lag;
    let mut s = 0.0f32;
    for i in 0..n {
        s += o[i] * o[i + lag];
    }
    s / n as f32
}

fn prior(bpm: f32) -> f32 {
    let x = (bpm / 120.0).log2();
    (-0.5 * x * x).exp()
}

/// Peak position by parabolic interpolation around integer `k` of `f`.
fn parabolic(f: &dyn Fn(usize) -> f32, k: usize) -> f32 {
    if k == 0 {
        return k as f32;
    }
    let (a, b, c) = (f(k - 1), f(k), f(k + 1));
    let d = a - 2.0 * b + c;
    if d.abs() < 1e-12 {
        return k as f32;
    }
    let off = (0.5 * (a - c) / d).clamp(-0.5, 0.5);
    k as f32 + off
}

fn global_tempo(o: &[f32], fps: f32) -> Option<Tempo> {
    // Lags for 30..400 BPM (octave relatives of the 60..200 range).
    let lag_of = |bpm: f32| 60.0 * fps / bpm;
    let lo = lag_of(400.0).floor().max(2.0) as usize;
    let hi = (lag_of(30.0).ceil() as usize + 2).min(o.len().saturating_sub(1));
    if hi <= lo + 4 {
        return None;
    }
    let mut r = vec![0f32; hi + 2];
    for (lag, v) in r.iter_mut().enumerate().take(hi + 2).skip(lo.saturating_sub(1)) {
        *v = acf(o, lag);
    }
    let raw = |lag: usize| r.get(lag).copied().unwrap_or(0.0);
    // Peak value near a (fractional) lag.
    let near = |lag: f32| {
        let k = lag.round() as usize;
        raw(k.saturating_sub(1)).max(raw(k)).max(raw(k + 1))
    };
    let score = |lag: usize| raw(lag) * prior(60.0 * fps / lag as f32);
    let lmin = lag_of(MAX_BPM).floor() as usize;
    let lmax = lag_of(MIN_BPM).ceil() as usize;
    let range: Vec<usize> = (lmin.max(lo)..=lmax.min(hi)).collect();
    let best = *range
        .iter()
        .max_by(|&&a, &&b| score(a).total_cmp(&score(b)))?;
    if raw(best) <= 0.0 {
        return None;
    }
    let scores: Vec<f32> = range.iter().map(|&l| score(l)).collect();
    let mean = scores.iter().sum::<f32>() / scores.len() as f32;
    let std = (scores.iter().map(|s| (s - mean) * (s - mean)).sum::<f32>() / scores.len() as f32)
        .sqrt();
    let raw_conf = score(best) / (mean + std).max(1e-6);
    let confidence = ((raw_conf - 1.0) / 3.0).clamp(0.0, 1.0);

    let mut lag = parabolic(&|k| raw(k), best);
    let bpm = |lag: f32| 60.0 * fps / lag;
    let in_pref = |lag: f32| (80.0..=160.0).contains(&bpm(lag));
    let wscore = |lag: f32| near(lag) * prior(bpm(lag));
    // Octave check against double and half tempo.
    for other in [lag / 2.0, lag * 2.0] {
        let ob = bpm(other);
        if !(MIN_BPM * 0.97..=MAX_BPM * 1.03).contains(&ob) {
            continue;
        }
        let (s_me, s_other) = (wscore(lag), wscore(other));
        let switch = if in_pref(lag) && !in_pref(other) {
            s_other >= 1.5 * s_me
        } else if in_pref(other) && !in_pref(lag) {
            s_me < 1.5 * s_other
        } else {
            s_other > s_me
        };
        if switch {
            lag = other;
            break;
        }
    }
    // A pulse whose off-beats are as strong as its beats is the beat itself
    // (e.g. a 174 BPM metronome is not 87 BPM).
    let fast = lag / 2.0;
    if bpm(fast) <= MAX_BPM * 1.03 && near(fast) >= 0.9 * near(lag) {
        lag = fast;
    }
    // Re-centre on the exact peak near the chosen lag.
    let k = lag.round() as usize;
    let k = [k.saturating_sub(1), k, k + 1]
        .into_iter()
        .max_by(|&a, &b| raw(a).total_cmp(&raw(b)))
        .unwrap_or(k);
    lag = parabolic(&|i| raw(i), k);
    Some(Tempo { lag, confidence })
}

/// Beat period (frames) per 20 s window, searched within ±15 % of the global one.
fn tempo_curve(o: &[f32], fps: f32, global: f32) -> Vec<f32> {
    let win = (20.0 * fps) as usize;
    let mut out = Vec::new();
    let mut start = 0;
    while start < o.len() {
        let end = (start + win).min(o.len());
        // Include a little context so short final windows still work.
        let seg = &o[start.saturating_sub(win / 4)..(end + win / 4).min(o.len())];
        let lo = (global * 0.85).floor() as usize;
        let hi = (global * 1.15).ceil() as usize;
        let mut best = None;
        let mut best_v = 0.0;
        if seg.len() > hi * 3 {
            for lag in lo.max(2)..=hi {
                let v = acf(seg, lag);
                if v > best_v {
                    best_v = v;
                    best = Some(lag);
                }
            }
        }
        let local = match best {
            Some(k) if k > lo && k < hi => parabolic(&|i| acf(seg, i), k),
            _ => global,
        };
        // Steady songs: keep the precise global value unless it clearly differs.
        out.push(if (local / global - 1.0).abs() < 0.02 {
            global
        } else {
            local
        });
        start = end;
    }
    out
}

/// Ellis dynamic-programming beat tracker. Returns beat frames.
fn track_beats(o: &[f32], period_at: &dyn Fn(usize) -> f32, global: f32) -> Vec<usize> {
    let n = o.len();
    // Local score: onset smoothed with a Gaussian of width period/32.
    let half = global.round() as isize;
    let kernel: Vec<f32> = (-half..=half)
        .map(|k| (-0.5 * (k as f32 * 32.0 / global).powi(2)).exp())
        .collect();
    let mut local = vec![0f32; n];
    for (i, l) in local.iter_mut().enumerate() {
        let mut s = 0.0;
        for (j, &w) in kernel.iter().enumerate() {
            let idx = i as isize + j as isize - half;
            if idx >= 0 && (idx as usize) < n {
                s += w * o[idx as usize];
            }
        }
        *l = s;
    }
    let max_local = local.iter().fold(0f32, |a, &b| a.max(b));
    let thresh = 0.01 * max_local;
    let mut cum = vec![0f32; n];
    let mut back = vec![-1isize; n];
    let mut first = true;
    for i in 0..n {
        let p = period_at(i).max(2.0);
        let lo = i as isize - (2.0 * p).round() as isize;
        let hi = i as isize - (p / 2.0).round() as isize;
        let mut best = f32::NEG_INFINITY;
        let mut arg = -1isize;
        let mut t = lo.max(0);
        while t <= hi {
            let d = (i as isize - t) as f32;
            let pen = -TIGHTNESS * (d / p).ln().powi(2);
            let v = cum[t as usize] + pen;
            if v > best {
                best = v;
                arg = t;
            }
            t += 1;
        }
        if arg >= 0 {
            cum[i] = local[i] + best;
            back[i] = arg;
        } else {
            cum[i] = local[i];
        }
        if first && local[i] < thresh {
            back[i] = -1;
            cum[i] = local[i];
        } else {
            first = false;
        }
    }
    // Last beat: the last local maximum of cum above half the median of the maxima.
    let maxima: Vec<usize> = (1..n.saturating_sub(1))
        .filter(|&i| cum[i] > cum[i - 1] && cum[i] >= cum[i + 1])
        .collect();
    if maxima.is_empty() {
        return vec![];
    }
    let mut vals: Vec<f32> = maxima.iter().map(|&i| cum[i]).collect();
    vals.sort_by(|a, b| a.total_cmp(b));
    let med = vals[vals.len() / 2];
    let last = maxima
        .iter()
        .rev()
        .find(|&&i| cum[i] >= 0.5 * med)
        .copied()
        .unwrap_or(maxima[maxima.len() - 1]);
    let mut beats = vec![last];
    let mut i = last as isize;
    while back[i as usize] >= 0 {
        i = back[i as usize];
        beats.push(i as usize);
    }
    beats.reverse();
    // Trim weak beats at both ends (fade-ins, silence).
    let w: Vec<f32> = beats.iter().map(|&b| local[b]).collect();
    let rms = (w.iter().map(|v| v * v).sum::<f32>() / w.len().max(1) as f32).sqrt();
    let th = 0.5 * rms;
    let s = w.iter().position(|&v| v >= th).unwrap_or(0);
    let e = w.iter().rposition(|&v| v >= th).map_or(beats.len(), |p| p + 1);
    beats[s..e.max(s)].to_vec()
}

/// Least-squares slope of beat frames vs beat index (the beat period).
fn ls_slope(beats: &[usize]) -> Option<f32> {
    let n = beats.len() as f64;
    if n < 4.0 {
        return None;
    }
    let mx = (n - 1.0) / 2.0;
    let my = beats.iter().map(|&b| b as f64).sum::<f64>() / n;
    let mut num = 0.0;
    let mut den = 0.0;
    for (i, &b) in beats.iter().enumerate() {
        let dx = i as f64 - mx;
        num += dx * (b as f64 - my);
        den += dx * dx;
    }
    (den > 0.0).then(|| (num / den) as f32)
}

/// Bar phase 0..4 with the most bass onset + harmonic change at its beats.
fn downbeat_phase(beats: &[usize], low: &[f32], chroma: &[[u8; 12]]) -> (usize, f32) {
    if beats.len() < 8 {
        return (0, 0.0);
    }
    let n = low.len();
    let bass = |f: usize| {
        let lo = f.saturating_sub(2);
        let hi = (f + 3).min(n);
        low[lo..hi].iter().fold(0f32, |a, &b| a.max(b))
    };
    let mean_chroma = |a: usize, b: usize| {
        let mut m = [0f32; 12];
        for c in &chroma[a.min(chroma.len())..b.min(chroma.len())] {
            for k in 0..12 {
                m[k] += c[k] as f32;
            }
        }
        m
    };
    let cos = |a: &[f32; 12], b: &[f32; 12]| {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if na * nb < 1e-9 {
            1.0
        } else {
            dot / (na * nb)
        }
    };
    let bass_v: Vec<f32> = beats.iter().map(|&b| bass(b)).collect();
    let bmax = bass_v.iter().fold(0f32, |a, &b| a.max(b)).max(1e-6);
    let mut feat = vec![0f32; beats.len()];
    for k in 1..beats.len() - 1 {
        let before = mean_chroma(beats[k - 1], beats[k]);
        let after = mean_chroma(beats[k], beats[k + 1]);
        feat[k] = bass_v[k] / bmax + (1.0 - cos(&before, &after));
    }
    feat[0] = bass_v[0] / bmax;
    let mut score = [0f32; 4];
    let mut count = [0f32; 4];
    for (k, &f) in feat.iter().enumerate() {
        score[k % 4] += f;
        count[k % 4] += 1.0;
    }
    for p in 0..4 {
        score[p] /= count[p].max(1.0);
    }
    let mut order: Vec<usize> = (0..4).collect();
    order.sort_by(|&a, &b| score[b].total_cmp(&score[a]));
    let best = order[0];
    let conf = if score[best] > 1e-6 {
        ((score[best] - score[order[1]]) / score[best]).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (best, conf)
}

/// dB values mapped to 0..255 between the 5th and 95th percentile.
fn to_u8_curve(lin_power: &[f32]) -> Vec<u8> {
    if lin_power.is_empty() {
        return vec![];
    }
    let db: Vec<f32> = lin_power
        .iter()
        .map(|&p| 10.0 * (p.max(1e-12)).log10())
        .collect();
    let mut s = db.clone();
    s.sort_by(|a, b| a.total_cmp(b));
    let pct = |q: f32| s[((s.len() - 1) as f32 * q).round() as usize];
    let lo = pct(0.05);
    let hi = pct(0.95).max(lo + 6.0); // at least a 6 dB span
    db.iter()
        .map(|&v| (((v - lo) / (hi - lo)).clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect()
}

fn energy_curves(rms: &[f32], band_e: &[Vec<f32>; 3], band_n: &[u16]) -> Energy {
    let n = rms.len();
    let band = |i: usize| -> Vec<f32> {
        (0..n)
            .map(|b| {
                let c = band_n.get(b).copied().unwrap_or(0).max(1) as f32;
                band_e[i].get(b).copied().unwrap_or(0.0) / c
            })
            .collect()
    };
    let rms_p: Vec<f32> = rms.iter().map(|r| r * r).collect();
    Energy {
        rms: to_u8_curve(&rms_p),
        low: to_u8_curve(&band(0)),
        mid: to_u8_curve(&band(1)),
        high: to_u8_curve(&band(2)),
    }
}

/// Sections from a checkerboard novelty over 5 Hz energy vectors.
fn sections(e: &Energy, duration_ms: u64) -> Vec<Section> {
    let n10 = e.rms.len();
    if n10 == 0 || duration_ms == 0 {
        return vec![];
    }
    // 5 Hz feature vectors (pairs of 10 Hz points), lightly smoothed.
    let n = n10.div_ceil(2);
    let get = |v: &[u8], i: usize| v.get(i).copied().unwrap_or(0) as f32 / 255.0;
    let mut feat: Vec<[f32; 4]> = (0..n)
        .map(|i| {
            let a = 2 * i;
            let b = (2 * i + 1).min(n10 - 1);
            [
                (get(&e.rms, a) + get(&e.rms, b)) / 2.0,
                (get(&e.low, a) + get(&e.low, b)) / 2.0,
                (get(&e.mid, a) + get(&e.mid, b)) / 2.0,
                (get(&e.high, a) + get(&e.high, b)) / 2.0,
            ]
        })
        .collect();
    // 1 s moving average.
    let sm: Vec<[f32; 4]> = (0..n)
        .map(|i| {
            let lo = i.saturating_sub(2);
            let hi = (i + 3).min(n);
            let mut s = [0f32; 4];
            for f in &feat[lo..hi] {
                for k in 0..4 {
                    s[k] += f[k];
                }
            }
            s.map(|v| v / (hi - lo) as f32)
        })
        .collect();
    feat = sm;
    let dist = |a: &[f32; 4], b: &[f32; 4]| {
        a.iter()
            .zip(b)
            .map(|(x, y)| (x - y) * (x - y))
            .sum::<f32>()
            .sqrt()
    };
    const L: isize = 20; // ±4 s at 5 Hz
    let g: Vec<f32> = (-L..L)
        .map(|k| {
            let x = (k as f32 + 0.5) / (L as f32 / 2.0);
            (-0.5 * x * x).exp()
        })
        .collect();
    let mut nov = vec![0f32; n];
    for (i, nv) in nov.iter_mut().enumerate() {
        let mut s = 0.0;
        for a in -L..L {
            let ia = i as isize + a;
            if ia < 0 || ia as usize >= n {
                continue;
            }
            for b in 0..L {
                // Only cross pairs (a < 0 ≤ b) contribute: same-side pairs are
                // similar by construction under the negative-distance kernel.
                if a >= 0 {
                    break;
                }
                let ib = i as isize + b;
                if ib as usize >= n {
                    break;
                }
                s += g[(a + L) as usize]
                    * g[(b + L) as usize]
                    * dist(&feat[ia as usize], &feat[ib as usize]);
            }
        }
        *nv = s;
    }
    let mean = nov.iter().sum::<f32>() / n as f32;
    let std = (nov.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n as f32).sqrt();
    let th = mean + 0.5 * std;
    let min_gap = 40usize; // 8 s at 5 Hz
    let mut cands: Vec<(usize, f32)> = (1..n.saturating_sub(1))
        .filter(|&i| nov[i] > th && nov[i] >= nov[i - 1] && nov[i] >= nov[i + 1])
        .map(|i| (i, nov[i]))
        .collect();
    cands.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut bounds: Vec<usize> = Vec::new();
    for (i, _) in cands {
        if i < min_gap || i + min_gap > n {
            continue;
        }
        if bounds.iter().all(|&b| b.abs_diff(i) >= min_gap) {
            bounds.push(i);
        }
    }
    bounds.sort_unstable();
    let mut edges = vec![0usize];
    edges.extend(bounds);
    edges.push(n);
    let dur = duration_ms.min(u64::from(u32::MAX)) as u32;
    edges
        .windows(2)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            let m = feat[a..b].iter().map(|f| f[0]).sum::<f32>() / (b - a).max(1) as f32;
            let level = if m >= 0.62 {
                Level::High
            } else if m <= 0.38 {
                Level::Low
            } else {
                Level::Mid
            };
            Section {
                start_ms: (a as u32 * 200).min(dur),
                end_ms: if b == n { dur } else { (b as u32 * 200).min(dur) },
                level,
            }
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::smartlist::SplitMix;

    /// Click track: `bpm` with ±`jitter` (fraction of the period) timing noise,
    /// accented downbeats (with a kick) every 4 beats starting at beat
    /// `bar_phase`, plus background noise. Returns (samples, true beat times ms).
    pub fn click_track(
        bpm: f32,
        seconds: f32,
        rate: u32,
        jitter: f32,
        noise: f32,
        bar_phase: usize,
        seed: u64,
    ) -> (Vec<f32>, Vec<f32>) {
        let mut rng = SplitMix(seed);
        let n = (seconds * rate as f32) as usize;
        let mut s: Vec<f32> = (0..n).map(|_| (rng.unit() * 2.0 - 1.0) * noise).collect();
        let period = 60.0 / bpm;
        let mut t = 0.5f32;
        let mut k = 0usize;
        let mut truth = Vec::new();
        while t < seconds - 0.2 {
            let jit = (rng.unit() * 2.0 - 1.0) * jitter * period;
            let at = t + jit;
            truth.push(at * 1000.0);
            let start = (at * rate as f32) as usize;
            let down = k % 4 == bar_phase;
            let len = (0.03 * rate as f32) as usize;
            for i in 0..len {
                if start + i >= n {
                    break;
                }
                let tt = i as f32 / rate as f32;
                let env = (-tt / 0.008).exp();
                let click = (std::f32::consts::TAU * 2000.0 * tt).sin() * 0.5
                    + (rng.unit() * 2.0 - 1.0) * 0.3;
                let kick = if down {
                    (std::f32::consts::TAU * 60.0 * tt).sin() * (-tt / 0.05).exp() * 0.8
                } else {
                    0.0
                };
                s[start + i] += click * env + kick;
            }
            // The kick rings longer than the click.
            if down {
                for i in len..(0.15 * rate as f32) as usize {
                    if start + i >= n {
                        break;
                    }
                    let tt = i as f32 / rate as f32;
                    s[start + i] +=
                        (std::f32::consts::TAU * 60.0 * tt).sin() * (-tt / 0.05).exp() * 0.8;
                }
            }
            t += period;
            k += 1;
        }
        (s, truth)
    }

    /// Beat-tracking F-measure with a ±70 ms window (MIREX).
    pub fn f_measure(est: &[u32], truth: &[f32]) -> f32 {
        let mut used = vec![false; truth.len()];
        let mut hits = 0;
        for &e in est {
            if let Some(j) = (0..truth.len())
                .filter(|&j| !used[j] && (truth[j] - e as f32).abs() <= 70.0)
                .min_by(|&a, &b| {
                    (truth[a] - e as f32)
                        .abs()
                        .total_cmp(&(truth[b] - e as f32).abs())
                })
            {
                used[j] = true;
                hits += 1;
            }
        }
        if est.is_empty() || truth.is_empty() {
            return 0.0;
        }
        let p = hits as f32 / est.len() as f32;
        let r = hits as f32 / truth.len() as f32;
        if p + r == 0.0 {
            0.0
        } else {
            2.0 * p * r / (p + r)
        }
    }

    #[test]
    fn click_tracks_tempo_and_beats() {
        for (bpm, rate) in [(90.0, 44_100), (120.0, 22_050), (174.0, 48_000), (128.0, 44_100)] {
            let (s, truth) = click_track(bpm, 40.0, rate, 0.01, 0.02, 0, 7);
            let a = analyze(&s, rate);
            let err = (a.bpm / bpm - 1.0).abs();
            assert!(err < 0.01, "{bpm} BPM @ {rate}: got {} ({err})", a.bpm);
            let f = f_measure(&a.beats, &truth);
            assert!(f > 0.9, "{bpm} BPM: F = {f}");
            assert!(a.bpm_confidence > 0.3, "{bpm}: conf {}", a.bpm_confidence);
            // Mean timing error well inside one frame.
            let mean_abs: f32 = a
                .beats
                .iter()
                .map(|&b| {
                    truth
                        .iter()
                        .map(|t| (t - b as f32).abs())
                        .fold(f32::MAX, f32::min)
                })
                .sum::<f32>()
                / a.beats.len() as f32;
            assert!(mean_abs < 25.0, "{bpm}: mean error {mean_abs} ms");
        }
    }

    #[test]
    fn octave_errors_are_avoided() {
        // 60 BPM clicks are 60, not 120; 200 BPM stays 200 (not 100).
        for bpm in [66.0f32, 100.0, 150.0, 195.0] {
            let (s, _) = click_track(bpm, 40.0, 22_050, 0.0, 0.01, 0, 3);
            let a = analyze(&s, 22_050);
            assert!((a.bpm / bpm - 1.0).abs() < 0.01, "{bpm}: {}", a.bpm);
        }
    }

    #[test]
    fn downbeats_follow_the_kick() {
        for phase in 0..4 {
            let (s, truth) = click_track(120.0, 40.0, 22_050, 0.0, 0.01, phase, 11);
            let a = analyze(&s, 22_050);
            let down_truth: Vec<f32> = truth.iter().skip(phase).step_by(4).copied().collect();
            let f = f_measure(&a.downbeats, &down_truth);
            assert!(f > 0.9, "phase {phase}: F = {f}");
        }
    }

    #[test]
    fn noise_and_silence_have_no_confident_beat() {
        let mut rng = SplitMix(5);
        let noise: Vec<f32> = (0..22_050 * 20).map(|_| rng.unit() * 2.0 - 1.0).collect();
        let a = analyze(&noise, 22_050);
        assert!(a.bpm_confidence < 0.5, "noise conf {}", a.bpm_confidence);
        let silent = analyze(&vec![0.0; 22_050 * 10], 22_050);
        assert_eq!(silent.bpm, 0.0);
        assert!(silent.beats.is_empty());
        assert_eq!(silent.duration_ms, 10_000);
        assert_eq!(analyze(&[], 44_100).beats.len(), 0);
    }

    #[test]
    fn sections_find_loud_and_quiet_parts() {
        // 30 s quiet clicks, 30 s loud clicks + noise, 30 s quiet.
        let rate = 22_050;
        let (mut s, _) = click_track(120.0, 90.0, rate, 0.0, 0.005, 0, 1);
        let mut rng = SplitMix(9);
        for (i, v) in s.iter_mut().enumerate() {
            let t = i as f32 / rate as f32;
            if (30.0..60.0).contains(&t) {
                *v = *v * 4.0 + (rng.unit() * 2.0 - 1.0) * 0.4;
            }
        }
        let a = analyze(&s, rate);
        assert!(a.sections.len() >= 3, "{:?}", a.sections);
        let mid = a.section_at(45_000).unwrap();
        assert_eq!(mid.level, Level::High, "{:?}", a.sections);
        assert_ne!(a.section_at(10_000).unwrap().level, Level::High);
        assert_ne!(a.section_at(80_000).unwrap().level, Level::High);
        // A boundary near 30 s and near 60 s.
        for want in [30_000i64, 60_000] {
            assert!(
                a.sections
                    .iter()
                    .any(|s| (i64::from(s.start_ms) - want).abs() < 3_000),
                "{want}: {:?}",
                a.sections
            );
        }
        assert_eq!(a.sections.first().unwrap().start_ms, 0);
        assert_eq!(u64::from(a.sections.last().unwrap().end_ms), a.duration_ms);
        assert!(a.energy_at(45_000) > a.energy_at(10_000));
        assert_eq!(a.energy_10hz.rms.len(), 900);
        assert_eq!(a.energy_10hz.low.len(), a.energy_10hz.rms.len());
    }

    #[test]
    fn json_shape_and_size() {
        let (s, _) = click_track(120.0, 60.0, 44_100, 0.01, 0.02, 0, 2);
        let a = analyze(&s, 44_100);
        let j = serde_json::to_value(&a).unwrap();
        for k in [
            "v",
            "sr",
            "hopMs",
            "bpm",
            "bpmConfidence",
            "tempoCurve",
            "beats",
            "downbeats",
            "onsets",
            "energy10Hz",
            "sections",
        ] {
            assert!(j.get(k).is_some(), "{k}");
        }
        assert!(j["energy10Hz"]["rms"].is_array());
        assert!(j["sections"][0]["level"].is_string());
        let text = serde_json::to_string(&a).unwrap();
        // ~4× this for a 4 minute song: well under the 60 KB budget.
        assert!(text.len() < 16_000, "{}", text.len());
        let back: Analysis = serde_json::from_str(&text).unwrap();
        assert_eq!(back, a);
        let sum = a.summary();
        assert_eq!(sum.beat_count as usize, a.beats.len());
        assert!((sum.bpm - 120.0).abs() < 1.2);
        assert_eq!(sum.version, VERSION);
    }

    #[test]
    fn envelopes_and_pulse() {
        let beats = [1000u32, 1500, 2000];
        assert_eq!(envelope(&beats, 500, 120.0), 0.0);
        assert!((envelope(&beats, 1000, 120.0) - 1.0).abs() < 1e-6);
        let e = envelope(&beats, 1120, 120.0);
        assert!((e - (-1.0f32).exp()).abs() < 1e-4);
        assert!((envelope(&beats, 1500, 120.0) - 1.0).abs() < 1e-6);
        // Pulse: 120 BPM, phase 0: full on each 500 ms, dimmer between.
        assert_eq!(beat_pulse(0, 120.0, 0, 0.5, 100.0), 1.0);
        assert_eq!(beat_pulse(1500, 120.0, 0, 0.5, 100.0), 1.0);
        let mid = beat_pulse(250, 120.0, 0, 0.5, 100.0);
        assert!(mid > 0.5 && mid < 0.6, "{mid}");
        assert_eq!(beat_pulse(250, 0.0, 0, 0.5, 100.0), 1.0);
        assert_eq!(beat_pulse(250, 120.0, 0, 0.0, 100.0), 1.0);
        // Phase shifts the grid; negative phases work.
        assert_eq!(beat_pulse(100, 120.0, 100, 1.0, 50.0), 1.0);
        assert_eq!(beat_pulse(400, 120.0, -100, 1.0, 50.0), 1.0);
        assert_eq!(curve_at(&[0, 255], 50), 0.5);
    }

    #[test]
    fn duration_is_capped() {
        let mut a = Analyzer::new(1000);
        let chunk = vec![0.0f32; 1000 * 60];
        for _ in 0..25 {
            a.push(&chunk);
        }
        assert!(a.is_full());
        assert_eq!(a.duration_ms(), MAX_DURATION_MS);
    }
}
