//! PLAN_STATE_DIAGNOSTICS — Phase K2
//!
//! Diagnostic instrumentation for plan lifecycle tracing.
//!
//! Provides:
//! - `PlanLifecycleLog`: structured log of plan state transitions
//! - `PlanInvariantChecker`: formal invariant enforcement at each transition
//! - `CriticRetryGuard`: ensures critic retry cannot reset step index without counter
//! - `BudgetInvariantChecker`: enforces max_rounds ≥ plan.total_steps + critic_retries
//!
//! Integration: wire into `agent/mod.rs` and `mod.rs` at plan creation / step transition
//! / critic retry / termination sites.

use std::collections::HashMap;
use std::time::{Duration, Instant};
use uuid::Uuid;

// ── Plan Step Status ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagStepStatus {
    Pending,
    InProgress,
    Completed,
    /// Step was orphaned (e.g., plan replaced by critic retry before step executed).
    Orphaned,
    Failed(String),
}

// ── Plan Lifecycle Event ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PlanLifecycleEvent {
    pub timestamp: Duration,
    pub plan_id: Uuid,
    pub event: PlanEvent,
}

#[derive(Debug, Clone)]
pub enum PlanEvent {
    PlanCreated {
        total_steps: usize,
        goal: String,
        max_rounds_at_creation: usize,
    },
    StepStarted {
        step_index: usize,
        description: String,
    },
    StepCompleted {
        step_index: usize,
        duration_ms: u64,
        delegated: bool,
    },
    StepOrphaned {
        step_index: usize,
        reason: String,
    },
    PlanAborted {
        completed_steps: usize,
        total_steps: usize,
        termination_cause: String,
    },
    PlanComplete {
        duration_ms: u64,
    },
    CriticRetry {
        retry_number: u32,
        critic_confidence: f32,
        reason: String,
        /// max_rounds available for this retry attempt.
        max_rounds_retry: usize,
    },
    FsmTransition {
        from_state: String,
        to_state: String,
        round: usize,
    },
    BudgetSnapshot {
        round: usize,
        rounds_remaining: usize,
        tokens_used: u64,
        tokens_remaining: u64,
    },
    CriticDecision {
        round: usize,
        achieved: bool,
        confidence: f32,
        reasoning_summary: String,
    },
}

// ── Plan Lifecycle Log ────────────────────────────────────────────────────────

/// Stateful recorder for a plan's full lifecycle.
///
/// One instance per plan_id. Multiple plans may be active if critic retry
/// creates a new plan — each gets its own log instance.
pub struct PlanLifecycleLog {
    pub plan_id: Uuid,
    pub goal: String,
    pub total_steps: usize,
    pub events: Vec<PlanLifecycleEvent>,
    pub step_statuses: HashMap<usize, DiagStepStatus>,
    start: Instant,
}

impl PlanLifecycleLog {
    pub fn new(plan_id: Uuid, goal: impl Into<String>, total_steps: usize, max_rounds: usize) -> Self {
        let mut log = Self {
            plan_id,
            goal: goal.into(),
            total_steps,
            events: Vec::new(),
            step_statuses: HashMap::new(),
            start: Instant::now(),
        };
        for i in 0..total_steps {
            log.step_statuses.insert(i, DiagStepStatus::Pending);
        }
        log.push(PlanEvent::PlanCreated {
            total_steps,
            goal: log.goal.clone(),
            max_rounds_at_creation: max_rounds,
        });
        log
    }

    pub fn step_started(&mut self, index: usize, description: impl Into<String>) {
        self.step_statuses.insert(index, DiagStepStatus::InProgress);
        self.push(PlanEvent::StepStarted { step_index: index, description: description.into() });
    }

    pub fn step_completed(&mut self, index: usize, duration_ms: u64, delegated: bool) {
        self.step_statuses.insert(index, DiagStepStatus::Completed);
        self.push(PlanEvent::StepCompleted { step_index: index, duration_ms, delegated });
    }

    pub fn step_orphaned(&mut self, index: usize, reason: impl Into<String>) {
        let reason = reason.into();
        self.step_statuses.insert(index, DiagStepStatus::Orphaned);
        self.push(PlanEvent::StepOrphaned { step_index: index, reason });
    }

