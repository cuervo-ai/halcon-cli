//! Context compaction: rolling summarization when messages approach the context window limit.
//!
//! When the estimated token count of the conversation exceeds a configurable fraction
//! of the model's context window, old messages are summarized into a compact form,
//! preserving the most recent messages for continuity.

use std::collections::HashSet;

use halcon_context::estimate_tokens;
use halcon_core::types::{ChatMessage, CompactionConfig, ContentBlock, MessageContent, Role};

/// Manages context compaction decisions and message replacement.
pub struct ContextCompactor {
    config: CompactionConfig,
}

impl ContextCompactor {
    pub fn new(config: CompactionConfig) -> Self {
        Self { config }
    }

    /// Estimate the total token count across all messages.
    pub fn estimate_message_tokens(messages: &[ChatMessage]) -> usize {
        messages
            .iter()
            .map(|msg| match &msg.content {
                MessageContent::Text(t) => estimate_tokens(t),
                MessageContent::Blocks(blocks) => blocks
                    .iter()
                    .map(|b| match b {
                        halcon_core::types::ContentBlock::Text { text } => estimate_tokens(text),
                        halcon_core::types::ContentBlock::ToolUse { input, .. } => {
                            estimate_tokens(&input.to_string())
                        }
                        halcon_core::types::ContentBlock::ToolResult { content, .. } => {
                            estimate_tokens(content)
                        }
                        halcon_core::types::ContentBlock::Image { .. } => 255,
                        halcon_core::types::ContentBlock::AudioTranscript { text, .. } => {
                            estimate_tokens(text)
                        }
                    })
                    .sum(),
            })
            .sum()
    }

    /// Check if compaction is needed based on current token usage.
    pub fn needs_compaction(&self, messages: &[ChatMessage]) -> bool {
        if !self.config.enabled || self.config.max_context_tokens == 0 {
            return false;
        }
        let current = Self::estimate_message_tokens(messages);
        let threshold =
            (self.config.max_context_tokens as f32 * self.config.threshold_fraction) as usize;
        current >= threshold
    }

    /// Check if compaction is needed given an explicit pipeline budget.
    ///
    /// Use this instead of `needs_compaction()` when the pipeline budget has been derived
    /// from the model's actual context window (Fix A). Without this override, the compactor
    /// uses its stale config value (default 200K), causing compaction to trigger at
    /// 80% × 200K = 160K — never firing for providers like DeepSeek (64K context window).
    ///
    /// Threshold: 60% of `pipeline_budget`. At 60% of 80% = 48% of the model's total context
    /// window, which reserves 40% of the pipeline budget for output tokens plus a 10% safety
    /// margin above the old 70% threshold. This prevents the agent from invoking the model
    /// when insufficient headroom remains for a non-truncated response.
    pub fn needs_compaction_with_budget(
        &self,
        messages: &[ChatMessage],
        pipeline_budget: u32,
    ) -> bool {
        if !self.config.enabled || pipeline_budget == 0 {
            return false;
        }
        let current = Self::estimate_message_tokens(messages);
        let threshold = (pipeline_budget as f32 * 0.60) as usize;
        current >= threshold
    }

