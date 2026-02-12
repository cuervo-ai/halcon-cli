use async_trait::async_trait;

use crate::error::Result;

/// A chunk of context to include in the model's system prompt.
#[derive(Debug, Clone)]
pub struct ContextChunk {
    /// Source identifier (e.g., "instruction:project", "repo_map").
    pub source: String,
    /// Priority (higher = included first when budget is tight).
    pub priority: u32,
    /// The text content to include.
    pub content: String,
    /// Estimated token count for this chunk.
    pub estimated_tokens: usize,
}

/// Query parameters for context gathering.
#[derive(Debug, Clone, Default)]
pub struct ContextQuery {
    /// Current working directory.
    pub working_directory: String,
    /// The user's latest message (for relevance scoring).
    pub user_message: Option<String>,
    /// Maximum total tokens for all context combined.
    pub token_budget: usize,
}

/// Trait for sources that contribute context to model invocations.
///
/// Implementations gather relevant context from their domain
/// (instruction files, repo maps, session memory, etc.) and return
/// prioritized chunks that the assembler fits into the token budget.
#[async_trait]
pub trait ContextSource: Send + Sync {
    /// Unique name for this context source.
    fn name(&self) -> &str;

    /// Default priority (higher = included first).
    fn priority(&self) -> u32;

    /// Gather context chunks relevant to the given query.
    async fn gather(&self, query: &ContextQuery) -> Result<Vec<ContextChunk>>;
}
