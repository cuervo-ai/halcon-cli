# Cierre de Brechas de Implementación: Compaction Semántica — Fase 1

**Fecha:** 2026-04-03
**Objetivo:** Gate final antes de implementación

---

## 1. Implementation Gap Verdict

**Implementation-ready with minor fixes.**

La spec está bien estructurada. Los 7 componentes tienen contracts definidos, el orden de implementación es correcto, y las decisiones frozen son coherentes. Hay 4 brechas de implementación reales que deben cerrarse — todas son clarificaciones mecánicas, no problemas arquitectónicos.

---

## 2. What Is Frozen and Correct

| Item | Status |
|------|--------|
| Budget equation: S + P + K + R ≤ B | Frozen. Verificado matemáticamente. |
| Trigger: T ≥ B - C_reserve | Frozen. Derivado del budget. |
| 3 niveles: nominal/degraded/emergency | Frozen. Monotonic info preservation verificada. |
| Protected context fusionado al boundary message | Frozen. Un solo Role::User. |
| Tool result truncation como prerequisito | Frozen. Función pura, sin riesgo. |
| Ambos paths por TieredCompactor | Frozen. Proactivo + reactivo. |
| Runtime config, no Cargo feature | Frozen. semantic_compaction: bool en CompactionConfig. |
| Circuit breaker: 3 failures, scope de sesión | Frozen. Lifetime del TieredCompactor. |
| Utility ≤ 0 → abort | Frozen. Bounded por max_compact_attempts = 2. |
| CompactionConfig en config.agent.compaction | Frozen. Path verificado: repl/mod.rs:2521. |
| ContextCompactor reutilizado internamente | Frozen. TieredCompactor posee un ContextCompactor. |
| Scope: cancel_token y diminishing returns fuera | Frozen. |
| 7 métricas de canary | Frozen. |

---

## 3. Remaining Implementation Gaps

### Gap 1: Borrow conflict en la extracción del reactive path [MUST FIX]

**Problema concreto.** La spec propone este refactoring del match en línea 247:

```
match arbiter.decide(...) {
    TurnDecision::Recover(ref act) => {
        match act {
            RecoveryAction::Compact | ReactiveCompact => {
                // usa &mut messages, config.provider, &intent_anchor, etc.
                tc.compact(&mut messages, ...).await;
            }
            _ => {
                apply_recovery(&mut messages, act, ...);
            }
        }
    }
}
```

Esto funciona porque `arbiter.decide()` retorna un owned `TurnDecision`, y `act` se puede tomar por referencia para el inner match. No hay borrow conflict aquí. El `&mut messages` en el arm async y el `&mut messages` en apply_recovery son mutuamente excluyentes (diferentes arms del match).

**Verificación: este enfoque es correcto.** `act` es owned por el arm del match. `apply_recovery` toma `&RecoveryAction` (referencia). El refactoring propuesto no tiene borrow issues.

**Sin embargo**, el `compact_count += 1` que actualmente está dentro de `apply_recovery` (línea 273: `*cmp += 1`) debe moverse al arm del loop. En la spec esto está indicado pero debe enfatizarse: el incremento de `compact_count` ocurre en el arm del loop body, NO dentro de TieredCompactor. TieredCompactor no conoce el counter del arbiter.

**Resolución:** Congelar. El approach es correcto. Solo asegurar que `compact_count += 1` y `esc_count = 0` ocurren en el arm del loop antes de llamar a `tc.compact()`.

### Gap 2: Stream consumption — chunks vacíos y error mid-stream [MUST FIX]

**Problema concreto.** `provider.invoke()` retorna `BoxStream<'static, Result<ModelChunk>>`. El spec dice "helper interno que consume el stream y retorna Result<String>". Pero no especifica:

a) Qué hacer si el stream emite `ModelChunk::Error(msg)` mid-stream (después de algunos TextDelta).
b) Qué hacer si el stream termina sin `ModelChunk::Done`.
c) Qué hacer si no se emite ningún `ModelChunk::TextDelta` (response vacío).

