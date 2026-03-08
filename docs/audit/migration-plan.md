# Migration Plan: Halcon CLI Architecture

**Date**: 2026-03-08
**Addresses**: Findings from halcon-cli-audit-2026.md
**Constraint**: Zero test regressions at each step. Current baseline: 4,656 tests.

---

## Dependency Graph Between Phases

```
Phase A (Elimination) ──┐
                        ├──▶ Phase B (Decomposition) ──▶ Phase C (Integration) ──▶ Phase D (Hardening) ──▶ Phase E (Verify)
Phase B (Decomposition)─┘
```

Steps within each phase are ordered by dependency. Steps marked [PARALLEL] can be executed concurrently with others in the same phase.

---

## Phase A: Elimination (Dead and Redundant Code)

### A1 — Remove SignalArbitrator

**Finding**: DEAD-001
**Type**: Code deletion
**Files affected**: `crates/halcon-cli/src/repl/domain/signal_arbitrator.rs`, `crates/halcon-cli/src/repl/domain/mod.rs`
**Pre-condition**: Verify no production callsite: `grep -rn "SignalArbitrator\|signal_arbitrator" src/ | grep -v test | grep -v deprecated`
**Execution**:
1. Delete `repl/domain/signal_arbitrator.rs`
2. Remove `pub mod signal_arbitrator;` from `repl/domain/mod.rs:72`
3. Remove the backward-compat re-export if any in `repl/mod.rs`
**Verification**: `cargo test -p halcon-cli 2>&1 | grep -E "FAILED|error"` — must be zero
**Rollback**: `git checkout -- crates/halcon-cli/src/repl/domain/signal_arbitrator.rs crates/halcon-cli/src/repl/domain/mod.rs`
**Effort**: 1 hour
**Risk**: Low — no production callsites confirmed

**Alternative (preferred if wiring is planned in C2)**: Keep the file, remove `#[deprecated]`, and wire it in Phase C instead of deleting.

### A2 — Remove FeedbackCollector (or promote to observability)

**Finding**: INCOMPLETE-003
**Type**: Code deletion or promotion
**Files affected**: `crates/halcon-cli/src/repl/decision_engine/decision_feedback.rs`
**Pre-condition**: Confirm zero production callsites outside tests
**Execution (deletion path)**:
1. Delete `decision_engine/decision_feedback.rs`
2. Remove `pub mod decision_feedback;` from `decision_engine/mod.rs:48`
**Execution (promotion path)**:
1. Add `FeedbackCollector` as a field to `LoopState` (in `hicon` sub-struct)
2. Record `SessionOutcome` in `result_assembly::build()`
3. Emit via `tracing::info!` in post-loop cleanup
**Verification**: `cargo test -p halcon-cli`
**Effort**: 2 hours (deletion) / 4 hours (promotion)
**Risk**: Low

### A3 — Rename duplicate DecisionTrace types [PARALLEL with A2]

**Finding**: DUP-002
**Type**: Rename
**Files affected**: `repl/domain/decision_trace.rs` → `repl/domain/agent_decision_trace.rs`, all callers
**Pre-condition**: None
**Execution**:
1. Rename file to `agent_decision_trace.rs`
2. Update `mod.rs:66` to `pub mod agent_decision_trace;`
3. Update `ConvergenceState` field type reference in `loop_state.rs`
4. Update all `super::domain::decision_trace::` paths in `convergence_phase.rs`, `result_assembly.rs`
**Verification**: `cargo test -p halcon-cli`
**Effort**: 2 hours
**Risk**: Low (purely mechanical rename)

---

## Phase B: Decomposition (God Objects)

### B1 — Decompose AgentContext into sub-structs

