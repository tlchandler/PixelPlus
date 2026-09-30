//! The playback engine: a tokio control task plus a dedicated high-priority
//! output thread that owns everything time-critical.
//!
//! ```text
//!  PlayerHandle ──PlayerCmd──▶ control task (tokio) ──CoreCmd──▶ output thread ("pp-output")
//!  store changes, schedule facts (1 s), identity ────┘                 │ owns: playback state,
//!                                  ◀──CoreEvent (logs, games, DJ)──────┘ audio, overlays, tests,
//!  PlayerStatus watch ◀──────────────────────────────────────────────── maps, pipeline, backend
//! ```
//!
//! The output thread runs one iteration per frame: apply commands (it sleeps
//! in `recv_timeout`, so commands are applied immediately), advance playback
//! (item ends, crossfades, schedule decisions), compose the frame (sequence /
//! effect → tests → overlays), route it through the NodeMap, apply colour
//! order / brightness / gamma and write it to the backend, then publish
//! status and preview. Every iteration runs under `catch_unwind`: a bug in a
//! frame never takes the show down.
//!
//! Timing (ARCHITECTURE §7.5): every frame is chosen for the moment it
//! *lights up*, not the moment it is composed. On DPI that is the next usable
//! vblank (predicted from page-flip timestamps) plus the WS281x latch delay of
//! this controller's strings; the loop is paced from the vblank grid so frame
//! changes land within ±half a refresh of their ideal instant. Without a
//! scanned-out display (simulation) frames light up when written and the loop
//! wakes at the timeline's frame boundaries. The timeline itself is a
//! [`Servo`]: on the leader it follows the audio clock (see `audio.rs`; the
//! monotonic clock when no audio plays) minus `settings.audio.outputDelayMs`,
//! on followers the leader's timeline anchors.

use super::audio::{AudioEngine, TrackId};
use super::clock::{
    self, crossfade_progress, crossfade_start, frame_for_slot, next_update, Fade, MonoClock, Servo,
    ServoGains,
};
use super::compose::{self, find_effect, EffectLayer, PropSlot, Sink, TestLayer};
use super::limiter::EngineLimiter;
use super::overlay::OverlayManager;
use super::playlist::PlaylistCursor;
use super::reader::{FrameLayout, FrameReader, SeqMeta};
use super::scheduler::{self, Origin, SchedAction, ScheduleFacts, Scheduler};
use super::surprise::{self, SurpriseLayer, SurpriseRequest, SurpriseStarted};
use super::types::*;
use super::{Anchor, OverlayCmd, PlayerCmd, PlayerHandle, SyncPacket};
use crate::api::{ApiError, ApiResult};
use crate::events::ToastKind;
use crate::node::LocalRole;
use crate::services::journal::Event as JournalEvent;
use crate::services::tts::DynamicContext;
use crate::state::AppState;
use pixelplus_core::calpattern::{self, CalSchedule};
use pixelplus_core::effects::countdown;
use pixelplus_core::mapcode::MapPlan;
use pixelplus_core::mapping::{NodeMap, OutputFrame, PropMap};
use pixelplus_core::model::{
    BoardKind, EffectPreset, NodePowerBudget, OutputConfig, Playlist, PlaylistItem, Show,
};
use pixelplus_output::{
    BackendKind, OutputFrameRef, PixelOutput, PixelPipeline, SimHandle, SimOutput,
};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};

/// Light fade when stopping with `fade: true` or a schedule `fadeOut`.
const STOP_FADE_MS: f64 = 1000.0;
/// Idle refresh period when nothing is shown.
const IDLE_PERIOD: Duration = Duration::from_millis(100);
/// Preview frames at most every 50 ms (20 fps).
const PREVIEW_EVERY: Duration = Duration::from_millis(50);
/// A sequence whose first frame is not readable within this time is skipped.
const OPEN_TIMEOUT_MS: f64 = 5000.0;
/// Presentation slot without a scanned-out display: software timer granularity.
const SOFT_SLOT_MS: f64 = 1.0;
/// Calibration ("Sync lights to sound"): a click and a white flash every second.
pub const CALIBRATION_ID: &str = "calibration";
pub const CALIBRATION_FLASH_MS: f64 = 50.0;
const CALIBRATION_LEN_MS: u64 = 60_000;
/// Anchor epochs: the engine's epoch in the high bits, timeline jumps below.
const EPOCH_SHIFT: u32 = 20;
/// Phone calibration (F1 v2): item ids are `v2:<seed>`.
const CALIBRATION_V2_PREFIX: &str = "v2:";
/// Show-derived context (budgets, prop mask, smart playlists) is refreshed at
/// least this often.
const CONTEXT_EVERY: Duration = Duration::from_secs(30);
/// Mapping plans kept for `mapRunId` references.
const MAP_PLANS_KEPT: usize = 8;
/// Mapping plans with up to this many targets (≈ 4 KB of JSON) also travel
/// in sync packets; larger ones only by command.
const MAP_INLINE_TARGETS: usize = 64;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Engine options (from the environment in production, explicit in tests).
#[derive(Debug, Clone)]
pub struct EngineOptions {
    /// Play audio (`PIXELPLUS_AUDIO=none` disables it).
    pub audio: bool,
    /// Force an output backend (otherwise from `config.output`).
    pub output: Option<BackendKind>,
    /// Where shared-memory overlay buffers are created (`PIXELPLUS_SHM_DIR`, default `/dev/shm`).
    pub shm_dir: PathBuf,
    /// Try SCHED_FIFO for the output thread.
    pub realtime: bool,
    /// Simulated output only: pretend to scan out at this refresh rate
    /// (`PIXELPLUS_SIM_REFRESH_HZ`), so presentation-time pacing runs as on
    /// DPI (development, tests).
    pub sim_refresh_hz: Option<f64>,
}

impl EngineOptions {
    pub fn from_env() -> Self {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        EngineOptions {
            audio: !matches!(
                env("PIXELPLUS_AUDIO").as_deref(),
                Some("none" | "off" | "0" | "false")
            ),
            output: None,
            shm_dir: env("PIXELPLUS_SHM_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/dev/shm")),
            realtime: true,
            sim_refresh_hz: env("PIXELPLUS_SIM_REFRESH_HZ")
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|hz| hz.is_finite() && *hz >= 1.0 && *hz <= 1000.0),
        }
    }
}

/// A running engine (tests keep it to read the simulated output and stop it).
#[cfg_attr(not(test), allow(dead_code))]
pub struct Engine {
    pub handle: PlayerHandle,
    /// The simulated output's frames (when the sim backend is in use).
    pub sim: Option<SimHandle>,
    core_tx: Sender<CoreCmd>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl Engine {
    /// Stop the output thread (blanks the outputs).
    pub fn shutdown(&self) {
        let _ = self.core_tx.send(CoreCmd::Shutdown);
    }
}

/// Start the engine and register it in `state.services.player`.
pub async fn start(state: &AppState) -> anyhow::Result<PlayerHandle> {
    let engine = start_with(state, EngineOptions::from_env())?;
    let handle = engine.handle.clone();
    let _ = state.services.player.set(handle.clone());
    // The engine lives for the whole process.
    std::mem::forget(engine);
    Ok(handle)
}

/// Start with explicit options (tests). Must be called inside a tokio runtime.
pub fn start_with(state: &AppState, opts: EngineOptions) -> anyhow::Result<Engine> {
    let (cmd_tx, cmd_rx) = mpsc::channel::<PlayerCmd>(64);
    let (status_tx, status_rx) = watch::channel(PlayerStatus::default());
    let (core_tx, core_rx) = std::sync::mpsc::channel::<CoreCmd>();
    let (ev_tx, ev_rx) = mpsc::unbounded_channel::<CoreEvent>();

    let mut core = Core::new(state.clone(), opts, core_rx, ev_tx, status_tx);
    if state.config.dev || state.config.output == crate::config::OutputMode::Sim {
        let tap = state
            .services
            .debug_output
            .get_or_init(|| std::sync::Arc::new(super::debugtap::OutputTap::new()))
            .clone();
        core.tap = Some(tap);
    }
    let sim = core.output.sim.clone();
    std::thread::Builder::new()
        .name("pp-output".into())
        .spawn(move || run_output_thread(core))?;

    tokio::spawn(control_task(state.clone(), cmd_rx, core_tx.clone(), ev_rx));
    tokio::spawn(context_task(state.clone(), core_tx.clone()));
    tokio::spawn(status_publisher(state.clone(), status_rx.clone()));
    tokio::spawn(power_publisher(state.clone(), status_rx.clone()));
    Ok(Engine {
        handle: PlayerHandle::new(cmd_tx, status_rx),
        sim,
        core_tx,
    })
}

/// DPI geometry vs. configured strings (for the pre-show health check).
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GeometryStatus {
    /// False when a string is longer than the DPI mode set at boot allows.
    pub ok: bool,
    /// Longest configured string on this node (pixels).
    pub longest_string: u32,
    /// Longest string the running DPI mode can drive (None: not DPI / unknown).
    pub max_pixels: Option<u32>,
    /// String length the boot configuration should be sized for.
    pub needed_pixels: Option<u32>,
    pub message: Option<String>,
}

static GEOMETRY: parking_lot::Mutex<Option<GeometryStatus>> = parking_lot::const_mutex(None);

/// Current DPI geometry check (see [`GeometryStatus`]).
pub fn geometry_status() -> GeometryStatus {
    GEOMETRY.lock().clone().unwrap_or(GeometryStatus {
        ok: true,
        ..Default::default()
    })
}

// ---------------------------------------------------------------------------
// Messages between the control task and the output thread
// ---------------------------------------------------------------------------

#[allow(clippy::large_enum_variant)]
enum CoreCmd {
    Player(PlayerCmd),
    Show(Arc<Show>),
    Facts(ScheduleFacts),
    Identity {
        id: String,
        role: LocalRole,
        board: Option<BoardKind>,
    },
    DjRendered {
        clip_id: String,
        path: PathBuf,
    },
    /// Show-derived context computed off the output thread.
    Context(Box<EngineContext>),
    /// Measured supply currents (group id, A) from sensor nodes (F20).
    Measured(Vec<(String, f32)>),
    Shutdown,
}

/// What the output thread needs from files and the journal (computed by the
/// context task; see [`compute_context`]).
#[derive(Debug, Default, Clone, PartialEq)]
struct EngineContext {
    /// Power limiter budget of this node (F12).
    budget: Option<NodePowerBudget>,
    /// Props kept dark by the season profile (F8).
    disabled: Vec<String>,
    /// Smart playlists (F18): tonight's items per playlist id.
    smart: HashMap<String, Vec<PlaylistItem>>,
}

enum CoreEvent {
    Log {
        level: &'static str,
        message: String,
        toast: bool,
    },
    Games(serde_json::Value),
    PrerenderDj {
        clip_id: String,
        ctx: DynamicContext,
    },
    /// Master brightness, volume or lights-off changed (saved for restarts).
    Levels(SavedLevels),
    /// A mapping test started on the leader: its plan goes to every follower
    /// once by command (sync packets carry only the run id when it is large).
    PushTest(Box<TestRequest>),
}

/// Master brightness, volume and lights-off of the leader, kept in
/// `<data>/player.json` so a restart (power cut, update) doesn't bring the
/// lights back at full brightness or the sound back at the old volume.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedLevels {
    pub brightness: u8,
    pub volume: u8,
    #[serde(default)]
    pub blackout: bool,
}

impl SavedLevels {
    fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("player.json")
    }

    pub fn load(data_dir: &Path) -> Option<SavedLevels> {
        let text = std::fs::read_to_string(Self::path(data_dir)).ok()?;
        let l: SavedLevels = serde_json::from_str(&text).ok()?;
        Some(SavedLevels {
            brightness: l.brightness.min(100),
            volume: l.volume.min(100),
            blackout: l.blackout,
        })
    }

    fn save(&self, data_dir: &Path) -> std::io::Result<()> {
        use std::io::Write;
        let path = Self::path(data_dir);
        let tmp = path.with_extension("json.tmp");
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&serde_json::to_vec(self).unwrap_or_default())?;
            f.sync_all()?;
        }
        std::fs::rename(tmp, path)
    }
}

// ---------------------------------------------------------------------------
// Control task (tokio)
// ---------------------------------------------------------------------------

async fn control_task(
    state: AppState,
    mut rx: mpsc::Receiver<PlayerCmd>,
    core: Sender<CoreCmd>,
    mut ev_rx: mpsc::UnboundedReceiver<CoreEvent>,
) {
    let mut show_rx = state.store.subscribe();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_identity = None;
    let levels_writer = Arc::new(LevelsWriter::default());
    loop {
        tokio::select! {
            cmd = rx.recv() => match cmd {
                Some(cmd) => {
                    if core.send(CoreCmd::Player(cmd)).is_err() {
                        break;
                    }
                }
                None => break,
            },
            changed = show_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let _ = core.send(CoreCmd::Show(state.store.get()));
            }
            _ = tick.tick() => {
                let id = state.identity();
                let key = (id.id.clone(), id.role, id.board);
                if last_identity.as_ref() != Some(&key) {
                    let _ = core.send(CoreCmd::Identity { id: id.id.clone(), role: id.role, board: id.board });
                    last_identity = Some(key);
                }
                let show = state.store.get();
                let facts = scheduler::facts_for(&show, chrono::Utc::now());
                if core.send(CoreCmd::Facts(facts)).is_err() {
                    break;
                }
            }
            Some(ev) = ev_rx.recv() => handle_event(&state, &core, &levels_writer, ev),
        }
    }
    let _ = core.send(CoreCmd::Shutdown);
}

/// Serializes the `player.json` writes of [`CoreEvent::Levels`].
#[derive(Default)]
struct LevelsWriter {
    latest: parking_lot::Mutex<Option<SavedLevels>>,
    writing: tokio::sync::Mutex<()>,
}

fn handle_event(
    state: &AppState,
    core: &Sender<CoreCmd>,
    levels_writer: &Arc<LevelsWriter>,
    ev: CoreEvent,
) {
    match ev {
        CoreEvent::Log {
            level,
            message,
            toast,
        } => {
            state.events.publish(
                "log",
                &serde_json::json!({ "level": level, "message": message, "time": chrono::Utc::now().to_rfc3339() }),
            );
            if toast {
                let kind = if level == "error" {
                    ToastKind::Error
                } else {
                    ToastKind::Warning
                };
                state.events.toast(kind, message);
            }
        }
        CoreEvent::Games(cmd) => {
            let path = state.config.games_socket.clone();
            let events = state.events.clone();
            tokio::spawn(async move {
                if let Err(e) = games_command(&path, &cmd).await {
                    tracing::warn!("games command failed: {e}");
                    events.publish(
                        "log",
                        &serde_json::json!({ "level": "warning", "message": format!("Games: {e}"), "time": chrono::Utc::now().to_rfc3339() }),
                    );
                }
            });
        }
        CoreEvent::Levels(levels) => {
            // One writer at a time, always the newest levels (a slider sends many).
            *levels_writer.latest.lock() = Some(levels);
            let dir = state.config.data_dir.clone();
            let w = levels_writer.clone();
            tokio::spawn(async move {
                let _one = w.writing.lock().await;
                let Some(levels) = w.latest.lock().take() else {
                    return; // an earlier writer already saved it
                };
                let saved = tokio::task::spawn_blocking(move || levels.save(&dir)).await;
                if let Ok(Err(e)) = saved {
                    tracing::warn!("could not save the brightness and volume: {e}");
                }
            });
        }
        CoreEvent::PushTest(test) => {
            if let Some(cluster) = state.services.cluster.get().cloned() {
                tokio::spawn(async move {
                    let results = cluster
                        .send_command(
                            None,
                            crate::cluster::ClusterCommand::TestStart { test: *test },
                        )
                        .await;
                    for r in results.iter().filter(|r| !r.ok) {
                        tracing::warn!(
                            "mapping plan not delivered to {}: {}",
                            r.node_id,
                            r.error.as_deref().unwrap_or("no answer")
                        );
                    }
                });
            }
        }
        CoreEvent::PrerenderDj { clip_id, ctx } => {
            let core = core.clone();
            let state = state.clone();
            tokio::spawn(async move {
                crate::services::tts::warmup(&state).await;
                let render = crate::services::tts::render_dynamic_clip(&state, &clip_id, ctx);
                match tokio::time::timeout(Duration::from_secs(180), render).await {
                    Ok(Ok(path)) => {
                        let _ = core.send(CoreCmd::DjRendered { clip_id, path });
                    }
                    Ok(Err(e)) => tracing::warn!(
                        "rendering DJ clip {clip_id} failed: {e:#}; using its last render"
                    ),
                    Err(_) => tracing::warn!(
                        "rendering DJ clip {clip_id} timed out; using its last render"
                    ),
                }
            });
        }
    }
}

/// Send one JSON line to the games sidecar's control socket.
async fn games_command(path: &Path, cmd: &serde_json::Value) -> Result<serde_json::Value, String> {
    #[cfg(unix)]
    {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let fut = async {
            let mut stream = tokio::net::UnixStream::connect(path)
                .await
                .map_err(|_| "the games service isn't running".to_string())?;
            let mut line = serde_json::to_vec(cmd).map_err(|e| e.to_string())?;
            line.push(b'\n');
            stream.write_all(&line).await.map_err(|e| e.to_string())?;
            let mut reader = BufReader::new(stream);
            let mut resp = String::new();
            reader
                .read_line(&mut resp)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::from_str::<serde_json::Value>(resp.trim()).map_err(|e| e.to_string())
        };
        tokio::time::timeout(Duration::from_secs(4), fut)
            .await
            .map_err(|_| "the games service did not answer".to_string())?
    }
    #[cfg(not(unix))]
    {
        let _ = (path, cmd);
        Err("games are only available on Linux".into())
    }
}

