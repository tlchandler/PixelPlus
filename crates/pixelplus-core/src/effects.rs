//! Procedural effects ("looks"), test patterns and built-in presets.
//!
//! An [`EffectRenderer`] renders one [`EffectPreset`] onto a set of props:
//!
//! ```
//! use pixelplus_core::effects::{builtin_presets, EffectRenderer};
//! use pixelplus_core::model::{Prop, PropKind};
//!
//! let prop = Prop {
//!     id: "arch1".into(), name: "Arch".into(), kind: PropKind::Arch, pixel_count: 50,
//!     xlights_model: None, channel_start: 0, channels_per_pixel: 3, segments: vec![],
//!     group_ids: vec![], layout: None, matrix: None, color: None,
//!     max_milliamps_per_pixel: None, notes: None,
//! };
//! let preset = &builtin_presets()[0];
//! let renderer = EffectRenderer::new(preset, &[&prop]);
//! let mut frame = vec![0u8; renderer.frame_len()];
//! renderer.render(1_000, &mut frame);
//! ```
//!
//! # Determinism
//!
//! Rendering is a pure function of the preset, the props and the time: there
//! is no hidden state and no thread RNG. All randomness is derived by hashing
//! (preset id, `params.seed`, prop id, pixel index, time bucket) with
//! platform-independent integer hashes. A leader and its followers therefore
//! render identical pixels for the same effect at the same effect time, even
//! when each renders only its own subset of props.
//!
//! Display-wide effects (wave, rainbow/colorwash "across") position pixels in
//! world coordinates normalised to [`WorldBounds`]. By default those are the
//! bounds of the props passed to the renderer, which differ between a leader
//! (all props) and a follower (its own props). Before sending an effect to
//! followers, the leader should call [`stamp_world_bounds`] on the copy it
//! sends; the renderer then uses the stamped bounds everywhere.
//!
//! # Reserved parameters
//!
//! Besides the parameters in [`param_schema`], two optional hidden keys are
//! honoured: `seed` (number, varies the random pattern) and `world`
//! (`{x, y, w, h}`, see above).

mod color;
mod geometry;
mod noise;
mod params;
mod presets;
mod test_pattern;

pub use color::{palette_cyclic, palette_linear, ParseColorError, Rgb};
pub use geometry::WorldBounds;
pub use params::{param_schema, resolve_params, ParamKind, ParamSpec, MAX_COLORS};
pub use presets::builtin_presets;
pub use test_pattern::{render_test_pattern, TestPattern, DEFAULT_STEP_RATE, DEFAULT_TEST_COLOR};

use crate::model::{EffectKind, EffectPreset, Prop};
use noise::{fbm3, fnv1a, hash3, mix64, unit};
use params::Params;
use serde::{Deserialize, Serialize};
use std::f64::consts::TAU;
use std::ops::Range;

/// Every effect kind, in catalogue order.
pub const ALL_EFFECT_KINDS: [EffectKind; 13] = [
    EffectKind::Solid,
    EffectKind::Chase,
    EffectKind::Twinkle,
    EffectKind::Rainbow,
    EffectKind::Colorwash,
    EffectKind::Candycane,
    EffectKind::Fire,
    EffectKind::Snow,
    EffectKind::Sparkle,
    EffectKind::Wave,
    EffectKind::Meteor,
    EffectKind::Strobe,
    EffectKind::Breathe,
];

/// Pixels rendered per prop at most; pixels beyond this are left untouched.
/// Guards against absurd pixel counts in a damaged show file.
pub const MAX_PROP_PIXELS: usize = 1 << 20;

/// Hidden parameter key holding [`WorldBounds`] (see [`stamp_world_bounds`]).
pub const WORLD_PARAM: &str = "world";
/// Hidden parameter key varying the random pattern.
pub const SEED_PARAM: &str = "seed";

/// Catalogue entry describing an effect for the UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectInfo {
    pub kind: EffectKind,
    pub label: String,
    pub description: String,
    pub params: Vec<ParamSpec>,
}

/// Human-readable name of an effect kind.
pub fn effect_label(kind: EffectKind) -> &'static str {
    match kind {
        EffectKind::Solid => "Solid",
        EffectKind::Chase => "Chase",
        EffectKind::Twinkle => "Twinkle",
        EffectKind::Rainbow => "Rainbow",
        EffectKind::Colorwash => "Color Wash",
        EffectKind::Candycane => "Candy Cane",
        EffectKind::Fire => "Fire",
        EffectKind::Snow => "Snow",
        EffectKind::Sparkle => "Sparkle",
        EffectKind::Wave => "Wave",
        EffectKind::Meteor => "Meteor",
        EffectKind::Strobe => "Strobe",
        EffectKind::Breathe => "Breathe",
    }
}

