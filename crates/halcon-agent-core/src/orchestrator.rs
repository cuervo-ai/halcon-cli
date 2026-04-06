//! DagOrchestrator — DAG-based multi-agent task decomposition.
//!
//! ## Role in GDEM
//!
//! When a goal requires parallel sub-tasks (e.g., scan multiple directories
//! concurrently, run linting + tests in parallel), the orchestrator decomposes
//! the [`PlanTree`] into a Directed Acyclic Graph of [`SubTask`]s and executes
//! them in topological waves via Tokio.
//!
//! ## Design invariants
//!
//! - Each [`SubTask`] has an explicit dependency list.
//! - Tasks with no pending dependencies form a "wave" and run concurrently.
//! - A task's output is injected into dependents' context before they start.
//! - Budget (token spend + wall time) is tracked via atomic counters shared
//!   across all concurrent sub-tasks.

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::Duration;
use tracing::{debug, warn};
use uuid::Uuid;

// ─── SubTask ──────────────────────────────────────────────────────────────────

/// Status of a single sub-task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubTaskStatus {
    Pending,
    Running,
    Completed,
    Failed(String),
}

/// A single unit of work in the orchestration DAG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubTask {
    pub id: Uuid,
    pub label: String,
    /// IDs of tasks that must complete before this one can start.
    pub depends_on: Vec<Uuid>,
    /// Natural-language description of what this sub-task should do.
    pub objective: String,
    /// Tools this sub-task is allowed to use (subset of registered tools).
    pub allowed_tools: Vec<String>,
    /// Timeout for this sub-task.
    pub timeout_secs: u64,
    pub status: SubTaskStatus,
    pub result: Option<SubTaskResult>,
}

impl SubTask {
    pub fn new(label: impl Into<String>, objective: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            label: label.into(),
            depends_on: Vec::new(),
            objective: objective.into(),
            allowed_tools: Vec::new(),
            timeout_secs: 60,
            status: SubTaskStatus::Pending,
            result: None,
        }
    }

    pub fn with_deps(mut self, deps: Vec<Uuid>) -> Self {
        self.depends_on = deps;
        self
    }

    pub fn with_tools(mut self, tools: Vec<String>) -> Self {
        self.allowed_tools = tools;
        self
    }

    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    pub fn is_ready(&self, completed: &HashSet<Uuid>) -> bool {
        self.status == SubTaskStatus::Pending
            && self.depends_on.iter().all(|dep| completed.contains(dep))
    }
}

/// Output produced by a completed sub-task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubTaskResult {
    pub task_id: Uuid,
    pub output_text: String,
    pub tools_used: Vec<String>,
    pub tokens_consumed: u64,
    pub duration_ms: u64,
    pub succeeded: bool,
    pub completed_at: DateTime<Utc>,
}

// ─── SubTaskExecutor trait ────────────────────────────────────────────────────

/// Trait implemented by the caller to actually execute sub-tasks.
///
/// In production this wraps `run_gdem_loop`. In tests, a mock can be provided.
#[async_trait]
pub trait SubTaskExecutor: Send + Sync {
    async fn execute(&self, task: &SubTask, context: &str) -> Result<SubTaskResult>;
}

// ─── OrchestratorConfig ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    /// Maximum concurrent sub-tasks per wave.
    pub max_concurrency: usize,
    /// Default timeout for sub-tasks (can be overridden per task).
    pub default_timeout_secs: u64,
    /// Maximum total token budget across all sub-tasks.
    pub max_total_tokens: u64,
    /// Whether to inject predecessor outputs into successor context.
    pub inject_predecessor_outputs: bool,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            max_concurrency: 4,
            default_timeout_secs: 60,
            max_total_tokens: 100_000,
            inject_predecessor_outputs: true,
        }
    }
}

// ─── OrchestratorResult ───────────────────────────────────────────────────────

#[derive(Debug)]
pub struct OrchestratorResult {
    pub session_id: Uuid,
    pub tasks_completed: usize,
    pub tasks_failed: usize,
    pub total_tokens: u64,
    pub total_duration_ms: u64,
    pub results: Vec<SubTaskResult>,
}

