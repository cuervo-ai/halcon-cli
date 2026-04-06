# Halcon Remote-Control V2: Frontier-Grade Architecture

> System expansion from CLI remote-control to a fully integrated, multi-agent remote
> execution platform synchronized with Cenzontle.

---

## 1. CURRENT SYSTEM DECONSTRUCTION

### 1.1 Subsystem Map

```
halcon remote-control (v1)
 |
 +-- CLI Layer (halcon-cli/commands/remote_control/)
 |    +-- mod.rs          : clap subcommands (start/status/approve/reject/replan/cancel/attach)
 |    +-- client.rs       : HTTP+WS client (RemoteControlClient)
 |    +-- interactive.rs  : Claude Code-style attach mode (crossterm raw mode + WS)
 |    +-- protocol.rs     : Shared types (RemoteCommand, RemoteControlEvent, ReplanPayload)
 |
 +-- API Layer (halcon-api/server/)
 |    +-- handlers/remote_control.rs  : REST endpoints (replan, status, context-inject)
 |    +-- handlers/chat.rs            : Session CRUD, message submit, permission resolve
 |    +-- ws.rs                        : Broadcast event multiplexer
 |    +-- state.rs                     : AppState (DashMap sessions, broadcast channel)
 |    +-- router.rs                    : Route registration
 |
 +-- Execution Engine (halcon-cli/repl/agent/)
 |    +-- loop_state.rs   : 8-state FSM (AgentPhase), SynthesisControl, LoopState
 |    +-- provider_round.rs: Streaming invocation, headroom guard, budget enforcement
 |    +-- checkpoint.rs   : Fire-and-forget JSON snapshots
 |    +-- planning_policy.rs: Composable planning gate pipeline
 |
 +-- Event System
      +-- halcon-core/types/event.rs    : DomainEvent with W3C trace context (75+ payloads)
      +-- halcon-api/types/ws.rs        : WsServerEvent (40+ variants, 12 channels)
      +-- halcon-storage/event_buffer.rs: PersistentEventBuffer (SQLite, ACK tracking)
      +-- halcon-cli/commands/serve.rs  : Bridge relay to Cenzontle (persistent, DLQ)
```

### 1.2 Architectural Strengths

| Strength | Evidence |
|----------|----------|
| **Universal agent abstraction** | `RuntimeAgent` trait spans LLM/MCP/CLI/HTTP/Remote |
| **Wave-based DAG parallelism** | Topological sort + concurrent wave execution with `SharedContext` |
| **Typed FSM** | `AgentPhase` with deterministic transitions, terminal state stickiness |
| **Cryptographic mailbox** | HMAC-SHA256 signed inter-agent messages, nonce replay protection |
| **Zero-loss bridge** | `PersistentEventBuffer` with SQLite + ACK + DLQ + resume-from-seq |
| **Multi-layer permission** | 9 decisions x 4 scopes x 3 pattern types, persisted in SQLite |
| **Synthesis governance** | Evidence gate, intent phase locking, 6 synthesis origins, rescue classification |
| **Atomic budget tracking** | `AtomicU64` for tokens/cost across concurrent sub-tasks |

### 1.3 Architectural Limitations (V1 Gaps)

| Limitation | Impact |
|------------|--------|
| **No pause/resume** | Cancel is the only intervention; no step-by-step execution |
| **Static DAGs** | Replan replaces entire DAG; no in-flight node insertion/removal |
| **Single-session CLI** | `attach` binds to one session; no multi-session orchestration |
| **Replan = message hack** | `@replan` prefix in chat message, not a first-class operation |
| **No execution versioning** | No vector clock or Lamport timestamp for cross-system ordering |
| **Cenzontle bridge is relay-only** | Events forwarded, but no bidirectional state sync or shared DAG |
| **No speculative execution** | No parallel branch exploration with late-binding winner selection |
| **Permission is fire-once** | No escalation, no delegation, no time-boxed auto-approve |
| **Frontend undefined** | Only CLI; no web dashboard or DAG visualization |

---

## 2. TARGET ARCHITECTURE (2026)

### 2.1 Three-Plane Separation

```
                    +===========================+
                    |    INTELLIGENCE LAYER     |
                    |  (Cenzontle + Local GDEM) |
                    +===========================+
                              |  ^
              goal/plan push  |  |  evaluation/replan
                              v  |
                    +===========================+
                    |      CONTROL PLANE        |
                    |  (halcon-api + RC service) |
                    +===========================+
                              |  ^
            task dispatch      |  |  events + artifacts
                              v  |
                    +===========================+
                    |     EXECUTION PLANE       |
                    | (runtime + agents + tools)|
                    +===========================+
```

### 2.2 Component Diagram

```
+-----------------------------------------------------------------------+
|                        INTELLIGENCE LAYER                              |
|                                                                        |
|  +------------------+    +-------------------+    +------------------+ |
|  | Cenzontle        |    | Local Planner     |    | Strategy         | |
|  | Meta-Orchestrator|<-->| (AdaptivePlanner) |<-->| Learner (UCB1)  | |
|  | (remote)         |    | Tree-of-Thoughts  |    |                  | |
|  +------------------+    +-------------------+    +------------------+ |
|         |                        |                        |            |
|  +------v------------------------v------------------------v----------+ |
|  |              DECISION BUS (Intent + Plan + Evaluation)            | |
|  +-------------------------------------------------------------------+ |
+-----------------------------------------------------------------------+
          |                                            ^
          | PlanEnvelope (versioned, signed)            | EvalReport
          v                                            |
+-----------------------------------------------------------------------+
|                         CONTROL PLANE                                  |
|                                                                        |
|  +------------------+    +-------------------+    +------------------+ |
|  | Session Manager  |    | DAG Controller    |    | Permission       | |
|  | (multi-session)  |    | (mutable DAG)     |    | Arbiter          | |
|  +------------------+    +-------------------+    +------------------+ |
|  +------------------+    +-------------------+    +------------------+ |
|  | RC API           |    | Sync Engine       |    | Event Journal    | |
|  | (REST + WS + gRPC)|   | (vector clock)    |    | (event sourcing) | |
|  +------------------+    +-------------------+    +------------------+ |
|                                                                        |
|  +-------------------------------------------------------------------+ |
|  |           BROADCAST BUS (WsServerEvent, capacity 16384)           | |
|  +-------------------------------------------------------------------+ |
+-----------------------------------------------------------------------+
          |                                            ^
          | AgentRequest (budget, timeout)              | AgentResponse
          v                                            |
+-----------------------------------------------------------------------+
|                        EXECUTION PLANE                                 |
|                                                                        |
|  +------------------+    +-------------------+    +------------------+ |
|  | Agent Registry   |    | Wave Executor     |    | Sandbox          | |
|  | (capability idx) |    | (parallel waves)  |    | (macOS/Linux)    | |
|  +------------------+    +-------------------+    +------------------+ |
|  +------------------+    +-------------------+    +------------------+ |
|  | Tool Registry    |    | Federation Router |    | Budget Enforcer  | |
|  | (20+ built-in)   |    | (mailbox + HMAC)  |    | (atomic counters)| |
|  +------------------+    +-------------------+    +------------------+ |
+-----------------------------------------------------------------------+
```

