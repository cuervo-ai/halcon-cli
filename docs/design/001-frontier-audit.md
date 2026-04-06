# Auditoría Frontier: Compaction Semántica — Fase 1

**Fecha:** 2026-04-03
**Tipo:** Implementation audit + frontier validation + research-backed correction

---

## 1. Executive Verdict

**Near frontier-ready.**

La implementación es estructuralmente sólida — los 7 componentes existen, compilan, pasan 45+ tests nuevos, y la integración en simplified_loop cubre ambos paths (proactivo + reactivo) con fallback correcto cuando `semantic_compaction = false`. El budget model está matemáticamente verificado. La degradación de 3 niveles es un avance genuino sobre XIYO (que no tiene degradación — falla binariamente).

Hay 3 defectos reales que impiden el estatus AAA:
1. `verify_and_fix()` no actualiza `tokens_after` en el `CompactionResult` retornado — el caller recibe métricas stale.
2. Emergency mode usa `keep = 4` hardcoded en vez del budget.
3. No hay test de integración que demuestre el flujo completo nominal → degraded → emergency.

Estos son corregibles en < 1 día sin cambios arquitectónicos.

---

## 2. Verified Implementation Status

| Componente | Esperado por spec | Implementado | Estado | Evidencia |
|---|---|---|---|---|
| CompactionConfig extension | 10 campos nuevos, serde defaults, backward compat | 10 campos con `#[serde(default)]`, Default impl, free functions para defaults | **Correct** | config.rs verificado, tests existentes pasan |
| ContextCompactor.config() | Getter público | `pub fn config() -> &CompactionConfig` | **Correct** | compaction.rs:23-25 |
| ContextCompactor.apply_compaction_with_keep_count() | Método público con safe_keep_boundary | Implementado, retorna keep efectivo | **Correct** | compaction.rs:32-41 |
| IntentAnchor | Inmutable, from_messages, format_for_boundary | Struct con 5 campos, regex file extraction, 5 tests | **Correct** | intent_anchor.rs completo |
| ToolResultTruncator | Función pura, preserva IDs, skip last 2 | truncate_large_tool_results(), 8 tests | **Correct** | tool_result_truncator.rs completo |
| CompactionBudgetCalculator | compute, should_compact, verify, utility_ratio | 4 funciones, 13 tests, DeepSeek + Claude values | **Correct** | compaction_budget.rs completo |
| CompactionSummaryBuilder | Prompt 9 secciones, format_messages | build_prompt + format, 5 tests | **Correct** | compaction_summary.rs completo |
| ProtectedContextInjector | Bloque con boundary markers | build_block(), 6 tests | **Correct** | protected_context.rs completo |
| TieredCompactor | 3 niveles, circuit breaker, stream consumption | compact() async, 3 niveles, 8 tests | **Partial** | verify_and_fix no actualiza result; emergency hardcodes keep=4 |
| Proactive compaction integration | TieredCompactor en loop, truncation previa | Implementado con CompactionBudgetCalculator | **Correct** | simplified_loop.rs:178-229 |
| Reactive compaction extraction | Compact/ReactiveCompact fuera de apply_recovery | Extraído al async body, apply_recovery tiene unreachable!() | **Correct** | simplified_loop.rs:331-378, :400-402 |
| compact_count increment | En loop body, antes de compact | Proactivo: línea 208 (después). Reactivo: línea 334 (antes). | **Correct** | Posiciones consistentes con spec |
| Config propagation | CompactionConfig al SimplifiedLoopConfig via dispatch | ctx.compactor.map(\|c\| c.config().clone()).unwrap_or_default() | **Correct** | dispatch.rs:56-58 |
| files_modified tracking | Extraer de Edit/Write tool calls | Implementado post-tool-execution | **Correct** | simplified_loop.rs:294-303 |
| Feature flag off behavior | Placeholder compaction preservada | Fallback path preserva COMPACTION_THRESHOLD=0.90 y placeholder | **Correct** | simplified_loop.rs:223-228 |

---

## 3. Halcon vs XIYO vs Frontier Evidence

