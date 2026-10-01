//! MQTT / Home Assistant integration.
//!
//! When `settings.mqtt.enabled`, PixelPlus connects to the broker, publishes
//! its state under `baseTopic` and (optionally) Home Assistant discovery
//! configs, and listens for commands:
//!
//! | topic | payload | effect |
//! |---|---|---|
//! | `<base>/show/set` | `ON` / `OFF` | play the scheduled (or first) playlist / stop |
//! | `<base>/playlist/set` | playlist name | play that playlist |
//! | `<base>/next/press`, `<base>/stop/press` | anything | next item / stop |
//! | `<base>/light/set` | HA JSON light `{"state","brightness"}` (0–100) | blackout / brightness |
//! | `<base>/volume/set` | 0–100 | volume |
//! | `<base>/trigger/<id>/press` | anything | fire that HTTP / GPIO trigger (through its gates) |
//!
//! With Home Assistant discovery on, every HTTP and GPIO trigger is also an
//! HA `button` ("Trigger: <name>") on that command topic, so Home Assistant
//! needs no trigger-link token; buttons of removed triggers are withdrawn
//! (empty retained config) and all of them disappear while *Buttons &
//! triggers* is off.
//!
//! State: `<base>/availability` (online/offline, retained LWT), `<base>/state`
//! (JSON), `<base>/show/state`, `<base>/playlist/state`, `<base>/now_playing`,
//! `<base>/player_state`, `<base>/light/state`, `<base>/volume/state`,
//! `<base>/sensor/<id>`. The client reconnects whenever the settings change.

use crate::events::Event;
use crate::player::{PlayRequest, PlayerCmd, PlayerState, PlayerStatus};
use crate::state::AppState;
use parking_lot::Mutex;
use pixelplus_core::model::{FeatureId, MqttSettings, Show, TriggerKind};
use rumqttc::{AsyncClient, EventLoop, LastWill, MqttOptions, Packet, QoS};
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Default)]
pub struct MqttState {
    status: Mutex<(bool, Option<String>)>,
}

impl MqttState {
    /// (connected, last error)
    pub fn status(&self) -> (bool, Option<String>) {
        self.status.lock().clone()
    }
    fn set(&self, connected: bool, err: Option<String>) {
        *self.status.lock() = (connected, err);
    }
}

pub fn base_topic(s: &MqttSettings) -> String {
    let b = s.base_topic.trim().trim_matches('/');
    if b.is_empty() {
        "pixelplus".into()
    } else {
        b.to_string()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Show(bool),
    Playlist(String),
    Next,
    Stop,
    Light {
        on: Option<bool>,
        brightness: Option<u8>,
    },
    Volume(u8),
    /// Fire a trigger (by id).
    Trigger(String),
}

pub fn parse_command(base: &str, topic: &str, payload: &[u8]) -> Option<Command> {
    let rest = topic.strip_prefix(base)?.strip_prefix('/')?;
    let text = String::from_utf8_lossy(payload).trim().to_string();
    match rest {
        "show/set" => match text.to_ascii_uppercase().as_str() {
            "ON" | "1" | "TRUE" => Some(Command::Show(true)),
            "OFF" | "0" | "FALSE" => Some(Command::Show(false)),
            _ => None,
        },
        "playlist/set" if !text.is_empty() => Some(Command::Playlist(text)),
        "next/press" => Some(Command::Next),
        _ if rest.starts_with("trigger/") && rest.ends_with("/press") => {
            let id = &rest["trigger/".len()..rest.len() - "/press".len()];
            topic_safe(id).then(|| Command::Trigger(id.to_string()))
        }
        "stop/press" => Some(Command::Stop),
        "volume/set" => text
            .parse::<f32>()
            .ok()
            .map(|v| Command::Volume(v.clamp(0.0, 100.0).round() as u8)),
        "light/set" => {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                let on = v["state"].as_str().map(|s| s.eq_ignore_ascii_case("ON"));
                let brightness = v["brightness"]
                    .as_f64()
                    .map(|b| b.clamp(0.0, 100.0).round() as u8);
                Some(Command::Light { on, brightness })
            } else {
                match text.to_ascii_uppercase().as_str() {
                    "ON" => Some(Command::Light {
                        on: Some(true),
                        brightness: None,
                    }),
                    "OFF" => Some(Command::Light {
                        on: Some(false),
                        brightness: None,
                    }),
                    _ => None,
                }
            }
        }
        _ => None,
    }
}

