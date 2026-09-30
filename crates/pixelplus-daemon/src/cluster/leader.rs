//! Leader side: its own node, adoption, node health, sync sender, slice
//! warming, commands and overlay forwarding.

use super::proto::{self, FileProgress, Msg, Ping, Pong, SyncState};
use super::sig;
use super::{
    log_warning, net, sleep_or_stop, ClusterCommand, ClusterEvent, CommandResult, DiscoveredNode,
    NodeStatus, Peer, Shared,
};
use crate::api::{ApiError, ApiResult};
use crate::node::LocalRole;
use crate::player::{PlayerState, PlayerStatus, SyncPacket};
use crate::state::AppState;
use pixelplus_core::model::{BoardKind, Node, NodeRole, OutputConfig, Show};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Discovered nodes older than this are not offered for adoption.
const DISCOVERY_FRESH: Duration = Duration::from_secs(10);
/// Don't raise "offline" alerts for followers that simply have not reported
/// since this leader started.
const STARTUP_GRACE: Duration = Duration::from_secs(30);
/// After an adoption, wait this long for the follower's first authenticated
/// beacon before judging (re-adopting, "offline") it.
const ADOPT_GRACE: Duration = Duration::from_secs(10);
const READOPT_EVERY: Duration = Duration::from_secs(15);

pub(crate) fn spawn(state: &AppState, sh: &Arc<Shared>) {
    tokio::spawn(monitor_loop(state.clone(), sh.clone()));
    tokio::spawn(sync_loop(state.clone(), sh.clone()));
    tokio::spawn(slice_worker(state.clone(), sh.clone()));
}

fn is_leader(state: &AppState) -> bool {
    state.identity().role == LocalRole::Leader
}

// ---------------------------------------------------------------------------
// The leader's own node
// ---------------------------------------------------------------------------

/// Resize an output list to `board`, keeping the settings of outputs that
/// still exist and relabelling them.
pub fn fit_outputs(outputs: &[OutputConfig], board: BoardKind) -> Vec<OutputConfig> {
    board
        .default_outputs()
        .into_iter()
        .map(
            |default| match outputs.iter().find(|o| o.index == default.index) {
                Some(o) => OutputConfig {
                    label: default.label.clone(),
                    ..o.clone()
                },
                None => default,
            },
        )
        .collect()
}

/// Make sure the leader itself is in `show.nodes` (role leader) with current
/// hardware facts. Call after the setup wizard makes this node the leader.
/// Does nothing on followers; cheap when nothing changed.
///
/// If the show has a leader node with another id (a show restored onto a new
/// Pi) that node is taken over, keeping its wiring and output settings.
pub async fn ensure_self_node(state: &AppState) -> anyhow::Result<()> {
    let identity = state.identity();
    if identity.role != LocalRole::Leader {
        return Ok(());
    }
    let (board, board_rev) = net::local_board(state);
    let hostname = net::hostname();
    let pi_model = net::pi_model();
    let desired = |n: &Node| {
        n.role == NodeRole::Leader
            && n.adopted
            && n.board == board
            && n.hostname == hostname
            && n.board_rev == board_rev
            && n.pi_model == pi_model
            && n.outputs.len() == board.output_count()
    };
    let show = state.store.get();
    let others_lead = show
        .nodes
        .iter()
        .any(|n| n.id != identity.id && n.role == NodeRole::Leader);
    if show.node(&identity.id).is_some_and(desired) && !others_lead {
        return Ok(());
    }
    let my_id = identity.id.clone();
    let default_name = identity
        .name
        .clone()
        .unwrap_or_else(|| net::title_case(&hostname));
    state
        .store
        .update(move |s| {
            if s.node(&my_id).is_none() {
                let old: Vec<String> = s
                    .nodes
                    .iter()
                    .filter(|n| n.role == NodeRole::Leader)
                    .map(|n| n.id.clone())
                    .collect();
                if let [old_id] = old.as_slice() {
                    tracing::info!("taking over leader node {old_id} of the restored show");
                    rename_node(s, old_id, &my_id);
                } else {
                    s.nodes.insert(
                        0,
                        Node {
                            id: my_id.clone(),
                            name: default_name.clone(),
                            hostname: hostname.clone(),
                            role: NodeRole::Leader,
                            board,
                            board_rev: board_rev.clone(),
                            pi_model: pi_model.clone(),
                            outputs: board.default_outputs(),
                            adopted: true,
                            last_seen: None,
                            notes: None,
                        },
                    );
                }
            }
            for n in s.nodes.iter_mut() {
                if n.id == my_id {
                    n.role = NodeRole::Leader;
                    n.adopted = true;
                    n.hostname = hostname.clone();
                    n.board_rev = board_rev.clone();
                    n.pi_model = pi_model.clone();
                    if n.board != board || n.outputs.len() != board.output_count() {
                        n.outputs = fit_outputs(&n.outputs, board);
                        n.board = board;
                    }
                } else if n.role == NodeRole::Leader {
                    // Only one leader per show.
                    n.role = NodeRole::Follower;
                    n.adopted = false;
                }
            }
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("{}", e.message))?;
    Ok(())
}

/// Replace node id `from` with `to` everywhere in the show.
pub fn rename_node(show: &mut Show, from: &str, to: &str) {
    for n in show.nodes.iter_mut().filter(|n| n.id == from) {
        n.id = to.to_string();
    }
    for p in show.props.iter_mut() {
        for s in p.segments.iter_mut().filter(|s| s.node_id == from) {
            s.node_id = to.to_string();
        }
    }
    for r in show.receivers.iter_mut().filter(|r| r.node_id == from) {
        r.node_id = to.to_string();
    }
}

/// Remove a node and everything wired to it (segments, receivers).
pub fn remove_node(show: &mut Show, id: &str) {
    show.nodes.retain(|n| n.id != id);
    for p in show.props.iter_mut() {
        p.segments.retain(|s| s.node_id != id);
    }
    show.receivers.retain(|r| r.node_id != id);
}

// ---------------------------------------------------------------------------
// Peers & status
// ---------------------------------------------------------------------------

/// A peer that is adopted by us and proved it has our key.
fn member<'a>(peer: Option<&'a Peer>, my_id: &str) -> Option<&'a Peer> {
    peer.filter(|p| p.authenticated && p.beacon.adopted_by.as_deref() == Some(my_id))
}

