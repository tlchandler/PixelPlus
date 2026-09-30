//! Visitor song requests.
//!
//! Requests wait here; only the head of the line is handed to the player
//! (`PlayerCmd::Enqueue`, "play this next"). When the player has started and
//! finished it, the next one is sent. That keeps the admin "remove" button
//! meaningful for everything still waiting.

use crate::api::{ApiError, ApiResult};
use crate::player::{PlayerCmd, PlayerStatus};
use crate::state::AppState;
use parking_lot::Mutex;
use pixelplus_core::model::{PlaylistItem, Sequence, Show};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// Per visitor (IP): at most this many requests...
pub const RATE_MAX: usize = 3;
/// ...within this window.
pub const RATE_WINDOW: Duration = Duration::from_secs(10 * 60);
const NAME_MAX: usize = 30;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SongRequest {
    pub id: String,
    pub sequence_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_by: Option<String>,
    pub requested_at: String,
}

#[derive(Debug, Clone, PartialEq)]
enum Handoff {
    /// Sent to the player, not seen playing yet.
    Sent { at: Instant },
    /// Seen playing.
    Playing,
}

#[derive(Default)]
pub struct RequestQueue {
    items: Mutex<VecDeque<SongRequest>>,
    hits: Mutex<HashMap<IpAddr, VecDeque<Instant>>>,
    /// State of the head of the queue relative to the player.
    head: Mutex<Option<(String, Handoff)>>,
}

impl RequestQueue {
    pub fn list(&self) -> Vec<SongRequest> {
        self.items.lock().iter().cloned().collect()
    }

    pub fn remove(&self, id: &str) -> bool {
        let mut items = self.items.lock();
        let before = items.len();
        items.retain(|r| r.id != id);
        let removed = items.len() != before;
        if removed && self.head.lock().as_ref().is_some_and(|(h, _)| h == id) {
            *self.head.lock() = None;
        }
        removed
    }

    /// Per-IP rate limiting. Records the hit when allowed.
    pub fn check_rate(&self, ip: Option<IpAddr>, now: Instant) -> bool {
        let Some(ip) = ip else { return true };
        let mut hits = self.hits.lock();
        hits.retain(|_, v| {
            v.back()
                .is_some_and(|t| now.duration_since(*t) < RATE_WINDOW)
        });
        let q = hits.entry(ip).or_default();
        while q
            .front()
            .is_some_and(|t| now.duration_since(*t) >= RATE_WINDOW)
        {
            q.pop_front();
        }
        if q.len() >= RATE_MAX {
            return false;
        }
        q.push_back(now);
        true
    }

    /// Validate and add a request. Returns its 1-based position.
    pub fn submit(
        &self,
        show: &Show,
        sequence_id: &str,
        name: Option<&str>,
        ip: Option<IpAddr>,
        now: Instant,
    ) -> ApiResult<(SongRequest, usize)> {
        let rs = &show.settings.requests;
        if !rs.enabled {
            return Err(ApiError::new(
                axum::http::StatusCode::FORBIDDEN,
                "requests_closed",
                "Song requests are closed right now.",
            ));
        }
        let seq = requestable(show)
            .into_iter()
            .find(|s| s.id == sequence_id)
            .ok_or_else(|| ApiError::not_found("That song"))?;
        {
            let items = self.items.lock();
            if items.iter().any(|r| r.sequence_id == sequence_id) {
                return Err(ApiError::new(
                    axum::http::StatusCode::CONFLICT,
                    "already_queued",
                    "That song is already in the line-up!",
                ));
            }
            if items.len() >= rs.max_queue.max(1) as usize {
                return Err(ApiError::new(
                    axum::http::StatusCode::TOO_MANY_REQUESTS,
                    "queue_full",
                    "The request line is full. Try again in a few minutes.",
                ));
            }
        }
        if !self.check_rate(ip, now) {
            return Err(ApiError::new(
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "You've requested a few songs already. Give everyone a turn and try again in a few minutes.",
            ));
        }
        let requested_by = name.map(clean_name).filter(|n| !n.is_empty());
        let req = SongRequest {
            id: pixelplus_core::model::new_id(),
            sequence_id: seq.id.clone(),
            name: seq.name.clone(),
            requested_by,
            requested_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        };
        let mut items = self.items.lock();
        // Re-check under the lock (concurrent submissions).
        if items.iter().any(|r| r.sequence_id == sequence_id) {
            return Err(ApiError::new(
                axum::http::StatusCode::CONFLICT,
                "already_queued",
                "That song is already in the line-up!",
            ));
        }
        items.push_back(req.clone());
        Ok((req, items.len()))
    }

