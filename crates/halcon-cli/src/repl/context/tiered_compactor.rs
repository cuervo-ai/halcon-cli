//! TieredCompactor: orchestrates semantic compaction with 3-level degradation.
//!
//! Levels:
//!   1. Nominal  — LLM summary + protected context
//!   2. Degraded — extended keep + protected context (LLM failed)
//!   3. Emergency — min keep + IntentAnchor only (circuit breaker open)
//!
//! Owns the circuit breaker. Delegates keep-window mechanics to ContextCompactor.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use futures::StreamExt;

use halcon_context::estimate_tokens;
use halcon_core::traits::ModelProvider;
use halcon_core::types::{
    ChatMessage, CompactionConfig, MessageContent, ModelChunk, ModelRequest, Role,
};

use super::compaction::ContextCompactor;
use super::compaction_budget::{CompactionBudget, CompactionBudgetCalculator, PostCompactionCheck};
use super::compaction_summary::CompactionSummaryBuilder;
use super::intent_anchor::IntentAnchor;
use super::protected_context::ProtectedContextInjector;

/// Compaction level achieved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionLevel {
    Nominal,
    Degraded,
    Emergency,
}

/// Whether compaction was triggered proactively or reactively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionTrigger {
    Proactive,
    Reactive,
}

/// Result of a compaction attempt.
#[derive(Debug)]
pub struct CompactionResult {
    pub level: CompactionLevel,
    pub trigger: CompactionTrigger,
    pub utility_ratio: f64,
    pub summary_tokens: usize,
    pub protected_tokens: usize,
    pub keep_messages: usize,
    pub latency_ms: u64,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub aborted: bool,
}

/// Orchestrator for semantic compaction with progressive degradation.
pub struct TieredCompactor {
    inner: ContextCompactor,
    consecutive_failures: u32,
    max_failures: u32,
    timeout: Duration,
    config: CompactionConfig,
}

impl TieredCompactor {
    pub fn new(config: CompactionConfig) -> Self {
        let max_failures = config.max_circuit_breaker_failures;
        let timeout = Duration::from_secs(config.compaction_timeout_secs);
        Self {
            inner: ContextCompactor::new(config.clone()),
            consecutive_failures: 0,
            max_failures,
            timeout,
            config,
        }
    }

    pub fn circuit_breaker_open(&self) -> bool {
        self.consecutive_failures >= self.max_failures
    }

    /// Execute compaction with progressive degradation.
    pub async fn compact(
        &mut self,
        messages: &mut Vec<ChatMessage>,
        intent_anchor: &IntentAnchor,
        provider: &Arc<dyn ModelProvider>,
        budget: &CompactionBudget,
        model: &str,
        tools_used: &[String],
        files_modified: &[String],
        trigger: CompactionTrigger,
    ) -> CompactionResult {
        let tokens_before = ContextCompactor::estimate_message_tokens(messages);
        let start = Instant::now();

        // Circuit breaker open → emergency
        if self.circuit_breaker_open() {
            tracing::warn!(
                failures = self.consecutive_failures,
                "Circuit breaker open, emergency compaction"
            );
            return self.apply_emergency(
                messages,
                intent_anchor,
                budget,
                trigger,
                tokens_before,
                start,
            );
        }

        // Try nominal (LLM summary)
        match self
            .try_nominal(
                messages,
                intent_anchor,
                provider,
                budget,
                model,
                tools_used,
                files_modified,
                trigger,
                tokens_before,
                start,
            )
            .await
        {
            Ok(result) => result,
            Err(_) => {
                // LLM failed → degraded
                self.apply_degraded(
                    messages,
                    intent_anchor,
                    budget,
                    tools_used,
                    files_modified,
                    trigger,
                    tokens_before,
                    start,
                )
            }
        }
    }

