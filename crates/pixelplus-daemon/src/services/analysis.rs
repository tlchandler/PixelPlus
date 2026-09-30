//! Background jobs for audio analysis, auto light shows and browser previews
//! (F2, F3; ARCHITECTURE §12.2–12.3).
//!
//! One worker thread (`pp-analysis`, `nice 10`) runs one job at a time, so a
//! Pi's playback threads always win the CPU. Jobs the owner is waiting for
//! (auto show, preview, "analyze again") run before background work (analysis
//! of new uploads, regenerating auto shows after a layout change). On a Pi
//! Zero, background work waits while a scheduled show is running.
//!
//! Every job reports progress as a WebSocket `job` message
//! `{id, kind: "analysis"|"autoshow"|"preview", pct, state:
//! "queued"|"running"|"done"|"failed", result?: {sequenceId?, message?},
//! subject?}` (`subject` = the media or sequence id it is about); the latest
//! status of recent jobs is also served by `GET /jobs/:id`.
//!
//! Files: `media/<id>.analysis.json` (full [`Analysis`]), generated sequences
//! in `sequences/`, previews in `cache/preview/<seqId>-<mappingHash>.pppv`
//! (500 MB quota, least recently used first), temporary auto-show previews in
//! `cache/autoshow/tmp-<id>.fseq` (deleted after an hour).

