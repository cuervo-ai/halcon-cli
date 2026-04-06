# Fase 2: Recoverable Context — Diseño Frontier

**Fecha:** 2026-04-03
**Estado:** Diseño aprobado para implementación
**Base:** Fase 1 AAA completada + auditoría XIYO + literatura frontier 2025-2026

---

## 1. Executive Assessment

### Dónde está Halcon hoy

Halcon tiene un sistema de compaction semántica que supera a XIYO en rigor formal (budget model S+P+K+R≤B), control de degradación (3 niveles vs binario), progress tracking (tool-success-based vs token-based), y circuit breaker con funcionalidad mantenida. La infraestructura de observabilidad (LoopMetrics, conditional reflection) no tiene equivalente directo en XIYO.

### Por qué todavía no supera completamente a XIYO

XIYO tiene cuatro capacidades que Halcon carece y que importan operacionalmente:

1. **Tool result persistence con recovery.** XIYO persiste outputs grandes a disco con recovery handles; Halcon los trunca inline — información destruida irrecuperablemente.
2. **File re-read post-compact.** XIYO re-inyecta hasta 5 archivos (50K tokens) más recientes post-compaction; Halcon solo lista paths como metadata.
3. **Multi-tier eviction.** XIYO tiene snip + microcompact como tiers pre-autocompact; Halcon salta directamente a semantic compaction.
4. **Session memory.** XIYO mantiene notas persistentes en markdown que sobreviven compaction; Halcon no tiene persistencia fuera del context window.

### Qué capacidades entran en esta fase

| Prioridad | Capacidad | Razón |
|-----------|-----------|-------|
| P0 | Tool result persistence a disco | Elimina la mayor fuente de pérdida irrecuperable de información. Máximo leverage. |
| P0 | File re-read post-compact | Restaura contexto operativo real (contenido de archivos), no solo metadata. |
| P1 | Tool result eviction por edad | Segundo tier de eviction pre-compaction. Reduce frecuencia de semantic compaction. |
| P2 | Compaction quality probes | Evaluación heurística zero-cost del summary. Observabilidad de calidad. |

**No entra:** Session memory (complejidad alta, background agent), cache edits (API-specific), snip compaction (annotation system complejo), post-compact hooks (bajo impacto).

---

## 2. Halcon vs XIYO vs Frontier Research Matrix

| Capability | Halcon actual | XIYO | Evidencia pública | Gap | Sev. | Pri. |
|---|---|---|---|---|---|---|
| Tool result persistence | Truncation inline 8K tokens. Lossy. | Persistencia a disco, recovery via Read, 50K chars/tool. Atomic writes. | Manus: "compression without recovery = destruction". Anthropic: 84% token reduction con context editing. | Pérdida irrecuperable | Crítica | P0 |
| File re-read post-compact | Metadata-only (paths listados en protected context) | Re-inyección de 5 archivos, 50K budget, 5K/archivo, sorted by recency | Anthropic: CLAUDE.md re-read from disk post-compact. Just-in-time loading. | Pérdida de contenido | Alta | P0 |
| Tool result eviction | ToolResultTruncator (size-based, pre-compaction) | Microcompact time-based (60 min cache TTL) + snip (low-cost pre-autocompact) | Active Context Compression (arXiv:2601.07190). AgeMem (arXiv:2601.01885): forgetting as first-class operation. | Sin eviction por edad | Alta | P1 |
| Compaction quality eval | 7 métricas canary definidas, sin evaluación del summary | BigQuery analytics, A/B testing | Factory.ai: continuation probes > ROUGE. AMA-Bench (arXiv:2602.22769). | Sin eval formal | Media | P2 |
| Budget model | S+P+K+R≤B formal, post-verify, auto-correct | Threshold heurístico (effectiveWindow - 13K buffer), sin post-verify | No hay equivalente público documentado | **Halcon superior** | — | Congelado |
| Degradation | 3 niveles: nominal/degraded/emergency | Binario: pass/fail. Circuit breaker desactiva autocompact | Zylos 2026: 65% failures por context drift | **Halcon superior** | — | Congelado |
| Progress tracking | ProgressTracker (tool-success-based), conditional reflection | Token-based diminishing returns | — | **Halcon superior** | — | Congelado |
| Circuit breaker | 3 failures → emergency (mantiene funcionalidad) | 3 failures → desactiva autocompact | — | **Halcon superior** | — | Congelado |

