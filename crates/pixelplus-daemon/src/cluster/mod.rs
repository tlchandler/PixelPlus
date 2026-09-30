//! Leader / follower clustering (ARCHITECTURE §7, §10).
//!
//! "Configure everything on the leader": followers announce themselves with UDP
//! beacons (and mDNS), get adopted with one click, then automatically receive
//! their manifest (their node, props, looks, sequences), download their
//! `.ppseq` slice of every sequence, and follow the leader's playback via sync
//! packets and an NTP-style clock estimate.
//!
//! Every loop runs for the whole daemon lifetime and looks at the *current*
//! role on each iteration, so the setup wizard (or an adoption) can change the
//! role at runtime without a restart.
//!
//! | file | what |
//! |---|---|
//! | [`proto`] | wire formats (JSON on the cluster port, binary overlay frames), HMAC |
//! | [`clock`] | min-RTT clock offset filter |
//! | [`manifest`] | leader → follower manifests and the follower-local show |
//! | [`slices`] | leader slice cache (keys, generation, cleanup) |
//! | [`net`] | interfaces, hostname, hardware |
//! | [`leader`] | adoption, sync sender, node health, slice warming |
//! | [`follower`] | adopt/release/command handling, manifest + slice download, sync receiver |
//! | [`discovery`] | beacons, UDP receive loop, mDNS |

pub mod clock;
pub mod discovery;
pub mod follower;
pub mod leader;
pub mod manifest;
pub mod net;
pub mod proto;
pub mod slices;

#[cfg(test)]
mod tests;

use crate::node::LocalRole;
use crate::player::{PlayerCmd, TestRequest};
use crate::state::{AppInner, AppState};
use parking_lot::{Mutex, RwLock};
use pixelplus_core::model::{BoardKind, EffectPreset, NodeRole};
use proto::{Beacon, FileProgress, SyncState};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::{broadcast, watch, Notify};

pub use leader::ensure_self_node;

/// PixelPlus version announced in beacons.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// HTTP header carrying the cluster key.
pub const KEY_HEADER: &str = "x-pixelplus-key";

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// Cluster runtime settings (from [`Config`](crate::config::Config) + environment).
///
/// | env | default | meaning |
/// |---|---|---|
/// | `PIXELPLUS_CLUSTER_PORT` | 32320 | UDP beacons / sync / clock (via `Config`) |
/// | `PIXELPLUS_CLUSTER_OVERLAY_PORT` | port + 1 | UDP overlay frames |
/// | `PIXELPLUS_CLUSTER_BIND` | 0.0.0.0 | local address for both UDP sockets |
/// | `PIXELPLUS_CLUSTER_PEERS` | – | static peers `host[:port],…` (unicast beacons/sync; for networks without broadcast) |
/// | `PIXELPLUS_CLUSTER_BROADCAST` | 1 | `0` disables UDP broadcast |
/// | `PIXELPLUS_MDNS` | 1 | `0` disables the mDNS advertisement |
#[derive(Debug, Clone)]
pub struct ClusterSettings {
    pub port: u16,
    pub overlay_port: u16,
    pub bind: IpAddr,
    /// This node's HTTP port (announced in beacons).
    pub http_port: u16,
    pub peers: Vec<String>,
    pub broadcast: bool,
    pub mdns: bool,
    pub beacon_interval: Duration,
    /// A follower silent for this long is shown offline.
    pub offline_after: Duration,
    /// Followers re-check their manifest at least this often.
    pub manifest_poll: Duration,
    pub ping_interval: Duration,
    pub sync_interval: Duration,
}

impl ClusterSettings {
    pub fn from_config(config: &crate::config::Config) -> Self {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let off =
            |k: &str| env(k).is_some_and(|v| matches!(v.trim(), "0" | "false" | "off" | "no"));
        let port = config.cluster_port;
        ClusterSettings {
            port,
            overlay_port: env("PIXELPLUS_CLUSTER_OVERLAY_PORT")
                .and_then(|p| p.parse().ok())
                .unwrap_or(port.wrapping_add(1)),
            bind: env("PIXELPLUS_CLUSTER_BIND")
                .and_then(|b| b.parse().ok())
                .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            http_port: config.http_addr.port(),
            peers: env("PIXELPLUS_CLUSTER_PEERS")
                .map(|p| parse_peers(&p))
                .unwrap_or_default(),
            broadcast: !off("PIXELPLUS_CLUSTER_BROADCAST"),
            mdns: !off("PIXELPLUS_MDNS"),
            ..Self::defaults(port, config.http_addr.port())
        }
    }

