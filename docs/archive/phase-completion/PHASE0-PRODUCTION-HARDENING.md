# FASE 0 — Production Hardening: Technical Diagnosis & Architecture

**Date:** 2026-02-07
**Scope:** Cuervo CLI runtime → industrial-grade production system
**Baseline:** 437 tests, 10MB binary, 9 crates, ~17K LoC Rust

---

## 1. TECHNICAL DIAGNOSIS — CURRENT STATE

### 1.1 Agent Loop (`crates/cuervo-cli/src/repl/agent.rs` — 876 lines)

**Strengths:**
- Clean `tokio::select!` cancellation (Ctrl+C races against `stream.next()`)
- Fire-and-forget trace recording (never blocks main loop)
- Stop conditions as enum (EndTurn, MaxRounds, TokenBudget, DurationBudget, Interrupted, ProviderError)
- Token/duration budget guards checked after each round
- Tool timeout via `tokio::time::timeout()`

**Weaknesses:**
- **Sequential tool execution only** — `executor.rs` has parallel infrastructure but agent.rs doesn't use it
- **No response cache integration** — `response_cache.rs` fully implemented but never called from agent loop
- **No metrics persistence** — `ModelInvoked` event emitted but `db.insert_metric()` never called
- **No speculative routing integration** — `speculative.rs` exists but agent.rs calls `provider.invoke()` directly
- **No retry on stream error** — single chunk error cancels entire model round
- **stdin.lock() in async context** — permission check blocks tokio worker thread

### 1.2 Tool Executor (`crates/cuervo-cli/src/repl/executor.rs` — 621 lines)

**Strengths:**
- `plan_execution()` correctly partitions ReadOnly (parallel) vs ReadWrite/Destructive (sequential)
- `execute_parallel_batch()` uses `futures::future::join_all` — correct pattern
- Per-tool timeout enforcement
- Trace recording for parallel batches
- Results sorted by tool_use_id for deterministic ordering

**Weaknesses:**
- **Not wired into agent.rs** — parallel executor exists but unused in actual agent loop
- **No tool retry on transient failure** — single attempt per tool
- **No tool cancellation** — timed-out tool processes may linger
- **No concurrent tool limit** — unbounded `join_all` could spawn 50+ parallel reads
- **No backpressure** — tools execute as fast as they arrive

### 1.3 Sandbox (`crates/cuervo-tools/src/sandbox.rs`)

**Strengths:**
- rlimits (CPU + FSIZE) via `pre_exec` — correct Unix pattern
- Best-effort application (ignores setrlimit errors)
- Output truncation: 60% head + 30% tail + marker

**Weaknesses:**
- **No RLIMIT_AS (memory)** — skipped for macOS compat
- **No network isolation** — subprocess can make arbitrary network calls
- **No filesystem isolation** — only path_security (allowlist/blocklist)
- **No cgroup/namespace support** — Linux-only opportunity missed
- **No process tree tracking** — spawned subprocesses not monitored

### 1.4 Router & Speculative Invoker

**Router** (`router.rs` — 218 lines): Sequential retry + fallback.
**Speculative** (`speculative.rs` — 291 lines): `futures::select_ok` racing.

**Weaknesses:**
- **No circuit breaker** — retries dead providers until max_retries exhausted
- **No health tracking** — no per-provider state (up/down/degraded)
- **No backoff between retries** — router retries immediately (provider-level backoff only)
- **speculation_providers config field unused** — all fallbacks raced, not filtered
- **Not integrated into agent loop** — agent.rs bypasses both router and speculative invoker

### 1.5 Cache (`response_cache.rs` — 238 lines + `cuervo-storage/src/cache.rs`)

**Strengths:**
- SHA-256 semantic key (model + system_prompt + last 3 messages + tool_names)
- TTL expiry + max_entries pruning
- Correctly skips caching `stop_reason=ToolUse`

**Weaknesses:**
- **NOT INTEGRATED** — response cache never called from agent loop
- **No L1 in-memory layer** — every lookup hits SQLite
- **No cache warming** — cold start on every session
- **No invalidation on context change** — system prompt change silently returns stale data
- **No auto-pruning** — `prune_cache()` requires manual call
- **3-message window** — semantic collision risk for similar conversations

