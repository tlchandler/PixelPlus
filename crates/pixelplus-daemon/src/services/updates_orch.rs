//! Signed over-the-air updates across the cluster (F15, ARCHITECTURE §12.13):
//! stage → commit → verify → roll back.
//!
//! The leader orchestrates ("Update everything"): it downloads the signed
//! packages for every architecture in the show (followers often have no
//! internet), serves them to its followers (`GET /cluster/update/:file`,
//! signed), has every node **stage** its package through the root helper
//! (download, size / SHA-256 / minisign checks, a copy of the current version
//! kept for rollback), then **commits** the followers in parallel and itself
//! last. Every node gates its own install (the helper rolls back unless the
//! new daemon is healthy within 3 minutes); if any node fails, the leader
//! rolls **all** nodes back to the version they had.
//!
//! The leader's own commit restarts it: the job is persisted
//! (`updates/job.json`) and resumed by the new daemon ([`start`]).
//!
//! The steps are generic over a [`Fleet`] (the real cluster, or a fake one in
//! the tests), so the state machine is tested without packages or helpers.

use super::platform::{self, ExtVerb, HelperOpts, HelperState, HelperVerb};
use super::updates::{self, ReleaseIndex};
use crate::api::{ApiError, ApiResult};
use crate::cluster::proto::UpdateReport;
use crate::cluster::ClusterCommand;
use crate::node::LocalRole;
use crate::state::AppState;
use parking_lot::Mutex;
use pixelplus_core::model::{AutoUpdate, NodeRole, Schedule, UpdateChannel, UpdateSettings};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// How often an automatic check may hit the release index.
const INDEX_TTL: Duration = Duration::from_secs(6 * 3600);
const HISTORY_MAX: usize = 50;

// ---------------------------------------------------------------------------
// Job model (persisted, and shown by `GET /system/update` as `run`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    /// The leader and every adopted follower.
    Cluster,
    /// Only this controller.
    This,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum JobKind {
    Update,
    Rollback,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Staging,
    CommittingFollowers,
    CommittingLeader,
    RollingBack,
    Done,
    RolledBack,
    Failed,
}

