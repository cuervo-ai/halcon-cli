//! Gemini provider with Google's unique API format.
//!
//! Key differences from OpenAI:
//! - URL: `/v1beta/models/{model}:streamGenerateContent?alt=sse&key={api_key}`
//! - Auth: API key in query param (not header)
//! - Body: `contents` with `parts`, not `messages`
//! - Function calls are complete (not streaming deltas)

pub mod types;

use std::time::Duration;

use async_trait::async_trait;
use eventsource_stream::Eventsource as _;
use futures::stream::{self, BoxStream};
use futures::StreamExt;
use tracing::{debug, warn};
use uuid::Uuid;

use cuervo_core::error::{CuervoError, Result};
use cuervo_core::traits::ModelProvider;
use cuervo_core::types::{
    ContentBlock, HttpConfig, MessageContent, ModelChunk, ModelInfo, ModelRequest, StopReason,
    TokenCost, TokenUsage,
};

use crate::http;
use types::{
    GeminiContent, GeminiFunctionCall, GeminiFunctionDecl, GeminiFunctionResponse,
    GeminiGenerationConfig, GeminiPart, GeminiRequest, GeminiStreamChunk, GeminiToolDeclaration,
};

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";

/// Gemini provider for Google's generative AI API.
pub struct GeminiProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    http_config: HttpConfig,
    models: Vec<ModelInfo>,
}

impl std::fmt::Debug for GeminiProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeminiProvider")
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}

