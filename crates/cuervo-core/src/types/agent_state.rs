//! Formal agent state machine for structured lifecycle tracking.
//!
//! Replaces implicit state (loop iteration) with an explicit state machine
//! that enables checkpointing, debugging, and external observability.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The discrete states an agent can occupy during execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// Agent is initialized but has not started processing.
    Idle,
    /// Agent is generating or refining an execution plan.
    Planning,
    /// Agent is invoking the model provider.
    Executing,
    /// Agent is waiting for tool execution results.
    ToolWait,
    /// Agent is evaluating results and deciding next steps.
    Reflecting,
    /// Agent has finished successfully.
    Complete,
    /// Agent has terminated due to an error.
    Failed,
}

impl AgentState {
    /// Check whether a transition from `self` to `next` is valid.
    pub fn can_transition_to(&self, next: AgentState) -> bool {
        use AgentState::*;
        matches!(
            (self, next),
            (Idle, Planning)
                | (Idle, Executing)
                | (Planning, Executing)
                | (Executing, ToolWait)
                | (Executing, Complete)
                | (Executing, Failed)
                | (Executing, Reflecting)
                | (ToolWait, Reflecting)
                | (ToolWait, Executing)
                | (Reflecting, Planning)
                | (Reflecting, Executing)
                | (Reflecting, Complete)
        )
    }
}

impl std::fmt::Display for AgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentState::Idle => write!(f, "idle"),
            AgentState::Planning => write!(f, "planning"),
            AgentState::Executing => write!(f, "executing"),
            AgentState::ToolWait => write!(f, "tool_wait"),
            AgentState::Reflecting => write!(f, "reflecting"),
            AgentState::Complete => write!(f, "complete"),
            AgentState::Failed => write!(f, "failed"),
        }
    }
}

/// A recorded state transition within the agent lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateTransition {
    /// Execution ID of the agent loop run.
    pub execution_id: Uuid,
    /// Previous state.
    pub from: AgentState,
    /// New state.
    pub to: AgentState,
    /// Round number when the transition occurred.
    pub round: usize,
    /// Timestamp of the transition.
    pub timestamp: DateTime<Utc>,
    /// Optional reason for the transition.
    pub reason: Option<String>,
}

/// Formal state machine tracking agent lifecycle.
pub struct AgentStateMachine {
    current: AgentState,
    execution_id: Uuid,
    transitions: Vec<StateTransition>,
}

impl AgentStateMachine {
    /// Create a new state machine starting in `Idle`.
    pub fn new(execution_id: Uuid) -> Self {
        Self {
            current: AgentState::Idle,
            execution_id,
            transitions: Vec::new(),
        }
    }

    /// Get the current state.
    pub fn current(&self) -> AgentState {
        self.current
    }

    /// Attempt a state transition. Returns the recorded transition on success,
    /// or an error string describing the invalid transition.
    pub fn transition(
        &mut self,
        to: AgentState,
        round: usize,
        timestamp: DateTime<Utc>,
        reason: Option<String>,
    ) -> Result<&StateTransition, String> {
        if !self.current.can_transition_to(to) {
            return Err(format!(
                "invalid transition: {} -> {}",
                self.current, to,
            ));
        }
        let transition = StateTransition {
            execution_id: self.execution_id,
            from: self.current,
            to,
            round,
            timestamp,
            reason,
        };
        self.current = to;
        self.transitions.push(transition);
        Ok(self.transitions.last().unwrap())
    }

    /// Check if the agent is in a terminal state (Complete or Failed).
    pub fn is_terminal(&self) -> bool {
        matches!(self.current, AgentState::Complete | AgentState::Failed)
    }

