# Spec Técnica Ejecutable: Compaction Semántica — Fase 1

**Fecha:** 2026-04-03
**Base:** Design doc hardened + final readiness review
**Estado:** Spec para implementación directa

---

# 1. Implementation Scope

## Entra en esta implementación

| Componente | Descripción |
|------------|-------------|
| IntentAnchor | Struct inmutable, captura intent del primer mensaje User |
| ToolResultTruncator | Función que trunca tool results > threshold |
| CompactionBudgetCalculator | Lógica pura de budget: trigger, caps, verification |
| CompactionSummaryBuilder | Construye prompt de 9 secciones, formatea respuesta |
| ProtectedContextInjector | Formatea bloque de protected context para fusión |
| TieredCompactor | Orquestador async con 3 niveles de degradación |
| Integración simplified_loop | Reemplazo de compaction proactiva + extracción de reactiva |
| Config: semantic_compaction | Campo runtime en CompactionConfig |
| Tracing span de compaction | 14 campos por evento |

## No entra

- cancel_token wiring (Fase 1.5, doc separado)
- diminishing returns adjustment (Fase 1.5, doc separado)
- hook_runner wiring (Fase 2)
- FallbackProvider real (Fase 2)
- Tool result persistencia a disco (Fase 2)
- IntentAnchor dinámico (Fase 2)
- Multi-tier compaction: snip, microcompact (Fase 2)
- Semantic stagnation detection (Fase 2)
- Goal verification (Fase 2)

---

# 2. Component Contracts

## 2.1 IntentAnchor

**Archivo:** `crates/halcon-cli/src/repl/context/intent_anchor.rs` (nuevo)

**Struct:**
```
pub struct IntentAnchor {
    pub user_message: String,        // verbatim, primer Role::User
    pub task_summary: String,        // primeros 500 chars de user_message
    pub mentioned_files: Vec<String>,// paths extraídos del mensaje
    pub working_dir: String,         // working_dir del loop
    pub created_at: std::time::Instant,
}
```

**API:**
```
impl IntentAnchor {
    pub fn from_messages(messages: &[ChatMessage], working_dir: &str) -> Self
    pub fn format_for_boundary(&self) -> String
}
```

**Behavior de `from_messages`:** Busca el primer mensaje con `role == Role::User`. Si existe, extrae `user_message` del texto (via `as_text()` o primer bloque Text). Si no existe (edge case: solo system prompt), crea IntentAnchor con `user_message = "[no user message found]"` y `mentioned_files` vacío.

**Extracción de files:** Regex simple sobre el mensaje: paths que coincidan con patrones tipo `path/to/file.ext`, `/absolute/path`, `./relative`. No es perfecto — es heuristic. Suficiente para Fase 1.

**Ownership:** Owned. Se crea una vez, se pasa como `&IntentAnchor` a todos los consumidores.

**Sync/Async:** Sync. Sin I/O.

**Invariante:** Inmutable después de creación. Ningún método `&mut self`.

**Failure domain:** No falla. Inputs vacíos producen IntentAnchor con defaults vacíos.

## 2.2 ToolResultTruncator

**Archivo:** `crates/halcon-cli/src/repl/context/tool_result_truncator.rs` (nuevo)

**API:**
```
pub fn truncate_large_tool_results(
    messages: &mut Vec<ChatMessage>,
    threshold_tokens: usize,
    preview_tokens: usize,
) -> u32  // retorna número de results truncados
```

**Behavior:** Itera sobre todos los mensajes excepto los últimos 2 (turno actual). Para cada `ContentBlock::ToolResult` cuyo `estimate_tokens(content)` > `threshold_tokens`:
- Calcula preview: primeros `preview_tokens` tokens (estimados en chars × 4).
- Reemplaza `content` con: `"[Tool result truncated from {original_tokens} to {preview_tokens} tokens. Use the tool again for full output.]\n{preview}"`.
- Preserva `tool_use_id` y `is_error` intactos.

**Sync/Async:** Sync. Función pura.

**Invariante:** `tool_use_id` nunca se modifica. `is_error` nunca se modifica.

**Failure domain:** No falla. Si un ContentBlock no es ToolResult, se ignora. Si messages tiene ≤ 2 mensajes, noop.

**Retorno:** Contador de results truncados, para tracing.

## 2.3 CompactionBudgetCalculator

**Archivo:** `crates/halcon-cli/src/repl/context/compaction_budget.rs` (nuevo)

**Structs:**
```
pub struct CompactionBudget {
    pub pipeline_budget: usize,      // B
    pub trigger_threshold: usize,    // B - C_reserve
    pub max_summary_tokens: usize,   // S_max
    pub max_protected_tokens: usize, // P_max
    pub keep_count: usize,           // mensajes nominales
    pub extended_keep_count: usize,  // mensajes para nivel degraded
    pub reserve: usize,              // R = max_output_tokens
}

pub enum PostCompactionCheck {
    Ok,
    SummaryTruncationNeeded { target_tokens: usize },
    KeepReductionNeeded { target_keep: usize },
}
```

