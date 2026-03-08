/// Architecture & Design Context Server (Server 2).
///
/// Provides context from architecture decision records (ADRs), design documents,
/// system diagrams, and technical specifications.
/// Phase: Planning
/// Priority: 90

use async_trait::async_trait;
use halcon_context::estimate_tokens;
use halcon_core::error::{HalconError, Result};
use halcon_core::traits::{ContextChunk, ContextQuery, ContextSource};
use halcon_core::types::SdlcPhase;
use halcon_storage::{AsyncDatabase, Database};
use std::sync::Arc;

pub struct ArchitectureServer {
    db: AsyncDatabase,
    priority: u32,
    token_budget: u32,
}

impl ArchitectureServer {
    pub fn new(db: AsyncDatabase, priority: u32, token_budget: u32) -> Self {
        Self {
            db,
            priority,
            token_budget,
        }
    }

    pub fn sdlc_phase(&self) -> SdlcPhase {
        SdlcPhase::Planning
    }

    async fn fetch_architecture_docs(&self, query: Option<&str>) -> Result<Vec<ArchitectureDoc>> {
        let db = self.db.clone();
        let query_opt = query.map(String::from);

        tokio::task::spawn_blocking(move || {
            let db_ref = db.inner();
            let conn = db_ref.conn()
                .map_err(|e| HalconError::DatabaseError(e.to_string()))?;

            let docs = if let Some(q) = query_opt {
                // FTS5 search
                let mut stmt = conn.prepare(
                    "SELECT doc_id, title, content, doc_type, status, created_at, updated_at
                     FROM architecture_documents
                     WHERE doc_id IN (
                       SELECT rowid FROM architecture_documents_fts WHERE architecture_documents_fts MATCH ?
                     )
                     ORDER BY created_at DESC
                     LIMIT 10"
                ).map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                let rows = stmt.query_map([q], |row| {
                    Ok(ArchitectureDoc {
                        doc_id: row.get(0)?,
                        title: row.get(1)?,
                        content: row.get(2)?,
                        doc_type: row.get(3)?,
                        status: row.get(4)?,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                    })
                }).map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                rows.collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| HalconError::DatabaseError(e.to_string()))?
            } else {
                // No query → return recent active/approved docs
                let mut stmt = conn.prepare(
                    "SELECT doc_id, title, content, doc_type, status, created_at, updated_at
                     FROM architecture_documents
                     WHERE status IN ('active', 'approved')
                     ORDER BY created_at DESC
                     LIMIT 10"
                ).map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                let rows = stmt.query_map([], |row| {
                    Ok(ArchitectureDoc {
                        doc_id: row.get(0)?,
                        title: row.get(1)?,
                        content: row.get(2)?,
                        doc_type: row.get(3)?,
                        status: row.get(4)?,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                    })
                }).map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                rows.collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| HalconError::DatabaseError(e.to_string()))?
            };

            Ok::<Vec<ArchitectureDoc>, HalconError>(docs)
        })
        .await
        .map_err(|e| HalconError::DatabaseError(e.to_string()))?
    }
}

#[derive(Debug)]
struct ArchitectureDoc {
    doc_id: String,
    title: String,
    content: String,
    doc_type: String,
    status: String,
    #[allow(dead_code)]
    created_at: i64,
    #[allow(dead_code)]
    updated_at: i64,
}

#[async_trait]
impl ContextSource for ArchitectureServer {
    fn name(&self) -> &str {
        "architecture"
    }

    fn priority(&self) -> u32 {
        self.priority
    }

