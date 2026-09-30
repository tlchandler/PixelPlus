//! In-memory simulated output for development, Docker and the live preview.

use super::{OutputStats, PixelOutput, PresentTiming};
use crate::decoder::{DecodedFrame, WsDecoder};
use crate::encoder::{BufferState, FrameBufferMut, FrameBufferRef, WsEncoder};
use crate::error::{OutputError, Result};
use crate::frame::OutputFrameRef;
use crate::layout::OutputLayout;
use crate::timing::{DpiGeometry, RESET_MIN_NS};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// The most recent frame a [`SimOutput`] received.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SimSnapshot {
    /// Frames received so far (the number of this frame).
    pub frame: u64,
    /// Wire-order bytes per output.
    pub outputs: Vec<Vec<u8>>,
}

/// A cheap, cloneable, thread-safe reader of a [`SimOutput`]'s last frame
/// (e.g. for the web UI preview running on another task).
#[derive(Debug, Clone)]
pub struct SimHandle(Arc<Mutex<SimSnapshot>>);

impl SimHandle {
    fn lock(&self) -> MutexGuard<'_, SimSnapshot> {
        // A poisoned lock only means a writer panicked mid-copy; the data is
        // still plain bytes, so keep serving it.
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Copy of the last frame.
    pub fn snapshot(&self) -> SimSnapshot {
        self.lock().clone()
    }

    /// Number of frames received.
    pub fn frame_number(&self) -> u64 {
        self.lock().frame
    }

    /// Run `f` on the last frame without copying it.
    pub fn with_latest<R>(&self, f: impl FnOnce(&SimSnapshot) -> R) -> R {
        f(&self.lock())
    }
}

#[derive(Debug)]
struct Verifier {
    encoder: WsEncoder,
    decoder: WsDecoder,
    words: Vec<u32>,
    state: BufferState,
    last: Option<DecodedFrame>,
}

/// Keeps the last frame per output in memory; optionally proves every frame
/// by encoding it to a DPI framebuffer and decoding it back.
#[derive(Debug)]
pub struct SimOutput {
    shared: Arc<Mutex<SimSnapshot>>,
    stats: OutputStats,
    max_outputs: Option<usize>,
    verifier: Option<Verifier>,
    vblank: Option<SimVblank>,
    bottom_align: bool,
}

/// A simulated vblank grid (see [`SimOutput::with_refresh`]).
#[derive(Debug, Clone, Copy)]
struct SimVblank {
    epoch: Instant,
    period: Duration,
    /// Vblank the last written frame takes effect at.
    queued: Option<Instant>,
}

impl SimVblank {
    fn at_or_after(&self, t: Instant) -> Instant {
        if t <= self.epoch {
            return self.epoch;
        }
        let p = self.period.as_secs_f64();
        let n = ((t - self.epoch).as_secs_f64() / p).ceil();
        self.epoch + Duration::from_secs_f64(n * p)
    }
}

impl SimOutput {
    /// A simulated output accepting any number of outputs.
    pub fn new() -> Self {
        SimOutput {
            shared: Arc::new(Mutex::new(SimSnapshot::default())),
            stats: OutputStats::new("sim"),
            max_outputs: None,
            verifier: None,
            vblank: None,
            bottom_align: false,
        }
    }

    /// Pretend to scan out on a free-running vblank grid of `period`, like
    /// DPI: [`PixelOutput::present_timing`] reports it, and a frame written
    /// takes effect at the next vblank. Lets the player's presentation-time
    /// pacing run (and be tested) without hardware.
    pub fn with_refresh(mut self, period: Duration) -> Self {
        if !period.is_zero() {
            self.vblank = Some(SimVblank {
                // An arbitrary phase: every node's grid is independent.
                epoch: Instant::now() + period.mul_f64(0.37),
                period,
                queued: None,
            });
            self.stats.refresh_hz = Some(1.0 / period.as_secs_f64());
            self.stats.vblank_period_us = Some(period.as_secs_f64() * 1e6);
        }
        self
    }

    /// A simulated output for a board with `outputs` outputs.
    pub fn with_outputs(outputs: usize) -> Self {
        let mut s = Self::new();
        s.max_outputs = Some(outputs);
        s.stats.outputs = outputs;
        s
    }

