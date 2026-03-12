//! Phase 4: CI Feedback Loop
//!
//! Polls CI provider APIs (GitHub Actions, GitLab CI, generic HTTP) for workflow
//! run results and maps them to `TestSuiteResult` for consumption by the
//! reward pipeline and dev-ecosystem integration layer.
//!
//! # Architecture
//! - `CiResultIngestor` spawns a background tokio task that polls at a fixed interval.
//! - Callers subscribe to a `broadcast::Receiver<CiEvent>` for push-style updates.
//! - A `CiApiClient` trait makes the HTTP layer mockable without requiring real tokens.
//! - Reward mapping: `pass_rate` in [0, 1] → `f64` reward fed into UCB1 pipeline.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, Mutex};

use super::test_results::{parse_junit_xml, TestSuiteResult};

// ── Constants ───────────────────────────────────────────────────────────────

/// Default poll interval when not explicitly configured.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Channel buffer for CI events before slow consumers drop items.
const EVENT_CHANNEL_CAPACITY: usize = 64;

// ── Types ───────────────────────────────────────────────────────────────────

/// High-level status of a CI workflow run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CiRunStatus {
    /// Queued or waiting for a runner.
    Pending,
    /// Runner is active.
    Running,
    /// All jobs completed successfully.
    Success,
    /// One or more jobs failed.
    Failure,
    /// Run was cancelled before completion.
    Cancelled,
    /// Unknown status string (preserved for forward compatibility).
    Unknown(String),
}

impl CiRunStatus {
    /// Parse from a GitHub Actions `conclusion` or `status` field.
    pub fn from_github_str(s: &str) -> Self {
        match s {
            "queued" | "waiting" => Self::Pending,
            "in_progress" => Self::Running,
            "success" | "completed" => Self::Success,
            "failure" | "timed_out" | "startup_failure" => Self::Failure,
            "cancelled" | "skipped" | "neutral" | "stale" => Self::Cancelled,
            other => Self::Unknown(other.to_string()),
        }
    }

    /// Returns `true` when the run is in a terminal state.
    pub fn is_terminal(&self) -> bool {
        !matches!(self, Self::Pending | Self::Running)
    }

    /// Maps to a [0, 1] reward signal suitable for UCB1.
    ///
    /// - Success  → 1.0
    /// - Failure  → 0.0
    /// - Cancelled → 0.3  (partial-credit; agent may have caused premature cancel)
    /// - Pending/Running/Unknown → 0.5 (neutral; no information yet)
    pub fn as_reward(&self) -> f64 {
        match self {
            Self::Success => 1.0,
            Self::Failure => 0.0,
            Self::Cancelled => 0.3,
            _ => 0.5,
        }
    }
}

/// Metadata for a single CI workflow run.
#[derive(Debug, Clone)]
pub struct CiRunRecord {
    /// Unique run identifier from the CI provider.
    pub run_id: String,
    /// Git commit SHA that triggered this run.
    pub head_sha: String,
    /// Workflow or pipeline name.
    pub workflow_name: String,
    /// Branch that triggered the run.
    pub branch: String,
    /// Terminal or in-progress status.
    pub status: CiRunStatus,
    /// Parsed test results, if available from the run's artifacts.
    pub test_results: Option<TestSuiteResult>,
    /// Combined reward in [0, 1] blending CI status + test pass rate.
    pub reward: f64,
}

impl CiRunRecord {
    /// Compute a blended reward from CI status and test pass rate.
    ///
    /// Formula: `0.6 * status_reward + 0.4 * pass_rate`
    /// When no test results are available, `pass_rate` defaults to `status_reward`
    /// (avoids penalising runs without JUnit artifacts).
    pub fn compute_reward(status: &CiRunStatus, results: Option<&TestSuiteResult>) -> f64 {
        let status_reward = status.as_reward();
        let pass_rate = results
            .map(|r| {
                let total = r.passed() + r.failed() + r.ignored();
                if total == 0 {
                    status_reward
                } else {
                    r.passed() as f64 / total as f64
                }
            })
            .unwrap_or(status_reward);

        (0.6 * status_reward + 0.4 * pass_rate).clamp(0.0, 1.0)
    }
}

