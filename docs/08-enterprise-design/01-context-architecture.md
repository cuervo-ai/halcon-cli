# FASE 1 — Arquitectura de Contexto (Context Layer)

> **Versión**: 1.0.0 | **Fecha**: 2026-02-06
> **Autores**: Enterprise Architecture Team
> **Estado**: Design Complete — Ready for Implementation Review

---

## 1. Visión General

El Context Layer es el subsistema responsable de capturar, almacenar, versionar, aislar y servir toda la información contextual que los agentes de Cuervo necesitan para operar de forma inteligente. No es un simple key-value store: es un **sistema distribuido de conocimiento** que opera a múltiples escalas (usuario, proyecto, organización, agente) con garantías de aislamiento multi-tenant, consistencia eventual, y rendimiento sub-milisegundo para lecturas en hot path.

### 1.1 Principios de Diseño

| Principio | Justificación |
|-----------|---------------|
| **Tenant-first isolation** | Todo contexto pertenece a un tenant; no existe contexto "global" accesible entre tenants |
| **Layered resolution** | El contexto se resuelve en cascada: user > project > org > system defaults |
| **Immutable history** | Los cambios de contexto son append-only con versionado; nunca se sobreescriben |
| **Offline-first** | El contexto local (SQLite) es la fuente de verdad para el CLI; sync a cloud es eventual |
| **Lazy materialization** | Los embeddings y contexto semántico se generan bajo demanda, no proactivamente |
| **Encryption at rest** | Todo contexto sensible (tokens, secretos, PII) se cifra con AES-256-GCM |
| **TTL-driven eviction** | Cada tipo de contexto tiene un TTL configurable; la invalidación es explícita o por TTL |

---

## 2. Taxonomía del Contexto

### 2.1 Dimensiones del Contexto

```
┌─────────────────────────────────────────────────────────────────────────┐
│                        CONTEXT DIMENSIONS                               │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐                  │
│  │   IDENTITY    │  │   SCOPE      │  │   TEMPORAL   │                  │
│  │               │  │              │  │              │                  │
│  │  • User       │  │  • Session   │  │  • Ephemeral │                  │
│  │  • Service    │  │  • Project   │  │  • Short     │                  │
│  │  • Agent      │  │  • Org       │  │  • Persistent│                  │
│  │  • System     │  │  • Global    │  │  • Archival  │                  │
│  └──────────────┘  └──────────────┘  └──────────────┘                  │
│                                                                         │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐                  │
│  │   TYPE        │  │   SECURITY   │  │   OPERATIONAL│                  │
│  │               │  │              │  │              │                  │
│  │  • Config     │  │  • Roles     │  │  • Quotas    │                  │
│  │  • Memory     │  │  • Scopes    │  │  • Costs     │                  │
│  │  • Semantic   │  │  • Policies  │  │  • Models    │                  │
│  │  • Operational│  │  • Secrets   │  │  • Features  │                  │
│  └──────────────┘  └──────────────┘  └──────────────┘                  │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
```

### 2.2 Tipos de Contexto

| Tipo | Scope | Persistencia | Storage Primario | Ejemplo |
|------|-------|-------------|-----------------|---------|
| **User Context** | Per-user | Persistent | SQLite / PostgreSQL | Preferencias, historial, shortcuts |
| **Project Context** | Per-repo | Persistent | SQLite + .cuervo/ | Configuración del proyecto, reglas, patrones |
| **Organization Context** | Per-tenant | Persistent | PostgreSQL | Políticas, modelos permitidos, SSO config |
| **Session Context** | Per-session | Ephemeral | In-memory + SQLite | Conversación actual, estado del agente |
| **Agent Memory** | Per-agent-instance | Mixed | SQLite + Vector DB | Decisiones pasadas, aprendizajes |
| **Semantic Context** | Per-project | Persistent | Vector DB (pgvector/Qdrant) | Embeddings del codebase, RAG index |
| **Security Context** | Per-user-session | Ephemeral | In-memory (encrypted) | JWT claims, roles, scopes, permissions |
| **Operational Context** | Per-org | Persistent | PostgreSQL + Redis | Cuotas, costos acumulados, feature flags |
| **Workspace Config** | Per-workspace | Persistent | .cuervo/config.yml | Settings dinámicos del workspace |

---

## 3. Modelos de Datos

### 3.1 Core Context Entity

```typescript
// domain/entities/context.ts

/**
 * Represents the hierarchical scope at which context is defined.
 * Resolution order: SESSION > USER > PROJECT > ORG > SYSTEM
 */
enum ContextScope {
  SYSTEM = 'system',       // Platform defaults (immutable by tenants)
  ORGANIZATION = 'org',    // Org-wide policies and configs
  PROJECT = 'project',     // Project-specific settings
  USER = 'user',           // User preferences
  SESSION = 'session',     // Current session state
  AGENT = 'agent',         // Agent-specific memory
}

enum ContextType {
  CONFIG = 'config',
  MEMORY = 'memory',
  SEMANTIC = 'semantic',
  SECURITY = 'security',
  OPERATIONAL = 'operational',
}

/**
 * Immutable context entry. All mutations create new versions.
 */
interface ContextEntry<T = unknown> {
  /** Globally unique identifier */
  readonly id: string;

  /** Tenant isolation boundary */
  readonly tenantId: string;

  /** Hierarchical scope */
  readonly scope: ContextScope;

  /** Classification for routing and storage */
  readonly type: ContextType;

  /** Dot-notation key (e.g., "models.default", "security.mfa_required") */
  readonly key: string;

  /** Typed value — schema-validated per key */
  readonly value: T;

  /** Monotonically increasing version within this key's lineage */
  readonly version: number;

  /** Hash of previous version (append-only chain integrity) */
  readonly parentHash: string | null;

  /** SHA-256 hash of (tenantId + scope + key + value + version + parentHash) */
  readonly hash: string;

  /** ISO-8601 creation timestamp */
  readonly createdAt: string;

  /** Identity that created this entry */
  readonly createdBy: string;

  /** TTL in seconds; null = never expires */
  readonly ttlSeconds: number | null;

  /** Expiration timestamp (computed from createdAt + ttlSeconds) */
  readonly expiresAt: string | null;

  /** Encryption metadata if value is encrypted at rest */
  readonly encryption: EncryptionMeta | null;

  /** Optional tags for indexing and filtering */
  readonly tags: Record<string, string>;
}

interface EncryptionMeta {
  algorithm: 'aes-256-gcm';
  keyId: string;       // Reference to encryption key in key management
  iv: string;          // Base64-encoded initialization vector
  authTag: string;     // GCM authentication tag
}

/**
 * Resolved context: the final merged view after cascading resolution.
 */
interface ResolvedContext {
  readonly tenantId: string;
  readonly userId: string;
  readonly projectId: string | null;
  readonly sessionId: string;
  readonly entries: Map<string, ContextEntry>;
  readonly resolvedAt: string;
  readonly resolutionChain: ContextScope[]; // Scopes that contributed
}
```

### 3.2 User Context Model

