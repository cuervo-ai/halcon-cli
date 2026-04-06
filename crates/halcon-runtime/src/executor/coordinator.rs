//! Execution Coordinator — production-grade DAG execution with pause/resume/step,
//! retry policies, speculative branches, and budget-aware scheduling.
//!
//! Wraps `MutableDag` with execution control and emits structured events.
//! This is the primary interface for the control plane to manage execution.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::{watch, Notify, RwLock};
use uuid::Uuid;

use super::mutable_dag::{DagSnapshot, MutationAuthor, MutableDag, NodeStatus};
use super::{AgentSelector, TaskNode};
use crate::error::{Result, RuntimeError};
use halcon_storage::{EventCategory, EventStore};

/// Execution mode controlling how the coordinator advances.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    /// Run all ready nodes continuously until completion.
    Continuous,
    /// Execute one wave of ready nodes, then pause.
    StepWave,
    /// Execute exactly one node, then pause.
    StepNode,
    /// Execution is paused.
    Paused,
}

/// Reason why execution was paused.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    UserRequested,
    StepCompleted,
    BudgetWarning { utilization_pct: f32 },
    CenzontleHold { reason: String },
    PermissionEscalation { request_id: Uuid },
    NodeFailed { node_id: Uuid, error: String },
}

/// Retry policy for a node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub backoff_base_ms: u64,
    pub backoff_max_ms: u64,
    pub retry_on_timeout: bool,
    pub retry_on_error: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 2,
            backoff_base_ms: 500,
            backoff_max_ms: 10_000,
            retry_on_timeout: true,
            retry_on_error: true,
        }
    }
}

impl RetryPolicy {
    /// Compute backoff duration for a given attempt (exponential with jitter).
    pub fn backoff_for(&self, attempt: u32) -> Duration {
        let base = self.backoff_base_ms * 2u64.saturating_pow(attempt);
        let capped = base.min(self.backoff_max_ms);
        // Add ~25% jitter
        let jitter = capped / 4;
        let actual = capped + (rand_u64() % (jitter.max(1)));
        Duration::from_millis(actual)
    }
}

/// Event emitted by the coordinator during execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "event")]
pub enum CoordinatorEvent {
    /// Execution started or resumed.
    ExecutionStarted {
        session_id: Uuid,
        dag_version: u64,
        node_count: usize,
    },
    /// Execution paused.
    ExecutionPaused {
        session_id: Uuid,
        reason: PauseReason,
    },
    /// Execution resumed.
    ExecutionResumed {
        session_id: Uuid,
        mode: ExecutionMode,
    },
    /// A node started executing.
    NodeStarted {
        session_id: Uuid,
        node_id: Uuid,
        instruction: String,
    },
    /// A node completed.
    NodeCompleted {
        session_id: Uuid,
        node_id: Uuid,
        duration_ms: u64,
        retry_count: u32,
    },
    /// A node failed.
    NodeFailed {
        session_id: Uuid,
        node_id: Uuid,
        error: String,
        retryable: bool,
        retry_count: u32,
    },
    /// A node is being retried.
    NodeRetrying {
        session_id: Uuid,
        node_id: Uuid,
        attempt: u32,
        backoff_ms: u64,
    },
    /// DAG was mutated during execution.
    DagMutated {
        session_id: Uuid,
        dag_version: u64,
        mutation_kind: String,
    },
    /// Execution completed (all nodes terminal).
    ExecutionCompleted {
        session_id: Uuid,
        total_duration_ms: u64,
        nodes_completed: usize,
        nodes_failed: usize,
        nodes_skipped: usize,
    },
    /// Budget warning threshold reached.
    BudgetWarning {
        session_id: Uuid,
        tokens_used: u64,
        tokens_limit: u64,
        utilization_pct: f32,
    },
}

impl CoordinatorEvent {
    /// Get the event type as a string for categorization.
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::ExecutionStarted { .. } => "execution_started",
            Self::ExecutionPaused { .. } => "execution_paused",
            Self::ExecutionResumed { .. } => "execution_resumed",
            Self::NodeStarted { .. } => "node_started",
            Self::NodeCompleted { .. } => "node_completed",
            Self::NodeFailed { .. } => "node_failed",
            Self::NodeRetrying { .. } => "node_retrying",
            Self::DagMutated { .. } => "dag_mutated",
            Self::ExecutionCompleted { .. } => "execution_completed",
            Self::BudgetWarning { .. } => "budget_warning",
        }
    }
}