| Capability | Halcon actual | XIYO | Evidencia pública | Gap | Severidad | Recomendación |
|---|---|---|---|---|---|---|
| **Compaction semántica** | Prompt 9 secciones via LLM | Prompt 9 secciones via forked agent | Anthropic Compaction API (beta 2026-01). ACON paper: LLM summarization viable pero naive compression falla. | Paridad funcional. Halcon tiene "Decisions Made" que XIYO omite. | Ninguna | Congelado |
| **Post-compact reinjection** | Metadata-only (intent, tools, files como strings) | File re-read desde disco (50K budget, hasta 5 archivos re-attached) | Anthropic: CLAUDE.md re-read from disk post-compact. | **Halcon no re-lee archivos**. Solo lista metadata. El modelo pierde contenido real de archivos leídos. | Alta (Fase 2) | Fase 2: persistencia a disco + re-lectura |
| **Tool result budgeting** | Truncation inline 8K tokens, preview 2K | Persistencia a disco 50K chars, recovery via Read, 200K aggregate/mensaje | Anthropic: 84% token reduction con context editing. | Halcon trunca inline (pierde info). XIYO persiste y permite recovery. | Media (Fase 1.5) | Fase 1 tiene truncation — suficiente como MVP. Fase 2 para persistencia. |
| **Budget model** | S+P+K+R≤B con post-verification | effectiveWindow - 13K buffer. Sin post-verification. | No hay evidencia pública de budget equations formales en sistemas agentic. | **Halcon es superior** a XIYO en rigor de budget. | Ventaja Halcon | Congelado |
| **Progressive degradation** | 3 niveles: nominal/degraded/emergency | Binario: pass/fail. Circuit breaker desactiva autocompact. | Zylos: 65% de failures por context drift. LangChain: 300 tokens focalizados > 113K sin foco. | **Halcon es superior** a XIYO. Degradación progresiva es un avance real. | Ventaja Halcon | Congelado |
| **Intent preservation** | IntentAnchor inmutable desde primer User message | Prompt sección 1 + "ALL user messages" en resumen | ACE paper: "evolving playbooks". Anthropic: `/compact focus` directive. IMPACT framework: Intent component. | Paridad funcional. XIYO preserva en el resumen; Halcon además tiene anchor estructural separado. | Ninguna | Congelado |
| **Circuit breaker** | 3 failures → emergency mode (mantiene funcionalidad degradada) | 3 failures → desactiva autocompact (sin fallback) | XIYO data interna: 250K API calls/día desperdiciadas pre-breaker. | **Halcon es superior** — emergency mode sigue operando vs XIYO que se detiene. | Ventaja Halcon | Congelado |
| **Boundary message** | Fusionado al summary como Role::User con markers | Attachments separados (plan, skills, tools, MCP, agents, files) | Anthropic: attachment system con tipos tipados. | XIYO tiene tipado de attachments; Halcon usa texto plano con markers. | Baja | Aceptable para Fase 1. Fase 2 puede estructurar. |
| **Observabilidad** | Tracing span con 14 campos + sequence_number | Analytics pipeline con pre/post token counts, cache metrics | Anthropic blog: necesario para canary. | Halcon cubre lo necesario para canary. XIYO tiene analytics más ricos. | Baja | Suficiente para Fase 1 |
| **Selective forgetting** | Tool result truncation + compaction por niveles | Snip + microcompact + context collapse + autocompact (5 niveles) | MemoryAgentBench (ICLR 2026): "selective forgetting" es una de 4 competencias esenciales. | Halcon tiene 1 nivel de truncation + 1 de compaction. XIYO tiene 5. | Media (Fase 2) | Fase 2: multi-tier compaction |
| **Evaluation** | Utility ratio + 7 métricas canary definidas | Analytics BigQuery + A/B testing infrastructure | MemoryAgentBench: 4 competencias. MemBench: factual + reflective. AMA-Bench: memory processing + retrieval. | Halcon no tiene benchmark formal de calidad de compaction. | Media | Fase 1.5: evaluation framework mínimo |

---

## 4. Scientific and Technical Validation

### Compaction Semantics
El prompt de 9 secciones está alineado con la práctica de frontera. Anthropic's engineering blog recomienda explícitamente "summarize message history preserving architectural decisions and unresolved bugs" — que es exactamente lo que hacen las secciones 4 (Errors), 5 (Decisions), 7 (Pending Tasks). El paper ACON (arXiv:2510.00615) muestra que LLM summarization mantiene >95% accuracy cuando las guidelines de compresión están bien definidas. El prompt de Halcon tiene guidelines explícitas ("preserve ALL user messages", "include exact file paths", "error codes verbatim"). **Validado como frontier-grade.**

