//! Pixel positions used by spatial effects.
//!
//! Positions come from, in order of preference: `layout.points`, the matrix
//! pixel map, or a shape derived from the prop kind. The derivation mirrors
//! the web UI's preview (`web/src/lib/util/geometry.ts`) so effects look the
//! same in the preview and on the house.

use crate::model::{Prop, PropKind};
use serde::{Deserialize, Serialize};
use std::f32::consts::PI;

/// The display area effects are laid out in, in layout (world) units with y
/// pointing down. Leader and followers must use the same bounds for
/// display-wide effects to line up; see [`super::stamp_world_bounds`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldBounds {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl WorldBounds {
    /// The unit square.
    pub const UNIT: WorldBounds = WorldBounds {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    };

    /// Bounds enclosing every prop's layout box, or [`WorldBounds::UNIT`] if
    /// no prop has a layout.
    pub fn of_props<'a>(props: impl IntoIterator<Item = &'a Prop>) -> WorldBounds {
        let mut acc: Option<(f32, f32, f32, f32)> = None;
        for l in props.into_iter().filter_map(|p| p.layout.as_ref()) {
            if ![l.x, l.y, l.w, l.h].iter().all(|v| v.is_finite()) {
                continue;
            }
            let (x0, y0, x1, y1) = (l.x, l.y, l.x + l.w.max(0.0), l.y + l.h.max(0.0));
            acc = Some(match acc {
                None => (x0, y0, x1, y1),
                Some((a, b, c, d)) => (a.min(x0), b.min(y0), c.max(x1), d.max(y1)),
            });
        }
        match acc {
            Some((x0, y0, x1, y1)) => WorldBounds {
                x: x0,
                y: y0,
                w: (x1 - x0).max(1e-3),
                h: (y1 - y0).max(1e-3),
            },
            None => WorldBounds::UNIT,
        }
    }

    pub(crate) fn sanitized(self) -> WorldBounds {
        let ok = |v: f32| v.is_finite();
        if ok(self.x) && ok(self.y) && ok(self.w) && ok(self.h) && self.w > 0.0 && self.h > 0.0 {
            self
        } else {
            WorldBounds::UNIT
        }
    }
}

/// Normalised positions within the prop's own box (`x` right, `y` down,
/// 0..1), one per pixel.
pub(crate) fn local_points(prop: &Prop, n: usize) -> Vec<[f32; 2]> {
    let mut pts = derive_points(prop.kind, n, prop.matrix.as_ref());
    if let Some(given) = prop.layout.as_ref().and_then(|l| l.points.as_ref()) {
        for (dst, src) in pts.iter_mut().zip(given) {
            if src[0].is_finite() && src[1].is_finite() {
                *dst = *src;
            }
        }
    }
    pts
}

/// Map local points to world coordinates using the prop's layout box and
/// rotation (degrees, about the box centre). Props without a layout occupy the
/// whole world.
pub(crate) fn world_points(prop: &Prop, local: &[[f32; 2]], world: WorldBounds) -> Vec<[f32; 2]> {
    let (bx, by, bw, bh, rot) = match &prop.layout {
        Some(l) if [l.x, l.y, l.w, l.h].iter().all(|v| v.is_finite()) => {
            (l.x, l.y, l.w, l.h, if l.rotation.is_finite() { l.rotation } else { 0.0 })
        }
        _ => (world.x, world.y, world.w, world.h, 0.0),
    };
    let (sin, cos) = rot.to_radians().sin_cos();
    let (cx, cy) = (bx + bw / 2.0, by + bh / 2.0);
    local
        .iter()
        .map(|&[px, py]| {
            let (dx, dy) = (bx + px * bw - cx, by + py * bh - cy);
            [cx + dx * cos - dy * sin, cy + dx * sin + dy * cos]
        })
        .collect()
}

