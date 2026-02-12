# Phase 7: Production-Grade & State-of-the-Art Extension — Research Report

**Date**: February 7, 2026
**Status**: Phase 1 Research Complete
**Baseline**: 553 tests, 5.0MB binary, 23,134 LOC, 97 Rust files, 9 crates

---

## 1. CODEBASE ARCHITECTURE ANALYSIS

### 1.1 Crate Dependency Graph

```
cuervo-core (ZERO I/O — domain types + traits)
  ↑ depended on by ALL other crates

cuervo-providers → cuervo-core (reqwest, eventsource-stream)
cuervo-tools     → cuervo-core (git2, regex, glob, libc)
cuervo-auth      → cuervo-core (keyring, reqwest, sha2, rand)
cuervo-storage   → cuervo-core (rusqlite, tokio, sha2, uuid)
cuervo-security  → cuervo-core (regex)
cuervo-context   → cuervo-core (tokio)
cuervo-mcp       → cuervo-core (tokio, serde_json)
cuervo-cli       → ALL 8 crates (clap, reedline, crossterm, termimad, syntect)
```

**Heaviest dependencies**: git2, rusqlite(bundled), syntect, reqwest — these dominate binary size.

### 1.2 Module Size Distribution (REPL — 7,661 LOC across 17 files)

| Module | LOC | Complexity |
|--------|-----|-----------|
| agent.rs | 1,593 | HIGH — main agent loop, invoke_with_fallback |
| mod.rs | 1,110 | HIGH — REPL lifecycle, session management |
| executor.rs | 631 | MEDIUM — tool execution, permissions |
| circuit_breaker.rs | 603 | MEDIUM — state machine, exponential backoff |
| resilience.rs | 589 | MEDIUM — facade coordinating breaker+backpressure |
| response_cache.rs | 552 | MEDIUM — L1/L2 two-tier cache |
| permissions.rs | 319 | LOW — permission checking |
| accumulator.rs | 294 | LOW — stream accumulation |
| speculative.rs | 289 | MEDIUM — speculative routing |
| backpressure.rs | 262 | LOW — semaphore guard |
| health.rs | 256 | LOW — health scoring |
| optimizer.rs | 231 | LOW — cost/latency optimization |
| commands.rs | 221 | LOW — REPL commands |
| router.rs | 216 | LOW — model routing |
| memory_source.rs | 216 | LOW — memory context source |
| planning_source.rs | 153 | LOW — plan-execute prompt |
| prompt.rs | 126 | LOW — prompt construction |

### 1.3 Async Patterns

**Strengths**:
- All blocking DB work in `spawn_blocking` (AsyncDatabase pattern)
- No `block_on()` calls anywhere
- No `std::sync::Mutex` held across `.await` points
- `tokio::broadcast` for events (fire-and-forget, no deadlock risk)
- Permission I/O via `spawn_blocking` (stdin/stderr)

**Risks**:
- `McpHost` wrapped in `Arc<tokio::sync::Mutex>` — lock held across async tool calls (acceptable for small volumes)
- No structured concurrency (no task groups/nurseries pattern)
- No cancellation tokens for graceful shutdown of agent loop sub-tasks

### 1.4 Error Handling

**CuervoError**: 26 variants across 7 categories. Well-structured with `thiserror`.

**Production unwrap() risk**: Only 1 identified —
`mcp/host.rs:94`: `self.server_info.as_ref().unwrap()` — can panic if `initialize()` not called before use.

**All `panic!()` calls (25 total)**: Confirmed test-only. No production panics.

### 1.5 Database Layer

- 7 migrations, 19 tables, 11 indexes
- All queries use parameterized statements — zero SQL injection risk
- PRAGMAs: WAL, busy_timeout=5000, synchronous=NORMAL, foreign_keys=ON
- FTS5 with BM25 for semantic memory search
- AsyncDatabase wraps 11 methods via spawn_blocking

---

## 2. TEST COVERAGE AUDIT

### 2.1 Coverage Statistics

| Metric | Value |
|--------|-------|
| Total .rs files | 97 |
| Files with #[cfg(test)] | 62 (64%) |
| Files WITHOUT tests | 35 (36%) |
| Total tests | 553 |

### 2.2 Untested Modules (Critical Gaps)

