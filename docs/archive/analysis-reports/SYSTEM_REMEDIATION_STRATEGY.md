# SYSTEM-LEVEL REMEDIATION STRATEGY — Halcon REPL Subsystem

> **Date**: 2026-04-02
> **Author**: Architecture Review (Deep Code Analysis)
> **Scope**: `crates/halcon-cli/src/repl/` — 4 god-files, 170 responsibilities, 15,120 LOC
> **Baseline**: Commit `63cd4d9` | Cross-referenced against Xiyo source

---

## 1. ROOT CAUSE ANALYSIS

### 1.1 Why Did the System Evolve Into God-Files?

The god-file pattern didn't emerge from carelessness — it's a predictable consequence of **four systemic forces**:

**Force 1: Accretive Feature Gating Without Structural Refactoring**

Each "Phase N" feature (94 phases identified in comments) was added inline with `if self.features.X.is_some()` guards. The pattern:
```
Phase N: Add feature → wrap in Option<> → add 30-60 LOC guard block → ship
```
This yields 60+ `Option<>` checks in `handle_message_with_sink()` alone. No phase triggered structural decomposition because each individual addition was "small enough."

**Force 2: The 42-Field AgentContext is a Dependency Magnet**

`AgentContext` (42 fields, 6 `&mut` references) acts as a **God Parameter Object**. When a new subsystem needs data from the REPL, the path of least resistance is:
```
1. Add field to AgentContext
2. Pass it through run_agent_loop()
3. Access in whichever phase function needs it
```
This avoids Rust's borrow checker complaints (which arise from passing individual references) at the cost of growing the context indefinitely. Every new field increases coupling between Repl construction and agent loop internals.

**Force 3: Rust's Borrow Checker Resists Natural Decomposition**

The 6 mutable references in AgentContext (`session`, `permissions`, `permission_pipeline`, `resilience`, `task_bridge`, `context_manager`) cannot be split into sub-structs without careful ownership redesign. The `from_parts()` workaround (line 186) groups immutable fields into 3 sub-structs while keeping mutables flat — a practical but incomplete solution that discourages further decomposition.

**Force 4: Vertical Integration as Performance Optimization**

`handle_message_with_sink()` mutates `self.runtime.guards` for load-once patterns (4 boolean flags). These guards prevent re-querying the database on every message. Extracting them into separate functions would require either:
- Passing `&mut RuntimeGuards` everywhere (borrow checker friction)
- Using `Cell<bool>` or `AtomicBool` (runtime cost for safety)

The path of least resistance: keep the guards inline, keep the function monolithic.

### 1.2 Broken Architectural Invariants

| Invariant | Expected | Actual | Severity |
|-----------|----------|--------|----------|
| **Single Responsibility** | 1 responsibility per module | 67 in mod.rs, 52 in agent/mod.rs | Critical |
| **Explicit Dependencies** | Functions declare what they need | AgentContext bundles 42 fields | Critical |
| **Side-Effect Isolation** | Side effects at boundaries | 4 `tokio::spawn` + 15+ `self.` mutations inline | High |
| **Testability** | Each concern independently testable | `handle_message_with_sink` requires full Repl construction | High |
| **Information Hiding** | Sub-structs encapsulate state | `self.infra` accessed 102 times (41.8% of all field access) | High |
| **Ownership Clarity** | Each piece of state has one owner | `plugin_registry` has dual-path: V3 + new `PluginSystem` | Medium |

### 1.3 Hidden Coupling Patterns

**Pattern 1: Infrastructure Gravity Well**

`self.infra` (8 fields: config, db, async_db, registry, tool_registry, event_tx, user_display_name, dev_gateway) is accessed 102 times — 41.8% of all field accesses. It's a **service locator** disguised as a struct. Every subsystem reaches through it to get config, database, or event bus.

**Pattern 2: Session as Mutable Accumulator**

`self.session` (72 accesses, 29.5%) is modified in three distinct ways:
1. User message recording (before agent loop)
2. Agent response accumulation (during loop, via `&mut` borrow)
3. Token counter updates (after loop)

These three mutation phases have no explicit boundary — they all operate on the same `&mut Session`.

**Pattern 3: Feature-Gated Initialization Cascade**

```
if self.features.plugin_registry.is_none() && config.plugins.enabled {
    // 60 LOC of plugin system initialization
    self.features.plugin_registry = Some(Arc::new(Mutex::new(registry)));
    self.features.plugin_transport_runtime = Some(Arc::new(runtime));
}
```
This pattern repeats for MCP, Cenzontle, reasoning engine, and model selector — lazy initialization scattered across the monolith function, each requiring mutable access to different `self` fields.

**Pattern 4: Control Channel Ownership Shell Game**

The TUI control channel (`ctrl_rx`) is `.take()`-ed from `self.runtime` before the agent loop and `.restore()`-ed after. This ownership dance prevents simultaneous borrows but creates a temporal coupling: if any code path between take and restore panics, the channel is lost.

### 1.4 Systemic Design Failures

1. **No Command/Query Separation**: `handle_message_with_sink()` both queries state (capabilities, database) and commands mutations (session, cache, guards). CQRS would split these concerns.

2. **Missing Domain Events for Internal Coordination**: The `event_tx` emits external events (ToolExecuted, ReasoningStarted) but internal coordination uses direct `&mut` access. There's no internal event bus for coordinating subsystem initialization.

3. **Configuration Coupling**: `self.infra.config` is a monolithic `AppConfig` read by every subsystem. There's no per-subsystem config extraction.

---

## 2. RESPONSIBILITY DECOMPOSITION MODEL

### 2.1 Formal Classification Taxonomy

Every responsibility in the 4 god-files falls into exactly one of 5 categories:

