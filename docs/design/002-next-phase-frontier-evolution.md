# Fase 2: Evolución Frontier — Recoverable Context + Multi-Tier Compaction

**Fecha:** 2026-04-03
**Base:** Fase 1 AAA completada + auditoría frontier + análisis XIYO + literatura 2025-2026

---

## 1. Executive Assessment

### Dónde está Halcon hoy

Halcon tiene un sistema de compaction semántica de Fase 1 que supera a XIYO en tres dimensiones: budget model formal (S+P+K+R≤B), degradación progresiva de 3 niveles, y circuit breaker con funcionalidad mantenida. Además tiene ProgressTracker (progreso basado en tool success, no tokens), LoopMetrics (observabilidad estructurada), y conditional reflection (signal-driven, no reflexiva).

### Por qué aún no supera completamente a XIYO

XIYO tiene cuatro capacidades que Halcon carece y que importan operacionalmente:

1. **Recoverable tool outputs.** XIYO persiste tool results grandes a disco y permite recovery via Read. Halcon los trunca inline — la información se pierde irrecuperablemente. Un `Read` de un archivo de 10K líneas consume ~40K tokens que después de truncation son irrecuperables.

2. **File re-read post-compact.** XIYO re-inyecta hasta 5 archivos (50K tokens budget) más recientes que el modelo leyó. Halcon solo lista los paths como metadata — el modelo no puede ver el contenido de archivos previamente leídos.

3. **Multi-tier eviction.** XIYO evicta tool results viejos (microcompact) antes de recurrir a summarization completa (autocompact). Halcon salta directamente a summarization semántica. El tool result truncation de Fase 1 es un paliativo, no un tier real.

4. **Session memory persistente.** XIYO mantiene un archivo markdown que acumula knowledge en background y sobrevive compaction. Halcon no tiene persistencia fuera del context window.

### Qué capacidades faltan para superar a XIYO

En orden de leverage:

| Prioridad | Capacidad | Leverage | Justificación |
|-----------|-----------|----------|---------------|
| P0 | Tool result persistence a disco | Altísimo | Elimina la mayor fuente de inflación de contexto. Permite recovery. XIYO y Anthropic docs lo recomiendan explícitamente. |
| P0 | File re-read post-compact | Alto | Restaura contexto operativo real (contenido de archivos), no solo metadata. |
| P1 | Tool result eviction tier | Alto | Evicta tool results antiguos ANTES de triggear summarization. Reduce frecuencia de compaction semántica. |
| P2 | Continuation-probe evaluation | Medio | Permite medir si la compaction funciona. Factory.ai: structured probes > ROUGE scores. |
| P3 | Session memory persistent | Medio | Acumula knowledge across compactions. Pero añade complejidad significativa (background agent). |

---

## 2. Halcon vs XIYO — Next Phase Gap Matrix

| Capability | Halcon actual | XIYO | Evidencia pública | Gap | Severidad | Prioridad |
|---|---|---|---|---|---|---|
| Tool result persistence | Truncation inline (8K, preview 2K). Info perdida. | Persistencia a disco, recovery via Read, 50K chars/tool. | Anthropic: "minimize tool overlap, return token-efficient info". Manus: "compression with recovery path = caching". | **Pérdida irrecuperable** de info | Crítica | P0 |
| File re-read post-compact | Metadata-only (paths listados) | Re-inyección de 5 archivos, 50K budget, 5K/archivo | Anthropic: CLAUDE.md re-read from disk post-compact. | **Pérdida de contenido** de archivos leídos | Alta | P0 |
| Tool result eviction | Tool result truncator (pre-compaction) | Microcompact (time-based clearing), cache_edits | Anthropic blog: "clear older tool outputs first". Active Context Compression (arXiv:2601.07190). | Halcon trunca; XIYO evicta selectivamente | Alta | P1 |
| Compaction tiers | 1 tier (semantic) con 3 niveles de degradación | 5 tiers (snip, microcompact, autocompact, collapse, reactive) | ACON (arXiv:2510.00615): "graduated compression preserves >95% accuracy". Multi-Layered Memory (arXiv:2603.29194). | Halcon salta a summarization directamente | Media | P1 |
| Evaluation framework | 7 métricas canary definidas, sin benchmark formal | BigQuery analytics + A/B testing | Factory.ai: 4 probe types (recall, artifact, continuation, decision). AMA-Bench (arXiv:2602.22769). | Sin evaluación formal de calidad | Media | P2 |
| Session memory | IntentAnchor (inmutable, in-memory) | Background session memory (persistent markdown, 40K threshold) | Memoria (arXiv:2512.12686). A-Mem (arXiv:2502.12110). | Sin persistencia across compactions | Media | P3 |
| Post-compact hooks | Ninguno | Trigger-aware hooks (manual/auto) | — | Bajo impacto operacional | Baja | P3 |
| Cache edits integration | Ninguno | Incremental cache_edits con API | Requiere API 4.5+ features | Bajo impacto directo | Baja | P3 |

