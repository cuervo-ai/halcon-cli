//! Trace recording types for deterministic replay.
//!
//! Each `TraceStep` captures one discrete step in the agent loop:
//! model requests, model responses, tool calls, tool results, and errors.
//! Steps are append-only and ordered by `step_index` within a session.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The type of step recorded in the trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceStepType {
    /// Model request sent to provider (captures model, message count, tool count).
    ModelRequest,
    /// Model response received (captures text, stop_reason, usage, latency).
    ModelResponse,
    /// Tool call dispatched (captures tool name, input arguments).
    ToolCall,
    /// Tool execution result (captures output, is_error, duration).
    ToolResult,
    /// Error during model invocation or tool execution.
    Error,
}

impl TraceStepType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ModelRequest => "model_request",
            Self::ModelResponse => "model_response",
            Self::ToolCall => "tool_call",
            Self::ToolResult => "tool_result",
            Self::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "model_request" => Some(Self::ModelRequest),
            "model_response" => Some(Self::ModelResponse),
            "tool_call" => Some(Self::ToolCall),
            "tool_result" => Some(Self::ToolResult),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

/// A single recorded step in the agent loop trace.
///
/// Steps are append-only and ordered by `(session_id, step_index)`.
/// `data_json` contains a serialized payload whose schema depends on `step_type`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceStep {
    pub session_id: Uuid,
    pub step_index: u32,
    pub step_type: TraceStepType,
    /// JSON-serialized step data. Schema varies by `step_type`:
    /// - `ModelRequest`: `{ model, message_count, tool_count, has_system }`
    /// - `ModelResponse`: `{ text, stop_reason, usage, latency_ms }`
    /// - `ToolCall`: `{ tool_use_id, tool_name, input }`
    /// - `ToolResult`: `{ tool_use_id, tool_name, content, is_error, duration_ms }`
    /// - `Error`: `{ message, context }`
    pub data_json: String,
    pub duration_ms: u64,
    pub timestamp: DateTime<Utc>,
}

/// Deterministic trace export format.
#[derive(Debug, Serialize, Deserialize)]
pub struct TraceExport {
    pub session_id: Uuid,
    pub exported_at: DateTime<Utc>,
    pub step_count: u32,
    pub steps: Vec<TraceStep>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_type_roundtrip() {
        let types = [
            TraceStepType::ModelRequest,
            TraceStepType::ModelResponse,
            TraceStepType::ToolCall,
            TraceStepType::ToolResult,
            TraceStepType::Error,
        ];
        for t in &types {
            let s = t.as_str();
            let parsed = TraceStepType::parse(s).expect("should parse");
            assert_eq!(*t, parsed);
        }
    }

    #[test]
    fn step_type_serde_roundtrip() {
        let step_type = TraceStepType::ToolCall;
        let json = serde_json::to_string(&step_type).unwrap();
        assert_eq!(json, "\"tool_call\"");
        let parsed: TraceStepType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, step_type);
    }

    #[test]
    fn trace_step_serde() {
        let step = TraceStep {
            session_id: Uuid::nil(),
            step_index: 0,
            step_type: TraceStepType::ModelRequest,
            data_json: r#"{"model":"echo","message_count":1}"#.to_string(),
            duration_ms: 42,
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&step).unwrap();
        let parsed: TraceStep = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.step_index, 0);
        assert_eq!(parsed.step_type, TraceStepType::ModelRequest);
        assert_eq!(parsed.duration_ms, 42);
    }

    #[test]
    fn trace_export_serde() {
        let export = TraceExport {
            session_id: Uuid::nil(),
            exported_at: Utc::now(),
            step_count: 0,
            steps: vec![],
        };
        let json = serde_json::to_string_pretty(&export).unwrap();
        let parsed: TraceExport = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.step_count, 0);
        assert!(parsed.steps.is_empty());
    }

    #[test]
    fn unknown_step_type_returns_none() {
        assert!(TraceStepType::parse("unknown").is_none());
        assert!(TraceStepType::parse("").is_none());
    }
}
