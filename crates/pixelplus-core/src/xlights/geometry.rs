//! Per-`DisplayAs` model interpretation: pixel counts, string structure, preview
//! geometry and matrix maps.
//!
//! Point generation follows the node ordering xLights uses when it assigns channels, so
//! point `i` is the pixel at byte offset `3*i` from the model's start channel.
//! Positions are produced in *model-local* coordinates (y up, xLights convention) and
//! mapped to world coordinates according to the model's screen-location type.

use roxmltree::Node;
use std::f32::consts::PI;

use crate::layout;
use crate::model::{MatrixInfo, PropKind};

/// Attribute access with xLights' parsing quirks (C `strtol` semantics).
#[derive(Clone, Copy)]
pub(crate) struct Attrs<'a, 'i>(pub Node<'a, 'i>);

impl<'a, 'i> Attrs<'a, 'i> {
    pub fn str(&self, name: &str) -> Option<&'a str> {
        self.0
            .attribute(name)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }

    pub fn raw(&self, name: &str) -> Option<&'a str> {
        self.0.attribute(name)
    }

    /// Integer attribute (leading integer of the text, like `strtol`).
    pub fn int(&self, name: &str) -> Option<i64> {
        self.str(name).map(strtol)
    }

    /// New-style named attribute with legacy `parmN` fallback.
    pub fn named(&self, name: &str, parm: &str, default: i64) -> i64 {
        self.int(name).or_else(|| self.int(parm)).unwrap_or(default)
    }

    pub fn float(&self, name: &str) -> Option<f32> {
        self.str(name)
            .and_then(|s| s.parse::<f32>().ok())
            .filter(|v| v.is_finite())
    }

    pub fn flag(&self, name: &str) -> bool {
        matches!(
            self.str(name).map(|s| s.to_ascii_lowercase()).as_deref(),
            Some("true" | "1" | "yes")
        )
    }
}

/// C `strtol`-style parse: optional sign and leading digits, anything else ignored.
pub(crate) fn strtol(s: &str) -> i64 {
    let s = s.trim_start();
    let (neg, rest) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let mut v: i64 = 0;
    for b in rest.bytes() {
        if !b.is_ascii_digit() {
            break;
        }
        v = v.saturating_mul(10).saturating_add((b - b'0') as i64);
    }
    if neg {
        -v
    } else {
        v
    }
}

/// Upper bound on the pixels of one model; anything larger is treated as corrupt input
/// (the largest real props have tens of thousands of pixels).
pub(crate) const MAX_MODEL_PIXELS: u64 = 1_000_000;

/// Product of counts if it stays within [`MAX_MODEL_PIXELS`].
fn pixels(parts: &[u32]) -> Option<u32> {
    let p = parts
        .iter()
        .try_fold(1u64, |acc, &v| acc.checked_mul(v as u64))?;
    (p <= MAX_MODEL_PIXELS).then_some(p as u32)
}

fn too_big() -> Shape {
    Shape::skipped("pixel count is unreasonably large (corrupt file?)", 0)
}

fn clamp_count(v: i64) -> u32 {
    v.clamp(0, 1_000_000) as u32
}

/// How the model is positioned in the xLights world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Placement {
    /// Centred box: local render units × scale + world position.
    Boxed { render_w: f32, render_h: f32 },
    /// Line from world position to world position + (X2, Y2); local x in 0..=len_units.
    TwoPoint,
    /// Like two-point with the local y axis scaled by `Height`; local x in 0..=len_units.
    ThreePoint,
    /// Local points already are world points.
    World,
    /// Unknown geometry.
    None,
}

/// How a model's nodes use xLights' per-string start channels (`stringStartChan`),
/// which differ from the contiguous layout only with individual ("Advanced") start
/// channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StringChannels {
    /// Each string's nodes start at that string's start channel (arches, candy canes,
    /// matrices, trees, lines, circles, stars, spinners, ...).
    PerString,
    /// All nodes are contiguous from string 1's start channel (icicles, window frames,
    /// cubes, layered arches).
    FromFirst,
    /// All nodes are contiguous from the lowest string start channel (custom models).
    FromLowest,
}

/// Interpretation of one xLights model.
#[derive(Debug, Clone)]
pub(crate) struct Shape {
    pub kind: PropKind,
    /// Pixel (node) count.
    pub nodes: u32,
    /// Channels per node (3 for RGB nodes).
    pub channels_per_node: u32,
    /// Whole strings are a single node (dumb strings, single colour).
    pub single_node: bool,
    /// xLights `GetNumStrings` (used for per-string channel math).
    pub strings: u32,
    /// Physical strings (each goes to its own controller port).
    pub physical_strings: u32,
    /// Individual start nodes (1-based) per physical string, when the model has them.
    pub string_start_nodes: Option<Vec<u32>>,
    /// How xLights lays out node channels when strings have individual start channels.
    pub string_channels: StringChannels,
    /// Pixel positions in channel order (local coordinates, y up). May be empty.
    pub points: Vec<[f32; 2]>,
    /// Length of the local x axis for two/three-point placement.
    pub len_units: f32,
    pub placement: Placement,
    pub matrix: Option<MatrixInfo>,
    /// Reason this model cannot become a prop (still consumes channels).
    pub skip: Option<String>,
}

impl Shape {
    /// Channels the model occupies.
    pub fn channels(&self) -> u64 {
        if self.single_node {
            self.strings as u64 * self.channels_per_node as u64
        } else {
            self.nodes as u64 * self.channels_per_node as u64
        }
    }

    fn new(kind: PropKind, nodes: u32, strings: u32) -> Shape {
        Shape {
            kind,
            nodes,
            channels_per_node: 3,
            single_node: false,
            strings: strings.max(1),
            physical_strings: strings.max(1),
            string_start_nodes: None,
            string_channels: StringChannels::PerString,
            points: Vec::new(),
            len_units: 1.0,
            placement: Placement::None,
            matrix: None,
            skip: None,
        }
    }

    fn skipped(reason: impl Into<String>, channels: u32) -> Shape {
        let mut s = Shape::new(PropKind::Other, channels, 1);
        s.channels_per_node = 1;
        s.skip = Some(reason.into());
        s
    }
}

/// Channels per node and whether strings are single nodes, from `StringType`.
fn string_type(t: &str) -> (u32, bool) {
    let t = t.trim();
    let lower = t.to_ascii_lowercase();
    if lower == "3 channel rgb" {
        (3, true)
    } else if lower == "4 channel rgbw" {
        (4, true)
    } else if lower == "node single color" {
        (1, false)
    } else if lower.starts_with("single color") || lower.starts_with("strobes") {
        (1, true)
    } else if lower.ends_with(" nodes") && t.len() == "RGBW Nodes".len() {
        (4, false)
    } else {
        // "RGB Nodes", "GRB Nodes", ..., "Superstring" and anything unknown.
        (3, false)
    }
}