fn effect_description(kind: EffectKind) -> &'static str {
    match kind {
        EffectKind::Solid => "Every pixel one steady colour.",
        EffectKind::Chase => "Bands of colour running along each prop.",
        EffectKind::Twinkle => "Pixels gently fade in and out at random.",
        EffectKind::Rainbow => "A flowing rainbow along each prop or across the display.",
        EffectKind::Colorwash => "The whole display slowly blends through a list of colours.",
        EffectKind::Candycane => "Moving stripes, like a candy cane.",
        EffectKind::Fire => "Flickering flames rising from the bottom of each prop.",
        EffectKind::Snow => "Snowflakes drifting down.",
        EffectKind::Sparkle => "Quick glints over a background colour.",
        EffectKind::Wave => "Smooth waves of colour rolling across the display.",
        EffectKind::Meteor => "Shooting stars with fading tails.",
        EffectKind::Strobe => "Fast flashes.",
        EffectKind::Breathe => "Slowly brightens and dims, like breathing.",
    }
}

/// Every effect with its label, description and parameter schema.
pub fn effect_catalog() -> Vec<EffectInfo> {
    ALL_EFFECT_KINDS
        .iter()
        .map(|&kind| EffectInfo {
            kind,
            label: effect_label(kind).into(),
            description: effect_description(kind).into(),
            params: param_schema(kind),
        })
        .collect()
}

/// Store the world bounds of `all_props` in the preset's hidden `world`
/// parameter, so that followers rendering a subset of props lay out
/// display-wide effects identically. Stamp the copy you send, not the stored
/// preset.
pub fn stamp_world_bounds(preset: &mut EffectPreset, all_props: &[Prop]) {
    let b = WorldBounds::of_props(all_props);
    if let Ok(v) = serde_json::to_value(b) {
        preset.params.insert(WORLD_PARAM.into(), v);
    }
}

// ---------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------

/// Per-prop precomputed data.
#[derive(Debug, Clone)]
struct PropCtx {
    id: String,
    /// Hash key for this prop's randomness.
    key: u64,
    /// Rendered pixel count.
    n: usize,
    /// Byte offset of this prop in the concatenated frame.
    offset: usize,
    /// Position within the prop's own box: x right, y **up**, 0..1.
    local: Vec<[f32; 2]>,
    /// Position within the world bounds: x right, y **up**, 0..1.
    world: Vec<[f32; 2]>,
    /// Whether the prop has (almost) no vertical extent, e.g. a roof line:
    /// vertical effects then run along the pixels instead.
    flat: bool,
}

