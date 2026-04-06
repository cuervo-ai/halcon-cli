# Compaction Semántica y Preservación de Intent — Fase 1

**Autor:** Oscar Valois
**Fecha:** 2026-04-03
**Estado:** Propuesta
**Revisores:** —

---

## Summary

El runtime agentic de Halcon destruye contexto operativo durante compaction. Cuando el context window se satura, el sistema reemplaza la historia completa con un placeholder estático (`"[Context compacted]"`) que no preserva intent, decisiones, progreso ni estado de trabajo. Esto provoca pérdida de objetivo, oscilación del agente, loops improductivos y halts prematuros.

Este diseño introduce tres capacidades que resuelven la causa raíz:

1. **Intent Anchor** — estructura inmutable que captura el objetivo original del usuario y sobrevive a todas las compactions.
2. **Compaction semántica** — invocación al LLM para generar resúmenes estructurados del contexto antes de descartarlo, siguiendo un prompt de 9 secciones que preserva intent, decisiones, errores, estado y siguiente paso.
3. **Re-inyección de contexto protegido** — restauración automática de intent, herramientas usadas y archivos modificados después de cada compaction.

**Queda fuera de esta fase:** compaction multi-nivel, tool result budgeting, fallback de provider, hook lifecycle, goal verification semántico, unificación de runtimes.

---

## Problem Statement

### Síntomas observados

- El agente pierde el objetivo del usuario después de compaction y oscila entre acciones inconexas.
- Repite trabajo ya completado porque no recuerda haberlo hecho.
- Contradice decisiones previas porque no tiene registro de ellas.
- Se detiene prematuramente por diminishing returns tras compaction (la compaction destructiva produce respuestas cortas que activan el halt).
- El usuario debe reiniciar manualmente y re-explicar la tarea.

### Impacto operacional

Toda tarea que consuma suficiente contexto para activar compaction (inevitable en tareas de más de ~30 turnos) sufre degradación. El sistema es auto-reforzante: compaction destructiva genera confusión, la confusión genera tokens desperdiciados, los tokens desperdiciados aceleran la siguiente compaction. Investigación externa cuantifica el efecto: reducción de 42% en task success rate y 216% más intervenciones humanas tras degradación sostenida del contexto (Agent Stability Index < 0.70).

### Por qué es arquitectónico

No es un bug local. El transformer degrada con más input tokens — esto es una propiedad estructural (validada por Chroma en 18 modelos frontier). La compaction es necesaria. El problema es que la implementación actual descarta información en vez de transformarla. El placeholder `"[Context compacted]"` tiene contenido semántico nulo: después de compaction, el agente opera con los últimos N mensajes sin historia, sin dirección, sin awareness de su propio estado.

XIYO (la implementación de producción del agente CLI de referencia) resuelve esto con un sistema de 5 niveles donde la autocompaction usa un agente forked con un prompt de 9 secciones que produce resúmenes semánticos ricos, re-inyecta 6 tipos de contexto protegido post-compaction, y persiste tool results grandes a disco antes de que contaminen el context window. Halcon no implementa ninguno de estos mecanismos.

---

## Goals

1. Eliminar compaction destructiva: todo evento de compaction produce un resumen semántico generado por LLM, no un placeholder.
2. Preservar el intent original del usuario a través de cualquier número de compactions mediante un Intent Anchor inmutable.
3. Restaurar contexto operativo esencial (intent, herramientas, archivos modificados) automáticamente después de cada compaction.
4. Cablear cancel_token para habilitar cancelación event-driven.
5. Reducir halts prematuros por falsos positivos de diminishing returns post-compaction.
6. Mantener backwards compatibility total: el sistema debe comportarse igual o mejor que el actual en todos los escenarios.

---

## Non-Goals

