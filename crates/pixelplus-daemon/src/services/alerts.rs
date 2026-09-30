//! Alerts by email (SMTP, STARTTLS/TLS) and ntfy, with de-duplication and
//! rate limiting. Rules come from `settings.alerts.rules`: board temperature,
//! 12 V supply voltage, follower offline, show failure (player error) and
//! failed pre-show checks.
//!
//! Also the delivery for the nightly report (F11: [`send_email_html`],
//! [`send_ntfy_with`] with a click link) and the journal of controller
//! online/offline transitions and show failures the report counts.

use crate::cluster::ClusterEvent;
use crate::events::ToastKind;
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
        if last
            .get(key)
            .is_some_and(|t| now.duration_since(*t) < DEDUP_WINDOW)
        {
            return false;
        }
        let mut sent = self.sent.lock();
        while sent
            .front()
            .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(3600))
        {
            sent.pop_front();
        }
        if sent.len() >= MAX_PER_HOUR {
            return false;
        }
        sent.push_back(now);
        last.insert(key.to_string(), now);
        true
    }

    pub fn was_active(&self, key: &str) -> bool {
        self.active.lock().get(key).copied().unwrap_or(false)
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

/// Someone signed in to the admin UI from outside the home network (F14:
/// through a tunnel, Tailscale or a public address). Raises a warning
/// (deduplicated per address for [`DEDUP_WINDOW`]) in the background.
pub fn remote_sign_in(state: &AppState, ip: Option<std::net::IpAddr>, via: &str) {
    let who = ip.map_or_else(|| "an unknown address".to_string(), |ip| ip.to_string());
    tracing::info!("Remote sign-in from {who} through {via}");
    let key = format!("remote-sign-in:{who}");
    let body = format!(
        "Someone signed in to PixelPlus from {who} through {via}. If this wasn't you, change your password in Settings → Security and check Settings → Remote access."
    );
    let state = state.clone();
    tokio::spawn(async move {
        raise(&state, &key, Severity::Warning, "Remote sign-in", &body).await;
    });
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
    if let Some(email) = settings
        .email
        .clone()
        .filter(|e| !e.smtp_host.is_empty() && !e.to.is_empty())
    {
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
            let ntfy = settings
                .ntfy
                .filter(|n| !n.topic.is_empty())
                .ok_or("Choose an ntfy topic first.")?;
            send_ntfy(&ntfy, &subject, body, Severity::Info).await?;
            Ok(format!("Test notification sent to \"{}\".", ntfy.topic))
        }
        _ => Err("Pick email or ntfy.".into()),
    }
}

pub async fn send_email(cfg: &EmailSettings, subject: &str, body: &str) -> Result<(), String> {
    use lettre::message::header::ContentType;
    let msg = email_builder(cfg, subject)?
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string())
        .map_err(|e| format!("Couldn't build the email: {e}"))?;
    deliver(cfg, msg).await
}

/// Send an email with an HTML body and its plain-text alternative (the
/// nightly report, F11).
pub async fn send_email_html(
    cfg: &EmailSettings,
    subject: &str,
    text: &str,
    html: &str,
) -> Result<(), String> {
    use lettre::message::MultiPart;
    let msg = email_builder(cfg, subject)?
        .multipart(MultiPart::alternative_plain_html(
            text.to_string(),
            html.to_string(),
        ))
        .map_err(|e| format!("Couldn't build the email: {e}"))?;
    deliver(cfg, msg).await
}

