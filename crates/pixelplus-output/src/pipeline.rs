//! Per-output colour processing: colour order, brightness and gamma.
//!
//! The pipeline turns show-order RGB bytes (as rendered from the `.fseq`,
//! effects and overlays) into wire-order bytes for the encoder. Brightness
//! and gamma are folded into one 256-entry lookup table per output, so the
//! per-byte cost is a table lookup.

use crate::frame::{OutputFrame, OutputFrameRef};
use pixelplus_core::model::{ColorOrder, OutputConfig};

/// Gamma values outside this range are clamped (and non-finite ones treated as 1.0).
pub const GAMMA_RANGE: (f32, f32) = (0.1, 5.0);

#[derive(Debug, Clone)]
struct Stage {
    order: [usize; 3],
    enabled: bool,
    brightness: u8,
    gamma: f32,
    lut: [u8; 256],
}

impl Stage {
    fn new(cfg: &OutputConfig, master: u8) -> Self {
        let mut stage = Stage {
            order: cfg.color_order.source_indices(),
            enabled: cfg.enabled,
            brightness: cfg.brightness.min(100),
            gamma: sanitize_gamma(cfg.gamma),
            lut: [0; 256],
        };
        stage.rebuild(master);
        stage
    }

    fn rebuild(&mut self, master: u8) {
        self.lut = build_lut(self.gamma, self.brightness, master);
    }
}

fn sanitize_gamma(gamma: f32) -> f32 {
    if gamma.is_finite() {
        gamma.clamp(GAMMA_RANGE.0, GAMMA_RANGE.1)
    } else {
        1.0
    }
}