```
┌─────────────────────────────────────────────────────────┐
│                    ORCHESTRATION                         │
│  Coordinates execution order. No domain logic.           │
│  Pure control flow: sequence, branch, loop, dispatch.    │
│  Example: Phase dispatch, round loop, retry coordination │
├─────────────────────────────────────────────────────────┤
│                    DOMAIN LOGIC                          │
│  Implements business rules. Deterministic, testable.     │
│  No I/O, no state mutation beyond return value.          │
│  Example: Error classification, risk scoring, planning   │
├─────────────────────────────────────────────────────────┤
│                    SIDE EFFECTS                          │
│  I/O operations: network, filesystem, database.          │
│  Non-deterministic. Must be isolated for testing.        │
│  Example: DB saves, API calls, file reads, spawned tasks │
├─────────────────────────────────────────────────────────┤
│                    INFRASTRUCTURE                        │
│  Cross-cutting services. Lifecycle > request scope.      │
│  Example: Tracing, metrics, event bus, session mgmt      │
├─────────────────────────────────────────────────────────┤
│                    POLICY / PERMISSIONS                  │
│  Authorization decisions. Must be auditable.             │
│  Example: TBAC, blacklist, permission pipeline, risk     │
└─────────────────────────────────────────────────────────┘
```

### 2.2 Current → Target Module Mapping

#### mod.rs (67 responsibilities)

| Category | Count | Current Location | Target Module |
|----------|-------|-----------------|---------------|
| **Orchestration** | 12 | `handle_message_with_sink()` control flow | `message_pipeline.rs` |
| **Domain Logic** | 8 | Media extraction, context injection, request build | `message_pipeline/stages/*.rs` |
| **Side Effects** | 22 | DB loads, MCP init, plugin discovery, media API | `message_pipeline/side_effects.rs` + per-subsystem init |
| **Infrastructure** | 18 | Session persist, trace cache, telemetry spawn | `session_lifecycle.rs` + `tracing/recorder.rs` |
| **Policy** | 7 | Onboarding, plugin recommendation, model selection | `message_pipeline/policies.rs` |

#### executor.rs (30 responsibilities)

| Category | Count | Current Location | Target Module |
|----------|-------|-----------------|---------------|
| **Orchestration** | 4 | plan_execution, execute_one_tool | `executor/mod.rs` |
| **Domain Logic** | 10 | Transient/deterministic classification, validation, path resolution | `executor/validation.rs` + `executor/retry.rs` |
| **Side Effects** | 6 | Tool execution, DB recording, event emission | `executor/parallel.rs` + `executor/sequential.rs` |
| **Infrastructure** | 5 | Tracing, metrics, result construction | `executor/mod.rs` (helpers) |
| **Policy** | 5 | TBAC, permission handling, risk assessment, plugin gates | `security/permission_pipeline.rs` (already exists) |

#### agent/mod.rs (52 concerns)

| Category | Count | Current Location | Target Module |
|----------|-------|-----------------|---------------|
| **Orchestration** | 8 | Prologue phases, round dispatch, convergence | `agent/prologue.rs` + existing phase files |
| **Domain Logic** | 15 | Intent scoring, planning policy, tool selection, convergence | `agent/intent_pipeline.rs` + `agent/planning.rs` |
| **Side Effects** | 12 | Sub-agent delegation, DB loads, memory writes | `agent/delegation.rs` + `agent/epilogue.rs` |
| **Infrastructure** | 10 | LoopState init, FSM events, telemetry spans | `agent/loop_state.rs` (expand) |
| **Policy** | 7 | TBAC context push, tool enforcement, execution intent | `agent/tool_policy.rs` |

#### orchestrator.rs (21 responsibilities)

| Category | Count | Current Location | Target Module |
|----------|-------|-----------------|---------------|
| **Orchestration** | 5 | Wave dispatch, concurrency control, budget enforcement | `orchestrator/waves.rs` |
| **Domain Logic** | 6 | Topo sort, budget estimation, success classification | `orchestrator/topo.rs` + `orchestrator/budget.rs` |
| **Side Effects** | 4 | Sub-agent spawn, DB persistence, event emission | `orchestrator/spawn.rs` |
| **Infrastructure** | 3 | Shared context, panic isolation, event audit | `orchestrator/isolation.rs` |
| **Policy** | 3 | Tool surface narrowing, permission routing, role limits | `orchestrator/security.rs` |

---

## 3. TARGET ARCHITECTURE (NEXT-GEN)

### 3.1 Core Design Principle: Pipeline of Stages

Replace the monolithic `handle_message_with_sink()` with an **explicit pipeline** where each stage:
- Declares its inputs (typed struct)
- Declares its outputs (typed struct)
- Isolates its side effects behind trait boundaries
- Can be tested in isolation with mock inputs

```
┌─────────────────────────────────────────────────────────────────────┐
│                     MESSAGE PIPELINE                                 │
│                                                                      │
│  UserInput                                                           │
│     │                                                                │
│     ▼                                                                │
│  ┌────────────────┐   ┌──────────────┐   ┌──────────────────────┐   │
│  │ 1. INTAKE      │──▶│ 2. CONTEXT   │──▶│ 3. PROVIDER RESOLVE  │   │
│  │ Guard checks   │   │ Assembly     │   │ Fallback chain       │   │
│  │ Plugin resume  │   │ System prompt│   │ Model selection      │   │
│  │ Record message │   │ Media inject │   │ Planner resolution   │   │
│  │ Media extract  │   │ Dev context  │   │                      │   │
│  └────────────────┘   └──────────────┘   └──────────────────────┘   │
│                                                 │                    │
│                                                 ▼                    │
│  ┌──────────────────────┐   ┌──────────────────────────────────┐    │
│  │ 5. POST-PROCESSING   │◀──│ 4. AGENT LOOP                   │    │
│  │ Critic retry decision│   │ run_agent_loop(ctx)              │    │
│  │ Quality recording    │   │ (existing phase-driven pipeline) │    │
│  │ Plugin metrics       │   │                                  │    │
│  │ Memory consolidation │   └──────────────────────────────────┘    │
│  │ Session persistence  │                                           │
│  └──────────────────────┘                                           │
└─────────────────────────────────────────────────────────────────────┘
```

