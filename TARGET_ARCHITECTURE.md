# HALCON REPL — Target Architecture

**Date**: 2026-04-02
**Status**: Design specification (pre-implementation)
**Constraint**: Incremental migration, zero behavior drift, backward compatible

---

## 1. Design Philosophy

The target architecture evolves HALCON's existing layered system — it does NOT replace it. The C-1→C-8 refactoring already created the right directory structure. What remains is:

1. **Seal the seams** — unify fragmented concerns (permissions, failure handling)
2. **Thin the coordinators** — decompose Repl and LoopState
3. **Enable streaming execution** — add StreamingToolExecutor alongside existing batch executor
4. **Formalize what's implicit** — type-state LoopState, pipeline permissions

---

## 2. Target Component Structure

```
Repl (thin coordinator — session lifecycle + I/O only)
  │
  ├── ReplBuilder (factory — extracted from Repl constructor)
  │     └── Builds all components, returns configured Repl
  │
  ├── QueryEngine (message processing pipeline)
  │     ├── Receives user message
  │     ├── Assembles context (delegates to ContextManager)
  │     ├── Routes to model (delegates to ModelRouter)
  │     └── Returns to agent loop
  │
  ├── ExecutionEngine (tool execution — existing executor/ enhanced)
  │     ├── BatchExecutor (current: parallel.rs + sequential.rs)
  │     ├── StreamingExecutor (NEW: starts tools during streaming)
  │     ├── PermissionPipeline (NEW: unified permission decisions)
  │     ├── FailureHandler (existing failure_handler/ + retry.rs)
  │     └── HookRunner (existing hooks.rs)
  │
  ├── IntelligenceLayer (agent loop + planning — existing agent/ refined)
  │     ├── AgentLoop (FSM with typed LoopState)
  │     ├── PlanningEngine (existing planning/)
  │     ├── ConvergenceOracle (existing convergence logic)
  │     └── Orchestrator (existing orchestrator.rs)
  │
  ├── ExtensionManager (plugins + MCP — existing plugins/ unified)
  │     ├── PluginRegistry (existing)
  │     ├── MCPBridge (existing bridges/cenzontle_mcp_bridge.rs)
  │     └── ToolDiscovery (MCP + plugin tool resolution)
  │
  └── ObservabilityHub (existing metrics/ + bridges/ consolidated)
        ├── MetricsPipeline (existing metrics/)
        ├── TraceRecorder (existing trace_recording/)
        ├── ExecutionTracker (existing bridges/execution_tracker.rs)
        └── RuntimeEvents (existing runtime_events.rs)
```

### Key Invariants

1. **Each component has ONE public entry method** (or a small, cohesive API)
2. **Components communicate via typed events**, not direct field access
3. **No component holds > 15 fields** (current Repl: 43, target: ≤12)
4. **All components are independently testable** via trait injection

---

## 3. Component Specifications

### 3.1 Repl (Thin Coordinator)

**Responsibility**: Session lifecycle, user I/O routing, slash command dispatch.

**Target fields** (≤12):
```rust
pub struct Repl {
    // Session
    session: Session,
    config: AppConfig,
    
    // I/O
    editor: Reedline,             // or TUI handle
    render_sink: Box<dyn RenderSink>,
    
    // Components (built by ReplBuilder)
    query_engine: QueryEngine,
    execution_engine: ExecutionEngine,
    intelligence: IntelligenceLayer,
    extensions: ExtensionManager,
    observability: ObservabilityHub,
    
    // Communication
    event_tx: EventSender,
    ctrl_rx: Option<Receiver<ControlEvent>>,
    
    // Mode flags
    mode: ReplMode,  // enum { Classic, Tui, SinglePrompt, JsonRpc, CI }
}
```

**What moves out**:
- Model quality cache → `IntelligenceLayer`
- Plugin registry → `ExtensionManager`
- Context manager → `QueryEngine`
- Reasoning engine → `IntelligenceLayer`
- Trace cursor → `ObservabilityHub`
- Speculation → `IntelligenceLayer`
- MCP manager → `ExtensionManager`
- 31 other fields → distributed to owning components

**Migration**: Extract fields into components one-by-one. Each extraction is a single commit that:
1. Moves field to component
2. Updates all access sites to go through component
3. Passes existing tests

### 3.2 ReplBuilder (Factory)

**Responsibility**: Construct all components from config + environment.

