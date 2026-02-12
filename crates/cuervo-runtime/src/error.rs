use thiserror::Error;

/// Runtime-specific error types.
#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("agent '{id}' not found in registry")]
    AgentNotFound { id: String },

    #[error("agent '{name}' health check failed: {reason}")]
    HealthCheckFailed { name: String, reason: String },

    #[error("transport error: {0}")]
    Transport(String),

    #[error("federation error: {0}")]
    Federation(String),

    #[error("execution error: {0}")]
    Execution(String),

    #[error("budget exceeded: {0}")]
    BudgetExceeded(String),

    #[error("cycle detected in task DAG")]
    CycleDetected,

    #[error("missing dependency: task '{task_id}' depends on unknown task '{dep_id}'")]
    MissingDependency { task_id: String, dep_id: String },

    #[error("plugin error: {0}")]
    Plugin(String),

    #[error("agent invocation timed out after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },

    #[error("agent shutdown failed: {0}")]
    ShutdownFailed(String),

    #[error("{0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, RuntimeError>;
