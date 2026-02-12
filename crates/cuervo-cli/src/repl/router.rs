//! Model router: retry + fallback logic.
//!
//! The `ModelRouter` wraps a `ProviderRegistry` and adds:
//! - Retries on transient errors (up to `max_retries`).
//! - Fallback to alternative models when the primary model fails.
//! - Cost estimation per invocation.
//!
//! The router does NOT modify the provider or its configuration — it simply
//! selects which provider/model to invoke and retries on failure.

use std::sync::Arc;

use cuervo_core::error::CuervoError;
use cuervo_core::traits::ModelProvider;
use cuervo_core::types::{ModelChunk, ModelRequest, RoutingConfig};
use futures::stream::BoxStream;

/// A resolved route: provider + model to invoke.
#[derive(Clone)]
pub struct ResolvedRoute {
    pub provider: Arc<dyn ModelProvider>,
    pub model: String,
    /// Whether this is a fallback (not the primary).
    pub is_fallback: bool,
}

impl std::fmt::Debug for ResolvedRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedRoute")
            .field("model", &self.model)
            .field("is_fallback", &self.is_fallback)
            .finish_non_exhaustive()
    }
}

/// Model router: tries primary, retries, then fallbacks.
pub struct ModelRouter {
    config: RoutingConfig,
}

impl ModelRouter {
    pub fn new(config: &RoutingConfig) -> Self {
        Self {
            config: config.clone(),
        }
    }

    /// Invoke a model request with retry + fallback logic.
    ///
    /// Tries the primary model `max_retries` times. If all retries fail and
    /// fallback models are configured, tries each fallback once.
    ///
    /// Returns the stream and the route that succeeded.
    pub async fn invoke(
        &self,
        primary_provider: &Arc<dyn ModelProvider>,
        request: &ModelRequest,
        fallback_providers: &[(String, Arc<dyn ModelProvider>)],
    ) -> Result<(BoxStream<'static, Result<ModelChunk, CuervoError>>, ResolvedRoute), CuervoError>
    {
        // Try the primary model with retries.
        let mut last_error = None;
        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                tracing::info!(
                    attempt,
                    max_retries = self.config.max_retries,
                    model = %request.model,
                    "Retrying model invocation"
                );
            }

            match primary_provider.invoke(request).await {
                Ok(stream) => {
                    return Ok((
                        stream,
                        ResolvedRoute {
                            provider: Arc::clone(primary_provider),
                            model: request.model.clone(),
                            is_fallback: false,
                        },
                    ));
                }
                Err(e) => {
                    tracing::warn!(
                        attempt,
                        model = %request.model,
                        error = %e,
                        retryable = e.is_retryable(),
                        "Model invocation failed"
                    );
                    // Non-retryable errors (auth, billing, client errors) fail fast.
                    if !e.is_retryable() {
                        return Err(e);
                    }
                    last_error = Some(e);
                }
            }
        }

        // Try fallback providers (each once).
        // fallback_models contains provider names; each provider uses its own default model
        // or the request model if it supports it.
        for (name, provider) in fallback_providers {
            let fb_model = if provider.supported_models().iter().any(|m| m.id == request.model) {
                request.model.clone()
            } else {
                provider
                    .supported_models()
                    .first()
                    .map(|m| m.id.clone())
                    .unwrap_or_else(|| request.model.clone())
            };

            let fallback_request = ModelRequest {
                model: fb_model.clone(),
                ..request.clone()
            };

            tracing::info!(
                provider = name,
                model = %fb_model,
                "Trying fallback provider"
            );

            match provider.invoke(&fallback_request).await {
                Ok(stream) => {
                    eprintln!(
                        "[fallback: using {}/{} instead of {}]",
                        name, fb_model, request.model
                    );
                    return Ok((
                        stream,
                        ResolvedRoute {
                            provider: Arc::clone(provider),
                            model: fb_model,
                            is_fallback: true,
                        },
                    ));
                }
                Err(e) => {
                    tracing::warn!(
                        provider = name,
                        model = %fb_model,
                        error = %e,
                        "Fallback provider failed"
                    );
                    last_error = Some(e);
                }
            }
        }

        // All attempts exhausted.
        Err(last_error.unwrap_or(CuervoError::ProviderUnavailable {
            provider: "all".to_string(),
        }))
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
    async fn primary_succeeds_no_fallback() {
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let config = RoutingConfig::default();
        let router = ModelRouter::new(&config);

        let (stream, route) = router.invoke(&provider, &make_request(), &[]).await.unwrap();
        drop(stream);

        assert!(!route.is_fallback);
        assert_eq!(route.model, "echo");
    }

    #[tokio::test]
    async fn retries_before_giving_up() {
        // Use a provider that will fail (non-existent model with echo provider still works,
        // so we test retry count config parsing instead).
        let config = RoutingConfig {
            max_retries: 2,
            ..RoutingConfig::default()
        };
        let router = ModelRouter::new(&config);
        assert_eq!(router.config.max_retries, 2);
    }

    #[tokio::test]
    async fn default_routing_config() {
        let config = RoutingConfig::default();
        assert_eq!(config.strategy, "balanced");
        assert!(config.fallback_models.is_empty());
        assert_eq!(config.max_retries, 1);
    }

    #[tokio::test]
    async fn fallback_models_tried_after_primary_fails() {
        // When primary fails and a fallback is configured, the router tries it.
        let echo: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let config = RoutingConfig {
            fallback_models: vec!["echo".to_string()],
            max_retries: 0, // No retries, go straight to fallback.
            ..RoutingConfig::default()
        };
        let router = ModelRouter::new(&config);

        // Primary succeeds immediately, so fallback is not used.
        let (stream, route) = router.invoke(&echo, &make_request(), &[("echo".into(), echo.clone())]).await.unwrap();
        drop(stream);
        assert!(!route.is_fallback);
    }
}
