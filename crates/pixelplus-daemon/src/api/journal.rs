//! `GET /journal?date=YYYY-MM-DD&types=a,b` (admin, debugging): one local
//! day of the show journal (`services/journal.rs`, F11). Owned by WS0.

use super::{ApiError, ApiResult};
use crate::services::journal::{self, Record};
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::NaiveDate;
use serde::Deserialize;

#[derive(Deserialize)]
struct JournalQuery {
    /// Local date; default today.
    #[serde(default)]
    date: Option<String>,
    /// Comma-separated event names (`itemStart,error`); default all.
    #[serde(default)]
    types: Option<String>,
}

async fn get_journal(
    State(state): State<AppState>,
    Query(q): Query<JournalQuery>,
) -> ApiResult<Json<Vec<Record>>> {
    let date = match q.date.as_deref().filter(|d| !d.is_empty()) {
        Some(d) => NaiveDate::parse_from_str(d, "%Y-%m-%d")
            .map_err(|_| ApiError::bad_request("The date must look like 2026-12-24."))?,
        None => state.services.journal.now().date_naive(),
    };
    let types: Option<Vec<String>> = q.types.as_deref().filter(|t| !t.is_empty()).map(|t| {
        t.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    });
    state.services.journal.flush().await;
    let dir = journal::dir(&state.config.data_dir);
    let recs = tokio::task::spawn_blocking(move || journal::read_day(&dir, date, types.as_deref()))
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(recs))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/journal", get(get_journal))
}

#[cfg(test)]
mod tests {
    use crate::api::testkit::TestApp;
    use crate::services::journal::{self, Event, Record};
    use axum::http::StatusCode;

    #[tokio::test]
    async fn journal_endpoint_reads_a_day_and_filters() {
        let app = TestApp::new();
        let dir = journal::dir(&app.state.config.data_dir);
        let rec = |ts: &str, event| Record {
            ts: ts.into(),
            event,
        };
        journal::append(
            &dir,
            &[
                rec(
                    "2026-12-01T18:00:00-06:00",
                    Event::Trigger {
                        id: "t1".into(),
                        via: None,
                        from: None,
                    },
                ),
                rec(
                    "2026-12-01T18:05:00-06:00",
                    Event::Error {
                        code: "x".into(),
                        msg: "boom".into(),
                    },
                ),
            ],
        )
        .unwrap();
        let (st, v) = app.json("GET", "/journal?date=2026-12-01", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v.as_array().unwrap().len(), 2);
        assert_eq!(v[0]["ev"], "trigger");
        let (_, v) = app
            .json("GET", "/journal?date=2026-12-01&types=error", None)
            .await;
        assert_eq!(v.as_array().unwrap().len(), 1);
        assert_eq!(v[0]["msg"], "boom");
        let (st, _) = app.json("GET", "/journal?date=12/01", None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }
}