### Memory Handling
MemoryAgentBench (ICLR 2026) define 4 competencias: accurate retrieval, test-time learning, long-range understanding, selective forgetting. Halcon cubre: retrieval parcial (el summary preserva paths y decisiones), learning parcial (intent anchor), long-range parcial (protected context). Selective forgetting es el gap más grande — Halcon tiene truncation de tool results pero no tiene snip/microcompact multi-nivel. **Near frontier — falta multi-tier para completar.**

### Budget Correctness
No existe evidencia pública de que otros sistemas agentic implementen una ecuación formal `S + P + K + R ≤ B` con post-verification. XIYO usa thresholds heurísticos (13K buffer). El approach de Halcon es **más riguroso que el estado del arte observable**. Anthropic's Compaction API (beta) usa triggers por porcentaje sin verificación formal post-compaction. **Halcon es frontier-grade en budget model.**

### Utility-Based Control
El abort cuando `utility ≤ 0` no tiene equivalente directo en XIYO ni en la literatura pública. Es un guardrail que previene compaction de beneficio negativo. ACON paper muestra que naive compression puede degradar performance — el utility check es una protección contra esto. **Frontier-grade.**

### Degradation Design
3 niveles con monotonic information preservation. No hay evidencia de que otros sistemas públicos implementen degradación progresiva en compaction. XIYO falla binariamente. La investigación de Zylos (2026) reporta 65% de failures por context drift — la degradación progresiva es exactamente la respuesta correcta. **Frontier-grade en diseño. La implementación tiene un defecto en verify_and_fix que la baja a near-frontier.**

### Evaluation Methodology
7 métricas canary definidas (utility ratio, summary compliance, fallback rate, halt rate, task completion, duplicate action rate, latency). Sin embargo, no hay un benchmark formal de calidad semántica del summary. MemBench y MemoryAgentBench definen frameworks — Halcon no los implementa. **Near frontier. Fase 1.5 debería incorporar evaluation mínimo.**

### Observability Rigor
14 campos de tracing span + sequence_number. Suficiente para canary, rollback, y diagnóstico. XIYO tiene analytics más sofisticados pero Halcon cubre lo necesario operacionalmente. **Frontier-grade para Fase 1.**

---

## 5. Critical Remaining Gaps

### Gap 1: `verify_and_fix()` no actualiza CompactionResult [CRITICAL]

**Componente:** TieredCompactor::try_nominal(), líneas ~280-290

`tokens_after` se calcula, luego se llama `verify_and_fix()` que puede mutar mensajes, luego hay un segundo `tokens_after = estimate_message_tokens(messages)`. Sin embargo, el `CompactionResult` retornado usa el último `tokens_after` — que sí se actualiza. **Al revisar el código más cuidadosamente: hay una re-estimación post-verify_and_fix en try_nominal.** El gap reportado por el audit agent es parcialmente incorrecto.

**Pero** el path `KeepReductionNeeded` dentro de `verify_and_fix()` no verifica éxito de la re-aplicación. Si `messages.first()` no es Text (improbable pero posible si el boundary message fue corrompido), la reducción se silencia.

**Severidad:** Media. El path es infrecuente y la pre-condición (boundary message no es Text) es anómala.

**Fix:** Añadir log de error si el primer mensaje no es Text en el path de KeepReductionNeeded.

### Gap 2: Emergency mode usa keep=4 hardcoded [HIGH]

**Componente:** TieredCompactor::apply_emergency()

Keep hardcoded a 4 en vez de `budget.keep_count.max(4)`. Para context windows pequeños (DeepSeek 64K), `budget.keep_count = 5`, que es mayor que 4 y sería correcto usar. Para windows grandes, `budget.keep_count = 16`, y usar 4 es innecesariamente agresivo.

**Severidad:** Alta para calidad semántica; baja para correctness (no viola budget).

**Fix:** Cambiar a `budget.keep_count.max(4)`.

### Gap 3: No hay test de integración del flujo completo [MEDIUM]

No existe un test que demuestre la cascada nominal → degraded → emergency en una secuencia realista de compactions. Los tests individuales de cada nivel pasan, pero la interacción entre niveles no está probada.

**Severidad:** Media. Cada nivel funciona en aislamiento; el riesgo es bajo.

