//! The show journal (F11, ARCHITECTURE §12.9): an append-only record of what
//! happened, one JSON object per line in `journal/<YYYY-MM-DD>.jsonl` (local
//! date in the show's time zone). The nightly report, play history (F18) and
//! debugging (`GET /journal`) read it.
//!
//! Recording never blocks and never fails: [`Journal::record`] hands the event
//! to a writer task through a bounded channel and drops it (counting the drop)
//! when the channel is full or the journal is not running (unit tests). Files
//! older than [`KEEP_DAYS`] days are deleted daily; a day's file stops growing
//! at [`MAX_DAY_BYTES`] so a runaway loop cannot fill the SD card.
//!
//! ```ignore
//! state.services.journal.record(Event::ItemStart { .. });
//! // or, where only the state is at hand:
//! crate::services::journal::record(&state, Event::Restart { reason: "start".into() });
//! ```

use crate::state::AppState;
use chrono::{DateTime, Duration as ChronoDuration, FixedOffset, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use tokio::sync::{mpsc, oneshot};

/// Journal files are deleted after this many days.
pub const KEEP_DAYS: i64 = 120;
/// A day's file is not written past this size.
pub const MAX_DAY_BYTES: u64 = 16 * 1024 * 1024;
/// Events waiting for the writer; more are dropped.
const QUEUE: usize = 4096;

/// Something that happened. Serialized with its name in `ev`, fields camelCase.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "ev", rename_all = "camelCase")]
pub enum Event {
    /// A playlist item (or single play) started. `item` is the
    /// [`crate::player::ItemRef`] type ("sequence", "dj", "effect", …).
    #[serde(rename_all = "camelCase")]
    ItemStart {
        item: String,
        id: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        playlist_id: Option<String>,
    },
    /// An item ended. `ended_by`: "finished" | "skipped" | "stopped" |
    /// "windowEnd" | "error".
    #[serde(rename_all = "camelCase")]
    ItemEnd {
        item: String,
        id: String,
        name: String,
        dur_ms: u64,
        ended_by: String,
    },
    /// A schedule window started / ended.
    #[serde(rename_all = "camelCase")]
    ShowStart {
        entry_id: String,
        #[serde(default)]
        name: String,
    },
    #[serde(rename_all = "camelCase")]
    ShowEnd {
        entry_id: String,
        #[serde(default)]
        name: String,
    },
    /// A visitor song request was queued.
    #[serde(rename_all = "camelCase")]
    Request {
        sequence_id: String,
        #[serde(default)]
        name: String,
    },
    /// A visitor game ended after `s` seconds.
    Game {
        s: u32,
    },
    Error {
        code: String,
        msg: String,
    },
    Warn {
        code: String,
        msg: String,
    },
    /// Result of a health check run (the check list as sent to the UI).
    Health {
        checks: serde_json::Value,
    },
    NodeOnline {
        id: String,
    },
    NodeOffline {
        id: String,
    },
    /// The daemon started (`reason`: "start", "update", "crash", …).
    Restart {
        reason: String,
    },
    Update {
        from: String,
        to: String,
        ok: bool,
    },
    /// A trigger fired. `via`: "gpio" | "http" | "link" (secret link, F-hooks)
    /// | "sensor" | "mqtt"; `from`: the caller's address for links.
    Trigger {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        via: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ProfileSwitch {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<String>,
        to: String,
    },
    /// The power limiter scaled `port` of `node_id` for `sec` seconds (F12).
    #[serde(rename_all = "camelCase")]
    Limiter {
        node_id: String,
        port: u32,
        sec: f32,
    },
    /// Sync quality sample of a follower (1-min sampler, F11).
    #[serde(rename_all = "camelCase")]
    SyncSample {
        node_id: String,
        offset_error_ms: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeline_error_ms: Option<f64>,
    },
    /// Any other measurement (temperature, voltage, disk, …).
    #[serde(rename_all = "camelCase")]
    Metric {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node_id: Option<String>,
        name: String,
        value: f64,
    },
}

impl Event {
    /// The `ev` name, as written to the file (for `?types=` filters).
    pub fn name(&self) -> &'static str {
        match self {
            Event::ItemStart { .. } => "itemStart",
            Event::ItemEnd { .. } => "itemEnd",
            Event::ShowStart { .. } => "showStart",
            Event::ShowEnd { .. } => "showEnd",
            Event::Request { .. } => "request",
            Event::Game { .. } => "game",
            Event::Error { .. } => "error",
            Event::Warn { .. } => "warn",
            Event::Health { .. } => "health",
            Event::NodeOnline { .. } => "nodeOnline",
            Event::NodeOffline { .. } => "nodeOffline",
            Event::Restart { .. } => "restart",
            Event::Update { .. } => "update",
            Event::Trigger { .. } => "trigger",
            Event::ProfileSwitch { .. } => "profileSwitch",
            Event::Limiter { .. } => "limiter",
            Event::SyncSample { .. } => "syncSample",
            Event::Metric { .. } => "metric",
        }
    }
}

/// One journal line: `{"ts":"2026-12-01T17:30:00.123-06:00","ev":"showStart",…}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Record {
    /// RFC 3339 in the show's time zone.
    pub ts: String,
    #[serde(flatten)]
    pub event: Event,
}

