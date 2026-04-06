//! Tool result eviction by age/recency (Fase 2).
//!
//! Replaces old tool result content with compact summaries, reducing context
//! inflation BEFORE compaction is triggered. Unlike truncation (which caps
//! individual large results), eviction targets old results regardless of size.
//!
//! Design:
//!   - Operates on messages older than `keep_recent` turns.
//!   - Preserves tool_use_id and is_error (tool pair safety).
//!   - Replaces content with a one-line summary: status + first line.
//!   - Configurable via keep_recent count and age threshold.
//!   - Runs after persistence (so originals are on disk) and before truncation.

use halcon_core::types::{ChatMessage, ContentBlock, MessageContent};

// ── EvictionPolicy ──────────────────────────────────────────────────────────

/// Policy for deciding which tool results to evict.
#[derive(Debug, Clone)]
pub struct EvictionPolicy {
    /// Number of most recent messages to never evict from.
    pub keep_recent: usize,
    /// Minimum content length (chars) to consider for eviction.
    /// Results shorter than this are left alone.
    pub min_content_chars: usize,
    /// Maximum length of the evicted summary (chars).
    pub summary_max_chars: usize,
}

impl Default for EvictionPolicy {
    fn default() -> Self {
        Self {
            keep_recent: 6,
            min_content_chars: 500,
            summary_max_chars: 120,
        }
    }
}

// ── EvictionResult ──────────────────────────────────────────────────────────