    async fn gather(&self, query: &ContextQuery) -> Result<Vec<ContextChunk>> {
        let docs = self
            .fetch_architecture_docs(query.user_message.as_deref())
            .await?;

        if docs.is_empty() {
            return Ok(vec![]);
        }

        let mut chunks = Vec::new();
        let mut total_tokens = 0usize;

        for doc in docs {
            // Truncate content if too long (max 1000 chars per doc)
            let content_preview = if doc.content.len() > 1000 {
                format!("{}...", &doc.content[..1000])
            } else {
                doc.content.clone()
            };

            let content = format!(
                "[Architecture Context]\n\
                 Document: {}\n\
                 Type: {}\n\
                 Status: {}\n\
                 Content:\n{}\n",
                doc.title, doc.doc_type, doc.status, content_preview
            );

            let token_estimate = estimate_tokens(&content);

            // Budget check: stop if we exceed budget
            if total_tokens + token_estimate > self.token_budget as usize {
                break;
            }

            total_tokens += token_estimate;

            chunks.push(ContextChunk {
                source: self.name().to_string(),
                priority: self.priority,
                content,
                estimated_tokens: token_estimate,
            });
        }

        Ok(chunks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_architecture_server_creation() {
        let db = Database::open_in_memory().unwrap();
        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = ArchitectureServer::new(async_db, 90, 4000);
        assert_eq!(server.name(), "architecture");
        assert_eq!(server.priority(), 90);
        assert_eq!(server.sdlc_phase(), SdlcPhase::Planning);
    }

    #[tokio::test]
    async fn test_gather_no_docs() {
        let db = Database::open_in_memory().unwrap();
        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = ArchitectureServer::new(async_db, 90, 4000);

        let query = ContextQuery {
            working_directory: "/tmp".to_string(),
            user_message: None,
            token_budget: 10000,
        };

        let chunks = server.gather(&query).await.unwrap();
        assert_eq!(chunks.len(), 0);
    }

    #[tokio::test]
    async fn test_gather_with_data() {
        let db = Database::open_in_memory().unwrap();

        // Insert test architecture doc
        db.conn().unwrap()
            .execute(
                "INSERT INTO architecture_documents (doc_id, title, content, doc_type, status, created_at, updated_at)
                 VALUES ('ADR-001', 'Microservices Architecture', 'We will use microservices...', 'ADR', 'active', 1739606400, 1739606400)",
                [],
            )
            .unwrap();

        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = ArchitectureServer::new(async_db, 90, 4000);

        let query = ContextQuery {
            working_directory: "/tmp".to_string(),
            user_message: None,
            token_budget: 10000,
        };

        let chunks = server.gather(&query).await.unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.contains("Microservices Architecture"));
        assert!(chunks[0].content.contains("ADR"));
    }

    #[tokio::test]
    async fn test_content_truncation() {
        let db = Database::open_in_memory().unwrap();

        // Insert doc with long content
        let long_content = "x".repeat(2000);
        db.conn().unwrap()
            .execute(
                "INSERT INTO architecture_documents (doc_id, title, content, doc_type, status, created_at, updated_at)
                 VALUES ('ADR-002', 'Long Doc', ?1, 'ADR', 'active', 1739606400, 1739606400)",
                [&long_content],
            )
            .unwrap();

        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = ArchitectureServer::new(async_db, 90, 4000);

        let query = ContextQuery {
            working_directory: "/tmp".to_string(),
            user_message: None,
            token_budget: 10000,
        };

        let chunks = server.gather(&query).await.unwrap();
        assert_eq!(chunks.len(), 1);
        // Content should be truncated to ~1000 chars + "..."
        assert!(chunks[0].content.contains("..."));
        assert!(chunks[0].content.len() < 1200); // Some overhead for formatting
    }

    #[tokio::test]
    async fn test_budget_enforcement() {
        let db = Database::open_in_memory().unwrap();

        // Insert doc
        db.conn().unwrap()
            .execute(
                "INSERT INTO architecture_documents (doc_id, title, content, doc_type, status, created_at, updated_at)
                 VALUES ('ADR-003', 'Design Doc', 'Some content', 'Design', 'active', 1739606400, 1739606400)",
                [],
            )
            .unwrap();

        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = ArchitectureServer::new(async_db, 90, 100); // Very small budget

        let query = ContextQuery {
            working_directory: "/tmp".to_string(),
            user_message: None,
            token_budget: 10000,
        };

        let chunks = server.gather(&query).await.unwrap();
        // Should stop early due to budget constraint
        assert!(chunks.len() <= 1);
    }
}