### 1.6 Storage (`cuervo-storage/src/sqlite.rs` — 1,173 lines)

**Strengths:**
- WAL mode enabled
- 5 clean migrations (sessions, audit, trace, cache, metrics)
- Hash-chained audit trail (SOC 2 ready)
- FTS5 BM25 search with Porter stemming

**Weaknesses:**
- **Single `Mutex<Connection>`** — no connection pooling, serialized access
- **All sync I/O** — rusqlite is blocking, no `spawn_blocking()` wrappers
- **Blocks tokio workers** — DB operations hold worker threads during queries
- **No prepared statement caching** — statements recompiled per call
- **No WAL checkpoint control** — grows unbounded
- **No vacuum/optimize schedule** — DB fragmentation over time

### 1.7 Tracing & Observability

**Strengths:**
- `#[instrument]` on agent loop + round spans + tool_exec spans
- Trace recording to SQLite (append-only steps)
- JSON export (`TraceExport`)
- DomainEvent system via `tokio::broadcast`

**Weaknesses:**
- **Only 3 spans in entire codebase** — agent_loop, agent_round, tool_exec
- **No per-phase latency breakdown** — can't see time in context assembly vs model vs tools
- **No flamegraph export** — no Chrome trace format
- **No trace verification** — no `cuervo trace verify` command
- **No replay determinism guarantee** — timestamps are real-time, not virtual
- **No metrics export** — no Prometheus/OpenTelemetry integration
- **Event subscribers not persisting** — broadcast events are fire-and-forget

### 1.8 Core Types & Traits (`cuervo-core/`)

**Strengths:**
- 8 well-defined traits (ModelProvider, Tool, ContextSource, Planner, etc.)
- Zero I/O in core crate — clean domain boundary
- Comprehensive error enum (26 variants)
- Contract tests validating all trait implementations

**Weaknesses:**
- **No trait versioning** — adding methods is breaking change
- **No `#[non_exhaustive]` on enums** — EventPayload, StopReason, etc.
- **No middleware/decorator pattern** — can't wrap Tool for metrics/logging
- **No plugin loading** — tools/providers hardcoded at compile time

### 1.9 MCP Runtime (`cuervo-mcp/`)

**Weaknesses:**
- **No timeout on `receive()`** — can block indefinitely on hung server
- **No resource quotas** — MCP servers unlimited CPU/memory
- **All MCP tools default to Destructive** — overly conservative
- **stderr discarded** — diagnostic info lost
- **No heartbeat/keep-alive** — can't detect dead servers

### 1.10 Test Infrastructure

- **437 tests** — all unit tests, no integration/E2E/stress/chaos/benchmark
- **No CI pipeline** — no `.github/workflows/` directory
- **No benchmarks** — no criterion, no regression detection
- **No coverage tracking** — no lcov/grcov/tarpaulin
- **No mock server** — Anthropic tests use EchoProvider only
- **No property-based tests** — no proptest/quickcheck

---

## 2. COMPARISON WITH MODERN RUNTIMES

| Capability | Claude Code | Copilot Runtime | Cuervo (current) | Gap |
|---|---|---|---|---|
| Circuit breakers | Yes (per-provider) | Yes (with Hystrix-like) | **No** | Critical |
| L1+L2 cache | Memory + disk | Redis + disk | **Disk only** | High |
| Parallel tool exec | Yes (read-only) | Yes (all safe) | **Implemented but unused** | High |
| Trace replay verify | Deterministic | Deterministic | **No verification** | High |
| Flamegraph export | Chrome trace | OTLP | **None** | Medium |
| Connection pooling | Yes (r2d2/deadpool) | Yes (pgbouncer) | **Single mutex** | High |
| Async DB bridge | Yes (spawn_blocking) | Yes (sqlx async) | **Direct sync calls** | High |
| Benchmark suite | criterion + regression | Custom harness | **None** | High |
| CI pipeline | GitHub Actions | GitHub Actions | **None** | Critical |
| Plugin SDK | Dynamic (WASM/FFI) | Plugin manifest | **Compile-time only** | Medium |
| Health checks | Per-component | Full readiness | **None** | High |
| Resource limits | Per-tool + global | Global + per-tenant | **Per-tool rlimits only** | Medium |
| Chaos testing | Failure injection | Chaos monkey | **None** | High |
| Backpressure | Token bucket | Queue-based | **None** | Medium |
| Auto-pruning | Background task | Cron-based | **Manual only** | Medium |

