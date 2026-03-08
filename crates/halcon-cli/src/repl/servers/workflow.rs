/// CI/CD Workflow Context Server (Server 4).
///
/// Provides context from GitHub Actions workflows, CI/CD pipelines, deployment logs,
/// and workflow execution history.
/// Phase: Testing / Deployment
/// Priority: 80

use async_trait::async_trait;
use halcon_context::estimate_tokens;
use halcon_core::error::{HalconError, Result};
use halcon_core::traits::{ContextChunk, ContextQuery, ContextSource};
use halcon_core::types::SdlcPhase;
use halcon_storage::{AsyncDatabase, Database};
use std::sync::Arc;

pub struct WorkflowServer {
    db: AsyncDatabase,
    priority: u32,
    token_budget: u32,
}

impl WorkflowServer {
    pub fn new(db: AsyncDatabase, priority: u32, token_budget: u32) -> Self {
        Self {
            db,
            priority,
            token_budget,
        }
    }

    pub fn sdlc_phase(&self) -> SdlcPhase {
        SdlcPhase::Testing
    }

    async fn fetch_workflows(&self, query: Option<&str>) -> Result<Vec<WorkflowInfo>> {
        let db = self.db.clone();
        let query_opt = query.map(String::from);

        tokio::task::spawn_blocking(move || {
            let db_ref = db.inner();
            let conn = db_ref.conn()
                .map_err(|e| HalconError::DatabaseError(e.to_string()))?;

            let workflows = if let Some(q) = query_opt {
                // FTS5 search
                let mut stmt = conn.prepare(
                    "SELECT workflow_id, workflow_name, workflow_file, description,
                            trigger_events, last_run_status, last_run_at, last_run_duration_ms,
                            failure_count, success_count
                     FROM ci_workflows
                     WHERE workflow_id IN (
                       SELECT rowid FROM ci_workflows_fts WHERE ci_workflows_fts MATCH ?
                     )
                     ORDER BY last_run_at DESC NULLS LAST
                     LIMIT 10"
                ).map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                let rows = stmt.query_map([q], |row| {
                    Ok(WorkflowInfo {
                        workflow_id: row.get(0)?,
                        workflow_name: row.get(1)?,
                        workflow_file: row.get(2)?,
                        description: row.get(3)?,
                        trigger_events: row.get(4)?,
                        last_run_status: row.get(5)?,
                        last_run_at: row.get(6)?,
                        last_run_duration_ms: row.get(7)?,
                        failure_count: row.get(8)?,
                        success_count: row.get(9)?,
                    })
                }).map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                rows.collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| HalconError::DatabaseError(e.to_string()))?
            } else {
                // No query → return recent workflows (failures first, then most recent)
                let mut stmt = conn.prepare(
                    "SELECT workflow_id, workflow_name, workflow_file, description,
                            trigger_events, last_run_status, last_run_at, last_run_duration_ms,
                            failure_count, success_count
                     FROM ci_workflows
                     ORDER BY
                        CASE WHEN last_run_status = 'failure' THEN 0 ELSE 1 END,
                        last_run_at DESC NULLS LAST
                     LIMIT 10"
                ).map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                let rows = stmt.query_map([], |row| {
                    Ok(WorkflowInfo {
                        workflow_id: row.get(0)?,
                        workflow_name: row.get(1)?,
                        workflow_file: row.get(2)?,
                        description: row.get(3)?,
                        trigger_events: row.get(4)?,
                        last_run_status: row.get(5)?,
                        last_run_at: row.get(6)?,
                        last_run_duration_ms: row.get(7)?,
                        failure_count: row.get(8)?,
                        success_count: row.get(9)?,
                    })
                }).map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                rows.collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| HalconError::DatabaseError(e.to_string()))?
            };

            Ok::<Vec<WorkflowInfo>, HalconError>(workflows)
        })
        .await
        .map_err(|e| HalconError::DatabaseError(e.to_string()))?
    }
}

#[derive(Debug)]
struct WorkflowInfo {
    workflow_id: String,
    workflow_name: String,
    workflow_file: String,
    description: String,
    trigger_events: String,
    last_run_status: Option<String>,
    last_run_at: Option<i64>,
    last_run_duration_ms: Option<i64>,
    failure_count: i64,
    success_count: i64,
}

#[async_trait]
impl ContextSource for WorkflowServer {
    fn name(&self) -> &str {
        "workflow"
    }

    fn priority(&self) -> u32 {
        self.priority
    }

