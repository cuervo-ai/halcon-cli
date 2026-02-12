# Phase 2 — Self-Healing Runtime: Resilience Layer

> Technical Design Document — Phase 0 Investigation Complete
> Date: 2026-02-07
> Status: DESIGN (no code written)

---

## 1. Executive Summary

Convert Cuervo from a "first failure = session death" runtime into a **self-healing runtime** with:

- **Circuit breakers** that isolate failing providers
- **Health scoring** that quantifies provider reliability in real-time
- **Backpressure** that prevents saturation
- **Auto-degradation** that gracefully falls back when providers fail
- **Observability** via `cuervo doctor` and persisted resilience events
- **Zero breaking changes** — opt-in via `ResilienceConfig`

---

## 2. Current State — 31 Critical Failure Points

### 2.1 Provider Layer (Fatal)

| # | Location | Issue | Impact |
|---|----------|-------|--------|
| 1 | `agent.rs:178` | No timeout on `provider.invoke()` | Hangs indefinitely on unresponsive provider |
| 2 | `agent.rs:185` | One provider error = session death | `Err(e) => return Err(...)` kills the entire session |
| 3 | `agent.rs:190-220` | No per-chunk timeout on SSE stream | Slow chunk stalls the entire loop |
| 4 | `agent.rs:139-177` | Cache miss → no fallback | Single provider path, no degradation |

### 2.2 Router / Speculative (Dead Code)

| # | Location | Issue | Impact |
|---|----------|-------|--------|
| 5 | `router.rs` | Not wired into agent loop | 100% dead code |
| 6 | `router.rs:39-78` | No backoff between retries | Hammers failing provider |
| 7 | `router.rs:48` | No circuit breaker check | Retries to a known-broken provider |
| 8 | `speculative.rs` | Not wired into agent loop | 100% dead code |
| 9 | `speculative.rs:65` | No timeout on race | `select_ok` waits forever if all futures hang |

### 2.3 Tool Execution

| # | Location | Issue | Impact |
|---|----------|-------|--------|
| 10 | `executor.rs:parallel` | No batch-level timeout | `join_all` waits for slowest tool |
| 11 | `executor.rs:sequential` | Individual timeout exists but no aggregate | 10 tools × 30s = 5 minutes |
| 12 | `agent.rs:tool_loop` | No tool failure circuit breaker | Repeatedly invokes failing tool type |

### 2.4 Metrics Feedback Loop

| # | Location | Issue | Impact |
|---|----------|-------|--------|
| 13 | `optimizer.rs` | Not wired into agent loop | Dead code |
| 14 | `metrics.rs` | Metrics recorded but never read for routing | No feedback loop |
| 15 | `agent.rs:279-296` | Error metrics lack timeout categorization | Can't distinguish timeout vs auth vs network |

### 2.5 No Recovery Mechanisms

| # | Issue |
|---|-------|
| 16 | No automatic retry with backoff at agent level |
| 17 | No provider health tracking (only point-in-time `is_available()`) |
| 18 | No cache-as-fallback when providers are down |
| 19 | No user notification of degraded state |
| 20 | No breaker state persistence across restarts |

---

## 3. Architecture Design

### 3.1 Component Overview

```
┌─────────────────────────────────────────────────────────┐
│                    Agent Loop (agent.rs)                  │
│                                                           │
│  ┌──────────────────────────────────────────────────┐    │
│  │           ResilienceManager (facade)              │    │
│  │                                                    │    │
│  │  ┌──────────┐ ┌────────────┐ ┌────────────────┐  │    │
│  │  │ Circuit   │ │  Health    │ │  Backpressure  │  │    │
│  │  │ Breakers  │ │  Scorer    │ │  Guards        │  │    │
│  │  │ (per-prov)│ │ (per-prov) │ │  (per-prov)    │  │    │
│  │  └──────────┘ └────────────┘ └────────────────┘  │    │
│  │                                                    │    │
│  │  ┌──────────────────┐  ┌──────────────────────┐  │    │
│  │  │  Auto-Degrader   │  │  Event Recorder      │  │    │
│  │  │ (fallback logic) │  │ (DB persistence)     │  │    │
│  │  └──────────────────┘  └──────────────────────┘  │    │
│  └──────────────────────────────────────────────────┘    │
│                                                           │
│  ┌─────────────┐  ┌──────────┐  ┌───────────────┐       │
│  │ Provider    │  │ Response │  │   Metrics     │       │
│  │ Registry    │  │ Cache    │  │   DB          │       │
│  └─────────────┘  └──────────┘  └───────────────┘       │
└─────────────────────────────────────────────────────────┘

┌─────────────────────┐
│  cuervo doctor CLI  │ ← reads all above state
└─────────────────────┘
```

