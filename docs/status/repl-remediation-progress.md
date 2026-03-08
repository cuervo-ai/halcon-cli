# Halcon repl/ Remediation Progress Report

**Date**: 2026-03-08
**Branch**: feature/sota-intent-architecture
**Executor**: Claude Sonnet 4.6

---

## Summary

FASE 2 through FASE 4 of the repl/ remediation plan have been executed. Below is a complete accounting of all actions taken.

---

## FASE 1 (Prior Work) — Files Deleted

8 files deleted in the previous session:
- 7 orphan files not declared in mod.rs: `ambiguity_detector.rs`, `clarification_gate.rs`, `goal_hierarchy.rs`, `input_risk_classifier.rs`, `intent_classifier.rs`, `pre_execution_critique.rs`, `tool_executor.rs`
- 1 rollback file: `rollback.rs`

Baseline after FASE 1: **143 flat repl/ files**

---

## FASE 2 — Async Correctness Bugs (ADDENDUM-1)

### Finding
After auditing all `std::sync::Mutex` usages in repl/, the plan's assumption that these were incorrectly used was found to be incorrect:

| File | Mutex usage | Finding |
|------|-------------|---------|
| `model_selector.rs` | 3 Mutex fields | All `.lock()` calls in sync methods — no await inside critical section |
| `idempotency.rs` | 1 Mutex field | Only sync methods — correct |
| `response_cache.rs` | 1 Mutex field | Lock released in scoped `{}` block before every `.await` — correct (COUPLING-001) |
| `schema_validator.rs` | 1 static LazyLock<Mutex> | Sync-only; process-global static — correct choice |
| `permission_lifecycle.rs` | 2 Mutex fields | Already fixed (COUPLING-001 comment) — lock released before await |

**Decision**: FASE 2-A and 2-B migrations SKIPPED. Per Tokio documentation, `std::sync::Mutex` is acceptable when no `.await` is held inside the critical section. All existing usage is compliant. Migration to `tokio::sync::Mutex` would require cascading async API changes with no safety benefit.

**ADDENDUM-1** documented in `docs/audit/repl-forensic-audit-2026.md`.

---

## FASE 3 — Wire Partial Subsystems

### STEP 3-A: ci_detection.rs (DONE)

**Finding**: The plan stated `ci_detection.rs` had 0 callsites. This was incorrect — `CIDetectionPolicy` was already wired in `authorization.rs:240`. However, the gap was real: CI detection did NOT set `non_interactive` mode (only handled tool auto-approval via the auth chain).

**Action**: Added `CiEnvironment` struct and public `detect()` function to `ci_detection.rs`. Wired into `Repl::new()` after permissions construction — if CI detected, calls `permissions.set_non_interactive()`.

**New tests**: 3 (detect_returns_is_ci_true_for_github_actions, detect_returns_is_ci_false_when_no_ci_vars, detect_returns_generic_ci_var)

**Commit**: `ae91bd4`

### STEP 3-B: EpisodicSource/MemorySource swap (SKIPPED)

**Finding**: Already correctly implemented in `mod.rs:432-451` via `if config.memory.episodic { EpisodicSource } else { MemorySource }`. Mutual exclusion is enforced by the if/else branch. No change needed.

---

## FASE 4 — Reorganize into Subdirectories

### STEP 4-A: plugins/ subdirectory (DONE)

**Files moved** (10 files):
| Old path | New path |
|----------|----------|
| `repl/plugin_registry.rs` | `repl/plugins/registry.rs` |
| `repl/plugin_loader.rs` | `repl/plugins/loader.rs` |
| `repl/plugin_manifest.rs` | `repl/plugins/manifest.rs` |
| `repl/plugin_transport_runtime.rs` | `repl/plugins/transport.rs` |
| `repl/plugin_proxy_tool.rs` | `repl/plugins/proxy_tool.rs` |
| `repl/plugin_permission_gate.rs` | `repl/plugins/permission_gate.rs` |
| `repl/plugin_circuit_breaker.rs` | `repl/plugins/circuit_breaker.rs` |
| `repl/plugin_cost_tracker.rs` | `repl/plugins/cost_tracker.rs` |
| `repl/plugin_recommendation.rs` | `repl/plugins/recommendation.rs` |
| `repl/plugin_auto_bootstrap.rs` | `repl/plugins/auto_bootstrap.rs` |

