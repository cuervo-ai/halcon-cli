# Integration Map: Agent Loop Current State

**Date**: 2026-03-08
**Branch**: feature/sota-intent-architecture

---

## Agent Loop Round Trace

The following traces one complete agent round from user input to convergence decision. Each step cites the actual function and file:line where it executes.

### Pre-Loop Setup (run_agent_loop prologue, agent/mod.rs:215-900)

| Step | Action | Function / Location | Status |
|------|--------|---------------------|--------|
| P1 | PII check + provider fallback banner | `agent/mod.rs:261-270` | ✓ ACTIVE |
| P2 | Context pipeline initialization | `agent/mod.rs:345-363` (ContextPipeline::new) | ✓ ACTIVE |
| P3 | IntentScorer — task profile | `agent/mod.rs:434` (`IntentScorer::score`) | ✓ ACTIVE |
| P4 | Planning gate + planner invocation | `agent/mod.rs:446-550` (`planning_policy::decide`, `planner.plan()`) | ✓ ACTIVE (conditional) |
| P5 | Tool selection (dynamic intent-based) | `agent/mod.rs:~640-720` | ✓ ACTIVE (conditional) |
| P6 | Instruction store injection (HALCON.md) | `agent/mod.rs:~720-734` | ✓ ACTIVE (policy-gated) |
| P7 | InputNormalizer boundary normalization | `agent/mod.rs:735` | ✓ ACTIVE |
| P8 | BoundaryDecisionEngine evaluation | `agent/mod.rs:753` (`BoundaryDecisionEngine::evaluate`) | ✓ ACTIVE (policy-gated) |
| P9 | IntentPipeline reconciliation | `agent/mod.rs:795` (`IntentPipeline::resolve`) | ✓ ACTIVE (policy-gated) |
| P10 | ConvergenceController construction | `agent/mod.rs:1749-1785` (`new_with_budget`) | ✓ ACTIVE |
| P11 | Auto-memory injection | `agent/mod.rs:~800-850` | ✓ ACTIVE (policy-gated) |
| P12 | Agent registry manifest injection | `agent/mod.rs:~850-900` (Feature 4) | ✓ ACTIVE (policy-gated) |
| P13 | UserPromptSubmit lifecycle hook | `agent/mod.rs:~900-930` (Feature 2) | ✓ ACTIVE (policy-gated) |

### Per-Round Loop Body (`'agent_loop`, agent/mod.rs:~950 onwards)

| Step | Phase | Action | Function / Location | Status |
|------|-------|--------|---------------------|--------|
| F1 | round_setup | Auto-pause + ctrl_rx check | `round_setup::run()`, `agent/mod.rs:~960` | ✓ ACTIVE |
| F2 | round_setup | Instruction hot-reload check | `round_setup.rs:~50-80` | ✓ ACTIVE |
| F3 | round_setup | Tool selection + TBAC filter | `round_setup.rs` | ✓ ACTIVE |
| F4 | provider_round | Model request construction + invoke | `provider_round::run()` | ✓ ACTIVE |
| F5 | provider_round | Stream response, accumulate tool uses | `provider_round.rs` | ✓ ACTIVE |
| F6 | post_batch | Tool deduplication (LoopGuard hash) | `post_batch::run()`, `loop_guard.rs` | ✓ ACTIVE |
| F7 | post_batch | Tool execution (parallel/sequential) | `executor::execute_parallel_batch()`, `executor::execute_sequential()` | ✓ ACTIVE |
| F8 | post_batch | Plan step tracking | `execution_tracker.rs` | ✓ ACTIVE |
| F9 | post_batch | PostBatchSupervisor evaluation | `supervisor.rs` (called from post_batch) | ✓ ACTIVE |
| F10 | post_batch | Reflexion + memory | `reflexion.rs` (called from post_batch) | ○ CONDITIONAL |
| F11 | post_batch | Plugin pre/post invoke gates | `plugin_registry.rs` (in post_batch, if Some) | ○ CONDITIONAL |
| F12 | convergence_phase | ConvergenceController::observe_round | `convergence_phase.rs:67` | ✓ ACTIVE |
| F13 | convergence_phase | ARIMA anomaly detection | `convergence_phase.rs:~150-200` | ✓ ACTIVE |
| F14 | convergence_phase | TerminationOracle::adjudicate | `convergence_phase.rs:545` | ✓ ACTIVE |
| F15 | convergence_phase | RoutingAdaptor::check (T1-T4) | `convergence_phase.rs:561` | ○ CONDITIONAL (boundary_decision only) |
| F16 | convergence_phase | RoundScorer evaluation | `convergence_phase.rs:~300-400` | ✓ ACTIVE |
| F17 | convergence_phase | SynthesisGate evaluation | `loop_state.rs:662,687` | ✓ ACTIVE |
| F18 | convergence_phase | LoopGuard match arms + dispatch | `convergence_phase.rs:~800-1300` | ✓ ACTIVE |
| F19 | convergence_phase | Checkpoint save | `checkpoint.rs` (called from agent/mod.rs) | ✓ ACTIVE |
| **MISSING** | any | **SignalArbitrator** | not called anywhere in loop | ✗ ORPHAN |
| **MISSING** | convergence | **reward_pipeline::compute_reward** | called in `repl/mod.rs:2919` post-session | ✗ NOT IN LOOP |
| **MISSING** | pre-loop | **StrategySelector::select** result used | strategy_context passed in but UCB1 update only post-session | ✗ INTER-SESSION ONLY |

