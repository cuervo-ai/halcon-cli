# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.9] — 2026-03-19

### Added

- **Interactive Auth Gate**: When `halcon chat` starts with no authenticated provider, a rich crossterm terminal UI appears before the session begins. The gate detects all supported providers (cenzontle, anthropic, openai, deepseek, gemini, claude_code, ollama), shows their status, and lets the user configure one interactively:
  - **API key providers** (anthropic, openai, deepseek, gemini): masked input, saved securely to OS keystore and injected into the current process so the rebuilt registry picks it up immediately.
  - **OAuth/browser providers** (cenzontle, claude_code): launches the respective browser flow and waits for completion.
  - **Local providers** (ollama): shows setup instructions.
  - Skippable with `S` or `Esc` — the session continues and `precheck_providers_explicit` shows its normal error if no provider becomes available.
- **`commands/auth_gate.rs`**: new module — `probe_providers()`, `any_authenticated()`, `registry_has_no_real_providers()`, `run_if_needed()`, 9 unit tests.

### Changed

- `chat::run()` now calls the auth gate between registry build and provider precheck when no real AI provider is registered.

---

## [0.3.8] — 2026-03-19

### Fixed

- **Cenzontle as default provider**: When a cenzontle token is present in the OS keystore but the config still lists `default_provider = "anthropic"` (users who logged in before v0.3.8), halcon now automatically promotes cenzontle to the default at runtime and patches the on-disk config atomically. Requires no manual intervention.

---

## [0.3.7] — 2026-03-19

### Fixed

- **Cenzontle auto-detection**: Provider is now auto-registered when a valid token exists in the OS keystore from a prior `halcon auth login cenzontle` — no longer requires `cenzontle.enabled = true` in config. Existing authenticated users upgrade transparently.
- **TUI on Linux**: Release binaries for all Linux targets now include TUI rendering (`ratatui` + `tui-textarea`). Previously Linux binaries were compiled `headless` (no TUI). Clipboard (`arboard`) remains unavailable on cross-compiled Linux builds due to X11 header requirements; paste/copy operations gracefully return an error instead of crashing.

### Changed

- `tui` feature no longer includes `arboard`. New `tui-clipboard` feature opt-in adds clipboard support (macOS and native Linux builds). This is a compile-time-only change — runtime behavior is identical for users with clipboard support.

---

## [0.3.6] — 2026-03-19

### Security

- **Credential store hardening**: All OAuth tokens (access + refresh + expiry) are now written atomically via a single `rename(2)` syscall on Linux, eliminating the race window where an access token could be read without its associated expiry. Uses `set_multiple_secrets()` new API on `KeyStore`.
- **RFC 3986 scope encoding**: OAuth 2.1 authorization URL `scope` parameter now correctly uses `%20` for spaces instead of `+` (form-encoding). Using `+` caused interoperability failures with strict OAuth servers (RFC 6749 §3.3).
- **Concurrent tmp file safety**: `FileCredentialStore` now uses a per-write unique tmp suffix (`tmp.{nanos}{tid}`) preventing concurrent writers from clobbering each other's in-flight temp file during the `chmod 0600` + `rename` window.

### Fixed

- **Token refresh with unknown expiry**: `refresh_if_needed()` now attempts a silent refresh when token expiry is missing from the store (previously skipped, which could leave stale tokens after a partially-failed write).
- **Credential corruption logging**: `read_map_or_default()` now emits a `warn!` log when the credential file is corrupt instead of silently discarding it, making the issue diagnosable without `RUST_LOG=debug`.
- **No redundant D-Bus probe on display**: `print_store_outcome(Persisted)` no longer constructs a second `KeyStore` (which triggered a D-Bus probe on Linux headless environments) just to show the backend name.
- **Exact tokenization compaction thresholds**: Updated context compaction budget calculations for exact tiktoken counts (sprint1 exact tokenizer change).

### Added

- `KeyStore::set_multiple_secrets()` — atomic multi-key credential write, backed by `FileCredentialStore::set_multiple()` on Linux and sequential writes on OS keyring backends.
- `halcon-auth` crate: `test-support` feature flag enabling `FileCredentialStore::at(path)` for external integration test crates.
- `IntelligentRouter` + `IntentClassifier` in `halcon-providers::router` — regex-based, sub-microsecond intent routing across providers.
- `SemanticCache` in `halcon-context` — two-layer (SHA-256 + cosine similarity) in-process LLM response cache with tenant/model isolation and task-type TTLs.
- Exact tiktoken tokenization, prompt caching hints, fastembed embeddings, TTFT tracing (sprint1).