```rust
pub struct ReplBuilder {
    config: AppConfig,
    db: Database,
    async_db: AsyncDatabase,
}

impl ReplBuilder {
    pub fn new(config: AppConfig, db: Database, async_db: AsyncDatabase) -> Self;
    
    pub fn build(self) -> Result<Repl> {
        let observability = self.build_observability()?;
        let extensions = self.build_extensions()?;
        let execution_engine = self.build_execution_engine(&extensions)?;
        let intelligence = self.build_intelligence()?;
        let query_engine = self.build_query_engine()?;
        
        Ok(Repl {
            session: self.init_session()?,
            config: self.config,
            query_engine,
            execution_engine,
            intelligence,
            extensions,
            observability,
            // ... minimal remaining fields
        })
    }
}
```

**Benefit**: New capabilities only require changes in `ReplBuilder::build_*()` — not in `Repl` itself.

### 3.3 PermissionPipeline (NEW — Closes Gap G1)

**Responsibility**: Single authority for all permission decisions.

```rust
/// Single entry point for ALL permission checks in the system.
pub struct PermissionPipeline {
    rules: PermissionRules,          // allow/deny/ask rule sets
    blacklist: Blacklist,            // dangerous command patterns
    tool_policy: ToolPolicyClassifier, // permission level classification
    trust_scorer: ToolTrustScorer,   // per-tool reputation
    denial_tracker: DenialTracker,   // NEW: denial counter + escalation
    hook_runner: HookRunner,         // pre/post permission hooks
}

impl PermissionPipeline {
    /// THE permission check. All paths call this. No exceptions.
    pub async fn check(
        &self,
        tool: &dyn Tool,
        input: &ToolInput,
        context: &PermissionContext,
    ) -> PermissionDecision {
        // Phase 1: Rule check (deny rules → allow rules → ask rules)
        if let Some(deny) = self.rules.check_deny(tool, input) {
            self.denial_tracker.record(tool.name());
            return PermissionDecision::Deny(deny.reason);
        }
        if let Some(allow) = self.rules.check_allow(tool, input) {
            self.denial_tracker.record_success(tool.name());
            return PermissionDecision::Allow(allow.reason);
        }
        
        // Phase 2: Security gates (blacklist, risk, trust)
        if self.blacklist.matches(tool, input) {
            return PermissionDecision::Deny(Reason::Blacklisted);
        }
        let risk = self.tool_policy.classify(tool, input);
        let trust = self.trust_scorer.score(tool);
        
        // Phase 3: Hook check
        if let Some(hook_decision) = self.hook_runner.pre_permission(tool, input).await? {
            return hook_decision;
        }
        
        // Phase 4: Ask user (with denial escalation)
        if self.denial_tracker.should_fallback_to_prompting(tool.name()) {
            return PermissionDecision::Ask {
                reason: Reason::DenialThresholdReached,
                risk_level: risk,
            };
        }
        
        PermissionDecision::Ask { reason: Reason::Policy(risk), risk_level: risk }
    }
}

pub enum PermissionDecision {
    Allow(Reason),
    Deny(Reason),
    Ask { reason: Reason, risk_level: RiskLevel },
}
```

**Migration**:
1. Create `permission_pipeline.rs` with the struct
2. Route `executor/sequential.rs` permission checks through it
3. Route `security/conversational.rs` checks through it
4. Remove direct permission logic from executor
5. All tests should pass unchanged (behavior preserved)

### 3.4 StreamingToolExecutor (NEW — Closes Gap G2)

**Responsibility**: Execute tools as their blocks arrive during model streaming.

```rust
/// Receives tool blocks from streaming channel, executes eagerly.
pub struct StreamingToolExecutor {
    tool_rx: mpsc::Receiver<StreamedToolBlock>,
    result_tx: mpsc::Sender<OrderedToolResult>,
    executor: Arc<ExecutionEngine>,
    permission_pipeline: Arc<PermissionPipeline>,
    
    // Concurrency control
    active: Vec<JoinHandle<()>>,
    pending_queue: VecDeque<TrackedTool>,
    has_errored: AtomicBool,
    sibling_cancel: CancellationToken,
}

struct TrackedTool {
    id: String,
    block: ToolUseBlock,
    status: ToolStatus,
    is_concurrency_safe: bool,
    result: Option<ToolResult>,
}

enum ToolStatus {
    Queued,
    Executing,
    Completed,
    Yielded,
}

impl StreamingToolExecutor {
    /// Called as each tool_use block arrives from the model stream.
    pub fn add_tool(&mut self, block: ToolUseBlock) {
        let is_safe = self.executor.is_concurrency_safe(&block);
        self.pending_queue.push_back(TrackedTool {
            id: block.id.clone(),
            block,
            status: ToolStatus::Queued,
            is_concurrency_safe: is_safe,
            result: None,
        });
        self.process_queue();
    }
    
    /// Start queued tools respecting concurrency rules.
    fn process_queue(&mut self) {
        while let Some(tool) = self.next_eligible() {
            let handle = tokio::spawn(self.execute_tool(tool));
            self.active.push(handle);
        }
    }
    
    fn can_execute(&self, is_safe: bool) -> bool {
        self.active.is_empty() 
            || (is_safe && self.all_active_are_safe())
    }
    
    /// Collect results in original order after stream ends.
    pub async fn collect_results(self) -> Vec<OrderedToolResult> {
        // Join all active handles
        // Return results sorted by original tool order
    }
    
    /// Sibling error cascading: abort pending tools on failure.
    fn on_tool_error(&self, failed_tool: &str) {
        self.has_errored.store(true, Ordering::SeqCst);
        self.sibling_cancel.cancel();
    }
}
```