---

## 3. REAL GAPS (PRIORITIZED)

### P0 — Must fix (blocks production use)
1. **No CI pipeline** — zero automated quality gates
2. **Response cache not integrated** — implemented but dead code
3. **Metrics not persisted** — optimizer can't learn
4. **Parallel executor not integrated** — tools always sequential
5. **Sync DB calls blocking tokio** — thread pool starvation risk

### P1 — Should fix (reliability/performance)
6. **No circuit breakers** — retries dead providers
7. **No L1 memory cache** — every lookup hits disk
8. **No connection pooling** — single mutex serializes everything
9. **No benchmark suite** — can't detect regressions
10. **No trace verification** — can't prove determinism

### P2 — Nice to have (production maturity)
11. **No flamegraph/Chrome trace export** — debugging blind spots
12. **No health check command** — `cuervo doctor`
13. **No plugin architecture** — compile-time only tools
14. **No chaos tests** — untested failure paths
15. **No auto-pruning** — unbounded DB growth

---

## 4. PROPOSED ARCHITECTURE

### 4.1 New Module: `cuervo-runtime` (extracted from cuervo-cli)

Extract the runtime engine from the CLI binary into a reusable library crate:

```
crates/
├── cuervo-runtime/        # NEW: Extracted runtime engine
│   ├── agent_loop.rs      # Agent loop (from cli/repl/agent.rs)
│   ├── executor.rs        # Parallel executor (from cli/repl/executor.rs)
│   ├── cache.rs           # Multi-layer cache (L1+L2)
│   ├── circuit_breaker.rs # Per-provider circuit breakers
│   ├── router.rs          # Model routing + speculative (merged)
│   ├── optimizer.rs       # Cost/latency optimizer
│   ├── profiler.rs        # Per-phase latency profiling
│   ├── health.rs          # Component health checks
│   └── chaos.rs           # Failure injection (test only)
```

### 4.2 Multi-Layer Cache Architecture

```
Request → L1 (LRU in-memory, ~100 entries, sub-ms)
       → L2 (SQLite, ~1000 entries, <10ms)
       → Provider (network, 200-5000ms)

On response:
  Store in L2 (SQLite)
  Promote to L1 (LRU)

Invalidation:
  Hash-based (context change → different key)
  TTL (configurable per entry)
  Event-based (config change → flush L1)
```

### 4.3 Circuit Breaker Per Provider

```
States: Closed → Open → HalfOpen → Closed

Closed:
  Track: failure_count, last_failure_time
  Threshold: 3 consecutive failures → Open

Open:
  All requests immediately fail (no network call)
  Timeout: 30s → transition to HalfOpen

HalfOpen:
  Allow 1 probe request
  Success → Closed (reset counters)
  Failure → Open (restart timeout)
```

### 4.4 Profiling Architecture

```
AgentLoop
├── [span] context_assembly     ← NEW: time spent building system prompt
│   ├── instruction_source
│   ├── planning_source
│   └── memory_source
├── [span] cache_lookup          ← NEW: L1+L2 cache check
├── [span] model_invocation      ← EXISTS: provider.invoke()
│   ├── routing_decision
│   └── stream_processing
├── [span] tool_execution        ← EXISTS: tool_exec
│   ├── permission_check
│   ├── parallel_batch
│   └── sequential_batch
└── [span] post_processing       ← NEW: metrics, trace, session save

Export formats:
  - JSON (structured)
  - Chrome trace (chrome://tracing compatible)
  - Summary table (CLI output)
```

### 4.5 Deterministic Execution Model

