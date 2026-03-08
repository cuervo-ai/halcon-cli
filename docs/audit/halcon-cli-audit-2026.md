# Halcon CLI Architecture Audit 2026

**Auditor**: Claude Sonnet 4.6 (principal systems architect mode)
**Date**: 2026-03-08
**Branch**: feature/sota-intent-architecture
**Scope**: `crates/halcon-cli` and direct dependencies

---

## Executive Summary

The `halcon-cli` crate is a 174,068-line monolith masquerading as a modular system. The core agent loop (`repl/agent/mod.rs`, 2,472 lines) coordinates at least 9 subsystems via `LoopState` — a god object with 62+ public fields across 6 nested sub-structs. The crate contains 151 source files in `repl/` alone, with a median of 700 lines per file.

The fundamental engineering risk is an **integration gap between implemented subsystems and the live agent loop**: `SignalArbitrator` is fully implemented (14 tests), marked `#[deprecated]`, and never called. `StrategySelector` (UCB1) is implemented but only exercised via an indirect `ReasoningEngine` path that may or may not be invoked. `reward_pipeline` is wired only in the REPL-level `mod.rs` (4,266 lines), not in the agent loop itself. The SynthesisGate's `GovernanceRescue` trigger enforcement was added as a patch in Phase 2 remediation but the `SignalArbitrator` that was supposed to arbitrate between governance signals was never wired.

**The single most important thing to fix first**: The `LoopState` god object (477 fields across 6 sub-structs, `loop_state.rs:477`) is the root cause of every other architectural problem. Every phase function takes `&mut LoopState`, meaning the entire agent loop state is mutable from 8 different files simultaneously. This makes it impossible to reason about invariants, creates test brittleness, and blocks extraction of any subsystem into its own crate. Decomposing `LoopState` into `AgentSession` (immutable config), `RoundState` (per-round), and `ConvergenceState` (owned by convergence_phase) is the prerequisite for every other migration step.

---

## Phase 0: Raw Metrics

### 0.1 Workspace Package Graph

```
halcon-cli -> 19 workspace crates + 64 external deps
  halcon-agent-core, halcon-api, halcon-auth, halcon-context, halcon-core,
  halcon-mcp, halcon-multimodal, halcon-providers, halcon-runtime, halcon-search,
  halcon-security, halcon-storage, halcon-tools, momoto-core, momoto-intelligence,
  momoto-metrics (+ rhai, ratatui, reedline, ...)

halcon-core -> [async-trait, chrono, futures, glob, hex, serde, serde_json, sha2,
               thiserror, tokio, tracing, uuid]  -- leaf node, correct

halcon-providers -> [halcon-core, halcon-storage, reqwest, ...]
halcon-tools -> [halcon-context, halcon-core, halcon-files, halcon-search, halcon-storage, ...]
```

Notable: `halcon-cli` depends on EVERY internal crate. It is the only binary and acts as a composition root. This is architecturally sound for a monorepo CLI, but the internal `repl/` module structure is not modular — it is a single namespace with 151 files.

### 0.2 Size and Complexity

**Top 15 files by line count:**

| File | Lines |
|------|-------|
| `repl/agent/tests.rs` | 5,484 |
| `repl/mod.rs` | 4,266 |
| `repl/executor.rs` | 3,334 |
| `repl/agent/mod.rs` | 2,472 |
| `render/theme.rs` | 2,264 |
| `render/sink.rs` | 2,207 |
| `render/intelligent_theme.rs` | 2,166 |
| `repl/agent/convergence_phase.rs` | 1,889 |
| `repl/orchestrator.rs` | 1,861 |
| `repl/model_selector.rs` | 1,776 |
| `repl/agent/provider_round.rs` | 1,470 |
| `repl/slash_commands.rs` | 1,443 |
| `repl/agent/post_batch.rs` | 1,334 |
| `repl/agent/loop_state.rs` | 1,333 |
| `repl/execution_tracker.rs` | 1,280 |

**Total**: 174,068 lines across ~260 `.rs` files.

**Test count**: 4,656 `#[test]` / `#[tokio::test]` annotations.

### 0.3 Module Structure

- `repl/` contains **151 `.rs` files** (not counting subdirectories)
- `repl/domain/` contains **30 domain modules** (pure business logic)
- `repl/decision_engine/` contains **11 modules** (BDE pipeline)
- `repl/agent/` contains **17 modules** (agent loop decomposed)
- `repl/hooks/`, `repl/auto_memory/`, `repl/instruction_store/`, `repl/agent_registry/` are feature modules (Frontier Roadmap 2026)

### 0.4 Test Coverage Gaps

Files with no test annotations (production files only, sample):
- `repl/anomaly_detector.rs`
- `repl/arima_predictor.rs`
- `repl/backpressure.rs`
- `repl/capability_resolver.rs`
- `repl/ci_detection.rs`
- `repl/context_governance.rs`
- Most server files (`architecture_server.rs`, `codebase_server.rs`, etc.)

