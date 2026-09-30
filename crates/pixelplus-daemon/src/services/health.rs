//! Pre-show health check. Runs automatically 15 minutes before each scheduled
//! show (and on demand with `POST /health/run`); a failing check raises an
//! alert.

use super::system::{disk_space, have, in_docker, run};
use crate::state::AppState;
use parking_lot::Mutex;
use pixelplus_core::model::{MediaKind, PlaylistItem, Show};
use pixelplus_hw::{SensorKind, SensorStatus};
use serde::Serialize;
use std::time::Duration;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub id: String,
    pub label: String,
    pub status: Status,
    pub detail: String,
    /// A fix the UI can offer as a button (e.g. `applyOutputGeometry`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HealthReport {
    pub ok: bool,
    pub ran_at: String,
    pub checks: Vec<Check>,
}

#[derive(Default)]
pub struct HealthState {
    last: Mutex<Option<HealthReport>>,
    /// Start time (RFC 3339) of the show we last checked automatically.
    checked_show: Mutex<Option<String>>,
}

impl HealthState {
    pub fn last(&self) -> Option<HealthReport> {
        self.last.lock().clone()
    }
}

fn check(id: &str, label: &str, status: Status, detail: impl Into<String>) -> Check {
    Check {
        id: id.into(),
        label: label.into(),
        status,
        detail: detail.into(),
        action: None,
    }
}

fn list_names(names: &[String]) -> String {
    match names.len() {
        0 => String::new(),
        1..=3 => names.join(", "),
        n => format!("{} and {} more", names[..3].join(", "), n - 3),
    }
}

/// Props that aren't (fully) wired, or wired to ports that don't exist.
fn wiring_check(show: &Show) -> Check {
    let unwired: Vec<String> = show
        .props
        .iter()
        .filter(|p| p.segments.is_empty())
        .map(|p| p.name.clone())
        .collect();
    let partly: Vec<String> = show
        .props
        .iter()
        .filter(|p| !p.segments.is_empty() && p.unwired_pixels() > 0)
        .map(|p| {
            format!(
                "{} ({} of {} pixels)",
                p.name,
                p.unwired_pixels(),
                p.pixel_count
            )
        })
        .collect();
    let impossible: Vec<String> = show
        .props
        .iter()
        .flat_map(|p| p.segments.iter().map(move |s| (p, s)))
        .filter_map(|(p, s)| match show.node(&s.node_id) {
            None => Some(format!("{} (its controller was removed)", p.name)),
            Some(n) if s.output == 0 || s.output as usize > n.outputs.len() => Some(format!(
                "{} (port {} doesn't exist on {})",
                p.name, s.output, n.name
            )),
            Some(_) => None,
        })
        .collect();
    let wiring = |status, detail: String| check("wiring", "Prop wiring", status, detail);
    if !impossible.is_empty() {
        return wiring(
            Status::Fail,
            format!(
                "Wired to a port that isn't there: {}",
                list_names(&impossible)
            ),
        );
    }
    let mut problems = Vec::new();
    if !unwired.is_empty() {
        problems.push(format!("{} not wired to any port", list_names(&unwired)));
    }
    if !partly.is_empty() {
        problems.push(format!("Partly wired: {}", list_names(&partly)));
    }
    if !problems.is_empty() {
        return wiring(Status::Warn, problems.join(". "));
    }
    wiring(
        Status::Ok,
        if show.props.is_empty() {
            "No props yet".to_string()
        } else {
            format!("All {} props are wired to a port", show.props.len())
        },
    )
}