fn sensor_class(kind: &str) -> Option<&'static str> {
    match kind {
        "temperature" => Some("temperature"),
        "voltage" => Some("voltage"),
        "current" => Some("current"),
        "power" => Some("power"),
        _ => None,
    }
}

/// Trigger ids that can be used in a topic (no `/`, `+`, `#`, spaces…).
fn topic_safe(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The triggers Home Assistant gets a button for: HTTP and GPIO ones, while
/// *Buttons & triggers* is on.
pub fn button_triggers(show: &Show) -> Vec<&pixelplus_core::model::Trigger> {
    if !show.feature(FeatureId::Triggers) {
        return vec![];
    }
    show.settings
        .triggers
        .iter()
        .filter(|t| matches!(t.kind, TriggerKind::Http | TriggerKind::Gpio) && topic_safe(&t.id))
        .collect()
}

/// Discovery topic of a trigger's button.
pub fn trigger_button_topic(node_id: &str, trigger_id: &str) -> String {
    format!("homeassistant/button/pixelplus_{node_id}/trigger_{trigger_id}/config")
}

/// Home Assistant discovery messages (topic, retained JSON payload).
pub fn discovery_messages(
    base: &str,
    node_id: &str,
    show: &Show,
    sensors: &Value,
) -> Vec<(String, String)> {
    let uid = format!("pixelplus_{node_id}");
    let device = json!({
        "identifiers": [uid],
        "name": show.name,
        "manufacturer": "PixelPlus",
        "model": "PixelPlus show controller",
        "sw_version": env!("CARGO_PKG_VERSION"),
    });
    let avail = format!("{base}/availability");
    let mut out = Vec::new();
    let mut add = |component: &str, object: &str, mut cfg: Value| {
        cfg["unique_id"] = json!(format!("{uid}_{object}"));
        cfg["object_id"] = json!(format!("{}_{object}", base.replace('/', "_")));
        cfg["availability_topic"] = json!(avail);
        cfg["device"] = device.clone();
        out.push((
            format!("homeassistant/{component}/{uid}/{object}/config"),
            cfg.to_string(),
        ));
    };
    add(
        "switch",
        "show",
        json!({ "name": "Show", "icon": "mdi:string-lights", "state_topic": format!("{base}/show/state"), "command_topic": format!("{base}/show/set") }),
    );
    let options: Vec<&str> = show.playlists.iter().map(|p| p.name.as_str()).collect();
    if !options.is_empty() {
        add(
            "select",
            "playlist",
            json!({ "name": "Playlist", "icon": "mdi:playlist-music", "state_topic": format!("{base}/playlist/state"), "command_topic": format!("{base}/playlist/set"), "options": options }),
        );
    }
    add(
        "sensor",
        "now_playing",
        json!({ "name": "Now playing", "icon": "mdi:music", "state_topic": format!("{base}/now_playing") }),
    );
    add(
        "sensor",
        "player_state",
        json!({ "name": "Player", "icon": "mdi:play-pause", "state_topic": format!("{base}/player_state") }),
    );
    add(
        "button",
        "next",
        json!({ "name": "Next song", "icon": "mdi:skip-next", "command_topic": format!("{base}/next/press") }),
    );
    add(
        "button",
        "stop",
        json!({ "name": "Stop show", "icon": "mdi:stop", "command_topic": format!("{base}/stop/press") }),
    );
    add(
        "light",
        "lights",
        json!({ "name": "Lights", "schema": "json", "brightness": true, "brightness_scale": 100, "state_topic": format!("{base}/light/state"), "command_topic": format!("{base}/light/set") }),
    );
    add(
        "number",
        "volume",
        json!({ "name": "Volume", "icon": "mdi:volume-high", "min": 0, "max": 100, "step": 1, "unit_of_measurement": "%", "state_topic": format!("{base}/volume/state"), "command_topic": format!("{base}/volume/set") }),
    );
    for t in button_triggers(show) {
        add(
            "button",
            &format!("trigger_{}", t.id),
            json!({
                "name": format!("Trigger: {}", t.name),
                "icon": if t.kind == TriggerKind::Gpio { "mdi:gesture-tap-button" } else { "mdi:lightning-bolt" },
                "command_topic": format!("{base}/trigger/{}/press", t.id),
            }),
        );
    }
    for s in sensors.as_array().into_iter().flatten() {
        let Some(id) = s["id"].as_str() else { continue };
        let mut cfg = json!({
            "name": s["label"].as_str().unwrap_or(id),
            "state_topic": format!("{base}/sensor/{id}"),
            "unit_of_measurement": s["unit"].as_str().unwrap_or(""),
            "state_class": "measurement",
        });
        if let Some(dc) = s["kind"].as_str().and_then(sensor_class) {
            cfg["device_class"] = json!(dc);
        }
        add("sensor", &format!("sensor_{id}"), cfg);
    }
    out
}

/// State messages for a player status (topic, payload, retained).
pub fn state_messages(base: &str, st: &PlayerStatus) -> Vec<(String, String)> {
    let on = matches!(
        st.state,
        PlayerState::Playing | PlayerState::Paused | PlayerState::Effect
    );
    let state_name = serde_json::to_value(st.state)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    let now = st
        .item
        .as_ref()
        .map(|i| i.name.clone())
        .unwrap_or_else(|| "Nothing".into());
    vec![
        (
            format!("{base}/state"),
            serde_json::to_string(st).unwrap_or_default(),
        ),
        (
            format!("{base}/show/state"),
            if on { "ON" } else { "OFF" }.into(),
        ),
        (
            format!("{base}/playlist/state"),
            st.playlist
                .as_ref()
                .map(|p| p.name.clone())
                .unwrap_or_default(),
        ),
        (format!("{base}/now_playing"), now),
        (format!("{base}/player_state"), state_name),
        (
            format!("{base}/light/state"),
            json!({ "state": if st.blackout { "OFF" } else { "ON" }, "brightness": st.brightness })
                .to_string(),
        ),
        (format!("{base}/volume/state"), st.volume.to_string()),
    ]
}

fn options(s: &MqttSettings, node_id: &str, base: &str) -> MqttOptions {
    let mut o = MqttOptions::new(format!("pixelplus-{node_id}"), s.host.trim(), s.port);
    o.set_keep_alive(Duration::from_secs(30));
    if let Some(u) = s.username.as_deref().filter(|u| !u.is_empty()) {
        o.set_credentials(u, s.password.clone().unwrap_or_default());
    }
    o.set_last_will(LastWill::new(
        format!("{base}/availability"),
        "offline",
        QoS::AtLeastOnce,
        true,
    ));
    o
}

/// Default playlist for "show on": the one scheduled now, the next
/// scheduled, else the first.
fn default_playlist(show: &Show) -> Option<String> {
    let s = &show.schedule;
    if let Ok(tz) = pixelplus_core::schedule::schedule_timezone(s) {
        let now = chrono::Utc::now().with_timezone(&tz);
        if let Some(o) = pixelplus_core::schedule::active_at(s, now)
            .or_else(|| pixelplus_core::schedule::next_show(s, now))
        {
            return Some(o.playlist_id);
        }
    }
    s.entries
        .iter()
        .find(|e| e.enabled)
        .map(|e| e.playlist_id.clone())
        .or_else(|| show.playlists.first().map(|p| p.id.clone()))
}

async fn execute(state: &AppState, cmd: Command) {
    let Some(p) = state.services.player.get() else {
        return;
    };
    let show = state.store.get();
    let play = |playlist_id: String| PlayRequest {
        loop_until_stopped: Default::default(),
        playlist_id: Some(playlist_id),
        sequence_id: None,
        dj_clip_id: None,
        effect_id: None,
        media_id: None,
        start_index: None,
    };
    let res = match cmd {
        Command::Show(true) => match default_playlist(&show) {
            Some(id) => p.play(play(id)).await,
            None => Ok(()),
        },
        Command::Show(false) | Command::Stop => p.send(PlayerCmd::Stop { fade: true }).await,
        Command::Playlist(name) => match show
            .playlists
            .iter()
            .find(|pl| pl.name == name || pl.id == name)
        {
            Some(pl) => p.play(play(pl.id.clone())).await,
            None => Ok(()),
        },
        Command::Next => p.send(PlayerCmd::Next).await,
        Command::Trigger(id) => {
            let Some(t) = button_triggers(&show)
                .into_iter()
                .find(|t| t.id == id)
                .cloned()
            else {
                tracing::info!("MQTT: no trigger \"{id}\" to press");
                return;
            };
            match crate::services::triggers::fire_trigger(state, &t, "mqtt").await {
                Ok(msg) => tracing::info!("MQTT button \"{}\": {msg}", t.name),
                Err(e) => tracing::info!("MQTT button \"{}\": {}", t.name, e.message),
            }
            Ok(())
        }
        Command::Volume(v) => p.send(PlayerCmd::SetVolume(v)).await,
        Command::Light { on, brightness } => {
            if let Some(b) = brightness {
                let _ = p.send(PlayerCmd::SetBrightness(b)).await;
            }
            match on {
                Some(on) => p.send(PlayerCmd::Blackout(!on)).await,
                None => Ok(()),
            }
        }
    };
    if let Err(e) = res {
        tracing::warn!("MQTT command failed: {}", e.message);
    }
}

fn publish(client: &AsyncClient, msgs: Vec<(String, String)>, retain: bool) {
    for (t, p) in msgs {
        let _ = client.try_publish(t, QoS::AtLeastOnce, retain, p);
    }
}

/// Connect once with the given settings (for `POST /mqtt/test`).
pub async fn test_connection(s: &MqttSettings, node_id: &str) -> Result<String, String> {
    if s.host.trim().is_empty() {
        return Err("Enter your MQTT broker's address first.".into());
    }
    let base = base_topic(s);
    let mut o = options(s, &format!("{node_id}-test"), &base);
    o.set_clean_session(true);
    let (client, mut ev) = AsyncClient::new(o, 10);
    let res = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            match ev.poll().await {
                Ok(rumqttc::Event::Incoming(Packet::ConnAck(_))) => return Ok(()),
                Ok(_) => {}
                Err(e) => return Err(friendly_error(&e)),
            }
        }
    })
    .await
    .unwrap_or_else(|_| Err(format!("{}:{} didn't answer.", s.host, s.port)));
    let _ = client.try_disconnect();
    res.map(|_| format!("Connected to {}:{}.", s.host.trim(), s.port))
}