use crate::api::ApiError;
use crate::services::{media as media_svc, paths};
use crate::state::AppState;
use parking_lot::{Condvar, Mutex};
use pixelplus_core::audio_analysis::{self, Analysis, Analyzer};
use pixelplus_core::autoshow;
use pixelplus_core::model::{new_id, GeneratedInfo, GeneratedKind, Sequence};
use pixelplus_core::preview::{self, PreviewHeader};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// Preview cache size limit.
pub const PREVIEW_QUOTA: u64 = 500 * 1024 * 1024;
/// Temporary auto-show previews are deleted after this.
pub const TEMP_KEEP: Duration = Duration::from_secs(60 * 60);
/// Recent job statuses kept for `GET /jobs/:id`.
const KEEP_JOBS: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobKind {
    Analysis,
    Autoshow,
    Preview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobState {
    Queued,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence_id: Option<String>,
    /// Friendly text (the reason when it failed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// WebSocket `job` message / `GET /jobs/:id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobStatus {
    pub id: String,
    pub kind: JobKind,
    pub pct: u8,
    pub state: JobState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<JobResult>,
    /// Media or sequence id the job is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
}

/// Where an auto show goes.
#[derive(Debug, Clone, PartialEq)]
pub enum AutoTarget {
    /// A new sequence in the library.
    New { name: Option<String> },
    /// Re-render an existing generated sequence in place (same id).
    Regenerate(String),
    /// A temporary `tmp-…` sequence for previewing before creating.
    Temp(String),
}

#[derive(Debug, Clone)]
pub struct AutoShowTask {
    pub media_id: String,
    pub style: String,
    pub prop_ids: Vec<String>,
    pub seed: u32,
    pub target: AutoTarget,
}

#[derive(Debug, Clone)]
enum Task {
    Analyze { media_id: String },
    AutoShow(AutoShowTask),
    Preview(PreviewSource),
}

/// What a preview is built from.
#[derive(Debug, Clone)]
struct PreviewSource {
    seq_id: String,
    fseq: PathBuf,
    hash: String,
    frame_ms: u32,
    out: PathBuf,
}

/// A rendered temporary auto show (`POST /autoshow/preview`).
#[derive(Debug, Clone)]
struct Temp {
    fseq: PathBuf,
    hash: String,
    frame_ms: u32,
    made: Instant,
}

#[derive(Default)]
struct Inner {
    interactive: VecDeque<(String, Task)>,
    background: VecDeque<(String, Task)>,
    jobs: HashMap<String, JobStatus>,
    finished: VecDeque<String>,
    /// Dedupe key ("analysis:<media>", "preview:<file>", "autoshow:<seq>") → job id.
    keys: HashMap<String, String>,
    /// Media removed while queued / running.
    cancelled: HashSet<String>,
    /// Media whose analysis failed (not retried by the sweep).
    failed_media: HashSet<String>,
    temps: HashMap<String, Temp>,
    headers: HashMap<PathBuf, Arc<PreviewHeader>>,
}

/// Runtime state of this service (`state.services.analysis`).
#[derive(Default)]
pub struct AnalysisState {
    inner: Mutex<Inner>,
    wake: Condvar,
    started: OnceLock<()>,
}

// ---------------------------------------------------------------------------
// Public API (handlers, content import)
// ---------------------------------------------------------------------------

/// `media/<id>.analysis.json`.
pub fn analysis_path(state: &AppState, media_id: &str) -> Option<PathBuf> {
    paths::safe_name(media_id).then(|| {
        state
            .config
            .media_dir()
            .join(format!("{media_id}.analysis.json"))
    })
}

/// The stored full analysis of a media item, if it is ready.
pub fn load_analysis(state: &AppState, media_id: &str) -> Option<Analysis> {
    let p = analysis_path(state, media_id)?;
    let bytes = std::fs::read(p).ok()?;
    serde_json::from_slice::<Analysis>(&bytes)
        .ok()
        .filter(|a| a.v == audio_analysis::VERSION)
}

/// Queue the beat analysis of a media item (background priority; no-op when
/// already queued). Returns the job id.
pub fn enqueue_analysis(state: &AppState, media_id: &str) -> Option<String> {
    enqueue(
        state,
        Task::Analyze {
            media_id: media_id.to_string(),
        },
        false,
    )
}

/// Same, ahead of background work ("Analyze again").
pub fn enqueue_analysis_now(state: &AppState, media_id: &str) -> Option<String> {
    let svc = &state.services.analysis;
    {
        let mut g = svc.inner.lock();
        g.failed_media.remove(media_id);
        // Promote a queued background job.
        if let Some(pos) = g
            .background
            .iter()
            .position(|(_, t)| matches!(t, Task::Analyze { media_id: m } if m == media_id))
        {
            if let Some(job) = g.background.remove(pos) {
                let id = job.0.clone();
                g.interactive.push_back(job);
                svc.wake.notify_all();
                return Some(id);
            }
        }
    }
    enqueue(
        state,
        Task::Analyze {
            media_id: media_id.to_string(),
        },
        true,
    )
}

/// Queue an auto show. Returns the job id.
pub fn enqueue_autoshow(state: &AppState, task: AutoShowTask, interactive: bool) -> Option<String> {
    enqueue(state, Task::AutoShow(task), interactive)
}

/// A media item was deleted or replaced: stop its jobs, drop its analysis file.
pub fn forget(state: &AppState, media_id: &str) {
    let svc = &state.services.analysis;
    {
        let mut g = svc.inner.lock();
        g.cancelled.insert(media_id.to_string());
        g.failed_media.remove(media_id);
        let is_it = |t: &Task| matches!(t, Task::Analyze { media_id: m } if m == media_id);
        g.background.retain(|(_, t)| !is_it(t));
        g.interactive.retain(|(_, t)| !is_it(t));
        g.keys.remove(&format!("analysis:{media_id}"));
    }
    if let Some(p) = analysis_path(state, media_id) {
        let _ = std::fs::remove_file(p);
    }
}

/// A sequence was deleted: drop its cached previews.
pub fn forget_sequence(state: &AppState, seq_id: &str) {
    if !paths::safe_name(seq_id) {
        return;
    }
    let dir = preview_dir(state);
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with(&format!("{seq_id}-")) && name.ends_with(".pppv") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    state
        .services
        .analysis
        .inner
        .lock()
        .headers
        .retain(|p, _| !p.starts_with(&dir) || !file_is_for(p, seq_id));
}

fn file_is_for(p: &Path, seq_id: &str) -> bool {
    p.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with(&format!("{seq_id}-")))
}

/// Latest status of a job.
pub fn job(state: &AppState, id: &str) -> Option<JobStatus> {
    state.services.analysis.inner.lock().jobs.get(id).cloned()
}

/// Queued and running jobs, oldest first.
pub fn active_jobs(state: &AppState) -> Vec<JobStatus> {
    let g = state.services.analysis.inner.lock();
    let mut v: Vec<JobStatus> = g
        .jobs
        .values()
        .filter(|j| matches!(j.state, JobState::Queued | JobState::Running))
        .cloned()
        .collect();
    v.sort_by(|a, b| {
        b.state
            .eq(&JobState::Running)
            .cmp(&a.state.eq(&JobState::Running))
    });
    v
}

/// A temporary sequence id from `POST /autoshow/preview`.
pub fn is_temp_id(id: &str) -> bool {
    id.starts_with("tmp-") && paths::safe_name(id)
}

/// Result of [`preview`].
pub enum PreviewLookup {
    /// The file is cached: serve it.
    Ready {
        path: PathBuf,
        header: Arc<PreviewHeader>,
    },
    /// Being built (the job was queued if needed).
    Building(JobStatus),
    /// No such sequence, or its file is missing.
    Missing(String),
}