    /// Production timings; tests shorten them.
    pub fn defaults(port: u16, http_port: u16) -> Self {
        ClusterSettings {
            port,
            overlay_port: port.wrapping_add(1),
            bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            http_port,
            peers: vec![],
            broadcast: true,
            mdns: true,
            beacon_interval: Duration::from_secs(2),
            offline_after: Duration::from_secs(6),
            manifest_poll: Duration::from_secs(30),
            ping_interval: Duration::from_secs(1),
            sync_interval: Duration::from_millis(250),
        }
    }
}

pub fn parse_peers(s: &str) -> Vec<String> {
    s.split([',', ' ', ';'])
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect()
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Cluster happenings for alerting (email / push are done by the alerts service).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ClusterEvent {
    #[serde(rename_all = "camelCase")]
    NodeOnline { node_id: String, name: String },
    #[serde(rename_all = "camelCase")]
    NodeOffline {
        node_id: String,
        name: String,
        last_seen: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    SyncProblem {
        node_id: String,
        name: String,
        message: String,
    },
}

/// One entry of `GET /nodes` status and the `nodes` WebSocket message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NodeStatus {
    pub id: String,
    pub name: String,
    pub role: NodeRole,
    pub adopted: bool,
    pub online: bool,
    pub last_seen: Option<String>,
    pub board: BoardKind,
    /// Estimated sync accuracy (ms); 0 when unknown / not applicable.
    pub sync_offset_ms: f64,
    pub sync_state: SyncState,
    pub files: FileProgress,
    pub ip: Option<String>,
    pub version: Option<String>,
    pub pi_model: Option<String>,
    pub hostname: String,
    pub problem: Option<String>,
}

/// An announced node that is not (or no longer) adopted by this leader.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredNode {
    pub id: String,
    pub name: String,
    pub hostname: String,
    pub role: LocalRole,
    pub board: BoardKind,
    pub board_rev: Option<String>,
    /// Raspberry Pi model.
    pub pi: Option<String>,
    pub ip: String,
    pub ips: Vec<IpAddr>,
    pub http: u16,
    /// PixelPlus version.
    pub ver: String,
    /// Leader that currently owns it (another leader), if any.
    pub adopted_by: Option<String>,
    pub last_seen: String,
}

/// Commands the leader sends to followers (`POST /cluster/command`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ClusterCommand {
    TestStart {
        test: TestRequest,
    },
    TestStop,
    Blackout {
        on: bool,
    },
    /// Show a look now (runs as an effect test on the follower).
    Effect {
        effect: EffectPreset,
    },
    #[serde(rename_all = "camelCase")]
    OverlayEnable {
        prop_id: String,
        enabled: bool,
    },
    #[serde(rename_all = "camelCase")]
    OverlayText {
        prop_id: String,
        text: String,
        color: String,
        #[serde(default)]
        scroll: bool,
        duration_ms: u64,
    },
    #[serde(rename_all = "camelCase")]
    OverlayQr {
        prop_id: String,
        url: String,
        duration_ms: u64,
    },
    /// Re-fetch the manifest and slices now.
    Refresh,
    /// Blink this controller's outputs (white chase) so it can be found.
    #[serde(rename_all = "camelCase")]
    Identify {
        duration_ms: u64,
    },
}

impl ClusterCommand {
    /// Pass every effect through `stamp_world_bounds` (ARCHITECTURE §7.4).
    pub fn stamp(&mut self, props: &[pixelplus_core::model::Prop]) {
        use pixelplus_core::effects::stamp_world_bounds;
        match self {
            ClusterCommand::TestStart { test } => {
                if let Some(e) = test.effect.as_mut() {
                    stamp_world_bounds(e, props);
                }
            }
            ClusterCommand::Effect { effect } => stamp_world_bounds(effect, props),
            _ => {}
        }
    }
}

/// Result of a command sent to one follower.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CommandResult {
    pub node_id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// A node heard on the network (any role).
