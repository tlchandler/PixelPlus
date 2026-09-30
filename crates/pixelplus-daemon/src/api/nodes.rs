//! Controllers (`/api/v1/nodes*`): the leader and its followers, discovery and
//! adoption, output settings.

use super::cluster::handle;
use super::crud::merge_patch;
use super::{ApiError, ApiResult};
use crate::cluster::leader::{self, AdoptRequest, ReleaseResult};
use crate::cluster::{ClusterCommand, CommandResult, DiscoveredNode, NodeStatus};
use crate::node::LocalRole;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use pixelplus_core::model::{new_id, BoardKind, Node, NodeRole, OutputConfig};
use serde::Deserialize;
use serde_json::{json, Value};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/nodes", get(list).post(create))
        .route("/nodes/discovered", get(discovered))
        .route("/nodes/adopt", post(adopt))
        .route(
            "/nodes/{id}",
            get(get_one).put(update).patch(update).delete(delete),
        )
        .route("/nodes/{id}/outputs/{index}", put(update_output))
        .route("/nodes/{id}/release", post(release))
        .route("/nodes/{id}/resync", post(resync))
        .route("/nodes/{id}/identify", post(identify))
}

/// A node plus its live status (status fields win).
fn with_status(node: &Node, status: Option<&NodeStatus>) -> Value {
    let mut v = serde_json::to_value(node).unwrap_or(Value::Null);
    if let (Value::Object(map), Some(s)) = (&mut v, status) {
        if let Ok(Value::Object(st)) = serde_json::to_value(s) {
            for (k, val) in st {
                if !matches!(
                    k.as_str(),
                    "name" | "role" | "board" | "adopted" | "hostname"
                ) || !map.contains_key(&k)
                {
                    map.insert(k, val);
                }
            }
        }
    }
    v
}

fn statuses(state: &AppState) -> Vec<NodeStatus> {
    state
        .services
        .cluster
        .get()
        .map(|c| c.nodes_status())
        .unwrap_or_default()
}

async fn list(State(state): State<AppState>) -> Json<Vec<Value>> {
    let show = state.store.get();
    let st = statuses(&state);
    Json(
        show.nodes
            .iter()
            .map(|n| with_status(n, st.iter().find(|s| s.id == n.id)))
            .collect(),
    )
}

async fn get_one(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let show = state.store.get();
    let node = show
        .node(&id)
        .ok_or_else(|| ApiError::not_found("That controller"))?;
    let st = statuses(&state);
    Ok(Json(with_status(node, st.iter().find(|s| s.id == id))))
}

async fn discovered(State(state): State<AppState>) -> ApiResult<Json<Vec<DiscoveredNode>>> {
    Ok(Json(handle(&state)?.discovered()))
}

fn require_leader(state: &AppState) -> ApiResult<()> {
    if state.identity().role == LocalRole::Leader {
        Ok(())
    } else {
        Err(ApiError::conflict(
            "Controllers are managed on the show leader.",
        ))
    }
}

