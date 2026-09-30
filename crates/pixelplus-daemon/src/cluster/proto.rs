//! Cluster wire formats (ARCHITECTURE §7, §10).
//!
//! **UDP cluster port** (default 32320): one JSON object per datagram, tagged by
//! `"t"`: `beacon`, `sync`, `ping`, `pong`.
//!
//! **UDP overlay port** (cluster port + 1, default 32321): binary overlay frames
//! `u8 'O' | u8 idLen | propId | u32 frameNo (LE) | RGB… | 32-byte MAC`.
//!
//! ## Authentication
//! Once a node belongs to a cluster it shares the `clusterKey` with its leader.
//! Every packet a cluster member sends is authenticated with HMAC-SHA256 keyed
//! by that key:
//!
//! * JSON packets get a trailing `,"mac":"<64 hex>"}` member appended to the
//!   serialized object; the MAC covers the object *without* that member (i.e.
//!   the exact bytes that precede it, plus the closing `}`). The packet stays
//!   ordinary JSON, so unauthenticated readers (e.g. `tcpdump`) still work.
//! * Overlay frames carry the raw 32-byte MAC of everything before it.
//!
//! Unadopted nodes send unauthenticated beacons (that is how discovery works).
//! Anything that can change what a node *does* (sync, pong, the leader's
//! address, overlay pixels, follower status reports) is only accepted with a
//! valid MAC.

use crate::node::LocalRole;
use crate::player::SyncPacket;
use pixelplus_core::model::BoardKind;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::net::IpAddr;

/// Largest JSON datagram we send or accept.
pub const MAX_JSON_PACKET: usize = 32 * 1024;
/// Largest overlay datagram (the IPv4 UDP payload limit).
pub const MAX_OVERLAY_PACKET: usize = 65_507;
/// Overlay frame type byte.
pub const OVERLAY_MAGIC: u8 = b'O';

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
    /// Leader clock when the ping was received (ms since its daemon start).
    pub t1: f64,
    /// Leader boot id (a change resets the follower's clock filter).
    #[serde(default)]
    pub boot: String,
}

/// Every JSON datagram on the cluster port.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Msg {
    Beacon(Beacon),
    Sync(SyncPacket),
    Ping(Ping),
    Pong(Pong),
}

/// A decoded datagram and whether it carried a valid MAC for our key.
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
    let inner = Sha256::new().chain_update(ipad).chain_update(msg).finalize();
    Sha256::new().chain_update(opad).chain_update(inner).finalize().into()
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

/// Serialize `msg`, appending a MAC when `key` is set.
pub fn encode(msg: &Msg, key: Option<&str>) -> Vec<u8> {
    let mut json = serde_json::to_vec(msg).expect("cluster messages always serialize");
    if let Some(key) = key.filter(|k| !k.is_empty()) {
        let mac = hmac_sha256(key.as_bytes(), &json);
        json.pop(); // closing '}'
        json.extend_from_slice(b",\"mac\":\"");
        json.extend_from_slice(pixelplus_core::fseq::to_hex(&mac).as_bytes());
        json.extend_from_slice(b"\"}");
    }
    json
}

