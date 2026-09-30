//! Show scheduler (ARCHITECTURE §4 `Schedule`).
//!
//! Every second the engine's control task computes [`ScheduleFacts`] (which
//! window is active, the next show, the volume curfew) in the schedule's time
//! zone and hands them to the engine, which asks [`Scheduler::decide`] what to
//! do given what is playing. The decision logic is pure and tested with fake
//! facts.
//!
//! Rules:
//! * At a window's start, if nothing is playing, the entry's playlist starts.
//! * **Manual playback started during a window ends with that window.**
//!   Starting something by hand during a window replaces the scheduled
//!   playlist; at the window's end (or when a higher-priority window takes
//!   over) it ends with the entry's end behaviour, exactly like the scheduled
//!   playlist would. When it finishes on its own inside the window, the
//!   window's playlist starts again from the top. If the user presses *Stop*
//!   during a window, the window stays quiet (idle look) until the next window.
//! * **Manual playback outside windows plays once** (a playlist's `repeat` is
//!   ignored) and is never touched by the scheduler; a window that starts
//!   meanwhile waits until it finishes.
//! * **"Loop until I stop"** (`PlayRequest.loopUntilStopped`): the playlist
//!   repeats and keeps playing past window ends until the user stops it.
//! * A scheduled playlist that finishes (non-repeating) inside its window is
//!   not restarted; the idle look shows for the rest of the window.
//! * At a window's end: `finishSong` finishes the current item and plays the
//!   playlist's outro, `stopNow` stops at once, `fadeOut` fades out over one
//!   second. A window that is `preempted` by a higher-priority entry switches
//!   immediately.
//! * Idle inside a window → `idleEffectId` (or dark); outside → `offEffectId`
//!   (or dark).

use super::types::NextShowRef;
use chrono::{DateTime, Utc};
use pixelplus_core::model::{EndBehavior, Schedule};
use pixelplus_core::schedule;
use std::collections::HashSet;

/// The window in charge right now.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveWindow {
    pub entry_id: String,
    pub name: String,
    pub playlist_id: String,
    /// Unique per occurrence: `entryId@start`.
    pub key: String,
    /// RFC 3339.
    pub ends_at: String,
    pub end_behavior: EndBehavior,
    pub preempted: bool,
}

/// Everything the engine needs to know about the schedule at one instant.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScheduleFacts {
    pub enabled: bool,
    pub active: Option<ActiveWindow>,
    pub next_show: Option<NextShowRef>,
    /// Curfew volume while the curfew is active.
    pub volume_cap: Option<u8>,
    pub idle_effect_id: Option<String>,
    pub off_effect_id: Option<String>,
}

/// Evaluate the schedule at `now` (UTC; converted to the schedule's zone).
pub fn facts_at(s: &Schedule, now: DateTime<Utc>) -> ScheduleFacts {
    let tz = schedule::schedule_timezone(s).unwrap_or(chrono_tz::UTC);
    let local = now.with_timezone(&tz);
    let active = if s.enabled {
        schedule::active_at(s, local)
    } else {
        None
    };
    let next = if s.enabled {
        schedule::next_show(s, local)
    } else {
        None
    };
    ScheduleFacts {
        enabled: s.enabled,
        active: active.map(|o| ActiveWindow {
            key: format!("{}@{}", o.entry_id, o.start.to_rfc3339()),
            entry_id: o.entry_id,
            name: o.name,
            playlist_id: o.playlist_id,
            ends_at: o.end.to_rfc3339(),
            end_behavior: o.end_behavior,
            preempted: o.preempted,
        }),
        next_show: next.map(|o| NextShowRef {
            name: o.name,
            starts_at: o.start.to_rfc3339(),
        }),
        volume_cap: match &s.volume_curfew {
            Some(c) if schedule::curfew_active(s, local) => Some(c.volume),
            _ => None,
        },
        idle_effect_id: s.idle_effect_id.clone(),
        off_effect_id: s.off_effect_id.clone(),
    }
}

/// Who started what is playing.
#[derive(Debug, Clone, PartialEq)]
pub enum Origin {
    /// Started by hand (UI, trigger, MQTT, request, calibration).
    Manual {
        /// The window active when it started: the playback ends with it.
        window: Option<ActiveWindow>,
        /// "Loop until I stop": never ended by the schedule.
        looping: bool,
    },
    /// Started by the scheduler for the window with this key.
    Schedule(String),
}

