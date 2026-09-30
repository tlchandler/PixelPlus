//! Smart playlist expansion (F18, ARCHITECTURE §12.15).
//!
//! A smart playlist ([`SmartRules`] in `Playlist::smart`) has no fixed song
//! list: [`expand`] picks tonight's songs from the library by tag, length,
//! play history, time-of-day rules and rotation. It is a **pure, deterministic
//! function**: the same show, rules, history, time and seed always give the
//! same items, so the "Tonight" preview in the UI equals what plays.
//!
//! ```ignore
//! use pixelplus_core::smartlist::{self, PlayHistory};
//! let seed = smartlist::night_seed(smartlist::night_of(now.naive_local()), &playlist.id);
//! let out = smartlist::expand(&show, rules, &history, now, seed);
//! // out.items: the playlist items to play this pass (pinned first/last and
//! // interleaved items included); out.notes: plain-language explanations.
//! ```
//!
//! The daemon builds [`PlayHistory`] from the journal (`itemStart` events);
//! `api::library::smart_items` does all of that for the engine.
//!
//! Algorithm:
//! 1. Candidates = sequences matching the include tags (any/all), minus the
//!    exclude tags, minus songs longer than `maxItemMs`, minus songs played in
//!    the last `noRepeatNights` show nights (a show night runs noon to noon).
//!    If that leaves fewer than [`MIN_CANDIDATES`], the no-repeat window is
//!    shortened a night at a time (with a note).
//! 2. Order: `leastRecent` (never played first, then longest ago; a small
//!    seeded jitter breaks ties), `shuffle` (seeded), `rotation` (library
//!    order by name, starting after the song this playlist played last, so a
//!    30-song library is cycled through across the week), `fixed` (library
//!    order).
//! 3. Greedy fill in that order, simulating start times from `now`: while a
//!    time rule is in force (before its time), only songs carrying all its
//!    tags are placed. With a target length the fill stops at the song that
//!    brings the total closest to it (the last slot may pick a shorter song).
//! 4. Pinned first / last items are added, and one interleave item (rotating
//!    through the list) after every `interleaveEvery` songs.

use crate::model::{PlaylistItem, Show, SmartOrder, SmartRules, TagMatch};
use crate::schedule::{resolve_time, schedule_timezone};
use chrono::{DateTime, Duration, FixedOffset, NaiveDate, NaiveDateTime, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Below this many candidates the no-repeat window is relaxed.
pub const MIN_CANDIDATES: usize = 3;
/// Longest tag (characters) and most tags per item, see [`normalize_tags`].
pub const MAX_TAG_LEN: usize = 32;
pub const MAX_TAGS: usize = 24;

/// One past play of a sequence (or media item).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Play {
    /// Sequence id (or media id for audio-only items).
    pub id: String,
    pub at: DateTime<FixedOffset>,
    /// The playlist it played in, when it was part of one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playlist_id: Option<String>,
}

/// What played recently (from the journal). Order does not matter.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlayHistory {
    pub plays: Vec<Play>,
}

impl PlayHistory {
    /// Most recent play of `id`.
    pub fn last_played(&self, id: &str) -> Option<DateTime<FixedOffset>> {
        self.plays.iter().filter(|p| p.id == id).map(|p| p.at).max()
    }

    /// Show nights (see [`night_of`]) on which `id` played.
    fn nights_of(&self, id: &str) -> Vec<NaiveDate> {
        self.plays
            .iter()
            .filter(|p| p.id == id)
            .map(|p| night_of(p.at.naive_local()))
            .collect()
    }
}

/// Result of [`expand`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Expansion {
    pub items: Vec<PlaylistItem>,
    /// Sum of the item durations (commands count as 0).
    pub total_ms: u64,
    /// Plain-language notes for the owner ("Only 2 songs are tagged kids …").
    pub notes: Vec<String>,
}