    pub fn plan_aborted(&mut self, termination_cause: impl Into<String>) {
        let completed = self.step_statuses.values()
            .filter(|s| **s == DiagStepStatus::Completed)
            .count();
        // Mark all remaining Pending steps as Orphaned.
        let pending_indices: Vec<usize> = self.step_statuses.iter()
            .filter(|(_, s)| **s == DiagStepStatus::Pending)
            .map(|(i, _)| *i)
            .collect();
        for i in pending_indices {
            self.step_statuses.insert(i, DiagStepStatus::Orphaned);
        }
        self.push(PlanEvent::PlanAborted {
            completed_steps: completed,
            total_steps: self.total_steps,
            termination_cause: termination_cause.into(),
        });
    }

    pub fn fsm_transition(&mut self, from: &str, to: &str, round: usize) {
        self.push(PlanEvent::FsmTransition {
            from_state: from.to_string(),
            to_state: to.to_string(),
            round,
        });
    }

    pub fn budget_snapshot(&mut self, round: usize, rounds_remaining: usize,
                            tokens_used: u64, tokens_remaining: u64) {
        self.push(PlanEvent::BudgetSnapshot { round, rounds_remaining, tokens_used, tokens_remaining });
    }

    pub fn critic_decision(&mut self, round: usize, achieved: bool, confidence: f32, summary: &str) {
        self.push(PlanEvent::CriticDecision {
            round,
            achieved,
            confidence,
            reasoning_summary: summary.to_string(),
        });
    }

    /// Check PlanComplete invariant: all steps must be Completed.
    ///
    /// INVARIANT: PlanComplete → All steps.status == Completed
    pub fn check_plan_complete_invariant(&self) -> Result<(), String> {
        let non_completed: Vec<usize> = self.step_statuses.iter()
            .filter(|(_, s)| **s != DiagStepStatus::Completed)
            .map(|(i, _)| *i)
            .collect();
        if !non_completed.is_empty() {
            return Err(format!(
                "INVARIANT VIOLATION [PlanComplete]: plan_id={} declares complete but \
                 steps {:?} are not Completed (statuses: {:?})",
                self.plan_id,
                non_completed,
                non_completed.iter().map(|i| self.step_statuses.get(i)).collect::<Vec<_>>()
            ));
        }
        Ok(())
    }

    fn push(&mut self, event: PlanEvent) {
        self.events.push(PlanLifecycleEvent {
            timestamp: self.start.elapsed(),
            plan_id: self.plan_id,
            event,
        });
    }

    /// Render a compact trace for logging.
    pub fn trace(&self) -> String {
        let completed = self.step_statuses.values()
            .filter(|s| **s == DiagStepStatus::Completed)
            .count();
        format!(
            "plan_id={} goal=\"{}\" steps={}/{} events={}",
            self.plan_id, self.goal, completed, self.total_steps, self.events.len()
        )
    }
}

// ── CriticRetryGuard ─────────────────────────────────────────────────────────

/// Guards the critic retry loop to enforce:
///
/// INVARIANT: CriticRetry cannot reset step index without incrementing retry counter
/// INVARIANT: max_rounds must not terminate before plan steps evaluated
pub struct CriticRetryGuard {
    pub retry_count: u32,
    pub max_retries: u32,
    /// step index at the start of each retry (must be 0 since retry resets).
    pub step_index_at_retry: Vec<usize>,
}

impl CriticRetryGuard {
    pub fn new(max_retries: u32) -> Self {
        Self {
            retry_count: 0,
            max_retries,
            step_index_at_retry: Vec::new(),
        }
    }

    /// Called before starting a critic retry.
    ///
    /// Returns Err if the retry counter would overflow max_retries.
    pub fn begin_retry(&mut self, current_step_index: usize) -> Result<u32, String> {
        if self.retry_count >= self.max_retries {
            return Err(format!(
                "INVARIANT VIOLATION [CriticRetry]: retry_count={} >= max_retries={}: \
                 cannot retry further",
                self.retry_count, self.max_retries
            ));
        }
        self.retry_count += 1;
        self.step_index_at_retry.push(current_step_index);
        tracing::info!(
            retry = self.retry_count,
            step_index_before_retry = current_step_index,
            "CriticRetryGuard: retry {} of {} (step reset: {} → 0)",
            self.retry_count, self.max_retries, current_step_index
        );
        Ok(self.retry_count)
    }

    pub fn can_retry(&self) -> bool {
        self.retry_count < self.max_retries
    }
}

// ── BudgetInvariantChecker ───────────────────────────────────────────────────

/// Enforces:
///
/// INVARIANT: max_rounds ≥ plan.total_steps + critic_retries
pub struct BudgetInvariantChecker;