/// Checks that only depend on the show and the data directory (pure-ish, testable).
pub fn content_checks(show: &Show, data_dir: &std::path::Path) -> Vec<Check> {
    let mut out = Vec::new();
    // Sequences used by playlists.
    let mut missing_seq = Vec::new();
    let mut missing_file = Vec::new();
    let mut no_audio = Vec::new();
    let mut missing_audio = Vec::new();
    let mut unrendered = Vec::new();
    let mut used = 0;
    for pl in &show.playlists {
        let used_in_schedule = show
            .schedule
            .entries
            .iter()
            .any(|e| e.enabled && e.playlist_id == pl.id);
        if !used_in_schedule && !show.schedule.entries.is_empty() {
            continue;
        }
        for item in pl.intro.iter().chain(&pl.items).chain(&pl.outro) {
            match item {
                PlaylistItem::Sequence { sequence_id, .. } => {
                    used += 1;
                    match show.sequence(sequence_id) {
                        None => missing_seq.push(format!("{} (in {})", sequence_id, pl.name)),
                        Some(s) => {
                            if !data_dir.join(&s.file).is_file() {
                                missing_file.push(s.name.clone());
                            }
                            match s.media_id.as_deref().map(|id| show.media_item(id)) {
                                None => no_audio.push(s.name.clone()),
                                Some(None) => missing_audio.push(s.name.clone()),
                                Some(Some(m)) if !data_dir.join(&m.file).is_file() => {
                                    missing_audio.push(s.name.clone())
                                }
                                _ => {}
                            }
                        }
                    }
                }
                // DJ Studio off: its clips are skipped, nothing to check.
                PlaylistItem::Dj { .. } if !show.feature(pixelplus_core::model::FeatureId::Dj) => {}
                PlaylistItem::Dj { dj_clip_id, .. } => match show.dj_clip(dj_clip_id) {
                    Some(c) if !c.dynamic => {
                        let ok = c
                            .media_id
                            .as_deref()
                            .and_then(|id| show.media_item(id))
                            .is_some_and(|m| {
                                m.kind == MediaKind::Dj && data_dir.join(&m.file).is_file()
                            });
                        if !ok {
                            unrendered.push(c.name.clone());
                        }
                    }
                    Some(_) => {}
                    None => missing_seq.push(format!("DJ clip {} (in {})", dj_clip_id, pl.name)),
                },
                _ => {}
            }
        }
    }
    let mut seq_status = Status::Ok;
    let mut details = Vec::new();
    if !missing_seq.is_empty() {
        seq_status = Status::Fail;
        details.push(format!(
            "Playlists point at deleted items: {}",
            list_names(&missing_seq)
        ));
    }
    if !missing_file.is_empty() {
        seq_status = Status::Fail;
        details.push(format!(
            "Sequence files are missing for {} (upload them again)",
            list_names(&missing_file)
        ));
    }
    if !missing_audio.is_empty() {
        seq_status = Status::Fail;
        details.push(format!(
            "Audio files are missing for {}",
            list_names(&missing_audio)
        ));
    }
    if !no_audio.is_empty() {
        seq_status = seq_status.max(Status::Warn);
        details.push(format!(
            "No audio linked to {} (they'll play silently)",
            list_names(&no_audio)
        ));
    }
    if !unrendered.is_empty() {
        seq_status = seq_status.max(Status::Warn);
        details.push(format!(
            "DJ clips not rendered yet: {}",
            list_names(&unrendered)
        ));
    }
    out.push(check(
        "sequences",
        "Sequences & audio",
        seq_status,
        if details.is_empty() {
            if used == 0 {
                "No sequences in the scheduled playlists yet".to_string()
            } else {
                format!("All {used} scheduled sequences have their files and audio")
            }
        } else {
            details.join(". ")
        },
    ));
    out.push(wiring_check(show));
    // Schedule.
    let issues = pixelplus_core::schedule::validate(&show.schedule);
    out.push(if !show.schedule.enabled {
        check(
            "schedule",
            "Schedule",
            Status::Warn,
            "The schedule is turned off, so shows won't start by themselves",
        )
    } else if !issues.is_empty() {
        check(
            "schedule",
            "Schedule",
            Status::Warn,
            issues
                .iter()
                .map(|i| i.message.clone())
                .collect::<Vec<_>>()
                .join(". "),
        )
    } else {
        let next = pixelplus_core::schedule::schedule_timezone(&show.schedule)
            .ok()
            .and_then(|tz| {
                let now = chrono::Utc::now().with_timezone(&tz);
                pixelplus_core::schedule::active_at(&show.schedule, now)
                    .map(|o| {
                        format!(
                            "\"{}\" is on now until {}",
                            o.name,
                            o.end.format("%-I:%M %p")
                        )
                    })
                    .or_else(|| {
                        pixelplus_core::schedule::next_show(&show.schedule, now).map(|o| {
                            format!(
                                "Next show: \"{}\" {}",
                                o.name,
                                o.start.format("%a %b %-d at %-I:%M %p")
                            )
                        })
                    })
            });
        check(
            "schedule",
            "Schedule",
            Status::Ok,
            next.unwrap_or_else(|| "No upcoming shows".into()),
        )
    });
    if let Some(c) = location_check(show) {
        out.push(c);
    }
    out
}

