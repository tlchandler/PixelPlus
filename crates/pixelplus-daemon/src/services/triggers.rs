//! Triggers (`settings.triggers`): physical buttons on free GPIOs, HTTP
//! triggers (`POST /triggers/:id/fire`) and sensor-node inputs (F20), mapped
//! to player actions, including **surprises** layered over the show.
//!
//! Every firing (button, HTTP, sensor) passes the trigger's gates first
//! ([`check`]): `when` (always / showOnly / idleOnly / offOnly), the daily
//! `activeWindow`, `cooldownS` and `maxPerHour`. Entry points:
//! * [`fire`] — HTTP (`POST /triggers/:id/fire`), by id or name;
//! * [`sensor_input`] — **for WS6's sensor-node service**: call it for every
//!   authenticated input change of an adopted sensor node; it fires the
//!   `kind: "sensor"` triggers of that input on its active edge;
//! * [`run_action`] — carry out an action without gates (UI "Test" buttons,
//!   `POST /surprises/test`).

use crate::api::{ApiError, ApiResult};
use crate::player::surprise::{SurpriseRequest, SurpriseStarted};
use crate::player::{PlayRequest, PlayerCmd, PlayerState, PlayerStatus};
use crate::services::journal::Event;
use crate::state::AppState;
use parking_lot::Mutex;
use pixelplus_core::model::{
    FeatureId, Show, TimeWindow, Trigger, TriggerAction, TriggerActionType, TriggerKind,
    TriggerWhen,
};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn player(state: &AppState) -> ApiResult<&crate::player::PlayerHandle> {
    state.services.player.get().ok_or_else(|| {
        ApiError::unavailable("The player is still starting. Try again in a moment.")
    })
}

/// Carry out a trigger action. Returns a short description of what happened.
pub async fn run_action(state: &AppState, action: &TriggerAction) -> ApiResult<String> {
    let show = state.store.get();
    let r = action.r#ref.as_deref().unwrap_or("");
    let p = player(state)?;
    let empty = PlayRequest::default();
    match action.kind {
        TriggerActionType::Stop => {
            p.send(PlayerCmd::Stop { fade: true }).await?;
            Ok("Stopped the show".into())
        }
        TriggerActionType::PlayPlaylist => {
            let pl = show
                .playlist(r)
                .or_else(|| {
                    show.playlists
                        .iter()
                        .find(|p| p.name.eq_ignore_ascii_case(r))
                })
                .ok_or_else(|| {
                    ApiError::bad_request(
                        "This trigger's playlist no longer exists. Edit the trigger.",
                    )
                })?;
            p.play(PlayRequest {
                playlist_id: Some(pl.id.clone()),
                ..empty
            })
            .await?;
            Ok(format!("Playing {}", pl.name))
        }
        TriggerActionType::PlaySequence => {
            let s = show.sequence(r).ok_or_else(|| {
                ApiError::bad_request("This trigger's sequence no longer exists. Edit the trigger.")
            })?;
            p.play(PlayRequest {
                sequence_id: Some(s.id.clone()),
                ..empty
            })
            .await?;
            Ok(format!("Playing {}", s.name))
        }
        TriggerActionType::Effect => {
            let e = show.effect(r).ok_or_else(|| {
                ApiError::bad_request("This trigger's look no longer exists. Edit the trigger.")
            })?;
            p.play(PlayRequest {
                effect_id: Some(e.id.clone()),
                ..empty
            })
            .await?;
            Ok(format!("Showing {}", e.name))
        }
        TriggerActionType::Surprise => {
            crate::api::features::require(state, FeatureId::Surprises)?;
            let req = surprise_request(&show, "test", action)?;
            let s = p.surprise(req).await?;
            Ok(surprise_message(&s))
        }
    }
}

