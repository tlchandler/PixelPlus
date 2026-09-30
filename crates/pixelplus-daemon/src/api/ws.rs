//! `/api/v1/ws`: real-time push channel to the UI (ARCHITECTURE §8.1).

use crate::events::Event;
use crate::state::AppState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;

/// Number of clients currently subscribed to live preview frames. The playback
/// engine only builds preview frames while this is non-zero.
pub static PREVIEW_SUBSCRIBERS: AtomicU32 = AtomicU32::new(0);

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ClientMsg {
    SubscribePreview {
        #[serde(default = "default_fps")]
        fps: u32,
    },
    UnsubscribePreview,
    Ping,
}

fn default_fps() -> u32 {
    20
}

pub async fn handler(
    State(state): State<AppState>,
    peer: super::Peer,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    // Signed-in UI, or the local games sidecar (read-only events).
    if !super::auth::is_authenticated_for(
        &state,
        &headers,
        peer.0,
        &axum::http::Method::GET,
        "/api/v1/ws",
    ) {
        return super::ApiError::unauthorized().into_response();
    }
    // Clients only send small control messages.
    ws.max_message_size(64 * 1024)
        .max_frame_size(64 * 1024)
        .on_upgrade(move |socket| client(state, socket))
}

struct PreviewGuard {
    active: bool,
}

impl PreviewGuard {
    fn set(&mut self, on: bool) {
        if on != self.active {
            if on {
                PREVIEW_SUBSCRIBERS.fetch_add(1, Ordering::Relaxed);
            } else {
                PREVIEW_SUBSCRIBERS.fetch_sub(1, Ordering::Relaxed);
            }
            self.active = on;
        }
    }
}

impl Drop for PreviewGuard {
    fn drop(&mut self) {
        self.set(false);
    }
}

async fn client(state: AppState, socket: WebSocket) {
    let (mut tx, mut rx) = socket.split();
    let mut events = state.events.subscribe();
    let preview_interval = Arc::new(parking_lot::Mutex::new(None::<std::time::Duration>));
    let mut guard = PreviewGuard { active: false };
    let mut last_preview = std::time::Instant::now() - std::time::Duration::from_secs(1);

    // Greet with the current show version so the client can sync immediately.
    let hello = serde_json::json!({ "type": "show", "data": { "version": state.store.version() } });
    if tx.send(Message::Text(hello.to_string().into())).await.is_err() {
        return;
    }
    for (kind, data) in state.services.snapshot_for_new_client() {
        let msg = serde_json::json!({ "type": kind, "data": data });
        if tx.send(Message::Text(msg.to_string().into())).await.is_err() {
            return;
        }
    }

    loop {
        tokio::select! {
            ev = events.recv() => match ev {
                Ok(Event::Json { kind, data }) => {
                    let msg = serde_json::json!({ "type": kind, "data": data });
                    if tx.send(Message::Text(msg.to_string().into())).await.is_err() { break; }
                }
                Ok(Event::Preview(frame)) => {
                    let Some(every) = *preview_interval.lock() else { continue };
                    if last_preview.elapsed() >= every {
                        last_preview = std::time::Instant::now();
                        if tx.send(Message::Binary(frame)).await.is_err() { break; }
                    }
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            },
            msg = rx.next() => match msg {
                Some(Ok(Message::Text(text))) => match serde_json::from_str::<ClientMsg>(&text) {
                    Ok(ClientMsg::SubscribePreview { fps }) => {
                        let fps = fps.clamp(1, 40);
                        *preview_interval.lock() = Some(std::time::Duration::from_millis(1000 / fps as u64 - 2));
                        guard.set(true);
                    }
                    Ok(ClientMsg::UnsubscribePreview) => {
                        *preview_interval.lock() = None;
                        guard.set(false);
                    }
                    Ok(ClientMsg::Ping) => {
                        let _ = tx.send(Message::Text(r#"{"type":"pong","data":null}"#.into())).await;
                    }
                    Err(_) => {}
                },
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            }
        }
    }
}
