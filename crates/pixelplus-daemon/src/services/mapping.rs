//! Camera mapping runs (F6/F7) and install-tool sessions (F7 manual count,
//! F9 receiver wizard). Owned by WS4.
//!
//! * Runs are stored as `<data>/mapping/<runId>.json` (plus an optional
//!   `<runId>.jpg` photo); they are not part of `show.json`. The newest
//!   [`KEEP_RUNS`] are kept.
//! * The light pattern itself is `pixelplus_core::mapcode` rendered by the
//!   engine (test mode `mapCode`); this module builds plans from the show,
//!   remembers the running pattern so it can be stopped, and applies the
//!   proposals the phone sends back (always after an automatic snapshot).
//!
//! `services/mod.rs` registers [`MappingState`] in `Services::mapping` and
//! calls [`start`] from `start_all`.

use crate::api::{ApiError, ApiResult};
use crate::state::AppState;
use parking_lot::Mutex;
use pixelplus_core::faultfinder::CountSearch;
use pixelplus_core::mapcode::{self, MapPlan, MapTarget};
use pixelplus_core::model::{
    ColorOrder, MeasuredCount, PropLayout, PropSegment, LayoutSource, Show,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

/// Mapping runs kept on disk.
pub const KEEP_RUNS: usize = 30;
/// Largest photo stored with a run.
pub const MAX_PHOTO: usize = 2 * 1024 * 1024;
/// Install-tool sessions end after this long without use.
pub const SESSION_TTL_S: u64 = 30 * 60;

/// Runtime state of this service (`state.services.mapping`).
#[derive(Default)]
pub struct MappingState {
    /// The pattern currently lit (a mapping run or a count probe).
    pub active: Mutex<Option<ActivePattern>>,
    /// Manual pixel-count searches (F7).
    pub counts: Mutex<HashMap<String, CountSession>>,
    /// Receiver wizards (F9).
    pub wizards: Mutex<HashMap<String, WizardSession>>,
}

/// What the lights currently show on behalf of this module.
pub struct ActivePattern {
    /// Run id or session id.
    pub id: String,
    /// Stops the pattern after its schedule (mapping runs only).
    pub stopper: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for ActivePattern {
    fn drop(&mut self) {
        if let Some(t) = self.stopper.take() {
            t.abort();
        }
    }
}

/// A manual pixel-count search.
pub struct CountSession {
    pub node_id: String,
    pub output: u32,
    pub configured: u32,
    pub search: CountSearch,
    pub last_used: Instant,
}

/// A receiver wizard in progress.
pub struct WizardSession {
    pub node_id: String,
    /// Jacks still possible (narrowed by each identify round).
    pub candidates: Vec<u32>,
    pub jack: Option<u32>,
    /// "identify" (blink signals) or "sequential" (one jack at a time).
    pub method: String,
    pub last_used: Instant,
}

/// Start the service (called once from `services::start_all`).
pub fn start(state: &AppState) {
    let dir = dir(state);
    // Keep the folder tidy (runs are small; photos up to 2 MB).
    std::thread::spawn(move || prune_blocking(&dir));
}

// ---------------------------------------------------------------------------
// Runs on disk
// ---------------------------------------------------------------------------

/// Scope of a mapping run (`{all}` | `{nodeId}` | `{propIds}`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MapScope {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub all: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prop_ids: Vec<String>,
}

/// One target as the phone shows it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunTarget {
    pub k: u32,
    pub node_id: String,
    pub output: u32,
    pub label: String,
    /// Props wired to this output (in string order).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prop_ids: Vec<String>,
    /// Pixels configured on the output (end of the last segment).
    #[serde(default)]
    pub configured: u32,
}

/// A change the phone proposes (see `web/src/lib/cv/analyze.ts`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub id: String,
    /// "swap" | "reverse" | "pixelCount" | "notSeen" | "layout"
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prop_id: Option<String>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Decoded lights of one target: `[pixelIndex, x, y, confidence]` (image
/// coordinates normalised 0..1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Detected {
    pub k: u32,
    #[serde(default)]
    pub pixels: Vec<[f32; 4]>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunResults {
    #[serde(default)]
    pub detected: Vec<Detected>,
    #[serde(default)]
    pub proposals: Vec<Proposal>,
    /// Decoder statistics (free-form, for support).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<Value>,
}

/// A stored mapping run (`mapping/<id>.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MappingRun {
    pub id: String,
    /// "map" (F6) | "pixelCount" (F7 camera).
    #[serde(default = "kind_map")]
    pub kind: String,
    pub started_at: String,
    pub scope: MapScope,
    pub plan: MapPlan,
    pub targets: Vec<RunTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub results: Option<RunResults>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_snapshot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applied_proposal_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub photo: bool,
}