---

## 3. Frontier Research Synthesis

### Hallazgos accionables para esta fase

**1. Two-phase eviction (Anthropic, Claude Code, Manus)**
- Fuente: Anthropic engineering blog, Manus blog, Claude Code docs
- Hallazgo: Evictar tool outputs primero (alta ratio tokens/recoverable), luego summarizar conversación (baja ratio, lossy). Claude Code hace esto internamente.
- Acción: Implementar tool result eviction como tier previo a semantic compaction.

**2. Recovery handles obligatorios (Manus pattern)**
- Fuente: Manus blog — 100:1 compression con recovery via URL/file path
- Hallazgo: "Compression without a recovery path is information destruction; compression with a recovery path is just caching."
- Acción: Todo output persistido debe tener un recovery handle (file path). El agente puede re-leer con Read tool.

**3. Three-tier storage (arXiv:2603.29194, arXiv:2601.07190)**
- Fuente: Multi-Layered Memory Architectures, Active Context Compression
- Hallazgo: Full-fidelity reciente → structured knowledge blocks (mid-range) → indexed references (old).
- Acción: Mapear a nuestros tiers: recent messages → semantic summary → persisted tool results + file paths.

**4. Continuation probes para evaluación (Factory.ai)**
- Fuente: Factory.ai evaluation framework
- Hallazgo: 4 tipos de probes: recall, artifact, continuation, decision. Structured summaries scored 3.70/5.0 vs genéricos 3.35/5.0.
- Acción: Implementar continuation probes como eval mínimo: "¿puede el agente seguir trabajando post-compaction?"

**5. Selective forgetting como first-class operation (AgeMem, SAGE)**
- Fuente: AgeMem (arXiv:2601.01885), SAGE (Ebbinghaus curve)
- Hallazgo: Forgetting no es un bug — es una operación de memoria. Los tool results viejos DEBEN evictarse proactivamente.
- Acción: Tool result eviction con policy de recency (más viejo → más agresivo).

---

## 4. Proposed Next Phase Architecture

### Vista general

```
simplified_loop
│
├── [cada turno, ANTES de compaction]
│   ├── ToolResultPersister.persist_large_results(&mut messages)  ← NUEVO P0
│   │   └── Persiste results > threshold a disco, reemplaza con recovery handle
│   └── ToolResultEvictor.evict_old_results(&mut messages)         ← NUEVO P1
│       └── Evicta results > age threshold, reemplaza con marker
│
├── [compaction trigger check]
│   └── CompactionBudgetCalculator.should_compact(est, budget)
│
├── [compaction]
│   └── TieredCompactor.compact(...)
│       ├── Nivel 1 (nominal): summary + protected context + file re-read
│       ├── Nivel 2 (degraded): extended keep + protected context + file re-read
│       └── Nivel 3 (emergency): intent + min keep
│
├── [POST-COMPACT: file re-read]                                   ← NUEVO P0
│   └── FileReReader.inject_recent_files(&mut messages, budget)
│       └── Re-lee archivos más recientes y los inyecta como context
│
└── [evaluation hooks]                                              ← NUEVO P2
    └── CompactionEvaluator.probe_continuation_quality(...)
```

