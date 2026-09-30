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
//! | [`proto`] | wire formats (JSON on the cluster port, binary overlay frames), HMAC, replay guard |
//! | [`sig`] | per-follower keys (X25519 at adoption), signed HTTP calls |
//! | [`clock`] | leader clock model (4-timestamp exchange, offset + drift fit) |
//! | [`manifest`] | leader → follower manifests and the follower-local show |
//! | [`slices`] | leader slice cache (keys, generation, cleanup) |
//! | [`net`] | interfaces, hostname, hardware, socket priority, kernel timestamps |
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
pub mod sig;
pub mod slices;

#[cfg(test)]
mod tests;

use crate::node::LocalRole;
use crate::player::{PlayerCmd, TestRequest};
use crate::state::{AppInner, AppState};
use parking_lot::{Mutex, RwLock};
use pixelplus_core::model::{BoardKind, EffectPreset, NodeRole};
use proto::{Beacon, FileProgress, SyncQuality, SyncState};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::{broadcast, watch, Notify};

pub use leader::ensure_self_node;

/// PixelPlus version announced in beacons.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// How long "Join another show" / "Allow a new leader" stays open.
pub const JOIN_WINDOW: Duration = Duration::from_secs(15 * 60);

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// Cluster runtime settings (from [`Config`](crate::config::Config) + environment).
///
/// | env | default | meaning |
/// |---|---|---|
/// | `PIXELPLUS_CLUSTER_PORT` | 32420 | UDP beacons / sync / clock (via `Config`) |
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
    /// Time between ping bursts (a fast 16-ping burst goes out after joining,
    /// a leader restart or a detected clock step).
    pub ping_interval: Duration,
    /// Pings per regular burst and their spacing.
    pub ping_burst: usize,
    pub ping_gap: Duration,
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
            ping_interval: Duration::from_secs(2),
            ping_burst: 5,
            ping_gap: Duration::from_millis(20),
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
    /// Clock / timeline sync quality (followers running protocol 2).
    #[serde(default)]
    pub sync: Option<SyncQuality>,
    /// Wi-Fi power saving is on (bad for sync), `None` when unknown / wired.
    #[serde(default)]
    pub wifi_power_save: Option<bool>,
    /// Cluster protocol version the node runs.
    #[serde(default)]
    pub protocol: u32,
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
    /// Two devices announce this id from different addresses (cloned SD card
    /// or an impostor): adoption is refused until it clears.
    #[serde(default)]
    pub duplicate: bool,
    /// A show leader whose admin chose "Join another show".
    #[serde(default)]
    pub joining: bool,
}

/// Commands the leader sends to followers (`POST /cluster/command`).
#[allow(clippy::large_enum_variant)] // TestRequest grew (map plans, F6); one per command
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
    /// The beacon carried a valid, fresh MAC for the key we share with it.
    pub authenticated: bool,
    /// Another device announced the same id from a different address
    /// recently (spoofing or a cloned SD card): shown as a possible duplicate.
    pub duplicate_until: Option<Instant>,
}

impl Peer {
    pub fn http_base(&self) -> String {
        net::http_url(self.addr.ip(), self.beacon.http)
    }

    pub fn duplicate(&self) -> bool {
        self.duplicate_until.is_some_and(|t| t > Instant::now())
    }
}

/// Keys this node holds besides `node.json` (`cluster/keys.json`, 0600).
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KeyStore {
    /// Leader: one key per adopted follower.
    #[serde(default)]
    pub followers: std::collections::BTreeMap<String, String>,
    /// Follower: our key comes from an adoption the leader has not used yet
    /// (the same leader may then repeat the adoption unsigned, e.g. after a
    /// timeout).
    #[serde(default)]
    pub pending: bool,
}