/// The show night a local time belongs to: nights run noon to noon, so a
/// song at 00:30 belongs to the evening before.
pub fn night_of(local: NaiveDateTime) -> NaiveDate {
    (local - Duration::hours(12)).date()
}

/// Deterministic seed for a playlist on a show night (`hash(date, playlistId)`).
pub fn night_seed(night: NaiveDate, playlist_id: &str) -> u64 {
    fnv1a(format!("{}|{playlist_id}", night.format("%Y-%m-%d")).as_bytes())
}

/// Clean up user-entered tags: trimmed, lower case, single spaces, no commas,
/// at most [`MAX_TAG_LEN`] characters, no duplicates (first wins), at most
/// [`MAX_TAGS`].
pub fn normalize_tags(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in tags {
        let t: String = t
            .replace(',', " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_TAG_LEN)
            .collect();
        let t = t.trim().to_string();
        if !t.is_empty() && !out.contains(&t) && out.len() < MAX_TAGS {
            out.push(t);
        }
    }
    out
}

/// Every tag used in the library with how many sequences / media carry it,
/// sorted by name.
pub fn tag_counts(show: &Show) -> Vec<(String, usize)> {
    let mut m: HashMap<&str, usize> = HashMap::new();
    for t in show
        .sequences
        .iter()
        .flat_map(|s| s.tags.iter())
        .chain(show.media.iter().flat_map(|m| m.tags.iter()))
    {
        *m.entry(t.as_str()).or_default() += 1;
    }
    let mut v: Vec<(String, usize)> = m.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    v.sort();
    v
}

/// Duration of a playlist item in the show (commands and unknown items: 0).
pub fn item_duration_ms(show: &Show, item: &PlaylistItem) -> u64 {
    match item {
        PlaylistItem::Sequence { sequence_id, .. } => {
            show.sequence(sequence_id).map_or(0, |s| s.duration_ms)
        }
        PlaylistItem::Media { media_id, .. } => {
            show.media_item(media_id).map_or(0, |m| m.duration_ms)
        }
        PlaylistItem::Dj { dj_clip_id, .. } => show
            .dj_clip(dj_clip_id)
            .and_then(|c| c.media_id.as_deref())
            .and_then(|m| show.media_item(m))
            .map_or(0, |m| m.duration_ms),
        PlaylistItem::Effect { duration_ms, .. }
        | PlaylistItem::Pause { duration_ms, .. }
        | PlaylistItem::Countdown { duration_ms, .. } => *duration_ms,
        PlaylistItem::Command { .. } => 0,
    }
}

/// Does a tag list satisfy the include rule?
fn includes(tags: &[String], want: &[String], mode: TagMatch) -> bool {
    if want.is_empty() {
        return true;
    }
    match mode {
        TagMatch::Any => want.iter().any(|w| tags.contains(w)),
        TagMatch::All => want.iter().all(|w| tags.contains(w)),
    }
}

struct Candidate<'a> {
    id: &'a str,
    name: &'a str,
    tags: &'a [String],
    dur: u64,
}

/// Expand smart `rules` at local time `now` (see the module docs).
pub fn expand<Tz: TimeZone>(
    show: &Show,
    rules: &SmartRules,
    history: &PlayHistory,
    now: DateTime<Tz>,
    seed: u64,
) -> Expansion {
    expand_for(show, rules, history, now, seed, None)
}

/// [`expand`] for a stored playlist: rotation continues after the song this
/// playlist played last.
pub fn expand_playlist<Tz: TimeZone>(
    show: &Show,
    playlist_id: &str,
    history: &PlayHistory,
    now: DateTime<Tz>,
    seed: u64,
) -> Option<Expansion> {
    let pl = show.playlists.iter().find(|p| p.id == playlist_id)?;
    let rules = pl.smart.as_ref()?;
    Some(expand_for(show, rules, history, now, seed, Some(playlist_id)))
}