**Resolución:**

a) Error mid-stream: acumular lo que se tiene hasta el error. Si hay texto acumulado > 0, usarlo como summary parcial (mejor que nada). Si no hay texto, tratar como failure.

b) Stream sin Done: tratar como success si hay texto acumulado. El Done es un signal de completion, pero el texto ya se acumuló.

c) Response vacío (cero TextDelta): tratar como failure. Incrementar circuit breaker.

**Contract del helper:**
```
async fn consume_summary_stream(
    stream: BoxStream<'static, Result<ModelChunk>>,
    timeout: Duration,
) -> Result<String>
// Ok(text) si al menos un TextDelta fue recibido
// Err si timeout, si error antes de cualquier texto, o si cero TextDelta
```

### Gap 3: CompactionConfig propagation — dispatch.rs no tiene acceso a config [MUST FIX]

**Problema concreto.** dispatch.rs construye `SimplifiedLoopConfig` desde `AgentContext`. La spec añade `compaction_config: CompactionConfig` a SimplifiedLoopConfig. Pero `AgentContext` no tiene un campo `CompactionConfig` directo — tiene `compactor: Option<&'a ContextCompactor>`, y el ContextCompactor tiene `config: CompactionConfig` (privado, sin getter público).

**Opciones:**
1. Añadir `pub fn config(&self) -> &CompactionConfig` al ContextCompactor.
2. Añadir `compaction_config: CompactionConfig` al AgentContext.
3. Extraer la config del ContextCompactor en dispatch.rs.

**Resolución:** Opción 1. Añadir un getter público `config()` al ContextCompactor. Es el cambio mínimo — una línea. Luego en dispatch.rs:

```
compaction_config: ctx.compactor
    .map(|c| c.config().clone())
    .unwrap_or_default(),
```

### Gap 4: `apply_compaction_keep` es privada — TieredCompactor nivel 2 no puede usarla directamente [MUST FIX]

**Problema concreto.** En nivel 2 (degraded), el TieredCompactor necesita aplicar compaction con extended_keep_count diferente del keep_count normal. La spec dice usar `apply_compaction_with_budget()`. Pero `apply_compaction_keep()` (que es la función interna que realmente aplica con un keep específico) es privada en ContextCompactor.

**Opciones:**
1. Hacer `apply_compaction_keep` pública.
2. Usar `apply_compaction_with_budget` con un pipeline_budget artificial que produce el extended_keep_count deseado.
3. TieredCompactor implementa la lógica de replacement directamente (duplicación).

**Resolución:** Opción 1. Cambiar `fn apply_compaction_keep` a `pub fn apply_compaction_keep` en compaction.rs. Es un cambio de visibilidad de una línea. Permite al TieredCompactor usar cualquier keep count con safe_keep_boundary_n. Alternativa: exponer un nuevo método `pub fn apply_compaction_with_keep(&self, messages, summary, keep_count)` que combine safe_keep_boundary_n + apply_compaction_keep. Preferible este último para no exponer el internal.

---

## 4. Contract Corrections

### 4.1 TieredCompactor::compact — firma completa

La spec actual define:
```
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
```

**Corrección:** Añadir `trigger_source: CompactionTrigger` para distinguir proactivo de reactivo en tracing:
```
pub enum CompactionTrigger { Proactive, Reactive }
```

No afecta la lógica — solo el campo `compaction.trigger` en tracing.

### 4.2 ContextCompactor — nuevo método público

Añadir:
```
pub fn apply_compaction_with_keep_count(
    &self,
    messages: &mut Vec<ChatMessage>,
    summary: &str,
    keep_count: usize,
) -> usize  // retorna keep efectivo (post safe_keep_boundary)
```

Este método:
1. Calcula `safe_keep = self.safe_keep_boundary_n(messages, keep_count)`.
2. Llama `self.apply_compaction_keep(messages, summary, safe_keep)`.
3. Retorna `safe_keep` para tracing.

Necesario para TieredCompactor nivel 2 (extended keep) y nivel 3 (min keep = 4).

