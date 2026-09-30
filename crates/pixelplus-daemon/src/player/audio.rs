//! Audio playback: decode (symphonia) → resample to the device rate (rubato)
//! → mix (per-track gain, fades, crossfades, smoothed master volume) → ALSA
//! through cpal.
//!
//! **Audio is the master clock.** The mixer counts the frames it has handed
//! to the device for each track; a track's position is that count minus the
//! device latency, interpolated between callbacks. The lights follow it.
//!
//! Nothing here may stop the lights: if there is no audio device (Docker,
//! development machines, a USB DAC unplugged mid-show) [`AudioEngine`] reports
//! itself unavailable and the engine runs on its monotonic clock instead.
//!
//! Threads: one output thread owns the cpal stream (cpal streams are not
//! `Send`), one decoder thread per playing track fills that track's queue up
//! to ~2 s ahead. The device callback only pops samples, so it stays cheap on
//! a Zero 2 W.

// Without the `audio` feature the mixer is only exercised by tests.
#![cfg_attr(not(feature = "audio"), allow(dead_code))]

use super::clock::equal_power;
use parking_lot::{Condvar, Mutex};
use serde::Serialize;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub type TrackId = u64;

static NEXT_TRACK: AtomicU64 = AtomicU64::new(1);

/// An ALSA output device (for the audio settings page).
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AudioDevice {
    /// Value for `settings.audio.device`.
    pub id: String,
    /// Human readable name.
    pub name: String,
    pub is_default: bool,
}

/// List output devices. Always contains `"default"` first. Never fails
/// (returns just the default entry when audio support is missing).
#[allow(dead_code, unused_mut)]
pub fn list_audio_devices() -> Vec<AudioDevice> {
    let mut out = vec![AudioDevice {
        id: "default".into(),
        name: "System default".into(),
        is_default: true,
    }];
    #[cfg(feature = "audio")]
    {
        use cpal::traits::{DeviceTrait, HostTrait};
        let host = cpal::default_host();
        if let Ok(devices) = host.output_devices() {
            for d in devices {
                if let Ok(name) = d.name() {
                    if name != "default" && !out.iter().any(|x| x.id == name) {
                        out.push(AudioDevice {
                            name: pretty_device_name(&name),
                            id: name,
                            is_default: false,
                        });
                    }
                }
            }
        }
    }
    out
}

#[allow(dead_code)]
fn pretty_device_name(alsa: &str) -> String {
    // "hw:CARD=Headphones,DEV=0" → "Headphones (hw)"
    if let Some((kind, rest)) = alsa.split_once(':') {
        if let Some(card) = rest.strip_prefix("CARD=") {
            let card = card.split(',').next().unwrap_or(card);
            return format!("{card} ({kind})");
        }
    }
    alsa.to_string()
}

/// Master volume percent → linear gain (square law ≈ perceptual taper).
pub fn volume_gain(percent: u8) -> f32 {
    let v = percent.min(100) as f32 / 100.0;
    v * v
}

/// dB → linear gain, limited to ±24 dB.
pub fn db_gain(db: f32) -> f32 {
    if !db.is_finite() {
        return 1.0;
    }
    10f32.powf(db.clamp(-24.0, 24.0) / 20.0)
}

// ---------------------------------------------------------------------------
// Track queue (decoder → mixer)
// ---------------------------------------------------------------------------

/// Stereo interleaved samples at the device rate.
struct TrackQueue {
    buf: Mutex<VecDeque<f32>>,
    cond: Condvar,
    eof: AtomicBool,
    stop: AtomicBool,
    /// Total duration in ms (0 = unknown).
    duration_ms: AtomicU64,
    error: Mutex<Option<String>>,
    /// Upper bound of buffered samples before the decoder waits.
    max_samples: usize,
}

impl TrackQueue {
    fn new(rate: u32) -> Arc<Self> {
        Arc::new(TrackQueue {
            buf: Mutex::new(VecDeque::with_capacity(rate as usize * 4)),
            cond: Condvar::new(),
            eof: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            duration_ms: AtomicU64::new(0),
            error: Mutex::new(None),
            max_samples: rate as usize * 2 * 2,
        })
    }

