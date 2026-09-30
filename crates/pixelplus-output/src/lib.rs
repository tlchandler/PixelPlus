//! # PixelPlus pixel output
//!
//! WS281x (800 kHz) output for Raspberry Pi pixel controllers through the
//! DPI display peripheral, as designed in `crates/pixelplus-output/DESIGN.md`.
//!
//! Data flow for one frame:
//!
//! ```text
//! show-order RGB per output ──PixelPipeline──▶ wire bytes (OutputFrame)
//!        colour order, brightness, gamma            │
//!                                                   ▼
//!                           PixelOutput::write_frame (DpiOutput / SimOutput / NullOutput)
//!                                                   │ WsEncoder
//!                                                   ▼
//!                  XRGB8888 framebuffer: 1 line = 1 LED of every output
//! ```
//!
//! ```
//! use pixelplus_core::model::BoardKind;
//! use pixelplus_output::{DpiGeometry, OutputFrameRef, OutputLayout, PixelOutput, SimOutput};
//!
//! let layout = OutputLayout::for_board(BoardKind::Difftx);
//! let geometry = DpiGeometry::for_pixels(50)?;
//! // A simulated output that encodes and decodes every frame to prove it.
//! let mut out = SimOutput::verifying(layout, geometry)?;
//! out.start()?;
//! let port1 = [255u8, 0, 0, 0, 255, 0];
//! out.write_frame(&OutputFrameRef::new(vec![&port1]))?;
//! assert_eq!(out.stats().frames, 1);
//! # Ok::<(), pixelplus_output::OutputError>(())
//! ```

#![warn(missing_docs)]

mod backend;
pub mod decoder;
pub mod encoder;
mod error;
mod frame;
pub mod layout;
pub mod patterns;
pub mod pi_config;
mod pipeline;
pub mod timing;

#[cfg(target_os = "linux")]
pub mod pinmux;

#[cfg(target_os = "linux")]
#[path = "backend/dpi.rs"]
mod dpi;

pub use backend::{NullOutput, OutputStats, PixelOutput, SimHandle, SimOutput, SimSnapshot};
pub use decoder::{DecodedFrame, DecodedOutput, WsDecoder};
pub use encoder::{BufferState, EncodeReport, FrameBufferMut, FrameBufferRef, WsEncoder};
pub use error::{OutputError, Result};
pub use frame::{OutputFrame, OutputFrameRef};
pub use layout::{OutputLayout, OutputMode};
pub use patterns::{ScopePattern, TestPattern};
pub use pi_config::DpiSoc;
pub use pipeline::{to_wire_order, PixelPipeline};
pub use timing::{BitTiming, DpiGeometry, Ws281xSpec, PIXEL_CLOCK_HZ};

#[cfg(target_os = "linux")]
pub use dpi::{detect_soc, DpiConfig, DpiOutput};

/// Which backend to construct (mirrors `PIXELPLUS_OUTPUT=dpi|sim|none`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    /// Real hardware through DPI.
    Dpi,
    /// In-memory simulation.
    Sim,
    /// Discard frames.
    None,
}

impl std::str::FromStr for BackendKind {
    type Err = OutputError;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "dpi" => Ok(BackendKind::Dpi),
            "sim" | "simulated" => Ok(BackendKind::Sim),
            "none" | "null" | "off" => Ok(BackendKind::None),
            other => Err(OutputError::Unsupported(format!(
                "unknown output backend `{other}` (expected dpi, sim or none)"
            ))),
        }
    }
}

/// Build a backend for `board`.
///
/// `Sim` returns a plain in-memory [`SimOutput`] (use
/// [`SimOutput::handle`] via [`SimOutput`] directly if you need the preview
/// handle). `Dpi` on a non-Linux platform returns an error.
pub fn create_backend(kind: BackendKind, board: pixelplus_core::model::BoardKind) -> Result<Box<dyn PixelOutput>> {
    let outputs = board.output_count();
    match kind {
        BackendKind::None => Ok(Box::new(NullOutput::with_outputs(outputs))),
        BackendKind::Sim => Ok(Box::new(SimOutput::with_outputs(outputs))),
        #[cfg(target_os = "linux")]
        BackendKind::Dpi => Ok(Box::new(DpiOutput::new(DpiConfig::for_board(board)))),
        #[cfg(not(target_os = "linux"))]
        BackendKind::Dpi => Err(OutputError::Unsupported(
            "DPI output is only available on Linux".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixelplus_core::model::BoardKind;

    #[test]
    fn backend_kind_parsing() {
        assert_eq!("DPI".parse::<BackendKind>().unwrap(), BackendKind::Dpi);
        assert_eq!(" sim ".parse::<BackendKind>().unwrap(), BackendKind::Sim);
        assert_eq!("none".parse::<BackendKind>().unwrap(), BackendKind::None);
        assert!("hdmi".parse::<BackendKind>().is_err());
    }

    #[test]
    fn factory() {
        let mut b = create_backend(BackendKind::Sim, BoardKind::Difftxlarge).unwrap();
        b.start().unwrap();
        assert_eq!(b.stats().outputs, 60);
        assert_eq!(b.stats().backend, "sim");
        let n = create_backend(BackendKind::None, BoardKind::Virtual).unwrap();
        assert_eq!(n.stats().backend, "none");
    }
}