### 4.3 ContextCompactor — getter público

Añadir:
```
pub fn config(&self) -> &CompactionConfig
```

Necesario para dispatch.rs config propagation.

### 4.4 consume_summary_stream — helper interno

```
async fn consume_summary_stream(
    provider: &Arc<dyn ModelProvider>,
    request: &ModelRequest,
    timeout: Duration,
) -> Result<String>
```

Internamente:
1. `tokio::time::timeout(timeout, provider.invoke(request))`
2. Consume stream: accumula TextDelta, ignora ThinkingDelta, registra Usage.
3. Si Error chunk: si hay texto acumulado, retorna Ok(texto). Si no, retorna Err.
4. Si stream termina sin texto: retorna Err.

---

## 5. Runtime Safety Corrections

### Assertions que faltan en la spec

| # | Condición | Dónde | Severity | Acción |
|---|-----------|-------|----------|--------|
| S1 | `config.utilization_factor > 0.0 && config.utilization_factor <= 1.0` | CompactionBudgetCalculator::compute | Guard | Clamp a [0.5, 1.0], loguear warning |
| S2 | `pipeline_budget > C_reserve` | CompactionBudgetCalculator::compute | Guard | Si no, loguear error, usar fallback B×0.60 |
| S3 | `keep_count ≤ messages.len()` después de safe_keep_boundary_n | TieredCompactor::compact | Invariant | safe_keep_boundary_n ya garantiza esto |
| S4 | Summary no contiene tool calls | TieredCompactor::compact | Guard | Si el summary del LLM contiene `[Tool call:` o ToolUse blocks: truncar al texto puro, loguear warning |
| S5 | Boundary message no excede P_max + S_max | TieredCompactor, post-merge | Guard | Si excede: truncar summary portion |
| S6 | `messages.len() > 0` post-compaction | TieredCompactor::compact | Debug assert | Si 0 mensajes post-compaction, el loop fallará en el siguiente invoke |
| S7 | Primer mensaje post-compaction es Role::User | TieredCompactor::compact | Debug assert | apply_compaction_keep garantiza esto; verificar en test |

### Fallo silencioso a prevenir

**Riesgo:** Si `estimate_tokens` subestima consistentemente, el trigger se activa tarde, el summary se genera, pero `T_after + R > B` siempre falla el post-verification. Resultado: truncación repetida del summary, degradando su calidad sin alerta clara.

**Mitigación:** Si post-compaction verification necesita truncar más de 50% del summary en 2 compactions consecutivas: loguear `tracing::error!("token estimation may be unreliable")`. Esto permite detectar el problema en canary.

---

## 6. Operational Sanity Check

### Secuencia operacional completa

```
TURNO N:

1. CANCELLATION CHECK
   is_cancelled() → return si true
   ✓ Sin cambios

2. TOOL RESULT TRUNCATION  [NUEVO]
   SI semantic_compaction = true:
     truncate_large_tool_results(&mut messages, threshold, preview)
     → loguear count si > 0
   ✓ Pre-condición: messages tiene al menos los mensajes del turno anterior

3. TOKEN ESTIMATION
   est = ContextCompactor::estimate_message_tokens(&messages)
   ✓ Estimación sobre mensajes ya truncados

4. BUDGET COMPUTATION  [NUEVO]
   SI semantic_compaction = true:
     budget = CompactionBudgetCalculator::compute(pipeline_budget, max_output_tokens, &config)
     should_compact = est ≥ budget.trigger_threshold
   ELSE:
     should_compact = est > ctx_budget × COMPACTION_THRESHOLD (0.90)
   ✓ Fallback preserva behavior actual

5. COMPACTION (si should_compact)
   SI semantic_compaction = true:
     result = tiered_compactor.compact(...).await
     compact_count += 1  ← IMPORTANTE: incrementar AQUÍ, no en TieredCompactor
     SI result.aborted: loguear, continuar sin compactar
   ELSE:
     c.apply_compaction(&mut messages, "[Context compacted proactively]")
   ✓ Ambos paths cubiertos

6. MODEL CALL + STREAMING
   ✓ Sin cambios

7. TOOL EXECUTION (si tool_use)
   ✓ Sin cambios
   Post-execution: extraer files_modified de Edit/Write tool calls

8. ARBITER DECISION (si no tool_use)
   match arbiter.decide() {
     Complete → return
     Recover(Compact | ReactiveCompact) → {
       compact_count += 1; esc_count = 0;
       SI semantic_compaction:
         truncate + compact async  ← EXTRAÍDO de apply_recovery
       ELSE:
         placeholder compaction
     }
     Recover(other) → apply_recovery(...)  ← SYNC, sin cambios
     Halt → return
   }

9. CONTINUE LOOP
```

