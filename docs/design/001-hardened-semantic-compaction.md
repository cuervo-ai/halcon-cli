# Compaction Semántica y Preservación de Intent — Fase 1 (Hardened)

**Autor:** Oscar Valois
**Fecha:** 2026-04-03
**Estado:** Revisado — listo para implementación
**Base:** Design doc original + design review + evidencia verificada

---

## 1. Executive Verdict

**Viable con correcciones incorporadas.**

El diseño original identifica correctamente la causa raíz (compaction placeholder-based que destruye contexto) y propone los componentes adecuados (IntentAnchor, CompactionSummaryBuilder, TieredCompactor, ProtectedContextInjector). Las correcciones incorporadas en esta versión cierran las cinco brechas estructurales detectadas en review:

1. Se añade un budget model explícito con ecuación verificable.
2. Se cubren ambos paths de compaction (proactivo y reactivo).
3. La re-inyección se fusiona al compaction boundary message, no como mensaje separado.
4. Se implementa degradación escalonada de 3 niveles en vez de fallback binario.
5. Se incorpora tool result truncation como prerequisito mínimo.

Se separan del scope: cancel_token, diminishing returns, hook_runner.

---

## 2. Verified vs Unverified Claims

| # | Claim | Status | Evidence type | Design consequence |
|---|-------|--------|---------------|-------------------|
| 1 | simplified_loop es el único runtime activo | Verified | Codebase: dispatch.rs:32-59, GDEM feature-gated nunca compilado | No se necesita compatibilidad con GDEM. Base firme. |
| 2 | Compaction usa placeholder sin LLM en dos paths | Verified | Codebase: simplified_loop.rs:161 (proactivo), :274 (reactivo) | Ambos paths deben corregirse. El doc original solo cubría uno. |
| 3 | compaction_prompt() existe pero nunca se invoca | Verified | Codebase: compaction.rs:84-131, cero call sites en el loop | El prompt existente tiene 4 secciones. Se reemplaza, no se reutiliza. |
| 4 | cancel_token: None, hook_runner: None | Verified | Codebase: dispatch.rs:55, :33-34 | Separados del scope de este doc. Correcciones ortogonales. |
| 5 | max_compact_attempts = 2, max 2 compactions → halt | Verified | Codebase: feedback_arbiter.rs:116 | Se mantiene. La compaction semántica no cambia este límite. |
| 6 | BudgetTracker.diminishing es sticky (once true, stays true) | Verified | Codebase: simplified_loop.rs:89, campo bool sin reset | Bug latente. Fuera de scope pero documentado como riesgo conocido. |
| 7 | needs_compaction_with_budget() existe con threshold 60% | Verified | Codebase: compaction.rs:70-81 | Debe usarse en vez del threshold hardcoded de 90%. |
| 8 | CompactionConfig tiene threshold_fraction 0.80, max_context_tokens 200K | Verified | Codebase: config.rs:1247-1255 | El loop ignora estos valores y usa sus propias constantes. Incoherencia a resolver. |
| 9 | ctx_budget = model_context_window() sin factor de utilización | Verified | Codebase: simplified_loop.rs:150 | El loop usa el window completo (e.g. 200K), no un pipeline budget (e.g. 160K). El trigger a 90% de 200K = 180K. |
| 10 | XIYO usa prompt de 9 secciones para compaction | Verified internally | Fuente interna: xiyo/services/compact/prompt.ts:61-127 | Patrón adoptado. No es evidencia pública generalizable. |
| 11 | XIYO re-inyecta 6 tipos post-compact | Verified internally | Fuente interna: xiyo/services/compact/compact.ts:541-585 | Patrón adoptado parcialmente (3 tipos en Fase 1). |
| 12 | XIYO circuit breaker: 3 failures, motivado por 250K API calls/día desperdiciadas | Verified internally | Fuente interna: xiyo/autoCompact.ts:68-70, comentario BQ | Valor adoptado con justificación interna real. |
| 13 | XIYO summary cap: min(20K, model_max_output) | Verified internally | Fuente interna: xiyo/compact.ts:1317-1320 | Nuestro cap será proporcional al budget, más conservador. |
| 14 | XIYO threshold: effectiveWindow - 13K buffer | Verified internally | Fuente interna: xiyo/autoCompact.ts:62-76 | Patrón de absolute buffer, no porcentaje fijo. Adoptado como inspiración. |
| 15 | XIYO tool result budget: 50K chars/tool, 200K chars/message, ejecutado cada turno antes de compaction | Verified internally | Fuente interna: xiyo/toolLimits.ts, query.ts:379 | Confirma que tool result budgeting es prerequisito de compaction, no addon. |
| 16 | "42% reducción en task success rate" por context rot | Weak evidence | Paper/preprint sin cita completa verificable | NO usar como hecho cuantitativo. Usar como motivación cualitativa: "evidencia preliminar sugiere degradación significativa". |
| 17 | "Context rot es propiedad estructural del transformer" (Chroma) | Weak evidence | Benchmark referenciado sin cita completa | Hipótesis plausible pero no verificada internamente. Motivación, no fundamento. |
| 18 | evidence_verified: true hardcoded | Verified | Codebase: simplified_loop.rs:125 | Fuera de scope Fase 1. Riesgo conocido. |