/// What a `surprise` action asks the engine for: `source` says whether `ref`
/// is a sequence or a look (guessed from the id when missing); `target` the
/// props (empty = all); `durationMs` the length (default: the sequence's,
/// or 5 s for a look).
pub fn surprise_request(
    show: &Show,
    id: &str,
    action: &TriggerAction,
) -> ApiResult<SurpriseRequest> {
    let r = action.r#ref.as_deref().unwrap_or("").trim();
    if r.is_empty() {
        return Err(ApiError::bad_request(
            "Pick the sequence or look this surprise shows.",
        ));
    }
    let kind = match action.source.as_deref() {
        Some("sequence") => "sequence",
        Some("effect") => "effect",
        _ if show.sequence(r).is_some() => "sequence",
        _ => "effect",
    };
    if kind == "sequence" && show.sequence(r).is_none() {
        return Err(ApiError::bad_request(
            "This surprise's sequence no longer exists. Edit the trigger.",
        ));
    }
    if kind == "effect" && crate::player::compose::find_effect(show, r).is_none() {
        return Err(ApiError::bad_request(
            "This surprise's look no longer exists. Edit the trigger.",
        ));
    }
    let targets: Vec<String> = match &action.target {
        Some(t) if t.all || (t.prop_ids.is_empty() && t.group_ids.is_empty()) => vec![],
        Some(t) => {
            let ids: Vec<String> = t.resolve(show).into_iter().map(|p| p.id.clone()).collect();
            if ids.is_empty() {
                return Err(ApiError::bad_request(
                    "This surprise's props no longer exist. Edit the trigger.",
                ));
            }
            ids
        }
        None => vec![],
    };
    Ok(SurpriseRequest {
        id: id.to_string(),
        kind: kind.into(),
        r#ref: r.to_string(),
        targets,
        duration_ms: action.duration_ms,
    })
}

fn surprise_message(s: &SurpriseStarted) -> String {
    format!(
        "Surprise: {} on {} prop{} for {:.1} s",
        s.name,
        s.props,
        if s.props == 1 { "" } else { "s" },
        s.duration_ms as f64 / 1000.0
    )
}

// ---------------------------------------------------------------------------
// Gates: when, active window, cooldown, max per hour
// ---------------------------------------------------------------------------

/// Why a trigger did not fire.
#[derive(Debug, Clone, PartialEq)]
pub enum Blocked {
    /// `when` does not match what the display is doing.
    When(TriggerWhen),
    /// Outside the trigger's daily active window.
    Window,
    /// Fired less than `cooldownS` ago.
    Cooldown { left_s: u32 },
    /// Already fired `maxPerHour` times in the last hour.
    MaxPerHour,
}

impl Blocked {
    pub fn message(&self) -> String {
        match self {
            Blocked::When(TriggerWhen::ShowOnly) => "it only fires while the show plays".into(),
            Blocked::When(TriggerWhen::IdleOnly) => {
                "it only fires while a look shows between shows".into()
            }
            Blocked::When(TriggerWhen::OffOnly) => "it only fires outside show times".into(),
            Blocked::When(TriggerWhen::Always) => "not now".into(),
            Blocked::Window => "it is outside its active hours".into(),
            Blocked::Cooldown { left_s } => format!("it is cooling down ({left_s} s left)"),
            Blocked::MaxPerHour => "it already fired as often as allowed this hour".into(),
        }
    }
}

/// What the display is doing (for `when`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Moment {
    /// A playlist / song plays (or is paused).
    pub show_playing: bool,
    /// A look shows (idle look, off look, live look).
    pub look: bool,
    /// A schedule window is in progress.
    pub in_window: bool,
    /// Inside the trigger's `activeWindow` (true when it has none).
    pub in_active_window: bool,
}

impl Moment {
    pub fn of(status: &PlayerStatus) -> Moment {
        Moment {
            show_playing: matches!(status.state, PlayerState::Playing | PlayerState::Paused),
            look: status.state == PlayerState::Effect,
            in_window: status.schedule_entry.is_some(),
            in_active_window: true,
        }
    }
}