- Migrar a GDEM o unificar runtimes. GDEM está feature-gated, sin tests de integración y sin path de activación seguro.
- Implementar compaction multi-nivel (snip, microcompact, context collapse). Requiere cache infrastructure que no existe.
- Implementar tool result budgeting con persistencia a disco. Es Fase 2.
- Implementar fallback de provider real. El stub actual solo loguea warning; reemplazarlo requiere multi-provider infrastructure.
- Implementar goal verification semántico. Requiere desacoplar criterios del GDEM.
- Cablear hook_runner. Requiere definir lifecycle hooks primero.
- Implementar planner/executor separation. Es un cambio arquitectónico fundamental fuera del alcance de remediación.
- Reemplazar stagnation detection basada en hash por embedding similarity.
- Hacer el Intent Anchor dinámico (actualizarlo con feedback del usuario mid-session). Es Fase 2.

---

## Design Principles

**Preserve user intent.** El objetivo del usuario nunca debe desaparecer del contexto operativo, independientemente del número de compactions.

**Fail safe over aggressive compaction.** Si la compaction semántica falla, el sistema cae a placeholder — degradado pero funcional. Nunca peor que el estado actual.

**Semantic continuity over token-only heuristics.** Las decisiones de compaction y continuación deben basarse en preservación de significado, no solo en conteo de tokens.

**Incremental adoption.** Cada componente nuevo es independiente, testeable y desplegable por separado. La implementación puede detenerse en cualquier paso sin romper el sistema.

**Bounded blast radius.** Los cambios se limitan al path de compaction dentro del simplified_loop. No se modifica el control flow principal del loop, no se tocan boundaries de tool execution, no se altera el modelo de mensajes.

**Modular evolution without forced runtime unification.** El diseño habilita evolución futura (multi-tier, tool budgeting, goal verification) sin requerir migración a GDEM ni reescritura del loop.

---

## Proposed Architecture

### Vista general

La arquitectura introduce cuatro componentes nuevos que operan como un subsistema de compaction dentro del simplified_loop existente. No reemplazan el loop — extienden el punto de compaction actual con capacidades semánticas.

```
simplified_loop
├── IntentAnchor              (creado una vez al inicio, inmutable)
├── TieredCompactor           (orquestador de compaction)
│   ├── CompactionSummaryBuilder   (genera prompt semántico, parsea respuesta)
│   ├── ContextCompactor           (existente — tool pair safety, keep window)
│   └── ProtectedContextInjector   (re-inyecta contexto post-compact)
└── [resto del loop sin cambios]
```

### Componentes

#### Intent Anchor

**Responsabilidad:** Capturar y preservar el objetivo original del usuario de forma inmutable durante toda la sesión.

**Se crea:** Una vez, al inicio del loop, a partir del primer mensaje del usuario.

**Contiene:** Mensaje original verbatim, resumen del task (primeros 500 caracteres), archivos mencionados extraídos del mensaje, working directory, timestamp de creación.

**Se consume:** Por el CompactionSummaryBuilder (como input del prompt de compaction) y por el ProtectedContextInjector (como bloque re-inyectado post-compaction).

**Invariante que protege:** El intent original del usuario nunca desaparece del contexto operativo.

**Por qué existe:** La compaction actual descarta todos los mensajes antiguos incluyendo el request original. Post-compaction, el agente no tiene ninguna referencia al objetivo. XIYO resuelve esto con la sección "Primary Request and Intent" en el prompt de compaction y la re-inyección de "ALL user messages". El Intent Anchor es el equivalente estructural: una referencia fija que no depende de la calidad del resumen del LLM.

**Boundary:** Es una estructura de datos pasiva, read-only después de creación. No toma decisiones, no muta estado, no interactúa con el provider. Su único output es un bloque de texto formateado para inyección en prompts.

#### Compaction Summary Builder

**Responsabilidad:** Construir el prompt semántico para compaction y formatear la respuesta del LLM para inyección.

**Inputs:** Mensajes a compactar, Intent Anchor, tamaño de keep window.

**Outputs:** Prompt de compaction (string) y resumen formateado para inyección (string).