```typescript
// domain/entities/user-context.ts

interface UserContext {
  readonly userId: string;
  readonly tenantId: string;

  /** User preferences */
  preferences: {
    defaultModel: string;
    theme: 'dark' | 'light' | 'auto';
    language: string;           // ISO 639-1
    confirmDestructive: boolean;
    streamResponses: boolean;
    maxTokensPerRequest: number;
    editorIntegration: 'vscode' | 'neovim' | 'jetbrains' | 'none';
    outputVerbosity: 'minimal' | 'normal' | 'verbose' | 'debug';
  };

  /** Persistent memory across sessions */
  memory: {
    facts: UserFact[];           // Learned facts about user's codebase/style
    shortcuts: CommandShortcut[]; // Custom command aliases
    recentProjects: ProjectRef[];
    pinnedContexts: string[];    // Context IDs always loaded
  };

  /** Usage tracking */
  usage: {
    totalTokensUsed: number;
    totalCostUSD: number;
    sessionsCount: number;
    lastActiveAt: string;
    modelUsageBreakdown: Record<string, { tokens: number; cost: number }>;
  };

  /** Security preferences */
  security: {
    mfaEnabled: boolean;
    trustedDevices: TrustedDevice[];
    apiKeyRotationDays: number;
    sessionTimeoutMinutes: number;
  };
}

interface UserFact {
  id: string;
  content: string;
  source: 'explicit' | 'inferred' | 'imported';
  confidence: number;     // 0.0 - 1.0
  createdAt: string;
  lastUsedAt: string;
  usageCount: number;
  embedding?: Float32Array; // For semantic retrieval
}

interface CommandShortcut {
  alias: string;
  command: string;
  description: string;
  scope: 'global' | 'project';
}

interface TrustedDevice {
  deviceId: string;
  name: string;
  fingerprint: string;
  lastUsedAt: string;
  expiresAt: string;
}
```

### 3.3 Project Context Model

```typescript
// domain/entities/project-context.ts

interface ProjectContext {
  readonly projectId: string;
  readonly tenantId: string;
  readonly repositoryUrl: string | null;

  /** Project configuration (.cuervo/config.yml materialized) */
  config: {
    models: ModelConfig;
    tools: ToolPermissions;
    agents: AgentConfig;
    security: ProjectSecurityConfig;
    integrations: IntegrationConfig[];
  };

  /** Codebase understanding */
  codebase: {
    /** File tree snapshot (refreshed on git changes) */
    fileTree: FileTreeNode;

    /** Detected languages and frameworks */
    stack: DetectedStack;

    /** Architectural patterns detected */
    patterns: ArchPattern[];

    /** Convention analysis (naming, structure, testing) */
    conventions: CodeConventions;

    /** Last indexing metadata */
    lastIndexedAt: string;
    lastIndexedCommit: string;
    indexingStatus: 'idle' | 'indexing' | 'stale' | 'error';
  };

  /** Semantic index for RAG */
  semanticIndex: {
    totalChunks: number;
    totalEmbeddings: number;
    embeddingModel: string;
    lastUpdatedAt: string;
    coverage: number; // 0.0 - 1.0 (percentage of files indexed)
  };

  /** Project-specific memory */
  memory: {
    decisions: ArchitecturalDecision[];  // ADRs learned from interactions
    patterns: LearnedPattern[];           // Code patterns specific to this project
    rules: ProjectRule[];                 // .cuervo/rules.md materialized
  };
}

interface DetectedStack {
  primaryLanguage: string;
  languages: { language: string; percentage: number }[];
  frameworks: string[];
  buildTools: string[];
  testFrameworks: string[];
  packageManager: string | null;
  cicd: string | null;
}

interface ArchitecturalDecision {
  id: string;
  title: string;
  context: string;
  decision: string;
  consequences: string[];
  learnedAt: string;
  confidence: number;
  source: 'explicit' | 'inferred';
}

interface ModelConfig {
  default: string;
  profiles: Record<string, {
    provider: string;
    model: string;
    maxTokens: number;
    temperature: number;
  }>;
  routing: {
    auto: boolean;
    fallbackChain: string[];
    rules: RoutingRule[];
  };
  budget: {
    dailyLimitUSD: number;
    perRequestMaxUSD: number;
    warnThresholdUSD: number;
    monthlyLimitUSD: number;
  };
}

interface RoutingRule {
  condition: {
    taskType?: string;
    complexity?: 'trivial' | 'simple' | 'medium' | 'complex';
    fileCount?: { gt?: number; lt?: number };
    language?: string;
  };
  model: string;
  priority: number;
}
```

### 3.4 Organization Context Model

```typescript
// domain/entities/organization-context.ts

interface OrganizationContext {
  readonly orgId: string;
  readonly tenantId: string; // Usually same as orgId for top-level orgs

  /** Organization identity */
  identity: {
    name: string;
    slug: string;
    plan: 'free' | 'team' | 'business' | 'enterprise';
    createdAt: string;
    billingEmail: string;
  };

  /** Security policies enforced org-wide */
  securityPolicies: {
    /** Require MFA for all members */
    mfaRequired: boolean;

    /** Allowed authentication methods */
    allowedAuthMethods: ('password' | 'sso' | 'passkey')[];

    /** SSO configuration */
    sso: SSOConfig | null;

    /** SCIM provisioning */
    scim: SCIMConfig | null;

    /** Session constraints */
    maxSessionDuration: number;        // minutes
    maxConcurrentSessions: number;

    /** IP allowlisting */
    ipAllowlist: string[] | null;      // CIDR ranges; null = no restriction

    /** Data residency */
    dataResidency: 'us' | 'eu' | 'latam' | 'ap' | null;

    /** Encryption requirements */
    requireEncryptionAtRest: boolean;
    customerManagedKeys: boolean;

    /** Model restrictions */
    allowedProviders: string[] | null; // null = all allowed
    allowedModels: string[] | null;
    blockedModels: string[];

    /** Tool restrictions */
    blockedTools: string[];
    requireApprovalFor: string[];      // Tools requiring human approval

    /** Zero-retention mode */
    zeroRetention: boolean;

    /** Audit requirements */
    auditLogRetentionDays: number;
    auditLogExportEnabled: boolean;
    auditLogDestination: AuditExportConfig | null;
  };

  /** Operational quotas */
  quotas: {
    maxMembers: number;
    maxProjects: number;
    maxTokensPerDay: number;
    maxTokensPerMonth: number;
    maxCostPerDay: number;
    maxCostPerMonth: number;
    maxConcurrentAgents: number;
    maxStorageGB: number;
    rateLimits: {
      requestsPerMinute: number;
      requestsPerHour: number;
      tokensPerMinute: number;
    };
  };

  /** Feature flags */
  features: {
    multiAgent: boolean;
    rag: boolean;
    plugins: boolean;
    customModels: boolean;
    fineTuning: boolean;
    offlineMode: boolean;
    apiAccess: boolean;
    webhooks: boolean;
    sso: boolean;
    scim: boolean;
    advancedAudit: boolean;
    dataExport: boolean;
    customIntegrations: boolean;
  };

  /** Usage tracking (aggregated) */
  usage: {
    currentPeriodStart: string;
    totalTokensUsed: number;
    totalCostUSD: number;
    activeMembers: number;
    activeProjects: number;
    storageUsedGB: number;
  };
}

interface SSOConfig {
  provider: 'okta' | 'azure_ad' | 'google_workspace' | 'onelogin' | 'custom';
  protocol: 'oidc' | 'saml';
  issuerUrl: string;
  clientId: string;
  /** Encrypted at rest */
  clientSecret: string;
  /** SAML-specific */
  metadataUrl?: string;
  certificate?: string;
  /** Attribute mappings */
  attributeMapping: {
    email: string;
    firstName: string;
    lastName: string;
    groups?: string;
    role?: string;
  };
  /** Auto-provision users on first login */
  jitProvisioning: boolean;
  /** Default role for JIT-provisioned users */
  defaultRole: string;
  /** Enforce SSO (disable password login) */
  enforced: boolean;
}

interface SCIMConfig {
  enabled: boolean;
  endpoint: string;
  bearerToken: string; // Encrypted
  syncInterval: number; // minutes
  deprovisionAction: 'suspend' | 'delete';
  groupMapping: Record<string, string>; // IdP group -> Cuervo role
}

interface AuditExportConfig {
  type: 'siem' | 's3' | 'gcs' | 'azure_blob' | 'webhook';
  endpoint: string;
  credentials: string; // Encrypted reference
  format: 'json' | 'cef' | 'leef';
  batchSize: number;
  flushInterval: number; // seconds
}
```