| File | LOC | Risk | Notes |
|------|-----|------|-------|
| repl/prompt.rs | 126 | LOW | Prompt construction |
| render/markdown.rs | ~150 | LOW | Markdown rendering |
| render/syntax.rs | ~100 | LOW | Syntax highlighting |
| context/instruction_source.rs | ~200 | MEDIUM | Instruction file loading |
| tools/path_security.rs | ~80 | HIGH | Path traversal prevention |
| mcp/transport.rs | 150 | MEDIUM | Stdio transport (partial tests) |
| commands/config.rs | ~100 | LOW | Config CLI handlers |
| commands/session.rs | ~100 | LOW | Session CLI handlers |

### 2.3 Error Handling Audit

| Pattern | Count | Location |
|---------|-------|----------|
| `unwrap()` in non-test | ~5 | http.rs:22 (expect, safe), pii.rs:17 (expect, safe), mcp/host.rs:94 (UNSAFE) |
| `expect()` in non-test | ~3 | Hardcoded valid configs — safe |
| `todo!()` | 0 | None |
| `unimplemented!()` | 0 | None |
| `panic!()` in non-test | 0 | All 25 are test-only |

### 2.4 Dead Code Annotations

~19 intentional `#[allow(dead_code)]` remain — mostly on fields/variants used in future phases or for API completeness.

### 2.5 Technical Debt Markers

No `TODO`, `FIXME`, `HACK`, or `XXX` comments found in production code.

---

## 3. STATE-OF-THE-ART RESEARCH

### 3.1 LLM Agent Architecture (2025-2026)

**Industry patterns from Claude Code, aider, cursor, goose**:
- **ReAct loop**: Reason-Act-Observe cycle (Cuervo has this in agent.rs)
- **Multi-agent orchestration**: Specialist agents with handoff policies
- **Structured output**: JSON mode, tool-use schemas
- **Chain-of-thought routing**: Route to different models based on task complexity

**tower-llm crate** (Rust, crates.io): Implements LLM agents as composable Tower services with middleware layers for resilience, tracing, metrics, memory, and parallel tool execution. Highly relevant architecture reference.

**Recommendation**: Consider tower-llm's layer/service pattern for cross-cutting concerns (tracing, retry, timeout) instead of inline code in agent loop.

### 3.2 Observability for LLM Applications

**Industry standard**: OpenTelemetry spans for agent loops, with token usage as span attributes.

**Tools**: LangSmith, Braintrust, Helicone — all use structured trace formats with:
- Per-turn latency + token counts
- Tool invocation timing
- Cost attribution per model/provider
- Session-level aggregation

**Current Cuervo state**: Has trace_steps table + invocation_metrics + resilience_events — good foundation. Missing: OpenTelemetry export, structured spans, trace visualization.

**Recommendation**: Add OpenTelemetry exporter as optional feature. Current tracing crate integrates natively with opentelemetry-rust.

### 3.3 Memory & RAG Patterns

**State-of-the-art**:
- **Sliding window + landmark**: Keep recent context + important moments
- **Conversation summarization**: Compress old context into summaries
- **Semantic memory hierarchies**: Short-term (session) → Long-term (cross-session) → Persistent (facts)
- **Vector embeddings**: For semantic similarity search beyond BM25

**Current Cuervo state**: FTS5/BM25 memory with session-scoped + persistent entries. Good for exact keyword match, weak for semantic similarity.

**Recommendation**: Add optional vector embedding support (local model or API) for semantic search. Keep BM25 as fast fallback.

### 3.4 Security — OWASP LLM Top 10 (2025)

| # | Risk | Cuervo Status |
|---|------|--------------|
| LLM01 | Prompt Injection | PARTIAL — PII detection, but no input sanitization for prompt injection |
| LLM02 | Sensitive Info Disclosure | GOOD — PII regex detection (7 patterns), key redaction |
| LLM03 | Supply Chain | N/A — Cuervo is the tool, not consuming untrusted plugins |
| LLM05 | Improper Output Handling | PARTIAL — Tool output truncation, but no output sanitization |
| LLM06 | Excessive Agency | GOOD — Permission levels (ReadOnly/ReadWrite/Destructive) |
| LLM07 | System Prompt Leakage | MEDIUM — System prompts stored in session JSON |
| LLM08 | Vector/Embedding Weaknesses | N/A — No RAG vectors yet |
| LLM09 | Misinformation | N/A — Tool-based, not free-form generation |
| LLM10 | Unbounded Consumption | PARTIAL — No max token budget per session |

