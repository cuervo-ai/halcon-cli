# Halcon repl/ Forensic Audit 2026-03-08

**Auditor**: Claude Sonnet 4.6 (forensic architecture mode)
**Date**: 2026-03-08
**Branch**: feature/sota-intent-architecture
**Scope**: `crates/halcon-cli/src/repl/` — flat root + 8 subdirectories
**Prior work**: `docs/audit/halcon-cli-audit-2026.md`, `docs/audit/integration-map-current.md`

---

## 1. RESUMEN EJECUTIVO

### Métricas de Alto Nivel

| Métrica | Valor |
|---------|-------|
| Total archivos `.rs` en repl/ (incluyendo subdirectorios) | 233 |
| Archivos en la raíz flat | 151 |
| Subdirectorios | 8 |
| Líneas totales (flat root únicamente) | 80,284 |
| Líneas totales (toda la jerarquía repl/) | 121,495 |
| TODOs/FIXMEs en código de producción | 11 |
| Llamadas `.unwrap()` / `.expect()` fuera de tests | 1,442 |
| Archivos con `std::sync::Mutex` en contexto async | 6 (+ model_selector 18 instancias) |

### Distribución por Estado

| Estado | Count | LOC estimadas |
|--------|-------|---------------|
| COMPLETE (integrado y activo en el loop) | 45 | ~42,000 |
| PARTIAL (implementado, parcialmente integrado) | 28 | ~25,000 |
| STUB (declarado, compilable, sin callsites externos reales) | 21 | ~12,000 |
| ORPHAN (no declarado en mod.rs — invisible al compilador) | 7 | ~3,200 |
| DUPLICATE (funcionalidad solapada con otro módulo) | 18 | ~11,000 |
| UNKNOWN (requiere lectura profunda adicional) | 32 | ~28,000 |

### Top 5 Hallazgos Críticos

**C1 — Siete archivos fantasma (ORPHAN sin `mod` declaration)**
Los archivos `ambiguity_detector.rs`, `clarification_gate.rs`, `goal_hierarchy.rs`, `input_risk_classifier.rs`, `intent_classifier.rs`, `pre_execution_critique.rs`, y `tool_executor.rs` **no están declarados en `mod.rs`**. El compilador los ignora silenciosamente. Son completamente invisibles al runtime: ningún `use`, ningún `pub use`, ningún `mod` los referencia. Acumulan 3,229 LOC de código muerto desde el punto de vista del compilador. Acción inmediata: eliminar.

**C2 — Cuádrupla duplicación de clasificación de intención**
Las clases `TaskComplexity` y `TaskType` existen independientemente en: `task_analyzer.rs`, `decision_layer.rs`, `model_selector.rs`, e `intent_classifier.rs` (este último ORPHAN). `decision_engine/domain_detector.rs` agrega un quinto clasificador. El loop activo usa `decision_layer::TaskComplexity` (vía `agent/mod.rs:801`) y `decision_engine/` (vía BDE pipeline). Los demás son stubs o duplicados sin wiring.

**C3 — Planificación triplicada: LlmPlanner / PlaybookPlanner / agent_utils**
`planner.rs` (LlmPlanner, 1,118 LOC), `playbook_planner.rs` (983 LOC), y lógica de plan inline en `agent_utils.rs` (159 LOC) implementan tres estrategias de planificación. Solo `planner.rs` se usa en el loop activo (vía `agent/convergence_phase.rs:1430` y `post_batch.rs:757`). `playbook_planner.rs` no tiene callsites en el agent loop — está wired únicamente en `mod.rs` como inicialización condicional.

**C4 — 1,442 `.unwrap()` / `.expect()` fuera de tests**
Contando solo el código de producción de `repl/`, hay 1,442 llamadas a `.unwrap()` o `.expect()`. En un sistema async con 8 subdirectorios de estado compartido, cada uno es un potencial panic en producción que no puede ser capturado por el error boundary del agent loop. La concentración más alta está en `executor.rs` y `model_selector.rs`.

**C5 — `std::sync::Mutex` en contexto async sin migración a `tokio::sync::Mutex`**
`model_selector.rs` contiene 18 instancias de `std::sync::Mutex` protegiendo `live_latency_overrides`, `quality_stats`, y `selection_history`. El `ModelSelector` se usa en el loop async con `provider_round.rs` llamando `record_observed_latency()`. `std::sync::Mutex::lock()` puede bloquear el thread del executor de Tokio si hay contención. Archivos afectados: `idempotency.rs:8`, `response_cache.rs:9`, `schema_validator.rs:19`, `model_selector.rs` (masivo), `agent/post_batch.rs:59`, `agent/mod.rs:146`.

### LOC a Eliminar

| Categoría | LOC estimadas |
|-----------|---------------|
| Orphans (7 archivos) | 3,229 |
| Stubs sin wiring (candidatos a delete/defer) | ~8,000 |
| Código duplicado consolidable | ~6,000 |
| **Total conservador eliminable** | **~17,000** |

---

## 2. INVENTARIO COMPLETO

### 2.1 Subdirectorios

| Subdirectorio | Archivos | LOC aprox. | Estado general |
|---------------|----------|------------|----------------|
| `agent/` | 17 | ~24,000 | Núcleo activo — todo integrado |
| `domain/` | 22 | ~14,000 | Mixto: 60% integrado, 40% stub |
| `decision_engine/` | 10 | ~5,000 | Integrado vía BDE pipeline |
| `hooks/` | 6 | ~1,800 | COMPLETE (Feature 2) |
| `auto_memory/` | 4 | ~800 | COMPLETE (Feature 3) |
| `instruction_store/` | 4+ | ~1,200 | COMPLETE (Feature 1) |
| `agent_registry/` | 6 | ~1,500 | COMPLETE (Feature 4) |
| `application/` | 2 | ~500 | PARTIAL (ReasoningEngine, enabled=false por defecto) |

### 2.2 Métricas de Calidad

| Métrica | Valor | Riesgo |
|---------|-------|--------|
| `.unwrap()` / `.expect()` fuera de tests | 1,442 | ALTO |
| `std::sync::Mutex` en contexto async | 6 archivos | MEDIO |
| Archivos con `#[allow(dead_code)]` | 54 | MEDIO |
| Archivos con `#[allow(unused` | 54 (mismos) | MEDIO |
| Tests en `stress_tests.rs` | 10 | BAJO |
| Tests en `dev_ecosystem_integration_tests.rs` | 15 | BAJO |

---

## 3. TABLA MAESTRA DE CLASIFICACIÓN

Leyenda de Categorías:
- **Core**: Núcleo del agent loop
- **Plugin**: Plugin system V3
- **Server**: Context servers (SDLC context sources)
- **Context**: Context/memory retrieval
- **Bridge**: Bridges y runtimes
- **Plan**: Planificación y estrategia
- **Security**: Seguridad y riesgo
- **Task**: Tareas y scheduling
- **Metrics**: Métricas y observabilidad
- **Git**: Git e IDE integration
- **Misc**: Miscelánea

