# Revisión Técnica Crítica — Cuervo CLI

**Comité de Revisión:** Principal Software Architect, Research Scientist (LLMs), Staff Engineer, Product Strategist, Security & Compliance Lead
**Fecha:** 6 de febrero de 2026
**Clasificación:** Confidencial — Uso Interno
**Documentos Evaluados:** 13 documentos técnicos + 1 índice maestro

---

## RESUMEN EJECUTIVO

La documentación de Cuervo CLI presenta una visión ambiciosa y bien articulada. Sin embargo, esta revisión identifica **23 debilidades críticas**, **14 riesgos arquitectónicos**, y **9 oportunidades de mejora 10x** que deben abordarse antes de escribir la primera línea de código.

**Veredicto general:** La documentación sufre de tres problemas estructurales:
1. **Scope creep disfrazado de diferenciación** — Intentar 10 diferenciadores simultáneamente diluye la ejecución
2. **Arquitectura aspiracional sin validación** — Decisiones técnicas tomadas desde la teoría, no desde prototipos
3. **Subestimación sistemática de complejidad** — 47 requisitos Must en 16 semanas con 6-8 personas es inviable

---

# FASE 1: AUDITORÍA TÉCNICA PROFUNDA

## 1.1 Arquitectura (DDD + Clean Architecture)

### Hallazgo CRÍTICO: Sobre-ingeniería arquitectónica

**Problema:** Se propone una arquitectura DDD con 4 capas (Presentation, Application, Domain, Infrastructure), entidades formales, value objects, domain events, y aggregate roots para lo que es esencialmente una **aplicación de terminal single-user**.

**Evidencia:**
- `src/domain/entities/Agent.ts`, `Model.ts`, `Tool.ts`, `Session.ts`, `Task.ts` — 5 entidades de dominio formales
- `src/domain/value-objects/` — 4 value objects
- `src/domain/events/` — 3 categorías de domain events
- `src/domain/repositories/` — 3 interfaces de repositorio
- `src/domain/services/` — 3 domain services

**Análisis:** Claude Code, Copilot y Cursor — productos con millones de usuarios — **no usan DDD**. Usan arquitectura pragmática orientada a módulos. DDD está diseñado para sistemas con **dominio de negocio complejo** (banca, logística, healthcare), no para CLIs de desarrollo. El overhead cognitivo y de boilerplate de DDD ralentizará el desarrollo del MVP sin beneficio proporcional.

**Recomendación:** Reemplazar DDD formal con **arquitectura modular por feature**:
```
src/
├── core/          # Model gateway, agent loop, tool system
├── providers/     # Anthropic, OpenAI, Ollama adapters
├── tools/         # File ops, bash, git, search
├── agents/        # Agent types and orchestration
├── storage/       # SQLite, cache, embeddings
├── ui/            # REPL, rendering, prompts
└── config/        # Configuration, auth
```

**Impacto:** -40% boilerplate, +30% velocidad de desarrollo MVP, misma testabilidad con dependency injection simple (tsyringe o inversify-lite).

---

### Hallazgo ALTO: Contradicción Commander.js + Ink

**Problema (doc: `03-architecture/01-arquitectura-escalable.md`, línea 280-288):** Se selecciona Commander.js para CLI parsing e Ink (React para terminal) para rendering. Estos frameworks sirven paradigmas opuestos:
- Commander.js → CLI tradicional (parse args → execute → exit)
- Ink → TUI interactiva persistente (React component tree)

Un REPL interactivo con agentes, spinners, diffs, y tablas en tiempo real es fundamentalmente una aplicación Ink. Commander.js solo sirve para el entry point y mode dispatch (one-shot vs interactive).

**Riesgo adicional:** Ink carga React runtime completo. Esto impacta directamente:
- RNF-004: Startup time <500ms — React + JSX compilation + component tree init agrega ~200-400ms
- RNF-005: Memoria idle <100MB — React runtime + component tree + virtual DOM agrega ~30-50MB

**Recomendación:** Evaluar alternativas más ligeras:
- **@clack/prompts** + **ansi-escapes** para MVP (minimal footprint)
- **blessed-contrib** para TUI avanzada sin React overhead
- O aceptar Ink pero **ajustar targets de performance** realistamente (startup <1s, idle <150MB)

---

### Hallazgo ALTO: Ausencia de estrategia de gestión de contexto

**Problema:** Ningún documento aborda cómo manejar la **limitación fundamental** de ventanas de contexto. Los modelos tienen límites (Claude: 200K, GPT-4o: 128K, modelos locales 8B: 4-8K). Cuando una conversación + codebase context excede estos límites:
- ¿Cómo se comprime el contexto?
- ¿Qué mensajes se descartan?
- ¿Cómo se resume el historial?
- ¿Cómo se manejan las diferencias de límites entre modelos en un routing multi-modelo?

**Esto no es un nice-to-have.** Es el problema #1 de cualquier herramienta agéntica de codificación. Claude Code implementa compresión automática con summarización. Sin esta estrategia, sesiones largas simplemente fallan.

**Recomendación:** Diseñar un **Context Window Manager** como componente de primera clase:
1. Token counting preciso por provider (cada tokenizer es diferente)
2. Estrategia de compaction: summarize → trim old messages → keep system + recent
3. Sliding window con importance scoring (tool results > assistant > user history)
4. Cached summaries de conversación para re-hydration
5. Context budget allocation: X% para system prompt, Y% para historial, Z% para codebase context

---

### Hallazgo MEDIO: Plugin sandboxing indefinido

