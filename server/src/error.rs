//! API error type and the wire error envelope.
//!
//! Every error response has the shape `{"error":{"code":"...","message":"..."}}`
//! (CONTRACT §4.4). The conflict variant additionally carries a top-level
//! `server_revision` field for the sync flow.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// The single error type used by every handler.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    AccessDenied(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{message}")]
    Conflict {
        message: String,
        /// Current server-side collection revision, surfaced on stale PUTs.
        server_revision: Option<i64>,
    },
    #[error("{0}")]
    Internal(String),
}

impl ApiError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        ApiError::BadRequest(msg.into())
    }

    pub fn unauthorized(msg: impl Into<String>) -> Self {
        ApiError::Unauthorized(msg.into())
    }

    pub fn access_denied(msg: impl Into<String>) -> Self {
        ApiError::AccessDenied(msg.into())
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        ApiError::NotFound(msg.into())
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        ApiError::Internal(msg.into())
    }

    /// Stable machine-readable error code (CONTRACT §4.4).
    pub fn code(&self) -> &'static str {
        match self {
            ApiError::BadRequest(_) => "bad_request",
            ApiError::Unauthorized(_) => "unauthorized",
            ApiError::AccessDenied(_) => "access_denied",
            ApiError::NotFound(_) => "not_found",
            ApiError::Conflict { .. } => "conflict",
            ApiError::Internal(_) => "internal",
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            ApiError::AccessDenied(_) => StatusCode::FORBIDDEN,
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::Conflict { .. } => StatusCode::CONFLICT,
            ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            ApiError::BadRequest(m)
            | ApiError::Unauthorized(m)
            | ApiError::AccessDenied(m)
            | ApiError::NotFound(m)
            | ApiError::Internal(m) => m,
            ApiError::Conflict { message, .. } => message,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = match &self {
            ApiError::Conflict {
                server_revision, ..
            } => json!({
                "error": {"code": self.code(), "message": self.message()},
                "server_revision": server_revision,
            }),
            other => json!({
                "error": {"code": other.code(), "message": other.message()},
            }),
        };
        (status, Json(body)).into_response()
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(err: rusqlite::Error) -> Self {
        log::error!("database error: {err}");
        ApiError::internal("internal database error")
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
