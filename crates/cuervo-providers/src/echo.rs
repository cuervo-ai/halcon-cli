use async_trait::async_trait;
use futures::stream::{self, BoxStream};

use cuervo_core::error::Result;
use cuervo_core::traits::ModelProvider;
use cuervo_core::types::{ModelChunk, ModelInfo, ModelRequest, StopReason, TokenCost, TokenUsage};

/// Echo provider for testing and development.
///
/// Returns the last user message back as markdown-formatted assistant response.
/// Simulates streaming by emitting one chunk per word.
pub struct EchoProvider {
    models: Vec<ModelInfo>,
}

impl EchoProvider {
    pub fn new() -> Self {
        Self {
            models: vec![ModelInfo {
                id: "echo".into(),
                name: "Echo (dev)".into(),
                provider: "echo".into(),
                context_window: 4096,
                max_output_tokens: 4096,
                supports_streaming: true,
                supports_tools: false,
                supports_vision: false,
                supports_reasoning: false,
                cost_per_input_token: 0.0,
                cost_per_output_token: 0.0,
            }],
        }
    }
}

impl Default for EchoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModelProvider for EchoProvider {
    fn name(&self) -> &str {
        "echo"
    }

    fn supported_models(&self) -> &[ModelInfo] {
        &self.models
    }

    async fn invoke(
        &self,
        request: &ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelChunk>>> {
        // Extract the last user message.
        let user_msg = request
            .messages
            .iter()
            .rev()
            .find(|m| m.role == cuervo_core::types::Role::User)
            .and_then(|m| m.content.as_text())
            .unwrap_or("(no message)")
            .to_string();

        let response = format!("**Echo:** {user_msg}");
        let word_count = response.split_whitespace().count() as u32;

        // Emit one chunk per word to simulate streaming.
        let words: Vec<String> = response
            .split_inclusive(char::is_whitespace)
            .map(String::from)
            .collect();

        let usage = TokenUsage {
            input_tokens: user_msg.len() as u32 / 4,
            output_tokens: word_count,
            ..Default::default()
        };

        let chunks: Vec<Result<ModelChunk>> = words
            .into_iter()
            .map(|w| Ok(ModelChunk::TextDelta(w)))
            .chain(std::iter::once(Ok(ModelChunk::Usage(usage))))
            .chain(std::iter::once(Ok(ModelChunk::Done(StopReason::EndTurn))))
            .collect();

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
    use futures::StreamExt;

    fn make_request(msg: &str) -> ModelRequest {
        ModelRequest {
            model: "echo".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text(msg.into()),
            }],
            tools: vec![],
            max_tokens: None,
            temperature: None,
            system: None,
            stream: true,
        }
    }

    #[tokio::test]
    async fn echo_returns_user_message() {
        let provider = EchoProvider::new();
        let req = make_request("hello world");
        let mut stream = provider.invoke(&req).await.unwrap();

        let mut text = String::new();
        while let Some(chunk) = stream.next().await {
            if let Ok(ModelChunk::TextDelta(delta)) = chunk {
                text.push_str(&delta);
            }
        }
        assert!(text.contains("hello world"));
    }

    #[tokio::test]
    async fn echo_stream_ends_with_done() {
        let provider = EchoProvider::new();
        let req = make_request("test");
        let stream = provider.invoke(&req).await.unwrap();

        let chunks: Vec<Result<ModelChunk>> = stream.collect().await;
        let last = chunks.last().unwrap().as_ref().unwrap();
        assert!(matches!(last, ModelChunk::Done(StopReason::EndTurn)));
    }

    #[tokio::test]
    async fn echo_emits_usage() {
        let provider = EchoProvider::new();
        let req = make_request("test message");
        let stream = provider.invoke(&req).await.unwrap();

        let chunks: Vec<Result<ModelChunk>> = stream.collect().await;
        let has_usage = chunks
            .iter()
            .any(|c| matches!(c, Ok(ModelChunk::Usage(u)) if u.output_tokens > 0));
        assert!(has_usage);
    }

    #[tokio::test]
    async fn echo_is_always_available() {
        let provider = EchoProvider::new();
        assert!(provider.is_available().await);
    }

    #[test]
    fn echo_name_and_models() {
        let provider = EchoProvider::new();
        assert_eq!(provider.name(), "echo");
        assert_eq!(provider.supported_models().len(), 1);
        assert_eq!(provider.supported_models()[0].id, "echo");
    }

    #[test]
    fn echo_cost_is_zero() {
        let provider = EchoProvider::new();
        let req = make_request("x");
        let cost = provider.estimate_cost(&req);
        assert_eq!(cost.estimated_cost_usd, 0.0);
    }
}
