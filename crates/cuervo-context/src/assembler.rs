//! Context assembler: collects chunks from multiple sources and
//! fits them into a token budget.

use futures::future::join_all;

use cuervo_core::traits::{ContextChunk, ContextQuery, ContextSource};

/// Assemble context from multiple sources within a token budget.
///
/// Gathers chunks from all sources in parallel, sorts by priority (descending),
/// and includes as many as fit within the token budget.
pub async fn assemble_context(
    sources: &[Box<dyn ContextSource>],
    query: &ContextQuery,
) -> Vec<ContextChunk> {
    // Gather all sources in parallel (max-of-latencies instead of sum-of-latencies).
    let futures: Vec<_> = sources
        .iter()
        .map(|source| {
            let name = source.name().to_string();
            async move {
                match source.gather(query).await {
                    Ok(chunks) => chunks,
                    Err(e) => {
                        tracing::warn!(source = %name, error = %e, "Context source failed");
                        Vec::new()
                    }
                }
            }
        })
        .collect();

    let results = join_all(futures).await;
    let mut all_chunks: Vec<ContextChunk> = Vec::new();
    for chunks in results {
        all_chunks.extend(chunks);
    }

    // Sort by priority descending (highest priority first).
    all_chunks.sort_by(|a, b| b.priority.cmp(&a.priority));

    // Fit to budget.
    let mut budget_remaining = query.token_budget;
    let mut selected: Vec<ContextChunk> = Vec::new();

    for chunk in all_chunks {
        if chunk.estimated_tokens <= budget_remaining {
            budget_remaining -= chunk.estimated_tokens;
            selected.push(chunk);
        }
    }

    selected
}

/// Combine assembled chunks into a single system prompt string.
pub fn chunks_to_system_prompt(chunks: &[ContextChunk]) -> String {
    if chunks.is_empty() {
        return String::new();
    }

    chunks
        .iter()
        .map(|c| c.content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Estimate token count for a string.
///
/// Simple heuristic: ~4 characters per token (English text average).
/// Good enough for budgeting; exact counting requires a tokenizer.
pub fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use cuervo_core::error::Result;

    struct MockSource {
        name: &'static str,
        priority: u32,
        chunks: Vec<ContextChunk>,
    }

    #[async_trait]
    impl ContextSource for MockSource {
        fn name(&self) -> &str {
            self.name
        }
        fn priority(&self) -> u32 {
            self.priority
        }
        async fn gather(&self, _query: &ContextQuery) -> Result<Vec<ContextChunk>> {
            Ok(self.chunks.clone())
        }
    }

    struct FailingSource;

    #[async_trait]
    impl ContextSource for FailingSource {
        fn name(&self) -> &str {
            "failing"
        }
        fn priority(&self) -> u32 {
            100
        }
        async fn gather(&self, _query: &ContextQuery) -> Result<Vec<ContextChunk>> {
            Err(cuervo_core::error::CuervoError::Internal(
                "test error".into(),
            ))
        }
    }

    fn chunk(source: &str, priority: u32, content: &str, tokens: usize) -> ContextChunk {
        ContextChunk {
            source: source.into(),
            priority,
            content: content.into(),
            estimated_tokens: tokens,
        }
    }

    fn query(budget: usize) -> ContextQuery {
        ContextQuery {
            working_directory: "/tmp".into(),
            user_message: None,
            token_budget: budget,
        }
    }

    #[tokio::test]
    async fn empty_sources_returns_empty() {
        let sources: Vec<Box<dyn ContextSource>> = vec![];
        let result = assemble_context(&sources, &query(1000)).await;
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn single_source_fits_budget() {
        let sources: Vec<Box<dyn ContextSource>> = vec![Box::new(MockSource {
            name: "test",
            priority: 10,
            chunks: vec![chunk("test", 10, "hello world", 3)],
        })];
        let result = assemble_context(&sources, &query(1000)).await;
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "hello world");
    }

    #[tokio::test]
    async fn budget_respected() {
        let sources: Vec<Box<dyn ContextSource>> = vec![Box::new(MockSource {
            name: "test",
            priority: 10,
            chunks: vec![chunk("a", 10, "small", 5), chunk("b", 10, "large", 100)],
        })];
        let result = assemble_context(&sources, &query(10)).await;
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "small");
    }

    #[tokio::test]
    async fn priority_ordering() {
        let sources: Vec<Box<dyn ContextSource>> = vec![
            Box::new(MockSource {
                name: "low",
                priority: 1,
                chunks: vec![chunk("low", 1, "low-pri", 5)],
            }),
            Box::new(MockSource {
                name: "high",
                priority: 100,
                chunks: vec![chunk("high", 100, "high-pri", 5)],
            }),
        ];
        let result = assemble_context(&sources, &query(1000)).await;
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].content, "high-pri");
        assert_eq!(result[1].content, "low-pri");
    }

    #[tokio::test]
    async fn high_priority_wins_when_budget_tight() {
        let sources: Vec<Box<dyn ContextSource>> = vec![Box::new(MockSource {
            name: "test",
            priority: 10,
            chunks: vec![
                chunk("high", 100, "important", 8),
                chunk("low", 1, "filler", 8),
            ],
        })];
        // Budget for only one chunk.
        let result = assemble_context(&sources, &query(10)).await;
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "important");
    }

    #[tokio::test]
    async fn failing_source_does_not_break_assembly() {
        let sources: Vec<Box<dyn ContextSource>> = vec![
            Box::new(FailingSource),
            Box::new(MockSource {
                name: "ok",
                priority: 10,
                chunks: vec![chunk("ok", 10, "good data", 5)],
            }),
        ];
        let result = assemble_context(&sources, &query(1000)).await;
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "good data");
    }

    #[test]
    fn chunks_to_prompt_empty() {
        assert_eq!(chunks_to_system_prompt(&[]), "");
    }

    #[test]
    fn chunks_to_prompt_joins_with_double_newline() {
        let chunks = vec![chunk("a", 10, "first", 1), chunk("b", 5, "second", 1)];
        let prompt = chunks_to_system_prompt(&chunks);
        assert_eq!(prompt, "first\n\nsecond");
    }

    #[tokio::test]
    async fn parallel_sources_all_contribute() {
        let sources: Vec<Box<dyn ContextSource>> = vec![
            Box::new(MockSource {
                name: "alpha",
                priority: 10,
                chunks: vec![chunk("alpha", 10, "from-alpha", 5)],
            }),
            Box::new(MockSource {
                name: "beta",
                priority: 20,
                chunks: vec![chunk("beta", 20, "from-beta", 5)],
            }),
            Box::new(MockSource {
                name: "gamma",
                priority: 30,
                chunks: vec![chunk("gamma", 30, "from-gamma", 5)],
            }),
        ];
        let result = assemble_context(&sources, &query(1000)).await;
        assert_eq!(result.len(), 3);
        // Verify all sources contributed (sorted by priority desc).
        assert_eq!(result[0].source, "gamma");
        assert_eq!(result[1].source, "beta");
        assert_eq!(result[2].source, "alpha");
    }

    #[test]
    fn estimate_tokens_basic() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("a"), 1);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        // ~100 chars → ~25 tokens
        let text = "a".repeat(100);
        assert_eq!(estimate_tokens(&text), 25);
    }
}
