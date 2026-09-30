//! In-process cluster integration test: one leader and two followers on
//! 127.0.0.1 with their own HTTP and UDP ports (static peers instead of
//! broadcast), each with a stub player whose commands the test reads.

use super::*;
use crate::config::Config;
use crate::events::EventBus;
use crate::node::NodeIdentity;
use crate::player::{ItemRef, OverlayCmd, PlayerCmd, PlayerHandle, PlayerState, PlayerStatus};
use crate::state::AppInner;
use pixelplus_core::mapping::NodeMap;
use pixelplus_core::model::*;
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::sync::mpsc;

struct TestNode {
    state: AppState,
    cluster: ClusterHandle,
    player_rx: mpsc::Receiver<PlayerCmd>,
    status_tx: watch::Sender<PlayerStatus>,
    http: u16,
    dir: PathBuf,
}

impl TestNode {
    fn id(&self) -> String {
        self.state.identity().id
    }
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}/api/v1{path}", self.http)
    }
}

/// A UDP port whose successor (the overlay port) is free too.
fn free_udp_pair() -> u16 {
    loop {
        let a = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = a.local_addr().unwrap().port();
        if port < 65_000 && std::net::UdpSocket::bind(("127.0.0.1", port + 1)).is_ok() {
            return port;
        }
    }
}

async fn spawn_node(role: LocalRole, udp: u16, peers: Vec<u16>) -> TestNode {
    let dir = std::env::temp_dir().join(format!("pp-cluster-{}", new_id()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = listener.local_addr().unwrap().port();
    let mut config = Config::from_env();
    config.data_dir = dir.clone();
    config.web_dir = dir.join("web");
    config.http_addr = SocketAddr::from(([127, 0, 0, 1], http));
    config.cluster_port = udp;
    config.ensure_dirs().unwrap();

    let events = EventBus::new();
    let store = crate::store::ShowStore::load(&config.show_path(), events.clone()).unwrap();
    let mut identity = NodeIdentity::load_or_create(&config.node_path()).unwrap();
    identity.role = role;
    identity.board = Some(BoardKind::Difftx);
    identity.save(&config.node_path()).unwrap();

    let state = AppState(Arc::new(AppInner {
        config,
        events,
        store,
        identity: parking_lot::RwLock::new(identity),
        sessions: Default::default(),
        started: Instant::now(),
        services: Default::default(),
    }));
    let (tx, player_rx) = mpsc::channel(1024);
    let (status_tx, status_rx) = watch::channel(PlayerStatus::default());
    let _ = state.services.player.set(PlayerHandle::new(tx, status_rx));

    let app = crate::api::router(state.clone());
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });

    let mut settings = ClusterSettings::defaults(udp, http);
    settings.bind = IpAddr::V4(Ipv4Addr::LOCALHOST);
    settings.broadcast = false;
    settings.mdns = false;
    settings.peers = peers.iter().map(|p| format!("127.0.0.1:{p}")).collect();
    settings.beacon_interval = Duration::from_millis(150);
    settings.offline_after = Duration::from_millis(1500);
    settings.manifest_poll = Duration::from_secs(2);
    settings.ping_interval = Duration::from_millis(100);
    settings.sync_interval = Duration::from_millis(100);
    let cluster = start_with(&state, settings).await.unwrap();
    TestNode {
        state,
        cluster,
        player_rx,
        status_tx,
        http,
        dir,
    }
}

async fn eventually<T>(what: &str, timeout: Duration, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(v) = f() {
            return v;
        }
        if Instant::now() > deadline {
            panic!("timed out waiting for: {what}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn next_cmd(
    rx: &mut mpsc::Receiver<PlayerCmd>,
    what: &str,
    mut pred: impl FnMut(&PlayerCmd) -> bool,
) -> PlayerCmd {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(cmd)) if pred(&cmd) => return cmd,
            Ok(Some(_)) => continue,
            Ok(None) => panic!("player channel closed waiting for {what}"),
            Err(_) => panic!("timed out waiting for player command: {what}"),
        }
    }
}

fn write_fseq(path: &std::path::Path, channels: u32, frames: u32) {
    use pixelplus_core::fseq::{FseqWriter, FseqWriterOptions};
    let mut w = FseqWriter::create(path, FseqWriterOptions::new(channels, 25)).unwrap();
    let mut frame = vec![0u8; channels as usize];
    for f in 0..frames {
        for (i, b) in frame.iter_mut().enumerate() {
            *b = (i as u32 * 3 + f * 5 + 1) as u8;
        }
        w.write_frame(&frame).unwrap();
    }
    w.finish().unwrap();
}

