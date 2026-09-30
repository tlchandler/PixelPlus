//! The power limiter in the output thread (F12, ARCHITECTURE §12.11).
//!
//! Every output frame, after colour order / brightness / gamma (the wire
//! bytes are what the pixels draw current for), [`EngineLimiter::process`]
//! sums each output's bytes, turns them into amps with the budget's mA per
//! pixel, advances the [`Limiter`] (`pixelplus_core::power`) and, in `limit`
//! mode, scales the outputs it asks for. Cost: one add per byte plus a
//! 256-entry table per scaled output.
//!
//! The budget comes from `power::node_budget` on the leader and from the
//! manifest (`NodeManifest.power`, `ClusterHandle::manifest_power`) on
//! followers ([`budget_for`]). A snapshot
//! for `GET /power/live`, the WS `power` message and the follower's beacon
//! report is kept in a process-wide cell ([`live`], [`follower_report`]).

use crate::cluster::proto::LimiterReport;
use crate::services::journal::Event;
use parking_lot::Mutex;
use pixelplus_core::model::{LimiterMode, NodePowerBudget, PowerGroupKind, Show};
use pixelplus_core::power::{GroupLive, Limiter};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A group must be quiet this long before a limiting episode counts as over.
const EPISODE_GAP_MS: f64 = 2_000.0;
/// Episodes shorter than this are not journaled.
const EPISODE_MIN_S: f64 = 1.0;
/// How often the live snapshot is refreshed.
const SNAPSHOT_EVERY_MS: f64 = 250.0;

/// This node's limiter as `GET /power/live` shows it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct LiveNode {
    pub node_id: String,
    pub mode: LimiterMode,
    pub groups: Vec<GroupLive>,
    pub limiting: bool,
    pub min_scale: f32,
    pub active_groups: Vec<String>,
    pub seconds_limited: f64,
}

struct Shared {
    node: Option<LiveNode>,
    /// Lowest scale since the last follower report.
    period_min: f32,
}

impl Default for Shared {
    fn default() -> Self {
        Shared {
            node: None,
            period_min: 1.0,
        }
    }
}

/// Per node id (one engine per daemon; keyed so tests running several
/// engines in one process don't mix).
static LIVE: Mutex<Option<HashMap<String, Shared>>> = parking_lot::const_mutex(None);

fn with_live<R>(node_id: &str, f: impl FnOnce(&mut Shared) -> R) -> R {
    let mut g = LIVE.lock();
    f(g.get_or_insert_with(HashMap::new)
        .entry(node_id.to_string())
        .or_default())
}

/// Node `node_id`'s limiter right now (`None` while the limiter is off).
pub fn live(node_id: &str) -> Option<LiveNode> {
    with_live(node_id, |l| l.node.clone())
}

/// What a follower reports in its beacon (`FollowerReport.limiter`): the
/// groups scaling now, the lowest scale since the previous report and the
/// seconds spent limiting. `None` while the limiter is off.
pub fn follower_report(node_id: &str) -> Option<LimiterReport> {
    with_live(node_id, |l| {
        let min = std::mem::replace(&mut l.period_min, 1.0);
        let n = l.node.as_ref()?;
        Some(LimiterReport {
            active_groups: n.active_groups.clone(),
            min_scale: (min.min(n.min_scale) * 1000.0).round() / 1000.0,
            seconds_limited: (n.seconds_limited * 10.0).round() as f32 / 10.0,
        })
    })
}

/// The budget this node runs with: the leader computes it from the show
/// (`power::node_budget`); a follower uses the one its leader put in its
/// manifest (`NodeManifest.power`, via [`crate::cluster::ClusterHandle::manifest_power`]).
pub fn budget_for(
    state: &crate::state::AppState,
    show: &Show,
    node_id: &str,
    follower: bool,
) -> Option<NodePowerBudget> {
    if follower {
        state
            .services
            .cluster
            .get()
            .and_then(|c| c.manifest_power())
            .filter(|b| b.mode != LimiterMode::Off)
    } else {
        pixelplus_core::power::node_budget(show, node_id)
    }
}