    /// Get all recorded transitions.
    pub fn transitions(&self) -> &[StateTransition] {
        &self.transitions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn state_idle_to_planning() {
        let mut sm = AgentStateMachine::new(Uuid::new_v4());
        assert!(sm.transition(AgentState::Planning, 0, now(), None).is_ok());
        assert_eq!(sm.current(), AgentState::Planning);
    }

    #[test]
    fn state_idle_to_executing() {
        let mut sm = AgentStateMachine::new(Uuid::new_v4());
        assert!(sm
            .transition(AgentState::Executing, 0, now(), None)
            .is_ok());
        assert_eq!(sm.current(), AgentState::Executing);
    }

    #[test]
    fn state_planning_to_executing() {
        let mut sm = AgentStateMachine::new(Uuid::new_v4());
        sm.transition(AgentState::Planning, 0, now(), None).unwrap();
        assert!(sm
            .transition(AgentState::Executing, 0, now(), None)
            .is_ok());
    }

    #[test]
    fn state_executing_to_tool_wait() {
        let mut sm = AgentStateMachine::new(Uuid::new_v4());
        sm.transition(AgentState::Executing, 0, now(), None)
            .unwrap();
        assert!(sm
            .transition(AgentState::ToolWait, 0, now(), None)
            .is_ok());
    }

    #[test]
    fn state_executing_to_complete() {
        let mut sm = AgentStateMachine::new(Uuid::new_v4());
        sm.transition(AgentState::Executing, 0, now(), None)
            .unwrap();
        assert!(sm
            .transition(AgentState::Complete, 0, now(), None)
            .is_ok());
    }

    #[test]
    fn state_executing_to_failed() {
        let mut sm = AgentStateMachine::new(Uuid::new_v4());
        sm.transition(AgentState::Executing, 0, now(), None)
            .unwrap();
        assert!(sm.transition(AgentState::Failed, 0, now(), None).is_ok());
    }

    #[test]
    fn state_tool_wait_to_reflecting() {
        let mut sm = AgentStateMachine::new(Uuid::new_v4());
        sm.transition(AgentState::Executing, 0, now(), None)
            .unwrap();
        sm.transition(AgentState::ToolWait, 0, now(), None).unwrap();
        assert!(sm
            .transition(AgentState::Reflecting, 0, now(), None)
            .is_ok());
    }

    #[test]
    fn state_reflecting_to_planning() {
        let mut sm = AgentStateMachine::new(Uuid::new_v4());
        sm.transition(AgentState::Executing, 0, now(), None)
            .unwrap();
        sm.transition(AgentState::ToolWait, 0, now(), None).unwrap();
        sm.transition(AgentState::Reflecting, 0, now(), None)
            .unwrap();
        assert!(sm.transition(AgentState::Planning, 1, now(), None).is_ok());
    }

    #[test]
    fn state_reflecting_to_executing() {
        let mut sm = AgentStateMachine::new(Uuid::new_v4());
        sm.transition(AgentState::Executing, 0, now(), None)
            .unwrap();
        sm.transition(AgentState::ToolWait, 0, now(), None).unwrap();
        sm.transition(AgentState::Reflecting, 0, now(), None)
            .unwrap();
        assert!(sm
            .transition(AgentState::Executing, 1, now(), None)
            .is_ok());
    }

    #[test]
    fn state_reflecting_to_complete() {
        let mut sm = AgentStateMachine::new(Uuid::new_v4());
        sm.transition(AgentState::Executing, 0, now(), None)
            .unwrap();
        sm.transition(AgentState::Reflecting, 0, now(), None)
            .unwrap();
        assert!(sm
            .transition(AgentState::Complete, 0, now(), None)
            .is_ok());
    }

    #[test]
    fn state_invalid_transition_rejected() {
        let mut sm = AgentStateMachine::new(Uuid::new_v4());
        // Idle -> Complete is not valid
        let result = sm.transition(AgentState::Complete, 0, now(), None);
        assert!(result.is_err());
        assert_eq!(sm.current(), AgentState::Idle);
    }

    #[test]
    fn state_terminal_check() {
        let mut sm = AgentStateMachine::new(Uuid::new_v4());
        assert!(!sm.is_terminal());
        sm.transition(AgentState::Executing, 0, now(), None)
            .unwrap();
        assert!(!sm.is_terminal());
        sm.transition(AgentState::Complete, 0, now(), None).unwrap();
        assert!(sm.is_terminal());
    }

    #[test]
    fn state_machine_full_lifecycle() {
        let mut sm = AgentStateMachine::new(Uuid::new_v4());
        sm.transition(AgentState::Planning, 0, now(), Some("initial plan".into()))
            .unwrap();
        sm.transition(AgentState::Executing, 0, now(), None)
            .unwrap();
        sm.transition(AgentState::ToolWait, 0, now(), None).unwrap();
        sm.transition(AgentState::Reflecting, 0, now(), None)
            .unwrap();
        sm.transition(AgentState::Executing, 1, now(), None)
            .unwrap();
        sm.transition(AgentState::Complete, 1, now(), Some("done".into()))
            .unwrap();
        assert!(sm.is_terminal());
        assert_eq!(sm.transitions().len(), 6);
    }

    #[test]
    fn state_machine_transitions_recorded() {
        let eid = Uuid::new_v4();
        let mut sm = AgentStateMachine::new(eid);
        sm.transition(AgentState::Executing, 0, now(), None)
            .unwrap();
        sm.transition(AgentState::Complete, 0, now(), None).unwrap();

        let transitions = sm.transitions();
        assert_eq!(transitions.len(), 2);
        assert_eq!(transitions[0].from, AgentState::Idle);
        assert_eq!(transitions[0].to, AgentState::Executing);
        assert_eq!(transitions[1].from, AgentState::Executing);
        assert_eq!(transitions[1].to, AgentState::Complete);
        assert_eq!(transitions[0].execution_id, eid);
    }

    #[test]
    fn state_serde_roundtrip() {
        let state = AgentState::ToolWait;
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, r#""tool_wait""#);
        let parsed: AgentState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, state);
    }
}