### 0.5 Config Struct Proliferation

Found **60+ Config structs** across the workspace. In `halcon-core/src/types/config.rs` alone:
`McpServeConfig`, `AppConfig`, `PluginsConfig`, `ReasoningConfig`, `SearchConfig`,
`GeneralConfig`, `ModelsConfig`, `ProviderConfig`, `OAuthConfig`, `HttpConfig`,
`ToolsConfig`, `ToolRetryConfig`, `SandboxConfig`, `SecurityConfig`, `GuardrailsConfig`,
`StorageConfig`, `LoggingConfig`, `McpConfig`, `McpServerConfig`, `AgentConfig`,
`ModelSelectionConfig`, `CompactionConfig`, `RoutingConfig`, `CacheConfig`,
`PlanningConfig`, `MemoryConfig`, `ResilienceConfig`, `CircuitBreakerConfig`, ...

`PolicyConfig` in `halcon-core/src/types/policy_config.rs` has 50+ fields and governs thresholds for reward pipeline, critic, evidence, tool trust, retry, convergence, SLA, and intent pipeline — a second god object at the config layer.

---

## Phase 1: Findings

### ARCH-001: LoopState God Object (CRITICAL)

**Severity**: Critical
**Location**: `crates/halcon-cli/src/repl/agent/loop_state.rs:477-579`
**Evidence**: `LoopState` has 6 nested sub-structs (`TokenAccounting`, `EvidenceState`, `SynthesisControl`, `ConvergenceState`, `HiconSubsystems`, `LoopGuardState`) with 62 total public fields. `ConvergenceState` alone contains 14 sub-objects. The struct is passed as `&mut LoopState` to 8 different phase functions.
**Impact**: No phase function can be tested in isolation. Any change to `LoopState` requires touching all 8 phase files. Impossible to extract any subsystem without breaking the full parameter surface.
**Effort**: High (2 weeks)

### ARCH-002: run_agent_loop Monolith (CRITICAL)

**Severity**: Critical
**Location**: `crates/halcon-cli/src/repl/agent/mod.rs:215-end (2472 lines)`
**Evidence**: The `run_agent_loop()` function coordinates: PII check, context pipeline, planning, tool selection, instruction injection, memory injection, agent registry, lifecycle hooks, BDE pipeline, intent pipeline, convergence setup, the `'agent_loop` loop body (provider_round → post_batch → convergence_phase), session retrospective, auto-memory, stop hook. That is at minimum 15 distinct concerns in a single function body of ~2,200 lines.
**Impact**: Cyclomatic complexity renders the function untestable as a unit. The `dispatch!` macro hides control flow.
**Effort**: High (3 weeks to extract setup functions)

### ARCH-003: repl/mod.rs Second Monolith (HIGH)

**Severity**: High
**Location**: `crates/halcon-cli/src/repl/mod.rs:1-4266`
**Evidence**: 4,266 lines. Contains: module declarations (lines 1-255), the `Repl` struct (lines 279-400), the REPL run loop including strategy_selector and reward_pipeline wiring (lines 2700-3242). The reward pipeline (`reward_pipeline::compute_reward`) is called at line 2919 in `mod.rs`, not in the agent loop — this means the UCB1 learning signal is wired at the REPL level, not inside the agent loop where the data originates.
**Impact**: REPL-level wiring means any test of reward_pipeline in agent context must instantiate the full Repl struct.
**Effort**: Medium (1 week)

### DEAD-001: SignalArbitrator Never Called (HIGH)

**Severity**: High
**Location**: `crates/halcon-cli/src/repl/domain/signal_arbitrator.rs:112`
**Evidence**:
```
#[deprecated(
    since = "0.3.0",
    note = "... SignalArbitrator is orphaned — not called from the agent loop."
)]
pub struct SignalArbitrator;
```
The module has 14 tests (`signal_arbitrator.rs:275-399`) and a full implementation including `SignalBundle`, `ArbitrationResult`, `ConflictType`, `ResolvedAction`. It is never imported outside of its own test module. `convergence_phase.rs` calls `TerminationOracle::adjudicate()` directly without going through `SignalArbitrator`.
**Impact**: Conflict patterns listed in the module doc (Replan vs Synthesis, EBS vs Everything) are not being enforced by the arbitrator. Whether `TerminationOracle` replicates this logic is unclear.
**Effort**: Low to remove, Medium to wire

### DEAD-002: PluginRegistry Never Called from Agent Loop (HIGH)