/// Keep the output thread's [`EngineContext`] current: on show and identity
/// changes and every [`CONTEXT_EVERY`] (smart playlists follow the journal).
async fn context_task(state: AppState, core: Sender<CoreCmd>) {
    let mut show_rx = state.store.subscribe();
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last: Option<EngineContext> = None;
    let mut last_key = None;
    let mut last_full = std::time::Instant::now() - CONTEXT_EVERY;
    loop {
        tokio::select! {
            r = show_rx.changed() => if r.is_err() { break },
            _ = tick.tick() => {
                let id = state.identity();
                // Measured supply currents (F20 sensors) every tick.
                if id.role != LocalRole::Follower {
                    let m = super::limiter::measured(&state, &state.store.get(), &id.id);
                    if !m.is_empty() && core.send(CoreCmd::Measured(m)).is_err() {
                        break;
                    }
                }
                let key = (id.id.clone(), id.role, state.store.get().version);
                if last_key.as_ref() == Some(&key) && last_full.elapsed() < CONTEXT_EVERY {
                    continue;
                }
                last_key = Some(key);
            }
        }
        last_full = std::time::Instant::now();
        let st = state.clone();
        let Ok(ctx) = tokio::task::spawn_blocking(move || compute_context(&st)).await else {
            continue;
        };
        if last.as_ref() != Some(&ctx) {
            last = Some(ctx.clone());
            if core.send(CoreCmd::Context(Box::new(ctx))).is_err() {
                break;
            }
        }
    }
}

/// Budget, prop mask and smart playlist expansions for this node.
fn compute_context(state: &AppState) -> EngineContext {
    let show = state.store.get();
    let id = state.identity();
    let follower = id.role == LocalRole::Follower;
    EngineContext {
        budget: super::limiter::budget_for(state, &show, &id.id, follower),
        disabled: super::limiter::disabled_props(&show),
        smart: if follower {
            HashMap::new()
        } else {
            smart_expansions(state, &show)
        },
    }
}

/// Tonight's items of every smart playlist (F18): WS2's expansion
/// (`api::library::smart_items`: the journal's play history, seeded by show
/// night and playlist), the same the "Tonight" preview shows.
fn smart_expansions(state: &AppState, show: &Show) -> HashMap<String, Vec<PlaylistItem>> {
    show.playlists
        .iter()
        .filter(|p| p.smart.is_some())
        .filter_map(|p| {
            Some((
                p.id.clone(),
                crate::api::library::smart_items(state, &p.id)?,
            ))
        })
        .collect()
}

/// WS `power` (F12): every second while something lights the display and
/// the limiter is on, the live power view (`GET /power/live`).
async fn power_publisher(state: AppState, rx: watch::Receiver<PlayerStatus>) {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let st = rx.borrow().clone();
        let lit = matches!(
            st.state,
            PlayerState::Playing | PlayerState::Effect | PlayerState::Testing
        );
        if st.power.is_some() && lit {
            let v = super::limiter::power_live(&state);
            state.events.publish("power", &v);
        }
        if rx.has_changed().is_err() {
            break;
        }
    }
}

