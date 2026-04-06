# PHASE 2 CROSS-AUDIT — Halcon REPL vs Xiyo Source Verification

> **Date**: 2026-04-02
> **Scope**: Audit of 4 god-files in `crates/halcon-cli/src/repl/` cross-referenced against Xiyo source (`~/Documents/Github/xiyo/`)
> **Verdict**: Phase 2 plan is **structurally sound** with **7 corrections** needed based on actual code analysis.

---

## 1. FILE AUDIT SUMMARY

| File | Actual LOC | Plan Estimate | Responsibilities Found | Plan Estimate |
|------|-----------|---------------|----------------------|---------------|
| `repl/mod.rs` | 4,659 | ~4,699 | **67** | 10+ |
| `repl/executor.rs` | 4,678 | ~4,678 | **30** | 15+ |
| `repl/agent/mod.rs` | 2,810 | ~2,796 | **52** | 17+ |
| `repl/orchestrator.rs` | 2,973 | ~2,967 | **21** | 11+ |
| **TOTAL** | **15,120** | **15,140** | **170** | 53+ |

**Finding**: LOC estimates are accurate. Responsibility counts are **3.2x higher** than planned minimums — decomposition is more urgent than anticipated.

---

## 2. XIYO REFERENCE VERIFICATION

### 2.1 query.ts — State Object (VERIFIED: 10 fields)

Xiyo's `State` type at lines 204-217 has exactly **10 fields**:

| Field | Type | Halcon Equivalent |
|-------|------|-------------------|
| `messages` | `Message[]` | `session.messages` |
| `toolUseContext` | `ToolUseContext` | `AgentContext` (~60 params) |
| `autoCompactTracking` | `AutoCompactTrackingState?` | `ConvergenceController` |
| `maxOutputTokensRecoveryCount` | `number` | Scattered in `handle_message_with_sink()` |
| `hasAttemptedReactiveCompact` | `boolean` | `ContextCompactor` state |
| `maxOutputTokensOverride` | `number?` | Not present (fixed config) |
| `pendingToolUseSummary` | `Promise?` | Not present |
| `stopHookActive` | `boolean?` | `HookRunner` state |
| `turnCount` | `number` | `session.round_count` |
| `transition` | `Continue?` | `PhaseOutcome` enum |

**Gap**: Halcon's `LoopState` (agent/mod.rs:2054-2203) bundles ~6 sub-states (`TokenAccounting`, `EvidenceState`, `SynthesisControl`, `ConvergenceState`, `LoopGuardState`, `HiconSubsystems`). This is already **better decomposed** than Xiyo's flat State. No action needed here — Halcon is ahead.

### 2.2 QueryEngine.ts — Field Count (CORRECTED: 8 fields, NOT 10)

Plan states "QueryEngine has ~10 fields". Actual count: **8 instance fields** (lines 185-198):
- `config`, `mutableMessages`, `abortController`, `permissionDenials`, `totalUsage`, `readFileState`, `discoveredSkillNames`, `loadedNestedMemoryPaths`

**But**: `config: QueryEngineConfig` contains **29 sub-fields**, and the external `AppState` store holds **254 fields**. Xiyo achieves its lean class by pushing state into two external systems.

**Correction for Phase 2e**: The plan's target of "Repl with 5 sub-structs" is valid, but the comparison basis is misleading. Xiyo's QueryEngine looks lean (8 fields) only because it delegates 283 fields to `config` + `AppState`. Halcon's Repl currently has ~12 top-level fields with 7 grouped sub-structs — it's **already partially decomposed** (Phase 3.2 refactoring completed).

### 2.3 Tool.ts — Single call() Entry Point (VERIFIED)

Lines 379-385 define a single `call()` entry point:
```typescript
call(args, context, canUseTool, parentMessage, onProgress?): Promise<ToolResult<Output>>
```

Lines 500-503 define `checkPermissions()` returning `PermissionResult` with 3 variants: `allow | deny | ask`.

**Halcon equivalent**: `executor.rs:execute_one_tool()` (lines 821-984) serves as the single entry point but chains **8 pre/post gates** inline. The Phase 2a plan to extract these into `mod.rs/validation.rs/hooks.rs` is **correct and necessary**.

