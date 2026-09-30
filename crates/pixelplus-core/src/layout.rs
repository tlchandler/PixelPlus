//! 2D preview geometry for props.
//!
//! Coordinate conventions (shared with the web UI):
//!
//! * A prop's [`PropLayout`] box (`x`, `y`, `w`, `h`) lives on the show *canvas*, in
//!   arbitrary canvas units with **y pointing down** (x, y = top-left corner).
//!   Imported shows use xLights world units; auto-arranged props use roughly one unit
//!   per pixel spacing.
//! * `rotation` is in degrees, clockwise, about the box centre.
//! * `points` are per-pixel positions normalized to the box: `[0,0]` is the top-left and
//!   `[1,1]` the bottom-right corner. Pixel `i` of the prop is `points[i]`.
//!
//! When a prop has no `points` (hand-made props, or shapes the importer could not
//! model), [`default_points`] synthesizes a plausible shape from the prop kind and
//! pixel count, and [`auto_arrange`] places props without any layout on free canvas
//! space.

use crate::model::{MatrixInfo, Prop, PropKind, PropLayout};
use std::f32::consts::PI;

/// Gap between auto-arranged props, in canvas units.
pub const ARRANGE_GAP: f32 = 20.0;

/// Width of the auto-arrange canvas when there is nothing to align with.
pub const DEFAULT_CANVAS_WIDTH: f32 = 1200.0;

/// Smallest box edge used for degenerate shapes (a straight line has zero height).
const MIN_EDGE: f32 = 1.0;

/// Default per-pixel geometry for a prop of `kind` with `pixel_count` pixels,
/// normalized to the unit box (y down).
pub fn default_points(kind: PropKind, pixel_count: u32) -> Vec<[f32; 2]> {
    let n = pixel_count as usize;
    if n == 0 {
        return Vec::new();
    }
    let raw: Vec<[f32; 2]> = match kind {
        PropKind::Arch => arc_points(n, PI),
        PropKind::Candycane => cane_points(n),
        PropKind::Tree => {
            let strands = pick_strands(pixel_count);
            let per = (n / strands).max(1);
            tree_points(n, strands, per)
        }
        PropKind::Matrix | PropKind::Custom | PropKind::Other => {
            let (w, _) = grid_dims(pixel_count);
            (0..n)
                .map(|i| {
                    let (col, row) = (i % w, i / w);
                    // Serpentine rows, like most hand-built matrices.
                    let col = if row % 2 == 1 { w - 1 - col } else { col };
                    [col as f32, row as f32]
                })
                .collect()
        }
        PropKind::Line => (0..n).map(|i| [i as f32, 0.0]).collect(),
        PropKind::Circle => ring_points(n, 1.0),
        PropKind::Star => star_points(n, 5, 2.618),
        PropKind::Spinner => {
            let arms = [8usize, 6, 12, 4, 5, 3, 2]
                .into_iter()
                .find(|a| n % a == 0 && n / a >= 2)
                .unwrap_or(1);
            spinner_points(n, arms)
        }
        PropKind::Window => window_points(n),
        PropKind::Icicles => icicle_points(n, &[3, 4, 5, 4]),
    };
    normalize(&raw).2
}

/// Default box size (canvas units) for a prop without a layout.
pub fn default_size(kind: PropKind, pixel_count: u32) -> (f32, f32) {
    let n = pixel_count.max(1) as f32;
    let s = 4.0; // canvas units between neighbouring pixels
    match kind {
        PropKind::Line => ((n * s).clamp(40.0, 600.0), 10.0),
        PropKind::Arch => {
            let w = (n * s / 1.6).clamp(40.0, 300.0);
            (w, w / 2.0)
        }
        PropKind::Candycane => {
            let h = (n * s * 0.8).clamp(40.0, 200.0);
            (h / 2.0, h)
        }
        PropKind::Tree => {
            let h = (n.sqrt() * s * 3.0).clamp(80.0, 500.0);
            (h * 0.6, h)
        }
        PropKind::Matrix | PropKind::Custom | PropKind::Other => {
            let (w, h) = grid_dims(pixel_count);
            ((w as f32 * s).clamp(20.0, 600.0), (h as f32 * s).clamp(20.0, 600.0))
        }
        PropKind::Circle | PropKind::Star | PropKind::Spinner => {
            let d = (n * s / PI).clamp(40.0, 300.0);
            (d, d)
        }
        PropKind::Window => {
            let side = (n * s / 4.0).clamp(40.0, 300.0);
            (side, side * 0.75)
        }
        PropKind::Icicles => ((n * s / 4.0).clamp(40.0, 600.0), 30.0),
    }
}

