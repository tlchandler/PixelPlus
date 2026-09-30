//! Frame composition helpers: where prop pixels go (channel space on the
//! leader, the node's output frame on followers), effect looks, test
//! patterns and the live-preview frame (ARCHITECTURE §8.1).

use super::types::TestRequest;
use pixelplus_core::effects::{builtin_presets, EffectRenderer, TestPattern};
use pixelplus_core::mapping::{read_prop_channels, write_channel_runs, OutputFrame, PropMap};
use pixelplus_core::model::{ChannelRun, EffectPreset, Prop, Show, Target};
use std::ops::Range;

/// One prop as a composition target.
#[derive(Debug, Clone)]
pub struct PropSlot {
    pub id: String,
    /// Where the prop's pixels live in channel space (`Prop::channel_ranges`).
    pub runs: Vec<ChannelRun>,
    /// `pixelCount × 3`.
    pub len: usize,
}

impl PropSlot {
    pub fn of(prop: &Prop) -> Self {
        PropSlot {
            id: prop.id.clone(),
            runs: prop.channel_ranges().collect(),
            len: prop.pixel_count as usize * 3,
        }
    }
}

/// Where layers write prop pixels.
pub enum Sink<'a> {
    /// Leader: absolute channel space (routed by the NodeMap afterwards; the
    /// preview reads every prop from here).
    Chan(&'a mut [u8]),
    /// Follower: straight into the node's output frame.
    Frame(&'a mut OutputFrame, &'a PropMap),
}

impl Sink<'_> {
    pub fn put(&mut self, slot: &PropSlot, px: &[u8]) {
        match self {
            Sink::Chan(chan) => {
                write_channel_runs(
                    slot.runs.iter().copied(),
                    &px[..slot.len.min(px.len())],
                    chan,
                );
            }
            Sink::Frame(frame, map) => {
                map.apply_overlay(&slot.id, &px[..slot.len.min(px.len())], frame);
            }
        }
    }
}

/// Look up a preset: the show's effects, then the built-in looks.
pub fn find_effect(show: &Show, id: &str) -> Option<EffectPreset> {
    show.effect(id)
        .cloned()
        .or_else(|| builtin_presets().into_iter().find(|e| e.id == id))
}

/// Props a target covers; an empty target means every prop.
pub fn resolve_target<'a>(show: &'a Show, target: &Target) -> Vec<&'a Prop> {
    if !target.all && target.prop_ids.is_empty() && target.group_ids.is_empty() {
        return show.props.iter().collect();
    }
    target.resolve(show)
}

/// An effect rendered onto a set of props.
pub struct EffectLayer {
    pub preset: EffectPreset,
    renderer: EffectRenderer,
    slots: Vec<(PropSlot, Range<usize>)>,
    scratch: Vec<u8>,
}

impl EffectLayer {
    /// Render `preset` on its own target (empty target = all props).
    pub fn new(show: &Show, preset: &EffectPreset) -> Self {
        Self::on_target(show, preset, &preset.target)
    }

    pub fn on_target(show: &Show, preset: &EffectPreset, target: &Target) -> Self {
        let props = resolve_target(show, target);
        let renderer = EffectRenderer::new(preset, &props);
        let slots = props
            .iter()
            .map(|p| PropSlot::of(p))
            .zip(renderer.prop_ranges().map(|(_, r)| r))
            .collect();
        EffectLayer {
            preset: preset.clone(),
            scratch: vec![0; renderer.frame_len()],
            renderer,
            slots,
        }
    }

    pub fn render(&mut self, t_ms: u64, sink: &mut Sink<'_>) {
        self.renderer.render(t_ms, &mut self.scratch);
        for (slot, r) in &self.slots {
            if let Some(px) = self.scratch.get(r.clone()) {
                sink.put(slot, px);
            }
        }
    }
}

/// A running test.
pub struct TestLayer {
    pub req: TestRequest,
    kind: TestKind,
    slots: Vec<PropSlot>,
    /// Raw output test on this node: `Some(None)` = every output (identify),
    /// `Some(Some(i))` = 0-based output `i`.
    pub raw_output: Option<Option<usize>>,
    /// The test targets another node only (nothing to show here).
    pub remote_only: bool,
    pub started_ms: f64,
    scratch: Vec<u8>,
}

