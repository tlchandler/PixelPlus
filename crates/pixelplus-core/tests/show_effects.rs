//! Effects on a realistic display: built-in presets, targets, catalogue and
//! a render-cost smoke test.

use pixelplus_core::effects::{
    builtin_presets, effect_catalog, param_schema, render_test_pattern, stamp_world_bounds,
    EffectRenderer, TestPattern, ALL_EFFECT_KINDS,
};
use pixelplus_core::model::{EffectPreset, MatrixInfo, Prop, PropKind, PropLayout, Show};
use std::time::Instant;

fn prop(id: &str, kind: PropKind, n: u32, x: f32, y: f32, w: f32, h: f32) -> Prop {
    Prop {
        id: id.into(),
        name: id.into(),
        kind,
        pixel_count: n,
        xlights_model: None,
        channel_start: 0,
        channels_per_pixel: 3,
        channel_runs: None,
        segments: vec![],
        group_ids: vec![],
        layout: Some(PropLayout {
            x,
            y,
            w,
            h,
            rotation: 0.0,
            points: None,
        }),
        matrix: None,
        color: None,
        max_milliamps_per_pixel: None,
        notes: None,
    }
}

fn display() -> Show {
    let mut matrix = prop(
        "matrix",
        PropKind::Matrix,
        32 * 16,
        400.0,
        100.0,
        320.0,
        160.0,
    );
    matrix.matrix = Some(MatrixInfo {
        width: 32,
        height: 16,
        pixel_map: (0..32 * 16).collect(),
    });
    Show {
        props: vec![
            prop("roof", PropKind::Line, 300, 0.0, 0.0, 1200.0, 10.0),
            prop("arch1", PropKind::Arch, 100, 50.0, 300.0, 200.0, 100.0),
            prop("arch2", PropKind::Arch, 100, 900.0, 300.0, 200.0, 100.0),
            prop("tree", PropKind::Tree, 1600, 500.0, 50.0, 200.0, 350.0),
            prop("cane", PropKind::Candycane, 50, 300.0, 330.0, 30.0, 70.0),
            prop("icicles", PropKind::Icicles, 400, 0.0, 10.0, 1200.0, 60.0),
            prop("star", PropKind::Star, 120, 560.0, 0.0, 80.0, 80.0),
            matrix,
        ],
        ..Show::default()
    }
}

#[test]
fn every_builtin_preset_renders_on_a_real_display() {
    let show = display();
    let total: usize = show.props.iter().map(|p| p.pixel_count as usize * 3).sum();
    for preset in builtin_presets() {
        let props = preset.target.resolve(&show);
        assert_eq!(props.len(), show.props.len(), "{}", preset.name);
        let r = EffectRenderer::new(&preset, &props);
        assert_eq!(r.frame_len(), total);
        let mut frame = vec![0u8; total];
        let mut lit = false;
        for t in (0..5_000).step_by(250) {
            r.render(t, &mut frame);
            lit |= frame.iter().any(|&b| b > 0);
        }
        assert!(lit, "{} stays dark", preset.name);
    }
}

#[test]
fn follower_slice_matches_leader_for_every_preset() {
    let show = display();
    let all: Vec<&Prop> = show.props.iter().collect();
    let tree_offset: usize = show.props[..3]
        .iter()
        .map(|p| p.pixel_count as usize * 3)
        .sum();
    let tree_len = show.props[3].pixel_count as usize * 3;
    for preset in builtin_presets() {
        let mut sent: EffectPreset = preset.clone();
        stamp_world_bounds(&mut sent, &show.props);
        // The preset travels as JSON in the sync packet.
        let received: EffectPreset =
            serde_json::from_str(&serde_json::to_string(&sent).unwrap()).unwrap();
        let leader = EffectRenderer::new(&sent, &all);
        let follower = EffectRenderer::new(&received, &[&show.props[3]]);
        let mut a = vec![0u8; leader.frame_len()];
        let mut b = vec![0u8; follower.frame_len()];
        for t in [0u64, 333, 60_000, 3_600_000] {
            leader.render(t, &mut a);
            follower.render(t, &mut b);
            assert_eq!(
                &a[tree_offset..tree_offset + tree_len],
                &b[..],
                "{}",
                preset.name
            );
        }
    }
}

#[test]
fn catalogue_matches_schema() {
    for info in effect_catalog() {
        assert_eq!(info.params, param_schema(info.kind));
        assert!(!info.label.is_empty() && !info.description.is_empty());
        assert!(info.params.iter().any(|p| p.key == "brightness"));
    }
    assert_eq!(effect_catalog().len(), ALL_EFFECT_KINDS.len());
}

#[test]
fn test_patterns_cover_a_whole_output() {
    let mut out = vec![0u8; 800 * 3];
    for p in [
        TestPattern::Solid { color: None },
        TestPattern::Chase { color: None },
        TestPattern::RgbCycle,
        TestPattern::CountPixels,
        TestPattern::Walk {
            color: None,
            speed: None,
        },
    ] {
        render_test_pattern(&p, 12_345, &mut out);
        assert!(out.iter().any(|&b| b > 0), "{p:?}");
    }
}

/// Coarse cost check: a 3,000-pixel display must render every effect well
/// within a 25 ms frame even in unoptimised test builds.
#[test]
fn render_cost_smoke_test() {
    let show = display();
    let props: Vec<&Prop> = show.props.iter().collect();
    for kind in ALL_EFFECT_KINDS {
        let preset = EffectPreset {
            id: "perf".into(),
            name: "perf".into(),
            effect: kind,
            params: Default::default(),
            target: Default::default(),
        };
        let r = EffectRenderer::new(&preset, &props);
        let mut frame = vec![0u8; r.frame_len()];
        let frames = 10u32;
        let start = Instant::now();
        for k in 0..frames {
            r.render(u64::from(k) * 25, &mut frame);
        }
        let per_frame = start.elapsed() / frames;
        assert!(
            per_frame.as_millis() < 250,
            "{kind:?} takes {per_frame:?} per frame"
        );
    }
}
