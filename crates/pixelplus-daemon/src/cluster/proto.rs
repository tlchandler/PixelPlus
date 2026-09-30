//! Cluster wire formats (ARCHITECTURE §7, §10).
//!
//! **UDP cluster port** (default 32420; not FPP's 32320): one JSON object per datagram, tagged by
//! `"t"`: `beacon`, `sync`, `ping`, `pong`.
//!
//! **UDP overlay port** (cluster port + 1, default 32421): binary overlay frames
//! `u8 'O' | u8 idLen | propId | u32 frameNo (LE) | RGB… | 32-byte MAC`.
//!
//! ## Authentication
//! Adopted followers share a key with their leader (one key per follower, see
//! [`super::sig`]). Every packet between them is authenticated with
//! HMAC-SHA256 keyed by that key, and carries the sender's boot id and a
//! sequence number that grows with every packet it sends (replay protection,
//! see [`ReplayGuard`]):
//!
//! * JSON packets get `,"bt":"<boot>","sq":<seq>` and then a trailing
//!   `,"mac":"<64 hex>"}` appended to the serialized object; the MAC covers the
//!   object *without* the `mac` member (i.e. the exact bytes that precede it,
//!   plus the closing `}`). The packet stays ordinary JSON, so unauthenticated
//!   readers (e.g. `tcpdump`) still work.
//! * Overlay frames: `u8 'P' | u8 bootLen | boot | u64 seq (LE) | u8 idLen |
//!   propId | RGB… | 32-byte MAC` of everything before the MAC.
//!
//! Unadopted nodes send unauthenticated beacons (that is how discovery works),
//! and the leader broadcasts an unauthenticated beacon for them. Anything that
//! can change what a node *does* (sync, pong, the leader's address, overlay
//! pixels, follower status reports) is only accepted with a valid MAC and a
//! fresh sequence number.

use crate::node::LocalRole;
use crate::player::SyncPacket;
use pixelplus_core::model::BoardKind;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::net::IpAddr;

/// Cluster protocol version, announced in beacons (`proto`). Bumped whenever
/// the timing packets change meaning; a leader and its followers must run
/// the same version (a mismatch is shown on the Controllers page and in the
/// health check). History: 1 = ping/pong with 3 timestamps, sync packets with
/// integer-ms positions; 2 = 4-timestamp pong (`t2`), timeline anchors in sync
/// packets (`anchor`), sync-quality reports.
pub const PROTOCOL_VERSION: u32 = 2;
/// Oldest protocol this release still interoperates with (beacon
/// `protoMin`, F15 version tolerance). A future release that changes the
/// wire format keeps speaking the previous version while its peers are older,
/// so a cluster update never leaves controllers unable to talk.
pub const PROTOCOL_MIN: u32 = 2;
/// Newest protocol this release speaks (beacon `protoMax`).
pub const PROTOCOL_MAX: u32 = PROTOCOL_VERSION;

/// The protocol range a beacon announces (`protoMin..=protoMax`, else just `proto`).
pub fn proto_range(b: &Beacon) -> (u32, u32) {
    let min = b.proto_min.unwrap_or(b.proto);
    let max = b.proto_max.unwrap_or(b.proto).max(min);
    (min, max)
}

/// The protocol two nodes speak: the newest both support (the leader speaks
/// `min(leader.max, follower.max)`), `None` when their ranges don't overlap.
pub fn negotiate(a: (u32, u32), b: (u32, u32)) -> Option<u32> {
    let v = a.1.min(b.1);
    (v >= a.0.max(b.0)).then_some(v)
}

fn proto_v1() -> u32 {
    1
}

/// Largest JSON datagram we send or accept.
pub const MAX_JSON_PACKET: usize = 32 * 1024;
/// Largest overlay datagram (the IPv4 UDP payload limit).
pub const MAX_OVERLAY_PACKET: usize = 65_507;
/// Overlay frame type byte (v2: with boot id and sequence number).
pub const OVERLAY_MAGIC: u8 = b'P';