/// Interpret a `<model>` element.
pub(crate) fn shape(a: Attrs) -> Shape {
    let display = a.str("DisplayAs").unwrap_or("");
    let mut s = match display {
        "Arches" => arches(a),
        "Candy Canes" => candy_canes(a),
        "Matrix" | "Horiz Matrix" | "Vert Matrix" => matrix(a, display),
        d if d == "Tree" || d.starts_with("Tree ") => tree(a, d),
        "Single Line" => single_line(a),
        "Poly Line" => poly_line(a),
        "MultiPoint" => multi_point(a),
        "Circle" => circle(a),
        "Wreath" => wreath(a),
        "Star" => star(a),
        "Spinner" => spinner(a),
        "Window Frame" => window(a),
        "Icicles" => icicles(a),
        "Custom" => custom(a),
        "Sphere" => {
            let Some((s, _)) = grid_model(a, true) else {
                return too_big();
            };
            Shape {
                kind: PropKind::Other,
                points: Vec::new(),
                placement: Placement::Boxed {
                    render_w: 1.0,
                    render_h: 1.0,
                },
                matrix: None,
                ..s
            }
        }
        "Cube" => {
            let Some(n) = pixels(&[
                clamp_count(a.named("CubeWidth", "parm1", 1)),
                clamp_count(a.named("CubeHeight", "parm2", 1)),
                clamp_count(a.named("CubeDepth", "parm3", 1)),
            ]) else {
                return too_big();
            };
            let strings = clamp_count(a.int("Strings").unwrap_or(1)).clamp(1, n.max(1));
            let mut s = Shape::new(PropKind::Other, n, strings);
            s.string_channels = StringChannels::FromFirst;
            s.placement = Placement::Boxed {
                render_w: 1.0,
                render_h: 1.0,
            };
            s
        }
        "Channel Block" => Shape::skipped(
            "channel blocks are not pixels",
            clamp_count(a.named("NumChannels", "parm1", 1)),
        ),
        d if d.starts_with("Dmx") => Shape::skipped(
            "DMX fixtures are not supported",
            clamp_count(a.named("DmxChannelCount", "parm1", 1)),
        ),
        "Image" | "Label" => {
            // One (non-pixel) node that still occupies channels for `>`/`@` chaining.
            let (cpn, _) = string_type(a.str("StringType").unwrap_or("RGB Nodes"));
            let mut s = Shape::skipped("image/label models have no pixels", 1);
            s.channels_per_node = cpn;
            s
        }
        "" => Shape::skipped("model has no DisplayAs", 0),
        other => Shape::skipped(format!("unsupported model type '{other}'"), 0),
    };
    if s.skip.is_none() {
        let (cpn, single) = string_type(a.str("StringType").unwrap_or("RGB Nodes"));
        s.channels_per_node = cpn;
        s.single_node = single;
        if single || cpn != 3 {
            s.skip = Some(format!(
                "string type '{}' is not an RGB pixel string",
                a.str("StringType").unwrap_or("")
            ));
        }
        // Smart-receiver "ts" (strings per physical port).
        let ts =
            a.0.children()
                .find(|c| c.has_tag_name("ControllerConnection"))
                .and_then(|c| c.attribute("ts"))
                .map(strtol)
                .unwrap_or(0);
        if ts > 1 {
            s.physical_strings = ((s.physical_strings as i64 / ts).max(1)) as u32;
        }
    }
    s
}

fn is_ltor(a: Attrs) -> bool {
    a.str("Dir") != Some("R")
}

/// Bottom-to-top start (xLights default when `StartSide` is absent).
fn is_btot(a: Attrs) -> bool {
    !matches!(a.str("StartSide"), Some(s) if s != "B")
}

fn reverse_if(mut pts: Vec<[f32; 2]>, rev: bool) -> Vec<[f32; 2]> {
    if rev {
        pts.reverse();
    }
    pts
}

// ---------------------------------------------------------------------------

fn arches(a: Attrs) -> Shape {
    let layers: Vec<u32> = layer_sizes(a);
    let arches = clamp_count(a.named("NumArches", "parm1", 1)).max(1);
    let per = clamp_count(a.named("NodesPerArch", "parm2", 1)).max(1);
    let arc = a
        .int("Arc")
        .or_else(|| a.int("arc"))
        .unwrap_or(180)
        .clamp(1, 360) as f32;
    let sweep = arc.to_radians();
    let Some(total) = pixels(&[arches, per]) else {
        return too_big();
    };
    // xLights switches to the layered layout (one string of `NodesPerArch` nodes spread
    // over the layers) as soon as any layer size is set, whether or not the layer sizes
    // add up to `NodesPerArch`.
    let layered = !layers.is_empty();
    let mut s;
    if !layered {
        s = Shape::new(PropKind::Arch, total, arches);
        let mut pts = Vec::with_capacity(total as usize);
        for arch in 0..arches {
            for p in layout::arc_points(per as usize, sweep) {
                // arc_points is y-down around (0,0) radius 1.
                pts.push([arch as f32 * 2.0 + 1.0 + p[0], -p[1]]);
            }
        }
        s.points = reverse_if(pts, !is_ltor(a));
        s.len_units = arches as f32 * 2.0;
    } else {
        // Layered single arch: NodesPerArch nodes spread over concentric arcs.
        s = Shape::new(PropKind::Arch, per, 1);
        let n = layers.len() as f32;
        let mut pts = Vec::new();
        for (i, &sz) in layers.iter().enumerate() {
            let r = 1.0 - i as f32 / (n + 1.0);
            for p in layout::arc_points(sz as usize, sweep) {
                pts.push([1.0 + p[0] * r, -p[1] * r]);
            }
        }
        pts.resize(per as usize, [1.0, 0.0]);
        s.points = pts;
        s.len_units = 2.0;
        // Layered arches number every node from string 1's start channel.
        s.string_channels = StringChannels::FromFirst;
    }
    s.physical_strings = 1;
    s.placement = Placement::ThreePoint;
    s
}

fn candy_canes(a: Attrs) -> Shape {
    let canes = clamp_count(a.named("NumCanes", "parm1", 1)).max(1);
    let per = clamp_count(a.named("NodesPerCane", "parm2", 1)).max(1);
    let reverse = a.flag("CandyCaneReverse");
    let sticks = a.flag("CandyCaneSticks");
    let Some(total) = pixels(&[canes, per]) else {
        return too_big();
    };
    let mut s = Shape::new(PropKind::Candycane, total, canes);
    let (stick, r) = (2.5f32, 0.5f32);
    let total = if sticks { 3.0 } else { stick + PI * r };
    let mut pts = Vec::new();
    for c in 0..canes {
        let x0 = c as f32 * 1.5 + 0.25;
        for i in 0..per {
            let d = (i as f32 + 0.5) / per as f32 * total;
            let (x, y) = if sticks || d <= stick {
                (0.0, d)
            } else {
                let ang = PI - (d - stick) / r;
                (r + r * ang.cos(), stick + r * ang.sin())
            };
            let x = if reverse { 1.0 - x } else { x };
            pts.push([x0 + x, y]);
        }
    }
    s.points = reverse_if(pts, !is_ltor(a));
    s.len_units = canes as f32 * 1.5;
    s.physical_strings = 1;
    s.placement = Placement::ThreePoint;
    s
}

/// Buffer coordinates of a matrix-like model.
struct Grid {
    /// `(bufX, bufY)` per node in channel order (bufY = 0 is the bottom row).
    coords: Vec<(u32, u32)>,
    width: u32,
    height: u32,
}

