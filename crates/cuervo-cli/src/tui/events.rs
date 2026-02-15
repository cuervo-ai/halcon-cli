//! UI event protocol for TUI rendering.
//!
//! These events flow from the agent loop (via `TuiSink`) to the TUI render
//! loop over an mpsc channel, decoupling business logic from display.

use serde_json::Value;

/// Events sent from the agent loop to the TUI render loop.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum UiEvent {
    /// Incremental text from the streaming model response.
    StreamChunk(String),
    /// A fenced code block completed (language, full code).
    StreamCodeBlock { lang: String, code: String },
    /// Model indicated a tool call is coming (marker in stream).
    StreamToolMarker(String),
    /// Streaming response complete for this round.
    StreamDone,
    /// Stream-level error from provider.
    StreamError(String),
    /// A tool execution is starting.
    ToolStart { name: String, input: Value },
    /// A tool execution completed.
    ToolOutput {
        name: String,
        content: String,
        is_error: bool,
        duration_ms: u64,
    },
    /// A tool was denied by the user or permission system.
    ToolDenied(String),
    /// Spinner should start (inference waiting).
    SpinnerStart(String),
    /// Spinner should stop.
    SpinnerStop,
    /// A warning message for display.
    Warning {
        message: String,
        hint: Option<String>,
    },
    /// An error message for display.
    Error {
        message: String,
        hint: Option<String>,
    },
    /// An informational status line (round separators, compaction notices, etc.).
    Info(String),
    /// Status bar update (provider, model, tokens, cost, etc.).
    StatusUpdate {
        provider: Option<String>,
        model: Option<String>,
        round: Option<usize>,
        tokens: Option<u64>,
        cost: Option<f64>,
        session_id: Option<String>,
        elapsed_ms: Option<u64>,
        tool_count: Option<u32>,
        input_tokens: Option<u32>,
        output_tokens: Option<u32>,
    },
    /// A new agent round is starting.
    RoundStart(usize),
    /// An agent round has completed.
    RoundEnd(usize),
    /// Force a redraw of the TUI.
    Redraw,
    /// The agent loop has finished — TUI should show prompt again.
    AgentDone,
    /// Request to quit the TUI application.
    Quit,
    /// Plan progress update — shows/updates the plan overview in the activity zone.
    PlanProgress {
        goal: String,
        steps: Vec<PlanStepStatus>,
        current_step: usize,
        elapsed_ms: u64,
    },

    // --- Phase 42B: Cockpit feedback events (9 new) ---

    /// An agent round is starting with provider/model info.
    RoundStarted { round: usize, provider: String, model: String },
    /// An agent round ended with metrics.
    RoundEnded { round: usize, input_tokens: u32, output_tokens: u32, cost: f64, duration_ms: u64 },
    /// Model was selected/changed.
    ModelSelected { model: String, provider: String, reason: String },
    /// Provider fallback was triggered.
    ProviderFallback { from: String, to: String, reason: String },
    /// Tool loop guard took an escalation action.
    LoopGuardAction { action: String, reason: String },
    /// Context compaction completed.
    CompactionComplete { old_msgs: usize, new_msgs: usize, tokens_saved: u64 },
    /// Cache hit or miss.
    CacheStatus { hit: bool, source: String },
    /// Speculative tool execution result.
    SpeculativeResult { tool: String, hit: bool },
    /// Awaiting permission for tool execution (Phase I-6C: extended with args + risk).
    PermissionAwaiting {
        tool: String,
        args: serde_json::Value,
        risk_level: String,
    },

    // --- Phase 43C: Feedback completeness events (4 new) ---

    /// Reflection analysis started.
    ReflectionStarted,
    /// Reflection complete with analysis and score.
    ReflectionComplete { analysis: String, score: f64 },
    /// Memory consolidation operation in progress.
    ConsolidationStatus { action: String },
    /// Memory consolidation operation completed.
    ConsolidationComplete { merged: usize, pruned: usize, duration_ms: u64 },
    /// Tool retrying after failure.
    ToolRetrying { tool: String, attempt: usize, max_attempts: usize, delay_ms: u64 },

    // --- Phase 43D: Live panel data events ---

    /// Context tier usage update from pipeline.
    ContextTierUpdate {
        l0_tokens: u32,
        l0_capacity: u32,
        l1_tokens: u32,
        l1_entries: usize,
        l2_entries: usize,
        l3_entries: usize,
        l4_entries: usize,
        total_tokens: u32,
    },
    /// Reasoning engine status update.
    ReasoningUpdate {
        strategy: String,
        task_type: String,
        complexity: String,
    },

    // --- Phase 44B: Continuous interaction events (Phase 2) ---

    /// Agent started processing a prompt (dequeue from channel).
    AgentStartedPrompt,
    /// Agent finished processing a prompt (ready for next).
    AgentFinishedPrompt,
    /// Current prompt queue status (how many waiting).
    PromptQueueStatus(usize),

    // --- Phase 44A: Observability events ---

    /// Dry-run mode active indicator. Persistent banner when true.
    DryRunActive(bool),

    /// Token budget update: current usage vs limit.
    TokenBudgetUpdate {
        used: u64,
        limit: u64,
        rate_per_minute: f64,
    },

    /// Provider health status change.
    ProviderHealthUpdate {
        provider: String,
        status: ProviderHealthStatus,
    },

    // --- Phase B4: Circuit breaker events ---

    /// Circuit breaker state change for a provider.
    CircuitBreakerUpdate {
        provider: String,
        state: CircuitBreakerState,
        failure_count: u32,
    },

    // --- Phase B5: Agent state transition events ---

    /// Agent state transition (FSM change).
    AgentStateTransition {
        from: AgentState,
        to: AgentState,
        reason: String,
    },

    // --- Sprint 1 B2+B3: Data parity events ---

    /// Task status update (parity with ClassicSink).
    TaskStatus {
        title: String,
        status: String,
        duration_ms: Option<u64>,
        artifact_count: usize,
    },

    /// Reasoning engine status (parity with ClassicSink).
    ReasoningStatus {
        task_type: String,
        complexity: String,
        strategy: String,
        score: f64,
        success: bool,
    },
}