### 3.2 Data Flow — Resilient Invoke

```
User message arrives
    │
    ▼
┌─ ResilienceManager.invoke() ─────────────────────────┐
│                                                        │
│  1. Check circuit breaker for primary provider         │
│     ├─ Open? → skip to fallback                        │
│     └─ Closed/HalfOpen? → continue                    │
│                                                        │
│  2. Acquire backpressure permit                        │
│     ├─ Saturated? → try fallback or queue             │
│     └─ Acquired? → continue                           │
│                                                        │
│  3. Invoke provider with timeout                       │
│     ├─ Success → record success, release permit       │
│     │            update health, return response        │
│     └─ Failure → record failure, release permit       │
│                   update breaker, try fallback         │
│                                                        │
│  4. Fallback chain (if primary failed)                │
│     ├─ Cache lookup (if available)                     │
│     ├─ Next healthy provider                          │
│     └─ Degraded response (inform user)                │
│                                                        │
│  5. Record resilience event (async, fire-and-forget)  │
└────────────────────────────────────────────────────────┘
```

---

## 4. New Types & Traits

### 4.1 Config Types — `cuervo-core/src/types/config.rs`

```rust
/// Top-level resilience configuration. Added to AppConfig.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ResilienceConfig {
    /// Master switch. When false, all resilience features are bypassed.
    pub enabled: bool,
    pub circuit_breaker: CircuitBreakerConfig,
    pub health: HealthConfig,
    pub backpressure: BackpressureConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures to trip the breaker.
    pub failure_threshold: u32,         // default: 5
    /// Sliding window for counting failures (seconds).
    pub window_secs: u64,               // default: 60
    /// Duration the breaker stays open before transitioning to half-open.
    pub open_duration_secs: u64,        // default: 30
    /// Number of probe requests allowed in half-open state.
    pub half_open_probes: u32,          // default: 2
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HealthConfig {
    /// Lookback window for health score computation (minutes).
    pub window_minutes: u64,            // default: 60
    /// Health score below this = Degraded.
    pub degraded_threshold: u32,        // default: 50
    /// Health score below this = Unhealthy.
    pub unhealthy_threshold: u32,       // default: 30
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BackpressureConfig {
    /// Max concurrent provider invocations per provider.
    pub max_concurrent_per_provider: u32,   // default: 5
    /// Timeout waiting for a permit (seconds). 0 = fail immediately.
    pub queue_timeout_secs: u64,            // default: 30
}
```

**Default impls:**
```rust
impl Default for ResilienceConfig {
    fn default() -> Self {
        Self {
            enabled: false, // opt-in
            circuit_breaker: CircuitBreakerConfig::default(),
            health: HealthConfig::default(),
            backpressure: BackpressureConfig::default(),
        }
    }
}
```

### 4.2 Breaker State — `cuervo-cli/src/repl/circuit_breaker.rs`