```
Trace Recording:
  - All RNG seeds captured (global seed at session start)
  - Virtual timestamps (monotonic counter, not wall clock)
  - Request/response hashes (SHA-256 of model I/O)
  - Tool I/O hashes

Trace Verification:
  cuervo trace verify <session-id>

  1. Load recorded trace
  2. Replay with recorded seeds + mocked provider responses
  3. Compare tool call sequence + arguments
  4. Report divergences with diff

Determinism guarantees:
  - Parallel tool results sorted by tool_use_id (already done)
  - RNG seeded from session ID (new)
  - Timestamps virtual in replay mode (new)
```

### 4.6 Self-Healing & Resilience

```
Tool Recovery:
  - Retry transient failures (network errors, timeouts) up to 2x
  - Kill lingering processes on timeout (SIGKILL after SIGTERM)
  - Isolate crashed tools (don't poison other tools)

Provider Degradation:
  - Circuit breaker triggers → automatic failover
  - Optimizer downgrades slow providers in real-time
  - Cached responses served during provider outage

Backpressure:
  - Max concurrent tool executions: 8 (configurable)
  - Tool execution queue with priority (read > write > destructive)
  - Memory limit on accumulated tool output
```

---

## 5. NEW MODULES & TRAITS

### 5.1 New Traits

```rust
/// Circuit breaker for provider health tracking
pub trait HealthTracker: Send + Sync {
    fn record_success(&self, provider: &str);
    fn record_failure(&self, provider: &str, error: &CuervoError);
    fn is_available(&self, provider: &str) -> bool;
    fn state(&self, provider: &str) -> CircuitState;
}

/// Multi-layer cache with L1+L2
pub trait ResponseCacheLayer: Send + Sync {
    fn lookup(&self, key: &str) -> Option<CacheEntry>;
    fn store(&self, key: &str, entry: CacheEntry);
    fn invalidate(&self, key: &str);
    fn stats(&self) -> CacheLayerStats;
}

/// Profiler for per-phase latency tracking
pub trait PhaseProfiler: Send + Sync {
    fn start_phase(&self, name: &str) -> PhaseHandle;
    fn end_phase(&self, handle: PhaseHandle);
    fn export_chrome_trace(&self) -> serde_json::Value;
    fn summary(&self) -> ProfileSummary;
}

/// Benchmark harness trait
pub trait Benchmark: Send + Sync {
    fn name(&self) -> &str;
    fn setup(&mut self) -> Result<()>;
    fn run(&self) -> Result<BenchmarkResult>;
    fn teardown(&mut self) -> Result<()>;
}
```

### 5.2 New Structs

```rust
// Circuit breaker state machine
pub struct CircuitBreaker {
    state: AtomicU8,  // Closed=0, Open=1, HalfOpen=2
    failure_count: AtomicU32,
    last_failure: Mutex<Option<Instant>>,
    config: CircuitBreakerConfig,
}

// L1 in-memory LRU cache
pub struct MemoryCache {
    entries: Mutex<lru::LruCache<String, CacheEntry>>,
    hits: AtomicU64,
    misses: AtomicU64,
}

// Multi-layer cache coordinator
pub struct LayeredCache {
    l1: MemoryCache,
    l2: Arc<Database>,
    config: CacheConfig,
}

// Per-phase profiler
pub struct RuntimeProfiler {
    phases: Mutex<Vec<PhaseRecord>>,
    session_start: Instant,
}

// Async database wrapper
pub struct AsyncDatabase {
    inner: Arc<Database>,
    pool: Arc<tokio::runtime::Handle>,
}

// Health checker
pub struct HealthChecker {
    providers: Vec<Arc<dyn ModelProvider>>,
    db: Option<Arc<Database>>,
    tools: ToolRegistry,
}
```

---

## 6. IMPACT BY FILE

### Modified Files

