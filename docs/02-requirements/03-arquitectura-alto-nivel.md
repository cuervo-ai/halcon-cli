# Arquitectura de Alto Nivel

**Proyecto:** Cuervo CLI — Plataforma de IA Generativa para Desarrollo de Software
**Versión:** 1.0
**Fecha:** 6 de febrero de 2026

---

## Resumen Ejecutivo

La arquitectura de Cuervo CLI sigue un diseño **híbrido por capas** que combina ejecución local (CLI + modelos locales + herramientas de sistema) con servicios cloud (modelos frontier + backend de plataforma + ecosistema Cuervo). Se basa en principios de **DDD (Domain-Driven Design)**, **Clean Architecture** y **event-driven architecture**, consistente con el ecosistema Cuervo existente.

---

## 1. Diagrama de Arquitectura General

```
┌─────────────────────────────────────────────────────────────────────────┐
│                                                                         │
│                        ╔═══════════════════════╗                        │
│                        ║   USUARIO / TERMINAL  ║                        │
│                        ╚═══════════╤═══════════╝                        │
│                                    │                                    │
│  ┌─────────────────────────────────┼─────────────────────────────────┐  │
│  │                      CUERVO CLI (LOCAL)                           │  │
│  │                                                                   │  │
│  │  ┌─────────────┐  ┌──────────────┐  ┌──────────────────────┐    │  │
│  │  │ PRESENTATION│  │ APPLICATION  │  │ DOMAIN               │    │  │
│  │  │             │  │              │  │                      │    │  │
│  │  │ • REPL      │  │ • Orchestr.  │  │ • Agent entities     │    │  │
│  │  │ • Commands  │  │ • Use Cases  │  │ • Model entities     │    │  │
│  │  │ • Renderer  │  │ • Agent Mgr  │  │ • Tool entities      │    │  │
│  │  │ • I/O       │  │ • Session    │  │ • Config value obj.  │    │  │
│  │  │             │  │ • Pipeline   │  │ • Domain events      │    │  │
│  │  └──────┬──────┘  └──────┬───────┘  └──────────┬───────────┘    │  │
│  │         │                │                      │                │  │
│  │  ┌──────┴────────────────┴──────────────────────┴───────────┐   │  │
│  │  │                 INFRASTRUCTURE                            │   │  │
│  │  │                                                           │   │  │
│  │  │  ┌───────────┐ ┌───────────┐ ┌───────────┐ ┌──────────┐│   │  │
│  │  │  │ Model     │ │ Tool      │ │ Storage   │ │ Event    ││   │  │
│  │  │  │ Gateway   │ │ Executor  │ │ Provider  │ │ Bus      ││   │  │
│  │  │  │           │ │           │ │           │ │          ││   │  │
│  │  │  │• Anthropic│ │• File Ops │ │• Local FS │ │• Domain  ││   │  │
│  │  │  │• OpenAI   │ │• Bash     │ │• SQLite   │ │  Events  ││   │  │
│  │  │  │• Google   │ │• Git      │ │• Cache    │ │• Hooks   ││   │  │
│  │  │  │• Ollama   │ │• Search   │ │• Embeddings│ │• Plugins ││   │  │
│  │  │  │• Custom   │ │• Web      │ │           │ │          ││   │  │
│  │  │  └─────┬─────┘ └─────┬─────┘ └─────┬─────┘ └────┬─────┘│   │  │
│  │  └────────┼─────────────┼─────────────┼──────────────┼──────┘   │  │
│  └───────────┼─────────────┼─────────────┼──────────────┼──────────┘  │
│              │             │             │              │              │
│  ════════════╪═════════════╪═════════════╪══════════════╪══════════   │
│   LOCAL      │  LOCAL      │   LOCAL     │   LOCAL      │             │
│              │             │             │              │              │
│  ┌───────────┴──┐  ┌──────┴──────┐  ┌──┴───────┐  ┌──┴──────────┐  │
│  │ Ollama       │  │ File System │  │ SQLite   │  │ Plugin      │  │
│  │ (Local LLMs) │  │ + Git       │  │ + Vector │  │ Registry    │  │
│  └──────────────┘  └─────────────┘  └──────────┘  └─────────────┘  │
│                                                                      │
│  ════════════════════════ NETWORK ═══════════════════════════════    │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │                    CLOUD SERVICES                             │   │
│  │                                                               │   │
│  │  ┌────────────┐  ┌────────────┐  ┌────────────────────────┐ │   │
│  │  │ Model APIs │  │ Cuervo     │  │ Cuervo Platform        │ │   │
│  │  │            │  │ Auth       │  │                        │ │   │
│  │  │ • Anthropic│  │ Service    │  │ • MCP Orchestration    │ │   │
│  │  │ • OpenAI   │  │ (JWT/RBAC) │  │ • Prompt Service       │ │   │
│  │  │ • Google   │  │            │  │ • Analytics            │ │   │
│  │  │ • DeepSeek │  │            │  │ • Admin Dashboard      │ │   │
│  │  └────────────┘  └────────────┘  └────────────────────────┘ │   │
│  │                                                               │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
└──────────────────────────────────────────────────────────────────────┘
```

