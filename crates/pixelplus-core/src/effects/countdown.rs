//! The show-start countdown (F4, ARCHITECTURE §12.4).
//!
//! A countdown playlist item renders as an [`EffectKind::Countdown`] preset
//! built by [`countdown_preset`]: big digits on a matrix prop (the `core::text`
//! bitmap fonts through the matrix pixel map) while every other prop fills up
//! like a progress bar, pulses at each second, or stays dark, and an optional
//! white flash in the last moments before zero. Everything is a pure function
//! of the item position, so followers render it identically from the preset
//! the leader sends (with `matrixPropId` already resolved).

use super::color::Rgb;
use crate::model::{
    CountdownFinale, CountdownOthers, EffectKind, EffectParams, EffectPreset, MatrixInfo, Prop,
    PropKind, Show, Target,
};
use crate::text::{best_scale, draw_text, text_width, Font, RgbGrid};
use serde_json::{json, Value};

/// Countdown length limits (ms).
pub const MIN_DURATION_MS: u64 = 1_000;
pub const MAX_DURATION_MS: u64 = 600_000;
/// Length of the white finale flash just before zero.
pub const FINALE_MS: f64 = 200.0;
/// Decay of the per-second pulse (and of the digits' accent).
pub const PULSE_DECAY_MS: f64 = 260.0;
/// Id of the preset built for a countdown item (`countdown:<itemId>`).
pub const PRESET_PREFIX: &str = "countdown:";

/// A countdown's parameters (the preset's `params`).
#[derive(Debug, Clone, PartialEq)]
pub struct CountdownSpec {
    pub duration_ms: u64,
    /// Template: `{s}` seconds left, `{mm}`/`{ss}` minutes and seconds.
    pub text: String,
    pub color: Rgb,
    pub others: CountdownOthers,
    pub finale: CountdownFinale,
    pub matrix_prop_id: Option<String>,
}

impl Default for CountdownSpec {
    fn default() -> Self {
        CountdownSpec {
            duration_ms: 10_000,
            text: "{s}".into(),
            color: Rgb::WHITE,
            others: CountdownOthers::Fill,
            finale: CountdownFinale::Flash,
            matrix_prop_id: None,
        }
    }
}

impl CountdownSpec {
    /// Read from preset params; anything missing or invalid falls back to the
    /// defaults, so a damaged preset still counts down.
    pub fn from_params(p: &EffectParams) -> Self {
        let d = CountdownSpec::default();
        let str_of = |k: &str| p.get(k).and_then(Value::as_str).map(str::to_string);
        CountdownSpec {
            duration_ms: p
                .get("durationMs")
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite() && *v > 0.0)
                .map_or(d.duration_ms, |v| v as u64)
                .clamp(MIN_DURATION_MS, MAX_DURATION_MS),
            text: str_of("text")
                .filter(|t| !t.trim().is_empty())
                .map(|t| t.chars().take(40).collect())
                .unwrap_or(d.text),
            color: str_of("color")
                .as_deref()
                .and_then(Rgb::from_hex)
                .unwrap_or(d.color),
            others: p
                .get("others")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or(d.others),
            finale: p
                .get("finale")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or(d.finale),
            matrix_prop_id: str_of("matrixPropId").filter(|s| !s.is_empty()),
        }
    }

    /// As preset params (inverse of [`CountdownSpec::from_params`]).
    pub fn to_params(&self) -> EffectParams {
        let mut p = EffectParams::new();
        p.insert("durationMs".into(), json!(self.duration_ms));
        p.insert("text".into(), json!(self.text));
        p.insert("color".into(), json!(self.color.to_hex()));
        p.insert("others".into(), json!(self.others));
        p.insert("finale".into(), json!(self.finale));
        if let Some(id) = &self.matrix_prop_id {
            p.insert("matrixPropId".into(), json!(id));
        }
        p
    }

    /// Whole seconds left at `t_ms` (rounded up: "10" at the start, "1" in the
    /// last second, "0" from zero on).
    pub fn seconds_left(&self, t_ms: f64) -> u64 {
        let rem = (self.duration_ms as f64 - t_ms.max(0.0)).max(0.0);
        (rem / 1000.0).ceil() as u64
    }

    /// The text shown at `t_ms`.
    pub fn text_at(&self, t_ms: f64) -> String {
        format_text(&self.text, self.seconds_left(t_ms))
    }

    /// Milliseconds since the displayed number last changed (digit changes
    /// happen at whole seconds before zero).
    pub fn since_change_ms(&self, t_ms: f64) -> f64 {
        let rem = self.duration_ms as f64 - t_ms.max(0.0);
        if rem <= 0.0 {
            return -rem;
        }
        let frac = rem % 1000.0;
        if frac == 0.0 {
            0.0
        } else {
            1000.0 - frac
        }
        .min(t_ms.max(0.0))
    }

    /// The finale flash is on at `t_ms`.
    pub fn flashing(&self, t_ms: f64) -> bool {
        let d = self.duration_ms as f64;
        self.finale == CountdownFinale::Flash && t_ms >= d - FINALE_MS && t_ms < d
    }
}