/// Firing history of one trigger.
#[derive(Debug, Clone, Default)]
pub struct Stats {
    /// Seconds (monotonic) of recent firings, oldest first (≤ 1 h kept).
    fired: VecDeque<f64>,
}

impl Stats {
    pub fn record(&mut self, now_s: f64) {
        self.fired.push_back(now_s);
        while self.fired.front().is_some_and(|&t| now_s - t > 3600.0) {
            self.fired.pop_front();
        }
    }
}

/// The gates, in the order people think about them. Pure (tested).
pub fn check(t: &Trigger, m: &Moment, stats: &Stats, now_s: f64) -> Result<(), Blocked> {
    let ok = match t.when {
        TriggerWhen::Always => true,
        TriggerWhen::ShowOnly => m.show_playing,
        TriggerWhen::IdleOnly => !m.show_playing && m.look,
        TriggerWhen::OffOnly => !m.show_playing && !m.in_window,
    };
    if !ok {
        return Err(Blocked::When(t.when));
    }
    if !m.in_active_window {
        return Err(Blocked::Window);
    }
    if let Some(&last) = stats.fired.back() {
        let since = now_s - last;
        if t.cooldown_s > 0 && since < f64::from(t.cooldown_s) {
            return Err(Blocked::Cooldown {
                left_s: (f64::from(t.cooldown_s) - since).ceil() as u32,
            });
        }
    }
    if t.max_per_hour > 0 {
        let recent = stats.fired.iter().filter(|&&f| now_s - f < 3600.0).count();
        if recent >= t.max_per_hour as usize {
            return Err(Blocked::MaxPerHour);
        }
    }
    Ok(())
}

/// Whether local time `now` is inside `w` (wrapping midnight when `to` is
/// before `from`), in the show's time zone.
pub fn in_window(show: &Show, w: &TimeWindow, now: chrono::DateTime<chrono::Utc>) -> bool {
    use pixelplus_core::schedule::{resolve_time, schedule_timezone};
    let s = &show.schedule;
    let tz = schedule_timezone(s).unwrap_or(chrono_tz::UTC);
    let local = now.with_timezone(&tz);
    let today = local.date_naive();
    [today - chrono::Duration::days(1), today]
        .into_iter()
        .any(|d| {
            let (Some(from), Some(to)) = (
                resolve_time(&w.from, d, &tz, s),
                resolve_time(&w.to, d, &tz, s),
            ) else {
                return false;
            };
            let to = if to <= from {
                to + chrono::Duration::days(1)
            } else {
                to
            };
            from <= local && local < to
        })
}

static STATS: Mutex<Option<HashMap<String, Stats>>> = parking_lot::const_mutex(None);
static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn now_s() -> f64 {
    EPOCH
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f64()
}

/// Fire `t` (from `source`: "http" | "gpio" | "sensor") through its gates.
/// `Err` when a gate blocked it or the action failed.
pub async fn fire_trigger(state: &AppState, t: &Trigger, source: &str) -> ApiResult<String> {
    let show = state.store.get();
    // Off in Settings → Features: triggers (and surprises) never fire.
    crate::api::features::require(state, FeatureId::Triggers)?;
    if t.action.kind == TriggerActionType::Surprise {
        crate::api::features::require(state, FeatureId::Surprises)?;
    }
    let player = player(state)?;
    let mut m = Moment::of(&player.status());
    m.in_active_window = t
        .active_window
        .as_ref()
        .map_or(true, |w| in_window(&show, w, chrono::Utc::now()));
    let now = now_s();
    // Check and take the slot under one lock: two presses arriving together
    // (a double click, two HTTP calls) must not both pass the cooldown.
    {
        let mut g = STATS.lock();
        let stats = g
            .get_or_insert_with(HashMap::new)
            .entry(t.id.clone())
            .or_default();
        if let Err(b) = check(t, &m, stats, now) {
            return Err(ApiError::conflict(format!(
                "“{}” didn't fire: {}.",
                t.name,
                b.message()
            )));
        }
        stats.record(now);
    }
    let result = if t.action.kind == TriggerActionType::Surprise {
        match surprise_request(&show, &t.id, &t.action) {
            Ok(req) => player.surprise(req).await.map(|s| surprise_message(&s)),
            Err(e) => Err(e),
        }
    } else {
        run_action(state, &t.action).await
    };
    let msg = match result {
        Ok(m) => m,
        Err(e) => {
            // It didn't happen: give the slot back.
            if let Some(s) = STATS.lock().as_mut().and_then(|m| m.get_mut(&t.id)) {
                if let Some(pos) = s.fired.iter().rposition(|&f| f == now) {
                    s.fired.remove(pos);
                }
            }
            return Err(e);
        }
    };
    state
        .services
        .journal
        .record(Event::Trigger { id: t.id.clone() });
    tracing::info!("Trigger \"{}\" ({source}): {msg}", t.name);
    Ok(msg)
}