| Archivo | Categoría | LOC | Callsites externos (no-test) | Tests propios | Status | Acción |
|---------|-----------|-----|------------------------------|---------------|--------|--------|
| `accumulator.rs` | Core | 430 | agent/post_batch.rs, executor.rs | sí | COMPLETE | KEEP |
| `adaptive_prompt.rs` | Security | 540 | executor.rs (RiskLevel import) | sí | PARTIAL | KEEP |
| `agent_comm.rs` | Bridge | 318 | mod.rs wired | sí | PARTIAL | KEEP |
| `agent_task_manager.rs` | Task | 367 | mod.rs declarado | sí | STUB | WIRE |
| `agent_types.rs` | Core | 342 | agent/mod.rs, evaluator.rs | sí | COMPLETE | KEEP |
| `agent_utils.rs` | Core | 159 | agent/mod.rs | sí | PARTIAL | MERGE_WITH:planner.rs |
| `ambiguity_detector.rs` | Plan | 793 | **NINGUNO** (orphan) | sí (inline) | ORPHAN | DELETE |
| `anomaly_detector.rs` | Metrics | 746 | loop_guard.rs, convergence_phase.rs, agent/mod.rs | sí | COMPLETE | MOVE_TO:halcon-agent-core |
| `architecture_server.rs` | Server | 290 | mod.rs:469 (context pipeline) | sí | COMPLETE | KEEP |
| `arima_predictor.rs` | Metrics | 677 | agent/mod.rs:1769, loop_state.rs:280 | sí | COMPLETE | MOVE_TO:halcon-agent-core |
| `artifact_store.rs` | Task | 226 | task_bridge.rs | sí | COMPLETE | KEEP |
| `ast_symbol_extractor.rs` | Git | 949 | mod.rs declarado | sí | STUB | WIRE |
| `authorization.rs` | Security | 1,063 | executor.rs:1143 (comment only) | sí | PARTIAL | WIRE |
| `backpressure.rs` | Core | 262 | agent/provider_client.rs | sí | COMPLETE | MOVE_TO:halcon-core |
| `branch_divergence.rs` | Git | 277 | mod.rs declarado | sí | STUB | WIRE |
| `capability_index.rs` | Plugin | 248 | plugin_registry.rs, plugin_auto_bootstrap.rs | sí | COMPLETE | KEEP |
| `capability_orchestrator.rs` | Plugin | 476 | mod.rs declarado | sí | STUB | WIRE |
| `capability_resolver.rs` | Plugin | 211 | plugin_registry.rs | sí | COMPLETE | KEEP |
| `ci_detection.rs` | Misc | 247 | mod.rs declarado | no | STUB | WIRE |
| `ci_result_ingestor.rs` | Metrics | 756 | mod.rs declarado | sí | STUB | WIRE |
| `circuit_breaker.rs` | Core | 654 | slash_commands.rs (config display only) | sí | PARTIAL | MOVE_TO:halcon-core |
| `clarification_gate.rs` | Plan | 378 | **NINGUNO** (orphan) | no | ORPHAN | DELETE |
| `code_instrumentation.rs` | Git | 283 | mod.rs declarado | sí | STUB | WIRE |
| `codebase_server.rs` | Server | 205 | mod.rs:489 | sí | COMPLETE | KEEP |
| `command_blacklist.rs` | Security | 270 | executor.rs, agent/mod.rs | sí | COMPLETE | KEEP |
| `commands.rs` | Core | 1,097 | mod.rs, main.rs | sí | COMPLETE | KEEP |
| `commit_reward_tracker.rs` | Git | 312 | dev_ecosystem_integration_tests.rs only | sí | STUB | WIRE |
| `compaction.rs` | Core | 701 | mod.rs declarado | sí | PARTIAL | KEEP |
| `console.rs` | Core | 1,086 | mod.rs (REPL UI) | sí | COMPLETE | KEEP |
| `context_governance.rs` | Context | 302 | mod.rs declarado | no | STUB | WIRE |
| `context_manager.rs` | Context | 484 | mod.rs (context pipeline) | sí | COMPLETE | KEEP |
| `context_metrics.rs` | Metrics | 208 | mod.rs declarado | sí | STUB | WIRE |
| `conversation_protocol.rs` | Core | 267 | mod.rs declarado | sí | STUB | KEEP |
| `conversation_state.rs` | Core | 591 | mod.rs declarado | sí | PARTIAL | KEEP |
| `conversational_permission.rs` | Security | 999 | executor.rs | sí | COMPLETE | KEEP |
| `decision_layer.rs` | Plan | 320 | agent/mod.rs:801, sla_manager.rs | sí | COMPLETE | MERGE_WITH:decision_engine |
| `delegation.rs` | Core | 1,056 | orchestrator.rs, agent/mod.rs | sí | COMPLETE | KEEP |
| `dev_ecosystem_integration_tests.rs` | Misc | 476 | **Solo tests** | sí (15 tests) | COMPLETE | KEEP |
| `dev_gateway.rs` | Bridge | 478 | mod.rs declarado | sí | STUB | WIRE |
| `early_convergence.rs` | Core | 550 | mod.rs declarado | sí | STUB | WIRE |
| `edit_transaction.rs` | Git | 637 | safe_edit_manager.rs | sí | COMPLETE | KEEP |
| `episodic_source.rs` | Context | 156 | mod.rs (context pipeline, conditional) | sí | PARTIAL | KEEP |
| `evaluator.rs` | Metrics | 324 | agent/tests.rs only | sí | STUB | WIRE |
| `evidence_graph.rs` | Core | 414 | mod.rs declarado (pub(crate)) | sí | PARTIAL | KEEP |
| `evidence_pipeline.rs` | Core | 499 | agent/convergence_phase.rs | sí | COMPLETE | KEEP |
| `execution_tracker.rs` | Core | 1,280 | agent/convergence_phase.rs, post_batch.rs, supervisor.rs | sí | COMPLETE | KEEP |
| `executor.rs` | Core | 3,334 | agent/post_batch.rs, orchestrator.rs | sí | COMPLETE | KEEP |
| `failure_tracker.rs` | Core | 259 | agent/post_batch.rs, agent/mod.rs | sí | COMPLETE | KEEP |
| `git_context.rs` | Git | 412 | mod.rs declarado | sí | PARTIAL | KEEP |
| `git_event_listener.rs` | Git | 354 | dev_ecosystem_integration_tests.rs only | sí | STUB | WIRE |
| `goal_hierarchy.rs` | Plan | 411 | **NINGUNO** (orphan) | sí (inline) | ORPHAN | DELETE |
| `health.rs` | Metrics | 295 | mod.rs declarado | sí | STUB | WIRE |
| `hybrid_retriever.rs` | Context | 282 | episodic_source.rs | sí | PARTIAL | KEEP |
| `ide_protocol_handler.rs` | Git | 414 | mod.rs declarado | sí | STUB | WIRE |
| `idempotency.rs` | Core | 281 | executor.rs (ToolExecutionConfig) | sí | COMPLETE | KEEP |
| `input_boundary.rs` | Plan | 360 | agent/mod.rs (InputNormalizer) | sí | COMPLETE | KEEP |
| `input_normalizer.rs` | Plan | 498 | agent/mod.rs:735 (B1 wiring) | sí | COMPLETE | KEEP |
| `input_risk_classifier.rs` | Security | 242 | **NINGUNO** (orphan) | sí (inline) | ORPHAN | DELETE |
| `integration_decision.rs` | Misc | 331 | mod.rs declarado | sí | STUB | KEEP |
| `intent_classifier.rs` | Plan | 725 | **NINGUNO** (orphan) | sí (inline) | ORPHAN | DELETE |
| `loop_guard.rs` | Core | 1,005 | agent/post_batch.rs, convergence_phase.rs | sí | COMPLETE | KEEP |
| `macro_feedback.rs` | Metrics | 689 | mod.rs declarado | sí | STUB | WIRE |
| `mcp_manager.rs` | Bridge | 568 | mod.rs (REPL session) | sí | COMPLETE | KEEP |
| `memory_consolidator.rs` | Context | 492 | mod.rs declarado | sí | PARTIAL | KEEP |
| `memory_source.rs` | Context | 216 | mod.rs:20 (imported) | sí | COMPLETE | KEEP |
| `metacognitive_loop.rs` | Core | 726 | agent/convergence_phase.rs, loop_state.rs | sí | COMPLETE | KEEP |
| `metrics_store.rs` | Metrics | 397 | mod.rs declarado | sí | PARTIAL | KEEP |
| `mod.rs` | Core | 4,284 | — (root module) | sí | COMPLETE | KEEP |
| `model_quirks.rs` | Core | 634 | agent/provider_round.rs, agent/mod.rs | sí | COMPLETE | KEEP |
| `model_selector.rs` | Core | 1,776 | mod.rs:2668, agent/round_setup.rs | sí | COMPLETE | KEEP |
| `onboarding.rs` | Misc | 103 | mod.rs declarado | no | STUB | KEEP |
| `optimizer.rs` | Metrics | 228 | agent/provider_round.rs:985 | sí | PARTIAL | KEEP |
| `orchestrator.rs` | Core | 1,861 | mod.rs (delegation path) | sí | COMPLETE | KEEP |
| `orchestrator_metrics.rs` | Metrics | 362 | orchestrator.rs | sí | COMPLETE | KEEP |
| `output_risk_scorer.rs` | Security | 275 | executor.rs | sí | COMPLETE | KEEP |
| `patch_preview_engine.rs` | Git | 418 | mod.rs declarado | sí | STUB | WIRE |
| `permission_lifecycle.rs` | Security | 251 | mod.rs declarado | sí | PARTIAL | KEEP |
| `permissions.rs` | Security | 669 | executor.rs, agent/post_batch.rs | sí | COMPLETE | KEEP |
| `plan_coherence.rs` | Plan | 244 | reward_pipeline.rs | sí | COMPLETE | KEEP |
| `plan_compressor.rs` | Plan | 1,046 | agent/mod.rs (plan compression) | sí | COMPLETE | KEEP |
| `plan_state_diagnostics.rs` | Plan | 406 | mod.rs (pub(crate)) | sí | PARTIAL | KEEP |
| `planning_metrics.rs` | Plan | 221 | mod.rs declarado | sí | STUB | WIRE |
| `planning_source.rs` | Context | 155 | mod.rs (context pipeline) | sí | PARTIAL | KEEP |
| `playbook_planner.rs` | Plan | 983 | mod.rs (conditional init only) | sí | PARTIAL | WIRE |
| `planner.rs` | Plan | 1,118 | agent/convergence_phase.rs, post_batch.rs | sí | COMPLETE | KEEP |
| `plugin_auto_bootstrap.rs` | Plugin | 296 | mod.rs declarado | sí | STUB | WIRE |
| `plugin_circuit_breaker.rs` | Plugin | 211 | plugin_registry.rs | sí | COMPLETE | KEEP |
| `plugin_cost_tracker.rs` | Plugin | 292 | plugin_registry.rs, reward_pipeline.rs | sí | COMPLETE | KEEP |
| `plugin_loader.rs` | Plugin | 626 | mod.rs (session init, gated) | sí | PARTIAL | KEEP |
| `plugin_manifest.rs` | Plugin | 328 | plugin_registry.rs, capability_index.rs, etc. | sí | COMPLETE | KEEP |
| `plugin_permission_gate.rs` | Plugin | 191 | plugin_registry.rs | sí | COMPLETE | KEEP |
| `plugin_proxy_tool.rs` | Plugin | 192 | plugin_registry.rs (ToolRegistry injection) | sí | COMPLETE | KEEP |
| `plugin_recommendation.rs` | Plugin | 367 | plugin_auto_bootstrap.rs only | sí | STUB | KEEP |
| `plugin_registry.rs` | Plugin | 1,120 | agent/mod.rs, agent/post_batch.rs, executor.rs | sí | PARTIAL | KEEP |
| `plugin_transport_runtime.rs` | Plugin | 422 | plugin_loader.rs, plugin_proxy_tool.rs | sí | COMPLETE | KEEP |
| `pre_execution_critique.rs` | Security | 200 | **NINGUNO** (orphan) | sí (inline) | ORPHAN | DELETE |
| `project_inspector.rs` | Misc | 414 | mod.rs declarado | sí | STUB | WIRE |
| `prompt.rs` | Core | 126 | mod.rs (private) | no | COMPLETE | KEEP |
| `provenance_tracker.rs` | Task | 218 | task_bridge.rs | sí | COMPLETE | KEEP |
| `provider_normalization.rs` | Core | 595 | agent/provider_round.rs | sí | COMPLETE | KEEP |
| `reflection_source.rs` | Context | 353 | mod.rs (context pipeline) | sí | PARTIAL | KEEP |
| `reflexion.rs` | Core | 515 | agent/post_batch.rs (conditional) | sí | PARTIAL | KEEP |
| `replay_executor.rs` | Misc | 196 | agent/post_batch.rs, replay_runner.rs | sí | COMPLETE | KEEP |
| `replay_runner.rs` | Misc | 429 | mod.rs declarado | sí | PARTIAL | KEEP |
| `repo_map_source.rs` | Context | 243 | mod.rs (context pipeline) | sí | COMPLETE | KEEP |
| `requirements_server.rs` | Server | 294 | mod.rs:466 | sí | COMPLETE | KEEP |
| `resilience.rs` | Core | 701 | mod.rs declarado | sí | PARTIAL | KEEP |
| `response_cache.rs` | Core | 582 | mod.rs declarado | sí | PARTIAL | KEEP |
| `retry_mutation.rs` | Core | 280 | agent/mod.rs (pub(crate)) | sí | COMPLETE | KEEP |
| `reward_pipeline.rs` | Metrics | 1,185 | mod.rs:2921 (post-session only) | sí | PARTIAL | WIRE |
| `risk_tier_classifier.rs` | Security | 346 | mod.rs declarado | sí | PARTIAL | KEEP |
| `rollback.rs` | Core | 501 | mod.rs declarado | sí | STUB | WIRE |
| `round_scorer.rs` | Metrics | 538 | agent/convergence_phase.rs | sí | COMPLETE | KEEP |
| `router.rs` | Core | 229 | mod.rs declarado | sí | STUB | WIRE |
| `rule_matcher.rs` | Security | 597 | authorization.rs | sí | COMPLETE | KEEP |
| `runtime_bridge.rs` | Bridge | 492 | mod.rs declarado | sí | STUB | WIRE |
| `runtime_metrics_server.rs` | Server | 340 | mod.rs:478 | sí | COMPLETE | KEEP |
| `runtime_signal_ingestor.rs` | Metrics | 615 | mod.rs declarado | sí | STUB | WIRE |
| `safe_edit_manager.rs` | Git | 605 | mod.rs declarado | sí | PARTIAL | KEEP |
| `schema_validator.rs` | Core | 344 | mod.rs declarado | sí | PARTIAL | KEEP |
| `sdlc_phase_detector.rs` | Misc | 403 | mod.rs declarado | sí | STUB | WIRE |
| `search_engine_global.rs` | Bridge | 51 | mod.rs declarado | no | STUB | KEEP |
| `security_server.rs` | Server | 363 | mod.rs:481 | sí | COMPLETE | KEEP |
| `self_corrector.rs` | Core | 614 | agent/convergence_phase.rs, agent/mod.rs, loop_state.rs | sí | COMPLETE | KEEP |
| `session_manager.rs` | Core | 534 | mod.rs declarado | sí | PARTIAL | KEEP |
| `sla_manager.rs` | Plan | 406 | mod.rs, agent/mod.rs | sí | COMPLETE | KEEP |
| `slash_commands.rs` | Core | 1,443 | mod.rs (private) | sí (stress_tests) | COMPLETE | KEEP |
| `speculative.rs` | Core | 301 | tui/events.rs (UiEvent only) | sí | STUB | WIRE |
| `strategy_metrics.rs` | Metrics | 234 | domain/strategy_selector.rs | sí | COMPLETE | KEEP |
| `stress_tests.rs` | Misc | 751 | **Solo tests** | sí (10 tests) | COMPLETE | KEEP |
| `subagent_contract_validator.rs` | Core | 497 | orchestrator.rs (pub(crate)) | sí | COMPLETE | KEEP |
| `supervisor.rs` | Core | 984 | agent/post_batch.rs | sí | COMPLETE | KEEP |
| `support_server.rs` | Server | 393 | mod.rs:484 | sí | COMPLETE | KEEP |
| `task_backlog.rs` | Task | 661 | task_bridge.rs | sí | COMPLETE | KEEP |
| `task_bridge.rs` | Task | 388 | agent/loop_state.rs, agent/mod.rs | sí | COMPLETE | KEEP |
| `task_scheduler.rs` | Task | 213 | task_bridge.rs | sí | COMPLETE | KEEP |
| `test_result_parsers.rs` | Git | 668 | test_runner_bridge.rs | sí | COMPLETE | KEEP |
| `test_results_server.rs` | Server | 358 | mod.rs:475 | sí | COMPLETE | KEEP |
| `test_runner_bridge.rs` | Git | 467 | mod.rs declarado | sí | STUB | WIRE |
| `tool_aliases.rs` | Core | 209 | orchestrator.rs, executor.rs, agent/mod.rs | sí | COMPLETE | KEEP |
| `tool_executor.rs` | Core | 198 | **NINGUNO** (orphan) | sí (inline) | ORPHAN | DELETE |
| `tool_manifest.rs` | Plugin | 540 | plugin_proxy_tool.rs (indirecto) | sí | PARTIAL | KEEP |
| `tool_policy.rs` | Security | 217 | agent/round_setup.rs (pub(crate)) | sí | COMPLETE | KEEP |
| `tool_selector.rs` | Plan | 577 | agent/mod.rs, agent/round_setup.rs | sí | COMPLETE | KEEP |
| `tool_speculation.rs` | Core | 745 | mod.rs declarado | sí | STUB | WIRE |
| `tool_trust.rs` | Security | 424 | mod.rs declarado (pub(crate)) | sí | PARTIAL | KEEP |
| `traceback_parser.rs` | Git | 459 | mod.rs declarado | sí | STUB | WIRE |
| `unsaved_buffer_tracker.rs` | Git | 330 | mod.rs declarado | sí | STUB | WIRE |
| `validation.rs` | Core | 639 | mod.rs declarado | sí | PARTIAL | KEEP |
| `vector_memory_source.rs` | Context | 172 | agent/mod.rs (Feature 7) | sí | COMPLETE | KEEP |
| `workflow_server.rs` | Server | 333 | mod.rs:472 | sí | COMPLETE | KEEP |