| File | Changes | Risk |
|---|---|---|
| `cuervo-cli/src/repl/agent.rs` | Wire cache, parallel executor, router, profiler, metrics persistence | **High** — core loop |
| `cuervo-cli/src/repl/mod.rs` | Add LayeredCache, CircuitBreaker, Profiler initialization | **Medium** |
| `cuervo-cli/src/repl/executor.rs` | Add concurrency limit (semaphore), tool retry | **Medium** |
| `cuervo-cli/src/repl/router.rs` | Add circuit breaker integration | **Medium** |
| `cuervo-cli/src/repl/response_cache.rs` | Refactor to use LayeredCache trait | **Low** |
| `cuervo-cli/src/repl/optimizer.rs` | Add real-time metric ingestion | **Low** |
| `cuervo-storage/src/sqlite.rs` | Add `spawn_blocking` wrappers, prepared stmt cache | **High** — all callers |
| `cuervo-tools/src/bash.rs` | Add process tree kill on timeout | **Medium** |
| `cuervo-core/src/types/config.rs` | Add CircuitBreakerConfig, ProfilerConfig, BenchConfig | **Low** |
| `cuervo-core/src/traits/mod.rs` | Add HealthTracker, PhaseProfiler traits | **Low** |
| `cuervo-mcp/src/transport.rs` | Add receive timeout | **Low** |

### New Files

| File | Purpose |
|---|---|
| `cuervo-cli/src/repl/circuit_breaker.rs` | CircuitBreaker state machine |
| `cuervo-cli/src/repl/layered_cache.rs` | L1+L2 cache coordinator |
| `cuervo-cli/src/repl/profiler.rs` | Per-phase latency profiling + Chrome trace export |
| `cuervo-cli/src/repl/health.rs` | `cuervo doctor` health checks |
| `cuervo-cli/src/commands/bench.rs` | `cuervo bench` command |
| `cuervo-cli/src/commands/profile.rs` | `cuervo profile` command |
| `cuervo-cli/src/commands/trace_verify.rs` | `cuervo trace verify` command |
| `cuervo-cli/src/commands/doctor.rs` | `cuervo doctor` command |
| `cuervo-cli/src/commands/stats.rs` | `cuervo stats` command |
| `cuervo-cli/src/commands/explain.rs` | `cuervo explain` command |
| `cuervo-storage/src/async_db.rs` | Async wrapper over Database |
| `benches/agent_loop.rs` | Criterion benchmark: agent loop throughput |
| `benches/cache.rs` | Criterion benchmark: cache L1/L2 latency |
| `benches/tool_execution.rs` | Criterion benchmark: tool dispatch |
| `tests/chaos/` | Chaos test suite (timeouts, failures, OOM) |
| `.github/workflows/ci.yml` | CI pipeline (build + test + clippy + bench) |

---

## 7. RISKS

| Risk | Impact | Mitigation |
|---|---|---|
| Agent loop refactor breaks tool execution | **High** | Run all 437 tests after each change; add integration test covering full loop |
| `spawn_blocking` migration changes error types | **Medium** | Wrapper returns same `Result<T>` type; JoinError → Internal |
| L1 cache causes stale responses | **Medium** | Hash-based invalidation + short TTL (60s) for L1 |
| Circuit breaker false positives | **Medium** | Conservative threshold (5 failures, 60s open) + manual override |
| Benchmark noise in CI | **Low** | Use `criterion --significance-level 0.01`; compare only against main branch |
| New dependencies increase binary size | **Low** | `lru` is 50KB; no other new deps needed |
| Parallel executor introduces race conditions | **Medium** | ReadOnly tools only; sorted results; deterministic ordering |

---

## 8. INCREMENTAL IMPLEMENTATION PLAN

### Phase 1: Wire Existing Infrastructure (1-2 days)
> Goal: Activate dead code. Zero new logic. Immediate value.

1. **Wire response cache** into agent loop (before `provider.invoke()`)
2. **Wire parallel executor** into agent loop (replace sequential tool loop)
3. **Wire metrics persistence** (call `db.insert_metric()` after each model invocation)
4. **Wire speculative invoker** (replace direct `provider.invoke()`)
5. **Add `spawn_blocking`** wrapper for all DB calls from async contexts

**Tests:** Extend existing agent loop tests to verify cache hit/miss, parallel execution, metric recording.
**Target:** All 437 existing tests pass + 15-20 new integration tests.

### Phase 2: Circuit Breaker + Health (1 day)
> Goal: Prevent cascading failures.

1. **Implement `CircuitBreaker`** state machine (Closed/Open/HalfOpen)
2. **Integrate with router** — check `is_available()` before invoke
3. **Implement `cuervo doctor`** — check provider availability, DB health, tool registry
4. **Add health state to `cuervo stats`** output

