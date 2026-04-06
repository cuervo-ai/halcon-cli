//! CompactionSummaryBuilder: constructs the 9-section semantic compaction prompt.
//!
//! Pure string builder — does not invoke the LLM. That is TieredCompactor's job.

use halcon_context::estimate_tokens;
use halcon_core::types::{ChatMessage, ContentBlock, MessageContent, Role};

use super::intent_anchor::IntentAnchor;

/// Builds the semantic compaction prompt following a 9-section structure.
pub struct CompactionSummaryBuilder;

impl CompactionSummaryBuilder {
    /// Build the prompt for the LLM to generate a structured summary.
    ///
    /// `messages[..len - keep_count]` are the messages to summarize.
    /// The prompt includes the intent anchor and a token cap instruction.
    pub fn build_prompt(
        messages: &[ChatMessage],
        intent_anchor: &IntentAnchor,
        keep_count: usize,
        max_summary_tokens: usize,
    ) -> String {
        let end = messages.len().saturating_sub(keep_count);
        let to_summarize = &messages[..end];
        let context = Self::format_messages_for_prompt(to_summarize);

        format!(
            "You are summarizing a conversation to preserve continuity after context compaction.\n\n\
             ORIGINAL USER INTENT:\n{intent}\n\n\
             Produce a structured summary with these sections:\n\
             1. **Primary Request and Intent** — The user's explicit requests and goals. Include direct quotes from user messages.\n\
             2. **Key Technical Context** — Technologies, frameworks, patterns discussed.\n\
             3. **Files and Code** — Specific files examined or modified, with exact paths.\n\
             4. **Errors and Fixes** — Errors encountered and how they were resolved. Include error codes verbatim.\n\
             5. **Decisions Made** — Key decisions and their rationale.\n\
             6. **User Feedback** — All user corrections, refinements, or direction changes.\n\
             7. **Pending Tasks** — Work explicitly requested but not yet completed.\n\
             8. **Current State** — What was being done immediately before this summary.\n\
             9. **Next Step** — The single most important next action, with direct quote from the user's most recent request.\n\n\
             CRITICAL RULES:\n\
             - Preserve ALL user messages (not tool results) — they define intent.\n\
             - Include exact file paths, not descriptions.\n\
             - Include error codes and messages verbatim.\n\
             - Keep the summary under {max_tokens} tokens.\n\
             - Do NOT call any tools. Respond with text only.\n\n\
             CONVERSATION TO SUMMARIZE:\n{context}",
            intent = intent_anchor.format_for_boundary(),
            max_tokens = max_summary_tokens,
            context = context,
        )
    }

    /// Format messages for inclusion in the summary prompt.
    /// Truncates large tool results and text blocks to keep prompt manageable.
    pub fn format_messages_for_prompt(messages: &[ChatMessage]) -> String {
        let mut parts = Vec::new();
        for msg in messages {
            let role = match msg.role {
                Role::User => "USER",
                Role::Assistant => "ASSISTANT",
                Role::System => "SYSTEM",
            };
            let content = match &msg.content {
                MessageContent::Text(t) => {
                    if t.len() > 2000 {
                        format!("{}...[truncated, {} chars total]", &t[..500], t.len())
                    } else {
                        t.clone()
                    }
                }
                MessageContent::Blocks(blocks) => Self::format_blocks(blocks),
            };
            if !content.is_empty() {
                parts.push(format!("[{}]: {}", role, content));
            }
        }
        parts.join("\n\n")
    }

    fn format_blocks(blocks: &[ContentBlock]) -> String {
        blocks
            .iter()
            .map(|b| match b {
                ContentBlock::Text { text } => {
                    if text.len() > 500 {
                        format!("{}...[truncated]", &text[..500])
                    } else {
                        text.clone()
                    }
                }
                ContentBlock::ToolUse { name, id, .. } => {
                    format!("[Tool call: {} (id: {})]", name, id)
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => {
                    let preview = if content.len() > 200 {
                        format!("{}...[truncated]", &content[..200])
                    } else {
                        content.clone()
                    };
                    format!("[Tool result for {}: {}]", tool_use_id, preview)
                }
                _ => "[other content]".to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Estimate the tokens of the prompt itself for budget accounting.
    pub fn estimate_prompt_tokens(
        messages: &[ChatMessage],
        intent_anchor: &IntentAnchor,
        keep_count: usize,
        max_summary_tokens: usize,
    ) -> usize {
        let prompt = Self::build_prompt(messages, intent_anchor, keep_count, max_summary_tokens);
        estimate_tokens(&prompt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::types::{ChatMessage, ContentBlock, MessageContent, Role};

    fn user_msg(text: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: MessageContent::Text(text.to_string()),
        }
    }
    fn assistant_msg(text: &str) -> ChatMessage {
        ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Text(text.to_string()),
        }
    }

    fn make_anchor() -> IntentAnchor {
        IntentAnchor::from_messages(&[user_msg("Fix the build")], "/project")
    }

    #[test]
    fn prompt_contains_9_sections() {
        let msgs = vec![
            user_msg("Fix bug"),
            assistant_msg("Looking at it"),
            user_msg("Also check tests"),
        ];
        let prompt = CompactionSummaryBuilder::build_prompt(&msgs, &make_anchor(), 1, 2000);
        assert!(prompt.contains("Primary Request and Intent"));
        assert!(prompt.contains("Key Technical Context"));
        assert!(prompt.contains("Files and Code"));
        assert!(prompt.contains("Errors and Fixes"));
        assert!(prompt.contains("Decisions Made"));
        assert!(prompt.contains("User Feedback"));
        assert!(prompt.contains("Pending Tasks"));
        assert!(prompt.contains("Current State"));
        assert!(prompt.contains("Next Step"));
    }

    #[test]
    fn prompt_includes_intent_anchor() {
        let msgs = vec![user_msg("Hello"), assistant_msg("Hi")];
        let prompt = CompactionSummaryBuilder::build_prompt(&msgs, &make_anchor(), 0, 2000);
        assert!(prompt.contains("Fix the build"));
        assert!(prompt.contains("Original intent:"));
    }

    #[test]
    fn prompt_truncates_long_tool_results() {
        let long_content = "x".repeat(500);
        let msgs = vec![ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "t1".to_string(),
                content: long_content,
                is_error: false,
            }]),
        }];
        let prompt = CompactionSummaryBuilder::build_prompt(&msgs, &make_anchor(), 0, 2000);
        assert!(prompt.contains("truncated"));
    }

    #[test]
    fn prompt_respects_keep_count() {
        let msgs = vec![
            user_msg("first"),
            assistant_msg("response"),
            user_msg("last message"),
        ];
        // keep_count = 1, so only first 2 messages should be in summary
        let prompt = CompactionSummaryBuilder::build_prompt(&msgs, &make_anchor(), 1, 2000);
        assert!(prompt.contains("first"));
        assert!(!prompt.contains("last message"));
    }

    #[test]
    fn prompt_includes_token_cap() {
        let msgs = vec![user_msg("test")];
        let prompt = CompactionSummaryBuilder::build_prompt(&msgs, &make_anchor(), 0, 3000);
        assert!(prompt.contains("3000"));
    }
}