**Estructura del prompt:** 9 secciones, alineadas con el patrón validado en XIYO:
1. Primary Request and Intent — objetivos explícitos del usuario, con citas directas.
2. Key Technical Context — tecnologías, frameworks, patrones discutidos.
3. Files and Code — archivos examinados o modificados, con paths exactos.
4. Errors and Fixes — errores encontrados y resoluciones, verbatim.
5. Decisions Made — decisiones clave y su razonamiento.
6. User Feedback — correcciones, refinamientos, cambios de dirección del usuario.
7. Pending Tasks — trabajo solicitado pero no completado.
8. Current State — qué se estaba haciendo inmediatamente antes de la compaction.
9. Next Step — la siguiente acción más importante, con cita directa del request más reciente.

**Reglas del prompt:** Preservar todos los mensajes del usuario (no tool results). Incluir file paths exactos. Incluir códigos de error verbatim. Límite de ~2000 tokens en el output. No invocar herramientas.

**Invariante que protege:** La compaction nunca sustituye contexto por un placeholder sin semántica útil.

**Por qué existe:** El prompt de compaction ya existe en Halcon (`compaction_prompt()` en `compaction.rs:84-131`) pero nunca se invoca desde el loop de producción. Este componente lo reemplaza con un builder que sigue el patrón de 9 secciones de XIYO, probado en producción a escala.

**Relación con otros componentes:** Recibe el Intent Anchor como input para anclar el resumen al objetivo original. Su output es consumido por el TieredCompactor para reemplazar el placeholder.

#### Protected Context Injector

**Responsabilidad:** Construir mensajes de restauración de contexto para inyectar en la conversación después de cada compaction.

**Inputs:** Intent Anchor, lista de herramientas usadas en la sesión, lista de archivos modificados, estado de trabajo opcional.

**Outputs:** Uno o más mensajes de chat que restauran contexto operativo esencial.

**Contenido inyectado:**
- Intent Anchor formateado (siempre presente).
- Herramientas usadas en la sesión (extraídas de tool_use blocks en mensajes).
- Archivos modificados (extraídos de Edit/Write tool calls).
- Estado de trabajo pre-compaction (cuando disponible).
- Instrucción explícita de continuar sin repetir trabajo completado.

**Invariante que protege:** El agente mantiene awareness de su estado operativo después de compaction. No re-descubre herramientas, no repite ediciones, no pierde dirección.

**Por qué existe:** XIYO re-inyecta 6 tipos de attachment post-compaction (file attachments, async agents, plan, plan mode, invoked skills, deltas de tools/agents/MCP). Halcon no re-inyecta nada. El Protected Context Injector es una versión reducida y viable para Fase 1 que cubre los 3 tipos de mayor impacto: intent, herramientas y archivos.

**Boundary:** Solo produce mensajes. No muta el estado del loop. No interactúa con el provider. Es puro y testeable.

**Extension point:** En Fase 2, este componente puede extenderse para re-inyectar tool schemas, plan context, MCP instructions y session metadata sin cambiar su interface.

#### Tiered Compactor

**Responsabilidad:** Orquestar el proceso completo de compaction semántica con fallback y circuit breaker.

**Inputs:** Mensajes mutables, Intent Anchor, referencia al provider, budget del pipeline.

**Outputs:** `CompactionResult` — indica si se produjo resumen semántico o se cayó a placeholder.

**Flujo interno:**
1. Verificar circuit breaker. Si está abierto (≥3 fallos consecutivos), usar placeholder directamente.
2. Calcular keep window adaptativo (mismo `adaptive_keep_recent()` existente).
3. Construir prompt semántico via CompactionSummaryBuilder.
4. Invocar al provider con timeout de 30 segundos y temperature 0.
5. Si éxito: aplicar compaction con resumen semántico, reset del contador de fallos, inyectar contexto protegido via ProtectedContextInjector.
6. Si fallo: incrementar contador, loguear error, aplicar compaction con placeholder marcado como degradado.

**Invariantes que protege:**
- Compaction nunca bloquea el loop indefinidamente (timeout + circuit breaker).
- Fallo de compaction semántica nunca es peor que el estado actual (fallback a placeholder).
- Tool pair safety se preserva (delega a `safe_keep_boundary_n()` existente sin modificarlo).