**Integration with existing executor**:
- `StreamingToolExecutor` wraps the existing `execute_tool_pipeline()` — it doesn't replace it
- Feature-gated behind `streaming-executor` flag initially
- Fallback to current batch execution if streaming executor is disabled
- The 9-stage pipeline remains intact; only the scheduling changes

### 3.5 Typed LoopState (Closes Gap G3)

**Responsibility**: Replace 50+ field bag with focused sub-structs.

```rust
/// Core loop FSM — ONLY state machine transitions.
pub struct LoopFSM {
    phase: LoopPhase,
    round: u32,
    max_rounds: u32,
    transition: Option<LoopTransition>,
    is_sub_agent: bool,
}

pub enum LoopPhase {
    Setup,
    AwaitingProvider,
    ExecutingTools,
    Converging,
    Synthesizing,
    Complete,
}

/// Per-round metrics — reset each round, accumulated in session metrics.
pub struct RoundMetrics {
    tokens_input: u64,
    tokens_output: u64,
    tools_called: u32,
    tools_failed: u32,
    cost_usd: f64,
    duration: Duration,
}

/// Planning state — optional, only present when planning is active.
pub struct PlanState {
    task_analysis: Option<TaskAnalysis>,
    active_plan: Option<Plan>,
    plan_steps_completed: u32,
    evidence: Vec<Evidence>,
}

/// Convergence signals — derived from other state, minimal storage.
pub struct ConvergenceState {
    end_turn_received: bool,
    guard_limit_hit: Option<GuardLimit>,
    quality_score: f64,
    synthesis_triggered: bool,
}

/// Replaces LoopState — composition of focused sub-structs.
pub struct AgentLoopState {
    pub fsm: LoopFSM,
    pub metrics: RoundMetrics,
    pub session_metrics: SessionMetrics,  // accumulated across rounds
    pub plan: Option<PlanState>,
    pub convergence: ConvergenceState,
}
```

**Migration**:
1. Create the sub-structs alongside existing `LoopState`
2. Add `impl From<&LoopState> for AgentLoopState` conversion
3. Migrate consumers one-by-one to use sub-structs
4. When all consumers migrated, remove original `LoopState`

---

## 4. Communication Model

### Current: Direct Field Access
```
Repl.field_x → AgentContext.field_x → agent_loop uses field_x
```
Problem: every field addition requires threading through all layers.

### Target: Typed Event Bus + Trait Injection
```
Repl dispatches message → QueryEngine processes → AgentLoop executes
                                                       │
                                    event_tx.send(ToolExecuted { .. })
                                                       │
                                    ObservabilityHub receives → metrics, traces
```

**Event types** (extend existing `DomainEvent`):
```rust
pub enum ExecutionEvent {
    // Existing events preserved
    ToolExecuted { tool: String, duration: Duration, success: bool },
    PermissionRequested { tool: String, level: RiskLevel },
    PermissionDecided { tool: String, decision: PermissionDecision },
    
    // New events
    StreamingToolStarted { tool: String, tool_use_id: String },
    SiblingErrorCascade { failed_tool: String, cancelled_count: u32 },
    RoundCompleted { metrics: RoundMetrics },
    ConvergenceSignal { reason: ConvergenceReason },
}
```

---

## 5. Testing Strategy

### Per-Component Test Surface

| Component | Test Strategy | Mock Boundary |
|-----------|--------------|---------------|
| `PermissionPipeline` | Unit test each phase independently | Mock `HookRunner`, `DenialTracker` |
| `StreamingToolExecutor` | Integration test with fake tools | Mock `ExecutionEngine` |
| `LoopFSM` | State transition property tests | Pure (no I/O) |
| `RoundMetrics` | Accumulation tests | Pure (no I/O) |
| `ReplBuilder` | Build with minimal config | Mock DB, providers |
| `QueryEngine` | End-to-end with mock provider | Mock `ModelProvider`, `ContextManager` |

