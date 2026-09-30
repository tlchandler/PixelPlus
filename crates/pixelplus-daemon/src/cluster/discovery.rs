//! Discovery: UDP sockets, the receive loops, beacons and mDNS.

use super::proto::{self, Beacon, Freshness, Msg};
use super::{follower, leader, net, sleep_or_stop, Peer, Shared};
use crate::node::LocalRole;
use crate::state::AppState;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;

/// Peers not heard from for this long are forgotten.
const PEER_EXPIRY: Duration = Duration::from_secs(300);
/// A peer entry resists beacons of the same id from another address (and an
/// authenticated entry resists unauthenticated beacons) until it has been
/// silent this long.
const ENTRY_STICKY: Duration = Duration::from_secs(30);
/// How long a "possible duplicate" warning stays up.
const DUPLICATE_HOLD: Duration = Duration::from_secs(60);

pub(crate) fn spawn(state: &AppState, sh: &Arc<Shared>) {
    tokio::spawn(bind_and_receive(state.clone(), sh.clone(), false));
    tokio::spawn(bind_and_receive(state.clone(), sh.clone(), true));
    tokio::spawn(beacon_loop(state.clone(), sh.clone()));
    if sh.settings.mdns {
        tokio::spawn(mdns_loop(state.clone(), sh.clone()));
    }
}

async fn bind(sh: &Shared, port: u16, overlay: bool) -> std::io::Result<UdpSocket> {
    let sock = UdpSocket::bind(SocketAddr::new(sh.settings.bind, port)).await?;
    sock.set_broadcast(true)?;
    // WMM priority for timing packets (clock probes, sync), a notch lower for
    // overlay frames.
    let tos = if overlay { net::TOS_AF41 } else { net::TOS_EF };
    if let Err(e) = net::set_priority(&sock, tos) {
        tracing::debug!("could not set the priority of UDP {port}: {e}");
    }
    if !overlay {
        let on = net::enable_rx_timestamps(&sock);
        sh.kernel_ts.store(on, std::sync::atomic::Ordering::Relaxed);
        if !on {
            tracing::info!(
                "kernel receive timestamps unavailable; clock probes use userspace time"
            );
        }
    }
    Ok(sock)
}

/// Bind (retrying every 5 s while the port is busy), then receive forever.
async fn bind_and_receive(state: AppState, sh: Arc<Shared>, overlay: bool) {
    let mut stop = sh.stop_rx();
    let port = if overlay {
        sh.settings.overlay_port
    } else {
        sh.settings.port
    };
    let what = if overlay { "overlay" } else { "cluster" };
    let mut warned = false;
    let sock = loop {
        match bind(&sh, port, overlay).await {
            Ok(s) => break Arc::new(s),
            Err(e) => {
                if !warned {
                    super::log_warning(
                        &state,
                        format!("Cannot open the {what} port UDP {port} ({e}); controllers cannot talk to each other until it is free. Retrying…"),
                    );
                    warned = true;
                }
                if sleep_or_stop(&mut stop, Duration::from_secs(5)).await {
                    return;
                }
            }
        }
    };
    if warned {
        tracing::info!("{what} port UDP {port} is open now");
    }
    let slot = if overlay {
        &sh.overlay_socket
    } else {
        &sh.socket
    };
    let _ = slot.set(sock.clone());

    let mut buf = vec![0u8; 65_536];
    loop {
        let (n, src, stamp) = tokio::select! {
            r = net::recv_timestamped(&sock, &mut buf) => match r {
                Ok(v) => v,
                Err(e) => {
                    // ICMP errors (e.g. port unreachable after a send) surface here on
                    // some platforms; they are harmless. Never spin on a broken socket.
                    tracing::trace!("{what} recv: {e}");
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    continue;
                }
            },
            _ = stop.changed() => return,
        };
        // Arrival time on the cluster clock (kernel stamp where available).
        let rx_ms = sh.rx_time_ms(stamp);
        let data = &buf[..n];
        if overlay {
            follower::on_overlay_packet(&state, &sh, data).await;
        } else {
            on_packet(&state, &sh, data, src, rx_ms).await;
        }
    }
}