### 2.3 Data Flow

```
User (CLI/Web/Cenzontle)
  --> Control Plane: submit goal / intervention
    --> Intelligence Layer: plan generation / evaluation
      --> Control Plane: PlanEnvelope with version + signature
        --> Execution Plane: wave dispatch
          --> Agent invocation (tool use, LLM calls)
            --> Execution Plane: AgentResponse + DomainEvent
          --> Control Plane: event journaling + broadcast
        --> Intelligence Layer: step verification + critic scoring
      --> Control Plane: replan if needed (version increment)
    --> User: real-time event stream (WS/SSE)
  --> All systems: persistent audit trail (SQLite hash-chain)
```

### 2.4 Control Flow State Machine (V2)

```
                    +---------+
                    |  IDLE   |
                    +----+----+
                         |  submit goal
                         v
                    +---------+
            +------>| PLANNING|<-----------+
            |       +----+----+            |
            |            |  plan ready     |
            |            v                 |
            |       +---------+            |
            |  +--->|EXECUTING|--+         |
            |  |    +----+----+  |         |
            |  |         |       | pause   |
            |  | resume  |       v         |
            |  |         |  +---------+    |
            |  +---------+  | PAUSED  |    |
            |               +----+----+    |
            |                    |         |
     replan |         step/resume|         |
            |                    v         |
            |       +---------+            |
            +-------+VERIFYING+------------+
                    +----+----+
                         |
              +----------+----------+
              |                     |
              v                     v
         +---------+          +---------+
         |CONVERGED|          |  ERROR  |
         +---------+          +---------+
```

New states vs V1: **PAUSED** (explicit), **VERIFYING** (post-step validation separate from executing).

---

## 3. CENZONTLE SYNCHRONIZATION LAYER

### 3.1 Role Definition

Cenzontle is a **meta-orchestrator and reasoning engine**:

| Role | Description |
|------|-------------|
| **Meta-orchestrator** | Assigns goals to Halcon instances; tracks multi-instance progress |
| **Reasoning engine** | Provides plan evaluation, RAG context, and strategy selection |
| **Supervisor** | Can override local Halcon decisions when confidence is low |
| **NOT** an executor | Cenzontle does not run tools; all execution happens in Halcon |

Halcon is the **execution engine and local authority**:

| Role | Description |
|------|-------------|
| **Executor** | Runs tools in sandboxed environment |
| **Local authority** | Owns permission decisions for local filesystem/processes |
| **Event source** | Emits the canonical event stream for audit and observability |
| **NOT** a planner of last resort | Defers complex planning to Cenzontle when confidence < threshold |

### 3.2 Bidirectional Sync Protocol

```
+-------------------+                          +-------------------+
|     HALCON        |                          |   CENZONTLE       |
|                   |  -- PlanEnvelope ------> |                   |
|                   |  <-- PlanEnvelope ------ |                   |
|                   |                          |                   |
|                   |  -- EventJournal ------> |                   |
|                   |  <-- ContextInjection -- |                   |
|                   |                          |                   |
|                   |  -- EvalRequest -------> |                   |
|                   |  <-- EvalResponse ------ |                   |
|                   |                          |                   |
|                   |  -- PermissionEscalation>|                   |
|                   |  <-- PermissionOverride -|                   |
+-------------------+                          +-------------------+
         |                                              |
         +---------- Shared State (CRDT) ---------------+
         |          (session_id, plan_version,           |
         |           permission_decisions,               |
         |           execution_progress)                 |
         +----------------------------------------------+
```

### 3.3 Shared State Model

**State is partitioned by authority domain:**

```rust
/// State owned by Halcon (local authority).
struct HalconOwnedState {
    execution_progress: HashMap<TaskId, TaskStatus>,  // Halcon is source of truth
    tool_results: HashMap<ToolUseId, ToolOutput>,
    permission_decisions: HashMap<RequestId, PermissionDecision>,  // Local user decisions
    resource_usage: BudgetSnapshot,
}

/// State owned by Cenzontle (intelligence authority).
struct CenzontleOwnedState {
    active_plan: PlanEnvelope,           // Cenzontle plans, Halcon executes
    evaluation_scores: HashMap<StepId, f32>,
    strategy_selection: StrategyId,
    cross_instance_context: Vec<ContextFragment>,
}

/// Shared mutable state (CRDT — last-writer-wins register per key).
struct SharedState {
    session_metadata: LWWRegister<SessionMetadata>,
    plan_version: LWWRegister<u64>,          // Monotonic; higher version wins
    annotations: LWWMap<String, String>,      // Human notes, tags
}
```

**Conflict resolution: Authority-based partitioning + monotonic versioning.**

- Execution state conflicts: Halcon always wins (it has ground truth from the sandbox).
- Plan conflicts: Higher `plan_version` wins. If equal, Cenzontle wins (it has broader context).
- Permission conflicts: See Section 3.7 (failure scenario).