## [Unreleased] — Chat Integration + Desktop + Claude Code Provider (2026-02-25)

### Added

#### ARQ-001: Control Plane API + Chat Integration
- `halcon serve` command — HTTP/WebSocket server exposing Control Plane API on configurable port (default 9000)
- `ChatExecutor` trait in `halcon-core` — breaks circular dependency between `halcon-api` and `halcon-cli`
- `AgentBridgeImpl` — hexagonal bridge layer: `CoreChatExecutor::execute` spawns OS thread + `LocalSet::block_on` to run !Send agent loop headlessly
- REST endpoints: `POST /api/v1/chat/sessions`, `POST /api/v1/chat/sessions/:id/messages`, `GET /api/v1/chat/sessions`, etc. (7 handlers)
- WebSocket endpoint `/api/v1/ws` — real-time streaming: `ChatStreamToken`, `ConversationCompleted`, `ExecutionFailed`, `PermissionRequired`, `PermissionExpired`
- Session persistence: chat history stored/restored from SQLite on server restart
- `HALCON_API_TOKEN` bearer auth middleware on all `/api/v1` routes
- New types: `ChatSession`, `ChatMessage`, `ChatTokenUsage`, `PermissionRequest` in `halcon-api/types/chat.rs`
- New WS events: `SubAgentStarted`, `SubAgentCompleted`, `MediaAnalysisProgress`, `ChatSessionCreated` in `halcon-api/types/ws.rs`

#### Claude Code Provider
- `claude_code` provider — spawns `claude` CLI subprocess, communicates via NDJSON (`--print --output-format stream-json`)
- Root detection: `libc::getuid() == 0` → downgrades Auto→Chat mode (uid=0 blocks `--dangerously-skip-permissions`)
- Pre-spawn model update to avoid `send_set_model` on first use
- Nested session guard: removes `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`, `SUDO_COMMAND`, `SUDO_USER` from env
- Model path guard: skips `--model` flag when value contains `/` (command-path alias)
- `set_current_model()` method on `ManagedProcess` for post-spawn model sync
- Availability check: file-existence first + WARN log level

#### Desktop App (halcon-desktop)
- `views/chat.rs` — egui chat view with streaming token display, session list, message history
- `workers/` directory (8 new files): `connection.rs`, `chat_handlers.rs`, `media_handlers.rs`, `ws_translator.rs`, `mod.rs`, + message dispatch
- `ws_translator.rs` — translates `WsServerEvent` → typed `BackendMessage` variants (5 unit tests)
- Auto-reconnect: desktop reconnects 5s after WS close (both `Disconnected` and `ConnectionError`)
- Sub-agent panel with activity display (SubAgentStarted/Completed events)
- Permission modal widget (`widgets/permission_modal.rs`) — desktop-native tool approval UI
- Thinking bubble widget (`widgets/thinking_bubble.rs`) — animated UI for extended thinking display
- Activity panel widget (`widgets/activity_panel.rs`) — live tool execution feed

#### Multimodal Pipeline
- `MediaAttachmentInline` type in `halcon-core/traits/chat_executor.rs` — cross-cutting inline base64 attachment
- Magic-byte MIME detection + extension fallback in `workers/media_handlers.rs` (20MB limit enforced)
- `SubmitMessageRequest.attachments` → `ChatExecutionInput.media_attachments` → `TurnContext` → `ContentBlock::Image/Text`
- Drag-and-drop + attach button + file chips in `views/chat.rs`
- WS events: `MediaAnalysisStarted/Progress/Completed`

#### Agent Execution Hardening
- `LoopState` decomposition scaffolding in `loop_state_roles.rs`: `ControlSignals`, `LoopAccumulator`, `TokenBudget`, `SessionMetadata`, `SubsystemHealth`
- GDEM bridge (`agent_bridge/gdem_bridge.rs`) behind `feature="gdem-primary"` — IntentGraph expanded to 63 tools

### Fixed