### Post-Loop Cleanup (agent/mod.rs after 'agent_loop break)

| Step | Action | Function / Location | Status |
|------|--------|---------------------|--------|
| C1 | Plan synthesis step mark | `execution_tracker.rs` | ✓ ACTIVE |
| C2 | SessionRetrospective analysis | `domain/session_retrospective.rs` | ✓ ACTIVE (tracing only) |
| C3 | result_assembly::build | `result_assembly.rs` | ✓ ACTIVE |
| C4 | Auto-memory background write | `auto_memory::record_session_snapshot` | ✓ ACTIVE (tokio::spawn) |
| C5 | Stop lifecycle hook | `hooks::HookEventName::Stop` | ✓ ACTIVE (policy-gated) |

---

## Subsystem Integration Status Table

| Subsystem | File | Status | Evidence |
|-----------|------|--------|----------|
| **TerminationOracle** | `domain/termination_oracle.rs` | **INTEGRATED** | Called at `convergence_phase.rs:545`; authoritative. 14 tests pass. |
| **ConvergenceController** | `domain/convergence_controller.rs` | **INTEGRATED** | Constructed at `agent/mod.rs:1749`; `observe_round()` called at `convergence_phase.rs:67`. |
| **ToolLoopGuard** | `repl/loop_guard.rs` | **INTEGRATED** | `ToolLoopGuard` in `LoopGuardState`; hash dedup in post_batch; match arms in convergence_phase. |
| **BoundaryDecisionEngine** | `decision_engine/mod.rs` | **INTEGRATED** | Called at `agent/mod.rs:753` when `policy.use_boundary_decision_engine`. |
| **UCB1 StrategySelector** | `domain/strategy_selector.rs` | **PARTIAL** | `StrategyContext` passed to agent loop via `AgentContext.strategy_context`. UCB1 record_reward() only called post-session in `repl/mod.rs`. No within-session adaptation. |
| **reward_pipeline** | `repl/reward_pipeline.rs` | **PARTIAL** | Called in `repl/mod.rs:2919` post-session. Not called inside `run_agent_loop()`. |
| **PluginRegistry** | `repl/plugin_registry.rs` | **PARTIAL** | Passed as `Option<Arc<Mutex<PluginRegistry>>>`. Plugin gating in post_batch gated by `if let Some`. Supervisor integration comment exists but not implemented. |
| **SignalArbitrator** | `domain/signal_arbitrator.rs` | **ORPHAN** | `#[deprecated]`. 14 tests. Zero callsites outside tests. Explicit note: "not called from the agent loop." |
| **RoutingAdaptor T1/T2** | `decision_engine/routing_adaptor.rs` | **INTEGRATED** | T1 (security signals) and T2 (tool failure ≥60%) checked at `convergence_phase.rs:561`. |
| **RoutingAdaptor T3/T4** | `decision_engine/routing_adaptor.rs` | **PARTIAL** | T3/T4 triggers are implemented in `RoutingAdaptor::check()` but the escalation path does not update `sla_budget`, only `conv_ctrl.set_max_rounds()`. |
| **SynthesisGate GovernanceRescue** | `domain/synthesis_gate.rs` | **INTEGRATED** | `evaluate()` enforced at `loop_state.rs:662,687`. GovernanceRescue blocks when reflection_score < 0.15 && rounds_executed < 3. |
| **IntentPipeline** | `decision_engine/intent_pipeline.rs` | **INTEGRATED** | Called at `agent/mod.rs:795` when `policy.use_intent_pipeline`. Reconciles IntentScorer + BDE. |
| **AutoMemory** | `repl/auto_memory/` | **INTEGRATED** | Injector at loop start; background write tokio::spawn at loop end. |
| **HALCON.md InstructionStore** | `repl/instruction_store/` | **INTEGRATED** | Initialized in pre-loop; per-round hot-reload in `round_setup.rs`. |
| **LifecycleHooks** | `repl/hooks/` | **INTEGRATED** | UserPromptSubmit, PreToolUse, PostToolUse, PostToolUseFailure, Stop all wired. |
| **AgentRegistry** | `repl/agent_registry/` | **INTEGRATED** | Manifest injected into system prompt when `enable_agent_registry=true`. |
| **FeedbackCollector** | `decision_engine/decision_feedback.rs` | **STUB** | Implemented. Zero callsites outside tests. Never aggregated. |
| **SessionRetrospective** | `domain/session_retrospective.rs` | **PARTIAL** | Called at post-loop cleanup; result logged via `tracing::info!` but not surfaced to user or stored. |

