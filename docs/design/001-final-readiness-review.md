# Final Readiness Review: Compaction Semántica — Fase 1 (Hardened)

**Revisor:** Principal Systems Architect / Runtime Reliability
**Fecha:** 2026-04-03
**Documento evaluado:** `001-hardened-semantic-compaction.md`

---

## 1. Final Readiness Verdict

**Ready with minor corrections.**

El diseño hardened es arquitectónicamente sólido. El budget model está bien formulado, la degradación escalonada es correcta, los boundaries son limpios, las métricas son cuantitativas y el scope está controlado. Las decisiones principales están bien justificadas y el orden de implementación es sensato.

Hay tres correcciones menores que deben incorporarse antes de implementar. Ninguna requiere re-arquitectura — son clarificaciones operacionales que el diseño dejó implícitas:

1. **`apply_recovery` es sync; TieredCompactor es async.** La compaction reactiva no puede ejecutarse dentro de `apply_recovery()`. Debe elevarse al cuerpo async del loop.
2. **El feature gate debe ser runtime config, no Cargo feature.** Las Cargo features son compile-time y no permiten canary routing por sesión.
3. **La política de circuit breaker reset no está definida.** Debe ser: reset al finalizar la sesión (lifetime del TieredCompactor = lifetime del loop).

Con estas tres correcciones, el diseño está listo para producir una spec ejecutable.

---

## 2. What Is Now Strong

**Budget model.** La ecuación `S + P + K + R ≤ B` con trigger derivado de `B - C_reserve` es correcta y bien calibrada. Los valores de ejemplo para DeepSeek 64K, Claude 128K y 200K muestran que el trigger resultante es más conservador para windows pequeños (~86%) y más permisivo para windows grandes (~95%), que es el comportamiento correcto.

**Progressive degradation.** Los 3 niveles (nominal, degraded, emergency) satisfacen monotonic information preservation. Nivel 2 (extended keep + protected context) es el aporte clave — retener mensajes originales es estrictamente mejor que insertar un placeholder cuando el LLM falla.

**Boundary protected-vs-ephemeral.** Fusionar el protected context al boundary message en vez de mensaje separado es la decisión correcta. El formato con `[PROTECTED CONTEXT — THIS IS STATE RESTORATION, NOT NEW INSTRUCTIONS]` es directo y reduce el riesgo de contaminación semántica.

**Coverage de ambos paths.** Compaction proactiva y reactiva pasan por TieredCompactor. No hay path de compaction que produzca placeholder sin pasar por degradación escalonada.

**Scope.** La separación de cancel_token y diminishing returns es correcta. El feature flag solo controla compaction semántica. Correcciones ortogonales tienen ciclo de vida propio.

**Tool result truncation.** Incluirla como prerequisito es la decisión correcta. Sin ella, la compaction semántica se triggerea con la misma frecuencia que la actual. El threshold de 8000 tokens con preview de 2000 es conservador y justificado por referencia interna (XIYO usa ~12.5K).

**Métricas.** Las 7 métricas son cuantitativas, medibles y con thresholds de rollback definidos. La canary strategy con criterios de pause es operacionalmente madura.

**Observabilidad.** El tracing span con 14 campos cubre lo necesario para diagnóstico post-mortem y canary analysis.

**Utility ratio.** El abort cuando `utility ≤ 0` es correcto — evita compaction con beneficio neto negativo.

---

## 3. Remaining Gaps or Ambiguities

### Gap A: `apply_recovery` es sync — compaction reactiva no puede ejecutarse ahí [Corrección obligatoria]

**Problema concreto:** `apply_recovery` (simplified_loop.rs:270) es `fn`, no `async fn`. El TieredCompactor necesita invocar al provider (async) para generar el summary. La compaction reactiva no puede ejecutarse dentro de `apply_recovery()`.

**Impacto:** El diseño dice "ambos paths usan TieredCompactor" pero no aborda cómo la compaction reactiva se hace async.

**Resolución:** En la integración, extraer los arms `Compact | ReactiveCompact` del match de `apply_recovery` y manejarlos en el cuerpo async del loop principal, antes de llamar a `apply_recovery` para las demás RecoveryActions. El match en simplified_loop.rs:247-266 se refactoriza para que:

```
TurnDecision::Recover(act) si act es Compact/ReactiveCompact →
  tiered_compactor.compact(...).await
TurnDecision::Recover(act) para todo lo demás →
  apply_recovery(...)
```

Esto es un cambio mecánico — no altera la arquitectura.

### Gap B: Feature gate debe ser runtime, no compile-time [Corrección obligatoria]

**Problema concreto:** El diseño dice "feature flag: `semantic-compaction`". Las Cargo features del proyecto son compile-time (`cfg(feature = "...")`). Un feature compile-time no permite:
- Canary routing (10% de sesiones).
- Toggle sin recompilación.
- A/B testing en producción.

**Resolución:** Usar un campo en CompactionConfig (runtime):

```
pub struct CompactionConfig {
    pub enabled: bool,
    pub threshold_fraction: f32,
    pub keep_recent: usize,
    pub max_context_tokens: u32,
    pub semantic_compaction: bool,  // NUEVO — runtime toggle
}
```

Default: `false` (comportamiento actual). Toggle a `true` via config. Esto permite:
- Canary: activar para subset de sesiones basado en config.
- Rollback instantáneo: cambiar config, no recompilar.
- Testing: activar/desactivar en tests sin recompilación.

### Gap C: Política de reset del circuit breaker no está definida [Corrección menor]

**Problema concreto:** El diseño dice "3 failures consecutivas → nivel 3. No intentar LLM hasta reset de sesión." Pero no define cuándo ni cómo se resetea el circuit breaker.

**Resolución:** El `consecutive_failures: u32` vive en el TieredCompactor. El TieredCompactor se crea al inicio del loop y se destruye al final. Por lo tanto, el circuit breaker se resetea naturalmente al final de la sesión (nuevo loop = nuevo TieredCompactor = counter en 0). Esto es correcto y debe documentarse explícitamente como: "El circuit breaker tiene scope de sesión. Se resetea cuando el loop termina."

### Gap D: Semántica exacta del abort cuando utility ≤ 0 [Clarificación]

**Situación:** Cuando utility ≤ 0, el diseño dice "ABORT, no compactar, dejar que el budget se agote naturalmente". Pero si el trigger sigue cumpliéndose cada turno (T ≥ B - C_reserve), el abort se repite indefinidamente hasta que el API falla con prompt_too_long. Entonces el arbiter activa Compact recovery, que vuelve a intentar y aborta de nuevo. Tras 2 attempts, halt con UnrecoverableError.

**Evaluación:** Esto es comportamiento correcto. Si la compaction tiene beneficio negativo (los mensajes recientes son tan grandes que el summary + protected context no cabe), el sistema no puede auto-remediarse. El halt con UnrecoverableError es la respuesta adecuada. El loop de abort-retry-abort-halt es corto (2 iteraciones máximo por max_compact_attempts) y observable en logs.

**No requiere cambio.** Solo documentar que este escenario resulta en halt después de 2 intentos, y que la causa más probable es un tool result extremadamente grande que debería haber sido truncado. Esto refuerza la importancia del ToolResultTruncator.

### Gap E: IntentAnchor y mensajes multi-turn [Clarificación]

**Situación:** El diseño dice "se crea al inicio del loop a partir del primer mensaje del usuario". Pero `config.request.messages` puede contener múltiples mensajes (e.g., system prompt + user message, o continuación de sesión previa). El IntentAnchor debe crearse a partir del primer mensaje con `Role::User`, no del primer mensaje en general.

**Resolución:** La spec de implementación debe especificar: "Buscar el primer mensaje con `role == Role::User` en `config.request.messages`. Si no existe (edge case: solo system prompt), crear IntentAnchor vacío con task_summary = '[no user message found]'."

### Gap F: ToolResultTruncator y tool pair safety [Clarificación]

**Situación:** El ToolResultTruncator muta ToolResult blocks in-place. Si un ToolResult se trunca, su `tool_use_id` se preserva (solo cambia el contenido). Esto no afecta tool pair safety porque `safe_keep_boundary_n()` opera sobre IDs, no contenido.