const MAC_HEX_LEN: usize = 64;
/// `,"mac":"` + 64 hex + `"}`
const MAC_SUFFIX_LEN: usize = 8 + MAC_HEX_LEN + 2;
const MAC_LEN: usize = 32;

/// Follower → leader status carried inside a follower's beacon.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct FollowerReport {
    /// "synced" | "syncing" | "offline" (leader not heard from).
    pub state: SyncState,
    /// Estimated sync accuracy: half the best round-trip time (ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_offset_ms: Option<f64>,
    /// Raw clock offset leader − follower (ms), for diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_offset_ms: Option<f64>,
    /// Show version of the manifest this follower has applied.
    #[serde(default)]
    pub manifest_version: u64,
    #[serde(default)]
    pub files: FileProgress,
    /// Human-readable problem (download failed, missing slice, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    /// Clock / timeline sync quality (protocol 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<SyncQuality>,
    /// Wi-Fi power saving is on (adds 50–1000 ms latency spikes): `None`
    /// when unknown or not on Wi-Fi.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wifi_power_save: Option<bool>,
    /// Power limiter activity (F12).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limiter: Option<LimiterReport>,
    /// Software update state (F15 cluster updates).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<UpdateReport>,
}

/// A follower's software-update state, in its beacon report (F15).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdateReport {
    /// Debian architecture (`arm64`, `amd64`).
    pub arch: String,
    /// It can install packages (packaged helper present, not Docker).
    #[serde(default)]
    pub can_apply: bool,
    /// Free space in the data directory (MB).
    #[serde(default)]
    pub disk_free_mb: u64,
    /// `idle` | `staging` | `staged` | `committing` | `rollingBack` | `failed`.
    pub phase: String,
    /// Version being staged / staged / installed by the last job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// What a follower's power limiter did (F12), in its beacon report.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct LimiterReport {
    /// Budget groups currently scaling (ids from the manifest's `power`).
    #[serde(default)]
    pub active_groups: Vec<String>,
    /// Lowest scale applied in the last report period (1 = none).
    pub min_scale: f32,
    /// Seconds spent limiting since the daemon started.
    #[serde(default)]
    pub seconds_limited: f32,
}

/// How well a follower follows its leader (Controllers page badge, health
/// check). All times in ms.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SyncQuality {
    /// Error bound of the leader-clock estimate: half the best round trip
    /// (path asymmetry) plus the residual RMS of the fit.
    pub offset_error_ms: f64,
    /// Residual RMS of the clock fit.
    pub jitter_ms: f64,
    /// Drift of the leader clock against this one (ppm).
    pub drift_ppm: f64,
    /// Round trip: best, median, 95th percentile (last 90 s).
    pub rtt_ms: f64,
    pub rtt_p50_ms: f64,
    pub rtt_p95_ms: f64,
    /// Pings without a pong (last 90 s), percent.
    pub loss_pct: f64,
    /// Clock samples in the window.
    pub samples: u32,
    /// Smoothed error of the player following the leader's timeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline_error_ms: Option<f64>,
    /// Display refresh of this controller's pixel output (Hz); frame changes
    /// land within ±half a refresh of the ideal instant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_hz: Option<f64>,
    /// Kernel receive timestamps are in use (userspace otherwise).
    #[serde(default)]
    pub kernel_timestamps: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SyncState {
    Synced,
    #[default]
    Syncing,
    Offline,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct FileProgress {
    pub pending: u32,
    pub total: u32,
}

/// Discovery / heartbeat beacon (ARCHITECTURE §7.1), sent every 2 s.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Beacon {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub hostname: String,
    pub role: LocalRole,
    pub board: BoardKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_rev: Option<String>,
    /// Raspberry Pi model string.
    #[serde(default, alias = "piModel", skip_serializing_if = "Option::is_none")]
    pub pi: Option<String>,
    /// PixelPlus version.
    pub ver: String,
    /// HTTP port.
    pub http: u16,
    /// Overlay UDP port (0 = cluster port + 1).
    #[serde(default)]
    pub overlay: u16,
    /// Leader id this node is adopted by (followers), `null` when unadopted.
    #[serde(default)]
    pub adopted_by: Option<String>,
    /// This node's IPv4 addresses.
    #[serde(default)]
    pub ips: Vec<IpAddr>,
    /// Random id per daemon start (detects restarts).
    #[serde(default)]
    pub boot: String,
    /// Leader: current show version. Follower: applied manifest version.
    #[serde(default)]
    pub show_version: u64,
    /// Followers only: sync / download status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<FollowerReport>,
    /// Its admin opened "Join another show": it accepts adoption for a while
    /// (lets a leader offer a controller that is itself a leader).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub joining: bool,
    /// Cluster protocol version ([`PROTOCOL_VERSION`]; absent = 1).
    #[serde(default = "proto_v1")]
    pub proto: u32,
    /// Oldest / newest protocol this node can speak (F15 version tolerance;
    /// absent = exactly `proto`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proto_min: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proto_max: Option<u32>,
    /// Hardware serial (board EEPROM, else the Pi's), F10: tells a retired
    /// controller from its replacement, which took over its id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hw: Option<String>,
}