/// A default layout at canvas position (`x`, `y`).
pub fn default_layout(kind: PropKind, pixel_count: u32, x: f32, y: f32) -> PropLayout {
    let (w, h) = default_size(kind, pixel_count);
    PropLayout {
        x,
        y,
        w,
        h,
        rotation: 0.0,
        points: Some(default_points(kind, pixel_count)),
    }
}

/// Normalized points of a prop: its own `layout.points` when they match the pixel
/// count, else derived from its matrix map, else [`default_points`].
pub fn prop_points(prop: &Prop) -> Vec<[f32; 2]> {
    if let Some(p) = prop.layout.as_ref().and_then(|l| l.points.as_ref()) {
        if p.len() == prop.pixel_count as usize {
            return p.clone();
        }
    }
    if let Some(m) = &prop.matrix {
        if let Some(p) = points_from_matrix(m, prop.pixel_count) {
            return p;
        }
    }
    default_points(prop.kind, prop.pixel_count)
}

/// Pixel positions of a matrix map (cell centres), normalized; `None` if some pixel is
/// not in the map.
pub fn points_from_matrix(m: &MatrixInfo, pixel_count: u32) -> Option<Vec<[f32; 2]>> {
    if m.width == 0 || m.height == 0 {
        return None;
    }
    let mut pts = vec![None; pixel_count as usize];
    for (cell, &p) in m.pixel_map.iter().enumerate() {
        if p >= 0 && (p as u32) < pixel_count && pts[p as usize].is_none() {
            let x = (cell as u32 % m.width) as f32 + 0.5;
            let y = (cell as u32 / m.width) as f32 + 0.5;
            pts[p as usize] = Some([x / m.width as f32, y / m.height as f32]);
        }
    }
    pts.into_iter().collect()
}

/// Absolute canvas positions of every pixel of `prop` (box, points and rotation
/// applied). Empty if the prop has no layout.
pub fn world_points(prop: &Prop) -> Vec<[f32; 2]> {
    let Some(l) = &prop.layout else {
        return Vec::new();
    };
    let pts = prop_points(prop);
    let (cx, cy) = (l.x + l.w / 2.0, l.y + l.h / 2.0);
    let (s, c) = l.rotation.to_radians().sin_cos();
    pts.iter()
        .map(|p| {
            let dx = l.x + p[0] * l.w - cx;
            let dy = l.y + p[1] * l.h - cy;
            // Clockwise on a y-down canvas.
            [cx + dx * c - dy * s, cy + dx * s + dy * c]
        })
        .collect()
}

/// Bounding box `(min_x, min_y, max_x, max_y)` of all laid-out props.
pub fn bounds(props: &[Prop]) -> Option<(f32, f32, f32, f32)> {
    props
        .iter()
        .filter_map(|p| p.layout.as_ref())
        .map(|l| (l.x, l.y, l.x + l.w, l.y + l.h))
        .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
}

/// Give every prop without a layout a default one, packed in rows below the existing
/// layout (or from the origin on an empty canvas). Props that already have a layout
/// but no points keep their box. Returns the number of props placed.
pub fn auto_arrange(props: &mut [Prop]) -> usize {
    let (left, top, width) = match bounds(props) {
        Some((x0, _, x1, y1)) => (x0, y1 + ARRANGE_GAP * 2.0, (x1 - x0).max(DEFAULT_CANVAS_WIDTH)),
        None => (0.0, 0.0, DEFAULT_CANVAS_WIDTH),
    };
    let mut x = left;
    let mut y = top;
    let mut row_h = 0.0f32;
    let mut placed = 0;
    for prop in props.iter_mut().filter(|p| p.layout.is_none()) {
        let (w, h) = default_size(prop.kind, prop.pixel_count);
        if x > left && x + w > left + width {
            x = left;
            y += row_h + ARRANGE_GAP;
            row_h = 0.0;
        }
        prop.layout = Some(PropLayout {
            x,
            y,
            w,
            h,
            rotation: 0.0,
            points: Some(match &prop.matrix {
                Some(m) => points_from_matrix(m, prop.pixel_count)
                    .unwrap_or_else(|| default_points(prop.kind, prop.pixel_count)),
                None => default_points(prop.kind, prop.pixel_count),
            }),
        });
        x += w + ARRANGE_GAP;
        row_h = row_h.max(h);
        placed += 1;
    }
    placed
}

