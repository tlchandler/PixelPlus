//! First-run setup: shared by the wizard (`POST /system/setup`) and by
//! provisioning (`provision.json`, written from pixelplus.txt / the imager).

use crate::api::{ApiError, ApiResult};
use crate::node::LocalRole;
use crate::services::system as sys;
use crate::state::AppState;
use pixelplus_core::model::{BoardKind, Location};

/// What to set up. Everything is optional; `None` leaves it as it is.
#[derive(Debug, Clone, Default)]
pub struct SetupRequest {
    pub role: Option<LocalRole>,
    pub show_name: Option<String>,
    /// This controller's friendly name.
    pub name: Option<String>,
    pub board: Option<BoardKind>,
    pub board_rev: Option<String>,
    pub location: Option<Location>,
    pub timezone: Option<String>,
    /// Web UI password (plain text; hashed here, never stored).
    pub password: Option<String>,
    /// Also write the board EEPROM when it is blank.
    pub write_eeprom: bool,
}

/// Result of [`apply`].
#[derive(Debug, Clone, Default)]
pub struct SetupOutcome {
    /// Non-fatal problems to show the user.
    pub notes: Vec<String>,
    /// A password was set.
    pub password_set: bool,
}

pub fn parse_role(role: &str) -> Option<LocalRole> {
    match role.trim().to_ascii_lowercase().as_str() {
        "leader" => Some(LocalRole::Leader),
        "follower" => Some(LocalRole::Follower),
        _ => None,
    }
}

pub fn valid_rev(rev: &str) -> bool {
    !rev.is_empty() && rev.len() <= 8 && rev.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
}

/// Check everything before changing anything.
pub fn validate(state: &AppState, req: &SetupRequest) -> ApiResult<Option<String>> {
    let current = state.identity();
    if current.role == LocalRole::Follower
        && current.leader_id.is_some()
        && req.role == Some(LocalRole::Leader)
    {
        return Err(ApiError::conflict(
            "This controller belongs to another show's leader. Release it from that leader first.",
        ));
    }
    if let Some(name) = req.show_name.as_deref() {
        if name.trim().is_empty() || name.chars().count() > 120 {
            return Err(ApiError::bad_request("Give your show a name (up to 120 characters)."));
        }
    }
    if let Some(name) = req.name.as_deref() {
        if name.chars().count() > 64 {
            return Err(ApiError::bad_request("A controller name can be at most 64 characters."));
        }
    }
    let tz = req
        .timezone
        .clone()
        .or_else(|| req.location.as_ref().map(|l| l.timezone.clone()))
        .filter(|t| !t.trim().is_empty());
    if let Some(tz) = &tz {
        if tz.parse::<chrono_tz::Tz>().is_err() {
            return Err(ApiError::bad_request(format!(
                "\"{tz}\" isn't a time zone PixelPlus knows. Pick your city again."
            )));
        }
    }
    if let Some(l) = &req.location {
        if !(-90.0..=90.0).contains(&l.lat) || !(-180.0..=180.0).contains(&l.lon) {
            return Err(ApiError::bad_request(
                "That location doesn't look right. Pick your city again.",
            ));
        }
    }
    if let Some(rev) = &req.board_rev {
        if !valid_rev(rev) {
            return Err(ApiError::bad_request(
                "The board revision is a letter printed on the board, like E.",
            ));
        }
    }
    if let Some(p) = req.password.as_deref().filter(|p| !p.is_empty()) {
        if p.chars().count() < 4 {
            return Err(ApiError::bad_request("Use at least 4 characters for the password."));
        }
    }
    Ok(tz)
}

/// Apply a setup request: identity (role, board, name), show defaults for a
/// leader, password, time zone. Validates first; changes nothing on error.
pub async fn apply(state: &AppState, req: SetupRequest) -> ApiResult<SetupOutcome> {
    let tz = validate(state, &req)?;
    let password_hash = match req.password.as_deref().filter(|p| !p.is_empty()) {
        Some(p) => Some(crate::api::auth::hash_password(p)?),
        None => None,
    };
    let mut out = SetupOutcome {
        password_set: password_hash.is_some(),
        ..Default::default()
    };

    // Board: keep the wizard's choice when it differs from (or replaces) detection.
    let (det, _) = tokio::task::spawn_blocking(sys::detection)
        .await
        .map_err(ApiError::internal)?;
    if let Some(board) = req.board {
        if req.write_eeprom
            && det.board.is_none()
            && matches!(board, BoardKind::Difftx | BoardKind::Difftxlarge | BoardKind::Diffsmart)
        {
            let rev = req.board_rev.clone().unwrap_or_else(|| "A".into());
            if let Err(e) = write_eeprom(board, rev).await {
                out.notes.push(format!(
                    "The board EEPROM couldn't be written ({e}); your choice is saved anyway."
                ));
            }
        }
    }
    let name = req
        .name
        .clone()
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty());
    let role = state
        .set_identity(|id| {
            if let Some(role) = req.role {
                id.role = role;
            }
            if let Some(board) = req.board {
                let rev_matches = match req.board_rev.as_ref() {
                    None => true,
                    Some(a) => det.rev.as_ref().is_some_and(|b| a.eq_ignore_ascii_case(b)),
                };
                let detected_same = det.board == Some(board) && rev_matches;
                id.board = if detected_same { None } else { Some(board) };
                id.board_rev = if detected_same { None } else { req.board_rev.clone() };
            }
            if name.is_some() {
                id.name = name.clone();
            }
        })
        .map_err(ApiError::internal)?
        .role;

    if role == LocalRole::Leader && req.role == Some(LocalRole::Leader) {
        let show_name = req.show_name.clone().map(|n| n.trim().to_string());
        let location = req.location.clone();
        let tz2 = tz.clone();
        let hash = password_hash.clone();
        state
            .store
            .update(move |s| {
                if let Some(n) = show_name {
                    s.name = n;
                }
                if let Some(mut l) = location {
                    if let Some(tz) = &tz2 {
                        l.timezone = tz.clone();
                    }
                    s.schedule.location = l;
                } else if let Some(tz) = tz2 {
                    s.schedule.location.timezone = tz;
                }
                if hash.is_some() {
                    s.settings.security.password_hash = hash;
                }
                crate::services::seed::seed_defaults(s);
                Ok(())
            })
            .await?;
        if let Err(e) = crate::cluster::ensure_self_node(state).await {
            tracing::warn!("Couldn't add this controller to the show: {e:#}");
        }
    } else if let Some(hash) = password_hash {
        state
            .store
            .update(move |s| {
                s.settings.security.password_hash = Some(hash);
                Ok(())
            })
            .await?;
    }
    if let Some(tz) = tz {
        tokio::spawn(async move { sys::set_system_timezone(&tz).await });
    }
    Ok(out)
}

/// Write the PPX1 record to the board EEPROM (sysfs when writable, else i2c-dev).
pub async fn write_eeprom(board: BoardKind, rev: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        #[cfg(target_os = "linux")]
        {
            let mut store = sys::open_board_eeprom(true)?;
            let record = pixelplus_hw::Ppx1Record::new(board, &rev);
            pixelplus_hw::eeprom::write_record(store.as_mut(), &record).map_err(|e| e.to_string())?;
            sys::redetect_board();
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (board, rev);
            Err("only possible on a Raspberry Pi".into())
        }
    })
    .await
    .map_err(|e| e.to_string())?
}