/// Parse a datagram and check its MAC against `key`.
pub fn decode(bytes: &[u8], key: Option<&str>) -> Result<Decoded<Msg>, ProtoError> {
    if bytes.len() > MAX_JSON_PACKET {
        return Err(ProtoError::TooLarge(bytes.len()));
    }
    let msg: Msg =
        serde_json::from_slice(bytes).map_err(|e| ProtoError::Malformed(e.to_string()))?;
    let authenticated = match key.filter(|k| !k.is_empty()) {
        Some(key) => verify_json_mac(bytes, key),
        None => false,
    };
    Ok(Decoded { msg, authenticated })
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
// Overlay frames
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayFrame {
    pub prop_id: String,
    pub frame_no: u32,
    pub rgb: Vec<u8>,
}

/// Encode an overlay frame (always authenticated).
pub fn encode_overlay(prop_id: &str, frame_no: u32, rgb: &[u8], key: &str) -> Result<Vec<u8>, ProtoError> {
    let id = prop_id.as_bytes();
    if id.is_empty() || id.len() > 255 {
        return Err(ProtoError::Malformed("prop id must be 1..255 bytes".into()));
    }
    let len = 2 + id.len() + 4 + rgb.len() + MAC_LEN;
    if len > MAX_OVERLAY_PACKET {
        return Err(ProtoError::TooLarge(len));
    }
    let mut out = Vec::with_capacity(len);
    out.push(OVERLAY_MAGIC);
    out.push(id.len() as u8);
    out.extend_from_slice(id);
    out.extend_from_slice(&frame_no.to_le_bytes());
    out.extend_from_slice(rgb);
    let mac = hmac_sha256(key.as_bytes(), &out);
    out.extend_from_slice(&mac);
    Ok(out)
}

/// Decode an overlay frame; `None` unless well-formed *and* authenticated.
pub fn decode_overlay(bytes: &[u8], key: &str) -> Option<OverlayFrame> {
    if bytes.len() > MAX_OVERLAY_PACKET || bytes.len() < 2 + 1 + 4 + MAC_LEN || bytes[0] != OVERLAY_MAGIC {
        return None;
    }
    let (body, mac) = bytes.split_at(bytes.len() - MAC_LEN);
    if !ct_eq(&hmac_sha256(key.as_bytes(), body), mac) {
        return None;
    }
    let id_len = body[1] as usize;
    if body.len() < 2 + id_len + 4 {
        return None;
    }
    let prop_id = std::str::from_utf8(&body[2..2 + id_len]).ok()?.to_string();
    let n = 2 + id_len;
    let frame_no = u32::from_le_bytes(body[n..n + 4].try_into().ok()?);
    let rgb = body[n + 4..].to_vec();
    if rgb.len() % 3 != 0 {
        return None;
    }
    Some(OverlayFrame { prop_id, frame_no, rgb })
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
        let mac = hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First");
        assert_eq!(
            pixelplus_core::fseq::to_hex(&mac),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    fn sample_sync() -> Msg {
        Msg::Sync(SyncPacket {
            leader: "leader0001".into(),
            show_version: 7,
            state: PlayerState::Playing,
            item: None,
            pos_ms: 1234,
            sent_at_ms: 99_000,
            effect: None,
            test: None,
            brightness: 80,
            blackout: false,
        })
    }

    #[test]
    fn json_roundtrip_with_mac() {
        let msg = sample_sync();
        let bytes = encode(&msg, Some("secret"));
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
        let bytes = encode(&sample_sync(), Some("secret"));
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
            id: "abc".into(),
            name: "Garage".into(),
            hostname: "pixelplus-garage".into(),
            role: LocalRole::Follower,
            board: BoardKind::Difftx,
            board_rev: Some("E".into()),
            pi: Some("Raspberry Pi 4".into()),
            ver: "0.1.0".into(),
            http: 80,
            overlay: 32321,
            adopted_by: None,
            ips: vec!["10.0.0.5".parse().unwrap()],
            boot: "b".into(),
            show_version: 0,
            report: None,
        });
        let v: serde_json::Value = serde_json::from_slice(&encode(&b, None)).unwrap();
        assert_eq!(v["t"], "beacon");
        assert_eq!(v["board"], "difftx");
        assert_eq!(v["boardRev"], "E");
        assert_eq!(v["pi"], "Raspberry Pi 4");
        assert_eq!(v["adoptedBy"], serde_json::Value::Null);
        assert_eq!(v["http"], 80);
        assert_eq!(v["role"], "follower");
    }

    #[test]
    fn overlay_roundtrip() {
        let rgb: Vec<u8> = (0..30u8).collect();
        let bytes = encode_overlay("prop1", 42, &rgb, "k").unwrap();
        assert_eq!(bytes[0], b'O');
        let f = decode_overlay(&bytes, "k").unwrap();
        assert_eq!(f, OverlayFrame { prop_id: "prop1".into(), frame_no: 42, rgb });
        assert!(decode_overlay(&bytes, "wrong").is_none());
        let mut bad = bytes.clone();
        bad[10] ^= 1;
        assert!(decode_overlay(&bad, "k").is_none());
        assert!(encode_overlay("p", 0, &vec![0; MAX_OVERLAY_PACKET], "k").is_err());
    }
}