impl Origin {
    /// Manual playback started now: tied to the active window unless `looping`.
    pub fn manual(facts: &ScheduleFacts, looping: bool) -> Origin {
        Origin::Manual {
            window: if looping {
                None
            } else {
                facts.active.clone().filter(|_| facts.enabled)
            },
            looping,
        }
    }

    pub fn is_manual(&self) -> bool {
        matches!(self, Origin::Manual { .. })
    }
}

/// What the engine should do.
#[derive(Debug, Clone, PartialEq)]
pub enum SchedAction {
    /// Start the window's playlist.
    Start(ActiveWindow),
    /// End the scheduled playback with this behaviour.
    End(EndBehavior),
    /// Nothing plays: show this look (effect id) or dark.
    Look(Option<String>),
}

/// Scheduler memory between ticks.
#[derive(Debug, Default, Clone)]
pub struct Scheduler {
    /// The last window the scheduler started.
    started: Option<ActiveWindow>,
    /// Windows that must not (re)start: stopped by the user or finished.
    suppressed: HashSet<String>,
    /// Windows whose end was already acted on.
    ended: HashSet<String>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decide what to do. `playing` is the origin of the current playback
    /// (None when idle).
    pub fn decide(
        &mut self,
        facts: &ScheduleFacts,
        playing: Option<&Origin>,
    ) -> Option<SchedAction> {
        let active = facts.active.as_ref().filter(|_| facts.enabled);
        match playing {
            Some(Origin::Schedule(key)) => {
                if active.is_some_and(|w| &w.key == key) || self.ended.contains(key) {
                    return None;
                }
                self.ended.insert(key.clone());
                let started = self.started.as_ref().filter(|w| &w.key == key);
                let behavior = match started {
                    Some(w) if w.preempted => EndBehavior::StopNow,
                    Some(w) => w.end_behavior,
                    None => EndBehavior::FinishSong,
                };
                Some(SchedAction::End(behavior))
            }
            Some(Origin::Manual {
                window: Some(w),
                looping: false,
            }) => {
                if active.is_some_and(|a| a.key == w.key) || self.ended.contains(&w.key) {
                    return None;
                }
                self.ended.insert(w.key.clone());
                // Another window took over: switch now; else end like the
                // window's own playlist would.
                let behavior = if active.is_some() || w.preempted {
                    EndBehavior::StopNow
                } else {
                    w.end_behavior
                };
                Some(SchedAction::End(behavior))
            }
            Some(Origin::Manual { .. }) => None,
            None => match active {
                Some(w) if !self.suppressed.contains(&w.key) => {
                    self.started = Some(w.clone());
                    self.ended.remove(&w.key);
                    Some(SchedAction::Start(w.clone()))
                }
                Some(_) => Some(SchedAction::Look(facts.idle_effect_id.clone())),
                None => Some(SchedAction::Look(if facts.enabled {
                    facts.off_effect_id.clone()
                } else {
                    None
                })),
            },
        }
    }

    /// The user stopped playback during window `key` (or any window when None
    /// is passed and a window is active): keep that window quiet.
    pub fn on_user_stop(&mut self, facts: &ScheduleFacts, origin: Option<&Origin>) {
        if let Some(Origin::Schedule(key)) = origin {
            self.suppressed.insert(key.clone());
        }
        if let Some(w) = &facts.active {
            self.suppressed.insert(w.key.clone());
        }
    }

    /// Scheduled playback of window `key` finished on its own.
    pub fn on_finished(&mut self, key: &str) {
        self.suppressed.insert(key.to_string());
    }

