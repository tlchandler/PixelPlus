//! Secret trigger links (ARCHITECTURE §12.18): `POST /api/v1/hooks/trigger/:id`
//! with a per-trigger token, for Home Assistant, doorbells, Stream Decks and
//! anything else that can't sign in to the web UI.
//!
//! * **Tokens**: `ppt_` + 43 base64url characters (256 random bits). Only the
//!   SHA-256 is stored (`Trigger.tokenHash`, never sent to browsers); the
//!   check compares hashes in constant time ([`verify`]).
//! * **Where from**: home network by default; through the public listener /
//!   a tunnel only when the owner allowed that trigger from the internet
//!   (`api::hooks` decides, [`Origin`]).
//! * **Limits**: wrong tokens back off per address like sign-in
//!   (`Sessions::hook_throttle`); good calls are capped per trigger and
//!   address ([`HookState::admit`], 10 a minute) on top of the trigger's own
//!   gates (when / window / cooldown / max per hour).
//! * **Book-keeping** (`<data>/trigger-links.json`): when each link last ran,
//!   from where and with what result, and the addresses it was used from
//!   (a new internet address raises an alert; every use is journaled).

use crate::state::AppState;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// Every token starts with this (easy to spot in configs and secret scanners).
pub const TOKEN_PREFIX: &str = "ppt_";
/// Random bytes per token (256 bits).
const TOKEN_BYTES: usize = 32;
/// Good calls allowed per trigger and address in [`RATE_WINDOW`].
pub const PER_ADDRESS_PER_MINUTE: usize = 10;
/// Good calls allowed per trigger from everyone together in [`RATE_WINDOW`].
pub const PER_TRIGGER_PER_MINUTE: usize = 30;
const RATE_WINDOW: Duration = Duration::from_secs(60);
/// Addresses remembered per trigger (oldest forgotten first).
const KNOWN_MAX: usize = 32;

/// base64url without padding.
fn base64url(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(T[(n >> 6) as usize & 63] as char);
        }
        if chunk.len() > 2 {
            out.push(T[n as usize & 63] as char);
        }
    }
    out
}

