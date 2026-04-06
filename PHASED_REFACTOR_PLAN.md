# HALCON REPL — Phased Refactor Plan

**Date**: 2026-04-02
**Duration estimate**: Not provided (per project guidelines)
**Constraint**: Each phase is independently shippable with zero behavior drift

---

## Phase Overview

```
Phase 1: Permission Pipeline Unification     [P0, Low risk]
Phase 2: LoopState Decomposition             [P0, Medium risk]
Phase 3: Repl Thinning (ReplBuilder)         [P1, Low risk]
Phase 4: Streaming Tool Executor             [P0, Medium risk]
Phase 5: AgentContext Refinement             [P1, Low risk]
Phase 6: Query-Level Recovery Patterns       [P1, Low risk]
Phase 7: Frontier Enhancements               [P2, Low risk]
```

Each phase produces:
- Regression tests (written FIRST)
- Implementation (incremental commits)
- Migration verification (all existing tests pass)

---

## Phase 1: Permission Pipeline Unification

**Goal**: Single entry point for all permission decisions.
**Files touched**: `executor/sequential.rs`, `security/`, new `permission_pipeline.rs`
**Risk**: Low — additive change, existing paths become wrappers

### Step 1.1: Write Regression Tests

```rust
// tests/permission_regression.rs
#[tokio::test]
async fn test_readonly_tool_permitted_without_prompt() { /* current behavior */ }

#[tokio::test]
async fn test_destructive_tool_requires_confirmation() { /* current behavior */ }

#[tokio::test]
async fn test_blacklisted_command_denied() { /* current behavior */ }

#[tokio::test]
async fn test_plugin_preinvoke_gate_blocks() { /* current behavior */ }

#[tokio::test]
async fn test_hook_can_override_permission() { /* current behavior */ }
```

### Step 1.2: Create `permission_pipeline.rs`

Location: `crates/halcon-cli/src/repl/permission_pipeline.rs`

- Struct `PermissionPipeline` with phases: Rules → Security → Hooks → Ask
- Single `pub async fn check()` method returning `PermissionDecision`
- Move blacklist, tool_policy, tool_trust checks into pipeline phases
- Preserve all existing logic, just centralize call path

### Step 1.3: Create `DenialTracker`

Location: `crates/halcon-cli/src/repl/security/denial_tracker.rs`

- Per-tool denial counter
- Threshold (default 3) before escalation to user prompting
- `record_denial()`, `record_success()`, `should_escalate()`
- Adopted from Xiyo's proven pattern

### Step 1.4: Route Executor Through Pipeline

- `executor/sequential.rs`: replace inline permission checks with `pipeline.check()`
- `executor/mod.rs`: replace plugin gate with `pipeline.check()` phase
- `security/conversational.rs`: becomes a `pipeline.check()` phase, not a standalone call

### Step 1.5: Verify

- All regression tests pass
- All existing integration tests pass
- Permission behavior is identical (just routed through single path)

### Deliverable
```
repl/
  ├── permission_pipeline.rs          (NEW — ~200 LOC)
  ├── security/
  │   ├── denial_tracker.rs           (NEW — ~80 LOC)
  │   └── ... (existing, unchanged)
  ├── executor/
  │   ├── sequential.rs               (MODIFIED — permission calls routed)
  │   └── mod.rs                      (MODIFIED — plugin gate routed)
  └── ... (everything else unchanged)
```

---

## Phase 2: LoopState Decomposition

**Goal**: Replace 50+ field bag with focused sub-structs.
**Files touched**: `agent/loop_state.rs`, `agent/loop_state_roles.rs`, consumers
**Risk**: Medium — widely referenced struct, but change is structural not behavioral

### Step 2.1: Write Regression Tests

```rust
#[test]
fn test_loop_state_round_transition() { /* FSM transitions */ }

#[test]
fn test_loop_state_metrics_accumulation() { /* per-round metric collection */ }

#[test]
fn test_loop_state_convergence_signals() { /* convergence detection */ }

#[test]
fn test_loop_state_plan_tracking() { /* plan state progression */ }
```

### Step 2.2: Create Sub-Structs (Alongside Existing)

