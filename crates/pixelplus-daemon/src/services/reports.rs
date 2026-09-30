//! Nightly health report (F11, ARCHITECTURE §12.10): turns one show night of
//! the journal into a [`NightReport`], stores it as `reports/<date>.json`,
//! and sends it by email (HTML + text) and push (ntfy).
//!
//! * **Show night** `D` = `D 12:00` → `D+1 12:00` local time (25 or 23 hours
//!   across a daylight-saving change). The report for night `D` is made at
//!   `settings.reports.time` on the morning of `D+1` (default 07:00), or with
//!   `time: "afterShow"` 15 minutes after the night's last show window ends.
//! * **Sampler**: every minute this service journals the leader's board
//!   temperature and voltage and its disk space (`metric`), and each online
//!   follower's sync error (`syncSample`), so the report has something to
//!   chart. Engine events (`itemStart`, `request`, `error`, …) are journaled
//!   by their owners; controller online/offline by `services/alerts.rs`.
//! * **Status**: `fail` with errors or a controller offline for more than
//!   10 minutes; `warn` with warnings, a hot controller, power limiting,
//!   suspect pixels, low disk or an old backup; `ok` otherwise.
//! * Reports older than `settings.reports.keepDays` are deleted.
//!
//! No secrets or addresses go into a report; the push text only links to it.

use super::journal::{Event, Record};
use crate::state::AppState;
use chrono::{DateTime, Duration as ChronoDuration, FixedOffset, NaiveDate, NaiveTime, TimeZone};
use chrono_tz::Tz;
use parking_lot::Mutex;
use pixelplus_core::model::{AlertRules, Show};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// `settings.reports.time` value: "15 minutes after the last show window".
pub const AFTER_SHOW: &str = "afterShow";
/// How long after the last window an `afterShow` report is made.
const AFTER_SHOW_DELAY_MIN: i64 = 15;
/// A controller offline this long makes the night a `fail`.
const OFFLINE_FAIL_MIN: f64 = 10.0;
/// Backups older than this make the night a `warn`.
const BACKUP_WARN_DAYS: u32 = 30;
/// Less free disk than this makes the night a `warn`.
const DISK_WARN_PCT: f64 = 10.0;
/// Chart buckets per series.
const SERIES_BUCKET_MIN: i64 = 15;

pub type Status = &'static str;