### 3.2 Core Abstractions

```rust
/// A pipeline stage that transforms input into output.
/// Pure stages have no side effects; effectful stages declare them.
trait PipelineStage {
    type Input;
    type Output;
    type Error;
    
    async fn execute(&self, input: Self::Input) -> Result<Self::Output, Self::Error>;
}

/// Replaces the 42-field AgentContext with focused capability slices.
/// Each phase function receives ONLY the slice it needs.
struct ProviderSlice<'a> {
    provider: &'a Arc<dyn ModelProvider>,
    fallbacks: &'a [(String, Arc<dyn ModelProvider>)],
    routing: &'a RoutingConfig,
}

struct SecuritySlice<'a> {
    permissions: &'a mut ConversationalPermissionHandler,
    pipeline: &'a mut PermissionPipeline,
    guardrails: &'a [Box<dyn Guardrail>],
}

struct ObservabilitySlice<'a> {
    trace_db: Option<&'a AsyncDatabase>,
    event_tx: &'a EventSender,
    recorder: &'a TraceRecorder,
    metrics: &'a Arc<ContextMetrics>,
}

/// Thin coordinator — delegates everything to sub-structs.
/// Max 5-7 fields (matches Xiyo's QueryEngine decomposition pattern
/// while maintaining Rust ownership semantics).
pub struct Repl {
    engine: QueryEngine,           // session, messages, turn_count, provider/model
    execution: ExecutionEngine,    // executor, permission pipeline, sandbox
    intelligence: IntelligenceLayer, // planner, classifier, convergence, reasoning
    extensions: ExtensionManager,  // MCP, plugins, hooks, skills
    observability: ObservabilityHub, // TraceRecorder, metrics, event_bus
}
```

### 3.3 Execution Model

The system uses three distinct execution models, each appropriate for its domain:

| Layer | Model | Pattern | Rationale |
|-------|-------|---------|-----------|
| Message Pipeline | **Sequential stages** | Stage 1 → 2 → 3 → 4 → 5 | Each stage depends on previous output |
| Agent Loop | **Phase-driven pipeline** (existing) | RoundSetup → ProviderRound → PostBatch → Convergence | Already well-designed, keep as-is |
| Tool Execution | **Partitioned concurrency** | ReadOnly → parallel, Destructive → sequential | Already implemented in executor |
| Observability | **Fire-and-forget** | `mpsc::UnboundedSender` | Non-blocking, never on critical path |

### 3.4 Data Flow Contracts

```
IntakeOutput {
    user_message: ChatMessage,      // Recorded in session
    media_paths: Vec<PathBuf>,      // Extracted, not yet analyzed
    guards_applied: GuardSnapshot,  // Which load-once checks ran
}

ContextOutput {
    system_prompt: String,          // Assembled from all sources
    media_context: Option<String>,  // Analyzed media descriptions
    model_request: ModelRequest,    // Ready for provider
    tools: Vec<ToolDefinition>,     // Filtered by intent
}

ProviderResolveOutput {
    provider: Arc<dyn ModelProvider>,
    fallbacks: Vec<(String, Arc<dyn ModelProvider>)>,
    planner: Option<Box<dyn Planner>>,
    model_selector: Option<ModelSelector>,
    agent_context: AgentContext,    // Assembled from all prior outputs
}

AgentLoopOutput {
    result: AgentLoopResult,        // Existing type
    blocked_tools: Vec<(String, String)>,
    timeline_json: Option<String>,
}

PostProcessOutput {
    retry_performed: bool,
    quality_recorded: bool,
    memory_consolidated: bool,
    session_saved: bool,
}
```

### 3.5 Where This Surpasses Xiyo

| Dimension | Xiyo | Halcon Target |
|-----------|------|---------------|
| **Modularity** | Single `query()` generator with 7 continue sites | 5 typed pipeline stages with explicit contracts |
| **Observability** | `logEvent()` fire-and-forget (untyped) | `TraceRecorder` with typed `TraceEvent` enum + structured spans |
| **Extensibility** | Hardcoded tool/hook integration | `ExtensionManager` with trait-based plugin/MCP/hook registration |
| **Error Recovery** | 4-stage context-overflow waterfall, no tool retry | 3-level circuit breaker (tool + provider + plan) with adaptive mutation |
| **Reasoning** | Turn count only | Multi-signal convergence (IntentScorer + BoundaryDecision + ConvergenceController + LoopCritic) |
| **State Management** | Flat 10-field State + 254-field AppState | 6 typed sub-states in LoopState + 5 Repl sub-structs |

---

## 4. CRITICAL P0 REMEDIATION PLAN

### 4.A handle_message_with_sink() → Message Pipeline

**Current**: 1,584 LOC monolith, 38+ inline responsibilities, 11 await points, 4 tokio::spawn

**Target**: 5 stages, each < 200 LOC, typed contracts between stages

#### Stage 1: Intake (lines 2305-2361 → `message_pipeline/intake.rs`)