/// Follower → leader clock probe.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Ping {
    pub id: String,
    /// Follower clock at send (ms since its daemon start).
    pub t0: f64,
}

/// Leader → follower clock reply.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Pong {
    /// Leader id.
    pub id: String,
    /// Echo of the ping's `t0`.
    pub t0: f64,
    /// Leader clock when the ping was received (kernel receive timestamp
    /// where available; ms since its daemon start).
    pub t1: f64,
    /// Leader clock just before the pong was sent (protocol 2; the leader's
    /// processing time `t2 − t1` is excluded from the round trip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t2: Option<f64>,
    /// Leader boot id (a change resets the follower's clock filter).
    #[serde(default)]
    pub boot: String,
}

/// Every JSON datagram on the cluster port.
#[allow(clippy::large_enum_variant)] // short-lived, one per datagram
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Msg {
    Beacon(Beacon),
    Sync(SyncPacket),
    Ping(Ping),
    Pong(Pong),
}

/// A decoded datagram and whether it carried a valid MAC for our key.
#[cfg(test)]
#[derive(Debug)]
pub struct Decoded<T> {
    pub msg: T,
    pub authenticated: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProtoError {
    #[error("packet too large ({0} bytes)")]
    TooLarge(usize),
    #[error("malformed packet: {0}")]
    Malformed(String),
}

// ---------------------------------------------------------------------------
// HMAC-SHA256 (RFC 2104)
// ---------------------------------------------------------------------------

pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..64 {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let inner = Sha256::new()
        .chain_update(ipad)
        .chain_update(msg)
        .finalize();
    Sha256::new()
        .chain_update(opad)
        .chain_update(inner)
        .finalize()
        .into()
}

/// Constant-time equality.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn decode_hex32(s: &[u8]) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, pair) in s.chunks(2).enumerate() {
        out[i] = hex_val(pair[0])? << 4 | hex_val(pair[1])?;
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// JSON packets
// ---------------------------------------------------------------------------

/// Authentication of an outgoing packet: key, our boot id, sequence number.
#[derive(Debug, Clone, Copy)]
pub struct Stamp<'a> {
    pub key: &'a str,
    pub boot: &'a str,
    pub seq: u64,
}

/// Serialize `msg`, appending boot id, sequence number and MAC when stamped.
pub fn encode(msg: &Msg, stamp: Option<Stamp>) -> Vec<u8> {
    let mut json = serde_json::to_vec(msg).expect("cluster messages always serialize");
    if let Some(st) = stamp.filter(|s| !s.key.is_empty()) {
        json.pop(); // closing '}'
        json.extend_from_slice(b",\"bt\":");
        json.extend_from_slice(
            serde_json::to_string(st.boot)
                .unwrap_or_default()
                .as_bytes(),
        );
        json.extend_from_slice(format!(",\"sq\":{}}}", st.seq).as_bytes());
        let mac = hmac_sha256(st.key.as_bytes(), &json);
        json.pop();
        json.extend_from_slice(b",\"mac\":\"");
        json.extend_from_slice(pixelplus_core::fseq::to_hex(&mac).as_bytes());
        json.extend_from_slice(b"\"}");
    }
    json
}