#[derive(Debug, Clone)]
pub(crate) struct Peer {
    pub beacon: Beacon,
    /// UDP source address of its beacons (its cluster socket).
    pub addr: SocketAddr,
    pub last_seen: Instant,
    pub seen_at: chrono::DateTime<chrono::Utc>,
    /// The beacon carried a valid MAC for our cluster key.
    pub authenticated: bool,
}

impl Peer {
    pub fn http_base(&self) -> String {
        net::http_url(self.addr.ip(), self.beacon.http)
    }
}

/// Follower-side runtime state.
#[derive(Debug, Default)]
pub(crate) struct FollowerRuntime {
    pub manifest_version: u64,
    pub files: FileProgress,
    /// Manifest / download problem.
    pub problem: Option<String>,
    /// The leader plays something we do not have.
    pub missing: Option<String>,
    pub syncing: bool,
    /// Show version the leader last announced.
    pub leader_show_version: u64,
    pub last_leader_contact: Option<Instant>,
    pub leader_udp: Option<SocketAddr>,
    pub last_sync_sent_at: Option<u64>,
    pub overlay_frames: HashMap<String, u32>,
    /// Sequences whose slice is on disk and verified.
    pub local_sequences: std::collections::HashSet<String>,
}

/// Leader-side health tracking per follower.
#[derive(Debug, Default, Clone)]
pub(crate) struct Health {
    pub online: Option<bool>,
    pub problem: Option<String>,
    pub last_readopt: Option<Instant>,
    pub warned_foreign: bool,
}

/// Extra state carried in sync packets that the status alone cannot express.
#[derive(Debug, Default, Clone)]
pub(crate) struct SyncExtras {
    pub effect: Option<EffectPreset>,
    pub test: Option<TestRequest>,
}

pub(crate) struct Shared {
    pub app: Weak<AppInner>,
    pub settings: ClusterSettings,
    pub started: Instant,
    pub boot: String,
    pub socket: OnceLock<Arc<UdpSocket>>,
    pub overlay_socket: OnceLock<Arc<UdpSocket>>,
    pub http: reqwest::Client,
    pub events: broadcast::Sender<ClusterEvent>,
    pub peers: RwLock<HashMap<String, Peer>>,
    pub clock: Mutex<clock::ClockSync>,
    pub follower: Mutex<FollowerRuntime>,
    pub manifest_trigger: Notify,
    /// Serializes follower show installs with adopt / release, so a sync that
    /// was in flight can never re-install a show from a leader we just left.
    pub install_lock: tokio::sync::Mutex<()>,
    /// Wakes the leader sync sender immediately (show / extras changed).
    pub sync_trigger: Notify,
    pub slices: slices::SliceCache,
    pub health: Mutex<HashMap<String, Health>>,
    pub extras: Mutex<SyncExtras>,
    #[allow(dead_code)] // used by `forward_overlay`
    pub overlay_frame: AtomicU32,
    pub shutdown: watch::Sender<bool>,
    pub cluster_dir: PathBuf,
}

impl Shared {
    pub fn app(&self) -> Option<AppState> {
        self.app.upgrade().map(AppState)
    }

    /// Monotonic ms since daemon start (the cluster clock).
    pub fn now_ms(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1000.0
    }