/// Adopted followers heard from within the last minute: (id, UDP address, key).
pub(crate) fn follower_targets(state: &AppState, sh: &Shared) -> Vec<(String, SocketAddr, String)> {
    let my_id = state.identity().id;
    let show = state.store.get();
    let found: Vec<(String, SocketAddr)> = {
        let peers = sh.peers.read();
        show.nodes
            .iter()
            .filter(|n| n.role == NodeRole::Follower && n.adopted)
            .filter_map(|n| member(peers.get(&n.id), &my_id))
            .filter(|p| p.last_seen.elapsed() < Duration::from_secs(60))
            .map(|p| (p.beacon.id.clone(), p.addr))
            .collect()
    };
    found
        .into_iter()
        .filter_map(|(id, addr)| {
            let key = sh.follower_key(state, &id)?;
            Some((id, addr, key))
        })
        .collect()
}

fn rfc3339(t: chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn self_status(state: &AppState, sh: &Shared, node: Option<&Node>) -> NodeStatus {
    let identity = state.identity();
    let (board, _) = net::local_board(state);
    let hostname = net::hostname();
    let follower = identity.role == LocalRole::Follower;
    let report = follower.then(|| super::follower::report(state, sh));
    NodeStatus {
        id: identity.id.clone(),
        name: node
            .map(|n| n.name.clone())
            .or(identity.name.clone())
            .unwrap_or_else(|| net::title_case(&hostname)),
        role: if follower {
            NodeRole::Follower
        } else {
            NodeRole::Leader
        },
        adopted: node
            .map(|n| n.adopted)
            .unwrap_or(identity.role == LocalRole::Leader)
            || identity.leader_id.is_some(),
        online: true,
        last_seen: Some(rfc3339(chrono::Utc::now())),
        board,
        sync_offset_ms: report
            .as_ref()
            .and_then(|r| r.sync_offset_ms)
            .unwrap_or(0.0),
        sync_state: report
            .as_ref()
            .map(|r| r.state)
            .unwrap_or(SyncState::Synced),
        files: report.as_ref().map(|r| r.files).unwrap_or_default(),
        ip: net::interfaces().ips.first().map(|i| i.to_string()),
        version: Some(super::VERSION.to_string()),
        pi_model: net::pi_model(),
        hostname,
        problem: report.and_then(|r| r.problem),
    }
}

fn follower_status(
    show: &Show,
    node: &Node,
    peer: Option<&Peer>,
    my_id: &str,
    sh: &Shared,
) -> NodeStatus {
    let m = member(peer, my_id);
    let online = m.is_some_and(|p| p.last_seen.elapsed() < sh.settings.offline_after);
    let report = m.and_then(|p| p.beacon.report.clone()).unwrap_or_default();
    let sync_state = if !online {
        SyncState::Offline
    } else if report.state == SyncState::Synced && report.manifest_version == show.version {
        SyncState::Synced
    } else {
        SyncState::Syncing
    };
    let problem = match peer {
        _ if !node.adopted => None,
        Some(p) if m.is_none() && p.last_seen.elapsed() < sh.settings.offline_after => {
            Some(match &p.beacon.adopted_by {
                None => "It forgot this leader; adopting it again…".to_string(),
                Some(other) if other != my_id => {
                    "It is controlled by another show leader.".to_string()
                }
                Some(_) => {
                    "Its cluster key does not match; release it and adopt it again.".to_string()
                }
            })
        }
        _ if online => report.problem.clone(),
        _ => None,
    };
    NodeStatus {
        id: node.id.clone(),
        name: node.name.clone(),
        role: node.role,
        adopted: node.adopted,
        online,
        last_seen: peer
            .map(|p| rfc3339(p.seen_at))
            .or_else(|| node.last_seen.clone()),
        board: node.board,
        sync_offset_ms: report.sync_offset_ms.filter(|_| online).unwrap_or(0.0),
        sync_state,
        files: if online {
            report.files
        } else {
            FileProgress::default()
        },
        ip: peer.map(|p| p.addr.ip().to_string()),
        version: peer.map(|p| p.beacon.ver.clone()),
        pi_model: node
            .pi_model
            .clone()
            .or_else(|| peer.and_then(|p| p.beacon.pi.clone())),
        hostname: node.hostname.clone(),
        problem,
    }
}

pub(crate) fn nodes_status(state: &AppState, sh: &Shared) -> Vec<NodeStatus> {
    let identity = state.identity();
    let show = state.store.get();
    let peers = sh.peers.read();
    match identity.role {
        LocalRole::Leader => show
            .nodes
            .iter()
            .map(|n| {
                if n.id == identity.id {
                    self_status(state, sh, Some(n))
                } else {
                    follower_status(&show, n, peers.get(&n.id), &identity.id, sh)
                }
            })
            .collect(),
        LocalRole::Follower | LocalRole::Unconfigured => {
            let mut out = vec![self_status(state, sh, show.node(&identity.id))];
            if let Some(leader) = identity.leader_id.as_ref().and_then(|l| peers.get(l)) {
                let contact = sh.follower.lock().last_leader_contact;
                let online = contact.is_some_and(|t| t.elapsed() < sh.settings.offline_after);
                out.push(NodeStatus {
                    id: leader.beacon.id.clone(),
                    name: leader.beacon.name.clone(),
                    role: NodeRole::Leader,
                    adopted: true,
                    online,
                    last_seen: Some(rfc3339(leader.seen_at)),
                    board: leader.beacon.board,
                    sync_offset_ms: 0.0,
                    sync_state: if online {
                        SyncState::Synced
                    } else {
                        SyncState::Offline
                    },
                    files: FileProgress::default(),
                    ip: Some(leader.addr.ip().to_string()),
                    version: Some(leader.beacon.ver.clone()),
                    pi_model: leader.beacon.pi.clone(),
                    hostname: leader.beacon.hostname.clone(),
                    problem: None,
                });
            }
            out
        }
    }
}

pub(crate) fn discovered(state: &AppState, sh: &Shared) -> Vec<DiscoveredNode> {
    let identity = state.identity();
    let show = state.store.get();
    let peers = sh.peers.read();
    let mut out: Vec<DiscoveredNode> = peers
        .values()
        .filter(|p| p.last_seen.elapsed() < DISCOVERY_FRESH)
        // A leader is offered only while its admin has "Join another show" open.
        .filter(|p| p.beacon.role != LocalRole::Leader || p.beacon.joining)
        .filter(|p| !show.node(&p.beacon.id).is_some_and(|n| n.adopted))
        .filter(|p| {
            p.beacon.adopted_by.as_deref() != Some(identity.id.as_str()) || !p.authenticated
        })
        .map(|p| DiscoveredNode {
            id: p.beacon.id.clone(),
            name: p.beacon.name.clone(),
            hostname: p.beacon.hostname.clone(),
            role: p.beacon.role,
            board: p.beacon.board,
            board_rev: p.beacon.board_rev.clone(),
            pi: p.beacon.pi.clone(),
            ip: p.addr.ip().to_string(),
            ips: p.beacon.ips.clone(),
            http: p.beacon.http,
            ver: p.beacon.ver.clone(),
            adopted_by: p.beacon.adopted_by.clone(),
            last_seen: rfc3339(p.seen_at),
            duplicate: p.duplicate(),
            joining: p.beacon.joining,
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    out
}

// ---------------------------------------------------------------------------
// Adoption
// ---------------------------------------------------------------------------

/// Body of `POST /nodes/adopt`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptRequest {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    /// Id of a placeholder node (created before the hardware arrived) whose
    /// wiring this controller takes over.
    #[serde(default)]
    pub replaces: Option<String>,
    /// Take it over from another (unreachable) leader.
    #[serde(default)]
    pub force: bool,
}

/// Leader → follower `POST /cluster/adopt` (signed with the follower's
/// current key when the leader has one: re-adoption / re-keying).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptCall {
    pub leader_id: String,
    pub leader_url: String,
    /// The leader's X25519 public key (hex) for this adoption.
    pub dh: String,
    #[serde(default)]
    pub force: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Follower's reply to an adoption.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptReply {
    pub id: String,
    pub name: String,
    pub hostname: String,
    pub board: BoardKind,
    #[serde(default)]
    pub board_rev: Option<String>,
    #[serde(default)]
    pub pi_model: Option<String>,
    /// The follower's X25519 public key (hex).
    #[serde(default)]
    pub dh: String,
    /// [`sig::adopt_proof`] with the derived key.
    #[serde(default)]
    pub proof: String,
}

pub fn validate_node_name(name: &str) -> ApiResult<()> {
    if name.trim().is_empty() {
        return Err(ApiError::bad_request("Please give the controller a name."));
    }
    if name.chars().count() > 120 {
        return Err(ApiError::bad_request(
            "That name is too long (120 characters max).",
        ));
    }
    Ok(())
}

/// Extract `{"error":{"message"}}` from a PixelPlus error response.
pub(crate) async fn error_message(resp: reqwest::Response) -> String {
    let status = resp.status();
    match resp.json::<serde_json::Value>().await {
        Ok(v) => v["error"]["message"]
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| format!("HTTP {status}")),
        Err(_) => format!("HTTP {status}"),
    }
}