impl GeminiProvider {
    fn default_models() -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "gemini-2.0-flash".into(),
                name: "Gemini 2.0 Flash".into(),
                provider: "gemini".into(),
                context_window: 1_048_576,
                max_output_tokens: 8192,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                supports_reasoning: false,
                cost_per_input_token: 0.10 / 1_000_000.0,
                cost_per_output_token: 0.40 / 1_000_000.0,
            },
            ModelInfo {
                id: "gemini-2.5-pro".into(),
                name: "Gemini 2.5 Pro".into(),
                provider: "gemini".into(),
                context_window: 1_048_576,
                max_output_tokens: 65536,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                supports_reasoning: true,
                cost_per_input_token: 1.25 / 1_000_000.0,
                cost_per_output_token: 10.0 / 1_000_000.0,
            },
        ]
    }

    /// Create a new Gemini provider.
    pub fn new(api_key: String, base_url: Option<String>, http_config: HttpConfig) -> Self {
        Self {
            client: http::build_client(&http_config),
            api_key,
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            http_config,
            models: Self::default_models(),
        }
    }

    /// Build a Gemini request from a `ModelRequest`.
    pub fn build_request(request: &ModelRequest) -> GeminiRequest {
        let mut contents = Vec::new();

        for msg in &request.messages {
            let role = match msg.role {
                cuervo_core::types::Role::User => "user",
                cuervo_core::types::Role::Assistant => "model",
                cuervo_core::types::Role::System => "user", // Gemini uses systemInstruction for system
            };

            match &msg.content {
                MessageContent::Text(t) => {
                    contents.push(GeminiContent {
                        role: Some(role.into()),
                        parts: vec![GeminiPart::Text { text: t.clone() }],
                    });
                }
                MessageContent::Blocks(blocks) => {
                    let mut parts = Vec::new();
                    let mut tool_response_parts = Vec::new();

                    for block in blocks {
                        match block {
                            ContentBlock::Text { text } => {
                                parts.push(GeminiPart::Text { text: text.clone() });
                            }
                            ContentBlock::ToolUse { name, input, .. } => {
                                parts.push(GeminiPart::FunctionCall {
                                    function_call: GeminiFunctionCall {
                                        name: name.clone(),
                                        args: input.clone(),
                                    },
                                });
                            }
                            ContentBlock::ToolResult {
                                content,
                                tool_use_id,
                                ..
                            } => {
                                tool_response_parts.push(GeminiPart::FunctionResponse {
                                    function_response: GeminiFunctionResponse {
                                        name: tool_use_id.clone(),
                                        response: serde_json::json!({"result": content}),
                                    },
                                });
                            }
                        }
                    }

                    if !parts.is_empty() {
                        contents.push(GeminiContent {
                            role: Some(role.into()),
                            parts,
                        });
                    }

                    if !tool_response_parts.is_empty() {
                        contents.push(GeminiContent {
                            role: Some("user".into()),
                            parts: tool_response_parts,
                        });
                    }
                }
            }
        }

        let system_instruction = request.system.as_ref().map(|s| GeminiContent {
            role: None,
            parts: vec![GeminiPart::Text { text: s.clone() }],
        });

        let tools: Vec<GeminiToolDeclaration> = if request.tools.is_empty() {
            vec![]
        } else {
            vec![GeminiToolDeclaration {
                function_declarations: request
                    .tools
                    .iter()
                    .map(|t| GeminiFunctionDecl {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        parameters: t.input_schema.clone(),
                    })
                    .collect(),
            }]
        };

        let generation_config =
            if request.temperature.is_some() || request.max_tokens.is_some() {
                Some(GeminiGenerationConfig {
                    temperature: request.temperature,
                    max_output_tokens: request.max_tokens,
                })
            } else {
                None
            };

        GeminiRequest {
            contents,
            system_instruction,
            tools,
            generation_config,
        }
    }

    /// Map a Gemini stream chunk to ModelChunk variants.
    pub fn map_stream_chunk(chunk: &GeminiStreamChunk) -> Vec<ModelChunk> {
        let mut results = Vec::new();

        for candidate in &chunk.candidates {
            if let Some(ref content) = candidate.content {
                for part in &content.parts {
                    match part {
                        GeminiPart::Text { text } => {
                            if !text.is_empty() {
                                results.push(ModelChunk::TextDelta(text.clone()));
                            }
                        }
                        GeminiPart::FunctionCall { function_call } => {
                            // Gemini sends complete function calls (not streaming deltas).
                            let id = format!("gemini_{}", Uuid::new_v4());
                            results.push(ModelChunk::ToolUse {
                                id,
                                name: function_call.name.clone(),
                                input: function_call.args.clone(),
                            });
                        }
                        GeminiPart::FunctionResponse { .. } => {
                            // Function responses are sent by us, not received.
                        }
                    }
                }
            }

            if let Some(ref reason) = candidate.finish_reason {
                let stop = match reason.as_str() {
                    "STOP" => StopReason::EndTurn,
                    "MAX_TOKENS" => StopReason::MaxTokens,
                    "SAFETY" => StopReason::StopSequence,
                    _ => StopReason::EndTurn,
                };
                results.push(ModelChunk::Done(stop));
            }
        }

        if let Some(ref usage) = chunk.usage_metadata {
            results.push(ModelChunk::Usage(TokenUsage {
                input_tokens: usage.prompt_token_count,
                output_tokens: usage.candidates_token_count,
                ..Default::default()
            }));
        }

        results
    }

    /// Build the SSE stream from an HTTP response.
    fn build_sse_stream(response: reqwest::Response) -> BoxStream<'static, Result<ModelChunk>> {
        let byte_stream = response.bytes_stream();
        let sse_stream = byte_stream.eventsource();

        let chunk_stream = sse_stream.flat_map(|sse_result| match sse_result {
            Ok(event) => {
                let data = event.data;
                if data.trim().is_empty() {
                    return stream::iter(vec![]);
                }
                match serde_json::from_str::<GeminiStreamChunk>(&data) {
                    Ok(chunk) => {
                        let mapped: Vec<Result<ModelChunk>> =
                            Self::map_stream_chunk(&chunk).into_iter().map(Ok).collect();
                        stream::iter(mapped)
                    }
                    Err(e) => {
                        warn!(error = %e, data = %data, "Failed to parse Gemini SSE chunk");
                        stream::iter(vec![])
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "Gemini SSE stream error");
                stream::iter(vec![Err(CuervoError::StreamError(format!(
                    "Gemini SSE error: {e}"
                )))])
            }
        });

        Box::pin(chunk_stream)
    }
}

#[async_trait]
impl ModelProvider for GeminiProvider {
    fn name(&self) -> &str {
        "gemini"
    }

    fn supported_models(&self) -> &[ModelInfo] {
        &self.models
    }

