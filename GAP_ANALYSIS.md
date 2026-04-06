# HALCON vs Xiyo — Architectural Gap Analysis

**Date**: 2026-04-02
**Purpose**: Systematic comparison of HALCON REPL against Xiyo reference architecture
**Method**: Side-by-side structural analysis of equivalent subsystems

---

## 1. Gap Table

| Concern | HALCON (Current) | Xiyo (Reference) | Gap | Severity |
|---------|-----------------|------------------|-----|----------|
| **Tool execution entry point** | `execute_tool_pipeline()` — 9-stage pipeline, unified per tool | `Tool.call()` — single method on trait, all tools implement | Equivalent | ✅ OK |
| **Tool concurrency model** | `plan_execution()` → parallel (ReadOnly) + sequential (Destructive) batches | `runTools()` → partitioned batches: consecutive read-only concurrent, non-read-only serial | Equivalent | ✅ OK |
| **Permission pipeline** | Fragmented: `executor/` checks permissions AND `security/conversational` checks independently | Unified: `canUseTool()` → Rules → Hooks → Classifier → Dialog (single call path) | **Dual entry points** | **P0** |
| **Permission denial tracking** | Implicit in conversational handler | Explicit: `recordDenial()` → counter → `shouldFallbackToPrompting()` after 3 denials | Missing denial escalation | **P1** |
| **Failure handling** | `retry.rs`: 40+ transient patterns, exponential backoff, adaptive arg mutation, env repair | `withRetry()`: 10 max retries, 529/429 handling, fast-mode fallback, persistent mode | HALCON more comprehensive | ✅ HALCON > Xiyo |
| **Failure waterfall** | `retry.rs` → env_repair (Cargo lock) → adaptive mutation → give up | `retry → repair → fallback model → persistent mode → manual` | Both have cascading strategies | ✅ OK |
| **Streaming execution** | **Post-hoc**: collect ALL tool blocks from response, THEN execute batch | **Streaming-native**: `StreamingToolExecutor` starts tools as blocks arrive during streaming | Missing streaming execution | **P0** |
| **State model (orchestrator)** | `Repl` 43 fields + `AgentContext` 36 fields + `LoopState` 50+ fields | `QueryEngine` 7 fields + `State` 9 fields (BUT `AppState` 120+ and `ToolUseContext` 37) | Different distribution, HALCON has fewer total fields | **P1** (LoopState only) |
| **State immutability** | Mutable structs, manual discipline | `DeepImmutable<>` wrapper + explicit mutable escape hatches | No compile-time immutability enforcement | **P2** |
| **Context compaction** | L0-L4 tiered pipeline with governance, HNSW vector memory | Auto-compact + reactive-compact triggered by token thresholds | HALCON more sophisticated | ✅ HALCON > Xiyo |
| **Multi-agent orchestration** | Full topological wave sort, SharedBudget, dependency cascading | Coordinator module (simpler), Agent tool spawning | HALCON significantly more advanced | ✅ HALCON > Xiyo |
| **Plugin system** | V3 with cost tracking, circuit breakers, UCB1 routing, Stdio/HTTP/Local transport | Plugin system with enabled/disabled lifecycle, reconnect, error tracking | HALCON more mature | ✅ HALCON > Xiyo |
| **Observability** | 12-file metrics module, ARIMA anomaly detection, execution tracker, trace recording | Telemetry hooks, API metrics (TTFT), spinners, simple counters | HALCON far more advanced | ✅ HALCON >> Xiyo |
| **Model routing** | UCB1 adaptive routing, model quality cache, balanced mode, SLA-driven | Simple model selection, fast mode toggle, effort levels | HALCON more sophisticated | ✅ HALCON > Xiyo |
| **Planning** | LLM planner + playbook planner + model selector + SLA routing | No formal planning layer | HALCON has planning; Xiyo doesn't | ✅ HALCON > Xiyo |
| **Loop convergence** | Convergence controller, termination oracle, scoring, guard limits | Max turns + budget check + stop hooks | HALCON more rigorous | ✅ HALCON > Xiyo |
| **Hook system** | Pre/PostToolUse hooks via lifecycle | Pre-tool, permission, post-tool, stop hooks — all in execution pipeline | Both have hooks; Xiyo's are more integrated into permission flow | **P2** |
| **Error recovery (query loop)** | Guard limits, circuit breakers, resilience manager | `maxOutputTokensRecovery`, `promptTooLong` recovery, streaming fallback with tombstoning | Missing query-level recovery patterns | **P1** |
| **Speculation** | Speculator module (exists) | SpeculationState with fast-mode integration | Both have speculation | ✅ OK |
| **Memory system** | auto_memory/, episodic, vector_memory, reflection | memdir/ for CLAUDE.md, simpler | HALCON more advanced | ✅ HALCON > Xiyo |