/// Fill in a countdown text template for `seconds` left.
pub fn format_text(template: &str, seconds: u64) -> String {
    template
        .replace("{mm}", &format!("{:02}", seconds / 60))
        .replace("{ss}", &format!("{:02}", seconds % 60))
        .replace("{m}", &(seconds / 60).to_string())
        .replace("{s}", &seconds.to_string())
}

/// Positions (ms from the item start) of the tick sounds: one at every digit
/// change, i.e. `duration − k × 1 s` for k ≥ 1.
pub fn tick_times(duration_ms: u64) -> Vec<u64> {
    let mut v: Vec<u64> = (1..=duration_ms / 1000)
        .map(|k| duration_ms - k * 1000)
        .collect();
    v.reverse();
    v
}

/// The matrix prop a countdown draws its digits on: `wanted` when it is a
/// matrix, else the largest matrix prop of the show.
pub fn pick_matrix<'a>(show: &'a Show, wanted: Option<&str>) -> Option<&'a Prop> {
    let usable = |p: &&Prop| p.matrix.as_ref().is_some_and(|m| m.width > 0 && m.height > 0);
    if let Some(id) = wanted {
        if let Some(p) = show.prop(id).filter(usable) {
            return Some(p);
        }
    }
    show.props
        .iter()
        .filter(usable)
        .max_by_key(|p| (p.kind == PropKind::Matrix, p.pixel_count))
}

/// The preset a countdown playlist item renders as (all props; the matrix
/// prop resolved so followers draw on the same one).
#[allow(clippy::too_many_arguments)]
pub fn countdown_preset(
    show: &Show,
    item_id: &str,
    duration_ms: u64,
    matrix_prop_id: Option<&str>,
    text: &str,
    color: Option<&str>,
    others: CountdownOthers,
    finale: CountdownFinale,
) -> EffectPreset {
    let spec = CountdownSpec {
        duration_ms: duration_ms.clamp(MIN_DURATION_MS, MAX_DURATION_MS),
        text: if text.trim().is_empty() {
            "{s}".into()
        } else {
            text.to_string()
        },
        color: color.and_then(Rgb::from_hex).unwrap_or(Rgb::WHITE),
        others,
        finale,
        matrix_prop_id: pick_matrix(show, matrix_prop_id).map(|p| p.id.clone()),
    };
    EffectPreset {
        id: format!("{PRESET_PREFIX}{item_id}"),
        name: "Countdown".into(),
        effect: EffectKind::Countdown,
        params: spec.to_params(),
        target: Target {
            all: true,
            ..Default::default()
        },
    }
}

/// What the renderer keeps for a countdown.
#[derive(Debug, Clone)]
pub(crate) struct CountdownKernel {
    pub spec: CountdownSpec,
    /// The matrix prop's geometry, when it is among the rendered props.
    pub matrix: Option<(String, MatrixInfo)>,
}

impl CountdownKernel {
    pub fn new(params: &EffectParams, props: &[&Prop]) -> Self {
        let spec = CountdownSpec::from_params(params);
        let matrix = spec.matrix_prop_id.as_deref().and_then(|id| {
            props
                .iter()
                .find(|p| p.id == id)
                .and_then(|p| p.matrix.clone().map(|m| (p.id.clone(), m)))
        });
        CountdownKernel { spec, matrix }
    }