impl PropCtx {
    fn along(&self, i: usize) -> f32 {
        if self.n <= 1 {
            0.0
        } else {
            i as f32 / (self.n - 1) as f32
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaveDir {
    Right,
    Left,
    Up,
    Down,
    Out,
    In,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StrobePattern {
    All,
    Random,
    Alternate,
}

/// Typed, resolved parameters for each effect.
#[derive(Debug, Clone)]
enum Kernel {
    Solid(Rgb),
    Chase {
        colors: Vec<Rgb>,
        background: Rgb,
        speed: f64,
        size: f64,
        gap: f64,
        reverse: bool,
        fade: bool,
    },
    Twinkle {
        colors: Vec<Rgb>,
        density: f32,
        speed: f64,
        glow: f32,
    },
    Rainbow {
        speed: f64,
        spread: f64,
        saturation: f32,
        across: bool,
        reverse: bool,
    },
    Colorwash {
        colors: Vec<Rgb>,
        speed: f64,
        spread: f64,
    },
    Candycane {
        colors: Vec<Rgb>,
        width: f64,
        speed: f64,
        reverse: bool,
    },
    Fire {
        palette: &'static [Rgb],
        height: f32,
        speed: f64,
    },
    Snow {
        color: Rgb,
        background: Rgb,
        density: f32,
        speed: f64,
        size: f32,
        wind: f32,
    },
    Sparkle {
        colors: Vec<Rgb>,
        sparkle: Rgb,
        density: f32,
        speed: f64,
    },
    Wave {
        colors: Vec<Rgb>,
        speed: f64,
        wavelength: f64,
        dir: WaveDir,
        across: bool,
    },
    Meteor {
        colors: Vec<Rgb>,
        speed: f64,
        tail: f64,
        count: usize,
        reverse: bool,
        sparkle_tail: bool,
    },
    Strobe {
        color: Rgb,
        rate: f64,
        duty: f64,
        pattern: StrobePattern,
    },
    Breathe {
        colors: Vec<Rgb>,
        period: f64,
        min: f32,
    },
}

const FIRE_CLASSIC: [Rgb; 5] = [
    Rgb::new(0, 0, 0),
    Rgb::new(128, 0, 0),
    Rgb::new(255, 48, 0),
    Rgb::new(255, 144, 0),
    Rgb::new(255, 208, 112),
];
const FIRE_EMBER: [Rgb; 4] = [
    Rgb::new(0, 0, 0),
    Rgb::new(64, 0, 0),
    Rgb::new(160, 16, 0),
    Rgb::new(255, 64, 0),
];
const FIRE_BLUE: [Rgb; 5] = [
    Rgb::new(0, 0, 0),
    Rgb::new(0, 0, 96),
    Rgb::new(0, 64, 255),
    Rgb::new(64, 192, 255),
    Rgb::new(224, 255, 255),
];
const FIRE_GREEN: [Rgb; 5] = [
    Rgb::new(0, 0, 0),
    Rgb::new(0, 48, 0),
    Rgb::new(0, 160, 32),
    Rgb::new(96, 255, 64),
    Rgb::new(224, 255, 192),
];
const FIRE_PURPLE: [Rgb; 5] = [
    Rgb::new(0, 0, 0),
    Rgb::new(48, 0, 64),
    Rgb::new(128, 0, 192),
    Rgb::new(224, 64, 255),
    Rgb::new(255, 208, 255),
];

impl Kernel {
    fn new(kind: EffectKind, p: &Params<'_>) -> Kernel {
        let reverse = p.select("direction") == "reverse";
        let f = |k: &str| f64::from(p.num(k));
        match kind {
            EffectKind::Solid => Kernel::Solid(p.color("color")),
            EffectKind::Chase => Kernel::Chase {
                colors: p.colors("colors"),
                background: p.color("background"),
                speed: f("speed"),
                size: f("size").max(1.0),
                gap: f("gap").max(0.0),
                reverse,
                fade: p.flag("fade"),
            },
            EffectKind::Twinkle => Kernel::Twinkle {
                colors: p.colors("colors"),
                density: p.num("density"),
                speed: f("speed").max(0.01),
                glow: p.num("glow"),
            },
            EffectKind::Rainbow => Kernel::Rainbow {
                speed: f("speed"),
                spread: f("spread"),
                saturation: p.num("saturation"),
                across: p.select("mode") == "across",
                reverse,
            },
            EffectKind::Colorwash => Kernel::Colorwash {
                colors: p.colors("colors"),
                speed: f("speed"),
                spread: f("spread"),
            },
            EffectKind::Candycane => Kernel::Candycane {
                colors: p.colors("colors"),
                width: f("stripeWidth").max(1.0),
                speed: f("speed"),
                reverse,
            },
            EffectKind::Fire => Kernel::Fire {
                palette: match p.select("palette") {
                    "ember" => &FIRE_EMBER,
                    "blue" => &FIRE_BLUE,
                    "green" => &FIRE_GREEN,
                    "purple" => &FIRE_PURPLE,
                    _ => &FIRE_CLASSIC,
                },
                height: p.num("height").max(0.05),
                speed: f("speed"),
            },
            EffectKind::Snow => Kernel::Snow {
                color: p.color("color"),
                background: p.color("background"),
                density: p.num("density"),
                speed: f("speed").max(0.01),
                size: p.num("flakeSize").max(0.005),
                wind: p.num("wind"),
            },
            EffectKind::Sparkle => Kernel::Sparkle {
                colors: p.colors("colors"),
                sparkle: p.color("sparkleColor"),
                density: p.num("density"),
                speed: f("speed").max(0.01),
            },
            EffectKind::Wave => Kernel::Wave {
                colors: p.colors("colors"),
                speed: f("speed"),
                wavelength: f("wavelength").max(0.01),
                dir: match p.select("direction") {
                    "left" => WaveDir::Left,
                    "up" => WaveDir::Up,
                    "down" => WaveDir::Down,
                    "out" => WaveDir::Out,
                    "in" => WaveDir::In,
                    _ => WaveDir::Right,
                },
                across: p.select("mode") != "along",
            },
            EffectKind::Meteor => Kernel::Meteor {
                colors: p.colors("colors"),
                speed: f("speed"),
                tail: f("tailLength").max(1.0),
                count: (p.num("count").round() as usize).clamp(1, 20),
                reverse,
                sparkle_tail: p.flag("sparkleTail"),
            },
            EffectKind::Strobe => Kernel::Strobe {
                color: p.color("color"),
                rate: f("rate").max(0.01),
                duty: f("duty"),
                pattern: match p.select("pattern") {
                    "random" => StrobePattern::Random,
                    "alternate" => StrobePattern::Alternate,
                    _ => StrobePattern::All,
                },
            },
            EffectKind::Breathe => Kernel::Breathe {
                colors: p.colors("colors"),
                period: f("period").max(0.1),
                min: p.num("minBrightness"),
            },
        }
    }
}

/// Renders an effect preset onto props. Cheap to render repeatedly; build a
/// new renderer when the preset or the props change.
#[derive(Debug, Clone)]
pub struct EffectRenderer {
    kind: EffectKind,
    kernel: Kernel,
    brightness: f32,
    props: Vec<PropCtx>,
    frame_len: usize,
}

impl EffectRenderer {
    /// Prepare `preset` for rendering onto `props` (in output order). The
    /// preset's `target` is not consulted: pass the props it should cover.
    /// Parameters are validated with [`resolve_params`]; nothing here panics
    /// on bad input.
    pub fn new(preset: &EffectPreset, props: &[&Prop]) -> Self {
        let resolved = resolve_params(preset.effect, &preset.params);
        let p = Params(&resolved);
        let seed_param = preset
            .params
            .get(SEED_PARAM)
            .and_then(serde_json::Value::as_f64)
            .filter(|s| s.is_finite())
            .map_or(0, |s| s as i64 as u64);
        let seed = mix64(fnv1a(&preset.id) ^ mix64(seed_param));
        let world = preset
            .params
            .get(WORLD_PARAM)
            .and_then(|v| serde_json::from_value::<WorldBounds>(v.clone()).ok())
            .unwrap_or_else(|| WorldBounds::of_props(props.iter().copied()))
            .sanitized();

        let mut offset = 0;
        let ctxs = props
            .iter()
            .map(|prop| {
                let n = (prop.pixel_count as usize).min(MAX_PROP_PIXELS);
                let local_down = geometry::local_points(prop, n);
                let world_pts = geometry::world_points(prop, &local_down, world);
                let local: Vec<[f32; 2]> = local_down.iter().map(|&[x, y]| [x, 1.0 - y]).collect();
                let (vmin, vmax) = local
                    .iter()
                    .fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[1]), b.max(p[1])));
                let world_norm = world_pts
                    .iter()
                    .map(|&[x, y]| [(x - world.x) / world.w, 1.0 - (y - world.y) / world.h])
                    .collect();
                let ctx = PropCtx {
                    id: prop.id.clone(),
                    key: mix64(seed ^ fnv1a(&prop.id)),
                    n,
                    offset,
                    local,
                    world: world_norm,
                    flat: n == 0 || vmax - vmin < 0.05,
                };
                offset += n * 3;
                ctx
            })
            .collect();

        EffectRenderer {
            kind: preset.effect,
            kernel: Kernel::new(preset.effect, &p),
            brightness: p.num("brightness") / 100.0,
            props: ctxs,
            frame_len: offset,
        }
    }

    /// The effect being rendered.
    pub fn kind(&self) -> EffectKind {
        self.kind
    }

    /// Bytes in a full frame (sum of prop pixel counts × 3).
    pub fn frame_len(&self) -> usize {
        self.frame_len
    }

    /// Number of props.
    pub fn prop_count(&self) -> usize {
        self.props.len()
    }

    /// Each prop's id and byte range within a full frame.
    pub fn prop_ranges(&self) -> impl Iterator<Item = (&str, Range<usize>)> + '_ {
        self.props
            .iter()
            .map(|p| (p.id.as_str(), p.offset..p.offset + p.n * 3))
    }

