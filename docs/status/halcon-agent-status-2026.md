# Halcon Agent System — Status Report 2026-03-08

**Branch**: `feature/sota-intent-architecture`
**Generated**: 2026-03-08 (read-only diagnosis — no files modified)
**Auditor**: automated code analysis grounded in source citations

---

## 1. RESUMEN EJECUTIVO

Halcon is a Rust-based autonomous coding agent CLI that competes in the same space as Claude Code and Cursor. It runs a multi-round agent loop — taking a user query, selecting tools, executing them (bash, file read/write, grep, git, web fetch, and ~58 others), and iterating until a convergence controller signals completion or a round limit is reached. The system is backed by 7 model providers (Anthropic, OpenAI, Gemini, DeepSeek, Ollama, OpenAI-compatible, ClaudeCode), a 4-tier context pipeline with semantic retrieval, HMAC-signed audit export in JSONL/CSV/PDF, lifecycle hooks, a declarative sub-agent registry, an MCP client+server, a TUI, a JSON-RPC mode for VS Code, and 8 feature-flagged advanced subsystems. The workspace spans 768 Rust source files and 317,632 total lines, with 8,206 annotated tests.

What doesn't work right now: UCB1 strategy adaptation is inter-session only — it cannot adjust strategy mid-conversation (`repl/mod.rs:2919`). `SessionRetrospective` analysis is computed but emitted only to `tracing::info!`, never stored or surfaced to the user (`domain/session_retrospective.rs`). `SignalArbitrator`, intended to mediate conflicts between `TerminationOracle` and `SynthesisGate`, is `#[deprecated]` with zero production callsites (`domain/signal_arbitrator.rs:112`). All 8 advanced feature flags (`use_halcon_md`, `enable_hooks`, `enable_auto_memory`, `enable_agent_registry`, `enable_semantic_memory`, `use_intent_pipeline`, `use_boundary_decision_engine`) default to `false`. For most users in a default configuration, Halcon runs as a capable but unaugmented multi-round agent.

Halcon is not production-ready by the definition of "stable enough for a paying customer as their primary coding agent." The `run_agent_loop()` function is a ~2,200-line monolith (`repl/agent/mod.rs`) coordinating 15+ concerns through a 62-field god-object `LoopState`. The Classic REPL passes `None` for the cancellation channel, so `Ctrl-C` has undefined behavior in non-TUI mode. `std::sync::Mutex` is used in async context in four modules, creating deadlock risk. The system has the architectural depth to be competitive in 3–6 months with focused wiring work, but is not there today.

---

## 2. MÉTRICAS BRUTAS

| Metric | Value | Source |
|---|---|---|
| Total Rust source files | 768 | `find crates/ -name "*.rs" \| wc -l` |
| Total lines of Rust code (workspace) | 317,632 | `wc -l` aggregate |
| `halcon-cli` lines | ~174,068 | `docs/audit/halcon-cli-audit-2026.md` |
| Test annotations (`#[test]` + `#[tokio::test]`) | 8,206 | grep workspace |
| Tests in `halcon-cli` alone | ~4,656 | audit doc |
| Crates in workspace | 19 | Cargo metadata |
| Tool implementations (`impl Tool`) | 65 | grep `halcon-tools/src/` |
| Tools in default registry | 58 | `halcon-tools/src/lib.rs` |
| Model providers implemented | 7 real + Echo + Replay | `crates/halcon-providers/src/` |
| `PolicyConfig` fields | ~120 | `halcon-core/src/types/policy_config.rs` |
| Feature flags off by default | 8 | `policy_config.rs:511–606` |
| CLI top-level commands | 21 | `main.rs` |
| SQLite tables tracked | 8 | `audit/query.rs` |
| CATASTROPHIC_PATTERNS | 18 | `halcon-core/src/security.rs` |
| DANGEROUS_COMMAND_PATTERNS | 12 | `halcon-core/src/security.rs` |

---

## 3. ESTADO DEL AGENT LOOP

### 3.1 Pre-loop setup

| Component | Status | File:Line | Condition | Notes |
|---|---|---|---|---|
| Provider fallback banner | CONDITIONAL | `agent/mod.rs:329-337` | requested ≠ actual provider name | Cosmetic only |
| `ContextPipeline::new` (L0–L4) | ACTIVE | `agent/mod.rs:411-429` | always | Cold archive loaded from disk |
| Context assembly via `ContextManager` | CONDITIONAL | `agent/mod.rs:477-494` | `context_manager is Some` | |
| `IntentScorer::score` | ACTIVE | `agent/mod.rs:500` | always | Multi-signal intent classification |
| `PlanningPolicy::decide` + `planner.plan()` | CONDITIONAL | `agent/mod.rs:512-576` | `planner is Some AND !SkipPlanning` | Returns `ExecutionPlan` |
| Tool selection (intent-based) | CONDITIONAL | `agent/mod.rs:640-720` | `tool_selection_enabled` | Narrows tool surface per intent |
| `InstructionStore` injection (HALCON.md) | CONDITIONAL | `agent/mod.rs:720-734` | `policy.use_halcon_md = true` | Off by default |
| `InputNormalizer::normalize` | ACTIVE | `agent/mod.rs:735` | always | Strips PII, normalizes query |
| `BoundaryDecisionEngine::evaluate` | CONDITIONAL | `agent/mod.rs:753` | `policy.use_boundary_decision_engine = true` | Off by default |
| `IntentPipeline::resolve` | CONDITIONAL | `agent/mod.rs:795` | `policy.use_intent_pipeline = true` | Off by default |
| Auto-memory injection | CONDITIONAL | `agent/mod.rs:800-850` | `policy.enable_auto_memory = true` | Off by default |
| Agent registry manifest injection | CONDITIONAL | `agent/mod.rs:850-900` | `policy.enable_agent_registry = true` | Off by default |
| `UserPromptSubmit` lifecycle hook | CONDITIONAL | `agent/mod.rs:900-930` | `policy.enable_hooks = true` | Off by default |
| `ConvergenceController::new_with_budget` | ACTIVE | `agent/mod.rs:1749-1785` | always | Calibrated from SLA tier |

