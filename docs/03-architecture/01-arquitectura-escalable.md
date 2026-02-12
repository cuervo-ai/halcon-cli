# Arquitectura Escalable — Microservicios, APIs, Almacenamiento y Colas

**Proyecto:** Cuervo CLI
**Versión:** 1.0
**Fecha:** 6 de febrero de 2026

---

## Resumen Ejecutivo

La arquitectura de Cuervo CLI implementa un diseño **Clean Architecture con DDD** en el CLI local, que se conecta opcionalmente a una infraestructura de microservicios cloud basada en el ecosistema Cuervo existente. El diseño prioriza **offline-first**, escalabilidad horizontal, y tolerancia a fallos.

---

## 1. Arquitectura Detallada del CLI (Local)

### 1.1 Estructura de Paquetes

```
cuervo-cli/
├── src/
│   ├── domain/                    # Capa de Dominio (sin deps externas)
│   │   ├── entities/
│   │   │   ├── Agent.ts           # Entidad Agent (explorer, planner, executor, reviewer)
│   │   │   ├── Model.ts           # Entidad Model (provider, capabilities, config)
│   │   │   ├── Tool.ts            # Entidad Tool (file ops, bash, git, search)
│   │   │   ├── Session.ts         # Aggregate Root — conversación + contexto
│   │   │   └── Task.ts            # Entidad Task (tracking de trabajo)
│   │   ├── value-objects/
│   │   │   ├── ModelConfig.ts     # Configuración inmutable de modelo
│   │   │   ├── Permission.ts      # Permisos de ejecución
│   │   │   ├── TokenBudget.ts     # Presupuesto de tokens
│   │   │   └── ToolResult.ts      # Resultado de ejecución de tool
│   │   ├── events/
│   │   │   ├── DomainEvent.ts     # Base event
│   │   │   ├── AgentEvents.ts     # AgentStarted, AgentCompleted, AgentFailed
│   │   │   ├── ModelEvents.ts     # ModelInvoked, ModelFallback, ModelError
│   │   │   └── ToolEvents.ts      # ToolExecuted, FileModified, BashExecuted
│   │   ├── repositories/          # Interfaces (ports)
│   │   │   ├── IModelRepository.ts
│   │   │   ├── ISessionRepository.ts
│   │   │   └── IConfigRepository.ts
│   │   └── services/              # Domain services
│   │       ├── ModelRouter.ts     # Routing inteligente de modelos
│   │       ├── AgentOrchestrator.ts
│   │       └── PermissionChecker.ts
│   │
│   ├── application/               # Capa de Aplicación (use cases)
│   │   ├── use-cases/
│   │   │   ├── ImplementFeature.ts
│   │   │   ├── DebugError.ts
│   │   │   ├── RefactorCode.ts
│   │   │   ├── ReviewCode.ts
│   │   │   ├── CommitChanges.ts
│   │   │   ├── ExplainCode.ts
│   │   │   ├── GenerateTests.ts
│   │   │   └── ManageModels.ts
│   │   ├── services/
│   │   │   ├── SessionManager.ts
│   │   │   ├── PipelineEngine.ts
│   │   │   ├── TaskTracker.ts
│   │   │   └── ContextBuilder.ts
│   │   └── dto/                   # Data Transfer Objects
│   │       ├── ModelInvokeDTO.ts
│   │       ├── ToolExecuteDTO.ts
│   │       └── AgentTaskDTO.ts
│   │
│   ├── infrastructure/            # Capa de Infraestructura (adapters)
│   │   ├── model-providers/       # Implementaciones de Model Gateway
│   │   │   ├── AnthropicProvider.ts
│   │   │   ├── OpenAIProvider.ts
│   │   │   ├── GoogleProvider.ts
│   │   │   ├── OllamaProvider.ts
│   │   │   ├── DeepSeekProvider.ts
│   │   │   └── CustomProvider.ts
│   │   ├── native/                # Bridge TypeScript ↔ Rust (napi-rs)
│   │   │   ├── NativeLoader.ts    # Carga dinámica con fallback a TS puro
│   │   │   ├── NativeScanner.ts   # Wrapper: fastGlob(), fastGrep()
│   │   │   ├── NativeTokenizer.ts # Wrapper: countTokens(), truncateToTokens()
│   │   │   ├── NativeTreeSitter.ts# Wrapper: parseFile(), parseFiles()
│   │   │   └── NativePII.ts       # Wrapper: scanPII(), redactPII()
│   │   ├── tools/                 # Implementaciones de Tools
│   │   │   ├── FileOperations.ts
│   │   │   ├── BashExecutor.ts
│   │   │   ├── GitOperations.ts
│   │   │   ├── SearchEngine.ts    # Usa NativeScanner cuando disponible
│   │   │   ├── WebFetcher.ts
│   │   │   └── NotebookEditor.ts
│   │   ├── storage/
│   │   │   ├── SQLiteStorage.ts   # Persistencia local
│   │   │   ├── LanceDBStore.ts    # Vector store: LanceDB (Rust core, napi-rs)
│   │   │   ├── VectorStore.ts     # Abstracción: LanceDB primary, fallback USearch
│   │   │   ├── FileSystemCache.ts
│   │   │   └── KeychainStore.ts   # Secrets management
│   │   ├── events/
│   │   │   ├── EventBus.ts
│   │   │   ├── HookExecutor.ts    # Pre/post hooks
│   │   │   └── PluginNotifier.ts
│   │   ├── auth/
│   │   │   └── AuthClient.ts      # Integración con cuervo-auth-service
│   │   ├── telemetry/
│   │   │   ├── AuditLogger.ts
│   │   │   ├── MetricsCollector.ts
│   │   │   └── PIIRedactor.ts
│   │   └── plugins/
│   │       ├── PluginLoader.ts
│   │       ├── PluginSandbox.ts
│   │       └── PluginRegistry.ts
│   │
│   └── presentation/              # Capa de Presentación (UI)
│       ├── cli/
│       │   ├── REPL.ts            # Main REPL loop
│       │   ├── CommandParser.ts
│       │   ├── InputHandler.ts
│       │   └── commands/          # Slash commands
│       │       ├── CommitCommand.ts
│       │       ├── ReviewCommand.ts
│       │       ├── ExplainCommand.ts
│       │       └── ...
│       ├── renderer/
│       │   ├── MarkdownRenderer.ts
│       │   ├── DiffRenderer.ts
│       │   ├── TableRenderer.ts
│       │   ├── SpinnerRenderer.ts
│       │   └── PermissionPrompt.ts
│       └── formatters/
│           ├── CodeFormatter.ts
│           └── ErrorFormatter.ts
│
├── native/                        # Capa de performance Rust (napi-rs)
│   ├── Cargo.toml                # Workspace config + dependencias Rust
│   ├── src/
│   │   ├── lib.rs                # Entry point napi-rs, re-exports
│   │   ├── scanner.rs            # Glob + grep paralelo (rayon + ignore)
│   │   ├── treesitter.rs         # AST parsing multi-lenguaje (tree-sitter)
│   │   ├── tokenizer.rs          # Token counting multi-provider (tiktoken-rs)
│   │   └── pii.rs                # PII detection regex SIMD-accelerated
│   ├── __test__/                 # Tests napi-rs (Jest + native)
│   └── build.rs                  # Build script para tree-sitter grammars
├── npm/                           # Paquetes platform-specific precompilados
│   ├── darwin-arm64/             # @cuervo/native-darwin-arm64
│   ├── darwin-x64/               # @cuervo/native-darwin-x64
│   ├── linux-x64-gnu/            # @cuervo/native-linux-x64-gnu
│   ├── linux-arm64-gnu/          # @cuervo/native-linux-arm64-gnu
│   └── win32-x64-msvc/           # @cuervo/native-win32-x64-msvc
├── config/
│   ├── default.yml               # Configuración por defecto
│   └── schema.json               # JSON Schema para validación
├── plugins/                       # Plugins bundled
├── tests/
│   ├── unit/
│   ├── integration/
│   └── e2e/
├── package.json
├── tsconfig.json
└── vitest.config.ts
```