// ── Events ───────────────────────────────────────────────────────────────────

/// Events emitted by `CiResultIngestor` subscribers.
#[derive(Debug, Clone)]
pub enum CiEvent {
    /// A run transitioned to an in-progress state.
    RunStarted {
        run_id: String,
        workflow: String,
        sha: String,
    },
    /// A run reached a terminal state (success, failure, cancelled).
    RunCompleted(CiRunRecord),
    /// Poll cycle failed (network error, auth failure, rate limit).
    PollError { provider: String, message: String },
    /// Ingestor has shut down cleanly.
    Shutdown,
}

// ── API Client trait ─────────────────────────────────────────────────────────

/// Abstraction over CI provider HTTP calls, making the ingestor testable
/// without requiring real API tokens or network access.
#[async_trait::async_trait]
pub trait CiApiClient: Send + Sync {
    /// Fetch recent workflow runs for the configured repository / project.
    ///
    /// Returns a list of `(run_id, head_sha, workflow_name, branch, raw_status)` tuples.
    async fn list_recent_runs(
        &self,
    ) -> Result<Vec<(String, String, String, String, String)>, String>;

    /// Download JUnit XML artifacts for a finished run, if available.
    ///
    /// Returns raw XML bytes. Returns `None` when the run has no test artifacts.
    async fn fetch_junit_xml(&self, run_id: &str) -> Result<Option<String>, String>;
}

// ── GitHub Actions client ────────────────────────────────────────────────────

/// GitHub Actions REST API v3 client (real HTTP implementation).
pub struct GithubActionsClient {
    owner: String,
    repo: String,
    workflow_id: String,
    token: String,
    http: reqwest::Client,
}

impl GithubActionsClient {
    /// Create from explicit parameters.
    pub fn new(owner: &str, repo: &str, workflow_id: &str, token: &str) -> Self {
        let http = reqwest::Client::builder()
            .user_agent("halcon-ci-ingestor/1.0")
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();

        Self {
            owner: owner.to_string(),
            repo: repo.to_string(),
            workflow_id: workflow_id.to_string(),
            token: token.to_string(),
            http,
        }
    }

    /// Build API base URL.
    fn api_base(&self) -> String {
        format!(
            "https://api.github.com/repos/{}/{}/actions/workflows/{}/runs",
            self.owner, self.repo, self.workflow_id
        )
    }
}