### 3.5 Session Context Model

```typescript
// domain/entities/session-context.ts

interface SessionContext {
  readonly sessionId: string;
  readonly userId: string;
  readonly tenantId: string;
  readonly projectId: string | null;

  /** Current conversation state */
  conversation: {
    messages: ConversationMessage[];
    totalTokens: number;
    totalCost: number;
    startedAt: string;
    lastActivityAt: string;
  };

  /** Active agent states */
  agents: Map<string, AgentState>;

  /** Working set: files currently in context */
  workingSet: {
    files: WorkingFile[];
    totalTokenEstimate: number;
    maxTokenBudget: number;
  };

  /** Pending operations (tool calls awaiting confirmation) */
  pendingOperations: PendingOperation[];

  /** Session-scoped feature overrides */
  overrides: Map<string, unknown>;

  /** Git state snapshot */
  gitState: {
    branch: string;
    uncommittedChanges: number;
    lastCommit: string;
    isClean: boolean;
  } | null;
}

interface AgentState {
  agentId: string;
  agentType: 'explorer' | 'planner' | 'executor' | 'reviewer' | 'custom';
  status: 'idle' | 'thinking' | 'executing' | 'waiting_approval' | 'error';
  currentTask: string | null;
  shortTermMemory: Map<string, unknown>;
  toolCallHistory: ToolCallRecord[];
  tokenBudgetRemaining: number;
}

interface WorkingFile {
  path: string;
  hash: string;          // SHA-256 of content
  tokenEstimate: number;
  relevanceScore: number; // 0.0 - 1.0
  addedAt: string;
  source: 'explicit' | 'auto' | 'rag';
}

interface ToolCallRecord {
  toolName: string;
  input: Record<string, unknown>;
  output: string;
  durationMs: number;
  timestamp: string;
  approved: boolean;
}
```

### 3.6 Agent Memory Model

```typescript
// domain/entities/agent-memory.ts

interface AgentMemory {
  readonly agentId: string;
  readonly tenantId: string;
  readonly projectId: string;

  /** Episodic memory: significant events and outcomes */
  episodes: Episode[];

  /** Semantic memory: learned facts and patterns */
  knowledge: KnowledgeEntry[];

  /** Procedural memory: successful strategies */
  procedures: Procedure[];

  /** Working memory: current task context (volatile) */
  workingMemory: {
    currentGoal: string | null;
    subGoals: string[];
    hypotheses: Hypothesis[];
    observations: Observation[];
    constraints: string[];
  };
}

interface Episode {
  id: string;
  timestamp: string;
  type: 'success' | 'failure' | 'correction' | 'discovery';
  summary: string;
  context: {
    taskType: string;
    filesInvolved: string[];
    modelUsed: string;
    userFeedback: string | null;
  };
  embedding: Float32Array;
  importance: number; // 0.0 - 1.0 (for memory consolidation)
}

interface KnowledgeEntry {
  id: string;
  fact: string;
  source: 'observation' | 'user_correction' | 'documentation' | 'code_analysis';
  confidence: number;
  createdAt: string;
  lastValidatedAt: string;
  validationCount: number;
  embedding: Float32Array;
  scope: 'project' | 'language' | 'framework' | 'general';
}

interface Procedure {
  id: string;
  name: string;
  trigger: string;          // When to apply this procedure
  steps: string[];           // Ordered steps
  successRate: number;       // Historical success rate
  applicableWhen: string[];  // Conditions
  lastUsedAt: string;
  usageCount: number;
}

interface Hypothesis {
  statement: string;
  confidence: number;
  evidence: string[];
  createdAt: string;
}

interface Observation {
  content: string;
  source: string;
  timestamp: string;
  relevance: number;
}
```

---

## 4. Arquitectura de Storage

### 4.1 Estrategia Multi-Store

```
┌────────────────────────────────────────────────────────────────────────────┐
│                         CONTEXT STORAGE ARCHITECTURE                       │
├────────────────────────────────────────────────────────────────────────────┤
│                                                                            │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                        HOT PATH (< 1ms)                             │   │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐              │   │
│  │  │  L1: Process  │  │  L1: Session │  │  L1: Security│              │   │
│  │  │  Memory LRU   │  │  State Cache │  │  Token Cache │              │   │
│  │  │  (100MB max)  │  │  (per REPL)  │  │  (JWT claims)│              │   │
│  │  └──────────────┘  └──────────────┘  └──────────────┘              │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                              │ miss                                         │
│                              ▼                                              │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                       WARM PATH (1-10ms)                            │   │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐              │   │
│  │  │  L2: SQLite   │  │  L2: SQLite  │  │  L2: Redis   │              │   │
│  │  │  Context DB   │  │  Vector Ext  │  │  (cloud only)│              │   │
│  │  │  (local)      │  │  (embeddings)│  │  (shared)    │              │   │
│  │  └──────────────┘  └──────────────┘  └──────────────┘              │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                              │ miss                                         │
│                              ▼                                              │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                       COLD PATH (10-100ms)                          │   │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐              │   │
│  │  │  L3: Postgres │  │  L3: Qdrant  │  │  L3: S3/GCS  │              │   │
│  │  │  (cloud SQL)  │  │  (vectors)   │  │  (archives)  │              │   │
│  │  └──────────────┘  └──────────────┘  └──────────────┘              │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                                                            │
└────────────────────────────────────────────────────────────────────────────┘
```

### 4.2 Storage por Tipo de Contexto

| Contexto | Local (CLI) | Cloud (Enterprise) | Sync Strategy |
|----------|------------|-------------------|---------------|
| User preferences | SQLite `~/.cuervo/context.db` | PostgreSQL `user_contexts` | Bidirectional merge (LWW) |
| Project config | `.cuervo/config.yml` + SQLite | PostgreSQL `project_contexts` | Git-tracked (config.yml) + cloud sync |
| Org policies | SQLite cache | PostgreSQL `org_contexts` | Pull-on-connect, TTL 5min |
| Session state | In-memory | Redis (optional) | No sync (ephemeral) |
| Agent memory | SQLite `project.db` | PostgreSQL + pgvector | Push-on-session-end |
| Semantic index | SQLite vec / Hnswlib | Qdrant | Rebuild on cloud (separate index) |
| Security context | In-memory (encrypted) | Redis (encrypted) | No sync (derived from JWT) |
| Operational context | SQLite cache | PostgreSQL + Redis | Pull-on-connect, TTL 1min |