**Tests:** Unit tests for state transitions + integration test with mock failing provider.
**Target:** +20 tests.

### Phase 3: Layered Cache (1 day)
> Goal: Sub-millisecond cache hits.

1. **Implement `MemoryCache`** (LRU, 100 entries, Mutex-protected)
2. **Implement `LayeredCache`** coordinator (L1 → L2 → miss)
3. **Add auto-pruning** on insert (enforce max_entries)
4. **Add cache stats** to `cuervo stats` output
5. **Add L1 invalidation** on config change event

**Tests:** Unit tests for L1/L2 promotion, eviction, invalidation + benchmark.
**Target:** +15 tests.

### Phase 4: Profiling & Observability (1-2 days)
> Goal: See where time goes.

1. **Implement `RuntimeProfiler`** — phase recording with start/end handles
2. **Add spans** for context_assembly, cache_lookup, post_processing
3. **Implement Chrome trace export** (`cuervo profile <session-id>`)
4. **Add per-phase summary** to session end output
5. **Wire profiler** into agent loop at each phase boundary

**Tests:** Unit tests for profiler + integration test verifying span hierarchy.
**Target:** +12 tests.

### Phase 5: Deterministic Replay Verification (1 day)
> Goal: Prove trace + inputs = same output.

1. **Add RNG seed** to session (derived from session UUID)
2. **Add request/response hashing** to trace steps
3. **Implement `cuervo trace verify`** command
4. **Add virtual timestamps** in replay mode
5. **Report divergences** with structured diff

**Tests:** Replay test with known trace → verify no divergence.
**Target:** +10 tests.

### Phase 6: Benchmark & Regression Harness (1 day)
> Goal: Measurable performance baseline.

1. **Add `criterion` dependency** and benchmark scaffolding
2. **Benchmark: cache L1 vs L2 latency** (target: L1 < 1ms, L2 < 10ms)
3. **Benchmark: agent loop throughput** (echo provider, 10 rounds)
4. **Benchmark: tool dispatch latency** (parallel vs sequential)
5. **Implement `cuervo bench`** and `cuervo bench compare`
6. **Add to CI** with regression detection

**Tests:** Criterion benchmarks (not counted as unit tests).
**Target:** 3 benchmark suites.

### Phase 7: CI Pipeline (0.5 days)
> Goal: Automated quality gates.

1. **Create `.github/workflows/ci.yml`**
   - `cargo build --release`
   - `cargo test --workspace`
   - `cargo clippy --workspace -- -D warnings`
   - `cargo bench` (criterion, compare against main)
2. **Add Makefile** with common commands (`make test`, `make bench`, `make lint`)
3. **Add pre-commit hook** (clippy + test)

### Phase 8: Resilience & Chaos (1 day)
> Goal: Never panic, never block.

1. **Add tool retry** — transient failures retried 1x with backoff
2. **Add process tree kill** — SIGTERM → 5s → SIGKILL for timed-out tools
3. **Add concurrency limiter** — `tokio::sync::Semaphore(8)` for parallel tools
4. **Add MCP receive timeout** — 30s default
5. **Chaos test suite:**
   - Mock provider that fails on Nth request
   - Mock provider with 10s latency
   - Mock tool that panics
   - Mock tool that writes 1GB output
   - Concurrent 50-tool batch

**Tests:** Chaos tests verifying graceful degradation.
**Target:** +15 chaos tests.

### Phase 9: DX Commands (0.5 days)
> Goal: Actionable debugging.

1. **`cuervo doctor`** — provider connectivity, DB health, tool availability, MCP server status
2. **`cuervo stats`** — session count, cache hit rate, model usage, cost breakdown
3. **`cuervo explain <session-id>`** — why model/tool was chosen, routing decision, cache hit/miss
4. **Improve error messages** — include actionable suggestions

**Tests:** Unit tests for each command's output format.
**Target:** +8 tests.

---

## 9. TEST PLAN

### Test Categories