impl BudgetInvariantChecker {
    /// Check at plan creation time that max_rounds is sufficient.
    ///
    /// Returns Ok(()) if invariant holds, Err(corrected_max_rounds) if it is violated.
    /// The caller should use corrected_max_rounds instead.
    pub fn check_max_rounds_invariant(
        max_rounds: usize,
        plan_total_steps: usize,
        max_critic_retries: u32,
    ) -> Result<(), usize> {
        let required = plan_total_steps + max_critic_retries as usize + 1; // +1 for synthesis round
        if max_rounds < required {
            tracing::warn!(
                max_rounds,
                required,
                plan_total_steps,
                max_critic_retries,
                "INVARIANT VIOLATION [BudgetInvariant]: max_rounds({}) < \
                 plan.total_steps({}) + critic_retries({}) + 1 = {}",
                max_rounds, plan_total_steps, max_critic_retries, required
            );
            Err(required)
        } else {
            Ok(())
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_complete_invariant_passes_when_all_completed() {
        let id = Uuid::new_v4();
        let mut log = PlanLifecycleLog::new(id, "goal", 2, 4);
        log.step_started(0, "step 0");
        log.step_completed(0, 100, false);
        log.step_started(1, "step 1");
        log.step_completed(1, 200, false);
        assert!(log.check_plan_complete_invariant().is_ok());
    }

    #[test]
    fn plan_complete_invariant_fails_when_step_pending() {
        let id = Uuid::new_v4();
        let mut log = PlanLifecycleLog::new(id, "goal", 2, 4);
        log.step_started(0, "step 0");
        log.step_completed(0, 100, false);
        // Step 1 never executed → still Pending
        let result = log.check_plan_complete_invariant();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("INVARIANT VIOLATION [PlanComplete]"));
    }

    #[test]
    fn plan_aborted_marks_pending_as_orphaned() {
        let id = Uuid::new_v4();
        let mut log = PlanLifecycleLog::new(id, "goal", 3, 4);
        log.step_started(0, "step 0");
        log.step_completed(0, 100, false);
        // Steps 1 and 2 are Pending — simulate abort
        log.plan_aborted("max_rounds_reached");
        assert_eq!(log.step_statuses[&1], DiagStepStatus::Orphaned);
        assert_eq!(log.step_statuses[&2], DiagStepStatus::Orphaned);
        assert_eq!(log.step_statuses[&0], DiagStepStatus::Completed);
    }

    #[test]
    fn critic_retry_guard_increments_counter() {
        let mut guard = CriticRetryGuard::new(2);
        assert!(guard.begin_retry(1).is_ok());
        assert_eq!(guard.retry_count, 1);
        assert!(guard.begin_retry(0).is_ok());
        assert_eq!(guard.retry_count, 2);
        // Third retry exceeds max
        assert!(guard.begin_retry(0).is_err());
    }

    #[test]
    fn critic_retry_cannot_retry_beyond_max() {
        let mut guard = CriticRetryGuard::new(1);
        assert!(guard.begin_retry(0).is_ok());
        assert!(!guard.can_retry());
        let err = guard.begin_retry(0).unwrap_err();
        assert!(err.contains("INVARIANT VIOLATION [CriticRetry]"));
    }

    #[test]
    fn budget_invariant_passes_when_rounds_sufficient() {
        // 2-step plan, 1 critic retry, 1 synthesis round → need 4
        let result = BudgetInvariantChecker::check_max_rounds_invariant(4, 2, 1);
        assert!(result.is_ok());
    }

    #[test]
    fn budget_invariant_fails_when_rounds_too_low() {
        // 2-step plan, 1 critic retry → need 4, given 2
        let result = BudgetInvariantChecker::check_max_rounds_invariant(2, 2, 1);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), 4);
    }

    #[test]
    fn budget_invariant_fails_for_analiza_implementacion() {
        // Reproduces the observed failure:
        // max_rounds=2 (Conversational), plan_steps=2, max_retries=1
        let result = BudgetInvariantChecker::check_max_rounds_invariant(2, 2, 1);
        assert!(result.is_err(), "max_rounds=2 must fail for 2-step plan with 1 retry");
        let corrected = result.unwrap_err();
        assert_eq!(corrected, 4, "corrected max_rounds must be at least 4");
    }

    #[test]
    fn lifecycle_log_trace_shows_correct_counts() {
        let id = Uuid::new_v4();
        let mut log = PlanLifecycleLog::new(id, "analyze impl", 2, 2);
        log.step_started(0, "read files");
        log.step_completed(0, 8800, true);
        let trace = log.trace();
        assert!(trace.contains("steps=1/2"));
    }
}