---

## 3. Critical Gaps (cerrados en esta versión)

### Gap 1: Ausencia de budget model post-compaction — CERRADO
**Severidad:** Crítica.
**Problema:** No existía ecuación que verificara viabilidad del estado post-compaction.
**Resolución:** Sección 5 define el budget model completo con ecuación, variables, caps, failure conditions y monitoreo.

### Gap 2: Cobertura incompleta de paths — CERRADO
**Severidad:** Crítica.
**Problema:** El doc original solo cubría compaction proactiva (simplified_loop.rs:158-163). La compaction reactiva (apply_recovery:272-275) seguía usando placeholder.
**Resolución:** El TieredCompactor se usa en ambos paths. apply_recovery delega al TieredCompactor para RecoveryAction::Compact y ReactiveCompact.

### Gap 3: Re-inyección con boundary incorrecto — CERRADO
**Severidad:** Alta.
**Problema:** Protected context como mensaje Role::User separado puede ser interpretado como instrucción nueva.
**Resolución:** Protected context se fusiona al compaction boundary message existente. Un solo mensaje Role::User con boundary markers internos. Ver decisión D4.

### Gap 4: Fallback binario insuficiente — CERRADO
**Severidad:** Alta.
**Problema:** Solo dos niveles: semantic summary OR placeholder. Viola progressive degradation.
**Resolución:** 3 niveles de degradación: nominal, degraded, emergency. Ver decisión D3.

### Gap 5: Tool result budgeting excluido — CERRADO
**Severidad:** Alta.
**Problema:** Sin tool result truncation, la misma inflación por tool outputs sigue forzando compaction con la misma frecuencia.
**Resolución:** Tool result truncation inline entra en Fase 1 como prerequisito. Ver decisión D5.

### Gap 6: Thresholds fijos sin justificación — CERRADO
**Severidad:** Media.
**Problema:** 85% (propuesto) y 90% (actual) son constantes arbitrarias que ignoran el budget real.
**Resolución:** Trigger derivado del budget con compaction_reserve explícito. Ver decisión D1.

### Gap 7: Métricas de validación débiles — CERRADO
**Severidad:** Media.
**Problema:** "Reducción medible en oscilación" no es una métrica.
**Resolución:** 7 métricas cuantitativas con definiciones, thresholds y señales de rollback. Ver sección 7.

### Gap 8: Scope contaminado — CERRADO
**Severidad:** Baja.
**Problema:** cancel_token y diminishing returns mezclados con compaction semántica.
**Resolución:** Separados a correcciones independientes fuera de este doc. Ver decisión D6.

---

## 4. Corrected Design Decisions

### D1: Trigger de compaction — derivado del budget, no constante fija

**Cambio:** Reemplazar `COMPACTION_THRESHOLD: f64 = 0.90` con trigger basado en reserve explícita.

**Formulación:**
```
trigger_when: estimated_tokens(messages) ≥ B - C_reserve

Donde:
  B = pipeline_budget = context_window × utilization_factor
  C_reserve = S_max + P_max + R
  S_max = cap de summary tokens
  P_max = cap de protected context tokens (500)
  R = max_output_tokens (default 4096)
```

**Implementación:** Reutilizar `needs_compaction_with_budget()` (compaction.rs:70-81) modificando su threshold de `0.60 * pipeline_budget` a `pipeline_budget - C_reserve`. Esto es más preciso que un porcentaje fijo porque adapta el trigger al overhead real de compaction.

**Justificación:**
- El threshold actual de 90% × ctx_budget usa el context_window COMPLETO (200K), no un pipeline budget. Esto trigger a 180K para un window de 200K — dejando solo 20K, insuficiente si el summary + protected context + response necesitan más.
- El threshold propuesto de 85% era igualmente arbitrario.
- XIYO usa `effectiveWindow - 13K_buffer`, que es un absolute reserve, no un porcentaje. Nuestro approach generaliza esto con un reserve derivado del budget model.

**Trade-off:** El trigger se activa ligeramente más temprano que el 90% actual para windows grandes (porque C_reserve < 10% de B cuando B > 70K). Para windows pequeños (DeepSeek 64K), el trigger se activa significativamente más temprano, lo cual es correcto porque hay menos margen.

**Fallback de compatibilidad:** Si `pipeline_budget` no está disponible (provider no reporta context_window), usar `config.max_context_tokens × config.threshold_fraction` del CompactionConfig existente.

### D2: Cap del summary — proporcional al budget con floor y ceiling

**Cambio:** El cap de summary no es fijo. Se calcula como:
```
S_max = clamp(B / 20, 1000, 4000)
```

**Valores resultantes:**

| Context window | B (×0.80) | S_max |
|----------------|-----------|-------|
| 32K | 25.6K | 1280 |
| 64K (DeepSeek) | 51.2K | 2560 |
| 128K | 102.4K | 4000 (cap) |
| 200K (Claude) | 160K | 4000 (cap) |

