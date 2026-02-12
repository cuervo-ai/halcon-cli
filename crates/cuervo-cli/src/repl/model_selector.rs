//! Context-aware model selector.
//!
//! Selects the optimal model per request based on task complexity,
//! historical metrics, tool requirements, and budget constraints.

use cuervo_core::types::{
    ChatMessage, MessageContent, ModelInfo, ModelRequest, ModelSelectionConfig,
};
use cuervo_providers::ProviderRegistry;
use tracing::debug;

/// Detected task complexity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskComplexity {
    Simple,
    Standard,
    Complex,
}

/// Model selection result.
#[derive(Debug, Clone)]
pub struct ModelSelection {
    pub model_id: String,
    pub provider_name: String,
    pub reason: String,
}

/// Selects the optimal model based on context.
///
/// IMPORTANT: By default, the selector is **provider-scoped** — it only considers
/// models from the active provider. This prevents silent cross-provider switching
/// (e.g., selecting gemini-2.0-flash when the user configured deepseek).
pub struct ModelSelector {
    config: ModelSelectionConfig,
    available_models: Vec<ModelInfo>,
    /// The provider to scope selection to. When set, only models from this provider
    /// are considered. When None, all providers are eligible (legacy behavior).
    scoped_provider: Option<String>,
}

impl ModelSelector {
    /// Create a new model selector scoped to a specific provider.
    ///
    /// Only models from `active_provider` will be considered for selection,
    /// preventing silent cross-provider switching.
    pub fn new(config: ModelSelectionConfig, registry: &ProviderRegistry) -> Self {
        let mut available_models = Vec::new();
        for provider_name in registry.list() {
            if let Some(provider) = registry.get(provider_name) {
                available_models.extend_from_slice(provider.supported_models());
            }
        }
        Self {
            config,
            available_models,
            scoped_provider: None,
        }
    }

    /// Scope selection to a specific provider.
    ///
    /// When set, `select_model()` only considers models from this provider.
    /// This is the recommended default to prevent cross-provider model switching.
    pub fn with_provider_scope(mut self, provider_name: &str) -> Self {
        self.scoped_provider = Some(provider_name.to_string());
        self
    }

    /// Select the best model for a given request context.
    pub fn select_model(
        &self,
        request: &ModelRequest,
        session_spend_usd: f64,
    ) -> Option<ModelSelection> {
        if !self.config.enabled {
            return None;
        }

        // Budget gate: if spending >= 90% of cap, force cheapest
        if self.config.budget_cap_usd > 0.0
            && session_spend_usd >= self.config.budget_cap_usd * 0.9
        {
            return self.cheapest_model(request);
        }

        let complexity = Self::detect_complexity(request, self.config.complexity_token_threshold);
        debug!(?complexity, "Detected task complexity");

        match complexity {
            TaskComplexity::Simple => {
                if let Some(ref model_id) = self.config.simple_model {
                    return self.find_model(model_id, "config override (simple)");
                }
                self.select_by_strategy(request, "cheap")
            }
            TaskComplexity::Standard => self.select_by_strategy(request, "balanced"),
            TaskComplexity::Complex => {
                if let Some(ref model_id) = self.config.complex_model {
                    return self.find_model(model_id, "config override (complex)");
                }
                self.select_by_strategy(request, "fast")
            }
        }
    }

    /// Detect task complexity from request content using weighted multi-signal scoring.
    ///
    /// Scores 0-100 across 6 dimensions:
    /// - Token volume (0-30): based on estimated token count vs threshold
    /// - Conversation depth (0-20): message count and multi-turn patterns
    /// - Tool interaction (0-15): presence of tools or tool results
    /// - Semantic keywords (0-25): reasoning/analysis/design keywords
    /// - Multi-step indicators (0-10): "then", "after that", "step", numbered lists
    /// - Question count (0-10): number of questions in last user message
    ///
    /// Thresholds: ≥25=Complex, ≥10=Standard, <10=Simple.
    pub fn detect_complexity(request: &ModelRequest, threshold: u32) -> TaskComplexity {
        let score = Self::complexity_score(request, threshold);
        debug!(score, "Complexity score");

        if score >= 25 {
            TaskComplexity::Complex
        } else if score >= 10 {
            TaskComplexity::Standard
        } else {
            TaskComplexity::Simple
        }
    }