### Agujeros detectados y cerrados

**Agujero 1: ¿truncation se ejecuta también antes de compaction reactiva?**
Sí. En el arm Compact/ReactiveCompact del step 8, se ejecuta truncation antes de compact. Esto es necesario porque la compaction reactiva ocurre cuando el API ya rechazó por prompt_too_long — los tool results grandes pueden ser la causa.

**Agujero 2: ¿compact_count se incrementa antes o después de compact()?**
ANTES de llamar a compact(). Razón: compact_count alimenta al arbiter en la siguiente iteración. Si compact() tarda y el loop se cancela, compact_count ya refleja el intento. Si compact() aborta (utility ≤ 0), compact_count sigue incrementado — correcto, porque el arbiter necesita saber que se intentó compactar.

**Agujero 3: ¿qué pasa si semantic_compaction se cambia mid-session?**
La config se lee al construir TieredCompactor (inicio del loop). Si alguien cambia la config durante la sesión, el cambio no toma efecto hasta la siguiente sesión. Esto es correcto y deseable — no queremos behavior mixto dentro de una sesión.

---

## 7. Observability and Rollout Corrections

### Campo faltante en tracing

Añadir `compaction.estimated_tokens_before_truncation: u64` — los tokens ANTES de tool result truncation. Esto permite calcular cuántos tokens ahorró la truncation vs cuántos ahorró la compaction, y calibrar el threshold de truncation en canary.

### Señal faltante para duplicate action rate

La métrica "duplicate action rate" requiere comparar tool calls post-compaction con tool calls pre-compaction. El tracing span de compaction no tiene esta info. **Resolución:** No es responsabilidad del span de compaction. El duplicate action rate se calcula offline correlacionando tool_use events antes y después de eventos de compaction en la misma sesión. El tracing ya emite tool names en tools_executed. Suficiente.

### Rollout: first-compaction behavior

Cuando `semantic_compaction = true`, la PRIMERA compaction de una sesión es la de mayor riesgo (primera invocación LLM para summary, latencia desconocida, calidad desconocida). Las métricas de canary deben poder filtrar por "first compaction in session" vs "subsequent compactions".

**Resolución:** Añadir `compaction.sequence_number: u32` al tracing span (1 = primera, 2 = segunda, etc.). Es `compact_count` al momento de la compaction.

### Config rollback

`semantic_compaction = false` en config → siguiente sesión usa placeholder. Sesión en curso no se afecta (TieredCompactor ya creado). Esto es correcto. Documentar: "cambio de config toma efecto en la siguiente sesión, no en la sesión en curso."

---

## 8. Decisions to Freeze Now

| # | Decisión | Valor final | Categoría |
|---|----------|-------------|-----------|
| 1 | `compact_count` se incrementa en el loop body, no en TieredCompactor | Siempre antes de llamar compact() | Frozen |
| 2 | ContextCompactor expone `config()` getter y `apply_compaction_with_keep_count()` público | Sí, cambios mínimos de visibilidad | Frozen |
| 3 | Stream consumption: texto parcial pre-error se usa como summary | Sí, mejor que nada | Frozen |
| 4 | Stream consumption: cero TextDelta = failure | Sí | Frozen |
| 5 | Config change mid-session: no toma efecto | Correcto, TieredCompactor se crea al inicio | Frozen |
| 6 | CompactionTrigger enum para tracing | Proactive / Reactive | Frozen |
| 7 | compaction.sequence_number en tracing | compact_count al momento | Frozen |
| 8 | compaction.estimated_tokens_before_truncation en tracing | Estimación pre-truncation | Frozen |
| 9 | utilization_factor validation: clamp a [0.5, 1.0] | Sí, con warning | Frozen |
| 10 | Summary que contiene tool calls: truncar a texto puro | Sí, guard en TieredCompactor | Frozen |