/// Circuit breaker states for provider resilience.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CircuitBreakerState {
    Closed,
    Open,
    HalfOpen,
}

/// Agent execution state for state-transition tracking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentState {
    Idle,
    Planning,
    Executing,
    ToolWait,
    Reflecting,
    Paused,
    Complete,
    Failed,
}

impl AgentState {
    /// Returns the set of valid successor states for FSM transition validation.
    /// Invalid transitions are logged as warnings but not blocked (observability).
    pub fn valid_successors(&self) -> &'static [AgentState] {
        match self {
            AgentState::Idle => &[AgentState::Planning, AgentState::Executing],
            AgentState::Planning => &[AgentState::Executing, AgentState::Failed, AgentState::Paused],
            AgentState::Executing => &[
                AgentState::ToolWait,
                AgentState::Reflecting,
                AgentState::Complete,
                AgentState::Failed,
                AgentState::Paused,
            ],
            AgentState::ToolWait => &[AgentState::Executing, AgentState::Failed, AgentState::Paused],
            AgentState::Reflecting => &[
                AgentState::Planning,
                AgentState::Executing,
                AgentState::Complete,
                AgentState::Failed,
                AgentState::Paused,
            ],
            AgentState::Paused => &[
                AgentState::Planning,
                AgentState::Executing,
                AgentState::ToolWait,
                AgentState::Reflecting,
                AgentState::Idle,
            ],
            AgentState::Complete => &[AgentState::Idle],
            AgentState::Failed => &[AgentState::Idle],
        }
    }

    /// Check whether transitioning from `self` to `to` is valid.
    pub fn can_transition_to(&self, to: &AgentState) -> bool {
        self.valid_successors().contains(to)
    }
}

/// Provider health status for UI display.
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderHealthStatus {
    Healthy,
    Degraded { failure_rate: f64, latency_p95_ms: u64 },
    Unhealthy { reason: String },
}

/// Control events sent from the TUI to the agent loop (reverse direction).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlEvent {
    /// Pause the agent loop.
    Pause,
    /// Resume the agent loop from paused state.
    Resume,
    /// Execute one step then pause.
    Step,
    /// Approve the pending action.
    ApproveAction,
    /// Reject the pending action.
    RejectAction,
    /// Cancel the agent loop entirely.
    CancelAgent,
}

/// Display status for a single plan step in the TUI.
#[derive(Debug, Clone)]
pub struct PlanStepStatus {
    pub description: String,
    pub tool_name: Option<String>,
    pub status: PlanStepDisplayStatus,
    pub duration_ms: Option<u64>,
}