### Componentes nuevos

#### ToolResultPersister (P0)

**Responsabilidad:** Persistir tool results que excedan un threshold a disco. Reemplazar en contexto con un recovery handle.

**Archivo:** `crates/halcon-cli/src/repl/context/tool_result_persister.rs`

**Behavior:**
- Se ejecuta cada turno, ANTES de estimación de tokens.
- Para cada `ContentBlock::ToolResult` con `estimate_tokens(content) > persistence_threshold`:
  1. Genera file path: `{session_dir}/tool-results/{tool_use_id}.txt`
  2. Escribe content a disco (best-effort, no panic si falla).
  3. Reemplaza content con: `[Tool output persisted to {path} ({original_tokens} tokens). Use Read tool to access full output.]\n{first 2000 tokens preview}`
- Preserva `tool_use_id` y `is_error` intactos.

**Diferencia con ToolResultTruncator:**
- Truncator pierde la información. Persister la externaliza con recovery handle.
- Truncator se mantiene como fallback si persistence falla.

**Threshold:** Configurable. Default: 10000 tokens (XIYO usa 50K chars ≈ 12.5K tokens).

**Recovery:** El agente puede invocar `Read` tool con el path para obtener el output completo. No requiere cambios en el loop — el modelo ya tiene acceso a Read.

**Failure domain:** Si la escritura a disco falla, cae a truncation inline (Truncator existente). Log de warning.

**Invariante:** `tool_use_id` nunca se modifica. `is_error` nunca se modifica. El recovery handle siempre incluye el path y el tamaño original.

#### FileReReader (P0)

**Responsabilidad:** Después de compaction, re-inyectar el contenido de archivos recientemente leídos para restaurar contexto operativo real.

**Archivo:** `crates/halcon-cli/src/repl/context/file_re_reader.rs`

**Behavior:**
- Se ejecuta inmediatamente DESPUÉS de compaction (dentro de TieredCompactor o como post-step).
- Mantiene un `ReadFileState: HashMap<String, Instant>` que trackea archivos leídos y cuándo.
- Post-compaction, selecciona los N archivos más recientes que no están ya en el keep window.
- Lee cada archivo de disco (best-effort) y lo inyecta como mensaje `Role::User` con marker: `[Post-compaction file restoration: {path}]\n{content}`
- Respeta budget: max `file_reread_budget` tokens total, max `file_reread_per_file` tokens por archivo.

**Tracking de archivos leídos:**
- El loop principal registra paths cuando el agente ejecuta Read/Glob/Grep tools exitosamente.
- El registro es un `Vec<(String, Instant)>` — path + timestamp.
- Solo archivos del filesystem local (no URLs).

**Budget:** Configurable. Defaults: 20000 tokens total, 4000 tokens por archivo, max 5 archivos (alineado con XIYO pero con budget más conservador).

**Failure domain:** Si la lectura de un archivo falla (borrado, permisos), se omite silenciosamente con log de debug. No afecta el resto del flow.

**Por qué no dentro del boundary message:** Los archivos re-leídos son contenido real, no metadata. Inyectarlos como mensajes separados permite al modelo distinguir entre "resumen de lo que pasó" y "contenido actual de archivos".

#### ToolResultEvictor (P1)

**Responsabilidad:** Evictar tool results antiguos del contexto ANTES de que triggeen compaction semántica. Es el tier de eviction pre-compaction.

**Archivo:** `crates/halcon-cli/src/repl/context/tool_result_evictor.rs`

**Behavior:**
- Se ejecuta cada turno, DESPUÉS de persistence pero ANTES de compaction trigger check.
- Para tool results más viejos que `eviction_age_turns` (default: 10 turnos), reemplaza content con marker: `[Tool result evicted — {age} turns old. Originally {tokens} tokens.]`
- No evicta el turno actual ni los últimos `eviction_keep_recent` resultados (default: 5).
- Respeta tool pair safety: solo evicta content, no borra el ToolResult block.