```rust
// agent/loop_fsm.rs
pub struct LoopFSM {
    pub phase: LoopPhase,
    pub round: u32,
    pub max_rounds: u32,
    pub transition: Option<LoopTransition>,
    pub is_sub_agent: bool,
}

// agent/round_metrics.rs  
pub struct RoundMetrics {
    pub tokens_input: u64,
    pub tokens_output: u64,
    pub tools_called: u32,
    pub tools_failed: u32,
    pub cost_usd: f64,
    pub duration: Duration,
}

// agent/plan_state.rs
pub struct PlanState {
    pub task_analysis: Option<TaskAnalysis>,
    pub active_plan: Option<Plan>,
    pub steps_completed: u32,
    pub evidence: Vec<Evidence>,
}

// agent/convergence_state.rs
pub struct ConvergenceState {
    pub end_turn_received: bool,
    pub guard_limit_hit: Option<GuardLimit>,
    pub quality_score: f64,
    pub synthesis_triggered: bool,
}
```

### Step 2.3: Add Bridge Methods to Existing LoopState

```rust
impl LoopState {
    pub fn fsm(&self) -> LoopFSM { /* extract from current fields */ }
    pub fn round_metrics(&self) -> RoundMetrics { /* extract */ }
    pub fn plan_state(&self) -> Option<PlanState> { /* extract */ }
    pub fn convergence(&self) -> ConvergenceState { /* extract */ }
}
```

### Step 2.4: Migrate Consumers One-by-One

For each consumer of `LoopState`:
1. Replace `state.field_x` with `state.fsm().phase` or `state.round_metrics().tokens_input`
2. Verify tests pass after each migration

### Step 2.5: Replace LoopState with AgentLoopState

Once all consumers use bridge methods:
```rust
pub struct AgentLoopState {
    pub fsm: LoopFSM,
    pub metrics: RoundMetrics,
    pub session_metrics: SessionMetrics,
    pub plan: Option<PlanState>,
    pub convergence: ConvergenceState,
}
```

### Deliverable
```
repl/agent/
  ├── loop_fsm.rs                    (NEW — ~50 LOC)
  ├── round_metrics.rs               (NEW — ~40 LOC)
  ├── plan_state.rs                  (NEW — ~30 LOC)
  ├── convergence_state.rs           (NEW — ~30 LOC)
  ├── loop_state.rs                  (MODIFIED → eventually replaced)
  └── loop_state_roles.rs            (DEPRECATED → removed after migration)
```

---

## Phase 3: Repl Thinning (ReplBuilder)

**Goal**: Extract factory logic from Repl, reduce from 43 to ≤12 fields.
**Files touched**: `repl/mod.rs`, new `repl/builder.rs`
**Risk**: Low — pure structural extraction, no behavior change

### Step 3.1: Create ReplBuilder

Location: `crates/halcon-cli/src/repl/builder.rs`

Move all component construction from `Repl::new()` / `Repl::init()` into:
```rust
impl ReplBuilder {
    pub fn build(self) -> Result<Repl>;
    fn build_context_manager(&self) -> Result<ContextManager>;
    fn build_execution_engine(&self) -> Result<ExecutionEngine>;
    fn build_intelligence_layer(&self) -> Result<IntelligenceLayer>;
    fn build_extensions(&self) -> Result<ExtensionManager>;
    fn build_observability(&self) -> Result<ObservabilityHub>;
}
```

### Step 3.2: Group Fields into Component Structs

Create wrapper structs for field groups:
```rust
pub struct ExecutionEngine {
    pub executor: ToolExecutor,
    pub permission_pipeline: PermissionPipeline,  // from Phase 1
    pub registry: ToolRegistry,
    pub sandbox: SandboxPolicy,
}

pub struct IntelligenceLayer {
    pub model_router: ModelRouter,
    pub reasoning_engine: ReasoningEngine,
    pub speculator: Speculator,
    pub playbook_planner: PlaybookPlanner,
    pub model_quality_cache: ModelQualityCache,
}

pub struct ExtensionManager {
    pub plugin_registry: PluginRegistry,
    pub mcp_manager: McpManager,
    pub plugin_transport: PluginTransportRuntime,
}
```

### Step 3.3: Migrate Access Sites

For each field moved:
1. `self.field_x` → `self.execution_engine.field_x`
2. All callers updated
3. Tests pass

### Deliverable
```
repl/
  ├── mod.rs                         (MODIFIED — 43 fields → ≤12)
  ├── builder.rs                     (NEW — ~300 LOC)
  └── ... (everything else unchanged)
```