### 3.2 Per-round execution

| Component | Status | File:Line | Condition | Notes |
|---|---|---|---|---|
| `ControlReceiver` check (cancel/pause) | CONDITIONAL | `round_setup.rs:~50` | `ctrl_rx is Some` | Classic REPL passes `None` — no cancellation |
| `InstructionStore` hot-reload | ACTIVE | `round_setup.rs:50-80` | always | No-ops if `use_halcon_md=false` |
| TBAC tool filter | ACTIVE | `round_setup.rs` | always | Narrows tools per `TaskContext` |
| `provider_round::run` (model + stream) | ACTIVE | `provider_round.rs` | always | Core model invocation |
| XML artifact filter (deepseek/ollama) | ACTIVE | `provider_round.rs:36-60` | always | Strips malformed function-call XML |
| `ToolLoopGuard` hash deduplication | ACTIVE | `post_batch.rs` | always | Prevents infinite tool loops |
| `execute_parallel_batch` (ReadOnly tools) | ACTIVE | `executor.rs` | when `ReadOnly` tools batched | `futures::join_all` |
| `execute_sequential` (Destructive tools) | ACTIVE | `executor.rs` | when `Destructive/ReadWrite` | One at a time with confirmation |
| `PreToolUse` hook | CONDITIONAL | `executor.rs:step5.6` | `hook_runner is Some AND enable_hooks` | Blocking (exit_code=2 → Deny) |
| `PostToolUse/Failure` hook | CONDITIONAL | `executor.rs:step6.5` | same | Best-effort, non-blocking |
| Plan step tracking | ACTIVE | `execution_tracker.rs` | always | Marks steps complete |
| `PostBatchSupervisor` evaluation | ACTIVE | `supervisor.rs` | always | Asserts post-conditions |
| Reflexion (`Reflector`) | CONDITIONAL | `reflexion.rs via post_batch` | `reflector is Some` | Requires `--reflexion` flag |
| Plugin pre/post gates | CONDITIONAL | `plugin_registry.rs via post_batch` | `plugin_registry is Some` | Dead for default sessions |
| `ConvergenceController::observe_round` | ACTIVE | `convergence_phase.rs:67` | always | Feeds ARIMA predictor |
| ARIMA anomaly detection | ACTIVE | `convergence_phase.rs:150-200` | always | Detects round-over-round divergence |
| `RoundScorer` evaluation | ACTIVE | `convergence_phase.rs:300-400` | always | Multi-signal round score |
| `TerminationOracle::adjudicate` | ACTIVE | `convergence_phase.rs:545` | always | Primary stop signal |
| `RoutingAdaptor` T1–T4 checks | CONDITIONAL | `convergence_phase.rs:561` | `boundary_decision is Some` | Requires `use_boundary_decision_engine=true` |
| `SynthesisGate::evaluate` | ACTIVE | `loop_state.rs:662,687` | always | GovernanceRescue enforcement |
| `LoopGuard` dispatch (dispatch! macro) | ACTIVE | `convergence_phase.rs:800-1300` | always | Routes convergence actions |
| `checkpoint::save` (fire-and-forget) | ACTIVE | `checkpoint.rs via agent/mod.rs` | always | tokio::spawn, not awaited |
| `AdaptivePolicy::observe` | ACTIVE | `domain/adaptive_policy.rs` | always | Adjusts temperature, retry policy |
| `StrategyWeights` intra-session adjust | ACTIVE | `domain/strategy_weights.rs` | always | Adjusts Bayesian weights |
| `SignalArbitrator::arbitrate` | ORPHAN | `domain/signal_arbitrator.rs:112` | never | `#[deprecated]`, 14 self-tests only |

### 3.3 Post-loop cleanup

| Component | Status | File:Line | Condition | Notes |
|---|---|---|---|---|
| `ExecutionTracker` plan synthesis mark | ACTIVE | `execution_tracker.rs` | always | Marks plan as synthesized |
| `SessionRetrospective::analyze` | ACTIVE | `domain/session_retrospective.rs` | always | Output goes to `tracing::info!` only — not stored |
| `result_assembly::build` (LoopCritic) | ACTIVE | `result_assembly.rs` | always | Adversarial evaluation of final answer |
| Auto-memory background write | CONDITIONAL | `auto_memory::record_session_snapshot` | `enable_auto_memory=true` | `tokio::spawn`, non-blocking |
| `Stop` lifecycle hook | CONDITIONAL | `hooks/ via agent/mod.rs` | `enable_hooks=true` | Best-effort |
| `reward_pipeline::compute_reward` (UCB1) | ACTIVE | `repl/mod.rs:2919` | post-session at REPL level | **Not inside `run_agent_loop()`** — inter-session only |
| `reasoning_engine.record_outcome` | ACTIVE | `repl/mod.rs:~2928` | post-session at REPL level | Same — inter-session UCB1 update |

---

## 4. INVENTARIO DE FUNCIONALIDADES

### 4.1 Razonamiento y planificación

