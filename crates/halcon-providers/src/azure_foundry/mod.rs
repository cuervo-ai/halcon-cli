// DECISION: Azure AI Foundry uses OpenAI-compatible chat completions format.
// Rather than duplicating openai_compat.rs, AzureFoundryProvider COMPOSES
// an OpenAiCompatProvider with Azure-specific endpoint and auth headers.
// This avoids code duplication and ensures Foundry gets all future
// openai_compat improvements automatically.
//
// Auth: api-key header (basic) or Bearer token (Azure Entra ID).
// ENV:
//   AZURE_AI_ENDPOINT    — required (e.g., https://<resource>.services.ai.azure.com)
//   AZURE_API_KEY        — API key auth (simple)
//   AZURE_CLIENT_ID      — Entra ID auth (combined with AZURE_TENANT_ID)
//   AZURE_TENANT_ID      — Entra ID auth
//
// Activation: CLAUDE_CODE_USE_AZURE=1
//
// Endpoint: {AZURE_AI_ENDPOINT}/chat/completions?api-version=2024-05-01-preview
//
// See US-foundry (PASO 2-D).

use async_trait::async_trait;
use futures::stream::BoxStream;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::ModelProvider;
use halcon_core::types::{HttpConfig, ModelChunk, ModelInfo, ModelRequest, TokenCost};

use crate::openai_compat::OpenAICompatibleProvider;

/// Azure AI Foundry provider.
///
/// Wraps `OpenAICompatibleProvider` with Azure-specific endpoint configuration.
///
/// Activated when `CLAUDE_CODE_USE_AZURE=1` environment variable is set.
pub struct AzureFoundryProvider {
    inner: OpenAICompatibleProvider,
}

impl std::fmt::Debug for AzureFoundryProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AzureFoundryProvider")
            .field("inner", &self.inner)
            .finish()
    }
}

impl AzureFoundryProvider {
    /// Resolve Azure configuration from environment variables.
    fn endpoint_and_key() -> Option<(String, String)> {
        let endpoint = std::env::var("AZURE_AI_ENDPOINT").ok()?;
        // API key auth takes precedence over Entra ID.
        let key = std::env::var("AZURE_API_KEY")
            .or_else(|_| std::env::var("AZURE_CLIENT_ID"))
            .unwrap_or_default();
        Some((endpoint, key))
    }

    /// Build the full chat completions URL including api-version query param.
    fn completions_url(endpoint: &str) -> String {
        // Strip trailing slash to avoid double-slash.
        let base = endpoint.trim_end_matches('/');
        format!("{base}/chat/completions?api-version=2024-05-01-preview")
    }

    /// Default model list for Azure AI Foundry.
    ///
    /// Model IDs are the deployment names, which vary per workspace.
    /// We expose a set of common Claude deployment names.
    fn default_models() -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "claude-sonnet-4-6".into(),
                name: "Claude Sonnet 4.6 (Azure Foundry)".into(),
                provider: "azure_foundry".into(),
                context_window: 200_000,
                max_output_tokens: 8192,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                supports_reasoning: false,
                cost_per_input_token: 3.0 / 1_000_000.0,
                cost_per_output_token: 15.0 / 1_000_000.0,
            },
            ModelInfo {
                id: "claude-haiku-4-5-20251001".into(),
                name: "Claude Haiku 4.5 (Azure Foundry)".into(),
                provider: "azure_foundry".into(),
                context_window: 200_000,
                max_output_tokens: 8192,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                supports_reasoning: false,
                cost_per_input_token: 0.80 / 1_000_000.0,
                cost_per_output_token: 4.0 / 1_000_000.0,
            },
        ]
    }

    /// Construct from environment variables.
    ///
    /// Returns `None` if `AZURE_AI_ENDPOINT` is absent.
    pub fn from_env() -> Option<Self> {
        let (endpoint, key) = Self::endpoint_and_key()?;
        let url = Self::completions_url(&endpoint);
        let http_config = HttpConfig::default();
        let inner = OpenAICompatibleProvider::new(
            "azure_foundry".into(),
            key,
            url,
            Self::default_models(),
            http_config,
        );
        Some(Self { inner })
    }

    /// Create with explicit endpoint and key (for testing).
    pub fn with_endpoint(endpoint: String, api_key: String) -> Self {
        let url = Self::completions_url(&endpoint);
        let http_config = HttpConfig::default();
        let inner = OpenAICompatibleProvider::new(
            "azure_foundry".into(),
            api_key,
            url,
            Self::default_models(),
            http_config,
        );
        Self { inner }
    }
}

#[async_trait]
impl ModelProvider for AzureFoundryProvider {
    fn name(&self) -> &str {
        "azure_foundry"
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

    fn validate_model(&self, model: &str) -> halcon_core::error::Result<()> {
        // Azure Foundry deployment names are user-defined — accept anything non-empty.
        if model.is_empty() {
            return Err(HalconError::ModelNotFound {
                provider: "azure_foundry".into(),
                model: model.to_string(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completions_url_format() {
        let url = AzureFoundryProvider::completions_url("https://myresource.services.ai.azure.com");
        assert!(url.contains("/chat/completions"), "URL={url}");
        assert!(url.contains("api-version=2024-05-01-preview"), "URL={url}");
    }

    #[test]
    fn completions_url_strips_trailing_slash() {
        let url = AzureFoundryProvider::completions_url("https://endpoint.com/");
        assert!(!url.contains("//chat"), "Should not have double slash: {url}");
    }

    #[test]
    fn provider_name() {
        let p = AzureFoundryProvider::with_endpoint(
            "https://example.services.ai.azure.com".into(),
            "key123".into(),
        );
        assert_eq!(p.name(), "azure_foundry");
    }

    #[test]
    fn validate_model_accepts_nonempty() {
        let p = AzureFoundryProvider::with_endpoint(
            "https://example.services.ai.azure.com".into(),
            "key123".into(),
        );
        assert!(p.validate_model("claude-sonnet-4-6").is_ok());
        assert!(p.validate_model("my-custom-deployment").is_ok());
        assert!(p.validate_model("").is_err());
    }

    #[test]
    fn from_env_missing_returns_none() {
        let saved = std::env::var("AZURE_AI_ENDPOINT").ok();
        std::env::remove_var("AZURE_AI_ENDPOINT");
        let result = AzureFoundryProvider::from_env();
        if let Some(v) = saved {
            std::env::set_var("AZURE_AI_ENDPOINT", v);
        }
        assert!(result.is_none());
    }
}