/// Run the adoption handshake with `peer`: returns its reply and the new
/// follower key. The call is signed with the key we already share with that
/// controller, if any (re-adoption, re-keying).
async fn call_adopt(
    state: &AppState,
    sh: &Shared,
    peer: &Peer,
    force: bool,
    name: Option<String>,
) -> ApiResult<(AdoptReply, String)> {
    let identity = state.identity();
    let ip = net::local_ip_towards(peer.addr.ip())
        .or_else(|| net::interfaces().ips.first().copied())
        .ok_or_else(|| ApiError::internal("this leader has no network address"))?;
    let offer = sig::dh_offer().map_err(ApiError::internal)?;
    let call = AdoptCall {
        leader_id: identity.id.clone(),
        leader_url: net::http_url(ip, sh.settings.http_port),
        dh: offer.public_hex.clone(),
        force,
        name,
    };
    let body = serde_json::to_vec(&call).map_err(ApiError::internal)?;
    let url = format!("{}/api/v1/cluster/adopt", peer.http_base());
    let unreachable = |e: &dyn std::fmt::Display| {
        ApiError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "unreachable",
            format!(
                "Couldn't reach {} at {} ({e}). Check that it is powered on and on the same network.",
                peer.beacon.name,
                peer.addr.ip(),
            ),
        )
    };
    let resp = match sh.follower_key(state, &peer.beacon.id) {
        Some(key) => {
            sig::call(
                sh,
                &key,
                &identity.id,
                &peer.beacon.id,
                reqwest::Method::POST,
                &url,
                Some(body),
                Duration::from_secs(8),
                &[],
            )
            .await
            .map_err(|e| unreachable(&e))?
            .resp
        }
        None => sh
            .http
            .post(&url)
            .header("x-pixelplus-request", "1")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .timeout(Duration::from_secs(8))
            .send()
            .await
            .map_err(|e| unreachable(&short_err(&e)))?,
    };
    if !resp.status().is_success() {
        let msg = error_message(resp).await;
        return Err(ApiError::conflict(format!(
            "{} declined: {msg}",
            peer.beacon.name
        )));
    }
    let reply = resp
        .json::<AdoptReply>()
        .await
        .map_err(|e| ApiError::internal(format!("unexpected adopt reply: {e}")))?;
    let bad = || {
        ApiError::conflict(format!(
            "{} answered, but could not prove it completed the secure handshake. Is it running an older PixelPlus? Update it and try again.",
            peer.beacon.name
        ))
    };
    if !sig::valid_public(&reply.dh) {
        return Err(bad());
    }
    let key = sig::derive_key(
        offer,
        &reply.dh,
        &identity.id,
        &reply.id,
        &call.dh,
        &reply.dh,
    )
    .ok_or_else(bad)?;
    if !proto::ct_eq(
        sig::adopt_proof(&key, &identity.id, &reply.id).as_bytes(),
        reply.proof.as_bytes(),
    ) {
        return Err(bad());
    }
    Ok((reply, key))
}