**Evaluación:** Correcto. No requiere cambio. Solo confirmar en los tests que truncation preserva `tool_use_id` y `is_error`.

---

## 4. Mathematical / Operational Sanity Check

### Budget Equation: VÁLIDA

`S + P + K + R ≤ B` con `B = W × U` es correcta. Las variables están bien definidas y los caps son razonables.

**Verificación con DeepSeek 64K:**
- W = 64,000, U = 0.80, B = 51,200
- S_max = clamp(51200/20, 1000, 4000) = 2,560
- P_max = 500
- R = 4,096
- C_reserve = 2,560 + 500 + 4,096 = 7,156
- Trigger = 51,200 - 7,156 = 44,044
- Post-compaction: S(2560) + P(500) + K(?) + R(4096) ≤ 51,200 → K ≤ 44,044
- adaptive_keep_recent(51200) = 5 mensajes → K ≈ 5 × ~1000 = ~5,000 tokens
- 2560 + 500 + 5000 + 4096 = 12,156 ≤ 51,200 ✓ (amplio margen)

**Verificación con Claude 200K:**
- W = 200,000, U = 0.80, B = 160,000
- S_max = 4,000 (cap)
- C_reserve = 4,000 + 500 + 4,096 = 8,596
- Trigger = 151,404
- Post: 4000 + 500 + K + 4096 ≤ 160,000 → K ≤ 151,404
- adaptive_keep_recent(160000) = 16 mensajes → K ≈ 16 × ~1000 = ~16,000
- 4000 + 500 + 16000 + 4096 = 24,596 ≤ 160,000 ✓

El budget es consistente en ambos extremos.

### Trigger Policy: VÁLIDA

El trigger `T ≥ B - C_reserve` es correcto. El trigger se adapta al tamaño del window y al overhead de compaction. Para windows pequeños es más conservador (86%), para grandes más permisivo (95%). Esto es el comportamiento deseado.

**Constraint implícita verificada:** El trigger nunca produce `C_reserve > B` porque:
- S_max mínimo = 1000, P_max = 500, R = 4096 → C_reserve mínimo = 5,596
- B mínimo práctico = W(32K) × U(0.80) = 25,600
- 5,596 < 25,600 ✓

### Summary Cap Policy: VÁLIDA con observación

`S_max = clamp(B/20, 1000, 4000)` es razonable. La proporción 1/20 es una hipótesis operacional — correctamente marcada como tal.

**Observación:** Para sesiones con muchos archivos y decisiones complejas, 4000 tokens puede ser insuficiente para 9 secciones densas. Pero 4000 tokens ≈ 3000 palabras, que es sustancial para un summary. El riesgo de insuficiencia es bajo para Fase 1.

**El floor de 1000 es el punto más tenso.** 9 secciones × ~100 tokens = 900 tokens mínimo. Floor de 1000 deja ~100 tokens de margen. Si una sesión tiene muchos file paths o errores, el floor puede ser insuficiente. Recomendación: en canary, monitorear el % de summaries que se truncan a floor. Si > 10%, subir floor a 1500.

### Utility Ratio: VÁLIDO con clarificación

```
utility = (T_freed - T_added) / T_freed
T_freed = T_before - K
T_added = S + P
```

**Caso donde utility > 0 pero el resultado es malo:** Si T_freed = 100,000 y T_added = 4,500 (S=4000 + P=500), utility = 0.955 — excelente. Pero si el summary de 4000 tokens pierde información crítica (e.g., omite pending tasks), el utility ratio no lo detecta. El utility ratio mide eficiencia de tokens, no calidad semántica.

**Evaluación:** Esto es una limitación conocida y aceptable. El utility ratio detecta compaction de beneficio negativo (cuando añade más de lo que libera), que es el failure mode mecánico. La calidad semántica del summary es responsabilidad del prompt de 9 secciones y se valida con métricas online (duplicate action rate, conditional task completion), no con el utility ratio. El diseño no confunde las dos cosas.

### Extended Keep Count: VÁLIDO con edge case documentado

```
extended_keep_count = initial_keep + (S_max / avg_tokens_per_recent_message)
```