**Diferencia con Persister:**
- Persister: externaliza results GRANDES a disco con recovery handle.
- Evictor: elimina contenido de results VIEJOS que ya no son relevantes, sin recovery.
- Son complementarios: un result puede ser primero persistido (por tamaño) y luego evictado (por edad).

**Threshold de edad:** Configurable. Default: 10 turnos. XIYO usa time-based (60 min) para cache TTL; nosotros usamos turn-based porque es más predecible en un loop síncrono.

**Invariante:** `tool_use_id` y `is_error` nunca se modifican. El marker siempre indica edad y tamaño original.

#### CompactionEvaluator (P2)

**Responsabilidad:** Evaluar la calidad de la compaction con continuation probes.

**Archivo:** `crates/halcon-cli/src/repl/context/compaction_evaluator.rs`

**Behavior:**
- Se ejecuta opcionalmente post-compaction (gated por config).
- Define 3 tipos de probes (inspirados en Factory.ai):
  1. **Recall probe:** ¿El boundary message menciona los archivos modificados? (check automático)
  2. **Continuation probe:** ¿El boundary message contiene un "next step" explícito? (check automático)
  3. **Decision probe:** ¿El boundary message menciona decisiones clave? (check automático)
- Produce un `CompactionQualityScore` con 3 sub-scores (0.0–1.0) y un score agregado.
- El score se emite como tracing field `compaction.quality_score`.

**No es un evaluator LLM-based.** Es un verificador heurístico que busca señales en el texto del summary. Simple, rápido, sin costo de invocación.

**Criterio de calidad:**
- Score ≥ 0.6: summary aceptable.
- Score < 0.6: loguear warning "low quality compaction detected".
- Score < 0.3: loguear error "compaction quality critically low".

### Config nuevos campos

```
// Fase 2 — tool result persistence
pub tool_result_persistence_enabled: bool,       // default: false
pub tool_result_persistence_threshold: u32,      // default: 10000 tokens
pub tool_result_persistence_dir: Option<String>, // default: None (auto: .halcon/tool-results/)

// Fase 2 — file re-read
pub file_reread_enabled: bool,                   // default: false
pub file_reread_budget: u32,                     // default: 20000 tokens
pub file_reread_per_file: u32,                   // default: 4000 tokens
pub file_reread_max_files: u32,                  // default: 5

// Fase 2 — tool result eviction
pub tool_result_eviction_enabled: bool,          // default: false
pub tool_result_eviction_age_turns: u32,         // default: 10
pub tool_result_eviction_keep_recent: u32,       // default: 5

// Fase 2 — compaction evaluation
pub compaction_evaluation_enabled: bool,         // default: false
```

Todos con `#[serde(default)]` y defaults conservadores (off por default).

---

## 5. Why This Is Better Than XIYO

| Aspecto | Halcon Fase 2 | XIYO | Superioridad |
|---------|---------------|------|-------------|
| **Recovery handles** | Path explícito en el message, agente usa Read para recovery | Path en `<persisted-output>` XML | Halcon: más directo, sin wrapper XML. Recovery via herramienta estándar. |
| **Eviction policy** | Turn-based (predecible) + size-based (persistence) — dos dimensiones | Time-based (cache TTL, 60 min) — una dimensión | Halcon: eviction por dos ejes ortogonales. Más granular. |
| **Budget post-compact** | S+P+K+R+F≤B (F = file re-read budget incluido en ecuación) | Sin ecuación formal post-compact | Halcon: budget verificable formalmente incluyendo file re-read. |
| **Quality evaluation** | Heuristic probes (recall, continuation, decision) — automático, sin costo LLM | Sin evaluación formal de summary quality | Halcon: evaluación integrada, zero-cost. |
| **Degradation** | 3 niveles + eviction tier + persistence tier = 5 capas de protección | 5 tiers (snip, micro, auto, collapse, reactive) pero sin degradation progresiva formal | Halcon: degradation con monotonic preservation + eviction multi-dimensional. |
| **Observability** | LoopMetrics + ProgressTracker + CompactionEvaluator + tracing span 16 campos | BigQuery analytics (más rico pero más opaco) | Halcon: observabilidad inline, immediate, structured. |