    /// Forget windows that are over (keeps the sets small).
    pub fn prune(&mut self, facts: &ScheduleFacts) {
        let keep = facts.active.as_ref().map(|w| w.key.clone());
        if self.suppressed.len() + self.ended.len() > 64 {
            self.suppressed.retain(|k| Some(k) == keep.as_ref());
            self.ended.retain(|k| Some(k) == keep.as_ref());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use pixelplus_core::model::{ScheduleEntry, TimeSpec, Weekday};

    fn window(key: &str, behavior: EndBehavior, preempted: bool) -> ActiveWindow {
        ActiveWindow {
            entry_id: key.split('@').next().unwrap().into(),
            name: "Nightly".into(),
            playlist_id: "pl".into(),
            key: key.into(),
            ends_at: "2026-12-01T22:00:00-06:00".into(),
            end_behavior: behavior,
            preempted,
        }
    }

    fn facts(active: Option<ActiveWindow>) -> ScheduleFacts {
        ScheduleFacts {
            enabled: true,
            active,
            idle_effect_id: Some("idle".into()),
            off_effect_id: Some("off".into()),
            ..Default::default()
        }
    }

    #[test]
    fn starts_window_when_idle_and_shows_looks() {
        let mut s = Scheduler::new();
        assert_eq!(
            s.decide(&facts(None), None),
            Some(SchedAction::Look(Some("off".into())))
        );
        let w = window("e1@1", EndBehavior::FinishSong, false);
        assert_eq!(
            s.decide(&facts(Some(w.clone())), None),
            Some(SchedAction::Start(w.clone()))
        );
        // Playing it: nothing to do.
        let o = Origin::Schedule("e1@1".into());
        assert_eq!(s.decide(&facts(Some(w.clone())), Some(&o)), None);
        // Window ends: finish song, once.
        assert_eq!(
            s.decide(&facts(None), Some(&o)),
            Some(SchedAction::End(EndBehavior::FinishSong))
        );
        assert_eq!(s.decide(&facts(None), Some(&o)), None);
        // Stopped: off look.
        assert_eq!(
            s.decide(&facts(None), None),
            Some(SchedAction::Look(Some("off".into())))
        );
    }

    #[test]
    fn disabled_schedule_is_dark_and_never_starts() {
        let mut s = Scheduler::new();
        let mut f = facts(Some(window("e1@1", EndBehavior::StopNow, false)));
        f.enabled = false;
        assert_eq!(s.decide(&f, None), Some(SchedAction::Look(None)));
    }

    #[test]
    fn manual_in_window_ends_with_the_window() {
        let mut s = Scheduler::new();
        let w = window("e1@1", EndBehavior::FadeOut, false);
        let f = facts(Some(w.clone()));
        assert!(matches!(s.decide(&f, None), Some(SchedAction::Start(_))));
        // User plays something by hand: the scheduler leaves it alone while the window lasts.
        let manual = Origin::manual(&f, false);
        assert_eq!(
            manual,
            Origin::Manual {
                window: Some(w.clone()),
                looping: false
            }
        );
        assert_eq!(s.decide(&f, Some(&manual)), None);
        // Manual playback finished on its own: the window's playlist starts again.
        assert!(matches!(s.decide(&f, None), Some(SchedAction::Start(_))));
        // At the window's end the manual playback ends with the entry's behaviour, once.
        assert_eq!(
            s.decide(&facts(None), Some(&manual)),
            Some(SchedAction::End(EndBehavior::FadeOut))
        );
        assert_eq!(s.decide(&facts(None), Some(&manual)), None);
    }

    #[test]
    fn manual_in_window_stops_when_another_window_takes_over() {
        let mut s = Scheduler::new();
        let low = window("low@1", EndBehavior::FinishSong, false);
        let manual = Origin::manual(&facts(Some(low)), false);
        let high = window("high@1", EndBehavior::FinishSong, false);
        assert_eq!(
            s.decide(&facts(Some(high)), Some(&manual)),
            Some(SchedAction::End(EndBehavior::StopNow))
        );
    }

    #[test]
    fn manual_outside_windows_and_looping_are_left_alone() {
        let mut s = Scheduler::new();
        // Outside any window: not tied to one; a window starting later waits.
        let outside = Origin::manual(&facts(None), false);
        assert_eq!(
            outside,
            Origin::Manual {
                window: None,
                looping: false
            }
        );
        let w = window("e1@1", EndBehavior::StopNow, false);
        assert_eq!(s.decide(&facts(Some(w.clone())), Some(&outside)), None);
        // "Loop until I stop" started in a window survives its end.
        let looping = Origin::manual(&facts(Some(w)), true);
        assert_eq!(
            looping,
            Origin::Manual {
                window: None,
                looping: true
            }
        );
        assert_eq!(s.decide(&facts(None), Some(&looping)), None);
        // A disabled schedule never ties manual playback to a window.
        let mut f = facts(Some(window("e2@1", EndBehavior::StopNow, false)));
        f.enabled = false;
        assert_eq!(
            Origin::manual(&f, false),
            Origin::Manual {
                window: None,
                looping: false
            }
        );
    }

    #[test]
    fn user_stop_keeps_window_quiet() {
        let mut s = Scheduler::new();
        let w = window("e1@1", EndBehavior::FinishSong, false);
        let f = facts(Some(w.clone()));
        assert!(matches!(s.decide(&f, None), Some(SchedAction::Start(_))));
        s.on_user_stop(&f, Some(&Origin::Schedule(w.key.clone())));
        assert_eq!(
            s.decide(&f, None),
            Some(SchedAction::Look(Some("idle".into())))
        );
        // The next occurrence starts normally.
        let w2 = window("e1@2", EndBehavior::FinishSong, false);
        assert!(matches!(
            s.decide(&facts(Some(w2)), None),
            Some(SchedAction::Start(_))
        ));
    }

    #[test]
    fn finished_playlist_not_restarted_in_same_window() {
        let mut s = Scheduler::new();
        let w = window("e1@1", EndBehavior::FinishSong, false);
        let f = facts(Some(w.clone()));
        s.decide(&f, None);
        s.on_finished(&w.key);
        assert_eq!(
            s.decide(&f, None),
            Some(SchedAction::Look(Some("idle".into())))
        );
    }

    #[test]
    fn preempted_window_switches_immediately() {
        let mut s = Scheduler::new();
        let low = window("low@1", EndBehavior::FinishSong, true);
        s.decide(&facts(Some(low.clone())), None);
        let high = window("high@1", EndBehavior::FinishSong, false);
        let o = Origin::Schedule(low.key.clone());
        assert_eq!(
            s.decide(&facts(Some(high.clone())), Some(&o)),
            Some(SchedAction::End(EndBehavior::StopNow))
        );
        // Once stopped, the higher-priority window starts.
        assert_eq!(
            s.decide(&facts(Some(high.clone())), None),
            Some(SchedAction::Start(high))
        );
    }

    #[test]
    fn end_behaviors_pass_through() {
        for b in [
            EndBehavior::StopNow,
            EndBehavior::FadeOut,
            EndBehavior::FinishSong,
        ] {
            let mut s = Scheduler::new();
            let w = window("e@1", b, false);
            s.decide(&facts(Some(w.clone())), None);
            assert_eq!(
                s.decide(&facts(None), Some(&Origin::Schedule(w.key))),
                Some(SchedAction::End(b))
            );
        }
    }

    #[test]
    fn facts_from_real_schedule() {
        let mut sch = Schedule {
            enabled: true,
            entries: vec![ScheduleEntry {
                start_exact: Default::default(),
                id: "e1".into(),
                name: "Nightly".into(),
                enabled: true,
                playlist_id: "pl".into(),
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
                    time: "18:00".into(),
                },
                end: TimeSpec::Clock {
                    time: "22:00".into(),
                },
                priority: 0,
                end_behavior: EndBehavior::FadeOut,
            }],
            idle_effect_id: Some("idle".into()),
            ..Default::default()
        };
        sch.location.timezone = "America/Chicago".into();
        // 2026-12-01 19:00 CST = 2026-12-02 01:00 UTC.
        let now = Utc.with_ymd_and_hms(2026, 12, 2, 1, 0, 0).unwrap();
        let f = facts_at(&sch, now);
        let w = f.active.expect("inside the window");
        assert_eq!(w.entry_id, "e1");
        assert_eq!(w.end_behavior, EndBehavior::FadeOut);
        assert!(w.ends_at.starts_with("2026-12-01T22:00:00"));
        assert!(f
            .next_show
            .unwrap()
            .starts_at
            .starts_with("2026-12-02T18:00:00"));
        // 23:00 local: outside.
        let later = Utc.with_ymd_and_hms(2026, 12, 2, 5, 0, 0).unwrap();
        assert!(facts_at(&sch, later).active.is_none());
        sch.enabled = false;
        assert!(facts_at(&sch, now).active.is_none());
    }
}
