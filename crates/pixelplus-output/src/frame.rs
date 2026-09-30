//! Per-output pixel byte buffers handed to an output backend.
//!
//! Bytes are in **wire order**: colour order, brightness and gamma have
//! already been applied (see [`crate::PixelPipeline`]). Output *i* of the
//! frame is board output *i + 1*. An output may carry any number of bytes;
//! a trailing partial LED is padded with zeros.

use crate::error::{OutputError, Result};

/// A borrowed frame: one byte slice per output.
#[derive(Debug, Clone, Default)]
pub struct OutputFrameRef<'a> {
    outputs: Vec<&'a [u8]>,
}

impl<'a> OutputFrameRef<'a> {
    /// A frame from one slice per output.
    pub fn new(outputs: Vec<&'a [u8]>) -> Self {
        OutputFrameRef { outputs }
    }

    /// A frame from an output-major contiguous buffer (the `.ppseq` layout):
    /// output 0's `pixel_counts[0] × 3` bytes, then output 1's, and so on.
    pub fn from_contiguous(data: &'a [u8], pixel_counts: &[u32]) -> Result<Self> {
        let mut outputs = Vec::with_capacity(pixel_counts.len());
        let mut offset = 0usize;
        for (i, &count) in pixel_counts.iter().enumerate() {
            let len = count as usize * 3;
            let end = offset
                .checked_add(len)
                .filter(|&e| e <= data.len())
                .ok_or_else(|| {
                    OutputError::InvalidFrame(format!(
                        "output {} needs bytes {offset}..{} but the frame has {}",
                        i + 1,
                        offset.saturating_add(len),
                        data.len()
                    ))
                })?;
            outputs.push(&data[offset..end]);
            offset = end;
        }
        Ok(OutputFrameRef { outputs })
    }

    /// Number of outputs in the frame.
    pub fn len(&self) -> usize {
        self.outputs.len()
    }

    /// `true` when the frame has no outputs.
    pub fn is_empty(&self) -> bool {
        self.outputs.is_empty()
    }

    /// Bytes of output `index` (0-based); empty when out of range.
    pub fn output(&self, index: usize) -> &'a [u8] {
        self.outputs.get(index).copied().unwrap_or(&[])
    }

    /// Iterate over the per-output byte slices.
    pub fn iter(&self) -> impl Iterator<Item = &'a [u8]> + '_ {
        self.outputs.iter().copied()
    }

    /// The longest output, in LEDs (rounded up).
    pub fn max_pixels(&self) -> usize {
        self.outputs
            .iter()
            .map(|o| o.len().div_ceil(3))
            .max()
            .unwrap_or(0)
    }
}

/// An owned frame; reuse it between frames to avoid allocations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutputFrame {
    /// Wire-order bytes per output.
    pub outputs: Vec<Vec<u8>>,
}

impl OutputFrame {
    /// `outputs` outputs of `pixels` black LEDs each.
    pub fn black(outputs: usize, pixels: usize) -> Self {
        OutputFrame {
            outputs: vec![vec![0; pixels * 3]; outputs],
        }
    }

    /// Resize to `outputs` outputs of `pixels` LEDs, keeping allocations.
    pub fn reshape(&mut self, outputs: usize, pixels: usize) {
        self.outputs.resize_with(outputs, Vec::new);
        for o in &mut self.outputs {
            o.resize(pixels * 3, 0);
        }
    }

    /// Borrow as an [`OutputFrameRef`].
    pub fn as_frame_ref(&self) -> OutputFrameRef<'_> {
        OutputFrameRef::new(self.outputs.iter().map(Vec::as_slice).collect())
    }
}

impl<'a> From<&'a OutputFrame> for OutputFrameRef<'a> {
    fn from(frame: &'a OutputFrame) -> Self {
        frame.as_frame_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contiguous_split() {
        let data: Vec<u8> = (0..15).collect();
        let f = OutputFrameRef::from_contiguous(&data, &[2, 0, 3]).unwrap();
        assert_eq!(f.len(), 3);
        assert_eq!(f.output(0), &[0, 1, 2, 3, 4, 5]);
        assert!(f.output(1).is_empty());
        assert_eq!(f.output(2), &data[6..15]);
        assert!(f.output(9).is_empty());
        assert_eq!(f.max_pixels(), 3);
    }

    #[test]
    fn contiguous_too_short() {
        let data = [0u8; 5];
        assert!(OutputFrameRef::from_contiguous(&data, &[2]).is_err());
        assert!(OutputFrameRef::from_contiguous(&data, &[u32::MAX, u32::MAX]).is_err());
    }

    #[test]
    fn owned_reshape() {
        let mut f = OutputFrame::black(2, 4);
        f.reshape(3, 1);
        assert_eq!(f.outputs.len(), 3);
        assert!(f.outputs.iter().all(|o| o.len() == 3));
        assert_eq!(f.as_frame_ref().max_pixels(), 1);
    }
}
