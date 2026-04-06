//! Agent loop prologue — pre-loop setup extracted from `run_agent_loop()`.
//!
//! # Architecture
//!
//! The prologue runs BEFORE the main `for round in 0..effective_max_rounds` loop.
//! It handles:
//! - Intent scoring and task analysis (SOTA 2026)
//! - Adaptive planning policy evaluation
//! - Plan validation, compression, and ingestion
//! - TBAC context push
//! - Execution tracking setup
//! - Convergence detector calibration
//! - Model capability validation
//! - Intent-based tool selection
//! - Input normalization and boundary decision
//! - SLA budget derivation
//! - Intent pipeline reconciliation
//! - System prompt assembly from multiple sources
//!
//! # Xiyo Comparison
//!
//! Xiyo's pre-loop setup is minimal: `fetchSystemPromptParts()` + `initializeHooks()`
//! (query.ts:275-305). Halcon's prologue provides:
//! - **Intent scoring**: Multi-signal task analysis (scope, depth, language, latency)
//! - **Adaptive planning**: Composable policy chain (ToolAware → ReasoningModel → IntentDriven)
//! - **Convergence calibration**: Provider-aware headroom (8% of pipeline_budget)
//! - **TBAC scoping**: Task context restricts tool usage to planned tools only
//!
//! # Migration Status
//!
//! Extraction uses the **strangler fig pattern**:
//! - `setup.rs`: `build_context_pipeline()` — EXTRACTED
//! - `prologue.rs` (this file): Additional setup helpers — IN PROGRESS
//! - `mod.rs`: Still contains inline prologue code — TO BE MIGRATED
//!
//! Functions are extracted incrementally. Each is added here, called from
//! `run_agent_loop()`, and the inline code removed only after tests pass.

use std::sync::Arc;

use halcon_core::traits::ModelProvider;
use halcon_core::types::ModelRequest;

/// Check whether the provider's model supports tool use.
///
/// Returns `true` if tools should be forcibly disabled for this model.
/// Models that can't be validated against the provider are assumed to
/// NOT support tools, preventing API errors.
///
/// # Xiyo Comparison
///
/// Xiyo doesn't validate tool support — it relies on the API to return errors.
/// Halcon validates proactively to avoid burning a round on an impossible request.
pub(super) fn should_force_no_tools(
    provider: &Arc<dyn ModelProvider>,
    request: &ModelRequest,
) -> bool {
    // If the provider doesn't recognise this model at all, it may not support tools.
    // We check supported_models() — if the model isn't listed AND validate_model fails,
    // we assume no tool support.
    let known = provider
        .supported_models()
        .iter()
        .any(|m| m.id == request.model);
    if known {
        // Known model — check if it has tool-use support via ModelInfo.
        // Models with supports_tools=false should not receive tool definitions.
        provider
            .supported_models()
            .iter()
            .find(|m| m.id == request.model)
            .map(|m| !m.supports_tools)
            .unwrap_or(false)
    } else {
        // Unknown model — allow tools (permissive default for custom deployments)
        false
    }
}

/// Resolve the compaction model for the ContextCompactor.
///
/// The request.model may belong to a different provider (e.g., "claude-sonnet"
/// with DeepSeek active provider). We must validate against the active provider
/// and fall back to the provider's first available model.
///
/// # Returns
///
/// The model name to use for compaction, guaranteed to be valid on the active provider.
pub(super) fn resolve_compaction_model(
    provider: &Arc<dyn ModelProvider>,
    request_model: &str,
) -> String {
    if provider.validate_model(request_model).is_ok() {
        return request_model.to_string();
    }

    // Fallback: use the provider's first available model.
    let models = provider.supported_models();
    if let Some(first) = models.first() {
        tracing::warn!(
            request_model,
            fallback = %first.id,
            provider = provider.name(),
            "Compaction model mismatch — falling back to provider's first model"
        );
        first.id.clone()
    } else {
        tracing::warn!(
            request_model,
            provider = provider.name(),
            "Provider has no models — using request model as-is"
        );
        request_model.to_string()
    }
}

/// Derive the execution intent phase from plan tool requirements.
///
/// - **Execution**: Plan has ≥2 steps requiring execution tools (bash, file_write, etc.)
/// - **Investigation**: Plan has ≥1 step requiring any tool
/// - **Uncategorized**: No tool requirements detected
///
/// Execution tasks MUST keep tools active until all steps complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExecutionIntentPhase {
    Execution,
    Investigation,
    Uncategorized,
}

impl ExecutionIntentPhase {
    /// Classify intent from a set of tool names referenced in plan steps.
    pub fn from_tool_names(tool_names: &[&str]) -> Self {
        const EXECUTION_TOOLS: &[&str] = &[
            "bash",
            "file_write",
            "file_edit",
            "git_commit",
            "create_file",
            "run_command",
        ];

        let has_executable = tool_names.iter().any(|t| EXECUTION_TOOLS.contains(t));
        let tool_steps = tool_names.len();

        if has_executable && tool_steps >= 2 {
            Self::Execution
        } else if tool_steps >= 1 {
            Self::Investigation
        } else {
            Self::Uncategorized
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_intent_with_bash_and_file_write() {
        let tools = vec!["bash", "file_write", "grep"];
        assert_eq!(
            ExecutionIntentPhase::from_tool_names(&tools),
            ExecutionIntentPhase::Execution
        );
    }

    #[test]
    fn investigation_intent_with_read_tools() {
        let tools = vec!["file_read"];
        assert_eq!(
            ExecutionIntentPhase::from_tool_names(&tools),
            ExecutionIntentPhase::Investigation
        );
    }

    #[test]
    fn uncategorized_with_no_tools() {
        let tools: Vec<&str> = vec![];
        assert_eq!(
            ExecutionIntentPhase::from_tool_names(&tools),
            ExecutionIntentPhase::Uncategorized
        );
    }

    #[test]
    fn execution_requires_two_steps() {
        // Only 1 executable tool — Investigation, not Execution
        let tools = vec!["bash"];
        assert_eq!(
            ExecutionIntentPhase::from_tool_names(&tools),
            ExecutionIntentPhase::Investigation
        );
    }
}
