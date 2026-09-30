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

pub(super) struct Env {
    pub(super) state: AppState,
    pub(super) dir: PathBuf,
    pub(super) engine: Engine,
}

impl Drop for Env {
    fn drop(&mut self) {
        self.engine.shutdown();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Env {
    pub(super) fn out(&self, i: usize) -> Vec<u8> {
        let sim = self.engine.sim.as_ref().expect("sim output");
        sim.with_latest(|s| s.outputs.get(i).cloned().unwrap_or_default())
    }

    /// Outputs `i` and `j` read from the same frame (one lock), so a frame
    /// arriving between two separate reads can't tear the comparison.
    pub(super) fn out_pair(&self, i: usize, j: usize) -> (Vec<u8>, Vec<u8>) {
        let sim = self.engine.sim.as_ref().expect("sim output");
        sim.with_latest(|s| {
            (
                s.outputs.get(i).cloned().unwrap_or_default(),
                s.outputs.get(j).cloned().unwrap_or_default(),
            )
        })
    }

    pub(super) fn status(&self) -> PlayerStatus {
        self.engine.handle.status()
    }
}

pub(super) fn prop(id: &str, n: u32, chan: u32, output: u32) -> Prop {
    Prop {
        suspect_pixels: Default::default(),
        id: id.into(),
        name: id.to_uppercase(),
        kind: PropKind::Line,
        pixel_count: n,
        xlights_model: None,
        channel_start: chan,
        channels_per_pixel: 3,
        channel_runs: None,
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
pub(super) fn write_fseq(path: &Path, frames: u32, frame_ms: u8, value: impl Fn(u32) -> u8) {
    let mut w = FseqWriter::create(path, FseqWriterOptions::new(15, frame_ms)).unwrap();
    for f in 0..frames {
        w.write_frame(&[value(f); 15]).unwrap();
    }
    w.finish().unwrap();
}

pub(super) fn sequence(
    dir: &Path,
    id: &str,
    frames: u32,
    frame_ms: u8,
    value: impl Fn(u32) -> u8,
) -> Sequence {
    write_fseq(
        &dir.join(format!("sequences/{id}.fseq")),
        frames,
        frame_ms,
        value,
    );
    Sequence {
        generated: Default::default(),
        tags: Default::default(),
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

pub(super) fn base_show() -> Show {
    let mut show = Show::default();
    show.nodes.push(Node {
        hardware_history: Default::default(),
        serial: Default::default(),
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

pub(super) fn playlist(id: &str, items: &[&str], crossfade_ms: u32) -> Playlist {
    Playlist {
        smart: Default::default(),
        id: id.into(),
        name: "Tonight".into(),
        items: items
            .iter()
            .map(|s| PlaylistItem::Sequence {
                id: format!("i-{s}"),
                sequence_id: s.to_string(),
            })
            .collect(),
        intro: vec![],
        outro: vec![],
        shuffle: false,
        repeat: false,
        crossfade_ms,
    }
}

pub(super) async fn env(role: LocalRole, audio: bool, setup: impl FnOnce(&Path, &mut Show)) -> Env {
    env_with(role, audio, None, setup).await
}

pub(super) async fn env_with(
    role: LocalRole,
    audio: bool,
    sim_refresh_hz: Option<f64>,
    setup: impl FnOnce(&Path, &mut Show),
) -> Env {
    let dir = std::env::temp_dir().join(format!("pp-engine-{}", new_id()));
    let config = Config {
        https_port: Default::default(),
        public_port: Default::default(),
        sensor_port: Default::default(),
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
        sim_refresh_hz,
    };
    let engine = start_with(&state, opts).unwrap();
    Env { state, dir, engine }
}

pub(super) async fn wait_for(timeout_ms: u64, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    cond()
}

pub(super) fn uniform(bytes: &[u8]) -> Option<u8> {
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
            tags: Default::default(),
            analysis: Default::default(),
            original_name: Default::default(),
            original_size: Default::default(),
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
            loop_until_stopped: Default::default(),
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
        let (a, b) = e.out_pair(0, 1);
        assert_eq!(a.len(), 6, "output 1 carries prop A's 2 pixels");
        assert_eq!(b.len(), 9, "output 2 carries prop B's 3 pixels");
        let (va, vb) = (uniform(&a).expect("uniform"), uniform(&b).expect("uniform"));
        assert_eq!(va, vb, "both props show the same frame");
        samples.push((t0.elapsed().as_millis() as u64, va, vb));
        statuses.push(e.status());
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let first_s1 = samples
        .iter()
        .find(|s| (1..=40).contains(&s.1))
        .expect("s1 played")
        .0;
    let first_s2 = samples.iter().find(|s| s.1 >= 100).expect("s2 played").0;
    let gap = first_s2 - first_s1;
    assert!(
        (850..=1250).contains(&gap),
        "s1 lasted {gap} ms (expected ~1000)"
    );
    // Frames follow the clock: frame index ≈ elapsed / 25 ms.
    for &(t, v, _) in &samples {
        if (1..=40).contains(&v) && t > first_s1 + 50 && t < first_s2 {
            let expected = ((t - first_s1) / 25) as i64;
            assert!(
                (v as i64 - 1 - expected).abs() <= 6,
                "at {t} ms frame {} (expected ~{expected})",
                v - 1
            );
        }
    }
    // s1 frames only move forward.
    let s1_vals: Vec<u8> = samples
        .iter()
        .map(|s| s.1)
        .filter(|v| (1..=40).contains(v))
        .collect();
    assert!(s1_vals.windows(2).all(|w| w[1] >= w[0]));
    assert!(*s1_vals.last().unwrap() >= 35, "reached the end of s1");
    // After the playlist: dark and idle.
    assert!(
        wait_for(1500, || uniform(&e.out(0)) == Some(0)
            && e.status().state == PlayerState::Idle)
        .await
    );

    // Status progressed through both items with a playlist reference.
    let playing: Vec<_> = statuses
        .iter()
        .filter(|s| s.state == PlayerState::Playing)
        .collect();
    assert!(playing.iter().any(|s| s
        .item
        .as_ref()
        .is_some_and(|i| i.id == "s1" && i.kind == "sequence")));
    assert!(playing
        .iter()
        .any(|s| s.item.as_ref().is_some_and(|i| i.id == "s2")));
    let p = playing[0].playlist.as_ref().unwrap();
    assert_eq!((p.id.as_str(), p.count), ("p1", 2));
    assert!(playing
        .iter()
        .any(|s| s.next_item.as_ref().is_some_and(|i| i.id == "s2")));
    let s1_pos: Vec<u64> = playing
        .iter()
        .filter(|s| s.item.as_ref().is_some_and(|i| i.id == "s1"))
        .map(|s| s.pos_ms)
        .collect();
    assert!(
        s1_pos.windows(2).all(|w| w[1] >= w[0]),
        "position never goes back"
    );
    assert!(
        s1_pos.iter().any(|&p| p > 400),
        "position advanced: {s1_pos:?}"
    );
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
    e.engine
        .handle
        .play(PlayRequest {
            playlist_id: Some("p1".into()),
            ..empty_req()
        })
        .await
        .unwrap();
    assert!(wait_for(2000, || uniform(&e.out(0)) == Some(10)).await);
    e.engine
        .handle
        .send(PlayerCmd::Enqueue {
            sequence_id: "s3".into(),
            name: Some("Ava".into()),
        })
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
    assert_eq!(
        order,
        [10, 30, 20],
        "request plays after the current item, then the playlist resumes"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requests_never_start_the_music_outside_show_windows() {
    let e = env(LocalRole::Leader, false, |dir, show| {
        show.sequences = vec![sequence(dir, "s3", 24, 25, |_| 30)];
        show.playlists = vec![playlist("p1", &["s3"], 0)];
        // A schedule whose only window is not now.
        show.schedule.enabled = true;
        show.schedule.entries.push(ScheduleEntry {
            start_exact: Default::default(),
            id: "e".into(),
            name: "Christmas Eve".into(),
            enabled: true,
            playlist_id: "p1".into(),
            days: vec![Weekday::Mon],
            date_range: Some(DateRange {
                start: "12-24".into(),
                end: "12-24".into(),
            }),
            start: TimeSpec::Clock {
                time: "18:00".into(),
            },
            end: TimeSpec::Clock {
                time: "18:01".into(),
            },
            priority: 0,
            end_behavior: EndBehavior::FinishSong,
        });
    })
    .await;
    // Let the schedule facts reach the engine (every second).
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let enqueue = || PlayerCmd::Enqueue {
        sequence_id: "s3".into(),
        name: None,
    };
    e.engine.handle.send(enqueue()).await.unwrap();
    assert!(
        !wait_for(800, || e.status().state == PlayerState::Playing).await,
        "a request outside the show must not start playback"
    );
    // Schedule off (the owner runs the show by hand): requests play.
    e.state
        .store
        .update(|s| {
            s.schedule.enabled = false;
            Ok(())
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    e.engine.handle.send(enqueue()).await.unwrap();
    assert!(wait_for(2000, || e.status().state == PlayerState::Playing).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn brightness_volume_and_lights_off_survive_a_restart() {
    let e = env(LocalRole::Leader, false, |_, _| {}).await;
    for cmd in [
        PlayerCmd::SetBrightness(40),
        PlayerCmd::SetVolume(33),
        PlayerCmd::Blackout(true),
    ] {
        e.engine.handle.send(cmd).await.unwrap();
    }
    let dir = e.dir.clone();
    let want = engine::SavedLevels {
        brightness: 40,
        volume: 33,
        blackout: true,
    };
    assert!(wait_for(3000, || engine::SavedLevels::load(&dir) == Some(want)).await);
    // The daemon restarts (power cut, update): same levels, not 100 %.
    let again = start_with(
        &e.state,
        EngineOptions {
            audio: false,
            output: Some(BackendKind::Sim),
            shm_dir: dir.join("shm2"),
            realtime: false,
            sim_refresh_hz: None,
        },
    )
    .unwrap();
    assert!(
        wait_for(2000, || {
            let st = again.handle.status();
            st.brightness == 40 && st.volume == 33 && st.blackout
        })
        .await,
        "{:?}",
        again.handle.status()
    );
    again.shutdown();
}

/// A follower adopted (or given a re-uploaded sequence) while the leader plays:
/// its slice arrives in the middle of the song. It must light up then, not
/// stay dark until the leader moves to another item.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn follower_picks_up_a_slice_that_arrives_mid_song() {
    let slice = std::sync::Arc::new(parking_lot::Mutex::new(None::<PathBuf>));
    let keep = slice.clone();
    let e = env(LocalRole::Follower, false, move |dir, show| {
        let seq = sequence(dir, "s1", 400, 25, |f| (f / 4) as u8);
        let map = NodeMap::build(show, "n1").unwrap();
        // Built aside: "not downloaded yet".
        let aside = dir.join("s1.ppseq.download");
        pixelplus_core::ppseq::write_slice_from_path(dir.join("sequences/s1.fseq"), &map, &aside)
            .unwrap();
        std::fs::remove_file(dir.join("sequences/s1.fseq")).unwrap();
        *keep.lock() = Some(aside);
        show.sequences = vec![Sequence {
            file: "sequences/s1.ppseq".into(),
            ..seq
        }];
    })
    .await;
    let h = &e.engine.handle;
    let now_ms = || e.state.started.elapsed().as_millis() as u64;
    let packet = |pos: u64| SyncPacket {
        surprise: Default::default(),
        leader: "leader".into(),
        show_version: 1,
        state: PlayerState::Playing,
        item: Some(ItemRef {
            kind: "sequence".into(),
            id: "s1".into(),
            name: "Song".into(),
        }),
        pos_ms: pos,
        sent_at_ms: now_ms(),
        anchor: None,
        effect: None,
        test: None,
        brightness: 100,
        blackout: false,
    };
    h.send(PlayerCmd::Sync(packet(2000))).await.unwrap();
    assert!(
        wait_for(1000, || e
            .status()
            .error
            .is_some_and(|m| m.contains("not downloaded")))
        .await
    );
    // The download finishes (renamed into place like `download_slice` does).
    let aside = slice.lock().clone().unwrap();
    std::fs::rename(&aside, e.dir.join("sequences/s1.ppseq")).unwrap();
    // Sync packets keep coming for the same song.
    let t0 = Instant::now();
    let mut lit = false;
    while t0.elapsed() < Duration::from_millis(3000) && !lit {
        let pos = 4000 + t0.elapsed().as_millis() as u64;
        h.send(PlayerCmd::Sync(packet(pos))).await.unwrap();
        tokio::time::sleep(Duration::from_millis(250)).await;
        lit = uniform(&e.out(0)).is_some_and(|v| v >= 40);
    }
    assert!(lit, "the follower lights up once its slice is there");
    assert!(e.status().error.is_none(), "{:?}", e.status().error);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scheduled_show_ends_a_forgotten_test_pattern() {
    let e = env(LocalRole::Leader, false, |dir, show| {
        show.sequences = vec![sequence(dir, "s1", 400, 25, |_| 10)];
        show.playlists = vec![playlist("p1", &["s1"], 0)];
        show.schedule.location.timezone = "UTC".into();
        show.schedule.entries.push(ScheduleEntry {
            start_exact: Default::default(),
            id: "e".into(),
            name: "All day".into(),
            enabled: true,
            playlist_id: "p1".into(),
            days: vec![
                Weekday::Mon,
                Weekday::Tue,
                Weekday::Wed,
                Weekday::Thu,
                Weekday::Fri,
                Weekday::Sat,
                Weekday::Sun,
            ],
            date_range: None,
            start: TimeSpec::Clock {
                time: "00:00".into(),
            },
            end: TimeSpec::Clock {
                time: "00:00".into(),
            },
            priority: 0,
            end_behavior: EndBehavior::FinishSong,
        });
        // Turned on later, after the afternoon test.
        show.schedule.enabled = false;
    })
    .await;
    let mut white = solid_req();
    white.target.props.all = true;
    white.color = Some("#ffffff".into());
    e.engine.handle.test_start(white).await.unwrap();
    assert!(wait_for(1000, || e.status().state == PlayerState::Testing).await);
    assert!(wait_for(1000, || uniform(&e.out(0)) == Some(255)).await);
    e.state
        .store
        .update(|s| {
            s.schedule.enabled = true;
            Ok(())
        })
        .await
        .unwrap();
    assert!(
        wait_for(3000, || e.status().state == PlayerState::Playing).await,
        "{:?}",
        e.status().state
    );
    // The sequence shows, not the solid test colour.
    assert!(wait_for(1000, || uniform(&e.out(0)) == Some(10)).await);
}

fn empty_req() -> PlayRequest {
    PlayRequest::default()
}

/// Manual play outside show windows plays a playlist once (its `repeat` is
/// ignored); "Loop until I stop" repeats it (scheduler.rs rules).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_playlist_plays_once_unless_loop_until_stopped() {
    let e = env(LocalRole::Leader, false, |dir, show| {
        show.sequences = vec![sequence(dir, "s1", 8, 25, |_| 40)];
        let mut pl = playlist("p1", &["s1"], 0);
        pl.repeat = true;
        show.playlists = vec![pl];
    })
    .await;
    let play = |looping: bool| PlayRequest {
        playlist_id: Some("p1".into()),
        loop_until_stopped: looping,
        ..PlayRequest::default()
    };
    e.engine.handle.play(play(false)).await.unwrap();
    assert!(wait_for(1000, || e.status().state == PlayerState::Playing).await);
    assert!(
        wait_for(3000, || e.status().state == PlayerState::Idle).await,
        "a repeating playlist played by hand outside a window plays once"
    );
    e.engine.handle.play(play(true)).await.unwrap();
    assert!(wait_for(1000, || e.status().state == PlayerState::Playing).await);
    // Eight times the 200 ms song: still going.
    tokio::time::sleep(Duration::from_millis(1600)).await;
    assert_eq!(e.status().state, PlayerState::Playing);
    e.engine
        .handle
        .send(PlayerCmd::Stop { fade: false })
        .await
        .unwrap();
    assert!(wait_for(2000, || e.status().state == PlayerState::Idle).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crossfade_blends_items() {
    let e = env(LocalRole::Leader, false, |dir, show| {
        show.sequences = vec![
            sequence(dir, "s1", 40, 25, |_| 40),
            sequence(dir, "s2", 40, 25, |_| 200),
        ];
        show.playlists = vec![playlist("p1", &["s1", "s2"], 400)];
    })
    .await;
    e.engine
        .handle
        .play(PlayRequest {
            playlist_id: Some("p1".into()),
            ..empty_req()
        })
        .await
        .unwrap();
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
    assert!(
        blended >= 3,
        "crossfade frames between the two looks: {seen:?}"
    );
    // Monotonic rise through the blend.
    let mid: Vec<u8> = seen
        .iter()
        .copied()
        .filter(|&v| v > 40 && v < 200)
        .collect();
    assert!(mid.windows(2).all(|w| w[1] + 8 >= w[0]), "{mid:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn brightness_blackout_fade_tests_and_overlays() {
    let e = env(LocalRole::Leader, false, |dir, show| {
        show.sequences = vec![sequence(dir, "s1", 400, 25, |_| 200)];
    })
    .await;
    let h = &e.engine.handle;
    h.play(PlayRequest {
        sequence_id: Some("s1".into()),
        ..empty_req()
    })
    .await
    .unwrap();
    assert!(wait_for(2000, || uniform(&e.out(0)) == Some(200)).await);
    h.send(PlayerCmd::SetBrightness(50)).await.unwrap();
    assert!(
        wait_for(1000, || uniform(&e.out(0)) == Some(100)).await,
        "master brightness halves"
    );
    h.send(PlayerCmd::Blackout(true)).await.unwrap();
    assert!(
        wait_for(1000, || uniform(&e.out(0)) == Some(0)
            && e.status().blackout)
        .await
    );
    h.send(PlayerCmd::Blackout(false)).await.unwrap();
    h.send(PlayerCmd::SetBrightness(100)).await.unwrap();
    assert!(wait_for(1000, || uniform(&e.out(0)) == Some(200)).await);

    // Test pattern on prop B only (solid red); A keeps the sequence.
    h.test_start(TestRequest {
        map_run_id: Default::default(),
        cal: Default::default(),
        identify: Default::default(),
        map: Default::default(),
        mode: "solid".into(),
        color: Some("#ff0000".into()),
        speed: None,
        target: TestTarget {
            props: Target {
                prop_ids: vec!["b".into()],
                ..Default::default()
            },
            ..Default::default()
        },
        effect: None,
    })
    .await
    .unwrap();
    assert!(wait_for(1000, || e.out(1) == [255, 0, 0, 255, 0, 0, 255, 0, 0]).await);
    assert_eq!(uniform(&e.out(0)), Some(200));
    assert!(wait_for(1000, || e.status().state == PlayerState::Testing).await);
    // Bad test requests are rejected.
    assert!(h
        .test_start(TestRequest {
            mode: "bogus".into(),
            ..solid_req()
        })
        .await
        .is_err());
    h.send(PlayerCmd::TestStop).await.unwrap();
    assert!(wait_for(1000, || uniform(&e.out(1)) == Some(200)).await);

    // Overlay on prop A: explicit enable + prop pixels.
    let info = h.overlay_open("a".into()).await.unwrap();
    assert!(info.shm.ends_with("pixelplus-overlay-a"));
    assert_eq!((info.width, info.height), (2, 1));
    h.send(PlayerCmd::Overlay(OverlayCmd::Enable {
        prop_id: "a".into(),
        enabled: true,
    }))
    .await
    .unwrap();
    h.send(PlayerCmd::Overlay(OverlayCmd::PropPixels {
        prop_id: "a".into(),
        rgb: vec![1, 2, 3, 4, 5, 6],
    }))
    .await
    .unwrap();
    assert!(wait_for(1000, || e.out(0) == [1, 2, 3, 4, 5, 6]).await);
    // Shared memory: the writer sets bit 0; the engine copies the frame.
    {
        use std::os::unix::fs::FileExt;
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&info.shm)
            .unwrap();
        f.write_all_at(&[9, 9, 9, 7, 7, 7], 12).unwrap();
        f.write_all_at(&1u32.to_ne_bytes(), 8).unwrap();
    }
    assert!(wait_for(1000, || e.out(0) == [9, 9, 9, 7, 7, 7]).await);
    h.send(PlayerCmd::Overlay(OverlayCmd::Enable {
        prop_id: "a".into(),
        enabled: false,
    }))
    .await
    .unwrap();
    assert!(wait_for(1000, || uniform(&e.out(0)) == Some(200)).await);
    assert!(h.overlay_open("nope".into()).await.is_err());

    // Stop with fade: dims over ~1 s, then idle and dark.
    let t0 = Instant::now();
    h.send(PlayerCmd::Stop { fade: true }).await.unwrap();
    assert!(
        wait_for(600, || uniform(&e.out(0))
            .is_some_and(|v| v > 20 && v < 180))
        .await,
        "fading"
    );
    assert!(
        wait_for(2000, || uniform(&e.out(0)) == Some(0)
            && e.status().state == PlayerState::Idle)
        .await
    );
    assert!(
        t0.elapsed() >= Duration::from_millis(800),
        "the fade took about a second"
    );
}

fn solid_req() -> TestRequest {
    TestRequest {
        map_run_id: Default::default(),
        cal: Default::default(),
        identify: Default::default(),
        map: Default::default(),
        mode: "solid".into(),
        color: None,
        speed: None,
        target: TestTarget::default(),
        effect: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_and_corrupt_items_are_skipped() {
    let e = env(LocalRole::Leader, false, |dir, show| {
        let good = sequence(dir, "good", 20, 25, |_| 77);
        let mut missing = sequence(dir, "missing", 20, 25, |_| 1);
        missing.file = "sequences/not-there.fseq".into();
        let corrupt = sequence(dir, "corrupt", 20, 25, |_| 2);
        std::fs::write(
            dir.join("sequences/corrupt.fseq"),
            b"garbage garbage garbage",
        )
        .unwrap();
        show.sequences = vec![missing, corrupt, good];
        show.playlists = vec![playlist("p1", &["missing", "corrupt", "good"], 0)];
    })
    .await;
    let mut events = e.state.events.subscribe();
    e.engine
        .handle
        .play(PlayRequest {
            playlist_id: Some("p1".into()),
            ..empty_req()
        })
        .await
        .unwrap();
    assert!(
        wait_for(3000, || uniform(&e.out(0)) == Some(77)).await,
        "the good item plays"
    );
    let mut warnings = 0;
    while let Ok(ev) = events.try_recv() {
        if let crate::events::Event::Json { kind: "log", data } = ev {
            if data["message"]
                .as_str()
                .unwrap_or("")
                .starts_with("Skipped")
            {
                warnings += 1;
            }
        }
    }
    assert!(warnings >= 2, "a warning per skipped item");
    // A playlist that is entirely unplayable stops instead of spinning.
    assert!(e
        .engine
        .handle
        .play(PlayRequest {
            sequence_id: Some("missing".into()),
            ..empty_req()
        })
        .await
        .is_err());
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
            params: [("color".to_string(), serde_json::json!("#00ff00"))]
                .into_iter()
                .collect(),
            target: Target {
                all: true,
                ..Default::default()
            },
        });
    })
    .await;
    let h = &e.engine.handle;
    h.play(PlayRequest {
        effect_id: Some("green".into()),
        ..empty_req()
    })
    .await
    .unwrap();
    assert!(
        wait_for(1500, || {
            let o = e.out(0);
            o.len() == 6 && o[1] > 0 && o[0] == 0 && o[2] == 0
        })
        .await
    );
    assert!(wait_for(1000, || e.status().state == PlayerState::Effect).await);

    h.play(PlayRequest {
        sequence_id: Some("s1".into()),
        ..empty_req()
    })
    .await
    .unwrap();
    assert!(wait_for(1500, || uniform(&e.out(0)).is_some_and(|v| v > 3)).await);
    h.send(PlayerCmd::Pause).await.unwrap();
    assert!(wait_for(1000, || e.status().state == PlayerState::Paused).await);
    let held = uniform(&e.out(0));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(uniform(&e.out(0)), held, "paused: frame holds");
    h.send(PlayerCmd::Resume).await.unwrap();
    assert!(
        wait_for(1000, || uniform(&e.out(0)) != held).await,
        "resumed"
    );
    h.send(PlayerCmd::Seek(5000)).await.unwrap();
    // Frame 200 → value (200 % 200) + 1 = 1, then counting up again.
    assert!(
        wait_for(1000, || uniform(&e.out(0)).is_some_and(|v| v < 20)).await,
        "seeked"
    );
    assert!(wait_for(1000, || e.status().pos_ms >= 5000).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn follower_plays_slices_from_sync() {
    let e = env(LocalRole::Follower, false, |dir, show| {
        let seq = sequence(dir, "s1", 400, 25, |f| (f / 4) as u8);
        // Build this node's slice like the leader would.
        let map = NodeMap::build(show, "n1").unwrap();
        pixelplus_core::ppseq::write_slice_from_path(
            dir.join("sequences/s1.fseq"),
            &map,
            dir.join("sequences/s1.ppseq"),
        )
        .unwrap();
        std::fs::remove_file(dir.join("sequences/s1.fseq")).unwrap();
        show.sequences = vec![Sequence {
            file: "sequences/s1.ppseq".into(),
            ..seq
        }];
    })
    .await;
    let h = &e.engine.handle;
    let now_ms = || e.state.started.elapsed().as_millis() as u64;
    let packet = |pos: u64| SyncPacket {
        surprise: Default::default(),
        leader: "leader".into(),
        show_version: 1,
        state: PlayerState::Playing,
        item: Some(ItemRef {
            kind: "sequence".into(),
            id: "s1".into(),
            name: "Song".into(),
        }),
        pos_ms: pos,
        sent_at_ms: now_ms(),
        anchor: None,
        effect: None,
        test: None,
        brightness: 100,
        blackout: false,
    };
    // Followers refuse local playback.
    assert!(h
        .play(PlayRequest {
            sequence_id: Some("s1".into()),
            ..empty_req()
        })
        .await
        .is_err());

    h.send(PlayerCmd::Sync(packet(4000))).await.unwrap();
    // Frame 160 → value 40.
    assert!(
        wait_for(2000, || uniform(&e.out(0))
            .is_some_and(|v| (40..=44).contains(&v)))
        .await
    );
    // Jump (> 250 ms).
    h.send(PlayerCmd::Sync(packet(8000))).await.unwrap();
    assert!(
        wait_for(1000, || uniform(&e.out(0))
            .is_some_and(|v| (80..=84).contains(&v)))
        .await
    );
    assert!(wait_for(1000, || e.status().item.is_some_and(|i| i.id == "s1")).await);
    // Leader releases us: dark.
    h.send(PlayerCmd::Sync(SyncPacket {
        leader: String::new(),
        state: PlayerState::Idle,
        item: None,
        ..packet(0)
    }))
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
    e.engine
        .handle
        .play(PlayRequest {
            sequence_id: Some("s1".into()),
            ..empty_req()
        })
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut found = None;
    while Instant::now() < deadline && found.is_none() {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Ok(crate::events::Event::Preview(b))) if b.len() == 5 + 18 && b[5] == 50 => {
                found = Some(b)
            }
            _ => {}
        }
    }
    crate::api::ws::PREVIEW_SUBSCRIBERS.fetch_sub(1, Ordering::Relaxed);
    let b = found.expect("preview frame");
    assert_eq!(b[0], 0x50);
    // A (6) + B (9) + C (3 bytes), all from the sequence.
    assert!(b[5..].iter().all(|&v| v == 50));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn play_ends_a_live_look() {
    let e = env(LocalRole::Leader, false, |dir, show| {
        show.sequences = vec![sequence(dir, "s1", 400, 25, |_| 77)];
    })
    .await;
    let h = &e.engine.handle;
    let look = EffectPreset {
        id: "live".into(),
        name: "Blue".into(),
        effect: EffectKind::Solid,
        params: [("color".to_string(), serde_json::json!("#0000ff"))]
            .into_iter()
            .collect(),
        target: Target {
            all: true,
            ..Default::default()
        },
    };
    h.test_start(TestRequest {
        map_run_id: Default::default(),
        cal: Default::default(),
        identify: Default::default(),
        map: Default::default(),
        mode: "effect".into(),
        color: None,
        speed: None,
        target: TestTarget {
            node_id: None,
            output: None,
            props: look.target.clone(),
        },
        effect: Some(look),
    })
    .await
    .unwrap();
    assert!(wait_for(1000, || e.status().state == PlayerState::Effect).await);
    h.play(PlayRequest {
        sequence_id: Some("s1".into()),
        ..empty_req()
    })
    .await
    .unwrap();
    assert!(
        wait_for(1500, || e.status().state == PlayerState::Playing).await,
        "{:?}",
        e.status().state
    );
    assert!(
        wait_for(1500, || uniform(&e.out(0)) == Some(77)).await,
        "the sequence shows, not the look"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn debug_tap_records_the_shown_sequence_frame() {
    let e = env(LocalRole::Leader, false, |dir, show| {
        show.sequences = vec![sequence(dir, "s1", 400, 25, |f| (f % 200) as u8 + 1)];
    })
    .await;
    e.engine
        .handle
        .play(PlayRequest {
            sequence_id: Some("s1".into()),
            ..empty_req()
        })
        .await
        .unwrap();
    let tap = e
        .state
        .services
        .debug_output
        .get()
        .expect("tap enabled in dev/sim")
        .clone();
    assert!(
        wait_for(1500, || tap
            .snapshot()
            .sequence
            .as_ref()
            .is_some_and(|(_, f)| *f > 2))
        .await
    );
    let t = tap.snapshot();
    let (id, frame) = t.sequence.clone().unwrap();
    assert_eq!(id, "s1");
    assert_eq!(t.pixels_per_output[..2], [2, 3]);
    // Prop A (output 1) shows the value of that frame (colour order RGB, full brightness).
    assert_eq!(t.rgb[0], (frame % 200) as u8 + 1);
    assert_eq!(t.wire[0][..6], t.rgb[..6]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn follower_leader_test_replaces_local_identify() {
    let e = env(LocalRole::Follower, false, |_, _| {}).await;
    let h = &e.engine.handle;
    // "Identify" runs as a local white chase on every output of this node.
    h.test_start(TestRequest {
        map_run_id: Default::default(),
        cal: Default::default(),
        identify: Default::default(),
        map: Default::default(),
        mode: "solid".into(),
        color: Some("#ffffff".into()),
        speed: None,
        target: TestTarget {
            node_id: Some("n1".into()),
            output: None,
            props: Default::default(),
        },
        effect: None,
    })
    .await
    .unwrap();
    assert!(wait_for(1000, || uniform(&e.out(0)) == Some(255)).await);
    // The leader starts a red test on all props (e.g. the fault finder or a test pattern).
    let red = TestRequest {
        map_run_id: Default::default(),
        cal: Default::default(),
        identify: Default::default(),
        map: Default::default(),
        mode: "solid".into(),
        color: Some("#ff0000".into()),
        speed: None,
        target: TestTarget {
            node_id: None,
            output: None,
            props: Target {
                all: true,
                ..Default::default()
            },
        },
        effect: None,
    };
    let now_ms = e.state.started.elapsed().as_millis() as u64;
    h.send(PlayerCmd::Sync(SyncPacket {
        surprise: Default::default(),
        leader: "leader".into(),
        show_version: 1,
        state: PlayerState::Testing,
        item: None,
        pos_ms: 0,
        sent_at_ms: now_ms,
        anchor: None,
        effect: None,
        test: Some(red),
        brightness: 100,
        blackout: false,
    }))
    .await
    .unwrap();
    assert!(
        wait_for(1000, || e.out(0).chunks(3).all(|p| p == [255, 0, 0])).await,
        "leader's test shows, not the local identify: {:?}",
        e.out(0)
    );
}

// ---------------------------------------------------------------------------
// Timing: presentation time, anchors, sound delay, calibration
// ---------------------------------------------------------------------------

/// Poll the output tap as fast as possible for `ms`, keeping every new frame.
async fn tap_frames(e: &Env, ms: u64) -> Vec<debugtap::TapFrame> {
    let tap = e.state.services.debug_output.get().expect("tap").clone();
    let mut out: Vec<debugtap::TapFrame> = Vec::new();
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(ms) {
        let f = tap.snapshot();
        if out.last().map_or(true, |l| l.frame_no != f.frame_no) {
            out.push(f);
        }
        tokio::time::sleep(Duration::from_micros(300)).await;
    }
    out
}

/// Frame changes land within ±R/2 of their ideal instant on a vblank grid,
/// with updates only at frame boundaries when the refresh is faster than
/// the sequence (R < F), and on every refresh when it is slower (R > F).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn frames_are_chosen_for_their_light_up_time_on_a_vblank_grid() {
    for (hz, frame_ms) in [(100.0, 25u8), (20.0, 25u8), (40.3, 50u8)] {
        let r = 1000.0 / hz;
        let e = env_with(LocalRole::Leader, false, Some(hz), |dir, show| {
            show.sequences = vec![sequence(dir, "s1", 400, frame_ms, |f| (f % 250) as u8)];
        })
        .await;
        e.engine
            .handle
            .play(PlayRequest {
                sequence_id: Some("s1".into()),
                ..empty_req()
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let frames = tap_frames(&e, 1500).await;
        let f = frame_ms as f64;
        // Light-up times sit on the vblank grid (plus a constant latch delay).
        let lights: Vec<f64> = frames.iter().map(|t| t.light_ms).collect();
        for w in lights.windows(2) {
            let n = (w[1] - w[0]) / r;
            assert!(
                (n - n.round()).abs() < 0.02 && n.round() >= 1.0,
                "{hz} Hz: gap {n} refreshes"
            );
        }
        let (mut checked, mut missed) = (0, 0);
        for w in frames.windows(2) {
            let (Some((_, a)), Some((_, b))) = (&w[0].sequence, &w[1].sequence) else {
                continue;
            };
            let pos = w[1].pos_ms.unwrap();
            // The shown frame covers the middle of its presentation slot.
            assert_eq!(*b, ((pos + r / 2.0) / f).floor() as u32, "{hz} Hz");
            if r < f && w[1].frame_no == w[0].frame_no + 1 && *b == a + 1 {
                // A frame change: within half a refresh of its ideal instant
                // (unless the loaded test machine made us miss a vblank:
                // then exactly one refresh later).
                let early = *b as f64 * f - pos;
                if early.abs() > r / 2.0 + 0.5 {
                    assert!(
                        early + r >= -r / 2.0 - 0.5,
                        "{hz} Hz: frame {b} {early:.2} ms off"
                    );
                    missed += 1;
                }
                checked += 1;
            }
        }
        if r < f {
            assert!(checked > 5, "{hz} Hz: only {checked} frame changes seen");
            assert!(
                missed * 5 <= checked,
                "{hz} Hz: {missed}/{checked} vblanks missed"
            );
        }
        let span = (lights.last().unwrap() - lights[0]) / 1000.0;
        let rate = (frames.len() - 1) as f64 / span;
        let expected = if r < f { 1000.0 / f } else { hz };
        assert!(
            (rate - expected).abs() < expected * 0.15,
            "{hz} Hz / {f} ms frames: {rate:.1} updates/s, expected {expected:.1}"
        );
    }
}

/// Followers follow the anchor continuously: an offset of 8 ms (well inside
/// the old ±1-frame deadband) and a 150 ppm rate difference are removed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn follower_follows_the_anchor_without_a_deadband() {
    let e = env(LocalRole::Follower, false, |dir, show| {
        show.sequences = vec![sequence(dir, "s1", 800, 25, |f| (f % 250) as u8)];
    })
    .await;
    let h = &e.engine.handle;
    let now_ms = || e.state.started.elapsed().as_secs_f64() * 1000.0;
    let packet = |a: Anchor| SyncPacket {
        surprise: Default::default(),
        leader: "leader".into(),
        show_version: 1,
        state: PlayerState::Playing,
        item: Some(ItemRef {
            kind: "sequence".into(),
            id: "s1".into(),
            name: "Song".into(),
        }),
        pos_ms: a.pos_ms as u64,
        sent_at_ms: now_ms() as u64,
        anchor: Some(a),
        effect: None,
        test: None,
        brightness: 100,
        blackout: false,
    };
    let start = now_ms();
    let first = Anchor {
        pos_ms: 2000.0,
        at_ms: start,
        rate: 1.0,
        epoch: 1,
    };
    h.send(PlayerCmd::Sync(packet(first))).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    // Same epoch, 8 ms ahead and 150 ppm fast: slew, no jump.
    let t = now_ms();
    let second = Anchor {
        pos_ms: first.pos_at(t) + 8.0,
        at_ms: t,
        rate: 1.00015,
        epoch: 1,
    };
    for _ in 0..16 {
        h.send(PlayerCmd::Sync(packet(second))).await.unwrap();
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let frames = tap_frames(&e, 500).await;
    let worst = frames
        .iter()
        .filter_map(|f| f.pos_ms.map(|p| (p - second.pos_at(f.light_ms)).abs()))
        .fold(0.0f64, f64::max);
    assert!(!frames.is_empty());
    assert!(worst < 0.5, "timeline error {worst} ms after 4 s");
    let err = e.status().sync_error_ms.expect("sync error reported");
    assert!(err < 1.0, "{err}");
}

/// `settings.audio.outputDelayMs` delays the lights (and the anchors sent to
/// followers) against the audio clock.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sound_delay_shifts_the_lights_timeline() {
    let e = env(LocalRole::Leader, false, |dir, show| {
        show.sequences = vec![sequence(dir, "s1", 800, 25, |f| (f % 250) as u8)];
        show.settings.audio.output_delay_ms = 300;
    })
    .await;
    e.engine
        .handle
        .play(PlayRequest {
            sequence_id: Some("s1".into()),
            ..empty_req()
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let tap = e.state.services.debug_output.get().unwrap().snapshot();
    let s = e.status();
    let a = s.anchor.expect("leader anchor");
    // Heard position ≈ at_ms-based; lights = heard − 300.
    let heard_at_light = s.pos_ms as f64 + (tap.light_ms - a.at_ms);
    let lights = tap.pos_ms.unwrap();
    assert!(
        (heard_at_light - lights - 300.0).abs() < 30.0,
        "heard {heard_at_light} vs lights {lights}"
    );
    assert!((a.pos_at(tap.light_ms) - lights).abs() < 1.0);
    // Changing the delay moves the timeline at once (a new epoch).
    let epoch = a.epoch;
    let mut show = (*e.state.store.get()).clone();
    show.settings.audio.output_delay_ms = -100;
    e.state.store.replace(show).await.unwrap();
    assert!(
        wait_for(1500, || e.status().anchor.is_some_and(
            |b| b.epoch != epoch && (b.pos_ms - (a.pos_at(b.at_ms) + 400.0)).abs() < 30.0
        ))
        .await
    );
}

/// "Sync lights to sound": every prop flashes white once a second.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn calibration_flashes_every_second() {
    let e = env(LocalRole::Leader, false, |_, _| {}).await;
    e.engine
        .handle
        .send(PlayerCmd::Calibrate(true))
        .await
        .unwrap();
    assert!(
        wait_for(1500, || e
            .status()
            .item
            .is_some_and(|i| i.kind == engine::CALIBRATION_ID))
        .await
    );
    // Within 2 s we see both a white flash and dark.
    let (mut white, mut dark) = (false, false);
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(2200) && !(white && dark) {
        match uniform(&e.out(0)) {
            Some(255) => white = true,
            Some(0) => dark = true,
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(white && dark, "white {white} dark {dark}");
    assert!(std::fs::metadata(e.dir.join("cache/calibration-click.wav")).is_ok());
    e.engine
        .handle
        .send(PlayerCmd::Calibrate(false))
        .await
        .unwrap();
    assert!(wait_for(1500, || e.status().state == PlayerState::Idle).await);
}

#[test]
fn click_track_is_a_valid_wav() {
    let wav = engine::click_wav(2_000);
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(&wav[8..16], b"WAVEfmt ");
    assert_eq!(wav.len(), 44 + 2 * 48_000);
    // Silence between clicks, a loud click at 1 s.
    let sample = |i: usize| i16::from_le_bytes([wav[44 + 2 * i], wav[45 + 2 * i]]);
    assert_eq!(sample(12_000), 0);
    assert!(
        (24_000..24_100)
            .map(sample)
            .map(i16::unsigned_abs)
            .max()
            .unwrap()
            > 20_000
    );
    assert!(engine::flash_on(1_010.0, 1.0) && !engine::flash_on(1_300.0, 1.0));
    assert!(!engine::flash_on(-5.0, 1.0));
}