**Por qué existe:** Necesitamos un coordinador que maneje la invocación al LLM, el fallback, el circuit breaker y la re-inyección como una unidad cohesiva. Sin este orquestador, la lógica de compaction semántica estaría dispersa en el simplified_loop.

**Relación con otros componentes:** Consume CompactionSummaryBuilder, ProtectedContextInjector e IntentAnchor. Envuelve (no reemplaza) el ContextCompactor existente para la mecánica de keep window y tool pair safety.

**Failure domain:** Aislado. Si el TieredCompactor falla completamente, el sistema cae a placeholder — comportamiento idéntico al actual. No propaga fallos al loop principal.

---

## Lifecycle / Flow

### Inicio de sesión

El simplified_loop recibe el request del usuario. Antes de entrar al loop principal, crea un Intent Anchor a partir del mensaje inicial. Este anchor es inmutable y vive en stack durante toda la ejecución del loop. También se instancia el TieredCompactor envolviendo al ContextCompactor existente.

### Ejecución normal (pre-compaction)

El loop opera sin cambios. Mensajes se acumulan: user, assistant, tool_use, tool_result. El context crece linealmente con cada turno. El Intent Anchor existe pero no se consulta — no tiene costo operativo.

### Trigger de compaction

Cuando el estimado de tokens alcanza el 85% del budget (ajustado desde 90% para dar margen a la generación del resumen), el loop delega al TieredCompactor.

### Durante compaction

1. El TieredCompactor verifica el circuit breaker.
2. Construye el prompt semántico incluyendo el Intent Anchor como ancla.
3. Invoca al provider con un request dedicado (max 2000 tokens, temperature 0, timeout 30s).
4. El provider genera un resumen estructurado en 9 secciones.
5. El ContextCompactor existente aplica la mecánica de compaction: calcula keep window, ejecuta `safe_keep_boundary_n()`, reemplaza mensajes descartados con el resumen (en vez del placeholder).

### Después de compaction

El ProtectedContextInjector genera mensajes de restauración y los append al final de la conversación. Estos mensajes contienen: el Intent Anchor formateado, las herramientas usadas, los archivos modificados, y una instrucción de continuidad. El agente recibe en su siguiente turno: resumen semántico + últimos N mensajes preservados + contexto protegido re-inyectado.

### Durante recuperación (compaction fallida)

Si la invocación al LLM falla (timeout, error de API, respuesta vacía): el contador de fallos se incrementa, se loguea el error con contexto, se aplica compaction con placeholder marcado como degradado (`"[Context compacted — summary unavailable]"`). El loop continúa. Si se acumulan 3 fallos consecutivos, el circuit breaker se abre y las compactions subsiguientes usan placeholder directamente sin intentar invocación.

### Preservación de continuidad

La continuidad semántica se preserva en tres capas redundantes:
- **Capa 1:** El resumen semántico contiene la historia condensada con citas directas del usuario.
- **Capa 2:** El Intent Anchor re-inyectado repite el objetivo original sin depender de la calidad del resumen.
- **Capa 3:** El contexto protegido re-inyectado restaura awareness operativo (herramientas, archivos).

Si la Capa 1 falla (resumen de baja calidad o placeholder), las Capas 2 y 3 mantienen un mínimo de continuidad. El agente puede perder historia detallada pero no pierde dirección ni estado operativo.

### Estado coherente post-compaction

Después de compaction, el estado de mensajes es:

```
[resumen semántico o placeholder]  ← contexto condensado
[mensajes recientes preservados]   ← keep window (4-20 mensajes)
[contexto protegido re-inyectado]  ← intent + tools + files
```

El agente ve: un resumen de lo que ocurrió, los mensajes recientes para continuidad inmediata, y el contexto protegido para dirección y awareness. Esto es un estado operativo coherente — no un vacío con un placeholder.

---

## Architectural Invariants

1. **El intent original del usuario nunca desaparece del contexto operativo.** El Intent Anchor se re-inyecta después de cada compaction. No hay path de ejecución donde el agente opere sin referencia al objetivo.

2. **La compaction nunca sustituye contexto por un placeholder sin semántica útil.** El path primario produce resumen semántico. El path de fallback produce placeholder explícitamente marcado como degradado. No existe un path que produzca un placeholder genérico silencioso.

