//! Text, QR codes and pixel grids for matrix props.
//!
//! Overlays (scrolling messages, QR codes for the song-request page, game
//! invites) draw into an [`RgbGrid`], a row-major RGB image with the origin at
//! the top-left, the same layout as the overlay shared-memory buffer
//! (ARCHITECTURE §10). [`grid_to_prop_pixels`] then maps it onto a matrix
//! prop's pixels through its [`MatrixInfo::pixel_map`].
//!
//! Two bitmap fonts are included: [`Font::Small`] (3×5, capitals only, ported
//! from fpp-mariobros) for tiny matrices and [`Font::Medium`] (5×7, full ASCII,
//! proportionally spaced).
//!
//! ```
//! use pixelplus_core::effects::Rgb;
//! use pixelplus_core::text::{render_text, text_width, Font};
//!
//! let grid = render_text("HI", Rgb::WHITE, 16, 5, 0);
//! assert_eq!(text_width("HI", Font::Small, 1), 7);
//! assert_eq!(grid.get(0, 0), Some(Rgb::WHITE)); // top-left of the H
//! ```

mod font3x5;
mod font5x7;

pub use crate::effects::Rgb;
use crate::model::MatrixInfo;
use qrcode::{EcLevel, QrCode};
use serde::{Deserialize, Serialize};

/// Largest grid side length accepted; larger requests are clamped so bad
/// input can never allocate unbounded memory.
pub const MAX_GRID_DIM: u32 = 2048;

// ---------------------------------------------------------------------------
// Grid
// ---------------------------------------------------------------------------

/// A row-major RGB image, origin top-left, 3 bytes per pixel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbGrid {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl RgbGrid {
    /// A black grid. Dimensions are clamped to [`MAX_GRID_DIM`].
    pub fn new(width: u32, height: u32) -> Self {
        Self::filled(width, height, Rgb::BLACK)
    }

    /// A grid filled with `color`. Dimensions are clamped to [`MAX_GRID_DIM`].
    pub fn filled(width: u32, height: u32, color: Rgb) -> Self {
        let (width, height) = (width.min(MAX_GRID_DIM), height.min(MAX_GRID_DIM));
        let n = width as usize * height as usize;
        let mut data = Vec::with_capacity(n * 3);
        for _ in 0..n {
            data.extend_from_slice(&color.to_array());
        }
        RgbGrid {
            width,
            height,
            data,
        }
    }

    /// Wrap existing row-major RGB bytes. Returns `None` if the length does
    /// not match `width * height * 3` or a dimension exceeds [`MAX_GRID_DIM`].
    pub fn from_bytes(width: u32, height: u32, data: Vec<u8>) -> Option<Self> {
        (width <= MAX_GRID_DIM
            && height <= MAX_GRID_DIM
            && data.len() == width as usize * height as usize * 3)
            .then_some(RgbGrid {
                width,
                height,
                data,
            })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Raw row-major RGB bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Consume into raw row-major RGB bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.data
    }

    fn offset(&self, x: i64, y: i64) -> Option<usize> {
        (x >= 0 && y >= 0 && x < i64::from(self.width) && y < i64::from(self.height))
            .then(|| (y as usize * self.width as usize + x as usize) * 3)
    }

    /// Colour at `(x, y)`, or `None` outside the grid.
    pub fn get(&self, x: i64, y: i64) -> Option<Rgb> {
        self.offset(x, y)
            .map(|o| Rgb::new(self.data[o], self.data[o + 1], self.data[o + 2]))
    }

    /// Set `(x, y)`; coordinates outside the grid are ignored.
    pub fn set(&mut self, x: i64, y: i64, color: Rgb) {
        if let Some(o) = self.offset(x, y) {
            self.data[o..o + 3].copy_from_slice(&color.to_array());
        }
    }

    /// Fill a rectangle, clipped to the grid.
    pub fn fill_rect(&mut self, x: i64, y: i64, w: u32, h: u32, color: Rgb) {
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + i64::from(w)).min(i64::from(self.width));
        let y1 = (y + i64::from(h)).min(i64::from(self.height));
        for yy in y0..y1 {
            for xx in x0..x1 {
                self.set(xx, yy, color);
            }
        }
    }

    /// Fill the whole grid.
    pub fn fill(&mut self, color: Rgb) {
        for px in self.data.chunks_exact_mut(3) {
            px.copy_from_slice(&color.to_array());
        }
    }
}