```rust
/// Circuit breaker states (Closed → Open → HalfOpen → Closed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    /// Normal operation. All requests pass through.
    Closed,
    /// Provider is isolated. All requests fail-fast.
    Open { since: Instant, until: Instant },
    /// Allowing probe requests to test recovery.
    HalfOpen { probes_remaining: u32 },
}

/// Per-provider circuit breaker.
pub struct ProviderBreaker {
    provider: String,
    state: BreakerState,
    config: CircuitBreakerConfig,
    /// Timestamps of recent failures within the sliding window.
    failure_timestamps: VecDeque<Instant>,
    /// Consecutive successes in half-open state.
    half_open_successes: u32,
}

impl ProviderBreaker {
    pub fn new(provider: String, config: CircuitBreakerConfig) -> Self;

    /// Check if a request is allowed through.
    /// Returns Ok(()) if allowed, Err(BreakerOpen) if rejected.
    pub fn check(&mut self) -> Result<(), BreakerOpen>;

    /// Record a successful invocation.
    pub fn record_success(&mut self);

    /// Record a failed invocation.
    pub fn record_failure(&mut self) -> Option<BreakerTransition>;

    /// Get current state (for diagnostics).
    pub fn state(&self) -> BreakerState;

    /// Get the provider name.
    pub fn provider(&self) -> &str;
}

/// Returned when a breaker state changes.
#[derive(Debug, Clone)]
pub struct BreakerTransition {
    pub provider: String,
    pub from: &'static str,  // "closed", "open", "half_open"
    pub to: &'static str,
}

/// Error returned when breaker is open.
#[derive(Debug, Clone)]
pub struct BreakerOpen {
    pub provider: String,
    pub retry_after: Duration,
}
```

**State Machine Logic:**

```
record_failure():
  Closed:
    - push timestamp to failure_timestamps
    - prune timestamps outside window_secs
    - if failure_timestamps.len() >= failure_threshold:
        → transition to Open(now, now + open_duration_secs)
        → return Some(BreakerTransition { from: "closed", to: "open" })

  HalfOpen:
    → transition to Open(now, now + open_duration_secs)
    → return Some(BreakerTransition { from: "half_open", to: "open" })

  Open:
    → no-op (already open)

record_success():
  Closed: no-op
  HalfOpen:
    - half_open_successes += 1
    - if half_open_successes >= half_open_probes:
        → transition to Closed
        → clear failure_timestamps
  Open: unreachable (check() rejects)

check():
  Closed: Ok(())
  Open:
    - if now >= until:
        → transition to HalfOpen { probes_remaining: half_open_probes }
        → Ok(())
    - else: Err(BreakerOpen { retry_after: until - now })
  HalfOpen:
    - if probes_remaining > 0:
        probes_remaining -= 1
        → Ok(())
    - else: Err(BreakerOpen { retry_after: 0 })
```

### 4.3 Health Scorer — `cuervo-cli/src/repl/health.rs`

```rust
/// Health level derived from composite score.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthLevel {
    Healthy,    // score 80-100
    Degraded,   // score 50-79
    Unhealthy,  // score 0-49
}

/// Health assessment for a provider.
#[derive(Debug, Clone)]
pub struct HealthReport {
    pub provider: String,
    pub score: u32,             // 0-100
    pub level: HealthLevel,
    pub error_rate: f64,        // 0.0-1.0
    pub avg_latency_ms: f64,
    pub p95_latency_ms: u64,
    pub timeout_rate: f64,      // 0.0-1.0  (stop_reason = "timeout")
    pub invocation_count: u64,  // in window
}

/// Computes health scores from metrics DB.
pub struct HealthScorer {
    db: Arc<Database>,
    config: HealthConfig,
}

impl HealthScorer {
    pub fn new(db: Arc<Database>, config: HealthConfig) -> Self;

    /// Compute health for a specific provider.
    pub fn assess(&self, provider: &str) -> HealthReport;

    /// Compute health for all known providers.
    pub fn assess_all(&self) -> Vec<HealthReport>;

    /// Get the health level for a provider.
    pub fn level(&self, provider: &str) -> HealthLevel;
}
```

**Score Computation:**

```
score = (
    (1.0 - error_rate)    * 30   // 30% weight: reliability
  + latency_score         * 25   // 25% weight: speed
  + (1.0 - timeout_rate)  * 25   // 25% weight: availability
  + success_rate           * 20   // 20% weight: consistency
) → clamp to 0-100

latency_score = 1.0 / (1.0 + avg_latency_ms / 2000.0)  // normalized
```