    /// Push stereo samples, waiting while the queue is full. Returns false when
    /// the track was stopped.
    fn push(&self, samples: &[f32]) -> bool {
        let mut q = self.buf.lock();
        while q.len() >= self.max_samples {
            if self.stop.load(Ordering::Relaxed) {
                return false;
            }
            self.cond.wait_for(&mut q, Duration::from_millis(100));
        }
        q.extend(samples.iter().copied());
        !self.stop.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// Mixer
// ---------------------------------------------------------------------------

struct Track {
    id: TrackId,
    queue: Arc<TrackQueue>,
    gain: f32,
    /// Fade envelope level 0..1 (equal-power curve applied).
    env: f32,
    env_target: f32,
    /// Envelope change per frame.
    env_step: f32,
    /// Remove the track once its fade-out completes.
    remove_after_fade: bool,
    paused: bool,
    /// Frames handed to the device (includes the start offset).
    played: u64,
    finished: bool,
}

/// The mixing core (device independent, unit tested).
pub(crate) struct Mixer {
    rate: u32,
    tracks: Vec<Track>,
    volume: f32,
    volume_target: f32,
    /// Per-frame smoothing coefficient for volume changes (~50 ms).
    volume_coef: f32,
    last_cb: Option<Instant>,
    /// Frames played per track at the start of the last callback.
    latency: Duration,
    last_cb_frames: usize,
    scratch_done: Vec<TrackId>,
}

impl Mixer {
    pub(crate) fn new(rate: u32) -> Self {
        Mixer {
            rate,
            tracks: Vec::new(),
            volume: 0.64,
            volume_target: 0.64,
            volume_coef: 1.0 - (-1.0 / (0.05 * rate as f32)).exp(),
            last_cb: None,
            latency: Duration::ZERO,
            last_cb_frames: 0,
            scratch_done: Vec::new(),
        }
    }

    fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    fn track_mut(&mut self, id: TrackId) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    fn fade(&mut self, id: TrackId, target: f32, ms: u32, remove: bool) {
        let rate = self.rate as f32;
        if let Some(t) = self.track_mut(id) {
            t.env_target = target;
            t.remove_after_fade = remove;
            let frames = (ms as f32 / 1000.0 * rate).max(1.0);
            t.env_step = (target - t.env).abs() / frames;
            if ms == 0 {
                t.env = target;
            }
        }
    }

    /// Mix `frames` frames into `out` (interleaved, `channels` per frame).
    pub(crate) fn mix(&mut self, out: &mut [f32], channels: usize) {
        out.fill(0.0);
        let channels = channels.max(1);
        let frames = out.len() / channels;
        self.last_cb_frames = frames;
        for t in &mut self.tracks {
            if t.paused || t.finished {
                continue;
            }
            let mut q = t.queue.buf.lock();
            let avail = q.len() / 2;
            let n = frames.min(avail);
            for f in 0..n {
                let l = q.pop_front().unwrap_or(0.0);
                let r = q.pop_front().unwrap_or(0.0);
                if t.env != t.env_target {
                    if t.env < t.env_target {
                        t.env = (t.env + t.env_step).min(t.env_target);
                    } else {
                        t.env = (t.env - t.env_step).max(t.env_target);
                    }
                    if (t.env - t.env_target).abs() < 1e-4 {
                        t.env = t.env_target;
                    }
                }
                let g = t.gain * equal_power(t.env);
                let o = &mut out[f * channels..(f + 1) * channels];
                if channels == 1 {
                    o[0] += (l + r) * 0.5 * g;
                } else {
                    o[0] += l * g;
                    o[1] += r * g;
                }
            }
            drop(q);
            t.queue.cond.notify_one();
            t.played += n as u64;
            if n < frames && t.queue.eof.load(Ordering::Acquire) {
                t.finished = true;
            }
        }
        // Master volume (smoothed per frame) and a hard limit.
        for f in 0..frames {
            self.volume += (self.volume_target - self.volume) * self.volume_coef;
            for s in &mut out[f * channels..(f + 1) * channels] {
                *s = (*s * self.volume).clamp(-1.0, 1.0);
            }
        }
        // Drop faded-out tracks.
        self.scratch_done.clear();
        for t in &self.tracks {
            if t.remove_after_fade && t.env <= 0.0 {
                self.scratch_done.push(t.id);
            }
        }
        if !self.scratch_done.is_empty() {
            let done = std::mem::take(&mut self.scratch_done);
            self.tracks.retain(|t| {
                let gone = done.contains(&t.id);
                if gone {
                    t.queue.stop.store(true, Ordering::Relaxed);
                    t.queue.cond.notify_all();
                }
                !gone
            });
            self.scratch_done = done;
        }
    }
}

// ---------------------------------------------------------------------------
// Device output
// ---------------------------------------------------------------------------

struct Output {
    mixer: Mutex<Mixer>,
    rate: u32,
    failed: Mutex<Option<String>>,
    stop: AtomicBool,
}

/// The audio engine. Cheap to query from the output thread.
pub struct AudioEngine {
    out: Option<Arc<Output>>,
    error: Option<String>,
}

impl AudioEngine {
    /// An engine without output (lights-only).
    pub fn disabled(reason: impl Into<String>) -> Self {
        AudioEngine {
            out: None,
            error: Some(reason.into()),
        }
    }

    /// Open `device` ("default" or an ALSA name). Never fails: on error the
    /// engine is unavailable and [`AudioEngine::error`] says why.
    pub fn open(device: &str) -> Self {
        let mut e = Self::disabled("audio not started");
        match open_output(device) {
            Ok(out) => {
                e.out = Some(out);
                e.error = None;
            }
            Err(err) => e.error = Some(err),
        }
        e
    }

    /// True while a device is open and healthy.
    pub fn available(&self) -> bool {
        self.out.as_ref().is_some_and(|o| o.failed.lock().is_none())
    }

    /// Why audio is not available.
    pub fn error(&self) -> Option<String> {
        if let Some(o) = &self.out {
            if let Some(e) = o.failed.lock().clone() {
                return Some(e);
            }
        }
        self.error.clone()
    }

    /// Start playing `path` from `start_ms` with `gain_db`, fading in over
    /// `fade_in_ms`. Returns None when audio is unavailable. Track ids are
    /// unique per process, so ids from a replaced engine never collide.
    pub fn play(
        &mut self,
        path: &Path,
        start_ms: u64,
        gain_db: f32,
        fade_in_ms: u32,
    ) -> Option<TrackId> {
        let out = self
            .out
            .as_ref()
            .filter(|o| o.failed.lock().is_none())?
            .clone();
        let id = NEXT_TRACK.fetch_add(1, Ordering::Relaxed);
        let queue = TrackQueue::new(out.rate);
        spawn_decoder(path.to_path_buf(), start_ms, queue.clone(), out.rate);
        let mut m = out.mixer.lock();
        m.tracks.push(Track {
            id,
            queue,
            gain: db_gain(gain_db),
            env: if fade_in_ms > 0 { 0.0 } else { 1.0 },
            env_target: 1.0,
            env_step: 1.0 / (fade_in_ms.max(1) as f32 / 1000.0 * out.rate as f32),
            remove_after_fade: false,
            paused: false,
            played: start_ms * out.rate as u64 / 1000,
            finished: false,
        });
        Some(id)
    }

    /// Heard position of track `id` in ms. None if the track is unknown or the
    /// device stalled (the caller then falls back to its own clock).
    pub fn position_ms(&self, id: TrackId) -> Option<f64> {
        let out = self.out.as_ref()?;
        if out.failed.lock().is_some() {
            return None;
        }
        let m = out.mixer.lock();
        let t = m.track(id)?;
        let rate = out.rate as f64;
        let played_ms = t.played as f64 * 1000.0 / rate;
        let Some(last) = m.last_cb else {
            return Some(played_ms - m.latency.as_secs_f64() * 1000.0);
        };
        let since = last.elapsed();
        if since > Duration::from_millis(750) {
            return None; // device stalled
        }
        // `played` includes the frames of the last callback; interpolate from
        // its start, never beyond its end.
        let cb_ms = m.last_cb_frames as f64 * 1000.0 / rate;
        let before = played_ms - if t.paused { 0.0 } else { cb_ms };
        let into = if t.paused {
            0.0
        } else {
            (since.as_secs_f64() * 1000.0).min(cb_ms)
        };
        Some((before + into - m.latency.as_secs_f64() * 1000.0).max(0.0))
    }

    /// The track played to its end (decoder finished and the queue drained).
    pub fn is_finished(&self, id: TrackId) -> bool {
        match &self.out {
            Some(o) => o.mixer.lock().track(id).map_or(true, |t| t.finished),
            None => true,
        }
    }

    /// Decoder error of a track (unreadable file).
    pub fn track_error(&self, id: TrackId) -> Option<String> {
        let o = self.out.as_ref()?;
        let m = o.mixer.lock();
        let e = m.track(id)?.queue.error.lock().clone();
        e
    }

    /// Fade a track out over `ms` and remove it.
    pub fn fade_out(&self, id: TrackId, ms: u32) {
        if let Some(o) = &self.out {
            o.mixer.lock().fade(id, 0.0, ms, true);
        }
    }

    /// Stop a track immediately (a 10 ms fade avoids a click).
    pub fn stop(&self, id: TrackId) {
        self.fade_out(id, 10);
    }

    /// Fade out every track.
    pub fn stop_all(&self, fade_ms: u32) {
        if let Some(o) = &self.out {
            let mut m = o.mixer.lock();
            let ids: Vec<_> = m.tracks.iter().map(|t| t.id).collect();
            for id in ids {
                m.fade(id, 0.0, fade_ms.max(10), true);
            }
        }
    }

    pub fn set_paused(&self, id: TrackId, paused: bool) {
        if let Some(o) = &self.out {
            if let Some(t) = o.mixer.lock().track_mut(id) {
                t.paused = paused;
            }
        }
    }

    /// Master volume 0..=100 (smoothed).
    pub fn set_volume(&self, percent: u8) {
        if let Some(o) = &self.out {
            o.mixer.lock().volume_target = volume_gain(percent);
        }
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        if let Some(o) = &self.out {
            o.stop.store(true, Ordering::Relaxed);
            let m = o.mixer.lock();
            for t in &m.tracks {
                t.queue.stop.store(true, Ordering::Relaxed);
                t.queue.cond.notify_all();
            }
        }
    }
}

#[cfg(not(feature = "audio"))]
fn open_output(_device: &str) -> Result<Arc<Output>, String> {
    Err("this build has no audio support".into())
}

#[cfg(feature = "audio")]
fn open_output(device: &str) -> Result<Arc<Output>, String> {
    let (tx, rx) = std::sync::mpsc::channel::<Result<Arc<Output>, String>>();
    let device = device.to_string();
    std::thread::Builder::new()
        .name("pp-audio".into())
        .spawn(move || cpal_thread(device, tx))
        .map_err(|e| format!("could not start the audio thread: {e}"))?;
    rx.recv_timeout(Duration::from_secs(5))
        .map_err(|_| "the audio device did not respond".to_string())?
}

#[cfg(feature = "audio")]
fn cpal_thread(device_name: String, tx: std::sync::mpsc::Sender<Result<Arc<Output>, String>>) {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();
    let device = if device_name.is_empty() || device_name == "default" {
        host.default_output_device()
    } else {
        host.output_devices()
            .ok()
            .and_then(|mut it| it.find(|d| d.name().is_ok_and(|n| n == device_name)))
            .or_else(|| {
                tracing::warn!("audio device '{device_name}' not found; using the default device");
                host.default_output_device()
            })
    };
    let Some(device) = device else {
        let _ = tx.send(Err("no audio output device found".into()));
        return;
    };
    let supported = match device.default_output_config() {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(Err(format!("the audio device cannot play ({e})")));
            return;
        }
    };
    let format = supported.sample_format();
    let mut config: cpal::StreamConfig = supported.config();
    // Prefer 44.1 kHz (most songs) so no resampling is needed, if supported.
    if let Ok(mut ranges) = device.supported_output_configs() {
        if ranges.any(|r| {
            r.sample_format() == format
                && r.channels() == config.channels
                && r.min_sample_rate().0 <= 44_100
                && r.max_sample_rate().0 >= 44_100
        }) {
            config.sample_rate = cpal::SampleRate(44_100);
        }
    }
    let rate = config.sample_rate.0;
    let channels = config.channels as usize;
    let out = Arc::new(Output {
        mixer: Mutex::new(Mixer::new(rate)),
        rate,
        failed: Mutex::new(None),
        stop: AtomicBool::new(false),
    });
    let err_out = out.clone();
    let on_error = move |e: cpal::StreamError| {
        tracing::warn!("audio device error: {e}");
        *err_out.failed.lock() = Some(format!("the audio device stopped working ({e})"));
    };
    let stream = match format {
        cpal::SampleFormat::F32 => {
            build_stream::<f32>(&device, &config, channels, out.clone(), on_error)
        }
        cpal::SampleFormat::I16 => {
            build_stream::<i16>(&device, &config, channels, out.clone(), on_error)
        }
        cpal::SampleFormat::U16 => {
            build_stream::<u16>(&device, &config, channels, out.clone(), on_error)
        }
        cpal::SampleFormat::I32 => {
            build_stream::<i32>(&device, &config, channels, out.clone(), on_error)
        }
        other => Err(format!("unsupported audio sample format {other:?}")),
    };
    let stream = match stream {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(Err(e));
            return;
        }
    };
    if let Err(e) = stream.play() {
        let _ = tx.send(Err(format!("could not start audio ({e})")));
        return;
    }
    tracing::info!(
        "audio output: {} at {rate} Hz, {channels} ch",
        device.name().unwrap_or_else(|_| "?".into())
    );
    let _ = tx.send(Ok(out.clone()));
    while !out.stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(stream);
}