**Finding**: COUPLING-002
**Type**: Struct decomposition
**Files affected**: `repl/agent/mod.rs`, `repl/orchestrator.rs`, `repl/replay_runner.rs`, `repl/agent_bridge/executor.rs`, and all JSON-RPC / chat command construction sites
**Pre-condition**: Phase A must be complete (reduces field count)
**Execution**:
```rust
pub struct AgentInfrastructure<'a> {
    pub provider: &'a Arc<dyn ModelProvider>,
    pub tool_registry: &'a ToolRegistry,
    pub trace_db: Option<&'a AsyncDatabase>,
    pub response_cache: Option<&'a ResponseCache>,
    pub resilience: &'a mut ResilienceManager,
    pub fallback_providers: &'a [(String, Arc<dyn ModelProvider>)],
    pub event_tx: &'a EventSender,
}

pub struct AgentPolicyContext<'a> {
    pub limits: &'a AgentLimits,
    pub routing_config: &'a RoutingConfig,
    pub planning_config: &'a PlanningConfig,
    pub orchestrator_config: &'a OrchestratorConfig,
    pub policy: Arc<PolicyConfig>,
    pub security_config: &'a SecurityConfig,
    pub phase14: Phase14Context,
}

pub struct AgentOptional<'a> {
    pub compactor: Option<...>,
    pub planner: Option<...>,
    pub guardrails: &'a [...],
    pub reflector: Option<...>,
    pub replay_tool_executor: Option<...>,
    pub model_selector: Option<...>,
    pub critic_provider: Option<...>,
    pub critic_model: Option<String>,
    pub plugin_registry: Option<...>,
    pub strategy_context: Option<...>,
}

// Collapsed AgentContext remains for backward compat via From<(infra, policy, optional)>
```
1. Define the three sub-structs in new file `repl/agent/context.rs`
2. Add `impl From<(AgentInfrastructure<'a>, AgentPolicyContext<'a>, AgentOptional<'a>)> for AgentContext<'a>`
3. Update all construction sites to use named sub-struct fields
4. Update `run_agent_loop` destructure
**Verification**: `cargo test -p halcon-cli`
**Effort**: 8 hours
**Risk**: Medium (many construction sites, but mechanical)

### B2 — Extract LoopState.convergence into ConvergencePhaseOwner

**Finding**: ARCH-001 (partial)
**Type**: Struct extraction
**Files affected**: `repl/agent/loop_state.rs`, `repl/agent/convergence_phase.rs`, `repl/agent/mod.rs`
**Pre-condition**: B1 complete
**Execution**:
1. Define `pub(super) struct ConvergencePhaseState` that owns all fields currently in `ConvergenceState`
2. Separate `convergence_phase::run()` signature to take `&mut ConvergencePhaseState` instead of `&mut LoopState`
3. Pass `&state.convergence` explicitly; remove the other LoopState fields from convergence_phase's function body
4. Keep `LoopState.convergence: ConvergencePhaseState` as before — no structural change to LoopState yet
**Verification**: `cargo test -p halcon-cli`
**Effort**: 6 hours
**Risk**: Medium — convergence_phase.rs currently reads ~20 fields from LoopState beyond just `state.convergence`

### B3 — Split run_agent_loop into setup + loop functions

**Finding**: ARCH-002
**Type**: Function extraction
**Files affected**: `repl/agent/mod.rs`
**Pre-condition**: B1, B2 complete
**Execution**:
```rust
// agent/mod.rs: extract 3 functions
async fn build_context_pipeline(ctx: &AgentContext) -> ContextPipeline { ... }
async fn build_loop_state(ctx: &mut AgentContext, pipeline: ContextPipeline) -> Result<LoopState> { ... }
async fn run_rounds(state: &mut LoopState, ctx: &AgentContext, ...) -> Result<AgentLoopResult> { ... }

pub async fn run_agent_loop(ctx: AgentContext<'_>) -> Result<AgentLoopResult> {
    let pipeline = build_context_pipeline(&ctx).await;
    let mut state = build_loop_state(&mut ctx, pipeline).await?;
    run_rounds(&mut state, &ctx, ...).await
}
```
1. Extract the prologue (lines 216-900) into `build_loop_state()`
2. Extract the `'agent_loop` body into `run_rounds()`
3. `run_agent_loop()` becomes a 30-line orchestrator
**Verification**: `cargo test -p halcon-cli`
**Effort**: 12 hours
**Risk**: High — many `let mut` variables cross function boundaries; lifetime constraints

### B4 — Decompose repl/mod.rs into repl/repl.rs + repl/session_loop.rs [PARALLEL with B3]