**Fix:** Añadir un test que fuerce 4 compactions: 1 nominal exitosa, 1 fallida (degraded), 2 más fallidas → circuit breaker → emergency.

### Gap 4: files_modified tiene check redundante [LOW]

simplified_loop.rs:296 tiene `!files_modified.contains(&tu.name)` que compara contra el nombre del tool, no contra el file path. Es redundante porque la línea 299 ya compara contra el path. No causa bugs pero es dead logic.

**Severidad:** Cosmética.

**Fix:** Eliminar la condición redundante.

---

## 6. Corrections Required

### Corrección 1: Emergency keep from budget

**Archivo:** tiered_compactor.rs, apply_emergency()
**Cambio:** `keep_count: 4` → `budget.keep_count.max(4)`
**Justificación:** Respeta el budget calculado sin ser peor que el mínimo actual.
**Impacto:** Emergency mode retiene más contexto en windows grandes (16 mensajes vs 4).
**Riesgo de no corregir:** Pérdida innecesaria de contexto en emergency.

### Corrección 2: Guard en verify_and_fix KeepReductionNeeded

**Archivo:** tiered_compactor.rs, verify_and_fix()
**Cambio:** Añadir `tracing::error!` si `messages.first()` no es Text.
**Justificación:** Previene silencio en un path anómalo.
**Impacto:** Mejor observabilidad de un escenario que no debería ocurrir.
**Riesgo de no corregir:** Fallo silencioso en un caso extremo.

### Corrección 3: files_modified check redundante

**Archivo:** simplified_loop.rs:296
**Cambio:** Eliminar `&& !files_modified.contains(&tu.name)`.
**Justificación:** Dead logic que confunde lectura.
**Impacto:** Claridad de código.
**Riesgo de no corregir:** Ninguno operacional.

### Corrección 4: Test de integración multi-nivel

**Archivo:** tiered_compactor.rs, tests
**Cambio:** Añadir test async que alterne providers mock (success → error → error → error → success check breaker).
**Justificación:** Verifica cascada completa.
**Impacto:** Confianza en el flujo de degradación.

---

## 7. What Is Already Frontier-Grade

| Componente | Por qué es frontier-grade |
|---|---|
| Budget model (S+P+K+R≤B) | Más riguroso que XIYO y que cualquier sistema público documentado. Post-verification con auto-corrección. |
| Progressive degradation (3 niveles) | Sin equivalente en XIYO (binario) ni en literatura pública. Monotonic information preservation verificada. |
| Utility-based abort | Previene compaction de beneficio negativo. Sin equivalente público conocido. |
| IntentAnchor | Separación estructural del intent, no dependiente de calidad del summary. Alineado con ACE paper y IMPACT framework. |
| Circuit breaker con degradation | Superior a XIYO (que desactiva compaction completamente). Emergency mode mantiene funcionalidad. |
| Tool result truncation como prerequisito | Alineado con Anthropic engineering blog (84% token reduction con context editing). |
| Runtime feature flag | `semantic_compaction = false` preserva behavior actual al 100%. Rollback seguro. |
| Backward compatibility | Todos los tests existentes pasan. Config deserializa sin problemas con campos nuevos. |
| Prompt de 9 secciones | Alineado con XIYO y con best practices de Anthropic. Incluye "Decisions Made" que XIYO omite. |
| Stream consumption robusta | Texto parcial pre-error = usable. Alineado con el principio de fail-safe. |

---

## 8. What Prevents AAA Frontier Status

| Blocker | Tipo | Esfuerzo | Fase |
|---|---|---|---|
| Emergency keep hardcoded a 4 | Bug de implementación | 1 línea | Ahora |
| verify_and_fix silencioso en edge case | Observabilidad | 3 líneas | Ahora |
| No test de integración multi-nivel | Cobertura | ~50 líneas | Ahora |
| files_modified check redundante | Limpieza | 1 línea | Ahora |
| No re-lectura de archivos post-compact | Gap vs XIYO | Fase 2 completa | Fase 2 |
| No multi-tier compaction (snip/microcompact) | Gap vs XIYO | Fase 2 completa | Fase 2 |
| No evaluation benchmark de calidad de summary | Gap vs literatura | Framework nuevo | Fase 1.5 |
| No persistencia a disco de tool results | Gap vs XIYO | Módulo nuevo | Fase 2 |