**Trade-off honesto:** XIYO tiene cache_edits integration (API-level optimization) que Halcon no implementa. Esto reduce tokens en la API pero es una optimización de costo, no de calidad. Puede entrar en Fase 3 si la API de Halcon lo soporta.

---

## 6. Implementation Plan

### Etapa 1: Tool Result Persistence (P0, semana 1)

1. **ToolResultPersister** — función que persiste a disco + recovery handle.
2. Integrar en simplified_loop ANTES de ToolResultTruncator.
3. ToolResultTruncator se convierte en fallback de Persister.
4. Tests: persistence + recovery handle format + fallback a truncation.

**Dependencias:** Necesita resolver session directory path (usar working_dir o `.halcon/tool-results/`).

### Etapa 2: File Re-read Post-Compact (P0, semana 1-2)

1. **ReadFileState tracker** — registra archivos leídos con timestamp en el loop.
2. **FileReReader** — re-lee archivos post-compaction.
3. Integrar en TieredCompactor como post-step de compact().
4. Actualizar budget equation: S+P+K+R+F≤B.
5. Tests: re-read con budget, archivos no existentes, deduplicación.

**Dependencias:** Requiere tracking de archivos leídos en el loop (nuevo Vec en simplified_loop).

### Etapa 3: Tool Result Eviction (P1, semana 2)

1. **ToolResultEvictor** — evicta results viejos por edad.
2. Integrar entre Persister y compaction trigger.
3. Tests: eviction por edad, keep_recent, tool pair safety.

**Dependencias:** Depende de Persister (eviction puede referir al path persistido).

### Etapa 4: Compaction Evaluator (P2, semana 3)

1. **CompactionEvaluator** — probes heurísticos.
2. Integrar post-compaction en TieredCompactor.
3. Tracing field `compaction.quality_score`.
4. Tests: probes contra summaries de distinta calidad.

**Dependencias:** Solo TieredCompactor (ya existe).

### Paralelización

- Etapas 1 y 2 son parallelizables (Persister no depende de FileReReader).
- Etapa 3 depende de Etapa 1 conceptualmente pero puede implementarse en paralelo.
- Etapa 4 es independiente de todo.

---

## 7. Evaluation and Benchmark Plan

### Métricas

| Métrica | Definición | Fuente | Target |
|---------|------------|--------|--------|
| Tool result recovery rate | % de tool results persistidos que el agente re-lee exitosamente | Tracing | > 20% (cuando needed) |
| Context inflation reduction | % reducción en tokens estimados por turn (con persistence + eviction vs sin) | LoopMetrics | > 30% |
| Compaction frequency reduction | Compactions por sesión (con Fase 2 vs sin) | Tracing | Reducción > 25% |
| File re-read utilization | % de archivos re-leídos que el agente referencia en los 3 turnos siguientes | Tracing | > 50% |
| Compaction quality score | Score de CompactionEvaluator (recall + continuation + decision probes) | Tracing | > 0.6 avg |
| Post-compaction task completion | % sesiones que completan tarea tras ≥1 compaction (Fase 2 vs Fase 1) | Canary | Aumento > 5% |
| Duplicate action rate | Tool calls repetidos post-compaction (Fase 2 vs Fase 1) | Canary | Reducción > 10% |

### Criterios de éxito para rollout

1. Compaction frequency reducida ≥ 25% (eviction + persistence reducen inflación).
2. Post-compaction task completion no empeora vs Fase 1.
3. Zero regresiones en tool pair safety.
4. Quality score promedio > 0.6.

### Rollback signals

