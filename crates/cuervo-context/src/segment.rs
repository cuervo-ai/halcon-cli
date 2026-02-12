//! Context segment: a compacted unit of conversation context.
//!
//! Segments are the fundamental unit of storage in L1-L4 tiers.
//! Each segment represents a contiguous range of conversation rounds,
//! compressed into a summary with extracted metadata.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A segment of compacted conversation context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSegment {
    /// First round in this segment.
    pub round_start: u32,
    /// Last round in this segment.
    pub round_end: u32,
    /// Human-readable summary of the segment content.
    pub summary: String,
    /// Key decisions made in this segment.
    pub decisions: Vec<String>,
    /// Files modified during this segment.
    pub files_modified: Vec<String>,
    /// Tools used during this segment.
    pub tools_used: Vec<String>,
    /// Pre-computed token estimate for this segment.
    pub token_estimate: u32,
    /// When this segment was created.
    pub created_at: DateTime<Utc>,
}

impl ContextSegment {
    /// Create a new segment from a round range and summary.
    pub fn new(round_start: u32, round_end: u32, summary: String) -> Self {
        let token_estimate = crate::assembler::estimate_tokens(&summary) as u32;
        Self {
            round_start,
            round_end,
            summary,
            decisions: Vec::new(),
            files_modified: Vec::new(),
            tools_used: Vec::new(),
            token_estimate,
            created_at: Utc::now(),
        }
    }

    /// Merge two segments into one (combines summaries and metadata).
    pub fn merge(a: &ContextSegment, b: &ContextSegment) -> ContextSegment {
        let summary = format!("{} {}", a.summary, b.summary);
        let mut decisions = a.decisions.clone();
        decisions.extend(b.decisions.iter().cloned());

        let mut files = a.files_modified.clone();
        for f in &b.files_modified {
            if !files.contains(f) {
                files.push(f.clone());
            }
        }

        let mut tools = a.tools_used.clone();
        for t in &b.tools_used {
            if !tools.contains(t) {
                tools.push(t.clone());
            }
        }

        let token_estimate = crate::assembler::estimate_tokens(&summary) as u32
            + crate::assembler::estimate_tokens(&decisions.join(" ")) as u32
            + crate::assembler::estimate_tokens(&files.join(" ")) as u32;

        ContextSegment {
            round_start: a.round_start.min(b.round_start),
            round_end: a.round_end.max(b.round_end),
            summary,
            decisions,
            files_modified: files,
            tools_used: tools,
            token_estimate,
            created_at: Utc::now(),
        }
    }

    /// Total estimated tokens for this segment (summary + metadata).
    pub fn total_tokens(&self) -> u32 {
        self.token_estimate
    }

    /// Format this segment as a context string for inclusion in model prompt.
    pub fn to_context_string(&self) -> String {
        let mut parts = Vec::new();
        parts.push(format!(
            "[Rounds {}-{}] {}",
            self.round_start, self.round_end, self.summary
        ));
        if !self.decisions.is_empty() {
            parts.push(format!("Decisions: {}", self.decisions.join(", ")));
        }
        if !self.files_modified.is_empty() {
            parts.push(format!("Files: {}", self.files_modified.join(", ")));
        }
        if !self.tools_used.is_empty() {
            parts.push(format!("Tools: {}", self.tools_used.join(", ")));
        }
        parts.join("\n")
    }
}

