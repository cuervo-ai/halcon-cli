//! OpenAI-compatible base provider.
//!
//! Encapsulates the OpenAI Chat Completions API format (SSE streaming, tool_calls,
//! Bearer auth). Used by both OpenAI and DeepSeek providers.

pub mod types;

use std::time::Duration;

use async_trait::async_trait;
use eventsource_stream::Eventsource as _;
use futures::stream::{self, BoxStream};
use futures::StreamExt;
use tracing::{debug, warn};

use cuervo_core::error::{CuervoError, Result};
use cuervo_core::traits::ModelProvider;
use cuervo_core::types::{
    ContentBlock, HttpConfig, MessageContent, ModelChunk, ModelInfo, ModelRequest, StopReason,
    TokenCost, TokenUsage,
};

use crate::http;
use types::{
    OpenAIChatMessage, OpenAIChatRequest, OpenAIFunctionDef, OpenAIMessageContent, OpenAISseChunk,
    OpenAITool, OpenAIToolCall, OpenAIFunctionCall, StreamOptions,
};

/// A provider that speaks the OpenAI Chat Completions protocol.
///
/// Parameterized by name, URL, key, and model list so it can serve
/// both OpenAI and DeepSeek (and any other OpenAI-compatible API).
pub struct OpenAICompatibleProvider {
    client: reqwest::Client,
    provider_name: String,
    api_key: String,
    base_url: String,
    models: Vec<ModelInfo>,
    http_config: HttpConfig,
}

impl std::fmt::Debug for OpenAICompatibleProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAICompatibleProvider")
            .field("provider_name", &self.provider_name)
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}

impl OpenAICompatibleProvider {
    /// Create a new OpenAI-compatible provider.
    pub fn new(
        provider_name: String,
        api_key: String,
        base_url: String,
        models: Vec<ModelInfo>,
        http_config: HttpConfig,
    ) -> Self {
        Self {
            client: http::build_client(&http_config),
            provider_name,
            api_key,
            base_url,
            models,
            http_config,
        }
    }