---

## 3. Frontier Research Synthesis

### Hallazgos accionables

**1. Recovery handles son obligatorios (Manus, Anthropic)**
- Fuente: Manus blog, Anthropic engineering blog
- "Compression without a recovery path is information destruction; compression with a recovery path is just caching."
- Implicación directa: ToolResultPersister debe producir file paths que el agente pueda re-leer con Read tool.

**2. Two-phase eviction (Claude Code interno)**
- Fuente: Claude Code docs, Anthropic blog
- Evictar tool outputs primero (alta ratio tokens/recoverable), luego summarizar conversación (baja ratio, lossy). Claude Code lo hace internamente.
- Implicación: ToolResultPersister + ToolResultEvictor como tiers pre-compaction.

**3. Just-in-time context loading (Anthropic engineering blog)**
- "Maintain lightweight identifiers and dynamically load data via tools."
- Implicación: Post-compact, re-inyectar archivos como contenido recuperado, no como metadata estática.

**4. Continuation probes para evaluación (Factory.ai)**
- 4 tipos: recall, artifact, continuation, decision. Structured summaries scored 3.70/5.0 vs genéricos 3.35/5.0.
- Implicación: CompactionQualityProbes con checks automáticos del boundary message.

**5. Selective forgetting como operación de primera clase (AgeMem, ICLR 2026)**
- MemoryAgentBench: 4 competencias: retrieval, learning, long-range understanding, selective forgetting.
- Implicación: ToolResultEvictor implementa selective forgetting turn-based como first-class operation.

---

## 4. Proposed Next-Phase Architecture

### Vista de componentes

```
simplified_loop (cada turno)
│
├── [PRE-COMPACTION PIPELINE]
│   ├── ToolResultPersister.persist(messages, session_dir)     ← P0 NUEVO
│   │   └── Outputs > threshold → disco + recovery handle
│   ├── ToolResultEvictor.evict(messages, turn_count)          ← P1 NUEVO
│   │   └── Results > age → evict content, keep IDs
│   └── ToolResultTruncator.truncate(messages)                 ← EXISTENTE (fallback)
│       └── Remaining large results → inline truncation
│
├── [COMPACTION TRIGGER + EXECUTION]
│   └── TieredCompactor.compact(messages, ..., read_state)     ← MODIFICADO
│       ├── Nominal: summary + protected context + file re-read
│       ├── Degraded: extended keep + protected context + file re-read
│       └── Emergency: intent + budget keep
│
├── [POST-COMPACTION]
│   └── FileReReader.inject(messages, read_state, budget)      ← P0 NUEVO
│       └── Re-lee archivos recientes de disco → inyecta como mensajes
│
└── [QUALITY EVALUATION]
    └── CompactionQualityProbes.evaluate(boundary_msg)         ← P2 NUEVO
        └── recall + continuation + decision probes → score
```

### Componentes

#### ToolResultPersister (P0)

**Archivo:** `crates/halcon-cli/src/repl/context/tool_result_persister.rs`

**Responsabilidad:** Externalizar tool results grandes a disco con recovery handle. La información se preserva fuera del context window y es recuperable por el agente.

**Contract:**
```
pub fn persist_large_results(
    messages: &mut Vec<ChatMessage>,
    threshold_tokens: usize,
    preview_tokens: usize,
    session_dir: &Path,
) -> PersistenceResult { persisted: u32, failed: u32, bytes_written: u64 }
```

**Behavior:**
- Itera mensajes excepto los últimos 2 (turno actual).
- Para cada `ToolResult` con `estimate_tokens(content) > threshold`:
  - Path: `{session_dir}/tool-results/{tool_use_id}.txt`
  - Crea directorio si no existe (`create_dir_all`, best-effort).
  - Escribe content completo a disco.
  - Reemplaza content con: `"[Output persisted ({N} tokens) at: {path}\nUse Read tool for full output.]\n{preview}"`
  - Si escritura falla: log warning, dejar content intacto → Truncator lo maneja después.