/// "Join another show" / "Allow a new leader", opened by a signed-in admin.
#[derive(Debug, Clone, Copy)]
pub(crate) struct JoinWindow {
    pub until: Instant,
    /// Only this leader address may adopt us (when the admin named one).
    pub leader_ip: Option<IpAddr>,
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
    /// Anchor epoch of the last sync packet (a restarted leader starts over).
    pub last_sync_epoch: Option<u64>,
    /// Leader boot id and highest sequence number of accepted overlay frames.
    pub overlay_boot: String,
    pub overlay_seq: u64,
    /// Pings sent recently: `t0` → record (a pong must answer one of them).
    pub pings: HashMap<u64, PingRecord>,
    /// Pings of the last 90 s and whether they were answered (loss %).
    pub ping_log: std::collections::VecDeque<(Instant, u64, bool)>,
    /// The leader runs another cluster protocol version.
    pub protocol_problem: Option<String>,
    /// Last challenge ping sent because of an unknown leader boot.
    pub last_challenge: Option<Instant>,
    /// Sequences whose slice is on disk and verified.
    pub local_sequences: std::collections::HashSet<String>,
    /// Power limiter budget from the installed manifest (F12).
    pub power: Option<pixelplus_core::model::NodePowerBudget>,
}

/// A ping in flight.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PingRecord {
    pub at: Instant,
    /// Cluster clock when it actually left (after encoding).
    pub sent_ms: f64,
    pub burst: u32,
    /// Already used to confirm a new leader run.
    pub confirmed: bool,
}

/// Leader-side health tracking per follower.
#[derive(Debug, Default, Clone)]
pub(crate) struct Health {
    pub online: Option<bool>,
    pub problem: Option<String>,
    pub last_readopt: Option<Instant>,
    /// Just adopted: its beacons from before the adoption (`adoptedBy: null`,
    /// unauthenticated) may still be in the peer table for a moment.
    pub adopted_at: Option<Instant>,
    pub warned_foreign: bool,
    pub warned_power_save: bool,
    pub warned_protocol: bool,
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
    pub clock: Mutex<clock::ClockModel>,
    /// Wakes the follower ping loop (fast burst after a reset).
    pub ping_trigger: Notify,
    /// The cluster socket delivers kernel receive timestamps.
    pub kernel_ts: AtomicBool,
    /// Smoothed time to encode + MAC a pong (ms), added to its `t2`.
    pub pong_encode_ms: Mutex<f64>,
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
    /// Follower keys (leader) / key state (follower), see [`KeyStore`].
    pub keys: Mutex<KeyStore>,
    /// Sequence number of the next authenticated packet we send.
    pub seq: AtomicU64,
    /// Boot ids and sequence numbers of authenticated senders.
    pub replay: Mutex<proto::ReplayGuard>,
    /// Nonces of signed HTTP requests we accepted.
    pub nonces: sig::NonceCache,
    /// Clock offset of peers (their unix time − ours), learnt from signed
    /// `X-PixelPlus-Time` answers.
    pub skew: Mutex<HashMap<String, i64>>,
    pub join: Mutex<Option<JoinWindow>>,
}

impl Shared {
    pub fn app(&self) -> Option<AppState> {
        self.app.upgrade().map(AppState)
    }

    /// Monotonic ms since daemon start (the cluster clock).
    pub fn now_ms(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1000.0
    }

    /// When a datagram arrived, on the cluster clock: its kernel receive
    /// timestamp where available (see [`net::enable_rx_timestamps`]),
    /// otherwise now.
    pub fn rx_time_ms(&self, stamp_ns: Option<i128>) -> f64 {
        let mono = self.now_ms();
        stamp_ns
            .and_then(|s| net::stamp_to_mono_ms(s, net::real_now_ns(), mono))
            .unwrap_or(mono)
    }

