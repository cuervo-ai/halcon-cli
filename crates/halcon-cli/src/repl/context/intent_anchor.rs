//! IntentAnchor: immutable capture of the user's original intent.
//!
//! Created once at loop start, survives all compactions.
//! Used by CompactionSummaryBuilder and ProtectedContextInjector.

use halcon_core::types::{ChatMessage, MessageContent, Role};
use std::time::Instant;

/// Immutable anchor capturing the user's original intent.
#[derive(Clone, Debug)]
pub struct IntentAnchor {
    pub user_message: String,
    pub task_summary: String,
    pub mentioned_files: Vec<String>,
    pub working_dir: String,
    pub created_at: Instant,
}

impl IntentAnchor {
    /// Create from the initial message list. Uses the first `Role::User` message.
    pub fn from_messages(messages: &[ChatMessage], working_dir: &str) -> Self {
        let user_text = messages
            .iter()
            .find(|m| m.role == Role::User)
            .and_then(extract_text)
            .unwrap_or_default();

        let task_summary: String = user_text.chars().take(500).collect();
        let mentioned_files = extract_file_references(&user_text);

        Self {
            user_message: if user_text.is_empty() {
                "[no user message found]".to_string()
            } else {
                user_text
            },
            task_summary,
            mentioned_files,
            working_dir: working_dir.to_string(),
            created_at: Instant::now(),
        }
    }

    /// Format for inclusion in the compaction boundary message.
    pub fn format_for_boundary(&self) -> String {
        let files = if self.mentioned_files.is_empty() {
            "none".to_string()
        } else {
            self.mentioned_files.join(", ")
        };
        format!(
            "Original intent: {}\nTask: {}\nWorking directory: {}\nKey files: {}",
            self.user_message, self.task_summary, self.working_dir, files
        )
    }
}

fn extract_text(msg: &ChatMessage) -> Option<String> {
    match &msg.content {
        MessageContent::Text(t) if !t.is_empty() => Some(t.clone()),
        MessageContent::Blocks(blocks) => {
            let texts: Vec<&str> = blocks
                .iter()
                .filter_map(|b| match b {
                    halcon_core::types::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            if texts.is_empty() {
                None
            } else {
                Some(texts.join("\n"))
            }
        }
        _ => None,
    }
}

/// Simple heuristic extraction of file paths from user text.
fn extract_file_references(text: &str) -> Vec<String> {
    let re = regex::Regex::new(r#"(?:^|[\s"'`(])([./~]?[\w./-]+\.\w{1,10})\b"#).unwrap();
    let mut files: Vec<String> = Vec::new();
    for cap in re.captures_iter(text) {
        let path = cap[1].to_string();
        // Filter out obvious non-paths
        if path.len() >= 3 && !files.contains(&path) {
            files.push(path);
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::types::{ChatMessage, MessageContent, Role};

    fn user_msg(text: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: MessageContent::Text(text.to_string()),
        }
    }
    fn system_msg(text: &str) -> ChatMessage {
        ChatMessage {
            role: Role::System,
            content: MessageContent::Text(text.to_string()),
        }
    }
    fn assistant_msg(text: &str) -> ChatMessage {
        ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Text(text.to_string()),
        }
    }

    #[test]
    fn from_messages_extracts_first_user() {
        let msgs = vec![
            system_msg("You are helpful"),
            user_msg("Fix the bug in src/main.rs"),
            user_msg("Also check tests"),
        ];
        let anchor = IntentAnchor::from_messages(&msgs, "/project");
        assert_eq!(anchor.user_message, "Fix the bug in src/main.rs");
        assert_eq!(anchor.working_dir, "/project");
    }

    #[test]
    fn from_messages_no_user_message() {
        let msgs = vec![system_msg("System only")];
        let anchor = IntentAnchor::from_messages(&msgs, "/tmp");
        assert_eq!(anchor.user_message, "[no user message found]");
        assert!(anchor.mentioned_files.is_empty());
    }

    #[test]
    fn from_messages_multi_user_uses_first() {
        let msgs = vec![
            user_msg("First task"),
            assistant_msg("OK"),
            user_msg("Second task"),
        ];
        let anchor = IntentAnchor::from_messages(&msgs, "/home");
        assert_eq!(anchor.user_message, "First task");
    }

    #[test]
    fn mentioned_files_extraction() {
        let msgs = vec![user_msg(
            "Please fix src/main.rs and update tests/test_foo.py",
        )];
        let anchor = IntentAnchor::from_messages(&msgs, "/project");
        assert!(anchor.mentioned_files.contains(&"src/main.rs".to_string()));
        assert!(anchor
            .mentioned_files
            .contains(&"tests/test_foo.py".to_string()));
    }

    #[test]
    fn format_for_boundary_contains_all_fields() {
        let msgs = vec![user_msg("Fix bug in app.rs")];
        let anchor = IntentAnchor::from_messages(&msgs, "/work");
        let output = anchor.format_for_boundary();
        assert!(output.contains("Fix bug in app.rs"));
        assert!(output.contains("/work"));
        assert!(output.contains("Original intent:"));
        assert!(output.contains("Task:"));
    }
}