- Preserva `tool_use_id` e `is_error` siempre.

**Invariante:** Cada result persistido tiene un recovery handle con path absoluto. El agente puede invocar Read con ese path sin asistencia adicional.

**Failure domain:** Aislado. Fallo de I/O → fallback a Truncator. No afecta el loop.

**Diferencia con XIYO:** XIYO usa `<persisted-output>` XML tags. Halcon usa texto plano con recovery handle directo — más simple, igualmente funcional. XIYO persiste como JSON arrays; Halcon persiste como texto plano — suficiente para Fase 2.

#### FileReReader (P0)

**Archivo:** `crates/halcon-cli/src/repl/context/file_re_reader.rs`

**Responsabilidad:** Post-compaction, re-inyectar contenido de archivos recientemente leídos para restaurar contexto operativo real.

**Contract:**
```
pub fn inject_recent_files(
    messages: &mut Vec<ChatMessage>,
    read_state: &[(String, std::time::Instant)],
    budget_tokens: usize,
    per_file_tokens: usize,
    max_files: usize,
    preserved_paths: &HashSet<String>,
) -> ReReadResult { files_injected: u32, tokens_used: usize }
```

**Behavior:**
- Ordena `read_state` por timestamp (más reciente primero).
- Filtra paths que ya están en `preserved_paths` (keep window).
- Para cada archivo (hasta max_files):
  - Lee de disco (`std::fs::read_to_string`, best-effort).
  - Si excede `per_file_tokens`: trunca con marker.
  - Si total excede `budget_tokens`: stop.
  - Inyecta como: `ChatMessage { role: User, content: Text("[File restored post-compaction: {path}]\n{content}") }`
- Si lectura falla: skip, log debug.

**Integración:** Se ejecuta en TieredCompactor DESPUÉS de apply_compaction, ANTES de calcular tokens_after final. Se ejecuta en niveles nominal y degraded. NO en emergency.

**Tracking de archivos leídos:** El loop registra paths cuando el agente ejecuta Read/Glob/Grep exitosamente. Nuevo campo en simplified_loop: `read_file_state: Vec<(String, Instant)>`.

**Budget equation actualizada:**
```
S + P + K + R + F ≤ B
donde F = tokens usados por file re-read
```
El trigger threshold se ajusta: `B - (S_max + P_max + R + F_max)`

**Diferencia con XIYO:** XIYO usa `preCompactReadFileState` como cache de contenido leído; nosotros re-leemos de disco (versión actual del archivo). Esto es superior — el modelo ve la versión más reciente, no una copia stale.

#### ToolResultEvictor (P1)

**Archivo:** `crates/halcon-cli/src/repl/context/tool_result_evictor.rs`

**Responsabilidad:** Evictar contenido de tool results antiguos del contexto para reducir inflación pre-compaction.

**Contract:**
```
pub fn evict_old_results(
    messages: &mut Vec<ChatMessage>,
    current_turn: u32,
    max_age_turns: u32,
    keep_recent_results: usize,
) -> u32 // evicted count
```

**Behavior:**
- Itera mensajes excepto los últimos 2.
- Para cada `ToolResult`, estima su "edad" en turnos basándose en su posición relativa.
- Si edad > `max_age_turns` y no está en los `keep_recent_results` más recientes:
  - Reemplaza content con: `"[Evicted — {age} turns old, originally {tokens} tokens]"`
- Preserva `tool_use_id` e `is_error`.

**Orden de ejecución:** DESPUÉS de Persister, ANTES de Truncator. Persister captura results grandes; Evictor limpia results viejos; Truncator maneja lo que queda.

**Diferencia con XIYO:** XIYO usa time-based eviction (60 min cache TTL). Halcon usa turn-based (más predecible en un loop síncrono). Complementario: size (Persister) + age (Evictor) — dos ejes ortogonales de eviction.

#### CompactionQualityProbes (P2)

**Archivo:** `crates/halcon-cli/src/repl/context/compaction_quality.rs`

**Responsabilidad:** Evaluar heurísticamente la calidad del summary post-compaction sin costo LLM.

