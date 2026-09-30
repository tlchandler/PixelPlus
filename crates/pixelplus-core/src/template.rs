//! DJ script placeholders.
//!
//! DJ clips may contain placeholders such as `{daysUntilChristmas}` (see
//! [`DJ_PLACEHOLDERS`]) that are filled in at show time and then spoken by a
//! text-to-speech voice. Everything here is formatted for the *ear*: times read
//! "seven thirty PM", dates "Thursday, December 24th", and numbers can
//! optionally be spelled out ("twelve").
//!
//! ```
//! use chrono::TimeZone;
//! use pixelplus_core::template::{render_template, TemplateContext};
//!
//! let now = chrono_tz::America::Chicago.with_ymd_and_hms(2026, 12, 13, 19, 30, 0).unwrap();
//! let ctx = TemplateContext::new(now);
//! assert_eq!(
//!     render_template("It's {time}. Only {daysUntilChristmas} days until Christmas!", &ctx),
//!     "It's seven thirty PM. Only 12 days until Christmas!"
//! );
//! ```

use crate::model::DJ_PLACEHOLDERS;
use chrono::{DateTime, Datelike, NaiveDate, NaiveTime, Timelike};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

/// Temperature unit for `{temperature}`. Only affects documentation of the
/// value; the spoken text is "N degrees".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TemperatureUnit {
    #[default]
    Fahrenheit,
    Celsius,
}

/// Values available to placeholders. Optional values that are missing are
/// replaced by a natural fallback phrase (see [`render_template_report`]).
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateContext {
    /// Current local time (`{time}`, `{date}`, `{day}`, `{daysUntilChristmas}`).
    pub now: DateTime<Tz>,
    /// `{showName}`.
    pub show_name: Option<String>,
    /// `{nextSong}`: title of the song after this clip.
    pub next_song: Option<String>,
    /// `{prevSong}`: title of the song before this clip.
    pub prev_song: Option<String>,
    /// `{temperature}`, in `temperature_unit`.
    pub temperature: Option<f64>,
    /// Unit of `temperature`.
    pub temperature_unit: TemperatureUnit,
    /// `{sunset}`: today's sunset in local time.
    pub sunset: Option<DateTime<Tz>>,
    /// `{requestName}`: who requested the next song.
    pub request_name: Option<String>,
    /// Spell out plain numbers ("twelve" instead of "12"). Times are always
    /// spoken as words.
    pub numbers_as_words: bool,
}

impl TemplateContext {
    /// A context with only the current time set.
    pub fn new(now: DateTime<Tz>) -> Self {
        TemplateContext {
            now,
            show_name: None,
            next_song: None,
            prev_song: None,
            temperature: None,
            temperature_unit: TemperatureUnit::default(),
            sunset: None,
            request_name: None,
            numbers_as_words: false,
        }
    }

    /// Fill `sunset` from the observer's location (degrees, north/east positive).
    pub fn with_location(mut self, lat: f64, lon: f64) -> Self {
        let times = crate::sun::sun_times(lat, lon, self.now.date_naive());
        self.sunset = times.sunset.map(|t| t.with_timezone(&self.now.timezone()));
        self
    }
}

/// Result of rendering a template, with diagnostics for the editor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderedTemplate {
    /// The text with placeholders replaced.
    pub text: String,
    /// Placeholders that are not recognised; they are left in the text as-is.
    pub unknown: Vec<String>,
    /// Known placeholders whose value was unavailable and replaced with a
    /// fallback phrase (e.g. `nextSong` → "our next song").
    pub missing: Vec<String>,
}

/// Replace placeholders in `text`. Unknown placeholders are left intact.
pub fn render_template(text: &str, ctx: &TemplateContext) -> String {
    render_template_report(text, ctx).text
}

