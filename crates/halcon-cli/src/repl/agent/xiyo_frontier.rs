//! Xiyo Frontier — unified pre-invocation gate for the agent loop.
//!
//! Consolidates all pre-flight validations into a single `evaluate()` call
//! that returns a typed `FrontierVerdict`. This replaces the scattered checks
//! that were previously distributed across `provider_round.rs` (budget preflight,
//! output headroom, EBS-B2) and `round_setup.rs` (capability, schema validation).
//!
//! # Design
//!
//! The frontier is a **pure function** over an immutable snapshot of session state.
//! It produces a verdict without side effects — the caller decides how to act.
//!
//! # Gate ordering (defense-in-depth)
//!
//! 1. **Budget gate**: reject if token/duration/cost budget already exceeded
//! 2. **Headroom gate**: reject if remaining tokens < minimum for useful response
//! 3. **Provider health gate**: warn if provider circuit breaker is open
//! 4. **Capability gate**: verify model supports tools when tools are requested
//! 5. **Evidence gate**: block synthesis-path rounds when evidence is insufficient
//!
//! Each gate can produce `Reject` (stop), `Warn` (proceed with caution), or `Pass`.
//! The frontier returns the FIRST rejection, or all accumulated warnings.

use std::time::Instant;

// ── FrontierVerdict ──────────────────────────────────────────────────────────

/// Decision returned by the Xiyo frontier gate.
#[derive(Debug, Clone)]
pub(super) enum FrontierVerdict {
    /// All gates passed — proceed with provider invocation.
    Proceed {
        /// Non-fatal warnings accumulated during evaluation.
        warnings: Vec<FrontierWarning>,
    },
    /// A gate rejected the round — do NOT invoke the provider.
    Reject {
        /// Which gate triggered the rejection.
        gate: FrontierGate,
        /// Human-readable reason for the rejection.
        reason: String,
        /// Suggested synthesis trigger if the round is rejected.
        suggested_trigger: SuggestedTrigger,
    },
}

/// Which frontier gate issued the rejection or warning.
///
/// Provider health is intentionally NOT a frontier gate — it's handled by
/// `ResilienceManager` which has richer state (circuit breaker, backpressure,
/// health scorer) than what a pure function can evaluate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FrontierGate {
    TokenBudget,
    DurationBudget,
    CostBudget,
    OutputHeadroom,
    ModelCapability,
    EvidenceGate,
}

/// A non-fatal warning from a frontier gate.
#[derive(Debug, Clone)]
pub(super) struct FrontierWarning {
    pub gate: FrontierGate,
    pub message: String,
}

/// Hint for the caller about how to handle a rejection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SuggestedTrigger {
    /// Use MaxRoundsReached synthesis trigger.
    BudgetExhausted,
    /// Use ToolExhaustion synthesis trigger (headroom critical).
    HeadroomCritical,
    /// Use ReplanTimeout synthesis trigger.
    DurationExceeded,
}

// ── FrontierContext ──────────────────────────────────────────────────────────

/// Immutable snapshot of session state for frontier evaluation.
///
/// Constructed by the caller from `LoopState`, `Session`, and `AgentLimits`.
/// All fields are value types — safe to pass across async boundaries.
pub(super) struct FrontierContext<'a> {
    // Budget state
    pub tokens_used: u64,
    pub max_total_tokens: u64,
    pub elapsed_secs: u64,
    pub max_duration_secs: u64,
    pub cost_usd: f64,
    pub max_cost_usd: f64,

    // Headroom
    pub output_headroom_tokens: u64,
    pub round: usize,

    // Provider state
    pub provider_name: &'a str,
    pub model_name: &'a str,
    pub model_supports_tools: bool,
    pub tool_format_known: bool,
    pub tools_in_request: bool,

    // Evidence state (for EBS-B2)
    pub evidence_gate_fires: bool,
    pub evidence_already_handled: bool,
    pub is_synthesis_round: bool,
}

// ── evaluate ─────────────────────────────────────────────────────────────────