**API:**
```
impl CompactionBudgetCalculator {
    pub fn compute(
        pipeline_budget: usize,
        max_output_tokens: u32,
        config: &CompactionConfig,
    ) -> CompactionBudget

    pub fn should_compact(
        estimated_tokens: usize,
        budget: &CompactionBudget,
    ) -> bool

    pub fn verify_post_compaction(
        tokens_after: usize,
        budget: &CompactionBudget,
    ) -> PostCompactionCheck

    pub fn utility_ratio(
        tokens_before: usize,
        tokens_keep: usize,
        tokens_summary: usize,
        tokens_protected: usize,
    ) -> f64
}
```

**Derivación de `pipeline_budget`:** En el loop, `pipeline_budget = (ctx_budget as f64 * config.utilization_factor) as usize`. Si `ctx_budget == 0` (provider sin info), usar `config.max_context_tokens` como fallback.

**Derivación de `keep_count`:** Usa `ContextCompactor::adaptive_keep_recent(pipeline_budget as u32)` existente.

**Derivación de `extended_keep_count`:** `initial_keep + (max_summary_tokens / avg_msg_tokens)` donde `avg_msg_tokens` se estima de los últimos 10 mensajes. Cap: `min(extended, messages.len() * 2 / 3)`. Floor: `keep_count`.

**Sync/Async:** Sync. Lógica pura aritmética.

**Failure domain:** No falla. Valores inválidos (pipeline_budget = 0) producen budget con trigger_threshold = 0 (siempre compact).

## 2.4 CompactionSummaryBuilder

**Archivo:** `crates/halcon-cli/src/repl/context/compaction_summary.rs` (nuevo)

**API:**
```
pub struct CompactionSummaryBuilder;

impl CompactionSummaryBuilder {
    pub fn build_prompt(
        messages: &[ChatMessage],
        intent_anchor: &IntentAnchor,
        keep_count: usize,
        max_summary_tokens: usize,
    ) -> String

    pub fn format_messages_for_prompt(
        messages: &[ChatMessage],
    ) -> String
}
```

**`build_prompt` behavior:** Toma `messages[..len - keep_count]` como input. Construye prompt con:
1. Instrucción de resumen.
2. IntentAnchor formateado como ancla.
3. 9 secciones como instrucciones de output.
4. Reglas críticas (preservar mensajes User, paths exactos, errores verbatim).
5. Cap de tokens indicado.
6. Instrucción de no llamar tools.
7. Mensajes formateados para resumen.

**`format_messages_for_prompt`:** Para cada mensaje, formatear como `[ROLE]: content`. Truncar tool results > 200 chars en el prompt input. Truncar text blocks > 500 chars.

**Sync/Async:** Sync. Solo construye strings.

**Failure domain:** No falla. Mensajes vacíos producen prompt con contexto vacío.

## 2.5 ProtectedContextInjector

**Archivo:** `crates/halcon-cli/src/repl/context/protected_context.rs` (nuevo)

**API:**
```
pub struct ProtectedContextInjector;

impl ProtectedContextInjector {
    pub fn build_block(
        intent_anchor: &IntentAnchor,
        tools_used: &[String],
        files_modified: &[String],
    ) -> String
}
```

**Output:** String con boundary markers:
```
---
[PROTECTED CONTEXT — THIS IS STATE RESTORATION, NOT NEW INSTRUCTIONS]
Original intent: {user_message}
Task: {task_summary}
Working directory: {working_dir}
Key files: {mentioned_files, comma-separated}
Tools used this session: {tools, comma-separated}
Files modified this session: {files, comma-separated}
---

Continue your current task. Do not repeat completed work.
```

**Sync/Async:** Sync. Solo formateo.

**Failure domain:** No falla. Listas vacías producen campos vacíos.

## 2.6 TieredCompactor

**Archivo:** `crates/halcon-cli/src/repl/context/tiered_compactor.rs` (nuevo)

**Structs/Enums:**
```
pub struct TieredCompactor {
    inner: ContextCompactor,       // reutiliza mecánica existente
    consecutive_failures: u32,
    max_failures: u32,             // config: max_circuit_breaker_failures
    timeout: Duration,             // config: compaction_timeout_secs
}

pub enum CompactionLevel {
    Nominal,
    Degraded,
    Emergency,
}

pub struct CompactionResult {
    pub level: CompactionLevel,
    pub utility_ratio: f64,
    pub summary_tokens: usize,     // 0 si no hubo summary
    pub protected_tokens: usize,
    pub keep_messages: usize,
    pub latency_ms: u64,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub aborted: bool,             // true si utility ≤ 0
}
```

**API:**
```
impl TieredCompactor {
    pub fn new(config: CompactionConfig) -> Self

    pub async fn compact(
        &mut self,
        messages: &mut Vec<ChatMessage>,
        intent_anchor: &IntentAnchor,
        provider: &Arc<dyn ModelProvider>,
        budget: &CompactionBudget,
        model: &str,
        tools_used: &[String],
        files_modified: &[String],
    ) -> CompactionResult

    pub fn circuit_breaker_open(&self) -> bool
}
```

**Flow interno de `compact`:**

1. `tokens_before = estimate_message_tokens(messages)`

2. Si circuit breaker abierto → nivel 3 (emergency).