#### Sub-Agent Orchestration (3 bugs — 3442 tests pass)
- **Orphan permission modals**: Sub-agents use `confirm_destructive=false` → `ui_event_handler.rs` now auto-approves when `reply_tx=Some` — no blocking modal shown for sub-agent tools
- **Description leak**: Pill labels now show clean `"Coder [3/3]"` format instead of raw 60-char instruction slice
- **Spinner never completing**: `sub_agent_spawned` and `sub_agent_completed` both now use `task_id_to_step` lookup for consistent step index

#### Tool Spinner/Skeleton (3442 tests pass)
- `ToolDenied` no longer leaves zombie spinners — `executing_tools.remove()` called + `deny_tool()` invoked
- `ToolOutcome` enum: `Success | Error | Denied` — replaces `is_error: bool`, clean 3-state outcome
- Renderer: Success=`✓` green, Error=`✗` red, Denied=`⊘` orange

#### Synthesis Pipeline (5 vulnerabilities — 3440 tests pass)
- **V1**: Pre-loop synthesis guard moved BEFORE `AUTONOMOUS_AGENT_DIRECTIVE` injection — directive no longer injected when `cached_tools=[]`
- **V2**: Post-orchestration sanitization in `round_setup.rs` — strips `## Autonomous Agent Behavior` sections when `round_request.tools.is_empty()`
- **V3**: `strip_tool_xml_artifacts()` in `provider_round.rs` — filters `<function_calls>`, `<invoke>`, `<halcon::tool_call>` XML from synthesis round text
- **V4**: Response cache skipped when `contains_tool_xml_artifacts()` — prevents cache poisoning
- **V5**: `LoopCritic` evaluates LAST 1500 chars of `full_text` (synthesis output), not FIRST 1500

#### UTF-8 Safety (289 tests pass)
- `segment.rs::truncate_text()` — replaced byte-index slice with `char_indices().nth(max_chars)` — prevents panic on multi-byte chars
- `assembler.rs::estimate_tokens()` — changed `text.len()/4` (bytes) to `text.chars().count()/4` (scalars) — CJK/emoji no longer over-counted 3-4×

#### macOS Code Signing
- `target/` directory owned by root → adhoc linker signature rejected by macOS 15.3 Sequoia taskgated
- Fix: `sudo chown -R oscarvalois:staff target/` + `codesign --force --sign - <binary>` after each release build

## [Unreleased] — SOTA Architecture + Permission Fixes (2026-02-23)

### Added

#### `halcon-agent-core` Crate — 10-Layer GDEM Architecture
- New standalone crate implementing Goal-Driven Execution Machine (GDEM) with 10 formal layers
- `AgentFsm` with states: Idle → Planning → Executing → Verifying → Converged / Error
- `UCB1Bandit` multi-armed bandit for strategy selection with `arm_stats()`, `record_outcome()`, `best_arm()`
- `GoalSpecParser` with `GoalSpec`, `KeywordPresence`, `ConfidenceScore` — typed goal specification
- `LoopCritic` in-loop goal verification: `Evidence` (tool_outputs, tools_called, assistant_text), `CriticVerdict`
- **127 tests pass** (was 74 after initial GDEM — +53 via Phase A+D hardening)
- Formal invariants (`invariants.rs`): I-1.1→I-5.2, proof methods PROVED/SIMULATED/ASSERTED
- `simulate_ucb1_convergence()`: deterministic proof that UCB1 converges on best arm (>85% fraction after 1000 rounds)
- Property-based tests with proptest: ConfidenceScore bounds, GAS monotonicity, UCB1 finiteness/infinity-for-unplayed

#### `halcon-sandbox` Crate — Execution Sandbox
- New standalone crate: macOS `sandbox-exec` + Linux `unshare` isolation
- Policy engine + executor with configurable resource limits (16 tests pass)

#### Session Metrics — GAS/RER/SCR/SID/ToolPR
- `SessionMetricsReport` with Goal Achievement Score (GAS): `0.6×confidence + 0.3×efficiency + 0.1×achieved_bonus`
- Tiers S/A/B/C/D, Runtime Efficiency Rate (RER), Success-to-Call Ratio (SCR), Skill-to-Invocation Density (SID)