fn kind_map() -> String {
    "map".into()
}

pub fn dir(state: &AppState) -> PathBuf {
    state.config.data_dir.join("mapping")
}

/// Run ids are our own 10-char nanoids.
pub fn valid_id(id: &str) -> bool {
    (1..=32).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

fn run_path(state: &AppState, id: &str) -> Option<PathBuf> {
    valid_id(id).then(|| dir(state).join(format!("{id}.json")))
}

pub fn photo_path(state: &AppState, id: &str) -> Option<PathBuf> {
    valid_id(id).then(|| dir(state).join(format!("{id}.jpg")))
}

pub async fn save(state: &AppState, run: &MappingRun) -> ApiResult<()> {
    let path = run_path(state, &run.id).ok_or_else(|| ApiError::bad_request("Bad run id."))?;
    let json = serde_json::to_vec_pretty(run).map_err(ApiError::internal)?;
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        std::fs::create_dir_all(path.parent().unwrap())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(tmp, path)
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(|e| {
        if e.raw_os_error() == Some(28) {
            ApiError::storage_full()
        } else {
            ApiError::internal(format!("couldn't save the mapping run: {e}"))
        }
    })
}

pub async fn load(state: &AppState, id: &str) -> ApiResult<MappingRun> {
    let path = run_path(state, id).ok_or_else(|| ApiError::not_found("That mapping run"))?;
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| ApiError::not_found("That mapping run"))?;
    serde_json::from_slice(&bytes).map_err(|e| ApiError::internal(format!("mapping run: {e}")))
}

/// All runs, newest first.
pub async fn list(state: &AppState) -> Vec<MappingRun> {
    let dir = dir(state);
    tokio::task::spawn_blocking(move || {
        let mut runs: Vec<MappingRun> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
            .collect();
        runs.sort_by(|a: &MappingRun, b| b.started_at.cmp(&a.started_at));
        runs
    })
    .await
    .unwrap_or_default()
}

pub async fn delete(state: &AppState, id: &str) -> ApiResult<()> {
    let path = run_path(state, id).ok_or_else(|| ApiError::not_found("That mapping run"))?;
    if !path.exists() {
        return Err(ApiError::not_found("That mapping run"));
    }
    let _ = tokio::fs::remove_file(&path).await;
    if let Some(p) = photo_path(state, id) {
        let _ = tokio::fs::remove_file(p).await;
    }
    Ok(())
}

fn prune_blocking(dir: &std::path::Path) {
    let mut runs: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            let run: MappingRun = serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok()?;
            Some((run.started_at, e.path()))
        })
        .collect();
    runs.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, p) in runs.into_iter().skip(KEEP_RUNS) {
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(p.with_extension("jpg"));
    }
}

pub async fn prune(state: &AppState) {
    let dir = dir(state);
    let _ = tokio::task::spawn_blocking(move || prune_blocking(&dir)).await;
}

// ---------------------------------------------------------------------------
// Plans (pure)
// ---------------------------------------------------------------------------

/// Configured pixels on a node output: the end of its last segment.
pub fn output_length(show: &Show, node_id: &str, output: u32) -> u32 {
    show.props
        .iter()
        .flat_map(|p| &p.segments)
        .filter(|s| s.node_id == node_id && s.output == output)
        .map(|s| s.start_pixel + s.pixel_count)
        .max()
        .unwrap_or(0)
}

/// Props on a node output, in string order.
pub fn props_on_output(show: &Show, node_id: &str, output: u32) -> Vec<String> {
    let mut v: Vec<(u32, String)> = show
        .props
        .iter()
        .flat_map(|p| p.segments.iter().map(move |s| (p, s)))
        .filter(|(_, s)| s.node_id == node_id && s.output == output)
        .map(|(p, s)| (s.start_pixel, p.id.clone()))
        .collect();
    v.sort();
    v.dedup_by(|a, b| a.1 == b.1);
    v.into_iter().map(|x| x.1).collect()
}

