//! Season profiles (F8, ARCHITECTURE §12.7): named copies of the
//! season-specific parts of a show (schedule, idle / off looks, song-request
//! playlist and message, games on/off, late-night dimming, props kept dark,
//! the default DJ voice), switched by hand or by date.
//!
//! **Switching copies**: the profile is copied into the live show fields, so
//! the scheduler, request page, games and power limiter keep reading
//! `show.schedule`, `settings.requests`, … unchanged. Before that, the live
//! values are saved back into the profile that was active, so edits made on
//! the ordinary pages are never lost. The library (sequences, media, props,
//! playlists, looks) is global: profiles only reference it by id.
//!
//! What a profile holds and how it is applied:
//!
//! | Profile field | Live field | `None` in the profile |
//! |---|---|---|
//! | `schedule` (except `location`) | `show.schedule` | — |
//! | `requestsPlaylistId` | `settings.requests.playlistId` | all songs |
//! | `requestsMessage` | `settings.requests.message` | unchanged |
//! | `gamesEnabled` | `settings.games.enabled` | unchanged |
//! | `power.dim`, `power.maxBrightness` | `settings.power.{dim, maxBrightness}` | unchanged |
//! | `disabledPropIds` | [`disabled_prop_ids`] (render mask, health) | — |
//! | `defaultDjVoice`, `tags` | read by the UI from [`active`] | — |
//!
//! The schedule's `location` (time zone, sunset) is never switched: it belongs
//! to the place, not the season.
//!
//! **Prop mask** (contract for WS3 engine and WS5 manifests): props listed in
//! the active profile's `disabledPropIds` are rendered dark and skipped by
//! the health checks. Call [`disabled_prop_ids`] with the current show; the
//! leader copies it into `ManifestSettings.disabledPropIds` for followers.
//!
//! **Auto-switch** (`show.profileAutoSwitch`): at boot and daily from 12:00
//! local time, outside show windows, the profile whose `dateRange` contains
//! today wins (highest `priority`, then the narrowest range, then list
//! order). No matching profile: nothing changes. A switch takes an automatic
//! backup first, is journaled (`profileSwitch`) and announced by the alerts
//! channels ("Switched to Christmas").

use crate::api::{ApiError, ApiResult};
use crate::state::AppState;
use chrono::{DateTime, Datelike, NaiveDate, NaiveTime};
use chrono_tz::Tz;
use parking_lot::Mutex;
use pixelplus_core::model::{new_id, DateRange, PowerProfilePart, Schedule, Show, ShowProfile};
use pixelplus_core::schedule;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;

/// Local time of the daily auto-switch check.
pub const AUTO_SWITCH_AT: NaiveTime = match NaiveTime::from_hms_opt(12, 0, 0) {
    Some(t) => t,
    None => unreachable!(),
};
/// Longest profile name.
pub const MAX_NAME: usize = 60;
/// Most profiles a show may keep.
pub const MAX_PROFILES: usize = 24;

/// Runtime state of this service (`state.services.profiles`).
#[derive(Default)]
pub struct ProfilesState {
    /// Local date of the last noon check (done or not needed).
    noon_done: Mutex<Option<NaiveDate>>,
    /// The check at boot ran.
    boot_done: Mutex<bool>,
    /// Serializes switches (manual and automatic).
    switching: tokio::sync::Mutex<()>,
}

/// Why a switch happened (journal, alert text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchReason {
    Manual,
    Auto,
}

// ---------------------------------------------------------------------------
// Pure functions (unit-tested)
// ---------------------------------------------------------------------------

/// The active profile, if any.
pub fn active(show: &Show) -> Option<&ShowProfile> {
    let id = show.active_profile_id.as_deref()?;
    show.profiles.iter().find(|p| p.id == id)
}