/// Shape of a prop derived from its kind (normalised, y down).
pub(crate) fn derive_points(
    kind: PropKind,
    n: usize,
    matrix: Option<&crate::model::MatrixInfo>,
) -> Vec<[f32; 2]> {
    let mut pts = vec![[0.5f32, 0.5f32]; n];
    if n == 0 {
        return pts;
    }
    let last = (n.max(2) - 1) as f32;
    match kind {
        PropKind::Arch => {
            for (i, p) in pts.iter_mut().enumerate() {
                let a = PI - (i as f32 / last) * PI;
                *p = [0.5 + 0.5 * a.cos(), 1.0 - a.sin()];
            }
        }
        PropKind::Candycane => {
            let shaft = ((n as f32 * 0.62).round() as usize).max(1);
            for (i, p) in pts.iter_mut().enumerate() {
                *p = if i < shaft {
                    [0.72, 1.0 - (i as f32 / shaft as f32) * 0.72]
                } else {
                    let t = (i - shaft) as f32 / (n.saturating_sub(shaft + 1)).max(1) as f32;
                    let a = t * PI;
                    [0.46 + 0.26 * a.cos(), 0.28 - 0.26 * a.sin()]
                };
            }
        }
        PropKind::Tree => {
            let strands = ((n as f32 / 3.0).sqrt().round() as usize).clamp(4, 24);
            let per = n.div_ceil(strands).max(1);
            for (i, p) in pts.iter_mut().enumerate() {
                let s = i / per;
                let t = (i % per) as f32 / (per.max(2) - 1) as f32;
                let bx = s as f32 / (strands - 1) as f32;
                let up = if s % 2 == 0 { t } else { 1.0 - t };
                *p = [0.5 + (bx - 0.5) * (1.0 - up), 1.0 - up];
            }
        }
        PropKind::Matrix => {
            let mut from_map = false;
            if let Some(m) = matrix.filter(|m| m.width > 0 && m.height > 0) {
                for (cell, &idx) in m.pixel_map.iter().enumerate() {
                    if let Some(p) = usize::try_from(idx).ok().and_then(|i| pts.get_mut(i)) {
                        let (x, y) = (cell % m.width as usize, cell / m.width as usize);
                        *p = [
                            (x as f32 + 0.5) / m.width as f32,
                            (y as f32 + 0.5) / m.height as f32,
                        ];
                        from_map = true;
                    }
                }
            }
            if !from_map {
                let w = ((n as f32 * 2.0).sqrt().ceil() as usize).max(1);
                let h = n.div_ceil(w).max(1);
                for (i, p) in pts.iter_mut().enumerate() {
                    *p = [
                        ((i % w) as f32 + 0.5) / w as f32,
                        ((i / w) as f32 + 0.5) / h as f32,
                    ];
                }
            }
        }
        PropKind::Circle => {
            for (i, p) in pts.iter_mut().enumerate() {
                let a = (i as f32 / n as f32) * 2.0 * PI - PI / 2.0;
                *p = [0.5 + 0.5 * a.cos(), 0.5 + 0.5 * a.sin()];
            }
        }
        PropKind::Spinner => {
            let arms = 8;
            let per = n.div_ceil(arms).max(1);
            for (i, p) in pts.iter_mut().enumerate() {
                let a = ((i / per) as f32 / arms as f32) * 2.0 * PI;
                let r = (((i % per) + 1) as f32 / per as f32) * 0.5;
                *p = [0.5 + r * a.cos(), 0.5 + r * a.sin()];
            }
        }
        PropKind::Star => {
            let verts: Vec<[f32; 2]> = (0..10)
                .map(|k| {
                    let a = (k as f32 / 10.0) * 2.0 * PI - PI / 2.0;
                    let r = if k % 2 == 0 { 0.5 } else { 0.21 };
                    [0.5 + r * a.cos(), 0.52 + r * a.sin()]
                })
                .collect();
            polyline(&verts, &mut pts);
        }
        PropKind::Window => {
            polyline(&[[0.0, 1.0], [0.0, 0.0], [1.0, 0.0], [1.0, 1.0]], &mut pts);
        }
        PropKind::Icicles => {
            let drops = ((n as f32 / 10.0).round() as usize).max(4);
            let per = n.div_ceil(drops).max(1);
            for (i, p) in pts.iter_mut().enumerate() {
                let d = i / per;
                let k = (i % per) as f32 / per as f32;
                let len = 0.35 + 0.65 * (((d as f32 * 2.3).sin() + 1.0) / 2.0);
                let down = if d % 2 == 0 { k } else { 1.0 - k };
                *p = [(d as f32 + 0.5) / drops as f32, down * len];
            }
        }
        PropKind::Line | PropKind::Custom | PropKind::Other => {
            for (i, p) in pts.iter_mut().enumerate() {
                *p = [if n == 1 { 0.5 } else { i as f32 / last }, 0.5];
            }
        }
    }
    pts
}

/// Spread `pts.len()` points evenly along a closed polyline.
fn polyline(verts: &[[f32; 2]], pts: &mut [[f32; 2]]) {
    let n = pts.len();
    let segs: Vec<([f32; 2], [f32; 2], f32)> = (0..verts.len())
        .map(|k| {
            let (a, b) = (verts[k], verts[(k + 1) % verts.len()]);
            (a, b, (b[0] - a[0]).hypot(b[1] - a[1]))
        })
        .collect();
    let total: f32 = segs.iter().map(|s| s.2).sum();
    for (i, p) in pts.iter_mut().enumerate() {
        let mut d = (i as f32 / n as f32) * total;
        for &(a, b, len) in &segs {
            if d <= len {
                let t = if len > 0.0 { d / len } else { 0.0 };
                *p = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
                break;
            }
            d -= len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_produces_normalised_points() {
        for kind in [
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
        ] {
            for n in [0usize, 1, 2, 3, 50, 333] {
                let pts = derive_points(kind, n, None);
                assert_eq!(pts.len(), n);
                for p in pts {
                    assert!(
                        (-0.01..=1.01).contains(&p[0]) && (-0.01..=1.01).contains(&p[1]),
                        "{kind:?} {n} {p:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn arch_goes_up_and_down() {
        let pts = derive_points(PropKind::Arch, 3, None);
        assert!((pts[0][1] - 1.0).abs() < 1e-5);
        assert!(pts[1][1].abs() < 1e-5);
        assert!((pts[2][0] - 1.0).abs() < 1e-5);
    }
}