### 4.3 Schema SQL — Local SQLite

```sql
-- Context entries (generic key-value with versioning)
CREATE TABLE context_entries (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    scope TEXT NOT NULL CHECK (scope IN ('system','org','project','user','session','agent')),
    type TEXT NOT NULL CHECK (type IN ('config','memory','semantic','security','operational')),
    key TEXT NOT NULL,
    value_json TEXT NOT NULL,         -- JSON-serialized value
    value_encrypted BLOB,            -- AES-256-GCM encrypted (mutually exclusive with value_json)
    encryption_key_id TEXT,
    encryption_iv TEXT,
    encryption_auth_tag TEXT,
    version INTEGER NOT NULL DEFAULT 1,
    parent_hash TEXT,
    hash TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f', 'now')),
    created_by TEXT NOT NULL,
    ttl_seconds INTEGER,
    expires_at TEXT,
    tags_json TEXT DEFAULT '{}',
    is_deleted INTEGER NOT NULL DEFAULT 0,    -- Soft delete

    UNIQUE(tenant_id, scope, key, version)
);

-- Index for fast scope-based resolution
CREATE INDEX idx_context_resolution
    ON context_entries(tenant_id, scope, key, is_deleted)
    WHERE is_deleted = 0;

-- Index for TTL expiration cleanup
CREATE INDEX idx_context_expiry
    ON context_entries(expires_at)
    WHERE expires_at IS NOT NULL AND is_deleted = 0;

-- Index for type-based queries
CREATE INDEX idx_context_type
    ON context_entries(tenant_id, type, is_deleted)
    WHERE is_deleted = 0;

-- Agent memory episodes
CREATE TABLE agent_episodes (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    type TEXT NOT NULL CHECK (type IN ('success','failure','correction','discovery')),
    summary TEXT NOT NULL,
    context_json TEXT NOT NULL,
    importance REAL NOT NULL DEFAULT 0.5,
    embedding BLOB,                   -- Float32Array serialized
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f', 'now')),

    FOREIGN KEY (tenant_id) REFERENCES tenants(id)
);

CREATE INDEX idx_episodes_lookup
    ON agent_episodes(tenant_id, project_id, agent_id, type);

-- Agent knowledge base
CREATE TABLE agent_knowledge (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    fact TEXT NOT NULL,
    source TEXT NOT NULL,
    confidence REAL NOT NULL DEFAULT 0.5,
    scope TEXT NOT NULL DEFAULT 'project',
    embedding BLOB,
    created_at TEXT NOT NULL,
    last_validated_at TEXT NOT NULL,
    validation_count INTEGER NOT NULL DEFAULT 0,

    FOREIGN KEY (tenant_id) REFERENCES tenants(id)
);

-- Semantic index chunks (for RAG)
CREATE TABLE semantic_chunks (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    file_path TEXT NOT NULL,
    file_hash TEXT NOT NULL,          -- SHA-256 of source file
    chunk_index INTEGER NOT NULL,
    chunk_type TEXT NOT NULL,          -- 'function', 'class', 'module', 'comment', 'block'
    content TEXT NOT NULL,
    start_line INTEGER NOT NULL,
    end_line INTEGER NOT NULL,
    language TEXT NOT NULL,
    embedding BLOB NOT NULL,          -- Float32Array
    token_count INTEGER NOT NULL,
    metadata_json TEXT DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f', 'now')),

    UNIQUE(tenant_id, project_id, file_path, chunk_index)
);

CREATE INDEX idx_chunks_file
    ON semantic_chunks(tenant_id, project_id, file_path);

-- Conversations & messages
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    project_id TEXT,
    started_at TEXT NOT NULL,
    last_activity_at TEXT NOT NULL,
    total_tokens INTEGER NOT NULL DEFAULT 0,
    total_cost_usd REAL NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'active',
    metadata_json TEXT DEFAULT '{}',

    FOREIGN KEY (tenant_id) REFERENCES tenants(id)
);

CREATE TABLE messages (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    tenant_id TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('user','assistant','system','tool')),
    content TEXT NOT NULL,
    tool_calls_json TEXT,
    tokens_input INTEGER,
    tokens_output INTEGER,
    cost_usd REAL,
    model TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f', 'now')),

    FOREIGN KEY (session_id) REFERENCES sessions(id),
    FOREIGN KEY (tenant_id) REFERENCES tenants(id)
);

CREATE INDEX idx_messages_session
    ON messages(session_id, created_at);
```

### 4.4 Schema SQL — Cloud PostgreSQL

```sql
-- Tenant isolation via Row Level Security (RLS)
CREATE TABLE tenants (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    slug TEXT UNIQUE NOT NULL,
    name TEXT NOT NULL,
    plan TEXT NOT NULL DEFAULT 'free',
    data_residency TEXT DEFAULT 'us',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    settings JSONB NOT NULL DEFAULT '{}',

    CONSTRAINT valid_plan CHECK (plan IN ('free','team','business','enterprise')),
    CONSTRAINT valid_residency CHECK (data_residency IN ('us','eu','latam','ap'))
);

-- Enable RLS on all tenant-scoped tables
ALTER TABLE tenants ENABLE ROW LEVEL SECURITY;

-- Context entries with RLS
CREATE TABLE context_entries (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    scope TEXT NOT NULL,
    type TEXT NOT NULL,
    key TEXT NOT NULL,
    value JSONB NOT NULL,
    value_encrypted BYTEA,
    encryption_meta JSONB,
    version INTEGER NOT NULL DEFAULT 1,
    parent_hash TEXT,
    hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by UUID NOT NULL,
    ttl_seconds INTEGER,
    expires_at TIMESTAMPTZ,
    tags JSONB DEFAULT '{}',
    is_deleted BOOLEAN NOT NULL DEFAULT false,

    UNIQUE(tenant_id, scope, key, version)
);

ALTER TABLE context_entries ENABLE ROW LEVEL SECURITY;

-- RLS policy: users can only see their tenant's data
CREATE POLICY tenant_isolation ON context_entries
    USING (tenant_id = current_setting('app.current_tenant')::UUID);

-- Partition context_entries by tenant for large-scale deployments
-- (Optional: for >1000 tenants)
-- CREATE TABLE context_entries_partitioned (LIKE context_entries INCLUDING ALL)
--     PARTITION BY HASH (tenant_id);

-- Vector embeddings with pgvector
CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE embeddings (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    project_id UUID NOT NULL,
    source_type TEXT NOT NULL,        -- 'code_chunk', 'episode', 'knowledge', 'document'
    source_id TEXT NOT NULL,          -- Reference to source record
    content_preview TEXT,             -- First 200 chars for debugging
    embedding vector(1536) NOT NULL,  -- Dimension depends on model
    metadata JSONB DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

ALTER TABLE embeddings ENABLE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON embeddings
    USING (tenant_id = current_setting('app.current_tenant')::UUID);

-- HNSW index for fast ANN search (per-tenant scoped queries)
CREATE INDEX idx_embeddings_vector
    ON embeddings
    USING hnsw (embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 200);

CREATE INDEX idx_embeddings_tenant_project
    ON embeddings(tenant_id, project_id, source_type);

-- Operational counters (for quotas)
CREATE TABLE usage_counters (
    tenant_id UUID NOT NULL REFERENCES tenants(id),
    period TEXT NOT NULL,              -- '2026-02', '2026-02-06'
    counter_type TEXT NOT NULL,        -- 'tokens', 'cost', 'requests', 'storage_bytes'
    value BIGINT NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (tenant_id, period, counter_type)
);

ALTER TABLE usage_counters ENABLE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON usage_counters
    USING (tenant_id = current_setting('app.current_tenant')::UUID);
```