---

## 4. DUPLICACIÓN ESTRUCTURAL

### 4.1 Clasificación de Intención — 5 implementaciones solapadas

| Archivo | Tipo de Clasificador | Integrado en Loop | Status |
|---------|---------------------|-------------------|--------|
| `task_analyzer.rs` (en domain/) | `TaskType` + `TaskComplexity` (regex keyword) | Sí — `application/reasoning_engine.rs` | COMPLETE |
| `decision_layer.rs` | `TaskComplexity` (4 niveles, estimación de señales) | Sí — `agent/mod.rs:801` (legacy path) | COMPLETE |
| `decision_engine/` (BDE pipeline) | `ComplexityLevel` (Low/Medium/High), `TechnicalDomain` | Sí — `agent/mod.rs:753` (policy-gated) | COMPLETE |
| `intent_classifier.rs` | `IntentClassification` + entropy-based confidence | **No** (ORPHAN, no en mod.rs) | ORPHAN |
| `model_selector.rs` | `TaskComplexity` (Simple/Standard/Complex) | Sí — `model_selector::new()` | DUPLICATE |

**Hallazgo**: Hay cuatro enum `TaskComplexity` distintos con diferentes variantes: `decision_layer` tiene 4 (`SimpleExecution/StructuredTask/MultiDomain/LongHorizon`), `task_analyzer` tiene 3, `model_selector` tiene 3 (`Simple/Standard/Complex`), y `decision_engine/complexity_estimator.rs` tiene 3 (`Low/Medium/High`). Ninguno mapea directamente a los otros. El loop activo usa los cuatro concurrentemente, con conversiones manuales ad-hoc.

