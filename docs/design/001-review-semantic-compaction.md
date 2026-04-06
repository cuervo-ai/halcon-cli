# Design Review: Compaction Semántica y Preservación de Intent — Fase 1

**Revisor:** Principal Systems Architect
**Fecha:** 2026-04-03
**Documento revisado:** `001-semantic-compaction-intent-preservation.md`
**Veredicto:** Parcialmente viable — requiere correcciones estructurales antes de implementación

---

## 1. Executive Verdict

El diseño identifica correctamente la causa raíz y propone componentes razonables. Sin embargo, tiene cinco problemas estructurales que deben corregirse antes de implementar:

1. **Ausencia de modelo de budget post-compaction.** No existe una ecuación que relacione summary + protected context + keep window + reserve. Sin esto, la compaction puede producir un estado post-compaction que exceda el budget o deje un margen insuficiente para el siguiente turno.

2. **El fallback a placeholder viola monotonic information preservation.** El diseño dice "nunca peor que el estado actual", pero esto es insuficiente como invariante de diseño. Un sistema con IntentAnchor + ProtectedContextInjector que cae a placeholder pierde el resumen pero mantiene el overhead de re-inyección — consumiendo tokens sin beneficio proporcional.

3. **La re-inyección como `Role::User` contamina la semántica conversacional.** El agente puede interpretar el contexto re-inyectado como una instrucción nueva del usuario, no como restauración de estado. Esto introduce un failure mode no abordado.

4. **Dos paths de compaction existen, pero el diseño solo aborda uno.** La compaction proactiva (simplified_loop.rs:158-163) y la compaction reactiva (apply_recovery, línea 272-275) son paths distintos. El diseño solo cubre el proactivo. La compaction reactiva — activada por el arbiter ante `prompt_too_long` — seguiría usando placeholder puro.

5. **Tool result budgeting excluido sin justificación operacional suficiente.** Los tool outputs grandes son la causa primaria de context inflation que trigger la compaction. Resolver la compaction sin resolver la inflación que la causa es tratar el síntoma sin mitigar el amplificador.

El diseño es implementable con las correcciones descritas en este review. Ninguna corrección requiere re-arquitectura — son refinamientos en boundaries, budget model y failure modes.

---

## 2. Verified vs Unverified Claims