**Severity**: High
**Location**: `crates/halcon-cli/src/repl/plugin_registry.rs:90`, `repl/agent/mod.rs:142`
**Evidence**: `AgentContext::plugin_registry` is `Option<Arc<Mutex<PluginRegistry>>>` — gated by `if let Some(pr)`. Examination of `agent/mod.rs` shows plugin_registry is passed through to `post_batch` and `result_assembly` but the supervisor comment (`supervisor.rs:137`) says "The agent loop should call `plugin_registry.suspend_plugin(plugin_id, reason)`" — indicating the primary integration point is documented but not implemented.
**Impact**: Plugin gating, cost tracking, and circuit breaker features exist only as isolated modules with self-tests.
**Effort**: Medium

### DEAD-003: reward_pipeline Not in Agent Loop (HIGH)

**Severity**: High
**Location**: `crates/halcon-cli/src/repl/mod.rs:2898-2921`
**Evidence**: `reward_pipeline::compute_reward()` is called in `repl/mod.rs` at the REPL level (post-session), not inside `run_agent_loop()`. The UCB1 signal therefore feeds the `ReasoningEngine` for the _next_ session, not for round-by-round adaptation within the current session.
**Impact**: The "SOTA intent architecture" claim that UCB1 adapts within a session is incorrect — adaptation is inter-session only.
**Effort**: Medium (wire reward_pipeline into convergence_phase)

### DUP-001: Duplicate Intent Analysis Paths (MEDIUM)

**Severity**: Medium
**Location**: `repl/agent/mod.rs:434`, `repl/mod.rs:2718`, `repl/domain/intent_scorer.rs`
**Evidence**: `IntentScorer::score()` is called in `run_agent_loop()` at line 434. `strategy_selector::ReasoningStrategy::from_str()` is called at `mod.rs:2718` via `ReasoningEngine`. `BoundaryDecisionEngine::evaluate()` is called at `agent/mod.rs:753`. `IntentPipeline::resolve()` runs at `agent/mod.rs:795`. That is 4 separate intent/routing analysis systems running for each request.
**Impact**: Latency overhead; conflicting outputs require complex reconciliation logic; IntentPipeline partially reconciles two of them, but all four run.
**Effort**: Medium (consolidate)

### DUP-002: Two decision_trace Modules (MEDIUM)

**Severity**: Medium
**Location**: `repl/domain/decision_trace.rs` and `repl/decision_engine/decision_trace.rs`
**Evidence**: Both files define `DecisionTraceCollector`/`DecisionTrace` types for different purposes. The domain one tracks per-round agent decisions; the decision_engine one tracks BDE pipeline routing decisions.
**Impact**: Name collision risk; callers must use qualified paths.
**Effort**: Low (rename one)

### INCOMPLETE-001: RoutingAdaptor T3/T4 Partially Wired (MEDIUM)

**Severity**: Medium
**Location**: `repl/agent/convergence_phase.rs:553-573`
**Evidence**: `RoutingAdaptor::check()` is called at convergence_phase.rs:561, gated by `if let Some(ref mut bd) = state.boundary_decision`. This means it only runs when `use_boundary_decision_engine=true` AND `use_intent_pipeline=true` AND `boundary_decision` is populated. The T3 trigger (evidence < 25% at round ≥ 4) and T4 (combined_score > 0.90 at round ≥ 3) do fire, but the escalation path (`conv_ctrl.set_max_rounds(current + delta)`) is wired into ConvergenceController, not persisted to `LoopState.sla_budget` — so SLA enforcement does not reflect the escalation.
**Impact**: Mid-session escalation works for max_rounds but not for budget enforcement.
**Effort**: Low

### INCOMPLETE-002: SynthesisGate GovernanceRescue Enforcement (MEDIUM)

**Severity**: Medium
**Location**: `repl/domain/synthesis_gate.rs` (inferred from convergence_phase.rs references)
**Evidence**: Per the project memory, `synthesis_gate::evaluate()` returns `allow: false` for `GovernanceRescue` when `reflection_score < 0.15 && rounds_executed < 3`. This enforcement was added as Phase 2B3. However, `SignalArbitrator` (which was supposed to formally arbitrate EBS vs synthesis signals) is orphaned. The gate fires correctly in isolation but its interaction with `TerminationOracle` decisions is unverified — `TerminationOracle::adjudicate()` runs at `convergence_phase.rs:545` without seeing the SynthesisGate verdict.
**Impact**: GovernanceRescue may be overridden by an oracle Synthesize decision.
**Effort**: Low

### INCOMPLETE-003: FeedbackCollector Never Aggregated (LOW)

**Severity**: Low
**Location**: `repl/decision_engine/decision_feedback.rs:32`
**Evidence**: `FeedbackCollector` and `SessionOutcome` are implemented. The `escalated_mid_session` field is noted. No callsite outside tests populates or reads the `FeedbackSummary` for routing efficiency analysis.
**Impact**: Routing efficiency metric exists but is never surfaced.
**Effort**: Low

### COUPLING-001: std::sync::Mutex in Async Context (MEDIUM)

