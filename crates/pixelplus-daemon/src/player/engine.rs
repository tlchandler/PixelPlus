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
//! Timing: while a sequence plays the loop runs at the sequence's frame rate
//! and picks the frame for the *current position*, which comes from the audio
//! clock when audio plays (see `audio.rs`) or a monotonic clock otherwise.

use super::audio::{AudioEngine, TrackId};
use super::clock::{self, crossfade_progress, crossfade_start, Fade, MonoClock, SlewClock};
use super::compose::{self, find_effect, EffectLayer, PropSlot, Sink, TestLayer};
use crate::services::tts::DynamicContext;
use super::overlay::OverlayManager;
use super::playlist::PlaylistCursor;
use super::reader::{FrameLayout, FrameReader, SeqMeta};
use super::scheduler::{self, Origin, SchedAction, ScheduleFacts, Scheduler};
use super::types::*;
use super::{OverlayCmd, PlayerCmd, PlayerHandle, SyncPacket};
use crate::api::{ApiError, ApiResult};
use crate::events::ToastKind;
use crate::node::LocalRole;
use crate::state::AppState;
use pixelplus_core::mapping::{NodeMap, OutputFrame, PropMap};
use pixelplus_core::model::{BoardKind, EffectPreset, OutputConfig, PlaylistItem, Show};
use pixelplus_output::{BackendKind, OutputFrameRef, PixelOutput, PixelPipeline, SimHandle, SimOutput};
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
}

impl EngineOptions {
    pub fn from_env() -> Self {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        EngineOptions {
            audio: !matches!(env("PIXELPLUS_AUDIO").as_deref(), Some("none" | "off" | "0" | "false")),
            output: None,
            shm_dir: env("PIXELPLUS_SHM_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/dev/shm")),
            realtime: true,
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
    tokio::spawn(status_publisher(state.clone(), status_rx.clone()));
    Ok(Engine { handle: PlayerHandle::new(cmd_tx, status_rx), sim, core_tx })
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
    GEOMETRY.lock().clone().unwrap_or(GeometryStatus { ok: true, ..Default::default() })
}

// ---------------------------------------------------------------------------
// Messages between the control task and the output thread
// ---------------------------------------------------------------------------

#[allow(clippy::large_enum_variant)]
enum CoreCmd {
    Player(PlayerCmd),
    Show(Arc<Show>),
    Facts(ScheduleFacts),
    Identity { id: String, role: LocalRole, board: Option<BoardKind> },
    DjRendered { clip_id: String, path: PathBuf },
    Shutdown,
}

enum CoreEvent {
    Log { level: &'static str, message: String, toast: bool },
    Games(serde_json::Value),
    PrerenderDj { clip_id: String, ctx: DynamicContext },
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
                let facts = scheduler::facts_at(&show.schedule, chrono::Utc::now());
                if core.send(CoreCmd::Facts(facts)).is_err() {
                    break;
                }
            }
            Some(ev) = ev_rx.recv() => handle_event(&state, &core, ev),
        }
    }
    let _ = core.send(CoreCmd::Shutdown);
}