fn short_err(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timed out".into()
    } else if e.is_connect() {
        "connection refused".into()
    } else {
        e.to_string()
    }
}

/// Adopt a discovered controller and add it to the show.
pub async fn adopt(state: &AppState, sh: &Shared, req: AdoptRequest) -> ApiResult<Node> {
    if !is_leader(state) {
        return Err(ApiError::conflict(
            "Only the show leader can adopt controllers.",
        ));
    }
    let my_id = state.identity().id;
    if req.id == my_id {
        return Err(ApiError::bad_request(
            "This controller is the leader itself.",
        ));
    }
    if let Some(n) = &req.name {
        validate_node_name(n)?;
    }
    let show = state.store.get();
    if let Some(ph) = &req.replaces {
        let node = show
            .node(ph)
            .ok_or_else(|| ApiError::not_found("The controller to replace"))?;
        if node.role == NodeRole::Leader {
            return Err(ApiError::bad_request(
                "The show leader cannot be replaced by a follower.",
            ));
        }
        if ph != &req.id && show.node(&req.id).is_some() {
            return Err(ApiError::conflict(
                "That controller is already part of the show.",
            ));
        }
    }
    let peer = sh
        .peers
        .read()
        .get(&req.id)
        .filter(|p| p.last_seen.elapsed() < Duration::from_secs(30))
        .cloned()
        .ok_or_else(|| {
            ApiError::new(
                axum::http::StatusCode::NOT_FOUND,
                "not_found",
                "That controller is not announcing itself any more. Check that it is powered on and on the same network.",
            )
        })?;
    if peer.duplicate() {
        return Err(ApiError::conflict(format!(
            "Two devices on the network claim to be {} (possible duplicate or impostor). Check that only one controller uses this SD card, then try again in a minute.",
            peer.beacon.name
        )));
    }
    if peer.beacon.role == LocalRole::Leader {
        if !peer.beacon.joining {
            return Err(ApiError::conflict(format!(
                "{} is itself a show leader. To add it to this show, open PixelPlus on {} and choose Controllers → Join another show, then adopt it here.",
                peer.beacon.name, peer.beacon.name
            )));
        }
        if !req.force {
            return Err(ApiError::conflict(format!(
                "{} is itself a show leader. Adopting it replaces its own show with this one; confirm to continue.",
                peer.beacon.name
            )));
        }
    }
    let (reply, key) = call_adopt(state, sh, &peer, req.force, req.name.clone()).await?;
    if reply.id != req.id {
        return Err(ApiError::conflict(
            "A different controller answered at that address; try again.",
        ));
    }
    let reply_id = reply.id.clone();
    sh.update_keys(|k| {
        k.followers.insert(reply_id, key);
    });
    sh.replay.lock().forget(&reply.id);

    let name = req
        .name
        .clone()
        .map(|n| n.trim().to_string())
        .unwrap_or_else(|| {
            if reply.name.trim().is_empty() {
                net::title_case(&reply.hostname)
            } else {
                reply.name.clone()
            }
        });
    let replaces = req.replaces.clone();
    let (node, _) = state
        .store
        .update(move |s| {
            if let Some(ph) = replaces.as_deref().filter(|ph| *ph != reply.id) {
                rename_node(s, ph, &reply.id);
            }
            let fresh = Node {
                id: reply.id.clone(),
                name: name.clone(),
                hostname: reply.hostname.clone(),
                role: NodeRole::Follower,
                board: reply.board,
                board_rev: reply.board_rev.clone(),
                pi_model: reply.pi_model.clone(),
                outputs: reply.board.default_outputs(),
                adopted: true,
                last_seen: None,
                notes: None,
            };
            let node = match s.nodes.iter_mut().find(|n| n.id == reply.id) {
                Some(n) => {
                    // Re-adoption or placeholder: keep the user's settings.
                    n.role = NodeRole::Follower;
                    n.adopted = true;
                    n.hostname = fresh.hostname;
                    n.board_rev = fresh.board_rev;
                    n.pi_model = fresh.pi_model;
                    if n.board != fresh.board || n.outputs.len() != fresh.board.output_count() {
                        n.outputs = fit_outputs(&n.outputs, fresh.board);
                        n.board = fresh.board;
                    }
                    if req.name.is_some() {
                        n.name = name;
                    }
                    n.clone()
                }
                None => {
                    s.nodes.push(fresh.clone());
                    fresh
                }
            };
            Ok(node)
        })
        .await?;
    sh.health.lock().insert(
        node.id.clone(),
        super::Health {
            adopted_at: Some(Instant::now()),
            ..Default::default()
        },
    );
    tracing::info!("adopted {} ({}) at {}", node.name, node.id, peer.addr.ip());
    Ok(node)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseResult {
    pub ok: bool,
    /// The follower was told (it may be offline).
    pub reached: bool,
    pub removed: bool,
}

/// Tell a follower to forget this leader (best effort).
/// Also forgets the follower's key (it is worthless from now on).
pub(crate) async fn call_release(state: &AppState, sh: &Shared, node_id: &str) -> bool {
    let reached = send_release(state, sh, node_id).await;
    sh.update_keys(|k| {
        k.followers.remove(node_id);
    });
    reached
}

async fn send_release(state: &AppState, sh: &Shared, node_id: &str) -> bool {
    let my_id = state.identity().id;
    let Some(key) = sh.follower_key(state, node_id) else {
        return false;
    };
    let Some(peer) = member(sh.peers.read().get(node_id), &my_id).cloned() else {
        return false;
    };
    let url = format!("{}/api/v1/cluster/release", peer.http_base());
    match sig::call(
        sh,
        &key,
        &my_id,
        node_id,
        reqwest::Method::POST,
        &url,
        Some(b"{}".to_vec()),
        Duration::from_secs(4),
        &[],
    )
    .await
    .map(|r| r.resp)
    {
        Ok(r) if r.status().is_success() => true,
        Ok(r) => {
            tracing::warn!(
                "{} refused release: {}",
                peer.beacon.name,
                error_message(r).await
            );
            false
        }
        Err(e) => {
            tracing::warn!(
                "could not tell {} to forget this leader: {e}",
                peer.beacon.name
            );
            false
        }
    }
}

/// Release a follower; optionally remove it (and its wiring) from the show.
pub async fn release(
    state: &AppState,
    sh: &Shared,
    node_id: &str,
    remove: bool,
) -> ApiResult<ReleaseResult> {
    let show = state.store.get();
    let node = show
        .node(node_id)
        .ok_or_else(|| ApiError::not_found("That controller"))?;
    if node.role == NodeRole::Leader {
        return Err(ApiError::bad_request("The show leader cannot be released."));
    }
    let reached = call_release(state, sh, node_id).await;
    let id = node_id.to_string();
    state
        .store
        .update(move |s| {
            if remove {
                remove_node(s, &id);
            } else if let Some(n) = s.nodes.iter_mut().find(|n| n.id == id) {
                n.adopted = false;
            }
            Ok(())
        })
        .await?;
    sh.health.lock().remove(node_id);
    Ok(ReleaseResult {
        ok: true,
        reached,
        removed: remove,
    })
}

// ---------------------------------------------------------------------------
// Commands & overlays
// ---------------------------------------------------------------------------

pub(crate) async fn send_command(
    sh: &Shared,
    node_id: Option<&str>,
    mut cmd: ClusterCommand,
) -> Vec<CommandResult> {
    let Some(state) = sh.app() else { return vec![] };
    let identity = state.identity();
    let show = state.store.get();
    cmd.stamp(&show.props);
    let targets: Vec<(String, Option<Peer>)> = {
        let peers = sh.peers.read();
        show.nodes
            .iter()
            .filter(|n| n.role == NodeRole::Follower && n.adopted)
            .filter(|n| node_id.map_or(true, |id| id == n.id))
            .map(|n| {
                let p = member(peers.get(&n.id), &identity.id)
                    .filter(|p| p.last_seen.elapsed() < sh.settings.offline_after)
                    .cloned();
                (n.id.clone(), p)
            })
            .collect()
    };
    let body = serde_json::to_vec(&cmd).unwrap_or_default();
    let calls = targets.into_iter().map(|(id, peer)| {
        let key = sh.follower_key(&state, &id);
        let body = body.clone();
        let my_id = identity.id.clone();
        async move {
            let (Some(peer), Some(key)) = (peer, key) else {
                return CommandResult {
                    node_id: id,
                    ok: false,
                    error: Some("offline".into()),
                };
            };
            let url = format!("{}/api/v1/cluster/command", peer.http_base());
            let r = sig::call(
                sh,
                &key,
                &my_id,
                &id,
                reqwest::Method::POST,
                &url,
                Some(body),
                Duration::from_secs(4),
                &[],
            )
            .await
            .map(|r| r.resp);
            match r {
                Ok(r) if r.status().is_success() => CommandResult {
                    node_id: id,
                    ok: true,
                    error: None,
                },
                Ok(r) => CommandResult {
                    node_id: id,
                    ok: false,
                    error: Some(error_message(r).await),
                },
                Err(e) => CommandResult {
                    node_id: id,
                    ok: false,
                    error: Some(e.short()),
                },
            }
        }
    });
    futures::future::join_all(calls).await
}

#[allow(dead_code)] // called through ClusterHandle by the player engine
pub(crate) fn forward_overlay(sh: &Shared, prop_id: &str, rgb: &[u8]) -> usize {
    let Some(state) = sh.app() else { return 0 };
    let identity = state.identity();
    if identity.role != LocalRole::Leader {
        return 0;
    }
    let Some(sock) = sh.overlay_socket.get() else {
        return 0;
    };
    let show = state.store.get();
    let Some(prop) = show.prop(prop_id) else {
        return 0;
    };
    let mut nodes: Vec<&str> = prop
        .segments
        .iter()
        .map(|s| s.node_id.as_str())
        .filter(|id| *id != identity.id)
        .collect();
    nodes.sort_unstable();
    nodes.dedup();
    if nodes.is_empty() {
        return 0;
    }
    let dests: Vec<(String, SocketAddr)> = {
        let peers = sh.peers.read();
        nodes
            .into_iter()
            .filter_map(|id| member(peers.get(id), &identity.id))
            .map(|peer| {
                let port = match peer.beacon.overlay {
                    0 => peer.addr.port().wrapping_add(1),
                    p => p,
                };
                (
                    peer.beacon.id.clone(),
                    SocketAddr::new(peer.addr.ip(), port),
                )
            })
            .collect()
    };
    let mut sent = 0;
    for (id, dest) in dests {
        let Some(key) = sh.follower_key(&state, &id) else {
            continue;
        };
        // One packet per follower: each is MACed with that follower's key.
        let packet = match proto::encode_overlay(prop_id, rgb, sh.stamp(&key)) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!("overlay for {prop_id} not forwarded: {e}");
                return sent;
            }
        };
        if sock.try_send_to(&packet, dest).is_ok() {
            sent += 1;
        }
    }
    sent
}