/// Matrix-like models (matrix, tree, sphere): returns the shape (nodes/strings set) and
/// the node buffer coordinates in channel order.
fn grid_model(a: Attrs, vertical: bool) -> Option<(Shape, Grid)> {
    let strings = clamp_count(a.named("NumStrings", "parm1", 1)).max(1);
    let nps = clamp_count(a.named("NodesPerString", "parm2", 1)).max(1);
    let sps = clamp_count(a.named("StrandsPerString", "parm3", 1)).clamp(1, nps);
    let pps = nps / sps; // pixels per strand
    let per_string = pps * sps;
    let nodes = pixels(&[strings, per_string])?;
    let strands = strings * sps;
    let alternate = a.flag("AlternateNodes");
    let no_zig = a.flag("NoZig");
    let ltor = is_ltor(a);
    let btot = is_btot(a);
    let mut coords = Vec::with_capacity(nodes as usize);
    let alt = |i: u32, len: u32, forward: bool| -> u32 {
        let v = if i < len.div_ceil(2) {
            i * 2
        } else {
            (len - (i + 1)) * 2 + 1
        };
        if forward {
            v
        } else {
            len - 1 - v
        }
    };
    if vertical {
        for x in 0..strands {
            let seg = x % sps;
            for y in 0..pps {
                let bx = if ltor { x } else { strands - 1 - x };
                let by = if alternate {
                    alt(y, pps, btot)
                } else if no_zig {
                    if btot {
                        y
                    } else {
                        pps - 1 - y
                    }
                } else if btot == (seg & 1 == 0) {
                    y
                } else {
                    pps - 1 - y
                };
                coords.push((bx, by));
            }
        }
        let mut s = Shape::new(PropKind::Matrix, nodes, strings);
        s.placement = Placement::Boxed {
            render_w: strands as f32,
            render_h: pps as f32,
        };
        Some((
            s,
            Grid {
                coords,
                width: strands,
                height: pps,
            },
        ))
    } else {
        for y in 0..strands {
            let seg = y % sps;
            for x in 0..pps {
                let by = if btot { y } else { strands - 1 - y };
                let bx = if alternate {
                    alt(x, pps, ltor)
                } else if no_zig {
                    if ltor {
                        x
                    } else {
                        pps - 1 - x
                    }
                } else if ltor != (seg & 1 == 0) {
                    pps - 1 - x
                } else {
                    x
                };
                coords.push((bx, by));
            }
        }
        let mut s = Shape::new(PropKind::Matrix, nodes, strings);
        s.placement = Placement::Boxed {
            render_w: pps as f32,
            render_h: strands as f32,
        };
        Some((
            s,
            Grid {
                coords,
                width: pps,
                height: strands,
            },
        ))
    }
}

/// Matrix map from buffer coordinates (bufY = 0 is the bottom row).
fn matrix_from_coords(coords: &[(u32, u32)], w: u32, h: u32) -> Option<MatrixInfo> {
    let cells = (w as usize).checked_mul(h as usize)?;
    if cells == 0 || cells > 4_000_000 {
        return None;
    }
    let mut map = vec![-1i32; cells];
    for (i, &(x, y)) in coords.iter().enumerate() {
        if x < w && y < h {
            let row = (h - 1 - y) as usize;
            map[row * w as usize + x as usize] = i as i32;
        }
    }
    Some(MatrixInfo {
        width: w,
        height: h,
        pixel_map: map,
    })
}

fn matrix(a: Attrs, display: &str) -> Shape {
    let vertical = display == "Vert Matrix" || (display == "Matrix" && a.flag("Vertical"));
    let Some((
        mut s,
        Grid {
            coords,
            width: w,
            height: h,
        },
    )) = grid_model(a, vertical)
    else {
        return too_big();
    };
    s.points = coords.iter().map(|&(x, y)| [x as f32, y as f32]).collect();
    s.matrix = matrix_from_coords(&coords, w, h);
    s
}

fn tree(a: Attrs, display: &str) -> Shape {
    // Tree type: 0 = round (degrees), 1 = flat, 2 = ribbon.
    let (flat, ribbon, degrees) = match display.strip_prefix("Tree ") {
        Some("Flat") => (true, false, 0),
        Some("Ribbon") => (false, true, -1),
        Some(d) => (false, false, strtol(d)),
        None => match a.int("TreeType").unwrap_or(0) {
            1 => (true, false, 0),
            2 => (false, true, -1),
            _ => (false, false, a.int("TreeDegrees").unwrap_or(360)),
        },
    };
    // `StrandDir` (default "Vertical"): trees can also be strung horizontally.
    let vertical = !matches!(a.raw("StrandDir"), Some(d) if d != "Vertical");
    let Some((
        mut s,
        Grid {
            mut coords,
            width: w,
            height: h,
        },
    )) = grid_model(a, vertical)
    else {
        return too_big();
    };
    s.kind = PropKind::Tree;
    // `exportFirstStrand` (1-based): the strand wired first. xLights rotates the strand
    // start channels so that strand's pixels come first in channel order.
    if vertical {
        let first = a.int("exportFirstStrand").unwrap_or(0) - 1;
        if first > 0 && (first as u64) < w as u64 {
            let per_strand = h as usize;
            let k = (first as usize * per_strand).min(coords.len());
            coords.rotate_left(k);
        }
    }
    let (w_f, h_f) = (w as f32, h as f32);
    if degrees > 0 {
        let render_h = h_f * 3.0;
        let render_w = render_h / 1.8;
        let rad = (degrees.min(360) as f32).to_radians();
        let radius = render_w / 2.0;
        let ratio = a.float("TreeBottomTopRatio").unwrap_or(6.0);
        let (mut rb, mut rt) = (
            radius,
            if ratio != 0.0 {
                radius / ratio.abs()
            } else {
                radius
            },
        );
        if ratio < 0.0 {
            std::mem::swap(&mut rb, &mut rt);
        }
        let incr = if degrees < 350 && w > 1 {
            rad / (w_f - 1.0)
        } else {
            rad / w_f
        };
        let start = -rad / 2.0 + a.float("TreeRotation").unwrap_or(3.0).to_radians();
        s.points = coords
            .iter()
            .map(|&(x, y)| {
                let ang = start + x as f32 * incr;
                let t = if h > 1 { y as f32 / (h_f - 1.0) } else { 0.5 };
                let xb = rb * ang.sin();
                let xt = rt * ang.sin();
                [xb + (xt - xb) * t, render_h * t]
            })
            .collect();
        s.placement = Placement::Boxed { render_w, render_h };
    } else {
        let scale = if ribbon { 5.0 } else { 4.0 };
        let render_h = h_f * 2.0;
        s.points = coords
            .iter()
            .map(|&(x, y)| {
                let xt = (x as f32 + 0.5 - w_f / 2.0) * 0.9;
                let xb = (x as f32 + 0.5 - w_f / 2.0) * scale;
                let t = if h > 1 { y as f32 / (h_f - 1.0) } else { 0.5 };
                [xb + (xt - xb) * t, render_h * t]
            })
            .collect();
        s.placement = Placement::Boxed {
            render_w: w_f * scale + 2.0,
            render_h,
        };
        if flat {
            s.matrix = matrix_from_coords(&coords, w, h);
        }
    }
    s
}

fn single_line(a: Attrs) -> Shape {
    let strings = clamp_count(a.named("NumStrings", "parm1", 1)).max(1);
    let per = clamp_count(a.named("NodesPerString", "parm2", 50)).max(1);
    let Some(n) = pixels(&[strings, per]) else {
        return too_big();
    };
    let mut s = Shape::new(PropKind::Line, n, strings);
    s.points = (0..n).map(|i| [i as f32 + 0.5, 0.0]).collect();
    s.len_units = n as f32;
    s.placement = Placement::TwoPoint;
    s
}

