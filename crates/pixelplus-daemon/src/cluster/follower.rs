//! Follower side: adoption, release and commands from the leader; manifest and
//! slice download; clock pings; turning sync packets into player commands.
//!
//! Local files (under the data dir):
//!
//! * `cluster/manifest.json` – the last applied [`NodeManifest`]
//! * `cluster/slices.json`   – `{seqId: {key, sha256, bytes}}` of verified slices
//! * `sequences/<id>.ppseq`  – the slices themselves (`.ppseq.part` while downloading)
//! * `show.json`             – the follower-local show built from the manifest

use super::leader::{error_message, AdoptCall, AdoptReply};
use super::manifest::{self, ManifestSequence, NodeManifest};
use super::proto::{self, FileProgress, FollowerReport, Msg, Ping, Pong, SyncState};
use super::{net, sleep_or_stop, to_player, ClusterCommand, Shared, KEY_HEADER};
use crate::api::{ApiError, ApiResult};
use crate::node::LocalRole;
use crate::player::{OverlayCmd, PlayerCmd, PlayerState, SyncPacket, TestRequest, TestTarget};
use crate::state::AppState;
use anyhow::Context;
use axum::http::HeaderMap;
use pixelplus_core::model::{Node, NodeRole};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

/// A leader silent this long may be replaced by a forced adoption.
const LEADER_GONE: Duration = Duration::from_secs(30);

/// A verified local slice.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LocalSlice {
    pub key: String,
    pub sha256: String,
    pub bytes: u64,
}

type SliceIndex = HashMap<String, LocalSlice>;

fn manifest_path(sh: &Shared) -> PathBuf {
    sh.cluster_dir.join("manifest.json")
}

fn index_path(sh: &Shared) -> PathBuf {
    sh.cluster_dir.join("slices.json")
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let text = std::fs::read(path).ok()?;
    match serde_json::from_slice(&text) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!("ignoring unreadable {}: {e}", path.display());
            None
        }
    }
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        std::io::Write::write_all(&mut f, &serde_json::to_vec_pretty(value)?)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn slice_path(state: &AppState, seq_id: &str) -> PathBuf {
    state
        .config
        .data_dir
        .join(manifest::follower_slice_file(seq_id))
}

/// Restore what the follower knew before a restart.
pub(crate) fn load_local_state(state: &AppState, sh: &Shared) {
    let Some(m) = read_json::<NodeManifest>(&manifest_path(sh)) else {
        return;
    };
    let index: SliceIndex = read_json(&index_path(sh)).unwrap_or_default();
    let mut f = sh.follower.lock();
    f.manifest_version = m.show_version;
    f.local_sequences = m
        .sequences
        .iter()
        .filter(|s| {
            index.get(&s.id).is_some_and(|l| l.key == s.hash) && slice_path(state, &s.id).exists()
        })
        .map(|s| s.id.clone())
        .collect();
    let pending = m.sequences.len() - f.local_sequences.len();
    f.files = FileProgress {
        pending: pending as u32,
        total: m.sequences.len() as u32,
    };
}

pub(crate) fn spawn(state: &AppState, sh: &Arc<Shared>) {
    tokio::spawn(ping_loop(state.clone(), sh.clone()));
    tokio::spawn(manifest_loop(state.clone(), sh.clone()));
}