Los primeros 4 son corregibles inmediatamente. Los últimos 4 son evolución planificada y no bloquean Fase 1.

---

## 9. Completion Plan to Reach AAA

### Hardening Inmediato (hoy)

1. Fix emergency keep: `budget.keep_count.max(4)` — 1 línea
2. Guard en verify_and_fix: log si boundary message no es Text — 3 líneas
3. Fix files_modified check redundante — 1 línea
4. Test de integración multi-nivel — ~50 líneas

### Validación Experimental (semana 1 post-merge)

5. Activar `semantic_compaction = true` en dev local
6. Ejecutar sesiones de 30+ turnos con provider real
7. Verificar que summary contiene las 9 secciones
8. Verificar utility ratio > 0.3 en sesiones típicas
9. Verificar que degraded y emergency no se activan sin causa

### Fase 1.5 (semanas 2-3)

10. cancel_token wiring
11. diminishing returns adjustment (200 tokens, 3 rounds)
12. Evaluation mínimo: script que compara task completion pre/post compaction
13. Canary deployment 10%

### Fase 2 (mes 2)

14. Tool result persistencia a disco
15. File re-read post-compact (modelo XIYO)
16. Multi-tier compaction (snip, microcompact)
17. IntentAnchor dinámico
18. Evaluation benchmark formal

### Postergar

- Planner/executor separation
- Episodic memory / vector storage
- UCB1 strategy learning
- Unificación GDEM/simplified_loop

---

## 10. Final Corrective Prompt for Claude Code

```
Actúa como Senior Rust Engineer para Halcon CLI.

Aplica 4 correcciones menores a la implementación de Compaction
Semántica Fase 1. NO cambies la arquitectura. NO añadas features
de Fase 2. Solo corrige lo identificado en la auditoría frontier.

CORRECCIÓN 1: Emergency keep from budget
Archivo: crates/halcon-cli/src/repl/context/tiered_compactor.rs
En apply_emergency(), cambiar:
  let keep = self.inner.apply_compaction_with_keep_count(messages, &boundary, 4);
a:
  let keep = self.inner.apply_compaction_with_keep_count(
      messages, &boundary, budget.keep_count.max(4),
  );

CORRECCIÓN 2: Guard en verify_and_fix
Archivo: crates/halcon-cli/src/repl/context/tiered_compactor.rs
En verify_and_fix(), en el arm KeepReductionNeeded, después del
if let Some(msg) = messages.first(), añadir un else:
  else {
      tracing::error!("verify_and_fix: no boundary message found post-compaction");
  }
Y dentro del if let MessageContent::Text, añadir un else:
  else {
      tracing::error!("verify_and_fix: boundary message is not Text, cannot reduce keep");
  }

CORRECCIÓN 3: files_modified check redundante
Archivo: crates/halcon-cli/src/repl/agent/simplified_loop.rs
En el bloque de tracking de files_modified (después de tool execution),
cambiar:
  if (tu.name == "Edit" || tu.name == "Write" || tu.name == "file_write")
      && !files_modified.contains(&tu.name)
a:
  if tu.name == "Edit" || tu.name == "Write" || tu.name == "file_write"

El check de deduplicación del path ya ocurre en la línea siguiente.

CORRECCIÓN 4: Test de integración multi-nivel
Archivo: crates/halcon-cli/src/repl/context/tiered_compactor.rs
Añadir un test async llamado multi_level_degradation_cascade que:
1. Cree un TieredCompactor
2. Compacte con mock success → assert nivel Nominal
3. Compacte con mock error → assert nivel Degraded, failures=1
4. Compacte con mock error → assert nivel Degraded, failures=2
5. Compacte con mock error → assert nivel Degraded, failures=3
6. Compacte (circuit breaker open) → assert nivel Emergency
7. Compacte con mock success → assert circuit breaker SIGUE open
   (porque no se intenta nominal cuando breaker está open)
El test debe usar mensajes frescos (make_messages(30)) en cada
paso para evitar noop por keep >= messages.len().

DESPUÉS de las correcciones:
- cargo check --package halcon-cli
- cargo test --package halcon-cli --lib -- "tiered_compactor"
- cargo test --package halcon-cli --lib -- "repl::context::"
- cargo clippy --package halcon-cli --lib

Todos deben pasar sin errores.
```