**Justificación:**
- XIYO usa cap de min(20K, model_max_output) — mucho más generoso. Nuestro cap es conservador para Fase 1 porque cada token de summary desplaza un token de contexto real.
- El floor de 1000 garantiza espacio para las 9 secciones incluso en windows pequeños. Un summary con 9 secciones a ~100 tokens por sección = ~900 tokens mínimo.
- El ceiling de 4000 evita summaries que consuman >2.5% del budget en windows grandes. Suficiente para sesiones de alta complejidad.
- La proporción 1/20 es una **hipótesis operacional** basada en el análisis de que las 9 secciones necesitan ~100-400 tokens cada una para sesiones de 30-80 turnos. NO es un hecho empírico — debe validarse en canary.

**Trade-off:** Summaries más cortos = más información perdida. Summaries más largos = menos espacio para contexto real. El punto 1/20 es un compromiso conservador que puede ajustarse post-canary.

### D3: Degradación escalonada de 3 niveles — placeholder puro rechazado como fallback primario

**Cambio:** Reemplazar el fallback binario (semantic / placeholder) con 3 niveles.

**Nivel 1 — Nominal:**
- Summary semántico generado por LLM.
- Protected context fusionado al boundary message.
- Keep window estándar (adaptive_keep_recent).

**Nivel 2 — Degraded:**
- Sin summary LLM (timeout, error, o circuit breaker abierto).
- Keep window EXTENDIDO: los tokens que se habrían usado para summary se usan para retener más mensajes.
- Protected context fusionado al boundary message.
- El boundary message dice: `"[Summary unavailable — extended recent context preserved below]"` seguido del protected context.

**Nivel 3 — Emergency:**
- Circuit breaker abierto Y el budget no permite extended keep.
- Keep window mínimo (4 mensajes).
- Solo IntentAnchor en el boundary message.
- El boundary message dice: `"[Emergency compaction — only intent preserved]"` seguido solo del IntentAnchor formateado.

**Invariante de monotonic information preservation:**
- Nominal preserva: summary + protected context + keep window.
- Degraded preserva: protected context + extended keep window. Estrictamente ≥ Nominal en mensajes reales (pierde summary, gana mensajes originales).
- Emergency preserva: IntentAnchor + keep mínimo. Estrictamente ≤ Degraded. Pero estrictamente ≥ el placeholder actual (que no preserva IntentAnchor).

**Justificación:** El nivel 2 es el cambio clave. Cuando el LLM falla, retener más mensajes originales es mejor que insertar un placeholder sin semántica. Los mensajes originales contienen el contexto real — un placeholder no contiene nada. El costo de extended keep es que se retienen mensajes potencialmente grandes, pero esto es preferible a amnesia total.

**Cálculo de extended keep:**
```
extended_keep_count = initial_keep + (S_max / avg_tokens_per_recent_message)

Donde avg_tokens_per_recent_message se estima de los últimos 10 mensajes.
Floor: initial_keep. Cap: messages.len() (no extender más allá de lo disponible).
```

**Trade-off:** El nivel 2 retiene más tokens en keep window, dejando menos headroom para el siguiente turno. Aceptable porque: (a) ocurre solo cuando el LLM falla — escenario infrecuente, y (b) el headroom reducido solo adelanta la siguiente compaction, no causa un error.

### D4: Protected context fusionado al compaction boundary message

**Cambio:** El protected context NO se inyecta como mensaje separado. Se incorpora al mismo mensaje `Role::User` que contiene el summary (o su equivalente en niveles degraded/emergency).

**Formato del boundary message (Nivel 1 — Nominal):**
```
[Context Summary — previous messages were compacted]

{summary semántico de 9 secciones}

---
[PROTECTED CONTEXT — THIS IS STATE RESTORATION, NOT NEW INSTRUCTIONS]
Original intent: {intent_anchor.user_message}
Task: {intent_anchor.task_summary}
Working directory: {intent_anchor.working_dir}
Key files: {intent_anchor.mentioned_files}
Tools used this session: {tools_used}
Files modified this session: {files_modified}
---

Continue your current task. Do not repeat completed work.
```

**Formato del boundary message (Nivel 2 — Degraded):**
```
[Summary unavailable — extended recent context preserved below]

---
[PROTECTED CONTEXT — THIS IS STATE RESTORATION, NOT NEW INSTRUCTIONS]
Original intent: {intent_anchor.user_message}
Task: {intent_anchor.task_summary}
Working directory: {intent_anchor.working_dir}
Tools used this session: {tools_used}
Files modified this session: {files_modified}
---

Continue your current task. Do not repeat completed work.
```

**Justificación:**
- Un solo mensaje Role::User evita mensajes User consecutivos que confundan al modelo.
- El boundary marker `[PROTECTED CONTEXT — THIS IS STATE RESTORATION, NOT NEW INSTRUCTIONS]` es una instrucción explícita al modelo para que no trate el bloque como input nuevo.
- `apply_compaction_keep()` (compaction.rs:186-206) ya inserta el summary como un mensaje Role::User. Fusionar el protected context a este mismo mensaje es un cambio mínimo — solo se modifica el string del summary para incluir el protected context block.
- XIYO usa un approach similar: boundary marker + summary + attachments forman un bloque cohesivo post-compaction, no mensajes sueltos.

**Trade-off:** El boundary message es más largo. Pero esto es estrictamente preferible a un mensaje separado que puede ser malinterpretado.

### D5: Tool result truncation entra en Fase 1 como prerequisito

