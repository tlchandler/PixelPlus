//! Schedule expansion: turning a [`Schedule`] into concrete show windows.
//!
//! A schedule is a list of [`ScheduleEntry`] rules ("every day from sunset to
//! 22:00 between Nov 25 and Jan 6"). This module expands those rules into
//! [`Occurrence`]s on a real calendar, in the show's time zone, and resolves
//! overlaps so that at any instant exactly one entry is in charge: the one
//! with the highest `priority` (ties go to the entry that started first, then
//! to the entry listed first).
//!
//! Rules:
//!
//! * `days` and `dateRange` refer to the date an occurrence **starts**. A show
//!   from 22:00 to 01:00 on Friday runs into Saturday morning.
//! * If an entry's end time is not after its start time, the end is taken on
//!   the following day (end past midnight).
//! * `dateRange` is inclusive and wraps the year end when `start > end`
//!   (`11-25`..`01-06`). `02-29` only matches in leap years.
//! * Clock times that do not exist because of a daylight-saving jump forward
//!   resolve to the first valid minute after the gap (02:30 → 03:00); times
//!   that occur twice when clocks fall back resolve to the first occurrence.
//! * Sunset/sunrise times come from [`crate::sun`]; during polar day/night the
//!   fallback from [`SunTimes::sunset_or_fallback`](crate::sun::SunTimes::sunset_or_fallback)
//!   is used so shows still run.
//!
//! All functions take instants in a [`chrono_tz::Tz`]; obtain it with
//! [`schedule_timezone`] and convert the current time with
//! `Utc::now().with_timezone(&tz)`.

use crate::model::{DateRange, EndBehavior, Schedule, ScheduleEntry, TimeSpec, Weekday};
use crate::sun::sun_times;
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, TimeZone};
use chrono_tz::Tz;
use serde::Serialize;

/// Upper bound on how many days [`occurrences`] will expand (ten years).
pub const MAX_PREVIEW_DAYS: u32 = 3660;

/// How far ahead [`next_show`] looks for the next show window.
const NEXT_SHOW_HORIZON_DAYS: u32 = 400;

/// Rules are expanded this many days before and after the requested window
/// so that occurrences reaching into it (a show still running after
/// midnight, or a large sunset offset) are found and overlaps with them are
/// resolved consistently. Sun offsets are limited by [`MAX_SUN_OFFSET_MIN`].
const MARGIN_DAYS: i64 = 2;

/// Largest sunset/sunrise offset honoured, in minutes (±12 h). Larger offsets
/// are clamped (and reported by [`validate`]).
pub const MAX_SUN_OFFSET_MIN: i32 = 720;

/// Errors from schedule evaluation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScheduleError {
    /// The schedule's `location.timezone` is not a known IANA zone name.
    #[error("unknown time zone \"{0}\"; use an IANA name such as \"America/Chicago\"")]
    UnknownTimezone(String),
}

/// Problems with a single schedule entry, reported by [`validate`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleIssue {
    /// Entry the problem belongs to (`None` for schedule-wide problems).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_id: Option<String>,
    /// Which field is wrong, in camelCase (`start`, `dateRange.end`, ...).
    pub field: String,
    /// Human-readable explanation.
    pub message: String,
}

/// One concrete, non-overlapping show window.
///
/// When a higher-priority entry interrupts a lower-priority one, the lower
/// entry is split into pieces around it; each piece is its own occurrence.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Occurrence {
    /// Local date the entry's rule fired on (the start date of the rule, even
    /// for a piece that resumes after midnight).
    pub date: NaiveDate,
    /// Start of this window.
    pub start: DateTime<Tz>,
    /// End of this window (exclusive).
    pub end: DateTime<Tz>,
    pub entry_id: String,
    pub playlist_id: String,
    pub name: String,
    pub priority: i32,
    /// What to do when the window ends naturally.
    pub end_behavior: EndBehavior,
    /// `true` if this window ends early because a higher-priority entry takes
    /// over (the player should switch immediately rather than apply
    /// `end_behavior`).
    pub preempted: bool,
}

impl Occurrence {
    /// Whether `t` lies inside this window (`start <= t < end`).
    pub fn contains(&self, t: DateTime<Tz>) -> bool {
        self.start <= t && t < self.end
    }

    /// Length of this window.
    pub fn duration(&self) -> Duration {
        self.end - self.start
    }
}