fn friendly_error(e: &rumqttc::ConnectionError) -> String {
    match e {
        rumqttc::ConnectionError::ConnectionRefused(code) => match format!("{code:?}").as_str() {
            "BadUserNamePassword" | "NotAuthorized" => {
                "The broker rejected the username or password.".into()
            }
            other => format!("The broker refused the connection ({other})."),
        },
        rumqttc::ConnectionError::Io(io) => format!("Couldn't reach the broker: {io}"),
        other => format!("MQTT error: {other}"),
    }
}

async fn session(state: &AppState, s: MqttSettings) {
    let node_id = state.identity().id;
    let base = base_topic(&s);
    let (client, mut ev): (AsyncClient, EventLoop) =
        AsyncClient::new(options(&s, &node_id, &base), 256);
    let mut events = state.events.subscribe();
    let mut show_rx = state.store.subscribe();
    let mut player_rx = state.services.player.get().map(|p| p.watch());
    let mut last_status: Option<PlayerStatus> = None;
    let mut backoff = Duration::from_secs(2);
    let mut throttle = tokio::time::interval(Duration::from_secs(1));
    let mut dirty = false;
    // Trigger buttons announced to Home Assistant (to withdraw removed ones).
    let mut buttons: Vec<String> = Vec::new();
    loop {
        tokio::select! {
            r = ev.poll() => match r {
                Ok(rumqttc::Event::Incoming(Packet::ConnAck(_))) => {
                    backoff = Duration::from_secs(2);
                    state.services.mqtt.set(true, None);
                    tracing::info!("MQTT connected to {}:{}", s.host, s.port);
                    let _ = client.try_subscribe(format!("{base}/+/set"), QoS::AtLeastOnce);
                    let _ = client.try_subscribe(format!("{base}/+/press"), QoS::AtLeastOnce);
                    let _ = client.try_subscribe(format!("{base}/trigger/+/press"), QoS::AtLeastOnce);
                    publish(&client, vec![(format!("{base}/availability"), "online".into())], true);
                    if s.home_assistant_discovery {
                        let sensors = latest_sensors(state);
                        let show = state.store.get();
                        publish(&client, withdrawn_buttons(&node_id, &mut buttons, &show), true);
                        publish(&client, discovery_messages(&base, &node_id, &show, &sensors), true);
                    }
                    if let Some(p) = state.services.player.get() {
                        publish(&client, state_messages(&base, &p.status()), true);
                    }
                }
                Ok(rumqttc::Event::Incoming(Packet::Publish(p))) => {
                    if let Some(cmd) = parse_command(&base, &p.topic, &p.payload) {
                        execute(state, cmd).await;
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    let msg = friendly_error(&e);
                    let was = state.services.mqtt.status();
                    if was.1.as_deref() != Some(msg.as_str()) {
                        tracing::warn!("MQTT: {msg}");
                    }
                    state.services.mqtt.set(false, Some(msg));
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                }
            },
            r = show_rx.changed() => {
                if r.is_err() { return; }
                let show = state.store.get();
                if !show.settings.mqtt.enabled
                    || !show.feature(FeatureId::Mqtt)
                    || show.settings.mqtt != s
                {
                    publish(&client, vec![(format!("{base}/availability"), "offline".into())], true);
                    let _ = client.try_disconnect();
                    // Let the disconnect go out.
                    let _ = tokio::time::timeout(Duration::from_millis(500), ev.poll()).await;
                    state.services.mqtt.set(false, None);
                    return;
                }
                if s.home_assistant_discovery {
                    let sensors = latest_sensors(state);
                    publish(&client, withdrawn_buttons(&node_id, &mut buttons, &show), true);
                    publish(&client, discovery_messages(&base, &node_id, &show, &sensors), true);
                }
            },
            ev2 = events.recv() => {
                if let Ok(Event::Json { kind: "sensors", data }) = ev2 {
                    let msgs = data.as_array().into_iter().flatten().filter_map(|r| {
                        Some((format!("{base}/sensor/{}", r["id"].as_str()?), r["value"].to_string()))
                    }).collect();
                    publish(&client, msgs, false);
                }
            },
            r = async { match player_rx.as_mut() { Some(rx) => rx.changed().await, None => std::future::pending().await } } => {
                if r.is_err() { player_rx = None; } else { dirty = true; }
            },
            _ = throttle.tick() => {
                if player_rx.is_none() {
                    player_rx = state.services.player.get().map(|p| p.watch());
                    dirty = player_rx.is_some();
                }
                if dirty {
                    dirty = false;
                    if let Some(p) = state.services.player.get() {
                        let st = p.status();
                        // Position ticks alone don't need a publish.
                        let changed = last_status.as_ref().differs_from(&st);
                        if changed {
                            publish(&client, state_messages(&base, &st), true);
                            last_status = Some(st);
                        }
                    }
                }
            },
        }
    }
}

/// Empty retained configs for trigger buttons announced before that are gone
/// now (trigger removed, kind changed, triggers turned off); `announced`
/// becomes the current set.
pub fn withdrawn_buttons(
    node_id: &str,
    announced: &mut Vec<String>,
    show: &Show,
) -> Vec<(String, String)> {
    let now: Vec<String> = button_triggers(show)
        .into_iter()
        .map(|t| t.id.clone())
        .collect();
    let gone = announced
        .iter()
        .filter(|id| !now.contains(id))
        .map(|id| (trigger_button_topic(node_id, id), String::new()))
        .collect();
    *announced = now;
    gone
}

trait StatusDiff {
    fn differs_from(self, st: &PlayerStatus) -> bool;
}

impl StatusDiff for Option<&PlayerStatus> {
    fn differs_from(self, st: &PlayerStatus) -> bool {
        match self {
            None => true,
            Some(o) => {
                o.state != st.state
                    || o.item != st.item
                    || o.playlist.as_ref().map(|p| &p.id) != st.playlist.as_ref().map(|p| &p.id)
                    || o.volume != st.volume
                    || o.brightness != st.brightness
                    || o.blackout != st.blackout
                    || o.pos_ms.abs_diff(st.pos_ms) > 10_000
            }
        }
    }
}

fn latest_sensors(state: &AppState) -> Value {
    serde_json::to_value(state.services.sensors.latest()).unwrap_or(Value::Null)
}

/// Keep a session running while MQTT is enabled; restart on settings changes.
pub fn start(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let mut rx = state.store.subscribe();
        loop {
            let show = state.store.get();
            let s = show.settings.mqtt.clone();
            // Off in Settings → Features: disconnected (the settings are kept).
            if s.enabled && show.feature(FeatureId::Mqtt) && !s.host.trim().is_empty() {
                session(&state, s).await;
            } else {
                state.services.mqtt.set(false, None);
                if rx.changed().await.is_err() {
                    return;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands() {
        assert_eq!(
            parse_command("pp", "pp/show/set", b"ON"),
            Some(Command::Show(true))
        );
        assert_eq!(
            parse_command("pp", "pp/show/set", b"off"),
            Some(Command::Show(false))
        );
        assert_eq!(
            parse_command("pp", "pp/playlist/set", b"Main Show"),
            Some(Command::Playlist("Main Show".into()))
        );
        assert_eq!(
            parse_command("pp", "pp/volume/set", b"55.4"),
            Some(Command::Volume(55))
        );
        assert_eq!(
            parse_command("pp", "pp/light/set", br#"{"state":"ON","brightness":40}"#),
            Some(Command::Light {
                on: Some(true),
                brightness: Some(40)
            })
        );
        assert_eq!(parse_command("pp", "other/show/set", b"ON"), None);
        assert_eq!(
            parse_command("pp", "pp/next/press", b"PRESS"),
            Some(Command::Next)
        );
        assert_eq!(
            parse_command("pp", "pp/trigger/tr_1-a/press", b"PRESS"),
            Some(Command::Trigger("tr_1-a".into()))
        );
        assert_eq!(parse_command("pp", "pp/trigger//press", b""), None);
        assert_eq!(parse_command("pp", "pp/trigger/a/b/press", b""), None);
    }

    #[test]
    fn discovery() {
        let mut show = Show::default();
        show.playlists.push(pixelplus_core::model::Playlist {
            smart: Default::default(),
            id: "p".into(),
            name: "Main Show".into(),
            items: vec![],
            intro: vec![],
            outro: vec![],
            shuffle: false,
            repeat: true,
            crossfade_ms: 0,
        });
        let sensors = json!([{"id": "cpuTemp", "label": "CPU temperature", "kind": "temperature", "value": 50.0, "unit": "°C"}]);
        let msgs = discovery_messages("pixelplus", "abc", &show, &sensors);
        let sel = msgs
            .iter()
            .find(|(t, _)| t == "homeassistant/select/pixelplus_abc/playlist/config")
            .unwrap();
        let v: Value = serde_json::from_str(&sel.1).unwrap();
        assert_eq!(v["options"][0], "Main Show");
        assert_eq!(v["command_topic"], "pixelplus/playlist/set");
        let s = msgs
            .iter()
            .find(|(t, _)| t.contains("sensor_cpuTemp"))
            .unwrap();
        assert!(s.1.contains("\"device_class\":\"temperature\""));
        assert!(
            !msgs.iter().any(|(t, _)| t.contains("/trigger_")),
            "no triggers, no buttons"
        );
        let st = state_messages("pixelplus", &PlayerStatus::default());
        assert!(st
            .iter()
            .any(|(t, p)| t == "pixelplus/show/state" && p == "OFF"));
    }

    #[test]
    fn triggers_become_home_assistant_buttons() {
        use pixelplus_core::model::Trigger;
        let t = |id: &str, name: &str, kind: TriggerKind| -> Trigger {
            serde_json::from_value(json!({
                "id": id, "name": name, "kind": kind, "action": {"type": "stop"}
            }))
            .unwrap()
        };
        let mut show = Show::default();
        show.settings.triggers = vec![
            t("door1", "Doorbell", TriggerKind::Http),
            t("btn1", "Mailbox button", TriggerKind::Gpio),
            t("pir1", "Sidewalk", TriggerKind::Sensor),
            t("bad/id", "Weird", TriggerKind::Http),
        ];
        let msgs = discovery_messages("pp", "abc", &show, &Value::Null);
        let door = msgs
            .iter()
            .find(|(t, _)| *t == trigger_button_topic("abc", "door1"))
            .expect("doorbell button");
        let v: Value = serde_json::from_str(&door.1).unwrap();
        assert_eq!(v["command_topic"], "pp/trigger/door1/press");
        assert_eq!(v["name"], "Trigger: Doorbell");
        assert_eq!(v["unique_id"], "pixelplus_abc_trigger_door1");
        assert!(msgs
            .iter()
            .any(|(t, _)| *t == trigger_button_topic("abc", "btn1")));
        assert!(
            !msgs.iter().any(|(t, _)| t.contains("trigger_pir1")),
            "sensors: no button"
        );
        assert!(!msgs
            .iter()
            .any(|(t, _)| t.contains("Weird") || t.contains("bad")));
        // Pressing it in HA sends this.
        assert_eq!(
            parse_command("pp", "pp/trigger/door1/press", b"PRESS"),
            Some(Command::Trigger("door1".into()))
        );
        // Removed triggers are withdrawn; triggers off withdraws them all.
        let mut announced = Vec::new();
        assert!(withdrawn_buttons("abc", &mut announced, &show).is_empty());
        assert_eq!(announced, vec!["door1", "btn1"]);
        show.settings.triggers.remove(1);
        let gone = withdrawn_buttons("abc", &mut announced, &show);
        assert_eq!(
            gone,
            vec![(trigger_button_topic("abc", "btn1"), String::new())]
        );
        show.settings.features.set(FeatureId::Triggers, false);
        let gone = withdrawn_buttons("abc", &mut announced, &show);
        assert_eq!(gone.len(), 1);
        assert!(announced.is_empty());
        let msgs = discovery_messages("pp", "abc", &show, &Value::Null);
        assert!(!msgs.iter().any(|(t, _)| t.contains("/trigger_")));
    }
}