/// Evaluate all frontier gates in order. Returns on first rejection.
///
/// Pure function — no side effects, no state mutations.
pub(super) fn evaluate(ctx: &FrontierContext<'_>) -> FrontierVerdict {
    let mut warnings = Vec::new();

    // Gate 1: Token budget
    if ctx.max_total_tokens > 0 && ctx.tokens_used >= ctx.max_total_tokens {
        tracing::info!(
            metric.frontier_reject = true,
            gate = "token_budget",
            used = ctx.tokens_used,
            budget = ctx.max_total_tokens,
            "Xiyo frontier: token budget exceeded"
        );
        return FrontierVerdict::Reject {
            gate: FrontierGate::TokenBudget,
            reason: format!(
                "Token budget exceeded: {} / {} tokens",
                ctx.tokens_used, ctx.max_total_tokens
            ),
            suggested_trigger: SuggestedTrigger::BudgetExhausted,
        };
    }

    // Gate 2: Duration budget
    if ctx.max_duration_secs > 0 && ctx.elapsed_secs >= ctx.max_duration_secs {
        tracing::info!(
            metric.frontier_reject = true,
            gate = "duration_budget",
            elapsed = ctx.elapsed_secs,
            budget = ctx.max_duration_secs,
            "Xiyo frontier: duration budget exceeded"
        );
        return FrontierVerdict::Reject {
            gate: FrontierGate::DurationBudget,
            reason: format!(
                "Duration budget exceeded: {}s / {}s",
                ctx.elapsed_secs, ctx.max_duration_secs
            ),
            suggested_trigger: SuggestedTrigger::DurationExceeded,
        };
    }

    // Gate 3: Cost budget
    if ctx.max_cost_usd > 0.0 && ctx.cost_usd >= ctx.max_cost_usd {
        tracing::info!(
            metric.frontier_reject = true,
            gate = "cost_budget",
            spent = format!("${:.4}", ctx.cost_usd),
            budget = format!("${:.2}", ctx.max_cost_usd),
            "Xiyo frontier: cost budget exceeded"
        );
        return FrontierVerdict::Reject {
            gate: FrontierGate::CostBudget,
            reason: format!(
                "Cost budget exceeded: ${:.4} / ${:.2}",
                ctx.cost_usd, ctx.max_cost_usd
            ),
            suggested_trigger: SuggestedTrigger::BudgetExhausted,
        };
    }

    // Gate 4: Output headroom (only after round 0)
    if ctx.max_total_tokens > 0 && ctx.round > 0 {
        let remaining = ctx.max_total_tokens.saturating_sub(ctx.tokens_used);
        if remaining < ctx.output_headroom_tokens {
            tracing::info!(
                metric.frontier_reject = true,
                gate = "output_headroom",
                remaining,
                headroom = ctx.output_headroom_tokens,
                "Xiyo frontier: output headroom critical"
            );
            return FrontierVerdict::Reject {
                gate: FrontierGate::OutputHeadroom,
                reason: format!(
                    "Output headroom critical: {} tokens remaining (need {})",
                    remaining, ctx.output_headroom_tokens
                ),
                suggested_trigger: SuggestedTrigger::HeadroomCritical,
            };
        }
    }

    // Gate 5: Model capability (tools requested but model can't handle them)
    if ctx.tools_in_request && !ctx.model_supports_tools {
        tracing::info!(
            metric.frontier_warn = true,
            gate = "model_capability",
            model = ctx.model_name,
            provider = ctx.provider_name,
            "Xiyo frontier: model does not support tools"
        );
        warnings.push(FrontierWarning {
            gate: FrontierGate::ModelCapability,
            message: format!(
                "Model '{}' does not support tools — tools will be stripped",
                ctx.model_name
            ),
        });
    }

    // Gate 5b: Tool format unknown with tools requested
    if ctx.tools_in_request && !ctx.tool_format_known && ctx.model_supports_tools {
        warnings.push(FrontierWarning {
            gate: FrontierGate::ModelCapability,
            message: format!(
                "Provider '{}' has Unknown tool format — tool calls may fail",
                ctx.provider_name
            ),
        });
    }

    // Gate 6: Evidence gate (EBS-B2 pre-invocation)
    if ctx.is_synthesis_round && ctx.evidence_gate_fires && !ctx.evidence_already_handled {
        tracing::info!(
            metric.frontier_reject = true,
            gate = "evidence_gate",
            "Xiyo frontier: evidence gate fired — insufficient evidence for synthesis"
        );
        return FrontierVerdict::Reject {
            gate: FrontierGate::EvidenceGate,
            reason: "Evidence gate: insufficient extracted text for synthesis".to_string(),
            suggested_trigger: SuggestedTrigger::HeadroomCritical,
        };
    }

    // Log frontier passage
    if !warnings.is_empty() {
        tracing::debug!(
            warning_count = warnings.len(),
            "Xiyo frontier: passed with {} warning(s)",
            warnings.len()
        );
    }

    FrontierVerdict::Proceed { warnings }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_default() -> FrontierContext<'static> {
        FrontierContext {
            tokens_used: 1000,
            max_total_tokens: 100_000,
            elapsed_secs: 30,
            max_duration_secs: 300,
            cost_usd: 0.01,
            max_cost_usd: 1.0,
            output_headroom_tokens: 5000,
            round: 3,
            provider_name: "anthropic",
            model_name: "claude-sonnet-4-6",
            model_supports_tools: true,
            tool_format_known: true,
            tools_in_request: true,
            evidence_gate_fires: false,
            evidence_already_handled: false,
            is_synthesis_round: false,
        }
    }

    #[test]
    fn healthy_context_passes() {
        let ctx = ctx_default();
        let verdict = evaluate(&ctx);
        assert!(matches!(verdict, FrontierVerdict::Proceed { warnings } if warnings.is_empty()));
    }

    #[test]
    fn token_budget_exceeded_rejects() {
        let ctx = FrontierContext {
            tokens_used: 100_001,
            ..ctx_default()
        };
        let verdict = evaluate(&ctx);
        assert!(matches!(
            verdict,
            FrontierVerdict::Reject {
                gate: FrontierGate::TokenBudget,
                ..
            }
        ));
    }

    #[test]
    fn duration_budget_exceeded_rejects() {
        let ctx = FrontierContext {
            elapsed_secs: 301,
            ..ctx_default()
        };
        let verdict = evaluate(&ctx);
        assert!(matches!(
            verdict,
            FrontierVerdict::Reject {
                gate: FrontierGate::DurationBudget,
                ..
            }
        ));
    }

    #[test]
    fn cost_budget_exceeded_rejects() {
        let ctx = FrontierContext {
            cost_usd: 1.5,
            ..ctx_default()
        };
        let verdict = evaluate(&ctx);
        assert!(matches!(
            verdict,
            FrontierVerdict::Reject {
                gate: FrontierGate::CostBudget,
                ..
            }
        ));
    }

    #[test]
    fn headroom_critical_rejects() {
        let ctx = FrontierContext {
            tokens_used: 96_000,
            max_total_tokens: 100_000,
            output_headroom_tokens: 5000,
            round: 3,
            ..ctx_default()
        };
        let verdict = evaluate(&ctx);
        assert!(matches!(
            verdict,
            FrontierVerdict::Reject {
                gate: FrontierGate::OutputHeadroom,
                ..
            }
        ));
    }

    #[test]
    fn headroom_skipped_on_round_zero() {
        let ctx = FrontierContext {
            tokens_used: 96_000,
            max_total_tokens: 100_000,
            output_headroom_tokens: 5000,
            round: 0, // round 0 — skip headroom check
            ..ctx_default()
        };
        let verdict = evaluate(&ctx);
        assert!(matches!(verdict, FrontierVerdict::Proceed { .. }));
    }

    #[test]
    fn model_without_tools_warns() {
        let ctx = FrontierContext {
            model_supports_tools: false,
            ..ctx_default()
        };
        let verdict = evaluate(&ctx);
        match verdict {
            FrontierVerdict::Proceed { warnings } => {
                assert_eq!(warnings.len(), 1);
                assert_eq!(warnings[0].gate, FrontierGate::ModelCapability);
            }
            _ => panic!("Expected Proceed with warnings"),
        }
    }

    #[test]
    fn evidence_gate_rejects_synthesis() {
        let ctx = FrontierContext {
            is_synthesis_round: true,
            evidence_gate_fires: true,
            evidence_already_handled: false,
            ..ctx_default()
        };
        let verdict = evaluate(&ctx);
        assert!(matches!(
            verdict,
            FrontierVerdict::Reject {
                gate: FrontierGate::EvidenceGate,
                ..
            }
        ));
    }

    #[test]
    fn evidence_gate_skipped_when_handled() {
        let ctx = FrontierContext {
            is_synthesis_round: true,
            evidence_gate_fires: true,
            evidence_already_handled: true,
            ..ctx_default()
        };
        let verdict = evaluate(&ctx);
        assert!(matches!(verdict, FrontierVerdict::Proceed { .. }));
    }

    #[test]
    fn token_budget_checked_before_duration() {
        let ctx = FrontierContext {
            tokens_used: 100_001,
            elapsed_secs: 301,
            ..ctx_default()
        };
        let verdict = evaluate(&ctx);
        // Token budget should be checked first
        assert!(matches!(
            verdict,
            FrontierVerdict::Reject {
                gate: FrontierGate::TokenBudget,
                ..
            }
        ));
    }

    #[test]
    fn no_budgets_configured_passes() {
        let ctx = FrontierContext {
            max_total_tokens: 0,
            max_duration_secs: 0,
            max_cost_usd: 0.0,
            ..ctx_default()
        };
        let verdict = evaluate(&ctx);
        assert!(matches!(verdict, FrontierVerdict::Proceed { .. }));
    }
}