---

## 2. Critical Gaps (Must Address)

### Gap G1: Permission Pipeline Fragmentation (P0)

**HALCON Current State:**
```
Tool execution request
    ├── executor/sequential.rs: checks conversational_permission
    ├── executor/mod.rs: checks plugin pre-invoke gate
    ├── security/blacklist.rs: checks command patterns
    ├── security/tool_policy.rs: classifies permission level
    └── security/conversational.rs: prompts user if needed
    
    (Multiple entry points, no single decision authority)
```

**Xiyo Reference:**
```
Tool execution request
    └── canUseTool(tool, input, context, message, id)
        ├── Phase 1: Rule check (allow/deny/ask rules)
        ├── Phase 2: Hook check (pre-tool hooks + tool.checkPermissions())
        ├── Phase 3: Classifier (auto-mode heuristics)
        └── Phase 4: User dialog (if 'ask')
        
        → Returns: { behavior: 'allow'|'deny'|'ask', reason }
        
    (Single function, deterministic pipeline)
```

**Gap**: HALCON lacks a single `check_permission()` → `PermissionDecision` pipeline. Permissions are evaluated at multiple points with potentially inconsistent results.

**Required Change**: Introduce `permission_pipeline.rs` that consolidates all permission logic into one `fn check(&self, tool, input) -> PermissionDecision` call. All other modules call this single entry point.

---

### Gap G2: Streaming Tool Execution (P0)

**HALCON Current State:**
```
Stream response → Collect ALL tool_use blocks → plan_execution() → execute batch
                  ~~~~~~~~~~~~~~~~~~~~~~~~
                  Entire stream must complete before any tool starts
```

**Xiyo Reference:**
```
Stream response ─┬─ tool_use block arrives → StreamingToolExecutor.addTool()
                 ├─ tool_use block arrives → StreamingToolExecutor.addTool()
                 │                            └─ processQueue() starts safe tools immediately
                 └─ stream ends → getRemainingResults() collects ordered output
```

**Gap**: HALCON adds latency proportional to stream length × tool count. For multi-tool responses, this is significant.

**Required Change**: Implement `StreamingToolExecutor` in Rust that:
1. Accepts tool blocks as they arrive via channel
2. Starts concurrency-safe tools immediately
3. Queues non-safe tools until safe batch completes
4. Returns results in original order

---

### Gap G3: LoopState Explosion (P0→P1)

**HALCON Current State:**
```rust
struct LoopState {
    // FSM state (~5 fields)
    // Metrics accumulator (~15 fields)  
    // Plan tracker (~10 fields)
    // Control signals (~10 fields)
    // Round accumulators (~10+ fields)
    // = 50+ fields, ALL mutable
}
```

**Xiyo Reference:**
```typescript
type State = {
    messages: Message[]                          // conversation state
    toolUseContext: ToolUseContext               // shared context
    autoCompactTracking: ... | undefined        // optional
    maxOutputTokensRecoveryCount: number        // simple counter
    hasAttemptedReactiveCompact: boolean         // flag
    maxOutputTokensOverride: number | undefined  // override
    pendingToolUseSummary: Promise<...> | undefined  // pending
    stopHookActive: boolean | undefined          // flag
    turnCount: number                            // counter
    transition: Continue | undefined             // next action
}
// = 9 fields, each with clear purpose
```

**Gap**: HALCON's LoopState serves 4+ roles simultaneously. Xiyo keeps loop state minimal and pushes other state to `ToolUseContext` and `AppState`.

**Required Change**: Decompose LoopState into:
- `LoopFSM` — state machine transitions only (5 fields)
- `RoundMetrics` — accumulated per-round stats (reset each round)
- `PlanState` — task analysis and planning (optional)
- `ControlSignals` — convergence flags (derived, not stored)

