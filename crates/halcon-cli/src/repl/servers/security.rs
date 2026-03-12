/// Security & Compliance Context Server (Server 7).
///
/// Provides context from security scans, vulnerability reports, CVE tracking,
/// compliance violations, and security audit results.
/// Phase: Security (cross-cutting)
/// Priority: 65
use async_trait::async_trait;
use halcon_context::estimate_tokens;
use halcon_core::error::{HalconError, Result};
use halcon_core::traits::{ContextChunk, ContextQuery, ContextSource};
use halcon_core::types::SdlcPhase;
use halcon_storage::AsyncDatabase;

pub struct SecurityServer {
    db: AsyncDatabase,
    priority: u32,
    token_budget: u32,
}

impl SecurityServer {
    pub fn new(db: AsyncDatabase, priority: u32, token_budget: u32) -> Self {
        Self {
            db,
            priority,
            token_budget,
        }
    }

    pub fn sdlc_phase(&self) -> SdlcPhase {
        // Security is cross-cutting, but maps to Review phase for ordering
        SdlcPhase::Review
    }

    async fn fetch_findings(&self, query: Option<&str>) -> Result<Vec<SecurityFinding>> {
        let db = self.db.clone();
        let query_opt = query.map(String::from);

        tokio::task::spawn_blocking(move || {
            let db_ref = db.inner();
            let conn = db_ref
                .conn()
                .map_err(|e| HalconError::DatabaseError(e.to_string()))?;

            let findings = if let Some(q) = query_opt {
                // FTS5 search
                let mut stmt = conn
                    .prepare(
                        "SELECT finding_id, finding_type, severity, title, description,
                            affected_file, affected_line, cve_id, cvss_score, remediation, status
                     FROM security_findings
                     WHERE finding_id IN (
                       SELECT rowid FROM security_findings_fts WHERE security_findings_fts MATCH ?
                     )
                     ORDER BY
                        CASE severity
                            WHEN 'critical' THEN 0
                            WHEN 'high' THEN 1
                            WHEN 'medium' THEN 2
                            WHEN 'low' THEN 3
                            WHEN 'info' THEN 4
                            ELSE 5
                        END
                     LIMIT 12",
                    )
                    .map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                let rows = stmt
                    .query_map([q], |row| {
                        Ok(SecurityFinding {
                            finding_id: row.get(0)?,
                            finding_type: row.get(1)?,
                            severity: row.get(2)?,
                            title: row.get(3)?,
                            description: row.get(4)?,
                            affected_file: row.get(5)?,
                            affected_line: row.get(6)?,
                            cve_id: row.get(7)?,
                            cvss_score: row.get(8)?,
                            remediation: row.get(9)?,
                            status: row.get(10)?,
                        })
                    })
                    .map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                rows.collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| HalconError::DatabaseError(e.to_string()))?
            } else {
                // No query → return critical/high severity open findings first
                let mut stmt = conn
                    .prepare(
                        "SELECT finding_id, finding_type, severity, title, description,
                            affected_file, affected_line, cve_id, cvss_score, remediation, status
                     FROM security_findings
                     WHERE status IN ('open', 'acknowledged', 'in_progress')
                     ORDER BY
                        CASE severity
                            WHEN 'critical' THEN 0
                            WHEN 'high' THEN 1
                            WHEN 'medium' THEN 2
                            WHEN 'low' THEN 3
                            WHEN 'info' THEN 4
                            ELSE 5
                        END
                     LIMIT 12",
                    )
                    .map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                let rows = stmt
                    .query_map([], |row| {
                        Ok(SecurityFinding {
                            finding_id: row.get(0)?,
                            finding_type: row.get(1)?,
                            severity: row.get(2)?,
                            title: row.get(3)?,
                            description: row.get(4)?,
                            affected_file: row.get(5)?,
                            affected_line: row.get(6)?,
                            cve_id: row.get(7)?,
                            cvss_score: row.get(8)?,
                            remediation: row.get(9)?,
                            status: row.get(10)?,
                        })
                    })
                    .map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                rows.collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| HalconError::DatabaseError(e.to_string()))?
            };

            Ok::<Vec<SecurityFinding>, HalconError>(findings)
        })
        .await
        .map_err(|e| HalconError::DatabaseError(e.to_string()))?
    }
}

#[derive(Debug)]
struct SecurityFinding {
    finding_id: String,
    finding_type: String,
    severity: String,
    title: String,
    description: String,
    affected_file: Option<String>,
    affected_line: Option<i64>,
    cve_id: Option<String>,
    cvss_score: Option<f64>,
    remediation: Option<String>,
    status: String,
}

#[async_trait]
impl ContextSource for SecurityServer {
    fn name(&self) -> &str {
        "security"
    }

    fn priority(&self) -> u32 {
        self.priority
    }