fn handle_event(state: &AppState, core: &Sender<CoreCmd>, ev: CoreEvent) {
    match ev {
        CoreEvent::Log { level, message, toast } => {
            state.events.publish(
                "log",
                &serde_json::json!({ "level": level, "message": message, "time": chrono::Utc::now().to_rfc3339() }),
            );
            if toast {
                let kind = if level == "error" { ToastKind::Error } else { ToastKind::Warning };
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
                    Ok(Err(e)) => tracing::warn!("rendering DJ clip {clip_id} failed: {e:#}; using its last render"),
                    Err(_) => tracing::warn!("rendering DJ clip {clip_id} timed out; using its last render"),
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
            let mut stream = tokio::net::UnixStream::connect(path).await.map_err(|_| "the games service isn't running".to_string())?;
            let mut line = serde_json::to_vec(cmd).map_err(|e| e.to_string())?;
            line.push(b'\n');
            stream.write_all(&line).await.map_err(|e| e.to_string())?;
            let mut reader = BufReader::new(stream);
            let mut resp = String::new();
            reader.read_line(&mut resp).await.map_err(|e| e.to_string())?;
            serde_json::from_str::<serde_json::Value>(resp.trim()).map_err(|e| e.to_string())
        };
        tokio::time::timeout(Duration::from_secs(4), fut).await.map_err(|_| "the games service did not answer".to_string())?
    }
    #[cfg(not(unix))]
    {
        let _ = (path, cmd);
        Err("games are only available on Linux".into())
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
        // Wait for the frame deadline, applying commands as they arrive.
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
        match catch_unwind(AssertUnwindSafe(|| core.tick())) {
            Ok(()) => core.panics = 0,
            Err(_) => core.recover_from_panic("a frame"),
        }
        let period = core.frame_period();
        next += period;
        let now = Instant::now();
        if next + period < now {
            // Fell behind (slow SD card, overloaded CPU): resynchronise instead of bursting.
            next = now + period;
        }
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
            tracing::debug!("realtime priority not available ({}); using normal priority", std::io::Error::last_os_error());
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
    fn new(kind: BackendKind, board: BoardKind) -> Self {
        let (backend, sim): (Box<dyn PixelOutput>, _) = match kind {
            BackendKind::Sim => {
                let s = SimOutput::new();
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

/// Something that will play next.
#[derive(Debug, Clone)]
enum Pending {
    Item(PlaylistItem),
    Request { sequence_id: String, name: Option<String> },
}

enum ActiveKind {
    Sequence { reader: FrameReader, meta: Option<SeqMeta> },
    Effect(Box<EffectLayer>),
    /// DJ clip or media: audio only, the idle look runs under it.
    Audio,
    Pause,
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
    buf: Vec<u8>,
    have_frame: bool,
    warned_audio: bool,
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
            buf: Vec::new(),
            have_frame: false,
            warned_audio: false,
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
}

#[derive(Default)]
struct FollowState {
    pkt: Option<SyncPacket>,
    rx_ms: f64,
    clock: Option<SlewClock>,
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
}

fn item_ref(kind: &str, id: &str, name: &str) -> ItemRef {
    ItemRef { kind: kind.into(), id: id.into(), name: name.into() }
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
                pixelplus_hw::board::PiFamily::Zero2W | pixelplus_hw::board::PiFamily::Zero | pixelplus_hw::board::PiFamily::Pi1
            )
        });
        let board = board_for(&show, &identity.id, identity.board, is_pi);
        let kind = choose_backend(&app, &opts, board, is_pi);
        tracing::info!("pixel output: {kind:?} for board {board:?}");
        let audio_device = show.settings.audio.device.clone();
        let want_audio = opts.audio && identity.role != LocalRole::Follower;
        let audio = AudioEngine::disabled(if want_audio { "the audio device is starting" } else { "audio is disabled on this controller" });
        let volume = show.settings.audio.volume.min(100);
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
            output: OutputStage::new(kind, board),
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
            brightness: 100,
            volume,
            applied_volume: None,
            settings_volume: volume,
            blackout: false,
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
            show: show.clone(),
            app,
            opts,
        };
        core.rebuild_maps();
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
        let _ = self.events.send(CoreEvent::Log { level: "warning", message, toast });
    }

    fn recover_from_panic(&mut self, what: &str) {
        self.panics += 1;
        tracing::error!("the player hit an internal error in {what}; recovering (#{})", self.panics);
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
            self.item_error = Some(("Playback stopped after an internal error".into(), Instant::now()));
            let _ = self.events.send(CoreEvent::Log {
                level: "error",
                message: "Playback stopped after an internal error; the player recovered.".into(),
                toast: true,
            });
        }
        if self.panics >= 6 {
            // Maybe the output backend is wedged: rebuild it.
            self.output = OutputStage::new(self.output.kind, self.board);
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
                self.prop_map = PropMap::build_with_layout(n, &show.props, self.node_map.pixels_per_output());
                let count = self.node_map.pixels_per_output().len();
                let configs: Vec<OutputConfig> = (0..count)
                    .map(|i| {
                        n.outputs
                            .iter()
                            .find(|o| o.index as usize == i + 1)
                            .cloned()
                            .unwrap_or_else(|| OutputConfig { index: i as u32 + 1, ..Default::default() })
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
        self.slots = show.props.iter().map(|p| (p.id.clone(), PropSlot::of(p))).collect();
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
            .unwrap_or(0);
        self.chan.resize(chan_len, 0);
        self.chan_b.resize(chan_len, 0);
        let total = self.node_map.total_pixels();
        let longest = self.node_map.pixels_per_output().iter().copied().max().unwrap_or(0);
        self.effect_period = if self.slow_pi || total > 20_000 || longest > 800 {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(25)
        };
        self.update_geometry();
    }

    fn update_geometry(&self) {
        let longest = self.node_map.pixels_per_output().iter().copied().max().unwrap_or(0);
        let max = self.output.max_pixels();
        let needed = pixelplus_output::DpiGeometry::for_pixels(longest.max(1)).ok().map(|g| g.pixels_per_output);
        let ok = max.map_or(true, |m| longest <= m);
        let message = (!ok).then(|| {
            format!(
                "The longest string has {longest} pixels but the pixel output was set up at boot for {} per output. Reboot the controller to apply the new length.",
                max.unwrap_or(0)
            )
        });
        *GEOMETRY.lock() = Some(GeometryStatus { ok, longest_string: longest, max_pixels: max, needed_pixels: needed, message });
    }

    fn reload(&mut self, show: Arc<Show>) {
        self.show = show;
        let board = board_for(&self.show, &self.node_id, self.identity_board, self.is_pi);
        let kind = choose_backend(&self.app, &self.opts, board, self.is_pi);
        if kind != self.output.kind || (kind == BackendKind::Dpi && board != self.output.board) {
            tracing::info!("pixel output changes to {kind:?} for board {board:?}");
            self.output.backend.stop();
            self.output = OutputStage::new(kind, board);
        }
        self.board = board;
        self.rebuild_maps();
        let show = self.show.clone();
        self.overlays.retain_props(|id| show.prop(id).is_some());
        // Settings: volume set in the UI, audio device.
        let sv = show.settings.audio.volume.min(100);
        if sv != self.settings_volume {
            self.settings_volume = sv;
            self.volume = sv;
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
            PlayerCmd::SetVolume(v) => self.volume = v.min(100),
            PlayerCmd::SetBrightness(b) => self.brightness = b.min(100),
            PlayerCmd::Blackout(on) => self.blackout = on,
            PlayerCmd::TestStart(req, reply) => {
                let r = match TestLayer::new(&self.show, &self.node_id, &req, now_ms) {
                    Ok(t) => {
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
            PlayerCmd::Overlay(o) => self.overlay_cmd(o),
            PlayerCmd::Reload => {
                let show = self.app.store.get();
                self.reload(show);
            }
        }
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
            OverlayCmd::Text { prop_id, text, color, scroll, duration_ms } => {
                if let Some(p) = prop_of(&prop_id) {
                    self.overlays.text(p, &text, &color, scroll, duration_ms, now);
                }
            }
            OverlayCmd::Qr { prop_id, url, duration_ms } => {
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
            return Err(ApiError::conflict("This controller follows its show leader; control playback on the leader."));
        }
        // Pressing play ends a live look ("Effects → Show live"): it is drawn on top of
        // everything and would otherwise hide what was just started.
        if self.test.as_ref().is_some_and(|t| t.req.mode == "effect") {
            self.test = None;
        }
        let show = self.show.clone();
        let (source, first, playlist, crossfade) = if let Some(id) = &req.playlist_id {
            let pl = show.playlist(id).ok_or_else(|| ApiError::not_found("That playlist"))?;
            let mut cursor = PlaylistCursor::new(pl.clone());
            if let Some(i) = req.start_index {
                cursor.start_at(i as usize);
            }
            let first = cursor.current().cloned().ok_or_else(|| ApiError::bad_request("That playlist is empty."))?;
            (
                Source::Playlist(Box::new(cursor)),
                Pending::Item(first),
                Some((pl.id.clone(), pl.name.clone())),
                pl.crossfade_ms,
            )
        } else if let Some(id) = &req.sequence_id {
            show.sequence(id).ok_or_else(|| ApiError::not_found("That sequence"))?;
            (Source::Single, Pending::Item(PlaylistItem::Sequence { id: "manual".into(), sequence_id: id.clone() }), None, 0)
        } else if let Some(id) = &req.dj_clip_id {
            show.dj_clip(id).ok_or_else(|| ApiError::not_found("That DJ clip"))?;
            (Source::Single, Pending::Item(PlaylistItem::Dj { id: "manual".into(), dj_clip_id: id.clone() }), None, 0)
        } else if let Some(id) = &req.effect_id {
            find_effect(&show, id).ok_or_else(|| ApiError::not_found("That look"))?;
            (
                Source::Single,
                Pending::Item(PlaylistItem::Effect { id: "manual".into(), effect_id: id.clone(), duration_ms: 0 }),
                None,
                0,
            )
        } else if let Some(id) = &req.media_id {
            show.media_item(id).ok_or_else(|| ApiError::not_found("That audio file"))?;
            (Source::Single, Pending::Item(PlaylistItem::Media { id: "manual".into(), media_id: id.clone() }), None, 0)
        } else {
            return Err(ApiError::bad_request("Choose a playlist, sequence, DJ clip, look or audio file to play."));
        };
        self.start_program(Program { origin: Origin::Manual, source, playlist, crossfade_ms: crossfade }, first, now_ms);
        match &self.program {
            Some(p) if p.origin == Origin::Manual => Ok(()),
            _ => Err(ApiError::bad_request(
                self.item_error.as_ref().map(|e| e.0.clone()).unwrap_or_else(|| "Nothing playable.".into()),
            )),
        }
    }

    fn start_program(&mut self, program: Program, first: Pending, now_ms: f64) {
        // Replace whatever plays (quick fade to avoid clicks).
        self.audio.stop_all(80);
        self.current = None;
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
            if let Some(id) = a.audio {
                self.audio.stop(id);
            }
        }
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
        for a in self.current.iter_mut().chain(self.outgoing.iter_mut().map(|o| &mut o.active)) {
            a.clock.set_paused(paused, now_ms);
            if let Some(id) = a.audio {
                self.audio.set_paused(id, paused);
            }
        }
    }

    fn seek(&mut self, pos: f64, now_ms: f64) {
        let Some(a) = self.current.as_mut() else { return };
        let pos = match a.duration_ms {
            Some(d) => pos.clamp(0.0, d as f64),
            None => pos.max(0.0),
        };
        a.clock.seek(pos, now_ms);
        a.last_pos = pos;
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
            self.warn("A song request was for a sequence that no longer exists", false);
            return;
        }
        if self.program.is_none() || self.stop_fade.is_some() {
            self.start_program(
                Program { origin: Origin::Manual, source: Source::Single, playlist: None, crossfade_ms: 0 },
                Pending::Request { sequence_id, name },
                now_ms,
            );
        } else {
            self.requests.push_back((sequence_id, name));
        }
    }

    /// Stop the current item's audio (quick fade) and drop it.
    fn end_current(&mut self, fade_ms: u32) {
        if let Some(a) = self.current.take() {
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
        match self.program.as_mut().map(|p| &mut p.source) {
            Some(Source::Playlist(c)) => c.advance().cloned().map(Pending::Item),
            _ => None,
        }
    }

    fn peek_next(&self) -> Option<Pending> {
        if let Some((sequence_id, name)) = self.requests.front() {
            return Some(Pending::Request { sequence_id: sequence_id.clone(), name: name.clone() });
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
                    active.fade_in_ms = fade_in_ms;
                    self.current = Some(active);
                    self.after_begin(&p);
                    return;
                }
                Ok(None) => {} // instant item (command)
                Err(e) => {
                    let name = self.pending_ref(&p).name;
                    let msg = format!("Skipped “{name}”: {e}");
                    self.item_error = Some((msg.clone(), Instant::now()));
                    self.warn(msg, true);
                }
            }
            attempts += 1;
            if attempts >= 64 {
                self.warn("Nothing in this playlist can be played right now; stopping.", true);
                self.playback_finished();
                return;
            }
            item = self.take_next();
        }
    }

    fn playback_finished(&mut self) {
        if let Some(Program { origin: Origin::Schedule(key), .. }) = &self.program {
            self.sched.on_finished(key);
        }
        self.clear_playback();
        let now_ms = self.now_ms();
        self.run_scheduler(now_ms);
    }

    /// After an item started: pre-render an upcoming dynamic DJ clip.
    fn after_begin(&mut self, started: &Pending) {
        let Some(Pending::Item(PlaylistItem::Dj { dj_clip_id, .. })) = self.peek_next() else { return };
        let Some(clip) = self.show.dj_clip(&dj_clip_id) else { return };
        if !clip.dynamic {
            return;
        }
        // The song after the DJ clip.
        let next_song = match self.program.as_ref().map(|p| &p.source) {
            Some(Source::Playlist(c)) => {
                let mut c = c.clone();
                c.advance();
                c.advance().cloned().map(|i| self.pending_ref(&Pending::Item(i)).name)
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
        let _ = self.events.send(CoreEvent::PrerenderDj { clip_id: dj_clip_id, ctx });
    }

    fn pending_ref(&self, p: &Pending) -> ItemRef {
        let show = &self.show;
        match p {
            Pending::Request { sequence_id, .. } => {
                let name = show.sequence(sequence_id).map_or("Song request", |s| s.name.as_str());
                item_ref("request", sequence_id, name)
            }
            Pending::Item(i) => match i {
                PlaylistItem::Sequence { sequence_id, .. } => {
                    item_ref("sequence", sequence_id, show.sequence(sequence_id).map_or("Missing sequence", |s| &s.name))
                }
                PlaylistItem::Dj { dj_clip_id, .. } => {
                    item_ref("dj", dj_clip_id, show.dj_clip(dj_clip_id).map_or("Missing DJ clip", |c| &c.name))
                }
                PlaylistItem::Effect { effect_id, .. } => {
                    let name = find_effect(show, effect_id).map(|e| e.name).unwrap_or_else(|| "Missing look".into());
                    item_ref("effect", effect_id, &name)
                }
                PlaylistItem::Media { media_id, .. } => {
                    item_ref("media", media_id, show.media_item(media_id).map_or("Missing audio", |m| &m.name))
                }
                PlaylistItem::Pause { id, .. } => item_ref("pause", id, "Pause"),
                PlaylistItem::Command { id, command, .. } => item_ref("command", id, command),
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
        let single = matches!(self.program.as_ref().map(|p| &p.source), Some(Source::Single));
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
                        None => self.warn(format!("The audio for “{}” is missing; playing lights only", seq.name), true),
                    },
                    None => self.warn(format!("The audio linked to “{}” was deleted; playing lights only", seq.name), false),
                }
            }
            return Ok(Some(a));
        }
        let Pending::Item(item) = p else { unreachable!("requests are sequences") };
        match item {
            PlaylistItem::Sequence { .. } => unreachable!("handled above"),
            PlaylistItem::Dj { dj_clip_id, .. } => {
                let clip = show.dj_clip(dj_clip_id).ok_or("the DJ clip was deleted")?;
                let rendered = self.dj_rendered.get(dj_clip_id).filter(|p| p.exists()).cloned();
                let media = clip.media_id.as_deref().and_then(|m| show.media_item(m));
                let (path, gain, duration) = match (rendered, media) {
                    (Some(p), m) => (p, 0.0, m.map(|m| m.duration_ms)),
                    (None, Some(m)) => (
                        self.media_path(&m.file).ok_or("the DJ clip's audio file is missing")?,
                        self.media_gain(m),
                        Some(m.duration_ms),
                    ),
                    (None, None) => return Err("the DJ clip has not been rendered yet".into()),
                };
                let mut a = Active::new(iref, ActiveKind::Audio, now_ms);
                a.audio_src = Some((path, gain));
                a.duration_ms = duration.filter(|&d| d > 0);
                Ok(Some(a))
            }
            PlaylistItem::Media { media_id, .. } => {
                let m = show.media_item(media_id).ok_or("the audio file was deleted")?;
                let path = self.media_path(&m.file).ok_or("the audio file is missing")?;
                let mut a = Active::new(iref, ActiveKind::Audio, now_ms);
                a.audio_src = Some((path, self.media_gain(m)));
                a.duration_ms = (m.duration_ms > 0).then_some(m.duration_ms);
                Ok(Some(a))
            }
            PlaylistItem::Effect { effect_id, duration_ms, .. } => {
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
        }
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
                let duration = args.get("durationMs").and_then(|v| v.as_u64()).unwrap_or(15_000);
                self.overlays.text(prop, &text, &color, scroll, duration, Instant::now());
            }
            other => self.warn(format!("Unknown playlist command “{other}” skipped"), false),
        }
    }

    // ----- scheduler -----------------------------------------------------

    fn run_scheduler(&mut self, now_ms: f64) {
        if self.is_follower() {
            return;
        }
        let origin = if self.stop_fade.is_some() { None } else { self.program.as_ref().map(|p| p.origin.clone()) };
        if self.stop_fade.is_some() {
            return; // decide once the fade has finished
        }
        let Some(action) = self.sched.decide(&self.facts, origin.as_ref()) else { return };
        self.sched.prune(&self.facts);
        match action {
            SchedAction::Start(w) => {
                let show = self.show.clone();
                let Some(pl) = show.playlist(&w.playlist_id) else {
                    self.warn(format!("The schedule “{}” plays a playlist that no longer exists", w.name), true);
                    self.sched.on_finished(&w.key);
                    return;
                };
                let cursor = PlaylistCursor::new(pl.clone());
                let Some(first) = cursor.current().cloned() else {
                    self.sched.on_finished(&w.key);
                    return;
                };
                tracing::info!("schedule: starting “{}” ({})", w.name, pl.name);
                self.look = None;
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
            }
            SchedAction::End(behavior) => {
                use pixelplus_core::model::EndBehavior;
                tracing::info!("schedule: window ended ({behavior:?})");
                match behavior {
                    EndBehavior::FinishSong => {
                        self.requests.clear();
                        if let Some(Source::Playlist(c)) = self.program.as_mut().map(|p| &mut p.source) {
                            c.finish_after_current();
                        }
                    }
                    EndBehavior::StopNow => self.stop(false, false, now_ms),
                    EndBehavior::FadeOut => self.stop(true, false, now_ms),
                }
            }
            SchedAction::Look(id) => self.set_look(id, false),
        }
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
            })
        });
    }

    /// The idle look that runs under DJ clips and pauses.
    fn ensure_idle_layer(&mut self) {
        let want = self.facts.idle_effect_id.clone().or_else(|| self.show.schedule.idle_effect_id.clone());
        let have = self.idle_layer.as_ref().map(|l| l.id.clone());
        if want == have {
            return;
        }
        let now_ms = self.now_ms();
        self.idle_layer = want.and_then(|id| {
            let preset = find_effect(&self.show, &id)?;
            Some(Look { id, name: preset.name.clone(), layer: EffectLayer::new(&self.show, &preset), started_ms: now_ms })
        });
    }

    // ----- per-frame -----------------------------------------------------

    fn tick(&mut self) {
        let now = Instant::now();
        let now_ms = self.now_ms();
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
        let busy = self.current.as_ref().is_some_and(|a| a.audio.is_some()) || self.outgoing.is_some();
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
        if want && !busy && (device_changed || (!self.audio.available() && now >= self.audio_retry_at)) {
            if device_changed {
                self.audio_backoff = Duration::from_secs(15);
            }
            self.audio_device = wanted.clone();
            // Close the old device first (ALSA devices are exclusive).
            self.audio = AudioEngine::disabled("the audio device is starting");
            let (tx, rx) = std::sync::mpsc::channel();
            let spawned = std::thread::Builder::new().name("pp-audio-open".into()).spawn(move || {
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

    fn position(audio: &AudioEngine, a: &mut Active, paused: bool, now_ms: f64) -> f64 {
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
        p
    }

    fn try_start(&mut self, now_ms: f64) -> Result<(), String> {
        let paused = self.paused;
        let Some(a) = self.current.as_mut() else { return Ok(()) };
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
        a.clock = MonoClock::new(0.0, now_ms);
        if paused {
            a.clock.set_paused(true, now_ms);
        }
        if let Some((path, gain)) = a.audio_src.clone() {
            a.audio = self.audio.play(&path, 0, gain, a.fade_in_ms);
            a.use_audio_clock = a.audio.is_some();
            if let (Some(id), true) = (a.audio, paused) {
                self.audio.set_paused(id, true);
            }
        }
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
            let name = self.current.as_ref().map(|a| a.iref.name.clone()).unwrap_or_default();
            let msg = format!("Skipped “{name}”: {e}");
            self.item_error = Some((msg.clone(), Instant::now()));
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
        let Some(a) = self.current.as_mut() else { return };
        if !a.started || paused {
            return;
        }
        let pos = Self::position(&self.audio, a, paused, now_ms);
        // Audio problems: warn once; audio-only items end with their track.
        if let Some(id) = a.audio {
            if let Some(e) = self.audio.track_error(id) {
                if !a.warned_audio {
                    a.warned_audio = true;
                    let msg = format!("Couldn't play the audio for “{}”: {e}", a.iref.name);
                    self.item_error = Some((msg.clone(), Instant::now()));
                    let _ = self.events.send(CoreEvent::Log { level: "warning", message: msg, toast: true });
                }
            }
        }
        let ended = match &a.kind {
            ActiveKind::Sequence { .. } | ActiveKind::Effect(_) | ActiveKind::Pause => {
                a.duration_ms.is_some_and(|d| pos >= d as f64)
            }
            ActiveKind::Audio => match a.audio {
                Some(id) => self.audio.is_finished(id) || a.duration_ms.is_some_and(|d| pos >= d as f64 + 5000.0),
                // No audio device: keep the show's timing using the file's duration.
                None => a.duration_ms.map_or(true, |d| pos >= d as f64),
            },
        };
        let remaining = a.duration_ms.map(|d| d as f64 - pos);
        let duration = a.duration_ms;
        if ended {
            if let Some(id) = a.audio {
                self.audio.fade_out(id, 30);
            }
            self.current = None;
            let next = self.take_next();
            self.start_next(next, now_ms, 0);
            return;
        }
        // Preload the next sequence a few seconds ahead (gapless starts).
        if remaining.is_some_and(|r| r < 4000.0) && self.preload.is_none() {
            let next_seq = match self.peek_next() {
                Some(Pending::Request { sequence_id, .. }) => Some(sequence_id),
                Some(Pending::Item(PlaylistItem::Sequence { sequence_id, .. })) => Some(sequence_id),
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
        // Crossfade into the next item.
        if crossfade_ms > 0 && self.outgoing.is_none() {
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
                    self.outgoing = Some(Outgoing { active: old, start_ms: now_ms, len_ms: len });
                    let next = self.take_next();
                    self.start_next(next, now_ms, len as u32);
                }
            }
        }
    }

    /// Render an item into channel space.
    fn render_active(
        a: &mut Active,
        pos: f64,
        chan: &mut [u8],
        idle: Option<&mut Look>,
        now_ms: f64,
    ) {
        match &mut a.kind {
            ActiveKind::Sequence { reader, meta } => {
                if let Some(m) = meta {
                    let idx = (pos / m.frame_ms as f64) as u32;
                    if a.buf.len() == m.frame_len && reader.get(idx, &mut a.buf) {
                        a.have_frame = true;
                    }
                }
                if a.have_frame {
                    let n = chan.len().min(a.buf.len());
                    chan[..n].copy_from_slice(&a.buf[..n]);
                }
            }
            ActiveKind::Effect(layer) => layer.render(pos as u64, &mut Sink::Chan(chan)),
            ActiveKind::Audio | ActiveKind::Pause => {
                if let Some(l) = idle {
                    l.layer.render((now_ms - l.started_ms).max(0.0) as u64, &mut Sink::Chan(chan));
                }
            }
        }
    }

    fn compose_leader(&mut self, now_ms: f64) {
        let paused = self.paused;
        let needs_idle = self
            .current
            .iter()
            .chain(self.outgoing.iter().map(|o| &o.active))
            .any(|a| matches!(a.kind, ActiveKind::Audio | ActiveKind::Pause));
        if needs_idle {
            self.ensure_idle_layer();
        }
        self.chan.fill(0);
        let mut blend_t = None;
        if let Some(o) = self.outgoing.as_mut() {
            self.chan_b.fill(0);
            let pos = Self::position(&self.audio, &mut o.active, paused, now_ms);
            Self::render_active(&mut o.active, pos, &mut self.chan_b, self.idle_layer.as_mut(), now_ms);
            blend_t = Some(crossfade_progress(o.start_ms, o.len_ms, now_ms));
        }
        self.shown = None;
        if let Some(a) = self.current.as_mut() {
            let pos = if a.started { Self::position(&self.audio, a, paused, now_ms) } else { 0.0 };
            if a.started || blend_t.is_none() {
                Self::render_active(a, pos, &mut self.chan, self.idle_layer.as_mut(), now_ms);
            }
            if let (true, ActiveKind::Sequence { meta: Some(m), .. }) = (a.have_frame, &a.kind) {
                self.shown = Some((a.iref.id.clone(), (pos / m.frame_ms as f64) as u32));
            }
        } else if self.program.is_none() {
            if let Some(l) = self.look.as_mut() {
                l.layer.render((now_ms - l.started_ms).max(0.0) as u64, &mut Sink::Chan(&mut self.chan));
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
            t.render_raw(now_ms, &mut self.frame);
        }
    }

    fn compose_follower(&mut self, now_ms: f64) {
        self.frame.clear();
        let f = &mut self.follow;
        if f.meta.is_none() {
            // The slice finished opening since the last sync packet.
            if let Some(m) = f.reader.as_ref().and_then(|r| r.meta()) {
                f.buf = vec![0; m.frame_len];
                f.meta = Some(m);
            }
        }
        let pos = f.clock.as_mut().map_or(0.0, |c| c.advance(now_ms));
        self.shown = None;
        if let (Some(reader), Some(meta)) = (f.reader.as_ref(), f.meta.as_ref()) {
            let idx = (pos.max(0.0) / meta.frame_ms as f64) as u32;
            if f.buf.len() == meta.frame_len && reader.get(idx, &mut f.buf) {
                f.have_frame = true;
            }
            if f.have_frame {
                let id = f.pkt.as_ref().and_then(|p| p.item.as_ref()).map(|i| i.id.clone());
                self.shown = id.map(|id| (id, idx));
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
            e.render(pos.max(0.0) as u64, &mut Sink::Frame(&mut self.frame, &self.prop_map));
        }
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
            t.render_raw(now_ms, &mut self.frame);
        }
        if let Some(t) = self.test.as_mut() {
            t.render_raw(now_ms, &mut self.frame);
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

    fn write_output(&mut self, now: Instant, now_ms: f64) {
        let level = self.light_level(now_ms);
        let master = (self.brightness as f32 * level).round() as u8;
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
                let wire = self.wire.as_frame_ref();
                self.output.write(&wire, now);
                if let Some(tap) = &self.tap {
                    let shown = self.shown.as_ref().map(|(id, f)| (id.as_str(), *f));
                    tap.record(self.frames_out + 1, now_ms, shown, master, &ppo[..n], data, &mut wire.iter());
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
        let level = self.light_level(now_ms) * self.brightness as f32 / 100.0;
        let bytes = if self.is_follower() {
            compose::preview_frame(&self.show, self.preview_no, level, None, Some((&self.frame, &self.prop_map)))
        } else {
            compose::preview_frame(&self.show, self.preview_no, level, Some(&self.chan), None)
        };
        self.app.events.preview(bytes::Bytes::from(bytes));
    }

    fn frame_period(&self) -> Duration {
        let clamp = |ms: u32| Duration::from_millis(ms.clamp(10, 100) as u64);
        let seq_ms = if self.is_follower() {
            self.follow.meta.as_ref().filter(|_| self.follow.reader.is_some()).map(|m| m.frame_ms)
        } else {
            match self.current.as_ref().map(|a| &a.kind) {
                Some(ActiveKind::Sequence { meta: Some(m), .. }) => Some(m.frame_ms),
                _ => None,
            }
        };
        if let Some(ms) = seq_ms {
            // Crossfades and live overlays (games) run at least at the effect rate.
            let fast = self.outgoing.is_some() || self.overlays.any_active();
            return clamp(ms).min(if fast { self.effect_period } else { Duration::MAX });
        }
        let busy = self.program.is_some()
            || self.look.is_some()
            || self.test.is_some()
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
        // Position at `now` (sent_at is already on our clock; see cluster::follower::localize_sync).
        let age = (now_ms - p.sent_at_ms as f64).clamp(0.0, 2000.0);
        let playing = p.state == PlayerState::Playing || p.state == PlayerState::Effect;
        let target = p.pos_ms as f64 + if playing { age } else { 0.0 };

        // Tests carried in the packet.
        let mut new_leader_test = false;
        match (&p.test, p.state) {
            (Some(t), PlayerState::Testing) => {
                if f.test.as_ref().map(|x| &x.req) != Some(t) {
                    f.test = TestLayer::new(&show, &self.node_id, t, now_ms).ok();
                    new_leader_test = true;
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
        let key = seq_item.as_ref().map(|i| format!("seq:{}", i.id));
        if key != f.item_key {
            f.item_key = key;
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
                    None => f.missing = Some(format!("“{}” is not on this controller yet", item.name)),
                }
            }
            f.clock = None;
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
        let effect = if seq_item.is_none() { p.effect.clone() } else { None };
        match effect {
            Some(e) => {
                if f.effect.as_ref().map(|x| &x.preset) != Some(&e) {
                    f.effect = Some(EffectLayer::new(&show, &e));
                }
            }
            None => f.effect = None,
        }
        // Clock.
        let frame_ms = f.meta.as_ref().map_or(25.0, |m| m.frame_ms as f64);
        let active = seq_item.is_some() || f.effect.is_some();
        if !active {
            f.clock = None;
        } else {
            let c = f.clock.get_or_insert_with(|| SlewClock::new(target, now_ms));
            if p.state == PlayerState::Paused {
                c.set_running(false, now_ms);
                c.update(target, now_ms, frame_ms);
            } else {
                c.set_running(true, now_ms);
                c.update(target, now_ms, frame_ms);
            }
        }
        let released = p.leader.is_empty() && p.state == PlayerState::Idle;
        f.pkt = Some(p);
        if new_leader_test {
            // A test started on the leader (fault finder, test pattern) replaces a
            // local one, e.g. the "identify" chase, which would otherwise cover it.
            self.test = None;
        }
        if released {
            // Released by the leader: stop everything it had us doing.
            self.follow = FollowState::default();
            self.test = None;
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
                message: "Lost contact with the show leader; lights are dark until it returns.".into(),
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
                s.state = if f.hold_since.is_some() && p.state != PlayerState::Idle { PlayerState::Paused } else { p.state };
                s.item = p.item.clone();
                s.pos_ms = f.clock.as_ref().map_or(p.pos_ms, |c| c.pos().max(0.0) as u64);
            }
            if let Some(m) = &f.meta {
                s.duration_ms = m.duration_ms();
            }
            if let Some(m) = &f.missing {
                errors.push(m.clone());
            }
            if f.pkt.as_ref().is_some_and(|p| p.state != PlayerState::Idle) && now_ms - f.rx_ms > 3000.0 {
                errors.push("Lost contact with the show leader".into());
            }
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
                if let (Source::Playlist(c), Some((id, name))) = (&prog.source, &prog.playlist) {
                    let (index, count) = c.flat_index();
                    s.playlist = Some(PlaylistRef { id: id.clone(), name: name.clone(), index, count });
                }
                s.next_item = self.peek_next().map(|p| self.pending_ref(&p));
                let wants_audio = a.audio_src.is_some();
                if wants_audio && self.opts.audio && !self.audio.available() {
                    errors.push(format!(
                        "No sound: {}. The lights keep playing.",
                        self.audio.error().unwrap_or_else(|| "the audio device is unavailable".into())
                    ));
                }
            } else if let Some(l) = &self.look {
                s.state = PlayerState::Effect;
                s.item = Some(item_ref("effect", &l.id, &l.name));
                s.pos_ms = (now_ms - l.started_ms).max(0.0) as u64;
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
            }
            Some(_) => s.state = PlayerState::Testing,
            None => {}
        }
        if !errors.is_empty() {
            s.error = Some(errors.join(" · "));
        }
        s
    }

    fn publish_status(&mut self, now: Instant, now_ms: f64) {
        let s = self.build_status(now_ms);
        let strip = |s: &PlayerStatus| PlayerStatus { pos_ms: 0, fps: 0.0, ..s.clone() };
        let changed = strip(&s) != strip(&self.last_status);
        let moving = matches!(s.state, PlayerState::Playing | PlayerState::Effect | PlayerState::Testing);
        let every = if moving { Duration::from_millis(250) } else { Duration::from_secs(2) };
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
                ActiveKind::Audio | ActiveKind::Pause => self.idle_layer.as_ref().map(|l| l.layer.preset.clone()),
                ActiveKind::Sequence { .. } => None,
            },
            _ => self.look.as_ref().map(|l| l.layer.preset.clone()),
        };
        // A live look (effect test) goes out as the effect, on the test's target.
        let (effect, test) = match self.test.as_ref() {
            Some(t) if t.is_look() => (t.look_preset(), None),
            Some(t) => (effect, Some(t.req.clone())),
            None => (effect, None),
        };
        if (effect.as_ref(), test.as_ref()) != (self.last_extras.0.as_ref(), self.last_extras.1.as_ref()) {
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

fn dummy_node() -> pixelplus_core::model::Node {
    pixelplus_core::model::Node {
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
fn board_for(show: &Show, node_id: &str, identity_board: Option<BoardKind>, is_pi: bool) -> BoardKind {
    // PIXELPLUS_BOARD (e.g. `virtual` in Docker) overrides everything, as in
    // services::system::effective_board.
    if let Some(b) = crate::services::system::board_override() {
        return b;
    }
    show.node(node_id)
        .map(|n| n.board)
        .or(identity_board)
        .unwrap_or(if is_pi { BoardKind::BarePi } else { BoardKind::Virtual })
}

/// Pick the output backend. `Auto`: DPI on a Pi with a pixel board and a DRM
/// device; otherwise the simulator (so the live preview and tests work).
fn choose_backend(app: &AppState, opts: &EngineOptions, board: BoardKind, is_pi: bool) -> BackendKind {
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