**Problema (doc: `03-architecture/01-arquitectura-escalable.md`, línea 99-101):** Se lista `PluginLoader.ts`, `PluginSandbox.ts`, `PluginRegistry.ts` pero no hay diseño del modelo de seguridad de plugins:
- ¿Qué APIs expone el sandbox?
- ¿Pueden los plugins ejecutar bash? ¿Leer archivos arbitrarios?
- ¿Cómo se aíslan los plugins entre sí?
- ¿V8 isolates? ¿Worker threads? ¿Procesos separados?

Un plugin malicioso con acceso a las mismas herramientas que los agentes puede exfiltrar código, ejecutar comandos arbitrarios, o comprometer API keys.

**Recomendación:** Definir antes de Beta:
- Capability-based security model (plugins declaran permisos, usuario aprueba)
- Worker thread isolation con API limitada
- Filesystem access restringido al directorio del proyecto
- Sin acceso directo a network — solo vía API proxy del host

---

## 1.2 Multi-Model Integration

### Hallazgo CRÍTICO: Abstracción "unificada" es una falacia

**Problema (doc: `03-architecture/02-integracion-modelos.md`, líneas 92-133):** Las interfaces `UnifiedRequest` y `UnifiedResponse` asumen que todos los modelos son intercambiables. En realidad:

| Feature | Claude | OpenAI | Gemini | Ollama |
|---------|--------|--------|--------|--------|
| Tool use format | Native `tool_use` blocks | `function_calling` | `functionDeclarations` | Varies by model |
| Streaming protocol | SSE con `content_block_delta` | SSE con `choices[0].delta` | SSE diferente | Custom streaming |
| System prompt | `system` field separado | `messages[0].role=system` | `systemInstruction` | `system` field |
| Extended thinking | `thinking` blocks nativos | N/A (o-series chain-of-thought es opaco) | N/A | N/A |
| Caching | Prompt caching nativo | No equivalente | Context caching diferente | N/A |
| Image input | `image` content blocks | `image_url` en messages | Inline blobs | Model-dependent |
| Max output tokens | Configurable separado | `max_completion_tokens` | `maxOutputTokens` | `num_predict` |

**Impacto:** El "adaptador" para cada provider no es un mapping trivial — es una **traducción semántica compleja** que requiere tests exhaustivos. El time estimate implícito (Sprint 3-4, 4 semanas) para implementar 2 providers es realista, pero el claim de 5+ providers en Beta requiere ~12 semanas adicionales de trabajo de integración.

**Recomendación:**
1. MVP: Solo Anthropic + Ollama (como está). No intentar abstracción unificada — usar adapters concretos.
2. Abstraer progresivamente: La interfaz unificada debe **emerger** de implementaciones concretas, no diseñarse a priori.
3. Aceptar que features provider-specific (extended thinking, prompt caching, structured outputs) se exponen como **capabilities opcionales**, no como lowest-common-denominator.

---

### Hallazgo ALTO: Model Router agrega latencia sin evidencia de valor

**Problema (doc: `02-requirements/03-arquitectura-alto-nivel.md`, líneas 189-227):** El Task Complexity Classifier se describe como un "lightweight local model" que clasifica cada request en 4 niveles (TRIVIAL/SIMPLE/MEDIUM/COMPLEX).

**Problemas concretos:**
1. Ejecutar un modelo de clasificación en cada request agrega **100-500ms de latencia** — violando el target de <200ms TTFT para autocompletado (RNF-001)
2. ¿Qué modelo local se usa para clasificación? ¿Un fine-tuned? ¿Un modelo general con prompt? Ninguno está especificado
3. La clasificación correcta de complejidad **es en sí misma una tarea compleja** — un modelo local 3B no puede determinar confiablemente si "fix the auth bug" requiere Opus o Haiku sin entender el codebase
4. No hay training data ni benchmarks para este clasificador

**Recomendación:** Reemplazar el ML classifier con **heurísticas deterministas** para MVP:
- Archivos mencionados > 3 → COMPLEX
- Palabras clave (architecture, refactor, migrate) → COMPLEX
- Slash commands → mapeo estático (/explain → SIMPLE, /commit → SIMPLE, feature request → COMPLEX)
- User override siempre disponible (`--model=opus`)
- Iterar hacia ML classifier solo cuando haya datos de producción para entrenar

---

### Hallazgo ALTO: Token counting es un problema no trivial ignorado — **RESUELTO**

**Problema original:** Cada provider usa tokenizers diferentes. Los documentos no especificaban cómo hacer conteo preciso pre-request.

**Resolución:** Se diseñó el módulo **tokenizer.rs** (Rust/napi-rs) como parte de la Rust Performance Layer (ver `01-research/04-rust-performance-layer.md` y `03-architecture/02-integracion-modelos.md` sección 6):
- **tiktoken-rs** para conteo exacto de tokens OpenAI (cl100k_base, o200k_base)
- Estimadores calibrados para Claude (~±5%) y Gemini (~±8%)
- Heurística ultra-rápida (~5ns) para estimaciones rough
- Latencia: ~0.1ms/1K tokens (vs ~1ms/1K en js-tiktoken WASM)
- Fallback automático a `js-tiktoken` (WASM) si el binario nativo no está disponible
- Margin of safety del 10% integrado en todas las estimaciones

**Estado:** Diseñado para fase Beta. Integrado en la arquitectura del Model Gateway.

---

## 1.3 Almacenamiento y RAG

### Hallazgo ALTO: sqlite-vec es software experimental — **RESUELTO**

**Problema original (doc: `03-architecture/01-arquitectura-escalable.md`):** Se proponía "SQLite con vec extension" para embeddings locales. sqlite-vec es pre-1.0 y experimental.