/// The schedule entry in charge at a given instant.
pub type ActiveEntry = Occurrence;

/// Parse the schedule's IANA time zone.
pub fn schedule_timezone(schedule: &Schedule) -> Result<Tz, ScheduleError> {
    schedule
        .location
        .timezone
        .trim()
        .parse::<Tz>()
        .map_err(|_| ScheduleError::UnknownTimezone(schedule.location.timezone.clone()))
}

/// Check every entry for malformed values. Entries with issues are skipped by
/// the expansion functions, so the UI should surface these.
pub fn validate(schedule: &Schedule) -> Vec<ScheduleIssue> {
    let mut issues = Vec::new();
    if schedule_timezone(schedule).is_err() {
        issues.push(ScheduleIssue {
            entry_id: None,
            field: "location.timezone".into(),
            message: format!(
                "Unknown time zone \"{}\". Pick one like \"America/Chicago\".",
                schedule.location.timezone
            ),
        });
    }
    let loc = &schedule.location;
    if !(loc.lat.is_finite() && (-90.0..=90.0).contains(&loc.lat)) {
        issues.push(ScheduleIssue {
            entry_id: None,
            field: "location.lat".into(),
            message: "Latitude must be between -90 and 90.".into(),
        });
    }
    if !(loc.lon.is_finite() && (-180.0..=180.0).contains(&loc.lon)) {
        issues.push(ScheduleIssue {
            entry_id: None,
            field: "location.lon".into(),
            message: "Longitude must be between -180 and 180.".into(),
        });
    }
    if let Some(curfew) = &schedule.volume_curfew {
        if let TimeSpec::Clock { time } = &curfew.time {
            if parse_clock(time).is_none() {
                issues.push(ScheduleIssue {
                    entry_id: None,
                    field: "volumeCurfew.time".into(),
                    message: format!("\"{time}\" is not a valid time; use HH:MM (24-hour)."),
                });
            }
        }
    }
    for entry in &schedule.entries {
        let mut push = |field: &str, message: String| {
            issues.push(ScheduleIssue {
                entry_id: Some(entry.id.clone()),
                field: field.into(),
                message,
            })
        };
        for (field, spec) in [("start", &entry.start), ("end", &entry.end)] {
            match spec {
                TimeSpec::Clock { time } => {
                    if parse_clock(time).is_none() {
                        push(
                            field,
                            format!("\"{time}\" is not a valid time; use HH:MM (24-hour)."),
                        );
                    }
                }
                TimeSpec::Sunset { offset_min } | TimeSpec::Sunrise { offset_min } => {
                    if offset_min.abs() > MAX_SUN_OFFSET_MIN {
                        push(
                            field,
                            format!(
                                "Offsets are limited to {} hours either side.",
                                MAX_SUN_OFFSET_MIN / 60
                            ),
                        );
                    }
                }
            }
        }
        if entry.days.is_empty() {
            push("days", "Pick at least one day of the week.".into());
        }
        if let Some(range) = &entry.date_range {
            if parse_month_day(&range.start).is_none() {
                push(
                    "dateRange.start",
                    format!("\"{}\" is not a valid date; use MM-DD.", range.start),
                );
            }
            if parse_month_day(&range.end).is_none() {
                push(
                    "dateRange.end",
                    format!("\"{}\" is not a valid date; use MM-DD.", range.end),
                );
            }
        }
        if entry.start == entry.end {
            push(
                "end",
                "Start and end are the same, so this runs for a full 24 hours.".into(),
            );
        }
    }
    issues
}

/// Expand the schedule into non-overlapping show windows for `days` local
/// calendar days starting with the date of `from`.
///
/// Returns every window that intersects `[from, midnight after the last
/// day)`, including one already running at `from` (its `start` is then before
/// `from`). Results are sorted by start. Times are computed in `from`'s zone.
/// Disabled schedules and disabled or invalid entries produce nothing.
/// `days` is capped at [`MAX_PREVIEW_DAYS`].
pub fn occurrences(schedule: &Schedule, from: DateTime<Tz>, days: u32) -> Vec<Occurrence> {
    let days = days.min(MAX_PREVIEW_DAYS);
    if !schedule.enabled || days == 0 {
        return Vec::new();
    }
    let tz = from.timezone();
    let first = from.date_naive();
    let last = first + Duration::days(i64::from(days) - 1);
    let window_end = local_instant(&tz, last + Duration::days(1), NaiveTime::MIN);
    resolve(
        schedule,
        &tz,
        first - Duration::days(MARGIN_DAYS),
        last + Duration::days(MARGIN_DAYS),
    )
    .into_iter()
    .filter(|o| o.end > from && o.start < window_end)
    .collect()
}