// ---------------------------------------------------------------------------
// Fonts
// ---------------------------------------------------------------------------

/// Built-in bitmap fonts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Font {
    /// 3×5 capitals, fixed width (fits matrices only 5 pixels tall).
    #[default]
    Small,
    /// 5×7 full ASCII, proportionally spaced.
    Medium,
}

/// A single glyph: `rows[y]` has bit `x` set when column `x` is lit.
#[derive(Debug, Clone, Copy)]
struct Glyph {
    rows: [u8; 7],
    /// First drawn column.
    left: u8,
    /// Number of columns advanced over (excluding letter spacing).
    width: u8,
}

impl Glyph {
    fn lit(&self, x: u32, y: u32) -> bool {
        y < 7 && x < 8 && self.rows[y as usize] & (1 << (x + u32::from(self.left))) != 0
    }
}

impl Font {
    /// Glyph height in pixels.
    pub fn height(self) -> u32 {
        match self {
            Font::Small => 5,
            Font::Medium => 7,
        }
    }

    /// The largest font whose glyphs fit in `height` pixels (Small for
    /// anything below 7).
    pub fn for_height(height: u32) -> Font {
        if height >= Font::Medium.height() {
            Font::Medium
        } else {
            Font::Small
        }
    }

    /// Whether this font can draw `c` (other characters render as `?`).
    pub fn supports(self, c: char) -> bool {
        self.raw_glyph(normalize_char(c)).is_some()
    }

    fn raw_glyph(self, c: char) -> Option<Glyph> {
        fn parse<const N: usize>(rows: &[&str; N]) -> [u8; 7] {
            let mut out = [0u8; 7];
            for (y, row) in rows.iter().enumerate().take(7) {
                for (x, ch) in row.bytes().enumerate().take(8) {
                    if ch == b'#' {
                        out[y] |= 1 << x;
                    }
                }
            }
            out
        }
        match self {
            Font::Small => {
                let c = c.to_uppercase().next().unwrap_or(c);
                font3x5::GLYPHS
                    .iter()
                    .find(|(g, _)| *g == c)
                    .map(|(_, rows)| Glyph {
                        rows: parse(rows),
                        left: 0,
                        width: 3,
                    })
            }
            Font::Medium => font5x7::GLYPHS
                .iter()
                .find(|(g, _)| *g == c)
                .map(|(_, rows)| {
                    let rows = parse(rows);
                    let mask = rows.iter().fold(0u8, |a, r| a | r);
                    if mask == 0 {
                        // Space: fixed advance.
                        Glyph {
                            rows,
                            left: 0,
                            width: 3,
                        }
                    } else {
                        let left = mask.trailing_zeros() as u8;
                        let right = 7 - mask.leading_zeros() as u8;
                        Glyph {
                            rows,
                            left,
                            width: right - left + 1,
                        }
                    }
                }),
        }
    }

    fn glyph(self, c: char) -> Glyph {
        self.raw_glyph(normalize_char(c))
            .or_else(|| self.raw_glyph('?'))
            .unwrap_or(Glyph {
                rows: [0; 7],
                left: 0,
                width: 3,
            })
    }
}

/// Map typographic characters to ASCII look-alikes.
fn normalize_char(c: char) -> char {
    match c {
        '\u{2018}' | '\u{2019}' | '\u{201B}' | '\u{2032}' => '\'',
        '\u{201C}' | '\u{201D}' | '\u{2033}' => '"',
        '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
        '\u{2026}' => '.',
        '\u{00A0}' | '\t' => ' ',
        '\u{2764}' | '\u{2665}' => '♥',
        '\u{2B50}' | '\u{2605}' => '★',
        '\u{266A}' | '\u{266B}' => '♪',
        _ => c,
    }
}