**Data source:** `db.model_stats(provider, model)` aggregated per provider. New DB method `db.provider_metrics(provider, window_minutes)` needed for windowed queries.

### 4.4 Backpressure Guard — `cuervo-cli/src/repl/backpressure.rs`

```rust
/// Per-provider concurrency limiter.
pub struct BackpressureGuard {
    semaphores: HashMap<String, Arc<Semaphore>>,
    config: BackpressureConfig,
}

/// RAII permit — released on drop.
pub struct InvokePermit {
    _permit: OwnedSemaphorePermit,
    provider: String,
}

impl BackpressureGuard {
    pub fn new(config: BackpressureConfig) -> Self;

    /// Register a provider (creates semaphore with max_concurrent permits).
    pub fn register(&mut self, provider: &str);

    /// Acquire a permit. Returns Err if timeout expires.
    pub async fn acquire(&self, provider: &str) -> Result<InvokePermit, BackpressureFull>;

    /// Try to acquire without waiting.
    pub fn try_acquire(&self, provider: &str) -> Result<InvokePermit, BackpressureFull>;

    /// Current utilization for a provider (acquired / max).
    pub fn utilization(&self, provider: &str) -> (u32, u32);
}

#[derive(Debug)]
pub struct BackpressureFull {
    pub provider: String,
    pub queue_depth: u32,
    pub max_concurrent: u32,
}
```

### 4.5 Resilience Manager — `cuervo-cli/src/repl/resilience.rs`

```rust
/// Facade coordinating circuit breakers, health, and backpressure.
pub struct ResilienceManager {
    breakers: HashMap<String, ProviderBreaker>,
    health: HealthScorer,
    backpressure: BackpressureGuard,
    config: ResilienceConfig,
    db: Option<Arc<Database>>,
    event_tx: Option<broadcast::Sender<DomainEvent>>,
}

/// Result of a pre-invoke check.
pub enum PreInvokeDecision {
    /// Proceed with invocation. Permit is held.
    Proceed { permit: InvokePermit },
    /// Primary provider is unavailable. Try fallback.
    Fallback { reason: FallbackReason },
    /// All providers exhausted.
    Exhausted { reason: String },
}

#[derive(Debug, Clone)]
pub enum FallbackReason {
    BreakerOpen { provider: String, retry_after: Duration },
    Unhealthy { provider: String, score: u32 },
    Saturated { provider: String },
    InvokeError { provider: String, error: String },
}

impl ResilienceManager {
    pub fn new(
        config: ResilienceConfig,
        db: Option<Arc<Database>>,
        event_tx: Option<broadcast::Sender<DomainEvent>>,
    ) -> Self;

    /// Register a provider for tracking.
    pub fn register_provider(&mut self, name: &str);

    /// Pre-invoke check: breaker + health + backpressure.
    pub async fn pre_invoke(&mut self, provider: &str) -> PreInvokeDecision;

    /// Record successful invocation.
    pub fn record_success(&mut self, provider: &str);

    /// Record failed invocation.
    pub fn record_failure(&mut self, provider: &str);

    /// Get diagnostic report for all providers.
    pub fn diagnostics(&self) -> Vec<ProviderDiagnostic>;

    /// Select the best available provider from a list.
    pub fn select_provider(&mut self, candidates: &[String]) -> Option<String>;
}

/// Diagnostic snapshot for a single provider.
#[derive(Debug, Clone)]
pub struct ProviderDiagnostic {
    pub provider: String,
    pub breaker_state: &'static str,
    pub health_score: u32,
    pub health_level: HealthLevel,
    pub backpressure_utilization: (u32, u32),
    pub recent_failures: u32,
}
```

### 4.6 New Event Variants — `cuervo-core/src/types/event.rs`