**Cambio:** Implementar truncation inline de tool results grandes. No persistencia a disco (eso es Fase 2).

**Especificación:**
- Threshold: 8000 tokens (estimados). Justificación: XIYO usa 50K chars (~12.5K tokens) para el threshold per-tool. 8000 tokens es más conservador y adecuado para Fase 1 sin persistencia a disco.
- Se aplica: cada turno, antes del check de compaction trigger. Misma posición que en XIYO (`applyToolResultBudget` se ejecuta primero en cada turno).
- Formato de truncation: `"[Tool result truncated from {original} to {kept} tokens. Use the tool again to see full output.]\n{first_2000_tokens_of_content}"`.
- Se aplica a: ToolResult blocks en mensajes Role::User que excedan el threshold.
- NO se aplica a: mensajes del turno actual (solo a mensajes anteriores al turno en curso).

**Justificación operacional:**
- Sin esto, un solo `Read` de un archivo largo (e.g. 10K líneas = ~40K tokens) puede consumir 25% del budget de un window de 160K, forzando compaction prematura.
- La compaction semántica nueva no reduce la frecuencia de compaction — solo mejora su calidad. Tool result truncation SÍ reduce la frecuencia porque elimina la inflación que trigger la compaction.
- XIYO ejecuta `applyToolResultBudget()` en cada turno ANTES de cualquier check de compaction. Esto no es accidental — es prerequisito.

**Trade-off:** La truncation pierde información del tool result. Mitigado por: (a) preview de 2000 tokens suficiente para la mayoría de outputs, (b) el agente puede re-invocar la herramienta para el output completo, (c) estrictamente mejor que perder TODO el contexto en compaction prematura.

**Implementación:** Función pura `truncate_large_tool_results(messages: &mut Vec<ChatMessage>, threshold: usize)`. Sin side effects, sin I/O, sin dependencia en provider. Trivial de implementar y testear.

### D6: Scope limpiado — correcciones adyacentes fuera

**Sale de este doc:**
- cancel_token wiring → corrección independiente, sin feature gate compartido.
- diminishing returns adjustment → corrección independiente.
- hook_runner wiring → Fase 2.
- FallbackProvider → Fase 2.

**Se queda en este doc:**
- IntentAnchor
- CompactionSummaryBuilder
- ProtectedContextInjector (fusionado a boundary message)
- TieredCompactor (3 niveles, ambos paths)
- CompactionBudgetCalculator
- ToolResultTruncator
- Feature gate: `semantic-compaction`

**Justificación:** El feature flag `semantic-compaction` debe controlar solo la compaction semántica. Si se desactiva, cancel_token y diminishing returns no deben revertirse. Son mejoras ortogonales con ciclo de vida propio.

### D7: Métricas de validación endurecidas

**Métricas obligatorias (sección 7):**

1. **Compaction utility ratio** — tokens liberados netos / tokens liberados brutos. Alerta si < 0.3.
2. **Summary budget compliance** — % de summaries ≤ S_max. Threshold: > 95%.
3. **Fallback rate** — % de compactions en nivel 2 o 3. Threshold: < 20% para healthy.
4. **Post-compaction halt rate** — % de sesiones que haltan en los 3 turnos siguientes a compaction. Señal de regresión si AUMENTA vs baseline.
5. **Conditional task completion** — % de sesiones que completan tarea después de ≥1 compaction. Señal de mejora si AUMENTA vs baseline.
6. **Duplicate action rate** — tool calls repetidos (mismo tool + mismo input) en los 5 turnos post-compaction / total tool calls en esos turnos. Señal de mejora si DISMINUYE vs baseline.
7. **Compaction latency** — p50 y p99 de la invocación LLM para compaction. Alerta si p99 > 30s.

---

## 5. Mathematical / Operational Model

### Variables

| Variable | Definición | Fuente |
|----------|------------|--------|
| W | Context window del modelo (tokens) | `provider.model_context_window()` |
| U | Utilization factor (0.0-1.0) | Policy, default 0.80 |
| B | Pipeline budget = W × U | Derivado |
| S | Tokens del summary post-compaction | Medido post-LLM |
| S_max | Cap de summary = clamp(B/20, 1000, 4000) | Policy |
| P | Tokens del protected context | Medido post-construcción |
| P_max | Cap de protected context | Policy, default 500 |
| K | Tokens del keep window | Estimado de mensajes retenidos |
| R | Reserve para siguiente turno | max_output_tokens, default 4096 |
| C_reserve | Reserve para compaction = S_max + P_max + R | Derivado |
| T | Tokens totales estimados de mensajes actuales | estimate_message_tokens() |

### Ecuación de Budget

```
INVARIANTE POST-COMPACTION:

  S + P + K + R ≤ B

Donde S ≤ S_max, P ≤ P_max, R = max_output_tokens.
```

### Trigger de Compaction

```
TRIGGER:

  T ≥ B - C_reserve

Equivale a:

  T ≥ B - (S_max + P_max + R)
```

**Valores de ejemplo:**