---

## 5. Versionado de Contexto

### 5.1 Modelo de Versionado

El contexto usa un modelo **append-only con hash chain** inspirado en Git:

```
Version 1 (hash: abc123, parent: null)
    │
    ▼
Version 2 (hash: def456, parent: abc123)
    │
    ▼
Version 3 (hash: ghi789, parent: def456)  ← current
```

```typescript
// domain/services/context-versioning.ts

interface ContextVersioningService {
  /**
   * Create a new version of a context entry.
   * Computes hash chain, validates parent, increments version.
   */
  createVersion<T>(
    tenantId: string,
    scope: ContextScope,
    key: string,
    value: T,
    createdBy: string,
  ): Promise<ContextEntry<T>>;

  /**
   * Get the current (latest) version of a context entry.
   */
  getCurrent<T>(
    tenantId: string,
    scope: ContextScope,
    key: string,
  ): Promise<ContextEntry<T> | null>;

  /**
   * Get a specific version.
   */
  getVersion<T>(
    tenantId: string,
    scope: ContextScope,
    key: string,
    version: number,
  ): Promise<ContextEntry<T> | null>;

  /**
   * Get version history for a context key.
   */
  getHistory(
    tenantId: string,
    scope: ContextScope,
    key: string,
    options?: { limit?: number; before?: string },
  ): Promise<ContextEntry[]>;

  /**
   * Verify integrity of the hash chain for a key.
   */
  verifyChain(
    tenantId: string,
    scope: ContextScope,
    key: string,
  ): Promise<{ valid: boolean; brokenAt?: number }>;

  /**
   * Compact old versions (keep last N, archive rest).
   * Only callable by system processes, respects retention policies.
   */
  compact(
    tenantId: string,
    scope: ContextScope,
    key: string,
    keepVersions: number,
  ): Promise<{ compacted: number; archived: number }>;
}
```

### 5.2 Hash Computation

```typescript
import { createHash } from 'node:crypto';

function computeContextHash(entry: {
  tenantId: string;
  scope: string;
  key: string;
  value: unknown;
  version: number;
  parentHash: string | null;
}): string {
  const payload = JSON.stringify({
    t: entry.tenantId,
    s: entry.scope,
    k: entry.key,
    v: entry.value,
    n: entry.version,
    p: entry.parentHash,
  });
  return createHash('sha256').update(payload).digest('hex');
}
```

---

## 6. Resolución de Contexto (Context Resolution)

### 6.1 Algoritmo de Resolución en Cascada

La resolución sigue un orden de precedencia donde los scopes más específicos override los menos específicos:

```
SESSION (highest priority)
    ↓ fallback
  USER
    ↓ fallback
  PROJECT
    ↓ fallback
  ORGANIZATION
    ↓ fallback
  SYSTEM (lowest priority — platform defaults)
```

```typescript
// application/services/context-resolver.ts

interface ContextResolver {
  /**
   * Resolve a single context key across all scopes.
   * Returns the value from the highest-priority scope that has it defined.
   */
  resolve<T>(
    params: ResolutionParams,
    key: string,
  ): Promise<ResolvedValue<T>>;

  /**
   * Resolve all context entries for a given session.
   * Merges all scopes according to precedence.
   */
  resolveAll(
    params: ResolutionParams,
  ): Promise<ResolvedContext>;

  /**
   * Resolve with explicit scope override (skip cascading).
   */
  resolveAtScope<T>(
    params: ResolutionParams,
    scope: ContextScope,
    key: string,
  ): Promise<ContextEntry<T> | null>;
}

interface ResolutionParams {
  tenantId: string;
  userId: string;
  projectId?: string;
  sessionId: string;
  /** Override scopes to check (default: all) */
  scopes?: ContextScope[];
  /** If true, include resolution metadata */
  includeTrace?: boolean;
}

interface ResolvedValue<T> {
  value: T;
  source: ContextScope;          // Which scope provided this value
  version: number;
  resolvedAt: string;
  /** Resolution trace (if includeTrace = true) */
  trace?: {
    checked: ContextScope[];
    found: ContextScope;
    duration_us: number;
  };
}
```

### 6.2 Merge Strategies

Para contextos complejos (objetos, arrays), la resolución soporta múltiples estrategias de merge:

```typescript
enum MergeStrategy {
  /** Last scope wins completely (default) */
  OVERRIDE = 'override',

  /** Deep merge objects; arrays concatenated */
  DEEP_MERGE = 'deep_merge',

  /** Deep merge objects; arrays deduplicated */
  DEEP_MERGE_UNIQUE = 'deep_merge_unique',

  /** Only merge top-level keys */
  SHALLOW_MERGE = 'shallow_merge',

  /** Additive only — child scopes can add but not remove/override */
  ADDITIVE = 'additive',
}

/**
 * Configuration for how specific context keys should be merged.
 * Defined at the schema level.
 */
const MERGE_RULES: Record<string, MergeStrategy> = {
  'models.default':        MergeStrategy.OVERRIDE,
  'models.profiles':       MergeStrategy.DEEP_MERGE,
  'models.budget':         MergeStrategy.OVERRIDE,       // Org sets limit; user can't override
  'tools.blocked':         MergeStrategy.ADDITIVE,        // Org blocks propagate down
  'tools.permissions':     MergeStrategy.DEEP_MERGE,
  'security.*':            MergeStrategy.OVERRIDE,        // Org security is authoritative
  'features.*':            MergeStrategy.OVERRIDE,        // Plan-based, not overridable
  'preferences.*':         MergeStrategy.OVERRIDE,        // User preferences win
};
```

---

## 7. Aislamiento Multi-Tenant

### 7.1 Estrategia de Aislamiento