**Edge case:** Si `avg_tokens_per_recent_message` es muy pequeño (e.g., mensajes de confirmación cortos: "ok", "done" → ~5 tokens), entonces `S_max / avg` puede ser muy grande (2560 / 5 = 512 mensajes). El cap `messages.len()` lo limita, pero si hay 200 mensajes, el extended keep retiene 200 — no compactando nada.

**Evaluación:** Si todos los mensajes caben en el extended keep, `apply_compaction_keep` hace noop (compaction.rs:187-189: `if keep >= messages.len() { return; }`). El trigger se sigue cumpliendo cada turno, y eventualmente los mensajes crecen lo suficiente para que extended_keep < messages.len(). Esto no es un loop infinito — es un delay de compaction que puede afectar performance pero no correctness.

**Recomendación:** Añadir un cap explícito al extended_keep_count: `min(extended_keep_count, messages.len() * 2/3)`. Esto garantiza que siempre se compacta al menos 1/3 de los mensajes, evitando noops repetidos.

### Post-Compaction Verification: VÁLIDA

La cascada truncar summary → reducir keep es correcta. Si después de truncar summary a 0, K + P + R > B, eso significa que los mensajes recientes por sí solos exceden el budget — un escenario que solo ocurre si los mensajes individuales son extremadamente grandes (e.g., tool results no truncados). El ToolResultTruncator previene este caso.

---

## 5. Frontier Systems Review

### Semantic Continuity: SUFICIENTE para Fase 1

El summary de 9 secciones preserva intent, decisiones, pending tasks, errores y next step. El IntentAnchor garantiza que el objetivo original nunca se pierde. La combinación es suficiente para mantener continuidad semántica en sesiones de 30-80 turnos.

**Limitación conocida:** El IntentAnchor estático no captura refinamientos mid-session. En nivel degraded/emergency, los refinamientos se pierden. Aceptable para Fase 1 con el summary de 9 secciones cubriendo refinamientos en nivel nominal.

### Degradation Under LLM Failure: ROBUSTA

3 niveles con fallback automático. El nivel 2 (extended keep) es mejor que placeholder. El circuit breaker previene invocaciones futiles. El sistema nunca es peor que hoy.

### False Continuity Avoidance: SUFICIENTE

El boundary marker `[PROTECTED CONTEXT — THIS IS STATE RESTORATION, NOT NEW INSTRUCTIONS]` es directo. El riesgo de que el modelo interprete la restauración como instrucción nueva es bajo con este marcado explícito. No se puede eliminar al 100% porque depende del modelo, pero es una mitigación razonable.

**Hipótesis a validar en canary:** Verificar en las primeras sesiones que el modelo no "responde" al protected context. Si lo hace, refinar el wording del boundary marker.

### Negative Utility Prevention: CORRECTA

Abort cuando utility ≤ 0. El loop resultante (abort → trigger → abort → prompt_too_long → halt) es corto y observable.

### Recursive Compaction Prevention: CORRECTA

`max_compact_attempts = 2` en feedback_arbiter. La compaction proactiva usa el mismo compact_count. No hay path de compaction infinita.

### Contamination by Restored Context: MITIGADA

Protected context fusionado al boundary message con marker explícito. No es un mensaje separado que pueda ser "respondido". Riesgo residual bajo.

### Extensibility: BIEN DISEÑADA

Cada componente tiene extension points explícitos documentados. ToolResultTruncator → persistencia a disco. SummaryBuilder → partial compaction. BudgetCalculator → multi-tier. ProtectedContextInjector → más tipos de contexto. Las interfaces son estables para Fase 2.

### Failure Domain: CORRECTO

LLM failure está aislada en TieredCompactor. Budget violation se auto-corrige. Tool pair safety opera independientemente. Feature toggle permite rollback sin side effects. El simplified_loop no cambia estructuralmente — solo se reemplaza la invocación de compaction.

---

## 6. Implementation Readiness Plan