### 2.4 StreamingToolExecutor (VERIFIED — More Complex Than Plan Suggests)

Xiyo's implementation (17KB) includes:
- **Concurrency model**: concurrent-safe tools run in parallel; non-concurrent get exclusive access (lines 129-150)
- **Abort cascading**: parent → sibling → per-tool abort controllers (lines 59-61, 301-318)
- **Bash error cascading**: only Bash errors cancel siblings (lines 359-363)
- **Order-preserving output**: generator breaks at first non-concurrent incomplete tool (line 437)
- **Progress streaming**: `pendingProgress[]` for immediate yield during execution (lines 368-374)

**Correction for Phase 2a stretch goal**: The plan's `execute_during_stream()` sketch is a **simplified version** that doesn't capture:
1. Concurrency gating (concurrent vs exclusive tools)
2. Abort signal propagation hierarchy
3. Order-preserving result emission
4. Progress streaming mid-execution

Recommend expanding the stretch goal or deferring to Phase 3.

### 2.5 toolExecution.ts — Permission Pipeline (VERIFIED: 3 gates, NOT 5)

Xiyo has **3 sequential gates** (not the 5 described in the Phase 2b plan):

| Gate | Xiyo | Halcon |
|------|------|--------|
| 1 | PreToolUse hooks (`runPreToolUseHooks()`) | `HookGate` (pre-tool hooks) |
| 2 | `resolveHookPermissionDecision()` — cascading resolver | `TbacGate` + `LegacyGate` merged |
| 3 | Rule-based permissions (`checkRuleBasedPermissions()`) | `PluginGate` + `RiskScoringGate` |

**Key difference**: Xiyo has **no single `authorize()` function**. Instead it uses a cascading if-tree in `resolveHookPermissionDecision()` (toolHooks.ts:347-432) where hook results can be overridden by rules.

**Correction for Phase 2b**: The plan's `authorize_tool()` with sequential `PermissionGate` trait impls is a **better design than Xiyo's**. Xiyo's cascading resolver is harder to test and reason about. Proceed with the plan's design, but note:
- Xiyo allows hooks to **mutate input** (`hookUpdatedInput`). The plan's `GateDecision::Allow(ToolInput)` captures this correctly.
- Xiyo's deny-rule-overrides-hook-allow pattern maps to gate ordering: `HookGate` before `RuleGate` in the chain, with `RuleGate` able to emit `Deny` even after `HookGate` emitted `Allow`.

### 2.6 Error Waterfall (CORRECTED: Different Pattern Than Described)

Xiyo's error waterfall in query.ts (lines 1085-1256) is **4-stage recovery for context overflow**, not for tool failures:
1. Context collapse drain → retry
2. Reactive compact → retry
3. Max output tokens escalation → retry
4. Stop hooks → possible retry

Xiyo's **tool error handling** in toolExecution.ts is a **single try-catch** (line 1206) with classification but **no automatic retry and no backoff**.

**Correction for Phase 2c**: The plan's `handle_tool_failure()` waterfall is a **Halcon-original design**, not a direct Xiyo transfer. This is fine — Halcon's retry logic in executor.rs (lines 625-792) is already more sophisticated than Xiyo's. The plan correctly consolidates it.

### 2.7 Analytics as Leaf Service (VERIFIED)

Xiyo pattern: `logEvent()` is non-blocking, non-awaited, fire-and-forget throughout query.ts (16+ call sites).

Halcon equivalent: Trace recording is scattered across executor.rs, agent/mod.rs, and execution_tracker. Phase 2d's `TraceRecorder` with `mpsc::UnboundedSender` is the **correct translation**.

---

## 3. GOD-FILE RESPONSIBILITY CATALOG

### 3.1 repl/mod.rs (4,659 LOC — 67 responsibilities)

**Category Breakdown:**