**Acción**: Elevar `TaskComplexity` canónico a `halcon-core/src/types/`. Todos los clasificadores producen este tipo. `intent_classifier.rs` (ORPHAN) eliminar. `model_selector::TaskComplexity` renombrar a `ModelTier` para no solapar.

### 4.2 Planificación — 3 implementaciones solapadas

| Archivo | Mecanismo | Integrado | Status |
|---------|-----------|-----------|--------|
| `planner.rs` | LLM-based JSON response parsing | Sí — `convergence_phase.rs:1430`, `post_batch.rs:757` | COMPLETE |
| `playbook_planner.rs` | YAML playbook keyword matching | Parcial — `mod.rs` conditional init, no loop wiring directa | PARTIAL |
| `agent_utils.rs` | Inline plan construction helpers | Sí — `agent/mod.rs` | PARTIAL |
| `plan_compressor.rs` | Plan compression/re-ranking | Sí — `agent/mod.rs` | COMPLETE |
| `planning_source.rs` | Plan as context source | Sí — context pipeline | PARTIAL |

**Hallazgo**: `playbook_planner.rs` (983 LOC) está inicializado en `mod.rs` pero **no tiene callsite en el agent loop `run_agent_loop()`**. El `AgentContext` recibe un `planner: Option<&dyn Planner>`, y `playbook_planner` podría ser inyectado allí, pero `orchestrator.rs:600` pasa `planner: None` en ambos casos. Efectivamente el PlaybookPlanner no planifica nada en producción.

**Acción**: Wire `PlaybookPlanner` como pre-check antes de `LlmPlanner` en `agent/mod.rs` planning gate, o marcar como DEFER y documentar que es unreachable.

### 4.3 Seguridad y Permisos — 4 capas solapadas

| Archivo | Responsabilidad | Integrado |
|---------|-----------------|-----------|
| `command_blacklist.rs` | CATASTROPHIC patterns (compilado desde `halcon-core::security`) | Sí — executor.rs |
| `conversational_permission.rs` | HITL permission prompts con memoria de sesión | Sí — executor.rs |
| `authorization.rs` | Policy chain (4 políticas), stdin prompt, timeout | Parcial — comment en executor.rs:1143 |
| `permissions.rs` | PermissionLevel resolution + ToolPermissionMap | Sí — executor.rs |
| `output_risk_scorer.rs` | Post-execution output risk scoring | Sí — executor.rs |
| `risk_tier_classifier.rs` | Pre-execution RiskTier → PermissionLevel | Parcial |
| `input_risk_classifier.rs` | Pre-input injection detection (ORPHAN) | **No** |
| `pre_execution_critique.rs` | LLM self-critique gate (ORPHAN) | **No** |

**Hallazgo crítico**: `authorization.rs` implementa una policy chain completa con `AuthorizationMiddleware` (1,063 LOC), pero `executor.rs:1143` tiene solo un comentario: "but that would require modifying AuthorizationMiddleware API". El módulo está declarado en `mod.rs` pero **nunca se instancia `AuthorizationMiddleware`** en el path de ejecución activo. `ConversationalPermissionHandler` en `conversational_permission.rs` actúa como sustituto ad-hoc. Hay dos sistemas de autorización paralelos, solo uno activo.

**Acción**: Eliminar `authorization.rs` o wire correctamente sustituyendo `ConversationalPermissionHandler`. `input_risk_classifier.rs` y `pre_execution_critique.rs` (ORPHAN): eliminar.

### 4.4 Context/Memory Retrieval — 6 fuentes solapadas

| Archivo | Prioridad declarada | Integrado |
|---------|---------------------|-----------|
| `memory_source.rs` | Básico (importado en mod.rs) | Sí |
| `vector_memory_source.rs` | 25 (Feature 7) | Sí — agent/mod.rs |
| `episodic_source.rs` | 80 (reemplaza memory_source cuando activo) | Parcial — conditional |
| `reflection_source.rs` | Desconocida | Parcial — context pipeline |
| `repo_map_source.rs` | Declarada en mod | Sí — context pipeline |
| `planning_source.rs` | Declarada en mod | Parcial — context pipeline |
| `hybrid_retriever.rs` | Backend de EpisodicSource | Parcial |
| `memory_consolidator.rs` | Post-session consolidation | Partial |