enum TestKind {
    Pattern(TestPattern),
    Effect(Box<EffectLayer>),
}

impl TestLayer {
    /// Build from a request. `node_id` is this node's id.
    pub fn new(show: &Show, node_id: &str, req: &TestRequest, now_ms: f64) -> Result<Self, String> {
        let mut raw_output = None;
        let mut remote_only = false;
        if let Some(n) = &req.target.node_id {
            if n == node_id {
                raw_output = Some(req.target.output.map(|o| o.max(1) as usize - 1));
            } else {
                remote_only = true;
            }
        }
        let kind = if req.mode == "effect" {
            let preset = req.effect.clone().ok_or("an effect test needs an effect")?;
            let target = if is_empty(&req.target.props) {
                preset.target.clone()
            } else {
                req.target.props.clone()
            };
            TestKind::Effect(Box::new(EffectLayer::on_target(show, &preset, &target)))
        } else {
            TestKind::Pattern(test_pattern(req)?)
        };
        let slots = if raw_output.is_some() || remote_only {
            vec![]
        } else {
            resolve_target(show, &req.target.props)
                .into_iter()
                .map(PropSlot::of)
                .collect()
        };
        Ok(TestLayer {
            req: req.clone(),
            kind,
            slots,
            raw_output,
            remote_only,
            started_ms: now_ms,
            scratch: Vec::new(),
        })
    }

    /// Paint the test onto its props.
    pub fn render_props(&mut self, now_ms: f64, sink: &mut Sink<'_>) {
        let t = (now_ms - self.started_ms).max(0.0) as u64;
        match &mut self.kind {
            TestKind::Effect(layer) => {
                if self.raw_output.is_none() && !self.remote_only {
                    layer.render(t, sink);
                }
            }
            TestKind::Pattern(p) => {
                for slot in &self.slots {
                    self.scratch.resize(slot.len, 0);
                    p.render(t, &mut self.scratch);
                    sink.put(slot, &self.scratch);
                }
            }
        }
    }

    /// Paint a raw output test onto the node's output frame.
    pub fn render_raw(&mut self, now_ms: f64, frame: &mut OutputFrame) {
        let Some(which) = self.raw_output else { return };
        let t = (now_ms - self.started_ms).max(0.0) as u64;
        let outputs = match which {
            Some(o) => o..o + 1,
            None => 0..frame.output_count(),
        };
        for o in outputs {
            let out = frame.output_mut(o);
            match &self.kind {
                TestKind::Pattern(p) => p.render(t, out),
                // Effects need prop geometry; show a solid colour instead.
                TestKind::Effect(_) => TestPattern::Solid { color: None }.render(t, out),
            }
        }
    }

    /// A live look (effect-mode test on props), reported as state "effect".
    pub fn is_look(&self) -> bool {
        matches!(self.kind, TestKind::Effect(_)) && self.raw_output.is_none() && !self.remote_only
    }

    /// The look's preset with the test's target (for followers).
    pub fn look_preset(&self) -> Option<EffectPreset> {
        let mut p = self.req.effect.clone()?;
        if !is_empty(&self.req.target.props) {
            p.target = self.req.target.props.clone();
        }
        Some(p)
    }
}

fn is_empty(t: &Target) -> bool {
    !t.all && t.prop_ids.is_empty() && t.group_ids.is_empty()
}

/// Build a core test pattern from the API request.
pub fn test_pattern(req: &TestRequest) -> Result<TestPattern, String> {
    let mut v = serde_json::json!({ "mode": req.mode });
    if let Some(c) = &req.color {
        if !c.is_empty() {
            v["color"] = serde_json::Value::String(c.clone());
        }
    }
    if let Some(s) = req.speed {
        v["speed"] = serde_json::json!(s);
    }
    serde_json::from_value(v).map_err(|e| format!("unknown test mode '{}' ({e})", req.mode))
}