/// A parsed datagram, not yet authenticated.
#[derive(Debug)]
pub struct Raw<'a> {
    pub msg: Msg,
    /// Sender boot id and sequence number (authenticated packets only).
    pub boot: Option<String>,
    pub seq: Option<u64>,
    bytes: &'a [u8],
}

impl Raw<'_> {
    /// Id of the node that sent this packet.
    pub fn sender(&self) -> &str {
        match &self.msg {
            Msg::Beacon(b) => &b.id,
            Msg::Sync(p) => &p.leader,
            Msg::Ping(p) => &p.id,
            Msg::Pong(p) => &p.id,
        }
    }

    /// Carries a valid MAC for `key` (and a boot id + sequence number).
    pub fn verify(&self, key: &str) -> bool {
        !key.is_empty()
            && self.boot.is_some()
            && self.seq.is_some()
            && verify_json_mac(self.bytes, key)
    }
}

#[derive(Deserialize)]
struct StampFields {
    #[serde(default)]
    bt: Option<String>,
    #[serde(default)]
    sq: Option<u64>,
}

/// Parse a datagram (check it with [`Raw::verify`]).
pub fn parse(bytes: &[u8]) -> Result<Raw<'_>, ProtoError> {
    if bytes.len() > MAX_JSON_PACKET {
        return Err(ProtoError::TooLarge(bytes.len()));
    }
    let msg: Msg =
        serde_json::from_slice(bytes).map_err(|e| ProtoError::Malformed(e.to_string()))?;
    let st: StampFields =
        serde_json::from_slice(bytes).map_err(|e| ProtoError::Malformed(e.to_string()))?;
    Ok(Raw {
        msg,
        boot: st.bt.filter(|b| !b.is_empty() && b.len() <= 64),
        seq: st.sq,
        bytes,
    })
}

/// Parse a datagram and check its MAC against `key`.
#[cfg(test)]
pub fn decode(bytes: &[u8], key: Option<&str>) -> Result<Decoded<Msg>, ProtoError> {
    let raw = parse(bytes)?;
    let authenticated = key.is_some_and(|k| raw.verify(k));
    Ok(Decoded {
        msg: raw.msg,
        authenticated,
    })
}

fn verify_json_mac(bytes: &[u8], key: &str) -> bool {
    if bytes.len() < MAC_SUFFIX_LEN + 2 {
        return false;
    }
    let split = bytes.len() - MAC_SUFFIX_LEN;
    let suffix = &bytes[split..];
    if !suffix.starts_with(b",\"mac\":\"") || !suffix.ends_with(b"\"}") {
        return false;
    }
    let Some(mac) = decode_hex32(&suffix[8..8 + MAC_HEX_LEN]) else {
        return false;
    };
    let mut body = Vec::with_capacity(split + 1);
    body.extend_from_slice(&bytes[..split]);
    body.push(b'}');
    ct_eq(&hmac_sha256(key.as_bytes(), &body), &mac)
}

// ---------------------------------------------------------------------------
// Replay protection
// ---------------------------------------------------------------------------

/// What [`ReplayGuard::check`] decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Newer than anything seen from this sender: accepted and recorded.
    Fresh,
    /// Same boot, sequence number not newer (duplicate or replay).
    Replayed,
    /// A boot id we do not accept (older run of the sender, or not yet
    /// confirmed by a challenge).
    UnknownBoot,
}

/// How far below the highest sequence number a late packet is still accepted.
pub const REPLAY_WINDOW: u64 = 64;