---

## 2. Componentes Principales

### 2.1 Capa de Presentación (Presentation Layer)

**Responsabilidad:** Interfaz con el usuario a través del terminal.

| Componente | Descripción |
|-----------|-------------|
| **REPL Engine** | Loop interactivo de lectura-evaluación-impresión con soporte multi-línea |
| **Command Parser** | Parsing de slash commands, flags, y argumentos |
| **Output Renderer** | Formateo Markdown, syntax highlighting, diffs, tablas, spinners |
| **Input Handler** | Captura de input del usuario, autocompletado, historial |
| **Permission Prompt** | Sistema de confirmación para operaciones que requieren aprobación |

### 2.2 Capa de Aplicación (Application Layer)

**Responsabilidad:** Orquestación de casos de uso y coordinación de agentes.

| Componente | Descripción |
|-----------|-------------|
| **Orchestrator** | Punto central que coordina el flujo entre agentes, herramientas y modelos |
| **Use Case Handlers** | Implementación de cada caso de uso (implementar feature, debug, review, etc.) |
| **Agent Manager** | Lifecycle de agentes: creación, ejecución, cancelación, resultados |
| **Session Manager** | Gestión de conversaciones, historial, contexto persistente |
| **Pipeline Engine** | Ejecución de workflows multi-paso (plan → implement → test → review) |
| **Task Tracker** | Sistema de tracking de tareas visible para el usuario |

### 2.3 Capa de Dominio (Domain Layer)

**Responsabilidad:** Lógica de negocio pura, sin dependencias externas.

| Entidad/VO | Descripción |
|-----------|-------------|
| **Agent** | Entidad que representa un agente especializado (explorer, planner, executor, reviewer) |
| **Model** | Entidad que representa un modelo de IA con sus capacidades y configuración |
| **Tool** | Entidad que representa una herramienta disponible (file ops, bash, git, search) |
| **Session** | Aggregado que gestiona el contexto de una conversación |
| **Configuration** | Value Object para configuración del proyecto y usuario |
| **Permission** | Value Object para permisos de ejecución |
| **DomainEvent** | Eventos de dominio (AgentStarted, ToolExecuted, ModelInvoked, etc.) |

### 2.4 Capa de Infraestructura (Infrastructure Layer)

**Responsabilidad:** Implementaciones concretas de interfaces del dominio.

| Componente | Descripción |
|-----------|-------------|
| **Model Gateway** | Abstracción multi-proveedor con implementaciones para cada API |
| **Tool Executor** | Ejecución sandboxed de herramientas del sistema |
| **Storage Provider** | Persistencia local (SQLite, filesystem, cache) |
| **Event Bus** | Distribución de eventos a hooks, plugins, y logging |
| **Plugin Loader** | Carga dinámica de plugins y extensiones |
| **Auth Client** | Integración con cuervo-auth-service |

---

## 3. Flujo de Datos Principal

```
┌──────────┐     ┌──────────┐     ┌──────────────┐     ┌─────────────┐
│ User     │────▶│ REPL     │────▶│ Orchestrator │────▶│ Agent Mgr   │
│ Input    │     │ Engine   │     │              │     │             │
└──────────┘     └──────────┘     └──────┬───────┘     └──────┬──────┘
                                         │                     │
                                         │   ┌────────────────┘
                                         │   │
                                    ┌────▼───▼─────┐
                                    │ Task Planner │
                                    └──────┬───────┘
                                           │
                              ┌────────────┼────────────┐
                              │            │            │
                         ┌────▼────┐  ┌────▼────┐ ┌────▼────┐
                         │ Context │  │ Model   │ │ Tool    │
                         │ Builder │  │ Gateway │ │ Executor│
                         └────┬────┘  └────┬────┘ └────┬────┘
                              │            │            │
                    ┌─────────┤      ┌─────┤      ┌────┤
                    │         │      │     │      │    │
               ┌────▼──┐ ┌───▼──┐ ┌─▼───┐ │   ┌──▼─┐ │
               │Search │ │Files │ │Local│ │   │Bash│ │
               │Engine │ │      │ │LLM  │ │   │    │ │
               └───────┘ └──────┘ └─────┘ │   └────┘ │
                                     ┌────▼────┐ ┌───▼───┐
                                     │Cloud API│ │ Git   │
                                     └─────────┘ └───────┘
                                           │
                                    ┌──────▼───────┐
                                    │ Response     │
                                    │ Assembler    │
                                    └──────┬───────┘
                                           │
                                    ┌──────▼───────┐
                                    │ Output       │
                                    │ Renderer     │
                                    └──────┬───────┘
                                           │
                                    ┌──────▼───────┐
                                    │ Terminal     │
                                    │ Display     │
                                    └──────────────┘
```

