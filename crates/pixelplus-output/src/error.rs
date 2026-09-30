//! Error type shared by every part of the output crate.

use std::io;

/// Everything that can go wrong while configuring or driving pixel outputs.
///
/// No function in this crate panics on bad input; invalid layouts, timings,
/// frames and missing devices are all reported through this type.
#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    /// The output-to-DPI-bit layout is inconsistent (duplicate bits, bit out
    /// of range, a latch-enable colliding with a data line, ...).
    #[error("invalid output layout: {0}")]
    InvalidLayout(String),

    /// The requested WS281x bit timing or display geometry cannot be realised.
    #[error("invalid DPI timing: {0}")]
    InvalidTiming(String),

    /// A frame carried more outputs than the layout drives.
    #[error("frame has {got} outputs but this board drives {max}")]
    TooManyOutputs {
        /// Outputs present in the frame.
        got: usize,
        /// Outputs the layout supports.
        max: usize,
    },

    /// A frame or framebuffer has the wrong shape.
    #[error("invalid frame: {0}")]
    InvalidFrame(String),

    /// No suitable display device (DRM card with a DPI connector) exists.
    #[error("DPI output device not found: {0}")]
    DeviceNotFound(String),

    /// An I/O error from the kernel (DRM ioctls, mmap, sysfs, ...).
    #[error("{context}: {source}")]
    Io {
        /// What PixelPlus was doing when the error happened.
        context: String,
        /// The underlying OS error.
        #[source]
        source: io::Error,
    },

    /// Waiting for the display to finish a page flip took too long.
    #[error("timed out waiting for {0}")]
    Timeout(String),

    /// `write_frame` was called on a backend that is not started.
    #[error("output is not running; call start() first")]
    NotRunning,

    /// Switching GPIO pins between GPIO and DPI function failed.
    #[error("GPIO pin configuration failed: {0}")]
    PinMux(String),

    /// The operation is not available on this platform or board.
    #[error("unsupported: {0}")]
    Unsupported(String),
}

impl OutputError {
    /// Wrap an [`io::Error`] with a short description of the failed action.
    pub fn io(context: impl Into<String>, source: io::Error) -> Self {
        OutputError::Io {
            context: context.into(),
            source,
        }
    }
}

/// Convenience alias used throughout the crate.
pub type Result<T, E = OutputError> = std::result::Result<T, E>;
