//! A backend that accepts and discards frames.

use super::{OutputStats, PixelOutput};
use crate::error::{OutputError, Result};
use crate::frame::OutputFrameRef;

/// Discards frames; used on nodes without pixel outputs (`bare-pi`, `virtual`
/// without preview) and when `PIXELPLUS_OUTPUT=none`.
#[derive(Debug, Clone)]
pub struct NullOutput {
    stats: OutputStats,
    max_outputs: Option<usize>,
}

impl NullOutput {
    /// A null output accepting any number of outputs.
    pub fn new() -> Self {
        NullOutput {
            stats: OutputStats::new("none"),
            max_outputs: None,
        }
    }

    /// A null output that validates frames against `outputs` outputs.
    pub fn with_outputs(outputs: usize) -> Self {
        let mut s = Self::new();
        s.max_outputs = Some(outputs);
        s.stats.outputs = outputs;
        s
    }
}

impl Default for NullOutput {
    fn default() -> Self {
        Self::new()
    }
}

impl PixelOutput for NullOutput {
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
        self.stats.frames += 1;
        Ok(())
    }

    fn stop(&mut self) {
        self.stats.running = false;
    }

    fn stats(&self) -> OutputStats {
        self.stats.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle() {
        let mut out = NullOutput::with_outputs(2);
        let px = [0u8; 3];
        assert!(matches!(
            out.write_frame(&OutputFrameRef::new(vec![&px])),
            Err(OutputError::NotRunning)
        ));
        out.start().unwrap();
        out.write_frame(&OutputFrameRef::new(vec![&px, &px]))
            .unwrap();
        assert!(out.write_frame(&OutputFrameRef::new(vec![&px; 3])).is_err());
        let s = out.stats();
        assert_eq!((s.frames, s.errors, s.running), (1, 1, true));
        out.stop();
        out.stop();
        assert!(!out.stats().running);
    }
}