/// The coordinator's observable state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinatorState {
    pub session_id: Uuid,
    pub mode: ExecutionMode,
    pub dag_version: u64,
    pub nodes_total: usize,
    pub nodes_completed: usize,
    pub nodes_failed: usize,
    pub nodes_running: usize,
    pub nodes_pending: usize,
    pub nodes_skipped: usize,
    pub paused: bool,
    pub pause_reason: Option<PauseReason>,
    pub elapsed_ms: u64,
    pub tokens_used: u64,
}

/// Execution coordinator — manages a MutableDag's lifecycle with full control.
pub struct ExecutionCoordinator {
    session_id: Uuid,
    dag: Arc<MutableDag>,
    mode: Arc<RwLock<ExecutionMode>>,
    paused: Arc<AtomicBool>,
    pause_reason: Arc<RwLock<Option<PauseReason>>>,
    resume_notify: Arc<Notify>,
    cancelled: Arc<AtomicBool>,
    default_retry: RetryPolicy,
    node_retries: Arc<RwLock<HashMap<Uuid, RetryPolicy>>>,
    tokens_used: Arc<AtomicU64>,
    tokens_limit: u64,
    budget_warn_pct: f32,
    started_at: Instant,
    event_tx: tokio::sync::mpsc::UnboundedSender<CoordinatorEvent>,
    /// Optional event store for time-travel debugging and replay.
    event_store: Option<Arc<EventStore>>,
}

impl ExecutionCoordinator {
    /// Create a new coordinator for a session.
    pub fn new(
        session_id: Uuid,
        dag: Arc<MutableDag>,
        tokens_limit: u64,
        event_tx: tokio::sync::mpsc::UnboundedSender<CoordinatorEvent>,
    ) -> Self {
        Self {
            session_id,
            dag,
            mode: Arc::new(RwLock::new(ExecutionMode::Continuous)),
            paused: Arc::new(AtomicBool::new(false)),
            pause_reason: Arc::new(RwLock::new(None)),
            resume_notify: Arc::new(Notify::new()),
            cancelled: Arc::new(AtomicBool::new(false)),
            default_retry: RetryPolicy::default(),
            node_retries: Arc::new(RwLock::new(HashMap::new())),
            tokens_used: Arc::new(AtomicU64::new(0)),
            tokens_limit,
            budget_warn_pct: 0.8,
            started_at: Instant::now(),
            event_tx,
            event_store: None,
        }
    }

    /// Attach an event store for time-travel debugging and replay.
    pub fn with_event_store(mut self, store: Arc<EventStore>) -> Self {
        self.event_store = Some(store);
        self
    }

    /// Get current coordinator state.
    pub async fn state(&self) -> CoordinatorState {
        let snap = self.dag.snapshot();
        let mode = *self.mode.read().await;
        let pause_reason = self.pause_reason.read().await.clone();

        let mut completed = 0usize;
        let mut failed = 0usize;
        let mut running = 0usize;
        let mut pending = 0usize;
        let mut skipped = 0usize;

        for node in &snap.nodes {
            match &node.status {
                NodeStatus::Completed => completed += 1,
                NodeStatus::Failed { .. } => failed += 1,
                NodeStatus::Running => running += 1,
                NodeStatus::Skipped => skipped += 1,
                NodeStatus::Pending | NodeStatus::Ready => pending += 1,
                NodeStatus::Speculative => pending += 1,
            }
        }

        CoordinatorState {
            session_id: self.session_id,
            mode,
            dag_version: snap.version,
            nodes_total: snap.nodes.len(),
            nodes_completed: completed,
            nodes_failed: failed,
            nodes_running: running,
            nodes_pending: pending,
            nodes_skipped: skipped,
            paused: self.paused.load(Ordering::Relaxed),
            pause_reason,
            elapsed_ms: self.started_at.elapsed().as_millis() as u64,
            tokens_used: self.tokens_used.load(Ordering::Relaxed),
        }
    }