**Resolución (ADR-009):** Se adoptó **LanceDB** como vector store primario basado en investigación profunda (ver `01-research/04-rust-performance-layer.md`). LanceDB ofrece:
- Core en Rust con bindings napi-rs nativos para Node.js
- Formato columnar Lance con memory-mapped I/O (zero-copy reads)
- IVF-PQ + DiskANN para indexación ANN de alta performance
- Full-text search integrado vía Tantivy (motor de búsqueda Rust)
- Hybrid search nativo (ANN + FTS con Reciprocal Rank Fusion)
- **USearch** como fallback (C++ HNSW, bindings Node.js oficiales, ~0.1ms search)

**Estado:** Integrado en la arquitectura. Documentación actualizada en secciones 2 y 3.

---

### Hallazgo MEDIO: Incremental indexing no diseñado

**Problema:** La indexación RAG descrita (Tree-sitter → AST → Chunking → Embedding → Storage) es batch-oriented. Para un codebase de 100K archivos (RNF-007):
- Indexación completa podría tomar 30-60 minutos (embedding ~50ms per chunk × ~200K chunks)
- No hay estrategia de indexación incremental (solo archivos modificados)
- No hay invalidación de cache (¿qué pasa cuando se edita un archivo indexado?)
- No hay priorización (indexar archivos relevantes al contexto actual primero)

**Recomendación:** Diseñar indexación incremental desde el inicio:
1. File watcher (chokidar) para detectar cambios
2. Content-hash based invalidation (ya existe `content_hash` en schema — usarlo)
3. Lazy indexing: indexar solo cuando se necesite para una búsqueda
4. Priority queue: archivos abiertos/mencionados se indexan primero

---

## 1.4 Sistema de Agentes

### Hallazgo ALTO: Agentes sin protocolo de comunicación definido

**Problema:** La arquitectura describe 4 tipos de agentes (Explorer, Planner, Executor, Reviewer) con un Orchestrator que los coordina. Pero no define:
- ¿Cómo pasa contexto el Orchestrator a un sub-agente? ¿Full conversation history? ¿Summary?
- ¿Cómo reporta un sub-agente resultados parciales?
- ¿Qué pasa si un Executor falla a mitad de una implementación multi-archivo?
- ¿Cómo evitar loops (Planner → Executor → tests fail → re-plan → Executor → fail again)?
- ¿Hay límite de recursión? ¿Budget de tokens por agent chain?

**Recomendación:** Definir un **Agent Communication Protocol**:
```typescript
interface AgentHandoff {
  fromAgent: AgentType;
  toAgent: AgentType;
  context: {
    summary: string;          // Compressed context
    relevantFiles: string[];  // Files discovered/modified
    plan?: string;            // If from Planner
    errors?: string[];        // If retry
  };
  constraints: {
    maxTokenBudget: number;
    maxTurns: number;         // Prevent infinite loops
    allowedTools: string[];   // Least privilege
  };
}
```

---

### Hallazgo MEDIO: Agentes MVP sin Orchestrator es incoherente

**Problema (doc: `02-requirements/01-requisitos-funcionales-no-funcionales.md`):**
- RF-401 (Orchestrator) = Must / **Beta**
- RF-402/403/404 (Explorer, Planner, Executor) = Must / **MVP**

Esto significa que en MVP hay agentes especializados pero **no hay orquestador que los coordine**. ¿Quién decide cuándo usar Explorer vs Planner vs Executor? ¿El usuario manualmente?

**Recomendación:** Dos opciones:
1. Mover RF-401 a MVP (recomendado) — sin Orchestrator los agentes no son usables
2. En MVP, el REPL principal actúa como orchestrator implícito (un solo agent loop, sin delegation) — más honesto sobre el alcance real

---

## 1.5 Seguridad

### Hallazgo ALTO: Bash sandboxing sin diseño técnico

**Problema:** Múltiples documentos mencionan "sandboxed execution" como feature crítica (RNF-303, RF-304), pero no hay diseño del mecanismo de sandbox:
- ¿Containers (Docker)? → Agrega dependencia y ~2s startup latency por command
- ¿bubblewrap/firejail? → Solo Linux, no macOS
- ¿Namespace isolation (nsjail)? → Linux only
- ¿Process-level restrictions (macOS Sandbox)? → Platform-specific

**Realidad de Claude Code:** Claude Code ejecuta bash **sin sandbox real**. Depende del human-in-the-loop (permission prompt) y del principle of least surprise. Es pragmático, no seguro en el sentido formal.

**Recomendación:**
1. MVP: Seguir el modelo Claude Code — permission prompt + allowlist/denylist de comandos + working directory restriction
2. Beta: Evaluar `firejail` (Linux) y `sandbox-exec` (macOS) como capas opcionales
3. No prometer "sandboxed" en marketing sin implementación real — usar "controlled execution with user approval"

---

### Hallazgo MEDIO: PII detection pipeline requiere NER model no especificado

**Problema (doc: `05-security-legal/02-privacidad-datos.md`, líneas 172-197):** El pipeline PII incluye un paso "NER (Named Entity Recognition)" para detectar nombres de personas y organizaciones. Esto requiere:
- Un modelo NLP/ML ejecutándose localmente
- ~50-200MB de memoria adicional
- ~100-500ms de latencia por invocación
- No hay modelo específico mencionado (spaCy? transformers? regex-based?)

**Recomendación:**
1. MVP: Solo regex-based detection (emails, IPs, API keys, credit cards) — cubre ~80% de casos
2. Beta: Agregar NER con spaCy small model (`en_core_web_sm`, ~15MB) como opt-in
3. Ser transparente en docs: "PII detection cubre patrones comunes; contenido semántico requiere mode avanzado"