/// Width in pixels of `text` drawn with `font` at integer `scale` (≥ 1),
/// including 1-pixel (× scale) spacing between characters. Newlines and other
/// control characters are ignored.
pub fn text_width(text: &str, font: Font, scale: u32) -> u32 {
    let scale = scale.max(1);
    let mut width = 0u32;
    let mut count = 0u32;
    for c in text.chars().filter(|c| !c.is_control()) {
        width = width.saturating_add(u32::from(font.glyph(c).width));
        count += 1;
    }
    if count == 0 {
        return 0;
    }
    width
        .saturating_add(count - 1)
        .saturating_mul(scale)
}

/// Draw `text` into `grid` with its top-left at `(x, y)`, clipped to the grid.
/// Returns the drawn width (same as [`text_width`]).
pub fn draw_text(
    grid: &mut RgbGrid,
    text: &str,
    font: Font,
    scale: u32,
    x: i64,
    y: i64,
    color: Rgb,
) -> u32 {
    let scale = scale.max(1).min(MAX_GRID_DIM);
    let s = i64::from(scale);
    let mut pen = x;
    let grid_w = i64::from(grid.width());
    for c in text.chars().filter(|c| !c.is_control()) {
        let g = font.glyph(c);
        let advance = i64::from(g.width) * s;
        if pen >= grid_w {
            break;
        }
        if pen + advance > 0 {
            for gy in 0..font.height() {
                for gx in 0..u32::from(g.width) {
                    if g.lit(gx, gy) {
                        grid.fill_rect(
                            pen + i64::from(gx) * s,
                            y + i64::from(gy) * s,
                            scale,
                            scale,
                            color,
                        );
                    }
                }
            }
        }
        pen += advance + s;
    }
    text_width(text, font, scale)
}

/// Render one line of text into a new `width`×`height` grid, choosing the
/// largest font that fits the height, vertically centred, with its left edge
/// at `x = -scroll_offset` (increase `scroll_offset` to scroll left; see
/// [`marquee_offset`]).
pub fn render_text(text: &str, color: Rgb, width: u32, height: u32, scroll_offset: i64) -> RgbGrid {
    render_text_with(
        text,
        Font::for_height(height),
        1,
        color,
        width,
        height,
        scroll_offset,
    )
}

/// Like [`render_text`] with an explicit font and scale.
pub fn render_text_with(
    text: &str,
    font: Font,
    scale: u32,
    color: Rgb,
    width: u32,
    height: u32,
    scroll_offset: i64,
) -> RgbGrid {
    let mut grid = RgbGrid::new(width, height);
    let scale = scale.max(1);
    let text_h = i64::from(font.height()) * i64::from(scale);
    let y = (i64::from(grid.height()) - text_h) / 2;
    draw_text(
        &mut grid,
        text,
        font,
        scale,
        scroll_offset.saturating_neg(),
        y,
        color,
    );
    grid
}

/// Largest integer scale (≥ 1) at which `text` fits in `max_w`×`max_h`.
pub fn best_scale(text: &str, font: Font, max_w: u32, max_h: u32) -> u32 {
    let mut s = 1;
    while s < MAX_GRID_DIM
        && text_width(text, font, s + 1) <= max_w
        && font.height() * (s + 1) <= max_h
    {
        s += 1;
    }
    s
}

/// Render `text` centred in a new grid at the largest scale that fits. Text
/// wider than the grid is centred and clipped on both sides.
pub fn render_text_fit(text: &str, font: Font, color: Rgb, width: u32, height: u32) -> RgbGrid {
    let mut grid = RgbGrid::new(width, height);
    let scale = best_scale(text, font, width, height);
    let w = i64::from(text_width(text, font, scale));
    let h = i64::from(font.height() * scale);
    let x = (i64::from(grid.width()) - w) / 2;
    let y = (i64::from(grid.height()) - h) / 2;
    draw_text(&mut grid, text, font, scale, x, y, color);
    grid
}