| # | Claim | Status | Source | Consecuencia para diseño |
|---|-------|--------|--------|--------------------------|
| 1 | simplified_loop es el único runtime activo | **Verified** | dispatch.rs:32-59, GDEM feature-gated | Base firme. No se necesita compatibilidad con GDEM. |
| 2 | Compaction usa placeholder `"[Context compacted]"` sin LLM | **Verified** | simplified_loop.rs:161, apply_recovery:274 | Causa raíz confirmada. Dos paths (proactivo + reactivo) usan placeholder. |
| 3 | `compaction_prompt()` existe pero nunca se invoca | **Verified** | compaction.rs:84-131, sin call sites en el loop | El prompt existente es de 4 secciones, no 9. El diseño propone reemplazarlo, no usarlo. |
| 4 | cancel_token siempre es None | **Verified** | dispatch.rs:55 | Corrección trivial confirmada. |
| 5 | hook_runner siempre es None | **Verified** | dispatch.rs:33-34 | Fuera de scope Fase 1. Correcto. |
| 6 | FallbackProvider es stub | **Verified** | simplified_loop.rs:283 | Fuera de scope Fase 1. Correcto. |
| 7 | DIMINISHING_RETURNS_THRESHOLD = 500, 2 rounds consecutivos | **Verified** | simplified_loop.rs:41, BudgetTracker:89 | Pero el check es `d < 500 && prev_delta < 500`, o sea 2 rounds consecutivos. El diseño lo describe correctamente. |
| 8 | max_compact_attempts = 2 | **Verified** | feedback_arbiter.rs:116 | Se mantiene en diseño. Correcto. |
| 9 | evidence_verified: true hardcoded | **Verified** | simplified_loop.rs:125 | Fuera de scope Fase 1. |
| 10 | XIYO usa forked agent con prompt de 9 secciones | **Verified internamente** | xiyo/services/compact/prompt.ts:61-127 | Referencia interna, no pública. Las 9 secciones están confirmadas en el source de XIYO local. |
| 11 | XIYO re-inyecta 6 tipos post-compact | **Verified internamente** | xiyo/services/compact/compact.ts:541-585 | Referencia interna. Los 6 tipos están confirmados. |
| 12 | XIYO tiene circuit breaker de 3 failures | **Verified internamente** | xiyo/services/compact/autoCompact.ts:70 con comentario BQ: 1,279 sessions con 50+ failures | El valor 3 está respaldado por datos reales de XIYO (250K API calls/día desperdiciadas). |
| 13 | XIYO persiste tool results a disco | **Verified internamente** | xiyo/utils/toolResultStorage.ts, query.ts:99 `applyToolResultBudget` | Se llama en cada turno del query loop, antes de compaction. |
| 14 | "42% reducción en task success rate" por context rot | **Weak** | Referencia a paper sin cita completa | Hipótesis respaldada por preprint no verificable. No usar como hecho cuantitativo. |
| 15 | "Context rot es propiedad estructural del transformer" (Chroma) | **Weak** | Referencia a paper/benchmark sin cita | Claim plausible pero presentado con más certeza de la que la evidencia soporta. |
| 16 | Threshold de 85% es correcto | **Unsupported** | Decisión de diseño sin justificación cuantitativa | No hay modelo que demuestre que 85% deja margen suficiente para summary + protected context + reserve. |
| 17 | Cap de 2000 tokens para summary es adecuado | **Unsupported** | Decisión de diseño arbitraria | XIYO no tiene cap explícito de tokens en el output del summary. 2000 es razonable pero no está justificado. |
| 18 | 30 segundos es timeout adecuado para compaction | **Inferred** | No hay data de latencia de invocación en Halcon | XIYO no tiene timeout explícito en autocompact. 30s es razonable para un modelo con ~50K tokens input. |
| 19 | `needs_compaction_with_budget()` con 60% threshold ya existe | **Verified** | compaction.rs:70-81 | El diseño ignora completamente esta función. El loop usa raw threshold de 90% vs ctx_budget, no esta función budget-aware. |

---

## 3. Critical Gaps

### Gap 1: Ausencia de modelo de budget post-compaction [CRÍTICA]

El diseño no define una ecuación de budget. Dice "2000 tokens para summary" y "200-500 tokens para protected context" y "threshold 85%", pero no formaliza la relación:

```
tokens(summary) + tokens(protected_context) + tokens(keep_window) + reserve ≤ available_budget
```

Sin esto:
- No se puede verificar que el estado post-compaction cabe en el window.
- No se puede dimensionar el summary proporcionalmente al budget.
- No se puede detectar compaction con beneficio neto negativo (el overhead de summary + re-inyección supera lo liberado).

### Gap 2: Dos paths de compaction, solo uno abordado [CRÍTICA]

El diseño solo modifica la compaction proactiva (simplified_loop.rs:158-163). Pero la compaction reactiva (apply_recovery, líneas 272-275) es activada por el arbiter ante `RecoveryAction::Compact` y `RecoveryAction::ReactiveCompact`. Ambos paths usan placeholder:

- Proactivo: `c.apply_compaction(&mut messages, "[Context compacted proactively]")`
- Reactivo: `c.apply_compaction(msgs, "[Context compacted]")`

Si solo se corrige el proactivo, las compactions reactivas (prompt_too_long, reactive overflow) siguen destruyendo contexto. Es un agujero en la cobertura.

### Gap 3: Re-inyección como Role::User [ALTA]

El `apply_compaction_keep()` existente (compaction.rs:186-206) inserta el summary como `Role::User`. El diseño propone que el ProtectedContextInjector también produzca mensajes `Role::User`. Esto significa que post-compaction, el agente ve:

```
[User]: [Context Summary — previous messages were compacted] ...summary...
[User/Assistant]: ...mensajes recientes...
[User]: [POST-COMPACTION CONTEXT RESTORATION] ...intent + tools + files...
```

Dos mensajes de `Role::User` consecutivos (si el último mensaje reciente es de User) pueden ser interpretados por el modelo como instrucciones nuevas, no como restauración de estado. Esto puede provocar:
- El agente "obedece" la restauración como si fuera un nuevo request.
- El agente responde al bloque de restauración en vez de continuar la tarea.

### Gap 4: Fallback escalonado ausente [ALTA]

El diseño define un fallback binario: semantic summary OR placeholder. No hay nivel intermedio. Esto viola progressive degradation:

- Nivel completo: semantic summary + protected context → máxima continuidad.
- Nivel fallback: placeholder `"[Context compacted — summary unavailable]"` + protected context → el IntentAnchor y tools se re-inyectan, pero el historial se pierde.

Falta un nivel intermedio: protected context only (sin summary, sin placeholder genérico). Si el LLM falla, re-inyectar solo el IntentAnchor + tools + files + keep window da más continuidad que un placeholder + re-inyección.

### Gap 5: Tool result budgeting excluido sin mitigación [ALTA]

XIYO llama `applyToolResultBudget()` en **cada turno**, antes de cualquier compaction. Esto persiste tool results > threshold a disco y los reemplaza con un puntero XML. Sin esto, un solo `Read` de un archivo de 10K líneas puede consumir 40K tokens — forzando compaction prematura.

El diseño reconoce el gap (tabla de gap analysis, Severidad ALTA) pero lo pospone a Fase 2 sin mitigación. El resultado: la compaction semántica nueva se triggerea con la misma frecuencia innecesaria que la compaction placeholder actual, porque la causa de inflación (tool results grandes) no se aborda.

### Gap 6: `needs_compaction_with_budget()` ignorada [MEDIA]

Existe una función budget-aware (`compaction.rs:70-81`) con threshold de 60% del pipeline budget que ya maneja correctamente providers con context windows pequeños (DeepSeek 64K). El simplified_loop no la usa — usa raw `ctx_budget * 0.90`. El diseño propone cambiar a 85% pero no menciona esta función existente. La decisión correcta es usar `needs_compaction_with_budget()` en vez de reinventar el threshold.

### Gap 7: Métricas de validación débiles [MEDIA]

El diseño lista "reducción medible en oscilación" y "el agente mantiene dirección coherente" como señales de éxito, pero no define cómo medirlas. Sin métricas cuantitativas:
- No se puede saber si la compaction semántica es mejor que placeholder.
- No se puede comparar A/B.
- No se puede detectar regresión.

### Gap 8: cancel_token y diminishing returns contaminan el scope [BAJA]

El cancel_token y el ajuste de diminishing returns no están relacionados con compaction semántica. Son correcciones adyacentes legítimas pero mezclan el alcance del diseño. Si la compaction semántica falla en rollout, estas correcciones no deben revertirse. Si se revierten junto con el feature flag, se pierden mejoras ortogonales.

---

## 4. Revised Design Decisions

### D1: El threshold deja de ser constante fija y se deriva del budget

**Decisión:** Reemplazar `COMPACTION_THRESHOLD: f64 = 0.90` (y la propuesta de 0.85) con el uso de `needs_compaction_with_budget()` ya existente, modificando su threshold para acomodar el overhead de compaction semántica.

**Justificación:** La función budget-aware ya existe (`compaction.rs:70-81`) y resuelve correctamente providers con context windows pequeños. El threshold fijo de 90% ignora el tamaño real del window. El threshold propuesto de 85% es igualmente arbitrario.

**Formulación:** El trigger de compaction debe activarse cuando:

```
estimated_tokens(messages) ≥ pipeline_budget - compaction_reserve
```