impl Phase {
    pub fn finished(self) -> bool {
        matches!(self, Phase::Done | Phase::RolledBack | Phase::Failed)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum NodePhase {
    Pending,
    Staging,
    Staged,
    Committing,
    Healthy,
    Failed,
    RollingBack,
    RolledBack,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobNode {
    pub id: String,
    pub name: String,
    /// The node running this job (the leader).
    pub is_self: bool,
    pub arch: String,
    /// Version before the job.
    pub from: String,
    pub phase: NodePhase,
    /// It was committed (and so needs rolling back if the job fails).
    #[serde(default)]
    pub committed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateJob {
    pub id: String,
    pub kind: JobKind,
    pub scope: Scope,
    /// Target version (a rollback: the version going back to).
    pub version: String,
    /// The leader's version before the job.
    pub from: String,
    pub phase: Phase,
    pub nodes: Vec<JobNode>,
    /// RFC 3339.
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Started by the automatic updater.
    #[serde(default)]
    pub auto: bool,
    /// Snapshot taken before the update (restored if a rollback needs it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
}

impl UpdateJob {
    fn node_mut(&mut self, id: &str) -> Option<&mut JobNode> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub at: String,
    pub from: String,
    pub to: String,
    pub ok: bool,
    pub scope: Scope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Controllers involved.
    #[serde(default)]
    pub nodes: usize,
}

// ---------------------------------------------------------------------------
// The fleet: what the state machine needs from the cluster
// ---------------------------------------------------------------------------

/// A controller as the updater sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct FleetNode {
    pub id: String,
    pub name: String,
    pub is_self: bool,
    pub online: bool,
    pub version: String,
    pub arch: String,
    pub can_apply: bool,
    pub disk_free_mb: Option<u64>,
    /// Its update phase (`idle`, `staging`, `staged`, `failed`, …).
    pub phase: String,
    pub message: Option<String>,
}

/// The release being installed.
#[derive(Debug, Clone)]
pub struct Target {
    pub version: String,
    /// Package path per architecture (leader side).
    pub packages: HashMap<String, PathBuf>,
}

/// How long each step may take (production: minutes; tests: milliseconds).
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub stage: Duration,
    pub commit: Duration,
    pub poll: Duration,
}

impl Timing {
    pub fn production() -> Self {
        Timing {
            stage: Duration::from_secs(20 * 60),
            // The helper's health gate allows 180 s after the restart.
            commit: Duration::from_secs(8 * 60),
            poll: Duration::from_secs(2),
        }
    }
}

/// Operations on the controllers (the real cluster: [`ClusterFleet`]).
pub trait Fleet: Send + Sync {
    fn nodes(&self) -> Vec<FleetNode>;
    /// Stage `target` on `node`; resolves when it is staged (or failed).
    fn stage(
        &self,
        node: &FleetNode,
        target: &Target,
    ) -> impl Future<Output = Result<(), String>> + Send;
    /// Start installing the staged `version` on `node`.
    fn commit(
        &self,
        node: &FleetNode,
        version: &str,
    ) -> impl Future<Output = Result<(), String>> + Send;
    /// Start going back to the previous version on `node`.
    fn rollback(&self, node: &FleetNode) -> impl Future<Output = Result<(), String>> + Send;
    /// Persist and publish the job (called at every state change).
    fn save(&self, job: &UpdateJob);
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Wait until every node in `ids` runs `version` (online, not failed), or
/// fails / times out. Returns the ids that failed, with a reason.
async fn wait_versions<F: Fleet>(
    fleet: &F,
    ids: &[String],
    version: &str,
    timing: Timing,
) -> Vec<(String, String)> {
    let deadline = Instant::now() + timing.commit;
    let mut pending: Vec<String> = ids.to_vec();
    let mut failed = Vec::new();
    loop {
        let nodes = fleet.nodes();
        pending.retain(|id| {
            let Some(n) = nodes.iter().find(|n| &n.id == id) else {
                return true;
            };
            if n.online && n.version == version && n.phase != "failed" {
                return false;
            }
            if n.online && n.phase == "failed" {
                failed.push((
                    id.clone(),
                    n.message
                        .clone()
                        .unwrap_or_else(|| "the update failed and was undone".into()),
                ));
                return false;
            }
            true
        });
        if pending.is_empty() {
            return failed;
        }
        if Instant::now() >= deadline {
            let nodes = fleet.nodes();
            for id in pending {
                let why = match nodes.iter().find(|n| n.id == id) {
                    Some(n) if !n.online => "didn't come back online in time".to_string(),
                    Some(n) => format!("still runs {} (the update didn't take)", n.version),
                    None => "disappeared".to_string(),
                };
                failed.push((id, why));
            }
            return failed;
        }
        tokio::time::sleep(timing.poll).await;
    }
}

/// Run an update job to its end (or to the leader's own commit, which
/// restarts this daemon; [`resume`] continues from there).
pub async fn run_job<F: Fleet>(
    fleet: &F,
    mut job: UpdateJob,
    target: &Target,
    timing: Timing,
) -> UpdateJob {
    // 1. Stage everywhere (nothing is installed yet: failures just stop).
    job.phase = Phase::Staging;
    for n in &mut job.nodes {
        n.phase = NodePhase::Staging;
    }
    fleet.save(&job);
    let nodes = fleet.nodes();
    let stages = job.nodes.iter().map(|jn| {
        let node = nodes.iter().find(|n| n.id == jn.id).cloned();
        async move {
            match node {
                Some(n) => (jn.id.clone(), fleet.stage(&n, target).await),
                None => (jn.id.clone(), Err("not found".to_string())),
            }
        }
    });
    let results = futures::future::join_all(stages).await;
    let mut stage_failed = false;
    for (id, r) in results {
        let n = job.node_mut(&id).expect("job node");
        match r {
            Ok(()) => n.phase = NodePhase::Staged,
            Err(e) => {
                n.phase = NodePhase::Failed;
                n.message = Some(e);
                stage_failed = true;
            }
        }
    }
    if stage_failed {
        return finish(
            fleet,
            job,
            Phase::Failed,
            Some("Preparing the update failed; nothing was installed.".into()),
        );
    }
    fleet.save(&job);

    // 2. Followers first, in parallel.
    let followers: Vec<String> = job
        .nodes
        .iter()
        .filter(|n| !n.is_self)
        .map(|n| n.id.clone())
        .collect();
    if !followers.is_empty() {
        job.phase = Phase::CommittingFollowers;
        for n in job.nodes.iter_mut().filter(|n| !n.is_self) {
            n.phase = NodePhase::Committing;
            n.committed = true;
        }
        fleet.save(&job);
        let nodes = fleet.nodes();
        let commits = followers.iter().map(|id| {
            let node = nodes.iter().find(|n| &n.id == id).cloned();
            let version = job.version.clone();
            async move {
                match node {
                    Some(n) => (id.clone(), fleet.commit(&n, &version).await),
                    None => (id.clone(), Err("not found".to_string())),
                }
            }
        });
        let mut bad: Vec<(String, String)> = futures::future::join_all(commits)
            .await
            .into_iter()
            .filter_map(|(id, r)| r.err().map(|e| (id, e)))
            .collect();
        let started: Vec<String> = followers
            .iter()
            .filter(|id| !bad.iter().any(|(b, _)| b == *id))
            .cloned()
            .collect();
        bad.extend(wait_versions(fleet, &started, &job.version, timing).await);
        for id in &started {
            if !bad.iter().any(|(b, _)| b == id) {
                job.node_mut(id).expect("job node").phase = NodePhase::Healthy;
            }
        }
        if !bad.is_empty() {
            for (id, why) in &bad {
                let n = job.node_mut(id).expect("job node");
                n.phase = NodePhase::Failed;
                n.message = Some(why.clone());
            }
            let names = names_of(&job, &bad);
            return rollback_all(
                fleet,
                job,
                timing,
                format!("The update failed on {names}; every controller was put back."),
            )
            .await;
        }
        fleet.save(&job);
    }

    // 3. The leader (this node) last.
    if let Some(me) = job.nodes.iter().find(|n| n.is_self).map(|n| n.id.clone()) {
        job.phase = Phase::CommittingLeader;
        {
            let n = job.node_mut(&me).expect("job node");
            n.phase = NodePhase::Committing;
            n.committed = true;
        }
        fleet.save(&job);
        let node = fleet.nodes().into_iter().find(|n| n.id == me);
        let r = match node {
            Some(n) => fleet.commit(&n, &job.version).await,
            None => Err("not found".into()),
        };
        // In production the daemon restarts inside that commit; `resume`
        // takes over. Otherwise (tests, or an install that failed before
        // restarting) check here.
        let bad = match r {
            Err(e) => vec![(me.clone(), e)],
            Ok(()) => wait_versions(fleet, std::slice::from_ref(&me), &job.version, timing).await,
        };
        if let Some((_, why)) = bad.first() {
            let n = job.node_mut(&me).expect("job node");
            n.phase = NodePhase::Failed;
            n.message = Some(why.clone());
            return rollback_all(
                fleet,
                job,
                timing,
                "The update failed on the show leader; every controller was put back.".into(),
            )
            .await;
        }
        job.node_mut(&me).expect("job node").phase = NodePhase::Healthy;
    }
    verify_all(fleet, job, timing).await
}

/// Everything committed: all nodes must run the new version.
async fn verify_all<F: Fleet>(fleet: &F, mut job: UpdateJob, timing: Timing) -> UpdateJob {
    let ids: Vec<String> = job.nodes.iter().map(|n| n.id.clone()).collect();
    let bad = wait_versions(fleet, &ids, &job.version, timing).await;
    if bad.is_empty() {
        for n in &mut job.nodes {
            n.phase = NodePhase::Healthy;
        }
        let msg = format!(
            "{} → {} on {} controller{}.",
            job.from,
            job.version,
            job.nodes.len(),
            if job.nodes.len() == 1 { "" } else { "s" }
        );
        return finish(fleet, job, Phase::Done, Some(msg));
    }
    for (id, why) in &bad {
        let n = job.node_mut(id).expect("job node");
        n.phase = NodePhase::Failed;
        n.message = Some(why.clone());
    }
    let names = names_of(&job, &bad);
    rollback_all(
        fleet,
        job,
        timing,
        format!("{names} failed after the update; every controller was put back."),
    )
    .await
}

fn names_of(job: &UpdateJob, bad: &[(String, String)]) -> String {
    bad.iter()
        .map(|(id, _)| {
            job.nodes
                .iter()
                .find(|n| &n.id == id)
                .map(|n| n.name.clone())
                .unwrap_or_else(|| id.clone())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Put every committed node back to its `from` version: followers first,
/// then the leader (which restarts this daemon).
async fn rollback_all<F: Fleet>(
    fleet: &F,
    mut job: UpdateJob,
    timing: Timing,
    message: String,
) -> UpdateJob {
    job.phase = Phase::RollingBack;
    job.message = Some(message.clone());
    fleet.save(&job);
    let nodes = fleet.nodes();
    // Nodes that run the new version, or might (committed, whatever they report).
    let todo: Vec<JobNode> = job
        .nodes
        .iter()
        .filter(|n| n.committed)
        .filter(|n| {
            nodes
                .iter()
                .find(|x| x.id == n.id)
                .map_or(true, |x| x.version != n.from || !x.online)
        })
        .cloned()
        .collect();
    let mut problems = Vec::new();
    for pass in [false, true] {
        // pass 0: followers (parallel), pass 1: the leader.
        let batch: Vec<&JobNode> = todo.iter().filter(|n| n.is_self == pass).collect();
        if batch.is_empty() {
            continue;
        }
        for n in &batch {
            job.node_mut(&n.id).expect("job node").phase = NodePhase::RollingBack;
        }
        fleet.save(&job);
        let nodes = fleet.nodes();
        let calls = batch.iter().map(|jn| {
            let node = nodes.iter().find(|n| n.id == jn.id).cloned();
            async move {
                match node {
                    Some(n) if n.online => (jn.id.clone(), fleet.rollback(&n).await),
                    _ => (jn.id.clone(), Err("offline".to_string())),
                }
            }
        });
        let results = futures::future::join_all(calls).await;
        let mut started: Vec<(String, String)> = Vec::new();
        for (id, r) in results {
            match r {
                Ok(()) => started.push((
                    id.clone(),
                    job.node_mut(&id).expect("job node").from.clone(),
                )),
                Err(e) => problems.push((id, e)),
            }
        }
        // Each node goes back to its own `from`.
        let mut by_version: HashMap<String, Vec<String>> = HashMap::new();
        for (id, from) in started {
            by_version.entry(from).or_default().push(id);
        }
        for (from, ids) in by_version {
            for (id, why) in wait_versions(fleet, &ids, &from, timing).await {
                problems.push((id, why));
            }
            for id in ids {
                if !problems.iter().any(|(p, _)| *p == id) {
                    job.node_mut(&id).expect("job node").phase = NodePhase::RolledBack;
                }
            }
        }
    }
    if problems.is_empty() {
        finish(fleet, job, Phase::RolledBack, Some(message))
    } else {
        let names = names_of(&job, &problems);
        for (id, why) in &problems {
            let n = job.node_mut(id).expect("job node");
            n.phase = NodePhase::Failed;
            n.message = Some(why.clone());
        }
        finish(
            fleet,
            job,
            Phase::Failed,
            Some(format!(
                "{message} Going back failed on {names}: check them."
            )),
        )
    }
}

fn finish<F: Fleet>(
    fleet: &F,
    mut job: UpdateJob,
    phase: Phase,
    message: Option<String>,
) -> UpdateJob {
    job.phase = phase;
    job.message = message;
    job.finished_at = Some(now_rfc3339());
    fleet.save(&job);
    job
}

/// Continue a job after this daemon restarted (its own commit or rollback).
pub async fn resume<F: Fleet>(fleet: &F, mut job: UpdateJob, timing: Timing) -> UpdateJob {
    let me = job.nodes.iter().find(|n| n.is_self).cloned();
    match job.phase {
        Phase::CommittingLeader => {
            let Some(me) = me else {
                return verify_all(fleet, job, timing).await;
            };
            if fleet
                .nodes()
                .iter()
                .any(|n| n.is_self && n.version == job.version)
            {
                job.node_mut(&me.id).expect("job node").phase = NodePhase::Healthy;
                verify_all(fleet, job, timing).await
            } else {
                // The helper put the leader back (its health gate failed).
                let n = job.node_mut(&me.id).expect("job node");
                n.phase = NodePhase::RolledBack;
                n.committed = false;
                n.message = Some("the new version didn't start properly and was undone".into());
                rollback_all(
                    fleet,
                    job,
                    timing,
                    "The update failed on the show leader; every controller was put back.".into(),
                )
                .await
            }
        }
        Phase::RollingBack => {
            // The leader was the last to go back: done if it runs `from` now.
            let msg = job.message.clone().unwrap_or_default();
            if let Some(me) = me {
                let back = fleet
                    .nodes()
                    .iter()
                    .any(|n| n.is_self && n.version == me.from);
                let n = job.node_mut(&me.id).expect("job node");
                if back {
                    n.phase = NodePhase::RolledBack;
                    n.committed = false;
                }
            }
            rollback_all(fleet, job, timing, msg).await
        }
        Phase::Staging => finish(
            fleet,
            job,
            Phase::Failed,
            Some("The update was interrupted while preparing; nothing was installed.".into()),
        ),
        Phase::CommittingFollowers => {
            rollback_all(
                fleet,
                job,
                timing,
                "The update was interrupted; every controller was put back.".into(),
            )
            .await
        }
        _ => job,
    }
}

// ---------------------------------------------------------------------------
// Preflight and the update window
// ---------------------------------------------------------------------------

/// What would block an update right now (empty: go).
pub fn preflight(
    nodes: &[FleetNode],
    target_version: &str,
    archs: &[String],
    package_mb: u64,
    playing: bool,
    show_soon: Option<String>,
) -> Vec<String> {
    let mut out = Vec::new();
    if playing {
        out.push("The show is playing. Update when it's quiet.".into());
    }
    if let Some(s) = show_soon {
        out.push(s);
    }
    for n in nodes {
        if !n.online {
            out.push(format!("{} is offline.", n.name));
            continue;
        }
        if !n.can_apply {
            out.push(format!(
                "{} can't install updates by itself (not a PixelPlus Pi image or package).",
                n.name
            ));
        }
        if n.version != target_version && !archs.contains(&n.arch) {
            out.push(format!(
                "There's no {} package of {target_version} for {}.",
                n.arch, n.name
            ));
        }
        if let Some(free) = n.disk_free_mb {
            let need = package_mb * 3 + 256;
            if free < need {
                out.push(format!(
                    "{} has only {free} MB free (the update needs about {need} MB).",
                    n.name
                ));
            }
        }
        if n.phase == "staging" || n.phase == "committing" || n.phase == "rollingBack" {
            out.push(format!("{} is busy with an update already.", n.name));
        }
    }
    out
}

fn parse_hm(s: &str) -> Option<chrono::NaiveTime> {
    pixelplus_core::schedule::parse_clock(s)
}

/// Why an automatic install can't run at `now` (show time zone), `None` = go.
pub fn window_problem(
    settings: &UpdateSettings,
    schedule: &Schedule,
    now: chrono::DateTime<chrono_tz::Tz>,
) -> Option<String> {
    use chrono::Datelike;
    let (Some(from), Some(to)) = (
        parse_hm(&settings.window.from),
        parse_hm(&settings.window.to),
    ) else {
        return Some("The update window isn't set.".into());
    };
    let t = now.time();
    let inside = if from <= to {
        t >= from && t < to
    } else {
        t >= from || t < to // wraps midnight
    };
    if !inside {
        return Some(format!(
            "Outside the update window ({}–{}).",
            settings.window.from, settings.window.to
        ));
    }
    if !settings.window.days.is_empty() {
        let today = pixelplus_core::schedule::weekday_of(now.date_naive());
        if !settings.window.days.contains(&today) {
            return Some(format!("Not an update day ({:?}).", now.weekday()));
        }
    }
    show_soon(schedule, settings.avoid_show_hours, now)
}

/// A show window running now or starting within `hours`.
pub fn show_soon(
    schedule: &Schedule,
    hours: u32,
    now: chrono::DateTime<chrono_tz::Tz>,
) -> Option<String> {
    use pixelplus_core::schedule;
    if let Some(o) = schedule::active_at(schedule, now) {
        return Some(format!("The show “{}” is on now.", o.name));
    }
    let horizon = now + chrono::Duration::hours(hours.max(1) as i64);
    schedule::next_show(schedule, now)
        .filter(|o| o.start <= horizon)
        .map(|o| {
            format!(
                "The show “{}” starts at {} (no updates within {} h of a show).",
                o.name,
                o.start.format("%H:%M"),
                hours.max(1)
            )
        })
}

// ---------------------------------------------------------------------------
// Runtime state
// ---------------------------------------------------------------------------

/// This node's own update state (what followers report to the leader).
#[derive(Debug, Clone, Default)]
struct LocalUpdate {
    phase: String,
    version: Option<String>,
    message: Option<String>,
}

/// Runtime state of this service (`state.services.updates_orch`).
#[derive(Default)]
pub struct UpdatesOrchState {
    job: Mutex<Option<UpdateJob>>,
    running: AtomicBool,
    local: Mutex<LocalUpdate>,
    /// Last index check: (when, channel, result).
    index: Mutex<Option<(Instant, UpdateChannel, Result<ReleaseIndex, String>)>>,
    report_cache: Mutex<Option<(Instant, u64, bool)>>,
}

fn updates_dir(state: &AppState) -> PathBuf {
    state.config.data_dir.join("updates")
}

fn read_json<T: serde::de::DeserializeOwned>(p: &Path) -> Option<T> {
    std::fs::read(p)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
}

fn write_json<T: Serialize>(p: &Path, v: &T) {
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = p.with_extension("tmp");
    let res = serde_json::to_vec_pretty(v)
        .map_err(std::io::Error::other)
        .and_then(|b| std::fs::write(&tmp, b))
        .and_then(|_| std::fs::rename(&tmp, p));
    if let Err(e) = res {
        tracing::warn!("couldn't save {}: {e}", p.display());
    }
}

/// Past updates, newest first.
pub fn history(state: &AppState) -> Vec<HistoryEntry> {
    read_json(&updates_dir(state).join("history.json")).unwrap_or_default()
}

fn push_history(state: &AppState, e: HistoryEntry) {
    let mut h = history(state);
    h.insert(0, e);
    h.truncate(HISTORY_MAX);
    write_json(&updates_dir(state).join("history.json"), &h);
}

/// The current (or last) job.
pub fn current_job(state: &AppState) -> Option<UpdateJob> {
    state.services.updates_orch.job.lock().clone()
}

/// Whether the updater is busy (a job runs).
pub fn busy(state: &AppState) -> bool {
    state.services.updates_orch.running.load(Ordering::SeqCst)
}

fn set_local(state: &AppState, phase: &str, version: Option<String>, message: Option<String>) {
    let mut l = state.services.updates_orch.local.lock();
    l.phase = phase.to_string();
    l.version = version;
    l.message = message;
}

/// This node's update state for its beacon report (and the leader's own row).
pub fn local_report(state: &AppState) -> UpdateReport {
    let orch = &state.services.updates_orch;
    let (free_mb, can_apply) = {
        let mut c = orch.report_cache.lock();
        match *c {
            Some((at, f, a)) if at.elapsed() < Duration::from_secs(30) => (f, a),
            _ => {
                let f = super::system::disk_space(&state.config.data_dir)
                    .map(|(f, _)| f / 1_000_000)
                    .unwrap_or(0);
                let a = can_apply_here();
                *c = Some((Instant::now(), f, a));
                (f, a)
            }
        }
    };
    let l = orch.local.lock().clone();
    UpdateReport {
        arch: updates::deb_arch().to_string(),
        can_apply,
        disk_free_mb: free_mb,
        phase: if l.phase.is_empty() {
            "idle".into()
        } else {
            l.phase
        },
        version: l.version,
        message: l.message,
    }
}

/// This machine installs packages through the packaged root helper.
fn can_apply_here() -> bool {
    platform::helper_installed() && !super::system::in_docker()
}

// ---------------------------------------------------------------------------
// Helper verbs (packaging/bin/pixelplus-helper)
// ---------------------------------------------------------------------------

fn verb_stage(version: &str) -> HelperVerb {
    HelperVerb::Ext(ExtVerb::new(
        "update-stage",
        Some(version),
        format!("Preparing PixelPlus {version}"),
        Duration::from_secs(20 * 60),
        "install updates",
    ))
}

fn verb_commit(version: &str) -> HelperVerb {
    HelperVerb::Ext(ExtVerb::new(
        "update-commit",
        Some(version),
        format!("Installing PixelPlus {version}"),
        Duration::from_secs(10 * 60),
        "install updates",
    ))
}

fn verb_rollback() -> HelperVerb {
    HelperVerb::Ext(ExtVerb::new(
        "update-rollback",
        None,
        "Going back to the previous PixelPlus version",
        Duration::from_secs(10 * 60),
        "install updates",
    ))
}

/// `update-channel:<stable|beta>`: the apt source's suite (installs from apt).
pub fn verb_channel(channel: UpdateChannel) -> HelperVerb {
    let c = match channel {
        UpdateChannel::Stable => "stable",
        UpdateChannel::Beta => "beta",
    };
    HelperVerb::Ext(ExtVerb::new(
        "update-channel",
        Some(c),
        format!("Switching to the {c} channel"),
        Duration::from_secs(120),
        "change the update channel",
    ))
}

async fn run_verb(state: &AppState, verb: HelperVerb, timeout: Duration) -> Result<String, String> {
    match platform::run_helper(state, verb, HelperOpts { quiet: true }).await {
        Ok(job) => {
            let s = job.wait(timeout).await;
            if s.state == HelperState::Ok {
                Ok(s.message)
            } else {
                Err(s.message)
            }
        }
        Err(e) => Err(e.message),
    }
}

/// Put a verified package where the helper picks it up, and stage it.
async fn stage_local(state: &AppState, pkg: &Path, sig: &str, version: &str) -> Result<(), String> {
    let incoming = updates::incoming_dir(&state.config.data_dir);
    tokio::fs::create_dir_all(&incoming)
        .await
        .map_err(|e| e.to_string())?;
    let name = pkg
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or("bad package path")?;
    let dst = incoming.join(&name);
    if dst != pkg {
        tokio::fs::copy(pkg, &dst)
            .await
            .map_err(|e| e.to_string())?;
    }
    tokio::fs::write(incoming.join(format!("{name}.minisig")), sig)
        .await
        .map_err(|e| e.to_string())?;
    let r = run_verb(state, verb_stage(version), Duration::from_secs(20 * 60)).await;
    // The helper copied it into its own (root-only) directory.
    let _ = tokio::fs::remove_file(&dst).await;
    let _ = tokio::fs::remove_file(incoming.join(format!("{name}.minisig"))).await;
    r.map(|_| ())
}

// ---------------------------------------------------------------------------
// Follower side: commands from the leader
// ---------------------------------------------------------------------------

/// `update.stage | update.commit | update.rollback` from our leader.
/// Returns at once; progress shows in the beacon report.
pub async fn follower_command(state: &AppState, cmd: ClusterCommand) -> ApiResult<()> {
    if !can_apply_here() {
        return Err(ApiError::forbidden(
            "This controller can't install updates by itself.",
        ));
    }
    match cmd {
        ClusterCommand::UpdateStage {
            version,
            file,
            size,
            sha256,
            sig,
        } => {
            if !updates::valid_version(&version)
                || file != format!("pixelplus_{version}_{}.deb", updates::deb_arch())
                || sha256.len() != 64
                || size == 0
                || size > updates::MAX_PACKAGE
            {
                return Err(ApiError::bad_request("Invalid update package."));
            }
            {
                let l = state.services.updates_orch.local.lock();
                if matches!(l.phase.as_str(), "staging" | "committing" | "rollingBack") {
                    return Err(ApiError::conflict("An update is already in progress here."));
                }
            }
            set_local(state, "staging", Some(version.clone()), None);
            let st = state.clone();
            tokio::spawn(async move {
                let r = follower_stage(&st, &version, &file, size, &sha256, &sig).await;
                match r {
                    Ok(()) => set_local(&st, "staged", Some(version), None),
                    Err(e) => {
                        tracing::warn!("staging {version} failed: {e}");
                        set_local(&st, "failed", Some(version), Some(e));
                    }
                }
            });
            Ok(())
        }
        ClusterCommand::UpdateCommit { version } => {
            {
                let l = state.services.updates_orch.local.lock();
                if l.phase != "staged" || l.version.as_deref() != Some(version.as_str()) {
                    return Err(ApiError::conflict("That version isn't prepared here."));
                }
            }
            set_local(state, "committing", Some(version.clone()), None);
            let st = state.clone();
            tokio::spawn(async move {
                // Success restarts this daemon (the new one reports its version).
                if let Err(e) =
                    run_verb(&st, verb_commit(&version), Duration::from_secs(10 * 60)).await
                {
                    set_local(&st, "failed", Some(version), Some(e));
                }
            });
            Ok(())
        }
        ClusterCommand::UpdateRollback => {
            set_local(state, "rollingBack", None, None);
            let st = state.clone();
            tokio::spawn(async move {
                if let Err(e) = run_verb(&st, verb_rollback(), Duration::from_secs(10 * 60)).await {
                    set_local(&st, "failed", None, Some(e));
                }
            });
            Ok(())
        }
        _ => Ok(()),
    }
}

async fn follower_stage(
    state: &AppState,
    version: &str,
    file: &str,
    size: u64,
    sha256: &str,
    sig: &str,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let identity = state.identity();
    let (Some(leader_url), Some(leader_id), Some(key)) = (
        identity.leader_url.clone(),
        identity.leader_id.clone(),
        identity.cluster_key.clone(),
    ) else {
        return Err("no leader".into());
    };
    let cluster = state.services.cluster.get().ok_or("cluster not running")?;
    let incoming = updates::incoming_dir(&state.config.data_dir);
    tokio::fs::create_dir_all(&incoming)
        .await
        .map_err(|e| e.to_string())?;
    if let Some((free, _)) = super::system::disk_space(&incoming) {
        if free < size.saturating_mul(3) + 256 * 1024 * 1024 {
            return Err(format!(
                "not enough free space ({} MB free)",
                free / 1_000_000
            ));
        }
    }
    let url = format!(
        "{}/api/v1/cluster/update/{file}",
        leader_url.trim_end_matches('/')
    );
    let resp = crate::cluster::sig::call(
        &cluster.shared,
        &key,
        &identity.id,
        &leader_id,
        reqwest::Method::GET,
        &url,
        None,
        Duration::from_secs(600),
        &[],
    )
    .await
    .map_err(|e| format!("download from the leader failed: {e}"))?
    .resp;
    if !resp.status().is_success() {
        return Err(format!("the leader answered HTTP {}", resp.status()));
    }
    let path = incoming.join(file);
    let part = incoming.join(format!("{file}.part"));
    let mut out = tokio::fs::File::create(&part)
        .await
        .map_err(|e| e.to_string())?;
    let mut total = 0u64;
    let mut resp = resp;
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        total += chunk.len() as u64;
        if total > size {
            break;
        }
        out.write_all(&chunk).await.map_err(|e| e.to_string())?;
    }
    out.sync_all().await.map_err(|e| e.to_string())?;
    drop(out);
    let checked = {
        let (part, want, sig, keys) = (
            part.clone(),
            sha256.to_string(),
            sig.to_string(),
            updates::trusted_keys(),
        );
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            if total != size {
                return Err("the download was incomplete".into());
            }
            let got = updates::sha256_file(&part).map_err(|e| e.to_string())?;
            if !got.eq_ignore_ascii_case(&want) {
                return Err("the package is damaged (checksum)".into());
            }
            // The leader can't push unsigned code either.
            updates::verify_file(&keys, &part, &sig)
        })
        .await
        .map_err(|e| e.to_string())?
    };
    if let Err(e) = checked {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }
    tokio::fs::rename(&part, &path)
        .await
        .map_err(|e| e.to_string())?;
    stage_local(state, &path, sig, version).await
}

// ---------------------------------------------------------------------------
// The real fleet
// ---------------------------------------------------------------------------

/// The cluster as the leader's updater drives it.
pub struct ClusterFleet {
    pub state: AppState,
    pub scope: Scope,
}

impl ClusterFleet {
    fn self_node(&self) -> FleetNode {
        let st = &self.state;
        let identity = st.identity();
        let r = local_report(st);
        FleetNode {
            id: identity.id.clone(),
            name: st
                .store
                .get()
                .node(&identity.id)
                .map(|n| n.name.clone())
                .unwrap_or_else(|| "This controller".into()),
            is_self: true,
            online: true,
            version: CURRENT.to_string(),
            arch: r.arch,
            can_apply: r.can_apply,
            disk_free_mb: Some(r.disk_free_mb),
            phase: r.phase,
            message: r.message,
        }
    }
}

impl Fleet for ClusterFleet {
    fn nodes(&self) -> Vec<FleetNode> {
        let mut out = vec![self.self_node()];
        if self.scope == Scope::This {
            return out;
        }
        let Some(cluster) = self.state.services.cluster.get() else {
            return out;
        };
        let show = self.state.store.get();
        let status = cluster.nodes_status();
        let peers = cluster.shared.peers.read();
        for n in show
            .nodes
            .iter()
            .filter(|n| n.role == NodeRole::Follower && n.adopted)
        {
            let st = status.iter().find(|s| s.id == n.id);
            let report = peers
                .get(&n.id)
                .filter(|p| p.authenticated)
                .and_then(|p| p.beacon.report.as_ref())
                .and_then(|r| r.update.clone());
            out.push(FleetNode {
                id: n.id.clone(),
                name: n.name.clone(),
                is_self: false,
                online: st.is_some_and(|s| s.online),
                version: st.and_then(|s| s.version.clone()).unwrap_or_default(),
                arch: report
                    .as_ref()
                    .map(|r| r.arch.clone())
                    .unwrap_or_else(|| "arm64".into()),
                can_apply: report.as_ref().is_some_and(|r| r.can_apply),
                disk_free_mb: report.as_ref().map(|r| r.disk_free_mb),
                phase: report
                    .as_ref()
                    .map(|r| r.phase.clone())
                    .unwrap_or_else(|| "idle".into()),
                message: report.and_then(|r| r.message),
            });
        }
        out
    }

    async fn stage(&self, node: &FleetNode, target: &Target) -> Result<(), String> {
        let pkg = target
            .packages
            .get(&node.arch)
            .ok_or_else(|| format!("no {} package", node.arch))?;
        let name = pkg
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let sig = tokio::fs::read_to_string(pkg.with_file_name(format!("{name}.minisig")))
            .await
            .map_err(|e| format!("signature missing: {e}"))?;
        if node.is_self {
            set_local(&self.state, "staging", Some(target.version.clone()), None);
            let r = stage_local(&self.state, pkg, &sig, &target.version).await;
            match &r {
                Ok(()) => set_local(&self.state, "staged", Some(target.version.clone()), None),
                Err(e) => set_local(
                    &self.state,
                    "failed",
                    Some(target.version.clone()),
                    Some(e.clone()),
                ),
            }
            return r;
        }
        let cluster = self
            .state
            .services
            .cluster
            .get()
            .ok_or("cluster not running")?;
        let (size, sha) = {
            let p = pkg.clone();
            tokio::task::spawn_blocking(move || -> std::io::Result<(u64, String)> {
                Ok((std::fs::metadata(&p)?.len(), updates::sha256_file(&p)?))
            })
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?
        };
        let res = cluster
            .send_command(
                Some(&node.id),
                ClusterCommand::UpdateStage {
                    version: target.version.clone(),
                    file: name,
                    size,
                    sha256: sha,
                    sig,
                },
            )
            .await;
        if let Some(e) = res
            .into_iter()
            .find_map(|r| (!r.ok).then(|| r.error.unwrap_or_default()))
        {
            return Err(e);
        }
        // Wait for its report to say staged (or failed).
        let deadline = Instant::now() + Timing::production().stage;
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let n = self.nodes().into_iter().find(|n| n.id == node.id);
            match n {
                Some(n) if n.phase == "staged" => return Ok(()),
                Some(n) if n.phase == "failed" => {
                    return Err(n.message.unwrap_or_else(|| "staging failed".into()))
                }
                _ => {}
            }
            if Instant::now() > deadline {
                return Err("preparing took too long".into());
            }
        }
    }

    async fn commit(&self, node: &FleetNode, version: &str) -> Result<(), String> {
        if node.is_self {
            set_local(&self.state, "committing", Some(version.to_string()), None);
            // Restarts this daemon on success (the job is saved already).
            return run_verb(
                &self.state,
                verb_commit(version),
                Duration::from_secs(10 * 60),
            )
            .await
            .map(|_| ());
        }
        let cluster = self
            .state
            .services
            .cluster
            .get()
            .ok_or("cluster not running")?;
        let res = cluster
            .send_command(
                Some(&node.id),
                ClusterCommand::UpdateCommit {
                    version: version.to_string(),
                },
            )
            .await;
        match res.into_iter().find(|r| !r.ok) {
            Some(r) => Err(r.error.unwrap_or_default()),
            None => Ok(()),
        }
    }

    async fn rollback(&self, node: &FleetNode) -> Result<(), String> {
        if node.is_self {
            set_local(&self.state, "rollingBack", None, None);
            return run_verb(&self.state, verb_rollback(), Duration::from_secs(10 * 60))
                .await
                .map(|_| ());
        }
        let cluster = self
            .state
            .services
            .cluster
            .get()
            .ok_or("cluster not running")?;
        let res = cluster
            .send_command(Some(&node.id), ClusterCommand::UpdateRollback)
            .await;
        match res.into_iter().find(|r| !r.ok) {
            Some(r) => Err(r.error.unwrap_or_default()),
            None => Ok(()),
        }
    }

    fn save(&self, job: &UpdateJob) {
        save_job(&self.state, job);
    }
}

fn save_job(state: &AppState, job: &UpdateJob) {
    *state.services.updates_orch.job.lock() = Some(job.clone());
    write_json(&updates_dir(state).join("job.json"), job);
    state.events.publish("updateJob", job);
}

/// A finished job: history, journal, alert.
async fn record_result(state: &AppState, job: &UpdateJob) {
    let ok = job.phase == Phase::Done;
    push_history(
        state,
        HistoryEntry {
            at: job.finished_at.clone().unwrap_or_else(now_rfc3339),
            from: job.from.clone(),
            to: job.version.clone(),
            ok,
            scope: job.scope,
            message: job.message.clone(),
            nodes: job.nodes.len(),
        },
    );
    super::journal::record(
        state,
        super::journal::Event::Update {
            from: job.from.clone(),
            to: job.version.clone(),
            ok,
        },
    );
    let msg = job.message.clone().unwrap_or_default();
    match job.phase {
        Phase::Done => state
            .events
            .toast(crate::events::ToastKind::Success, format!("Updated: {msg}")),
        _ => {
            let title = if job.kind == JobKind::Rollback {
                "Going back to the previous version"
            } else {
                "Update failed"
            };
            super::alerts::raise(
                state,
                &format!("update-{}", job.id),
                super::alerts::Severity::Warning,
                title,
                &format!("PixelPlus {}: {msg}", job.version),
            )
            .await;
        }
    }
}

// ---------------------------------------------------------------------------
// Starting jobs (API) and the release index
// ---------------------------------------------------------------------------

/// The latest release of the configured channel (cached for 6 hours unless
/// `refresh`).
pub async fn latest(state: &AppState, refresh: bool) -> Result<ReleaseIndex, String> {
    let channel = state.store.get().settings.updates.channel;
    {
        let c = state.services.updates_orch.index.lock();
        if let Some((at, ch, r)) = c.as_ref() {
            if !refresh && *ch == channel && at.elapsed() < INDEX_TTL {
                return r.clone();
            }
        }
    }
    let keys = updates::trusted_keys();
    let r = if keys.is_empty() {
        Err(
            "This PixelPlus build has no update signing key, so it can't check for signed updates."
                .into(),
        )
    } else {
        let ch = match channel {
            UpdateChannel::Stable => "stable",
            UpdateChannel::Beta => "beta",
        };
        updates::fetch_index(&updates::index_base(), ch, &keys).await
    };
    *state.services.updates_orch.index.lock() = Some((Instant::now(), channel, r.clone()));
    r
}

/// Signed updates can run here (a key, a packaged helper, not Docker).
pub fn ota_available() -> bool {
    !updates::trusted_keys().is_empty() && can_apply_here()
}

/// Body of `POST /system/update`.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StartRequest {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub scope: Option<Scope>,
    /// Ignore the "show soon" / protocol warnings (never offline nodes).
    #[serde(default)]
    pub force: bool,
}

fn leader_or_standalone(state: &AppState) -> ApiResult<()> {
    match state.identity().role {
        LocalRole::Leader => Ok(()),
        LocalRole::Follower if state.identity().leader_id.is_some() => Err(ApiError::conflict(
            "This controller is updated by its show leader (Settings → Updates there).",
        )),
        _ => Ok(()),
    }
}

fn playing(state: &AppState) -> bool {
    state
        .services
        .player
        .get()
        .is_some_and(|p| p.status().state == crate::player::PlayerState::Playing)
}

fn show_soon_now(state: &AppState) -> Option<String> {
    let show = state.store.get();
    let tz = pixelplus_core::schedule::schedule_timezone(&show.schedule).ok()?;
    show_soon(
        &show.schedule,
        show.settings.updates.avoid_show_hours,
        chrono::Utc::now().with_timezone(&tz),
    )
}

/// What `POST /system/update` would be blocked by now.
pub async fn plan(state: &AppState, scope: Scope, index: &ReleaseIndex) -> Vec<String> {
    let fleet = ClusterFleet {
        state: state.clone(),
        scope,
    };
    let nodes = fleet.nodes();
    let archs: Vec<String> = index.files.iter().map(|f| f.arch.clone()).collect();
    let mb = index.files.iter().map(|f| f.size).max().unwrap_or(0) / 1_000_000;
    let mut p = preflight(
        &nodes,
        &index.version,
        &archs,
        mb,
        playing(state),
        show_soon_now(state),
    );
    let (cur_min, cur_max) = (
        crate::cluster::proto::PROTOCOL_MIN,
        crate::cluster::proto::PROTOCOL_MAX,
    );
    if crate::cluster::proto::negotiate((index.proto_min, index.proto_max), (cur_min, cur_max))
        .is_none()
        && nodes.len() > 1
    {
        p.push(format!(
            "PixelPlus {} can't talk to {CURRENT} (cluster protocol {}–{}); all controllers must be updated together.",
            index.version, index.proto_min, index.proto_max
        ));
    }
    p
}

/// Start an update job (leader or a standalone controller).
pub async fn start_update(state: &AppState, req: StartRequest, auto: bool) -> ApiResult<UpdateJob> {
    leader_or_standalone(state)?;
    if !can_apply_here() {
        return Err(ApiError::forbidden(
            "This PixelPlus can't install updates by itself (Docker, or not installed from the PixelPlus package).",
        ));
    }
    let orch = &state.services.updates_orch;
    if orch.running.swap(true, Ordering::SeqCst) {
        return Err(ApiError::conflict("An update is already running."));
    }
    struct Release<'a>(&'a AtomicBool, bool);
    impl Drop for Release<'_> {
        fn drop(&mut self) {
            if self.1 {
                self.0.store(false, Ordering::SeqCst);
            }
        }
    }
    let mut guard = Release(&orch.running, true);
    let index = latest(state, true).await.map_err(ApiError::unavailable)?;
    if let Some(v) = &req.version {
        if *v != index.version {
            return Err(ApiError::bad_request(format!(
                "Only the latest release ({}) can be installed.",
                index.version
            )));
        }
    }
    if updates::compare_versions(&index.version, CURRENT) != std::cmp::Ordering::Greater {
        return Err(ApiError::conflict("PixelPlus is up to date."));
    }
    let scope = req.scope.unwrap_or(Scope::Cluster);
    let problems = plan(state, scope, &index).await;
    let blocking: Vec<&String> = problems
        .iter()
        .filter(|p| {
            !req.force || p.contains("offline") || p.contains("can't install") || p.contains("free")
        })
        .collect();
    if !blocking.is_empty() {
        return Err(ApiError::conflict(
            blocking
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(" "),
        ));
    }
    let fleet = ClusterFleet {
        state: state.clone(),
        scope,
    };
    let nodes = fleet.nodes();
    // Download the packages every node needs (followers often have no internet).
    let keys = updates::trusted_keys();
    let dir = updates::packages_dir(&state.config.data_dir);
    let mut packages = HashMap::new();
    let archs: std::collections::BTreeSet<String> = nodes.iter().map(|n| n.arch.clone()).collect();
    for arch in archs {
        let f = index.file_for(&arch).ok_or_else(|| {
            ApiError::unavailable(format!("There's no {arch} package of {}.", index.version))
        })?;
        let p = updates::ensure_package(&dir, f, &keys)
            .await
            .map_err(ApiError::unavailable)?;
        packages.insert(arch, p);
    }
    let snapshot = super::snapshots::create(
        state,
        &format!("Before update to {}", index.version),
        true,
        false,
    )
    .await
    .ok()
    .map(|s| s.id);
    let job = UpdateJob {
        id: pixelplus_core::model::new_id(),
        kind: JobKind::Update,
        scope,
        version: index.version.clone(),
        from: CURRENT.to_string(),
        phase: Phase::Staging,
        nodes: nodes
            .iter()
            .map(|n| JobNode {
                id: n.id.clone(),
                name: n.name.clone(),
                is_self: n.is_self,
                arch: n.arch.clone(),
                from: n.version.clone(),
                phase: NodePhase::Pending,
                committed: false,
                message: None,
            })
            .collect(),
        started_at: now_rfc3339(),
        finished_at: None,
        message: None,
        auto,
        snapshot,
    };
    save_job(state, &job);
    let target = Target {
        version: index.version.clone(),
        packages,
    };
    guard.1 = false; // the task below owns `running` now
    let st = state.clone();
    let first = job.clone();
    tokio::spawn(async move {
        let fleet = ClusterFleet {
            state: st.clone(),
            scope,
        };
        let done = run_job(&fleet, job, &target, Timing::production()).await;
        if done.phase.finished() {
            record_result(&st, &done).await;
        }
        st.services
            .updates_orch
            .running
            .store(false, Ordering::SeqCst);
        updates::prune_packages(
            &updates::packages_dir(&st.config.data_dir),
            &[done.version.clone(), done.from.clone()],
        );
    });
    Ok(first)
}

