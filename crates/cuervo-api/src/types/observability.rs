use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Log severity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// A structured log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: DateTime<Utc>,
    pub level: LogLevel,
    pub target: String,
    pub message: String,
    pub fields: HashMap<String, serde_json::Value>,
    pub span: Option<String>,
}

/// A single metric data point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricPoint {
    pub name: String,
    pub value: f64,
    pub unit: String,
    pub timestamp: DateTime<Utc>,
    pub labels: HashMap<String, String>,
}

/// Snapshot of current system metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub timestamp: DateTime<Utc>,
    pub agent_count: usize,
    pub tool_count: usize,
    pub total_invocations: u64,
    pub total_tool_executions: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_usd: f64,
    pub uptime_seconds: u64,
    pub active_tasks: usize,
    pub completed_tasks: usize,
    pub failed_tasks: usize,
    pub events_per_second: f64,
    pub agent_metrics: Vec<AgentMetricSummary>,
}

/// Per-agent metric summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetricSummary {
    pub agent_id: Uuid,
    pub agent_name: String,
    pub invocation_count: u64,
    pub avg_latency_ms: f64,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub error_rate: f64,
}

/// A trace span for distributed tracing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceSpan {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub operation: String,
    pub start: DateTime<Utc>,
    pub duration_ms: u64,
    pub status: SpanStatus,
    pub attributes: HashMap<String, serde_json::Value>,
}

/// Status of a trace span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanStatus {
    Ok,
    Error,
    Unset,
}

/// Query parameters for log streaming.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LogStreamQuery {
    pub min_level: Option<LogLevel>,
    pub target_filter: Option<String>,
    pub search: Option<String>,
}

/// Query parameters for metrics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetricsQuery {
    pub since_seconds: Option<u64>,
}
