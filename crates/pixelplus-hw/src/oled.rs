//! SSD1306 128×64 I²C OLED (difftxlarge J22 socket, address 0x3C) and the
//! PixelPlus status screen.
//!
//! [`Canvas`] is a 1-bit framebuffer in the controller's page layout;
//! [`StatusScreen`] draws the node name, player state, current song, IP
//! address and temperature onto it; [`Ssd1306`] initialises the panel and
//! pushes the canvas.

use crate::error::Result;
use crate::font::{glyph, CELL_H, CELL_W, GLYPH_W};
use crate::i2c::I2cBus;

/// Default SSD1306 address.
pub const OLED_ADDR: u8 = 0x3C;
/// Panel width.
pub const WIDTH: usize = 128;
/// Panel height.
pub const HEIGHT: usize = 64;
/// Characters per text row at scale 1.
pub const COLUMNS: usize = WIDTH / CELL_W;

/// A 128×64 monochrome image in SSD1306 page order
/// (byte `page × 128 + x`, bit `y % 8`).
#[derive(Clone, PartialEq, Eq)]
pub struct Canvas {
    buf: [u8; WIDTH * HEIGHT / 8],
}

impl std::fmt::Debug for Canvas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Canvas")
            .field("lit_pixels", &self.buf.iter().map(|b| b.count_ones()).sum::<u32>())
            .finish()
    }
}

impl Default for Canvas {
    fn default() -> Self {
        Canvas {
            buf: [0; WIDTH * HEIGHT / 8],
        }
    }
}

impl Canvas {
    /// A blank canvas.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear to black.
    pub fn clear(&mut self) {
        self.buf.fill(0);
    }

    /// Raw bytes in controller order.
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    /// Set one pixel; coordinates outside the panel are ignored.
    pub fn set(&mut self, x: i32, y: i32, on: bool) {
        if x < 0 || y < 0 || x >= WIDTH as i32 || y >= HEIGHT as i32 {
            return;
        }
        let (x, y) = (x as usize, y as usize);
        let i = (y / 8) * WIDTH + x;
        let bit = 1 << (y % 8);
        if on {
            self.buf[i] |= bit;
        } else {
            self.buf[i] &= !bit;
        }
    }

    /// Read one pixel (false outside the panel).
    pub fn get(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x >= WIDTH as i32 || y >= HEIGHT as i32 {
            return false;
        }
        let (x, y) = (x as usize, y as usize);
        self.buf[(y / 8) * WIDTH + x] & (1 << (y % 8)) != 0
    }

    /// Fill a rectangle.
    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, on: bool) {
        for yy in y..y.saturating_add(h) {
            for xx in x..x.saturating_add(w) {
                self.set(xx, yy, on);
            }
        }
    }

    /// Draw `text` with its top-left at (`x`, `y`), each font pixel `scale`
    /// panel pixels square. `on` is the ink colour (false draws dark text on
    /// a lit background). Returns the x after the last character.
    pub fn text(&mut self, x: i32, y: i32, text: &str, scale: u8, on: bool) -> i32 {
        let s = i32::from(scale.max(1));
        let mut cx = x;
        for c in text.chars() {
            let cols = glyph(c);
            for (col, bits) in cols.iter().enumerate() {
                for row in 0..7 {
                    if bits & (1 << row) != 0 {
                        self.fill_rect(cx + col as i32 * s, y + row * s, s, s, on);
                    }
                }
            }
            cx += CELL_W as i32 * s;
        }
        cx
    }
}

/// Shorten `s` to at most `max` characters, marking the cut with `..`.
pub fn ellipsize(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    if max <= 2 {
        return s.chars().take(max).collect();
    }
    let head: String = s.chars().take(max - 2).collect();
    let mut out = head.trim_end().to_string();
    out.push_str("..");
    out
}

/// Split `s` into at most `lines` lines of `width` characters at word
/// boundaries where possible; the last line is ellipsized.
pub fn wrap(s: &str, width: usize, lines: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in s.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if candidate.chars().count() <= width {
            current = candidate;
            continue;
        }
        if !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        current = word.to_string();
        // Hard-split words longer than a line.
        while current.chars().count() > width {
            let head: String = current.chars().take(width).collect();
            current = current.chars().skip(width).collect();
            out.push(head);
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    if out.len() > lines {
        let rest = out[lines - 1..].join(" ");
        out.truncate(lines - 1);
        out.push(ellipsize(&rest, width));
    }
    out
}

/// What the status screen shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StatusScreen {
    /// Node / show name (title bar).
    pub name: String,
    /// Player state, e.g. `Playing`, `Idle`, `Testing`.
    pub state: String,
    /// Current song or sequence.
    pub song: Option<String>,
    /// Primary IP address.
    pub ip: Option<String>,
    /// Hottest relevant temperature in °C.
    pub temp_c: Option<f64>,
}

