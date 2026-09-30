//! Library tools (F18, ARCHITECTURE §12.15): tags, smart playlists, play history.
//!
//! | Method & path | |
//! |---|---|
//! | `POST /sequences/tags` | `{ids[], add[], remove[]}` (sequence or media ids) → all sequences |
//! | `GET /library/tags` | `[{name, color?, sequences, media}]` |
//! | `PUT /library/tags/:name` | `{name?, color?}` rename / recolour everywhere |
//! | `DELETE /library/tags/:name` | remove the tag from everything |
//! | `GET /library/history?days=14` | `[{sequenceId, plays, lastPlayed?}]` from the journal |
//! | `GET /playlists/:id/preview?date=YYYY-MM-DD&start=HH:MM&seed=` | tonight's smart expansion |
//! | `POST /library/smart-preview` | `{rules, playlistId?, date?, start?, seed?}` (unsaved rules) |
//!
//! The previews answer `{items, totalMs, notes, startsAt[], start, seed}`.
//!
//! # For the playback engine (WS3)
//!
//! ```ignore
//! // At playlist start and at each repeat pass of a smart playlist:
//! if let Some(items) = crate::api::library::smart_items(&state, &playlist.id) {
//!     // play `items` instead of `playlist.items` (intro / outro unchanged)
//! }
//! ```
//! [`smart_items`] reads the journal (a few small files, blocking): call it
//! from `spawn_blocking` in a hot async path. The seed is
//! `hash(show night, playlist id)`, so what plays equals the preview.

use super::{ApiError, ApiResult};
use crate::services::journal;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::{DateTime, Duration, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use pixelplus_core::model::{PlaylistItem, Sequence, Show, SmartRules, TagDef};
use pixelplus_core::smartlist::{self, Expansion, Play, PlayHistory};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/sequences/tags", post(bulk_tags))
        .route("/library/tags", get(list_tags))
        .route("/library/tags/{name}", put(edit_tag).delete(delete_tag))
        .route("/library/history", get(history))
        .route("/playlists/{id}/preview", get(playlist_preview))
        .route("/library/smart-preview", post(rules_preview))
}

/// Longest history window read from the journal.
pub const MAX_HISTORY_DAYS: u32 = 60;

fn tz_of(show: &Show) -> Tz {
    pixelplus_core::schedule::schedule_timezone(&show.schedule).unwrap_or(chrono_tz::UTC)
}

/// Plays of sequences / media in the last `days` days (journal `itemStart`).
pub fn play_history(state: &AppState, days: u32) -> PlayHistory {
    let show = state.store.get();
    let now = Utc::now().with_timezone(&tz_of(&show)).fixed_offset();
    let from = now - Duration::days(i64::from(days.clamp(1, MAX_HISTORY_DAYS)));
    let types = ["itemStart".to_string()];
    let recs = journal::read_range(
        &journal::dir(&state.config.data_dir),
        from,
        now + Duration::minutes(1),
        Some(&types),
    );
    PlayHistory {
        plays: recs
            .into_iter()
            .filter_map(|r| {
                let at = r.time()?;
                match r.event {
                    journal::Event::ItemStart {
                        item,
                        id,
                        playlist_id,
                        ..
                    } if matches!(item.as_str(), "sequence" | "request" | "media") => Some(Play {
                        id,
                        at,
                        playlist_id,
                    }),
                    _ => None,
                }
            })
            .collect(),
    }
}

/// Expand a stored smart playlist at `now` (None when it isn't smart).
pub fn smart_expansion(
    state: &AppState,
    playlist_id: &str,
    now: DateTime<Tz>,
    seed: Option<u64>,
) -> Option<Expansion> {
    let show = state.store.get();
    let pl = show.playlists.iter().find(|p| p.id == playlist_id)?;
    let rules = pl.smart.as_ref()?;
    let history = play_history(state, rules.no_repeat_nights.saturating_add(1).max(14));
    let seed = seed.unwrap_or_else(|| {
        smartlist::night_seed(smartlist::night_of(now.naive_local()), playlist_id)
    });
    smartlist::expand_playlist(&show, playlist_id, &history, now, seed)
}

/// The items a smart playlist plays now (see the module docs); None when the
/// playlist doesn't exist or isn't smart.
#[allow(dead_code)] // contract for WS3 (engine)
pub fn smart_items(state: &AppState, playlist_id: &str) -> Option<Vec<PlaylistItem>> {
    let tz = tz_of(&state.store.get());
    smart_expansion(state, playlist_id, Utc::now().with_timezone(&tz), None).map(|e| e.items)
}

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BulkTags {
    ids: Vec<String>,
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    remove: Vec<String>,
}

