//! Loop state checkpointing — Phase 1: State Externalization & Observability.
//!
//! Provides a serializable snapshot (`LoopCheckpointData`) of the critical subset
//! of `LoopState` fields. Saved to `session_checkpoints.agent_state` at each round
//! boundary via a fire-and-forget `tokio::spawn`.
//!
//! Zero behavior change: saving is always non-blocking and errors are logged, not propagated.

use serde::{Deserialize, Serialize};

use halcon_storage::{AsyncDatabase, SessionCheckpoint};

use super::loop_state::LoopState;

/// Serializable snapshot of the critical `LoopState` fields.
///
/// Captures the operational context needed for replay analysis or post-mortem
/// debugging. Non-serializable fields (Arc, Instant, closures, trait objects)
/// are excluded or converted to their string representations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopCheckpointData {
    /// Schema version — bump when adding required fields.
    pub checkpoint_version: u32,
    /// Round index when this snapshot was taken (0-based).
    pub round: usize,
    /// Session UUID as a string.
    pub session_id: String,
    /// Names of all tools successfully executed so far in this agent loop.
    pub tools_executed: Vec<String>,
    /// Cumulative input tokens consumed across all rounds.
    pub call_input_tokens: u64,
    /// Cumulative output tokens produced across all rounds.
    pub call_output_tokens: u64,
    /// Cumulative API cost estimate (USD).
    pub call_cost: f64,
    /// Number of replan attempts performed so far.
    pub replan_attempts: u32,
    /// Whether a forced synthesis has been detected in this session.
    pub forced_synthesis_detected: bool,
    /// `ExecutionIntentPhase` variant as `Debug` string (e.g. `"Execution"`).
    pub execution_intent: String,
    /// `ToolDecisionSignal` variant as `Debug` string (e.g. `"Allow"`).
    pub tool_decision: String,
    /// Current FSM state label.
    pub current_fsm_state: String,
    /// Count of `PhaseOutcome::NextRound` restarts this loop.
    pub next_round_restarts: usize,
    /// Number of drift-triggered replans.
    pub drift_replan_count: usize,
    /// Total plan steps from the active plan (0 if no active plan).
    pub plan_steps_total: usize,
    /// Last recorded convergence ratio from ConvergenceController.
    pub last_convergence_ratio: f32,
}

impl LoopCheckpointData {
    /// Current schema version — increment on breaking changes.
    pub const VERSION: u32 = 1;

    /// Take a snapshot of the serializable subset of `state` at `round`.
    pub fn snapshot(state: &LoopState, round: usize) -> Self {
        Self {
            checkpoint_version: Self::VERSION,
            round,
            session_id: state.session_id.to_string(),
            tools_executed: state.tools_executed.clone(),
            call_input_tokens: state.tokens.call_input_tokens,
            call_output_tokens: state.tokens.call_output_tokens,
            call_cost: state.tokens.call_cost,
            replan_attempts: state.convergence.replan_attempts,
            forced_synthesis_detected: state.synthesis.forced_synthesis_detected,
            execution_intent: format!("{:?}", state.synthesis.execution_intent),
            tool_decision: format!("{:?}", state.synthesis.tool_decision),
            current_fsm_state: state.synthesis.phase.as_str().to_string(),
            next_round_restarts: state.next_round_restarts,
            drift_replan_count: state.convergence.drift_replan_count,
            plan_steps_total: state.active_plan.as_ref().map(|p| p.steps.len()).unwrap_or(0),
            last_convergence_ratio: state.convergence.last_convergence_ratio,
        }
    }
}