    async fn gather(&self, query: &ContextQuery) -> Result<Vec<ContextChunk>> {
        let workflows = self
            .fetch_workflows(query.user_message.as_deref())
            .await?;

        if workflows.is_empty() {
            return Ok(vec![]);
        }

        let mut chunks = Vec::new();
        let mut total_tokens = 0usize;

        for workflow in workflows {
            // Truncate description if too long (max 800 chars)
            let description_preview = if workflow.description.len() > 800 {
                format!("{}...", &workflow.description[..800])
            } else {
                workflow.description.clone()
            };

            let status_info = if let Some(status) = workflow.last_run_status {
                let success_rate = if workflow.success_count + workflow.failure_count > 0 {
                    (workflow.success_count as f64 / (workflow.success_count + workflow.failure_count) as f64) * 100.0
                } else {
                    0.0
                };

                format!(
                    "Last Status: {} | Success Rate: {:.1}% ({}/{} runs)",
                    status,
                    success_rate,
                    workflow.success_count,
                    workflow.success_count + workflow.failure_count
                )
            } else {
                "Status: Never run".to_string()
            };

            let duration_info = if let Some(duration) = workflow.last_run_duration_ms {
                format!(" | Duration: {:.1}s", duration as f64 / 1000.0)
            } else {
                String::new()
            };

            let content = format!(
                "[CI/CD Workflow Context]\n\
                 Workflow: {}\n\
                 File: {}\n\
                 Triggers: {}\n\
                 {}{}\n\
                 Description:\n{}\n",
                workflow.workflow_name,
                workflow.workflow_file,
                workflow.trigger_events,
                status_info,
                duration_info,
                description_preview
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
    async fn test_workflow_server_creation() {
        let db = Database::open_in_memory().unwrap();
        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = WorkflowServer::new(async_db, 80, 3000);
        assert_eq!(server.name(), "workflow");
        assert_eq!(server.priority(), 80);
        assert_eq!(server.sdlc_phase(), SdlcPhase::Testing);
    }

    #[tokio::test]
    async fn test_gather_no_workflows() {
        let db = Database::open_in_memory().unwrap();
        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = WorkflowServer::new(async_db, 80, 3000);

        let query = ContextQuery {
            working_directory: "/tmp".to_string(),
            user_message: None,
            token_budget: 10000,
        };

        let chunks = server.gather(&query).await.unwrap();
        assert_eq!(chunks.len(), 0);
    }

    #[tokio::test]
    async fn test_gather_with_workflow() {
        let db = Database::open_in_memory().unwrap();

        // Insert test workflow
        db.conn().unwrap()
            .execute(
                "INSERT INTO ci_workflows (workflow_id, workflow_name, workflow_file, description, trigger_events, last_run_status, last_run_at, last_run_duration_ms, failure_count, success_count, created_at, updated_at)
                 VALUES ('wf-001', 'CI Build', '.github/workflows/ci.yml', 'Runs tests and builds', 'push,pull_request', 'success', 1739606400, 45000, 2, 18, 1739606400, 1739606400)",
                [],
            )
            .unwrap();

        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = WorkflowServer::new(async_db, 80, 3000);

        let query = ContextQuery {
            working_directory: "/tmp".to_string(),
            user_message: None,
            token_budget: 10000,
        };

        let chunks = server.gather(&query).await.unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.contains("CI Build"));
        assert!(chunks[0].content.contains("Success Rate: 90.0%"));
        assert!(chunks[0].content.contains("Duration: 45.0s"));
    }

    #[tokio::test]
    async fn test_description_truncation() {
        let db = Database::open_in_memory().unwrap();

        // Insert workflow with long description
        let long_desc = "x".repeat(1500);
        db.conn().unwrap()
            .execute(
                "INSERT INTO ci_workflows (workflow_id, workflow_name, workflow_file, description, trigger_events, created_at, updated_at)
                 VALUES ('wf-002', 'Long Workflow', '.github/workflows/long.yml', ?1, 'push', 1739606400, 1739606400)",
                [&long_desc],
            )
            .unwrap();

        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = WorkflowServer::new(async_db, 80, 3000);

        let query = ContextQuery {
            working_directory: "/tmp".to_string(),
            user_message: None,
            token_budget: 10000,
        };

        let chunks = server.gather(&query).await.unwrap();
        assert_eq!(chunks.len(), 1);
        // Description should be truncated to ~800 chars + "..."
        assert!(chunks[0].content.contains("..."));
        assert!(chunks[0].content.len() < 1000);
    }

    #[tokio::test]
    async fn test_budget_enforcement() {
        let db = Database::open_in_memory().unwrap();

        // Insert workflow
        db.conn().unwrap()
            .execute(
                "INSERT INTO ci_workflows (workflow_id, workflow_name, workflow_file, description, trigger_events, created_at, updated_at)
                 VALUES ('wf-003', 'Test Workflow', '.github/workflows/test.yml', 'Some content', 'push', 1739606400, 1739606400)",
                [],
            )
            .unwrap();

        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = WorkflowServer::new(async_db, 80, 100); // Very small budget

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