### 3.4 PlanEnvelope (Versioned, Signed)

```rust
struct PlanEnvelope {
    plan_id: Uuid,
    version: u64,                    // Monotonically increasing
    origin: PlanOrigin,              // Cenzontle | HalconLocal | HumanOverride
    dag: Vec<PlanNode>,
    parent_version: Option<u64>,     // Which version this replaces
    signature: String,               // HMAC-SHA256(plan_id + version + dag_hash, shared_secret)
    created_at: DateTime<Utc>,
    vector_clock: VectorClock,       // {halcon: N, cenzontle: M}
}

enum PlanOrigin {
    Cenzontle { confidence: f32, strategy: String },
    HalconLocal { planner: String },
    HumanOverride { user_id: String, reason: String },
}
```

### 3.5 Event Mirroring

Events flow from Halcon to Cenzontle via the existing bridge relay, enhanced with:

```rust
struct MirroredEvent {
    event: DomainEvent,              // Full event with trace context
    halcon_seq: u64,                 // Local monotonic sequence
    vector_clock: VectorClock,       // For ordering across systems
    ack_required: bool,              // Critical events require ACK
}
```

**Mirroring guarantees:**
- At-least-once delivery (PersistentEventBuffer + DLQ)
- Ordering within a session (monotonic halcon_seq)
- Cross-system ordering (vector clock comparison)
- Critical events (permission decisions, plan changes) require ACK within 10s; otherwise retry

### 3.6 Context Injection Pipeline

```
Cenzontle RAG engine
  --> ContextFragment { source, relevance_score, content, expires_at }
    --> Bridge WebSocket ({"t":"ctx","d":{...}})
      --> Halcon bridge handler
        --> AppState.inject_context(session_id, fragment)
          --> Appended to session history as system message
          --> Available in next agent round's context window
```

**Fragments are scored and pruned:** Only fragments with `relevance_score > 0.3` are injected.
Expired fragments are silently dropped.

### 3.7 Cross-System Task Delegation

```
Cenzontle assigns goal to Halcon:
  POST /v1/bridge/delegate
  {
    "task_id": "...",
    "instructions": "Refactor auth module to use JWT",
    "timeout_ms": 300000,
    "budget": { "max_tokens": 50000, "max_cost_usd": 1.0 },
    "plan_envelope": { ... },  // Optional pre-computed plan
    "context_fragments": [ ... ]
  }

Halcon acknowledges:
  {"t":"ack","task_id":"...","accepted":true}

Halcon streams events back:
  {"t":"event","d":{"type":"tool_executed","name":"file_edit",...}}

Halcon completes:
  {"t":"result","task_id":"...","success":true,"output":"...","usage":{...}}
```

### 3.8 Failure Handling Between Systems

| Failure | Detection | Recovery |
|---------|-----------|----------|
| Cenzontle unreachable | Heartbeat timeout (40s) | Halcon continues locally; buffer events for replay |
| Halcon disconnects | Cenzontle heartbeat timeout | Cenzontle marks instance as degraded; reassigns to another instance if available |
| Plan version conflict | Vector clock divergence | Higher version wins; if equal, Cenzontle authority |
| Event replay after reconnect | `X-Resume-From` header with last_acked_seq | Cenzontle replays missed events from its journal |
| Budget exceeded remotely | Cenzontle sends budget-stop command | Halcon honors immediately; emits `BudgetExhausted` event |

### 3.9 Consistency Model

**Eventual consistency** for non-critical state (annotations, metadata).
**Strong consistency** for:
- Permission decisions (first-writer-wins, then broadcast to all clients)
- Plan version (monotonic, reject stale versions)
- Budget enforcement (local atomic counters; Cenzontle sets ceiling)

---

## 4. ADVANCED REMOTE CONTROL MODEL

### 4.1 New Endpoints

```
# Execution control (new)
POST   /api/v1/rc/sessions/:id/pause          # Pause after current tool completes
POST   /api/v1/rc/sessions/:id/resume          # Resume paused execution
POST   /api/v1/rc/sessions/:id/step            # Execute one step, then pause
POST   /api/v1/rc/sessions/:id/override        # Override next agent decision

# DAG mutation (new)
POST   /api/v1/rc/sessions/:id/dag/nodes       # Insert node into running DAG
DELETE /api/v1/rc/sessions/:id/dag/nodes/:nid   # Remove pending node
PATCH  /api/v1/rc/sessions/:id/dag/nodes/:nid   # Update node (args, deps)
GET    /api/v1/rc/sessions/:id/dag              # Full DAG snapshot with status

# Multi-session (new)
POST   /api/v1/rc/orchestrate                   # Submit multi-session goal
GET    /api/v1/rc/orchestrate/:id               # Orchestration status
POST   /api/v1/rc/orchestrate/:id/sessions/:sid/promote  # Promote sub-session result

# Permission evolution (new)
POST   /api/v1/rc/sessions/:id/permissions/:rid/escalate  # Escalate to Cenzontle
POST   /api/v1/rc/sessions/:id/permissions/auto-approve   # Time-boxed auto-approve rule
GET    /api/v1/rc/sessions/:id/permissions/pending         # List all pending
```

### 4.2 New CLI Commands

```bash
# Execution control
halcon remote-control pause   [-s SESSION]
halcon remote-control resume  [-s SESSION]
halcon remote-control step    [-s SESSION]           # Execute one step
halcon remote-control override [-s SESSION] --decision "use file_edit instead"

# DAG manipulation
halcon remote-control dag show    [-s SESSION]        # Print DAG as tree
halcon remote-control dag insert  [-s SESSION] --after STEP_ID --tool bash --args '...'
halcon remote-control dag remove  [-s SESSION] STEP_ID
halcon remote-control dag edit    [-s SESSION] STEP_ID --args '...'

# Multi-session
halcon remote-control orchestrate --plan plan.json    # Launch multi-session execution
halcon remote-control orchestrate status ORC_ID
halcon remote-control orchestrate attach ORC_ID       # Attach to orchestration

# Permission
halcon remote-control permissions list [-s SESSION]
halcon remote-control permissions auto-approve --tool file_read --duration 5m
halcon remote-control permissions escalate PERM_ID
```