/// Human label of a node output ("Garage J4-2").
pub fn output_label(show: &Show, node_id: &str, output: u32) -> String {
    match show.node(node_id) {
        Some(n) => {
            let port = n
                .outputs
                .iter()
                .find(|o| o.index == output)
                .map(|o| o.label.clone())
                .filter(|l| !l.is_empty())
                .unwrap_or_else(|| n.board.output_label(output as usize));
            format!("{} {port}", n.name)
        }
        None => format!("{node_id} {output}"),
    }
}

/// The wired outputs a scope covers, ordered by `(nodeId, output)`.
pub fn scope_targets(show: &Show, scope: &MapScope) -> Vec<(String, u32)> {
    let mut outs: Vec<(String, u32)> = show
        .props
        .iter()
        .filter(|p| scope.prop_ids.is_empty() || scope.prop_ids.contains(&p.id))
        .flat_map(|p| p.segments.iter())
        .filter(|s| scope.node_id.as_ref().map_or(true, |n| &s.node_id == n))
        .filter(|s| show.node(&s.node_id).is_some() && s.output >= 1)
        .map(|s| (s.node_id.clone(), s.output))
        .collect();
    outs.sort();
    outs.dedup();
    outs
}

/// Options of a mapping run.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlanOptions {
    #[serde(default)]
    pub bit_ms: Option<u32>,
    #[serde(default)]
    pub level: Option<u8>,
    #[serde(default)]
    pub passes: Option<u8>,
    /// Light this many extra pixels past the configured end (F7 probing).
    #[serde(default)]
    pub probe_extra: bool,
}

/// Pixels probed on an output configured with `configured` pixels (F7):
/// `max(configured × 1.25, configured + 64)`, clamped to `limit`.
pub fn probe_len(configured: u32, limit: Option<u32>) -> u32 {
    let want = (configured.saturating_mul(5) / 4).max(configured.saturating_add(64));
    match limit {
        Some(l) if l > 0 => want.min(l),
        _ => want,
    }
    .min(1 << mapcode::MAX_PIXEL_BITS)
}

/// Build a plan and its target list for `outputs`.
pub fn build_plan(
    show: &Show,
    outputs: &[(String, u32)],
    opts: &PlanOptions,
    phases: u8,
    limit: impl Fn(&str) -> Option<u32>,
) -> Result<(MapPlan, Vec<RunTarget>), String> {
    let mut targets = Vec::new();
    let mut run_targets = Vec::new();
    for (k, (node, out)) in outputs.iter().enumerate() {
        let configured = output_length(show, node, *out);
        let max_pixels = if opts.probe_extra {
            probe_len(configured.max(1), limit(node))
        } else {
            configured.max(1)
        };
        targets.push(MapTarget {
            node_id: node.clone(),
            output: *out,
            max_pixels,
        });
        run_targets.push(RunTarget {
            k: k as u32,
            node_id: node.clone(),
            output: *out,
            label: output_label(show, node, *out),
            prop_ids: props_on_output(show, node, *out),
            configured,
        });
    }
    let most = targets.iter().map(|t| t.max_pixels).max().unwrap_or(1);
    let plan = MapPlan {
        seed: rand::random::<u32>() & 0x7fff_ffff,
        bit_ms: opts.bit_ms.unwrap_or(200),
        level: opts.level.unwrap_or(77).min(mapcode::MAX_LEVEL),
        passes: opts.passes.unwrap_or(3),
        phases,
        targets,
        pixel_bits: mapcode::pixel_bits_for(most),
        start_pos_ms: 0,
        count_probe: None,
    };
    mapcode::validate(&plan)?;
    Ok((plan, run_targets))
}

