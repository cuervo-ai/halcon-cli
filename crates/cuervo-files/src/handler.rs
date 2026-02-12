//! FileHandler trait and FileContent output type.

use async_trait::async_trait;

use crate::detect::{FileInfo, FileType};
use crate::Error;

/// Result of extracting content from a file.
#[derive(Debug, Clone)]
pub struct FileContent {
    /// Extracted text suitable for LLM context.
    pub text: String,
    /// Estimated tokens for the extracted text.
    pub estimated_tokens: usize,
    /// Format-specific metadata.
    pub metadata: serde_json::Value,
    /// Whether the content was truncated to fit the token budget.
    pub truncated: bool,
}

/// Handler for a specific file format.
///
/// Implementations extract text and metadata from files of supported types.
/// All handlers must be async-safe (sync operations wrapped in `spawn_blocking`).
#[async_trait]
pub trait FileHandler: Send + Sync {
    /// Handler name (e.g., "json", "csv", "pdf").
    fn name(&self) -> &str;

    /// File types this handler supports.
    fn supported_types(&self) -> &[FileType];

    /// Estimate output tokens without reading the full file.
    fn estimate_tokens(&self, info: &FileInfo) -> usize;

    /// Extract text content from a file within a token budget.
    async fn extract(&self, info: &FileInfo, token_budget: usize) -> Result<FileContent, Error>;
}

/// Estimate tokens from a string using the standard heuristic (~4 chars per token).
pub fn estimate_tokens_from_text(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Truncate text to fit within a token budget.
pub fn truncate_to_budget(text: &str, token_budget: usize) -> (String, bool) {
    let max_chars = token_budget * 4;
    if text.len() <= max_chars {
        (text.to_string(), false)
    } else {
        // Find a safe char boundary.
        let mut end = max_chars;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        let mut truncated = text[..end].to_string();
        truncated.push_str("\n\n[... truncated to fit token budget ...]");
        (truncated, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_tokens_basic() {
        assert_eq!(estimate_tokens_from_text(""), 0);
        assert_eq!(estimate_tokens_from_text("abcd"), 1);
        assert_eq!(estimate_tokens_from_text("abcde"), 2);
        assert_eq!(estimate_tokens_from_text(&"a".repeat(100)), 25);
    }

    #[test]
    fn truncate_within_budget() {
        let (text, truncated) = truncate_to_budget("hello", 100);
        assert_eq!(text, "hello");
        assert!(!truncated);
    }

    #[test]
    fn truncate_exceeds_budget() {
        let long = "a".repeat(1000);
        let (text, truncated) = truncate_to_budget(&long, 10);
        assert!(truncated);
        assert!(text.len() < 1000);
        assert!(text.contains("truncated"));
    }

    #[test]
    fn truncate_respects_char_boundary() {
        // 4-byte UTF-8 chars.
        let emojis = "🦀".repeat(100);
        let (text, truncated) = truncate_to_budget(&emojis, 5);
        assert!(truncated);
        // Should not panic or produce invalid UTF-8.
        assert!(text.is_char_boundary(0));
    }

    #[test]
    fn truncate_zero_budget() {
        let (text, truncated) = truncate_to_budget("hello world", 0);
        assert!(truncated);
        assert!(text.contains("truncated"));
    }

    #[test]
    fn truncate_exact_budget() {
        // 5 chars = 2 tokens at 4 chars/token (ceil)
        let (text, truncated) = truncate_to_budget("hello", 2);
        assert_eq!(text, "hello");
        assert!(!truncated);
    }

    #[test]
    fn estimate_tokens_multibyte() {
        // 3-byte UTF-8 chars: each 'こ' is 3 bytes
        let text = "こんにちは"; // 5 chars, 15 bytes
        let tokens = estimate_tokens_from_text(text);
        assert_eq!(tokens, 4); // 15 / 4 = 3.75 → ceil = 4
    }

    #[test]
    fn truncate_single_multibyte_char() {
        // Budget of 1 token = 4 chars; one emoji is 4 bytes but 1 char
        let (text, truncated) = truncate_to_budget("🦀🦀🦀🦀🦀", 1);
        assert!(truncated);
        // Verify valid UTF-8 — should not split a codepoint
        let _ = text.len();
    }
}