/// Props kept dark by the active season (existing props only). **Contract**:
/// WS3's engine renders these black (compose mask) and the health checks skip
/// them; WS5 copies the list into `ManifestSettings.disabledPropIds`.
pub fn disabled_prop_ids(show: &Show) -> Vec<String> {
    let Some(p) = active(show) else {
        return vec![];
    };
    let mut ids: Vec<String> = p
        .disabled_prop_ids
        .iter()
        .filter(|id| show.prop(id).is_some())
        .cloned()
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// A profile holding the current live season settings.
pub fn capture(show: &Show, name: &str) -> ShowProfile {
    let mut p = ShowProfile {
        id: new_id(),
        name: name.trim().to_string(),
        icon: None,
        color: None,
        date_range: None,
        priority: 0,
        schedule: Schedule::default(),
        requests_playlist_id: None,
        requests_message: None,
        default_dj_voice: None,
        games_enabled: None,
        power: None,
        // A copy of the current season keeps its dark props.
        disabled_prop_ids: active(show)
            .map(|a| a.disabled_prop_ids.clone())
            .unwrap_or_default(),
        tags: vec![],
    };
    store_live(show, &mut p);
    if let Some(a) = active(show) {
        p.default_dj_voice = a.default_dj_voice.clone();
    }
    p
}

/// Copy the live season fields into `p` (identity, date range, dark props,
/// voice and tags are kept).
fn store_live(show: &Show, p: &mut ShowProfile) {
    p.schedule = show.schedule.clone();
    p.requests_playlist_id = show.settings.requests.playlist_id.clone();
    p.requests_message = Some(show.settings.requests.message.clone());
    p.games_enabled = Some(show.settings.games.enabled);
    p.power = Some(PowerProfilePart {
        dim: show.settings.power.dim.clone(),
        max_brightness: Some(show.settings.power.max_brightness),
    });
}

/// Copy a profile into the live fields (see the module table).
fn apply(show: &mut Show, p: &ShowProfile) {
    let location = show.schedule.location.clone();
    show.schedule = p.schedule.clone();
    show.schedule.location = location;
    show.settings.requests.playlist_id = p.requests_playlist_id.clone();
    if let Some(m) = &p.requests_message {
        show.settings.requests.message = m.clone();
    }
    if let Some(g) = p.games_enabled {
        show.settings.games.enabled = g;
    }
    if let Some(pw) = &p.power {
        show.settings.power.dim = pw.dim.clone();
        if let Some(b) = pw.max_brightness {
            show.settings.power.max_brightness = b.clamp(1, 100);
        }
    }
}

/// Check a profile against the library before it goes live: every schedule
/// entry's playlist and the looks it names must exist.
pub fn check_references(show: &Show, p: &ShowProfile) -> ApiResult<()> {
    for e in &p.schedule.entries {
        if show.playlist(&e.playlist_id).is_none() {
            return Err(ApiError::bad_request(format!(
                "In \"{}\", the show time \"{}\" plays a playlist that no longer exists. Edit the season first.",
                p.name, e.name
            )));
        }
    }
    for (what, id) in [
        ("idle look", &p.schedule.idle_effect_id),
        ("off look", &p.schedule.off_effect_id),
    ] {
        if let Some(id) = id {
            if show.effect(id).is_none() {
                return Err(ApiError::bad_request(format!(
                    "The {what} of \"{}\" no longer exists. Edit the season first.",
                    p.name
                )));
            }
        }
    }
    if let Some(id) = &p.requests_playlist_id {
        if show.playlist(id).is_none() {
            return Err(ApiError::bad_request(format!(
                "The song-request playlist of \"{}\" no longer exists. Edit the season first.",
                p.name
            )));
        }
    }
    Ok(())
}

/// Validate a profile's own fields (name, date range, brightness, colours).
pub fn validate(p: &ShowProfile) -> ApiResult<()> {
    let n = p.name.trim();
    if n.is_empty() {
        return Err(ApiError::bad_request("Please give the season a name."));
    }
    if n.chars().count() > MAX_NAME {
        return Err(ApiError::bad_request(format!(
            "That name is too long ({MAX_NAME} characters max)."
        )));
    }
    if let Some(r) = &p.date_range {
        if schedule::parse_month_day(&r.start).is_none()
            || schedule::parse_month_day(&r.end).is_none()
        {
            return Err(ApiError::bad_request(
                "The dates must look like 11-01 (month-day).",
            ));
        }
    }
    if p.power
        .as_ref()
        .and_then(|pw| pw.max_brightness)
        .is_some_and(|b| b == 0 || b > 100)
    {
        return Err(ApiError::bad_request(
            "Brightness must be between 1 and 100 %.",
        ));
    }
    if p.icon.as_ref().is_some_and(|i| i.chars().count() > 40)
        || p.color.as_ref().is_some_and(|c| c.chars().count() > 40)
    {
        return Err(ApiError::bad_request("That icon or colour isn't valid."));
    }
    if p.tags.len() > 32 || p.disabled_prop_ids.len() > 10_000 {
        return Err(ApiError::bad_request("That's too many entries."));
    }
    Ok(())
}

/// Switch the live show to profile `id`, saving the live values into the
/// active profile first when `save_current`. Returns the previous profile id.
pub fn switch(show: &mut Show, id: &str, save_current: bool) -> ApiResult<Option<String>> {
    let target = show
        .profiles
        .iter()
        .find(|p| p.id == id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("That season"))?;
    let previous = show.active_profile_id.clone();
    if save_current {
        if let Some(prev) = previous.as_deref().filter(|p| *p != id) {
            let live = show.clone();
            if let Some(p) = show.profiles.iter_mut().find(|p| p.id == prev) {
                store_live(&live, p);
            }
        }
    }
    check_references(show, &target)?;
    apply(show, &target);
    show.active_profile_id = Some(target.id.clone());
    Ok(previous)
}

/// Save the live values into the active profile (after edits on the normal
/// pages, before an export). No active profile: nothing happens.
pub fn save_live_into_active(show: &mut Show) -> bool {
    let Some(id) = show.active_profile_id.clone() else {
        return false;
    };
    let live = show.clone();
    match show.profiles.iter_mut().find(|p| p.id == id) {
        Some(p) => {
            store_live(&live, p);
            true
        }
        None => false,
    }
}

/// Days a date range covers (for "the narrowest range wins").
fn range_days(r: &DateRange) -> u32 {
    let (Some((sm, sd)), Some((em, ed))) = (
        schedule::parse_month_day(&r.start),
        schedule::parse_month_day(&r.end),
    ) else {
        return u32::MAX;
    };
    let day = |m: u32, d: u32| {
        NaiveDate::from_ymd_opt(2024, m, d)
            .map(|x| x.ordinal())
            .unwrap_or(1)
    };
    let (s, e) = (day(sm, sd), day(em, ed));
    if s <= e {
        e - s + 1
    } else {
        366 - s + e + 1
    }
}

/// The profile whose date range contains `date`: highest priority, then the
/// narrowest range (a "Thanksgiving week" inside "Christmas" wins), then
/// list order.
pub fn pick_for_date(profiles: &[ShowProfile], date: NaiveDate) -> Option<&ShowProfile> {
    profiles
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            p.date_range
                .as_ref()
                .is_some_and(|r| schedule::date_in_range(r, date))
        })
        .max_by(|(ia, a), (ib, b)| {
            a.priority
                .cmp(&b.priority)
                .then_with(|| {
                    // Narrower is better: compare reversed.
                    range_days(b.date_range.as_ref().unwrap())
                        .cmp(&range_days(a.date_range.as_ref().unwrap()))
                })
                .then_with(|| ib.cmp(ia))
        })
        .map(|(_, p)| p)
}

