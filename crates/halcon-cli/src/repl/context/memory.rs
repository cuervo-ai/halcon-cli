//! ContextSource adapter for the persistent memory store.
//!
//! Retrieves relevant memories via BM25 full-text search (FTS5)
//! and returns them as context chunks for the assembler.

use async_trait::async_trait;

use halcon_context::estimate_tokens;
use halcon_core::error::Result;
use halcon_core::traits::{ContextChunk, ContextQuery, ContextSource};
use halcon_storage::AsyncDatabase;

/// A ContextSource that retrieves relevant memories from the database.
///
/// Uses BM25 full-text search to find memories matching the user's query.
/// Falls back gracefully when no user message is available or no results found.
pub struct MemorySource {
    db: AsyncDatabase,
    top_k: usize,
    token_budget: usize,
}

impl MemorySource {
    pub fn new(db: AsyncDatabase, top_k: usize, token_budget: usize) -> Self {
        Self {
            db,
            top_k,
            token_budget,
        }
    }
}

#[async_trait]
impl ContextSource for MemorySource {
    fn name(&self) -> &str {
        "memory"
    }

    fn priority(&self) -> u32 {
        80 // Below instructions (100), above lower-priority sources.
    }

    async fn gather(&self, query: &ContextQuery) -> Result<Vec<ContextChunk>> {
        let user_message = match &query.user_message {
            Some(msg) if !msg.trim().is_empty() => msg.clone(),
            _ => return Ok(vec![]),
        };

        // Use the user message as FTS query.
        // FTS5 handles tokenization and stemming.
        // AsyncDatabase handles spawn_blocking internally.
        let entries = self.db.search_memory_fts(&user_message, self.top_k).await?;

        if entries.is_empty() {
            return Ok(vec![]);
        }

        // Build memory context within our sub-budget.
        let mut chunks = Vec::new();
        let mut budget_used = 0usize;
        let budget = self.token_budget.min(query.token_budget);

        for entry in &entries {
            let label = entry.entry_type.as_str();
            let line = format!("[{label}] {}", entry.content);
            let tokens = estimate_tokens(&line);

            if budget_used + tokens > budget {
                break;
            }

            chunks.push(ContextChunk {
                source: format!("memory:{label}"),
                priority: self.priority(),
                content: line,
                estimated_tokens: tokens,
            });
            budget_used += tokens;
        }

        Ok(chunks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use chrono::Utc;
    use halcon_storage::{Database, MemoryEntry, MemoryEntryType};
    use sha2::{Digest, Sha256};
    use uuid::Uuid;

    fn make_entry(content: &str, entry_type: MemoryEntryType) -> MemoryEntry {
        let hash = hex::encode(Sha256::digest(content.as_bytes()));
        MemoryEntry {
            entry_id: Uuid::new_v4(),
            session_id: None,
            entry_type,
            content: content.to_string(),
            content_hash: hash,
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            expires_at: None,
            relevance_score: 1.0,
        }
    }

    fn query_with_message(msg: &str) -> ContextQuery {
        ContextQuery {
            working_directory: "/tmp".into(),
            user_message: Some(msg.to_string()),
            token_budget: 10000,
        }
    }

    fn query_no_message() -> ContextQuery {
        ContextQuery {
            working_directory: "/tmp".into(),
            user_message: None,
            token_budget: 10000,
        }
    }

    #[tokio::test]
    async fn no_user_message_returns_empty() {
        let db = AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap()));
        let source = MemorySource::new(db, 5, 2000);
        let chunks = source.gather(&query_no_message()).await.unwrap();
        assert!(chunks.is_empty());
    }

    #[tokio::test]
    async fn empty_user_message_returns_empty() {
        let db = AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap()));
        let source = MemorySource::new(db, 5, 2000);
        let query = ContextQuery {
            working_directory: "/tmp".into(),
            user_message: Some("   ".to_string()),
            token_budget: 10000,
        };
        let chunks = source.gather(&query).await.unwrap();
        assert!(chunks.is_empty());
    }

    #[tokio::test]
    async fn finds_relevant_memories() {
        let db = AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap()));

        db.inner().insert_memory(&make_entry(
            "Rust workspace with nine crates for the CLI tool",
            MemoryEntryType::Fact,
        ))
        .unwrap();
        db.inner().insert_memory(&make_entry(
            "Decision to use tokio async runtime for concurrency",
            MemoryEntryType::Decision,
        ))
        .unwrap();

        let source = MemorySource::new(db, 5, 2000);
        let chunks = source.gather(&query_with_message("tokio")).await.unwrap();

        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.contains("tokio"));
        assert_eq!(chunks[0].source, "memory:decision");
        assert_eq!(chunks[0].priority, 80);
    }

    #[tokio::test]
    async fn respects_token_budget() {
        let db = AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap()));

        // Insert multiple entries that all match "rust"
        for i in 0..10 {
            db.inner().insert_memory(&make_entry(
                &format!("Rust fact number {i} with some extra content to increase token count significantly"),
                MemoryEntryType::Fact,
            ))
            .unwrap();
        }

        // Very small token budget: should limit results
        let source = MemorySource::new(db, 10, 50);
        let chunks = source.gather(&query_with_message("rust")).await.unwrap();

        // Should have fewer than 10 due to budget
        assert!(chunks.len() < 10);
        assert!(!chunks.is_empty());
    }

    #[tokio::test]
    async fn no_matches_returns_empty() {
        let db = AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap()));
        db.inner().insert_memory(&make_entry(
            "Python script for data analysis",
            MemoryEntryType::CodeSnippet,
        ))
        .unwrap();

        let source = MemorySource::new(db, 5, 2000);
        let chunks = source
            .gather(&query_with_message("nonexistent_xyz_query"))
            .await
            .unwrap();
        assert!(chunks.is_empty());
    }

    #[test]
    fn metadata() {
        let db = AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap()));
        let source = MemorySource::new(db, 5, 2000);
        assert_eq!(source.name(), "memory");
        assert_eq!(source.priority(), 80);
    }
}