/// Render several `(text, colour)` lines as a centred block, all at the
/// largest common scale that fits (gap between lines = one scaled pixel).
pub fn render_lines(lines: &[(&str, Rgb)], font: Font, width: u32, height: u32) -> RgbGrid {
    let mut grid = RgbGrid::new(width, height);
    if lines.is_empty() {
        return grid;
    }
    let n = lines.len() as u64;
    let block_h = |s: u32| n * u64::from(font.height() * s) + (n - 1) * u64::from(s);
    let fits =
        |s: u32| block_h(s) <= u64::from(height) && lines.iter().all(|(t, _)| text_width(t, font, s) <= width);
    let mut scale = 1;
    while scale < MAX_GRID_DIM && fits(scale + 1) {
        scale += 1;
    }
    let mut y = (i64::from(grid.height()) - block_h(scale) as i64) / 2;
    for (text, color) in lines {
        let w = i64::from(text_width(text, font, scale));
        let x = (i64::from(grid.width()) - w) / 2;
        draw_text(&mut grid, text, font, scale, x, y, *color);
        y += i64::from(font.height() * scale + scale);
    }
    grid
}

/// Scroll offset for a looping marquee at time `t_ms`, moving
/// `px_per_second` pixels per second: the text enters from the right edge,
/// crosses the grid and leaves on the left, then repeats.
pub fn marquee_offset(t_ms: u64, px_per_second: f32, text_width: u32, grid_width: u32) -> i64 {
    let cycle = i64::from(text_width) + i64::from(grid_width);
    if cycle == 0 || !px_per_second.is_finite() || px_per_second <= 0.0 {
        return -i64::from(grid_width);
    }
    let travelled = (t_ms as f64 * f64::from(px_per_second) / 1000.0) as i64;
    travelled.rem_euclid(cycle) - i64::from(grid_width)
}

// ---------------------------------------------------------------------------
// QR codes
// ---------------------------------------------------------------------------

/// Colours for a QR code on LEDs. Scanners expect dark modules on a light
/// background, so the light colour is lit and the dark colour is usually off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QrStyle {
    pub dark: Rgb,
    pub light: Rgb,
}

impl Default for QrStyle {
    /// Off modules on a half-brightness white background (bright enough for
    /// phone cameras at night without blooming).
    fn default() -> Self {
        QrStyle {
            dark: Rgb::BLACK,
            light: Rgb::new(128, 128, 128),
        }
    }
}

/// Why a QR code could not be drawn.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QrError {
    /// The data does not fit in any QR code.
    #[error("the text is too long for a QR code")]
    TooLong,
    /// The matrix is too small for even the smallest code for this data.
    #[error(
        "a QR code for this text needs at least {needed}×{needed} pixels, but the matrix is {width}×{height}; use a shorter link"
    )]
    TooSmall { needed: u32, width: u32, height: u32 },
}

