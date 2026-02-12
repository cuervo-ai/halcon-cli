//! MCP-specific error types.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum McpError {
    #[error("MCP transport error: {0}")]
    Transport(String),

    #[error("MCP protocol error: {0}")]
    Protocol(String),

    #[error("MCP server returned error {code}: {message}")]
    ServerError { code: i64, message: String },

    #[error("MCP server process failed to start: {0}")]
    ProcessStart(String),

    #[error("MCP server did not respond within timeout")]
    Timeout,

    #[error("MCP server is not initialized")]
    NotInitialized,

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type McpResult<T> = std::result::Result<T, McpError>;