    /// Build the OpenAI chat request from a `ModelRequest`.
    pub fn build_request(&self, request: &ModelRequest) -> OpenAIChatRequest {
        let mut messages = Vec::new();

        // System prompt as a system-role message.
        if let Some(ref system) = request.system {
            messages.push(OpenAIChatMessage {
                role: "system".into(),
                content: Some(OpenAIMessageContent::Text(system.clone())),
                tool_calls: None,
                tool_call_id: None,
            });
        }

        for msg in &request.messages {
            let role = match msg.role {
                cuervo_core::types::Role::User => "user",
                cuervo_core::types::Role::Assistant => "assistant",
                cuervo_core::types::Role::System => "system",
            };

            match &msg.content {
                MessageContent::Text(t) => {
                    messages.push(OpenAIChatMessage {
                        role: role.into(),
                        content: Some(OpenAIMessageContent::Text(t.clone())),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }
                MessageContent::Blocks(blocks) => {
                    // Separate text, tool_use, and tool_result blocks.
                    let mut text_parts = Vec::new();
                    let mut tool_calls = Vec::new();
                    let mut tool_results = Vec::new();

                    for block in blocks {
                        match block {
                            ContentBlock::Text { text } => {
                                text_parts.push(text.clone());
                            }
                            ContentBlock::ToolUse { id, name, input } => {
                                tool_calls.push(OpenAIToolCall {
                                    id: id.clone(),
                                    call_type: "function".into(),
                                    function: OpenAIFunctionCall {
                                        name: name.clone(),
                                        arguments: serde_json::to_string(input)
                                            .unwrap_or_default(),
                                    },
                                });
                            }
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                ..
                            } => {
                                tool_results.push((tool_use_id.clone(), content.clone()));
                            }
                        }
                    }

                    // Assistant message with tool_calls.
                    if !tool_calls.is_empty() {
                        let content = if text_parts.is_empty() {
                            None
                        } else {
                            Some(OpenAIMessageContent::Text(text_parts.join("\n")))
                        };
                        messages.push(OpenAIChatMessage {
                            role: "assistant".into(),
                            content,
                            tool_calls: Some(tool_calls),
                            tool_call_id: None,
                        });
                    } else if !text_parts.is_empty() {
                        messages.push(OpenAIChatMessage {
                            role: role.into(),
                            content: Some(OpenAIMessageContent::Text(text_parts.join("\n"))),
                            tool_calls: None,
                            tool_call_id: None,
                        });
                    }

                    // Tool result messages.
                    for (tool_use_id, content) in tool_results {
                        messages.push(OpenAIChatMessage {
                            role: "tool".into(),
                            content: Some(OpenAIMessageContent::Text(content)),
                            tool_calls: None,
                            tool_call_id: Some(tool_use_id),
                        });
                    }
                }
            }
        }

        let tools: Vec<OpenAITool> = request
            .tools
            .iter()
            .map(|t| OpenAITool {
                tool_type: "function".into(),
                function: OpenAIFunctionDef {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.input_schema.clone(),
                },
            })
            .collect();

        let stream_options = if request.stream {
            Some(StreamOptions {
                include_usage: true,
            })
        } else {
            None
        };

        // Reasoning models (o1, o3-mini, deepseek-reasoner) require
        // max_completion_tokens instead of max_tokens, and do NOT
        // support the temperature parameter.
        let is_reasoning = self
            .models
            .iter()
            .any(|m| m.id == request.model && m.supports_reasoning);

        let (max_tokens, max_completion_tokens, temperature) = if is_reasoning {
            (None, request.max_tokens, None)
        } else {
            (request.max_tokens, None, request.temperature)
        };

        OpenAIChatRequest {
            model: request.model.clone(),
            messages,
            max_tokens,
            max_completion_tokens,
            temperature,
            stream: request.stream,
            tools,
            stream_options,
        }
    }

    /// Map an SSE chunk to ModelChunk variants.
    pub fn map_sse_chunk(chunk: &OpenAISseChunk) -> Vec<ModelChunk> {
        let mut results = Vec::new();

        for choice in &chunk.choices {
            if let Some(ref delta) = choice.delta {
                // Text content.
                if let Some(ref content) = delta.content {
                    if !content.is_empty() {
                        results.push(ModelChunk::TextDelta(content.clone()));
                    }
                }

                // Tool calls.
                if let Some(ref tool_calls) = delta.tool_calls {
                    for tc in tool_calls {
                        if let Some(ref func) = tc.function {
                            // If we have an id, this is a new tool call start.
                            if let Some(ref id) = tc.id {
                                let name = func.name.clone().unwrap_or_default();
                                results.push(ModelChunk::ToolUseStart {
                                    index: tc.index,
                                    id: id.clone(),
                                    name,
                                });
                            }
                            // If we have arguments, emit a delta.
                            if let Some(ref args) = func.arguments {
                                if !args.is_empty() {
                                    results.push(ModelChunk::ToolUseDelta {
                                        index: tc.index,
                                        partial_json: args.clone(),
                                    });
                                }
                            }
                        }
                    }
                }
            }

            // Finish reason.
            if let Some(ref reason) = choice.finish_reason {
                let stop = match reason.as_str() {
                    "stop" => StopReason::EndTurn,
                    "length" => StopReason::MaxTokens,
                    "tool_calls" => StopReason::ToolUse,
                    _ => StopReason::EndTurn,
                };
                results.push(ModelChunk::Done(stop));
            }
        }

        // Usage (sent as a separate chunk with stream_options.include_usage).
        if let Some(ref usage) = chunk.usage {
            results.push(ModelChunk::Usage(TokenUsage {
                input_tokens: usage.prompt_tokens,
                output_tokens: usage.completion_tokens,
                ..Default::default()
            }));
        }

        results
    }

    /// Build the SSE stream from an HTTP response.
    fn build_sse_stream(
        response: reqwest::Response,
        provider_name: String,
    ) -> BoxStream<'static, Result<ModelChunk>> {
        let byte_stream = response.bytes_stream();
        let sse_stream = byte_stream.eventsource();

        let chunk_stream = sse_stream
            .flat_map(move |sse_result| {
                // Use &provider_name (captured by move) — no per-chunk clone.
                match sse_result {
                    Ok(event) => {
                        let data = event.data;
                        if data.trim() == "[DONE]" {
                            return stream::iter(vec![]);
                        }
                        match serde_json::from_str::<OpenAISseChunk>(&data) {
                            Ok(chunk) => {
                                let mapped: Vec<Result<ModelChunk>> =
                                    Self::map_sse_chunk(&chunk).into_iter().map(Ok).collect();
                                stream::iter(mapped)
                            }
                            Err(e) => {
                                warn!(
                                    provider = %provider_name,
                                    error = %e,
                                    data = %data,
                                    "Failed to parse SSE chunk"
                                );
                                stream::iter(vec![])
                            }
                        }
                    }
                    Err(e) => {
                        warn!(provider = %provider_name, error = %e, "SSE stream error");
                        stream::iter(vec![Err(CuervoError::StreamError(format!(
                            "{} SSE error: {e}", provider_name
                        )))])
                    }
                }
            });

        Box::pin(chunk_stream)
    }