```rust
// Add to existing EventPayload enum:

EventPayload::CircuitBreakerTripped {
    provider: String,
    from_state: String,   // "closed", "open", "half_open"
    to_state: String,
},

EventPayload::HealthChanged {
    provider: String,
    old_score: u32,
    new_score: u32,
    level: String,        // "healthy", "degraded", "unhealthy"
},

EventPayload::BackpressureSaturated {
    provider: String,
    current: u32,
    max: u32,
},

EventPayload::ProviderFallback {
    from_provider: String,
    to_provider: String,
    reason: String,
},
```

---

## 5. Database Extension

### 5.1 Migration 006 — `resilience_events` Table

```sql
-- Migration 006: resilience_events
CREATE TABLE IF NOT EXISTS resilience_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    provider TEXT NOT NULL,
    event_type TEXT NOT NULL,
    from_state TEXT,
    to_state TEXT,
    score INTEGER,
    details TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_resilience_provider
    ON resilience_events(provider);
CREATE INDEX IF NOT EXISTS idx_resilience_type
    ON resilience_events(event_type);
CREATE INDEX IF NOT EXISTS idx_resilience_created
    ON resilience_events(created_at DESC);
```

**event_type values:** `breaker_trip`, `health_change`, `saturation`, `fallback`, `recovery`

### 5.2 New Database Methods — `cuervo-storage/src/resilience.rs`

```rust
impl Database {
    /// Insert a resilience event.
    pub fn insert_resilience_event(&self, event: &ResilienceEvent) -> Result<()>;

    /// Query recent resilience events for a provider.
    pub fn resilience_events(
        &self,
        provider: Option<&str>,
        event_type: Option<&str>,
        limit: u32,
    ) -> Result<Vec<ResilienceEvent>>;

    /// Provider-level metrics within a time window.
    pub fn provider_metrics_windowed(
        &self,
        provider: &str,
        window_minutes: u64,
    ) -> Result<ProviderWindowedMetrics>;

    /// Prune old resilience events.
    pub fn prune_resilience_events(&self, max_age_days: u32) -> Result<u64>;
}

pub struct ResilienceEvent {
    pub provider: String,
    pub event_type: String,
    pub from_state: Option<String>,
    pub to_state: Option<String>,
    pub score: Option<u32>,
    pub details: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub struct ProviderWindowedMetrics {
    pub provider: String,
    pub total_invocations: u64,
    pub successful_invocations: u64,
    pub failed_invocations: u64,
    pub timeout_count: u64,          // stop_reason = 'timeout'
    pub avg_latency_ms: f64,
    pub p95_latency_ms: u64,
    pub error_rate: f64,
    pub timeout_rate: f64,
}
```

### 5.3 Extend `InvocationMetric`

Add a new `stop_reason` value: `"timeout"` — recorded when provider.invoke() exceeds timeout. This enables distinguishing timeouts from other failures in health scoring.

---

## 6. Integration Points — Affected Files

### 6.1 Files Modified (existing)

| File | Changes |
|------|---------|
| `cuervo-core/src/types/config.rs` | Add `ResilienceConfig` + sub-configs, add `resilience` field to `AppConfig` |
| `cuervo-core/src/types/event.rs` | Add 4 new `EventPayload` variants |
| `cuervo-core/src/types/mod.rs` | Re-export new types if needed |
| `cuervo-storage/src/migrations.rs` | Add migration 006 |
| `cuervo-storage/src/sqlite.rs` | Add resilience event methods + windowed metrics query |
| `cuervo-storage/src/lib.rs` | Re-export new types |
| `cuervo-cli/src/repl/agent.rs` | Wrap `provider.invoke()` with ResilienceManager pre/post checks |
| `cuervo-cli/src/repl/mod.rs` | Initialize ResilienceManager, pass to agent loop |
| `cuervo-cli/src/commands/mod.rs` | Add `doctor` module |
| `cuervo-cli/src/main.rs` | Add `doctor` subcommand to CLI |

### 6.2 Files Created (new)