---

## Phase 4: Streaming Tool Executor

**Goal**: Start tool execution during model streaming, not after.
**Files touched**: new `executor/streaming.rs`, `agent/mod.rs` integration
**Risk**: Medium — new execution path, must coexist with batch executor

### Step 4.1: Write Tests First

```rust
#[tokio::test]
async fn test_streaming_executor_starts_safe_tools_immediately() {
    // Send 3 read-only tool blocks with delays
    // Verify all 3 started before stream completion
}

#[tokio::test]
async fn test_streaming_executor_queues_unsafe_tool() {
    // Send 1 read-only + 1 destructive tool
    // Verify destructive waits for read-only to complete
}

#[tokio::test]
async fn test_streaming_executor_sibling_error_cascading() {
    // Tool A fails → Tool B (pending) gets synthetic error
}

#[tokio::test]
async fn test_streaming_executor_results_in_order() {
    // Tool B completes before Tool A
    // Results returned in original [A, B] order
}

#[tokio::test]
async fn test_streaming_executor_fallback_to_batch() {
    // Feature flag disabled → falls back to existing batch execution
}
```

### Step 4.2: Implement StreamingToolExecutor

Location: `crates/halcon-cli/src/repl/executor/streaming.rs`

Feature gate: `#[cfg(feature = "streaming-executor")]`

Core implementation:
- `mpsc::channel` receives tool blocks from streaming response handler
- `process_queue()` starts eligible tools using existing `execute_tool_pipeline()`
- `CancellationToken` for sibling error cascading
- Results collected in original order via indexed `Vec<Option<ToolResult>>`

### Step 4.3: Integrate with Agent Loop

In `agent/provider_round.rs` (or equivalent streaming handler):
```rust
// Current: collect all tool blocks, then execute
// New: feed tool blocks to StreamingToolExecutor as they arrive

#[cfg(feature = "streaming-executor")]
{
    while let Some(chunk) = stream.next().await {
        if let ContentBlock::ToolUse(block) = chunk {
            streaming_executor.add_tool(block);
        }
    }
    let results = streaming_executor.collect_results().await;
}

#[cfg(not(feature = "streaming-executor"))]
{
    // Existing batch execution path (unchanged)
}
```

### Step 4.4: Benchmark

- Measure round latency with and without streaming executor
- Expect improvement proportional to (stream_duration × tool_count)
- No regression on single-tool rounds

### Deliverable
```
repl/executor/
  ├── streaming.rs                   (NEW — ~350 LOC, feature-gated)
  ├── mod.rs                         (MODIFIED — re-export streaming)
  └── ... (existing files unchanged)
```

---

## Phase 5: AgentContext Refinement

**Goal**: Split 36-field bag into required + optional groups.
**Files touched**: `agent/mod.rs`, `agent/context.rs`
**Risk**: Low — structural grouping, no logic change

### Step 5.1: Group by Optionality

```rust
/// Always required for agent loop execution.
pub struct AgentCoreContext<'a> {
    pub provider: &'a dyn ModelProvider,
    pub session: &'a Session,
    pub tool_registry: &'a ToolRegistry,
    pub permissions: &'a PermissionPipeline,  // from Phase 1
    pub event_tx: &'a EventSender,
    pub render_sink: &'a dyn RenderSink,
    pub limits: AgentLimits,
    pub working_dir: &'a Path,
}

/// Optional capabilities — feature-dependent.
pub struct AgentCapabilities<'a> {
    pub planner: Option<&'a dyn Planner>,
    pub compactor: Option<&'a ContextCompactor>,
    pub reflector: Option<&'a Reflector>,
    pub context_manager: Option<&'a ContextManager>,
    pub speculator: Option<&'a Speculator>,
    pub plugin_registry: Option<&'a PluginRegistry>,
    pub task_bridge: Option<&'a TaskBridge>,
}

/// Configuration — immutable for the duration of the loop.
pub struct AgentConfig {
    pub routing_config: RoutingConfig,
    pub planning_config: PlanningConfig,
    pub security_config: SecurityConfig,
    pub is_sub_agent: bool,
    pub tool_selection_enabled: bool,
}

/// Replaces AgentContext — grouped by purpose.
pub struct AgentContext<'a> {
    pub core: AgentCoreContext<'a>,
    pub capabilities: AgentCapabilities<'a>,
    pub config: AgentConfig,
}
```