async fn on_packet(state: &AppState, sh: &Arc<Shared>, data: &[u8], src: SocketAddr, rx_ms: f64) {
    let identity = state.identity();
    let raw = match proto::parse(data) {
        Ok(r) => r,
        Err(e) => {
            tracing::trace!("ignoring packet from {src}: {e}");
            return;
        }
    };
    let sender = raw.sender().to_string();
    if sender == identity.id {
        return; // our own broadcast
    }
    // The key we share with the sender, if any: a leader has one per adopted
    // follower, a follower only the one shared with its leader.
    let key: Option<String> = match identity.role {
        LocalRole::Leader => sh.follower_key(state, &sender),
        LocalRole::Follower if identity.leader_id.as_deref() == Some(sender.as_str()) => {
            identity.cluster_key.clone()
        }
        _ => None,
    };
    let mac_ok = key.as_deref().is_some_and(|k| raw.verify(k));
    let fresh = if mac_ok {
        let boot = raw.boot.clone().unwrap_or_default();
        let seq = raw.seq.unwrap_or_default();
        let new_boot_ok = match (identity.role, &raw.msg) {
            // A follower accepts a new leader run only through a pong answering
            // one of its own recent pings (a replayed packet can't do that).
            (LocalRole::Follower, Msg::Pong(p)) => follower::answers_recent_ping(sh, p.t0),
            (LocalRole::Follower, _) => false,
            // A leader accepts a follower restart unless it is an older run.
            _ => true,
        };
        let verdict = sh.replay.lock().check(&sender, &boot, seq, new_boot_ok);
        match verdict {
            Freshness::Fresh => true,
            Freshness::Replayed => {
                tracing::trace!("dropping replayed packet from {sender} ({src})");
                return;
            }
            Freshness::UnknownBoot => {
                if identity.role == LocalRole::Follower {
                    // The leader restarted (or moved): confirm with a ping.
                    follower::challenge(state, sh, src).await;
                }
                return;
            }
        }
    } else {
        false
    };
    match raw.msg {
        Msg::Beacon(b) => on_beacon(state, sh, b, src, fresh),
        Msg::Sync(p) => {
            if fresh {
                follower::on_sync(state, sh, p, src).await;
            }
        }
        Msg::Ping(p) => {
            if let (true, Some(key)) = (fresh, key.as_deref()) {
                leader::on_ping(state, sh, p, key, src, rx_ms).await;
            }
        }
        Msg::Pong(p) => {
            if fresh {
                follower::on_pong(state, sh, p, src, rx_ms);
            }
        }
    }
}

/// Is `new` (from `src`) plausibly the same device as the entry `old`?
fn same_device(old: &Peer, new: &Beacon, src: SocketAddr) -> bool {
    old.addr.ip() == src.ip()
        || new.ips.contains(&old.addr.ip())
        || old.beacon.ips.contains(&src.ip())
}

fn on_beacon(state: &AppState, sh: &Arc<Shared>, b: Beacon, src: SocketAddr, authenticated: bool) {
    let now = Instant::now();
    // Released nodes legitimately switch to unauthenticated beacons.
    let show = state.store.get();
    let node = show.node(&b.id).filter(|n| n.adopted);
    let member = node.is_some();
    // A replaced controller (F10) still believes it belongs to us but has a
    // revoked key: keep it apart from its replacement, which has its id now.
    if member
        && !authenticated
        && state.identity().role == LocalRole::Leader
        && b.adopted_by.as_deref() == Some(state.identity().id.as_str())
        // The replacement's own broadcast beacons are unauthenticated too.
        && !sh
            .peers
            .read()
            .get(&b.id)
            .is_some_and(|p| p.authenticated && same_device(p, &b, src))
        && leader::is_retired_beacon(sh, &b, node.and_then(|n| n.serial.as_deref()))
    {
        sh.retired.lock().insert(
            b.id.clone(),
            leader::RetiredSighting {
                beacon: b,
                addr: src,
                last_seen: now,
            },
        );
        return;
    }
    drop(show);
    {
        let mut peers = sh.peers.write();
        peers.retain(|_, p| now.duration_since(p.last_seen) < PEER_EXPIRY);
        if peers.len() >= 1024 && !peers.contains_key(&b.id) {
            return; // someone is flooding us with made-up ids
        }
        let mut duplicate_until = None;
        if let Some(p) = peers.get_mut(&b.id) {
            duplicate_until = p.duplicate_until.filter(|t| *t > now);
            let recent = now.duration_since(p.last_seen) < ENTRY_STICKY;
            if !authenticated && recent {
                let other_device = !same_device(p, &b, src);
                if p.authenticated && member {
                    // An unauthenticated beacon never displaces a peer that proved
                    // its key recently (spoofing). If the member really lost its
                    // key its authenticated beacons stop and this gives way.
                    if other_device {
                        p.duplicate_until = Some(now + DUPLICATE_HOLD);
                    }
                    return;
                }
                if other_device {
                    // Two unverified devices claim the same id: keep the first,
                    // warn, and refuse to adopt it until it clears.
                    p.duplicate_until = Some(now + DUPLICATE_HOLD);
                    return;
                }
            }
        }
        peers.insert(
            b.id.clone(),
            Peer {
                beacon: b.clone(),
                addr: src,
                last_seen: now,
                seen_at: chrono::Utc::now(),
                authenticated,
                duplicate_until,
            },
        );
    }
    let identity = state.identity();
    if identity.role == LocalRole::Follower
        && authenticated
        && b.role == LocalRole::Leader
        && identity.leader_id.as_deref() == Some(b.id.as_str())
    {
        follower::on_leader_beacon(state, sh, &b, src);
    }
}