impl StatusScreen {
    /// Draw onto `canvas` (which is cleared first).
    ///
    /// ```text
    /// ┌────────────────────────┐
    /// │▓ Front Yard Leader    ▓│  title bar (inverted)
    /// │ PLAYING                │  state, double size
    /// │ Wizards in Winter -    │  song, two lines
    /// │ TSO                    │
    /// │ IP 192.168.1.20        │
    /// │ 48.3°C                 │
    /// └────────────────────────┘
    /// ```
    pub fn render(&self, canvas: &mut Canvas) {
        canvas.clear();
        canvas.fill_rect(0, 0, WIDTH as i32, CELL_H as i32 + 1, true);
        let title = if self.name.trim().is_empty() { "PixelPlus" } else { self.name.trim() };
        canvas.text(2, 1, &ellipsize(title, COLUMNS - 1), 1, false);

        let state = ellipsize(&self.state.to_uppercase(), COLUMNS / 2);
        canvas.text(2, 12, &state, 2, true);

        if let Some(song) = self.song.as_deref().filter(|s| !s.trim().is_empty()) {
            for (i, line) in wrap(song, COLUMNS, 2).iter().enumerate() {
                canvas.text(2, 30 + i as i32 * CELL_H as i32, line, 1, true);
            }
        }
        let mut bottom = String::new();
        if let Some(ip) = self.ip.as_deref().filter(|s| !s.is_empty()) {
            bottom = format!("IP {ip}");
        }
        canvas.text(2, 48, &ellipsize(&bottom, COLUMNS), 1, true);
        if let Some(t) = self.temp_c.filter(|t| t.is_finite()) {
            let temp = format!("{t:.1}°C");
            canvas.text(2, 56, &temp, 1, true);
        }
    }
}

/// SSD1306 initialisation (datasheet application note sequence for a
/// 128×64 panel with the internal charge pump).
const INIT: &[u8] = &[
    0xAE, // display off
    0xD5, 0x80, // clock divide / oscillator
    0xA8, 0x3F, // multiplex 64
    0xD3, 0x00, // display offset 0
    0x40, // start line 0
    0x8D, 0x14, // charge pump on
    0x20, 0x00, // horizontal addressing
    0xA1, // segment remap (column 127 = SEG0)
    0xC8, // COM scan descending
    0xDA, 0x12, // COM pins: alternative, no remap
    0x81, 0xCF, // contrast
    0xD9, 0xF1, // pre-charge
    0xDB, 0x40, // VCOMH deselect
    0xA4, // display follows RAM
    0xA6, // normal (not inverted)
    0x2E, // scrolling off
    0xAF, // display on
];

/// Bytes of display data per I²C transaction.
const DATA_CHUNK: usize = 128;

/// An SSD1306 on an [`I2cBus`].
pub struct Ssd1306<B: I2cBus> {
    bus: B,
    addr: u8,
}

impl<B: I2cBus> Ssd1306<B> {
    /// A panel at `addr` (usually [`OLED_ADDR`]).
    pub fn new(bus: B, addr: u8) -> Self {
        Ssd1306 { bus, addr }
    }

    /// Give the bus back.
    pub fn into_inner(self) -> B {
        self.bus
    }

    fn commands(&mut self, cmds: &[u8]) -> Result<()> {
        let mut msg = Vec::with_capacity(cmds.len() + 1);
        msg.push(0x00); // Co = 0, D/C = 0: command stream
        msg.extend_from_slice(cmds);
        self.bus.write(self.addr, &msg)
    }

    /// Initialise and switch on the panel.
    pub fn init(&mut self) -> Result<()> {
        self.commands(INIT)
    }

    /// Switch the panel on or off (contents are kept).
    pub fn set_power(&mut self, on: bool) -> Result<()> {
        self.commands(&[if on { 0xAF } else { 0xAE }])
    }

    /// Set the contrast (brightness).
    pub fn set_contrast(&mut self, contrast: u8) -> Result<()> {
        self.commands(&[0x81, contrast])
    }