    async fn gather(&self, query: &ContextQuery) -> Result<Vec<ContextChunk>> {
        let findings = self.fetch_findings(query.user_message.as_deref()).await?;

        if findings.is_empty() {
            return Ok(vec![]);
        }

        let mut chunks = Vec::new();
        let mut total_tokens = 0usize;

        for finding in findings {
            let location_info = if let Some(file) = &finding.affected_file {
                let line_info = if let Some(line) = finding.affected_line {
                    format!(":L{}", line)
                } else {
                    String::new()
                };
                format!("Location: {}{}\n", file, line_info)
            } else {
                String::new()
            };

            let cve_info = if let Some(cve) = &finding.cve_id {
                let cvss_info = if let Some(score) = finding.cvss_score {
                    format!(" (CVSS: {:.1})", score)
                } else {
                    String::new()
                };
                format!("CVE: {}{}\n", cve, cvss_info)
            } else {
                String::new()
            };

            // Truncate description to max 600 chars
            let description_preview = if finding.description.len() > 600 {
                format!("{}...", &finding.description[..600])
            } else {
                finding.description.clone()
            };

            let remediation_info = if let Some(rem) = &finding.remediation {
                // Truncate remediation to max 500 chars
                let rem_preview = if rem.len() > 500 {
                    format!(
                        "{}...",
                        &rem[..{
                            let mut _fcb = (500).min(rem.len());
                            while _fcb > 0 && !rem.is_char_boundary(_fcb) {
                                _fcb -= 1;
                            }
                            _fcb
                        }]
                    )
                } else {
                    rem.clone()
                };
                format!("Remediation:\n{}\n", rem_preview)
            } else {
                String::new()
            };

            let content = format!(
                "[Security Finding]\n\
                 Title: {}\n\
                 Type: {}\n\
                 Severity: {}\n\
                 Status: {}\n\
                 {}{}\
                 Description:\n{}\n\
                 {}",
                finding.title,
                finding.finding_type,
                finding.severity.to_uppercase(),
                finding.status,
                location_info,
                cve_info,
                description_preview,
                remediation_info
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
    use std::sync::Arc;

    #[tokio::test]
    async fn test_security_server_creation() {
        let db = Database::open_in_memory().unwrap();
        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = SecurityServer::new(async_db, 65, 3000);
        assert_eq!(server.name(), "security");
        assert_eq!(server.priority(), 65);
        assert_eq!(server.sdlc_phase(), SdlcPhase::Review);
    }

    #[tokio::test]
    async fn test_gather_no_findings() {
        let db = Database::open_in_memory().unwrap();
        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = SecurityServer::new(async_db, 65, 3000);

        let query = ContextQuery {
            working_directory: "/tmp".to_string(),
            user_message: None,
            token_budget: 10000,
        };

        let chunks = server.gather(&query).await.unwrap();
        assert_eq!(chunks.len(), 0);
    }

    #[tokio::test]
    async fn test_gather_with_critical_finding() {
        let db = Database::open_in_memory().unwrap();

        // Insert critical security finding
        db.conn().unwrap()
            .execute(
                "INSERT INTO security_findings (finding_id, finding_type, severity, title, description, affected_file, affected_line, cve_id, cvss_score, remediation, status, detected_at, created_at)
                 VALUES ('sec-001', 'vulnerability', 'critical', 'SQL Injection', 'Unvalidated user input in query', 'src/api/users.rs', 42, 'CVE-2024-1234', 9.8, 'Use prepared statements', 'open', 1739606400, 1739606400)",
                [],
            )
            .unwrap();

        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = SecurityServer::new(async_db, 65, 3000);

        let query = ContextQuery {
            working_directory: "/tmp".to_string(),
            user_message: None,
            token_budget: 10000,
        };

        let chunks = server.gather(&query).await.unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.contains("SQL Injection"));
        assert!(chunks[0].content.contains("Severity: CRITICAL"));
        assert!(chunks[0].content.contains("CVE: CVE-2024-1234"));
        assert!(chunks[0].content.contains("CVSS: 9.8"));
        assert!(chunks[0].content.contains("Location: src/api/users.rs:L42"));
        assert!(chunks[0].content.contains("prepared statements"));
    }

    #[tokio::test]
    async fn test_gather_resolved_filtered() {
        let db = Database::open_in_memory().unwrap();

        // Insert resolved finding (should NOT be returned)
        db.conn().unwrap()
            .execute(
                "INSERT INTO security_findings (finding_id, finding_type, severity, title, description, status, detected_at, created_at)
                 VALUES ('sec-002', 'code_smell', 'low', 'Unused variable', 'Variable x is never used', 'resolved', 1739606400, 1739606400)",
                [],
            )
            .unwrap();

        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = SecurityServer::new(async_db, 65, 3000);

        let query = ContextQuery {
            working_directory: "/tmp".to_string(),
            user_message: None,
            token_budget: 10000,
        };

        let chunks = server.gather(&query).await.unwrap();
        // Should return 0 because resolved findings are filtered out
        assert_eq!(chunks.len(), 0);
    }

    #[tokio::test]
    async fn test_budget_enforcement() {
        let db = Database::open_in_memory().unwrap();

        // Insert finding
        db.conn().unwrap()
            .execute(
                "INSERT INTO security_findings (finding_id, finding_type, severity, title, description, status, detected_at, created_at)
                 VALUES ('sec-003', 'secret_leak', 'high', 'API Key Leak', 'API key found in source code', 'open', 1739606400, 1739606400)",
                [],
            )
            .unwrap();

        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = SecurityServer::new(async_db, 65, 100); // Very small budget

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
