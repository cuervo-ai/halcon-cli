//! Message sequence validation for tool use protocol compliance.
//!
//! Ensures that every `ToolResult` block in a message sequence has a
//! matching `ToolUse` block with the same ID in a preceding `Assistant`
//! message. This is a hard invariant required by all providers (Anthropic,
//! OpenAI, Gemini, DeepSeek) — violations cause 400 errors.

use std::collections::HashSet;

use super::{ChatMessage, ContentBlock, MessageContent, Role};

/// A protocol violation found in a message sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolViolation {
    /// A `ToolResult` references a `tool_use_id` that was never seen
    /// in any preceding `Assistant` message's `ToolUse` block.
    OrphanedToolResult {
        message_index: usize,
        tool_use_id: String,
    },
    /// A `ToolUse` ID appears more than once across `Assistant` messages.
    DuplicateToolUseId {
        message_index: usize,
        tool_use_id: String,
    },
    /// A `ToolResult` block appears in a non-`User` message.
    ToolResultWrongRole {
        message_index: usize,
        role: Role,
    },
    /// A `ToolUse` block appears in a non-`Assistant` message.
    ToolUseWrongRole {
        message_index: usize,
        role: Role,
    },
    /// A `ToolUse` ID was never answered by any `ToolResult`.
    UnresolvedToolUse {
        message_index: usize,
        tool_use_id: String,
    },
}

impl std::fmt::Display for ProtocolViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolViolation::OrphanedToolResult { message_index, tool_use_id } => {
                write!(f, "msg[{message_index}]: orphaned ToolResult with tool_use_id '{tool_use_id}' — no matching ToolUse in preceding Assistant message")
            }
            ProtocolViolation::DuplicateToolUseId { message_index, tool_use_id } => {
                write!(f, "msg[{message_index}]: duplicate ToolUse id '{tool_use_id}'")
            }
            ProtocolViolation::ToolResultWrongRole { message_index, role } => {
                write!(f, "msg[{message_index}]: ToolResult in {role:?} message (expected User)")
            }
            ProtocolViolation::ToolUseWrongRole { message_index, role } => {
                write!(f, "msg[{message_index}]: ToolUse in {role:?} message (expected Assistant)")
            }
            ProtocolViolation::UnresolvedToolUse { message_index, tool_use_id } => {
                write!(f, "msg[{message_index}]: ToolUse id '{tool_use_id}' has no matching ToolResult in subsequent messages")
            }
        }
    }
}

/// Validate a message sequence for tool use protocol compliance.
///
/// Returns an empty vec if the sequence is valid. Otherwise returns all
/// violations found in a single pass.
///
/// # Invariants checked
///
/// 1. Every `ToolResult.tool_use_id` references an `id` from a preceding
///    `ToolUse` block in an `Assistant` message.
/// 2. No duplicate `ToolUse` IDs across the entire sequence.
/// 3. `ToolUse` blocks only appear in `Assistant` messages.
/// 4. `ToolResult` blocks only appear in `User` messages.
///
/// # Note on unresolved tool uses
///
/// The last `Assistant` message in a sequence may have `ToolUse` blocks
/// whose results haven't been appended yet (they're about to be executed).
/// Pass `allow_trailing_tool_use = true` to suppress `UnresolvedToolUse`
/// for the final assistant message.
pub fn validate_message_sequence(
    messages: &[ChatMessage],
    allow_trailing_tool_use: bool,
) -> Vec<ProtocolViolation> {
    let mut violations = Vec::new();
    let mut declared_tool_use_ids: HashSet<String> = HashSet::new();
    let mut resolved_tool_use_ids: HashSet<String> = HashSet::new();
    // Track (message_index, id) for unresolved check.
    let mut tool_use_declarations: Vec<(usize, String)> = Vec::new();

    for (idx, msg) in messages.iter().enumerate() {
        let blocks = match &msg.content {
            MessageContent::Blocks(blocks) => blocks,
            MessageContent::Text(_) => continue,
        };

        for block in blocks {
            match block {
                ContentBlock::ToolUse { id, .. } => {
                    // Check role.
                    if msg.role != Role::Assistant {
                        violations.push(ProtocolViolation::ToolUseWrongRole {
                            message_index: idx,
                            role: msg.role,
                        });
                    }
                    // Check duplicate.
                    if !declared_tool_use_ids.insert(id.clone()) {
                        violations.push(ProtocolViolation::DuplicateToolUseId {
                            message_index: idx,
                            tool_use_id: id.clone(),
                        });
                    }
                    tool_use_declarations.push((idx, id.clone()));
                }
                ContentBlock::ToolResult { tool_use_id, .. } => {
                    // Check role.
                    if msg.role != Role::User {
                        violations.push(ProtocolViolation::ToolResultWrongRole {
                            message_index: idx,
                            role: msg.role,
                        });
                    }
                    // Check matching ToolUse exists.
                    if !declared_tool_use_ids.contains(tool_use_id) {
                        violations.push(ProtocolViolation::OrphanedToolResult {
                            message_index: idx,
                            tool_use_id: tool_use_id.clone(),
                        });
                    }
                    resolved_tool_use_ids.insert(tool_use_id.clone());
                }
                ContentBlock::Text { .. } => {}
            }
        }
    }

    // Check for unresolved tool uses (declared but never answered).
    if !allow_trailing_tool_use || tool_use_declarations.is_empty() {
        for (decl_idx, id) in &tool_use_declarations {
            if !resolved_tool_use_ids.contains(id) {
                violations.push(ProtocolViolation::UnresolvedToolUse {
                    message_index: *decl_idx,
                    tool_use_id: id.clone(),
                });
            }
        }
    } else {
        // allow_trailing_tool_use: skip unresolved check only for the LAST
        // assistant message that has tool uses.
        let last_assistant_tool_idx = tool_use_declarations.last().map(|(idx, _)| *idx);
        for (decl_idx, id) in &tool_use_declarations {
            if !resolved_tool_use_ids.contains(id) {
                if Some(*decl_idx) == last_assistant_tool_idx {
                    continue; // Allow trailing unresolved.
                }
                violations.push(ProtocolViolation::UnresolvedToolUse {
                    message_index: *decl_idx,
                    tool_use_id: id.clone(),
                });
            }
        }
    }

    violations
}

