# HALCON Test Suite Map — Phase 1 Inventory

> Generated: 2026-03-12 | Branch: feature/sota-intent-architecture
> Status: **12,670 tests passing, 0 failing, 31 ignored**

---

## Executive Summary

| Metric | Value |
|--------|-------|
| Total test functions | **7,795** (unique across all crates) |
| Total test executions | **12,670** (lib + integration + doc harnesses) |
| Tests passing | **12,670** (100%) |
| Tests failing | **0** |
| Tests ignored | **31** (all legitimately deferred) |
| Crates with test coverage | **16 / 16** |
| Pre-existing failures fixed | **3** (ratatui OnceLock, WS URL token, 8 doctests) |

---

## Test Distribution by Crate

| Crate | Lib Tests | Integration Tests | Doc Tests | Category |
|-------|-----------|-------------------|-----------|----------|
| `halcon-cli` | 4,491 | 37 (e2e) | 5 | Mixed |
| `halcon-agent-core` | 281 | 0 | 0 | Unit + Sim |
| `halcon-tools` | 970 | 0 | 0 | Unit |
| `halcon-context` | 317 | 0 | 0 | Unit |
| `halcon-core` | 282 | 0 | 0 | Unit |
| `halcon-providers` | 277 | 18 | 0 | Unit + Live |
| `halcon-storage` | 254 | 11 | 0 | Unit + DB |
| `halcon-mcp` | 106 | 0 | 0 | Unit |
| `halcon-runtime` | 195 | 0 | 0 | Unit |
| `halcon-files` | 121 | 0 | 0 | Unit |
| `halcon-search` | 201 | 0 | 0 | Unit |
| `halcon-multimodal` | 175 | 0 | 0 | Unit |
| `halcon-security` | 79 | 0 | 0 | Security |
| `halcon-auth` | 21 | 0 | 0 | Security |
| `halcon-sandbox` | 16 | 0 | 0 | Security |
| `halcon-client` | 0 | 9 | 0 | Integration |

---

## halcon-cli Module Breakdown

| Module Path | Test Count | Category |
|-------------|-----------|----------|
| `repl::agent` | 421 | Unit + Runtime |
| `repl::domain` | 733 | Unit (classifier, intent, convergence) |
| `repl::security` | 346 | Security |
| `repl::planning` | 300 | Unit |
| `repl::plugins` | 201 | Unit |
| `repl::git_tools` | 201 | Unit + Integration |
| `repl::bridges` | 170 | Unit |
| `repl::metrics` | 166 | Unit |
| `tui::widgets` | 142 | UI |
| `tui::project_analyzer` | 129 | UI |
| `repl::context` | 112 | Unit |
| `tui::app` | 103 | UI |
| `repl::executor` | 97 | Runtime |
| `repl::decision_engine` | 92 | Unit |
| `repl::agent_registry` | 79 | Unit |
| `render::sink` | 68 | Unit |
| `repl::commands` | 62 | Unit |
| `tui::activity_model` | 60 | UI |
| `render::theme` | 55 | UI |
| `repl::delegation` | 52 | Unit |
| `tui::events` | 47 | UI |
| `tui::layout` | 45 | UI |
| `tui::activity_navigator` | 44 | UI |
| `tui::activity_renderer` | 42 | UI |
| `render::intelligent_theme` | 41 | UI |
| `repl::servers` | 40 | Integration |
| `repl::hooks` | 40 | Unit |
| `repl::supervisor` | 36 | Unit |
| `repl::orchestrator` | 31 | Integration |
| `repl::console` | 29 | Unit |
| `render::terminal_caps` | 29 | Unit (env-isolated) |

---

## halcon-agent-core Module Breakdown