---

## 1.6 Roadmap y Viabilidad

### Hallazgo CRÍTICO: MVP scope inviable en timeline propuesto

**Problema:** El MVP define 47 requisitos Must-Have en 16 semanas (8 sprints de 2 semanas) con un equipo de 6-8 personas.

**Cálculo de realidad:**
- 47 requisitos / 8 sprints = ~6 requisitos por sprint
- Incluye: REPL completo, 2 model providers, file ops sandboxed, git integration, permission system, auth client (cuervo-auth-service), config system, instalación npm/brew, docs en 2 idiomas, >80% test coverage, CI/CD pipeline
- Esto es comparable al **scope total de Claude Code 1.0**, que fue desarrollado por un equipo de ~15+ ingenieros de Anthropic

**Requisitos que deberían deferirse a post-MVP:**
| Requisito | Razón para diferir |
|-----------|-------------------|
| RF-801/802 (Auth con cuervo-auth-service) | Un CLI local no necesita auth centralizada para funcionar |
| RNF-503 (Docs en español + inglés) | Empezar con un idioma, traducir después |
| RF-505 (Hooks system) | Nice-to-have para MVP |
| RF-402/403/404 (Agentes especializados) | Un solo agent loop es suficiente para MVP |
| RNF-001 (TTFT <200ms local) | Target demasiado agresivo para MVP |
| RF-010 (Modo verbose/debug) | Standard logging es suficiente |

**Recomendación:** Reducir MVP a **30 requisitos core**. Definir un **MVP-0 (8 semanas)** con: REPL + Claude adapter + file ops + git basics + config. Luego MVP-1 (8 semanas) con: Ollama + permission system + polish + tests. Esto duplica la probabilidad de entregar algo funcional.

---

# FASE 2: REVISIÓN CIENTÍFICA vs ESTADO DEL ARTE

## 2.1 Gaps en el Análisis Competitivo

### Hallazgo ALTO: Competidores clave omitidos

El estado del arte omite herramientas significativas:

| Herramienta | Por qué importa | Gap en documentación |
|-------------|-----------------|---------------------|
| **Cline** (VS Code extension, open source) | CLI-like agent en VS Code, multi-model, 50K+ stars — competidor directo en open source | No mencionada |
| **Void** (open source AI IDE) | Fork open source de Cursor — amenaza directa al claim "open source core" | No mencionada |
| **bolt.new / Lovable** | Cloud agents que generan apps completas — paradigma "describe to deploy" | No mencionada |
| **v0.dev (Vercel)** | UI generation from prompt — tendencia de vertical AI tools | No mencionada |
| **Cody (Sourcegraph)** | Enterprise code intelligence con RAG avanzado — competidor en enterprise RAG | Mencionada superficialmente |
| **Zed AI** | Editor nativo con AI integration — trend de AI-native editors | No mencionada |

**Impacto:** El landscape competitivo está subestimado. La conclusión "no hay solución multi-modelo self-hosted" ignora que **Cline ya es multi-modelo y open source**, y que **Ollama + cualquier herramienta** ya ofrece self-hosted básico.

---

### Hallazgo ALTO: SWE-bench claims sin soporte

**Problema (doc: `01-research/01-estado-del-arte-2026.md`, línea 263):** "Top agents: 40-50%+ resolución autónoma" y target propio de 30% MVP, 40% Beta, 50% GA.

**Realidad:** SWE-bench Lite verified (enero 2026 estimado):
- Claude Code con Opus: ~50-55%
- Devin: ~45-50%
- AutoCodeRover: ~35-40%
- Aider: ~30-35%

**Cuervo CLI targetea 30% en MVP** — esto requiere capacidades agénticas comparables a Aider, que tiene años de desarrollo. Con un CLI nuevo y 16 semanas, un target realista para MVP es **15-20%** (resolver issues triviales de un solo archivo).

**Recomendación:** Ajustar targets de SWE-bench a: MVP 15%, Beta 30%, GA 40%. Estos siguen siendo ambiciosos pero no prometen lo impromisible.

---

### Hallazgo MEDIO: Análisis de costos superficial

**Problema (doc: `01-research/03-comparativa-rendimiento-costos.md`):** Las proyecciones de costos de API no incluyen **unit economics realistas por usuario activo**.

**Cálculo básico:**
- Desarrollador promedio: ~50 requests/día de chat
- Request promedio: ~5K tokens input + ~2K output (incluyendo context)
- Con Claude Sonnet: ~$0.015/request → **$0.75/usuario/día → $22.50/mes**
- Target MRR de $100K con 20K MAU → $5/usuario/mes → **Revenue no cubre costos de API**

**Recomendación:** El modelo de negocio necesita:
1. Semantic caching agresivo (reducir requests de API ~40%)
2. Modelo routing que envíe ~60% de requests a modelos locales o baratos
3. Tier pricing que refleje costos reales ($0-20 free, $20-50 pro, custom enterprise)
4. Incluir análisis detallado de unit economics en la documentación

---

## 2.2 Tecnologías y Tendencias No Cubiertas

### Hallazgo ALTO: Sin estrategia para Computer Use / Browser Agents

**Tendencia 2026:** Los agentes de codificación están expandiéndose a interacciones con browser y GUI:
- Claude Computer Use (control de desktop)
- Puppeteer/Playwright para testing automatizado via agentes
- Screenshot → code (UI generation)

El documento de integración de modelos no menciona computer use como capability. Para un CLI que compite con herramientas de desarrollo, la capacidad de un agente para:
- Verificar visualmente que una UI se ve correcta
- Ejecutar tests E2E con browser automation
- Interactuar con tools web (Jira, GitHub web UI) directamente

