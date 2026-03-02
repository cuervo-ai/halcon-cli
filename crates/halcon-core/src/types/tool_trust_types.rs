//! Types for the ToolTrust trait interface.

use serde::{Deserialize, Serialize};

/// Trust decision for a single tool — returned by `ToolTrust::decide()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolTrustDecision {
    /// Tool is trusted — include normally.
    Include,
    /// Tool has low trust — include but deprioritize (move to end).
    Deprioritize,
    /// Tool has very low trust — hide from tool surface.
    Hide,
}

/// Snapshot of per-tool trust metrics (export-friendly, no `Instant`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolTrustMetrics {
    pub tool_name: String,
    pub success_rate: f64,
    pub avg_latency_ms: f64,
    pub call_count: u32,
    pub failure_count: u32,
}

/// Tool failure info for retry mutation — tools with at least one failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFailureInfo {
    pub tool_name: String,
    pub failure_count: u32,
}
