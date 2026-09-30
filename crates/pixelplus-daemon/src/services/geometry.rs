//! DPI output geometry vs. the configured strings.
//!
//! The DPI pixel engine's display mode (how many pixels each output can drive)
//! is fixed at boot by `/boot/firmware/pixelplus.conf` (`dtoverlay=...,vactive=N`,
//! written by `pixelplus config-txt`). When a string is made longer than that,
//! the player reports it ([`crate::player::geometry_status`]) and this module
//! offers the fix: regenerate the boot fragment through the root helper
//! (`pixelplus-helper@config-txt:<board>:<pixels>`) and reboot.

use super::platform::{self, HelperOpts, HelperState, HelperStatus, HelperVerb, PowerAction};
use crate::api::{ApiError, ApiResult};
use crate::events::ToastKind;
use crate::state::AppState;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The boot fragment's default string length (`pixelplus config-txt` without --pixels).
pub const DEFAULT_PIXELS: u32 = 800;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OutputGeometry {
    /// False when a string is longer than the running DPI mode allows.
    pub ok: bool,
    /// Longest configured string on this node (pixels).
    pub longest_string: u32,
    /// Longest string the running DPI mode can drive (None: not DPI / unknown).
    pub max_pixels: Option<u32>,
    /// String length the boot fragment on disk is set up for (None: unknown).
    pub configured_pixels: Option<u32>,
    /// The boot fragment already fits; only a reboot is missing.
    pub pending_reboot: bool,
    /// "Apply & reboot" is possible here.
    pub can_apply: bool,
    /// Length "Apply & reboot" would configure.
    pub target_pixels: Option<u32>,
    /// Hard limit of this Pi (display lines), if known.
    pub pi_max_pixels: Option<u32>,
    pub message: Option<String>,
}

/// The boot partition (`PIXELPLUS_BOOT_DIR`, else /boot/firmware, else /boot).
pub fn boot_dir() -> Option<PathBuf> {
    let env = std::env::var_os("PIXELPLUS_BOOT_DIR").map(PathBuf::from);
    env.into_iter()
        .chain([PathBuf::from("/boot/firmware"), PathBuf::from("/boot")])
        .find(|d| d.join("config.txt").is_file() || d.join("pixelplus.conf").is_file())
}

/// Pixels per output that a `pixelplus.conf` fragment configures, from the
/// comment `pixelplus config-txt` writes ("... up to N pixels per output ...").
pub fn parse_configured_pixels(text: &str) -> Option<u32> {
    text.lines().find_map(|l| {
        let l = l.trim_start_matches('#').trim();
        let rest = &l[l.find("up to ")? + "up to ".len()..];
        let (n, tail) = rest.split_once(' ')?;
        tail.starts_with("pixels per output")
            .then(|| n.parse().ok())?
    })
}

fn configured_pixels(dir: Option<&Path>) -> Option<u32> {
    let text = std::fs::read_to_string(dir?.join("pixelplus.conf")).ok()?;
    parse_configured_pixels(&text)
}

/// The length to configure for `longest`: rounded up to 100 (so adding a few
/// pixels later doesn't need another reboot), at least the default.
pub fn target_for(longest: u32) -> u32 {
    longest
        .div_ceil(100)
        .saturating_mul(100)
        .max(DEFAULT_PIXELS)
}

fn pi_max_pixels() -> Option<u32> {
    let (_, pi) = super::system::detection();
    pixelplus_output::DpiSoc::from_model(&pi?.model).map(|s| s.max_pixels_per_output())
}

/// Build the geometry view from its inputs (pure; tested).
pub fn evaluate(
    player: &crate::player::GeometryStatus,
    configured: Option<u32>,
    pi_max: Option<u32>,
    can_run_helper: bool,
    board_has_outputs: bool,
) -> OutputGeometry {
    let longest = player.longest_string;
    let pending_reboot = !player.ok && configured.is_some_and(|c| c >= longest);
    let target = (!player.ok).then(|| target_for(longest));
    let target = match (target, pi_max) {
        (Some(t), Some(max)) if t > max && longest <= max => Some(max),
        (t, _) => t,
    };
    let too_long = pi_max.is_some_and(|m| longest > m);
    let message = if player.ok {
        None
    } else if too_long {
        Some(format!(
            "The longest string has {longest} pixels, more than this Raspberry Pi can drive on one output ({}). Split it across two outputs.",
            pi_max.unwrap_or(0)
        ))
    } else if pending_reboot {
        Some(format!(
            "The pixel output is set up for {} pixels per output after the next restart. Restart the controller to light the whole {longest}-pixel string.",
            configured.unwrap_or(0)
        ))
    } else {
        Some(format!(
            "The longest string has {longest} pixels but the pixel output was set up at boot for {} per output. Apply the new length and restart to light all of it.",
            player.max_pixels.unwrap_or(0)
        ))
    };
    OutputGeometry {
        ok: player.ok,
        longest_string: longest,
        max_pixels: player.max_pixels,
        configured_pixels: configured,
        pending_reboot,
        can_apply: !player.ok
            && !too_long
            && !pending_reboot
            && can_run_helper
            && board_has_outputs,
        target_pixels: target,
        pi_max_pixels: pi_max,
        message,
    }
}

fn can_run_helper() -> bool {
    platform::helper_installed()
        || (super::system::is_root()
            && !super::system::in_docker()
            && Path::new(platform::FIRSTBOOT).is_file())
}