### 1.2 Dependencias entre Capas

```
┌──────────────┐
│ Presentation │ ──depends on──▶ Application
└──────────────┘                      │
                                      │ depends on
                                      ▼
                               ┌──────────┐
                               │  Domain   │ ← NO dependencies (pure)
                               └──────────┘
                                      ▲
                                      │ implements
┌──────────────┐                      │
│Infrastructure│ ──depends on──────────┘
└──────────────┘

REGLA: Las dependencias siempre apuntan hacia el dominio (Dependency Inversion)
```

---

## 2. APIs y Contratos

### 2.1 Model Provider Interface (Strategy Pattern)

```typescript
// domain/repositories/IModelProvider.ts
interface IModelProvider {
  readonly name: string;
  readonly capabilities: ModelCapabilities;

  invoke(request: ModelRequest): AsyncGenerator<ModelChunk>;
  embed(texts: string[]): Promise<number[][]>;
  isAvailable(): Promise<boolean>;
  estimateCost(request: ModelRequest): TokenCost;
}

interface ModelRequest {
  messages: Message[];
  tools?: ToolDefinition[];
  model: string;
  maxTokens?: number;
  temperature?: number;
  stream: boolean;
}

interface ModelChunk {
  type: 'text' | 'tool_use' | 'thinking';
  content: string;
  toolUse?: { name: string; input: Record<string, unknown> };
}
```