    /// Compute the raw complexity score (0-100) for a request.
    pub fn complexity_score(request: &ModelRequest, threshold: u32) -> u32 {
        let estimated_tokens = estimate_message_tokens(&request.messages);
        let last_user_text = request
            .messages
            .iter()
            .rev()
            .find(|m| m.role == cuervo_core::types::Role::User)
            .and_then(|m| m.content.as_text())
            .unwrap_or("");
        let lower = last_user_text.to_lowercase();

        // 1. Token volume (0-30): linear scale up to threshold
        let token_score = if threshold > 0 {
            std::cmp::min(30, (estimated_tokens * 30 / threshold).min(30))
        } else {
            0
        };

        // 2. Conversation depth (0-20)
        let msg_count = request.messages.len();
        let depth_score = if msg_count > 10 {
            20
        } else if msg_count > 6 {
            15
        } else if msg_count > 3 {
            8
        } else {
            0
        };

        // 3. Tool interaction (0-15)
        let has_tools = !request.tools.is_empty();
        let has_tool_results = request.messages.iter().any(|m| {
            matches!(&m.content, MessageContent::Blocks(blocks) if blocks.iter().any(|b|
                matches!(b, cuervo_core::types::ContentBlock::ToolResult { .. })
            ))
        });
        let tool_score = if has_tool_results {
            15
        } else if has_tools {
            10
        } else {
            0
        };

        // 4. Semantic keywords (0-25)
        let keyword_patterns = [
            "explain", "analyze", "reason", "think step", "complex",
            "architecture", "design", "implement", "refactor", "debug",
            "optimize", "investigate", "compare",
        ];
        let keyword_hits: u32 = keyword_patterns
            .iter()
            .filter(|kw| lower.contains(**kw))
            .count() as u32;
        let keyword_score = std::cmp::min(25, keyword_hits * 8);

        // 5. Multi-step indicators (0-10)
        let multistep_patterns = [
            "then ", "after that", "step ", "first ", "next ",
            "finally ", "1.", "2.", "3.",
        ];
        let multistep_hits: u32 = multistep_patterns
            .iter()
            .filter(|p| lower.contains(**p))
            .count() as u32;
        let multistep_score = std::cmp::min(10, multistep_hits * 5);

        // 6. Question count (0-10)
        let question_count = last_user_text.matches('?').count() as u32;
        let question_score = std::cmp::min(10, question_count * 5);

        token_score + depth_score + tool_score + keyword_score + multistep_score + question_score
    }

    /// Get models eligible for selection, respecting provider scope.
    fn eligible_models(&self) -> impl Iterator<Item = &ModelInfo> {
        let scope = self.scoped_provider.clone();
        self.available_models
            .iter()
            .filter(move |m| match &scope {
                Some(p) => m.provider == *p,
                None => true,
            })
    }