| Modelo | W | B (U=0.80) | S_max | C_reserve | Trigger |
|--------|---|------------|-------|-----------|---------|
| DeepSeek 64K | 64,000 | 51,200 | 2,560 | 7,156 | 44,044 (86% de B) |
| Claude 128K | 128,000 | 102,400 | 4,000 | 8,596 | 93,804 (92% de B) |
| Claude 200K | 200,000 | 160,000 | 4,000 | 8,596 | 151,404 (95% de B) |

Observar: para windows grandes, el trigger porcentual resultante es ~95% — cercano al 90% actual pero derivado del budget real. Para windows pequeños, es ~86% — significativamente más conservador, correcto dado el menor margen.

### Compaction Utility Ratio

```
utility = (T_freed - T_added) / T_freed

Donde:
  T_freed = T_before - K   (tokens eliminados de mensajes compactados)
  T_added = S + P           (tokens añadidos por summary + protected context)

INVARIANTES:
  utility > 0       → compaction tiene beneficio neto positivo
  utility ≥ 0.3     → compaction es healthy
  utility < 0.3     → warning: compaction de bajo valor
  utility ≤ 0       → ABORT: compaction tiene beneficio negativo, NO aplicar
```

### Verificación Post-Compaction

```
Después de compaction, verificar:

  T_after = S + P + K

  SI T_after + R > B:
    → truncar summary hasta que T_after + R ≤ B
    → loguear warning: "summary truncated post-compaction to fit budget"
    → SI truncar a 0 no alcanza:
      → esto indica que K + P + R > B — el keep window es demasiado grande
      → reducir K hasta cumplir invariante
      → loguear error: "keep window reduced to fit budget"
```

### Failure Conditions

| Condición | Detección | Acción |
|-----------|-----------|--------|
| `utility ≤ 0` antes de aplicar | Calcular T_freed y T_added estimados | NO compactar. Loguear. Dejar que el budget se agote naturalmente y que el arbiter maneje prompt_too_long. |
| `S > S_max` después de LLM | Medir tokens del summary | Truncar summary con marker `[truncated]`. |
| `T_after + R > B` después de aplicar | Verificar post-compaction | Truncar summary, luego reducir keep si necesario. |
| LLM timeout (>30s) | Timeout en invoke | Degradar a nivel 2. Incrementar circuit breaker. |
| LLM error | Error en invoke | Degradar a nivel 2. Incrementar circuit breaker. |
| 3 failures consecutivas | Circuit breaker counter | Degradar a nivel 3. No intentar LLM hasta reset de sesión. |

### Monitoreo Obligatorio

Todo evento de compaction emite un tracing span con estos campos:

| Campo | Tipo | Descripción |
|-------|------|-------------|
| `compaction.level` | string | "nominal", "degraded", "emergency" |
| `compaction.trigger` | string | "proactive", "reactive" |
| `compaction.tokens_before` | u64 | T antes de compaction |
| `compaction.tokens_after` | u64 | T después de compaction |
| `compaction.summary_tokens` | u64 | S real (0 si nivel 2 o 3) |
| `compaction.protected_tokens` | u64 | P real |
| `compaction.keep_messages` | u32 | Mensajes en keep window |
| `compaction.keep_tokens` | u64 | K estimado |
| `compaction.utility_ratio` | f64 | Ratio calculado |
| `compaction.pipeline_budget` | u64 | B usado |
| `compaction.summary_cap` | u64 | S_max para esta compaction |
| `compaction.latency_ms` | u64 | Latencia de invocación LLM (0 si no hubo) |
| `compaction.circuit_breaker` | u32 | Failures consecutivas |
| `compaction.tool_results_truncated` | u32 | Tool results truncados este turno |

---

## 6. Revised Architecture

### Vista General

```
simplified_loop
│
├── IntentAnchor (inmutable, creado al inicio)
│
├── [cada turno, antes de compaction check]
│   └── ToolResultTruncator.truncate(&mut messages, threshold)
│
├── CompactionBudgetCalculator
│   ├── should_compact(T, B) → bool
│   ├── compute_budget(B) → CompactionBudget
│   └── verify_post_compaction(T_after, B) → PostCompactionCheck
│
├── TieredCompactor (orquestador, ambos paths)
│   ├── Nivel 1: CompactionSummaryBuilder → LLM → summary → ProtectedContextInjector → merge
│   ├── Nivel 2: extended keep → ProtectedContextInjector → merge
│   └── Nivel 3: min keep → IntentAnchor only → merge
│
└── [arbiter → recovery]
    └── Compact/ReactiveCompact → TieredCompactor (mismo path)
```

### Componentes

#### IntentAnchor

**Responsabilidad:** Preservar el objetivo original del usuario de forma inmutable.

**Creación:** Una vez, al inicio del loop, a partir del primer mensaje del usuario.

**Contenido:** Mensaje original verbatim, task summary (primeros 500 chars), archivos mencionados, working directory, timestamp.

**Consume:** CompactionSummaryBuilder (como input del prompt), boundary message (como bloque de protected context).

**Invariante:** El intent original nunca desaparece del contexto operativo post-compaction, independientemente del nivel de degradación.

**Boundary:** Read-only después de creación. Función pura de formateo. Sin I/O, sin side effects.

**Nota:** El IntentAnchor es estático — no captura refinamientos mid-session. Esto es una limitación conocida. El summary semántico (sección 6: User Feedback) captura refinamientos cuando el LLM está disponible. En niveles degraded/emergency, los refinamientos se pierden. Fase 2 introduce IntentAnchor dinámico para cerrar este gap.

