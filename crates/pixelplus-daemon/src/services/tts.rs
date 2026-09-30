//! DJ voices: talks to the Kokoro TTS sidecar (`tts/`, loopback HTTP) and
//! renders DJ clips into media files.
//!
//! Mode resolution (`settings.tts.mode`): `auto` → `device` when the sidecar
//! answers `/health` with the model installed, else `browser` (the web UI
//! renders with kokoro-js and uploads the audio).

use crate::api::{ApiError, ApiResult};
use crate::state::AppState;
use parking_lot::Mutex;
use pixelplus_core::model::{DjClip, DjLine, Media, MediaKind, Show, TtsMode};
use pixelplus_core::template::{render_template, TemplateContext};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn client() -> &'static reqwest::Client {
    static C: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(300))
            .no_proxy()
            .build()
            .expect("HTTP client")
    })
}

static HEALTH: Mutex<Option<(Instant, Option<Value>)>> = Mutex::new(None);

/// Sidecar `/health` (cached 10 s). `None` when unreachable.
pub async fn health(state: &AppState) -> Option<Value> {
    if let Some((at, v)) = HEALTH.lock().clone() {
        if at.elapsed() < Duration::from_secs(10) {
            return v;
        }
    }
    let url = format!("{}/health", state.config.tts_url.trim_end_matches('/'));
    let v = match client().get(&url).timeout(Duration::from_secs(3)).send().await {
        Ok(r) if r.status().is_success() => r.json::<Value>().await.ok(),
        _ => None,
    };
    *HEALTH.lock() = Some((Instant::now(), v.clone()));
    v
}

/// On-device rendering possible right now.
pub async fn device_available(state: &AppState) -> bool {
    health(state)
        .await
        .is_some_and(|h| h["ok"].as_bool().unwrap_or(true) && h["modelAvailable"].as_bool().unwrap_or(true))
}