/// Strip orphaned `ToolResult` blocks from a message sequence (auto-repair).
///
/// Returns a new message sequence where any `ToolResult` block that references
/// a nonexistent `ToolUse` is removed. Messages that become empty after
/// stripping are also removed.
///
/// This is a best-effort repair — it should only be used as a fallback when
/// validation fails and we need to send something to the provider.
pub fn strip_orphaned_tool_results(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    // Collect all declared ToolUse IDs.
    let mut declared: HashSet<String> = HashSet::new();
    for msg in messages {
        if let MessageContent::Blocks(blocks) = &msg.content {
            for block in blocks {
                if let ContentBlock::ToolUse { id, .. } = block {
                    declared.insert(id.clone());
                }
            }
        }
    }

    // Filter out orphaned ToolResult blocks.
    let mut result = Vec::with_capacity(messages.len());
    for msg in messages {
        match &msg.content {
            MessageContent::Text(_) => result.push(msg.clone()),
            MessageContent::Blocks(blocks) => {
                let filtered: Vec<ContentBlock> = blocks
                    .iter()
                    .filter(|block| {
                        if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                            declared.contains(tool_use_id)
                        } else {
                            true
                        }
                    })
                    .cloned()
                    .collect();

                if filtered.is_empty() {
                    // Message had only orphaned ToolResults — skip entirely.
                    continue;
                }

                result.push(ChatMessage {
                    role: msg.role,
                    content: MessageContent::Blocks(filtered),
                });
            }
        }
    }

    result
}

/// Extract all ToolUse IDs declared in a message sequence.
pub fn extract_tool_use_ids(messages: &[ChatMessage]) -> HashSet<String> {
    let mut ids = HashSet::new();
    for msg in messages {
        if let MessageContent::Blocks(blocks) = &msg.content {
            for block in blocks {
                if let ContentBlock::ToolUse { id, .. } = block {
                    ids.insert(id.clone());
                }
            }
        }
    }
    ids
}

