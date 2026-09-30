//! Terminal styling. Output goes through `anstream`, which strips colours
//! automatically when stdout is not a terminal or `NO_COLOR` is set.

use anstyle::{AnsiColor, Color, Style};

/// Section headings.
pub const HEADING: Style = Style::new().bold();
/// Field names in key/value listings.
pub const KEY: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));
/// Secondary information.
pub const DIM: Style = Style::new().dimmed();
/// Success.
pub const OK: Style = Style::new()
    .fg_color(Some(Color::Ansi(AnsiColor::Green)))
    .bold();
/// Warnings.
pub const WARN: Style = Style::new()
    .fg_color(Some(Color::Ansi(AnsiColor::Yellow)))
    .bold();
/// Failures.
pub const FAIL: Style = Style::new()
    .fg_color(Some(Color::Ansi(AnsiColor::Red)))
    .bold();

/// Wrap `text` in `style`.
pub fn paint(style: Style, text: impl std::fmt::Display) -> String {
    format!("{style}{text}{style:#}")
}

/// A health level shared by `detect`, `doctor` and sensors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Fine.
    Ok,
    /// Needs attention.
    Warn,
    /// Broken.
    Fail,
}

impl Level {
    /// A coloured status badge (`✓`, `!`, `✗`).
    pub fn badge(self) -> String {
        match self {
            Level::Ok => paint(OK, "✓"),
            Level::Warn => paint(WARN, "!"),
            Level::Fail => paint(FAIL, "✗"),
        }
    }
}

/// Print a `key  value` line with the key padded to `width`.
pub fn kv(key: &str, value: impl std::fmt::Display, width: usize) {
    anstream::println!("  {}  {value}", paint(KEY, format!("{key:<width$}")));
}

/// Print a heading.
pub fn heading(text: &str) {
    anstream::println!("{}", paint(HEADING, text));
}