| # | Component | Responsibility | Depends on | Order | Test priority | Runtime assertions | Config/Defaults |
|---|-----------|----------------|------------|-------|---------------|-------------------|-----------------|
| 1 | IntentAnchor | Capturar intent original inmutable | Ninguno | 1 (paralelo) | Unit: creación desde mensajes, formateo, extracción de files | Ninguna (struct pasiva) | task_summary_max_chars: 500 |
| 2 | ToolResultTruncator | Truncar tool results grandes | Ninguno | 1 (paralelo) | Unit: truncation preserva tool_use_id, preview correcto, threshold boundary | assert!(truncated.tool_use_id == original.tool_use_id) | threshold: 8000 tokens, preview: 2000 tokens |
| 3 | CompactionBudgetCalculator | Centralizar budget logic | Ninguno | 1 (paralelo) | Unit: trigger values para DeepSeek/Claude, S_max scaling, post-compaction verification, utility ratio | assert!(trigger_threshold < pipeline_budget) | U: 0.80, S_proportion: 1/20, S_floor: 1000, S_cap: 4000, P_max: 500, R: 4096 |
| 4 | CompactionSummaryBuilder | Construir prompt 9 secciones | IntentAnchor (tipo) | 2 | Unit: prompt contiene 9 secciones, incluye intent anchor, trunca tool outputs en prompt input, respeta S_max | Ninguna (produce strings) | tool_output_preview_in_prompt: 200 chars |
| 5 | ProtectedContextInjector | Formatear bloque protected context | IntentAnchor (tipo) | 2 (paralelo con 4) | Unit: output contiene intent, tools, files; boundary markers presentes | assert!(output.len() ≤ P_max estimado) | Ninguno |
| 6 | TieredCompactor | Orquestar compaction 3 niveles | 1-5 | 3 | Integration: nivel 1 con mock provider exitoso, nivel 2 con mock que falla, nivel 3 con circuit breaker, utility abort, post-compaction budget check, ambos triggers (proactivo/reactivo) | assert!(utility > 0 before apply), assert!(T_after + R ≤ B after apply) | timeout: 30s, temperature: 0.0, max_circuit_breaker: 3, extended_keep_cap: 2/3 × messages.len() |
| 7 | Integración en simplified_loop | Reemplazar placeholder, cubrir ambos paths | 6, refactoring de apply_recovery | 4 | Integration: loop completo con compaction mock, loop con compaction real (provider test), reactive path via arbiter trigger | Feature flag check at integration point | CompactionConfig.semantic_compaction: false (default) |

### Parallelización

- **Paralelo (Fase 1a):** Componentes 1, 2, 3 — cero dependencias entre sí. Tests unitarios puros.
- **Paralelo (Fase 1b):** Componentes 4, 5 — dependen de tipos de 1 pero no de su implementación completa. Solo necesitan el tipo IntentAnchor para compilar.
- **Secuencial (Fase 1c):** Componente 6 integra 1-5. Componente 7 integra 6 en el loop.

### Qué probar primero

1. **CompactionBudgetCalculator.** Si el budget model está mal, todo lo demás falla. Testear exhaustivamente con valores edge (window 32K, 64K, 200K, 1M).
2. **ToolResultTruncator.** Si la truncation rompe tool_use_id o is_error, tool pair safety se rompe. Testear con todos los tipos de ContentBlock.
3. **TieredCompactor degradation.** Si el fallback a nivel 2/3 falla, el sistema puede quedar sin compaction funcional. Testear con mock providers que fallan de distintas formas (timeout, error, respuesta vacía).

---

## 7. Decisions to Freeze Now

| # | Decision | Value | Status | Justification |
|---|----------|-------|--------|---------------|
| 1 | Budget equation | S + P + K + R ≤ B | Frozen | Matemáticamente verificada para todos los window sizes relevantes. |
| 2 | Trigger formula | T ≥ B - C_reserve | Frozen | Derivada del budget model, adaptativa por diseño. |
| 3 | 3 niveles de degradación | Nominal/Degraded/Emergency | Frozen | Satisface monotonic information preservation. |
| 4 | Protected context fusionado | Parte del boundary message | Frozen | Evita mensajes separados, reduce riesgo de contaminación. |
| 5 | Tool result truncation en Fase 1 | Threshold 8000 tokens | Frozen | Prerequisito confirmado por evidencia interna. |
| 6 | Circuit breaker: 3 failures | 3 failures → nivel 3 | Frozen | Alineado con XIYO, justificado por data interna. |
| 7 | Scope: cancel_token y diminishing fuera | Separados | Frozen | Ortogonales a compaction semántica. |
| 8 | Feature gate: runtime config | CompactionConfig.semantic_compaction | Frozen | Necesario para canary. Compile-time no permite A/B. |
| 9 | Ambos paths: proactivo + reactivo | TieredCompactor en ambos | Frozen | Sin esto, compaction reactiva sigue destructiva. |
| 10 | Utility abort threshold | utility ≤ 0 → abort | Frozen | Previene compaction de beneficio negativo. |