fn parse_drops(a: Attrs, default: &str) -> Vec<i64> {
    let d: Vec<i64> = a
        .str("DropPattern")
        .unwrap_or(default)
        .split(',')
        .map(|v| strtol(v).clamp(-10_000, 10_000))
        .filter(|&v| v != 0)
        .collect();
    if d.is_empty() {
        vec![1]
    } else {
        d
    }
}

fn poly_points(a: Attrs) -> Vec<[f32; 2]> {
    let (wx, wy) = (
        a.float("WorldPosX").unwrap_or(0.0),
        a.float("WorldPosY").unwrap_or(0.0),
    );
    let (sx, sy) = (
        a.float("ScaleX").filter(|v| *v > 0.0).unwrap_or(1.0),
        a.float("ScaleY").filter(|v| *v > 0.0).unwrap_or(1.0),
    );
    let vals: Vec<f32> = a
        .str("PointData")
        .unwrap_or("")
        .split(',')
        .filter_map(|v| v.trim().parse::<f32>().ok())
        .collect();
    vals.chunks_exact(3)
        .map(|c| [c[0] * sx + wx, c[1] * sy + wy])
        .collect()
}

fn poly_line(a: Attrs) -> Shape {
    // xLights (PolyLineModel::ParseDropSizes) treats a 0 drop as 1 light; it is not
    // skipped, so it still takes a node.
    let drops: Vec<i64> = a
        .str("DropPattern")
        .unwrap_or("1")
        .split(',')
        .map(|v| match strtol(v).clamp(-10_000, 10_000) {
            0 => 1,
            d => d,
        })
        .collect();
    let path = poly_points(a);
    let segments = path.len().saturating_sub(1).max(1);
    let seg_sizes: Option<Vec<u32>> = if a.str("Seg1").is_some() {
        Some(
            (1..=segments)
                .map(|i| a.int(&format!("Seg{i}")).unwrap_or(0).clamp(0, 1_000_000) as u32)
                .collect(),
        )
    } else {
        None
    };
    // Drop points (positions along the line) and the lights hanging at each.
    let mut drop_lights: Vec<u32> = Vec::new();
    let mut di = 0usize;
    match &seg_sizes {
        Some(sizes) => {
            if sizes.iter().map(|&n| n as u64).sum::<u64>() > MAX_MODEL_PIXELS {
                return too_big();
            }
            for &n in sizes {
                for _ in 0..n {
                    drop_lights.push(drops[di % drops.len()].unsigned_abs() as u32);
                    di += 1;
                }
            }
        }
        None => {
            let mut lights = a.named("NodesPerString", "parm2", 0).clamp(0, 1_000_000);
            while lights > 0 {
                let d = drops[di % drops.len()].abs();
                drop_lights.push(d as u32);
                lights -= d;
                di += 1;
            }
        }
    }
    let Some(nodes) = u32::try_from(drop_lights.iter().map(|&d| d as u64).sum::<u64>())
        .ok()
        .filter(|&n| n as u64 <= MAX_MODEL_PIXELS)
    else {
        return too_big();
    };
    let strings = clamp_count(a.int("PolyStrings").unwrap_or(1)).max(1);
    let mut s = Shape::new(PropKind::Line, nodes, strings);
    if strings > 1 && a.str("PolyNode1").is_some() {
        s.string_start_nodes = Some(
            (1..=strings)
                .map(|i| clamp_count(a.int(&format!("PolyNode{i}")).unwrap_or(1)).max(1))
                .collect(),
        );
    }
    if path.len() >= 2 {
        let anchors = path_positions(&path, &seg_sizes, drop_lights.len());
        let spacing = path_length(&path) / drop_lights.len().max(1) as f32;
        let mut pts = Vec::with_capacity(nodes as usize);
        for (p, &k) in anchors.iter().zip(&drop_lights) {
            for j in 0..k {
                pts.push([p[0], p[1] - j as f32 * spacing]);
            }
        }
        s.points = pts;
        s.placement = Placement::World;
    }
    s
}

fn path_length(path: &[[f32; 2]]) -> f32 {
    path.windows(2)
        .map(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt())
        .sum()
}

/// Positions of `n` drop points along `path` (y up); per-segment counts if given.
fn path_positions(path: &[[f32; 2]], seg_sizes: &Option<Vec<u32>>, n: usize) -> Vec<[f32; 2]> {
    match seg_sizes {
        Some(sizes) => {
            let mut out = Vec::with_capacity(n);
            for (i, &k) in sizes.iter().enumerate() {
                if i + 1 >= path.len() {
                    break;
                }
                let (a, b) = (path[i], path[i + 1]);
                for j in 0..k {
                    let t = (j as f32 + 0.5) / k as f32;
                    out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
                }
            }
            out.resize(n, *path.last().unwrap_or(&[0.0, 0.0]));
            out
        }
        None => layout::along_path(path, n, true),
    }
}

fn multi_point(a: Attrs) -> Shape {
    let pts = poly_points(a);
    let strings = clamp_count(a.int("MultiStrings").unwrap_or(1)).max(1);
    let mut s = Shape::new(PropKind::Other, pts.len() as u32, strings);
    if strings > 1 && a.str("MultiNode1").is_some() {
        s.string_start_nodes = Some(
            (1..=strings)
                .map(|i| clamp_count(a.int(&format!("MultiNode{i}")).unwrap_or(1)).max(1))
                .collect(),
        );
    }
    s.points = pts;
    s.placement = Placement::World;
    s
}

fn layer_sizes(a: Attrs) -> Vec<u32> {
    a.str("LayerSizes")
        .unwrap_or("")
        .split(',')
        .map(strtol)
        .filter(|&v| v > 0)
        .map(clamp_count)
        .collect()
}

fn ring_layers(n: u32, layers: &[u32]) -> Vec<u32> {
    if layers.is_empty() || layers.iter().map(|&l| l as u64).sum::<u64>() != n as u64 {
        vec![n]
    } else {
        layers.to_vec()
    }
}

fn circle(a: Attrs) -> Shape {
    let strings = clamp_count(a.named("NumStrings", "parm1", 1)).max(1);
    let per = clamp_count(a.named("NodesPerString", "parm2", 1)).max(1);
    let Some(n) = pixels(&[strings, per]) else {
        return too_big();
    };
    let mut s = Shape::new(PropKind::Circle, n, strings);
    let layers = match a.str("circleSizes") {
        Some(c) => {
            let mut v: Vec<u32> = c
                .split(',')
                .map(strtol)
                .filter(|&v| v > 0)
                .map(clamp_count)
                .collect();
            v.reverse();
            v
        }
        None => layer_sizes(a),
    };
    let layers = ring_layers(n, &layers);
    let inside_out = a.str("InsideOut") == Some("1");
    let inner = (a.named("centerPercent", "parm3", 0).clamp(0, 100) as f32) / 100.0;
    let count = layers.len();
    let start_bottom = a.str("StartSide") == Some("B");
    let cw = is_ltor(a);
    let max_layer = *layers.iter().max().unwrap_or(&1) as f32;
    let mut pts = Vec::with_capacity(n as usize);
    for (c, &sz) in layers.iter().enumerate() {
        let frac = if count == 1 {
            0.0
        } else if inside_out {
            (count - c - 1) as f32 / (count - 1) as f32
        } else {
            c as f32 / (count - 1) as f32
        };
        let r = if count == 1 {
            1.0
        } else {
            inner + (1.0 - inner) * (1.0 - frac)
        };
        pts.extend(ring(sz, r * max_layer / 2.0, start_bottom, cw));
    }
    s.points = pts;
    s.placement = Placement::Boxed {
        render_w: max_layer,
        render_h: max_layer,
    };
    s
}

