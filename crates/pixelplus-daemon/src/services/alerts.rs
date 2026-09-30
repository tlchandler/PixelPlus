//! Alerts by email (SMTP, STARTTLS/TLS) and ntfy, with de-duplication and
//! rate limiting. Rules come from `settings.alerts.rules`: board temperature,
//! 12 V supply voltage, follower offline, show failure (player error) and
//! failed pre-show checks.

use crate::events::{Event, ToastKind};
use crate::state::AppState;
use parking_lot::Mutex;
use pixelplus_core::model::{AlertSettings, EmailSettings, NtfySettings};
use pixelplus_hw::SensorKind;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// The same alert is not repeated within this window.
pub const DEDUP_WINDOW: Duration = Duration::from_secs(30 * 60);
/// At most this many alerts per hour overall.
pub const MAX_PER_HOUR: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Warning,
    Critical,
    Info,
}

#[derive(Default)]
pub struct AlertState {
    last: Mutex<HashMap<String, Instant>>,
    sent: Mutex<VecDeque<Instant>>,
    /// Conditions currently active (so we alert on the transition only).
    active: Mutex<HashMap<String, bool>>,
}

impl AlertState {
    /// Whether an alert with `key` may go out now (records it if so).
    pub fn admit(&self, key: &str, now: Instant) -> bool {
        let mut last = self.last.lock();
        if last.get(key).is_some_and(|t| now.duration_since(*t) < DEDUP_WINDOW) {
            return false;
        }
        let mut sent = self.sent.lock();
        while sent.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(3600)) {
            sent.pop_front();
        }
        if sent.len() >= MAX_PER_HOUR {
            return false;
        }
        sent.push_back(now);
        last.insert(key.to_string(), now);
        true
    }

    /// Track a boolean condition; returns true on a false→true transition.
    pub fn rising(&self, key: &str, on: bool) -> bool {
        let mut a = self.active.lock();
        let was = a.insert(key.to_string(), on).unwrap_or(false);
        on && !was
    }
}

fn show_label(state: &AppState) -> String {
    let show = state.store.get();
    show.name.clone()
}

/// Raise an alert: toast in the UI, and email/ntfy if configured (deduplicated
/// by `key`, rate limited).
pub async fn raise(state: &AppState, key: &str, severity: Severity, title: &str, body: &str) {
    let toast = match severity {
        Severity::Critical => ToastKind::Error,
        Severity::Warning => ToastKind::Warning,
        Severity::Info => ToastKind::Info,
    };
    if !state.services.alerts.admit(key, Instant::now()) {
        return;
    }
    tracing::warn!("Alert: {title}: {body}");
    state.events.toast(toast, format!("{title}: {body}"));
    let settings = state.store.get().settings.alerts.clone();
    let show = show_label(state);
    let subject = format!("[{show}] {title}");
    if let Some(email) = settings.email.clone().filter(|e| !e.smtp_host.is_empty() && !e.to.is_empty()) {
        if let Err(e) = send_email(&email, &subject, body).await {
            tracing::warn!("Couldn't send the alert email: {e}");
        }
    }
    if let Some(ntfy) = settings.ntfy.clone().filter(|n| !n.topic.is_empty()) {
        if let Err(e) = send_ntfy(&ntfy, &subject, body, severity).await {
            tracing::warn!("Couldn't send the ntfy alert: {e}");
        }
    }
}

/// Send a test message through one channel.
pub async fn send_test(state: &AppState, channel: &str) -> Result<String, String> {
    let settings: AlertSettings = state.store.get().settings.alerts.clone();
    let show = show_label(state);
    let subject = format!("[{show}] Test alert");
    let body = "This is a test from PixelPlus. If you can read this, alerts are working.";
    match channel {
        "email" => {
            let email = settings
                .email
                .filter(|e| !e.smtp_host.is_empty())
                .ok_or("Fill in the email settings (server and recipient) first.")?;
            send_email(&email, &subject, body).await?;
            Ok(format!("Test email sent to {}.", email.to))
        }
        "ntfy" => {
            let ntfy = settings.ntfy.filter(|n| !n.topic.is_empty()).ok_or("Choose an ntfy topic first.")?;
            send_ntfy(&ntfy, &subject, body, Severity::Info).await?;
            Ok(format!("Test notification sent to \"{}\".", ntfy.topic))
        }
        _ => Err("Pick email or ntfy.".into()),
    }
}

pub async fn send_email(cfg: &EmailSettings, subject: &str, body: &str) -> Result<(), String> {
    use lettre::message::header::ContentType;
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

    let from_addr = if cfg.from.trim().is_empty() { cfg.username.trim() } else { cfg.from.trim() };
    let from = from_addr
        .parse()
        .map_err(|_| format!("\"{from_addr}\" isn't a valid sender address."))?;
    let mut builder = Message::builder().from(from).subject(subject);
    for to in cfg.to.split([',', ';']).map(str::trim).filter(|s| !s.is_empty()) {
        builder = builder.to(to.parse().map_err(|_| format!("\"{to}\" isn't a valid email address."))?);
    }
    let msg = builder
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string())
        .map_err(|e| format!("Couldn't build the email: {e}"))?;
    let host = cfg.smtp_host.trim();
    let transport = if cfg.tls && cfg.smtp_port == 465 {
        AsyncSmtpTransport::<Tokio1Executor>::relay(host)
    } else if cfg.tls {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)
    } else {
        Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host))
    }
    .map_err(|e| format!("Couldn't use mail server {host}: {e}"))?;
    let mut transport = transport.port(cfg.smtp_port).timeout(Some(Duration::from_secs(20)));
    if !cfg.username.is_empty() {
        transport = transport.credentials(Credentials::new(cfg.username.clone(), cfg.password.clone()));
    }
    transport
        .build()
        .send(msg)
        .await
        .map(|_| ())
        .map_err(|e| format!("The mail server {host} didn't accept the email: {e}"))
}