/// Visual state of a plan step.
#[derive(Debug, Clone, PartialEq)]
pub enum PlanStepDisplayStatus {
    Pending,
    InProgress,
    Succeeded,
    Failed,
    Skipped,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_chunk_construction() {
        let ev = UiEvent::StreamChunk("hello".into());
        assert!(matches!(ev, UiEvent::StreamChunk(ref s) if s == "hello"));
    }

    #[test]
    fn stream_code_block_construction() {
        let ev = UiEvent::StreamCodeBlock {
            lang: "rust".into(),
            code: "fn main() {}".into(),
        };
        assert!(matches!(ev, UiEvent::StreamCodeBlock { ref lang, .. } if lang == "rust"));
    }

    #[test]
    fn tool_start_construction() {
        let ev = UiEvent::ToolStart {
            name: "file_read".into(),
            input: serde_json::json!({"path": "test.rs"}),
        };
        assert!(matches!(ev, UiEvent::ToolStart { ref name, .. } if name == "file_read"));
    }

    #[test]
    fn tool_output_construction() {
        let ev = UiEvent::ToolOutput {
            name: "bash".into(),
            content: "output".into(),
            is_error: false,
            duration_ms: 42,
        };
        assert!(matches!(ev, UiEvent::ToolOutput { duration_ms: 42, .. }));
    }

    #[test]
    fn warning_with_hint() {
        let ev = UiEvent::Warning {
            message: "something".into(),
            hint: Some("try this".into()),
        };
        assert!(matches!(ev, UiEvent::Warning { hint: Some(_), .. }));
    }

    #[test]
    fn info_construction() {
        let ev = UiEvent::Info("round separator".into());
        assert!(matches!(ev, UiEvent::Info(ref s) if s == "round separator"));
    }

    #[test]
    fn status_update_partial() {
        let ev = UiEvent::StatusUpdate {
            provider: Some("anthropic".into()),
            model: None,
            round: Some(1),
            tokens: None,
            cost: None,
            session_id: Some("abc12345".into()),
            elapsed_ms: Some(1500),
            tool_count: Some(3),
            input_tokens: Some(1200),
            output_tokens: Some(450),
        };
        assert!(matches!(ev, UiEvent::StatusUpdate { round: Some(1), .. }));
    }

    #[test]
    fn plan_progress_construction() {
        let ev = UiEvent::PlanProgress {
            goal: "Fix bug".into(),
            steps: vec![
                PlanStepStatus {
                    description: "Read file".into(),
                    tool_name: Some("file_read".into()),
                    status: PlanStepDisplayStatus::Succeeded,
                    duration_ms: Some(120),
                },
                PlanStepStatus {
                    description: "Edit file".into(),
                    tool_name: Some("file_edit".into()),
                    status: PlanStepDisplayStatus::InProgress,
                    duration_ms: None,
                },
            ],
            current_step: 1,
            elapsed_ms: 500,
        };
        assert!(matches!(ev, UiEvent::PlanProgress { current_step: 1, .. }));
    }

    #[test]
    fn plan_step_display_status_eq() {
        assert_eq!(PlanStepDisplayStatus::Pending, PlanStepDisplayStatus::Pending);
        assert_ne!(PlanStepDisplayStatus::Succeeded, PlanStepDisplayStatus::Failed);
    }

    // --- Phase 42B: Cockpit event construction tests ---

    #[test]
    fn round_started_construction() {
        let ev = UiEvent::RoundStarted { round: 1, provider: "deepseek".into(), model: "deepseek-chat".into() };
        assert!(matches!(ev, UiEvent::RoundStarted { round: 1, .. }));
    }

    #[test]
    fn round_ended_construction() {
        let ev = UiEvent::RoundEnded { round: 2, input_tokens: 500, output_tokens: 200, cost: 0.002, duration_ms: 1500 };
        assert!(matches!(ev, UiEvent::RoundEnded { round: 2, duration_ms: 1500, .. }));
    }

    #[test]
    fn model_selected_construction() {
        let ev = UiEvent::ModelSelected { model: "gpt-4o".into(), provider: "openai".into(), reason: "complex task".into() };
        assert!(matches!(ev, UiEvent::ModelSelected { ref model, .. } if model == "gpt-4o"));
    }

    #[test]
    fn provider_fallback_construction() {
        let ev = UiEvent::ProviderFallback { from: "anthropic".into(), to: "deepseek".into(), reason: "auth error".into() };
        assert!(matches!(ev, UiEvent::ProviderFallback { ref from, .. } if from == "anthropic"));
    }

