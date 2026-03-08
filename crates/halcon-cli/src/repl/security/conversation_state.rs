//! Conversation state machine for multi-turn permission dialogues.
//!
//! Phase I-1 of Questionnaire SOTA Audit (Feb 14, 2026)
//!
//! This module implements a deterministic FSM that tracks the state of a permission
//! dialogue and handles transitions based on user messages. States range from Idle
//! → Prompting → (various clarification states) → Resolved/TimedOut.

use std::time::{Duration, Instant};

use halcon_core::types::PermissionDecision;

use super::conversation_protocol::{BatchScope, DetailAspect, PermissionMessage};

/// FSM for multi-turn permission dialogue.
///
/// State transitions are deterministic and always progress toward Resolved or TimedOut.
/// Intermediate states represent ongoing clarification/modification requests.
#[derive(Debug, Clone, PartialEq)]
pub enum ConversationState {
    /// No active conversation.
    Idle,

    /// Initial permission prompt shown, awaiting user response.
    Prompting {
        tool: String,
        args: serde_json::Value,
        /// Timestamp when prompt started (for timeout).
        started_at: Instant,
    },

    /// User asked question, awaiting answer (future: could call model).
    ///
    /// For now, we'll provide canned answers or feedback to agent.
    RespondingToQuestion {
        tool: String,
        args: serde_json::Value,
        question: String,
    },

    /// User requested modification, validating if feasible.
    ValidatingModification {
        tool: String,
        original_args: serde_json::Value,
        requested_change: String,
    },

    /// User requested more details, rendering progressive disclosure.
    ShowingDetails {
        tool: String,
        args: serde_json::Value,
        aspect: DetailAspect,
    },

    /// User deferred decision (waiting for user to check something).
    Deferred {
        tool: String,
        args: serde_json::Value,
        reason: Option<String>,
        deferred_at: Instant,
    },

    /// Conversation resolved with decision.
    Resolved {
        decision: PermissionDecision,
        /// Optional feedback to send to agent loop.
        feedback: Option<String>,
    },

    /// Conversation timed out (fail-safe denial).
    TimedOut,
}

impl ConversationState {
    /// Check if state has exceeded timeout.
    pub fn is_timed_out(&self, timeout: Duration) -> bool {
        match self {
            ConversationState::Prompting { started_at, .. }
            | ConversationState::Deferred { deferred_at: started_at, .. } => {
                started_at.elapsed() > timeout
            }
            _ => false,
        }
    }