    pub fn stop_rx(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    pub fn emit(&self, event: ClusterEvent) {
        let _ = self.events.send(event);
    }

    /// Send a JSON datagram on the cluster socket (best effort).
    pub async fn send_json(&self, msg: &proto::Msg, key: Option<&str>, dests: &[SocketAddr]) {
        let Some(sock) = self.socket.get() else {
            return;
        };
        let bytes = proto::encode(msg, key);
        if bytes.len() > proto::MAX_JSON_PACKET {
            tracing::warn!("cluster packet too large ({} bytes), not sent", bytes.len());
            return;
        }
        for d in dests {
            if let Err(e) = sock.send_to(&bytes, d).await {
                tracing::trace!("cluster send to {d} failed: {e}");
            }
        }
    }

    /// Resolve static peers (`host[:port]`).
    pub async fn static_peers(&self) -> Vec<SocketAddr> {
        let mut out = Vec::new();
        for p in &self.settings.peers {
            let with_port = if p.parse::<SocketAddr>().is_ok()
                || p.rsplit_once(':')
                    .is_some_and(|(h, port)| !h.contains(':') && port.parse::<u16>().is_ok())
            {
                p.clone()
            } else {
                format!("{p}:{}", self.settings.port)
            };
            match tokio::net::lookup_host(with_port).await {
                Ok(addrs) => out.extend(addrs.filter(|a| a.is_ipv4())),
                Err(e) => tracing::debug!("cluster peer {p}: {e}"),
            }
        }
        out
    }

    /// Broadcast destinations for `port` (global + per-subnet).
    pub fn broadcast_dests(&self, port: u16) -> Vec<SocketAddr> {
        if !self.settings.broadcast {
            return vec![];
        }
        let mut out = vec![SocketAddr::from((Ipv4Addr::BROADCAST, port))];
        for b in net::interfaces().broadcasts {
            out.push(SocketAddr::from((b, port)));
        }
        out
    }
}

/// Cheap, cloneable handle to the cluster service.
#[derive(Clone)]
pub struct ClusterHandle {
    pub(crate) shared: Arc<Shared>,
}

// Part of this API is for the player engine and the alerts service.
#[allow(dead_code)]
impl ClusterHandle {
    /// Cluster events for the alerts service.
    pub fn subscribe(&self) -> broadcast::Receiver<ClusterEvent> {
        self.shared.events.subscribe()
    }

    pub fn settings(&self) -> &ClusterSettings {
        &self.shared.settings
    }

    /// Monotonic cluster clock (ms since daemon start).
    pub fn now_ms(&self) -> f64 {
        self.shared.now_ms()
    }

    /// Leader: forward overlay pixels (prop order, `pixelCount × 3` RGB) of a
    /// prop to every follower that has some of its segments. Non-blocking;
    /// returns how many followers the frame was sent to.
    pub fn forward_overlay(&self, prop_id: &str, rgb: &[u8]) -> usize {
        leader::forward_overlay(&self.shared, prop_id, rgb)
    }

    /// Leader: the look currently shown (for sync packets) when it is not a
    /// saved preset the status `item` refers to. `None` clears it.
    pub fn set_sync_effect(&self, effect: Option<EffectPreset>) {
        self.shared.extras.lock().effect = effect;
        self.shared.sync_trigger.notify_one();
    }

    /// Leader: the test pattern currently running (carried in sync packets).
    pub fn set_sync_test(&self, test: Option<TestRequest>) {
        self.shared.extras.lock().test = test;
        self.shared.sync_trigger.notify_one();
    }

    /// Leader: send a command to one follower (`Some(id)`) or all adopted,
    /// online followers (`None`), in parallel. Effects are stamped here.
    pub async fn send_command(
        &self,
        node_id: Option<&str>,
        cmd: ClusterCommand,
    ) -> Vec<CommandResult> {
        leader::send_command(&self.shared, node_id, cmd).await
    }

    /// Current status of every node in the show (leader) or of this node and
    /// its leader (follower).
    pub fn nodes_status(&self) -> Vec<NodeStatus> {
        match self.shared.app() {
            Some(state) => leader::nodes_status(&state, &self.shared),
            None => vec![],
        }
    }

    /// Leader: unadopted nodes announcing themselves.
    pub fn discovered(&self) -> Vec<DiscoveredNode> {
        match self.shared.app() {
            Some(state) => leader::discovered(&state, &self.shared),
            None => vec![],
        }
    }

    /// Follower: re-check the manifest now.
    pub fn refresh_manifest(&self) {
        self.shared.manifest_trigger.notify_one();
    }