```rust
pub struct IntakeStage<'a> {
    guards: &'a mut RuntimeGuards,
    session: &'a mut Session,
    plugin_system: &'a mut Option<PluginSystem>,
    config: &'a AppConfig,
}

pub struct IntakeOutput {
    media_paths: Vec<PathBuf>,
    onboarding_result: Option<OnboardingResult>,
    plugin_recommendations: Vec<PluginRecommendation>,
}

impl IntakeStage<'_> {
    pub async fn execute(&mut self, input: &str, sink: &dyn RenderSink) -> Result<IntakeOutput> {
        // 1. Onboarding check (guard-gated, ~13 LOC)
        // 2. Plugin resume (~7 LOC)
        // 3. Plugin recommendation (guard-gated, ~25 LOC)
        // 4. Record user message (~4 LOC)
        // 5. Extract media paths (~10 LOC)
    }
}
```

#### Stage 2: Context Assembly (lines 2363-2706 → `message_pipeline/context.rs`)

```rust
pub struct ContextStage<'a> {
    multimodal: &'a Option<Arc<MultimodalSubsystem>>,
    context_manager: &'a mut Option<ContextManager>,
    dev_gateway: &'a DevGateway,
    mcp_manager: &'a mut McpResourceManager,
    config: &'a AppConfig,
}

pub struct ContextOutput {
    system_prompt: String,
    media_context: Option<String>,
    model_request: ModelRequest,
}

impl ContextStage<'_> {
    pub async fn execute(
        &mut self, input: &str, session: &Session, 
        media_paths: &[PathBuf], sink: &dyn RenderSink,
    ) -> Result<ContextOutput> {
        // 1. Media analysis pipeline (5 phases, ~190 LOC → extract to media_analyzer.rs)
        // 2. MCP lazy init (~25 LOC)
        // 3. Cenzontle bridge (~30 LOC)
        // 4. System prompt assembly (~30 LOC)
        // 5. Context injection (user, dev, media) (~50 LOC)
        // 6. ModelRequest construction (~15 LOC)
    }
}
```

#### Stage 3: Provider Resolution (lines 2708-3162 → `message_pipeline/resolve.rs`)

```rust
pub struct ResolveStage<'a> {
    registry: &'a ProviderRegistry,
    config: &'a AppConfig,
    async_db: &'a Option<AsyncDatabase>,
    features: &'a mut ReplFeatures,
    cache: &'a mut ReplCacheState,
    runtime: &'a mut ReplRuntimeControl,
}

pub struct ResolveOutput {
    agent_context: AgentContext<'static>, // Fully assembled
}

impl ResolveStage<'_> {
    pub async fn execute(
        &mut self, context: ContextOutput, session: &mut Session,
        security: &mut ReplSecurity, sink: &dyn RenderSink,
    ) -> Result<ResolveOutput> {
        // 1. Provider lookup + fallback chain (~15 LOC)
        // 2. Planner resolution (~45 LOC)
        // 3. Model quality DB load (guard-gated, ~35 LOC)
        // 4. Plugin system init (guard-gated, ~60 LOC)
        // 5. Plugin UCB1 load (guard-gated, ~30 LOC)
        // 6. Model selector setup (~25 LOC)
        // 7. Task bridge init (~6 LOC)
        // 8. UCB1 experience load (guard-gated, ~50 LOC)
        // 9. Reasoning engine pre-loop (~45 LOC)
        // 10. AgentContext assembly (~70 LOC)
    }
}
```

#### Stage 4: Agent Execution (lines 3168-3191 → already `agent::run_agent_loop`)

No changes needed — this is already well-decomposed into the phase-driven pipeline.

#### Stage 5: Post-Processing (lines 3196-3884 → `message_pipeline/post_process.rs`)

```rust
pub struct PostProcessStage<'a> {
    features: &'a mut ReplFeatures,
    cache: &'a mut ReplCacheState,
    runtime: &'a ReplRuntimeControl,
    async_db: &'a Option<AsyncDatabase>,
    config: &'a AppConfig,
}

pub struct PostProcessOutput {
    retry_result: Option<AgentLoopResult>,
    quality_recorded: bool,
}

impl PostProcessStage<'_> {
    pub async fn execute(
        &mut self, result: AgentLoopResult, session: &mut Session,
        security: &mut ReplSecurity, sink: &dyn RenderSink,
    ) -> Result<PostProcessOutput> {
        // 1. Merge blocked tools (~10 LOC)
        // 2. Cache timeline (~5 LOC)
        // 3. Critic retry decision (~400 LOC → extract to critic_retry.rs)
        // 4. Model quality recording (~95 LOC → extract to quality_recorder.rs)
        // 5. Plugin metrics recording (~50 LOC)
        // 6. Playbook auto-learning (~25 LOC)
        // 7. Runtime signal ingest (~20 LOC, fire-and-forget)
        // 8. Token update (~3 LOC)
        // 9. Result summary display (~30 LOC)
        // 10. Memory consolidation (~40 LOC)
    }
}
```

**The new `handle_message_with_sink()` becomes a thin orchestrator:**

```rust
pub(crate) async fn handle_message_with_sink(
    &mut self, input: &str, sink: &dyn RenderSink,
) -> Result<()> {
    // Stage 1: Intake (~5 LOC delegation)
    let intake = IntakeStage::new(&mut self.runtime.guards, &mut self.session, ...);
    let intake_out = intake.execute(input, sink).await?;
    
    // Stage 2: Context (~5 LOC delegation)
    let ctx_stage = ContextStage::new(&self.features.multimodal, ...);
    let ctx_out = ctx_stage.execute(input, &self.session, &intake_out.media_paths, sink).await?;
    
    // Stage 3: Resolve (~5 LOC delegation)
    let resolve = ResolveStage::new(&self.infra.registry, ...);
    let resolve_out = resolve.execute(ctx_out, &mut self.session, &mut self.security, sink).await?;
    
    // Stage 4: Agent loop (~3 LOC)
    let result = agent::run_agent_loop(resolve_out.agent_context).await?;
    
    // Stage 5: Post-process (~5 LOC delegation)
    let post = PostProcessStage::new(&mut self.features, &mut self.cache, ...);
    post.execute(result, &mut self.session, &mut self.security, sink).await?;
    
    // Session auto-save (~3 LOC)
    self.auto_save_session();
    Ok(())
}
```