Donde `compaction_reserve` = `max_summary_tokens` + `max_protected_context_tokens` + `min_next_turn_reserve`. Esto garantiza que después de compaction, el estado resultante tiene espacio suficiente para el summary, el contexto protegido y al menos un turno completo.

Con valores iniciales: `compaction_reserve = 2000 + 500 + 4096 = 6596`. Para un pipeline_budget de 51K (DeepSeek 64K * 0.8): trigger a 44.4K tokens ≈ 87%. Para 160K (200K * 0.8): trigger a 153.4K ≈ 96%. Esto es correcto: windows más grandes pueden esperar más.

La fórmula debe ser configurable vía `CompactionConfig` con un campo `compaction_reserve_tokens: u32`.

### D2: El cap de summary es proporcional al budget, no fijo

**Decisión:** `max_summary_tokens = min(pipeline_budget / 20, 4000)`. Floor de 1000 tokens.

**Justificación:** Un cap fijo de 2000 es insuficiente para sesiones largas con mucho contexto técnico en windows grandes (200K), y excesivo para windows pequeños (64K). La proporción 1/20 del pipeline budget equilibra:
- DeepSeek 64K: `51200 / 20 = 2560` → cap 2560 tokens.
- Claude 200K: `160000 / 20 = 8000` → cap capped a 4000 tokens.
- Window mínimo 32K: `25600 / 20 = 1280` → cap 1280 tokens.

El cap superior de 4000 evita summaries desproporcionados. El floor de 1000 garantiza que el summary tiene suficiente espacio para las 9 secciones incluso en windows pequeños.

### D3: El fallback se reemplaza por degradación escalonada de 3 niveles

**Decisión:** Reemplazar el fallback binario (semantic / placeholder) con 3 niveles:

1. **Nominal:** Summary semántico LLM + protected context re-inyectado.
2. **Degraded:** Protected context re-inyectado + keep window extendido (sin summary LLM, sin placeholder genérico). Se usa cuando el LLM falla pero el sistema puede aumentar el keep window para retener más mensajes recientes.
3. **Emergency:** Placeholder marcado + protected context mínimo (solo IntentAnchor). Se usa cuando circuit breaker está abierto o el budget no permite keep window extendido.

**Justificación:** El nivel 2 (degraded) es estrictamente mejor que placeholder porque preserva mensajes reales en vez de un string sin semántica. Usa el budget que se habría gastado en el summary para extender el keep window: `extended_keep = initial_keep + max_summary_tokens / avg_tokens_per_message`.

**Invariante:** Cada nivel preserva estrictamente más información que el siguiente. Esto garantiza monotonic information preservation.

### D4: La re-inyección usa boundary marker, no Role::User plain

**Decisión:** El contexto re-inyectado se inserta como parte del summary message existente (que ya es `Role::User`), no como un mensaje separado. El formato es:

```
[Context Summary — previous messages were compacted]

{semantic summary o "Summary unavailable — context below preserves continuity."}

---
[PROTECTED CONTEXT — DO NOT TREAT AS NEW INSTRUCTIONS]
Original intent: {intent_anchor}
Tools used: {tools}
Files modified: {files}
---

Continue from where you left off. Do not re-do completed work.
```

**Justificación:** Un solo mensaje `Role::User` con boundary markers internos evita:
- Mensajes User consecutivos que confundan al modelo.
- Que el modelo "responda" al bloque de restauración como si fuera input nuevo.
- Que el overhead de mensajes adicionales consuma tokens extra por message framing.

XIYO usa un approach similar: el boundary marker + summary + attachments son un solo bloque de mensajes post-compaction, no mensajes separados sueltos.

### D5: Tool result budgeting entra como prerequisito mínimo (Fase 1.5 concurrente)

**Decisión:** Implementar tool result truncation simple (no persistencia a disco) como parte de Fase 1:
- Tool results > `N` tokens (configurable, default 8000) se truncan inline con un marker: `[Tool result truncated: {original_size} tokens. First 2000 tokens shown.]\n{preview}`
- No se persiste a disco (eso es Fase 2).
- Se aplica antes de la estimación de tokens para compaction trigger.