    async fn invoke(
        &self,
        request: &ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelChunk>>> {
        let gemini_request = Self::build_request(request);
        let url = format!(
            "{}/v1beta/models/{}:streamGenerateContent?alt=sse&key={}",
            self.base_url, request.model, self.api_key
        );
        let max_retries = self.http_config.max_retries;
        let timeout_secs = self.http_config.request_timeout_secs;

        debug!(
            model = %request.model,
            contents = gemini_request.contents.len(),
            "Invoking Gemini"
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
                    .json(&gemini_request)
                    .send(),
            )
            .await;

            let response = match result {
                Ok(Ok(resp)) => resp,
                Ok(Err(e)) => {
                    if e.is_connect() && attempt < max_retries {
                        warn!(attempt = attempt + 1, "Gemini connection error, retrying");
                        continue;
                    }
                    if e.is_connect() {
                        return Err(CuervoError::ConnectionError {
                            provider: "gemini".into(),
                            message: format!("Cannot connect to Gemini: {e}"),
                        });
                    }
                    return Err(CuervoError::ApiError {
                        message: format!("Gemini request failed: {e}"),
                        status: e.status().map(|s| s.as_u16()),
                    });
                }
                Err(_) => {
                    if attempt < max_retries {
                        warn!(attempt = attempt + 1, "Gemini timeout, retrying");
                        continue;
                    }
                    return Err(CuervoError::RequestTimeout {
                        provider: "gemini".into(),
                        timeout_secs,
                    });
                }
            };

            let status = response.status();

            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(CuervoError::AuthFailed(
                    "Gemini: invalid API key".into(),
                ));
            }

            if status.as_u16() == 429 {
                if attempt < max_retries {
                    let delay = http::backoff_delay(2000, attempt);
                    tokio::time::sleep(delay).await;
                    continue;
                }
                return Err(CuervoError::RateLimited {
                    provider: "gemini".into(),
                    retry_after_secs: 60,
                });
            }

            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                if http::is_retryable_status(status.as_u16()) && attempt < max_retries {
                    warn!(
                        status = status.as_u16(),
                        attempt = attempt + 1,
                        "Gemini retryable error"
                    );
                    continue;
                }
                return Err(CuervoError::ApiError {
                    message: format!("Gemini returned {status}: {body}"),
                    status: Some(status.as_u16()),
                });
            }