fn expand_for<Tz: TimeZone>(
    show: &Show,
    rules: &SmartRules,
    history: &PlayHistory,
    now: DateTime<Tz>,
    seed: u64,
    playlist_id: Option<&str>,
) -> Expansion {
    let now = now.fixed_offset();
    let tonight = night_of(now.naive_local());
    let mut notes: Vec<String> = Vec::new();
    let mut rng = SplitMix(seed);
    let norm = |v: &[String]| normalize_tags(v);
    let include = norm(&rules.include_tags);
    let exclude = norm(&rules.exclude_tags);

    // 1. Candidates.
    let base: Vec<Candidate> = show
        .sequences
        .iter()
        .filter(|s| s.duration_ms > 0)
        .filter(|s| includes(&s.tags, &include, rules.include_mode))
        .filter(|s| !s.tags.iter().any(|t| exclude.contains(t)))
        .filter(|s| rules.max_item_ms.map_or(true, |m| s.duration_ms <= m))
        .map(|s| Candidate {
            id: &s.id,
            name: &s.name,
            tags: &s.tags,
            dur: s.duration_ms,
        })
        .collect();
    if base.is_empty() {
        notes.push(if include.is_empty() {
            "No songs match these rules yet. Upload sequences or loosen the rules.".into()
        } else {
            format!(
                "No songs are tagged {}. Tag some songs on the Sequences page.",
                join_tags(&include, rules.include_mode)
            )
        });
    }
    let mut nights = rules.no_repeat_nights;
    let recent = |nights: u32, c: &Candidate| {
        nights > 0
            && history.nights_of(c.id).iter().any(|n| {
                let ago = (tonight - *n).num_days();
                ago >= 1 && ago <= i64::from(nights)
            })
    };
    let mut cands: Vec<&Candidate> = base.iter().filter(|c| !recent(nights, c)).collect();
    while nights > 0 && cands.len() < MIN_CANDIDATES.min(base.len()) {
        nights -= 1;
        cands = base.iter().filter(|c| !recent(nights, c)).collect();
    }
    if nights < rules.no_repeat_nights {
        notes.push(if nights == 0 {
            "There aren't enough songs to skip everything that played recently, so some repeats are allowed.".into()
        } else {
            format!(
                "There aren't enough songs to skip {} nights of repeats; skipping the last {nights} instead.",
                rules.no_repeat_nights
            )
        });
    }

    // 2. Order.
    let mut cands: Vec<&Candidate> = cands;
    match rules.order {
        SmartOrder::Fixed => {}
        SmartOrder::Shuffle => {
            // Fisher–Yates with the night's seed.
            for i in (1..cands.len()).rev() {
                let j = (rng.next() % (i as u64 + 1)) as usize;
                cands.swap(i, j);
            }
        }
        SmartOrder::LeastRecent => {
            let now_s = now.timestamp();
            let mut keyed: Vec<(i64, &Candidate)> = cands
                .into_iter()
                .map(|c| {
                    // Seconds since the last play; never played sorts first.
                    let age = history
                        .last_played(c.id)
                        .map_or(i64::MAX / 4, |t| (now_s - t.timestamp()).max(0));
                    // Up to 30 min of jitter so equal histories don't always
                    // play in library order.
                    let jitter = (rng.next() % 1800) as i64;
                    (age.saturating_add(jitter), c)
                })
                .collect();
            keyed.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(b.1.id)));
            cands = keyed.into_iter().map(|(_, c)| c).collect();
        }
        SmartOrder::Rotation => {
            cands.sort_by(|a, b| {
                a.name
                    .to_lowercase()
                    .cmp(&b.name.to_lowercase())
                    .then_with(|| a.id.cmp(b.id))
            });
            // The pointer: the candidate this playlist (else anything) played last.
            let last = history
                .plays
                .iter()
                .filter(|p| cands.iter().any(|c| c.id == p.id))
                .filter(|p| playlist_id.map_or(true, |id| p.playlist_id.as_deref() == Some(id)))
                .max_by_key(|p| p.at)
                .map(|p| p.id.clone());
            if let Some(pos) = last.and_then(|l| cands.iter().position(|c| c.id == l)) {
                let n = cands.len();
                cands.rotate_left((pos + 1) % n);
            }
        }
    }

    // 3. Greedy fill.
    let pinned_ms = |v: &[PlaylistItem]| v.iter().map(|i| item_duration_ms(show, i)).sum::<u64>();
    let first_ms = pinned_ms(&rules.pinned_first);
    let last_ms = pinned_ms(&rules.pinned_last);
    let every = rules.interleave_every as usize;
    let inter = if every > 0 { &rules.interleave[..] } else { &[] };
    let tz = schedule_timezone(&show.schedule).ok();
    // (rule end instant, required tags), only rules still ahead tonight.
    let mut rules_ahead: Vec<(DateTime<FixedOffset>, Vec<String>)> = Vec::new();
    for r in &rules.time_rules {
        let tags = norm(&r.require_tags);
        if tags.is_empty() {
            continue;
        }
        let at = tz.as_ref().and_then(|tz| {
            // The rule's time on this night: the evening's date, or the next
            // morning for times after midnight.
            let mut t = resolve_time(&r.before, tonight, tz, &show.schedule)?.fixed_offset();
            if (t - now).num_hours() < -12 {
                t = resolve_time(&r.before, tonight + Duration::days(1), tz, &show.schedule)?
                    .fixed_offset();
            }
            Some(t)
        });
        match at {
            Some(t) if t > now => rules_ahead.push((t, tags)),
            Some(_) => {}
            None => notes.push("A time rule has a time PixelPlus can't read; it was skipped.".into()),
        }
    }
    let allowed = |c: &Candidate, t: DateTime<FixedOffset>| {
        rules_ahead
            .iter()
            .filter(|(until, _)| t < *until)
            .all(|(_, tags)| tags.iter().all(|x| c.tags.contains(x)))
    };
    let target = rules.target_duration_ms;
    let mut placed: Vec<&Candidate> = Vec::new();
    let mut used = vec![false; cands.len()];
    let mut t = now + ms(first_ms);
    let mut total = first_ms + last_ms;
    let mut inter_k = 0usize;
    let mut relaxed_rule = false;
    loop {
        // Interleave duration that would come before the next song.
        let gap = if every > 0 && !inter.is_empty() && !placed.is_empty() && placed.len() % every == 0 {
            item_duration_ms(show, &inter[inter_k % inter.len()])
        } else {
            0
        };
        let at = t + ms(gap);
        let mut pick = (0..cands.len()).find(|&i| !used[i] && allowed(cands[i], at));
        if pick.is_none() && target.is_none() {
            // Not enough songs for the rule: play the rest anyway (noted).
            pick = (0..cands.len()).find(|&i| !used[i]);
            if pick.is_some() {
                relaxed_rule = true;
            }
        }
        let Some(i) = pick else { break };
        if let Some(target) = target {
            if total + gap + cands[i].dur > target {
                // Last slot: the allowed song that lands closest to the
                // target, if that is closer than stopping here.
                let off = |j: usize| (total + gap + cands[j].dur).abs_diff(target);
                // Rotation never skips ahead (that would break the cycle).
                let best = if rules.order == SmartOrder::Rotation {
                    i
                } else {
                    (0..cands.len())
                        .filter(|&j| !used[j] && allowed(cands[j], at))
                        .min_by_key(|&j| (off(j), j))
                        .unwrap_or(i)
                };
                if off(best) < total.abs_diff(target) {
                    placed.push(cands[best]);
                    total += gap + cands[best].dur;
                }
                break;
            }
        }
        used[i] = true;
        if gap > 0 {
            inter_k += 1;
        }
        placed.push(cands[i]);
        total += gap + cands[i].dur;
        t = at + ms(cands[i].dur);
    }
    if relaxed_rule {
        if let Some((until, tags)) = rules_ahead.first() {
            notes.push(format!(
                "Not enough songs are tagged {} to fill the time until {}, so other songs play too.",
                join_tags(tags, TagMatch::All),
                until.format("%-I:%M %p")
            ));
        }
    }
    if let Some(target) = target {
        if !base.is_empty() && total + 60_000 < target && placed.len() == cands.len() {
            notes.push(format!(
                "All matching songs together run {}, shorter than the {} you asked for.",
                fmt_min(total),
                fmt_min(target)
            ));
        }
    }

    // 4. Assemble.
    let mut items: Vec<PlaylistItem> = rules.pinned_first.clone();
    let mut k = 0usize;
    for (n, c) in placed.iter().enumerate() {
        if every > 0 && !inter.is_empty() && n > 0 && n % every == 0 {
            items.push(with_id(&inter[k % inter.len()], &format!("si{n}")));
            k += 1;
        }
        items.push(PlaylistItem::Sequence {
            id: format!("sm{n}-{}", c.id),
            sequence_id: c.id.to_string(),
        });
    }
    items.extend(rules.pinned_last.iter().cloned());
    let total_ms = items.iter().map(|i| item_duration_ms(show, i)).sum();
    Expansion {
        items,
        total_ms,
        notes,
    }
}

