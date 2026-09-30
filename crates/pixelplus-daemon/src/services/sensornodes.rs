//! ESP32 sensor nodes (F20, ARCHITECTURE §7.5 and §12.16): discovery,
//! adoption, authenticated input events and heartbeats on UDP
//! `Config::sensor_port` (32422), live input states, and the sensor
//! triggers they fire.
//!
//! ## Wire protocol (version 1)
//! One JSON object per datagram, tagged by `"t"`:
//!
//! | `t` | From → to | MAC | Fields |
//! |---|---|---|---|
//! | `sbeacon` | node → broadcast, every 2 s (10 s once adopted) | no | `id, name, hw, ver, http, adoptedBy, inputs[], proto` |
//! | `sevent` | node → leader | yes | `id, input, state, ms, lb` |
//! | `sstatus` | node → leader, every 10 s | yes | `id, rssi, uptime, ver, cfg, inputs{}, amps{}, volts{}` |
//! | `sack` | leader → node | yes | `id, ack, sb, ok, lb, now, cfg` |
//! | `scmd` | leader → node | yes | `id, cmd` ("identify") |
//!
//! **MAC canonicalization** (identical to cluster datagrams, `cluster/proto.rs`):
//! the object is serialized, `,"bt":"<sender boot id>","sq":<sequence>` is
//! inserted before the closing brace, and `,"mac":"<64 hex>"}` replaces it,
//! where the MAC is HMAC-SHA256, keyed with the ASCII hex key, over every byte
//! before `,"mac"` plus a closing `}`. Test vectors shared with the firmware:
//! `firmware/esp32-sensor/test/vectors.json`.
//!
//! **Replay**: the leader keeps a [`ReplayGuard`] per node (boot id, highest
//! sequence number, 64-packet window). Events also carry `lb`, the leader
//! boot id the node last saw in a `sack`: an event with another `lb` (e.g.
//! captured before the leader restarted) fires nothing; the leader answers
//! `ok:false` with its current boot id and the node resends. A node's new
//! boot id is accepted from a heartbeat, or from an event with the current
//! `lb`. A `sack` echoes the node's boot id (`sb`) and the acknowledged
//! sequence number (`ack`), so a node never accepts an old acknowledgement.
//!
//! **Adoption** (TOFU like Pi followers): the leader POSTs
//! `http://<node>/adopt {leaderId, leaderUrl, sensorPort, dh}` and the node
//! answers `{id, dh, proof, hw, ver, inputs}`. Both derive
//! `key = hex(HMAC-SHA256(X25519 shared secret, "pixelplus-sensor-key-v1\n
//! <leaderId>\n<sensorId>\n<leaderPublic>\n<sensorPublic>"))`; `proof =
//! hex(HMAC(key, "pixelplus-sensor-adopted-v1\n<leaderId>\n<sensorId>"))`.
//! Keys live in `<data>/sensor-keys.json` (0600), never in `show.json`.
//! Release: `POST http://<node>/release`, signed like cluster calls
//! (`X-PixelPlus-Auth`, `cluster/sig.rs`). A node's configuration comes
//! from `GET /api/v1/cluster/sensor-config/<id>`, signed by the node.
//!
//! **What a key allows**: that node's events and heartbeats, and reading its
//! own configuration. A stolen or spoofed node can at most fire the
//! surprises configured for its inputs (rate-limited by the trigger's
//! cooldown and `maxPerHour`).
//!
//! ## Contracts for other services
//! * [`amps`]: latest current reading of a `kind: current` input (INA219 /
//!   INA226 on a receiver bus), for WS3's power limiter (`PowerSupply.sensor`).
//! * Sensor triggers (`TriggerKind::Sensor`) fire through WS3's
//!   `services::triggers::sensor_input` on every input event (it fires on the
//!   rising edge, `active` = state ≠ 0).

use crate::api::{ApiError, ApiResult};
use crate::cluster::proto::{ct_eq, hmac_sha256, Freshness, ReplayGuard};
use crate::cluster::sig;
use crate::state::AppState;
use parking_lot::{Mutex, RwLock};
use pixelplus_core::model::{SensorInput, SensorInputKind, SensorNode, SensorRef};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// Protocol version announced in beacons.
pub const PROTO: u32 = 1;
/// Largest datagram accepted.
pub const MAX_PACKET: usize = 2048;
/// A discovered node disappears from the list after this.
const DISCOVERY_TTL: Duration = Duration::from_secs(30);
/// A node without a heartbeat for this long is offline.
pub const OFFLINE_AFTER: Duration = Duration::from_secs(35);
/// Current readings older than this are not reported by [`amps`].
const AMPS_FRESH: Duration = Duration::from_secs(30);
/// Most inputs per node.
pub const MAX_INPUTS: usize = 8;
/// Most not-yet-adopted nodes remembered at once: beacons are unauthenticated,
/// so a flood of made-up ids from the LAN must not grow memory without end.
pub const MAX_DISCOVERED: usize = 64;

const MAC_HEX_LEN: usize = 64;
/// `,"mac":"` + 64 hex + `"}`
const MAC_SUFFIX_LEN: usize = 8 + MAC_HEX_LEN + 2;

// ---------------------------------------------------------------------------
// Wire format (pure; shared test vectors)
// ---------------------------------------------------------------------------

fn hex(b: &[u8]) -> String {
    pixelplus_core::fseq::to_hex(b)
}

/// A node's id from its Wi-Fi MAC address: `sn` + the last four bytes in hex
/// (10 characters, like every PixelPlus id). Accepts `:`/`-` separated or
/// plain hex, any case. The leader never sees a node's MAC address (nodes
/// announce their id), so this only checks the firmware's derivation against
/// the shared test vectors.
#[cfg(test)]
pub fn sensor_id_from_mac(mac: &str) -> Option<String> {
    let digits: String = mac
        .chars()
        .filter(|c| !matches!(c, ':' | '-' | '.'))
        .collect();
    if digits.len() != 12 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("sn{}", digits[4..].to_ascii_lowercase()))
}

/// The node key from the X25519 shared secret (see the module docs).
pub fn derive_key(
    shared: &[u8],
    leader_id: &str,
    sensor_id: &str,
    leader_public_hex: &str,
    sensor_public_hex: &str,
) -> String {
    let info = format!(
        "pixelplus-sensor-key-v1\n{leader_id}\n{sensor_id}\n{leader_public_hex}\n{sensor_public_hex}"
    );
    hex(&hmac_sha256(shared, info.as_bytes()))
}

/// The node's proof (in its adopt reply) that it derived the same key.
pub fn adopt_proof(key: &str, leader_id: &str, sensor_id: &str) -> String {
    hex(&hmac_sha256(
        key.as_bytes(),
        format!("pixelplus-sensor-adopted-v1\n{leader_id}\n{sensor_id}").as_bytes(),
    ))
}