| Capability | Status | Primary File |
|---|---|---|
| Single-shot task execution | COMPLETE | `agent/mod.rs` (default when `planner=None`) |
| Multi-round tool execution | COMPLETE | `agent/mod.rs` 'agent_loop + `convergence_phase.rs` |
| Pre-execution planning (`ExecutionPlan`) | COMPLETE | `agent/mod.rs:512-576` via `PlanningPolicy` |
| Replanning when step fails | COMPLETE | `convergence_phase.rs` `ConvergenceAction::Replan` arm |
| Convergence detection | COMPLETE | `TerminationOracle` + `ConvergenceController` + `RoundScorer` |
| Stagnation detection | COMPLETE | `ToolLoopGuard` hash dedup + `ConvergenceController::stagnation_window` |
| UCB1 strategy selection | PARTIAL | `StrategySelector` in `repl/mod.rs`; inter-session only — not per-round |

### 4.2 Herramientas disponibles

| Capability | Status | Primary File |
|---|---|---|
| bash (with security guard) | COMPLETE | `halcon-tools/src/bash.rs` |
| file_read, file_write, file_edit, file_delete | COMPLETE | `halcon-tools/src/file_*.rs` |
| grep, glob, directory_tree | COMPLETE | `halcon-tools/src/` |
| web_fetch, web_search, http_request, http_probe | COMPLETE | `halcon-tools/src/` |
| git (status, diff, log, add, commit, branch, stash, blame) | COMPLETE | `halcon-tools/src/git/` |
| Code analysis (symbol_search, semantic_grep, code_metrics, dep_graph) | COMPLETE | `halcon-tools/src/` |
| Docker, make, CI logs | COMPLETE | `halcon-tools/src/` |
| SQL query, JSON transform, template engine | COMPLETE | `halcon-tools/src/` |
| execute_test, test_run, code_coverage, lint_check | COMPLETE | `halcon-tools/src/` |
| secret_scan, checksum, openapi_validate | COMPLETE | `halcon-tools/src/` |
| background_start/output/kill | COMPLETE | `halcon-tools/src/background/` |
| search_memory (semantic retrieval) | COMPLETE | `halcon-tools/src/search_memory.rs` (injected when `enable_semantic_memory=true`) |
| Parallel tool execution (ReadOnly) | COMPLETE | `executor.rs` + `post_batch.rs` |
| Sequential enforcement (Destructive) | COMPLETE | `executor.rs` + `ConversationalPermissionHandler` |
| Tool retry with backoff | COMPLETE | `ToolRetryConfig` + `retry_mutation.rs` |
| Idempotency registry | COMPLETE | `repl/idempotency.rs` (conditional on Phase 14 flag) |

### 4.3 Gestión de contexto y memoria

| Capability | Status | Primary File |
|---|---|---|
| L0–L4 context pipeline | COMPLETE | `halcon-context/src/pipeline.rs` |
| Token budget tracking | COMPLETE | `agent/mod.rs:397-404` (model.context_window × 0.80) |
| Tool output truncation (`ToolOutputElider`) | COMPLETE | `halcon-context/src/elider.rs` |
| Semantic retrieval (TF-IDF cosine + MMR) | COMPLETE | `halcon-context/src/vector_store.rs` (when `enable_semantic_memory=true`) |
| HALCON.md 4-scope instruction system | COMPLETE | `repl/instruction_store/` (when `use_halcon_md=true`) |
| Auto-memory write (background, LRU-bounded) | COMPLETE | `repl/auto_memory/` (when `enable_auto_memory=true`) |
| Per-agent memory (registry + scope) | COMPLETE | `repl/agent_registry/` |

### 4.4 Seguridad y guardrails

| Capability | Status | Primary File |
|---|---|---|
| FASE-2 path gate (pre-execution, structural) | COMPLETE | `executor.rs:step5` |
| 18 CATASTROPHIC_PATTERNS | COMPLETE | `halcon-core/src/security.rs` |
| 12 DANGEROUS_COMMAND_PATTERNS (G7 hard veto) | COMPLETE | `halcon-core/src/security.rs` |
| TBAC (Tool-Based Access Control) | COMPLETE | `TaskContext` push/pop in `result_assembly.rs` |
| Circuit breakers | COMPLETE | `ToolFailureTracker` + `failure_tracker.rs` |
| Permission modes (ReadOnly / ReadWrite / Destructive) | COMPLETE | `executor.rs` + `permission_lifecycle.rs` |
| Guardrail output scan (before injecting to model) | PARTIAL | `repl/guardrails.rs` exists; coverage of all tool result paths not confirmed |

### 4.5 Sub-agentes y orquestación

| Capability | Status | Primary File |
|---|---|---|
| Sub-agent spawning | COMPLETE | `orchestrator.rs` + recursive `run_agent_loop()` |
| Wave-based orchestration (topological sort) | COMPLETE | `orchestrator.rs:dependency_waves()` |
| SharedBudget (atomic token budget) | COMPLETE | `orchestrator.rs:30-74` |
| Declarative sub-agent config (YAML frontmatter) | COMPLETE | `repl/agent_registry/loader.rs` |
| Agent registry (discoverable named agents) | COMPLETE | `repl/agent_registry/` (when `enable_agent_registry=true`) |
| Per-agent model selection | COMPLETE | `SubAgentTask.model: Option<String>` in `orchestrator.rs` |

### 4.6 MCP

| Capability | Status | Primary File |
|---|---|---|
| MCP client (stdio + HTTP SSE) | COMPLETE | `halcon-mcp/src/` + `repl/mcp_manager.rs` |
| OAuth 2.1 with PKCE S256 | COMPLETE | `halcon-mcp/src/oauth.rs` |
| HTTP SSE transport | COMPLETE | `halcon-mcp/src/http_transport.rs` |
| Tool search index (nucleo-matcher, deferred load) | COMPLETE | `halcon-mcp/src/tool_search.rs` |
| 3-scope MCP config (local > project > user) | COMPLETE | `halcon-mcp/src/scope.rs` |
| MCP server mode (stdio + HTTP/axum) | COMPLETE | `halcon-mcp/src/http_server.rs` + `commands/mcp_serve.rs` |