3. Si cerrado → intentar nivel 1 (nominal):
   a. `prompt = CompactionSummaryBuilder::build_prompt(messages, intent_anchor, budget.keep_count, budget.max_summary_tokens)`
   b. Construir `ModelRequest` con model, prompt como User message, `max_tokens = budget.max_summary_tokens as u32`, `temperature = Some(0.0)`, `stream = false`, sin tools.
   c. `result = tokio::time::timeout(self.timeout, provider.invoke(&request)).await`
   d. Si éxito: extraer texto del stream, estimar tokens del summary.
   e. Si `summary_tokens > budget.max_summary_tokens`: truncar a `max_summary_tokens` chars estimados.
   f. Calcular `protected_block = ProtectedContextInjector::build_block(...)`.
   g. Calcular `utility = CompactionBudgetCalculator::utility_ratio(tokens_before, keep_tokens, summary_tokens, protected_tokens)`.
   h. Si `utility ≤ 0`: ABORT, retornar `CompactionResult { aborted: true, ... }`.
   i. Construir boundary message: `summary + "\n\n" + protected_block`.
   j. Aplicar via `self.inner.apply_compaction_with_budget(messages, &boundary_message, budget.pipeline_budget as u32)`.
   k. Reset `consecutive_failures = 0`.
   l. Verificar post-compaction con `CompactionBudgetCalculator::verify_post_compaction()`.
   m. Si truncation/reduction needed: aplicar.

4. Si fallo en paso 3 → `consecutive_failures += 1`, ir a nivel 2.

5. Nivel 2 (degraded):
   a. `protected_block = ProtectedContextInjector::build_block(...)`.
   b. `boundary_message = "[Summary unavailable — extended recent context preserved below]\n\n" + protected_block`.
   c. Usar `extended_keep_count` en vez de `keep_count`.
   d. Aplicar compaction manualmente: `messages.clear()`, push boundary, extend con últimos `extended_keep_count` mensajes (respetando `safe_keep_boundary_n`).

6. Nivel 3 (emergency):
   a. `boundary_message = "[Emergency compaction — only intent preserved]\n\n" + intent_anchor.format_for_boundary()`.
   b. Usar keep mínimo (4).
   c. Aplicar vía `self.inner.apply_compaction(messages, &boundary_message)`.

7. `tokens_after = estimate_message_tokens(messages)`.
8. Retornar `CompactionResult` con métricas.

**Extracción de tools_used y files_modified:** Se calculan en el loop a partir de `tools_executed: Vec<String>` (ya existe en simplified_loop.rs:141) y extrayendo paths de Edit/Write tool calls de mensajes. La extracción se hace ANTES de llamar a compact, no dentro de TieredCompactor.

**Ownership del ContextCompactor:** TieredCompactor POSEE un ContextCompactor (no referencia). Se construye con la misma CompactionConfig. Esto permite que TieredCompactor viva como campo mutable en el loop sin conflictos de borrow.

**Sync/Async:** `compact()` es async (invoca provider). Constructor es sync.

**Lifetime del circuit breaker:** `consecutive_failures` vive en el TieredCompactor. El TieredCompactor se crea al inicio del loop y se destruye al final. El circuit breaker se resetea cuando el loop termina (nueva sesión = nuevo TieredCompactor = 0 failures). **Frozen decision.**

**Failure domains:**
- LLM failure: aislado, degrada a nivel 2.
- Budget violation: detectada post-compaction, auto-corregida por truncation/reduction.
- Negative utility: detectada pre-apply, abort sin mutar mensajes.
- Stream parsing error: se trata como LLM failure, degrada a nivel 2.

## 2.7 Integración en simplified_loop

**Archivo:** `crates/halcon-cli/src/repl/agent/simplified_loop.rs` (modificación)

**Cambios en SimplifiedLoopConfig:**
```
// Añadir:
pub compaction_config: CompactionConfig,  // config completa para TieredCompactor
// compactor: Option<&'a ContextCompactor> se mantiene para backward compat
// cuando semantic_compaction = false
```

**Flujo revisado del loop principal:**

**Antes del loop (después de línea 151):**
```
let intent_anchor = IntentAnchor::from_messages(&messages, config.working_dir);
let mut tiered_compactor = if config.compaction_config.semantic_compaction {
    Some(TieredCompactor::new(config.compaction_config.clone()))
} else {
    None
};
let pipeline_budget = (ctx_budget as f64 * config.compaction_config.utilization_factor) as usize;
// files_modified tracker:
let mut files_modified: Vec<String> = Vec::new();
```

**Reemplazo del bloque proactivo (líneas 158-163):**
```
// ANTES:
if let Some(c) = config.compactor { ... c.apply_compaction(...) }

// DESPUÉS:
if let Some(ref mut tc) = tiered_compactor {
    let budget = CompactionBudgetCalculator::compute(pipeline_budget, ...);
    let est = ContextCompactor::estimate_message_tokens(&messages);
    if CompactionBudgetCalculator::should_compact(est, &budget) {
        // truncate tool results first
        let truncated = truncate_large_tool_results(&mut messages, ...);
        // re-estimate after truncation
        let est = ContextCompactor::estimate_message_tokens(&messages);
        if CompactionBudgetCalculator::should_compact(est, &budget) {
            let result = tc.compact(&mut messages, &intent_anchor, config.provider, &budget, &model, &tools_executed, &files_modified).await;
            compact_count += 1;
            // emit tracing span with result
        }
    }
} else if let Some(c) = config.compactor {
    // fallback: placeholder compaction when semantic_compaction = false
    let est = ContextCompactor::estimate_message_tokens(&messages);
    if est > (ctx_budget as f64 * COMPACTION_THRESHOLD) as usize {
        c.apply_compaction(&mut messages, "[Context compacted proactively]");
    }
}
```