### 4.3 State Transitions (V2)

```rust
enum SessionState {
    Idle,
    Planning,
    Executing,
    Paused { reason: PauseReason, resumable: bool },
    AwaitingPermission { request_id: Uuid, deadline: Instant },
    AwaitingHuman { prompt: String },     // Human-in-the-loop decision point
    StepComplete { step_id: Uuid },       // After single-step mode
    Verifying,
    Converged,
    Error { code: String, recoverable: bool },
    Cancelled,
}

enum PauseReason {
    UserRequested,        // halcon rc pause
    StepMode,             // halcon rc step (auto-pause after each step)
    BudgetWarning,        // 80% budget consumed
    CenzontleHold,        // Cenzontle requested hold for evaluation
    PermissionEscalation, // Waiting for escalated permission
}
```

### 4.4 Live DAG Mutation

```rust
struct DagMutation {
    mutation_id: Uuid,
    session_id: Uuid,
    kind: DagMutationKind,
    timestamp: DateTime<Utc>,
    author: MutationAuthor,  // User | Cenzontle | AutoReplan
}

enum DagMutationKind {
    InsertNode {
        node: TaskNode,
        after: Option<Uuid>,   // Insert after this node (add dependency)
    },
    RemoveNode {
        node_id: Uuid,
        cascade: bool,          // Also remove dependents
    },
    UpdateNode {
        node_id: Uuid,
        instruction: Option<String>,
        args: Option<HashMap<String, Value>>,
    },
    AddDependency {
        from: Uuid,
        to: Uuid,
    },
    RemoveDependency {
        from: Uuid,
        to: Uuid,
    },
}
```

