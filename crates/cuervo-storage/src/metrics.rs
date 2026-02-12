//! Invocation metrics: per-request cost and latency tracking.
//!
//! Each model invocation records latency, token counts, estimated cost,
//! and success/failure. Used by the CostLatencyOptimizer to rank models.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single model invocation metric record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvocationMetric {
    /// Provider name (e.g., "anthropic", "ollama").
    pub provider: String,
    /// Model ID (e.g., "claude-sonnet-4-5-20250929").
    pub model: String,
    /// Total latency in milliseconds (time-to-first-token or full response).
    pub latency_ms: u64,
    /// Input tokens consumed.
    pub input_tokens: u32,
    /// Output tokens generated.
    pub output_tokens: u32,
    /// Estimated cost in USD (0.0 if unknown).
    pub estimated_cost_usd: f64,
    /// Whether the invocation succeeded.
    pub success: bool,
    /// Stop reason (e.g., "end_turn", "tool_use", "error").
    pub stop_reason: String,
    /// Session ID (optional).
    pub session_id: Option<String>,
    /// When this invocation occurred.
    pub created_at: DateTime<Utc>,
}

/// Aggregated statistics for a specific model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelStats {
    /// Provider name.
    pub provider: String,
    /// Model ID.
    pub model: String,
    /// Total number of invocations.
    pub total_invocations: u64,
    /// Number of successful invocations.
    pub successful_invocations: u64,
    /// Average latency in milliseconds.
    pub avg_latency_ms: f64,
    /// P95 latency in milliseconds.
    pub p95_latency_ms: u64,
    /// Total tokens consumed (input + output).
    pub total_tokens: u64,
    /// Total estimated cost in USD.
    pub total_cost_usd: f64,
    /// Average cost per invocation.
    pub avg_cost_per_invocation: f64,
    /// Success rate (0.0 - 1.0).
    pub success_rate: f64,
}

/// Overall system metrics summary.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemMetrics {
    /// Total invocations across all models.
    pub total_invocations: u64,
    /// Total estimated cost in USD.
    pub total_cost_usd: f64,
    /// Total tokens consumed.
    pub total_tokens: u64,
    /// Per-model statistics.
    pub models: Vec<ModelStats>,
}

/// Provider-level metrics within a time window (for health scoring).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderWindowedMetrics {
    /// Provider name.
    pub provider: String,
    /// Total invocations in the window.
    pub total_invocations: u64,
    /// Successful invocations.
    pub successful_invocations: u64,
    /// Failed invocations.
    pub failed_invocations: u64,
    /// Number of invocations that timed out (stop_reason = "timeout").
    pub timeout_count: u64,
    /// Average latency in milliseconds.
    pub avg_latency_ms: f64,
    /// P95 latency in milliseconds.
    pub p95_latency_ms: u64,
    /// Error rate (0.0 - 1.0).
    pub error_rate: f64,
    /// Timeout rate (0.0 - 1.0).
    pub timeout_rate: f64,
}

/// A single tool execution metric record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecutionMetric {
    /// Tool name (e.g., "bash", "read_file").
    pub tool_name: String,
    /// Session ID (optional).
    pub session_id: Option<String>,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Whether the execution succeeded.
    pub success: bool,
    /// Whether this was part of a parallel batch.
    pub is_parallel: bool,
    /// Truncated input summary (optional).
    pub input_summary: Option<String>,
    /// When this execution occurred.
    pub created_at: DateTime<Utc>,
}

/// Aggregated statistics for a specific tool.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolStats {
    /// Tool name.
    pub tool_name: String,
    /// Total number of executions.
    pub total_executions: u64,
    /// Average duration in milliseconds.
    pub avg_duration_ms: f64,
    /// Success rate (0.0 - 1.0).
    pub success_rate: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocation_metric_serde_roundtrip() {
        let metric = InvocationMetric {
            provider: "anthropic".into(),
            model: "claude-sonnet-4-5-20250929".into(),
            latency_ms: 1500,
            input_tokens: 500,
            output_tokens: 200,
            estimated_cost_usd: 0.002,
            success: true,
            stop_reason: "end_turn".into(),
            session_id: None,
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&metric).unwrap();
        let back: InvocationMetric = serde_json::from_str(&json).unwrap();
        assert_eq!(back.provider, "anthropic");
        assert_eq!(back.latency_ms, 1500);
    }

    #[test]
    fn model_stats_defaults() {
        let stats = ModelStats {
            provider: "test".into(),
            model: "echo".into(),
            total_invocations: 0,
            successful_invocations: 0,
            avg_latency_ms: 0.0,
            p95_latency_ms: 0,
            total_tokens: 0,
            total_cost_usd: 0.0,
            avg_cost_per_invocation: 0.0,
            success_rate: 0.0,
        };
        assert_eq!(stats.total_invocations, 0);
    }

    #[test]
    fn system_metrics_default() {
        let metrics = SystemMetrics::default();
        assert_eq!(metrics.total_invocations, 0);
        assert!(metrics.models.is_empty());
    }
}