/// Sunset/sunrise times computed for the built-in default location (the
/// middle of the USA) are wrong almost everywhere: controllers set up from
/// pixelplus.txt or the imager never saw the wizard's location step.
pub fn location_check(show: &Show) -> Option<Check> {
    use pixelplus_core::model::{Location, TimeSpec};
    let s = &show.schedule;
    let sun = |t: &TimeSpec| !matches!(t, TimeSpec::Clock { .. });
    let uses_sun = s
        .entries
        .iter()
        .any(|e| e.enabled && (sun(&e.start) || sun(&e.end)))
        || s.volume_curfew.as_ref().is_some_and(|c| sun(&c.time));
    let d = Location::default();
    let unset = s.location.lat == d.lat && s.location.lon == d.lon;
    (s.enabled && uses_sun && unset).then(|| {
        check(
            "location",
            "Show location",
            Status::Warn,
            "Sunset and sunrise times are worked out for the middle of the USA. Set your \
             show's location under Schedule so shows start at your sunset.",
        )
    })
}

async fn host_checks(state: &AppState) -> Vec<Check> {
    let mut out = Vec::new();
    // Controllers (followers): live cluster status (the last `nodes` WebSocket
    // message can be seconds old, e.g. right after followers finished syncing).
    let nodes = state
        .services
        .cluster
        .get()
        .and_then(|c| serde_json::to_value(c.nodes_status()).ok())
        .or_else(|| {
            state
                .services
                .snapshot_for_new_client()
                .into_iter()
                .find(|(k, _)| *k == "nodes")
                .map(|(_, v)| v)
        });
    let show = state.store.get();
    let followers: Vec<_> = show
        .nodes
        .iter()
        .filter(|n| n.role == pixelplus_core::model::NodeRole::Follower && n.adopted)
        .collect();
    if followers.is_empty() {
        out.push(check(
            "followers",
            "Controllers",
            Status::Ok,
            "This controller runs the whole show",
        ));
    } else {
        let status_of = |id: &str| {
            nodes
                .as_ref()
                .and_then(|v| v.as_array())
                .and_then(|a| a.iter().find(|n| n["id"] == id))
                .cloned()
        };
        let mut offline = Vec::new();
        let mut syncing = Vec::new();
        for f in &followers {
            match status_of(&f.id) {
                Some(s) if s["online"].as_bool() == Some(false) => offline.push(f.name.clone()),
                Some(s) if s["syncState"].as_str() == Some("syncing") => {
                    syncing.push(f.name.clone())
                }
                Some(_) => {}
                None => offline.push(f.name.clone()),
            }
        }
        out.push(if !offline.is_empty() {
            check(
                "followers",
                "Controllers",
                Status::Fail,
                format!("Offline: {}", list_names(&offline)),
            )
        } else if !syncing.is_empty() {
            check(
                "followers",
                "Controllers",
                Status::Warn,
                format!("Still receiving files: {}", list_names(&syncing)),
            )
        } else {
            check(
                "followers",
                "Controllers",
                Status::Ok,
                format!("All {} controllers online and in sync", followers.len() + 1),
            )
        });
    }
    // Timing between controllers (sync quality, Wi-Fi power save, versions).
    let statuses: Vec<crate::cluster::NodeStatus> = nodes
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let own_ps = crate::services::network::wifi_power_save();
    if let Some(c) = sync_check(&statuses, own_ps, !followers.is_empty()) {
        out.push(c);
    }
    // Disk.
    if let Some((free, _)) = disk_space(&state.config.data_dir) {
        let mb = free / (1024 * 1024);
        out.push(if mb < 100 {
            check(
                "disk",
                "Storage",
                Status::Fail,
                format!("Only {mb} MB free. Delete unused sequences or snapshots."),
            )
        } else if mb < 500 {
            check(
                "disk",
                "Storage",
                Status::Warn,
                format!("{mb} MB free. Getting full."),
            )
        } else {
            check(
                "disk",
                "Storage",
                Status::Ok,
                format!("{:.1} GB free", mb as f64 / 1024.0),
            )
        });
    }
    // Sensors.
    let readings = state.services.sensors.latest();
    let temps: Vec<_> = readings
        .iter()
        .filter(|r| r.sensor.kind == SensorKind::Temperature)
        .collect();
    if let Some(hot) = temps
        .iter()
        .max_by(|a, b| a.sensor.value.total_cmp(&b.sensor.value))
    {
        let st = temps
            .iter()
            .map(|r| r.status)
            .max()
            .unwrap_or(SensorStatus::Ok);
        let limit = show.settings.alerts.rules.temp_c as f64;
        let status = match st {
            SensorStatus::Crit => Status::Fail,
            SensorStatus::Warn => Status::Warn,
            SensorStatus::Ok if hot.sensor.value > limit => Status::Warn,
            SensorStatus::Ok => Status::Ok,
        };
        out.push(check(
            "temp",
            "Temperatures",
            status,
            format!("Highest {:.0} °C ({})", hot.sensor.value, hot.sensor.label),
        ));
    }
    if let Some(v) = readings
        .iter()
        .find(|r| r.sensor.kind == SensorKind::Voltage)
    {
        let min = show.settings.alerts.rules.voltage_min as f64;
        let status = match v.status {
            SensorStatus::Crit => Status::Fail,
            SensorStatus::Warn => Status::Warn,
            SensorStatus::Ok if v.sensor.value < min => Status::Warn,
            _ => Status::Ok,
        };
        out.push(check(
            "power",
            "12 V supply",
            status,
            format!("{:.2} V at the transmitter", v.sensor.value),
        ));
    }
    // Audio device.
    out.push(if crate::player::engine::EngineOptions::from_env().audio {
        let playing_silently = state
            .services
            .player
            .get()
            .and_then(|p| p.status().error)
            .filter(|e| is_sound_problem(e));
        match playing_silently {
            Some(e) => check("audio", "Audio output", Status::Fail, e),
            None => audio_check(&show.settings.audio.device).await,
        }
    } else {
        audio_off_check()
    });
    // Player / output.
    out.push(match state.services.player.get() {
        None => check(
            "output",
            "Light output",
            Status::Fail,
            "The player isn't running",
        ),
        Some(p) => {
            // "No sound: …" is the audio check's business; the lights keep playing.
            match p.status().error.filter(|e| !is_sound_problem(e)) {
                Some(e) => check("output", "Light output", Status::Fail, e),
                None => check("output", "Light output", Status::Ok, "Ready"),
            }
        }
    });
    // Pixel output length vs. the DPI mode set at boot.
    if let Some(c) = geometry_check(&super::geometry::status(state)) {
        out.push(c);
    }
    // Clock.
    out.push(clock_check(state).await);
    out
}