| File | Purpose |
|------|---------|
| `cuervo-cli/src/repl/circuit_breaker.rs` | `ProviderBreaker` state machine |
| `cuervo-cli/src/repl/health.rs` | `HealthScorer` composite scoring |
| `cuervo-cli/src/repl/backpressure.rs` | `BackpressureGuard` semaphore wrapper |
| `cuervo-cli/src/repl/resilience.rs` | `ResilienceManager` facade |
| `cuervo-cli/src/commands/doctor.rs` | `cuervo doctor` diagnostic command |
| `cuervo-storage/src/resilience.rs` | `ResilienceEvent` type + DB methods |

### 6.3 Agent Loop Wiring — `agent.rs` Changes

```
BEFORE (current):
  loop {
      cache_check()
      provider.invoke()        ← no protection
      collect_stream()
      record_metrics()
      if tool_use → execute_tools()
  }

AFTER (with resilience):
  loop {
      cache_check()

      // NEW: Pre-invoke resilience check
      let decision = resilience.pre_invoke(provider_name).await;
      match decision {
          Proceed { permit } => {
              // Invoke with timeout wrapper
              let result = tokio::time::timeout(
                  Duration::from_secs(invoke_timeout),
                  provider.invoke(&request)
              ).await;

              match result {
                  Ok(Ok(stream)) => {
                      resilience.record_success(provider_name);
                      // ... existing stream collection ...
                  }
                  Ok(Err(e)) => {
                      resilience.record_failure(provider_name);
                      // Try fallback
                  }
                  Err(_timeout) => {
                      resilience.record_failure(provider_name);
                      // Record as timeout, try fallback
                  }
              }
              drop(permit); // release backpressure
          }
          Fallback { reason } => {
              // Try cache, then alternate provider
              tracing::warn!(?reason, "Primary provider unavailable, trying fallback");
              // ... fallback logic ...
          }
          Exhausted { reason } => {
              // All providers exhausted
              return Err(anyhow!("All providers unavailable: {reason}"));
          }
      }

      record_metrics()
      if tool_use → execute_tools()
  }
```

---

## 7. Implementation Plan — Sub-Phases

### Sub-Phase 1: Circuit Breaker (foundation)

**Scope:** State machine + per-provider breaker + tests

**Files:**
- CREATE `cuervo-cli/src/repl/circuit_breaker.rs`
- MODIFY `cuervo-core/src/types/config.rs` — add `CircuitBreakerConfig`
- MODIFY `cuervo-cli/src/repl/mod.rs` — declare module

**Tests (6):**
1. `breaker_starts_closed` — initial state
2. `breaker_opens_after_threshold` — N failures → Open
3. `breaker_rejects_when_open` — returns BreakerOpen error
4. `breaker_transitions_to_half_open` — after cooldown
5. `half_open_success_closes` — probe success → Closed
6. `half_open_failure_reopens` — probe failure → Open
7. `failures_outside_window_dont_count` — sliding window prunes old failures

**Deliverable:** Standalone `ProviderBreaker` with deterministic state machine. Zero I/O.

---

### Sub-Phase 2: Health Scoring

**Scope:** Composite score from metrics DB + health levels

**Files:**
- CREATE `cuervo-cli/src/repl/health.rs`
- CREATE `cuervo-storage/src/resilience.rs` — `ProviderWindowedMetrics` + query
- MODIFY `cuervo-core/src/types/config.rs` — add `HealthConfig`
- MODIFY `cuervo-storage/src/sqlite.rs` — add `provider_metrics_windowed()`
- MODIFY `cuervo-storage/src/lib.rs` — re-exports

**Tests (5):**
1. `healthy_provider_scores_high` — 100% success, low latency → 80+
2. `failing_provider_scores_low` — 50% error rate → Unhealthy
3. `slow_provider_degrades_score` — high latency reduces score
4. `timeout_rate_impacts_score` — timeouts count heavily
5. `empty_metrics_returns_healthy` — no data = assume healthy (not punished)

**Deliverable:** `HealthScorer` that reads from DB, computes scores, returns `HealthReport`.

---