### 2.2 Tool Interface

```typescript
// domain/entities/Tool.ts
interface ITool {
  readonly name: string;
  readonly description: string;
  readonly permissions: ToolPermission;

  execute(input: ToolInput): Promise<ToolResult>;
  validate(input: ToolInput): ValidationResult;
  requiresConfirmation(input: ToolInput): boolean;
}

type ToolPermission = 'read-only' | 'read-write' | 'destructive';
```

### 2.3 Agent Interface

```typescript
// domain/entities/Agent.ts
interface IAgent {
  readonly type: AgentType;
  readonly tools: ITool[];
  readonly model: IModelProvider;

  run(task: AgentTask): AsyncGenerator<AgentEvent>;
  cancel(): Promise<void>;
  getStatus(): AgentStatus;
}

type AgentType = 'explorer' | 'planner' | 'executor' | 'reviewer' | 'custom';
type AgentStatus = 'idle' | 'running' | 'completed' | 'failed' | 'cancelled';
```

---

## 3. Almacenamiento Local

### 3.1 SQLite Schema

```sql
-- Sesiones de conversación
CREATE TABLE sessions (
  id TEXT PRIMARY KEY,
  project_path TEXT NOT NULL,
  created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  metadata JSON
);

-- Mensajes de conversación
CREATE TABLE messages (
  id TEXT PRIMARY KEY,
  session_id TEXT REFERENCES sessions(id),
  role TEXT NOT NULL CHECK(role IN ('user', 'assistant', 'system', 'tool')),
  content TEXT NOT NULL,
  model_id TEXT,
  tokens_used INTEGER,
  cost_usd REAL,
  created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Índice de embeddings del codebase
CREATE TABLE embeddings (
  id TEXT PRIMARY KEY,
  file_path TEXT NOT NULL,
  chunk_start INTEGER,
  chunk_end INTEGER,
  content_hash TEXT NOT NULL,
  vector BLOB NOT NULL,  -- float32 array serializado
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Audit log
CREATE TABLE audit_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
  level TEXT NOT NULL,
  category TEXT NOT NULL,
  event TEXT NOT NULL,
  session_id TEXT,
  data JSON,
  hash TEXT NOT NULL,      -- integrity chain
  prev_hash TEXT           -- link to previous entry
);

-- Configuración de modelos
CREATE TABLE model_registry (
  id TEXT PRIMARY KEY,
  provider TEXT NOT NULL,
  name TEXT NOT NULL,
  version TEXT,
  capabilities JSON,
  config JSON,
  is_active BOOLEAN DEFAULT 1,
  created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Cache semántico
CREATE TABLE semantic_cache (
  id TEXT PRIMARY KEY,
  prompt_hash TEXT NOT NULL,
  prompt_embedding BLOB,
  response TEXT NOT NULL,
  model_id TEXT,
  hits INTEGER DEFAULT 0,
  created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  expires_at DATETIME
);

-- Índices
CREATE INDEX idx_messages_session ON messages(session_id);
CREATE INDEX idx_embeddings_path ON embeddings(file_path);
CREATE INDEX idx_audit_timestamp ON audit_log(timestamp);
CREATE INDEX idx_audit_category ON audit_log(category, event);
CREATE INDEX idx_cache_hash ON semantic_cache(prompt_hash);
```