    #[test]
    fn loop_guard_action_construction() {
        let ev = UiEvent::LoopGuardAction { action: "inject_synthesis".into(), reason: "round 3".into() };
        assert!(matches!(ev, UiEvent::LoopGuardAction { ref action, .. } if action == "inject_synthesis"));
    }

    #[test]
    fn compaction_complete_construction() {
        let ev = UiEvent::CompactionComplete { old_msgs: 50, new_msgs: 10, tokens_saved: 4000 };
        assert!(matches!(ev, UiEvent::CompactionComplete { old_msgs: 50, .. }));
    }

    #[test]
    fn cache_status_construction() {
        let ev = UiEvent::CacheStatus { hit: true, source: "response_cache".into() };
        assert!(matches!(ev, UiEvent::CacheStatus { hit: true, .. }));
    }

    #[test]
    fn speculative_result_construction() {
        let ev = UiEvent::SpeculativeResult { tool: "file_read".into(), hit: true };
        assert!(matches!(ev, UiEvent::SpeculativeResult { hit: true, .. }));
    }

    #[test]
    fn permission_awaiting_construction() {
        let ev = UiEvent::PermissionAwaiting {
            tool: "bash".into(),
            args: serde_json::json!({"command": "ls"}),
            risk_level: "Low".into(),
        };
        assert!(matches!(ev, UiEvent::PermissionAwaiting { ref tool, .. } if tool == "bash"));
    }

    // --- Phase 42D: Control event tests ---

    #[test]
    fn control_event_variants() {
        let events = vec![
            ControlEvent::Pause,
            ControlEvent::Resume,
            ControlEvent::Step,
            ControlEvent::ApproveAction,
            ControlEvent::RejectAction,
            ControlEvent::CancelAgent,
        ];
        assert_eq!(events.len(), 6);
        assert_eq!(ControlEvent::Pause, ControlEvent::Pause);
        assert_ne!(ControlEvent::Pause, ControlEvent::Resume);
        assert_ne!(ControlEvent::Step, ControlEvent::ApproveAction);
    }

    // --- Phase 43C: Feedback completeness event tests ---

    #[test]
    fn reflection_started_construction() {
        let ev = UiEvent::ReflectionStarted;
        assert!(matches!(ev, UiEvent::ReflectionStarted));
    }

    #[test]
    fn reflection_complete_construction() {
        let ev = UiEvent::ReflectionComplete { analysis: "2 failures detected".into(), score: 0.7 };
        assert!(matches!(ev, UiEvent::ReflectionComplete { score, .. } if (score - 0.7).abs() < f64::EPSILON));
    }

    #[test]
    fn consolidation_status_construction() {
        let ev = UiEvent::ConsolidationStatus { action: "merging 25 reflections".into() };
        assert!(matches!(ev, UiEvent::ConsolidationStatus { ref action } if action.contains("merging")));
    }

    #[test]
    fn tool_retrying_construction() {
        let ev = UiEvent::ToolRetrying { tool: "bash".into(), attempt: 2, max_attempts: 3, delay_ms: 500 };
        assert!(matches!(ev, UiEvent::ToolRetrying { attempt: 2, max_attempts: 3, .. }));
    }

    // --- Phase 43D: Live panel data event tests ---

    #[test]
    fn context_tier_update_construction() {
        let ev = UiEvent::ContextTierUpdate {
            l0_tokens: 500, l0_capacity: 2000,
            l1_tokens: 300, l1_entries: 5,
            l2_entries: 10, l3_entries: 8, l4_entries: 3,
            total_tokens: 1200,
        };
        assert!(matches!(ev, UiEvent::ContextTierUpdate { l0_tokens: 500, total_tokens: 1200, .. }));
    }

    #[test]
    fn dry_run_active_construction() {
        let ev = UiEvent::DryRunActive(true);
        assert!(matches!(ev, UiEvent::DryRunActive(true)));
        let ev2 = UiEvent::DryRunActive(false);
        assert!(matches!(ev2, UiEvent::DryRunActive(false)));
    }

    #[test]
    fn token_budget_update_construction() {
        let ev = UiEvent::TokenBudgetUpdate { used: 500, limit: 1000, rate_per_minute: 120.5 };
        assert!(matches!(ev, UiEvent::TokenBudgetUpdate { used: 500, limit: 1000, .. }));
    }

    #[test]
    fn provider_health_update_construction() {
        let ev = UiEvent::ProviderHealthUpdate {
            provider: "anthropic".into(),
            status: ProviderHealthStatus::Degraded { failure_rate: 0.3, latency_p95_ms: 5000 },
        };
        assert!(matches!(ev, UiEvent::ProviderHealthUpdate { ref provider, .. } if provider == "anthropic"));
    }