pub async fn fire(state: &AppState, id: &str) -> ApiResult<String> {
    let show = state.store.get();
    let t: Trigger = show
        .settings
        .triggers
        .iter()
        .find(|t| t.id == id || t.name.eq_ignore_ascii_case(id))
        .cloned()
        .ok_or_else(|| ApiError::not_found("That trigger"))?;
    fire_trigger(state, &t, "http").await
}

/// One trigger fired (or blocked) by a sensor input.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fired {
    pub trigger_id: String,
    pub ok: bool,
    pub message: String,
}

/// **Sensor-node entry point (F20, called by WS6's `services/sensornodes.rs`).**
/// `active` is the input's new logical state (after its `activeLow`
/// inversion); triggers fire on the rising edge (`active == true`) only.
/// Returns one result per matching `kind: "sensor"` trigger (empty when none
/// matches or on a release).
pub async fn sensor_input(
    state: &AppState,
    sensor_node_id: &str,
    input: &str,
    active: bool,
) -> Vec<Fired> {
    if !active {
        return vec![];
    }
    let show = state.store.get();
    let matching: Vec<Trigger> = show
        .settings
        .triggers
        .iter()
        .filter(|t| t.kind == TriggerKind::Sensor)
        .filter(|t| {
            t.sensor
                .as_ref()
                .is_some_and(|s| s.sensor_node_id == sensor_node_id && s.input == input)
        })
        .cloned()
        .collect();
    let mut out = Vec::new();
    for t in matching {
        let r = fire_trigger(state, &t, "sensor").await;
        if let Err(e) = &r {
            tracing::info!("Sensor trigger \"{}\": {}", t.name, e.message);
        }
        out.push(Fired {
            trigger_id: t.id.clone(),
            ok: r.is_ok(),
            message: match r {
                Ok(m) => m,
                Err(e) => e.message,
            },
        });
    }
    out
}

fn gpio_pins(state: &AppState) -> Vec<u8> {
    let show = state.store.get();
    // Off in Settings → Features: no GPIO lines are held.
    if !show.feature(FeatureId::Triggers) {
        return vec![];
    }
    let mut pins: Vec<u8> = show
        .settings
        .triggers
        .iter()
        .filter(|t| t.kind == TriggerKind::Gpio)
        .filter_map(|t| t.gpio)
        .collect();
    pins.sort_unstable();
    pins.dedup();
    pins
}