/// A copy of `item` with a unique id (the same interleave item appears many times).
fn with_id(item: &PlaylistItem, suffix: &str) -> PlaylistItem {
    let mut v = serde_json::to_value(item).unwrap_or_default();
    if let Some(id) = v.get("id").and_then(|x| x.as_str()).map(str::to_string) {
        v["id"] = serde_json::Value::String(format!("{id}-{suffix}"));
    }
    serde_json::from_value(v).unwrap_or_else(|_| item.clone())
}

fn ms(v: u64) -> Duration {
    Duration::milliseconds(v.min(i64::MAX as u64 / 2) as i64)
}

fn fmt_min(ms: u64) -> String {
    let m = (ms + 30_000) / 60_000;
    if m == 1 {
        "1 minute".into()
    } else {
        format!("{m} minutes")
    }
}

fn join_tags(tags: &[String], mode: TagMatch) -> String {
    let q: Vec<String> = tags.iter().map(|t| format!("“{t}”")).collect();
    match q.len() {
        0 => String::new(),
        1 => q[0].clone(),
        _ => {
            let (last, rest) = q.split_last().unwrap_or((&q[0], &[]));
            let word = if mode == TagMatch::All { "and" } else { "or" };
            format!("{} {word} {last}", rest.join(", "))
        }
    }
}

