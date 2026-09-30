//! Contract tests for the FPP Connect subset: they replay the requests
//! xLights makes (`src-core/controllers/FPP.cpp`: `AuthenticateAndUpdateVersions`,
//! discovery, `CheckUploadMedia`, `PrepareUploadSequence`, `uploadFileV7`,
//! `UploadPlaylist`, `Restart`) with the same headers and bodies, and apply
//! xLights' own decisions (FPP detection, skip-unchanged) to the answers.

use super::*;
use crate::api::testkit::{make_fseq, TestApp};
use axum::body::Body;
use axum::http::{Method, Request};
use serde_json::json;

const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_14_1) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/70.0.3538.77 Safari/537.36";

async fn enable(app: &TestApp, upload_password: Option<&str>, admin_password: bool) {
    let hash = upload_password.map(|p| crate::api::auth::hash_password(p).unwrap());
    let admin = admin_password.then(|| crate::api::auth::hash_password("sign-in-pw").unwrap());
    app.state
        .store
        .update(move |s| {
            s.settings.xlights.fpp_connect = true;
            s.settings.xlights.password_hash = hash;
            s.settings.security.password_hash = admin;
            Ok(())
        })
        .await
        .unwrap();
}

/// A request as xLights' curl sends it (no PixelPlus CSRF header; `send`
/// adds one, which the FPP routes ignore).
fn xreq(method: Method, uri: &str) -> axum::http::request::Builder {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("user-agent", UA)
        .header("host", "192.168.1.40")
}

async fn get(app: &TestApp, uri: &str) -> (StatusCode, HeaderMap, Vec<u8>) {
    app.send(xreq(Method::GET, uri).body(Body::empty()).unwrap())
        .await
}

fn basic(pw: &str) -> String {
    // xLights sends whatever user name is configured; PixelPlus ignores it.
    let raw = format!("admin:{pw}");
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in raw.as_bytes().chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    format!("Basic {out}")
}