    /// A simulated output that encodes every frame for `layout`/`geometry`,
    /// decodes it with the independent wire model and fails the frame if the
    /// data or the WS281x timing is wrong.
    pub fn verifying(layout: OutputLayout, geometry: DpiGeometry) -> Result<Self> {
        let encoder = WsEncoder::new(layout.clone(), geometry)?;
        let words = vec![0u32; geometry.hactive() as usize * geometry.vactive() as usize];
        let mut s = Self::with_outputs(layout.output_count());
        s.stats.refresh_hz = Some(geometry.refresh_hz());
        s.stats.max_pixels_per_output = Some(geometry.pixels_per_output);
        s.verifier = Some(Verifier {
            encoder,
            decoder: WsDecoder::new(layout, geometry),
            words,
            state: BufferState::new(),
            last: None,
        });
        Ok(s)
    }

    /// A handle for reading frames from other threads.
    pub fn handle(&self) -> SimHandle {
        SimHandle(Arc::clone(&self.shared))
    }

    /// The decode of the last verified frame (verifying mode only).
    pub fn last_decoded(&self) -> Option<&DecodedFrame> {
        self.verifier.as_ref().and_then(|v| v.last.as_ref())
    }

    fn verify(verifier: &mut Verifier, frame: &OutputFrameRef<'_>) -> Result<bool> {
        let g = *verifier.encoder.geometry();
        let (w, h) = (g.hactive() as usize, g.vactive() as usize);
        let mut fb = FrameBufferMut::new(&mut verifier.words, w, h, w)?;
        let report = verifier
            .encoder
            .encode(frame, &mut fb, &mut verifier.state)?;
        let fb = FrameBufferRef::new(&verifier.words, w, h, w)?;
        let decoded = verifier.decoder.decode(&fb)?;
        let outcome = decoded.verify(frame, g.pixels_per_output as usize);
        verifier.last = Some(decoded);
        outcome.map_err(|e| OutputError::InvalidFrame(format!("simulation check failed: {e}")))?;
        Ok(report.truncated_outputs > 0)
    }
}

impl Default for SimOutput {
    fn default() -> Self {
        Self::new()
    }
}

impl PixelOutput for SimOutput {
    fn start(&mut self) -> Result<()> {
        self.stats.running = true;
        Ok(())
    }

    fn write_frame(&mut self, frame: &OutputFrameRef<'_>) -> Result<()> {
        if !self.stats.running {
            return Err(OutputError::NotRunning);
        }
        if let Some(max) = self.max_outputs {
            if frame.len() > max {
                let err = OutputError::TooManyOutputs {
                    got: frame.len(),
                    max,
                };
                self.stats.record_error(err.to_string());
                return Err(err);
            }
        }
        let started = Instant::now();
        {
            let mut snap = self.shared.lock().unwrap_or_else(|p| p.into_inner());
            snap.outputs.resize_with(frame.len(), Vec::new);
            for (dst, src) in snap.outputs.iter_mut().zip(frame.iter()) {
                dst.clear();
                dst.extend_from_slice(src);
            }
            snap.frame += 1;
        }
        if let Some(verifier) = self.verifier.as_mut() {
            match Self::verify(verifier, frame) {
                Ok(truncated) => self.stats.truncated_frames += u64::from(truncated),
                Err(e) => {
                    self.stats.record_error(e.to_string());
                    return Err(e);
                }
            }
        }
        self.stats.record_encode(started.elapsed());
        self.stats.frames += 1;
        if let Some(v) = self.vblank.as_mut() {
            let now = Instant::now();
            // One flip per vblank: a frame written while another is queued
            // replaces it at the vblank after (like DPI after its wait).
            let after = match v.queued {
                Some(q) if q > now => q + v.period / 2,
                _ => now,
            };
            let at = v.at_or_after(after + Duration::from_nanos(1));
            v.queued = Some(at);
        }
        Ok(())
    }

    fn stop(&mut self) {
        self.stats.running = false;
    }

    fn stats(&self) -> OutputStats {
        self.stats.clone()
    }

    fn set_bottom_align(&mut self, on: bool) {
        self.bottom_align = on;
        if let Some(v) = self.verifier.as_mut() {
            v.encoder.set_bottom_align(on);
        }
        self.stats.bottom_aligned = on;
    }

