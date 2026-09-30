//! Browser sequence preview (F3, ARCHITECTURE §12.3). Leader only; never
//! touches the lights.
//!
//! * `GET /sequences/:id/preview` → the `PPPV` header JSON when the cached
//!   preview is ready (`ETag` = its mapping hash), else **202**
//!   `{jobId, pct, state}` while it is built (WS `job` messages report
//!   progress). Works for `tmp-…` auto-show previews too.
//! * `GET /sequences/:id/preview/data` → the whole `.pppv` file, with
//!   `Accept-Ranges`/`Range` (the browser fetches blocks lazily) and `ETag`.

use super::{ApiError, ApiResult};
use crate::services::analysis::{self as svc, PreviewLookup};
use crate::state::AppState;
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use tower::ServiceExt;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/sequences/{id}/preview", get(header_json))
        .route("/sequences/{id}/preview/data", get(data))
        .route("/sequences/{id}/preview/block/{n}", get(block))
}

fn etag(hash: &str) -> HeaderValue {
    HeaderValue::from_str(&format!("\"{hash}\"")).unwrap_or(HeaderValue::from_static("\"\""))
}

async fn header_json(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Response> {
    let s = state.clone();
    let found = tokio::task::spawn_blocking(move || svc::preview(&s, &id))
        .await
        .map_err(ApiError::internal)?;
    match found {
        PreviewLookup::Ready { header, .. } => {
            let tag = etag(&header.mapping_hash);
            let mut resp = Json(&*header).into_response();
            resp.headers_mut().insert(header::ETAG, tag);
            Ok(resp)
        }
        PreviewLookup::Building(job) => Ok((
            StatusCode::ACCEPTED,
            Json(json!({ "jobId": job.id, "pct": job.pct, "state": job.state })),
        )
            .into_response()),
        PreviewLookup::Missing(why) => Err(ApiError::new(StatusCode::NOT_FOUND, "not_found", why)),
    }
}

async fn data(
    State(state): State<AppState>,
    Path(id): Path<String>,
    req: Request,
) -> ApiResult<Response> {
    let s = state.clone();
    let found = tokio::task::spawn_blocking(move || svc::preview(&s, &id))
        .await
        .map_err(ApiError::internal)?;
    let PreviewLookup::Ready { path, header } = found else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "not_ready",
            "The preview isn't ready yet. Ask for /preview first.",
        ));
    };
    let resp = tower_http::services::ServeFile::new_with_mime(
        path,
        &"application/octet-stream"
            .parse()
            .map_err(ApiError::internal)?,
    )
    .oneshot(req)
    .await
    .map_err(ApiError::internal)?;
    let mut resp = resp.map(axum::body::Body::new);
    let h = resp.headers_mut();
    h.insert(header::ETAG, etag(&header.mapping_hash));
    // The blocks are gzip already: stop the compression layer from wrapping
    // them again (and from breaking Range offsets).
    h.insert(
        header::CONTENT_ENCODING,
        HeaderValue::from_static("identity"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(resp)
}

/// One gzip block (for clients that can't send Range headers).
async fn block(
    State(state): State<AppState>,
    Path((id, n)): Path<(String, usize)>,
) -> ApiResult<Response> {
    let s = state.clone();
    let found = tokio::task::spawn_blocking(move || svc::preview(&s, &id))
        .await
        .map_err(ApiError::internal)?;
    let PreviewLookup::Ready { path, header } = found else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "not_ready",
            "The preview isn't ready yet. Ask for /preview first.",
        ));
    };
    let b = *header
        .blocks
        .get(n)
        .ok_or_else(|| ApiError::not_found("That part of the preview"))?;
    let bytes = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = std::fs::File::open(path)?;
        f.seek(SeekFrom::Start(b.offset))?;
        let mut buf = vec![0u8; b.len as usize];
        f.read_exact(&mut buf)?;
        Ok(buf)
    })
    .await
    .map_err(ApiError::internal)??;
    let mut resp = bytes.into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    h.insert(header::ETAG, etag(&header.mapping_hash));
    h.insert(
        header::CONTENT_ENCODING,
        HeaderValue::from_static("identity"),
    );
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::super::autoshow::tests::{app_with_props, wait_job};
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use pixelplus_core::fseq::{FseqWriter, FseqWriterOptions};
    use serde_json::json;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn preview_is_built_once_then_served_with_ranges() {
        let app = app_with_props().await;
        // A 510-channel, 25 ms sequence of 200 frames.
        let rel = "sequences/s1.fseq";
        let mut w = FseqWriter::create(app.dir.join(rel), FseqWriterOptions::new(510, 25)).unwrap();
        for f in 0..200u32 {
            let frame: Vec<u8> = (0..510).map(|c| ((f + c) % 256) as u8).collect();
            w.write_frame(&frame).unwrap();
        }
        w.finish().unwrap();
        let hash = pixelplus_core::fseq::sha256_file(app.dir.join(rel)).unwrap();
        app.state
            .store
            .update(move |show| {
                show.sequences.push(
                    serde_json::from_value(json!({"id":"s1","name":"S","file":"sequences/s1.fseq","durationMs":5000,"frameMs":25,"channelCount":510,"hash":hash})).unwrap(),
                );
                Ok(())
            })
            .await
            .unwrap();
        let (st, _) = app.json("GET", "/sequences/nope/preview", None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let (st, _) = app.json("GET", "/sequences/s1/preview/data", None).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "not built yet");
        let (st, r) = app.json("GET", "/sequences/s1/preview", None).await;
        assert_eq!(st, StatusCode::ACCEPTED, "{r}");
        // Asking again while building gives the same job.
        let (_, r2) = app.json("GET", "/sequences/s1/preview", None).await;
        assert_eq!(r2["jobId"], r["jobId"]);
        let done = wait_job(&app, r["jobId"].as_str().unwrap()).await;
        assert_eq!(done["state"], "done", "{done}");
        let (st, h) = app.json("GET", "/sequences/s1/preview", None).await;
        assert_eq!(st, StatusCode::OK, "{h}");
        assert_eq!(h["frameMs"], 50);
        assert_eq!(h["frameCount"], 100);
        assert_eq!(h["frameBytes"], 510);
        assert_eq!(h["props"].as_array().unwrap().len(), 3);
        let blocks = h["blocks"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);

        // Range request for block 1 returns exactly that gzip block.
        let off = blocks[1]["offset"].as_u64().unwrap();
        let len = blocks[1]["len"].as_u64().unwrap();
        let req = Request::builder()
            .uri("/api/v1/sequences/s1/preview/data")
            .header(header::RANGE, format!("bytes={off}-{}", off + len - 1))
            .body(Body::empty())
            .unwrap();
        let (st, headers, body) = app.send(req).await;
        assert_eq!(st, StatusCode::PARTIAL_CONTENT);
        assert_eq!(body.len() as u64, len);
        assert_eq!(&body[..2], &[0x1f, 0x8b], "a gzip member");
        assert_eq!(
            headers[header::ETAG].to_str().unwrap(),
            format!("\"{}\"", h["mappingHash"].as_str().unwrap())
        );
        assert_eq!(headers[header::CONTENT_ENCODING], "identity");
        let raw = pixelplus_core::preview::inflate_block(&body).unwrap();
        assert_eq!(raw.len(), 36 * 510, "frames 64..100");
        // Frame 64 = source frame 128; tree pixel 0 = channels 0..3.
        assert_eq!(&raw[..3], &[128, 129, 130]);
        // The same block by number.
        let req = Request::builder()
            .uri("/api/v1/sequences/s1/preview/block/1")
            .body(Body::empty())
            .unwrap();
        let (st, _, b1) = app.send(req).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(b1, body);
        let (st, _) = app.json("GET", "/sequences/s1/preview/block/9", None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);

        // A layout change makes a new preview (new mapping hash).
        app.state
            .store
            .update(|show| {
                show.props[2].pixel_count = 19;
                Ok(())
            })
            .await
            .unwrap();
        let (st, r) = app.json("GET", "/sequences/s1/preview", None).await;
        assert_eq!(st, StatusCode::ACCEPTED);
        wait_job(&app, r["jobId"].as_str().unwrap()).await;
        let (st, h2) = app.json("GET", "/sequences/s1/preview", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_ne!(h2["mappingHash"], h["mappingHash"]);
        let files = std::fs::read_dir(app.dir.join("cache/preview"))
            .unwrap()
            .count();
        assert_eq!(files, 1, "the stale preview was removed");

        // Deleting the sequence removes its previews.
        let (st, _) = app.json("DELETE", "/sequences/s1", None).await;
        assert_eq!(st, StatusCode::OK);
        let files = std::fs::read_dir(app.dir.join("cache/preview"))
            .unwrap()
            .count();
        assert_eq!(files, 0);
    }
}
