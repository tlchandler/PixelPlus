//! Power (F12, ARCHITECTURE §12.11): CRUD `/power-supplies`, `GET /power/live`
//! and `GET /power/budget` (`/power/estimate` stays in tools; its supply view
//! and limiter simulation come from `pixelplus_core::power`).

use super::crud::{self, Entity};
use super::{ApiError, ApiResult};
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use pixelplus_core::model::{PowerSupply, Show};
use serde::Deserialize;
use serde_json::Value;

impl Entity for PowerSupply {
    const LABEL: &'static str = "Power supply";
    fn id(&self) -> &str {
        &self.id
    }
    fn set_id(&mut self, id: String) {
        self.id = id;
    }
    fn list(show: &Show) -> &Vec<Self> {
        &show.power_supplies
    }
    fn list_mut(show: &mut Show) -> &mut Vec<Self> {
        &mut show.power_supplies
    }
    fn validate(&self, show: &Show) -> ApiResult<()> {
        validate_supply(self, show)
    }
}

/// A supply must be plausible and feed things that exist, each only once.
pub fn validate_supply(s: &PowerSupply, show: &Show) -> ApiResult<()> {
    if s.name.trim().is_empty() {
        return Err(ApiError::bad_request(
            "Please give the power supply a name.",
        ));
    }
    if s.name.chars().count() > 120 {
        return Err(ApiError::bad_request(
            "That name is too long (120 characters max).",
        ));
    }
    if !(s.volts.is_finite() && s.volts > 0.0 && s.volts <= 60.0) {
        return Err(ApiError::bad_request(
            "Enter the supply's voltage (for example 12 or 5).",
        ));
    }
    if !(s.amps.is_finite() && s.amps > 0.0 && s.amps <= 1000.0) {
        return Err(ApiError::bad_request(
            "Enter the supply's current rating in amps (for example 29 for a 350 W 12 V supply).",
        ));
    }
    for rid in &s.receiver_ids {
        if !show.receivers.iter().any(|r| &r.id == rid) {
            return Err(ApiError::bad_request(
                "One of the receivers this supply feeds no longer exists.",
            ));
        }
    }
    for o in &s.direct_outputs {
        let node = show.node(&o.node_id).ok_or_else(|| {
            ApiError::bad_request(
                "One of the outputs points at a controller that no longer exists.",
            )
        })?;
        if o.output == 0 || o.output as usize > node.outputs.len().max(node.board.output_count()) {
            return Err(ApiError::bad_request(format!(
                "{} doesn't have output {}.",
                node.name, o.output
            )));
        }
    }
    // Each output is fed by one supply.
    let mine = pixelplus_core::power::supply_outputs(show, s);
    for other in show.power_supplies.iter().filter(|o| o.id != s.id) {
        let theirs = pixelplus_core::power::supply_outputs(show, other);
        if let Some((node, out)) = mine.iter().find(|m| theirs.contains(m)) {
            let label = show
                .node(node)
                .map(|n| format!("{} {}", n.name, n.board.output_label(*out as usize)))
                .unwrap_or_else(|| format!("output {out}"));
            return Err(ApiError::bad_request(format!(
                "{label} is already fed by “{}”. Each output has one supply.",
                other.name
            )));
        }
    }
    Ok(())
}

/// `GET /power/live`: every node's limiter groups (current, budget, scale).
async fn live(State(state): State<AppState>) -> Json<Value> {
    Json(crate::player::limiter::power_live(&state))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BudgetQuery {
    node_id: Option<String>,
}

/// `GET /power/budget[?nodeId=]`: the limiter budgets computed from the show
/// (what each node's manifest carries).
async fn budget(
    State(state): State<AppState>,
    Query(q): Query<BudgetQuery>,
) -> ApiResult<Json<Value>> {
    let show = state.store.get();
    let all = pixelplus_core::power::show_budgets(&show);
    let v = match q.node_id {
        Some(id) => {
            show.node(&id)
                .ok_or_else(|| ApiError::not_found("That controller"))?;
            serde_json::to_value(all.get(&id)).map_err(ApiError::internal)?
        }
        None => serde_json::to_value(&all).map_err(ApiError::internal)?,
    };
    Ok(Json(v))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(crud::routes::<PowerSupply>("power-supplies"))
        .route("/power/live", get(live))
        .route("/power/budget", get(budget))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixelplus_core::model::*;

    fn show() -> Show {
        let mut s = Show::default();
        s.nodes.push(Node {
            hardware_history: vec![],
            serial: None,
            id: "n1".into(),
            name: "Garage".into(),
            hostname: "pp".into(),
            role: NodeRole::Leader,
            board: BoardKind::Difftx,
            board_rev: None,
            pi_model: None,
            outputs: BoardKind::Difftx.default_outputs(),
            adopted: true,
            last_seen: None,
            notes: None,
        });
        s.receivers.push(Receiver {
            main_fuse_amps: None,
            id: "r1".into(),
            name: "Porch".into(),
            kind: ReceiverKind::Diffrx,
            node_id: "n1".into(),
            jack: 1,
            location: None,
            fuse_amps: None,
            notes: None,
        });
        s
    }

    fn supply(id: &str) -> PowerSupply {
        PowerSupply {
            id: id.into(),
            name: "PSU".into(),
            volts: 12.0,
            amps: 29.0,
            receiver_ids: vec!["r1".into()],
            direct_outputs: vec![],
            sensor: None,
        }
    }

    #[test]
    fn validation() {
        let mut s = show();
        assert!(validate_supply(&supply("a"), &s).is_ok());
        let mut bad = supply("a");
        bad.volts = 0.0;
        assert!(validate_supply(&bad, &s).is_err());
        bad = supply("a");
        bad.amps = f32::NAN;
        assert!(validate_supply(&bad, &s).is_err());
        bad = supply("a");
        bad.receiver_ids = vec!["gone".into()];
        assert!(validate_supply(&bad, &s).is_err());
        bad = supply("a");
        bad.direct_outputs = vec![NodeOutputRef {
            node_id: "n1".into(),
            output: 99,
        }];
        bad.receiver_ids.clear();
        assert!(validate_supply(&bad, &s).is_err());
        // One supply per output.
        s.power_supplies.push(supply("a"));
        let e = validate_supply(&supply("b"), &s).unwrap_err();
        assert!(e.message.contains("already fed"), "{}", e.message);
        assert!(
            validate_supply(&supply("a"), &s).is_ok(),
            "editing itself is fine"
        );
    }
}
