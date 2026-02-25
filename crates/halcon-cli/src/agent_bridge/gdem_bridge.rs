//! GDEM bridge — wires `halcon-agent-core`'s `run_gdem_loop` to the
//! `agent_bridge` execution layer. Compiled only when `feature = "gdem-primary"`.
//!
//! ## Design
//!
//! `GdemToolExecutor` implements `halcon_agent_core::loop_driver::ToolExecutor`
//! by dispatching to the `halcon-tools` `ToolRegistry` + `Tool::execute()`.
//!
//! `GdemLlmClient` implements `halcon_agent_core::loop_driver::LlmClient` by
//! calling `ModelProvider::invoke()` and collecting the full stream.
//!
//! ## Invariants
//!
//! - Default behavior is **unchanged**: `gdem-primary` is off by default.
//! - `gdem-primary` and `legacy-repl` can coexist; the bridge selection lives
//!   in `executor.rs::execute_turn()`.
//! - The GDEM bridge is purely additive — it does not touch REPL loop paths.

#![cfg(feature = "gdem-primary")]

use std::sync::Arc;
use anyhow::Result;
use async_trait::async_trait;
use futures::StreamExt;
use uuid::Uuid;

use halcon_agent_core::loop_driver::{
    GdemConfig, GdemContext, LlmClient, ToolCallResult, ToolExecutor,
};
use halcon_agent_core::router::EmbeddingProvider;
use halcon_core::traits::ModelProvider;
use halcon_core::types::{ChatMessage, ContentBlock, MessageContent, ModelChunk, ModelRequest, Role, ToolInput};
use halcon_tools::ToolRegistry;

// ── GdemToolExecutor ─────────────────────────────────────────────────────────

/// Bridges GDEM's `ToolExecutor` to the `halcon-tools` `ToolRegistry`.
pub struct GdemToolExecutor {
    registry: Arc<ToolRegistry>,
    working_dir: String,
}

impl GdemToolExecutor {
    pub fn new(registry: Arc<ToolRegistry>, working_dir: impl Into<String>) -> Self {
        Self { registry, working_dir: working_dir.into() }
    }
}

#[async_trait]
impl ToolExecutor for GdemToolExecutor {
    async fn execute_tool(&self, tool_name: &str, input: &str) -> Result<ToolCallResult> {
        let start = std::time::Instant::now();

        let Some(tool) = self.registry.get(tool_name) else {
            return Ok(ToolCallResult {
                tool_name: tool_name.to_string(),
                output: format!("tool '{tool_name}' not found in registry"),
                is_error: true,
                tokens_consumed: 0,
                latency_ms: start.elapsed().as_millis() as u64,
            });
        };

        let arguments: serde_json::Value = serde_json::from_str(input)
            .unwrap_or_else(|_| serde_json::json!({ "input": input }));

        let tool_input = ToolInput {
            tool_use_id: Uuid::new_v4().to_string(),
            arguments,
            working_directory: self.working_dir.clone(),
        };

        let latency_ms = start.elapsed().as_millis() as u64;
        match tool.execute(tool_input).await {
            Ok(output) => Ok(ToolCallResult {
                tool_name: tool_name.to_string(),
                output: output.content,
                is_error: output.is_error,
                tokens_consumed: 0,
                latency_ms,
            }),
            Err(e) => Ok(ToolCallResult {
                tool_name: tool_name.to_string(),
                output: format!("tool execution error: {e}"),
                is_error: true,
                tokens_consumed: 0,
                latency_ms,
            }),
        }
    }
}

// ── GdemLlmClient ─────────────────────────────────────────────────────────────

/// Bridges GDEM's `LlmClient` to a `ModelProvider`.
///
/// Calls `provider.invoke()` and collects the full text stream.
/// GDEM handles its own retry logic — this client is intentionally simple.
pub struct GdemLlmClient {
    provider: Arc<dyn ModelProvider>,
    model: String,
}

impl GdemLlmClient {
    pub fn new(provider: Arc<dyn ModelProvider>, model: impl Into<String>) -> Self {
        Self { provider, model: model.into() }
    }
}

#[async_trait]
impl LlmClient for GdemLlmClient {
    async fn complete(&self, system: &str, user: &str) -> Result<(String, u32)> {
        let request = ModelRequest {
            model: self.model.clone(),
            system: Some(system.to_string()),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![
                    ContentBlock::Text { text: user.to_string() },
                ]),
            }],
            max_tokens: Some(4096),
            temperature: Some(0.0),
            tools: vec![],
            stream: false,
        };

        let mut stream = self.provider.invoke(&request).await?;
        let mut text = String::new();
        let mut output_tokens: u32 = 0;

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(ModelChunk::TextDelta(delta)) => text.push_str(&delta),
                Ok(ModelChunk::Usage(usage)) => {
                    output_tokens = output_tokens.max(usage.output_tokens);
                }
                Ok(ModelChunk::Done(_)) | Ok(ModelChunk::Error(_)) => break,
                Ok(_) => {} // ToolUse*, ThinkingDelta — not expected in single-turn call
                Err(e) => {
                    tracing::warn!(model = %self.model, "gdem_llm_client stream error: {e}");
                    break;
                }
            }
        }

        Ok((text, output_tokens))
    }
}

// ── NullEmbeddingProvider ─────────────────────────────────────────────────────

/// Fallback embedding provider that returns zero-vectors.
///
/// Used when no semantic embedding service is configured. The GDEM's
/// `SemanticToolRouter` degrades gracefully to uniform random selection
/// when embeddings are zero.
pub struct NullEmbeddingProvider;

#[async_trait]
impl EmbeddingProvider for NullEmbeddingProvider {
    async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        Ok(vec![0.0f32; 384])
    }

    fn dimension(&self) -> usize {
        384
    }
}

// ── build_gdem_context ────────────────────────────────────────────────────────

/// Build a `GdemContext` from the agent bridge's resources.
///
/// Called by `executor.rs::execute_turn()` when `feature = "gdem-primary"` is
/// active. Uses `NullEmbeddingProvider` until a real embedding backend is wired.
pub fn build_gdem_context(
    session_id: uuid::Uuid,
    tool_registry: Arc<ToolRegistry>,
    provider: Arc<dyn ModelProvider>,
    model: impl Into<String>,
    working_dir: impl Into<String>,
) -> GdemContext {
    let tool_defs = tool_registry.tool_definitions();
    let tool_names: Vec<(String, String)> = tool_defs
        .iter()
        .map(|d| (d.name.clone(), d.description.clone()))
        .collect();

    GdemContext {
        session_id,
        config: GdemConfig::default(),
        tool_executor: Arc::new(GdemToolExecutor::new(tool_registry, working_dir)),
        llm_client: Arc::new(GdemLlmClient::new(provider, model)),
        embedding_provider: Arc::new(NullEmbeddingProvider),
        strategy_learner: None,
        memory: None,
        tool_registry: tool_names,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn null_embedding_provider_returns_384_dims() {
        let p = NullEmbeddingProvider;
        let v = p.embed("hello world").await.unwrap();
        assert_eq!(v.len(), 384);
        assert!(v.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn null_embedding_dimension_is_384() {
        assert_eq!(NullEmbeddingProvider.dimension(), 384);
    }
}