/// How well the followers keep time with this leader: protocol mismatches
/// fail, Wi-Fi power save, a large clock error or lossy links warn. `None`
/// when there is nothing to say (no followers and power save off/unknown).
pub fn sync_check(
    nodes: &[crate::cluster::NodeStatus],
    own_power_save: Option<bool>,
    has_followers: bool,
) -> Option<Check> {
    use pixelplus_core::model::NodeRole;
    let followers: Vec<_> = nodes
        .iter()
        .filter(|n| n.role == NodeRole::Follower && n.adopted && n.online)
        .collect();
    let mismatch: Vec<String> = followers
        .iter()
        .filter(|n| n.protocol != 0 && n.protocol != crate::cluster::proto::PROTOCOL_VERSION)
        .map(|n| n.name.clone())
        .collect();
    if !mismatch.is_empty() {
        return Some(check(
            "sync",
            "Timing",
            Status::Fail,
            format!(
                "{} run another PixelPlus version (cluster protocol). Update every controller \
                 to the same version.",
                list_names(&mismatch)
            ),
        ));
    }
    let mut power_save: Vec<String> = followers
        .iter()
        .filter(|n| n.wifi_power_save == Some(true))
        .map(|n| n.name.clone())
        .collect();
    if own_power_save == Some(true) {
        power_save.insert(0, "this controller".into());
    }
    if !power_save.is_empty() {
        return Some(check(
            "sync",
            "Timing",
            Status::Warn,
            format!(
                "Wi-Fi power saving is on for {}: packets can be delayed by up to a second. \
                 Update PixelPlus or run `sudo iw dev wlan0 set power_save off`.",
                list_names(&power_save)
            ),
        ));
    }
    if !has_followers {
        return None;
    }
    let with_q: Vec<_> = followers
        .iter()
        .filter_map(|n| n.sync.map(|q| (n, q)))
        .collect();
    let poor: Vec<String> = with_q
        .iter()
        .filter(|(_, q)| q.offset_error_ms > 5.0 || q.loss_pct > 10.0)
        .map(|(n, q)| {
            format!(
                "{} (±{:.1} ms, {:.0} % lost)",
                n.name, q.offset_error_ms, q.loss_pct
            )
        })
        .collect();
    if !poor.is_empty() {
        return Some(check(
            "sync",
            "Timing",
            Status::Warn,
            format!(
                "Weak network timing: {}. Move the access point closer, use 5 GHz or Ethernet.",
                list_names(&poor)
            ),
        ));
    }
    let worst = with_q
        .iter()
        .map(|(_, q)| q.offset_error_ms)
        .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))));
    Some(match worst {
        Some(w) => check(
            "sync",
            "Timing",
            Status::Ok,
            format!("Controllers keep time within ±{w:.1} ms"),
        ),
        None => check(
            "sync",
            "Timing",
            Status::Ok,
            "Waiting for timing reports from the controllers",
        ),
    })
}