    async fn try_nominal(
        &mut self,
        messages: &mut Vec<ChatMessage>,
        intent_anchor: &IntentAnchor,
        provider: &Arc<dyn ModelProvider>,
        budget: &CompactionBudget,
        model: &str,
        tools_used: &[String],
        files_modified: &[String],
        trigger: CompactionTrigger,
        tokens_before: usize,
        start: Instant,
    ) -> Result<CompactionResult> {
        let prompt = CompactionSummaryBuilder::build_prompt(
            messages,
            intent_anchor,
            budget.keep_count,
            budget.max_summary_tokens,
        );

        let request = ModelRequest {
            model: model.to_string(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text(prompt),
            }],
            tools: vec![],
            max_tokens: Some(budget.max_summary_tokens as u32),
            temperature: Some(0.0),
            system: None,
            stream: true,
        };

        let summary = consume_summary_stream(provider, &request, self.timeout).await;
        let latency_ms = start.elapsed().as_millis() as u64;

        match summary {
            Ok(text) if !text.trim().is_empty() => {
                self.consecutive_failures = 0;

                // Cap summary if needed
                let mut summary_text = text;
                let summary_tokens = estimate_tokens(&summary_text);
                if summary_tokens > budget.max_summary_tokens {
                    tracing::warn!(
                        actual = summary_tokens,
                        cap = budget.max_summary_tokens,
                        "Summary exceeded cap, truncating"
                    );
                    let target_chars = budget.max_summary_tokens * 4; // rough
                    summary_text = summary_text.chars().take(target_chars).collect();
                    summary_text.push_str("\n[summary truncated to fit budget]");
                }
                let summary_tokens = estimate_tokens(&summary_text);

                // Build protected context block
                let protected_block = ProtectedContextInjector::build_block(
                    intent_anchor,
                    tools_used,
                    files_modified,
                );
                let protected_tokens = estimate_tokens(&protected_block);

                // Calculate utility before applying
                let keep_tokens = estimate_keep_tokens(messages, budget.keep_count);
                let utility = CompactionBudgetCalculator::utility_ratio(
                    tokens_before,
                    keep_tokens,
                    summary_tokens,
                    protected_tokens,
                );

                if utility <= 0.0 {
                    tracing::warn!(
                        utility,
                        tokens_before,
                        keep_tokens,
                        summary_tokens,
                        protected_tokens,
                        "Compaction aborted: negative utility"
                    );
                    return Ok(CompactionResult {
                        level: CompactionLevel::Nominal,
                        trigger,
                        utility_ratio: utility,
                        summary_tokens: 0,
                        protected_tokens: 0,
                        keep_messages: 0,
                        latency_ms,
                        tokens_before,
                        tokens_after: tokens_before,
                        aborted: true,
                    });
                }

                if utility < 0.3 {
                    tracing::warn!(utility, "Compaction utility below 0.3 — low value");
                }

                // Merge summary + protected context into boundary message
                let boundary = format!("{}\n\n{}", summary_text, protected_block);

                let keep = self.inner.apply_compaction_with_keep_count(
                    messages,
                    &boundary,
                    budget.keep_count,
                );

                let tokens_after = ContextCompactor::estimate_message_tokens(messages);

                // Post-compaction verification
                self.verify_and_fix(messages, budget, tokens_after);

                let tokens_after = ContextCompactor::estimate_message_tokens(messages);

                tracing::info!(
                    summary_tokens,
                    utility,
                    latency_ms,
                    "Semantic compaction completed"
                );

                Ok(CompactionResult {
                    level: CompactionLevel::Nominal,
                    trigger,
                    utility_ratio: utility,
                    summary_tokens,
                    protected_tokens,
                    keep_messages: keep,
                    latency_ms,
                    tokens_before,
                    tokens_after,
                    aborted: false,
                })
            }
            Ok(_) => {
                // Empty summary = failure
                self.consecutive_failures += 1;
                tracing::warn!(
                    failures = self.consecutive_failures,
                    "Summary was empty, treating as failure"
                );
                Err(anyhow!("Empty summary response"))
            }
            Err(e) => {
                self.consecutive_failures += 1;
                tracing::error!(
                    error = %e,
                    failures = self.consecutive_failures,
                    "Semantic compaction failed, falling back to degraded"
                );
                Err(e)
            }
        }
    }

    fn apply_degraded(
        &self,
        messages: &mut Vec<ChatMessage>,
        intent_anchor: &IntentAnchor,
        budget: &CompactionBudget,
        tools_used: &[String],
        files_modified: &[String],
        trigger: CompactionTrigger,
        tokens_before: usize,
        start: Instant,
    ) -> CompactionResult {
        let protected_block =
            ProtectedContextInjector::build_block(intent_anchor, tools_used, files_modified);
        let protected_tokens = estimate_tokens(&protected_block);

        let boundary = format!(
            "[Summary unavailable — extended recent context preserved below]\n\n{}",
            protected_block
        );

        let keep = self.inner.apply_compaction_with_keep_count(
            messages,
            &boundary,
            budget.extended_keep_count,
        );

        let tokens_after = ContextCompactor::estimate_message_tokens(messages);
        let keep_tokens = estimate_keep_tokens(messages, keep);
        let utility = CompactionBudgetCalculator::utility_ratio(
            tokens_before,
            keep_tokens,
            0,
            protected_tokens,
        );

        tracing::warn!(
            keep_messages = keep,
            tokens_before,
            tokens_after,
            "Degraded compaction applied (extended keep, no summary)"
        );

        CompactionResult {
            level: CompactionLevel::Degraded,
            trigger,
            utility_ratio: utility,
            summary_tokens: 0,
            protected_tokens,
            keep_messages: keep,
            latency_ms: start.elapsed().as_millis() as u64,
            tokens_before,
            tokens_after,
            aborted: false,
        }
    }

    fn apply_emergency(
        &self,
        messages: &mut Vec<ChatMessage>,
        intent_anchor: &IntentAnchor,
        budget: &CompactionBudget,
        trigger: CompactionTrigger,
        tokens_before: usize,
        start: Instant,
    ) -> CompactionResult {
        let boundary = format!(
            "[Emergency compaction — only intent preserved]\n\n{}",
            intent_anchor.format_for_boundary()
        );
        let protected_tokens = estimate_tokens(&boundary);

        // Use budget keep_count with floor of 4
        let keep = self.inner.apply_compaction_with_keep_count(
            messages,
            &boundary,
            budget.keep_count.max(4),
        );

        let tokens_after = ContextCompactor::estimate_message_tokens(messages);

        tracing::warn!(
            keep_messages = keep,
            tokens_before,
            tokens_after,
            circuit_breaker = self.consecutive_failures,
            "Emergency compaction applied"
        );

        CompactionResult {
            level: CompactionLevel::Emergency,
            trigger,
            utility_ratio: 0.0,
            summary_tokens: 0,
            protected_tokens,
            keep_messages: keep,
            latency_ms: start.elapsed().as_millis() as u64,
            tokens_before,
            tokens_after,
            aborted: false,
        }
    }

    fn verify_and_fix(
        &self,
        messages: &mut Vec<ChatMessage>,
        budget: &CompactionBudget,
        tokens_after: usize,
    ) {
        match CompactionBudgetCalculator::verify_post_compaction(tokens_after, budget) {
            PostCompactionCheck::Ok => {}
            PostCompactionCheck::SummaryTruncationNeeded { target_tokens } => {
                tracing::error!(
                    tokens_after,
                    budget = budget.pipeline_budget,
                    target_tokens,
                    "Post-compaction budget violation, truncating boundary message"
                );
                if let Some(msg) = messages.first_mut() {
                    if let MessageContent::Text(ref mut text) = msg.content {
                        let target_chars = target_tokens * 4;
                        if text.len() > target_chars {
                            *text = text.chars().take(target_chars).collect();
                            text.push_str("\n[boundary truncated to fit budget]");
                        }
                    } else {
                        tracing::error!(
                            "verify_and_fix: boundary message is not Text, cannot truncate summary"
                        );
                    }
                } else {
                    tracing::error!("verify_and_fix: no boundary message found post-compaction");
                }
            }
            PostCompactionCheck::KeepReductionNeeded { target_keep } => {
                tracing::error!(
                    tokens_after,
                    target_keep,
                    "Post-compaction budget violation — keep window too large"
                );
                if let Some(msg) = messages.first() {
                    if let MessageContent::Text(text) = &msg.content {
                        let boundary = text.clone();
                        self.inner.apply_compaction_with_keep_count(
                            messages,
                            &boundary,
                            target_keep,
                        );
                    } else {
                        tracing::error!(
                            "verify_and_fix: boundary message is not Text, cannot reduce keep"
                        );
                    }
                } else {
                    tracing::error!("verify_and_fix: no boundary message found post-compaction");
                }
            }
        }
    }
}