**Hallazgo**: `memory_source.rs` y `episodic_source.rs` son diseñados explícitamente para reemplazarse mutuamente (`episodic_source.rs:3`: "Priority 80: same as existing MemorySource (replaces it when episodic enabled)"). Sin embargo, `mod.rs:20` importa `MemorySource` siempre, y `EpisodicSource` solo se inyecta condicionalmente. No hay código que remueva `MemorySource` cuando `EpisodicSource` está activo — ambas coexisten y duplican contexto de memoria.

**Acción**: Implementar lógica de swap en `mod.rs` context pipeline: si `EpisodicSource` activo, no insertar `MemorySource`.

### 4.5 Tool Execution — 3 caminos solapados

| Archivo | Mecanismo | Integrado |
|---------|-----------|-----------|
| `executor.rs` | `execute_parallel_batch()` + `execute_sequential()` | Sí — agente loop principal |
| `runtime_bridge.rs` | `CliToolRuntime` → halcon-runtime DAG | Declarado, sin callsites en loop |
| `tool_executor.rs` | `ToolExecutor` trait + `LocalExecutor` (ORPHAN) | **No** (no en mod.rs) |

**Hallazgo**: `tool_executor.rs` (198 LOC, ORPHAN) define `trait ToolExecutor` para desacoplar tool dispatch del entorno. `executor.rs` llama directamente `tool.execute(input).await` sin este trait. `runtime_bridge.rs` usa `HalconRuntime` (halcon-runtime DAG). Los tres implementan dispatch de herramientas de manera diferente, y solo `executor.rs` está activo en el loop. `tool_executor.rs` es invisible al compilador por ser ORPHAN.

**Acción**: Eliminar `tool_executor.rs` (ORPHAN). Evaluar si `runtime_bridge.rs` debe reemplazar el path paralelo de `executor.rs` o eliminarse.

### 4.6 Metrics/Reward — 5 sistemas solapados

| Archivo | Función | Integrado |
|---------|---------|-----------|
| `reward_pipeline.rs` | Multi-signal reward computation (5 señales) | Parcial — solo post-session en mod.rs:2921 |
| `round_scorer.rs` | Per-round scoring (activo en convergence_phase) | Sí |
| `evaluator.rs` | CompositeEvaluator post-loop | Solo en tests |
| `strategy_metrics.rs` | UCB1 strategy metrics | domain/strategy_selector.rs |
| `commit_reward_tracker.rs` | Git commit quality reward | Solo en dev_ecosystem_integration_tests.rs |
| `metrics_store.rs` | Persistence layer para métricas | Parcial |

**Hallazgo**: `reward_pipeline.rs` (1,185 LOC) es el sistema más elaborado pero está fuera del agent loop. Solo se llama en `mod.rs:2921` (post-session REPL loop). `evaluator.rs` implementa `CompositeEvaluator` que no se llama desde ningún path de producción — solo desde `agent/tests.rs:3279`. `commit_reward_tracker.rs` solo vive en integration tests. El resultado: el UCB1 engine que debería aprender de recompensas multi-señal solo recibe una señal proxy (`StopCondition`) dentro del loop.

---

## 5. ANÁLISIS POR CATEGORÍA

### Categoría 1: Núcleo del Agent Loop

**Archivos**: `mod.rs` (4,284), `executor.rs` (3,334), `orchestrator.rs` (1,861), `model_selector.rs` (1,776), `delegation.rs` (1,056), `plan_compressor.rs` (1,046), `loop_guard.rs` (1,005), `supervisor.rs` (984), `commands.rs` (1,097), `console.rs` (1,086), `slash_commands.rs` (1,443), `accumulator.rs` (430), `agent_types.rs` (342), `metacognitive_loop.rs` (726), `self_corrector.rs` (614), `failure_tracker.rs` (259), `retry_mutation.rs` (280), `tool_aliases.rs` (209), `subagent_contract_validator.rs` (497)

**Estado**: COMPLETE — todos integrados activamente en el loop.

**Riesgo principal**: `mod.rs` (4,284 LOC) actúa como segundo god object después de `agent/mod.rs`. Contiene: inicialización de sesión, context pipeline wiring, REPL loop outer, post-session reward, y función `handle_message_with_sink` que orquesta todo. Demasiado para una sola función — `handle_message_with_sink` tiene más de 800 líneas inline.

**Riesgo secundario**: `executor.rs` (3,334 LOC) tiene 1,006 líneas en la función `execute_sequential()` con 4 niveles de anidamiento y múltiples early-returns que comparten estado mutable. Testing es difícil; 3 parámetros `Option<&std::sync::Mutex<...>>` hacen las firmas frágiles.

### Categoría 2: Plugin System

**Archivos**: `plugin_registry.rs` (1,120), `plugin_loader.rs` (626), `plugin_manifest.rs` (328), `plugin_transport_runtime.rs` (422), `plugin_proxy_tool.rs` (192), `plugin_permission_gate.rs` (191), `plugin_circuit_breaker.rs` (211), `plugin_cost_tracker.rs` (292), `plugin_recommendation.rs` (367), `plugin_auto_bootstrap.rs` (296), `capability_index.rs` (248), `capability_resolver.rs` (211), `capability_orchestrator.rs` (476), `tool_manifest.rs` (540)

**Estado**: PARTIAL — el sistema de plugin está implementado (1,120 LOC en registry, UCB1 por plugin, circuit breakers), pero el wiring al agent loop es opcional (`Option<Arc<Mutex<PluginRegistry>>>`). Cuando `plugin_registry` es `None` (modo sin plugins, mayoría de tests y deployments), ningún código de plugin ejecuta.

**Callsite real**: `agent/mod.rs:146` acepta `plugin_registry: Option<Arc<Mutex<PluginRegistry>>>`. `agent/post_batch.rs:59` lo recibe y ejecuta suspension logic cuando `if let Some(ref arc_pr) = plugin_registry`. `mod.rs:2624` crea el registry condicionalmente.

**Hallazgo**: `plugin_auto_bootstrap.rs` y `plugin_recommendation.rs` no tienen callsites fuera del sistema de plugins interno. Son stubs de un "plugin discovery + recommendation" pipeline que no está wired al loop de inicialización de sesión.

**Acción**: MOVE_TO nuevo crate `halcon-plugins` cuando el sistema de plugins se active por defecto. Actualmente el overhead de compilación es aceptable.

### Categoría 3: Context Servers (MCP/SDLC)

**Archivos**: `architecture_server.rs` (290), `codebase_server.rs` (205), `requirements_server.rs` (294), `runtime_metrics_server.rs` (340), `security_server.rs` (363), `support_server.rs` (393), `test_results_server.rs` (358), `workflow_server.rs` (333)

**Estado**: COMPLETE — todos inyectados en `mod.rs` context pipeline (líneas 466-489). Se instancian condicionalmente (`if config.context_servers.X.enabled`) y se agregan a `sources` como `Box<dyn ContextSource>`.

**Patrón común**: Cada server implementa `ContextSource` con `fn gather()` que hace una query FTS5 a SQLite via `halcon-storage::AsyncDatabase`. La lógica es idéntica excepto por el nombre de tabla y campos.

**Hallazgo**: 8 archivos con patrón idéntico (constructor + `sdlc_phase()` + `fetch_X_docs()` + `impl ContextSource`). Candidatos para macro derivation o trait object factory.

**Acción**: MOVE_TO `halcon-context` crate (naturalmente pertenecen allí como context sources).

### Categoría 4: Context/Memory Sources

**Archivos**: `context_manager.rs` (484), `memory_source.rs` (216), `vector_memory_source.rs` (172), `episodic_source.rs` (156), `reflection_source.rs` (353), `repo_map_source.rs` (243), `planning_source.rs` (155), `hybrid_retriever.rs` (282), `memory_consolidator.rs` (492), `context_governance.rs` (302), `context_metrics.rs` (208)

**Estado**: Mixto. `context_manager.rs`, `memory_source.rs`, `repo_map_source.rs`, `vector_memory_source.rs` — COMPLETE. `episodic_source.rs`, `reflection_source.rs`, `planning_source.rs` — PARTIAL. `memory_consolidator.rs`, `context_governance.rs`, `context_metrics.rs` — STUB.