/// Measured currents (A) of this node's supplies that have a current sensor
/// (`PowerSupply.sensor`, a sensor node's `current` input, F20), keyed by
/// limiter group id. Only supplies whose outputs are all on this node: a
/// supply spanning nodes is split, and one reading can't be.
pub fn measured(state: &crate::state::AppState, show: &Show, node_id: &str) -> Vec<(String, f32)> {
    show.power_supplies
        .iter()
        .filter_map(|s| {
            let sensor = s.sensor.as_ref()?;
            let outs = pixelplus_core::power::supply_outputs(show, s);
            if outs.is_empty() || outs.iter().any(|(n, _)| n != node_id) {
                return None;
            }
            let a = crate::services::sensornodes::amps(state, sensor)?;
            Some((format!("supply:{}", s.id), a as f32))
        })
        .collect()
}

/// Props the active season profile keeps dark (F8). The same on both roles:
/// a follower's local show carries the leader's mask as its active profile.
pub fn disabled_props(show: &Show) -> Vec<String> {
    crate::services::profiles::disabled_prop_ids(show)
}

/// A limiting episode that just ended (for the journal).
#[derive(Debug, Clone, PartialEq)]
pub struct Episode {
    pub group_id: String,
    /// The output of a port group (1-based), else 0.
    pub port: u32,
    pub seconds: f32,
}

impl Episode {
    pub fn event(&self, node_id: &str) -> Event {
        Event::Limiter {
            node_id: node_id.to_string(),
            port: self.port,
            sec: (self.seconds * 10.0).round() / 10.0,
        }
    }
}

/// The output thread's limiter.
#[derive(Default)]
pub struct EngineLimiter {
    node_id: String,
    budget: Option<NodePowerBudget>,
    limiter: Option<Limiter>,
    sums: Vec<u64>,
    amps: Vec<f32>,
    last_ms: Option<f64>,
    /// Group → (first limited, last limited) engine ms.
    episodes: HashMap<String, (f64, f64)>,
    ports: HashMap<String, u32>,
    last_snapshot: f64,
}

impl EngineLimiter {
    /// Use `budget` from now on (unchanged budgets keep their state).
    pub fn set_budget(&mut self, node_id: &str, budget: Option<NodePowerBudget>) {
        if budget == self.budget && node_id == self.node_id {
            return;
        }
        if node_id != self.node_id {
            let old = std::mem::take(&mut self.node_id);
            with_live(&old, |l| *l = Shared::default());
        }
        self.node_id = node_id.to_string();
        self.limiter = budget
            .as_ref()
            .filter(|b| b.mode != LimiterMode::Off)
            .map(Limiter::new);
        self.ports = budget
            .iter()
            .flat_map(|b| &b.groups)
            .filter(|g| g.kind == PowerGroupKind::Port)
            .map(|g| (g.id.clone(), g.members.first().copied().unwrap_or(0)))
            .collect();
        self.budget = budget;
        self.episodes.clear();
        self.last_ms = None;
        with_live(node_id, |l| *l = Shared::default());
    }