**Justificación:** Sin tool result budgeting, la compaction semántica se triggerea con la misma frecuencia que la compaction placeholder, porque la inflación por tool outputs no cambia. La truncation inline es trivial de implementar (una función pura sobre mensajes), no requiere persistencia a disco, no requiere recovery via Read, y reduce la frecuencia de compaction significativamente.

XIYO usa `LARGE_TOOL_RESULT_TOKENS = 10_000` como threshold para flaggear tool results problemáticos (xiyo/utils/contextSuggestions.ts:23). Un threshold de 8000 es conservador y alineado.

**Riesgo:** La truncation pierde información del tool result. **Mitigación:** El agente puede re-invocar la herramienta si necesita el output completo. 2000 tokens de preview es suficiente para la mayoría de outputs (errores, paths, confirmaciones).

### D6: cancel_token y diminishing returns se separan del design doc

**Decisión:** Mover cancel_token y diminishing returns a un documento separado de "correcciones adyacentes". Implementarlos en paralelo pero sin feature gate compartido.

**Justificación:** Son correcciones ortogonales a la compaction semántica. El feature flag `semantic-compaction` debe controlar solo la compaction semántica. Si la compaction falla en rollout y se desactiva, el cancel_token y el ajuste de diminishing returns no deben revertirse — son mejoras independientes.

**El core de este design doc es:** IntentAnchor + CompactionSummaryBuilder + ProtectedContextInjector + TieredCompactor + tool result truncation.

### D7: Ambos paths de compaction deben usar TieredCompactor

**Decisión:** El TieredCompactor se usa tanto en compaction proactiva como en compaction reactiva. El `apply_recovery` en simplified_loop.rs debe delegar al TieredCompactor en vez de llamar directamente a `c.apply_compaction()`.

**Justificación:** Si solo se corrige la compaction proactiva, la compaction reactiva (prompt_too_long) sigue produciendo placeholders destructivos. Es el mismo failure mode.

**Implicación:** `apply_recovery` necesita acceso al TieredCompactor, al IntentAnchor y al provider. Esto requiere que estos objetos estén accesibles desde el contexto del loop, no solo en el path proactivo.

---

## 5. Mathematical / Operational Model

### Budget Equation

```
S + P + K + R ≤ B

Donde:
  B = pipeline_budget (context_window × utilization_factor, típicamente 0.80)
  S = tokens(summary)           — cap: min(B/20, 4000), floor: 1000
  P = tokens(protected_context) — estimado: 200-500 tokens (IntentAnchor + tools + files)
  K = tokens(keep_window)       — adaptive_keep_recent(B) mensajes × avg_tokens_per_message
  R = reserve_for_next_turn     — mínimo: max_output_tokens (default 4096)
```

### Trigger Condition

```
Compaction se activa cuando:
  estimated_tokens(messages) ≥ B - (S_max + P_max + R)

Donde S_max y P_max son los caps configurados, no los valores reales.
Esto garantiza que SIEMPRE hay espacio para el peor caso de summary + protected context + response.
```

### Compaction Utility Ratio

```
utility = (tokens_freed - tokens_added) / tokens_freed

Donde:
  tokens_freed = tokens_before_compaction - tokens(keep_window)
  tokens_added = tokens(summary) + tokens(protected_context)

Si utility < 0.3, la compaction tiene beneficio neto bajo — loguear warning.
Si utility < 0, la compaction tiene beneficio neto NEGATIVO — ABORT, no compactar.
```

### Failure Conditions

| Condición | Detección | Acción |
|-----------|-----------|--------|
| `S + P + K + R > B` post-compaction | Verificar post-compaction | Truncar summary hasta cumplir budget |
| `utility < 0` | Calcular antes de aplicar | No compactar, loguear, dejar que el budget se agote naturalmente |
| `utility < 0.3` | Calcular después de aplicar | Warning en logs, considerar tool result truncation más agresiva |
| LLM timeout (>30s) | Timeout en invoke | Degradar a nivel 2 (protected context + extended keep) |
| LLM error | Error en invoke | Incrementar circuit breaker, degradar a nivel 2 |
| 3 LLM failures consecutivas | Circuit breaker | Degradar a nivel 3 (emergency), no intentar más |