### Regression Test Anchors

Every existing test must pass at each migration step. The migration is a sequence of:
```
existing test passes → extract component → existing test still passes
```

No test is deleted. New tests are added for new components.

---

## 6. Dependency Graph (Target)

```
┌──────────────┐
│   Repl (12)  │ ← Thin: session + I/O + dispatch
└──┬───────────┘
   │
   ├── QueryEngine
   │     ├── ContextManager (existing, unchanged)
   │     └── ModelRouter (existing model_selector)
   │
   ├── ExecutionEngine
   │     ├── BatchExecutor (existing executor/)
   │     ├── StreamingExecutor (NEW)
   │     ├── PermissionPipeline (NEW, consolidates security/)
   │     └── FailureHandler (existing)
   │
   ├── IntelligenceLayer
   │     ├── AgentLoop (existing agent/, refined LoopState)
   │     ├── PlanningEngine (existing planning/)
   │     ├── ConvergenceOracle (existing convergence logic)
   │     └── Orchestrator (existing orchestrator.rs)
   │
   ├── ExtensionManager
   │     ├── PluginRegistry (existing plugins/)
   │     └── MCPBridge (existing bridges/)
   │
   └── ObservabilityHub
         ├── MetricsPipeline (existing metrics/)
         ├── TraceRecorder (existing trace_recording/)
         └── ExecutionTracker (existing bridges/)
```

**Key property**: No component depends on `Repl`. Dependencies flow downward only. `Repl` is the only component that knows about all others.

---

## 7. What Does NOT Change

| Preserved System | Reason |
|-----------------|--------|
| `executor/` 5-file structure | Already correctly decomposed |
| `orchestrator.rs` | Clean, well-tested, minimal coupling |
| `context/` module | Well-isolated with governance |
| `security/` module internals | Only the entry point changes (via PermissionPipeline) |
| `metrics/` module | Already independent |
| `bridges/` module | Already independent |
| `planning/` module | Already independent |
| `domain/` module | Pure algorithms, no coupling |
| Trait-based testing | All existing mocks preserved |
| Feature flags | `gdem-primary`, `simplified-loop` etc. unchanged |
| Public API surface | All `pub use` re-exports maintained |

---

## 8. Frontier-Level Enhancements (Beyond Xiyo)

### 8.1 Deterministic Execution Graph

Each REPL turn produces a DAG of actions that can be:
- Inspected before execution (dry-run mode already exists)
- Replayed deterministically (replay_executor already exists)
- Compared across runs for regression detection

**Implementation**: Extend existing `bridges/replay_executor.rs` to emit DAG structure alongside execution. No new module needed — enhance existing.

### 8.2 Pluggable Failure Strategies

Currently, retry logic is hardcoded in `retry.rs`. Make it injectable:

```rust
pub trait FailureStrategy: Send + Sync {
    fn classify(&self, error: &ToolError) -> FailureClass;
    fn next_action(&self, class: FailureClass, attempt: u32) -> FailureAction;
}

pub enum FailureAction {
    Retry { delay: Duration, mutate_args: bool },
    Repair { repair_fn: Box<dyn FnOnce() -> Result<()>> },
    Fallback { alternative_tool: String },
    Surface { message: String },
}
```

**Integration**: `executor/retry.rs` already has the 40+ pattern classification. Wrap it in the `FailureStrategy` trait to make it pluggable without losing existing patterns.

### 8.3 Zero-Cost Observability

Current metrics module is comprehensive but synchronous in some paths. Target:
- All metric emission via `event_tx` (fire-and-forget channel)
- Observer tasks consume events asynchronously
- Zero allocations in hot path (pre-allocated metric buffers)

**Implementation**: Existing `event_tx: EventSender` already provides the channel. Ensure all metric recording goes through it rather than direct struct mutation.

### 8.4 Sibling Error Cascading

When tool A fails in a parallel batch, tools B and C should receive cancellation:

```rust
impl StreamingToolExecutor {
    fn on_tool_error(&self, failed: &str) {
        self.has_errored.store(true, Ordering::SeqCst);
        self.sibling_cancel.cancel(); // CancellationToken propagates to all siblings
        
        // Pending tools get synthetic error results
        for tool in &mut self.pending_queue {
            if tool.status == ToolStatus::Queued {
                tool.result = Some(ToolResult::SiblingError {
                    caused_by: failed.to_string(),
                });
                tool.status = ToolStatus::Completed;
            }
        }
    }
}
```

This is a Xiyo pattern that HALCON should adopt — it prevents wasted work when a parallel batch partially fails.