/// Effective mode: "device" or "browser".
pub async fn resolved_mode(state: &AppState) -> &'static str {
    match state.store.get().settings.tts.mode {
        TtsMode::Browser => "browser",
        TtsMode::Device => "device",
        TtsMode::Auto => {
            if device_available(state).await {
                "device"
            } else {
                "browser"
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VoiceInfo {
    pub id: String,
    pub name: String,
    pub language: String,
    pub gender: String,
}

/// Kokoro v1.0 voices (used when the sidecar isn't reachable).
pub fn static_voices() -> Vec<VoiceInfo> {
    const V: &[(&str, &str, &str, &str)] = &[
        ("af_heart", "Heart", "en-us", "female"),
        ("af_alloy", "Alloy", "en-us", "female"),
        ("af_aoede", "Aoede", "en-us", "female"),
        ("af_bella", "Bella", "en-us", "female"),
        ("af_jessica", "Jessica", "en-us", "female"),
        ("af_kore", "Kore", "en-us", "female"),
        ("af_nicole", "Nicole", "en-us", "female"),
        ("af_nova", "Nova", "en-us", "female"),
        ("af_river", "River", "en-us", "female"),
        ("af_sarah", "Sarah", "en-us", "female"),
        ("af_sky", "Sky", "en-us", "female"),
        ("am_adam", "Adam", "en-us", "male"),
        ("am_echo", "Echo", "en-us", "male"),
        ("am_eric", "Eric", "en-us", "male"),
        ("am_fenrir", "Fenrir", "en-us", "male"),
        ("am_liam", "Liam", "en-us", "male"),
        ("am_michael", "Michael", "en-us", "male"),
        ("am_onyx", "Onyx", "en-us", "male"),
        ("am_puck", "Puck", "en-us", "male"),
        ("am_santa", "Santa", "en-us", "male"),
        ("bf_alice", "Alice", "en-gb", "female"),
        ("bf_emma", "Emma", "en-gb", "female"),
        ("bf_isabella", "Isabella", "en-gb", "female"),
        ("bf_lily", "Lily", "en-gb", "female"),
        ("bm_daniel", "Daniel", "en-gb", "male"),
        ("bm_fable", "Fable", "en-gb", "male"),
        ("bm_george", "George", "en-gb", "male"),
        ("bm_lewis", "Lewis", "en-gb", "male"),
    ];
    V.iter()
        .map(|(id, name, lang, g)| VoiceInfo { id: (*id).into(), name: (*name).into(), language: (*lang).into(), gender: (*g).into() })
        .collect()
}

/// `{base, presets}` from the sidecar, or the static list.
pub async fn voices(state: &AppState) -> Value {
    let url = format!("{}/voices", state.config.tts_url.trim_end_matches('/'));
    if let Ok(r) = client().get(&url).timeout(Duration::from_secs(5)).send().await {
        if r.status().is_success() {
            if let Ok(v) = r.json::<Value>().await {
                return v;
            }
        }
    }
    json!({ "base": static_voices(), "presets": super::seed::builtin_voices() })
}

pub async fn status(state: &AppState) -> Value {
    let mode = resolved_mode(state).await;
    let health = health(state).await;
    let base: Vec<VoiceInfo> = match &health {
        Some(_) => serde_json::from_value(voices(state).await["base"].clone()).unwrap_or_else(|_| static_voices()),
        None => static_voices(),
    };
    json!({
        "mode": mode,
        "configuredMode": state.store.get().settings.tts.mode,
        "available": if mode == "device" { health.is_some() } else { true },
        "deviceAvailable": device_available(state).await,
        "voices": base,
        "device": health,
    })
}

/// Placeholder values at render time.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicContext {
    #[serde(default)]
    pub next_song: Option<String>,
    #[serde(default)]
    pub prev_song: Option<String>,
    #[serde(default)]
    pub request_name: Option<String>,
    /// °F (spoken as "N degrees").
    #[serde(default)]
    pub temperature: Option<f64>,
}

fn template_context(show: &Show, ctx: &DynamicContext) -> TemplateContext {
    let tz = pixelplus_core::schedule::schedule_timezone(&show.schedule).unwrap_or(chrono_tz::UTC);
    let now = chrono::Utc::now().with_timezone(&tz);
    let loc = &show.schedule.location;
    let mut t = TemplateContext::new(now).with_location(loc.lat, loc.lon);
    t.show_name = Some(show.name.clone());
    t.next_song = ctx.next_song.clone();
    t.prev_song = ctx.prev_song.clone();
    t.request_name = ctx.request_name.clone();
    t.temperature = ctx.temperature;
    t
}

/// Context from the player (next/previous song).
pub fn context_from_player(state: &AppState) -> DynamicContext {
    let mut ctx = DynamicContext::default();
    if let Some(p) = state.services.player.get() {
        let st = p.status();
        ctx.next_song = st.next_item.filter(|i| i.kind == "sequence" || i.kind == "request").map(|i| i.name);
        ctx.prev_song = st.item.filter(|i| i.kind == "sequence" || i.kind == "request").map(|i| i.name);
    }
    ctx
}

/// Resolve a line's voice: a show DjVoice id/name becomes the full object;
/// anything else (Kokoro id, preset alias) is passed through.
fn resolve_voice(show: &Show, voice: &Value) -> Value {
    if let Some(v) = voice.as_str() {
        if let Some(dv) = show
            .dj_voices
            .iter()
            .find(|d| d.id == v || d.name.eq_ignore_ascii_case(v))
        {
            return serde_json::to_value(dv).unwrap_or_else(|_| voice.clone());
        }
    }
    voice.clone()
}

/// Build the sidecar `/render` body.
pub fn render_body(show: &Show, lines: &[Value], speed: f32, ctx: &DynamicContext, format: &str) -> Value {
    let tctx = template_context(show, ctx);
    let lines: Vec<Value> = lines
        .iter()
        .map(|l| {
            let mut l = l.clone();
            if let Some(obj) = l.as_object_mut() {
                if let Some(v) = obj.get("voice").cloned() {
                    obj.insert("voice".into(), resolve_voice(show, &v));
                }
                if let Some(text) = obj.get("text").and_then(Value::as_str) {
                    let rendered = render_template(text, &tctx);
                    obj.insert("text".into(), Value::String(rendered));
                }
            }
            l
        })
        .collect();
    json!({
        "lines": lines,
        "voices": show.dj_voices,
        "speed": speed.clamp(0.5, 2.0),
        "pronunciations": show.pronunciations,
        "builtinPronunciations": true,
        "placeholders": {
            "showName": show.name,
            "nextSong": ctx.next_song,
            "prevSong": ctx.prev_song,
            "requestName": ctx.request_name,
        },
        "format": format,
        "loudnessLufs": show.settings.audio.target_lufs,
        "fx": true,
    })
}

/// Rendered audio + the sidecar's measurements.
pub struct Rendered {
    pub bytes: bytes::Bytes,
    pub content_type: String,
    pub duration_ms: Option<u64>,
    pub loudness_lufs: Option<f32>,
    pub warnings: Option<String>,
}

async fn sidecar_error(r: reqwest::Response) -> ApiError {
    let status = r.status();
    let body: Value = r.json().await.unwrap_or(Value::Null);
    let msg = body["error"]["message"].as_str().map(str::to_string).unwrap_or_else(|| format!("the voice service answered {status}"));
    let code = body["error"]["code"].as_str().unwrap_or("");
    match code {
        "model_missing" => ApiError::unavailable("The voice model isn't installed on this controller yet. Use browser voices, or install it with the PixelPlus TTS installer."),
        "busy" => ApiError::unavailable("The voice engine is busy with other clips. Try again in a minute."),
        _ if status.is_client_error() => ApiError::bad_request(msg),
        _ => ApiError::unavailable(format!("The voice engine couldn't render that: {msg}")),
    }
}

/// POST to the sidecar and return audio.
pub async fn post_audio(state: &AppState, path: &str, body: &Value) -> ApiResult<Rendered> {
    let url = format!("{}{path}", state.config.tts_url.trim_end_matches('/'));
    let r = client().post(&url).json(body).send().await.map_err(|e| {
        if e.is_timeout() {
            ApiError::unavailable("The voice engine took too long. Shorter clips render faster on a Pi.")
        } else {
            ApiError::unavailable("On-device voices aren't available on this controller. Your browser can render the clip instead.")
        }
    })?;
    if !r.status().is_success() {
        return Err(sidecar_error(r).await);
    }
    let h = r.headers().clone();
    let get = |k: &str| h.get(k).and_then(|v| v.to_str().ok()).map(str::to_string);
    let content_type = get("content-type").unwrap_or_else(|| "audio/mpeg".into());
    let duration_ms = get("x-duration-ms").and_then(|v| v.parse().ok());
    let loudness_lufs = get("x-loudness-lufs").and_then(|v| v.parse().ok());
    let warnings = get("x-warnings");
    let bytes = r.bytes().await.map_err(|e| ApiError::unavailable(format!("The voice engine stopped mid-render: {e}")))?;
    Ok(Rendered { bytes, content_type, duration_ms, loudness_lufs, warnings })
}

fn clip_lines(clip: &DjClip) -> Vec<Value> {
    clip.lines.iter().map(|l: &DjLine| serde_json::to_value(l).unwrap_or(Value::Null)).collect()
}

fn with_bed(state: &AppState, show: &Show, clip: &DjClip, mut body: Value) -> Value {
    if let Some(bed) = clip.music_bed_media_id.as_deref().and_then(|id| show.media_item(id)) {
        let path = state.config.data_dir.join(&bed.file);
        body["musicBed"] = json!({ "path": path.to_string_lossy(), "duckDb": 12, "gainDb": -6, "introMs": 1500, "outroMs": 2500 });
    }
    body
}

/// Render a clip on the device and store it as the clip's media (kind `dj`).
pub async fn render_clip(state: &AppState, clip_id: &str) -> ApiResult<DjClip> {
    let show = state.store.get();
    let clip = show.dj_clip(clip_id).cloned().ok_or_else(|| ApiError::not_found("That DJ clip"))?;
    if clip.lines.iter().all(|l| l.text.trim().is_empty()) {
        return Err(ApiError::bad_request("Write something for the DJ to say first."));
    }
    if resolved_mode(state).await != "device" {
        return Err(ApiError::unavailable(
            "On-device voices aren't available on this controller. Your browser can render the clip instead.",
        ));
    }
    let ctx = context_from_player(state);
    let body = with_bed(state, &show, &clip, render_body(&show, &clip_lines(&clip), clip.speed, &ctx, "mp3"));
    let r = post_audio(state, "/render", &body).await?;
    save_clip_audio(state, &clip, &r.bytes, "mp3", r.duration_ms, r.loudness_lufs).await
}

/// Store audio for a clip (reusing the clip's own dj media id when it has one).
pub async fn save_clip_audio(
    state: &AppState,
    clip: &DjClip,
    audio: &[u8],
    ext: &str,
    duration_ms: Option<u64>,
    loudness_lufs: Option<f32>,
) -> ApiResult<DjClip> {
    let show = state.store.get();
    let media_dir = state.config.media_dir();
    let existing = clip.media_id.as_deref().and_then(|id| show.media_item(id)).filter(|m| m.kind == MediaKind::Dj).cloned();
    let id = existing.as_ref().map(|m| m.id.clone()).unwrap_or_else(pixelplus_core::model::new_id);
    let rel = format!("media/{id}.{ext}");
    let path = state.config.data_dir.join(&rel);
    tokio::fs::create_dir_all(&media_dir).await?;
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, audio).await?;
    tokio::fs::rename(&tmp, &path).await?;
    if let Some(old) = &existing {
        if old.file != rel {
            let _ = tokio::fs::remove_file(state.config.data_dir.join(&old.file)).await;
        }
    }
    // Measure what the sidecar didn't tell us.
    let p = path.clone();
    let meta = tokio::task::spawn_blocking(move || super::media::analyze(&p)).await.map_err(ApiError::internal)?;
    let mut meta = meta.unwrap_or_default();
    if let Some(d) = duration_ms {
        meta.duration_ms = d;
    }
    if loudness_lufs.is_some() {
        meta.loudness_lufs = loudness_lufs;
    }
    meta.original_name = format!("{}.{ext}", clip.name);
    let _ = super::media::write_meta(&media_dir, &id, &meta);
    let target = show.settings.audio.target_lufs;
    let media = Media {
        id: id.clone(),
        name: format!("DJ: {}", clip.name),
        kind: MediaKind::Dj,
        file: rel,
        duration_ms: meta.duration_ms,
        loudness_lufs: meta.loudness_lufs,
        gain_db: super::media::gain_for(target, meta.loudness_lufs),
    };
    let clip_id = clip.id.clone();
    let (clip, _) = state
        .store
        .update(move |s| {
            match s.media.iter_mut().find(|m| m.id == media.id) {
                Some(m) => *m = media.clone(),
                None => s.media.push(media.clone()),
            }
            let c = s.dj_clips.iter_mut().find(|c| c.id == clip_id).ok_or_else(|| ApiError::not_found("That DJ clip"))?;
            c.media_id = Some(media.id.clone());
            Ok(c.clone())
        })
        .await?;
    Ok(clip)
}

/// Render a dynamic clip for showtime with live placeholder values. The
/// file is written to `media/live-<clipId>.mp3` (not added to the show).
/// For the player: call shortly before the clip is due; on error, fall back
/// to the clip's last rendered `mediaId`.
pub async fn render_dynamic_clip(state: &AppState, clip_id: &str, ctx: DynamicContext) -> anyhow::Result<PathBuf> {
    let show = state.store.get();
    let clip = show.dj_clip(clip_id).cloned().ok_or_else(|| anyhow::anyhow!("DJ clip {clip_id} not found"))?;
    let body = with_bed(state, &show, &clip, render_body(&show, &clip_lines(&clip), clip.speed, &ctx, "mp3"));
    let r = post_audio(state, "/render", &body).await.map_err(|e| anyhow::anyhow!(e.message))?;
    let path = state.config.media_dir().join(format!("live-{clip_id}.mp3"));
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, &r.bytes).await?;
    tokio::fs::rename(&tmp, &path).await?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_body_resolves_voices_and_placeholders() {
        let mut show = Show::default();
        show.name = "Chandler Lights".into();
        show.dj_voices = super::super::seed::builtin_voices();
        let lines = vec![
            json!({"voice": "nick", "text": "Welcome to {showName}!", "pauseMs": 0}),
            json!({"voice": "af_sky", "text": "Up next: {nextSong}", "pauseMs": 300}),
        ];
        let ctx = DynamicContext { next_song: Some("Feliz Navidad".into()), ..Default::default() };
        let b = render_body(&show, &lines, 1.0, &ctx, "mp3");
        assert_eq!(b["lines"][0]["voice"]["blend"]["am_puck"], 0.4);
        assert_eq!(b["lines"][0]["text"], "Welcome to Chandler Lights!");
        assert_eq!(b["lines"][1]["voice"], "af_sky");
        assert_eq!(b["lines"][1]["text"], "Up next: Feliz Navidad");
        assert_eq!(b["loudnessLufs"], -16.0);
    }
}