### 3.2 Vector Store para RAG (LanceDB)

**Decisión:** Reemplazar sqlite-vec/Hnswlib por **LanceDB** (core Rust, bindings napi-rs nativos). Ver ADR-009 en `02-requirements/03-arquitectura-alto-nivel.md`.

**Justificación del cambio:**
- sqlite-vec es pre-1.0, sin garantía de estabilidad
- Hnswlib requiere serialización manual a disco, sin persistencia nativa
- LanceDB ofrece: formato columnar Lance con mmap I/O, IVF-PQ + DiskANN, FTS integrado via Tantivy, zero-copy reads

```
INDEXACIÓN DE CODEBASE:

1. Tree-sitter parse → AST (via Rust nativo: treesitter.rs)
2. Chunking inteligente por:
   - Funciones/métodos
   - Clases/interfaces
   - Módulos/archivos
   - Bloques de comentarios
3. Embedding generation (modelo local o API)
4. Almacenamiento en LanceDB (embedded, persistent, mmap)

SCHEMA LANCEDB:
embeddings table:
  id:           string (PK)
  file_path:    string (indexed)
  chunk_type:   string ('function' | 'class' | 'module' | 'comment')
  chunk_name:   string
  start_line:   int
  end_line:     int
  content_hash: string (para invalidación incremental)
  content_text: string (FTS via Tantivy)
  vector:       float32[768] (ANN via IVF-PQ/DiskANN)

BÚSQUEDA HÍBRIDA:
1. Query embedding → ANN search en LanceDB
2. Full-text search via Tantivy (integrado en LanceDB)
3. Hybrid re-ranking (RRF: Reciprocal Rank Fusion)
4. Context assembly con chunks relevantes

FALLBACK: Si LanceDB no disponible → USearch (C++ HNSW, bindings Node.js)
```

---

## 4. Arquitectura Cloud (Opcional)

### 4.1 Integración con Ecosistema Cuervo