#### ToolResultTruncator

**Responsabilidad:** Truncar tool results que excedan un threshold configurable, antes de la estimación de tokens para compaction trigger.

**Se ejecuta:** En cada turno, sobre mensajes previos al turno actual. Muta in-place.

**Threshold:** Configurable (default 8000 tokens estimados). Preserva 2000 tokens de preview.

**Invariante:** Un tool result truncado siempre indica su tamaño original y sugiere re-invocación.

**Boundary:** Función pura sobre Vec<ChatMessage>. Sin I/O. Sin dependencia en provider.

**Failure domain:** Aislado. Si falla o se desactiva, el sistema se comporta como hoy — sin truncation, compaction se triggerea más frecuentemente.

**Extension point:** Fase 2 reemplaza truncation inline con persistencia a disco (modelo XIYO).

#### CompactionBudgetCalculator

**Responsabilidad:** Centralizar toda la lógica de budget de compaction.

**Funciones:**
- `should_compact(estimated_tokens, pipeline_budget) → bool`: trigger basado en reserve.
- `compute_budget(pipeline_budget) → CompactionBudget`: calcula S_max, P_max, keep count, reserve.
- `verify_post_compaction(tokens_after, pipeline_budget) → PostCompactionCheck`: verifica invariante post-compaction, retorna si se necesita truncation de summary.

**CompactionBudget (struct):**
- `trigger_threshold: usize` — umbral para activar compaction.
- `max_summary_tokens: usize` — S_max.
- `max_protected_tokens: usize` — P_max.
- `keep_count: usize` — número de mensajes a retener.
- `reserve: usize` — R.
- `extended_keep_count: usize` — keep para nivel degraded.

**Invariante:** `S_max + P_max + K_estimated + R ≤ B` se verifica antes de compaction. Si no se cumple (mensajes recientes demasiado grandes), el keep count se reduce.

**Boundary:** Lógica pura. Sin I/O. Sin provider. Testeable con valores numéricos.

**Por qué es un componente separado:** Sin esto, la lógica de budget estaría dispersa entre trigger check, summary builder, injector y verificación post-compaction. Centralizar hace el budget model testeable y auditable como unidad.

#### CompactionSummaryBuilder

**Responsabilidad:** Construir el prompt semántico de 9 secciones y formatear la respuesta.

**Inputs:** Mensajes a compactar, IntentAnchor, S_max (del budget calculator).

**Outputs:** Prompt string para el LLM. Parsing del response: string de summary.

**Estructura del prompt:** 9 secciones (alineadas con patrón verificado internamente en XIYO):
1. Primary Request and Intent — citas directas del usuario.
2. Key Technical Context — tecnologías, frameworks, patrones.
3. Files and Code — paths exactos de archivos examinados/modificados.
4. Errors and Fixes — errores y resoluciones verbatim.
5. Decisions Made — decisiones clave con razonamiento.
6. User Feedback — correcciones, refinamientos, cambios de dirección.
7. Pending Tasks — trabajo solicitado no completado.
8. Current State — qué se estaba haciendo antes de compaction.
9. Next Step — siguiente acción con cita directa del request más reciente.

**Validación de output:** Si tokens(summary) > S_max, truncar con marker. Loguear.

**Boundary:** Construye strings. No invoca al LLM (eso es responsabilidad del TieredCompactor).

#### ProtectedContextInjector

**Responsabilidad:** Construir el bloque de protected context para fusión al boundary message.

**Inputs:** IntentAnchor, tools usados, files modificados.

**Output:** String formateado con boundary markers.

**NO produce mensajes.** Produce un bloque de texto que el TieredCompactor fusiona al boundary message del ContextCompactor existente.

**Boundary:** Función pura. Sin I/O. Sin side effects.

#### TieredCompactor

**Responsabilidad:** Orquestar el proceso completo de compaction con degradación escalonada.

**Inputs:** Messages (mut), IntentAnchor, provider, CompactionBudget.

**Flow:**
1. Verificar circuit breaker.
   - Si abierto → nivel 3 (emergency).
2. Si cerrado → intentar nivel 1 (nominal).
   - Construir prompt via CompactionSummaryBuilder.
   - Invocar provider con timeout 30s, temperature 0, max_tokens = S_max.
   - Si éxito → calcular utility ratio.
     - Si utility ≤ 0 → ABORT compaction, loguear, retornar sin compactar.
     - Si utility > 0 → aplicar compaction con summary + protected context.
   - Si fallo → incrementar circuit breaker, ir a nivel 2.
3. Nivel 2 (degraded).
   - Calcular extended_keep_count del budget.
   - Aplicar compaction con boundary message (sin summary, con protected context) + extended keep.
4. Nivel 3 (emergency).
   - Aplicar compaction con boundary message (sin summary, solo IntentAnchor) + keep mínimo.
5. Post-compaction: verificar budget via CompactionBudgetCalculator.verify_post_compaction().
   - Si viola invariante → truncar boundary message hasta cumplir.

**Output:** `CompactionResult { level, utility_ratio, summary_tokens, latency_ms }`.