/// Our beacon.
pub(crate) fn build_beacon(state: &AppState, sh: &Shared, ips: Vec<std::net::IpAddr>) -> Beacon {
    let identity = state.identity();
    let (board, board_rev) = net::local_board(state);
    let hostname = net::hostname();
    let show = state.store.get();
    let own_node = show.node(&identity.id);
    let name = own_node
        .map(|n| n.name.clone())
        .or(identity.name.clone())
        .unwrap_or_else(|| net::title_case(&hostname));
    let (adopted_by, show_version, report) = match identity.role {
        LocalRole::Leader => (None, show.version, None),
        LocalRole::Follower => {
            let r = follower::report(state, sh);
            (
                identity.leader_id.clone(),
                r.manifest_version,
                Some(r).filter(|_| identity.leader_id.is_some()),
            )
        }
        LocalRole::Unconfigured => (None, 0, None),
    };
    Beacon {
        proto_max: Some(proto::PROTOCOL_MAX),
        proto_min: Some(proto::PROTOCOL_MIN),
        hw: net::hardware_serial(),
        id: identity.id.clone(),
        name,
        hostname,
        role: identity.role,
        board,
        board_rev,
        pi: net::pi_model(),
        ver: super::VERSION.to_string(),
        http: sh.settings.http_port,
        overlay: sh.settings.overlay_port,
        adopted_by,
        ips,
        boot: sh.boot.clone(),
        show_version,
        report,
        joining: sh.join_window().is_some(),
        proto: proto::PROTOCOL_VERSION,
    }
}