fn email_builder(
    cfg: &EmailSettings,
    subject: &str,
) -> Result<lettre::message::MessageBuilder, String> {
    use lettre::Message;
    let from_addr = if cfg.from.trim().is_empty() {
        cfg.username.trim()
    } else {
        cfg.from.trim()
    };
    let from = from_addr
        .parse()
        .map_err(|_| format!("\"{from_addr}\" isn't a valid sender address."))?;
    let mut builder = Message::builder().from(from).subject(subject);
    for to in cfg
        .to
        .split([',', ';'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        builder = builder.to(to
            .parse()
            .map_err(|_| format!("\"{to}\" isn't a valid email address."))?);
    }
    Ok(builder)
}

async fn deliver(cfg: &EmailSettings, msg: lettre::Message) -> Result<(), String> {
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
    let host = cfg.smtp_host.trim();
    let transport = if cfg.tls && cfg.smtp_port == 465 {
        AsyncSmtpTransport::<Tokio1Executor>::relay(host)
    } else if cfg.tls {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)
    } else {
        Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
            host,
        ))
    }
    .map_err(|e| format!("Couldn't use mail server {host}: {e}"))?;
    let mut transport = transport
        .port(cfg.smtp_port)
        .timeout(Some(Duration::from_secs(20)));
    if !cfg.username.is_empty() {
        transport =
            transport.credentials(Credentials::new(cfg.username.clone(), cfg.password.clone()));
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

pub async fn send_ntfy(
    cfg: &NtfySettings,
    title: &str,
    body: &str,
    severity: Severity,
) -> Result<(), String> {
    send_ntfy_with(cfg, title, body, severity, None).await
}

/// [`send_ntfy`] with a link opened when the notification is tapped.
pub async fn send_ntfy_with(
    cfg: &NtfySettings,
    title: &str,
    body: &str,
    severity: Severity,
    click: Option<&str>,
) -> Result<(), String> {
    let server = if cfg.server.trim().is_empty() {
        "https://ntfy.sh"
    } else {
        cfg.server.trim()
    };
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
    // Header values must be one line of visible ASCII (RFC 7230); ntfy
    // decodes RFC 2047 encoded words for anything else (emoji, accents).
    let mut req = http_client()
        .post(&url)
        .header("Title", header_text(title))
        .header("Priority", priority)
        .header("Tags", tags);
    if let Some(c) = click.filter(|c| c.starts_with("http://") || c.starts_with("https://")) {
        req = req.header("Click", c.replace(['\r', '\n'], ""));
    }
    let resp = req
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

/// A header-safe title: plain ASCII as is, otherwise an RFC 2047
/// `=?UTF-8?B?…?=` encoded word (ntfy understands both).
fn header_text(s: &str) -> String {
    let one_line = s.replace(['\r', '\n'], " ");
    if one_line.bytes().all(|b| (0x20..0x7f).contains(&b)) {
        return one_line;
    }
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let data = one_line.as_bytes();
    let mut b64 = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        b64.push(T[(n >> 18) as usize & 63] as char);
        b64.push(T[(n >> 12) as usize & 63] as char);
        b64.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        b64.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    format!("=?UTF-8?B?{b64}?=")
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
                format!(
                    "{} is {:.0} °C (limit {:.0} °C). Check ventilation and sun exposure.",
                    s.label, s.value, rules.temp_c
                ),
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
    // Follower offline / sync problems: cluster events.
    let st = state.clone();
    tokio::spawn(async move {
        let cluster = loop {
            if let Some(c) = st.services.cluster.get() {
                break c.clone();
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        };
        let mut rx = cluster.subscribe();
        loop {
            let ev = match rx.recv().await {
                Ok(ev) => ev,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            };
            // Journal (F11 nightly report): every transition, alert or not.
            match &ev {
                ClusterEvent::NodeOffline { node_id, .. } => {
                    st.services
                        .journal
                        .record(super::journal::Event::NodeOffline {
                            id: node_id.clone(),
                        })
                }
                ClusterEvent::NodeOnline { node_id, .. } => {
                    st.services
                        .journal
                        .record(super::journal::Event::NodeOnline {
                            id: node_id.clone(),
                        })
                }
                ClusterEvent::SyncProblem { message, .. } => {
                    st.services.journal.record(super::journal::Event::Warn {
                        code: "files".into(),
                        msg: message.clone(),
                    })
                }
            }
            if !st.store.get().settings.alerts.rules.follower_offline {
                continue;
            }
            match ev {
                ClusterEvent::NodeOffline { node_id, name, .. } => {
                    let key = format!("offline:{node_id}");
                    if st.services.alerts.rising(&key, true) {
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
                ClusterEvent::NodeOnline { node_id, name } => {
                    let key = format!("offline:{node_id}");
                    if st.services.alerts.was_active(&key) {
                        st.services.alerts.rising(&key, false);
                        st.events
                            .toast(ToastKind::Success, format!("{name} is back online."));
                    }
                }
                ClusterEvent::SyncProblem {
                    node_id,
                    name,
                    message,
                } => {
                    let key = format!("sync:{node_id}");
                    raise(
                        &st,
                        &key,
                        Severity::Warning,
                        &format!("{name} can't get its show files"),
                        &message,
                    )
                    .await;
                }
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
            let (err, state) = {
                let s = rx.borrow_and_update();
                (s.error.clone(), s.state)
            };
            let key = "player:error";
            let rising = st.services.alerts.rising(key, err.is_some());
            if rising {
                // Journal (F11): the nightly report counts show problems.
                let (level, _) = player_alert(state);
                let msg = err.clone().unwrap_or_default();
                st.services.journal.record(match level {
                    Severity::Critical => super::journal::Event::Error {
                        code: "show".into(),
                        msg,
                    },
                    _ => super::journal::Event::Warn {
                        code: "show".into(),
                        msg,
                    },
                });
            }
            if rising && st.store.get().settings.alerts.rules.show_failure {
                let (severity, title) = player_alert(state);
                raise(
                    &st,
                    key,
                    severity,
                    title,
                    err.as_deref().unwrap_or("Playback failed."),
                )
                .await;
            }
            if rx.changed().await.is_err() {
                break;
            }
        }
    });
}

/// Severity and title of a player problem: most (no sound, a skipped item, a
/// follower missing a file) leave the lights running, so only an idle player
/// "stopped".
fn player_alert(state: crate::player::PlayerState) -> (Severity, &'static str) {
    use crate::player::PlayerState;
    match state {
        PlayerState::Idle => (Severity::Critical, "The show stopped"),
        _ => (Severity::Warning, "Show problem"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_problems_while_playing_are_not_a_stopped_show() {
        use crate::player::PlayerState;
        assert_eq!(player_alert(PlayerState::Playing).1, "Show problem");
        assert!(matches!(
            player_alert(PlayerState::Playing).0,
            Severity::Warning
        ));
        assert_eq!(player_alert(PlayerState::Idle).1, "The show stopped");
    }

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
    fn push_titles_are_header_safe() {
        assert_eq!(header_text("Show problem"), "Show problem");
        assert_eq!(header_text("a\r\nb"), "a  b");
        let enc = header_text("✅ All good");
        assert!(enc.starts_with("=?UTF-8?B?") && enc.ends_with("?="));
        assert!(enc.is_ascii());
        // "✅ All good" in base64.
        assert_eq!(enc, "=?UTF-8?B?4pyFIEFsbCBnb29k?=");
    }

    #[test]
    fn html_email_builds_with_both_parts() {
        let cfg = EmailSettings {
            smtp_host: "smtp.example.com".into(),
            smtp_port: 587,
            username: "me@example.com".into(),
            password: "x".into(),
            from: String::new(),
            to: "a@example.com; b@example.com".into(),
            tls: true,
        };
        let msg = email_builder(&cfg, "Report")
            .unwrap()
            .multipart(lettre::message::MultiPart::alternative_plain_html(
                "text".to_string(),
                "<b>html</b>".to_string(),
            ))
            .unwrap();
        let raw = String::from_utf8(msg.formatted()).unwrap();
        assert!(raw.contains("multipart/alternative"));
        assert!(raw.contains("text/html"));
        assert!(raw.contains("a@example.com") && raw.contains("b@example.com"));
        let bad = EmailSettings {
            to: "not an address".into(),
            ..cfg
        };
        assert!(email_builder(&bad, "x").is_err());
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
