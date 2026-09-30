//! Engine integration tests for the feature wave (WS3): countdown (F4),
//! power limiter and dimming (F12), surprises (F20), timeline test modes
//! (F1 calibration v2, F6 mapping codes, F9 identify), the season prop mask
//! (F8), smart playlists (F18) and the journal (F11).

use super::tests::*;
use super::*;
use crate::node::LocalRole;
use pixelplus_core::effects::countdown;
use pixelplus_core::effects::EffectRenderer;
use pixelplus_core::mapcode::{MapPlan, MapTarget, PHASE_A, PHASE_B};
use pixelplus_core::model::*;
use std::time::{Duration, Instant};

fn solid(id: &str, color: &str, target: Target) -> EffectPreset {
    EffectPreset {
        id: id.into(),
        name: format!("Solid {color}"),
        effect: EffectKind::Solid,
        params: [("color".to_string(), serde_json::json!(color))]
            .into_iter()
            .collect(),
        target,
    }
}

fn all() -> Target {
    Target {
        all: true,
        ..Default::default()
    }
}

fn countdown_item(
    duration_ms: u64,
    others: CountdownOthers,
    finale: CountdownFinale,
) -> PlaylistItem {
    PlaylistItem::Countdown {
        id: "cd".into(),
        duration_ms,
        matrix_prop_id: None,
        text: "{s}".into(),
        color: None,
        others,
        finale,
        dj_clip_id: None,
        dj_offset_ms: 0,
        tick: false,
    }
}

fn now_ms(e: &Env) -> f64 {
    e.state.started.elapsed().as_secs_f64() * 1000.0
}

fn px(bytes: &[u8], i: usize) -> [u8; 3] {
    [bytes[i * 3], bytes[i * 3 + 1], bytes[i * 3 + 2]]
}

