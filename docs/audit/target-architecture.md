# Target Architecture: Halcon CLI

**Date**: 2026-03-08
**Target**: Post-migration state after all phases complete

---

## Target Module Structure

```
crates/halcon-cli/src/
├── main.rs                          (CLI entry, subcommand dispatch)
├── lib.rs                           (library facade)
├── config_loader.rs
│
├── commands/                        (thin command handlers — no business logic)
│   ├── mod.rs
│   ├── chat.rs
│   ├── agents.rs, mcp.rs, mcp_serve.rs, json_rpc.rs, ...
│   └── (all existing command files)
│
├── render/                          (terminal rendering — no change needed)
│   └── (all existing render files)
│
├── tui/                             (TUI app — no change needed)
│   └── (all existing tui files)
│
├── agent_bridge/                    (bridge to external agent runtimes)
│   └── (all existing bridge files)
│
└── repl/
    ├── mod.rs                       (< 100 lines: module declarations + re-exports)
    ├── repl.rs                      (Repl struct + impl)
    ├── session_loop.rs              (Repl::run(), handle_message, slash commands)
    │
    ├── agent/                       (agent loop — decomposed)
    │   ├── context.rs               (AgentContext with 3 sub-structs: Infrastructure/Policy/Optional)
    │   ├── mod.rs                   (run_agent_loop: 30-line orchestrator)
    │   ├── setup.rs                 (build_context_pipeline, build_loop_state)
    │   ├── rounds.rs                (run_rounds: 'agent_loop body)
    │   ├── loop_state.rs            (LoopState with 6 named sub-structs)
    │   ├── round_setup.rs
    │   ├── provider_round.rs
    │   ├── post_batch.rs
    │   ├── convergence_phase.rs     (takes ConvergencePhaseState, not full LoopState)
    │   ├── result_assembly.rs
    │   ├── checkpoint.rs
    │   ├── tests.rs
    │   └── (other existing agent/ files)
    │
    ├── domain/                      (pure business logic — zero I/O)
    │   ├── mod.rs
    │   ├── intent_scorer.rs
    │   ├── convergence_controller.rs
    │   ├── termination_oracle.rs
    │   ├── signal_arbitrator.rs     (WIRED — arbitrates TerminationOracle vs SynthesisGate)
    │   ├── synthesis_gate.rs
    │   ├── round_feedback.rs
    │   ├── adaptive_policy.rs
    │   ├── strategy_selector.rs     (UCB1 — reward fed per-round from convergence_phase)
    │   ├── agent_decision_trace.rs  (renamed from decision_trace.rs)
    │   └── (all other domain modules)
    │
    ├── decision_engine/             (BDE pipeline — no change needed)
    │   ├── mod.rs
    │   ├── decision_feedback.rs     (FeedbackCollector WIRED to result_assembly)
    │   └── (all other decision_engine modules)
    │
    ├── executor.rs                  (tool execution — no structural change)
    ├── orchestrator.rs              (multi-agent — no structural change)
    │
    ├── hooks/                       (lifecycle hooks Feature 2)
    ├── auto_memory/                 (auto-memory Feature 3)
    ├── instruction_store/           (HALCON.md Feature 1)
    ├── agent_registry/              (agent registry Feature 4)
    │
    └── (all other existing repl modules)
```

---

## Module Boundary Diagram (ASCII)

```
┌─────────────────────────────────────────────────────────────────────┐
│                          halcon-cli binary                           │
│                                                                      │
│  main.rs → commands/ → repl/session_loop.rs → repl/agent/mod.rs    │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │  repl/agent/  (Agent Loop Layer)                             │   │
│  │                                                              │   │
│  │  run_agent_loop()                                            │   │
│  │    ├── setup.rs → builds LoopState                          │   │
│  │    └── rounds.rs → 'agent_loop                              │   │
│  │          ├── round_setup.rs      (prepares request)         │   │
│  │          ├── provider_round.rs   (invokes model)            │   │
│  │          ├── post_batch.rs       (executes tools)           │   │
│  │          └── convergence_phase.rs (decides termination)     │   │
│  │                ├── TerminationOracle::adjudicate()          │   │
│  │                ├── SignalArbitrator::arbitrate()  [TARGET]  │   │
│  │                └── reward_pipeline per-round     [TARGET]  │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                           │                                          │
│          ┌────────────────┼────────────────┐                        │
│          ▼                ▼                ▼                        │
│  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐               │
│  │ repl/domain/ │ │repl/decision │ │ repl/executor│               │
│  │ (pure logic) │ │  _engine/    │ │  (tool exec) │               │
│  │              │ │ (BDE pipe)   │ │              │               │
│  │ NO I/O       │ │ NO I/O       │ │ ASYNC I/O    │               │
│  └──────────────┘ └──────────────┘ └──────────────┘               │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ Feature modules (policy-gated)                               │   │
│  │ hooks/ | auto_memory/ | instruction_store/ | agent_registry/ │   │
│  └──────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
         │                           │
         ▼                           ▼
┌──────────────────┐       ┌──────────────────────────────┐
│   halcon-core    │       │  halcon-providers             │
│   (types,traits) │       │  halcon-tools                 │
│   policy_config  │       │  halcon-storage               │
│   (grouped subs) │       │  halcon-context               │
└──────────────────┘       └──────────────────────────────┘
```