    /// Estimate token count from message text (rough: ~4 chars per token).
    fn estimate_tokens(request: &ModelRequest) -> u32 {
        let chars: usize = request
            .messages
            .iter()
            .map(|m| match &m.content {
                MessageContent::Text(t) => t.len(),
                MessageContent::Blocks(blocks) => blocks
                    .iter()
                    .map(|b| match b {
                        ContentBlock::Text { text } => text.len(),
                        ContentBlock::ToolResult { content, .. } => content.len(),
                        ContentBlock::ToolUse { input, .. } => estimate_value_size(input),
                    })
                    .sum(),
            })
            .sum();
        (chars / 4) as u32
    }
}

/// Estimate the serialized size of a `serde_json::Value` without allocating a String.
pub(crate) fn estimate_value_size(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Null => 4,
        serde_json::Value::Bool(b) => if *b { 4 } else { 5 },
        serde_json::Value::Number(n) => {
            // itoa is typically 1-20 digits; use display len as approximation.
            // Avoid allocation — estimate based on magnitude.
            let n64 = n.as_f64().unwrap_or(0.0);
            if n64 == 0.0 { 1 } else { (n64.abs().log10() as usize).saturating_add(2) }
        }
        serde_json::Value::String(s) => s.len() + 2, // quotes
        serde_json::Value::Array(arr) => {
            // brackets + commas + element sizes
            2 + arr.iter().map(estimate_value_size).sum::<usize>() + arr.len().saturating_sub(1)
        }
        serde_json::Value::Object(map) => {
            // braces + key:value pairs + commas
            2 + map
                .iter()
                .map(|(k, v)| k.len() + 3 + estimate_value_size(v)) // "key":value
                .sum::<usize>()
                + map.len().saturating_sub(1)
        }
    }
}