    /// Transition to next state based on user message.
    ///
    /// Returns `StateTransition` describing what happened.
    pub fn transition(&mut self, msg: PermissionMessage) -> StateTransition {
        let current = self.clone();

        match (current, msg) {
            // From Prompting → direct decisions
            (ConversationState::Prompting { .. }, PermissionMessage::Approve) => {
                *self = ConversationState::Resolved {
                    decision: PermissionDecision::Allowed,
                    feedback: None,
                };
                StateTransition::Approved
            }

            (ConversationState::Prompting { .. }, PermissionMessage::Reject) => {
                *self = ConversationState::Resolved {
                    decision: PermissionDecision::Denied,
                    feedback: Some("User rejected without clarification".into()),
                };
                StateTransition::Denied
            }

            // From Prompting → clarification states
            (
                ConversationState::Prompting { tool, args, .. },
                PermissionMessage::AskQuestion { question },
            ) => {
                *self = ConversationState::RespondingToQuestion {
                    tool: tool.clone(),
                    args: args.clone(),
                    question: question.clone(),
                };
                StateTransition::NeedsClarification { question }
            }

            (
                ConversationState::Prompting { tool, args, .. },
                PermissionMessage::ModifyParameters { clarification },
            ) => {
                *self = ConversationState::ValidatingModification {
                    tool: tool.clone(),
                    original_args: args.clone(),
                    requested_change: clarification.clone(),
                };
                StateTransition::ValidatingChange { change: clarification }
            }

            (
                ConversationState::Prompting { tool, args, .. },
                PermissionMessage::RequestDetails { aspect },
            ) => {
                *self = ConversationState::ShowingDetails {
                    tool: tool.clone(),
                    args: args.clone(),
                    aspect: aspect.clone(),
                };
                StateTransition::ShowDetails { aspect }
            }

            (
                ConversationState::Prompting { tool, args, .. },
                PermissionMessage::Defer { reason },
            ) => {
                *self = ConversationState::Deferred {
                    tool: tool.clone(),
                    args: args.clone(),
                    reason: reason.clone(),
                    deferred_at: Instant::now(),
                };
                StateTransition::Deferred
            }

            (ConversationState::Prompting { .. }, PermissionMessage::SuggestAlternative { suggestion }) => {
                *self = ConversationState::Resolved {
                    decision: PermissionDecision::Denied,
                    feedback: Some(format!("User suggested safer alternative: {}", suggestion)),
                };
                StateTransition::Denied
            }

            (ConversationState::Prompting { .. }, PermissionMessage::BatchApprove { scope }) => {
                *self = ConversationState::Resolved {
                    decision: PermissionDecision::AllowedAlways, // Treat batch as "always" for now
                    feedback: Some(format!("User approved in batch: {:?}", scope)),
                };
                StateTransition::Approved
            }

            // From ShowingDetails → back to decision
            (ConversationState::ShowingDetails { tool, args, .. }, PermissionMessage::Approve) => {
                *self = ConversationState::Resolved {
                    decision: PermissionDecision::Allowed,
                    feedback: None,
                };
                StateTransition::Approved
            }

            (ConversationState::ShowingDetails { tool, args, .. }, PermissionMessage::Reject) => {
                *self = ConversationState::Resolved {
                    decision: PermissionDecision::Denied,
                    feedback: Some("User rejected after viewing details".into()),
                };
                StateTransition::Denied
            }

            // From ShowingDetails → another detail aspect
            (
                ConversationState::ShowingDetails { tool, args, .. },
                PermissionMessage::RequestDetails { aspect },
            ) => {
                *self = ConversationState::ShowingDetails {
                    tool,
                    args,
                    aspect: aspect.clone(),
                };
                StateTransition::ShowDetails { aspect }
            }

            // From RespondingToQuestion → back to prompting or decision
            (
                ConversationState::RespondingToQuestion { tool, args, .. },
                PermissionMessage::Approve,
            ) => {
                *self = ConversationState::Resolved {
                    decision: PermissionDecision::Allowed,
                    feedback: None,
                };
                StateTransition::Approved
            }

            (
                ConversationState::RespondingToQuestion { tool, args, .. },
                PermissionMessage::Reject,
            ) => {
                *self = ConversationState::Resolved {
                    decision: PermissionDecision::Denied,
                    feedback: Some("User rejected after clarification".into()),
                };
                StateTransition::Denied
            }

            // From ValidatingModification → send feedback to agent
            (
                ConversationState::ValidatingModification { tool, original_args, requested_change },
                PermissionMessage::Approve,
            ) => {
                *self = ConversationState::Resolved {
                    decision: PermissionDecision::Denied, // Deny original, send feedback
                    feedback: Some(format!(
                        "User requested parameter modification: {}. Please replan with this feedback.",
                        requested_change
                    )),
                };
                StateTransition::Denied
            }

            (
                ConversationState::ValidatingModification { .. },
                PermissionMessage::Reject,
            ) => {
                *self = ConversationState::Resolved {
                    decision: PermissionDecision::Denied,
                    feedback: Some("User rejected modification and original proposal".into()),
                };
                StateTransition::Denied
            }

            // From Deferred → resume or timeout
            (
                ConversationState::Deferred { tool, args, .. },
                PermissionMessage::Approve,
            ) => {
                *self = ConversationState::Resolved {
                    decision: PermissionDecision::Allowed,
                    feedback: None,
                };
                StateTransition::Approved
            }

            (
                ConversationState::Deferred { tool, args, .. },
                PermissionMessage::Reject,
            ) => {
                *self = ConversationState::Resolved {
                    decision: PermissionDecision::Denied,
                    feedback: Some("User rejected after deferring".into()),
                };
                StateTransition::Denied
            }

            // Invalid transitions
            _ => StateTransition::InvalidTransition,
        }
    }

    /// Force timeout transition if elapsed time exceeds limit.
    pub fn maybe_timeout(&mut self, timeout: Duration) {
        if self.is_timed_out(timeout) {
            *self = ConversationState::TimedOut;
        }
    }