### Step 5.2: Migrate `from_parts()` Constructor

Update the existing constructor to build grouped sub-structs.

### Deliverable
```
repl/agent/
  ├── context.rs                     (MODIFIED — grouped sub-structs)
  └── mod.rs                         (MODIFIED — uses grouped access)
```

---

## Phase 6: Query-Level Recovery Patterns

**Goal**: Adopt Xiyo's recovery patterns for maxOutputTokens and promptTooLong.
**Files touched**: `agent/mod.rs`, new recovery logic
**Risk**: Low — additive, feature-gated

### Step 6.1: maxOutputTokens Recovery

```rust
pub struct OutputRecovery {
    recovery_count: u32,
    max_recoveries: u32,  // default: 3
}

impl OutputRecovery {
    /// Called when provider returns max_output_tokens stop reason.
    pub fn attempt_recovery(&mut self, state: &mut AgentLoopState) -> RecoveryAction {
        if self.recovery_count >= self.max_recoveries {
            return RecoveryAction::ForceStop;
        }
        self.recovery_count += 1;
        RecoveryAction::TruncateThinking { 
            reduce_output_limit_by: 1024 
        }
    }
}
```

### Step 6.2: promptTooLong Recovery

```rust
/// Triggered when context exceeds provider limits.
pub fn handle_prompt_too_long(
    context_manager: &ContextManager,
    state: &mut AgentLoopState,
) -> RecoveryAction {
    // Attempt reactive compaction (existing L0-L4 pipeline)
    if context_manager.can_compact() {
        return RecoveryAction::ReactiveCompact;
    }
    RecoveryAction::ForceStop
}
```

### Deliverable
```
repl/agent/
  ├── recovery.rs                    (NEW — ~120 LOC)
  └── mod.rs                         (MODIFIED — recovery integration)
```

---

## Phase 7: Frontier Enhancements

**Goal**: Go beyond Xiyo's capabilities.
**Prerequisite**: Phases 1-6 complete.

### 7.1: Pluggable Failure Strategies

Extract `retry.rs` logic behind `FailureStrategy` trait:
```rust
pub trait FailureStrategy: Send + Sync {
    fn classify(&self, error: &ToolError) -> FailureClass;
    fn next_action(&self, class: FailureClass, attempt: u32) -> FailureAction;
}
```

Default implementation wraps existing 40+ pattern classifier. Custom strategies injectable at runtime.

### 7.2: Execution DAG Emission

Enhance `bridges/replay_executor.rs` to emit execution DAG per turn:
```rust
pub struct ExecutionDAG {
    nodes: Vec<ExecutionNode>,
    edges: Vec<(usize, usize)>,  // dependency edges
    metadata: TurnMetadata,
}
```

Enables time-travel debugging by replaying any DAG node.

### 7.3: Zero-Cost Tracing

Ensure all metric emission goes through `event_tx` channel:
- Remove any direct metric struct mutation in hot paths
- Observer tasks consume events asynchronously
- Pre-allocated buffers for common event types

### 7.4: Sibling Error Cascading

Part of Phase 4 (StreamingToolExecutor) — `CancellationToken` propagation across parallel tool batches.

---

## Risk Analysis

### What Could Break

| Phase | Risk | Mitigation |
|-------|------|------------|
| Phase 1 (Permissions) | Permission behavior changes subtly | Regression tests capturing every current path |
| Phase 2 (LoopState) | Consumer code accesses wrong sub-struct | Bridge methods preserve old API during migration |
| Phase 3 (ReplBuilder) | Constructor wiring order matters | Builder methods have explicit dependency ordering |
| Phase 4 (Streaming) | Race conditions in concurrent tool execution | Feature-gated; existing batch path unchanged as fallback |
| Phase 4 (Streaming) | Tool ordering assumptions violated | Results always returned in original order |
| Phase 5 (AgentContext) | Optional capabilities accessed without check | `Option<>` forces explicit handling (compiler enforces) |
| Phase 6 (Recovery) | Over-aggressive recovery masks real errors | Recovery counter with strict limit (3 max) |

### What Could Regress

