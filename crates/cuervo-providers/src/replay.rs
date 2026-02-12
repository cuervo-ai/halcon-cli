//! Replay provider: replays recorded model responses from trace steps
//! instead of calling a real API. Used for deterministic replay verification.

use std::sync::Mutex;
use std::collections::VecDeque;

use async_trait::async_trait;
use futures::stream::{self, BoxStream};

use cuervo_core::error::{CuervoError, Result};
use cuervo_core::traits::ModelProvider;
use cuervo_core::types::{ModelChunk, ModelInfo, ModelRequest, StopReason, TokenCost, TokenUsage};

/// A recorded tool use from a trace step.
#[derive(Debug, Clone)]
struct RecordedToolUse {
    id: String,
    name: String,
    input: serde_json::Value,
}

/// A recorded model response parsed from a trace step's data_json.
#[derive(Debug, Clone)]
struct RecordedResponse {
    text: String,
    tool_uses: Vec<RecordedToolUse>,
    stop_reason: StopReason,
    usage: TokenUsage,
}

/// A `ModelProvider` that replays recorded model responses from trace steps
/// instead of calling a real API.
///
/// Responses are consumed sequentially from a `VecDeque`. Each `invoke()`
/// pops the next recorded response and emits it as a stream of `ModelChunk`s.
pub struct ReplayProvider {
    responses: Mutex<VecDeque<RecordedResponse>>,
    models: Vec<ModelInfo>,
}

impl ReplayProvider {
    /// Construct a ReplayProvider from trace steps.
    ///
    /// Filters for `ModelResponse` steps, parses each `data_json` into a
    /// `RecordedResponse`, and stores them in a `VecDeque` for sequential consumption.
    pub fn from_trace(steps: &[cuervo_storage::TraceStep], model: &str) -> Result<Self> {
        let mut responses = VecDeque::new();

        for step in steps {
            if step.step_type != cuervo_storage::TraceStepType::ModelResponse {
                continue;
            }

            let data: serde_json::Value = serde_json::from_str(&step.data_json)
                .map_err(|e| CuervoError::Internal(format!("parse trace data_json: {e}")))?;

            // Skip cache-hit entries (they don't represent real model responses).
            if data.get("cache_hit").and_then(|v| v.as_bool()).unwrap_or(false) {
                continue;
            }

            let text = data.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();

            let stop_reason = match data.get("stop_reason").and_then(|v| v.as_str()).unwrap_or("end_turn") {
                "end_turn" => StopReason::EndTurn,
                "max_tokens" => StopReason::MaxTokens,
                "tool_use" => StopReason::ToolUse,
                "stop_sequence" => StopReason::StopSequence,
                _ => StopReason::EndTurn,
            };

            let usage = if let Some(u) = data.get("usage") {
                TokenUsage {
                    input_tokens: u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                    output_tokens: u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                    ..Default::default()
                }
            } else {
                TokenUsage::default()
            };

            let tool_uses = data.get("tool_uses")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter().filter_map(|t| {
                        Some(RecordedToolUse {
                            id: t.get("id")?.as_str()?.to_string(),
                            name: t.get("name")?.as_str()?.to_string(),
                            input: t.get("input").cloned().unwrap_or(serde_json::Value::Null),
                        })
                    }).collect()
                })
                .unwrap_or_default();