/// Extract a segment from a ChatMessage (local extraction, no LLM call).
pub fn extract_segment_from_message(
    msg: &cuervo_core::types::ChatMessage,
    round: u32,
) -> ContextSegment {
    use cuervo_core::types::{ContentBlock, MessageContent};

    match &msg.content {
        MessageContent::Text(t) => {
            let decisions = extract_decisions(t);
            let files = extract_file_paths(t);
            let summary = truncate_text(t, 500);
            let mut seg = ContextSegment::new(round, round, summary);
            seg.decisions = decisions;
            seg.files_modified = files;
            seg
        }
        MessageContent::Blocks(blocks) => {
            let mut tool_names = Vec::new();
            let mut outcomes = Vec::new();
            let mut text_parts = Vec::new();

            for block in blocks {
                match block {
                    ContentBlock::Text { text } => {
                        text_parts.push(truncate_text(text, 200));
                    }
                    ContentBlock::ToolUse { name, .. } => {
                        if !tool_names.contains(name) {
                            tool_names.push(name.clone());
                        }
                    }
                    ContentBlock::ToolResult { content, is_error, .. } => {
                        let prefix = if *is_error { "ERROR" } else { "OK" };
                        let first_line = content.lines().next().unwrap_or("");
                        outcomes.push(format!("[{prefix}] {}", truncate_text(first_line, 100)));
                    }
                }
            }

            let summary = if text_parts.is_empty() {
                outcomes.join("; ")
            } else {
                text_parts.join(" ")
            };

            let mut seg = ContextSegment::new(round, round, summary);
            seg.tools_used = tool_names;
            seg
        }
    }
}

/// Extract decision-like sentences from text.
fn extract_decisions(text: &str) -> Vec<String> {
    let decision_keywords = ["decided", "chose", "will use", "switched to", "selected", "using"];
    text.lines()
        .filter(|line| {
            let lower = line.to_lowercase();
            decision_keywords.iter().any(|kw| lower.contains(kw))
        })
        .map(|l| truncate_text(l, 200).to_string())
        .take(5) // max 5 decisions per message
        .collect()
}

/// Extract file paths from text using a simple heuristic.
fn extract_file_paths(text: &str) -> Vec<String> {
    let mut files = Vec::new();
    // Split on whitespace and common delimiters
    for word in text.split(|c: char| c.is_whitespace() || c == ',' || c == ';') {
        let trimmed = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '/' && c != '.' && c != '_' && c != '-');
        // Trim trailing period that's sentence punctuation (not part of extension)
        let trimmed = if trimmed.ends_with('.') && !has_code_extension(trimmed) {
            &trimmed[..trimmed.len() - 1]
        } else {
            trimmed
        };
        if trimmed.contains('/')
            && trimmed.contains('.')
            && trimmed.len() > 3
            && !files.contains(&trimmed.to_string())
        {
            files.push(trimmed.to_string());
        }
    }
    files.into_iter().take(10).collect() // max 10 file paths
}

/// Check if a string ends with a common code file extension.
fn has_code_extension(s: &str) -> bool {
    let exts = [".rs", ".py", ".ts", ".js", ".tsx", ".jsx", ".md", ".toml",
                ".json", ".yaml", ".yml", ".go", ".c", ".h", ".cpp", ".java",
                ".rb", ".sh", ".css", ".html", ".sql", ".txt", ".lock"];
    exts.iter().any(|ext| s.ends_with(ext))
}