/// Watch GPIO buttons; re-open them whenever the trigger list changes.
pub fn start(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u8>();
        let mut changes = state.store.subscribe();
        let mut current: Vec<u8> = Vec::new();
        let mut stop: Option<Arc<AtomicBool>> = None;
        let mut watcher: Option<std::thread::JoinHandle<()>> = None;
        loop {
            let pins = gpio_pins(&state);
            if pins != current {
                if let Some(s) = stop.take() {
                    s.store(true, Ordering::Relaxed);
                }
                current = pins.clone();
                if !pins.is_empty() {
                    let (board, _) = super::system::effective_board(&state);
                    let flag = Arc::new(AtomicBool::new(false));
                    stop = Some(flag.clone());
                    // The previous watcher still holds its GPIO lines for up to half a
                    // second (line requests are exclusive): the new one waits for it.
                    watcher = spawn_gpio_thread(
                        board,
                        pins,
                        tx.clone(),
                        flag,
                        state.events.clone(),
                        watcher.take(),
                    );
                }
            }
            tokio::select! {
                r = changes.changed() => if r.is_err() { break },
                Some(gpio) = rx.recv() => {
                    let show = state.store.get();
                    for t in show.settings.triggers.iter().filter(|t| t.kind == TriggerKind::Gpio && t.gpio == Some(gpio)) {
                        match fire_trigger(&state, t, "gpio").await {
                            Ok(msg) => tracing::info!("Button \"{}\" (GPIO{gpio}): {msg}", t.name),
                            Err(e) => tracing::warn!("Button \"{}\" (GPIO{gpio}) failed: {}", t.name, e.message),
                        }
                    }
                }
            }
        }
    });
}

#[cfg(target_os = "linux")]
fn spawn_gpio_thread(
    board: pixelplus_core::model::BoardKind,
    pins: Vec<u8>,
    tx: tokio::sync::mpsc::UnboundedSender<u8>,
    stop: Arc<AtomicBool>,
    events: crate::events::EventBus,
    previous: Option<std::thread::JoinHandle<()>>,
) -> Option<std::thread::JoinHandle<()>> {
    use pixelplus_hw::gpio::{ButtonSource, GpioButtons, DEFAULT_DEBOUNCE};
    std::thread::Builder::new()
        .name("pp-gpio".into())
        .spawn(move || {
            if let Some(h) = previous {
                let _ = h.join();
            }
            if stop.load(Ordering::Relaxed) {
                return; // replaced again meanwhile
            }
            let mut buttons = match GpioButtons::open(board, &pins, DEFAULT_DEBOUNCE) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!("Trigger buttons unavailable: {e}");
                    events.toast(
                        crate::events::ToastKind::Warning,
                        format!("Trigger buttons can't be used: {e}"),
                    );
                    return;
                }
            };
            tracing::info!("Watching trigger buttons on GPIO {pins:?}");
            while !stop.load(Ordering::Relaxed) {
                match buttons.wait(std::time::Duration::from_millis(500)) {
                    Ok(Some(ev)) if ev.pressed => {
                        let _ = tx.send(ev.gpio);
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!("Trigger buttons stopped: {e}");
                        return;
                    }
                }
            }
        })
        .ok()
}