es un diferenciador emergente.

**Recomendación:** Incluir como feature de Beta/GA: "Agent Visual Verification" — el agente puede tomar screenshots y verificar que los cambios de UI se ven correctos.

---

### Hallazgo MEDIO: No hay estrategia de structured outputs

**Tendencia 2026:** OpenAI structured outputs, Anthropic tool use con schemas estrictos, y JSON mode están transformando la confiabilidad de outputs de modelos. La documentación no diseña:
- Cómo forzar outputs estructurados para tool calls
- Validación de schemas en tool call responses
- Recovery cuando el modelo produce output malformed

**Recomendación:** Diseñar un `OutputValidator` que:
1. Defina JSON schemas para cada tool call
2. Use structured outputs nativos cuando el provider lo soporte
3. Retry con re-prompt cuando el output no valide
4. Log malformed outputs para debugging

---

### Hallazgo MEDIO: MCP Protocol como afterthought

**Problema:** MCP (Model Context Protocol) se lista como RF-309 (Should/Beta), pero MCP está emergiendo como el **estándar de interoperabilidad** entre herramientas de IA y data sources. Claude Code ya implementa MCP servers/clients como feature central.

**Recomendación:** Elevar MCP a Must/MVP. Razones:
1. Permite que Cuervo CLI consuma tools de terceros inmediatamente (ya hay 100+ MCP servers)
2. Permite que otros tools consuman Cuervo CLI tools
3. Reduce la necesidad de implementar integraciones custom
4. Es el estándar que Anthropic está promoviendo activamente

---

# FASE 3: EVALUACIÓN DE ESTRATEGIA DE PRODUCTO

## 3.1 Análisis de la Propuesta de Valor

### Hallazgo CRÍTICO: 10 diferenciadores = 0 diferenciadores

**Problema:** La documentación lista 10 diferenciadores simultáneos:
1. Multi-modelo
2. Modelos locales
3. Self-hosted
4. Open source core
5. Plugin ecosystem
6. Fine-tuning integrado
7. LATAM support
8. Multi-agent
9. Offline mode
10. Compliance by design

**Ningún startup puede ejecutar 10 diferenciadores simultáneamente.** Cada uno de estos requiere inversión significativa. El resultado más probable es mediocridad en todos vs excelencia en 2-3.

**Análisis de defensibilidad real:**

| Diferenciador | ¿Es realmente diferente? | ¿Es defendible? |
|---------------|------------------------|-----------------|
| Multi-modelo | Copilot y Cursor ya lo hacen | No — table stakes |
| Modelos locales | Cline + Ollama ya existe | No — fácil de copiar |
| Self-hosted | Legítimamente escaso | **Sí** — requiere infraestructura |
| Open source core | Cline, Continue, Aider ya existen | No — mercado crowded |
| Plugin ecosystem | Todos los competidores lo tienen o planean | No — follower, no leader |
| Fine-tuning integrado | Escaso en CLIs de desarrollo | **Parcial** — complejo de copiar |
| LATAM support | Modelos ya entienden español | **Débil** — no es barrera técnica |
| Multi-agent | Claude Code ya lo implementa | No — table stakes 2026 |
| Offline mode | Legítimamente escaso en calidad | **Sí** — si los modelos locales son buenos |
| Compliance by design | Legítimamente escaso en CLIs | **Sí** — requiere inversión significativa |

**Diferenciadores realmente defendibles: Self-hosted + Compliance + Offline quality**

**Recomendación:** Reducir a **3 pilares de diferenciación**:
1. **"Works Everywhere"** — Online, offline, self-hosted, air-gapped. Tu código nunca sale de tu perímetro si no quieres.
2. **"Enterprise-Ready from Day 1"** — Compliance (EU AI Act, SOC 2, GDPR), audit logging, zero-retention, SSO. Lo que los competidores agregan como afterthought, Cuervo lo tiene by design.
3. **"Your Models, Your Rules"** — Multi-model routing donde TÚ decides qué modelo para qué tarea, con transparencia total de costos.

---

### Hallazgo ALTO: "LATAM-first" no es ventaja técnica

**Problema:** El diferenciador "LATAM-first" se reduce a:
- Documentación en español (cualquiera puede traducir docs)
- Modelos que "entienden español" (Claude, GPT, Gemini ya lo hacen nativamente)
- Soporte para LGPD (un checkbox de compliance)

**Esto no es un moat.** Cualquier competidor puede localizar en 2-4 semanas. La oportunidad real en LATAM es **distribución y relaciones**, no tecnología.

**Recomendación:** Reposicionar LATAM como **estrategia de go-to-market**, no como diferenciador de producto. El producto debe ser globally competitive; la distribución puede ser LATAM-first.

---

### Hallazgo MEDIO: Modelo de pricing no validado

**Problema (doc: `06-deliverables/resumen-ejecutivo-consolidado.md`, línea 130-134):**
- Target: 20K MAU, $100K MRR → $5/usuario/mes de revenue promedio
- Pero costo de API por usuario activo: ~$15-25/mes (ver cálculo Fase 2)
- Gross margin negativo en modelo actual

**Recomendación:** Diseñar pricing model concreto:
| Tier | Precio | Incluye | Margen estimado |
|------|--------|---------|-----------------|
| **Free** | $0 | Ollama only, sin cloud models | 100% (costo $0) |
| **Pro** | $25/mes | Cloud models, 200 requests/día | ~20% |
| **Team** | $45/user/mes | Pro + shared config + audit logs | ~40% |
| **Enterprise** | Custom ($100+/user) | Self-hosted, SSO, SLA, compliance | ~60% |

---

