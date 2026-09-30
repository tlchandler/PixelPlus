//! Nightly reports (F11, ARCHITECTURE §12.10).
//!
//! * `GET /reports?limit=30` → `ReportSummary[]`, newest first
//! * `GET /reports/:date` → `NightReport` (404 until made)
//! * `GET /reports/:date/email` → the HTML email as sent (preview)
//! * `POST /reports/run {date?, send?}` → make (and send) a report now;
//!   `date` defaults to the latest night (last night before noon, tonight
//!   after). Sending ignores "only when there are problems".
//!
//! The logic lives in `services/reports.rs`.

use super::{ApiError, ApiResult};
use crate::services::reports::{self as svc, NightReport, ReportSummary};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::NaiveDate;
use serde::Deserialize;

fn parse_date(s: &str) -> ApiResult<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| ApiError::bad_request("The date must look like 2026-12-24."))
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    limit: Option<usize>,
}

async fn list(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Vec<ReportSummary>>> {
    let limit = q.limit.unwrap_or(30).clamp(1, 400);
    let dir = svc::dir(&state.config.data_dir);
    let reports = tokio::task::spawn_blocking(move || svc::list(&dir, limit))
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(reports.iter().map(NightReport::summary).collect()))
}

async fn load(state: &AppState, date: &str) -> ApiResult<NightReport> {
    let date = parse_date(date)?;
    let dir = svc::dir(&state.config.data_dir);
    tokio::task::spawn_blocking(move || svc::load(&dir, date))
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("The report for that night"))
}

async fn get_one(
    State(state): State<AppState>,
    Path(date): Path<String>,
) -> ApiResult<Json<NightReport>> {
    Ok(Json(load(&state, &date).await?))
}

async fn email_preview(
    State(state): State<AppState>,
    Path(date): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let r = load(&state, &date).await?;
    let show = state.store.get();
    let link = svc::report_link(&show, &r.date);
    let html = svc::render_html(&r, &show.name, Some(&link));
    Ok((
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            // Shown in a sandboxed iframe: nothing in it may run.
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; style-src 'unsafe-inline'; img-src data:",
            ),
        ],
        html,
    ))
}

#[derive(Deserialize, Default)]
struct RunBody {
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    send: bool,
}

async fn run(
    State(state): State<AppState>,
    body: Option<Json<RunBody>>,
) -> ApiResult<Json<NightReport>> {
    let body = body.map(|b| b.0).unwrap_or_default();
    let show = state.store.get();
    let tz = crate::services::profiles::show_tz(&show);
    let now = chrono::Utc::now().with_timezone(&tz);
    let date = match body.date.as_deref().filter(|d| !d.is_empty()) {
        Some(d) => parse_date(d)?,
        None => svc::night_of(now),
    };
    if date > now.date_naive() {
        return Err(ApiError::bad_request("That night hasn't happened yet."));
    }
    if (now.date_naive() - date).num_days() > i64::from(crate::services::journal::KEEP_DAYS as u32)
    {
        return Err(ApiError::bad_request(
            "The journal doesn't go back that far (120 days).",
        ));
    }
    Ok(Json(svc::run(&state, date, body.send, true).await))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/reports", get(list))
        .route("/reports/run", post(run))
        .route("/reports/{date}", get(get_one))
        .route("/reports/{date}/email", get(email_preview))
}

#[cfg(test)]
mod tests {
    use crate::api::testkit::TestApp;
    use crate::services::journal::{self, Event, Record};
    use axum::http::StatusCode;
    use serde_json::json;

    #[tokio::test]
    async fn run_list_get_and_preview() {
        let app = TestApp::new();
        // Set the show's time zone so the night is computed in Chicago time.
        let (st, _) = app
            .json(
                "PUT",
                "/schedule",
                Some(
                    json!({"location": {"lat": 41.9, "lon": -87.6, "timezone": "America/Chicago"}}),
                ),
            )
            .await;
        assert_eq!(st, StatusCode::OK);
        let dir = journal::dir(&app.state.config.data_dir);
        // A night three days ago (the journal keeps 120 days).
        let tz: chrono_tz::Tz = "America/Chicago".parse().unwrap();
        let date = chrono::Utc::now().with_timezone(&tz).date_naive() - chrono::Duration::days(3);
        let d = date.format("%Y-%m-%d").to_string();
        let (noon, _) = crate::services::reports::night_window(&tz, date);
        let ts = |min: i64| (noon + chrono::Duration::minutes(min)).to_rfc3339();
        let rec = |ts: String, event| Record { ts, event };
        journal::append(
            &dir,
            &[
                rec(
                    ts(360),
                    Event::ShowStart {
                        entry_id: "e".into(),
                        name: "Nightly".into(),
                    },
                ),
                rec(
                    ts(361),
                    Event::ItemStart {
                        item: "sequence".into(),
                        id: "s".into(),
                        name: "Song".into(),
                        playlist_id: None,
                    },
                ),
                rec(
                    ts(362),
                    Event::Request {
                        sequence_id: "s".into(),
                        name: "Song".into(),
                    },
                ),
                rec(
                    ts(540),
                    Event::ShowEnd {
                        entry_id: "e".into(),
                        name: "Nightly".into(),
                    },
                ),
            ],
        )
        .unwrap();
        let (st, r) = app
            .json("POST", "/reports/run", Some(json!({"date": d})))
            .await;
        assert_eq!(st, StatusCode::OK, "{r}");
        assert_eq!(r["date"], d.as_str());
        assert_eq!(r["shows"][0]["runtimeMin"], 180);
        assert_eq!(r["itemsPlayed"], 1);
        assert_eq!(r["requests"], 1);
        assert!(r["headline"]
            .as_str()
            .unwrap()
            .starts_with("1 show, 1 song, 1 request"));

        let (st, l) = app.json("GET", "/reports?limit=30", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(l[0]["date"], d.as_str());
        assert_eq!(l[0]["itemsPlayed"], 1);

        let (st, one) = app.json("GET", &format!("/reports/{d}"), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(one["requests"], 1);
        let (st, _) = app.json("GET", "/reports/2020-08-01", None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let (st, _) = app.json("GET", "/reports/yesterday", None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);

        let req = axum::http::Request::builder()
            .uri(format!("/api/v1/reports/{d}/email"))
            .body(axum::body::Body::empty())
            .unwrap();
        let (st, headers, body) = app.send(req).await;
        assert_eq!(st, StatusCode::OK);
        assert!(headers[axum::http::header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html"));
        assert!(String::from_utf8(body).unwrap().contains("nightly report"));

        // Sending without email / push set up says so instead of failing.
        let (st, r) = app
            .json(
                "POST",
                "/reports/run",
                Some(json!({"date": d, "send": true})),
            )
            .await;
        assert_eq!(st, StatusCode::OK);
        assert!(r["delivery"][0].as_str().unwrap().contains("not set up"));

        let (st, _) = app
            .json("POST", "/reports/run", Some(json!({"date": "2999-01-01"})))
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }
}