/// What the auto-switch should do at `now`: the profile to switch to, if
/// auto-switch is on, no show window is running, and a different profile's
/// date range contains today.
pub fn auto_target(show: &Show, now: DateTime<Tz>) -> Option<String> {
    if !show.profile_auto_switch || show.profiles.is_empty() {
        return None;
    }
    if schedule::active_at(&show.schedule, now).is_some() {
        return None;
    }
    let p = pick_for_date(&show.profiles, now.date_naive())?;
    (show.active_profile_id.as_deref() != Some(p.id.as_str())).then(|| p.id.clone())
}

/// One line per season field that changes, for the "Switch now" confirm.
pub fn diff(show: &Show, target: &ShowProfile) -> Vec<String> {
    let mut lines = vec![];
    let effect_name = |id: &Option<String>| -> String {
        id.as_deref()
            .map(|i| {
                show.effect(i)
                    .map(|e| e.name.clone())
                    .unwrap_or_else(|| "(deleted look)".into())
            })
            .unwrap_or_else(|| "none".into())
    };
    let playlist_name = |id: &Option<String>, none: &str| -> String {
        id.as_deref()
            .map(|i| {
                show.playlist(i)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "(deleted playlist)".into())
            })
            .unwrap_or_else(|| none.into())
    };
    let live = &show.schedule;
    let new = &target.schedule;
    let plural = |n: usize, what: &str| {
        if n == 1 {
            format!("1 {what}")
        } else {
            format!("{n} {what}s")
        }
    };
    if live.entries != new.entries {
        lines.push(format!(
            "Schedule: {} → {}",
            plural(live.entries.len(), "show time"),
            plural(new.entries.len(), "show time")
        ));
    }
    if live.enabled != new.enabled {
        lines.push(format!(
            "Schedule: {}",
            if new.enabled { "turns on" } else { "turns off" }
        ));
    }
    if live.idle_effect_id != new.idle_effect_id {
        lines.push(format!(
            "Idle look: {} → {}",
            effect_name(&live.idle_effect_id),
            effect_name(&new.idle_effect_id)
        ));
    }
    if live.off_effect_id != new.off_effect_id {
        lines.push(format!(
            "Off look: {} → {}",
            effect_name(&live.off_effect_id),
            effect_name(&new.off_effect_id)
        ));
    }
    if live.volume_curfew != new.volume_curfew {
        lines.push("Quiet hours: change".into());
    }
    let req = &show.settings.requests;
    if req.playlist_id != target.requests_playlist_id {
        lines.push(format!(
            "Song requests: {} → {}",
            playlist_name(&req.playlist_id, "all songs"),
            playlist_name(&target.requests_playlist_id, "all songs")
        ));
    }
    if target
        .requests_message
        .as_ref()
        .is_some_and(|m| *m != req.message)
    {
        lines.push("Request page message: changes".into());
    }
    if let Some(g) = target.games_enabled {
        if g != show.settings.games.enabled {
            lines.push(format!(
                "Games: {}",
                if g { "off → on" } else { "on → off" }
            ));
        }
    }
    if let Some(pw) = &target.power {
        if pw.dim != show.settings.power.dim {
            lines.push(format!(
                "Late-night dimming: {} → {}",
                plural(show.settings.power.dim.len(), "window"),
                plural(pw.dim.len(), "window")
            ));
        }
        if let Some(b) = pw.max_brightness {
            if b != show.settings.power.max_brightness {
                lines.push(format!(
                    "Brightness limit: {} % → {} %",
                    show.settings.power.max_brightness, b
                ));
            }
        }
    }
    let now_dark = disabled_prop_ids(show);
    let mut next_dark: Vec<String> = target
        .disabled_prop_ids
        .iter()
        .filter(|id| show.prop(id).is_some())
        .cloned()
        .collect();
    next_dark.sort();
    next_dark.dedup();
    if now_dark != next_dark {
        let names: Vec<String> = next_dark
            .iter()
            .take(3)
            .filter_map(|id| show.prop(id).map(|p| p.name.clone()))
            .collect();
        let more = next_dark.len().saturating_sub(names.len());
        lines.push(if next_dark.is_empty() {
            "Props kept dark: none (every prop lights up)".into()
        } else {
            format!(
                "Props kept dark: {}{}",
                names.join(", "),
                if more > 0 {
                    format!(" and {more} more")
                } else {
                    String::new()
                }
            )
        });
    }
    let voice_now = active(show).and_then(|a| a.default_dj_voice.clone());
    if voice_now != target.default_dj_voice {
        let name = |v: &Option<String>| {
            v.as_deref()
                .map(|id| {
                    show.dj_voices
                        .iter()
                        .find(|x| x.id == id)
                        .map(|x| x.name.clone())
                        .unwrap_or_else(|| id.to_string())
                })
                .unwrap_or_else(|| "none".into())
        };
        lines.push(format!(
            "Default DJ voice: {} → {}",
            name(&voice_now),
            name(&target.default_dj_voice)
        ));
    }
    if lines.is_empty() {
        lines.push("Nothing changes: this season matches what's live now.".into());
    }
    lines
}

