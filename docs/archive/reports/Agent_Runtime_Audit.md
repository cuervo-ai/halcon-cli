# Agent Runtime Architecture Audit

**Date:** 2026-02-08
**Auditor:** Automated deep-analysis (4 parallel research agents)
**Scope:** Full cuervo-cli codebase — 701 tests, 9 crates, ~30k LOC, 5.1MB binary
**Status:** Phase 10 UX complete, pre-Phase 11 (Agent Runtime Hardening)

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Runtime Execution Model](#2-runtime-execution-model)
3. [Observability & Tracing](#3-observability--tracing)
4. [Safety & Security](#4-safety--security)
5. [User Experience Layer](#5-user-experience-layer)
6. [Consolidated Issue Registry](#6-consolidated-issue-registry)
7. [Recommendations](#7-recommendations)

---

## 1. Executive Summary

Cuervo CLI's agent runtime is architecturally sound with strong foundations: round-based execution with budget enforcement, multi-provider resilience, TBAC authorization, hash-chained audit trail, and NO_COLOR-aware UX. However, critical gaps exist in four areas:

| Area | Critical | Major | Minor | Total |
|------|----------|-------|-------|-------|
| Runtime | 4 | 7 | 5 | 16 |
| Observability | 6 | 14 | 5 | 25 |
| Safety | 4 | 6 | 6 | 16 |
| UX | 3 | 5 | 6 | 14 |
| **Total** | **17** | **32** | **22** | **71** |

**Top 5 Critical Findings:**
1. Provider invocation has no timeout — process hangs on network failure
2. Tool results not sanitized — prompt injection via tool output
3. 13 of 21 domain events defined but never emitted — 38% audit coverage
4. Tool execution metrics not persisted — cannot identify slow/failing tools
5. Default budget is unlimited (0) — unbounded API spend without opt-in limits

---

## 2. Runtime Execution Model

### 2.1 Agent Loop Architecture

**File:** `crates/cuervo-cli/src/repl/agent.rs` (1,886 lines)

The agent loop is a **sequential state machine with explicit rounds**:

```
for round in 0..limits.max_rounds {
    Build ModelRequest (messages, tools, system prompt)
    → Check pre-invocation guardrails
    → Attempt response cache lookup
    → If miss: invoke provider with resilience routing
    → Stream response via tokio::select! { chunk, ctrl_c }
    → Accumulate text + tool uses
    → Track usage (tokens, cost, latency)
    → Check post-invocation guardrails
    → Check token/duration budgets
    → If ToolUse: execute tools, append results, continue
    → If EndTurn: append message, break
}
```

**Stop conditions** (checked in order):
1. `EndTurn` — provider signals completion (line 872)
2. `TokenBudget` — cumulative tokens >= limit (line 807)
3. `DurationBudget` — elapsed wall-clock >= limit (line 840)
4. `GuardrailBlock` — pre/post guardrail blocks (lines 475, 764)
5. `MaxRounds` — loop exhausts max_rounds (line 1134, default 25)
6. `ProviderError` — invoke_with_fallback returns Err (line 611)
7. `Interrupted` — Ctrl+C via tokio::signal (line 585)

**Determinism:** Mostly deterministic with two non-determinism sources:
- Parallel tool execution uses `futures::join_all()` — results sorted by tool_use_id for deterministic output order, but execution timing varies
- Speculative routing uses `futures::select_ok()` — first provider to respond wins

### 2.2 Concurrency Model

| Primitive | Location | Purpose |
|-----------|----------|---------|
| `tokio::select!` | agent.rs:548 | Stream polling + Ctrl+C cancellation |
| `tokio::time::timeout` | agent.rs:359 | 15s compaction timeout |
| `futures::join_all` | executor.rs:194 | Parallel tool execution |
| `futures::select_ok` | speculative.rs | Speculative provider racing |
| `tokio::sync::Semaphore` | backpressure.rs | Per-provider permit limiting |
| `tokio::broadcast` | cuervo-core | Domain event bus (256 buffer) |
| `spawn_blocking` | executor.rs:345 | stdin/stderr permission I/O |

**Blocking I/O points:** Permission prompts (spawn_blocking), all DB operations (AsyncDatabase wrapper).

### 2.3 Error Handling

**Provider errors:** invoke_with_fallback Err → stop spinner → log → record failed metric → user_error() → return ProviderError. No retry on promoted-fallback failure.

**Tool errors:** Wrapped in ToolResult with `is_error: true` → appended to messages → triggers reflexion (if enabled) → triggers adaptive replanning (first failure only, once per round).

**DB errors:** Fire-and-forget with tracing::warn!. Failures never propagate or affect loop execution.

**Panics in production:** None found. All unwrap()/expect() confined to test code or safe contexts.

### 2.4 Memory & Token Management

**Message history growth:** Unbounded unless compaction or budget enforcement active. Per-round: 1-3 messages appended (assistant text, tool uses, tool results). No hard limit on message count.

**Context compaction:** Optional, 15s timeout, invokes same provider for summarization. If timeout or disabled, messages accumulate indefinitely.

**Token budget:** Checked AFTER response received (not before) — can overshoot by one full round. Default: 0 (unlimited).

**Cost tracking:** Estimated only (provider.estimate_cost()), not validated against actual API billing. Tracked but not enforced as a budget.

### 2.5 Runtime Issues

| ID | Issue | Severity | Location | Description |
|----|-------|----------|----------|-------------|
| RT-1 | Provider invocation has no timeout | **CRITICAL** | agent.rs:530 | invoke_with_fallback() can hang indefinitely on network failure. Only Ctrl+C or OS kill terminates. |
| RT-2 | Unbounded message history growth | **CRITICAL** | agent.rs:513,913,1117 | No hard limit on message count. Compaction is best-effort (15s timeout, can be skipped). OOM on long sessions. |
| RT-3 | Token budget checked post-response | **CRITICAL** | agent.rs:807 | Budget enforced AFTER response, not before. Can overshoot by 10k+ tokens in one round. |
| RT-4 | Tool result accumulation unbounded | **CRITICAL** | agent.rs:963 | All tool results held in memory before append. OOM on parallel batch with large outputs (e.g., file_read 100MB). |
| RT-5 | Speculative routing non-determinism | MAJOR | speculative.rs | Different provider used on each run. Cost/latency varies unpredictably. Not logged which provider won. |
| RT-6 | No fallback retry chain | MAJOR | agent.rs:138 | If promoted fallback also fails, no further fallback attempted. Single point of failure. |
| RT-7 | Spinner may race with first chunk | MAJOR | agent.rs:526 | Spinner started as fire-and-forget. May attempt stop() before start() on fast responses. |
| RT-8 | Resilience pre-filters but no "all unhealthy" fallback | MAJOR | agent.rs:71 | If ALL providers unhealthy, returns error instead of trying anyway. |
| RT-9 | Tool permissions lack granularity | MAJOR | executor.rs:284 | TBAC checks tool name + params but no context-aware argument validation. |
| RT-10 | No per-tool invocation limit across rounds | MAJOR | agent.rs | Same tool can be called up to max_rounds times (25 default). |
| RT-11 | Event bus has no backpressure | MAJOR | cuervo-core | broadcast::channel(256) drops events if subscriber can't keep up. |
| RT-12 | Message history fully cloned per round | MINOR | agent.rs:418 | request.messages.clone() copies entire history. Use Arc or message IDs instead. |
| RT-13 | Reflexion always runs on non-Success | MINOR | agent.rs:987 | May pollute reflection storage with transient error noise. |
| RT-14 | Trace step index incremented on error | MINOR | agent.rs:229 | Trace steps may have gaps if DB writes fail. |
| RT-15 | Cache miss-then-hit in same round impossible | MINOR | agent.rs:481 | Cache checked before invocation. Next message has different messages, so cache ineffective for streaks. |
| RT-16 | Cost estimates only, not enforced | MINOR | agent.rs:676 | estimated_cost_usd tracked but never used to stop loop (unlike tokens). |

---

## 3. Observability & Tracing

### 3.1 Structured Logging

**Framework:** tracing + tracing-subscriber (stderr, --verbose flag)

**Agent loop spans:**
```
run_agent_loop() [#[instrument(skip_all)]]
  └─ gen_ai.agent.round (info_span!)
     └─ [no child spans — flat hierarchy]
```

**Gap:** No spans for tool execution, provider invocation, permission checks, or resilience decisions. Only top-level round span exists.

**Ad-hoc logging:** 11 `eprintln!` calls in agent.rs bypass structured logging (lines 357, 474, 483, 544, 593, 646, 763, 815, 848, 888, 1136). These critical operational messages are not queryable, timestamped, or captured in log files.

### 3.2 Trace Recording

**Schema:** `trace_steps` table with session_id, step_index, step_type, data_json, duration_ms, timestamp.

**Step types:** ModelRequest, ModelResponse, Error, ToolCall, ToolResult (5 types).

**Coverage:** Complete for model invocation + tool execution. Missing: plan generation, compaction, reflection, resilience decisions.

**Replay:** Designed for replay (append-only, ordered by step_index) but no replay implementation exists.

### 3.3 Metrics

**Persisted:** InvocationMetric per model round — provider, model, latency_ms, tokens, cost, success, stop_reason, session_id.

**Missing metrics:**

| Metric | Impact |
|--------|--------|
| Tool execution time | Cannot identify slow tools or set tool-level SLOs |
| Tool invocation count | Unknown tool usage patterns |
| Tool success/failure rate | Cannot identify problematic tools |
| Context compaction overhead | Cannot tune compaction thresholds |
| Reflection cost (tokens, latency) | Conflated with round cost |
| Guard rejection rates | Cannot assess guard effectiveness |
| Cache hit/miss ratio by model | Cannot recommend cache tuning per model |
| Provider fallback frequency | Opaque fallback patterns |

### 3.4 Domain Event Bus

**21 EventPayload variants defined. Only 8 actively emitted (38%).**

| Event | Status | Emitted From |
|-------|--------|-------------|
| ModelInvoked | Emitted | agent.rs:665 |
| ToolExecuted | Emitted | executor.rs:208,415 |
| PermissionRequested | Emitted | executor.rs:334 |
| PermissionGranted | Emitted | executor.rs:382 |
| PermissionDenied | Emitted | executor.rs:363 |
| PlanGenerated | Emitted | agent.rs:292 |
| PlanStepCompleted | Emitted | agent.rs:1057 |
| GuardrailTriggered | Emitted | agent.rs:467,756 |
| ReflectionGenerated | Emitted | agent.rs:999 |
| AgentStarted | **NOT EMITTED** | — |
| AgentCompleted | **NOT EMITTED** | — |
| PiiDetected | **NOT EMITTED** | — |
| SessionStarted | **NOT EMITTED** | — |
| SessionEnded | **NOT EMITTED** | — |
| ConfigChanged | **NOT EMITTED** | — |
| CircuitBreakerTripped | **NOT EMITTED** | — |
| HealthChanged | **NOT EMITTED** | — |
| BackpressureSaturated | **NOT EMITTED** | — |
| ProviderFallback | **NOT EMITTED** | — |
| PolicyDecision | **NOT EMITTED** | — |
| EpisodeCreated | **NOT EMITTED** | — |
| MemoryRetrieved | **NOT EMITTED** | — |

**Resilience events bypass the event bus entirely** — written directly to resilience_events table, never broadcast as DomainEvent.

### 3.5 Audit Trail

**Schema:** audit_log table with hash chain (SHA-256, previous_hash → hash).

**Coverage:** 8 of 21 event types actually persisted.

**Gap:** session_id is inside JSON payload, not a column — cannot efficiently query "all events for session X" without JSON parsing.

### 3.6 OpenTelemetry Readiness

**Status: Not ready.**
- Only 1 span uses GenAI conventions (`gen_ai.agent.round`)
- Missing: `gen_ai.response.input_tokens`, `gen_ai.response.output_tokens`, `gen_ai.response.finish_reason`
- No OTLP exporter configured
- No span hierarchy for tool/provider/permission/resilience operations

### 3.7 Observability Issues

| ID | Issue | Severity | Location | Description |
|----|-------|----------|----------|-------------|
| OB-1 | Tool execution duration not persisted | **CRITICAL** | executor.rs | duration_ms captured locally but never inserted to metrics DB |
| OB-2 | Resilience events bypass event bus | **CRITICAL** | resilience.rs | Circuit breaker/health/fallback events not broadcast |
| OB-3 | 13 domain events never emitted | **CRITICAL** | event.rs | Security audit blind spots (PiiDetected, PolicyDecision not tracked) |
| OB-4 | No cost validation against actual API spend | **CRITICAL** | metrics.rs | Estimates may diverge from actual billing |
| OB-5 | No tool-level metrics table | **CRITICAL** | — | Cannot optimize tool performance or set SLOs |
| OB-6 | Session ID not indexed in audit_log | **CRITICAL** | audit.rs | Cannot efficiently reconstruct session history |
| OB-7 | 11 eprintln! bypass structured logging | MAJOR | agent.rs | Critical messages not queryable or timestamped |
| OB-8 | Stop reason format inconsistent | MAJOR | agent.rs:777 | Debug format "EndTurn" vs serde "end_turn" in different tables |
| OB-9 | No span context for provider invocation | MAJOR | agent.rs | Provider latency invisible to distributed tracing |
| OB-10 | Reflection cost not tracked separately | MAJOR | agent.rs:990 | Conflated with round cost |
| OB-11 | Cache not linked to model | MAJOR | response_cache.rs | Cannot analyze cache effectiveness per model |
| OB-12 | Permission events only for Destructive | MAJOR | executor.rs:333 | ReadOnly execution invisible to audit |
| OB-13 | Guardrail violations not queryable | MAJOR | agent.rs:750 | No aggregate table |
| OB-14 | Plan generation not persisted to trace | MAJOR | agent.rs:299 | Plan cost not in replay trace |
| OB-15 | Compaction overhead not measured | MAJOR | agent.rs:359 | Cannot identify compaction as bottleneck |
| OB-16 | Trace recording failures not monitored | MAJOR | agent.rs:228 | Silent trace data loss |
| OB-17 | No OTLP export configured | MAJOR | — | Not compatible with cloud observability |
| OB-18 | Cost estimation accuracy not validated | MAJOR | metrics.rs | No reconciliation against API billing |
| OB-19 | Round outcomes not persisted | MAJOR | executor.rs | Cannot identify round success/failure patterns |
| OB-20 | Health score formula duplicated | MAJOR | resilience.rs, doctor.rs | Two independent implementations may diverge |
| OB-21 | EventPayload match not exhaustive-enforced | MINOR | audit.rs | New variants silently missed |
| OB-22 | Trace step JSON schema not documented | MINOR | trace.rs | Consumers must reverse-engineer format |
| OB-23 | Memory retrieval not tracked as event | MINOR | memory.rs | Cannot audit knowledge base queries |
| OB-24 | Episode creation not tracked as event | MINOR | episodes.rs | Cannot audit episodic memory lifecycle |
| OB-25 | Cost display precision truncates micro-txns | MINOR | agent.rs:687 | format!("${:.4}") rounds < $0.0001 |

---

## 4. Safety & Security

### 4.1 Permission Model

**Two-layer authorization:**

**Layer 1: TBAC (Task-Based Authorization Control)**
- `TaskContext` with allowed_tools (HashSet), parameter_constraints (PathRestriction, CommandAllowlist, ValueAllowlist), max_invocations, expires_at
- Child contexts enforce narrowing (intersection of parent + child tools)
- **Disabled by default** (`tbac_enabled: false` in config)
- Returns AuthzDecision: Allowed, ToolNotAllowed, ParamViolation, ContextInvalid, NoContext

**Layer 2: Legacy Permission Check**
- ReadOnly/ReadWrite: auto-allowed (no prompt)
- Destructive: prompt if `confirm_destructive=true` AND tool not in `always_allowed`
- "Always allow" is session-scoped, in-memory only

**Wiring:** TBAC check runs BEFORE legacy check (executor.rs:284). If TBAC returns NoContext, falls through to legacy.

### 4.2 Sandboxing

**Unix rlimit-based only:**
- RLIMIT_CPU: max CPU seconds (default 60s)
- RLIMIT_FSIZE: max file size bytes (default 50MB)
- Output truncation: 60% head + 30% tail (max_output_bytes)

**Not enforced:**
- RLIMIT_AS (memory) — omitted for macOS/container compatibility
- RLIMIT_NPROC (subprocess limits) — not implemented
- Network isolation — no rlimit, namespace, or eBPF firewall
- Filesystem isolation — no chroot or mount namespace
- System call filtering — no seccomp

**Platform:** Unix only (#[cfg(unix)]). No-op on Windows.

### 4.3 Input Validation

**PII Detection:** 14-pattern RegexSet (SIMD) detector exists in cuervo-security but is **NOT wired into the agent loop**. Manual configuration required.

**Tool Input:** Minimal validation. Bash commands passed directly to `Command::new("bash").arg("-c").arg(command)`. No escaping, no length limits, no sanitization.

**Blocked Patterns:** Config defines patterns (.env, *.key, credentials.json) but they are **NOT enforced by any tool**. Config-only with no implementation.

### 4.4 Guardrails

**Built-in (always-on when enabled):**
1. PromptInjectionGuardrail — 4 patterns, **warn-only**
2. CodeInjectionGuardrail — 5 destructive patterns (rm -rf, fork bomb, mkfs, dd, curl|bash), **warn-only**

**Checkpoints:**
- Pre-invocation: scans last user message before sending to model
- Post-invocation: scans model output after response

**Default action: Warn only.** Built-in guardrails do not block or redact. Only custom config-based guardrails with explicit "block" action will halt execution.

### 4.5 Tool Output Injection

**Attack flow:** Tool results are appended directly to message history without sanitization. Malicious tool output (e.g., "ignore previous instructions") reaches the model in the next round as a legitimate ToolResult block. Post-invocation guardrails only warn after the model has already been influenced.

### 4.6 Safety Issues

| ID | Issue | Severity | Location | Description |
|----|-------|----------|----------|-------------|
| SF-1 | Default budget unlimited (0) | **CRITICAL** | config.rs:530 | Unbounded API spend without opt-in limits. Config warns but doesn't enforce. |
| SF-2 | No tool input validation/escaping | **CRITICAL** | bash.rs:52 | Command injection via model-generated bash commands. `$(...)` expansion not prevented. |
| SF-3 | Tool results not sanitized | **CRITICAL** | agent.rs:972 | Prompt injection via tool output influences next round. Guardrails only warn post-hoc. |
| SF-4 | confirm_destructive=false bypasses all prompts | **CRITICAL** | permissions.rs:126 | User misconfiguration allows bash/file_write without any interaction. |
| SF-5 | TBAC disabled by default | MAJOR | config.rs:375 | Parameter constraints (command allowlist, path restrictions) not enforced unless explicitly enabled. |
| SF-6 | Blocked patterns not enforced | MAJOR | config.rs:311 | .env, *.key patterns defined but NOT validated by file tools. |
| SF-7 | Post-invocation guardrails warn-only | MAJOR | guardrails.rs:233 | Model output with dangerous patterns continues to next round. |
| SF-8 | No network isolation for bash | MAJOR | bash.rs | Commands can curl/wget, exfiltrate data, connect to external servers. |
| SF-9 | No memory limit on subprocess | MAJOR | sandbox.rs:97 | RLIMIT_AS omitted. Bash can consume all available memory. |
| SF-10 | Unbounded parallel tool execution | MAJOR | executor.rs:194 | No max concurrent limit on join_all. Could spawn hundreds of processes. |
| SF-11 | PII detector not wired | MINOR | pii.rs | 14-pattern detector exists but never called in agent loop. |
| SF-12 | Prompt injection patterns basic | MINOR | guardrails.rs:207 | Only 4 hardcoded patterns. Sophisticated injection bypasses detection. |
| SF-13 | No per-tool rate limiting | MINOR | tools/* | Same tool invocable unlimited times per session without TBAC. |
| SF-14 | Backpressure semaphore no timeout | MINOR | backpressure.rs | acquire() blocks indefinitely if saturated. |
| SF-15 | Compaction timeout hardcoded 15s | MINOR | agent.rs:360 | Not configurable. Slow models may always timeout. |
| SF-16 | No parallel batch concurrency limit | MINOR | executor.rs:194 | join_all spawns all at once. No batching (e.g., max 10). |

---

## 5. User Experience Layer

### 5.1 Streaming UX

**StreamRenderer** (render/stream.rs) uses a two-state machine:
- **Prose:** Text deltas printed immediately per chunk
- **CodeBlock:** Accumulated in buffer until closing ``` fence, then syntax-highlighted and printed all at once

**Gap:** Complete code blocks wait for closing fence — no incremental rendering feedback during long code blocks.

**Spinner:** "Thinking..." with elapsed time. 200ms delay before display (avoids flicker). Stops on first TextDelta, ToolUseStart, or Error chunk.

### 5.2 Tool Execution Display

- **Start:** `╭─ name(args)` with summarized args (path, command first 50 chars)
- **Result:** `╰─ [OK/ERROR 42ms]` with formatted duration
- **Output:** Truncated to 50 lines max with "... (N more lines)"
- **Permission:** `[y]es [n]o [a]lways` prompt for destructive tools
- **Denied:** `╰─ [DENIED] name`

All box-drawing chars respect NO_COLOR (color.rs fallback to ASCII).

### 5.3 Error & Warning Display

Standardized via `render/feedback.rs`:
```
Error: {message}
  Hint: {hint}

Warning: {message}
  Hint: {hint}
```

Coverage: Provider errors, config validation, MCP failures, session save failures. All written to stderr.

**Gap:** Silent failures via `let _ = ...` in 6+ places (event sends, trace recording, auto-save). User unaware of data loss.

### 5.4 Progress & State Visibility

| Signal | Location | Display |
|--------|----------|---------|
| Per-message | mod.rs:463 | `[N tokens \| Xs \| $cost \| M tool rounds]` |
| Per-session (exit) | mod.rs:300 | `Session: N rounds \| Xs \| $cost \| M tools \| session_id` |
| Cache hit | agent.rs:483 | `[cached]` |
| Compaction | agent.rs:357 | `[compacting context...]` |
| Guardrail block | agent.rs:474 | `[blocked by guardrail]` |
| Token budget | agent.rs:815 | `[token budget exceeded: X / Y tokens]` |
| Duration budget | agent.rs:848 | `[duration budget exceeded: Xs / Ys]` |
| Max rounds | agent.rs:1136 | `[max rounds reached: X]` |
| Round separator | agent.rs:888 | `--- round N ---` (only if rounds > 1) |

### 5.5 Accessibility

- NO_COLOR env var + TERM=dumb detection (OnceLock at startup)
- Unicode fallback: `╭→+`, `╰→+`, `─→-`, `│→|`
- Syntax highlighting disabled when NO_COLOR set
- No structured/JSON output mode for script consumption

### 5.6 UX Issues

| ID | Issue | Severity | Location | Description |
|----|-------|----------|----------|-------------|
| UX-1 | First round has no separator | **CRITICAL** | agent.rs:887 | User can't tell when tool execution starts. Separator only shown for rounds > 1. |
| UX-2 | Auto-save failures silent | **CRITICAL** | mod.rs:513 | Fire-and-forget with tracing::warn! only. User unaware of session data loss. |
| UX-3 | Code blocks not rendered incrementally | **CRITICAL** | stream.rs:135 | Entire code block waits for closing fence. No feedback during long code blocks. |
| UX-4 | No per-round cost display | MAJOR | agent.rs:684 | Cost only at debug log level. Users can't track per-round spend. |
| UX-5 | Round separator timing confusing | MAJOR | agent.rs:888 | Shown AFTER round completes, not BEFORE. Confuses round boundaries. |
| UX-6 | TBAC denial no user-friendly message | MAJOR | executor.rs:303 | Generic error, doesn't call render_tool_denied() for consistency. |
| UX-7 | No progress during long tool execution | MAJOR | executor.rs:70 | Tools can take seconds with no feedback. CLI appears hung. |
| UX-8 | Config validation hints incomplete | MAJOR | chat.rs:152 | Suggestions missing for many validation errors. |
| UX-9 | Stream error doesn't set done flag | MINOR | stream.rs:69 | Error chunk doesn't terminate rendering. Must wait for Done. |
| UX-10 | stdout/stderr mixed | MINOR | tool.rs:23 | Tool status → stderr, responses → stdout. Inconsistent when combined. |
| UX-11 | Cache miss silent | MINOR | agent.rs:480 | Only hit shows [cached]. No indication of miss. |
| UX-12 | Spinner brackets inconsistent | MINOR | spinner.rs | Uses () for elapsed time. Other status uses []. |
| UX-13 | Language label detection fragile | MINOR | stream.rs:99 | Language label can be split across TextDelta chunks. |
| UX-14 | No JSON output mode | MINOR | — | All output human-formatted. Not script-parseable. |

---

## 6. Consolidated Issue Registry

### Critical Issues (17 total)

| ID | Category | Issue | Impact | Complexity |
|----|----------|-------|--------|------------|
| RT-1 | Runtime | Provider invoke no timeout | Process hangs on network failure | Low |
| RT-2 | Runtime | Unbounded message history | OOM on long sessions | Medium |
| RT-3 | Runtime | Token budget post-check | Budget overshoot by 10k+ tokens | Low |
| RT-4 | Runtime | Tool results unbounded | OOM on large parallel batch | Medium |
| OB-1 | Observability | Tool metrics not persisted | Cannot optimize tools | Low |
| OB-2 | Observability | Resilience events bypass bus | Unauditable resilience decisions | Medium |
| OB-3 | Observability | 13 events never emitted | 38% audit coverage | Medium |
| OB-4 | Observability | No cost validation | Estimates may diverge from billing | High |
| OB-5 | Observability | No tool metrics table | Cannot set tool SLOs | Low |
| OB-6 | Observability | Session ID not in audit_log | Cannot reconstruct sessions | Low |
| SF-1 | Safety | Default budget unlimited | Unbounded API spend | Low |
| SF-2 | Safety | No tool input validation | Command injection via bash | High |
| SF-3 | Safety | Tool output not sanitized | Prompt injection via results | Medium |
| SF-4 | Safety | confirm_destructive bypass | Silent destructive execution | Low |
| UX-1 | UX | First round invisible | Tool start unclear | Low |
| UX-2 | UX | Auto-save failures silent | Data loss undetected | Low |
| UX-3 | UX | Code blocks not incremental | No feedback on long blocks | Medium |

### Major Issues (32 total)

Runtime: RT-5 through RT-11 (7 issues)
Observability: OB-7 through OB-20 (14 issues)
Safety: SF-5 through SF-10 (6 issues)
UX: UX-4 through UX-8 (5 issues)

### Minor Issues (22 total)

Runtime: RT-12 through RT-16 (5 issues)
Observability: OB-21 through OB-25 (5 issues)
Safety: SF-11 through SF-16 (6 issues)
UX: UX-9 through UX-14 (6 issues)

---

## 7. Recommendations

### Phase 1: Critical Safety (Blocks production use)

**Priority: Immediate**

1. **Add provider invoke timeout** (RT-1)
   - Wrap invoke_with_fallback() with tokio::time::timeout(30s)
   - Configurable per-provider via HttpConfig

2. **Add max_message_count limit** (RT-2)
   - Default 100 messages. Mandatory compaction when exceeded.
   - New config field: agent.limits.max_messages

3. **Check token budget before invoke** (RT-3)
   - Move budget check to BEFORE provider invocation, not after

4. **Enforce non-zero budget defaults or mandatory warning** (SF-1)
   - Change default to max_total_tokens=200000 or make startup block on zero budget

5. **Add tool result sanitization** (SF-3)
   - Run guardrail patterns on tool results before appending to messages
   - Redact detected patterns (not just warn)

6. **Change code_injection guardrail to Block** (SF-7)
   - Built-in guardrails should block by default, not just warn

### Phase 2: Observability Foundation

**Priority: Next sprint**

7. **Create tool_execution_metrics table** (OB-1, OB-5)
   - Schema: tool_name, duration_ms, success, session_id, created_at
   - Insert per tool execution in executor.rs

8. **Emit missing domain events** (OB-3)
   - AgentStarted/Completed at loop boundaries
   - SessionStarted/Ended in Repl
   - PiiDetected in security module (when wired)

9. **Route resilience events through event bus** (OB-2)
   - Emit DomainEvent BEFORE persisting to resilience_events table

10. **Add session_id column to audit_log** (OB-6)
    - New migration: ALTER TABLE audit_log ADD COLUMN session_id TEXT
    - Index for session queries

11. **Replace eprintln! with structured logging** (OB-7)
    - All 11 instances → tracing::info!/warn! with structured fields
    - Separate render layer for user-facing messages

### Phase 3: Runtime Hardening

**Priority: Following sprint**

12. **Add provider invocation spans** (OB-9)
    - #[instrument] on invoke_with_fallback(), resilience.pre_invoke()
    - Add GenAI span attributes (tokens, cost, stop_reason)

13. **Implement fallback retry chain** (RT-6)
    - Try primary, then each fallback sequentially (not just first)

14. **Cap parallel tool concurrency** (SF-10)
    - futures::stream::buffer_unordered(max_concurrent) instead of join_all
    - Default max_concurrent=10

15. **Wire PII detector** (SF-11)
    - Call PiiDetector::redact() on tool results and model output when pii_detection=true

16. **Enforce blocked_patterns in file tools** (SF-6)
    - Validate file paths against blocked_patterns before tool execution

### Phase 4: UX Hardening

**Priority: Subsequent sprint**

17. **Fix round separator timing** (UX-1, UX-5)
    - Print separator BEFORE model invocation, not after
    - Show for all rounds including first

18. **Surface auto-save failures** (UX-2)
    - Show user_warning() on session save failure, not just tracing::warn!

19. **Add per-round cost display** (UX-4)
    - Print cost to stderr after each round (not just debug log)

20. **Add tool execution spinner** (UX-7)
    - Show "Executing tool..." spinner during long tool execution

### Phase 5: Advanced Observability

**Priority: When cloud deployment needed**

21. **OTLP export** (OB-17)
    - Add tracing-opentelemetry + OTLP HTTP exporter
    - Span hierarchy: session → round → provider.invoke → tool.execute

22. **Cost reconciliation** (OB-4)
    - Periodic validation against Anthropic API usage endpoint

23. **Tool-level cost attribution** (OB-10)
    - Track reflection cost separately from round cost

24. **Trace replay implementation**
    - Execute agent loop from recorded trace steps for deterministic testing

---

## Attack Surface Summary

| Vector | Exploitability | Current Mitigation | Risk |
|--------|---------------|-------------------|------|
| confirm_destructive=false | High (single config) | Config warning on startup | **CRITICAL** |
| Bash command injection | High (model-generated) | Permission prompt (if enabled) | **CRITICAL** |
| Tool output prompt injection | High (any tool output) | Post-invocation guardrails (warn only) | **CRITICAL** |
| Path traversal via file tools | Medium (TBAC disabled) | blocked_patterns (NOT enforced) | **MAJOR** |
| Network exfiltration via bash | Medium (requires bash tool) | No network isolation | **MAJOR** |
| Memory exhaustion via bash | Medium (no RLIMIT_AS) | CPU+FSIZE limits only | **MAJOR** |
| Unbounded parallel fork | Low (model must request) | No concurrency cap | **MAJOR** |
| Unbounded API spend | Low (requires long session) | Budget warning (not enforced) | **MAJOR** |

---

## Files Analyzed

### Core Runtime
- `crates/cuervo-cli/src/repl/agent.rs` — 1,886 lines (agent loop)
- `crates/cuervo-cli/src/repl/mod.rs` — 1,247 lines (REPL lifecycle)
- `crates/cuervo-cli/src/repl/executor.rs` — 680 lines (tool execution)
- `crates/cuervo-cli/src/repl/accumulator.rs` — 295 lines (stream accumulation)
- `crates/cuervo-cli/src/repl/permissions.rs` — 450+ lines (TBAC + legacy)
- `crates/cuervo-cli/src/repl/circuit_breaker.rs` — circuit breaker FSM
- `crates/cuervo-cli/src/repl/backpressure.rs` — semaphore-based limiting
- `crates/cuervo-cli/src/repl/resilience.rs` — resilience manager facade

### Observability
- `crates/cuervo-core/src/types/event.rs` — 21 domain event variants
- `crates/cuervo-storage/src/db/traces.rs` — trace recording
- `crates/cuervo-storage/src/db/metrics_repo.rs` — invocation metrics
- `crates/cuervo-storage/src/db/audit.rs` — hash-chained audit log
- `crates/cuervo-storage/src/db/resilience_repo.rs` — resilience events

### Safety & Security
- `crates/cuervo-security/src/pii.rs` — 14-pattern PII detector
- `crates/cuervo-security/src/guardrails.rs` — prompt/code injection guards
- `crates/cuervo-tools/src/sandbox.rs` — rlimit sandboxing
- `crates/cuervo-tools/src/bash.rs` — bash tool execution
- `crates/cuervo-core/src/types/auth.rs` — TBAC TaskContext
- `crates/cuervo-core/src/types/config.rs` — security configuration

### UX Layer
- `crates/cuervo-cli/src/render/stream.rs` — streaming state machine
- `crates/cuervo-cli/src/render/tool.rs` — tool execution display
- `crates/cuervo-cli/src/render/spinner.rs` — inference spinner
- `crates/cuervo-cli/src/render/feedback.rs` — error/warning formatting
- `crates/cuervo-cli/src/render/color.rs` — NO_COLOR + accessibility
- `crates/cuervo-cli/src/render/syntax.rs` — syntax highlighting
- `crates/cuervo-cli/src/commands/chat.rs` — chat command entry point
- `crates/cuervo-cli/src/commands/doctor.rs` — diagnostic command

**Total lines analyzed:** ~8,000+ across 20+ files
