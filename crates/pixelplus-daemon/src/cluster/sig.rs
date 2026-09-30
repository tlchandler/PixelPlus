//! Cluster authentication (ARCHITECTURE §7.5).
//!
//! * **Keys.** Every follower has its own key, agreed at adoption with an
//!   ephemeral X25519 exchange ([`DhOffer`]), so the key never crosses the
//!   network. The leader keeps one key per follower (`node.json`
//!   `followerKeys`); a follower only knows its own. A follower key never
//!   authenticates the admin API, only the `/cluster/*` calls between that
//!   follower and its leader.
//! * **HTTP.** Cluster calls are signed, never carry the key:
//!   `X-PixelPlus-Auth: v1 <senderId> <unixTime> <nonce> <hmac>` where the
//!   HMAC-SHA256 covers method, path + query, sender, time, nonce and the
//!   SHA-256 of the body. The receiver rejects times more than
//!   [`MAX_SKEW_S`] away from its own clock and nonces it has already seen.
//!   Controllers without a real-time clock may disagree about the time: a
//!   request that is signed correctly but outside the window is answered
//!   `401` with `X-PixelPlus-Time: <unixTime> <hmac>` (MACed with the same key
//!   over the request nonce), and the sender retries once with the corrected
//!   offset.
//! * **Replies** that carry data a follower acts on (manifest, slice) are MACed
//!   too (`X-PixelPlus-Reply`), bound to the request nonce.

use super::proto::{ct_eq, hmac_sha256};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const AUTH_HEADER: &str = "x-pixelplus-auth";
pub const TIME_HEADER: &str = "x-pixelplus-time";
pub const REPLY_HEADER: &str = "x-pixelplus-reply";
/// Accepted difference between the sender's and our clock (seconds).
pub const MAX_SKEW_S: i64 = 30;
/// How long a nonce is remembered (covers the whole acceptance window).
const NONCE_TTL: Duration = Duration::from_secs(2 * MAX_SKEW_S as u64 + 10);
const NONCE_CAP: usize = 8192;

pub fn now_s() -> i64 {
    chrono::Utc::now().timestamp()
}

pub fn hex(b: &[u8]) -> String {
    pixelplus_core::fseq::to_hex(b)
}