### Monitoreo

| Variable | Cómo se observa | Alerta |
|----------|-----------------|--------|
| `B` (pipeline budget) | Log al inicio del loop | — |
| `S` (summary tokens) | Log post-compaction | > S_max |
| `P` (protected context tokens) | Log post-compaction | > 800 tokens |
| `K` (keep window tokens) | Log post-compaction | — |
| `R` (reserve) | Derivado | < max_output_tokens |
| `utility` | Calculado post-compaction | < 0.3 |
| Fallback rate | Counter por sesión | > 20% de compactions |
| Circuit breaker state | Log en cada trigger | Open |
| Compaction latency | Span timing | p99 > 25s |

---

## 6. Revised Architecture

### Componentes

#### Intent Anchor
**Sin cambios respecto al diseño original.** Estructura inmutable, creada al inicio, contiene mensaje original + task summary + files mencionados + working dir. Se consume por CompactionSummaryBuilder y por el mensaje post-compaction.

#### Tool Result Truncator (nuevo, Fase 1)
**Responsabilidad:** Truncar tool results que excedan un threshold configurable, antes de la estimación de tokens.

**Se ejecuta:** En cada turno, antes del check de compaction trigger. Muta mensajes in-place.

**Boundary:** Función pura sobre `Vec<ChatMessage>`. Sin side effects, sin I/O, sin dependencia en provider. Threshold configurable (default 8000 tokens).

**Invariante:** Un tool result truncado preserva al menos 2000 tokens de preview. El marker de truncation indica el tamaño original para que el agente pueda re-invocar si necesita el output completo.

**Relación:** Reduce frecuencia de compaction trigger. Opera independientemente del TieredCompactor.

#### Compaction Summary Builder
**Cambios respecto al diseño original:**
- `max_summary_tokens` se calcula como `min(pipeline_budget / 20, 4000)` con floor de 1000. No es fijo.
- El prompt incluye el cap dinámico en la instrucción.
- El output se valida: si excede el cap, se trunca con un marker `[Summary truncated to fit budget]`.

#### Compaction Budget Calculator (nuevo)
**Responsabilidad:** Calcular el trigger threshold, el cap de summary, el keep window y el reserve. Verificar utility ratio post-compaction.

**Inputs:** `pipeline_budget`, `max_output_tokens`, `CompactionConfig`.

**Outputs:** `CompactionBudget { trigger_threshold, max_summary_tokens, max_protected_tokens, keep_count, reserve, utility_ratio }`.

**Invariante:** `S_max + P_max + K_estimated + R ≤ B` siempre se cumple pre-compaction. Si post-compaction el invariante se viola, truncar summary.

**Por qué existe:** Centraliza la lógica de budget que de otra forma estaría dispersa entre el trigger check, el summary builder y el injector. Hace el budget model explícito y testeable.

#### Protected Context Injector
**Cambios respecto al diseño original:**
- No produce mensajes separados. Produce un bloque de texto que se incorpora al mensaje de compaction boundary (el mismo mensaje que contiene el summary).
- Usa boundary markers (`---[PROTECTED CONTEXT]---`) para separar visualmente del summary sin crear un mensaje nuevo.

#### Tiered Compactor
**Cambios respecto al diseño original:**
- 3 niveles de degradación en vez de 2 (nominal, degraded, emergency).
- Calcula `utility_ratio` post-compaction y loguea warning si < 0.3.
- Verifica budget post-compaction y trunca summary si excede.
- Se usa en AMBOS paths: proactivo y reactivo.

**Failure domains:**
- **LLM failure:** Aislado. Se degrada a nivel 2 sin afectar el loop.
- **Budget violation:** Detectada post-compaction. Se auto-corrige truncando summary.
- **Negative utility:** Detectada pre-apply. Se aborta la compaction.