/// Build a preview frame: `0x50 | u32 LE frameNo | per prop (show order)
/// pixelCount×3 RGB`, scaled by `level` (0..=1).
pub fn preview_frame(
    show: &Show,
    frame_no: u32,
    level: f32,
    chan: Option<&[u8]>,
    local: Option<(&OutputFrame, &PropMap)>,
) -> Vec<u8> {
    let total: usize = show.props.iter().map(|p| p.pixel_count as usize * 3).sum();
    let mut out = Vec::with_capacity(5 + total);
    out.push(0x50);
    out.extend_from_slice(&frame_no.to_le_bytes());
    for p in &show.props {
        let n = p.pixel_count as usize * 3;
        let start = out.len();
        if let Some(chan) = chan {
            out.resize(start + n, 0);
            read_prop_channels(p, chan, &mut out[start..start + n]);
        } else {
            out.resize(start + n, 0);
            if let Some((frame, map)) = local {
                map.read_prop(&p.id, frame, &mut out[start..start + n]);
            }
        }
    }
    super::clock::scale(&mut out[5..], level);
    out
}

#[cfg(test)]
mod tests {
    use super::super::types::TestTarget;
    use super::*;
    use pixelplus_core::model::{BoardKind, EffectKind, Node, NodeRole, PropKind, PropSegment};

    pub(crate) fn prop(id: &str, n: u32, chan: u32, out: u32, start: u32) -> Prop {
        Prop {
            suspect_pixels: Default::default(),
            id: id.into(),
            name: id.into(),
            kind: PropKind::Line,
            pixel_count: n,
            xlights_model: None,
            channel_start: chan,
            channels_per_pixel: 3,
            channel_runs: None,
            segments: vec![PropSegment {
                node_id: "n1".into(),
                output: out,
                start_pixel: start,
                pixel_count: n,
                prop_offset: 0,
                reverse: false,
                null_pixels: 0,
            }],
            group_ids: vec![],
            layout: None,
            matrix: None,
            color: None,
            max_milliamps_per_pixel: None,
            notes: None,
        }
    }

    fn show() -> Show {
        let mut s = Show::default();
        s.nodes.push(Node {
            hardware_history: Default::default(),
            serial: Default::default(),
            id: "n1".into(),
            name: "n1".into(),
            hostname: "n1".into(),
            role: NodeRole::Leader,
            board: BoardKind::Difftx,
            board_rev: None,
            pi_model: None,
            outputs: BoardKind::Difftx.default_outputs(),
            adopted: true,
            last_seen: None,
            notes: None,
        });
        s.props = vec![prop("a", 2, 0, 1, 0), prop("b", 3, 6, 2, 0)];
        s
    }

    #[test]
    fn sinks_write_channel_space_and_frames() {
        let s = show();
        let mut chan = vec![0u8; 15];
        let slot = PropSlot::of(&s.props[1]);
        Sink::Chan(&mut chan).put(&slot, &[7; 9]);
        assert_eq!(&chan[6..15], &[7; 9]);
        assert_eq!(&chan[0..6], &[0; 6]);
        // Short channel buffer: clipped, never panics.
        let mut short = vec![0u8; 8];
        Sink::Chan(&mut short).put(&slot, &[7; 9]);
        assert_eq!(short, [0, 0, 0, 0, 0, 0, 7, 7]);
        let map = PropMap::build(&s, "n1").unwrap();
        let mut frame = OutputFrame::new(map.pixels_per_output());
        Sink::Frame(&mut frame, &map).put(&slot, &[9; 9]);
        assert_eq!(frame.output(1), &[9; 9]);

        // xLights individual start channels: pixel 0 at byte 12, pixels 1-2 at byte 3;
        // the preview reads them back in prop order.
        let mut s = s;
        s.props[1].channel_runs = Some(vec![
            ChannelRun {
                prop_offset: 0,
                channel_start: 12,
                pixel_count: 1,
            },
            ChannelRun {
                prop_offset: 1,
                channel_start: 3,
                pixel_count: 2,
            },
        ]);
        let slot = PropSlot::of(&s.props[1]);
        let mut chan = vec![0u8; 15];
        Sink::Chan(&mut chan).put(&slot, &[1, 1, 1, 2, 2, 2, 3, 3, 3]);
        assert_eq!(chan, [0, 0, 0, 2, 2, 2, 3, 3, 3, 0, 0, 0, 1, 1, 1]);
        let pv = preview_frame(&s, 1, 1.0, Some(&chan), None);
        assert_eq!(&pv[5 + 6..], &[1, 1, 1, 2, 2, 2, 3, 3, 3]);
    }