pub fn sha256_hex(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

pub fn random_hex(bytes: usize) -> String {
    use rand::RngCore;
    let mut raw = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut raw);
    hex(&raw)
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

fn safe_token(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn request_mac(
    key: &str,
    method: &str,
    path: &str,
    sender: &str,
    ts: i64,
    nonce: &str,
    body: &[u8],
) -> [u8; 32] {
    let msg = format!(
        "pixelplus-req-v1\n{}\n{path}\n{sender}\n{ts}\n{nonce}\n{}",
        method.to_ascii_uppercase(),
        sha256_hex(body)
    );
    hmac_sha256(key.as_bytes(), msg.as_bytes())
}

/// Sign a request; returns `(header value, nonce)`.
pub fn sign(
    key: &str,
    sender: &str,
    method: &str,
    path: &str,
    body: &[u8],
    ts: i64,
) -> (String, String) {
    let nonce = random_hex(16);
    let mac = request_mac(key, method, path, sender, ts, &nonce, body);
    (format!("v1 {sender} {ts} {nonce} {}", hex(&mac)), nonce)
}

/// A parsed `X-PixelPlus-Auth` header (not yet verified).
#[derive(Debug, Clone, PartialEq)]
pub struct Signed {
    pub sender: String,
    pub ts: i64,
    pub nonce: String,
    mac: Vec<u8>,
}

pub fn parse(header: &str) -> Option<Signed> {
    let mut it = header.split_ascii_whitespace();
    if it.next()? != "v1" {
        return None;
    }
    let sender = it.next()?.to_string();
    let ts = it.next()?.parse().ok()?;
    let nonce = it.next()?.to_string();
    let mac = unhex(it.next()?)?;
    if it.next().is_some() || !safe_token(&sender, 64) || !safe_token(&nonce, 64) || mac.len() != 32
    {
        return None;
    }
    Some(Signed {
        sender,
        ts,
        nonce,
        mac,
    })
}

impl Signed {
    /// The MAC matches (time and nonce are checked separately).
    pub fn verify(&self, key: &str, method: &str, path: &str, body: &[u8]) -> bool {
        let mac = request_mac(key, method, path, &self.sender, self.ts, &self.nonce, body);
        ct_eq(&mac, &self.mac)
    }
}

/// Why a signed request was refused.
#[derive(Debug, PartialEq)]
pub enum Refusal {
    /// No or malformed header, unknown sender or wrong MAC.
    Unauthenticated,
    /// Correct MAC, but the time is outside the window; `now` is our clock.
    Skew { now: i64 },
    /// Nonce seen before.
    Replay,
}

/// Remembers recently used nonces (replay protection).
#[derive(Default)]
pub struct NonceCache {
    seen: Mutex<HashMap<String, Instant>>,
}

impl NonceCache {
    /// `true` the first time `(sender, nonce)` is presented.
    pub fn fresh(&self, sender: &str, nonce: &str) -> bool {
        let now = Instant::now();
        let mut seen = self.seen.lock();
        if seen.len() >= NONCE_CAP {
            seen.retain(|_, t| now.duration_since(*t) < NONCE_TTL);
            if seen.len() >= NONCE_CAP {
                // Flooded with valid signatures (only key holders can do this):
                // refuse rather than forget nonces inside the window.
                return false;
            }
        }
        let k = format!("{sender} {nonce}");
        if seen
            .get(&k)
            .is_some_and(|t| now.duration_since(*t) < NONCE_TTL)
        {
            return false;
        }
        seen.insert(k, now);
        true
    }
}

/// A refusal, plus (for [`Refusal::Skew`]) the key and nonce to answer with
/// our clock.
pub type CheckError = (Refusal, Option<(String, String)>);

/// Full check of a signed request: header, MAC with `key_for(sender)`, time
/// window and nonce. Returns the sender and the key that verified it.
pub fn check(
    header: Option<&str>,
    method: &str,
    path: &str,
    body: &[u8],
    nonces: &NonceCache,
    now: i64,
    key_for: impl Fn(&str) -> Option<String>,
) -> Result<(Signed, String), CheckError> {
    let Some(signed) = header.and_then(parse) else {
        return Err((Refusal::Unauthenticated, None));
    };
    let Some(key) = key_for(&signed.sender) else {
        return Err((Refusal::Unauthenticated, None));
    };
    if !signed.verify(&key, method, path, body) {
        return Err((Refusal::Unauthenticated, None));
    }
    if (signed.ts - now).abs() > MAX_SKEW_S {
        return Err((Refusal::Skew { now }, Some((key, signed.nonce))));
    }
    if !nonces.fresh(&signed.sender, &signed.nonce) {
        return Err((Refusal::Replay, None));
    }
    Ok((signed, key))
}

fn time_mac(key: &str, now: i64, nonce: &str) -> [u8; 32] {
    hmac_sha256(
        key.as_bytes(),
        format!("pixelplus-time-v1\n{now}\n{nonce}").as_bytes(),
    )
}

/// `X-PixelPlus-Time` value telling a key holder our clock.
pub fn time_proof(key: &str, now: i64, nonce: &str) -> String {
    format!("{now} {}", hex(&time_mac(key, now, nonce)))
}

/// Our peer's clock from a `X-PixelPlus-Time` answer to our request `nonce`.
pub fn verify_time_proof(key: &str, header: &str, nonce: &str) -> Option<i64> {
    let (now, mac) = header.trim().split_once(' ')?;
    let now: i64 = now.parse().ok()?;
    ct_eq(&time_mac(key, now, nonce), &unhex(mac)?).then_some(now)
}

/// MAC for a reply to the request with `nonce`, covering `what` (e.g. the
/// body hash, or the slice ETag and checksum).
pub fn reply_mac(key: &str, nonce: &str, what: &str) -> String {
    hex(&hmac_sha256(
        key.as_bytes(),
        format!("pixelplus-reply-v1\n{nonce}\n{what}").as_bytes(),
    ))
}

pub fn verify_reply(key: &str, nonce: &str, what: &str, header: Option<&str>) -> bool {
    header.is_some_and(|h| ct_eq(reply_mac(key, nonce, what).as_bytes(), h.trim().as_bytes()))
}

// ---------------------------------------------------------------------------
// Key agreement (adoption)
// ---------------------------------------------------------------------------

/// Our half of an X25519 exchange.
pub struct DhOffer {
    private: ring::agreement::EphemeralPrivateKey,
    pub public_hex: String,
}

pub fn dh_offer() -> anyhow::Result<DhOffer> {
    use ring::agreement;
    let rng = ring::rand::SystemRandom::new();
    let private = agreement::EphemeralPrivateKey::generate(&agreement::X25519, &rng)
        .map_err(|_| anyhow::anyhow!("no randomness for a key exchange"))?;
    let public = private
        .compute_public_key()
        .map_err(|_| anyhow::anyhow!("key exchange failed"))?;
    Ok(DhOffer {
        private,
        public_hex: hex(public.as_ref()),
    })
}

/// Valid X25519 public key (hex).
pub fn valid_public(hex_key: &str) -> bool {
    hex_key.len() == 64 && unhex(hex_key).is_some()
}

/// The follower key both sides derive from the exchange.
pub fn derive_key(
    offer: DhOffer,
    peer_public_hex: &str,
    leader_id: &str,
    follower_id: &str,
    leader_public_hex: &str,
    follower_public_hex: &str,
) -> Option<String> {
    use ring::agreement;
    let peer = unhex(peer_public_hex).filter(|p| p.len() == 32)?;
    let peer = agreement::UnparsedPublicKey::new(&agreement::X25519, peer);
    agreement::agree_ephemeral(offer.private, &peer, |shared| {
        let info = format!(
            "pixelplus-follower-key-v1\n{leader_id}\n{follower_id}\n{leader_public_hex}\n{follower_public_hex}"
        );
        hex(&hmac_sha256(shared, info.as_bytes()))
    })
    .ok()
}

/// The follower's proof (in its adopt reply) that it derived the same key.
pub fn adopt_proof(key: &str, leader_id: &str, follower_id: &str) -> String {
    hex(&hmac_sha256(
        key.as_bytes(),
        format!("pixelplus-adopted-v1\n{leader_id}\n{follower_id}").as_bytes(),
    ))
}

// ---------------------------------------------------------------------------
// Signed calls (client side)
// ---------------------------------------------------------------------------

/// Response of a signed call, with the nonce it was signed with (replies are
/// bound to it).
pub(crate) struct SignedResponse {
    pub resp: reqwest::Response,
    pub nonce: String,
}

#[derive(Debug)]
pub(crate) enum CallError {
    Http(reqwest::Error),
    BadUrl(String),
}

impl CallError {
    /// Short human-readable reason.
    pub fn short(&self) -> String {
        match self {
            CallError::Http(e) if e.is_timeout() => "timed out".into(),
            CallError::Http(e) if e.is_connect() => "connection refused".into(),
            CallError::Http(e) => e.to_string(),
            CallError::BadUrl(u) => format!("invalid address {u}"),
        }
    }
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.short())
    }
}