**Extracción de compaction reactiva del match de `apply_recovery` (línea 253):**

El match en línea 247-266 se refactoriza:

```
match arbiter.decide(...) {
    TurnDecision::Complete { .. } => return Ok(...),
    TurnDecision::Recover(ref act) => {
        match act {
            RecoveryAction::Compact | RecoveryAction::ReactiveCompact => {
                // ASYNC: extraído de apply_recovery
                compact_count += 1;
                esc_count = 0;
                if let Some(ref mut tc) = tiered_compactor {
                    let budget = CompactionBudgetCalculator::compute(pipeline_budget, ...);
                    truncate_large_tool_results(&mut messages, ...);
                    let result = tc.compact(&mut messages, &intent_anchor, config.provider, &budget, &model, &tools_executed, &files_modified).await;
                    // emit tracing
                } else if let Some(c) = config.compactor {
                    c.apply_compaction(&mut messages, "[Context compacted]");
                } else {
                    // fallback sin compactor (drain existente)
                    let k = 8.min(messages.len());
                    if messages.len() > k { ... }
                }
            }
            _ => {
                // Resto de recovery actions: sync, delegado a apply_recovery
                apply_recovery(&mut messages, act, &mut max_tokens, &mut esc_count, &mut compact_count, &mut replan_count, config.compactor);
            }
        }
    }
    TurnDecision::Halt(reason) => { ... }
}
```

**Extracción de files_modified:** Después de tool execution (línea 226), extraer file paths de los tool results que correspondan a Edit/Write:

```
// después de messages.push(tool results):
for tu in &tus {
    if tu.name == "Edit" || tu.name == "Write" || tu.name == "file_write" {
        if let Some(path) = tu.input.get("file_path").and_then(|v| v.as_str()) {
            if !files_modified.contains(&path.to_string()) {
                files_modified.push(path.to_string());
            }
        }
    }
}
```

**Cambios en apply_recovery (línea 270):**
Los arms `RecoveryAction::Compact | RecoveryAction::ReactiveCompact` se ELIMINAN de apply_recovery. El match de la función queda:

```
fn apply_recovery(msgs, act, mt, esc, cmp, rpl, compactor) {
    match act {
        RecoveryAction::Compact | RecoveryAction::ReactiveCompact => {
            unreachable!("Handled in async loop body")
        }
        RecoveryAction::EscalateTokens => { ... }  // sin cambios
        RecoveryAction::FallbackProvider => { ... } // sin cambios
        RecoveryAction::StopHookBlocked => {}       // sin cambios
        RecoveryAction::Replan { .. } => { ... }    // sin cambios
        RecoveryAction::ReplanWithFeedback(..) => { ... } // sin cambios
    }
}
```

**Registrar nuevos módulos en context/mod.rs:**
```
pub mod compaction;            // existente
pub mod compaction_budget;     // NUEVO
pub mod compaction_summary;    // NUEVO
pub mod intent_anchor;         // NUEVO
pub mod protected_context;     // NUEVO
pub mod tiered_compactor;      // NUEVO
pub mod tool_result_truncator; // NUEVO
```

---

# 3. Config Schema

**Archivo:** `crates/halcon-core/src/types/config.rs`

Campos a añadir a `CompactionConfig`:

| Campo | Tipo | Default | Category | Descripción |
|-------|------|---------|----------|-------------|
| `semantic_compaction` | `bool` | `false` | **Operational default** | Runtime toggle. `false` = placeholder (behavior actual). `true` = TieredCompactor. |
| `utilization_factor` | `f32` | `0.80` | **Frozen** | Factor para calcular pipeline_budget = window × factor. |
| `summary_proportion` | `f32` | `0.05` | **Empirical hypothesis** | S_max = B × proportion, clamped. 0.05 = 1/20. |
| `summary_floor` | `u32` | `1000` | **Operational default** | Floor de S_max en tokens. |
| `summary_cap` | `u32` | `4000` | **Operational default** | Cap de S_max en tokens. |
| `protected_context_cap` | `u32` | `500` | **Operational default** | P_max en tokens. |
| `tool_result_truncation_threshold` | `u32` | `8000` | **Empirical hypothesis** | Tokens estimados para trigger truncation. |
| `tool_result_preview_size` | `u32` | `2000` | **Empirical hypothesis** | Tokens de preview tras truncation. |
| `compaction_timeout_secs` | `u64` | `30` | **Operational default** | Timeout de invocación LLM para summary. |
| `max_circuit_breaker_failures` | `u32` | `3` | **Frozen** | Failures consecutivas para abrir circuit breaker. |

**Backward compatibility:** Todos los campos nuevos tienen default via `#[serde(default)]`. Configs existentes deserializan sin problemas — los campos nuevos toman sus defaults. `semantic_compaction = false` por default significa que el sistema se comporta exactamente como hoy hasta que se active explícitamente.

**Default impl actualizado:**
```
impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            // Campos existentes
            enabled: true,
            threshold_fraction: 0.80,
            keep_recent: 4,
            max_context_tokens: 200_000,
            // Campos nuevos
            semantic_compaction: false,
            utilization_factor: 0.80,
            summary_proportion: 0.05,
            summary_floor: 1000,
            summary_cap: 4000,
            protected_context_cap: 500,
            tool_result_truncation_threshold: 8000,
            tool_result_preview_size: 2000,
            compaction_timeout_secs: 30,
            max_circuit_breaker_failures: 3,
        }
    }
}
```