impl Record {
    pub fn time(&self) -> Option<DateTime<FixedOffset>> {
        DateTime::parse_from_rfc3339(&self.ts).ok()
    }
}

enum Msg {
    Rec(Record),
    Flush(oneshot::Sender<()>),
}

/// Handle in [`crate::services::Services::journal`].
#[derive(Default)]
pub struct Journal {
    tx: OnceLock<mpsc::Sender<Msg>>,
    tz: parking_lot::RwLock<Option<chrono_tz::Tz>>,
    dropped: AtomicU64,
}

impl Journal {
    /// Record an event now. Never blocks; drops the event when the journal
    /// is not running or is backed up.
    pub fn record(&self, event: Event) {
        let rec = Record {
            ts: self
                .now()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, false),
            event,
        };
        self.push(rec);
    }

    /// Record an event with an explicit time (tests).
    #[cfg(test)]
    pub fn record_at(&self, at: DateTime<FixedOffset>, event: Event) {
        self.push(Record {
            ts: at.to_rfc3339_opts(chrono::SecondsFormat::Millis, false),
            event,
        });
    }

    fn push(&self, rec: Record) {
        let sent = self
            .tx
            .get()
            .is_some_and(|tx| tx.try_send(Msg::Rec(rec)).is_ok());
        if !sent {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Events dropped so far (not running, queue full).
    #[cfg(test)]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Wait until everything recorded so far is on disk.
    pub async fn flush(&self) {
        let Some(tx) = self.tx.get() else { return };
        let (done, wait) = oneshot::channel();
        if tx.send(Msg::Flush(done)).await.is_ok() {
            let _ = wait.await;
        }
    }

    /// Current time in the show's time zone (UTC until known).
    pub fn now(&self) -> DateTime<FixedOffset> {
        let now = Utc::now();
        match *self.tz.read() {
            Some(tz) => now.with_timezone(&tz).fixed_offset(),
            None => now.fixed_offset(),
        }
    }

    fn set_tz(&self, tz: Option<chrono_tz::Tz>) {
        *self.tz.write() = tz;
    }
}

/// Shorthand for `state.services.journal.record(event)`.
pub fn record(state: &AppState, event: Event) {
    state.services.journal.record(event);
}

/// `<data>/journal`.
pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("journal")
}

fn day_file(dir: &Path, date: NaiveDate) -> PathBuf {
    dir.join(format!("{}.jsonl", date.format("%Y-%m-%d")))
}