### 4.7 Superficies y acceso

| Capability | Status | Primary File |
|---|---|---|
| Interactive terminal REPL | COMPLETE | `repl/mod.rs` (reedline) |
| TUI (ratatui full-screen) | COMPLETE | `tui/` (`--features tui`) |
| Headless / scripted mode | COMPLETE | `agent_bridge/` (`--features headless`) |
| HTTP control plane API | COMPLETE | `commands/serve.rs` (axum) |
| JSON-RPC mode (VS Code bridge) | COMPLETE | `commands/json_rpc.rs` (`--mode json-rpc`) |
| VS Code extension | COMPLETE | `halcon-vscode/` (TypeScript, xterm.js webview) |
| WebSocket transport | ABSENT | Not found anywhere in codebase |
| Slash commands | COMPLETE | `repl/slash_commands.rs` |

### 4.8 Observabilidad y persistencia

| Capability | Status | Primary File |
|---|---|---|
| Session persistence (SQLite) | COMPLETE | `halcon-storage::AsyncDatabase` |
| 8 SQLite tables | COMPLETE | `audit/query.rs` (sessions, audit_log, policy_decisions, resilience_events, execution_loop_events, session_checkpoints, invocation_metrics, trace_steps) |
| Per-round checkpoint (fire-and-forget) | COMPLETE | `agent/checkpoint.rs` |
| DecisionTrace (BDE pipeline) | COMPLETE | `repl/decision_engine/decision_trace.rs` |
| Agent round audit trace | COMPLETE | `repl/domain/agent_decision_trace.rs` |
| SessionRetrospective analysis | PARTIAL | `domain/session_retrospective.rs` — computed, emitted to `tracing::info!` only, not stored |
| Export: JSONL, CSV, PDF | COMPLETE | `audit/` 7-module package (`halcon audit export`) |
| HMAC-SHA256 chain integrity verify | COMPLETE | `audit/integrity.rs` + `audit_hmac_key` table |

### 4.9 Lifecycle hooks

| Capability | Status | Primary File |
|---|---|---|
| `PreToolUse` (blocking, exit_code=2 denies) | COMPLETE | `executor.rs:step5.6` |
| `PostToolUse` (best-effort) | COMPLETE | `executor.rs:step6.5` |
| `PostToolUseFailure` (best-effort) | COMPLETE | `executor.rs:step6.5` |
| `UserPromptSubmit` (blocking) | COMPLETE | `agent/mod.rs Feature 2 block` |
| `Stop` (best-effort) | COMPLETE | `agent/mod.rs post-loop` |
| `SessionEnd` | COMPLETE | `repl/mod.rs cleanup` |
| Shell command runner | COMPLETE | `repl/hooks/command_hook.rs` |
| Rhai sandboxed script runner | COMPLETE | `repl/hooks/rhai_hook.rs` (Engine::new_raw, max_ops=10K) |

### 4.10 Configuración

| Capability | Status | Primary File |
|---|---|---|
| TOML config file | COMPLETE | `config/default.toml` |
| `PolicyConfig` (~120 fields) | COMPLETE | `halcon-core/src/types/policy_config.rs` |
| Environment variable override | COMPLETE | `config` crate |
| CLI flag override (`--provider`, `--model`, etc.) | COMPLETE | `main.rs` clap arguments |
| Multi-provider support | COMPLETE | 7 providers in `halcon-providers/` |
| Provider fallback chain | COMPLETE | `provider_round.rs:invoke_with_fallback()` |
| Feature flags (all off by default) | COMPLETE | `policy_config.rs:511-606` |

---

## 5. LO QUE FUNCIONA (EVIDENCIA REAL)

Each entry: verified implementation + hot-path callsite + at least one test.

