/// Test Results & Coverage Context Server (Server 5).
///
/// Provides context from test execution results, coverage reports, test failures,
/// and historical test reliability data.
/// Phase: Testing
/// Priority: 75

use async_trait::async_trait;
use halcon_context::estimate_tokens;
use halcon_core::error::{HalconError, Result};
use halcon_core::traits::{ContextChunk, ContextQuery, ContextSource};
use halcon_core::types::SdlcPhase;
use halcon_storage::{AsyncDatabase, Database};
use std::sync::Arc;

pub struct TestResultsServer {
    db: AsyncDatabase,
    priority: u32,
    token_budget: u32,
}

impl TestResultsServer {
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

    async fn fetch_test_results(&self, query: Option<&str>) -> Result<Vec<TestResult>> {
        let db = self.db.clone();
        let query_opt = query.map(String::from);

        tokio::task::spawn_blocking(move || {
            let db_ref = db.inner();
            let conn = db_ref.conn()
                .map_err(|e| HalconError::DatabaseError(e.to_string()))?;

            let results = if let Some(q) = query_opt {
                // FTS5 search
                let mut stmt = conn.prepare(
                    "SELECT test_id, test_suite, test_name, test_file, status,
                            duration_ms, failure_message, stack_trace, coverage_percent,
                            assertions_count, run_at
                     FROM test_results
                     WHERE test_id IN (
                       SELECT rowid FROM test_results_fts WHERE test_results_fts MATCH ?
                     )
                     ORDER BY run_at DESC
                     LIMIT 15"
                ).map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                let rows = stmt.query_map([q], |row| {
                    Ok(TestResult {
                        test_id: row.get(0)?,
                        test_suite: row.get(1)?,
                        test_name: row.get(2)?,
                        test_file: row.get(3)?,
                        status: row.get(4)?,
                        duration_ms: row.get(5)?,
                        failure_message: row.get(6)?,
                        stack_trace: row.get(7)?,
                        coverage_percent: row.get(8)?,
                        assertions_count: row.get(9)?,
                        run_at: row.get(10)?,
                    })
                }).map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                rows.collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| HalconError::DatabaseError(e.to_string()))?
            } else {
                // No query → return recent test failures first, then recent successes
                let mut stmt = conn.prepare(
                    "SELECT test_id, test_suite, test_name, test_file, status,
                            duration_ms, failure_message, stack_trace, coverage_percent,
                            assertions_count, run_at
                     FROM test_results
                     ORDER BY
                        CASE status
                            WHEN 'failed' THEN 0
                            WHEN 'error' THEN 1
                            WHEN 'skipped' THEN 2
                            WHEN 'passed' THEN 3
                            ELSE 4
                        END,
                        run_at DESC
                     LIMIT 15"
                ).map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                let rows = stmt.query_map([], |row| {
                    Ok(TestResult {
                        test_id: row.get(0)?,
                        test_suite: row.get(1)?,
                        test_name: row.get(2)?,
                        test_file: row.get(3)?,
                        status: row.get(4)?,
                        duration_ms: row.get(5)?,
                        failure_message: row.get(6)?,
                        stack_trace: row.get(7)?,
                        coverage_percent: row.get(8)?,
                        assertions_count: row.get(9)?,
                        run_at: row.get(10)?,
                    })
                }).map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                rows.collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| HalconError::DatabaseError(e.to_string()))?
            };

            Ok::<Vec<TestResult>, HalconError>(results)
        })
        .await
        .map_err(|e| HalconError::DatabaseError(e.to_string()))?
    }
}

#[derive(Debug)]
struct TestResult {
    test_id: String,
    test_suite: String,
    test_name: String,
    test_file: String,
    status: String,
    duration_ms: Option<i64>,
    failure_message: Option<String>,
    stack_trace: Option<String>,
    coverage_percent: Option<f64>,
    assertions_count: Option<i64>,
    run_at: i64,
}

#[async_trait]
impl ContextSource for TestResultsServer {
    fn name(&self) -> &str {
        "test_results"
    }

    fn priority(&self) -> u32 {
        self.priority
    }