    /// Push the whole canvas to the panel.
    pub fn flush(&mut self, canvas: &Canvas) -> Result<()> {
        self.commands(&[0x21, 0x00, (WIDTH - 1) as u8, 0x22, 0x00, (HEIGHT / 8 - 1) as u8])?;
        let mut msg = Vec::with_capacity(DATA_CHUNK + 1);
        for chunk in canvas.as_bytes().chunks(DATA_CHUNK) {
            msg.clear();
            msg.push(0x40); // Co = 0, D/C = 1: data stream
            msg.extend_from_slice(chunk);
            self.bus.write(self.addr, &msg)?;
        }
        Ok(())
    }
}

/// Glyph width re-exported for layout calculations.
pub const CHAR_WIDTH: usize = CELL_W;
/// Glyph height re-exported for layout calculations.
pub const CHAR_HEIGHT: usize = CELL_H;
/// Visible glyph width.
pub const GLYPH_WIDTH: usize = GLYPH_W;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i2c::{MockI2c, MockRecorder};

    #[test]
    fn canvas_pixels_and_bounds() {
        let mut c = Canvas::new();
        c.set(0, 0, true);
        c.set(127, 63, true);
        c.set(-1, 5, true);
        c.set(128, 5, true);
        c.set(5, 64, true);
        assert!(c.get(0, 0) && c.get(127, 63));
        assert_eq!(c.as_bytes()[0], 1);
        assert_eq!(c.as_bytes()[1023], 0x80);
        assert_eq!(c.as_bytes().iter().map(|b| b.count_ones()).sum::<u32>(), 2);
        c.set(0, 0, false);
        assert!(!c.get(0, 0));
    }

    #[test]
    fn text_draws_glyphs() {
        let mut c = Canvas::new();
        let end = c.text(0, 0, "I", 1, true);
        assert_eq!(end, 6);
        // 'I' = 00 41 7F 41 00: column 2 is fully lit for 7 rows.
        assert!((0..7).all(|y| c.get(2, y)));
        assert!(!c.get(2, 7));
        let mut big = Canvas::new();
        assert_eq!(big.text(0, 0, "I", 2, true), 12);
        assert!(big.get(4, 13) && big.get(5, 13));
    }

    #[test]
    fn wrapping_and_ellipsis() {
        assert_eq!(ellipsize("hello", 10), "hello");
        assert_eq!(ellipsize("hello world", 7), "hello..");
        assert_eq!(ellipsize("abc", 2), "ab");
        assert_eq!(wrap("Wizards in Winter by TSO", 12, 2), vec!["Wizards in", "Winter by.."]);
        assert_eq!(wrap("short", 12, 2), vec!["short"]);
        assert_eq!(wrap("abcdefghijklmnop", 5, 3), vec!["abcde", "fghij", "klm.."]);
        assert!(wrap("", 5, 2).is_empty());
    }

    #[test]
    fn status_screen_renders() {
        let screen = StatusScreen {
            name: "Front Yard Leader".into(),
            state: "Playing".into(),
            song: Some("Carol of the Bells (Trans-Siberian Orchestra)".into()),
            ip: Some("192.168.1.20".into()),
            temp_c: Some(48.26),
        };
        let mut c = Canvas::new();
        screen.render(&mut c);
        // Title bar is lit, with dark text inside it.
        assert!(c.get(0, 0) && c.get(127, 8));
        let lit = c.as_bytes().iter().map(|b| b.count_ones()).sum::<u32>();
        assert!(lit > 1200 && lit < 4000, "{lit}");
        // Empty screen still renders a title.
        let mut e = Canvas::new();
        StatusScreen::default().render(&mut e);
        assert!(e.get(0, 0));
    }

    #[test]
    fn driver_sequence() {
        let bus = MockI2c::new().with(OLED_ADDR, MockRecorder::default());
        let mut oled = Ssd1306::new(bus, OLED_ADDR);
        oled.init().unwrap();
        let mut c = Canvas::new();
        c.set(0, 0, true);
        oled.flush(&c).unwrap();
        oled.set_power(false).unwrap();
        let bus = oled.into_inner();
        let rec = bus.device::<MockRecorder>(OLED_ADDR).unwrap();
        assert_eq!(rec.writes[0][0], 0x00);
        assert_eq!(&rec.writes[0][1..], INIT);
        let data: Vec<u8> = rec
            .writes
            .iter()
            .filter(|w| w[0] == 0x40)
            .flat_map(|w| w[1..].to_vec())
            .collect();
        assert_eq!(data.len(), 1024);
        assert_eq!(data[0], 1);
        assert_eq!(rec.writes.last().unwrap(), &vec![0x00, 0xAE]);
        let mut none = Ssd1306::new(MockI2c::new(), OLED_ADDR);
        assert!(none.init().is_err());
    }
}