### Sub-Phase 3: Backpressure

**Scope:** Semaphore-based concurrency limiter per provider

**Files:**
- CREATE `cuervo-cli/src/repl/backpressure.rs`
- MODIFY `cuervo-core/src/types/config.rs` — add `BackpressureConfig`

**Tests (4):**
1. `acquire_within_limit_succeeds` — permits available
2. `acquire_at_limit_blocks` — waits for release
3. `try_acquire_at_limit_fails` — non-blocking rejection
4. `utilization_tracks_permits` — correct (acquired, max) tuple

**Deliverable:** `BackpressureGuard` with async acquire, try_acquire, and utilization reporting.

---

### Sub-Phase 4: Resilience Manager + Wiring

**Scope:** Facade integrating breaker + health + backpressure; wire into agent loop

**Files:**
- CREATE `cuervo-cli/src/repl/resilience.rs`
- MODIFY `cuervo-cli/src/repl/agent.rs` — wrap invoke with resilience checks
- MODIFY `cuervo-cli/src/repl/mod.rs` — initialize ResilienceManager
- MODIFY `cuervo-core/src/types/config.rs` — add `ResilienceConfig` to `AppConfig`

**Tests (5):**
1. `pre_invoke_succeeds_when_healthy` — happy path
2. `pre_invoke_fallback_on_breaker_open` — returns Fallback
3. `pre_invoke_fallback_on_unhealthy` — health < threshold
4. `pre_invoke_fallback_on_saturated` — backpressure full
5. `select_provider_prefers_healthy` — picks highest health from candidates

**Deliverable:** Fully wired resilience in agent loop. Opt-in via `resilience.enabled = true`.

---

### Sub-Phase 5: Metrics Extension + Persistence

**Scope:** Migration 006, resilience event persistence, new event types

**Files:**
- MODIFY `cuervo-storage/src/migrations.rs` — add migration 006
- MODIFY `cuervo-storage/src/sqlite.rs` — CRUD for resilience_events
- MODIFY `cuervo-storage/src/resilience.rs` — `ResilienceEvent` type
- MODIFY `cuervo-core/src/types/event.rs` — 4 new EventPayload variants
- MODIFY `cuervo-storage/src/lib.rs` — re-exports

**Tests (4):**
1. `insert_and_query_resilience_event` — round-trip
2. `query_filters_by_provider_and_type` — filtered queries
3. `prune_removes_old_events` — age-based cleanup
4. `windowed_metrics_computes_correctly` — aggregation within window

**Deliverable:** Persistent resilience event log with indexes and pruning.

---

### Sub-Phase 6: `cuervo doctor` Command

**Scope:** Diagnostic CLI command aggregating all resilience state

**Files:**
- CREATE `cuervo-cli/src/commands/doctor.rs`
- MODIFY `cuervo-cli/src/commands/mod.rs` — add doctor module
- MODIFY `cuervo-cli/src/main.rs` — add doctor subcommand

**Output format:**
```
╭─ Cuervo Doctor ─────────────────────────────────────────╮
│                                                          │
│  Providers                                               │
│  ┌──────────┬────────┬───────┬──────────┬─────────────┐ │
│  │ Provider │ Health │ Score │ Breaker  │ Utilization │  │
│  ├──────────┼────────┼───────┼──────────┼─────────────┤ │
│  │ anthropic│ ●      │ 92    │ Closed   │ 2/5         │  │
│  │ ollama   │ ◐      │ 61    │ Closed   │ 0/5         │  │
│  └──────────┴────────┴───────┴──────────┴─────────────┘ │
│                                                          │
│  Cache                                                   │
│  Entries: 47  │  Hit rate: 68%  │  Oldest: 2h ago       │
│                                                          │
│  Recent Activity (24h)                                   │
│  Invocations: 156  │  Success: 98.7%  │  Avg: 1.2s     │
│                                                          │
│  Recommendations                                         │
│  ⚠  ollama: Degraded health (score 61). Consider        │
│     checking connection or increasing timeout.           │
│                                                          │
╰──────────────────────────────────────────────────────────╯
```