/// One `uploadFileV7` chunk: headers exactly as `prepareCurlForMulti`.
async fn patch_chunk(
    app: &TestApp,
    dir: &str,
    name: &str,
    total: u64,
    offset: u64,
    chunk: &[u8],
    auth: Option<&str>,
) -> (StatusCode, Value) {
    let mut b = xreq(Method::PATCH, &format!("/api/file/{dir}"))
        .header("content-type", "application/offset+octet-stream")
        .header("x-requested-with", "FPPConnect")
        .header("connection", "keep-alive")
        .header("upload-offset", offset.to_string())
        .header("upload-length", total.to_string())
        .header("upload-name", name)
        .header("content-length", chunk.len().to_string());
    if let Some(a) = auth {
        b = b.header("authorization", basic(a));
    }
    let (st, _, body) = app.send(b.body(Body::from(chunk.to_vec())).unwrap()).await;
    (st, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// `uploadFileV7` as a loop, restarting from 0 on a non-200 up to 3 times.
async fn upload_v7(
    app: &TestApp,
    dir: &str,
    name: &str,
    data: &[u8],
    block: usize,
    auth: Option<&str>,
) -> StatusCode {
    let total = data.len() as u64;
    let mut offset = 0usize;
    let mut errors = 0;
    loop {
        let end = (offset + block).min(data.len());
        let (st, _) = patch_chunk(
            app,
            dir,
            name,
            total,
            offset as u64,
            &data[offset..end],
            auth,
        )
        .await;
        if st != StatusCode::OK {
            if errors < 3 {
                offset = 0;
                errors += 1;
                continue;
            }
            return st;
        }
        offset = end;
        if offset >= data.len() {
            return st;
        }
    }
}

/// xLights `FPP::parseConfig` (character for character).
fn xlights_parse_config(v: &str) -> std::collections::HashMap<String, String> {
    let mut settings = std::collections::HashMap::new();
    for line in v.split('\n') {
        let to = line.trim_start();
        if to.len() >= 8 && &to[..8] == "settings" {
            let to = &to[10..];
            let i = to.find('\'').unwrap();
            let key = to[..i].to_string();
            let to = &to[to.find('"').unwrap() + 1..];
            let to = &to[..to.find(';').unwrap() - 1];
            settings.insert(key, to.to_string());
        }
    }
    settings
}

/// xLights `PrepareUploadSequence` skip decision for a full V2 zstd file.
fn xlights_would_upload(meta: Option<&Value>, f: &FseqFile) -> bool {
    let Some(m) = meta else { return true };
    let h = f.header();
    let version = m["Version"].as_str().unwrap_or("");
    let mut up = version.starts_with('1');
    let ct = m["CompressionType"].as_i64().unwrap_or(1);
    if ct != 1 {
        up = true;
    }
    if m["ID"].as_str() != Some(h.unique_id.to_string().as_str()) {
        up = true;
    }
    if m["NumFrames"].as_u64() != Some(u64::from(h.frame_count)) {
        up = true;
    }
    if m["StepTime"].as_i64() != Some(i64::from(h.step_time_ms)) {
        up = true;
    }
    if m["MaxChannel"].as_u64() != Some(u64::from(h.channel_count))
        || m["ChannelCount"].as_u64() != Some(u64::from(h.channel_count))
    {
        up = true;
    }
    if m.get("Ranges").is_some() {
        up = true;
    }
    up
}

fn fseq_bytes(app: &TestApp, pixels: u32, frames: u32, media: Option<&str>) -> Vec<u8> {
    let p = app.dir.join(format!("src-{}.fseq", new_id()));
    make_fseq(&p, pixels, frames, media);
    let b = std::fs::read(&p).unwrap();
    std::fs::remove_file(&p).unwrap();
    b
}

#[tokio::test]
async fn invisible_until_enabled_and_lan_only() {
    let app = TestApp::new();
    let (st, _, _) = get(&app, "/config.php").await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, _) = get(&app, "/api/system/info").await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    enable(&app, None, false).await;
    let (st, _, _) = get(&app, "/config.php").await;
    assert_eq!(st, StatusCode::OK);
    // From the internet (or through a tunnel): invisible.
    let mut req = xreq(Method::GET, "/config.php")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo::<std::net::SocketAddr>(
            "8.8.8.8:5000".parse().unwrap(),
        ));
    let (st, _, _) = app.send(req).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let req = xreq(Method::GET, "/config.php")
        .header("x-forwarded-for", "8.8.8.8")
        .body(Body::empty())
        .unwrap();
    let (st, _, _) = app.send(req).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // The SPA and /api/v1 are unaffected by the root routes.
    let (st, _, _) = get(&app, "/api/v1/system").await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn detection_and_discovery_match_xlights() {
    let app = TestApp::new();
    enable(&app, None, false).await;
    // AuthenticateAndUpdateVersions: config.php → FPP type only for "Falcon Player".
    let (st, h, body) = get(&app, "/config.php").await;
    assert_eq!(st, StatusCode::OK);
    assert!(h[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("text/javascript"));
    let settings = xlights_parse_config(&String::from_utf8(body).unwrap());
    assert!(settings["Title"].contains("Falcon Player"), "{settings:?}");
    assert!(!settings["HostName"].is_empty());
    // parseSysInfo
    let (st, _, body) = get(&app, "/api/system/info").await;
    assert_eq!(st, StatusCode::OK);
    let v: Value = serde_json::from_slice(&body).unwrap();
    for k in [
        "Platform",
        "Variant",
        "Version",
        "HostName",
        "HostDescription",
        "Mode",
        "uuid",
    ] {
        assert!(v[k].is_string(), "sysinfo {k}: {v}");
    }
    assert_eq!(v["Mode"], "player");
    assert!(v.get("channelRanges").is_none(), "full files, please");
    let version = v["Version"].as_str().unwrap();
    let major: u32 = version.split('.').next().unwrap().parse().unwrap();
    assert!(
        major >= 7 && v["majorVersion"] == major,
        "FPP ≥ 7.1 upload path"
    );
    assert!(v["typeId"].as_u64().unwrap() < 0x80 && v.get("typId").is_some());
    // ProcessFPPSystems (FPP ≥ 6 format).
    let (st, _, body) = get(&app, "/api/fppd/multiSyncSystems").await;
    assert_eq!(st, StatusCode::OK);
    let v: Value = serde_json::from_slice(&body).unwrap();
    let sys = &v["systems"][0];
    assert_eq!(sys["address"], "192.168.1.40");
    assert!(sys["address"].as_str().unwrap().len() <= 16);
    assert_eq!(sys["typeId"], 1);
    assert_eq!(sys["fppModeString"], "player");
    assert!(sys["uuid"].as_str().unwrap().starts_with("PixelPlus-"));
    // Discovery side calls: all harmless.
    for (p, want) in [
        ("/api/channel/output/co-pixelStrings", StatusCode::OK),
        ("/api/channel/output/channelOutputsJSON", StatusCode::OK),
        ("/api/channel/output/co-other", StatusCode::OK),
        ("/api/playlists", StatusCode::OK),
        ("/api/proxies", StatusCode::OK),
        ("/api/cape", StatusCode::NOT_FOUND),
        ("/api/configfile/ci-universes.json", StatusCode::NOT_FOUND),
    ] {
        let (st, _, body) = get(&app, p).await;
        assert_eq!(st, want, "{p}");
        if st == StatusCode::OK {
            let v: Value = serde_json::from_slice(&body).unwrap();
            if p.starts_with("/api/channel/output/") {
                assert_eq!(v["channelOutputs"], json!([]));
            } else {
                assert!(v.is_array(), "{p}: {v}");
            }
        }
    }
}

#[tokio::test]
async fn sequence_and_song_upload_replay() {
    let app = TestApp::new();
    enable(&app, None, false).await;
    let seq = fseq_bytes(&app, 60, 40, Some("C:\\Users\\me\\Music\\Jingle Bells.wav"));
    let wav_path = app.dir.join("Jingle Bells.wav");
    crate::services::media::tests::sine_wav(&wav_path, 2.0, 0.4);
    let wav = std::fs::read(&wav_path).unwrap();

    // CheckUploadMedia: meta 404 → upload to "music".
    let (st, _, _) = get(&app, "/api/media/Jingle%20Bells.wav/meta").await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(
        upload_v7(&app, "music", "Jingle Bells.wav", &wav, 30_000, None).await,
        StatusCode::OK
    );
    // Now xLights would skip it: format.size equals the local size.
    let (st, _, body) = get(&app, "/api/media/Jingle%20Bells.wav/meta").await;
    assert_eq!(st, StatusCode::OK);
    let m: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(m["format"]["size"], wav.len().to_string());

    // PrepareUploadSequence: meta 404 → FinalizeUploadSequence uploads to "sequences".
    let (st, _, _) = get(&app, "/api/sequence/Jingle%20Bells.fseq/meta").await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(
        upload_v7(&app, "sequences", "Jingle Bells.fseq", &seq, 1000, None).await,
        StatusCode::OK
    );
    let show = app.state.store.get();
    let s = show
        .sequences
        .iter()
        .find(|s| s.xlights_name.as_deref() == Some("Jingle Bells.fseq"))
        .expect("imported");
    let first_id = s.id.clone();
    assert!(
        s.media_id.is_some(),
        "linked to its song by the fseq media header"
    );
    // Meta now makes xLights skip the unchanged file…
    let (st, _, body) = get(&app, "/api/sequence/Jingle%20Bells.fseq/meta").await;
    assert_eq!(st, StatusCode::OK);
    let meta: Value = serde_json::from_slice(&body).unwrap();
    let local_file = {
        let p = app.dir.join("check.fseq");
        std::fs::write(&p, &seq).unwrap();
        FseqFile::open(&p).unwrap()
    };
    assert!(!xlights_would_upload(Some(&meta), &local_file), "{meta}");
    // …and upload a re-rendered one (new unique id), replaced in place.
    let seq2 = fseq_bytes(&app, 60, 50, Some("Jingle Bells.wav"));
    let p2 = app.dir.join("check2.fseq");
    std::fs::write(&p2, &seq2).unwrap();
    assert!(xlights_would_upload(
        Some(&meta),
        &FseqFile::open(&p2).unwrap()
    ));
    assert_eq!(
        upload_v7(&app, "sequences", "Jingle Bells.fseq", &seq2, 4096, None).await,
        StatusCode::OK
    );
    let show = app.state.store.get();
    let same: Vec<_> = show
        .sequences
        .iter()
        .filter(|s| s.xlights_name.as_deref() == Some("Jingle Bells.fseq"))
        .collect();
    assert_eq!(same.len(), 1);
    assert_eq!(same[0].id, first_id, "replaced in place, id kept");

    // UploadPlaylist: GET (404 → empty object), then POST the FPP JSON.
    let (st, _, _) = get(&app, "/api/playlist/Christmas%202026").await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let pl = json!({
        "mainPlaylist": [{
            "type": "both", "enabled": 1, "playOnce": 0,
            "sequenceName": "Jingle Bells.fseq", "mediaName": "Jingle Bells.wav",
            "videoOut": "--Default--", "duration": 2.0
        }, {
            "type": "sequence", "enabled": 1, "playOnce": 0,
            "sequenceName": "Not Uploaded.fseq", "duration": 3.0
        }],
        "name": "Christmas 2026", "random": 0,
        "playlistInfo": {"total_items": 2, "total_duration": 5.0}
    });
    let post = || {
        xreq(Method::POST, "/api/playlist/Christmas%202026")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string_pretty(&pl).unwrap()))
            .unwrap()
    };
    let (st, _, body) = app.send(post()).await;
    assert_eq!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let resp: Value = serde_json::from_slice(&body).unwrap();
    assert!(resp["Message"]
        .as_str()
        .unwrap()
        .contains("Not Uploaded.fseq"));
    // Posting again adds nothing twice.
    let (st, _, _) = app.send(post()).await;
    assert_eq!(st, StatusCode::OK);
    let show = app.state.store.get();
    let p = show
        .playlists
        .iter()
        .find(|p| p.name == "Christmas 2026")
        .unwrap();
    assert_eq!(p.items.len(), 1);
    let (st, _, body) = get(&app, "/api/playlist/Christmas%202026").await;
    assert_eq!(st, StatusCode::OK);
    let back: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(back["mainPlaylist"][0]["sequenceName"], "Jingle Bells.fseq");
    assert_eq!(back["mainPlaylist"][0]["type"], "both");
    let (_, _, body) = get(&app, "/api/playlists").await;
    assert!(serde_json::from_slice::<Vec<String>>(&body)
        .unwrap()
        .contains(&"Christmas 2026".to_string()));

    // Controller configuration is accepted and ignored; Restart is harmless.
    for (m, p, ct, b) in [
        (Method::POST, "/api/models", "application/json", "[]"),
        (
            Method::POST,
            "/api/channel/output/universeOutputs",
            "application/json",
            "{}",
        ),
        (Method::POST, "/api/proxies", "application/json", "[]"),
        (Method::PUT, "/api/settings/restartFlag", "text/plain", "0"),
        (
            Method::POST,
            "/api/configfile/virtualdisplaymap",
            "application/octet-stream",
            "x",
        ),
    ] {
        let req = xreq(m, p)
            .header("content-type", ct)
            .body(Body::from(b))
            .unwrap();
        let (st, _, _) = app.send(req).await;
        assert_eq!(st, StatusCode::OK, "{p}");
    }
    let (st, _, _) = get(&app, "/api/system/fppd/restart?quick=1").await;
    assert_eq!(st, StatusCode::OK);

    // The uploads log shows what happened.
    let (_, status) = app.json("GET", "/xlights/status", None).await;
    let names: Vec<&str> = status["uploads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["Jingle Bells.fseq", "Jingle Bells.fseq", "Jingle Bells.wav"]
    );
    assert_eq!(status["uploads"][0]["replaced"], true);
    assert_eq!(status["ready"], true);
}