/// Find (or start building) the preview of a sequence.
pub fn preview(state: &AppState, seq_id: &str) -> PreviewLookup {
    let src = match preview_source(state, seq_id) {
        Ok(s) => s,
        Err(e) => return PreviewLookup::Missing(e),
    };
    let svc = &state.services.analysis;
    if src.out.is_file() {
        let cached = svc.inner.lock().headers.get(&src.out).cloned();
        let header = match cached {
            Some(h) => Some(h),
            None => preview::read_header(&src.out).ok().map(Arc::new),
        };
        if let Some(header) = header {
            touch(&src.out);
            let mut g = svc.inner.lock();
            if g.headers.len() > 64 {
                g.headers.clear();
            }
            g.headers.insert(src.out.clone(), header.clone());
            return PreviewLookup::Ready {
                path: src.out,
                header,
            };
        }
        // Unreadable (old version, torn write): rebuild.
        let _ = std::fs::remove_file(&src.out);
    }
    let key = format!("preview:{}", src.out.display());
    if let Some(st) = {
        let g = svc.inner.lock();
        g.keys.get(&key).and_then(|id| g.jobs.get(id)).cloned()
    } {
        if matches!(st.state, JobState::Queued | JobState::Running) {
            return PreviewLookup::Building(st);
        }
    }
    match enqueue(state, Task::Preview(src), true).and_then(|id| job(state, &id)) {
        Some(st) => PreviewLookup::Building(st),
        None => PreviewLookup::Missing("The preview couldn't be started.".into()),
    }
}

fn preview_dir(state: &AppState) -> PathBuf {
    state.config.data_dir.join("cache").join("preview")
}

fn temp_dir(state: &AppState) -> PathBuf {
    state.config.data_dir.join("cache").join("autoshow")
}

fn preview_source(state: &AppState, seq_id: &str) -> Result<PreviewSource, String> {
    let show = state.store.get();
    let (fseq, hash, frame_ms) = if is_temp_id(seq_id) {
        let g = state.services.analysis.inner.lock();
        let t = g
            .temps
            .get(seq_id)
            .ok_or("That preview has expired. Make it again.")?;
        (t.fseq.clone(), t.hash.clone(), t.frame_ms)
    } else {
        let seq = show
            .sequence(seq_id)
            .ok_or("That sequence doesn't exist.")?;
        let fseq = paths::resolve(&state.config.data_dir, &seq.file, paths::Kind::Sequence)
            .filter(|p| p.is_file())
            .ok_or("The sequence file is missing. Upload it again.")?;
        let hash = if seq.hash.len() >= 16 {
            seq.hash.clone()
        } else {
            // Older uploads without a hash: size + mtime identify the file.
            let m = std::fs::metadata(&fseq).map_err(|e| e.to_string())?;
            let mt = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            format!("{}-{mt}", m.len())
        };
        (fseq, hash, seq.frame_ms)
    };
    if !paths::safe_name(seq_id) {
        return Err("That sequence doesn't exist.".into());
    }
    let mh = preview::mapping_hash(&hash, frame_ms, &show.props);
    let out = preview_dir(state).join(format!("{seq_id}-{mh}.pppv"));
    Ok(PreviewSource {
        seq_id: seq_id.to_string(),
        fseq,
        hash,
        frame_ms,
        out,
    })
}

fn touch(p: &Path) {
    if let Ok(f) = std::fs::File::options().append(true).open(p) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
}

/// Is this a low-power board (Pi Zero), where background work waits for
/// the show to end?
pub fn low_power() -> bool {
    static LOW: OnceLock<bool> = OnceLock::new();
    *LOW.get_or_init(|| {
        crate::cluster::net::pi_model().is_some_and(|m| m.contains("Zero"))
            || std::thread::available_parallelism().is_ok_and(|n| n.get() < 2)
    })
}

fn show_running(state: &AppState) -> bool {
    state
        .services
        .player
        .get()
        .is_some_and(|p| p.status().schedule_entry.is_some())
}

// ---------------------------------------------------------------------------
// Queue
// ---------------------------------------------------------------------------

fn task_key(t: &Task) -> (String, JobKind, String) {
    match t {
        Task::Analyze { media_id } => (
            format!("analysis:{media_id}"),
            JobKind::Analysis,
            media_id.clone(),
        ),
        Task::AutoShow(a) => match &a.target {
            AutoTarget::Regenerate(id) => (format!("autoshow:{id}"), JobKind::Autoshow, id.clone()),
            AutoTarget::Temp(id) => (
                format!("autoshow:{id}"),
                JobKind::Autoshow,
                a.media_id.clone(),
            ),
            AutoTarget::New { .. } => (
                format!("autoshow:new:{}", new_id()),
                JobKind::Autoshow,
                a.media_id.clone(),
            ),
        },
        Task::Preview(p) => (
            format!("preview:{}", p.out.display()),
            JobKind::Preview,
            p.seq_id.clone(),
        ),
    }
}