/// Publish `status` on the WebSocket whenever the engine publishes one (every
/// 250 ms while playing, every 2 s when idle, and on every change).
async fn status_publisher(state: AppState, mut rx: watch::Receiver<PlayerStatus>) {
    loop {
        let status = rx.borrow_and_update().clone();
        if let Ok(v) = serde_json::to_value(&status) {
            state.services.remember("status", v.clone());
            state.events.publish("status", &v);
        }
        if rx.changed().await.is_err() {
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// Output thread
// ---------------------------------------------------------------------------

fn run_output_thread(mut core: Core) {
    if core.opts.realtime {
        set_realtime_priority();
    }
    let mut next = Instant::now();
    loop {
        // Wait for the frame deadline (see `Core::plan_next`), applying
        // commands as they arrive.
        loop {
            let now = Instant::now();
            if now >= next {
                break;
            }
            match core.rx.recv_timeout(next - now) {
                Ok(CoreCmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => {
                    core.shutdown();
                    return;
                }
                Ok(cmd) => {
                    if catch_unwind(AssertUnwindSafe(|| core.handle(cmd))).is_err() {
                        core.recover_from_panic("a command");
                    }
                }
                Err(RecvTimeoutError::Timeout) => break,
            }
        }
        let started = Instant::now();
        match catch_unwind(AssertUnwindSafe(|| core.tick())) {
            Ok(()) => core.panics = 0,
            Err(_) => core.recover_from_panic("a frame"),
        }
        core.note_tick(started.elapsed());
        // Fell behind (slow SD card, overloaded CPU): the plan starts from now,
        // so there is never a burst of catch-up frames.
        next = match catch_unwind(AssertUnwindSafe(|| core.plan_next())) {
            Ok(t) => t,
            Err(_) => Instant::now() + core.frame_period(),
        };
    }
}

fn set_realtime_priority() {
    #[cfg(target_os = "linux")]
    {
        let param = libc::sched_param { sched_priority: 40 };
        // SAFETY: plain syscall on the current thread with a valid parameter struct.
        let r = unsafe { libc::sched_setscheduler(0, libc::SCHED_FIFO, &param) };
        if r == 0 {
            tracing::info!("output thread running with realtime priority");
        } else {
            tracing::debug!(
                "realtime priority not available ({}); using normal priority",
                std::io::Error::last_os_error()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Output stage
// ---------------------------------------------------------------------------

struct OutputStage {
    kind: BackendKind,
    board: BoardKind,
    backend: Box<dyn PixelOutput>,
    running: bool,
    next_retry: Instant,
    backoff: Duration,
    error: Option<String>,
    consecutive_errors: u32,
    sim: Option<SimHandle>,
}

impl OutputStage {
    fn new(kind: BackendKind, board: BoardKind, sim_refresh_hz: Option<f64>) -> Self {
        let (backend, sim): (Box<dyn PixelOutput>, _) = match kind {
            BackendKind::Sim => {
                let mut s = SimOutput::new();
                if let Some(hz) = sim_refresh_hz {
                    s = s.with_refresh(Duration::from_secs_f64(1.0 / hz));
                }
                let h = s.handle();
                (Box::new(s), Some(h))
            }
            other => match pixelplus_output::create_backend(other, board) {
                Ok(b) => (b, None),
                Err(e) => {
                    tracing::warn!("pixel output {other:?} unavailable: {e}");
                    (Box::new(pixelplus_output::NullOutput::new()), None)
                }
            },
        };
        OutputStage {
            kind,
            board,
            backend,
            running: false,
            next_retry: Instant::now(),
            backoff: Duration::from_secs(1),
            error: None,
            consecutive_errors: 0,
            sim,
        }
    }

    fn write(&mut self, frame: &OutputFrameRef<'_>, now: Instant) {
        if !self.running {
            if now < self.next_retry {
                return;
            }
            match self.backend.start() {
                Ok(()) => {
                    if self.error.is_some() {
                        tracing::info!("pixel output recovered");
                    }
                    self.running = true;
                    self.error = None;
                    self.backoff = Duration::from_secs(1);
                }
                Err(e) => {
                    let msg = format!("Pixel output is not working: {e}");
                    if self.error.as_deref() != Some(msg.as_str()) {
                        tracing::warn!("{msg}");
                    }
                    self.error = Some(msg);
                    self.next_retry = now + self.backoff;
                    self.backoff = (self.backoff * 2).min(Duration::from_secs(30));
                    return;
                }
            }
        }
        match self.backend.write_frame(frame) {
            Ok(()) => {
                self.consecutive_errors = 0;
                if self.error.is_some() {
                    self.error = None;
                }
            }
            Err(e) => {
                self.consecutive_errors += 1;
                self.error = Some(format!("Pixel output error: {e}"));
                if self.consecutive_errors >= 20 {
                    tracing::warn!("pixel output failing repeatedly ({e}); restarting it");
                    self.backend.stop();
                    self.running = false;
                    self.consecutive_errors = 0;
                    self.next_retry = now + self.backoff;
                    self.backoff = (self.backoff * 2).min(Duration::from_secs(30));
                }
            }
        }
    }

    fn max_pixels(&self) -> Option<u32> {
        if self.kind == BackendKind::Dpi {
            self.backend.stats().max_pixels_per_output
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Playback state
// ---------------------------------------------------------------------------

enum Source {
    Playlist(Box<PlaylistCursor>),
    Single,
}

struct Program {
    origin: Origin,
    source: Source,
    playlist: Option<(String, String)>,
    crossfade_ms: u32,
}

/// Countdown DJ clip (F4): starts at item position `at_pos` so that it ends
/// at zero (plus the item's offset); `skip_ms` into the clip when it is
/// longer than the countdown.
#[derive(Debug, Clone)]
struct DelayedAudio {
    path: PathBuf,
    gain: f32,
    at_pos: f64,
    skip_ms: u64,
}

/// Something that will play next.
#[derive(Debug, Clone)]
enum Pending {
    Item(PlaylistItem),
    Request {
        sequence_id: String,
        name: Option<String>,
    },
    /// "Sync lights to sound" calibration pattern.
    Calibration,
    /// Phone calibration (F1): the v2 pattern with this seed.
    CalibrationV2(u32),
}

enum ActiveKind {
    Sequence {
        reader: FrameReader,
        meta: Option<SeqMeta>,
    },
    Effect(Box<EffectLayer>),
    /// DJ clip or media: audio only, the idle look runs under it.
    Audio,
    Pause,
    /// Calibration: every prop flashes white at each click of the audio
    /// (v1: every second; v2: the seeded pattern).
    Flash(Option<Box<CalSchedule>>),
}

struct Active {
    iref: ItemRef,
    kind: ActiveKind,
    audio_src: Option<(PathBuf, f32)>,
    audio: Option<TrackId>,
    use_audio_clock: bool,
    fade_in_ms: u32,
    clock: MonoClock,
    started: bool,
    begun_ms: f64,
    /// None = until stopped (manual look) / until the audio ends.
    duration_ms: Option<u64>,
    last_pos: f64,
    /// The item's timeline as the lights follow it (tracks `clock`, which
    /// tracks the audio clock); created when the item starts.
    servo: Option<Servo>,
    buf: Vec<u8>,
    have_frame: bool,
    warned_audio: bool,
    /// Countdown DJ clip waiting for its moment (F4).
    delayed_audio: Option<DelayedAudio>,
    /// Keep the audio playing past the item's end (a countdown clip that
    /// ends after zero).
    detach_audio: bool,
    /// Start this far into the item (exact show starts, F4).
    start_at_ms: f64,
}

impl Active {
    fn new(iref: ItemRef, kind: ActiveKind, now_ms: f64) -> Self {
        Active {
            iref,
            kind,
            audio_src: None,
            audio: None,
            use_audio_clock: false,
            fade_in_ms: 0,
            clock: MonoClock::new(0.0, now_ms),
            started: false,
            begun_ms: now_ms,
            duration_ms: None,
            last_pos: 0.0,
            servo: None,
            buf: Vec::new(),
            have_frame: false,
            warned_audio: false,
            delayed_audio: None,
            detach_audio: false,
            start_at_ms: 0.0,
        }
    }

    fn is_manual_look(&self) -> bool {
        matches!(self.kind, ActiveKind::Effect(_)) && self.duration_ms.is_none()
    }
}

struct Outgoing {
    active: Active,
    start_ms: f64,
    len_ms: f64,
}

struct Look {
    id: String,
    name: String,
    layer: EffectLayer,
    started_ms: f64,
    /// The song the look's beat pulse follows (`beatFollowSong`, F2).
    beat_key: Option<String>,
}

#[derive(Default)]
struct FollowState {
    pkt: Option<SyncPacket>,
    rx_ms: f64,
    /// The leader's timeline on the local clock (from the last packet).
    anchor: Option<Anchor>,
    /// Our position, following `anchor` (see [`Servo`]).
    clock: Option<Servo>,
    /// Calibration flash pattern (v2: its schedule).
    flash: bool,
    cal: Option<CalSchedule>,
    item_key: Option<String>,
    reader: Option<FrameReader>,
    meta: Option<SeqMeta>,
    buf: Vec<u8>,
    have_frame: bool,
    effect: Option<EffectLayer>,
    test: Option<TestLayer>,
    lost_fade: Option<Fade>,
    /// No sync for a while and the item has ended: holding the last frame since.
    hold_since: Option<f64>,
    lost_logged: bool,
    missing: Option<String>,
}

// ---------------------------------------------------------------------------
// Core (runs on the output thread)
// ---------------------------------------------------------------------------

struct Core {
    app: AppState,
    opts: EngineOptions,
    rx: Receiver<CoreCmd>,
    events: mpsc::UnboundedSender<CoreEvent>,
    status_tx: watch::Sender<PlayerStatus>,
    t0: Instant,

    show: Arc<Show>,
    node_id: String,
    role: LocalRole,
    identity_board: Option<BoardKind>,
    board: BoardKind,
    is_pi: bool,
    slow_pi: bool,

    node_map: NodeMap,
    prop_map: PropMap,
    slots: HashMap<String, PropSlot>,
    remote_props: HashSet<String>,
    pipeline: PixelPipeline,
    output: OutputStage,
    frame: OutputFrame,
    wire: pixelplus_output::OutputFrame,
    chan: Vec<u8>,
    chan_b: Vec<u8>,
    effect_period: Duration,

    audio: AudioEngine,
    audio_retry_at: Instant,
    audio_backoff: Duration,
    /// An audio device being opened on a helper thread (opening can hang).
    audio_pending: Option<Receiver<AudioEngine>>,
    audio_device: String,

    program: Option<Program>,
    current: Option<Active>,
    outgoing: Option<Outgoing>,
    preload: Option<(String, FrameReader)>,
    requests: VecDeque<(String, Option<String>)>,
    paused: bool,
    stop_fade: Option<Fade>,
    dj_rendered: HashMap<String, PathBuf>,
    look: Option<Look>,
    idle_layer: Option<Look>,
    sched: Scheduler,
    facts: ScheduleFacts,
    item_error: Option<(String, Instant)>,

    follow: FollowState,
    overlays: OverlayManager,
    test: Option<TestLayer>,

    brightness: u8,
    volume: u8,
    applied_volume: Option<u8>,
    settings_volume: u8,
    blackout: bool,

    frames_out: u64,
    fps: f32,
    fps_window: (Instant, u64),
    last_status_at: Instant,
    last_status: PlayerStatus,
    last_preview: Instant,
    preview_no: u32,
    panics: u32,
    last_extras: (Option<EffectPreset>, Option<TestRequest>),
    /// Development output tap (`GET /debug/output`).
    tap: Option<std::sync::Arc<super::debugtap::OutputTap>>,
    /// Sequence and frame index composed for the current output frame.
    shown: Option<(String, u32)>,
    /// Timeline position (ms, unquantised) the shown frame was chosen for.
    shown_pos: Option<f64>,

    // ----- feature wave -----
    /// Power limiter on the wire bytes (F12).
    limiter: EngineLimiter,
    /// Props the season profile keeps dark (F8).
    disabled_ids: Vec<String>,
    disabled_slots: Vec<PropSlot>,
    zeros: Vec<u8>,
    /// Smart playlists (F18): tonight's items per playlist id.
    smart: HashMap<String, Vec<PlaylistItem>>,
    /// The running surprise (F20).
    surprise: Option<SurpriseLayer>,
    surprise_seq: u64,
    /// Mapping plans by run id (F6/F7; sync packets carry only the id).
    map_plans: VecDeque<(String, MapPlan)>,
    /// A timeline test started: its anchor epoch.
    test_epoch: u64,
    /// Schedule window of the running program (journal `showEnd`).
    show_window: Option<(String, String)>,

    // ----- presentation timing (see `plan_next`) -----
    /// Engine clock (ms) at which the frame being composed lights up.
    light_ms: f64,
    /// How long a frame is shown at least: the display refresh period, or
    /// the software timer granularity.
    slot_ms: f64,
    /// Vblank planned for the next frame and the latch delay after it (ms).
    planned: Option<(Instant, f64)>,
    /// Smoothed duration of a tick (compose + encode), ms.
    tick_ms: f64,
    /// Longest and mean (non-empty) string of this node, in LEDs.
    string_lines: (f64, f64),
    /// Timeline discontinuities (new item, seek, pause, delay change).
    epoch: u64,
    /// `settings.audio.outputDelayMs` (leader).
    output_delay_ms: f64,
    latch_align: bool,
}

fn item_ref(kind: &str, id: &str, name: &str) -> ItemRef {
    ItemRef {
        kind: kind.into(),
        id: id.into(),
        name: name.into(),
    }
}

impl Core {
    fn new(
        app: AppState,
        opts: EngineOptions,
        rx: Receiver<CoreCmd>,
        events: mpsc::UnboundedSender<CoreEvent>,
        status_tx: watch::Sender<PlayerStatus>,
    ) -> Self {
        let show = app.store.get();
        let identity = app.identity();
        let pi = pixelplus_hw::board::read_pi_info();
        let is_pi = pi.is_some();
        let slow_pi = pi.as_ref().is_some_and(|p| {
            matches!(
                p.family,
                pixelplus_hw::board::PiFamily::Zero2W
                    | pixelplus_hw::board::PiFamily::Zero
                    | pixelplus_hw::board::PiFamily::Pi1
            )
        });
        let board = board_for(&show, &identity.id, identity.board, is_pi);
        let kind = choose_backend(&app, &opts, board, is_pi);
        tracing::info!("pixel output: {kind:?} for board {board:?}");
        let audio_device = show.settings.audio.device.clone();
        let want_audio = opts.audio && identity.role != LocalRole::Follower;
        let audio = AudioEngine::disabled(if want_audio {
            "the audio device is starting"
        } else {
            "audio is disabled on this controller"
        });
        let settings_volume = show.settings.audio.volume.min(100);
        // The leader's own levels survive a restart; followers get theirs
        // from the leader.
        let saved = (identity.role != LocalRole::Follower)
            .then(|| SavedLevels::load(&app.config.data_dir))
            .flatten();
        let volume = saved.map_or(settings_volume, |l| l.volume);
        let t0 = app.started;
        let shm_dir = opts.shm_dir.clone();
        let mut core = Core {
            rx,
            events,
            status_tx,
            t0,
            node_id: identity.id.clone(),
            role: identity.role,
            identity_board: identity.board,
            board,
            is_pi,
            slow_pi,
            node_map: NodeMap::identity("", &[]),
            prop_map: PropMap::build_with_layout(&dummy_node(), &[], &[]),
            slots: HashMap::new(),
            remote_props: HashSet::new(),
            pipeline: PixelPipeline::new(&[]),
            output: OutputStage::new(kind, board, opts.sim_refresh_hz),
            frame: OutputFrame::new(&[]),
            wire: pixelplus_output::OutputFrame::default(),
            chan: Vec::new(),
            chan_b: Vec::new(),
            effect_period: Duration::from_millis(25),
            audio,
            audio_retry_at: Instant::now(),
            audio_backoff: Duration::from_secs(15),
            audio_pending: None,
            audio_device,
            program: None,
            current: None,
            outgoing: None,
            preload: None,
            requests: VecDeque::new(),
            paused: false,
            stop_fade: None,
            dj_rendered: HashMap::new(),
            look: None,
            idle_layer: None,
            sched: Scheduler::new(),
            facts: ScheduleFacts::default(),
            item_error: None,
            follow: FollowState::default(),
            overlays: OverlayManager::new(shm_dir),
            test: None,
            brightness: saved.map_or(100, |l| l.brightness),
            volume,
            applied_volume: None,
            settings_volume,
            blackout: saved.is_some_and(|l| l.blackout),
            frames_out: 0,
            fps: 0.0,
            fps_window: (Instant::now(), 0),
            last_status_at: Instant::now() - Duration::from_secs(10),
            last_status: PlayerStatus::default(),
            last_preview: Instant::now(),
            preview_no: 0,
            panics: 0,
            last_extras: (None, None),
            tap: None,
            shown: None,
            shown_pos: None,
            limiter: EngineLimiter::default(),
            disabled_ids: Vec::new(),
            disabled_slots: Vec::new(),
            zeros: Vec::new(),
            smart: HashMap::new(),
            surprise: None,
            surprise_seq: 0,
            map_plans: VecDeque::new(),
            test_epoch: 0,
            show_window: None,
            light_ms: 0.0,
            slot_ms: SOFT_SLOT_MS,
            planned: None,
            tick_ms: 2.0,
            string_lines: (0.0, 0.0),
            epoch: 1,
            output_delay_ms: show.settings.audio.output_delay_ms as f64,
            latch_align: show.settings.output.latch_align,
            show: show.clone(),
            app,
            opts,
        };
        core.rebuild_maps();
        let align = core.latch_align;
        core.output.backend.set_bottom_align(align);
        core
    }

    fn now_ms(&self) -> f64 {
        self.t0.elapsed().as_secs_f64() * 1000.0
    }

    fn is_follower(&self) -> bool {
        self.role == LocalRole::Follower
    }

    // ----- logging -------------------------------------------------------

    fn warn(&self, message: impl Into<String>, toast: bool) {
        let message = message.into();
        tracing::warn!("{message}");
        let _ = self.events.send(CoreEvent::Log {
            level: "warning",
            message,
            toast,
        });
    }

    fn recover_from_panic(&mut self, what: &str) {
        self.panics += 1;
        tracing::error!(
            "the player hit an internal error in {what}; recovering (#{})",
            self.panics
        );
        if self.panics >= 3 {
            // Something in the current program keeps failing: drop it.
            self.program = None;
            self.current = None;
            self.outgoing = None;
            self.preload = None;
            self.test = None;
            self.look = None;
            self.follow = FollowState::default();
            self.audio.stop_all(50);
            self.item_error = Some((
                "Playback stopped after an internal error".into(),
                Instant::now(),
            ));
            let _ = self.events.send(CoreEvent::Log {
                level: "error",
                message: "Playback stopped after an internal error; the player recovered.".into(),
                toast: true,
            });
        }
        if self.panics >= 6 {
            // Maybe the output backend is wedged: rebuild it.
            self.output = OutputStage::new(self.output.kind, self.board, self.opts.sim_refresh_hz);
            self.panics = 0;
        }
    }

    fn shutdown(&mut self) {
        self.audio.stop_all(50);
        self.output.backend.stop();
    }

    // ----- maps & configuration -------------------------------------------

    fn rebuild_maps(&mut self) {
        let show = self.show.clone();
        let node = show.node(&self.node_id).cloned();
        match &node {
            Some(n) => {
                self.node_map = NodeMap::build_for_node(n, &show.props);
                for w in &self.node_map.warnings {
                    tracing::debug!("mapping: {w}");
                }
                self.prop_map =
                    PropMap::build_with_layout(n, &show.props, self.node_map.pixels_per_output());
                let count = self.node_map.pixels_per_output().len();
                let configs: Vec<OutputConfig> = (0..count)
                    .map(|i| {
                        n.outputs
                            .iter()
                            .find(|o| o.index as usize == i + 1)
                            .cloned()
                            .unwrap_or_else(|| OutputConfig {
                                index: i as u32 + 1,
                                ..Default::default()
                            })
                    })
                    .collect();
                let master = self.pipeline.master_brightness();
                self.pipeline = PixelPipeline::new(&configs);
                self.pipeline.set_master_brightness(master);
            }
            None => {
                self.node_map = NodeMap::identity(&self.node_id, &[]);
                self.prop_map = PropMap::build_with_layout(&dummy_node(), &[], &[]);
                self.pipeline = PixelPipeline::new(&[]);
            }
        }
        self.frame = self.node_map.new_frame();
        self.slots = show
            .props
            .iter()
            .map(|p| (p.id.clone(), PropSlot::of(p)))
            .collect();
        self.remote_props = show
            .props
            .iter()
            .filter(|p| p.segments.iter().any(|s| s.node_id != self.node_id))
            .map(|p| p.id.clone())
            .collect();
        let chan_len = show
            .props
            .iter()
            .map(|p| p.channel_end() as usize)
            .max()
            .unwrap_or(0)
            // A hostile or corrupt show file must not make us allocate
            // gigabytes: no real sequence frame is larger than this.
            .min(pixelplus_core::fseq::MAX_FRAME_BYTES as usize);
        self.chan.resize(chan_len, 0);
        self.chan_b.resize(chan_len, 0);
        let total = self.node_map.total_pixels();
        let longest = self
            .node_map
            .pixels_per_output()
            .iter()
            .copied()
            .max()
            .unwrap_or(0);
        self.effect_period = if self.slow_pi || total > 20_000 || longest > 800 {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(25)
        };
        let used: Vec<f64> = self
            .node_map
            .pixels_per_output()
            .iter()
            .filter(|&&p| p > 0)
            .map(|&p| p as f64)
            .collect();
        let mean = if used.is_empty() {
            0.0
        } else {
            used.iter().sum::<f64>() / used.len() as f64
        };
        self.string_lines = (longest as f64, mean);
        self.update_geometry();
    }

    fn update_geometry(&self) {
        let longest = self
            .node_map
            .pixels_per_output()
            .iter()
            .copied()
            .max()
            .unwrap_or(0);
        let max = self.output.max_pixels();
        let needed = pixelplus_output::DpiGeometry::for_pixels(longest.max(1))
            .ok()
            .map(|g| g.pixels_per_output);
        let ok = max.map_or(true, |m| longest <= m);
        let message = (!ok).then(|| {
            format!(
                "The longest string has {longest} pixels but the pixel output was set up at boot for {} per output. Reboot the controller to apply the new length.",
                max.unwrap_or(0)
            )
        });
        *GEOMETRY.lock() = Some(GeometryStatus {
            ok,
            longest_string: longest,
            max_pixels: max,
            needed_pixels: needed,
            message,
        });
    }

    fn reload(&mut self, show: Arc<Show>) {
        self.show = show;
        let board = board_for(&self.show, &self.node_id, self.identity_board, self.is_pi);
        let kind = choose_backend(&self.app, &self.opts, board, self.is_pi);
        if kind != self.output.kind || (kind == BackendKind::Dpi && board != self.output.board) {
            tracing::info!("pixel output changes to {kind:?} for board {board:?}");
            self.output.backend.stop();
            self.output = OutputStage::new(kind, board, self.opts.sim_refresh_hz);
            self.planned = None;
        }
        let align = self.show.settings.output.latch_align;
        if align != self.latch_align || self.output.backend.stats().bottom_aligned != align {
            if align != self.latch_align {
                tracing::info!(
                    "pixel strings are now {} (experimental latch alignment)",
                    if align {
                        "bottom-aligned"
                    } else {
                        "top-aligned"
                    }
                );
            }
            self.latch_align = align;
            self.output.backend.set_bottom_align(align);
        }
        // The sound delay moves the lights timeline: a new epoch makes the
        // followers jump to it at once (live while calibrating).
        let delay = self.show.settings.audio.output_delay_ms as f64;
        if delay != self.output_delay_ms {
            self.output_delay_ms = delay;
            self.epoch += 1;
        }
        self.board = board;
        self.rebuild_maps();
        let show = self.show.clone();
        self.overlays.retain_props(|id| show.prop(id).is_some());
        self.rebuild_mask();
        // Settings: volume set in the UI, audio device.
        let sv = show.settings.audio.volume.min(100);
        if sv != self.settings_volume {
            self.settings_volume = sv;
            self.volume = sv;
            self.save_levels();
        }
        // Looks: pick up edited presets.
        if let Some(look) = &self.look {
            let id = look.id.clone();
            if let Some(p) = find_effect(&show, &id) {
                if p != look.layer.preset {
                    self.set_look(Some(id), true);
                }
            }
        }
        self.idle_layer = None;
    }

    fn set_identity(&mut self, id: String, role: LocalRole, board: Option<BoardKind>) {
        let was_follower = self.is_follower();
        self.node_id = id;
        self.role = role;
        self.identity_board = board;
        let now_is_follower = self.is_follower();
        if was_follower != now_is_follower {
            tracing::info!("player role is now {role:?}");
            self.clear_playback();
            self.look = None;
            self.follow = FollowState::default();
            self.audio = AudioEngine::disabled("audio is disabled on this controller");
            self.audio_pending = None;
            self.audio_retry_at = Instant::now();
            self.audio_backoff = Duration::from_secs(15);
        }
        let show = self.show.clone();
        self.reload(show);
    }

    // ----- commands ------------------------------------------------------

    fn handle(&mut self, cmd: CoreCmd) {
        let now_ms = self.now_ms();
        match cmd {
            CoreCmd::Player(p) => self.handle_player(p, now_ms),
            CoreCmd::Show(show) => {
                if show.version != self.show.version {
                    self.reload(show);
                }
            }
            CoreCmd::Facts(f) => {
                self.facts = f;
                if !self.is_follower() {
                    self.run_scheduler(now_ms);
                }
            }
            CoreCmd::Identity { id, role, board } => {
                if id != self.node_id || role != self.role || board != self.identity_board {
                    self.set_identity(id, role, board);
                }
            }
            CoreCmd::DjRendered { clip_id, path } => {
                self.dj_rendered.insert(clip_id, path);
            }
            CoreCmd::Context(ctx) => self.set_context(*ctx),
            CoreCmd::Measured(m) => {
                for (group, amps) in m {
                    self.limiter.feedback(&group, amps);
                }
            }
            CoreCmd::Shutdown => {}
        }
    }

    fn handle_player(&mut self, cmd: PlayerCmd, now_ms: f64) {
        match cmd {
            PlayerCmd::Play(req, reply) => {
                let r = self.play(req, now_ms);
                let _ = reply.send(r);
            }
            PlayerCmd::Stop { fade } => self.stop(fade, true, now_ms),
            PlayerCmd::Pause => self.set_paused(true, now_ms),
            PlayerCmd::Resume => self.set_paused(false, now_ms),
            PlayerCmd::Next => self.next(now_ms),
            PlayerCmd::Previous => self.previous(now_ms),
            PlayerCmd::Seek(pos) => self.seek(pos as f64, now_ms),
            PlayerCmd::SetVolume(v) => {
                self.volume = v.min(100);
                self.save_levels();
            }
            PlayerCmd::SetBrightness(b) => {
                self.brightness = b.min(100);
                self.save_levels();
            }
            PlayerCmd::Blackout(on) => {
                self.blackout = on;
                self.save_levels();
            }
            PlayerCmd::TestStart(req, reply) => {
                if let (Some(plan), Some(id)) = (&req.map, &req.map_run_id) {
                    self.map_plans.retain(|(k, _)| k != id);
                    self.map_plans.push_back((id.clone(), plan.clone()));
                    while self.map_plans.len() > MAP_PLANS_KEPT {
                        self.map_plans.pop_front();
                    }
                }
                let plan = self.plan_for(&req);
                if !self.is_follower() && req.mode == "mapCode" && req.map.is_some() {
                    let _ = self.events.send(CoreEvent::PushTest(Box::new(req.clone())));
                }
                let r = match TestLayer::with_plan(
                    &self.show,
                    &self.node_id,
                    &req,
                    now_ms,
                    plan.as_ref(),
                ) {
                    Ok(t) => {
                        if t.is_timeline() {
                            self.test_epoch += 1;
                            // Mapping and calibration patterns need a clean display.
                            self.surprise = None;
                        }
                        self.test = Some(t);
                        Ok(())
                    }
                    Err(e) => Err(ApiError::bad_request(e)),
                };
                let _ = reply.send(r);
            }
            PlayerCmd::TestStop => self.test = None,
            PlayerCmd::Enqueue { sequence_id, name } => self.enqueue(sequence_id, name, now_ms),
            PlayerCmd::Sync(p) => {
                if self.is_follower() {
                    self.on_sync(p, now_ms);
                }
            }
            PlayerCmd::Calibrate(on) => self.calibrate(on, now_ms),
            PlayerCmd::CalibrateV2(seed) => self.calibrate_v2(seed, now_ms),
            PlayerCmd::Surprise(req, reply) => {
                let r = self.start_surprise(req, now_ms);
                let _ = reply.send(r);
            }
            PlayerCmd::SurpriseStop => self.surprise = None,
            PlayerCmd::Overlay(o) => self.overlay_cmd(o),
            PlayerCmd::Reload => {
                let show = self.app.store.get();
                self.reload(show);
            }
        }
    }

    fn set_context(&mut self, ctx: EngineContext) {
        self.limiter.set_budget(&self.node_id, ctx.budget);
        self.smart = ctx.smart;
        if ctx.disabled != self.disabled_ids {
            self.disabled_ids = ctx.disabled;
            self.rebuild_mask();
        }
    }

    /// Slots of the props kept dark (F8), for the current show.
    fn rebuild_mask(&mut self) {
        self.disabled_slots = self
            .disabled_ids
            .iter()
            .filter_map(|id| self.show.prop(id))
            .map(PropSlot::of)
            .collect();
        let most = self.disabled_slots.iter().map(|s| s.len).max().unwrap_or(0);
        self.zeros = vec![0; most];
    }

    /// The mapping plan a test refers to (inline, or by run id).
    fn plan_for(&self, req: &TestRequest) -> Option<MapPlan> {
        req.map.clone().or_else(|| {
            let id = req.map_run_id.as_ref()?;
            self.map_plans
                .iter()
                .find(|(k, _)| k == id)
                .map(|(_, p)| p.clone())
        })
    }

    fn journal(&self, ev: JournalEvent) {
        if !self.is_follower() {
            self.app.services.journal.record(ev);
        }
    }

    fn journal_end(&self, a: &Active, ended_by: &str) {
        if !a.started || matches!(a.kind, ActiveKind::Flash(_)) {
            return;
        }
        self.journal(JournalEvent::ItemEnd {
            item: a.iref.kind.clone(),
            id: a.iref.id.clone(),
            name: a.iref.name.clone(),
            dur_ms: (self.now_ms() - a.begun_ms).max(0.0) as u64,
            ended_by: ended_by.into(),
        });
    }

    /// Start a surprise over whatever plays (F20; leader).
    fn start_surprise(&mut self, req: SurpriseRequest, now_ms: f64) -> ApiResult<SurpriseStarted> {
        let calibrating = self
            .current
            .as_ref()
            .is_some_and(|a| a.iref.kind == CALIBRATION_ID);
        let testing = self.test.as_ref().is_some_and(|t| !t.is_look());
        if let Some(why) =
            surprise::refusal(self.is_follower(), self.blackout, testing, calibrating)
        {
            return Err(ApiError::conflict(why));
        }
        let show = self.show.clone();
        let kind = if req.kind == "sequence" {
            "sequence"
        } else {
            "effect"
        };
        let default_ms = match kind {
            "sequence" => {
                show.sequence(&req.r#ref)
                    .ok_or_else(|| {
                        ApiError::bad_request("That surprise's sequence no longer exists.")
                    })?
                    .duration_ms
            }
            _ => {
                find_effect(&show, &req.r#ref).ok_or_else(|| {
                    ApiError::bad_request("That surprise's look no longer exists.")
                })?;
                surprise::DEFAULT_EFFECT_MS
            }
        };
        let duration_ms = req
            .duration_ms
            .filter(|d| *d > 0)
            .unwrap_or(default_ms)
            .min(if kind == "sequence" {
                default_ms.max(surprise::MIN_MS)
            } else {
                u64::MAX
            })
            .clamp(surprise::MIN_MS, surprise::MAX_MS);
        let targets: Vec<String> = req
            .targets
            .iter()
            .filter(|id| show.prop(id).is_some())
            .cloned()
            .collect();
        if !req.targets.is_empty() && targets.is_empty() {
            return Err(ApiError::bad_request(
                "The surprise's props no longer exist.",
            ));
        }
        self.surprise_seq += 1;
        let anchor = SurpriseAnchor {
            id: req.id.clone(),
            kind: kind.into(),
            r#ref: req.r#ref.clone(),
            targets,
            start_pos: 0.0,
            duration_ms,
            epoch: ((now_ms.max(0.0) as u64) << 8) | (self.surprise_seq & 0xff),
        };
        let layer = SurpriseLayer::new(&show, &self.app.config.data_dir, anchor, now_ms)
            .map_err(ApiError::bad_request)?;
        let started = SurpriseStarted {
            name: layer.name.clone(),
            duration_ms,
            props: if layer.anchor.targets.is_empty() {
                show.props.len()
            } else {
                layer.anchor.targets.len()
            },
            replaced: self.surprise.is_some(),
        };
        tracing::info!("surprise: {} for {} ms", started.name, duration_ms);
        self.surprise = Some(layer);
        Ok(started)
    }

    /// Remember the leader's levels for the next start (written off this thread).
    fn save_levels(&self) {
        if self.is_follower() {
            return;
        }
        let _ = self.events.send(CoreEvent::Levels(SavedLevels {
            brightness: self.brightness,
            volume: self.volume,
            blackout: self.blackout,
        }));
    }

    fn overlay_cmd(&mut self, cmd: OverlayCmd) {
        let show = self.show.clone();
        let now = Instant::now();
        let prop_of = |id: &str| show.prop(id);
        match cmd {
            OverlayCmd::Open { prop_id, reply } => {
                let r = match prop_of(&prop_id) {
                    Some(p) => self.overlays.open(p).map_err(ApiError::internal),
                    None => Err(ApiError::not_found("That prop")),
                };
                let _ = reply.send(r);
            }
            OverlayCmd::Enable { prop_id, enabled } => {
                if let Some(p) = prop_of(&prop_id) {
                    self.overlays.enable(p, enabled);
                }
            }
            OverlayCmd::Frame { prop_id, rgb } => {
                if let Some(p) = prop_of(&prop_id) {
                    self.overlays.set_frame(p, &rgb, now);
                }
            }
            OverlayCmd::PropPixels { prop_id, rgb } => {
                if let Some(p) = prop_of(&prop_id) {
                    self.overlays.set_prop_pixels(p, &rgb, now);
                }
            }
            OverlayCmd::Text {
                prop_id,
                text,
                color,
                scroll,
                duration_ms,
            } => {
                if let Some(p) = prop_of(&prop_id) {
                    self.overlays
                        .text(p, &text, &color, scroll, duration_ms, now);
                }
            }
            OverlayCmd::Qr {
                prop_id,
                url,
                duration_ms,
            } => {
                if let Some(p) = prop_of(&prop_id) {
                    if let Err(e) = self.overlays.qr(p, &url, duration_ms, now) {
                        self.warn(format!("Can't show the QR code on {}: {e}", p.name), true);
                    }
                }
            }
        }
    }

    fn play(&mut self, req: PlayRequest, now_ms: f64) -> ApiResult<()> {
        if self.is_follower() {
            return Err(ApiError::conflict(
                "This controller follows its show leader; control playback on the leader.",
            ));
        }
        // Pressing play ends a live look ("Effects → Show live"): it is drawn on top of
        // everything and would otherwise hide what was just started.
        if self.test.as_ref().is_some_and(|t| t.req.mode == "effect") {
            self.test = None;
        }
        let show = self.show.clone();
        let (source, first, playlist, crossfade) = if let Some(id) = &req.playlist_id {
            let pl = show
                .playlist(id)
                .ok_or_else(|| ApiError::not_found("That playlist"))?;
            // Manual play (scheduler.rs rules): "Loop until I stop" repeats; outside
            // the show windows a playlist plays once; inside one it follows its
            // own `repeat` until the window ends.
            let mut pl = self.smart_playlist(pl);
            if req.loop_until_stopped {
                pl.repeat = true;
            } else if !(self.facts.enabled && self.facts.active.is_some()) {
                pl.repeat = false;
            }
            let mut cursor = PlaylistCursor::new(pl.clone());
            if let Some(i) = req.start_index {
                cursor.start_at(i as usize);
            }
            let first = cursor
                .current()
                .cloned()
                .ok_or_else(|| ApiError::bad_request("That playlist is empty."))?;
            (
                Source::Playlist(Box::new(cursor)),
                Pending::Item(first),
                Some((pl.id.clone(), pl.name.clone())),
                pl.crossfade_ms,
            )
        } else if let Some(id) = &req.sequence_id {
            show.sequence(id)
                .ok_or_else(|| ApiError::not_found("That sequence"))?;
            (
                Source::Single,
                Pending::Item(PlaylistItem::Sequence {
                    id: "manual".into(),
                    sequence_id: id.clone(),
                }),
                None,
                0,
            )
        } else if let Some(id) = &req.dj_clip_id {
            show.dj_clip(id)
                .ok_or_else(|| ApiError::not_found("That DJ clip"))?;
            (
                Source::Single,
                Pending::Item(PlaylistItem::Dj {
                    id: "manual".into(),
                    dj_clip_id: id.clone(),
                }),
                None,
                0,
            )
        } else if let Some(id) = &req.effect_id {
            find_effect(&show, id).ok_or_else(|| ApiError::not_found("That look"))?;
            (
                Source::Single,
                Pending::Item(PlaylistItem::Effect {
                    id: "manual".into(),
                    effect_id: id.clone(),
                    duration_ms: 0,
                }),
                None,
                0,
            )
        } else if let Some(id) = &req.media_id {
            show.media_item(id)
                .ok_or_else(|| ApiError::not_found("That audio file"))?;
            (
                Source::Single,
                Pending::Item(PlaylistItem::Media {
                    id: "manual".into(),
                    media_id: id.clone(),
                }),
                None,
                0,
            )
        } else {
            return Err(ApiError::bad_request(
                "Choose a playlist, sequence, DJ clip, look or audio file to play.",
            ));
        };
        self.start_program(
            Program {
                origin: Origin::manual(&self.facts, req.loop_until_stopped),
                source,
                playlist,
                crossfade_ms: crossfade,
            },
            first,
            now_ms,
        );
        match &self.program {
            Some(p) if p.origin.is_manual() => Ok(()),
            _ => Err(ApiError::bad_request(
                self.item_error
                    .as_ref()
                    .map(|e| e.0.clone())
                    .unwrap_or_else(|| "Nothing playable.".into()),
            )),
        }
    }

    /// Start or stop the calibration pattern (leader only).
    fn calibrate(&mut self, on: bool, now_ms: f64) {
        if self.is_follower() {
            return;
        }
        let running_any = self
            .current
            .as_ref()
            .is_some_and(|a| a.iref.kind == CALIBRATION_ID);
        let running = running_any
            && self
                .current
                .as_ref()
                .is_some_and(|a| a.iref.id == CALIBRATION_ID);
        if on && !running {
            self.test = None;
            self.start_program(
                Program {
                    // A tool with its own length: the schedule never ends it.
                    origin: Origin::Manual {
                        window: None,
                        looping: true,
                    },
                    source: Source::Single,
                    playlist: None,
                    crossfade_ms: 0,
                },
                Pending::Calibration,
                now_ms,
            );
        } else if !on && running_any {
            self.stop(false, true, now_ms);
        }
    }

    /// Start the phone calibration pattern (F1 v2) with `seed` (leader).
    fn calibrate_v2(&mut self, seed: u32, now_ms: f64) {
        if self.is_follower() {
            return;
        }
        self.test = None;
        self.surprise = None;
        self.start_program(
            Program {
                origin: Origin::Manual {
                    window: None,
                    looping: true,
                },
                source: Source::Single,
                playlist: None,
                crossfade_ms: 0,
            },
            Pending::CalibrationV2(seed),
            now_ms,
        );
    }

    fn start_program(&mut self, program: Program, first: Pending, now_ms: f64) {
        // Replace whatever plays (quick fade to avoid clicks).
        self.audio.stop_all(80);
        if let Some(a) = self.current.take() {
            self.journal_end(&a, "stopped");
        }
        self.end_show_window();
        self.outgoing = None;
        self.stop_fade = None;
        self.paused = false;
        self.program = Some(program);
        self.start_next(Some(first), now_ms, 0);
    }

    fn stop(&mut self, fade: bool, user: bool, now_ms: f64) {
        if user {
            let origin = self.program.as_ref().map(|p| p.origin.clone());
            self.sched.on_user_stop(&self.facts, origin.as_ref());
        }
        if self.program.is_none() {
            return;
        }
        if fade {
            if self.stop_fade.is_none() {
                self.stop_fade = Some(Fade::new(now_ms, STOP_FADE_MS, 1.0, 0.0));
                self.audio.stop_all(STOP_FADE_MS as u32);
            }
        } else {
            self.audio.stop_all(30);
            self.clear_playback();
        }
    }

    fn clear_playback(&mut self) {
        if let Some(a) = self.current.take() {
            self.journal_end(&a, "stopped");
            if let Some(id) = a.audio {
                self.audio.stop(id);
            }
        }
        self.end_show_window();
        if let Some(o) = self.outgoing.take() {
            if let Some(id) = o.active.audio {
                self.audio.stop(id);
            }
        }
        self.program = None;
        self.preload = None;
        self.paused = false;
        self.stop_fade = None;
    }

    fn set_paused(&mut self, paused: bool, now_ms: f64) {
        if self.program.is_none() || self.paused == paused {
            return;
        }
        self.paused = paused;
        self.epoch += 1;
        for a in self
            .current
            .iter_mut()
            .chain(self.outgoing.iter_mut().map(|o| &mut o.active))
        {
            a.clock.set_paused(paused, now_ms);
            if let Some(id) = a.audio {
                self.audio.set_paused(id, paused);
            }
        }
    }

    fn seek(&mut self, pos: f64, now_ms: f64) {
        let Some(a) = self.current.as_mut() else {
            return;
        };
        let pos = match a.duration_ms {
            Some(d) => pos.clamp(0.0, d as f64),
            None => pos.max(0.0),
        };
        a.clock.seek(pos, now_ms);
        a.last_pos = pos;
        self.epoch += 1;
        if let Some(o) = self.outgoing.take() {
            if let Some(id) = o.active.audio {
                self.audio.stop(id);
            }
        }
        if let Some(id) = a.audio.take() {
            self.audio.stop(id);
        }
        if a.started {
            if let Some((path, gain)) = a.audio_src.clone() {
                a.audio = self.audio.play(&path, pos as u64, gain, 20);
                a.use_audio_clock = a.audio.is_some();
                if let (Some(id), true) = (a.audio, self.paused) {
                    self.audio.set_paused(id, true);
                }
            }
        }
    }

    fn next(&mut self, now_ms: f64) {
        if self.program.is_none() || self.stop_fade.is_some() {
            return;
        }
        self.end_current(120);
        self.paused = false;
        let next = self.take_next();
        self.start_next(next, now_ms, 0);
    }

    fn previous(&mut self, now_ms: f64) {
        if self.program.is_none() || self.stop_fade.is_some() {
            return;
        }
        if self.current.as_ref().is_some_and(|a| a.last_pos > 3000.0) {
            self.seek(0.0, now_ms);
            return;
        }
        let prev = match self.program.as_mut().map(|p| &mut p.source) {
            Some(Source::Playlist(c)) => c.previous().cloned().map(Pending::Item),
            _ => None,
        };
        match prev {
            Some(item) => {
                self.end_current(120);
                self.paused = false;
                self.start_next(Some(item), now_ms, 0);
            }
            None => self.seek(0.0, now_ms),
        }
    }

    fn enqueue(&mut self, sequence_id: String, name: Option<String>, now_ms: f64) {
        if self.is_follower() {
            return;
        }
        if self.show.sequence(&sequence_id).is_none() {
            self.warn(
                "A song request was for a sequence that no longer exists",
                false,
            );
            return;
        }
        if self.program.is_none() || self.stop_fade.is_some() {
            if self.facts.enabled && self.facts.active.is_none() {
                // Outside the show windows a request never starts the music.
                tracing::info!("song request ignored: the show is not on");
                return;
            }
            self.start_program(
                Program {
                    origin: Origin::manual(&self.facts, false),
                    source: Source::Single,
                    playlist: None,
                    crossfade_ms: 0,
                },
                Pending::Request { sequence_id, name },
                now_ms,
            );
        } else {
            self.requests.push_back((sequence_id, name));
        }
    }

    /// Journal the end of the scheduled show (if one was running).
    fn end_show_window(&mut self) {
        if let Some((entry_id, name)) = self.show_window.take() {
            self.journal(JournalEvent::ShowEnd { entry_id, name });
        }
    }

    /// Stop the current item's audio (quick fade) and drop it.
    fn end_current(&mut self, fade_ms: u32) {
        if let Some(a) = self.current.take() {
            self.journal_end(&a, "skipped");
            if let Some(id) = a.audio {
                self.audio.fade_out(id, fade_ms);
            }
        }
        if let Some(o) = self.outgoing.take() {
            if let Some(id) = o.active.audio {
                self.audio.fade_out(id, fade_ms);
            }
        }
    }

    // ----- sequencing ----------------------------------------------------

    fn take_next(&mut self) -> Option<Pending> {
        if let Some((sequence_id, name)) = self.requests.pop_front() {
            return Some(Pending::Request { sequence_id, name });
        }
        // Smart playlists re-expand for every repeat pass (F18).
        let smart_items = self
            .program
            .as_ref()
            .and_then(|p| p.playlist.as_ref())
            .and_then(|(id, _)| self.show.playlist(id))
            .filter(|pl| pl.smart.is_some())
            .map(|pl| self.smart_playlist(pl).items);
        if let (Some(items), Some(Source::Playlist(c))) =
            (smart_items, self.program.as_mut().map(|p| &mut p.source))
        {
            c.set_next_cycle_items(items);
        }
        match self.program.as_mut().map(|p| &mut p.source) {
            Some(Source::Playlist(c)) => c.advance().cloned().map(Pending::Item),
            _ => None,
        }
    }

    fn peek_next(&self) -> Option<Pending> {
        if let Some((sequence_id, name)) = self.requests.front() {
            return Some(Pending::Request {
                sequence_id: sequence_id.clone(),
                name: name.clone(),
            });
        }
        match self.program.as_ref().map(|p| &p.source) {
            Some(Source::Playlist(c)) => c.peek_next().cloned().map(Pending::Item),
            _ => None,
        }
    }

    /// Start `first` (or, if it cannot play, the items after it).
    fn start_next(&mut self, first: Option<Pending>, now_ms: f64, fade_in_ms: u32) {
        let mut item = first;
        let mut attempts = 0;
        loop {
            let Some(p) = item else {
                self.playback_finished();
                return;
            };
            match self.begin(&p, now_ms) {
                Ok(Some(mut active)) => {
                    self.epoch += 1;
                    active.fade_in_ms = fade_in_ms;
                    if !matches!(active.kind, ActiveKind::Flash(_)) {
                        self.journal(JournalEvent::ItemStart {
                            item: active.iref.kind.clone(),
                            id: active.iref.id.clone(),
                            name: active.iref.name.clone(),
                            playlist_id: self
                                .program
                                .as_ref()
                                .and_then(|p| p.playlist.as_ref())
                                .map(|p| p.0.clone()),
                        });
                    }
                    self.current = Some(active);
                    self.after_begin(&p);
                    return;
                }
                Ok(None) => {} // instant item (command)
                Err(e) => {
                    let name = self.pending_ref(&p).name;
                    let msg = format!("Skipped “{name}”: {e}");
                    self.item_error = Some((msg.clone(), Instant::now()));
                    self.journal(JournalEvent::Error {
                        code: "itemSkipped".into(),
                        msg: msg.clone(),
                    });
                    self.warn(msg, true);
                }
            }
            attempts += 1;
            if attempts >= 64 {
                self.warn(
                    "Nothing in this playlist can be played right now; stopping.",
                    true,
                );
                self.playback_finished();
                return;
            }
            item = self.take_next();
        }
    }

    fn playback_finished(&mut self) {
        if let Some(Program {
            origin: Origin::Schedule(key),
            ..
        }) = &self.program
        {
            self.sched.on_finished(key);
        }
        self.clear_playback();
        let now_ms = self.now_ms();
        self.run_scheduler(now_ms);
    }

    /// After an item started: pre-render an upcoming dynamic DJ clip.
    fn after_begin(&mut self, started: &Pending) {
        let Some(Pending::Item(PlaylistItem::Dj { dj_clip_id, .. })) = self.peek_next() else {
            return;
        };
        let Some(clip) = self.show.dj_clip(&dj_clip_id) else {
            return;
        };
        if !clip.dynamic {
            return;
        }
        // The song after the DJ clip.
        let next_song = match self.program.as_ref().map(|p| &p.source) {
            Some(Source::Playlist(c)) => {
                let mut c = c.clone();
                c.advance();
                c.advance()
                    .cloned()
                    .map(|i| self.pending_ref(&Pending::Item(i)).name)
            }
            _ => None,
        };
        let request_name = match started {
            Pending::Request { name, .. } => name.clone(),
            _ => None,
        };
        let ctx = DynamicContext {
            next_song,
            prev_song: Some(self.pending_ref(started).name),
            request_name,
            ..Default::default()
        };
        // Forget the previous showtime render (its placeholders are stale); if the
        // new one is not ready in time the clip's last saved render plays.
        self.dj_rendered.remove(&dj_clip_id);
        let _ = self.events.send(CoreEvent::PrerenderDj {
            clip_id: dj_clip_id,
            ctx,
        });
    }

    fn pending_ref(&self, p: &Pending) -> ItemRef {
        let show = &self.show;
        match p {
            Pending::Request { sequence_id, .. } => {
                let name = show
                    .sequence(sequence_id)
                    .map_or("Song request", |s| s.name.as_str());
                item_ref("request", sequence_id, name)
            }
            Pending::Calibration => {
                item_ref(CALIBRATION_ID, CALIBRATION_ID, "Sync lights to sound")
            }
            Pending::CalibrationV2(seed) => item_ref(
                CALIBRATION_ID,
                &format!("{CALIBRATION_V2_PREFIX}{seed}"),
                "Measure with my phone",
            ),
            Pending::Item(i) => match i {
                PlaylistItem::Sequence { sequence_id, .. } => item_ref(
                    "sequence",
                    sequence_id,
                    show.sequence(sequence_id)
                        .map_or("Missing sequence", |s| &s.name),
                ),
                PlaylistItem::Dj { dj_clip_id, .. } => item_ref(
                    "dj",
                    dj_clip_id,
                    show.dj_clip(dj_clip_id)
                        .map_or("Missing DJ clip", |c| &c.name),
                ),
                PlaylistItem::Effect { effect_id, .. } => {
                    let name = find_effect(show, effect_id)
                        .map(|e| e.name)
                        .unwrap_or_else(|| "Missing look".into());
                    item_ref("effect", effect_id, &name)
                }
                PlaylistItem::Media { media_id, .. } => item_ref(
                    "media",
                    media_id,
                    show.media_item(media_id)
                        .map_or("Missing audio", |m| &m.name),
                ),
                PlaylistItem::Pause { id, .. } => item_ref("pause", id, "Pause"),
                PlaylistItem::Command { id, command, .. } => item_ref("command", id, command),
                PlaylistItem::Countdown { id, .. } => item_ref("countdown", id, "Countdown"),
            },
        }
    }

    fn media_path(&self, file: &str) -> Option<PathBuf> {
        let data = &self.app.config.data_dir;
        let p = data.join(file);
        if p.exists() {
            return Some(p);
        }
        let p = self.app.config.media_dir().join(file);
        p.exists().then_some(p)
    }

    fn media_gain(&self, media: &pixelplus_core::model::Media) -> f32 {
        let a = &self.show.settings.audio;
        if !a.normalize {
            return 0.0;
        }
        media
            .loudness_lufs
            .map(|l| a.target_lufs - l)
            .or(media.gain_db)
            .unwrap_or(0.0)
            .clamp(-12.0, 12.0)
    }

    /// Prepare an item. `Ok(None)` for instant items (commands).
    fn begin(&mut self, p: &Pending, now_ms: f64) -> Result<Option<Active>, String> {
        let iref = self.pending_ref(p);
        let show = self.show.clone();
        let single = matches!(
            self.program.as_ref().map(|p| &p.source),
            Some(Source::Single)
        );
        let seq_id = match p {
            Pending::Request { sequence_id, .. } => Some(sequence_id.clone()),
            Pending::Item(PlaylistItem::Sequence { sequence_id, .. }) => Some(sequence_id.clone()),
            _ => None,
        };
        if let Some(seq_id) = seq_id {
            let seq = show.sequence(&seq_id).ok_or("the sequence was deleted")?;
            let path = self.app.config.data_dir.join(&seq.file);
            if !path.exists() {
                return Err(format!("the sequence file {} is missing", seq.file));
            }
            let reader = match self.preload.take() {
                Some((id, r)) if id == seq_id => r,
                _ => FrameReader::open(path, 0),
            };
            let mut a = Active::new(iref, ActiveKind::Sequence { reader, meta: None }, now_ms);
            a.duration_ms = Some(seq.duration_ms);
            if let Some(mid) = &seq.media_id {
                match show.media_item(mid) {
                    Some(m) => match self.media_path(&m.file) {
                        Some(path) => a.audio_src = Some((path, self.media_gain(m))),
                        None => self.warn(
                            format!(
                                "The audio for “{}” is missing; playing lights only",
                                seq.name
                            ),
                            true,
                        ),
                    },
                    None => self.warn(
                        format!(
                            "The audio linked to “{}” was deleted; playing lights only",
                            seq.name
                        ),
                        false,
                    ),
                }
            }
            return Ok(Some(a));
        }
        if matches!(p, Pending::Calibration) {
            let path = calibration_click(&self.app.config.data_dir)
                .map_err(|e| format!("could not write the calibration sound: {e}"))?;
            let mut a = Active::new(iref, ActiveKind::Flash(None), now_ms);
            a.audio_src = Some((path, 0.0));
            a.duration_ms = Some(CALIBRATION_LEN_MS);
            return Ok(Some(a));
        }
        if let Pending::CalibrationV2(seed) = p {
            let path = calibration_v2_wav(&self.app.config.data_dir, *seed)
                .map_err(|e| format!("could not write the calibration sound: {e}"))?;
            let sched = calpattern::schedule(*seed, self.slot_ms);
            let mut a = Active::new(iref, ActiveKind::Flash(Some(Box::new(sched))), now_ms);
            a.audio_src = Some((path, 0.0));
            a.duration_ms = Some(calpattern::RUN_MS);
            return Ok(Some(a));
        }
        let Pending::Item(item) = p else {
            unreachable!("requests are sequences")
        };
        match item {
            PlaylistItem::Sequence { .. } => unreachable!("handled above"),
            PlaylistItem::Dj { dj_clip_id, .. } => {
                let (path, gain, duration) = self.dj_audio(dj_clip_id)?;
                let mut a = Active::new(iref, ActiveKind::Audio, now_ms);
                a.audio_src = Some((path, gain));
                a.duration_ms = duration.filter(|&d| d > 0);
                Ok(Some(a))
            }
            PlaylistItem::Media { media_id, .. } => {
                let m = show
                    .media_item(media_id)
                    .ok_or("the audio file was deleted")?;
                let path = self
                    .media_path(&m.file)
                    .ok_or("the audio file is missing")?;
                let mut a = Active::new(iref, ActiveKind::Audio, now_ms);
                a.audio_src = Some((path, self.media_gain(m)));
                a.duration_ms = (m.duration_ms > 0).then_some(m.duration_ms);
                Ok(Some(a))
            }
            PlaylistItem::Effect {
                effect_id,
                duration_ms,
                ..
            } => {
                let preset = find_effect(&show, effect_id).ok_or("the look was deleted")?;
                let layer = EffectLayer::new(&show, &preset);
                let mut a = Active::new(iref, ActiveKind::Effect(Box::new(layer)), now_ms);
                a.duration_ms = match (*duration_ms, single) {
                    (0, true) => None,
                    (0, false) => Some(30_000),
                    (d, _) => Some(d),
                };
                Ok(Some(a))
            }
            PlaylistItem::Pause { duration_ms, .. } => {
                let mut a = Active::new(iref, ActiveKind::Pause, now_ms);
                a.duration_ms = Some(*duration_ms);
                Ok(Some(a))
            }
            PlaylistItem::Command { command, args, .. } => {
                self.run_command(command, args);
                Ok(None)
            }
            PlaylistItem::Countdown {
                id,
                duration_ms,
                matrix_prop_id,
                text,
                color,
                others,
                finale,
                dj_clip_id,
                dj_offset_ms,
                tick,
            } => {
                let preset = countdown::countdown_preset(
                    &show,
                    id,
                    *duration_ms,
                    matrix_prop_id.as_deref(),
                    text,
                    color.as_deref(),
                    *others,
                    *finale,
                );
                let dur = countdown::CountdownSpec::from_params(&preset.params).duration_ms;
                let layer = EffectLayer::new(&show, &preset);
                let mut a = Active::new(iref, ActiveKind::Effect(Box::new(layer)), now_ms);
                a.duration_ms = Some(dur);
                if let Some(clip) = dj_clip_id {
                    // The clip ends at zero (+ offset): "Showtime in 3, 2, 1…".
                    match self.dj_audio(clip) {
                        Ok((path, gain, clip_ms)) => {
                            let start =
                                dur as i64 - clip_ms.unwrap_or(0) as i64 + i64::from(*dj_offset_ms);
                            a.delayed_audio = Some(DelayedAudio {
                                path,
                                gain,
                                at_pos: start.max(0) as f64,
                                skip_ms: (-start).max(0) as u64,
                            });
                            a.detach_audio = true;
                        }
                        Err(e) => self.warn(
                            format!("The countdown plays without its DJ clip: {e}"),
                            false,
                        ),
                    }
                } else if *tick {
                    match countdown_ticks(&self.app.config.data_dir, dur) {
                        Ok(path) => a.audio_src = Some((path, 0.0)),
                        Err(e) => tracing::warn!("countdown tick sound: {e}"),
                    }
                }
                Ok(Some(a))
            }
        }
    }

    /// A DJ clip's audio: its showtime render or its saved audio, with the
    /// gain and length.
    fn dj_audio(&self, clip_id: &str) -> Result<(PathBuf, f32, Option<u64>), String> {
        let show = self.show.clone();
        let clip = show.dj_clip(clip_id).ok_or("the DJ clip was deleted")?;
        let rendered = self
            .dj_rendered
            .get(clip_id)
            .filter(|p| p.exists())
            .cloned();
        let media = clip.media_id.as_deref().and_then(|m| show.media_item(m));
        Ok(match (rendered, media) {
            (Some(p), m) => (p, 0.0, m.map(|m| m.duration_ms)),
            (None, Some(m)) => (
                self.media_path(&m.file)
                    .ok_or("the DJ clip's audio file is missing")?,
                self.media_gain(m),
                Some(m.duration_ms),
            ),
            (None, None) => return Err("the DJ clip has not been rendered yet".into()),
        })
    }

    fn run_command(&mut self, command: &str, args: &serde_json::Value) {
        match command {
            "games.invite" | "games.stop" => {
                let mut cmd = serde_json::json!({ "cmd": if command == "games.invite" { "invite" } else { "stop" } });
                if let (Some(obj), Some(extra)) = (cmd.as_object_mut(), args.as_object()) {
                    for (k, v) in extra {
                        if k != "cmd" {
                            obj.insert(k.clone(), v.clone());
                        }
                    }
                }
                let _ = self.events.send(CoreEvent::Games(cmd));
            }
            "overlay.text" => {
                let s = |k: &str| args.get(k).and_then(|v| v.as_str()).map(str::to_string);
                let Some(prop_id) = s("propId") else {
                    self.warn("overlay.text needs a propId", false);
                    return;
                };
                let show = self.show.clone();
                let Some(prop) = show.prop(&prop_id) else {
                    self.warn("overlay.text: that prop no longer exists", false);
                    return;
                };
                let text = s("text").unwrap_or_default();
                let color = s("color").unwrap_or_else(|| "#ffffff".into());
                let scroll = args.get("scroll").and_then(|v| v.as_bool()).unwrap_or(true);
                let duration = args
                    .get("durationMs")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(15_000);
                self.overlays
                    .text(prop, &text, &color, scroll, duration, Instant::now());
            }
            other => self.warn(format!("Unknown playlist command “{other}” skipped"), false),
        }
    }

    // ----- scheduler -----------------------------------------------------

    fn run_scheduler(&mut self, now_ms: f64) {
        if self.is_follower() {
            return;
        }
        let origin = if self.stop_fade.is_some() {
            None
        } else {
            self.program.as_ref().map(|p| p.origin.clone())
        };
        if self.stop_fade.is_some() {
            return; // decide once the fade has finished
        }
        let Some(action) = self.sched.decide(&self.facts, origin.as_ref()) else {
            return;
        };
        self.sched.prune(&self.facts);
        match action {
            SchedAction::Start(w) => {
                let show = self.show.clone();
                let Some(pl) = show.playlist(&w.playlist_id) else {
                    self.warn(
                        format!(
                            "The schedule “{}” plays a playlist that no longer exists",
                            w.name
                        ),
                        true,
                    );
                    self.sched.on_finished(&w.key);
                    return;
                };
                let pl = self.smart_playlist(pl);
                let cursor = PlaylistCursor::new(pl.clone());
                let Some(first) = cursor.current().cloned() else {
                    self.sched.on_finished(&w.key);
                    return;
                };
                tracing::info!("schedule: starting “{}” ({})", w.name, pl.name);
                self.look = None;
                // A test pattern or live look left running (a phone put away after
                // testing in the afternoon) would cover the show all evening.
                if let Some(t) = self.test.take() {
                    self.warn(
                        format!(
                            "The show started, so the {} that was still running was turned off",
                            if t.is_look() {
                                "live look"
                            } else {
                                "test pattern"
                            }
                        ),
                        true,
                    );
                }
                self.start_program(
                    Program {
                        origin: Origin::Schedule(w.key.clone()),
                        source: Source::Playlist(Box::new(cursor)),
                        playlist: Some((pl.id.clone(), pl.name.clone())),
                        crossfade_ms: pl.crossfade_ms,
                    },
                    Pending::Item(first),
                    now_ms,
                );
                if self.program.is_some() {
                    // Exact start (F4): the intro began `late_ms` ago.
                    if w.late_ms > 0 {
                        if let Some(a) = self.current.as_mut() {
                            a.start_at_ms = w.late_ms as f64;
                        }
                    }
                    self.journal(JournalEvent::ShowStart {
                        entry_id: w.entry_id.clone(),
                        name: w.name.clone(),
                    });
                    self.show_window = Some((w.entry_id.clone(), w.name.clone()));
                }
            }
            SchedAction::End(behavior) => {
                use pixelplus_core::model::EndBehavior;
                tracing::info!("schedule: window ended ({behavior:?})");
                // Requests still waiting belong to tonight's show.
                self.requests.clear();
                match behavior {
                    EndBehavior::FinishSong => {
                        if let Some(Source::Playlist(c)) =
                            self.program.as_mut().map(|p| &mut p.source)
                        {
                            c.finish_after_current();
                        } else if self.current.as_ref().is_some_and(|a| {
                            matches!(a.kind, ActiveKind::Effect(_)) && a.duration_ms.is_none()
                        }) {
                            // A look started by hand has no end to finish at: fade it.
                            self.stop(true, false, now_ms);
                        }
                    }
                    EndBehavior::StopNow => self.stop(false, false, now_ms),
                    EndBehavior::FadeOut => self.stop(true, false, now_ms),
                }
            }
            SchedAction::Look(id) => self.set_look(id, false),
        }
    }

    /// A smart playlist (F18) with tonight's items; other playlists as they are.
    fn smart_playlist(&self, pl: &Playlist) -> Playlist {
        let mut pl = pl.clone();
        if pl.smart.is_some() {
            pl.items = match self.smart.get(&pl.id) {
                Some(items) => items.clone(),
                // Not expanded yet (just created): expand without history.
                None => {
                    let show = &self.show;
                    let tz = pixelplus_core::schedule::schedule_timezone(&show.schedule)
                        .unwrap_or(chrono_tz::UTC);
                    let now = chrono::Utc::now().with_timezone(&tz);
                    let night = pixelplus_core::smartlist::night_of(now.naive_local());
                    let seed = pixelplus_core::smartlist::night_seed(night, &pl.id);
                    pixelplus_core::smartlist::expand_playlist(
                        show,
                        &pl.id,
                        &Default::default(),
                        now,
                        seed,
                    )
                    .map(|e| e.items)
                    .unwrap_or_default()
                }
            };
        }
        pl
    }

    fn set_look(&mut self, id: Option<String>, force: bool) {
        if !force && self.look.as_ref().map(|l| &l.id) == id.as_ref() {
            return;
        }
        let now_ms = self.now_ms();
        self.look = id.and_then(|id| {
            let preset = find_effect(&self.show, &id)?;
            Some(Look {
                id,
                name: preset.name.clone(),
                layer: EffectLayer::new(&self.show, &preset),
                started_ms: now_ms,
                beat_key: None,
            })
        });
    }

    /// An idle look that "follows the song's beat" (F2): under a song or DJ
    /// clip whose beat was analysed (`Media.analysis`, WS2), its pulse is
    /// stamped with the song's tempo and first beat; followers get the stamped
    /// preset as the sync effect, so every node pulses on the same beats.
    fn sync_idle_beat(&mut self, now_ms: f64) {
        let Some(l) = self.idle_layer.as_mut() else {
            return;
        };
        let song = self
            .current
            .as_ref()
            .filter(|a| a.started && matches!(a.kind, ActiveKind::Audio))
            .and_then(|a| {
                let media_id = match a.iref.kind.as_str() {
                    "media" => Some(a.iref.id.clone()),
                    "dj" => self
                        .show
                        .dj_clip(&a.iref.id)
                        .and_then(|c| c.media_id.clone()),
                    _ => None,
                }?;
                let an = self.show.media_item(&media_id)?.analysis.clone()?;
                Some((format!("{}@{}", a.iref.id, a.begun_ms), an, a.last_pos))
            });
        let want = song.as_ref().map(|s| s.0.clone());
        if want == l.beat_key {
            return;
        }
        let Some(original) = find_effect(&self.show, &l.id) else {
            return;
        };
        let preset = match &song {
            Some((_, an, pos)) => {
                let start = (now_ms - l.started_ms) - pos;
                pixelplus_core::effects::follow_song_beat(
                    &original,
                    an.bpm,
                    an.first_beat_ms,
                    start,
                )
                .unwrap_or(original)
            }
            None => original,
        };
        if preset != l.layer.preset {
            l.layer = EffectLayer::new(&self.show, &preset);
        }
        l.beat_key = want;
    }

    /// The idle look that runs under DJ clips and pauses.
    fn ensure_idle_layer(&mut self) {
        let want = self
            .facts
            .idle_effect_id
            .clone()
            .or_else(|| self.show.schedule.idle_effect_id.clone());
        let have = self.idle_layer.as_ref().map(|l| l.id.clone());
        if want == have {
            return;
        }
        let now_ms = self.now_ms();
        self.idle_layer = want.and_then(|id| {
            let preset = find_effect(&self.show, &id)?;
            Some(Look {
                id,
                name: preset.name.clone(),
                layer: EffectLayer::new(&self.show, &preset),
                started_ms: now_ms,
                beat_key: None,
            })
        });
    }

    // ----- per-frame -----------------------------------------------------

    fn tick(&mut self) {
        let now = Instant::now();
        let now_ms = self.now_ms();
        self.begin_frame(now, now_ms);
        if self.is_follower() {
            self.follower_advance(now_ms);
        } else {
            self.maintain_audio(now);
            self.leader_advance(now_ms);
        }
        self.overlays.update(now);
        if self.is_follower() {
            self.compose_follower(now_ms);
        } else {
            self.compose_leader(now_ms);
        }
        self.write_output(now, now_ms);
        self.preview(now, now_ms);
        self.publish_status(now, now_ms);
        self.publish_extras();
    }

    fn maintain_audio(&mut self, now: Instant) {
        // Volume (with curfew).
        let effective = match self.facts.volume_cap {
            Some(cap) => self.volume.min(cap),
            None => self.volume,
        };
        if self.applied_volume != Some(effective) {
            self.audio.set_volume(effective);
            self.applied_volume = Some(effective);
        }
        // (Re)open after a failure or a device change, only while no audio plays.
        // Opening happens on a helper thread: a hanging device never stalls the lights.
        let wanted = self.show.settings.audio.device.clone();
        let busy =
            self.current.as_ref().is_some_and(|a| a.audio.is_some()) || self.outgoing.is_some();
        if let Some(rx) = &self.audio_pending {
            if busy {
                return; // swap engines between items only
            }
            match rx.try_recv() {
                Ok(engine) => {
                    self.audio_pending = None;
                    if engine.available() {
                        tracing::info!("audio output ready ({})", self.audio_device);
                        self.audio_backoff = Duration::from_secs(15);
                    } else {
                        let e = engine.error().unwrap_or_default();
                        tracing::warn!(
                            "audio unavailable: {e}; lights run on the system clock (retrying in {} s)",
                            self.audio_backoff.as_secs()
                        );
                        self.audio_retry_at = now + self.audio_backoff;
                        self.audio_backoff = (self.audio_backoff * 2).min(Duration::from_secs(600));
                    }
                    self.audio = engine;
                    self.applied_volume = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.audio_pending = None,
            }
            return;
        }
        let want = self.opts.audio && !self.is_follower();
        let device_changed = wanted != self.audio_device;
        if want
            && !busy
            && (device_changed || (!self.audio.available() && now >= self.audio_retry_at))
        {
            if device_changed {
                self.audio_backoff = Duration::from_secs(15);
            }
            self.audio_device = wanted.clone();
            // Close the old device first (ALSA devices are exclusive).
            self.audio = AudioEngine::disabled("the audio device is starting");
            let (tx, rx) = std::sync::mpsc::channel();
            let spawned = std::thread::Builder::new()
                .name("pp-audio-open".into())
                .spawn(move || {
                    let _ = tx.send(AudioEngine::open(&wanted));
                });
            match spawned {
                Ok(_) => self.audio_pending = Some(rx),
                Err(e) => {
                    tracing::warn!("could not start the audio thread: {e}");
                    self.audio_retry_at = now + self.audio_backoff;
                }
            }
        }
    }

    /// The heard position of `a` now (audio clock, or monotonic without
    /// audio); also advances the lights timeline (`a.servo`) that follows it.
    fn position(audio: &AudioEngine, a: &mut Active, paused: bool, now_ms: f64, epoch: u64) -> f64 {
        if !a.started {
            return 0.0;
        }
        if a.use_audio_clock && !paused {
            let id = a.audio.unwrap_or(0);
            match audio.position_ms(id) {
                Some(p) if !audio.is_finished(id) => a.clock.seek(p, now_ms),
                _ => {
                    // Audio ended, failed or stalled: continue on the monotonic clock.
                    a.use_audio_clock = false;
                }
            }
        }
        let p = a.clock.pos(now_ms).max(a.last_pos);
        a.last_pos = p;
        if let Some(s) = a.servo.as_mut() {
            if paused {
                s.set_running(false, now_ms);
                s.update(p, Some(0.0), Some(epoch), now_ms);
            } else {
                if !s.running() {
                    s.set_running(true, now_ms);
                }
                // The monotonic clock is exact (rate 1); the audio clock's rate
                // is learnt and its jitter filtered.
                let ff = (!a.use_audio_clock).then_some(1.0);
                s.update(p, ff, Some(epoch), now_ms);
            }
        }
        p
    }

    /// Timeline position of `a` for the lights at clock time `t_ms`: the
    /// followed timeline minus the sound delay.
    fn lights_pos(a: &Active, t_ms: f64, delay_ms: f64) -> f64 {
        a.servo.as_ref().map_or(a.last_pos, |s| s.pos_at(t_ms)) - delay_ms
    }

    fn try_start(&mut self, now_ms: f64) -> Result<(), String> {
        let paused = self.paused;
        let Some(a) = self.current.as_mut() else {
            return Ok(());
        };
        if a.started {
            return Ok(());
        }
        if let ActiveKind::Sequence { reader, meta } = &mut a.kind {
            if let Some(e) = reader.error() {
                return Err(e);
            }
            if meta.is_none() {
                *meta = reader.meta();
                if let Some(m) = meta.as_ref() {
                    a.buf = vec![0; m.frame_len];
                    a.duration_ms = Some(m.duration_ms());
                    if m.frame_count == 0 {
                        return Err("the sequence has no frames".into());
                    }
                    if m.layout != FrameLayout::Channels {
                        return Err("this controller needs the full sequence (.fseq)".into());
                    }
                }
            }
            if meta.is_none() || !reader.ready(0) {
                if now_ms - a.begun_ms > OPEN_TIMEOUT_MS {
                    return Err("the sequence file could not be read in time".into());
                }
                return Ok(()); // still opening
            }
            if reader.get(0, &mut a.buf) {
                a.have_frame = true;
            }
        }
        a.started = true;
        let start = a
            .start_at_ms
            .clamp(0.0, a.duration_ms.map_or(0.0, |d| d as f64 - 1.0).max(0.0));
        a.clock = MonoClock::new(start, now_ms);
        a.last_pos = start;
        if paused {
            a.clock.set_paused(true, now_ms);
        }
        if let Some((path, gain)) = a.audio_src.clone() {
            a.audio = self.audio.play(&path, start as u64, gain, a.fade_in_ms);
            a.use_audio_clock = a.audio.is_some();
            if let (Some(id), true) = (a.audio, paused) {
                self.audio.set_paused(id, true);
            }
        }
        let gains = if a.use_audio_clock {
            ServoGains::AUDIO
        } else {
            ServoGains::FOLLOWER
        };
        a.servo = Some(Servo::new(start, now_ms, gains));
        Ok(())
    }

    fn leader_advance(&mut self, now_ms: f64) {
        if let Some(f) = self.stop_fade {
            if f.done(now_ms) {
                self.clear_playback();
                self.run_scheduler(now_ms);
            }
            return;
        }
        if self.program.is_some() && self.current.is_none() {
            self.playback_finished();
            return;
        }
        // Start (opening files) / skip broken items.
        if let Err(e) = self.try_start(now_ms) {
            let name = self
                .current
                .as_ref()
                .map(|a| a.iref.name.clone())
                .unwrap_or_default();
            let msg = format!("Skipped “{name}”: {e}");
            self.item_error = Some((msg.clone(), Instant::now()));
            self.journal(JournalEvent::Error {
                code: "itemSkipped".into(),
                msg: msg.clone(),
            });
            self.warn(msg, true);
            self.current = None;
            let next = self.take_next();
            self.start_next(next, now_ms, 0);
            return;
        }
        let paused = self.paused;
        let crossfade_ms = self.program.as_ref().map_or(0, |p| p.crossfade_ms) as u64;
        // Outgoing crossfade item: drop when done.
        if let Some(o) = &self.outgoing {
            if crossfade_progress(o.start_ms, o.len_ms, now_ms) >= 1.0 && !paused {
                let o = self.outgoing.take().expect("checked");
                if let Some(id) = o.active.audio {
                    self.audio.stop(id);
                }
            }
        }
        let Some(a) = self.current.as_mut() else {
            return;
        };
        if !a.started || paused {
            return;
        }
        let pos = Self::position(&self.audio, a, paused, now_ms, self.epoch);
        // Audio problems: warn once; audio-only items end with their track.
        if let Some(id) = a.audio {
            if let Some(e) = self.audio.track_error(id) {
                if !a.warned_audio {
                    a.warned_audio = true;
                    let msg = format!("Couldn't play the audio for “{}”: {e}", a.iref.name);
                    self.item_error = Some((msg.clone(), Instant::now()));
                    let _ = self.events.send(CoreEvent::Log {
                        level: "warning",
                        message: msg,
                        toast: true,
                    });
                }
            }
        }
        // Countdown DJ clip (F4): starts so that it ends at zero.
        if a.delayed_audio.as_ref().is_some_and(|d| pos >= d.at_pos) {
            if let Some(d) = a.delayed_audio.take() {
                let into = d.skip_ms + (pos - d.at_pos).max(0.0) as u64;
                a.audio = self.audio.play(&d.path, into, d.gain, 0);
                if let (Some(id), true) = (a.audio, paused) {
                    self.audio.set_paused(id, true);
                }
            }
        }
        let ended = match &a.kind {
            ActiveKind::Sequence { .. }
            | ActiveKind::Effect(_)
            | ActiveKind::Pause
            | ActiveKind::Flash(_) => a.duration_ms.is_some_and(|d| pos >= d as f64),
            ActiveKind::Audio => match a.audio {
                Some(id) => {
                    self.audio.is_finished(id)
                        || a.duration_ms.is_some_and(|d| pos >= d as f64 + 5000.0)
                }
                // No audio device: keep the show's timing using the file's duration.
                None => a.duration_ms.map_or(true, |d| pos >= d as f64),
            },
        };
        let remaining = a.duration_ms.map(|d| d as f64 - pos);
        let duration = a.duration_ms;
        let countdown = a.iref.kind == "countdown";
        if ended {
            if let (Some(id), false) = (a.audio, a.detach_audio) {
                self.audio.fade_out(id, 30);
            }
            if let Some(a) = self.current.take() {
                self.journal_end(&a, "finished");
            }
            let next = self.take_next();
            self.start_next(next, now_ms, 0);
            return;
        }
        // Preload the next sequence a few seconds ahead (gapless starts).
        if remaining.is_some_and(|r| r < 4000.0) && self.preload.is_none() {
            let next_seq = match self.peek_next() {
                Some(Pending::Request { sequence_id, .. }) => Some(sequence_id),
                Some(Pending::Item(PlaylistItem::Sequence { sequence_id, .. })) => {
                    Some(sequence_id)
                }
                _ => None,
            };
            if let Some(id) = next_seq {
                if let Some(seq) = self.show.sequence(&id) {
                    let path = self.app.config.data_dir.join(&seq.file);
                    if path.exists() {
                        self.preload = Some((id, FrameReader::open(path, 0)));
                    }
                }
            }
        }
        // Crossfade into the next item (never out of a countdown: the song
        // must start exactly at zero).
        if crossfade_ms > 0 && self.outgoing.is_none() && !countdown {
            if let Some(d) = duration {
                let start_at = crossfade_start(d, crossfade_ms) as f64;
                let blendable = matches!(
                    self.peek_next(),
                    Some(Pending::Request { .. })
                        | Some(Pending::Item(
                            PlaylistItem::Sequence { .. }
                                | PlaylistItem::Effect { .. }
                                | PlaylistItem::Dj { .. }
                                | PlaylistItem::Media { .. }
                        ))
                );
                if pos >= start_at && blendable {
                    let len = (d as f64 - start_at).max(1.0);
                    let old = self.current.take().expect("current exists");
                    if let Some(id) = old.audio {
                        self.audio.fade_out(id, len as u32);
                    }
                    self.outgoing = Some(Outgoing {
                        active: old,
                        start_ms: now_ms,
                        len_ms: len,
                    });
                    let next = self.take_next();
                    self.start_next(next, now_ms, len as u32);
                }
            }
        }
    }

    /// Render an item into channel space at lights position `pos` (a
    /// presentation slot of `slot_ms`). Returns the sequence frame shown.
    fn render_active(
        a: &mut Active,
        pos: f64,
        slot_ms: f64,
        chan: &mut [u8],
        idle: Option<&mut Look>,
        now_ms: f64,
    ) -> Option<u32> {
        match &mut a.kind {
            ActiveKind::Sequence { reader, meta } => {
                let mut shown = None;
                if let Some(m) = meta {
                    let idx = frame_for_slot(pos, slot_ms, m.frame_ms as f64);
                    if a.buf.len() == m.frame_len && reader.get(idx, &mut a.buf) {
                        a.have_frame = true;
                    }
                    shown = Some(idx);
                }
                if a.have_frame {
                    let n = chan.len().min(a.buf.len());
                    chan[..n].copy_from_slice(&a.buf[..n]);
                }
                shown.filter(|_| a.have_frame)
            }
            ActiveKind::Effect(layer) => {
                layer.render(pos.max(0.0) as u64, &mut Sink::Chan(chan));
                None
            }
            ActiveKind::Flash(sched) => {
                let lit = match sched {
                    Some(s) => {
                        let len = calpattern::flash_len_ms(slot_ms);
                        if (s.flash_ms - len).abs() > 0.01 {
                            **s = calpattern::schedule(s.seed, slot_ms);
                        }
                        s.flash_on(pos)
                    }
                    None => flash_on(pos, slot_ms),
                };
                if lit {
                    chan.fill(255);
                }
                None
            }
            ActiveKind::Audio | ActiveKind::Pause => {
                if let Some(l) = idle {
                    l.layer.render(
                        (now_ms - l.started_ms).max(0.0) as u64,
                        &mut Sink::Chan(chan),
                    );
                }
                None
            }
        }
    }

    fn compose_leader(&mut self, now_ms: f64) {
        let paused = self.paused;
        let (light, slot, delay, epoch) = (
            self.light_ms,
            self.slot_ms,
            self.output_delay_ms,
            self.epoch,
        );
        let needs_idle = self
            .current
            .iter()
            .chain(self.outgoing.iter().map(|o| &o.active))
            .any(|a| matches!(a.kind, ActiveKind::Audio | ActiveKind::Pause));
        if needs_idle {
            self.ensure_idle_layer();
            self.sync_idle_beat(now_ms);
        }
        self.chan.fill(0);
        let mut blend_t = None;
        if let Some(o) = self.outgoing.as_mut() {
            self.chan_b.fill(0);
            Self::position(&self.audio, &mut o.active, paused, now_ms, epoch);
            let pos = Self::lights_pos(&o.active, light, delay);
            Self::render_active(
                &mut o.active,
                pos,
                slot,
                &mut self.chan_b,
                self.idle_layer.as_mut(),
                now_ms,
            );
            blend_t = Some(crossfade_progress(o.start_ms, o.len_ms, now_ms));
        }
        self.shown = None;
        self.shown_pos = None;
        if let Some(a) = self.current.as_mut() {
            let pos = if a.started {
                Self::position(&self.audio, a, paused, now_ms, epoch);
                Self::lights_pos(a, light, delay)
            } else {
                0.0
            };
            if a.started || blend_t.is_none() {
                let shown = Self::render_active(
                    a,
                    pos,
                    slot,
                    &mut self.chan,
                    self.idle_layer.as_mut(),
                    now_ms,
                );
                if let Some(idx) = shown {
                    self.shown = Some((a.iref.id.clone(), idx));
                }
                if a.started {
                    self.shown_pos = Some(pos);
                }
            }
        } else if self.program.is_none() {
            if let Some(l) = self.look.as_mut() {
                l.layer.render(
                    (light - l.started_ms).max(0.0) as u64,
                    &mut Sink::Chan(&mut self.chan),
                );
            }
        }
        if let Some(t) = blend_t {
            let started = self.current.as_ref().is_some_and(|a| a.started);
            let t = if started { t } else { 0.0 };
            // chan = outgoing·(1-t) + incoming·t
            let (a, b) = (&self.chan_b, &mut self.chan);
            let incoming = b.clone();
            clock::blend(a, &incoming, t, b);
        }
        // Surprise layer over the show (F20), then the season's prop mask (F8).
        if let Some(sp) = self.surprise.as_mut() {
            if sp.done(now_ms) {
                self.surprise = None;
            } else {
                if let Some(ms) = sp.sequence_ms() {
                    sp.anchor.duration_ms = sp.anchor.duration_ms.min(ms.max(surprise::MIN_MS));
                }
                sp.render_chan(light, slot, &mut self.chan);
            }
        }
        for s in &self.disabled_slots {
            Sink::Chan(&mut self.chan).put(s, &self.zeros[..s.len.min(self.zeros.len())]);
        }
        if let Some(t) = self.test.as_mut() {
            t.render_props(now_ms, &mut Sink::Chan(&mut self.chan));
        }
        {
            let slots = &self.slots;
            let chan = &mut self.chan;
            self.overlays.for_each_active(|id, px| {
                if let Some(s) = slots.get(id) {
                    Sink::Chan(chan).put(s, px);
                }
            });
        }
        if !self.remote_props.is_empty() {
            if let Some(cluster) = self.app.services.cluster.get() {
                let remote = &self.remote_props;
                self.overlays.take_forwards(Instant::now(), |id, px| {
                    if remote.contains(id) {
                        cluster.forward_overlay(id, px);
                    }
                });
            }
        }
        self.node_map.render(&self.chan, &mut self.frame);
        if let Some(t) = self.test.as_mut() {
            if t.is_timeline() {
                t.render_timeline(light - t.started_ms, slot, &mut self.frame);
            } else {
                t.render_raw(now_ms, &mut self.frame);
            }
        }
    }

    fn compose_follower(&mut self, now_ms: f64) {
        self.frame.clear();
        let (light, slot) = (self.light_ms, self.slot_ms);
        let f = &mut self.follow;
        if f.meta.is_none() {
            // The slice finished opening since the last sync packet.
            if let Some(m) = f.reader.as_ref().and_then(|r| r.meta()) {
                f.buf = vec![0; m.frame_len];
                f.meta = Some(m);
            }
        }
        // Follow the leader's timeline continuously (every frame, not only
        // when a packet arrives: the anchor is a function of time).
        if let (Some(c), Some(a)) = (f.clock.as_mut(), f.anchor) {
            if c.running() && f.hold_since.is_none() {
                c.update(a.pos_at(now_ms), Some(a.rate), Some(a.epoch), now_ms);
            } else {
                c.advance(now_ms);
            }
        }
        // …evaluated when this frame lights up.
        let pos = f.clock.as_ref().map_or(0.0, |c| c.pos_at(light));
        self.shown = None;
        self.shown_pos = f.clock.as_ref().map(|_| pos);
        if let (Some(reader), Some(meta)) = (f.reader.as_ref(), f.meta.as_ref()) {
            let idx = frame_for_slot(pos, slot, meta.frame_ms as f64);
            if f.buf.len() == meta.frame_len && reader.get(idx, &mut f.buf) {
                f.have_frame = true;
            }
            if f.have_frame {
                let id = f
                    .pkt
                    .as_ref()
                    .and_then(|p| p.item.as_ref())
                    .map(|i| i.id.clone());
                self.shown = id.map(|id| (id, idx));
                self.shown_pos = Some(pos);
            }
            if f.have_frame {
                match &meta.layout {
                    FrameLayout::Outputs(ppo) => {
                        let mut off = 0usize;
                        for (i, &px) in ppo.iter().enumerate() {
                            let len = px as usize * 3;
                            let src = f.buf.get(off..off + len).unwrap_or(&[]);
                            let dst = self.frame.output_mut(i);
                            let n = dst.len().min(src.len());
                            dst[..n].copy_from_slice(&src[..n]);
                            off += len;
                        }
                    }
                    FrameLayout::Channels => self.node_map.render(&f.buf, &mut self.frame),
                }
            }
        } else if let Some(e) = f.effect.as_mut() {
            e.render(
                pos.max(0.0) as u64,
                &mut Sink::Frame(&mut self.frame, &self.prop_map),
            );
        } else if f.flash {
            let lit = match f.cal.as_mut() {
                Some(c) => {
                    let len = calpattern::flash_len_ms(slot);
                    if (c.flash_ms - len).abs() > 0.01 {
                        *c = calpattern::schedule(c.seed, slot);
                    }
                    c.flash_on(pos)
                }
                None => flash_on(pos, slot),
            };
            if lit {
                for i in 0..self.frame.output_count() {
                    self.frame.output_mut(i).fill(255);
                }
            }
        }
        // Surprise layer (F20), then the season's prop mask (F8).
        if let Some(sp) = self.surprise.as_mut() {
            if sp.done(now_ms) {
                self.surprise = None;
            } else {
                sp.render_frame(light, slot, &mut self.frame, &self.prop_map);
            }
        }
        for s in &self.disabled_slots {
            self.prop_map.apply_overlay(
                &s.id,
                &self.zeros[..s.len.min(self.zeros.len())],
                &mut self.frame,
            );
        }
        let f = &mut self.follow;
        if let Some(t) = f.test.as_mut() {
            t.render_props(now_ms, &mut Sink::Frame(&mut self.frame, &self.prop_map));
        }
        if let Some(t) = self.test.as_mut() {
            t.render_props(now_ms, &mut Sink::Frame(&mut self.frame, &self.prop_map));
        }
        {
            let frame = &mut self.frame;
            let map = &self.prop_map;
            let slots = &self.slots;
            self.overlays.for_each_active(|id, px| {
                if let Some(s) = slots.get(id) {
                    Sink::Frame(frame, map).put(s, px);
                }
            });
        }
        if let Some(t) = self.follow.test.as_mut() {
            if t.is_timeline() {
                // The leader's test timeline (anchored like a song).
                let at = self
                    .follow
                    .clock
                    .as_ref()
                    .filter(|_| self.follow.anchor.is_some())
                    .map_or(light - t.started_ms, |c| c.pos_at(light));
                t.render_timeline(at, slot, &mut self.frame);
            } else {
                t.render_raw(now_ms, &mut self.frame);
            }
        }
        if let Some(t) = self.test.as_mut() {
            if t.is_timeline() {
                t.render_timeline(light - t.started_ms, slot, &mut self.frame);
            } else {
                t.render_raw(now_ms, &mut self.frame);
            }
        }
    }

    fn light_level(&self, now_ms: f64) -> f32 {
        let mut level = 1.0;
        if let Some(f) = &self.stop_fade {
            level *= f.level(now_ms);
        }
        if let Some(f) = &self.follow.lost_fade {
            level *= f.level(now_ms);
        }
        if self.blackout {
            level = 0.0;
        }
        level
    }

    /// The master brightness the lights use: the leader caps its own by
    /// late-night dimming (F12); followers get the capped value in sync.
    fn light_brightness(&self) -> u8 {
        match (self.is_follower(), self.facts.brightness_cap) {
            (false, Some(cap)) => self.brightness.min(cap),
            _ => self.brightness,
        }
    }

    fn write_output(&mut self, now: Instant, now_ms: f64) {
        let level = self.light_level(now_ms);
        let master = (self.light_brightness() as f32 * level).round() as u8;
        self.pipeline.set_master_brightness(master);
        if level <= 0.0 {
            self.frame.clear();
        }
        let ppo = self.node_map.pixels_per_output();
        let n = if self.output.kind == BackendKind::Dpi && self.board.output_count() > 0 {
            ppo.len().min(self.board.output_count())
        } else {
            ppo.len()
        };
        let bytes: usize = ppo[..n].iter().map(|&p| p as usize * 3).sum();
        let data = &self.frame.as_bytes()[..bytes.min(self.frame.len())];
        match OutputFrameRef::from_contiguous(data, &ppo[..n]) {
            Ok(input) => {
                self.pipeline.process(&input, &mut self.wire);
                // Power limiter (F12) on what the pixels actually draw.
                for ep in self.limiter.process(&mut self.wire, now_ms) {
                    tracing::info!(
                        "power limiter: {} limited for {:.0} s",
                        ep.group_id,
                        ep.seconds
                    );
                    self.app.services.journal.record(ep.event(&self.node_id));
                }
                let wire = self.wire.as_frame_ref();
                self.output.write(&wire, now);
                if let Some(tap) = &self.tap {
                    let shown = self.shown.as_ref().map(|(id, f)| (id.as_str(), *f));
                    let meta = super::debugtap::TapMeta {
                        frame_no: self.frames_out + 1,
                        at_ms: now_ms,
                        sequence: shown,
                        pos_ms: self.shown_pos,
                        light_at_ms: self.light_ms,
                        engine_now_ms: self.t0.elapsed().as_secs_f64() * 1000.0,
                        master,
                    };
                    tap.record(meta, &ppo[..n], data, &mut wire.iter());
                }
            }
            Err(e) => tracing::debug!("frame layout mismatch: {e}"),
        }
        self.frames_out += 1;
        let (since, count) = self.fps_window;
        let el = now.duration_since(since);
        if el >= Duration::from_secs(1) {
            self.fps = ((self.frames_out - count) as f64 / el.as_secs_f64()) as f32;
            self.fps_window = (now, self.frames_out);
            // Re-check the geometry once the DPI backend knows its mode.
            if self.output.kind == BackendKind::Dpi {
                self.update_geometry();
            }
        }
    }

    fn preview(&mut self, now: Instant, now_ms: f64) {
        use std::sync::atomic::Ordering;
        if crate::api::ws::PREVIEW_SUBSCRIBERS.load(Ordering::Relaxed) == 0 {
            return;
        }
        if now.duration_since(self.last_preview) < PREVIEW_EVERY {
            return;
        }
        self.last_preview = now;
        self.preview_no = self.preview_no.wrapping_add(1);
        let level = self.light_level(now_ms) * self.light_brightness() as f32 / 100.0;
        let bytes = if self.is_follower() {
            compose::preview_frame(
                &self.show,
                self.preview_no,
                level,
                None,
                Some((&self.frame, &self.prop_map)),
            )
        } else {
            compose::preview_frame(&self.show, self.preview_no, level, Some(&self.chan), None)
        };
        self.app.events.preview(bytes::Bytes::from(bytes));
    }

    fn frame_period(&self) -> Duration {
        let clamp = |ms: u32| Duration::from_millis(ms.clamp(10, 100) as u64);
        let seq_ms = if self.is_follower() {
            self.follow
                .meta
                .as_ref()
                .filter(|_| self.follow.reader.is_some())
                .map(|m| m.frame_ms)
        } else {
            match self.current.as_ref().map(|a| &a.kind) {
                Some(ActiveKind::Sequence { meta: Some(m), .. }) => Some(m.frame_ms),
                _ => None,
            }
        };
        if let Some(ms) = seq_ms {
            // Crossfades, live overlays (games) and surprises run at least at
            // the effect rate.
            let fast =
                self.outgoing.is_some() || self.overlays.any_active() || self.surprise.is_some();
            return clamp(ms).min(if fast {
                self.effect_period
            } else {
                Duration::MAX
            });
        }
        let busy = self.program.is_some()
            || self.look.is_some()
            || self.test.is_some()
            || self.surprise.is_some()
            || self.follow.effect.is_some()
            || self.follow.test.is_some()
            || self.follow.lost_fade.is_some()
            || self.overlays.any_active();
        if busy {
            self.effect_period
        } else {
            IDLE_PERIOD
        }
    }

    // ----- presentation timing -----------------------------------------

    /// Presentation timing of the running output (DPI), if it has any.
    fn present_timing(&self) -> Option<pixelplus_output::PresentTiming> {
        if !self.output.running {
            return None;
        }
        self.output.backend.present_timing()
    }

    fn engine_ms(&self, t: Instant) -> f64 {
        match t.checked_duration_since(self.t0) {
            Some(d) => d.as_secs_f64() * 1000.0,
            None => -(self.t0.duration_since(t).as_secs_f64() * 1000.0),
        }
    }

    fn engine_instant(&self, ms: f64) -> Instant {
        if ms >= 0.0 {
            self.t0 + Duration::from_secs_f64(ms / 1000.0)
        } else {
            self.t0
        }
    }

    /// Latch delay (ms) of this node's strings after scan-out starts: all
    /// strings latch after the longest when bottom-aligned; otherwise the
    /// mean string length is the best single compromise.
    fn latch_ms(&self, t: &pixelplus_output::PresentTiming) -> f64 {
        let (longest, mean) = self.string_lines;
        let lines = if t.bottom_aligned { longest } else { mean };
        t.latch_after(lines).as_secs_f64() * 1000.0
    }

    /// Time a frame needs from the start of a tick until its flip is queued.
    fn ready_margin(&self) -> Duration {
        Duration::from_secs_f64((self.tick_ms * 1.5 + 1.0).clamp(2.0, 25.0) / 1000.0)
    }

    fn note_tick(&mut self, took: Duration) {
        let ms = took.as_secs_f64() * 1000.0;
        // Rise fast, decay slowly: a slow frame must not miss its vblank twice.
        self.tick_ms = if ms > self.tick_ms {
            self.tick_ms * 0.5 + ms * 0.5
        } else {
            self.tick_ms * 0.98 + ms * 0.02
        };
    }

    /// When the frame being composed now lights up (sets `light_ms`/`slot_ms`).
    fn begin_frame(&mut self, now: Instant, now_ms: f64) {
        match self.present_timing() {
            Some(t) => {
                let ready = now + self.ready_margin() / 2;
                let (mut v, latch) = self
                    .planned
                    .take()
                    .unwrap_or_else(|| (t.vblank_at_or_after(ready), self.latch_ms(&t)));
                if v < ready {
                    // Too late for the planned vblank: it goes out a refresh later.
                    v = t.vblank_at_or_after(ready);
                }
                if let Some(p) = t.pending_vblank {
                    if v <= p {
                        v = t.vblank_at_or_after(p + t.period / 2);
                    }
                }
                self.light_ms = self.engine_ms(v) + latch;
                self.slot_ms = t.period.as_secs_f64() * 1000.0;
            }
            None => {
                self.planned = None;
                self.light_ms = now_ms;
                self.slot_ms = SOFT_SLOT_MS;
            }
        }
    }

    /// The timeline that decides frame changes right now: (lights position
    /// now, rate, frame ms, frame shown).
    fn frame_timeline(&self, now_ms: f64) -> Option<(f64, f64, f64, Option<u32>)> {
        let shown = self.shown.as_ref().map(|(_, i)| *i);
        if self.is_follower() {
            let f = &self.follow;
            let m = f.meta.as_ref().filter(|_| f.reader.is_some())?;
            let c = f.clock.as_ref()?;
            return Some((c.pos_at(now_ms), c.rate(), m.frame_ms as f64, shown));
        }
        let a = self.current.as_ref().filter(|a| a.started)?;
        let ActiveKind::Sequence { meta: Some(m), .. } = &a.kind else {
            return None;
        };
        let sv = a.servo.as_ref()?;
        let rate = if self.paused { 0.0 } else { sv.rate() };
        Some((
            sv.pos_at(now_ms) - self.output_delay_ms,
            rate,
            m.frame_ms as f64,
            shown,
        ))
    }

    /// Plan the next frame: returns when to start composing it.
    ///
    /// While a sequence plays, the next update is timed for the next frame
    /// boundary of the timeline ([`next_update`]); otherwise it follows the
    /// frame period. On DPI the update is moved to the vblank at which it
    /// will show (after any flip still pending) and composing starts early
    /// enough for the flip to make it; the frame is then chosen for that
    /// vblank plus the latch delay.
    fn plan_next(&mut self) -> Instant {
        let now = Instant::now();
        let now_ms = self.engine_ms(now);
        let period = self.frame_period();
        let period_ms = period.as_secs_f64() * 1000.0;
        let timing = self.present_timing();
        let slot = timing.map_or(SOFT_SLOT_MS, |t| t.period.as_secs_f64() * 1000.0);
        let mut desired = self.light_ms.max(now_ms - period_ms) + period_ms;
        if let Some((pos, rate, frame_ms, shown)) = self.frame_timeline(now_ms) {
            // Faster updates (crossfades, games) keep the frame period.
            if period_ms >= frame_ms * 0.9 {
                let shown = shown.unwrap_or_else(|| frame_for_slot(pos, slot, frame_ms));
                if let Some(t) = next_update(pos, rate, now_ms, shown, frame_ms, slot) {
                    desired = t.min(now_ms + frame_ms * 4.0 / rate.max(0.25));
                }
            }
        }
        match timing {
            Some(t) => {
                let latch = self.latch_ms(&t);
                let margin = self.ready_margin();
                let mut earliest = now + margin;
                if let Some(p) = t.pending_vblank {
                    earliest = earliest.max(p + t.period / 2);
                }
                let want = self.engine_instant(desired - latch).max(earliest);
                let v = t.vblank_at_or_after(want);
                self.planned = Some((v, latch));
                v.checked_sub(margin).unwrap_or(now).max(now)
            }
            None => {
                self.planned = None;
                let at = self.engine_instant(desired);
                at.max(now + Duration::from_micros(500))
            }
        }
    }

    // ----- follower ------------------------------------------------------

    fn on_sync(&mut self, p: SyncPacket, now_ms: f64) {
        let show = self.show.clone();
        self.brightness = p.brightness.min(100);
        self.blackout = p.blackout;
        let f = &mut self.follow;
        f.rx_ms = now_ms;
        f.lost_fade = None;
        f.hold_since = None;
        f.lost_logged = false;
        let playing = p.state == PlayerState::Playing || p.state == PlayerState::Effect;
        // The leader's timeline on our clock (see cluster::follower::localize_sync);
        // packets without one (released, old leader) give a position at `sent_at`.
        let anchor = p.anchor.unwrap_or_else(|| {
            let age = (now_ms - p.sent_at_ms as f64).clamp(0.0, 2000.0);
            Anchor {
                pos_ms: p.pos_ms as f64 + if playing { age } else { 0.0 },
                at_ms: now_ms,
                rate: if playing { 1.0 } else { 0.0 },
                epoch: 0,
            }
        });
        let target = anchor.pos_at(now_ms);

        // Tests carried in the packet.
        let mut new_leader_test = false;
        match (&p.test, p.state) {
            (Some(t), PlayerState::Testing) => {
                if f.test.as_ref().map(|x| &x.req) != Some(t) {
                    let plan = t.map.clone().or_else(|| {
                        let id = t.map_run_id.as_ref()?;
                        self.map_plans
                            .iter()
                            .find(|(k, _)| k == id)
                            .map(|(_, p)| p.clone())
                    });
                    f.test =
                        TestLayer::with_plan(&show, &self.node_id, t, now_ms, plan.as_ref()).ok();
                    new_leader_test = f.test.is_some();
                }
            }
            _ => f.test = None,
        }

        let seq_item = p
            .item
            .as_ref()
            .filter(|_| matches!(p.state, PlayerState::Playing | PlayerState::Paused))
            .filter(|i| i.kind == "sequence" || i.kind == "request")
            .cloned();
        let flash = p
            .item
            .as_ref()
            .filter(|_| matches!(p.state, PlayerState::Playing | PlayerState::Paused))
            .is_some_and(|i| i.kind == CALIBRATION_ID);
        let key = match (&seq_item, flash) {
            (Some(i), _) => Some(format!("seq:{}", i.id)),
            (None, true) => Some(CALIBRATION_ID.to_string()),
            (None, false) => None,
        };
        // Same song, but its slice wasn't here when it started (adopted mid-show,
        // or a re-uploaded sequence still downloading): look again, so the lights
        // come on as soon as the download lands instead of at the next song.
        let retry = key == f.item_key && seq_item.is_some() && f.reader.is_none();
        if key != f.item_key || retry {
            if key != f.item_key {
                f.item_key = key;
                f.clock = None;
            }
            f.reader = None;
            f.meta = None;
            f.have_frame = false;
            f.missing = None;
            if let Some(item) = &seq_item {
                match show.sequence(&item.id) {
                    Some(seq) => {
                        let path = self.app.config.data_dir.join(&seq.file);
                        if path.exists() {
                            f.reader = Some(FrameReader::open(path, 0));
                        } else {
                            f.missing = Some(format!("“{}” is not downloaded yet", item.name));
                        }
                    }
                    None => {
                        f.missing = Some(format!("“{}” is not on this controller yet", item.name))
                    }
                }
            }
        }
        if f.meta.is_none() {
            if let Some(r) = &f.reader {
                f.meta = r.meta();
                if let Some(m) = &f.meta {
                    f.buf = vec![0; m.frame_len];
                }
            }
        }
        // Effect (looks, effect items, the look under DJ clips).
        let effect = if seq_item.is_none() {
            p.effect.clone()
        } else {
            None
        };
        match effect {
            Some(e) => {
                if f.effect.as_ref().map(|x| &x.preset) != Some(&e) {
                    f.effect = Some(EffectLayer::new(&show, &e));
                }
            }
            None => f.effect = None,
        }
        f.flash = flash;
        // Phone calibration (F1): the item id carries the pattern's seed.
        f.cal = p
            .item
            .as_ref()
            .filter(|_| flash)
            .and_then(|i| i.id.strip_prefix(CALIBRATION_V2_PREFIX))
            .and_then(|seed| seed.parse::<u32>().ok())
            .map(|seed| match f.cal.take() {
                Some(c) if c.seed == seed => c,
                _ => calpattern::schedule(seed, 0.0),
            });
        // Clock: follow the anchor (jump on a new epoch, slew otherwise).
        let timeline_test = f.test.as_ref().is_some_and(|t| t.is_timeline());
        let active = seq_item.is_some() || f.effect.is_some() || flash || timeline_test;
        if !active {
            f.clock = None;
            f.anchor = None;
        } else {
            let c = f
                .clock
                .get_or_insert_with(|| Servo::new(target, now_ms, ServoGains::FOLLOWER));
            if p.state == PlayerState::Paused {
                c.set_running(false, now_ms);
                c.update(target, Some(0.0), Some(anchor.epoch), now_ms);
            } else {
                c.set_running(true, now_ms);
                c.update(target, Some(anchor.rate), Some(anchor.epoch), now_ms);
            }
            f.anchor = Some(anchor);
        }
        let released = p.leader.is_empty() && p.state == PlayerState::Idle;
        let surprise = p.surprise.clone();
        let base_ms = f.anchor.map_or(now_ms, |a| a.at_ms);
        let packet_anchor_ms = p.anchor.map_or(now_ms, |a| a.at_ms);
        f.pkt = Some(p);
        // Surprise layer (F20): placed on our clock like the timeline.
        let _ = base_ms;
        match surprise {
            Some(sa) => {
                let start = packet_anchor_ms + sa.start_pos;
                let same = self
                    .surprise
                    .as_ref()
                    .is_some_and(|l| l.anchor.id == sa.id && l.anchor.epoch == sa.epoch);
                if same {
                    if let Some(l) = self.surprise.as_mut() {
                        l.start_ms = start;
                        l.anchor.duration_ms = sa.duration_ms;
                    }
                } else {
                    match SurpriseLayer::new(&show, &self.app.config.data_dir, sa, start) {
                        Ok(l) => self.surprise = Some(l),
                        Err(e) => {
                            tracing::debug!("surprise not shown here: {e}");
                            self.surprise = None;
                        }
                    }
                }
            }
            None => self.surprise = None,
        }
        if new_leader_test {
            // A test started on the leader (fault finder, test pattern) replaces a
            // local one, e.g. the "identify" chase, which would otherwise cover it.
            self.test = None;
        }
        if released {
            // Released by the leader: stop everything it had us doing.
            self.follow = FollowState::default();
            self.test = None;
            self.surprise = None;
        }
    }

    /// Without sync packets the current item plays on to its end (effects and
    /// tests for up to a minute), then the last frame holds for 3 s and fades
    /// to dark. A new packet resumes normal following at once.
    fn follower_advance(&mut self, now_ms: f64) {
        let f = &mut self.follow;
        let Some(p) = &f.pkt else { return };
        if p.state == PlayerState::Idle {
            return;
        }
        let age = now_ms - f.rx_ms;
        if age <= 3000.0 {
            return;
        }
        if !f.lost_logged {
            f.lost_logged = true;
            tracing::warn!("no sync from the leader for 3 s; playing on to the end of the item");
        }
        let pos = f.clock.as_ref().map_or(0.0, |c| c.pos());
        let ended = match &f.meta {
            Some(m) if f.reader.is_some() => pos >= m.duration_ms() as f64,
            _ => age > 60_000.0,
        };
        if ended && f.hold_since.is_none() {
            f.hold_since = Some(now_ms);
            if let Some(c) = f.clock.as_mut() {
                c.set_running(false, now_ms);
            }
        }
        if f.hold_since.is_some_and(|h| now_ms - h > 3000.0) && f.lost_fade.is_none() {
            f.lost_fade = Some(Fade::new(now_ms, 1000.0, 1.0, 0.0));
        }
        if f.lost_fade.is_some_and(|fd| fd.done(now_ms)) {
            self.follow = FollowState::default();
            let _ = self.events.send(CoreEvent::Log {
                level: "warning",
                message: "Lost contact with the show leader; lights are dark until it returns."
                    .into(),
                toast: false,
            });
        }
    }

    // ----- status --------------------------------------------------------

    fn build_status(&self, now_ms: f64) -> PlayerStatus {
        let mut s = PlayerStatus {
            volume: self.volume,
            brightness: self.brightness,
            blackout: self.blackout,
            fps: (self.fps * 10.0).round() / 10.0,
            request_queue: self.requests.len() as u32,
            ..Default::default()
        };
        let mut errors: Vec<String> = Vec::new();
        if let Some(e) = &self.output.error {
            errors.push(e.clone());
        }
        let geo = geometry_status();
        if let Some(m) = geo.message {
            errors.push(m);
        }
        if self.is_follower() {
            let f = &self.follow;
            if let Some(p) = &f.pkt {
                s.state = if f.hold_since.is_some() && p.state != PlayerState::Idle {
                    PlayerState::Paused
                } else {
                    p.state
                };
                s.item = p.item.clone();
                s.pos_ms = f
                    .clock
                    .as_ref()
                    .map_or(p.pos_ms, |c| c.pos().max(0.0) as u64);
            }
            if let Some(m) = &f.meta {
                s.duration_ms = m.duration_ms();
            }
            if let Some(m) = &f.missing {
                errors.push(m.clone());
            }
            if f.pkt.as_ref().is_some_and(|p| p.state != PlayerState::Idle)
                && now_ms - f.rx_ms > 3000.0
            {
                errors.push("Lost contact with the show leader".into());
            }
            s.sync_error_ms = f
                .clock
                .as_ref()
                .filter(|c| c.running() && f.anchor.is_some())
                .map(|c| c.error_ms());
        } else {
            s.schedule_entry = self.facts.active.as_ref().map(|w| ScheduleRef {
                id: w.entry_id.clone(),
                name: w.name.clone(),
                ends_at: w.ends_at.clone(),
            });
            s.next_show = self.facts.next_show.clone();
            if let (Some(prog), Some(a)) = (&self.program, &self.current) {
                s.state = if self.paused {
                    PlayerState::Paused
                } else if a.is_manual_look() {
                    PlayerState::Effect
                } else {
                    PlayerState::Playing
                };
                s.item = Some(a.iref.clone());
                s.pos_ms = a.last_pos.max(0.0) as u64;
                s.duration_ms = a.duration_ms.unwrap_or(0);
                // The lights timeline for followers, stamped with our own clock.
                s.anchor = a.servo.as_ref().map(|sv| Anchor {
                    pos_ms: sv.pos_at(now_ms) - self.output_delay_ms,
                    at_ms: now_ms,
                    rate: if self.paused { 0.0 } else { sv.freq() },
                    epoch: (self.epoch << EPOCH_SHIFT) + sv.jumps(),
                });
                if let (Source::Playlist(c), Some((id, name))) = (&prog.source, &prog.playlist) {
                    let (index, count) = c.flat_index();
                    s.playlist = Some(PlaylistRef {
                        id: id.clone(),
                        name: name.clone(),
                        index,
                        count,
                    });
                }
                s.next_item = self.peek_next().map(|p| self.pending_ref(&p));
                let wants_audio = a.audio_src.is_some();
                if wants_audio && self.opts.audio && !self.audio.available() {
                    errors.push(format!(
                        "No sound: {}. The lights keep playing.",
                        self.audio
                            .error()
                            .unwrap_or_else(|| "the audio device is unavailable".into())
                    ));
                }
            } else if let Some(l) = &self.look {
                s.state = PlayerState::Effect;
                s.item = Some(item_ref("effect", &l.id, &l.name));
                s.pos_ms = (now_ms - l.started_ms).max(0.0) as u64;
                s.anchor = Some(Anchor {
                    pos_ms: now_ms - l.started_ms,
                    at_ms: now_ms,
                    rate: 1.0,
                    epoch: self.epoch << EPOCH_SHIFT,
                });
            }
            if let Some((e, at)) = &self.item_error {
                if at.elapsed() < Duration::from_secs(30) {
                    errors.push(e.clone());
                }
            }
        }
        match self.test.as_ref().or(self.follow.test.as_ref()) {
            // Live looks (Effects page) are tests in "effect" mode.
            Some(t) if t.is_look() => {
                s.state = PlayerState::Effect;
                if let Some(p) = &t.req.effect {
                    s.item = Some(item_ref("look", &p.id, &p.name));
                }
                s.pos_ms = (now_ms - t.started_ms).max(0.0) as u64;
                s.duration_ms = 0;
                s.anchor = Some(Anchor {
                    pos_ms: now_ms - t.started_ms,
                    at_ms: now_ms,
                    rate: 1.0,
                    epoch: self.epoch << EPOCH_SHIFT,
                });
            }
            Some(t) => {
                s.state = PlayerState::Testing;
                // Mapping codes, identify and calibration run on a timeline
                // every controller follows (like a song).
                if t.is_timeline() && !self.is_follower() {
                    s.anchor = Some(Anchor {
                        pos_ms: now_ms - t.started_ms,
                        at_ms: now_ms,
                        rate: 1.0,
                        epoch: ((self.epoch + self.test_epoch) << EPOCH_SHIFT) | 1,
                    });
                }
            }
            None => {}
        }
        if !self.is_follower() {
            s.light_brightness = Some(self.light_brightness());
            if let Some(sp) = &self.surprise {
                let anchor = s.anchor.get_or_insert(Anchor {
                    pos_ms: 0.0,
                    at_ms: now_ms,
                    rate: 0.0,
                    epoch: self.epoch << EPOCH_SHIFT,
                });
                let mut a = sp.anchor.clone();
                // Relative to the timeline anchor's clock time (see surprise.rs).
                a.start_pos = ((sp.start_ms - anchor.at_ms) * 1000.0).round() / 1000.0;
                s.surprise = Some(a);
            }
        }
        s.power = self.limiter.status();
        if !errors.is_empty() {
            s.error = Some(errors.join(" · "));
        }
        s.refresh_hz = self
            .present_timing()
            .map(|t| 1.0 / t.period.as_secs_f64().max(1e-6));
        s
    }

    fn publish_status(&mut self, now: Instant, now_ms: f64) {
        let s = self.build_status(now_ms);
        // Position, fps and the anchor's position/rate change every frame; a new
        // anchor epoch (seek, pause, new item) is published at once.
        let strip = |s: &PlayerStatus| PlayerStatus {
            pos_ms: 0,
            fps: 0.0,
            anchor: s.anchor.map(|a| Anchor {
                pos_ms: 0.0,
                at_ms: 0.0,
                rate: 0.0,
                epoch: a.epoch,
            }),
            sync_error_ms: None,
            refresh_hz: None,
            surprise: s.surprise.as_ref().map(|a| SurpriseAnchor {
                start_pos: 0.0,
                ..a.clone()
            }),
            ..s.clone()
        };
        let changed = strip(&s) != strip(&self.last_status);
        let moving = matches!(
            s.state,
            PlayerState::Playing | PlayerState::Effect | PlayerState::Testing
        );
        let every = if moving {
            Duration::from_millis(250)
        } else {
            Duration::from_secs(2)
        };
        if changed || now.duration_since(self.last_status_at) >= every {
            self.last_status_at = now;
            self.last_status = s.clone();
            self.status_tx.send_replace(s);
        }
    }

    /// Tell the cluster which look/test followers should show.
    fn publish_extras(&mut self) {
        if self.is_follower() {
            return;
        }
        let effect = match (&self.program, &self.current) {
            (Some(_), Some(a)) => match &a.kind {
                ActiveKind::Effect(l) => Some(l.preset.clone()),
                ActiveKind::Audio | ActiveKind::Pause => {
                    self.idle_layer.as_ref().map(|l| l.layer.preset.clone())
                }
                ActiveKind::Sequence { .. } | ActiveKind::Flash(_) => None,
            },
            _ => self.look.as_ref().map(|l| l.layer.preset.clone()),
        };
        // A live look (effect test) goes out as the effect, on the test's target.
        let (effect, test) = match self.test.as_ref() {
            Some(t) if t.is_look() => (t.look_preset(), None),
            Some(t) => {
                let mut req = t.req.clone();
                // Large plans don't fit a datagram: followers got them by
                // command (F6) and look them up by run id.
                let big = req
                    .map
                    .as_ref()
                    .is_some_and(|m| m.targets.len() > MAP_INLINE_TARGETS);
                if req.map_run_id.is_some() && big {
                    req.map = None;
                }
                (effect, Some(req))
            }
            None => (effect, None),
        };
        if (effect.as_ref(), test.as_ref())
            != (self.last_extras.0.as_ref(), self.last_extras.1.as_ref())
        {
            self.last_extras = (effect.clone(), test.clone());
            if let Some(cluster) = self.app.services.cluster.get() {
                cluster.set_sync_effect(effect);
                cluster.set_sync_test(test);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Calibration flash: on for [`CALIBRATION_FLASH_MS`] from each whole second
/// (at least one presentation slot, so it is never skipped).
pub fn flash_on(pos_ms: f64, slot_ms: f64) -> bool {
    pos_ms >= 0.0 && pos_ms % 1000.0 < CALIBRATION_FLASH_MS.max(slot_ms)
}

/// The calibration sound (a sharp click at every whole second, 60 s), written
/// once to `<data>/cache/calibration-click.wav`.
fn calibration_click(data_dir: &Path) -> std::io::Result<PathBuf> {
    let dir = data_dir.join("cache");
    let path = dir.join("calibration-click.wav");
    let bytes = click_wav(CALIBRATION_LEN_MS);
    if std::fs::metadata(&path).map(|m| m.len()).ok() != Some(bytes.len() as u64) {
        std::fs::create_dir_all(&dir)?;
        let tmp = dir.join("calibration-click.wav.tmp");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &path)?;
    }
    Ok(path)
}

/// The phone calibration sound for `seed` (F1 v2), written once to
/// `<data>/cache/cal-<seed>.wav`.
fn calibration_v2_wav(data_dir: &Path, seed: u32) -> std::io::Result<PathBuf> {
    let dir = data_dir.join("cache");
    let path = dir.join(calpattern::wav_file_name(seed));
    if !path.exists() {
        std::fs::create_dir_all(&dir)?;
        // Only the newest few patterns are kept.
        if let Ok(rd) = std::fs::read_dir(&dir) {
            let mut old: Vec<(std::time::SystemTime, PathBuf)> = rd
                .filter_map(Result::ok)
                .filter(|e| {
                    let n = e.file_name().to_string_lossy().to_string();
                    n.starts_with("cal-") && n.ends_with(".wav")
                })
                .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
                .collect();
            old.sort();
            let excess = old.len().saturating_sub(3);
            for (_, p) in old.into_iter().take(excess) {
                let _ = std::fs::remove_file(p);
            }
        }
        let tmp = dir.join(format!("{}.tmp", calpattern::wav_file_name(seed)));
        std::fs::write(&tmp, calpattern::wav(seed))?;
        std::fs::rename(&tmp, &path)?;
    }
    Ok(path)
}

/// Countdown tick sound (F4): a soft 1 kHz tick at every digit change, the
/// last one a second before zero; `<data>/cache/countdown-<ms>.wav`.
fn countdown_ticks(data_dir: &Path, duration_ms: u64) -> std::io::Result<PathBuf> {
    let dir = data_dir.join("cache");
    let path = dir.join(format!("countdown-{duration_ms}.wav"));
    if path.exists() {
        return Ok(path);
    }
    const RATE: u32 = 16_000;
    let samples = (duration_ms * RATE as u64 / 1000) as usize;
    let mut pcm = vec![0i16; samples];
    let tick = (RATE as usize) * 20 / 1000;
    for t in countdown::tick_times(duration_ms) {
        let start = (t * RATE as u64 / 1000) as usize;
        for i in 0..tick.min(samples.saturating_sub(start)) {
            let x = i as f64 / RATE as f64;
            let env = (-(i as f64) / (tick as f64 / 5.0)).exp();
            let v = (2.0 * std::f64::consts::PI * 1000.0 * x).sin() * env * 0.5;
            pcm[start + i] = (v * i16::MAX as f64) as i16;
        }
    }
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join(format!("countdown-{duration_ms}.wav.tmp"));
    std::fs::write(&tmp, wav_mono16(RATE, &pcm))?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

fn wav_mono16(rate: u32, pcm: &[i16]) -> Vec<u8> {
    let data_len = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + pcm.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// 16-bit mono 24 kHz WAV: a 4 ms 2 kHz tone burst with a sharp attack at
/// every whole second of `len_ms`.
pub fn click_wav(len_ms: u64) -> Vec<u8> {
    const RATE: u32 = 24_000;
    let samples = (len_ms * RATE as u64 / 1000) as usize;
    let mut pcm = vec![0i16; samples];
    let click = (RATE as usize) * 4 / 1000;
    for start in (0..samples).step_by(RATE as usize) {
        for i in 0..click.min(samples - start) {
            let t = i as f64 / RATE as f64;
            let env = (-(i as f64) / (click as f64 / 4.0)).exp();
            let v = (2.0 * std::f64::consts::PI * 2000.0 * t).sin() * env * 0.9;
            pcm[start + i] = (v * i16::MAX as f64) as i16;
        }
    }
    let data_len = (samples * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

fn dummy_node() -> pixelplus_core::model::Node {
    pixelplus_core::model::Node {
        hardware_history: Default::default(),
        serial: Default::default(),
        id: String::new(),
        name: String::new(),
        hostname: String::new(),
        role: pixelplus_core::model::NodeRole::Leader,
        board: BoardKind::Virtual,
        board_rev: None,
        pi_model: None,
        outputs: vec![],
        adopted: false,
        last_seen: None,
        notes: None,
    }
}

/// This node's board: PIXELPLUS_BOARD, the show, the setup wizard, or the platform.
fn board_for(
    show: &Show,
    node_id: &str,
    identity_board: Option<BoardKind>,
    is_pi: bool,
) -> BoardKind {
    // PIXELPLUS_BOARD (e.g. `virtual` in Docker) overrides everything, as in
    // services::system::effective_board.
    if let Some(b) = crate::services::system::board_override() {
        return b;
    }
    show.node(node_id)
        .map(|n| n.board)
        .or(identity_board)
        .unwrap_or(if is_pi {
            BoardKind::BarePi
        } else {
            BoardKind::Virtual
        })
}

/// Pick the output backend. `Auto`: DPI on a Pi with a pixel board and a DRM
/// device; otherwise the simulator (so the live preview and tests work).
fn choose_backend(
    app: &AppState,
    opts: &EngineOptions,
    board: BoardKind,
    is_pi: bool,
) -> BackendKind {
    if let Some(k) = opts.output {
        return k;
    }
    use crate::config::OutputMode;
    match app.config.output {
        OutputMode::Dpi => BackendKind::Dpi,
        OutputMode::Sim => BackendKind::Sim,
        OutputMode::None => BackendKind::None,
        OutputMode::Auto => {
            if board.output_count() > 0 && is_pi && Path::new("/dev/dri").exists() {
                BackendKind::Dpi
            } else {
                BackendKind::Sim
            }
        }
    }
}