/// Truncate text to max chars at a word boundary.
fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let break_at = text[..max_chars]
        .rfind(char::is_whitespace)
        .unwrap_or(max_chars);
    format!("{}...", &text[..break_at])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_segment() {
        let seg = ContextSegment::new(1, 3, "Summary of rounds 1-3".to_string());
        assert_eq!(seg.round_start, 1);
        assert_eq!(seg.round_end, 3);
        assert!(seg.token_estimate > 0);
        assert!(seg.decisions.is_empty());
    }

    #[test]
    fn merge_segments() {
        let a = ContextSegment {
            round_start: 1,
            round_end: 3,
            summary: "First part.".to_string(),
            decisions: vec!["Use Rust.".to_string()],
            files_modified: vec!["src/main.rs".to_string()],
            tools_used: vec!["file_read".to_string()],
            token_estimate: 10,
            created_at: Utc::now(),
        };
        let b = ContextSegment {
            round_start: 4,
            round_end: 6,
            summary: "Second part.".to_string(),
            decisions: vec!["Add tests.".to_string()],
            files_modified: vec!["src/main.rs".to_string(), "tests/test.rs".to_string()],
            tools_used: vec!["bash".to_string()],
            token_estimate: 12,
            created_at: Utc::now(),
        };

        let merged = ContextSegment::merge(&a, &b);
        assert_eq!(merged.round_start, 1);
        assert_eq!(merged.round_end, 6);
        assert!(merged.summary.contains("First part."));
        assert!(merged.summary.contains("Second part."));
        assert_eq!(merged.decisions.len(), 2);
        // Files deduped
        assert_eq!(merged.files_modified.len(), 2);
        assert_eq!(merged.tools_used.len(), 2);
    }

    #[test]
    fn to_context_string_includes_metadata() {
        let seg = ContextSegment {
            round_start: 1,
            round_end: 5,
            summary: "Summary text".to_string(),
            decisions: vec!["Use tokio".to_string()],
            files_modified: vec!["src/lib.rs".to_string()],
            tools_used: vec!["bash".to_string()],
            token_estimate: 20,
            created_at: Utc::now(),
        };
        let ctx = seg.to_context_string();
        assert!(ctx.contains("[Rounds 1-5]"));
        assert!(ctx.contains("Summary text"));
        assert!(ctx.contains("Decisions: Use tokio"));
        assert!(ctx.contains("Files: src/lib.rs"));
        assert!(ctx.contains("Tools: bash"));
    }

    #[test]
    fn extract_decisions_from_text() {
        let text = "We decided to use Rust.\nThe code is clean.\nWe chose SQLite for storage.";
        let decisions = extract_decisions(text);
        assert_eq!(decisions.len(), 2);
        assert!(decisions[0].contains("decided"));
        assert!(decisions[1].contains("chose"));
    }

    #[test]
    fn extract_file_paths_from_text() {
        let text = "Modified src/main.rs and tests/test.rs. Also updated Cargo.toml";
        let files = extract_file_paths(text);
        assert!(files.contains(&"src/main.rs".to_string()));
        assert!(files.contains(&"tests/test.rs".to_string()));
    }

    #[test]
    fn truncate_text_short() {
        assert_eq!(truncate_text("hello", 100), "hello");
    }

    #[test]
    fn truncate_text_long() {
        let text = "This is a long sentence that should be truncated at a word boundary";
        let result = truncate_text(text, 30);
        assert!(result.len() <= 34); // 30 + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn extract_segment_from_text_message() {
        use cuervo_core::types::{ChatMessage, MessageContent, Role};
        let msg = ChatMessage {
            role: Role::User,
            content: MessageContent::Text(
                "We decided to use Rust. Modified src/main.rs and tests/lib.rs.".to_string(),
            ),
        };
        let seg = extract_segment_from_message(&msg, 5);
        assert_eq!(seg.round_start, 5);
        assert_eq!(seg.round_end, 5);
        assert!(!seg.decisions.is_empty());
        assert!(!seg.files_modified.is_empty());
    }

    #[test]
    fn extract_segment_from_blocks_message() {
        use cuervo_core::types::{ChatMessage, ContentBlock, MessageContent, Role};
        let msg = ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![
                ContentBlock::Text {
                    text: "Running tests".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"cmd": "cargo test"}),
                },
            ]),
        };
        let seg = extract_segment_from_message(&msg, 3);
        assert_eq!(seg.tools_used, vec!["bash"]);
        assert!(seg.summary.contains("Running tests"));
    }

    #[test]
    fn extract_segment_from_tool_result() {
        use cuervo_core::types::{ChatMessage, ContentBlock, MessageContent, Role};
        let msg = ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "t1".to_string(),
                content: "test result: 42 passed, 0 failed".to_string(),
                is_error: false,
            }]),
        };
        let seg = extract_segment_from_message(&msg, 4);
        assert!(seg.summary.contains("[OK]"));
    }

    #[test]
    fn decisions_capped_at_5() {
        let lines: Vec<String> = (0..20)
            .map(|i| format!("We decided item {i}"))
            .collect();
        let text = lines.join("\n");
        let decisions = extract_decisions(&text);
        assert_eq!(decisions.len(), 5);
    }

    #[test]
    fn file_paths_capped_at_10() {
        let paths: Vec<String> = (0..20)
            .map(|i| format!("src/module_{i}.rs"))
            .collect();
        let text = paths.join(" ");
        let files = extract_file_paths(&text);
        assert!(files.len() <= 10);
    }
}
