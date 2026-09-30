//! Schedule expansion: weekdays, date ranges, sun times, priorities, DST and curfew.

use chrono::{DateTime, Duration, NaiveDate, TimeZone};
use chrono_tz::Tz;
use pixelplus_core::model::{
    DateRange, EndBehavior, Location, Schedule, ScheduleEntry, TimeSpec, VolumeCurfew, Weekday,
};
use pixelplus_core::schedule::{
    active_at, curfew_active, date_in_range, next_show, occurrences, schedule_timezone, validate,
    volume_at, MAX_PREVIEW_DAYS,
};

const ALL_DAYS: [Weekday; 7] = [
    Weekday::Mon,
    Weekday::Tue,
    Weekday::Wed,
    Weekday::Thu,
    Weekday::Fri,
    Weekday::Sat,
    Weekday::Sun,
];

fn clock(s: &str) -> TimeSpec {
    TimeSpec::Clock { time: s.into() }
}

fn entry(id: &str, start: TimeSpec, end: TimeSpec) -> ScheduleEntry {
    ScheduleEntry {
        id: id.into(),
        name: format!("Show {id}"),
        enabled: true,
        playlist_id: format!("pl-{id}"),
        days: ALL_DAYS.to_vec(),
        date_range: None,
        start,
        end,
        priority: 0,
        end_behavior: EndBehavior::FinishSong,
    }
}

fn chicago(entries: Vec<ScheduleEntry>) -> Schedule {
    Schedule {
        enabled: true,
        location: Location {
            lat: 41.8781,
            lon: -87.6298,
            timezone: "America/Chicago".into(),
            label: Some("Chicago".into()),
        },
        entries,
        ..Schedule::default()
    }
}

fn at(s: &Schedule, y: i32, m: u32, d: u32, h: u32, mi: u32) -> DateTime<Tz> {
    schedule_timezone(s)
        .unwrap()
        .with_ymd_and_hms(y, m, d, h, mi, 0)
        .earliest()
        .unwrap()
}

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn assert_non_overlapping(occ: &[pixelplus_core::schedule::Occurrence]) {
    for w in occ.windows(2) {
        assert!(w[0].start < w[0].end);
        assert!(
            w[0].end <= w[1].start,
            "{} [{}..{}) overlaps {} [{}..{})",
            w[0].entry_id,
            w[0].start,
            w[0].end,
            w[1].entry_id,
            w[1].start,
            w[1].end
        );
    }
}

#[test]
fn respects_days_of_week() {
    let mut e = entry("wk", clock("18:00"), clock("21:00"));
    e.days = vec![Weekday::Fri, Weekday::Sat];
    let s = chicago(vec![e]);
    // Tue 2026-12-01 .. Mon 2026-12-07
    let occ = occurrences(&s, at(&s, 2026, 12, 1, 0, 0), 7);
    let dates: Vec<_> = occ.iter().map(|o| o.date).collect();
    assert_eq!(dates, vec![date(2026, 12, 4), date(2026, 12, 5)]);
}

#[test]
fn date_range_wraps_year_end() {
    let mut e = entry("season", clock("17:00"), clock("22:00"));
    e.date_range = Some(DateRange {
        start: "11-25".into(),
        end: "01-06".into(),
    });
    let s = chicago(vec![e]);
    let occ = occurrences(&s, at(&s, 2026, 12, 30, 0, 0), 10);
    let dates: Vec<_> = occ.iter().map(|o| o.date).collect();
    let expected: Vec<_> = [(2026, 12, 30), (2026, 12, 31)]
        .into_iter()
        .chain((1..=6).map(|d| (2027, 1, d)))
        .map(|(y, m, d)| date(y, m, d))
        .collect();
    assert_eq!(dates, expected);

    let r = DateRange {
        start: "11-25".into(),
        end: "01-06".into(),
    };
    assert!(date_in_range(&r, date(2026, 11, 25)));
    assert!(date_in_range(&r, date(2027, 1, 6)));
    assert!(!date_in_range(&r, date(2027, 1, 7)));
    assert!(!date_in_range(&r, date(2026, 11, 24)));
}

#[test]
fn leap_day_range_only_matches_leap_years() {
    let mut e = entry("leap", clock("18:00"), clock("19:00"));
    e.date_range = Some(DateRange {
        start: "02-29".into(),
        end: "02-29".into(),
    });
    let s = chicago(vec![e]);
    assert!(occurrences(&s, at(&s, 2027, 2, 20, 0, 0), 20).is_empty());
    let occ = occurrences(&s, at(&s, 2028, 2, 20, 0, 0), 20);
    assert_eq!(occ.len(), 1);
    assert_eq!(occ[0].date, date(2028, 2, 29));
}

