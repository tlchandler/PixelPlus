//! Pixel fonts, scrolling text, QR codes and matrix mapping.

use pixelplus_core::effects::Rgb;
use pixelplus_core::model::MatrixInfo;
use pixelplus_core::text::{
    best_scale, blit_grid_to_prop, draw_text, grid_to_prop_pixels, marquee_offset,
    prop_pixels_to_grid, render_lines, render_qr, render_text, render_text_fit, render_text_with,
    text_width, Font, QrError, QrStyle, RgbGrid, MAX_GRID_DIM,
};

const RED: Rgb = Rgb::new(255, 0, 0);

/// ASCII art of a grid: `#` for lit, `.` for black.
fn art(g: &RgbGrid) -> Vec<String> {
    (0..g.height())
        .map(|y| {
            (0..g.width())
                .map(|x| {
                    if g.get(i64::from(x), i64::from(y)) == Some(Rgb::BLACK) {
                        '.'
                    } else {
                        '#'
                    }
                })
                .collect()
        })
        .collect()
}

#[test]
fn small_font_matches_mario_port() {
    let g = render_text_with("A1", Font::Small, 1, RED, 7, 5, 0);
    assert_eq!(
        art(&g),
        vec![".#...#.", "#.#.##.", "###..#.", "#.#..#.", "#.#.###"]
    );
    // Lower case renders as capitals in the small font.
    assert_eq!(
        render_text_with("a", Font::Small, 1, RED, 3, 5, 0),
        render_text_with("A", Font::Small, 1, RED, 3, 5, 0)
    );
}

#[test]
fn widths() {
    assert_eq!(text_width("", Font::Small, 1), 0);
    assert_eq!(text_width("HI", Font::Small, 1), 7);
    assert_eq!(text_width("HI", Font::Small, 2), 14);
    // Proportional: "i" is narrower than "M" in the 5×7 font.
    assert!(text_width("i", Font::Medium, 1) < text_width("M", Font::Medium, 1));
    assert_eq!(text_width("M", Font::Medium, 1), 5);
    assert_eq!(text_width(" ", Font::Medium, 1), 3);
    // Control characters are ignored.
    assert_eq!(text_width("H\nI", Font::Small, 1), 7);
}

#[test]
fn medium_font_glyph() {
    let g = render_text_with("T", Font::Medium, 1, RED, 5, 7, 0);
    assert_eq!(
        art(&g),
        vec!["#####", "..#..", "..#..", "..#..", "..#..", "..#..", "..#.."]
    );
}

#[test]
fn unknown_characters_draw_question_mark() {
    let q = render_text_with("?", Font::Medium, 1, RED, 6, 7, 0);
    let x = render_text_with("\u{4E2D}", Font::Medium, 1, RED, 6, 7, 0);
    assert_eq!(q, x);
    assert!(Font::Medium.supports('\u{2019}'));
    assert!(Font::Medium.supports('♥'));
    assert!(!Font::Small.supports('♥'));
}

#[test]
fn render_text_picks_font_and_centres_vertically() {
    let g = render_text("HI", RED, 20, 9, 0);
    // 7-pixel font centred in 9 rows: first and last rows empty.
    assert!(art(&g)[0].chars().all(|c| c == '.'));
    assert!(art(&g)[8].chars().all(|c| c == '.'));
    assert!(art(&g)[1].starts_with('#'));
}

#[test]
fn scrolling_shifts_left() {
    let a = render_text("HELLO", RED, 10, 5, 0);
    let b = render_text("HELLO", RED, 10, 5, 1);
    for y in 0..5 {
        for x in 0..9 {
            assert_eq!(a.get(x + 1, y), b.get(x, y));
        }
    }
    // Negative offsets move text right; off-grid text is fine.
    let c = render_text("HELLO", RED, 10, 5, -8);
    assert_eq!(art(&c)[0].find('#'), Some(8));
    let gone = render_text("HELLO", RED, 10, 5, 1000);
    assert!(gone.as_bytes().iter().all(|&b| b == 0));
    let _ = render_text("HELLO", RED, 10, 5, i64::MIN);
    let _ = render_text("HELLO", RED, 10, 5, i64::MAX);
}

#[test]
fn marquee_cycle() {
    // Text 20 px wide on a 10 px grid at 10 px/s: enters at -10, loops every 3 s.
    assert_eq!(marquee_offset(0, 10.0, 20, 10), -10);
    assert_eq!(marquee_offset(1000, 10.0, 20, 10), 0);
    assert_eq!(marquee_offset(2900, 10.0, 20, 10), 19);
    assert_eq!(marquee_offset(3000, 10.0, 20, 10), -10);
    assert_eq!(marquee_offset(5000, f32::NAN, 20, 10), -10);
    assert_eq!(marquee_offset(5000, 10.0, 0, 0), 0);
}