**Severity**: Medium
**Location**: `repl/idempotency.rs:8,43`, `repl/permission_lifecycle.rs:11`, `repl/schema_validator.rs:19`, `repl/response_cache.rs:9`
**Evidence**:
```rust
// repl/idempotency.rs:8
use std::sync::Mutex;
// repl/idempotency.rs:43
records: Mutex<HashMap<String, ExecutionRecord>>,
```
Standard `std::sync::Mutex` inside async contexts is a deadlock risk if the lock is held across an `.await` point. `tokio::sync::Mutex` is used correctly in `safe_edit_manager.rs` and `tool_speculation.rs`. Inconsistency.
**Impact**: Potential tokio runtime thread starvation under contention.
**Effort**: Low (swap to tokio::sync::Mutex or dashmap)

### COUPLING-002: AgentContext Has 40 Fields (HIGH)

**Severity**: High
**Location**: `repl/agent/mod.rs:71-160`
**Evidence**: `AgentContext<'a>` has exactly 40 public fields. Sub-agents, the orchestrator, the replay runner, and the JSON-RPC bridge all construct this struct manually. Any addition to `AgentContext` requires updates in 4+ construction sites.
**Impact**: Brittle API surface; forces partial construction in tests with meaningless zero values.
**Effort**: Medium (group into sub-structs)

### ASYNC-001: sync std::fs in TUI Project Analyzer (LOW)

**Severity**: Low
**Location**: `tui/project_analyzer/tools.rs:470`, and 15 other sites
**Evidence**: `std::fs::read_dir()`, `std::fs::read_to_string()` called directly in `project_analyzer` code that runs in a tokio task context.
**Impact**: Blocks tokio thread; acceptable for project-analysis background task but inconsistent with the rest of the async codebase.
**Effort**: Low

### CONFIG-001: PolicyConfig Has 50+ Fields (MEDIUM)

**Severity**: Medium
**Location**: `crates/halcon-core/src/types/policy_config.rs`
**Evidence**: PolicyConfig governs: reward thresholds, critic thresholds, evidence thresholds, synthesis tokens, tool trust, retry, growth, mini-critic, SLA budgets, intent pipeline thresholds, halcon_md, auto_memory, hooks, boundary_decision_engine, intent_pipeline, agent_registry, and memory_importance_threshold. 50+ `#[serde(default)]` fields.
**Impact**: Any new feature adds another field; no grouping by concern makes it impossible to pass a "just the reward config" subset.
**Effort**: Medium (group into sub-structs with serde flatten)

### INTEGRATION-001: StrategySelector Not Called in Agent Loop (HIGH)

**Severity**: High
**Location**: `repl/application/reasoning_engine.rs:56`, `repl/domain/strategy_selector.rs`
**Evidence**: `StrategySelector` (UCB1 multi-armed bandit) is owned by `ReasoningEngine`. `ReasoningEngine` is called from `repl/mod.rs` in the REPL-level handling, not from inside `run_agent_loop()`. The `strategy_context: Option<StrategyContext>` field in `AgentContext` carries the UCB1 output _into_ the loop, but the round-by-round reward update (`StrategySelector::record_reward()`) happens _after_ the loop exits, via `reasoning_engine.record_outcome()` called from `mod.rs`.
**Impact**: UCB1 learns between sessions, not within. Strategy cannot adapt mid-task.
**Effort**: Medium

### ERROR-001: unwrap() in Production Tests (LOW)

**Severity**: Low
**Location**: Multiple (executor.rs:1477, 1661, 1676, 1691, 1704, 1717, 1730; input_normalizer.rs:350)
**Evidence**: `panic!("expected ToolResult")` and `panic!("Expected RequestDetails")` in test helper functions. While technically in test code, these are assertion helpers that disguise unexpected states as panics rather than test failures.
**Impact**: Test failures produce cryptic crash messages rather than descriptive assertion errors.
**Effort**: Low (use `assert_matches!` or proper `assert_eq!`)

---

## Summary Table

| Category | Count | Critical | High | Medium | Low |
|----------|-------|----------|------|--------|-----|
| ARCH | 3 | 2 | 1 | 0 | 0 |
| DEAD | 3 | 0 | 3 | 0 | 0 |
| DUP | 2 | 0 | 0 | 2 | 0 |
| INCOMPLETE | 3 | 0 | 0 | 2 | 1 |
| COUPLING | 2 | 0 | 1 | 1 | 0 |
| ASYNC | 1 | 0 | 0 | 0 | 1 |
| CONFIG | 1 | 0 | 0 | 1 | 0 |
| INTEGRATION | 1 | 0 | 1 | 0 | 0 |
| ERROR | 1 | 0 | 0 | 0 | 1 |
| **TOTAL** | **17** | **2** | **6** | **6** | **3** |