**Key gaps**:
1. No prompt injection defense beyond PII detection
2. No session-level cost budget / token limit
3. PII detection misses SSH keys, JWT tokens, private keys
4. No audit trail linking permission approvals to specific tool invocations

### 3.5 Sandbox Hardening

**Current**: rlimits (CPU + FSIZE) via pre_exec — basic process-level limits.

**Industry best practice (2025-2026)**:
- **Container isolation**: Docker/gVisor/Firecracker for strong process isolation
- **Filesystem restrictions**: Read-only root, explicit writable paths
- **Network restrictions**: No network access for sandboxed commands by default
- **Seccomp/AppArmor**: Syscall filtering
- **Defense in depth**: Sandbox + monitoring + human-in-the-loop + signed artifacts

**Recommendation**: Add optional container-based sandbox mode (Docker if available, fallback to rlimits). Add network restriction flag.

### 3.6 Performance & Binary Optimization

**Current**: 5.0MB with opt-level="z", lto="fat", codegen-units=1, panic="abort", strip=true.

**Additional techniques**:
- **PGO (Profile-Guided Optimization)**: 10-15% runtime speedup via cargo-pgo. Requires training profile.
- **BOLT (Binary Optimization and Layout Tool)**: Additional 2-5% on top of PGO
- **Feature gating**: Optional features for heavy deps (syntect, git2) to reduce base binary
- **Dynamic linking**: Not recommended for CLI tools (portability)

**Recommendation**: PGO is high-ROI for startup time. Feature gating syntect/git2 could reduce binary to ~3MB for minimal installs.

### 3.7 Configuration & Plugin Systems

**MCP protocol**: Rapidly becoming standard for LLM tool extensibility. Cuervo already has MCP support — good position.

**Industry patterns**:
- JSON Schema validation for config files
- Plugin discovery via MCP servers
- Hot-reload of config changes
- Config profiles (dev/staging/prod)

**Current Cuervo state**: Manual TOML validation (validate_config), MCP host+bridge. Missing: hot-reload, config profiles.

---

## 4. GAP ANALYSIS & PRIORITIZED OPPORTUNITIES

### 4.1 Critical Gaps (Must Fix)

| Gap | Impact | Effort | Priority |
|-----|--------|--------|----------|
| `mcp/host.rs:94` unwrap | Production panic risk | XS | P0 |
| No session cost budget | Unbounded API spend | S | P0 |
| PII misses private keys | Security hole | S | P0 |
| path_security.rs untested | Security bypass risk | S | P0 |

### 4.2 High-Value Improvements

| Improvement | Impact | Effort | Priority |
|-------------|--------|--------|----------|
| OpenTelemetry export | Observability for production | M | P1 |
| Conversation summarization | Long sessions don't exceed context | M | P1 |
| Token budget per session | Cost control | S | P1 |
| Enhanced PII patterns | +SSH, JWT, private keys | S | P1 |
| Prompt injection detection | OWASP LLM01 mitigation | M | P1 |
| Container sandbox mode | Stronger tool isolation | L | P1 |

### 4.3 Competitive Differentiators

| Feature | Impact | Effort | Priority |
|---------|--------|--------|----------|
| PGO build pipeline | Faster startup | M | P2 |
| Vector embedding memory | Semantic search beyond BM25 | L | P2 |
| Multi-agent orchestration | Complex task decomposition | L | P2 |
| Config hot-reload | DX improvement | S | P2 |
| Streaming WebSocket transport | Lower latency | M | P3 |

### 4.4 Technical Debt

| Debt | Impact | Effort |
|------|--------|--------|
| 35 untested files | Regression risk | M |
| 19 dead_code annotations | Code hygiene | XS |
| agent.rs 1,593 LOC | Maintenance difficulty | M (refactor into modules) |
| Duplicate health scoring (doctor assess_sync) | Divergence risk | S |

---

## 5. PROPOSED PHASE 7 SCOPE

Based on research, the highest-impact work items for "production-grade 2026-2027" are:

### Tier 1: Security & Safety (Must-Have)
1. Fix mcp/host.rs unwrap → Result
2. Session-level token/cost budget with configurable limits
3. Enhanced PII patterns (+SSH keys, JWT, private keys, certificates)
4. Prompt injection detection layer (input sanitization)
5. path_security.rs test coverage

### Tier 2: Observability & Reliability
6. OpenTelemetry trace export (optional feature flag)
7. Conversation context summarization (prevent context overflow)
8. Agent loop refactor (extract agent.rs into sub-modules)
9. Structured error context (error chains with tracing spans)

### Tier 3: Performance & DX
10. PGO build pipeline (cargo-pgo integration in CI)
11. Feature-gated heavy deps (syntect, git2 optional)
12. Config hot-reload via file watcher
13. Enhanced test coverage for remaining 35 untested files

---

## 6. ARCHITECTURE RECOMMENDATIONS

### 6.1 Agent Loop Refactoring

Current `agent.rs` (1,593 LOC) should be split into:
- `agent/loop.rs` — Main ReAct loop
- `agent/invoke.rs` — Provider invocation + fallback
- `agent/tools.rs` — Tool execution orchestration
- `agent/cache.rs` — Cache lookup/store logic
- `agent/metrics.rs` — Metric recording

### 6.2 Tower-Inspired Middleware

Consider extracting cross-cutting concerns from the agent loop into composable middleware:
- `RetryLayer` — Already have backoff logic, formalize it
- `TracingLayer` — Span creation per turn
- `MetricsLayer` — Token/cost recording
- `CacheLayer` — Response cache check/store
- `ResilientLayer` — Circuit breaker + backpressure

This is a larger refactor but would make the architecture more testable and extensible.

### 6.3 Memory Architecture

```
┌─────────────────────────────────────┐
│ L0: Working Memory (current session) │ ← messages Vec
├─────────────────────────────────────┤
│ L1: Session Summary (compressed)     │ ← conversation summarization
├─────────────────────────────────────┤
│ L2: Semantic Memory (BM25/FTS5)      │ ← current memory_entries
├─────────────────────────────────────┤
│ L3: Vector Memory (embeddings)       │ ← future: optional vector store
└─────────────────────────────────────┘
```

---

## 7. TRADE-OFF ANALYSIS

### 7.1 tower-llm Adoption vs. Custom Architecture

| Factor | tower-llm | Current Custom |
|--------|-----------|----------------|
| Composability | Excellent (Service+Layer) | Good (but inline) |
| Learning curve | High (tower traits) | Low (already built) |
| Binary size | +200KB (tower dep) | No change |
| Flexibility | Constrained by tower API | Unlimited |
| Testing | Excellent (mock services) | Good (mock providers) |

**Decision**: Don't adopt tower-llm as dependency. Instead, adopt its architectural patterns (service/layer abstraction) as internal refactoring. Avoids external dependency while gaining composability.

### 7.2 Vector Embeddings: Local vs. API

| Factor | Local (e.g., ort/candle) | API (provider endpoint) |
|--------|--------------------------|------------------------|
| Latency | ~10ms | 50-200ms |
| Binary size | +10-50MB | No change |
| Offline support | Yes | No |
| Quality | Good (all-MiniLM-L6-v2) | Excellent (ada-002+) |
| Complexity | High (model management) | Low (HTTP call) |

**Decision**: Defer vector embeddings to P2. BM25/FTS5 covers 80% of use cases. When implemented, prefer API-based embeddings with local fallback.

### 7.3 Container Sandbox vs. rlimits Enhancement

| Factor | Container (Docker/gVisor) | Enhanced rlimits |
|--------|--------------------------|-----------------|
| Isolation | Strong (kernel-level) | Weak (process-level) |
| Availability | Requires Docker | Always available |
| Performance | ~50ms overhead per invocation | Zero overhead |
| Complexity | High (container lifecycle) | Low |
| macOS support | Docker Desktop required | Native |

**Decision**: Keep rlimits as default. Add optional container mode when Docker is available. Detect Docker presence at startup.

---

*Report compiled from: deep codebase analysis (97 files), web research (OWASP, tower-llm, cargo-pgo, LLM security best practices), and test coverage audit.*