pub fn http_client() -> &'static reqwest::Client {
    static C: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .user_agent(concat!("PixelPlus/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("HTTP client")
    })
}

pub async fn send_ntfy(cfg: &NtfySettings, title: &str, body: &str, severity: Severity) -> Result<(), String> {
    let server = if cfg.server.trim().is_empty() { "https://ntfy.sh" } else { cfg.server.trim() };
    let server = if server.starts_with("http://") || server.starts_with("https://") {
        server.to_string()
    } else {
        format!("https://{server}")
    };
    let url = format!("{}/{}", server.trim_end_matches('/'), cfg.topic.trim());
    let (priority, tags) = match severity {
        Severity::Critical => ("urgent", "rotating_light"),
        Severity::Warning => ("high", "warning"),
        Severity::Info => ("default", "christmas_tree"),
    };
    let resp = http_client()
        .post(&url)
        .header("Title", title.replace(['\r', '\n'], " "))
        .header("Priority", priority)
        .header("Tags", tags)
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| format!("Couldn't reach {server}: {e}"))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("{server} answered {}", resp.status()))
    }
}

/// Apply temperature / voltage rules to a fresh set of readings.
pub async fn check_sensors(state: &AppState, readings: &[super::sensors::Reading]) {
    let rules = state.store.get().settings.alerts.rules.clone();
    for r in readings {
        let s = &r.sensor;
        let (bad, title, body) = match s.kind {
            SensorKind::Temperature => (
                s.value as f32 > rules.temp_c,
                "Controller is too hot",
                format!("{} is {:.0} °C (limit {:.0} °C). Check ventilation and sun exposure.", s.label, s.value, rules.temp_c),
            ),
            SensorKind::Voltage => (
                (s.value as f32) < rules.voltage_min,
                "Power supply voltage is low",
                format!(
                    "{} is {:.2} V (minimum {:.1} V). Check the power supply and its connections.",
                    s.label, s.value, rules.voltage_min
                ),
            ),
            _ => continue,
        };
        let key = format!("sensor:{}:{}", r.node_id, s.id);
        if state.services.alerts.rising(&key, bad) {
            raise(state, &key, Severity::Warning, title, &body).await;
        }
    }
}

/// Watch player errors and follower status for alert rules.
pub fn start(state: &AppState) {
    // Follower offline: watch `nodes` events.
    let st = state.clone();
    tokio::spawn(async move {
        let mut rx = st.events.subscribe();
        loop {
            match rx.recv().await {
                Ok(Event::Json { kind: "nodes", data }) => {
                    if !st.store.get().settings.alerts.rules.follower_offline {
                        continue;
                    }
                    for n in data.as_array().into_iter().flatten() {
                        let id = n["id"].as_str().unwrap_or_default();
                        let online = n["online"].as_bool().unwrap_or(true);
                        let key = format!("offline:{id}");
                        if st.services.alerts.rising(&key, !online) {
                            let name = n["name"].as_str().unwrap_or(id).to_string();
                            raise(
                                &st,
                                &key,
                                Severity::Critical,
                                "Controller offline",
                                &format!("{name} stopped responding. Check its power and network connection."),
                            )
                            .await;
                        }
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => break,
            }
        }
    });
    // Show failure: watch the player's error.
    let st = state.clone();
    tokio::spawn(async move {
        let player = loop {
            if let Some(p) = st.services.player.get() {
                break p.clone();
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        };
        let mut rx = player.watch();
        loop {
            let err = rx.borrow_and_update().error.clone();
            let key = "player:error";
            if st.services.alerts.rising(key, err.is_some()) && st.store.get().settings.alerts.rules.show_failure {
                raise(&st, key, Severity::Critical, "The show stopped", err.as_deref().unwrap_or("Playback failed.")).await;
            }
            if rx.changed().await.is_err() {
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_and_rate_limit() {
        let a = AlertState::default();
        let t = Instant::now();
        assert!(a.admit("x", t));
        assert!(!a.admit("x", t + Duration::from_secs(60)));
        assert!(a.admit("x", t + DEDUP_WINDOW + Duration::from_secs(1)));
        let b = AlertState::default();
        for i in 0..MAX_PER_HOUR {
            assert!(b.admit(&format!("k{i}"), t));
        }
        assert!(!b.admit("another", t));
        assert!(b.admit("another", t + Duration::from_secs(3601)));
    }

    #[test]
    fn rising_edges() {
        let a = AlertState::default();
        assert!(!a.rising("t", false));
        assert!(a.rising("t", true));
        assert!(!a.rising("t", true));
        assert!(!a.rising("t", false));
        assert!(a.rising("t", true));
    }
}