**Finding**: ARCH-003
**Type**: File split
**Files affected**: `repl/mod.rs`
**Pre-condition**: None (independent of B1-B3)
**Execution**:
1. Extract the `Repl` struct definition and its `impl` into `repl/repl.rs`
2. Extract the REPL run loop (REPL::run, handle_message_with_sink, etc.) into `repl/session_loop.rs`
3. Keep `repl/mod.rs` as a thin re-export facade (< 100 lines)
4. Move reward_pipeline wiring into `session_loop.rs`
**Verification**: `cargo test -p halcon-cli`
**Effort**: 8 hours
**Risk**: Medium (many cross-module imports)

---

## Phase C: Integration Wiring

### C1 — Wire reward_pipeline into convergence_phase (intra-session UCB1)

**Finding**: DEAD-003, INTEGRATION-001
**Type**: Feature wiring
**Files affected**: `repl/agent/convergence_phase.rs`, `repl/agent/loop_state.rs`
**Pre-condition**: B3 (run_rounds extracted), so reward_pipeline can be called per-round
**Execution**:
1. Add `reward_accumulator: Vec<RawRewardSignals>` to `LoopState` (in `hicon` sub-struct)
2. In `convergence_phase.rs` after `TerminationOracle::adjudicate()`, compute per-round reward from `RoundFeedback` fields and push to accumulator
3. On Halt/Synthesize, call `reasoning_engine.record_outcome_from_accumulator(&accumulator)` if reasoning_engine is available
4. Remove the REPL-level reward call from `repl/mod.rs:2919` (or keep as fallback for sessions without reasoning engine)
**Verification**: Run `cargo test -p halcon-cli`; verify `reward_pipeline_feeds_ucb1_strategy_learning` test in `reasoning_engine.rs` still passes
**Effort**: 6 hours
**Risk**: Low — additive; existing REPL-level path is fallback

### C2 — Wire SignalArbitrator into convergence_phase (or remove — see A1)

**Finding**: DEAD-001, INCOMPLETE-002
**Type**: Feature wiring
**Files affected**: `repl/agent/convergence_phase.rs`, `repl/domain/signal_arbitrator.rs`
**Pre-condition**: A1 decision (wire vs delete); B2 (ConvergencePhaseState)
**Execution (wire path)**:
1. Remove `#[deprecated]` from `SignalArbitrator`
2. In `convergence_phase.rs`, after collecting all signals (ConvergenceController action, TerminationOracle decision, SynthesisGate verdict, EBS state), assemble a `SignalBundle`
3. Call `SignalArbitrator::arbitrate(&bundle)` and use `ArbitrationResult.action` for loop dispatch
4. This replaces the current inline priority logic scattered across `convergence_phase.rs:800-1300`
**Verification**: All 14 existing `signal_arbitrator` tests must pass; overall `cargo test -p halcon-cli`
**Effort**: 10 hours
**Risk**: Medium — changes the convergence dispatch path; extensive test coverage required

### C3 — Wire FeedbackCollector to SessionOutcome [PARALLEL with C1]

**Finding**: INCOMPLETE-003
**Type**: Feature wiring
**Files affected**: `repl/decision_engine/decision_feedback.rs`, `repl/agent/result_assembly.rs`
**Pre-condition**: A2 (promotion path chosen)
**Execution**:
1. Add `feedback_collector: FeedbackCollector` to `LoopState.hicon`
2. Record `RoutingEscalation` in `convergence_phase.rs` after `RoutingAdaptor::check()`
3. In `result_assembly::build()`, call `feedback_collector.summarize()` and attach to `AgentLoopResult`
4. Log summary via `tracing::info!`
**Effort**: 4 hours
**Risk**: Low

### C4 — Fix std::sync::Mutex in async contexts [PARALLEL with C1]

**Finding**: COUPLING-001
**Type**: Correctness fix
**Files affected**: `repl/idempotency.rs`, `repl/permission_lifecycle.rs`, `repl/response_cache.rs`, `repl/schema_validator.rs`
**Pre-condition**: None
**Execution**: Replace `std::sync::Mutex` with `tokio::sync::Mutex` in the four files; update `lock()` calls to `.lock().await`
**Verification**: `cargo test -p halcon-cli`
**Effort**: 3 hours
**Risk**: Low (mechanical substitution)

---

## Phase D: Hardening

### D1 — Decompose PolicyConfig into sub-structs