async fn adopt(
    State(state): State<AppState>,
    Json(req): Json<AdoptRequest>,
) -> ApiResult<Json<Node>> {
    let cluster = handle(&state)?;
    let node = leader::adopt(&state, &cluster.shared, req).await?;
    Ok(Json(node))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ReleaseBody {
    /// Also remove it (and its wiring) from the show.
    #[serde(default)]
    remove: bool,
}

async fn release(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<ReleaseBody>>,
) -> ApiResult<Json<ReleaseResult>> {
    require_leader(&state)?;
    let cluster = handle(&state)?;
    let remove = body.map(|b| b.0.remove).unwrap_or(false);
    Ok(Json(
        leader::release(&state, &cluster.shared, &id, remove).await?,
    ))
}

async fn resync(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<CommandResult>>> {
    require_leader(&state)?;
    let cluster = handle(&state)?;
    if state.store.get().node(&id).is_none() {
        return Err(ApiError::not_found("That controller"));
    }
    Ok(Json(
        cluster
            .send_command(Some(&id), ClusterCommand::Refresh)
            .await,
    ))
}

// ---------------------------------------------------------------------------
// Create / update / delete
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateBody {
    name: String,
    board: BoardKind,
    #[serde(default)]
    board_rev: Option<String>,
    #[serde(default)]
    notes: Option<String>,
}

/// Create a placeholder follower (plan the wiring before the hardware
/// arrives); adopt the real controller later with `replaces`.
async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateBody>,
) -> ApiResult<Json<Node>> {
    require_leader(&state)?;
    leader::validate_node_name(&body.name)?;
    let node = Node {
        hardware_history: Default::default(),
        serial: Default::default(),
        id: new_id(),
        name: body.name.trim().to_string(),
        hostname: String::new(),
        role: NodeRole::Follower,
        board: body.board,
        board_rev: body.board_rev,
        pi_model: None,
        outputs: body.board.default_outputs(),
        adopted: false,
        last_seen: None,
        notes: body.notes.filter(|n| !n.trim().is_empty()),
    };
    let (node, _) = state
        .store
        .update(move |s| {
            s.nodes.push(node.clone());
            Ok(node)
        })
        .await?;
    Ok(Json(node))
}

/// Validate (and normalise) a node's outputs.
pub fn validate_outputs(node: &mut Node) -> ApiResult<()> {
    let expected = node.board.output_count();
    if node.outputs.len() != expected {
        return Err(ApiError::bad_request(format!(
            "{} has {expected} output{}, not {}.",
            node.board.display_name(),
            if expected == 1 { "" } else { "s" },
            node.outputs.len()
        )));
    }
    for (i, o) in node.outputs.iter_mut().enumerate() {
        validate_output(o, i as u32 + 1, node.board)?;
    }
    Ok(())
}

fn validate_output(o: &mut OutputConfig, index: u32, board: BoardKind) -> ApiResult<()> {
    if o.index != index {
        return Err(ApiError::bad_request(format!(
            "Output {} is out of order (expected {index}).",
            o.index
        )));
    }
    if o.brightness > 100 {
        return Err(ApiError::bad_request("Brightness is a percentage (0–100)."));
    }
    if !o.gamma.is_finite() || !(0.1..=5.0).contains(&o.gamma) {
        return Err(ApiError::bad_request(
            "Gamma must be between 0.1 and 5 (1.0 = none, 2.2 typical).",
        ));
    }
    let label = o.label.trim();
    if label.is_empty() {
        o.label = board.output_label(index as usize);
    } else if label.chars().count() > 40 {
        return Err(ApiError::bad_request(
            "Output labels are limited to 40 characters.",
        ));
    } else {
        o.label = label.to_string();
    }
    Ok(())
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(patch): Json<Value>,
) -> ApiResult<Json<Node>> {
    require_leader(&state)?;
    if !patch.is_object() {
        return Err(ApiError::bad_request(
            "Send the fields to change as a JSON object.",
        ));
    }
    let (node, _) = state
        .store
        .update(|show| {
            let idx = show
                .nodes
                .iter()
                .position(|n| n.id == id)
                .ok_or_else(|| ApiError::not_found("That controller"))?;
            let old = show.nodes[idx].clone();
            let mut value = serde_json::to_value(&old).map_err(ApiError::internal)?;
            merge_patch(&mut value, &patch);
            let mut node: Node = serde_json::from_value(value)
                .map_err(|e| ApiError::bad_request(format!("That change isn't valid: {e}")))?;
            // Facts reported by the hardware (or the cluster) are not editable.
            let placeholder = !old.adopted && old.hostname.is_empty();
            if node.id != old.id
                || node.role != old.role
                || node.adopted != old.adopted
                || node.hostname != old.hostname
                || node.pi_model != old.pi_model
                || (!placeholder && (node.board != old.board || node.board_rev != old.board_rev))
            {
                return Err(ApiError::bad_request(
                    "Only the name, notes and outputs of a controller can be changed.",
                ));
            }
            if node.board != old.board && patch.get("outputs").is_none() {
                node.outputs = leader::fit_outputs(&old.outputs, node.board);
            }
            node.last_seen = old.last_seen.clone();
            node.name = node.name.trim().to_string();
            leader::validate_node_name(&node.name)?;
            if node
                .notes
                .as_deref()
                .is_some_and(|n| n.chars().count() > 2000)
            {
                return Err(ApiError::bad_request(
                    "Notes are limited to 2000 characters.",
                ));
            }
            validate_outputs(&mut node)?;
            // Wiring must still fit the (possibly smaller) board.
            let outputs = node.outputs.len() as u32;
            if show.props.iter().any(|p| {
                p.segments
                    .iter()
                    .any(|s| s.node_id == id && s.output > outputs)
            }) {
                return Err(ApiError::conflict(
                    "Some props are wired to outputs this board does not have; move them first.",
                ));
            }
            show.nodes[idx] = node.clone();
            Ok(node)
        })
        .await?;
    Ok(Json(node))
}

async fn update_output(
    State(state): State<AppState>,
    Path((id, index)): Path<(String, u32)>,
    Json(patch): Json<Value>,
) -> ApiResult<Json<OutputConfig>> {
    require_leader(&state)?;
    if !patch.is_object() {
        return Err(ApiError::bad_request(
            "Send the output settings as a JSON object.",
        ));
    }
    let (out, _) = state
        .store
        .update(|show| {
            let node = show
                .nodes
                .iter_mut()
                .find(|n| n.id == id)
                .ok_or_else(|| ApiError::not_found("That controller"))?;
            let board = node.board;
            let slot = node
                .outputs
                .iter_mut()
                .find(|o| o.index == index)
                .ok_or_else(|| ApiError::not_found(format!("Output {index}")))?;
            let mut value = serde_json::to_value(&*slot).map_err(ApiError::internal)?;
            merge_patch(&mut value, &patch);
            let mut o: OutputConfig = serde_json::from_value(value)
                .map_err(|e| ApiError::bad_request(format!("That change isn't valid: {e}")))?;
            o.index = index;
            validate_output(&mut o, index, board)?;
            *slot = o.clone();
            Ok(o)
        })
        .await?;
    Ok(Json(out))
}

#[derive(Deserialize, Default)]
struct DeleteQuery {
    #[serde(default)]
    force: Option<String>,
}

/// `DELETE /nodes/:id`: release a controller.
///
/// * Adopted followers are told to forget this leader (best effort).
/// * A controller with nothing wired to it is removed from the show.
/// * A wired one stays in the show as "released" (its props keep their wiring
///   and light again when it is adopted again), unless `?force=1`, which
///   removes it together with its wiring (the props themselves stay).
async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<DeleteQuery>,
) -> ApiResult<Json<Value>> {
    require_leader(&state)?;
    let force = q
        .force
        .as_deref()
        .is_some_and(|f| matches!(f, "1" | "true" | "yes"));
    let show = state.store.get();
    let node = show
        .node(&id)
        .ok_or_else(|| ApiError::not_found("That controller"))?;
    if node.role == NodeRole::Leader || id == state.identity().id {
        return Err(ApiError::bad_request("The show leader cannot be removed."));
    }
    let props = show
        .props
        .iter()
        .filter(|p| p.segments.iter().any(|s| s.node_id == id))
        .count();
    let receivers = show.receivers.iter().filter(|r| r.node_id == id).count();
    let mut released = false;
    if node.adopted {
        if let Ok(cluster) = handle(&state) {
            released = leader::call_release(&state, &cluster.shared, &id).await;
        }
    }
    let remove = force || (props == 0 && receivers == 0);
    let id2 = id.clone();
    state
        .store
        .update(move |s| {
            if remove {
                leader::remove_node(s, &id2);
            } else if let Some(n) = s.nodes.iter_mut().find(|n| n.id == id2) {
                n.adopted = false;
            }
            Ok(())
        })
        .await?;
    Ok(Json(json!({
        "ok": true,
        "released": released,
        "removed": remove,
        "props": props,
        "receivers": receivers,
    })))
}