/// Short label, e.g. "🎄 Christmas".
pub fn label(p: &ShowProfile) -> String {
    match p
        .icon
        .as_deref()
        .filter(|i| !i.is_empty() && i.chars().count() <= 4)
    {
        Some(icon) => format!("{icon} {}", p.name),
        None => p.name.clone(),
    }
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

/// Status for the dashboard chip (`GET /profiles/active`).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ActiveSeason {
    pub id: Option<String>,
    pub name: Option<String>,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub auto_switch: bool,
    /// The season the calendar picks today (auto-switch target), if any.
    pub scheduled_id: Option<String>,
    /// The next date-based switch: `{profileId, date}`.
    pub next_switch: Option<NextSwitch>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NextSwitch {
    pub profile_id: String,
    pub name: String,
    /// Local date "YYYY-MM-DD".
    pub date: String,
}

/// The next date (after `today`, up to a year ahead) on which the calendar
/// picks a different profile than today's pick.
pub fn next_switch(show: &Show, today: NaiveDate) -> Option<NextSwitch> {
    let current = pick_for_date(&show.profiles, today)
        .map(|p| p.id.clone())
        .or_else(|| show.active_profile_id.clone());
    for d in 1..=366 {
        let date = today + chrono::Duration::days(d);
        if let Some(p) = pick_for_date(&show.profiles, date) {
            if Some(&p.id) != current.as_ref() {
                return Some(NextSwitch {
                    profile_id: p.id.clone(),
                    name: p.name.clone(),
                    date: date.format("%Y-%m-%d").to_string(),
                });
            }
        }
    }
    None
}

pub fn active_season(show: &Show, today: NaiveDate) -> ActiveSeason {
    let a = active(show);
    ActiveSeason {
        id: a.map(|p| p.id.clone()),
        name: a.map(|p| p.name.clone()),
        icon: a.and_then(|p| p.icon.clone()),
        color: a.and_then(|p| p.color.clone()),
        auto_switch: show.profile_auto_switch,
        scheduled_id: pick_for_date(&show.profiles, today).map(|p| p.id.clone()),
        next_switch: next_switch(show, today),
    }
}

/// The show's local time zone (UTC when unknown).
pub fn show_tz(show: &Show) -> Tz {
    schedule::schedule_timezone(&show.schedule).unwrap_or(chrono_tz::UTC)
}

/// Switch to profile `id` now: automatic backup, save the live values into
/// the old profile, copy the new one in, journal it (and announce automatic
/// switches). Returns the new show.
pub async fn activate(
    state: &AppState,
    id: &str,
    save_current: bool,
    reason: SwitchReason,
) -> ApiResult<Arc<Show>> {
    let _one = state.services.profiles.switching.lock().await;
    let show = state.store.get();
    let target = show
        .profiles
        .iter()
        .find(|p| p.id == id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("That season"))?;
    check_references(&show, &target)?;
    super::snapshots::auto(state, &format!("Before switching to {}", target.name)).await;
    let id = id.to_string();
    let (previous, show) = state
        .store
        .update(move |s| switch(s, &id, save_current))
        .await?;
    let from_name = previous
        .as_deref()
        .and_then(|p| show.profiles.iter().find(|x| x.id == p))
        .map(|p| p.name.clone());
    state
        .services
        .journal
        .record(super::journal::Event::ProfileSwitch {
            from: from_name.clone(),
            to: target.name.clone(),
        });
    tracing::info!(
        "Season switched to \"{}\" ({})",
        target.name,
        match reason {
            SwitchReason::Manual => "by hand",
            SwitchReason::Auto => "by date",
        }
    );
    if reason == SwitchReason::Auto {
        let body = match &from_name {
            Some(f) => format!(
                "PixelPlus switched from {f} to {} for the new season: schedule, looks and request list changed with it.",
                target.name
            ),
            None => format!(
                "PixelPlus switched to {} for the new season: schedule, looks and request list changed with it.",
                target.name
            ),
        };
        super::alerts::raise(
            state,
            &format!("season:{}", target.id),
            super::alerts::Severity::Info,
            &format!("Switched to {}", label(&target)),
            &body,
        )
        .await;
    }
    Ok(show)
}

/// Run the auto-switch check once (at boot / noon).
async fn auto_check(state: &AppState) {
    let show = state.store.get();
    let now = chrono::Utc::now().with_timezone(&show_tz(&show));
    let Some(id) = auto_target(&show, now) else {
        return;
    };
    if let Err(e) = activate(state, &id, true, SwitchReason::Auto).await {
        let name = show
            .profiles
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        tracing::warn!(
            "Automatic season switch to \"{name}\" failed: {}",
            e.message
        );
        state.services.journal.record(super::journal::Event::Warn {
            code: "profileSwitch".into(),
            msg: e.message.clone(),
        });
        super::alerts::raise(
            state,
            &format!("season-failed:{id}"),
            super::alerts::Severity::Warning,
            &format!("Couldn't switch to {name}"),
            &e.message,
        )
        .await;
    }
}

/// Whether the check should run now: once shortly after boot, then once per
/// day from 12:00 (retried every minute while a show window runs, see
/// [`auto_target`]). Marks the check as done.
fn due(st: &ProfilesState, now: DateTime<Tz>, show: &Show) -> bool {
    let in_window = schedule::active_at(&show.schedule, now).is_some();
    let mut boot = st.boot_done.lock();
    if !*boot {
        if in_window {
            return false;
        }
        *boot = true;
        // The boot check also counts for today's noon check once past noon.
        if now.time() >= AUTO_SWITCH_AT {
            *st.noon_done.lock() = Some(now.date_naive());
        }
        return true;
    }
    drop(boot);
    let mut noon = st.noon_done.lock();
    if now.time() < AUTO_SWITCH_AT || *noon == Some(now.date_naive()) || in_window {
        return false;
    }
    *noon = Some(now.date_naive());
    true
}

/// Start the service (called once from `services::start_all`).
pub fn start(state: &AppState) {
    // The xLights watch folder (F16, also WS6) has no service slot of its
    // own; starting it here keeps the shared `services/mod.rs` untouched.
    crate::api::fppcompat::start(state);
    let state = state.clone();
    tokio::spawn(async move {
        // Let the scheduler and cluster settle after boot.
        tokio::time::sleep(Duration::from_secs(20)).await;
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let show = state.store.get();
            if !show.profile_auto_switch || show.profiles.is_empty() {
                continue;
            }
            let now = chrono::Utc::now().with_timezone(&show_tz(&show));
            if due(&state.services.profiles, now, &show) {
                auto_check(&state).await;
            }
        }
    });
}