    /// Render prop `id` (`n` pixels, `along(i)` its position 0..1) at `t_ms`.
    pub fn render(
        &self,
        id: &str,
        n: usize,
        along: impl Fn(usize) -> f32,
        t_ms: f64,
        mut put: impl FnMut(usize, Rgb),
    ) {
        let s = &self.spec;
        let d = s.duration_ms as f64;
        if t_ms >= d || t_ms < 0.0 {
            (0..n).for_each(|i| put(i, Rgb::BLACK));
            return;
        }
        if s.flashing(t_ms) {
            (0..n).for_each(|i| put(i, Rgb::WHITE));
            return;
        }
        let accent = (-(s.since_change_ms(t_ms)) / PULSE_DECAY_MS).exp() as f32;
        if let Some((mid, m)) = &self.matrix {
            if mid == id {
                (0..n).for_each(|i| put(i, Rgb::BLACK));
                let grid = digits_grid(&s.text_at(t_ms), s.color.scale(0.8 + 0.2 * accent), m);
                for y in 0..m.height.min(grid.height()) {
                    for x in 0..m.width.min(grid.width()) {
                        let Some(&idx) = m.pixel_map.get((y * m.width + x) as usize) else {
                            continue;
                        };
                        if let (Ok(idx), Some(c)) =
                            (usize::try_from(idx), grid.get(i64::from(x), i64::from(y)))
                        {
                            if idx < n {
                                put(idx, c);
                            }
                        }
                    }
                }
                return;
            }
        }
        match s.others {
            CountdownOthers::Dark => (0..n).for_each(|i| put(i, Rgb::BLACK)),
            CountdownOthers::Pulse => {
                let c = s.color.scale(0.12 + 0.88 * accent);
                (0..n).for_each(|i| put(i, c));
            }
            CountdownOthers::Fill => {
                // A progress bar along each prop, with a soft leading pixel.
                let level = (t_ms / d) as f32 * n as f32;
                for i in 0..n {
                    let pos = along(i) * n.saturating_sub(1).max(1) as f32;
                    let b = (level - pos).clamp(0.0, 1.0);
                    put(i, s.color.scale(b));
                }
            }
        }
    }
}