/// `POST /nodes/:id/identify`: blink the controller's outputs for 5 s.
async fn identify(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    require_leader(&state)?;
    let show = state.store.get();
    let node = show
        .node(&id)
        .ok_or_else(|| ApiError::not_found("That controller"))?;
    if id == state.identity().id {
        crate::cluster::identify_local(&state, &id, crate::cluster::IDENTIFY_MS).await?;
        return Ok(Json(json!({ "ok": true })));
    }
    if !node.adopted {
        return Err(ApiError::conflict(format!("{} is not adopted.", node.name)));
    }
    let cluster = handle(&state)?;
    let res = cluster
        .send_command(
            Some(&id),
            ClusterCommand::Identify {
                duration_ms: crate::cluster::IDENTIFY_MS,
            },
        )
        .await;
    match res.into_iter().next() {
        Some(r) if r.ok => Ok(Json(json!({ "ok": true }))),
        Some(r) if r.error.as_deref() == Some("offline") => {
            Err(ApiError::unavailable(format!("{} is offline.", node.name)))
        }
        Some(r) => Err(ApiError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "node_error",
            format!(
                "{}: {}",
                node.name,
                r.error.unwrap_or_else(|| "failed".into())
            ),
        )),
        None => Err(ApiError::unavailable(format!("{} is offline.", node.name))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_validation() {
        let mut n = Node {
            hardware_history: Default::default(),
            serial: Default::default(),
            id: "n".into(),
            name: "N".into(),
            hostname: String::new(),
            role: NodeRole::Follower,
            board: BoardKind::Difftx,
            board_rev: None,
            pi_model: None,
            outputs: BoardKind::Difftx.default_outputs(),
            adopted: false,
            last_seen: None,
            notes: None,
        };
        assert!(validate_outputs(&mut n).is_ok());
        n.outputs[2].label = "  ".into();
        validate_outputs(&mut n).unwrap();
        assert_eq!(n.outputs[2].label, "Port 3");
        n.outputs[1].brightness = 101;
        assert!(validate_outputs(&mut n).is_err());
        n.outputs[1].brightness = 50;
        n.outputs[1].gamma = f32::NAN;
        assert!(validate_outputs(&mut n).is_err());
        n.outputs[1].gamma = 2.2;
        n.outputs.pop();
        assert!(validate_outputs(&mut n).is_err());
    }
}
