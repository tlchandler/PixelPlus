//! In-process event bus. Everything that the UI should hear about in real time
//! is published here and fanned out to WebSocket clients (see `api::ws`).

use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;

/// A message pushed to WebSocket clients as `{"type": ..., "data": ...}`.
#[derive(Debug, Clone)]
pub enum Event {
    /// JSON message (`status`, `show`, `nodes`, `sensors`, `log`, `toast`, ...).
    Json { kind: &'static str, data: Value },
    /// Binary live-preview frame (see ARCHITECTURE §8.1).
    Preview(bytes::Bytes),
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        EventBus { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    /// Publish a JSON event. Silently dropped when nobody listens.
    pub fn publish<T: Serialize>(&self, kind: &'static str, data: &T) {
        match serde_json::to_value(data) {
            Ok(data) => {
                let _ = self.tx.send(Event::Json { kind, data });
            }
            Err(e) => tracing::error!("failed to serialize {kind} event: {e}"),
        }
    }

    pub fn preview(&self, frame: bytes::Bytes) {
        let _ = self.tx.send(Event::Preview(frame));
    }

    pub fn toast(&self, kind: ToastKind, message: impl Into<String>) {
        self.publish(
            "toast",
            &serde_json::json!({ "kind": kind, "message": message.into() }),
        );
    }

    pub fn has_listeners(&self) -> bool {
        self.tx.receiver_count() > 0
    }
}
