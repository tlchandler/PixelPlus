//! In-memory log ring buffer + tracing layer.
//!
//! `main.rs` installs [`layer()`] next to the fmt layer. The layer keeps the
//! last [`CAPACITY`] log lines (served by `GET /system/logs` when journald is
//! not available) and, once [`attach_events`] has been called, publishes
//! warnings and errors to WebSocket clients as `log` events.

use crate::events::EventBus;
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, OnceLock};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

/// Lines kept in memory.
pub const CAPACITY: usize = 2000;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    /// `debug` | `info` | `warn` | `error`
    pub level: &'static str,
    pub message: String,
    /// RFC 3339 (local time).
    pub time: String,
}

#[derive(Default)]
pub struct LogRing {
    lines: Mutex<VecDeque<LogLine>>,
    events: OnceLock<EventBus>,
}

static RING: OnceLock<Arc<LogRing>> = OnceLock::new();

/// The process-wide log ring.
pub fn ring() -> &'static Arc<LogRing> {
    RING.get_or_init(|| Arc::new(LogRing::default()))
}

/// Tracing layer feeding [`ring()`]. Install it in `main.rs`:
/// `tracing_subscriber::registry().with(filter).with(fmt::layer()).with(services::logs::layer()).init()`.
pub fn layer() -> RingLayer {
    RingLayer {
        ring: ring().clone(),
    }
}

/// Start publishing warnings/errors as `log` WebSocket events.
pub fn attach_events(bus: EventBus) {
    let _ = ring().events.set(bus);
}

impl LogRing {
    pub fn push(&self, line: LogLine) {
        let publish = matches!(line.level, "warn" | "error");
        if publish {
            if let Some(bus) = self.events.get() {
                bus.publish("log", &line);
            }
        }
        let mut lines = self.lines.lock();
        if lines.len() >= CAPACITY {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    /// The most recent `n` lines, oldest first.
    pub fn recent(&self, n: usize) -> Vec<LogLine> {
        let lines = self.lines.lock();
        let skip = lines.len().saturating_sub(n);
        lines.iter().skip(skip).cloned().collect()
    }
}

/// Render lines as plain text (`time  LEVEL  message`).
pub fn format_lines(lines: &[LogLine]) -> String {
    let mut out = String::new();
    for l in lines {
        let _ = writeln!(
            out,
            "{}  {:<5}  {}",
            l.time,
            l.level.to_uppercase(),
            l.message
        );
    }
    out
}

pub struct RingLayer {
    ring: Arc<LogRing>,
}

thread_local! {
    static IN_LAYER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[derive(Default)]
struct MessageVisitor {
    message: String,
    fields: String,
}

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            let _ = write!(self.fields, " {}={}", field.name(), value);
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else if !field.name().starts_with("log.") {
            let _ = write!(self.fields, " {}={:?}", field.name(), value);
        }
    }
}

fn level_name(level: &Level) -> &'static str {
    match *level {
        Level::ERROR => "error",
        Level::WARN => "warn",
        Level::INFO => "info",
        _ => "debug",
    }
}

impl<S: Subscriber> Layer<S> for RingLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let level = *event.metadata().level();
        if level == Level::TRACE {
            return;
        }
        // Guard against re-entrancy (publishing an event must never log).
        if IN_LAYER.with(|f| f.replace(true)) {
            return;
        }
        let mut v = MessageVisitor::default();
        event.record(&mut v);
        let mut message = v.message;
        message.push_str(&v.fields);
        self.ring.push(LogLine {
            level: level_name(&level),
            message,
            time: chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        });
        IN_LAYER.with(|f| f.set(false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_keeps_last_lines_and_formats() {
        let ring = LogRing::default();
        for i in 0..(CAPACITY + 10) {
            ring.push(LogLine {
                level: "info",
                message: format!("line {i}"),
                time: "t".into(),
            });
        }
        let recent = ring.recent(3);
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[2].message, format!("line {}", CAPACITY + 9));
        assert_eq!(ring.recent(usize::MAX).len(), CAPACITY);
        let text = format_lines(&recent[..1]);
        assert!(text.contains("INFO"));
    }

    #[tokio::test]
    async fn warnings_are_published() {
        let ring = LogRing::default();
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let _ = ring.events.set(bus);
        ring.push(LogLine {
            level: "info",
            message: "quiet".into(),
            time: "t".into(),
        });
        ring.push(LogLine {
            level: "warn",
            message: "loud".into(),
            time: "t".into(),
        });
        match rx.recv().await.unwrap() {
            crate::events::Event::Json { kind, data } => {
                assert_eq!(kind, "log");
                assert_eq!(data["message"], "loud");
            }
            _ => panic!("unexpected event"),
        }
    }
}