## 3.2 Ideal Customer Profile (ICP)

### Hallazgo ALTO: ICP no definido

Ningún documento define el ICP concreto. "Desarrolladores" y "Enterprise LATAM" es demasiado amplio.

**ICP recomendado para MVP/Beta (priorizar):**

**Primario:** Desarrollador individual que:
- Ya usa CLI tools (git, docker, npm) cómodamente
- Ha probado Claude Code o Aider pero quiere control sobre el modelo
- Tiene una GPU decente (MacBook M1+ o NVIDIA con 8GB+ VRAM)
- Valora privacidad o trabaja con código propietario sensible

**Secundario (Beta):** Tech lead en equipo de 5-15 devs que:
- Necesita herramientas aprobadas por security/compliance
- Opera en industria regulada (fintech, healthtech, gobierno)
- Quiere estandarizar el uso de AI coding tools en su equipo

---

# FASE 4: FEATURES DIFERENCIADORES E INNOVACIÓN

## 4.1 Propuestas de Mejora 10x

### PROPUESTA 1: Cuervo Memory Graph (Conocimiento Persistente del Proyecto)

**Problema que resuelve:** Cada sesión de un AI coding assistant empieza de cero. Claude Code tiene `CLAUDE.md` para memoria declarativa, pero no aprende del uso.

**Propuesta:** Un **knowledge graph persistente** del proyecto que se enriquece automáticamente:

```
MEMORY GRAPH:
├── Architecture Map
│   ├── Components discovered via exploration
│   ├── Dependencies between modules
│   └── Data flow patterns observed
├── Developer Preferences
│   ├── Coding style (observado, no configurado)
│   ├── Framework preferences
│   ├── Review feedback patterns
│   └── Common commands/workflows
├── Project History
│   ├── Bugs fixed and their root causes
│   ├── Refactorings performed
│   ├── Decisions made and rationale
│   └── Failed approaches (anti-patterns for this project)
└── Team Knowledge (Enterprise)
    ├── Code ownership map
    ├── Review preferences per team member
    └── Common questions from new developers
```

**Implementación:** Graph storage en SQLite (adjacency list), enriched via domain events. Cada tool execution, model response, y user feedback incrementally updates the graph.

**Ventaja 10x:** Después de 1 semana de uso, Cuervo "conoce" el proyecto tan bien como un developer senior. Ningún competidor tiene esto.

**Esfuerzo:** ~4-6 semanas de desarrollo. Target: Beta.

---

### PROPUESTA 2: Self-Healing Agent Loop

**Problema que resuelve:** Los agentes actuales fallan silenciosamente o se atoran en loops. Cuando un test falla después de un cambio, la mayoría de tools requieren intervención manual para diagnosticar y re-intentar.

**Propuesta:** Un agent loop que **detecta fallos y se auto-corrige**:

```
SELF-HEALING LOOP:
1. Agent executes change
2. Validation gate: tests, lint, type check
3. IF pass → commit/continue
4. IF fail → analyze failure
   a. Parse error output
   b. Compare with original intent
   c. Classify: fixable vs needs human
   d. IF fixable → auto-fix (max 3 attempts)
   e. IF needs human → explain clearly + suggest options
5. Exponential backoff on retries
6. Budget limit prevents infinite loops
```

**Clave:** El agent no solo re-intenta — **analiza por qué falló** y adapta su approach. Si el primer intento usó un API que no existe, el segundo intento busca la API correcta en el codebase.

**Ventaja 10x:** Tasa de éxito end-to-end pasa de ~30% a ~50% sin intervención humana, comparable a los mejores agents del mercado.

**Esfuerzo:** ~3-4 semanas. Target: MVP (integrado en agent loop core).

---

### PROPUESTA 3: Cost-Aware Intelligent Routing

**Problema que resuelve:** Los desarrolladores no saben cuánto cuesta cada request, y los modelos caros se usan para tareas triviales.

**Propuesta:** Routing que **muestra costos en tiempo real y optimiza automáticamente**:

```
┌─────────────────────────────────────────┐
│ cuervo> Explain this function           │
│                                         │
│ 🔀 Routing: llama-3.1-8b (local, $0)   │
│    Reason: Simple explanation task      │
│    Alternative: sonnet ($0.01) [enter]  │
│                                         │
│ [Explanation appears here...]           │
│                                         │
│ Session cost: $0.03 | Budget: $9.97/day │
└─────────────────────────────────────────┘
```

**Diferenciador vs competencia:**
- Claude Code: Muestra tokens pero no costo en tiempo real
- Copilot: Costo oculto en subscription
- Cursor: Similar a Copilot

**Ventaja 10x:** Transparencia total de costos + optimización automática. Reduce API costs 40-60% vs uso naive.

**Esfuerzo:** ~2-3 semanas. Target: MVP.

---

### PROPUESTA 4: Proyecto "Snapshot & Rollback"

**Problema que resuelve:** Cuando un agente hace cambios extensos que resultan incorrectos, el rollback es manual y doloroso (git stash, checkout, etc.).

**Propuesta:** Snapshots automáticos antes de cada operación agéntica:

```bash
cuervo> Implement OAuth2 authentication
# [Cuervo creates automatic snapshot: snap_2026-02-06_14:30]
# [Agent makes changes across 8 files...]
# [Tests fail]

cuervo> /rollback
# Reverted to snap_2026-02-06_14:30
# 8 files restored to pre-change state
# Working tree is clean

cuervo> /snapshots
# snap_2026-02-06_14:30  "Pre: Implement OAuth2"  [8 files]
# snap_2026-02-06_13:45  "Pre: Fix auth bug"      [2 files]
```

