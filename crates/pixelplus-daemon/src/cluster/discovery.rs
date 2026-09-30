//! Discovery: UDP sockets, the receive loops, beacons and mDNS.

use super::proto::{self, Beacon, Msg};
use super::{follower, leader, net, sleep_or_stop, Peer, Shared};
use crate::node::LocalRole;
use crate::state::AppState;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;

/// Peers not heard from for this long are forgotten.
const PEER_EXPIRY: Duration = Duration::from_secs(300);
/// How long an authenticated peer entry resists unauthenticated beacons.
const MEMBER_STICKY: Duration = Duration::from_secs(10);

pub(crate) fn spawn(state: &AppState, sh: &Arc<Shared>) {
    tokio::spawn(bind_and_receive(state.clone(), sh.clone(), false));
    tokio::spawn(bind_and_receive(state.clone(), sh.clone(), true));
    tokio::spawn(beacon_loop(state.clone(), sh.clone()));
    if sh.settings.mdns {
        tokio::spawn(mdns_loop(state.clone(), sh.clone()));
    }
}

async fn bind(sh: &Shared, port: u16) -> std::io::Result<UdpSocket> {
    let sock = UdpSocket::bind(SocketAddr::new(sh.settings.bind, port)).await?;
    sock.set_broadcast(true)?;
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
        match bind(&sh, port).await {
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
        let (n, src) = tokio::select! {
            r = sock.recv_from(&mut buf) => match r {
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
        let data = &buf[..n];
        if overlay {
            follower::on_overlay_packet(&state, &sh, data).await;
        } else {
            on_packet(&state, &sh, data, src).await;
        }
    }
}

async fn on_packet(state: &AppState, sh: &Arc<Shared>, data: &[u8], src: SocketAddr) {
    let identity = state.identity();
    let decoded = match proto::decode(data, identity.cluster_key.as_deref()) {
        Ok(d) => d,
        Err(e) => {
            tracing::trace!("ignoring packet from {src}: {e}");
            return;
        }
    };
    let auth = decoded.authenticated;
    match decoded.msg {
        Msg::Beacon(b) => {
            if b.id == identity.id {
                return; // our own broadcast
            }
            on_beacon(state, sh, b, src, auth);
        }
        Msg::Sync(p) => {
            if p.leader == identity.id {
                return;
            }
            if auth {
                follower::on_sync(state, sh, p, src).await;
            }
        }
        Msg::Ping(p) => {
            if auth && p.id != identity.id {
                leader::on_ping(state, sh, p, src).await;
            }
        }
        Msg::Pong(p) => {
            if auth && p.id != identity.id {
                follower::on_pong(state, sh, p, src);
            }
        }
    }
}

fn on_beacon(state: &AppState, sh: &Arc<Shared>, b: Beacon, src: SocketAddr, authenticated: bool) {
    let now = Instant::now();
    // Released nodes legitimately switch to unauthenticated beacons.
    let member = state.store.get().node(&b.id).is_some_and(|n| n.adopted);
    {
        let mut peers = sh.peers.write();
        peers.retain(|_, p| now.duration_since(p.last_seen) < PEER_EXPIRY);
        if peers.len() >= 1024 && !peers.contains_key(&b.id) {
            return; // someone is flooding us with made-up ids
        }
        // An unauthenticated beacon must not displace a cluster member that
        // proved its key recently (spoofing). If the member really lost its
        // key, its authenticated beacons stop and this gives way after 10 s.
        if !authenticated
            && member
            && peers
                .get(&b.id)
                .is_some_and(|p| p.authenticated && now.duration_since(p.last_seen) < MEMBER_STICKY)
        {
            return;
        }
        peers.insert(
            b.id.clone(),
            Peer {
                beacon: b.clone(),
                addr: src,
                last_seen: now,
                seen_at: chrono::Utc::now(),
                authenticated,
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
        match identity.role {
            // Unicast to known followers too: Wi-Fi broadcast is lossy and some
            // networks filter it.
            LocalRole::Leader => dests.extend(leader::follower_addrs(&state, &sh)),
            LocalRole::Follower => dests.extend(follower::leader_addr(&state, &sh)),
            LocalRole::Unconfigured => {}
        }
        dests.sort();
        dests.dedup();
        sh.send_json(
            &Msg::Beacon(beacon),
            identity.cluster_key.as_deref(),
            &dests,
        )
        .await;
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