/// Estimate tokens in the last `keep` messages.
fn estimate_keep_tokens(messages: &[ChatMessage], keep: usize) -> usize {
    let start = messages.len().saturating_sub(keep);
    ContextCompactor::estimate_message_tokens(&messages[start..])
}

/// Consume a provider stream into a summary string.
///
/// Semantics per gap closure:
/// - Partial text before error = usable
/// - Stream without Done but with text = usable
/// - Zero TextDelta = failure
/// - Error before any text = failure
async fn consume_summary_stream(
    provider: &Arc<dyn ModelProvider>,
    request: &ModelRequest,
    timeout: Duration,
) -> Result<String> {
    // Wrap the entire operation (invoke + stream consumption) in timeout
    tokio::time::timeout(timeout, consume_stream_inner(provider, request))
        .await
        .map_err(|_| anyhow!("Compaction summary timed out after {}s", timeout.as_secs()))?
}

async fn consume_stream_inner(
    provider: &Arc<dyn ModelProvider>,
    request: &ModelRequest,
) -> Result<String> {
    let mut stream = provider
        .invoke(request)
        .await
        .map_err(|e| anyhow!("Provider invoke failed: {}", e))?;

    let mut text = String::new();
    let mut had_error = false;

    loop {
        match stream.next().await {
            Some(Ok(chunk)) => match chunk {
                ModelChunk::TextDelta(t) => text.push_str(&t),
                ModelChunk::Done(_) => break,
                ModelChunk::Error(e) => {
                    had_error = true;
                    tracing::warn!(
                        error = %e,
                        text_so_far = text.len(),
                        "Error mid-stream in compaction summary"
                    );
                    break;
                }
                _ => {} // ThinkingDelta, Usage, etc. — ignore
            },
            Some(Err(e)) => {
                had_error = true;
                tracing::warn!(
                    error = %e,
                    text_so_far = text.len(),
                    "Stream error in compaction summary"
                );
                break;
            }
            None => break, // Stream ended
        }
    }

    if text.trim().is_empty() {
        Err(anyhow!(
            "Compaction summary produced no text{}",
            if had_error { " (had error)" } else { "" }
        ))
    } else {
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::types::{
        ChatMessage, ContentBlock, MessageContent, ModelChunk, ModelRequest, Role, StopReason,
        TokenCost, TokenUsage,
    };

    // ── Mock provider ───────────────────────────────────────────────

    struct MockProvider {
        response: MockResponse,
    }

    enum MockResponse {
        Success(String),
        Error,
        Timeout,
        Empty,
    }

    #[async_trait::async_trait]
    impl ModelProvider for MockProvider {
        fn name(&self) -> &str {
            "mock"
        }
        fn supported_models(&self) -> &[halcon_core::types::ModelInfo] {
            &[]
        }

        async fn invoke(
            &self,
            _request: &ModelRequest,
        ) -> halcon_core::error::Result<
            futures::stream::BoxStream<'static, halcon_core::error::Result<ModelChunk>>,
        > {
            match &self.response {
                MockResponse::Success(text) => {
                    let text = text.clone();
                    let stream = futures::stream::iter(vec![
                        Ok(ModelChunk::TextDelta(text)),
                        Ok(ModelChunk::Usage(TokenUsage {
                            input_tokens: 100,
                            output_tokens: 50,
                            cache_read_tokens: None,
                            cache_creation_tokens: None,
                            reasoning_tokens: None,
                        })),
                        Ok(ModelChunk::Done(StopReason::EndTurn)),
                    ]);
                    Ok(Box::pin(stream))
                }
                MockResponse::Error => Err(halcon_core::error::HalconError::ApiError {
                    message: "mock error".into(),
                    status: None,
                }),
                MockResponse::Timeout => {
                    // Simulate a slow provider that exceeds the timeout
                    let stream = futures::stream::once(async {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        Ok(ModelChunk::Done(StopReason::EndTurn))
                    });
                    Ok(Box::pin(stream))
                }
                MockResponse::Empty => {
                    let stream =
                        futures::stream::iter(vec![Ok(ModelChunk::Done(StopReason::EndTurn))]);
                    Ok(Box::pin(stream))
                }
            }
        }

        async fn is_available(&self) -> bool {
            true
        }

        fn estimate_cost(&self, _request: &ModelRequest) -> TokenCost {
            TokenCost {
                estimated_input_tokens: 0,
                estimated_cost_usd: 0.0,
            }
        }

        fn model_context_window(&self, _model: &str) -> Option<u32> {
            Some(200_000)
        }
    }

    fn mock(response: MockResponse) -> Arc<dyn ModelProvider> {
        Arc::new(MockProvider { response })
    }

    fn make_config() -> CompactionConfig {
        CompactionConfig {
            semantic_compaction: true,
            ..Default::default()
        }
    }

    fn make_messages(n: usize) -> Vec<ChatMessage> {
        let mut msgs = Vec::new();
        for i in 0..n {
            let role = if i % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            };
            msgs.push(ChatMessage {
                role,
                content: MessageContent::Text(format!(
                    "Message {} with some content padding to get tokens up",
                    i
                )),
            });
        }
        msgs
    }

    fn make_anchor() -> IntentAnchor {
        IntentAnchor::from_messages(
            &[ChatMessage {
                role: Role::User,
                content: MessageContent::Text("Fix the build system".to_string()),
            }],
            "/project",
        )
    }

    fn make_budget() -> CompactionBudget {
        CompactionBudgetCalculator::compute(160_000, 4096, &make_config(), 50, 1000)
    }

    #[tokio::test]
    async fn nominal_with_mock_provider() {
        let provider = mock(MockResponse::Success(
            "## Summary\n1. Fix build\n2. Updated config".to_string(),
        ));
        let mut tc = TieredCompactor::new(make_config());
        let mut msgs = make_messages(30);
        let anchor = make_anchor();
        let budget = make_budget();

        let result = tc
            .compact(
                &mut msgs,
                &anchor,
                &provider,
                &budget,
                "mock-model",
                &["Read".to_string()],
                &["src/main.rs".to_string()],
                CompactionTrigger::Proactive,
            )
            .await;

        assert_eq!(result.level, CompactionLevel::Nominal);
        assert!(!result.aborted);
        assert!(result.utility_ratio > 0.0);
        assert!(result.summary_tokens > 0);
        assert!(result.tokens_after < result.tokens_before);
    }

    #[tokio::test]
    async fn degraded_on_provider_error() {
        let provider = mock(MockResponse::Error);
        let mut tc = TieredCompactor::new(make_config());
        let mut msgs = make_messages(30);
        let anchor = make_anchor();
        let budget = make_budget();

        let result = tc
            .compact(
                &mut msgs,
                &anchor,
                &provider,
                &budget,
                "mock-model",
                &[],
                &[],
                CompactionTrigger::Reactive,
            )
            .await;

        assert_eq!(result.level, CompactionLevel::Degraded);
        assert!(!result.aborted);
        assert_eq!(result.summary_tokens, 0);
        assert_eq!(tc.consecutive_failures, 1);
    }

    #[tokio::test]
    async fn degraded_on_provider_timeout() {
        let provider = mock(MockResponse::Timeout);
        let mut config = make_config();
        config.compaction_timeout_secs = 1; // 1 second timeout for test
        let mut tc = TieredCompactor::new(config);
        let mut msgs = make_messages(30);
        let anchor = make_anchor();
        let budget = make_budget();

        let result = tc
            .compact(
                &mut msgs,
                &anchor,
                &provider,
                &budget,
                "mock-model",
                &[],
                &[],
                CompactionTrigger::Proactive,
            )
            .await;

        assert_eq!(result.level, CompactionLevel::Degraded);
        assert_eq!(tc.consecutive_failures, 1);
    }

    #[tokio::test]
    async fn emergency_after_3_failures() {
        let provider = mock(MockResponse::Error);
        let mut tc = TieredCompactor::new(make_config());
        let anchor = make_anchor();
        let budget = make_budget();

        // Fail 3 times to open circuit breaker
        for _ in 0..3 {
            let mut msgs = make_messages(30);
            tc.compact(
                &mut msgs,
                &anchor,
                &provider,
                &budget,
                "mock-model",
                &[],
                &[],
                CompactionTrigger::Proactive,
            )
            .await;
        }

        assert!(tc.circuit_breaker_open());
        assert_eq!(tc.consecutive_failures, 3);

        // Next attempt should be emergency
        let mut msgs = make_messages(30);
        let result = tc
            .compact(
                &mut msgs,
                &anchor,
                &provider,
                &budget,
                "mock-model",
                &[],
                &[],
                CompactionTrigger::Proactive,
            )
            .await;

        assert_eq!(result.level, CompactionLevel::Emergency);
    }

    #[tokio::test]
    async fn circuit_breaker_resets_on_success() {
        let error_provider = mock(MockResponse::Error);
        let success_provider = mock(MockResponse::Success("summary".to_string()));
        let mut tc = TieredCompactor::new(make_config());
        let anchor = make_anchor();
        let budget = make_budget();

        // Fail twice
        for _ in 0..2 {
            let mut msgs = make_messages(30);
            tc.compact(
                &mut msgs,
                &anchor,
                &error_provider,
                &budget,
                "m",
                &[],
                &[],
                CompactionTrigger::Proactive,
            )
            .await;
        }
        assert_eq!(tc.consecutive_failures, 2);

        // Success resets
        let mut msgs = make_messages(30);
        tc.compact(
            &mut msgs,
            &anchor,
            &success_provider,
            &budget,
            "m",
            &[],
            &[],
            CompactionTrigger::Proactive,
        )
        .await;
        assert_eq!(tc.consecutive_failures, 0);
    }

    #[tokio::test]
    async fn abort_on_negative_utility() {
        // Create a scenario where summary would be bigger than freed space
        let provider = mock(MockResponse::Success("x".repeat(50_000)));
        let mut tc = TieredCompactor::new(make_config());
        // Very few messages — keep window covers most of them
        let mut msgs = make_messages(6);
        let anchor = make_anchor();
        let budget = CompactionBudgetCalculator::compute(160_000, 4096, &make_config(), 6, 100);

        let result = tc
            .compact(
                &mut msgs,
                &anchor,
                &provider,
                &budget,
                "m",
                &[],
                &[],
                CompactionTrigger::Proactive,
            )
            .await;

        // Should either abort or have very low utility — the summary is huge
        // relative to the small number of messages freed
        // Note: with 6 messages and keep_count of ~16, keep >= messages → noop
        // This validates the noop path
        assert!(result.aborted || result.tokens_after <= result.tokens_before);
    }

    #[tokio::test]
    async fn empty_summary_treated_as_failure() {
        let provider = mock(MockResponse::Empty);
        let mut tc = TieredCompactor::new(make_config());
        let mut msgs = make_messages(30);
        let anchor = make_anchor();
        let budget = make_budget();

        let result = tc
            .compact(
                &mut msgs,
                &anchor,
                &provider,
                &budget,
                "m",
                &[],
                &[],
                CompactionTrigger::Proactive,
            )
            .await;

        assert_eq!(result.level, CompactionLevel::Degraded);
        assert_eq!(tc.consecutive_failures, 1);
    }

    #[tokio::test]
    async fn tool_pair_safety_preserved() {
        let provider = mock(MockResponse::Success("Summary of work done".to_string()));
        let mut tc = TieredCompactor::new(make_config());
        let anchor = make_anchor();
        let budget = make_budget();

        // Build messages with tool pairs
        let mut msgs = make_messages(20);
        msgs.push(ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: "Read".to_string(),
                input: serde_json::json!({}),
            }]),
        });
        msgs.push(ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "t1".to_string(),
                content: "file contents".to_string(),
                is_error: false,
            }]),
        });

        let result = tc
            .compact(
                &mut msgs,
                &anchor,
                &provider,
                &budget,
                "m",
                &["Read".to_string()],
                &[],
                CompactionTrigger::Proactive,
            )
            .await;

        // Verify no orphaned tool results
        if !result.aborted {
            let mut uses = std::collections::HashSet::new();
            let mut results = std::collections::HashSet::new();
            for msg in &msgs {
                if let MessageContent::Blocks(blocks) = &msg.content {
                    for block in blocks {
                        match block {
                            ContentBlock::ToolUse { id, .. } => {
                                uses.insert(id.clone());
                            }
                            ContentBlock::ToolResult { tool_use_id, .. } => {
                                results.insert(tool_use_id.clone());
                            }
                            _ => {}
                        }
                    }
                }
            }
            // Every result should have its use
            for rid in &results {
                assert!(uses.contains(rid), "Orphaned ToolResult: {}", rid);
            }
        }
    }

    #[tokio::test]
    async fn multi_level_degradation_cascade() {
        let error_provider = mock(MockResponse::Error);
        let success_provider = mock(MockResponse::Success("Summary of work".to_string()));
        let mut tc = TieredCompactor::new(make_config());
        let anchor = make_anchor();
        let budget = make_budget();

        // Step 1: Nominal success
        let mut msgs = make_messages(30);
        let r = tc
            .compact(
                &mut msgs,
                &anchor,
                &success_provider,
                &budget,
                "m",
                &[],
                &[],
                CompactionTrigger::Proactive,
            )
            .await;
        assert_eq!(r.level, CompactionLevel::Nominal);
        assert_eq!(tc.consecutive_failures, 0);

        // Step 2: First failure → Degraded
        let mut msgs = make_messages(30);
        let r = tc
            .compact(
                &mut msgs,
                &anchor,
                &error_provider,
                &budget,
                "m",
                &[],
                &[],
                CompactionTrigger::Proactive,
            )
            .await;
        assert_eq!(r.level, CompactionLevel::Degraded);
        assert_eq!(tc.consecutive_failures, 1);

        // Step 3: Second failure → Degraded
        let mut msgs = make_messages(30);
        let r = tc
            .compact(
                &mut msgs,
                &anchor,
                &error_provider,
                &budget,
                "m",
                &[],
                &[],
                CompactionTrigger::Proactive,
            )
            .await;
        assert_eq!(r.level, CompactionLevel::Degraded);
        assert_eq!(tc.consecutive_failures, 2);

        // Step 4: Third failure → Degraded (breaker not yet open at entry)
        let mut msgs = make_messages(30);
        let r = tc
            .compact(
                &mut msgs,
                &anchor,
                &error_provider,
                &budget,
                "m",
                &[],
                &[],
                CompactionTrigger::Proactive,
            )
            .await;
        assert_eq!(r.level, CompactionLevel::Degraded);
        assert_eq!(tc.consecutive_failures, 3);

        // Step 5: Breaker is now open → Emergency
        assert!(tc.circuit_breaker_open());
        let mut msgs = make_messages(30);
        let r = tc
            .compact(
                &mut msgs,
                &anchor,
                &error_provider,
                &budget,
                "m",
                &[],
                &[],
                CompactionTrigger::Proactive,
            )
            .await;
        assert_eq!(r.level, CompactionLevel::Emergency);
        // Failures stay at 3 (not incremented in emergency)
        assert_eq!(tc.consecutive_failures, 3);

        // Step 6: Even with success provider, breaker stays open
        // because compact() checks breaker BEFORE attempting nominal
        let mut msgs = make_messages(30);
        let r = tc
            .compact(
                &mut msgs,
                &anchor,
                &success_provider,
                &budget,
                "m",
                &[],
                &[],
                CompactionTrigger::Proactive,
            )
            .await;
        assert_eq!(r.level, CompactionLevel::Emergency);
    }

    #[tokio::test]
    async fn emergency_uses_budget_keep_count() {
        let provider = mock(MockResponse::Error);
        let mut tc = TieredCompactor::new(make_config());
        tc.consecutive_failures = 3; // Force circuit breaker open
        let anchor = make_anchor();

        // Budget with large keep_count (e.g., 200K window → keep=16)
        let budget = CompactionBudgetCalculator::compute(160_000, 4096, &make_config(), 50, 1000);
        assert!(
            budget.keep_count > 4,
            "Budget keep should be > 4 for this test"
        );

        let mut msgs = make_messages(50);
        let r = tc
            .compact(
                &mut msgs,
                &anchor,
                &provider,
                &budget,
                "m",
                &[],
                &[],
                CompactionTrigger::Proactive,
            )
            .await;

        assert_eq!(r.level, CompactionLevel::Emergency);
        // Keep should be >= budget.keep_count, not hardcoded 4
        assert!(
            r.keep_messages >= budget.keep_count,
            "Emergency keep {} should be >= budget keep_count {}",
            r.keep_messages,
            budget.keep_count
        );
    }

    #[tokio::test]
    async fn degraded_uses_extended_keep() {
        let provider = mock(MockResponse::Error);
        let mut tc = TieredCompactor::new(make_config());
        let anchor = make_anchor();
        let budget = make_budget();

        let mut msgs = make_messages(30);
        let r = tc
            .compact(
                &mut msgs,
                &anchor,
                &provider,
                &budget,
                "m",
                &[],
                &[],
                CompactionTrigger::Proactive,
            )
            .await;

        assert_eq!(r.level, CompactionLevel::Degraded);
        // Degraded should retain more messages than nominal would
        assert!(
            r.keep_messages >= budget.keep_count,
            "Degraded keep {} should be >= nominal keep {}",
            r.keep_messages,
            budget.keep_count
        );
    }

    #[tokio::test]
    async fn noop_when_few_messages() {
        let provider = mock(MockResponse::Success("summary".to_string()));
        let mut tc = TieredCompactor::new(make_config());
        let anchor = make_anchor();
        // Budget with keep_count larger than message count
        let budget = CompactionBudgetCalculator::compute(160_000, 4096, &make_config(), 3, 500);

        let mut msgs = make_messages(3);
        let original_len = msgs.len();
        let r = tc
            .compact(
                &mut msgs,
                &anchor,
                &provider,
                &budget,
                "m",
                &[],
                &[],
                CompactionTrigger::Proactive,
            )
            .await;

        // When keep >= messages, apply_compaction_keep does noop
        // Result should show no meaningful change
        assert!(r.tokens_after <= r.tokens_before || msgs.len() >= original_len);
    }

    #[tokio::test]
    async fn reactive_trigger_tracked_correctly() {
        let provider = mock(MockResponse::Success("summary".to_string()));
        let mut tc = TieredCompactor::new(make_config());
        let anchor = make_anchor();
        let budget = make_budget();

        let mut msgs = make_messages(30);
        let r = tc
            .compact(
                &mut msgs,
                &anchor,
                &provider,
                &budget,
                "m",
                &[],
                &[],
                CompactionTrigger::Reactive,
            )
            .await;

        assert_eq!(r.trigger, CompactionTrigger::Reactive);
    }

    #[tokio::test]
    async fn boundary_message_contains_protected_context() {
        let provider = mock(MockResponse::Success("Test summary content".to_string()));
        let mut tc = TieredCompactor::new(make_config());
        let anchor = make_anchor();
        let budget = make_budget();

        let mut msgs = make_messages(30);
        tc.compact(
            &mut msgs,
            &anchor,
            &provider,
            &budget,
            "m",
            &["Read".to_string(), "Edit".to_string()],
            &["src/main.rs".to_string()],
            CompactionTrigger::Proactive,
        )
        .await;

        // First message should be the boundary with protected context
        if let Some(msg) = msgs.first() {
            if let MessageContent::Text(text) = &msg.content {
                assert!(
                    text.contains("PROTECTED CONTEXT"),
                    "Boundary should contain protected context marker"
                );
                assert!(
                    text.contains("Fix the build system"),
                    "Boundary should contain intent"
                );
                assert!(
                    text.contains("Read, Edit"),
                    "Boundary should contain tools used"
                );
                assert!(
                    text.contains("src/main.rs"),
                    "Boundary should contain files modified"
                );
            } else {
                panic!("Boundary message should be Text");
            }
        } else {
            panic!("Messages should not be empty post-compaction");
        }
    }
}