    async fn gather(&self, query: &ContextQuery) -> Result<Vec<ContextChunk>> {
        let results = self
            .fetch_test_results(query.user_message.as_deref())
            .await?;

        if results.is_empty() {
            return Ok(vec![]);
        }

        let mut chunks = Vec::new();
        let mut total_tokens = 0usize;

        for result in results {
            let duration_info = if let Some(duration) = result.duration_ms {
                format!(" | Duration: {}ms", duration)
            } else {
                String::new()
            };

            let coverage_info = if let Some(coverage) = result.coverage_percent {
                format!(" | Coverage: {:.1}%", coverage)
            } else {
                String::new()
            };

            let assertions_info = if let Some(assertions) = result.assertions_count {
                format!(" | Assertions: {}", assertions)
            } else {
                String::new()
            };

            // Include failure details for failed/error tests
            let failure_details = if result.status == "failed" || result.status == "error" {
                let mut details = String::new();
                if let Some(msg) = &result.failure_message {
                    // Truncate failure message to max 500 chars
                    let msg_preview = if msg.len() > 500 {
                        format!("{}...", &msg[..{ let mut _fcb = (500).min(msg.len()); while _fcb > 0 && !msg.is_char_boundary(_fcb) { _fcb -= 1; } _fcb }])
                    } else {
                        msg.clone()
                    };
                    details.push_str(&format!("Failure: {}\n", msg_preview));
                }
                if let Some(trace) = &result.stack_trace {
                    // Truncate stack trace to max 600 chars
                    let trace_preview = if trace.len() > 600 {
                        format!("{}...", &trace[..{ let mut _fcb = (600).min(trace.len()); while _fcb > 0 && !trace.is_char_boundary(_fcb) { _fcb -= 1; } _fcb }])
                    } else {
                        trace.clone()
                    };
                    details.push_str(&format!("Stack Trace:\n{}\n", trace_preview));
                }
                details
            } else {
                String::new()
            };

            let content = format!(
                "[Test Result Context]\n\
                 Test: {}\n\
                 Suite: {}\n\
                 File: {}\n\
                 Status: {}{}{}{}\n\
                 {}", result.test_name,
                result.test_suite,
                result.test_file,
                result.status.to_uppercase(),
                duration_info,
                coverage_info,
                assertions_info,
                failure_details
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
    async fn test_test_results_server_creation() {
        let db = Database::open_in_memory().unwrap();
        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = TestResultsServer::new(async_db, 75, 4000);
        assert_eq!(server.name(), "test_results");
        assert_eq!(server.priority(), 75);
        assert_eq!(server.sdlc_phase(), SdlcPhase::Testing);
    }

    #[tokio::test]
    async fn test_gather_no_results() {
        let db = Database::open_in_memory().unwrap();
        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = TestResultsServer::new(async_db, 75, 4000);

        let query = ContextQuery {
            working_directory: "/tmp".to_string(),
            user_message: None,
            token_budget: 10000,
        };

        let chunks = server.gather(&query).await.unwrap();
        assert_eq!(chunks.len(), 0);
    }

    #[tokio::test]
    async fn test_gather_with_passed_test() {
        let db = Database::open_in_memory().unwrap();

        // Insert test result (passed)
        db.conn().unwrap()
            .execute(
                "INSERT INTO test_results (test_id, test_suite, test_name, test_file, status, duration_ms, coverage_percent, assertions_count, run_at, created_at)
                 VALUES ('test-001', 'unit', 'test_addition', 'tests/math.rs', 'passed', 12, 95.5, 3, 1739606400, 1739606400)",
                [],
            )
            .unwrap();

        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = TestResultsServer::new(async_db, 75, 4000);

        let query = ContextQuery {
            working_directory: "/tmp".to_string(),
            user_message: None,
            token_budget: 10000,
        };

        let chunks = server.gather(&query).await.unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.contains("test_addition"));
        assert!(chunks[0].content.contains("PASSED"));
        assert!(chunks[0].content.contains("Duration: 12ms"));
        assert!(chunks[0].content.contains("Coverage: 95.5%"));
        assert!(chunks[0].content.contains("Assertions: 3"));
    }

    #[tokio::test]
    async fn test_gather_with_failed_test() {
        let db = Database::open_in_memory().unwrap();

        // Insert failed test with failure message
        db.conn().unwrap()
            .execute(
                "INSERT INTO test_results (test_id, test_suite, test_name, test_file, status, failure_message, stack_trace, run_at, created_at)
                 VALUES ('test-002', 'integration', 'test_api_endpoint', 'tests/api.rs', 'failed', 'AssertionError: expected 200, got 500', 'at tests/api.rs:42', 1739606400, 1739606400)",
                [],
            )
            .unwrap();

        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = TestResultsServer::new(async_db, 75, 4000);

        let query = ContextQuery {
            working_directory: "/tmp".to_string(),
            user_message: None,
            token_budget: 10000,
        };

        let chunks = server.gather(&query).await.unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.contains("test_api_endpoint"));
        assert!(chunks[0].content.contains("FAILED"));
        assert!(chunks[0].content.contains("expected 200, got 500"));
        assert!(chunks[0].content.contains("Stack Trace"));
    }

    #[tokio::test]
    async fn test_budget_enforcement() {
        let db = Database::open_in_memory().unwrap();

        // Insert test result
        db.conn().unwrap()
            .execute(
                "INSERT INTO test_results (test_id, test_suite, test_name, test_file, status, run_at, created_at)
                 VALUES ('test-003', 'unit', 'test_budget', 'tests/budget.rs', 'passed', 1739606400, 1739606400)",
                [],
            )
            .unwrap();

        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = TestResultsServer::new(async_db, 75, 100); // Very small budget

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