```
┌─────────────────────────────────────────────────────────────┐
│                  CUERVO CLOUD PLATFORM                       │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌────────────────┐     ┌────────────────┐                  │
│  │ API Gateway    │     │ Load Balancer  │                  │
│  │ (Kong/Traefik) │     │ (NGINX/Envoy)  │                  │
│  └────────┬───────┘     └────────┬───────┘                  │
│           │                      │                           │
│     ┌─────┴──────────────────────┴─────┐                    │
│     │          Service Mesh (Istio)     │                    │
│     └──┬──────┬──────┬──────┬──────┬───┘                    │
│        │      │      │      │      │                         │
│   ┌────▼──┐ ┌▼────┐ ┌▼────┐ ┌▼───┐ ┌▼─────────┐           │
│   │Auth   │ │Prompt│ │Model│ │MCP │ │Analytics  │           │
│   │Service│ │Svc   │ │Proxy│ │Core│ │Service    │           │
│   │(3001) │ │(3007)│ │(3010│ │(3000│ │(3020)    │           │
│   └───┬───┘ └──┬───┘ └──┬──┘ └─┬──┘ └────┬─────┘           │
│       │        │        │      │          │                  │
│  ─────┴────────┴────────┴──────┴──────────┴───────────      │
│                    Data Layer                                 │
│  ┌──────────┐ ┌───────┐ ┌──────────┐ ┌────────────┐        │
│  │PostgreSQL│ │ Redis │ │MeiliSearch│ │ Qdrant     │        │
│  │+ pgvector│ │ Cache │ │ FTS      │ │ VectorDB   │        │
│  └──────────┘ └───────┘ └──────────┘ └────────────┘        │
│                                                              │
│  ┌──────────────────────────────────────────┐               │
│  │ Observability Stack                       │               │
│  │ Prometheus → Grafana → Jaeger → ELK      │               │
│  └──────────────────────────────────────────┘               │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### 4.2 Model Proxy Service (Nuevo)

```
┌────────────────────────────────────────────────────────────┐
│                 MODEL PROXY SERVICE                          │
│              (cuervo-model-proxy:3010)                       │
├────────────────────────────────────────────────────────────┤
│                                                             │
│  RESPONSABILIDADES:                                         │
│  • Unified API para todos los model providers               │
│  • Caching semántico centralizado                           │
│  • Rate limiting y quota management                         │
│  • Cost tracking y billing                                  │
│  • Routing inteligente server-side                          │
│  • Load balancing entre instancias de modelo                │
│  • Fallback chain management                                │
│  • Audit logging centralizado                               │
│                                                             │
│  API:                                                       │
│  POST /v1/chat/completions  ← OpenAI-compatible format     │
│  POST /v1/embeddings                                        │
│  GET  /v1/models                                            │
│  GET  /v1/usage                                             │
│                                                             │
│  NOTA: El CLI puede conectarse directamente a providers     │
│  O a través del Model Proxy para features enterprise        │
│                                                             │
└────────────────────────────────────────────────────────────┘
```

---

## 5. Escalabilidad y Tolerancia a Fallos

### 5.1 Circuit Breaker Pattern

```
┌─────────────────────────────────────────────────────┐
│              CIRCUIT BREAKER PER PROVIDER             │
├─────────────────────────────────────────────────────┤
│                                                      │
│  State Machine:                                      │
│                                                      │
│  ┌────────┐    failures > threshold    ┌───────┐    │
│  │ CLOSED │ ──────────────────────────▶│ OPEN  │    │
│  │(normal)│                            │(block)│    │
│  └────┬───┘                            └───┬───┘    │
│       ▲                                    │        │
│       │     success                        │ timeout│
│       │                              ┌─────▼─────┐  │
│       └──────────────────────────────│ HALF-OPEN │  │
│                                      │ (test)    │  │
│                                      └───────────┘  │
│                                                      │
│  Config per provider:                                │
│  • failure_threshold: 5                              │
│  • timeout: 30s                                      │
│  • half_open_requests: 3                             │
│                                                      │
│  On OPEN → route to next provider in fallback chain  │
│                                                      │
└─────────────────────────────────────────────────────┘
```

### 5.2 Fallback Chain

```
PRIMARY (user configured)
    │
    ├── fails → SECONDARY (auto-selected)
    │              │
    │              ├── fails → LOCAL (Ollama)
    │              │              │
    │              │              ├── fails → GRACEFUL DEGRADATION
    │              │              │           "Modelo no disponible.
    │              │              │            Opciones: retry, change model,
    │              │              │            work offline"
    │              │              │
    │              │              └── succeeds → Continue (with quality notice)
    │              │
    │              └── succeeds → Continue
    │
    └── succeeds → Continue
```

### 5.3 Estrategia de Caching Multi-Nivel

```
L1: IN-MEMORY LRU CACHE
    • Embeddings recientes
    • Resultados de búsqueda
    • Model capabilities
    TTL: 5 minutos | Size: 100MB max

L2: SQLITE SEMANTIC CACHE
    • Responses a prompts similares (cosine similarity > 0.95)
    • File content hashes (skip re-read)
    • AST parse results
    TTL: 24 horas | Size: 1GB max