```
┌──────────────────────────────────────────────────────────────────┐
│                    TENANT ISOLATION MODEL                        │
├──────────────────────────────────────────────────────────────────┤
│                                                                  │
│   TIER: FREE / TEAM                                              │
│   ┌──────────────────────────────────────────┐                   │
│   │  Shared Database (PostgreSQL)            │                   │
│   │  ├── Row-Level Security (RLS)            │                   │
│   │  ├── tenant_id on every row              │                   │
│   │  └── Connection-level SET app.tenant     │                   │
│   └──────────────────────────────────────────┘                   │
│                                                                  │
│   TIER: BUSINESS                                                 │
│   ┌──────────────────────────────────────────┐                   │
│   │  Shared Database, Separate Schema        │                   │
│   │  ├── CREATE SCHEMA tenant_<id>           │                   │
│   │  ├── RLS as defense-in-depth             │                   │
│   │  └── Schema-level resource quotas        │                   │
│   └──────────────────────────────────────────┘                   │
│                                                                  │
│   TIER: ENTERPRISE                                               │
│   ┌──────────────────────────────────────────┐                   │
│   │  Dedicated Database Instance             │                   │
│   │  ├── Separate PostgreSQL cluster         │                   │
│   │  ├── Customer-managed encryption keys    │                   │
│   │  ├── Dedicated Qdrant namespace/cluster  │                   │
│   │  └── Data residency compliance           │                   │
│   └──────────────────────────────────────────┘                   │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

### 7.2 Enforcement Points

```typescript
// infrastructure/middleware/tenant-isolation.ts

/**
 * Middleware that sets the tenant context for every database operation.
 * Applied at the infrastructure layer — domain layer is tenant-unaware.
 */
interface TenantIsolationMiddleware {
  /**
   * Extract tenant from authenticated request context (JWT claim).
   * Set on database connection: SET app.current_tenant = '<tenant_id>'
   * All RLS policies reference current_setting('app.current_tenant').
   */
  setTenantForRequest(tenantId: string): Promise<void>;

  /**
   * Verify that a resource belongs to the requesting tenant.
   * Defense-in-depth: even with RLS, application-level check.
   */
  verifyResourceOwnership(
    resourceId: string,
    resourceType: string,
    tenantId: string,
  ): Promise<boolean>;

  /**
   * Ensure cross-tenant queries are impossible.
   * Throws if tenantId in query differs from session tenant.
   */
  guardCrossTenantAccess(
    requestedTenantId: string,
    sessionTenantId: string,
  ): void;
}
```

### 7.3 Local CLI Isolation

En modo local (CLI standalone), el aislamiento se realiza a nivel de filesystem:

```
~/.cuervo/
├── context.db                    # User-scoped SQLite (single tenant)
├── config.yml                    # User preferences
├── credentials/                  # OS keychain references
└── cache/                        # Ephemeral cache

/project/.cuervo/
├── config.yml                    # Project-scoped config
├── rules.md                      # Project rules
├── project.db                    # Project-scoped SQLite
└── index/                        # Semantic index (embeddings)
```

---

## 8. Caching y Performance

### 8.1 Multi-Level Cache Architecture

```typescript
// infrastructure/cache/context-cache.ts

interface ContextCache {
  /**
   * L1: In-process memory (LRU).
   * - Hit time: < 0.1ms
   * - Capacity: 100MB (configurable)
   * - TTL: 5 minutes for config, 30s for security context
   * - Eviction: LRU with size-awareness
   */
  readonly l1: InMemoryLRUCache;

  /**
   * L2: SQLite local database.
   * - Hit time: 1-5ms
   * - Capacity: 1GB (configurable)
   * - TTL: 24h for semantic cache, 1h for context entries
   * - Used for: resolved contexts, embeddings, semantic cache
   */
  readonly l2: SQLiteCache;

  /**
   * L3: Redis (cloud mode only).
   * - Hit time: 5-20ms
   * - Capacity: Shared pool, per-tenant quotas
   * - TTL: 1h for context, 5min for security
   * - Used for: org policies, shared configs, session state
   */
  readonly l3: RedisCache | null;

  /**
   * Read-through: check L1 → L2 → L3 → source.
   * Populate lower caches on miss.
   */
  get<T>(key: CacheKey): Promise<CacheResult<T>>;

  /**
   * Write-through: update source, then invalidate caches.
   */
  set<T>(key: CacheKey, value: T, options?: CacheOptions): Promise<void>;

  /**
   * Explicit invalidation (cascade from L1 to L3).
   */
  invalidate(key: CacheKey): Promise<void>;

  /**
   * Pattern-based invalidation (e.g., all keys for a tenant).
   */
  invalidatePattern(pattern: string): Promise<void>;
}

interface CacheKey {
  tenant: string;
  scope: ContextScope;
  key: string;
  /** Optional version specifier (default: latest) */
  version?: number;
}

interface CacheOptions {
  ttlSeconds?: number;
  /** Only cache in specific layers */
  layers?: ('l1' | 'l2' | 'l3')[];
  /** Priority for eviction (higher = keep longer) */
  priority?: 'low' | 'normal' | 'high';
}

interface CacheResult<T> {
  value: T | null;
  hit: boolean;
  layer: 'l1' | 'l2' | 'l3' | 'source';
  latencyUs: number;
}
```

### 8.2 Invalidación

| Trigger | Scope | Strategy |
|---------|-------|----------|
| Config change via API | Specific key | Direct invalidation L1→L2→L3 |
| Config change via CLI | Specific key | Local L1→L2; publish event to L3 |
| Org policy change | All org keys | Pattern invalidation `tenant:*:org:*` |
| SCIM user sync | User keys | Pattern invalidation `tenant:*:user:<uid>:*` |
| Model routing update | Routing keys | Direct invalidation + broadcast |
| Session timeout | Session keys | Automatic TTL expiry |
| Deployment (new version) | All caches | Global flush via deployment hook |

### 8.3 Performance Targets

| Operation | Target Latency | Strategy |
|-----------|---------------|----------|
| Resolve single key | < 1ms (L1 hit) | LRU cache pre-warmed on session start |
| Resolve all context | < 10ms | Batch resolution with parallel scope queries |
| Semantic search (top-10) | < 50ms | HNSW index with pre-filtered tenant scope |
| Full context rebuild | < 200ms | Lazy loading, only resolve accessed keys |
| Context sync (local→cloud) | < 2s | Background, non-blocking, delta sync |

---

## 9. Sincronización Offline-First

### 9.1 Sync Protocol

```typescript
// infrastructure/sync/context-sync.ts

interface ContextSyncService {
  /**
   * Sync local context changes to cloud.
   * Uses CRDT-inspired Last-Writer-Wins (LWW) with vector clocks.
   */
  pushChanges(
    tenantId: string,
    changes: ContextChange[],
  ): Promise<SyncResult>;

  /**
   * Pull cloud context changes to local.
   * Fetches delta since last sync timestamp.
   */
  pullChanges(
    tenantId: string,
    since: string,  // ISO-8601 timestamp
  ): Promise<ContextChange[]>;

  /**
   * Full bidirectional sync.
   * Detects conflicts, applies resolution strategy.
   */
  sync(tenantId: string): Promise<SyncReport>;

  /**
   * Register for real-time push notifications (WebSocket).
   * Org policy changes are pushed immediately.
   */
  subscribe(
    tenantId: string,
    onUpdate: (change: ContextChange) => void,
  ): Unsubscribe;
}

interface ContextChange {
  entryId: string;
  scope: ContextScope;
  key: string;
  value: unknown;
  version: number;
  hash: string;
  timestamp: string;
  origin: 'local' | 'cloud';
  /** Vector clock for conflict detection */
  vectorClock: Record<string, number>;
}

interface SyncReport {
  pushed: number;
  pulled: number;
  conflicts: ConflictResolution[];
  duration_ms: number;
  nextSyncAt: string;
}