/// One night (`GET /reports/:date`). Field names mirror `NightReport` in
/// `web/src/lib/api/types.ts`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct NightReport {
    /// The evening's local date, "YYYY-MM-DD".
    pub date: String,
    /// "ok" | "warn" | "fail".
    pub status: String,
    pub headline: String,
    pub shows: Vec<ShowRun>,
    pub items_played: u32,
    pub requests: u32,
    pub top_requests: Vec<TopRequest>,
    pub problems: Vec<Problem>,
    pub nodes: Vec<NodeNight>,
    pub limiter: Vec<LimiterUse>,
    pub suspect_pixels: Vec<SuspectPixels>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_free_pct: Option<f64>,
    pub updates: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_age_days: Option<u32>,
    // ---- additions (optional in types.ts)
    /// RFC 3339.
    #[serde(default)]
    pub generated_at: String,
    /// The part of the night covered: RFC 3339 `[from, to)`.
    #[serde(default)]
    pub window: ReportWindow,
    /// Total show-window minutes.
    #[serde(default)]
    pub runtime_min: u32,
    /// Visitor games played and their total minutes.
    #[serde(default)]
    pub games: u32,
    #[serde(default)]
    pub game_minutes: f64,
    /// Triggers / sensor surprises fired.
    #[serde(default)]
    pub triggers: u32,
    /// Daemon starts during the night (crashes, updates, power cuts).
    #[serde(default)]
    pub restarts: u32,
    /// Active season, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<String>,
    /// Chart data.
    #[serde(default)]
    pub series: ReportSeries,
    /// What happened when it was sent ("Email sent to …", "Push failed: …").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub delivery: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReportWindow {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ShowRun {
    pub entry_id: String,
    pub name: String,
    pub started_at: String,
    pub runtime_min: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TopRequest {
    pub sequence_id: String,
    pub name: String,
    pub count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Problem {
    /// "warn" | "error".
    pub level: String,
    pub code: String,
    pub message: String,
    pub count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NodeNight {
    pub node_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temp_min_c: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temp_max_c: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volts_min: Option<f64>,
    pub offline_min: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_p50_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_p95_ms: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LimiterUse {
    pub node_id: String,
    pub port: u32,
    pub seconds: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SuspectPixels {
    pub prop_id: String,
    #[serde(default)]
    pub name: String,
    pub pixels: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReportSeries {
    /// Board temperature (°C), max per 15 minutes, per controller.
    pub temp_c: Vec<Series>,
    /// Sync error p95 (ms) per 15 minutes, per follower.
    pub sync_ms: Vec<Series>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Series {
    pub node_id: String,
    pub name: String,
    /// `[unix ms, value]`, oldest first.
    pub points: Vec<[f64; 2]>,
}

/// `GET /reports` entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReportSummary {
    pub date: String,
    pub status: String,
    pub headline: String,
    pub items_played: u32,
    pub requests: u32,
    pub problems: u32,
    pub runtime_min: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temp_max_c: Option<f64>,
}

impl NightReport {
    pub fn summary(&self) -> ReportSummary {
        ReportSummary {
            date: self.date.clone(),
            status: self.status.clone(),
            headline: self.headline.clone(),
            items_played: self.items_played,
            requests: self.requests,
            problems: self.problems.iter().map(|p| p.count).sum(),
            runtime_min: self.runtime_min,
            temp_max_c: self
                .nodes
                .iter()
                .filter_map(|n| n.temp_max_c)
                .fold(None, |a: Option<f64>, t| Some(a.map_or(t, |a| a.max(t)))),
        }
    }
}

// ---------------------------------------------------------------------------
// Show nights
// ---------------------------------------------------------------------------

/// Local noon of `date` (the earliest instant when ambiguous).
fn local_noon(tz: &Tz, date: NaiveDate) -> DateTime<FixedOffset> {
    let noon = date.and_time(NaiveTime::from_hms_opt(12, 0, 0).expect("noon"));
    tz.from_local_datetime(&noon)
        .earliest()
        .unwrap_or_else(|| tz.from_utc_datetime(&noon))
        .fixed_offset()
}

/// The show night `date`: `[date 12:00, date+1 12:00)` local.
pub fn night_window(tz: &Tz, date: NaiveDate) -> (DateTime<FixedOffset>, DateTime<FixedOffset>) {
    (
        local_noon(tz, date),
        local_noon(tz, date + ChronoDuration::days(1)),
    )
}

/// The night that `now` belongs to (before noon: the previous evening).
pub fn night_of(now: DateTime<Tz>) -> NaiveDate {
    let d = now.date_naive();
    if now.time() < NaiveTime::from_hms_opt(12, 0, 0).expect("noon") {
        d - ChronoDuration::days(1)
    } else {
        d
    }
}

/// When the report for night `date` is due, or `None` if `time` is invalid.
/// `"HH:MM"`: that time on the next morning (times from noon on count for
/// the same evening). `"afterShow"`: 15 min after the night's last window
/// (or the next day's noon when nothing was scheduled).
pub fn due_at(show: &Show, tz: &Tz, date: NaiveDate, time: &str) -> Option<DateTime<Tz>> {
    let (from, to) = night_window(tz, date);
    if time == AFTER_SHOW {
        let start = from.with_timezone(tz);
        let end = to.with_timezone(tz);
        let last = pixelplus_core::schedule::occurrences(&show.schedule, start, 2)
            .into_iter()
            .filter(|o| o.start >= start && o.start < end)
            .map(|o| o.end)
            .max();
        return Some(match last {
            Some(e) => e + ChronoDuration::minutes(AFTER_SHOW_DELAY_MIN),
            None => end,
        });
    }
    let t = pixelplus_core::schedule::parse_clock(time)?;
    let day = if t < NaiveTime::from_hms_opt(12, 0, 0).expect("noon") {
        date + ChronoDuration::days(1)
    } else {
        date
    };
    tz.from_local_datetime(&day.and_time(t)).earliest()
}

// ---------------------------------------------------------------------------
// Aggregation (pure)
// ---------------------------------------------------------------------------

/// Everything the aggregator needs besides the journal.
pub struct ReportContext<'a> {
    pub date: NaiveDate,
    pub from: DateTime<FixedOffset>,
    pub to: DateTime<FixedOffset>,
    pub generated_at: DateTime<FixedOffset>,
    pub show: &'a Show,
    /// This controller (the leader): board metrics are journaled under it.
    pub self_id: &'a str,
    /// Free disk now (percent), used when the journal has no sample.
    pub disk_free_pct: Option<f64>,
    pub backup_age_days: Option<u32>,
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    Some(sorted[idx.min(sorted.len() - 1)])
}

fn minutes(a: DateTime<FixedOffset>, b: DateTime<FixedOffset>) -> f64 {
    (b - a).num_milliseconds().max(0) as f64 / 60_000.0
}

fn node_name(show: &Show, id: &str) -> String {
    show.node(id)
        .map(|n| n.name.clone())
        .unwrap_or_else(|| id.to_string())
}

/// Metric names the sampler writes.
pub mod metric {
    pub const TEMP_C: &str = "tempC";
    pub const VOLTS: &str = "volts";
    pub const DISK_FREE_PCT: &str = "diskFreePct";
}

fn plural(n: u32, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// Build the report of one night from its journal records (any order).
pub fn aggregate(ctx: &ReportContext, records: &[Record]) -> NightReport {
    let show = ctx.show;
    let rules: &AlertRules = &show.settings.alerts.rules;
    let mut recs: Vec<(DateTime<FixedOffset>, &Event)> = records
        .iter()
        .filter_map(|r| r.time().map(|t| (t, &r.event)))
        .filter(|(t, _)| *t >= ctx.from && *t < ctx.to)
        .collect();
    recs.sort_by_key(|(t, _)| *t);
    let end = ctx.to.min(ctx.generated_at);

    // Shows: pair starts and ends per schedule entry.
    let mut shows: Vec<ShowRun> = vec![];
    let mut open: HashMap<&str, (DateTime<FixedOffset>, &str)> = HashMap::new();
    let close = |shows: &mut Vec<ShowRun>, id: &str, name: &str, a, b| {
        shows.push(ShowRun {
            entry_id: id.to_string(),
            name: name.to_string(),
            started_at: DateTime::<FixedOffset>::to_rfc3339(&a),
            runtime_min: minutes(a, b).round() as u32,
        })
    };
    let mut items_played = 0u32;
    let mut requests: BTreeMap<&str, (u32, &str)> = BTreeMap::new();
    let mut n_requests = 0u32;
    let mut problems: BTreeMap<(&'static str, String), Problem> = BTreeMap::new();
    let mut add_problem = |level: &'static str, code: &str, msg: &str| {
        let p = problems
            .entry((if level == "error" { "a" } else { "b" }, code.to_string()))
            .or_insert_with(|| Problem {
                level: level.into(),
                code: code.into(),
                message: String::new(),
                count: 0,
            });
        p.count += 1;
        p.message = msg.to_string(); // the latest wording
    };
    let mut temps: HashMap<&str, Vec<(i64, f64)>> = HashMap::new();
    let mut volts: HashMap<&str, f64> = HashMap::new();
    let mut disk: Option<f64> = None;
    let mut sync: HashMap<&str, Vec<(i64, f64)>> = HashMap::new();
    let mut offline_since: HashMap<&str, DateTime<FixedOffset>> = HashMap::new();
    let mut offline_min: HashMap<&str, f64> = HashMap::new();
    let mut limiter: BTreeMap<(&str, u32), f64> = BTreeMap::new();
    let mut updates = vec![];
    let (mut games, mut game_s, mut triggers, mut restarts) = (0u32, 0u64, 0u32, 0u32);
    let mut last_health: Option<&serde_json::Value> = None;

    for (t, ev) in &recs {
        let t = *t;
        match ev {
            Event::ShowStart { entry_id, name } => {
                if let Some((a, n)) = open.remove(entry_id.as_str()) {
                    close(&mut shows, entry_id, n, a, t);
                }
                open.insert(entry_id, (t, name));
            }
            Event::ShowEnd { entry_id, name } => match open.remove(entry_id.as_str()) {
                Some((a, n)) => close(&mut shows, entry_id, if n.is_empty() { name } else { n }, a, t),
                // Started before noon (unusual): count from the window start.
                None => close(&mut shows, entry_id, name, ctx.from, t),
            },
            Event::ItemStart { item, .. } if item == "sequence" => items_played += 1,
            Event::Request { sequence_id, name } => {
                n_requests += 1;
                let e = requests.entry(sequence_id).or_insert((0, name));
                e.0 += 1;
                if !name.is_empty() {
                    e.1 = name;
                }
            }
            Event::Game { s } => {
                games += 1;
                game_s += u64::from(*s);
            }
            Event::Error { code, msg } => add_problem("error", code, msg),
            Event::Warn { code, msg } => add_problem("warn", code, msg),
            Event::Health { checks } => last_health = Some(checks),
            Event::NodeOffline { id } => {
                offline_since.entry(id).or_insert(t);
            }
            Event::NodeOnline { id } => {
                if let Some(a) = offline_since.remove(id.as_str()) {
                    *offline_min.entry(id).or_default() += minutes(a, t);
                }
            }
            Event::Restart { .. } => restarts += 1,
            Event::Update { from, to, ok } => updates.push(format!(
                "{from} → {to}{}",
                if *ok { "" } else { " (failed, rolled back)" }
            )),
            Event::Trigger { .. } => triggers += 1,
            Event::Limiter { node_id, port, sec } => {
                *limiter.entry((node_id, *port)).or_default() += f64::from(*sec)
            }
            Event::SyncSample {
                node_id,
                offset_error_ms,
                timeline_error_ms,
            } => {
                let v = timeline_error_ms
                    .map(f64::abs)
                    .unwrap_or(0.0)
                    .max(offset_error_ms.abs());
                if v.is_finite() {
                    sync.entry(node_id)
                        .or_default()
                        .push((t.timestamp_millis(), v));
                }
            }
            Event::Metric {
                node_id,
                name,
                value,
            } if value.is_finite() => {
                let node = node_id.as_deref().unwrap_or(ctx.self_id);
                match name.as_str() {
                    metric::TEMP_C => temps
                        .entry(node)
                        .or_default()
                        .push((t.timestamp_millis(), *value)),
                    metric::VOLTS => {
                        let v = volts.entry(node).or_insert(*value);
                        *v = v.min(*value);
                    }
                    metric::DISK_FREE_PCT if node == ctx.self_id => disk = Some(*value),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    for (id, (a, n)) in open {
        close(&mut shows, id, n, a, end.max(a));
    }
    shows.sort_by(|a, b| a.started_at.cmp(&b.started_at));
    for (id, a) in offline_since {
        *offline_min.entry(id).or_default() += minutes(a, end.max(a));
    }
    // Failing health checks at the end of the night.
    if let Some(checks) = last_health.and_then(|c| c.as_array()) {
        for c in checks {
            let status = c.get("status").and_then(|s| s.as_str()).unwrap_or("");
            if status == "fail" || status == "warn" {
                let id = c.get("id").and_then(|s| s.as_str()).unwrap_or("check");
                let label = c.get("label").and_then(|s| s.as_str()).unwrap_or(id);
                let detail = c.get("detail").and_then(|s| s.as_str()).unwrap_or("");
                let msg = if detail.is_empty() {
                    label.to_string()
                } else {
                    format!("{label}: {detail}")
                };
                add_problem(
                    if status == "fail" { "error" } else { "warn" },
                    &format!("health:{id}"),
                    &msg,
                );
            }
        }
    }

    // Controllers: every adopted node, plus any that left traces.
    let mut node_ids: Vec<String> = show.nodes.iter().map(|n| n.id.clone()).collect();
    for id in temps
        .keys()
        .chain(sync.keys())
        .chain(offline_min.keys())
        .chain(volts.keys())
    {
        if !node_ids.iter().any(|n| n == id) {
            node_ids.push(id.to_string());
        }
    }
    let mut nodes = vec![];
    let mut series = ReportSeries::default();
    let bucket = |pts: &[(i64, f64)], agg: fn(&[f64]) -> f64| -> Vec<[f64; 2]> {
        let step = SERIES_BUCKET_MIN * 60_000;
        let mut b: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
        for (t, v) in pts {
            b.entry(t - t.rem_euclid(step)).or_default().push(*v);
        }
        b.into_iter()
            .map(|(t, vs)| [t as f64, round2(agg(&vs))])
            .collect()
    };
    fn max_of(v: &[f64]) -> f64 {
        v.iter().copied().fold(f64::MIN, f64::max)
    }
    fn p95_of(v: &[f64]) -> f64 {
        let mut s = v.to_vec();
        s.sort_by(|a, b| a.total_cmp(b));
        percentile(&s, 0.95).unwrap_or(0.0)
    }
    for id in &node_ids {
        let t = temps.get(id.as_str());
        let mut s: Vec<f64> = sync
            .get(id.as_str())
            .map(|v| v.iter().map(|x| x.1).collect())
            .unwrap_or_default();
        s.sort_by(|a, b| a.total_cmp(b));
        let name = node_name(show, id);
        nodes.push(NodeNight {
            node_id: id.clone(),
            name: name.clone(),
            temp_min_c: t.map(|v| round1(v.iter().map(|x| x.1).fold(f64::MAX, f64::min))),
            temp_max_c: t.map(|v| round1(v.iter().map(|x| x.1).fold(f64::MIN, f64::max))),
            volts_min: volts.get(id.as_str()).map(|v| round2(*v)),
            offline_min: round1(offline_min.get(id.as_str()).copied().unwrap_or(0.0)),
            sync_p50_ms: percentile(&s, 0.5).map(round2),
            sync_p95_ms: percentile(&s, 0.95).map(round2),
        });
        if let Some(t) = t {
            series.temp_c.push(Series {
                node_id: id.clone(),
                name: name.clone(),
                points: bucket(t, max_of),
            });
        }
        if let Some(v) = sync.get(id.as_str()) {
            series.sync_ms.push(Series {
                node_id: id.clone(),
                name,
                points: bucket(v, p95_of),
            });
        }
    }

    let mut top: Vec<TopRequest> = requests
        .into_iter()
        .map(|(id, (count, name))| TopRequest {
            sequence_id: id.to_string(),
            name: if name.is_empty() {
                show.sequence(id)
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| id.to_string())
            } else {
                name.to_string()
            },
            count,
        })
        .collect();
    top.sort_by(|a, b| b.count.cmp(&a.count).then(a.name.cmp(&b.name)));
    top.truncate(5);

    let suspect_pixels: Vec<SuspectPixels> = show
        .props
        .iter()
        .filter(|p| !p.suspect_pixels.is_empty())
        .filter(|p| !super::profiles::disabled_prop_ids(show).contains(&p.id))
        .map(|p| SuspectPixels {
            prop_id: p.id.clone(),
            name: p.name.clone(),
            pixels: p.suspect_pixels.clone(),
        })
        .collect();

    let problems: Vec<Problem> = problems.into_values().collect();
    let limiter: Vec<LimiterUse> = limiter
        .into_iter()
        .map(|((n, port), s)| LimiterUse {
            node_id: n.to_string(),
            port,
            seconds: round1(s),
        })
        .collect();
    let disk_free_pct = disk.or(ctx.disk_free_pct).map(round1);

    // Status.
    let errors = problems.iter().any(|p| p.level == "error");
    let offline = nodes.iter().any(|n| n.offline_min > OFFLINE_FAIL_MIN);
    let hot = nodes
        .iter()
        .any(|n| n.temp_max_c.is_some_and(|t| t > f64::from(rules.temp_c)));
    let low_volts = nodes
        .iter()
        .any(|n| n.volts_min.is_some_and(|v| v < f64::from(rules.voltage_min)));
    let status: Status = if errors || offline {
        "fail"
    } else if !problems.is_empty()
        || hot
        || low_volts
        || !limiter.is_empty()
        || !suspect_pixels.is_empty()
        || disk_free_pct.is_some_and(|d| d < DISK_WARN_PCT)
        || ctx.backup_age_days.is_some_and(|d| d > BACKUP_WARN_DAYS)
        || nodes.iter().any(|n| n.offline_min > 0.0)
    {
        "warn"
    } else {
        "ok"
    };
    let n_problems: u32 = problems.iter().map(|p| p.count).sum::<u32>()
        + nodes.iter().filter(|n| n.offline_min > 0.0).count() as u32;
    let headline = if shows.is_empty() && items_played == 0 {
        format!(
            "No show tonight, {}",
            plural(n_problems, "problem", "problems")
        )
    } else {
        format!(
            "{}, {}, {}, {}",
            plural(shows.len() as u32, "show", "shows"),
            plural(items_played, "song", "songs"),
            plural(n_requests, "request", "requests"),
            plural(n_problems, "problem", "problems")
        )
    };
    NightReport {
        date: ctx.date.format("%Y-%m-%d").to_string(),
        status: status.into(),
        headline,
        runtime_min: shows.iter().map(|s| s.runtime_min).sum(),
        shows,
        items_played,
        requests: n_requests,
        top_requests: top,
        problems,
        nodes,
        limiter,
        suspect_pixels,
        disk_free_pct,
        updates,
        backup_age_days: ctx.backup_age_days,
        generated_at: ctx.generated_at.to_rfc3339(),
        window: ReportWindow {
            from: ctx.from.to_rfc3339(),
            to: end.to_rfc3339(),
        },
        games,
        game_minutes: round1(game_s as f64 / 60.0),
        triggers,
        restarts,
        season: super::profiles::active(show).map(super::profiles::label),
        series,
        delivery: vec![],
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c => o.push(c),
        }
    }
    o
}

fn status_word(status: &str) -> (&'static str, &'static str, &'static str) {
    // (emoji, label, colour)
    match status {
        "fail" => ("❌", "Needs attention", "#c62828"),
        "warn" => ("⚠️", "Mostly fine", "#b26a00"),
        _ => ("✅", "All good", "#2e7d32"),
    }
}

/// The date as people say it: "Tuesday, Dec 1".
fn pretty_date(date: &str) -> String {
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map(|d| d.format("%A, %b %-d").to_string())
        .unwrap_or_else(|_| date.to_string())
}

/// Short push text: numbers and the worst problems, no addresses.
pub fn push_text(r: &NightReport) -> (String, String) {
    let (emoji, word, _) = status_word(&r.status);
    let title = format!("{emoji} {word}: {}", pretty_date(&r.date));
    let mut body = r.headline.clone();
    let hottest = r
        .nodes
        .iter()
        .filter_map(|n| n.temp_max_c.map(|t| (t, &n.name)))
        .max_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((t, name)) = hottest {
        body.push_str(&format!(". {name} peaked at {t:.0} °C"));
    }
    for p in r.problems.iter().take(2) {
        body.push_str(&format!(". {}", p.message));
    }
    for s in r.suspect_pixels.iter().take(1) {
        body.push_str(&format!(
            ". {} pixel {} suspected dead",
            s.name,
            s.pixels.first().map(|p| p + 1).unwrap_or(0)
        ));
    }
    if let Some(d) = r.disk_free_pct {
        body.push_str(&format!(". Disk {d:.0} % free"));
    }
    body.push('.');
    (title, body)
}

/// Plain-text email body.
pub fn render_text(r: &NightReport, show_name: &str, link: Option<&str>) -> String {
    let (_, word, _) = status_word(&r.status);
    let mut t = format!(
        "{show_name}: {}\n{word}. {}.\n\n",
        pretty_date(&r.date),
        r.headline
    );
    if !r.shows.is_empty() {
        t.push_str("Shows\n");
        for s in &r.shows {
            t.push_str(&format!("  {} ({} min)\n", s.name, s.runtime_min));
        }
        t.push('\n');
    }
    if !r.problems.is_empty() {
        t.push_str("Problems\n");
        for p in &r.problems {
            t.push_str(&format!("  {} ×{}: {}\n", p.level, p.count, p.message));
        }
        t.push('\n');
    }
    if !r.top_requests.is_empty() {
        t.push_str("Most requested\n");
        for q in &r.top_requests {
            t.push_str(&format!("  {} ×{}\n", q.name, q.count));
        }
        t.push('\n');
    }
    if !r.nodes.is_empty() {
        t.push_str("Controllers\n");
        for n in &r.nodes {
            let mut parts = vec![];
            if let (Some(a), Some(b)) = (n.temp_min_c, n.temp_max_c) {
                parts.push(format!("{a:.0}–{b:.0} °C"));
            }
            if let Some(v) = n.volts_min {
                parts.push(format!("min {v:.1} V"));
            }
            if let Some(p) = n.sync_p95_ms {
                parts.push(format!("sync p95 {p:.1} ms"));
            }
            if n.offline_min > 0.0 {
                parts.push(format!("offline {:.0} min", n.offline_min));
            }
            t.push_str(&format!("  {}: {}\n", n.name, parts.join(", ")));
        }
        t.push('\n');
    }
    for s in &r.suspect_pixels {
        let px: Vec<String> = s.pixels.iter().take(10).map(|p| (p + 1).to_string()).collect();
        t.push_str(&format!("Suspect pixels on {}: {}\n", s.name, px.join(", ")));
    }
    if let Some(d) = r.disk_free_pct {
        t.push_str(&format!("Disk: {d:.0} % free\n"));
    }
    if let Some(b) = r.backup_age_days {
        t.push_str(&format!("Newest backup: {b} days old\n"));
    }
    for u in &r.updates {
        t.push_str(&format!("Updated: {u}\n"));
    }
    if let Some(l) = link {
        t.push_str(&format!("\nFull report: {l}\n"));
    }
    t
}

/// HTML email body (inline styles only; email clients strip `<style>`).
pub fn render_html(r: &NightReport, show_name: &str, link: Option<&str>) -> String {
    let (emoji, word, color) = status_word(&r.status);
    let mut h = String::new();
    let cell = "padding:6px 10px;border-bottom:1px solid #eee;font-size:14px;";
    let head = "padding:6px 10px;border-bottom:2px solid #ddd;font-size:12px;color:#666;text-align:left;text-transform:uppercase;letter-spacing:.04em;";
    let section = |h: &mut String, title: &str| {
        h.push_str(&format!(
            "<h2 style=\"font-size:16px;margin:24px 0 8px;color:#222;\">{}</h2>",
            esc(title)
        ));
    };
    h.push_str("<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"></head>");
    h.push_str("<body style=\"margin:0;padding:0;background:#f4f5f7;font-family:-apple-system,Segoe UI,Roboto,Helvetica,Arial,sans-serif;color:#222;\">");
    h.push_str("<div style=\"max-width:640px;margin:0 auto;padding:16px;\">");
    h.push_str("<div style=\"background:#fff;border-radius:12px;padding:20px 20px 12px;\">");
    h.push_str(&format!(
        "<div style=\"font-size:13px;color:#666;\">{} · nightly report</div>",
        esc(show_name)
    ));
    h.push_str(&format!(
        "<h1 style=\"font-size:22px;margin:4px 0 10px;\">{}</h1>",
        esc(&pretty_date(&r.date))
    ));
    h.push_str(&format!(
        "<div style=\"display:inline-block;background:{color};color:#fff;border-radius:999px;padding:4px 12px;font-size:13px;font-weight:600;\">{emoji} {}</div>",
        esc(word)
    ));
    h.push_str(&format!(
        "<p style=\"font-size:16px;margin:12px 0 4px;\">{}.</p>",
        esc(&r.headline)
    ));
    // Numbers.
    let tiles = [
        ("Shows", r.shows.len().to_string()),
        ("Songs", r.items_played.to_string()),
        ("Requests", r.requests.to_string()),
        (
            "Show time",
            if r.runtime_min >= 60 {
                format!("{}h {:02}m", r.runtime_min / 60, r.runtime_min % 60)
            } else {
                format!("{} min", r.runtime_min)
            },
        ),
    ];
    h.push_str("<table role=\"presentation\" style=\"width:100%;border-collapse:collapse;margin:12px 0;\"><tr>");
    for (k, v) in tiles {
        h.push_str(&format!(
            "<td style=\"padding:8px;text-align:center;background:#f7f8fa;border:4px solid #fff;border-radius:8px;\"><div style=\"font-size:22px;font-weight:700;\">{}</div><div style=\"font-size:12px;color:#666;\">{}</div></td>",
            esc(&v),
            esc(k)
        ));
    }
    h.push_str("</tr></table>");
    if !r.problems.is_empty() {
        section(&mut h, "Problems");
        h.push_str("<table role=\"presentation\" style=\"width:100%;border-collapse:collapse;\">");
        for p in &r.problems {
            let c = if p.level == "error" { "#c62828" } else { "#b26a00" };
            h.push_str(&format!(
                "<tr><td style=\"{cell}width:18px;color:{c};\">●</td><td style=\"{cell}\">{}</td><td style=\"{cell}text-align:right;color:#666;\">×{}</td></tr>",
                esc(&p.message),
                p.count
            ));
        }
        h.push_str("</table>");
    }
    if !r.shows.is_empty() {
        section(&mut h, "Shows");
        h.push_str(&format!("<table role=\"presentation\" style=\"width:100%;border-collapse:collapse;\"><tr><th style=\"{head}\">Show</th><th style=\"{head}\">Started</th><th style=\"{head}text-align:right;\">Minutes</th></tr>"));
        for s in &r.shows {
            let started = DateTime::parse_from_rfc3339(&s.started_at)
                .map(|t| t.format("%-I:%M %p").to_string())
                .unwrap_or_default();
            h.push_str(&format!(
                "<tr><td style=\"{cell}\">{}</td><td style=\"{cell}\">{}</td><td style=\"{cell}text-align:right;\">{}</td></tr>",
                esc(&s.name),
                esc(&started),
                s.runtime_min
            ));
        }
        h.push_str("</table>");
    }
    if !r.top_requests.is_empty() {
        section(&mut h, "Most requested");
        h.push_str("<table role=\"presentation\" style=\"width:100%;border-collapse:collapse;\">");
        for q in &r.top_requests {
            h.push_str(&format!(
                "<tr><td style=\"{cell}\">{}</td><td style=\"{cell}text-align:right;\">{}</td></tr>",
                esc(&q.name),
                q.count
            ));
        }
        h.push_str("</table>");
    }
    if !r.nodes.is_empty() {
        section(&mut h, "Controllers");
        h.push_str(&format!("<table role=\"presentation\" style=\"width:100%;border-collapse:collapse;\"><tr><th style=\"{head}\">Controller</th><th style=\"{head}\">Temp</th><th style=\"{head}\">Sync p95</th><th style=\"{head}\">Offline</th></tr>"));
        for n in &r.nodes {
            let temp = match (n.temp_min_c, n.temp_max_c) {
                (Some(a), Some(b)) => format!("{a:.0}–{b:.0} °C"),
                _ => "—".into(),
            };
            let sync = n
                .sync_p95_ms
                .map(|p| format!("{p:.1} ms"))
                .unwrap_or_else(|| "—".into());
            let off = if n.offline_min > 0.0 {
                format!("{:.0} min", n.offline_min)
            } else {
                "—".into()
            };
            h.push_str(&format!(
                "<tr><td style=\"{cell}\">{}</td><td style=\"{cell}\">{}</td><td style=\"{cell}\">{}</td><td style=\"{cell}\">{}</td></tr>",
                esc(&n.name),
                esc(&temp),
                esc(&sync),
                esc(&off)
            ));
        }
        h.push_str("</table>");
    }
    if !r.suspect_pixels.is_empty() || !r.limiter.is_empty() {
        section(&mut h, "Lights");
        h.push_str("<ul style=\"padding-left:20px;font-size:14px;\">");
        for s in &r.suspect_pixels {
            let px: Vec<String> = s.pixels.iter().take(10).map(|p| (p + 1).to_string()).collect();
            h.push_str(&format!(
                "<li>{}: suspect pixel{} {}</li>",
                esc(&s.name),
                if s.pixels.len() == 1 { "" } else { "s" },
                esc(&px.join(", "))
            ));
        }
        for l in &r.limiter {
            h.push_str(&format!(
                "<li>Power limiter on {} port {}: {:.0} s</li>",
                esc(&r.nodes.iter().find(|n| n.node_id == l.node_id).map(|n| n.name.clone()).unwrap_or_else(|| l.node_id.clone())),
                l.port + 1,
                l.seconds
            ));
        }
        h.push_str("</ul>");
    }
    section(&mut h, "Housekeeping");
    h.push_str("<ul style=\"padding-left:20px;font-size:14px;\">");
    if let Some(d) = r.disk_free_pct {
        h.push_str(&format!("<li>Disk: {d:.0} % free</li>"));
    }
    match r.backup_age_days {
        Some(0) => h.push_str("<li>Newest backup: today</li>"),
        Some(b) => h.push_str(&format!("<li>Newest backup: {b} days old</li>")),
        None => h.push_str("<li>No backups yet</li>"),
    }
    if r.games > 0 {
        h.push_str(&format!(
            "<li>Visitors played {} game{} ({:.0} min)</li>",
            r.games,
            if r.games == 1 { "" } else { "s" },
            r.game_minutes
        ));
    }
    if r.triggers > 0 {
        h.push_str(&format!("<li>Triggers and sensors fired {} times</li>", r.triggers));
    }
    for u in &r.updates {
        h.push_str(&format!("<li>Updated {}</li>", esc(u)));
    }
    if r.restarts > 0 {
        h.push_str(&format!("<li>PixelPlus restarted {} time{}</li>", r.restarts, if r.restarts == 1 { "" } else { "s" }));
    }
    if let Some(s) = &r.season {
        h.push_str(&format!("<li>Season: {}</li>", esc(s)));
    }
    h.push_str("</ul>");
    if let Some(l) = link {
        h.push_str(&format!(
            "<p style=\"margin:20px 0 8px;\"><a href=\"{}\" style=\"display:inline-block;background:#1f6feb;color:#fff;text-decoration:none;border-radius:8px;padding:10px 16px;font-weight:600;\">Open the full report</a></p>",
            esc(l)
        ));
    }
    h.push_str("</div><p style=\"font-size:12px;color:#888;text-align:center;\">Sent by PixelPlus. Change or turn off in Settings → Nightly report.</p></div></body></html>");
    h
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

/// `<data>/reports`.
pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("reports")
}

fn file(dir: &Path, date: NaiveDate) -> PathBuf {
    dir.join(format!("{}.json", date.format("%Y-%m-%d")))
}

pub fn save(dir: &Path, r: &NightReport) -> std::io::Result<()> {
    let date = NaiveDate::parse_from_str(&r.date, "%Y-%m-%d").map_err(std::io::Error::other)?;
    std::fs::create_dir_all(dir)?;
    let path = file(dir, date);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(r).map_err(std::io::Error::other)?)?;
    std::fs::rename(tmp, path)
}

pub fn load(dir: &Path, date: NaiveDate) -> Option<NightReport> {
    serde_json::from_slice(&std::fs::read(file(dir, date)).ok()?).ok()
}

/// Stored reports, newest first.
pub fn list(dir: &Path, limit: usize) -> Vec<NightReport> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut dates: Vec<NaiveDate> = rd
        .flatten()
        .filter_map(|e| {
            let n = e.file_name();
            NaiveDate::parse_from_str(n.to_str()?.strip_suffix(".json")?, "%Y-%m-%d").ok()
        })
        .collect();
    dates.sort_unstable_by(|a, b| b.cmp(a));
    dates
        .into_iter()
        .take(limit)
        .filter_map(|d| load(dir, d))
        .collect()
}

/// Delete reports older than `keep_days` before `today`.
pub fn purge(dir: &Path, today: NaiveDate, keep_days: u32) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let cutoff = today - ChronoDuration::days(i64::from(keep_days.max(1)));
    for e in rd.flatten() {
        let n = e.file_name();
        let Some(d) = n
            .to_str()
            .and_then(|n| n.strip_suffix(".json"))
            .and_then(|n| NaiveDate::parse_from_str(n, "%Y-%m-%d").ok())
        else {
            continue;
        };
        if d < cutoff {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

/// Runtime state of this service (`state.services.reports`).
#[derive(Default)]
pub struct ReportsState {
    /// Night last made automatically (so it runs once).
    last_auto: Mutex<Option<NaiveDate>>,
    /// One report at a time.
    running: tokio::sync::Mutex<()>,
}

fn show_tz(show: &Show) -> Tz {
    super::profiles::show_tz(show)
}

/// Where the report can be opened: the public URL's origin (a tunnel), else
/// `http://<hostname>.local`.
pub fn report_link(show: &Show, date: &str) -> String {
    let base = show
        .settings
        .requests
        .public_url
        .as_deref()
        .and_then(|u| {
            let u = u.trim();
            let rest = u
                .strip_prefix("https://")
                .map(|r| ("https://", r))
                .or_else(|| u.strip_prefix("http://").map(|r| ("http://", r)))?;
            let host = rest.1.split('/').next()?;
            (!host.is_empty()).then(|| format!("{}{host}", rest.0))
        })
        .unwrap_or_else(|| format!("http://{}.local", super::system::hostname()));
    format!("{base}/reports?date={date}")
}

async fn backup_age_days(state: &AppState) -> Option<u32> {
    let newest = super::snapshots::list(state).await.into_iter().next()?;
    let t = DateTime::parse_from_rfc3339(&newest.created_at).ok()?;
    Some((chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_days().max(0) as u32)
}

fn disk_pct(state: &AppState) -> Option<f64> {
    let (free, total) = super::system::disk_space(&state.config.data_dir)?;
    (total > 0).then(|| free as f64 * 100.0 / total as f64)
}

/// Build (and store) the report of night `date` from the journal.
pub async fn generate(state: &AppState, date: NaiveDate) -> NightReport {
    let _one = state.services.reports.running.lock().await;
    state.services.journal.flush().await;
    let show = state.store.get();
    let tz = show_tz(&show);
    let (from, to) = night_window(&tz, date);
    let jdir = super::journal::dir(&state.config.data_dir);
    let recs = tokio::task::spawn_blocking(move || super::journal::read_range(&jdir, from, to, None))
        .await
        .unwrap_or_default();
    let self_id = state.identity().id;
    let ctx = ReportContext {
        date,
        from,
        to,
        generated_at: chrono::Utc::now().with_timezone(&tz).fixed_offset(),
        show: &show,
        self_id: &self_id,
        disk_free_pct: disk_pct(state),
        backup_age_days: backup_age_days(state).await,
    };
    let report = aggregate(&ctx, &recs);
    let rdir = dir(&state.config.data_dir);
    let r = report.clone();
    let keep = show.settings.reports.keep_days;
    let today = chrono::Utc::now().with_timezone(&tz).date_naive();
    let _ = tokio::task::spawn_blocking(move || {
        if let Err(e) = save(&rdir, &r) {
            tracing::warn!("Couldn't save the nightly report: {e}");
        }
        purge(&rdir, today, keep);
    })
    .await;
    report
}

/// Send a report by the channels enabled in `settings.reports`. Returns one
/// line per channel ("Email sent to …", "Push failed: …").
pub async fn send(state: &AppState, report: &NightReport, force: bool) -> Vec<String> {
    let show = state.store.get();
    let rs = &show.settings.reports;
    let alerts = &show.settings.alerts;
    let mut out = vec![];
    if !force && rs.only_when_problems && report.status == "ok" {
        out.push("Not sent: nothing needed attention.".into());
        return out;
    }
    let link = report_link(&show, &report.date);
    let (title, push_body) = push_text(report);
    if rs.email {
        match alerts
            .email
            .as_ref()
            .filter(|e| !e.smtp_host.is_empty() && !e.to.is_empty())
        {
            Some(email) => {
                let subject = format!("[{}] {title}", show.name);
                let text = render_text(report, &show.name, Some(&link));
                let html = render_html(report, &show.name, Some(&link));
                match super::alerts::send_email_html(email, &subject, &text, &html).await {
                    Ok(()) => out.push(format!("Email sent to {}.", email.to)),
                    Err(e) => out.push(format!("Email failed: {e}")),
                }
            }
            None => out.push("Email: not set up (Settings → Alerts).".into()),
        }
    }
    if rs.push {
        match alerts.ntfy.as_ref().filter(|n| !n.topic.is_empty()) {
            Some(ntfy) => {
                let sev = match report.status.as_str() {
                    "fail" => super::alerts::Severity::Warning,
                    _ => super::alerts::Severity::Info,
                };
                match super::alerts::send_ntfy_with(ntfy, &title, &push_body, sev, Some(&link)).await
                {
                    Ok(()) => out.push(format!("Push sent to \"{}\".", ntfy.topic)),
                    Err(e) => out.push(format!("Push failed: {e}")),
                }
            }
            None => out.push("Push: not set up (Settings → Alerts).".into()),
        }
    }
    for l in &out {
        tracing::info!("Nightly report {}: {l}", report.date);
    }
    out
}

/// Make, store and send the report of night `date`.
pub async fn run(state: &AppState, date: NaiveDate, send_it: bool, force: bool) -> NightReport {
    let mut r = generate(state, date).await;
    if send_it {
        r.delivery = send(state, &r, force).await;
        let rdir = dir(&state.config.data_dir);
        let saved = r.clone();
        let _ = tokio::task::spawn_blocking(move || save(&rdir, &saved)).await;
    }
    r
}

/// Samples for the charts (every minute).
fn sample(state: &AppState) {
    let j = &state.services.journal;
    let self_id = state.identity().id;
    let readings = state.services.sensors.latest();
    let temp = readings
        .iter()
        .filter(|r| r.sensor.kind == pixelplus_hw::SensorKind::Temperature)
        .map(|r| r.sensor.value)
        .filter(|v| v.is_finite())
        .fold(None, |a: Option<f64>, v| Some(a.map_or(v, |a| a.max(v))))
        .or_else(|| super::system::soc_temp().map(f64::from));
    if let Some(t) = temp {
        j.record(Event::Metric {
            node_id: Some(self_id.clone()),
            name: metric::TEMP_C.into(),
            value: round1(t),
        });
    }
    let volts = readings
        .iter()
        .filter(|r| r.sensor.kind == pixelplus_hw::SensorKind::Voltage)
        .map(|r| r.sensor.value)
        .filter(|v| v.is_finite() && *v > 1.0)
        .fold(None, |a: Option<f64>, v| Some(a.map_or(v, |a| a.min(v))));
    if let Some(v) = volts {
        j.record(Event::Metric {
            node_id: Some(self_id.clone()),
            name: metric::VOLTS.into(),
            value: round2(v),
        });
    }
    if let Some(d) = disk_pct(state) {
        j.record(Event::Metric {
            node_id: Some(self_id.clone()),
            name: metric::DISK_FREE_PCT.into(),
            value: round1(d),
        });
    }
    if let Some(c) = state.services.cluster.get() {
        for n in c.nodes_status() {
            if n.id == self_id || !n.online || !n.adopted {
                continue;
            }
            if let Some(q) = n.sync {
                j.record(Event::SyncSample {
                    node_id: n.id.clone(),
                    offset_error_ms: round2(q.offset_error_ms),
                    timeline_error_ms: q.timeline_error_ms.map(round2),
                });
            }
        }
    }
}

/// Start the service (called once from `services::start_all`).
pub fn start(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            // Followers report to their leader; only a leader samples and reports.
            let leader = state
                .store
                .get()
                .leader()
                .is_some_and(|l| l.id == state.identity().id);
            if !leader && !state.store.get().nodes.is_empty() {
                continue;
            }
            sample(&state);
            let show = state.store.get();
            let rs = show.settings.reports.clone();
            if !rs.enabled {
                continue;
            }
            let tz = show_tz(&show);
            let now = chrono::Utc::now().with_timezone(&tz);
            // The night that most recently became due (tonight for
            // "afterShow", last night otherwise).
            let tonight = night_of(now);
            let candidates = [tonight, tonight - ChronoDuration::days(1)];
            let Some(date) = candidates.into_iter().find(|d| {
                due_at(&show, &tz, *d, &rs.time).is_some_and(|due| now >= due)
                    && now < night_window(&tz, *d).1.with_timezone(&tz) + ChronoDuration::hours(24)
            }) else {
                continue;
            };
            {
                let last = state.services.reports.last_auto.lock();
                if *last >= Some(date) {
                    continue;
                }
            }
            // Already made and sent (a restart after sending)?
            let rdir = dir(&state.config.data_dir);
            if load(&rdir, date).is_some_and(|r| !r.delivery.is_empty()) {
                *state.services.reports.last_auto.lock() = Some(date);
                continue;
            }
            *state.services.reports.last_auto.lock() = Some(date);
            run(&state, date, true, false).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixelplus_core::model::{Node, Prop};

    fn tz() -> Tz {
        "America/Chicago".parse().unwrap()
    }

    fn at(s: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    fn rec(ts: &str, event: Event) -> Record {
        Record {
            ts: ts.into(),
            event,
        }
    }

    fn show() -> Show {
        let mut s = Show::default();
        s.name = "Chandler Lights".into();
        s.schedule.location.timezone = "America/Chicago".into();
        for (id, name) in [("nmain00001", "Main Controller"), ("ngarage001", "Garage")] {
            s.nodes.push(
                serde_json::from_value::<Node>(serde_json::json!({
                    "id": id, "name": name, "role": if id == "nmain00001" {"leader"} else {"follower"},
                    "board": "difftx", "hostname": name.to_lowercase(), "outputs": []
                }))
                .unwrap(),
            );
        }
        s.props.push(
            serde_json::from_value::<Prop>(serde_json::json!({
                "id": "parch00002", "name": "Arch 2", "kind": "arch", "pixelCount": 50,
                "channelStart": 0, "suspectPixels": [36]
            }))
            .unwrap(),
        );
        s
    }

    /// A synthetic show night: two shows (one crossing midnight), songs,
    /// requests, a game, errors, a controller dropping off, sync samples,
    /// temperatures, limiter use and an update.
    fn journal() -> Vec<Record> {
        let mut v = vec![
            // Before the window (morning): not part of the night.
            rec("2026-12-01T09:00:00-06:00", Event::Error { code: "x".into(), msg: "morning".into() }),
            rec("2026-12-01T16:59:00-06:00", Event::Restart { reason: "start".into() }),
            rec("2026-12-01T17:00:00-06:00", Event::ShowStart { entry_id: "scweeknt01".into(), name: "Weeknights".into() }),
            rec("2026-12-01T17:00:01-06:00", Event::ItemStart { item: "sequence".into(), id: "s1".into(), name: "Wizards in Winter".into(), playlist_id: Some("p".into()) }),
            rec("2026-12-01T17:03:05-06:00", Event::ItemEnd { item: "sequence".into(), id: "s1".into(), name: "Wizards in Winter".into(), dur_ms: 184_000, ended_by: "finished".into() }),
            rec("2026-12-01T17:03:06-06:00", Event::ItemStart { item: "dj".into(), id: "d1".into(), name: "Welcome".into(), playlist_id: None }),
            rec("2026-12-01T17:04:00-06:00", Event::ItemStart { item: "sequence".into(), id: "s2".into(), name: "All I Want".into(), playlist_id: Some("p".into()) }),
            rec("2026-12-01T17:10:00-06:00", Event::Request { sequence_id: "s2".into(), name: "All I Want".into() }),
            rec("2026-12-01T17:12:00-06:00", Event::Request { sequence_id: "s2".into(), name: "All I Want".into() }),
            rec("2026-12-01T17:13:00-06:00", Event::Request { sequence_id: "s1".into(), name: "Wizards in Winter".into() }),
            rec("2026-12-01T17:20:00-06:00", Event::Game { s: 90 }),
            rec("2026-12-01T17:30:00-06:00", Event::Warn { code: "temp".into(), msg: "Garage reached 58 °C".into() }),
            rec("2026-12-01T18:00:00-06:00", Event::NodeOffline { id: "ngarage001".into() }),
            rec("2026-12-01T18:04:30-06:00", Event::NodeOnline { id: "ngarage001".into() }),
            rec("2026-12-01T18:30:00-06:00", Event::Limiter { node_id: "ngarage001".into(), port: 2, sec: 12.5 }),
            rec("2026-12-01T18:31:00-06:00", Event::Limiter { node_id: "ngarage001".into(), port: 2, sec: 2.5 }),
            rec("2026-12-01T19:00:00-06:00", Event::Trigger { id: "t1".into() }),
            rec("2026-12-01T22:00:00-06:00", Event::ShowEnd { entry_id: "scweeknt01".into(), name: "Weeknights".into() }),
            rec("2026-12-01T23:30:00-06:00", Event::ShowStart { entry_id: "late".into(), name: "Late show".into() }),
            rec("2026-12-02T00:30:00-06:00", Event::ShowEnd { entry_id: "late".into(), name: "Late show".into() }),
            rec("2026-12-02T03:00:00-06:00", Event::Update { from: "1.4.0".into(), to: "1.4.1".into(), ok: true }),
            rec("2026-12-02T03:01:00-06:00", Event::Restart { reason: "update".into() }),
            // After the window: next night.
            rec("2026-12-02T12:00:00-06:00", Event::Error { code: "y".into(), msg: "next day".into() }),
        ];
        for i in 0..20 {
            let m = 17 * 60 + i * 10;
            let ts = format!("2026-12-01T{:02}:{:02}:00-06:00", m / 60, m % 60);
            v.push(rec(&ts, Event::Metric { node_id: Some("nmain00001".into()), name: metric::TEMP_C.into(), value: 40.0 + f64::from(i % 5) }));
            v.push(rec(&ts, Event::SyncSample { node_id: "ngarage001".into(), offset_error_ms: 0.2 + f64::from(i) * 0.05, timeline_error_ms: Some(0.1) }));
        }
        v.push(rec("2026-12-01T20:00:00-06:00", Event::Metric { node_id: Some("nmain00001".into()), name: metric::VOLTS.into(), value: 11.8 }));
        v.push(rec("2026-12-01T20:00:00-06:00", Event::Metric { node_id: Some("nmain00001".into()), name: metric::DISK_FREE_PCT.into(), value: 71.04 }));
        v
    }

    fn ctx<'a>(show: &'a Show) -> ReportContext<'a> {
        let date = NaiveDate::from_ymd_opt(2026, 12, 1).unwrap();
        let (from, to) = night_window(&tz(), date);
        ReportContext {
            date,
            from,
            to,
            generated_at: at("2026-12-02T07:00:00-06:00"),
            show,
            self_id: "nmain00001",
            disk_free_pct: Some(50.0),
            backup_age_days: Some(1),
        }
    }

    fn golden_path(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/reports")
            .join(name)
    }

    /// Compare with a golden file; `UPDATE_GOLDEN=1` rewrites it.
    fn golden(name: &str, actual: &str) {
        let p = golden_path(name);
        if std::env::var_os("UPDATE_GOLDEN").is_some() || !p.exists() {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, actual).unwrap();
        }
        let expected = std::fs::read_to_string(&p).unwrap();
        assert_eq!(
            expected, actual,
            "{} differs (UPDATE_GOLDEN=1 to accept)",
            p.display()
        );
    }

    #[test]
    fn aggregates_a_synthetic_night_golden() {
        let s = show();
        let mut recs = journal();
        recs.reverse(); // order must not matter
        let r = aggregate(&ctx(&s), &recs);
        assert_eq!(r.date, "2026-12-01");
        assert_eq!(r.status, "warn", "{r:#?}");
        assert_eq!(r.shows.len(), 2);
        assert_eq!(r.shows[0].runtime_min, 300);
        assert_eq!(r.shows[1].runtime_min, 60, "crosses midnight");
        assert_eq!(r.runtime_min, 360);
        assert_eq!(r.items_played, 2, "sequences only");
        assert_eq!(r.requests, 3);
        assert_eq!(r.top_requests[0].name, "All I Want");
        assert_eq!(r.top_requests[0].count, 2);
        assert_eq!(r.problems.len(), 1);
        assert!(!r.problems.iter().any(|p| p.message == "morning" || p.message == "next day"));
        let garage = r.nodes.iter().find(|n| n.node_id == "ngarage001").unwrap();
        assert_eq!(garage.offline_min, 4.5);
        assert_eq!(garage.sync_p50_ms, Some(0.7));
        assert_eq!(garage.sync_p95_ms, Some(1.1));
        let main = r.nodes.iter().find(|n| n.node_id == "nmain00001").unwrap();
        assert_eq!((main.temp_min_c, main.temp_max_c), (Some(40.0), Some(44.0)));
        assert_eq!(main.volts_min, Some(11.8));
        assert_eq!(r.limiter, vec![LimiterUse { node_id: "ngarage001".into(), port: 2, seconds: 15.0 }]);
        assert_eq!(r.suspect_pixels[0].pixels, vec![36]);
        assert_eq!(r.disk_free_pct, Some(71.0), "journal sample wins over live value");
        assert_eq!(r.updates, vec!["1.4.0 → 1.4.1".to_string()]);
        assert_eq!((r.games, r.game_minutes, r.triggers, r.restarts), (1, 1.5, 1, 2));
        assert_eq!(r.headline, "2 shows, 2 songs, 3 requests, 2 problems");
        assert!(!r.series.temp_c[0].points.is_empty());
        golden("night-2026-12-01.json", &serde_json::to_string_pretty(&r).unwrap());
    }

    #[test]
    fn status_levels() {
        let s = show();
        let c = ctx(&s);
        let mut s2 = s.clone();
        s2.props[0].suspect_pixels.clear();
        let quiet = aggregate(&ReportContext { show: &s2, ..ctx(&s2) }, &[]);
        assert_eq!(quiet.status, "ok");
        assert!(quiet.headline.starts_with("No show tonight"));
        let err = aggregate(
            &c,
            &[rec("2026-12-01T19:00:00-06:00", Event::Error { code: "show".into(), msg: "The show stopped".into() })],
        );
        assert_eq!(err.status, "fail");
        // Offline until the end of the window: counted to the report time.
        let off = aggregate(&c, &[rec("2026-12-02T06:00:00-06:00", Event::NodeOffline { id: "ngarage001".into() })]);
        assert_eq!(off.status, "fail");
        assert_eq!(off.nodes.iter().find(|n| n.node_id == "ngarage001").unwrap().offline_min, 60.0);
        // A failing health check at the end of the night.
        let h = aggregate(
            &ReportContext { show: &s2, ..ctx(&s2) },
            &[rec(
                "2026-12-01T16:00:00-06:00",
                Event::Health { checks: serde_json::json!([{"id":"disk","label":"Storage","status":"warn","detail":"8 % free"},{"id":"ok","label":"x","status":"ok","detail":""}]) },
            )],
        );
        assert_eq!(h.status, "warn");
        assert_eq!(h.problems[0].message, "Storage: 8 % free");
    }

    #[test]
    fn night_windows_across_dst() {
        let tz = tz();
        // Fall back (Nov 1, 2026): the night of Oct 31 has 25 hours.
        let (a, b) = night_window(&tz, NaiveDate::from_ymd_opt(2026, 10, 31).unwrap());
        assert_eq!(a.to_rfc3339(), "2026-10-31T12:00:00-05:00");
        assert_eq!(b.to_rfc3339(), "2026-11-01T12:00:00-06:00");
        assert_eq!((b - a).num_hours(), 25);
        // Spring forward (Mar 8, 2026): 23 hours.
        let (a, b) = night_window(&tz, NaiveDate::from_ymd_opt(2026, 3, 7).unwrap());
        assert_eq!((b - a).num_hours(), 23);
        // Both 01:30s of the repeated hour are in the night.
        let s = show();
        let date = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let (from, to) = night_window(&tz, date);
        let r = aggregate(
            &ReportContext { date, from, to, generated_at: at("2026-11-01T07:00:00-06:00"), ..ctx(&s) },
            &[
                rec("2026-11-01T01:30:00-05:00", Event::Request { sequence_id: "s".into(), name: "A".into() }),
                rec("2026-11-01T01:30:00-06:00", Event::Request { sequence_id: "s".into(), name: "A".into() }),
                rec("2026-10-31T11:59:59-05:00", Event::Request { sequence_id: "s".into(), name: "A".into() }),
                rec("2026-11-01T12:00:00-06:00", Event::Request { sequence_id: "s".into(), name: "A".into() }),
            ],
        );
        assert_eq!(r.requests, 2);
        // Which night a moment belongs to.
        let t = |s: &str| at(s).with_timezone(&tz);
        assert_eq!(night_of(t("2026-12-02T07:00:00-06:00")), NaiveDate::from_ymd_opt(2026, 12, 1).unwrap());
        assert_eq!(night_of(t("2026-12-01T22:15:00-06:00")), NaiveDate::from_ymd_opt(2026, 12, 1).unwrap());
    }

    #[test]
    fn due_times() {
        let tz = tz();
        let mut s = show();
        let d = NaiveDate::from_ymd_opt(2026, 12, 1).unwrap();
        assert_eq!(
            due_at(&s, &tz, d, "07:00").unwrap().fixed_offset().to_rfc3339(),
            "2026-12-02T07:00:00-06:00"
        );
        assert_eq!(
            due_at(&s, &tz, d, "23:00").unwrap().fixed_offset().to_rfc3339(),
            "2026-12-01T23:00:00-06:00"
        );
        assert!(due_at(&s, &tz, d, "7am").is_none());
        // afterShow: 15 min after the last window; none scheduled → next noon.
        assert_eq!(
            due_at(&s, &tz, d, AFTER_SHOW).unwrap().fixed_offset().to_rfc3339(),
            "2026-12-02T12:00:00-06:00"
        );
        s.schedule.enabled = true;
        s.playlists.push(serde_json::from_value(serde_json::json!({"id":"p","name":"P"})).unwrap());
        s.schedule.entries.push(
            serde_json::from_value(serde_json::json!({
                "id":"e","name":"Nightly","enabled":true,"playlistId":"p",
                "days":["mon","tue","wed","thu","fri","sat","sun"],
                "start":{"kind":"clock","time":"17:00"},"end":{"kind":"clock","time":"22:00"}
            }))
            .unwrap(),
        );
        assert_eq!(
            due_at(&s, &tz, d, AFTER_SHOW).unwrap().fixed_offset().to_rfc3339(),
            "2026-12-01T22:15:00-06:00"
        );
    }

    #[test]
    fn renders_email_and_push_golden() {
        let s = show();
        let mut r = aggregate(&ctx(&s), &journal());
        r.problems.push(Problem {
            level: "error".into(),
            code: "x".into(),
            message: "<script>alert(1)</script> & more".into(),
            count: 1,
        });
        let html = render_html(&r, "Chandler <Lights>", Some("http://pixelplus.local/reports?date=2026-12-01"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt; &amp; more"));
        assert!(!html.contains("<script>"));
        assert!(html.contains("Chandler &lt;Lights&gt;"));
        assert!(html.contains("Tuesday, Dec 1"));
        golden("night-2026-12-01.html", &html);
        let text = render_text(&r, "Chandler Lights", Some("http://pixelplus.local/reports?date=2026-12-01"));
        golden("night-2026-12-01.txt", &text);
        let (title, body) = push_text(&r);
        assert_eq!(title, "⚠️ Mostly fine: Tuesday, Dec 1");
        assert!(body.starts_with("2 shows, 2 songs, 3 requests"), "{body}");
        assert!(body.contains("Arch 2 pixel 37 suspected dead"), "{body}");
        assert!(!body.contains("192.168"));
    }

    #[test]
    fn storage_list_and_purge() {
        let d = std::env::temp_dir().join(format!("pp-reports-{}", pixelplus_core::model::new_id()));
        let s = show();
        for day in 1..=5 {
            let date = NaiveDate::from_ymd_opt(2026, 12, day).unwrap();
            let mut c = ctx(&s);
            c.date = date;
            save(&d, &aggregate(&c, &[])).unwrap();
        }
        std::fs::write(d.join("notes.txt"), "x").unwrap();
        let l = list(&d, 3);
        assert_eq!(l.iter().map(|r| r.date.as_str()).collect::<Vec<_>>(), ["2026-12-05", "2026-12-04", "2026-12-03"]);
        purge(&d, NaiveDate::from_ymd_opt(2026, 12, 6).unwrap(), 3);
        assert_eq!(list(&d, 30).len(), 3);
        assert!(d.join("notes.txt").exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn links_never_leak_more_than_the_origin() {
        let mut s = show();
        s.settings.requests.public_url = Some("https://lights.example.com/request?x=1".into());
        assert_eq!(report_link(&s, "2026-12-01"), "https://lights.example.com/reports?date=2026-12-01");
        s.settings.requests.public_url = None;
        assert!(report_link(&s, "2026-12-01").ends_with(".local/reports?date=2026-12-01"));
    }
}
