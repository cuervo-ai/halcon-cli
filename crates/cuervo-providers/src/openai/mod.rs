//! OpenAI provider — thin wrapper over `OpenAICompatibleProvider`.
//!
//! Models: gpt-4o, gpt-4o-mini, o1, o3-mini.
//! Default base URL: `https://api.openai.com/v1`
//! Env var: `OPENAI_API_KEY`

use async_trait::async_trait;
use futures::stream::BoxStream;

use cuervo_core::error::Result;
use cuervo_core::traits::ModelProvider;
use cuervo_core::types::{HttpConfig, ModelChunk, ModelInfo, ModelRequest, TokenCost};

use crate::openai_compat::OpenAICompatibleProvider;

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// OpenAI provider for GPT-4o and o-series models.
pub struct OpenAIProvider {
    inner: OpenAICompatibleProvider,
}

impl std::fmt::Debug for OpenAIProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAIProvider")
            .field("inner", &self.inner)
            .finish()
    }
}

impl OpenAIProvider {
    /// Create a new OpenAI provider.
    pub fn new(api_key: String, base_url: Option<String>, http_config: HttpConfig) -> Self {
        let url = base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        Self {
            inner: OpenAICompatibleProvider::new(
                "openai".into(),
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
                id: "gpt-4o".into(),
                name: "GPT-4o".into(),
                provider: "openai".into(),
                context_window: 128_000,
                max_output_tokens: 16384,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                supports_reasoning: false,
                cost_per_input_token: 2.50 / 1_000_000.0,
                cost_per_output_token: 10.0 / 1_000_000.0,
            },
            ModelInfo {
                id: "gpt-4o-mini".into(),
                name: "GPT-4o Mini".into(),
                provider: "openai".into(),
                context_window: 128_000,
                max_output_tokens: 16384,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                supports_reasoning: false,
                cost_per_input_token: 0.15 / 1_000_000.0,
                cost_per_output_token: 0.60 / 1_000_000.0,
            },
            ModelInfo {
                id: "o1".into(),
                name: "o1".into(),
                provider: "openai".into(),
                context_window: 200_000,
                max_output_tokens: 100_000,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                supports_reasoning: true,
                cost_per_input_token: 15.0 / 1_000_000.0,
                cost_per_output_token: 60.0 / 1_000_000.0,
            },
            ModelInfo {
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
            },
        ]
    }
}

#[async_trait]
impl ModelProvider for OpenAIProvider {
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
            model: "gpt-4o".into(),
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
    fn name_is_openai() {
        let provider = OpenAIProvider::new("sk-test".into(), None, HttpConfig::default());
        assert_eq!(provider.name(), "openai");
    }

    #[test]
    fn supported_models_count() {
        let provider = OpenAIProvider::new("sk-test".into(), None, HttpConfig::default());
        let models = provider.supported_models();
        assert_eq!(models.len(), 4);
        for m in models {
            assert_eq!(m.provider, "openai");
        }
    }

    #[tokio::test]
    async fn is_available_with_key() {
        let provider = OpenAIProvider::new("sk-test".into(), None, HttpConfig::default());
        assert!(provider.is_available().await);
    }

    #[test]
    fn estimate_cost_positive() {
        let provider = OpenAIProvider::new("sk-test".into(), None, HttpConfig::default());
        let req = make_request("test message for cost");
        let cost = provider.estimate_cost(&req);
        assert!(cost.estimated_input_tokens > 0);
        assert!(cost.estimated_cost_usd > 0.0);
    }

    #[test]
    fn debug_redacts_key() {
        let provider = OpenAIProvider::new("sk-secret-key".into(), None, HttpConfig::default());
        let debug = format!("{provider:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("sk-secret-key"));
    }

    #[test]
    fn custom_base_url() {
        let provider = OpenAIProvider::new(
            "sk-test".into(),
            Some("https://custom.openai.com/v1".into()),
            HttpConfig::default(),
        );
        let debug = format!("{provider:?}");
        assert!(debug.contains("custom.openai.com"));
    }
}