### Diagrama revisado

```
simplified_loop
│
├─ [cada turno] ToolResultTruncator.truncate_large_results(&mut messages)
│
├─ [trigger check] CompactionBudgetCalculator.should_compact(messages, budget)
│     │
│     └─ SI → TieredCompactor.compact(messages, intent_anchor, provider, budget_calc)
│           │
│           ├─ Nivel 1 (nominal): CompactionSummaryBuilder → LLM → summary
│           │   └─ ProtectedContextInjector → merge into compaction boundary message
│           │
│           ├─ Nivel 2 (degraded): extended keep window + protected context only
│           │   └─ keep_extended = initial_keep + max_summary_tokens / avg_msg_tokens
│           │
│           └─ Nivel 3 (emergency): placeholder + IntentAnchor only
│               └─ Circuit breaker open
│
├─ [model call + streaming] (sin cambios)
│
├─ [tool execution] (sin cambios)
│
└─ [arbiter → recovery]
      └─ RecoveryAction::Compact → TieredCompactor.compact(...)  ← NUEVO
      └─ RecoveryAction::ReactiveCompact → TieredCompactor.compact(...)  ← NUEVO
```

### Extension Points

- **Tool Result Truncator** → evolucionable a persistencia a disco (Fase 2) sin cambiar interface.
- **CompactionSummaryBuilder** → evolucionable a partial compaction (XIYO `PARTIAL_COMPACT_PROMPT`) sin cambiar TieredCompactor.
- **CompactionBudgetCalculator** → evolucionable a multi-tier budgeting (snip + microcompact levels) sin cambiar el loop.
- **ProtectedContextInjector** → evolucionable a re-inyectar tool schemas, MCP, plan context (Fase 2) sin cambiar TieredCompactor.

---

## 7. Revised Validation Model

### Acceptance Criteria (obligatorios para merge)

| # | Criterio | Cómo verificar |
|---|----------|----------------|
| AC1 | Compaction nominal produce summary con ≥7 de 9 secciones presentes | Test de integración: compactar 30 mensajes con tool calls, verificar secciones en output |
| AC2 | IntentAnchor presente en context post-compaction en los 3 niveles de degradación | Test unitario por nivel |
| AC3 | `S + P + K + R ≤ B` se cumple post-compaction | Test unitario con budget calculado, assertion en producción (tracing + alert) |
| AC4 | Utility ratio ≥ 0 en todas las compactions | Assertion en TieredCompactor, abort si negativo |
| AC5 | Tool pair safety preservada | Tests existentes siguen pasando (compaction.rs tests) |
| AC6 | Fallback LLM → nivel degraded funciona | Test con mock provider que falla |
| AC7 | Circuit breaker abre tras 3 fallos, usa nivel emergency | Test con mock provider que falla 3 veces |
| AC8 | Ambos paths (proactivo + reactivo) usan TieredCompactor | Test de integración para cada path |
| AC9 | Tool results > threshold se truncan antes de compaction trigger | Test unitario |

### Observabilidad Mínima Obligatoria

Todo evento de compaction debe emitir un tracing span con:

```
compaction.level: "nominal" | "degraded" | "emergency"
compaction.tokens_before: u64
compaction.tokens_after: u64
compaction.summary_tokens: u64
compaction.protected_context_tokens: u64
compaction.keep_window_messages: u64
compaction.utility_ratio: f64
compaction.latency_ms: u64
compaction.pipeline_budget: u64
compaction.circuit_breaker_failures: u32
compaction.trigger: "proactive" | "reactive"
```

### Métricas Online