---

# 4. Budget and Runtime Policy

## Ecuación de Budget

```
POST-COMPACTION INVARIANT:
  S + P + K + R ≤ B

WHERE:
  B = pipeline_budget = context_window × utilization_factor
  S = tokens(summary)           ≤ S_max
  P = tokens(protected_context) ≤ P_max
  K = tokens(keep_window)       = estimate de mensajes retenidos
  R = max_output_tokens          (default 4096)
```

**Category: Frozen decision.**

## Variables y derivaciones

| Variable | Derivación | Category |
|----------|-----------|----------|
| B | `provider.model_context_window(model) × config.utilization_factor`. Fallback: `config.max_context_tokens × config.utilization_factor`. | Frozen |
| S_max | `clamp(B × config.summary_proportion, config.summary_floor, config.summary_cap)` | Hypothesis (proportion), Default (floor/cap) |
| P_max | `config.protected_context_cap` | Default |
| R | `request.max_tokens.unwrap_or(4096)` | Frozen |
| C_reserve | `S_max + P_max + R` | Derived |
| Trigger | `B - C_reserve` | Frozen |

## Trigger policy

Compaction se activa cuando: `estimate_message_tokens(messages) ≥ trigger_threshold`.

Si trigger_threshold resulta ≤ 0 (C_reserve > B, teóricamente imposible para B ≥ 25K): loguear error, usar fallback de `B × 0.60` (compatibilidad con `needs_compaction_with_budget` existente). **Category: Frozen guardrail.**

## Summary cap policy

`S_max = clamp(B × 0.05, 1000, 4000)`. El LLM recibe `max_tokens = S_max`. Si el output excede S_max (posible con streaming), truncar el string al equivalente en chars de S_max tokens. **Category: Hypothesis (0.05). Default (1000, 4000).**

## Extended keep policy

```
extended_keep_count = keep_count + (S_max / avg_tokens_per_recent_message)
avg_tokens_per_recent_message = sum(tokens(last 10 messages)) / min(10, messages.len())
Cap: min(extended, messages.len() × 2 / 3)
Floor: keep_count
```

Si `avg_tokens_per_recent_message = 0`: usar `keep_count` (noop extension). **Category: Hypothesis (2/3 cap).**

## Utility ratio

```
utility = (T_freed - T_added) / T_freed
T_freed = tokens_before - keep_tokens
T_added = summary_tokens + protected_tokens
```

- `utility > 0`: compaction tiene beneficio neto. Aplicar.
- `utility ≤ 0`: compaction de beneficio negativo. ABORT. No mutar mensajes. Retornar `CompactionResult { aborted: true }`. **Category: Frozen.**
- `utility < 0.3 && utility > 0`: Warning en logs. Aplicar de todos modos. **Category: Default threshold.**

## Abort semantics si utility ≤ 0

No se compacta. Los mensajes quedan intactos. El trigger se cumplirá de nuevo en el siguiente turno. Eventualmente el API falla con prompt_too_long. El arbiter activa `RecoveryAction::Compact`. TieredCompactor intenta de nuevo y obtiene utility ≤ 0 de nuevo. `compact_count` se incrementa (ya se incrementa antes de llamar a compact). Tras `max_compact_attempts = 2`, el arbiter emite `HaltReason::UnrecoverableError`. **El abort no es infinito; está bounded por max_compact_attempts.** **Category: Frozen.**

## Verificación post-compaction

Después de aplicar compaction:
1. `T_after = estimate_message_tokens(messages)`.
2. Si `T_after + R > B`:
   - Buscar el boundary message (primer mensaje post-compaction).
   - Truncar su contenido hasta que `T_after + R ≤ B`.
   - Si truncar a vacío no alcanza → reducir keep_count y re-aplicar.
   - Loguear error en ambos casos.

**Category: Frozen guardrail.**

## Circuit breaker policy

| Estado | Condición | Acción |
|--------|-----------|--------|
| Closed | `consecutive_failures < max_failures` | Intentar nivel 1 |
| Open | `consecutive_failures ≥ max_failures` | Ir directo a nivel 3 |
| Increment | LLM timeout, error, o stream parsing error | `consecutive_failures += 1` |
| Reset (success) | Summary generado exitosamente | `consecutive_failures = 0` |
| Reset (session) | Loop termina (TieredCompactor se destruye) | Implicit: nuevo TieredCompactor = 0 |

**Category: Frozen (3 failures). Session scope frozen.**

## Fallback entre niveles

```
Nominal (1) → si LLM falla → Degraded (2)
Degraded (2) se aplica siempre que circuit breaker esté cerrado pero LLM falle
Emergency (3) se aplica cuando circuit breaker está abierto
```

No hay "Degraded → Emergency" dentro de una misma compaction. Si Degraded falla mecánicamente (e.g., extended keep calculation error), eso es un bug — no un nivel de degradación. **Category: Frozen.**

---

# 5. Integration Plan

## Puntos exactos de integración

### simplified_loop.rs