**Contract:**
```
pub fn evaluate_boundary(
    boundary_text: &str,
    intent_anchor: &IntentAnchor,
    files_modified: &[String],
) -> QualityScore { recall: f32, continuation: f32, decision: f32, aggregate: f32 }
```

**3 probes (inspirados en Factory.ai):**
1. **Recall:** ¿El boundary menciona archivos modificados? Score = archivos mencionados / archivos conocidos.
2. **Continuation:** ¿El boundary contiene "Next Step" o equivalent? Binary: 1.0 si presente, 0.0 si no.
3. **Decision:** ¿El boundary contiene "Decisions Made" o equivalent? Binary: 1.0 si presente, 0.0 si no.

**Aggregate:** Media de los 3 scores. Se emite como tracing field.

**Umbrales:**
- ≥ 0.6: summary aceptable.
- < 0.6: `tracing::warn!("low quality compaction")`
- < 0.3: `tracing::error!("critically low compaction quality")`

### Config nuevos campos

```
// P0: Tool result persistence
tool_result_persistence_enabled: bool       // default: false
tool_result_persistence_threshold: u32      // default: 10000

// P0: File re-read
file_reread_enabled: bool                   // default: false
file_reread_budget: u32                     // default: 20000
file_reread_per_file: u32                   // default: 4000
file_reread_max_files: u32                  // default: 5

// P1: Tool result eviction
tool_result_eviction_enabled: bool          // default: false
tool_result_eviction_age_turns: u32         // default: 10
tool_result_eviction_keep_recent: u32       // default: 5

// P2: Quality probes
compaction_quality_probes_enabled: bool     // default: false
```

Todos `#[serde(default)]`, todos off por default.

### Invariantes

1. **Recovery handle siempre presente.** Todo result persistido contiene un file path legible por el agente.
2. **tool_use_id e is_error nunca se modifican.** En Persister, Evictor y Truncator.
3. **Budget equation con F.** `S + P + K + R + F ≤ B` se verifica post-compaction incluyendo file re-read.
4. **Re-read de disco, no de cache.** FileReReader lee la versión actual del archivo, no una copia stale.
5. **Orden de ejecución: Persist → Evict → Truncate → Trigger → Compact → ReRead.** Cada tier es aditivo, no interfiere con los demás.
6. **Feature flags off = behavior Fase 1 exacto.** Cada feature está gated independientemente.

### Failure domains

| Componente | Fallo | Impacto | Fallback |
|---|---|---|---|
| ToolResultPersister | I/O error | Bajo | Content intacto → Truncator lo maneja |
| FileReReader | Archivo no existe | Bajo | Skip, log debug |
| FileReReader | Contenido demasiado grande | Bajo | Truncación per-file |
| ToolResultEvictor | Estimación de edad incorrecta | Bajo | Eviction conservadora (default 10 turnos) |
| CompactionQualityProbes | Probe falla | Ninguno | Score = 0.0, log warning |

---

## 5. Why This Architecture Beats XIYO

| Aspecto | Halcon Fase 2 | XIYO | Superioridad |
|---|---|---|---|
| Recovery handles | Path explícito en texto plano, recovery via Read estándar | XML `<persisted-output>` tags | Halcon: más directo, sin parsing XML. |
| File re-read source | Re-lee de disco (versión actual) | Re-inyecta desde cache (posiblemente stale) | **Halcon superior**: modelo ve versión actual, no copia vieja. |
| Budget con file re-read | S+P+K+R+F≤B formal, verificado | Sin budget formal para file attachments | **Halcon superior**: budget verificable. |
| Eviction policy | 2 ejes ortogonales: size (Persister) + age (Evictor) | 1 eje: time-based (60 min cache TTL) | **Halcon superior**: más granular. |
| Quality evaluation | 3 probes heurísticos automáticos, zero-cost | Sin evaluación de calidad | **Halcon superior**: observabilidad de calidad integrada. |
| Tier ordering | Persist → Evict → Truncate → Compact → ReRead (5 steps) | Snip → Microcompact → Autocompact → Collapse → Reactive (5 steps) | Comparable en profundidad. XIYO tiene API-level optimizations (cache_edits) que Halcon no. |
| Degradation post-compact | 3 niveles + file re-read en nominal y degraded | Sin degradación formal | **Halcon superior**. |

