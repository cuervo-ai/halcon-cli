//! Trait interface for tool trust scoring.
//!
//! Implementations track per-tool reliability metrics and make inclusion/exclusion
//! decisions for the tool surface presented to the LLM.

use crate::types::{ToolDefinition, ToolFailureInfo, ToolTrustDecision, ToolTrustMetrics};

/// Dynamic tool trust scoring — runtime reliability tracking for tool selection.
///
/// Implementations accumulate success/failure/latency metrics across the session
/// and expose trust-based decisions for tool surface curation.
pub trait ToolTrust: Send + Sync {
    /// Record a successful tool execution.
    fn record_success(&mut self, tool_name: &str, latency_ms: u64);

    /// Record a failed tool execution.
    fn record_failure(&mut self, tool_name: &str, latency_ms: u64, error: Option<&str>);

    /// Compute trust score for a tool. Range: [0.0, 1.0].
    fn trust_score(&self, tool_name: &str) -> f64;

    /// Decide whether to include, deprioritize, or hide a tool.
    fn decide(&self, tool_name: &str) -> ToolTrustDecision;

    /// Filter a tool list based on trust scores.
    /// Returns (included_tools, hidden_count).
    fn filter_tools(&self, tools: Vec<ToolDefinition>) -> (Vec<ToolDefinition>, usize);

    /// Get metrics snapshot for a tool (for observability).
    fn get_metrics(&self, tool_name: &str) -> Option<ToolTrustMetrics>;

    /// Get failure records for retry mutation — tools with at least one failure.
    fn failure_records(&self) -> Vec<ToolFailureInfo>;

    /// Get all tools with their current trust scores.
    fn all_scores(&self) -> Vec<(String, f64)>;
}