L3: REDIS (Cloud only)
    • Shared cache entre usuarios
    • Popular prompt-response pairs
    • Model routing decisions
    TTL: 1 hora | Size: configured
```

---

## 6. Capa de Performance Nativa (Rust + napi-rs)

### 6.1 Arquitectura de Módulos Rust

```
┌─────────────────────────────────────────────────────────────┐
│                 RUST PERFORMANCE LAYER                        │
│              (compilado vía napi-rs → Node.js addon)         │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌──────────┐  ┌──────────────┐  ┌────────────┐            │
│  │scanner.rs│  │treesitter.rs │  │tokenizer.rs│            │
│  │          │  │              │  │            │            │
│  │• fastGlob│  │• parseFile   │  │• countToks │            │
│  │• fastGrep│  │• parseFiles  │  │• truncate  │            │
│  │          │  │• getSymbols  │  │• estimate  │            │
│  │ rayon    │  │              │  │            │            │
│  │ ignore   │  │ tree-sitter  │  │ tiktoken-rs│            │
│  │ globset  │  │ grammars     │  │ estimators │            │
│  └──────┬───┘  └──────┬───────┘  └─────┬──────┘            │
│         │             │                │                     │
│  ┌──────┴─────────────┴────────────────┴──────┐            │
│  │              pii.rs                         │            │
│  │  • scanPII (regex SIMD-accelerated)         │            │
│  │  • redactPII (replacement in-place)         │            │
│  │  • patterns: email, IP, API key, CC, SSN    │            │
│  └─────────────────────────────────────────────┘            │
│                                                              │
│  DISTRIBUCIÓN: @cuervo/native-{platform}-{arch}            │
│  FALLBACK: TypeScript puro si binario no disponible          │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### 6.2 Patrón de Carga con Fallback

```typescript
// infrastructure/native/NativeLoader.ts
class NativeLoader {
  private static instance: NativeModule | null = null;

  static load(): NativeModule | null {
    if (this.instance !== undefined) return this.instance;
    try {
      this.instance = require('@cuervo/native');
      return this.instance;
    } catch {
      console.warn('[cuervo] Native module not available. Using JS fallback.');
      this.instance = null;
      return null;
    }
  }
}

// Uso en SearchEngine.ts:
const native = NativeLoader.load();
if (native) {
  results = await native.fastGrep(pattern, paths, options);  // ~10-50x faster
} else {
  results = await jsGrep(pattern, paths, options);            // fallback JS
}
```

### 6.3 Dependencias Rust (Cargo.toml)

| Crate | Versión | Propósito |
|-------|---------|-----------|
| `napi` + `napi-derive` | 2.x | Bridge Rust → Node.js |
| `rayon` | 1.x | Paralelismo data-parallel |
| `ignore` | 0.4 | .gitignore-aware file walking |
| `globset` | 0.4 | Pattern matching eficiente |
| `regex` | 1.x | SIMD-accelerated regex |
| `tree-sitter` | 0.22 | AST parsing (C/Rust core) |
| `tiktoken-rs` | 0.5 | Tokenizer compatible OpenAI |
| `serde` + `serde_json` | 1.x | Serialización |
| `lancedb` | 0.x | Vector DB (usado vía npm SDK) |

### 6.4 Impacto en Requisitos No Funcionales

| RNF | Target Original | Con Rust Layer | Mejora |
|-----|----------------|----------------|--------|
| RNF-001 (TTFT autocompletado) | <200ms | <100ms | ~2x |
| RNF-003 (Búsqueda codebase) | <100ms | <20ms | ~5x |
| RNF-004 (Startup time) | <500ms | <400ms | ~20% |
| RNF-005 (Memoria idle) | <100MB | <90MB | ~10% |
| RNF-007 (100K archivos) | Funcional | Fluido (<5s scan) | Viable |

---

*Documento de referencia para implementación. Sujeto a refinamiento durante desarrollo.*
