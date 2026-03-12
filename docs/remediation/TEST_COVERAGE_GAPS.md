# Test Coverage Gap Analysis — Phase 1

> Generated: 2026-03-12

---

## Summary

| Component | Coverage Status | Risk |
|-----------|-----------------|------|
| Agent loop core (`mod.rs`) | ❌ NO DIRECT TESTS | CRITICAL |
| Provider round execution | ❌ NO DIRECT TESTS | HIGH |
| Convergence phase | ❌ NO DIRECT TESTS | HIGH |
| Post-batch processing | ❌ NO DIRECT TESTS | HIGH |
| Round setup | ❌ NO DIRECT TESTS | MEDIUM |
| GDEM runtime integration | ⚠️ UNTESTED (disconnected) | CRITICAL |
| Security layer | ✅ 346 tests | LOW |
| UCB1 strategy learner | ✅ 9+ tests | LOW |
| Budget tracker | ✅ 12 tests | LOW |
| FSM state machine | ✅ 8+16 tests | LOW |
| HybridIntentClassifier | ✅ 58 tests | LOW |

---

## CRITICAL Coverage Gaps

### GAP-1: `repl/agent/mod.rs` (2,670 lines, 0 direct tests)

**Severity**: CRITICAL — This is the production agent loop entry point.

**What it contains**:
- `run_agent_loop()` — main agent execution function
- `AgentContext` struct construction
- Features 1–10 initialization blocks
- GDEM runtime integration hooks (currently no-ops)
- Per-round orchestration logic

**Why no direct tests**: Tests are in the separate `tests.rs` file (421 tests) which
imports `use super::*`. Many functions in `mod.rs` are tested indirectly through
`run_agent_loop()`. However, the file has 2,670 lines of complex orchestration with
large untested branches.

**Coverage estimate**: ~35% (main happy paths only)

**Action for Phase 2**: Add targeted unit tests for:
- `build_agent_context()` initialization paths
- Feature flag gates (Features 1–10)
- Early-exit conditions (budget exceeded, ctrl-C, supervisor denied)
- GDEM integration adapter (when implemented)

---

### GAP-2: `repl/agent/convergence_phase.rs` (2,194 lines, 0 tests)

**Severity**: CRITICAL — Convergence detection determines when agent stops.

**What it contains**:
- Evidence accumulation and verification
- Convergence scoring
- ForcedSynthesis trigger conditions
- Multi-round convergence state tracking

**Coverage estimate**: ~0% (no tests at any level)

**Action for Phase 2**: Critical module — add unit tests for convergence scoring
before implementing GDEM integration.

---

### GAP-3: `repl/agent/provider_round.rs` (1,552 lines, 0 tests)

**Severity**: HIGH — One model invocation per agent round.

**What it contains**:
- Stream processing from LLM providers
- Tool call parsing from streamed chunks
- Fallback provider logic invocation
- Token counting and cost tracking

**Coverage estimate**: ~20% (tested indirectly through integration tests)

**Action**: Add unit tests for stream parser, tool call extraction, cost tracking.

---

### GAP-4: `repl/agent/post_batch.rs` (1,535 lines, 0 tests)

**Severity**: HIGH — Post-tool-execution processing.

**What it contains**:
- Tool result aggregation
- Error classification and retry logic
- Budget consumption tracking
- Sub-agent result assembly

**Coverage estimate**: ~10% (minimal indirect testing)

---

### GAP-5: GDEM Runtime Integration (halcon-agent-core → halcon-cli)

**Severity**: CRITICAL — Phase 2 target.

**Current state**: `halcon-agent-core` has 281 tests covering the GDEM loop in isolation,
but the loop is NOT connected to production code. `loop_driver.rs` exists and is tested,
but `halcon-cli/src/repl/agent/mod.rs` does not call it.

**Missing tests**:
- `HalconToolExecutor` adapter (to be created in Phase 2)
- `HalconLlmClient` adapter (to be created in Phase 2)
- GDEM loop ↔ ToolRegistry integration
- GDEM loop ↔ provider invocation

**Action for Phase 2**: Create integration tests in `tests/gdem_integration.rs`
using `EchoProvider` and `ToolRegistry::new()`.

---

## HIGH Coverage Gaps

### GAP-6: `repl/slash_commands.rs` (1,634 lines, 0 tests)

**What it contains**: TUI slash command dispatch, autocomplete, history
**Coverage estimate**: 0%
**Risk**: Medium — UI-layer commands, tested manually

### GAP-7: `tui/app/ui_event_handler.rs` (1,178 lines, 0 tests)

**What it contains**: All TUI key event handling, modal routing
**Coverage estimate**: 0%
**Risk**: Medium — Complex event routing with no automated verification

### GAP-8: `repl/agent/round_setup.rs` (1,065 lines, 0 tests)

**What it contains**: Pre-round context assembly, system prompt construction, tool selection
**Coverage estimate**: ~5% (minimal indirect testing)
**Risk**: High — Bugs here cause wrong tool selection or missing context

---

## Well-Covered Components (LOW risk)

| Component | Tests | Coverage |
|-----------|-------|----------|
| `HybridIntentClassifier` | 58 | ~90% |
| `ExecutionBudget/BudgetTracker` | 12 | ~95% |
| `FormalAgentFSM` | 24 | ~90% |
| `UCB1StrategyLearner` | 9 | ~80% |
| `SecurityBlacklist` | 40+ | ~85% |
| `ResilienceManager` | 20+ | ~75% |
| `ContextCompactor` | 15+ | ~70% |
| `ToolRegistry` | 20+ | ~70% |
| `SQLite migrations` | 30+ | ~85% |
| `RBAC enforcement` | 21+ | ~80% |

---

## Coverage Improvement Plan

### Phase 1 (current): Fix broken tests — DONE ✅
- 11 tests fixed (ratatui OnceLock, WS URL, 8 doctests)
- 0 failing tests in workspace

### Phase 2: GDEM Integration Tests (next sprint)
- Create `tests/gdem_integration.rs`
- Add `HalconToolExecutor` adapter tests
- Add `HalconLlmClient` adapter tests
- Target: 50+ new integration tests

### Phase 3: Agent Core Coverage (deferred)
- Add tests for `convergence_phase.rs`
- Add tests for `provider_round.rs` stream parsing
- Add tests for `post_batch.rs` error classification
- Target: +150 tests

### Phase 4: TUI Coverage (deferred)
- Add property-based tests for event handling
- Add snapshot tests for widget rendering
- Target: +100 tests

---

## Coverage Metrics by Layer

| Layer | Lines (est.) | Test Functions | Est. Coverage |
|-------|-------------|----------------|---------------|
| GDEM core (`halcon-agent-core`) | ~4,500 | 281 | ~75% |
| Agent loop (`halcon-cli/repl/agent`) | ~12,000 | 421 | ~40% |
| Security (`repl/security`) | ~3,500 | 346 | ~80% |
| Planning (`repl/planning`) | ~4,000 | 300 | ~65% |
| Domain/Classifier | ~5,000 | 733 | ~85% |
| Tool system (`halcon-tools`) | ~8,000 | 970 | ~70% |
| Storage (`halcon-storage`) | ~3,000 | 254 | ~75% |
| Providers (`halcon-providers`) | ~4,000 | 277 | ~65% |
| TUI (`tui/`) | ~8,000 | ~500 | ~30% |
| MCP (`halcon-mcp`) | ~2,500 | 106 | ~55% |