| Category | Current | Target | Description |
|---|---|---|---|
| Unit | 437 | 520+ | Existing + new module tests |
| Integration | 0 | 30+ | Full agent loop with mock providers |
| Chaos | 0 | 15+ | Failure injection, timeouts, OOM |
| Benchmark | 0 | 3 suites | criterion: cache, agent loop, tool dispatch |
| Contract | ~20 | 30+ | Trait compliance (existing + new traits) |
| Regression | 0 | Automated | Benchmark comparison in CI |

### Key Integration Tests to Add

```
test_agent_loop_with_cache_hit
test_agent_loop_with_cache_miss_then_store
test_agent_loop_parallel_readonly_tools
test_agent_loop_sequential_destructive_tools
test_agent_loop_mixed_parallel_sequential
test_circuit_breaker_opens_on_failure
test_circuit_breaker_recovers
test_layered_cache_l1_promotion
test_layered_cache_l2_fallback
test_profiler_phase_hierarchy
test_trace_verify_deterministic
test_trace_verify_divergence_detected
test_doctor_all_healthy
test_doctor_provider_down
test_concurrent_tool_limit_enforced
test_tool_timeout_kills_process
test_mcp_receive_timeout
test_metrics_persisted_after_invocation
test_optimizer_uses_real_metrics
test_resilience_provider_crash_midstream
```

### Chaos Tests

```
chaos_provider_fails_every_3rd_request
chaos_provider_hangs_10_seconds
chaos_tool_writes_1gb_output
chaos_tool_spawns_fork_bomb (sandboxed)
chaos_50_concurrent_readonly_tools
chaos_mcp_server_dies_midcall
chaos_db_locked_during_write
chaos_ctrl_c_during_tool_execution
```

---

## 10. MEASURABLE SUCCESS METRICS

| Metric | Current | Target | How to Measure |
|---|---|---|---|
| Test count | 437 | 550+ | `cargo test --workspace 2>&1 \| grep "test result"` |
| Cache hit latency (L1) | N/A (unused) | < 1ms p99 | criterion benchmark |
| Cache hit latency (L2) | N/A (unused) | < 10ms p99 | criterion benchmark |
| Cache miss → provider | N/A | baseline measured | criterion benchmark |
| Parallel tool speedup (5 reads) | 1x (sequential) | 3-5x | benchmark: parallel vs sequential |
| Agent loop overhead per round | unmeasured | < 5ms | profiler: non-model time |
| Circuit breaker failover time | N/A | < 100ms | integration test |
| Trace verify accuracy | N/A | 100% on EchoProvider | replay test |
| CI pipeline time | N/A | < 3 min | GitHub Actions metric |
| Binary size | 10 MB | < 11 MB | `ls -la target/release/cuervo` |
| clippy warnings | 0 | 0 | `cargo clippy --workspace -- -D warnings` |
| Benchmark regression threshold | N/A | < 5% | criterion significance test |
| Max concurrent tool safety | unbounded | 8 (configurable) | chaos test: 50 tools |
| Provider failure recovery | manual | < 60s automatic | circuit breaker test |
| DB call blocking time | unmeasured | 0ms on tokio worker | spawn_blocking migration |

---

## SUMMARY

The Cuervo runtime is **functionally complete** but has **significant infrastructure gaps** that separate it from production-grade systems:

1. **Dead code** — Cache, parallel executor, speculative invoker, and optimizer are implemented but not wired into the agent loop
2. **No resilience** — No circuit breakers, no retry, no health tracking
3. **No observability depth** — Only 3 spans, no profiling, no Chrome trace
4. **No CI** — Zero automated quality gates
5. **Blocking async** — Sync SQLite calls on tokio worker threads
6. **No benchmarks** — Can't measure or detect regressions

The proposed 9-phase plan addresses all gaps incrementally, with each phase delivering measurable value:

- **Phase 1** (wire existing code) delivers the highest ROI — activates ~1,200 lines of dormant code
- **Phases 2-3** (circuit breaker + cache) deliver production reliability
- **Phases 4-5** (profiling + deterministic replay) deliver debugging power
- **Phases 6-7** (benchmarks + CI) deliver engineering discipline
- **Phases 8-9** (chaos + DX) deliver confidence and usability

**Estimated total: 8-10 implementation days → 550+ tests, sub-ms cache, measurable everything.**