fn seg(node: &str, output: u32, start: u32, count: u32, offset: u32) -> PropSegment {
    PropSegment {
        node_id: node.into(),
        output,
        start_pixel: start,
        pixel_count: count,
        prop_offset: offset,
        reverse: false,
        null_pixels: 0,
    }
}

fn prop(id: &str, pixels: u32, channel_start: u32, segments: Vec<PropSegment>) -> Prop {
    let mut p = crate::cluster::manifest::tests::prop(id, pixels, channel_start, segments);
    p.name = format!("Prop {id}");
    p
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn leader_adopts_followers_and_drives_them() {
    let (lp, p1, p2) = (free_udp_pair(), free_udp_pair(), free_udp_pair());
    let leader = spawn_node(LocalRole::Leader, lp, vec![p1, p2]).await;
    let mut f1 = spawn_node(LocalRole::Unconfigured, p1, vec![lp]).await;
    let mut f2 = spawn_node(LocalRole::Unconfigured, p2, vec![lp]).await;
    let (f1_id, f2_id, leader_id) = (f1.id(), f2.id(), leader.id());
    let http = reqwest::Client::builder().no_proxy().build().unwrap();

    // The leader put itself into the show.
    let me = leader
        .state
        .store
        .get()
        .node(&leader_id)
        .cloned()
        .expect("leader node");
    assert_eq!(me.role, NodeRole::Leader);
    assert_eq!(me.outputs.len(), 4);

    // A sequence on the leader.
    let fseq = leader.dir.join("sequences/s1.fseq");
    write_fseq(&fseq, 150, 40);
    let hash = pixelplus_core::fseq::sha256_file(&fseq).unwrap();
    leader
        .state
        .store
        .update(|s| {
            s.sequences.push(Sequence {
                id: "s1".into(),
                name: "Wizards".into(),
                file: "sequences/s1.fseq".into(),
                duration_ms: 1000,
                frame_ms: 25,
                channel_count: 150,
                media_id: None,
                xlights_name: None,
                thumbnail: None,
                hash,
            });
            Ok(())
        })
        .await
        .unwrap();

    // 1. Discovery.
    let found = eventually("followers discovered", Duration::from_secs(5), || {
        let d = leader.cluster.discovered();
        (d.iter().any(|n| n.id == f1_id) && d.iter().any(|n| n.id == f2_id)).then_some(d)
    })
    .await;
    assert!(found
        .iter()
        .all(|n| n.adopted_by.is_none() && n.ip == "127.0.0.1"));
    let r = http
        .get(leader.url("/nodes/discovered"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // 2. Adopt (via the HTTP API, as the UI does).
    let r = http
        .post(leader.url("/nodes/adopt"))
        .json(&serde_json::json!({ "id": f1_id, "name": "Garage" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let node: Node = r.json().await.unwrap();
    assert_eq!(
        (node.name.as_str(), node.role, node.board, node.adopted),
        ("Garage", NodeRole::Follower, BoardKind::Difftx, true)
    );
    assert_eq!(node.outputs.len(), 4);
    let r = http
        .post(leader.url("/nodes/adopt"))
        .json(&serde_json::json!({ "id": f2_id }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    assert_eq!(f1.state.identity().role, LocalRole::Follower);
    assert_eq!(
        f1.state.identity().leader_id.as_deref(),
        Some(leader_id.as_str())
    );
    assert_eq!(
        f1.state.identity().cluster_key,
        leader.state.identity().cluster_key
    );
    assert!(leader
        .cluster
        .discovered()
        .iter()
        .all(|n| n.id != f1_id && n.id != f2_id));

    // Adopting an adopted follower from a stranger is refused.
    let stranger = serde_json::json!({
        "leaderId": "stranger01", "leaderUrl": "http://127.0.0.1:9", "clusterKey": "k".repeat(64)
    });
    let r = http
        .post(f1.url("/cluster/adopt"))
        .json(&stranger)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409);

    // 3. Wire props: a on the leader, b on f1, c split across f1 and f2.
    leader
        .state
        .store
        .update(|s| {
            s.props = vec![
                prop("a", 10, 0, vec![seg(&leader_id, 1, 0, 10, 0)]),
                prop("b", 20, 30, vec![seg(&f1_id, 2, 0, 20, 0)]),
                prop(
                    "c",
                    10,
                    90,
                    vec![seg(&f1_id, 1, 2, 4, 0), seg(&f2_id, 3, 0, 6, 4)],
                ),
            ];
            Ok(())
        })
        .await
        .unwrap();

    // 4. Followers receive their manifest (only their props) and slices.
    let slice1 = f1.dir.join("sequences/s1.ppseq");
    let slice2 = f2.dir.join("sequences/s1.ppseq");
    eventually("f1 show installed", Duration::from_secs(8), || {
        let s = f1.state.store.get();
        (s.props.len() == 2 && s.sequences.len() == 1 && slice1.exists()).then_some(())
    })
    .await;
    eventually("f2 show installed", Duration::from_secs(8), || {
        let s = f2.state.store.get();
        (s.props.len() == 1 && s.sequences.len() == 1 && slice2.exists()).then_some(())
    })
    .await;
    let s1 = f1.state.store.get();
    assert_eq!(
        s1.props.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
        ["b", "c"]
    );
    assert!(s1
        .props
        .iter()
        .flat_map(|p| &p.segments)
        .all(|seg| seg.node_id == f1_id));
    assert_eq!(s1.nodes.len(), 1);
    assert_eq!(s1.nodes[0].id, f1_id);
    assert_eq!(s1.nodes[0].name, "Garage");
    assert_eq!(s1.sequences[0].file, "sequences/s1.ppseq");
    let s2 = f2.state.store.get();
    assert_eq!(s2.props[0].segments, vec![seg(&f2_id, 3, 0, 6, 4)]);
    assert!(f1.dir.join("cluster/manifest.json").exists());

    // Slices are byte-identical to what the leader generates locally.
    let leader_show = leader.state.store.get();
    for (fid, path) in [(&f1_id, &slice1), (&f2_id, &slice2)] {
        let expected = leader.dir.join(format!("expected-{fid}.ppseq"));
        let map = NodeMap::build(&leader_show, fid).unwrap();
        pixelplus_core::ppseq::write_slice_from_path(&fseq, &map, &expected).unwrap();
        assert_eq!(
            std::fs::read(path).unwrap(),
            std::fs::read(&expected).unwrap(),
            "slice of {fid}"
        );
        // …and the follower can play it with its own (identity) map.
        let local_show = if *fid == f1_id {
            s1.clone()
        } else {
            s2.clone()
        };
        let local_map = NodeMap::build(&local_show, fid).unwrap();
        let f = pixelplus_core::ppseq::PpseqFile::open(path).unwrap();
        assert_eq!(f.pixels_per_output(), local_map.pixels_per_output());
    }

    // 5. Status: both followers online and synced.
    eventually("followers synced", Duration::from_secs(8), || {
        let st = leader.cluster.nodes_status();
        let ok = |id: &str| {
            st.iter().any(|s| {
                s.id == id
                    && s.online
                    && s.sync_state == SyncState::Synced
                    && s.files
                        == FileProgress {
                            pending: 0,
                            total: 1,
                        }
            })
        };
        (ok(&f1_id) && ok(&f2_id)).then_some(())
    })
    .await;
    let r: Vec<serde_json::Value> = http
        .get(leader.url("/nodes"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r.len(), 3);
    let garage = r.iter().find(|n| n["id"] == f1_id.as_str()).unwrap();
    assert_eq!(garage["online"], true);
    assert_eq!(garage["syncState"], "synced");
    assert_eq!(garage["ip"], "127.0.0.1");

    // Resumable, verified downloads: a partial file of the same key is
    // resumed with a Range request (a wrong prefix is caught by the sha check),
    // then a clean retry succeeds.
    {
        let m: crate::cluster::manifest::NodeManifest =
            serde_json::from_slice(&std::fs::read(f1.dir.join("cluster/manifest.json")).unwrap())
                .unwrap();
        let seq = m.sequences[0].clone();
        let good = std::fs::read(&slice1).unwrap();
        let part = f1.dir.join("sequences/s1.ppseq.part");
        std::fs::write(f1.dir.join("sequences/s1.ppseq.part.key"), &seq.hash).unwrap();
        std::fs::write(&part, vec![0u8; 64]).unwrap();
        let r = follower::download_slice(&f1.state, &f1.cluster.shared, &seq).await;
        assert!(
            matches!(r, Err(follower::FetchError::Other(ref e)) if e.contains("checksum")),
            "{r:?}"
        );
        assert!(!part.exists());
        std::fs::write(f1.dir.join("sequences/s1.ppseq.part.key"), &seq.hash).unwrap();
        std::fs::write(&part, &good[..good.len() / 2]).unwrap();
        let local = follower::download_slice(&f1.state, &f1.cluster.shared, &seq)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&slice1).unwrap(), good);
        assert_eq!(local.key, seq.hash);
        assert!(!part.exists());
    }

    // Clock estimate: follower knows the leader clock within a few ms.
    let truth = f1
        .state
        .started
        .duration_since(leader.state.started)
        .as_secs_f64()
        * 1000.0
        - leader
            .state
            .started
            .duration_since(f1.state.started)
            .as_secs_f64()
            * 1000.0;
    let est = f1
        .cluster
        .shared
        .clock
        .lock()
        .offset_ms()
        .expect("clock estimate");
    assert!(
        (est - truth).abs() < 20.0,
        "offset estimate {est} vs {truth}"
    );

    // 6. Sync: the follower player follows the leader.
    leader.status_tx.send_replace(PlayerStatus {
        state: PlayerState::Playing,
        item: Some(ItemRef {
            kind: "sequence".into(),
            id: "s1".into(),
            name: "Wizards".into(),
        }),
        pos_ms: 400,
        duration_ms: 1000,
        brightness: 90,
        ..Default::default()
    });
    let cmd = next_cmd(
        &mut f1.player_rx,
        "sync playing",
        |c| matches!(c, PlayerCmd::Sync(p) if p.state == PlayerState::Playing),
    )
    .await;
    let PlayerCmd::Sync(p) = cmd else {
        unreachable!()
    };
    assert_eq!(p.leader, leader_id);
    assert_eq!(p.item.as_ref().unwrap().id, "s1");
    assert_eq!(p.brightness, 90);
    assert!((400..=1000).contains(&p.pos_ms), "pos {}", p.pos_ms);
    let local_now = f1.cluster.now_ms();
    assert!(
        (local_now - p.sent_at_ms as f64).abs() < 1000.0,
        "sent_at is local time"
    );
    // The effect of a look is stamped with the display-wide bounds.
    leader
        .state
        .store
        .update(|s| {
            s.effects.push(EffectPreset {
                id: "wash".into(),
                name: "Wash".into(),
                effect: EffectKind::Colorwash,
                params: Default::default(),
                target: Target {
                    all: true,
                    ..Default::default()
                },
            });
            Ok(())
        })
        .await
        .unwrap();
    leader.status_tx.send_replace(PlayerStatus {
        state: PlayerState::Effect,
        item: Some(ItemRef {
            kind: "effect".into(),
            id: "wash".into(),
            name: "Wash".into(),
        }),
        brightness: 90,
        ..Default::default()
    });
    let cmd = next_cmd(
        &mut f2.player_rx,
        "sync effect",
        |c| matches!(c, PlayerCmd::Sync(p) if p.state == PlayerState::Effect),
    )
    .await;
    let PlayerCmd::Sync(p) = cmd else {
        unreachable!()
    };
    let mut expected = leader.state.store.get().effect("wash").cloned().unwrap();
    pixelplus_core::effects::stamp_world_bounds(&mut expected, &leader.state.store.get().props);
    assert_eq!(p.effect, Some(expected));
    leader.status_tx.send_replace(PlayerStatus::default());
    next_cmd(
        &mut f1.player_rx,
        "sync idle",
        |c| matches!(c, PlayerCmd::Sync(p) if p.state == PlayerState::Idle),
    )
    .await;

    // 7. Overlay frames for a prop that lives on followers.
    let rgb: Vec<u8> = (0..30).collect();
    assert_eq!(leader.cluster.forward_overlay("c", &rgb), 2);
    assert_eq!(
        leader.cluster.forward_overlay("a", &rgb),
        0,
        "a is local to the leader"
    );
    let cmd = next_cmd(&mut f2.player_rx, "overlay", |c| {
        matches!(c, PlayerCmd::Overlay(OverlayCmd::PropPixels { .. }))
    })
    .await;
    let PlayerCmd::Overlay(OverlayCmd::PropPixels { prop_id, rgb: got }) = cmd else {
        unreachable!()
    };
    assert_eq!((prop_id.as_str(), got), ("c", rgb));

    // 8. Commands.
    let res = leader
        .cluster
        .send_command(Some(&f2_id), ClusterCommand::Blackout { on: true })
        .await;
    assert_eq!(
        res,
        vec![CommandResult {
            node_id: f2_id.clone(),
            ok: true,
            error: None
        }]
    );
    next_cmd(&mut f2.player_rx, "blackout", |c| {
        matches!(c, PlayerCmd::Blackout(true))
    })
    .await;

    // Identify: the follower blinks all its outputs.
    let req = tokio::spawn({
        let http = http.clone();
        let url = leader.url(&format!("/nodes/{f1_id}/identify"));
        async move { http.post(url).send().await.unwrap().status() }
    });
    let cmd = next_cmd(&mut f1.player_rx, "identify", |c| {
        matches!(c, PlayerCmd::TestStart(..))
    })
    .await;
    let PlayerCmd::TestStart(test, reply) = cmd else {
        unreachable!()
    };
    assert_eq!(test.mode, "chase");
    assert_eq!(test.target.node_id.as_deref(), Some(f1_id.as_str()));
    reply.send(Ok(())).unwrap();
    assert_eq!(req.await.unwrap(), 200);

    // 9. Cluster endpoints need the key.
    let r = http
        .get(leader.url(&format!("/cluster/manifest/{f1_id}")))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let r = http
        .get(leader.url(&format!("/cluster/slice/{f1_id}/s1")))
        .header(KEY_HEADER, "x".repeat(64))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    // Resumable download: a range request returns the tail of the slice.
    let key = leader.state.identity().cluster_key.unwrap();
    let full = std::fs::read(&slice1).unwrap();
    let r = http
        .get(leader.url(&format!("/cluster/slice/{f1_id}/s1")))
        .header(KEY_HEADER, &key)
        .header("range", "bytes=10-")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 206);
    assert_eq!(r.bytes().await.unwrap().as_ref(), &full[10..]);

    // 10. A follower goes offline → event + status.
    let mut events = leader.cluster.subscribe();
    f2.cluster.shutdown();
    let ev = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(ClusterEvent::NodeOffline { node_id, .. }) = events.recv().await {
                break node_id;
            }
        }
    })
    .await
    .expect("offline event");
    assert_eq!(ev, f2_id);
    let st = leader.cluster.nodes_status();
    let f2s = st.iter().find(|s| s.id == f2_id).unwrap();
    assert!(!f2s.online);
    assert_eq!(f2s.sync_state, SyncState::Offline);

    // 11. Re-routing a prop invalidates only the affected slice.
    let before = std::fs::read(&slice1).unwrap();
    leader
        .state
        .store
        .update(|s| {
            s.props[1].segments[0].start_pixel = 5;
            Ok(())
        })
        .await
        .unwrap();
    eventually("f1 re-downloaded", Duration::from_secs(8), || {
        let now = std::fs::read(&slice1).ok()?;
        (now != before && f1.state.store.get().props[0].segments[0].start_pixel == 5).then_some(())
    })
    .await;

    // 12. Release: the follower forgets its leader and goes dark.
    let r = http
        .post(leader.url(&format!("/nodes/{f1_id}/release")))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["reached"], true);
    assert_eq!(f1.state.identity().leader_id, None);
    assert!(f1.state.store.get().props.is_empty());
    assert!(!leader.state.store.get().node(&f1_id).unwrap().adopted);
    // It shows up as adoptable again.
    eventually("f1 rediscovered", Duration::from_secs(5), || {
        leader
            .cluster
            .discovered()
            .iter()
            .any(|n| n.id == f1_id)
            .then_some(())
    })
    .await;

    // 13. Deleting a wired controller releases it but keeps it (and its
    // wiring) in the show; `force` removes it with its wiring.
    let r = http
        .delete(leader.url(&format!("/nodes/{f2_id}")))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["removed"], false);
    assert_eq!(body["released"], true);
    assert!(!leader.state.store.get().node(&f2_id).unwrap().adopted);
    assert_eq!(f2.state.identity().leader_id, None);
    let r = http
        .delete(leader.url(&format!("/nodes/{f2_id}?force=1")))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let show = leader.state.store.get();
    assert!(show.node(&f2_id).is_none());
    assert_eq!(show.props[2].segments.len(), 1);

    for n in [&leader, &f1, &f2] {
        n.cluster.shutdown();
        let _ = std::fs::remove_dir_all(&n.dir);
    }
}