**Implementación:** Lightweight — git stash equivalent con metadata. No requiere infra nueva.

**Ventaja 10x:** Elimina el miedo de dejar al agente trabajar de forma autónoma. Los developers serán más aventureros con los agents si saben que el rollback es trivial.

**Esfuerzo:** ~1-2 semanas. Target: MVP.

---

### PROPUESTA 5: "Explain the Diff" Mode

**Problema que resuelve:** Code review de cambios hechos por IA es tedioso. Los developers aprueban cambios sin entenderlos.

**Propuesta:** Cada diff generado por un agente incluye **explicación inline contextual**:

```diff
// auth.service.ts
+ import { OAuth2Client } from 'google-auth-library';
  // 📝 Added Google OAuth2 client. Chosen over passport-google
  //    because the project already uses google-auth-library in
  //    src/integrations/calendar.ts (line 23)

+ async validateGoogleToken(token: string): Promise<UserPayload> {
+   const ticket = await this.client.verifyIdToken({
+     idToken: token,
+     audience: this.configService.get('GOOGLE_CLIENT_ID'),
+   });
  // 📝 Using verifyIdToken instead of getTokenInfo because it
  //    validates the audience claim, preventing token confusion
  //    attacks (OWASP API Security #5)
```

**Ventaja:** Convierte code review en **learning experience**. Developers entienden no solo QUÉ cambió sino POR QUÉ.

**Esfuerzo:** ~2 semanas. Target: MVP.

---

## 4.2 Tabla de Propuestas con Impacto/Esfuerzo

| # | Propuesta | Impacto | Esfuerzo | ROI | Fase |
|---|-----------|---------|----------|-----|------|
| P1 | Memory Graph | 🔴 Alto | 🟡 Medio (4-6 sem) | Alto | Beta |
| P2 | Self-Healing Agent | 🔴 Alto | 🟡 Medio (3-4 sem) | Muy Alto | MVP |
| P3 | Cost-Aware Routing | 🟡 Medio | 🟢 Bajo (2-3 sem) | Muy Alto | MVP |
| P4 | Snapshot & Rollback | 🟡 Medio | 🟢 Bajo (1-2 sem) | Muy Alto | MVP |
| P5 | Explain the Diff | 🟡 Medio | 🟢 Bajo (2 sem) | Alto | MVP |

---

# FASE 5: ROADMAP REPRIORIZADO

## 5.1 Matriz Impacto vs Esfuerzo (Requisitos Existentes)

```
                    ALTO IMPACTO
                         │
    ┌────────────────────┼────────────────────┐
    │                    │                    │
    │  QUICK WINS        │  STRATEGIC BETS    │
    │  ★ Do First        │  ★★ Plan Carefully │
    │                    │                    │
    │  • Snapshot/Rollback│  • Memory Graph    │
    │  • Cost display    │  • Self-healing    │
    │  • Explain diff    │  • Plugin system   │
    │  • File ops (R/W/E)│  • RAG + indexing  │
    │  • Basic git       │  • Multi-agent     │
    │  • REPL core       │  • Model routing   │
    │                    │                    │
────┼────────────────────┼────────────────────┼──
    │                    │                    │
    │  FILL-INS          │  MONEY PITS        │
    │  ☆ Do If Time      │  ✗ Avoid/Defer     │
    │                    │                    │
    │  • Verbose/debug   │  • Fine-tuning pipe│
    │  • Themes          │  • Self-hosted full│
    │  • Fish shell      │  • ISO certif.     │
    │  • tmux compat     │  • 5+ providers    │
    │  • GitLab/Bitbucket│  • Plugin marketplace│
    │                    │  • SSO/SAML        │
    │                    │  • DDD formal      │
    │                    │                    │
    └────────────────────┼────────────────────┘
                         │
                    BAJO IMPACTO
         BAJO ESFUERZO ──┼── ALTO ESFUERZO
```

## 5.2 Roadmap Repriorizado

### MVP-0: "Walking Skeleton" (Semanas 1-8)

**Objetivo:** Un CLI que funciona end-to-end con un modelo, probando la arquitectura core.

| Sprint | Entregable | Criterio |
|--------|-----------|----------|
| S1 (sem 1-2) | **REPL Core** | Input/output loop, markdown rendering, basic commands (/help, /clear, /exit) |
| S2 (sem 3-4) | **Claude Adapter + Agent Loop** | Single agent loop con Claude Sonnet, streaming, tool use |
| S3 (sem 5-6) | **Tool System** | File read/write/edit, glob, grep, basic bash (with permission prompt) |
| S4 (sem 7-8) | **Git + Config + Snapshot** | Git status/diff/commit, .cuervo/config.yml, snapshot/rollback |

**Exit criteria:** Un developer puede instalar via npm, configurar API key, y usar Cuervo CLI para implementar una feature simple en un proyecto existente.

**NOT included:** Auth, multiple providers, agents, plugins, hooks, i18n, vectors, RAG.

### MVP-1: "Competitive Parity" (Semanas 9-16)

**Objetivo:** Feature parity básica con Aider — multi-model, modelos locales, developer-ready.

| Sprint | Entregable | Criterio |
|--------|-----------|----------|
| S5 (sem 9-10) | **Ollama Adapter + Cost Display** | Modelos locales funcionales, cost tracking visible |
| S6 (sem 11-12) | **Self-Healing + Explain Diff** | Agent retries on failure, inline explanations |
| S7 (sem 13-14) | **MCP Client + Context Management** | Consume MCP servers, context window compaction |
| S8 (sem 15-16) | **Polish + Tests + Docs** | >70% coverage, instalación npm/brew, docs en inglés, CI/CD |

