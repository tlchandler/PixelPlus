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

/// The current show version and the latest `status`/`nodes`/`sensors`
/// messages (sent on connect, and again after the client lagged behind).
fn resync_messages(state: &AppState) -> Vec<String> {
    let mut out = vec![
        serde_json::json!({ "type": "show", "data": { "version": state.store.version() } })
            .to_string(),
    ];
    for (kind, data) in state.services.snapshot_for_new_client() {
        out.push(serde_json::json!({ "type": kind, "data": data }).to_string());
    }
    out
}

async fn resync<S>(state: &AppState, tx: &mut S) -> Result<(), S::Error>
where
    S: futures::Sink<Message> + Unpin,
{
    for m in resync_messages(state) {
        tx.send(Message::Text(m.into())).await?;
    }
    Ok(())
}

async fn client(state: AppState, socket: WebSocket) {
    let (mut tx, mut rx) = socket.split();
    let mut events = state.events.subscribe();
    let preview_interval = Arc::new(parking_lot::Mutex::new(None::<std::time::Duration>));
    let mut guard = PreviewGuard { active: false };
    let mut last_preview = std::time::Instant::now() - std::time::Duration::from_secs(1);

    // Greet with the current show version so the client can sync immediately.
    if resync(&state, &mut tx).await.is_err() {
        return;
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
                Err(RecvError::Lagged(_)) => {
                    // A slow client (phone on weak Wi-Fi) missed events, maybe a
                    // `show` change: resend what a new client gets, so it refetches
                    // the show and has current status/nodes/sensors.
                    if resync(&state, &mut tx).await.is_err() { break; }
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resync_carries_the_show_version_and_latest_status() {
        let app = crate::api::testkit::TestApp::new();
        app.state
            .services
            .remember("status", serde_json::json!({ "state": "playing" }));
        app.state
            .store
            .update(|s| {
                s.name = "x".into();
                Ok(())
            })
            .await
            .unwrap();
        let msgs: Vec<serde_json::Value> = resync_messages(&app.state)
            .iter()
            .map(|m| serde_json::from_str(m).unwrap())
            .collect();
        assert_eq!(msgs[0]["type"], "show");
        assert_eq!(msgs[0]["data"]["version"], app.state.store.version());
        assert!(msgs
            .iter()
            .any(|m| m["type"] == "status" && m["data"]["state"] == "playing"));
    }
}