/// The window in charge at `now`, if any.
pub fn active_at(schedule: &Schedule, now: DateTime<Tz>) -> Option<ActiveEntry> {
    occurrences(schedule, now, 1)
        .into_iter()
        .find(|o| o.contains(now))
}

/// The next window that starts strictly after `now` (looking up to ~13
/// months ahead). The currently running window, if any, is not returned.
pub fn next_show(schedule: &Schedule, now: DateTime<Tz>) -> Option<Occurrence> {
    // Search in chunks so the common case (a show within days) stays cheap.
    const CHUNK: u32 = 14;
    let mut offset = 0;
    while offset < NEXT_SHOW_HORIZON_DAYS {
        let chunk_start = if offset == 0 {
            now
        } else {
            let date = now.date_naive() + Duration::days(i64::from(offset));
            local_instant(&now.timezone(), date, NaiveTime::MIN)
        };
        if let Some(o) = occurrences(schedule, chunk_start, CHUNK)
            .into_iter()
            .find(|o| o.start > now)
        {
            return Some(o);
        }
        offset += CHUNK;
    }
    None
}

/// Whether the volume curfew is in effect at `now`.
///
/// A curfew starts at its time on each day and lasts until the following
/// local noon, so a 21:00 curfew still applies to a show running past
/// midnight. (A curfew time before noon, such as 01:00, lasts until noon of
/// that same day.)
pub fn curfew_active(schedule: &Schedule, now: DateTime<Tz>) -> bool {
    let Some(curfew) = &schedule.volume_curfew else {
        return false;
    };
    let tz = now.timezone();
    let today = now.date_naive();
    [today - Duration::days(1), today].into_iter().any(|date| {
        let Some(start) = resolve_time(&curfew.time, date, &tz, schedule) else {
            return false;
        };
        let noon = NaiveTime::from_hms_opt(12, 0, 0).unwrap_or(NaiveTime::MIN);
        let local_start = start.naive_local();
        let end_date = if local_start.time() < noon {
            local_start.date()
        } else {
            local_start.date() + Duration::days(1)
        };
        let end = local_instant(&tz, end_date, noon);
        start <= now && now < end
    })
}

/// Effective volume at `now`: `base_volume`, lowered to the curfew volume
/// while the curfew is active. The curfew never raises the volume.
pub fn volume_at(schedule: &Schedule, now: DateTime<Tz>, base_volume: u8) -> u8 {
    match &schedule.volume_curfew {
        Some(c) if curfew_active(schedule, now) => base_volume.min(c.volume),
        _ => base_volume,
    }
}

/// Resolve a [`TimeSpec`] on local date `date` to an instant.
///
/// Returns `None` only for malformed clock strings.
pub fn resolve_time(
    spec: &TimeSpec,
    date: NaiveDate,
    tz: &Tz,
    schedule: &Schedule,
) -> Option<DateTime<Tz>> {
    let loc = &schedule.location;
    match spec {
        TimeSpec::Clock { time } => parse_clock(time).map(|t| local_instant(tz, date, t)),
        TimeSpec::Sunset { offset_min } => {
            let base = sun_times(loc.lat, loc.lon, date).sunset_or_fallback();
            Some((base + sun_offset(*offset_min)).with_timezone(tz))
        }
        TimeSpec::Sunrise { offset_min } => {
            let base = sun_times(loc.lat, loc.lon, date).sunrise_or_fallback();
            Some((base + sun_offset(*offset_min)).with_timezone(tz))
        }
    }
}

/// Parse `"HH:MM"` (24-hour, `H:MM` accepted) into a time of day.
pub fn parse_clock(s: &str) -> Option<NaiveTime> {
    let (h, m) = s.trim().split_once(':')?;
    if h.is_empty() || h.len() > 2 || m.len() != 2 {
        return None;
    }
    let h: u32 = h.parse().ok()?;
    let m: u32 = m.parse().ok()?;
    NaiveTime::from_hms_opt(h, m, 0)
}