---

## 3. Gaps Where HALCON Leads

| Area | HALCON Advantage | Xiyo Limitation |
|------|-----------------|-----------------|
| **Multi-agent orchestration** | Topological wave sort, SharedBudget, dependency cascading, role-based scaling | Simple coordinator, no budget sharing, no dependency DAG |
| **Failure resilience** | 40+ transient patterns, adaptive arg mutation, env repair, circuit breakers | 10 retry max, model fallback only, no arg mutation |
| **Observability** | 12-file module, ARIMA anomaly detection, reward pipeline, round scoring | Basic API metrics (TTFT), spinner mode |
| **Context management** | L0-L4 tiered compaction, HNSW vector memory, BM25+semantic fusion, governance | Auto-compact threshold, reactive-compact |
| **Planning** | LLM planner, playbook planner, model selector, SLA routing | No formal planning |
| **Model routing** | UCB1 adaptive, quality cache, balanced mode | Static model + fast mode toggle |
| **Security depth** | 25 files: trust chain, risk tiers, tool reputation, output risk (PII), blacklist | Permission rules + classifier + hooks |

These advantages should be preserved and built upon, not regressed toward Xiyo's simpler model.

---

## 4. Gaps Where Xiyo Leads

| Area | Xiyo Advantage | HALCON Limitation |
|------|---------------|-------------------|
| **Streaming tool execution** | `StreamingToolExecutor` starts tools during stream | Post-hoc batch execution |
| **Unified permission pipeline** | Single `canUseTool()` → deterministic decision | Fragmented across modules |
| **Permission denial tracking** | Counter → escalation → fallback to prompting | No systematic denial tracking |
| **Query-level recovery** | `maxOutputTokensRecovery`, `promptTooLong`, streaming fallback | Guard limits only |
| **State distribution** | Thin loop state (9 fields), rich global store | Fat loop state (50+ fields) |
| **Generator-based streaming** | `AsyncGenerator<SDKMessage>` end-to-end | Channel-based with manual collection |
| **Immutability enforcement** | `DeepImmutable<>` at type level | Convention-based only |
| **Sibling error cascading** | Tool A errors → Tool B receives synthetic abort | No inter-tool error propagation |

---

## 5. Convergence Recommendations

### Must Adopt from Xiyo (P0):

1. **Unified permission pipeline** — single entry, deterministic decision
2. **Streaming tool executor** — start tools during stream
3. **Denial tracking with escalation** — counter → threshold → fallback

### Should Adopt from Xiyo (P1):

4. **Query-level recovery patterns** — maxOutputTokens, promptTooLong
5. **Thin loop state** — decompose into focused sub-structs
6. **Sibling error cascading** — abort related tools on failure

### Must Preserve from HALCON (Non-Negotiable):

7. **Topological wave orchestration** — Xiyo doesn't have this
8. **40+ retry patterns** — far more resilient than Xiyo's 10-retry cap
9. **L0-L4 context compaction** — superior to Xiyo's threshold-based compact
10. **UCB1 model routing** — adaptive routing Xiyo lacks
11. **ARIMA observability** — time-series anomaly detection Xiyo lacks
12. **Planning subsystem** — LLM + playbook planning
13. **Circuit breakers** — Xiyo only has 3-consecutive-529 counter

---

## 6. Priority Matrix

```
                HIGH IMPACT
                    │
     ┌──────────────┼──────────────┐
     │              │              │
     │  G1:Permission│  G2:Streaming│
     │  Pipeline    │  Execution   │
     │  (P0)       │  (P0)        │
     │              │              │
LOW ─┼──────────────┼──────────────┼─ HIGH
EFFORT│              │              │ EFFORT
     │  G3:Denial   │  G3:LoopState│
     │  Tracking    │  Decompose   │
     │  (P1)       │  (P1)        │
     │              │              │
     └──────────────┼──────────────┘
                    │
                LOW IMPACT
```

**Recommended order**: G1 → G3 (denial) → G3 (LoopState) → G2

Rationale: G1 is lowest effort and highest security impact. G2 (streaming execution) is highest effort and requires careful integration with the existing executor pipeline.