| Línea actual | Cambio | Descripción |
|-------------|--------|-------------|
| 32 | Import | Añadir imports de nuevos módulos |
| 43-56 | Struct | Añadir `compaction_config: CompactionConfig` a SimplifiedLoopConfig |
| 140-141 | Variable | Añadir `files_modified: Vec<String>` |
| 150-151 | Setup | Crear IntentAnchor, TieredCompactor condicional, pipeline_budget |
| 158-163 | Reemplazo | Bloque proactivo: condicional semantic vs placeholder |
| 216-226 | Tracking | Después de tool execution, extraer files_modified |
| 247-266 | Refactoring | Extraer Compact/ReactiveCompact del match, hacer async |
| 270-288 | Simplificación | Eliminar arms Compact/ReactiveCompact de apply_recovery |

### compaction.rs

**Sin cambios funcionales.** Se reutilizan:
- `ContextCompactor::new(config)` — dentro de TieredCompactor
- `ContextCompactor::estimate_message_tokens()` — en múltiples sitios
- `ContextCompactor::apply_compaction_with_budget()` — desde TieredCompactor nivel 1
- `ContextCompactor::apply_compaction()` — desde TieredCompactor nivel 3
- `ContextCompactor::adaptive_keep_recent()` — desde CompactionBudgetCalculator
- `safe_keep_boundary_n()` — internamente via apply_compaction*

### config.rs (halcon-core)

Añadir campos a `CompactionConfig` con `#[serde(default)]`. Actualizar Default impl.

### context/mod.rs

Añadir 6 líneas de `pub mod`.

### dispatch.rs

Añadir `compaction_config` al SimplifiedLoopConfig que construye. Obtener del `AgentContext` o construir default.

### feedback_arbiter.rs

**Sin cambios.** `max_compact_attempts = 2` se mantiene. La semántica no cambia — `compact_count` se incrementa igual.

---

# 6. Runtime Assertions and Invariants

| # | Assertion | Severity | On Failure | Log |
|---|-----------|----------|------------|-----|
| A1 | `budget.trigger_threshold > 0` | Error | Usar fallback `B × 0.60` | `tracing::error!("trigger_threshold ≤ 0, C_reserve exceeds budget")` |
| A2 | `utility > 0` antes de aplicar compaction | Guard | Abort compaction, retornar `aborted: true` | `tracing::warn!(utility, "compaction aborted: negative utility")` |
| A3 | `tokens_after + reserve ≤ pipeline_budget` post-compaction | Guard | Truncar boundary message | `tracing::error!(tokens_after, budget, "post-compaction budget violation, truncating")` |
| A4 | `tool_use_id` preservado en truncation | Debug assert | Panic en debug, noop en release | `debug_assert_eq!(before.tool_use_id, after.tool_use_id)` |
| A5 | IntentAnchor presente en boundary message (levels 1,2,3) | Debug assert | Panic en debug | `debug_assert!(boundary_msg.contains("Original intent:"))` |
| A6 | `consecutive_failures` no excede `max_failures + 1` | Debug assert | — | `debug_assert!(self.consecutive_failures <= self.max_failures + 1)` |
| A7 | `extended_keep_count ≥ keep_count` | Invariant | Usar keep_count como floor | `tracing::warn!("extended_keep below initial keep, using floor")` |
| A8 | `keep ≥ messages.len()` cuando apply_compaction_keep noop | Info | Noop es correcto | `tracing::info!("compaction noop: all messages fit in keep window")` |
| A9 | Summary tokens ≤ S_max post-LLM | Guard | Truncar summary | `tracing::warn!(actual, cap, "summary exceeded cap, truncating")` |
| A10 | `is_error` preservado en truncation | Debug assert | Panic en debug | `debug_assert_eq!(before.is_error, after.is_error)` |

---

# 7. Test Plan

## Unit Tests

### IntentAnchor (`context/intent_anchor.rs`)
- `from_messages_extracts_first_user` — primer Role::User se usa como intent
- `from_messages_no_user_message` — produce anchor con placeholder
- `from_messages_multi_user` — usa el primero, no el último
- `mentioned_files_extraction` — paths comunes extraídos correctamente
- `format_for_boundary_contains_all_fields` — output contiene user_message, task_summary, working_dir, files

### ToolResultTruncator (`context/tool_result_truncator.rs`)
- `truncates_large_results` — result > threshold se trunca
- `preserves_small_results` — result < threshold intacto
- `preserves_tool_use_id` — id no cambia después de truncation
- `preserves_is_error` — is_error no cambia
- `skips_last_two_messages` — turno actual no se trunca
- `returns_count` — contador correcto
- `noop_on_empty` — 0 mensajes → noop
- `handles_text_messages` — mensajes Text no se tocan

### CompactionBudgetCalculator (`context/compaction_budget.rs`)
- `deepseek_64k_budget` — pipeline 51200, S_max=2560, trigger=44044
- `claude_200k_budget` — pipeline 160000, S_max=4000, trigger=151404
- `minimum_32k_budget` — pipeline 25600, S_max=1280
- `should_compact_above_threshold` — T ≥ trigger → true
- `should_compact_below_threshold` — T < trigger → false
- `utility_ratio_positive` — freed > added → positive
- `utility_ratio_negative` — freed < added → negative
- `utility_ratio_zero` — freed = added → 0
- `verify_post_compaction_ok` — within budget → Ok
- `verify_post_compaction_truncation_needed` — over budget → SummaryTruncationNeeded
- `extended_keep_count_basic` — cálculo correcto con avg tokens
- `extended_keep_count_capped` — no excede 2/3 × messages.len()
- `extended_keep_count_floor` — nunca menos que keep_count