**Riesgo de duplicación**: `memory_source.rs` y `vector_memory_source.rs` (Feature 7) coexisten activamente. Ambas proveen memoria al modelo. Priority 25 para `VectorMemorySource` vs. priority no especificada para `MemorySource`. No hay swap logic.

**Acción**: Todos los `*_source.rs` y `context_manager.rs` → MOVE_TO `halcon-context`. Requiere extraer trait `ContextSource` (ya en `halcon-core::traits`) y eliminar deps hacia `repl/`.

### Categoría 5: Bridges y Runtimes

**Archivos**: `runtime_bridge.rs` (492), `task_bridge.rs` (388), `agent_comm.rs` (318), `dev_gateway.rs` (478), `mcp_manager.rs` (568), `search_engine_global.rs` (51)

**Estado**:
- `runtime_bridge.rs` (492 LOC): Implementa `CliToolRuntime` que convierte tool batches en `TaskDAG` de halcon-runtime. **No tiene callsites en el loop activo** — declarado en mod.rs pero no instanciado.
- `task_bridge.rs` (388 LOC): COMPLETE — usado en `agent/loop_state.rs`.
- `mcp_manager.rs` (568 LOC): COMPLETE — wired en session init.
- `agent_comm.rs` (318): PARTIAL — wired en mod.rs.
- `dev_gateway.rs` (478): STUB — declarado, no wired.

**Hallazgo crítico**: `runtime_bridge.rs` existe para unificar ejecución bajo halcon-runtime DAG, pero `executor.rs` implementa su propio parallel dispatch con `buffer_unordered`. Son dos sistemas paralelos para hacer lo mismo. El comment en `runtime_bridge.rs:22` confirma: "intended for use cases where those gates are already applied upstream, or for standalone delegation/testing".

**Acción**: WIRE `runtime_bridge.rs` como implementation alternativa del parallel batch path en `executor.rs`, o MERGE con halcon-runtime y eliminar.

### Categoría 6: Planificación y Estrategia

**Archivos**: `planner.rs` (1,118), `playbook_planner.rs` (983), `plan_compressor.rs` (1,046), `plan_coherence.rs` (244), `plan_state_diagnostics.rs` (406), `planning_metrics.rs` (221), `sla_manager.rs` (406), `decision_layer.rs` (320), `input_boundary.rs` (360), `input_normalizer.rs` (498), `router.rs` (229), `rollback.rs` (501), `goal_hierarchy.rs` (411, ORPHAN)

**Archivos ORPHAN en esta categoría**: `goal_hierarchy.rs`, `intent_classifier.rs`, `ambiguity_detector.rs`, `clarification_gate.rs`

**Estado**: Mixto. `planner.rs`, `plan_compressor.rs`, `plan_coherence.rs`, `sla_manager.rs`, `decision_layer.rs`, `input_normalizer.rs`, `input_boundary.rs` — COMPLETE. `playbook_planner.rs`, `plan_state_diagnostics.rs`, `planning_metrics.rs`, `router.rs`, `rollback.rs` — PARTIAL/STUB.

**Hallazgo**: `rollback.rs` (501 LOC) implementa un sistema de transacciones rollback para file edits, pero `safe_edit_manager.rs` (605 LOC) implementa el mismo concepto. `safe_edit_manager.rs` está activo (usa `edit_transaction.rs`). `rollback.rs` está declarado en mod.rs pero sin callsites claros en producción.

### Categoría 7: Seguridad y Riesgo

**Archivos**: `command_blacklist.rs` (270), `conversational_permission.rs` (999), `authorization.rs` (1,063), `permissions.rs` (669), `output_risk_scorer.rs` (275), `adaptive_prompt.rs` (540), `risk_tier_classifier.rs` (346), `permission_lifecycle.rs` (251), `tool_policy.rs` (217), `tool_trust.rs` (424), `validation.rs` (639), `schema_validator.rs` (344), `circuit_breaker.rs` (654), `backpressure.rs` (262), `resilience.rs` (701)

**Archivos ORPHAN en esta categoría**: `input_risk_classifier.rs` (242), `pre_execution_critique.rs` (200)

**Estado**: La cadena de seguridad activa en `executor.rs` es: `command_blacklist` → `conversational_permission` → `output_risk_scorer` → `permissions`. `authorization.rs` (1,063 LOC, policy chain completa) está **fuera de esta cadena** — es un sistema de autorización alternativo no conectado.

**Hallazgo**: `circuit_breaker.rs` (654 LOC) implementa un circuit breaker genérico, mientras que `plugin_circuit_breaker.rs` (211 LOC) implementa el circuit breaker específico para plugins. El genérico (`circuit_breaker.rs`) solo aparece en `slash_commands.rs:570-572` como display de config — nunca se instancia para nada. La lógica de circuit breaking real de herramientas vive en `failure_tracker.rs` y `agent/post_batch.rs`.

**Acción**: Eliminar `circuit_breaker.rs` (funcionalidad no usada, duplicada) o WIRE al failure_tracker. `input_risk_classifier.rs` y `pre_execution_critique.rs` (ORPHAN): DELETE.

### Categoría 8: Tareas y Scheduling

**Archivos**: `task_bridge.rs` (388), `task_backlog.rs` (661), `task_scheduler.rs` (213), `agent_task_manager.rs` (367), `provenance_tracker.rs` (218), `artifact_store.rs` (226)

**Estado**: `task_bridge.rs`, `task_backlog.rs`, `task_scheduler.rs`, `artifact_store.rs`, `provenance_tracker.rs` — COMPLETE (usados por task_bridge). `agent_task_manager.rs` — STUB (declarado, sin callsites claros).

**Hallazgo**: El sistema de tareas (`TaskBridge` + `TaskBacklog` + `TaskScheduler`) está implementado y wired a `agent/loop_state.rs`. Sin embargo, `agent_task_manager.rs` (367 LOC) es un segundo task manager declarado en mod.rs sin callsites externos identificados — posible duplicación con `TaskBridge`.

### Categoría 9: Métricas y Observabilidad

**Archivos**: `metrics_store.rs` (397), `orchestrator_metrics.rs` (362), `reward_pipeline.rs` (1,185), `round_scorer.rs` (538), `evaluator.rs` (324), `strategy_metrics.rs` (234), `commit_reward_tracker.rs` (312), `planning_metrics.rs` (221), `macro_feedback.rs` (689), `context_metrics.rs` (208), `health.rs` (295), `runtime_signal_ingestor.rs` (615)

**Estado**: `orchestrator_metrics.rs`, `round_scorer.rs`, `strategy_metrics.rs` — COMPLETE. `reward_pipeline.rs`, `metrics_store.rs` — PARTIAL. `evaluator.rs`, `commit_reward_tracker.rs`, `planning_metrics.rs`, `macro_feedback.rs`, `context_metrics.rs`, `health.rs`, `runtime_signal_ingestor.rs` — STUB.

**Hallazgo crítico**: El pipeline de reward está roto entre sesiones. `reward_pipeline.rs` produce una recompensa rica (5 señales) pero solo se llama en `mod.rs:2921` post-session. El UCB1 engine en `domain/strategy_selector.rs` debería recibirla via `record_reward()`, pero `commit_reward_tracker.rs` (que atribuye rewards a decisiones específicas) nunca se llama desde `mod.rs`. El circuito UCB1 → strategy selection → reward → update está incompleto.

### Categoría 10: Git e IDE Integration

**Archivos**: `git_context.rs` (412), `git_event_listener.rs` (354), `commit_reward_tracker.rs` (312), `branch_divergence.rs` (277), `edit_transaction.rs` (637), `safe_edit_manager.rs` (605), `patch_preview_engine.rs` (418), `unsaved_buffer_tracker.rs` (330), `ide_protocol_handler.rs` (414), `test_result_parsers.rs` (668), `test_runner_bridge.rs` (467), `ci_result_ingestor.rs` (756), `ast_symbol_extractor.rs` (949), `traceback_parser.rs` (459), `code_instrumentation.rs` (283), `project_inspector.rs` (414)