/// Append boot id, sequence number and MAC to a serialized JSON object.
pub fn seal(object_json: &[u8], key: &str, boot: &str, seq: u64) -> Vec<u8> {
    let mut json = object_json.to_vec();
    while json.last().is_some_and(|b| b.is_ascii_whitespace()) {
        json.pop();
    }
    json.pop(); // closing '}'
    json.extend_from_slice(b",\"bt\":");
    json.extend_from_slice(serde_json::to_string(boot).unwrap_or_default().as_bytes());
    json.extend_from_slice(format!(",\"sq\":{seq}}}").as_bytes());
    let mac = hmac_sha256(key.as_bytes(), &json);
    json.pop();
    json.extend_from_slice(b",\"mac\":\"");
    json.extend_from_slice(hex(&mac).as_bytes());
    json.extend_from_slice(b"\"}");
    json
}

fn unhex32(s: &[u8]) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, pair) in s.chunks(2).enumerate() {
        let v = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
        out[i] = v(pair[0])? << 4 | v(pair[1])?;
    }
    Some(out)
}

/// The packet carries a valid MAC for `key`.
pub fn verify(packet: &[u8], key: &str) -> bool {
    if key.is_empty() || packet.len() < MAC_SUFFIX_LEN + 2 {
        return false;
    }
    let split = packet.len() - MAC_SUFFIX_LEN;
    let suffix = &packet[split..];
    if !suffix.starts_with(b",\"mac\":\"") || !suffix.ends_with(b"\"}") {
        return false;
    }
    let Some(mac) = unhex32(&suffix[8..8 + MAC_HEX_LEN]) else {
        return false;
    };
    let mut body = Vec::with_capacity(split + 1);
    body.extend_from_slice(&packet[..split]);
    body.push(b'}');
    ct_eq(&hmac_sha256(key.as_bytes(), &body), &mac)
}

/// Discovery beacon of a node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SBeacon {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub hw: String,
    #[serde(default)]
    pub ver: String,
    #[serde(default = "http80")]
    pub http: u16,
    #[serde(default)]
    pub adopted_by: Option<String>,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub proto: u32,
}