/// Extract all ToolResult tool_use_ids referenced in a message sequence.
pub fn extract_tool_result_ids(messages: &[ChatMessage]) -> HashSet<String> {
    let mut ids = HashSet::new();
    for msg in messages {
        if let MessageContent::Blocks(blocks) = &msg.content {
            for block in blocks {
                if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                    ids.insert(tool_use_id.clone());
                }
            }
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text_msg(role: Role, text: &str) -> ChatMessage {
        ChatMessage {
            role,
            content: MessageContent::Text(text.to_string()),
        }
    }

    fn assistant_tool_use(id: &str, name: &str) -> ChatMessage {
        ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input: json!({}),
            }]),
        }
    }

    fn user_tool_result(tool_use_id: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: content.to_string(),
                is_error: false,
            }]),
        }
    }

    fn assistant_multi_tool(ids: &[(&str, &str)]) -> ChatMessage {
        let blocks = ids
            .iter()
            .map(|(id, name)| ContentBlock::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input: json!({}),
            })
            .collect();
        ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Blocks(blocks),
        }
    }

    fn user_multi_result(results: &[(&str, &str)]) -> ChatMessage {
        let blocks = results
            .iter()
            .map(|(id, content)| ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: content.to_string(),
                is_error: false,
            })
            .collect();
        ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(blocks),
        }
    }

    // ── Valid sequences ──

    #[test]
    fn valid_simple_tool_round() {
        let msgs = vec![
            text_msg(Role::User, "read file"),
            assistant_tool_use("t1", "file_read"),
            user_tool_result("t1", "contents"),
            text_msg(Role::Assistant, "done"),
        ];
        let v = validate_message_sequence(&msgs, false);
        assert!(v.is_empty(), "Expected no violations, got: {v:?}");
    }

    #[test]
    fn valid_multi_tool_round() {
        let msgs = vec![
            text_msg(Role::User, "read two files"),
            assistant_multi_tool(&[("t1", "file_read"), ("t2", "file_read")]),
            user_multi_result(&[("t1", "content1"), ("t2", "content2")]),
            text_msg(Role::Assistant, "got both"),
        ];
        let v = validate_message_sequence(&msgs, false);
        assert!(v.is_empty(), "Expected no violations, got: {v:?}");
    }

    #[test]
    fn valid_consecutive_tool_rounds() {
        let msgs = vec![
            text_msg(Role::User, "step 1"),
            assistant_tool_use("t1", "bash"),
            user_tool_result("t1", "ok"),
            assistant_tool_use("t2", "file_read"),
            user_tool_result("t2", "data"),
            text_msg(Role::Assistant, "all done"),
        ];
        let v = validate_message_sequence(&msgs, false);
        assert!(v.is_empty(), "Expected no violations, got: {v:?}");
    }

    #[test]
    fn valid_text_only_conversation() {
        let msgs = vec![
            text_msg(Role::User, "hello"),
            text_msg(Role::Assistant, "hi"),
        ];
        let v = validate_message_sequence(&msgs, false);
        assert!(v.is_empty());
    }

    #[test]
    fn valid_trailing_tool_use_allowed() {
        // After the model returns ToolUse, we haven't added the result yet.
        let msgs = vec![
            text_msg(Role::User, "run test"),
            assistant_tool_use("t1", "bash"),
        ];
        let v = validate_message_sequence(&msgs, true);
        assert!(v.is_empty(), "Trailing tool use should be allowed, got: {v:?}");
    }

    // ── Invalid sequences ──

    #[test]
    fn orphaned_tool_result_detected() {
        let msgs = vec![
            text_msg(Role::User, "hello"),
            user_tool_result("nonexistent_id", "some content"),
        ];
        let v = validate_message_sequence(&msgs, false);
        assert_eq!(v.len(), 1);
        assert!(matches!(
            &v[0],
            ProtocolViolation::OrphanedToolResult { tool_use_id, .. }
            if tool_use_id == "nonexistent_id"
        ));
    }

    #[test]
    fn duplicate_tool_use_id_detected() {
        let msgs = vec![
            text_msg(Role::User, "go"),
            assistant_tool_use("dup_id", "bash"),
            user_tool_result("dup_id", "ok"),
            assistant_tool_use("dup_id", "bash"),
            user_tool_result("dup_id", "ok again"),
        ];
        let v = validate_message_sequence(&msgs, false);
        assert!(v.iter().any(|v| matches!(
            v,
            ProtocolViolation::DuplicateToolUseId { tool_use_id, .. }
            if tool_use_id == "dup_id"
        )));
    }

    #[test]
    fn tool_result_wrong_role_detected() {
        let msgs = vec![
            text_msg(Role::User, "go"),
            assistant_tool_use("t1", "bash"),
            // Wrong: tool result in assistant message
            ChatMessage {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                    tool_use_id: "t1".to_string(),
                    content: "result".to_string(),
                    is_error: false,
                }]),
            },
        ];
        let v = validate_message_sequence(&msgs, false);
        assert!(v.iter().any(|v| matches!(v, ProtocolViolation::ToolResultWrongRole { .. })));
    }

    #[test]
    fn tool_use_wrong_role_detected() {
        let msgs = vec![
            // Wrong: tool use in user message
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "bash".to_string(),
                    input: json!({}),
                }]),
            },
        ];
        let v = validate_message_sequence(&msgs, true);
        assert!(v.iter().any(|v| matches!(v, ProtocolViolation::ToolUseWrongRole { .. })));
    }

    #[test]
    fn unresolved_tool_use_detected_when_not_trailing() {
        let msgs = vec![
            text_msg(Role::User, "go"),
            assistant_tool_use("t1", "bash"),
            // Missing: user_tool_result("t1", ...)
            text_msg(Role::User, "next question"),
            text_msg(Role::Assistant, "answer"),
        ];
        let v = validate_message_sequence(&msgs, false);
        assert!(v.iter().any(|v| matches!(
            v,
            ProtocolViolation::UnresolvedToolUse { tool_use_id, .. }
            if tool_use_id == "t1"
        )));
    }

    #[test]
    fn unresolved_mid_conversation_detected_even_with_trailing_allowed() {
        let msgs = vec![
            text_msg(Role::User, "first"),
            assistant_tool_use("t1", "bash"),
            // Missing result for t1
            text_msg(Role::User, "second"),
            assistant_tool_use("t2", "bash"), // This is trailing — OK
        ];
        let v = validate_message_sequence(&msgs, true);
        // t1 is mid-conversation unresolved (not trailing), should be flagged.
        assert!(v.iter().any(|v| matches!(
            v,
            ProtocolViolation::UnresolvedToolUse { tool_use_id, .. }
            if tool_use_id == "t1"
        )));
        // t2 is trailing, should NOT be flagged.
        assert!(!v.iter().any(|v| matches!(
            v,
            ProtocolViolation::UnresolvedToolUse { tool_use_id, .. }
            if tool_use_id == "t2"
        )));
    }

    // ── Strip orphans ──

    #[test]
    fn strip_orphaned_removes_orphan() {
        let msgs = vec![
            text_msg(Role::User, "hello"),
            user_tool_result("nonexistent", "orphan"),
            text_msg(Role::Assistant, "reply"),
        ];
        let cleaned = strip_orphaned_tool_results(&msgs);
        assert_eq!(cleaned.len(), 2); // Orphan message removed entirely
        assert_eq!(cleaned[0].content.as_text().unwrap(), "hello");
        assert_eq!(cleaned[1].content.as_text().unwrap(), "reply");
    }

    #[test]
    fn strip_orphaned_preserves_valid() {
        let msgs = vec![
            text_msg(Role::User, "go"),
            assistant_tool_use("t1", "bash"),
            user_tool_result("t1", "ok"),
        ];
        let cleaned = strip_orphaned_tool_results(&msgs);
        assert_eq!(cleaned.len(), 3);
    }

    #[test]
    fn strip_orphaned_partial_block_removal() {
        // Message with both valid and orphaned results
        let msgs = vec![
            text_msg(Role::User, "go"),
            assistant_tool_use("t1", "bash"),
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![
                    ContentBlock::ToolResult {
                        tool_use_id: "t1".to_string(),
                        content: "valid".to_string(),
                        is_error: false,
                    },
                    ContentBlock::ToolResult {
                        tool_use_id: "orphan".to_string(),
                        content: "orphan".to_string(),
                        is_error: false,
                    },
                ]),
            },
        ];
        let cleaned = strip_orphaned_tool_results(&msgs);
        assert_eq!(cleaned.len(), 3);
        if let MessageContent::Blocks(blocks) = &cleaned[2].content {
            assert_eq!(blocks.len(), 1); // Only valid result remains
            if let ContentBlock::ToolResult { tool_use_id, .. } = &blocks[0] {
                assert_eq!(tool_use_id, "t1");
            }
        }
    }

    // ── Extract helpers ──

    #[test]
    fn extract_tool_use_ids_basic() {
        let msgs = vec![
            assistant_multi_tool(&[("t1", "bash"), ("t2", "file_read")]),
            text_msg(Role::User, "text"),
        ];
        let ids = extract_tool_use_ids(&msgs);
        assert_eq!(ids.len(), 2);
        assert!(ids.contains("t1"));
        assert!(ids.contains("t2"));
    }

    #[test]
    fn extract_tool_result_ids_basic() {
        let msgs = vec![
            user_multi_result(&[("t1", "ok"), ("t2", "ok")]),
        ];
        let ids = extract_tool_result_ids(&msgs);
        assert_eq!(ids.len(), 2);
        assert!(ids.contains("t1"));
        assert!(ids.contains("t2"));
    }

    #[test]
    fn empty_messages_valid() {
        let v = validate_message_sequence(&[], false);
        assert!(v.is_empty());
    }

    #[test]
    fn display_violation_messages() {
        let v = ProtocolViolation::OrphanedToolResult {
            message_index: 3,
            tool_use_id: "t1".to_string(),
        };
        let s = v.to_string();
        assert!(s.contains("msg[3]"));
        assert!(s.contains("t1"));
        assert!(s.contains("orphaned"));
    }
}
