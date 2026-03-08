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
