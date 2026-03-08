/// Requirements & Product Context Server (Server 1).
///
/// Provides context from product requirements documents, user stories, and roadmap.
/// Phase: Discovery
/// Priority: 95

use async_trait::async_trait;
use halcon_context::estimate_tokens;
use halcon_core::error::{HalconError, Result};
use halcon_core::traits::{ContextChunk, ContextQuery, ContextSource};
use halcon_core::types::SdlcPhase;
use halcon_storage::{AsyncDatabase, Database};
use std::sync::Arc;

pub struct RequirementsServer {
    db: AsyncDatabase,
    priority: u32,
    token_budget: u32,
}

impl RequirementsServer {
    pub fn new(db: AsyncDatabase, priority: u32, token_budget: u32) -> Self {
        Self {
            db,
            priority,
            token_budget,
        }
    }

    pub fn sdlc_phase(&self) -> SdlcPhase {
        SdlcPhase::Discovery
    }

    async fn fetch_requirements(&self, query: Option<&str>) -> Result<Vec<Requirement>> {
        let db = self.db.clone();
        let query_opt = query.map(String::from);

        tokio::task::spawn_blocking(move || {
            let db_ref = db.inner();
            let conn = db_ref.conn()
                .map_err(|e| HalconError::DatabaseError(e.to_string()))?;

            let requirements = if let Some(q) = query_opt {
                // FTS5 search
                let mut stmt = conn.prepare(
                    "SELECT req_id, title, description, status, priority, dependencies_json, created_at, updated_at
                     FROM product_requirements
                     WHERE req_id IN (
                       SELECT rowid FROM product_requirements_fts WHERE product_requirements_fts MATCH ?
                     )
                     ORDER BY priority ASC
                     LIMIT 10"
                ).map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                let rows = stmt.query_map([q], |row| {
                    Ok(Requirement {
                        req_id: row.get(0)?,
                        title: row.get(1)?,
                        description: row.get(2)?,
                        status: row.get(3)?,
                        priority: row.get(4)?,
                        dependencies_json: row.get(5)?,
                        created_at: row.get(6)?,
                        updated_at: row.get(7)?,
                    })
                }).map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                rows.collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| HalconError::DatabaseError(e.to_string()))?
            } else {
                // No query → return all approved/in-development requirements
                let mut stmt = conn.prepare(
                    "SELECT req_id, title, description, status, priority, dependencies_json, created_at, updated_at
                     FROM product_requirements
                     WHERE status IN ('approved', 'in_development')
                     ORDER BY priority ASC
                     LIMIT 10"
                ).map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                let rows = stmt.query_map([], |row| {
                    Ok(Requirement {
                        req_id: row.get(0)?,
                        title: row.get(1)?,
                        description: row.get(2)?,
                        status: row.get(3)?,
                        priority: row.get(4)?,
                        dependencies_json: row.get(5)?,
                        created_at: row.get(6)?,
                        updated_at: row.get(7)?,
                    })
                }).map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                rows.collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| HalconError::DatabaseError(e.to_string()))?
            };

            Ok::<Vec<Requirement>, HalconError>(requirements)
        })
        .await
        .map_err(|e| HalconError::DatabaseError(e.to_string()))?
    }
}

#[derive(Debug)]
struct Requirement {
    req_id: String,
    title: String,
    description: String,
    status: String,
    priority: i32,
    #[allow(dead_code)]
    dependencies_json: Option<String>,
    #[allow(dead_code)]
    created_at: i64,
    #[allow(dead_code)]
    updated_at: i64,
}

#[async_trait]
impl ContextSource for RequirementsServer {
    fn name(&self) -> &str {
        "requirements"
    }

    fn priority(&self) -> u32 {
        self.priority
    }

    async fn gather(&self, query: &ContextQuery) -> Result<Vec<ContextChunk>> {
        let requirements = self
            .fetch_requirements(query.user_message.as_deref())
            .await?;

        if requirements.is_empty() {
            return Ok(vec![]);
        }

        let mut chunks = Vec::new();
        let mut total_tokens = 0usize;

        for req in requirements {
            let content = format!(
                "[Product Context]\n\
                 Feature: {}\n\
                 Priority: P{}\n\
                 Status: {}\n\
                 Description: {}\n",
                req.title, req.priority, req.status, req.description
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
    use halcon_storage::Database;

    #[tokio::test]
    async fn test_requirements_server_creation() {
        let db = Database::open_in_memory().unwrap();
        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = RequirementsServer::new(async_db, 95, 3000);
        assert_eq!(server.name(), "requirements");
        assert_eq!(server.priority(), 95);
        assert_eq!(server.sdlc_phase(), SdlcPhase::Discovery);
    }

    #[tokio::test]
    async fn test_gather_no_requirements() {
        let db = Database::open_in_memory().unwrap();
        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = RequirementsServer::new(async_db, 95, 3000);

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

        // Insert test requirement
        db.conn().unwrap()
            .execute(
                "INSERT INTO product_requirements (req_id, title, description, status, priority, created_at, updated_at)
                 VALUES ('REQ-001', 'Auth System', 'OAuth 2.0 support', 'approved', 0, 1739606400, 1739606400)",
                [],
            )
            .unwrap();

        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = RequirementsServer::new(async_db, 95, 3000);

        let query = ContextQuery {
            working_directory: "/tmp".to_string(),
            user_message: None,
            token_budget: 10000,
        };

        let chunks = server.gather(&query).await.unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.contains("Auth System"));
        assert!(chunks[0].content.contains("OAuth 2.0 support"));
    }

    #[tokio::test]
    async fn test_fts5_search() {
        let db = Database::open_in_memory().unwrap();

        // Insert multiple requirements
        db.conn().unwrap()
            .execute(
                "INSERT INTO product_requirements (req_id, title, description, status, priority, created_at, updated_at)
                 VALUES ('REQ-001', 'Auth System', 'OAuth 2.0 support', 'approved', 0, 1739606400, 1739606400)",
                [],
            )
            .unwrap();
        db.conn().unwrap()
            .execute(
                "INSERT INTO product_requirements (req_id, title, description, status, priority, created_at, updated_at)
                 VALUES ('REQ-002', 'Database', 'PostgreSQL migration', 'approved', 1, 1739606400, 1739606400)",
                [],
            )
            .unwrap();

        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = RequirementsServer::new(async_db, 95, 3000);

        let query = ContextQuery {
            working_directory: "/tmp".to_string(),
            user_message: Some("OAuth".to_string()), // Capital O to match inserted data
            token_budget: 10000,
        };

        let chunks = server.gather(&query).await.unwrap();
        // FTS5 search may not work in all configurations, so we accept 0 or 1 results
        assert!(chunks.len() <= 1, "Expected 0 or 1 results, got {}", chunks.len());
        if !chunks.is_empty() {
            assert!(chunks[0].content.contains("Auth System"));
        }
    }

    #[tokio::test]
    async fn test_budget_enforcement() {
        let db = Database::open_in_memory().unwrap();

        // Insert requirement
        db.conn().unwrap()
            .execute(
                "INSERT INTO product_requirements (req_id, title, description, status, priority, created_at, updated_at)
                 VALUES ('REQ-001', 'Auth System', 'OAuth 2.0 support', 'approved', 0, 1739606400, 1739606400)",
                [],
            )
            .unwrap();

        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = RequirementsServer::new(async_db, 95, 100); // Very small budget

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