/// A fresh token (shown to the owner once).
pub fn new_token() -> String {
    use rand::RngCore;
    let mut raw = [0u8; TOKEN_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    format!("{TOKEN_PREFIX}{}", base64url(&raw))
}

/// What is stored: lower-case hex SHA-256 of the whole token.
pub fn hash_token(token: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The last 4 characters (shown in the UI to tell links apart).
pub fn hint(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    chars[chars.len().saturating_sub(4)..].iter().collect()
}

/// Does `token` match `stored` (a [`hash_token`])? Constant time in the
/// token's content.
pub fn verify(stored: &str, token: &str) -> bool {
    if token.is_empty() || token.len() > 256 || stored.len() != 64 {
        return false;
    }
    crate::api::auth::constant_time_eq(hash_token(token).as_bytes(), stored.as_bytes())
}

/// The token of a request: `Authorization: Bearer <token>` (preferred), else
/// the `token` query parameter.
pub fn token_from(headers: &axum::http::HeaderMap, query: Option<&str>) -> Option<String> {
    if let Some(v) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        if let Some((scheme, rest)) = v.trim().split_once(' ') {
            if scheme.eq_ignore_ascii_case("bearer") && !rest.trim().is_empty() {
                return Some(rest.trim().to_string());
            }
        }
    }
    query?
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == "token")
        .map(|(_, v)| percent_decode(v))
        .filter(|v| !v.is_empty())
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16)
                {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Where a link call came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Origin {
    /// This controller's network (no proxy in between).
    Home,
    /// The public listener, a tunnel or reverse proxy, or a public address.
    Internet,
}

/// What the UI shows under a link: its last use.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LinkUse {
    /// RFC 3339.
    pub at: String,
    /// The caller's address.
    pub from: String,
    pub origin: Option<Origin>,
    /// It fired (false: a gate or the action said no).
    pub fired: bool,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LinkBook {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last: Option<LinkUse>,
    /// Addresses seen (most recent last).
    #[serde(default)]
    known: Vec<String>,
}

/// (trigger id, caller address).
type CallKey = (String, Option<IpAddr>);

/// Runtime state of the links (`Services::hooks`).
#[derive(Default)]
pub struct HookState {
    book: Mutex<Option<BTreeMap<String, LinkBook>>>,
    /// Good calls per (trigger, address) and per trigger.
    hits: Mutex<HashMap<CallKey, VecDeque<Instant>>>,
    per_trigger: Mutex<HashMap<String, VecDeque<Instant>>>,
    write: tokio::sync::Mutex<()>,
}

fn book_path(state: &AppState) -> std::path::PathBuf {
    state.config.data_dir.join("trigger-links.json")
}

fn prune(q: &mut VecDeque<Instant>, now: Instant) {
    while q
        .front()
        .is_some_and(|t| now.duration_since(*t) >= RATE_WINDOW)
    {
        q.pop_front();
    }
}

impl HookState {
    fn with_book<R>(
        &self,
        state: &AppState,
        f: impl FnOnce(&mut BTreeMap<String, LinkBook>) -> R,
    ) -> R {
        let mut g = self.book.lock();
        let book = g.get_or_insert_with(|| {
            std::fs::read(book_path(state))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default()
        });
        f(book)
    }

    async fn save(&self, state: &AppState) {
        let _w = self.write.lock().await;
        let json = self.with_book(state, |b| serde_json::to_vec_pretty(b).unwrap_or_default());
        let path = book_path(state);
        let tmp = path.with_extension("json.tmp");
        let res = async {
            tokio::fs::write(&tmp, &json).await?;
            tokio::fs::rename(&tmp, &path).await
        }
        .await;
        if let Err(e) = res {
            tracing::warn!("Couldn't save {}: {e}", path.display());
        }
    }

    /// Room for one more good call of `trigger` from `ip`? `Err(wait)`: the
    /// caller has to wait that long (10 a minute per address, 30 per trigger).
    pub fn admit(&self, trigger: &str, ip: Option<IpAddr>, now: Instant) -> Result<(), Duration> {
        let mut hits = self.hits.lock();
        if hits.len() > 4096 {
            hits.retain(|_, q| {
                q.back()
                    .is_some_and(|t| now.duration_since(*t) < RATE_WINDOW)
            });
        }
        let mine = hits.entry((trigger.to_string(), ip)).or_default();
        prune(mine, now);
        let mut all = self.per_trigger.lock();
        let every = all.entry(trigger.to_string()).or_default();
        prune(every, now);
        for (q, cap) in [
            (&*mine, PER_ADDRESS_PER_MINUTE),
            (&*every, PER_TRIGGER_PER_MINUTE),
        ] {
            if q.len() >= cap {
                let oldest = *q.front().unwrap_or(&now);
                return Err(RATE_WINDOW.saturating_sub(now.duration_since(oldest)));
            }
        }
        mine.push_back(now);
        every.push_back(now);
        Ok(())
    }

    /// Record a call that got past the token check. Returns true when `from`
    /// had never used this link before.
    pub async fn record(&self, state: &AppState, trigger: &str, used: LinkUse) -> bool {
        let new = self.with_book(state, |b| {
            let e = b.entry(trigger.to_string()).or_default();
            let new = !e.known.contains(&used.from);
            e.known.retain(|k| *k != used.from);
            e.known.push(used.from.clone());
            if e.known.len() > KNOWN_MAX {
                e.known.remove(0);
            }
            e.last = Some(used);
            new
        });
        self.save(state).await;
        new
    }

    /// Forget a trigger's history (its token was rotated or revoked: the
    /// addresses that knew the old one are strangers again).
    pub async fn forget(&self, state: &AppState, trigger: &str) {
        let changed = self.with_book(state, |b| b.remove(trigger).is_some());
        self.hits.lock().retain(|(t, _), _| t != trigger);
        self.per_trigger.lock().remove(trigger);
        if changed {
            self.save(state).await;
        }
    }

    /// Last use of every link (for `GET /triggers/links`).
    pub fn last_uses(&self, state: &AppState) -> BTreeMap<String, LinkUse> {
        self.with_book(state, |b| {
            b.iter()
                .filter_map(|(k, v)| v.last.clone().map(|l| (k.clone(), l)))
                .collect()
        })
    }
}

/// A link was used from an internet address it had never seen: tell the owner.
pub fn new_internet_address(state: &AppState, trigger_name: &str, from: &str) {
    let key = format!("trigger-link:{trigger_name}:{from}");
    let body = format!(
        "The link of the trigger “{trigger_name}” was used from {from} on the internet for the first time. \
         If you don't recognise this, open Settings → Triggers and rotate or revoke the link."
    );
    let state = state.clone();
    tokio::spawn(async move {
        crate::services::alerts::raise(
            &state,
            &key,
            crate::services::alerts::Severity::Warning,
            "Trigger link used from a new address",
            &body,
        )
        .await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    #[test]
    fn tokens_are_random_hashed_and_checked() {
        let a = new_token();
        let b = new_token();
        assert_ne!(a, b);
        assert!(a.starts_with(TOKEN_PREFIX));
        assert_eq!(a.len(), TOKEN_PREFIX.len() + 43, "256 bits in base64url");
        assert!(a[4..]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        let h = hash_token(&a);
        assert_eq!(h.len(), 64);
        assert!(!h.contains(&a[4..]), "the hash doesn't contain the token");
        assert!(verify(&h, &a));
        assert!(!verify(&h, &b));
        assert!(!verify(&h, ""));
        assert!(!verify(&h, &a[..a.len() - 1]));
        assert!(!verify("", &a), "no stored hash: nothing matches");
        assert!(!verify(&h, &"x".repeat(10_000)));
        assert_eq!(hint("ppt_abcdWXYZ"), "WXYZ");
        assert_eq!(hint("ab"), "ab");
        // Known vector: SHA-256("abc").
        assert_eq!(
            hash_token("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn token_comes_from_the_header_or_the_query() {
        let mut h = HeaderMap::new();
        assert_eq!(token_from(&h, None), None);
        assert_eq!(
            token_from(&h, Some("a=1&token=ppt_x%2Dy&b=2")).as_deref(),
            Some("ppt_x-y")
        );
        assert_eq!(token_from(&h, Some("token=")), None);
        h.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer ppt_header"),
        );
        assert_eq!(
            token_from(&h, Some("token=ppt_query")).as_deref(),
            Some("ppt_header"),
            "the header wins"
        );
        h.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Basic Zm9vOmJhcg=="),
        );
        assert_eq!(token_from(&h, None), None);
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn calls_are_capped_per_address_and_per_trigger() {
        let s = HookState::default();
        let t0 = Instant::now();
        let a: Option<IpAddr> = Some("192.168.1.5".parse().unwrap());
        for _ in 0..PER_ADDRESS_PER_MINUTE {
            assert!(s.admit("t", a, t0).is_ok());
        }
        let wait = s.admit("t", a, t0).unwrap_err();
        assert!(wait <= RATE_WINDOW && wait > Duration::from_secs(50));
        assert!(s.admit("other", a, t0).is_ok(), "per trigger");
        assert!(s.admit("t", a, t0 + RATE_WINDOW).is_ok(), "a minute later");
        // Everyone together.
        let s = HookState::default();
        for i in 0..PER_TRIGGER_PER_MINUTE {
            let ip = Some(IpAddr::from([10, 0, 0, i as u8]));
            assert!(s.admit("t", ip, t0).is_ok());
        }
        assert!(s.admit("t", Some(IpAddr::from([10, 0, 1, 1])), t0).is_err());
    }
}
