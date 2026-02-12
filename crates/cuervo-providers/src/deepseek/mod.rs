//! DeepSeek provider — thin wrapper over `OpenAICompatibleProvider`.
//!
//! Models: deepseek-chat, deepseek-coder, deepseek-reasoner.
//! Default base URL: `https://api.deepseek.com`
//! Env var: `DEEPSEEK_API_KEY`

use async_trait::async_trait;
use futures::stream::BoxStream;

use cuervo_core::error::Result;
use cuervo_core::traits::ModelProvider;
use cuervo_core::types::{HttpConfig, ModelChunk, ModelInfo, ModelRequest, TokenCost};

use crate::openai_compat::OpenAICompatibleProvider;

const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";

/// DeepSeek provider for coding and reasoning models.
pub struct DeepSeekProvider {
    inner: OpenAICompatibleProvider,
}

impl std::fmt::Debug for DeepSeekProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeepSeekProvider")
            .field("inner", &self.inner)
            .finish()
    }
}

impl DeepSeekProvider {
    /// Create a new DeepSeek provider.
    pub fn new(api_key: String, base_url: Option<String>, http_config: HttpConfig) -> Self {
        let url = base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        Self {
            inner: OpenAICompatibleProvider::new(
                "deepseek".into(),
                api_key,
                url,
                Self::default_models(),
                http_config,
            ),
        }
    }

    fn default_models() -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "deepseek-chat".into(),
                name: "DeepSeek Chat".into(),
                provider: "deepseek".into(),
                context_window: 64_000,
                max_output_tokens: 8192,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                supports_reasoning: false,
                cost_per_input_token: 0.14 / 1_000_000.0,
                cost_per_output_token: 0.28 / 1_000_000.0,
            },
            ModelInfo {
                id: "deepseek-coder".into(),
                name: "DeepSeek Coder".into(),
                provider: "deepseek".into(),
                context_window: 64_000,
                max_output_tokens: 8192,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                supports_reasoning: false,
                cost_per_input_token: 0.14 / 1_000_000.0,
                cost_per_output_token: 0.28 / 1_000_000.0,
            },
            ModelInfo {
                id: "deepseek-reasoner".into(),
                name: "DeepSeek Reasoner".into(),
                provider: "deepseek".into(),
                context_window: 64_000,
                max_output_tokens: 8192,
                supports_streaming: true,
                supports_tools: false, // Reasoner does not support tools
                supports_vision: false,
                supports_reasoning: true,
                cost_per_input_token: 0.55 / 1_000_000.0,
                cost_per_output_token: 2.19 / 1_000_000.0,
            },
        ]
    }
}

#[async_trait]
impl ModelProvider for DeepSeekProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn supported_models(&self) -> &[ModelInfo] {
        self.inner.supported_models()
    }

    async fn invoke(
        &self,
        request: &ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelChunk>>> {
        self.inner.invoke(request).await
    }

    async fn is_available(&self) -> bool {
        self.inner.is_available().await
    }

    fn estimate_cost(&self, request: &ModelRequest) -> TokenCost {
        self.inner.estimate_cost(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cuervo_core::types::{ChatMessage, MessageContent, Role};

    fn make_request(msg: &str) -> ModelRequest {
        ModelRequest {
            model: "deepseek-chat".into(),
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
    fn name_is_deepseek() {
        let provider = DeepSeekProvider::new("sk-test".into(), None, HttpConfig::default());
        assert_eq!(provider.name(), "deepseek");
    }

    #[test]
    fn supported_models_count() {
        let provider = DeepSeekProvider::new("sk-test".into(), None, HttpConfig::default());
        let models = provider.supported_models();
        assert_eq!(models.len(), 3);
        for m in models {
            assert_eq!(m.provider, "deepseek");
        }
    }

    #[tokio::test]
    async fn is_available_with_key() {
        let provider = DeepSeekProvider::new("sk-test".into(), None, HttpConfig::default());
        assert!(provider.is_available().await);
    }

    #[test]
    fn estimate_cost_positive() {
        let provider = DeepSeekProvider::new("sk-test".into(), None, HttpConfig::default());
        let req = make_request("test message for cost");
        let cost = provider.estimate_cost(&req);
        assert!(cost.estimated_input_tokens > 0);
        assert!(cost.estimated_cost_usd > 0.0);
    }

    #[test]
    fn debug_redacts_key() {
        let provider =
            DeepSeekProvider::new("sk-secret-deepseek".into(), None, HttpConfig::default());
        let debug = format!("{provider:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("sk-secret-deepseek"));
    }

    #[test]
    fn deepseek_reasoner_no_tools() {
        let provider = DeepSeekProvider::new("sk-test".into(), None, HttpConfig::default());
        let models = provider.supported_models();
        let reasoner = models.iter().find(|m| m.id == "deepseek-reasoner").unwrap();
        assert!(!reasoner.supports_tools);
    }
}