/// The digits drawn centred on a `m.width × m.height` grid at the largest
/// scale that fits (the 5×7 font when it fits, else 3×5).
fn digits_grid(text: &str, color: Rgb, m: &MatrixInfo) -> RgbGrid {
    let (w, h) = (m.width, m.height);
    let font = if text_width(text, Font::Medium, 1) <= w && h >= Font::Medium.height() {
        Font::Medium
    } else {
        Font::Small
    };
    let scale = best_scale(text, font, w, h);
    let mut grid = RgbGrid::new(w, h);
    let tw = i64::from(text_width(text, font, scale));
    let th = i64::from(font.height() * scale);
    draw_text(
        &mut grid,
        text,
        font,
        scale,
        (i64::from(w) - tw) / 2,
        (i64::from(h) - th) / 2,
        color,
    );
    grid
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::EffectRenderer;
    use crate::model::PropKind;

    fn prop(id: &str, kind: PropKind, n: u32) -> Prop {
        Prop {
            suspect_pixels: vec![],
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
            layout: None,
            matrix: None,
            color: None,
            max_milliamps_per_pixel: None,
            notes: None,
        }
    }

    fn matrix(w: u32, h: u32) -> Prop {
        let mut p = prop("m", PropKind::Matrix, w * h);
        p.matrix = Some(MatrixInfo {
            width: w,
            height: h,
            pixel_map: (0..(w * h) as i32).collect(),
        });
        p
    }

    fn show() -> Show {
        Show {
            props: vec![prop("line", PropKind::Line, 10), matrix(16, 8)],
            ..Default::default()
        }
    }

    fn render(r: &EffectRenderer, t: u64) -> Vec<u8> {
        let mut v = vec![0u8; r.frame_len()];
        r.render(t, &mut v);
        v
    }

    fn lit(frame: &[u8]) -> usize {
        frame.chunks(3).filter(|c| c.iter().any(|&b| b > 0)).count()
    }

    #[test]
    fn text_templates() {
        assert_eq!(format_text("{s}", 9), "9");
        assert_eq!(format_text("SHOW IN {s}", 10), "SHOW IN 10");
        assert_eq!(format_text("{mm}:{ss}", 75), "01:15");
        let s = CountdownSpec::default();
        assert_eq!(s.seconds_left(0.0), 10);
        assert_eq!(s.seconds_left(9_000.0), 1);
        assert_eq!(s.seconds_left(9_999.0), 1);
        assert_eq!(s.seconds_left(10_000.0), 0);
        assert_eq!(s.since_change_ms(1_250.0), 250.0);
        assert_eq!(tick_times(3_000), vec![0, 1_000, 2_000]);
        assert_eq!(tick_times(2_500), vec![500, 1_500]);
    }

    #[test]
    fn preset_round_trips_and_picks_the_matrix() {
        let s = show();
        let p = countdown_preset(
            &s,
            "i1",
            12_000,
            None,
            "{s}",
            Some("#ff0000"),
            CountdownOthers::Pulse,
            CountdownFinale::None,
        );
        assert_eq!(p.id, "countdown:i1");
        let spec = CountdownSpec::from_params(&p.params);
        assert_eq!(spec.matrix_prop_id.as_deref(), Some("m"));
        assert_eq!(spec.duration_ms, 12_000);
        assert_eq!(spec.color, Rgb::new(255, 0, 0));
        assert_eq!(spec.others, CountdownOthers::Pulse);
        assert_eq!(spec.to_params(), p.params);
        // A non-matrix "matrix" is ignored.
        let p = countdown_preset(&s, "i", 5_000, Some("line"), "", None, CountdownOthers::Fill, CountdownFinale::Flash);
        assert_eq!(CountdownSpec::from_params(&p.params).matrix_prop_id.as_deref(), Some("m"));
        // Garbage params still count down.
        let mut bad = EffectParams::new();
        bad.insert("durationMs".into(), json!("soon"));
        bad.insert("others".into(), json!("sideways"));
        let spec = CountdownSpec::from_params(&bad);
        assert_eq!((spec.duration_ms, spec.others), (10_000, CountdownOthers::Fill));
    }

    #[test]
    fn digits_on_the_matrix_fill_on_the_others_and_finale() {
        let s = show();
        let p = countdown_preset(&s, "i", 10_000, None, "{s}", None, CountdownOthers::Fill, CountdownFinale::Flash);
        let props: Vec<&Prop> = s.props.iter().collect();
        let r = EffectRenderer::new(&p, &props);
        let f0 = render(&r, 0);
        let (line, mat) = f0.split_at(30);
        assert_eq!(lit(line), 0, "the bar starts empty");
        assert!(lit(mat) > 10, "\"10\" is drawn");
        let half = render(&r, 5_000);
        assert!((4..=6).contains(&lit(&half[..30])), "half full: {}", lit(&half[..30]));
        // Digits change with the seconds.
        assert_ne!(render(&r, 1_500)[30..], render(&r, 2_500)[30..]);
        // Finale: everything white just before zero, dark after.
        let fin = render(&r, 9_900);
        assert!(fin.iter().all(|&b| b == 255));
        assert!(render(&r, 10_000).iter().all(|&b| b == 0));
        // Pulse and dark modes.
        let p = countdown_preset(&s, "i", 10_000, None, "{s}", None, CountdownOthers::Pulse, CountdownFinale::None);
        let r = EffectRenderer::new(&p, &props);
        assert!(render(&r, 1_010)[0] > render(&r, 1_900)[0], "pulses decay within each second");
        assert!(render(&r, 9_900)[..30].iter().any(|&b| b < 255), "no finale");
        let p = countdown_preset(&s, "i", 10_000, None, "{s}", None, CountdownOthers::Dark, CountdownFinale::None);
        let r = EffectRenderer::new(&p, &props);
        assert_eq!(lit(&render(&r, 3_000)[..30]), 0);
    }

    #[test]
    fn leader_and_follower_render_identically() {
        // A follower that has only the matrix renders the same pixels for it
        // as the leader rendering everything (the preset carries the matrix id).
        let s = show();
        let p = countdown_preset(&s, "i", 30_000, None, "SHOW IN {s}", Some("#00ff00"), CountdownOthers::Fill, CountdownFinale::Flash);
        let all: Vec<&Prop> = s.props.iter().collect();
        let leader = EffectRenderer::new(&p, &all);
        let follower = EffectRenderer::new(&p, &[&s.props[1]]);
        let line_only = EffectRenderer::new(&p, &[&s.props[0]]);
        for t in [0u64, 999, 1_000, 12_345, 29_850, 29_999, 30_000, 45_000] {
            let full = render(&leader, t);
            assert_eq!(full[30..], render(&follower, t)[..], "matrix at {t}");
            assert_eq!(full[..30], render(&line_only, t)[..], "line at {t}");
        }
    }

    #[test]
    fn tiny_matrix_uses_the_small_font() {
        let m = MatrixInfo {
            width: 8,
            height: 5,
            pixel_map: (0..40).collect(),
        };
        let g = digits_grid("10", Rgb::WHITE, &m);
        assert_eq!((g.width(), g.height()), (8, 5));
        assert!(g.as_bytes().iter().any(|&b| b > 0));
    }
}