    /// Render the frame at `t_ms` (milliseconds since the effect started)
    /// into `out`: each prop's pixels as RGB in prop pixel order, props
    /// concatenated in the order given to [`EffectRenderer::new`]. If `out`
    /// is shorter than [`frame_len`](Self::frame_len) the frame is truncated;
    /// extra bytes are left untouched.
    pub fn render(&self, t_ms: u64, out: &mut [u8]) {
        let t = t_ms as f64 / 1000.0;
        for p in &self.props {
            let end = (p.offset + p.n * 3).min(out.len());
            if p.offset >= end {
                break;
            }
            self.render_ctx(p, t, &mut out[p.offset..end]);
        }
    }

    /// Render only prop number `index` (in constructor order) into `out`
    /// (`pixel_count × 3` bytes; truncated if shorter). Unknown indices are
    /// ignored.
    pub fn render_prop(&self, index: usize, t_ms: u64, out: &mut [u8]) {
        if let Some(p) = self.props.get(index) {
            let end = (p.n * 3).min(out.len());
            self.render_ctx(p, t_ms as f64 / 1000.0, &mut out[..end]);
        }
    }

    /// Render the frame at `t_ms`, handing each prop's pixels to `f` along
    /// with its id and constructor index.
    pub fn render_each(&self, t_ms: u64, mut f: impl FnMut(usize, &str, &[u8])) {
        let t = t_ms as f64 / 1000.0;
        let mut buf = Vec::new();
        for (i, p) in self.props.iter().enumerate() {
            buf.clear();
            buf.resize(p.n * 3, 0);
            self.render_ctx(p, t, &mut buf);
            f(i, &p.id, &buf);
        }
    }

