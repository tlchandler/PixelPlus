//! Output backends: where encoded frames go.
//!
//! * [`crate::DpiOutput`] — the real thing: a DRM/KMS dumb buffer scanned out
//!   by the Pi's DPI peripheral (Linux only).
//! * [`SimOutput`] — keeps the last frame in memory for the UI preview and,
//!   optionally, encodes and decodes every frame to prove timing and data.
//! * [`NullOutput`] — discards frames (show director / audio-only nodes).

mod null;
mod sim;

pub use null::NullOutput;
pub use sim::{SimHandle, SimOutput, SimSnapshot};

use crate::error::Result;
use crate::frame::OutputFrameRef;
use serde::Serialize;
use std::time::Duration;

/// Running statistics of a backend (serialised camelCase for the API).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputStats {
    /// Backend name: `dpi`, `sim` or `none`.
    pub backend: String,
    /// `start()` succeeded and `stop()` has not been called.
    pub running: bool,
    /// Frames accepted by `write_frame`.
    pub frames: u64,
    /// Frames rejected with an error.
    pub errors: u64,
    /// Frames in which at least one output was longer than the geometry allows.
    pub truncated_frames: u64,
    /// Encode time of the last frame, microseconds.
    pub last_encode_us: u64,
    /// Longest encode time seen, microseconds.
    pub max_encode_us: u64,
    /// Exponentially smoothed encode time, microseconds.
    pub avg_encode_us: f64,
    /// Time `write_frame` spent waiting for the previous page flip, microseconds (last frame).
    pub last_wait_us: u64,
    /// Display refresh rate (maximum achievable frame rate), if known.
    pub refresh_hz: Option<f64>,
    /// Outputs this backend drives.
    pub outputs: usize,
    /// Longest string (LEDs per output) that fits one frame, if limited.
    pub max_pixels_per_output: Option<u32>,
    /// Most recent error message, if any.
    pub last_error: Option<String>,
}

impl OutputStats {
    /// Fresh statistics for backend `name`.
    pub fn new(name: &str) -> Self {
        OutputStats {
            backend: name.to_string(),
            ..Self::default()
        }
    }

    /// Record one frame's encode time.
    pub fn record_encode(&mut self, elapsed: Duration) {
        let us = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        self.last_encode_us = us;
        self.max_encode_us = self.max_encode_us.max(us);
        self.avg_encode_us = if self.frames == 0 {
            us as f64
        } else {
            self.avg_encode_us * 0.95 + us as f64 * 0.05
        };
    }

    /// Record a failed frame.
    pub fn record_error(&mut self, message: impl Into<String>) {
        self.errors += 1;
        self.last_error = Some(message.into());
    }
}

/// A destination for pixel frames.
///
/// Call order: `start` → `write_frame`* → `stop`. `write_frame` may block
/// for up to one display refresh (DPI waits for the previous page flip), so
/// run it on a dedicated output thread. Implementations never panic on bad
/// frames; they return an error and the next frame is attempted normally.
pub trait PixelOutput: Send {
    /// Open devices and begin output.
    fn start(&mut self) -> Result<()>;

    /// Show one frame of wire-order bytes (see [`crate::PixelPipeline`]).
    fn write_frame(&mut self, frame: &OutputFrameRef<'_>) -> Result<()>;

    /// Blank all outputs and release devices. Safe to call more than once.
    fn stop(&mut self);

    /// Current statistics.
    fn stats(&self) -> OutputStats;
}

impl<T: PixelOutput + ?Sized> PixelOutput for Box<T> {
    fn start(&mut self) -> Result<()> {
        (**self).start()
    }

    fn write_frame(&mut self, frame: &OutputFrameRef<'_>) -> Result<()> {
        (**self).write_frame(frame)
    }

    fn stop(&mut self) {
        (**self).stop()
    }

    fn stats(&self) -> OutputStats {
        (**self).stats()
    }
}