// ---------------------------------------------------------------------------
// Clock
// ---------------------------------------------------------------------------

/// A follower's (authenticated, fresh) clock probe: answer with a pong MACed
/// with its key.
pub(crate) async fn on_ping(state: &AppState, sh: &Shared, ping: Ping, key: &str, src: SocketAddr) {
    let t1 = sh.now_ms();
    let identity = state.identity();
    if identity.role != LocalRole::Leader {
        return;
    }
    let pong = Msg::Pong(Pong {
        id: identity.id.clone(),
        t0: ping.t0,
        t1,
        boot: sh.boot.clone(),
    });
    sh.send_json(&pong, Some(key), &[src]).await;
}

// ---------------------------------------------------------------------------
// Monitor: health, alerts, re-adoption, `nodes` event
// ---------------------------------------------------------------------------

async fn monitor_loop(state: AppState, sh: Arc<Shared>) {
    let mut stop = sh.stop_rx();
    let mut tick: u64 = 0;
    loop {
        if is_leader(&state) {
            if let Err(e) = ensure_self_node(&state).await {
                tracing::debug!("ensure_self_node: {e:#}");
            }
            check_health(&state, &sh).await;
        }
        if tick % 2 == 0 {
            publish_nodes(&state, &sh);
        }
        tick += 1;
        if sleep_or_stop(&mut stop, Duration::from_secs(1)).await {
            return;
        }
    }
}