---

## 4. Modelo de Routing de Modelos

```
┌────────────────────────────────────────────────────────────┐
│               MODEL ROUTING ENGINE                          │
├────────────────────────────────────────────────────────────┤
│                                                             │
│  Input: User request + context                              │
│                                                             │
│  ┌─────────────────────────────┐                           │
│  │ Task Complexity Classifier  │                           │
│  │ (lightweight local model)   │                           │
│  └──────────┬──────────────────┘                           │
│             │                                               │
│    ┌────────┼─────────────┬──────────────┐                 │
│    │        │             │              │                  │
│    ▼        ▼             ▼              ▼                  │
│  TRIVIAL  SIMPLE        MEDIUM        COMPLEX              │
│  Local    Local/Haiku   Sonnet/4o     Opus/o3              │
│                                                             │
│  • Rename  • Explain    • Multi-file  • Architecture       │
│  • Format  • Single-fn  • Test gen    • Debug complex      │
│  • Lint    • Completion • Refactor    • Full feature        │
│  • Search  • Type fix   • Review      • Migration          │
│                                                             │
│  <$0.001   ~$0.01       ~$0.05-0.10   ~$0.50-2.00         │
│  <100ms    <500ms       <2s           <10s                  │
│                                                             │
│  ┌─────────────────────────────────────┐                   │
│  │ User override: --model=opus         │                   │
│  │ Project config: default_model=sonnet│                   │
│  │ Budget limit: max_cost_per_request  │                   │
│  └─────────────────────────────────────┘                   │
│                                                             │
│  ┌─────────────────────────────────────┐                   │
│  │ Fallback chain:                      │                   │
│  │ Primary → Secondary → Local → Error  │                   │
│  └─────────────────────────────────────┘                   │
│                                                             │
└────────────────────────────────────────────────────────────┘
```

---

## 5. Modelo de Agentes

```
┌────────────────────────────────────────────────────────────┐
│                    AGENT ARCHITECTURE                        │
├────────────────────────────────────────────────────────────┤
│                                                             │
│  ┌───────────────────────────────────┐                     │
│  │       ORCHESTRATOR AGENT          │                     │
│  │   (Main conversation handler)     │                     │
│  └───────────────┬───────────────────┘                     │
│                   │                                         │
│    ┌──────────────┼──────────────┬──────────────┐          │
│    │              │              │              │           │
│    ▼              ▼              ▼              ▼           │
│  ┌──────┐     ┌──────┐     ┌──────┐     ┌──────┐         │
│  │EXPLORE│     │ PLAN │     │EXECUTE│     │REVIEW│         │
│  │Agent  │     │Agent │     │Agent  │     │Agent │         │
│  │       │     │      │     │       │     │      │         │
│  │Search │     │Design│     │Code   │     │Check │         │
│  │Navigate     │Break │     │Edit   │     │Test  │         │
│  │Understand   │down  │     │Build  │     │Score │         │
│  └──────┘     └──────┘     └──────┘     └──────┘         │
│    │              │              │              │           │
│    │ Read-only    │ Read-only    │ Read+Write   │ Read-only│
│    │ Fast model   │ Smart model  │ Smart model  │ Smart    │
│    └──────────────┴──────────────┴──────────────┘          │
│                                                             │
│  PRINCIPIOS:                                                │
│  • Least privilege: cada agente tiene solo los permisos     │
│    necesarios para su función                               │
│  • Isolation: agentes no comparten estado mutable           │
│  • Human-in-the-loop: operaciones de escritura requieren    │
│    aprobación del usuario                                   │
│  • Composability: agentes pueden invocarse entre sí         │
│                                                             │
└────────────────────────────────────────────────────────────┘
```

