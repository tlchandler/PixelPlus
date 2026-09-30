//! Provisioning: settings handed to the daemon by the (root) pixelplus.txt
//! applier, image/firstboot/firstboot.py, in `<data dir>/provision.json`:
//!
//! ```json
//! {"role": "leader", "uiPassword": "…", "board": "difftx",
//!  "source": "pixelplus.txt", "createdAt": "2026-09-30T18:00:00-05:00"}
//! ```
//!
//! Every key is optional (`name` and `showName`, `timezone` are accepted too).
//! The file is applied like `POST /system/setup` (password hashed, role and
//! board set, defaults seeded for a leader) and then deleted, so the plain-text
//! password never stays on disk. It is checked at startup and every few
//! seconds, because pixelplus.txt can be edited while we run.

use super::setup::{self, SetupRequest};
use crate::events::ToastKind;
use crate::state::AppState;
use pixelplus_core::model::BoardKind;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

const FILE: &str = "provision.json";
const EVERY: Duration = Duration::from_secs(5);

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionFile {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub ui_password: Option<String>,
    #[serde(default)]
    pub board: Option<String>,
    /// Friendly controller name.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub show_name: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub created_at: Option<serde_json::Value>,
}

pub fn path(state: &AppState) -> PathBuf {
    state.config.data_dir.join(FILE)
}

/// Turn the file into a setup request. Unknown values are skipped with a note.
pub fn to_request(p: &ProvisionFile) -> (SetupRequest, Vec<String>) {
    let mut notes = Vec::new();
    let mut req = SetupRequest::default();
    if let Some(r) = p.role.as_deref().filter(|r| !r.trim().is_empty()) {
        match setup::parse_role(r) {
            Some(role) => req.role = Some(role),
            None => notes.push(format!("role \"{r}\" isn't leader or follower")),
        }
    }
    if let Some(b) = p.board.as_deref().map(str::trim).filter(|b| !b.is_empty() && !b.eq_ignore_ascii_case("auto")) {
        match serde_json::from_value::<BoardKind>(serde_json::Value::String(b.to_ascii_lowercase())) {
            Ok(k) => req.board = Some(k),
            Err(_) => notes.push(format!("board \"{b}\" isn't a board PixelPlus knows")),
        }
    }
    req.password = p.ui_password.clone().filter(|p| !p.is_empty());
    req.name = p.name.clone().filter(|n| !n.trim().is_empty());
    req.show_name = p.show_name.clone().filter(|n| !n.trim().is_empty());
    req.timezone = p.timezone.clone().filter(|t| !t.trim().is_empty());
    (req, notes)
}

fn describe(req: &SetupRequest) -> Vec<String> {
    let mut what = Vec::new();
    if let Some(r) = req.role {
        what.push(format!("role {}", serde_json::to_value(r).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default()));
    }
    if let Some(b) = req.board {
        what.push(format!("board {}", b.display_name()));
    }
    if req.password.is_some() {
        what.push("web password".into());
    }
    if let Some(n) = &req.name {
        what.push(format!("name {n}"));
    }
    if req.show_name.is_some() {
        what.push("show name".into());
    }
    if let Some(t) = &req.timezone {
        what.push(format!("time zone {t}"));
    }
    what
}

fn remove(p: &Path) {
    if let Err(e) = std::fs::remove_file(p) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::error!(
                "Couldn't delete {} ({e}); it holds a password in plain text. Delete it by hand.",
                p.display()
            );
        }
    }
}

/// Outcome of one check (for tests and logging).
#[derive(Debug, PartialEq)]
pub enum Checked {
    NoFile,
    Applied(Vec<String>),
    Rejected(String),
    /// Couldn't be applied right now (disk error); kept for the next check.
    Retry(String),
}

/// Apply `provision.json` if present. Deletes it unless a retry makes sense.
pub async fn check_once(state: &AppState) -> Checked {
    let p = path(state);
    let text = match std::fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Checked::NoFile,
        Err(e) => {
            return Checked::Retry(format!("can't read {}: {e}", p.display()));
        }
    };
    let file: ProvisionFile = match serde_json::from_str(&text) {
        Ok(f) => f,
        Err(e) => {
            remove(&p);
            let msg = format!("The settings from pixelplus.txt couldn't be read ({e}); edit pixelplus.txt and try again.");
            tracing::error!("{}: {msg}", p.display());
            state.events.toast(ToastKind::Error, msg.clone());
            return Checked::Rejected(msg);
        }
    };
    let source = file.source.clone().unwrap_or_else(|| "provisioning".into());
    let (req, mut notes) = to_request(&file);
    let what = describe(&req);
    let mut req = req;
    // A follower that belongs to a leader keeps its role; apply the rest.
    let id = state.identity();
    if id.role == crate::node::LocalRole::Follower
        && id.leader_id.is_some()
        && req.role == Some(crate::node::LocalRole::Leader)
    {
        notes.push("role leader ignored: this controller belongs to another show's leader".into());
        req.role = None;
    }
    match setup::apply(state, req).await {
        Ok(out) => {
            remove(&p);
            notes.extend(out.notes);
            if what.is_empty() {
                tracing::info!("{} from {source}: nothing to apply", p.display());
            } else {
                tracing::info!("Applied settings from {source}: {}", what.join(", "));
                state.events.toast(
                    ToastKind::Success,
                    format!("Applied settings from {source}: {}.", what.join(", ")),
                );
            }
            for n in &notes {
                tracing::warn!("{source}: {n}");
                state.events.toast(ToastKind::Warning, format!("{source}: {n}"));
            }
            state.events.publish("system", &serde_json::json!({ "changed": true }));
            Checked::Applied(what)
        }
        Err(e) if e.status.is_server_error() => Checked::Retry(e.message),
        Err(e) => {
            remove(&p);
            let msg = format!("The settings from {source} couldn't be applied: {}", e.message);
            tracing::error!("{msg}");
            state.events.toast(ToastKind::Error, msg.clone());
            Checked::Rejected(msg)
        }
    }
}