#### SOTA Intent Architecture (IntentScorer + ModelRouter)
- `IntentScorer` multi-signal classifier: task_type, complexity, scope, reasoning_depth, suggested_max_rounds
- `ModelRouter` with `routing_bias_for()` — provider-aware model routing derived from IntentProfile
- Replaces keyword-only `TaskAnalyzer` with richer multi-dimensional intent profiling
- `IntentProfile.suggested_max_rounds()` caps UCB1 strategy's `max_rounds` (prevents over-allocation for conversational tasks)

#### Sub-Agent Pipeline Improvements
- `OrchestratorHeader` + `SubAgentTask` TUI activity lines — sub-agent progress visible in activity panel
- `Ctrl+B` toggles collapsed pill ↔ expanded tool+summary view for sub-agent results
- Context injection after sub-agent completion: sub-agent output injected into coordinator messages
- `PermissionAwaiter` callback: sub-agents route destructive tool permissions to TUI modal

### Fixed

#### Permission Modal (3 bugs resolved)
- **Silent timeout** (`permissions.rs`): When the 45-second TUI permission modal auto-denies (fail-closed), a `UiEvent::Warning` is now sent to the activity panel — user can see WHY the tool was denied even after missing the modal
- **Configurable timeout** (`permissions.rs`): TUI path now uses `config.tools.prompt_timeout_secs` (45s) instead of hardcoded 60s; stored as `tui_timeout_secs` with 30s floor
- **File path missing in delegation** (`delegation.rs`): `file_write` sub-agent instructions now include `Target file path: X` + `path="X"` directive — extracts from `expected_args.path` or infers via `infer_file_path()` (html→.html, python→.py, shell→.sh, etc.). Prevents sub-agents from generating content as text instead of calling file_write.

#### Orchestrator SOTA Gaps
- `allowed_tools` now filters tool definitions for sub-agents (sub-agents no longer see all 60+ tools)
- Sub-agent timeout capped at 200s (`SUB_AGENT_MAX_TIMEOUT_SECS=200`) — config `sub_agent_timeout_secs=200`
- `ConvergenceController` for sub-agents: max_rounds=6, stagnation_window=2, goal_coverage_threshold=0.10
- Multilingual keyword extraction: Spanish domain words translated to English for coverage matching (`estructura→structure`, `repositorio→repository`)
- `is_sub_agent: bool` field on `AgentContext` — sub-agent vs coordinator execution path separation

#### Tool Pipeline Fixes
- `native_search.rs`: uninitialized engine returns `is_error: true` (was false — caused model to retry infinitely)
- `executor.rs`: MCP pool connection errors reclassified as TRANSIENT (not deterministic) — enables recovery after temporary connection drops
- Tool output truncation: head+tail (60%+30%) UTF-8-safe — preserves both start AND end of long outputs

#### Agent Loop Fixes
- `LoopCritic`: uses `.rev().find()` for correct last-response extraction (not first)
- `ForcedSynthesis`: injects synthesis directive + `ForcedByOracle`, returns `NextRound` instead of immediately breaking
- UCB1 persistence: `match ... { Err(e) => warn!() }` instead of `let _ =` for visible error on DB failure
- Sub-agent `response_cache: None` — prevents caching of text-only "I will create..." responses as tool results

### Changed

#### Architecture Refactor — Clean Module Boundaries
- `repl/agent.rs` → `repl/agent/` module (provider_round, budget_guards, round_setup, convergence_phase, etc.)
- `repl/reasoning_engine.rs` → `repl/application/reasoning_engine.rs`
- `repl/strategy_selector.rs` → `repl/domain/strategy_selector.rs`
- `repl/task_analyzer.rs` → `repl/domain/task_analyzer.rs`
- `SessionManager` extracted from `repl/mod.rs` → `repl/session_manager.rs` (13 new tests)
- `ModelRouter` per-round: `forced_routing_bias` field on `LoopState` — single-round override without strategy mutation

### Tests
- **3404 total tests pass** (was 3396 before permission fixes, +8 new tests this session)
- New in this session: `file_write_with_explicit_path_uses_expected_args`, `file_write_infers_html/python_path`, `non_file_write_tools_have_no_path_hint`, `infer_html/python/shell_variants`, `infer_default_for_unknown`
- UCB1 closed-loop tests (Phase 9): `reward_pipeline_feeds_ucb1_strategy_learning`, `repeated_high_rewards_make_strategy_dominant`, `low_reward_does_not_mark_as_success`, `ucb1_total_experience_count_increments`

