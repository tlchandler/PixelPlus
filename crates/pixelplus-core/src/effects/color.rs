//! RGB colour type shared by effects, text and overlays.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// An 8-bit-per-channel RGB colour. Serialized as a `"#rrggbb"` string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// Error for a string that is not a colour.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("\"{0}\" is not a color; use #rrggbb")]
pub struct ParseColorError(pub String);

impl Rgb {
    pub const BLACK: Rgb = Rgb::new(0, 0, 0);
    pub const WHITE: Rgb = Rgb::new(255, 255, 255);
    /// A pleasant incandescent-like warm white.
    pub const WARM_WHITE: Rgb = Rgb::new(255, 180, 107);

    /// Construct from components.
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Rgb { r, g, b }
    }

    /// Parse `#rrggbb`, `rrggbb`, `#rgb` or `rgb` (case-insensitive).
    pub fn from_hex(s: &str) -> Option<Self> {
        let hex = s.trim();
        let hex = hex.strip_prefix('#').unwrap_or(hex);
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let nibble = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).ok();
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        match hex.len() {
            6 => Some(Rgb::new(byte(0)?, byte(2)?, byte(4)?)),
            3 => Some(Rgb::new(nibble(0)? * 17, nibble(1)? * 17, nibble(2)? * 17)),
            _ => None,
        }
    }

    /// Lower-case `#rrggbb`.
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// `[r, g, b]`.
    pub const fn to_array(self) -> [u8; 3] {
        [self.r, self.g, self.b]
    }

    /// Multiply every channel by `f` (clamped to 0..=1).
    pub fn scale(self, f: f32) -> Self {
        let f = if f.is_finite() {
            f.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let s = |c: u8| (f32::from(c) * f + 0.5) as u8;
        Rgb::new(s(self.r), s(self.g), s(self.b))
    }

    /// Linear blend from `self` (t = 0) to `other` (t = 1).
    pub fn lerp(self, other: Rgb, t: f32) -> Self {
        let t = if t.is_finite() {
            t.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let l = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t + 0.5) as u8;
        Rgb::new(l(self.r, other.r), l(self.g, other.g), l(self.b, other.b))
    }

    /// Per-channel maximum (additive-looking blend without overflow).
    pub fn max(self, other: Rgb) -> Self {
        Rgb::new(
            self.r.max(other.r),
            self.g.max(other.g),
            self.b.max(other.b),
        )
    }

    /// Colour from hue (0..1, wraps), saturation and value (0..1).
    pub fn from_hsv(h: f32, s: f32, v: f32) -> Self {
        let h = if h.is_finite() {
            h.rem_euclid(1.0)
        } else {
            0.0
        } * 6.0;
        let s = if s.is_finite() {
            s.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let v = if v.is_finite() {
            v.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let i = (h.floor() as i32).rem_euclid(6);
        let f = h - h.floor();
        let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
        let (r, g, b) = match i {
            0 => (v, t, p),
            1 => (q, v, p),
            2 => (p, v, t),
            3 => (p, q, v),
            4 => (t, p, v),
            _ => (v, p, q),
        };
        let c = |x: f32| (x * 255.0 + 0.5) as u8;
        Rgb::new(c(r), c(g), c(b))
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for Rgb {
    type Err = ParseColorError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Rgb::from_hex(s).ok_or_else(|| ParseColorError(s.to_string()))
    }
}

impl TryFrom<String> for Rgb {
    type Error = ParseColorError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<Rgb> for String {
    fn from(c: Rgb) -> String {
        c.to_hex()
    }
}

impl From<[u8; 3]> for Rgb {
    fn from(a: [u8; 3]) -> Self {
        Rgb::new(a[0], a[1], a[2])
    }
}

impl From<Rgb> for [u8; 3] {
    fn from(c: Rgb) -> Self {
        c.to_array()
    }
}

/// Sample a palette as a smooth cyclic gradient at `pos` (wraps at 1.0):
/// colour *i* sits at `i / len`, blending towards the next and back to the
/// first. An empty palette yields black.
pub fn palette_cyclic(colors: &[Rgb], pos: f32) -> Rgb {
    match colors.len() {
        0 => Rgb::BLACK,
        1 => colors[0],
        n => {
            let pos = if pos.is_finite() {
                pos.rem_euclid(1.0)
            } else {
                0.0
            } * n as f32;
            let i = (pos.floor() as usize).min(n - 1);
            colors[i].lerp(colors[(i + 1) % n], pos - i as f32)
        }
    }
}

/// Sample a palette as a linear gradient at `pos` (clamped to 0..1): first
/// colour at 0, last at 1.
pub fn palette_linear(colors: &[Rgb], pos: f32) -> Rgb {
    match colors.len() {
        0 => Rgb::BLACK,
        1 => colors[0],
        n => {
            let pos = if pos.is_finite() {
                pos.clamp(0.0, 1.0)
            } else {
                0.0
            } * (n - 1) as f32;
            let i = (pos.floor() as usize).min(n - 2);
            colors[i].lerp(colors[i + 1], pos - i as f32)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_forms() {
        assert_eq!(Rgb::from_hex("#ff8000"), Some(Rgb::new(255, 128, 0)));
        assert_eq!(Rgb::from_hex("FF8000"), Some(Rgb::new(255, 128, 0)));
        assert_eq!(Rgb::from_hex("#f80"), Some(Rgb::new(255, 136, 0)));
        for bad in ["", "#", "#12345", "#gggggg", "red", "#ff80001", "#+1+2+3"] {
            assert_eq!(Rgb::from_hex(bad), None, "{bad}");
        }
        assert_eq!(Rgb::new(1, 2, 255).to_hex(), "#0102ff");
    }

    #[test]
    fn serde_as_hex_string() {
        let c: Rgb = serde_json::from_str("\"#00ff00\"").unwrap();
        assert_eq!(c, Rgb::new(0, 255, 0));
        assert_eq!(serde_json::to_string(&c).unwrap(), "\"#00ff00\"");
        assert!(serde_json::from_str::<Rgb>("\"nope\"").is_err());
    }

    #[test]
    fn hsv_primaries() {
        assert_eq!(Rgb::from_hsv(0.0, 1.0, 1.0), Rgb::new(255, 0, 0));
        assert_eq!(Rgb::from_hsv(1.0 / 3.0, 1.0, 1.0), Rgb::new(0, 255, 0));
        assert_eq!(Rgb::from_hsv(2.0 / 3.0, 1.0, 1.0), Rgb::new(0, 0, 255));
        assert_eq!(Rgb::from_hsv(1.0, 1.0, 1.0), Rgb::new(255, 0, 0));
        assert_eq!(Rgb::from_hsv(f32::NAN, 0.0, 1.0), Rgb::WHITE);
    }

    #[test]
    fn palettes() {
        let p = [Rgb::BLACK, Rgb::WHITE];
        assert_eq!(palette_cyclic(&p, 0.0), Rgb::BLACK);
        assert_eq!(palette_cyclic(&p, 0.5), Rgb::WHITE);
        assert_eq!(palette_cyclic(&p, 0.25), Rgb::new(128, 128, 128));
        assert_eq!(palette_linear(&p, 1.0), Rgb::WHITE);
        assert_eq!(palette_linear(&p, 7.0), Rgb::WHITE);
        assert_eq!(palette_linear(&[], 0.3), Rgb::BLACK);
        assert_eq!(Rgb::WHITE.scale(0.5), Rgb::new(128, 128, 128));
        assert_eq!(Rgb::WHITE.scale(f32::NAN), Rgb::BLACK);
    }
}