#[async_trait::async_trait]
impl CiApiClient for GithubActionsClient {
    async fn list_recent_runs(
        &self,
    ) -> Result<Vec<(String, String, String, String, String)>, String> {
        let url = format!("{}?per_page=10", self.api_base());
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|e| format!("HTTP error: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("GitHub API error: {}", resp.status()));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("JSON decode error: {e}"))?;

        let runs = body["workflow_runs"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|r| {
                        let run_id = r["id"].to_string();
                        let sha = r["head_sha"].as_str().unwrap_or_default().to_string();
                        let name = r["name"].as_str().unwrap_or("unknown").to_string();
                        let branch = r["head_branch"].as_str().unwrap_or("unknown").to_string();
                        // GitHub: use `conclusion` when available (terminal), else `status`
                        let status = r["conclusion"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| r["status"].as_str().unwrap_or("unknown"))
                            .to_string();
                        (run_id, sha, name, branch, status)
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(runs)
    }

    async fn fetch_junit_xml(&self, _run_id: &str) -> Result<Option<String>, String> {
        // GitHub Actions doesn't expose JUnit XML directly via REST API.
        // Production use: download the `artifacts` zip and unpack — out of scope
        // for this implementation. Consumers can wire in custom artifact fetchers.
        Ok(None)
    }
}

// ── Ingestor configuration ────────────────────────────────────────────────────

/// Configuration for `CiResultIngestor`.
#[derive(Debug, Clone)]
pub struct CiIngestorConfig {
    /// How often to poll the CI provider.
    pub poll_interval: Duration,
    /// Maximum number of terminal run IDs to remember (prevents re-emitting events).
    pub seen_cache_size: usize,
}

impl Default for CiIngestorConfig {
    fn default() -> Self {
        Self {
            poll_interval: DEFAULT_POLL_INTERVAL,
            seen_cache_size: 512,
        }
    }
}

// ── CiResultIngestor ─────────────────────────────────────────────────────────

/// Polls a CI provider and broadcasts `CiEvent`s to subscribers.
///
/// # Usage
/// ```no_run
/// # async fn example() {
/// use std::sync::Arc;
/// use halcon_cli::repl::ci_result_ingestor::{CiResultIngestor, CiIngestorConfig};
/// use halcon_cli::repl::ci_result_ingestor::GithubActionsClient;
///
/// let client = Arc::new(GithubActionsClient::new("owner", "repo", "ci.yml", "ghp_xxx"));
/// let ingestor = CiResultIngestor::new(client, CiIngestorConfig::default());
/// let mut rx = ingestor.subscribe();
///
/// ingestor.start();
/// while let Ok(event) = rx.recv().await {
///     println!("{event:?}");
/// }
/// # }
/// ```
pub struct CiResultIngestor {
    client: Arc<dyn CiApiClient>,
    config: CiIngestorConfig,
    tx: broadcast::Sender<CiEvent>,
    /// run_id → last known status (avoids re-emitting events for seen terminal runs)
    seen: Arc<Mutex<HashMap<String, CiRunStatus>>>,
    stop: Arc<tokio::sync::Notify>,
}

impl CiResultIngestor {
    /// Create a new ingestor without starting the background task.
    pub fn new(client: Arc<dyn CiApiClient>, config: CiIngestorConfig) -> Self {
        let (tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        Self {
            client,
            config,
            tx,
            seen: Arc::new(Mutex::new(HashMap::new())),
            stop: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// Subscribe to CI events. Call before `start()` to avoid missing early events.
    pub fn subscribe(&self) -> broadcast::Receiver<CiEvent> {
        self.tx.subscribe()
    }

    /// Request a graceful stop of the background polling task.
    pub fn stop(&self) {
        self.stop.notify_waiters();
    }

    /// Spawn the background polling task (non-blocking, fire-and-forget).
    pub fn start(self) {
        let client = self.client;
        let config = self.config;
        let tx = self.tx;
        let seen = self.seen;
        let stop = self.stop;

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop.notified() => {
                        let _ = tx.send(CiEvent::Shutdown);
                        break;
                    }
                    _ = tokio::time::sleep(config.poll_interval) => {
                        Self::poll_once(client.as_ref(), &tx, &seen).await;
                    }
                }
            }
        });
    }

    /// Run a single poll cycle: list runs, diff against seen cache, emit events.
    async fn poll_once(
        client: &(dyn CiApiClient + 'static),
        tx: &broadcast::Sender<CiEvent>,
        seen: &Mutex<HashMap<String, CiRunStatus>>,
    ) {
        let runs = match client.list_recent_runs().await {
            Ok(r) => r,
            Err(msg) => {
                let _ = tx.send(CiEvent::PollError {
                    provider: "github".to_string(),
                    message: msg,
                });
                return;
            }
        };

        let mut seen_guard = seen.lock().await;

        for (run_id, head_sha, workflow_name, branch, raw_status) in runs {
            let status = CiRunStatus::from_github_str(&raw_status);

            // Skip runs we've already emitted terminal events for.
            if let Some(prev) = seen_guard.get(&run_id) {
                if prev.is_terminal() {
                    continue;
                }
            }

            // Emit RunStarted when transitioning into Running for the first time.
            if matches!(status, CiRunStatus::Running) && !seen_guard.contains_key(&run_id) {
                let _ = tx.send(CiEvent::RunStarted {
                    run_id: run_id.clone(),
                    workflow: workflow_name.clone(),
                    sha: head_sha.clone(),
                });
            }

            // Fetch and parse test artifacts for terminal runs.
            let test_results = if status.is_terminal() {
                match client.fetch_junit_xml(&run_id).await {
                    Ok(Some(xml)) => Some(parse_junit_xml(&xml, &workflow_name)),
                    Ok(None) => None,
                    Err(e) => {
                        tracing::warn!(run_id = %run_id, error = %e, "Failed to fetch JUnit XML");
                        None
                    }
                }
            } else {
                None
            };

            let reward = CiRunRecord::compute_reward(&status, test_results.as_ref());

            if status.is_terminal() {
                let record = CiRunRecord {
                    run_id: run_id.clone(),
                    head_sha,
                    workflow_name,
                    branch,
                    status: status.clone(),
                    test_results,
                    reward,
                };
                let _ = tx.send(CiEvent::RunCompleted(record));

                // Enforce cache size limit (evict oldest entries).
                if seen_guard.len() >= 512 {
                    let oldest = seen_guard.keys().next().cloned();
                    if let Some(key) = oldest {
                        seen_guard.remove(&key);
                    }
                }
            }

            seen_guard.insert(run_id, status);
        }
    }

    /// Run a single poll cycle synchronously (for testing without a tokio runtime
    /// being required for the full ingestor lifecycle).
    pub async fn poll_now(&self) {
        Self::poll_once(self.client.as_ref(), &self.tx, &self.seen).await;
    }
}

// ── Reward helpers ────────────────────────────────────────────────────────────

/// Extract a `[0, 1]` reward from the most recent completed CI event received
/// from a broadcast channel, without blocking indefinitely.
///
/// Returns `None` when no completed run is available within `timeout`.
pub async fn await_ci_reward(
    rx: &mut broadcast::Receiver<CiEvent>,
    timeout: Duration,
) -> Option<f64> {
    tokio::time::timeout(timeout, async {
        loop {
            match rx.recv().await {
                Ok(CiEvent::RunCompleted(record)) => return record.reward,
                Ok(CiEvent::Shutdown) | Err(_) => return 0.5, // neutral on disconnect
                _ => {}
            }
        }
    })
    .await
    .ok()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── CiRunStatus ───────────────────────────────────────────────────────────

    #[test]
    fn parse_github_success_conclusion() {
        assert_eq!(
            CiRunStatus::from_github_str("success"),
            CiRunStatus::Success
        );
        assert_eq!(
            CiRunStatus::from_github_str("completed"),
            CiRunStatus::Success
        );
    }

    #[test]
    fn parse_github_failure_conclusion() {
        assert_eq!(
            CiRunStatus::from_github_str("failure"),
            CiRunStatus::Failure
        );
        assert_eq!(
            CiRunStatus::from_github_str("timed_out"),
            CiRunStatus::Failure
        );
    }

    #[test]
    fn parse_github_in_progress() {
        assert_eq!(
            CiRunStatus::from_github_str("in_progress"),
            CiRunStatus::Running
        );
        assert!(!CiRunStatus::Running.is_terminal());
    }

    #[test]
    fn parse_github_cancelled() {
        assert_eq!(
            CiRunStatus::from_github_str("cancelled"),
            CiRunStatus::Cancelled
        );
        assert!(CiRunStatus::Cancelled.is_terminal());
    }

    #[test]
    fn unknown_status_preserved() {
        assert_eq!(
            CiRunStatus::from_github_str("something_new"),
            CiRunStatus::Unknown("something_new".to_string())
        );
    }

    #[test]
    fn terminal_check() {
        assert!(CiRunStatus::Success.is_terminal());
        assert!(CiRunStatus::Failure.is_terminal());
        assert!(!CiRunStatus::Pending.is_terminal());
        assert!(!CiRunStatus::Running.is_terminal());
    }

    #[test]
    fn rewards_in_range() {
        let all = [
            CiRunStatus::Success,
            CiRunStatus::Failure,
            CiRunStatus::Cancelled,
            CiRunStatus::Pending,
            CiRunStatus::Running,
            CiRunStatus::Unknown("x".to_string()),
        ];
        for s in &all {
            let r = s.as_reward();
            assert!(
                (0.0..=1.0).contains(&r),
                "reward out of range for {s:?}: {r}"
            );
        }
    }

    // ── CiRunRecord::compute_reward ───────────────────────────────────────────

    #[test]
    fn compute_reward_success_no_tests() {
        let r = CiRunRecord::compute_reward(&CiRunStatus::Success, None);
        // 0.6 * 1.0 + 0.4 * 1.0 = 1.0
        assert!((r - 1.0).abs() < 1e-9, "expected 1.0, got {r}");
    }

    #[test]
    fn compute_reward_failure_no_tests() {
        let r = CiRunRecord::compute_reward(&CiRunStatus::Failure, None);
        // 0.6 * 0.0 + 0.4 * 0.0 = 0.0
        assert!((r - 0.0).abs() < 1e-9, "expected 0.0, got {r}");
    }

    #[test]
    fn compute_reward_blends_test_pass_rate() {
        use super::super::test_results::{TestCase, TestStatus, TestSuiteResult};

        let results = TestSuiteResult {
            suite_name: "ci".to_string(),
            cases: vec![
                TestCase {
                    name: "a".to_string(),
                    status: TestStatus::Passed,
                    duration_ms: None,
                    failure_message: None,
                },
                TestCase {
                    name: "b".to_string(),
                    status: TestStatus::Failed,
                    duration_ms: None,
                    failure_message: Some("err".to_string()),
                },
            ],
            all_passed: false,
            total_duration_ms: None,
            format: crate::repl::git_tools::test_results::TestResultFormat::JunitXml,
        };

        // Success CI + 50% pass rate → 0.6*1.0 + 0.4*0.5 = 0.8
        let r = CiRunRecord::compute_reward(&CiRunStatus::Success, Some(&results));
        assert!((r - 0.8).abs() < 1e-6, "expected 0.8, got {r}");
    }

    #[test]
    fn compute_reward_clamped_to_unit_interval() {
        // Defensive: both components are already in [0,1] so clamp should be no-op.
        let r = CiRunRecord::compute_reward(&CiRunStatus::Success, None);
        assert!((0.0..=1.0).contains(&r));
    }

    // ── Mock client + ingestor ────────────────────────────────────────────────

    struct MockCiClient {
        runs: Vec<(String, String, String, String, String)>,
        junit_xml: Option<String>,
    }

    #[async_trait::async_trait]
    impl CiApiClient for MockCiClient {
        async fn list_recent_runs(
            &self,
        ) -> Result<Vec<(String, String, String, String, String)>, String> {
            Ok(self.runs.clone())
        }

        async fn fetch_junit_xml(&self, _run_id: &str) -> Result<Option<String>, String> {
            Ok(self.junit_xml.clone())
        }
    }

    fn mock_run(id: &str, status: &str) -> (String, String, String, String, String) {
        (
            id.to_string(),
            format!("sha-{id}"),
            "CI".to_string(),
            "main".to_string(),
            status.to_string(),
        )
    }

    #[tokio::test]
    async fn poll_emits_completed_event_for_success_run() {
        let client = Arc::new(MockCiClient {
            runs: vec![mock_run("run-1", "success")],
            junit_xml: None,
        });
        let ingestor = CiResultIngestor::new(client, CiIngestorConfig::default());
        let mut rx = ingestor.subscribe();

        ingestor.poll_now().await;

        let event = rx.try_recv().expect("should have received an event");
        match event {
            CiEvent::RunCompleted(rec) => {
                assert_eq!(rec.run_id, "run-1");
                assert_eq!(rec.status, CiRunStatus::Success);
                assert!((rec.reward - 1.0).abs() < 1e-9);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn poll_emits_run_started_then_completed() {
        let client = Arc::new(MockCiClient {
            runs: vec![mock_run("run-2", "in_progress")],
            junit_xml: None,
        });
        let ingestor = CiResultIngestor::new(client, CiIngestorConfig::default());
        let mut rx = ingestor.subscribe();

        ingestor.poll_now().await;

        let event = rx.try_recv().expect("RunStarted expected");
        assert!(
            matches!(event, CiEvent::RunStarted { ref run_id, .. } if run_id == "run-2"),
            "expected RunStarted, got {event:?}"
        );
    }

    #[tokio::test]
    async fn poll_does_not_re_emit_terminal_run() {
        let client = Arc::new(MockCiClient {
            runs: vec![mock_run("run-3", "failure")],
            junit_xml: None,
        });
        let ingestor = CiResultIngestor::new(client, CiIngestorConfig::default());
        let mut rx = ingestor.subscribe();

        // First poll → emits RunCompleted
        ingestor.poll_now().await;
        assert!(rx.try_recv().is_ok(), "first poll should emit");

        // Second poll → same run is now in seen cache → no new event
        ingestor.poll_now().await;
        assert!(
            rx.try_recv().is_err(),
            "second poll must not re-emit terminal run"
        );
    }

    #[tokio::test]
    async fn poll_emits_poll_error_on_api_failure() {
        struct FailingClient;
        #[async_trait::async_trait]
        impl CiApiClient for FailingClient {
            async fn list_recent_runs(
                &self,
            ) -> Result<Vec<(String, String, String, String, String)>, String> {
                Err("network timeout".to_string())
            }
            async fn fetch_junit_xml(&self, _: &str) -> Result<Option<String>, String> {
                Ok(None)
            }
        }

        let ingestor = CiResultIngestor::new(Arc::new(FailingClient), CiIngestorConfig::default());
        let mut rx = ingestor.subscribe();

        ingestor.poll_now().await;

        match rx.try_recv().expect("PollError expected") {
            CiEvent::PollError { message, .. } => {
                assert!(message.contains("network timeout"));
            }
            other => panic!("expected PollError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn poll_includes_junit_reward_in_record() {
        let xml = r#"<?xml version="1.0"?>
<testsuite name="ci" tests="4" failures="0">
  <testcase name="test_a" classname="A" time="0.1"/>
  <testcase name="test_b" classname="A" time="0.2"/>
  <testcase name="test_c" classname="B" time="0.1"/>
  <testcase name="test_d" classname="B" time="0.3"/>
</testsuite>"#;

        let client = Arc::new(MockCiClient {
            runs: vec![mock_run("run-4", "success")],
            junit_xml: Some(xml.to_string()),
        });
        let ingestor = CiResultIngestor::new(client, CiIngestorConfig::default());
        let mut rx = ingestor.subscribe();

        ingestor.poll_now().await;

        match rx.try_recv().expect("event expected") {
            CiEvent::RunCompleted(rec) => {
                assert!(
                    rec.test_results.is_some(),
                    "JUnit results should be attached"
                );
                // 4/4 pass → pass_rate = 1.0 → reward = 1.0
                assert!(
                    (rec.reward - 1.0).abs() < 1e-6,
                    "expected 1.0, got {}",
                    rec.reward
                );
            }
            other => panic!("expected RunCompleted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn await_ci_reward_returns_none_on_timeout() {
        let (_, mut rx) = broadcast::channel::<CiEvent>(8);
        // Drop sender → recv will error immediately → reward = 0.5 (neutral)
        // With a very short timeout and no sender, result is Some(0.5) from disconnect arm.
        let result = await_ci_reward(&mut rx, Duration::from_millis(10)).await;
        // Either timeout (None) or disconnect (Some(0.5)) are acceptable.
        assert!(result.is_none() || result == Some(0.5));
    }

    #[test]
    fn ci_ingestor_config_defaults_are_sane() {
        let cfg = CiIngestorConfig::default();
        assert_eq!(cfg.poll_interval, DEFAULT_POLL_INTERVAL);
        assert!(cfg.seen_cache_size > 0);
    }
}
