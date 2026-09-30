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
    let port = if overlay { sh.settings.overlay_port } else { sh.settings.port };
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
    let slot = if overlay { &sh.overlay_socket } else { &sh.socket };
    let _ = slot.set(sock.clone());

    let mut buf = vec![0u8; 65_536];
    loop {
        let (n, src) = tokio::select! {
            r = sock.recv_from(&mut buf) => match r {
                Ok(v) => v,
                Err(e) => {
                    // ICMP errors (e.g. port unreachable after a send) surface here on
                    // some platforms; they are harmless.
                    tracing::trace!("{what} recv: {e}");
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
    {
        let mut peers = sh.peers.write();
        peers.retain(|_, p| now.duration_since(p.last_seen) < PEER_EXPIRY);
        if peers.len() >= 1024 && !peers.contains_key(&b.id) {
            return; // someone is flooding us with made-up ids
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
    let hw = net::hardware();
    let (board, board_rev) = net::local_board(&identity);
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
            (identity.leader_id.clone(), r.manifest_version, Some(r).filter(|_| identity.leader_id.is_some()))
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
        pi: hw.pi_model.clone(),
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
        sh.send_json(&Msg::Beacon(beacon), identity.cluster_key.as_deref(), &dests).await;
        if sleep_or_stop(&mut stop, sh.settings.beacon_interval).await {
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// mDNS
// ---------------------------------------------------------------------------

const MDNS_TYPE: &str = "_pixelplus._tcp.local.";

async fn mdns_loop(state: AppState, sh: Arc<Shared>) {
    let mut stop = sh.stop_rx();
    let daemon = match mdns_sd::ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("mDNS unavailable ({e}); discovery uses UDP beacons only");
            return;
        }
    };
    // (fullname, TXT) of the current registration.
    let mut registered: Option<(String, Vec<(String, String)>)> = None;
    loop {
        let identity = state.identity();
        let (board, _) = net::local_board(&identity);
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
        let instance = format!("{hostname}-{}", identity.id);
        let fullname = format!("{instance}.{MDNS_TYPE}");
        if registered.as_ref() != Some(&(fullname.clone(), txt.clone())) {
            if let Some((old, _)) = registered.take() {
                let _ = daemon.unregister(&old);
            }
            let host = format!("{hostname}.local.");
            let props: std::collections::HashMap<String, String> = txt.iter().cloned().collect();
            match mdns_sd::ServiceInfo::new(MDNS_TYPE, &instance, &host, "", sh.settings.http_port, props) {
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