/// `out = round(255 × (in / 255)^gamma × brightness% × master%)`.
fn build_lut(gamma: f32, brightness: u8, master: u8) -> [u8; 256] {
    let scale = f64::from(brightness.min(100)) / 100.0 * f64::from(master.min(100)) / 100.0;
    let gamma = f64::from(gamma);
    let mut lut = [0u8; 256];
    for (i, v) in lut.iter_mut().enumerate() {
        let x = i as f64 / 255.0;
        let y = if (gamma - 1.0).abs() < f64::EPSILON {
            x
        } else {
            x.powf(gamma)
        };
        *v = (y * scale * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    lut
}

/// Colour order + brightness + gamma for every output of a node.
#[derive(Debug, Clone)]
pub struct PixelPipeline {
    stages: Vec<Stage>,
    master: u8,
}

impl PixelPipeline {
    /// Build from a node's output configs; `configs[i]` applies to output `i`
    /// (i.e. `Node::outputs` order, where `configs[i].index == i + 1`).
    pub fn new(configs: &[OutputConfig]) -> Self {
        PixelPipeline {
            stages: configs.iter().map(|c| Stage::new(c, 100)).collect(),
            master: 100,
        }
    }

    /// Number of outputs configured.
    pub fn output_count(&self) -> usize {
        self.stages.len()
    }

    /// Global (player/show) brightness in percent, applied on top of each output's own.
    pub fn master_brightness(&self) -> u8 {
        self.master
    }

    /// Set the global brightness (0..=100 %; larger values are clamped).
    pub fn set_master_brightness(&mut self, percent: u8) {
        let percent = percent.min(100);
        if percent != self.master {
            self.master = percent;
            for s in &mut self.stages {
                s.rebuild(percent);
            }
        }
    }

    /// Replace the config of output `index` (0-based), growing the pipeline if needed.
    pub fn update_output(&mut self, index: usize, config: &OutputConfig) {
        let stage = Stage::new(config, self.master);
        if index < self.stages.len() {
            self.stages[index] = stage;
        } else {
            let filler = Stage::new(&OutputConfig::default(), self.master);
            self.stages.resize(index, filler);
            self.stages.push(stage);
        }
    }

    /// Process one output: `rgb` (show order) → `out` (wire order).
    ///
    /// `out` is resized to `rgb.len()`. Outputs beyond the configured ones use
    /// defaults (RGB, 100 %, gamma 1.0). A disabled output produces black of
    /// the same length, so its string goes dark instead of freezing.
    pub fn apply(&self, index: usize, rgb: &[u8], out: &mut Vec<u8>) {
        out.clear();
        out.resize(rgb.len(), 0);
        let Some(stage) = self.stages.get(index) else {
            let lut = build_lut(1.0, 100, self.master);
            for (o, &i) in out.iter_mut().zip(rgb) {
                *o = lut[usize::from(i)];
            }
            return;
        };
        if !stage.enabled {
            return;
        }
        let [a, b, c] = stage.order;
        let lut = &stage.lut;
        let whole = rgb.len() / 3 * 3;
        for (dst, src) in out[..whole]
            .chunks_exact_mut(3)
            .zip(rgb[..whole].chunks_exact(3))
        {
            dst[0] = lut[usize::from(src[a])];
            dst[1] = lut[usize::from(src[b])];
            dst[2] = lut[usize::from(src[c])];
        }
        // A trailing partial LED keeps its bytes in place (brightness/gamma only).
        for (dst, &src) in out[whole..].iter_mut().zip(&rgb[whole..]) {
            *dst = lut[usize::from(src)];
        }
    }

    /// Process a whole frame into `out` (reusing its allocations).
    pub fn process(&self, input: &OutputFrameRef<'_>, out: &mut OutputFrame) {
        out.outputs.resize_with(input.len(), Vec::new);
        for (i, (rgb, dst)) in input.iter().zip(out.outputs.iter_mut()).enumerate() {
            self.apply(i, rgb, dst);
        }
    }
}

/// Reorder a single RGB triple into wire order (helper for UIs and tests).
pub fn to_wire_order(order: ColorOrder, rgb: [u8; 3]) -> [u8; 3] {
    let [a, b, c] = order.source_indices();
    [rgb[a], rgb[b], rgb[c]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(order: ColorOrder, brightness: u8, gamma: f32, enabled: bool) -> OutputConfig {
        OutputConfig {
            color_order: order,
            brightness,
            gamma,
            enabled,
            ..OutputConfig::default()
        }
    }

    #[test]
    fn colour_order_applied() {
        let p = PixelPipeline::new(&[cfg(ColorOrder::GRB, 100, 1.0, true)]);
        let mut out = Vec::new();
        p.apply(0, &[10, 20, 30, 1, 2, 3], &mut out);
        assert_eq!(out, vec![20, 10, 30, 2, 1, 3]);
        assert_eq!(to_wire_order(ColorOrder::BGR, [1, 2, 3]), [3, 2, 1]);
    }

    #[test]
    fn brightness_and_master() {
        let mut p = PixelPipeline::new(&[cfg(ColorOrder::RGB, 50, 1.0, true)]);
        let mut out = Vec::new();
        p.apply(0, &[255, 100, 0], &mut out);
        assert_eq!(out, vec![128, 50, 0]);
        p.set_master_brightness(50);
        p.apply(0, &[255, 100, 0], &mut out);
        assert_eq!(out, vec![64, 25, 0]);
        p.set_master_brightness(250);
        assert_eq!(p.master_brightness(), 100);
    }

    #[test]
    fn gamma_curve() {
        let p = PixelPipeline::new(&[cfg(ColorOrder::RGB, 100, 2.2, true)]);
        let mut out = Vec::new();
        p.apply(0, &[0, 128, 255], &mut out);
        assert_eq!(out[0], 0);
        assert_eq!(out[2], 255);
        assert!(out[1] > 50 && out[1] < 60, "{}", out[1]);
        // Nonsense gamma never panics.
        let p = PixelPipeline::new(&[cfg(ColorOrder::RGB, 200, f32::NAN, true)]);
        p.apply(0, &[7, 8, 9], &mut out);
        assert_eq!(out, vec![7, 8, 9]);
    }

    #[test]
    fn disabled_output_is_black_same_length() {
        let p = PixelPipeline::new(&[cfg(ColorOrder::RGB, 100, 1.0, false)]);
        let mut out = vec![1, 2, 3];
        p.apply(0, &[9; 7], &mut out);
        assert_eq!(out, vec![0; 7]);
    }

    #[test]
    fn frame_and_unconfigured_outputs() {
        let mut p = PixelPipeline::new(&[cfg(ColorOrder::BRG, 100, 1.0, true)]);
        let a = [1u8, 2, 3, 4];
        let b = [5u8, 6, 7];
        let input = OutputFrameRef::new(vec![&a, &b]);
        let mut out = OutputFrame::default();
        p.process(&input, &mut out);
        assert_eq!(out.outputs[0], vec![3, 1, 2, 4]);
        assert_eq!(out.outputs[1], vec![5, 6, 7]);
        p.update_output(3, &cfg(ColorOrder::GRB, 100, 1.0, true));
        assert_eq!(p.output_count(), 4);
        let mut o = Vec::new();
        p.apply(3, &[1, 2, 3], &mut o);
        assert_eq!(o, vec![2, 1, 3]);
    }
}