    /// Stop all cluster tasks (tests, graceful shutdown).
    pub fn shutdown(&self) {
        let _ = self.shared.shutdown.send(true);
    }
}

/// Sleep for `d`; returns `true` if shutdown was requested meanwhile.
pub(crate) async fn sleep_or_stop(stop: &mut watch::Receiver<bool>, d: Duration) -> bool {
    if *stop.borrow() {
        return true;
    }
    tokio::select! {
        _ = tokio::time::sleep(d) => *stop.borrow(),
        r = stop.changed() => r.is_err() || *stop.borrow(),
    }
}

/// Publish a warning to the UI log stream.
pub(crate) fn log_warning(state: &AppState, message: impl Into<String>) {
    let message = message.into();
    tracing::warn!("{message}");
    state.events.publish(
        "log",
        &serde_json::json!({
            "level": "warning",
            "message": message,
            "time": chrono::Utc::now().to_rfc3339(),
        }),
    );
}

/// Forward a command to the local player, if it is running.
pub(crate) async fn to_player(state: &AppState, cmd: PlayerCmd) -> bool {
    let Some(player) = state.services.player.get() else {
        return false;
    };
    matches!(
        tokio::time::timeout(Duration::from_millis(200), player.send(cmd)).await,
        Ok(Ok(()))
    )
}

/// Default length of an "identify" blink.
pub const IDENTIFY_MS: u64 = 5_000;

/// Blink every output of `node_id` (this node) with a white chase for
/// `duration_ms`, then stop the test (unless another test replaced it).
pub(crate) async fn identify_local(
    state: &AppState,
    node_id: &str,
    duration_ms: u64,
) -> crate::api::ApiResult<()> {
    let player = state
        .services
        .player
        .get()
        .cloned()
        .ok_or_else(|| crate::api::ApiError::unavailable("The player is not running."))?;
    let test = TestRequest {
        mode: "chase".into(),
        color: Some("#ffffff".into()),
        speed: None,
        target: crate::player::TestTarget {
            node_id: Some(node_id.to_string()),
            output: None,
            props: Default::default(),
        },
        effect: None,
    };
    player.test_start(test.clone()).await?;
    let duration = Duration::from_millis(duration_ms.clamp(1_000, 60_000));
    tokio::spawn(async move {
        tokio::time::sleep(duration).await;
        let status = player.status();
        // End the blink (if a test is still running).
        if status.state == crate::player::PlayerState::Testing {
            let _ = player.send(PlayerCmd::TestStop).await;
        }
    });
    Ok(())
}

// ---------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------

/// Start the cluster service and register its handle in `state.services`.
///
/// Never fails because of the network: sockets that cannot be bound are
/// retried in the background (and reported in the log).
pub async fn start(state: &AppState) -> anyhow::Result<ClusterHandle> {
    let settings = ClusterSettings::from_config(&state.config);
    start_with(state, settings).await
}

/// [`start`] with explicit settings (tests, custom ports).
pub async fn start_with(
    state: &AppState,
    settings: ClusterSettings,
) -> anyhow::Result<ClusterHandle> {
    let cluster_dir = state.config.data_dir.join("cluster");
    std::fs::create_dir_all(&cluster_dir)?;
    // Warm the hardware facts off the async runtime (I²C probing).
    let _ = tokio::task::spawn_blocking(crate::services::system::detection).await;

    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(4))
        .read_timeout(Duration::from_secs(30))
        .no_proxy()
        .user_agent(concat!("pixelplusd/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let (events, _) = broadcast::channel(64);
    let (shutdown, _) = watch::channel(false);
    let shared = Arc::new(Shared {
        app: Arc::downgrade(&state.0),
        started: state.started,
        boot: pixelplus_core::model::new_id(),
        socket: OnceLock::new(),
        overlay_socket: OnceLock::new(),
        http,
        events,
        peers: Default::default(),
        clock: Default::default(),
        follower: Default::default(),
        manifest_trigger: Notify::new(),
        install_lock: tokio::sync::Mutex::new(()),
        sync_trigger: Notify::new(),
        slices: slices::SliceCache::new(cluster_dir.join("slices")),
        health: Default::default(),
        extras: Default::default(),
        overlay_frame: AtomicU32::new(0),
        shutdown,
        cluster_dir,
        settings,
    });
    let handle = ClusterHandle {
        shared: shared.clone(),
    };
    let _ = state.services.cluster.set(handle.clone());

    follower::load_local_state(state, &shared);
    if state.identity().role == LocalRole::Leader {
        if let Err(e) = ensure_self_node(state).await {
            tracing::warn!("could not add this controller to the show: {e:#}");
        }
    }

    discovery::spawn(state, &shared);
    leader::spawn(state, &shared);
    follower::spawn(state, &shared);
    tracing::info!(
        "Cluster on UDP {} (overlay {}), role {:?}",
        shared.settings.port,
        shared.settings.overlay_port,
        state.identity().role
    );
    Ok(handle)
}