    /// Check if conversation is complete (Resolved or TimedOut).
    pub fn is_complete(&self) -> bool {
        matches!(
            self,
            ConversationState::Resolved { .. } | ConversationState::TimedOut
        )
    }

    /// Extract final decision if conversation is resolved.
    pub fn final_decision(&self) -> Option<PermissionDecision> {
        match self {
            ConversationState::Resolved { decision, .. } => Some(*decision),
            ConversationState::TimedOut => Some(PermissionDecision::Denied),
            _ => None,
        }
    }

    /// Extract feedback for agent loop if available.
    pub fn feedback(&self) -> Option<String> {
        match self {
            ConversationState::Resolved { feedback, .. } => feedback.clone(),
            _ => None,
        }
    }
}

/// Result of a state transition.
#[derive(Debug, PartialEq)]
pub enum StateTransition {
    Approved,
    Denied,
    NeedsClarification { question: String },
    ValidatingChange { change: String },
    ShowDetails { aspect: DetailAspect },
    Deferred,
    InvalidTransition,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_prompting_state() -> ConversationState {
        ConversationState::Prompting {
            tool: "bash".into(),
            args: serde_json::json!({"command": "ls"}),
            started_at: Instant::now(),
        }
    }

    #[test]
    fn idle_to_prompting() {
        let mut state = ConversationState::Idle;
        state = ConversationState::Prompting {
            tool: "test".into(),
            args: serde_json::json!({}),
            started_at: Instant::now(),
        };
        assert!(matches!(state, ConversationState::Prompting { .. }));
    }

    #[test]
    fn prompting_approve() {
        let mut state = make_prompting_state();
        let transition = state.transition(PermissionMessage::Approve);
        assert_eq!(transition, StateTransition::Approved);
        assert!(matches!(
            state,
            ConversationState::Resolved {
                decision: PermissionDecision::Allowed,
                feedback: None
            }
        ));
    }

    #[test]
    fn prompting_reject() {
        let mut state = make_prompting_state();
        let transition = state.transition(PermissionMessage::Reject);
        assert_eq!(transition, StateTransition::Denied);
        assert!(matches!(
            state,
            ConversationState::Resolved {
                decision: PermissionDecision::Denied,
                ..
            }
        ));
    }

    #[test]
    fn prompting_ask_question() {
        let mut state = make_prompting_state();
        let transition = state.transition(PermissionMessage::AskQuestion {
            question: "Why?".into(),
        });
        assert_eq!(
            transition,
            StateTransition::NeedsClarification {
                question: "Why?".into()
            }
        );
        assert!(matches!(
            state,
            ConversationState::RespondingToQuestion { .. }
        ));
    }

    #[test]
    fn prompting_modify_parameters() {
        let mut state = make_prompting_state();
        let transition = state.transition(PermissionMessage::ModifyParameters {
            clarification: "use /tmp".into(),
        });
        assert_eq!(
            transition,
            StateTransition::ValidatingChange {
                change: "use /tmp".into()
            }
        );
        assert!(matches!(
            state,
            ConversationState::ValidatingModification { .. }
        ));
    }

    #[test]
    fn prompting_request_details() {
        let mut state = make_prompting_state();
        let transition = state.transition(PermissionMessage::RequestDetails {
            aspect: DetailAspect::Parameters,
        });
        assert_eq!(
            transition,
            StateTransition::ShowDetails {
                aspect: DetailAspect::Parameters
            }
        );
        assert!(matches!(state, ConversationState::ShowingDetails { .. }));
    }

    #[test]
    fn prompting_defer() {
        let mut state = make_prompting_state();
        let transition = state.transition(PermissionMessage::Defer { reason: None });
        assert_eq!(transition, StateTransition::Deferred);
        assert!(matches!(state, ConversationState::Deferred { .. }));
    }

    #[test]
    fn showing_details_approve() {
        let mut state = ConversationState::ShowingDetails {
            tool: "bash".into(),
            args: serde_json::json!({}),
            aspect: DetailAspect::Parameters,
        };
        let transition = state.transition(PermissionMessage::Approve);
        assert_eq!(transition, StateTransition::Approved);
        assert!(matches!(
            state,
            ConversationState::Resolved {
                decision: PermissionDecision::Allowed,
                ..
            }
        ));
    }

