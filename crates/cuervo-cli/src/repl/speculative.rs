//! Speculative model invocation: race multiple providers, first-wins.
//!
//! Two modes:
//! - **Failover** (default): try primary, then fallbacks sequentially (delegates to ModelRouter).
//! - **Speculative**: race N providers concurrently, use first valid stream, cancel rest.

use std::sync::Arc;
use std::time::Instant;

use cuervo_core::error::CuervoError;
use cuervo_core::traits::ModelProvider;
use cuervo_core::types::{ModelChunk, ModelRequest, RoutingConfig};
use futures::stream::BoxStream;

use super::router::ModelRouter;

/// Result of a successful invocation through the speculative invoker.
pub struct InvocationResult {
    pub stream: BoxStream<'static, Result<ModelChunk, CuervoError>>,
    pub provider_name: String,
    pub model: String,
    pub is_fallback: bool,
    pub selection_latency_ms: u64,
}

impl std::fmt::Debug for InvocationResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InvocationResult")
            .field("provider_name", &self.provider_name)
            .field("model", &self.model)
            .field("is_fallback", &self.is_fallback)
            .field("selection_latency_ms", &self.selection_latency_ms)
            .finish_non_exhaustive()
    }
}

/// Speculative invoker: wraps ModelRouter with optional concurrent racing.
pub struct SpeculativeInvoker {
    router: ModelRouter,
    mode: String,
}

impl SpeculativeInvoker {
    pub fn new(config: &RoutingConfig) -> Self {
        Self {
            router: ModelRouter::new(config),
            mode: config.mode.clone(),
        }
    }

    /// Invoke a model request with the configured routing mode.
    pub async fn invoke(
        &self,
        primary_provider: &Arc<dyn ModelProvider>,
        request: &ModelRequest,
        fallback_providers: &[(String, Arc<dyn ModelProvider>)],
    ) -> Result<InvocationResult, CuervoError> {
        let start = Instant::now();

        if self.mode == "speculative" && !fallback_providers.is_empty() {
            self.invoke_speculative(primary_provider, request, fallback_providers, start)
                .await
        } else {
            self.invoke_failover(primary_provider, request, fallback_providers, start)
                .await
        }
    }