/// Remembers, per sender, the current boot id, the highest sequence number
/// and boots that were replaced (a replay of an older run is refused).
#[derive(Debug, Default)]
pub struct ReplayGuard {
    peers: std::collections::HashMap<String, PeerSeq>,
}

#[derive(Debug, Default)]
struct PeerSeq {
    boot: String,
    seq: u64,
    /// Bit i set: `seq − 1 − i` was seen (reordering window).
    window: u64,
    old_boots: std::collections::VecDeque<String>,
}

impl ReplayGuard {
    /// Check `(boot, seq)` from `sender`. A new boot id is accepted only when
    /// `new_boot_ok` (the caller verified it is live) and it is not one we
    /// already saw replaced.
    pub fn check(&mut self, sender: &str, boot: &str, seq: u64, new_boot_ok: bool) -> Freshness {
        if self.peers.len() > 4096 && !self.peers.contains_key(sender) {
            self.peers.clear();
        }
        let p = self.peers.entry(sender.to_string()).or_default();
        if p.boot == boot {
            // Packets may arrive slightly out of order (the sender stamps them
            // from several tasks; Wi-Fi reorders): accept each sequence number
            // once within a window of REPLAY_WINDOW below the highest seen.
            if seq > p.seq {
                let shift = seq - p.seq;
                p.window = if shift > REPLAY_WINDOW {
                    0
                } else {
                    // The old highest becomes "seen" at bit shift − 1.
                    (p.window << shift) | (1u64 << (shift - 1))
                };
                p.seq = seq;
                return Freshness::Fresh;
            }
            let back = p.seq - seq;
            if back == 0 || back > REPLAY_WINDOW {
                return Freshness::Replayed;
            }
            let bit = 1u64 << (back - 1);
            if p.window & bit != 0 {
                return Freshness::Replayed;
            }
            p.window |= bit;
            return Freshness::Fresh;
        }
        if !new_boot_ok || p.old_boots.iter().any(|b| b == boot) {
            return Freshness::UnknownBoot;
        }
        if !p.boot.is_empty() {
            p.old_boots.push_back(std::mem::take(&mut p.boot));
            if p.old_boots.len() > 16 {
                p.old_boots.pop_front();
            }
        }
        p.boot = boot.to_string();
        p.seq = seq;
        p.window = 0;
        Freshness::Fresh
    }

    /// The boot id currently accepted for `sender`.
    pub fn boot(&self, sender: &str) -> Option<&str> {
        self.peers
            .get(sender)
            .map(|p| p.boot.as_str())
            .filter(|b| !b.is_empty())
    }

    /// Forget a sender (new leader, released follower).
    pub fn forget(&mut self, sender: &str) {
        self.peers.remove(sender);
    }
}

// ---------------------------------------------------------------------------
// Overlay frames
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayFrame {
    pub boot: String,
    pub seq: u64,
    pub prop_id: String,
    pub rgb: Vec<u8>,
}

/// Encode an overlay frame (always authenticated).
#[allow(dead_code)] // used by `ClusterHandle::forward_overlay`
pub fn encode_overlay(prop_id: &str, rgb: &[u8], stamp: Stamp) -> Result<Vec<u8>, ProtoError> {
    let id = prop_id.as_bytes();
    let boot = stamp.boot.as_bytes();
    if id.is_empty() || id.len() > 255 || boot.len() > 255 {
        return Err(ProtoError::Malformed(
            "prop / boot id must be 1..255 bytes".into(),
        ));
    }
    let len = 2 + boot.len() + 8 + 1 + id.len() + rgb.len() + MAC_LEN;
    if len > MAX_OVERLAY_PACKET {
        return Err(ProtoError::TooLarge(len));
    }
    let mut out = Vec::with_capacity(len);
    out.push(OVERLAY_MAGIC);
    out.push(boot.len() as u8);
    out.extend_from_slice(boot);
    out.extend_from_slice(&stamp.seq.to_le_bytes());
    out.push(id.len() as u8);
    out.extend_from_slice(id);
    out.extend_from_slice(rgb);
    let mac = hmac_sha256(stamp.key.as_bytes(), &out);
    out.extend_from_slice(&mac);
    Ok(out)
}

