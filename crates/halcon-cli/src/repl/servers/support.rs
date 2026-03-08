/// Support & Incidents Context Server (Server 8).
///
/// Provides context from bug reports, user feedback, crash reports,
/// support tickets, and incident resolution history.
/// Phase: Support
/// Priority: 60

use async_trait::async_trait;
use halcon_context::estimate_tokens;
use halcon_core::error::{HalconError, Result};
use halcon_core::traits::{ContextChunk, ContextQuery, ContextSource};
use halcon_core::types::SdlcPhase;
use halcon_storage::{AsyncDatabase, Database};
use std::sync::Arc;

pub struct SupportServer {
    db: AsyncDatabase,
    priority: u32,
    token_budget: u32,
}

impl SupportServer {
    pub fn new(db: AsyncDatabase, priority: u32, token_budget: u32) -> Self {
        Self {
            db,
            priority,
            token_budget,
        }
    }

    pub fn sdlc_phase(&self) -> SdlcPhase {
        SdlcPhase::Support
    }

    async fn fetch_incidents(&self, query: Option<&str>) -> Result<Vec<SupportIncident>> {
        let db = self.db.clone();
        let query_opt = query.map(String::from);

        tokio::task::spawn_blocking(move || {
            let db_ref = db.inner();
            let conn = db_ref.conn()
                .map_err(|e| HalconError::DatabaseError(e.to_string()))?;

            let incidents = if let Some(q) = query_opt {
                // FTS5 search
                let mut stmt = conn.prepare(
                    "SELECT incident_id, incident_type, priority, title, description,
                            reporter, affected_component, reproducible, reproduction_steps,
                            error_message, stack_trace, resolution, status
                     FROM support_incidents
                     WHERE incident_id IN (
                       SELECT rowid FROM support_incidents_fts WHERE support_incidents_fts MATCH ?
                     )
                     ORDER BY
                        CASE priority
                            WHEN 'critical' THEN 0
                            WHEN 'high' THEN 1
                            WHEN 'medium' THEN 2
                            WHEN 'low' THEN 3
                            ELSE 4
                        END
                     LIMIT 15"
                ).map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                let rows = stmt.query_map([q], |row| {
                    Ok(SupportIncident {
                        incident_id: row.get(0)?,
                        incident_type: row.get(1)?,
                        priority: row.get(2)?,
                        title: row.get(3)?,
                        description: row.get(4)?,
                        reporter: row.get(5)?,
                        affected_component: row.get(6)?,
                        reproducible: row.get(7)?,
                        reproduction_steps: row.get(8)?,
                        error_message: row.get(9)?,
                        stack_trace: row.get(10)?,
                        resolution: row.get(11)?,
                        status: row.get(12)?,
                    })
                }).map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                rows.collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| HalconError::DatabaseError(e.to_string()))?
            } else {
                // No query → return open high-priority incidents first
                let mut stmt = conn.prepare(
                    "SELECT incident_id, incident_type, priority, title, description,
                            reporter, affected_component, reproducible, reproduction_steps,
                            error_message, stack_trace, resolution, status
                     FROM support_incidents
                     WHERE status IN ('new', 'triaged', 'in_progress')
                     ORDER BY
                        CASE priority
                            WHEN 'critical' THEN 0
                            WHEN 'high' THEN 1
                            WHEN 'medium' THEN 2
                            WHEN 'low' THEN 3
                            ELSE 4
                        END
                     LIMIT 15"
                ).map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                let rows = stmt.query_map([], |row| {
                    Ok(SupportIncident {
                        incident_id: row.get(0)?,
                        incident_type: row.get(1)?,
                        priority: row.get(2)?,
                        title: row.get(3)?,
                        description: row.get(4)?,
                        reporter: row.get(5)?,
                        affected_component: row.get(6)?,
                        reproducible: row.get(7)?,
                        reproduction_steps: row.get(8)?,
                        error_message: row.get(9)?,
                        stack_trace: row.get(10)?,
                        resolution: row.get(11)?,
                        status: row.get(12)?,
                    })
                }).map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                rows.collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| HalconError::DatabaseError(e.to_string()))?
            };

            Ok::<Vec<SupportIncident>, HalconError>(incidents)
        })
        .await
        .map_err(|e| HalconError::DatabaseError(e.to_string()))?
    }
}

#[derive(Debug)]
struct SupportIncident {
    incident_id: String,
    incident_type: String,
    priority: String,
    title: String,
    description: String,
    reporter: Option<String>,
    affected_component: Option<String>,
    reproducible: bool,
    reproduction_steps: Option<String>,
    error_message: Option<String>,
    stack_trace: Option<String>,
    resolution: Option<String>,
    status: String,
}

#[async_trait]
impl ContextSource for SupportServer {
    fn name(&self) -> &str {
        "support"
    }

    fn priority(&self) -> u32 {
        self.priority
    }