3. **El contexto protegido sobrevive a cualquier compaction ordinaria.** La re-inyección es automática y obligatoria después de cada compaction — no es opt-in, no depende de configuración, no puede omitirse accidentalmente.

4. **Recovery no degrada más de lo que repara.** El circuit breaker previene loops de compaction fallida. El fallback a placeholder es idéntico al comportamiento actual — nunca peor. La re-inyección de contexto protegido se ejecuta incluso en fallback.

5. **El sistema se degrada de forma progresiva, no abrupta.** Compaction semántica → compaction con placeholder + contexto protegido → halt por exhaustion. Cada nivel preserva más que el anterior.

6. **El boundary entre contexto efímero y contexto protegido es explícito.** El Intent Anchor, las herramientas usadas y los archivos modificados son contexto protegido — sobreviven compaction. Todo lo demás es efímero — se condensa o descarta.

7. **Tool pair safety se preserva sin modificación.** `safe_keep_boundary_n()` sigue activo. El TieredCompactor delega al ContextCompactor existente para la mecánica de keep window.

8. **Backwards compatibility total.** CompactionConfig acepta los nuevos campos con defaults que reproducen el comportamiento actual. El feature gate `semantic-compaction` permite activación gradual.

---

## Alternatives Considered

### Unificar runtimes ya (GDEM + simplified_loop)

GDEM tiene 10 capas experimentales sin tests de integración con el runtime activo. Activarlo requiere migración completa y paridad funcional probada. El riesgo de regresión es alto y no resuelve el problema de compaction — GDEM tampoco tiene compaction semántica. Descartada por riesgo desproporcionado vs impacto.

### No compactar — aumentar budget de contexto

No viable. El transformer degrada con más input tokens independientemente del window size. Context rot es una propiedad estructural: más tokens no la resuelven, la postergan. Además, el costo escala linealmente con el budget y los providers tienen límites duros.

### Trimming heurístico sin LLM

Eliminar mensajes antiguos por reglas (edad, tipo, tamaño) sin generar resumen. Más barato que invocar al LLM pero pierde información irrecuperablemente. Es lo que el sistema hace hoy y es la causa del incidente.

### Resúmenes no estructurados (prompt genérico)

Invocar al LLM con un prompt genérico ("resume esta conversación") sin la estructura de 9 secciones. Produce resúmenes de calidad variable que omiten información operativa crítica (file paths, errores exactos, pending tasks). El prompt estructurado de XIYO existe porque los resúmenes genéricos no preservan lo que el agente necesita para continuar.

### Resolver solo con planner

Introducir un planner que mantenga estado separado del contexto. Cambio arquitectónico fundamental que requiere planner/executor separation, persistencia de plan, re-sincronización post-compaction. Es una dirección correcta a largo plazo pero desproporcionada para Fase 1 y no resuelve el problema inmediato de compaction destructiva.

### Posponer a fase futura

El incidente afecta a toda tarea larga. Posponer significa aceptar degradación conocida e inevitable. La remediación propuesta es incremental, de bajo riesgo, y cada componente es independiente. No hay justificación para posponer.

---

## Risks and Trade-offs

### Latencia de compaction semántica

La invocación al LLM añade 5-15 segundos al evento de compaction. **Mitigación:** Timeout de 30s con fallback a placeholder. Feedback al usuario durante compaction ("Compactando contexto..."). La compaction es infrecuente (1-3 veces por sesión larga) — el costo amortizado es bajo.

### Resúmenes de baja calidad

El LLM puede omitir información crítica o producir resúmenes genéricos a pesar del prompt estructurado. **Mitigación:** El Intent Anchor es independiente del resumen — el objetivo se preserva aunque el resumen sea pobre. El prompt de 9 secciones está validado en producción por XIYO. Temperature 0 reduce variabilidad.

### Sobrepreservación de contexto