    #[test]
    fn test_layers() {
        let s = show();
        let req = TestRequest {
            map_run_id: Default::default(),
            cal: Default::default(),
            identify: Default::default(),
            map: Default::default(),
            mode: "solid".into(),
            color: Some("#ff0000".into()),
            speed: None,
            target: TestTarget {
                props: Target {
                    prop_ids: vec!["b".into()],
                    ..Default::default()
                },
                ..Default::default()
            },
            effect: None,
        };
        let mut t = TestLayer::new(&s, "n1", &req, 0.0).unwrap();
        let mut chan = vec![0u8; 15];
        t.render_props(10.0, &mut Sink::Chan(&mut chan));
        assert_eq!(&chan[6..9], &[255, 0, 0]);
        assert_eq!(&chan[0..3], &[0, 0, 0]);
        // Raw output test on this node.
        let mut raw = req.clone();
        raw.target = TestTarget {
            node_id: Some("n1".into()),
            output: Some(2),
            ..Default::default()
        };
        let mut t = TestLayer::new(&s, "n1", &raw, 0.0).unwrap();
        let mut frame = OutputFrame::new(&[2, 3]);
        t.render_raw(0.0, &mut frame);
        assert_eq!(frame.output(1), &[255, 0, 0, 255, 0, 0, 255, 0, 0]);
        assert_eq!(frame.output(0), &[0; 6]);
        // Identify: every output of this node.
        let mut all = raw.clone();
        all.target.output = None;
        let mut t = TestLayer::new(&s, "n1", &all, 0.0).unwrap();
        let mut frame = OutputFrame::new(&[2, 3]);
        t.render_raw(0.0, &mut frame);
        assert!(frame.as_bytes().chunks(3).all(|p| p == [255, 0, 0]));
        // Other node: nothing here.
        raw.target.node_id = Some("n2".into());
        assert!(TestLayer::new(&s, "n1", &raw, 0.0).unwrap().remote_only);
        // Every mode parses.
        for mode in ["solid", "chase", "rgbCycle", "countPixels", "walk"] {
            let r = TestRequest {
                mode: mode.into(),
                ..req.clone()
            };
            assert!(test_pattern(&r).is_ok(), "{mode}");
        }
        assert!(test_pattern(&TestRequest {
            mode: "bogus".into(),
            ..req.clone()
        })
        .is_err());
        // Effect mode.
        let preset = EffectPreset {
            id: "e".into(),
            name: "E".into(),
            effect: EffectKind::Solid,
            params: [("color".to_string(), serde_json::json!("#00ff00"))]
                .into_iter()
                .collect(),
            target: Target::default(),
        };
        let r = TestRequest {
            mode: "effect".into(),
            effect: Some(preset),
            ..req.clone()
        };
        let mut t = TestLayer::new(&s, "n1", &r, 0.0).unwrap();
        let mut chan = vec![0u8; 15];
        t.render_props(0.0, &mut Sink::Chan(&mut chan));
        assert!(chan[7] > 0 && chan[0] == 0, "only prop b, green: {chan:?}");
    }

    #[test]
    fn preview_layout() {
        let s = show();
        let chan: Vec<u8> = (0..15).collect();
        let p = preview_frame(&s, 7, 1.0, Some(&chan), None);
        assert_eq!(p[0], 0x50);
        assert_eq!(u32::from_le_bytes(p[1..5].try_into().unwrap()), 7);
        assert_eq!(&p[5..], &chan[..]);
        // Channel space shorter than the props: zero padded.
        let p = preview_frame(&s, 1, 0.5, Some(&chan[..4]), None);
        assert_eq!(p.len(), 5 + 15);
        assert_eq!(&p[5..9], &[0, 0, 1, 1]);
        assert!(p[9..].iter().all(|&b| b == 0));
    }
}