/// Metrics from an eviction pass.
#[derive(Debug, Clone)]
pub struct EvictionResult {
    /// Number of tool results evicted.
    pub evicted: u32,
    /// Estimated tokens freed by eviction.
    pub tokens_freed: usize,
    /// Number of tool results scanned but not evicted.
    pub skipped: u32,
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Evict old tool results from messages, replacing content with compact summaries.
///
/// Preserves:
/// - `tool_use_id` (tool pair safety — ToolResult still matches its ToolUse)
/// - `is_error` flag
/// - First line of content (for context)
///
/// Returns metrics about the eviction pass.
pub fn evict_old_tool_results(
    messages: &mut Vec<ChatMessage>,
    policy: &EvictionPolicy,
) -> EvictionResult {
    if messages.len() <= policy.keep_recent {
        return EvictionResult {
            evicted: 0,
            tokens_freed: 0,
            skipped: 0,
        };
    }

    let mut evicted: u32 = 0;
    let mut tokens_freed: usize = 0;
    let mut skipped: u32 = 0;

    let scan_end = messages.len() - policy.keep_recent;

    for msg in messages[..scan_end].iter_mut() {
        let MessageContent::Blocks(blocks) = &mut msg.content else {
            continue;
        };

        for block in blocks.iter_mut() {
            let ContentBlock::ToolResult {
                tool_use_id: _,
                content,
                is_error,
            } = block
            else {
                continue;
            };

            // Skip short results
            if content.len() < policy.min_content_chars {
                skipped += 1;
                continue;
            }

            // Skip already-evicted results
            if content.starts_with("[Evicted") || content.starts_with("[Tool result truncated") {
                skipped += 1;
                continue;
            }

            // Compute tokens freed
            let original_tokens = content.len() / 4;

            // Build compact summary
            let status = if *is_error { "ERROR" } else { "OK" };
            let first_line = content
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(policy.summary_max_chars)
                .collect::<String>();

            let summary = format!("[Evicted: ~{original_tokens} tokens] [{status}] {first_line}");
            let summary_tokens = summary.len() / 4;
            tokens_freed += original_tokens.saturating_sub(summary_tokens);

            *content = summary;
            evicted += 1;
        }
    }

    if evicted > 0 {
        tracing::info!(
            evicted,
            tokens_freed,
            skipped,
            keep_recent = policy.keep_recent,
            "tool_result_eviction"
        );
    }

    EvictionResult {
        evicted,
        tokens_freed,
        skipped,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::types::Role;

    fn tool_result_msg(id: &str, content: &str, is_error: bool) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: content.to_string(),
                is_error,
            }]),
        }
    }

    fn text_msg(text: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: MessageContent::Text(text.to_string()),
        }
    }

    #[test]
    fn evict_old_large_result() {
        let policy = EvictionPolicy {
            keep_recent: 2,
            min_content_chars: 100,
            summary_max_chars: 80,
        };
        let large = "x".repeat(2000);
        let mut msgs = vec![
            tool_result_msg("t1", &large, false),
            text_msg("middle"),
            text_msg("recent 1"),
            text_msg("recent 2"),
        ];
        let result = evict_old_tool_results(&mut msgs, &policy);
        assert_eq!(result.evicted, 1);
        assert!(result.tokens_freed > 0);

        // Check the evicted message
        if let MessageContent::Blocks(blocks) = &msgs[0].content {
            if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
                assert!(content.starts_with("[Evicted:"));
                assert!(content.contains("[OK]"));
            } else {
                panic!("Expected ToolResult");
            }
        }
    }

    #[test]
    fn keep_recent_not_evicted() {
        let policy = EvictionPolicy {
            keep_recent: 2,
            min_content_chars: 100,
            ..Default::default()
        };
        let large = "x".repeat(2000);
        let mut msgs = vec![
            text_msg("old"),
            tool_result_msg("t1", &large, false), // In keep_recent window
            text_msg("recent"),
        ];
        let result = evict_old_tool_results(&mut msgs, &policy);
        assert_eq!(result.evicted, 0);
    }

    #[test]
    fn skip_short_results() {
        let policy = EvictionPolicy {
            keep_recent: 1,
            min_content_chars: 500,
            ..Default::default()
        };
        let small = "short result";
        let mut msgs = vec![tool_result_msg("t1", small, false), text_msg("recent")];
        let result = evict_old_tool_results(&mut msgs, &policy);
        assert_eq!(result.evicted, 0);
        assert_eq!(result.skipped, 1);
    }

    #[test]
    fn preserves_tool_use_id_and_is_error() {
        let policy = EvictionPolicy {
            keep_recent: 1,
            min_content_chars: 100,
            ..Default::default()
        };
        let large = "x".repeat(2000);
        let mut msgs = vec![
            tool_result_msg("my_id_123", &large, true),
            text_msg("recent"),
        ];
        evict_old_tool_results(&mut msgs, &policy);

        if let MessageContent::Blocks(blocks) = &msgs[0].content {
            if let ContentBlock::ToolResult {
                tool_use_id,
                is_error,
                content,
            } = &blocks[0]
            {
                assert_eq!(tool_use_id, "my_id_123");
                assert!(*is_error);
                assert!(content.contains("[ERROR]"));
            } else {
                panic!("Expected ToolResult");
            }
        }
    }

    #[test]
    fn skip_already_evicted() {
        let policy = EvictionPolicy {
            keep_recent: 1,
            min_content_chars: 10,
            ..Default::default()
        };
        let mut msgs = vec![
            tool_result_msg("t1", "[Evicted: ~500 tokens] [OK] something", false),
            text_msg("recent"),
        ];
        let result = evict_old_tool_results(&mut msgs, &policy);
        assert_eq!(result.evicted, 0);
        assert_eq!(result.skipped, 1);
    }

    #[test]
    fn few_messages_noop() {
        let policy = EvictionPolicy {
            keep_recent: 6,
            ..Default::default()
        };
        let mut msgs = vec![text_msg("only one")];
        let result = evict_old_tool_results(&mut msgs, &policy);
        assert_eq!(result.evicted, 0);
    }

    #[test]
    fn error_status_preserved_in_summary() {
        let policy = EvictionPolicy {
            keep_recent: 1,
            min_content_chars: 100,
            ..Default::default()
        };
        let large = "Error: permission denied\n".repeat(50);
        let mut msgs = vec![tool_result_msg("t1", &large, true), text_msg("recent")];
        evict_old_tool_results(&mut msgs, &policy);
        if let MessageContent::Blocks(blocks) = &msgs[0].content {
            if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
                assert!(content.contains("[ERROR]"));
                assert!(content.contains("Error: permission denied"));
            }
        }
    }
}