| Category | Count | Line Range | Decomposition Target |
|----------|-------|------------|---------------------|
| Initialization & Setup | 9 | 0-790 | `repl/init.rs` |
| Entry Points | 3 | 792-865 | `repl/entry.rs` |
| Background Services | 1 | 866-921 | `repl/services/ci_polling.rs` |
| Interactive REPL Loop | 2 | 923-1183 | `repl/interactive.rs` |
| TUI Mode (8 concerns) | 8 | 1185-2156 | `repl/tui/` subdirectory |
| Message Handling Core | 3 | 2271-2294 | `repl/message_handler.rs` |
| Multimodal Processing | 2 | 2363-2553 | `repl/multimodal.rs` |
| Context Assembly | 6 | 2556-2706 | `repl/context/` (already exists) |
| MCP & Tool Init | 2 | 2583-2642 | `repl/mcp_bridge.rs` |
| Provider & Fallback | 2 | 2708-2779 | Already in provider module |
| Database & Metrics | 4 | 2781-2934 | `repl/metrics/` |
| Plugin System | 2 | 2816-2934 | `repl/plugins/` |
| Agent Loop Orchestration | 2 | 3020-3163 | `repl/agent/` (already exists) |
| Agent Post-Processing | 10 | 3168-3797 | **CRITICAL**: `repl/post_processing.rs` |
| Session Persistence | 3 | 3886-3916 | `repl/session.rs` |
| Utilities & Tests | 7 | 3918-4659 | Remain in mod.rs |

**Critical hotspot**: `handle_message_with_sink()` (lines 2300-3884) is a **1,584-LOC monolith** containing 40+ responsibilities. This single function is the #1 decomposition priority within mod.rs.

**Detailed responsibilities within `handle_message_with_sink()`:**

1. Onboarding check (one-time per session)
2. Plugin resume (cooling period check)
3. Plugin recommendation (one-time)
4. User message recording to session
5. Media path extraction from message
6. Multimodal file reads (Phase 1: sequential)
7. Audio fallback check (Phase 2)
8. Parallel multimodal API calls (Phase 3)
9. Result collection (Phase 4)
10. Context assembly (Phase 5)
11. System prompt building via ContextManager
12. Lazy MCP initialization
13. Cenzontle MCP bridge init (feature-gated)
14. ModelRequest construction
15. User context injection (username, platform)
16. Dev ecosystem context injection (git, IDE, CI)
17. Media context injection
18. Provider lookup & fallback chain
19. LLM planner resolution
20. Cross-session model quality loading
21. Plugin system lazy-init (auto-activation)
22. Plugin UCB1 metrics loading
23. Model selector setup
24. Task bridge initialization
25. UCB1 cross-session experience loading
26. Reasoning engine pre-loop (task analysis, complexity)
27. AgentContext assembly (~60 parameters)
28. Agent loop execution
29. Agent loop result processing
30. Critic retry decision & execution
31. Model quality recording
32. Plugin metrics recording
33. Playbook auto-learning
34. Runtime signal ingest (telemetry)
35. Session token update
36. Result summary display
37. Memory consolidation (30s timeout)
38. Fire-and-forget session auto-save

### 3.2 repl/executor.rs (4,678 LOC — 30 responsibilities)

**Maps to Phase 2a plan:**

| Plan Module | Responsibilities (from audit) | Lines |
|-------------|-------------------------------|-------|
| `mod.rs` (public API) | #4 execute_one_tool, #1 plan_execution | 75-116, 821-984 |
| `parallel.rs` | #2 parallel batch, #25 destructive guard | 1005-1202 |
| `sequential.rs` | #3 sequential execution, #19 TBAC, #20 permissions, #21 sudo | 1204-1589 |
| `validation.rs` | #11 arg validation, #12 path pre-validation, #13 path extraction, #14 suggestions, #15 tool resolution, #16 dry-run, #17 idempotency | 380-623 |
| `retry.rs` | #5 exponential backoff, #6 transient classification, #7 deterministic classification, #8 adaptive mutation, #10 jittered delay | 146-352, 625-792 |
| `hooks.rs` | #26 lifecycle hooks, #24 plugin gates | 842-960 |
| **NOT in plan** | #9 file edit diff preview (UX-9) | 280-345 |
| **NOT in plan** | #18 dynamic risk assessment | 1320-1349 |
| **NOT in plan** | #22 tracing & metrics | 1029-1050, 1494-1586 |
| **NOT in plan** | #23 event bus emissions | 1131-1136, 1552-1557 |
| **NOT in plan** | #27-29 result construction helpers | 118-135, 358-372, 991-1003 |