---

## 6. Stack Tecnológico

### 6.1 Selección de Tecnologías

| Capa | Tecnología | Justificación |
|------|-----------|---------------|
| **Runtime** | Node.js 22 LTS | Consistencia con ecosistema Cuervo (TypeScript everywhere) |
| **Lenguaje** | TypeScript 5.4+ | Type safety, DX, compatibilidad con shared-kernel |
| **Capa nativa (performance)** | Rust + napi-rs | Hot paths críticos: scanner, tokenizer, PII, AST parsing (ver ADR-008) |
| **CLI Framework** | Commander.js + Ink (React para terminal) | Madurez, extensibilidad, rendering flexible |
| **Build** | tsup + esbuild | Build rápido, output optimizado |
| **Build nativo** | cargo + napi-rs CLI | Compilación Rust → binarios nativos precompilados por plataforma |
| **Package Manager** | pnpm | Eficiencia en espacio, workspaces |
| **Base de datos local** | SQLite (better-sqlite3) | Zero-config, embedded, performant |
| **Vector store local** | LanceDB (Rust core, napi-rs bindings) | Embeddings locales, IVF-PQ + DiskANN, FTS via Tantivy, mmap I/O |
| **Cache** | LRU in-memory + SQLite | Rápido sin dependencias externas |
| **HTTP Client** | undici (built-in Node.js) | Máximo rendimiento HTTP |
| **Git integration** | simple-git | Wrapper Git robusto para Node.js |
| **Terminal UI** | Ink + chalk + ora + cli-table3 | Rendering rico en terminal |
| **Testing** | Vitest | Rápido, TypeScript nativo, compatible con Jest |
| **Linting** | ESLint 9 + Prettier | Estándar del ecosistema |

### 6.2 Dependencias de Servicios Cloud (Opcionales)

| Servicio | Uso | Obligatorio |
|---------|-----|-------------|
| **cuervo-auth-service** | Autenticación JWT/RBAC | No (modo local sin auth) |
| **cuervo-prompt-service** | Templates de prompts compartidos | No (prompts locales como fallback) |
| **cuervo-main (MCP)** | Orquestación avanzada | No (orquestación local por defecto) |
| **Model APIs** | Inferencia cloud (Claude, GPT, Gemini) | No (Ollama local como alternativa) |

> **Principio:** Cuervo CLI debe ser **100% funcional en modo offline** usando modelos locales (Ollama). Los servicios cloud añaden capacidades pero nunca son requeridos.

---

## 7. Modelo de Deployment

```
┌────────────────────────────────────────────────────────────┐
│                  DEPLOYMENT MODELS                          │
├────────────────────────────────────────────────────────────┤
│                                                             │
│  MODE 1: STANDALONE (Default)                               │
│  ┌──────────────────────────────┐                          │
│  │ Developer Machine            │                          │
│  │ ┌────────┐  ┌──────────────┐│                          │
│  │ │Cuervo  │  │ Ollama       ││  ← 100% local            │
│  │ │CLI     │──│ (Local LLMs) ││  ← No internet required   │
│  │ └────────┘  └──────────────┘│  ← Free                   │
│  └──────────────────────────────┘                          │
│                                                             │
│  MODE 2: HYBRID (Recommended)                               │
│  ┌──────────────────────────────┐                          │
│  │ Developer Machine            │                          │
│  │ ┌────────┐  ┌──────────────┐│     ┌──────────────┐     │
│  │ │Cuervo  │──│ Ollama       ││     │ Cloud APIs   │     │
│  │ │CLI     │──│ (fast tasks) ││────▶│ (complex)    │     │
│  │ └────────┘  └──────────────┘│     └──────────────┘     │
│  └──────────────────────────────┘                          │
│                                                             │
│  MODE 3: ENTERPRISE (Self-hosted)                           │
│  ┌──────────────────────────────┐   ┌──────────────────┐  │
│  │ Developer Machines           │   │ Corporate Cloud  │  │
│  │ ┌────────┐  ┌────────┐      │   │ ┌──────────────┐ │  │
│  │ │Cuervo  │  │Cuervo  │      │   │ │Cuervo Platform│ │  │
│  │ │CLI (1) │  │CLI (N) │      │──▶│ │(self-hosted) │ │  │
│  │ └────────┘  └────────┘      │   │ ├──────────────┤ │  │
│  └──────────────────────────────┘   │ │Auth Service  │ │  │
│                                      │ │Prompt Service│ │  │
│                                      │ │Model Serving │ │  │
│                                      │ │(vLLM/Ollama)│ │  │
│                                      │ └──────────────┘ │  │
│                                      └──────────────────┘  │
│                                                             │
│  MODE 4: SAAS (Managed)                                     │
│  ┌──────────────────────────────┐   ┌──────────────────┐  │
│  │ Developer Machines           │   │ Cuervo Cloud     │  │
│  │ ┌────────┐                   │   │ (managed)        │  │
│  │ │Cuervo  │                   │──▶│ All services     │  │
│  │ │CLI     │                   │   │ hosted by Cuervo │  │
│  │ └────────┘                   │   └──────────────────┘  │
│  └──────────────────────────────┘                          │
│                                                             │
└────────────────────────────────────────────────────────────┘
```