#[test]
fn end_past_midnight_belongs_to_start_day() {
    let mut e = entry("late", clock("22:00"), clock("01:00"));
    e.days = vec![Weekday::Fri];
    let s = chicago(vec![e]);
    // Friday 2026-12-04.
    let occ = occurrences(&s, at(&s, 2026, 12, 1, 0, 0), 7);
    assert_eq!(occ.len(), 1);
    assert_eq!(occ[0].start, at(&s, 2026, 12, 4, 22, 0));
    assert_eq!(occ[0].end, at(&s, 2026, 12, 5, 1, 0));

    // Still active after midnight on Saturday, attributed to Friday.
    let active = active_at(&s, at(&s, 2026, 12, 5, 0, 30)).expect("active");
    assert_eq!(active.entry_id, "late");
    assert_eq!(active.date, date(2026, 12, 4));
    assert!(active_at(&s, at(&s, 2026, 12, 5, 1, 0)).is_none());

    // A preview starting Saturday still reports the running window.
    let occ = occurrences(&s, at(&s, 2026, 12, 5, 0, 15), 1);
    assert_eq!(occ.len(), 1);
    assert_eq!(occ[0].date, date(2026, 12, 4));
}

#[test]
fn higher_priority_splits_lower() {
    let regular = entry("regular", clock("17:00"), clock("22:00"));
    let mut eve = entry("eve", clock("19:00"), clock("20:00"));
    eve.priority = 10;
    eve.date_range = Some(DateRange {
        start: "12-24".into(),
        end: "12-24".into(),
    });
    let s = chicago(vec![regular, eve]);
    let occ: Vec<_> = occurrences(&s, at(&s, 2026, 12, 24, 0, 0), 1);
    let summary: Vec<_> = occ
        .iter()
        .map(|o| {
            (
                o.entry_id.as_str(),
                o.start.format("%H:%M").to_string(),
                o.end.format("%H:%M").to_string(),
                o.preempted,
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            ("regular", "17:00".into(), "19:00".into(), true),
            ("eve", "19:00".into(), "20:00".into(), false),
            ("regular", "20:00".into(), "22:00".into(), false),
        ]
    );
    assert_eq!(
        active_at(&s, at(&s, 2026, 12, 24, 19, 30))
            .unwrap()
            .entry_id,
        "eve"
    );
    // Other days are untouched.
    assert_eq!(occurrences(&s, at(&s, 2026, 12, 23, 0, 0), 1).len(), 1);
}

#[test]
fn equal_priority_keeps_the_running_show() {
    let a = entry("a", clock("17:00"), clock("21:00"));
    let b = entry("b", clock("20:00"), clock("23:00"));
    let s = chicago(vec![b, a]);
    let occ = occurrences(&s, at(&s, 2026, 12, 2, 0, 0), 1);
    assert_eq!(occ.len(), 2);
    assert_eq!(occ[0].entry_id, "a");
    assert_eq!(occ[0].end, at(&s, 2026, 12, 2, 21, 0));
    assert!(!occ[0].preempted);
    assert_eq!(occ[1].entry_id, "b");
    assert_eq!(occ[1].start, at(&s, 2026, 12, 2, 21, 0));
}

#[test]
fn identical_windows_prefer_first_listed() {
    let a = entry("a", clock("17:00"), clock("21:00"));
    let b = entry("b", clock("17:00"), clock("21:00"));
    let s = chicago(vec![a, b]);
    let occ = occurrences(&s, at(&s, 2026, 12, 2, 0, 0), 1);
    assert_eq!(occ.len(), 1);
    assert_eq!(occ[0].entry_id, "a");
}

#[test]
fn sunset_relative_start() {
    let s = chicago(vec![entry(
        "dusk",
        TimeSpec::Sunset { offset_min: -15 },
        clock("22:00"),
    )]);
    let occ = occurrences(&s, at(&s, 2025, 12, 21, 0, 0), 1);
    assert_eq!(occ.len(), 1);
    // Sunset 16:22 CST, minus 15 minutes.
    let expected = at(&s, 2025, 12, 21, 16, 7);
    assert!((occ[0].start - expected).num_seconds().abs() <= 120);
    assert_eq!(occ[0].end, at(&s, 2025, 12, 21, 22, 0));
}

#[test]
fn sunrise_end_runs_overnight() {
    let s = chicago(vec![entry(
        "night",
        TimeSpec::Sunset { offset_min: 0 },
        TimeSpec::Sunrise { offset_min: 0 },
    )]);
    let occ = occurrences(&s, at(&s, 2025, 12, 21, 0, 0), 1);
    // The window from the previous evening is still running at midnight.
    assert_eq!(occ.len(), 2);
    let tonight = &occ[1];
    assert_eq!(tonight.date, date(2025, 12, 21));
    assert_eq!(tonight.end.date_naive(), date(2025, 12, 22));
    let hours = tonight.duration().num_minutes() as f64 / 60.0;
    assert!((14.5..15.5).contains(&hours), "{hours}");
}

#[test]
fn polar_night_still_schedules_sunset_shows() {
    let mut s = chicago(vec![entry(
        "arctic",
        TimeSpec::Sunset { offset_min: 0 },
        clock("20:00"),
    )]);
    s.location = Location {
        lat: 69.6492,
        lon: 18.9553,
        timezone: "Europe/Oslo".into(),
        label: None,
    };
    let occ = occurrences(&s, at(&s, 2025, 12, 20, 0, 0), 3);
    assert_eq!(occ.len(), 3);
    for o in &occ {
        assert!(o.start < o.end);
    }
}

#[test]
fn dst_fall_back_keeps_local_times() {
    // DST ends in Chicago on Sunday 2026-11-01.
    let s = chicago(vec![entry("eve", clock("18:00"), clock("23:00"))]);
    let occ = occurrences(&s, at(&s, 2026, 10, 31, 0, 0), 3);
    assert_eq!(occ.len(), 3);
    for o in &occ {
        assert_eq!(o.start.format("%H:%M").to_string(), "18:00");
        assert_eq!(o.duration(), Duration::hours(5));
    }
    assert_eq!(occ[0].start.format("%z").to_string(), "-0500");
    assert_eq!(occ[1].start.format("%z").to_string(), "-0600");
}

#[test]
fn dst_fall_back_overnight_is_an_hour_longer() {
    let s = chicago(vec![entry("late", clock("22:00"), clock("03:00"))]);
    let occ = occurrences(&s, at(&s, 2026, 10, 31, 12, 0), 1);
    assert_eq!(occ.len(), 1);
    assert_eq!(occ[0].duration(), Duration::hours(6));
}

#[test]
fn dst_ambiguous_time_uses_first_occurrence() {
    let s = chicago(vec![entry("fold", clock("01:30"), clock("02:30"))]);
    let occ = occurrences(&s, at(&s, 2026, 11, 1, 0, 0), 1);
    assert_eq!(occ[0].start.format("%H:%M %z").to_string(), "01:30 -0500");
    // 01:30 CDT to 02:30 CST is two real hours.
    assert_eq!(occ[0].duration(), Duration::hours(2));
}

#[test]
fn dst_spring_forward_gap_moves_to_gap_end() {
    // DST starts in Chicago on Sunday 2026-03-08: 02:00 → 03:00.
    let s = chicago(vec![entry("gap", clock("02:30"), clock("04:00"))]);
    let occ = occurrences(&s, at(&s, 2026, 3, 8, 0, 0), 1);
    assert_eq!(occ.len(), 1);
    assert_eq!(occ[0].start.format("%H:%M %z").to_string(), "03:00 -0500");
    assert_eq!(occ[0].duration(), Duration::hours(1));
}

#[test]
fn next_show_finds_the_following_window() {
    let s = chicago(vec![entry("eve", clock("17:30"), clock("22:00"))]);
    let next = next_show(&s, at(&s, 2026, 12, 1, 23, 0)).unwrap();
    assert_eq!(next.start, at(&s, 2026, 12, 2, 17, 30));

    // While a show runs, the next one is tomorrow's.
    let next = next_show(&s, at(&s, 2026, 12, 1, 18, 0)).unwrap();
    assert_eq!(next.date, date(2026, 12, 2));
}

#[test]
fn next_show_looks_almost_a_year_ahead() {
    let mut e = entry("eve", clock("18:00"), clock("23:00"));
    e.date_range = Some(DateRange {
        start: "12-24".into(),
        end: "12-24".into(),
    });
    let s = chicago(vec![e]);
    let next = next_show(&s, at(&s, 2027, 1, 5, 12, 0)).unwrap();
    assert_eq!(next.date, date(2027, 12, 24));
    assert!(next_show(&chicago(vec![]), at(&s, 2027, 1, 5, 12, 0)).is_none());
}

#[test]
fn disabled_and_invalid_entries_are_skipped() {
    let mut off = entry("off", clock("17:00"), clock("18:00"));
    off.enabled = false;
    let bad = entry("bad", clock("25:99"), clock("18:00"));
    let mut no_days = entry("nodays", clock("17:00"), clock("18:00"));
    no_days.days.clear();
    let s = chicago(vec![off, bad, no_days]);
    assert!(occurrences(&s, at(&s, 2026, 12, 1, 0, 0), 7).is_empty());

    let issues = validate(&s);
    assert!(issues
        .iter()
        .any(|i| i.entry_id.as_deref() == Some("bad") && i.field == "start"));
    assert!(issues
        .iter()
        .any(|i| i.entry_id.as_deref() == Some("nodays") && i.field == "days"));
}

#[test]
fn validate_flags_bad_ranges_and_offsets() {
    let mut e = entry("x", TimeSpec::Sunset { offset_min: 900 }, clock("22:00"));
    e.date_range = Some(DateRange {
        start: "02-30".into(),
        end: "1-1".into(),
    });
    let s = chicago(vec![e]);
    let fields: Vec<_> = validate(&s).into_iter().map(|i| i.field).collect();
    assert!(fields.contains(&"dateRange.start".to_string()));
    assert!(!fields.contains(&"dateRange.end".to_string()));
    assert!(fields.contains(&"start".to_string()));
    assert!(validate(&chicago(vec![entry("ok", clock("17:00"), clock("22:00"))])).is_empty());
}

#[test]
fn messy_schedule_is_non_overlapping_and_sorted() {
    let mut special = entry("special", clock("19:00"), clock("23:30"));
    special.priority = 5;
    special.days = vec![Weekday::Fri, Weekday::Sat];
    let mut late = entry("late", clock("23:00"), clock("00:30"));
    late.priority = 7;
    let mut season = entry(
        "season",
        TimeSpec::Sunset { offset_min: 10 },
        clock("22:00"),
    );
    season.date_range = Some(DateRange {
        start: "11-20".into(),
        end: "01-10".into(),
    });
    let s = chicago(vec![
        season,
        special,
        late,
        entry("always", clock("16:00"), clock("21:00")),
    ]);
    let occ = occurrences(&s, at(&s, 2026, 10, 25, 8, 0), 90);
    assert!(occ.len() > 150);
    assert_non_overlapping(&occ);
}

#[test]
fn preview_is_capped() {
    let s = chicago(vec![entry("eve", clock("18:00"), clock("19:00"))]);
    let occ = occurrences(&s, at(&s, 2026, 1, 1, 0, 0), u32::MAX);
    assert_eq!(occ.len(), MAX_PREVIEW_DAYS as usize);
}

#[test]
fn curfew_lowers_volume_until_noon() {
    let mut s = chicago(vec![entry("eve", clock("17:00"), clock("23:30"))]);
    s.volume_curfew = Some(VolumeCurfew {
        time: clock("21:00"),
        volume: 40,
    });
    assert_eq!(volume_at(&s, at(&s, 2026, 12, 1, 20, 59), 80), 80);
    assert_eq!(volume_at(&s, at(&s, 2026, 12, 1, 21, 0), 80), 40);
    assert_eq!(volume_at(&s, at(&s, 2026, 12, 2, 0, 30), 80), 40);
    assert_eq!(volume_at(&s, at(&s, 2026, 12, 2, 11, 59), 80), 40);
    assert_eq!(volume_at(&s, at(&s, 2026, 12, 2, 12, 0), 80), 80);
    // Never raises the volume.
    assert_eq!(volume_at(&s, at(&s, 2026, 12, 1, 22, 0), 30), 30);
    assert!(curfew_active(&s, at(&s, 2026, 12, 1, 22, 0)));

    s.volume_curfew = None;
    assert_eq!(volume_at(&s, at(&s, 2026, 12, 1, 22, 0), 80), 80);
}

#[test]
fn occurrence_serializes_camel_case_with_offsets() {
    let s = chicago(vec![entry("eve", clock("17:30"), clock("22:00"))]);
    let occ = occurrences(&s, at(&s, 2026, 12, 1, 0, 0), 1);
    let json = serde_json::to_value(&occ[0]).unwrap();
    assert_eq!(json["entryId"], "eve");
    assert_eq!(json["playlistId"], "pl-eve");
    assert_eq!(json["date"], "2026-12-01");
    assert_eq!(json["start"], "2026-12-01T17:30:00-06:00");
    assert_eq!(json["endBehavior"], "finishSong");
    assert_eq!(json["preempted"], false);
}