    #[test]
    fn showing_details_switch_aspect() {
        let mut state = ConversationState::ShowingDetails {
            tool: "bash".into(),
            args: serde_json::json!({}),
            aspect: DetailAspect::Parameters,
        };
        let transition = state.transition(PermissionMessage::RequestDetails {
            aspect: DetailAspect::RiskAssessment,
        });
        assert_eq!(
            transition,
            StateTransition::ShowDetails {
                aspect: DetailAspect::RiskAssessment
            }
        );
        if let ConversationState::ShowingDetails { aspect, .. } = state {
            assert_eq!(aspect, DetailAspect::RiskAssessment);
        } else {
            panic!("Expected ShowingDetails state");
        }
    }

    #[test]
    fn responding_to_question_approve() {
        let mut state = ConversationState::RespondingToQuestion {
            tool: "bash".into(),
            args: serde_json::json!({}),
            question: "Why?".into(),
        };
        let transition = state.transition(PermissionMessage::Approve);
        assert_eq!(transition, StateTransition::Approved);
        assert!(state.is_complete());
    }

    #[test]
    fn validating_modification_approve_sends_feedback() {
        let mut state = ConversationState::ValidatingModification {
            tool: "file_write".into(),
            original_args: serde_json::json!({"path": "/tmp/test.txt"}),
            requested_change: "use /tmp/output.txt".into(),
        };
        let transition = state.transition(PermissionMessage::Approve);
        assert_eq!(transition, StateTransition::Denied); // Deny original, send feedback
        if let ConversationState::Resolved { feedback, .. } = state {
            assert!(feedback.is_some());
            assert!(feedback.unwrap().contains("modification"));
        } else {
            panic!("Expected Resolved state");
        }
    }

    #[test]
    fn deferred_approve() {
        let mut state = ConversationState::Deferred {
            tool: "bash".into(),
            args: serde_json::json!({}),
            reason: None,
            deferred_at: Instant::now(),
        };
        let transition = state.transition(PermissionMessage::Approve);
        assert_eq!(transition, StateTransition::Approved);
        assert!(state.is_complete());
    }

    #[test]
    fn timeout_detection() {
        let state = ConversationState::Prompting {
            tool: "test".into(),
            args: serde_json::json!({}),
            started_at: Instant::now() - Duration::from_secs(100),
        };
        assert!(state.is_timed_out(Duration::from_secs(60)));
        assert!(!state.is_timed_out(Duration::from_secs(120)));
    }

    #[test]
    fn maybe_timeout_forces_transition() {
        let mut state = ConversationState::Prompting {
            tool: "test".into(),
            args: serde_json::json!({}),
            started_at: Instant::now() - Duration::from_secs(100),
        };
        state.maybe_timeout(Duration::from_secs(60));
        assert_eq!(state, ConversationState::TimedOut);
    }

    #[test]
    fn is_complete_checks() {
        assert!(!ConversationState::Idle.is_complete());
        assert!(!make_prompting_state().is_complete());
        assert!(ConversationState::Resolved {
            decision: PermissionDecision::Allowed,
            feedback: None
        }
        .is_complete());
        assert!(ConversationState::TimedOut.is_complete());
    }

    #[test]
    fn final_decision_extraction() {
        let state = ConversationState::Resolved {
            decision: PermissionDecision::Allowed,
            feedback: None,
        };
        assert_eq!(state.final_decision(), Some(PermissionDecision::Allowed));

        let timeout = ConversationState::TimedOut;
        assert_eq!(timeout.final_decision(), Some(PermissionDecision::Denied));

        let prompting = make_prompting_state();
        assert_eq!(prompting.final_decision(), None);
    }

    #[test]
    fn feedback_extraction() {
        let state = ConversationState::Resolved {
            decision: PermissionDecision::Denied,
            feedback: Some("test feedback".into()),
        };
        assert_eq!(state.feedback(), Some("test feedback".into()));

        let no_feedback = ConversationState::Resolved {
            decision: PermissionDecision::Allowed,
            feedback: None,
        };
        assert_eq!(no_feedback.feedback(), None);
    }

    #[test]
    fn invalid_transition() {
        let mut state = ConversationState::Idle;
        let transition = state.transition(PermissionMessage::Approve);
        assert_eq!(transition, StateTransition::InvalidTransition);
    }
}