// ─── DagOrchestrator ──────────────────────────────────────────────────────────

/// Topological-wave DAG executor for multi-agent task decomposition.
pub struct DagOrchestrator {
    config: OrchestratorConfig,
    executor: Arc<dyn SubTaskExecutor>,
    total_tokens: Arc<AtomicU64>,
}

impl DagOrchestrator {
    pub fn new(executor: Arc<dyn SubTaskExecutor>, config: OrchestratorConfig) -> Self {
        Self {
            config,
            executor,
            total_tokens: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Execute a DAG of sub-tasks in topological waves.
    ///
    /// Tasks with no pending dependencies run concurrently (up to `max_concurrency`).
    /// Each wave's outputs are aggregated and made available to the next wave.
    pub async fn execute(&self, tasks: Vec<SubTask>) -> Result<OrchestratorResult> {
        let start = std::time::Instant::now();
        let session_id = Uuid::new_v4();
        self.total_tokens.store(0, Ordering::Relaxed);

        // Validate DAG (detect cycles).
        Self::validate_dag(&tasks)?;

        let tasks = Arc::new(Mutex::new(tasks));
        let completed: Arc<Mutex<HashSet<Uuid>>> = Arc::new(Mutex::new(HashSet::new()));
        let results: Arc<Mutex<Vec<SubTaskResult>>> = Arc::new(Mutex::new(Vec::new()));
        let mut all_outputs: HashMap<Uuid, String> = HashMap::new();

        loop {
            // Find all ready tasks.
            let wave: Vec<SubTask> = {
                let ts = tasks.lock().await;
                let done = completed.lock().await;
                ts.iter().filter(|t| t.is_ready(&done)).cloned().collect()
            };

            if wave.is_empty() {
                let mut ts = tasks.lock().await;
                let _done = completed.lock().await;
                let pending_count = ts
                    .iter()
                    .filter(|t| t.status == SubTaskStatus::Pending)
                    .count();
                if pending_count > 0 {
                    warn!(
                        pending_count,
                        "B6: DAG execution stalled — tasks pending but none ready (dependency failure)"
                    );
                    // Mark stalled tasks as Failed so callers get explicit error instead of
                    // silently missing results.
                    for t in ts.iter_mut() {
                        if t.status == SubTaskStatus::Pending {
                            t.status = SubTaskStatus::Failed(
                                "Dependency stall — prerequisite task failed or was never completed"
                                    .into(),
                            );
                        }
                    }
                }
                break;
            }

            // B6 remediation: Check token budget with explicit failure semantics.
            // BEFORE: silent `break` left remaining tasks as Pending forever.
            // AFTER: mark remaining tasks as Failed(BudgetExhausted) and log clearly.
            if self.total_tokens.load(Ordering::Acquire) >= self.config.max_total_tokens {
                let consumed = self.total_tokens.load(Ordering::Acquire);
                warn!(
                    consumed,
                    limit = self.config.max_total_tokens,
                    "B6: orchestrator token budget exhausted — marking remaining tasks as failed"
                );
                // Mark all Pending tasks as Failed so they don't hang indefinitely.
                {
                    let mut ts = tasks.lock().await;
                    for t in ts.iter_mut() {
                        if t.status == SubTaskStatus::Pending {
                            t.status = SubTaskStatus::Failed(format!(
                                "Budget exhausted ({consumed}/{} tokens) — task never started",
                                self.config.max_total_tokens
                            ));
                        }
                    }
                }
                break;
            }

            debug!(wave_size = wave.len(), "Orchestrator starting wave");

            // Build context string for this wave (predecessor outputs).
            let context = if self.config.inject_predecessor_outputs {
                self.build_context(&wave, &all_outputs)
            } else {
                String::new()
            };

            // Mark wave tasks as running.
            {
                let mut ts = tasks.lock().await;
                let wave_ids: HashSet<Uuid> = wave.iter().map(|t| t.id).collect();
                for t in ts.iter_mut() {
                    if wave_ids.contains(&t.id) {
                        t.status = SubTaskStatus::Running;
                    }
                }
            }

            // Execute wave concurrently (up to max_concurrency chunks).
            let wave_results: Vec<(Uuid, Result<SubTaskResult>)> = {
                let executor = self.executor.clone();
                let context = context.clone();

                // Process in chunks to respect max_concurrency.
                let mut all_wave_results = Vec::new();
                for chunk in wave.chunks(self.config.max_concurrency) {
                    let chunk_futures: Vec<_> = chunk
                        .iter()
                        .map(|task| {
                            let exec = executor.clone();
                            let ctx = context.clone();
                            let task = task.clone();
                            async move {
                                let id = task.id;
                                let timeout = Duration::from_secs(task.timeout_secs);
                                let result =
                                    tokio::time::timeout(timeout, exec.execute(&task, &ctx)).await;
                                let outcome = match result {
                                    Ok(Ok(r)) => Ok(r),
                                    Ok(Err(e)) => Err(e),
                                    Err(_) => Err(anyhow::anyhow!(
                                        "Sub-task '{}' timed out after {}s",
                                        task.label,
                                        task.timeout_secs
                                    )),
                                };
                                (id, outcome)
                            }
                        })
                        .collect();

                    let chunk_results = futures::future::join_all(chunk_futures).await;
                    all_wave_results.extend(chunk_results);
                }
                all_wave_results
            };

            // Process results.
            let mut tasks_guard = tasks.lock().await;
            let mut done_guard = completed.lock().await;
            let mut results_guard = results.lock().await;

            for (task_id, outcome) in wave_results {
                match outcome {
                    Ok(result) => {
                        self.total_tokens
                            .fetch_add(result.tokens_consumed, Ordering::Relaxed);
                        all_outputs.insert(task_id, result.output_text.clone());

                        // Update task status.
                        if let Some(t) = tasks_guard.iter_mut().find(|t| t.id == task_id) {
                            t.status = SubTaskStatus::Completed;
                            t.result = Some(result.clone());
                        }
                        done_guard.insert(task_id);
                        results_guard.push(result);
                    }
                    Err(e) => {
                        warn!(task_id = %task_id, error = %e, "Sub-task failed");
                        if let Some(t) = tasks_guard.iter_mut().find(|t| t.id == task_id) {
                            t.status = SubTaskStatus::Failed(e.to_string());
                        }
                        done_guard.insert(task_id); // mark done to unblock dependents
                    }
                }
            }
        }

        let results_vec = results.lock().await.clone();
        let tasks_final = tasks.lock().await;
        let failed_count = tasks_final
            .iter()
            .filter(|t| matches!(t.status, SubTaskStatus::Failed(_)))
            .count();

        Ok(OrchestratorResult {
            session_id,
            tasks_completed: results_vec.len(),
            tasks_failed: failed_count,
            total_tokens: self.total_tokens.load(Ordering::Relaxed),
            total_duration_ms: start.elapsed().as_millis() as u64,
            results: results_vec,
        })
    }

    /// Validate that the DAG has no cycles (Kahn's algorithm).
    fn validate_dag(tasks: &[SubTask]) -> Result<()> {
        let id_set: HashSet<Uuid> = tasks.iter().map(|t| t.id).collect();

        // Check all deps reference known tasks.
        for task in tasks {
            for dep in &task.depends_on {
                if !id_set.contains(dep) {
                    return Err(anyhow::anyhow!(
                        "Task '{}' depends on unknown task ID {}",
                        task.label,
                        dep
                    ));
                }
            }
        }

        // Kahn's algorithm for cycle detection.
        let mut in_degree: HashMap<Uuid, usize> =
            tasks.iter().map(|t| (t.id, t.depends_on.len())).collect();
        let mut queue: Vec<Uuid> = in_degree
            .iter()
            .filter(|(_, &d)| d == 0)
            .map(|(k, _)| *k)
            .collect();
        let mut processed = 0;

        // Build reverse adjacency for cycle detection.
        let mut reverse_adj: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
        for task in tasks {
            for dep in &task.depends_on {
                reverse_adj.entry(*dep).or_default().push(task.id);
            }
        }

        while let Some(node) = queue.pop() {
            processed += 1;
            if let Some(dependents) = reverse_adj.get(&node) {
                for dep in dependents {
                    let d = in_degree.get_mut(dep).unwrap();
                    *d -= 1;
                    if *d == 0 {
                        queue.push(*dep);
                    }
                }
            }
        }

        if processed != tasks.len() {
            return Err(anyhow::anyhow!("DAG contains a cycle — cannot execute"));
        }

        Ok(())
    }

    fn build_context(&self, wave: &[SubTask], prior_outputs: &HashMap<Uuid, String>) -> String {
        let mut ctx = String::new();
        for task in wave {
            for dep_id in &task.depends_on {
                if let Some(output) = prior_outputs.get(dep_id) {
                    ctx.push_str(&format!(
                        "=== Output from predecessor {} ===\n{}\n\n",
                        dep_id, output
                    ));
                }
            }
        }
        ctx
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    struct MockExecutor {
        output: String,
    }

    #[async_trait]
    impl SubTaskExecutor for MockExecutor {
        async fn execute(&self, task: &SubTask, _ctx: &str) -> Result<SubTaskResult> {
            Ok(SubTaskResult {
                task_id: task.id,
                output_text: self.output.clone(),
                tools_used: vec!["bash".into()],
                tokens_consumed: 100,
                duration_ms: 10,
                succeeded: true,
                completed_at: Utc::now(),
            })
        }
    }

    fn orch(output: &str) -> DagOrchestrator {
        let exec = Arc::new(MockExecutor {
            output: output.into(),
        });
        DagOrchestrator::new(exec, OrchestratorConfig::default())
    }

    #[tokio::test]
    async fn single_task_completes() {
        let o = orch("result");
        let tasks = vec![SubTask::new("t1", "do something")];
        let result = o.execute(tasks).await.unwrap();
        assert_eq!(result.tasks_completed, 1);
        assert_eq!(result.tasks_failed, 0);
    }

    #[tokio::test]
    async fn two_independent_tasks_parallel() {
        let o = orch("ok");
        let tasks = vec![SubTask::new("t1", "task 1"), SubTask::new("t2", "task 2")];
        let result = o.execute(tasks).await.unwrap();
        assert_eq!(result.tasks_completed, 2);
    }

    #[tokio::test]
    async fn sequential_chain_respects_deps() {
        let o = orch("output");
        let t1 = SubTask::new("t1", "first step");
        let t2 = SubTask::new("t2", "second step").with_deps(vec![t1.id]);
        let tasks = vec![t1, t2];
        let result = o.execute(tasks).await.unwrap();
        assert_eq!(result.tasks_completed, 2);
        assert_eq!(result.tasks_failed, 0);
    }

    #[test]
    fn cycle_detected() {
        let mut t1 = SubTask::new("t1", "task 1");
        let mut t2 = SubTask::new("t2", "task 2");
        t1.depends_on = vec![t2.id];
        t2.depends_on = vec![t1.id];
        let result = DagOrchestrator::validate_dag(&[t1, t2]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cycle"));
    }

    #[test]
    fn unknown_dep_rejected() {
        let unknown_id = Uuid::new_v4();
        let t1 = SubTask::new("t1", "task").with_deps(vec![unknown_id]);
        let result = DagOrchestrator::validate_dag(&[t1]);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn empty_dag_completes() {
        let o = orch("ok");
        let result = o.execute(vec![]).await.unwrap();
        assert_eq!(result.tasks_completed, 0);
        assert_eq!(result.tasks_failed, 0);
    }

    #[tokio::test]
    async fn token_budget_tracked() {
        let o = orch("ok");
        let tasks = vec![SubTask::new("t1", "task 1"), SubTask::new("t2", "task 2")];
        let result = o.execute(tasks).await.unwrap();
        // MockExecutor consumes 100 tokens per task
        assert_eq!(result.total_tokens, 200);
    }
}
