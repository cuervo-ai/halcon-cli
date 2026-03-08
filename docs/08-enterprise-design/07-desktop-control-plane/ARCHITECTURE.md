# Cuervo Desktop Control Plane — Architecture

## System Overview

```
┌─────────────────────────────────────────────────────────┐
│                   cuervo-desktop                        │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐   │
│  │Dashboard │ │ Agents   │ │  Tasks   │ │  Tools   │   │
│  ├──────────┤ ├──────────┤ ├──────────┤ ├──────────┤   │
│  │  Logs    │ │ Metrics  │ │Protocols │ │  Files   │   │
│  └────┬─────┘ └────┬─────┘ └────┬─────┘ └────┬─────┘   │
│       └──────┬─────┴──────┬─────┴──────┬──────┘         │
│         ┌────▼────────────▼────────────▼────┐           │
│         │        Application State          │           │
│         │     (egui reactive + channels)    │           │
│         └────────────────┬──────────────────┘           │
│                          │                              │
│  Layer 1: UI             │  egui immediate-mode         │
├──────────────────────────┼──────────────────────────────┤
│                          │                              │
│  ┌───────────────────────▼──────────────────────┐       │
│  │            cuervo-client                     │       │
│  │  HTTP (reqwest) + WebSocket (tungstenite)    │       │
│  │  typed requests, streaming, auth             │       │
│  └───────────────────────┬──────────────────────┘       │
│                          │                              │
│  Layer 2: Client SDK     │  async, tokio                │
╞══════════════════════════╪══════════════════════════════╡
│           NETWORK        │  HTTP + WebSocket            │
╞══════════════════════════╪══════════════════════════════╡
│                          │                              │
│  ┌───────────────────────▼──────────────────────┐       │
│  │            cuervo-api (server)               │       │
│  │  axum router + WebSocket hub                 │       │
│  │  auth middleware + rate limiting              │       │
│  └───────────────────────┬──────────────────────┘       │
│                          │                              │
│  Layer 3: API Server     │  axum, tower                 │
├──────────────────────────┼──────────────────────────────┤
│                          │                              │
│  ┌───────────────────────▼──────────────────────┐       │
│  │          cuervo-runtime (existing)           │       │
│  │  CuervoRuntime facade                        │       │
│  │  AgentRegistry + MessageRouter + Executor    │       │
│  └──────────────────────────────────────────────┘       │
│                                                         │
│  Layer 4: Runtime        │  existing data plane         │
└─────────────────────────────────────────────────────────┘
```

## Crate Dependency Graph

```
cuervo-desktop ──► cuervo-client ──► cuervo-api (types only)
                                          │
                   cuervo-api (server) ───►├──► cuervo-runtime
                                          ├──► cuervo-core
                                          └──► cuervo-tools
```

## Crate Responsibilities

### cuervo-api
- **Shared types** (always compiled): Request/response DTOs, event types, error codes
- **Server** (feature-gated `server`): axum HTTP + WebSocket server wrapping CuervoRuntime
- Types are serialization-contract-only — no runtime dependency when used as types

### cuervo-client
- Async typed Rust SDK connecting to cuervo-api server
- HTTP client (reqwest) for request-response patterns
- WebSocket client (tokio-tungstenite) for streaming (logs, events, progress)
- Connection management, retries, auth token injection
- Zero knowledge of runtime internals

### cuervo-desktop
- egui (eframe) native desktop application
- Immediate-mode UI running on main thread
- Background tokio runtime for client communication
- mpsc channels bridge UI ↔ async worlds
- Local state cache for offline-resilient views

## Communication Protocol

### HTTP Endpoints (Request-Response)

| Method | Path | Purpose |
|--------|------|---------|
| GET | /api/v1/agents | List agents |
| POST | /api/v1/agents | Spawn agent |
| GET | /api/v1/agents/:id | Get agent |
| DELETE | /api/v1/agents/:id | Stop agent |
| POST | /api/v1/agents/:id/invoke | Invoke agent |
| GET | /api/v1/agents/:id/health | Agent health |
| GET | /api/v1/tasks | List tasks |
| POST | /api/v1/tasks | Submit task DAG |
| GET | /api/v1/tasks/:id | Get task status |
| DELETE | /api/v1/tasks/:id | Cancel task |
| GET | /api/v1/tools | List tools |
| POST | /api/v1/tools/:name/toggle | Enable/disable tool |
| GET | /api/v1/tools/:name/history | Tool execution history |
| GET | /api/v1/metrics | Current metrics snapshot |
| GET | /api/v1/system/status | System status |
| POST | /api/v1/system/shutdown | Graceful shutdown |

### WebSocket Streams (Server Push)

| Path | Purpose |
|------|---------|
| /ws/events | All runtime events (DomainEvent stream) |
| /ws/logs | Structured log stream (filtered) |
| /ws/tasks/:id/progress | Task execution progress |
| /ws/agents/:id/output | Agent output stream |

### Authentication

- Bearer token in `Authorization` header
- Token generated by server on startup, printed to stderr
- WebSocket auth via `?token=` query parameter
- All connections validated before processing

## Desktop App Architecture

```
┌─────────────────────────────────────────────────┐
│                  Main Thread                    │
│  ┌─────────────────────────────────────────┐    │
│  │ eframe event loop                       │    │
│  │  ├── CuervoApp::update()               │    │
│  │  │   ├── drain command_rx (responses)   │    │
│  │  │   ├── update AppState               │    │
│  │  │   └── render active view            │    │
│  │  └── request_repaint() on new data     │    │
│  └────────────┬────────────────────────────┘    │
│               │ command_tx / command_rx          │
│  ┌────────────▼────────────────────────────┐    │
│  │ Background Tokio Runtime                │    │
│  │  ├── ConnectionWorker                   │    │
│  │  │   ├── maintain connection            │    │
│  │  │   └── reconnect on failure           │    │
│  │  ├── EventStreamWorker                  │    │
│  │  │   └── WebSocket → command_tx         │    │
│  │  ├── PollingWorker                      │    │
│  │  │   └── periodic metric/status fetch   │    │
│  │  └── RequestWorker                      │    │
│  │      └── UI action → HTTP → command_tx  │    │
│  └─────────────────────────────────────────┘    │
└─────────────────────────────────────────────────┘
```

## Security Model

1. **Network isolation**: Server binds to 127.0.0.1 by default
2. **Token auth**: Random 256-bit token generated per session
3. **No shell injection**: All commands go through typed API, never raw strings
4. **Rate limiting**: Tower middleware, configurable per-endpoint
5. **Sandboxed tools**: Permission levels enforced server-side
6. **Crash containment**: Agent failures isolated, never crash the server
7. **Audit log**: Every API call logged with timestamp, caller, action

## Performance Design

- **Startup**: egui window visible in <200ms; background connection async
- **Memory**: egui base ~30MB; client state grows with retained data (capped buffers)
- **Streaming**: WebSocket events processed in background; UI polls via channels
- **No blocking**: All I/O on tokio runtime; UI thread only does rendering
- **Event throughput**: Channel-based pipeline, 10K+ events/sec capacity