**Tests (3):**
1. `doctor_runs_without_db` — graceful handling when no DB
2. `doctor_shows_provider_health` — with seeded metrics
3. `doctor_shows_cache_stats` — with seeded cache entries

**Deliverable:** User-facing diagnostic tool. Zero side effects (read-only).

---

## 8. Test Plan — 27 New Tests

| Sub-Phase | Tests | Running Total |
|-----------|-------|---------------|
| 1: Circuit Breaker | 7 | 449 |
| 2: Health Scoring | 5 | 454 |
| 3: Backpressure | 4 | 458 |
| 4: Resilience Manager | 5 | 463 |
| 5: Metrics Extension | 4 | 467 |
| 6: Doctor Command | 3 | 470 |

**Total: 442 (current) + 28 = ~470 tests**

### Chaos/Integration Tests (bonus, Sub-Phase 4)

- `resilience_disabled_bypasses_all_checks` — config.enabled = false
- `timeout_counts_as_failure` — invoke timeout → breaker failure
- `cache_fallback_on_provider_failure` — returns cached response when all providers fail

---

## 9. Success Metrics

| Metric | Target | How to Verify |
|--------|--------|---------------|
| Zero breaking changes | All 442 existing tests pass | `cargo test --workspace` |
| Opt-in activation | Default `resilience.enabled = false` | Config default test |
| Clippy clean | Zero warnings with `-D warnings` | `cargo clippy --workspace` |
| Test coverage | 27+ new tests | Test count |
| No new large crates | tokio+std only for concurrency | Cargo.toml audit |
| No panics | Zero `unwrap()` in resilience code | Clippy + code review |
| Deterministic breaker | State machine is time-controlled (Instant) | Unit tests with mock time |
| Performance neutral | < 1ms overhead per invoke check | Benchmark (manual) |

---

## 10. Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| `Instant` in tests is hard to mock | Flaky time-dependent tests | Use `Instant` offset arithmetic, not sleep. Tests compute expected transitions deterministically |
| Semaphore `acquire` can deadlock | Stuck agent loop | Always use `timeout(acquire())`, never bare `acquire()` |
| Health scorer DB queries add latency | Slower agent loop | Cache health scores for 30s (cheap in-memory TTL) |
| Too many resilience events fill DB | Storage bloat | Auto-prune events > 7 days (configurable) |
| HalfOpen probes may hit user-facing requests | User sees retry | Acceptable tradeoff; probe count is small (default 2) |
| Multiple concurrent agent loops share breakers | State conflicts | Single ResilienceManager per Repl (not global static) |

---

## 11. Dependency Graph

```
Sub-Phase 1 (Circuit Breaker)  ──┐
Sub-Phase 2 (Health Scoring)   ──┤
Sub-Phase 3 (Backpressure)     ──┼──▶ Sub-Phase 4 (Manager + Wiring) ──▶ Sub-Phase 6 (Doctor)
                                 │
                                 └──▶ Sub-Phase 5 (Metrics Extension)
```

- Phases 1, 2, 3 are **independent** (can be built in parallel)
- Phase 4 depends on 1+2+3 (integrates them)
- Phase 5 can proceed after 4 (persistence layer)
- Phase 6 depends on 4+5 (reads all state)

---

## 12. Constraints Checklist

- [x] No new large crates — only `tokio::sync::Semaphore` (already a dependency)
- [x] Zero breaking changes — `#[serde(default)]` on all new config fields
- [x] Opt-in config — `resilience.enabled = false` by default
- [x] Deterministic state machine — no random, no wall-clock in logic
- [x] No panic/unwrap — `thiserror` for errors, graceful fallbacks
- [x] Clippy clean target — `-D warnings` as CI gate
- [x] Existing patterns — follows cuervo-cli module structure (breaker.rs next to agent.rs)
- [x] In-memory breaker state — resets on restart (safe default, no stale state)
- [x] Fire-and-forget events — resilience events recorded async, never block agent loop