/// Decode an overlay frame; `None` unless well-formed *and* authenticated.
pub fn decode_overlay(bytes: &[u8], key: &str) -> Option<OverlayFrame> {
    if key.is_empty()
        || bytes.len() > MAX_OVERLAY_PACKET
        || bytes.len() < 2 + 8 + 1 + 1 + MAC_LEN
        || bytes[0] != OVERLAY_MAGIC
    {
        return None;
    }
    let (body, mac) = bytes.split_at(bytes.len() - MAC_LEN);
    if !ct_eq(&hmac_sha256(key.as_bytes(), body), mac) {
        return None;
    }
    let boot_len = body[1] as usize;
    let mut n = 2 + boot_len;
    if body.len() < n + 9 {
        return None;
    }
    let boot = std::str::from_utf8(&body[2..n]).ok()?.to_string();
    let seq = u64::from_le_bytes(body[n..n + 8].try_into().ok()?);
    n += 8;
    let id_len = body[n] as usize;
    n += 1;
    if id_len == 0 || body.len() < n + id_len {
        return None;
    }
    let prop_id = std::str::from_utf8(&body[n..n + id_len]).ok()?.to_string();
    let rgb = body[n + id_len..].to_vec();
    if rgb.len() % 3 != 0 {
        return None;
    }
    Some(OverlayFrame {
        boot,
        seq,
        prop_id,
        rgb,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::PlayerState;

    #[test]
    fn hmac_rfc4231_case_2() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            pixelplus_core::fseq::to_hex(&mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // Keys longer than the block size are hashed first (RFC 4231 case 6).
        let mac = hmac_sha256(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First",
        );
        assert_eq!(
            pixelplus_core::fseq::to_hex(&mac),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    fn st(key: &str, seq: u64) -> Stamp<'_> {
        Stamp {
            key,
            boot: "boot1",
            seq,
        }
    }

    fn sample_sync() -> Msg {
        Msg::Sync(SyncPacket {
            surprise: Default::default(),
            leader: "leader0001".into(),
            show_version: 7,
            state: PlayerState::Playing,
            item: None,
            pos_ms: 1234,
            sent_at_ms: 99_000,
            anchor: Some(crate::player::Anchor {
                pos_ms: 1234.567,
                at_ms: 99_000.125,
                rate: 1.000_012,
                epoch: 3,
            }),
            effect: None,
            test: None,
            brightness: 80,
            blackout: false,
        })
    }

    #[test]
    fn json_roundtrip_with_mac() {
        let msg = sample_sync();
        let bytes = encode(&msg, Some(st("secret", 1)));
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(text.starts_with("{\"t\":\"sync\""), "{text}");
        assert!(text.contains("\"mac\":\""));
        // Still valid JSON for any reader.
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["posMs"], 1234);

        let d = decode(&bytes, Some("secret")).unwrap();
        assert!(d.authenticated);
        assert_eq!(d.msg, msg);

        let d = decode(&bytes, Some("other")).unwrap();
        assert!(!d.authenticated);
        let d = decode(&bytes, None).unwrap();
        assert!(!d.authenticated);
    }

    #[test]
    fn tampered_packets_fail_authentication() {
        let bytes = encode(&sample_sync(), Some(st("secret", 1)));
        let text = String::from_utf8(bytes).unwrap().replace("1234", "9999");
        let d = decode(text.as_bytes(), Some("secret")).unwrap();
        assert!(!d.authenticated);
        assert!(matches!(&d.msg, Msg::Sync(p) if p.pos_ms == 9999));
    }

    #[test]
    fn unsigned_packets_decode_unauthenticated() {
        let bytes = encode(&sample_sync(), None);
        assert!(!std::str::from_utf8(&bytes).unwrap().contains("mac"));
        let d = decode(&bytes, Some("secret")).unwrap();
        assert!(!d.authenticated);
    }

    #[test]
    fn size_and_garbage_limits() {
        assert_eq!(
            decode(&vec![b' '; MAX_JSON_PACKET + 1], None).unwrap_err(),
            ProtoError::TooLarge(MAX_JSON_PACKET + 1)
        );
        assert!(decode(b"{\"t\":\"nope\"}", None).is_err());
        assert!(decode(b"\xff\xfe", None).is_err());
    }

    #[test]
    fn beacon_wire_format_matches_architecture() {
        let b = Msg::Beacon(Beacon {
            proto_max: Default::default(),
            proto_min: Default::default(),
            id: "abc".into(),
            name: "Garage".into(),
            hostname: "pixelplus-garage".into(),
            role: LocalRole::Follower,
            board: BoardKind::Difftx,
            board_rev: Some("E".into()),
            pi: Some("Raspberry Pi 4".into()),
            ver: "0.1.0".into(),
            http: 80,
            overlay: 32421,
            adopted_by: None,
            ips: vec!["10.0.0.5".parse().unwrap()],
            boot: "b".into(),
            show_version: 0,
            report: None,
            joining: false,
            proto: PROTOCOL_VERSION,
            hw: None,
        });
        let v: serde_json::Value = serde_json::from_slice(&encode(&b, None)).unwrap();
        assert_eq!(v["proto"], PROTOCOL_VERSION);
        assert_eq!(v["t"], "beacon");
        assert_eq!(v["board"], "difftx");
        assert_eq!(v["boardRev"], "E");
        assert_eq!(v["pi"], "Raspberry Pi 4");
        assert_eq!(v["adoptedBy"], serde_json::Value::Null);
        assert_eq!(v["http"], 80);
        assert_eq!(v["role"], "follower");
    }

    #[test]
    fn protocol_2_fields_are_optional_on_the_wire() {
        // An old (protocol 1) beacon and pong still parse.
        let old_beacon = br#"{"t":"beacon","id":"a","name":"A","role":"follower","board":"difftx","ver":"0.0.9","http":80}"#;
        match decode(old_beacon, None).unwrap().msg {
            Msg::Beacon(b) => assert_eq!(b.proto, 1),
            other => panic!("{other:?}"),
        }
        let old_pong = br#"{"t":"pong","id":"l","t0":1.5,"t1":9.25}"#;
        match decode(old_pong, None).unwrap().msg {
            Msg::Pong(p) => assert_eq!((p.t1, p.t2), (9.25, None)),
            other => panic!("{other:?}"),
        }
        // Positions keep sub-millisecond precision.
        let bytes = encode(&sample_sync(), None);
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["anchor"]["posMs"], 1234.567);
        assert_eq!(v["anchor"]["atMs"], 99_000.125);
        assert_eq!(v["anchor"]["epoch"], 3);
        let back = decode(&bytes, None).unwrap().msg;
        assert_eq!(back, sample_sync());
        // A protocol 1 sync packet (no anchor) is still understood.
        let text = String::from_utf8(bytes).unwrap();
        let cut = text.find(",\"anchor\"").unwrap();
        let end = text[cut..].find('}').unwrap() + cut + 1;
        let legacy = format!("{}{}", &text[..cut], &text[end..]);
        match decode(legacy.as_bytes(), None).unwrap().msg {
            Msg::Sync(p) => assert_eq!((p.anchor, p.pos_ms), (None, 1234)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn overlay_roundtrip() {
        let rgb: Vec<u8> = (0..30u8).collect();
        let bytes = encode_overlay("prop1", &rgb, st("k", 42)).unwrap();
        assert_eq!(bytes[0], b'P');
        let f = decode_overlay(&bytes, "k").unwrap();
        assert_eq!(
            f,
            OverlayFrame {
                boot: "boot1".into(),
                seq: 42,
                prop_id: "prop1".into(),
                rgb
            }
        );
        assert!(decode_overlay(&bytes, "wrong").is_none());
        assert!(decode_overlay(&bytes, "").is_none());
        let mut bad = bytes.clone();
        bad[10] ^= 1;
        assert!(decode_overlay(&bad, "k").is_none());
        assert!(encode_overlay("p", &vec![0; MAX_OVERLAY_PACKET], st("k", 1)).is_err());
    }

    #[test]
    fn stamped_packets_carry_boot_and_seq() {
        let bytes = encode(&sample_sync(), Some(st("secret", 77)));
        let raw = parse(&bytes).unwrap();
        assert_eq!((raw.boot.as_deref(), raw.seq), (Some("boot1"), Some(77)));
        assert_eq!(raw.sender(), "leader0001");
        assert!(raw.verify("secret"));
        // The sequence number is covered by the MAC.
        let text = String::from_utf8(bytes)
            .unwrap()
            .replace("\"sq\":77", "\"sq\":78");
        assert!(!parse(text.as_bytes()).unwrap().verify("secret"));
        // A MAC without boot/seq (old format) is not accepted.
        let mut legacy = serde_json::to_vec(&sample_sync()).unwrap();
        let mac = hmac_sha256(b"secret", &legacy);
        legacy.pop();
        legacy.extend_from_slice(
            format!(",\"mac\":\"{}\"}}", pixelplus_core::fseq::to_hex(&mac)).as_bytes(),
        );
        assert!(!parse(&legacy).unwrap().verify("secret"));
    }

    #[test]
    fn replay_guard() {
        let mut g = ReplayGuard::default();
        // A new boot needs confirmation.
        assert_eq!(g.check("l", "b1", 1, false), Freshness::UnknownBoot);
        assert_eq!(g.check("l", "b1", 1, true), Freshness::Fresh);
        assert_eq!(g.check("l", "b1", 2, false), Freshness::Fresh);
        assert_eq!(g.check("l", "b1", 2, false), Freshness::Replayed);
        assert_eq!(g.check("l", "b1", 1, false), Freshness::Replayed);
        // Reordering: 5 arrives before 3 and 4; both still count, once.
        assert_eq!(g.check("l", "b1", 5, false), Freshness::Fresh);
        assert_eq!(g.check("l", "b1", 4, false), Freshness::Fresh);
        assert_eq!(g.check("l", "b1", 3, false), Freshness::Fresh);
        assert_eq!(g.check("l", "b1", 4, false), Freshness::Replayed);
        assert_eq!(g.check("l", "b1", 5, false), Freshness::Replayed);
        // Too old for the window.
        assert_eq!(g.check("l", "b1", 200, false), Freshness::Fresh);
        assert_eq!(g.check("l", "b1", 100, false), Freshness::Replayed);
        assert_eq!(g.check("l", "b1", 199, false), Freshness::Fresh);
        // Restart: new boot accepted once confirmed; the old one is refused for good.
        assert_eq!(g.check("l", "b2", 1, true), Freshness::Fresh);
        assert_eq!(g.check("l", "b1", 99, true), Freshness::UnknownBoot);
        assert_eq!(g.boot("l"), Some("b2"));
        g.forget("l");
        assert_eq!(g.boot("l"), None);
    }

    #[test]
    fn protocol_ranges_negotiate() {
        assert_eq!(negotiate((2, 2), (2, 2)), Some(2));
        // A newer release that still speaks 2 talks 2 with an older node.
        assert_eq!(negotiate((2, 3), (2, 2)), Some(2));
        assert_eq!(negotiate((2, 3), (1, 3)), Some(3));
        assert_eq!(negotiate((3, 4), (1, 2)), None);
        let mut b: Beacon = serde_json::from_value(serde_json::json!({
            "id": "a", "name": "A", "role": "follower", "board": "difftx", "ver": "1", "http": 80
        }))
        .unwrap();
        assert_eq!(proto_range(&b), (1, 1), "absent = protocol 1");
        b.proto = 2;
        b.proto_max = Some(3);
        assert_eq!(proto_range(&b), (2, 3));
    }
}