    async fn gather(&self, query: &ContextQuery) -> Result<Vec<ContextChunk>> {
        let incidents = self
            .fetch_incidents(query.user_message.as_deref())
            .await?;

        if incidents.is_empty() {
            return Ok(vec![]);
        }

        let mut chunks = Vec::new();
        let mut total_tokens = 0usize;

        for incident in incidents {
            let reporter_info = if let Some(reporter) = &incident.reporter {
                format!("Reporter: {}\n", reporter)
            } else {
                String::new()
            };

            let component_info = if let Some(component) = &incident.affected_component {
                format!("Component: {}\n", component)
            } else {
                String::new()
            };

            let reproducible_info = if incident.reproducible {
                "Reproducible: Yes\n"
            } else {
                "Reproducible: No\n"
            };

            // Truncate description to max 500 chars
            let description_preview = if incident.description.len() > 500 {
                format!("{}...", &incident.description[..500])
            } else {
                incident.description.clone()
            };

            let repro_steps = if let Some(steps) = &incident.reproduction_steps {
                // Truncate steps to max 400 chars
                let steps_preview = if steps.len() > 400 {
                    format!("{}...", &steps[..{ let mut _fcb = (400).min(steps.len()); while _fcb > 0 && !steps.is_char_boundary(_fcb) { _fcb -= 1; } _fcb }])
                } else {
                    steps.clone()
                };
                format!("Reproduction Steps:\n{}\n", steps_preview)
            } else {
                String::new()
            };

            let error_info = if let Some(err) = &incident.error_message {
                // Truncate error to max 300 chars
                let err_preview = if err.len() > 300 {
                    format!("{}...", &err[..{ let mut _fcb = (300).min(err.len()); while _fcb > 0 && !err.is_char_boundary(_fcb) { _fcb -= 1; } _fcb }])
                } else {
                    err.clone()
                };
                format!("Error: {}\n", err_preview)
            } else {
                String::new()
            };

            let resolution_info = if let Some(res) = &incident.resolution {
                // Truncate resolution to max 400 chars
                let res_preview = if res.len() > 400 {
                    format!("{}...", &res[..{ let mut _fcb = (400).min(res.len()); while _fcb > 0 && !res.is_char_boundary(_fcb) { _fcb -= 1; } _fcb }])
                } else {
                    res.clone()
                };
                format!("Resolution:\n{}\n", res_preview)
            } else {
                String::new()
            };

            let content = format!(
                "[Support Incident]\n\
                 Title: {}\n\
                 Type: {}\n\
                 Priority: {}\n\
                 Status: {}\n\
                 {}{}{}\
                 Description:\n{}\n\
                 {}{}{}", incident.title,
                incident.incident_type,
                incident.priority.to_uppercase(),
                incident.status,
                reporter_info,
                component_info,
                reproducible_info,
                description_preview,
                repro_steps,
                error_info,
                resolution_info
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
    async fn test_support_server_creation() {
        let db = Database::open_in_memory().unwrap();
        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = SupportServer::new(async_db, 60, 3500);
        assert_eq!(server.name(), "support");
        assert_eq!(server.priority(), 60);
        assert_eq!(server.sdlc_phase(), SdlcPhase::Support);
    }

    #[tokio::test]
    async fn test_gather_no_incidents() {
        let db = Database::open_in_memory().unwrap();
        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = SupportServer::new(async_db, 60, 3500);

        let query = ContextQuery {
            working_directory: "/tmp".to_string(),
            user_message: None,
            token_budget: 10000,
        };

        let chunks = server.gather(&query).await.unwrap();
        assert_eq!(chunks.len(), 0);
    }

    #[tokio::test]
    async fn test_gather_with_bug_report() {
        let db = Database::open_in_memory().unwrap();

        // Insert bug report
        db.conn().unwrap()
            .execute(
                "INSERT INTO support_incidents (incident_id, incident_type, priority, title, description, reporter, affected_component, reproducible, reproduction_steps, error_message, status, reported_at, created_at)
                 VALUES ('inc-001', 'bug_report', 'high', 'App crashes on startup', 'Application crashes immediately after launch', 'user@example.com', 'launcher', 1, '1. Start app\n2. Wait 2 seconds\n3. Crash occurs', 'NullPointerException at main.rs:42', 'triaged', 1739606400, 1739606400)",
                [],
            )
            .unwrap();

        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = SupportServer::new(async_db, 60, 3500);

        let query = ContextQuery {
            working_directory: "/tmp".to_string(),
            user_message: None,
            token_budget: 10000,
        };

        let chunks = server.gather(&query).await.unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.contains("App crashes on startup"));
        assert!(chunks[0].content.contains("Priority: HIGH"));
        assert!(chunks[0].content.contains("Type: bug_report"));
        assert!(chunks[0].content.contains("Reporter: user@example.com"));
        assert!(chunks[0].content.contains("Component: launcher"));
        assert!(chunks[0].content.contains("Reproducible: Yes"));
        assert!(chunks[0].content.contains("Reproduction Steps"));
        assert!(chunks[0].content.contains("NullPointerException"));
    }

    #[tokio::test]
    async fn test_gather_resolved_filtered() {
        let db = Database::open_in_memory().unwrap();

        // Insert resolved incident (should NOT be returned)
        db.conn().unwrap()
            .execute(
                "INSERT INTO support_incidents (incident_id, incident_type, priority, title, description, status, reported_at, created_at)
                 VALUES ('inc-002', 'feature_request', 'low', 'Add dark mode', 'Request for dark theme', 'resolved', 1739606400, 1739606400)",
                [],
            )
            .unwrap();

        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = SupportServer::new(async_db, 60, 3500);

        let query = ContextQuery {
            working_directory: "/tmp".to_string(),
            user_message: None,
            token_budget: 10000,
        };

        let chunks = server.gather(&query).await.unwrap();
        // Should return 0 because resolved incidents are filtered out
        assert_eq!(chunks.len(), 0);
    }

    #[tokio::test]
    async fn test_budget_enforcement() {
        let db = Database::open_in_memory().unwrap();

        // Insert incident
        db.conn().unwrap()
            .execute(
                "INSERT INTO support_incidents (incident_id, incident_type, priority, title, description, status, reported_at, created_at)
                 VALUES ('inc-003', 'crash_report', 'critical', 'Memory leak', 'Application uses excessive memory', 'new', 1739606400, 1739606400)",
                [],
            )
            .unwrap();

        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = SupportServer::new(async_db, 60, 100); // Very small budget

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