/// Fit arbitrary points (y down) into a box: returns `(min, size, normalized points)`.
/// Degenerate dimensions get a size of 1 and the points are centred on that axis.
pub fn normalize(points: &[[f32; 2]]) -> ([f32; 2], [f32; 2], Vec<[f32; 2]>) {
    if points.is_empty() {
        return ([0.0, 0.0], [MIN_EDGE, MIN_EDGE], Vec::new());
    }
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    for p in points.iter().filter(|p| p[0].is_finite() && p[1].is_finite()) {
        for a in 0..2 {
            min[a] = min[a].min(p[a]);
            max[a] = max[a].max(p[a]);
        }
    }
    if !min[0].is_finite() {
        return ([0.0, 0.0], [MIN_EDGE, MIN_EDGE], vec![[0.5, 0.5]; points.len()]);
    }
    let mut size = [max[0] - min[0], max[1] - min[1]];
    let mut origin = min;
    for a in 0..2 {
        if size[a] < 1e-6 {
            origin[a] -= MIN_EDGE / 2.0;
            size[a] = MIN_EDGE;
        }
    }
    let norm = points
        .iter()
        .map(|p| {
            if p[0].is_finite() && p[1].is_finite() {
                [(p[0] - origin[0]) / size[0], (p[1] - origin[1]) / size[1]]
            } else {
                [0.5, 0.5]
            }
        })
        .collect();
    (origin, size, norm)
}

// ---------------------------------------------------------------------------
// Shape generators (y down, arbitrary units; callers normalize)
// ---------------------------------------------------------------------------

/// Pixels along an arc of `sweep` radians, starting on the left, centred on the top.
pub(crate) fn arc_points(n: usize, sweep: f32) -> Vec<[f32; 2]> {
    (0..n)
        .map(|i| {
            let t = (i as f32 + 0.5) / n as f32;
            let a = PI / 2.0 + sweep / 2.0 - t * sweep;
            [a.cos(), -a.sin()]
        })
        .collect()
}

fn cane_points(n: usize) -> Vec<[f32; 2]> {
    // Stick 2.5 high, hook radius 0.5 curving to the right.
    let stick = 2.5f32;
    let r = 0.5f32;
    let hook = PI * r;
    let total = stick + hook;
    (0..n)
        .map(|i| {
            let d = (i as f32 + 0.5) / n as f32 * total;
            if d <= stick {
                [0.0, -d]
            } else {
                let a = PI - (d - stick) / r;
                [r + r * a.cos(), -(stick + r * a.sin())]
            }
        })
        .collect()
}

fn tree_points(n: usize, strands: usize, per: usize) -> Vec<[f32; 2]> {
    // Flat cone: strands fan out from the apex; each strand runs bottom to top,
    // alternating direction (zig-zag), like a typical mega tree.
    (0..n)
        .map(|i| {
            let s = (i / per).min(strands - 1);
            let k = i % per;
            let up = if s % 2 == 0 { k } else { per - 1 - k };
            let t = if per > 1 { up as f32 / (per - 1) as f32 } else { 0.5 };
            let xb = (s as f32 + 0.5) / strands as f32 * 2.0 - 1.0;
            [xb * (1.0 - t * 0.9), -t * 2.0]
        })
        .collect()
}

fn ring_points(n: usize, r: f32) -> Vec<[f32; 2]> {
    (0..n)
        .map(|i| {
            let a = PI / 2.0 - 2.0 * PI * i as f32 / n as f32;
            [r * a.cos(), -r * a.sin()]
        })
        .collect()
}

/// Points evenly spaced along the outline of a star with `tips` points.
pub(crate) fn star_points(n: usize, tips: usize, ratio: f32) -> Vec<[f32; 2]> {
    let tips = tips.max(2);
    let ratio = if ratio.is_finite() && ratio > 0.1 { ratio } else { 2.618 };
    let verts: Vec<[f32; 2]> = (0..tips * 2)
        .map(|v| {
            let r = if v % 2 == 0 { 1.0 } else { 1.0 / ratio };
            let a = PI / 2.0 - PI * v as f32 / tips as f32;
            [r * a.cos(), -r * a.sin()]
        })
        .collect();
    let mut closed = verts.clone();
    closed.push(verts[0]);
    along_path(&closed, n, true)
}