| # | Capability | Key File | Test Evidence |
|---|---|---|---|
| 1 | Multi-round agent loop with configurable max_rounds | `agent/mod.rs:'agent_loop` | `repl/agent/` integration tests |
| 2 | FASE-2 path gate (independent of hooks, always fires) | `executor.rs:step5` | `fase2_catastrophic_patterns_independent_of_hook_outcome` |
| 3 | 18 CATASTROPHIC_PATTERNS + 12 DANGEROUS_COMMAND_PATTERNS | `halcon-core/src/security.rs` | 5 security tests |
| 4 | bash tool with stderr-redirect false-positive fix | `halcon-tools/src/bash.rs` | `stderr_redirect_to_dev_null_allowed` |
| 5 | Parallel tool execution (ReadOnly) | `executor.rs:execute_parallel_batch` | `parallel_batch_rejects_destructive_tools` |
| 6 | Sequential enforcement (Destructive guard) | `executor.rs` | `parallel_batch_rejects_file_write_as_destructive` |
| 7 | Tool alias resolution (`run_command` → bash) | `repl/tool_aliases.rs` + `executor.rs` | `alias_run_command_routes_to_bash_in_plan` |
| 8 | ToolLoopGuard deduplication (prevents infinite loops) | `post_batch.rs` | loop-guard tests |
| 9 | TerminationOracle adjudication | `domain/termination_oracle.rs` | oracle tests |
| 10 | ConvergenceController + ARIMA anomaly detection | `convergence_phase.rs:67,150-200` | convergence tests |
| 11 | SynthesisGate GovernanceRescue (reflection < 0.15, rounds < 3) | `domain/synthesis_gate.rs` | 4 synthesis_gate tests |
| 12 | RoundFeedback data integrity (real tool count, not zero) | `convergence_phase.rs:534-537` | `RoundFeedback` unit tests |
| 13 | Sub-agent wave orchestration (topological dependency) | `orchestrator.rs` | orchestrator tests |
| 14 | SharedBudget atomic token limit across sub-agents | `orchestrator.rs:30-74` | orchestrator tests |
| 15 | HALCON.md 4-scope instruction system + hot-reload | `repl/instruction_store/` | 21 instruction_store tests |
| 16 | RecommendedWatcher FSEvents hot-reload (<100ms) | `instruction_store/mod.rs` | hot-reload-within-200ms test |
| 17 | Auto-memory: scorer + writer + injector + LRU-bounded | `repl/auto_memory/` | auto_memory tests |
| 18 | Agent registry: YAML frontmatter, 3 scopes, levenshtein suggestions | `repl/agent_registry/` | 79 integration tests |
| 19 | Lifecycle hooks: shell + Rhai, 6 events, blocking semantics | `repl/hooks/` | 40 hook tests |
| 20 | MCP client (stdio + HTTP SSE) + OAuth 2.1 PKCE | `halcon-mcp/src/` | 92 halcon-mcp tests |
| 21 | MCP server mode (axum, Bearer auth, SSE, session TTL) | `halcon-mcp/src/http_server.rs` | 14 http_server tests |
| 22 | TF-IDF semantic memory (cosine sim + MMR) | `halcon-context/src/vector_store.rs` | 317 halcon-context tests |
| 23 | search_memory tool (session-injected, per-turn retrieval) | `halcon-tools/src/search_memory.rs` | 969 halcon-tools tests |
| 24 | Audit export: JSONL, CSV, PDF, HMAC-SHA256 chain | `crates/halcon-cli/src/audit/` | 7 audit unit tests + tamper detection test |
| 25 | AdaptivePolicy intra-session temperature + retry adjustment | `domain/adaptive_policy.rs` | adaptive_policy tests |
| 26 | ToolRetryConfig + argument mutation on transient failure | `retry_mutation.rs` | retry tests |
| 27 | InputNormalizer PII detection + normalization (always active) | `repl/input_boundary.rs` | input_boundary tests |
| 28 | L0–L4 context pipeline with cold archive | `halcon-context/src/pipeline.rs` | context pipeline tests |
| 29 | Token budget (80% of model context window) | `agent/mod.rs:397-404` | agent loop tests |
| 30 | 7 model providers (Anthropic, OpenAI, Gemini, DeepSeek, Ollama, ClaudeCode, OpenAI-compat) | `halcon-providers/src/` | provider tests |
| 31 | VS Code extension JSON-RPC bridge | `commands/json_rpc.rs` + `halcon-vscode/` | json_rpc tests |
| 32 | Circuit breakers (ToolFailureTracker threshold-based) | `repl/failure_tracker.rs` | failure_tracker tests |

---

## 6. LO QUE NO FUNCIONA (EVIDENCIA REAL)

### 6.1 Implementado pero no conectado (IMPLEMENTED, NOT WIRED)

**[GAP-1] UCB1 reward update es inter-sesión únicamente**
- `reward_pipeline::compute_reward()` se llama en `repl/mod.rs:2919`, que está al nivel del REPL después de que `run_agent_loop()` retorna. No se llama dentro del agent loop.
- Impacto: UCB1 no puede ajustar la estrategia a mitad de sesión. La documentación de "intra-session learning" es falsa para una sesión en curso.
- Fix: conectar `reasoning_engine.record_round_outcome(round_feedback)` en `convergence_phase.rs` después de cada `ConvergenceController::observe_round()`.

**[GAP-2] `SessionRetrospective` se computa pero se descarta**
- `SessionRetrospective::analyze()` produce análisis estructurado post-loop, pero el resultado se emite solo via `tracing::info!` (`domain/session_retrospective.rs`). No se escribe a SQLite, no se retorna en `AgentLoopResult`, no se presenta al usuario.
- Fix: incluir en `result_assembly::build()` return; guardar en tabla `session_checkpoints`.

**[GAP-3] `SignalArbitrator` es huérfano**
- `domain/signal_arbitrator.rs:112` está marcado `#[deprecated(since="0.3.0")]` con cero callsites en el agent loop. `TerminationOracle` y `SynthesisGate` pueden producir veredictos conflictivos sin mediador.
- Impacto: bloques GovernanceRescue (`synthesis_gate allow=false`) pueden ser silenciosamente sobreescritos cuando `TerminationOracle` retorna `Synthesize` — el orden en `loop_state.rs:662,687` vs `convergence_phase.rs:545` no está garantizado.

**[GAP-4] `FeedbackCollector` nunca agrega datos**
- `decision_engine/decision_feedback.rs` implementa `FeedbackCollector` con cero callsites en producción. El campo `escalated_mid_session` nunca se lee.
- Impacto: la eficiencia del routing no se mide; las escalaciones T3/T4 del `RoutingAdaptor` no se registran.

**[GAP-5] Classic REPL no tiene canal de cancelación**
- `repl/mod.rs` pasa `ctrl_rx: None` a `run_agent_loop()` para sesiones no-TUI. El check de cancelación en `round_setup.rs:~50` está gateado en `ctrl_rx is Some`. Ctrl-C envía SIGINT al proceso — comportamiento indefinido.
- Impacto: los usuarios no pueden cancelar gracefully una sesión agente en el modo terminal por defecto.

### 6.2 Parcialmente implementado (PARTIAL)

**[PARTIAL-1] RoutingAdaptor T3/T4 SLA budget sync incompleto**
- Funciona: escalación T3/T4 llama `conv_ctrl.set_max_rounds(current + delta)` (`convergence_phase.rs:561`).
- No funciona: el struct `sla_budget` no se actualiza. Las métricas de presión SLA siguen usando el budget original.