**Result**: ~30 LOC orchestrator replacing 1,584 LOC monolith. Each stage independently testable.

### 4.B agent/mod.rs Decomposition

The 52 concerns map to 7 extraction targets. Two are critical (highest LOC concentration):

**Extraction 1: `agent/prologue.rs` (~485 LOC)**

Lines 343-827: Everything before the main loop. Contains:
- Provider fallback warning (#1)
- Token attribution setup (#2)
- Context pipeline budget alignment (#3)
- Lifecycle FSM init (#4)
- Dry-run banner (#5)
- Intent scoring (#6)
- Adaptive planning policy (#7-9)
- Plan validation & compression (#9-10)
- TBAC context push (#10)
- Execution tracking (#11)
- Convergence detection setup (#12)
- Model-specific compaction (#13)
- Runtime model validation (#14)
- Intent-based tool selection (#15-16)
- Input normalization & boundary decision (#17)
- SLA budget derivation (#18)
- Intent pipeline reconciliation (#19)

```rust
pub(super) struct PrologueOutput {
    pub intent_profile: IntentProfile,
    pub planning_result: PlanningResult,
    pub effective_max_rounds: usize,
    pub cached_tools: Vec<ToolDefinition>,
    pub convergence_config: ConvergenceConfig,
    pub tbac_pushed: bool,
    pub boundary_decision: Option<BoundaryDecision>,
}

pub(super) async fn run_prologue(ctx: &mut AgentContext<'_>, sink: &dyn RenderSink) -> Result<PrologueOutput> {
    // All 19 concerns, in sequence
}
```

**Extraction 2: `agent/delegation.rs` (~595 LOC)**

Lines 1144-1739: Sub-agent delegation with contract validation. Contains:
- DelegationRouter evaluation (#26)
- Sub-agent spawning via orchestrator (#26)
- Contract validation (K6) (#26)
- RC-5 fix: include all sub-agents (#26)
- BRECHA-S1: evidence warnings (#26)
- Post-delegation tool policy (FASE 1) (#27)
- Core runtime tool protection (#27)
- Orchestrator failure recovery (H2) (#28)
- ExecutionIntentPhase derivation (#29)
- Pre-loop synthesis guard (3A) (#30)
- Autonomous agent directives (#31)

```rust
pub(super) struct DelegationOutput {
    pub sub_agent_results: Vec<SubAgentResult>,
    pub tool_policy_applied: bool,
    pub execution_intent: ExecutionIntentPhase,
    pub synthesis_guard_active: bool,
}

pub(super) async fn run_delegation(
    ctx: &mut AgentContext<'_>,
    plan: &ExecutionPlan,
    cached_tools: &mut Vec<ToolDefinition>,
    sink: &dyn RenderSink,
) -> Result<Option<DelegationOutput>> {
    // Returns None if delegation not applicable
}
```

### 4.C executor.rs — 5 Orphan Responsibility Assignment

| Responsibility | Lines | Assigned Module | Rationale |
|----------------|-------|----------------|-----------|
| **File edit diff preview** (UX-9) | 280-345 | `executor/preview.rs` | Pure rendering logic, no execution dependency |
| **Dynamic risk assessment** | 1320-1349 | Keep in `executor/sequential.rs` | Tightly coupled to permission flow in sequential execution |
| **Tracing & metrics recording** | 1029-1050, 1494-1586 | `executor/metrics.rs` | Extract into helper functions called from parallel.rs and sequential.rs |
| **Event bus emissions** | 1131-1136, 1552-1557 | Inline in `parallel.rs` and `sequential.rs` | 5-line snippets, extraction adds overhead without value |
| **Result construction helpers** | 118-135, 358-372, 991-1003 | `executor/mod.rs` (private functions) | Used by multiple submodules, best as shared helpers in the mod |

---

## 5. PERMISSION SYSTEM ANALYSIS

### 5.1 Current State (Already Better Than Plan Assumed)

**Critical finding**: The `PermissionPipeline` with `authorize_tool()` **already exists** at `security/permission_pipeline.rs` (lines 103-223). The Phase 2b plan to "create" it is unnecessary — it's already implemented.

Current 7-phase cascade (verified from code):

```
TBAC deny → Allow rules → Blacklist/G7 → Risk classify → Safety-sensitive → Denial tracking → Conversational
```

### 5.2 Comparison: Halcon (7 gates) vs Xiyo (3 gates)

| Gate | Halcon | Xiyo Equivalent |
|------|--------|-----------------|
| TBAC | Task context whitelist + budget | No equivalent |
| Allow rules | ReadOnly auto-allow | Implicit in `checkPermissions()` |
| Blacklist/G7 | Pattern matching on bash | No equivalent (relies on model alignment) |
| Risk classify | Additive scoring (≥50 blocks) | No equivalent |
| Safety-sensitive | Bypass-immune (.git/.ssh/.env) | Similar: `requiresInteraction` flag |
| Denial tracking | Escalation signaling | No equivalent |
| Conversational | Interactive prompt + TTL | `canUseTool()` + rule-based + hooks |

**Verdict**: Halcon's permission system is **more secure** than Xiyo's. The 7-gate cascade provides defense-in-depth that Xiyo lacks. **Do not simplify to 3 gates.**

### 5.3 Recommended Improvements (Not Redesign)

The permission system is well-designed. Minor improvements:

1. **Extract PluginPermissionGate into the pipeline**: Currently plugins have a separate `PluginPermissionGate` in `plugins/permission_gate.rs`. Integrate it as Gate 7.5 (after Conversational, before execution) for unified audit trail.

2. **Add structured audit log**: Each gate should emit a `PermissionAuditEntry` to `TraceRecorder`:
   ```rust
   struct PermissionAuditEntry {
       tool: String,
       gate: &'static str,
       decision: &'static str, // "allow", "deny", "pass", "ask"
       duration_us: u64,
       reason: Option<String>,
   }
   ```

3. **Consolidate `pre_invoke_gate` reference**: The plan references `plugin_registry.pre_invoke_gate` which doesn't exist. Plugin permission is handled via `PluginPermissionGate::evaluate()`. Update documentation.

---

## 6. ERROR HANDLING ARCHITECTURE

### 6.1 Current State Assessment

The error handling is **the strongest subsystem** in Halcon. It is already superior to Xiyo in every dimension:

| Capability | Halcon | Xiyo |
|-----------|--------|------|
| Tool retry | Exponential backoff + jitter + env repair + arg mutation | None |
| Error classification | Transient vs Deterministic (30+ patterns) | Single try-catch, classify for analytics only |
| Circuit breaker | Tool-level (3-strike) + Provider-level (ResilienceManager) | None |
| Plan integration | `FailedStepContext` injected into retry planner | No plan system |
| Graduated response | count=1 silent → count=2 suggest alt → count=3 stop | Immediate model retry |
| MCP failure isolation | Pattern detection → BreakLoop on all-mcp-unavailable | Not applicable |

### 6.2 Verdict: KEEP AND EVOLVE

The error waterfall is Halcon-original innovation. Do **not** replace it with Xiyo patterns.

### 6.3 Recommended Evolution

**Evolution 1: Typed Error Taxonomy**

Currently, error classification uses string matching. Migrate to typed enum:

```rust
pub enum ToolFailureKind {
    // Transient (may succeed on retry)
    NetworkTimeout { provider: String },
    RateLimit { retry_after: Option<Duration> },
    ServerError { status: u16 },
    ConnectionReset,
    ResourceContention { resource: String }, // cargo-lock, file lock
    McpTransport { server: String },
    
    // Deterministic (will never succeed)
    FileNotFound { path: PathBuf },
    PermissionDenied { path: Option<PathBuf> },
    InvalidSchema { field: String },
    AuthFailure { provider: String },
    McpInitFailure { server: String },
    UnknownTool { name: String },
    TbacDenied { context_id: Uuid },
    
    // Unknown
    Unclassified { message: String },
}

impl ToolFailureKind {
    pub fn is_transient(&self) -> bool { matches!(self, NetworkTimeout{..} | RateLimit{..} | ...) }
    pub fn is_deterministic(&self) -> bool { matches!(self, FileNotFound{..} | PermissionDenied{..} | ...) }
}
```

**Evolution 2: Propagation Rules**

```
Tool Failure
  ↓
┌──────────────────┐
│ classify()        │ → ToolFailureKind
└──────────────────┘
  ↓
┌──────────────────┐
│ retry.rs          │ → Retry(backoff) | GiveUp(kind)
│ - transient: retry│
│ - deterministic:  │
│   give up         │
│ - unknown: retry  │
│   once, then give │
│   up              │
└──────────────────┘
  ↓ GiveUp
┌──────────────────┐
│ post_batch.rs     │ → circuit_breaker.record()
│                   │ → execution_tracker.mark_failed()
│                   │ → failure_context for planner
└──────────────────┘
  ↓
┌──────────────────┐
│ convergence       │ → BreakLoop (all dead) | Continue (partial)
└──────────────────┘
```

**Evolution 3: Observability Hooks**

Every error transition should emit to `TraceRecorder`:
```rust
recorder.record(TraceEvent::ToolFailure {
    tool: name,
    kind: failure_kind,
    attempt: n,
    action: "retry" | "give_up" | "circuit_break",
    duration_ms: elapsed,
});
```

---

## 7. STREAMING & EXECUTION MODEL

### 7.1 Current State

Halcon's executor uses `buffer_unordered()` for parallel ReadOnly tools — this is correct for batch execution **after** model response completes.

Xiyo's `StreamingToolExecutor` adds a layer: tools execute **during** model streaming, not after. This requires:
1. Detecting complete tool_use blocks in partial stream chunks
2. Starting execution immediately (concurrent with continued streaming)
3. Order-preserving result emission (break at first non-concurrent incomplete tool)
4. Abort cascading on Bash errors

### 7.2 Recommended Design (Improving Over Xiyo)

```rust
/// Phase 1: Simple batch-after-stream (CURRENT — keep as default)
/// Phase 2: Concurrent-during-stream (NEW — opt-in via config)

pub struct StreamingExecutor {
    /// Concurrency semantics
    concurrent_tools: FuturesUnordered<Pin<Box<dyn Future<Output = ToolResult>>>>,
    exclusive_queue: VecDeque<ToolUse>,
    
    /// Abort hierarchy (3-level, matching Xiyo)
    session_cancel: CancellationToken,        // User interrupt
    sibling_cancel: CancellationToken,        // Bash error cascading
    // Per-tool cancel: child of sibling_cancel
    
    /// Backpressure (IMPROVEMENT over Xiyo)
    semaphore: Arc<Semaphore>,                // Max concurrent tools
    result_tx: mpsc::Sender<ExecutionEvent>,  // Bounded channel = backpressure
    
    /// Order preservation
    completed: Vec<Option<ToolResult>>,       // Indexed by submission order
    next_yield_idx: usize,                    // First un-yielded index
}

pub enum ExecutionEvent {
    Progress { tool_idx: usize, update: ProgressUpdate },
    Completed { tool_idx: usize, result: ToolResult },
    AllDone,
}

impl StreamingExecutor {
    /// Called as stream chunks arrive with complete tool_use blocks
    pub fn submit(&mut self, tool_use: ToolUse) {
        let idx = self.completed.len();
        self.completed.push(None);
        
        if tool_use.is_concurrent_safe() && !self.has_exclusive_running() {
            self.spawn_concurrent(idx, tool_use);
        } else {
            self.exclusive_queue.push_back(tool_use);
        }
    }
    
    /// Non-blocking: yields completed results in submission order
    pub fn drain_completed(&mut self) -> Vec<ToolResult> {
        let mut results = Vec::new();
        while self.next_yield_idx < self.completed.len() {
            match &self.completed[self.next_yield_idx] {
                Some(result) => {
                    results.push(result.clone());
                    self.next_yield_idx += 1;
                }
                None => break, // Order preservation: stop at first incomplete
            }
        }
        results
    }
    
    /// Blocking: wait for all remaining tools
    pub async fn finish(self) -> Vec<ToolResult> { /* ... */ }
}
```

**Improvements over Xiyo:**
1. **Bounded backpressure** via `Semaphore` + bounded `mpsc` (Xiyo uses unbounded)
2. **3-level cancellation** with explicit hierarchy (session → sibling → per-tool)
3. **Index-based order preservation** (cleaner than Xiyo's status-field tracking)
4. **Separate submit/drain** API (Xiyo conflates in single generator)

### 7.3 Implementation Priority

This is a **Phase 3 feature**, not P0. Current batch-after-stream works correctly. The streaming executor provides latency improvement for multi-tool responses but doesn't affect correctness.

---

## 8. MIGRATION STRATEGY

### 8.1 Phased Approach (Zero Breaking Changes)

```
Week 1-2: PHASE A — Extract message pipeline stages
  ├─ Create message_pipeline/ directory
  ├─ Extract IntakeStage (move code, keep calling convention)
  ├─ Extract ContextStage
  ├─ Extract ResolveStage
  ├─ Extract PostProcessStage
  ├─ Replace handle_message_with_sink() body with stage calls
  └─ cargo test --workspace (must pass with zero behavior changes)

Week 3: PHASE B — Extract agent prologue + delegation
  ├─ Create agent/prologue.rs
  ├─ Create agent/delegation.rs
  ├─ Move code, maintain exact same call signatures
  └─ cargo test --workspace

Week 4: PHASE C — Executor submodule completion
  ├─ Create executor/preview.rs (diff preview)
  ├─ Create executor/metrics.rs (tracing helpers)
  ├─ Verify all 30 responsibilities have a home
  └─ cargo test --workspace

Week 5: PHASE D — TraceRecorder consolidation
  ├─ Replace scattered trace recording with TraceRecorder::record()
  ├─ Add PermissionAuditEntry to pipeline
  └─ cargo test --workspace

Week 6: PHASE E — Final Repl decomposition
  ├─ Reorganize Repl into 5 sub-structs (engine, execution, intelligence, extensions, observability)
  ├─ Verify all public API unchanged
  └─ Full regression suite
```

### 8.2 Risk Analysis

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| Borrow checker rejects stage extraction | High | Medium | Use `from_parts()` pattern for multi-mut slices |
| Lifetime propagation through stages | Medium | High | Use owned types in stage outputs, references only for inputs |
| Phase ordering change introduces bug | Low | Critical | Diff test: capture `handle_message_with_sink()` outputs before/after |
| Plugin dual-path regression | Medium | Medium | Complete V3 migration in Phase C to eliminate dual-path |
| Guard state lost during extraction | Low | High | Unit test each guard independently |

### 8.3 Parallel Run Strategy

Not needed — this is a **pure refactoring** with no behavior changes. The migration is verified by:
1. `cargo check --workspace` after each move
2. `cargo test --workspace` after each phase
3. `cargo clippy --workspace -- -D warnings` after each phase
4. Manual smoke test of TUI + CLI modes after each phase

---

## 9. TIMELINE RECALIBRATION

### 9.1 Original vs Recalibrated Estimates

| Step | Original Estimate | Recalibrated | Delta | Reason |
|------|-------------------|-------------|-------|--------|
| 2a: executor.rs → executor/ | 2 weeks | 1 week | -1 week | **Already done** (executor/ subdirectory exists with parallel.rs, sequential.rs, validation.rs, retry.rs, hooks.rs) |
| 2b: Permission pipeline | 1 week | 2 days | -3 days | **Already exists** (permission_pipeline.rs with authorize_tool()) |
| 2c: Failure waterfall | 1 week | 3 days | -4 days | **Already exists** (failure_tracker.rs + retry.rs + ResilienceManager) |
| 2d: TraceRecorder | 3 days | 3 days | 0 | Accurate |
| 2e: Repl decomposition | 1 week | 1 week | 0 | Accurate |
| **NEW P0**: Message pipeline | Not in plan | 2 weeks | +2 weeks | 1,584 LOC monolith extraction |
| **NEW P2**: Agent prologue/delegation | Not in plan | 1 week | +1 week | 1,080 LOC extraction |

### 9.2 True Critical Path

```
P0: Message Pipeline Extraction     [2 weeks]  ← BLOCKS everything
 │
 ├── P1: TraceRecorder consolidation [3 days]   ← independent
 │
 ├── P2: Agent prologue/delegation   [1 week]   ← independent
 │
 └── P3: Permission audit entries    [2 days]   ← independent
      │
      └── P4: Repl struct final      [1 week]   ← REQUIRES P0 + P2
           │
           └── P5: Typed error enum  [3 days]   ← REQUIRES P4

Total critical path: 4 weeks (P0 → P4 → P5)
Parallel work fills gaps: P1, P2, P3 run alongside P0
```

### 9.3 What's Already Done (Discovered During Audit)

These items from the original Phase 2 plan are **already implemented**:

| Item | Status | Location |
|------|--------|----------|
| executor/ subdirectory | DONE | `executor/{mod,parallel,sequential,validation,retry,hooks}.rs` |
| `authorize_tool()` function | DONE | `security/permission_pipeline.rs:103-223` |
| PermissionPipeline with 7 gates | DONE | `security/permission_pipeline.rs` |
| ToolFailureTracker with circuit breaker | DONE | `agent/failure_tracker.rs` |
| ResilienceManager (provider-level) | DONE | `security/resilience.rs` |
| TraceRecorder with mpsc | DONE | `trace_recording/recorder.rs` |
| LoopState with 6 sub-states | DONE | `agent/loop_state.rs` |
| Phase dispatch with PhaseOutcome | DONE | `agent/dispatch.rs` + macro |
| FeedbackArbiter decision authority | DONE | `agent/feedback_arbiter.rs` |

**Net effect**: ~3 weeks of the original plan was already completed. The real remaining work is the **message pipeline extraction** (P0) and **agent prologue/delegation** (P2).

---

## 10. FINAL SYSTEM PROPERTIES

### 10.1 Target Properties Matrix

| Property | Current | After Remediation | Measurement |
|----------|---------|-------------------|-------------|
| **Max file LOC** | 4,678 | < 600 | `wc -l` on any `.rs` file |
| **Max function LOC** | 1,584 | < 150 | `handle_message_with_sink` → 30 LOC orchestrator |
| **Max responsibilities/file** | 67 | < 8 | Audit count |
| **AgentContext fields** | 42 | 42 (unchanged — fix in Phase 3) | Struct field count |
| **Repl sub-structs** | 7 | 5 (reorganized) | Top-level field count |
| **Test isolation** | Full Repl required | Per-stage mocking | Unit test count for pipeline stages |
| **Observability coverage** | Partial (scattered) | Complete (TraceRecorder) | % of operations with trace events |
| **Permission audit trail** | Event-only | Structured PermissionAuditEntry | Queryable trace records |
| **Error classification** | String matching | Typed enum | Compiler-enforced exhaustive matching |

### 10.2 Architectural Invariants (Enforced Post-Remediation)

1. **No function exceeds 200 LOC** — enforced by clippy lint `too_many_lines`
2. **No file exceeds 800 LOC** (excluding tests) — enforced by CI check
3. **Every pipeline stage has a typed Input/Output** — compiler-enforced
4. **Side effects only at stage boundaries** — code review convention
5. **Fire-and-forget operations go through TraceRecorder** — grep audit
6. **Permission decisions produce audit entries** — integration test
7. **Error classification is exhaustive** — Rust `match` compiler check

### 10.3 What We Do NOT Change

Some aspects are intentionally preserved:

1. **Agent loop phase-driven pipeline** — already well-designed, no changes needed
2. **LoopState 6-sub-state decomposition** — already better than Xiyo's flat State
3. **Permission pipeline 7-gate cascade** — more secure than Xiyo, keep as-is
4. **Retry waterfall with adaptive mutation** — Halcon-original innovation, keep and evolve
5. **AgentContext 42 fields** — borrow checker constraints make decomposition risky; defer to Phase 3 with potential ownership redesign
6. **Orchestrator.rs** — 55% tests, production code manageable at 1,340 LOC; defer splitting

---

## APPENDIX A: VERIFICATION CHECKLIST

After each phase, verify:

```bash
# Compilation
cargo check --workspace

# Tests
cargo test --workspace

# Lints
cargo clippy --workspace -- -D warnings

# LOC regression (no file should grow)
find crates/halcon-cli/src/repl -name "*.rs" -exec wc -l {} + | sort -rn | head -20

# Responsibility count (manual audit)
# Each new module should have < 8 responsibilities
```

## APPENDIX B: FILES ALREADY IMPLEMENTING PLAN ITEMS

```
crates/halcon-cli/src/repl/
├── executor/
│   ├── mod.rs           ← plan_execution, execute_one_tool (Step 2a: DONE)
│   ├── parallel.rs      ← execute_parallel_batch (Step 2a: DONE)
│   ├── sequential.rs    ← execute_sequential_tool, TBAC, permissions (Step 2a: DONE)
│   ├── validation.rs    ← validate_tool_args, path resolution (Step 2a: DONE)
│   ├── retry.rs         ← run_with_retry, backoff, classification (Step 2a: DONE)
│   └── hooks.rs         ← pre/post tool hooks (Step 2a: DONE)
├── security/
│   ├── permission_pipeline.rs  ← authorize_tool(), 7-gate cascade (Step 2b: DONE)
│   ├── resilience.rs           ← ResilienceManager, circuit breaker (Step 2c: partial)
│   └── ...26 more files
├── agent/
│   ├── loop_state.rs    ← LoopState with 6 sub-states (Step 2e: partial)
│   ├── dispatch.rs      ← PhaseOutcome enum, dispatch! macro
│   ├── post_batch.rs    ← PostBatchOutcome, failure handling
│   ├── feedback_arbiter.rs  ← FeedbackArbiter decision authority
│   ├── tool_executor.rs     ← Xiyo-aligned batch partitioning
│   └── simplified_loop.rs   ← Minimal alternative execution path
└── trace_recording/
    └── recorder.rs      ← TraceRecorder with mpsc::UnboundedSender (Step 2d: DONE)
```
