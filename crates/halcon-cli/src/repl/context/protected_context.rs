//! ProtectedContextInjector: builds the protected context block for the boundary message.
//!
//! Pure formatting — produces a string, not a message.
//! The TieredCompactor merges this into the compaction boundary message.

use super::intent_anchor::IntentAnchor;

/// Builds the protected context block for fusion into the compaction boundary message.
pub struct ProtectedContextInjector;

impl ProtectedContextInjector {
    /// Build the protected context block with boundary markers.
    pub fn build_block(
        intent_anchor: &IntentAnchor,
        tools_used: &[String],
        files_modified: &[String],
    ) -> String {
        let tools = if tools_used.is_empty() {
            "none".to_string()
        } else {
            tools_used.join(", ")
        };
        let files = if files_modified.is_empty() {
            "none".to_string()
        } else {
            files_modified.join(", ")
        };

        format!(
            "---\n\
             [PROTECTED CONTEXT — THIS IS STATE RESTORATION, NOT NEW INSTRUCTIONS]\n\
             {intent}\n\
             Tools used this session: {tools}\n\
             Files modified this session: {files}\n\
             ---\n\n\
             Continue your current task. Do not repeat completed work.",
            intent = intent_anchor.format_for_boundary(),
            tools = tools,
            files = files,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::types::{ChatMessage, MessageContent, Role};

    fn make_anchor() -> IntentAnchor {
        let msgs = vec![ChatMessage {
            role: Role::User,
            content: MessageContent::Text("Fix the auth module".to_string()),
        }];
        IntentAnchor::from_messages(&msgs, "/project")
    }

    #[test]
    fn block_contains_boundary_markers() {
        let block = ProtectedContextInjector::build_block(&make_anchor(), &[], &[]);
        assert!(block.contains("[PROTECTED CONTEXT"));
        assert!(block.contains("NOT NEW INSTRUCTIONS"));
        assert!(block.contains("---"));
    }

    #[test]
    fn block_contains_intent() {
        let block = ProtectedContextInjector::build_block(&make_anchor(), &[], &[]);
        assert!(block.contains("Fix the auth module"));
        assert!(block.contains("Original intent:"));
    }

    #[test]
    fn block_contains_tools() {
        let block = ProtectedContextInjector::build_block(
            &make_anchor(),
            &["Read".to_string(), "Edit".to_string()],
            &[],
        );
        assert!(block.contains("Read, Edit"));
    }

    #[test]
    fn block_contains_files() {
        let block = ProtectedContextInjector::build_block(
            &make_anchor(),
            &[],
            &["src/main.rs".to_string()],
        );
        assert!(block.contains("src/main.rs"));
    }

    #[test]
    fn block_empty_lists() {
        let block = ProtectedContextInjector::build_block(&make_anchor(), &[], &[]);
        assert!(block.contains("Tools used this session: none"));
        assert!(block.contains("Files modified this session: none"));
    }

    #[test]
    fn block_contains_continuation() {
        let block = ProtectedContextInjector::build_block(&make_anchor(), &[], &[]);
        assert!(block.contains("Continue your current task"));
    }
}