/// Codewords per target as bit arrays (MSB first), for the phone.
pub fn codebook_bits(plan: &MapPlan) -> Vec<Vec<u8>> {
    if plan.phases & mapcode::PHASE_A == 0 {
        return vec![];
    }
    let bits = mapcode::code_bits_for(plan.targets.len());
    (0..plan.targets.len())
        .map(|k| {
            let w = mapcode::codeword(plan, k);
            (0..bits).map(|i| ((w >> (bits - 1 - i)) & 1) as u8).collect()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Applying proposals (pure)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SegRef {
    prop_id: String,
    #[serde(default)]
    segment: usize,
}

#[derive(Deserialize)]
struct SwapData {
    a: SegRef,
    b: SegRef,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CountData {
    node_id: String,
    output: u32,
    count: u32,
    #[serde(default)]
    dead: Vec<u32>,
    #[serde(default)]
    update_prop_count: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LayoutData {
    prop_id: String,
    layout: PropLayout,
}

fn data<T: serde::de::DeserializeOwned>(p: &Proposal) -> ApiResult<T> {
    serde_json::from_value(p.data.clone().unwrap_or(Value::Null)).map_err(|e| {
        ApiError::bad_request(format!("Proposal \"{}\" is malformed ({e}).", p.message))
    })
}

fn seg_mut<'a>(show: &'a mut Show, r: &SegRef) -> ApiResult<&'a mut PropSegment> {
    let prop = show
        .props
        .iter_mut()
        .find(|p| p.id == r.prop_id)
        .ok_or_else(|| ApiError::not_found("A prop in this proposal"))?;
    let name = prop.name.clone();
    prop.segments
        .get_mut(r.segment)
        .ok_or_else(|| ApiError::bad_request(format!("\"{name}\" has no such wiring segment.")))
}

/// No two segments overlap on one output.
pub fn check_overlaps(show: &Show) -> ApiResult<()> {
    let mut by_out: HashMap<(&str, u32), Vec<(u32, u32, &str)>> = HashMap::new();
    for p in &show.props {
        for s in &p.segments {
            by_out
                .entry((s.node_id.as_str(), s.output))
                .or_default()
                .push((s.start_pixel, s.start_pixel + s.pixel_count, p.name.as_str()));
        }
    }
    for ((node, out), mut v) in by_out {
        v.sort();
        for w in v.windows(2) {
            if w[1].0 < w[0].1 {
                return Err(ApiError::bad_request(format!(
                    "\"{}\" and \"{}\" would overlap on {}.",
                    w[0].2,
                    w[1].2,
                    output_label(show, node, out)
                )));
            }
        }
    }
    Ok(())
}

/// Record a measured count on an output and optionally resize the last prop
/// on it. Returns a summary line.
pub fn apply_count(
    show: &mut Show,
    node_id: &str,
    output: u32,
    count: u32,
    dead: &[u32],
    method: &str,
    update_prop_count: bool,
) -> ApiResult<String> {
    let label = output_label(show, node_id, output);
    let node = show
        .nodes
        .iter_mut()
        .find(|n| n.id == node_id)
        .ok_or_else(|| ApiError::not_found("That controller"))?;
    let out = node
        .outputs
        .iter_mut()
        .find(|o| o.index == output)
        .ok_or_else(|| ApiError::bad_request(format!("{label} doesn't exist.")))?;
    let mut dead: Vec<u32> = dead.iter().copied().filter(|&d| d < count).collect();
    dead.sort_unstable();
    dead.dedup();
    dead.truncate(256);
    out.measured_pixels = Some(MeasuredCount {
        count,
        method: method.into(),
        at: chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        dead: dead.clone(),
    });
    // Dead pixels → suspect prop pixels.
    for p in show.props.iter_mut() {
        for s in &p.segments {
            if s.node_id != node_id || s.output != output {
                continue;
            }
            for &d in &dead {
                if d >= s.start_pixel && d < s.start_pixel + s.pixel_count {
                    let i = d - s.start_pixel;
                    let i = if s.reverse { s.pixel_count - 1 - i } else { i };
                    let idx = s.prop_offset + i;
                    if !p.suspect_pixels.contains(&idx) {
                        p.suspect_pixels.push(idx);
                    }
                }
            }
        }
        p.suspect_pixels.sort_unstable();
    }
    let mut summary = format!("{label}: {count} pixels measured");
    if update_prop_count {
        // The last prop on the string takes up the difference.
        let last = show
            .props
            .iter()
            .enumerate()
            .flat_map(|(pi, p)| p.segments.iter().enumerate().map(move |(si, s)| (pi, si, s)))
            .filter(|(_, _, s)| s.node_id == node_id && s.output == output)
            .max_by_key(|(_, _, s)| s.start_pixel)
            .map(|(pi, si, s)| (pi, si, s.start_pixel));
        let Some((pi, si, start)) = last else {
            return Err(ApiError::bad_request(format!("No prop is wired to {label}.")));
        };
        if count <= start {
            return Err(ApiError::bad_request(format!(
                "Only {count} pixels answer on {label}, but the last prop starts at pixel {}. \
                 Check the wiring before changing counts.",
                start + 1
            )));
        }
        let prop = &mut show.props[pi];
        let seg_len = prop.segments[si].pixel_count;
        let new_len = count - start;
        let seg_end = prop.segments[si].prop_offset + seg_len;
        if seg_end != prop.pixel_count {
            return Err(ApiError::bad_request(format!(
                "\"{}\" continues on another output, so its count can't be changed here. \
                 Edit its wiring instead.",
                prop.name
            )));
        }
        prop.segments[si].pixel_count = new_len;
        prop.pixel_count = prop.pixel_count - seg_len + new_len;
        prop.suspect_pixels.retain(|&i| i < prop.pixel_count);
        if let Some(l) = prop.layout.as_mut() {
            if l.points.as_ref().is_some_and(|p| p.len() as u32 != prop.pixel_count) {
                l.points = None;
            }
        }
        summary = format!("\"{}\" now has {} pixels", prop.name, prop.pixel_count);
    }
    Ok(summary)
}

/// Apply the selected proposals of `run` to `show`. Returns one summary line
/// per applied proposal.
pub fn apply_proposals(show: &mut Show, run: &MappingRun, ids: &[String]) -> ApiResult<Vec<String>> {
    let results = run
        .results
        .as_ref()
        .ok_or_else(|| ApiError::bad_request("This run has no results yet."))?;
    let mut done = Vec::new();
    for id in ids {
        let p = results
            .proposals
            .iter()
            .find(|p| &p.id == id)
            .ok_or_else(|| ApiError::not_found("A selected proposal"))?;
        match p.kind.as_str() {
            "swap" => {
                let d: SwapData = data(p)?;
                let a = seg_mut(show, &d.a)?.clone();
                let b = seg_mut(show, &d.b)?.clone();
                {
                    let sa = seg_mut(show, &d.a)?;
                    sa.node_id = b.node_id.clone();
                    sa.output = b.output;
                    sa.start_pixel = b.start_pixel;
                    sa.null_pixels = b.null_pixels;
                }
                {
                    let sb = seg_mut(show, &d.b)?;
                    sb.node_id = a.node_id.clone();
                    sb.output = a.output;
                    sb.start_pixel = a.start_pixel;
                    sb.null_pixels = a.null_pixels;
                }
                done.push(p.message.clone());
            }
            "reverse" => {
                let d: SegRef = data(p)?;
                let s = seg_mut(show, &d)?;
                s.reverse = !s.reverse;
                done.push(p.message.clone());
            }
            "pixelCount" => {
                let d: CountData = data(p)?;
                done.push(apply_count(
                    show,
                    &d.node_id,
                    d.output,
                    d.count,
                    &d.dead,
                    "camera",
                    d.update_prop_count,
                )?);
            }
            "layout" => {
                let d: LayoutData = data(p)?;
                let prop = show
                    .props
                    .iter_mut()
                    .find(|x| x.id == d.prop_id)
                    .ok_or_else(|| ApiError::not_found("A prop in this proposal"))?;
                let mut l = d.layout;
                let finite = [l.x, l.y, l.w, l.h, l.rotation].iter().all(|v| v.is_finite());
                if !finite || l.w < 0.0 || l.h < 0.0 {
                    return Err(ApiError::bad_request("The proposed layout is invalid."));
                }
                if let Some(pts) = &mut l.points {
                    if pts.len() as u32 != prop.pixel_count {
                        l.points = None;
                    } else {
                        for pt in pts.iter_mut() {
                            pt[0] = if pt[0].is_finite() { pt[0].clamp(0.0, 1.0) } else { 0.5 };
                            pt[1] = if pt[1].is_finite() { pt[1].clamp(0.0, 1.0) } else { 0.5 };
                        }
                    }
                }
                l.source = Some(LayoutSource::Camera);
                prop.layout = Some(l);
                done.push(p.message.clone());
            }
            // "notSeen" and unknown kinds are information only.
            _ => {}
        }
    }
    check_overlaps(show)?;
    Ok(done)
}

// ---------------------------------------------------------------------------
// Receiver wizard helpers (pure)
// ---------------------------------------------------------------------------

/// Identify signals, easy to tell apart whatever the strip's colour order:
/// white or blue, blinking 1–4 times.
pub const SIGNALS: [(&str, u8); 8] = [
    ("#707070", 1),
    ("#707070", 2),
    ("#707070", 3),
    ("#707070", 4),
    ("#0000c0", 1),
    ("#0000c0", 2),
    ("#0000c0", 3),
    ("#0000c0", 4),
];

/// Signals for candidate jacks: unique while there are at most 8, else
/// shared (the answer narrows the list for another round).
pub fn assign_signals(candidates: &[u32]) -> Vec<(u32, &'static str, u8)> {
    candidates
        .iter()
        .enumerate()
        .map(|(i, &j)| {
            let (c, b) = SIGNALS[i % SIGNALS.len()];
            (j, c, b)
        })
        .collect()
}

/// Jacks on `node` with no receiver.
pub fn free_jacks(show: &Show, node_id: &str) -> Vec<u32> {
    let Some(node) = show.node(node_id) else {
        return vec![];
    };
    let jacks = node.board.jack_count().max(node.outputs.len() / 4) as u32;
    (1..=jacks)
        .filter(|j| {
            !show
                .receivers
                .iter()
                .any(|r| r.node_id == node_id && r.jack == *j)
        })
        .collect()
}

/// The strip's real colour order, from what the user saw when the wizard
/// sent pure red and pure green through the output's configured order.
/// `seen_*` are 0 = red, 1 = green, 2 = blue.
pub fn detect_color_order(configured: ColorOrder, seen_red: usize, seen_green: usize) -> Option<ColorOrder> {
    if seen_red > 2 || seen_green > 2 || seen_red == seen_green {
        return None;
    }
    let src = configured.source_indices();
    // Wire byte position carrying logical colour c.
    let pos = |c: usize| src.iter().position(|&s| s == c).unwrap_or(0);
    let mut truth = [usize::MAX; 3];
    truth[pos(0)] = seen_red;
    truth[pos(1)] = seen_green;
    let rest = 3 - seen_red - seen_green;
    truth[pos(2)] = rest;
    [
        ColorOrder::RGB,
        ColorOrder::RBG,
        ColorOrder::GRB,
        ColorOrder::GBR,
        ColorOrder::BRG,
        ColorOrder::BGR,
    ]
    .into_iter()
    .find(|o| o.source_indices() == truth)
}

/// Drop sessions nobody used for [`SESSION_TTL_S`].
pub fn expire_sessions(state: &AppState) {
    let ttl = std::time::Duration::from_secs(SESSION_TTL_S);
    state
        .services
        .mapping
        .counts
        .lock()
        .retain(|_, s| s.last_used.elapsed() < ttl);
    state
        .services
        .mapping
        .wizards
        .lock()
        .retain(|_, s| s.last_used.elapsed() < ttl);
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;
    use pixelplus_core::model::{BoardKind, Node, NodeRole, Prop, PropKind, Receiver, ReceiverKind};

    pub fn show() -> Show {
        let mut s = Show::default();
        s.nodes.push(Node {
            id: "lead".into(),
            name: "Garage".into(),
            hostname: "garage".into(),
            role: NodeRole::Leader,
            board: BoardKind::Difftxlarge,
            board_rev: None,
            pi_model: None,
            outputs: BoardKind::Difftxlarge.default_outputs(),
            adopted: true,
            last_seen: None,
            notes: None,
            serial: None,
            hardware_history: vec![],
        });
        let prop = |id: &str, n: u32, out: u32, start: u32| Prop {
            id: id.into(),
            name: format!("Prop {id}"),
            kind: PropKind::Arch,
            pixel_count: n,
            xlights_model: None,
            channel_start: 0,
            channels_per_pixel: 3,
            channel_runs: None,
            segments: vec![PropSegment {
                node_id: "lead".into(),
                output: out,
                start_pixel: start,
                pixel_count: n,
                prop_offset: 0,
                reverse: false,
                null_pixels: 0,
            }],
            group_ids: vec![],
            layout: None,
            matrix: None,
            color: None,
            max_milliamps_per_pixel: None,
            notes: None,
            suspect_pixels: vec![],
        };
        s.props.push(prop("a", 50, 1, 0));
        s.props.push(prop("b", 30, 1, 50));
        s.props.push(prop("c", 100, 2, 0));
        s.receivers.push(Receiver {
            id: "r1".into(),
            name: "R1".into(),
            kind: ReceiverKind::Diffrx,
            node_id: "lead".into(),
            jack: 1,
            location: None,
            fuse_amps: None,
            notes: None,
            main_fuse_amps: None,
        });
        s
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::show;
    use super::*;

    fn run_with(show: &Show, proposals: Vec<Proposal>) -> MappingRun {
        let outs = scope_targets(show, &MapScope { all: true, ..Default::default() });
        let (plan, targets) =
            build_plan(show, &outs, &PlanOptions::default(), 3, |_| None).unwrap();
        MappingRun {
            id: "run1".into(),
            kind: "map".into(),
            started_at: "2026-09-30T20:00:00-05:00".into(),
            scope: MapScope::default(),
            plan,
            targets,
            results: Some(RunResults {
                proposals,
                ..Default::default()
            }),
            applied_snapshot_id: None,
            applied_proposal_ids: vec![],
            photo: false,
        }
    }

    fn prop_json(id: &str, kind: &str, data: Value) -> Proposal {
        Proposal {
            id: id.into(),
            kind: kind.into(),
            prop_id: None,
            message: format!("{kind} {id}"),
            data: Some(data),
        }
    }

    #[test]
    fn plan_covers_wired_outputs_in_order() {
        let s = show();
        let outs = scope_targets(&s, &MapScope { all: true, ..Default::default() });
        assert_eq!(outs, vec![("lead".into(), 1), ("lead".into(), 2)]);
        let (plan, targets) = build_plan(&s, &outs, &PlanOptions::default(), 3, |_| None).unwrap();
        assert_eq!(plan.targets[0].max_pixels, 80);
        assert_eq!(plan.targets[1].max_pixels, 100);
        assert_eq!(plan.pixel_bits, 7);
        assert_eq!(targets[0].label, "Garage J1-1");
        assert_eq!(targets[0].prop_ids, vec!["a", "b"]);
        let only_c = scope_targets(&s, &MapScope { prop_ids: vec!["c".into()], ..Default::default() });
        assert_eq!(only_c, vec![("lead".into(), 2)]);
        let bits = codebook_bits(&plan);
        assert_eq!(bits.len(), 2);
        assert_eq!(bits[0].iter().filter(|&&b| b == 1).count(), 6);
    }

    #[test]
    fn probe_length_rule() {
        assert_eq!(probe_len(50, None), 114);
        assert_eq!(probe_len(1000, None), 1250);
        assert_eq!(probe_len(1000, Some(1100)), 1100);
        assert_eq!(probe_len(4000, None), 4096);
    }

    #[test]
    fn swap_and_reverse_apply() {
        let mut s = show();
        // Same-length props swap cleanly.
        s.props[2].pixel_count = 50;
        s.props[2].segments[0].pixel_count = 50;
        let run = run_with(
            &s,
            vec![
                prop_json("p1", "swap", serde_json::json!({"a": {"propId": "a"}, "b": {"propId": "c"}})),
                prop_json("p2", "reverse", serde_json::json!({"propId": "b", "segment": 0})),
                prop_json("p3", "notSeen", serde_json::json!({})),
            ],
        );
        let done = apply_proposals(&mut s, &run, &["p1".into(), "p2".into(), "p3".into()]).unwrap();
        assert_eq!(done.len(), 2);
        assert_eq!(s.prop("a").unwrap().segments[0].output, 2);
        assert_eq!(s.prop("c").unwrap().segments[0].output, 1);
        assert!(s.prop("b").unwrap().segments[0].reverse);
    }

    #[test]
    fn overlapping_result_is_rejected() {
        let mut s = show();
        s.props[2].pixel_count = 60;
        s.props[2].segments[0].pixel_count = 60;
        let run = run_with(
            &s,
            vec![prop_json("p1", "swap", serde_json::json!({"a": {"propId": "a"}, "b": {"propId": "c"}}))],
        );
        // a (50) ↔ c (60): c at output 1 start 0 now covers 0..60, b starts at 50.
        let err = apply_proposals(&mut s, &run, &["p1".into()]).unwrap_err();
        assert!(err.message.contains("overlap"), "{}", err.message);
    }

    #[test]
    fn pixel_count_resizes_last_prop_and_marks_dead() {
        let mut s = show();
        let msg = apply_count(&mut s, "lead", 1, 76, &[55, 90], "manual", true).unwrap();
        assert!(msg.contains("now has 26 pixels"), "{msg}");
        let b = s.prop("b").unwrap();
        assert_eq!(b.pixel_count, 26);
        assert_eq!(b.segments[0].pixel_count, 26);
        assert_eq!(b.suspect_pixels, vec![5]);
        let m = s.node("lead").unwrap().outputs[0].measured_pixels.clone().unwrap();
        assert_eq!(m.count, 76);
        assert_eq!(m.dead, vec![55]);
        assert!(apply_count(&mut s, "lead", 1, 40, &[], "manual", true).is_err());
        assert!(apply_count(&mut s, "lead", 9, 40, &[], "manual", true).is_err());
        // Without resizing only the measurement is stored.
        apply_count(&mut s, "lead", 2, 90, &[], "camera", false).unwrap();
        assert_eq!(s.prop("c").unwrap().pixel_count, 100);
    }

    #[test]
    fn layout_proposal_marks_camera_source() {
        let mut s = show();
        let run = run_with(
            &s,
            vec![prop_json(
                "l",
                "layout",
                serde_json::json!({"propId": "b", "layout": {"x": 1.0, "y": 2.0, "w": 3.0, "h": 1.0, "points": [[0.5, 2.0]]}}),
            )],
        );
        apply_proposals(&mut s, &run, &["l".into()]).unwrap();
        let l = s.prop("b").unwrap().layout.clone().unwrap();
        assert_eq!(l.source, Some(LayoutSource::Camera));
        assert!(l.points.is_none(), "wrong point count dropped");
    }

    #[test]
    fn signals_and_free_jacks() {
        let s = show();
        let free = free_jacks(&s, "lead");
        assert_eq!(free.len(), 14);
        assert!(!free.contains(&1));
        let sig = assign_signals(&free);
        let unique: std::collections::HashSet<_> = sig[..8].iter().map(|x| (x.1, x.2)).collect();
        assert_eq!(unique.len(), 8);
        assert_eq!((sig[8].1, sig[8].2), (sig[0].1, sig[0].2));
    }

    #[test]
    fn color_order_detection() {
        // Configured RGB, strip is GRB: red shows green, green shows red.
        assert_eq!(detect_color_order(ColorOrder::RGB, 1, 0), Some(ColorOrder::GRB));
        assert_eq!(detect_color_order(ColorOrder::RGB, 0, 1), Some(ColorOrder::RGB));
        // Configured GRB and correct: colours look right.
        assert_eq!(detect_color_order(ColorOrder::GRB, 0, 1), Some(ColorOrder::GRB));
        // Configured GRB but strip is RGB: red shows green.
        assert_eq!(detect_color_order(ColorOrder::GRB, 1, 0), Some(ColorOrder::RGB));
        // Every combination round-trips.
        let all = [
            ColorOrder::RGB,
            ColorOrder::RBG,
            ColorOrder::GRB,
            ColorOrder::GBR,
            ColorOrder::BRG,
            ColorOrder::BGR,
        ];
        for conf in all {
            for truth in all {
                // Simulate: logical c goes to wire pos p (conf), strip shows truth[p].
                let pos = |c: usize| conf.source_indices().iter().position(|&s| s == c).unwrap();
                let seen = |c: usize| truth.source_indices()[pos(c)];
                assert_eq!(detect_color_order(conf, seen(0), seen(1)), Some(truth));
            }
        }
        assert_eq!(detect_color_order(ColorOrder::RGB, 1, 1), None);
    }
}
