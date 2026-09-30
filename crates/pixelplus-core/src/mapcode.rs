//! Camera mapping codes (F6/F7, owned by WS4): the deterministic light
//! pattern a phone decodes to find which output/pixel is where.
//!
//! WS0 created this file with the plan types only (they are part of the
//! `TestRequest` contract, `mode: "mapCode"`). The codebook, Gray coding and
//! the frame function `fn level_at(plan, target, pixel, pos_ms)` belong here.

use serde::{Deserialize, Serialize};

/// What the leader shows for one mapping run. Every node renders its own
/// outputs from this plan and the timeline position, so all nodes agree bit
/// for bit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MapPlan {
    pub seed: u32,
    /// Duration of one code bit (ms).
    #[serde(default = "default_bit_ms")]
    pub bit_ms: u32,
    /// "On" level, 0..255 (≤ 50 % enforced by the leader).
    #[serde(default = "default_level")]
    pub level: u8,
    #[serde(default = "default_passes")]
    pub passes: u8,
    /// bit 0 = phase A (identify targets), bit 1 = phase B (pixel positions).
    #[serde(default = "default_phases")]
    pub phases: u8,
    /// Ordered targets; the index is the code index.
    #[serde(default)]
    pub targets: Vec<MapTarget>,
    /// ceil(log2(max pixels)).
    #[serde(default)]
    pub pixel_bits: u8,
    /// Timeline-relative start (ms).
    #[serde(default)]
    pub start_pos_ms: u64,
}

/// One node output in a mapping run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct MapTarget {
    pub node_id: String,
    /// 1-based.
    pub output: u32,
    /// Pixels to light (may exceed the configured count when probing, F7).
    #[serde(default)]
    pub max_pixels: u32,
}

pub const PHASE_A: u8 = 1;
pub const PHASE_B: u8 = 2;

fn default_bit_ms() -> u32 {
    200
}
fn default_level() -> u8 {
    77
}
fn default_passes() -> u8 {
    3
}
fn default_phases() -> u8 {
    PHASE_A | PHASE_B
}

impl Default for MapPlan {
    fn default() -> Self {
        MapPlan {
            seed: 0,
            bit_ms: default_bit_ms(),
            level: default_level(),
            passes: default_passes(),
            phases: default_phases(),
            targets: vec![],
            pixel_bits: 0,
            start_pos_ms: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_defaults_from_minimal_json() {
        let p: MapPlan = serde_json::from_str(r#"{"seed":7}"#).unwrap();
        assert_eq!(p.bit_ms, 200);
        assert_eq!(p.level, 77);
        assert_eq!(p.phases, PHASE_A | PHASE_B);
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["bitMs"], 200);
        assert_eq!(v["startPosMs"], 0);
    }
}