**Se usa en:** Compaction proactiva (reemplaza c.apply_compaction directo) Y compaction reactiva (reemplaza apply_recovery para Compact/ReactiveCompact).

**Failure domains:**
- LLM failure: aislado, degrada a nivel 2.
- Budget violation: detectada post-compaction, auto-corregida.
- Negative utility: detectada pre-apply, aborta compaction.
- Circuit breaker open: degrada a nivel 3, sin intentos LLM.

**Invariantes:**
- Post-compaction SIEMPRE cumple S + P + K + R ≤ B (verificado).
- Tool pair safety preservada (delega a safe_keep_boundary_n() existente).
- Cada nivel preserva ≥ información que el nivel inferior.
- El sistema nunca es peor que hoy: nivel 3 (emergency) preserva IntentAnchor, que es más que el placeholder actual.

### Diagrama de Interacción

```
TURNO N del simplified_loop:

1. ToolResultTruncator.truncate(&mut messages, 8000)
   └─ Trunca tool results antiguos > 8000 tokens

2. T = estimate_message_tokens(&messages)
   B = pipeline_budget  // o ctx_budget × 0.80 como fallback
   budget = CompactionBudgetCalculator.compute_budget(B)

3. SI T ≥ budget.trigger_threshold:
   └─ TieredCompactor.compact(&mut messages, &intent_anchor, provider, &budget)
      │
      ├─ [circuit_breaker closed]
      │   ├─ prompt = SummaryBuilder.build_prompt(messages, intent_anchor, budget.max_summary_tokens)
      │   ├─ summary = provider.invoke(prompt, max_tokens=budget.max_summary_tokens, timeout=30s)
      │   │
      │   ├─ [success] → utility = calculate_utility(messages, summary, protected_context, keep)
      │   │   ├─ [utility > 0] → apply compaction (summary + protected context + keep)
      │   │   └─ [utility ≤ 0] → ABORT, log, return without compacting
      │   │
      │   └─ [failure] → increment circuit_breaker
      │       └─ Nivel 2: apply compaction (extended keep + protected context, no summary)
      │
      └─ [circuit_breaker open]
          └─ Nivel 3: apply compaction (min keep + IntentAnchor only)

4. Verificar post-compaction: T_after + R ≤ B
   └─ Si viola → truncar boundary message

5. Continuar loop normal (model call, streaming, tool execution)
```

### Extension Points

| Componente | Extension (Fase 2+) |
|------------|---------------------|
| ToolResultTruncator | Persistencia a disco + punteros XML (modelo XIYO) |
| CompactionSummaryBuilder | Partial compaction prompt (XIYO `PARTIAL_COMPACT_PROMPT`) |
| CompactionBudgetCalculator | Multi-tier budget (snip + microcompact) |
| ProtectedContextInjector | Re-inyectar tool schemas, MCP, plan context |
| TieredCompactor | Niveles adicionales (snip, microcompact, context collapse) |
| IntentAnchor | Actualización dinámica con feedback mid-session |

---

## 7. Revised Rollout and Validation Model

### Feature Gating

Feature flag: `semantic-compaction`. Controla:
- Uso de TieredCompactor (vs placeholder directo).
- Uso de ToolResultTruncator.
- Uso de IntentAnchor y protected context injection.

Cuando desactivado: el sistema se comporta exactamente como hoy. `apply_compaction()` con placeholder.

### Acceptance Criteria (obligatorios para merge)

| # | Criterio | Verificación |
|---|----------|-------------|
| AC1 | Summary nominal contiene ≥7 de 9 secciones | Test integración: compactar 30 mensajes con tool calls |
| AC2 | IntentAnchor presente en boundary message para los 3 niveles | Test unitario por nivel |
| AC3 | S + P + K + R ≤ B post-compaction | Assertion en TieredCompactor + test unitario |
| AC4 | Utility ratio > 0 para toda compaction aplicada | Assertion en TieredCompactor (abort si ≤ 0) |
| AC5 | Tool pair safety preservada | Tests existentes en compaction.rs siguen pasando |
| AC6 | Nivel 2 (degraded) funciona con extended keep | Test con mock provider que falla |
| AC7 | Nivel 3 (emergency) con circuit breaker abierto | Test con 3 failures consecutivas |
| AC8 | Ambos paths (proactivo + reactivo) usan TieredCompactor | Test integración |
| AC9 | Tool results > 8000 tokens truncados | Test unitario |
| AC10 | Budget model correcto para DeepSeek 64K | Test unitario con pipeline_budget = 51200 |

### Métricas Online

| Métrica | Definición | Threshold healthy | Señal rollback |
|---------|------------|-------------------|----------------|
| Compaction utility ratio | (T_freed - T_added) / T_freed | p10 > 0.3 | p10 < 0.1 sostenido 24h |
| Summary budget compliance | % summaries ≤ S_max | > 95% | < 80% |
| Fallback rate | % compactions nivel 2 + 3 / total | < 20% | > 40% sostenido 24h |
| Post-compaction halt rate | % sesiones que haltan en 3 turnos post-compaction | ≤ baseline | Aumento > 10% vs baseline |
| Conditional task completion | % sesiones que completan tarea tras ≥1 compaction | ≥ baseline | Disminución > 5% vs baseline |
| Duplicate action rate | Tool calls repetidos en 5 turnos post-compaction / total | ≤ baseline | Aumento > 15% vs baseline |
| Compaction latency p99 | ms de invocación LLM | < 30s | > 45s |