pub(crate) fn publish_nodes(state: &AppState, sh: &Shared) {
    let list = nodes_status(state, sh);
    match serde_json::to_value(&list) {
        Ok(v) => {
            state.services.remember("nodes", v.clone());
            state.events.publish("nodes", &v);
        }
        Err(e) => tracing::error!("nodes status: {e}"),
    }
}

pub(crate) async fn check_health(state: &AppState, sh: &Arc<Shared>) {
    let identity = state.identity();
    let show = state.store.get();
    let peers: std::collections::HashMap<String, Peer> = sh.peers.read().clone();
    let past_grace = sh.started.elapsed() > STARTUP_GRACE;
    let mut went_offline: Vec<(String, String)> = Vec::new();
    let mut readopt = Vec::new();
    let mut facts: Vec<(String, String, Option<String>, Option<String>)> = Vec::new();
    {
        let mut health = sh.health.lock();
        health.retain(|id, _| show.node(id).is_some());
        for node in show
            .nodes
            .iter()
            .filter(|n| n.role == NodeRole::Follower && n.adopted)
        {
            let peer = peers.get(&node.id);
            let m = member(peer, &identity.id);
            let online = m.is_some_and(|p| p.last_seen.elapsed() < sh.settings.offline_after);
            let h = health.entry(node.id.clone()).or_default();
            if online {
                h.adopted_at = None;
            } else if h.adopted_at.is_some_and(|t| t.elapsed() < ADOPT_GRACE) {
                // Adopted a moment ago; its first authenticated beacon is on the way.
                continue;
            }
            match (h.online, online) {
                (Some(false), true) => {
                    h.online = Some(true);
                    tracing::info!("{} is back online", node.name);
                    state.events.toast(
                        crate::events::ToastKind::Success,
                        format!("{} is back online", node.name),
                    );
                    sh.emit(ClusterEvent::NodeOnline {
                        node_id: node.id.clone(),
                        name: node.name.clone(),
                    });
                }
                (None, true) => h.online = Some(true),
                (Some(true), false) => {
                    h.online = Some(false);
                    let last = peer.map(|p| rfc3339(p.seen_at));
                    if let Some(t) = &last {
                        // Remember it across leader restarts ("last seen …").
                        went_offline.push((node.id.clone(), t.clone()));
                    }
                    log_warning(state, format!("{} went offline", node.name));
                    sh.emit(ClusterEvent::NodeOffline {
                        node_id: node.id.clone(),
                        name: node.name.clone(),
                        last_seen: last,
                    });
                }
                (None, false) if past_grace => {
                    h.online = Some(false);
                    log_warning(
                        state,
                        format!("{} has not checked in since PixelPlus started", node.name),
                    );
                    sh.emit(ClusterEvent::NodeOffline {
                        node_id: node.id.clone(),
                        name: node.name.clone(),
                        last_seen: peer.map(|p| rfc3339(p.seen_at)),
                    });
                }
                _ => {}
            }
            // Problems reported by the follower.
            let problem = m
                .filter(|_| online)
                .and_then(|p| p.beacon.report.as_ref())
                .and_then(|r| r.problem.clone());
            if problem != h.problem {
                if let Some(msg) = &problem {
                    log_warning(state, format!("{}: {msg}", node.name));
                    sh.emit(ClusterEvent::SyncProblem {
                        node_id: node.id.clone(),
                        name: node.name.clone(),
                        message: msg.clone(),
                    });
                }
                h.problem = problem;
            }
            let Some(peer) = peer.filter(|p| p.last_seen.elapsed() < sh.settings.offline_after)
            else {
                continue;
            };
            match peer.beacon.adopted_by.as_deref() {
                // It lost its settings (reset, released locally…): adopt it again.
                // Not while two devices claim this id (spoofing).
                None if peer.beacon.role != LocalRole::Leader && !peer.duplicate() => {
                    if h.last_readopt.map_or(true, |t| t.elapsed() > READOPT_EVERY) {
                        h.last_readopt = Some(Instant::now());
                        readopt.push(peer.clone());
                    }
                }
                // Adopted by an older PixelPlus with the show-wide key: give
                // it its own key (signed with the old one).
                Some(l)
                    if l == identity.id
                        && m.is_some()
                        && sh.uses_legacy_key(&node.id)
                        && h.last_readopt.map_or(true, |t| t.elapsed() > READOPT_EVERY) =>
                {
                    h.last_readopt = Some(Instant::now());
                    readopt.push(peer.clone());
                }
                Some(other) if other != identity.id => {
                    if !h.warned_foreign {
                        h.warned_foreign = true;
                        log_warning(
                            state,
                            format!("{} is now controlled by another show leader", node.name),
                        );
                    }
                }
                _ => h.warned_foreign = false,
            }
            if let Some(m) = m {
                let b = &m.beacon;
                if (!b.hostname.is_empty() && b.hostname != node.hostname)
                    || b.pi != node.pi_model
                    || b.board_rev != node.board_rev
                {
                    facts.push((
                        node.id.clone(),
                        b.hostname.clone(),
                        b.pi.clone(),
                        b.board_rev.clone(),
                    ));
                }
                if b.board != node.board && !h.warned_foreign {
                    tracing::warn!(
                        "{} reports board {:?} but is configured as {:?}",
                        node.name,
                        b.board,
                        node.board
                    );
                }
            }
        }
    }
    for peer in readopt {
        let sh2 = sh.clone();
        let state2 = state.clone();
        tokio::spawn(async move {
            match call_adopt(&state2, &sh2, &peer, false, None).await {
                Ok((reply, key)) if reply.id == peer.beacon.id => {
                    sh2.update_keys(|k| {
                        k.followers.insert(reply.id.clone(), key);
                    });
                    sh2.replay.lock().forget(&reply.id);
                    tracing::info!("re-adopted {} with a new key", peer.beacon.name);
                }
                Ok(_) => tracing::warn!(
                    "re-adopting {}: a different controller answered",
                    peer.beacon.name
                ),
                Err(e) => tracing::warn!("re-adopting {} failed: {}", peer.beacon.name, e.message),
            }
        });
    }
    if !facts.is_empty() || !went_offline.is_empty() {
        let _ = state
            .store
            .update(move |s| {
                for (id, seen) in went_offline {
                    if let Some(n) = s.nodes.iter_mut().find(|n| n.id == id) {
                        n.last_seen = Some(seen);
                    }
                }
                for (id, hostname, pi, rev) in facts {
                    if let Some(n) = s.nodes.iter_mut().find(|n| n.id == id) {
                        if !hostname.is_empty() {
                            n.hostname = hostname;
                        }
                        n.pi_model = pi;
                        n.board_rev = rev;
                    }
                }
                Ok(())
            })
            .await;
    }
}