/// Parse `"MM-DD"` into `(month, day)`. `02-29` is accepted.
pub fn parse_month_day(s: &str) -> Option<(u32, u32)> {
    let (m, d) = s.trim().split_once('-')?;
    let m: u32 = m.parse().ok()?;
    let d: u32 = d.parse().ok()?;
    // Validate against a leap year so 02-29 is allowed.
    NaiveDate::from_ymd_opt(2024, m, d).map(|_| (m, d))
}

/// Whether `date` falls inside an inclusive, possibly year-wrapping range.
/// Malformed ranges match nothing.
pub fn date_in_range(range: &DateRange, date: NaiveDate) -> bool {
    let (Some(start), Some(end)) = (parse_month_day(&range.start), parse_month_day(&range.end))
    else {
        return false;
    };
    let md = (date.month(), date.day());
    if start <= end {
        start <= md && md <= end
    } else {
        md >= start || md <= end
    }
}

/// Convert a chrono weekday to the model's weekday.
pub fn weekday_of(date: NaiveDate) -> Weekday {
    match date.weekday() {
        chrono::Weekday::Mon => Weekday::Mon,
        chrono::Weekday::Tue => Weekday::Tue,
        chrono::Weekday::Wed => Weekday::Wed,
        chrono::Weekday::Thu => Weekday::Thu,
        chrono::Weekday::Fri => Weekday::Fri,
        chrono::Weekday::Sat => Weekday::Sat,
        chrono::Weekday::Sun => Weekday::Sun,
    }
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

fn sun_offset(offset_min: i32) -> Duration {
    Duration::minutes(i64::from(
        offset_min.clamp(-MAX_SUN_OFFSET_MIN, MAX_SUN_OFFSET_MIN),
    ))
}

/// A local wall-clock time made concrete, handling DST gaps and folds.
fn local_instant(tz: &Tz, date: NaiveDate, time: NaiveTime) -> DateTime<Tz> {
    let naive = date.and_time(time);
    if let Some(dt) = tz.from_local_datetime(&naive).earliest() {
        return dt;
    }
    // In a DST gap: walk forward minute by minute to the first valid time.
    // Gaps are at most a few hours in practice; bound the search anyway.
    for minutes in 1..=(24 * 60) {
        let candidate = naive + Duration::minutes(minutes);
        if let Some(dt) = tz.from_local_datetime(&candidate).earliest() {
            return dt;
        }
    }
    // Unreachable for real zones; fall back to interpreting as UTC.
    tz.from_utc_datetime(&naive)
}

/// A raw (possibly overlapping) occurrence of one entry.
struct Raw<'a> {
    entry: &'a ScheduleEntry,
    order: usize,
    date: NaiveDate,
    start: DateTime<Tz>,
    end: DateTime<Tz>,
}

fn raw_for_date<'a>(
    schedule: &Schedule,
    entry: &'a ScheduleEntry,
    order: usize,
    date: NaiveDate,
    tz: &Tz,
) -> Option<Raw<'a>> {
    if !entry.enabled || !entry.days.contains(&weekday_of(date)) {
        return None;
    }
    if let Some(range) = &entry.date_range {
        if !date_in_range(range, date) {
            return None;
        }
    }
    let start = resolve_time(&entry.start, date, tz, schedule)?;
    let mut end = resolve_time(&entry.end, date, tz, schedule)?;
    if end <= start {
        end = resolve_time(&entry.end, date + Duration::days(1), tz, schedule)?;
    }
    (end > start).then_some(Raw {
        entry,
        order,
        date,
        start,
        end,
    })
}