interface ConflictResolution {
  key: string;
  localVersion: number;
  cloudVersion: number;
  resolution: 'local_wins' | 'cloud_wins' | 'merged';
  resolvedValue: unknown;
}
```

### 9.2 Conflict Resolution Rules

| Context Type | Resolution Strategy | Justification |
|-------------|-------------------|---------------|
| Org security policies | **Cloud wins always** | Security policy is authoritative |
| Org quotas | **Cloud wins always** | Billing/limits are server-side |
| User preferences | **Last-Writer-Wins (timestamp)** | User intent is latest write |
| Project config | **Git-tracked + LWW** | .cuervo/config.yml tracked in git |
| Agent memory | **Merge (append)** | All memories valuable |
| Session state | **No sync** | Ephemeral, session-scoped |
| Semantic index | **No sync** | Rebuilt per-environment |

### 9.3 Offline Queue

```typescript
// infrastructure/sync/offline-queue.ts

interface OfflineQueue {
  /**
   * Enqueue a change when offline.
   * Persisted to SQLite for durability.
   */
  enqueue(change: ContextChange): Promise<void>;

  /**
   * Flush queued changes when connectivity restored.
   * Applies in order, handles conflicts.
   */
  flush(): Promise<SyncReport>;

  /**
   * Get pending changes count.
   */
  pendingCount(): Promise<number>;

  /**
   * Discard stale changes (older than retention period).
   */
  prune(maxAgeHours: number): Promise<number>;
}
```

---

## 10. Contratos de API

### 10.1 Context Management API (REST)

```
BASE: /api/v1/context

┌────────────────────────────────────────────────────────────────────────┐
│ Endpoint                          │ Method │ Description               │
├────────────────────────────────────────────────────────────────────────┤
│ /entries                          │ GET    │ List context entries       │
│ /entries                          │ POST   │ Create context entry       │
│ /entries/:id                      │ GET    │ Get specific entry         │
│ /entries/:id                      │ PUT    │ Update (creates new ver)   │
│ /entries/:id                      │ DELETE │ Soft-delete entry          │
│ /entries/:id/history              │ GET    │ Get version history        │
│ /resolve                          │ POST   │ Resolve context (cascade)  │
│ /resolve/:key                     │ GET    │ Resolve single key         │
│ /sync                             │ POST   │ Sync local ↔ cloud        │
│ /sync/status                      │ GET    │ Get sync status            │
│ /search                           │ POST   │ Semantic search            │
│ /memory/episodes                  │ GET    │ List agent episodes        │
│ /memory/episodes                  │ POST   │ Record episode             │
│ /memory/knowledge                 │ GET    │ List knowledge entries     │
│ /memory/knowledge                 │ POST   │ Add knowledge entry        │
│ /memory/knowledge/:id/validate    │ POST   │ Validate knowledge         │
│ /admin/tenants/:id/quotas         │ GET    │ Get tenant quotas          │
│ /admin/tenants/:id/quotas         │ PUT    │ Update tenant quotas       │
│ /admin/tenants/:id/usage          │ GET    │ Get tenant usage           │
│ /admin/cache/invalidate           │ POST   │ Force cache invalidation   │
│ /admin/integrity/verify           │ POST   │ Verify hash chain          │
└────────────────────────────────────────────────────────────────────────┘
```

### 10.2 API Request/Response Examples

```typescript
// POST /api/v1/context/resolve
// Resolve full context for a session

// Request
interface ContextResolveRequest {
  userId: string;
  projectId?: string;
  sessionId: string;
  /** Specific keys to resolve (empty = all) */
  keys?: string[];
  /** Include resolution trace for debugging */
  includeTrace?: boolean;
}

// Response
interface ContextResolveResponse {
  tenantId: string;
  resolvedAt: string;
  entries: Record<string, {
    value: unknown;
    source: ContextScope;
    version: number;
    trace?: {
      checked: ContextScope[];
      found: ContextScope;
      duration_us: number;
    };
  }>;
  /** Aggregated resolution metadata */
  metadata: {
    totalKeys: number;
    cacheHits: number;
    cacheMisses: number;
    totalDuration_ms: number;
  };
}

// POST /api/v1/context/search
// Semantic search within project context

// Request
interface ContextSearchRequest {
  projectId: string;
  query: string;
  /** Filter by content type */
  types?: ('code' | 'documentation' | 'episode' | 'knowledge')[];
  /** Max results */
  limit?: number;
  /** Minimum similarity score (0.0 - 1.0) */
  minScore?: number;
  /** Filter by file patterns */
  filePatterns?: string[];
}

// Response
interface ContextSearchResponse {
  results: {
    id: string;
    type: string;
    content: string;
    filePath?: string;
    startLine?: number;
    endLine?: number;
    score: number;
    metadata: Record<string, unknown>;
  }[];
  totalResults: number;
  searchDuration_ms: number;
  embeddingModel: string;
}
```

### 10.3 Internal Service Interface (Domain Port)

```typescript
// domain/ports/context-repository.ts

interface IContextRepository {
  save(entry: ContextEntry): Promise<void>;
  findById(id: string): Promise<ContextEntry | null>;
  findByKey(tenantId: string, scope: ContextScope, key: string): Promise<ContextEntry | null>;
  findAllByScope(tenantId: string, scope: ContextScope): Promise<ContextEntry[]>;
  findByPattern(tenantId: string, keyPattern: string): Promise<ContextEntry[]>;
  softDelete(id: string): Promise<void>;
  getHistory(tenantId: string, scope: ContextScope, key: string, limit: number): Promise<ContextEntry[]>;
  countByTenant(tenantId: string): Promise<number>;
}

interface ISemanticIndexRepository {
  upsertChunks(projectId: string, chunks: SemanticChunk[]): Promise<void>;
  search(projectId: string, embedding: Float32Array, limit: number, minScore: number): Promise<SearchResult[]>;
  deleteByFile(projectId: string, filePath: string): Promise<number>;
  deleteByProject(projectId: string): Promise<number>;
  getIndexStats(projectId: string): Promise<IndexStats>;
}