    #[test]
    fn provider_health_status_variants() {
        let healthy = ProviderHealthStatus::Healthy;
        let degraded = ProviderHealthStatus::Degraded { failure_rate: 0.2, latency_p95_ms: 3000 };
        let unhealthy = ProviderHealthStatus::Unhealthy { reason: "timeout".into() };
        assert_eq!(healthy, ProviderHealthStatus::Healthy);
        assert_ne!(healthy, degraded);
        assert_ne!(degraded, unhealthy);
    }

    #[test]
    fn reasoning_update_construction() {
        let ev = UiEvent::ReasoningUpdate {
            strategy: "PlanExecuteReflect".into(),
            task_type: "CodeModification".into(),
            complexity: "Complex".into(),
        };
        assert!(matches!(ev, UiEvent::ReasoningUpdate { ref strategy, .. } if strategy == "PlanExecuteReflect"));
    }

    // --- Phase B4: Circuit breaker event tests ---

    #[test]
    fn circuit_breaker_update_construction() {
        let ev = UiEvent::CircuitBreakerUpdate {
            provider: "anthropic".into(),
            state: CircuitBreakerState::Open,
            failure_count: 5,
        };
        assert!(matches!(ev, UiEvent::CircuitBreakerUpdate { failure_count: 5, .. }));
    }

    #[test]
    fn circuit_breaker_state_variants() {
        assert_eq!(CircuitBreakerState::Closed, CircuitBreakerState::Closed);
        assert_ne!(CircuitBreakerState::Open, CircuitBreakerState::HalfOpen);
        assert_ne!(CircuitBreakerState::Closed, CircuitBreakerState::Open);
    }

    // --- Phase B5: Agent state transition tests ---

    #[test]
    fn agent_state_transition_construction() {
        let ev = UiEvent::AgentStateTransition {
            from: AgentState::Idle,
            to: AgentState::Planning,
            reason: "new task".into(),
        };
        assert!(matches!(ev, UiEvent::AgentStateTransition { ref reason, .. } if reason == "new task"));
    }

    #[test]
    fn agent_state_variants() {
        assert_eq!(AgentState::Idle, AgentState::Idle);
        assert_ne!(AgentState::Planning, AgentState::Executing);
        assert_ne!(AgentState::ToolWait, AgentState::Reflecting);
        assert_ne!(AgentState::Complete, AgentState::Failed);
        assert_ne!(AgentState::Paused, AgentState::Idle);
    }

    // --- Sprint 2: FSM transition validation tests ---

    #[test]
    fn fsm_idle_can_transition_to_planning() {
        assert!(AgentState::Idle.can_transition_to(&AgentState::Planning));
    }

    #[test]
    fn fsm_idle_can_transition_to_executing() {
        assert!(AgentState::Idle.can_transition_to(&AgentState::Executing));
    }

    #[test]
    fn fsm_idle_cannot_transition_to_complete() {
        assert!(!AgentState::Idle.can_transition_to(&AgentState::Complete));
    }

    #[test]
    fn fsm_executing_can_fail() {
        assert!(AgentState::Executing.can_transition_to(&AgentState::Failed));
    }

    #[test]
    fn fsm_planning_can_fail() {
        assert!(AgentState::Planning.can_transition_to(&AgentState::Failed));
    }

    #[test]
    fn fsm_toolwait_can_fail() {
        assert!(AgentState::ToolWait.can_transition_to(&AgentState::Failed));
    }

    #[test]
    fn fsm_reflecting_can_fail() {
        assert!(AgentState::Reflecting.can_transition_to(&AgentState::Failed));
    }

    #[test]
    fn fsm_executing_can_pause() {
        assert!(AgentState::Executing.can_transition_to(&AgentState::Paused));
    }

    #[test]
    fn fsm_paused_can_resume_to_executing() {
        assert!(AgentState::Paused.can_transition_to(&AgentState::Executing));
    }

    #[test]
    fn fsm_complete_can_return_to_idle() {
        assert!(AgentState::Complete.can_transition_to(&AgentState::Idle));
    }

    #[test]
    fn fsm_failed_can_return_to_idle() {
        assert!(AgentState::Failed.can_transition_to(&AgentState::Idle));
    }

    #[test]
    fn fsm_failed_cannot_transition_to_executing() {
        assert!(!AgentState::Failed.can_transition_to(&AgentState::Executing));
    }
}
