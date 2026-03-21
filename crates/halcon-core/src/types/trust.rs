//! Response trust classification — indicates how much confidence to place
//! in a given agent response based on its execution provenance.
//!
//! Every `AgentLoopResult` carries a `ResponseTrust` level so callers and
//! UI layers can communicate the epistemic status of the result to the user.

use serde::{Deserialize, Serialize};

/// Classification of an agent response based on how it was produced.
///
/// Determined at the end of each agent loop round in `result_assembly.rs`.
/// Displayed by render sinks as a visible badge so users understand whether
/// the response is backed by fresh tool evidence or synthesized from context.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "trust_level", rename_all = "snake_case")]
pub enum ResponseTrust {
    /// Response is backed by tool calls executed **in this round**.
    /// Highest confidence — evidence is fresh and directly verifiable.
    ToolVerified {
        tools_used: Vec<String>,
        round_id: usize,
    },
    /// Response is derived from tool calls in **earlier rounds** of this session.
    /// High confidence but evidence may be stale if underlying data changed.
    ToolDerived {
        last_tool_round: usize,
        rounds_ago: usize,
    },
    /// Response is synthesized from accumulated context without tool calls.
    /// Tools were actively suppressed this round (budget, timeout, or oracle).
    /// Medium-low confidence — accuracy depends on prior context quality.
    SynthesizedContext {
        suppression_reason: String,
        last_tool_round: Option<usize>,
        stale_rounds_count: usize,
    },
    /// No tool evidence available. Response based on model training only.
    /// Lowest confidence — cannot be verified against runtime data.
    #[default]
    Unverified,
}

impl ResponseTrust {
    /// Short badge string for display in UI (TUI status line, classic render).
    pub fn badge(&self) -> &'static str {
        match self {
            Self::ToolVerified { .. } => "✓ VERIFIED",
            Self::ToolDerived { .. } => "◈ DERIVED",
            Self::SynthesizedContext { .. } => "⚠ SYNTHESIZED",
            Self::Unverified => "○ UNVERIFIED",
        }
    }

    /// Whether this trust level indicates any form of tool-backed evidence.
    pub fn is_evidence_backed(&self) -> bool {
        matches!(self, Self::ToolVerified { .. } | Self::ToolDerived { .. })
    }

    /// Compute trust level from agent loop outcome signals.
    pub fn compute(
        tools_executed_this_round: usize,
        tools_suppressed_this_round: bool,
        last_tool_round: Option<usize>,
        current_round: usize,
        suppression_reason: Option<String>,
    ) -> Self {
        if tools_executed_this_round > 0 && !tools_suppressed_this_round {
            return Self::ToolVerified {
                tools_used: vec![], // populated by caller if needed
                round_id: current_round,
            };
        }

        if tools_suppressed_this_round {
            let stale = current_round.saturating_sub(last_tool_round.unwrap_or(0));
            return Self::SynthesizedContext {
                suppression_reason: suppression_reason
                    .unwrap_or_else(|| "tools_suppressed".to_string()),
                last_tool_round,
                stale_rounds_count: stale,
            };
        }

        if let Some(last) = last_tool_round {
            return Self::ToolDerived {
                last_tool_round: last,
                rounds_ago: current_round.saturating_sub(last),
            };
        }

        Self::Unverified
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_verified_when_tools_executed() {
        let trust = ResponseTrust::compute(3, false, Some(2), 5, None);
        assert!(matches!(
            trust,
            ResponseTrust::ToolVerified { round_id: 5, .. }
        ));
        assert_eq!(trust.badge(), "✓ VERIFIED");
        assert!(trust.is_evidence_backed());
    }

    #[test]
    fn synthesized_context_when_suppressed() {
        let trust = ResponseTrust::compute(0, true, Some(3), 7, Some("compaction_timeout".into()));
        assert!(matches!(
            trust,
            ResponseTrust::SynthesizedContext {
                stale_rounds_count: 4,
                ..
            }
        ));
        assert_eq!(trust.badge(), "⚠ SYNTHESIZED");
        assert!(!trust.is_evidence_backed());
    }

    #[test]
    fn tool_derived_when_prior_rounds_had_tools() {
        let trust = ResponseTrust::compute(0, false, Some(2), 5, None);
        assert!(matches!(
            trust,
            ResponseTrust::ToolDerived { rounds_ago: 3, .. }
        ));
        assert_eq!(trust.badge(), "◈ DERIVED");
        assert!(trust.is_evidence_backed());
    }

    #[test]
    fn unverified_when_no_tools_ever() {
        let trust = ResponseTrust::compute(0, false, None, 1, None);
        assert!(matches!(trust, ResponseTrust::Unverified));
        assert_eq!(trust.badge(), "○ UNVERIFIED");
        assert!(!trust.is_evidence_backed());
    }

    #[test]
    fn badge_strings_are_stable() {
        assert_eq!(ResponseTrust::Unverified.badge(), "○ UNVERIFIED");
        assert_eq!(
            ResponseTrust::SynthesizedContext {
                suppression_reason: "x".into(),
                last_tool_round: None,
                stale_rounds_count: 0,
            }
            .badge(),
            "⚠ SYNTHESIZED"
        );
    }

    #[test]
    fn serializes_with_trust_level_tag() {
        let t = ResponseTrust::ToolVerified {
            tools_used: vec!["bash".into()],
            round_id: 2,
        };
        let json = serde_json::to_string(&t).unwrap();
        assert!(
            json.contains("\"trust_level\":\"tool_verified\""),
            "json={json}"
        );
    }
}