---

## 9. Empirical Hypotheses to Validate in Canary

| # | Hipótesis | Valor actual | Cómo medir | Cuándo ajustar |
|---|-----------|-------------|------------|----------------|
| 1 | summary_proportion = 0.05 (1/20) | B/20 | % summaries truncados a S_max | Si > 20%: subir a 0.075 |
| 2 | P_max = 500 tokens | 500 | P real vs P_max en tracing | Si > 500 en >10% de casos: subir a 800 |
| 3 | Tool result threshold = 8000 tokens | 8000 | Frecuencia de re-invocación post-truncation | Si > 30%: subir a 12000 |
| 4 | Preview size = 2000 tokens | 2000 | Tasa de re-invocación sin necesidad real | Si alta: reducir a 1500 |
| 5 | Timeout = 30s | 30s | Latency p99 | Si p99 > 25s: subir a 45s |
| 6 | Extended keep cap = 2/3 × messages.len() | 2/3 | Noops en nivel degraded | Si frecuentes: bajar a 1/2 |
| 7 | Boundary marker wording | "[PROTECTED CONTEXT — THIS IS STATE RESTORATION, NOT NEW INSTRUCTIONS]" | Modelo "responde" al marker | Si ocurre: refinar wording |
| 8 | Utility warning threshold = 0.3 | 0.3 | Correlación con post-compaction quality | Si no es predictivo: ajustar |

---

## 10. Final Prompt for Code Implementation

