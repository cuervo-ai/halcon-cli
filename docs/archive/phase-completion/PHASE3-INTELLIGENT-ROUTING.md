# Phase 3: Intelligent Routing — Wire All Dead Code

## Executive Summary

Phase 2 built the resilience infrastructure (circuit breaker, health scoring,
backpressure, optimizer, speculative invoker). Phase 3 wires these modules into
the production agent loop, eliminating all `#[allow(dead_code)]` annotations
from the REPL layer.

**Goal**: Every module actively participates in request processing.

## Current State (Post Phase 2)

| Module | Built | Wired | Dead Code? |
|--------|-------|-------|-----------|
| Circuit Breaker | Yes | Yes (via ResilienceManager) | `#[allow(dead_code)]` on mod |
| Backpressure | Yes | Yes (via ResilienceManager) | `#[allow(dead_code)]` on mod |
| Health Scorer | Yes | **NO** | `#[allow(dead_code)]` on mod |
| Optimizer | Yes | **NO** | `#[allow(dead_code)]` on mod |
| Router | Yes | **NO** (used by Speculative only) | `#[allow(dead_code)]` on mod |
| Speculative Invoker | Yes | **NO** | `#[allow(dead_code)]` on mod |
| ResilienceManager.diagnostics() | Yes | **NO** | `#[allow(dead_code)]` on fn |
| ProviderDiagnostic | Yes | **NO** | `#[allow(dead_code)]` on struct |

## Architecture After Phase 3

```
User Message
    |
    v
[Context Assembly] --> system prompt
    |
    v
[Agent Loop]
    |
    +--> [Response Cache] -- HIT --> return cached
    |
    +--> [Resilience Pre-Invoke]
    |     |
    |     +--> Circuit Breaker check
    |     +--> Health Score check (NEW)
    |     +--> Backpressure permit
    |     |
    |     +--> FALLBACK? --> [Try Fallback Providers] (NEW)
    |                         |
    |                         +--> Sequential resilience-checked fallback
    |                         +--> All blocked? --> exit with error
    |
    +--> provider.invoke() --> SSE stream
    |
    +--> [Post-Invoke]
    |     +--> record_success/failure
    |     +--> Persist resilience events to DB (NEW)
    |
    +--> [Metrics] persist InvocationMetric
    +--> [Cache] store response
    +--> [Optimizer] log advisory ranking (NEW)
    |
    v
[Tool Execution or EndTurn]
```

## Sub-Phases

### Sub-Phase 1: Health Scorer → Resilience Manager (3 tests)

Wire `HealthScorer` into `ResilienceManager.pre_invoke()`.

**Changes:**
- `resilience.rs`: Add optional `HealthScorer` field
- `resilience.rs`: New `FallbackReason::ProviderUnhealthy` variant
- `resilience.rs`: `pre_invoke()` checks health between breaker and backpressure
- `mod.rs`: Pass DB to ResilienceManager for HealthScorer init
- Remove `#[allow(dead_code)]` from `health` module

### Sub-Phase 2: Resilience Event Persistence (3 tests)

Persist circuit breaker transitions and health changes to the DB.

**Changes:**
- `resilience.rs`: Add optional `Arc<Database>` field
- `resilience.rs`: On breaker trip/recovery → `db.insert_resilience_event()`
- `resilience.rs`: Remove `#[allow(dead_code)]` from `diagnostics()`
- `mod.rs`: Pass DB when constructing ResilienceManager

### Sub-Phase 3: Fallback on Resilience Rejection (4 tests)

When primary provider is rejected by resilience, try fallback providers.

**Changes:**
- `agent.rs`: Add `fallback_providers: &[(String, Arc<dyn ModelProvider>)]` param
- `agent.rs`: Extract `invoke_with_fallback()` helper function
- `agent.rs`: On Fallback, iterate fallback providers with resilience checks
- `mod.rs`: Build fallback_providers from ProviderRegistry + RoutingConfig
- Remove `#[allow(dead_code)]` from `router` and `speculative` modules

### Sub-Phase 4: Optimizer → Doctor + Advisory Logging (3 tests)

Wire optimizer into the doctor command and add advisory logging.

**Changes:**
- `doctor.rs`: Add optimizer ranking section (top models by strategy)
- `doctor.rs`: Wire HealthScorer for per-provider health in doctor output
- `agent.rs`: Log optimizer suggestion after each invocation (tracing::debug)
- Remove `#[allow(dead_code)]` from `optimizer` module

### Sub-Phase 5: Clean Up + Final Verification (0 new tests)

Remove all remaining `#[allow(dead_code)]` annotations and verify clean build.

**Changes:**
- Remove `#[allow(dead_code)]` from `backpressure`, `circuit_breaker` modules
- Remove `#[allow(dead_code)]` from `ProviderDiagnostic`, `RankedModel`
- Remove `#[allow(dead_code)]` from `InvocationResult` in speculative.rs
- Verify `cargo clippy --workspace -- -D warnings` is clean
- Verify `cargo test --workspace` passes

## Estimated Impact

- **New tests**: ~13
- **Dead code removed**: 8 `#[allow(dead_code)]` annotations eliminated
- **Risk**: Low — all modules are individually tested; wiring is additive

## Constraints

- Zero breaking changes to existing config/API
- All new features opt-in via `[resilience]` config section
- Clippy clean (`-D warnings`) after each sub-phase
- No new crate dependencies