            responses.push_back(RecordedResponse {
                text,
                tool_uses,
                stop_reason,
                usage,
            });
        }

        let models = vec![ModelInfo {
            id: model.into(),
            name: format!("Replay ({model})"),
            provider: "replay".into(),
            context_window: 0,
            max_output_tokens: 0,
            supports_streaming: true,
            supports_tools: true,
            supports_vision: false,
            supports_reasoning: false,
            cost_per_input_token: 0.0,
            cost_per_output_token: 0.0,
        }];
        Ok(Self {
            responses: Mutex::new(responses),
            models,
        })
    }

    /// Number of remaining responses.
    pub fn remaining(&self) -> usize {
        self.responses.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

#[async_trait]
impl ModelProvider for ReplayProvider {
    fn name(&self) -> &str {
        "replay"
    }

    fn supported_models(&self) -> &[ModelInfo] {
        &self.models
    }

    async fn invoke(
        &self,
        _request: &ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelChunk>>> {
        let response = {
            let mut queue = self.responses.lock().unwrap_or_else(|e| e.into_inner());
            queue.pop_front()
        };

        let response = response.ok_or_else(|| {
            CuervoError::Internal("replay trace exhausted: no more recorded responses".into())
        })?;

        let mut chunks: Vec<Result<ModelChunk>> = Vec::new();

        // Emit text delta.
        if !response.text.is_empty() {
            chunks.push(Ok(ModelChunk::TextDelta(response.text)));
        }

        // Emit tool uses.
        for tool in response.tool_uses {
            chunks.push(Ok(ModelChunk::ToolUse {
                id: tool.id,
                name: tool.name,
                input: tool.input,
            }));
        }

        // Emit usage.
        chunks.push(Ok(ModelChunk::Usage(response.usage)));

        // Emit done.
        chunks.push(Ok(ModelChunk::Done(response.stop_reason)));

        Ok(Box::pin(stream::iter(chunks)))
    }

    async fn is_available(&self) -> bool {
        true
    }

    fn estimate_cost(&self, _request: &ModelRequest) -> TokenCost {
        TokenCost::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cuervo_core::types::{ChatMessage, MessageContent, Role};
    use cuervo_storage::{TraceStep, TraceStepType};
    use chrono::Utc;
    use futures::StreamExt;
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

    fn make_request() -> ModelRequest {
        ModelRequest {
            model: "test".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text("hello".into()),
            }],
            tools: vec![],
            max_tokens: None,
            temperature: None,
            system: None,
            stream: true,
        }
    }

    #[test]
    fn from_trace_empty() {
        let provider = ReplayProvider::from_trace(&[], "test").unwrap();
        assert_eq!(provider.remaining(), 0);
    }

    #[test]
    fn from_trace_parses_text_only() {
        let steps = vec![make_trace_step(
            TraceStepType::ModelResponse,
            r#"{"round":0,"text":"Hello world","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5},"latency_ms":100,"tool_uses":[]}"#,
            0,
        )];
        let provider = ReplayProvider::from_trace(&steps, "test").unwrap();
        assert_eq!(provider.remaining(), 1);
    }

    #[test]
    fn from_trace_parses_tool_uses() {
        let steps = vec![make_trace_step(
            TraceStepType::ModelResponse,
            r#"{"round":0,"text":"","stop_reason":"tool_use","usage":{"input_tokens":10,"output_tokens":5},"latency_ms":100,"tool_uses":[{"id":"tu_1","name":"read_file","input":{"path":"/tmp/f"}}]}"#,
            0,
        )];
        let provider = ReplayProvider::from_trace(&steps, "test").unwrap();
        assert_eq!(provider.remaining(), 1);
    }

    #[tokio::test]
    async fn invoke_replays_text() {
        let steps = vec![make_trace_step(
            TraceStepType::ModelResponse,
            r#"{"round":0,"text":"Hello replay","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5},"latency_ms":100,"tool_uses":[]}"#,
            0,
        )];
        let provider = ReplayProvider::from_trace(&steps, "test").unwrap();
        let req = make_request();
        let mut stream = provider.invoke(&req).await.unwrap();

        let mut text = String::new();
        let mut got_usage = false;
        let mut got_done = false;
        while let Some(chunk) = stream.next().await {
            match chunk.unwrap() {
                ModelChunk::TextDelta(t) => text.push_str(&t),
                ModelChunk::Usage(u) => {
                    got_usage = true;
                    assert_eq!(u.input_tokens, 10);
                    assert_eq!(u.output_tokens, 5);
                }
                ModelChunk::Done(s) => {
                    got_done = true;
                    assert_eq!(s, StopReason::EndTurn);
                }
                _ => {}
            }
        }
        assert_eq!(text, "Hello replay");
        assert!(got_usage);
        assert!(got_done);
    }

    #[tokio::test]
    async fn invoke_replays_tool_uses() {
        let steps = vec![make_trace_step(
            TraceStepType::ModelResponse,
            r#"{"round":0,"text":"","stop_reason":"tool_use","usage":{"input_tokens":10,"output_tokens":5},"latency_ms":100,"tool_uses":[{"id":"tu_1","name":"bash","input":{"command":"ls"}}]}"#,
            0,
        )];
        let provider = ReplayProvider::from_trace(&steps, "test").unwrap();
        let req = make_request();
        let mut stream = provider.invoke(&req).await.unwrap();

        let mut got_tool = false;
        let mut got_done = false;
        while let Some(chunk) = stream.next().await {
            match chunk.unwrap() {
                ModelChunk::ToolUse { id, name, .. } => {
                    got_tool = true;
                    assert_eq!(id, "tu_1");
                    assert_eq!(name, "bash");
                }
                ModelChunk::Done(s) => {
                    got_done = true;
                    assert_eq!(s, StopReason::ToolUse);
                }
                _ => {}
            }
        }
        assert!(got_tool);
        assert!(got_done);
    }

    #[tokio::test]
    async fn invoke_sequential_consumption() {
        let steps = vec![
            make_trace_step(
                TraceStepType::ModelResponse,
                r#"{"round":0,"text":"First","stop_reason":"tool_use","usage":{"input_tokens":1,"output_tokens":1},"latency_ms":10,"tool_uses":[]}"#,
                0,
            ),
            make_trace_step(
                TraceStepType::ModelResponse,
                r#"{"round":1,"text":"Second","stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":2},"latency_ms":20,"tool_uses":[]}"#,
                1,
            ),
        ];
        let provider = ReplayProvider::from_trace(&steps, "test").unwrap();
        let req = make_request();

        // First invoke.
        let mut stream = provider.invoke(&req).await.unwrap();
        let mut text = String::new();
        while let Some(chunk) = stream.next().await {
            if let Ok(ModelChunk::TextDelta(t)) = chunk {
                text.push_str(&t);
            }
        }
        assert_eq!(text, "First");

        // Second invoke.
        let mut stream = provider.invoke(&req).await.unwrap();
        let mut text = String::new();
        while let Some(chunk) = stream.next().await {
            if let Ok(ModelChunk::TextDelta(t)) = chunk {
                text.push_str(&t);
            }
        }
        assert_eq!(text, "Second");

        assert_eq!(provider.remaining(), 0);
    }

    #[tokio::test]
    async fn invoke_exhausted_returns_error() {
        let provider = ReplayProvider::from_trace(&[], "test").unwrap();
        let req = make_request();
        let result = provider.invoke(&req).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn multiple_rounds() {
        let steps = vec![
            make_trace_step(TraceStepType::ModelResponse, r#"{"round":0,"text":"R0","stop_reason":"tool_use","usage":{"input_tokens":1,"output_tokens":1},"latency_ms":10,"tool_uses":[]}"#, 0),
            make_trace_step(TraceStepType::ModelResponse, r#"{"round":1,"text":"R1","stop_reason":"tool_use","usage":{"input_tokens":2,"output_tokens":2},"latency_ms":20,"tool_uses":[]}"#, 1),
            make_trace_step(TraceStepType::ModelResponse, r#"{"round":2,"text":"R2","stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":3},"latency_ms":30,"tool_uses":[]}"#, 2),
        ];
        let provider = ReplayProvider::from_trace(&steps, "test").unwrap();
        let req = make_request();

        for i in 0..3 {
            let mut stream = provider.invoke(&req).await.unwrap();
            let mut text = String::new();
            while let Some(chunk) = stream.next().await {
                if let Ok(ModelChunk::TextDelta(t)) = chunk {
                    text.push_str(&t);
                }
            }
            assert_eq!(text, format!("R{i}"));
        }
        assert_eq!(provider.remaining(), 0);
    }

    #[test]
    fn name_and_model() {
        let provider = ReplayProvider::from_trace(&[], "claude-3").unwrap();
        assert_eq!(provider.name(), "replay");
        let models = provider.supported_models();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "claude-3");
    }

    #[test]
    fn estimate_cost_zero() {
        let provider = ReplayProvider::from_trace(&[], "test").unwrap();
        let req = make_request();
        let cost = provider.estimate_cost(&req);
        assert_eq!(cost.estimated_cost_usd, 0.0);
    }

    #[test]
    fn from_trace_ignores_non_model_response() {
        let steps = vec![
            make_trace_step(TraceStepType::ModelRequest, r#"{"round":0}"#, 0),
            make_trace_step(TraceStepType::ToolCall, r#"{"tool_name":"bash"}"#, 1),
            make_trace_step(TraceStepType::ToolResult, r#"{"tool_use_id":"tu_1"}"#, 2),
            make_trace_step(TraceStepType::ModelResponse, r#"{"round":0,"text":"Hi","stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1},"latency_ms":10,"tool_uses":[]}"#, 3),
            make_trace_step(TraceStepType::Error, r#"{"context":"test","message":"err"}"#, 4),
        ];
        let provider = ReplayProvider::from_trace(&steps, "test").unwrap();
        assert_eq!(provider.remaining(), 1);
    }
}
