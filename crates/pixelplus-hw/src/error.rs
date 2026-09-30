//! Error type for hardware access.

use std::io;

/// Everything that can go wrong talking to board hardware.
#[derive(Debug, thiserror::Error)]
pub enum HwError {
    /// An OS-level I/O error (sysfs, device node, ioctl).
    #[error("{context}: {source}")]
    Io {
        /// What PixelPlus was doing.
        context: String,
        /// The underlying OS error.
        #[source]
        source: io::Error,
    },

    /// An I²C transfer to a device failed (usually: nothing at that address).
    #[error("I2C device 0x{addr:02x}: {message}")]
    I2c {
        /// 7-bit device address.
        addr: u8,
        /// What went wrong.
        message: String,
    },

    /// A device or file that should exist does not.
    #[error("not found: {0}")]
    NotFound(String),

    /// Data read from a device is malformed.
    #[error("invalid data: {0}")]
    InvalidData(String),

    /// A caller-supplied value is out of range.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// The operation is not available on this platform or board.
    #[error("unsupported: {0}")]
    Unsupported(String),
}

impl HwError {
    /// Wrap an [`io::Error`] with a short description of the failed action.
    pub fn io(context: impl Into<String>, source: io::Error) -> Self {
        HwError::Io {
            context: context.into(),
            source,
        }
    }

    /// An I²C error for device `addr`.
    pub fn i2c(addr: u8, message: impl Into<String>) -> Self {
        HwError::I2c {
            addr,
            message: message.into(),
        }
    }
}

/// Convenience alias used throughout the crate.
pub type Result<T, E = HwError> = std::result::Result<T, E>;
