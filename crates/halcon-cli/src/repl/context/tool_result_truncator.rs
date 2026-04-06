//! ToolResultTruncator: inline truncation of large tool results before compaction.
//!
//! Runs every turn, before compaction trigger check. Reduces context inflation
//! from large tool outputs, decreasing compaction frequency.

use halcon_context::estimate_tokens;
use halcon_core::types::{ChatMessage, ContentBlock, MessageContent};

/// Truncate tool results exceeding `threshold_tokens` in all messages
/// except the last `skip_recent` (current turn).
///
/// Returns the number of tool results truncated.
pub fn truncate_large_tool_results(
    messages: &mut Vec<ChatMessage>,
    threshold_tokens: usize,
    preview_tokens: usize,
) -> u32 {
    if messages.len() <= 2 || threshold_tokens == 0 {
        return 0;
    }

    let mut count = 0u32;
    let end = messages.len().saturating_sub(2);

    for msg in messages[..end].iter_mut() {
        if let MessageContent::Blocks(blocks) = &mut msg.content {
            for block in blocks.iter_mut() {
                if let ContentBlock::ToolResult {
                    content, is_error, ..
                } = block
                {
                    let tokens = estimate_tokens(content);
                    if tokens > threshold_tokens {
                        let original_tokens = tokens;
                        // Estimate chars for preview: ~4 chars per token
                        let preview_chars = preview_tokens * 4;
                        let preview: String = content
                            .chars()
                            .take(preview_chars.min(content.len()))
                            .collect();

                        let is_err = *is_error;
                        *content = format!(
                            "[Tool result truncated from ~{} to ~{} tokens. \
                             Use the tool again for full output.]\n{}",
                            original_tokens, preview_tokens, preview
                        );
                        // Verify is_error and id are not changed
                        debug_assert_eq!(*is_error, is_err, "is_error must not change");
                        count += 1;
                    }
                }
            }
        }
    }

    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::types::{ChatMessage, ContentBlock, MessageContent, Role};

    fn text_msg(role: Role, text: &str) -> ChatMessage {
        ChatMessage {
            role,
            content: MessageContent::Text(text.to_string()),
        }
    }

    fn tool_result_msg(tool_use_id: &str, content: &str, is_error: bool) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: content.to_string(),
                is_error,
            }]),
        }
    }

    fn tool_use_msg(id: &str) -> ChatMessage {
        ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: "Read".to_string(),
                input: serde_json::json!({}),
            }]),
        }
    }

    #[test]
    fn truncates_large_results() {
        let big = "word ".repeat(50_000); // ~50K tokens
        let mut msgs = vec![
            tool_use_msg("t1"),
            tool_result_msg("t1", &big, false),
            text_msg(Role::User, "continue"),     // recent
            text_msg(Role::Assistant, "ok done"), // recent
        ];
        let count = truncate_large_tool_results(&mut msgs, 8000, 2000);
        assert_eq!(count, 1);
        if let MessageContent::Blocks(blocks) = &msgs[1].content {
            if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
                assert!(content.contains("[Tool result truncated"));
                assert!(content.len() < big.len());
            } else {
                panic!("expected ToolResult");
            }
        }
    }

    #[test]
    fn preserves_small_results() {
        let small = "short output";
        let mut msgs = vec![
            tool_use_msg("t1"),
            tool_result_msg("t1", small, false),
            text_msg(Role::User, "next"),
            text_msg(Role::Assistant, "done"),
        ];
        let count = truncate_large_tool_results(&mut msgs, 8000, 2000);
        assert_eq!(count, 0);
        if let MessageContent::Blocks(blocks) = &msgs[1].content {
            if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
                assert_eq!(content, small);
            }
        }
    }

    #[test]
    fn preserves_tool_use_id() {
        let big = "x".repeat(100_000);
        let mut msgs = vec![
            tool_use_msg("my-unique-id"),
            tool_result_msg("my-unique-id", &big, false),
            text_msg(Role::User, "a"),
            text_msg(Role::Assistant, "b"),
        ];
        truncate_large_tool_results(&mut msgs, 8000, 2000);
        if let MessageContent::Blocks(blocks) = &msgs[1].content {
            if let ContentBlock::ToolResult { tool_use_id, .. } = &blocks[0] {
                assert_eq!(tool_use_id, "my-unique-id");
            }
        }
    }

    #[test]
    fn preserves_is_error() {
        let big = "err ".repeat(50_000);
        let mut msgs = vec![
            tool_use_msg("t1"),
            tool_result_msg("t1", &big, true),
            text_msg(Role::User, "a"),
            text_msg(Role::Assistant, "b"),
        ];
        truncate_large_tool_results(&mut msgs, 8000, 2000);
        if let MessageContent::Blocks(blocks) = &msgs[1].content {
            if let ContentBlock::ToolResult { is_error, .. } = &blocks[0] {
                assert!(*is_error);
            }
        }
    }

    #[test]
    fn skips_last_two_messages() {
        let big = "word ".repeat(50_000);
        let mut msgs = vec![
            text_msg(Role::User, "old"),
            // These are the last 2 — should NOT be truncated
            tool_use_msg("t1"),
            tool_result_msg("t1", &big, false),
        ];
        let count = truncate_large_tool_results(&mut msgs, 8000, 2000);
        assert_eq!(count, 0); // not truncated because it's in last 2
    }

    #[test]
    fn returns_correct_count() {
        let big = "x".repeat(100_000);
        let mut msgs = vec![
            tool_use_msg("t1"),
            tool_result_msg("t1", &big, false),
            tool_use_msg("t2"),
            tool_result_msg("t2", &big, false),
            text_msg(Role::User, "a"),
            text_msg(Role::Assistant, "b"),
        ];
        let count = truncate_large_tool_results(&mut msgs, 8000, 2000);
        assert_eq!(count, 2);
    }

    #[test]
    fn noop_on_empty() {
        let mut msgs: Vec<ChatMessage> = vec![];
        let count = truncate_large_tool_results(&mut msgs, 8000, 2000);
        assert_eq!(count, 0);
    }

    #[test]
    fn handles_text_messages_only() {
        let mut msgs = vec![
            text_msg(Role::User, "hello"),
            text_msg(Role::Assistant, "world"),
            text_msg(Role::User, "a"),
            text_msg(Role::Assistant, "b"),
        ];
        let count = truncate_large_tool_results(&mut msgs, 8000, 2000);
        assert_eq!(count, 0);
    }
}