    pub fn stop_rx(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    pub fn emit(&self, event: ClusterEvent) {
        let _ = self.events.send(event);
    }

    /// Stamp for the next authenticated packet (key, boot id, fresh sequence number).
    pub fn stamp<'a>(&'a self, key: &'a str) -> proto::Stamp<'a> {
        proto::Stamp {
            key,
            boot: &self.boot,
            seq: self.seq.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// Send a JSON datagram on the cluster socket (best effort), authenticated
    /// with `key` when given.
    pub async fn send_json(&self, msg: &proto::Msg, key: Option<&str>, dests: &[SocketAddr]) {
        let bytes = self.encode_json(msg, key);
        self.send_bytes(&bytes, dests).await;
    }

    /// Serialize (and MAC) a cluster packet.
    pub fn encode_json(&self, msg: &proto::Msg, key: Option<&str>) -> Vec<u8> {
        proto::encode(msg, key.filter(|k| !k.is_empty()).map(|k| self.stamp(k)))
    }

    /// Send encoded bytes on the cluster socket (best effort).
    pub async fn send_bytes(&self, bytes: &[u8], dests: &[SocketAddr]) {
        let Some(sock) = self.socket.get() else {
            return;
        };
        if bytes.len() > proto::MAX_JSON_PACKET {
            tracing::warn!("cluster packet too large ({} bytes), not sent", bytes.len());
            return;
        }
        for d in dests {
            if let Err(e) = sock.send_to(bytes, d).await {
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

    fn keys_path(&self) -> PathBuf {
        self.cluster_dir.join("keys.json")
    }

    pub fn load_keys(&self) {
        let path = self.keys_path();
        if let Ok(bytes) = std::fs::read(&path) {
            match serde_json::from_slice::<KeyStore>(&bytes) {
                Ok(k) => *self.keys.lock() = k,
                Err(e) => tracing::warn!("ignoring unreadable {}: {e}", path.display()),
            }
        }
    }

    /// Change and persist the key store.
    pub fn update_keys(&self, f: impl FnOnce(&mut KeyStore)) {
        let snapshot = {
            let mut k = self.keys.lock();
            let before = k.clone();
            f(&mut k);
            if *k == before {
                return;
            }
            k.clone()
        };
        if let Err(e) = write_private_json(&self.keys_path(), &snapshot) {
            tracing::error!("could not save the cluster keys: {e:#}");
        }
    }

    /// Leader: the key shared with follower `id`. Followers adopted by an
    /// older PixelPlus still use the show-wide key (`node.json` clusterKey)
    /// until they are re-keyed.
    pub fn follower_key(&self, state: &AppState, id: &str) -> Option<String> {
        // Only for followers adopted in the show right now: a released or
        // removed follower has forgotten its key and accepts a fresh adoption.
        let adopted = state
            .store
            .get()
            .node(id)
            .is_some_and(|n| n.role == NodeRole::Follower && n.adopted);
        if !adopted {
            return None;
        }
        if let Some(k) = self.keys.lock().followers.get(id) {
            return Some(k.clone());
        }
        let identity = state.identity();
        if identity.role != LocalRole::Leader {
            return None;
        }
        identity.cluster_key.filter(|k| !k.is_empty())
    }

    /// Leader: `id` still uses the show-wide legacy key.
    pub fn uses_legacy_key(&self, id: &str) -> bool {
        !self.keys.lock().followers.contains_key(id)
    }

    /// The join window, if open.
    pub fn join_window(&self) -> Option<JoinWindow> {
        let mut j = self.join.lock();
        if j.is_some_and(|w| w.until <= Instant::now()) {
            *j = None;
        }
        *j
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

    /// Name a peer announces in its beacons (e.g. the follower's leader).
    pub fn peer_name(&self, id: &str) -> Option<String> {
        self.shared
            .peers
            .read()
            .get(id)
            .map(|p| p.beacon.name.clone())
            .filter(|n| !n.trim().is_empty())
    }

    /// Follower: this node's power limiter budget from the leader's manifest
    /// (F12; `None` on a leader or before the first manifest). A leader
    /// computes its own with `pixelplus_core::power::node_budget`.
    pub fn manifest_power(&self) -> Option<pixelplus_core::model::NodePowerBudget> {
        self.shared.follower.lock().power.clone()
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

/// Write `value` as JSON readable by this user only (atomic).
pub(crate) fn write_private_json<T: Serialize>(
    path: &std::path::Path,
    value: &T,
) -> anyhow::Result<()> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension(format!("tmp-{}", sig::random_hex(4)));
    {
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        o.mode(0o600);
        let mut f = o.open(&tmp)?;
        f.write_all(&serde_json::to_vec_pretty(value)?)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
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
        map_run_id: Default::default(),
        cal: Default::default(),
        identify: Default::default(),
        map: Default::default(),
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
        ping_trigger: Notify::new(),
        kernel_ts: AtomicBool::new(false),
        pong_encode_ms: Mutex::new(0.0),
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
        keys: Default::default(),
        seq: AtomicU64::new(1),
        replay: Default::default(),
        nonces: Default::default(),
        skew: Default::default(),
        join: Default::default(),
    });
    shared.load_keys();
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