fn http80() -> u16 {
    80
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SEvent {
    pub id: String,
    pub input: String,
    pub state: i32,
    #[serde(default)]
    pub ms: u64,
    #[serde(default)]
    pub lb: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SStatus {
    pub id: String,
    #[serde(default)]
    pub rssi: i32,
    #[serde(default)]
    pub uptime: u64,
    #[serde(default)]
    pub ver: String,
    #[serde(default)]
    pub cfg: String,
    #[serde(default)]
    pub inputs: BTreeMap<String, i32>,
    #[serde(default)]
    pub amps: BTreeMap<String, f64>,
    #[serde(default)]
    pub volts: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "t")]
enum Incoming {
    #[serde(rename = "sbeacon")]
    Beacon(SBeacon),
    #[serde(rename = "sevent")]
    Event(SEvent),
    #[serde(rename = "sstatus")]
    Status(SStatus),
}

#[derive(Deserialize)]
struct StampFields {
    #[serde(default)]
    bt: Option<String>,
    #[serde(default)]
    sq: Option<u64>,
}

/// What a datagram meant.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Beacon(SBeacon),
    /// An authenticated, fresh input change.
    Input {
        node: String,
        input: String,
        state: i32,
    },
    /// An authenticated, fresh heartbeat.
    Status(SStatus),
    /// Ignored, with the reason (logged at debug level).
    Dropped(&'static str),
}

/// Leader side of the protocol: boot id, sequence numbers, replay state.
pub struct Engine {
    pub boot: String,
    seq: AtomicU64,
    guard: Mutex<ReplayGuard>,
}

impl Engine {
    pub fn new(boot: String) -> Engine {
        Engine {
            boot,
            seq: AtomicU64::new(1),
            guard: Mutex::new(ReplayGuard::default()),
        }
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }

    /// A MACed leader message to node `id`.
    pub fn message(&self, key: &str, mut obj: serde_json::Map<String, Value>) -> Vec<u8> {
        // Field order is fixed by insertion (serde_json preserve_order is off:
        // keys are sorted); any order is fine, the MAC covers the bytes sent.
        obj.remove("bt");
        obj.remove("sq");
        obj.remove("mac");
        let body = serde_json::to_vec(&Value::Object(obj)).unwrap_or_default();
        seal(&body, key, &self.boot, self.next_seq())
    }

    fn sack(&self, key: &str, id: &str, ack: u64, sb: &str, ok: bool, cfg: &str) -> Vec<u8> {
        let mut m = serde_json::Map::new();
        m.insert("t".into(), json!("sack"));
        m.insert("id".into(), json!(id));
        m.insert("ack".into(), json!(ack));
        m.insert("sb".into(), json!(sb));
        m.insert("ok".into(), json!(ok));
        m.insert("lb".into(), json!(self.boot));
        m.insert("now".into(), json!(chrono::Utc::now().timestamp()));
        m.insert("cfg".into(), json!(cfg));
        self.message(key, m)
    }

    /// Handle one datagram. `key_for(id)` is the adopted node's key;
    /// `cfg_for(id)` its configuration version. Returns what it meant and
    /// the reply to send back, if any.
    pub fn handle(
        &self,
        bytes: &[u8],
        key_for: impl Fn(&str) -> Option<String>,
        cfg_for: impl Fn(&str) -> String,
    ) -> (Outcome, Option<Vec<u8>>) {
        if bytes.len() > MAX_PACKET {
            return (Outcome::Dropped("too large"), None);
        }
        let Ok(msg) = serde_json::from_slice::<Incoming>(bytes) else {
            return (Outcome::Dropped("malformed"), None);
        };
        let (id, is_event) = match &msg {
            Incoming::Beacon(b) => {
                if !valid_id(&b.id) {
                    return (Outcome::Dropped("bad id"), None);
                }
                return (Outcome::Beacon(b.clone()), None);
            }
            Incoming::Event(e) => (e.id.clone(), true),
            Incoming::Status(s) => (s.id.clone(), false),
        };
        let Some(key) = key_for(&id) else {
            return (Outcome::Dropped("not adopted"), None);
        };
        if !verify(bytes, &key) {
            return (Outcome::Dropped("bad mac"), None);
        }
        let Ok(StampFields {
            bt: Some(bt),
            sq: Some(sq),
        }) = serde_json::from_slice::<StampFields>(bytes)
        else {
            return (Outcome::Dropped("no stamp"), None);
        };
        if bt.is_empty() || bt.len() > 64 {
            return (Outcome::Dropped("bad boot id"), None);
        }
        let lb_ok = match &msg {
            Incoming::Event(e) => e.lb == self.boot,
            _ => true,
        };
        let fresh = self.guard.lock().check(&id, &bt, sq, !is_event || lb_ok);
        let cfg = cfg_for(&id);
        match fresh {
            Freshness::Fresh => {}
            Freshness::Replayed => {
                // A retry whose ack got lost: acknowledge again, act once.
                return (
                    Outcome::Dropped("replayed"),
                    Some(self.sack(&key, &id, sq, &bt, lb_ok, &cfg)),
                );
            }
            Freshness::UnknownBoot => {
                return (
                    Outcome::Dropped("unknown boot"),
                    Some(self.sack(&key, &id, sq, &bt, false, &cfg)),
                );
            }
        }
        let reply = Some(self.sack(&key, &id, sq, &bt, lb_ok, &cfg));
        match msg {
            Incoming::Event(e) if lb_ok => (
                Outcome::Input {
                    node: id,
                    input: e.input,
                    state: e.state,
                },
                reply,
            ),
            Incoming::Event(_) => (Outcome::Dropped("stale leader boot"), reply),
            Incoming::Status(s) => (Outcome::Status(s), reply),
            Incoming::Beacon(_) => unreachable!("handled above"),
        }
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

// ---------------------------------------------------------------------------
// Configuration sent to a node
// ---------------------------------------------------------------------------

/// `GET /cluster/sensor-config/<id>` body.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NodeConfig {
    pub version: String,
    pub name: String,
    pub inputs: Vec<SensorInput>,
    pub status_every_sec: u32,
}

pub fn node_config(node: &SensorNode) -> NodeConfig {
    let inputs = node.inputs.clone();
    let digest = sig::sha256_hex(
        serde_json::to_string(&(&node.name, &inputs))
            .unwrap_or_default()
            .as_bytes(),
    );
    NodeConfig {
        version: digest[..8].to_string(),
        name: node.name.clone(),
        inputs,
        status_every_sec: 10,
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Check a node's inputs (ids, pins, timings).
pub fn validate_node(node: &SensorNode) -> ApiResult<()> {
    let name = node.name.trim();
    if name.is_empty() || name.chars().count() > 40 {
        return Err(ApiError::bad_request(
            "Give the sensor a name (40 characters at most).",
        ));
    }
    if node.inputs.len() > MAX_INPUTS {
        return Err(ApiError::bad_request(format!(
            "A sensor node has {MAX_INPUTS} inputs at most."
        )));
    }
    let mut seen = std::collections::HashSet::new();
    let mut pins = std::collections::HashSet::new();
    for i in &node.inputs {
        let label = if i.name.trim().is_empty() {
            i.id.as_str()
        } else {
            i.name.as_str()
        };
        if i.id.is_empty()
            || i.id.len() > 16
            || !i
                .id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return Err(ApiError::bad_request(format!(
                "\"{}\" isn't a valid input id (lowercase letters and digits, like pir1).",
                i.id
            )));
        }
        if !seen.insert(i.id.as_str()) {
            return Err(ApiError::bad_request(format!(
                "Two inputs are called \"{}\".",
                i.id
            )));
        }
        if i.name.chars().count() > 40 {
            return Err(ApiError::bad_request(
                "Input names are 40 characters at most.",
            ));
        }
        if i.kind == SensorInputKind::Current {
            if !(0x40..=0x4f).contains(&i.pin) {
                return Err(ApiError::bad_request(format!(
                    "{label}: a current sensor's I²C address is between 0x40 and 0x4F."
                )));
            }
            if !i
                .shunt_milliohms
                .is_some_and(|s| s.is_finite() && (0.05..=1000.0).contains(&s))
            {
                return Err(ApiError::bad_request(format!(
                    "{label}: enter the shunt resistance (0.05 to 1000 mΩ; 100 mΩ on most INA boards)."
                )));
            }
        } else {
            if i.pin > 48 {
                return Err(ApiError::bad_request(format!(
                    "{label}: GPIO {} doesn't exist on an ESP32.",
                    i.pin
                )));
            }
            if !pins.insert(i.pin) {
                return Err(ApiError::bad_request(format!(
                    "Two inputs use GPIO {}.",
                    i.pin
                )));
            }
        }
        if i.debounce_ms > 10_000 || i.hold_ms > 600_000 {
            return Err(ApiError::bad_request(format!(
                "{label}: debounce is at most 10 s and hold at most 10 minutes."
            )));
        }
    }
    Ok(())
}

/// Guess an input's kind from the id the firmware announces.
pub fn kind_from_id(id: &str) -> SensorInputKind {
    let id = id.to_ascii_lowercase();
    if id.starts_with("pir") || id.starts_with("motion") {
        SensorInputKind::Motion
    } else if id.starts_with("beam") || id.starts_with("ir") {
        SensorInputKind::Beam
    } else if id.starts_with("reed") || id.starts_with("contact") || id.starts_with("door") {
        SensorInputKind::Contact
    } else if id.starts_with("ina") || id.starts_with("amp") || id.starts_with("cur") {
        SensorInputKind::Current
    } else {
        SensorInputKind::Button
    }
}

fn default_name(kind: SensorInputKind, n: usize) -> String {
    let base = match kind {
        SensorInputKind::Motion => "Motion",
        SensorInputKind::Button => "Button",
        SensorInputKind::Beam => "Beam",
        SensorInputKind::Contact => "Contact",
        SensorInputKind::Current => "Current",
    };
    format!("{base} {n}")
}

// ---------------------------------------------------------------------------
// Runtime state
// ---------------------------------------------------------------------------

/// A node seen announcing itself.
#[derive(Debug, Clone)]
struct Discovered {
    beacon: SBeacon,
    ip: IpAddr,
    seen: Instant,
}

/// `GET /sensor-nodes/discovered` entry (`DiscoveredSensorNode` in types.ts).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredSensorNode {
    pub id: String,
    pub name: String,
    pub hw: String,
    pub ver: String,
    pub ip: Option<String>,
    pub adopted_by: Option<String>,
    pub inputs: Vec<String>,
}

/// Live state of an adopted node (`GET /sensor-nodes/:id/live`).
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Live {
    pub online: bool,
    pub rssi: Option<i32>,
    pub uptime_s: Option<u64>,
    pub ip: Option<String>,
    pub ver: Option<String>,
    /// RFC 3339.
    pub last_seen: Option<String>,
    pub inputs: BTreeMap<String, i32>,
    pub amps: BTreeMap<String, f64>,
    pub volts: BTreeMap<String, f64>,
    /// Input activations since the leader started.
    pub events: u64,
    /// Rejected datagrams (bad MAC, replays) since the leader started.
    pub rejected: u64,
}

#[derive(Debug, Clone, Default)]
struct LiveInner {
    live: Live,
    seen: Option<Instant>,
    addr: Option<SocketAddr>,
}

/// Runtime state of this service (`state.services.sensornodes`).
#[derive(Default)]
pub struct SensorNodesState {
    engine: OnceLock<Engine>,
    discovered: Mutex<HashMap<String, Discovered>>,
    live: Mutex<HashMap<String, LiveInner>>,
    keys: RwLock<Option<HashMap<String, String>>>,
    nonces: sig::NonceCache,
    /// The UDP socket while the listener is open (closed while Sensor nodes
    /// are off in Settings → Features).
    socket: Mutex<Option<Arc<tokio::net::UdpSocket>>>,
}

impl SensorNodesState {
    fn engine(&self) -> &Engine {
        self.engine.get_or_init(|| Engine::new(sig::random_hex(4)))
    }
}

fn keys_path(data_dir: &Path) -> PathBuf {
    data_dir.join("sensor-keys.json")
}

fn load_keys(state: &AppState) {
    let st = &state.services.sensornodes;
    if st.keys.read().is_some() {
        return;
    }
    let map: HashMap<String, String> = std::fs::read(keys_path(&state.config.data_dir))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let mut k = st.keys.write();
    if k.is_none() {
        *k = Some(map);
    }
}

fn key_of(state: &AppState, id: &str) -> Option<String> {
    load_keys(state);
    let adopted = state
        .store
        .get()
        .sensor_nodes
        .iter()
        .any(|n| n.id == id && n.adopted);
    if !adopted {
        return None;
    }
    state
        .services
        .sensornodes
        .keys
        .read()
        .as_ref()
        .and_then(|m| m.get(id).cloned())
}

fn save_keys(state: &AppState, f: impl FnOnce(&mut HashMap<String, String>)) -> ApiResult<()> {
    load_keys(state);
    let st = &state.services.sensornodes;
    let mut guard = st.keys.write();
    let map = guard.get_or_insert_with(HashMap::new);
    f(map);
    let path = keys_path(&state.config.data_dir);
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(&*map).map_err(ApiError::internal)?;
    // A leftover temp file may be readable: never write keys into it.
    let _ = std::fs::remove_file(&tmp);
    {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Latest current (A) of input `r` (a `kind: current` input), if the node
/// reported it in the last 30 s. **Contract for WS3** (`PowerSupply.sensor`).
pub fn amps(state: &AppState, r: &SensorRef) -> Option<f64> {
    let live = state.services.sensornodes.live.lock();
    let l = live.get(&r.sensor_node_id)?;
    if l.seen.map_or(true, |t| t.elapsed() > AMPS_FRESH) {
        return None;
    }
    l.live.amps.get(&r.input).copied()
}

/// Live state of node `id` (offline when never heard or silent).
pub fn live(state: &AppState, id: &str) -> Live {
    let map = state.services.sensornodes.live.lock();
    let Some(l) = map.get(id) else {
        return Live::default();
    };
    let mut out = l.live.clone();
    out.online = l.seen.is_some_and(|t| t.elapsed() < OFFLINE_AFTER);
    out
}

/// Nodes announcing themselves that this show hasn't adopted.
pub fn discovered(state: &AppState) -> Vec<DiscoveredSensorNode> {
    let show = state.store.get();
    let me = state.identity().id;
    let mut map = state.services.sensornodes.discovered.lock();
    map.retain(|_, d| d.seen.elapsed() < DISCOVERY_TTL);
    let mut out: Vec<DiscoveredSensorNode> = map
        .values()
        .filter(|d| {
            !(d.beacon.adopted_by.as_deref() == Some(me.as_str())
                && show
                    .sensor_nodes
                    .iter()
                    .any(|n| n.id == d.beacon.id && n.adopted))
        })
        .map(|d| DiscoveredSensorNode {
            id: d.beacon.id.clone(),
            name: d.beacon.name.clone(),
            hw: d.beacon.hw.clone(),
            ver: d.beacon.ver.clone(),
            ip: Some(d.ip.to_string()),
            adopted_by: d.beacon.adopted_by.clone(),
            inputs: d.beacon.inputs.clone(),
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn cfg_version(state: &AppState, id: &str) -> String {
    state
        .store
        .get()
        .sensor_nodes
        .iter()
        .find(|n| n.id == id)
        .map(|n| node_config(n).version)
        .unwrap_or_default()
}

/// Process one datagram from `from` (the UDP loop; tests call it directly).
/// Returns the reply to send.
pub async fn on_datagram(state: &AppState, bytes: &[u8], from: SocketAddr) -> Option<Vec<u8>> {
    let st = &state.services.sensornodes;
    let (outcome, reply) =
        st.engine()
            .handle(bytes, |id| key_of(state, id), |id| cfg_version(state, id));
    let now_rfc = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    match outcome {
        Outcome::Beacon(b) => {
            if b.proto != 0 && b.proto != PROTO {
                tracing::debug!("sensor node {} speaks protocol {}", b.id, b.proto);
            }
            let mut map = st.discovered.lock();
            if !map.contains_key(&b.id) && map.len() >= MAX_DISCOVERED {
                map.retain(|_, d| d.seen.elapsed() < DISCOVERY_TTL);
                if map.len() >= MAX_DISCOVERED {
                    // Still full of live beacons: forget the one heard least recently.
                    if let Some(oldest) = map
                        .iter()
                        .min_by_key(|(_, d)| d.seen)
                        .map(|(k, _)| k.clone())
                    {
                        map.remove(&oldest);
                    }
                }
            }
            map.insert(
                b.id.clone(),
                Discovered {
                    ip: from.ip(),
                    beacon: b,
                    seen: Instant::now(),
                },
            );
        }
        Outcome::Status(s) => {
            let mut map = st.live.lock();
            let l = map.entry(s.id.clone()).or_default();
            l.seen = Some(Instant::now());
            l.addr = Some(from);
            l.live.rssi = Some(s.rssi);
            l.live.uptime_s = Some(s.uptime);
            l.live.ip = Some(from.ip().to_string());
            l.live.ver = Some(s.ver.clone()).filter(|v| !v.is_empty());
            l.live.last_seen = Some(now_rfc);
            l.live.inputs = s.inputs;
            l.live.amps = s.amps;
            l.live.volts = s.volts;
        }
        Outcome::Input {
            node,
            input,
            state: value,
        } => {
            {
                let mut map = st.live.lock();
                let l = map.entry(node.clone()).or_default();
                l.seen = Some(Instant::now());
                l.addr = Some(from);
                l.live.ip = Some(from.ip().to_string());
                l.live.last_seen = Some(now_rfc.clone());
                l.live.inputs.insert(input.clone(), value);
                if value != 0 {
                    l.live.events += 1;
                }
            }
            let ev = json!({
                "sensorNodeId": node, "input": input, "state": value, "at": now_rfc
            });
            state.events.publish("sensorInput", &ev);
            let st2 = state.clone();
            tokio::spawn(async move { fire_triggers(&st2, &node, &input, value != 0).await });
        }
        Outcome::Dropped(why) => {
            tracing::debug!("sensor datagram from {from} dropped: {why}");
            if matches!(
                why,
                "bad mac" | "replayed" | "unknown boot" | "stale leader boot"
            ) {
                if let Some(id) = serde_json::from_slice::<Value>(bytes)
                    .ok()
                    .and_then(|v| v.get("id").and_then(Value::as_str).map(str::to_string))
                {
                    if let Some(l) = st.live.lock().get_mut(&id) {
                        l.live.rejected += 1;
                    }
                }
            }
        }
    }
    reply
}

/// Fire the sensor triggers bound to `node`/`input` through WS3's gated
/// entry point (`services::triggers::sensor_input`: when / window /
/// cooldown / per-hour limits, surprise or play actions, journal).
async fn fire_triggers(state: &AppState, node: &str, input: &str, active: bool) {
    for f in super::triggers::sensor_input(state, node, input, active).await {
        tracing::debug!("Sensor {node}/{input} → {}: {}", f.trigger_id, f.message);
    }
}

/// Send a MACed command to a node (`identify`).
pub async fn command(state: &AppState, id: &str, cmd: &str) -> ApiResult<()> {
    let key = key_of(state, id).ok_or_else(|| ApiError::not_found("That sensor"))?;
    let addr = state
        .services
        .sensornodes
        .live
        .lock()
        .get(id)
        .and_then(|l| l.addr)
        .ok_or_else(|| ApiError::unavailable("That sensor hasn't checked in yet."))?;
    let sock = state
        .services
        .sensornodes
        .socket
        .lock()
        .clone()
        .ok_or_else(|| ApiError::unavailable("Sensor nodes are turned off on this controller."))?;
    let mut m = serde_json::Map::new();
    m.insert("t".into(), json!("scmd"));
    m.insert("id".into(), json!(id));
    m.insert("cmd".into(), json!(cmd));
    let pkt = state.services.sensornodes.engine().message(&key, m);
    sock.send_to(&pkt, addr)
        .await
        .map_err(|e| ApiError::unavailable(format!("Couldn't reach the sensor: {e}")))?;
    Ok(())
}

/// Our address as seen from `peer` (the route the kernel would use).
fn local_ip_towards(peer: IpAddr) -> Option<IpAddr> {
    let bind: SocketAddr = if peer.is_ipv4() {
        "0.0.0.0:0".parse().ok()?
    } else {
        "[::]:0".parse().ok()?
    };
    let s = std::net::UdpSocket::bind(bind).ok()?;
    s.connect(SocketAddr::new(peer, 9)).ok()?;
    s.local_addr().ok().map(|a| a.ip())
}

#[derive(Deserialize)]
struct AdoptReply {
    id: String,
    dh: String,
    proof: String,
    #[serde(default)]
    hw: Option<String>,
    #[serde(default)]
    ver: Option<String>,
    #[serde(default)]
    inputs: Vec<AdoptInput>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdoptInput {
    id: String,
    #[serde(default)]
    pin: u8,
    #[serde(default)]
    kind: Option<SensorInputKind>,
    #[serde(default)]
    active_low: bool,
}

/// Adopt a discovered node: key exchange over HTTP, then add it to the show.
pub async fn adopt(state: &AppState, id: &str) -> ApiResult<SensorNode> {
    let me = state.identity().id;
    let d = state
        .services
        .sensornodes
        .discovered
        .lock()
        .get(id)
        .cloned()
        .ok_or_else(|| {
            ApiError::not_found("That sensor (it hasn't announced itself in the last 30 s)")
        })?;
    if let Some(other) = d.beacon.adopted_by.as_deref().filter(|o| *o != me) {
        return Err(ApiError::conflict(format!(
            "That sensor belongs to another show ({other}). Release it there first, or hold its BOOT button for 10 seconds to reset it."
        )));
    }
    let rng = ring::rand::SystemRandom::new();
    let private = ring::agreement::EphemeralPrivateKey::generate(&ring::agreement::X25519, &rng)
        .map_err(|_| ApiError::internal("no randomness for a key exchange"))?;
    let public = private
        .compute_public_key()
        .map_err(|_| ApiError::internal("key exchange failed"))?;
    let leader_pub = hex(public.as_ref());
    let leader_ip = local_ip_towards(d.ip)
        .ok_or_else(|| ApiError::unavailable("No network route to that sensor."))?;
    let http_port = state.config.http_addr.port();
    let leader_url = match leader_ip {
        IpAddr::V6(v6) => format!("http://[{v6}]:{http_port}"),
        v4 => format!("http://{v4}:{http_port}"),
    };
    let url = match d.ip {
        IpAddr::V6(v6) => format!("http://[{v6}]:{}/adopt", d.beacon.http),
        v4 => format!("http://{v4}:{}/adopt", d.beacon.http),
    };
    let resp = super::alerts::http_client()
        .post(&url)
        .json(&json!({
            "leaderId": me,
            "leaderUrl": leader_url,
            "sensorPort": state.config.sensor_port,
            "dh": leader_pub,
        }))
        .send()
        .await
        .map_err(|e| ApiError::unavailable(format!("Couldn't reach the sensor: {e}")))?;
    if resp.status() == reqwest::StatusCode::CONFLICT {
        return Err(ApiError::conflict(
            "The sensor refused: it belongs to another show. Hold its BOOT button for 10 seconds to reset it.",
        ));
    }
    if !resp.status().is_success() {
        return Err(ApiError::unavailable(format!(
            "The sensor answered {}.",
            resp.status()
        )));
    }
    let reply: AdoptReply = resp.json().await.map_err(|_| {
        ApiError::unavailable(
            "The sensor sent an answer PixelPlus doesn't understand. Update its firmware.",
        )
    })?;
    if reply.id != id || !sig::valid_public(&reply.dh) {
        return Err(ApiError::unavailable("The sensor's answer doesn't match."));
    }
    let peer_pub = (0..reply.dh.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&reply.dh[i..i + 2], 16).ok())
        .collect::<Vec<u8>>();
    let peer = ring::agreement::UnparsedPublicKey::new(&ring::agreement::X25519, peer_pub);
    let key = ring::agreement::agree_ephemeral(private, &peer, |shared| {
        derive_key(shared, &me, id, &leader_pub, &reply.dh)
    })
    .map_err(|_| ApiError::unavailable("Key exchange with the sensor failed."))?;
    if !ct_eq(
        adopt_proof(&key, &me, id).as_bytes(),
        reply.proof.trim().as_bytes(),
    ) {
        return Err(ApiError::unavailable(
            "The sensor couldn't prove it has the same key. Try again.",
        ));
    }
    let key2 = key.clone();
    let id2 = id.to_string();
    save_keys(state, move |m| {
        m.insert(id2, key2);
    })?;
    let announced: Vec<SensorInput> = if reply.inputs.is_empty() {
        d.beacon
            .inputs
            .iter()
            .enumerate()
            .map(|(i, input)| AdoptInput {
                id: input.clone(),
                pin: [4u8, 5, 6, 7, 10, 20, 21, 3][i % 8],
                kind: None,
                active_low: false,
            })
            .collect::<Vec<_>>()
    } else {
        reply.inputs
    }
    .into_iter()
    .take(MAX_INPUTS)
    .enumerate()
    .map(|(n, i)| {
        let kind = i.kind.unwrap_or_else(|| kind_from_id(&i.id));
        SensorInput {
            name: default_name(kind, n + 1),
            shunt_milliohms: (kind == SensorInputKind::Current).then_some(100.0),
            id: i.id,
            pin: i.pin,
            kind,
            active_low: i.active_low,
            debounce_ms: 30,
            hold_ms: 0,
        }
    })
    .collect();
    let hw = reply.hw.unwrap_or(d.beacon.hw.clone());
    let name = if d.beacon.name.trim().is_empty() {
        format!("Sensor {}", &id[id.len().saturating_sub(4)..])
    } else {
        d.beacon.name.chars().take(40).collect()
    };
    let id3 = id.to_string();
    let (node, _) = state
        .store
        .update(move |s| {
            let node = match s.sensor_nodes.iter_mut().find(|n| n.id == id3) {
                // Re-adoption (after a reset): keep names and trigger bindings.
                Some(n) => {
                    n.adopted = true;
                    n.hw = hw;
                    n.clone()
                }
                None => {
                    let n = SensorNode {
                        id: id3.clone(),
                        name,
                        hw,
                        location: None,
                        inputs: announced,
                        adopted: true,
                    };
                    s.sensor_nodes.push(n.clone());
                    n
                }
            };
            Ok(node)
        })
        .await?;
    if let Some(d) = state.services.sensornodes.discovered.lock().get_mut(id) {
        d.beacon.adopted_by = Some(me);
    }
    let _ = reply.ver;
    tracing::info!("Adopted sensor node {id} ({})", node.name);
    Ok(node)
}

/// Release a node: tell it (signed; best effort) and forget its key.
/// Returns a note when the node couldn't be told.
pub async fn release(state: &AppState, id: &str) -> ApiResult<Option<String>> {
    let show = state.store.get();
    if !show.sensor_nodes.iter().any(|n| n.id == id) {
        return Err(ApiError::not_found("That sensor"));
    }
    let key = key_of(state, id);
    let ip = state
        .services
        .sensornodes
        .live
        .lock()
        .get(id)
        .and_then(|l| l.addr.map(|a| a.ip()))
        .or_else(|| {
            state
                .services
                .sensornodes
                .discovered
                .lock()
                .get(id)
                .map(|d| d.ip)
        });
    let mut note = None;
    match (key, ip) {
        (Some(key), Some(ip)) => {
            let me = state.identity().id;
            let body = json!({ "leaderId": me }).to_string();
            let (auth, _) = sig::sign(&key, &me, "POST", "/release", body.as_bytes(), sig::now_s());
            let url = match ip {
                IpAddr::V6(v6) => format!("http://[{v6}]/release"),
                v4 => format!("http://{v4}/release"),
            };
            let r = super::alerts::http_client()
                .post(url)
                .header(sig::AUTH_HEADER, auth)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .timeout(Duration::from_secs(5))
                .body(body)
                .send()
                .await;
            if !r.is_ok_and(|r| r.status().is_success()) {
                note = Some(
                    "The sensor couldn't be told (offline?). Hold its BOOT button for 10 seconds to reset it before adopting it elsewhere."
                        .to_string(),
                );
            }
        }
        _ => {
            note = Some(
                "The sensor is offline. Hold its BOOT button for 10 seconds to reset it before adopting it elsewhere."
                    .to_string(),
            )
        }
    }
    let id2 = id.to_string();
    save_keys(state, move |m| {
        m.remove(&id2);
    })?;
    let id3 = id.to_string();
    state
        .store
        .update(move |s| {
            s.sensor_nodes.retain(|n| n.id != id3);
            Ok(())
        })
        .await?;
    state.services.sensornodes.live.lock().remove(id);
    tracing::info!("Released sensor node {id}");
    Ok(note)
}

/// Verify a node's signed config request and build the answer (the API
/// module adds the reply MAC). `path` is the full path + query as sent.
pub fn signed_config(
    state: &AppState,
    id: &str,
    auth: Option<&str>,
    path: &str,
) -> Result<(NodeConfig, String, String), sig::CheckError> {
    let key_for = |sender: &str| (sender == id).then(|| key_of(state, sender)).flatten();
    let (signed, key) = sig::check(
        auth,
        "GET",
        path,
        b"",
        &state.services.sensornodes.nonces,
        sig::now_s(),
        key_for,
    )?;
    let node = state
        .store
        .get()
        .sensor_nodes
        .iter()
        .find(|n| n.id == id)
        .cloned()
        .ok_or((sig::Refusal::Unauthenticated, None))?;
    Ok((node_config(&node), key, signed.nonce))
}

/// Start the service (called once from `services::start_all`).
pub fn start(state: &AppState) {
    let port = state.config.sensor_port;
    if port == 0 {
        return;
    }
    load_keys(state);
    let state = state.clone();
    tokio::spawn(async move {
        let mut changes = state.store.subscribe();
        loop {
            // Off in Settings → Features: the port stays closed until it's on.
            while !listening_wanted(&state) {
                if changes.changed().await.is_err() {
                    return;
                }
            }
            let sock = match tokio::net::UdpSocket::bind(("0.0.0.0", port)).await {
                Ok(s) => Arc::new(s),
                Err(e) => {
                    tracing::warn!("Sensor nodes unavailable: couldn't open UDP port {port}: {e}");
                    return;
                }
            };
            *state.services.sensornodes.socket.lock() = Some(sock.clone());
            tracing::info!("Listening for sensor nodes on UDP {port}");
            let mut buf = vec![0u8; MAX_PACKET + 1];
            loop {
                tokio::select! {
                    r = sock.recv_from(&mut buf) => {
                        let (n, from) = match r {
                            Ok(x) => x,
                            Err(e) => {
                                tracing::debug!("sensor socket: {e}");
                                tokio::time::sleep(Duration::from_millis(100)).await;
                                continue;
                            }
                        };
                        if let Some(reply) = on_datagram(&state, &buf[..n], from).await {
                            let _ = sock.send_to(&reply, from).await;
                        }
                    }
                    r = changes.changed() => {
                        if r.is_err() {
                            return;
                        }
                        if !listening_wanted(&state) {
                            break;
                        }
                    }
                }
            }
            *state.services.sensornodes.socket.lock() = None;
            drop(sock);
            tracing::info!("Sensor nodes turned off; UDP {port} closed");
        }
    });
}

/// Sensor nodes are on in Settings → Features.
fn listening_wanted(state: &AppState) -> bool {
    state
        .store
        .get()
        .feature(pixelplus_core::model::FeatureId::Sensors)
}

/// Whether the sensor node UDP listener is open (tests, status).
#[allow(dead_code)]
pub fn listening(state: &AppState) -> bool {
    state.services.sensornodes.socket.lock().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vectors() -> Value {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../firmware/esp32-sensor/test/vectors.json");
        serde_json::from_slice(&std::fs::read(p).expect("vectors.json")).unwrap()
    }

    fn s<'a>(v: &'a Value, k: &str) -> &'a str {
        v[k].as_str().unwrap()
    }

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn key_derivation_and_proof_match_the_shared_vectors() {
        let v = vectors();
        let kd = &v["keyDerivation"];
        let key = derive_key(
            &unhex(s(kd, "shared")),
            s(kd, "leaderId"),
            s(kd, "sensorId"),
            s(kd, "leaderPublic"),
            s(kd, "sensorPublic"),
        );
        assert_eq!(key, s(kd, "key"));
        let ap = &v["adoptProof"];
        assert_eq!(
            adopt_proof(s(ap, "key"), s(ap, "leaderId"), s(ap, "sensorId")),
            s(ap, "proof")
        );
        // The shared secret is the RFC 7748 one (what mbedTLS / ring compute).
        assert_eq!(s(&v["x25519"], "shared"), s(kd, "shared"));
    }

    #[test]
    fn datagram_canonicalization_matches_the_shared_vectors() {
        let v = vectors();
        for d in v["datagrams"].as_array().unwrap() {
            let packet = seal(
                s(d, "json").as_bytes(),
                s(d, "key"),
                s(d, "boot"),
                d["seq"].as_u64().unwrap(),
            );
            assert_eq!(
                String::from_utf8(packet.clone()).unwrap(),
                s(d, "packet"),
                "{}",
                s(d, "name")
            );
            assert!(verify(&packet, s(d, "key")));
            // One flipped byte anywhere breaks it.
            let mut bad = packet.clone();
            bad[10] ^= 1;
            assert!(!verify(&bad, s(d, "key")));
            assert!(!verify(&packet, &"0".repeat(64)));
        }
        // Same canonicalization as the cluster datagrams.
        let ping = crate::cluster::proto::Msg::Ping(
            serde_json::from_value(json!({"id":"n1","t0":1.5})).unwrap(),
        );
        let key = "k".repeat(64);
        let cluster = crate::cluster::proto::encode(
            &ping,
            Some(crate::cluster::proto::Stamp {
                key: &key,
                boot: "b",
                seq: 3,
            }),
        );
        let plain = crate::cluster::proto::encode(&ping, None);
        assert_eq!(seal(&plain, &key, "b", 3), cluster);
    }

    #[test]
    fn signed_requests_match_the_shared_vectors() {
        let v = vectors();
        for r in v["requests"].as_array().unwrap() {
            let signed = sig::parse(s(r, "header")).unwrap();
            assert_eq!(signed.sender, s(r, "sender"));
            assert!(signed.verify(
                s(r, "key"),
                s(r, "method"),
                s(r, "path"),
                s(r, "body").as_bytes()
            ));
            assert!(!signed.verify(
                s(r, "key"),
                s(r, "method"),
                "/other",
                s(r, "body").as_bytes()
            ));
        }
        let rp = &v["reply"];
        assert_eq!(sig::sha256_hex(s(rp, "body").as_bytes()), s(rp, "what"));
        assert_eq!(
            sig::reply_mac(s(rp, "key"), s(rp, "nonce"), s(rp, "what")),
            s(rp, "mac")
        );
        let tp = &v["timeProof"];
        assert_eq!(
            sig::time_proof(s(tp, "key"), tp["now"].as_i64().unwrap(), s(tp, "nonce")),
            s(tp, "header")
        );
        for m in v["sensorIds"].as_array().unwrap() {
            assert_eq!(sensor_id_from_mac(s(m, "mac")).as_deref(), Some(s(m, "id")));
        }
        assert_eq!(sensor_id_from_mac("24:6F:28:9C:1E"), None);
        assert_eq!(sensor_id_from_mac("zz:6F:28:9C:1E:2A"), None);
    }

    /// A node as the firmware behaves: stamps its packets.
    struct FakeNode {
        key: String,
        boot: String,
        seq: u64,
    }

    impl FakeNode {
        fn event(&mut self, input: &str, state: i32, lb: &str) -> Vec<u8> {
            self.seq += 1;
            let body = json!({"t":"sevent","id":"sn9c1e2a00","input":input,"state":state,"ms":1000,"lb":lb});
            seal(body.to_string().as_bytes(), &self.key, &self.boot, self.seq)
        }
        fn status(&mut self) -> Vec<u8> {
            self.seq += 1;
            let body = json!({"t":"sstatus","id":"sn9c1e2a00","rssi":-60,"uptime":5,"inputs":{"pir1":0},"amps":{"ina1":2.5}});
            seal(body.to_string().as_bytes(), &self.key, &self.boot, self.seq)
        }
    }

    fn ack_of(reply: &[u8], key: &str) -> Value {
        assert!(verify(reply, key), "sack must be MACed");
        serde_json::from_slice(reply).unwrap()
    }

    #[test]
    fn events_are_authenticated_fresh_and_bound_to_the_leader_boot() {
        let key = "7b8101026207edce5fd1255f3fa781b80b6c560cc0908416d605f3f47e81b32d".to_string();
        let e = Engine::new("b7d1c0de".into());
        let k = key.clone();
        let key_for = move |id: &str| (id == "sn9c1e2a00").then(|| k.clone());
        let cfg = |_: &str| "cfg1".to_string();
        let mut node = FakeNode {
            key: key.clone(),
            boot: "e5f00d01".into(),
            seq: 0,
        };

        // Before any sack the node doesn't know the leader boot: no trigger,
        // but an ack telling it the boot.
        let p = node.event("pir1", 1, "");
        let (o, r) = e.handle(&p, &key_for, cfg);
        assert_eq!(o, Outcome::Dropped("unknown boot"));
        let ack = ack_of(&r.unwrap(), &key);
        assert_eq!(
            (ack["ok"].as_bool(), ack["lb"].as_str()),
            (Some(false), Some("b7d1c0de"))
        );
        assert_eq!(ack["sb"], "e5f00d01");

        // Resent with the leader boot: fires once.
        let p = node.event("pir1", 1, "b7d1c0de");
        let (o, r) = e.handle(&p, &key_for, cfg);
        assert_eq!(
            o,
            Outcome::Input {
                node: "sn9c1e2a00".into(),
                input: "pir1".into(),
                state: 1
            }
        );
        let ack = ack_of(&r.unwrap(), &key);
        assert_eq!(ack["ack"], node.seq);
        assert_eq!(ack["ok"], true);
        assert_eq!(ack["cfg"], "cfg1");

        // The same packet again (lost ack / replay): acknowledged, not acted on.
        let (o, r) = e.handle(&p, &key_for, cfg);
        assert_eq!(o, Outcome::Dropped("replayed"));
        assert!(r.is_some());

        // Tampered, wrong key, unknown node.
        let mut bad = node.event("pir1", 1, "b7d1c0de");
        let i = bad.len() / 2;
        bad[i] ^= 0x20;
        assert_eq!(e.handle(&bad, &key_for, cfg).0, Outcome::Dropped("bad mac"));
        let mut other = FakeNode {
            key: "0".repeat(64),
            boot: "x".into(),
            seq: 0,
        };
        assert_eq!(
            e.handle(&other.event("pir1", 1, "b7d1c0de"), &key_for, cfg)
                .0,
            Outcome::Dropped("bad mac")
        );
        assert_eq!(
            e.handle(&p, |_: &str| None, cfg).0,
            Outcome::Dropped("not adopted")
        );

        // An event captured under an older leader boot fires nothing.
        let e2 = Engine::new("newboot1".into());
        let old = node.event("pir1", 1, "b7d1c0de");
        let (o, r) = e2.handle(&old, &key_for, cfg);
        assert_eq!(o, Outcome::Dropped("unknown boot"));
        assert_eq!(ack_of(&r.unwrap(), &key)["lb"], "newboot1");
        // A heartbeat establishes the node's boot with the new leader; an
        // event still needs the new leader boot.
        let (o, _) = e2.handle(&node.status(), &key_for, cfg);
        assert!(matches!(o, Outcome::Status(ref s) if s.amps["ina1"] == 2.5));
        let (o, _) = e2.handle(&node.event("pir1", 1, "b7d1c0de"), &key_for, cfg);
        assert_eq!(o, Outcome::Dropped("stale leader boot"));
        let (o, _) = e2.handle(&node.event("pir1", 0, "newboot1"), &key_for, cfg);
        assert!(matches!(o, Outcome::Input { state: 0, .. }));

        // A rebooted node (new boot id) is accepted; its old boot never again.
        let mut rebooted = FakeNode {
            key: key.clone(),
            boot: "a1a1a1a1".into(),
            seq: 0,
        };
        let (o, _) = e2.handle(&rebooted.event("pir1", 1, "newboot1"), &key_for, cfg);
        assert!(matches!(o, Outcome::Input { .. }));
        let (o, _) = e2.handle(&node.event("pir1", 1, "newboot1"), &key_for, cfg);
        assert_eq!(o, Outcome::Dropped("unknown boot"));
    }

    #[test]
    fn beacons_are_discovery_only() {
        let e = Engine::new("b".into());
        let b = br#"{"t":"sbeacon","id":"sn9c1e2a00","name":"PixelPlus-Sensor-1E2A","hw":"esp32c3","ver":"0.1.0","http":80,"adoptedBy":null,"inputs":["pir1","btn1"],"proto":1}"#;
        let (o, r) = e.handle(b, |_: &str| None, |_: &str| String::new());
        assert!(r.is_none());
        match o {
            Outcome::Beacon(b) => assert_eq!(b.inputs, vec!["pir1", "btn1"]),
            o => panic!("{o:?}"),
        }
        let bad = br#"{"t":"sbeacon","id":"../../x"}"#;
        assert_eq!(
            e.handle(bad, |_: &str| None, |_: &str| String::new()).0,
            Outcome::Dropped("bad id")
        );
        assert_eq!(
            e.handle(b"not json", |_: &str| None, |_: &str| String::new())
                .0,
            Outcome::Dropped("malformed")
        );
        let big = vec![b' '; MAX_PACKET + 1];
        assert_eq!(
            e.handle(&big, |_: &str| None, |_: &str| String::new()).0,
            Outcome::Dropped("too large")
        );
    }

    fn input(id: &str, pin: u8, kind: SensorInputKind) -> SensorInput {
        SensorInput {
            id: id.into(),
            name: String::new(),
            pin,
            kind,
            active_low: false,
            debounce_ms: 30,
            hold_ms: 0,
            shunt_milliohms: None,
        }
    }

    #[test]
    fn validation_and_config() {
        let mut n = SensorNode {
            id: "sn9c1e2a00".into(),
            name: "Sidewalk".into(),
            hw: "esp32c3".into(),
            location: None,
            inputs: vec![
                input("pir1", 4, SensorInputKind::Motion),
                input("btn1", 5, SensorInputKind::Button),
            ],
            adopted: true,
        };
        assert!(validate_node(&n).is_ok());
        n.inputs[1].pin = 4;
        assert!(validate_node(&n).is_err(), "same pin twice");
        n.inputs[1].pin = 5;
        n.inputs[1].id = "PIR 2".into();
        assert!(validate_node(&n).is_err());
        n.inputs[1].id = "pir1".into();
        assert!(validate_node(&n).is_err(), "duplicate id");
        n.inputs[1].id = "ina1".into();
        n.inputs[1].kind = SensorInputKind::Current;
        n.inputs[1].pin = 0x40;
        assert!(validate_node(&n).is_err(), "needs a shunt");
        n.inputs[1].shunt_milliohms = Some(1.5);
        assert!(validate_node(&n).is_ok());
        n.inputs[1].pin = 4; // an I²C address, not a GPIO: may equal a GPIO number
        assert!(validate_node(&n).is_err(), "address out of INA range");
        n.inputs[1].pin = 0x41;
        let c1 = node_config(&n);
        assert_eq!(c1.version.len(), 8);
        n.inputs[0].debounce_ms = 50;
        assert_ne!(node_config(&n).version, c1.version);
        assert_eq!(kind_from_id("pir2"), SensorInputKind::Motion);
        assert_eq!(kind_from_id("ina1"), SensorInputKind::Current);
        assert_eq!(kind_from_id("beam1"), SensorInputKind::Beam);
        assert_eq!(kind_from_id("btn3"), SensorInputKind::Button);
    }
}