interface IAgentMemoryRepository {
  saveEpisode(episode: Episode): Promise<void>;
  findEpisodes(agentId: string, projectId: string, options?: {
    type?: string;
    limit?: number;
    minImportance?: number;
  }): Promise<Episode[]>;
  searchEpisodes(agentId: string, projectId: string, embedding: Float32Array, limit: number): Promise<Episode[]>;
  saveKnowledge(entry: KnowledgeEntry): Promise<void>;
  findKnowledge(projectId: string, scope: string): Promise<KnowledgeEntry[]>;
  searchKnowledge(projectId: string, embedding: Float32Array, limit: number): Promise<KnowledgeEntry[]>;
  validateKnowledge(id: string): Promise<void>;
  saveProcedure(procedure: Procedure): Promise<void>;
  findProcedures(projectId: string, trigger?: string): Promise<Procedure[]>;
}
```

---

## 11. Diagrama de Componentes

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                          CONTEXT LAYER — COMPONENT DIAGRAM                  │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │                      PRESENTATION LAYER                              │   │
│  │  ┌─────────────┐  ┌──────────────┐  ┌────────────────┐              │   │
│  │  │ REPL/CLI    │  │ REST API     │  │ WebSocket      │              │   │
│  │  │ Commands    │  │ Controller   │  │ Gateway        │              │   │
│  │  └──────┬──────┘  └──────┬───────┘  └───────┬────────┘              │   │
│  └─────────┼────────────────┼──────────────────┼───────────────────────┘   │
│            │                │                  │                            │
│  ┌─────────┼────────────────┼──────────────────┼───────────────────────┐   │
│  │         ▼                ▼                  ▼                        │   │
│  │                      APPLICATION LAYER                              │   │
│  │  ┌─────────────────────────────────────────────────────────────┐    │   │
│  │  │                  Context Resolver                           │    │   │
│  │  │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐   │    │   │
│  │  │  │ Cascade  │  │ Merge    │  │ Validate │  │ Cache    │   │    │   │
│  │  │  │ Engine   │  │ Strategy │  │ Schema   │  │ Manager  │   │    │   │
│  │  │  └──────────┘  └──────────┘  └──────────┘  └──────────┘   │    │   │
│  │  └─────────────────────────────────────────────────────────────┘    │   │
│  │                                                                     │   │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐              │   │
│  │  │ Context      │  │ Agent Memory │  │ Semantic     │              │   │
│  │  │ Manager      │  │ Manager      │  │ Search       │              │   │
│  │  │              │  │              │  │ Service      │              │   │
│  │  │ • CRUD       │  │ • Episodes   │  │ • Index      │              │   │
│  │  │ • Versioning │  │ • Knowledge  │  │ • Search     │              │   │
│  │  │ • History    │  │ • Procedures │  │ • Chunking   │              │   │
│  │  │ • Integrity  │  │ • Retrieval  │  │ • Embed      │              │   │
│  │  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘              │   │
│  │         │                 │                  │                       │   │
│  │  ┌──────┴──────┐  ┌──────┴──────┐  ┌───────┴───────┐               │   │
│  │  │ Sync        │  │ Encryption  │  │ Quota         │               │   │
│  │  │ Service     │  │ Service     │  │ Enforcer      │               │   │
│  │  └──────┬──────┘  └──────┬──────┘  └───────┬───────┘               │   │
│  └─────────┼────────────────┼──────────────────┼──────────────────────┘   │
│            │                │                  │                           │
│  ┌─────────┼────────────────┼──────────────────┼──────────────────────┐   │
│  │         ▼                ▼                  ▼                       │   │
│  │                    INFRASTRUCTURE LAYER                             │   │
│  │                                                                     │   │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐              │   │
│  │  │ SQLite       │  │ PostgreSQL   │  │ Redis        │              │   │
│  │  │ Repository   │  │ Repository   │  │ Cache        │              │   │
│  │  │ (local)      │  │ (cloud)      │  │ (cloud)      │              │   │
│  │  └──────────────┘  └──────────────┘  └──────────────┘              │   │
│  │                                                                     │   │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐              │   │
│  │  │ SQLite Vec / │  │ Qdrant       │  │ Object Store │              │   │
│  │  │ Hnswlib      │  │ Client       │  │ (S3/GCS)     │              │   │
│  │  │ (local vec)  │  │ (cloud vec)  │  │ (archives)   │              │   │
│  │  └──────────────┘  └──────────────┘  └──────────────┘              │   │
│  │                                                                     │   │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐              │   │
│  │  │ OS Keychain  │  │ KMS Client   │  │ Event Bus    │              │   │
│  │  │ (secrets)    │  │ (cloud keys) │  │ (changes)    │              │   │
│  │  └──────────────┘  └──────────────┘  └──────────────┘              │   │
│  │                                                                     │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## 12. Decisiones Arquitectónicas

| # | Decisión | Alternativa Descartada | Justificación |
|---|----------|----------------------|---------------|
| ADR-C01 | **Append-only versioning con hash chain** | Mutable updates | Auditabilidad, integridad verificable, compliance SOC 2 |
| ADR-C02 | **RLS de PostgreSQL para aislamiento** | Schemas separados para todos | RLS es suficiente para Free/Team; schemas para Business; instancias dedicadas solo Enterprise. Costo-performance balance. |
| ADR-C03 | **LWW con vector clocks para sync** | CRDTs completos (Automerge) | Vector clocks son suficientes para nuestro modelo de datos. CRDTs añaden complejidad innecesaria para K-V context. |
| ADR-C04 | **SQLite como store local** | LevelDB, RocksDB | SQLite soporta extensiones vectoriales, es transaccional, y tiene el mejor tooling. Zero-config para CLI. |
| ADR-C05 | **Cascade resolution con merge strategies** | Flat namespace | La jerarquía refleja la realidad organizacional. Las policies de org DEBEN propagarse. Merge strategies permiten semántica precisa por tipo de config. |
| ADR-C06 | **Lazy materialization de embeddings** | Eager indexing | Indexar todo proactivamente tiene costo de CPU/memoria prohibitivo para CLIs. Lazy + background indexing es el balance correcto. |
| ADR-C07 | **Three-tier cache** | Single Redis cache | CLI necesita funcionar offline (L1+L2 local). Redis solo añade valor en modo cloud para estado compartido. |

---

## 13. Riesgos y Mitigaciones

| Riesgo | Probabilidad | Impacto | Mitigación |
|--------|-------------|---------|-----------|
| Hash chain corruption | Baja | Alto | Verificación periódica, backup antes de compact, rebuild desde audit log |
| Cache inconsistency entre L1/L2/L3 | Media | Medio | TTLs conservadores, invalidación explícita en write path, eventual consistency acceptable para reads |
| SQLite vec performance en repos grandes (>100K files) | Media | Alto | Chunking inteligente (solo archivos relevantes), indexación incremental, threshold de coverage configurable |
| Sync conflicts en equipos grandes | Media | Medio | Org policies siempre cloud-wins, user prefs LWW, conflictos visibles en CLI |
| Memory bloat por agent episodes | Baja | Medio | Importance-based consolidation, TTL en episodes, max episodes per agent configurable |
| Encryption key rotation complejidad | Baja | Alto | Key versioning, re-encrypt on read (lazy migration), key management via OS keychain o cloud KMS |

---

## 14. Plan de Implementación (Context Layer)

| Sprint | Entregable | Dependencias |
|--------|-----------|-------------|
| S1-S2 | Core context model + SQLite repository + basic CRUD | SQLite setup, domain entities |
| S3 | Context versioning + hash chain | Core context model |
| S4 | Cascade resolver + merge strategies | Core context model |
| S5-S6 | L1+L2 cache + invalidation | Resolver |
| S7-S8 | Agent memory (episodes + knowledge) | Core model |
| S9-S10 | Semantic indexing (Tree-sitter + embeddings) | SQLite vec / Hnswlib |
| S11-S12 | Offline sync protocol | Core model, versioning |
| S13-S14 | PostgreSQL repository + RLS | Cloud infrastructure |
| S15-S16 | Redis cache (L3) + Qdrant integration | Cloud infrastructure |
| S17-S18 | Encryption at rest + KMS integration | Security infrastructure |
| S19-S20 | Admin APIs + quota enforcement | All above |