#[test]
fn fit_and_lines() {
    assert_eq!(best_scale("HI", Font::Small, 32, 16), 3);
    let g = render_text_fit("HI", Font::Small, RED, 32, 16);
    let rows = art(&g);
    let lit_rows: Vec<_> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.contains('#'))
        .collect();
    assert_eq!(lit_rows.len(), 15);
    assert_eq!(lit_rows[0].0, 0);

    let g = render_lines(&[("GAME", RED), ("ON", Rgb::WHITE)], Font::Small, 20, 16);
    let rows = art(&g);
    assert!(rows.iter().any(|r| r.contains('#')));
    let empty = render_lines(&[], Font::Small, 4, 4);
    assert!(empty.as_bytes().iter().all(|&b| b == 0));
}

#[test]
fn draw_text_clips_everywhere() {
    let mut g = RgbGrid::new(4, 4);
    draw_text(&mut g, "WWWW", Font::Medium, 3, -5, -5, RED);
    draw_text(&mut g, "WWWW", Font::Medium, u32::MAX, 0, 0, RED);
    assert_eq!(g.width(), 4);
}

#[test]
fn grid_dimensions_are_clamped() {
    let g = RgbGrid::new(u32::MAX, 1);
    assert_eq!(g.width(), MAX_GRID_DIM);
    assert!(RgbGrid::from_bytes(2, 2, vec![0; 11]).is_none());
    assert!(RgbGrid::from_bytes(2, 2, vec![0; 12]).is_some());
}

#[test]
fn qr_code_fits_and_has_finder_patterns() {
    let style = QrStyle::default();
    let g = render_qr("http://pixelplus.local/request", 64, 48, style).unwrap();
    assert_eq!((g.width(), g.height()), (64, 48));
    // Find the top-left finder pattern: the first dark pixel scanning rows.
    let dark = |x: i64, y: i64| g.get(x, y) == Some(style.dark);
    let (fx, fy) = (0..48i64)
        .flat_map(|y| (0..64i64).map(move |x| (x, y)))
        .find(|&(x, y)| dark(x, y))
        .unwrap();
    // A 7-module finder: dark border row followed by the light ring.
    let mut run = 0;
    while dark(fx + run, fy) {
        run += 1;
    }
    assert!(run >= 7 && run % 7 == 0, "finder width {run}");
    let module = run / 7;
    assert!(!dark(fx + module, fy + module), "light ring");
    assert!(dark(fx + 3 * module, fy + 3 * module), "dark centre");
    // A quiet zone of at least one module is light.
    assert!(fx >= module && fy >= module);
    assert!((0..48).all(|y| g.get(0, y) == Some(style.light)));
}

#[test]
fn qr_errors() {
    assert!(matches!(
        render_qr("http://pixelplus.local/request", 10, 10, QrStyle::default()),
        Err(QrError::TooSmall { .. })
    ));
    let huge = "x".repeat(8000);
    assert_eq!(
        render_qr(&huge, 500, 500, QrStyle::default()),
        Err(QrError::TooLong)
    );
    // Tiny but possible: version 1 with no quiet zone.
    assert!(render_qr("HI", 21, 21, QrStyle::default()).is_ok());
}

fn serpentine(w: u32, h: u32) -> MatrixInfo {
    // Columns wired bottom-to-top, snaking; one missing cell.
    let mut map = vec![-1i32; (w * h) as usize];
    let mut idx = 0;
    for x in 0..w {
        for k in 0..h {
            let y = if x % 2 == 0 { h - 1 - k } else { k };
            if (x, y) == (w - 1, 0) {
                continue;
            }
            map[(y * w + x) as usize] = idx;
            idx += 1;
        }
    }
    MatrixInfo {
        width: w,
        height: h,
        pixel_map: map,
    }
}

#[test]
fn grid_maps_to_prop_pixels_and_back() {
    let m = serpentine(4, 3);
    let mut g = RgbGrid::new(4, 3);
    g.set(0, 2, RED); // bottom-left = pixel 0
    g.set(1, 0, Rgb::WHITE); // column 1 runs top-down: pixel 3
    let px = grid_to_prop_pixels(&g, &m, 11);
    assert_eq!(px.len(), 33);
    assert_eq!(&px[0..3], &[255, 0, 0]);
    assert_eq!(&px[9..12], &[255, 255, 255]);
    assert_eq!(prop_pixels_to_grid(&px, &m), g);

    // Blitting leaves unmapped pixels alone and ignores out-of-range indices.
    let mut out = vec![7u8; 33];
    let mut bad = m.clone();
    bad.pixel_map[5] = 10_000;
    bad.pixel_map.truncate(8);
    blit_grid_to_prop(&RgbGrid::filled(4, 3, RED), &bad, &mut out);
    assert!(out.chunks(3).any(|c| c == [7, 7, 7]));

    // Grid smaller than the matrix: only the overlap is used.
    let px = grid_to_prop_pixels(&RgbGrid::filled(1, 1, RED), &m, 11);
    assert_eq!(px.iter().filter(|&&b| b == 255).count(), 1);
}