/// FNV-1a 64-bit (stable across platforms and releases).
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// SplitMix64: a tiny deterministic generator (the output of `rand`'s
/// generators may change between versions; this never does).
#[derive(Debug, Clone)]
pub struct SplitMix(pub u64);

impl SplitMix {
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0.0..1.0`.
    pub fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Sequence, SmartTimeRule, TimeSpec};
    use chrono::TimeZone;
    use chrono_tz::America::Chicago;

    fn seq(id: &str, min: u64, tags: &[&str]) -> Sequence {
        Sequence {
            id: id.into(),
            name: format!("Song {id}"),
            file: format!("sequences/{id}.fseq"),
            duration_ms: min * 60_000,
            frame_ms: 50,
            channel_count: 3,
            media_id: None,
            xlights_name: None,
            thumbnail: None,
            hash: String::new(),
            generated: None,
            tags: tags.iter().map(|t| t.to_string()).collect(),
        }
    }

    fn show() -> Show {
        let mut s = Show::default();
        s.schedule.location.timezone = "America/Chicago".into();
        s.sequences = vec![
            seq("a", 3, &["kids", "classic"]),
            seq("b", 4, &["kids"]),
            seq("c", 5, &["upbeat"]),
            seq("d", 3, &["classic"]),
            seq("e", 4, &["upbeat", "classic"]),
            seq("f", 2, &["kids", "upbeat"]),
            seq("g", 6, &["halloween"]),
        ];
        s
    }

    fn at(h: u32, m: u32) -> DateTime<chrono_tz::Tz> {
        Chicago.with_ymd_and_hms(2026, 12, 12, h, m, 0).unwrap()
    }

    fn ids(e: &Expansion) -> Vec<String> {
        e.items
            .iter()
            .filter_map(|i| match i {
                PlaylistItem::Sequence { sequence_id, .. } => Some(sequence_id.clone()),
                _ => None,
            })
            .collect()
    }

    fn play(id: &str, t: DateTime<chrono_tz::Tz>, pl: Option<&str>) -> Play {
        Play {
            id: id.into(),
            at: t.fixed_offset(),
            playlist_id: pl.map(String::from),
        }
    }

    #[test]
    fn tags_are_normalized() {
        let t = normalize_tags(&[
            "  Kids ".into(),
            "kids".into(),
            "Classic,  Rock".into(),
            "".into(),
            "x".repeat(50),
        ]);
        assert_eq!(t[0], "kids");
        assert_eq!(t[1], "classic rock");
        assert_eq!(t[2].len(), MAX_TAG_LEN);
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn includes_excludes_and_modes() {
        let s = show();
        let mut r = SmartRules {
            include_tags: vec!["kids".into()],
            order: SmartOrder::Fixed,
            ..Default::default()
        };
        let e = expand(&s, &r, &PlayHistory::default(), at(18, 0), 1);
        assert_eq!(ids(&e), ["a", "b", "f"]);
        r.include_tags = vec!["kids".into(), "upbeat".into()];
        r.include_mode = TagMatch::All;
        assert_eq!(ids(&expand(&s, &r, &PlayHistory::default(), at(18, 0), 1)), ["f"]);
        r.include_mode = TagMatch::Any;
        r.exclude_tags = vec!["classic".into()];
        assert_eq!(
            ids(&expand(&s, &r, &PlayHistory::default(), at(18, 0), 1)),
            ["b", "c", "f"]
        );
        r.include_tags = vec!["nothing".into()];
        let e = expand(&s, &r, &PlayHistory::default(), at(18, 0), 1);
        assert!(e.items.is_empty());
        assert!(e.notes[0].contains("nothing"), "{:?}", e.notes);
    }

    #[test]
    fn deterministic_by_seed_and_duration_within_tolerance() {
        let s = show();
        for order in [SmartOrder::Shuffle, SmartOrder::LeastRecent, SmartOrder::Rotation] {
            for target_min in [5u64, 10, 15, 20] {
                for seed in 0..40u64 {
                    let r = SmartRules {
                        target_duration_ms: Some(target_min * 60_000),
                        order,
                        ..Default::default()
                    };
                    let a = expand(&s, &r, &PlayHistory::default(), at(18, 0), seed);
                    let b = expand(&s, &r, &PlayHistory::default(), at(18, 0), seed);
                    assert_eq!(a, b);
                    // Within one song (the longest is 6 min) of the target.
                    assert!(
                        a.total_ms.abs_diff(target_min * 60_000) <= 3 * 60_000,
                        "{order:?} {target_min} {seed}: {}",
                        a.total_ms
                    );
                    // No song twice.
                    let mut v = ids(&a);
                    let n = v.len();
                    v.sort();
                    v.dedup();
                    assert_eq!(v.len(), n);
                }
            }
        }
        let r = SmartRules {
            order: SmartOrder::Shuffle,
            ..Default::default()
        };
        let orders: std::collections::HashSet<Vec<String>> = (0..20)
            .map(|seed| ids(&expand(&s, &r, &PlayHistory::default(), at(18, 0), seed)))
            .collect();
        assert!(orders.len() > 5, "shuffle depends on the seed");
    }

    #[test]
    fn no_repeats_from_last_night_and_relaxation() {
        let s = show();
        let last_night = Chicago.with_ymd_and_hms(2026, 12, 11, 19, 0, 0).unwrap();
        let after_midnight = Chicago.with_ymd_and_hms(2026, 12, 12, 0, 30, 0).unwrap();
        let three_ago = Chicago.with_ymd_and_hms(2026, 12, 9, 19, 0, 0).unwrap();
        let h = PlayHistory {
            plays: vec![
                play("a", last_night, None),
                play("b", after_midnight, None), // still last night's show
                play("c", three_ago, None),
            ],
        };
        let r = SmartRules {
            no_repeat_nights: 1,
            order: SmartOrder::Fixed,
            ..Default::default()
        };
        assert_eq!(ids(&expand(&s, &r, &h, at(18, 0), 1)), ["c", "d", "e", "f", "g"]);
        let r3 = SmartRules {
            no_repeat_nights: 3,
            ..r.clone()
        };
        assert_eq!(ids(&expand(&s, &r3, &h, at(18, 0), 1)), ["d", "e", "f", "g"]);
        // Only kids songs a, b, f: two played last night -> relaxed.
        let kids = SmartRules {
            include_tags: vec!["kids".into()],
            ..r
        };
        let e = expand(&s, &kids, &h, at(18, 0), 1);
        assert_eq!(ids(&e), ["a", "b", "f"]);
        assert!(e.notes.iter().any(|n| n.contains("repeats")), "{:?}", e.notes);
    }

    #[test]
    fn least_recent_puts_never_played_first() {
        let s = show();
        let h = PlayHistory {
            plays: vec![
                play("a", at(17, 0), None),
                play("b", at(10, 0) - Duration::days(5), None),
            ],
        };
        let r = SmartRules {
            include_tags: vec!["kids".into()],
            order: SmartOrder::LeastRecent,
            ..Default::default()
        };
        for seed in 0..10 {
            assert_eq!(ids(&expand(&s, &r, &h, at(18, 0), seed)), ["f", "b", "a"]);
        }
    }

    #[test]
    fn time_rules_are_honoured() {
        let s = show();
        let r = SmartRules {
            time_rules: vec![SmartTimeRule {
                before: TimeSpec::Clock {
                    time: "19:00".into(),
                },
                require_tags: vec!["kids".into()],
            }],
            target_duration_ms: Some(40 * 60_000),
            order: SmartOrder::Shuffle,
            ..Default::default()
        };
        for seed in 0..50 {
            let e = expand(&s, &r, &PlayHistory::default(), at(18, 50), seed);
            // Simulate start times: any song starting before 19:00 is a kids song.
            let mut t = at(18, 50);
            for id in ids(&e) {
                let sq = s.sequence(&id).unwrap();
                if t < at(19, 0) {
                    assert!(sq.tags.contains(&"kids".to_string()), "seed {seed}: {id} at {t}");
                }
                t += Duration::milliseconds(sq.duration_ms as i64);
            }
        }
        // After 19:00 the rule is over.
        let e = expand(&s, &r, &PlayHistory::default(), at(19, 30), 3);
        assert!(ids(&e).iter().any(|id| !s.sequence(id).unwrap().tags.contains(&"kids".into())));
    }

    #[test]
    fn rule_without_enough_songs_is_noted_when_filling_everything() {
        let s = show();
        let r = SmartRules {
            time_rules: vec![SmartTimeRule {
                before: TimeSpec::Clock {
                    time: "23:00".into(),
                },
                require_tags: vec!["kids".into()],
            }],
            order: SmartOrder::Fixed,
            ..Default::default()
        };
        let e = expand(&s, &r, &PlayHistory::default(), at(18, 0), 1);
        assert_eq!(&ids(&e)[..3], ["a", "b", "f"]);
        assert_eq!(ids(&e).len(), 7);
        assert!(e.notes.iter().any(|n| n.contains("11:00 PM")), "{:?}", e.notes);
    }

    #[test]
    fn rotation_continues_across_nights() {
        let s = show();
        let r = SmartRules {
            target_duration_ms: Some(8 * 60_000),
            order: SmartOrder::Rotation,
            ..Default::default()
        };
        // Simulate a week: every night plays what expand says; over the week
        // every song plays and the order cycles through the sorted library.
        let mut h = PlayHistory::default();
        let mut seen: Vec<String> = Vec::new();
        for night in 0..7 {
            let start = at(18, 0) + Duration::days(night);
            let e = expand_for(&s, &r, &h, start, night as u64, Some("pl"));
            let mut t = start;
            for id in ids(&e) {
                h.plays.push(play(&id, t, Some("pl")));
                t += Duration::milliseconds(s.sequence(&id).unwrap().duration_ms as i64);
                seen.push(id);
            }
        }
        let first7: std::collections::BTreeSet<&String> = seen.iter().take(7).collect();
        assert_eq!(first7.len(), 7, "all songs before any repeats: {seen:?}");
        assert_eq!(&seen[..7], &seen[7..14], "a stable cycle");
    }

    #[test]
    fn pinned_and_interleave() {
        let mut s = show();
        s.dj_clips.push(
            serde_json::from_value(serde_json::json!({"id":"dj","name":"Hi","lines":[]})).unwrap(),
        );
        let intro = PlaylistItem::Sequence {
            id: "p1".into(),
            sequence_id: "g".into(),
        };
        let outro = PlaylistItem::Pause {
            id: "p2".into(),
            duration_ms: 1000,
        };
        let dj = PlaylistItem::Dj {
            id: "i1".into(),
            dj_clip_id: "dj".into(),
        };
        let r = SmartRules {
            include_tags: vec!["kids".into()],
            order: SmartOrder::Fixed,
            pinned_first: vec![intro.clone()],
            pinned_last: vec![outro.clone()],
            interleave: vec![dj],
            interleave_every: 2,
            ..Default::default()
        };
        let e = expand(&s, &r, &PlayHistory::default(), at(18, 0), 1);
        assert_eq!(e.items.first(), Some(&intro));
        assert_eq!(e.items.last(), Some(&outro));
        let kinds: Vec<&str> = e
            .items
            .iter()
            .map(|i| match i {
                PlaylistItem::Sequence { .. } => "s",
                PlaylistItem::Dj { .. } => "dj",
                _ => "x",
            })
            .collect();
        assert_eq!(kinds, ["s", "s", "s", "dj", "s", "x"]);
        let mut item_ids: Vec<&str> = e.items.iter().map(|i| i.id()).collect();
        item_ids.sort();
        item_ids.dedup();
        assert_eq!(item_ids.len(), e.items.len(), "unique item ids");
        assert_eq!(e.total_ms, (6 + 3 + 4 + 2) * 60_000 + 1000);
    }

    #[test]
    fn nights_and_seeds() {
        let d = |s: &str| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap();
        assert_eq!(night_of(d("2026-12-12 00:30")).to_string(), "2026-12-11");
        assert_eq!(night_of(d("2026-12-12 18:30")).to_string(), "2026-12-12");
        let n = NaiveDate::from_ymd_opt(2026, 12, 12).unwrap();
        assert_eq!(night_seed(n, "x"), night_seed(n, "x"));
        assert_ne!(night_seed(n, "x"), night_seed(n, "y"));
        assert_ne!(night_seed(n, "x"), night_seed(n.succ_opt().unwrap(), "x"));
        // Pinned: the seed must never change between releases.
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        let mut r = SplitMix(0);
        assert_eq!(r.next(), 0xE220_A839_7B1D_CDAF);
    }
}
