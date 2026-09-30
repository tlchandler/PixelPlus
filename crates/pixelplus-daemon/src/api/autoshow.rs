//! Auto light shows and beat analysis (F2, ARCHITECTURE §12.2).
//!
//! | Method & path | |
//! |---|---|
//! | `GET /autoshow/styles` | `[{id, name, description}]` |
//! | `POST /autoshow` | `{mediaId, style, propIds?, seed?, name?}` → `{jobId, seed}`; the job's result is `{sequenceId}` |
//! | `POST /autoshow/preview` | same body → `{jobId, seed}`; result `{sequenceId: "tmp-…"}` (previewable for an hour) |
//! | `POST /sequences/:id/regenerate` | `{style?, seed?, propIds?}` → `{jobId}` (generated sequences only; same id) |
//! | `GET /media/:id/analysis` | the full analysis JSON (404 until ready) |
//! | `POST /media/:id/analyze` | re-run the analysis → `{jobId}` |
//! | `GET /jobs`, `GET /jobs/:id` | recent / one job status (same shape as the WS `job` message) |

use super::{ApiError, ApiResult};
use crate::services::analysis::{self as svc, AutoShowTask, AutoTarget};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use pixelplus_core::autoshow;
use pixelplus_core::model::MediaKind;
use serde::Deserialize;
use serde_json::{json, Value};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/autoshow/styles", get(styles))
        .route("/autoshow", post(create))
        .route("/autoshow/preview", post(preview))
        .route("/sequences/{id}/regenerate", post(regenerate))
        .route("/media/{id}/analysis", get(analysis))
        .route("/media/{id}/analyze", post(analyze))
        .route("/jobs", get(jobs))
        .route("/jobs/{id}", get(job))
}