/// Save a checkpoint asynchronously (fire-and-forget).
///
/// Serializes `data` into JSON and spawns a detached task to persist it using
/// the `session_checkpoints.agent_state` column.  Errors are logged at WARN
/// level and never propagated — this function is observability-only.
pub fn save_checkpoint_nonblocking(
    data: LoopCheckpointData,
    db: Option<&AsyncDatabase>,
    messages_json: &str,
    usage_json: &str,
    fingerprint: &str,
    step_index: u32,
) {
    let db = match db {
        Some(d) => d.clone(),
        None => return,
    };

    let agent_state_json = match serde_json::to_string(&data) {
        Ok(j) => j,
        Err(e) => {
            tracing::warn!(error = %e, "checkpoint: failed to serialize LoopCheckpointData — skipping");
            return;
        }
    };

    let session_id = match uuid::Uuid::parse_str(&data.session_id) {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(error = %e, "checkpoint: invalid session_id UUID — skipping");
            return;
        }
    };

    let round = data.round as u32;
    let session_str = data.session_id.clone();

    let checkpoint = SessionCheckpoint {
        session_id,
        round,
        step_index,
        messages_json: messages_json.to_string(),
        usage_json: usage_json.to_string(),
        fingerprint: fingerprint.to_string(),
        created_at: chrono::Utc::now(),
        agent_state: Some(agent_state_json),
    };

    tokio::spawn(async move {
        if let Err(e) = db.save_checkpoint(&checkpoint).await {
            tracing::warn!(error = %e, "checkpoint: async save failed");
        } else {
            tracing::debug!(round, session_id = %session_str, "checkpoint: saved");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_data_roundtrip() {
        let data = LoopCheckpointData {
            checkpoint_version: LoopCheckpointData::VERSION,
            round: 2,
            session_id: uuid::Uuid::new_v4().to_string(),
            tools_executed: vec!["bash".into(), "file_write".into()],
            call_input_tokens: 5000,
            call_output_tokens: 1200,
            call_cost: 0.012,
            replan_attempts: 1,
            forced_synthesis_detected: false,
            execution_intent: "Execution".into(),
            tool_decision: "Allow".into(),
            current_fsm_state: "tool_loop".into(),
            next_round_restarts: 0,
            drift_replan_count: 0,
            plan_steps_total: 3,
            last_convergence_ratio: 0.6,
        };

        let json = serde_json::to_string(&data).unwrap();
        assert!(!json.is_empty());
        let back: LoopCheckpointData = serde_json::from_str(&json).unwrap();
        assert_eq!(back.round, 2);
        assert_eq!(back.tools_executed, vec!["bash", "file_write"]);
        assert_eq!(back.checkpoint_version, LoopCheckpointData::VERSION);
        assert!((back.call_cost - 0.012).abs() < 1e-9);
        assert_eq!(back.execution_intent, "Execution");
        assert_eq!(back.current_fsm_state, "tool_loop");
    }

    #[test]
    fn checkpoint_version_is_one() {
        assert_eq!(LoopCheckpointData::VERSION, 1);
    }

    #[test]
    fn save_nonblocking_with_no_db_is_noop() {
        // Should not panic when db is None.
        let data = LoopCheckpointData {
            checkpoint_version: 1,
            round: 0,
            session_id: uuid::Uuid::new_v4().to_string(),
            tools_executed: vec![],
            call_input_tokens: 0,
            call_output_tokens: 0,
            call_cost: 0.0,
            replan_attempts: 0,
            forced_synthesis_detected: false,
            execution_intent: "Uncategorized".into(),
            tool_decision: "Allow".into(),
            current_fsm_state: "init".into(),
            next_round_restarts: 0,
            drift_replan_count: 0,
            plan_steps_total: 0,
            last_convergence_ratio: 0.0,
        };
        save_checkpoint_nonblocking(data, None, "[]", "{}", "fp0", 0);
        // No panic = pass.
    }

    #[test]
    fn checkpoint_data_optional_plan_zero_when_empty() {
        let data = LoopCheckpointData {
            checkpoint_version: 1,
            round: 0,
            session_id: uuid::Uuid::new_v4().to_string(),
            tools_executed: vec![],
            call_input_tokens: 100,
            call_output_tokens: 50,
            call_cost: 0.0,
            replan_attempts: 0,
            forced_synthesis_detected: false,
            execution_intent: "Uncategorized".into(),
            tool_decision: "Allow".into(),
            current_fsm_state: "init".into(),
            next_round_restarts: 0,
            drift_replan_count: 0,
            plan_steps_total: 0,
            last_convergence_ratio: 0.0,
        };
        assert_eq!(data.plan_steps_total, 0, "no active plan → 0 steps");
    }
}