**Callers updated**: `agent/mod.rs`, `agent/post_batch.rs`, `agent/result_assembly.rs`, `agent/context.rs`, `executor.rs`, `agent_types.rs`, `reward_pipeline.rs`, `capability_index.rs`, `capability_resolver.rs`, `mod.rs`

**Commits**: `7e4ee5f` (moves), `044cbf2` (path fixes)

### STEP 4-B: security/ subdirectory (DONE)

**Files moved** (7 files):
| Old path | New path |
|----------|----------|
| `repl/command_blacklist.rs` | `repl/security/blacklist.rs` |
| `repl/output_risk_scorer.rs` | `repl/security/output_risk.rs` |
| `repl/risk_tier_classifier.rs` | `repl/security/risk_tier.rs` |
| `repl/permission_lifecycle.rs` | `repl/security/lifecycle.rs` |
| `repl/tool_policy.rs` | `repl/security/tool_policy.rs` |
| `repl/tool_trust.rs` | `repl/security/tool_trust.rs` |
| `repl/subagent_contract_validator.rs` | `repl/security/subagent_contract.rs` |

**Internal fixes**: `tool_trust.rs` — `super::retry_mutation` → `super::super::retry_mutation`. `tool_policy.rs` — `super::tool_aliases` → `super::super::tool_aliases`.

**Commit**: `09a55fd`

### STEP 4-C: servers/ subdirectory (DONE)

**Files moved** (8 files):
| Old path | New path |
|----------|----------|
| `repl/architecture_server.rs` | `repl/servers/architecture.rs` |
| `repl/codebase_server.rs` | `repl/servers/codebase.rs` |
| `repl/requirements_server.rs` | `repl/servers/requirements.rs` |
| `repl/workflow_server.rs` | `repl/servers/workflow.rs` |
| `repl/test_results_server.rs` | `repl/servers/test_results.rs` |
| `repl/runtime_metrics_server.rs` | `repl/servers/runtime_metrics.rs` |
| `repl/security_server.rs` | `repl/servers/security.rs` |
| `repl/support_server.rs` | `repl/servers/support.rs` |

**Commit**: `43a0edf`

---

## Final Metrics

| Metric | Before (FASE 1 end) | After (FASE 4 end) | Change |
|--------|--------------------|--------------------|--------|
| Flat repl/ files | 143 | 118 | -25 |
| Subdirectories | 8 | 11 | +3 |
| Test count | 4316 | 4319 | +3 |
| Build errors | 0 | 0 | 0 |

**New subdirectories added**:
- `repl/plugins/` — 10 files (plugin subsystem)
- `repl/security/` — 7 files (security/risk/trust subsystem)
- `repl/servers/` — 8 files (context server subsystem)

**Pre-existing subdirectories** (unchanged):
- `repl/agent/`, `repl/agent_registry/`, `repl/application/`, `repl/auto_memory/`,
  `repl/decision_engine/`, `repl/domain/`, `repl/hooks/`, `repl/instruction_store/`

---

## Backward Compatibility Strategy

All reorganizations used `pub use subdir::module as old_name;` re-exports in `repl/mod.rs` to preserve existing import paths. Callers that used `super::plugin_registry::PluginRegistry` were updated directly; callers using `crate::repl::command_blacklist::analyze_command` continue to work via re-exports.

---

## ADDENDUM Findings

**ADDENDUM-1**: `std::sync::Mutex` audit (FASE 2) — all usages are correct per Tokio documentation. No migration needed. See `docs/audit/repl-forensic-audit-2026.md`.