async fn styles() -> Json<Vec<autoshow::StyleInfo>> {
    Json(autoshow::styles())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutoShowBody {
    media_id: String,
    #[serde(default)]
    style: Option<String>,
    #[serde(default)]
    prop_ids: Vec<String>,
    #[serde(default)]
    seed: Option<u32>,
    #[serde(default)]
    name: Option<String>,
}

/// Validate a request; returns (style, seed, prop ids).
fn check(state: &AppState, b: &AutoShowBody) -> ApiResult<(String, u32, Vec<String>)> {
    let show = state.store.get();
    let m = show
        .media_item(&b.media_id)
        .ok_or_else(|| ApiError::bad_request("Pick a song first."))?;
    if m.duration_ms < 1000 {
        return Err(ApiError::bad_request(
            "That audio is too short for a light show.",
        ));
    }
    if m.duration_ms > autoshow::MAX_DURATION_MS {
        return Err(ApiError::bad_request(
            "That audio is longer than 20 minutes. Pick a single song.",
        ));
    }
    let style = b.style.clone().unwrap_or_else(|| {
        if m.kind == MediaKind::Dj {
            "voice".into()
        } else {
            "classic".into()
        }
    });
    if !autoshow::style_exists(&style) {
        return Err(ApiError::bad_request(format!(
            "\"{style}\" isn't a style PixelPlus knows."
        )));
    }
    let props: Vec<String> = b
        .prop_ids
        .iter()
        .filter(|id| show.props.iter().any(|p| &p.id == *id))
        .cloned()
        .collect();
    if !b.prop_ids.is_empty() && props.is_empty() {
        return Err(ApiError::bad_request(
            "Those props no longer exist. Pick props again.",
        ));
    }
    if autoshow::selected_props(&show, &props).is_empty() {
        return Err(ApiError::bad_request(
            "There are no props to light yet. Add props on the Props page first.",
        ));
    }
    let seed = b.seed.unwrap_or_else(rand::random::<u32>) % 1_000_000;
    Ok((style, seed, props))
}

async fn create(
    State(state): State<AppState>,
    Json(b): Json<AutoShowBody>,
) -> ApiResult<Json<Value>> {
    let (style, seed, prop_ids) = check(&state, &b)?;
    let id = svc::enqueue_autoshow(
        &state,
        AutoShowTask {
            media_id: b.media_id.clone(),
            style,
            prop_ids,
            seed,
            target: AutoTarget::New {
                name: b.name.clone().filter(|n| n.len() <= 200),
            },
        },
        true,
    )
    .ok_or_else(|| ApiError::unavailable("Couldn't start. Try again in a moment."))?;
    Ok(Json(json!({ "jobId": id, "seed": seed })))
}

async fn preview(
    State(state): State<AppState>,
    Json(b): Json<AutoShowBody>,
) -> ApiResult<Json<Value>> {
    let (style, seed, prop_ids) = check(&state, &b)?;
    let tmp = format!("tmp-{}", pixelplus_core::model::new_id());
    let id = svc::enqueue_autoshow(
        &state,
        AutoShowTask {
            media_id: b.media_id.clone(),
            style,
            prop_ids,
            seed,
            target: AutoTarget::Temp(tmp.clone()),
        },
        true,
    )
    .ok_or_else(|| ApiError::unavailable("Couldn't start. Try again in a moment."))?;
    Ok(Json(
        json!({ "jobId": id, "seed": seed, "sequenceId": tmp }),
    ))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegenerateBody {
    #[serde(default)]
    style: Option<String>,
    #[serde(default)]
    seed: Option<u32>,
    #[serde(default)]
    prop_ids: Option<Vec<String>>,
}

async fn regenerate(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<RegenerateBody>>,
) -> ApiResult<Json<Value>> {
    let b = body.map(|Json(b)| b).unwrap_or_default();
    let show = state.store.get();
    let seq = show
        .sequence(&id)
        .ok_or_else(|| ApiError::not_found("That sequence"))?;
    let g = seq.generated.clone().ok_or_else(|| {
        ApiError::bad_request("Only light shows PixelPlus made can be made again.")
    })?;
    let req = AutoShowBody {
        media_id: g.media_id.clone(),
        style: Some(b.style.unwrap_or(g.style)),
        prop_ids: b.prop_ids.unwrap_or(g.prop_ids),
        seed: Some(b.seed.unwrap_or(g.seed)),
        name: None,
    };
    let (style, seed, prop_ids) = check(&state, &req)?;
    let job = svc::enqueue_autoshow(
        &state,
        AutoShowTask {
            media_id: req.media_id,
            style,
            prop_ids,
            seed,
            target: AutoTarget::Regenerate(id),
        },
        true,
    )
    .ok_or_else(|| ApiError::unavailable("Couldn't start. Try again in a moment."))?;
    Ok(Json(json!({ "jobId": job, "seed": seed })))
}

async fn analysis(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Response> {
    if state.store.get().media_item(&id).is_none() {
        return Err(ApiError::not_found("That audio file"));
    }
    let path =
        svc::analysis_path(&state, &id).ok_or_else(|| ApiError::not_found("That audio file"))?;
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(_) => {
            svc::enqueue_analysis(&state, &id);
            return Err(ApiError::new(
                axum::http::StatusCode::NOT_FOUND,
                "not_ready",
                "The beat analysis isn't ready yet. It runs in the background after an upload.",
            ));
        }
    };
    let mut resp = bytes.into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(resp)
}

async fn analyze(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    if state.store.get().media_item(&id).is_none() {
        return Err(ApiError::not_found("That audio file"));
    }
    let job = svc::enqueue_analysis_now(&state, &id)
        .ok_or_else(|| ApiError::unavailable("Couldn't start. Try again in a moment."))?;
    Ok(Json(json!({ "jobId": job })))
}

async fn jobs(State(state): State<AppState>) -> Json<Vec<svc::JobStatus>> {
    Json(svc::active_jobs(&state))
}

async fn job(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<svc::JobStatus>> {
    svc::job(&state, &id)
        .map(Json)
        .ok_or_else(|| ApiError::not_found("That job"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::testkit::{Part, TestApp};
    use axum::http::StatusCode;
    use serde_json::{json, Value};
    use std::time::Duration;

    /// A 16-bit mono WAV click track (120 BPM, kicks on the bar).
    pub fn click_wav(seconds: f32) -> Vec<u8> {
        let rate = 22_050u32;
        let n = (seconds * rate as f32) as usize;
        let mut s = vec![0f32; n];
        let period = 0.5f32;
        let mut t = 0.25f32;
        let mut k = 0;
        while t < seconds - 0.1 {
            let start = (t * rate as f32) as usize;
            for i in 0..(0.12 * rate as f32) as usize {
                if start + i >= n {
                    break;
                }
                let tt = i as f32 / rate as f32;
                let click = (std::f32::consts::TAU * 2000.0 * tt).sin() * (-tt / 0.008).exp() * 0.5;
                let kick = if k % 4 == 0 {
                    (std::f32::consts::TAU * 60.0 * tt).sin() * (-tt / 0.05).exp() * 0.6
                } else {
                    0.0
                };
                s[start + i] += click + kick;
            }
            t += period;
            k += 1;
        }
        let mut data = Vec::with_capacity(n * 2);
        for v in s {
            data.extend_from_slice(&((v.clamp(-1.0, 1.0) * 30000.0) as i16).to_le_bytes());
        }
        let mut f = Vec::new();
        f.extend_from_slice(b"RIFF");
        f.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        f.extend_from_slice(b"WAVEfmt ");
        f.extend_from_slice(&16u32.to_le_bytes());
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&rate.to_le_bytes());
        f.extend_from_slice(&(rate * 2).to_le_bytes());
        f.extend_from_slice(&2u16.to_le_bytes());
        f.extend_from_slice(&16u16.to_le_bytes());
        f.extend_from_slice(b"data");
        f.extend_from_slice(&(data.len() as u32).to_le_bytes());
        f.extend_from_slice(&data);
        f
    }

    /// Wait until a job is done (or failed); returns its final status.
    pub async fn wait_job(app: &TestApp, id: &str) -> Value {
        for _ in 0..600 {
            let (st, j) = app.json("GET", &format!("/jobs/{id}"), None).await;
            assert_eq!(st, StatusCode::OK, "{j}");
            if j["state"] == "done" || j["state"] == "failed" {
                return j;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("job {id} did not finish");
    }

    pub async fn app_with_props() -> TestApp {
        let app = TestApp::new();
        crate::services::analysis::start(&app.state);
        app.state
            .store
            .update(|show| {
                show.props = vec![
                    serde_json::from_value(json!({"id":"tree","name":"Tree","kind":"tree","pixelCount":100,"channelStart":0,"channelsPerPixel":3,"segments":[],"groupIds":[]})).unwrap(),
                    serde_json::from_value(json!({"id":"arch","name":"Arch","kind":"arch","pixelCount":50,"channelStart":300,"channelsPerPixel":3,"segments":[],"groupIds":[]})).unwrap(),
                    serde_json::from_value(json!({"id":"cane","name":"Cane","kind":"candycane","pixelCount":20,"channelStart":450,"channelsPerPixel":3,"segments":[],"groupIds":[]})).unwrap(),
                ];
                Ok(())
            })
            .await
            .unwrap();
        app
    }

    pub async fn upload_song(app: &TestApp, seconds: f32) -> String {
        let (st, m) = app
            .upload(
                "/media",
                &[Part {
                    name: "file",
                    filename: Some("Click Track.wav"),
                    data: click_wav(seconds),
                }],
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{m}");
        m["id"].as_str().unwrap().to_string()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn upload_is_analysed_then_an_auto_show_is_made_and_regenerated() {
        let app = app_with_props().await;
        let (st, styles) = app.json("GET", "/autoshow/styles", None).await;
        assert_eq!(st, StatusCode::OK);
        assert!(styles.as_array().unwrap().len() >= 5);
        assert!(styles[0]["name"].is_string() && styles[0]["description"].is_string());

        let mid = upload_song(&app, 12.0).await;
        // The upload queued the analysis: wait for the summary to appear.
        let mut summary = Value::Null;
        for _ in 0..600 {
            let (_, m) = app.json("GET", &format!("/media/{mid}"), None).await;
            if m["analysis"].is_object() {
                summary = m["analysis"].clone();
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            (summary["bpm"].as_f64().unwrap() - 120.0).abs() < 1.5,
            "{summary}"
        );
        assert!(summary["beatCount"].as_u64().unwrap() >= 18);
        let (st, full) = app
            .json("GET", &format!("/media/{mid}/analysis"), None)
            .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(full["v"], 1);
        assert!(full["beats"].as_array().unwrap().len() >= 18);
        assert!(full["energy10Hz"]["rms"].as_array().unwrap().len() >= 100);

        // Validation.
        let (st, e) = app
            .json("POST", "/autoshow", Some(json!({"mediaId": "nope"})))
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{e}");
        let (st, _) = app
            .json(
                "POST",
                "/autoshow",
                Some(json!({"mediaId": mid, "style": "disco"})),
            )
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);

        // Create.
        let (st, r) = app
            .json(
                "POST",
                "/autoshow",
                Some(json!({"mediaId": mid, "style": "candy", "seed": 42})),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{r}");
        assert_eq!(r["seed"], 42);
        let done = wait_job(&app, r["jobId"].as_str().unwrap()).await;
        assert_eq!(done["state"], "done", "{done}");
        assert_eq!(done["kind"], "autoshow");
        let sid = done["result"]["sequenceId"].as_str().unwrap().to_string();
        let (_, seq) = app.json("GET", &format!("/sequences/{sid}"), None).await;
        assert_eq!(seq["mediaId"], mid.as_str());
        assert_eq!(seq["frameMs"], 25);
        assert_eq!(seq["channelCount"], 510);
        assert_eq!(seq["tags"], json!(["auto"]));
        assert_eq!(seq["generated"]["kind"], "autoShow");
        assert_eq!(seq["generated"]["style"], "candy");
        assert_eq!(seq["generated"]["seed"], 42);
        assert!(seq["name"].as_str().unwrap().contains("Candy"));
        assert!(app.dir.join(seq["file"].as_str().unwrap()).is_file());
        let hash = seq["hash"].as_str().unwrap().to_string();

        // Regenerate with the same inputs: same id, identical file.
        let (st, r) = app
            .json(
                "POST",
                &format!("/sequences/{sid}/regenerate"),
                Some(json!({})),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{r}");
        let done = wait_job(&app, r["jobId"].as_str().unwrap()).await;
        assert_eq!(done["state"], "done", "{done}");
        let (_, seq2) = app.json("GET", &format!("/sequences/{sid}"), None).await;
        assert_eq!(seq2["hash"].as_str().unwrap(), hash, "deterministic");
        // Another seed changes it.
        let (_, r) = app
            .json(
                "POST",
                &format!("/sequences/{sid}/regenerate"),
                Some(json!({"seed": 7})),
            )
            .await;
        wait_job(&app, r["jobId"].as_str().unwrap()).await;
        let (_, seq3) = app.json("GET", &format!("/sequences/{sid}"), None).await;
        assert_ne!(seq3["hash"], seq2["hash"]);
        assert_eq!(seq3["id"], sid.as_str());

        // Uploaded sequences can't be regenerated.
        let (st, _) = app.json("POST", "/sequences/none/regenerate", None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);

        // Deleting the song removes its analysis.
        let (st, _) = app.json("DELETE", &format!("/media/{mid}"), None).await;
        assert_eq!(st, StatusCode::OK);
        assert!(!app.dir.join(format!("media/{mid}.analysis.json")).exists());
        let (st, _) = app
            .json("GET", &format!("/media/{mid}/analysis"), None)
            .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn preview_before_create_then_reanalyse() {
        let app = app_with_props().await;
        let mid = upload_song(&app, 6.0).await;
        let (st, r) = app
            .json(
                "POST",
                "/autoshow/preview",
                Some(json!({"mediaId": mid, "style": "party", "propIds": ["tree"]})),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{r}");
        let tmp = r["sequenceId"].as_str().unwrap().to_string();
        assert!(tmp.starts_with("tmp-"));
        let done = wait_job(&app, r["jobId"].as_str().unwrap()).await;
        assert_eq!(done["state"], "done", "{done}");
        assert_eq!(done["result"]["sequenceId"], tmp.as_str());
        // The preview of the temporary show is ready at once.
        let (st, h) = app
            .json("GET", &format!("/sequences/{tmp}/preview"), None)
            .await;
        assert_eq!(st, StatusCode::OK, "{h}");
        assert_eq!(h["seqId"], tmp.as_str());
        assert_eq!(h["frameMs"], 50);
        // Nothing was added to the library.
        let (_, seqs) = app.json("GET", "/sequences", None).await;
        assert_eq!(seqs.as_array().unwrap().len(), 0);

        let (st, r) = app
            .json("POST", &format!("/media/{mid}/analyze"), None)
            .await;
        assert_eq!(st, StatusCode::OK);
        let done = wait_job(&app, r["jobId"].as_str().unwrap()).await;
        assert_eq!(done["state"], "done");
        assert_eq!(done["kind"], "analysis");
        let (st, _) = app.json("POST", "/media/zzz/analyze", None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let (st, list) = app.json("GET", "/jobs", None).await;
        assert_eq!(st, StatusCode::OK);
        assert!(list.is_array());
    }
}