/// Start the writer task and the daily cleanup; records a `restart` event.
pub fn start(state: &AppState) {
    let journal = &state.services.journal;
    let (tx, rx) = mpsc::channel(QUEUE);
    if journal.tx.set(tx).is_err() {
        return; // already running
    }
    let tz_of =
        |s: &AppState| pixelplus_core::schedule::schedule_timezone(&s.store.get().schedule).ok();
    journal.set_tz(tz_of(state));
    let dir = dir(&state.config.data_dir);
    tokio::spawn(writer(dir.clone(), rx));
    {
        // Follow time-zone changes; purge old files once a day.
        let state = state.clone();
        tokio::spawn(async move {
            let mut changes = state.store.subscribe();
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(24 * 3600));
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        let d = dir.clone();
                        let today = state.services.journal.now().date_naive();
                        let _ = tokio::task::spawn_blocking(move || purge(&d, today)).await;
                    }
                    r = changes.changed() => {
                        if r.is_err() {
                            break;
                        }
                        state.services.journal.set_tz(tz_of(&state));
                    }
                }
            }
        });
    }
    journal.record(Event::Restart {
        reason: "start".into(),
    });
}

async fn writer(dir: PathBuf, mut rx: mpsc::Receiver<Msg>) {
    let mut warned = false;
    while let Some(first) = rx.recv().await {
        let mut batch = vec![first];
        while batch.len() < 256 {
            match rx.try_recv() {
                Ok(m) => batch.push(m),
                Err(_) => break,
            }
        }
        let mut recs = Vec::new();
        let mut flushes = Vec::new();
        for m in batch {
            match m {
                Msg::Rec(r) => recs.push(r),
                Msg::Flush(done) => flushes.push(done),
            }
        }
        if !recs.is_empty() {
            let d = dir.clone();
            let res = tokio::task::spawn_blocking(move || append(&d, &recs))
                .await
                .unwrap_or_else(|e| Err(std::io::Error::other(e.to_string())));
            match res {
                Ok(()) => warned = false,
                Err(e) if !warned => {
                    warned = true;
                    tracing::warn!("journal: could not write ({e}); events are being lost");
                }
                Err(_) => {}
            }
        }
        for f in flushes {
            let _ = f.send(());
        }
    }
}

/// Append records to their day files.
pub fn append(dir: &Path, recs: &[Record]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut by_day: std::collections::BTreeMap<NaiveDate, Vec<u8>> = Default::default();
    for r in recs {
        let date = r
            .time()
            .map(|t| t.date_naive())
            .unwrap_or_else(|| Utc::now().date_naive());
        let buf = by_day.entry(date).or_default();
        serde_json::to_writer(&mut *buf, r).map_err(std::io::Error::other)?;
        buf.push(b'\n');
    }
    for (date, buf) in by_day {
        let path = day_file(dir, date);
        let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if len + buf.len() as u64 > MAX_DAY_BYTES {
            continue;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&path)?;
        // A power cut can leave the last line unfinished: start on a fresh
        // line so the first new record (often `restart`) isn't glued to it.
        if len > 0 && !ends_with_newline(&mut f)? {
            f.write_all(b"\n")?;
        }
        f.write_all(&buf)?;
        f.flush()?;
    }
    Ok(())
}

fn ends_with_newline(f: &mut std::fs::File) -> std::io::Result<bool> {
    use std::io::{Read, Seek, SeekFrom};
    f.seek(SeekFrom::End(-1))?;
    let mut b = [0u8; 1];
    f.read_exact(&mut b)?;
    Ok(b[0] == b'\n')
}