// ---------------------------------------------------------------------------
// Sync sender
// ---------------------------------------------------------------------------

/// Leader position right now, extrapolated from the last status update.
fn position_now(status: &PlayerStatus, received: Instant) -> u64 {
    let mut pos = status.pos_ms;
    if status.state == PlayerState::Playing {
        pos += received.elapsed().as_millis() as u64;
        if status.duration_ms > 0 {
            pos = pos.min(status.duration_ms);
        }
    }
    pos
}

/// Does `new` differ from `old` in a way followers must hear about at once?
pub(crate) fn significant_change(old: &PlayerStatus, old_at: Instant, new: &PlayerStatus) -> bool {
    if old.state != new.state
        || old.item != new.item
        || old.blackout != new.blackout
        || old.brightness != new.brightness
    {
        return true;
    }
    // A seek: the position jumped away from where it should be.
    let expected = position_now(old, old_at) as i64;
    (new.pos_ms as i64 - expected).abs() > 150
}

pub(crate) fn build_sync(
    state: &AppState,
    sh: &Shared,
    status: &PlayerStatus,
    received: Instant,
) -> SyncPacket {
    let identity = state.identity();
    let show = state.store.get();
    let extras = sh.extras.lock().clone();
    let effect = match (&status.item, status.state) {
        (Some(item), _) if item.kind == "effect" => show
            .effect(&item.id)
            .cloned()
            .or_else(|| {
                pixelplus_core::effects::builtin_presets()
                    .into_iter()
                    .find(|e| e.id == item.id)
            })
            .or(extras.effect.clone()),
        // Sequences carry their own lights; for everything else (DJ clips,
        // pauses, audio-only items, idle and live looks) followers show the
        // look the leader's engine is showing.
        (Some(item), _) if item.kind == "sequence" || item.kind == "request" => None,
        _ => extras.effect.clone(),
    }
    .map(|mut e| {
        pixelplus_core::effects::stamp_world_bounds(&mut e, &show.props);
        e
    });
    let test = (status.state == PlayerState::Testing)
        .then_some(extras.test)
        .flatten()
        .map(|mut t| {
            if let Some(e) = t.effect.as_mut() {
                pixelplus_core::effects::stamp_world_bounds(e, &show.props);
            }
            t
        });
    SyncPacket {
        leader: identity.id,
        show_version: show.version,
        state: status.state,
        item: status.item.clone(),
        pos_ms: position_now(status, received),
        sent_at_ms: sh.now_ms().round() as u64,
        effect,
        test,
        brightness: status.brightness,
        blackout: status.blackout,
    }
}