    /// Failover mode: delegate to ModelRouter (sequential fallback).
    async fn invoke_failover(
        &self,
        primary_provider: &Arc<dyn ModelProvider>,
        request: &ModelRequest,
        fallback_providers: &[(String, Arc<dyn ModelProvider>)],
        start: Instant,
    ) -> Result<InvocationResult, CuervoError> {
        let (stream, route) = self
            .router
            .invoke(primary_provider, request, fallback_providers)
            .await?;

        Ok(InvocationResult {
            stream,
            provider_name: route.provider.name().to_string(),
            model: route.model,
            is_fallback: route.is_fallback,
            selection_latency_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// Speculative mode: race primary + fallback providers concurrently.
    /// First successful stream wins; others are dropped (cancelling their SSE connections).
    async fn invoke_speculative(
        &self,
        primary_provider: &Arc<dyn ModelProvider>,
        request: &ModelRequest,
        fallback_providers: &[(String, Arc<dyn ModelProvider>)],
        start: Instant,
    ) -> Result<InvocationResult, CuervoError> {
        // Build futures for all candidates.
        let primary_name = primary_provider.name().to_string();
        let primary_model = request.model.clone();

        // Race: primary vs all fallbacks using futures::future::select_ok.
        type InvokeOutput = (
            BoxStream<'static, Result<ModelChunk, CuervoError>>,
            String,
            String,
            bool,
        );
        type InvokeFut =
            std::pin::Pin<Box<dyn std::future::Future<Output = Result<InvokeOutput, CuervoError>> + Send>>;

        let mut futs: Vec<InvokeFut> = Vec::new();

        // Primary provider.
        let p = Arc::clone(primary_provider);
        let req = request.clone();
        let pname = primary_name.clone();
        let pmodel = primary_model.clone();
        futs.push(Box::pin(async move {
            let stream = p.invoke(&req).await?;
            Ok((stream, pname, pmodel, false))
        }));

        // Fallback providers — adapt model to each provider's supported models.
        for (name, provider) in fallback_providers {
            let p = Arc::clone(provider);
            let fname = name.clone();
            // Adapt model: use the request model if supported, otherwise use provider's first model.
            let fb_model = if provider.supported_models().iter().any(|m| m.id == request.model) {
                request.model.clone()
            } else {
                provider
                    .supported_models()
                    .first()
                    .map(|m| m.id.clone())
                    .unwrap_or_else(|| request.model.clone())
            };
            let fb_req = ModelRequest {
                model: fb_model.clone(),
                ..request.clone()
            };
            futs.push(Box::pin(async move {
                let stream = p.invoke(&fb_req).await?;
                Ok((stream, fname, fb_model, true))
            }));
        }

        // select_ok: returns the first Ok result, cancels the rest.
        match futures::future::select_ok(futs).await {
            Ok(((stream, provider_name, model, is_fallback), _remaining)) => {
                let selection_latency_ms = start.elapsed().as_millis() as u64;
                tracing::info!(
                    provider = %provider_name,
                    model = %model,
                    is_fallback,
                    selection_latency_ms,
                    "Speculative invocation winner"
                );
                Ok(InvocationResult {
                    stream,
                    provider_name,
                    model,
                    is_fallback,
                    selection_latency_ms,
                })
            }
            Err(last_error) => {
                tracing::warn!(
                    error = %last_error,
                    "All speculative invocations failed"
                );
                Err(last_error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cuervo_core::types::{ChatMessage, MessageContent, Role};

    fn make_request() -> ModelRequest {
        ModelRequest {
            model: "echo".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text("test".into()),
            }],
            tools: vec![],
            max_tokens: Some(256),
            temperature: Some(0.0),
            system: None,
            stream: true,
        }
    }

    #[tokio::test]
    async fn failover_mode_primary_succeeds() {
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let config = RoutingConfig::default();
        let invoker = SpeculativeInvoker::new(&config);

        let result = invoker
            .invoke(&provider, &make_request(), &[])
            .await
            .unwrap();

        assert!(!result.is_fallback);
        assert_eq!(result.model, "echo");
        assert_eq!(result.provider_name, "echo");
    }

    #[tokio::test]
    async fn failover_mode_is_default() {
        let config = RoutingConfig::default();
        assert_eq!(config.mode, "failover");
    }

    #[tokio::test]
    async fn speculative_mode_with_single_provider() {
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let config = RoutingConfig {
            mode: "speculative".into(),
            ..RoutingConfig::default()
        };
        let invoker = SpeculativeInvoker::new(&config);

        // No fallback providers — falls through to failover.
        let result = invoker
            .invoke(&provider, &make_request(), &[])
            .await
            .unwrap();

        assert!(!result.is_fallback);
    }

    #[tokio::test]
    async fn speculative_mode_races_providers() {
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let echo2: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let config = RoutingConfig {
            mode: "speculative".into(),
            ..RoutingConfig::default()
        };
        let invoker = SpeculativeInvoker::new(&config);

        let fallbacks = vec![("echo2".into(), echo2)];
        let result = invoker
            .invoke(&provider, &make_request(), &fallbacks)
            .await
            .unwrap();

        // One of them should win.
        assert!(result.provider_name == "echo" || result.provider_name == "echo2");
    }

    #[tokio::test]
    async fn invocation_result_has_selection_latency() {
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let config = RoutingConfig::default();
        let invoker = SpeculativeInvoker::new(&config);

        let result = invoker
            .invoke(&provider, &make_request(), &[])
            .await
            .unwrap();

        // Latency should be small for EchoProvider.
        assert!(result.selection_latency_ms < 5000);
    }

    #[test]
    fn routing_config_serde_backward_compat() {
        // Old config without mode field should default to "failover".
        let toml_str = r#"
strategy = "balanced"
fallback_models = []
max_retries = 1
"#;
        let config: RoutingConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.mode, "failover");
        assert!(config.speculation_providers.is_empty());
    }

    #[test]
    fn routing_config_with_speculative() {
        let toml_str = r#"
strategy = "balanced"
mode = "speculative"
fallback_models = ["echo"]
max_retries = 0
speculation_providers = ["anthropic", "ollama"]
"#;
        let config: RoutingConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.mode, "speculative");
        assert_eq!(config.speculation_providers.len(), 2);
    }
}