**[PARTIAL-2] SynthesisGate vs TerminationOracle — bug de ordenación**
- Funciona: `SynthesisGate::evaluate()` bloquea synthesis cuando `reflection_score < 0.15 && rounds_executed < 3`.
- No funciona: `TerminationOracle::adjudicate()` se ejecuta en `convergence_phase.rs:545` *antes* de que `SynthesisGate` sea consultado en `loop_state.rs:662`. Si el Oracle retorna `Synthesize`, el loop puede saltarse el gate.
- Confirmado: `docs/audit/integration-map-current.md:144-148`.

**[PARTIAL-3] Plugin system wiring**
- Funciona: `PluginRegistry` tipo existe; campo `plugin_registry: Option<Arc<Mutex<PluginRegistry>>>` en `AgentContext`.
- No funciona: `supervisor.rs:137` documenta `suspend_plugin()` como "NOT implemented". Cost tracking y circuit-breaker de plugins son dead code para todas las sesiones.

**[PARTIAL-4] Guardrail output scan**
- Funciona: `repl/guardrails.rs` existe con lógica de scan.
- No confirmado: cobertura de todos los paths de inyección de tool results al modelo. El audit doc nota este gap sin resolución de línea.

### 6.3 Stubs / placeholders (STUB)

| Módulo | Archivo | Propósito Original |
|---|---|---|
| `anomaly_detector.rs` | `repl/anomaly_detector.rs` | Detector standalone (ARIMA embebido en convergence_phase en su lugar) |
| `arima_predictor.rs` | `repl/arima_predictor.rs` | ARIMA standalone (mismo) |
| `backpressure.rs` | `repl/backpressure.rs` | Backpressure de tokens/rate; cero callsites |
| `capability_resolver.rs` | `repl/capability_resolver.rs` | Negociación dinámica de capacidades; cero callsites |
| `ci_detection.rs` | `repl/ci_detection.rs` | Probe de entorno CI; cero callsites en agent loop |
| `context_governance.rs` | `repl/context_governance.rs` | Control de acceso al contexto basado en política |
| `architecture_server.rs` | `repl/architecture_server.rs` | Servidor MCP de análisis de arquitectura |
| `codebase_server.rs` | `repl/codebase_server.rs` | Servidor MCP de contexto de codebase |
| `requirements_server.rs` | `repl/requirements_server.rs` | Servidor MCP de requirements |
| `workflow_server.rs` | `repl/workflow_server.rs` | Servidor MCP de workflow |

### 6.4 Ausente / no implementado (ABSENT)

| Capacidad | Evidencia de Ausencia |
|---|---|
| WebSocket transport | Sin dependencia WebSocket; no encontrado en `halcon-mcp/` ni `halcon-api/` |
| UCB1 per-round update (intra-sesión) | Sin callsite dentro de `run_agent_loop()` |
| `SignalArbitrator` en producción | `#[deprecated]`; integration-map-current.md confirma |
| `FeedbackCollector` aggregation | Cero callsites en producción |
| `SessionRetrospective` storage persistente | No en SQLite schema, no en `result_assembly.rs` return |
| Plugin circuit-breaker / cost tracking | `supervisor.rs:137` documenta explícitamente como NOT implemented |

---

## 7. POSTURA DE PRODUCCIÓN

### 7.1 Confiabilidad

Los errores de provider son manejados por `invoke_with_fallback()` en `provider_round.rs`, que itera una lista de providers de fallback y activa `ResilienceManager` en fallos consecutivos. El timeout se aplica via `limits.provider_timeout_secs` (default 300s en `AgentLimits`). Dynamic Budget Reconciliation reduce el budget del context pipeline cuando se hace fallback a un modelo con ventana de contexto menor.

Los fallos de herramientas son tracked por `ToolFailureTracker` (`failure_tracker.rs`) por tool+error_pattern. Al superar `policy.tool_failure_threshold` fallos, un circuit trip directive llega al agent loop. `retry_mutation.rs` provee mutación de argumentos para fallos transitorios. Estos paths tienen tests.

El output malformado del modelo es manejado en dos puntos: el XML artifact filter en `provider_round.rs:36-60` elimina XML de llamadas de función falso-positivo (bug específico de DeepSeek/Ollama), y `ToolLoopGuard` detecta pares idénticos `(tool, args_hash)` entre rondas para romper bucles infinitos.

Límites de sesión: `AgentLimits.max_rounds`, `max_total_tokens`, `max_duration_secs`, y `max_cost_usd` son todos aplicados (`halcon-core/src/types/config.rs:1161`). Los sub-agentes usan límites más ajustados via `ConvergenceController::new_for_sub_agent()`.

La cancelación solo es confiable en modo TUI. El Classic REPL pasa `ctrl_rx: None` a `run_agent_loop()`, por lo que Ctrl-C dispara un SIGINT a nivel OS, no una cancelación controlada.

### 7.2 Seguridad

Lo peor que el agente puede hacer sin confirmación por invocación: ejecutar comandos bash arbitrarios en una sesión pre-aprobada (cuando el usuario pasó `--full` o aprobó bash anteriormente en `permission_lifecycle.rs`). Los únicos stops duros son los 18 CATASTROPHIC_PATTERNS y 12 DANGEROUS_COMMAND_PATTERNS en `halcon-core/src/security.rs`. Comandos que no encajan en ninguno de los dos grupos se ejecutan sin confirmación cuando los permisos están pre-aprobados.

Patrones bloqueados en duro incluyen: `rm -rf /`, `mkfs.*`, `dd if=.*of=.*`, `shutdown`, `:(){ :|:& }; :` (fork bomb), `curl.*|.*sh`, `wget.*|.*sh`, y 11 más.

Gap de seguridad conocido: `repl/guardrails.rs` existe pero su cobertura de los paths de inyección de resultados de herramientas al modelo no está completamente verificada por el audit.

### 7.3 Observabilidad