/// Expand rules fired on dates `first..=last` and resolve overlaps.
fn resolve(schedule: &Schedule, tz: &Tz, first: NaiveDate, last: NaiveDate) -> Vec<Occurrence> {
    let mut raws = Vec::new();
    let mut date = first;
    while date <= last {
        for (order, entry) in schedule.entries.iter().enumerate() {
            raws.extend(raw_for_date(schedule, entry, order, date, tz));
        }
        date += Duration::days(1);
    }
    if raws.is_empty() {
        return Vec::new();
    }

    raws.sort_by(|a, b| a.start.cmp(&b.start));
    let mut bounds: Vec<DateTime<Tz>> = raws.iter().flat_map(|r| [r.start, r.end]).collect();
    bounds.sort();
    bounds.dedup();

    // Sweep the elementary intervals between consecutive bounds, keeping the
    // set of raw occurrences covering the current interval. For each interval
    // pick the winner, merging consecutive intervals won by the same raw.
    let mut out: Vec<(usize, DateTime<Tz>, DateTime<Tz>)> = Vec::new();
    let mut active: Vec<usize> = Vec::new();
    let mut next = 0;
    for pair in bounds.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        while next < raws.len() && raws[next].start <= a {
            active.push(next);
            next += 1;
        }
        active.retain(|&i| raws[i].end > a);
        let winner = active.iter().copied().max_by(|&x, &y| {
            let (x, y) = (&raws[x], &raws[y]);
            x.entry
                .priority
                .cmp(&y.entry.priority)
                .then(y.start.cmp(&x.start))
                .then(y.order.cmp(&x.order))
        });
        let Some(w) = winner else { continue };
        match out.last_mut() {
            Some((prev, _, end)) if *prev == w && *end == a => *end = b,
            _ => out.push((w, a, b)),
        }
    }

    out.into_iter()
        .map(|(i, start, end)| {
            let r = &raws[i];
            Occurrence {
                date: r.date,
                start,
                end,
                entry_id: r.entry.id.clone(),
                playlist_id: r.entry.playlist_id.clone(),
                name: r.entry.name.clone(),
                priority: r.entry.priority,
                end_behavior: r.entry.end_behavior,
                preempted: end < r.end,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Location;

    fn clock(s: &str) -> TimeSpec {
        TimeSpec::Clock { time: s.into() }
    }

    fn entry(id: &str, start: TimeSpec, end: TimeSpec, priority: i32) -> ScheduleEntry {
        ScheduleEntry {
            id: id.into(),
            name: format!("Entry {id}"),
            enabled: true,
            playlist_id: format!("pl-{id}"),
            days: vec![
                Weekday::Mon,
                Weekday::Tue,
                Weekday::Wed,
                Weekday::Thu,
                Weekday::Fri,
                Weekday::Sat,
                Weekday::Sun,
            ],
            date_range: None,
            start,
            end,
            priority,
            end_behavior: EndBehavior::FinishSong,
        }
    }

    fn schedule(entries: Vec<ScheduleEntry>) -> Schedule {
        Schedule {
            enabled: true,
            location: Location {
                lat: 41.8781,
                lon: -87.6298,
                timezone: "America/Chicago".into(),
                label: None,
            },
            entries,
            ..Schedule::default()
        }
    }

    fn at(s: &Schedule, y: i32, m: u32, d: u32, h: u32, mi: u32) -> DateTime<Tz> {
        let tz = schedule_timezone(s).unwrap();
        tz.with_ymd_and_hms(y, m, d, h, mi, 0).earliest().unwrap()
    }

    #[test]
    fn parses_clock_and_month_day() {
        assert_eq!(parse_clock("7:05"), NaiveTime::from_hms_opt(7, 5, 0));
        assert_eq!(parse_clock(" 23:59 "), NaiveTime::from_hms_opt(23, 59, 0));
        for bad in ["24:00", "7", "07:5", "aa:bb", "", "123:00", "-1:00"] {
            assert_eq!(parse_clock(bad), None, "{bad}");
        }
        assert_eq!(parse_month_day("02-29"), Some((2, 29)));
        assert_eq!(parse_month_day("13-01"), None);
        assert_eq!(parse_month_day("04-31"), None);
    }

    #[test]
    fn simple_daily_window() {
        let s = schedule(vec![entry("a", clock("17:30"), clock("22:00"), 0)]);
        let from = at(&s, 2026, 12, 1, 12, 0);
        let occ = occurrences(&s, from, 3);
        assert_eq!(occ.len(), 3);
        assert_eq!(occ[0].start, at(&s, 2026, 12, 1, 17, 30));
        assert_eq!(occ[0].end, at(&s, 2026, 12, 1, 22, 0));
        assert!(!occ[0].preempted);
    }

    #[test]
    fn disabled_schedule_is_empty() {
        let mut s = schedule(vec![entry("a", clock("17:30"), clock("22:00"), 0)]);
        s.enabled = false;
        assert!(occurrences(&s, at(&s, 2026, 12, 1, 12, 0), 7).is_empty());
    }

    #[test]
    fn unknown_timezone_is_reported() {
        let mut s = schedule(vec![]);
        s.location.timezone = "Mars/Olympus".into();
        assert!(matches!(
            schedule_timezone(&s),
            Err(ScheduleError::UnknownTimezone(_))
        ));
        assert!(validate(&s).iter().any(|i| i.field == "location.timezone"));
    }
}