            return Ok(Self::build_sse_stream(response));
        }

        Err(CuervoError::ApiError {
            message: "Gemini: max retries exceeded".into(),
            status: None,
        })
    }

    async fn is_available(&self) -> bool {
        !self.api_key.is_empty()
    }

    fn estimate_cost(&self, request: &ModelRequest) -> TokenCost {
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
                        ContentBlock::ToolUse { input, .. } => {
                            crate::openai_compat::estimate_value_size(input)
                        }
                    })
                    .sum(),
            })
            .sum();
        let estimated_tokens = (chars / 4) as u32;

        let cost_per_input = self
            .supported_models()
            .iter()
            .find(|m| m.id == request.model)
            .map(|m| m.cost_per_input_token)
            .unwrap_or(0.10 / 1_000_000.0);

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

    fn make_request(msg: &str) -> ModelRequest {
        ModelRequest {
            model: "gemini-2.0-flash".into(),
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

    #[test]
    fn name_is_gemini() {
        let provider = GeminiProvider::new("test-key".into(), None, HttpConfig::default());
        assert_eq!(provider.name(), "gemini");
    }

    #[test]
    fn supported_models_count() {
        let provider = GeminiProvider::new("test-key".into(), None, HttpConfig::default());
        let models = provider.supported_models();
        assert_eq!(models.len(), 2);
        for m in models {
            assert_eq!(m.provider, "gemini");
            assert!(m.context_window > 0);
        }
    }

    #[test]
    fn debug_redacts_key() {
        let provider = GeminiProvider::new("secret-key".into(), None, HttpConfig::default());
        let debug = format!("{provider:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("secret-key"));
    }

    #[test]
    fn build_request_basic() {
        let req = make_request("hello");
        let gemini_req = GeminiProvider::build_request(&req);
        assert_eq!(gemini_req.contents.len(), 1);
        assert_eq!(gemini_req.contents[0].role.as_deref(), Some("user"));
        assert!(gemini_req.system_instruction.is_none());
        assert!(gemini_req.tools.is_empty());
    }

    #[test]
    fn build_request_with_system() {
        let req = ModelRequest {
            model: "gemini-2.0-flash".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text("hello".into()),
            }],
            tools: vec![],
            max_tokens: None,
            temperature: None,
            system: Some("You are helpful.".into()),
            stream: true,
        };
        let gemini_req = GeminiProvider::build_request(&req);
        assert!(gemini_req.system_instruction.is_some());
        let sys = gemini_req.system_instruction.unwrap();
        match &sys.parts[0] {
            GeminiPart::Text { text } => assert_eq!(text, "You are helpful."),
            _ => panic!("Expected text part"),
        }
    }

    #[test]
    fn build_request_with_tools() {
        let req = ModelRequest {
            model: "gemini-2.0-flash".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text("test".into()),
            }],
            tools: vec![ToolDefinition {
                name: "bash".into(),
                description: "Run a command".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }],
            max_tokens: None,
            temperature: None,
            system: None,
            stream: true,
        };
        let gemini_req = GeminiProvider::build_request(&req);
        assert_eq!(gemini_req.tools.len(), 1);
        assert_eq!(gemini_req.tools[0].function_declarations[0].name, "bash");
    }

    #[test]
    fn build_request_assistant_role_mapped_to_model() {
        let req = ModelRequest {
            model: "gemini-2.0-flash".into(),
            messages: vec![
                ChatMessage {
                    role: Role::User,
                    content: MessageContent::Text("hello".into()),
                },
                ChatMessage {
                    role: Role::Assistant,
                    content: MessageContent::Text("hi there".into()),
                },
            ],
            tools: vec![],
            max_tokens: None,
            temperature: None,
            system: None,
            stream: true,
        };
        let gemini_req = GeminiProvider::build_request(&req);
        assert_eq!(gemini_req.contents[1].role.as_deref(), Some("model"));
    }

    #[test]
    fn map_stream_chunk_text() {
        let chunk = GeminiStreamChunk {
            candidates: vec![types::GeminiCandidate {
                content: Some(GeminiContent {
                    role: Some("model".into()),
                    parts: vec![GeminiPart::Text {
                        text: "Hello".into(),
                    }],
                }),
                finish_reason: None,
            }],
            usage_metadata: None,
        };
        let mapped = GeminiProvider::map_stream_chunk(&chunk);
        assert_eq!(mapped.len(), 1);
        assert!(matches!(&mapped[0], ModelChunk::TextDelta(t) if t == "Hello"));
    }

    #[test]
    fn map_stream_chunk_function_call() {
        let chunk = GeminiStreamChunk {
            candidates: vec![types::GeminiCandidate {
                content: Some(GeminiContent {
                    role: Some("model".into()),
                    parts: vec![GeminiPart::FunctionCall {
                        function_call: GeminiFunctionCall {
                            name: "bash".into(),
                            args: serde_json::json!({"command": "ls"}),
                        },
                    }],
                }),
                finish_reason: Some("STOP".into()),
            }],
            usage_metadata: None,
        };
        let mapped = GeminiProvider::map_stream_chunk(&chunk);
        assert_eq!(mapped.len(), 2); // ToolUse + Done
        assert!(matches!(&mapped[0], ModelChunk::ToolUse { name, .. } if name == "bash"));
        assert!(matches!(mapped[1], ModelChunk::Done(StopReason::EndTurn)));
    }

    #[test]
    fn map_stream_chunk_usage() {
        let chunk = GeminiStreamChunk {
            candidates: vec![],
            usage_metadata: Some(types::GeminiUsageMetadata {
                prompt_token_count: 25,
                candidates_token_count: 100,
            }),
        };
        let mapped = GeminiProvider::map_stream_chunk(&chunk);
        assert_eq!(mapped.len(), 1);
        assert!(matches!(&mapped[0], ModelChunk::Usage(u) if u.input_tokens == 25 && u.output_tokens == 100));
    }

    #[test]
    fn map_stream_chunk_finish_reasons() {
        for (reason, expected) in [
            ("STOP", StopReason::EndTurn),
            ("MAX_TOKENS", StopReason::MaxTokens),
            ("SAFETY", StopReason::StopSequence),
        ] {
            let chunk = GeminiStreamChunk {
                candidates: vec![types::GeminiCandidate {
                    content: None,
                    finish_reason: Some(reason.into()),
                }],
                usage_metadata: None,
            };
            let mapped = GeminiProvider::map_stream_chunk(&chunk);
            assert_eq!(mapped.len(), 1);
            assert!(matches!(&mapped[0], ModelChunk::Done(s) if *s == expected));
        }
    }

    #[test]
    fn estimate_cost_positive() {
        let provider = GeminiProvider::new("test-key".into(), None, HttpConfig::default());
        let req = make_request("test message for cost estimation");
        let cost = provider.estimate_cost(&req);
        assert!(cost.estimated_input_tokens > 0);
        assert!(cost.estimated_cost_usd > 0.0);
    }

    #[tokio::test]
    async fn is_available_with_key() {
        let provider = GeminiProvider::new("test-key".into(), None, HttpConfig::default());
        assert!(provider.is_available().await);
    }

    #[tokio::test]
    async fn is_available_without_key() {
        let provider = GeminiProvider::new("".into(), None, HttpConfig::default());
        assert!(!provider.is_available().await);
    }

    #[test]
    fn custom_base_url() {
        let provider = GeminiProvider::new(
            "key".into(),
            Some("https://custom.example.com".into()),
            HttpConfig::default(),
        );
        let debug = format!("{provider:?}");
        assert!(debug.contains("custom.example.com"));
    }
}