/// Current geometry status of this node.
pub fn status(state: &AppState) -> OutputGeometry {
    let g = crate::player::geometry_status();
    let (board, _) = super::system::effective_board(state);
    let configured = if g.ok {
        None
    } else {
        configured_pixels(boot_dir().as_deref())
    };
    let pi_max = if g.ok { None } else { pi_max_pixels() };
    evaluate(
        &g,
        configured,
        pi_max,
        can_run_helper(),
        board.output_count() > 0,
    )
}

/// "Apply & reboot": write the boot fragment for the longest string via the
/// helper, then (when `reboot`) restart the controller once it succeeded.
pub async fn apply(state: &AppState, reboot: bool) -> ApiResult<HelperStatus> {
    let g = status(state);
    if g.ok {
        return Err(ApiError::conflict(
            "The pixel output already handles your longest string; nothing to change.",
        ));
    }
    if g.pending_reboot {
        if !reboot {
            return Err(ApiError::conflict(
                "The new string length is already saved; restart the controller to use it.",
            ));
        }
        let msg = platform::power_action(state, PowerAction::Reboot)?;
        lights_off(state).await;
        state.events.toast(ToastKind::Info, msg);
        return Ok(HelperStatus {
            verb: "config-txt".into(),
            state: HelperState::Ok,
            message: msg.into(),
            updated_at: chrono::Utc::now().timestamp(),
        });
    }
    if let Some(m) = g
        .message
        .as_ref()
        .filter(|_| g.pi_max_pixels.is_some_and(|max| g.longest_string > max))
    {
        return Err(ApiError::bad_request(m.clone()));
    }
    let (board, _) = super::system::effective_board(state);
    if board.output_count() == 0 {
        return Err(ApiError::bad_request(
            "This controller has no pixel outputs, so there is no boot setting to change.",
        ));
    }
    if reboot {
        // Fail early (before touching the boot partition) if we can't reboot.
        platform::check_power(PowerAction::Reboot)?;
    }
    let board_id = serde_json::to_value(board)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .ok_or_else(|| ApiError::internal("board id"))?;
    let pixels = g
        .target_pixels
        .unwrap_or_else(|| target_for(g.longest_string));
    let job = platform::run_helper(
        state,
        HelperVerb::ConfigTxt {
            board: board_id,
            pixels: Some(pixels),
        },
        HelperOpts { quiet: reboot },
    )
    .await?;
    let first = job.status.clone();
    if reboot {
        let st = state.clone();
        tokio::spawn(async move {
            let done = job.wait(Duration::from_secs(180)).await;
            if done.state != HelperState::Ok {
                st.events.toast(
                    ToastKind::Error,
                    format!("The new string length couldn't be saved: {}", done.message),
                );
                return;
            }
            match platform::power_action(&st, PowerAction::Reboot) {
                Ok(msg) => {
                    lights_off(&st).await;
                    st.events.toast(
                        ToastKind::Info,
                        format!("Pixel output set up for {pixels} pixels per output. {msg}"),
                    );
                }
                Err(e) => st.events.toast(
                    ToastKind::Warning,
                    format!("Saved for {pixels} pixels per output. {} ", e.message),
                ),
            }
        });
    }
    Ok(first)
}

async fn lights_off(state: &AppState) {
    if let Some(p) = state.services.player.get() {
        let _ = p.send(crate::player::PlayerCmd::Stop { fade: false }).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::GeometryStatus;

    fn over(longest: u32, max: u32) -> GeometryStatus {
        GeometryStatus {
            ok: longest <= max,
            longest_string: longest,
            max_pixels: Some(max),
            needed_pixels: Some(longest),
            message: None,
        }
    }

    #[test]
    fn parses_fragment_comment() {
        let frag = "# --- PixelPlus: x ---\n[all]\n# WS281x pixel engine: 4 outputs, up to 1600 pixels per output at 20.3 fps.\ndtoverlay=pixelplus-dpi,vactive=1607\n";
        assert_eq!(parse_configured_pixels(frag), Some(1600));
        assert_eq!(parse_configured_pixels("dtoverlay=pixelplus-dpi\n"), None);
        assert_eq!(
            parse_configured_pixels("# up to lots of pixels per output\n"),
            None
        );
    }

    #[test]
    fn targets_round_up() {
        assert_eq!(target_for(10), 800);
        assert_eq!(target_for(801), 900);
        assert_eq!(target_for(1600), 1600);
        assert_eq!(target_for(1601), 1700);
    }

    #[test]
    fn evaluation() {
        let ok = evaluate(&over(500, 800), None, None, true, true);
        assert!(ok.ok && !ok.can_apply && ok.message.is_none());

        let g = evaluate(&over(1234, 800), Some(800), Some(2041), true, true);
        assert!(!g.ok && g.can_apply && !g.pending_reboot);
        assert_eq!(g.target_pixels, Some(1300));
        assert!(g.message.unwrap().contains("1234"));

        // Fragment already rewritten: reboot only.
        let g = evaluate(&over(1234, 800), Some(1300), Some(2041), true, true);
        assert!(g.pending_reboot && !g.can_apply);

        // Longer than the Pi can do at all.
        let g = evaluate(&over(5000, 800), Some(800), Some(2041), true, true);
        assert!(!g.can_apply);
        assert!(g.message.unwrap().contains("Split"));

        // Near the limit: target capped at the Pi's maximum.
        let g = evaluate(&over(2030, 800), None, Some(2041), true, true);
        assert_eq!(g.target_pixels, Some(2041));

        // No helper (Docker / PC): explain, no button.
        assert!(!evaluate(&over(1234, 800), None, None, false, true).can_apply);
    }
}
