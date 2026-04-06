//! Snippet generation (KWIC - Keyword In Context).
//!
//! Extracts the most relevant portion of a document around query term matches,
//! with configurable context window size.

pub struct Snippeter {
    max_length: usize,
}

impl Snippeter {
    pub fn new(max_length: usize) -> Self {
        Self { max_length }
    }

    /// Generate a snippet around the first matching query term (KWIC algorithm).
    ///
    /// Strategy:
    /// 1. Find the first occurrence of any query term (case-insensitive).
    /// 2. Extract a window of `max_length` characters centered on the match.
    /// 3. Adjust to word boundaries to avoid cutting words.
    /// 4. Add ellipsis markers if the snippet doesn't cover the full text.
    pub fn generate(&self, text: &str, query_terms: &[String]) -> String {
        if text.is_empty() || query_terms.is_empty() {
            return String::from("...");
        }

        let text_lower = text.to_lowercase();

        // Find the earliest matching position.
        let mut best_pos: Option<usize> = None;
        for term in query_terms {
            let term_lower = term.to_lowercase();
            if let Some(pos) = text_lower.find(&term_lower) {
                best_pos = Some(match best_pos {
                    Some(prev) if prev < pos => prev,
                    _ => pos,
                });
            }
        }

        let match_pos = match best_pos {
            Some(pos) => pos,
            None => {
                // No match — return head of text.
                let end = text
                    .char_indices()
                    .take_while(|(i, _)| *i < self.max_length)
                    .last()
                    .map(|(i, c)| i + c.len_utf8())
                    .unwrap_or(text.len());
                let snippet = &text[..end];
                return if end < text.len() {
                    format!("{}...", snippet.trim_end())
                } else {
                    snippet.to_string()
                };
            }
        };

        // Compute window centered on match.
        let half = self.max_length / 2;
        let start = match_pos.saturating_sub(half);
        let end = (match_pos + half).min(text.len());

        // Adjust to UTF-8 boundaries.
        let start = text[start..]
            .char_indices()
            .next()
            .map(|(_, _)| start)
            .unwrap_or(start);
        let end = text[..end]
            .char_indices()
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(end);

        // Adjust to word boundaries (don't cut mid-word).
        let adjusted_start = if start > 0 {
            text[start..]
                .find(char::is_whitespace)
                .map(|ws| start + ws + 1)
                .unwrap_or(start)
        } else {
            0
        };

        let adjusted_end = if end < text.len() {
            text[..end].rfind(char::is_whitespace).unwrap_or(end)
        } else {
            text.len()
        };

        let snippet = text[adjusted_start..adjusted_end].trim();

        // Add ellipsis markers.
        let prefix = if adjusted_start > 0 { "..." } else { "" };
        let suffix = if adjusted_end < text.len() { "..." } else { "" };

        format!("{}{}{}", prefix, snippet, suffix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_with_match() {
        let snippeter = Snippeter::new(40);
        let text = "The quick brown fox jumps over the lazy dog near the river bank";
        let result = snippeter.generate(text, &["fox".to_string()]);
        assert!(result.contains("fox"), "snippet: {result}");
    }

    #[test]
    fn snippet_no_match_returns_head() {
        let snippeter = Snippeter::new(20);
        let text = "The quick brown fox jumps over the lazy dog";
        let result = snippeter.generate(text, &["elephant".to_string()]);
        assert!(result.starts_with("The"), "snippet: {result}");
        assert!(result.ends_with("..."), "snippet: {result}");
    }

    #[test]
    fn snippet_empty_text() {
        let snippeter = Snippeter::new(40);
        assert_eq!(snippeter.generate("", &["test".to_string()]), "...");
    }

    #[test]
    fn snippet_empty_terms() {
        let snippeter = Snippeter::new(40);
        assert_eq!(snippeter.generate("some text", &[]), "...");
    }

    #[test]
    fn snippet_case_insensitive() {
        let snippeter = Snippeter::new(60);
        let text = "Important Error Message found in the application log";
        let result = snippeter.generate(text, &["error".to_string()]);
        assert!(result.contains("Error"), "snippet: {result}");
    }

    #[test]
    fn snippet_short_text_no_ellipsis() {
        let snippeter = Snippeter::new(100);
        let text = "short text";
        let result = snippeter.generate(text, &["short".to_string()]);
        assert_eq!(result, "short text");
    }
}