/// Replace placeholders in `text` and report unknown and missing ones.
pub fn render_template_report(text: &str, ctx: &TemplateContext) -> RenderedTemplate {
    let mut out = String::with_capacity(text.len() + 32);
    let mut unknown = Vec::new();
    let mut missing = Vec::new();
    let mut last = 0;
    for (range, name) in scan(text) {
        out.push_str(&text[last..range.start]);
        match value_for(name, ctx) {
            Some(Value::Present(v)) => out.push_str(&v),
            Some(Value::Fallback(v)) => {
                out.push_str(v);
                push_unique(&mut missing, name);
            }
            None => {
                out.push_str(&text[range.clone()]);
                push_unique(&mut unknown, name);
            }
        }
        last = range.end;
    }
    out.push_str(&text[last..]);
    RenderedTemplate {
        text: out,
        unknown,
        missing,
    }
}

/// Distinct placeholder names in `text`, in order of first appearance
/// (known and unknown alike).
pub fn placeholders_in(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    for (_, name) in scan(text) {
        push_unique(&mut names, name);
    }
    names
}

/// Placeholders in `text` that PixelPlus does not know.
pub fn unknown_placeholders(text: &str) -> Vec<String> {
    placeholders_in(text)
        .into_iter()
        .filter(|n| !DJ_PLACEHOLDERS.contains(&n.as_str()))
        .collect()
}

/// Whether `text` contains any placeholder (a clip with placeholders is
/// "dynamic" and must be re-rendered at show time).
pub fn has_placeholders(text: &str) -> bool {
    scan(text).next().is_some()
}

fn push_unique(v: &mut Vec<String>, name: &str) {
    if !v.iter().any(|n| n == name) {
        v.push(name.to_string());
    }
}

/// Iterate `{identifier}` tokens: byte range (including braces) and name.
fn scan(text: &str) -> impl Iterator<Item = (std::ops::Range<usize>, &str)> {
    let bytes = text.as_bytes();
    let mut i = 0;
    std::iter::from_fn(move || {
        while i < bytes.len() {
            if bytes[i] == b'{' {
                let start = i;
                let mut j = i + 1;
                if j < bytes.len() && bytes[j].is_ascii_alphabetic() {
                    while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_')
                    {
                        j += 1;
                    }
                    if j < bytes.len() && bytes[j] == b'}' {
                        i = j + 1;
                        // Only ASCII bytes were matched, so these are char boundaries.
                        return Some((start..j + 1, &text[start + 1..j]));
                    }
                }
            }
            i += 1;
        }
        None
    })
}