**Correction**: 5 responsibilities not accounted for in the plan:
1. **File edit diff preview** (#9) → add to `validation.rs` or new `preview.rs`
2. **Dynamic risk assessment** (#18) → move to `security/permission_pipeline.rs` (Phase 2b)
3. **Tracing & metrics** (#22) → move to `tracing/recorder.rs` (Phase 2d)
4. **Event bus emissions** (#23) → inline in each module, or dedicated `events.rs`
5. **Result construction helpers** (#27-29) → add to `mod.rs` as private helpers

### 3.3 repl/agent/mod.rs (2,810 LOC — 52 concerns)

**Phase grouping by function in run_agent_loop():**

| Phase | Concerns | Lines | Extraction Target |
|-------|----------|-------|-------------------|
| Prologue (pre-loop) | #1-5 startup, #6-12 planning, #13-16 tool selection | 343-827 | `agent/prologue.rs` (~485 LOC) |
| System prompt assembly | #20-25 prompt layers | 913-1142 | `agent/system_prompt.rs` (~230 LOC) |
| Sub-agent delegation | #26-31 delegation + policy | 1144-1739 | `agent/delegation.rs` (~595 LOC) |
| Loop guards & budget | #32-37 convergence, budget, coherence | 1819-2037 | `agent/budget.rs` (~220 LOC) |
| State initialization | #38-41 LoopState, FSM, strategy | 2039-2257 | `agent/loop_state.rs` (already exists, expand) |
| Main loop phases | #42-47 round setup/exec/post-batch/convergence | 2259-2604 | Already extracted: `setup.rs`, `post_batch.rs`, `dispatch.rs` |
| Post-loop | #48-52 retrospective, memory, hooks | 2607-2806 | `agent/epilogue.rs` (~200 LOC) |

**Key finding**: Phase 2 doesn't explicitly target agent/mod.rs decomposition, but this file has **52 concerns** — the most of any file. Existing extractions (dispatch.rs, post_batch.rs, simplified_loop.rs, tool_executor.rs, feedback_arbiter.rs) have begun this work but the prologue (485 LOC) and delegation (595 LOC) sections remain monolithic.

### 3.4 repl/orchestrator.rs (2,973 LOC — 21 responsibilities)

| Concern | Lines | Notes |
|---------|-------|-------|
| Shared budget tracking (atomics) | 28-74 | Self-contained, could be own module |
| Topological sort + wave gen | 76-140 | Pure, testable — `orchestrator/topo.rs` |
| Token budget estimation | 142-165 | Pure heuristic — `orchestrator/budget.rs` |
| Sub-agent limits derivation | 167-216 | Pure — merge with budget |
| Main orchestrator function | 218-1317 | **1,100 LOC monolith** — needs splitting |
| Cyclic dependency handling | 284-319 | Part of main function |
| Shared context store | 261-276, 343-355 | Inter-wave communication |
| Tool surface narrowing | 566-666 | Security-critical — `orchestrator/tools.rs` |
| Success classification + retry | 800-1037 | P1-A/P1-B — `orchestrator/retry.rs` |
| Failure cascade | 357-429, 1200-1234 | Dependency graph logic |
| Provider override | 440-454 | Small, inline |
| Panic isolation + timeout | 788-1138 | Execution boundary |
| Budget enforcement mid-wave | 1143-1182 | P10/P4 fixes |
| Event emission + audit | scattered | 5 emission points |
| Task persistence | 539-549, 1237-1256 | DB integration |
| All-failed detection | 1282-1297 | FASE 6 R7 |
| Synthesis whitelist | 1319-1337 | SC-2 guard |
| Permission routing | 705-726 | TUI vs non-interactive |
| Response cache disabling | 748-754 | Sub-agent policy |
| Role-based multipliers | 487-506 | Limit derivation |
| Test suite | 1339-2973 | **1,634 LOC** (55% of file!) |

**Finding**: 55% of the file is tests. Production code is ~1,340 LOC — manageable. Phase 2 doesn't target this file, but the main function (1,100 LOC) should be split during Phase 3.

---

## 4. XIYO → RUST TRANSLATION VERIFICATION

| Xiyo Pattern | Plan Translation | Verification |
|--------------|-----------------|--------------|
| `async generator (yield)` | `loop + mpsc::channel or impl Stream` | **CORRECT** — Xiyo's `query()` is async generator; Halcon already uses `loop {}` with channel sends |
| `interface Tool` | `trait Tool` | **CORRECT** — Already exists in halcon-agent-core |
| `class StreamingToolExecutor` | `struct + impl` | **PARTIAL** — Missing concurrency gating and abort hierarchy. See Section 2.4 |
| `AbortController` | `CancellationToken` | **CORRECT** — Halcon uses `tokio_util::sync::CancellationToken` + ctrl_rx channels |
| `Promise.all()` | `tokio::join!() or FuturesUnordered` | **CORRECT** — executor.rs already uses `buffer_unordered()` (line 1005) |

**Additional translations not in plan:**

| Xiyo Pattern | Recommended Rust Translation |
|--------------|------------------------------|
| `AppState` (254-field external store) | Already handled by Halcon's sub-struct pattern |
| `logEvent()` fire-and-forget | `TraceRecorder::record()` via `mpsc::UnboundedSender` (Phase 2d) — **correct** |
| `using` (dispose pattern) for prefetch | `Drop` impl or `scopeguard::guard!()` |
| Hook `yield` for incremental results | `mpsc::Sender<HookEvent>` — already implemented |
| `resolveHookPermissionDecision()` cascading | `authorize_tool()` with `PermissionGate` trait chain — **improvement over Xiyo** |

---

## 5. PLAN CORRECTIONS REQUIRED

### Correction 1: Responsibility count underestimated (SEVERITY: Medium)
- **Plan**: "List all 10+ / 15+ / 17+ / 11+ distinct responsibilities"
- **Actual**: 67 / 30 / 52 / 21 = 170 total
- **Impact**: Decomposition effort is ~3x larger than estimated. Adjust timelines.

### Correction 2: agent/mod.rs not targeted by Phase 2 (SEVERITY: High)
- **Plan**: Phase 2 targets executor.rs decomposition but not agent/mod.rs
- **Actual**: agent/mod.rs has **52 concerns** (most of any file) with 485-LOC prologue and 595-LOC delegation sections
- **Action**: Add Step 2f for agent/mod.rs prologue/delegation extraction, or promote to Phase 3.

### Correction 3: StreamingToolExecutor stretch goal is under-specified (SEVERITY: Low)
- **Plan**: Simple `execute_during_stream()` sketch
- **Actual**: Xiyo's version handles concurrency gating, abort cascading, order preservation, progress streaming
- **Action**: Defer to Phase 3 or expand specification significantly.

### Correction 4: Xiyo has 3 permission gates, not 5 (SEVERITY: Low)
- **Plan**: "Current state: 5 sequential gates (TBAC → legacy → plugin → hooks → risk scoring)"
- **Clarification**: This describes **Halcon's current state**, not Xiyo's. Xiyo has 3 gates.
- **Impact**: Plan's 6-gate design is correct for Halcon — just clarify the Xiyo comparison is aspirational, not direct.

### Correction 5: Error waterfall is Halcon-original, not Xiyo transfer (SEVERITY: Low)
- **Plan**: "Xiyo waterfall: sequential strategies, first match wins"
- **Actual**: Xiyo's waterfall is for context overflow recovery, not tool failure. Xiyo has **no tool retry/backoff**.
- **Impact**: Phase 2c design is good but should be documented as Halcon-original innovation.

### Correction 6: 5 executor responsibilities missing from plan (SEVERITY: Medium)
- File edit diff preview, dynamic risk assessment, tracing, event bus, result helpers
- **Action**: Assign each to appropriate target module (see Section 3.2).

### Correction 7: mod.rs `handle_message_with_sink()` is the #1 hotspot (SEVERITY: High)
- **1,584 LOC** single function with 38+ inline responsibilities
- Not explicitly targeted by any Phase 2 step
- **Action**: Add dedicated extraction step before or alongside Phase 2e.

---

## 6. PRIORITY-ORDERED DECOMPOSITION ROADMAP

Based on actual code analysis, recommended execution order:

| Priority | Target | LOC | Effort | Rationale |
|----------|--------|-----|--------|-----------|
| **P0** | `mod.rs:handle_message_with_sink()` → extract | 1,584 | 1 week | #1 hotspot, 38+ inline concerns, blocks all mod.rs work |
| **P1** | `executor.rs` → `executor/` subdirectory | 4,678 | 2 weeks | Plan Step 2a, well-defined module boundaries |
| **P2** | `agent/mod.rs` prologue + delegation → extract | 1,080 | 1 week | 52 concerns, prologue (485) + delegation (595) standalone |
| **P3** | Permission pipeline unification | cross-file | 1 week | Plan Step 2b, touches executor + agent + security |
| **P4** | Failure waterfall consolidation | cross-file | 1 week | Plan Step 2c, executor retry + agent convergence |
| **P5** | TraceRecorder consolidation | cross-file | 3 days | Plan Step 2d, low risk |
| **P6** | Repl struct final decomposition | mod.rs | 1 week | Plan Step 2e, depends on P0 |
| **P7** | orchestrator.rs main function split | 1,100 | defer | 55% tests, production code manageable |

---

## 7. METRICS BASELINE

### Current State (for before/after comparison)

| Metric | Current Value |
|--------|--------------|
| Total LOC (4 files) | 15,120 |
| Production LOC (excluding tests) | ~10,800 |
| Test LOC | ~4,320 |
| Max function LOC | 1,584 (`handle_message_with_sink`) |
| Max file LOC | 4,678 (`executor.rs`) |
| Total responsibilities | 170 |
| Max responsibilities/file | 67 (`mod.rs`) |
| Avg responsibilities/file | 42.5 |
| Halcon Repl top-level fields | 12 (with 7 sub-structs) |
| Xiyo QueryEngine fields | 8 (with 29-field config + 254-field AppState) |

### Target After Phase 2

| Metric | Target |
|--------|--------|
| Max file LOC | < 800 |
| Max function LOC | < 200 |
| Max responsibilities/file | < 10 |
| Module count (from 4 files) | 20-25 focused modules |

---

## 8. WHERE HALCON IS AHEAD OF XIYO

Not all comparisons favor Xiyo. Several Halcon patterns are **superior**:

| Area | Halcon Advantage |
|------|-----------------|
| **LoopState decomposition** | 6 typed sub-states vs Xiyo's flat 10-field State |
| **Retry/backoff** | Full exponential backoff + adaptive mutation vs Xiyo's zero-retry |
| **Permission pipeline** | Trait-based gates (planned) vs Xiyo's cascading if-tree |
| **Convergence detection** | Multi-signal ConvergenceController vs Xiyo's turn-count only |
| **Sub-agent orchestration** | Full DAG with topological sort vs Xiyo's flat delegation |
| **Plan coherence** | Jaccard drift detection vs no equivalent in Xiyo |
| **Risk scoring** | Output risk scorer + command blacklist vs Xiyo's rule-based only |

---

## 9. CONCLUSION

The Phase 2 plan is **architecturally sound** — module boundaries, trait designs (`PermissionGate`, `TraceRecorder`), and TypeScript→Rust translations are valid. The plan's `authorize_tool()` pipeline is **better** than Xiyo's cascading if-tree, and Halcon's retry waterfall is an **original innovation** beyond Xiyo's capabilities.

**Critical gaps to address before executing:**
1. `handle_message_with_sink()` (1,584 LOC) is not targeted — must be extracted first (P0)
2. `agent/mod.rs` (52 concerns) is not in Phase 2 scope — needs prologue/delegation extraction
3. Responsibility count is 3.2x the plan's minimum — adjust effort estimates accordingly

**Recommendation**: Execute P0 (handle_message_with_sink extraction) as a prerequisite before the planned Step 2a-2e sequence. This unblocks the Repl struct decomposition (Step 2e) and reduces merge conflict risk for all subsequent steps.