/// Strings longer than the boot-time DPI mode (only reported when there is a problem).
pub fn geometry_check(g: &super::geometry::OutputGeometry) -> Option<Check> {
    if g.ok {
        return None;
    }
    let detail = g
        .message
        .clone()
        .unwrap_or_else(|| "A string is longer than the pixel output allows".into());
    let mut c = check("geometry", "String length", Status::Fail, detail);
    c.action = if g.can_apply {
        Some("applyOutputGeometry".into())
    } else if g.pending_reboot {
        Some("reboot".into())
    } else {
        None
    };
    Some(c)
}

/// The player's "No sound: … The lights keep playing." status error.
fn is_sound_problem(error: &str) -> bool {
    error.starts_with("No sound")
}

/// `PIXELPLUS_AUDIO=none`: the player never opens a sound card, so don't look for one.
fn audio_off_check() -> Check {
    check(
        "audio",
        "Audio output",
        Status::Warn,
        "Audio is turned off on this controller (PIXELPLUS_AUDIO=none); shows play without sound",
    )
}

/// A real-time clock chip (the difftxlarge's DS3231) registered with the kernel.
fn board_rtc_present(sys_class_rtc: &std::path::Path) -> bool {
    std::fs::read_dir(sys_class_rtc)
        .map(|dir| {
            dir.flatten().any(|e| {
                std::fs::read_to_string(e.path().join("name"))
                    .map(|n| {
                        let n = n.to_ascii_lowercase();
                        n.contains("ds1307") || n.contains("ds3231")
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

async fn audio_check(device: &str) -> Check {
    if !have("aplay") {
        return if in_docker() {
            check(
                "audio",
                "Audio output",
                Status::Warn,
                "No sound card in this container; audio won't play",
            )
        } else {
            check(
                "audio",
                "Audio output",
                Status::Warn,
                "Audio tools (alsa-utils) aren't installed",
            )
        };
    }
    let cards = std::fs::read_to_string("/proc/asound/cards").unwrap_or_default();
    if cards.trim().is_empty() || cards.contains("no soundcards") {
        return check(
            "audio",
            "Audio output",
            Status::Fail,
            "No audio device found. Plug in the USB sound card or enable the headphone jack.",
        );
    }
    if device == "default" || device.is_empty() {
        return check("audio", "Audio output", Status::Ok, "System default output");
    }
    match run("aplay", &["-L"], Duration::from_secs(5)).await {
        Ok(o) if o.stdout.lines().any(|l| l.trim() == device) => check(
            "audio",
            "Audio output",
            Status::Ok,
            format!("{device} is connected"),
        ),
        Ok(_) => check(
            "audio",
            "Audio output",
            Status::Fail,
            format!("The chosen audio output ({device}) isn't connected"),
        ),
        Err(e) => check("audio", "Audio output", Status::Warn, e),
    }
}

async fn clock_check(state: &AppState) -> Check {
    if have("timedatectl") {
        if let Ok(o) = run(
            "timedatectl",
            &["show", "-p", "NTPSynchronized", "--value"],
            Duration::from_secs(5),
        )
        .await
        {
            if o.stdout.trim() == "yes" {
                return check(
                    "clock",
                    "Clock",
                    Status::Ok,
                    "Synchronized with internet time",
                );
            }
        }
    }
    let (board, _) = super::system::effective_board(state);
    if board == pixelplus_core::model::BoardKind::Difftxlarge
        && board_rtc_present(std::path::Path::new("/sys/class/rtc"))
    {
        return check(
            "clock",
            "Clock",
            Status::Ok,
            "Kept by the board's real-time clock",
        );
    }
    if in_docker() || !cfg!(target_os = "linux") {
        return check("clock", "Clock", Status::Ok, "Using the computer's clock");
    }
    check(
        "clock",
        "Clock",
        Status::Warn,
        "Not synchronized with internet time; show times may be off",
    )
}

/// Run every check, store the report, alert on failure.
pub async fn run_checks(state: &AppState, alert: bool) -> HealthReport {
    let show = state.store.get();
    let data_dir = state.config.data_dir.clone();
    let mut checks = host_checks(state).await;
    let content = tokio::task::spawn_blocking(move || content_checks(&show, &data_dir))
        .await
        .unwrap_or_default();
    checks.extend(content);
    let ok = !checks.iter().any(|c| c.status == Status::Fail);
    let report = HealthReport {
        ok,
        ran_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        checks,
    };
    *state.services.health.last.lock() = Some(report.clone());
    if !ok && alert {
        let failed: Vec<String> = report
            .checks
            .iter()
            .filter(|c| c.status == Status::Fail)
            .map(|c| format!("{}: {}", c.label, c.detail))
            .collect();
        super::alerts::raise(
            state,
            "health",
            super::alerts::Severity::Warning,
            "Pre-show check failed",
            &failed.join("\n"),
        )
        .await;
    }
    report
}

/// Check 15 minutes before each scheduled show.
pub fn start(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            let show = state.store.get();
            let Ok(tz) = pixelplus_core::schedule::schedule_timezone(&show.schedule) else {
                continue;
            };
            let now = chrono::Utc::now().with_timezone(&tz);
            let Some(next) = pixelplus_core::schedule::next_show(&show.schedule, now) else {
                continue;
            };
            let until = next.start.signed_duration_since(now);
            if until <= chrono::Duration::minutes(15) && until > chrono::Duration::zero() {
                let key = next.start.to_rfc3339();
                let already =
                    state.services.health.checked_show.lock().as_deref() == Some(key.as_str());
                if !already {
                    *state.services.health.checked_show.lock() = Some(key);
                    tracing::info!("Running the pre-show check for \"{}\"", next.name);
                    run_checks(&state, true).await;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixelplus_core::model::*;

    #[test]
    fn content_problems_are_found() {
        let dir = std::env::temp_dir().join(format!("pp-health-{}", new_id()));
        std::fs::create_dir_all(dir.join("sequences")).unwrap();
        std::fs::write(dir.join("sequences/a.fseq"), b"x").unwrap();
        let mut s = Show::default();
        let seq = |id: &str| Sequence {
            generated: Default::default(),
            tags: Default::default(),
            id: id.into(),
            name: id.to_uppercase(),
            file: format!("sequences/{id}.fseq"),
            duration_ms: 1,
            frame_ms: 50,
            channel_count: 3,
            media_id: None,
            xlights_name: None,
            thumbnail: None,
            hash: String::new(),
        };
        s.sequences = vec![seq("a"), seq("b")];
        s.playlists.push(Playlist {
            smart: Default::default(),
            id: "p".into(),
            name: "Main".into(),
            items: vec![
                PlaylistItem::Sequence {
                    id: "1".into(),
                    sequence_id: "a".into(),
                },
                PlaylistItem::Sequence {
                    id: "2".into(),
                    sequence_id: "b".into(),
                },
            ],
            intro: vec![],
            outro: vec![],
            shuffle: false,
            repeat: true,
            crossfade_ms: 0,
        });
        let checks = content_checks(&s, &dir);
        let seqs = checks.iter().find(|c| c.id == "sequences").unwrap();
        assert_eq!(seqs.status, Status::Fail);
        assert!(seqs.detail.contains("missing for B"), "{}", seqs.detail);
        assert!(seqs.detail.contains("No audio"), "{}", seqs.detail);
        let sched = checks.iter().find(|c| c.id == "schedule").unwrap();
        assert_eq!(sched.status, Status::Warn);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn sunset_schedule_needs_a_location() {
        let mut s = Show::default();
        s.schedule.enabled = true;
        s.schedule.entries.push(ScheduleEntry {
            start_exact: Default::default(),
            id: "e".into(),
            name: "Nightly".into(),
            enabled: true,
            playlist_id: "p".into(),
            days: vec![Weekday::Fri],
            date_range: None,
            start: TimeSpec::Sunset { offset_min: 0 },
            end: TimeSpec::Clock {
                time: "22:00".into(),
            },
            priority: 0,
            end_behavior: EndBehavior::FinishSong,
        });
        assert_eq!(location_check(&s).map(|c| c.status), Some(Status::Warn));
        s.schedule.location.lat = 52.52;
        s.schedule.location.lon = 13.40;
        assert!(location_check(&s).is_none());
        // Clock times don't care where the show is.
        s.schedule.location = Location::default();
        s.schedule.entries[0].start = TimeSpec::Clock {
            time: "17:00".into(),
        };
        assert!(location_check(&s).is_none());
    }
}

#[cfg(test)]
mod host_tests {
    use super::*;

    #[test]
    fn rtc_is_only_claimed_when_the_chip_is_registered() {
        let dir = std::env::temp_dir().join(format!("pp-rtc-{}", pixelplus_core::model::new_id()));
        assert!(!board_rtc_present(&dir));
        std::fs::create_dir_all(dir.join("rtc0")).unwrap();
        std::fs::write(dir.join("rtc0/name"), "rtc_cmos\n").unwrap();
        assert!(
            !board_rtc_present(&dir),
            "a PC's CMOS clock isn't the board RTC"
        );
        std::fs::create_dir_all(dir.join("rtc1")).unwrap();
        std::fs::write(dir.join("rtc1/name"), "rtc-ds1307 1-0068\n").unwrap();
        assert!(board_rtc_present(&dir));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn wiring_finds_unwired_partly_wired_and_missing_ports() {
        use pixelplus_core::model::{BoardKind, Node, NodeRole, Prop, PropSegment};
        let mut show = Show::default();
        show.nodes.push(Node {
            hardware_history: Default::default(),
            serial: Default::default(),
            id: "n1".into(),
            name: "Porch".into(),
            hostname: "porch".into(),
            role: NodeRole::Follower,
            board: BoardKind::Difftx,
            board_rev: None,
            pi_model: None,
            outputs: BoardKind::Difftx.default_outputs(),
            adopted: true,
            last_seen: None,
            notes: None,
        });
        let prop = |name: &str, segs: Vec<(u32, u32)>| {
            let mut p: Prop = serde_json::from_value(serde_json::json!({
                "id": name, "name": name, "kind": "line", "pixelCount": 100, "channelStart": 0
            }))
            .unwrap();
            let mut off = 0;
            p.segments = segs
                .into_iter()
                .map(|(output, n)| {
                    let s = PropSegment {
                        node_id: "n1".into(),
                        output,
                        start_pixel: 0,
                        pixel_count: n,
                        prop_offset: off,
                        reverse: false,
                        null_pixels: 0,
                    };
                    off += n;
                    s
                })
                .collect();
            p
        };
        show.props = vec![prop("A", vec![(1, 100)])];
        assert_eq!(wiring_check(&show).status, Status::Ok);
        show.props.push(prop("B", vec![(2, 60)]));
        let c = wiring_check(&show);
        assert_eq!(c.status, Status::Warn);
        assert!(c.detail.contains("B (40 of 100 pixels)"), "{}", c.detail);
        show.props.push(prop("C", vec![]));
        let c = wiring_check(&show);
        assert!(c.detail.starts_with("C not wired"), "{}", c.detail);
        show.props.push(prop("D", vec![(5, 100)]));
        let c = wiring_check(&show);
        assert_eq!(c.status, Status::Fail);
        assert!(
            c.detail.contains("port 5 doesn't exist on Porch"),
            "{}",
            c.detail
        );
    }

    #[test]
    fn sound_problems_are_not_light_output_problems() {
        assert!(is_sound_problem(
            "No sound: the device is gone. The lights keep playing."
        ));
        assert!(!is_sound_problem("The pixel output stopped"));
    }

    #[test]
    fn audio_off_is_a_warning_not_a_failure() {
        let c = audio_off_check();
        assert_eq!(c.status, Status::Warn);
        assert!(c.detail.contains("turned off"));
    }
}

#[cfg(test)]
mod geometry_tests {
    use super::*;
    use crate::player::GeometryStatus;
    use crate::services::geometry::evaluate;

    #[test]
    fn sync_check_reports_timing_problems() {
        use crate::cluster::proto::{FileProgress, SyncQuality, SyncState, PROTOCOL_VERSION};
        use crate::cluster::NodeStatus;
        use pixelplus_core::model::{BoardKind, NodeRole};
        let node = |name: &str, q: Option<SyncQuality>, ps: Option<bool>, proto: u32| NodeStatus {
            id: name.into(),
            name: name.into(),
            role: NodeRole::Follower,
            adopted: true,
            online: true,
            last_seen: None,
            board: BoardKind::Difftx,
            sync_offset_ms: 0.0,
            sync_state: SyncState::Synced,
            sync: q,
            wifi_power_save: ps,
            protocol: proto,
            files: FileProgress::default(),
            ip: None,
            version: None,
            pi_model: None,
            hostname: String::new(),
            problem: None,
            limiter: None,
        };
        let good = SyncQuality {
            offset_error_ms: 0.4,
            loss_pct: 1.0,
            ..Default::default()
        };
        let c = sync_check(
            &[node("Garage", Some(good), Some(false), PROTOCOL_VERSION)],
            Some(false),
            true,
        )
        .unwrap();
        assert_eq!(c.status, Status::Ok);
        assert!(c.detail.contains("±0.4 ms"), "{}", c.detail);
        let c = sync_check(
            &[node("Garage", Some(good), Some(true), PROTOCOL_VERSION)],
            None,
            true,
        )
        .unwrap();
        assert_eq!(c.status, Status::Warn);
        assert!(c.detail.contains("power saving") && c.detail.contains("Garage"));
        let bad = SyncQuality {
            offset_error_ms: 9.0,
            loss_pct: 30.0,
            ..Default::default()
        };
        let c = sync_check(
            &[node("Tree", Some(bad), None, PROTOCOL_VERSION)],
            None,
            true,
        )
        .unwrap();
        assert_eq!(c.status, Status::Warn);
        assert!(
            c.detail.contains("Tree (±9.0 ms, 30 % lost)"),
            "{}",
            c.detail
        );
        let c = sync_check(&[node("Old", None, None, 1)], None, true).unwrap();
        assert_eq!(c.status, Status::Fail);
        // A single controller: only its own power save matters.
        assert!(sync_check(&[], Some(false), false).is_none());
        assert_eq!(
            sync_check(&[], Some(true), false).unwrap().status,
            Status::Warn
        );
    }

    #[test]
    fn geometry_check_offers_the_fix() {
        let fine = GeometryStatus {
            ok: true,
            longest_string: 100,
            max_pixels: Some(800),
            ..Default::default()
        };
        assert!(geometry_check(&evaluate(&fine, None, None, true, true)).is_none());
        let long = GeometryStatus {
            ok: false,
            longest_string: 1000,
            max_pixels: Some(800),
            needed_pixels: Some(1000),
            message: None,
        };
        let c = geometry_check(&evaluate(&long, Some(800), Some(2041), true, true)).unwrap();
        assert_eq!(c.status, Status::Fail);
        assert_eq!(c.action.as_deref(), Some("applyOutputGeometry"));
        let c = geometry_check(&evaluate(&long, Some(1000), Some(2041), true, true)).unwrap();
        assert_eq!(c.action.as_deref(), Some("reboot"));
        let c = geometry_check(&evaluate(&long, None, None, false, true)).unwrap();
        assert_eq!(c.action, None);
        let v = serde_json::to_value(&c).unwrap();
        assert!(v.get("action").is_none());
    }
}