#[async_trait]
impl ModelProvider for OpenAICompatibleProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    fn supported_models(&self) -> &[ModelInfo] {
        &self.models
    }

    async fn invoke(
        &self,
        request: &ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelChunk>>> {
        let chat_request = self.build_request(request);
        let url = format!("{}/chat/completions", self.base_url);
        let max_retries = self.http_config.max_retries;
        let timeout_secs = self.http_config.request_timeout_secs;

        debug!(
            provider = %self.provider_name,
            model = %chat_request.model,
            messages = chat_request.messages.len(),
            "Invoking OpenAI-compatible API"
        );

        for attempt in 0..=max_retries {
            if attempt > 0 {
                let delay = http::backoff_delay(1000, attempt);
                tokio::time::sleep(delay).await;
            }

            let result = tokio::time::timeout(
                Duration::from_secs(timeout_secs),
                self.client
                    .post(&url)
                    .bearer_auth(&self.api_key)
                    .json(&chat_request)
                    .send(),
            )
            .await;

            let response = match result {
                Ok(Ok(resp)) => resp,
                Ok(Err(e)) => {
                    if e.is_connect() {
                        if attempt < max_retries {
                            warn!(
                                provider = %self.provider_name,
                                attempt = attempt + 1,
                                "Connection error, retrying"
                            );
                            continue;
                        }
                        return Err(CuervoError::ConnectionError {
                            provider: self.provider_name.clone(),
                            message: format!("Cannot connect to {}: {e}", self.base_url),
                        });
                    }
                    return Err(CuervoError::ApiError {
                        message: format!("{} request failed: {e}", self.provider_name),
                        status: e.status().map(|s| s.as_u16()),
                    });
                }
                Err(_) => {
                    if attempt < max_retries {
                        warn!(
                            provider = %self.provider_name,
                            attempt = attempt + 1,
                            "Request timeout, retrying"
                        );
                        continue;
                    }
                    return Err(CuervoError::RequestTimeout {
                        provider: self.provider_name.clone(),
                        timeout_secs,
                    });
                }
            };

            let status = response.status();

            if status.as_u16() == 401 {
                return Err(CuervoError::AuthFailed(format!(
                    "{}: invalid API key",
                    self.provider_name
                )));
            }

            if status.as_u16() == 429 {
                if let Some(retry_after) = http::parse_retry_after(response.headers()) {
                    if attempt < max_retries {
                        tokio::time::sleep(Duration::from_secs(retry_after)).await;
                        continue;
                    }
                }
                return Err(CuervoError::RateLimited {
                    provider: self.provider_name.clone(),
                    retry_after_secs: http::parse_retry_after(response.headers()).unwrap_or(60),
                });
            }

            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                if http::is_retryable_status(status.as_u16()) && attempt < max_retries {
                    warn!(
                        provider = %self.provider_name,
                        status = status.as_u16(),
                        attempt = attempt + 1,
                        "Retryable error"
                    );
                    continue;
                }
                return Err(CuervoError::ApiError {
                    message: format!("{} returned {status}: {body}", self.provider_name),
                    status: Some(status.as_u16()),
                });
            }

            return Ok(Self::build_sse_stream(response, self.provider_name.clone()));
        }

        Err(CuervoError::ApiError {
            message: format!("{}: max retries exceeded", self.provider_name),
            status: None,
        })
    }

    async fn is_available(&self) -> bool {
        !self.api_key.is_empty()
    }

    fn estimate_cost(&self, request: &ModelRequest) -> TokenCost {
        let estimated_tokens = Self::estimate_tokens(request);
        let cost_per_input = self
            .models
            .iter()
            .find(|m| m.id == request.model)
            .map(|m| m.cost_per_input_token)
            .unwrap_or(0.0);

        TokenCost {
            estimated_input_tokens: estimated_tokens,
            estimated_cost_usd: estimated_tokens as f64 * cost_per_input,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cuervo_core::types::{ChatMessage, Role, ToolDefinition};

    fn test_models() -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: "test-model".into(),
            name: "Test Model".into(),
            provider: "test".into(),
            context_window: 128_000,
            max_output_tokens: 16384,
            supports_streaming: true,
            supports_tools: true,
            supports_vision: false,
            supports_reasoning: false,
            cost_per_input_token: 2.5 / 1_000_000.0,
            cost_per_output_token: 10.0 / 1_000_000.0,
        }]
    }

    fn make_request(msg: &str) -> ModelRequest {
        ModelRequest {
            model: "test-model".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text(msg.into()),
            }],
            tools: vec![],
            max_tokens: Some(1024),
            temperature: Some(0.7),
            system: None,
            stream: true,
        }
    }

    fn make_provider() -> OpenAICompatibleProvider {
        OpenAICompatibleProvider::new(
            "test".into(),
            "sk-test-key".into(),
            "https://api.test.com/v1".into(),
            test_models(),
            HttpConfig::default(),
        )
    }

    #[test]
    fn provider_name() {
        let provider = make_provider();
        assert_eq!(provider.name(), "test");
    }

    #[test]
    fn provider_models() {
        let provider = make_provider();
        let models = provider.supported_models();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "test-model");
    }

    #[test]
    fn debug_redacts_key() {
        let provider = make_provider();
        let debug = format!("{provider:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("sk-test-key"));
    }

    #[test]
    fn build_request_basic() {
        let provider = make_provider();
        let req = make_request("hello");
        let chat_req = provider.build_request(&req);
        assert_eq!(chat_req.model, "test-model");
        assert_eq!(chat_req.messages.len(), 1);
        assert_eq!(chat_req.messages[0].role, "user");
        assert!(chat_req.tools.is_empty());
        // Non-reasoning model → max_tokens set, max_completion_tokens None
        assert_eq!(chat_req.max_tokens, Some(1024));
        assert!(chat_req.max_completion_tokens.is_none());
    }

    #[test]
    fn build_request_with_system() {
        let provider = make_provider();
        let req = ModelRequest {
            model: "test-model".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text("hello".into()),
            }],
            tools: vec![],
            max_tokens: Some(1024),
            temperature: None,
            system: Some("You are helpful.".into()),
            stream: true,
        };
        let chat_req = provider.build_request(&req);
        assert_eq!(chat_req.messages.len(), 2);
        assert_eq!(chat_req.messages[0].role, "system");
    }

    #[test]
    fn build_request_with_tools() {
        let provider = make_provider();
        let req = ModelRequest {
            model: "test-model".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text("test".into()),
            }],
            tools: vec![ToolDefinition {
                name: "bash".into(),
                description: "Run a command".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }],
            max_tokens: Some(1024),
            temperature: None,
            system: None,
            stream: true,
        };
        let chat_req = provider.build_request(&req);
        assert_eq!(chat_req.tools.len(), 1);
        assert_eq!(chat_req.tools[0].function.name, "bash");
    }

    #[test]
    fn build_request_reasoning_model_uses_max_completion_tokens() {
        let models = vec![ModelInfo {
            id: "o3-mini".into(),
            name: "o3 Mini".into(),
            provider: "openai".into(),
            context_window: 200_000,
            max_output_tokens: 100_000,
            supports_streaming: true,
            supports_tools: true,
            supports_vision: false,
            supports_reasoning: true,
            cost_per_input_token: 1.10 / 1_000_000.0,
            cost_per_output_token: 4.40 / 1_000_000.0,
        }];
        let provider = OpenAICompatibleProvider::new(
            "openai".into(),
            "sk-test".into(),
            "https://api.openai.com/v1".into(),
            models,
            HttpConfig::default(),
        );
        let req = ModelRequest {
            model: "o3-mini".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text("reason about this".into()),
            }],
            tools: vec![],
            max_tokens: Some(8192),
            temperature: None,
            system: None,
            stream: true,
        };
        let chat_req = provider.build_request(&req);
        // Reasoning model → max_tokens None, max_completion_tokens set, temperature stripped
        assert!(chat_req.max_tokens.is_none());
        assert_eq!(chat_req.max_completion_tokens, Some(8192));
        assert!(chat_req.temperature.is_none());
    }

    #[test]
    fn build_request_reasoning_model_strips_temperature() {
        let models = vec![ModelInfo {
            id: "o3-mini".into(),
            name: "o3 Mini".into(),
            provider: "openai".into(),
            context_window: 200_000,
            max_output_tokens: 100_000,
            supports_streaming: true,
            supports_tools: true,
            supports_vision: false,
            supports_reasoning: true,
            cost_per_input_token: 1.10 / 1_000_000.0,
            cost_per_output_token: 4.40 / 1_000_000.0,
        }];
        let provider = OpenAICompatibleProvider::new(
            "openai".into(),
            "sk-test".into(),
            "https://api.openai.com/v1".into(),
            models,
            HttpConfig::default(),
        );
        let req = ModelRequest {
            model: "o3-mini".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text("reason about this".into()),
            }],
            tools: vec![],
            max_tokens: Some(8192),
            temperature: Some(0.7), // Explicitly set — should be stripped
            system: None,
            stream: true,
        };
        let chat_req = provider.build_request(&req);
        // Reasoning model: temperature must be stripped even when explicitly set
        assert!(
            chat_req.temperature.is_none(),
            "reasoning models must not send temperature"
        );
        // max_completion_tokens used instead of max_tokens
        assert!(chat_req.max_tokens.is_none());
        assert_eq!(chat_req.max_completion_tokens, Some(8192));
    }

    #[test]
    fn build_request_non_reasoning_preserves_temperature() {
        let provider = make_provider();
        let req = ModelRequest {
            model: "test-model".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text("hello".into()),
            }],
            tools: vec![],
            max_tokens: Some(1024),
            temperature: Some(0.5),
            system: None,
            stream: true,
        };
        let chat_req = provider.build_request(&req);
        assert_eq!(chat_req.temperature, Some(0.5));
        assert_eq!(chat_req.max_tokens, Some(1024));
        assert!(chat_req.max_completion_tokens.is_none());
    }

    #[test]
    fn map_sse_chunk_text() {
        let chunk = OpenAISseChunk {
            id: Some("chatcmpl-1".into()),
            choices: vec![types::OpenAIChoice {
                index: 0,
                delta: Some(types::OpenAIDelta {
                    role: None,
                    content: Some("Hello".into()),
                    tool_calls: None,
                }),
                finish_reason: None,
            }],
            usage: None,
        };
        let mapped = OpenAICompatibleProvider::map_sse_chunk(&chunk);
        assert_eq!(mapped.len(), 1);
        assert!(matches!(&mapped[0], ModelChunk::TextDelta(t) if t == "Hello"));
    }

    #[test]
    fn map_sse_chunk_tool_call() {
        let chunk = OpenAISseChunk {
            id: Some("chatcmpl-1".into()),
            choices: vec![types::OpenAIChoice {
                index: 0,
                delta: Some(types::OpenAIDelta {
                    role: None,
                    content: None,
                    tool_calls: Some(vec![types::OpenAIToolCallDelta {
                        index: 0,
                        id: Some("call_abc".into()),
                        function: Some(types::OpenAIFunctionDelta {
                            name: Some("bash".into()),
                            arguments: Some("{\"cmd\":".into()),
                        }),
                    }]),
                }),
                finish_reason: None,
            }],
            usage: None,
        };
        let mapped = OpenAICompatibleProvider::map_sse_chunk(&chunk);
        assert_eq!(mapped.len(), 2); // ToolUseStart + ToolUseDelta
        assert!(matches!(&mapped[0], ModelChunk::ToolUseStart { name, .. } if name == "bash"));
        assert!(matches!(&mapped[1], ModelChunk::ToolUseDelta { partial_json, .. } if partial_json == "{\"cmd\":"));
    }

    #[test]
    fn map_sse_chunk_done() {
        let chunk = OpenAISseChunk {
            id: Some("chatcmpl-1".into()),
            choices: vec![types::OpenAIChoice {
                index: 0,
                delta: Some(types::OpenAIDelta {
                    role: None,
                    content: None,
                    tool_calls: None,
                }),
                finish_reason: Some("stop".into()),
            }],
            usage: None,
        };
        let mapped = OpenAICompatibleProvider::map_sse_chunk(&chunk);
        assert_eq!(mapped.len(), 1);
        assert!(matches!(mapped[0], ModelChunk::Done(StopReason::EndTurn)));
    }

    #[test]
    fn map_sse_chunk_usage() {
        let chunk = OpenAISseChunk {
            id: Some("chatcmpl-1".into()),
            choices: vec![],
            usage: Some(types::OpenAIUsage {
                prompt_tokens: 50,
                completion_tokens: 100,
            }),
        };
        let mapped = OpenAICompatibleProvider::map_sse_chunk(&chunk);
        assert_eq!(mapped.len(), 1);
        assert!(matches!(&mapped[0], ModelChunk::Usage(u) if u.input_tokens == 50 && u.output_tokens == 100));
    }

    #[test]
    fn estimate_cost_uses_model_pricing() {
        let provider = make_provider();
        let req = make_request("test message for cost estimation");
        let cost = provider.estimate_cost(&req);
        assert!(cost.estimated_input_tokens > 0);
        assert!(cost.estimated_cost_usd > 0.0);
    }

    #[tokio::test]
    async fn is_available_with_key() {
        let provider = make_provider();
        assert!(provider.is_available().await);
    }

    #[tokio::test]
    async fn is_available_without_key() {
        let provider = OpenAICompatibleProvider::new(
            "test".into(),
            "".into(),
            "https://api.test.com/v1".into(),
            test_models(),
            HttpConfig::default(),
        );
        assert!(!provider.is_available().await);
    }
}