    fn cheapest_model(&self, request: &ModelRequest) -> Option<ModelSelection> {
        let needs_tools = !request.tools.is_empty();
        self.eligible_models()
            .filter(|m| !needs_tools || m.supports_tools)
            .min_by(|a, b| {
                a.cost_per_input_token
                    .partial_cmp(&b.cost_per_input_token)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|m| ModelSelection {
                model_id: m.id.clone(),
                provider_name: m.provider.clone(),
                reason: "budget limit (≥90% spent)".into(),
            })
    }

    fn select_by_strategy(
        &self,
        request: &ModelRequest,
        strategy: &str,
    ) -> Option<ModelSelection> {
        let needs_tools = !request.tools.is_empty();

        let mut candidates: Vec<&ModelInfo> = self
            .eligible_models()
            .filter(|m| !needs_tools || m.supports_tools)
            .collect();

        if candidates.is_empty() {
            return None;
        }

        match strategy {
            "cheap" => {
                candidates.sort_by(|a, b| {
                    a.cost_per_input_token
                        .partial_cmp(&b.cost_per_input_token)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
            "fast" => {
                // Prefer models with larger context and higher capability (proxy: higher cost)
                candidates.sort_by(|a, b| {
                    b.context_window
                        .cmp(&a.context_window)
                        .then_with(|| {
                            b.cost_per_input_token
                                .partial_cmp(&a.cost_per_input_token)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                });
            }
            _ => {
                // balanced: mid-range cost, prefer tools support
                candidates.sort_by(|a, b| {
                    let score_a = balance_score(a);
                    let score_b = balance_score(b);
                    score_b
                        .partial_cmp(&score_a)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
        }

        candidates.first().map(|m| ModelSelection {
            model_id: m.id.clone(),
            provider_name: m.provider.clone(),
            reason: format!("strategy={strategy}"),
        })
    }

    fn find_model(&self, model_id: &str, reason: &str) -> Option<ModelSelection> {
        self.eligible_models()
            .find(|m| m.id == model_id)
            .map(|m| ModelSelection {
                model_id: m.id.clone(),
                provider_name: m.provider.clone(),
                reason: reason.into(),
            })
    }
}

/// Balanced score: moderate cost, wide context, tool support bonus.
fn balance_score(model: &ModelInfo) -> f64 {
    let cost_efficiency = 1.0 / (1.0 + model.cost_per_input_token * 1_000_000.0);
    let context_score = (model.context_window as f64).log2() / 20.0;
    let tool_bonus = if model.supports_tools { 0.2 } else { 0.0 };
    cost_efficiency * 0.4 + context_score * 0.4 + tool_bonus
}

/// Rough token estimate: ~4 chars per token.
fn estimate_message_tokens(messages: &[ChatMessage]) -> u32 {
    let chars: usize = messages
        .iter()
        .map(|m| match &m.content {
            MessageContent::Text(t) => t.len(),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .map(|b| match b {
                    cuervo_core::types::ContentBlock::Text { text } => text.len(),
                    cuervo_core::types::ContentBlock::ToolResult { content, .. } => content.len(),
                    cuervo_core::types::ContentBlock::ToolUse { input, .. } => {
                        serde_json::to_string(input).map(|s| s.len()).unwrap_or(0)
                    }
                })
                .sum(),
        })
        .sum();
    (chars / 4) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use cuervo_core::types::{
        ChatMessage, ContentBlock, MessageContent, ModelRequest, Role, ToolDefinition,
    };

    fn make_request(msg: &str) -> ModelRequest {
        ModelRequest {
            model: "test".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text(msg.into()),
            }],
            tools: vec![],
            max_tokens: Some(1024),
            temperature: None,
            system: None,
            stream: true,
        }
    }

    fn make_long_request() -> ModelRequest {
        let long_text = "x".repeat(10000); // ~2500 tokens
        make_request(&long_text)
    }

    fn make_request_with_tools(msg: &str) -> ModelRequest {
        ModelRequest {
            model: "test".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text(msg.into()),
            }],
            tools: vec![ToolDefinition {
                name: "bash".into(),
                description: "Run".into(),
                input_schema: serde_json::json!({}),
            }],
            max_tokens: Some(1024),
            temperature: None,
            system: None,
            stream: true,
        }
    }

    fn make_multi_round_request() -> ModelRequest {
        ModelRequest {
            model: "test".into(),
            messages: vec![
                ChatMessage {
                    role: Role::User,
                    content: MessageContent::Text("hello".into()),
                },
                ChatMessage {
                    role: Role::Assistant,
                    content: MessageContent::Text("hi".into()),
                },
                ChatMessage {
                    role: Role::User,
                    content: MessageContent::Text("do something".into()),
                },
                ChatMessage {
                    role: Role::Assistant,
                    content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                        id: "t1".into(),
                        name: "bash".into(),
                        input: serde_json::json!({}),
                    }]),
                },
                ChatMessage {
                    role: Role::User,
                    content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                        tool_use_id: "t1".into(),
                        content: "result".into(),
                        is_error: false,
                    }]),
                },
                ChatMessage {
                    role: Role::Assistant,
                    content: MessageContent::Text("done".into()),
                },
                ChatMessage {
                    role: Role::User,
                    content: MessageContent::Text("now do more".into()),
                },
            ],
            tools: vec![],
            max_tokens: Some(1024),
            temperature: None,
            system: None,
            stream: true,
        }
    }

    fn test_models() -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "cheap-model".into(),
                name: "Cheap".into(),
                provider: "test".into(),
                context_window: 32_000,
                max_output_tokens: 4096,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                supports_reasoning: false,
                cost_per_input_token: 0.1 / 1_000_000.0,
                cost_per_output_token: 0.2 / 1_000_000.0,
            },
            ModelInfo {
                id: "mid-model".into(),
                name: "Mid".into(),
                provider: "test".into(),
                context_window: 128_000,
                max_output_tokens: 8192,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                supports_reasoning: false,
                cost_per_input_token: 2.5 / 1_000_000.0,
                cost_per_output_token: 10.0 / 1_000_000.0,
            },
            ModelInfo {
                id: "expensive-model".into(),
                name: "Expensive".into(),
                provider: "test".into(),
                context_window: 200_000,
                max_output_tokens: 32_000,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                supports_reasoning: true,
                cost_per_input_token: 15.0 / 1_000_000.0,
                cost_per_output_token: 75.0 / 1_000_000.0,
            },
            ModelInfo {
                id: "no-tools-model".into(),
                name: "NoTools".into(),
                provider: "test".into(),
                context_window: 64_000,
                max_output_tokens: 8192,
                supports_streaming: true,
                supports_tools: false,
                supports_vision: false,
                supports_reasoning: true,
                cost_per_input_token: 0.5 / 1_000_000.0,
                cost_per_output_token: 2.0 / 1_000_000.0,
            },
        ]
    }

    fn make_selector(config: ModelSelectionConfig) -> ModelSelector {
        ModelSelector {
            config,
            available_models: test_models(),
            scoped_provider: None,
        }
    }

    fn enabled_config() -> ModelSelectionConfig {
        ModelSelectionConfig {
            enabled: true,
            ..Default::default()
        }
    }

    // --- detect_complexity tests ---

    #[test]
    fn detect_simple_short_message() {
        let req = make_request("hello");
        assert_eq!(
            ModelSelector::detect_complexity(&req, 2000),
            TaskComplexity::Simple
        );
    }

    #[test]
    fn detect_complex_long_message() {
        let req = make_long_request();
        assert_eq!(
            ModelSelector::detect_complexity(&req, 2000),
            TaskComplexity::Complex
        );
    }

    #[test]
    fn detect_standard_with_tools() {
        let req = make_request_with_tools("run ls");
        assert_eq!(
            ModelSelector::detect_complexity(&req, 2000),
            TaskComplexity::Standard
        );
    }

    #[test]
    fn detect_complex_multi_round() {
        let req = make_multi_round_request();
        assert_eq!(
            ModelSelector::detect_complexity(&req, 2000),
            TaskComplexity::Complex
        );
    }

    // --- select_model tests ---

    #[test]
    fn disabled_returns_none() {
        let selector = make_selector(ModelSelectionConfig::default());
        let req = make_request("hello");
        assert!(selector.select_model(&req, 0.0).is_none());
    }

    #[test]
    fn budget_exceeded_forces_cheapest() {
        let config = ModelSelectionConfig {
            enabled: true,
            budget_cap_usd: 1.0,
            ..Default::default()
        };
        let selector = make_selector(config);
        let req = make_request("hello");
        let selection = selector.select_model(&req, 0.95).unwrap();
        assert_eq!(selection.model_id, "cheap-model");
        assert!(selection.reason.contains("budget"));
    }

    #[test]
    fn simple_selects_cheap() {
        let selector = make_selector(enabled_config());
        let req = make_request("hi");
        let selection = selector.select_model(&req, 0.0).unwrap();
        assert_eq!(selection.model_id, "cheap-model");
    }

    #[test]
    fn complex_selects_capable() {
        let selector = make_selector(enabled_config());
        let req = make_long_request();
        let selection = selector.select_model(&req, 0.0).unwrap();
        // For "fast" strategy, prefers largest context window
        assert_eq!(selection.model_id, "expensive-model");
    }

    #[test]
    fn tools_filter_excludes_no_tools_model() {
        let selector = make_selector(enabled_config());
        let req = make_request_with_tools("run something");
        let selection = selector.select_model(&req, 0.0).unwrap();
        // no-tools-model should be filtered out
        assert_ne!(selection.model_id, "no-tools-model");
    }

    #[test]
    fn simple_model_override() {
        let config = ModelSelectionConfig {
            enabled: true,
            simple_model: Some("mid-model".into()),
            ..Default::default()
        };
        let selector = make_selector(config);
        let req = make_request("hi");
        let selection = selector.select_model(&req, 0.0).unwrap();
        assert_eq!(selection.model_id, "mid-model");
        assert!(selection.reason.contains("override"));
    }

    #[test]
    fn complex_model_override() {
        let config = ModelSelectionConfig {
            enabled: true,
            complex_model: Some("cheap-model".into()),
            ..Default::default()
        };
        let selector = make_selector(config);
        let req = make_long_request();
        let selection = selector.select_model(&req, 0.0).unwrap();
        assert_eq!(selection.model_id, "cheap-model");
        assert!(selection.reason.contains("override"));
    }

    // --- config tests ---

    #[test]
    fn config_defaults() {
        let config = ModelSelectionConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.budget_cap_usd, 0.0);
        assert_eq!(config.complexity_token_threshold, 2000);
        assert!(config.simple_model.is_none());
        assert!(config.complex_model.is_none());
    }

    #[test]
    fn config_serde_roundtrip() {
        let config = ModelSelectionConfig {
            enabled: true,
            budget_cap_usd: 10.0,
            complexity_token_threshold: 3000,
            simple_model: Some("gpt-4o-mini".into()),
            complex_model: Some("claude-opus-4-6".into()),
        };
        let json = serde_json::to_string(&config).unwrap();
        let roundtrip: ModelSelectionConfig = serde_json::from_str(&json).unwrap();
        assert!(roundtrip.enabled);
        assert_eq!(roundtrip.budget_cap_usd, 10.0);
        assert_eq!(roundtrip.simple_model.as_deref(), Some("gpt-4o-mini"));
    }

    #[test]
    fn reasoning_keywords_trigger_complex() {
        let req = make_request("Please analyze this architecture and explain the design");
        assert_eq!(
            ModelSelector::detect_complexity(&req, 2000),
            TaskComplexity::Complex
        );
    }

    // --- Phase 18: Enhanced complexity detection tests ---

    #[test]
    fn multi_question_complex() {
        let req = make_request(
            "What is the architecture? How does routing work? Can you explain the fallback?"
        );
        let score = ModelSelector::complexity_score(&req, 2000);
        assert!(score >= 25, "3 questions + keywords should be complex, got {score}");
    }

    #[test]
    fn implement_and_test_complex() {
        let req = make_request(
            "Implement a new parser module. Then add unit tests. After that, refactor the existing code to use it."
        );
        let score = ModelSelector::complexity_score(&req, 2000);
        assert!(score >= 25, "implement+refactor+multi-step should be complex, got {score}");
    }

    #[test]
    fn greeting_simple() {
        let req = make_request("hello");
        let score = ModelSelector::complexity_score(&req, 2000);
        assert!(score < 10, "greeting should be simple, got {score}");
    }

    #[test]
    fn code_review_standard() {
        let req = make_request_with_tools("Review this function for bugs");
        let score = ModelSelector::complexity_score(&req, 2000);
        assert!(
            score >= 10,
            "code review with tools should be at least standard, got {score}"
        );
    }

    #[test]
    fn score_accumulation() {
        // Test that scores accumulate from multiple dimensions.
        let req_simple = make_request("hi");
        let score_simple = ModelSelector::complexity_score(&req_simple, 2000);

        let req_complex = make_request(
            "Explain and analyze the architecture. Design a new system. Then implement it step by step. What are the tradeoffs?"
        );
        let score_complex = ModelSelector::complexity_score(&req_complex, 2000);

        assert!(
            score_complex > score_simple,
            "complex request ({score_complex}) should score higher than simple ({score_simple})"
        );
        assert!(score_complex >= 25, "multi-signal request should be complex, got {score_complex}");
    }

    // --- Provider scoping tests ---

    #[test]
    fn scoped_selector_only_picks_from_scoped_provider() {
        // Add models from two providers.
        let models = vec![
            ModelInfo {
                id: "deepseek-chat".into(),
                name: "DeepSeek Chat".into(),
                provider: "deepseek".into(),
                context_window: 64_000,
                max_output_tokens: 4096,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                supports_reasoning: false,
                cost_per_input_token: 0.1 / 1_000_000.0,
                cost_per_output_token: 0.2 / 1_000_000.0,
            },
            ModelInfo {
                id: "gemini-flash".into(),
                name: "Gemini Flash".into(),
                provider: "gemini".into(),
                context_window: 128_000,
                max_output_tokens: 8192,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                supports_reasoning: false,
                cost_per_input_token: 0.05 / 1_000_000.0,
                cost_per_output_token: 0.1 / 1_000_000.0,
            },
        ];
        let selector = ModelSelector {
            config: enabled_config(),
            available_models: models,
            scoped_provider: Some("deepseek".into()),
        };
        let req = make_request("hi");
        let selection = selector.select_model(&req, 0.0).unwrap();
        // Must select from deepseek, not gemini (even though gemini is cheaper).
        assert_eq!(selection.provider_name, "deepseek");
        assert_eq!(selection.model_id, "deepseek-chat");
    }

    #[test]
    fn unscoped_selector_can_pick_cross_provider() {
        let models = vec![
            ModelInfo {
                id: "expensive".into(),
                name: "Expensive".into(),
                provider: "provider-a".into(),
                context_window: 32_000,
                max_output_tokens: 4096,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                supports_reasoning: false,
                cost_per_input_token: 10.0 / 1_000_000.0,
                cost_per_output_token: 20.0 / 1_000_000.0,
            },
            ModelInfo {
                id: "cheap".into(),
                name: "Cheap".into(),
                provider: "provider-b".into(),
                context_window: 32_000,
                max_output_tokens: 4096,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                supports_reasoning: false,
                cost_per_input_token: 0.01 / 1_000_000.0,
                cost_per_output_token: 0.02 / 1_000_000.0,
            },
        ];
        let selector = ModelSelector {
            config: enabled_config(),
            available_models: models,
            scoped_provider: None, // no scope — legacy behavior
        };
        let req = make_request("hi");
        let selection = selector.select_model(&req, 0.0).unwrap();
        // Without scoping, cheapest model from any provider wins.
        assert_eq!(selection.model_id, "cheap");
        assert_eq!(selection.provider_name, "provider-b");
    }

    #[test]
    fn scoped_to_empty_provider_returns_none() {
        let selector = ModelSelector {
            config: enabled_config(),
            available_models: test_models(), // all models are from "test" provider
            scoped_provider: Some("nonexistent".into()),
        };
        let req = make_request("hi");
        // No models match scope → returns None.
        assert!(selector.select_model(&req, 0.0).is_none());
    }
}