**Finding**: CONFIG-001
**Type**: Struct decomposition
**Files affected**: `crates/halcon-core/src/types/policy_config.rs`, all consumers
**Pre-condition**: None (but coordinate with any agents modifying policy_config)
**Execution**:
```rust
pub struct PolicyConfig {
    #[serde(flatten)] pub reward: RewardPolicyConfig,
    #[serde(flatten)] pub critic: CriticPolicyConfig,
    #[serde(flatten)] pub convergence: ConvergencePolicyConfig,
    #[serde(flatten)] pub feature_flags: FeatureFlagPolicyConfig,
    // ... 5 groups total
}
```
Use `#[serde(flatten)]` to maintain TOML/JSON backward compatibility.
**Verification**: `cargo test --workspace`
**Effort**: 8 hours
**Risk**: Medium (many consumers of PolicyConfig fields across all crates)

### D2 — Add integration tests for subsystem interactions

**Finding**: INCOMPLETE-001, INCOMPLETE-002
**Type**: Test addition
**Files affected**: New file `repl/agent/tests.rs` (extend existing) or new `repl/integration/`
**Pre-condition**: C1, C2 complete
**Execution**:
1. `test_routing_adaptor_escalation_updates_sla_budget` — verify T3/T4 escalation persists to budget
2. `test_synthesis_gate_governance_rescue_not_overridden_by_oracle_synthesize` — verify SynthesisGate veto is respected
3. `test_reward_pipeline_updates_ucb1_within_session` — verify per-round UCB1 update after C1
**Effort**: 8 hours
**Risk**: Low

### D3 — Resolve StrategySelector inter-session vs intra-session ambiguity

**Finding**: INTEGRATION-001
**Type**: Documentation + decision
**Files affected**: `repl/domain/strategy_selector.rs`, `repl/application/reasoning_engine.rs`
**Pre-condition**: C1 (reward_pipeline wired)
**Execution**:
1. If inter-session is intentional (learn between conversations): document it explicitly in `strategy_selector.rs` module doc; remove the EXPERIMENTAL.md ambiguity
2. If intra-session is desired: C1 already wires per-round reward; ensure `reasoning_engine.record_outcome()` is called per-round
**Effort**: 2 hours
**Risk**: Low

---

## Phase E: Verification

### E1 — Full test suite baseline

```bash
cargo test -p halcon-cli 2>&1 | tail -5
# Expected: test result: ok. XXXX passed; 0 failed
```

### E2 — Dead code confirmation

```bash
cargo clippy -p halcon-cli -- -W dead_code 2>&1 | grep "warning\[dead_code\]" | wc -l
# Target: < 20 after Phase A
```

### E3 — Async correctness check

```bash
cargo clippy -p halcon-cli -- -W clippy::await_holding_lock 2>&1 | grep warning | wc -l
# Target: 0 after Phase C4
```

### E4 — File size regression check

```bash
find crates/halcon-cli/src -name "*.rs" | xargs wc -l | sort -rn | head -10
# Target: no single file > 2000 lines after Phase B
```

---

## GANTT Timeline (text)

```
Week 1:  [A1][A2][A3] ── Elimination (parallel, ~12h total)
Week 2:  [B4][C4] ── repl/mod.rs split + mutex fix (parallel)
         [B1] ── AgentContext decomposition
Week 3:  [B2] ── ConvergencePhaseState extraction
Week 4:  [B3] ── run_agent_loop split (dependent on B2)
Week 5:  [C1][C3] ── reward_pipeline + FeedbackCollector wiring (parallel)
Week 6:  [C2] ── SignalArbitrator wiring (dependent on B2, B3)
Week 7:  [D1] ── PolicyConfig decomposition
         [D2] ── Integration tests
Week 8:  [D3] ── StrategySelector documentation
         [E1-E4] ── Full verification
```

**Total estimated effort**: 8 weeks (1 engineer), or 4 weeks (2 engineers using parallel tracks)

---

## Risk Register

| Step | Risk | Mitigation |
|------|------|-----------|
| B3 | Lifetime explosion when splitting run_agent_loop | Use `Arc` for shared fields; accept some cloning |
| C2 | SignalArbitrator changes convergence dispatch | Gate with feature flag initially; A/B test on test suite |
| D1 | PolicyConfig consumers break across crates | Use `#[serde(flatten)]` — zero serialization change; compiler errors are exhaustive |
| B2 | convergence_phase reads 20+ LoopState fields | Extract into a `ConvergenceInput` view struct passed by reference |