#[cfg(feature = "audio")]
fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    out: Arc<Output>,
    on_error: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    use cpal::traits::DeviceTrait;
    let mut scratch: Vec<f32> = Vec::new();
    device
        .build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                scratch.resize(data.len(), 0.0);
                let mut m = out.mixer.lock();
                let ts = info.timestamp();
                if let Some(lat) = ts.playback.duration_since(&ts.callback) {
                    // Smooth the latency estimate.
                    let old = m.latency.as_secs_f64();
                    let new = if old == 0.0 {
                        lat.as_secs_f64()
                    } else {
                        old * 0.9 + lat.as_secs_f64() * 0.1
                    };
                    m.latency = Duration::from_secs_f64(new);
                }
                m.last_cb = Some(Instant::now());
                m.mix(&mut scratch, channels);
                drop(m);
                for (d, s) in data.iter_mut().zip(scratch.iter()) {
                    *d = T::from_sample(*s);
                }
            },
            on_error,
            None,
        )
        .map_err(|e| format!("could not open the audio device ({e})"))
}

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

fn spawn_decoder(path: PathBuf, start_ms: u64, queue: Arc<TrackQueue>, rate: u32) {
    let q2 = queue.clone();
    let r = std::thread::Builder::new()
        .name("pp-decode".into())
        .spawn(move || {
            if let Err(e) = decode_into(&path, start_ms, &q2, rate) {
                tracing::warn!("audio {}: {e}", path.display());
                *q2.error.lock() = Some(e);
            }
            q2.eof.store(true, Ordering::Release);
        });
    if let Err(e) = r {
        *queue.error.lock() = Some(format!("could not start the decoder: {e}"));
        queue.eof.store(true, Ordering::Release);
    }
}

