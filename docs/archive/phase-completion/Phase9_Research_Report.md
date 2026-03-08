# Phase 9 — Research Report: Performance, Architecture & Scalability

**Date**: 2026-02-07
**Baseline**: 685 tests | 28,451 LOC | 107 files | 5.1MB binary | 9 crates | clippy clean

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Quantitative Benchmarks](#2-quantitative-benchmarks)
3. [Hot Path Analysis](#3-hot-path-analysis)
4. [Storage Layer Analysis](#4-storage-layer-analysis)
5. [Architectural Risk Assessment](#5-architectural-risk-assessment)
6. [State-of-the-Art Comparison](#6-state-of-the-art-comparison)
7. [Codebase Metrics](#7-codebase-metrics)
8. [Prioritized Improvement Plan](#8-prioritized-improvement-plan)

---

## 1. Executive Summary

Phase 9 Research analyzed the cuervo-cli codebase across six dimensions: hot path performance, storage layer efficiency, coupling/scaling risks, modern agent architecture patterns, quantitative benchmarks, and codebase metrics. The system is functionally complete through Phase 8 (675 core tests, all sub-phases done) with a resilient multi-provider agent loop, hybrid memory retrieval, adaptive planning, TBAC authorization, reflexion self-improvement, and episodic memory.

**Key Findings**:

| Dimension | Status | Top Issue |
|-----------|--------|-----------|
| Hot paths | 86 critical paths identified | 5 mutex locks + 12-16 spawn_blocking per round |
| Storage | Functional, N+1 patterns | system_metrics() executes 36 queries (should be 1) |
| Architecture | Monolithic Repl orchestrator | 18 fields, growing coupling risk |
| Memory model | BM25+RRF works | Missing tiered memory (Working/Semantic/Procedural) |
| Binary | 5.1MB (excellent) | 27s release compile (acceptable) |
| Tests | 685 passing | 1 test per 39 LOC density (healthy) |

**Critical path for Phase 9 Design**: Address the N+1 query patterns, decompose the Repl monolith, parallelize context assembly, and implement tiered memory — in that priority order.

---

## 2. Quantitative Benchmarks

All benchmarks collected from `crates/cuervo-storage/tests/perf_measurements.rs` (10 tests, 743 lines). Environment: macOS arm64, SQLite WAL mode, busy_timeout=5000, synchronous=NORMAL.

### 2.1 Memory Insertion Throughput

| Scale | Ops/sec | Notes |
|-------|---------|-------|
| 1K entries | 5,582 | Single-threaded, with FTS5 triggers |
| 5K entries | 2,841 | ~2x degradation due to index growth |
| 10K entries | 1,346 | FTS5 write amplification ~2x |

**Assessment**: Adequate for CLI workloads (typically <1K entries per session). FTS5 write amplification is the primary cost — acceptable tradeoff for full-text search capability.

### 2.2 FTS5 Search Latency (10K entries)

| Percentile | Latency |
|------------|---------|
| p50 | 10.3 ms |
| p95 | 32.4 ms |
| p99 | 43.4 ms |

**Assessment**: p50 acceptable for interactive use. p99 at 43ms may cause perceptible lag in rapid-fire queries. Optimization opportunity: pre-warm FTS5 index on session start.

### 2.3 Embedding Search (Brute-Force Cosine)

| Scale | p50 | Algorithm |
|-------|-----|-----------|
| 1K entries (384-dim) | 25.0 ms | Full-scan cosine similarity in Rust |

**Assessment**: Brute-force cosine is acceptable for <10K entries. Beyond that, consider approximate nearest neighbor (ANN) via sqlite-vss or hnswlib-rs. Current implementation is a pragmatic choice.

### 2.4 Episode CRUD

| Operation | Latency |
|-----------|---------|
| save_episode | 193 µs |
| link_entry_to_episode | 95 µs |
| load_episode_entries | 121 µs |

**Assessment**: All sub-millisecond. No optimization needed.

### 2.5 Session Save/Load

| Message Count | Save | Load |
|---------------|------|------|
| 0 messages | 196 µs | 142 µs |
| 10 messages | 402 µs | 287 µs |
| 100 messages | 1.8 ms | 1.2 ms |
| 1000 messages | 6.0 ms | 4.1 ms |

**Assessment**: Linear scaling. Even at 1000 messages (long session), save is <10ms. The auto-save-per-round pattern (firing every round) adds negligible overhead.

### 2.6 Response Cache

| Metric | Value |
|--------|-------|
| L1 hit | <1 µs |
| L2 hit (p50) | 67 µs |
| Cache miss (p50) | 17 µs |
| Throughput | 13,818 ops/sec |

**Assessment**: L1 cache eliminates spawn_blocking overhead for hot entries. L2 hit at 67µs is dominated by spawn_blocking scheduling, not SQLite query time. Cache is well-optimized.

### 2.7 Aggregate Query Performance

| Query | p50 | Issue |
|-------|-----|-------|
| system_metrics() | 16.3 ms | **N+1 pattern: 36 queries** |
| model_stats() | 5.5 ms | Acceptable |
| cache_stats() | ~3 ms | 4 full scans (could be 1) |
| memory_stats() | ~3 ms | 4 full scans (could be 1) |
| filtered memory list | 9.0 ms | vs 315 µs unfiltered |

**Assessment**: system_metrics() is the worst offender — 36 individual queries where a single GROUP BY would suffice. This is the #1 storage optimization target.

### 2.8 Concurrent Access

| Tasks | Speedup | Pattern |
|-------|---------|---------|
| 1 task | 1.0x | Baseline |
| 4 tasks | 1.63x | tokio spawn_blocking |

**Assessment**: WAL mode enables concurrent reads. Speedup is limited by spawn_blocking thread pool size and Mutex contention on L1 cache. Switching L1 to tokio::sync::RwLock could improve read-heavy concurrent access.

---

## 3. Hot Path Analysis

### 3.1 Per-Round Resource Consumption

The agent loop (`agent.rs:run_agent_loop`) is the primary hot path. Per round:

| Resource | Non-Tool Round | Tool Round (N tools) |
|----------|---------------|----------------------|
| spawn_blocking calls | 9-11 | 15-17 + N |
| SQL queries | 6-8 | 10 + 2N |
| Mutex locks | 5 | 5-7 |
| Heap allocation | ~100 KB | ~200-500 KB |
| Async context switches | 12-18 | 25-40 |

### 3.2 Breakdown by Component

| Component | Latency Range | spawn_blocking | SQL Queries |
|-----------|---------------|----------------|-------------|
| L1 cache lookup | <1 µs | 0 | 0 |
| L2 cache lookup | 2-5 ms | 1 | 1-3 |
| Context assembly | 20-50 ms | 3-5 | 3-5 |
| Provider invocation | 200-3000 ms | 0 (async) | 0 |
| Tool execution | 10-5000 ms | 1-N | 0-N |
| Metrics persistence | 2-5 ms | 1 | 1 |
| Session auto-save | 1-6 ms | 1 | 1 |
| Resilience pre/post | 1-3 ms | 0-1 | 0-1 |
| Compaction (if triggered) | 5-30 s | 2-4 | 2-4 |

### 3.3 Critical Hotspots

1. **Context assembly is sequential**: 5-7 ContextSources gathered one-by-one via async loop. Could be parallel with `futures::join_all`.

2. **ChatMessage cloning**: Full message history cloned per round for context assembly and cache key computation. For 100+ messages, this dominates heap allocation.

3. **spawn_blocking saturation**: At 15-17 spawn_blocking calls per tool round, the default tokio blocking thread pool (512 threads max, but typically ~4 warm) experiences scheduling latency. A dedicated DB thread pool would reduce contention.

4. **Compaction inline blocking**: When triggered, context compaction (5-30s LLM call) blocks the agent loop. Should be moved to background task.

5. **Mutex<LruCache> for L1**: std::sync::Mutex is non-async — holding it across await points is safe but suboptimal. Under concurrent access, this is a bottleneck.

---

## 4. Storage Layer Analysis

### 4.1 Query Efficiency

| Pattern | Location | Current | Optimal | Impact |
|---------|----------|---------|---------|--------|
| N+1 system_metrics() | sqlite.rs:1476-1533 | 36 queries | 1 (GROUP BY) | **HIGH** — called per doctor + advisory |
| P95 via OFFSET | sqlite.rs:1421-1428 | O(N log N) | O(1) PERCENTILE_CONT | **MEDIUM** — affects rankings |
| 3 queries per cache hit | sqlite.rs:808-852 | SELECT+DELETE+UPDATE | 1-2 (upsert) | **MEDIUM** — per cache lookup |
| 4 scans in cache_stats() | sqlite.rs:913-963 | 4 COUNT(*) | 1 compound | **LOW** — called infrequently |
| 4 scans in memory_stats() | sqlite.rs:719-775 | 4 COUNT(*) | 1 compound | **LOW** — called infrequently |

### 4.2 Index Coverage

28 indexes across 10 tables cover all primary access patterns. No missing indexes detected for current query patterns. Index overhead on writes is acceptable given the read-heavy workload.

### 4.3 Migration Health

10 migrations (001-010), all forward-only ALTER TABLE ADD COLUMN with DEFAULT values. Backward compatible. No destructive migrations. Schema is clean.

### 4.4 FTS5 Configuration

- `content=memory_entries, content_rowid=id` with 3 sync triggers (ai, ad, au)
- Write amplification ~2x (acceptable)
- No tokenizer customization — default unicode61 tokenizer
- Potential improvement: porter stemmer for better recall

### 4.5 Storage Scaling Projections

| Scale | DB Size | FTS5 Search | Embedding Search | Session Load |
|-------|---------|-------------|------------------|-------------|
| 1K entries | ~2 MB | <15 ms | <30 ms | <1 ms |
| 10K entries | ~20 MB | 10-45 ms | ~250 ms | <5 ms |
| 100K entries | ~200 MB | 50-200 ms | **2.5 s** (unacceptable) | <10 ms |
| 1M entries | ~2 GB | 200-800 ms | **25 s** (broken) | ~50 ms |

**Verdict**: Current architecture scales to ~10K entries comfortably. Beyond that, embedding search needs ANN indexing, and FTS5 may need partitioning or archival.

---

## 5. Architectural Risk Assessment

### 5.1 Risk Matrix

| # | Risk | Severity | Likelihood | Mitigation Effort |
|---|------|----------|------------|-------------------|
| 1 | Repl monolithic orchestrator (18 fields) | CRITICAL | HIGH | LARGE — requires decomposition |
| 2 | Session double-save overhead | HIGH | MEDIUM | SMALL — deduplicate save points |
| 3 | Inline compaction blocking (5-30s) | HIGH | MEDIUM | MEDIUM — background task |
| 4 | L1 cache Mutex contention | MEDIUM | LOW | SMALL — switch to RwLock |
| 5 | Context budget unfairness | MEDIUM | MEDIUM | MEDIUM — priority-weighted allocation |
| 6 | Event bus unused in production | MEDIUM | LOW | SMALL — remove or wire |
| 7 | Tool execution sequential for mixed ops | MEDIUM | MEDIUM | SMALL — already partitioned |
| 8 | Memory FTS5 write overhead | LOW | LOW | N/A — acceptable tradeoff |
| 9 | Config deep cloning | LOW | LOW | SMALL — Arc<AppConfig> |
| 10 | Provider registry linear scan | LOW | LOW | SMALL — HashMap lookup |

### 5.2 Risk #1 Detail: Repl Monolith

The `Repl` struct (mod.rs) holds 18 fields including config, provider, registry, tools, resilience, cache, async_db, event_bus, system_prompt, context_sources, session, compactor, and agent context. This creates:

- **High coupling**: Changes to any subsystem require understanding the full Repl
- **Testing difficulty**: Integration tests must construct full Repl
- **Concurrency limits**: Single Repl instance processes one message at a time
- **Memory footprint**: All subsystems live for entire session lifetime

**Recommended decomposition**:
- `AgentOrchestrator`: agent loop, tool execution, context assembly
- `SessionManager`: session CRUD, auto-save, resume
- `InfrastructureLayer`: cache, metrics, resilience, event bus
- `ProviderRouter`: provider selection, fallback, speculative invocation

### 5.3 Risk #3 Detail: Compaction Blocking

When context exceeds budget, `ContextCompactor::compact()` makes a synchronous LLM call (5-30s) inline in the agent loop. During this time:
- User sees no output
- No cancellation path
- Provider timeout may cascade

**Recommended fix**: Move compaction to a background tokio task. Use a `watch` channel to signal compaction completion. Fall back to truncation if compaction is in-progress.

---

## 6. State-of-the-Art Comparison

### 6.1 Multi-Agent Frameworks (2026)

| Framework | Memory Model | Coordination | Sandboxing | Relevance |
|-----------|-------------|--------------|------------|-----------|
| AutoGen 0.4 | Shared/private stores | Agent teams + orchestrator | Docker containers | HIGH — team pattern |
| CrewAI 0.80 | Shared crew memory | Sequential/hierarchical | Process isolation | MEDIUM — role-based |
| LangGraph | State graph + checkpoints | Graph edges + conditions | None built-in | HIGH — graph execution |
| Semantic Kernel | Plugin-based memory | Planner + stepwise | Azure sandboxes | MEDIUM — enterprise |

### 6.2 Memory Architecture Patterns

Modern agent memory systems use 4 tiers:

| Tier | Description | Cuervo Status | Gap |
|------|-------------|---------------|-----|
| **Working** | Current conversation context | Partial (message history) | No structured scratchpad |
| **Episodic** | Past session experiences | **Done** (Phase 8.5) | Missing cross-session linking |
| **Semantic** | Factual knowledge graph | Partial (flat key-value) | No entity relationships |
| **Procedural** | Learned tool/workflow patterns | **Missing** | No tool sequence learning |

**A-MEM (Zettelkasten-inspired)**: Each memory note has: content, keywords, connections to other notes, summary. Auto-generates connections via LLM. Enables associative retrieval beyond keyword matching.

**LightMem**: Two-phase memory — generative (profile, event, knowledge summaries) and retrieval (ranked by task relevance). Claims 10.9% accuracy improvement and 117x token reduction vs raw history.

### 6.3 Observability Standards

**OpenTelemetry GenAI Semantic Conventions** (2026):
- `gen_ai.agent.create`: Agent initialization span
- `gen_ai.agent.invoke`: Per-invocation span with input/output tokens
- `gen_ai.tool.call`: Tool execution span
- Attributes: `gen_ai.system`, `gen_ai.request.model`, `gen_ai.usage.input_tokens`, `gen_ai.usage.output_tokens`

Cuervo currently uses `tracing` with custom spans. Migrating to OTel semantic conventions would enable interoperability with LangSmith, Langfuse, Phoenix, and other agent observability platforms.

### 6.4 Sandboxing Evolution

| Approach | Isolation Level | Startup Time | Cuervo Relevance |
|----------|----------------|--------------|------------------|
| rlimits (current) | Process-level | 0 ms | **In use** — basic but sufficient |
| seccomp-bpf | Syscall filtering | 0 ms | HIGH — Linux only |
| WASI | Module-level | <10 ms | MEDIUM — portable, emerging |
| gVisor | Kernel-level | ~100 ms | LOW — server workloads |
| Firecracker | VM-level | ~125 ms | LOW — cloud workloads |

Current rlimits (CPU + FSIZE) are adequate for CLI use. WASI would be the natural next step for portable sandboxing if tool execution needs stronger isolation.

### 6.5 Gap Analysis vs State-of-Art

| Capability | State-of-Art | Cuervo Current | Gap Size |
|------------|-------------|----------------|----------|
| Multi-agent coordination | Team/graph-based | Single agent | LARGE |
| Memory tiers | 4-tier (W/E/S/P) | 2-tier (Working + Episodic) | MEDIUM |
| Tool sequence learning | Procedural memory | None | MEDIUM |
| Observability | OTel GenAI spans | Custom tracing | SMALL |
| Sandboxing | WASI / containers | rlimits | SMALL |
| Context management | Sliding window + compression | Compaction + truncation | SMALL |
| Cost optimization | Per-token routing | Per-invocation routing | SMALL |

---

## 7. Codebase Metrics

### 7.1 Size & Structure

| Metric | Value |
|--------|-------|
| Total LOC | 28,451 |
| Source LOC (non-blank, non-comment) | 26,694 |
| Source files | 107 |
| Crates | 9 |
| Tests | 685 |
| Test density | 1 test per 39 LOC |
| Binary size (release) | 5.1 MB |
| Release compile time | 27.17 s |
| Dependencies (Cargo.lock) | 369 entries |

### 7.2 Crate Distribution

| Crate | Source LOC | % Total | Tests | Largest File |
|-------|-----------|---------|-------|-------------|
| cuervo-cli | 12,491 | 47% | 327 | agent.rs (~1,200 lines) |
| cuervo-storage | 4,823 | 18% | 97 | sqlite.rs (3,043 lines) |
| cuervo-providers | 3,117 | 12% | 68 | anthropic/mod.rs (~800 lines) |
| cuervo-tools | 2,843 | 11% | 72 | bash.rs (~600 lines) |
| cuervo-core | 2,186 | 8% | 30 | types/config.rs (~500 lines) |
| cuervo-context | 1,124 | 4% | 25 | assembler.rs (~400 lines) |
| cuervo-security | 612 | 2% | 22 | pii.rs (~300 lines) |
| cuervo-mcp | 589 | 2% | 22 | host.rs (~250 lines) |
| cuervo-auth | 666 | 2% | 22 | device_flow.rs (~300 lines) |

### 7.3 Complexity Hotspots

| File | Lines | Concern |
|------|-------|---------|
| sqlite.rs | 3,043 | God file — CRUD for 10+ entity types |
| agent.rs | ~1,200 | Agent loop, tool execution, resilience wiring |
| mod.rs (repl) | ~800 | Repl orchestrator, session management |
| anthropic/mod.rs | ~800 | SSE parsing, streaming, auth |

**sqlite.rs decomposition** is the highest-value refactoring target. Natural split: `session_repo.rs`, `cache_repo.rs`, `memory_repo.rs`, `metrics_repo.rs`, `migration_runner.rs`.

### 7.4 Public API Surface

| Category | Count |
|----------|-------|
| Sync functions | 231 |
| Async functions | 49 |
| Total public | 280 |
| Traits | 12 |
| Structs (public) | ~85 |

---

## 8. Prioritized Improvement Plan

### Priority 1: Storage Optimization (HIGH impact, LOW effort)

| # | Change | Impact | Effort | Benchmark Target |
|---|--------|--------|--------|-----------------|
| 1.1 | Consolidate system_metrics() to 1 GROUP BY query | Reduce 36→1 queries | 2h | p50 < 2ms (from 16.3ms) |
| 1.2 | Consolidate cache_stats() to 1 compound query | Reduce 4→1 scans | 1h | p50 < 1ms |
| 1.3 | Consolidate memory_stats() to 1 compound query | Reduce 4→1 scans | 1h | p50 < 1ms |
| 1.4 | Optimize cache hit to 1-2 queries (upsert) | Reduce 3→1-2 queries | 1h | p50 < 50µs |
| 1.5 | Replace P95 OFFSET with window function | O(N log N) → O(N) | 1h | Constant time |

### Priority 2: Hot Path Optimization (HIGH impact, MEDIUM effort)

| # | Change | Impact | Effort |
|---|--------|--------|--------|
| 2.1 | Parallelize context assembly (join_all on ContextSources) | 20-50ms → 10-20ms | 4h |
| 2.2 | Arc<Vec<ChatMessage>> CoW for message history | Eliminate per-round cloning | 4h |
| 2.3 | Move compaction to background task | Unblock agent loop during 5-30s compaction | 6h |
| 2.4 | Switch L1 cache from Mutex to tokio::sync::RwLock | Better concurrent read throughput | 2h |

### Priority 3: Architectural Decomposition (CRITICAL impact, LARGE effort)

| # | Change | Impact | Effort |
|---|--------|--------|--------|
| 3.1 | Split sqlite.rs into domain-specific repo modules | Maintainability, testability | 8h |
| 3.2 | Extract SessionManager from Repl | Reduce coupling, enable concurrent sessions | 12h |
| 3.3 | Extract InfrastructureLayer (cache+metrics+resilience) | Clean separation of concerns | 8h |
| 3.4 | Extract AgentOrchestrator from Repl | Single-responsibility agent loop | 12h |

### Priority 4: Memory Model Enhancement (MEDIUM impact, LARGE effort)

| # | Change | Impact | Effort |
|---|--------|--------|--------|
| 4.1 | Implement Working Memory scratchpad | Structured intermediate state | 8h |
| 4.2 | Add Semantic Memory with entity relationships | Knowledge graph for facts | 16h |
| 4.3 | Add Procedural Memory for tool patterns | Learn common workflows | 16h |
| 4.4 | Cross-session episodic linking | Multi-session context | 8h |

### Priority 5: Observability & Standards (LOW impact, SMALL effort)

| # | Change | Impact | Effort |
|---|--------|--------|--------|
| 5.1 | Adopt OTel GenAI span conventions | Interoperability | 4h |
| 5.2 | Wire event bus to production (or remove) | Clean architecture | 2h |
| 5.3 | Add structured logging for agent decisions | Debuggability | 4h |

### Recommended Phase 9 Execution Order

```
Stage 2 (Design):  RFC for priorities 1-3 (storage, hot paths, decomposition)
Stage 3 (Dev):     Priority 1 (storage) → Priority 2 (hot paths) → Priority 3 (architecture)
Stage 4 (Testing): Regression + benchmark validation against targets
Stage 5 (Tuning):  Priority 4-5 (memory model, observability)
```

---

## Appendix A: Benchmark Test Suite

Location: `crates/cuervo-storage/tests/perf_measurements.rs` (743 lines, 10 tests)

Tests:
1. `bench_memory_insertion_throughput` — 1K/5K/10K entry insertion rates
2. `bench_fts5_search_latency` — p50/p95/p99 over 10K entries
3. `bench_embedding_storage_and_search` — brute-force cosine, 384-dim, 1K entries
4. `bench_episode_crud` — save/link/load cycle timing
5. `bench_session_save_load` — 0/10/100/1000 message sessions
6. `bench_cache_operations` — hit/miss/throughput
7. `bench_concurrent_access` — 1/4 task parallelism
8. `bench_metric_aggregation` — system_metrics() + model_stats()
9. `bench_db_growth` — storage bytes per 1K entries
10. `bench_memory_list_with_filter` — filtered vs unfiltered

## Appendix B: Methodology

- **Hot path analysis**: Static analysis of agent.rs + dynamic tracing of spawn_blocking/SQL calls per round
- **Storage benchmarks**: In-memory SQLite with WAL mode, 100 iterations per measurement, p50/p95/p99 via sorted array
- **Architecture assessment**: Manual code review + dependency graph analysis
- **State-of-art comparison**: Web search for 2026 agent frameworks, memory patterns, OTel conventions
- **Codebase metrics**: tokei for LOC, cargo-bloat for binary analysis, Cargo.lock for dependency count
