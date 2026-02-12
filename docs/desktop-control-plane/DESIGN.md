# Cuervo Desktop Control Plane — Design Document

## Technology Stack

| Component | Choice | Rationale |
|-----------|--------|-----------|
| UI framework | egui 0.29 (eframe) | Pure Rust, immediate-mode, cross-platform, <30MB base |
| HTTP server | axum 0.7 | Tower ecosystem, WebSocket native, production-grade |
| HTTP client | reqwest 0.12 | Already in workspace, TLS, streaming |
| WebSocket | tokio-tungstenite 0.24 | Async native, protocol-compliant |
| Async runtime | tokio 1.x | Already in workspace, multi-threaded |
| Serialization | serde + serde_json | Already in workspace, zero-copy where possible |
| Auth | Bearer token | Simple, secure for local/remote use |
| Local cache | In-memory (bounded) | Phase 1; sqlite in Phase 5 |
| Logging | tracing | Already in workspace, structured |

## API Contract (v1)

### Agent Types

```rust
AgentInfo {
    id: Uuid,
    name: String,
    kind: AgentKind,         // Llm, Mcp, CliProcess, HttpEndpoint, Plugin
    capabilities: Vec<String>,
    health: HealthStatus,    // Healthy, Degraded, Unavailable, Unknown
    registered_at: DateTime<Utc>,
    last_invoked: Option<DateTime<Utc>>,
    invocation_count: u64,
    max_concurrency: usize,
    metadata: HashMap<String, Value>,
}

SpawnAgentRequest {
    name: String,
    kind: AgentKind,
    capabilities: Vec<String>,
    config: HashMap<String, Value>,
}

InvokeAgentRequest {
    instruction: String,
    context: HashMap<String, Value>,
    budget: Option<BudgetSpec>,
    timeout_ms: Option<u64>,
}
```

### Task Types

```rust
TaskDagSpec {
    nodes: Vec<TaskNodeSpec>,
}

TaskNodeSpec {
    task_id: Uuid,
    instruction: String,
    agent_selector: AgentSelectorSpec,
    depends_on: Vec<Uuid>,
    budget: Option<BudgetSpec>,
}

AgentSelectorSpec {
    by_id: Option<Uuid>,
    by_capability: Option<Vec<String>>,
    by_kind: Option<AgentKind>,
    by_name: Option<String>,
}

TaskExecution {
    id: Uuid,
    dag: TaskDagSpec,
    status: TaskStatus,
    wave_count: usize,
    results: Vec<TaskNodeResult>,
    submitted_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
    total_usage: UsageInfo,
}
```

### WebSocket Events

```rust
WsClientMessage {
    Subscribe { channels: Vec<String> },
    Unsubscribe { channels: Vec<String> },
    Ping,
}

WsServerEvent {
    AgentRegistered(AgentInfo),
    AgentDeregistered { id: Uuid },
    AgentHealthChanged { id: Uuid, health: HealthStatus },
    TaskSubmitted { id: Uuid },
    TaskWaveStarted { task_id: Uuid, wave: usize },
    TaskNodeCompleted { task_id: Uuid, node_id: Uuid, success: bool },
    TaskCompleted { id: Uuid, success: bool },
    ToolExecuted { name: String, duration_ms: u64, success: bool },
    LogEntry(LogEntry),
    MetricUpdate(MetricPoint),
    ProtocolMessage(ProtocolMessageInfo),
    Error { code: String, message: String },
    Pong,
}
```

## UI Layout

```
┌─────────────────────────────────────────────────────────────┐
│  ◉ Cuervo Control Plane          [Connected ●]   [⚙ Settings]│
├────────────┬────────────────────────────────────────────────┤
│            │                                                │
│ ▸ Dashboard│   ┌─ Dashboard ──────────────────────────┐     │
│   Agents   │   │                                      │     │
│   Tasks    │   │  Agents: 5 active   Tasks: 12 total  │     │
│   Tools    │   │  Tools: 23 loaded   Events: 1.2K/s   │     │
│   ─────    │   │                                      │     │
│   Logs     │   │  ┌─ Active Tasks ──────────────────┐ │     │
│   Metrics  │   │  │ ● Code review     [████░░] 67%  │ │     │
│   Protocols│   │  │ ● Test generation [██████] done  │ │     │
│   ─────    │   │  │ ○ Deploy check    [waiting]      │ │     │
│   Files    │   │  └──────────────────────────────────┘ │     │
│   Settings │   │                                      │     │
│            │   │  ┌─ Recent Events ──────────────────┐ │     │
│            │   │  │ 12:01 AgentInvoked code-review   │ │     │
│            │   │  │ 12:00 ToolExecuted file_read     │ │     │
│            │   │  │ 11:59 TaskCompleted dag-7a3f     │ │     │
│            │   │  └──────────────────────────────────┘ │     │
│            │   └──────────────────────────────────────┘     │
├────────────┴────────────────────────────────────────────────┤
│  Status: 5 agents │ 23 tools │ uptime 2h 14m │ mem 42MB    │
└─────────────────────────────────────────────────────────────┘
```

## View Specifications

### Dashboard
- Agent count, task count, tool count, event rate
- Active task progress bars
- Recent events (last 50, auto-scroll)
- System health indicator

### Agents View
- Table: name, kind, health, capabilities, invocations, last active
- Detail panel: full info, invoke dialog, health timeline
- Actions: spawn, stop, restart, inspect

### Tasks View
- DAG visualization (topological layout)
- Wave-by-wave execution timeline
- Node status with colors (pending=gray, running=blue, done=green, failed=red)
- Submit new DAG dialog

### Tools View
- Table: name, permission, enabled, execution count
- Toggle enable/disable
- Execution history with timing

### Logs View
- Scrollable log window (ring buffer, last 10K entries)
- Level filter (trace, debug, info, warn, error)
- Text search
- Source filter (by crate/module)

### Metrics View
- Token usage (input/output over time)
- Latency per agent
- Tool execution duration histogram
- Memory usage

### Protocols View
- Message timeline (MCP/A2A/Federation)
- Raw JSON payload inspector
- Filter by protocol type
- Message replay

### Files View
- Project tree explorer
- File content viewer
- Diff viewer for edits

### Settings View
- Connection URL + token
- Theme (dark/light)
- Polling interval
- Buffer sizes
- Export config

## Thread Model

```
Thread 1: egui main loop
  - Reads AppState
  - Sends UiAction via channel
  - Drains BackendResponse channel
  - Calls ctx.request_repaint() on data change

Thread 2+: tokio runtime (multi-threaded)
  - Task A: WebSocket event listener
  - Task B: Periodic metric poller (5s interval)
  - Task C: Request executor (processes UiAction queue)
  - Task D: Health checker (30s interval)
```

## Error Handling

- Network errors: retry with exponential backoff, show "disconnected" in status bar
- Server errors: display in UI notification area, log to tracing
- Malformed data: skip entry, log warning, never crash
- Auth failure: prompt for new token

## Phased Delivery

| Phase | Deliverable |
|-------|-------------|
| 1 | Architecture + crate scaffold + API types + basic window |
| 2 | Agent CRUD + task submission + tool listing + connection |
| 3 | Log streaming + event viewer + metrics display |
| 4 | DAG visualization + protocol inspector + file viewer |
| 5 | Auth hardening + rate limiting + local cache |
| 6 | Performance tuning + benchmarks |
| 7 | Cross-platform builds + packaging |