/// Decode `path` from `start_ms`, resample to `rate` stereo and push into `queue`.
fn decode_into(path: &Path, start_ms: u64, queue: &TrackQueue, rate: u32) -> Result<(), String> {
    use rubato::Resampler;
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
    use symphonia::core::errors::Error as SymErr;
    use symphonia::core::formats::{FormatOptions, SeekMode, SeekTo};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;
    use symphonia::core::units::Time;

    let file =
        std::fs::File::open(path).map_err(|e| format!("cannot open the audio file ({e})"))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )
        .map_err(|e| format!("unsupported or damaged audio file ({e})"))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or("the file has no audio track")?
        .clone();
    let track_id = track.id;
    let src_rate = track.codec_params.sample_rate.unwrap_or(44_100);
    if let Some(n) = track.codec_params.n_frames {
        queue
            .duration_ms
            .store(n * 1000 / src_rate.max(1) as u64, Ordering::Relaxed);
    }
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("unsupported audio codec ({e})"))?;

    // Seek; then skip the remainder to the exact frame.
    let mut skip_frames: u64 = 0;
    if start_ms > 0 {
        let time = Time::from(start_ms as f64 / 1000.0);
        match format.seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time,
                track_id: Some(track_id),
            },
        ) {
            Ok(seeked) => {
                let diff = seeked.required_ts.saturating_sub(seeked.actual_ts);
                skip_frames = match track.codec_params.time_base {
                    Some(tb) => {
                        let t = tb.calc_time(diff);
                        ((t.seconds as f64 + t.frac) * src_rate as f64) as u64
                    }
                    None => diff,
                };
            }
            Err(_) => {
                // Not seekable: decode and drop from the start.
                skip_frames = start_ms * src_rate as u64 / 1000;
            }
        }
    }

    const CHUNK: usize = 1024;
    let mut resampler = if src_rate != rate {
        Some(
            rubato::FftFixedIn::<f32>::new(src_rate as usize, rate as usize, CHUNK, 2, 2)
                .map_err(|e| format!("cannot resample {src_rate} Hz audio ({e})"))?,
        )
    } else {
        None
    };
    let mut pending: [Vec<f32>; 2] = [Vec::with_capacity(CHUNK * 2), Vec::with_capacity(CHUNK * 2)];
    let mut out_interleaved: Vec<f32> = Vec::with_capacity(CHUNK * 4);
    let mut sample_buf: Option<SampleBuffer<f32>> = None;
    let mut decode_errors = 0u32;

    let flush = |pending: &mut [Vec<f32>; 2],
                 resampler: &mut Option<rubato::FftFixedIn<f32>>,
                 out: &mut Vec<f32>,
                 last: bool|
     -> Result<bool, String> {
        match resampler {
            None => {
                interleave(&pending[0], &pending[1], out);
                pending[0].clear();
                pending[1].clear();
                Ok(queue.push(out))
            }
            Some(rs) => loop {
                let need = rs.input_frames_next();
                if pending[0].len() < need {
                    if !last || pending[0].is_empty() {
                        return Ok(true);
                    }
                    let res = rs
                        .process_partial(Some(&[&pending[0][..], &pending[1][..]]), None)
                        .map_err(|e| e.to_string())?;
                    pending[0].clear();
                    pending[1].clear();
                    interleave(&res[0], &res[1], out);
                    return Ok(queue.push(out));
                }
                let res = rs
                    .process(&[&pending[0][..need], &pending[1][..need]], None)
                    .map_err(|e| e.to_string())?;
                pending[0].drain(..need);
                pending[1].drain(..need);
                interleave(&res[0], &res[1], out);
                if !queue.push(out) {
                    return Ok(false);
                }
            },
        }
    };

    loop {
        if queue.stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymErr::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymErr::ResetRequired) => break,
            Err(e) => {
                if decode_errors > 0 || skip_frames > 0 {
                    break;
                }
                return Err(format!("could not read the audio file ({e})"));
            }
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymErr::DecodeError(_)) | Err(SymErr::IoError(_)) => {
                decode_errors += 1;
                if decode_errors > 50 {
                    return Err("the audio file is damaged".into());
                }
                continue;
            }
            Err(e) => return Err(format!("audio decode failed ({e})")),
        };
        let spec = *decoded.spec();
        let chans = spec.channels.count().max(1);
        let sb = sample_buf
            .get_or_insert_with(|| SampleBuffer::<f32>::new(decoded.capacity() as u64, spec));
        if sb.capacity() < decoded.capacity() * chans {
            *sb = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        }
        sb.copy_interleaved_ref(decoded);
        let samples = sb.samples();
        let frames = samples.len() / chans;
        let mut start = 0usize;
        if skip_frames > 0 {
            let s = (skip_frames as usize).min(frames);
            skip_frames -= s as u64;
            start = s;
        }
        for f in start..frames {
            let l = samples[f * chans];
            let r = if chans > 1 { samples[f * chans + 1] } else { l };
            pending[0].push(l);
            pending[1].push(r);
        }
        if pending[0].len() >= CHUNK
            && !flush(&mut pending, &mut resampler, &mut out_interleaved, false)?
        {
            return Ok(());
        }
    }
    flush(&mut pending, &mut resampler, &mut out_interleaved, true)?;
    Ok(())
}