    /// Measure (and in `limit` mode scale) one frame of wire bytes. Returns
    /// limiting episodes that ended (to journal).
    pub fn process(
        &mut self,
        wire: &mut pixelplus_output::OutputFrame,
        now_ms: f64,
    ) -> Vec<Episode> {
        let Some(lim) = self.limiter.as_mut() else {
            return vec![];
        };
        let dt = self.last_ms.map_or(0.0, |t| (now_ms - t).max(0.0)) as f32;
        self.last_ms = Some(now_ms);
        wire.byte_sums(&mut self.sums);
        let n = lim.outputs();
        self.amps.clear();
        self.amps
            .extend((0..n).map(|i| lim.amps_for(i, self.sums.get(i).copied().unwrap_or(0))));
        lim.step(&self.amps, dt);
        if lim.mode() == LimiterMode::Limit {
            for i in 0..n.min(wire.outputs.len()) {
                let s = lim.scale(i);
                if s < 1.0 {
                    wire.scale_output(i, s);
                }
            }
        }
        // Episodes (journal) and the live snapshot.
        let mut ended = Vec::new();
        let groups = lim.groups();
        for g in &groups {
            if g.scale < 0.995 {
                self.episodes
                    .entry(g.id.clone())
                    .and_modify(|e| e.1 = now_ms)
                    .or_insert((now_ms, now_ms));
            }
        }
        self.episodes.retain(|id, (from, to)| {
            if now_ms - *to <= EPISODE_GAP_MS {
                return true;
            }
            let secs = (*to - *from) / 1000.0;
            if secs >= EPISODE_MIN_S {
                ended.push(Episode {
                    group_id: id.clone(),
                    port: self.ports.get(id).copied().unwrap_or(0),
                    seconds: secs as f32,
                });
            }
            false
        });
        let min_scale = lim.min_scale();
        let snapshot = now_ms - self.last_snapshot >= SNAPSHOT_EVERY_MS
            || now_ms < self.last_snapshot
            || with_live(&self.node_id, |l| l.node.is_none());
        with_live(&self.node_id, |l| {
            l.period_min = l.period_min.min(min_scale)
        });
        if snapshot {
            self.last_snapshot = now_ms;
            let node = Some(LiveNode {
                node_id: self.node_id.clone(),
                mode: lim.mode(),
                groups: groups
                    .into_iter()
                    .map(|g| GroupLive {
                        amps: (g.amps * 100.0).round() / 100.0,
                        budget: (g.budget * 100.0).round() / 100.0,
                        scale: (g.scale * 1000.0).round() / 1000.0,
                        id: g.id,
                    })
                    .collect(),
                limiting: lim.limiting(),
                min_scale: (min_scale * 1000.0).round() / 1000.0,
                active_groups: lim.active_groups(),
                seconds_limited: lim.seconds_limited,
            });
            with_live(&self.node_id, |l| l.node = node);
        }
        ended
    }

    /// Measured current of a group (F20 sensor on a supply): corrects the
    /// estimate slowly (see `power::Limiter::feedback`).
    pub fn feedback(&mut self, group_id: &str, amps: f32) {
        if let Some(l) = self.limiter.as_mut() {
            l.feedback(group_id, amps);
        }
    }

    /// `PlayerStatus.power` (None while the limiter is off).
    pub fn status(&self) -> Option<super::PowerStatus> {
        let lim = self.limiter.as_ref()?;
        Some(super::PowerStatus {
            limiting: lim.limiting(),
            min_scale: (lim.min_scale() * 100.0).round() / 100.0,
        })
    }
}