fn enqueue(state: &AppState, task: Task, interactive: bool) -> Option<String> {
    let svc = &state.services.analysis;
    let (key, kind, subject) = task_key(&task);
    let status = {
        let mut g = svc.inner.lock();
        if let Some(id) = g.keys.get(&key) {
            if g.jobs
                .get(id)
                .is_some_and(|j| matches!(j.state, JobState::Queued | JobState::Running))
            {
                return Some(id.clone());
            }
        }
        if let Task::Analyze { media_id } = &task {
            g.cancelled.remove(media_id);
        }
        let id = new_id();
        let st = JobStatus {
            id: id.clone(),
            kind,
            pct: 0,
            state: JobState::Queued,
            result: None,
            subject: Some(subject),
        };
        g.keys.insert(key, id.clone());
        g.jobs.insert(id.clone(), st.clone());
        if interactive {
            g.interactive.push_back((id, task));
        } else {
            g.background.push_back((id, task));
        }
        st
    };
    svc.wake.notify_all();
    state.events.publish("job", &status);
    Some(status.id)
}

fn update(state: &AppState, id: &str, f: impl FnOnce(&mut JobStatus)) {
    let svc = &state.services.analysis;
    let st = {
        let mut g = svc.inner.lock();
        let Some(j) = g.jobs.get_mut(id) else { return };
        f(j);
        let st = j.clone();
        if matches!(st.state, JobState::Done | JobState::Failed) {
            g.finished.push_back(id.to_string());
            while g.finished.len() > KEEP_JOBS {
                if let Some(old) = g.finished.pop_front() {
                    g.jobs.remove(&old);
                    g.keys.retain(|_, v| *v != old);
                }
            }
        }
        st
    };
    state.events.publish("job", &st);
}

/// Progress reporter: publishes at most every 3 % / 400 ms.
struct Progress<'a> {
    state: &'a AppState,
    id: &'a str,
    last_pct: u8,
    last_at: Instant,
    lo: f32,
    hi: f32,
}

impl<'a> Progress<'a> {
    fn new(state: &'a AppState, id: &'a str) -> Self {
        Progress {
            state,
            id,
            last_pct: 0,
            last_at: Instant::now(),
            lo: 0.0,
            hi: 1.0,
        }
    }

    /// Map the next stage's 0..1 onto `lo..hi` of the job.
    fn stage(&mut self, lo: f32, hi: f32) {
        self.lo = lo;
        self.hi = hi;
    }