/// Send a request signed with `key` as `sender` to `peer_id` at `url`.
/// Retries once when the peer answers with a (signed) clock correction.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn call(
    sh: &super::Shared,
    key: &str,
    sender: &str,
    peer_id: &str,
    method: reqwest::Method,
    url: &str,
    body: Option<Vec<u8>>,
    timeout: Duration,
    extra: &[(&str, String)],
) -> Result<SignedResponse, CallError> {
    let parsed = reqwest::Url::parse(url).map_err(|_| CallError::BadUrl(url.to_string()))?;
    let path = match parsed.query() {
        Some(q) => format!("{}?{q}", parsed.path()),
        None => parsed.path().to_string(),
    };
    let payload = body.clone().unwrap_or_default();
    let mut corrected = false;
    loop {
        let skew = sh.skew.lock().get(peer_id).copied().unwrap_or(0);
        let (auth, nonce) = sign(
            key,
            sender,
            method.as_str(),
            &path,
            &payload,
            now_s() + skew,
        );
        let mut req = sh
            .http
            .request(method.clone(), parsed.clone())
            .header(AUTH_HEADER, auth)
            .header("x-pixelplus-request", "1")
            .timeout(timeout);
        for (k, v) in extra {
            req = req.header(*k, v);
        }
        if let Some(b) = &body {
            req = req
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(b.clone());
        }
        let resp = req.send().await.map_err(CallError::Http)?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED && !corrected {
            let theirs = resp
                .headers()
                .get(TIME_HEADER)
                .and_then(|v| v.to_str().ok())
                .and_then(|h| verify_time_proof(key, h, &nonce));
            if let Some(theirs) = theirs {
                let offset = theirs - now_s();
                tracing::info!(
                    "clock of {peer_id} differs by {offset} s; adjusting cluster signatures"
                );
                sh.skew.lock().insert(peer_id.to_string(), offset);
                corrected = true;
                continue;
            }
        }
        return Ok(SignedResponse { resp, nonce });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify() {
        let (h, nonce) = sign(
            "k1",
            "leader1",
            "post",
            "/api/v1/cluster/command",
            b"{}",
            1000,
        );
        let s = parse(&h).unwrap();
        assert_eq!(
            (s.sender.as_str(), s.ts, s.nonce.as_str()),
            ("leader1", 1000, nonce.as_str())
        );
        assert!(s.verify("k1", "POST", "/api/v1/cluster/command", b"{}"));
        // Anything changed breaks the MAC.
        assert!(!s.verify("k2", "POST", "/api/v1/cluster/command", b"{}"));
        assert!(!s.verify("k1", "GET", "/api/v1/cluster/command", b"{}"));
        assert!(!s.verify("k1", "POST", "/api/v1/cluster/release", b"{}"));
        assert!(!s.verify("k1", "POST", "/api/v1/cluster/command", b"{\"a\":1}"));
        let forged = h.replace("leader1", "leader2");
        assert!(!parse(&forged)
            .unwrap()
            .verify("k1", "POST", "/api/v1/cluster/command", b"{}"));
        assert!(parse("v2 a 1 n ff").is_none());
        assert!(parse("v1 ../x 1 n 00").is_none());
        assert!(parse(&format!("{h} extra")).is_none());
    }

    #[test]
    fn check_enforces_window_and_nonces() {
        let nonces = NonceCache::default();
        let keys = |s: &str| (s == "f1").then(|| "key".to_string());
        let (h, _) = sign("key", "f1", "GET", "/p", b"", 5000);
        assert!(check(Some(&h), "GET", "/p", b"", &nonces, 5010, keys).is_ok());
        // Replay.
        assert_eq!(
            check(Some(&h), "GET", "/p", b"", &nonces, 5010, keys)
                .unwrap_err()
                .0,
            Refusal::Replay
        );
        // Old / future.
        let (h, nonce) = sign("key", "f1", "GET", "/p", b"", 5000);
        let err = check(
            Some(&h),
            "GET",
            "/p",
            b"",
            &nonces,
            5000 + MAX_SKEW_S + 1,
            keys,
        )
        .unwrap_err();
        assert_eq!(err.0, Refusal::Skew { now: 5031 });
        assert_eq!(err.1, Some(("key".to_string(), nonce)));
        // Unknown sender, missing header, bad MAC.
        let (h, _) = sign("key", "f2", "GET", "/p", b"", 5000);
        assert_eq!(
            check(Some(&h), "GET", "/p", b"", &nonces, 5000, keys)
                .unwrap_err()
                .0,
            Refusal::Unauthenticated
        );
        assert_eq!(
            check(None, "GET", "/p", b"", &nonces, 5000, keys)
                .unwrap_err()
                .0,
            Refusal::Unauthenticated
        );
        let (h, _) = sign("nope", "f1", "GET", "/p", b"", 5000);
        let err = check(Some(&h), "GET", "/p", b"", &nonces, 9999, keys).unwrap_err();
        assert_eq!(
            err,
            (Refusal::Unauthenticated, None),
            "no clock hint without the key"
        );
    }

    #[test]
    fn time_and_reply_proofs() {
        let p = time_proof("k", 1234, "n1");
        assert_eq!(verify_time_proof("k", &p, "n1"), Some(1234));
        assert_eq!(verify_time_proof("k", &p, "n2"), None);
        assert_eq!(verify_time_proof("x", &p, "n1"), None);
        let r = reply_mac("k", "n1", "abc");
        assert!(verify_reply("k", "n1", "abc", Some(&r)));
        assert!(!verify_reply("k", "n1", "abd", Some(&r)));
        assert!(!verify_reply("k", "n1", "abc", None));
    }

    #[test]
    fn key_agreement() {
        let leader = dh_offer().unwrap();
        let follower = dh_offer().unwrap();
        let (lp, fp) = (leader.public_hex.clone(), follower.public_hex.clone());
        assert!(valid_public(&lp) && !valid_public("zz"));
        let k1 = derive_key(leader, &fp, "L", "F", &lp, &fp).unwrap();
        let k2 = derive_key(follower, &lp, "L", "F", &lp, &fp).unwrap();
        assert_eq!(k1, k2);
        assert_eq!(k1.len(), 64);
        let other = dh_offer().unwrap();
        assert!(derive_key(other, "00", "L", "F", &lp, &fp).is_none());
        assert_eq!(adopt_proof(&k1, "L", "F"), adopt_proof(&k2, "L", "F"));
    }
}