    /// Drive the hand-off to the player from a status update. Returns the
    /// request to enqueue now, if any.
    pub fn on_status(&self, status: &PlayerStatus, now: Instant) -> Option<SongRequest> {
        let mut head = self.head.lock();
        let mut items = self.items.lock();
        let playing_id = status
            .item
            .as_ref()
            .filter(|i| i.kind == "request" || i.kind == "sequence")
            .map(|i| i.id.clone());
        if let Some((req_id, h)) = head.clone() {
            let Some(req) = items.iter().find(|r| r.id == req_id).cloned() else {
                *head = None;
                return None;
            };
            let is_playing = playing_id.as_deref() == Some(req.sequence_id.as_str());
            match h {
                Handoff::Sent { .. } if is_playing => *head = Some((req_id, Handoff::Playing)),
                Handoff::Sent { at } if now.duration_since(at) > Duration::from_secs(30 * 60) => {
                    items.retain(|r| r.id != req_id);
                    *head = None;
                }
                Handoff::Playing if !is_playing => {
                    items.retain(|r| r.id != req_id);
                    *head = None;
                }
                _ => return None,
            }
            if head.is_some() {
                return None;
            }
        }
        let next = items.front().cloned()?;
        *head = Some((next.id.clone(), Handoff::Sent { at: now }));
        Some(next)
    }
}

fn clean_name(n: &str) -> String {
    let s: String = n.chars().filter(|c| !c.is_control()).collect();
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(NAME_MAX)
        .collect()
}

/// Sequences visitors may pick: those in the requests playlist, else all.
pub fn requestable(show: &Show) -> Vec<&Sequence> {
    let rs = &show.settings.requests;
    match rs.playlist_id.as_deref().and_then(|id| show.playlist(id)) {
        Some(pl) => {
            let mut v: Vec<&Sequence> = Vec::new();
            for item in pl.intro.iter().chain(&pl.items).chain(&pl.outro) {
                if let PlaylistItem::Sequence { sequence_id, .. } = item {
                    if let Some(s) = show.sequence(sequence_id) {
                        if !v.iter().any(|x| x.id == s.id) {
                            v.push(s);
                        }
                    }
                }
            }
            v
        }
        None => show.sequences.iter().collect(),
    }
}

/// Public view of the request page (no private data).
pub fn public_view(state: &AppState) -> serde_json::Value {
    let show = state.store.get();
    let rs = &show.settings.requests;
    let songs: Vec<_> = requestable(&show)
        .into_iter()
        .map(|s| serde_json::json!({ "sequenceId": s.id, "name": s.name, "durationMs": s.duration_ms }))
        .collect();
    let queue: Vec<_> = state
        .services
        .requests
        .list()
        .into_iter()
        .map(|r| serde_json::json!({ "id": r.id, "sequenceId": r.sequence_id, "name": r.name, "requestedBy": r.requested_by }))
        .collect();
    let now_playing = state.services.player.get().map(|p| p.status()).and_then(|st| {
        let item = st.item.as_ref()?;
        (matches!(st.state, crate::player::PlayerState::Playing) && (item.kind == "sequence" || item.kind == "request"))
            .then(|| serde_json::json!({ "name": item.name, "posMs": st.pos_ms, "durationMs": st.duration_ms }))
    });
    serde_json::json!({
        "title": rs.title,
        "message": rs.message,
        "enabled": rs.enabled,
        "showName": show.name,
        "maxQueue": rs.max_queue,
        "songs": songs,
        "queue": queue,
        "nowPlaying": now_playing,
    })
}