El resumen semántico + contexto protegido re-inyectado puede consumir un porcentaje significativo del budget post-compaction, reduciendo el espacio útil. **Mitigación:** Cap de 2000 tokens en el resumen. El contexto protegido es ~200-500 tokens. El threshold se baja a 85% para dar margen. En el peor caso, el overhead es ~2500 tokens — menos del 5% de un window de 64K.

### Ambigüedad entre contexto protegido y efímero

Sin un boundary explícito en la estructura de mensajes, el agente podría tratar el contexto re-inyectado como input nuevo del usuario. **Mitigación:** El contexto protegido usa marcadores semánticos claros (`[ORIGINAL USER INTENT — DO NOT LOSE]`, `[POST-COMPACTION CONTEXT RESTORATION]`). El contenido es informativo, no directivo.

### Falsa sensación de continuidad

Un resumen que parece rico pero omite un detalle operativo crítico puede ser peor que un placeholder obvio: el agente actúa con confianza sobre información incompleta. **Mitigación:** El prompt exige file paths exactos, error codes verbatim, y citas directas. Los campos más propensos a omisión (pending tasks, next step) tienen secciones dedicadas. El Intent Anchor es un safety net independiente.

### Costo adicional por invocación LLM

Cada compaction añade una invocación (input: mensajes truncados, output: ~2000 tokens). **Mitigación:** 1-3 invocaciones por sesión larga. El costo es marginal comparado con el costo de la sesión principal. El beneficio (task completion vs abandono) justifica el costo.

### Acoplamiento del TieredCompactor con el provider

El TieredCompactor necesita acceso al provider para invocar el LLM. Esto introduce una dependencia en el path de compaction que antes no existía. **Mitigación:** La dependencia ya existe conceptualmente (el provider está disponible en el loop). El fallback a placeholder elimina la hard dependency — si el provider no está disponible, el sistema funciona igual que hoy.

---

## Rollout Strategy

### Feature gating

La compaction semántica se activa via feature flag `semantic-compaction`. Cuando el flag está desactivado, el sistema usa el path de placeholder actual sin cambios. Esto permite activación/desactivación sin deploy.

### Enablement gradual

1. **Dev local:** Activar flag, ejecutar sesiones de 30+ turnos, verificar que resúmenes preservan intent y estado.
2. **Staging:** Activar con monitoreo de latencia de compaction, calidad de resúmenes, y task completion post-compaction.
3. **Producción (canary):** Activar para un subconjunto de sesiones. Comparar métricas contra baseline de placeholder.
4. **Producción (full):** Activar para todas las sesiones después de validar métricas.

### Backward compatibility

- CompactionConfig acepta campos nuevos con defaults que reproducen comportamiento actual.
- El TieredCompactor es un wrapper sobre ContextCompactor existente — no lo reemplaza.
- Los mensajes post-compaction son mensajes de chat estándar — no requieren cambios en el modelo de datos.

### Fallback behavior

Si el flag se desactiva en producción: el sistema revierte automáticamente al path de placeholder. No requiere rollback de código, no deja estado inconsistente, no afecta sesiones en curso (la siguiente compaction usa el path configurado).

### Señales a observar

- **Latencia de compaction:** p50 y p99 de la invocación al LLM. Alerta si p99 > 30s.
- **Tasa de fallback:** Porcentaje de compactions que caen a placeholder. Alerta si > 20%.
- **Token overhead post-compaction:** Tokens consumidos por resumen + contexto protegido. Alerta si > 10% del budget.
- **Task completion post-compaction:** Proporción de sesiones que completan la tarea después de al menos una compaction. Comparar contra baseline.
- **Circuit breaker activations:** Frecuencia de apertura del circuit breaker. Alerta si > 5% de sesiones.

---

## Validation Strategy

### Criterios de aceptación

1. Después de compaction semántica, los mensajes contienen un resumen estructurado con las 9 secciones, no un placeholder.
2. El Intent Anchor está presente en el contexto post-compaction en todas las sesiones, independientemente del resultado de compaction.
3. El contexto protegido (herramientas, archivos) se re-inyecta automáticamente después de cada compaction.
4. Si la compaction semántica falla, el sistema cae a placeholder sin afectar el loop.
5. El circuit breaker se abre después de 3 fallos consecutivos.
6. Tool pair safety se preserva: no hay ToolResults huérfanos post-compaction.
7. cancel_token no es None en dispatch.