fn apply(tags: &mut Vec<String>, add: &[String], remove: &[String]) {
    tags.retain(|t| !remove.contains(t));
    let mut all = tags.clone();
    all.extend(add.iter().cloned());
    *tags = smartlist::normalize_tags(&all);
}

async fn bulk_tags(
    State(state): State<AppState>,
    Json(b): Json<BulkTags>,
) -> ApiResult<Json<Vec<Sequence>>> {
    if b.ids.is_empty() {
        return Err(ApiError::bad_request("Pick at least one song."));
    }
    let add = smartlist::normalize_tags(&b.add);
    let remove = smartlist::normalize_tags(&b.remove);
    if add.is_empty() && remove.is_empty() {
        return Err(ApiError::bad_request("Type a tag to add or remove."));
    }
    let (seqs, _) = state
        .store
        .update(move |show| {
            let mut hit = 0;
            for s in show.sequences.iter_mut().filter(|s| b.ids.contains(&s.id)) {
                apply(&mut s.tags, &add, &remove);
                hit += 1;
            }
            for m in show.media.iter_mut().filter(|m| b.ids.contains(&m.id)) {
                apply(&mut m.tags, &add, &remove);
                hit += 1;
            }
            if hit == 0 {
                return Err(ApiError::not_found("Those songs"));
            }
            Ok(show.sequences.clone())
        })
        .await?;
    Ok(Json(seqs))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TagInfo {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    color: Option<String>,
    sequences: usize,
    media: usize,
}

async fn list_tags(State(state): State<AppState>) -> Json<Vec<TagInfo>> {
    let show = state.store.get();
    let mut m: HashMap<String, (usize, usize)> = HashMap::new();
    for s in &show.sequences {
        for t in &s.tags {
            m.entry(t.clone()).or_default().0 += 1;
        }
    }
    for x in &show.media {
        for t in &x.tags {
            m.entry(t.clone()).or_default().1 += 1;
        }
    }
    for d in &show.tag_defs {
        m.entry(d.name.clone()).or_default();
    }
    let mut v: Vec<TagInfo> = m
        .into_iter()
        .map(|(name, (s, md))| TagInfo {
            color: show
                .tag_defs
                .iter()
                .find(|d| d.name == name)
                .and_then(|d| d.color.clone()),
            name,
            sequences: s,
            media: md,
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    Json(v)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EditTag {
    #[serde(default)]
    name: Option<String>,
    /// "#rrggbb", or null / "" to clear.
    #[serde(default)]
    color: Option<Option<String>>,
}

fn valid_color(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit())
}

async fn edit_tag(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(b): Json<EditTag>,
) -> ApiResult<Json<Value>> {
    let old = smartlist::normalize_tags(&[name])
        .pop()
        .ok_or_else(|| ApiError::bad_request("Which tag?"))?;
    let new = match &b.name {
        Some(n) => smartlist::normalize_tags(std::slice::from_ref(n))
            .pop()
            .ok_or_else(|| ApiError::bad_request("Give the tag a name."))?,
        None => old.clone(),
    };
    let color = match b.color {
        Some(Some(c)) if !c.is_empty() => {
            if !valid_color(&c) {
                return Err(ApiError::bad_request("Colors look like #ff8800."));
            }
            Some(Some(c.to_ascii_lowercase()))
        }
        Some(_) => Some(None),
        None => None,
    };
    state
        .store
        .update(move |show| {
            let rename = |tags: &mut Vec<String>| {
                if let Some(i) = tags.iter().position(|t| *t == old) {
                    tags[i] = new.clone();
                    *tags = smartlist::normalize_tags(tags);
                }
            };
            show.sequences.iter_mut().for_each(|s| rename(&mut s.tags));
            show.media.iter_mut().for_each(|m| rename(&mut m.tags));
            for p in &mut show.playlists {
                if let Some(r) = &mut p.smart {
                    for list in [&mut r.include_tags, &mut r.exclude_tags] {
                        rename(list);
                    }
                    for tr in &mut r.time_rules {
                        rename(&mut tr.require_tags);
                    }
                }
            }
            let prev = show.tag_defs.iter().position(|d| d.name == old);
            let mut def = prev.map(|i| show.tag_defs.remove(i)).unwrap_or(TagDef {
                name: new.clone(),
                color: None,
            });
            def.name = new.clone();
            if let Some(c) = color {
                def.color = c;
            }
            show.tag_defs.retain(|d| d.name != new);
            if def.color.is_some() {
                show.tag_defs.push(def);
            }
            show.tag_defs.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(())
        })
        .await?;
    Ok(Json(json!({ "ok": true })))
}

async fn delete_tag(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    let tag = smartlist::normalize_tags(&[name])
        .pop()
        .ok_or_else(|| ApiError::bad_request("Which tag?"))?;
    state
        .store
        .update(move |show| {
            show.sequences
                .iter_mut()
                .for_each(|s| s.tags.retain(|t| *t != tag));
            show.media
                .iter_mut()
                .for_each(|m| m.tags.retain(|t| *t != tag));
            show.tag_defs.retain(|d| d.name != tag);
            Ok(())
        })
        .await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// History and previews
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct HistoryQuery {
    #[serde(default)]
    days: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HistoryRow {
    sequence_id: String,
    plays: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_played: Option<String>,
}

async fn history(
    State(state): State<AppState>,
    Query(q): Query<HistoryQuery>,
) -> ApiResult<Json<Vec<HistoryRow>>> {
    let days = q.days.unwrap_or(14).clamp(1, MAX_HISTORY_DAYS);
    let s = state.clone();
    let h = tokio::task::spawn_blocking(move || play_history(&s, days))
        .await
        .map_err(ApiError::internal)?;
    let show = state.store.get();
    let rows = show
        .sequences
        .iter()
        .map(|seq| {
            let plays: Vec<&Play> = h.plays.iter().filter(|p| p.id == seq.id).collect();
            HistoryRow {
                sequence_id: seq.id.clone(),
                plays: plays.len() as u32,
                last_played: plays.iter().map(|p| p.at).max().map(|t| t.to_rfc3339()),
            }
        })
        .collect();
    Ok(Json(rows))
}

#[derive(Debug, Default, Deserialize)]
struct PreviewQuery {
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    seed: Option<u64>,
}

/// The instant a preview is for: `date` + `start` (local), else the first
/// scheduled start of `playlist_id` that day, else now / 6 pm.
fn preview_time(
    show: &Show,
    q: &PreviewQuery,
    playlist_id: Option<&str>,
) -> ApiResult<DateTime<Tz>> {
    let tz = tz_of(show);
    let now = Utc::now().with_timezone(&tz);
    let date = match q.date.as_deref().filter(|d| !d.is_empty()) {
        Some(d) => Some(
            NaiveDate::parse_from_str(d, "%Y-%m-%d")
                .map_err(|_| ApiError::bad_request("Dates look like 2026-12-24."))?,
        ),
        None => None,
    };
    if let Some(s) = q.start.as_deref().filter(|s| !s.is_empty()) {
        let t = pixelplus_core::schedule::parse_clock(s)
            .ok_or_else(|| ApiError::bad_request("Times look like 18:30."))?;
        let d = date.unwrap_or_else(|| now.date_naive());
        return tz
            .from_local_datetime(&d.and_time(t))
            .earliest()
            .ok_or_else(|| ApiError::bad_request("That time doesn't exist that day."));
    }
    let Some(d) = date else { return Ok(now) };
    let day_start = tz
        .from_local_datetime(&d.and_time(NaiveTime::MIN))
        .earliest()
        .unwrap_or(now);
    if let Some(pid) = playlist_id {
        if let Some(o) = pixelplus_core::schedule::occurrences(&show.schedule, day_start, 1)
            .into_iter()
            .find(|o| o.playlist_id == pid && o.date == d)
        {
            return Ok(o.start);
        }
    }
    Ok(tz
        .from_local_datetime(
            &d.and_time(NaiveTime::from_hms_opt(18, 0, 0).unwrap_or(NaiveTime::MIN)),
        )
        .earliest()
        .unwrap_or(now))
}

fn preview_json(show: &Show, e: Expansion, start: DateTime<Tz>, seed: u64) -> Value {
    let mut t = start;
    let starts: Vec<String> = e
        .items
        .iter()
        .map(|i| {
            let at = t.to_rfc3339();
            t += Duration::milliseconds(smartlist::item_duration_ms(show, i) as i64);
            at
        })
        .collect();
    json!({
        "items": e.items,
        "totalMs": e.total_ms,
        "notes": e.notes,
        "startsAt": starts,
        "start": start.to_rfc3339(),
        "seed": seed,
    })
}

async fn playlist_preview(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<PreviewQuery>,
) -> ApiResult<Json<Value>> {
    let show = state.store.get();
    let pl = show
        .playlists
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| ApiError::not_found("That playlist"))?;
    let start = preview_time(&show, &q, Some(&id))?;
    if pl.smart.is_none() {
        let e = Expansion {
            total_ms: pl
                .items
                .iter()
                .map(|i| smartlist::item_duration_ms(&show, i))
                .sum(),
            items: pl.items.clone(),
            notes: vec!["This playlist isn't smart; these are its songs.".into()],
        };
        return Ok(Json(preview_json(&show, e, start, 0)));
    }
    let seed = q
        .seed
        .unwrap_or_else(|| smartlist::night_seed(smartlist::night_of(start.naive_local()), &id));
    let s = state.clone();
    let e = tokio::task::spawn_blocking(move || smart_expansion(&s, &id, start, Some(seed)))
        .await
        .map_err(ApiError::internal)?
        .unwrap_or_default();
    Ok(Json(preview_json(&show, e, start, seed)))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RulesPreview {
    rules: SmartRules,
    #[serde(default)]
    playlist_id: Option<String>,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    seed: Option<u64>,
}

async fn rules_preview(
    State(state): State<AppState>,
    Json(b): Json<RulesPreview>,
) -> ApiResult<Json<Value>> {
    let show = state.store.get();
    let q = PreviewQuery {
        date: b.date.clone(),
        start: b.start.clone(),
        seed: b.seed,
    };
    let start = preview_time(&show, &q, b.playlist_id.as_deref())?;
    let pid = b.playlist_id.clone().unwrap_or_else(|| "new".into());
    let seed = b
        .seed
        .unwrap_or_else(|| smartlist::night_seed(smartlist::night_of(start.naive_local()), &pid));
    let s = state.clone();
    let rules = b.rules;
    let days = rules.no_repeat_nights.saturating_add(1).max(14);
    let history = tokio::task::spawn_blocking(move || play_history(&s, days))
        .await
        .map_err(ApiError::internal)?;
    let e = smartlist::expand(&show, &rules, &history, start, seed);
    Ok(Json(preview_json(&show, e, start, seed)))
}

#[cfg(test)]
mod tests {
    use super::super::testkit::TestApp;
    use crate::services::journal::{self, Event, Record};
    use axum::http::StatusCode;
    use chrono::{Duration, Utc};
    use serde_json::json;

    async fn app() -> TestApp {
        let app = TestApp::new();
        app.state
            .store
            .update(|show| {
                show.schedule.location.timezone = "America/Chicago".into();
                for (id, min, tags) in [
                    ("a", 3, vec!["kids", "classic"]),
                    ("b", 4, vec!["kids"]),
                    ("c", 5, vec!["upbeat"]),
                    ("d", 3, vec![]),
                ] {
                    show.sequences.push(
                        serde_json::from_value(json!({"id":id,"name":format!("Song {id}"),"file":format!("sequences/{id}.fseq"),"durationMs":min*60_000,"frameMs":50,"channelCount":3,"hash":"","tags":tags})).unwrap(),
                    );
                }
                show.playlists.push(
                    serde_json::from_value(json!({"id":"pl","name":"Tonight","items":[],"smart":{"includeTags":["kids","upbeat"],"order":"leastRecent","targetDurationMs":600000}})).unwrap(),
                );
                show.playlists.push(serde_json::from_value(json!({"id":"plain","name":"Plain","items":[{"type":"sequence","id":"i1","sequenceId":"d"}]})).unwrap());
                Ok(())
            })
            .await
            .unwrap();
        app
    }

    #[tokio::test]
    async fn bulk_tags_and_tag_management() {
        let app = app().await;
        let (st, seqs) = app
            .json(
                "POST",
                "/sequences/tags",
                Some(json!({"ids":["a","d"],"add":[" Halloween ","KIDS"],"remove":["classic"]})),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{seqs}");
        let a = seqs
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == "a")
            .unwrap();
        assert_eq!(a["tags"], json!(["kids", "halloween"]));
        let (st, _) = app
            .json(
                "POST",
                "/sequences/tags",
                Some(json!({"ids":["zz"],"add":["x"]})),
            )
            .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let (st, _) = app
            .json("POST", "/sequences/tags", Some(json!({"ids":["a"]})))
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        // PATCH /sequences/:id accepts tags (normalized).
        let (st, s) = app
            .json(
                "PATCH",
                "/sequences/c",
                Some(json!({"tags":["Upbeat","Rock"]})),
            )
            .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(s["tags"], json!(["upbeat", "rock"]));

        let (_, tags) = app.json("GET", "/library/tags", None).await;
        let kids = tags
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "kids")
            .unwrap();
        assert_eq!(kids["sequences"], 3);
        // Rename + colour: also renames it inside smart rules.
        let (st, _) = app
            .json(
                "PUT",
                "/library/tags/kids",
                Some(json!({"name":"family","color":"#FF8800"})),
            )
            .await;
        assert_eq!(st, StatusCode::OK);
        let show = app.state.store.get();
        assert!(show
            .sequences
            .iter()
            .all(|s| !s.tags.contains(&"kids".to_string())));
        assert_eq!(show.sequence("b").unwrap().tags, ["family"]);
        assert_eq!(
            show.playlists[0].smart.as_ref().unwrap().include_tags,
            ["family", "upbeat"]
        );
        assert_eq!(show.tag_defs[0].color.as_deref(), Some("#ff8800"));
        let (st, _) = app
            .json("PUT", "/library/tags/family", Some(json!({"color":"red"})))
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let (st, _) = app.json("DELETE", "/library/tags/family", None).await;
        assert_eq!(st, StatusCode::OK);
        let show = app.state.store.get();
        assert!(show.sequence("b").unwrap().tags.is_empty());
        assert!(show.tag_defs.is_empty());
    }

    #[tokio::test]
    async fn smart_preview_uses_the_journal_history() {
        let app = app().await;
        // "b" and "c" played last night; "a" never.
        let dir = journal::dir(&app.state.config.data_dir);
        let tz: chrono_tz::Tz = "America/Chicago".parse().unwrap();
        let last_night = (Utc::now().with_timezone(&tz) - Duration::days(1)).fixed_offset();
        let recs: Vec<Record> = ["b", "c"]
            .iter()
            .map(|id| Record {
                ts: last_night.to_rfc3339(),
                event: Event::ItemStart {
                    item: "sequence".into(),
                    id: id.to_string(),
                    name: String::new(),
                    playlist_id: Some("pl".into()),
                },
            })
            .collect();
        journal::append(&dir, &recs).unwrap();

        let (st, h) = app.json("GET", "/library/history?days=7", None).await;
        assert_eq!(st, StatusCode::OK);
        let row = |id: &str| {
            h.as_array()
                .unwrap()
                .iter()
                .find(|r| r["sequenceId"] == id)
                .unwrap()
                .clone()
        };
        assert_eq!(row("b")["plays"], 1);
        assert!(row("b")["lastPlayed"].is_string());
        assert_eq!(row("a")["plays"], 0);

        let (st, p) = app.json("GET", "/playlists/pl/preview", None).await;
        assert_eq!(st, StatusCode::OK, "{p}");
        let first = p["items"][0]["sequenceId"].as_str().unwrap();
        assert_eq!(first, "a", "never played first: {p}");
        assert_eq!(
            p["startsAt"].as_array().unwrap().len(),
            p["items"].as_array().unwrap().len()
        );
        assert!(p["totalMs"].as_u64().unwrap() >= 7 * 60_000);
        // Deterministic for a given date/start; re-roll with a seed.
        let q = "/playlists/pl/preview?date=2026-12-12&start=18:00";
        let (_, p1) = app.json("GET", q, None).await;
        let (_, p2) = app.json("GET", q, None).await;
        assert_eq!(p1, p2);
        assert!(p1["start"]
            .as_str()
            .unwrap()
            .starts_with("2026-12-12T18:00:00-06:00"));
        let (st, _) = app
            .json("GET", "/playlists/pl/preview?date=12/12", None)
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let (st, _) = app.json("GET", "/playlists/nope/preview", None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let (st, plain) = app.json("GET", "/playlists/plain/preview", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(plain["items"][0]["sequenceId"], "d");

        // Unsaved rules.
        let (st, r) = app
            .json(
                "POST",
                "/library/smart-preview",
                Some(json!({"rules":{"includeTags":["kids"],"order":"fixed"},"date":"2026-12-12","start":"18:00"})),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{r}");
        let ids: Vec<&str> = r["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["sequenceId"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["a", "b"]);

        // The engine entry point.
        let items = super::smart_items(&app.state, "pl").unwrap();
        assert!(!items.is_empty());
        assert!(super::smart_items(&app.state, "plain").is_none());
    }
}