fn spinner_points(n: usize, arms: usize) -> Vec<[f32; 2]> {
    let per = (n / arms).max(1);
    (0..n)
        .map(|i| {
            let arm = (i / per).min(arms - 1);
            let k = i % per;
            let r = 0.2 + 0.8 * (k as f32 + 0.5) / per as f32;
            let a = PI / 2.0 - 2.0 * PI * arm as f32 / arms as f32;
            [r * a.cos(), -r * a.sin()]
        })
        .collect()
}

fn window_points(n: usize) -> Vec<[f32; 2]> {
    let path = [[0.0, 1.0], [0.0, 0.0], [1.3, 0.0], [1.3, 1.0], [0.0, 1.0]];
    along_path(&path, n, true)
}

pub(crate) fn icicle_points(n: usize, pattern: &[u32]) -> Vec<[f32; 2]> {
    let pattern: Vec<u32> = pattern.iter().copied().filter(|&d| d > 0).collect();
    let pattern = if pattern.is_empty() { vec![1] } else { pattern };
    let mut out = Vec::with_capacity(n);
    let mut drop = 0usize;
    while out.len() < n {
        let len = pattern[drop % pattern.len()] as usize;
        for k in 0..len {
            if out.len() == n {
                break;
            }
            out.push([drop as f32, k as f32]);
        }
        drop += 1;
    }
    out
}

/// `n` points evenly spaced along a polyline. With `closed` the spacing treats the path
/// as a loop (no pixel exactly on the end point).
pub(crate) fn along_path(path: &[[f32; 2]], n: usize, closed: bool) -> Vec<[f32; 2]> {
    if n == 0 || path.is_empty() {
        return Vec::new();
    }
    if path.len() == 1 {
        return vec![path[0]; n];
    }
    let seg_len: Vec<f32> = path
        .windows(2)
        .map(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt())
        .collect();
    let total: f32 = seg_len.iter().sum();
    if total <= 0.0 {
        return vec![path[0]; n];
    }
    (0..n)
        .map(|i| {
            let t = if closed {
                (i as f32 + 0.5) / n as f32
            } else if n == 1 {
                0.5
            } else {
                i as f32 / (n - 1) as f32
            };
            let mut d = t * total;
            for (s, &l) in seg_len.iter().enumerate() {
                if d <= l || s == seg_len.len() - 1 {
                    let f = if l > 0.0 { (d / l).clamp(0.0, 1.0) } else { 0.0 };
                    let a = path[s];
                    let b = path[s + 1];
                    return [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f];
                }
                d -= l;
            }
            path[path.len() - 1]
        })
        .collect()
}

/// Grid dimensions (w, h) close to 2:1 landscape that hold `n` pixels.
fn grid_dims(n: u32) -> (usize, usize) {
    let n = n.max(1) as usize;
    // Prefer an exact factorization near sqrt(2n).
    let target = ((2 * n) as f32).sqrt().round() as usize;
    let mut best = None;
    for w in (1..=n).filter(|w| n % w == 0) {
        let h = n / w;
        if w >= h {
            let score = w.abs_diff(target);
            if best.map_or(true, |(_, _, s)| score < s) {
                best = Some((w, h, score));
            }
        }
    }
    match best {
        Some((w, h, _)) if h > 1 || n <= 16 => (w, h),
        _ => {
            let w = target.max(1);
            (w, n.div_ceil(w))
        }
    }
}