async fn beacon_loop(state: AppState, sh: Arc<Shared>) {
    let mut stop = sh.stop_rx();
    loop {
        let ifaces = net::interfaces();
        let identity = state.identity();
        let beacon = build_beacon(&state, &sh, ifaces.ips.clone());
        let mut dests = sh.broadcast_dests(sh.settings.port);
        dests.extend(sh.static_peers().await);
        if let Some(ip) = sh.join_window().and_then(|w| w.leader_ip) {
            // "Join another show" named a leader: make sure it hears us.
            dests.push(SocketAddr::new(ip, sh.settings.port));
        }
        let msg = Msg::Beacon(beacon);
        match identity.role {
            LocalRole::Leader => {
                // Unauthenticated for discovery, plus a copy MACed with each
                // follower's own key, unicast (Wi-Fi broadcast is lossy and
                // some networks filter it).
                dests.sort();
                dests.dedup();
                sh.send_json(&msg, None, &dests).await;
                for (_, addr, key) in leader::follower_targets(&state, &sh) {
                    sh.send_json(&msg, Some(&key), &[addr]).await;
                }
            }
            LocalRole::Follower | LocalRole::Unconfigured => {
                dests.extend(follower::leader_addr(&state, &sh));
                dests.sort();
                dests.dedup();
                let key = identity.cluster_key.as_deref().filter(|_| {
                    identity.role == LocalRole::Follower && identity.leader_id.is_some()
                });
                sh.send_json(&msg, key, &dests).await;
            }
        }
        if sleep_or_stop(&mut stop, sh.settings.beacon_interval).await {
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// mDNS
// ---------------------------------------------------------------------------
//
// Every node advertises `_pixelplus._tcp` (TXT id/role/board/ver). There must be
// exactly one mDNS responder per host name: two responders (avahi + mdns-sd)
// answering for `<hostname>.local` with different address sets each see the
// other's records as a name conflict and rename the host (`pixelplus-2.local`).
// Sharing UDP 5353 itself is fine (both use SO_REUSEADDR/SO_REUSEPORT and join
// the multicast group), so the rule is about *who owns the names*:
//
// * avahi-daemon running (PixelPlus images, most Linux desktops): publish the
//   service through avahi (`avahi-publish -s`, D-Bus), so avahi remains the
//   only responder; the static packaging/avahi/pixelplus.service only carries
//   `_http._tcp`.
// * otherwise (Docker, a PC without avahi): mdns-sd. In Docker, or whenever we
//   can't tell whether the host runs its own responder, its SRV target is a
//   PixelPlus-only host name (`pixelplus-<id>.local`) so it never competes for
//   the machine's own `<hostname>.local`.

const MDNS_TYPE: &str = "_pixelplus._tcp.local.";
const MDNS_TYPE_SHORT: &str = "_pixelplus._tcp";

/// (instance name, TXT key/values) we advertise.
type MdnsRecord = (String, Vec<(String, String)>);

/// The instance name and TXT record we advertise right now.
fn mdns_record(state: &AppState) -> MdnsRecord {
    let identity = state.identity();
    let (board, _) = net::local_board(state);
    let hostname = net::hostname();
    let role = match identity.role {
        LocalRole::Leader => "leader",
        LocalRole::Follower => "follower",
        LocalRole::Unconfigured => "unconfigured",
    };
    let board_id = serde_json::to_value(board)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    let txt = vec![
        ("id".to_string(), identity.id.clone()),
        ("role".to_string(), role.to_string()),
        ("board".to_string(), board_id),
        ("ver".to_string(), super::VERSION.to_string()),
    ];
    (format!("{hostname}-{}", identity.id), txt)
}

/// avahi-daemon is running here and we can publish through it.
pub(crate) fn avahi_available() -> bool {
    use crate::services::system::have;
    (std::path::Path::new("/run/avahi-daemon/pid").exists()
        || std::path::Path::new("/run/avahi-daemon/socket").exists())
        && have("avahi-publish")
}

/// `avahi-publish` arguments for our record.
pub(crate) fn avahi_publish_args(
    instance: &str,
    port: u16,
    txt: &[(String, String)],
) -> Vec<String> {
    let mut args = vec![
        "-s".to_string(),
        instance.to_string(),
        MDNS_TYPE_SHORT.to_string(),
        port.to_string(),
    ];
    args.extend(txt.iter().map(|(k, v)| format!("{k}={v}")));
    args
}

/// SRV target host for mdns-sd: the machine's own name only when nobody else
/// can be answering for it.
pub(crate) fn mdns_sd_host(hostname: &str, node_id: &str, shared_host: bool) -> String {
    if shared_host {
        let short: String = node_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(8)
            .collect();
        format!("pixelplus-{}.local.", short.to_ascii_lowercase())
    } else {
        format!("{hostname}.local.")
    }
}

async fn mdns_loop(state: AppState, sh: Arc<Shared>) {
    if avahi_available() {
        tracing::info!("mDNS: publishing {MDNS_TYPE_SHORT} through avahi-daemon");
        if avahi_loop(&state, &sh).await {
            return;
        }
        tracing::warn!(
            "mDNS: avahi-publish keeps failing; publishing with the built-in responder instead"
        );
    }
    mdns_sd_loop(state, sh).await
}

/// Keep an `avahi-publish -s` child running with our current record. Returns
/// true when stopped, false when avahi-publish keeps failing.
async fn avahi_loop(state: &AppState, sh: &Arc<Shared>) -> bool {
    let mut stop = sh.stop_rx();
    let mut child: Option<(tokio::process::Child, MdnsRecord)> = None;
    let mut quick_failures = 0u32;
    loop {
        let rec = mdns_record(state);
        let exited = match child.as_mut() {
            Some((c, _)) => matches!(c.try_wait(), Ok(Some(_)) | Err(_)),
            None => true,
        };
        let changed = child.as_ref().is_some_and(|(_, r)| *r != rec);
        if exited || changed {
            if let Some((mut c, _)) = child.take() {
                let _ = c.kill().await;
            }
            if exited && quick_failures >= 3 {
                return false;
            }
            let args = avahi_publish_args(&rec.0, sh.settings.http_port, &rec.1);
            match tokio::process::Command::new("avahi-publish")
                .args(&args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
            {
                Ok(c) => {
                    let started = Instant::now();
                    child = Some((c, rec));
                    // A child that dies within seconds (D-Bus denied, avahi gone) counts as a failure.
                    if sleep_or_stop(&mut stop, Duration::from_secs(3)).await {
                        return true;
                    }
                    let died = child
                        .as_mut()
                        .is_some_and(|(c, _)| matches!(c.try_wait(), Ok(Some(_))));
                    if died && started.elapsed() < Duration::from_secs(10) {
                        quick_failures += 1;
                        tracing::debug!("avahi-publish exited early ({quick_failures})");
                    } else {
                        quick_failures = 0;
                    }
                }
                Err(e) => {
                    tracing::warn!("mDNS: can't run avahi-publish: {e}");
                    return false;
                }
            }
        }
        if sleep_or_stop(&mut stop, Duration::from_secs(10)).await {
            if let Some((mut c, _)) = child.take() {
                let _ = c.kill().await;
            }
            return true;
        }
    }
}

async fn mdns_sd_loop(state: AppState, sh: Arc<Shared>) {
    let mut stop = sh.stop_rx();
    let daemon = match mdns_sd::ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("mDNS unavailable ({e}); discovery uses UDP beacons only");
            return;
        }
    };
    let shared_host = crate::services::system::in_docker() || avahi_available();
    // (fullname, TXT) of the current registration.
    let mut registered: Option<MdnsRecord> = None;
    loop {
        let (instance, txt) = mdns_record(&state);
        let fullname = format!("{instance}.{MDNS_TYPE}");
        if registered.as_ref() != Some(&(fullname.clone(), txt.clone())) {
            if let Some((old, _)) = registered.take() {
                let _ = daemon.unregister(&old);
            }
            let host = mdns_sd_host(&net::hostname(), &state.identity().id, shared_host);
            let props: std::collections::HashMap<String, String> = txt.iter().cloned().collect();
            match mdns_sd::ServiceInfo::new(
                MDNS_TYPE,
                &instance,
                &host,
                "",
                sh.settings.http_port,
                props,
            ) {
                Ok(info) => match daemon.register(info.enable_addr_auto()) {
                    Ok(()) => registered = Some((fullname, txt)),
                    Err(e) => tracing::warn!("mDNS registration failed: {e}"),
                },
                Err(e) => tracing::warn!("mDNS service info invalid: {e}"),
            }
        }
        if sleep_or_stop(&mut stop, Duration::from_secs(10)).await {
            let _ = daemon.shutdown();
            return;
        }
    }
}

#[cfg(test)]
mod mdns_tests {
    use super::*;

    #[test]
    fn avahi_args() {
        let txt = vec![
            ("id".to_string(), "abc".to_string()),
            ("role".to_string(), "leader".to_string()),
        ];
        assert_eq!(
            avahi_publish_args("garage-abc", 80, &txt),
            vec![
                "-s",
                "garage-abc",
                "_pixelplus._tcp",
                "80",
                "id=abc",
                "role=leader"
            ]
        );
    }

    #[test]
    fn mdns_sd_never_claims_a_shared_hostname() {
        assert_eq!(mdns_sd_host("garage", "01J9ZQ-XY", false), "garage.local.");
        assert_eq!(
            mdns_sd_host("garage", "01J9ZQ-XYzzzz", true),
            "pixelplus-01j9zqxy.local."
        );
    }
}
