//! Context compaction: rolling summarization when messages approach the context window limit.
//!
//! When the estimated token count of the conversation exceeds a configurable fraction
//! of the model's context window, old messages are summarized into a compact form,
//! preserving the most recent messages for continuity.

use std::collections::HashSet;

use cuervo_context::estimate_tokens;
use cuervo_core::types::{ChatMessage, CompactionConfig, ContentBlock, MessageContent, Role};

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
                        cuervo_core::types::ContentBlock::Text { text } => estimate_tokens(text),
                        cuervo_core::types::ContentBlock::ToolUse { input, .. } => {
                            estimate_tokens(&input.to_string())
                        }
                        cuervo_core::types::ContentBlock::ToolResult { content, .. } => {
                            estimate_tokens(content)
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
                        cuervo_core::types::ContentBlock::Text { text } => text.as_str(),
                        cuervo_core::types::ContentBlock::ToolUse { name, .. } => {
                            name.as_str()
                        }
                        cuervo_core::types::ContentBlock::ToolResult { content, .. } => {
                            // Truncate long tool results in summary input
                            if content.len() > 200 {
                                "[...tool output truncated for summary...]"
                            } else {
                                content.as_str()
                            }
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

    /// Compute the safe keep boundary that preserves tool_use/tool_result pairs.
    ///
    /// Starts from `config.keep_recent`, then extends backwards if any
    /// `ToolResult` in the keep window references a `ToolUse` outside it.
    fn safe_keep_boundary(&self, messages: &[ChatMessage]) -> usize {
        let mut keep = self.config.keep_recent.min(messages.len());
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

    fn make_config(enabled: bool, threshold: f32, keep: usize, max_tokens: u32) -> CompactionConfig {
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
        // 200k tokens * 0.80 = 160k threshold. Each token ~4 chars.
        // So we need ~640k chars to trigger.
        let compactor = ContextCompactor::new(make_config(true, 0.80, 4, 1000));
        // 1000 * 0.80 = 800 token threshold = 3200 chars
        let big_text = "x".repeat(4000);
        let messages = vec![text_msg(Role::User, &big_text)];
        assert!(compactor.needs_compaction(&messages));
    }

    #[test]
    fn needs_compaction_disabled() {
        let compactor = ContextCompactor::new(make_config(false, 0.80, 4, 1000));
        let big_text = "x".repeat(4000);
        let messages = vec![text_msg(Role::User, &big_text)];
        assert!(!compactor.needs_compaction(&messages));
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
        assert_eq!(
            messages[1].content.as_text().unwrap(),
            "recent message"
        );
        assert_eq!(
            messages[2].content.as_text().unwrap(),
            "recent response"
        );
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
            text_msg(Role::User, "hello"), // 5 chars → 2 tokens
            text_msg(Role::Assistant, "world"), // 5 chars → 2 tokens
        ];
        let tokens = ContextCompactor::estimate_message_tokens(&messages);
        assert_eq!(tokens, 4); // 10 chars / 4 = 2.5 → ceil = 4 (2+2)
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
            content: MessageContent::Blocks(vec![
                cuervo_core::types::ContentBlock::ToolUse {
                    id: id.to_string(),
                    name: name.to_string(),
                    input: serde_json::json!({}),
                },
            ]),
        }
    }

    fn tool_result_msg(tool_use_id: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(vec![
                cuervo_core::types::ContentBlock::ToolResult {
                    tool_use_id: tool_use_id.to_string(),
                    content: content.to_string(),
                    is_error: false,
                },
            ]),
        }
    }

    #[test]
    fn compaction_extends_keep_for_orphaned_tool_result() {
        // keep_recent=2: would keep only messages[4] and messages[5].
        // messages[4] is a ToolResult referencing "t1" which is in messages[3].
        // The compactor must extend to keep messages[3] too.
        let compactor = ContextCompactor::new(make_config(true, 0.80, 2, 200_000));
        let mut messages = vec![
            text_msg(Role::User, "old 1"),            // 0: discarded
            text_msg(Role::Assistant, "old 2"),        // 1: discarded
            text_msg(Role::User, "old 3"),             // 2: discarded
            tool_use_msg("t1", "bash"),                // 3: MUST be kept (has t1)
            tool_result_msg("t1", "ok"),               // 4: kept (references t1)
            text_msg(Role::Assistant, "done"),          // 5: kept
        ];
        compactor.apply_compaction(&mut messages, "Summary");

        // Summary + messages[3,4,5] = 4 messages
        assert_eq!(messages.len(), 4, "Expected summary + 3 kept messages");
        assert!(messages[0].content.as_text().unwrap().contains("Summary"));

        // Verify the tool_use message was preserved.
        if let MessageContent::Blocks(blocks) = &messages[1].content {
            assert!(matches!(&blocks[0], cuervo_core::types::ContentBlock::ToolUse { id, .. } if id == "t1"));
        } else {
            panic!("Expected blocks in message[1]");
        }
    }

    #[test]
    fn compaction_no_extension_when_pairs_intact() {
        // keep_recent=4: the tool pair is entirely within the keep window.
        let compactor = ContextCompactor::new(make_config(true, 0.80, 4, 200_000));
        let mut messages = vec![
            text_msg(Role::User, "old"),               // 0: discarded
            text_msg(Role::Assistant, "old"),           // 1: discarded
            text_msg(Role::User, "recent"),             // 2: kept
            tool_use_msg("t1", "bash"),                // 3: kept
            tool_result_msg("t1", "ok"),               // 4: kept
            text_msg(Role::Assistant, "done"),          // 5: kept
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
            text_msg(Role::User, "old"),               // 0
            text_msg(Role::Assistant, "old"),           // 1
            ChatMessage {                               // 2: assistant with 2 tool uses
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![
                    cuervo_core::types::ContentBlock::ToolUse {
                        id: "t1".to_string(),
                        name: "bash".to_string(),
                        input: serde_json::json!({}),
                    },
                    cuervo_core::types::ContentBlock::ToolUse {
                        id: "t2".to_string(),
                        name: "file_read".to_string(),
                        input: serde_json::json!({}),
                    },
                ]),
            },
            ChatMessage {                               // 3: user with 2 tool results
                role: Role::User,
                content: MessageContent::Blocks(vec![
                    cuervo_core::types::ContentBlock::ToolResult {
                        tool_use_id: "t1".to_string(),
                        content: "ok1".to_string(),
                        is_error: false,
                    },
                    cuervo_core::types::ContentBlock::ToolResult {
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
}