/// Check now (before the web server starts), then every few seconds.
pub async fn start(state: &AppState) {
    let first = check_once(state).await;
    if let Checked::Retry(e) = &first {
        tracing::warn!("provisioning: {e}; will retry");
    }
    let state = state.clone();
    tokio::spawn(async move {
        let mut warned = matches!(first, Checked::Retry(_));
        loop {
            tokio::time::sleep(EVERY).await;
            if !path(&state).exists() {
                continue;
            }
            match check_once(&state).await {
                Checked::Retry(e) if !warned => {
                    tracing::warn!("provisioning: {e}; will retry");
                    warned = true;
                }
                Checked::Retry(_) => {}
                _ => warned = false,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::testkit::TestApp;
    use crate::node::LocalRole;

    #[test]
    fn request_from_file() {
        let f: ProvisionFile = serde_json::from_str(
            r#"{"role":"Leader","uiPassword":"jingle","board":"difftxlarge","source":"pixelplus.txt","createdAt":"2026-09-30T18:00:00-05:00"}"#,
        )
        .unwrap();
        let (r, notes) = to_request(&f);
        assert!(notes.is_empty());
        assert_eq!(r.role, Some(LocalRole::Leader));
        assert_eq!(r.board, Some(BoardKind::Difftxlarge));
        assert_eq!(r.password.as_deref(), Some("jingle"));
        let f: ProvisionFile = serde_json::from_str(r#"{"role":"boss","board":"auto"}"#).unwrap();
        let (r, notes) = to_request(&f);
        assert_eq!(r.role, None);
        assert_eq!(r.board, None);
        assert_eq!(notes.len(), 1);
        let f: ProvisionFile = serde_json::from_str(r#"{"board":"bare-pi"}"#).unwrap();
        assert_eq!(to_request(&f).0.board, Some(BoardKind::BarePi));
    }

    #[tokio::test]
    async fn applies_and_deletes_the_file() {
        let app = TestApp::new();
        assert_eq!(check_once(&app.state).await, Checked::NoFile);
        let p = path(&app.state);
        std::fs::write(
            &p,
            r#"{"role":"leader","uiPassword":"jingle","board":"difftx","source":"pixelplus.txt","createdAt":"2026-09-30T18:00:00-05:00"}"#,
        )
        .unwrap();
        let r = check_once(&app.state).await;
        assert!(matches!(r, Checked::Applied(ref w) if w.len() == 3), "{r:?}");
        assert!(!p.exists(), "provision.json must be deleted");
        let id = app.state.identity();
        assert_eq!(id.role, LocalRole::Leader);
        assert_eq!(id.board, Some(BoardKind::Difftx));
        let show = app.state.store.get();
        let hash = show.settings.security.password_hash.clone().expect("password set");
        assert!(crate::api::auth::verify_password(&hash, "jingle"));
        assert!(!hash.contains("jingle"));
        assert!(show.playlists.iter().any(|p| p.name == "Main Show"), "defaults seeded");
    }

    #[tokio::test]
    async fn password_only_keeps_the_wizard() {
        let app = TestApp::new();
        std::fs::write(path(&app.state), r#"{"uiPassword":"secret1"}"#).unwrap();
        assert!(matches!(check_once(&app.state).await, Checked::Applied(_)));
        assert_eq!(app.state.identity().role, LocalRole::Unconfigured);
        assert!(app.state.store.get().settings.security.password_hash.is_some());
    }

    #[tokio::test]
    async fn bad_files_are_removed() {
        let app = TestApp::new();
        let p = path(&app.state);
        std::fs::write(&p, "{broken").unwrap();
        assert!(matches!(check_once(&app.state).await, Checked::Rejected(_)));
        assert!(!p.exists());
        // Too-short password: rejected as a whole, file removed (it holds a password).
        std::fs::write(&p, r#"{"role":"leader","uiPassword":"ab"}"#).unwrap();
        assert!(matches!(check_once(&app.state).await, Checked::Rejected(_)));
        assert!(!p.exists());
        assert_eq!(app.state.identity().role, LocalRole::Unconfigured);
    }
}