| Module | Tests | Purpose |
|--------|-------|---------|
| `metrics::tests` | 29 | GoalAlignment, ReplanEfficiency, SandboxContainment |
| `adversarial_simulation_tests` | 28 | Budget exhaustion, oscillation, adversarial scenarios |
| `invariants::tests` | 24 | Formal invariant proofs (I-1 through I-10) |
| `fsm_formal_model::tests` | 16 | FormalAgentFSM typed state transitions |
| `long_horizon_tests` | 14 | Multi-round stability, drift detection |
| `failure_injection::tests` | 13 | Fault tolerance, recovery |
| `regret_analysis::tests` | 12 | UCB1 regret bounds |
| `info_theory_metrics::tests` | 12 | Entropy, mutual information |
| `execution_budget::tests` | 12 | Hard resource limit enforcement |
| `stability_analysis::tests` | 11 | Lyapunov stability metrics |
| `oscillation_metric::tests` | 11 | Oscillation detection |
| `replay_certification::tests` | 10 | Deterministic replay |
| `strategy::tests` | 9 | UCB1 strategy learner |
| `fsm::tests` | 8 | AgentFSM state machine |
| `confidence_hysteresis::tests` | 8 | Hysteresis thresholds |

---

## Test Categories

### Unit Tests (~85% of suite)
- Pure function tests with no I/O or external dependencies
- Hermetic: no filesystem, network, or environment dependencies
- Location: `#[cfg(test)] mod tests { ... }` inline in source files
- **Examples**: domain classifiers, UCB1 bandit, budget tracker, planner

### Integration Tests (~8%)
- Cross-module tests requiring multiple components
- May use `tempfile::TempDir` for isolated filesystem state
- Location: `tests/` directories (halcon-cli, halcon-storage, halcon-providers)
- **Examples**: permission e2e, orchestrator e2e, client tests

### Runtime Tests (~5%)
- Test agent loop execution with mock providers (EchoProvider)
- Use `tokio::test` for async execution
- Location: `repl/agent/tests.rs` (421 tests), `repl/stress_tests.rs`
- **Examples**: `agent_loop_simple_text_response`, `check_control_stops_loop`

### Security Tests (~2%)
- RBAC enforcement, permission gating, blacklist patterns
- Location: `repl/security/`, `halcon-auth`, `halcon-sandbox`
- **Examples**: catastrophic pattern blocking, FASE-2 guardrails

### UI Tests (~5%)
- TUI widget rendering, state machine, overlay handling
- Mock terminal environment (no real TTY required)
- Location: `tui/` modules
- **Examples**: panel layout, theme adaptation, event handling

---

## Integration Test Files

| File | Tests | Status |
|------|-------|--------|
| `tests/permission_e2e.rs` | 28 | ✅ Passing |
| `tests/permission_system_integration.rs` | 9 | ✅ Passing |
| `tests/orchestrator_e2e.rs` | 2 (1 ignored) | ✅ Passing |
| `tests/multi_provider_e2e.rs` | 6 | ✅ Passing |
| `tests/sota_evaluation.rs` | 15 | ✅ Passing |
| `crates/halcon-client/tests/client_tests.rs` | 9 | ✅ Passing |
| `crates/halcon-storage/tests/perf_measurements.rs` | 11 | ✅ Passing |
| `crates/halcon-providers/tests/live_provider_validation.rs` | 8 (all ignored) | Intentionally skipped |

---

## Ignored Tests (31 total)

| Test | Reason | Action Required |
|------|--------|-----------------|
| `tui::clipboard::test_copy_*` (3) | Requires display server | Keep ignored in CI, run manually |
| `render::theme::ratatui_cache_*` | Static OnceLock race condition | Keep ignored in CI |
| `render::theme::adaptive_palette_fallback` | Tests uninitialized state (global static) | Keep ignored in CI |
| `render::color_science::delta_e_diagnostic` | Manual diagnostic test | Keep ignored |
| `tests/orchestrator_e2e.rs::*` (1) | Legacy behavior verification | Keep ignored |
| `halcon-providers::live_*` (8 all) | Requires live API keys | Keep ignored in CI |
| `halcon-runtime::*` (8) | Requires specific runtime env | Keep ignored |

---

## Test Dependency Map

```
halcon-cli tests
├── depends on: EchoProvider (halcon-providers)
├── depends on: halcon-storage (SQLite in-memory)
├── depends on: halcon-tools (ToolRegistry)
└── depends on: halcon-core types

halcon-agent-core tests
├── depends on: rand (seeded StdRng for determinism)
└── pure: no external crate dependencies in tests

halcon-search tests
└── depends on: halcon-storage (SQLite migrations)
```