/// "Roll back to <previous>": every controller goes back to the version it
/// had before the last update (the helpers keep that package).
pub async fn start_rollback(state: &AppState, scope: Scope) -> ApiResult<UpdateJob> {
    leader_or_standalone(state)?;
    if !can_apply_here() {
        return Err(ApiError::forbidden(
            "This PixelPlus can't change its version by itself.",
        ));
    }
    let last = history(state)
        .into_iter()
        .find(|h| h.ok && h.to == CURRENT)
        .ok_or_else(|| ApiError::conflict("There's no earlier version to go back to."))?;
    let orch = &state.services.updates_orch;
    if orch.running.swap(true, Ordering::SeqCst) {
        return Err(ApiError::conflict("An update is already running."));
    }
    let fleet = ClusterFleet {
        state: state.clone(),
        scope,
    };
    let nodes = fleet.nodes();
    if let Some(n) = nodes.iter().find(|n| !n.online) {
        orch.running.store(false, Ordering::SeqCst);
        return Err(ApiError::conflict(format!("{} is offline.", n.name)));
    }
    let job = UpdateJob {
        id: pixelplus_core::model::new_id(),
        kind: JobKind::Rollback,
        scope,
        version: last.from.clone(),
        from: CURRENT.to_string(),
        phase: Phase::RollingBack,
        nodes: nodes
            .iter()
            .map(|n| JobNode {
                id: n.id.clone(),
                name: n.name.clone(),
                is_self: n.is_self,
                arch: n.arch.clone(),
                from: last.from.clone(),
                phase: NodePhase::Pending,
                committed: n.version == CURRENT,
                message: None,
            })
            .collect(),
        started_at: now_rfc3339(),
        finished_at: None,
        message: None,
        auto: false,
        snapshot: None,
    };
    save_job(state, &job);
    let first = job.clone();
    let st = state.clone();
    tokio::spawn(async move {
        let fleet = ClusterFleet {
            state: st.clone(),
            scope,
        };
        let msg = format!("Back to PixelPlus {}.", job.version);
        let mut done = rollback_all(&fleet, job, Timing::production(), msg).await;
        if done.phase == Phase::RolledBack {
            done.phase = Phase::Done;
        }
        record_result(&st, &done).await;
        st.services
            .updates_orch
            .running
            .store(false, Ordering::SeqCst);
    });
    Ok(first)
}

