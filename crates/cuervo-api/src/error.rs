use serde::{Deserialize, Serialize};

/// API error response body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// Typed error codes for the control plane API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    // Client errors
    BadRequest,
    Unauthorized,
    NotFound,
    Conflict,
    ValidationFailed,

    // Server errors
    InternalError,
    RuntimeError,
    AgentError,
    TaskError,
    ToolError,
    Timeout,
    Unavailable,

    // Rate limiting
    RateLimited,
}

impl ApiError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::BadRequest,
            message: msg.into(),
            details: None,
        }
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::NotFound,
            message: msg.into(),
            details: None,
        }
    }

    pub fn unauthorized(msg: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::Unauthorized,
            message: msg.into(),
            details: None,
        }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::InternalError,
            message: msg.into(),
            details: None,
        }
    }

    pub fn runtime(msg: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::RuntimeError,
            message: msg.into(),
            details: None,
        }
    }

    pub fn timeout(msg: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::Timeout,
            message: msg.into(),
            details: None,
        }
    }

    /// HTTP status code for this error.
    pub fn status_code(&self) -> u16 {
        match self.code {
            ErrorCode::BadRequest | ErrorCode::ValidationFailed => 400,
            ErrorCode::Unauthorized => 401,
            ErrorCode::NotFound => 404,
            ErrorCode::Conflict => 409,
            ErrorCode::RateLimited => 429,
            ErrorCode::InternalError
            | ErrorCode::RuntimeError
            | ErrorCode::AgentError
            | ErrorCode::TaskError
            | ErrorCode::ToolError => 500,
            ErrorCode::Timeout => 504,
            ErrorCode::Unavailable => 503,
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{:?}] {}", self.code, self.message)
    }
}

impl std::error::Error for ApiError {}

#[cfg(feature = "server")]
mod server_impl {
    use super::*;
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};

    impl IntoResponse for ApiError {
        fn into_response(self) -> Response {
            let status = StatusCode::from_u16(self.status_code())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let body = serde_json::to_string(&self).unwrap_or_else(|_| {
                r#"{"code":"internal_error","message":"serialization failed"}"#.to_string()
            });
            (
                status,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                body,
            )
                .into_response()
        }
    }
}