    /// Generate a compaction prompt that asks the model to summarize the conversation.
    pub fn compaction_prompt(&self, messages: &[ChatMessage]) -> String {
        let keep = self.config.keep_recent.min(messages.len());
        let to_summarize = &messages[..messages.len().saturating_sub(keep)];

        let mut context = String::new();
        for msg in to_summarize {
            let role = match msg.role {
                Role::User => "User",
                Role::Assistant => "Assistant",
                Role::System => "System",
            };
            let text = match &msg.content {
                MessageContent::Text(t) => t.clone(),
                MessageContent::Blocks(blocks) => blocks
                    .iter()
                    .map(|b| match b {
                        halcon_core::types::ContentBlock::Text { text } => text.as_str(),
                        halcon_core::types::ContentBlock::ToolUse { name, .. } => name.as_str(),
                        halcon_core::types::ContentBlock::ToolResult { content, .. } => {
                            // Truncate long tool results in summary input
                            if content.len() > 200 {
                                "[...tool output truncated for summary...]"
                            } else {
                                content.as_str()
                            }
                        }
                        halcon_core::types::ContentBlock::Image { .. } => "[image]",
                        halcon_core::types::ContentBlock::AudioTranscript { text, .. } => {
                            text.as_str()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" | "),
            };
            context.push_str(&format!("[{role}]: {text}\n"));
        }

        format!(
            "Summarize the following conversation context into a concise summary. \
             Preserve:\n\
             - Key decisions made\n\
             - Files created or modified\n\
             - Pending tasks or next steps\n\
             - Critical context needed for continuity\n\n\
             Keep the summary under 500 words.\n\n\
             Conversation to summarize:\n{context}"
        )
    }

    /// Replace old messages with a compacted summary, preserving the most recent N messages.
    ///
    /// The resulting message list is: [summary_message, ...recent_messages].
    ///
    /// # Tool pair safety
    ///
    /// If the keep boundary would split a ToolUse/ToolResult pair (i.e. the kept
    /// messages contain a `ToolResult` whose matching `ToolUse` is outside the
    /// window), the window is extended backwards to include the matching
    /// Assistant message. This prevents orphaned ToolResults that cause 400
    /// errors from providers.
    pub fn apply_compaction(&self, messages: &mut Vec<ChatMessage>, summary: &str) {
        let keep = self.safe_keep_boundary(messages);
        self.apply_compaction_keep(messages, summary, keep);
    }

    /// Apply compaction with an adaptive keep count derived from `pipeline_budget`.
    ///
    /// Prefer this over `apply_compaction()` when the pipeline budget is known so that
    /// the preserved recent window scales proportionally to the available context window.
    /// Larger context windows can afford to keep more messages in view for continuity.
    ///
    /// See `adaptive_keep_recent()` for the scaling formula.
    pub fn apply_compaction_with_budget(
        &self,
        messages: &mut Vec<ChatMessage>,
        summary: &str,
        pipeline_budget: u32,
    ) {
        let adaptive_n = Self::adaptive_keep_recent(pipeline_budget);
        let keep = self.safe_keep_boundary_n(messages, adaptive_n);
        self.apply_compaction_keep(messages, summary, keep);
    }

    /// Compute an adaptive keep count proportional to the pipeline budget.
    ///
    /// Formula: `max(4, pipeline_budget / 10_000)` capped at 20.
    ///
    /// | pipeline_budget | keep |
    /// |----------------|------|
    /// | ≤ 40 K         | 4    |
    /// | 64 K (DeepSeek)| 6    |
    /// | 128 K          | 12   |
    /// | 160 K          | 16   |
    /// | ≥ 200 K        | 20   |
    pub fn adaptive_keep_recent(pipeline_budget: u32) -> usize {
        let proportional = (pipeline_budget / 10_000) as usize;
        proportional.max(4).min(20)
    }

    // ── Internal helpers ─────────────────────────────────────────────────────

    /// Apply compaction with a pre-computed keep count.
    fn apply_compaction_keep(&self, messages: &mut Vec<ChatMessage>, summary: &str, keep: usize) {
        if keep >= messages.len() {
            // Nothing to compact — all messages are "recent".
            return;
        }

        let recent: Vec<ChatMessage> = messages[messages.len() - keep..].to_vec();

        messages.clear();

        // Insert summary as a user message (provides context for the assistant).
        messages.push(ChatMessage {
            role: Role::User,
            content: MessageContent::Text(format!(
                "[Context Summary — previous messages were compacted]\n\n{summary}"
            )),
        });

        // Re-add recent messages.
        messages.extend(recent);
    }

    /// Compute the safe keep boundary using `config.keep_recent`.
    fn safe_keep_boundary(&self, messages: &[ChatMessage]) -> usize {
        self.safe_keep_boundary_n(messages, self.config.keep_recent)
    }

    /// Compute the safe keep boundary starting from an explicit `initial_keep` count.
    ///
    /// Extends the window backwards if any `ToolResult` in the keep window
    /// references a `ToolUse` outside it — preventing orphaned ToolResults
    /// that cause 400 errors from providers.
    fn safe_keep_boundary_n(&self, messages: &[ChatMessage], initial_keep: usize) -> usize {
        let mut keep = initial_keep.min(messages.len());
        if keep >= messages.len() {
            return keep;
        }

        // Collect ToolUse IDs declared in the keep window.
        let boundary = messages.len() - keep;
        let mut declared_in_window: HashSet<&str> = HashSet::new();
        let mut needed_ids: HashSet<&str> = HashSet::new();

        for msg in &messages[boundary..] {
            if let MessageContent::Blocks(blocks) = &msg.content {
                for block in blocks {
                    match block {
                        ContentBlock::ToolUse { id, .. } => {
                            declared_in_window.insert(id);
                        }
                        ContentBlock::ToolResult { tool_use_id, .. } => {
                            needed_ids.insert(tool_use_id);
                        }
                        _ => {}
                    }
                }
            }
        }

        // Find ToolResult IDs whose ToolUse is missing from the keep window.
        let orphaned: HashSet<&str> = needed_ids
            .difference(&declared_in_window)
            .copied()
            .collect();

        if orphaned.is_empty() {
            return keep;
        }

        // Scan backwards from the boundary to find Assistant messages with
        // the needed ToolUse IDs.
        for idx in (0..boundary).rev() {
            let msg = &messages[idx];
            if let MessageContent::Blocks(blocks) = &msg.content {
                let has_needed = blocks.iter().any(|block| {
                    if let ContentBlock::ToolUse { id, .. } = block {
                        orphaned.contains(id.as_str())
                    } else {
                        false
                    }
                });
                if has_needed {
                    // Extend keep to include this message (and everything after it).
                    keep = messages.len() - idx;
                    break;
                }
            }
        }

        keep
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(
        enabled: bool,
        threshold: f32,
        keep: usize,
        max_tokens: u32,
    ) -> CompactionConfig {
        CompactionConfig {
            enabled,
            threshold_fraction: threshold,
            keep_recent: keep,
            max_context_tokens: max_tokens,
        }
    }

    fn text_msg(role: Role, text: &str) -> ChatMessage {
        ChatMessage {
            role,
            content: MessageContent::Text(text.to_string()),
        }
    }

    #[test]
    fn needs_compaction_below_threshold() {
        let compactor = ContextCompactor::new(make_config(true, 0.80, 4, 200_000));
        let messages = vec![text_msg(Role::User, "short message")];
        assert!(!compactor.needs_compaction(&messages));
    }

    #[test]
    fn needs_compaction_above_threshold() {
        // 1000 * 0.80 = 800 token threshold.
        // BPE: "word " ≈ 1 token. Use 801 repeats to exceed 800.
        let compactor = ContextCompactor::new(make_config(true, 0.80, 4, 1000));
        let big_text = "word ".repeat(801);
        let messages = vec![text_msg(Role::User, &big_text)];
        assert!(compactor.needs_compaction(&messages));
    }

    #[test]
    fn needs_compaction_disabled() {
        let compactor = ContextCompactor::new(make_config(false, 0.80, 4, 1000));
        let big_text = "word ".repeat(801); // Would exceed threshold if enabled
        let messages = vec![text_msg(Role::User, &big_text)];
        assert!(!compactor.needs_compaction(&messages));
    }

    // --- Fix B: needs_compaction_with_budget ---

    /// Demonstrates the mismatch: old needs_compaction() uses 200K config → no trigger.
    /// New needs_compaction_with_budget(64K×0.8=51.2K, 60%) → triggers correctly.
    #[test]
    fn compaction_with_budget_fires_for_small_context_window() {
        // Compactor configured with stale 200K (old default).
        let compactor = ContextCompactor::new(make_config(true, 0.80, 4, 200_000));
        // BPE: "word " ≈ 1 token. 7000 repeats = ~7000 tokens.
        let big_text = "word ".repeat(7000);
        let messages = vec![text_msg(Role::User, &big_text)];

        // Old method: threshold = 80% × 200K = 160K → NOT triggered (7K < 160K).
        assert!(!compactor.needs_compaction(&messages));

        // Fix B method: threshold = 60% × 51200 = 30720.
        // Need > 30720 tokens.
        let big_text2 = "word ".repeat(30_721);
        let messages2 = vec![text_msg(Role::User, &big_text2)];
        let pipeline_budget = (64_000_u32 as f64 * 0.80) as u32; // 51200
        assert!(
            compactor.needs_compaction_with_budget(&messages2, pipeline_budget),
            "Should trigger compaction: 30721 tokens > 60% × 51.2K = 30720 threshold"
        );
    }

    #[test]
    fn compaction_with_budget_disabled_when_zero_budget() {
        let compactor = ContextCompactor::new(make_config(true, 0.80, 4, 200_000));
        let big_text = "word ".repeat(500_000);
        let messages = vec![text_msg(Role::User, &big_text)];
        // Budget of 0 = disabled.
        assert!(!compactor.needs_compaction_with_budget(&messages, 0));
    }

    #[test]
    fn compaction_with_budget_60_percent_threshold() {
        let compactor = ContextCompactor::new(make_config(true, 0.80, 4, 10_000));
        // pipeline_budget = 8000 tokens. 60% threshold = (8000 * 0.60) as usize = 4800 tokens.
        // BPE: "word ".repeat(N) ≈ N+1 tokens.
        let just_below = "word ".repeat(4798); // 4799 tokens — below threshold
        let just_above = "word ".repeat(4799); // 4800 tokens — at threshold
        let msgs_below = vec![text_msg(Role::User, &just_below)];
        let msgs_above = vec![text_msg(Role::User, &just_above)];
        assert!(
            !compactor.needs_compaction_with_budget(&msgs_below, 8000),
            "4799 tokens < 4800 threshold — should NOT compact"
        );
        assert!(
            compactor.needs_compaction_with_budget(&msgs_above, 8000),
            "4800 tokens >= 4800 threshold — should compact"
        );
    }

    #[test]
    fn compaction_prompt_includes_instructions() {
        let compactor = ContextCompactor::new(make_config(true, 0.80, 1, 200_000));
        let messages = vec![
            text_msg(Role::User, "Create a new file"),
            text_msg(Role::Assistant, "I created foo.rs"),
            text_msg(Role::User, "Now add tests"),
        ];
        let prompt = compactor.compaction_prompt(&messages);
        assert!(prompt.contains("Summarize"));
        assert!(prompt.contains("Key decisions"));
        assert!(prompt.contains("Create a new file"));
        // Last message (keep_recent=1) should NOT be in summary input
        assert!(!prompt.contains("Now add tests"));
    }

    #[test]
    fn apply_compaction_preserves_recent() {
        let compactor = ContextCompactor::new(make_config(true, 0.80, 2, 200_000));
        let mut messages = vec![
            text_msg(Role::User, "old message 1"),
            text_msg(Role::Assistant, "old response 1"),
            text_msg(Role::User, "recent message"),
            text_msg(Role::Assistant, "recent response"),
        ];
        compactor.apply_compaction(&mut messages, "Summary of old conversation");

        // Should have: summary + 2 recent
        assert_eq!(messages.len(), 3);
        assert!(messages[0]
            .content
            .as_text()
            .unwrap()
            .contains("Summary of old conversation"));
        assert_eq!(messages[1].content.as_text().unwrap(), "recent message");
        assert_eq!(messages[2].content.as_text().unwrap(), "recent response");
    }

    #[test]
    fn apply_compaction_all_recent_noop() {
        let compactor = ContextCompactor::new(make_config(true, 0.80, 10, 200_000));
        let mut messages = vec![
            text_msg(Role::User, "msg1"),
            text_msg(Role::Assistant, "msg2"),
        ];
        let original_len = messages.len();
        compactor.apply_compaction(&mut messages, "Summary");
        // keep_recent=10 > messages.len()=2, so no compaction
        assert_eq!(messages.len(), original_len);
    }

    #[test]
    fn estimate_message_tokens_basic() {
        let messages = vec![
            text_msg(Role::User, "hello"),
            text_msg(Role::Assistant, "world"),
        ];
        let tokens = ContextCompactor::estimate_message_tokens(&messages);
        // BPE: "hello" = 1 token, "world" = 1 token
        assert_eq!(tokens, 2);
    }

    #[test]
    fn compaction_config_defaults() {
        let config = CompactionConfig::default();
        assert!(config.enabled);
        assert!((config.threshold_fraction - 0.80).abs() < 0.01);
        assert_eq!(config.keep_recent, 4);
        assert_eq!(config.max_context_tokens, 200_000);
    }

    // ── Tool pair safety tests ──

    fn tool_use_msg(id: &str, name: &str) -> ChatMessage {
        ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![halcon_core::types::ContentBlock::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input: serde_json::json!({}),
            }]),
        }
    }