**Exit criteria:** Competitive con Aider en funcionalidad core. Funciona offline con Ollama. SWE-bench Lite ~15%.

### Beta (Semanas 17-32)

| Sprint | Entregable |
|--------|-----------|
| S9-10 | Multi-agent orchestration (Explorer, Planner, Executor, Reviewer) |
| S11-12 | RAG + Semantic search (Hnswlib + Tree-sitter indexing) |
| S13-14 | Memory Graph + OpenAI adapter |
| S15-16 | Plugin system v1 + Hooks + Custom slash commands |

### GA (Semanas 33-48)

| Sprint | Entregable |
|--------|-----------|
| S17-18 | Enterprise: Auth, audit logging, zero-retention |
| S19-20 | Additional providers (Gemini, DeepSeek) + Model routing |
| S21-22 | SOC 2 Type I + Compliance tooling |
| S23-24 | Self-hosted deployment mode + GA launch |

---

## 5.3 Experimentos Técnicos Recomendados

Antes de comprometerse con implementaciones completas, validar con prototipos de 1-2 días:

| # | Experimento | Pregunta a Responder | Semana |
|---|------------|---------------------|--------|
| E1 | Ink vs @clack/prompts | ¿Startup <500ms con Ink? ¿Memory idle <100MB? | Sem 1 |
| E2 | LanceDB integration spike | ¿LanceDB napi-rs bindings estables? ¿Performance con 100K vectors? | Sem 5 |
| E3 | Claude adapter streaming | ¿TTFT real con tool use + streaming en Node.js? | Sem 2 |
| E4 | Ollama cold start | ¿Latencia primera request después de model load? | Sem 5 |
| E5 | SWE-bench run | ¿Score real con single agent loop + Claude Sonnet? | Sem 8 |
| E6 | Context compaction | ¿Calidad de summarization con Haiku para compaction? | Sem 7 |
| E7 | Memory graph query | ¿SQLite adjacency list soporta graph queries complejas en <50ms? | Sem 12 |
| E8 | Plugin V8 isolate | ¿Overhead de vm2/isolated-vm para sandbox? | Sem 15 |

---

## 5.4 Métricas Ajustadas

| Métrica | Original | Ajustado | Justificación |
|---------|----------|----------|---------------|
| SWE-bench MVP | 30% | 15% | MVP sin multi-agent no puede competir con agents maduros |
| SWE-bench Beta | 40% | 30% | Con multi-agent, realista |
| SWE-bench GA | 50% | 40% | Competitivo con top tools |
| Startup time MVP | <500ms | <1s | Ink/React runtime + init |
| Memory idle MVP | <100MB | <150MB | React + SQLite + config |
| MAU Year 1 | 20,000 | 5,000-10,000 | New entrant sin marketing budget significativo |
| MRR Year 1 | $100K | $30-50K | Alineado con MAU realista |
| Equipo MVP | 6-8 | 4-5 focused | Mejor 5 personas ejecutando que 8 haciendo DDD |

---

# APÉNDICE: TABLA CONSOLIDADA DE HALLAZGOS

| # | Hallazgo | Severidad | Fase | Acción |
|---|----------|-----------|------|--------|
| 1 | DDD sobre-ingeniería | Crítico | Arquitectura | Simplificar a módulos por feature |
| 2 | Commander.js + Ink contradicción | Alto | Arquitectura | Resolver con spike técnico (E1) |
| 3 | Sin gestión de contexto | Crítico | Arquitectura | Diseñar Context Window Manager |
| 4 | Plugin sandboxing indefinido | Alto | Seguridad | Definir capability model |
| 5 | Abstracción multi-model naive | Crítico | Integración | Adapters concretos primero |
| 6 | Model router sin evidencia | Alto | Integración | Heurísticas deterministas MVP |
| 7 | Token counting ignorado | Alto | Integración | ✅ Resuelto: tokenizer.rs (Rust/napi-rs, tiktoken-rs) |
| 8 | sqlite-vec experimental | Alto | Storage | ✅ Resuelto: LanceDB (Rust core) como primary, USearch fallback |
| 9 | Sin indexación incremental | Medio | RAG | Diseñar con file watcher |
| 10 | Sin protocolo agent-to-agent | Alto | Agentes | Definir AgentHandoff |
| 11 | Agents MVP sin orchestrator | Medio | Requisitos | Mover RF-401 a MVP |
| 12 | Bash sandbox sin diseño | Alto | Seguridad | Permission-based MVP |
| 13 | NER PII sin modelo definido | Medio | Seguridad | Regex-only MVP |
| 14 | MVP scope inviable | Crítico | Roadmap | Split en MVP-0 + MVP-1 |
| 15 | Competidores omitidos | Alto | Research | Agregar Cline, Void, bolt.new, Zed |
| 16 | SWE-bench targets irreales | Alto | KPIs | Ajustar a 15/30/40 |
| 17 | Unit economics negativas | Alto | Business | Pricing model concreto |
| 18 | 10 diferenciadores = 0 | Crítico | Estrategia | Reducir a 3 pilares |
| 19 | LATAM no es moat técnico | Alto | Estrategia | Reposicionar como GTM |
| 20 | Sin computer use strategy | Medio | Research | Incluir en Beta plan |
| 21 | MCP como afterthought | Medio | Requisitos | Elevar a Must/MVP |
| 22 | Sin structured outputs | Medio | Integración | Diseñar OutputValidator |
| 23 | ICP no definido | Alto | Estrategia | Definir persona concreta |

---

*Revisión completada el 6 de febrero de 2026. Este documento debe ser tratado como input para una sesión de trabajo de alineación del equipo de liderazgo, no como decisiones finales.*