    /// Pause execution after current nodes complete.
    pub async fn pause(&self, reason: PauseReason) {
        self.paused.store(true, Ordering::Release);
        *self.pause_reason.write().await = Some(reason.clone());
        *self.mode.write().await = ExecutionMode::Paused;
        self.emit(CoordinatorEvent::ExecutionPaused {
            session_id: self.session_id,
            reason,
        });
    }

    /// Resume execution.
    pub async fn resume(&self, mode: ExecutionMode) {
        self.paused.store(false, Ordering::Release);
        *self.pause_reason.write().await = None;
        *self.mode.write().await = mode;
        self.resume_notify.notify_one();
        self.emit(CoordinatorEvent::ExecutionResumed {
            session_id: self.session_id,
            mode,
        });
    }

    /// Switch to step-node mode (execute one node then pause).
    pub async fn step_node(&self) {
        *self.mode.write().await = ExecutionMode::StepNode;
        self.paused.store(false, Ordering::Release);
        self.resume_notify.notify_one();
    }

    /// Switch to step-wave mode (execute one wave then pause).
    pub async fn step_wave(&self) {
        *self.mode.write().await = ExecutionMode::StepWave;
        self.paused.store(false, Ordering::Release);
        self.resume_notify.notify_one();
    }

    /// Cancel execution.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.resume_notify.notify_one();
    }

    /// Check if cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// Set retry policy for a specific node.
    pub async fn set_node_retry(&self, node_id: Uuid, policy: RetryPolicy) {
        self.node_retries.write().await.insert(node_id, policy);
    }

    /// Get the DAG snapshot.
    pub fn dag_snapshot(&self) -> DagSnapshot {
        self.dag.snapshot()
    }

    /// Access the mutable DAG (for mutations).
    pub fn dag(&self) -> &Arc<MutableDag> {
        &self.dag
    }

    /// Record token usage and check budget.
    pub fn record_tokens(&self, tokens: u64) {
        let used = self.tokens_used.fetch_add(tokens, Ordering::Relaxed) + tokens;
        if self.tokens_limit > 0 {
            let pct = used as f32 / self.tokens_limit as f32;
            if pct >= self.budget_warn_pct {
                self.emit(CoordinatorEvent::BudgetWarning {
                    session_id: self.session_id,
                    tokens_used: used,
                    tokens_limit: self.tokens_limit,
                    utilization_pct: pct,
                });
            }
        }
    }

    /// Wait for resume if paused.
    pub async fn wait_if_paused(&self) {
        while self.paused.load(Ordering::Acquire) && !self.cancelled.load(Ordering::Relaxed) {
            self.resume_notify.notified().await;
        }
    }

    /// Check if we should pause after completing a unit of work.
    pub async fn should_pause_after_unit(&self) -> bool {
        let mode = *self.mode.read().await;
        matches!(mode, ExecutionMode::StepNode | ExecutionMode::StepWave)
    }

    /// Execute the DAG to completion (or until paused/cancelled).
    ///
    /// `executor_fn` is called for each node and must return (output, tokens_used).
    /// This decouples the coordinator from the actual agent invocation.
    pub async fn run<F, Fut>(
        &self,
        executor_fn: F,
    ) -> Result<CoordinatorState>
    where
        F: Fn(TaskNode) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = std::result::Result<(String, u64), String>> + Send,
    {
        let snap = self.dag.snapshot();
        self.emit(CoordinatorEvent::ExecutionStarted {
            session_id: self.session_id,
            dag_version: snap.version,
            node_count: snap.nodes.len(),
        });

        loop {
            // Check termination conditions.
            if self.is_cancelled() {
                break;
            }

            self.wait_if_paused().await;
            if self.is_cancelled() {
                break;
            }

            // Get ready nodes.
            let ready = self.dag.ready_nodes();
            if ready.is_empty() {
                if self.dag.has_running() {
                    // Wait for running nodes to complete.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
                // No ready, no running → done.
                break;
            }

            let mode = *self.mode.read().await;

            // In StepNode mode, only take one node.
            let batch: Vec<TaskNode> = match mode {
                ExecutionMode::StepNode => vec![ready.into_iter().next().unwrap()],
                _ => ready,
            };

            // Execute the batch (sequential within wave for correctness).
            for node in batch {
                let node_id = node.task_id;
                self.dag.mark_running(node_id);

                self.emit(CoordinatorEvent::NodeStarted {
                    session_id: self.session_id,
                    node_id,
                    instruction: node.instruction.clone(),
                });

                let retries = self.node_retries.read().await;
                let policy = retries
                    .get(&node_id)
                    .cloned()
                    .unwrap_or_else(|| self.default_retry.clone());
                drop(retries);

                let dag = self.dag.clone();
                let sid = self.session_id;
                let event_tx = self.event_tx.clone();
                let executor = &executor_fn;

                // We can't move executor_fn into the spawn, so we use a channel pattern.
                // For now, execute sequentially within the wave (safe for correctness).
                let start = Instant::now();
                let mut attempt = 0u32;
                let mut last_error = String::new();

                loop {
                    match executor(node.clone()).await {
                        Ok((output, tokens)) => {
                            let dur = start.elapsed().as_millis() as u64;
                            self.record_tokens(tokens);
                            dag.mark_completed(node_id, output);
                            let _ = event_tx.send(CoordinatorEvent::NodeCompleted {
                                session_id: sid,
                                node_id,
                                duration_ms: dur,
                                retry_count: attempt,
                            });
                            break;
                        }
                        Err(err) => {
                            last_error = err.clone();
                            if attempt < policy.max_retries && policy.retry_on_error {
                                let backoff = policy.backoff_for(attempt);
                                let _ = event_tx.send(CoordinatorEvent::NodeRetrying {
                                    session_id: sid,
                                    node_id,
                                    attempt: attempt + 1,
                                    backoff_ms: backoff.as_millis() as u64,
                                });
                                tokio::time::sleep(backoff).await;
                                attempt += 1;
                            } else {
                                let dur = start.elapsed().as_millis() as u64;
                                dag.mark_failed(node_id, err.clone(), attempt < policy.max_retries);
                                let _ = event_tx.send(CoordinatorEvent::NodeFailed {
                                    session_id: sid,
                                    node_id,
                                    error: err,
                                    retryable: false,
                                    retry_count: attempt,
                                });
                                break;
                            }
                        }
                    }
                }
            }

            // After wave/node, check if we should auto-pause.
            let mode = *self.mode.read().await;
            match mode {
                ExecutionMode::StepNode | ExecutionMode::StepWave => {
                    self.pause(PauseReason::StepCompleted).await;
                }
                _ => {}
            }

            // Check budget.
            if self.tokens_limit > 0 {
                let used = self.tokens_used.load(Ordering::Relaxed);
                if used >= self.tokens_limit {
                    break;
                }
            }
        }

        let final_state = self.state().await;

        self.emit(CoordinatorEvent::ExecutionCompleted {
            session_id: self.session_id,
            total_duration_ms: self.started_at.elapsed().as_millis() as u64,
            nodes_completed: final_state.nodes_completed,
            nodes_failed: final_state.nodes_failed,
            nodes_skipped: final_state.nodes_skipped,
        });

        Ok(final_state)
    }

    fn emit(&self, event: CoordinatorEvent) {
        // Send to channel for real-time consumers.
        let _ = self.event_tx.send(event.clone());

        // Persist to event store if configured (for time-travel debugging).
        if let Some(store) = &self.event_store {
            let category = match &event {
                CoordinatorEvent::ExecutionStarted { .. }
                | CoordinatorEvent::ExecutionPaused { .. }
                | CoordinatorEvent::ExecutionResumed { .. }
                | CoordinatorEvent::ExecutionCompleted { .. }
                | CoordinatorEvent::NodeStarted { .. }
                | CoordinatorEvent::NodeCompleted { .. }
                | CoordinatorEvent::NodeFailed { .. }
                | CoordinatorEvent::NodeRetrying { .. }
                | CoordinatorEvent::BudgetWarning { .. } => EventCategory::Execution,
                CoordinatorEvent::DagMutated { .. } => EventCategory::DagMutation,
            };

            if let Ok(payload) = serde_json::to_string(&event) {
                let _ = store.append(
                    Uuid::new_v4(),
                    Some(self.session_id),
                    category,
                    &format!("coordinator.{}", event.event_type()),
                    &payload,
                    None,
                    None,
                );
            }
        }
    }
}