**Invariants enforced on every mutation:**
1. No cycles (Kahn's algorithm re-run)
2. No mutation of Completed/Running nodes
3. Dependency additions cannot create cycles
4. Removals cascade or fail if dependents exist

---

## 5. FRONTEND SYSTEM (REAL-TIME CONTROL UI)

### 5.1 UI Modules

```
+-----------------------------------------------------------------------+
|  HEADER BAR                                                            |
|  [Session: abc123] [Model: claude-sonnet] [Status: EXECUTING]          |
|  [Budget: 45% tokens, 23% cost] [Uptime: 12m34s]                      |
+-----------------------------------------------------------------------+
|                    |                          |                         |
|  DAG GRAPH         |  ACTIVITY STREAM         |  CONTEXT PANEL         |
|  (live SVG/Canvas) |  (scrollable log)        |  (collapsible)         |
|                    |                          |                         |
|  [node1] --+       |  12:34:01 tool bash      |  System Prompt         |
|            |       |    exit=0 (234ms)         |  [truncated...]        |
|  [node2] --+--+    |  12:34:02 PERMISSION     |                         |
|            |  |    |    file_write /src/...    |  Plan (3 steps)        |
|  [node3] --+  |    |    [Approve] [Reject]    |    1. Analyze [done]   |
|               |    |  12:34:05 thinking...     |    2. Refactor [now]   |
|  [node4] -----+    |  12:34:12 token stream   |    3. Test [pending]   |
|                    |    "I'll modify the..."   |                         |
+--------------------+--------------------------+-------------------------+
|  PERMISSION QUEUE                              |  INPUT                  |
|  [1] bash rm -rf (Destructive) [A] [R] 45s    |  > Type message...      |
|  [2] file_write /etc (Destructive) [A] [R] 30s|  [Send] [Pause] [Step]  |
+------------------------------------------------+-------------------------+
```

### 5.2 Data Subscriptions

```typescript
// WebSocket subscription on connect
ws.send({ type: "subscribe", channels: [
  "chat",          // Token streaming, session lifecycle
  "permissions",   // Permission requests/resolutions
  "execution",     // Execution failures, replan events
  "sub_agents",    // Sub-agent lifecycle
  "tools",         // Tool execution events
  "tasks",         // DAG task progress
] });
```

### 5.3 State Management Model

```typescript
// Frontend state (React/Solid store)
interface RemoteControlState {
  // Connection
  connected: boolean;
  serverVersion: string;

  // Session
  activeSession: SessionInfo | null;
  sessions: SessionInfo[];

  // DAG
  dagNodes: Map<string, DagNode>;       // Live DAG state
  dagEdges: Edge[];
  dagVersion: number;

  // Execution
  tokenStream: string;                   // Accumulated assistant output
  isThinking: boolean;
  toolExecutions: ToolExecution[];        // Recent tools (ring buffer, 100)

  // Permissions
  pendingPermissions: PendingPermission[];  // Ordered queue
  autoApproveRules: AutoApproveRule[];

  // Sub-agents
  activeSubAgents: Map<string, SubAgentInfo>;
  completedSubAgents: SubAgentResult[];

  // Budget
  tokensUsed: number;
  tokensBudget: number;
  costUsed: number;
  costBudget: number;
}

// Reducer: WsServerEvent -> StateUpdate (pure function)
function reduceEvent(state: RemoteControlState, event: WsServerEvent): RemoteControlState
```

### 5.4 DAG Visualization

```
Rendering: D3.js force-directed graph or dagre-d3 for layered layout.
Node colors:
  - Gray:   Pending
  - Blue:   Running (pulsing animation)
  - Green:  Completed
  - Red:    Failed
  - Yellow: Paused / AwaitingPermission
  - Purple: Speculative (parallel branch)

Edge styles:
  - Solid:  Dependency (must complete before)
  - Dashed: Soft dependency (context injection, non-blocking)

Interactions:
  - Click node: show details panel (instruction, tools, output)
  - Right-click node: context menu (remove, edit, add dependency)
  - Drag to create edge (add dependency)
  - Double-click empty space: insert new node
```

---

## 6. TASK & DAG EVOLUTION

### 6.1 Dynamic DAGs (Mutable at Runtime)

```rust
struct MutableDag {
    nodes: HashMap<Uuid, DagNode>,
    version: u64,                       // Incremented on every mutation
    mutation_log: Vec<DagMutation>,     // Append-only for replay
    lock: RwLock<()>,                   // Readers: executor; Writers: mutations
}

struct DagNode {
    task: TaskNode,
    status: NodeStatus,
    result: Option<AgentResponse>,
    started_at: Option<Instant>,
    completed_at: Option<Instant>,
    retry_count: u32,
}

enum NodeStatus {
    Pending,
    Ready,            // All deps satisfied, awaiting wave slot
    Running,
    Completed,
    Failed { error: String, retryable: bool },
    Skipped,          // Removed or dependency failed (non-cascade)
    Speculative,      // Parallel branch, may be discarded
}
```

**Mutation protocol:**
1. Acquire write lock on DAG
2. Validate mutation (cycle check, status check)
3. Apply mutation
4. Increment version
5. Append to mutation_log
6. Release lock
7. Broadcast `DagMutated { session_id, version, mutation }` event

### 6.2 Hierarchical Tasks

```
Goal: "Refactor authentication"
 |
 +-- Task 1: "Analyze current auth code" (agent: researcher)
 |    +-- SubTask 1.1: "Find all auth files" (tool: glob)
 |    +-- SubTask 1.2: "Read auth middleware" (tool: file_read)
 |
 +-- Task 2: "Implement JWT" (agent: coder)
 |    +-- SubTask 2.1: "Add jwt dependency" (tool: bash)
 |    +-- SubTask 2.2: "Write JWT middleware" (tool: file_write)
 |    +-- SubTask 2.3: "Update routes" (tool: file_edit)
 |
 +-- Task 3: "Test" (agent: tester)
      +-- SubTask 3.1: "Run tests" (tool: bash)
      +-- SubTask 3.2: "Verify coverage" (tool: bash)
```

Each level maintains its own DAG. Parent tasks aggregate child results.

### 6.3 Retry Policies

```rust
struct RetryPolicy {
    max_retries: u32,               // Default: 2
    backoff: BackoffStrategy,       // Exponential(base_ms, max_ms) | Fixed(ms) | None
    retry_on: Vec<RetryCondition>,  // ErrorCode | Timeout | BudgetExceeded
    fallback: Option<FallbackAction>,
}

enum FallbackAction {
    SkipNode,                        // Mark as Skipped, continue DAG
    SubstituteAgent(AgentSelector),  // Try different agent
    EscalateToHuman,                 // Pause and request intervention
    FailDag,                         // Abort entire DAG
}
```

### 6.4 Speculative Execution

```rust
struct SpeculativeBranch {
    branch_id: Uuid,
    nodes: Vec<Uuid>,                // Nodes in this branch
    confidence: f32,                 // From planner
    committed: bool,                 // false = may be discarded
    resource_cap: AgentBudget,       // Limited budget for speculation
}
```

**Execution model:**
1. Planner generates N branches (tree-of-thoughts)
2. Top-K branches execute in parallel with reduced budgets
3. Verifier evaluates completed branches
4. Winner is committed; losers are discarded (nodes → Skipped)
5. If no branch succeeds, escalate to replan or human

### 6.5 Comparison with Modern Systems

| Feature | Claude Code | Codex | Halcon V2 |
|---------|------------|-------|-----------|
| DAG execution | Sequential with sub-agents | DAG with retry | Mutable DAG + speculative |
| Permission model | Per-tool, session-scoped | Auto-approve in sandbox | Multi-scope + escalation + auto-approve |
| Pause/resume | Implicit (prompt boundary) | No | Explicit pause/resume/step |
| Live DAG mutation | No | No | Yes (insert/remove/edit at runtime) |
| Multi-session | No | No | Orchestration across sessions |
| Cross-system sync | No | No | Cenzontle vector-clocked sync |
| Event sourcing | Partial (conversation history) | No | Full (mutation log + event journal) |
| Speculative execution | Sub-agents explore | No | Parallel branch with late-binding |

---

## 7. VALIDATION & SAFETY LAYERS

### 7.1 Multi-Layer Validation Pipeline

```
Layer 0: INPUT VALIDATION
  - Schema validation (JSON Schema / serde)
  - Size limits (message content, plan steps, attachment bytes)
  - Rate limiting (per-session, per-user)

Layer 1: PLAN VALIDATION
  - DAG integrity (cycle detection, dependency resolution)
  - Tool existence (all referenced tools in registry)
  - Budget feasibility (estimated cost vs remaining budget)
  - Permission pre-check (will any tool definitely be denied?)

Layer 2: PRE-EXECUTION VALIDATION
  - Sandbox policy check (command denylist, network isolation)
  - Working directory verification (exists, readable)
  - Headroom guard (enough context window for output)

Layer 3: EXECUTION VALIDATION
  - Per-tool timeout enforcement
  - Budget guard (atomic check before each invocation)
  - Output sanitization (PII detection, secret scanning)
  - Exit code validation (non-zero → failure pathway)

Layer 4: POST-CONDITION VALIDATION
  - Step verifier (did the output match the goal criterion?)
  - Critic scoring (alignment with original intent)
  - Regression detection (did this step undo previous progress?)
  - Artifact integrity (file checksums, diff validation)
```

### 7.2 Permission Escalation Model

```
Tool invocation
  --> Check local rules (Session → Directory → Repository → Global)
    --> If Allowed: execute
    --> If Denied: deny
    --> If No Rule:
      --> Check auto-approve rules (time-boxed, tool-scoped)
        --> If match: execute + log
        --> If no match:
          --> Prompt user (CLI/Web)
            --> If user responds: apply decision
            --> If timeout:
              --> Escalate to Cenzontle (if connected)
                --> Cenzontle evaluates risk + context
                --> Returns: Approve | Deny | RequestMoreContext
              --> If Cenzontle unavailable: fail-closed (deny)
```

### 7.3 Guardrails

```rust
struct Guardrails {
    // Hard limits (cannot be overridden)
    max_cost_per_session_usd: f64,        // Default: 10.0
    max_tokens_per_session: u64,          // Default: 500_000
    max_concurrent_tools: usize,          // Default: 8
    max_file_write_size_bytes: u64,       // Default: 10MB
    command_denylist: Vec<Regex>,          // rm -rf /, sudo, etc.

    // Soft limits (warnings, user can override)
    warn_cost_threshold_usd: f64,         // Default: 5.0
    warn_token_threshold: u64,            // Default: 250_000
    warn_on_destructive_tool: bool,       // Default: true
}
```

---

## 8. FAILURE MODES & RESILIENCE

### 8.1 Failure Catalog

| Failure | Detection | Impact | Recovery |
|---------|-----------|--------|----------|
| **API unreachable** | HTTP timeout (10s) | CLI commands fail | Retry with exponential backoff; offline queue |
| **WebSocket desync** | Sequence gap in events | Missed events in attach mode | Reconnect + fetch state snapshot via REST |
| **Double approval** | Second POST to /permissions/:rid | Idempotent (first wins) | Return previous decision; no re-execution |
| **DAG corruption** | Cycle detection on mutation | Mutation rejected | Return error; DAG unchanged; log for debug |
| **Partial execution** | Task fails mid-wave | Dependent tasks blocked | Retry policy; or pause for human intervention |
| **Executor panic** | Event channel closes without terminal event | Session stuck in Executing | Synthetic `ExecutionFailed` (already implemented in B3) |
| **Cenzontle disconnect** | Heartbeat timeout | No remote evaluation | Continue locally; buffer events; reconnect |
| **Budget exceeded mid-tool** | Atomic counter check | Tool may complete over budget | Allow current tool to finish; deny next |
| **Permission timeout** | Deadline timer | Tool auto-denied | `PermissionExpired` event; fail-closed |
| **Concurrent session mutation** | DashMap prevents data races | State inconsistency | All session mutations are atomic via DashMap |

### 8.2 Event Journal (Event Sourcing)

```rust
struct EventJournal {
    db: AsyncDatabase,
    session_id: Uuid,
}

impl EventJournal {
    /// Append event atomically.
    async fn append(&self, event: DomainEvent) -> Result<u64>  // Returns sequence number

    /// Replay all events for a session (for state reconstruction).
    async fn replay(&self, session_id: Uuid) -> Result<Vec<DomainEvent>>

    /// Replay from a specific sequence number (for reconnection).
    async fn replay_from(&self, session_id: Uuid, from_seq: u64) -> Result<Vec<DomainEvent>>

    /// Snapshot current state (checkpoint for fast replay).
    async fn snapshot(&self, state: &SessionState) -> Result<u64>  // Returns snapshot seq
}
```

**State reconstruction:** On WebSocket reconnect or crash recovery:
1. Load latest snapshot (if any)
2. Replay events from snapshot sequence onward
3. Apply each event to rebuild state
4. Resume from reconstructed state

### 8.3 Idempotency Guarantees

| Operation | Idempotency Key | Behavior on Duplicate |
|-----------|----------------|----------------------|
| Create session | Session UUID | Return existing |
| Submit message | `(session_id, content_hash, timestamp_bucket)` | Reject with 409 Conflict |
| Resolve permission | `(session_id, request_id)` | Return previous decision |
| Submit replan | `(session_id, plan_version)` | Reject stale version |
| Cancel session | `session_id` | No-op if already cancelled |
| DAG mutation | `mutation_id` | Dedup by mutation_id |

---

## 9. OBSERVABILITY & DEBUGGING

### 9.1 Structured Logging

```rust
// Every log line includes:
tracing::info!(
    session_id = %session_id,
    correlation_id = %correlation_id,    // Links CLI → API → executor
    plan_version = plan_version,
    dag_version = dag_version,
    agent_phase = ?phase,
    "event description"
);
```

**Log schema:**
```json
{
  "timestamp": "2026-03-30T12:34:56.789Z",
  "level": "INFO",
  "session_id": "abc123",
  "correlation_id": "def456",
  "plan_version": 3,
  "dag_version": 7,
  "agent_phase": "Executing",
  "tool": "bash",
  "duration_ms": 234,
  "message": "tool execution completed"
}
```

### 9.2 Distributed Tracing

```
Trace: session_abc123
  |
  +-- Span: plan_generation (12ms)
  |    +-- Span: cenzontle_eval_request (45ms)
  |
  +-- Span: wave_1 (2340ms)
  |    +-- Span: task_analyze (1200ms)
  |    |    +-- Span: tool_glob (34ms)
  |    |    +-- Span: tool_file_read (56ms)
  |    +-- Span: task_search (890ms)
  |         +-- Span: tool_grep (890ms)
  |
  +-- Span: wave_2 (5670ms)
       +-- Span: task_refactor (5670ms)
            +-- Span: permission_wait (3000ms)
            +-- Span: tool_file_edit (234ms)
            +-- Span: tool_bash_test (1200ms)
```

**Implementation:** `DomainEvent.trace_id` and `DomainEvent.span_id` (W3C Trace-Context) are
auto-injected via task-local `EXECUTION_CTX`. Compatible with Jaeger/Zipkin/Datadog.

### 9.3 Metrics

```
# Counters
remote_control_commands_total{command="approve|reject|replan|cancel|pause|resume|step"}
permission_decisions_total{decision="approve|deny|escalate|timeout"}
dag_mutations_total{kind="insert|remove|update"}
cenzontle_sync_events_total{direction="send|receive",status="ok|error"}

# Histograms
permission_latency_seconds{tool="bash|file_write|..."}
tool_execution_duration_seconds{tool="bash|file_write|..."}
plan_generation_duration_seconds{origin="cenzontle|local|human"}
websocket_event_delivery_seconds

# Gauges
active_sessions_count
pending_permissions_count
dag_nodes_by_status{status="pending|running|completed|failed"}
budget_utilization_ratio{resource="tokens|cost|time"}
cenzontle_connection_status  # 0=disconnected, 1=connected, 2=degraded
```

### 9.4 Debug Hooks

```rust
// Compile-time debug hooks (cfg(debug_assertions) only)
trait DebugHook: Send + Sync {
    /// Called before each tool execution. Return false to skip.
    fn pre_tool(&self, tool: &str, args: &Value) -> bool { true }

    /// Called after each agent round. Dump state for inspection.
    fn post_round(&self, state: &LoopState) {}

    /// Called on DAG mutation. Validate invariants.
    fn on_dag_mutated(&self, dag: &MutableDag, mutation: &DagMutation) {}
}
```

---

## 10. SECURITY MODEL

### 10.1 Token System Evolution

```
V1: Single static token (HALCON_API_TOKEN)
  - Generated on `halcon serve` startup
  - All clients use the same token
  - No expiration

V2: Scoped, expiring tokens with RBAC
  - Token types:
    - Admin:    full access (session CRUD, config, user management)
    - Operator: execution control (approve, reject, pause, resume, replan)
    - Observer: read-only (status, attach, events)
  - Token expiration: configurable (default 24h)
  - Token rotation: `POST /api/v1/auth/rotate`
  - Cenzontle tokens: JWT with claims {instance_id, scopes[], exp}
```

### 10.2 Permission Boundaries

```
+-------------------+     +-------------------+     +-------------------+
| LOCAL USER        |     | CENZONTLE         |     | REMOTE USER       |
| (CLI/TUI)         |     | (meta-orchestrator)|    | (Web UI)          |
+-------------------+     +-------------------+     +-------------------+
| Can:               |     | Can:               |    | Can:               |
|  - approve/reject  |     |  - suggest plans   |    |  - approve/reject  |
|  - pause/resume    |     |  - inject context  |    |  - view events     |
|  - replan          |     |  - evaluate steps  |    |  - submit messages |
|  - cancel          |     |  - delegate tasks  |    |  - pause/resume    |
|  - mutate DAG      |     | Cannot:            |    | Cannot:            |
| Cannot:            |     |  - approve perms   |    |  - mutate DAG      |
|  - (none; full     |     |  - execute tools   |    |    (unless Admin)  |
|    local authority) |     |  - access local FS |    |  - cancel          |
+-------------------+     +-------------------+     |    (unless Operator)|
                                                     +-------------------+
```

### 10.3 Multi-Tenant Support

```rust
struct TenantConfig {
    tenant_id: String,
    allowed_providers: Vec<String>,
    budget_ceiling: AgentBudget,
    allowed_tools: Vec<String>,         // Whitelist (empty = all)
    denied_tools: Vec<String>,          // Blacklist (takes precedence)
    data_isolation: DataIsolation,      // Separate | Shared
    audit_retention_days: u32,
}

enum DataIsolation {
    Separate,   // Separate SQLite databases per tenant
    Shared,     // Single DB, tenant_id column on all tables
}
```

### 10.4 Attack Vectors

| Vector | Mitigation |
|--------|------------|
| **Token theft** | Short-lived tokens + rotation; HTTPS-only in production |
| **WebSocket hijacking** | Bearer token on upgrade; CORS restricted to localhost |
| **Plan injection** | PlanEnvelope signature verification (HMAC-SHA256) |
| **Permission bypass** | Server-side enforcement; CLI is a client, not an authority |
| **Tool command injection** | Sandbox denylist + macOS sandbox-exec / Linux unshare |
| **Cross-session data leak** | Session isolation in DashMap; no cross-session queries without auth |
| **Cenzontle impersonation** | Mutual TLS or JWT with instance_id claim |
| **Replay attack (mailbox)** | Monotonic nonce per (agent, team) pair |
| **Budget drain** | Atomic enforcement; Cenzontle can set ceiling remotely |
| **Event flood (DoS)** | Broadcast channel capacity (16384); lagged clients dropped |

---

## 11. COMPARISON WITH FRONTIER SYSTEMS

### 11.1 Claude Code

| Dimension | Claude Code | Halcon V2 |
|-----------|------------|-----------|
| Agent loop | Single-threaded, sequential rounds | FSM with 10 states, parallel waves |
| Sub-agents | Spawned per-task, fire-and-forget | DAG-orchestrated with dependency tracking |
| Permission UX | Inline CLI prompt | Multi-channel (CLI/Web/Cenzontle), escalation |
| Planning | Implicit (model decides) | Explicit AdaptivePlanner with tree-of-thoughts |
| Observability | Console output | Structured logs, W3C tracing, metrics, event journal |
| Remote control | None (local only) | Full REST+WS API, multi-client, multi-session |
| State persistence | Conversation history | Event-sourced with snapshots and replay |

**Where Halcon V2 exceeds:** Mutable DAGs, pause/resume/step, Cenzontle integration, multi-session orchestration, speculative execution.

### 11.2 Codex

| Dimension | Codex | Halcon V2 |
|-----------|-------|-----------|
| Execution model | Sandboxed container | macOS/Linux sandbox with tool-level isolation |
| Reliability | Retry on failure | Configurable retry policies + fallback actions |
| Human control | Submit and wait | Real-time intervention at any point |
| Multi-agent | No | DAG-based with federation protocol |
| Cost control | Per-task pricing | Atomic budget with per-session ceilings |

**Where Halcon V2 exceeds:** Live intervention, DAG mutation, cross-system sync, transparency (full event stream).

### 11.3 Autonomous Agent Frameworks (AutoGPT, CrewAI, LangGraph)

| Dimension | Typical Framework | Halcon V2 |
|-----------|------------------|-----------|
| Orchestration | Python process, in-memory | Rust runtime, persistent, crash-recoverable |
| Type safety | Dynamic (Python dicts) | Static (Rust enums, serde) |
| Security | Minimal sandboxing | macOS sandbox-exec, Linux unshare, HMAC mailbox |
| Performance | Single-threaded Python | Tokio async, zero-copy broadcast, atomic budgets |
| Compliance | None | SOC 2 audit trail with hash-chain |
| Production readiness | Prototype-grade | Production-grade (WAL, DLQ, circuit breakers) |

---

## 12. IMPLEMENTATION ROADMAP

### Phase 1: Core Refactor (4 weeks)

| Task | Priority | Effort |
|------|----------|--------|
| Replace static DAG with `MutableDag` in runtime executor | P0 | L |
| Add `Paused` state to `SessionState` and `AgentPhase` | P0 | M |
| Implement pause/resume/step endpoints + CLI commands | P0 | M |
| Add event journal (append-only table + replay) | P0 | M |
| Implement DAG mutation endpoints + cycle validation | P1 | L |
| Add vector clock to `DomainEvent` | P1 | S |
| Expand broadcast channel capacity to 16384 | P2 | S |

### Phase 2: Cenzontle Deep Integration (4 weeks)

| Task | Priority | Effort |
|------|----------|--------|
| Define `PlanEnvelope` with versioning + HMAC signature | P0 | M |
| Implement bidirectional plan sync (push/pull) | P0 | L |
| Replace `@replan` message hack with first-class replan API | P0 | M |
| Add context injection scoring + pruning pipeline | P1 | M |
| Implement permission escalation to Cenzontle | P1 | M |
| Add evaluation request/response protocol | P1 | L |
| Implement shared state CRDT (LWW registers) | P2 | L |

### Phase 3: Frontend + Advanced Control (6 weeks)

| Task | Priority | Effort |
|------|----------|--------|
| Build React/Solid web frontend shell | P0 | L |
| Implement DAG visualization (dagre-d3) | P0 | L |
| Build permission queue UI with inline approve/reject | P0 | M |
| Build token stream display with thinking indicator | P1 | M |
| Implement multi-session orchestration CLI + API | P1 | L |
| Add auto-approve rules (time-boxed) | P1 | M |
| Build sub-agent activity panel | P2 | M |
| Build budget dashboard widget | P2 | S |

### Phase 4: Autonomy + Optimization (4 weeks)

| Task | Priority | Effort |
|------|----------|--------|
| Implement speculative execution (parallel branches) | P1 | L |
| Add retry policies with configurable fallback | P1 | M |
| Implement hierarchical task decomposition | P1 | L |
| Add scoped RBAC tokens (Admin/Operator/Observer) | P1 | M |
| Add multi-tenant support | P2 | L |
| Implement debug hooks (pre-tool, post-round) | P2 | M |
| Performance optimization (benchmark + profile) | P2 | M |

**Legend:** P0 = must-have, P1 = should-have, P2 = nice-to-have.
S = small (1-2 days), M = medium (3-5 days), L = large (5-10 days).

---

## APPENDIX A: FAILURE SCENARIO SIMULATION

### Scenario: Plan Disagreement + Permission Conflict

**Setup:**
- Halcon is executing a plan (version 5) locally.
- Cenzontle evaluates step 3 output and generates a new plan (version 6).
- Simultaneously, the user submits a replan from CLI (would be version 6 too).
- During this conflict, a permission request is approved in CLI but Cenzontle sends a deny override.

**Timeline:**

```
T=0   Halcon executing plan v5, step 3 running
T=1   Cenzontle evaluates step 3 output, scores 0.2 (low)
T=2   Cenzontle generates new plan, sends PlanEnvelope(v=6, origin=Cenzontle)
T=3   User submits CLI replan, generates PlanEnvelope(v=6, origin=HumanOverride)
T=4   Permission request for bash tool emitted (request_id=X)
T=5   User approves permission X from CLI
T=6   Cenzontle sends PermissionOverride(request_id=X, decision=Deny)
```

**Resolution:**

**Plan conflict (T=2 vs T=3):**
1. Both arrive at control plane with version 6.
2. Control plane checks vector clocks:
   - Cenzontle envelope: `{halcon: 5, cenzontle: 3}`
   - Human envelope: `{halcon: 5, cenzontle: 2}` (user hasn't seen Cenzontle's v=3 evaluation)
3. Cenzontle's vector clock dominates (higher cenzontle component).
4. **BUT** `PlanOrigin::HumanOverride` has priority over `PlanOrigin::Cenzontle`.
5. **Resolution:** Human override wins. The rationale: a human who explicitly submits a plan
   has final authority over an automated system. This is the core human-in-the-loop guarantee.
6. Cenzontle receives `PlanEnvelope(v=7, origin=HumanOverride)` as acknowledgment.
7. Cenzontle can protest by sending an evaluation with `confidence: 0.1`, but cannot override.

**Permission conflict (T=5 vs T=6):**
1. User's approval arrives at `POST /permissions/X` at T=5.
2. `perm_decision_rx` channel receives `(X, true)`.
3. Executor approves and begins tool execution.
4. Cenzontle's deny override arrives at T=6.
5. Control plane checks: permission X already resolved.
6. **Resolution:** First-writer-wins. The tool has already started executing.
7. Cenzontle receives `PermissionResolved(X, decision=approve, source=local_user)`.
8. Cenzontle logs the disagreement. If this pattern repeats (3+ overrides in a session),
   Cenzontle escalates to an administrator alert.

**Key invariant:** Local user approval is irrevocable once the tool starts executing.
Cenzontle can only influence future permissions, not retroactively revoke.

**Recovery path if the tool causes damage:**
1. Cenzontle detects regression in post-step verification (score drops).
2. Cenzontle sends a replan that includes a rollback step (e.g., `git checkout -- <file>`).
3. If user hasn't overridden, Cenzontle's replan is accepted automatically.
4. Rollback step executes, restoring state.
5. New plan continues from a safe checkpoint.

---

## APPENDIX B: PROTOCOL VERSION COMPATIBILITY

```
Protocol versions:
  1.0.0 — Current (V1 remote-control)
  2.0.0 — V2 (this document)

Backward compatibility:
  - V2 server accepts V1 clients (no pause/resume/step; replan via @replan hack)
  - V2 client rejects V1 server (feature detection via Connected.server_version)
  - Unknown events: #[serde(other)] Unknown variant (forward-compatible)
  - New WsServerEvent variants: V1 clients silently ignore (serde skip)
```