// ---------------------------------------------------------------------------
// F4 countdown
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn countdown_intro_fills_up_then_the_first_song_starts_at_zero() {
    let e = env(LocalRole::Leader, false, |dir, show| {
        show.sequences = vec![sequence(dir, "s1", 400, 25, |_| 10)];
        let mut pl = playlist("p1", &["s1"], 500);
        pl.intro = vec![countdown_item(
            2_000,
            CountdownOthers::Fill,
            CountdownFinale::None,
        )];
        show.playlists = vec![pl];
    })
    .await;
    e.engine
        .handle
        .play(PlayRequest {
            playlist_id: Some("p1".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        wait_for(4000, || e
            .status()
            .item
            .is_some_and(|i| i.kind == "countdown"))
        .await
    );
    let t0 = Instant::now();
    // The bar starts empty and fills up.
    assert!(
        wait_for(4000, || px(&e.out(0), 0) == [255, 255, 255]).await,
        "{:?}",
        e.out(0)
    );
    // On zero the song starts: no crossfade out of a countdown.
    assert!(
        wait_for(4000, || e
            .status()
            .item
            .is_some_and(|i| i.kind == "sequence"))
        .await
    );
    let took = t0.elapsed().as_millis() as i64;
    assert!(
        (1_300..=2_300).contains(&took),
        "the song started {took} ms into a 2 s countdown"
    );
    assert!(
        wait_for(3000, || uniform(&e.out(0)) == Some(10)).await,
        "{:?}",
        e.out(0)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn follower_renders_the_countdown_like_the_leader() {
    let e = env(LocalRole::Follower, false, |_, _| {}).await;
    let show = e.state.store.get();
    let preset = countdown::countdown_preset(
        &show,
        "cd",
        10_000,
        None,
        "{s}",
        Some("#00ff00"),
        CountdownOthers::Fill,
        CountdownFinale::Flash,
    );
    let pkt = SyncPacket {
        surprise: None,
        leader: "leader".into(),
        show_version: 1,
        state: PlayerState::Playing,
        item: Some(ItemRef {
            kind: "countdown".into(),
            id: "cd".into(),
            name: "Countdown".into(),
        }),
        pos_ms: 5_000,
        sent_at_ms: 0,
        anchor: Some(Anchor {
            pos_ms: 5_000.0,
            at_ms: now_ms(&e),
            rate: 0.0,
            epoch: 7,
        }),
        effect: Some(preset.clone()),
        test: None,
        brightness: 100,
        blackout: false,
    };
    e.engine.handle.send(PlayerCmd::Sync(pkt)).await.unwrap();
    // What the leader renders for prop "a" at 5 s (half full: pixel 0 on).
    let r = EffectRenderer::new(&preset, &[show.prop("a").unwrap()]);
    let mut want = vec![0u8; r.frame_len()];
    r.render(5_000, &mut want);
    assert_eq!(px(&want, 0), [0, 255, 0]);
    assert!(
        wait_for(4000, || e.out(0) == want).await,
        "{:?} vs {want:?}",
        e.out(0)
    );
}

// ---------------------------------------------------------------------------
// F12 power limiter and dimming
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn limiter_scales_a_supply_down_and_warn_mode_only_reports() {
    let e = env(LocalRole::Leader, false, |_, show| {
        show.effects = vec![solid("white", "#ffffff", all())];
        show.settings.power.mode = LimiterMode::Limit;
        // Prop "a" (2 px × 60 mA = 0.12 A at white) on a 0.1 A supply:
        // budget 0.09 A → scale 0.75.
        show.power_supplies = vec![PowerSupply {
            id: "psu".into(),
            name: "Tiny PSU".into(),
            volts: 12.0,
            amps: 0.1,
            receiver_ids: vec![],
            direct_outputs: vec![NodeOutputRef {
                node_id: "n1".into(),
                output: 1,
            }],
            sensor: None,
        }];
    })
    .await;
    e.engine
        .handle
        .play(PlayRequest {
            effect_id: Some("white".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        wait_for(4000, || uniform(&e.out(0))
            .is_some_and(|v| (186..=196).contains(&v)))
        .await,
        "{:?}",
        e.out(0)
    );
    assert_eq!(uniform(&e.out(1)), Some(255), "output 2 is on no supply");
    assert!(
        wait_for(4000, || e
            .status()
            .power
            .is_some_and(|p| p.limiting && p.min_scale < 0.8))
        .await
    );
    // Warn mode: full brightness, still reported.
    e.state
        .store
        .update(|s| {
            s.settings.power.mode = LimiterMode::Warn;
            Ok::<_, crate::api::ApiError>(())
        })
        .await
        .unwrap();
    assert!(
        wait_for(4000, || uniform(&e.out(0)) == Some(255)).await,
        "{:?}",
        e.out(0)
    );
    assert!(e.status().power.is_some_and(|p| p.limiting));
    // Off: no status.
    e.state
        .store
        .update(|s| {
            s.settings.power.mode = LimiterMode::Off;
            Ok::<_, crate::api::ApiError>(())
        })
        .await
        .unwrap();
    assert!(wait_for(4000, || e.status().power.is_none()).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_night_dimming_caps_the_master_brightness() {
    let e = env(LocalRole::Leader, false, |_, show| {
        show.effects = vec![solid("white", "#ffffff", all())];
        show.schedule.location.timezone = "UTC".into();
        show.settings.power.mode = LimiterMode::Off;
        show.settings.power.dim = vec![DimWindow {
            from: TimeSpec::Clock {
                time: "00:00".into(),
            },
            to: TimeSpec::Clock {
                time: "00:00".into(),
            },
            brightness: 40,
            days: vec![],
        }];
    })
    .await;
    e.engine
        .handle
        .play(PlayRequest {
            effect_id: Some("white".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        wait_for(3000, || uniform(&e.out(0)) == Some(102)).await,
        "{:?}",
        e.out(0)
    );
    let st = e.status();
    assert_eq!(st.brightness, 100, "the owner's brightness is unchanged");
    assert_eq!(
        st.light_brightness,
        Some(40),
        "followers get the dimmed one"
    );
}

// ---------------------------------------------------------------------------
// F20 surprises
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn surprise_layers_over_the_song_then_goes_away() {
    let e = env(LocalRole::Leader, false, |dir, show| {
        show.sequences = vec![sequence(dir, "s1", 800, 25, |_| 10)];
        show.effects = vec![solid("red", "#ff0000", all())];
    })
    .await;
    let h = &e.engine.handle;
    h.play(PlayRequest {
        sequence_id: Some("s1".into()),
        ..Default::default()
    })
    .await
    .unwrap();
    assert!(wait_for(4000, || uniform(&e.out(1)) == Some(10)).await);
    let started = h
        .surprise(surprise::SurpriseRequest {
            id: "t1".into(),
            kind: "effect".into(),
            r#ref: "red".into(),
            targets: vec!["b".into()],
            duration_ms: Some(1_200),
        })
        .await
        .unwrap();
    assert_eq!(
        (started.props, started.duration_ms, started.replaced),
        (1, 1_200, false)
    );
    assert!(
        wait_for(4000, || e.out_pair(0, 1)
            == (vec![10; 6], [255u8, 0, 0].repeat(3)))
        .await,
        "{:?}",
        e.out_pair(0, 1)
    );
    // The leader's sync packets carry it, relative to the timeline anchor.
    let st = e.status();
    let sa = st.surprise.clone().expect("surprise in status");
    assert_eq!((sa.kind.as_str(), sa.r#ref.as_str()), ("effect", "red"));
    assert!(
        sa.start_pos <= 0.0 && sa.start_pos > -1_500.0,
        "{}",
        sa.start_pos
    );
    assert!(st.anchor.is_some());
    assert_eq!(st.item.unwrap().kind, "sequence", "the song goes on");
    // It fades out and the song shows again.
    assert!(
        wait_for(4000, || uniform(&e.out(1)) == Some(10)).await,
        "{:?}",
        e.out(1)
    );
    assert!(e.status().surprise.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn surprises_wait_for_tests_and_a_new_one_replaces_the_old() {
    let e = env(LocalRole::Leader, false, |_, show| {
        show.effects = vec![
            solid("red", "#ff0000", all()),
            solid("blue", "#0000ff", all()),
        ];
    })
    .await;
    let h = &e.engine.handle;
    let req = |r: &str| surprise::SurpriseRequest {
        id: "t".into(),
        kind: "effect".into(),
        r#ref: r.into(),
        targets: vec![],
        duration_ms: Some(5_000),
    };
    // A running test pattern has priority.
    h.test_start(TestRequest {
        mode: "solid".into(),
        color: Some("#00ff00".into()),
        target: TestTarget {
            props: all(),
            ..Default::default()
        },
        ..Default::default()
    })
    .await
    .unwrap();
    let err = h.surprise(req("red")).await.unwrap_err();
    assert!(err.message.contains("test"), "{}", err.message);
    h.send(PlayerCmd::TestStop).await.unwrap();
    // Idle display: the surprise shows on dark props.
    assert!(!h.surprise(req("red")).await.unwrap().replaced);
    assert!(wait_for(4000, || px(&e.out(0), 0) == [255, 0, 0]).await);
    assert!(h.surprise(req("blue")).await.unwrap().replaced);
    assert!(wait_for(4000, || px(&e.out(0), 0) == [0, 0, 255]).await);
    h.send(PlayerCmd::SurpriseStop).await.unwrap();
    assert!(wait_for(4000, || uniform(&e.out(0)) == Some(0)).await);
    // Blackout refuses.
    h.send(PlayerCmd::Blackout(true)).await.unwrap();
    let err = h.surprise(req("red")).await.unwrap_err();
    assert!(err.message.contains("blackout"), "{}", err.message);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn follower_renders_the_surprise_from_sync() {
    let e = env(LocalRole::Follower, false, |_, show| {
        show.effects = vec![solid("red", "#ff0000", all())];
    })
    .await;
    let pkt = |surprise: Option<SurpriseAnchor>, at: f64| SyncPacket {
        surprise,
        leader: "leader".into(),
        show_version: 1,
        state: PlayerState::Idle,
        item: None,
        pos_ms: 0,
        sent_at_ms: 0,
        anchor: Some(Anchor {
            pos_ms: 0.0,
            at_ms: at,
            rate: 0.0,
            epoch: 1,
        }),
        effect: None,
        test: None,
        brightness: 100,
        blackout: false,
    };
    let sa = SurpriseAnchor {
        id: "t1".into(),
        kind: "effect".into(),
        r#ref: "red".into(),
        targets: vec!["b".into()],
        start_pos: -300.0,
        duration_ms: 5_000,
        epoch: 42,
    };
    let h = &e.engine.handle;
    h.send(PlayerCmd::Sync(pkt(Some(sa.clone()), now_ms(&e))))
        .await
        .unwrap();
    assert!(
        wait_for(4000, || e.out_pair(0, 1)
            == (vec![0; 6], [255u8, 0, 0].repeat(3)))
        .await,
        "{:?}",
        e.out_pair(0, 1)
    );
    // Gone from the packets: gone from the lights.
    h.send(PlayerCmd::Sync(pkt(None, now_ms(&e))))
        .await
        .unwrap();
    assert!(wait_for(4000, || uniform(&e.out(1)) == Some(0)).await);
    // A surprise that ended long ago on the leader's clock shows nothing.
    let mut old = sa;
    old.epoch = 43;
    old.start_pos = -60_000.0;
    h.send(PlayerCmd::Sync(pkt(Some(old), now_ms(&e))))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(uniform(&e.out(1)), Some(0));
}

// ---------------------------------------------------------------------------
// Timeline test modes: mapping codes (F6), identify (F9), calibration v2 (F1)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn map_code_test_lights_only_its_targets() {
    let e = env(LocalRole::Leader, false, |_, _| {}).await;
    let plan = MapPlan {
        seed: 1,
        bit_ms: 120,
        level: 77,
        passes: 1,
        phases: PHASE_A | PHASE_B,
        targets: vec![MapTarget {
            node_id: "n1".into(),
            output: 1,
            max_pixels: 2,
        }],
        pixel_bits: 1,
        ..Default::default()
    };
    e.engine
        .handle
        .test_start(TestRequest {
            mode: "mapCode".into(),
            map: Some(plan),
            map_run_id: Some("run1".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(wait_for(4000, || e.status().state == PlayerState::Testing).await);
    assert!(
        e.status().anchor.is_some(),
        "followers get the pattern's timeline"
    );
    let (mut on, mut off) = (false, false);
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(2500) && !(on && off) {
        let (a, b) = e.out_pair(0, 1);
        assert_eq!(uniform(&b), Some(0), "not a target");
        match uniform(&a) {
            Some(77) => on = true,
            Some(0) => off = true,
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(3)).await;
    }
    assert!(on && off, "on {on} off {off}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn identify_lights_the_chosen_output_in_its_colour() {
    let e = env(LocalRole::Leader, false, |_, _| {}).await;
    e.engine
        .handle
        .test_start(TestRequest {
            mode: "identify".into(),
            identify: Some(vec![IdentifyLight {
                node_id: "n1".into(),
                output: 2,
                color: "#ff0000".into(),
                blinks: 0,
            }]),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        wait_for(4000, || e.out_pair(0, 1)
            == (vec![0; 6], [255u8, 0, 0].repeat(3)))
        .await,
        "{:?}",
        e.out_pair(0, 1)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn phone_calibration_plays_the_seeded_pattern() {
    let e = env(LocalRole::Leader, false, |_, _| {}).await;
    e.engine
        .handle
        .send(PlayerCmd::CalibrateV2(1234))
        .await
        .unwrap();
    assert!(
        wait_for(4000, || e.status().item.is_some_and(|i| i.kind
            == engine::CALIBRATION_ID
            && i.id == "v2:1234"))
        .await
    );
    assert!(e.dir.join("cache/cal-1234.wav").exists());
    // 2 s dark lead-in, then flashes at the seeded events.
    let (mut white, mut dark) = (false, false);
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(4000) && !(white && dark) {
        match uniform(&e.out(0)) {
            Some(255) => white = true,
            Some(0) => dark = true,
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(white && dark, "white {white} dark {dark}");
    e.engine
        .handle
        .send(PlayerCmd::Calibrate(false))
        .await
        .unwrap();
    assert!(wait_for(4000, || e.status().state == PlayerState::Idle).await);
}

// ---------------------------------------------------------------------------
// F8 prop mask, F18 smart playlists, F11 journal
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn season_profile_keeps_disabled_props_dark() {
    let e = env(LocalRole::Leader, false, |_, show| {
        show.effects = vec![solid("white", "#ffffff", all())];
        show.settings.power.mode = LimiterMode::Off;
        show.profiles = vec![serde_json::from_value(serde_json::json!({
            "id": "xmas", "name": "Christmas", "disabledPropIds": ["b"]
        }))
        .unwrap()];
        show.active_profile_id = Some("xmas".into());
    })
    .await;
    e.engine
        .handle
        .play(PlayRequest {
            effect_id: Some("white".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        wait_for(4000, || e.out_pair(0, 1) == (vec![255; 6], vec![0; 9])).await,
        "{:?}",
        e.out_pair(0, 1)
    );
    // Tests still reach a disabled prop (checking an inflatable in July).
    e.engine
        .handle
        .test_start(TestRequest {
            mode: "solid".into(),
            color: Some("#00ff00".into()),
            target: TestTarget {
                props: Target {
                    prop_ids: vec!["b".into()],
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(wait_for(4000, || px(&e.out(1), 0) == [0, 255, 0]).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn smart_playlist_plays_tonights_songs() {
    let e = env(LocalRole::Leader, false, |dir, show| {
        let mut kids = sequence(dir, "s1", 400, 25, |_| 30);
        kids.tags = vec!["kids".into()];
        show.sequences = vec![kids, sequence(dir, "s2", 400, 25, |_| 60)];
        let mut pl = playlist("smart", &[], 0);
        pl.smart = Some(SmartRules {
            include_tags: vec!["kids".into()],
            ..Default::default()
        });
        show.playlists = vec![pl];
    })
    .await;
    e.engine
        .handle
        .play(PlayRequest {
            playlist_id: Some("smart".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(wait_for(4000, || e.status().item.is_some_and(|i| i.id == "s1")).await);
    assert!(wait_for(4000, || uniform(&e.out(0)) == Some(30)).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plays_are_journaled() {
    let e = env(LocalRole::Leader, false, |dir, show| {
        show.sequences = vec![sequence(dir, "s1", 20, 25, |_| 10)];
    })
    .await;
    crate::services::journal::start(&e.state);
    e.engine
        .handle
        .play(PlayRequest {
            sequence_id: Some("s1".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(wait_for(4000, || e.status().item.is_some()).await);
    assert!(
        wait_for(4000, || e.status().state == PlayerState::Idle
            && e.status().item.is_none())
        .await
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    e.state.services.journal.flush().await;
    let dir = crate::services::journal::dir(&e.dir);
    let today = e.state.services.journal.now().date_naive();
    let recs = crate::services::journal::read_day(&dir, today, None);
    let names: Vec<&str> = recs.iter().map(|r| r.event.name()).collect();
    assert!(
        names.contains(&"itemStart") && names.contains(&"itemEnd"),
        "{names:?} {recs:?}"
    );
    let end = recs
        .iter()
        .find_map(|r| match &r.event {
            crate::services::journal::Event::ItemEnd { id, ended_by, .. } => {
                Some((id.clone(), ended_by.clone()))
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(end, ("s1".to_string(), "finished".to_string()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_look_pulses_with_the_analysed_song() {
    let e = env(LocalRole::Leader, false, |dir, show| {
        std::fs::write(dir.join("media/song.mp3"), b"not really audio").unwrap();
        show.media = vec![serde_json::from_value(serde_json::json!({
            "id": "m1", "name": "Song", "kind": "song", "file": "media/song.mp3", "durationMs": 20000,
            "analysis": {"version": 1, "bpm": 120.0, "bpmConfidence": 0.9, "beatCount": 40,
                         "firstBeatMs": 0, "energy": 0.5}
        }))
        .unwrap()];
        let mut look = solid("idle", "#ffffff", all());
        look.params.insert("beatFollowSong".into(), serde_json::json!(true));
        look.params.insert("beatDepth".into(), serde_json::json!(1.0));
        look.params.insert("beatDecayMs".into(), serde_json::json!(60));
        show.effects = vec![look];
        show.schedule.idle_effect_id = Some("idle".into());
        show.settings.power.mode = LimiterMode::Off;
    })
    .await;
    e.engine
        .handle
        .play(PlayRequest {
            media_id: Some("m1".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(wait_for(4000, || e.status().item.is_some_and(|i| i.id == "m1")).await);
    // 120 BPM: a bright pulse every 500 ms, nearly dark in between.
    let (mut bright, mut dim) = (false, false);
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(2500) && !(bright && dim) {
        match uniform(&e.out(0)) {
            Some(v) if v > 200 => bright = true,
            Some(v) if v < 30 => dim = true,
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(3)).await;
    }
    assert!(bright && dim, "bright {bright} dim {dim}");
}