/// `n` points on a circle of radius `r` (y up), starting at the top (or bottom).
fn ring(n: u32, r: f32, start_bottom: bool, clockwise: bool) -> Vec<[f32; 2]> {
    let a0 = if start_bottom { -PI / 2.0 } else { PI / 2.0 };
    let dir = if clockwise { -1.0 } else { 1.0 };
    (0..n)
        .map(|i| {
            let ang = a0 + dir * 2.0 * PI * i as f32 / n.max(1) as f32;
            [r * ang.cos(), r * ang.sin()]
        })
        .collect()
}

fn wreath(a: Attrs) -> Shape {
    let strings = clamp_count(a.named("NumStrings", "parm1", 1)).max(1);
    let per = clamp_count(a.named("NodesPerString", "parm2", 50)).max(1);
    let Some(n) = pixels(&[strings, per]) else {
        return too_big();
    };
    let mut s = Shape::new(PropKind::Circle, n, strings);
    s.points = ring(n, n as f32 / 2.0, false, true);
    s.placement = Placement::Boxed {
        render_w: n as f32 + 1.0,
        render_h: n as f32 + 1.0,
    };
    s
}

fn star(a: Attrs) -> Shape {
    let strings = clamp_count(a.named("NumStrings", "parm1", 1)).max(1);
    let per = clamp_count(a.named("NodesPerString", "parm2", 1)).max(1);
    let tips = a.named("StarPoints", "parm3", 5).clamp(2, 64) as usize;
    let ratio = a.float("starRatio").unwrap_or(2.618);
    let Some(n) = pixels(&[strings, per]) else {
        return too_big();
    };
    let mut s = Shape::new(PropKind::Star, n, strings);
    let raw_layers = if a.str("starSizes").is_some() {
        a.str("starSizes")
            .unwrap_or("")
            .split(',')
            .map(strtol)
            .filter(|&v| v > 0)
            .map(clamp_count)
            .collect()
    } else {
        layer_sizes(a)
    };
    let layers = ring_layers(n, &raw_layers);
    let count = layers.len();
    let mut pts = Vec::with_capacity(n as usize);
    let mut max_render = 1.0f32;
    for (l, &sz) in layers.iter().enumerate() {
        // Outermost layer first, as xLights walks layers from the outside in.
        let scale = 1.0 - l as f32 / (count as f32 + 1.0);
        let outside = (count - l - 1) as f32;
        max_render = max_render.max(1.0 + (sz as f32 * (1.0 + outside / count as f32)).floor());
        for p in layout::star_points(sz as usize, tips, ratio) {
            pts.push([p[0] * scale, -p[1] * scale]);
        }
    }
    s.points = pts;
    s.placement = Placement::Boxed {
        render_w: max_render,
        render_h: max_render,
    };
    s
}

fn spinner(a: Attrs) -> Shape {
    let strings = clamp_count(a.named("NumStrings", "parm1", 1)).max(1);
    let per_arm = clamp_count(a.named("NodesPerArm", "parm2", 1)).max(1);
    let arms_per = clamp_count(a.named("ArmsPerString", "parm3", 1)).max(1);
    let hollow = a.int("Hollow").unwrap_or(20).clamp(0, 95) as f32;
    let start = a.int("StartAngle").unwrap_or(0) as f32;
    let arc = a.int("Arc").unwrap_or(360).clamp(1, 360) as f32;
    let zig = a.flag("ZigZag");
    let Some(n) = pixels(&[strings, arms_per, per_arm]) else {
        return too_big();
    };
    let arms = strings * arms_per;
    let mut s = Shape::new(PropKind::Spinner, n, strings);
    let inner = hollow / 100.0;
    let step = if arc >= 360.0 {
        arc / arms as f32
    } else {
        arc / (arms.max(2) - 1) as f32
    };
    let cw = is_ltor(a);
    let mut pts = Vec::with_capacity(n as usize);
    for arm in 0..arms {
        let deg = start + arm as f32 * step * if cw { 1.0 } else { -1.0 };
        let ang = (90.0 - deg).to_radians();
        for k in 0..per_arm {
            let k = if zig && arm % 2 == 1 {
                per_arm - 1 - k
            } else {
                k
            };
            let r = inner + (1.0 - inner) * (k as f32 + 0.5) / per_arm as f32;
            pts.push([r * ang.cos(), r * ang.sin()]);
        }
    }
    s.points = pts;
    let pa = per_arm as f32;
    s.placement = Placement::Boxed {
        render_w: 2.0 * pa + 3.0 + hollow * 4.0 * pa / 100.0,
        render_h: 2.0 * pa + 3.0 + hollow * 2.0 * pa / 100.0,
    };
    s
}

fn window(a: Attrs) -> Shape {
    let top = clamp_count(a.named("TopNodes", "parm1", 0));
    let side = clamp_count(a.named("SideNodes", "parm2", 0));
    let bottom = clamp_count(a.named("BottomNodes", "parm3", 0));
    let Some(n) = u32::try_from(top as u64 + 2 * side as u64 + bottom as u64)
        .ok()
        .filter(|&n| n as u64 <= MAX_MODEL_PIXELS)
    else {
        return too_big();
    };
    let mut s = Shape::new(PropKind::Window, n, 1);
    s.string_channels = StringChannels::FromFirst;
    let w = top.max(bottom) as f32 + 2.0;
    let h = side.max(1) as f32;
    let line = |k: u32, from: [f32; 2], to: [f32; 2]| -> Vec<[f32; 2]> {
        (0..k)
            .map(|i| {
                let t = (i as f32 + 0.5) / k.max(1) as f32;
                [
                    from[0] + (to[0] - from[0]) * t,
                    from[1] + (to[1] - from[1]) * t,
                ]
            })
            .collect()
    };
    let (bl, tl, tr, br) = ([0.0, 0.0], [0.0, h], [w, h], [w, 0.0]);
    let clockwise = !matches!(a.str("Rotation"), Some("CCW" | "Counter Clockwise"));
    let mut pts = Vec::with_capacity(n as usize);
    if clockwise {
        pts.extend(line(side, bl, tl));
        pts.extend(line(top, tl, tr));
        pts.extend(line(side, tr, br));
        pts.extend(line(bottom, br, bl));
    } else {
        pts.extend(line(bottom, bl, br));
        pts.extend(line(side, br, tr));
        pts.extend(line(top, tr, tl));
        pts.extend(line(side, tl, bl));
    }
    s.points = pts;
    s.placement = Placement::Boxed {
        render_w: w,
        render_h: h,
    };
    s
}

fn icicles(a: Attrs) -> Shape {
    let strings = clamp_count(a.named("NumStrings", "parm1", 1)).max(1);
    let per = clamp_count(a.named("NodesPerString", "parm2", 1)).max(1);
    let drops: Vec<u32> = parse_drops(a, "3,4,5,4")
        .iter()
        .map(|d| d.unsigned_abs().min(10_000) as u32)
        .collect();
    let Some(n) = pixels(&[strings, per]) else {
        return too_big();
    };
    let mut s = Shape::new(PropKind::Icicles, n, strings);
    s.string_channels = StringChannels::FromFirst;
    let pts = layout::icicle_points(n as usize, &drops);
    let width = pts.last().map(|p| p[0] + 1.0).unwrap_or(1.0);
    s.points = pts.iter().map(|p| [p[0] + 0.5, -p[1]]).collect();
    s.len_units = width;
    s.placement = Placement::ThreePoint;
    s
}