---

## [Unreleased] — Phase 78-80: HALCON V3 Plugin Suite (2026-02-19)

### Added

#### Plugin System V3 — 7 Plugins, 33 Herramientas
- Complete plugin infrastructure: `plugin_manifest.rs`, `plugin_registry.rs`, `plugin_circuit_breaker.rs`, `plugin_cost_tracker.rs`, `plugin_permission_gate.rs`, `capability_index.rs`, `capability_resolver.rs`
- UCB1 bandit per-plugin reward tracking with `record_reward()` + `select_best_for_capability()`
- BM25 `CapabilityIndex` with `exact_match()` fallback for deterministic plugin tool resolution
- `BatchVerdict::SuspendPlugin` in supervisor.rs — Gate 0 fires before existing batch gates
- `plugin_adjusted_reward()` — `(0.90 × base + 0.10 × plugin_success_rate).clamp(0.0, 1.0)`
- Plugin registry wired into `AgentContext` and `executor.rs` pre/post hooks

#### New Plugin: `halcon-otel-tracer` (Arquitectura — 5 herramientas)
- `trace_coverage_scan` — Mide cobertura de trazado: `#[tracing::instrument]`, spans manuales, OTel JS SDK, opentelemetry Python, Go otel spans
- `metric_inventory` — Inventario de métricas: `counter!`/`histogram!`/`gauge!` macros en Rust, MeterProvider en TS, Prometheus
- `log_pattern_scan` — Analiza patrones de logging: structured vs unstructured ratio, hotspots de `println!`/`console.log`
- `otel_compliance_check` — Verifica 7 puntos de cumplimiento OTel: exportadores, resource detection, W3C TraceContext, sampler
- `observability_health_report` — Score holístico 0-100: Trazado (40%), Métricas (30%), Logging (20%), Pipeline (10%)
- **Hallazgo real en HALCON**: 1% cobertura de trazado (3/205 archivos), 18 llamadas `println!`, 0% OTel → Grade D (16/100)

#### New Plugin: `halcon-perf-analyzer` (Frontend — 5 herramientas)
- `bundle_size_analyzer` — Indicadores de bundle JS/TS: importaciones dinámicas, barrel exports, librerías sin tree-shaking
- `lazy_loading_audit` — Auditoría de code-splitting: React.lazy, Suspense, React.memo, useCallback/useMemo, preload hints
- `render_blocking_scan` — Detección de recursos bloqueantes: `<script>` sin async/defer, inline `<style>` >2KB, Google Fonts sin font-display:swap
- `image_optimization_check` — Verificación de imágenes: >200KB, missing loading='lazy', alt attrs, width/height, WebP/AVIF
- `perf_health_report` — Score 0-100: Bundle Size (30%), Code Splitting (25%), Resource Loading (25%), Asset Optimization (20%)
- **Resultado en website/src**: Grade A (98/100)

#### New Plugin: `halcon-schema-oracle` (Backend — 5 herramientas)
- `db_schema_analyzer` — Analiza esquemas desde archivos SQL, Diesel `schema.rs`, entidades SeaORM
- `migration_health` — Auditoría de migraciones: reversibilidad, DROP sin Down, NOT NULL sin DEFAULT
- `index_advisor` — Sugerencias de índices: FKs sin índice, columnas filtradas frecuentemente, genera CREATE INDEX SQL
- `query_pattern_scan` — Patrones peligrosos: SELECT *, N+1 queries, joins cartesianos, SQL injection por concatenación
- `schema_health_report` — Score 0-100: Schema Richness (30%), Migraciones (25%), Query Safety (25%), FK Coverage (20%)
- **Nota**: HALCON usa SQL embebido en constantes Rust (no archivos .sql) — plugin reporta 0 tablas correctamente

#### Previously Added Plugins (Phase 79)
- `halcon-dev-sentinel` — 4 herramientas de seguridad: secret scanning, dependency audit, SAST, OWASP top 10
- `halcon-dependency-auditor` — 4 herramientas: auditoría Cargo.lock/package-lock.json, licencias, CVE
- `halcon-ui-inspector` — 5 herramientas: componentes UI, accesibilidad WCAG, rendimiento de renders
- `halcon-api-sculptor` — 5 herramientas: análisis REST/GraphQL, contratos OpenAPI, seguridad de endpoints