    fn tool_result_msg(tool_use_id: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(vec![halcon_core::types::ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: content.to_string(),
                is_error: false,
            }]),
        }
    }

    #[test]
    fn compaction_extends_keep_for_orphaned_tool_result() {
        // keep_recent=2: would keep only messages[4] and messages[5].
        // messages[4] is a ToolResult referencing "t1" which is in messages[3].
        // The compactor must extend to keep messages[3] too.
        let compactor = ContextCompactor::new(make_config(true, 0.80, 2, 200_000));
        let mut messages = vec![
            text_msg(Role::User, "old 1"),      // 0: discarded
            text_msg(Role::Assistant, "old 2"), // 1: discarded
            text_msg(Role::User, "old 3"),      // 2: discarded
            tool_use_msg("t1", "bash"),         // 3: MUST be kept (has t1)
            tool_result_msg("t1", "ok"),        // 4: kept (references t1)
            text_msg(Role::Assistant, "done"),  // 5: kept
        ];
        compactor.apply_compaction(&mut messages, "Summary");

        // Summary + messages[3,4,5] = 4 messages
        assert_eq!(messages.len(), 4, "Expected summary + 3 kept messages");
        assert!(messages[0].content.as_text().unwrap().contains("Summary"));

        // Verify the tool_use message was preserved.
        if let MessageContent::Blocks(blocks) = &messages[1].content {
            assert!(
                matches!(&blocks[0], halcon_core::types::ContentBlock::ToolUse { id, .. } if id == "t1")
            );
        } else {
            panic!("Expected blocks in message[1]");
        }
    }

    #[test]
    fn compaction_no_extension_when_pairs_intact() {
        // keep_recent=4: the tool pair is entirely within the keep window.
        let compactor = ContextCompactor::new(make_config(true, 0.80, 4, 200_000));
        let mut messages = vec![
            text_msg(Role::User, "old"),       // 0: discarded
            text_msg(Role::Assistant, "old"),  // 1: discarded
            text_msg(Role::User, "recent"),    // 2: kept
            tool_use_msg("t1", "bash"),        // 3: kept
            tool_result_msg("t1", "ok"),       // 4: kept
            text_msg(Role::Assistant, "done"), // 5: kept
        ];
        compactor.apply_compaction(&mut messages, "Summary");

        // Summary + 4 recent = 5 messages
        assert_eq!(messages.len(), 5);
    }

    #[test]
    fn compaction_with_no_tool_blocks_unchanged() {
        let compactor = ContextCompactor::new(make_config(true, 0.80, 2, 200_000));
        let mut messages = vec![
            text_msg(Role::User, "old 1"),
            text_msg(Role::Assistant, "old 2"),
            text_msg(Role::User, "recent"),
            text_msg(Role::Assistant, "latest"),
        ];
        compactor.apply_compaction(&mut messages, "Summary");

        // Summary + 2 recent = 3 messages (no extension needed)
        assert_eq!(messages.len(), 3);
    }

    #[test]
    fn compaction_extends_for_multi_tool_pair() {
        // Two tool calls in one assistant message, results in next user message.
        // keep_recent=1 would only keep messages[3], which has ToolResult for t1,t2.
        // Must extend back to messages[2] which has the ToolUse blocks.
        let compactor = ContextCompactor::new(make_config(true, 0.80, 1, 200_000));
        let mut messages = vec![
            text_msg(Role::User, "old"),      // 0
            text_msg(Role::Assistant, "old"), // 1
            ChatMessage {
                // 2: assistant with 2 tool uses
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![
                    halcon_core::types::ContentBlock::ToolUse {
                        id: "t1".to_string(),
                        name: "bash".to_string(),
                        input: serde_json::json!({}),
                    },
                    halcon_core::types::ContentBlock::ToolUse {
                        id: "t2".to_string(),
                        name: "file_read".to_string(),
                        input: serde_json::json!({}),
                    },
                ]),
            },
            ChatMessage {
                // 3: user with 2 tool results
                role: Role::User,
                content: MessageContent::Blocks(vec![
                    halcon_core::types::ContentBlock::ToolResult {
                        tool_use_id: "t1".to_string(),
                        content: "ok1".to_string(),
                        is_error: false,
                    },
                    halcon_core::types::ContentBlock::ToolResult {
                        tool_use_id: "t2".to_string(),
                        content: "ok2".to_string(),
                        is_error: false,
                    },
                ]),
            },
        ];
        compactor.apply_compaction(&mut messages, "Summary");

        // Summary + messages[2,3] = 3
        assert_eq!(messages.len(), 3, "Expected summary + 2 kept (tool pair)");
    }

    // ── A-2: Truncation indicator tests ──

    #[test]
    fn compaction_prompt_includes_truncation_indicator() {
        let compactor = ContextCompactor::new(make_config(true, 0.80, 0, 200_000));
        let long_content = "x".repeat(300); // > 200 chars
        let messages = vec![
            tool_use_msg("t1", "bash"),
            tool_result_msg("t1", &long_content),
        ];
        let prompt = compactor.compaction_prompt(&messages);
        assert!(
            prompt.contains("[...tool output truncated for summary...]"),
            "Expected truncation indicator in compaction prompt, got:\n{prompt}"
        );
        // The raw long content should NOT appear
        assert!(
            !prompt.contains(&long_content),
            "Long tool output should not appear verbatim in prompt"
        );
    }

    #[test]
    fn compaction_prompt_preserves_short_tool_results() {
        let compactor = ContextCompactor::new(make_config(true, 0.80, 0, 200_000));
        let short_content = "file created successfully";
        let messages = vec![
            tool_use_msg("t1", "file_write"),
            tool_result_msg("t1", short_content),
        ];
        let prompt = compactor.compaction_prompt(&messages);
        assert!(
            prompt.contains(short_content),
            "Short tool result should be preserved in prompt"
        );
        assert!(
            !prompt.contains("[...tool output truncated for summary...]"),
            "Short tool result should NOT show truncation indicator"
        );
    }

    // ── Adaptive keep_recent tests ──

    #[test]
    fn adaptive_keep_recent_minimum_floor() {
        // Budgets ≤ 40K should always yield at least 4.
        assert_eq!(ContextCompactor::adaptive_keep_recent(0), 4);
        assert_eq!(
            ContextCompactor::adaptive_keep_recent(10_000),
            1_usize.max(4)
        );
        assert_eq!(
            ContextCompactor::adaptive_keep_recent(39_999),
            3_usize.max(4)
        );
        assert_eq!(ContextCompactor::adaptive_keep_recent(40_000), 4);
    }

    #[test]
    fn adaptive_keep_recent_deepseek_64k() {
        // 64K × 80% = 51.2K pipeline_budget → proportional = 5, max(5,4) = 5 → wait:
        // 51_200 / 10_000 = 5. max(5,4).min(20) = 5. Hmm.
        // Actually: 64_000 * 0.80 = 51_200 as u32.
        // 51_200 / 10_000 = 5. max(5,4) = 5. min(5,20) = 5.
        let pipeline_budget = (64_000_u32 as f64 * 0.80) as u32; // 51_200
        assert_eq!(ContextCompactor::adaptive_keep_recent(pipeline_budget), 5);
    }

    #[test]
    fn adaptive_keep_recent_128k_window() {
        // 128K × 80% = 102.4K → 102_400 / 10_000 = 10. max(10,4).min(20) = 10.
        let pipeline_budget = (128_000_u32 as f64 * 0.80) as u32;
        assert_eq!(ContextCompactor::adaptive_keep_recent(pipeline_budget), 10);
    }

    #[test]
    fn adaptive_keep_recent_200k_window() {
        // 200K × 80% = 160K → 160_000 / 10_000 = 16. max(16,4).min(20) = 16.
        let pipeline_budget = (200_000_u32 as f64 * 0.80) as u32;
        assert_eq!(ContextCompactor::adaptive_keep_recent(pipeline_budget), 16);
    }

    #[test]
    fn adaptive_keep_recent_very_large_window_capped() {
        // Very large budget (e.g. 1M) → proportional = 80. Capped at 20.
        let pipeline_budget = (1_000_000_u32 as f64 * 0.80) as u32;
        assert_eq!(ContextCompactor::adaptive_keep_recent(pipeline_budget), 20);
    }

    #[test]
    fn apply_compaction_with_budget_keeps_proportional_messages() {
        // With a 64K pipeline_budget, adaptive_keep = 5.
        // 10-message conversation → 5 recent + summary.
        let compactor = ContextCompactor::new(make_config(true, 0.80, 4, 200_000));
        let mut messages: Vec<ChatMessage> = (0..10)
            .map(|i| {
                let role = if i % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                };
                text_msg(role, &format!("message {i}"))
            })
            .collect();
        let pipeline_budget = (64_000_u32 as f64 * 0.80) as u32; // 51_200 → keep = 5
        let expected_keep = ContextCompactor::adaptive_keep_recent(pipeline_budget);
        compactor.apply_compaction_with_budget(&mut messages, "Summary", pipeline_budget);
        // summary + expected_keep
        assert_eq!(
            messages.len(),
            1 + expected_keep,
            "Expected summary + {} recent messages",
            expected_keep
        );
        assert!(messages[0].content.as_text().unwrap().contains("Summary"));
    }

    #[test]
    fn apply_compaction_with_budget_noop_when_all_recent() {
        // If messages.len() <= keep, no compaction occurs.
        let compactor = ContextCompactor::new(make_config(true, 0.80, 4, 200_000));
        let mut messages = vec![text_msg(Role::User, "a"), text_msg(Role::Assistant, "b")];
        let pipeline_budget = (64_000_u32 as f64 * 0.80) as u32; // keep = 5 > 2
        let original_len = messages.len();
        compactor.apply_compaction_with_budget(&mut messages, "Summary", pipeline_budget);
        assert_eq!(
            messages.len(),
            original_len,
            "Should be no-op when all messages fit in keep window"
        );
    }
}