fn is_follower_with_leader(state: &AppState) -> bool {
    let i = state.identity();
    i.role == LocalRole::Follower
        && i.leader_id.is_some()
        && i.leader_url.is_some()
        && i.cluster_key.is_some()
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// This follower's status (sent to the leader inside beacons).
pub(crate) fn report(state: &AppState, sh: &Shared) -> FollowerReport {
    let identity = state.identity();
    let f = sh.follower.lock();
    let clock = sh.clock.lock();
    let contact = f
        .last_leader_contact
        .is_some_and(|t| t.elapsed() < sh.settings.offline_after);
    let problem = f.problem.clone().or_else(|| f.missing.clone());
    let sync_state = if identity.leader_id.is_none() || !contact {
        SyncState::Offline
    } else if clock.offset_ms().is_none()
        || f.syncing
        || f.files.pending > 0
        || problem.is_some()
        || (f.leader_show_version != 0 && f.leader_show_version != f.manifest_version)
    {
        SyncState::Syncing
    } else {
        SyncState::Synced
    };
    FollowerReport {
        state: sync_state,
        sync_offset_ms: clock.accuracy_ms().map(|a| (a * 10.0).round() / 10.0),
        clock_offset_ms: clock.offset_ms().map(|o| (o * 10.0).round() / 10.0),
        manifest_version: f.manifest_version,
        files: f.files,
        problem,
    }
}

/// Where to send pings / unicast beacons for the leader.
pub(crate) fn leader_addr(state: &AppState, sh: &Shared) -> Option<SocketAddr> {
    if let Some(a) = sh.follower.lock().leader_udp {
        return Some(a);
    }
    let url = state.identity().leader_url?;
    let host = reqwest::Url::parse(&url)
        .ok()?
        .host_str()?
        .trim_matches(['[', ']'])
        .to_string();
    let ip: std::net::IpAddr = host.parse().ok()?;
    Some(SocketAddr::new(ip, sh.settings.port))
}

fn touch_leader(sh: &Shared, src: SocketAddr, leader_version: Option<u64>) -> bool {
    let mut f = sh.follower.lock();
    f.last_leader_contact = Some(Instant::now());
    f.leader_udp = Some(src);
    if let Some(v) = leader_version {
        f.leader_show_version = v;
        return v != f.manifest_version;
    }
    false
}

// ---------------------------------------------------------------------------
// UDP handlers
// ---------------------------------------------------------------------------

/// The leader restarted: its clock starts over, so forget packet ordering.
fn leader_boot(sh: &Shared, boot: &str) {
    let restarted = sh.clock.lock().observe_boot(boot);
    if restarted {
        tracing::info!("the show leader restarted");
        sh.follower.lock().last_sync_sent_at = None;
    }
}

pub(crate) fn on_leader_beacon(state: &AppState, sh: &Shared, b: &proto::Beacon, src: SocketAddr) {
    leader_boot(sh, &b.boot);
    if touch_leader(sh, src, Some(b.show_version)) {
        sh.manifest_trigger.notify_one();
    }
    // Follow the leader to a new address (DHCP renumbering, moved to Wi-Fi…).
    let identity = state.identity();
    let Some(url) = identity
        .leader_url
        .as_deref()
        .and_then(|u| reqwest::Url::parse(u).ok())
    else {
        return;
    };
    let host_ip: Option<std::net::IpAddr> = url
        .host_str()
        .and_then(|h| h.trim_matches(['[', ']']).parse().ok());
    let port_ok = url.port_or_known_default() == Some(b.http);
    let host_ok = host_ip.is_some_and(|ip| ip == src.ip() || b.ips.contains(&ip));
    if !(port_ok && host_ok) {
        let new_url = net::http_url(src.ip(), b.http);
        tracing::info!("leader moved: {} → {new_url}", url);
        if let Err(e) = state.set_identity(|i| i.leader_url = Some(new_url.clone())) {
            tracing::warn!("could not save the new leader address: {e:#}");
        }
        sh.manifest_trigger.notify_one();
    }
}

pub(crate) fn on_pong(state: &AppState, sh: &Shared, p: Pong, src: SocketAddr) {
    let t2 = sh.now_ms();
    let identity = state.identity();
    if identity.role != LocalRole::Follower || identity.leader_id.as_deref() != Some(p.id.as_str())
    {
        return;
    }
    leader_boot(sh, &p.boot);
    sh.clock.lock().add(p.t0, p.t1, t2);
    touch_leader(sh, src, None);
}

/// Convert a leader sync packet to the local clock (see module docs of
/// [`crate::cluster`]): the returned packet's `sent_at_ms` is *local*
/// monotonic ms and `pos_ms` is the leader position at that instant.
pub fn localize_sync(mut p: SyncPacket, offset_ms: Option<f64>, local_now_ms: f64) -> SyncPacket {
    // Transit time of the packet. Implausible values (a stale estimate right
    // after a leader restart) count as zero rather than jumping the show.
    let age = offset_ms
        .map(|o| local_now_ms - (p.sent_at_ms as f64 - o))
        .filter(|a| (0.0..=2_000.0).contains(a))
        .unwrap_or(0.0);
    if p.state == PlayerState::Playing {
        p.pos_ms += age.round() as u64;
    }
    p.sent_at_ms = local_now_ms.round() as u64;
    p
}

pub(crate) async fn on_sync(state: &AppState, sh: &Shared, p: SyncPacket, src: SocketAddr) {
    let identity = state.identity();
    if identity.role != LocalRole::Follower
        || identity.leader_id.as_deref() != Some(p.leader.as_str())
    {
        return;
    }
    let local_now = sh.now_ms();
    {
        let mut f = sh.follower.lock();
        if let Some(last) = f.last_sync_sent_at {
            // Duplicate (unicast + broadcast) or reordered packet. A big step
            // back means the leader restarted: accept.
            if p.sent_at_ms == last || (p.sent_at_ms < last && last - p.sent_at_ms < 5_000) {
                return;
            }
        }
        f.last_sync_sent_at = Some(p.sent_at_ms);
        f.missing = match (&p.item, p.state) {
            (Some(item), PlayerState::Playing | PlayerState::Paused)
                if item.kind == "sequence" && !f.local_sequences.contains(&item.id) =>
            {
                Some(format!(
                    "“{}” is not downloaded yet, so it plays dark here",
                    item.name
                ))
            }
            _ => None,
        };
    }
    if touch_leader(sh, src, Some(p.show_version)) {
        sh.manifest_trigger.notify_one();
    }
    let offset = sh.clock.lock().offset_ms();
    to_player(state, PlayerCmd::Sync(localize_sync(p, offset, local_now))).await;
}

pub(crate) async fn on_overlay_packet(state: &AppState, sh: &Shared, data: &[u8]) {
    let identity = state.identity();
    if identity.role != LocalRole::Follower {
        return;
    }
    let Some(key) = identity.cluster_key.as_deref() else {
        return;
    };
    let Some(frame) = proto::decode_overlay(data, key) else {
        return;
    };
    let Some(prop) = state.store.get().prop(&frame.prop_id).cloned() else {
        return;
    };
    if frame.rgb.len() != prop.pixel_count as usize * 3 {
        tracing::debug!(
            "overlay for {} has {} bytes, expected {}",
            prop.name,
            frame.rgb.len(),
            prop.pixel_count * 3
        );
        return;
    }
    {
        let mut f = sh.follower.lock();
        let last = f.overlay_frames.get(&frame.prop_id).copied();
        if let Some(last) = last {
            let behind = last.wrapping_sub(frame.frame_no);
            if frame.frame_no == last || (behind > 0 && behind < 1_000) {
                return; // duplicate or late
            }
        }
        f.overlay_frames
            .insert(frame.prop_id.clone(), frame.frame_no);
    }
    to_player(
        state,
        PlayerCmd::Overlay(OverlayCmd::PropPixels {
            prop_id: frame.prop_id,
            rgb: frame.rgb,
        }),
    )
    .await;
}

async fn ping_loop(state: AppState, sh: Arc<Shared>) {
    let mut stop = sh.stop_rx();
    loop {
        if is_follower_with_leader(&state) {
            let identity = state.identity();
            if let Some(dest) = leader_addr(&state, &sh) {
                let ping = Msg::Ping(Ping {
                    id: identity.id.clone(),
                    t0: sh.now_ms(),
                });
                sh.send_json(&ping, identity.cluster_key.as_deref(), &[dest])
                    .await;
            }
        }
        if sleep_or_stop(&mut stop, sh.settings.ping_interval).await {
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// HTTP handlers (called from api::cluster)
// ---------------------------------------------------------------------------

fn valid_key(k: &str) -> bool {
    (16..=256).contains(&k.len())
        && k.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn validate_call(call: &AdoptCall) -> ApiResult<()> {
    if !super::slices::safe_id(&call.leader_id) {
        return Err(ApiError::bad_request("Invalid leader id."));
    }
    if !valid_key(&call.cluster_key) {
        return Err(ApiError::bad_request("Invalid cluster key."));
    }
    let url = reqwest::Url::parse(&call.leader_url)
        .map_err(|_| ApiError::bad_request("Invalid leader address."))?;
    if call.leader_url.len() > 256
        || !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
    {
        return Err(ApiError::bad_request("Invalid leader address."));
    }
    if let Some(n) = &call.name {
        super::leader::validate_node_name(n)?;
    }
    Ok(())
}

fn leader_label(sh: &Shared, leader_id: &str) -> String {
    sh.peers
        .read()
        .get(leader_id)
        .map(|p| format!("“{}” ({})", p.beacon.name, p.addr.ip()))
        .unwrap_or_else(|| leader_id.to_string())
}

/// `POST /cluster/adopt`.
pub async fn handle_adopt(
    state: &AppState,
    sh: &Shared,
    headers: &HeaderMap,
    call: AdoptCall,
) -> ApiResult<AdoptReply> {
    validate_call(&call)?;
    let identity = state.identity();
    if call.leader_id == identity.id {
        return Err(ApiError::bad_request("A controller cannot adopt itself."));
    }
    let has_key = crate::api::auth::has_cluster_key(state, headers);
    match identity.role {
        LocalRole::Unconfigured => {}
        LocalRole::Follower if identity.leader_id.is_none() => {}
        LocalRole::Follower => {
            let same = identity.leader_id.as_deref() == Some(call.leader_id.as_str())
                && identity
                    .cluster_key
                    .as_deref()
                    .is_some_and(|k| proto::ct_eq(k.as_bytes(), call.cluster_key.as_bytes()));
            let leader_gone = sh
                .follower
                .lock()
                .last_leader_contact
                .map_or(true, |t| t.elapsed() > LEADER_GONE);
            if !(has_key || same || (call.force && leader_gone)) {
                let current = leader_label(sh, identity.leader_id.as_deref().unwrap_or_default());
                let hint = if call.force {
                    "That leader is still online: release this controller there first."
                } else {
                    "Release it there first, or, if that leader is gone for good, take it over."
                };
                return Err(ApiError::conflict(format!(
                    "This controller already belongs to show leader {current}. {hint}"
                )));
            }
            if !same && !has_key {
                tracing::warn!(
                    "taken over from silent leader {:?} by {}",
                    identity.leader_id,
                    call.leader_id
                );
            }
        }
        LocalRole::Leader => {
            if !call.force {
                return Err(ApiError::conflict(
                    "This controller is itself a show leader. Adopting it replaces its show with the leader's; confirm to continue.",
                ));
            }
            // Keep the old show, just in case.
            let backup = sh.cluster_dir.join(format!(
                "show-before-adopt-{}.json",
                chrono::Utc::now().format("%Y%m%d%H%M%S")
            ));
            let show = state.store.get();
            if let Err(e) = write_json_atomic(&backup, &*show) {
                tracing::warn!("could not back up the show before adoption: {e:#}");
            }
        }
    }
    let changed_leader = identity.leader_id.as_deref() != Some(call.leader_id.as_str());
    let _guard = sh.install_lock.lock().await;
    let identity = state
        .set_identity(|i| {
            i.role = LocalRole::Follower;
            i.leader_id = Some(call.leader_id.clone());
            i.leader_url = Some(call.leader_url.clone());
            i.cluster_key = Some(call.cluster_key.clone());
            if let Some(n) = &call.name {
                i.name = Some(n.trim().to_string());
            }
        })
        .map_err(ApiError::internal)?;
    if changed_leader {
        sh.clock.lock().reset();
        let mut f = sh.follower.lock();
        f.last_sync_sent_at = None;
        f.leader_udp = None;
        f.leader_show_version = 0;
        f.problem = None;
        f.missing = None;
    }
    sh.manifest_trigger.notify_one();
    let hostname = net::hostname();
    let (board, board_rev) = net::local_board(state);
    tracing::info!(
        "adopted by leader {} at {}",
        call.leader_id,
        call.leader_url
    );
    state.events.toast(
        crate::events::ToastKind::Success,
        "This controller was adopted by the show leader",
    );
    Ok(AdoptReply {
        id: identity.id.clone(),
        name: identity
            .name
            .clone()
            .unwrap_or_else(|| net::title_case(&hostname)),
        hostname,
        board,
        board_rev,
        pi_model: net::pi_model(),
    })
}

/// This node as a standalone follower (no leader).
fn own_node(state: &AppState) -> Node {
    let identity = state.identity();
    let hostname = net::hostname();
    let (board, board_rev) = net::local_board(state);
    let existing = state.store.get().node(&identity.id).cloned();
    Node {
        id: identity.id.clone(),
        name: identity
            .name
            .clone()
            .unwrap_or_else(|| net::title_case(&hostname)),
        hostname,
        role: NodeRole::Follower,
        board,
        board_rev,
        pi_model: net::pi_model(),
        outputs: existing
            .filter(|n| n.board == board)
            .map(|n| n.outputs)
            .unwrap_or_else(|| board.default_outputs()),
        adopted: false,
        last_seen: None,
        notes: None,
    }
}

/// `POST /cluster/release`: forget the leader and go dark.
pub async fn handle_release(state: &AppState, sh: &Shared) -> ApiResult<()> {
    let identity = state.identity();
    if identity.role != LocalRole::Follower {
        return Err(ApiError::conflict("This controller is not a follower."));
    }
    let _guard = sh.install_lock.lock().await;
    state
        .set_identity(|i| {
            i.leader_id = None;
            i.leader_url = None;
            i.cluster_key = None;
        })
        .map_err(ApiError::internal)?;
    sh.clock.lock().reset();
    {
        let mut f = sh.follower.lock();
        *f = Default::default();
    }
    let _ = std::fs::remove_file(manifest_path(sh));
    let current = state.store.get();
    let dark = manifest::standalone_show(own_node(state), &current);
    state.store.replace(dark).await.map_err(ApiError::from)?;
    // Stop whatever the leader had us doing.
    let blank = SyncPacket {
        leader: String::new(),
        show_version: 0,
        state: PlayerState::Idle,
        item: None,
        pos_ms: 0,
        sent_at_ms: sh.now_ms().round() as u64,
        effect: None,
        test: None,
        brightness: 100,
        blackout: false,
    };
    to_player(state, PlayerCmd::Sync(blank)).await;
    tracing::info!("released by the leader; waiting to be adopted");
    state.events.toast(
        crate::events::ToastKind::Info,
        "This controller was released by its show leader",
    );
    Ok(())
}

/// `POST /cluster/command`.
pub async fn handle_command(state: &AppState, sh: &Shared, cmd: ClusterCommand) -> ApiResult<()> {
    if let ClusterCommand::Refresh = cmd {
        sh.manifest_trigger.notify_one();
        return Ok(());
    }
    let player = state
        .services
        .player
        .get()
        .cloned()
        .ok_or_else(|| ApiError::unavailable("The player is not running."))?;
    match cmd {
        ClusterCommand::TestStart { test } => player.test_start(test).await,
        ClusterCommand::Effect { effect } => {
            player
                .test_start(TestRequest {
                    mode: "effect".into(),
                    color: None,
                    speed: None,
                    target: TestTarget {
                        node_id: None,
                        output: None,
                        props: effect.target.clone(),
                    },
                    effect: Some(effect),
                })
                .await
        }
        ClusterCommand::TestStop => player.send(PlayerCmd::TestStop).await,
        ClusterCommand::Blackout { on } => player.send(PlayerCmd::Blackout(on)).await,
        ClusterCommand::OverlayEnable { prop_id, enabled } => {
            player
                .send(PlayerCmd::Overlay(OverlayCmd::Enable { prop_id, enabled }))
                .await
        }
        ClusterCommand::OverlayText {
            prop_id,
            text,
            color,
            scroll,
            duration_ms,
        } => {
            player
                .send(PlayerCmd::Overlay(OverlayCmd::Text {
                    prop_id,
                    text,
                    color,
                    scroll,
                    duration_ms,
                }))
                .await
        }
        ClusterCommand::OverlayQr {
            prop_id,
            url,
            duration_ms,
        } => {
            player
                .send(PlayerCmd::Overlay(OverlayCmd::Qr {
                    prop_id,
                    url,
                    duration_ms,
                }))
                .await
        }
        ClusterCommand::Refresh => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Manifest & slices
// ---------------------------------------------------------------------------

async fn manifest_loop(state: AppState, sh: Arc<Shared>) {
    let mut stop = sh.stop_rx();
    let mut failures: u32 = 0;
    loop {
        if is_follower_with_leader(&state) {
            sh.follower.lock().syncing = true;
            let result = sync_manifest(&state, &sh).await;
            let mut f = sh.follower.lock();
            f.syncing = false;
            match result {
                Ok(()) => {
                    failures = 0;
                    f.problem = None;
                }
                Err(e) => {
                    failures += 1;
                    let msg = format!("{e:#}");
                    if f.problem.as_deref() != Some(msg.as_str()) {
                        tracing::warn!("sync with leader failed: {msg}");
                    }
                    f.problem = Some(msg);
                }
            }
        }
        // Retry failures quickly at first, then back off (5 s … 30 s).
        let wait = if failures > 0 {
            Duration::from_secs((5 * failures as u64).min(30)).min(sh.settings.manifest_poll)
        } else {
            sh.settings.manifest_poll
        };
        tokio::select! {
            _ = sh.manifest_trigger.notified() => {
                // Coalesce bursts (every sync packet may notify).
                if sleep_or_stop(&mut stop, Duration::from_millis(200)).await { return; }
            }
            _ = tokio::time::sleep(wait) => {}
            _ = stop.changed() => return,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum FetchError {
    #[error("the leader is still preparing this sequence")]
    NotReady,
    #[error("{0}")]
    Other(String),
}

async fn fetch_manifest(state: &AppState, sh: &Shared) -> anyhow::Result<NodeManifest> {
    let identity = state.identity();
    let (Some(url), Some(key)) = (identity.leader_url.clone(), identity.cluster_key.clone()) else {
        anyhow::bail!("no leader configured");
    };
    let resp = sh
        .http
        .get(format!("{url}/api/v1/cluster/manifest/{}", identity.id))
        .header(KEY_HEADER, key)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("cannot reach the show leader at {url} ({e})"))?;
    match resp.status().as_u16() {
        200 => {}
        401 | 403 => {
            anyhow::bail!("the show leader rejected this controller's key; adopt it again")
        }
        404 => anyhow::bail!("the show leader no longer lists this controller"),
        _ => anyhow::bail!("the show leader answered: {}", error_message(resp).await),
    }
    let m: NodeManifest = resp.json().await.context("reading the manifest")?;
    if m.node.id != identity.id {
        anyhow::bail!("the leader sent the manifest of another controller");
    }
    Ok(m)
}

/// Install the follower show for `manifest` with the `available` sequences,
/// unless we meanwhile left that leader.
async fn install(
    state: &AppState,
    sh: &Shared,
    m: &NodeManifest,
    available: &HashSet<String>,
) -> anyhow::Result<bool> {
    let _guard = sh.install_lock.lock().await;
    let identity = state.identity();
    if identity.role != LocalRole::Follower
        || identity.leader_id.as_deref() != Some(m.leader_id.as_str())
    {
        anyhow::bail!("no longer following that leader");
    }
    let current = state.store.get();
    let avail: HashMap<String, bool> = available.iter().map(|id| (id.clone(), true)).collect();
    let show = manifest::follower_show(m, &avail, &current);
    if *current == show {
        return Ok(false);
    }
    state.store.replace(show).await?;
    Ok(true)
}

async fn sync_manifest(state: &AppState, sh: &Shared) -> anyhow::Result<()> {
    let m = fetch_manifest(state, sh).await?;
    {
        let (path, m2) = (manifest_path(sh), m.clone());
        tokio::task::spawn_blocking(move || write_json_atomic(&path, &m2)).await??;
    }
    let mut index: SliceIndex = read_json(&index_path(sh)).unwrap_or_default();
    let mut available: HashSet<String> = m
        .sequences
        .iter()
        .filter(|s| {
            index.get(&s.id).is_some_and(|l| l.key == s.hash) && slice_path(state, &s.id).exists()
        })
        .map(|s| s.id.clone())
        .collect();
    let pending: Vec<ManifestSequence> = m
        .sequences
        .iter()
        .filter(|s| !available.contains(&s.id))
        .cloned()
        .collect();
    let set_progress = |available: &HashSet<String>| {
        let mut f = sh.follower.lock();
        f.local_sequences = available.clone();
        f.files = FileProgress {
            pending: (m.sequences.len() - available.len()) as u32,
            total: m.sequences.len() as u32,
        };
    };
    set_progress(&available);
    if install(state, sh, &m, &available).await? {
        tracing::info!("applied show version {} from the leader", m.show_version);
    }
    sh.follower.lock().manifest_version = m.show_version;

    let mut first_error: Option<anyhow::Error> = None;
    for s in &pending {
        match download_slice(state, sh, s).await {
            Ok(local) => {
                index.insert(s.id.clone(), local);
                let (path, idx) = (index_path(sh), index.clone());
                tokio::task::spawn_blocking(move || write_json_atomic(&path, &idx)).await??;
                available.insert(s.id.clone());
                set_progress(&available);
            }
            Err(FetchError::NotReady) => {
                first_error.get_or_insert_with(|| {
                    anyhow::anyhow!("waiting for the leader to prepare “{}”", s.name)
                });
            }
            Err(FetchError::Other(e)) => {
                first_error
                    .get_or_insert_with(|| anyhow::anyhow!("downloading “{}” failed: {e}", s.name));
            }
        }
    }
    // Forget slices that are no longer part of the show.
    let wanted: HashSet<String> = m.sequences.iter().map(|s| s.id.clone()).collect();
    index.retain(|id, _| wanted.contains(id));
    {
        let (path, idx) = (index_path(sh), index.clone());
        tokio::task::spawn_blocking(move || write_json_atomic(&path, &idx)).await??;
    }
    let dir = state.config.sequences_dir();
    let removed = tokio::task::spawn_blocking(move || remove_unreferenced(&dir, &wanted)).await?;
    if removed > 0 {
        tracing::info!("removed {removed} sequence slices no longer in the show");
    }
    if install(state, sh, &m, &available).await? {
        tracing::info!(
            "{} of {} sequences ready",
            available.len(),
            m.sequences.len()
        );
    }
    match first_error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Delete `.ppseq` slices (and partial downloads) whose id is not wanted.
pub fn remove_unreferenced(dir: &Path, wanted: &HashSet<String>) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let id = ["ppseq.part.key", "ppseq.part", "ppseq"]
            .iter()
            .find_map(|ext| name.strip_suffix(&format!(".{ext}")));
        if let Some(id) = id {
            if !wanted.contains(id) && std::fs::remove_file(e.path()).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

/// Download one slice (resuming a partial download of the same key), verify
/// its sha256 and structure, and move it into place.
pub(crate) async fn download_slice(
    state: &AppState,
    sh: &Shared,
    s: &ManifestSequence,
) -> Result<LocalSlice, FetchError> {
    let other = |e: &dyn std::fmt::Display| FetchError::Other(e.to_string());
    let identity = state.identity();
    let (Some(url), Some(key)) = (identity.leader_url.clone(), identity.cluster_key.clone()) else {
        return Err(FetchError::Other("no leader configured".into()));
    };
    if !super::slices::safe_id(&s.id) {
        return Err(FetchError::Other("invalid sequence id".into()));
    }
    let final_path = slice_path(state, &s.id);
    let part = final_path.with_extension("ppseq.part");
    let part_key = final_path.with_extension("ppseq.part.key");
    if let Some(dir) = final_path.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| other(&e))?;
    }
    let etag = format!("\"{}\"", s.hash);
    let mut offset = 0u64;
    let resumable =
        tokio::fs::read_to_string(&part_key).await.ok().as_deref() == Some(s.hash.as_str());
    if resumable {
        offset = tokio::fs::metadata(&part)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
    } else {
        let _ = tokio::fs::remove_file(&part).await;
        tokio::fs::write(&part_key, &s.hash)
            .await
            .map_err(|e| other(&e))?;
    }

    let mut req = sh
        .http
        .get(format!(
            "{url}/api/v1/cluster/slice/{}/{}",
            identity.id, s.id
        ))
        .header(KEY_HEADER, &key);
    if offset > 0 {
        req = req
            .header("range", format!("bytes={offset}-"))
            .header("if-range", &etag);
    }
    let mut resp = req.send().await.map_err(|e| other(&e))?;
    match resp.status().as_u16() {
        200 => offset = 0,
        206 => {
            let cr = resp
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            if !cr.starts_with(&format!("bytes {offset}-")) {
                let _ = tokio::fs::remove_file(&part).await;
                return Err(FetchError::Other(format!("unexpected resume range {cr}")));
            }
        }
        416 => {
            // Our partial file is longer than the slice: start over next time.
            let _ = tokio::fs::remove_file(&part).await;
            return Err(FetchError::Other(
                "partial download was invalid; restarting".into(),
            ));
        }
        503 => return Err(FetchError::NotReady),
        _ => return Err(FetchError::Other(error_message(resp).await)),
    }
    let header = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(String::from)
    };
    if header("etag").as_deref() != Some(etag.as_str()) {
        // The leader's slice changed since we read the manifest.
        let _ = tokio::fs::remove_file(&part).await;
        sh.manifest_trigger.notify_one();
        return Err(FetchError::Other(
            "the sequence changed on the leader; retrying".into(),
        ));
    }
    let Some(expected_sha) = header("x-pixelplus-sha256") else {
        return Err(FetchError::Other(
            "the leader did not send a checksum".into(),
        ));
    };
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(offset == 0)
        .open(&part)
        .await
        .map_err(|e| other(&e))?;
    if offset > 0 {
        use tokio::io::AsyncSeekExt;
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|e| other(&e))?;
    }
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => file.write_all(&chunk).await.map_err(|e| other(&e))?,
            Ok(None) => break,
            // Keep what we have; the next attempt resumes.
            Err(e) => {
                let _ = file.flush().await;
                return Err(FetchError::Other(format!("connection lost ({e})")));
            }
        }
    }
    file.flush().await.map_err(|e| other(&e))?;
    file.sync_all().await.map_err(|e| other(&e))?;
    drop(file);

    let part2 = part.clone();
    let expected_len = s.frame_bytes;
    let verified = tokio::task::spawn_blocking(move || -> Result<(String, u64), String> {
        let sha = pixelplus_core::fseq::sha256_file(&part2).map_err(|e| e.to_string())?;
        let bytes = std::fs::metadata(&part2).map_err(|e| e.to_string())?.len();
        if sha != expected_sha {
            return Err("checksum mismatch".into());
        }
        let f = pixelplus_core::ppseq::PpseqFile::open(&part2).map_err(|e| e.to_string())?;
        if expected_len != 0 && f.frame_bytes() as u32 != expected_len {
            return Err(format!(
                "slice frame size {} ≠ expected {expected_len}",
                f.frame_bytes()
            ));
        }
        Ok((sha, bytes))
    })
    .await
    .map_err(|e| other(&e))?;
    let (sha256, bytes) = match verified {
        Ok(v) => v,
        Err(e) => {
            let _ = tokio::fs::remove_file(&part).await;
            let _ = tokio::fs::remove_file(&part_key).await;
            return Err(FetchError::Other(e));
        }
    };
    tokio::fs::rename(&part, &final_path)
        .await
        .map_err(|e| other(&e))?;
    let _ = tokio::fs::remove_file(&part_key).await;
    Ok(LocalSlice {
        key: s.hash.clone(),
        sha256,
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::ItemRef;

    fn pkt(state: PlayerState, pos: u64, sent: u64) -> SyncPacket {
        SyncPacket {
            leader: "l".into(),
            show_version: 1,
            state,
            item: Some(ItemRef {
                kind: "sequence".into(),
                id: "s".into(),
                name: "S".into(),
            }),
            pos_ms: pos,
            sent_at_ms: sent,
            effect: None,
            test: None,
            brightness: 100,
            blackout: false,
        }
    }

    #[test]
    fn localize_accounts_for_transit_time() {
        // Leader clock = local + 10_000. Sent at leader 20_000 (= local 10_000),
        // received at local 10_004: the leader has moved on by 4 ms.
        let p = localize_sync(
            pkt(PlayerState::Playing, 5_000, 20_000),
            Some(10_000.0),
            10_004.0,
        );
        assert_eq!(p.pos_ms, 5_004);
        assert_eq!(p.sent_at_ms, 10_004);

        // Paused: position frozen.
        let p = localize_sync(
            pkt(PlayerState::Paused, 5_000, 20_000),
            Some(10_000.0),
            10_004.0,
        );
        assert_eq!(p.pos_ms, 5_000);

        // No clock estimate yet: assume zero transit.
        let p = localize_sync(pkt(PlayerState::Playing, 5_000, 20_000), None, 77.0);
        assert_eq!((p.pos_ms, p.sent_at_ms), (5_000, 77));

        // Absurd ages (stale estimate) are ignored.
        let p = localize_sync(pkt(PlayerState::Playing, 0, 0), Some(1e9), 1.0);
        assert_eq!(p.pos_ms, 0);
        let p = localize_sync(pkt(PlayerState::Playing, 0, 100), Some(0.0), 1.0);
        assert_eq!(p.pos_ms, 0);
    }

    #[test]
    fn key_and_call_validation() {
        assert!(valid_key(&"a".repeat(64)));
        assert!(!valid_key("short"));
        assert!(!valid_key(&format!("{}!", "a".repeat(20))));
        let ok = AdoptCall {
            leader_id: "leader1".into(),
            leader_url: "http://10.0.0.2:80".into(),
            cluster_key: "k".repeat(64),
            force: false,
            name: None,
        };
        assert!(validate_call(&ok).is_ok());
        let mut bad = ok.clone();
        bad.leader_url = "file:///etc/passwd".into();
        assert!(validate_call(&bad).is_err());
        let mut bad = ok.clone();
        bad.leader_id = "../x".into();
        assert!(validate_call(&bad).is_err());
    }

    #[test]
    fn unreferenced_slices_are_removed() {
        let dir =
            std::env::temp_dir().join(format!("pp-unref-{}", pixelplus_core::model::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in [
            "keep.ppseq",
            "gone.ppseq",
            "gone.ppseq.part",
            "gone.ppseq.part.key",
            "song.fseq",
            "keep.ppseq.part",
        ] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let wanted = HashSet::from(["keep".to_string()]);
        assert_eq!(remove_unreferenced(&dir, &wanted), 3);
        assert!(dir.join("keep.ppseq").exists());
        assert!(dir.join("keep.ppseq.part").exists());
        assert!(
            dir.join("song.fseq").exists(),
            "leader-style files are never touched"
        );
        std::fs::remove_dir_all(dir).ok();
    }
}