/// A new, empty profile (no show times) with the given name.
pub fn empty(name: &str) -> ShowProfile {
    ShowProfile {
        id: new_id(),
        name: name.trim().to_string(),
        icon: None,
        color: None,
        date_range: None,
        priority: 0,
        schedule: Schedule {
            enabled: true,
            ..Schedule::default()
        },
        requests_playlist_id: None,
        requests_message: None,
        default_dj_voice: None,
        games_enabled: None,
        power: None,
        disabled_prop_ids: vec![],
        tags: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use pixelplus_core::model::{
        EffectKind, EffectPreset, Playlist, Prop, ScheduleEntry, Target, TimeSpec, Weekday,
    };

    fn show() -> Show {
        let mut s = Show::default();
        for (id, name) in [("plx", "Christmas Mix"), ("plh", "Halloween Mix")] {
            s.playlists.push(
                serde_json::from_value::<Playlist>(serde_json::json!({"id": id, "name": name}))
                    .unwrap(),
            );
        }
        for (id, name) in [("candy", "Candy Stripes"), ("fog", "Spooky Fog")] {
            s.effects.push(EffectPreset {
                id: id.into(),
                name: name.into(),
                effect: EffectKind::Solid,
                params: Default::default(),
                target: Target {
                    all: true,
                    ..Target::default()
                },
            });
        }
        for (id, name) in [("p1", "Arch"), ("p2", "Inflatable Santa")] {
            s.props.push(
                serde_json::from_value::<Prop>(serde_json::json!({
                    "id": id, "name": name, "kind": "line", "pixelCount": 10,
                    "channelStart": 0
                }))
                .unwrap(),
            );
        }
        s.schedule.location.timezone = "America/Chicago".into();
        s
    }

    fn entry(id: &str, pl: &str) -> ScheduleEntry {
        ScheduleEntry {
            id: id.into(),
            name: format!("Show {id}"),
            enabled: true,
            playlist_id: pl.into(),
            days: vec![
                Weekday::Mon,
                Weekday::Tue,
                Weekday::Wed,
                Weekday::Thu,
                Weekday::Fri,
                Weekday::Sat,
                Weekday::Sun,
            ],
            date_range: None,
            start: TimeSpec::Clock {
                time: "17:00".into(),
            },
            end: TimeSpec::Clock {
                time: "22:00".into(),
            },
            priority: 0,
            end_behavior: Default::default(),
            start_exact: false,
        }
    }

    fn two_seasons() -> Show {
        let mut s = show();
        s.schedule.entries = vec![entry("e1", "plx"), entry("e2", "plx")];
        s.schedule.idle_effect_id = Some("candy".into());
        s.settings.requests.playlist_id = Some("plx".into());
        let mut xmas = capture(&s, "Christmas");
        xmas.id = "xmas".into();
        xmas.icon = Some("🎄".into());
        xmas.date_range = Some(DateRange {
            start: "11-01".into(),
            end: "01-06".into(),
        });
        let mut hw = empty("Halloween");
        hw.id = "hw".into();
        hw.schedule.entries = vec![entry("h1", "plh")];
        hw.schedule.idle_effect_id = Some("fog".into());
        hw.requests_playlist_id = Some("plh".into());
        hw.disabled_prop_ids = vec!["p2".into(), "gone".into()];
        hw.games_enabled = Some(false);
        hw.date_range = Some(DateRange {
            start: "10-01".into(),
            end: "10-31".into(),
        });
        s.profiles = vec![xmas, hw];
        s.active_profile_id = Some("xmas".into());
        s
    }

    #[test]
    fn profiles_round_trip_through_json() {
        let s = two_seasons();
        let json = serde_json::to_string(&s).unwrap();
        let back: Show = serde_json::from_str(&json).unwrap();
        assert_eq!(back.profiles, s.profiles);
        assert_eq!(back.active_profile_id.as_deref(), Some("xmas"));
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["profiles"][1]["disabledPropIds"][0], "p2");
        assert_eq!(v["profiles"][0]["dateRange"]["start"], "11-01");
    }

    #[test]
    fn switching_copies_in_and_saves_live_edits_back() {
        let mut s = two_seasons();
        let library = (
            s.sequences.clone(),
            s.playlists.clone(),
            s.effects.clone(),
            s.props.clone(),
        );
        // Edit live Christmas settings on the normal pages.
        s.schedule.entries.push(entry("e3", "plx"));
        s.settings.power.max_brightness = 80;
        let prev = switch(&mut s, "hw", true).unwrap();
        assert_eq!(prev.as_deref(), Some("xmas"));
        assert_eq!(s.active_profile_id.as_deref(), Some("hw"));
        assert_eq!(s.schedule.entries.len(), 1);
        assert_eq!(s.schedule.idle_effect_id.as_deref(), Some("fog"));
        assert_eq!(s.settings.requests.playlist_id.as_deref(), Some("plh"));
        assert!(!s.settings.games.enabled);
        // The location is not part of a season.
        assert_eq!(s.schedule.location.timezone, "America/Chicago");
        // Library ids untouched.
        assert_eq!(
            library,
            (
                s.sequences.clone(),
                s.playlists.clone(),
                s.effects.clone(),
                s.props.clone()
            )
        );
        assert_eq!(disabled_prop_ids(&s), vec!["p2".to_string()]);
        // Christmas kept the live edits made before the switch.
        let x = s.profiles.iter().find(|p| p.id == "xmas").unwrap();
        assert_eq!(x.schedule.entries.len(), 3);
        assert_eq!(x.power.as_ref().unwrap().max_brightness, Some(80));
        // And comes back exactly.
        switch(&mut s, "xmas", true).unwrap();
        assert_eq!(s.schedule.entries.len(), 3);
        assert_eq!(s.settings.power.max_brightness, 80);
        assert_eq!(s.settings.requests.playlist_id.as_deref(), Some("plx"));
        assert!(disabled_prop_ids(&s).is_empty());
        // Halloween stayed as it was.
        let h = s.profiles.iter().find(|p| p.id == "hw").unwrap();
        assert_eq!(h.schedule.entries.len(), 1);
    }

    #[test]
    fn switch_without_saving_discards_live_edits() {
        let mut s = two_seasons();
        s.schedule.entries.clear();
        switch(&mut s, "hw", false).unwrap();
        let x = s.profiles.iter().find(|p| p.id == "xmas").unwrap();
        assert_eq!(x.schedule.entries.len(), 2);
    }

    #[test]
    fn switch_refuses_dangling_references() {
        let mut s = two_seasons();
        s.playlists.retain(|p| p.id != "plh");
        let err = switch(&mut s, "hw", true).unwrap_err();
        assert!(err.message.contains("no longer exists"), "{}", err.message);
        assert_eq!(s.active_profile_id.as_deref(), Some("xmas"));
        assert!(switch(&mut s, "nope", true).is_err());
    }

    #[test]
    fn calendar_picks_with_year_wrap_priority_and_narrowest() {
        let mut s = two_seasons();
        let d = |m, d| NaiveDate::from_ymd_opt(2026, m, d).unwrap();
        let pick = |s: &Show, date| pick_for_date(&s.profiles, date).map(|p| p.id.clone());
        assert_eq!(pick(&s, d(10, 15)).as_deref(), Some("hw"));
        assert_eq!(pick(&s, d(11, 1)).as_deref(), Some("xmas"));
        assert_eq!(pick(&s, d(12, 31)).as_deref(), Some("xmas"));
        assert_eq!(
            pick(&s, NaiveDate::from_ymd_opt(2027, 1, 6).unwrap()).as_deref(),
            Some("xmas")
        );
        assert_eq!(pick(&s, NaiveDate::from_ymd_opt(2027, 1, 7).unwrap()), None);
        assert_eq!(pick(&s, d(7, 4)), None);
        // A week inside Christmas wins by being narrower…
        let mut tg = empty("Thanksgiving");
        tg.id = "tg".into();
        tg.date_range = Some(DateRange {
            start: "11-20".into(),
            end: "11-28".into(),
        });
        s.profiles.push(tg);
        assert_eq!(pick(&s, d(11, 25)).as_deref(), Some("tg"));
        // …unless Christmas has the higher priority.
        s.profiles[0].priority = 5;
        assert_eq!(pick(&s, d(11, 25)).as_deref(), Some("xmas"));
    }

    #[test]
    fn auto_target_only_outside_show_windows_and_when_enabled() {
        let mut s = two_seasons();
        let tz: Tz = "America/Chicago".parse().unwrap();
        let oct_noon = tz.with_ymd_and_hms(2026, 10, 2, 12, 0, 0).unwrap();
        assert_eq!(auto_target(&s, oct_noon), None, "auto-switch is off");
        s.profile_auto_switch = true;
        assert_eq!(auto_target(&s, oct_noon).as_deref(), Some("hw"));
        // During the 17:00–22:00 window nothing switches.
        s.schedule.enabled = true;
        let oct_evening = tz.with_ymd_and_hms(2026, 10, 2, 18, 0, 0).unwrap();
        assert_eq!(auto_target(&s, oct_evening), None);
        // Already active: nothing to do.
        let dec = tz.with_ymd_and_hms(2026, 12, 2, 12, 0, 0).unwrap();
        assert_eq!(auto_target(&s, dec), None);
        // Year wrap: 5 Jan still Christmas, 7 Jan nothing.
        switch(&mut s, "hw", true).unwrap();
        let jan5 = tz.with_ymd_and_hms(2027, 1, 5, 12, 0, 0).unwrap();
        assert_eq!(auto_target(&s, jan5).as_deref(), Some("xmas"));
        let jan7 = tz.with_ymd_and_hms(2027, 1, 7, 12, 0, 0).unwrap();
        assert_eq!(auto_target(&s, jan7), None);
    }

    #[test]
    fn noon_and_boot_checks_run_once() {
        let st = ProfilesState::default();
        let mut s = two_seasons();
        s.schedule.enabled = true;
        let tz: Tz = "America/Chicago".parse().unwrap();
        let t = |d, h, m| tz.with_ymd_and_hms(2026, 10, d, h, m, 0).unwrap();
        assert!(due(&st, t(2, 9, 0), &s), "boot check");
        assert!(!due(&st, t(2, 11, 59), &s));
        assert!(due(&st, t(2, 12, 0), &s), "noon");
        assert!(!due(&st, t(2, 12, 1), &s));
        // Next day at noon, but a show runs from 17:00 only: due.
        assert!(due(&st, t(3, 12, 30), &s));
        // A window over noon postpones the check until it ends.
        let st = ProfilesState::default();
        *st.boot_done.lock() = true;
        s.schedule.entries[0].start = TimeSpec::Clock {
            time: "11:00".into(),
        };
        s.schedule.entries[0].end = TimeSpec::Clock {
            time: "13:00".into(),
        };
        assert!(!due(&st, t(4, 12, 0), &s));
        assert!(due(&st, t(4, 13, 1), &s));
    }

    #[test]
    fn diff_lists_what_changes() {
        let s = two_seasons();
        let hw = s.profiles[1].clone();
        let lines = diff(&s, &hw);
        let all = lines.join("\n");
        assert!(
            all.contains("Schedule: 2 show times → 1 show time"),
            "{all}"
        );
        assert!(
            all.contains("Idle look: Candy Stripes → Spooky Fog"),
            "{all}"
        );
        assert!(
            all.contains("Song requests: Christmas Mix → Halloween Mix"),
            "{all}"
        );
        assert!(all.contains("Props kept dark: Inflatable Santa"), "{all}");
        assert!(all.contains("Games: on → off") || !s.settings.games.enabled);
        let same = diff(&s, &s.profiles[0].clone());
        assert_eq!(same.len(), 1);
        assert!(same[0].starts_with("Nothing changes"));
    }

    #[test]
    fn validation() {
        let mut p = empty("  ");
        assert!(validate(&p).is_err());
        p.name = "Christmas".into();
        assert!(validate(&p).is_ok());
        p.date_range = Some(DateRange {
            start: "13-01".into(),
            end: "01-06".into(),
        });
        assert!(validate(&p).is_err());
        p.date_range = Some(DateRange {
            start: "02-29".into(),
            end: "03-01".into(),
        });
        assert!(validate(&p).is_ok());
        p.power = Some(PowerProfilePart {
            dim: vec![],
            max_brightness: Some(0),
        });
        assert!(validate(&p).is_err());
    }

    #[test]
    fn next_switch_date() {
        let s = two_seasons();
        let n = next_switch(&s, NaiveDate::from_ymd_opt(2026, 9, 30).unwrap()).unwrap();
        assert_eq!(
            (n.profile_id.as_str(), n.date.as_str()),
            ("hw", "2026-10-01")
        );
        let n = next_switch(&s, NaiveDate::from_ymd_opt(2026, 10, 20).unwrap()).unwrap();
        assert_eq!(
            (n.profile_id.as_str(), n.date.as_str()),
            ("xmas", "2026-11-01")
        );
    }
}