**Trade-off honesto:** XIYO tiene session memory (persistent markdown across compactions) y cache_edits (API-level efficiency). Halcon no. Session memory es Fase 3; cache_edits requiere soporte API que puede no estar disponible en todos los providers.

---

## 6. Evaluation and Benchmark Plan

### Métricas de Fase 2

| Métrica | Definición | Target | Rollback signal |
|---|---|---|---|
| Persistence rate | % tool results persistidos / tool results > threshold | > 90% | < 50% (I/O problems) |
| Recovery utilization | % results persistidos que el agente re-lee | Observar (no target fijo) | — |
| File re-read utilization | % archivos re-leídos que el agente referencia en 3 turnos | > 40% | — |
| Context inflation reduction | Tokens estimados/turno con Fase 2 vs Fase 1 | Reducción > 25% | Aumento vs baseline |
| Compaction frequency | Compactions/sesión con Fase 2 vs Fase 1 | Reducción > 20% | Aumento vs baseline |
| Quality score (probes) | Aggregate de recall + continuation + decision | > 0.6 avg | < 0.3 avg |
| Post-compaction task completion | % sesiones con task completion tras ≥1 compaction | ≥ Fase 1 baseline | Disminución > 5% |

### Canary strategy

- Activar features individualmente: persistence primero, luego re-read, luego eviction.
- 10% de sesiones durante 7 días por feature.
- Comparar contra control (Fase 1 pura).

### Rollback

- Cada feature tiene flag independiente → rollback individual.
- `tool_result_persistence_enabled = false` → Truncator como fallback.
- `file_reread_enabled = false` → metadata-only como Fase 1.

---

## 7. Implementation Priorities

### P0 — Semana 1-2: Persistence + Re-read

| Paso | Componente | Dependencia | Output |
|---|---|---|---|
| 1 | Config: 4 campos nuevos (persistence + re-read) | — | Compile + serde compat |
| 2 | ToolResultPersister | Config | 6 tests |
| 3 | ReadFileState tracker en simplified_loop | — | Vec<(String, Instant)> |
| 4 | FileReReader | Config | 5 tests |
| 5 | Budget equation update (F en la ecuación) | CompactionBudgetCalculator | 3 tests |
| 6 | Integración: Persister en loop + ReReader en TieredCompactor | Pasos 2-5 | Integration tests |
| 7 | Tracing: persistence + re-read fields | — | Span fields |

### P1 — Semana 2-3: Eviction

| Paso | Componente | Dependencia | Output |
|---|---|---|---|
| 8 | Config: 3 campos nuevos (eviction) | — | Compile |
| 9 | ToolResultEvictor | Config | 5 tests |
| 10 | Integración: Evictor entre Persister y Truncator | Paso 9 | Integration test |

### P2 — Semana 3: Quality probes

| Paso | Componente | Dependencia | Output |
|---|---|---|---|
| 11 | Config: 1 campo (probes) | — | Compile |
| 12 | CompactionQualityProbes | — | 4 tests |
| 13 | Integración: probes post-compaction | TieredCompactor | Tracing field |

---

## 8. Risks and Mitigations

| Riesgo | Prob. | Impacto | Mitigación |
|---|---|---|---|
| Disk I/O falla en persistence | Media | Bajo | Fallback a Truncator. Log warning. |
| File re-read inyecta archivo obsoleto (editado por el agente) | Media | Medio | Re-lee de disco (versión actual, no cache). Worst case: contenido reciente. |
| Eviction demasiado agresiva | Baja | Medio | Default conservador (10 turnos). Configurable. |
| Recovery handle inútil (modelo no invoca Read) | Media | Bajo | Preview de 2K tokens preserva lo esencial. |
| File re-read budget excede headroom | Baja | Medio | F incluido en budget equation. Post-verify existente controla. |
| Session directory no writable | Baja | Bajo | Check en init. Fallback a Truncator. |
| Eviction break tool pair safety | Muy baja | Alto | Evictor preserva tool_use_id y is_error. ToolResult block sigue existiendo. |

