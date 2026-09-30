//! Software updates from the PixelPlus apt repository (`pixelplus` package).
//! In Docker the image is updated instead.
//!
//! Checking (`apt-cache policy`) works as the unprivileged service user, using
//! the package lists apt refreshes daily. Installing goes through the root
//! helper (`pixelplus-helper@update.service`: apt-get update + upgrade), whose
//! progress is reported as `helper` WebSocket messages and toasts.

use super::platform::{self, HelperOpts, HelperStatus, HelperVerb};
use super::system::{have, in_docker, is_root, run};
use crate::api::{ApiError, ApiResult};
use crate::state::AppState;
use serde::Serialize;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub available: bool,
    /// Whether `POST /system/update` can install it here.
    pub can_apply: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The latest install run (progress of `POST /system/update`), if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job: Option<HelperStatus>,
}

const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// Parse `apt-cache policy pixelplus` → (installed, candidate, channel).
pub fn parse_policy(text: &str) -> (Option<String>, Option<String>, Option<String>) {
    let field = |name: &str| {
        text.lines()
            .find_map(|l| l.trim().strip_prefix(name).map(|v| v.trim().to_string()))
            .filter(|v| !v.is_empty() && v != "(none)")
    };
    let channel = text
        .lines()
        .skip_while(|l| !l.contains("***"))
        .nth(1)
        .and_then(|l| {
            l.split_whitespace()
                .nth(2)
                .map(|s| s.split('/').next().unwrap_or(s).to_string())
        });
    (field("Installed:"), field("Candidate:"), channel)
}

/// Whether this machine can install the update itself.
fn can_install() -> bool {
    platform::helper_installed() || (is_root() && have("systemd-run") && have("apt-get"))
}

pub async fn check(state: &AppState) -> UpdateInfo {
    let mut info = UpdateInfo {
        current: CURRENT.into(),
        latest: CURRENT.into(),
        available: false,
        can_apply: false,
        notes: None,
        channel: None,
        message: None,
        job: state.services.helpers.get("update"),
    };
    if in_docker() {
        info.message = Some(
            "PixelPlus runs in Docker here. To update, pull the new image and recreate the container (docker compose pull && docker compose up -d).".into(),
        );
        return info;
    }
    if !have("apt-cache") {
        info.message = Some("Updates are installed with the PixelPlus Imager or your package manager on this computer.".into());
        return info;
    }
    if is_root() && !platform::helper_installed() {
        // Development machine running as root: refresh the index (ignore failures: offline).
        // As the service user the lists are refreshed daily by apt, and by the helper.
        let _ = run("apt-get", &["update", "-qq"], Duration::from_secs(90)).await;
    }
    match run(
        "apt-cache",
        &["policy", "pixelplus"],
        Duration::from_secs(20),
    )
    .await
    {
        Ok(o) if o.success => {
            let (installed, candidate, channel) = parse_policy(&o.stdout);
            let installed = installed.unwrap_or_else(|| CURRENT.into());
            info.current = installed.clone();
            info.channel = channel;
            if let Some(c) = candidate {
                info.available = c != installed;
                info.latest = c;
            } else {
                info.latest = installed;
                info.message =
                    Some("The PixelPlus package repository isn't set up on this computer.".into());
            }
            info.can_apply = info.available && can_install();
            if info.available && !info.can_apply {
                info.message = Some("An update is available. Install it with: sudo apt install --only-upgrade pixelplus".into());
            }
        }
        _ => {
            info.message =
                Some("Couldn't check for updates right now. Is the internet connected?".into())
        }
    }
    info
}

/// Start the upgrade through the root helper (it restarts pixelplusd when done).
pub async fn apply(state: &AppState) -> ApiResult<String> {
    if in_docker() {
        return Err(ApiError::forbidden(
            "PixelPlus runs in Docker here. Update by pulling the new container image.",
        ));
    }
    if !can_install() {
        return Err(ApiError::forbidden(
            "This PixelPlus can't update itself. Run: sudo apt install --only-upgrade pixelplus",
        ));
    }
    platform::run_helper(state, HelperVerb::Update, HelperOpts::default()).await?;
    Ok(
        "Installing the update. PixelPlus will restart by itself when it's done (a minute or two)."
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_parsing() {
        let t = "pixelplus:\n  Installed: 0.9.0\n  Candidate: 0.9.2\n  Version table:\n     0.9.2 500\n        500 https://apt.pixelplus.dev stable/main arm64 Packages\n *** 0.9.0 100\n        100 /var/lib/dpkg/status\n";
        let (i, c, _) = parse_policy(t);
        assert_eq!(i.as_deref(), Some("0.9.0"));
        assert_eq!(c.as_deref(), Some("0.9.2"));
        let (i, c, _) = parse_policy("pixelplus:\n  Installed: (none)\n  Candidate: 1.0\n");
        assert_eq!(i, None);
        assert_eq!(c.as_deref(), Some("1.0"));
    }
}