#[tokio::test]
async fn chunked_upload_resume_edges() {
    let app = TestApp::new();
    enable(&app, None, false).await;
    let data = fseq_bytes(&app, 50, 30, None);
    let total = data.len() as u64;
    let cut = data.len() / 2;
    let (a, b) = data.split_at(cut);
    // First chunk.
    let (st, v) = patch_chunk(&app, "sequences", "Resume.fseq", total, 0, a, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["offset"], cut);
    // A gap or an overlap is refused (xLights then restarts at 0).
    let (st, _) = patch_chunk(
        &app,
        "sequences",
        "Resume.fseq",
        total,
        cut as u64 - 1,
        b,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    let (st, _) = patch_chunk(
        &app,
        "sequences",
        "Resume.fseq",
        total,
        cut as u64 + 10,
        &b[10..],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    // A different total length for the same name is a different file.
    let (st, _) = patch_chunk(
        &app,
        "sequences",
        "Resume.fseq",
        total + 1,
        cut as u64,
        b,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    // More bytes than announced.
    let mut too_much = b.to_vec();
    too_much.push(0);
    let (st, _) = patch_chunk(
        &app,
        "sequences",
        "Resume.fseq",
        total,
        cut as u64,
        &too_much,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::PAYLOAD_TOO_LARGE);
    // The right continuation completes it (the refused tries left no trace).
    let (st, v) = patch_chunk(&app, "sequences", "Resume.fseq", total, cut as u64, b, None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert!(app
        .state
        .store
        .get()
        .sequences
        .iter()
        .any(|s| s.xlights_name.as_deref() == Some("Resume.fseq")));
    // Restart from 0 after an interrupted upload works.
    let (st, _) = patch_chunk(&app, "sequences", "Again.fseq", total, 0, a, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        upload_v7(&app, "sequences", "Again.fseq", &data, 700, None).await,
        StatusCode::OK
    );
    // A broken file is reported, not imported.
    let junk = vec![7u8; 5000];
    let (st, v) = patch_chunk(&app, "sequences", "Junk.fseq", 5000, 0, &junk, None).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    // Videos and effect sequences: refused with a reason.
    let (st, v) = patch_chunk(&app, "videos", "Clip.mp4", 10, 0, b"0123456789", None).await;
    assert_eq!(st, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert!(v["message"].as_str().unwrap().contains("videos"));
    let (st, _) = patch_chunk(&app, "virtualdisplay_assets", "bg.png", 3, 0, b"png", None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = patch_chunk(&app, "config", "x", 1, 0, b"x", None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = patch_chunk(&app, "../../etc", "x", 1, 0, b"x", None).await;
    assert!(st.is_client_error(), "{st}");
    // Missing headers / names that are only a path.
    let (st, _) = patch_chunk(&app, "sequences", "../", 1, 0, b"x", None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // Oversized announcements.
    let (st, _) = patch_chunk(
        &app,
        "music",
        "Huge.mp3",
        content::AUDIO_MAX + 1,
        0,
        b"x",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn legacy_upload_and_move() {
    let app = TestApp::new();
    enable(&app, None, false).await;
    let data = fseq_bytes(&app, 20, 10, None);
    let req = xreq(Method::POST, "/api/file/uploads/Old%20Style.fseq")
        .header("content-type", "application/octet-stream")
        .header("x-requested-with", "FPPConnect")
        .body(Body::from(data))
        .unwrap();
    let (st, _, _) = app.send(req).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _, _) = get(&app, "/api/file/move/Old%20Style.fseq").await;
    assert_eq!(st, StatusCode::OK);
    assert!(app
        .state
        .store
        .get()
        .sequences
        .iter()
        .any(|s| s.xlights_name.as_deref() == Some("Old Style.fseq")));
    let (st, _, _) = get(&app, "/api/file/move/Never%20Sent.fseq").await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn auth_and_csrf_matrix() {
    let app = TestApp::new();
    let data = fseq_bytes(&app, 10, 5, None);
    let total = data.len() as u64;

    // No passwords at all (LAN-trust): PATCH passes, a CORS-simple POST doesn't.
    enable(&app, None, false).await;
    let (st, _) = patch_chunk(&app, "sequences", "A.fseq", total, 0, &data, None).await;
    assert_eq!(st, StatusCode::OK);
    let req = xreq(Method::POST, "/api/playlist/X")
        .header("content-type", "text/plain")
        .body(Body::from("{}"))
        .unwrap();
    let (st, _, _) = app.send(req).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // The show has a sign-in password but no upload password: writes refused
    // (reads still work so xLights can find us and show the error).
    enable(&app, None, true).await;
    let (st, v) = patch_chunk(&app, "sequences", "B.fseq", total, 0, &data, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{v}");
    let (st, _, _) = get(&app, "/config.php").await;
    assert_eq!(st, StatusCode::OK);

    // Upload password: 401 challenge (curl then retries with Basic), wrong
    // password 401, right one (any user name) 200.
    enable(&app, Some("xl-upload-1"), true).await;
    let req = xreq(Method::PATCH, "/api/file/sequences")
        .header("upload-offset", "0")
        .header("upload-length", total.to_string())
        .header("upload-name", "C.fseq")
        .body(Body::from(data.clone()))
        .unwrap();
    let (st, h, _) = app.send(req).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert!(h[header::WWW_AUTHENTICATE]
        .to_str()
        .unwrap()
        .starts_with("Basic"));
    let (st, _) = patch_chunk(
        &app,
        "sequences",
        "C.fseq",
        total,
        0,
        &data,
        Some("wrong-pw"),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _) = patch_chunk(
        &app,
        "sequences",
        "C.fseq",
        total,
        0,
        &data,
        Some("xl-upload-1"),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    // The admin API sets and clears it (write-only; never returned). The
    // test client isn't signed in, so drop the sign-in password first.
    enable(&app, Some("xl-upload-1"), false).await;
    let (st, _) = app
        .json(
            "PUT",
            "/xlights/password",
            Some(json!({"password": "short"})),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, v) = app
        .json(
            "PUT",
            "/xlights/password",
            Some(json!({"password": "new-upload-pw"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (_, show) = app.json("GET", "/show", None).await;
    assert_eq!(show["settings"]["xlights"]["passwordHash"], "");
    let (st, _) = patch_chunk(
        &app,
        "sequences",
        "D.fseq",
        total,
        0,
        &data,
        Some("new-upload-pw"),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (_, s) = app.json("GET", "/xlights/status", None).await;
    assert_eq!(
        (s["passwordSet"].as_bool(), s["ready"].as_bool()),
        (Some(true), Some(true))
    );
    let (st, _) = app
        .json("PUT", "/xlights/password", Some(json!({"password": ""})))
        .await;
    assert_eq!(st, StatusCode::OK);
    let (_, s) = app.json("GET", "/xlights/status", None).await;
    assert_eq!(
        (s["passwordSet"].as_bool(), s["ready"].as_bool()),
        (Some(false), Some(true))
    );
    // With a sign-in password the status explains what's missing.
    app.state
        .store
        .update(|s| {
            s.settings.security.password_hash =
                Some(crate::api::auth::hash_password("sign-in-pw").unwrap());
            Ok(())
        })
        .await
        .unwrap();
    let status = admin_status(State(app.state.clone())).await.0;
    assert!(!status.ready);
    assert!(status.reason.unwrap().contains("upload password"));
}

#[test]
fn names_dirs_and_folders() {
    assert_eq!(
        clean_name("C:\\shows\\Wizards.fseq").as_deref(),
        Some("Wizards.fseq")
    );
    assert_eq!(clean_name("../../etc/passwd").as_deref(), Some("passwd"));
    assert_eq!(clean_name(".."), None);
    assert_eq!(clean_name(""), None);
    assert_eq!(clean_name("a\u{0}b.fseq").as_deref(), Some("ab.fseq"));
    assert_eq!(classify_dir("sequences"), Some(Dir::Sequences));
    assert!(matches!(classify_dir("videos"), Some(Dir::Unsupported(_))));
    assert_eq!(classify_dir("uploads"), None);
    assert_eq!(kind_for_name("Song.MP3"), Dir::Music);
    assert_eq!(kind_for_name("Show.fseq"), Dir::Sequences);
    assert!(watch_folder_ok("/srv/xlights-drop"));
    assert!(watch_folder_ok("/var/lib/pixelplus/xlights-drop"));
    assert!(!watch_folder_ok("relative/dir"));
    assert!(!watch_folder_ok("/"));
    assert!(!watch_folder_ok("/etc/pixelplus"));
    assert!(!watch_folder_ok("/proc"));
    assert_eq!(config_value("My \"Show\"; <b>"), "My Show b");
}

#[tokio::test]
async fn watch_folder_imports_finished_files() {
    let app = TestApp::new();
    let folder = app.dir.join("xlights-drop");
    std::fs::create_dir_all(&folder).unwrap();
    make_fseq(&folder.join("Dropped.fseq"), 30, 20, None);
    std::fs::write(folder.join("notes.txt"), "x").unwrap();
    std::fs::write(folder.join("Broken.fseq"), [1u8; 100]).unwrap();
    // Make the files look old enough.
    let old = SystemTime::now() - Duration::from_secs(60);
    for f in ["Dropped.fseq", "Broken.fseq"] {
        std::fs::File::options()
            .write(true)
            .open(folder.join(f))
            .unwrap()
            .set_modified(old)
            .unwrap();
    }
    let mut seen = Seen::default();
    scan_once(&app.state, &folder, &mut seen).await.unwrap();
    assert!(
        app.state.store.get().sequences.is_empty(),
        "first scan only notes sizes"
    );
    scan_once(&app.state, &folder, &mut seen).await.unwrap();
    let show = app.state.store.get();
    assert!(show
        .sequences
        .iter()
        .any(|s| s.xlights_name.as_deref() == Some("Dropped.fseq")));
    assert!(folder.join("imported/Dropped.fseq").exists());
    assert!(folder.join("failed/Broken.fseq").exists());
    assert!(
        folder.join("notes.txt").exists(),
        "other files are left alone"
    );
    let log = read_log(&app.state);
    assert_eq!(log.len(), 2);
    assert!(log.iter().all(|e| e.source == "folder"));
}
