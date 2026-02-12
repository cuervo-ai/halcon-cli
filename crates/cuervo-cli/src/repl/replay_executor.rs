//! Replay tool executor: intercepts tool execution during replay by returning
//! recorded results instead of actually running tools.

use std::collections::HashMap;

use cuervo_storage::{TraceStep, TraceStepType};

/// A recorded tool result from a trace step.
#[derive(Debug, Clone)]
pub struct RecordedToolResult {
    pub content: String,
    pub is_error: bool,
    #[allow(dead_code)]
    pub duration_ms: u64,
}

/// Intercepts tool execution during replay by returning recorded results
/// instead of running tools.
///
/// Built from trace steps of type `ToolResult`, keyed by `tool_use_id`.
pub struct ReplayToolExecutor {
    results: HashMap<String, RecordedToolResult>,
}

impl ReplayToolExecutor {
    /// Construct from trace steps.
    ///
    /// Filters for `ToolResult` steps, parses each `data_json` to extract
    /// `tool_use_id`, `content`, `is_error`, and `duration_ms`.
    pub fn from_trace(steps: &[TraceStep]) -> Self {
        let mut results = HashMap::new();

        for step in steps {
            if step.step_type != TraceStepType::ToolResult {
                continue;
            }

            let data: serde_json::Value = match serde_json::from_str(&step.data_json) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let tool_use_id = match data.get("tool_use_id").and_then(|v| v.as_str()) {
                Some(id) => id.to_string(),
                None => continue,
            };

            let content = data.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let is_error = data.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
            let duration_ms = data.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0);

            results.insert(tool_use_id, RecordedToolResult {
                content,
                is_error,
                duration_ms,
            });
        }

        Self { results }
    }

    /// Look up a recorded result by tool_use_id.
    pub fn get_result(&self, tool_use_id: &str) -> Option<&RecordedToolResult> {
        self.results.get(tool_use_id)
    }

    /// Number of recorded results.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.results.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn make_trace_step(step_type: TraceStepType, data_json: &str, step_index: u32) -> TraceStep {
        TraceStep {
            session_id: Uuid::new_v4(),
            step_index,
            step_type,
            data_json: data_json.to_string(),
            duration_ms: 100,
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn from_trace_empty() {
        let executor = ReplayToolExecutor::from_trace(&[]);
        assert_eq!(executor.len(), 0);
    }

    #[test]
    fn from_trace_parses_results() {
        let steps = vec![make_trace_step(
            TraceStepType::ToolResult,
            r#"{"tool_use_id":"tu_1","tool_name":"bash","content":"file1.txt\nfile2.txt","is_error":false,"duration_ms":42,"parallel":false}"#,
            0,
        )];
        let executor = ReplayToolExecutor::from_trace(&steps);
        assert_eq!(executor.len(), 1);
        let result = executor.get_result("tu_1").unwrap();
        assert_eq!(result.content, "file1.txt\nfile2.txt");
        assert!(!result.is_error);
        assert_eq!(result.duration_ms, 42);
    }

    #[test]
    fn get_result_found() {
        let steps = vec![make_trace_step(
            TraceStepType::ToolResult,
            r#"{"tool_use_id":"tu_abc","tool_name":"read_file","content":"contents","is_error":false,"duration_ms":10,"parallel":false}"#,
            0,
        )];
        let executor = ReplayToolExecutor::from_trace(&steps);
        assert!(executor.get_result("tu_abc").is_some());
    }

    #[test]
    fn get_result_not_found() {
        let executor = ReplayToolExecutor::from_trace(&[]);
        assert!(executor.get_result("nonexistent").is_none());
    }

    #[test]
    fn parallel_batch_results() {
        let steps = vec![
            make_trace_step(
                TraceStepType::ToolResult,
                r#"{"tool_use_id":"tu_1","tool_name":"read_file","content":"a","is_error":false,"duration_ms":10,"parallel":true}"#,
                0,
            ),
            make_trace_step(
                TraceStepType::ToolResult,
                r#"{"tool_use_id":"tu_2","tool_name":"read_file","content":"b","is_error":false,"duration_ms":20,"parallel":true}"#,
                1,
            ),
            make_trace_step(
                TraceStepType::ToolResult,
                r#"{"tool_use_id":"tu_3","tool_name":"bash","content":"c","is_error":false,"duration_ms":30,"parallel":true}"#,
                2,
            ),
        ];
        let executor = ReplayToolExecutor::from_trace(&steps);
        assert_eq!(executor.len(), 3);
        assert_eq!(executor.get_result("tu_1").unwrap().content, "a");
        assert_eq!(executor.get_result("tu_2").unwrap().content, "b");
        assert_eq!(executor.get_result("tu_3").unwrap().content, "c");
    }

    #[test]
    fn error_result_preserved() {
        let steps = vec![make_trace_step(
            TraceStepType::ToolResult,
            r#"{"tool_use_id":"tu_err","tool_name":"bash","content":"command not found","is_error":true,"duration_ms":5,"parallel":false}"#,
            0,
        )];
        let executor = ReplayToolExecutor::from_trace(&steps);
        let result = executor.get_result("tu_err").unwrap();
        assert!(result.is_error);
        assert_eq!(result.content, "command not found");
    }

    #[test]
    fn duration_preserved() {
        let steps = vec![make_trace_step(
            TraceStepType::ToolResult,
            r#"{"tool_use_id":"tu_dur","tool_name":"bash","content":"ok","is_error":false,"duration_ms":12345,"parallel":false}"#,
            0,
        )];
        let executor = ReplayToolExecutor::from_trace(&steps);
        assert_eq!(executor.get_result("tu_dur").unwrap().duration_ms, 12345);
    }

    #[test]
    fn from_trace_ignores_non_tool_result() {
        let steps = vec![
            make_trace_step(TraceStepType::ModelRequest, r#"{"round":0}"#, 0),
            make_trace_step(TraceStepType::ModelResponse, r#"{"round":0,"text":"hi","stop_reason":"end_turn"}"#, 1),
            make_trace_step(TraceStepType::ToolCall, r#"{"tool_name":"bash"}"#, 2),
            make_trace_step(
                TraceStepType::ToolResult,
                r#"{"tool_use_id":"tu_only","tool_name":"bash","content":"ok","is_error":false,"duration_ms":1,"parallel":false}"#,
                3,
            ),
            make_trace_step(TraceStepType::Error, r#"{"context":"test","message":"err"}"#, 4),
        ];
        let executor = ReplayToolExecutor::from_trace(&steps);
        assert_eq!(executor.len(), 1);
        assert!(executor.get_result("tu_only").is_some());
    }
}
