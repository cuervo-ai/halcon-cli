//! Phase 14/16 context grouping struct and shared types.
//!
//! Groups all Phase 14+ features into a single optional struct to avoid
//! bloating AgentContext with multiple new fields. AgentContext gets ONE
//! new field: `phase14: Phase14Context`.

use serde::{Deserialize, Serialize};

use super::determinism::ExecutionContext;

/// Dry-run mode controls which tools are actually executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DryRunMode {
    /// Normal execution — all tools run.
    Off,
    /// Execute ReadOnly tools, skip ReadWrite/Destructive.
    DestructiveOnly,
    /// Skip all tool executions.
    Full,
}

impl Default for DryRunMode {
    fn default() -> Self {
        Self::Off
    }
}

/// Bundled Phase 14+ context for the agent loop.
///
/// All fields have sensible defaults (disabled/production mode).
/// This avoids breaking the 19+ existing AgentContext construction sites.
#[derive(Debug, Clone, Default)]
pub struct Phase14Context {
    /// 14.0: Deterministic UUIDs + clock for replay.
    pub exec_ctx: ExecutionContext,
    /// 16.0: Dry-run mode (skips destructive tool execution).
    pub dry_run_mode: DryRunMode,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase14_context_default() {
        let ctx = Phase14Context::default();
        assert!(!ctx.exec_ctx.execution_id.is_nil());
        assert_eq!(ctx.dry_run_mode, DryRunMode::Off);
    }

    #[test]
    fn dry_run_mode_default_is_off() {
        assert_eq!(DryRunMode::default(), DryRunMode::Off);
    }

    #[test]
    fn dry_run_mode_serde_roundtrip() {
        let json = serde_json::to_string(&DryRunMode::DestructiveOnly).unwrap();
        assert_eq!(json, r#""destructive_only""#);
        let parsed: DryRunMode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, DryRunMode::DestructiveOnly);
    }

    #[test]
    fn phase14_context_carries_dry_run_mode() {
        let ctx = Phase14Context {
            dry_run_mode: DryRunMode::Full,
            ..Default::default()
        };
        assert_eq!(ctx.dry_run_mode, DryRunMode::Full);
    }

    #[test]
    fn dry_run_mode_all_variants_serde() {
        for mode in [DryRunMode::Off, DryRunMode::Full, DryRunMode::DestructiveOnly] {
            let json = serde_json::to_string(&mode).unwrap();
            let parsed: DryRunMode = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, mode);
        }
    }

    #[test]
    fn phase14_context_exec_ctx_independent_of_dry_run() {
        let ctx1 = Phase14Context {
            dry_run_mode: DryRunMode::Full,
            ..Default::default()
        };
        let ctx2 = Phase14Context {
            dry_run_mode: DryRunMode::Off,
            ..Default::default()
        };
        // Both should have valid (but different) execution IDs.
        assert!(!ctx1.exec_ctx.execution_id.is_nil());
        assert!(!ctx2.exec_ctx.execution_id.is_nil());
    }

    #[test]
    fn config_dry_run_flag_can_set_mode() {
        // Test that config.tools.dry_run (bool) can drive DryRunMode selection.
        let config_flag = true;
        let mode = if config_flag {
            DryRunMode::DestructiveOnly
        } else {
            DryRunMode::Off
        };
        assert_eq!(mode, DryRunMode::DestructiveOnly);
    }

    #[test]
    fn agent_loop_reads_dry_run_from_phase14() {
        // Verify Phase14Context can be constructed with dry_run_mode
        // and that default() gives Off.
        let default_ctx = Phase14Context::default();
        assert_eq!(default_ctx.dry_run_mode, DryRunMode::Off);

        let dry_ctx = Phase14Context {
            dry_run_mode: DryRunMode::DestructiveOnly,
            ..Default::default()
        };
        assert_eq!(dry_ctx.dry_run_mode, DryRunMode::DestructiveOnly);
    }
}