/// Upper bound on the cells of a custom model grid (all layers).
const MAX_CUSTOM_CELLS: usize = 16_000_000;

/// Parse `CustomModelCompressed` (`node,row,col[,layer];…`) or `CustomModel`
/// (`,`-separated cells, `;` rows, `|` layers) like xLights' `ParseCompressed` /
/// `ParseCustomModel`. Returns `layers × rows × cols` of 1-based node numbers
/// (0 = empty). Every layer has the same number of rows and columns.
pub(crate) fn custom_layers(a: Attrs) -> Vec<Vec<Vec<u32>>> {
    if let Some(c) = a.str("CustomModelCompressed") {
        let mut cells: Vec<(u32, usize, usize, usize)> = Vec::new();
        for item in c.split(';') {
            let f: Vec<i64> = item.split(',').map(strtol).collect();
            if f.len() != 3 && f.len() != 4 {
                continue;
            }
            let layer = f.get(3).copied().unwrap_or(0);
            if f[0] <= 0 || f[1] < 0 || f[2] < 0 || layer < 0 {
                continue;
            }
            if f[1] > 100_000 || f[2] > 100_000 || layer > 100_000 {
                continue;
            }
            cells.push((
                clamp_count(f[0]),
                f[1] as usize,
                f[2] as usize,
                layer as usize,
            ));
        }
        let rows = cells.iter().map(|c| c.1 + 1).max().unwrap_or(0);
        let cols = cells.iter().map(|c| c.2 + 1).max().unwrap_or(0);
        let layers = cells.iter().map(|c| c.3 + 1).max().unwrap_or(0);
        if rows.saturating_mul(cols).saturating_mul(layers) > MAX_CUSTOM_CELLS {
            return Vec::new();
        }
        let mut g = vec![vec![vec![0u32; cols]; rows]; layers];
        for (n, r, c, l) in cells {
            g[l][r][c] = n;
        }
        g
    } else {
        let data = a.raw("CustomModel").unwrap_or("");
        let mut layers: Vec<Vec<Vec<u32>>> = Vec::new();
        let mut cells = 0usize;
        for layer in data.split('|') {
            let mut rows = Vec::new();
            for row in layer.split(';') {
                let r: Vec<u32> = row
                    .split(',')
                    .map(|v| {
                        let v = v.trim();
                        if v.is_empty() {
                            0
                        } else {
                            clamp_count(strtol(v))
                        }
                    })
                    .collect();
                cells = cells.saturating_add(r.len().max(1));
                if cells > MAX_CUSTOM_CELLS {
                    return Vec::new();
                }
                rows.push(r);
            }
            layers.push(rows);
        }
        // xLights sizes every layer like the last one, and every row to the widest.
        let height = layers.last().map(|l| l.len()).unwrap_or(0);
        let width = layers.iter().flatten().map(|r| r.len()).max().unwrap_or(0);
        if width.saturating_mul(height).saturating_mul(layers.len()) > MAX_CUSTOM_CELLS {
            return Vec::new();
        }
        for l in &mut layers {
            l.resize(height, Vec::new());
            for r in l.iter_mut() {
                r.resize(width, 0);
            }
        }
        if layers.iter().flatten().flatten().all(|&v| v == 0) {
            layers.clear();
        }
        layers
    }
}

fn custom(a: Attrs) -> Shape {
    let layers = custom_layers(a);
    let depth = layers.len().max(1) as u32;
    let data_h = layers.first().map(|l| l.len()).unwrap_or(0) as u32;
    let data_w = layers
        .first()
        .and_then(|l| l.first())
        .map(|r| r.len())
        .unwrap_or(0) as u32;
    let width = clamp_count(a.named("CustomWidth", "parm1", data_w as i64))
        .max(data_w)
        .max(1);
    let height = clamp_count(a.named("CustomHeight", "parm2", data_h as i64))
        .max(data_h)
        .max(1);
    // Channels span node numbers 1..=max over *all* layers (xLights assigns node n the
    // channels at `start + (n-1)*3`, whether or not every number is used).
    let nodes = layers
        .iter()
        .flatten()
        .flatten()
        .copied()
        .max()
        .unwrap_or(0);
    let strings = clamp_count(a.int("CustomStrings").unwrap_or(1)).max(1);
    let mut s = Shape::new(PropKind::Custom, nodes, strings);
    s.string_channels = StringChannels::FromLowest;
    if strings > 1 {
        // `NodeStartN`; files older than 2020 used `StringN` when the model did not
        // also have individual start channels.
        let legacy = a.int("Advanced").unwrap_or(0) == 0;
        let starts: Vec<u32> = (1..=strings)
            .map(|i| {
                a.int(&format!("NodeStart{i}"))
                    .or_else(|| legacy.then(|| a.int(&format!("String{i}"))).flatten())
                    .map(clamp_count)
                    .unwrap_or(0)
            })
            .collect();
        if starts.iter().all(|&v| v > 0) {
            s.string_start_nodes = Some(starts);
        }
    }
    // Pixel positions: average of the cells holding that node (layers seen from the
    // front). The 2D map puts layers side by side, like xLights' default buffer.
    let mut sum = vec![[0f32; 3]; nodes as usize];
    let map_w = (width as usize).saturating_mul(depth as usize);
    let cells = map_w.saturating_mul(height as usize);
    let mut map = if cells <= 4_000_000 {
        Some(vec![-1i32; cells])
    } else {
        None
    };
    for (l, layer) in layers.iter().enumerate() {
        for (r, row) in layer.iter().enumerate() {
            for (c, &v) in row.iter().enumerate() {
                if v == 0 {
                    continue;
                }
                let i = (v - 1) as usize;
                sum[i][0] += c as f32 + 0.5;
                sum[i][1] += (height as usize - r) as f32 - 0.5;
                sum[i][2] += 1.0;
                if let Some(m) = map.as_mut() {
                    let x = l * width as usize + c;
                    if let Some(cell) = m.get_mut(r * map_w + x) {
                        if *cell < 0 {
                            *cell = i as i32;
                        }
                    }
                }
            }
        }
    }
    s.points = sum
        .iter()
        .map(|p| {
            if p[2] > 0.0 {
                [p[0] / p[2], p[1] / p[2]]
            } else {
                [width as f32 / 2.0, height as f32 / 2.0]
            }
        })
        .collect();
    s.matrix = map.map(|pixel_map| MatrixInfo {
        width: map_w as u32,
        height,
        pixel_map,
    });
    s.placement = Placement::Boxed {
        render_w: width as f32,
        render_h: height as f32,
    };
    s
}