| Concern | Current Metric | Guard Rail |
|---------|---------------|------------|
| Permission latency | ~0ms (in-process check) | Benchmark test: permission check < 1ms |
| Tool execution latency | Current batch time | Benchmark: streaming ≤ batch (never worse) |
| Memory usage | Current baseline | Profiling: no new allocations in hot path |
| Test suite pass rate | 100% | CI gate: all tests must pass per phase |

### Dependencies Between Phases

```
Phase 1 ──→ Phase 3 (Repl uses PermissionPipeline)
         ──→ Phase 4 (StreamingExecutor uses PermissionPipeline)
         ──→ Phase 5 (AgentContext references PermissionPipeline)

Phase 2 ──→ Phase 5 (AgentContext uses refined LoopState)

Phase 3 ──→ (none, independently shippable)

Phase 4 ──→ Phase 7.4 (sibling cascading is part of streaming)

Phase 5 ──→ (none, independently shippable after Phase 1+2)

Phase 6 ──→ (none, independently shippable)

Phase 7 ──→ Phases 1-6 complete
```

**Critical path**: Phase 1 → Phase 4 (permissions must unify before streaming executor can use them)

---

## Implementation-Ready Module Structure

After all phases complete:

```
crates/halcon-cli/src/repl/
├── mod.rs                          (THINNED — ≤12 fields)
├── builder.rs                      (NEW — factory)
├── permission_pipeline.rs          (NEW — unified permissions)
│
├── agent/
│   ├── mod.rs                      (REFINED — uses AgentContext groups)
│   ├── context.rs                  (REFINED — core + capabilities + config)
│   ├── loop_fsm.rs                 (NEW — typed FSM)
│   ├── round_metrics.rs            (NEW — per-round stats)
│   ├── plan_state.rs               (NEW — planning state)
│   ├── convergence_state.rs        (NEW — convergence signals)
│   ├── recovery.rs                 (NEW — query-level recovery)
│   ├── accumulator.rs              (existing)
│   ├── budget_guards.rs            (existing)
│   ├── checkpoint.rs               (existing)
│   ├── convergence_phase.rs        (existing)
│   ├── failure_tracker.rs          (existing)
│   ├── feedback_arbiter.rs         (existing)
│   ├── loop_events.rs              (existing)
│   ├── planning_policy.rs          (existing)
│   ├── post_batch.rs               (existing)
│   ├── provider_client.rs          (existing)
│   ├── provider_round.rs           (MODIFIED — streaming integration)
│   ├── repair.rs                   (existing)
│   ├── result_assembly.rs          (existing)
│   ├── round_setup.rs              (existing)
│   ├── setup.rs                    (existing)
│   ├── simplified_loop.rs          (existing)
│   ├── tool_executor.rs            (existing)
│   └── tests.rs                    (EXPANDED)
│
├── executor/
│   ├── mod.rs                      (MODIFIED — routes through permission_pipeline)
│   ├── parallel.rs                 (existing)
│   ├── sequential.rs               (MODIFIED — routes through permission_pipeline)
│   ├── validation.rs               (existing)
│   ├── retry.rs                    (existing, optionally behind FailureStrategy trait)
│   ├── hooks.rs                    (existing)
│   └── streaming.rs                (NEW — StreamingToolExecutor)
│
├── security/
│   ├── denial_tracker.rs           (NEW)
│   └── ... (existing, unchanged)
│
├── orchestrator.rs                 (existing, unchanged)
├── context/                        (existing, unchanged)
├── metrics/                        (existing, unchanged)
├── bridges/                        (existing, unchanged)
├── planning/                       (existing, unchanged)
├── plugins/                        (existing, unchanged)
├── domain/                         (existing, unchanged)
└── ... (all other existing modules unchanged)
```

**New files**: 9
**Modified files**: ~6
**Deleted files**: 1 (`loop_state_roles.rs` after Phase 2 complete)
**Unchanged files**: ~155

---

## Success Criteria

| Metric | Current | Target | Measurement |
|--------|---------|--------|-------------|
| Repl struct fields | 43 | ≤12 | `grep` struct fields |
| LoopState fields | 50+ | 4 sub-structs, ≤15 each | `grep` struct fields |
| Permission entry points | 2+ | 1 | Code audit |
| Tool execution start | After stream completes | During streaming | Latency benchmark |
| Denial tracking | None | Counter + escalation | Unit tests |
| Test coverage | Existing | Existing + ~40 new tests | CI report |
| Behavior drift | — | Zero | All existing tests pass |