fn interleave(l: &[f32], r: &[f32], out: &mut Vec<f32>) {
    out.clear();
    for (a, b) in l.iter().zip(r) {
        out.push(*a);
        out.push(*b);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Write a 16-bit PCM WAV (tests).
#[cfg(test)]
pub(crate) fn write_test_wav(path: &Path, rate: u32, seconds: f32, freq: f32, amplitude: f32) {
    let n = (rate as f32 * seconds) as u32;
    let mut data = Vec::with_capacity(44 + n as usize * 4);
    let byte_rate = rate * 4;
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&(36 + n * 4).to_le_bytes());
    data.extend_from_slice(b"WAVEfmt ");
    data.extend_from_slice(&16u32.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&2u16.to_le_bytes());
    data.extend_from_slice(&rate.to_le_bytes());
    data.extend_from_slice(&byte_rate.to_le_bytes());
    data.extend_from_slice(&4u16.to_le_bytes());
    data.extend_from_slice(&16u16.to_le_bytes());
    data.extend_from_slice(b"data");
    data.extend_from_slice(&(n * 4).to_le_bytes());
    for i in 0..n {
        let v = ((i as f32 / rate as f32 * freq * std::f32::consts::TAU).sin()
            * amplitude
            * 32767.0) as i16;
        data.extend_from_slice(&v.to_le_bytes());
        data.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, data).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("pp-audio-{}", pixelplus_core::model::new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn drain(queue: &TrackQueue) -> Vec<f32> {
        let mut all = Vec::new();
        loop {
            {
                let mut q = queue.buf.lock();
                all.extend(q.drain(..));
            }
            queue.cond.notify_all();
            if queue.eof.load(Ordering::Acquire) && queue.buf.lock().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        all
    }

    #[test]
    fn decodes_wav_without_resampling() {
        let d = tmp();
        let p = d.join("a.wav");
        write_test_wav(&p, 44_100, 1.0, 440.0, 0.5);
        let q = TrackQueue::new(44_100);
        let q2 = q.clone();
        let h = std::thread::spawn(move || {
            decode_into(&p, 0, &q2, 44_100).map(|_| q2.eof.store(true, Ordering::Release))
        });
        let samples = drain(&q);
        h.join().unwrap().unwrap();
        assert_eq!(samples.len(), 44_100 * 2);
        assert_eq!(q.duration_ms.load(Ordering::Relaxed), 1000);
        let peak = samples.iter().fold(0f32, |a, &s| a.max(s.abs()));
        assert!((peak - 0.5).abs() < 0.01, "peak {peak}");
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn decodes_with_seek_and_resampling() {
        let d = tmp();
        let p = d.join("a.wav");
        write_test_wav(&p, 44_100, 2.0, 440.0, 0.5);
        let q = TrackQueue::new(48_000);
        let q2 = q.clone();
        let p2 = p.clone();
        let h = std::thread::spawn(move || {
            decode_into(&p2, 500, &q2, 48_000).map(|_| q2.eof.store(true, Ordering::Release))
        });
        let samples = drain(&q);
        h.join().unwrap().unwrap();
        let frames = samples.len() / 2;
        // 1.5 s at 48 kHz, allowing for resampler delay/padding.
        assert!((frames as i64 - 72_000).abs() < 3_000, "frames {frames}");
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn bad_files_report_errors() {
        let d = tmp();
        let q = TrackQueue::new(44_100);
        assert!(decode_into(&d.join("missing.mp3"), 0, &q, 44_100).is_err());
        let bad = d.join("bad.mp3");
        std::fs::write(&bad, vec![0x42u8; 4096]).unwrap();
        assert!(decode_into(&bad, 0, &q, 44_100).is_err());
        std::fs::remove_dir_all(d).ok();
    }

    fn mixer_with_track(values: &[f32]) -> (Mixer, Arc<TrackQueue>) {
        let mut m = Mixer::new(1000);
        m.volume = 1.0;
        m.volume_target = 1.0;
        let q = TrackQueue::new(1000);
        q.buf.lock().extend(values.iter().copied());
        m.tracks.push(Track {
            id: 1,
            queue: q.clone(),
            gain: 1.0,
            env: 1.0,
            env_target: 1.0,
            env_step: 0.0,
            remove_after_fade: false,
            paused: false,
            played: 0,
            finished: false,
        });
        (m, q)
    }

    #[test]
    fn mixer_counts_frames_and_finishes() {
        let (mut m, q) = mixer_with_track(&[0.5; 20]);
        let mut out = vec![0f32; 8];
        m.mix(&mut out, 2);
        assert_eq!(m.tracks[0].played, 4);
        assert!(out.iter().all(|&s| (s - 0.5).abs() < 1e-6));
        // Mono device: averaged.
        let mut mono = vec![0f32; 4];
        m.mix(&mut mono, 1);
        assert_eq!(m.tracks[0].played, 8);
        assert!((mono[0] - 0.5).abs() < 1e-6);
        // Underrun without eof: silence, not finished, position holds.
        let mut out = vec![0f32; 8];
        m.mix(&mut out, 2);
        m.mix(&mut out, 2);
        assert_eq!(m.tracks[0].played, 10);
        assert!(!m.tracks[0].finished);
        q.eof.store(true, Ordering::Release);
        m.mix(&mut out, 2);
        assert!(m.tracks[0].finished);
    }

    #[test]
    fn mixer_fades_and_removes() {
        let (mut m, _q) = mixer_with_track(&[1.0; 2000]);
        m.fade(1, 0.0, 100, true); // 100 ms at 1 kHz = 100 frames
        let mut out = vec![0f32; 100];
        m.mix(&mut out, 2); // 50 frames: halfway
        let level = m.tracks[0].env;
        assert!((level - 0.5).abs() < 0.02, "env {level}");
        // Equal-power: half-way level is sin(π/4) ≈ 0.707.
        assert!((out[98] - equal_power(level)).abs() < 0.02);
        m.mix(&mut out, 2);
        assert!(m.tracks.is_empty(), "removed after fade-out");
    }

    #[test]
    fn master_volume_is_smoothed() {
        let (mut m, _q) = mixer_with_track(&[1.0; 20_000]);
        m.volume_target = 0.0;
        let mut out = vec![0f32; 2];
        m.mix(&mut out, 2);
        assert!(out[0] > 0.9, "no jump on the first frame");
        let mut out = vec![0f32; 2000];
        m.mix(&mut out, 2);
        assert!(out[1998].abs() < 0.01, "reaches the target");
    }

    #[test]
    fn gains() {
        assert_eq!(volume_gain(0), 0.0);
        assert_eq!(volume_gain(100), 1.0);
        assert!((volume_gain(50) - 0.25).abs() < 1e-6);
        assert!((db_gain(-6.0) - 0.501).abs() < 0.01);
        assert_eq!(db_gain(f32::NAN), 1.0);
        assert!((db_gain(100.0) - db_gain(24.0)).abs() < 1e-6);
        assert_eq!(
            pretty_device_name("hw:CARD=Headphones,DEV=0"),
            "Headphones (hw)"
        );
        assert!(list_audio_devices().iter().any(|d| d.id == "default"));
    }

    #[test]
    fn disabled_engine_is_harmless() {
        let mut e = AudioEngine::disabled("no device");
        assert!(!e.available());
        assert_eq!(e.error().as_deref(), Some("no device"));
        assert!(e.play(Path::new("/nope.mp3"), 0, 0.0, 0).is_none());
        assert!(e.position_ms(1).is_none());
        assert!(e.is_finished(1));
        e.set_volume(50);
        e.stop_all(100);
    }
}