/// World-space pixel positions (y up) for a shape and its model attributes.
pub(crate) fn world_points(s: &Shape, a: Attrs) -> Vec<[f32; 2]> {
    let (wx, wy) = (
        a.float("WorldPosX").unwrap_or(0.0),
        a.float("WorldPosY").unwrap_or(0.0),
    );
    let sx = a.float("ScaleX").filter(|v| *v > 0.0).unwrap_or(1.0);
    let sy = a.float("ScaleY").filter(|v| *v > 0.0).unwrap_or(1.0);
    match s.placement {
        Placement::None => Vec::new(),
        Placement::World => s.points.clone(),
        Placement::Boxed { render_w, render_h } => {
            // Fit the local points into the render box (uniform scale, centred), then
            // scale and rotate like xLights' BoxedScreenLocation.
            let (_, size, norm) = layout::normalize(&s.points);
            let aspect_pts = size[0] / size[1];
            let aspect_box = render_w.max(1e-3) / render_h.max(1e-3);
            let (fw, fh) = if aspect_pts > aspect_box {
                (render_w, render_w / aspect_pts)
            } else {
                (render_h * aspect_pts, render_h)
            };
            let rot = a.float("RotateZ").unwrap_or(0.0).to_radians();
            let (sn, cs) = rot.sin_cos();
            norm.iter()
                .map(|p| {
                    let lx = (p[0] - 0.5) * fw * sx;
                    let ly = (p[1] - 0.5) * fh * sy;
                    [wx + lx * cs - ly * sn, wy + lx * sn + ly * cs]
                })
                .collect()
        }
        Placement::TwoPoint | Placement::ThreePoint => {
            let x2 = a.float("X2").unwrap_or(0.0);
            let y2 = a.float("Y2").unwrap_or(0.0);
            let len = (x2 * x2 + y2 * y2).sqrt();
            let (ux, uy) = if len > 1e-6 {
                (x2 / len, y2 / len)
            } else {
                (1.0, 0.0)
            };
            let len = if len > 1e-6 {
                len
            } else {
                s.len_units.max(1.0)
            };
            let scale = len / s.len_units.max(1e-6);
            let yscale = if s.placement == Placement::ThreePoint {
                scale * a.float("Height").unwrap_or(1.0)
            } else {
                scale
            };
            // Local +y is perpendicular to the baseline (rotate direction 90° CCW).
            let (px, py) = (-uy, ux);
            s.points
                .iter()
                .map(|p| {
                    let along = p[0] * scale;
                    let up = p[1] * yscale;
                    [wx + ux * along + px * up, wy + uy * along + py * up]
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_model<R>(attrs: &str, f: impl FnOnce(Attrs) -> R) -> R {
        let xml = format!("<model {attrs}/>");
        let doc = roxmltree::Document::parse(&xml).unwrap();
        f(Attrs(doc.root_element()))
    }

    #[test]
    fn strtol_semantics() {
        assert_eq!(strtol(" 42abc"), 42);
        assert_eq!(strtol("-7"), -7);
        assert_eq!(strtol("x"), 0);
        assert_eq!(strtol("99999999999999999999999"), i64::MAX);
    }

    #[test]
    fn string_types() {
        assert_eq!(string_type("RGB Nodes"), (3, false));
        assert_eq!(string_type("GRB Nodes"), (3, false));
        assert_eq!(string_type("RGBW Nodes"), (4, false));
        assert_eq!(string_type("Single Color White"), (1, true));
        assert_eq!(string_type("3 Channel RGB"), (3, true));
        assert_eq!(string_type("Node Single Color"), (1, false));
    }

    /// Vertical matrix, 2 strings × 20 nodes, 2 strands per string, start bottom-left.
    #[test]
    fn vertical_matrix_zigzag() {
        with_model(
            r#"DisplayAs="Vert Matrix" parm1="2" parm2="20" parm3="2" StartSide="B" Dir="L""#,
            |a| {
                let s = shape(a);
                assert_eq!((s.nodes, s.strings, s.physical_strings), (40, 2, 2));
                let m = s.matrix.unwrap();
                assert_eq!((m.width, m.height), (4, 10));
                // Pixel 0 bottom-left, climbs strand 0, pixel 10 top of strand 1 going down.
                assert_eq!(m.pixel_map[9 * 4], 0);
                assert_eq!(m.pixel_map[0], 9);
                assert_eq!(m.pixel_map[1], 10);
                assert_eq!(m.pixel_map[9 * 4 + 1], 19);
                // String 2 restarts at the bottom.
                assert_eq!(m.pixel_map[9 * 4 + 2], 20);
            },
        );
    }

    #[test]
    fn horizontal_matrix_top_right_start() {
        with_model(
            r#"DisplayAs="Horiz Matrix" NumStrings="1" NodesPerString="6" StrandsPerString="3" StartSide="T" Dir="R""#,
            |a| {
                let s = shape(a);
                let m = s.matrix.unwrap();
                assert_eq!((m.width, m.height), (2, 3));
                // Top row, starting at the right.
                assert_eq!(&m.pixel_map[0..2], &[1, 0]);
                assert_eq!(&m.pixel_map[2..4], &[2, 3]);
                assert_eq!(&m.pixel_map[4..6], &[5, 4]);
            },
        );
    }

    #[test]
    fn custom_compressed_and_plain_agree() {
        let plain = with_model(
            r#"DisplayAs="Custom" parm1="3" parm2="2" CustomModel="1,,2;,3,""#,
            shape,
        );
        let comp = with_model(
            r#"DisplayAs="Custom" CustomWidth="3" CustomHeight="2" CustomModelCompressed="1,0,0;2,0,2;3,1,1""#,
            shape,
        );
        assert_eq!(plain.nodes, 3);
        assert_eq!(plain.matrix, comp.matrix);
        assert_eq!(plain.matrix.unwrap().pixel_map, vec![0, -1, 1, -1, 2, -1]);
    }

    #[test]
    fn counts_for_common_types() {
        let cases = [
            (
                r#"DisplayAs="Arches" parm1="3" parm2="25""#,
                75,
                PropKind::Arch,
                1,
            ),
            (
                r#"DisplayAs="Candy Canes" NumCanes="4" NodesPerCane="18""#,
                72,
                PropKind::Candycane,
                1,
            ),
            (
                r#"DisplayAs="Tree 360" parm1="16" parm2="50" parm3="1""#,
                800,
                PropKind::Tree,
                16,
            ),
            (
                r#"DisplayAs="Single Line" parm1="1" parm2="100""#,
                100,
                PropKind::Line,
                1,
            ),
            (
                r#"DisplayAs="Circle" parm1="1" parm2="60""#,
                60,
                PropKind::Circle,
                1,
            ),
            (
                r#"DisplayAs="Star" parm1="1" parm2="50" parm3="5""#,
                50,
                PropKind::Star,
                1,
            ),
            (
                r#"DisplayAs="Spinner" parm1="2" parm2="10" parm3="4""#,
                80,
                PropKind::Spinner,
                2,
            ),
            (
                r#"DisplayAs="Window Frame" parm1="20" parm2="10" parm3="20""#,
                60,
                PropKind::Window,
                1,
            ),
            (
                r#"DisplayAs="Icicles" parm1="1" parm2="80""#,
                80,
                PropKind::Icicles,
                1,
            ),
            (
                r#"DisplayAs="Wreath" parm1="1" parm2="40""#,
                40,
                PropKind::Circle,
                1,
            ),
        ];
        for (attrs, nodes, kind, strings) in cases {
            with_model(attrs, |a| {
                let s = shape(a);
                assert_eq!(s.nodes, nodes, "{attrs}");
                assert_eq!(s.kind, kind, "{attrs}");
                assert_eq!(s.physical_strings, strings, "{attrs}");
                assert_eq!(s.points.len(), nodes as usize, "{attrs}");
                assert!(s.skip.is_none());
            });
        }
    }

    #[test]
    fn poly_line_drop_math() {
        with_model(
            r#"DisplayAs="Poly Line" parm2="10" DropPattern="1,2" NumPoints="3" PointData="0,0,0,10,0,0,10,10,0" WorldPosX="100" WorldPosY="50""#,
            |a| {
                let s = shape(a);
                // 10 lights with drops 1,2,1,2,... -> 1+2+1+2+1+2+1 = 10
                assert_eq!(s.nodes, 10);
                assert_eq!(s.points.len(), 10);
                assert!(s.points.iter().all(|p| p[0] >= 100.0 && p[0] <= 110.0));
            },
        );
        with_model(
            r#"DisplayAs="Poly Line" NumPoints="3" PointData="0,0,0,1,0,0,1,1,0" Seg1="5" Seg2="7""#,
            |a| assert_eq!(shape(a).nodes, 12),
        );
    }

    #[test]
    fn skipped_types_still_count_channels() {
        with_model(r#"DisplayAs="Channel Block" parm1="16""#, |a| {
            let s = shape(a);
            assert!(s.skip.is_some());
            assert_eq!(s.channels(), 16);
        });
        with_model(
            r#"DisplayAs="Single Line" parm1="4" parm2="1" StringType="Single Color White""#,
            |a| {
                let s = shape(a);
                assert!(s.skip.is_some());
                assert_eq!(s.channels(), 4);
            },
        );
        with_model(
            r#"DisplayAs="Arches" parm1="1" parm2="10" StringType="RGBW Nodes""#,
            |a| assert_eq!(shape(a).channels(), 40),
        );
    }

    #[test]
    fn layered_arch_uses_nodes_per_arch_even_if_layers_do_not_add_up() {
        // xLights 2024+: NumArches="3" NodesPerArch="50" with layers set → one layered
        // arch of 50 nodes (not 150), whatever the layer sizes sum to.
        with_model(
            r#"DisplayAs="Arches" NumArches="3" NodesPerArch="50" LightsPerNode="1" LayerSizes="20,20" Arc="180" Hollow="70""#,
            |a| {
                let s = shape(a);
                assert_eq!((s.nodes, s.channels(), s.points.len()), (50, 150, 50));
            },
        );
        with_model(
            r#"DisplayAs="Arches" NumArches="3" NodesPerArch="50" LayerSizes="""#,
            |a| assert_eq!(shape(a).nodes, 150),
        );
    }

    #[test]
    fn custom_model_layers_all_count() {
        // 3D custom model: node 5 only exists on layer 1.
        let comp = with_model(
            r#"DisplayAs="Custom" CustomWidth="2" CustomHeight="2" Depth="2" CustomModelCompressed="1,0,0;2,0,1;3,1,0,0;4,1,1,1;5,0,0,1""#,
            shape,
        );
        assert_eq!(comp.nodes, 5);
        assert_eq!(comp.points.len(), 5);
        let m = comp.matrix.unwrap();
        assert_eq!((m.width, m.height), (4, 2));
        // Layer 1 sits to the right of layer 0 (xLights buffer layout).
        assert_eq!(m.pixel_map, vec![0, 1, 4, -1, 2, -1, -1, 3]);
        let plain = with_model(
            r#"DisplayAs="Custom" parm1="2" parm2="2" CustomModel="1,2;3,|5,;,4""#,
            shape,
        );
        assert_eq!(plain.nodes, 5);
        assert_eq!(plain.matrix.unwrap().pixel_map, m.pixel_map);
        // Malformed compressed entries (wrong arity, negative) are ignored.
        let bad = with_model(
            r#"DisplayAs="Custom" CustomModelCompressed="1,0,0;9,0;7,0,0,0,0;8,-1,0;2,0,1""#,
            shape,
        );
        assert_eq!(bad.nodes, 2);
    }

    #[test]
    fn custom_legacy_string_start_nodes() {
        let s = with_model(
            r#"DisplayAs="Custom" CustomStrings="2" String1="1" String2="3" CustomModel="1,2,3,4""#,
            shape,
        );
        assert_eq!(s.string_start_nodes, Some(vec![1, 3]));
        let s = with_model(
            r#"DisplayAs="Custom" CustomStrings="2" NodeStart1="1" NodeStart2="2" CustomModel="1,2,3,4""#,
            shape,
        );
        assert_eq!(s.string_start_nodes, Some(vec![1, 2]));
    }

    #[test]
    fn tree_strand_direction_and_first_strand() {
        // Flat tree, 4 strands of 5, wired starting with strand 3 (exportFirstStrand is
        // 1-based): channel order begins at buffer column 2.
        let s = with_model(
            r#"DisplayAs="Tree" TreeType="1" NumStrings="4" NodesPerString="5" StrandsPerString="1" StrandDir="Vertical" exportFirstStrand="3" StartSide="B" Dir="L""#,
            shape,
        );
        let m = s.matrix.unwrap();
        assert_eq!((m.width, m.height), (4, 5));
        let pos = |p: i32| m.pixel_map.iter().position(|&v| v == p).unwrap();
        assert_eq!(pos(0), 4 * 4 + 2); // bottom row, column 2
        assert_eq!(pos(10), 4 * 4); // strand 1 follows strand 4 (wrap-around)
        assert_eq!(pos(19), 1); // last pixel: top of column 1
                                // Horizontal strands.
        let s = with_model(
            r#"DisplayAs="Tree" TreeType="1" NumStrings="2" NodesPerString="6" StrandsPerString="1" StrandDir="Horizontal" exportFirstStrand="1""#,
            shape,
        );
        let m = s.matrix.unwrap();
        assert_eq!((m.width, m.height, s.nodes), (6, 2, 12));
    }

    #[test]
    fn image_and_label_occupy_one_node_of_channels() {
        with_model(r#"DisplayAs="Label" StringType="RGB Nodes""#, |a| {
            let s = shape(a);
            assert!(s.skip.is_some());
            assert_eq!(s.channels(), 3);
        });
        with_model(
            r#"DisplayAs="Image" StringType="Single Color White""#,
            |a| {
                assert_eq!(shape(a).channels(), 1);
            },
        );
    }

    #[test]
    fn cube_strings_are_physical_strings() {
        with_model(
            r#"DisplayAs="Cube" CubeWidth="5" CubeHeight="5" CubeDepth="4" Strings="4""#,
            |a| {
                let s = shape(a);
                assert_eq!((s.nodes, s.physical_strings), (100, 4));
            },
        );
    }

    #[test]
    fn poly_line_zero_drops_count_as_one() {
        with_model(
            r#"DisplayAs="Poly Line" NodesPerString="12" DropPattern="3,0,2" NumPoints="2" PointData="0,0,0,10,0,0""#,
            |a| {
                // Drops 3,1,2,3,1,2 → 12 lights over 6 drop points.
                let s = shape(a);
                assert_eq!(s.nodes, 12);
            },
        );
        with_model(
            r#"DisplayAs="Poly Line" DropPattern="3,0,2" NumPoints="3" PointData="0,0,0,10,0,0,20,0,0" Seg1="2" Seg2="2""#,
            |a| assert_eq!(shape(a).nodes, 3 + 1 + 2 + 3),
        );
    }

    #[test]
    fn arch_world_geometry() {
        with_model(
            r#"DisplayAs="Arches" parm1="1" parm2="11" WorldPosX="0" WorldPosY="0" X2="100" Y2="0" Height="1""#,
            |a| {
                let s = shape(a);
                let w = world_points(&s, a);
                assert!(w[0][0] < 10.0 && w[10][0] > 90.0);
                assert!((w[5][1] - 50.0).abs() < 1.0, "apex {:?}", w[5]);
            },
        );
    }
}