| Métrica | Definición | Señal |
|---------|------------|-------|
| **Compaction frequency** | Compactions por sesión | Reducción esperada vs baseline (por tool result truncation) |
| **Fallback rate** | % compactions nivel 2 o 3 / total | < 20% para considerar el sistema healthy |
| **Summary budget compliance** | % summaries ≤ max_summary_tokens | > 95% |
| **Utility ratio distribution** | Histograma de utility ratio | p10 > 0.3 |
| **Duplicate action rate** | Tool calls repetidos en los 5 turnos post-compaction / tool calls totales en esos turnos | Reducción vs baseline |
| **Conditional task completion** | % de sesiones que completan tarea después de ≥1 compaction | Aumento vs baseline |
| **Post-compaction halt rate** | % de sesiones que haltan (diminishing returns o stagnation) en los 3 turnos siguientes a compaction | Reducción vs baseline |

### Señales de Rollback

Desactivar feature flag si:
- Fallback rate > 40% sostenido por 24h.
- Post-compaction halt rate AUMENTA respecto a baseline.
- p99 compaction latency > 45s.
- Utility ratio p10 < 0.1.
- Regresión en tool pair safety (errores 400 por orphaned ToolResult).

### Canary Criteria

Fase canary (10% de sesiones) durante 1 semana. Comparar contra control (90% con placeholder):
- Conditional task completion: delta ≥ +5% para proceder a full rollout.
- Post-compaction halt rate: delta ≤ 0% (no empeorar).
- Compaction latency p50 < 10s, p99 < 30s.
- No errores nuevos en logs de producción atribuibles a compaction.

---

## 8. Final Recommendation

### Debe cambiar antes de implementar

1. **Implementar budget model explícito** (CompactionBudgetCalculator). Sin esto, no se puede garantizar que el estado post-compaction es viable.
2. **Usar `needs_compaction_with_budget()` existente** en vez de reinventar threshold. Ajustar su threshold para incluir compaction_reserve.
3. **Cubrir ambos paths de compaction** (proactivo y reactivo). apply_recovery debe delegar al TieredCompactor.
4. **Merge protected context en el summary message**, no como mensaje separado.
5. **Definir 3 niveles de degradación**, no 2. El nivel intermedio (extended keep + protected context, sin placeholder) es el diferenciador clave.
6. **Rebajar claims de papers** (42% reducción, Chroma) de hechos a hipótesis contextuales.

### Puede implementarse ya

1. **IntentAnchor** — riesgo cero, independiente de todo lo demás.
2. **Tool Result Truncator** — función pura, independiente, reduce frecuencia de compaction.
3. **CompactionBudgetCalculator** — lógica pura, testeable, informa todo lo demás.
4. **CompactionSummaryBuilder** — con cap proporcional, independiente del loop.

### Debe moverse a Fase 1.5 o Fase 2

| Item | Fase |
|------|------|
| Tool result persistencia a disco (vs truncation inline) | Fase 2 |
| cancel_token wiring | Corrección independiente, no este doc |
| Diminishing returns adjustment | Corrección independiente, no este doc |
| Hook runner wiring | Fase 2 |
| FallbackProvider real | Fase 2 |
| IntentAnchor dinámico | Fase 2 |
| Multi-tier compaction (snip/microcompact) | Fase 2 |

### Afirmaciones que deben rebajarse de hecho a hipótesis

- "42% reducción en task success rate": hipótesis respaldada por preprint no verificado.
- "Context rot es propiedad estructural del transformer": inferencia plausible de benchmark no reproducido internamente.
- "5-15s de latencia por compaction": estimación sin data de Halcon. Depende del modelo, del tamaño del input, y de la infraestructura.
- "1-3 invocaciones por sesión larga": hipótesis sin data de frecuencia de compaction en sesiones reales.
- "El prompt de 9 secciones preserva lo necesario": extrapolación de XIYO (referencia interna) — la calidad del summary depende del modelo y del contenido, no solo del prompt.

---

**Conclusión.** El diseño es el approach correcto. Los componentes centrales (IntentAnchor, CompactionSummaryBuilder, TieredCompactor, ProtectedContextInjector) son sólidos. Las correcciones requeridas son refinamientos de boundary, budget model y coverage — no re-arquitectura. Con las 6 correcciones descritas, el diseño es implementable, defendible y operacionalmente seguro.