/// Render `data` as a QR code centred in a `width`×`height` grid, as large as
/// possible (integer module size). The whole grid is filled with the light
/// colour so the code has the widest possible quiet zone.
///
/// Among the error-correction levels that give the largest module size, the
/// most robust is chosen; a quiet zone of at least one module is kept when the
/// matrix allows it.
pub fn render_qr(data: &str, width: u32, height: u32, style: QrStyle) -> Result<RgbGrid, QrError> {
    let (width, height) = (width.min(MAX_GRID_DIM), height.min(MAX_GRID_DIM));
    let side = width.min(height);
    struct Best {
        key: (u32, u32, u8, u32),
        code: QrCode,
        scale: u32,
        quiet: u32,
    }
    let mut best: Option<Best> = None;
    let mut smallest_modules: Option<u32> = None;
    for (rank, ec) in [EcLevel::H, EcLevel::Q, EcLevel::M, EcLevel::L]
        .into_iter()
        .enumerate()
    {
        let Ok(code) = QrCode::with_error_correction_level(data.as_bytes(), ec) else {
            continue;
        };
        let modules = code.width() as u32;
        smallest_modules = Some(smallest_modules.map_or(modules, |m| m.min(modules)));
        for quiet in 0..=4u32 {
            let scale = side / (modules + 2 * quiet);
            if scale == 0 {
                continue;
            }
            // Prefer: a quiet zone at all, then bigger modules, then a quiet
            // zone of 2, then stronger error correction, then more quiet zone.
            let key = (
                u32::from(quiet >= 1) * 1000 + scale,
                quiet.min(2),
                3 - rank as u8,
                quiet,
            );
            let better = match &best {
                None => true,
                Some(b) => b.key < key,
            };
            if better {
                best = Some(Best {
                    key,
                    code: code.clone(),
                    scale,
                    quiet,
                });
            }
        }
    }
    let Some(best) = best else {
        return Err(match smallest_modules {
            None => QrError::TooLong,
            Some(needed) => QrError::TooSmall {
                needed,
                width,
                height,
            },
        });
    };

    let mut grid = RgbGrid::filled(width, height, style.light);
    let modules = best.code.width() as u32;
    let size = (modules + 2 * best.quiet) * best.scale;
    let x0 = i64::from((width - size) / 2 + best.quiet * best.scale);
    let y0 = i64::from((height - size) / 2 + best.quiet * best.scale);
    let colors = best.code.to_colors();
    let s = i64::from(best.scale);
    for (i, c) in colors.iter().enumerate() {
        if *c == qrcode::Color::Dark {
            let (mx, my) = ((i as u32 % modules) as i64, (i as u32 / modules) as i64);
            grid.fill_rect(x0 + mx * s, y0 + my * s, best.scale, best.scale, style.dark);
        }
    }
    Ok(grid)
}

// ---------------------------------------------------------------------------
// Matrix mapping
// ---------------------------------------------------------------------------

/// Map a grid onto a matrix prop: returns `pixel_count × 3` bytes in prop
/// pixel order. Prop pixels not covered by the pixel map are black. The grid
/// is not scaled; only the area it shares with the matrix is used.
pub fn grid_to_prop_pixels(grid: &RgbGrid, matrix: &MatrixInfo, pixel_count: usize) -> Vec<u8> {
    let mut out = vec![0u8; pixel_count.saturating_mul(3)];
    blit_grid_to_prop(grid, matrix, &mut out);
    out
}

/// Like [`grid_to_prop_pixels`] but writes into an existing prop buffer
/// (e.g. to composite an overlay over a sequence frame). Only mapped pixels
/// are written; indices beyond `out` are ignored.
pub fn blit_grid_to_prop(grid: &RgbGrid, matrix: &MatrixInfo, out: &mut [u8]) {
    let w = matrix.width.min(grid.width());
    let h = matrix.height.min(grid.height());
    for y in 0..h {
        for x in 0..w {
            let map_i = y as usize * matrix.width as usize + x as usize;
            let Some(&idx) = matrix.pixel_map.get(map_i) else {
                continue;
            };
            let Ok(idx) = usize::try_from(idx) else {
                continue;
            };
            let o = idx.saturating_mul(3);
            if let (Some(dst), Some(c)) = (
                out.get_mut(o..o.saturating_add(3)),
                grid.get(i64::from(x), i64::from(y)),
            ) {
                dst.copy_from_slice(&c.to_array());
            }
        }
    }
}

/// Inverse of [`grid_to_prop_pixels`]: build a `width × height` grid from prop
/// pixels (useful for previews). Unmapped cells are black.
pub fn prop_pixels_to_grid(pixels: &[u8], matrix: &MatrixInfo) -> RgbGrid {
    let mut grid = RgbGrid::new(matrix.width, matrix.height);
    for y in 0..grid.height() {
        for x in 0..grid.width() {
            let map_i = y as usize * matrix.width as usize + x as usize;
            let idx = matrix
                .pixel_map
                .get(map_i)
                .and_then(|&i| usize::try_from(i).ok());
            if let Some(px) = idx.and_then(|i| pixels.get(i * 3..i * 3 + 3)) {
                grid.set(
                    i64::from(x),
                    i64::from(y),
                    Rgb::new(px[0], px[1], px[2]),
                );
            }
        }
    }
    grid
}