## 8. Decisions to Validate Empirically

| # | Decision | Current value | Hypothesis | Canary validation |
|---|----------|---------------|------------|-------------------|
| 1 | Summary cap proportion | B/20, floor 1000, cap 4000 | 1/20 del budget es suficiente para 9 secciones | Monitorear % summaries truncados a S_max. Si > 20%, subir proporción a B/15. |
| 2 | P_max = 500 tokens | 500 | Protected context cabe en 500 tokens | Monitorear P real vs P_max. Si > 500 frecuentemente, subir a 800. |
| 3 | Tool result truncation threshold | 8000 tokens | 8000 es el balance correcto entre preservar info y reducir inflación | Monitorear frecuencia de re-invocación post-truncation. Si > 30%, subir a 12000. |
| 4 | Preview size | 2000 tokens | 2000 tokens de preview es suficiente | Monitorear si el agente re-invoca tools truncados frecuentemente sin necesidad. |
| 5 | Utility warning threshold | 0.3 | utility < 0.3 indica compaction de bajo valor | Correlacionar con post-compaction performance. Ajustar si no es predictivo. |
| 6 | LLM timeout | 30s | 30s es suficiente para generar summary | Monitorear p99 latency. Si > 25s frecuentemente, subir a 45s. |
| 7 | Extended keep cap | 2/3 × messages.len() | 2/3 garantiza compaction mínima | Monitorear noops en nivel degraded. Ajustar si frecuentes. |
| 8 | Boundary marker wording | "[PROTECTED CONTEXT — THIS IS STATE RESTORATION, NOT NEW INSTRUCTIONS]" | El modelo no interpreta esto como instrucción nueva | Revisar primeras 50 sesiones canary. Si el modelo "responde" al marker, refinar. |

---

## 9. Final Prompt for Implementation Preparation