- Post-compaction task completion disminuye > 5%.
- Tool pair safety errors (400 de provider).
- Disk I/O errors > 1% de sesiones.
- File re-read causando context overflow (F demasiado grande).

---

## 8. Risk Analysis

| Riesgo | Prob. | Impacto | Mitigación |
|--------|-------|---------|------------|
| Disk I/O falla en persistence | Media | Bajo | Fallback a truncation inline. Log warning. |
| File re-read inyecta archivos obsoletos (editados post-read) | Media | Medio | Re-leer de disco (versión actual), no de cache. Worst case: modelo ve versión vieja. |
| Eviction demasiado agresiva pierde info útil | Baja | Medio | Default conservador (10 turnos). Configurable. |
| Recovery handle inútil (modelo no invoca Read) | Media | Bajo | Preview de 2K tokens preserva lo esencial. Recovery es oportunista, no obligatorio. |
| File re-read budget excede headroom | Baja | Medio | Budget incluido en ecuación S+P+K+R+F≤B. Post-verification existente lo controla. |
| Persistence directory no writable | Baja | Bajo | Check en init. Fallback a truncation. |
| Session directory colisión | Muy baja | Bajo | Usar tool_use_id como filename (único). |

---

## 9. Final Recommendation

### Construir ahora (Fase 2a — semanas 1-2)
- **ToolResultPersister** — máximo leverage, elimina la pérdida irrecuperable de info.
- **FileReReader** — segundo mayor leverage, restaura contexto operativo real.
- **ReadFileState tracker** — prerequisito de FileReReader.

### Construir después (Fase 2b — semana 3)
- **ToolResultEvictor** — reduce compaction frequency, complementa Persister.
- **CompactionEvaluator** — instrumentación de calidad, zero-cost.

### No construir todavía (Fase 3+)
- Session memory persistente (background agent — complejidad alta, leverage medio).
- Cache edits integration (requiere API-level support).
- Snip compaction (selective annotation — interesante pero no urgente).
- Post-compact hooks (bajo impacto operacional).

### Camino a superioridad sobre XIYO

Con Fase 2a + 2b, Halcon tendrá:
- Budget model formal (ya superior)
- Degradación progresiva (ya superior)
- Tool result persistence con recovery handles (paridad con XIYO)
- File re-read post-compact (paridad con XIYO)
- Tool result eviction turn-based (diferente approach, comparable)
- Compaction quality evaluation (superior — XIYO no tiene)
- ProgressTracker + LoopMetrics (ya superior)
- Conditional reflection (ya superior)

Las áreas donde XIYO seguiría adelante (cache_edits, session memory, 5-tier compaction) son optimizaciones de eficiencia o features avanzados, no gaps de correctness.

---

## 10. Final Implementation Prompt for Claude Code