---

## 9. Decisions to Freeze

| # | Decisión | Valor | Categoría |
|---|---|---|---|
| 1 | Persistence format | Texto plano + path en recovery handle | Frozen |
| 2 | Re-read source | Disco (versión actual), no cache | Frozen |
| 3 | Re-read injection | Mensajes Role::User separados (no fusionado al boundary) | Frozen |
| 4 | Eviction policy | Turn-based (no time-based) | Frozen |
| 5 | Pipeline order | Persist → Evict → Truncate → Trigger → Compact → ReRead | Frozen |
| 6 | Budget equation | S+P+K+R+F≤B | Frozen |
| 7 | Quality probes | Heurísticos (recall, continuation, decision), no LLM-based | Frozen |
| 8 | Feature flags | Independientes, off por default | Frozen |
| 9 | Re-read en emergency | NO — emergency preserva solo intent | Frozen |
| 10 | Session memory | Fuera de Fase 2 — requiere background agent | Frozen |

---

## 10. Final Implementation Prompt for Agent B

```
Actúa como Senior Rust Engineer para Halcon CLI.

Implementa la Fase 2 de Compaction Semántica: Recoverable Context.
Sigue estrictamente el diseño aprobado en
docs/design/002-phase2-recoverable-context.md.

NO reabras la arquitectura de Fase 1.
NO cambies TieredCompactor salvo para integrar file re-read.
NO implementes session memory, cache edits, ni snip compaction.
NO cambies FeedbackArbiter, ProgressTracker ni LoopMetrics.

DECISIONES CONGELADAS
- Pipeline: Persist → Evict → Truncate → Trigger → Compact → ReRead
- Persistence format: texto plano + path recovery handle
- Re-read source: disco (versión actual)
- Re-read injection: mensajes User separados
- Eviction: turn-based
- Budget: S+P+K+R+F≤B
- Quality probes: heurísticos, no LLM
- Features: independientes, off por default
- Re-read: no en emergency

ORDEN DE IMPLEMENTACIÓN

PASO 1: Config extension
Archivo: crates/halcon-core/src/types/config.rs
Añadir 8 campos a CompactionConfig con #[serde(default)]:
  tool_result_persistence_enabled: bool (false)
  tool_result_persistence_threshold: u32 (10000)
  file_reread_enabled: bool (false)
  file_reread_budget: u32 (20000)
  file_reread_per_file: u32 (4000)
  file_reread_max_files: u32 (5)
  tool_result_eviction_enabled: bool (false)
  tool_result_eviction_age_turns: u32 (10)
  tool_result_eviction_keep_recent: u32 (5)
  compaction_quality_probes_enabled: bool (false)
Tests: backward compat con configs existentes.

PASO 2: ToolResultPersister
Archivo: crates/halcon-cli/src/repl/context/tool_result_persister.rs (nuevo)
pub fn persist_large_results(
    messages: &mut Vec<ChatMessage>,
    threshold_tokens: usize,
    preview_tokens: usize,
    session_dir: &Path,
) -> PersistenceResult

Struct PersistenceResult { persisted: u32, failed: u32, bytes_written: u64 }

Behavior:
- Skip últimos 2 mensajes
- Para ToolResult > threshold: escribir a disco, reemplazar con
  recovery handle + preview
- Preservar tool_use_id e is_error SIEMPRE
- Si escritura falla: log warning, content intacto
- Crear session_dir/tool-results/ si no existe

Recovery handle format:
  "[Output persisted ({tokens} tokens) at: {path}
  Use Read tool for full output.]
  {first preview_tokens tokens of preview}"

Tests: 6 (persistence exitosa, fallback, IDs preservados,
skip últimos 2, counters, directorio creado)

PASO 3: ReadFileState tracker
En simplified_loop.rs, añadir:
  let mut read_file_state: Vec<(String, std::time::Instant)> = Vec::new();

Después de tool execution, para Read/Glob/Grep exitosos:
  Extraer file_path del input, registrar (path, Instant::now())

PASO 4: FileReReader
Archivo: crates/halcon-cli/src/repl/context/file_re_reader.rs (nuevo)
pub fn inject_recent_files(
    messages: &mut Vec<ChatMessage>,
    read_state: &[(String, std::time::Instant)],
    budget_tokens: usize,
    per_file_tokens: usize,
    max_files: usize,
    preserved_paths: &HashSet<String>,
) -> ReReadResult

Struct ReReadResult { files_injected: u32, tokens_used: usize }

Behavior:
- Ordenar por timestamp (más reciente primero)
- Filtrar paths en preserved_paths
- Leer de disco (best-effort)
- Truncar si > per_file_tokens
- Stop si total > budget_tokens
- Inyectar como mensaje User:
  "[File restored post-compaction: {path}]\n{content}"
- Sync, no async (std::fs::read_to_string)

Tests: 5 (orden recency, budget total, per-file, skip preserved,
skip inexistente)

PASO 5: Budget equation update
Archivo: crates/halcon-cli/src/repl/context/compaction_budget.rs
Añadir F_max al CompactionBudget:
  pub file_reread_budget: usize
Actualizar compute(): trigger = B - (S_max + P_max + R + F_max)
Actualizar verify_post_compaction para incluir F.

Tests: 3 (DeepSeek con F, Claude con F, trigger con F)

PASO 6: ToolResultEvictor
Archivo: crates/halcon-cli/src/repl/context/tool_result_evictor.rs (nuevo)
pub fn evict_old_results(
    messages: &mut Vec<ChatMessage>,
    current_turn: u32,
    max_age_turns: u32,
    keep_recent_results: usize,
) -> u32

Behavior:
- Skip últimos 2 mensajes
- Estimar edad: position-based (turn tracking por
  conteo de pares assistant+user)
- Evictar si age > max_age_turns y no en keep_recent
- Preservar tool_use_id e is_error SIEMPRE
- Marker: "[Evicted — {age} turns old, {tokens} tokens]"

Tests: 5 (eviction por edad, keep_recent, IDs preservados,
skip recientes, counter)

PASO 7: CompactionQualityProbes
Archivo: crates/halcon-cli/src/repl/context/compaction_quality.rs (nuevo)
pub fn evaluate_boundary(
    boundary_text: &str,
    intent_anchor: &IntentAnchor,
    files_modified: &[String],
) -> QualityScore

Struct QualityScore { recall: f32, continuation: f32,
  decision: f32, aggregate: f32 }

Probes:
1. Recall: archivos mencionados en boundary / archivos conocidos
2. Continuation: contiene "Next Step" o similar → 1.0
3. Decision: contiene "Decisions" o similar → 1.0
Aggregate: media de los 3.

Tests: 4 (full quality, missing sections, no files, empty boundary)

PASO 8: Registrar módulos
context/mod.rs: añadir 4 nuevos pub mod

PASO 9: Integración en simplified_loop
Orden cada turno:
  1. ToolResultPersister (si enabled)
  2. ToolResultEvictor (si enabled)
  3. ToolResultTruncator (existente)
  4. Compaction trigger check
  5. Compaction (si triggered)

PASO 10: Integración en TieredCompactor
Después de apply compaction en nominal y degraded:
  Si file_reread_enabled: llamar FileReReader.inject_recent_files()
  Pasar read_state como parámetro nuevo de compact()
  Incluir tokens de re-read en tokens_after

Post-compaction: si quality_probes_enabled:
  Evaluar boundary message con CompactionQualityProbes
  Emitir compaction.quality_score en tracing

PASO 11: Tests de integración
- Persist + Compact + ReRead en secuencia
- Todos los features off = behavior Fase 1 exacto
- Budget con F incluido
- Pipeline completo: Persist → Evict → Truncate → Compact → ReRead

REGLAS
- Cada paso compila antes del siguiente
- Preservar TODOS los tests existentes (163 context, 208 agent, 10 stress)
- No cambiar FeedbackArbiter
- Usar tracing para observabilidad
- Usar anyhow::Result para errores
- Usar std::fs para I/O (no tokio::fs — las operaciones son small/fast)
- Mantener estilo del codebase existente
- Si ambigüedad: decisión conservadora + TODO(phase2)
```