    /// Render one prop. `out` may be shorter than the prop.
    fn render_ctx(&self, p: &PropCtx, t: f64, out: &mut [u8]) {
        let bright = self.brightness;
        let n = p.n;
        let mut put = |i: usize, c: Rgb| {
            if let Some(px) = out.get_mut(i * 3..i * 3 + 3) {
                px.copy_from_slice(&c.scale(bright).to_array());
            }
        };
        let idx = |i: usize, reverse: bool| if reverse { n - 1 - i } else { i } as f64;
        let pick = |colors: &[Rgb], k: i64| colors[k.rem_euclid(colors.len() as i64) as usize];

        match &self.kernel {
            Kernel::Solid(c) => (0..n).for_each(|i| put(i, *c)),

            Kernel::Chase {
                colors,
                background,
                speed,
                size,
                gap,
                reverse,
                fade,
            } => {
                let period = size + gap;
                for i in 0..n {
                    let pos = idx(i, *reverse) - t * speed;
                    let band = (pos / period).floor();
                    let within = pos - band * period;
                    let c = if within < *size {
                        let c = pick(colors, band as i64);
                        if *fade {
                            c.scale((0.25 + 0.75 * (within + 1.0) / size) as f32)
                        } else {
                            c
                        }
                    } else {
                        *background
                    };
                    put(i, c);
                }
            }

            Kernel::Twinkle {
                colors,
                density,
                speed,
                glow,
            } => {
                for i in 0..n {
                    let h = hash3(p.key, i as u64, 0);
                    let period = (1.0 + 1.5 * f64::from(unit(h))) / speed;
                    let phase = t / period + f64::from(unit(mix64(h)));
                    let cycle = phase.floor();
                    let f = (phase - cycle) as f32;
                    let ch = hash3(p.key, i as u64, cycle as u64 + 1);
                    let lit = unit(mix64(ch)) < *density;
                    let c = if lit {
                        let env = (std::f32::consts::PI * f).sin().powi(2);
                        pick(colors, (ch % 1024) as i64).scale(glow + (1.0 - glow) * env)
                    } else {
                        pick(colors, (h % 1024) as i64).scale(*glow)
                    };
                    put(i, c);
                }
            }

            Kernel::Rainbow {
                speed,
                spread,
                saturation,
                across,
                reverse,
            } => {
                let shift = if *reverse { t * speed } else { -t * speed };
                for i in 0..n {
                    let pos = if *across {
                        f64::from(p.world[i][0])
                    } else {
                        f64::from(p.along(i))
                    };
                    let hue = (pos * spread + shift).rem_euclid(1.0);
                    put(i, Rgb::from_hsv(hue as f32, *saturation, 1.0));
                }
            }

            Kernel::Colorwash {
                colors,
                speed,
                spread,
            } => {
                let base = (t * speed).rem_euclid(1.0);
                for i in 0..n {
                    let pos = base + spread * f64::from(p.world[i][0]);
                    put(i, palette_cyclic(colors, pos.rem_euclid(1.0) as f32));
                }
            }

            Kernel::Candycane {
                colors,
                width,
                speed,
                reverse,
            } => {
                let edge = (1.0 / width).min(0.5);
                for i in 0..n {
                    let s = (idx(i, *reverse) - t * speed) / width;
                    let stripe = s.floor();
                    let f = s - stripe;
                    let a = pick(colors, stripe as i64);
                    let c = if f > 1.0 - edge {
                        a.lerp(
                            pick(colors, stripe as i64 + 1),
                            ((f - (1.0 - edge)) / edge) as f32,
                        )
                    } else {
                        a
                    };
                    put(i, c);
                }
            }

            Kernel::Fire {
                palette,
                height,
                speed,
            } => {
                let ts = t * speed;
                for i in 0..n {
                    let (u, v) = if p.flat {
                        (0.5, p.along(i))
                    } else {
                        (p.local[i][0], p.local[i][1])
                    };
                    let noise = fbm3(
                        p.key,
                        f64::from(u) * 3.0,
                        f64::from(v) * 2.5 - ts * 1.6,
                        ts * 0.6,
                    );
                    let fall = 1.0 - v / height;
                    let heat = (fall + (noise - 0.5) * 0.9).clamp(0.0, 1.0).powf(1.3);
                    put(i, palette_linear(palette, heat));
                }
            }

            Kernel::Snow {
                color,
                background,
                density,
                speed,
                size,
                wind,
            } => {
                let count = if *density <= 0.0 {
                    0
                } else if p.flat {
                    ((density * 8.0).round() as usize).max(1)
                } else {
                    ((density * 50.0).round() as usize).max(1)
                };
                let r = *size;
                let travel = 1.0 + 2.0 * f64::from(r);
                // At most 50 flakes (density ≤ 1): a stack array keeps the per-frame
                // render allocation-free.
                const MAX_FLAKES: usize = 50;
                let count = count.min(MAX_FLAKES);
                let mut flake_buf = [[0f32; 2]; MAX_FLAKES];
                for (slot, k) in flake_buf.iter_mut().zip(0..count as u64) {
                    *slot = {
                        let sk = speed * (0.7 + 0.6 * f64::from(unit(hash3(p.key, k, 1))));
                        let phase = t * sk / travel + f64::from(unit(hash3(p.key, k, 2)));
                        let cycle = phase.floor();
                        let f = phase - cycle;
                        let y = (1.0 + f64::from(r) - f * travel) as f32;
                        let lane = unit(hash3(p.key, k, cycle as u64 + 3));
                        let wobble = 0.02 * ((t * 1.3 + k as f64).sin() as f32);
                        let x = (lane + wind * 0.3 * (1.0 - y) + wobble).rem_euclid(1.0);
                        [x, y]
                    };
                }
                let flakes = &flake_buf[..count];
                for i in 0..n {
                    let (u, v) = if p.flat {
                        (0.5, p.along(i))
                    } else {
                        (p.local[i][0], p.local[i][1])
                    };
                    let b = flakes
                        .iter()
                        .map(|&[x, y]| {
                            let d = if p.flat {
                                (v - y).abs()
                            } else {
                                (u - x).hypot(v - y)
                            };
                            (1.0 - d / r).max(0.0)
                        })
                        .fold(0.0f32, f32::max);
                    put(i, background.lerp(*color, b));
                }
            }

            Kernel::Sparkle {
                colors,
                sparkle,
                density,
                speed,
            } => {
                const FADE_BUCKETS: u32 = 6;
                let bt = t * 20.0 * speed;
                let bucket = bt.floor();
                let frac = (bt - bucket) as f32;
                let p_spark = density * 0.25;
                for i in 0..n {
                    let bg = palette_cyclic(colors, p.along(i));
                    let mut level = 0.0f32;
                    for k in 0..FADE_BUCKETS {
                        let b = (bucket as i64 - i64::from(k)) as u64;
                        if unit(hash3(p.key, i as u64, b)) < p_spark {
                            let l = 1.0 - (k as f32 + frac) / FADE_BUCKETS as f32;
                            level = level.max(l * l);
                        }
                    }
                    put(i, bg.lerp(*sparkle, level));
                }
            }

            Kernel::Wave {
                colors,
                speed,
                wavelength,
                dir,
                across,
            } => {
                for i in 0..n {
                    let pos = if *across {
                        let [u, v] = p.world[i];
                        let radial = || ((u - 0.5).hypot(v - 0.5) * 2.0).min(1.5);
                        match dir {
                            WaveDir::Right => u,
                            WaveDir::Left => 1.0 - u,
                            WaveDir::Up => v,
                            WaveDir::Down => 1.0 - v,
                            WaveDir::Out => radial(),
                            WaveDir::In => 1.5 - radial(),
                        }
                    } else {
                        match dir {
                            WaveDir::Right | WaveDir::Up | WaveDir::Out => p.along(i),
                            _ => 1.0 - p.along(i),
                        }
                    };
                    let phase = f64::from(pos) / wavelength - t * speed;
                    let val = 0.5 + 0.5 * (TAU * phase).sin();
                    put(i, palette_linear(colors, val as f32));
                }
            }

            Kernel::Meteor {
                colors,
                speed,
                tail,
                count,
                reverse,
                sparkle_tail,
            } => {
                let span = n as f64 + tail;
                let flicker_bucket = (t * 15.0).floor() as u64;
                for i in 0..n {
                    let x = idx(i, *reverse);
                    let mut c = Rgb::BLACK;
                    for m in 0..*count {
                        let head = (t * speed + m as f64 * span / *count as f64).rem_euclid(span);
                        let d = head - x;
                        if (0.0..*tail).contains(&d) {
                            let mut b = (1.0 - d / tail).powf(1.5) as f32;
                            if *sparkle_tail && d >= 1.0 {
                                b *= 0.35 + 0.65 * unit(hash3(p.key, i as u64, flicker_bucket));
                            }
                            c = c.max(pick(colors, m as i64).scale(b));
                        }
                    }
                    put(i, c);
                }
            }

            Kernel::Strobe {
                color,
                rate,
                duty,
                pattern,
            } => {
                let phase = t * rate;
                let cycle = phase.floor();
                let on = phase - cycle < *duty;
                for i in 0..n {
                    let lit = on
                        && match pattern {
                            StrobePattern::All => true,
                            StrobePattern::Random => {
                                unit(hash3(p.key, i as u64, cycle as u64)) < 0.35
                            }
                            StrobePattern::Alternate => (i as u64 + cycle as u64) & 1 == 0,
                        };
                    put(i, if lit { *color } else { Rgb::BLACK });
                }
            }

            Kernel::Breathe {
                colors,
                period,
                min,
            } => {
                let phase = t / period;
                let cycle = phase.floor();
                let f = phase - cycle;
                let level = min + (1.0 - min) * (0.5 - 0.5 * (TAU * f).cos()) as f32;
                let c = pick(colors, cycle as i64).scale(level);
                (0..n).for_each(|i| put(i, c));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EffectParams, PropKind, PropLayout, Target};
    use serde_json::json;

    fn prop(id: &str, kind: PropKind, n: u32) -> Prop {
        Prop {
            id: id.into(),
            name: id.into(),
            kind,
            pixel_count: n,
            xlights_model: None,
            channel_start: 0,
            channels_per_pixel: 3,
            segments: vec![],
            group_ids: vec![],
            layout: None,
            matrix: None,
            color: None,
            max_milliamps_per_pixel: None,
            notes: None,
        }
    }

    fn preset(kind: EffectKind, params: serde_json::Value) -> EffectPreset {
        let params: EffectParams = serde_json::from_value(params).unwrap();
        EffectPreset {
            id: "p1".into(),
            name: "Test".into(),
            effect: kind,
            params,
            target: Target::default(),
        }
    }

    fn frame(r: &EffectRenderer, t: u64) -> Vec<u8> {
        let mut out = vec![0u8; r.frame_len()];
        r.render(t, &mut out);
        out
    }

    #[test]
    fn solid_fills_with_brightness() {
        let p = prop("a", PropKind::Line, 4);
        let r = EffectRenderer::new(
            &preset(
                EffectKind::Solid,
                json!({"color": "#ff8000", "brightness": 50}),
            ),
            &[&p],
        );
        assert_eq!(r.frame_len(), 12);
        assert_eq!(frame(&r, 0), [128, 64, 0].repeat(4));
    }

    #[test]
    fn concatenates_props_in_order() {
        let a = prop("a", PropKind::Line, 2);
        let b = prop("b", PropKind::Line, 3);
        let r = EffectRenderer::new(&preset(EffectKind::Solid, json!({})), &[&a, &b]);
        let ranges: Vec<_> = r.prop_ranges().collect();
        assert_eq!(ranges, vec![("a", 0..6), ("b", 6..15)]);
        let mut seen = vec![];
        r.render_each(0, |i, id, px| seen.push((i, id.to_string(), px.len())));
        assert_eq!(seen, vec![(0, "a".into(), 6), (1, "b".into(), 9)]);
    }

    #[test]
    fn short_and_long_buffers_are_safe() {
        let a = prop("a", PropKind::Line, 10);
        for kind in ALL_EFFECT_KINDS {
            let r = EffectRenderer::new(&preset(kind, json!({})), &[&a]);
            r.render(5, &mut [0u8; 7]);
            let mut long = vec![7u8; 40];
            r.render(5, &mut long);
            assert_eq!(&long[30..], &[7u8; 10]);
            r.render_prop(0, 5, &mut [0u8; 2]);
            r.render_prop(9, 5, &mut [0u8; 30]);
        }
    }

    #[test]
    fn chase_moves_forward_and_reverse() {
        let a = prop("a", PropKind::Line, 12);
        let params = json!({"colors": ["#ff0000"], "size": 1, "gap": 3, "speed": 1, "fade": false});
        let r = EffectRenderer::new(&preset(EffectKind::Chase, params.clone()), &[&a]);
        let lit = |f: &[u8]| {
            f.chunks(3)
                .enumerate()
                .filter(|(_, c)| c[0] > 0)
                .map(|(i, _)| i)
                .collect::<Vec<_>>()
        };
        assert_eq!(lit(&frame(&r, 0)), vec![0, 4, 8]);
        assert_eq!(lit(&frame(&r, 1000)), vec![1, 5, 9]);
        let mut rev = params;
        rev["direction"] = json!("reverse");
        let r = EffectRenderer::new(&preset(EffectKind::Chase, rev), &[&a]);
        assert_eq!(lit(&frame(&r, 1000)), vec![2, 6, 10]);
    }

    #[test]
    fn deterministic_across_renderers_and_subsets() {
        let mut a = prop("a", PropKind::Arch, 60);
        a.layout = Some(PropLayout {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 50.0,
            rotation: 0.0,
            points: None,
        });
        let mut b = prop("b", PropKind::Tree, 90);
        b.layout = Some(PropLayout {
            x: 300.0,
            y: -40.0,
            w: 60.0,
            h: 120.0,
            rotation: 15.0,
            points: None,
        });
        for kind in ALL_EFFECT_KINDS {
            let mut pr = preset(kind, json!({}));
            stamp_world_bounds(&mut pr, &[a.clone(), b.clone()]);
            let leader = EffectRenderer::new(&pr, &[&a, &b]);
            let follower = EffectRenderer::new(&pr, &[&b]);
            for t in [0u64, 17, 1234, 98_765, 86_400_000 * 3] {
                let full = frame(&leader, t);
                assert_eq!(full, frame(&EffectRenderer::new(&pr, &[&a, &b]), t));
                let mut sub = vec![0u8; follower.frame_len()];
                follower.render(t, &mut sub);
                assert_eq!(&full[180..], &sub[..], "{kind:?} at {t}");
            }
        }
    }

    #[test]
    fn seed_changes_random_effects() {
        let a = prop("a", PropKind::Line, 200);
        let r1 = EffectRenderer::new(&preset(EffectKind::Twinkle, json!({})), &[&a]);
        let r2 = EffectRenderer::new(&preset(EffectKind::Twinkle, json!({"seed": 42})), &[&a]);
        assert_ne!(frame(&r1, 5000), frame(&r2, 5000));
    }

    #[test]
    fn effects_animate_and_light_something() {
        let mut m = prop("m", PropKind::Matrix, 256);
        m.layout = Some(PropLayout {
            x: 0.0,
            y: 0.0,
            w: 32.0,
            h: 32.0,
            rotation: 0.0,
            points: None,
        });
        let line = prop("l", PropKind::Line, 100);
        for kind in ALL_EFFECT_KINDS {
            let r = EffectRenderer::new(&preset(kind, json!({})), &[&m, &line]);
            let frames: Vec<_> = (0..20).map(|k| frame(&r, 1000 + k * 170)).collect();
            assert!(
                frames.iter().any(|f| f.iter().any(|&b| b > 0)),
                "{kind:?} is dark"
            );
            if kind != EffectKind::Solid {
                assert!(
                    frames.windows(2).any(|w| w[0] != w[1]),
                    "{kind:?} never changes"
                );
            }
        }
    }

    #[test]
    fn fire_is_hotter_at_the_bottom() {
        let mut m = prop("m", PropKind::Matrix, 400);
        m.matrix = Some(crate::model::MatrixInfo {
            width: 20,
            height: 20,
            pixel_map: (0..400).collect(),
        });
        let r = EffectRenderer::new(&preset(EffectKind::Fire, json!({"height": 0.6})), &[&m]);
        let mut top = 0u64;
        let mut bottom = 0u64;
        for k in 0..10 {
            let f = frame(&r, k * 333);
            for x in 0..20 {
                let sum = |y: usize| {
                    f[(y * 20 + x) * 3..(y * 20 + x) * 3 + 3]
                        .iter()
                        .map(|&b| u64::from(b))
                        .sum::<u64>()
                };
                top += sum(0) + sum(1);
                bottom += sum(18) + sum(19);
            }
        }
        assert!(bottom > top * 3, "bottom {bottom} top {top}");
    }

    #[test]
    fn catalog_covers_all_kinds() {
        let cat = effect_catalog();
        assert_eq!(cat.len(), 13);
        let v = serde_json::to_value(&cat[5]).unwrap();
        assert_eq!(v["kind"], "candycane");
        assert_eq!(v["label"], "Candy Cane");
        assert!(v["params"].as_array().unwrap().len() > 2);
    }

    #[test]
    fn huge_and_zero_pixel_props() {
        let z = prop("z", PropKind::Arch, 0);
        let r = EffectRenderer::new(&preset(EffectKind::Snow, json!({})), &[&z]);
        assert_eq!(r.frame_len(), 0);
        r.render(0, &mut []);
        let big = prop("big", PropKind::Line, u32::MAX);
        let r = EffectRenderer::new(&preset(EffectKind::Solid, json!({})), &[&big]);
        assert_eq!(r.frame_len(), MAX_PROP_PIXELS * 3);
    }

    #[test]
    fn world_param_round_trips() {
        let mut a = prop("a", PropKind::Line, 3);
        a.layout = Some(PropLayout {
            x: 10.0,
            y: 20.0,
            w: 30.0,
            h: 40.0,
            rotation: 0.0,
            points: None,
        });
        let mut pr = preset(EffectKind::Wave, json!({}));
        stamp_world_bounds(&mut pr, &[a]);
        assert_eq!(
            pr.params[WORLD_PARAM],
            json!({"x": 10.0, "y": 20.0, "w": 30.0, "h": 40.0})
        );
    }
}
