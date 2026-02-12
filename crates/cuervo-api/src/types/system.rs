use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Overall system status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemStatus {
    pub version: String,
    pub started_at: DateTime<Utc>,
    pub uptime_seconds: u64,
    pub agent_count: usize,
    pub tool_count: usize,
    pub active_tasks: usize,
    pub health: SystemHealth,
    pub platform: PlatformInfo,
}

/// System health assessment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemHealth {
    Healthy,
    Degraded,
    Unhealthy,
}

/// Platform information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformInfo {
    pub os: String,
    pub arch: String,
    pub rust_version: String,
    pub pid: u32,
    pub memory_usage_bytes: u64,
}

/// Shutdown request with optional reason.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShutdownRequest {
    #[serde(default)]
    pub graceful: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Shutdown response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShutdownResponse {
    pub accepted: bool,
    pub message: String,
}