enum Value {
    Present(String),
    Fallback(&'static str),
}

fn value_for(name: &str, ctx: &TemplateContext) -> Option<Value> {
    let text_or = |v: &Option<String>, fallback: &'static str| match v {
        Some(s) if !s.trim().is_empty() => Value::Present(s.trim().to_string()),
        _ => Value::Fallback(fallback),
    };
    let today = ctx.now.date_naive();
    Some(match name {
        "time" => Value::Present(speak_time(ctx.now.time())),
        "date" => Value::Present(speak_date(today, ctx.numbers_as_words)),
        "day" => Value::Present(weekday_name(today).to_string()),
        "daysUntilChristmas" => Value::Present(speak_number(
            i64::from(days_until_christmas(today)),
            ctx.numbers_as_words,
        )),
        "nextSong" => text_or(&ctx.next_song, "our next song"),
        "prevSong" => text_or(&ctx.prev_song, "that last song"),
        "showName" => text_or(&ctx.show_name, "the show"),
        "requestName" => text_or(&ctx.request_name, "a special friend"),
        "temperature" => match ctx.temperature {
            Some(t) if t.is_finite() => Value::Present(speak_temperature(t, ctx.numbers_as_words)),
            _ => Value::Fallback("nice and chilly"),
        },
        "sunset" => match ctx.sunset {
            Some(t) => Value::Present(speak_time(t.time())),
            None => Value::Fallback("sunset"),
        },
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Speech formatting
// ---------------------------------------------------------------------------

/// Days from `today` until the next December 25th (0 on Christmas Day).
pub fn days_until_christmas(today: NaiveDate) -> u32 {
    let this_year = NaiveDate::from_ymd_opt(today.year(), 12, 25);
    let target = match this_year {
        Some(d) if d >= today => d,
        _ => NaiveDate::from_ymd_opt(today.year() + 1, 12, 25).unwrap_or(today),
    };
    (target - today).num_days().max(0) as u32
}

/// A time of day as spoken: "seven thirty PM", "nine o'clock AM",
/// "ten oh five PM", "noon", "midnight".
pub fn speak_time(t: NaiveTime) -> String {
    let (h, m) = (t.hour(), t.minute());
    match (h, m) {
        (0, 0) => return "midnight".into(),
        (12, 0) => return "noon".into(),
        _ => {}
    }
    let hour12 = match h % 12 {
        0 => 12,
        x => x,
    };
    let suffix = if h < 12 { "AM" } else { "PM" };
    let minutes = match m {
        0 => "o'clock".to_string(),
        1..=9 => format!("oh {}", number_to_words(i64::from(m))),
        _ => number_to_words(i64::from(m)),
    };
    format!(
        "{} {} {}",
        number_to_words(i64::from(hour12)),
        minutes,
        suffix
    )
}

/// A date as spoken: "Thursday, December 24th" (or "... twenty-fourth"
/// with `words`).
pub fn speak_date(date: NaiveDate, words: bool) -> String {
    let day = date.day();
    let day = if words {
        ordinal_words(day)
    } else {
        format!("{day}{}", ordinal_suffix(day))
    };
    format!(
        "{}, {} {}",
        weekday_name(date),
        month_name(date.month()),
        day
    )
}

fn speak_number(n: i64, words: bool) -> String {
    if words {
        number_to_words(n)
    } else {
        n.to_string()
    }
}

fn speak_temperature(t: f64, words: bool) -> String {
    let n = t.round().clamp(-1000.0, 1000.0) as i64;
    let unit = if n.abs() == 1 { "degree" } else { "degrees" };
    let number = if words {
        number_to_words(n)
    } else if n < 0 {
        format!("minus {}", -n)
    } else {
        n.to_string()
    };
    format!("{number} {unit}")
}

/// English weekday name.
pub fn weekday_name(date: NaiveDate) -> &'static str {
    match date.weekday() {
        chrono::Weekday::Mon => "Monday",
        chrono::Weekday::Tue => "Tuesday",
        chrono::Weekday::Wed => "Wednesday",
        chrono::Weekday::Thu => "Thursday",
        chrono::Weekday::Fri => "Friday",
        chrono::Weekday::Sat => "Saturday",
        chrono::Weekday::Sun => "Sunday",
    }
}

/// English month name for 1..=12 (empty for anything else).
pub fn month_name(month: u32) -> &'static str {
    const NAMES: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    month
        .checked_sub(1)
        .and_then(|i| NAMES.get(i as usize))
        .copied()
        .unwrap_or("")
}

/// "st", "nd", "rd" or "th" for a positive integer.
pub fn ordinal_suffix(n: u32) -> &'static str {
    match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    }
}

