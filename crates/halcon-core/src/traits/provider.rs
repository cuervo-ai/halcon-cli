use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::error::Result;
use crate::types::{ModelChunk, ModelInfo, ModelRequest, ProviderHandle, TokenCost, TokenizerHint, ToolFormat};

/// Trait for model providers (Anthropic, Ollama, OpenAI, etc.).
///
/// Each provider adapts a specific LLM API into the unified Halcon interface.
/// Implementations must be Send + Sync for use across async tasks.
#[async_trait]
pub trait ModelProvider: Send + Sync {
    /// Unique identifier for this provider (e.g., "anthropic", "ollama").
    fn name(&self) -> &str;

    /// List of models available through this provider.
    fn supported_models(&self) -> &[ModelInfo];

    /// Send a request and receive a stream of response chunks.
    async fn invoke(
        &self,
        request: &ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelChunk>>>;

    /// Check if the provider is currently reachable.
    async fn is_available(&self) -> bool;

    /// Estimate the cost of a request before sending it.
    fn estimate_cost(&self, request: &ModelRequest) -> TokenCost;

    /// Validate that a model name is supported by this provider.
    ///
    /// Returns `Ok(())` if the model is valid, or `Err(ModelNotFound)` if not.
    /// Default implementation checks against `supported_models()`.
    fn validate_model(&self, model: &str) -> crate::error::Result<()> {
        if self.supported_models().iter().any(|m| m.id == model) {
            Ok(())
        } else {
            Err(crate::error::HalconError::ModelNotFound {
                provider: self.name().to_string(),
                model: model.to_string(),
            })
        }
    }

    /// Get the context window size for a model.
    ///
    /// Returns `None` if the model is not found in this provider.
    fn model_context_window(&self, model: &str) -> Option<u32> {
        self.supported_models()
            .iter()
            .find(|m| m.id == model)
            .map(|m| m.context_window)
    }

    /// Return a typed `ProviderHandle` for this provider.
    ///
    /// Default implementation wraps `self.name()` in a `ProviderHandle`.
    /// Phase 2+: routing code can compare handles instead of strings.
    /// All existing implementations inherit this default automatically —
    /// no existing code needs to change.
    fn handle(&self) -> ProviderHandle {
        ProviderHandle::new(self.name())
    }

    /// The wire format this provider uses for tool definitions.
    ///
    /// Default returns `Unknown`. First-party providers override this.
    fn tool_format(&self) -> ToolFormat {
        ToolFormat::Unknown
    }

    /// Hint about the tokenizer family used by this provider's models.
    ///
    /// Used for token estimation when a real tokenizer is unavailable.
    /// Default returns `Unknown` (~4.0 chars/token conservative estimate).
    fn tokenizer_hint(&self) -> TokenizerHint {
        TokenizerHint::Unknown
    }

    /// Maximum output tokens for a specific model.
    ///
    /// Looks up the model in `supported_models()` and returns its `max_output_tokens`.
    /// Returns `None` if the model is not found.
    fn model_max_output_tokens(&self, model: &str) -> Option<u32> {
        self.supported_models()
            .iter()
            .find(|m| m.id == model)
            .map(|m| m.max_output_tokens)
    }
}