#[cfg(not(target_os = "linux"))]
fn spawn_gpio_thread(
    _board: pixelplus_core::model::BoardKind,
    _pins: Vec<u8>,
    _tx: tokio::sync::mpsc::UnboundedSender<u8>,
    _stop: Arc<AtomicBool>,
    _events: crate::events::EventBus,
    _previous: Option<std::thread::JoinHandle<()>>,
) -> Option<std::thread::JoinHandle<()>> {
    tracing::info!("GPIO trigger buttons are only available on a Raspberry Pi");
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixelplus_core::model::TimeSpec;

    fn trigger(when: TriggerWhen, cooldown_s: u32, max_per_hour: u32) -> Trigger {
        Trigger {
            id: "t".into(),
            name: "Sidewalk PIR".into(),
            kind: TriggerKind::Sensor,
            gpio: None,
            action: TriggerAction {
                kind: TriggerActionType::Surprise,
                r#ref: Some("sparkle".into()),
                target: None,
                duration_ms: None,
                source: None,
            },
            sensor: None,
            cooldown_s,
            when,
            active_window: None,
            max_per_hour,
        }
    }

    fn moment(show_playing: bool, look: bool, in_window: bool) -> Moment {
        Moment {
            show_playing,
            look,
            in_window,
            in_active_window: true,
        }
    }

    #[test]
    fn when_gates() {
        let s = Stats::default();
        let playing = moment(true, false, true);
        let idle_look = moment(false, true, true);
        let off = moment(false, false, false);
        let off_look = moment(false, true, false);
        for (when, ok) in [
            (TriggerWhen::Always, [true, true, true, true]),
            (TriggerWhen::ShowOnly, [true, false, false, false]),
            (TriggerWhen::IdleOnly, [false, true, false, true]),
            (TriggerWhen::OffOnly, [false, false, true, true]),
        ] {
            let t = trigger(when, 0, 0);
            let got: Vec<bool> = [playing, idle_look, off, off_look]
                .iter()
                .map(|m| check(&t, m, &s, 0.0).is_ok())
                .collect();
            assert_eq!(got, ok, "{when:?}");
        }
        let mut m = playing;
        m.in_active_window = false;
        assert_eq!(
            check(&trigger(TriggerWhen::Always, 0, 0), &m, &s, 0.0),
            Err(Blocked::Window)
        );
    }

    #[test]
    fn cooldown_and_hourly_cap() {
        let t = trigger(TriggerWhen::Always, 60, 3);
        let m = moment(true, false, true);
        let mut s = Stats::default();
        assert!(check(&t, &m, &s, 0.0).is_ok());
        s.record(0.0);
        assert_eq!(
            check(&t, &m, &s, 10.0),
            Err(Blocked::Cooldown { left_s: 50 })
        );
        assert!(check(&t, &m, &s, 60.0).is_ok());
        s.record(60.0);
        s.record(120.0);
        assert_eq!(check(&t, &m, &s, 200.0), Err(Blocked::MaxPerHour));
        // An hour after the first one, it may fire again.
        assert!(check(&t, &m, &s, 3601.0).is_ok());
        s.record(3601.0);
        assert_eq!(s.fired.len(), 3, "older than an hour is forgotten");
        assert!(Blocked::Cooldown { left_s: 5 }.message().contains("5 s"));
        // No limits: fires every time.
        let free = trigger(TriggerWhen::Always, 0, 0);
        assert!(check(&free, &m, &s, 3601.0).is_ok());
    }

    #[test]
    fn active_window_wraps_midnight() {
        use chrono::TimeZone;
        let mut show = Show::default();
        show.schedule.location.timezone = "UTC".into();
        let w = TimeWindow {
            from: TimeSpec::Clock {
                time: "17:00".into(),
            },
            to: TimeSpec::Clock {
                time: "01:00".into(),
            },
        };
        let at = |h: u32, m: u32| chrono::Utc.with_ymd_and_hms(2026, 12, 1, h, m, 0).unwrap();
        assert!(in_window(&show, &w, at(18, 0)));
        assert!(in_window(&show, &w, at(0, 30)));
        assert!(!in_window(&show, &w, at(1, 0)));
        assert!(!in_window(&show, &w, at(12, 0)));
    }

    #[test]
    fn surprise_requests_resolve_refs_and_targets() {
        use pixelplus_core::model::*;
        let show = Show::default();
        let mut a = trigger(TriggerWhen::Always, 0, 0).action;
        // A built-in look id resolves as a look.
        a.r#ref = Some(pixelplus_core::effects::builtin_presets()[0].id.clone());
        let r = surprise_request(&show, "t", &a).unwrap();
        assert_eq!((r.kind.as_str(), r.targets.len()), ("effect", 0));
        a.r#ref = Some("nope".into());
        assert!(surprise_request(&show, "t", &a).is_err());
        a.r#ref = Some(pixelplus_core::effects::builtin_presets()[0].id.clone());
        a.target = Some(Target {
            prop_ids: vec!["gone".into()],
            ..Default::default()
        });
        assert!(
            surprise_request(&show, "t", &a).is_err(),
            "targets that no longer exist"
        );
        a.target = None;
        a.source = Some("sequence".into());
        assert!(surprise_request(&show, "t", &a).is_err());
    }
}
