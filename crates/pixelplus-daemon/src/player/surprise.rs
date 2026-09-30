//! Surprises (F20, ARCHITECTURE §12.16): a short sequence or look drawn on
//! some props **on top of** whatever plays (the song goes on), with 150 ms
//! fades in and out.
//!
//! The leader starts one on a trigger ([`crate::services::triggers`]) and,
//! while it runs, carries a [`SurpriseAnchor`] in its sync packets; followers
//! render the same layer from their own sequence slices or the look preset.
//! Timing: `SurpriseAnchor.startPos` is the surprise's start on the leader's
//! clock **relative to the packet's timeline anchor `atMs`** (≤ 0 while it
//! runs), so a follower places it on its own clock exactly like the timeline
//! (`start = anchor.atMs(local) + startPos`) and both render the frame for
//! the same moment.

use super::clock::frame_for_slot;
use super::compose::{find_effect, EffectLayer, PropSlot, Sink};
use super::reader::{FrameLayout, FrameReader, SeqMeta};
use super::types::SurpriseAnchor;
use pixelplus_core::mapping::{read_prop_channels, OutputFrame, PropMap};
use pixelplus_core::model::{EffectPreset, Prop, Show, Target};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Fade in and out (ms).
pub const FADE_MS: f64 = 150.0;
/// Default length of a look surprise.
pub const DEFAULT_EFFECT_MS: u64 = 5_000;
/// Shortest / longest surprise.
pub const MIN_MS: u64 = 500;
pub const MAX_MS: u64 = 120_000;

/// What to start (built by the triggers service from a `surprise` action).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SurpriseRequest {
    /// Trigger id (or "test").
    pub id: String,
    /// "sequence" | "effect"
    pub kind: String,
    pub r#ref: String,
    /// Prop ids to draw on (empty = every prop).
    #[serde(default)]
    pub targets: Vec<String>,
    /// `None`: the sequence's length / [`DEFAULT_EFFECT_MS`].
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

/// What the engine answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SurpriseStarted {
    pub name: String,
    pub duration_ms: u64,
    pub props: usize,
    /// A surprise that was still running was replaced.
    pub replaced: bool,
}

/// Why a surprise can't start now (pure: tested with fakes).
pub fn refusal(
    follower: bool,
    blackout: bool,
    test_running: bool,
    calibrating: bool,
) -> Option<&'static str> {
    if follower {
        Some("This controller follows its show leader; surprises start on the leader.")
    } else if blackout {
        Some("The lights are off (blackout).")
    } else if calibrating {
        Some("Sound calibration is running.")
    } else if test_running {
        Some("A test pattern is running.")
    } else {
        None
    }
}

/// Fade level of a surprise `t_ms` into it (0 outside, ramps of [`FADE_MS`]).
pub fn fade_level(t_ms: f64, duration_ms: u64) -> f32 {
    let d = duration_ms as f64;
    if !(0.0..d).contains(&t_ms) {
        return 0.0;
    }
    let fade = FADE_MS.min(d / 2.0).max(1.0);
    (t_ms / fade).min((d - t_ms) / fade).min(1.0) as f32
}

enum Content {
    Effect(Box<EffectLayer>),
    Sequence {
        reader: FrameReader,
        meta: Option<SeqMeta>,
        buf: Vec<u8>,
        have: bool,
        /// Followers: the slice frame unpacked into the node's outputs.
        tmp: Option<OutputFrame>,
    },
}

/// A running surprise on this node.
pub struct SurpriseLayer {
    /// What followers get (its `start_pos` is filled in per status).
    pub anchor: SurpriseAnchor,
    pub name: String,
    /// Start on this node's engine clock (ms).
    pub start_ms: f64,
    content: Content,
    props: Vec<Prop>,
    slots: Vec<PropSlot>,
    px: Vec<u8>,
    base: Vec<u8>,
}

