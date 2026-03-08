# Remediation Baseline — 2026-03-08

## BASELINE (captured before any changes)

| Métrica | Baseline |
|---------|----------|
| Tests passing | (running) |
| Compiler warnings | 838 |
| Compiler errors | 0 |
| ctrl_rx: None occurrences | 12 |
| std::sync::Mutex in async (non-test) | 48 |
| #[deprecated] in domain/ | 0 |

## Files with std::sync::Mutex (non-test):
- repl/idempotency.rs:8
- repl/response_cache.rs:9
- repl/schema_validator.rs:19
- repl/model_selector.rs (3 uses)
- repl/agent/post_batch.rs:59 (plugin_registry Arc<Mutex>)
- repl/agent/mod.rs:146,1018,1019 (plugin_registry, VectorMemoryStore)
- repl/agent/result_assembly.rs:43 (plugin_registry)
- repl/agent/context.rs:86 (plugin_registry)
- repl/mod.rs:347,2624 (plugin_registry)
- repl/permission_lifecycle.rs:11 (confirmed exists)

## Key findings from audit:
- BV-1 (ConvergenceController calibration): PARTIALLY FIXED — IntentPipeline path uses new_with_budget().
  Legacy path (use_intent_pipeline=false) still has mismatch.
- R1-A (SynthesisGate ordering): SynthesisGate runs via request_synthesis_with_gate() which is
  called before convergence_phase. But TerminationOracle can independently output InjectSynthesis
  WITHOUT going through SynthesisGate — this is the bug. Fix: add governance_rescue_active to
  RoundFeedback and downgrade in TerminationOracle.
- GAP-5 (ctrl_rx): Non-TUI ControlReceiver = () (unit type). Cannot pass real channel.
  Requires changing ControlReceiver type for non-TUI mode.
- COUPLING-001: idempotency.rs/response_cache.rs/schema_validator.rs use std::sync::Mutex
  but their lock() calls are NOT held across .await points (synchronous operations).
  Risk is lower than audit claimed. Still best practice to use tokio::sync::Mutex.

---

## Remediation Results — 2026-03-08

### FASE 1 (Critical Bugs) — COMPLETE
| Issue | Fix | Commit |
|-------|-----|--------|
| ARCH-SYNC-1: SynthesisGate vs TerminationOracle | Added `governance_rescue_active` to RoundFeedback; TerminationOracle gates all InjectSynthesis paths | 59b222a |
| GAP-5: Classic REPL cancellation | ClassicCancelSignal enum; non-TUI ControlReceiver now real channel; ctrl_c handler; try_recv() in provider_round | ef0301d |
| BV-1: ConvergenceController calibration | Both IntentPipeline and legacy paths now use new_with_budget(); BV-1 regression tests | adc56e6 |
| COUPLING-001: std::Mutex across .await | permission_lifecycle.rs: lock dropped before .await; search_engine_global.rs: tokio::Mutex | 143fc36 |
| feature flags default | use_halcon_md, enable_auto_memory, enable_agent_registry all default true | 71aa8dd |

### FASE 2 (Feature Activation) — COMPLETE
| Issue | Fix | Commit |
|-------|-----|--------|
| R2-B (GAP-1): UCB1 per-round | record_per_round_signals() method; wired in repl/mod.rs after post_loop_with_reward() | 66a7008 |
| R2-C (GAP-2): SessionRetrospective | Fire-and-forget JSONL write to .halcon/retrospectives/sessions.jsonl | 66a7008 |
| R2-D (GAP-4): FeedbackCollector | routing_escalation_count: u32 field in ConvergenceState + AgentLoopResult | 66a7008 |
| R2-E (PARTIAL-1): SLA budget sync | state.sla_budget.max_rounds updated when RoutingAdaptor escalation fires | 66a7008 |

### FASE 3 (Decision Engine) — ALREADY COMPLETE
- GAP-3 (SignalArbitrator): Already deleted in A1 migration commit
- PARTIAL-3 (Plugin circuit-breaker): Already wired in post_batch.rs:844 (SuspendPlugin verdict)
- PARTIAL-4 (Guardrail coverage): Already scanning all tool_result_blocks in post_batch.rs:376-400

### FASE 4 (Stub Cleanup) — RECLASSIFIED
All 10 "stub" modules from the audit report are ACTIVELY USED:
- anomaly_detector.rs: 19 callsites (loop_guard, convergence_phase)
- arima_predictor.rs: 2 callsites (agent/mod.rs, loop_state.rs)
- backpressure.rs: 21 callsites (provider_client, resilience)
- capability_resolver.rs: 6 callsites (plugin_registry)
- ci_detection.rs: 1 callsite (authorization)
- context_governance.rs: 8 callsites (context_manager, mod.rs)
- architecture_server.rs, codebase_server.rs, requirements_server.rs, workflow_server.rs: all used in mod.rs

### FASE 5 (Verification) — COMPLETE
- **4318 tests pass** (2 pre-existing cache test failures unrelated to remediation work)
- Build: 0 errors, ~731 warnings
- Pre-existing failures: repl::agent::tests::cache_hit_skips_provider + cache_miss_then_store
  (cache key mismatch: stored with round_request tail, looked up with original request)

### Known Pre-existing Issues (NOT fixed, out of scope)
- Cache test failures: ResponseCache::compute_key() uses last-3-messages tail; test stores with
  round_request (includes assistant response in tail) but looks up with original request (no response yet)