---

## 8. Decisiones Arquitectónicas Clave (ADRs)

| ADR | Decisión | Alternativas Consideradas | Justificación |
|-----|----------|--------------------------|---------------|
| ADR-001 | TypeScript como lenguaje principal | Rust, Go, Python | Consistencia ecosistema, velocidad de desarrollo, shared-kernel |
| ADR-002 | SQLite como DB local | LevelDB, JSON files | Zero-config, SQL queries, extensión vector |
| ADR-003 | Arquitectura de plugins basada en eventos | Direct imports, RPC | Desacoplamiento, extensibilidad, no bloquea main thread |
| ADR-004 | Model Gateway pattern (Strategy) | Direct API calls | Abstracción multi-proveedor, testabilidad, fallback chain |
| ADR-005 | Agentes como entidades de dominio | Simple functions | Lifecycle management, composability, isolation |
| ADR-006 | Clean Architecture (4 capas) | Hexagonal, Simple MVC | Consistencia con ecosistema, testabilidad, mantenibilidad |
| ADR-007 | Modo offline-first | Cloud-first, Hybrid-first | Máxima autonomía del desarrollador, privacy by default |
| ADR-008 | Rust (napi-rs) para hot paths de performance | WASM, C++ addons, pure TypeScript | napi-rs ofrece rendimiento nativo sin overhead de serialización, mismo patrón que SWC/Biome/Rspack. Fallback TS para plataformas sin binario precompilado |
| ADR-009 | LanceDB como vector store | sqlite-vec, Hnswlib, USearch, Chroma | LanceDB tiene core Rust, bindings napi-rs nativos, IVF-PQ + DiskANN, FTS integrado via Tantivy, formato columnar Lance con mmap I/O. sqlite-vec es pre-1.0 experimental |

---

### 8.1 ADR-008: Rust Performance Layer (Detalle)

**Contexto:** TypeScript es el lenguaje principal por consistencia con el ecosistema Cuervo, pero las operaciones CPU-bound en hot paths (file scanning, token counting, AST parsing, PII detection) son 10-100x más lentas que sus equivalentes nativos. Proyectos de referencia como SWC (compilador), Biome (linter), Rspack (bundler), Lightning CSS y Oxc ya usan este patrón con éxito.

**Decisión:** Implementar módulos Rust compilados a binarios nativos Node.js vía **napi-rs**, con fallback automático a TypeScript puro cuando el binario nativo no esté disponible.

**Módulos Rust planificados:**

| Módulo | Función | Ganancia Esperada | Fase |
|--------|---------|-------------------|------|
| `scanner.rs` | Glob + grep paralelo (rayon + ignore crate) | 10-50x vs glob/ripgrep JS | MVP-1 |
| `tokenizer.rs` | Token counting multi-provider (tiktoken-rs) | 5-10x vs tiktoken-js | Beta |
| `treesitter.rs` | AST parsing para code chunking (tree-sitter) | 3-8x vs tree-sitter WASM | Beta |
| `pii.rs` | Regex PII detection (regex crate, SIMD-accelerated) | 5-20x vs JS regex | Beta |

**Distribución:** Paquetes npm platform-specific precompilados (`@cuervo/native-darwin-arm64`, `@cuervo/native-linux-x64-gnu`, etc.) con `optionalDependencies` en package.json.

**Fallback:** Cada módulo nativo tiene equivalente TypeScript. El patrón es:
```typescript
let native: typeof import('@cuervo/native') | null = null;
try { native = require('@cuervo/native'); } catch { /* use TS fallback */ }
```

---

*Este documento establece la base arquitectónica. El diseño detallado se expandirá en la Sección 3.*