En tiempo real: el TUI (`--features tui`) provee un display de 3 zonas con streaming de tool calls y actualizaciones del activity model via canal `UiEvent`. El Classic REPL hace streaming de tokens tal como llegan via `RenderSink`. Las completaciones de sub-agentes con error hints son visibles en el TUI (`activity_model::update_sub_agent_complete()`).

Post-sesión: `halcon audit list` / `halcon audit export` proveen exports JSONL, CSV, o PDF desde el audit log SQLite de 8 tablas. `halcon audit verify` retorna exit code 1 en caso de cadena HMAC tampering. Sin embargo, el análisis `SessionRetrospective` — el resumen de más alto nivel de qué fue mal y por qué — no está almacenado en ningún lugar accesible.

Los errores son expuestos al usuario via `RenderSink::error()` en las cuatro implementaciones (ClassicSink, TuiSink, bridge_sink, SilentSink). Los errores de sub-agentes incluyen un campo `error_hint: &str` agregado en Phase 2 Remediation.

### 7.4 Bugs conocidos

| ID | Descripción | Archivo:Línea |
|---|---|---|
| BV-1 | `ConvergenceController` calibrado para ventana de rondas larga, luego sobreescrito a ventana SLA corta; los thresholds de stagnation permanecen calibrados para la ventana original | `agent/mod.rs:1514-1548` |
| ARCH-SYNC-1 | `TerminationOracle` dispara antes de `SynthesisGate` en el orden del dispatch; veredictos `Synthesize` pueden bypasear bloques GovernanceRescue | `convergence_phase.rs:545` vs `loop_state.rs:662` |
| COUPLING-001 | `std::sync::Mutex` en contexto async en 4 módulos; riesgo de deadlock si el lock se sostiene a través de `.await` | `idempotency.rs:8,43`, `permission_lifecycle.rs:11`, `response_cache.rs:9` |
| ARCH-001 | `LoopState` god object: 62 campos públicos en 6 sub-structs; pasado como `&mut LoopState` a 8 archivos de phase | `repl/agent/loop_state.rs:477-579` |
| ARCH-002 | `run_agent_loop()` monolito: ~2200 líneas coordinando 15+ concerns | `repl/agent/mod.rs:281-end` |

### 7.5 Comparación honesta con Claude Code

| Dimensión | Halcon | vs Claude Code | Evidencia |
|---|---|---|---|
| Task completion reliability | Comparable para tareas bien acotadas | **Detrás**: sin cancelación en Classic REPL; COUPLING-001 deadlock risk; BV-1 calibration bug | `repl/mod.rs ctrl_rx=None`; `idempotency.rs:8` |
| Available tools | Más herramientas (65 impls vs ~20) | **Adelante**: Docker, CI logs, code coverage, SQL, secret scan, OpenAPI validate, background tasks | `halcon-tools/src/lib.rs` |
| Memory and context | Arquitectura más rica (L0–L4 + semantic + auto-memory) | **Adelante** con flags activos; **Equivalente** en defaults | `policy_config.rs:579,606` (off by default) |
| MCP ecosystem | MCP client+server+OAuth+tool search | **Adelante**: Halcon también puede actuar como servidor MCP con Bearer auth | `halcon-mcp/src/http_server.rs` |
| Safety and auditability | Más estructurado (HMAC chain, PDF export, 8 tablas) | **Adelante**: compliance audit export no existe en Claude Code | `audit/integrity.rs` |
| Ease of use | Detrás: 8 feature flags off; config compleja | **Detrás**: Claude Code funciona out of the box; Halcon requiere opt-in para sus features diferenciadoras | `policy_config.rs:511-606` |
| Code architecture | Detrás: monolito 2200 líneas, god object 62 campos | **Detrás**: deuda técnica se acumulará | `agent/mod.rs`, `loop_state.rs` |

---

## 8. MAPA DE DEPENDENCIAS DE FUNCIONALIDADES