const ONES: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const TENS: [&str; 10] = [
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

/// A whole number in English words: `-42` → "minus forty-two",
/// `1205` → "one thousand two hundred five".
pub fn number_to_words(n: i64) -> String {
    if n == 0 {
        return "zero".into();
    }
    let mut parts = Vec::new();
    if n < 0 {
        parts.push("minus".to_string());
    }
    let mut rest = n.unsigned_abs();
    const SCALES: [(u64, &str); 6] = [
        (1_000_000_000_000_000_000, "quintillion"),
        (1_000_000_000_000_000, "quadrillion"),
        (1_000_000_000_000, "trillion"),
        (1_000_000_000, "billion"),
        (1_000_000, "million"),
        (1_000, "thousand"),
    ];
    for (scale, name) in SCALES {
        if rest >= scale {
            parts.push(format!("{} {name}", under_thousand(rest / scale)));
            rest %= scale;
        }
    }
    if rest > 0 {
        parts.push(under_thousand(rest));
    }
    parts.join(" ")
}

fn under_thousand(n: u64) -> String {
    let (hundreds, rest) = (n / 100, n % 100);
    let mut s = String::new();
    if hundreds > 0 {
        s.push_str(ONES[hundreds as usize]);
        s.push_str(" hundred");
    }
    if rest > 0 {
        if !s.is_empty() {
            s.push(' ');
        }
        if rest < 20 {
            s.push_str(ONES[rest as usize]);
        } else {
            s.push_str(TENS[(rest / 10) as usize]);
            if rest % 10 > 0 {
                s.push('-');
                s.push_str(ONES[(rest % 10) as usize]);
            }
        }
    }
    s
}

/// An ordinal in English words: 1 → "first", 24 → "twenty-fourth".
pub fn ordinal_words(n: u32) -> String {
    let cardinal = number_to_words(i64::from(n));
    // Only the final word changes.
    let split = cardinal.rfind([' ', '-']).map_or(0, |i| i + 1);
    let (head, last) = cardinal.split_at(split);
    let last = match last {
        "one" => "first".to_string(),
        "two" => "second".to_string(),
        "three" => "third".to_string(),
        "five" => "fifth".to_string(),
        "eight" => "eighth".to_string(),
        "nine" => "ninth".to_string(),
        "twelve" => "twelfth".to_string(),
        w if w.ends_with('y') => format!("{}ieth", &w[..w.len() - 1]),
        w => format!("{w}th"),
    };
    format!("{head}{last}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ctx(y: i32, m: u32, d: u32, h: u32, mi: u32) -> TemplateContext {
        TemplateContext::new(
            chrono_tz::America::Chicago
                .with_ymd_and_hms(y, m, d, h, mi, 0)
                .unwrap(),
        )
    }

    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    #[test]
    fn times_are_speakable() {
        assert_eq!(speak_time(t(19, 30)), "seven thirty PM");
        assert_eq!(speak_time(t(19, 0)), "seven o'clock PM");
        assert_eq!(speak_time(t(22, 5)), "ten oh five PM");
        assert_eq!(speak_time(t(0, 0)), "midnight");
        assert_eq!(speak_time(t(12, 0)), "noon");
        assert_eq!(speak_time(t(0, 15)), "twelve fifteen AM");
        assert_eq!(speak_time(t(12, 45)), "twelve forty-five PM");
        assert_eq!(speak_time(t(9, 11)), "nine eleven AM");
    }

    #[test]
    fn dates_are_speakable() {
        let d = NaiveDate::from_ymd_opt(2026, 12, 24).unwrap();
        assert_eq!(speak_date(d, false), "Thursday, December 24th");
        assert_eq!(speak_date(d, true), "Thursday, December twenty-fourth");
        let d = NaiveDate::from_ymd_opt(2026, 12, 1).unwrap();
        assert_eq!(speak_date(d, true), "Tuesday, December first");
        assert_eq!(month_name(0), "");
        assert_eq!(month_name(13), "");
    }

    #[test]
    fn numbers_and_ordinals() {
        assert_eq!(number_to_words(0), "zero");
        assert_eq!(number_to_words(-42), "minus forty-two");
        assert_eq!(number_to_words(1205), "one thousand two hundred five");
        assert_eq!(number_to_words(2_000_013), "two million thirteen");
        assert!(number_to_words(i64::MIN).starts_with("minus nine quintillion"));
        for (n, s) in [
            (1, "st"),
            (2, "nd"),
            (3, "rd"),
            (4, "th"),
            (11, "th"),
            (12, "th"),
            (13, "th"),
            (21, "st"),
            (22, "nd"),
            (101, "st"),
            (111, "th"),
        ] {
            assert_eq!(ordinal_suffix(n), s, "{n}");
        }
        assert_eq!(ordinal_words(1), "first");
        assert_eq!(ordinal_words(12), "twelfth");
        assert_eq!(ordinal_words(20), "twentieth");
        assert_eq!(ordinal_words(23), "twenty-third");
        assert_eq!(ordinal_words(30), "thirtieth");
        assert_eq!(ordinal_words(100), "one hundredth");
    }

    #[test]
    fn christmas_countdown() {
        let d = |m, day| NaiveDate::from_ymd_opt(2026, m, day).unwrap();
        assert_eq!(days_until_christmas(d(12, 13)), 12);
        assert_eq!(days_until_christmas(d(12, 24)), 1);
        assert_eq!(days_until_christmas(d(12, 25)), 0);
        assert_eq!(days_until_christmas(d(12, 26)), 364);
    }

    #[test]
    fn renders_all_known_placeholders() {
        let mut c = ctx(2026, 12, 13, 19, 30);
        c.show_name = Some("Chandler Lights".into());
        c.next_song = Some("Carol of the Bells".into());
        c.prev_song = Some("Jingle Bell Rock".into());
        c.temperature = Some(33.6);
        c.request_name = Some("Emma".into());
        c = c.with_location(41.8781, -87.6298);
        let text = DJ_PLACEHOLDERS
            .iter()
            .map(|p| format!("{{{p}}}"))
            .collect::<Vec<_>>()
            .join("|");
        let r = render_template_report(&text, &c);
        assert!(r.unknown.is_empty() && r.missing.is_empty(), "{r:?}");
        assert_eq!(
            r.text,
            "seven thirty PM|Sunday, December 13th|Sunday|12|Carol of the Bells|\
             Jingle Bell Rock|Chandler Lights|34 degrees|four nineteen PM|Emma"
        );
    }

    #[test]
    fn numbers_as_words_option() {
        let mut c = ctx(2026, 12, 13, 8, 0);
        c.numbers_as_words = true;
        c.temperature = Some(-4.2);
        assert_eq!(
            render_template("{daysUntilChristmas} days, {temperature}", &c),
            "twelve days, minus four degrees"
        );
        c.numbers_as_words = false;
        c.temperature = Some(1.0);
        assert_eq!(render_template("{temperature}", &c), "1 degree");
        c.temperature = Some(-3.0);
        assert_eq!(render_template("{temperature}", &c), "minus 3 degrees");
    }

    #[test]
    fn unknown_left_intact_and_missing_reported() {
        let c = ctx(2026, 12, 13, 19, 30);
        let r = render_template_report(
            "Hi {guestName}! Up next: {nextSong}. {nextSong}! {temperature}",
            &c,
        );
        assert_eq!(
            r.text,
            "Hi {guestName}! Up next: our next song. our next song! nice and chilly"
        );
        assert_eq!(r.unknown, vec!["guestName"]);
        assert_eq!(r.missing, vec!["nextSong", "temperature"]);
    }

    #[test]
    fn non_placeholders_are_untouched() {
        let c = ctx(2026, 12, 13, 19, 30);
        for s in [
            "{",
            "}",
            "{}",
            "{ time }",
            "{1abc}",
            "{time",
            "émoji {day} ✨ {",
        ] {
            let r = render_template(s, &c);
            if s.contains("{day}") {
                assert_eq!(r, "émoji Sunday ✨ {");
            } else {
                assert_eq!(r, s);
            }
        }
        assert_eq!(placeholders_in("{a}{b}{a} {c_1}"), vec!["a", "b", "c_1"]);
        assert_eq!(unknown_placeholders("{time} {foo}"), vec!["foo"]);
        assert!(has_placeholders("x {time} y"));
        assert!(!has_placeholders("no {1} braces {}"));
    }

    #[test]
    fn blank_strings_use_fallback() {
        let mut c = ctx(2026, 12, 13, 19, 30);
        c.request_name = Some("   ".into());
        assert_eq!(
            render_template("For {requestName}", &c),
            "For a special friend"
        );
    }
}