    fn present_timing(&self) -> Option<PresentTiming> {
        let v = self.vblank?;
        let now = Instant::now();
        let next = v.at_or_after(now);
        let last = if next > now {
            next.checked_sub(v.period).unwrap_or(v.epoch)
        } else {
            next
        };
        Some(PresentTiming {
            last_vblank: last.max(v.epoch),
            period: v.period,
            pending_vblank: v.queued.filter(|q| *q > now),
            line: Duration::from_nanos(30_625),
            reset: Duration::from_nanos(RESET_MIN_NS as u64),
            bottom_aligned: self.bottom_align,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixelplus_core::model::BoardKind;

    #[test]
    fn keeps_last_frame() {
        let mut sim = SimOutput::with_outputs(4);
        let handle = sim.handle();
        sim.start().unwrap();
        let a = [1u8, 2, 3];
        sim.write_frame(&OutputFrameRef::new(vec![&a])).unwrap();
        let b = [4u8, 5, 6, 7, 8, 9];
        sim.write_frame(&OutputFrameRef::new(vec![&b, &a])).unwrap();
        let snap = handle.snapshot();
        assert_eq!(snap.frame, 2);
        assert_eq!(snap.outputs, vec![b.to_vec(), a.to_vec()]);
        assert_eq!(handle.with_latest(|s| s.outputs.len()), 2);
        assert!(sim.write_frame(&OutputFrameRef::new(vec![&a; 5])).is_err());
        assert_eq!(sim.stats().errors, 1);
    }

    #[test]
    fn verifying_mode_round_trips() {
        let layout = OutputLayout::for_board(BoardKind::Difftxlarge);
        let mut sim = SimOutput::verifying(layout, DpiGeometry::for_pixels(10).unwrap()).unwrap();
        sim.start().unwrap();
        let data: Vec<Vec<u8>> = (0..60).map(|o| vec![o as u8; 30]).collect();
        let frame = OutputFrameRef::new(data.iter().map(Vec::as_slice).collect());
        sim.write_frame(&frame).unwrap();
        // A shorter follow-up frame exercises the incremental encode path.
        let short: Vec<&[u8]> = data.iter().map(|d| &d[..3]).collect();
        sim.write_frame(&OutputFrameRef::new(short)).unwrap();
        assert_eq!(sim.last_decoded().unwrap().outputs[59].bytes, vec![59; 3]);
        // Over-long strings are truncated, counted, and still verify.
        let long = vec![7u8; 3 * 12];
        sim.write_frame(&OutputFrameRef::new(vec![&long])).unwrap();
        let stats = sim.stats();
        assert_eq!(
            (stats.frames, stats.truncated_frames, stats.errors),
            (3, 1, 0)
        );
        assert_eq!(stats.max_pixels_per_output, Some(10));
    }

    #[test]
    fn simulated_vblank_grid() {
        let mut sim = SimOutput::with_outputs(4).with_refresh(Duration::from_millis(10));
        sim.start().unwrap();
        let t = sim.present_timing().unwrap();
        assert_eq!(t.period, Duration::from_millis(10));
        assert!(t.pending_vblank.is_none());
        let px = [1u8, 2, 3];
        sim.write_frame(&OutputFrameRef::new(vec![&px])).unwrap();
        let t = sim.present_timing().unwrap();
        let q = t.pending_vblank.expect("queued for the next vblank");
        assert!(q > Instant::now() && q <= Instant::now() + Duration::from_millis(10));
        // A second frame before that vblank goes to the one after.
        sim.write_frame(&OutputFrameRef::new(vec![&px])).unwrap();
        let q2 = sim.present_timing().unwrap().pending_vblank.unwrap();
        assert_eq!((q2 - q).as_millis(), 10);
        assert!(SimOutput::new().present_timing().is_none());
    }

    #[test]
    fn bottom_aligned_frames_verify_incrementally() {
        let layout = OutputLayout::for_board(BoardKind::Difftx);
        let mut sim = SimOutput::verifying(layout, DpiGeometry::for_pixels(10).unwrap()).unwrap();
        sim.start().unwrap();
        sim.set_bottom_align(true);
        assert!(sim.stats().bottom_aligned);
        // Lengths change from frame to frame (the incremental path must
        // clear and re-place every output).
        for lens in [
            [10usize, 3, 0, 7],
            [2, 9, 4, 1],
            [10, 10, 10, 10],
            [1, 0, 0, 0],
        ] {
            let data: Vec<Vec<u8>> = lens
                .iter()
                .enumerate()
                .map(|(o, &n)| (0..n * 3).map(|i| (i * 7 + o * 31) as u8).collect())
                .collect();
            sim.write_frame(&OutputFrameRef::new(
                data.iter().map(Vec::as_slice).collect(),
            ))
            .unwrap();
            let d = sim.last_decoded().unwrap();
            let ends: Vec<f64> = d.outputs.iter().filter_map(|o| o.data_end_ns).collect();
            let spread = ends.iter().cloned().fold(f64::MIN, f64::max)
                - ends.iter().cloned().fold(f64::MAX, f64::min);
            assert!(
                spread < 1_300.0,
                "{lens:?}: strings latch {spread} ns apart"
            );
        }
        assert_eq!(sim.stats().errors, 0);
    }
}
