use thiserror::Error;

/// Core error types for the Cuervo CLI platform.
///
/// Library crates use these typed errors via `thiserror`.
/// The binary crate wraps them with `anyhow` for context.
#[derive(Debug, Error)]
pub enum CuervoError {
    // --- Provider errors ---
    #[error("provider '{provider}' is not available")]
    ProviderUnavailable { provider: String },

    #[error("model '{model}' not found in provider '{provider}'")]
    ModelNotFound { provider: String, model: String },

    #[error("API request failed: {message}")]
    ApiError {
        message: String,
        status: Option<u16>,
    },

    #[error("request to '{provider}' timed out after {timeout_secs}s")]
    RequestTimeout { provider: String, timeout_secs: u64 },

    #[error("connection to '{provider}' failed: {message}")]
    ConnectionError { provider: String, message: String },

    #[error("streaming interrupted: {0}")]
    StreamError(String),

    #[error("rate limited by provider '{provider}', retry after {retry_after_secs}s")]
    RateLimited {
        provider: String,
        retry_after_secs: u64,
    },

    // --- Tool errors ---
    #[error("tool '{tool}' execution failed: {message}")]
    ToolExecutionFailed { tool: String, message: String },

    #[error("permission denied: tool '{tool}' requires {required:?} permission")]
    PermissionDenied {
        tool: String,
        required: crate::types::PermissionLevel,
    },

    #[error("tool '{tool}' timed out after {timeout_secs}s")]
    ToolTimeout { tool: String, timeout_secs: u64 },

    #[error("user rejected operation: {0}")]
    UserRejected(String),

    // --- Storage errors ---
    #[error("database error: {0}")]
    DatabaseError(String),

    #[error("migration failed: {0}")]
    MigrationError(String),

    #[error("session '{0}' not found")]
    SessionNotFound(String),

    // --- Config errors ---
    #[error("configuration error: {0}")]
    ConfigError(String),

    #[error("invalid configuration value for '{key}': {message}")]
    ConfigValueInvalid { key: String, message: String },

    // --- Auth errors ---
    #[error("authentication required")]
    AuthRequired,

    #[error("authentication failed: {0}")]
    AuthFailed(String),

    #[error("token expired")]
    TokenExpired,

    // --- Security errors ---
    #[error("PII detected in {context}: {pii_type}")]
    PiiDetected { context: String, pii_type: String },

    #[error("content blocked by security policy: {0}")]
    SecurityBlocked(String),

    // --- Planning errors ---
    #[error("Planning failed: {0}")]
    PlanningFailed(String),

    // --- General ---
    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("{0}")]
    Internal(String),
}

impl CuervoError {
    /// Returns true if this error is transient and the operation should be retried.
    ///
    /// Non-retryable errors (auth failures, billing issues, client errors) fail fast.
    /// Retryable errors (timeouts, server errors, rate limits) may succeed on retry.
    pub fn is_retryable(&self) -> bool {
        match self {
            // Transient: network issues, server errors, overload — retry makes sense.
            CuervoError::RequestTimeout { .. } => true,
            CuervoError::ConnectionError { .. } => true,
            CuervoError::StreamError(_) => true,
            CuervoError::RateLimited { .. } => true,

            // API errors: only retry on 5xx server errors.
            CuervoError::ApiError { status, .. } => {
                matches!(status, Some(500 | 502 | 503 | 529))
            }

            // Non-retryable: auth, billing, client errors, config, permissions.
            CuervoError::AuthFailed(_)
            | CuervoError::AuthRequired
            | CuervoError::TokenExpired
            | CuervoError::ProviderUnavailable { .. }
            | CuervoError::ModelNotFound { .. }
            | CuervoError::ConfigError(_)
            | CuervoError::ConfigValueInvalid { .. }
            | CuervoError::PermissionDenied { .. }
            | CuervoError::UserRejected(_)
            | CuervoError::SecurityBlocked(_)
            | CuervoError::PiiDetected { .. }
            | CuervoError::DatabaseError(_)
            | CuervoError::MigrationError(_)
            | CuervoError::SessionNotFound(_)
            | CuervoError::PlanningFailed(_)
            | CuervoError::InvalidInput(_)
            | CuervoError::Internal(_)
            | CuervoError::ToolExecutionFailed { .. }
            | CuervoError::ToolTimeout { .. } => false,
        }
    }
}

/// Convenience Result alias using CuervoError.
pub type Result<T> = std::result::Result<T, CuervoError>;