```
FUNCIONA HOY (sin flags adicionales, always-on):
  ├── Multi-round tool execution (CATASTROPHIC + DANGEROUS guard)
  ├── FASE-2 path gate (structural, pre-hook)
  ├── TerminationOracle adjudication
  ├── ConvergenceController + ARIMA + RoundScorer
  ├── ToolLoopGuard deduplication
  ├── AdaptivePolicy (temperature + retry intra-session)
  ├── Parallel ReadOnly + Sequential Destructive execution
  ├── Sub-agent wave orchestration + SharedBudget
  ├── 7 model providers + fallback chain
  ├── L0–L4 context pipeline + token budget (80%)
  ├── Tool retry + circuit breakers
  ├── Per-round checkpoint (fire-and-forget SQLite)
  ├── Audit export (JSONL / CSV / PDF + HMAC verify)
  ├── MCP client (stdio + HTTP SSE) + OAuth 2.1
  ├── MCP server mode (stdio + HTTP/axum + Bearer auth)
  ├── JSON-RPC mode (VS Code bridge)
  └── Interactive REPL + TUI (--features tui)

FUNCIONA CON FLAGS ACTIVOS:
  ├── HALCON.md instructions (use_halcon_md=true)
  │     └── Enables: 4-scope injection, hot-reload, @import, path-glob rules
  ├── Auto-memory (enable_auto_memory=true)
  │     └── Enables: background write, LRU-bounded MEMORY.md, session injection
  ├── Agent registry (enable_agent_registry=true)
  │     └── Enables: .halcon/agents/*.md discovery, manifest injection, routing hints
  ├── Semantic memory (enable_semantic_memory=true)
  │     └── Enables: search_memory tool, VectorMemoryStore TF-IDF, MMR retrieval
  ├── Lifecycle hooks (enable_hooks=true)
  │     └── Enables: PreToolUse (blocking), PostToolUse, UserPromptSubmit, Stop, SessionEnd
  ├── BoundaryDecisionEngine (use_boundary_decision_engine=true)
  │     └── Enables: RoutingAdaptor T1–T4, mid-session escalation, SLA-aware routing
  └── IntentPipeline (use_intent_pipeline=true)
        └── Enables: full intent reconciliation (BDE + IntentScorer output merge)
        └── Depends on: use_boundary_decision_engine=true for full effect

REQUIERE TRABAJO PARA FUNCIONAR CORRECTAMENTE:
  ├── UCB1 per-round adaptation (reward_pipeline no está conectado dentro del agent loop)
  │     └── Fix: wire record_round_outcome() en convergence_phase.rs tras observe_round()
  │     └── Esfuerzo: 1–2 días
  ├── SynthesisGate fires antes del TerminationOracle (bug de ordenación)
  │     └── Fix: evaluar SynthesisGate ANTES de TerminationOracle::adjudicate()
  │     └── Esfuerzo: 1 día
  ├── Classic REPL cancellation (ctrl_rx=None → SIGINT no controlado)
  │     └── Fix: crear ControlChannel en repl/mod.rs y pasar ctrl_rx a run_agent_loop()
  │     └── Esfuerzo: 4–6 horas
  ├── SessionRetrospective persistent storage + surfacing
  │     └── Fix: incluir en result_assembly::build() return; guardar en SQLite
  │     └── Esfuerzo: 1 día
  ├── std::sync::Mutex → tokio::sync::Mutex en paths async
  │     └── Fix: reemplazar en idempotency.rs, permission_lifecycle.rs, response_cache.rs
  │     └── Esfuerzo: 1 día
  └── ConvergenceController calibration mismatch (BV-1)
        └── Fix: desacoplar stagnation_window del calibrado de intent-pipeline (agent/mod.rs:1514-1548)
        └── Esfuerzo: 1–2 días

REQUIERE IMPLEMENTACIÓN NUEVA:
  ├── WebSocket transport (ausente; sin dependencia de crate)
  ├── FeedbackCollector aggregation (decision_engine/decision_feedback.rs existe; wiring ausente)
  ├── Plugin circuit-breaker + cost tracking (supervisor.rs:137 "NOT implemented")
  ├── Guardrail scan en todos los paths de inyección de tool results
  └── SignalArbitrator: eliminar #[deprecated] o reemplazar con fix de ordenación real
```

---

## 9. PRÓXIMOS PASOS RECOMENDADOS

### Fix 1: Cancelación en Classic REPL (impacto alto, esfuerzo mínimo)

**Qué habilita**: Ctrl-C graceful en el modo terminal que usa el 100% de los usuarios que no usan `--tui`. Sin este fix, Ctrl-C durante una sesión larga termina el proceso abruptamente (perdiendo estado de sesión) o no hace nada si tokio captura SIGINT.

**Dónde hacer el cambio**:
1. `crates/halcon-cli/src/repl/mod.rs` — Crear un par `tokio::sync::mpsc::channel::<ControlAction>(1)` antes de llamar `run_agent_loop()`. Agregar un listener `tokio::signal::ctrl_c()` que envíe `ControlAction::Cancel` en el sender.
2. Pasar el receiver como `ctrl_rx: Some(rx)` a `run_agent_loop()`.
3. No se necesitan cambios dentro del agent loop — el path `check_control()` en `round_setup.rs:~50` ya maneja `ControlAction::Cancel`.

**Esfuerzo estimado**: 4–6 horas.

---

### Fix 2: SynthesisGate evalúa antes que TerminationOracle (bug de ordenación)

**Qué habilita**: Previene que el quality gate GovernanceRescue sea silenciosamente bypasseado. Hoy, cuando `reflection_score < 0.15` y `rounds_executed < 3`, `SynthesisGate` retorna `allow=false` — pero si `TerminationOracle` simultáneamente retorna `Synthesize`, el loop emite una respuesta de baja calidad sin advertencia.

**Dónde hacer el cambio**:
1. `convergence_phase.rs` — Mover la evaluación de `SynthesisGate::evaluate()` a *antes* de `TerminationOracle::adjudicate()` en el orden de dispatch de la ronda.
2. Cuando `SynthesisGate` retorna `allow=false`, downgrade el veredicto `Synthesize` del oracle a `Continue` (una ronda forzada más) o `StopWithWarning`.
3. Agregar un test de regresión para la interacción cross-gate; los 4 tests existentes de `synthesis_gate` cubren la condición boundary.

**Esfuerzo estimado**: 6–8 horas.

---

### Fix 3: Activar features por defecto o agregar `halcon init` interactivo

**Qué habilita**: Los 8 feature flags que diferencian Halcon de un agente multi-ronda básico (HALCON.md, auto-memory, lifecycle hooks, agent registry, semantic memory, BDE, IntentPipeline) son todos `false` por defecto. Un usuario nuevo que instala Halcon obtiene un agente capaz pero sin ninguna de las features diferenciadoras. Activar las más seguras por defecto expondría el valor real del sistema a cada usuario desde el primer run.

**Dónde hacer el cambio**:
1. `crates/halcon-core/src/types/policy_config.rs:511-606` — Cambiar defaults de al menos `use_halcon_md`, `enable_auto_memory`, y `enable_hooks` a `true`. Estos sistemas son de bajo riesgo y sus cambios de comportamiento son aditivos.
2. `config/default.toml` — Agregar las entradas correspondientes para que sean visibles y overridables.
3. Alternativa (menor riesgo): `crates/halcon-cli/src/commands/` — Agregar un flujo `halcon init` que pregunte "Enable auto-memory? [y/N]" y escriba a `~/.halcon/config.toml`.

**Esfuerzo estimado**: 4–8 horas (flip de defaults) o 1–2 días (wizard de init interactivo).