fn pick_strands(n: u32) -> usize {
    for s in [16u32, 12, 24, 8, 32, 10, 20, 6, 4] {
        if n % s == 0 && n / s >= 4 {
            return s as usize;
        }
    }
    (n as f32 / 25.0).ceil().clamp(1.0, 16.0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn in_unit(p: &[[f32; 2]]) -> bool {
        p.iter()
            .all(|q| (0.0..=1.0).contains(&q[0]) && (0.0..=1.0).contains(&q[1]))
    }

    #[test]
    fn default_points_for_every_kind() {
        let kinds = [
            PropKind::Arch,
            PropKind::Candycane,
            PropKind::Tree,
            PropKind::Matrix,
            PropKind::Line,
            PropKind::Circle,
            PropKind::Star,
            PropKind::Spinner,
            PropKind::Window,
            PropKind::Icicles,
            PropKind::Custom,
            PropKind::Other,
        ];
        for kind in kinds {
            for n in [0u32, 1, 2, 7, 50, 97, 400, 1600] {
                let p = default_points(kind, n);
                assert_eq!(p.len(), n as usize, "{kind:?} {n}");
                assert!(in_unit(&p), "{kind:?} {n} out of box");
                let (w, h) = default_size(kind, n);
                assert!(w > 0.0 && h > 0.0);
            }
        }
    }

    #[test]
    fn arch_is_a_rainbow_left_to_right() {
        let p = default_points(PropKind::Arch, 25);
        assert!(p[0][0] < 0.1 && p[24][0] > 0.9);
        assert!(p[12][1] < 0.05, "middle pixel at the top: {:?}", p[12]);
        assert!(p[0][1] > 0.9);
    }

    #[test]
    fn matrix_points_follow_pixel_map() {
        let m = MatrixInfo {
            width: 2,
            height: 2,
            pixel_map: vec![3, 2, 0, 1],
        };
        let p = points_from_matrix(&m, 4).unwrap();
        assert_eq!(p[0], [0.25, 0.75]);
        assert_eq!(p[3], [0.25, 0.25]);
        assert!(points_from_matrix(&m, 5).is_none());
    }

    #[test]
    fn auto_arrange_places_below_existing() {
        use crate::model::Prop;
        let mk = |kind, px, layout| Prop {
            id: "x".into(),
            name: "x".into(),
            kind,
            pixel_count: px,
            xlights_model: None,
            channel_start: 0,
            channels_per_pixel: 3,
            segments: vec![],
            group_ids: vec![],
            layout,
            matrix: None,
            color: None,
            max_milliamps_per_pixel: None,
            notes: None,
        };
        let existing = PropLayout {
            x: 100.0,
            y: 50.0,
            w: 200.0,
            h: 100.0,
            rotation: 0.0,
            points: None,
        };
        let mut props = vec![mk(PropKind::Arch, 50, Some(existing.clone()))];
        for _ in 0..30 {
            props.push(mk(PropKind::Tree, 400, None));
        }
        assert_eq!(auto_arrange(&mut props), 30);
        assert_eq!(props[0].layout.as_ref().unwrap(), &existing);
        for p in &props[1..] {
            let l = p.layout.as_ref().unwrap();
            assert!(l.y >= 150.0 && l.x >= 100.0);
            assert_eq!(l.points.as_ref().unwrap().len(), 400);
        }
        // Rows wrap: not all on one line.
        let ys: std::collections::BTreeSet<i32> =
            props[1..].iter().map(|p| p.layout.as_ref().unwrap().y as i32).collect();
        assert!(ys.len() > 1);
        assert_eq!(auto_arrange(&mut props), 0);
    }

    #[test]
    fn world_points_apply_box_and_rotation() {
        let mut p = crate::model::Prop {
            id: "a".into(),
            name: "a".into(),
            kind: PropKind::Line,
            pixel_count: 2,
            xlights_model: None,
            channel_start: 0,
            channels_per_pixel: 3,
            segments: vec![],
            group_ids: vec![],
            layout: Some(PropLayout {
                x: 10.0,
                y: 10.0,
                w: 10.0,
                h: 10.0,
                rotation: 90.0,
                points: Some(vec![[0.0, 0.5], [1.0, 0.5]]),
            }),
            matrix: None,
            color: None,
            max_milliamps_per_pixel: None,
            notes: None,
        };
        let w = world_points(&p);
        assert!((w[0][0] - 15.0).abs() < 1e-4 && (w[0][1] - 10.0).abs() < 1e-4);
        assert!((w[1][0] - 15.0).abs() < 1e-4 && (w[1][1] - 20.0).abs() < 1e-4);
        p.layout = None;
        assert!(world_points(&p).is_empty());
    }

    #[test]
    fn normalize_handles_degenerate_input() {
        let (o, s, n) = normalize(&[[5.0, 2.0], [9.0, 2.0]]);
        assert_eq!(o, [5.0, 1.5]);
        assert_eq!(s, [4.0, 1.0]);
        assert_eq!(n, vec![[0.0, 0.5], [1.0, 0.5]]);
        let (_, _, n) = normalize(&[[f32::NAN, 1.0]]);
        assert_eq!(n, vec![[0.5, 0.5]]);
    }
}