```
Actúa como Senior Rust Engineer para Halcon CLI.

Implementa la Fase 2a de la evolución frontier: Tool Result Persistence
y File Re-read Post-Compact.

NO reabras la arquitectura de Fase 1.
NO cambies TieredCompactor salvo para integrar file re-read como post-step.
NO implementes session memory, cache edits, ni snip compaction.

FUENTES DE VERDAD
- docs/design/002-next-phase-frontier-evolution.md
- Implementación actual de Fase 1 (context/ modules)
- XIYO como referencia para patterns de persistence y re-read

PASO 1: ToolResultPersister
Archivo: crates/halcon-cli/src/repl/context/tool_result_persister.rs (nuevo)

Implementa:
  pub fn persist_large_tool_results(
      messages: &mut Vec<ChatMessage>,
      threshold_tokens: usize,
      preview_tokens: usize,
      session_dir: &Path,
  ) -> PersistenceResult

PersistenceResult: { persisted: u32, failed: u32, total_bytes_written: u64 }

Behavior:
- Para cada ToolResult con estimate_tokens(content) > threshold:
  - Generar path: session_dir/tool-results/{tool_use_id}.txt
  - Crear directorio si no existe (std::fs::create_dir_all)
  - Escribir content a disco (best-effort)
  - Reemplazar content con recovery handle + preview
  - Si escritura falla: loguear warning, dejar content intacto
    (ToolResultTruncator lo manejará después)
- Preservar tool_use_id e is_error SIEMPRE
- NO tocar los últimos 2 mensajes (turno actual)
- Retornar conteo de persistidos y fallidos

Recovery handle format:
  "[Tool output persisted ({tokens} tokens). Full output at: {path}
  Use Read tool to access complete output.]\n{preview}"

Tests obligatorios:
- persistence exitosa escribe archivo
- recovery handle tiene path correcto
- fallback cuando directorio no writable
- tool_use_id e is_error preservados
- últimos 2 mensajes no tocados
- PersistenceResult counters correctos

PASO 2: ReadFileState tracker
En simplified_loop.rs, añadir tracking de archivos leídos:

let mut read_file_state: Vec<(String, Instant)> = Vec::new();

Después de tool execution, para cada tool con name == "Read"
o name == "file_read":
  Si input tiene "file_path", registrar (path, Instant::now())

Pasar read_file_state a TieredCompactor.compact() como nuevo parámetro.

PASO 3: FileReReader
Archivo: crates/halcon-cli/src/repl/context/file_re_reader.rs (nuevo)

Implementa:
  pub fn inject_recent_files(
      messages: &mut Vec<ChatMessage>,
      read_state: &[(String, Instant)],
      budget_tokens: usize,
      per_file_tokens: usize,
      max_files: usize,
      existing_paths: &[String], // paths ya en keep window
  ) -> FileReReadResult

FileReReadResult: { files_injected: u32, tokens_injected: usize }

Behavior:
- Ordenar read_state por timestamp (más reciente primero)
- Filtrar paths que ya están en existing_paths
- Para cada archivo (hasta max_files):
  - Leer de disco (std::fs::read_to_string, best-effort)
  - Si excede per_file_tokens: truncar con marker
  - Si total excede budget_tokens: stop
  - Añadir como mensaje Role::User:
    "[Post-compaction file restoration: {path}]\n{content}"
- Si lectura falla: skip, log debug

Integración: Llamar en TieredCompactor después de aplicar compaction,
ANTES de calcular tokens_after.

Tests obligatorios:
- inyecta archivos más recientes primero
- respeta budget total y per-file
- skip archivos ya en keep window
- skip archivos no existentes
- FileReReadResult counters correctos

PASO 4: Config
Añadir campos a CompactionConfig con #[serde(default)]:
  tool_result_persistence_enabled: bool (default false)
  tool_result_persistence_threshold: u32 (default 10000)
  file_reread_enabled: bool (default false)
  file_reread_budget: u32 (default 20000)
  file_reread_per_file: u32 (default 4000)
  file_reread_max_files: u32 (default 5)

PASO 5: Integración en simplified_loop
Orden de ejecución cada turno:
  1. ToolResultPersister (si enabled)
  2. ToolResultTruncator (existente, ahora es fallback)
  3. Compaction trigger check
  4. Compaction (si triggered)
  5. File re-read (si enabled, post-compaction)

PASO 6: Budget equation update
Actualizar CompactionBudgetCalculator para incluir F:
  S + P + K + R + F ≤ B
  donde F = file_reread_budget

El trigger threshold se ajusta: B - (S_max + P_max + R + F_max)

PASO 7: Registrar módulos
context/mod.rs: añadir pub mod file_re_reader y pub mod tool_result_persister

PASO 8: Tests de integración
- Persistence + compaction + file re-read en secuencia
- Feature flags off → behavior actual preservado
- Budget equation con F incluido

REGLAS
- Cada paso compila antes del siguiente
- Preservar TODOS los tests existentes
- No cambiar FeedbackArbiter
- No cambiar ProgressTracker ni LoopMetrics
- Usar tracing para observabilidad
- Usar anyhow::Result para errores
- Mantener estilo del codebase
```
