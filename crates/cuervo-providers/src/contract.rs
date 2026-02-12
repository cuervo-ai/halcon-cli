//! Provider contract tests.
//!
//! These tests validate that every `ModelProvider` implementation satisfies
//! the expected invariants. Add new providers here as they are implemented.

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cuervo_core::traits::ModelProvider;
    use cuervo_core::types::{
        ChatMessage, MessageContent, ModelChunk, ModelRequest, Role, StopReason,
    };
    use futures::StreamExt;

    use crate::{
        AnthropicProvider, DeepSeekProvider, EchoProvider, GeminiProvider, OllamaProvider,
        OpenAIProvider,
    };

    /// Build a list of all providers for contract testing.
    /// Providers use dummy keys (no real network calls in contract tests).
    fn all_providers() -> Vec<Arc<dyn ModelProvider>> {
        let http = cuervo_core::types::HttpConfig::default();
        vec![
            Arc::new(EchoProvider::new()),
            Arc::new(AnthropicProvider::new("sk-ant-api03-contract-test".into())),
            Arc::new(OllamaProvider::new(None, http.clone())),
            Arc::new(OpenAIProvider::new(
                "sk-contract-test".into(),
                None,
                http.clone(),
            )),
            Arc::new(DeepSeekProvider::new(
                "sk-contract-test".into(),
                None,
                http.clone(),
            )),
            Arc::new(GeminiProvider::new("contract-test".into(), None, http)),
        ]
    }

    fn simple_request(msg: &str) -> ModelRequest {
        ModelRequest {
            model: "echo".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text(msg.into()),
            }],
            tools: vec![],
            max_tokens: Some(64),
            temperature: Some(0.0),
            system: None,
            stream: true,
        }
    }

    // =======================================================
    // Contract: name() must return a non-empty, lowercase string
    // =======================================================

    #[test]
    fn contract_name_non_empty() {
        for p in all_providers() {
            let name = p.name();
            assert!(!name.is_empty(), "provider name must not be empty");
            assert_eq!(
                name,
                name.to_lowercase(),
                "provider name '{}' must be lowercase",
                name
            );
            assert!(
                name.chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-'),
                "provider name '{}' contains invalid characters",
                name
            );
        }
    }

    // =======================================================
    // Contract: supported_models() must return at least one model
    // =======================================================

    #[test]
    fn contract_supported_models_non_empty() {
        for p in all_providers() {
            let models = p.supported_models();
            assert!(
                !models.is_empty(),
                "provider '{}' must support at least one model",
                p.name()
            );

            for model in models {
                assert!(!model.id.is_empty(), "model ID must not be empty");
                assert!(!model.name.is_empty(), "model name must not be empty");
                assert_eq!(
                    model.provider,
                    p.name(),
                    "model provider field must match provider name"
                );
                assert!(
                    model.context_window > 0,
                    "model context window must be positive"
                );
                assert!(
                    model.max_output_tokens > 0,
                    "model max_output_tokens must be positive"
                );
            }
        }
    }

    // =======================================================
    // Contract: estimate_cost() returns non-negative values
    // =======================================================

    #[test]
    fn contract_estimate_cost_non_negative() {
        for p in all_providers() {
            let req = simple_request("estimate cost test");
            let cost = p.estimate_cost(&req);
            assert!(
                cost.estimated_cost_usd >= 0.0,
                "provider '{}': estimated cost must be non-negative",
                p.name()
            );
        }
    }

    // =======================================================
    // Contract: is_available() returns a bool without panic
    // =======================================================

    #[tokio::test]
    async fn contract_is_available_does_not_panic() {
        for p in all_providers() {
            let _ = p.is_available().await;
        }
    }

    // =======================================================
    // Contract: Echo provider stream produces text + usage + done
    // =======================================================

    #[tokio::test]
    async fn contract_echo_stream_completeness() {
        let provider = EchoProvider::new();
        let req = simple_request("contract test");
        let stream = provider.invoke(&req).await.unwrap();
        let chunks: Vec<_> = stream.collect().await;

        let has_text = chunks
            .iter()
            .any(|c| matches!(c, Ok(ModelChunk::TextDelta(_))));
        let has_usage = chunks.iter().any(|c| matches!(c, Ok(ModelChunk::Usage(_))));
        let has_done = chunks
            .iter()
            .any(|c| matches!(c, Ok(ModelChunk::Done(StopReason::EndTurn))));

        assert!(has_text, "stream must contain at least one TextDelta");
        assert!(has_usage, "stream must contain at least one Usage");
        assert!(has_done, "stream must end with Done(EndTurn)");
    }

    // =======================================================
    // Contract: provider names are unique across all providers
    // =======================================================

    #[test]
    fn contract_provider_names_unique() {
        let providers = all_providers();
        let names: Vec<&str> = providers.iter().map(|p| p.name()).collect();
        let mut deduped = names.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(
            names.len(),
            deduped.len(),
            "provider names must be unique: {:?}",
            names
        );
    }

    // =======================================================
    // Contract: model IDs are unique within each provider
    // =======================================================

    #[test]
    fn contract_model_ids_unique_per_provider() {
        for p in all_providers() {
            let models = p.supported_models();
            let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
            let mut deduped = ids.clone();
            deduped.sort();
            deduped.dedup();
            assert_eq!(
                ids.len(),
                deduped.len(),
                "provider '{}': model IDs must be unique: {:?}",
                p.name(),
                ids
            );
        }
    }

    // =======================================================
    // Contract: total provider count is exactly 6
    // =======================================================

    #[test]
    fn contract_total_provider_count() {
        assert_eq!(all_providers().len(), 6, "expected 6 providers");
    }

    // =======================================================
    // Contract: supports_reasoning field is consistent
    // =======================================================

    #[test]
    fn contract_supports_reasoning_field_present() {
        for p in all_providers() {
            for model in p.supported_models() {
                // Field exists and is a bool (compile-time check via pattern match).
                let _: bool = model.supports_reasoning;
            }
        }
    }

    // =======================================================
    // Contract: reasoning models exist in the expected providers
    // =======================================================

    #[test]
    fn contract_reasoning_models_exist() {
        let providers = all_providers();
        let all_models: Vec<_> = providers
            .iter()
            .flat_map(|p| p.supported_models())
            .collect();
        let reasoning_models: Vec<&str> = all_models
            .iter()
            .filter(|m| m.supports_reasoning)
            .map(|m| m.id.as_str())
            .collect();
        // At minimum o1, o3-mini, deepseek-reasoner, gemini-2.5-pro should support reasoning
        assert!(
            reasoning_models.len() >= 4,
            "expected at least 4 reasoning models, got: {:?}",
            reasoning_models
        );
    }

    // =======================================================
    // Contract: total model count across all providers
    // =======================================================

    #[test]
    fn contract_total_model_count() {
        let providers = all_providers();
        let total: usize = providers.iter().map(|p| p.supported_models().len()).sum();
        // echo(1) + anthropic(3) + ollama(3) + openai(4) + deepseek(3) + gemini(2) = 16
        assert_eq!(total, 16, "expected 16 total models, got {total}");
    }

    // =======================================================
    // Contract: cost_per_input_token is non-negative for all models
    // =======================================================

    #[test]
    fn contract_model_costs_non_negative() {
        for p in all_providers() {
            for model in p.supported_models() {
                assert!(
                    model.cost_per_input_token >= 0.0,
                    "{}/{}: input cost must be non-negative",
                    p.name(),
                    model.id
                );
                assert!(
                    model.cost_per_output_token >= 0.0,
                    "{}/{}: output cost must be non-negative",
                    p.name(),
                    model.id
                );
            }
        }
    }

    // =======================================================
    // Contract: models with supports_tools=false don't claim reasoning=false
    // (i.e., no-tool models have some other capability)
    // =======================================================

    #[test]
    fn contract_no_tools_models_have_reasoning() {
        for p in all_providers() {
            for model in p.supported_models() {
                if !model.supports_tools && p.name() != "echo" && p.name() != "ollama" {
                    assert!(
                        model.supports_reasoning,
                        "{}/{}: no-tools model should support reasoning",
                        p.name(),
                        model.id
                    );
                }
            }
        }
    }

    // =======================================================
    // Contract: Debug impl does not leak API keys
    // =======================================================

    #[test]
    fn contract_debug_does_not_leak_keys() {
        let http = cuervo_core::types::HttpConfig::default();
        let openai = OpenAIProvider::new("sk-secret-key-123".into(), None, http.clone());
        let debug_str = format!("{:?}", openai);
        assert!(!debug_str.contains("sk-secret"), "OpenAI Debug leaks key: {debug_str}");

        let deepseek = DeepSeekProvider::new("sk-deepseek-secret".into(), None, http.clone());
        let debug_str = format!("{:?}", deepseek);
        assert!(!debug_str.contains("sk-deepseek"), "DeepSeek Debug leaks key: {debug_str}");

        let gemini = GeminiProvider::new("AIzaSySecret123".into(), None, http);
        let debug_str = format!("{:?}", gemini);
        assert!(!debug_str.contains("AIzaSy"), "Gemini Debug leaks key: {debug_str}");
    }
}