### Señales de éxito

- Reducción medible en oscilación post-compaction (el agente no repite acciones ya realizadas).
- Reducción en halts prematuros por diminishing returns post-compaction.
- El agente mantiene dirección coherente después de compaction en sesiones de 50+ turnos.
- El usuario no necesita re-explicar la tarea después de compaction.

### Failure signals

- Resúmenes que no contienen file paths exactos o que usan descripciones genéricas.
- Aumento en latencia de turno post-compaction > 20s de media.
- Tasa de fallback a placeholder > 30% en producción.
- Circuit breaker activándose en > 10% de sesiones (indica problema sistémico con la invocación).
- Regresión en tool pair safety (ToolResults huérfanos detectados).

### Observabilidad esperada

- Log estructurado en cada compaction: pre/post token count, tipo de resultado (semántico vs placeholder), latencia de invocación, número de secciones presentes en el resumen.
- Métrica de circuit breaker: estado (abierto/cerrado), contador de fallos consecutivos.
- Tracing span para la invocación de compaction, separado del span del turno principal.

---

## Open Questions

1. **¿Debe el Intent Anchor capturar refinamientos del usuario mid-session?** El diseño actual es estático (captura el primer mensaje). Si el usuario cambia de dirección a mitad de sesión, el anchor no lo refleja. Fase 2 puede introducir un IntentAnchor dinámico que se actualice con mensajes que indiquen cambio de objetivo. ¿Es esto necesario para Fase 1 o el resumen semántico (sección 6: User Feedback) es suficiente?

2. **¿Qué modelo usar para la invocación de compaction?** El diseño propone usar el provider actual (el mismo modelo que ejecuta la tarea). XIYO usa el mismo modelo con cache sharing. ¿Tiene sentido usar un modelo más pequeño/barato para compaction? Trade-off: menor costo vs menor calidad de resumen.

3. **¿Cuál es el cap adecuado para el resumen?** El diseño propone 2000 tokens. XIYO no tiene un cap explícito — el prompt pide brevedad pero el output puede variar. ¿2000 es suficiente para sesiones con mucho contexto técnico? ¿Es excesivo para sesiones simples?

4. **¿Cómo medir calidad del resumen de forma automatizada?** La validación manual es lenta. ¿Existe una métrica proxy para "el resumen preserva lo necesario"? Posibilidades: presencia de file paths, presencia de citas del usuario, presencia de pending tasks. Ninguna es completa.

5. **¿Debe el contexto protegido incluir el system prompt o tool schemas?** XIYO re-inyecta tool definitions y MCP instructions. En Halcon, el system prompt se envía en cada request por diseño. ¿Es redundante re-inyectar tool schemas o hay escenarios donde se pierden?

---

## Recommendation

Este diseño resuelve la causa raíz del incidente de compaction destructiva con el mínimo cambio estructural y el máximo impacto en continuidad semántica.

La combinación de Intent Anchor + compaction semántica + re-inyección de contexto protegido elimina los tres vectores del problema: pérdida de intent (Intent Anchor), pérdida de historia (resumen semántico), y pérdida de awareness operativo (contexto protegido). El fallback a placeholder y el circuit breaker garantizan que el sistema nunca es peor que el estado actual.

El diseño es el balance correcto entre corrección, riesgo y evolución:
- **Corrección:** Resuelve el problema raíz sin workarounds.
- **Riesgo:** Cada componente tiene fallback, es testeable independientemente, y el feature gate permite rollback instantáneo.
- **Evolución:** Los extension points (multi-tier compaction, tool budgeting, intent dinámico, goal verification) están diseñados pero no implementados. Fase 2 puede construir sobre esta base sin re-trabajo conceptual.

El path de implementación es: IntentAnchor → CompactionSummaryBuilder → ProtectedContextInjector → TieredCompactor → integración en loop → cancel_token → diminishing returns. Cada paso es autónomo. El sistema mejora con cada paso completado.