### CompactionSummaryBuilder (`context/compaction_summary.rs`)
- `prompt_contains_9_sections` — cada sección presente en output
- `prompt_includes_intent_anchor` — intent anchor formateado presente
- `prompt_truncates_long_tool_results` — > 200 chars truncado
- `prompt_respects_keep_count` — últimos keep mensajes excluidos del resumen
- `prompt_includes_token_cap` — cap mencionado en instrucciones

### ProtectedContextInjector (`context/protected_context.rs`)
- `block_contains_boundary_markers` — markers presentes
- `block_contains_intent` — user_message presente
- `block_contains_tools` — tools listados
- `block_contains_files` — files listados
- `block_empty_lists` — listas vacías no rompen formato
- `block_contains_continuation` — instrucción "Continue" presente

### TieredCompactor (`context/tiered_compactor.rs`)
- `nominal_with_mock_provider` — mock retorna summary → nivel 1, utility > 0
- `degraded_on_provider_timeout` — mock timeout → nivel 2, extended keep
- `degraded_on_provider_error` — mock error → nivel 2
- `emergency_after_3_failures` — 3 failures → circuit breaker → nivel 3
- `circuit_breaker_resets_on_success` — success después de 2 failures → counter = 0
- `abort_on_negative_utility` — summary muy grande → utility ≤ 0 → abort
- `post_compaction_budget_ok` — verify dentro de budget
- `post_compaction_truncation` — verify over budget → truncation
- `tool_pair_safety_preserved` — safe_keep_boundary_n activo en los 3 niveles
- `noop_when_keep_exceeds_messages` — pocos mensajes → noop

## Integration Tests

### simplified_loop integration
- `proactive_compaction_semantic` — loop con mensajes suficientes → trigger → summary generado
- `proactive_compaction_fallback_off` — semantic_compaction=false → placeholder como hoy
- `reactive_compaction_async` — forzar prompt_too_long → arbiter Compact → TieredCompactor
- `feature_flag_off_preserves_behavior` — semantic_compaction=false → todo idéntico al estado actual

## Edge Cases

- `deepseek_64k_full_budget` — 51200 tokens, verify trigger, caps, keep count
- `summary_truncation_at_floor` — window 32K, S_max=1280, summary de 2000 → truncate
- `extended_keep_short_messages` — mensajes de ~5 tokens → extended keep alto → capped a 2/3
- `intent_anchor_no_user_role` — solo system messages → anchor con placeholder
- `utility_zero_exact` — T_freed = T_added exacto → abort (≤ 0)
- `circuit_breaker_exactly_3` — failure 1, 2, 3 → nivel 2, 2, 3
- `tool_result_truncation_boundary` — result de exactamente threshold tokens → no truncar
- `concurrent_truncation_and_compact` — truncation reduce tokens suficiente para skip compaction

---

# 8. Observability and Rollout Hooks

## Tracing Span

**Name:** `semantic_compaction`
**Level:** `INFO`

**Campos (todos registrados al cerrar el span):**

| Campo | Tipo | Source |
|-------|------|--------|
| `compaction.level` | &str | "nominal" / "degraded" / "emergency" / "aborted" |
| `compaction.trigger` | &str | "proactive" / "reactive" |
| `compaction.tokens_before` | u64 | estimate pre-compaction |
| `compaction.tokens_after` | u64 | estimate post-compaction |
| `compaction.summary_tokens` | u64 | S real (0 si nivel 2/3) |
| `compaction.protected_tokens` | u64 | P real |
| `compaction.keep_messages` | u32 | mensajes en keep window |
| `compaction.keep_tokens` | u64 | K estimado |
| `compaction.utility_ratio` | f64 | ratio calculado |
| `compaction.pipeline_budget` | u64 | B |
| `compaction.summary_cap` | u64 | S_max |
| `compaction.latency_ms` | u64 | tiempo de invocación LLM |
| `compaction.circuit_breaker` | u32 | consecutive_failures |
| `compaction.tool_results_truncated` | u32 | count del ToolResultTruncator |

## Logs específicos

| Evento | Level | Mensaje |
|--------|-------|---------|
| Compaction nominal exitosa | INFO | `"Semantic compaction completed"` con summary_tokens, utility |
| Fallback a degraded | WARN | `"Semantic compaction failed, using degraded mode"` con error |
| Emergency mode | WARN | `"Circuit breaker open, emergency compaction"` con failures |
| Abort (utility ≤ 0) | WARN | `"Compaction aborted: negative utility"` con utility, T_freed, T_added |
| Post-compaction truncation | ERROR | `"Post-compaction budget violation"` con tokens_after, budget |
| Tool results truncados | DEBUG | `"Truncated {n} tool results"` por turno |

## Métricas canary

Las 7 métricas del design doc se instrumentan via tracing events filtrados:
1. **Utility ratio:** histograma de `compaction.utility_ratio` por sesión
2. **Summary compliance:** % de compactions donde `summary_tokens ≤ summary_cap`
3. **Fallback rate:** % de compactions con level ≠ "nominal"
4. **Post-compaction halt rate:** correlacionar compaction events con halt events en 3 turnos
5. **Task completion:** correlacionar compaction events con session outcome
6. **Duplicate action rate:** comparar tool_use names en 5 turnos post-compaction
7. **Latency:** p50/p99 de `compaction.latency_ms` donde level = "nominal"