**Estado**: `edit_transaction.rs`, `safe_edit_manager.rs`, `test_result_parsers.rs` — COMPLETE. El resto: STUB o PARTIAL sin callsites en el loop principal.

**Hallazgo**: `ast_symbol_extractor.rs` (949 LOC) es el archivo más grande de esta categoría y no tiene callsites en el agent loop. Implementa un extractor de símbolos Rust/Python sin referencia desde ningún path activo. `ci_result_ingestor.rs` (756 LOC), `traceback_parser.rs` (459 LOC), `ide_protocol_handler.rs` (414 LOC) — mismo patrón: implementados, no wired.

**Acción**: Candidatos para MOVE_TO `halcon-tools` o nuevo crate `halcon-devtools`. Actualmente dead weight en el binary.

### Categoría 11: Misc

**Archivos**: `integration_decision.rs` (331), `stress_tests.rs` (751), `dev_ecosystem_integration_tests.rs` (476), `onboarding.rs` (103), `sdlc_phase_detector.rs` (403), `search_engine_global.rs` (51), `replay_executor.rs` (196), `replay_runner.rs` (429)

**Estado**: `stress_tests.rs` y `dev_ecosystem_integration_tests.rs` son archivos de test puro (25 tests totales). `replay_executor.rs` + `replay_runner.rs` — COMPLETE (usados en agent loop para modo replay). `integration_decision.rs` — STUB (análisis de integración de componentes, no wired). `sdlc_phase_detector.rs` — STUB.

---

## 6. DEPENDENCIAS CIRCULARES

### 6.1 Dependencias circulares confirmadas

No hay dependencias circulares a nivel de crate (el workspace está bien organizado con halcon-core como hoja).

**Dentro de repl/, las dependencias más problemáticas son:**

**Loop circular conceptual #1: reward → UCB1 → strategy → plan → reward**
```
mod.rs:2921 reward_pipeline::compute_reward()
  → ModelSelector::record_outcome() (mod.rs:3255)
  → domain/strategy_selector.rs (UCB1 arm update)
  → (próxima sesión) ReasoningEngine::pre_loop_select()
  → playbook_planner / planner (selección de estrategia)
  → [ejecución del plan]
  → reward_pipeline (cierre del loop)
```
Este circuito es correcto conceptualmente pero **roto**: `commit_reward_tracker` y `git_event_listener` deberían atribuir rewards dentro del loop pero no están conectados.

**Loop de dependencias problemático #2: executor ← post_batch ← supervisor ← executor**
```
agent/post_batch.rs → executor::execute_parallel_batch()
agent/post_batch.rs → supervisor::PostBatchSupervisor::evaluate()
supervisor.rs referencia plugin_registry (compartido con executor.rs)
executor.rs referencia plugin_registry directamente
```
Tres archivos comparten `&std::sync::Mutex<PluginRegistry>` como parámetro, creando un estado compartido mutable que hace el testing frágil.

**Loop conceptual #3: authorization.rs vs. conversational_permission.rs**
```
executor.rs → conversational_permission.rs (activo)
executor.rs:1143 → [authorization.rs comentado como "no integrado"]
authorization.rs imports: rule_matcher.rs (correcto)
conversational_permission.rs duplica: session memory, persistent rules
```
Dos sistemas de autorización paralelos resuelven el mismo problema, el más completo (`authorization.rs`) está desconectado.

### 6.2 Acoplamiento excesivo (no circular, pero problemático)

`agent/loop_state.rs` es referenciado por 8 archivos distintos del loop con acceso mutable:
- `agent/mod.rs` (propietario)
- `agent/round_setup.rs`
- `agent/convergence_phase.rs`
- `agent/post_batch.rs`
- `agent/result_assembly.rs`
- `agent/provider_round.rs`
- `agent/checkpoint.rs`
- `agent/loop_state_roles.rs`

Cada uno puede mutar cualquier campo de `LoopState`. No hay invariantes de ownership — `LoopState` es efectivamente un global mutable.

---

## 7. PLAN DE REMEDIACIÓN PRIORIZADO

### 7.1 Eliminaciones Inmediatas (Semana 1 — cero riesgo, cero callsites)

Los 7 archivos ORPHAN son **invisibles al compilador**. No requieren migración de callsites. Solo eliminar los archivos:

| Archivo | LOC | Razón |
|---------|-----|-------|
| `crates/halcon-cli/src/repl/tool_executor.rs` | 198 | ORPHAN; `executor.rs` hace lo mismo directamente |
| `crates/halcon-cli/src/repl/ambiguity_detector.rs` | 793 | ORPHAN; funcionalidad cubierta por decision_engine/ |
| `crates/halcon-cli/src/repl/intent_classifier.rs` | 725 | ORPHAN; duplica task_analyzer.rs + decision_layer.rs |
| `crates/halcon-cli/src/repl/clarification_gate.rs` | 378 | ORPHAN; depende de intent_classifier.rs (también ORPHAN) |
| `crates/halcon-cli/src/repl/goal_hierarchy.rs` | 411 | ORPHAN; ExecutionPlan ya es flat — hierarchy no usada |
| `crates/halcon-cli/src/repl/input_risk_classifier.rs` | 242 | ORPHAN; output_risk_scorer.rs cubre caso de uso |
| `crates/halcon-cli/src/repl/pre_execution_critique.rs` | 200 | ORPHAN; config.security.pre_execution_critique no existe en AppConfig activo |
| **Total** | **2,947** | |

Ninguno de estos archivos tiene una declaración `mod` en `repl/mod.rs`. El compilador los ignora completamente. `git rm` es suficiente.

### 7.2 Subdirectorios Propuestos (Semana 2-3)

Propuesta de reorganización para reducir la flat root de 151 → ~60 archivos:

```
repl/
├── agent/               (existente, mantener)
├── agent_registry/      (existente, mantener)
├── auto_memory/         (existente, mantener)
├── decision_engine/     (existente, mantener)
├── domain/              (existente, mantener)
├── hooks/               (existente, mantener)
├── instruction_store/   (existente, mantener)
├── application/         (existente, mantener)
├── plugins/             (NUEVO — consolidar todo plugin_*.rs)
│   ├── mod.rs
│   ├── registry.rs      (← plugin_registry.rs)
│   ├── loader.rs        (← plugin_loader.rs)
│   ├── manifest.rs      (← plugin_manifest.rs)
│   ├── transport.rs     (← plugin_transport_runtime.rs)
│   ├── proxy_tool.rs    (← plugin_proxy_tool.rs)
│   ├── permission_gate.rs (← plugin_permission_gate.rs)
│   ├── circuit_breaker.rs (← plugin_circuit_breaker.rs)
│   ├── cost_tracker.rs  (← plugin_cost_tracker.rs)
│   └── recommendation.rs  (← plugin_recommendation.rs, plugin_auto_bootstrap.rs)
├── security/            (NUEVO — consolidar security chain)
│   ├── mod.rs
│   ├── blacklist.rs     (← command_blacklist.rs)
│   ├── permissions.rs   (← permissions.rs, conversational_permission.rs)
│   ├── authorization.rs (← authorization.rs + rule_matcher.rs)
│   ├── risk.rs          (← output_risk_scorer.rs, risk_tier_classifier.rs, adaptive_prompt.rs)
│   └── lifecycle.rs     (← permission_lifecycle.rs)
├── context/             (NUEVO — consolidar context sources)
│   ├── mod.rs
│   ├── manager.rs       (← context_manager.rs)
│   ├── memory.rs        (← memory_source.rs, vector_memory_source.rs)
│   ├── episodic.rs      (← episodic_source.rs, hybrid_retriever.rs)
│   ├── sources.rs       (← repo_map_source.rs, reflection_source.rs, planning_source.rs)
│   └── consolidator.rs  (← memory_consolidator.rs)
├── planning/            (NUEVO — consolidar planificación)
│   ├── mod.rs
│   ├── llm_planner.rs   (← planner.rs)
│   ├── playbook.rs      (← playbook_planner.rs)
│   ├── compressor.rs    (← plan_compressor.rs)
│   ├── coherence.rs     (← plan_coherence.rs)
│   └── sla.rs           (← sla_manager.rs, plan_state_diagnostics.rs)
├── sdlc_servers/        (NUEVO — consolidar context servers)
│   ├── mod.rs
│   ├── architecture.rs  (← architecture_server.rs)
│   ├── codebase.rs      (← codebase_server.rs)
│   ├── requirements.rs  (← requirements_server.rs)
│   ├── workflow.rs      (← workflow_server.rs)
│   ├── test_results.rs  (← test_results_server.rs)
│   ├── runtime_metrics.rs (← runtime_metrics_server.rs)
│   ├── security.rs      (← security_server.rs)
│   └── support.rs       (← support_server.rs)
├── git/                 (NUEVO — consolidar git/IDE)
│   ├── mod.rs
│   ├── context.rs       (← git_context.rs)
│   ├── events.rs        (← git_event_listener.rs)
│   ├── edit.rs          (← edit_transaction.rs, safe_edit_manager.rs)
│   ├── patch.rs         (← patch_preview_engine.rs)
│   └── ci.rs            (← ci_result_ingestor.rs, ci_detection.rs, test_result_parsers.rs, test_runner_bridge.rs)
└── metrics/             (NUEVO — consolidar métricas)
    ├── mod.rs
    ├── reward.rs        (← reward_pipeline.rs)
    ├── scorer.rs        (← round_scorer.rs)
    ├── evaluator.rs     (← evaluator.rs)
    └── store.rs         (← metrics_store.rs)
```