---

## Intended Dependency Graph

```
halcon-cli
├── READS: halcon-core (types, traits, PolicyConfig)
├── READS: halcon-providers (ProviderRegistry, ModelProvider)
├── READS: halcon-tools (ToolRegistry, tool implementations)
├── READS: halcon-storage (AsyncDatabase, metrics)
├── READS: halcon-context (ContextPipeline)
├── READS: halcon-security (Guardrail)
├── READS: halcon-mcp (McpManager, McpHttpServer)
└── READS: halcon-agent-core (UCB1 learner, fastembed — future extraction)

repl/domain/   ── NO outward deps (pure Rust, no crate deps)
repl/decision_engine/ ── depends on: repl/domain/ (via super::)
repl/agent/    ── depends on: domain/, decision_engine/, executor, orchestrator
executor.rs    ── depends on: halcon-tools, halcon-storage
orchestrator.rs ── depends on: repl/agent/ (to spawn sub-agent loops)
```

**Key invariant**: `repl/domain/` must never import from `repl/executor.rs`, `repl/orchestrator.rs`, or any halcon-* infrastructure crate. This is currently respected and must be preserved.

---

## Architectural Invariants Checklist

After migration, the following invariants must hold:

### Invariant 1: Domain Layer Purity
- [ ] `repl/domain/*.rs` contains zero `use halcon_storage`, `use halcon_tools`, `use halcon_providers`
- [ ] `repl/domain/*.rs` contains zero `std::fs::`, `std::net::`, `tokio::fs::`, `reqwest::`
- [ ] Verifiable: `grep -r "halcon_storage\|halcon_tools\|halcon_providers" repl/domain/` returns empty

### Invariant 2: Agent Loop Termination Authority
- [ ] `TerminationOracle::adjudicate()` is called exactly once per round (in convergence_phase)
- [ ] `SignalArbitrator::arbitrate()` runs after TerminationOracle and before dispatch
- [ ] No other code sets `forced_synthesis_detected = true` outside of `LoopState::request_synthesis()`

### Invariant 3: LoopState Mutation Discipline
- [ ] `LoopState` is only mutated from within phase functions (`round_setup`, `post_batch`, `convergence_phase`, `result_assembly`)
- [ ] No direct mutation of `LoopState` fields from `run_agent_loop()` body after construction

### Invariant 4: Feature Flag Isolation
- [ ] Every feature-module call site is guarded by `if policy.enable_X`
- [ ] Disabling all feature flags must produce identical behavior to the pre-2026 baseline

### Invariant 5: Async Correctness
- [ ] No `std::sync::Mutex` held across `.await` points (enforced by `clippy::await_holding_lock`)
- [ ] No `std::fs` calls in async contexts outside of explicitly-documented synchronous-safe paths

### Invariant 6: Test Coverage Floor
- [ ] Every subsystem listed in the integration map has at least one integration test that exercises the call from the agent loop (not just unit tests in isolation)
- [ ] `cargo test -p halcon-cli` always passes 4,656+ tests

### Invariant 7: No God Objects Above Threshold
- [ ] No struct has more than 20 direct fields (use sub-structs above this)
- [ ] No function body exceeds 500 lines (use extracted helpers above this)
- [ ] `AgentContext` has at most 10 direct fields after B1

### Invariant 8: Config Backward Compatibility
- [ ] `PolicyConfig` serialization round-trips with existing `config/default.toml` (no field renames)
- [ ] `#[serde(flatten)]` used on all PolicyConfig sub-structs to preserve TOML key names