// ---------------------------------------------------------------------------
// Startup: health file, helper results, resuming, automatic updates
// ---------------------------------------------------------------------------

/// `/run/pixelplus/healthy.json`: written once the engine, the output and the
/// cluster socket are up. The helper's health gate waits for it (with this
/// version) before it keeps a new release.
fn write_health(state: &AppState) {
    let dir = platform::run_dir();
    if !dir.is_dir() {
        return;
    }
    let v = serde_json::json!({
        "version": CURRENT,
        "engine": state.services.player.get().is_some(),
        "output": true,
        "cluster": state.services.cluster.get().is_some_and(|c| c.shared.socket.get().is_some()),
        "at": now_rfc3339(),
        "pid": std::process::id(),
    });
    let tmp = dir.join(format!(".healthy.{}", std::process::id()));
    if std::fs::write(&tmp, v.to_string())
        .and_then(|_| std::fs::rename(&tmp, dir.join("healthy.json")))
        .is_err()
    {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// What the helper left after installing / rolling back
/// (`/run/pixelplus/update-result.json`, root-written).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HelperResult {
    from: String,
    to: String,
    ok: bool,
    #[serde(default)]
    message: Option<String>,
    at: i64,
}

async fn apply_helper_result(state: &AppState) {
    let path = platform::run_dir().join("update-result.json");
    let Some(r) = read_json::<HelperResult>(&path) else {
        return;
    };
    let marker = updates_dir(state).join("result-seen");
    let seen = std::fs::read_to_string(&marker).ok();
    if seen.as_deref() == Some(&r.at.to_string()) {
        return;
    }
    let _ = std::fs::create_dir_all(updates_dir(state));
    let _ = std::fs::write(&marker, r.at.to_string());
    if r.ok {
        return;
    }
    let msg = r
        .message
        .clone()
        .unwrap_or_else(|| format!("PixelPlus {} didn't start properly and was undone.", r.to));
    set_local(state, "failed", Some(r.to.clone()), Some(msg.clone()));
    // A newer release may have migrated the show: restore the snapshot taken
    // before the update when this version can't fully read it.
    let show = state.store.get();
    if show.format_version > pixelplus_core::model::SHOW_FORMAT_VERSION {
        if let Some(snap) =
            read_json::<UpdateJob>(&updates_dir(state).join("job.json")).and_then(|j| j.snapshot)
        {
            match super::snapshots::restore(state, &snap).await {
                Ok(_) => tracing::warn!("restored snapshot {snap} after the rollback"),
                Err(e) => tracing::error!("couldn't restore snapshot {snap}: {}", e.message),
            }
        }
    }
    if state.identity().role != LocalRole::Follower {
        super::alerts::raise(
            state,
            "update-rolled-back",
            super::alerts::Severity::Warning,
            &format!("Update to {} failed and was rolled back", r.to),
            &msg,
        )
        .await;
    }
    tracing::warn!("update {} → {} was rolled back: {msg}", r.from, r.to);
}

/// Start the service (called once from `services::start_all`).
pub fn start(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        // Health: once the engine and the cluster are up.
        for _ in 0..120 {
            if state.services.player.get().is_some() && state.services.cluster.get().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        write_health(&state);
        apply_helper_result(&state).await;
        // Resume a job this daemon's restart interrupted.
        if let Some(job) = read_json::<UpdateJob>(&updates_dir(&state).join("job.json")) {
            *state.services.updates_orch.job.lock() = Some(job.clone());
            if !job.phase.finished() && state.identity().role != LocalRole::Follower {
                state
                    .services
                    .updates_orch
                    .running
                    .store(true, Ordering::SeqCst);
                // Give the followers a moment to be heard again.
                tokio::time::sleep(Duration::from_secs(15)).await;
                let fleet = ClusterFleet {
                    state: state.clone(),
                    scope: job.scope,
                };
                let done = resume(&fleet, job, Timing::production()).await;
                if done.phase.finished() {
                    record_result(&state, &done).await;
                }
                state
                    .services
                    .updates_orch
                    .running
                    .store(false, Ordering::SeqCst);
            }
        }
        auto_loop(state).await;
    });
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutoState {
    #[serde(default)]
    notified: Option<String>,
    #[serde(default)]
    last_try: Option<String>,
}

async fn auto_loop(state: AppState) {
    tokio::time::sleep(Duration::from_secs(5 * 60)).await;
    loop {
        if let Err(e) = auto_tick(&state).await {
            tracing::debug!("automatic update: {e}");
        }
        tokio::time::sleep(Duration::from_secs(10 * 60)).await;
    }
}

async fn auto_tick(state: &AppState) -> Result<(), String> {
    let settings = state.store.get().settings.updates.clone();
    if settings.auto == AutoUpdate::Off
        || leader_or_standalone(state).is_err()
        || busy(state)
        || updates::trusted_keys().is_empty()
    {
        return Ok(());
    }
    let index = latest(state, false).await?;
    if updates::compare_versions(&index.version, CURRENT) != std::cmp::Ordering::Greater {
        return Ok(());
    }
    let path = updates_dir(state).join("auto.json");
    let mut st: AutoState = read_json(&path).unwrap_or_default();
    if st.notified.as_deref() != Some(index.version.as_str()) {
        st.notified = Some(index.version.clone());
        write_json(&path, &st);
        super::alerts::raise(
            state,
            &format!("update-available-{}", index.version),
            super::alerts::Severity::Info,
            "Update available",
            &format!(
                "PixelPlus {} is available (you have {CURRENT}). {}",
                index.version,
                if settings.auto == AutoUpdate::Install {
                    "It will be installed in the next update window."
                } else {
                    "Install it under Settings → Updates."
                }
            ),
        )
        .await;
    }
    if settings.auto != AutoUpdate::Install || !can_apply_here() {
        return Ok(());
    }
    let show = state.store.get();
    let tz =
        pixelplus_core::schedule::schedule_timezone(&show.schedule).map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().with_timezone(&tz);
    if let Some(why) = window_problem(&settings, &show.schedule, now) {
        return Err(why);
    }
    // One automatic attempt per version and day.
    let key = format!("{}@{}", index.version, now.date_naive());
    if st.last_try.as_deref() == Some(key.as_str()) {
        return Ok(());
    }
    st.last_try = Some(key);
    write_json(&path, &st);
    start_update(
        state,
        StartRequest {
            version: Some(index.version.clone()),
            scope: Some(Scope::Cluster),
            force: false,
        },
        true,
    )
    .await
    .map(|_| ())
    .map_err(|e| e.message)
}

/// Serve a package to a follower (`GET /cluster/update/:file`, leader).
pub fn package_path(state: &AppState, file: &str) -> Option<PathBuf> {
    if !updates::valid_package_name(file) {
        return None;
    }
    let p = updates::packages_dir(&state.config.data_dir).join(file);
    p.is_file().then_some(p)
}

// ---------------------------------------------------------------------------
// Tests: the state machine with a fake fleet
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A simulated cluster: versions change when commits "restart" nodes.
    #[derive(Clone, Default)]
    struct Fake {
        inner: Arc<Mutex<FakeInner>>,
    }

    #[derive(Default)]
    struct FakeInner {
        nodes: Vec<FleetNode>,
        /// Node ids whose staging fails (bad signature, no space…).
        stage_fails: Vec<String>,
        /// Node ids whose new version fails its health gate (the helper
        /// puts the old one back by itself).
        health_fails: Vec<String>,
        /// Node ids that never come back after a commit.
        vanish: Vec<String>,
        log: Vec<String>,
        saved: Vec<Phase>,
        previous: HashMap<String, String>,
    }

    impl Fake {
        fn new(n: usize) -> Self {
            let f = Fake::default();
            {
                let mut i = f.inner.lock();
                for k in 0..n {
                    i.nodes.push(FleetNode {
                        id: if k == 0 {
                            "leader".into()
                        } else {
                            format!("f{k}")
                        },
                        name: if k == 0 {
                            "Leader".into()
                        } else {
                            format!("Follower {k}")
                        },
                        is_self: k == 0,
                        online: true,
                        version: "1.0.0".into(),
                        arch: "arm64".into(),
                        can_apply: true,
                        disk_free_mb: Some(4000),
                        phase: "idle".into(),
                        message: None,
                    });
                }
            }
            f
        }
        fn version(&self, id: &str) -> String {
            self.inner
                .lock()
                .nodes
                .iter()
                .find(|n| n.id == id)
                .unwrap()
                .version
                .clone()
        }
        fn log(&self) -> Vec<String> {
            self.inner.lock().log.clone()
        }
    }

    impl Fleet for Fake {
        fn nodes(&self) -> Vec<FleetNode> {
            self.inner.lock().nodes.clone()
        }
        async fn stage(&self, node: &FleetNode, target: &Target) -> Result<(), String> {
            let mut i = self.inner.lock();
            i.log.push(format!("stage {}", node.id));
            if i.stage_fails.contains(&node.id) {
                return Err("The package signature doesn't match".into());
            }
            let _ = target;
            Ok(())
        }
        async fn commit(&self, node: &FleetNode, version: &str) -> Result<(), String> {
            let mut i = self.inner.lock();
            i.log.push(format!("commit {}", node.id));
            let health_fails = i.health_fails.contains(&node.id);
            let vanish = i.vanish.contains(&node.id);
            let n = i.nodes.iter_mut().find(|n| n.id == node.id).unwrap();
            let old = n.version.clone();
            if vanish {
                n.online = false;
            } else if health_fails {
                // The helper's gate put the old version back.
                n.phase = "failed".into();
                n.message = Some(format!(
                    "PixelPlus {version} didn't start properly and was undone."
                ));
            } else {
                n.version = version.to_string();
            }
            i.previous.insert(node.id.clone(), old);
            Ok(())
        }
        async fn rollback(&self, node: &FleetNode) -> Result<(), String> {
            let mut i = self.inner.lock();
            i.log.push(format!("rollback {}", node.id));
            let prev = i
                .previous
                .get(&node.id)
                .cloned()
                .unwrap_or_else(|| "1.0.0".into());
            let n = i.nodes.iter_mut().find(|n| n.id == node.id).unwrap();
            n.version = prev;
            n.phase = "idle".into();
            n.online = true;
            Ok(())
        }
        fn save(&self, job: &UpdateJob) {
            self.inner.lock().saved.push(job.phase);
        }
    }

    fn timing() -> Timing {
        Timing {
            stage: Duration::from_millis(200),
            commit: Duration::from_millis(200),
            poll: Duration::from_millis(5),
        }
    }

    fn job_for(fake: &Fake) -> UpdateJob {
        UpdateJob {
            id: "j".into(),
            kind: JobKind::Update,
            scope: Scope::Cluster,
            version: "1.1.0".into(),
            from: "1.0.0".into(),
            phase: Phase::Staging,
            nodes: fake
                .nodes()
                .iter()
                .map(|n| JobNode {
                    id: n.id.clone(),
                    name: n.name.clone(),
                    is_self: n.is_self,
                    arch: n.arch.clone(),
                    from: n.version.clone(),
                    phase: NodePhase::Pending,
                    committed: false,
                    message: None,
                })
                .collect(),
            started_at: now_rfc3339(),
            finished_at: None,
            message: None,
            auto: false,
            snapshot: None,
        }
    }

    fn target() -> Target {
        Target {
            version: "1.1.0".into(),
            packages: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn happy_path_updates_followers_first_then_the_leader() {
        let fake = Fake::new(3);
        let done = run_job(&fake, job_for(&fake), &target(), timing()).await;
        assert_eq!(done.phase, Phase::Done, "{:?}", done.message);
        assert!(done.nodes.iter().all(|n| n.phase == NodePhase::Healthy));
        for id in ["leader", "f1", "f2"] {
            assert_eq!(fake.version(id), "1.1.0");
        }
        let log = fake.log();
        let pos = |s: &str| log.iter().position(|l| l == s).unwrap();
        // All staged before anything is committed; the leader goes last.
        assert!(pos("stage f2") < pos("commit f1"));
        assert!(pos("commit f1") < pos("commit leader"));
        assert!(pos("commit f2") < pos("commit leader"));
        // The job was persisted before the leader's commit (it restarts).
        assert!(fake.inner.lock().saved.contains(&Phase::CommittingLeader));
    }

    #[tokio::test]
    async fn a_bad_signature_stops_before_anything_is_installed() {
        let fake = Fake::new(3);
        fake.inner.lock().stage_fails.push("f2".into());
        let done = run_job(&fake, job_for(&fake), &target(), timing()).await;
        assert_eq!(done.phase, Phase::Failed);
        assert!(done.message.unwrap().contains("nothing was installed"));
        assert!(fake.log().iter().all(|l| !l.starts_with("commit")));
        let f2 = done.nodes.iter().find(|n| n.id == "f2").unwrap();
        assert!(f2.message.as_ref().unwrap().contains("signature"));
    }

    #[tokio::test]
    async fn a_follower_failing_its_health_gate_rolls_everyone_back() {
        let fake = Fake::new(3);
        fake.inner.lock().health_fails.push("f2".into());
        let done = run_job(&fake, job_for(&fake), &target(), timing()).await;
        assert_eq!(done.phase, Phase::RolledBack, "{:?}", done.message);
        assert!(done.message.as_ref().unwrap().contains("Follower 2"));
        // The leader was never committed; f1 was and is back on 1.0.0.
        assert!(!fake.log().contains(&"commit leader".to_string()));
        assert!(fake.log().contains(&"rollback f1".to_string()));
        for id in ["leader", "f1", "f2"] {
            assert_eq!(fake.version(id), "1.0.0", "{id}");
        }
    }

    #[tokio::test]
    async fn a_follower_that_never_returns_times_out_and_rolls_back() {
        let fake = Fake::new(2);
        fake.inner.lock().vanish.push("f1".into());
        let done = run_job(&fake, job_for(&fake), &target(), timing()).await;
        // It is offline, so it can't be told to roll back: the job says so.
        assert_eq!(done.phase, Phase::Failed);
        let f1 = done.nodes.iter().find(|n| n.id == "f1").unwrap();
        assert_eq!(f1.phase, NodePhase::Failed);
        assert!(done.message.unwrap().contains("Follower 1"));
    }

    #[tokio::test]
    async fn the_leader_failing_its_gate_rolls_the_followers_back() {
        let fake = Fake::new(3);
        fake.inner.lock().health_fails.push("leader".into());
        let done = run_job(&fake, job_for(&fake), &target(), timing()).await;
        assert_eq!(done.phase, Phase::RolledBack, "{:?}", done.message);
        for id in ["leader", "f1", "f2"] {
            assert_eq!(fake.version(id), "1.0.0", "{id}");
        }
        // The leader's own helper already put it back; the followers follow.
        let log = fake.log();
        assert!(log.contains(&"rollback f1".to_string()));
        assert!(log.contains(&"rollback f2".to_string()));
        assert!(!log.contains(&"rollback leader".to_string()));
    }

    #[tokio::test]
    async fn a_follower_failing_after_the_leader_rolls_back_all_leader_last() {
        // Everything committed, then f1 fails the final check.
        let fake = Fake::new(3);
        let mut job = job_for(&fake);
        {
            let mut i = fake.inner.lock();
            for n in i.nodes.iter_mut() {
                n.version = "1.1.0".into();
            }
            for id in ["leader", "f1", "f2"] {
                i.previous.insert(id.into(), "1.0.0".into());
            }
            let f1 = i.nodes.iter_mut().find(|n| n.id == "f1").unwrap();
            f1.phase = "failed".into();
        }
        job.phase = Phase::CommittingLeader;
        job.nodes.iter_mut().for_each(|n| n.committed = true);
        let done = resume(&fake, job, timing()).await;
        assert_eq!(done.phase, Phase::RolledBack, "{:?}", done.message);
        let log = fake.log();
        let pos = |s: &str| log.iter().position(|l| l == s).unwrap();
        assert!(pos("rollback f2") < pos("rollback leader"));
        for id in ["leader", "f1", "f2"] {
            assert_eq!(fake.version(id), "1.0.0", "{id}");
        }
    }

    #[tokio::test]
    async fn resuming_after_the_leaders_restart() {
        // The leader restarted into 1.1.0: verify and finish.
        let fake = Fake::new(3);
        let mut job = job_for(&fake);
        for id in ["leader", "f1", "f2"] {
            fake.inner
                .lock()
                .nodes
                .iter_mut()
                .find(|n| n.id == id)
                .unwrap()
                .version = "1.1.0".into();
        }
        job.phase = Phase::CommittingLeader;
        job.nodes.iter_mut().for_each(|n| n.committed = true);
        let done = resume(&fake, job, timing()).await;
        assert_eq!(done.phase, Phase::Done);
        // The helper put the leader back to 1.0.0: roll the followers back too.
        let fake = Fake::new(3);
        let mut job = job_for(&fake);
        for id in ["f1", "f2"] {
            let mut i = fake.inner.lock();
            i.nodes.iter_mut().find(|n| n.id == id).unwrap().version = "1.1.0".into();
            i.previous.insert(id.into(), "1.0.0".into());
        }
        job.phase = Phase::CommittingLeader;
        job.nodes.iter_mut().for_each(|n| n.committed = true);
        let done = resume(&fake, job, timing()).await;
        assert_eq!(done.phase, Phase::RolledBack, "{:?}", done.message);
        assert!(!fake.log().contains(&"rollback leader".to_string()));
        for id in ["f1", "f2"] {
            assert_eq!(fake.version(id), "1.0.0");
        }
        // Interrupted while staging: nothing to undo.
        let fake = Fake::new(2);
        let mut job = job_for(&fake);
        job.phase = Phase::Staging;
        assert_eq!(resume(&fake, job, timing()).await.phase, Phase::Failed);
    }

    #[test]
    fn preflight_blocks_what_it_should() {
        let fake = Fake::new(3);
        let mut nodes = fake.nodes();
        assert!(preflight(&nodes, "1.1.0", &["arm64".into()], 30, false, None).is_empty());
        nodes[1].online = false;
        nodes[2].arch = "amd64".into();
        nodes[0].disk_free_mb = Some(100);
        let p = preflight(
            &nodes,
            "1.1.0",
            &["arm64".into()],
            30,
            true,
            Some("The show “X” is on now.".into()),
        );
        let all = p.join(" | ");
        assert!(all.contains("playing"), "{all}");
        assert!(all.contains("Follower 1 is offline"), "{all}");
        assert!(all.contains("no amd64 package"), "{all}");
        assert!(all.contains("only 100 MB free"), "{all}");
        assert!(all.contains("is on now"), "{all}");
    }

    #[test]
    fn update_window_and_show_times() {
        use chrono::TimeZone;
        use pixelplus_core::model::{UpdateWindow, Weekday};
        let tz: chrono_tz::Tz = "America/Chicago".parse().unwrap();
        let schedule = Schedule::default();
        let mut s = UpdateSettings::default();
        let at = |h, m| tz.with_ymd_and_hms(2026, 10, 7, h, m, 0).unwrap(); // a Wednesday
        assert!(window_problem(&s, &schedule, at(11, 0)).is_none());
        assert!(window_problem(&s, &schedule, at(9, 59)).is_some());
        assert!(window_problem(&s, &schedule, at(14, 0)).is_some());
        // Across midnight.
        s.window = UpdateWindow {
            from: "23:00".into(),
            to: "03:00".into(),
            days: vec![],
        };
        assert!(window_problem(&s, &schedule, at(1, 0)).is_none());
        assert!(window_problem(&s, &schedule, at(12, 0)).is_some());
        // Only some days.
        s.window = UpdateWindow {
            from: "10:00".into(),
            to: "14:00".into(),
            days: vec![Weekday::Mon],
        };
        assert!(window_problem(&s, &schedule, at(11, 0))
            .unwrap()
            .contains("update day"));
        // A show window soon (or now) blocks.
        let schedule: Schedule = serde_json::from_value(serde_json::json!({
            "enabled": true,
            "location": {"lat": 41.8, "lon": -87.6, "timezone": "America/Chicago"},
            "entries": [{"id": "e", "name": "Evening", "playlistId": "p", "enabled": true,
                         "days": ["mon","tue","wed","thu","fri","sat","sun"], "start": {"kind": "clock", "time": "12:30"},
                         "end": {"kind": "clock", "time": "13:30"}}]
        }))
        .unwrap();
        s.window.days.clear();
        let soon = window_problem(&s, &schedule, at(11, 0));
        assert!(
            soon.is_some_and(|w| w.contains("Evening")),
            "2 h before the show"
        );
        assert!(window_problem(&s, &schedule, at(13, 0))
            .unwrap()
            .contains("on now"));
    }

    #[test]
    fn helper_verbs_are_escaped_for_systemd() {
        let v = verb_stage("1.3.0~beta1");
        assert_eq!(v.instance().unwrap(), "update-stage:1.3.0~beta1");
        assert_eq!(
            v.unit().unwrap(),
            "pixelplus-helper@update-stage:1.3.0\\x7ebeta1.service"
        );
        assert!(HelperVerb::Ext(ExtVerb::new(
            "update-stage",
            Some("1.0;rm"),
            "x",
            Duration::from_secs(1),
            "x"
        ))
        .instance()
        .is_err());
        assert_eq!(
            verb_channel(UpdateChannel::Beta).instance().unwrap(),
            "update-channel:beta"
        );
        assert_eq!(verb_rollback().instance().unwrap(), "update-rollback");
    }
}
