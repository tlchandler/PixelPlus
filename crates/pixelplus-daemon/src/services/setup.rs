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
    /// The wizard's "What will you use?" choice (a new leader only).
    pub features: Option<pixelplus_core::model::FeatureSettings>,
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
            return Err(ApiError::bad_request(
                "Give your show a name (up to 120 characters).",
            ));
        }
    }
    if let Some(name) = req.name.as_deref() {
        if name.chars().count() > 64 {
            return Err(ApiError::bad_request(
                "A controller name can be at most 64 characters.",
            ));
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
        // Same rule as `auth::hash_password`, the wizard and pixelplus.txt.
        if p.chars().count() < crate::api::auth::MIN_PASSWORD {
            return Err(ApiError::bad_request(format!(
                "Use at least {} characters for the password.",
                crate::api::auth::MIN_PASSWORD
            )));
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
            && matches!(
                board,
                BoardKind::Difftx | BoardKind::Difftxlarge | BoardKind::Diffsmart
            )
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
                id.board_rev = if detected_same {
                    None
                } else {
                    req.board_rev.clone()
                };
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
        // Provisioned leaders (pixelplus.txt / the imager) don't go through the
        // wizard's location step: take the controller's own time zone rather than
        // leaving the schedule on the built-in default (America/Chicago).
        let tz2 = tz.clone().or_else(|| {
            let untouched = state.store.get().schedule.location == Location::default();
            sys::system_timezone()
                .filter(|t| untouched && location.is_none() && t.parse::<chrono_tz::Tz>().is_ok())
        });
        let hash = password_hash.clone();
        let features = req.features.clone().map(|mut f| {
            f.normalize();
            f
        });
        state
            .store
            .update(move |s| {
                if let Some(f) = features {
                    s.settings.features = f;
                }
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
    } else if password_hash.is_some() || (role == LocalRole::Leader && tz.is_some()) {
        // An already set-up leader: a new time zone (e.g. edited in pixelplus.txt)
        // is the one its schedule runs in, like the wizard's.
        let tz2 = tz.clone().filter(|_| role == LocalRole::Leader);
        state
            .store
            .update(move |s| {
                if let Some(hash) = password_hash {
                    s.settings.security.password_hash = Some(hash);
                }
                if let Some(tz) = tz2 {
                    s.schedule.location.timezone = tz;
                }
                Ok(())
            })
            .await?;
    }
    if let Some(tz) = tz {
        tokio::spawn(async move { sys::set_system_timezone(&tz).await });
    }
    crate::services::platform::publish_board(state);
    Ok(out)
}

/// Write the PPX1 record to the board EEPROM (sysfs when writable, else i2c-dev).
pub async fn write_eeprom(board: BoardKind, rev: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        #[cfg(target_os = "linux")]
        {
            let mut store = sys::open_board_eeprom(true)?;
            let record = pixelplus_hw::Ppx1Record::new(board, &rev);
            pixelplus_hw::eeprom::write_record(store.as_mut(), &record)
                .map_err(|e| e.to_string())?;
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

// ---------------------------------------------------------------------------
// Restore a show from a controller transfer file (F10, leader replacement)
// ---------------------------------------------------------------------------

use crate::services::snapshots::transfer;

/// Result of [`restore_transfer`].
#[derive(Debug, Clone, Default)]
pub struct RestoreOutcome {
    pub notes: Vec<String>,
    pub show_name: String,
    pub hostname: String,
    pub files: usize,
}

/// Free space a restore always leaves on the SD card.
const RESTORE_KEEP_FREE: u64 = 256 * 1024 * 1024;

/// Blocking `Read` over the upload's chunks (fed from the async handler).
pub struct ChunkReader {
    rx: tokio::sync::mpsc::Receiver<bytes::Bytes>,
    cur: bytes::Bytes,
}

impl ChunkReader {
    pub fn new(rx: tokio::sync::mpsc::Receiver<bytes::Bytes>) -> Self {
        ChunkReader {
            rx,
            cur: bytes::Bytes::new(),
        }
    }
}

impl std::io::Read for ChunkReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while self.cur.is_empty() {
            match self.rx.blocking_recv() {
                Some(b) => self.cur = b,
                None => return Ok(0),
            }
        }
        let n = buf.len().min(self.cur.len());
        buf[..n].copy_from_slice(&self.cur[..n]);
        self.cur = self.cur.slice(n..);
        Ok(n)
    }
}

fn restore_error(e: std::io::Error) -> ApiError {
    if transfer::wrong_passphrase(&e) {
        ApiError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "wrong_passphrase",
            "That passphrase doesn't open this transfer file.",
        )
    } else if transfer::no_space(&e) {
        ApiError::storage_full()
    } else {
        ApiError::bad_request(e.to_string())
    }
}

/// Make this new controller the show leader of a transfer file: the old
/// leader's id, cluster keys (its followers trust it at once), HTTPS
/// certificate authority, show, sequences and media, and host name.
///
/// Only on an unconfigured controller. `chunks` delivers the uploaded file;
/// everything is decrypted and unpacked into a staging directory first and
/// applied only once the whole file proved intact.
pub async fn restore_transfer(
    state: &AppState,
    passphrase: String,
    chunks: tokio::sync::mpsc::Receiver<bytes::Bytes>,
) -> ApiResult<RestoreOutcome> {
    if state.identity().role != LocalRole::Unconfigured {
        return Err(ApiError::conflict(
            "A show can only be restored onto a new (not yet set up) controller.",
        ));
    }
    let data_dir = state.config.data_dir.clone();
    let staging = data_dir.join(format!(
        ".transfer-restore-{}",
        crate::cluster::sig::random_hex(4)
    ));
    let free = sys::disk_space(&data_dir)
        .map(|(f, _)| f)
        .unwrap_or(u64::MAX);
    let budget = free.saturating_sub(RESTORE_KEEP_FREE);
    let stage_dir = staging.clone();
    let staged = tokio::task::spawn_blocking(move || {
        let reader = ChunkReader::new(chunks);
        let dec = transfer::Decryptor::new(reader, &passphrase)?;
        transfer::unpack(dec, &stage_dir, budget)
    })
    .await
    .map_err(ApiError::internal)?;
    let res = match staged {
        Ok(staged) => apply_restore(state, staged).await,
        Err(e) => Err(restore_error(e)),
    };
    let _ = tokio::fs::remove_dir_all(&staging).await;
    crate::services::snapshots::release_free_memory();
    res
}

async fn apply_restore(state: &AppState, staged: transfer::Staged) -> ApiResult<RestoreOutcome> {
    use pixelplus_core::model::{HardwareRecord, NodeRole};
    if staged.node.role != LocalRole::Leader {
        return Err(ApiError::bad_request(
            "That transfer file wasn't made on a show leader.",
        ));
    }
    if state.identity().role != LocalRole::Unconfigured {
        return Err(ApiError::conflict(
            "This controller was set up meanwhile; nothing was restored.",
        ));
    }
    let data_dir = state.config.data_dir.clone();
    let mut out = RestoreOutcome {
        show_name: staged.show.name.clone(),
        hostname: staged.manifest.hostname.clone(),
        files: staged.files.len(),
        ..Default::default()
    };
    // Validate what we are about to trust before touching anything.
    let keys: Option<crate::cluster::KeyStore> = match &staged.keys_json {
        Some(k) => Some(serde_json::from_slice(k).map_err(|_| {
            ApiError::bad_request("The cluster keys in the transfer file are damaged.")
        })?),
        None => None,
    };
    // 1. Data files (same file system: renames).
    {
        let (dir, files, data) = (staged.dir.clone(), staged.files.clone(), data_dir.clone());
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            for rel in files {
                let dst = data.join(&rel);
                if let Some(p) = dst.parent() {
                    std::fs::create_dir_all(p)?;
                }
                std::fs::rename(dir.join(&rel), &dst)?;
            }
            Ok(())
        })
        .await
        .map_err(ApiError::internal)?
        .map_err(|e| ApiError::internal(format!("couldn't move the show's files: {e}")))?;
    }
    // 2. The HTTPS certificate authority (phones keep trusting this controller).
    if let Some((key, crt, meta)) = &staged.ca {
        match crate::services::tls::import_ca(&data_dir, key, crt, meta) {
            Ok(()) => state.services.tls.poke(),
            Err(e) => out.notes.push(format!(
                "The secure-connection certificate couldn't be restored ({e:#}); phones must trust this controller again (Settings → Secure connection)."
            )),
        }
    }
    // 3. Identity: from now on this hardware *is* the old leader.
    let old = staged.node.clone();
    state
        .set_identity(|i| {
            i.id = old.id.clone();
            i.role = LocalRole::Leader;
            i.name = old.name.clone();
            i.cluster_key = old.cluster_key.clone();
            i.leader_id = None;
            i.leader_url = None;
        })
        .map_err(ApiError::internal)?;
    // 4. Follower keys.
    if let Some(cluster) = state.services.cluster.get() {
        let sh = &cluster.shared;
        let keys = keys.unwrap_or_default();
        sh.update_keys(|k| *k = keys.clone());
        *sh.replay.lock() = Default::default();
    } else if let Some(k) = &staged.keys_json {
        let dir = data_dir.join("cluster");
        let _ = std::fs::create_dir_all(&dir);
        let v: serde_json::Value = serde_json::from_slice(k).unwrap_or_default();
        crate::cluster::write_private_json(&dir.join("keys.json"), &v)
            .map_err(ApiError::internal)?;
    }
    // 5. The show. Tunnels belonged to the old hardware: set them up again.
    let mut show = staged.show.clone();
    crate::services::paths::sanitize_show(&mut show);
    if let Some(c) = show.settings.remote.cloudflare.as_mut() {
        if c.token_set {
            c.token_set = false;
            out.notes.push(
                "Cloudflare Tunnel: paste the tunnel token again (Settings → Remote access)."
                    .into(),
            );
        }
    }
    if show.settings.remote.tailscale.take().is_some() {
        out.notes
            .push("Tailscale: connect this controller again (Settings → Remote access).".into());
    }
    let (board, board_rev) = crate::cluster::net::local_board(state);
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    if let Some(n) = show
        .nodes
        .iter_mut()
        .find(|n| n.id == old.id && n.role == NodeRole::Leader)
    {
        n.hardware_history.push(HardwareRecord {
            at: now,
            serial: n.serial.take(),
            board: n.board,
            pi_model: n.pi_model.clone(),
            reason: "replaced from a transfer file".into(),
        });
        n.serial = crate::cluster::net::hardware_serial();
        n.board_rev = board_rev.clone();
        if n.board != board {
            n.outputs = crate::cluster::leader::fit_outputs(&n.outputs, board);
            n.board = board;
            out.notes.push(format!(
                "This controller is a {} board; the old leader was a different board. Check its outputs on the Controllers page.",
                board.display_name()
            ));
        }
    }
    state.store.replace(show).await.map_err(ApiError::from)?;
    if let Err(e) = crate::cluster::ensure_self_node(state).await {
        tracing::warn!("Couldn't update this controller in the restored show: {e:#}");
    }
    // 6. The old host name, so bookmarks and <name>.local keep working.
    let hostname = staged.manifest.hostname.trim().to_string();
    if !hostname.is_empty()
        && hostname != crate::cluster::net::hostname()
        && crate::services::network::validate_hostname(&hostname).is_ok()
    {
        if crate::services::network::managed() {
            crate::api::security::remember_previous_hostname(&crate::cluster::net::hostname());
            if let Err(e) = crate::services::platform::set_hostname(state, &hostname).await {
                out.notes.push(format!(
                    "The old name “{hostname}” couldn't be taken over ({e}); set it under Settings → Network."
                ));
            }
        } else {
            out.notes.push(format!(
                "The old leader was called “{hostname}”; rename this computer to match if bookmarks used that name."
            ));
        }
    }
    crate::services::platform::publish_board(state);
    tracing::info!(
        "Restored show “{}” from a transfer file ({} files); this controller is now leader {}",
        out.show_name,
        out.files,
        old.id
    );
    Ok(out)
}