## Señales de rollback

Config toggle: `semantic_compaction = false`. Efecto inmediato en siguiente compaction (no en sesión en curso — toma efecto al crear nuevo TieredCompactor).

Trigger automático si:
- Tool pair safety errors detectados en logs (errores 400 de provider con ToolResult huérfano)
- Post-compaction halt rate aumenta > 10% vs baseline en 24h

---

# 9. Open Implementation Questions

1. **¿Cómo llega CompactionConfig al SimplifiedLoopConfig?** Actualmente, el compactor llega como `Option<&ContextCompactor>` desde dispatch.rs. La config completa debe pasar via un nuevo campo o extraerse del ContextCompactor existente. Resolución propuesta: añadir `compaction_config: CompactionConfig` a SimplifiedLoopConfig, populado desde AgentContext en dispatch.rs.

2. **¿`provider.invoke()` retorna stream o response directa?** El trait retorna `BoxStream<ModelChunk>`. Para la invocación de compaction, debemos consumir el stream completo y concatenar TextDelta chunks para obtener el summary. No hay `chat()` o método non-streaming directo en el trait. Resolución: helper interno que consume el stream y retorna `Result<String>`.

3. **¿Cómo se maneja `stream: false` en ModelRequest?** El campo existe pero algunos providers pueden ignorarlo. Resolución: enviar `stream: true` y consumir el stream completo. Es más robusto.

4. **¿`estimate_tokens` subestima significativamente?** Usa BPE simple (`text.len() / 4` aprox). Si subestima, el trigger se activa tarde y el summary puede no caber. Resolución: aceptable para Fase 1. El post-compaction verification es el guardrail. Si la estimación es consistentemente mala, Fase 2 puede integrar un tokenizer real.

5. **¿Dónde se almacena la CompactionConfig en el sistema?** Actualmente está en halcon_core::types y se usa para construir ContextCompactor. Los nuevos campos se propagan por el mismo path. Verificar que el config loader (config_loader.rs) deserializa correctamente con los campos nuevos vía `#[serde(default)]`.

---

# 10. Execution Checklist

| # | Paso | Componente | Depende de | Output | Done when |
|---|------|-----------|------------|--------|-----------|
| 1 | Extender CompactionConfig | config.rs (halcon-core) | — | 10 campos nuevos con defaults, serde compat | Tests de deserialización con configs viejas pasan |
| 2 | IntentAnchor | context/intent_anchor.rs | — | Struct + from_messages + format | 5 unit tests pasan |
| 3 | ToolResultTruncator | context/tool_result_truncator.rs | — | Función truncate | 8 unit tests pasan |
| 4 | CompactionBudgetCalculator | context/compaction_budget.rs | Config (#1) | compute, should_compact, verify, utility | 13 unit tests pasan |
| 5 | CompactionSummaryBuilder | context/compaction_summary.rs | IntentAnchor (#2) | build_prompt, format_messages | 5 unit tests pasan |
| 6 | ProtectedContextInjector | context/protected_context.rs | IntentAnchor (#2) | build_block | 6 unit tests pasan |
| 7 | Registrar módulos | context/mod.rs | #2-#6 | 6 líneas pub mod | Compila |
| 8 | TieredCompactor | context/tiered_compactor.rs | #2-#6 | compact async, 3 niveles, circuit breaker | 10 unit tests con mock provider pasan |
| 9 | Stream consumer helper | tiered_compactor.rs (interno) | — | Función que consume BoxStream → String | Test con mock stream |
| 10 | Integración: proactiva | simplified_loop.rs:158-163 | #8, Config | Bloque reemplazado, condicional por flag | Test integración proactivo pasa |
| 11 | Integración: reactiva | simplified_loop.rs:247-266 | #8, Config | Arms extraídos, async | Test integración reactivo pasa |
| 12 | Integración: apply_recovery | simplified_loop.rs:270-288 | #11 | Arms Compact eliminados | unreachable! no se alcanza en tests normales |
| 13 | Integración: dispatch | dispatch.rs | #1 | compaction_config propagada | Loop arranca con config correcta |
| 14 | Integración: files tracking | simplified_loop.rs:216-226 | — | files_modified se popula | Verificar en test integración |
| 15 | Tracing span | tiered_compactor.rs | #8 | 14 campos logueados | Verify en test integración |
| 16 | Feature flag off test | simplified_loop.rs | #10-#13 | semantic_compaction=false → placeholder | Test pasa, behavior idéntico al actual |
| 17 | Edge case tests | Varios | #1-#16 | 8 edge cases | Todos pasan |
| 18 | Cargo test completo | — | #1-#17 | `cargo test` exitoso | Zero failures |
| 19 | Smoke test manual | — | #18 | Sesión de 30+ turnos con semantic_compaction=true | Summary generado, intent preservado, no regresiones |

**Pasos 1-6 son paralelizables** (1 es prerequisito de 4, pero 2, 3, 5, 6 pueden ir en paralelo).
**Paso 8 integra 2-6.**
**Pasos 10-14 son la integración secuencial.**
**Pasos 15-19 son validación.**