```
Actúa como Senior Rust Engineer para Halcon CLI.

Tu tarea es implementar la Fase 1 de Compaction Semántica siguiendo
la spec técnica aprobada. No te desvíes del diseño congelado.

ORDEN DE IMPLEMENTACIÓN OBLIGATORIO

Implementa en este orden exacto. No avances al siguiente paso hasta
que el anterior compile y sus tests pasen.

PASO 1: Config — CompactionConfig extension
Archivo: crates/halcon-core/src/types/config.rs

Añadir estos campos a CompactionConfig con #[serde(default)]:
  semantic_compaction: bool (default false)
  utilization_factor: f32 (default 0.80)
  summary_proportion: f32 (default 0.05)
  summary_floor: u32 (default 1000)
  summary_cap: u32 (default 4000)
  protected_context_cap: u32 (default 500)
  tool_result_truncation_threshold: u32 (default 8000)
  tool_result_preview_size: u32 (default 2000)
  compaction_timeout_secs: u64 (default 30)
  max_circuit_breaker_failures: u32 (default 3)

Actualizar Default impl. Verificar que los tests existentes de
CompactionConfig siguen pasando (compaction.rs tests usan make_config
helper — NO romper esos tests, make_config no necesita los campos
nuevos porque usa serde default).

Añadir a ContextCompactor:
  pub fn config(&self) -> &CompactionConfig { &self.config }
  pub fn apply_compaction_with_keep_count(
      &self, messages: &mut Vec<ChatMessage>, summary: &str,
      keep_count: usize,
  ) -> usize {
      let safe = self.safe_keep_boundary_n(messages, keep_count);
      self.apply_compaction_keep(messages, summary, safe);
      safe
  }

safe_keep_boundary_n ya es privada y la necesitas — NO la hagas pública.
apply_compaction_with_keep_count la usa internamente.

Tests: verificar que configs antiguas (sin campos nuevos) deserializan
correctamente con defaults.

PASO 2: IntentAnchor
Archivo: crates/halcon-cli/src/repl/context/intent_anchor.rs (nuevo)

Struct IntentAnchor con: user_message, task_summary (500 chars),
mentioned_files (regex heuristic), working_dir, created_at.
from_messages(&[ChatMessage], &str) → busca primer Role::User.
Si no hay User → user_message = "[no user message found]".
format_for_boundary() → string formateado.

Tests: 5 unit tests (ver spec sección 7).

PASO 3: ToolResultTruncator
Archivo: crates/halcon-cli/src/repl/context/tool_result_truncator.rs (nuevo)

Función pub truncate_large_tool_results(
    messages: &mut Vec<ChatMessage>,
    threshold_tokens: usize,
    preview_tokens: usize,
) -> u32

Itera mensajes excepto los últimos 2.
Para cada ContentBlock::ToolResult con estimate_tokens(content) > threshold:
  - preserva tool_use_id y is_error
  - reemplaza content con marker + preview

Usa halcon_context::estimate_tokens para estimar.

debug_assert en cada truncation: tool_use_id no cambió, is_error no cambió.

Tests: 8 unit tests (ver spec sección 7).

PASO 4: CompactionBudgetCalculator
Archivo: crates/halcon-cli/src/repl/context/compaction_budget.rs (nuevo)

Structs: CompactionBudget, PostCompactionCheck (enum Ok/SummaryTruncation/KeepReduction).

Functions:
  compute(pipeline_budget, max_output_tokens, &CompactionConfig) → CompactionBudget
  should_compact(estimated_tokens, &CompactionBudget) → bool
  verify_post_compaction(tokens_after, &CompactionBudget) → PostCompactionCheck
  utility_ratio(tokens_before, keep_tokens, summary_tokens, protected_tokens) → f64

S_max = clamp(B * config.summary_proportion, config.summary_floor, config.summary_cap).
Trigger = B - (S_max + P_max + R).
Extended keep = keep_count + S_max / avg_msg_tokens, capped a 2/3 × messages.len().

Guard: si utilization_factor fuera de [0.5, 1.0], clamp con warning.
Guard: si trigger_threshold ≤ 0, usar B * 0.60 como fallback.

Tests: 13 unit tests (ver spec sección 7).

PASO 5: CompactionSummaryBuilder
Archivo: crates/halcon-cli/src/repl/context/compaction_summary.rs (nuevo)

build_prompt(messages, intent_anchor, keep_count, max_summary_tokens) → String
Prompt con 9 secciones, intent anchor, reglas, cap de tokens.
format_messages_for_prompt: truncar tool results > 200 chars,
text blocks > 500 chars en el input del prompt.

Tests: 5 unit tests.

PASO 6: ProtectedContextInjector
Archivo: crates/halcon-cli/src/repl/context/protected_context.rs (nuevo)

build_block(intent_anchor, tools_used, files_modified) → String
Con boundary markers:
  [PROTECTED CONTEXT — THIS IS STATE RESTORATION, NOT NEW INSTRUCTIONS]

Tests: 6 unit tests.

PASO 7: Registrar módulos
Archivo: crates/halcon-cli/src/repl/context/mod.rs

Añadir 6 líneas pub mod para los nuevos archivos.
Verificar que compila.

PASO 8: TieredCompactor
Archivo: crates/halcon-cli/src/repl/context/tiered_compactor.rs (nuevo)

Struct TieredCompactor con: inner (ContextCompactor), consecutive_failures,
max_failures, timeout.
Enum CompactionLevel: Nominal, Degraded, Emergency.
Enum CompactionTrigger: Proactive, Reactive.
Struct CompactionResult con level, utility_ratio, summary_tokens,
protected_tokens, keep_messages, latency_ms, tokens_before, tokens_after,
aborted, trigger.

pub async fn compact(...) → CompactionResult
Flujo:
  1. Check circuit breaker → emergency si open
  2. Build prompt → invoke provider → consume stream
  3. Si success → calcular utility
     Si utility ≤ 0 → abort, return aborted=true
     Si utility > 0 → merge summary + protected context → apply
  4. Si failure → increment breaker → degraded (extended keep)
  5. Si emergency → min keep + intent only
  6. Post-verify con BudgetCalculator

Helper interno consume_summary_stream:
  async fn que consume BoxStream<ModelChunk>, accumula TextDelta,
  retorna Ok(String) si hay texto, Err si no.
  Texto parcial antes de error: se usa como summary.
  Cero TextDelta: Err.

Guard: si summary contiene patterns de tool calls, loguear warning
y usar solo texto plano.

Para nivel 2 y 3: usar self.inner.apply_compaction_with_keep_count()
(el nuevo método público del paso 1).

Tests: 10 unit tests con mock provider. El mock provider debe implementar
ModelProvider trait y retornar streams controlados (success, error,
timeout, empty).

PASO 9: Integración en simplified_loop
Archivo: crates/halcon-cli/src/repl/agent/simplified_loop.rs

A. Añadir compaction_config: CompactionConfig a SimplifiedLoopConfig.

B. Antes del loop (después de línea 151):
   - Crear IntentAnchor desde messages + working_dir.
   - Si semantic_compaction = true: crear TieredCompactor.
   - Calcular pipeline_budget.
   - Inicializar files_modified: Vec<String>.

C. Reemplazar bloque proactivo (líneas 158-163):
   - Si tiered_compactor exists: truncate → estimate → budget →
     should_compact → compact.await
   - Else: placeholder como hoy (preservar código actual).

D. Después de tool execution (línea 226):
   - Extraer file paths de Edit/Write tool calls → files_modified.

E. Refactorizar match del arbiter (líneas 247-266):
   - Recover(Compact | ReactiveCompact): manejar inline, async.
     compact_count += 1; esc_count = 0;
     truncate → compact.await
   - Recover(other): delegar a apply_recovery.

F. En apply_recovery (línea 270): reemplazar arms Compact|ReactiveCompact
   con unreachable!("handled in async loop body").

G. Actualizar dispatch.rs: propagar compaction_config desde
   ctx.compactor.map(|c| c.config().clone()).unwrap_or_default().

Tests de integración:
  - semantic_compaction=false → idéntico al behavior actual
  - semantic_compaction=true → compaction nominal con mock
  - reactive path → compaction via arbiter trigger

PASO 10: Tracing
En TieredCompactor::compact, crear un tracing::info_span!("semantic_compaction")
con los 16 campos:
  level, trigger, tokens_before, tokens_after, summary_tokens,
  protected_tokens, keep_messages, keep_tokens, utility_ratio,
  pipeline_budget, summary_cap, latency_ms, circuit_breaker,
  tool_results_truncated, sequence_number, estimated_tokens_before_truncation

PASO 11: Verificación final
  cargo test — todos los tests pasan
  cargo clippy — sin warnings nuevos
  Test manual con semantic_compaction=true en sesión de 30+ turnos

REGLAS ESTRICTAS

- NO modifiques FeedbackArbiter. max_compact_attempts = 2 se queda.
- NO toques cancel_token ni diminishing returns. Fuera de scope.
- NO cambies la lógica de tool_executor ni la de streaming.
- NO añadas features de Fase 2 (persistencia a disco, IntentAnchor
  dinámico, multi-tier).
- Preserva TODOS los tests existentes de compaction.rs.
- Cada paso debe compilar y pasar tests antes del siguiente.
- Usa tracing (no println/eprintln).
- Usa anyhow::Result para errors.
- Sigue el estilo del código existente (compacto, funcional).

IMPORTANTE: Si encuentras una ambigüedad real no cubierta aquí,
documéntala como comentario TODO(semantic-compaction) y toma la
decisión conservadora (la que preserva más información y es más
fácil de revertir). No te detengas a preguntar — implementa y
documenta.
```

---

**El diseño está listo. Las 4 brechas de implementación son mecánicas. El prompt de implementación cubre los 11 pasos en orden con criterios de done claros.**