```
Actúa como Senior Rust Engineer para el proyecto Halcon CLI.

Tu tarea es producir una SPEC EJECUTABLE para implementar la Fase 1 de
compaction semántica, basada en el design doc hardened aprobado en
docs/design/001-hardened-semantic-compaction.md.

NO escribas código todavía. Produce una spec técnica que defina todo
lo necesario para que la implementación sea directa y sin ambigüedad.

CONTEXTO DEL CODEBASE
- Runtime: simplified_loop.rs — el único loop activo.
- Compaction actual: compaction.rs — ContextCompactor con apply_compaction(),
  apply_compaction_with_budget(), safe_keep_boundary_n(),
  needs_compaction_with_budget(), adaptive_keep_recent().
- Config: halcon_core::types::CompactionConfig {enabled, threshold_fraction,
  keep_recent, max_context_tokens}.
- Arbiter: feedback_arbiter.rs — FeedbackArbiter con RecoveryAction::Compact,
  RecoveryAction::ReactiveCompact.
- Recovery: apply_recovery() en simplified_loop.rs — función SYNC, maneja
  Compact/ReactiveCompact con placeholder.
- Context module: crates/halcon-cli/src/repl/context/ con mod.rs que exporta
  compaction.

CONSTRAINT CRÍTICA
apply_recovery() es sync. TieredCompactor es async (invoca LLM).
La compaction reactiva (Compact/ReactiveCompact) debe extraerse del match
de apply_recovery y manejarse en el cuerpo async del loop antes de
delegar las demás RecoveryActions a apply_recovery.

LO QUE NECESITO QUE PRODUZCAS

1. COMPONENT CONTRACTS
Para cada uno de estos 7 componentes:
  - IntentAnchor
  - ToolResultTruncator
  - CompactionBudgetCalculator
  - CompactionSummaryBuilder
  - ProtectedContextInjector
  - TieredCompactor
  - Integration (simplified_loop modifications)

Define:
  - Archivo destino (ruta exacta en el proyecto).
  - Struct/trait/function signatures (tipos, no implementación).
  - Inputs y outputs con tipos concretos de Halcon
    (ChatMessage, ContentBlock, MessageContent, Role, CompactionConfig,
    ModelProvider, etc.).
  - Invariantes como assertions en runtime.
  - Error types y handling policy.

2. CONFIG SCHEMA
Define los campos nuevos a añadir a CompactionConfig:
  - semantic_compaction: bool (default false)
  - tool_result_truncation_threshold: usize (default 8000)
  - tool_result_preview_size: usize (default 2000)
  - utilization_factor: f32 (default 0.80)
  - summary_proportion: f32 (default 0.05, i.e. 1/20)
  - summary_floor: usize (default 1000)
  - summary_cap: usize (default 4000)
  - protected_context_cap: usize (default 500)
  - compaction_timeout_secs: u64 (default 30)
  - max_circuit_breaker_failures: u32 (default 3)
Define defaults, validation rules, y backward compatibility.

3. INTEGRATION SPEC
Define exactamente cómo se modifican:
  - simplified_loop.rs: dónde se crea IntentAnchor, dónde se ejecuta
    ToolResultTruncator, cómo se reemplaza el bloque proactivo (líneas
    158-163), cómo se extrae compaction reactiva del match de
    apply_recovery (línea 253), qué parámetros nuevos necesita
    SimplifiedLoopConfig.
  - apply_recovery: qué arms se eliminan, qué queda.
  - compaction.rs: qué funciones se reutilizan (apply_compaction_keep,
    safe_keep_boundary_n, adaptive_keep_recent, estimate_message_tokens),
    qué se añade.
  - context/mod.rs: qué módulos nuevos se registran.

4. ACCEPTANCE CRITERIA (los 10 del design doc)
Para cada AC, define:
  - test type (unit/integration)
  - test location (archivo)
  - setup (qué fixtures/mocks)
  - assertion exacta
  - qué failure mode detecta

5. RUNTIME ASSERTIONS
Lista exacta de assertions que deben existir en código de producción
(no solo en tests):
  - assert/debug_assert con condición y mensaje
  - tracing::warn/error para condiciones no-fatales
  - qué hacer si la assertion falla en producción

6. TRACING SPEC
Para el tracing span de compaction, define:
  - span name
  - 14 campos del design doc con tipos exactos
  - dónde se crea y cierra el span
  - qué log level usar para cada evento

7. TEST PLAN
Organizado por componente:
  - tests unitarios (qué, dónde, fixtures)
  - tests de integración (qué escenarios, qué mocks)
  - edge cases obligatorios:
    * DeepSeek 64K budget
    * utility ≤ 0 abort
    * circuit breaker open after 3 failures
    * tool pair safety post-truncation
    * extended keep con mensajes muy cortos
    * IntentAnchor con mensajes sin Role::User
    * reactive compaction path async
    * feature flag off → comportamiento actual preservado

RESTRICCIONES
- No escribas implementación todavía — solo spec.
- Usa tipos reales del proyecto (ChatMessage, ContentBlock, etc.).
- Referencia líneas de código exactas cuando sea relevante.
- Si hay una decisión de diseño que todavía parece ambigua, dilo —
  no la asumas resuelta.
- Mantén la spec concisa y ejecutable. Nada de texto decorativo.

OUTPUT
Un solo documento markdown con las 7 secciones.
Debe ser suficiente para que un ingeniero implemente los 7 componentes
sin necesidad de volver al design doc para clarificaciones.
```

---

**El diseño hardened está listo para pasar a spec ejecutable.** Las 3 correcciones menores (async path, runtime config, circuit breaker reset) son clarificaciones operacionales — no cambian la arquitectura. Las 8 hipótesis operacionales están correctamente identificadas para validación empírica. Las 10 decisiones frozen son defendibles. El budget model es matemáticamente sólido.
