//! Uniform API error type: `{"error": {"code": "...", "message": "..."}}`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        ApiError {
            status,
            code,
            message: message.into(),
        }
    }
    pub fn not_found(what: impl std::fmt::Display) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("{what} was not found"),
        )
    }
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", message)
    }
    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", "Please sign in")
    }
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", message)
    }
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "unavailable", message)
    }
    pub fn internal(err: impl std::fmt::Display) -> Self {
        let text = err.to_string();
        // ENOSPC from any write (upload, show.json, snapshot): say what to do.
        if text.contains("(os error 28)") {
            tracing::error!("storage full: {text}");
            return Self::storage_full();
        }
        tracing::error!("internal error: {err}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            format!("Something went wrong: {err}"),
        )
    }
}

impl ApiError {
    /// The SD card / data disk is full (HTTP 507).
    pub fn storage_full() -> Self {
        Self::new(
            StatusCode::INSUFFICIENT_STORAGE,
            "storage_full",
            "The controller's storage is full. Delete sequences, audio or backups you no \
             longer need, then try again.",
        )
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ApiError {}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError::internal(format!("{e:#}"))
    }
}

impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> Self {
        ApiError::internal(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({
                "error": { "code": self.code, "message": self.message }
            })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_disk_gets_a_helpful_answer() {
        let io = std::io::Error::from_raw_os_error(28);
        let e = ApiError::from(io);
        assert_eq!(e.status, StatusCode::INSUFFICIENT_STORAGE);
        assert_eq!(e.code, "storage_full");
        let wrapped = anyhow::Error::from(std::io::Error::from_raw_os_error(28))
            .context("couldn't save the snapshot");
        assert_eq!(ApiError::from(wrapped).code, "storage_full");
        assert_eq!(ApiError::internal("boom").code, "internal");
    }
}