/// The `GET /power/live` / WS `power` body: this node's groups plus, on the
/// leader, each follower's budget groups with its reported scale (followers
/// report which groups limit and their lowest scale, not currents; `amps`
/// is then `null`).
pub fn power_live(state: &crate::state::AppState) -> serde_json::Value {
    let show = state.store.get();
    let me = state.identity();
    let mut nodes = Vec::new();
    let own = live(&me.id);
    let own_groups = match &own {
        Some(n) => serde_json::to_value(&n.groups).unwrap_or_default(),
        None => serde_json::json!([]),
    };
    nodes.push(serde_json::json!({
        "nodeId": me.id,
        "mode": own.as_ref().map_or(LimiterMode::Off, |n| n.mode),
        "limiting": own.as_ref().is_some_and(|n| n.limiting),
        "minScale": own.as_ref().map_or(1.0, |n| n.min_scale),
        "groups": own_groups,
    }));
    if me.role != crate::node::LocalRole::Follower {
        let budgets = pixelplus_core::power::show_budgets(&show);
        let reports: HashMap<String, (bool, Option<LimiterReport>)> = state
            .services
            .cluster
            .get()
            .map(|c| c.nodes_status())
            .unwrap_or_default()
            .into_iter()
            .map(|n| (n.id, (n.online, n.limiter)))
            .collect();
        for (node_id, b) in budgets {
            if node_id == me.id {
                continue;
            }
            let (online, rep) = reports.get(&node_id).cloned().unwrap_or((false, None));
            let active: Vec<String> = rep
                .as_ref()
                .map(|r| r.active_groups.clone())
                .unwrap_or_default();
            let min = rep.as_ref().map_or(1.0, |r| r.min_scale);
            let groups: Vec<serde_json::Value> = b
                .groups
                .iter()
                .map(|g| {
                    serde_json::json!({
                        "id": g.id,
                        "amps": serde_json::Value::Null,
                        "budget": g.budget_a,
                        "scale": if active.contains(&g.id) { min } else { 1.0 },
                    })
                })
                .collect();
            nodes.push(serde_json::json!({
                "nodeId": node_id,
                "mode": b.mode,
                "online": online,
                "limiting": !active.is_empty(),
                "minScale": min,
                "secondsLimited": rep.as_ref().map_or(0.0, |r| r.seconds_limited),
                "groups": groups,
            }));
        }
    }
    serde_json::json!({ "nodes": nodes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixelplus_core::model::PowerGroup;
    use pixelplus_output::OutputFrame;

    fn budget(mode: LimiterMode) -> NodePowerBudget {
        NodePowerBudget {
            mode,
            safety: 0.9,
            groups: vec![
                PowerGroup {
                    id: "supply:s".into(),
                    kind: PowerGroupKind::Supply,
                    budget_a: 3.0,
                    tau_ms: 0,
                    members: vec![1],
                },
                PowerGroup {
                    id: "port:r:2".into(),
                    kind: PowerGroupKind::Port,
                    budget_a: 100.0,
                    tau_ms: 8000,
                    members: vec![2],
                },
            ],
            ma_pp: [(1, 60.0), (2, 60.0)].into_iter().collect(),
        }
    }

    #[test]
    fn scales_wire_bytes_in_limit_mode_only() {
        let mut l = EngineLimiter::default();
        l.set_budget("lim-a", Some(budget(LimiterMode::Limit)));
        // 100 white pixels = 6 A on output 1 against a 3 A supply.
        let mut f = OutputFrame {
            outputs: vec![vec![255; 300], vec![255; 30]],
        };
        l.process(&mut f, 0.0);
        assert!(
            f.outputs[0].iter().all(|&b| b == 128),
            "{}",
            f.outputs[0][0]
        );
        assert!(f.outputs[1].iter().all(|&b| b == 255));
        let st = l.status().unwrap();
        assert!(st.limiting && (st.min_scale - 0.5).abs() < 0.01);
        let live_now = live("lim-a").unwrap();
        assert_eq!(live_now.node_id, "lim-a");
        assert_eq!(live_now.active_groups, vec!["supply:s".to_string()]);
        let rep = follower_report("lim-a").unwrap();
        assert!((rep.min_scale - 0.5).abs() < 0.01);

        l.set_budget("lim-a", Some(budget(LimiterMode::Warn)));
        let mut f = OutputFrame {
            outputs: vec![vec![255; 300]],
        };
        l.process(&mut f, 0.0);
        assert!(f.outputs[0].iter().all(|&b| b == 255), "warn never scales");
        assert!(l.status().unwrap().limiting, "…but reports");

        l.set_budget("lim-a", None);
        assert!(l.status().is_none() && live("lim-a").is_none());
        assert!(follower_report("lim-a").is_none());
        let mut f = OutputFrame {
            outputs: vec![vec![255; 300]],
        };
        assert!(l.process(&mut f, 0.0).is_empty());
    }

    #[test]
    fn episodes_are_reported_when_they_end() {
        let mut l = EngineLimiter::default();
        l.set_budget("lim-b", Some(budget(LimiterMode::Limit)));
        let mut t = 0.0;
        let mut ended = vec![];
        for _ in 0..(3 * 40) {
            let mut f = OutputFrame {
                outputs: vec![vec![255; 300]],
            };
            ended.extend(l.process(&mut f, t));
            t += 25.0;
        }
        assert!(ended.is_empty(), "still limiting");
        for _ in 0..(3 * 40) {
            let mut f = OutputFrame {
                outputs: vec![vec![0; 300]],
            };
            ended.extend(l.process(&mut f, t));
            t += 25.0;
        }
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].group_id, "supply:s");
        assert!((ended[0].seconds - 3.0).abs() < 0.1, "{}", ended[0].seconds);
        match ended[0].event("n1") {
            Event::Limiter { node_id, port, sec } => {
                assert_eq!((node_id.as_str(), port), ("n1", 0));
                assert!(sec > 2.8);
            }
            e => panic!("{e:?}"),
        }
        l.set_budget("lim-b", None);
    }
}