impl SurpriseLayer {
    /// Build the layer for `anchor` from this node's show (`start_ms` on the
    /// local engine clock). Errors are for people.
    pub fn new(
        show: &Show,
        data_dir: &Path,
        anchor: SurpriseAnchor,
        start_ms: f64,
    ) -> Result<Self, String> {
        let props: Vec<Prop> = if anchor.targets.is_empty() {
            show.props.clone()
        } else {
            show.props
                .iter()
                .filter(|p| anchor.targets.contains(&p.id))
                .cloned()
                .collect()
        };
        let (content, name) = match anchor.kind.as_str() {
            "sequence" => {
                let seq = show
                    .sequence(&anchor.r#ref)
                    .ok_or("that sequence is not on this controller")?;
                let path = data_dir.join(&seq.file);
                if !path.exists() {
                    return Err(format!("the file of “{}” is missing", seq.name));
                }
                (
                    Content::Sequence {
                        reader: FrameReader::open(path, 0),
                        meta: None,
                        buf: vec![],
                        have: false,
                        tmp: None,
                    },
                    seq.name.clone(),
                )
            }
            _ => {
                let mut preset: EffectPreset =
                    find_effect(show, &anchor.r#ref).ok_or("that look no longer exists")?;
                // Same world bounds on every node (followers' presets are stamped
                // by the manifest; the leader stamps here with the whole show).
                if !preset
                    .params
                    .contains_key(pixelplus_core::effects::WORLD_PARAM)
                {
                    pixelplus_core::effects::stamp_world_bounds(&mut preset, &show.props);
                }
                let target = Target {
                    prop_ids: props.iter().map(|p| p.id.clone()).collect(),
                    ..Default::default()
                };
                let name = preset.name.clone();
                let layer = if target.prop_ids.is_empty() {
                    // Nothing of it on this node: an empty layer.
                    EffectLayer::on_target(
                        show,
                        &preset,
                        &Target {
                            prop_ids: vec!["\u{0}none".into()],
                            ..Default::default()
                        },
                    )
                } else {
                    EffectLayer::on_target(show, &preset, &target)
                };
                (Content::Effect(Box::new(layer)), name)
            }
        };
        Ok(SurpriseLayer {
            slots: props.iter().map(PropSlot::of).collect(),
            props,
            anchor,
            name,
            start_ms,
            content,
            px: vec![],
            base: vec![],
        })
    }

    /// Time into the surprise at engine time `t`.
    pub fn elapsed(&self, t_ms: f64) -> f64 {
        t_ms - self.start_ms
    }

    pub fn done(&self, t_ms: f64) -> bool {
        self.elapsed(t_ms) >= self.anchor.duration_ms as f64
    }

    /// The sequence's length once known (to trim the surprise to it).
    pub fn sequence_ms(&mut self) -> Option<u64> {
        if let Content::Sequence { reader, meta, .. } = &mut self.content {
            if meta.is_none() {
                *meta = reader.meta();
            }
            return meta.as_ref().map(|m| m.duration_ms());
        }
        None
    }

    /// Read the sequence frame for surprise time `t` (followers: unpacked
    /// into the node's outputs); false when there is nothing (yet).
    fn prepare(&mut self, t: f64, slot_ms: f64, follower: bool) -> bool {
        match &mut self.content {
            Content::Effect(_) => true,
            Content::Sequence {
                reader,
                meta,
                buf,
                have,
                tmp,
            } => {
                if meta.is_none() {
                    *meta = reader.meta();
                }
                let Some(m) = meta.as_ref() else {
                    return false;
                };
                if buf.len() != m.frame_len {
                    *buf = vec![0; m.frame_len];
                }
                let idx = frame_for_slot(t.max(0.0), slot_ms, m.frame_ms as f64)
                    .min(m.frame_count.saturating_sub(1));
                if reader.get(idx, buf) {
                    *have = true;
                }
                if !*have {
                    return false;
                }
                if let (FrameLayout::Outputs(ppo), true) = (&m.layout, follower) {
                    let t = tmp.get_or_insert_with(|| OutputFrame::new(ppo));
                    let mut off = 0usize;
                    for (i, &px) in ppo.iter().enumerate() {
                        let len = px as usize * 3;
                        let src = buf.get(off..off + len).unwrap_or(&[]);
                        let dst = t.output_mut(i);
                        let n = dst.len().min(src.len());
                        dst[..n].copy_from_slice(&src[..n]);
                        off += len;
                    }
                }
                true
            }
        }
    }

    /// Leader: draw over channel space (`light_ms` = when this frame lights up).
    pub fn render_chan(&mut self, light_ms: f64, slot_ms: f64, chan: &mut [u8]) {
        let t = self.elapsed(light_ms);
        let a = fade_level(t, self.anchor.duration_ms);
        if a <= 0.0 {
            return;
        }
        match &mut self.content {
            Content::Effect(layer) => {
                // Render into a scratch copy of channel space, then blend per prop.
                let mut scratch = chan.to_vec();
                layer.render(t.max(0.0) as u64, &mut Sink::Chan(&mut scratch));
                for (prop, slot) in self.props.iter().zip(&self.slots) {
                    self.px.resize(slot.len, 0);
                    self.base.resize(slot.len, 0);
                    read_prop_channels(prop, &scratch, &mut self.px);
                    read_prop_channels(prop, chan, &mut self.base);
                    blend_into(&mut self.base, &self.px, a);
                    Sink::Chan(chan).put(slot, &self.base);
                }
            }
            Content::Sequence { .. } => {
                if !self.prepare(t, slot_ms, false) {
                    return;
                }
                let Content::Sequence { buf, meta, .. } = &self.content else {
                    return;
                };
                if meta
                    .as_ref()
                    .is_some_and(|m| m.layout != FrameLayout::Channels)
                {
                    return;
                }
                for (prop, slot) in self.props.iter().zip(&self.slots) {
                    self.px.resize(slot.len, 0);
                    self.base.resize(slot.len, 0);
                    read_prop_channels(prop, buf, &mut self.px);
                    read_prop_channels(prop, chan, &mut self.base);
                    blend_into(&mut self.base, &self.px, a);
                    Sink::Chan(chan).put(slot, &self.base);
                }
            }
        }
    }

    /// Follower: draw over the node's output frame.
    pub fn render_frame(
        &mut self,
        light_ms: f64,
        slot_ms: f64,
        frame: &mut OutputFrame,
        map: &PropMap,
    ) {
        let t = self.elapsed(light_ms);
        let a = fade_level(t, self.anchor.duration_ms);
        if a <= 0.0 {
            return;
        }
        match &mut self.content {
            Content::Effect(layer) => {
                let mut scratch = frame.clone();
                layer.render(t.max(0.0) as u64, &mut Sink::Frame(&mut scratch, map));
                for slot in &self.slots {
                    self.px.resize(slot.len, 0);
                    self.base.resize(slot.len, 0);
                    map.read_prop(&slot.id, &scratch, &mut self.px);
                    map.read_prop(&slot.id, frame, &mut self.base);
                    blend_into(&mut self.base, &self.px, a);
                    map.apply_overlay(&slot.id, &self.base, frame);
                }
            }
            Content::Sequence { .. } => {
                if !self.prepare(t, slot_ms, true) {
                    return;
                }
                let Content::Sequence { tmp, buf, meta, .. } = &self.content else {
                    return;
                };
                let channels = meta
                    .as_ref()
                    .is_some_and(|m| m.layout == FrameLayout::Channels);
                for (prop, slot) in self.props.iter().zip(&self.slots) {
                    self.px.resize(slot.len, 0);
                    self.base.resize(slot.len, 0);
                    if channels {
                        read_prop_channels(prop, buf, &mut self.px);
                    } else if let Some(tf) = tmp {
                        map.read_prop(&slot.id, tf, &mut self.px);
                    } else {
                        continue;
                    }
                    map.read_prop(&slot.id, frame, &mut self.base);
                    blend_into(&mut self.base, &self.px, a);
                    map.apply_overlay(&slot.id, &self.base, frame);
                }
            }
        }
    }
}

/// `base = base·(1−a) + layer·a`.
fn blend_into(base: &mut [u8], layer: &[u8], a: f32) {
    let t = (a.clamp(0.0, 1.0) * 256.0) as u32;
    let s = 256 - t;
    for (b, &l) in base.iter_mut().zip(layer) {
        *b = ((*b as u32 * s + l as u32 * t) >> 8) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fades() {
        assert_eq!(fade_level(-1.0, 5000), 0.0);
        assert_eq!(fade_level(0.0, 5000), 0.0);
        assert!((fade_level(75.0, 5000) - 0.5).abs() < 1e-6);
        assert_eq!(fade_level(2500.0, 5000), 1.0);
        assert!((fade_level(4925.0, 5000) - 0.5).abs() < 1e-6);
        assert_eq!(fade_level(5000.0, 5000), 0.0);
        // Very short: the fades meet in the middle.
        assert!((fade_level(100.0, 200) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn refusals_in_priority_order() {
        assert!(refusal(true, true, true, true).unwrap().contains("leader"));
        assert!(refusal(false, true, true, true)
            .unwrap()
            .contains("blackout"));
        assert!(refusal(false, false, true, true)
            .unwrap()
            .contains("calibration"));
        assert!(refusal(false, false, true, false).unwrap().contains("test"));
        assert_eq!(refusal(false, false, false, false), None);
    }

    #[test]
    fn blending() {
        let mut b = [200u8, 0, 100];
        blend_into(&mut b, &[0, 200, 100], 0.5);
        assert_eq!(b, [100, 100, 100]);
        let mut b = [1u8, 2, 3];
        blend_into(&mut b, &[9, 9, 9], 1.0);
        assert_eq!(b, [9, 9, 9]);
    }
}