/// Hand requests to the player one at a time.
pub fn start(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let player = loop {
            if let Some(p) = state.services.player.get() {
                break p.clone();
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        };
        let mut rx = player.watch();
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        loop {
            tokio::select! {
                r = rx.changed() => if r.is_err() { break },
                _ = tick.tick() => {},
            }
            let status = rx.borrow().clone();
            if let Some(req) = state.services.requests.on_status(&status, Instant::now()) {
                let cmd = PlayerCmd::Enqueue {
                    sequence_id: req.sequence_id.clone(),
                    name: req.requested_by.clone(),
                };
                if player.send(cmd).await.is_err() {
                    tracing::warn!(
                        "Couldn't hand the song request \"{}\" to the player",
                        req.name
                    );
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::ItemRef;
    use pixelplus_core::model::*;

    pub fn show_with_songs() -> Show {
        let mut s = Show::default();
        s.settings.requests.enabled = true;
        s.settings.requests.max_queue = 3;
        for (id, name) in [
            ("s1", "Jingle"),
            ("s2", "Bells"),
            ("s3", "Rock"),
            ("s4", "Noel"),
        ] {
            s.sequences.push(Sequence {
                id: id.into(),
                name: name.into(),
                file: format!("sequences/{id}.fseq"),
                duration_ms: 1000,
                frame_ms: 50,
                channel_count: 3,
                media_id: None,
                xlights_name: None,
                thumbnail: None,
                hash: String::new(),
            });
        }
        s
    }

    #[test]
    fn submit_rules() {
        let q = RequestQueue::default();
        let s = show_with_songs();
        let ip: Option<IpAddr> = Some("10.0.0.5".parse().unwrap());
        let t = Instant::now();
        let (r, pos) = q.submit(&s, "s1", Some("  Tom\u{7} "), ip, t).unwrap();
        assert_eq!(pos, 1);
        assert_eq!(r.requested_by.as_deref(), Some("Tom"));
        assert_eq!(
            q.submit(&s, "s1", None, None, t).unwrap_err().code,
            "already_queued"
        );
        assert_eq!(
            q.submit(&s, "nope", None, None, t).unwrap_err().code,
            "not_found"
        );
        q.submit(&s, "s2", None, ip, t).unwrap();
        q.submit(&s, "s3", None, ip, t).unwrap();
        assert_eq!(
            q.submit(&s, "s4", None, None, t).unwrap_err().code,
            "queue_full"
        );
        let mut closed = s.clone();
        closed.settings.requests.enabled = false;
        assert_eq!(
            q.submit(&closed, "s4", None, None, t).unwrap_err().code,
            "requests_closed"
        );
    }

    #[test]
    fn rate_limit_per_ip() {
        let q = RequestQueue::default();
        let ip: Option<IpAddr> = Some("10.0.0.9".parse().unwrap());
        let t = Instant::now();
        for _ in 0..RATE_MAX {
            assert!(q.check_rate(ip, t));
        }
        assert!(!q.check_rate(ip, t));
        assert!(q.check_rate(Some("10.0.0.10".parse().unwrap()), t));
        assert!(q.check_rate(ip, t + RATE_WINDOW + Duration::from_secs(1)));
    }

    #[test]
    fn requestable_from_playlist() {
        let mut s = show_with_songs();
        s.playlists.push(Playlist {
            id: "p".into(),
            name: "Req".into(),
            items: vec![PlaylistItem::Sequence {
                id: "i".into(),
                sequence_id: "s2".into(),
            }],
            intro: vec![],
            outro: vec![],
            shuffle: false,
            repeat: false,
            crossfade_ms: 0,
        });
        s.settings.requests.playlist_id = Some("p".into());
        let r = requestable(&s);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id, "s2");
    }

    #[test]
    fn handoff_one_at_a_time() {
        let q = RequestQueue::default();
        let s = show_with_songs();
        let t = Instant::now();
        q.submit(&s, "s1", None, None, t).unwrap();
        q.submit(&s, "s2", None, None, t).unwrap();
        let idle = PlayerStatus::default();
        assert_eq!(q.on_status(&idle, t).unwrap().sequence_id, "s1");
        assert!(q.on_status(&idle, t).is_none());
        let mut playing = PlayerStatus {
            item: Some(ItemRef {
                kind: "request".into(),
                id: "s1".into(),
                name: "Jingle".into(),
            }),
            ..Default::default()
        };
        assert!(q.on_status(&playing, t).is_none());
        assert_eq!(q.list().len(), 2);
        playing.item = Some(ItemRef {
            kind: "sequence".into(),
            id: "zz".into(),
            name: "Other".into(),
        });
        assert_eq!(q.on_status(&playing, t).unwrap().sequence_id, "s2");
        assert_eq!(q.list().len(), 1);
    }
}