---

## Data Flow: Key Structures

### LoopState

```
LoopState {
  messages: Vec<ChatMessage>              -- mutated every round
  context_pipeline: ContextPipeline      -- token-budgeted view
  convergence: ConvergenceState {
    conv_ctrl: ConvergenceController      -- observes rounds, produces actions
    round_scorer: RoundScorer             -- per-round signal aggregate
    decision_trace: DecisionTraceCollector -- per-round audit trail
    mid_loop_critic: MidLoopCritic        -- progress-aware checkpoints
    ... 10 more fields
  }
  guards: LoopGuardState {
    loop_guard: ToolLoopGuard             -- hash-based stagnation detection
    failure_tracker: ToolFailureTracker   -- per-tool failure counts
    semantic_cycle_detector: ...
  }
  synthesis: SynthesisControl {
    forced_synthesis_detected: bool
    tool_decision: ToolDecisionSignal
    ... FSM state
  }
  boundary_decision: Option<BoundaryDecision>  -- populated pre-loop
  policy: Arc<PolicyConfig>              -- 50+ thresholds
}
```

### RoundFeedback (domain/round_feedback.rs)

Populated in `convergence_phase.rs:~300-540` from:
- `convergence.round_scorer.evaluate()`
- `round_convergence_action` (from ConvergenceController)
- `tool_failures.len()`, `tool_successes.len()` (REAL data, wired in A2 remediation)
- `security_signals_detected` (from post_batch)

Consumed by:
- `TerminationOracle::adjudicate(&round_feedback)` at `convergence_phase.rs:545`
- `RoutingAdaptor::check(mode, round, &round_feedback, &store)` at `convergence_phase.rs:561`
- `AdaptivePolicy` (indirect, via ConvergenceController state)

### BoundaryDecision (decision_engine/mod.rs)

Populated once in pre-loop at `agent/mod.rs:753`. Stored in `LoopState.boundary_decision`.
Consumed by:
- `post_batch.rs:540` for per-session convergence policy enforcement
- `convergence_phase.rs:558` for RoutingAdaptor gating

### DecisionTrace (decision_engine/decision_trace.rs vs domain/decision_trace.rs)

Two separate types:
- `decision_engine::DecisionTrace` — BDE pipeline routing trace (populated by BDE, stored in BoundaryDecision)
- `domain::decision_trace::DecisionTraceCollector` — per-round agent decision audit (stored in ConvergenceState, analyzed in SessionRetrospective)
