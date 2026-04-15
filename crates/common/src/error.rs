//! Unified application error type. Implements [`IntoResponse`] so handlers can
//! return `Result<_, AppError>` directly.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// Top-level error returned from route handlers. Each variant maps to an HTTP
/// status and a JSON `{ "error": { "code", "message" } }` body.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// 404 Not Found. The string is the machine-readable error code.
    #[error("Not found: {0}")]
    NotFound(String),

    /// 400 Bad Request. The string is a human-readable message.
    #[error("Bad request: {0}")]
    BadRequest(String),

    /// 401 Unauthorized. The string is the machine-readable error code.
    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    /// 403 Forbidden. The string is a human-readable message.
    #[error("Forbidden: {0}")]
    Forbidden(String),

    /// 409 Conflict. The string is the machine-readable error code.
    #[error("Conflict: {0}")]
    Conflict(String),

    /// 422 Unprocessable Entity. Request passed parsing but failed validation.
    #[error("Validation error: {0}")]
    Validation(String),

    /// 500 Internal Server Error. Generic fallback for unexpected failures.
    #[error("Internal error: {0}")]
    Internal(String),

    /// 500 Internal Server Error caused by a database failure.
    #[error("Database error: {0}")]
    Database(#[from] sea_orm::DbErr),

    /// 500 Internal Server Error caused by a repository-layer failure.
    #[error("Repository error: {0}")]
    Repo(#[from] repo::RepoError),
}

#[derive(Serialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Serialize)]
struct ErrorDetail {
    code: String,
    message: String,
}

impl AppError {
    fn status_code(&self) -> StatusCode {
        match self {
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            AppError::Forbidden(_) => StatusCode::FORBIDDEN,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::Repo(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn error_code(&self) -> &str {
        match self {
            AppError::NotFound(code) => code,
            AppError::BadRequest(_) => "BAD_REQUEST",
            AppError::Unauthorized(code) => code,
            AppError::Forbidden(_) => "FORBIDDEN",
            AppError::Conflict(code) => code,
            AppError::Validation(_) => "VALIDATION_ERROR",
            AppError::Internal(_) => "INTERNAL_ERROR",
            AppError::Database(_) => "INTERNAL_ERROR",
            AppError::Repo(_) => "INTERNAL_ERROR",
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let body = ErrorBody {
            error: ErrorDetail {
                code: self.error_code().to_string(),
                message: self.to_string(),
            },
        };
        (status, axum::Json(body)).into_response()
    }
}
