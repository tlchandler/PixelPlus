//! Test harness for the HTTP API (in-process `Router` + temp data dir + a
//! stub player), and the integration tests of the system/content endpoints.

use crate::config::{Config, OutputMode};
use crate::events::EventBus;
use crate::node::NodeIdentity;
use crate::player::{OverlayCmd, OverlayInfo, PlayerCmd, PlayerHandle, PlayerStatus};
use crate::state::{AppInner, AppState};
use crate::store::ShowStore;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use parking_lot::Mutex;
use serde_json::Value;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tower::ServiceExt;

pub struct TestApp {
    pub state: AppState,
    pub router: Router,
    pub dir: PathBuf,
    /// Debug renderings of every command the stub player received.
    pub commands: Arc<Mutex<Vec<String>>>,
    pub status: watch::Sender<PlayerStatus>,
    pub cookie: Option<String>,
}

impl Drop for TestApp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub struct Part<'a> {
    pub name: &'a str,
    pub filename: Option<&'a str>,
    pub data: Vec<u8>,
}

pub fn multipart_body(parts: &[Part]) -> (String, Vec<u8>) {
    let boundary = "----pixelplus-test-boundary-7MA4YWxkTrZu0gW";
    let mut body = Vec::new();
    for p in parts {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match p.filename {
            Some(f) => body.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{}\"; filename=\"{f}\"\r\nContent-Type: application/octet-stream\r\n\r\n", p.name).as_bytes(),
            ),
            None => body.extend_from_slice(format!("Content-Disposition: form-data; name=\"{}\"\r\n\r\n", p.name).as_bytes()),
        }
        body.extend_from_slice(&p.data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

impl TestApp {
    pub fn new() -> TestApp {
        let dir = std::env::temp_dir().join(format!("pp-api-{}", pixelplus_core::model::new_id()));
        let config = Config {
            data_dir: dir.clone(),
            web_dir: dir.join("web"),
            http_addr: "127.0.0.1:0".parse().unwrap(),
            cluster_port: 0,
            output: OutputMode::None,
            tts_url: "http://127.0.0.1:9".into(),
            games_socket: dir.join("games.sock"),
            dev: true,
        };
        config.ensure_dirs().unwrap();
        let events = EventBus::new();
        let store = ShowStore::load(&config.show_path(), events.clone()).unwrap();
        let identity = NodeIdentity::load_or_create(&config.node_path()).unwrap();
        let state = AppState(Arc::new(AppInner {
            config,
            events,
            store,
            identity: parking_lot::RwLock::new(identity),
            sessions: Default::default(),
            started: std::time::Instant::now(),
            services: Default::default(),
        }));
        let (tx, mut rx) = mpsc::channel::<PlayerCmd>(256);
        let (status_tx, status_rx) = watch::channel(PlayerStatus::default());
        let _ = state.services.player.set(PlayerHandle::new(tx, status_rx));
        let commands = Arc::new(Mutex::new(Vec::new()));
        let log = commands.clone();
        tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                let text = match &cmd {
                    PlayerCmd::Overlay(OverlayCmd::PropPixels { prop_id, rgb }) => {
                        format!("PropPixels {prop_id} {}", rgb.len())
                    }
                    other => format!("{other:?}"),
                };
                log.lock().push(text);
                match cmd {
                    PlayerCmd::Play(_, reply) => {
                        let _ = reply.send(Ok(()));
                    }
                    PlayerCmd::TestStart(_, reply) => {
                        let _ = reply.send(Ok(()));
                    }
                    PlayerCmd::Overlay(OverlayCmd::Open { prop_id, reply }) => {
                        let _ = reply.send(Ok(OverlayInfo {
                            shm: format!("/dev/shm/pixelplus-overlay-{prop_id}"),
                            width: 8,
                            height: 4,
                        }));
                    }
                    _ => {}
                }
            }
        });
        let router = super::router(state.clone());
        TestApp {
            state,
            router,
            dir,
            commands,
            status: status_tx,
            cookie: None,
        }
    }

    pub async fn send(
        &self,
        mut req: Request<Body>,
    ) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        if let Some(c) = &self.cookie {
            req.headers_mut().insert(header::COOKIE, c.parse().unwrap());
        }
        // What the web UI sends on every request (see api::security).
        if !req.headers().contains_key("x-pixelplus-request") {
            req.headers_mut()
                .insert("x-pixelplus-request", "1".parse().unwrap());
        }
        if req.extensions().get::<ConnectInfo<SocketAddr>>().is_none() {
            req.extensions_mut().insert(ConnectInfo::<SocketAddr>(
                "192.168.1.77:50000".parse().unwrap(),
            ));
        }
        let resp = self.router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec();
        (status, headers, body)
    }

    pub async fn json(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut b = Request::builder()
            .method(method)
            .uri(format!("/api/v1{path}"));
        let body = match body {
            Some(v) => {
                b = b.header(header::CONTENT_TYPE, "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let (status, _, bytes) = self.send(b.body(body).unwrap()).await;
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    pub async fn upload(&self, path: &str, parts: &[Part<'_>]) -> (StatusCode, Value) {
        let (ct, body) = multipart_body(parts);
        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/v1{path}"))
            .header(header::CONTENT_TYPE, ct)
            .body(Body::from(body))
            .unwrap();
        let (status, _, bytes) = self.send(req).await;
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    pub async fn commands_matching(&self, needle: &str) -> usize {
        // Commands are queued; give the stub a moment to log them.
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        self.commands
            .lock()
            .iter()
            .filter(|c| c.contains(needle))
            .count()
    }
}

/// An fseq with `pixels` RGB pixels and `frames` frames of a moving red dot.
pub fn make_fseq(path: &std::path::Path, pixels: u32, frames: u32, media: Option<&str>) {
    use pixelplus_core::fseq::{FseqWriter, FseqWriterOptions};
    let mut opts = FseqWriterOptions::new(pixels * 3, 50).fit_frame_count(frames);
    opts.media_filename = media.map(str::to_string);
    let mut w = FseqWriter::create(path, opts).unwrap();
    for f in 0..frames {
        let mut frame = vec![0u8; pixels as usize * 3];
        let i = (f % pixels) as usize * 3;
        frame[i] = 255;
        frame[i + 1] = (f * 5 % 256) as u8;
        w.write_frame(&frame).unwrap();
    }
    w.finish().unwrap();
}

pub fn testdata(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../pixelplus-core/testdata")
        .join(name);
    std::fs::read(p).unwrap()
}

// ---------------------------------------------------------------------------
// Integration tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn leader(app: &mut TestApp, password: Option<&str>) {
        let mut body = json!({ "role": "leader", "showName": "Chandler Lights", "timezone": "America/Chicago", "board": "difftx", "boardRev": "E" });
        if let Some(p) = password {
            body["password"] = json!(p);
        }
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/system/setup")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let (status, headers, bytes) = app.send(req).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        if password.is_some() {
            let cookie = headers
                .get(header::SET_COOKIE)
                .expect("session cookie")
                .to_str()
                .unwrap();
            app.cookie = Some(cookie.split(';').next().unwrap().to_string());
        }
    }

    #[tokio::test]
    async fn setup_seeds_show_and_protects_api() {
        let mut app = TestApp::new();
        let (s, info) = app.json("GET", "/system", None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(info["needsSetup"], true);
        assert_eq!(info["passwordSet"], false);
        leader(&mut app, Some("jingle")).await;
        let show = app.state.store.get();
        assert_eq!(show.name, "Chandler Lights");
        assert_eq!(show.schedule.location.timezone, "America/Chicago");
        assert!(show.dj_voices.iter().any(|v| v.id == "nick"));
        assert!(show.dj_voices.iter().any(|v| v.id == "holly"));
        assert!(show.pronunciations.len() > 50);
        assert!(show.playlists.iter().any(|p| p.name == "Main Show"));
        assert!(show.effects.iter().any(|e| e.id.starts_with("builtin-")));
        assert!(show.leader().is_some(), "leader node created");

        // Signed in via the setup cookie.
        let (s, _) = app.json("GET", "/show", None).await;
        assert_eq!(s, StatusCode::OK);
        let (s, info) = app.json("GET", "/system", None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(info["passwordSet"], true);
        assert!(info.get("cpuPct").is_some());
        // Without the cookie: locked, but /system and /public stay open.
        let cookie = app.cookie.take();
        assert_eq!(
            app.json("GET", "/show", None).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.json("GET", "/requests", None).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.json("GET", "/public/requests", None).await.0,
            StatusCode::OK
        );
        let (s, info) = app.json("GET", "/system", None).await;
        assert_eq!(s, StatusCode::OK);
        assert!(
            info.get("ips").is_none(),
            "no private details when signed out"
        );
        assert_eq!(
            app.json("POST", "/system/setup", Some(json!({"role": "leader"})))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        app.cookie = cookie;
        // The UI's password change shape ({current, password}).
        let (s, _) = app
            .json(
                "PUT",
                "/auth/password",
                Some(json!({"current": "jingle", "password": "sleighbells"})),
            )
            .await;
        assert_eq!(s, StatusCode::OK);
        let h = app
            .state
            .store
            .get()
            .settings
            .security
            .password_hash
            .clone()
            .unwrap();
        assert!(crate::api::auth::verify_password(&h, "sleighbells"));
    }

    #[tokio::test]
    async fn sequence_upload_links_audio_and_draws_thumbnail() {
        let mut app = TestApp::new();
        leader(&mut app, None).await;
        // A prop that needs more channels than the sequence has.
        let node = app.state.store.get().leader().unwrap().id.clone();
        let (s, _) = app
            .json(
                "POST",
                "/props",
                Some(json!({"name": "Big Tree", "kind": "tree", "pixelCount": 200, "channelStart": 0,
                    "segments": [{"nodeId": node, "output": 1, "startPixel": 0, "pixelCount": 200, "propOffset": 0}]})),
            )
            .await;
        assert_eq!(s, StatusCode::OK);
        // Song first.
        let wav = app.dir.join("song.wav");
        crate::services::media::tests::sine_wav(&wav, 1.5, 0.3);
        let (s, media) = app
            .upload(
                "/media",
                &[
                    Part {
                        name: "kind",
                        filename: None,
                        data: b"song".to_vec(),
                    },
                    Part {
                        name: "file",
                        filename: Some("Jingle_Bell_Rock.wav"),
                        data: std::fs::read(&wav).unwrap(),
                    },
                ],
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{media}");
        assert_eq!(media["name"], "Jingle Bell Rock");
        assert!((1400..=1600).contains(&media["durationMs"].as_u64().unwrap()));
        assert!(media["loudnessLufs"].as_f64().unwrap() < -5.0);
        assert!(media["gainDb"].is_number());
        let media_id = media["id"].as_str().unwrap().to_string();

        // Sequence whose header names the song.
        let fseq = app.dir.join("upload.fseq");
        make_fseq(
            &fseq,
            100,
            60,
            Some("C:\\Show\\Audio\\Jingle Bell Rock.mp3"),
        );
        let (s, seq) = app
            .upload(
                "/sequences",
                &[Part {
                    name: "fseq",
                    filename: Some("Jingle_Bell_Rock.fseq"),
                    data: std::fs::read(&fseq).unwrap(),
                }],
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{seq}");
        assert_eq!(seq["name"], "Jingle Bell Rock");
        assert_eq!(seq["mediaId"], media_id.as_str());
        assert_eq!(seq["channelCount"], 300);
        assert_eq!(seq["frameMs"], 50);
        assert_eq!(seq["durationMs"], 3000);
        assert_eq!(seq["hash"].as_str().unwrap().len(), 64);
        assert!(seq["warnings"][0].as_str().unwrap().contains("Big Tree"));
        let id = seq["id"].as_str().unwrap().to_string();
        let req = Request::builder()
            .uri(format!("/api/v1/sequences/{id}/thumbnail"))
            .body(Body::empty())
            .unwrap();
        let (s, h, png) = app.send(req).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(h[header::CONTENT_TYPE], "image/png");
        assert!(png.starts_with(b"\x89PNG"));

        // Re-upload with the same name replaces in place.
        let (s, again) = app
            .upload(
                "/sequences",
                &[Part {
                    name: "fseq",
                    filename: Some("Jingle_Bell_Rock.fseq"),
                    data: std::fs::read(&fseq).unwrap(),
                }],
            )
            .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(again["id"], id.as_str());
        assert_eq!(again["replaced"], true);
        assert_eq!(app.state.store.get().sequences.len(), 1);

        // Re-upload with audio that can't be read: refused, and the sequence on
        // disk still matches its hash (followers cache slices by it).
        let before = app.state.store.get().sequences[0].clone();
        let longer = app.dir.join("longer.fseq");
        make_fseq(&longer, 100, 90, None);
        let (s, _) = app
            .upload(
                "/sequences",
                &[
                    Part {
                        name: "fseq",
                        filename: Some("Jingle_Bell_Rock.fseq"),
                        data: std::fs::read(&longer).unwrap(),
                    },
                    Part {
                        name: "audio",
                        filename: Some("Jingle_Bell_Rock.mp3"),
                        data: b"not audio at all".to_vec(),
                    },
                ],
            )
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(app.state.store.get().sequences[0], before);
        let on_disk = pixelplus_core::fseq::sha256_file(app.dir.join(&before.file)).unwrap();
        assert_eq!(on_disk, before.hash, "the old file is untouched");

        // Garbage is rejected with a friendly message.
        let (s, err) = app
            .upload(
                "/sequences",
                &[Part {
                    name: "fseq",
                    filename: Some("oops.fseq"),
                    data: b"PK not a sequence".to_vec(),
                }],
            )
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert!(err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("isn't a sequence PixelPlus can play"));

        // Add to a playlist, delete (removed from the playlist), undo.
        let pl = app.state.store.get().playlists[0].id.clone();
        let (s, _) = app
            .json(
                "PUT",
                &format!("/playlists/{pl}"),
                Some(json!({"items": [{"id": "i1", "type": "sequence", "sequenceId": id}]})),
            )
            .await;
        assert_eq!(s, StatusCode::OK);
        let copy = app.json("GET", &format!("/sequences/{id}"), None).await.1;
        let (s, _) = app.json("DELETE", &format!("/sequences/{id}"), None).await;
        assert_eq!(s, StatusCode::OK);
        let show = app.state.store.get();
        assert!(show.sequences.is_empty());
        assert!(show.playlists[0].items.is_empty());
        assert!(!app.dir.join(format!("sequences/{id}.fseq")).exists());
        let (s, back) = app.json("POST", "/sequences", Some(copy)).await;
        assert_eq!(s, StatusCode::OK, "{back}");
        assert!(app.dir.join(format!("sequences/{id}.fseq")).exists());

        // Media: range requests and peaks.
        let req = Request::builder()
            .uri(format!("/api/v1/media/{media_id}/file"))
            .header(header::RANGE, "bytes=0-99")
            .body(Body::empty())
            .unwrap();
        let (s, h, bytes) = app.send(req).await;
        assert_eq!(s, StatusCode::PARTIAL_CONTENT);
        assert_eq!(bytes.len(), 100);
        assert!(h[header::CONTENT_RANGE]
            .to_str()
            .unwrap()
            .starts_with("bytes 0-99/"));
        let (s, peaks) = app
            .json("GET", &format!("/media/{media_id}/peaks?n=50"), None)
            .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(peaks.as_array().unwrap().len(), 50);
        // Deleting the song unlinks the sequence.
        let (s, _) = app
            .json("DELETE", &format!("/media/{media_id}"), None)
            .await;
        assert_eq!(s, StatusCode::OK);
        assert!(app.state.store.get().sequences[0].media_id.is_none());
        // Power estimate for the sequence (cached on the second call).
        let (s, p) = app
            .json("GET", &format!("/power/estimate?sequenceId={id}"), None)
            .await;
        assert_eq!(s, StatusCode::OK, "{p}");
        assert!(p["perProp"].is_array());
        let (s, _) = app.json("GET", "/power/estimate", None).await;
        assert_eq!(s, StatusCode::OK);
    }

    #[tokio::test]
    async fn xlights_import_preview_and_apply() {
        let mut app = TestApp::new();
        leader(&mut app, None).await;
        let (s, preview) = app
            .upload(
                "/import/xlights",
                &[
                    // Deliberately swapped slots: sorted out by root element.
                    Part {
                        name: "rgbeffects",
                        filename: Some("xlights_networks.xml"),
                        data: testdata("data_xlights_networks.xml"),
                    },
                    Part {
                        name: "networks",
                        filename: Some("xlights_rgbeffects.xml"),
                        data: testdata("data_xlights_rgbeffects.xml"),
                    },
                ],
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{preview}");
        let props = preview["props"].as_array().unwrap();
        assert!(props.len() >= 5, "{}", props.len());
        let controllers = preview["controllers"].as_array().unwrap();
        assert!(!controllers.is_empty());
        let leader_id = app.state.store.get().leader().unwrap().id.clone();
        let map: serde_json::Map<String, Value> = controllers
            .iter()
            .map(|c| (c["name"].as_str().unwrap().to_string(), json!(leader_id)))
            .collect();
        let (s, show) = app
            .json(
                "POST",
                "/import/xlights/apply",
                Some(json!({"preview": preview, "controllerMap": map})),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{show}");
        let stored = app.state.store.get();
        assert_eq!(stored.props.len(), props.len());
        assert!(stored.props.iter().all(|p| p.layout.is_some()));
        assert!(stored.props.iter().any(|p| !p.segments.is_empty()));
        // An automatic snapshot was taken first.
        let (_, snaps) = app.json("GET", "/snapshots", None).await;
        assert!(snaps.as_array().unwrap().iter().any(|s| s["auto"] == true));
        // Bad input.
        let (s, err) = app
            .upload(
                "/import/xlights",
                &[Part {
                    name: "rgbeffects",
                    filename: Some("x.xml"),
                    data: b"<html/>".to_vec(),
                }],
            )
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert!(err["error"]["message"].is_string());
    }

    #[tokio::test]
    async fn large_uploads_stream_to_disk() {
        use std::io::Write;
        let mut app = TestApp::new();
        leader(&mut app, None).await;
        // > 2 MB (axum's default body limit) audio.
        let wav = app.dir.join("long.wav");
        crate::services::media::tests::sine_wav(&wav, 40.0, 0.2);
        let data = std::fs::read(&wav).unwrap();
        assert!(data.len() > 3_000_000);
        let (s, m) = app
            .upload(
                "/media",
                &[Part {
                    name: "file",
                    filename: Some("Long Song.wav"),
                    data,
                }],
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{m}");
        assert!((39_900..=40_100).contains(&m["durationMs"].as_u64().unwrap()));
        // A zipped show folder with a big sequence inside.
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let stored = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            w.start_file("MyShow/xlights_rgbeffects.xml", stored)
                .unwrap();
            w.write_all(&testdata("data_xlights_rgbeffects.xml"))
                .unwrap();
            w.start_file("MyShow/xlights_networks.xml", stored).unwrap();
            w.write_all(&testdata("data_xlights_networks.xml")).unwrap();
            w.start_file("MyShow/big.fseq", stored).unwrap();
            w.write_all(&vec![7u8; 3_000_000]).unwrap();
            w.finish().unwrap();
        }
        let (s, p) = app
            .upload(
                "/import/xlights",
                &[Part {
                    name: "rgbeffects",
                    filename: Some("MyShow.zip"),
                    data: buf.into_inner(),
                }],
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{p}");
        assert!(!p["props"].as_array().unwrap().is_empty());
        assert!(
            p["controllers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["ip"].is_string()),
            "networks.xml was read from the zip"
        );
    }

    #[tokio::test]
    async fn snapshot_roundtrip() {
        let mut app = TestApp::new();
        leader(&mut app, None).await;
        let (s, snap) = app
            .json(
                "POST",
                "/snapshots",
                Some(json!({"label": "Before the party"})),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{snap}");
        assert_eq!(snap["label"], "Before the party");
        assert_eq!(snap["auto"], false);
        assert!(snap["sizeBytes"].as_u64().unwrap() > 0);
        let id = snap["id"].as_str().unwrap().to_string();
        app.json("PUT", "/show/name", Some(json!({"name": "Changed"})))
            .await;
        assert_eq!(app.state.store.get().name, "Changed");
        let (s, _) = app
            .json("POST", &format!("/snapshots/{id}/restore"), None)
            .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(app.state.store.get().name, "Chandler Lights");
        let (_, list) = app.json("GET", "/snapshots", None).await;
        let list = list.as_array().unwrap();
        assert!(list
            .iter()
            .any(|s| s["label"] == "Before restore" && s["auto"] == true));
        // Download and import it again.
        let req = Request::builder()
            .uri(format!("/api/v1/snapshots/{id}/download"))
            .body(Body::empty())
            .unwrap();
        let (s, h, bytes) = app.send(req).await;
        assert_eq!(s, StatusCode::OK);
        assert!(h[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .contains("attachment"));
        let (s, imported) = app
            .upload(
                "/snapshots/import",
                &[Part {
                    name: "file",
                    filename: Some("backup.tar.zst"),
                    data: bytes,
                }],
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{imported}");
        assert!(imported["label"].as_str().unwrap().starts_with("Imported"));
        let (s, _) = app
            .upload(
                "/snapshots/import",
                &[Part {
                    name: "file",
                    filename: Some("junk.tar.zst"),
                    data: b"junk".to_vec(),
                }],
            )
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, _) = app.json("DELETE", &format!("/snapshots/{id}"), None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(
            app.json("DELETE", &format!("/snapshots/{id}"), None)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            app.json("POST", "/snapshots/..%2Fetc/restore", None)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn song_requests_rate_limited_and_forwarded() {
        let mut app = TestApp::new();
        leader(&mut app, Some("jingle")).await;
        for i in 0..5 {
            let fseq = app.dir.join(format!("s{i}.fseq"));
            make_fseq(&fseq, 10, 10, None);
            let name = format!("Song_{i}.fseq");
            let (s, _) = app
                .upload(
                    "/sequences",
                    &[Part {
                        name: "fseq",
                        filename: Some(&name),
                        data: std::fs::read(&fseq).unwrap(),
                    }],
                )
                .await;
            assert_eq!(s, StatusCode::OK);
        }
        let (s, _) = app
            .json(
                "PUT",
                "/show/settings",
                Some(json!({"requests": {"enabled": true, "maxQueue": 10}})),
            )
            .await;
        assert_eq!(s, StatusCode::OK);
        let cookie = app.cookie.take();
        let (s, public) = app.json("GET", "/public/requests", None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(public["songs"].as_array().unwrap().len(), 5);
        assert!(public.to_string().find("passwordHash").is_none());
        let ids: Vec<String> = public["songs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["sequenceId"].as_str().unwrap().to_string())
            .collect();
        for (i, id) in ids.iter().take(3).enumerate() {
            let (s, r) = app
                .json(
                    "POST",
                    "/public/requests",
                    Some(json!({"sequenceId": id, "name": "Tom"})),
                )
                .await;
            assert_eq!(s, StatusCode::OK, "{r}");
            assert_eq!(r["position"], i as u64 + 1);
        }
        let (s, r) = app
            .json(
                "POST",
                "/public/requests",
                Some(json!({"sequenceId": ids[0]})),
            )
            .await;
        assert_eq!(s, StatusCode::CONFLICT);
        assert_eq!(r["error"]["code"], "already_queued");
        let (s, r) = app
            .json(
                "POST",
                "/public/requests",
                Some(json!({"sequenceId": ids[3]})),
            )
            .await;
        assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(r["error"]["code"], "rate_limited");
        // Another visitor is fine.
        let mut req = Request::builder()
            .method("POST")
            .uri("/api/v1/public/requests")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"sequenceId": ids[3]}).to_string()))
            .unwrap();
        req.extensions_mut().insert(ConnectInfo::<SocketAddr>(
            "192.168.1.99:1234".parse().unwrap(),
        ));
        assert_eq!(app.send(req).await.0, StatusCode::OK);
        // Admin view needs sign-in.
        assert_eq!(
            app.json("GET", "/requests", None).await.0,
            StatusCode::UNAUTHORIZED
        );
        app.cookie = cookie;
        let (s, q) = app.json("GET", "/requests", None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(q.as_array().unwrap().len(), 4);
        let first = q[0]["id"].as_str().unwrap().to_string();
        assert_eq!(
            app.json("DELETE", &format!("/requests/{first}"), None)
                .await
                .0,
            StatusCode::OK
        );
        // Hand-off: only the head goes to the player.
        crate::services::requests::start(&app.state);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let _ = app.status.send(PlayerStatus::default());
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(app.commands_matching("Enqueue").await, 1);
    }

    #[tokio::test]
    async fn a_show_window_starting_ends_a_forgotten_fault_finder() {
        let mut app = TestApp::new();
        leader(&mut app, None).await;
        let node = app.state.store.get().leader().unwrap().id.clone();
        let (_, prop) = app
            .json(
                "POST",
                "/props",
                Some(json!({"name": "Arch", "kind": "arch", "pixelCount": 50, "channelStart": 0,
                    "segments": [{"nodeId": node, "output": 1, "startPixel": 0, "pixelCount": 50, "propOffset": 0}]})),
            )
            .await;
        let prop_id = prop["id"].as_str().unwrap().to_string();
        let (s, _) = app
            .json(
                "POST",
                "/faultfinder/start",
                Some(json!({"propId": prop_id})),
            )
            .await;
        assert_eq!(s, StatusCode::OK);
        let off = format!("Enable {{ prop_id: \"{prop_id}\", enabled: false }}");
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        assert_eq!(app.commands_matching(&off).await, 0, "still running");
        // Sunset: the scheduled show window begins.
        app.status.send_modify(|st| {
            st.schedule_entry = Some(crate::player::ScheduleRef {
                id: "nightly".into(),
                name: "Nightly".into(),
                ends_at: "2026-12-01T22:00:00-06:00".into(),
            })
        });
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        assert_eq!(app.commands_matching(&off).await, 1);
        assert!(app.state.services.faults.session.lock().is_none());
    }

    #[tokio::test]
    async fn player_and_fault_finder() {
        let mut app = TestApp::new();
        leader(&mut app, None).await;
        let node = app.state.store.get().leader().unwrap().id.clone();
        let (_, prop) = app
            .json(
                "POST",
                "/props",
                Some(json!({"name": "Arch", "kind": "arch", "pixelCount": 50, "channelStart": 0,
                    "segments": [{"nodeId": node, "output": 1, "startPixel": 0, "pixelCount": 50, "propOffset": 0}]})),
            )
            .await;
        let prop_id = prop["id"].as_str().unwrap().to_string();
        assert_eq!(
            app.json("POST", "/player/blackout", Some(json!({"enabled": true})))
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(app.commands_matching("Blackout(true)").await, 1);
        assert_eq!(
            app.json("PUT", "/player/volume", Some(json!({"volume": 150})))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            app.json("PUT", "/player/brightness", Some(json!({"brightness": 40})))
                .await
                .0,
            StatusCode::OK
        );
        let (s, e) = app
            .json("POST", "/player/play", Some(json!({"sequenceId": "nope"})))
            .await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{e}");
        let (s, _) = app.json("POST", "/player/play", Some(json!({}))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "empty Main Show playlist");
        let (s, _) = app
            .json(
                "POST",
                "/test/start",
                Some(json!({"mode": "chase", "target": {"propIds": [prop_id]}})),
            )
            .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(app.commands_matching("TestStart").await, 1);
        let (s, _) = app
            .json(
                "POST",
                "/test/start",
                Some(json!({"mode": "disco", "target": {"all": true}})),
            )
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let look = app.state.store.get().effects[0].clone();
        assert_eq!(
            app.json("POST", "/player/effect", Some(json!({"effect": look})))
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(
            app.json(
                "POST",
                "/effects/preview-apply",
                Some(json!({"preset": look}))
            )
            .await
            .0,
            StatusCode::OK
        );

        let (s, step) = app
            .json(
                "POST",
                "/faultfinder/start",
                Some(json!({"propId": prop_id})),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{step}");
        let session = step["session"].as_str().unwrap().to_string();
        assert_eq!(step["litFrom"], 0);
        assert!(step["litTo"].as_u64().unwrap() >= 25);
        tokio::time::sleep(std::time::Duration::from_millis(160)).await;
        assert!(
            app.commands_matching("PropPixels").await >= 2,
            "overlay frames are sent"
        );
        let (_, s1) = app
            .json(
                "POST",
                &format!("/faultfinder/{session}/answer"),
                Some(json!({"lit": false})),
            )
            .await;
        let (_, s2) = app
            .json(
                "POST",
                &format!("/faultfinder/{session}/answer"),
                Some(json!({"ok": true})),
            )
            .await;
        assert_ne!(s1["litTo"], s2["litTo"]);
        let (_, back) = app
            .json("POST", &format!("/faultfinder/{session}/undo"), None)
            .await;
        assert_eq!(back["litTo"], s1["litTo"]);
        let mut last = back;
        for _ in 0..20 {
            if last["done"] == true {
                break;
            }
            last = app
                .json(
                    "POST",
                    &format!("/faultfinder/{session}/answer"),
                    Some(json!({"ok": false})),
                )
                .await
                .1;
        }
        assert_eq!(last["done"], true);
        assert_eq!(last["result"]["pixelIndex"], 0);
        assert_eq!(
            app.json("POST", "/faultfinder/stop", None).await.0,
            StatusCode::OK
        );
        let (s, e) = app
            .json(
                "POST",
                &format!("/faultfinder/{session}/answer"),
                Some(json!({"ok": true})),
            )
            .await;
        assert_eq!(s, StatusCode::CONFLICT, "{e}");
        let n = app.commands_matching("PropPixels").await;
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert_eq!(
            app.commands_matching("PropPixels").await,
            n,
            "frames stop after stop"
        );
    }

    #[tokio::test]
    async fn misc_endpoints() {
        let mut app = TestApp::new();
        leader(&mut app, None).await;
        let (s, schema) = app.json("GET", "/effects/schema", None).await;
        assert_eq!(s, StatusCode::OK);
        assert!(schema["rainbow"].is_array());
        assert_eq!(
            app.json("GET", "/effects/catalog", None)
                .await
                .1
                .as_array()
                .unwrap()
                .len(),
            13
        );
        let (s, games) = app.json("GET", "/games/status", None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(games["available"], false);
        assert_eq!(games["running"], false);
        assert_eq!(
            app.json("POST", "/games/invite", None).await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        let (s, tts) = app.json("GET", "/tts/status", None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(tts["mode"], "browser");
        assert!(tts["voices"].as_array().unwrap().len() > 20);
        let (s, health) = app.json("POST", "/health/run", None).await;
        assert_eq!(s, StatusCode::OK);
        assert!(health["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == "sequences"));
        let (s, prev) = app.json("GET", "/schedule/preview?days=7", None).await;
        assert_eq!(s, StatusCode::OK);
        assert!(prev.is_array());
        let (s, logs) = {
            let req = Request::builder()
                .uri("/api/v1/system/logs?lines=10")
                .body(Body::empty())
                .unwrap();
            let (s, _, b) = app.send(req).await;
            (s, String::from_utf8(b).unwrap())
        };
        assert_eq!(s, StatusCode::OK);
        assert!(!logs.is_empty());
        let (s, t) = app
            .json("POST", "/alerts/test", Some(json!({"channel": "email"})))
            .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(t["ok"], false);
        let (s, u) = app.json("GET", "/system/update", None).await;
        assert_eq!(s, StatusCode::OK);
        assert!(u["current"].is_string());
        let (s, n) = app
            .json(
                "PUT",
                "/system/network",
                Some(json!({"hostname": "bad name!"})),
            )
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{n}");
        // Props bulk + reorder.
        let (_, a) = app
            .json(
                "POST",
                "/props",
                Some(json!({"name": "A", "kind": "line", "pixelCount": 10, "channelStart": 0})),
            )
            .await;
        let (_, b) = app
            .json(
                "POST",
                "/props",
                Some(json!({"name": "B", "kind": "line", "pixelCount": 10, "channelStart": 30})),
            )
            .await;
        let (a, b) = (
            a["id"].as_str().unwrap().to_string(),
            b["id"].as_str().unwrap().to_string(),
        );
        let (s, _) = app
            .json("POST", "/props/reorder", Some(json!({"ids": [b, a]})))
            .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(app.state.store.get().props[0].id, b);
        let (s, props) = app
            .json("POST", "/props/bulk", Some(json!({"ops": [{"op": "update", "id": a, "patch": {"color": "#ff0000"}}, {"op": "delete", "id": b}]})))
            .await;
        assert_eq!(s, StatusCode::OK, "{props}");
        assert_eq!(props.as_array().unwrap().len(), 1);
        assert_eq!(props[0]["color"], "#ff0000");
        let (s, _) = app
            .json(
                "POST",
                "/props/bulk",
                Some(json!({"ops": [{"op": "update", "id": a, "patch": {"pixelCount": 0}}]})),
            )
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        // Triggers.
        let (s, _) = app
            .json("PUT", "/show/settings", Some(json!({"triggers": [{"id": "t1", "name": "Big red button", "kind": "http", "action": {"type": "stop"}}]})))
            .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(
            app.json("POST", "/triggers/t1/fire", None).await.0,
            StatusCode::OK
        );
        assert_eq!(app.commands_matching("Stop").await, 1);
        assert_eq!(
            app.json("POST", "/triggers/nope/fire", None).await.0,
            StatusCode::NOT_FOUND
        );
        // Overlay needs a matrix prop.
        let (s, e) = app
            .json(
                "POST",
                &format!("/overlay/{a}/text"),
                Some(json!({"text": "Hi"})),
            )
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{e}");
        assert_eq!(
            app.json("POST", "/triggers/t1", None).await.0,
            StatusCode::OK
        );
        // Matrix prop: overlays and the games test pattern.
        let map: Vec<i32> = (0..32).collect();
        let (s, m) = app
            .json(
                "POST",
                "/props",
                Some(
                    json!({"name": "Matrix", "kind": "matrix", "pixelCount": 32, "channelStart": 90,
                "matrix": {"width": 8, "height": 4, "pixelMap": map}}),
                ),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{m}");
        let mid = m["id"].as_str().unwrap().to_string();
        let (s, info) = app
            .json("POST", &format!("/overlay/{mid}/open"), None)
            .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(info["width"], 8);
        assert_eq!(
            app.json(
                "POST",
                &format!("/overlay/{mid}/text"),
                Some(json!({"text": "Merry Christmas", "color": "#ff0000"}))
            )
            .await
            .0,
            StatusCode::OK
        );
        let (s, e) = app
            .json(
                "POST",
                &format!("/overlay/{mid}/qr"),
                Some(json!({"url": "https://example.com/request"})),
            )
            .await;
        assert_eq!(
            s,
            StatusCode::BAD_REQUEST,
            "an 8x4 matrix is too small for a QR code: {e}"
        );
        let req = Request::builder()
            .method("PUT")
            .uri(format!("/api/v1/overlay/{mid}/frame"))
            .body(Body::from(vec![0u8; 8 * 4 * 3]))
            .unwrap();
        assert_eq!(app.send(req).await.0, StatusCode::OK);
        let req = Request::builder()
            .method("PUT")
            .uri(format!("/api/v1/overlay/{mid}/frame"))
            .body(Body::from(vec![0u8; 5]))
            .unwrap();
        assert_eq!(app.send(req).await.0, StatusCode::BAD_REQUEST);
        let (s, t) = app
            .json("POST", "/games/test-pattern", Some(json!({"propId": mid})))
            .await;
        assert_eq!(s, StatusCode::OK, "{t}");
        assert!(app.commands_matching("PropPixels").await >= 1);
        let g = crate::api::games::test_pattern_grid(8, 4);
        assert_eq!(g.get(0, 0).unwrap().to_array(), [255, 0, 0]);
        assert_eq!(g.get(7, 0).unwrap().to_array(), [0, 255, 0]);
        // PUT /show/name returns the show.
        let (s, show) = app
            .json("PUT", "/show/name", Some(json!({"name": "Renamed"})))
            .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(show["name"], "Renamed");
    }
}

#[cfg(test)]
mod platform_api_tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn public_health_is_open_and_cheap() {
        let mut app = TestApp::new();
        let (s, h) = app.json("GET", "/public/health", None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(h["ok"], true);
        assert_eq!(h["role"], "unconfigured");
        assert_eq!(h["version"], env!("CARGO_PKG_VERSION"));
        // Still open once a password protects everything else.
        app.state
            .store
            .update(|s| {
                s.settings.security.password_hash =
                    Some(crate::api::auth::hash_password("jingle").unwrap());
                Ok(())
            })
            .await
            .unwrap();
        app.cookie = None;
        assert_eq!(
            app.json("GET", "/show", None).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.json("GET", "/public/health", None).await.0,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn platform_endpoints_degrade_gracefully() {
        let app = TestApp::new();
        let (s, v) = app.json("GET", "/system/helpers", None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v, json!([]));
        let (s, g) = app.json("GET", "/system/output-geometry", None).await;
        assert_eq!(s, StatusCode::OK, "{g}");
        assert!(g["ok"].is_boolean());
        // Nothing to apply when the strings fit.
        if g["ok"] == true {
            let (s, _) = app
                .json("POST", "/system/output-geometry/apply", Some(json!({})))
                .await;
            assert_eq!(s, StatusCode::CONFLICT);
        }
        let (s, ssh) = app.json("GET", "/system/ssh", None).await;
        assert_eq!(s, StatusCode::OK);
        assert!(ssh.get("canChange").is_some());
        let (s, n) = app.json("GET", "/system/network", None).await;
        assert_eq!(s, StatusCode::OK);
        assert!(n.get("netwatch").is_some(), "{n}");
        let (s, info) = app.json("GET", "/system", None).await;
        assert_eq!(s, StatusCode::OK);
        assert!(info["platform"]["helper"].is_boolean());
        assert!(info["outputGeometry"]["ok"].is_boolean());
        // Invalid helper arguments never reach systemctl.
        let (s, _) = app
            .json("PUT", "/system/ssh", Some(json!({"enabled": "yes"})))
            .await;
        assert!(s.is_client_error());
    }
}