### Señales de Rollback Inmediato

Desactivar feature flag si cualquiera de estas se cumple:
- Regresión en tool pair safety (errores 400 por orphaned ToolResult).
- Post-compaction halt rate aumenta > 10% respecto a baseline.
- Compaction latency p99 > 45s sostenido por 4h.
- Errores no-recoverable nuevos atribuibles a compaction en logs.

### Canary Strategy

**Fase canary:** 10% de sesiones durante 7 días. Control: 90% con placeholder actual.

**Criterios para full rollout:**
- Conditional task completion: delta ≥ +5% (señal de que el sistema mejora).
- Post-compaction halt rate: delta ≤ 0% (no empeorar).
- Compaction latency p50 < 10s, p99 < 30s.
- Fallback rate < 30% (el LLM funciona en la mayoría de compactions).
- Cero regresiones en tool pair safety.

**Criterios para pausa y diagnóstico:**
- Delta de conditional task completion < +2% — efecto demasiado pequeño para justificar complejidad.
- Fallback rate > 50% — problema sistémico con la invocación LLM.

---

## 8. Final Recommendation

### Debe cambiar antes de implementar (ya incorporado en este doc)

1. Budget model explícito con ecuación verificable. ✓ Sección 5.
2. Cubrir ambos paths de compaction. ✓ Sección 6, TieredCompactor.
3. Fusionar protected context al boundary message. ✓ Decisión D4.
4. 3 niveles de degradación. ✓ Decisión D3.
5. Tool result truncation como prerequisito. ✓ Decisión D5.
6. Trigger derivado del budget. ✓ Decisión D1.

### Puede implementarse ya (orden recomendado)

1. **IntentAnchor** — struct inmutable, riesgo cero, cero dependencias.
2. **ToolResultTruncator** — función pura, independiente, reduce frecuencia de compaction.
3. **CompactionBudgetCalculator** — lógica pura, informa todo lo demás.
4. **CompactionSummaryBuilder** — prompt de 9 secciones, independiente del loop.
5. **ProtectedContextInjector** — formateo puro, independiente.
6. **TieredCompactor** — orquestador, depende de 1-5.
7. **Integración en simplified_loop** — compaction proactiva + reactiva.

Pasos 1-5 son paralelizables y testeables independientemente.
Paso 6 integra 1-5.
Paso 7 modifica el loop.

### Fase 1.5 (concurrente pero feature flag separado)

- cancel_token wiring en dispatch.rs.
- Diminishing returns: threshold a 200 tokens, 3 rounds.

### Fase 2

- Tool result persistencia a disco (reemplaza truncation inline).
- FallbackProvider real.
- Hook runner wiring.
- IntentAnchor dinámico (actualización mid-session).
- Multi-tier compaction (snip, microcompact).
- Semantic stagnation detection.
- Goal verification.

### Claims que deben rebajarse

| Claim | De | A |
|-------|----|---|
| "42% reducción en task success rate" | Hecho cuantitativo | Hipótesis: evidencia preliminar sugiere degradación significativa |
| "Context rot es propiedad estructural del transformer" | Hecho | Inferencia plausible basada en benchmarks no reproducidos internamente |
| "5-15s de latencia por compaction" | Estimación | Hipótesis sin data de Halcon; depende de modelo, tamaño de input, infraestructura |
| "1-3 invocaciones por sesión larga" | Estimación | Hipótesis sin data de frecuencia de compaction real |
| "El prompt de 9 secciones preserva lo necesario" | Hecho | Inferencia basada en referencia interna (XIYO); la calidad depende del modelo y contenido |
| "La proporción 1/20 del budget para summary es correcta" | Decisión justificada | Hipótesis operacional a validar en canary |

---

## Architectural Invariants (consolidadas)

1. **El intent original del usuario nunca desaparece del contexto operativo.** El IntentAnchor se incluye en el boundary message después de toda compaction, en los 3 niveles de degradación.

2. **Toda compaction obedece la ecuación de budget.** `S + P + K + R ≤ B` se verifica post-compaction. Violaciones se auto-corrigen truncando summary, luego reduciendo keep.

3. **La compaction nunca tiene beneficio neto negativo.** Si `utility ≤ 0`, la compaction se aborta.

4. **La degradación es monótonamente decreciente.** Nivel 1 ≥ Nivel 2 ≥ Nivel 3 ≥ placeholder actual. Cada nivel preserva al menos la información del nivel inferior.

5. **Tool pair safety se preserva.** `safe_keep_boundary_n()` opera sobre el keep window independientemente del nivel de compaction.

6. **Ambos paths de compaction usan el mismo subsistema.** Proactivo y reactivo pasan por TieredCompactor. No hay path que produzca placeholder sin pasar por degradación escalonada.

7. **El boundary entre contexto protegido y efímero es explícito.** Protected = IntentAnchor + tools + files (sobrevive compaction). Efímero = todo lo demás (se condensa o descarta).

8. **El sistema con feature flag desactivado es idéntico al actual.** Zero regression risk en rollback.
