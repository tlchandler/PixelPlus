//! Engine integration tests: a temp data dir with real `.fseq` files, a show
//! with two props on a simulated-output node, and the real output thread.

use super::engine::{start_with, Engine, EngineOptions};
use super::*;
use crate::config::{Config, OutputMode};
use crate::events::EventBus;
use crate::node::{LocalRole, NodeIdentity};
use crate::state::{AppInner, AppState};
use crate::store::ShowStore;
use pixelplus_core::fseq::{FseqWriter, FseqWriterOptions};
use pixelplus_core::mapping::NodeMap;
use pixelplus_core::model::*;
use pixelplus_output::BackendKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Env {
    state: AppState,
    dir: PathBuf,
    engine: Engine,
}

impl Drop for Env {
    fn drop(&mut self) {
        self.engine.shutdown();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Env {
    fn out(&self, i: usize) -> Vec<u8> {
        let sim = self.engine.sim.as_ref().expect("sim output");
        sim.with_latest(|s| s.outputs.get(i).cloned().unwrap_or_default())
    }

    fn status(&self) -> PlayerStatus {
        self.engine.handle.status()
    }
}

fn prop(id: &str, n: u32, chan: u32, output: u32) -> Prop {
    Prop {
        id: id.into(),
        name: id.to_uppercase(),
        kind: PropKind::Line,
        pixel_count: n,
        xlights_model: None,
        channel_start: chan,
        channels_per_pixel: 3,
        segments: vec![PropSegment {
            node_id: "n1".into(),
            output,
            start_pixel: 0,
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
    }
}

/// 15 channels: prop A = channels 0..6 (output 1), prop B = 6..15 (output 2).
fn write_fseq(path: &Path, frames: u32, frame_ms: u8, value: impl Fn(u32) -> u8) {
    let mut w = FseqWriter::create(path, FseqWriterOptions::new(15, frame_ms)).unwrap();
    for f in 0..frames {
        w.write_frame(&[value(f); 15]).unwrap();
    }
    w.finish().unwrap();
}

fn sequence(dir: &Path, id: &str, frames: u32, frame_ms: u8, value: impl Fn(u32) -> u8) -> Sequence {
    write_fseq(&dir.join(format!("sequences/{id}.fseq")), frames, frame_ms, value);
    Sequence {
        id: id.into(),
        name: format!("Song {id}"),
        file: format!("sequences/{id}.fseq"),
        duration_ms: frames as u64 * frame_ms as u64,
        frame_ms: frame_ms as u32,
        channel_count: 15,
        media_id: None,
        xlights_name: None,
        thumbnail: None,
        hash: String::new(),
    }
}

fn base_show() -> Show {
    let mut show = Show::default();
    show.nodes.push(Node {
        id: "n1".into(),
        name: "Leader".into(),
        hostname: "leader".into(),
        role: NodeRole::Leader,
        board: BoardKind::Difftx,
        board_rev: None,
        pi_model: None,
        outputs: BoardKind::Difftx.default_outputs(),
        adopted: true,
        last_seen: None,
        notes: None,
    });
    show.props = vec![prop("a", 2, 0, 1), prop("b", 3, 6, 2)];
    show
}

fn playlist(id: &str, items: &[&str], crossfade_ms: u32) -> Playlist {
    Playlist {
        id: id.into(),
        name: "Tonight".into(),
        items: items
            .iter()
            .map(|s| PlaylistItem::Sequence { id: format!("i-{s}"), sequence_id: s.to_string() })
            .collect(),
        intro: vec![],
        outro: vec![],
        shuffle: false,
        repeat: false,
        crossfade_ms,
    }
}

async fn env(role: LocalRole, audio: bool, setup: impl FnOnce(&Path, &mut Show)) -> Env {
    let dir = std::env::temp_dir().join(format!("pp-engine-{}", new_id()));
    let config = Config {
        data_dir: dir.clone(),
        web_dir: dir.join("web"),
        http_addr: "127.0.0.1:0".parse().unwrap(),
        cluster_port: 0,
        output: OutputMode::Sim,
        tts_url: "http://127.0.0.1:9".into(),
        games_socket: dir.join("games.sock"),
        dev: true,
    };
    config.ensure_dirs().unwrap();
    let mut show = base_show();
    setup(&dir, &mut show);
    let events = EventBus::new();
    let store = ShowStore::load(&config.show_path(), events.clone()).unwrap();
    store.replace(show).await.unwrap();
    let identity = NodeIdentity {
        id: "n1".into(),
        role,
        leader_url: None,
        leader_id: Some("leader".into()),
        cluster_key: None,
        board: None,
        board_rev: None,
        name: None,
    };
    let state = AppState(Arc::new(AppInner {
        config,
        events,
        store,
        identity: parking_lot::RwLock::new(identity),
        sessions: Default::default(),
        started: Instant::now(),
        services: Default::default(),
    }));
    let opts = EngineOptions {
        audio,
        output: Some(BackendKind::Sim),
        shm_dir: dir.join("shm"),
        realtime: false,
    };
    let engine = start_with(&state, opts).unwrap();
    Env { state, dir, engine }
}

async fn wait_for(timeout_ms: u64, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    cond()
}

fn uniform(bytes: &[u8]) -> Option<u8> {
    let first = *bytes.first()?;
    bytes.iter().all(|&b| b == first).then_some(first)
}

/// Plays a two-item playlist and checks pixels, timing and status.
async fn playlist_run(audio: bool) {
    let e = env(LocalRole::Leader, audio, |dir, show| {
        // s1: 40 frames × 25 ms = 1 s, frame f = f + 1; s2: 20 frames, 100 + f.
        let mut s1 = sequence(dir, "s1", 40, 25, |f| f as u8 + 1);
        let s2 = sequence(dir, "s2", 20, 25, |f| 100 + f as u8);
        // A linked song (exercises the audio path, or its fallback).
        crate::player::audio::write_test_wav(&dir.join("media/m1.wav"), 44_100, 1.0, 440.0, 0.3);
        show.media.push(Media {
            id: "m1".into(),
            name: "Song".into(),
            kind: MediaKind::Song,
            file: "media/m1.wav".into(),
            duration_ms: 1000,
            loudness_lufs: None,
            gain_db: None,
        });
        s1.media_id = Some("m1".into());
        show.sequences = vec![s1, s2];
        show.playlists = vec![playlist("p1", &["s1", "s2"], 0)];
    })
    .await;

    // Idle: dark.
    assert!(wait_for(2000, || e.engine.sim.as_ref().unwrap().frame_number() > 0).await);
    assert_eq!(uniform(&e.out(0)), Some(0));

    e.engine
        .handle
        .play(PlayRequest {
            playlist_id: Some("p1".into()),
            sequence_id: None,
            dj_clip_id: None,
            effect_id: None,
            media_id: None,
            start_index: None,
        })
        .await
        .unwrap();

    // Sample the output until the playlist is over.
    let t0 = Instant::now();
    let mut samples: Vec<(u64, u8, u8)> = vec![]; // (ms, out1 value, out2 value)
    let mut statuses = vec![];
    while t0.elapsed() < Duration::from_millis(2600) {
        let a = e.out(0);
        let b = e.out(1);
        assert_eq!(a.len(), 6, "output 1 carries prop A's 2 pixels");
        assert_eq!(b.len(), 9, "output 2 carries prop B's 3 pixels");
        let (va, vb) = (uniform(&a).expect("uniform"), uniform(&b).expect("uniform"));
        assert_eq!(va, vb, "both props show the same frame");
        samples.push((t0.elapsed().as_millis() as u64, va, vb));
        statuses.push(e.status());
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let first_s1 = samples.iter().find(|s| (1..=40).contains(&s.1)).expect("s1 played").0;
    let first_s2 = samples.iter().find(|s| s.1 >= 100).expect("s2 played").0;
    let gap = first_s2 - first_s1;
    assert!((850..=1250).contains(&gap), "s1 lasted {gap} ms (expected ~1000)");
    // Frames follow the clock: frame index ≈ elapsed / 25 ms.
    for &(t, v, _) in &samples {
        if (1..=40).contains(&v) && t > first_s1 + 50 && t < first_s2 {
            let expected = ((t - first_s1) / 25) as i64;
            assert!((v as i64 - 1 - expected).abs() <= 6, "at {t} ms frame {} (expected ~{expected})", v - 1);
        }
    }
    // s1 frames only move forward.
    let s1_vals: Vec<u8> = samples.iter().map(|s| s.1).filter(|v| (1..=40).contains(v)).collect();
    assert!(s1_vals.windows(2).all(|w| w[1] >= w[0]));
    assert!(*s1_vals.last().unwrap() >= 35, "reached the end of s1");
    // After the playlist: dark and idle.
    assert!(wait_for(1500, || uniform(&e.out(0)) == Some(0) && e.status().state == PlayerState::Idle).await);

    // Status progressed through both items with a playlist reference.
    let playing: Vec<_> = statuses.iter().filter(|s| s.state == PlayerState::Playing).collect();
    assert!(playing.iter().any(|s| s.item.as_ref().is_some_and(|i| i.id == "s1" && i.kind == "sequence")));
    assert!(playing.iter().any(|s| s.item.as_ref().is_some_and(|i| i.id == "s2")));
    let p = playing[0].playlist.as_ref().unwrap();
    assert_eq!((p.id.as_str(), p.count), ("p1", 2));
    assert!(playing.iter().any(|s| s.next_item.as_ref().is_some_and(|i| i.id == "s2")));
    let s1_pos: Vec<u64> = playing
        .iter()
        .filter(|s| s.item.as_ref().is_some_and(|i| i.id == "s1"))
        .map(|s| s.pos_ms)
        .collect();
    assert!(s1_pos.windows(2).all(|w| w[1] >= w[0]), "position never goes back");
    assert!(s1_pos.iter().any(|&p| p > 400), "position advanced: {s1_pos:?}");
    assert_eq!(playing[0].duration_ms, 1000);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plays_playlist_with_audio_disabled() {
    playlist_run(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plays_playlist_with_audio_or_fallback() {
    // With a sound card this plays the WAV and follows the audio clock; in
    // containers without one it falls back to the system clock seamlessly.
    playlist_run(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requests_play_next_then_playlist_resumes() {
    let e = env(LocalRole::Leader, false, |dir, show| {
        show.sequences = vec![
            sequence(dir, "s1", 24, 25, |_| 10),
            sequence(dir, "s2", 24, 25, |_| 20),
            sequence(dir, "s3", 24, 25, |_| 30),
        ];
        show.playlists = vec![playlist("p1", &["s1", "s2"], 0)];
    })
    .await;
    e.engine.handle.play(PlayRequest { playlist_id: Some("p1".into()), ..empty_req() }).await.unwrap();
    assert!(wait_for(2000, || uniform(&e.out(0)) == Some(10)).await);
    e.engine
        .handle
        .send(PlayerCmd::Enqueue { sequence_id: "s3".into(), name: Some("Ava".into()) })
        .await
        .unwrap();
    let mut order = vec![];
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(2500) {
        if let Some(v) = uniform(&e.out(0)) {
            if v != 0 && order.last() != Some(&v) {
                order.push(v);
            }
            if v == 30 {
                let st = e.status();
                if let Some(i) = st.item {
                    assert!(i.kind == "request" && i.id == "s3");
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(order, [10, 30, 20], "request plays after the current item, then the playlist resumes");
}

fn empty_req() -> PlayRequest {
    PlayRequest { playlist_id: None, sequence_id: None, dj_clip_id: None, effect_id: None, media_id: None, start_index: None }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crossfade_blends_items() {
    let e = env(LocalRole::Leader, false, |dir, show| {
        show.sequences = vec![sequence(dir, "s1", 40, 25, |_| 40), sequence(dir, "s2", 40, 25, |_| 200)];
        show.playlists = vec![playlist("p1", &["s1", "s2"], 400)];
    })
    .await;
    e.engine.handle.play(PlayRequest { playlist_id: Some("p1".into()), ..empty_req() }).await.unwrap();
    let mut seen = vec![];
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(2200) {
        if let Some(v) = uniform(&e.out(0)) {
            seen.push(v);
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(seen.contains(&40) && seen.contains(&200));
    let blended = seen.iter().filter(|&&v| v > 45 && v < 195).count();
    assert!(blended >= 3, "crossfade frames between the two looks: {seen:?}");
    // Monotonic rise through the blend.
    let mid: Vec<u8> = seen.iter().copied().filter(|&v| v > 40 && v < 200).collect();
    assert!(mid.windows(2).all(|w| w[1] + 8 >= w[0]), "{mid:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn brightness_blackout_fade_tests_and_overlays() {
    let e = env(LocalRole::Leader, false, |dir, show| {
        show.sequences = vec![sequence(dir, "s1", 400, 25, |_| 200)];
    })
    .await;
    let h = &e.engine.handle;
    h.play(PlayRequest { sequence_id: Some("s1".into()), ..empty_req() }).await.unwrap();
    assert!(wait_for(2000, || uniform(&e.out(0)) == Some(200)).await);
    h.send(PlayerCmd::SetBrightness(50)).await.unwrap();
    assert!(wait_for(1000, || uniform(&e.out(0)) == Some(100)).await, "master brightness halves");
    h.send(PlayerCmd::Blackout(true)).await.unwrap();
    assert!(wait_for(1000, || uniform(&e.out(0)) == Some(0) && e.status().blackout).await);
    h.send(PlayerCmd::Blackout(false)).await.unwrap();
    h.send(PlayerCmd::SetBrightness(100)).await.unwrap();
    assert!(wait_for(1000, || uniform(&e.out(0)) == Some(200)).await);

    // Test pattern on prop B only (solid red); A keeps the sequence.
    h.test_start(TestRequest {
        mode: "solid".into(),
        color: Some("#ff0000".into()),
        speed: None,
        target: TestTarget { props: Target { prop_ids: vec!["b".into()], ..Default::default() }, ..Default::default() },
        effect: None,
    })
    .await
    .unwrap();
    assert!(wait_for(1000, || e.out(1) == [255, 0, 0, 255, 0, 0, 255, 0, 0]).await);
    assert_eq!(uniform(&e.out(0)), Some(200));
    assert!(wait_for(1000, || e.status().state == PlayerState::Testing).await);
    // Bad test requests are rejected.
    assert!(h.test_start(TestRequest { mode: "bogus".into(), ..solid_req() }).await.is_err());
    h.send(PlayerCmd::TestStop).await.unwrap();
    assert!(wait_for(1000, || uniform(&e.out(1)) == Some(200)).await);

    // Overlay on prop A: explicit enable + prop pixels.
    let info = h.overlay_open("a".into()).await.unwrap();
    assert!(info.shm.ends_with("pixelplus-overlay-a"));
    assert_eq!((info.width, info.height), (2, 1));
    h.send(PlayerCmd::Overlay(OverlayCmd::Enable { prop_id: "a".into(), enabled: true })).await.unwrap();
    h.send(PlayerCmd::Overlay(OverlayCmd::PropPixels { prop_id: "a".into(), rgb: vec![1, 2, 3, 4, 5, 6] })).await.unwrap();
    assert!(wait_for(1000, || e.out(0) == [1, 2, 3, 4, 5, 6]).await);
    // Shared memory: the writer sets bit 0; the engine copies the frame.
    {
        use std::os::unix::fs::FileExt;
        let f = std::fs::OpenOptions::new().read(true).write(true).open(&info.shm).unwrap();
        f.write_all_at(&[9, 9, 9, 7, 7, 7], 12).unwrap();
        f.write_all_at(&1u32.to_ne_bytes(), 8).unwrap();
    }
    assert!(wait_for(1000, || e.out(0) == [9, 9, 9, 7, 7, 7]).await);
    h.send(PlayerCmd::Overlay(OverlayCmd::Enable { prop_id: "a".into(), enabled: false })).await.unwrap();
    assert!(wait_for(1000, || uniform(&e.out(0)) == Some(200)).await);
    assert!(h.overlay_open("nope".into()).await.is_err());

    // Stop with fade: dims over ~1 s, then idle and dark.
    let t0 = Instant::now();
    h.send(PlayerCmd::Stop { fade: true }).await.unwrap();
    assert!(wait_for(600, || uniform(&e.out(0)).is_some_and(|v| v > 20 && v < 180)).await, "fading");
    assert!(wait_for(2000, || uniform(&e.out(0)) == Some(0) && e.status().state == PlayerState::Idle).await);
    assert!(t0.elapsed() >= Duration::from_millis(800), "the fade took about a second");
}

fn solid_req() -> TestRequest {
    TestRequest { mode: "solid".into(), color: None, speed: None, target: TestTarget::default(), effect: None }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_and_corrupt_items_are_skipped() {
    let e = env(LocalRole::Leader, false, |dir, show| {
        let good = sequence(dir, "good", 20, 25, |_| 77);
        let mut missing = sequence(dir, "missing", 20, 25, |_| 1);
        missing.file = "sequences/not-there.fseq".into();
        let corrupt = sequence(dir, "corrupt", 20, 25, |_| 2);
        std::fs::write(dir.join("sequences/corrupt.fseq"), b"garbage garbage garbage").unwrap();
        show.sequences = vec![missing, corrupt, good];
        show.playlists = vec![playlist("p1", &["missing", "corrupt", "good"], 0)];
    })
    .await;
    let mut events = e.state.events.subscribe();
    e.engine.handle.play(PlayRequest { playlist_id: Some("p1".into()), ..empty_req() }).await.unwrap();
    assert!(wait_for(3000, || uniform(&e.out(0)) == Some(77)).await, "the good item plays");
    let mut warnings = 0;
    while let Ok(ev) = events.try_recv() {
        if let crate::events::Event::Json { kind: "log", data } = ev {
            if data["message"].as_str().unwrap_or("").starts_with("Skipped") {
                warnings += 1;
            }
        }
    }
    assert!(warnings >= 2, "a warning per skipped item");
    // A playlist that is entirely unplayable stops instead of spinning.
    assert!(e.engine.handle.play(PlayRequest { sequence_id: Some("missing".into()), ..empty_req() }).await.is_err());
    assert!(wait_for(1500, || e.status().state == PlayerState::Idle).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn effect_look_and_pause_resume() {
    let e = env(LocalRole::Leader, false, |dir, show| {
        show.sequences = vec![sequence(dir, "s1", 400, 25, |f| (f % 200) as u8 + 1)];
        show.effects.push(EffectPreset {
            id: "green".into(),
            name: "Green".into(),
            effect: EffectKind::Solid,
            params: [("color".to_string(), serde_json::json!("#00ff00"))].into_iter().collect(),
            target: Target { all: true, ..Default::default() },
        });
    })
    .await;
    let h = &e.engine.handle;
    h.play(PlayRequest { effect_id: Some("green".into()), ..empty_req() }).await.unwrap();
    assert!(wait_for(1500, || {
        let o = e.out(0);
        o.len() == 6 && o[1] > 0 && o[0] == 0 && o[2] == 0
    })
    .await);
    assert!(wait_for(1000, || e.status().state == PlayerState::Effect).await);

    h.play(PlayRequest { sequence_id: Some("s1".into()), ..empty_req() }).await.unwrap();
    assert!(wait_for(1500, || uniform(&e.out(0)).is_some_and(|v| v > 3)).await);
    h.send(PlayerCmd::Pause).await.unwrap();
    assert!(wait_for(1000, || e.status().state == PlayerState::Paused).await);
    let held = uniform(&e.out(0));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(uniform(&e.out(0)), held, "paused: frame holds");
    h.send(PlayerCmd::Resume).await.unwrap();
    assert!(wait_for(1000, || uniform(&e.out(0)) != held).await, "resumed");
    h.send(PlayerCmd::Seek(5000)).await.unwrap();
    // Frame 200 → value (200 % 200) + 1 = 1, then counting up again.
    assert!(wait_for(1000, || uniform(&e.out(0)).is_some_and(|v| v < 20)).await, "seeked");
    assert!(wait_for(1000, || e.status().pos_ms >= 5000).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn follower_plays_slices_from_sync() {
    let e = env(LocalRole::Follower, false, |dir, show| {
        let seq = sequence(dir, "s1", 400, 25, |f| (f / 4) as u8);
        // Build this node's slice like the leader would.
        let map = NodeMap::build(show, "n1").unwrap();
        pixelplus_core::ppseq::write_slice_from_path(dir.join("sequences/s1.fseq"), &map, dir.join("sequences/s1.ppseq"))
            .unwrap();
        std::fs::remove_file(dir.join("sequences/s1.fseq")).unwrap();
        show.sequences = vec![Sequence { file: "sequences/s1.ppseq".into(), ..seq }];
    })
    .await;
    let h = &e.engine.handle;
    let now_ms = || e.state.started.elapsed().as_millis() as u64;
    let packet = |pos: u64| SyncPacket {
        leader: "leader".into(),
        show_version: 1,
        state: PlayerState::Playing,
        item: Some(ItemRef { kind: "sequence".into(), id: "s1".into(), name: "Song".into() }),
        pos_ms: pos,
        sent_at_ms: now_ms(),
        effect: None,
        test: None,
        brightness: 100,
        blackout: false,
    };
    // Followers refuse local playback.
    assert!(h.play(PlayRequest { sequence_id: Some("s1".into()), ..empty_req() }).await.is_err());

    h.send(PlayerCmd::Sync(packet(4000))).await.unwrap();
    // Frame 160 → value 40.
    assert!(wait_for(2000, || uniform(&e.out(0)).is_some_and(|v| (40..=44).contains(&v))).await);
    // Jump (> 250 ms).
    h.send(PlayerCmd::Sync(packet(8000))).await.unwrap();
    assert!(wait_for(1000, || uniform(&e.out(0)).is_some_and(|v| (80..=84).contains(&v))).await);
    assert!(wait_for(1000, || e.status().item.is_some_and(|i| i.id == "s1")).await);
    // Leader releases us: dark.
    h.send(PlayerCmd::Sync(SyncPacket { leader: String::new(), state: PlayerState::Idle, item: None, ..packet(0) }))
        .await
        .unwrap();
    assert!(wait_for(1000, || uniform(&e.out(0)) == Some(0)).await);
    assert!(wait_for(2500, || e.status().state == PlayerState::Idle).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preview_frames_cover_all_props() {
    use std::sync::atomic::Ordering;
    let e = env(LocalRole::Leader, false, |dir, show| {
        show.sequences = vec![sequence(dir, "s1", 400, 25, |_| 50)];
        // A prop on another node: rendered from the full fseq in the preview.
        let mut remote = prop("c", 1, 3, 1);
        remote.segments[0].node_id = "other".into();
        show.props.push(remote);
    })
    .await;
    let mut rx = e.state.events.subscribe();
    crate::api::ws::PREVIEW_SUBSCRIBERS.fetch_add(1, Ordering::Relaxed);
    e.engine.handle.play(PlayRequest { sequence_id: Some("s1".into()), ..empty_req() }).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut found = None;
    while Instant::now() < deadline && found.is_none() {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Ok(crate::events::Event::Preview(b))) if b.len() == 5 + 18 && b[5] == 50 => found = Some(b),
            _ => {}
        }
    }
    crate::api::ws::PREVIEW_SUBSCRIBERS.fetch_sub(1, Ordering::Relaxed);
    let b = found.expect("preview frame");
    assert_eq!(b[0], 0x50);
    // A (6) + B (9) + C (3 bytes), all from the sequence.
    assert!(b[5..].iter().all(|&v| v == 50));
}