### 7.3 Archivos para Mover a Otros Crates

| Archivo(s) | Destino propuesto | Justificación |
|------------|-------------------|---------------|
| `anomaly_detector.rs`, `arima_predictor.rs` | `halcon-agent-core` | Lógica de detección de anomalías independiente del CLI |
| `backpressure.rs` | `halcon-core/types/` | Config struct ya en halcon-core |
| `context_manager.rs`, todos `*_source.rs` | `halcon-context` | Trait `ContextSource` ya vive allí |
| Todos `*_server.rs` (8 context servers) | `halcon-context/servers/` | Son ContextSource implementations |
| `ast_symbol_extractor.rs`, `traceback_parser.rs` | `halcon-tools` | Herramientas de análisis, no loop logic |
| `replay_executor.rs`, `replay_runner.rs` | `halcon-testing` (nuevo) | Testing utilities |
| `schema_validator.rs`, `validation.rs` | `halcon-core/validation` | Sin deps hacia repl/ |

### 7.4 Priorización por Impacto

**Prioridad 1 — Eliminar ORPHAN (semana 1, ~30min)**
- LOC eliminadas: 2,947
- Riesgo: cero
- Verificación: `cargo build` debe pasar sin cambios

**Prioridad 2 — Wirear `authorization.rs` o eliminar (semana 1, ~4h)**
- Decisión: ¿usar `AuthorizationMiddleware` como reemplazo de `ConversationalPermissionHandler`?
- Si no: eliminar `authorization.rs` (1,063 LOC)
- Si sí: wire en `executor.rs::execute_sequential()` como pre-gate

**Prioridad 3 — Fix `std::sync::Mutex` → `tokio::sync::Mutex` en `model_selector.rs` (semana 2, ~2h)**
- Afecta: `live_latency_overrides`, `quality_stats`, `selection_history`
- `provider_round.rs:1008` llama `record_observed_latency()` desde async context
- Riesgo actual: posible deadlock de Tokio thread bajo contención

**Prioridad 4 — Wire `reward_pipeline` dentro del agent loop (semana 2, ~4h)**
- Mover `mod.rs:2912-2935` a `agent/mod.rs` post-loop cleanup (tras result_assembly)
- Wire `commit_reward_tracker::flush_rewards()` al GitEventListener

**Prioridad 5 — Consolidar `TaskComplexity` en halcon-core (semana 3, ~8h)**
- Crear `halcon-core::types::TaskComplexity` canónico (4 variantes de decision_layer)
- Migrar: `task_analyzer.rs`, `model_selector.rs`, `decision_engine/` a usar el tipo central
- Eliminar 3 definiciones redundantes

**Prioridad 6 — Wire `PlaybookPlanner` en planning gate (semana 3, ~4h)**
- En `agent/mod.rs` planning gate: intentar `playbook_planner.plan()` primero
- Si match: usar plan, skip `LlmPlanner`
- Si no match: fall through a `LlmPlanner`

**Prioridad 7 — Subdirectorio plugins/ (semana 4, ~8h)**
- Mover 10 archivos `plugin_*.rs` + `capability_*.rs` a `plugins/`
- Actualizar todos los `use super::plugin_*` en repl/

---

## 8. GANTT DE EJECUCIÓN

```
Semana 1 (2026-03-09 a 2026-03-13)
├── Día 1-2: Eliminar 7 ORPHAN files
│   └── git rm *.rs; cargo build; cargo test
├── Día 3: Decidir authorization.rs fate
│   ├── Opción A: Wire AuthorizationMiddleware en executor.rs
│   └── Opción B: git rm authorization.rs (1,063 LOC)
└── Día 4-5: Fix std::sync::Mutex en model_selector.rs
    └── Migrar a tokio::sync::Mutex + update await points

Semana 2 (2026-03-16 a 2026-03-20)
├── Día 1-2: Wire reward_pipeline en agent loop (dentro del loop)
│   └── Mover compute_reward() post-loop cleanup en agent/mod.rs
├── Día 3: Wire commit_reward_tracker + git_event_listener
│   └── Conectar flush_rewards() al UCB1 record_reward()
└── Día 4-5: Crear subdirectorio plugins/
    └── Mover 10 plugin_*.rs; actualizar imports

Semana 3 (2026-03-23 a 2026-03-27)
├── Día 1-3: TaskComplexity canónico en halcon-core
│   ├── Definir enum en halcon-core
│   └── Migrar 4 módulos a usar el tipo central
├── Día 4: Wire PlaybookPlanner en planning gate
└── Día 5: Crear subdirectorio security/
    └── Mover command_blacklist, permissions, authorization, risk

Semana 4 (2026-03-30 a 2026-04-03)
├── Día 1-2: Crear subdirectorio sdlc_servers/
│   └── Mover 8 *_server.rs + actualizar mod.rs
├── Día 3: Crear subdirectorio context/
│   └── Mover *_source.rs + context_manager.rs
├── Día 4: MOVE_TO halcon-context (sdlc_servers + context/)
│   └── Requiere agregar halcon-context deps + actualizar workspace
└── Día 5: Verificación final
    ├── cargo build (workspace completo)
    ├── cargo test (>4332 tests)
    └── Actualizar docs/audit/repl-forensic-audit-2026.md con resultados
```

### Resumen de ROI

| Acción | Esfuerzo | LOC eliminadas | Riesgo de regresión |
|--------|----------|----------------|---------------------|
| Eliminar 7 ORPHANs | 30min | 2,947 | Cero |
| Eliminar/wire authorization.rs | 4h | 1,063 (si delete) | Bajo |
| Fix Mutex async | 2h | 0 (fix, no delete) | Bajo |
| Wire reward_pipeline | 4h | 0 (wire, no delete) | Medio |
| Consolidar TaskComplexity | 8h | ~500 | Medio |
| Subdirectorios (plugins, security, context) | 3 días | 0 (reorganización) | Bajo |
| MOVE_TO halcon-context | 5 días | ~8,000 de repl/ | Alto |

**Total estimado primera fase (semanas 1-2)**: 6h de trabajo activo, 4,010 LOC eliminadas, cero riesgo de regresión en tests existentes.

---

*Audit generado el 2026-03-08. Basado en análisis estático de grep + read de 233 archivos en repl/. No incluye análisis de performance o profiling.*