/// Simple pseudo-random u64 for jitter (avoids pulling in rand crate for one use).
fn rand_u64() -> u64 {
    use std::time::SystemTime;
    let t = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    t.as_nanos() as u64 ^ (t.subsec_nanos() as u64).wrapping_mul(6364136223846793005)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn make_dag_with_nodes(count: usize) -> Arc<MutableDag> {
        let dag = MutableDag::new();
        for i in 0..count {
            dag.insert_node(
                format!("task_{i}"),
                AgentSelector::ByName("test".to_string()),
                None,
                MutationAuthor::System,
            )
            .unwrap();
        }
        Arc::new(dag)
    }

    fn make_chain_dag() -> Arc<MutableDag> {
        let dag = MutableDag::new();
        let a = dag
            .insert_node(
                "A".to_string(),
                AgentSelector::ByName("test".to_string()),
                None,
                MutationAuthor::System,
            )
            .unwrap();
        dag.insert_node(
            "B".to_string(),
            AgentSelector::ByName("test".to_string()),
            Some(a),
            MutationAuthor::System,
        )
        .unwrap();
        Arc::new(dag)
    }

    #[tokio::test]
    async fn run_continuous_all_succeed() {
        let dag = make_dag_with_nodes(3);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let coord = ExecutionCoordinator::new(Uuid::new_v4(), dag, 0, tx);

        let state = coord
            .run(|node| async move { Ok((format!("done: {}", node.instruction), 10)) })
            .await
            .unwrap();

        assert_eq!(state.nodes_completed, 3);
        assert_eq!(state.nodes_failed, 0);
        assert!(!state.paused);

        // Verify events were emitted.
        let mut events = Vec::new();
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
        assert!(events.iter().any(|e| matches!(e, CoordinatorEvent::ExecutionStarted { .. })));
        assert!(events.iter().any(|e| matches!(e, CoordinatorEvent::ExecutionCompleted { .. })));
        assert_eq!(
            events.iter().filter(|e| matches!(e, CoordinatorEvent::NodeCompleted { .. })).count(),
            3
        );
    }

    #[tokio::test]
    async fn run_with_failure_and_retry() {
        let dag = make_dag_with_nodes(1);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let coord = ExecutionCoordinator::new(Uuid::new_v4(), dag, 0, tx);

        let attempt_counter = Arc::new(AtomicU64::new(0));
        let counter = attempt_counter.clone();

        let state = coord
            .run(move |_node| {
                let counter = counter.clone();
                async move {
                    let attempt = counter.fetch_add(1, Ordering::Relaxed);
                    if attempt < 2 {
                        Err("transient error".to_string())
                    } else {
                        Ok(("succeeded on retry".to_string(), 5))
                    }
                }
            })
            .await
            .unwrap();

        assert_eq!(state.nodes_completed, 1);
        assert_eq!(state.nodes_failed, 0);
        assert_eq!(attempt_counter.load(Ordering::Relaxed), 3); // 0, 1 fail; 2 succeeds

        let mut events = Vec::new();
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
        assert_eq!(
            events.iter().filter(|e| matches!(e, CoordinatorEvent::NodeRetrying { .. })).count(),
            2
        );
    }

    #[tokio::test]
    async fn run_chain_dag_sequential() {
        let dag = make_chain_dag();
        let (tx, _rx) = mpsc::unbounded_channel();
        let coord = ExecutionCoordinator::new(Uuid::new_v4(), dag, 0, tx);

        let state = coord
            .run(|node| async move { Ok((node.instruction.clone(), 1)) })
            .await
            .unwrap();

        assert_eq!(state.nodes_completed, 2);
    }

    #[tokio::test]
    async fn pause_and_resume() {
        let dag = make_dag_with_nodes(2);
        let (tx, _rx) = mpsc::unbounded_channel();
        let coord = Arc::new(ExecutionCoordinator::new(Uuid::new_v4(), dag, 0, tx));

        // Start paused.
        coord.pause(PauseReason::UserRequested).await;

        let coord_clone = coord.clone();
        let handle = tokio::spawn(async move {
            coord_clone
                .run(|node| async move { Ok((node.instruction.clone(), 1)) })
                .await
        });

        // Let it try to run (it should be blocked on pause).
        tokio::time::sleep(Duration::from_millis(50)).await;
        let state = coord.state().await;
        assert!(state.paused);
        assert_eq!(state.nodes_completed, 0);

        // Resume.
        coord.resume(ExecutionMode::Continuous).await;
        let final_state = handle.await.unwrap().unwrap();
        assert_eq!(final_state.nodes_completed, 2);
    }

    #[tokio::test]
    async fn cancel_stops_execution() {
        let dag = make_dag_with_nodes(5);
        let (tx, _rx) = mpsc::unbounded_channel();
        let coord = Arc::new(ExecutionCoordinator::new(Uuid::new_v4(), dag, 0, tx));

        coord.pause(PauseReason::UserRequested).await;

        let coord_clone = coord.clone();
        let handle = tokio::spawn(async move {
            coord_clone
                .run(|node| async move { Ok((node.instruction.clone(), 1)) })
                .await
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        coord.cancel();

        let state = handle.await.unwrap().unwrap();
        // Some nodes were never started.
        assert!(state.nodes_completed < 5);
    }

    #[tokio::test]
    async fn step_node_mode() {
        let dag = make_dag_with_nodes(3);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let coord = Arc::new(ExecutionCoordinator::new(Uuid::new_v4(), dag, 0, tx));

        // Set step-node mode.
        *coord.mode.write().await = ExecutionMode::StepNode;

        let coord_clone = coord.clone();
        let handle = tokio::spawn(async move {
            coord_clone
                .run(|node| async move { Ok((node.instruction.clone(), 1)) })
                .await
        });

        // Let one node execute.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let state = coord.state().await;
        assert_eq!(state.nodes_completed, 1);
        assert!(state.paused);

        // Step again.
        coord.step_node().await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let state = coord.state().await;
        assert_eq!(state.nodes_completed, 2);

        // Switch to continuous to finish.
        coord.resume(ExecutionMode::Continuous).await;
        let final_state = handle.await.unwrap().unwrap();
        assert_eq!(final_state.nodes_completed, 3);
    }

    #[tokio::test]
    async fn budget_warning_emitted() {
        let dag = make_dag_with_nodes(1);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let coord = ExecutionCoordinator::new(Uuid::new_v4(), dag, 100, tx);

        let state = coord
            .run(|_node| async move { Ok(("done".to_string(), 90)) })
            .await
            .unwrap();

        let mut events = Vec::new();
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
        assert!(events.iter().any(|e| matches!(e, CoordinatorEvent::BudgetWarning { .. })));
    }

    #[test]
    fn retry_policy_backoff() {
        let policy = RetryPolicy {
            max_retries: 3,
            backoff_base_ms: 100,
            backoff_max_ms: 5000,
            retry_on_timeout: true,
            retry_on_error: true,
        };

        let b0 = policy.backoff_for(0);
        let b1 = policy.backoff_for(1);
        let b2 = policy.backoff_for(2);

        // Exponential: 100, 200, 400 (with jitter)
        assert!(b0.as_millis() >= 100 && b0.as_millis() <= 150);
        assert!(b1.as_millis() >= 200 && b1.as_millis() <= 300);
        assert!(b2.as_millis() >= 400 && b2.as_millis() <= 600);
    }

    #[test]
    fn retry_policy_capped() {
        let policy = RetryPolicy {
            max_retries: 10,
            backoff_base_ms: 1000,
            backoff_max_ms: 5000,
            retry_on_timeout: true,
            retry_on_error: true,
        };

        let b8 = policy.backoff_for(8);
        // 1000 * 2^8 = 256000, capped at 5000 + jitter
        assert!(b8.as_millis() <= 7000);
    }
}