async fn sync_loop(state: AppState, sh: Arc<Shared>) {
    let mut stop = sh.stop_rx();
    let player = loop {
        if let Some(p) = state.services.player.get() {
            break p.clone();
        }
        if sleep_or_stop(&mut stop, Duration::from_millis(500)).await {
            return;
        }
    };
    let mut rx = player.watch();
    let mut current = (rx.borrow_and_update().clone(), Instant::now());
    let mut tick = tokio::time::interval(sh.settings.sync_interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut n: u64 = 0;
    loop {
        let urgent = tokio::select! {
            r = rx.changed() => {
                if r.is_err() {
                    return; // player stopped
                }
                let new = rx.borrow_and_update().clone();
                let urgent = significant_change(&current.0, current.1, &new);
                current = (new, Instant::now());
                if !urgent {
                    continue;
                }
                true
            }
            _ = tick.tick() => false,
            _ = sh.sync_trigger.notified() => true,
            _ = stop.changed() => return,
        };
        if !is_leader(&state) {
            continue;
        }
        n += 1;
        let active = current.0.state != PlayerState::Idle || current.0.blackout;
        // Idle: a slow heartbeat (every 2 s) so followers that just (re)started converge.
        if !urgent && !active && n % 8 != 0 {
            continue;
        }
        let packet = Msg::Sync(build_sync(&state, &sh, &current.0, current.1));
        // Unicast to every adopted follower, MACed with its own key (followers
        // announce their address in beacons every 2 s).
        for (_, addr, key) in follower_targets(&state, &sh) {
            sh.send_json(&packet, Some(&key), &[addr]).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Slice warming
// ---------------------------------------------------------------------------

async fn slice_worker(state: AppState, sh: Arc<Shared>) {
    let mut stop = sh.stop_rx();
    let mut changes = state.store.subscribe();
    let mut failed: std::collections::HashSet<String> = Default::default();
    // Let startup settle first.
    if sleep_or_stop(&mut stop, Duration::from_secs(2)).await {
        return;
    }
    loop {
        if is_leader(&state) {
            let show = state.store.get();
            let data_dir = state.config.data_dir.clone();
            let jobs =
                tokio::task::spawn_blocking(move || super::slices::all_jobs(&show, &data_dir))
                    .await
                    .unwrap_or_default();
            for job in &jobs {
                if *stop.borrow() {
                    return;
                }
                if failed.contains(&job.key) {
                    continue;
                }
                if let Err(e) = sh.slices.ensure(job.clone()).await {
                    failed.insert(job.key.clone());
                    log_warning(
                        &state,
                        format!("Could not prepare sequence data for a controller: {e}"),
                    );
                }
            }
            let cache = sh.slices.clone();
            let removed = tokio::task::spawn_blocking(move || cache.cleanup(&jobs))
                .await
                .unwrap_or(0);
            if removed > 0 {
                tracing::debug!("removed {removed} stale slices");
            }
        }
        tokio::select! {
            r = changes.changed() => if r.is_err() { return },
            _ = stop.changed() => return,
        }
        // Debounce bursts of edits.
        if sleep_or_stop(&mut stop, Duration::from_millis(1500)).await {
            return;
        }
        changes.borrow_and_update();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::manifest::tests::three_node_show;
    use crate::player::ItemRef;

    #[test]
    fn fit_outputs_keeps_existing_settings() {
        let mut outs = BoardKind::Difftx.default_outputs();
        outs[1].brightness = 40;
        let big = fit_outputs(&outs, BoardKind::Difftxlarge);
        assert_eq!(big.len(), 60);
        assert_eq!(big[1].brightness, 40);
        assert_eq!(big[1].label, "J1-2");
        let small = fit_outputs(&big, BoardKind::Diffsmart);
        assert_eq!(small.len(), 4);
        assert_eq!(small[1].label, "Out 2");
        assert_eq!(small[1].brightness, 40);
        assert!(fit_outputs(&outs, BoardKind::Virtual).is_empty());
    }

    #[test]
    fn rename_and_remove_nodes() {
        let mut s = three_node_show();
        rename_node(&mut s, "f1", "new1");
        assert!(s.node("f1").is_none());
        assert!(s.node("new1").is_some());
        assert_eq!(s.props[1].segments[0].node_id, "new1");
        assert_eq!(s.receivers[0].node_id, "new1");
        remove_node(&mut s, "new1");
        assert!(s.node("new1").is_none());
        assert!(s.props[1].segments.is_empty());
        assert_eq!(s.props[2].segments.len(), 1);
        assert!(s.receivers.is_empty());
    }

    #[test]
    fn significant_changes() {
        let at = Instant::now();
        let base = PlayerStatus {
            state: PlayerState::Playing,
            item: Some(ItemRef {
                kind: "sequence".into(),
                id: "s".into(),
                name: "S".into(),
            }),
            pos_ms: 1000,
            duration_ms: 60_000,
            brightness: 100,
            ..Default::default()
        };
        let mut s = base.clone();
        s.pos_ms = 1040; // normal progress
        assert!(!significant_change(&base, at, &s));
        s.pos_ms = 20_000; // seek
        assert!(significant_change(&base, at, &s));
        let mut s = base.clone();
        s.state = PlayerState::Paused;
        assert!(significant_change(&base, at, &s));
        let mut s = base.clone();
        s.brightness = 50;
        assert!(significant_change(&base, at, &s));
        let mut s = base.clone();
        s.item = None;
        assert!(significant_change(&base, at, &s));
    }

    #[test]
    fn position_extrapolates_only_while_playing() {
        let at = Instant::now() - Duration::from_millis(500);
        let mut s = PlayerStatus {
            state: PlayerState::Playing,
            pos_ms: 1000,
            duration_ms: 1200,
            ..Default::default()
        };
        assert_eq!(position_now(&s, at), 1200, "clamped to the duration");
        s.duration_ms = 0;
        assert!(position_now(&s, at) >= 1500);
        s.state = PlayerState::Paused;
        assert_eq!(position_now(&s, at), 1000);
    }
}