#### SOTA Meta-Cognition (Phases 73-78)
- `ReasoningEngine` + UCB1 `StrategySelector` — aprendizaje multi-armed bandit entre sesiones
- `LoopCritic` — evaluación autónoma de resultados del agente con umbral de confianza 0.80
- `RoundScorer` — puntuación por ronda: progress_delta×0.35 + tool_efficiency×0.30 + coherence×0.20 + token_score×0.15
- `PlanCoherenceChecker` — detección de drift semántico con umbral 0.70
- G1-G10 compliance gaps cerrados (Phantom Retry, Critic Separation, UCB1 Multi-Dim, ForceReplanNow, etc.)
- `StopCondition::EnvironmentError` + `StopCondition::CostBudget` para halts deterministas
- P0-A/B/C MCP dead-loop fixes: detección de servidores MCP caídos, circuit breaker, halt automático
- P1-A Parallel batch failure escalation, P1-B Compaction timeout escalation
- P2-C Cost budget hard stop, P2-D Deduplication visibility

### Fixed
- GOTCHA `extract_inline_attr` word boundary: `name="` coincidía dentro de `classname="` — fixed using `" name="` prefix
- GOTCHA BM25 IDF con documento único: idf = ln(4/3) ≈ 0.288 < MIN_PLUGIN_SCORE=0.5 → `exact_match()` bypass
- `Mutex<PluginRegistry>` en executor: `try_lock()` pattern para acceso concurrente en parallel batch

---

## [0.1.0] - 2026-02-14

### Added

#### Core Features
- Initial release of Cuervo CLI - AI-powered terminal assistant
- Multi-provider support (Anthropic Claude, OpenAI, DeepSeek, Ollama)
- Interactive REPL with rich terminal UI
- Full-featured TUI mode with multi-panel interface
- Model Context Protocol (MCP) integration
- Comprehensive tool system (file operations, git, directory tree, etc.)

#### Architecture
- Modular workspace architecture with 14 crates
- Async-first design with Tokio runtime
- Event-driven orchestration system
- Context management with automatic summarization
- Semantic memory with vector storage
- Audit logging and provenance tracking

#### TUI/UX
- Three-zone layout (Prompt, Activity, Status)
- Syntax highlighting for code blocks
- Real-time token usage and cost tracking
- Overlay system (Command Palette, Search, Help)
- Adaptive theming with color science (Momoto integration)
- Keyboard shortcuts and vim-style navigation
- Circuit breaker for API rate limiting
- Graceful degradation and error recovery

#### Security
- PII detection and redaction
- Sandbox mode for tool execution
- Dry-run mode for testing
- Keyring integration for secure credential storage
- Audit trail for all AI interactions
- Configurable safety guardrails

#### Distribution System
- One-line installation for Linux/macOS/Windows
- Automated cross-platform binary releases (6 targets)
- SHA256 checksum verification
- Automatic PATH configuration
- Fallback installation methods (cargo-binstall, cargo install)
- GitHub Actions CI/CD pipeline
- Comprehensive installation documentation

#### Documentation
- Quick start guide (5-minute setup)
- Complete installation guide with troubleshooting
- Visual installation examples
- Release process documentation
- Testing and validation guides
- API documentation and examples

#### Testing
- 1486+ passing tests across workspace
- Integration tests for core functionality
- TUI component tests
- Tool audit tests
- Installation script validation

### Technical Details

**Supported Platforms:**
- Linux x86_64 (glibc)
- Linux x86_64 (musl/Alpine)
- Linux ARM64
- macOS Intel (x86_64)
- macOS Apple Silicon (M1/M2/M3/M4)
- Windows x64

**Performance:**
- Optimized release builds (LTO, strip, size optimization)
- Lazy loading of heavy dependencies
- Streaming responses for real-time output
- Efficient context window management

**Developer Experience:**
- Hot-reloadable configuration
- Extensive logging with tracing
- Developer tools (stress tests, replay runner)
- Modular architecture for easy extension

---

[0.1.0]: https://github.com/cuervo-ai/cuervo-cli/releases/tag/v0.1.0