    fn set(&mut self, frac: f32) {
        let pct = ((self.lo + (self.hi - self.lo) * frac.clamp(0.0, 1.0)) * 100.0).min(99.0) as u8;
        if pct >= self.last_pct.saturating_add(3)
            || (pct > self.last_pct && self.last_at.elapsed() > Duration::from_millis(400))
        {
            self.last_pct = pct;
            self.last_at = Instant::now();
            update(self.state, self.id, |j| {
                j.pct = pct;
                j.state = JobState::Running;
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------

/// Start the service (called once from `services::start_all`).
pub fn start(state: &AppState) {
    let svc = &state.services.analysis;
    if svc.started.set(()).is_err() {
        return;
    }
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        tracing::warn!("analysis: no async runtime; jobs will not run");
        return;
    };
    let st = state.clone();
    let rt2 = rt.clone();
    let spawned = std::thread::Builder::new()
        .name("pp-analysis".into())
        .spawn(move || {
            lower_priority();
            loop {
                let (id, task) = next_task(&st);
                run(&st, &rt2, &id, task);
            }
        });
    if let Err(e) = spawned {
        tracing::error!("analysis: couldn't start the worker: {e}");
        return;
    }
    let st = state.clone();
    rt.spawn(async move {
        // First sweep shortly after start, then on show changes (debounced)
        // and every few minutes.
        tokio::time::sleep(Duration::from_secs(5)).await;
        let mut changes = st.store.subscribe();
        loop {
            let s2 = st.clone();
            let _ = tokio::task::spawn_blocking(move || maintenance(&s2)).await;
            tokio::select! {
                _ = changes.changed() => tokio::time::sleep(Duration::from_secs(20)).await,
                _ = tokio::time::sleep(Duration::from_secs(300)) => {}
            }
        }
    });
}

/// `nice 10` for this thread (Linux threads have their own nice value).
fn lower_priority() {
    #[cfg(target_os = "linux")]
    unsafe {
        // SAFETY: plain syscalls on the calling thread; failure is harmless.
        let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
        let _ = libc::setpriority(libc::PRIO_PROCESS, tid, 10);
    }
}

fn next_task(state: &AppState) -> (String, Task) {
    let svc = &state.services.analysis;
    let mut g = svc.inner.lock();
    loop {
        if let Some(t) = g.interactive.pop_front() {
            return t;
        }
        if !g.background.is_empty() {
            let paused = low_power() && {
                drop(g);
                let r = show_running(state);
                g = svc.inner.lock();
                r
            };
            if !paused {
                if let Some(t) = g.background.pop_front() {
                    return t;
                }
                continue;
            }
            svc.wake.wait_for(&mut g, Duration::from_secs(15));
            continue;
        }
        svc.wake.wait_for(&mut g, Duration::from_secs(60));
    }
}

fn run(state: &AppState, rt: &tokio::runtime::Handle, id: &str, task: Task) {
    update(state, id, |j| j.state = JobState::Running);
    let started = Instant::now();
    let what = match &task {
        Task::Analyze { .. } => "analysis",
        Task::AutoShow(_) => "auto show",
        Task::Preview(_) => "preview",
    };
    let res = match task {
        Task::Analyze { media_id } => {
            run_analysis(state, rt, id, &media_id).map(|_| JobResult::default())
        }
        Task::AutoShow(t) => run_autoshow(state, rt, id, t),
        Task::Preview(p) => run_preview(state, id, p),
    };
    match res {
        Ok(result) => {
            tracing::info!(
                "{what} job {id} done in {:.1} s",
                started.elapsed().as_secs_f32()
            );
            update(state, id, |j| {
                j.pct = 100;
                j.state = JobState::Done;
                j.result = Some(result);
            });
        }
        Err(msg) => {
            tracing::warn!("{what} job {id} failed: {msg}");
            update(state, id, |j| {
                j.state = JobState::Failed;
                j.result = Some(JobResult {
                    sequence_id: None,
                    message: Some(msg),
                });
            });
        }
    }
}

fn run_analysis(
    state: &AppState,
    rt: &tokio::runtime::Handle,
    id: &str,
    media_id: &str,
) -> Result<Analysis, String> {
    let mut prog = Progress::new(state, id);
    analyze_media(state, rt, media_id, &mut prog)
}

/// Decode + analyse a media item, store the JSON and the summary.
fn analyze_media(
    state: &AppState,
    rt: &tokio::runtime::Handle,
    media_id: &str,
    prog: &mut Progress,
) -> Result<Analysis, String> {
    let show = state.store.get();
    let m = show
        .media_item(media_id)
        .ok_or("That audio file no longer exists.")?;
    let path = paths::resolve(&state.config.data_dir, &m.file, paths::Kind::Media)
        .filter(|p| p.is_file())
        .ok_or("The audio file is missing. Upload it again.")?;
    let expected = m.duration_ms.clamp(1, audio_analysis::MAX_DURATION_MS);
    let started = Instant::now();
    let mut an: Option<Analyzer> = None;
    let mut cancelled = false;
    let svc = &state.services.analysis;
    let decoded = media_svc::decode_mono(&path, &mut |rate, s| {
        let a = an.get_or_insert_with(|| Analyzer::new(rate));
        a.push(s);
        prog.set(0.9 * a.duration_ms() as f32 / expected as f32);
        if svc.inner.lock().cancelled.contains(media_id) {
            cancelled = true;
            return false;
        }
        !a.is_full()
    });
    if cancelled {
        return Err("Cancelled: the audio file was removed.".into());
    }
    if let Err(e) = decoded {
        svc.inner.lock().failed_media.insert(media_id.to_string());
        return Err(format!("We couldn't read that audio file ({e})."));
    }
    let an = an.ok_or("The file contains no audio.")?;
    let decode_s = started.elapsed().as_secs_f32();
    let analysis = an.finish();
    tracing::info!(
        "analysed {media_id}: {:.0} s of audio, {} BPM ({:.0} %), {} beats, {} sections; decode {:.1} s, total {:.1} s",
        analysis.duration_ms as f32 / 1000.0,
        analysis.bpm,
        analysis.bpm_confidence * 100.0,
        analysis.beats.len(),
        analysis.sections.len(),
        decode_s,
        started.elapsed().as_secs_f32()
    );
    let p = analysis_path(state, media_id).ok_or("Bad media id.")?;
    write_atomic(
        &p,
        &serde_json::to_vec(&analysis).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("Couldn't save the analysis ({e})."))?;
    let summary = analysis.summary();
    let mid = media_id.to_string();
    rt.block_on(state.store.update(move |show| {
        if let Some(m) = show.media.iter_mut().find(|m| m.id == mid) {
            m.analysis = Some(summary);
        }
        Ok(())
    }))
    .map_err(|e| e.message)?;
    prog.set(1.0);
    Ok(analysis)
}

fn write_atomic(p: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_extension("json.tmp");
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::rename(&tmp, p)
}

fn style_name(id: &str) -> String {
    autoshow::styles()
        .into_iter()
        .find(|s| s.id == id)
        .map_or_else(|| id.to_string(), |s| s.name)
}

fn run_autoshow(
    state: &AppState,
    rt: &tokio::runtime::Handle,
    id: &str,
    t: AutoShowTask,
) -> Result<JobResult, String> {
    let mut prog = Progress::new(state, id);
    let analysis = match load_analysis(state, &t.media_id) {
        Some(a) => a,
        None => {
            prog.stage(0.0, 0.4);
            analyze_media(state, rt, &t.media_id, &mut prog)?
        }
    };
    let temp = matches!(t.target, AutoTarget::Temp(_));
    prog.stage(0.4, if temp { 0.7 } else { 0.95 });
    let show = state.store.get();
    let media = show
        .media_item(&t.media_id)
        .ok_or("That song no longer exists.")?
        .clone();
    let seq_id = match &t.target {
        AutoTarget::New { .. } => new_id(),
        AutoTarget::Regenerate(s) | AutoTarget::Temp(s) => s.clone(),
    };
    if !paths::safe_name(&seq_id) {
        return Err("Bad sequence id.".into());
    }
    let (dir, rel) = if temp {
        (temp_dir(state), None)
    } else {
        (
            state.config.sequences_dir(),
            Some(format!("sequences/{seq_id}.fseq")),
        )
    };
    let _ = std::fs::create_dir_all(&dir);
    let out = dir.join(format!("{seq_id}.fseq"));
    let staging = dir.join(format!(".gen-{seq_id}.fseq"));
    let original = media
        .original_name
        .clone()
        .or_else(|| {
            media_svc::read_meta(&state.config.media_dir(), &media.id).map(|m| m.original_name)
        })
        .filter(|n| !n.is_empty());
    let req = autoshow::AutoShowRequest {
        show: &show,
        prop_ids: &t.prop_ids,
        analysis: &analysis,
        style: &t.style,
        seed: t.seed,
        duration_ms: media.duration_ms,
        media_filename: original,
    };
    let started = Instant::now();
    let rendered = autoshow::render(&req, &staging, &mut |f| {
        prog.set(f);
        true
    })
    .map_err(|e| match e {
        autoshow::AutoShowError::NoProps => {
            "There are no props to light yet. Add props on the Props page first.".to_string()
        }
        autoshow::AutoShowError::UnknownStyle(s) => {
            format!("\"{s}\" isn't a style PixelPlus knows.")
        }
        other => format!("The light show couldn't be made ({other})."),
    })?;
    tracing::info!(
        "auto show {seq_id}: {} frames × {} channels in {:.1} s",
        rendered.frame_count,
        rendered.channel_count,
        started.elapsed().as_secs_f32()
    );
    let hash = pixelplus_core::fseq::sha256_file(&staging).map_err(|e| e.to_string())?;
    std::fs::rename(&staging, &out).map_err(|e| e.to_string())?;

    if let AutoTarget::Temp(tmp) = &t.target {
        state.services.analysis.inner.lock().temps.insert(
            tmp.clone(),
            Temp {
                fseq: out,
                hash,
                frame_ms: rendered.frame_ms,
                made: Instant::now(),
            },
        );
        // Build its preview right away, so "Preview" is one step.
        prog.stage(0.7, 1.0);
        let src = preview_source(state, tmp)?;
        build_preview(state, &src, &mut prog)?;
        return Ok(JobResult {
            sequence_id: Some(tmp.clone()),
            message: None,
        });
    }

    let rel = rel.unwrap_or_default();
    // Thumbnail strip, as for uploads.
    let thumb_rel = format!("thumbnails/{seq_id}.png");
    let _ = std::fs::create_dir_all(state.config.thumbnails_dir());
    let thumbnail = crate::api::content::generate_thumbnail(
        &out,
        &show.props,
        &state.config.data_dir.join(&thumb_rel),
    )
    .ok()
    .map(|_| thumb_rel);
    let generated = GeneratedInfo {
        kind: if t.style == "voice" {
            GeneratedKind::Voice
        } else {
            GeneratedKind::AutoShow
        },
        media_id: media.id.clone(),
        style: t.style.clone(),
        prop_ids: t.prop_ids.clone(),
        seed: t.seed,
        analysis_version: analysis.v,
        props_hash: autoshow::props_hash(&show, &t.prop_ids),
    };
    let name = match &t.target {
        AutoTarget::New { name: Some(n) } if !n.trim().is_empty() => n.trim().to_string(),
        _ => format!("{} ({} light show)", media.name, style_name(&t.style)),
    };
    let target = t.target.clone();
    let sid = seq_id.clone();
    let (seq, _) = rt
        .block_on(state.store.update(move |show| {
            let fresh = Sequence {
                id: sid.clone(),
                name,
                file: rel,
                duration_ms: rendered.duration_ms,
                frame_ms: rendered.frame_ms,
                channel_count: rendered.channel_count,
                media_id: Some(generated.media_id.clone()),
                xlights_name: None,
                thumbnail,
                hash,
                generated: Some(generated),
                tags: vec!["auto".into()],
            };
            match (show.sequences.iter_mut().find(|s| s.id == sid), target) {
                (Some(s), AutoTarget::Regenerate(_)) => {
                    // Same id, name and tags: playlists stay valid.
                    let keep = (s.name.clone(), s.tags.clone());
                    *s = Sequence {
                        name: keep.0,
                        tags: keep.1,
                        ..fresh
                    };
                    Ok(s.clone())
                }
                (None, AutoTarget::Regenerate(_)) => Err(ApiError::not_found("That sequence")),
                (_, _) => {
                    show.sequences.push(fresh.clone());
                    Ok(fresh)
                }
            }
        }))
        .map_err(|e| {
            let _ = std::fs::remove_file(
                state
                    .config
                    .data_dir
                    .join(format!("sequences/{seq_id}.fseq")),
            );
            e.message
        })?;
    forget_sequence_stale(state, &seq.id);
    Ok(JobResult {
        sequence_id: Some(seq.id),
        message: None,
    })
}

/// Drop previews of a sequence whose file changed (a new hash).
fn forget_sequence_stale(state: &AppState, seq_id: &str) {
    let keep = preview_source(state, seq_id).ok().map(|s| s.out);
    let dir = preview_dir(state);
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if file_is_for(&p, seq_id) && Some(&p) != keep.as_ref() {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}

fn run_preview(state: &AppState, id: &str, src: PreviewSource) -> Result<JobResult, String> {
    let mut prog = Progress::new(state, id);
    build_preview(state, &src, &mut prog)?;
    Ok(JobResult {
        sequence_id: Some(src.seq_id),
        message: None,
    })
}

fn build_preview(state: &AppState, src: &PreviewSource, prog: &mut Progress) -> Result<(), String> {
    if src.out.is_file() {
        return Ok(());
    }
    let show = state.store.get();
    let started = Instant::now();
    let header = preview::build(
        &src.fseq,
        &src.seq_id,
        &src.hash,
        &show.props,
        &src.out,
        &mut |f| {
            prog.set(f);
            true
        },
    )
    .map_err(|e| format!("The preview couldn't be made ({e})."))?;
    let size = std::fs::metadata(&src.out).map_or(0, |m| m.len());
    tracing::info!(
        "preview {}: {} frames, {} KB in {:.1} s (frame {} ms)",
        src.seq_id,
        header.frame_count,
        size / 1024,
        started.elapsed().as_secs_f32(),
        src.frame_ms
    );
    state
        .services
        .analysis
        .inner
        .lock()
        .headers
        .insert(src.out.clone(), Arc::new(header));
    // Older previews of the same sequence (another layout / file) are stale.
    if !is_temp_id(&src.seq_id) {
        forget_sequence_stale(state, &src.seq_id);
    }
    enforce_quota(&preview_dir(state), PREVIEW_QUOTA, Some(&src.out));
    Ok(())
}

/// Delete least recently used previews until the directory fits `quota`.
pub fn enforce_quota(dir: &Path, quota: u64, keep: Option<&Path>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            m.is_file().then(|| {
                (
                    m.modified().unwrap_or(std::time::UNIX_EPOCH),
                    m.len(),
                    e.path(),
                )
            })
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort();
    for (_, len, p) in files {
        if total <= quota {
            break;
        }
        if Some(p.as_path()) == keep {
            continue;
        }
        if std::fs::remove_file(&p).is_ok() {
            total -= len;
        }
    }
}

// ---------------------------------------------------------------------------
// Maintenance sweep
// ---------------------------------------------------------------------------

/// Queue missing analyses, regenerate stale auto shows, drop expired temps
/// and previews of deleted sequences.
fn maintenance(state: &AppState) {
    let show = state.store.get();
    // Expired temporary previews.
    let expired: Vec<Temp> = {
        let mut g = state.services.analysis.inner.lock();
        let dead: Vec<String> = g
            .temps
            .iter()
            .filter(|(_, t)| t.made.elapsed() > TEMP_KEEP)
            .map(|(k, _)| k.clone())
            .collect();
        dead.iter().filter_map(|k| g.temps.remove(k)).collect()
    };
    for t in expired {
        let _ = std::fs::remove_file(&t.fseq);
    }
    // Leftover temp renders (daemon restarted) and old tmp previews.
    for dir in [temp_dir(state), preview_dir(state)] {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            let live: HashSet<String> = state
                .services
                .analysis
                .inner
                .lock()
                .temps
                .keys()
                .cloned()
                .collect();
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                let tmp_id = name.split(['.', '-']).take(2).collect::<Vec<_>>().join("-");
                let is_tmp = name.starts_with("tmp-") || name.starts_with(".gen-");
                // Files being written right now are young: leave them.
                let age = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .unwrap_or_default();
                let old_enough =
                    age > Duration::from_secs(if name.starts_with(".gen-") { 3600 } else { 600 });
                let seq_gone = !is_tmp
                    && dir == preview_dir(state)
                    && name
                        .split_once('-')
                        .is_some_and(|(sid, _)| show.sequence(sid).is_none());
                if (is_tmp && old_enough && !live.contains(&tmp_id)) || seq_gone {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }
    // Media without a (current) analysis.
    for m in &show.media {
        let failed = state
            .services
            .analysis
            .inner
            .lock()
            .failed_media
            .contains(&m.id);
        let stale = m
            .analysis
            .as_ref()
            .map_or(true, |a| a.version != audio_analysis::VERSION);
        let file_missing = analysis_path(state, &m.id).is_some_and(|p| !p.is_file());
        if !failed && (stale || file_missing) {
            enqueue_analysis(state, &m.id);
        }
    }
    // Generated shows whose layout changed.
    for s in &show.sequences {
        let Some(g) = &s.generated else { continue };
        if show.media_item(&g.media_id).is_none() || !autoshow::style_exists(&g.style) {
            continue;
        }
        if autoshow::props_hash(&show, &g.prop_ids) != g.props_hash {
            tracing::info!("the layout changed: regenerating \"{}\"", s.name);
            enqueue_autoshow(
                state,
                AutoShowTask {
                    media_id: g.media_id.clone(),
                    style: g.style.clone(),
                    prop_ids: g.prop_ids.clone(),
                    seed: g.seed,
                    target: AutoTarget::Regenerate(s.id.clone()),
                },
                false,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_removes_least_recently_used_first() {
        let d = std::env::temp_dir().join(format!("pp-quota-{}", new_id()));
        std::fs::create_dir_all(&d).unwrap();
        let now = std::time::SystemTime::now();
        for (i, name) in ["a", "b", "c", "d"].iter().enumerate() {
            let p = d.join(format!("{name}.pppv"));
            std::fs::write(&p, vec![0u8; 100]).unwrap();
            std::fs::File::options()
                .append(true)
                .open(&p)
                .unwrap()
                .set_modified(now - Duration::from_secs(100 - i as u64 * 10))
                .unwrap();
        }
        // "a" is the oldest but must be kept (just built).
        enforce_quota(&d, 250, Some(&d.join("a.pppv")));
        let mut left: Vec<String> = std::fs::read_dir(&d)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        left.sort();
        assert_eq!(left, ["a.pppv", "d.pppv"]);
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn job_status_json() {
        let st = JobStatus {
            id: "j".into(),
            kind: JobKind::Autoshow,
            pct: 40,
            state: JobState::Running,
            result: None,
            subject: Some("m".into()),
        };
        assert_eq!(
            serde_json::to_value(&st).unwrap(),
            serde_json::json!({"id":"j","kind":"autoshow","pct":40,"state":"running","subject":"m"})
        );
        assert!(is_temp_id("tmp-abc123"));
        assert!(!is_temp_id("tmp-../x"));
        assert!(!is_temp_id("abc"));
    }
}