/// Delete day files older than [`KEEP_DAYS`] before `today`.
pub fn purge(dir: &Path, today: NaiveDate) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let cutoff = today - ChronoDuration::days(KEEP_DAYS);
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(".jsonl")) else {
            continue;
        };
        if let Ok(d) = NaiveDate::parse_from_str(stem, "%Y-%m-%d") {
            if d < cutoff {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Records of one local day, oldest first, optionally only the `types` named
/// (`ev` names). Unreadable lines (e.g. torn by a power cut) are skipped.
pub fn read_day(dir: &Path, date: NaiveDate, types: Option<&[String]>) -> Vec<Record> {
    let Ok(f) = std::fs::File::open(day_file(dir, date)) else {
        return vec![];
    };
    // Only lines that can be of a wanted type are parsed: the minute-by-minute
    // metrics make up most of a season's journal, and the smart playlists
    // read two weeks of it every 30 s (a cheap substring test first).
    let quoted: Option<Vec<Vec<u8>>> =
        types.map(|t| t.iter().map(|t| format!("\"{t}\"").into_bytes()).collect());
    // Split on raw bytes: a line torn by a power cut in the middle of a
    // multi-byte character is skipped, not the end of the day's reading.
    std::io::BufReader::new(f)
        .split(b'\n')
        .map_while(Result::ok)
        .filter(|l| {
            quoted.as_ref().map_or(true, |q| {
                q.iter()
                    .any(|q| l.windows(q.len()).any(|w| w == q.as_slice()))
            })
        })
        .filter_map(|l| serde_json::from_slice::<Record>(&l).ok())
        .filter(|r| types.map_or(true, |t| t.iter().any(|t| t == r.event.name())))
        .collect()
}

/// Records with `from <= ts < to` (e.g. a show night, noon to noon).
pub fn read_range(
    dir: &Path,
    from: DateTime<FixedOffset>,
    to: DateTime<FixedOffset>,
    types: Option<&[String]>,
) -> Vec<Record> {
    let mut out = vec![];
    // Files are by local date; a day either side covers offset changes.
    let mut d = from.date_naive() - ChronoDuration::days(1);
    let last = to.date_naive() + ChronoDuration::days(1);
    while d <= last {
        out.extend(
            read_day(dir, d, types)
                .into_iter()
                .filter(|r| r.time().is_some_and(|t| t >= from && t < to)),
        );
        d += ChronoDuration::days(1);
    }
    out.sort_by_key(|r| r.time());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("pp-journal-{}", pixelplus_core::model::new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
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

    #[test]
    fn records_are_one_camel_case_line_each() {
        let r = rec(
            "2026-12-01T17:30:00.000-06:00",
            Event::ItemEnd {
                item: "sequence".into(),
                id: "s1".into(),
                name: "Song".into(),
                dur_ms: 1000,
                ended_by: "finished".into(),
            },
        );
        let line = serde_json::to_string(&r).unwrap();
        assert_eq!(
            line,
            r#"{"ts":"2026-12-01T17:30:00.000-06:00","ev":"itemEnd","item":"sequence","id":"s1","name":"Song","durMs":1000,"endedBy":"finished"}"#
        );
        assert_eq!(serde_json::from_str::<Record>(&line).unwrap(), r);
        assert_eq!(r.event.name(), "itemEnd");
    }

    #[test]
    fn appends_by_local_day_and_reads_ranges() {
        let d = tmp();
        append(
            &d,
            &[
                rec(
                    "2026-12-01T11:00:00-06:00",
                    Event::Restart {
                        reason: "start".into(),
                    },
                ),
                rec(
                    "2026-12-01T17:30:00-06:00",
                    Event::ShowStart {
                        entry_id: "e".into(),
                        name: "Nightly".into(),
                    },
                ),
                rec(
                    "2026-12-02T01:00:00-06:00",
                    Event::Warn {
                        code: "temp".into(),
                        msg: "hot".into(),
                    },
                ),
                rec(
                    "2026-12-02T13:00:00-06:00",
                    Event::Trigger {
                        id: "t".into(),
                        via: None,
                        from: None,
                    },
                ),
            ],
        )
        .unwrap();
        assert!(d.join("2026-12-01.jsonl").exists() && d.join("2026-12-02.jsonl").exists());
        assert_eq!(
            read_day(&d, NaiveDate::from_ymd_opt(2026, 12, 1).unwrap(), None).len(),
            2
        );
        // Show night noon → noon.
        let night = read_range(
            &d,
            at("2026-12-01T12:00:00-06:00"),
            at("2026-12-02T12:00:00-06:00"),
            None,
        );
        assert_eq!(
            night.iter().map(|r| r.event.name()).collect::<Vec<_>>(),
            ["showStart", "warn"]
        );
        let only = vec!["warn".to_string()];
        let warns = read_range(
            &d,
            at("2026-11-30T00:00:00Z"),
            at("2026-12-03T00:00:00Z"),
            Some(&only),
        );
        assert_eq!(warns.len(), 1);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn torn_lines_are_skipped_and_old_files_purged() {
        let d = tmp();
        std::fs::write(
            d.join("2026-01-01.jsonl"),
            "{\"ts\":\"2026-01-01T10:00:00Z\",\"ev\":\"trigger\",\"id\":\"a\"}\n{\"ts\":\"2026-01-01T1",
        )
        .unwrap();
        // Torn in the middle of a multi-byte character, then appended to
        // after the restart: both complete records are read.
        let torn = b"{\"ts\":\"2026-01-02T10:00:00Z\",\"ev\":\"trigger\",\"id\":\"a\"}\n{\"ts\":\"2026-01-02T10:01:00Z\",\"ev\":\"warn\",\"code\":\"x\",\"msg\":\"\xe2\x80".to_vec();
        std::fs::write(d.join("2026-01-02.jsonl"), &torn).unwrap();
        append(
            &d,
            &[rec(
                "2026-01-02T10:02:00Z",
                Event::Restart {
                    reason: "power".into(),
                },
            )],
        )
        .unwrap();
        let day2 = read_day(&d, NaiveDate::from_ymd_opt(2026, 1, 2).unwrap(), None);
        assert_eq!(day2.len(), 2, "{day2:?}");
        assert!(matches!(day2[1].event, Event::Restart { .. }));
        let only = read_day(
            &d,
            NaiveDate::from_ymd_opt(2026, 1, 2).unwrap(),
            Some(&["restart".to_string()]),
        );
        assert_eq!(only.len(), 1);
        std::fs::write(d.join("2025-01-01.jsonl"), "").unwrap();
        std::fs::write(d.join("notes.txt"), "").unwrap();
        assert_eq!(
            read_day(&d, NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(), None).len(),
            1
        );
        purge(&d, NaiveDate::from_ymd_opt(2026, 3, 1).unwrap());
        assert!(d.join("2026-01-01.jsonl").exists());
        assert!(!d.join("2025-01-01.jsonl").exists());
        assert!(d.join("notes.txt").exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn not_running_drops_without_blocking() {
        let j = Journal::default();
        j.record(Event::Trigger {
            id: "x".into(),
            via: None,
            from: None,
        });
        assert_eq!(j.dropped(), 1);
    }

    #[tokio::test]
    async fn writer_task_persists_and_flushes() {
        let d = tmp();
        let j = Journal::default();
        let (tx, rx) = mpsc::channel(8);
        j.tx.set(tx).ok().unwrap();
        tokio::spawn(writer(d.clone(), rx));
        j.record_at(
            at("2026-12-01T18:00:00-06:00"),
            Event::Request {
                sequence_id: "s".into(),
                name: "Song".into(),
            },
        );
        j.record_at(at("2026-12-01T18:01:00-06:00"), Event::Game { s: 60 });
        j.flush().await;
        let recs = read_day(&d, NaiveDate::from_ymd_opt(2026, 12, 1).unwrap(), None);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[1].event, Event::Game { s: 60 });
        std::fs::remove_dir_all(&d).unwrap();
    }
}
