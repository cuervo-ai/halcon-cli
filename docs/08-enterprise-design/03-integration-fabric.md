# FASE 3 — Conectores e Integraciones (Integration Fabric)

> **Versión**: 1.0.0 | **Fecha**: 2026-02-06
> **Autores**: Platform Engineering Team
> **Estado**: Design Complete — Ready for Implementation Review

---

## 1. Visión General

El Integration Fabric de Cuervo es una capa de conectividad empresarial que abstrae la complejidad de integrar con docenas de servicios externos (GitHub, Jira, Slack, CI/CD, clouds, model providers) detrás de un framework uniforme de conectores. No es un simple wrapper de APIs: es un **sistema completo de lifecycle management** con autenticación delegada, sandboxing, rate limiting por conector, observabilidad granular, y un modelo de permisos que garantiza que cada conector opera con el mínimo privilegio necesario.

### 1.1 Principios de Diseño

| Principio | Justificación |
|-----------|---------------|
| **Adapter pattern** | Cada servicio externo se abstrae tras una interfaz uniforme; el dominio no conoce implementaciones |
| **Credential isolation** | Las credenciales de cada conector se almacenan, rotan y revocan independientemente |
| **Fail-open vs fail-closed** | Conectores no-críticos fallan silenciosamente; conectores de seguridad (IdP) fallan ruidosamente |
| **Backpressure** | Rate limiting propio + respeto de rate limits del proveedor; circuit breaker por conector |
| **Auditabilidad** | Toda interacción con servicio externo se loguea con request/response metadata |
| **Hot-swappable** | Conectores pueden actualizarse, desactivarse o reemplazarse sin downtime |

---

## 2. Arquitectura del Integration Hub

### 2.1 Diagrama de Componentes

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                        INTEGRATION FABRIC ARCHITECTURE                       │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  ┌────────────────────────────────────────────────────────────────────────┐  │
│  │                         CONNECTOR GATEWAY                              │  │
│  │  ┌──────────┐  ┌───────────┐  ┌──────────┐  ┌───────────────────┐    │  │
│  │  │ Router   │  │ Auth      │  │ Rate     │  │ Circuit Breaker   │    │  │
│  │  │          │  │ Injector  │  │ Limiter  │  │ (per connector)   │    │  │
│  │  └──────────┘  └───────────┘  └──────────┘  └───────────────────┘    │  │
│  │  ┌──────────┐  ┌───────────┐  ┌──────────┐  ┌───────────────────┐    │  │
│  │  │ Request  │  │ Response  │  │ Retry    │  │ Observability     │    │  │
│  │  │ Transform│  │ Transform │  │ Engine   │  │ (metrics, traces) │    │  │
│  │  └──────────┘  └───────────┘  └──────────┘  └───────────────────┘    │  │
│  └──────────────────────────┬─────────────────────────────────────────────┘  │
│                             │                                                │
│  ┌──────────────────────────┼─────────────────────────────────────────────┐  │
│  │                          ▼                                              │  │
│  │                    CONNECTOR REGISTRY                                   │  │
│  │  ┌──────────────────────────────────────────────────────────────────┐  │  │
│  │  │  Installed Connectors (per tenant)                               │  │  │
│  │  │                                                                  │  │  │
│  │  │  ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐  │  │  │
│  │  │  │ GitHub  │ │ GitLab  │ │ Jira    │ │ Slack   │ │ AWS     │  │  │  │
│  │  │  │ v2.3.0  │ │ v1.8.0  │ │ v2.1.0  │ │ v1.5.0  │ │ v3.0.0  │  │  │  │
│  │  │  └─────────┘ └─────────┘ └─────────┘ └─────────┘ └─────────┘  │  │  │
│  │  │                                                                  │  │  │
│  │  │  ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐  │  │  │
│  │  │  │ Linear  │ │ Teams   │ │ Jenkins │ │ GH      │ │ Anthropic│  │  │  │
│  │  │  │ v1.2.0  │ │ v1.0.0  │ │ v2.0.0  │ │ Actions │ │ v4.0.0  │  │  │  │
│  │  │  └─────────┘ └─────────┘ └─────────┘ │ v1.3.0  │ └─────────┘  │  │  │
│  │  │                                       └─────────┘              │  │  │
│  │  └──────────────────────────────────────────────────────────────────┘  │  │
│  └────────────────────────────────────────────────────────────────────────┘  │
│                                                                              │
│  ┌────────────────────────────────────────────────────────────────────────┐  │
│  │                       CREDENTIAL VAULT                                 │  │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────────────────┐  │  │
│  │  │ OAuth2   │  │ API Key  │  │ Service  │  │ Certificate          │  │  │
│  │  │ Tokens   │  │ Store    │  │ Accounts │  │ Store                │  │  │
│  │  │ (per     │  │ (per     │  │ (per     │  │ (mTLS, webhooks)     │  │  │
│  │  │  tenant) │  │  tenant) │  │  tenant) │  │                      │  │  │
│  │  └──────────┘  └──────────┘  └──────────┘  └──────────────────────┘  │  │
│  └────────────────────────────────────────────────────────────────────────┘  │
│                                                                              │
│  ┌────────────────────────────────────────────────────────────────────────┐  │
│  │                       EVENT BUS                                        │  │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────────────────┐  │  │
│  │  │ Webhook  │  │ Event    │  │ Pub/Sub  │  │ Dead Letter Queue    │  │  │
│  │  │ Receiver │  │ Router   │  │ Dispatch │  │ (failed deliveries)  │  │  │
│  │  └──────────┘  └──────────┘  └──────────┘  └──────────────────────┘  │  │
│  └────────────────────────────────────────────────────────────────────────┘  │
│                                                                              │
└──────────────────────────────────────────────────────────────────────────────┘
```

### 2.2 Patrón Arquitectónico

El Integration Fabric combina tres patrones:

1. **Gateway Pattern**: Punto de entrada único para todas las integraciones, con cross-cutting concerns (auth, rate limiting, retry, observability).

2. **Adapter Pattern**: Cada conector implementa una interfaz base (`IConnector`) y adapta el API externo a contratos internos uniformes.

3. **Event-Driven Pattern**: Los conectores emiten y consumen eventos a través del Event Bus, permitiendo workflows reactivos (e.g., "cuando se crea un PR en GitHub, ejecutar review automático").

---

## 3. Connector Framework

### 3.1 Interfaz Base del Conector

```typescript
// domain/ports/connector.ts

/**
 * Base interface that ALL connectors must implement.
 * This is the contract between the Integration Fabric and individual connectors.
 */
interface IConnector {
  /** Unique connector identifier (e.g., 'github', 'jira', 'slack') */
  readonly id: string;

  /** Semantic version */
  readonly version: string;

  /** Human-readable metadata */
  readonly metadata: ConnectorMetadata;

  /** Capabilities this connector provides */
  readonly capabilities: ConnectorCapability[];

  /** Required permissions/scopes */
  readonly requiredPermissions: ConnectorPermission[];

  /** Configuration schema (JSON Schema) */
  readonly configSchema: JSONSchema;

  // ─── Lifecycle Methods ───────────────────────────────────

  /**
   * Initialize the connector with tenant-specific configuration.
   * Called once when the connector is installed for a tenant.
   */
  initialize(config: ConnectorConfig): Promise<void>;

  /**
   * Perform OAuth2/credential exchange and store tokens.
   * Returns the authorization URL for OAuth2 flow, or void for API key auth.
   */
  authenticate(params: AuthParams): Promise<AuthResult>;

  /**
   * Verify that the connector can reach the external service.
   * Called periodically and on-demand.
   */
  healthCheck(): Promise<HealthCheckResult>;

  /**
   * Gracefully shutdown the connector.
   * Release resources, close connections, flush buffers.
   */
  shutdown(): Promise<void>;

  // ─── Credential Management ──────────────────────────────

  /**
   * Refresh credentials if expired.
   * Called automatically by the gateway before requests.
   */
  refreshCredentials(): Promise<void>;

  /**
   * Revoke all credentials and clean up.
   * Called when uninstalling the connector.
   */
  revokeCredentials(): Promise<void>;

  /**
   * Check if current credentials are valid.
   */
  isAuthenticated(): Promise<boolean>;

  // ─── Execution ──────────────────────────────────────────

  /**
   * Execute a connector action.
   * This is the main entry point for all connector operations.
   */
  execute(action: ConnectorAction): Promise<ConnectorResult>;

  /**
   * Stream results for long-running operations.
   */
  stream?(action: ConnectorAction): AsyncIterable<ConnectorEvent>;

  // ─── Sync ───────────────────────────────────────────────

  /**
   * Perform initial or incremental data sync.
   * E.g., sync repo metadata, issues, team members.
   */
  sync?(options: SyncOptions): Promise<SyncResult>;

  /**
   * Register webhook endpoints for real-time updates.
   */
  registerWebhooks?(webhooks: WebhookRegistration[]): Promise<void>;

  /**
   * Handle incoming webhook payload.
   */
  handleWebhook?(payload: WebhookPayload): Promise<WebhookHandleResult>;
}

interface ConnectorMetadata {
  name: string;
  description: string;
  icon: string;                    // URL or data URI
  category: ConnectorCategory;
  vendor: string;
  documentationUrl: string;
  supportUrl: string;
  privacyPolicyUrl: string;
  tags: string[];
  /** Minimum Cuervo version required */
  minPlatformVersion: string;
  /** External service API version targeted */
  targetApiVersion: string;
}

type ConnectorCategory =
  | 'source_control'    // GitHub, GitLab, Bitbucket
  | 'project_management' // Jira, Linear, Asana
  | 'communication'     // Slack, Teams, Discord
  | 'ci_cd'             // GitHub Actions, Jenkins, GitLab CI
  | 'cloud_provider'    // AWS, GCP, Azure
  | 'model_provider'    // Anthropic, OpenAI, Google AI
  | 'database'          // PostgreSQL, MySQL, MongoDB
  | 'monitoring'        // Datadog, Sentry, PagerDuty
  | 'security'          // Vault, 1Password, Snyk
  | 'custom';           // User-defined

interface ConnectorCapability {
  id: string;              // e.g., 'repo.read', 'pr.create', 'issue.list'
  name: string;
  description: string;
  requiredScopes: string[]; // External service scopes needed
}

interface ConnectorPermission {
  scope: string;           // Cuervo-side permission scope
  description: string;
  required: boolean;       // vs optional
  justification: string;   // Why this permission is needed
}
```

### 3.2 Connector Configuration

```typescript
// domain/entities/connector-config.ts

interface ConnectorConfig {
  /** Tenant that owns this connector instance */
  tenantId: string;

  /** Connector ID (e.g., 'github') */
  connectorId: string;

  /** Instance ID (a tenant can have multiple GitHub instances) */
  instanceId: string;

  /** Display name for this instance */
  displayName: string;

  /** Connector-specific settings (validated against configSchema) */
  settings: Record<string, unknown>;

  /** Authentication configuration */
  auth: ConnectorAuthConfig;

  /** Rate limiting (overrides connector defaults) */
  rateLimits: {
    requestsPerMinute: number;
    requestsPerHour: number;
    concurrentRequests: number;
  };

  /** Feature flags for this instance */
  features: {
    syncEnabled: boolean;
    webhooksEnabled: boolean;
    streamingEnabled: boolean;
  };

  /** Status */
  status: 'active' | 'paused' | 'error' | 'authenticating' | 'uninstalled';

  /** Lifecycle timestamps */
  installedAt: string;
  installedBy: string;
  lastSyncAt: string | null;
  lastHealthCheckAt: string | null;
  lastHealthStatus: 'healthy' | 'degraded' | 'unhealthy' | null;
  lastErrorAt: string | null;
  lastError: string | null;
}

type ConnectorAuthConfig =
  | { type: 'oauth2'; clientId: string; tokenEndpoint: string; scopes: string[] }
  | { type: 'api_key'; headerName: string }
  | { type: 'bearer_token' }
  | { type: 'basic_auth' }
  | { type: 'mtls'; certId: string }
  | { type: 'service_account'; credentialRef: string }
  | { type: 'none' };
```

### 3.3 Connector Actions & Results

```typescript
// domain/entities/connector-action.ts

/**
 * Unified action interface for all connector operations.
 * Each connector defines its own action types.
 */
interface ConnectorAction {
  /** Action identifier (e.g., 'repo.list', 'pr.create', 'issue.update') */
  action: string;

  /** Action parameters (connector-specific) */
  params: Record<string, unknown>;

  /** Request context */
  context: {
    tenantId: string;
    userId: string;
    sessionId: string;
    /** Timeout for this specific action */
    timeoutMs: number;
    /** Idempotency key for safe retries */
    idempotencyKey?: string;
    /** Trace ID for distributed tracing */
    traceId: string;
  };
}

interface ConnectorResult {
  /** Success or failure */
  success: boolean;

  /** Result data (connector-specific) */
  data: unknown;

  /** Error details (if !success) */
  error?: {
    code: string;
    message: string;
    retryable: boolean;
    retryAfterMs?: number;
  };

  /** Execution metadata */
  metadata: {
    durationMs: number;
    /** External API calls made */
    apiCalls: number;
    /** Tokens consumed (for model providers) */
    tokensUsed?: number;
    /** Rate limit headers from external service */
    rateLimitRemaining?: number;
    rateLimitResetAt?: string;
    /** Whether result was served from cache */
    cached: boolean;
  };
}

/**
 * Events emitted during streaming operations.
 */
type ConnectorEvent =
  | { type: 'progress'; message: string; percent: number }
  | { type: 'data'; chunk: unknown }
  | { type: 'warning'; message: string }
  | { type: 'complete'; summary: unknown }
  | { type: 'error'; error: { code: string; message: string } };
```

---

## 4. Connector Implementations

### 4.1 Source Control Connectors

```typescript
// infrastructure/connectors/source-control/github-connector.ts

/**
 * GitHub connector implementing IConnector + source control capabilities.
 */
interface IGitHubConnector extends IConnector {
  readonly id: 'github';
  readonly capabilities: [
    { id: 'repo.list' },
    { id: 'repo.read' },
    { id: 'repo.clone' },
    { id: 'pr.list' },
    { id: 'pr.create' },
    { id: 'pr.review' },
    { id: 'pr.merge' },
    { id: 'issue.list' },
    { id: 'issue.create' },
    { id: 'issue.update' },
    { id: 'actions.list' },
    { id: 'actions.trigger' },
    { id: 'actions.status' },
    { id: 'search.code' },
    { id: 'search.issues' },
    { id: 'webhook.receive' },
  ];
}

/**
 * Unified source control interface that all SCM connectors implement.
 * This allows swapping GitHub for GitLab without changing domain logic.
 */
interface ISourceControlAdapter {
  // Repository operations
  listRepositories(opts: { page: number; perPage: number; search?: string }): Promise<Repository[]>;
  getRepository(owner: string, repo: string): Promise<Repository>;
  getFileContent(owner: string, repo: string, path: string, ref?: string): Promise<FileContent>;
  getDirectoryTree(owner: string, repo: string, ref?: string): Promise<TreeNode[]>;

  // Pull Request / Merge Request operations
  listPullRequests(owner: string, repo: string, opts: PRListOptions): Promise<PullRequest[]>;
  createPullRequest(owner: string, repo: string, pr: CreatePRInput): Promise<PullRequest>;
  getPullRequest(owner: string, repo: string, number: number): Promise<PullRequestDetail>;
  addReviewComment(owner: string, repo: string, prNumber: number, comment: ReviewComment): Promise<void>;
  mergePullRequest(owner: string, repo: string, prNumber: number, opts: MergeOptions): Promise<void>;

  // Issue operations
  listIssues(owner: string, repo: string, opts: IssueListOptions): Promise<Issue[]>;
  createIssue(owner: string, repo: string, issue: CreateIssueInput): Promise<Issue>;
  updateIssue(owner: string, repo: string, number: number, update: UpdateIssueInput): Promise<Issue>;

  // CI/CD operations
  listWorkflowRuns(owner: string, repo: string, opts?: WorkflowRunOptions): Promise<WorkflowRun[]>;
  triggerWorkflow(owner: string, repo: string, workflowId: string, inputs: Record<string, string>): Promise<WorkflowRun>;
  getWorkflowRunStatus(owner: string, repo: string, runId: number): Promise<WorkflowRunDetail>;

  // Search
  searchCode(query: string, opts?: SearchOptions): Promise<CodeSearchResult[]>;
  searchIssues(query: string, opts?: SearchOptions): Promise<Issue[]>;
}

// Shared domain types (connector-agnostic)
interface Repository {
  id: string;
  name: string;
  fullName: string;
  description: string | null;
  url: string;
  cloneUrl: string;
  defaultBranch: string;
  isPrivate: boolean;
  language: string | null;
  updatedAt: string;
}

interface PullRequest {
  id: string;
  number: number;
  title: string;
  body: string;
  state: 'open' | 'closed' | 'merged';
  author: string;
  sourceBranch: string;
  targetBranch: string;
  createdAt: string;
  updatedAt: string;
  mergedAt: string | null;
  reviewStatus: 'pending' | 'approved' | 'changes_requested';
  labels: string[];
  url: string;
}

interface Issue {
  id: string;
  number: number;
  title: string;
  body: string;
  state: 'open' | 'closed';
  author: string;
  assignees: string[];
  labels: string[];
  createdAt: string;
  updatedAt: string;
  url: string;
}
```

### 4.2 Project Management Connectors

```typescript
// infrastructure/connectors/project-management/adapter.ts

interface IProjectManagementAdapter {
  // Projects
  listProjects(opts?: PaginationOptions): Promise<Project[]>;
  getProject(projectId: string): Promise<ProjectDetail>;

  // Issues / Tasks / Tickets
  listTasks(projectId: string, opts?: TaskListOptions): Promise<Task[]>;
  createTask(projectId: string, task: CreateTaskInput): Promise<Task>;
  updateTask(taskId: string, update: UpdateTaskInput): Promise<Task>;
  getTask(taskId: string): Promise<TaskDetail>;

  // Status transitions
  transitionTask(taskId: string, targetStatus: string): Promise<Task>;

  // Comments
  addComment(taskId: string, body: string): Promise<Comment>;
  listComments(taskId: string): Promise<Comment[]>;

  // Labels / Tags
  listLabels(projectId: string): Promise<Label[]>;
  addLabel(taskId: string, label: string): Promise<void>;

  // Search
  searchTasks(query: string, opts?: SearchOptions): Promise<Task[]>;
}

interface Task {
  id: string;
  key: string;              // e.g., 'PROJ-123' (Jira) or 'LIN-456' (Linear)
  title: string;
  description: string;
  status: string;
  priority: 'urgent' | 'high' | 'medium' | 'low' | 'none';
  assignee: string | null;
  reporter: string;
  labels: string[];
  createdAt: string;
  updatedAt: string;
  dueDate: string | null;
  estimatePoints: number | null;
  url: string;
  /** Connector-specific metadata */
  raw: Record<string, unknown>;
}
```

### 4.3 Communication Connectors

```typescript
// infrastructure/connectors/communication/adapter.ts

interface ICommunicationAdapter {
  // Channels
  listChannels(opts?: PaginationOptions): Promise<Channel[]>;
  getChannel(channelId: string): Promise<ChannelDetail>;

  // Messages
  sendMessage(channelId: string, message: MessageInput): Promise<Message>;
  updateMessage(channelId: string, messageId: string, content: string): Promise<Message>;
  deleteMessage(channelId: string, messageId: string): Promise<void>;

  // Threads
  replyInThread(channelId: string, threadId: string, message: MessageInput): Promise<Message>;

  // Notifications
  sendDirectMessage(userId: string, message: MessageInput): Promise<Message>;

  // Rich content
  sendRichMessage(channelId: string, blocks: MessageBlock[]): Promise<Message>;
}

interface MessageInput {
  text: string;
  /** Markdown formatting */
  markdown?: boolean;
  /** Code blocks */
  codeBlocks?: { language: string; code: string }[];
  /** Attachments */
  attachments?: { name: string; url: string; mimeType: string }[];
  /** Thread reference */
  threadId?: string;
}

interface MessageBlock {
  type: 'section' | 'divider' | 'code' | 'actions' | 'context';
  content?: string;
  fields?: { label: string; value: string }[];
  actions?: { label: string; actionId: string; style?: 'primary' | 'danger' }[];
}
```

### 4.4 Model Provider Connectors

```typescript
// infrastructure/connectors/model-providers/adapter.ts

/**
 * Model provider adapter — integrates with the existing Model Gateway.
 * This connector type has specialized handling for streaming, token counting,
 * and cost tracking.
 */
interface IModelProviderAdapter {
  readonly providerId: string;
  readonly supportedModels: ModelDefinition[];

  // Model operations
  listModels(): Promise<ModelDefinition[]>;
  getModelCapabilities(modelId: string): Promise<ModelCapabilities>;

  // Inference
  chatCompletion(request: ChatCompletionRequest): Promise<ChatCompletionResponse>;
  streamChatCompletion(request: ChatCompletionRequest): AsyncIterable<ChatCompletionChunk>;

  // Embeddings
  createEmbedding(request: EmbeddingRequest): Promise<EmbeddingResponse>;

  // Token counting
  countTokens(content: string, model: string): Promise<number>;

  // Health
  checkAvailability(): Promise<{ available: boolean; latencyMs: number }>;
}

interface ModelDefinition {
  id: string;
  name: string;
  provider: string;
  capabilities: ('chat' | 'completion' | 'embedding' | 'vision' | 'function_calling' | 'thinking')[];
  contextWindow: number;
  maxOutputTokens: number;
  pricing: {
    inputPer1kTokens: number;
    outputPer1kTokens: number;
    currency: 'USD';
  };
  supportedFeatures: {
    streaming: boolean;
    functionCalling: boolean;
    vision: boolean;
    thinking: boolean;
    systemPrompt: boolean;
  };
}
```

---

## 5. Connector Lifecycle

### 5.1 State Machine

```
┌─────────────────────────────────────────────────────────────────┐
│                  CONNECTOR LIFECYCLE STATE MACHINE                │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌──────────┐    install     ┌──────────────┐                    │
│  │ Available│──────────────> │ Installed     │                    │
│  │ (in      │                │ (config set,  │                    │
│  │ registry)│                │ not auth'd)   │                    │
│  └──────────┘                └──────┬────────┘                    │
│                                     │ authenticate                │
│                                     ▼                             │
│                              ┌──────────────┐                    │
│                              │Authenticating │                    │
│                              │ (OAuth flow   │                    │
│                              │  in progress) │                    │
│                              └──────┬────────┘                    │
│                           success   │   failure                   │
│                         ┌───────────┴──────────┐                  │
│                         ▼                      ▼                  │
│                  ┌──────────────┐        ┌──────────────┐         │
│                  │ Active       │        │ Auth Error   │         │
│                  │ (operational)│        │ (retry)      │         │
│                  └──────┬───────┘        └──────────────┘         │
│                         │                                         │
│              ┌──────────┼──────────┐                              │
│              ▼          ▼          ▼                               │
│       ┌──────────┐ ┌────────┐ ┌──────────┐                       │
│       │ Paused   │ │ Error  │ │ Degraded │                       │
│       │ (manual) │ │ (auto) │ │ (partial)│                       │
│       └──────────┘ └────────┘ └──────────┘                       │
│              │          │          │                               │
│              └──────────┴──────────┘                              │
│                         │ uninstall                               │
│                         ▼                                         │
│                  ┌──────────────┐                                 │
│                  │ Uninstalling │                                 │
│                  │ (revoke creds│                                 │
│                  │  cleanup)    │                                 │
│                  └──────┬───────┘                                 │
│                         │                                         │
│                         ▼                                         │
│                  ┌──────────────┐                                 │
│                  │ Uninstalled  │                                 │
│                  │ (removed)    │                                 │
│                  └──────────────┘                                 │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### 5.2 Lifecycle API

```typescript
// application/services/connector-lifecycle.ts

interface ConnectorLifecycleService {
  /**
   * Install a connector for a tenant.
   * Validates config against schema, stores configuration, initializes connector.
   */
  install(
    tenantId: string,
    connectorId: string,
    config: Record<string, unknown>,
    installedBy: string,
  ): Promise<ConnectorInstance>;

  /**
   * Initiate authentication flow for a connector.
   * For OAuth2: returns authorization URL.
   * For API key: validates and stores the key.
   */
  authenticate(
    tenantId: string,
    instanceId: string,
    credentials: ConnectorCredentials,
  ): Promise<AuthResult>;

  /**
   * Handle OAuth2 callback.
   */
  handleOAuthCallback(
    tenantId: string,
    instanceId: string,
    code: string,
    state: string,
  ): Promise<void>;

  /**
   * Pause a connector (stop processing, keep credentials).
   */
  pause(tenantId: string, instanceId: string, reason: string): Promise<void>;

  /**
   * Resume a paused connector.
   */
  resume(tenantId: string, instanceId: string): Promise<void>;

  /**
   * Uninstall a connector.
   * Revokes credentials, removes webhooks, deletes synced data.
   */
  uninstall(
    tenantId: string,
    instanceId: string,
    options: { deleteData: boolean; revokeTokens: boolean },
  ): Promise<void>;

  /**
   * Upgrade a connector to a new version.
   * Performs migration if needed.
   */
  upgrade(
    tenantId: string,
    instanceId: string,
    targetVersion: string,
  ): Promise<void>;

  /**
   * Get connector health for a tenant.
   */
  healthCheck(tenantId: string, instanceId: string): Promise<HealthCheckResult>;

  /**
   * List all installed connectors for a tenant.
   */
  listInstalled(tenantId: string): Promise<ConnectorInstance[]>;
}

type ConnectorCredentials =
  | { type: 'oauth2'; authorizationCode: string; state: string }
  | { type: 'api_key'; key: string }
  | { type: 'bearer_token'; token: string }
  | { type: 'basic_auth'; username: string; password: string }
  | { type: 'service_account'; credentialJson: string };

interface ConnectorInstance {
  instanceId: string;
  connectorId: string;
  tenantId: string;
  displayName: string;
  version: string;
  status: ConnectorStatus;
  config: Record<string, unknown>;
  capabilities: string[];
  installedAt: string;
  installedBy: string;
  lastActivityAt: string | null;
  health: HealthCheckResult | null;
  usage: {
    totalRequests: number;
    totalErrors: number;
    averageLatencyMs: number;
    last24hRequests: number;
  };
}

type ConnectorStatus =
  | 'installed'
  | 'authenticating'
  | 'active'
  | 'paused'
  | 'error'
  | 'degraded'
  | 'uninstalling'
  | 'uninstalled';

interface HealthCheckResult {
  status: 'healthy' | 'degraded' | 'unhealthy';
  checkedAt: string;
  latencyMs: number;
  details: {
    authValid: boolean;
    serviceReachable: boolean;
    rateLimitOk: boolean;
    lastError: string | null;
  };
}
```

---

## 6. Connector Gateway (Cross-Cutting Concerns)

### 6.1 Request Pipeline

```
┌─────────────────────────────────────────────────────────────────────┐
│                   CONNECTOR REQUEST PIPELINE                         │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Incoming Request                                                    │
│       │                                                              │
│       ▼                                                              │
│  ┌──────────────────┐                                                │
│  │ 1. Authenticate  │  Verify caller's JWT/API key                   │
│  │    Request       │  Check connector access permission             │
│  └────────┬─────────┘                                                │
│           │                                                          │
│           ▼                                                          │
│  ┌──────────────────┐                                                │
│  │ 2. Check Circuit │  If OPEN → return cached/error immediately     │
│  │    Breaker       │  If HALF_OPEN → allow probe request            │
│  └────────┬─────────┘                                                │
│           │                                                          │
│           ▼                                                          │
│  ┌──────────────────┐                                                │
│  │ 3. Rate Limit    │  Token bucket (per tenant + per connector)     │
│  │    Check         │  If exceeded → 429 with Retry-After            │
│  └────────┬─────────┘                                                │
│           │                                                          │
│           ▼                                                          │
│  ┌──────────────────┐                                                │
│  │ 4. Cache Check   │  Check if response is cached (GET-like ops)    │
│  │    (Optional)    │  If hit → return cached response               │
│  └────────┬─────────┘                                                │
│           │                                                          │
│           ▼                                                          │
│  ┌──────────────────┐                                                │
│  │ 5. Inject Auth   │  Attach connector credentials to request       │
│  │    Credentials   │  Auto-refresh if expired                       │
│  └────────┬─────────┘                                                │
│           │                                                          │
│           ▼                                                          │
│  ┌──────────────────┐                                                │
│  │ 6. Transform     │  Map internal action to external API call      │
│  │    Request       │  Apply connector-specific transformations      │
│  └────────┬─────────┘                                                │
│           │                                                          │
│           ▼                                                          │
│  ┌──────────────────┐                                                │
│  │ 7. Execute       │  HTTP call to external service                 │
│  │    (with retry)  │  Exponential backoff: 1s, 2s, 4s (max 3)      │
│  └────────┬─────────┘                                                │
│           │                                                          │
│           ▼                                                          │
│  ┌──────────────────┐                                                │
│  │ 8. Transform     │  Map external response to internal format      │
│  │    Response      │  Normalize errors                              │
│  └────────┬─────────┘                                                │
│           │                                                          │
│           ▼                                                          │
│  ┌──────────────────┐                                                │
│  │ 9. Update        │  Record success/failure for circuit breaker    │
│  │    Circuit State │  Update rate limit counters                    │
│  └────────┬─────────┘                                                │
│           │                                                          │
│           ▼                                                          │
│  ┌──────────────────┐                                                │
│  │ 10. Emit Metrics │  Duration, status, connector, action           │
│  │    & Audit Log   │  Full audit trail for compliance               │
│  └────────┬─────────┘                                                │
│           │                                                          │
│           ▼                                                          │
│  Response to Caller                                                  │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### 6.2 Rate Limiting

```typescript
// infrastructure/gateway/rate-limiter.ts

interface ConnectorRateLimiter {
  /**
   * Check and consume rate limit tokens.
   * Uses token bucket algorithm with per-tenant + per-connector granularity.
   */
  check(params: {
    tenantId: string;
    connectorId: string;
    instanceId: string;
    /** Weight of this request (most = 1, search = 5, bulk = 10) */
    weight: number;
  }): Promise<RateLimitResult>;

  /**
   * Report external rate limit headers from the service.
   * Adjusts internal limits to avoid hitting external limits.
   */
  reportExternalLimits(params: {
    connectorId: string;
    instanceId: string;
    remaining: number;
    resetAt: string;
    limit: number;
  }): void;
}

interface RateLimitResult {
  allowed: boolean;
  remaining: number;
  resetAt: string;
  retryAfterMs: number | null;
  /** Which limit was hit */
  limitType: 'tenant' | 'connector' | 'external' | null;
}
```

### 6.3 Circuit Breaker

```typescript
// infrastructure/gateway/circuit-breaker.ts

interface ConnectorCircuitBreaker {
  /** Per-connector-instance state machine */
  getState(instanceId: string): CircuitState;

  /** Record successful request */
  recordSuccess(instanceId: string): void;

  /** Record failed request */
  recordFailure(instanceId: string, error: Error): void;

  /** Force open (e.g., manual intervention) */
  forceOpen(instanceId: string, reason: string, durationMs: number): void;

  /** Force close (e.g., after fixing issue) */
  forceClose(instanceId: string): void;
}

interface CircuitState {
  state: 'closed' | 'open' | 'half_open';
  failureCount: number;
  successCount: number;
  lastFailureAt: string | null;
  lastSuccessAt: string | null;
  openedAt: string | null;
  /** When the circuit will transition from OPEN to HALF_OPEN */
  nextRetryAt: string | null;
}

// Configuration per connector type
const CIRCUIT_BREAKER_DEFAULTS: Record<ConnectorCategory, CircuitBreakerConfig> = {
  source_control: {
    failureThreshold: 5,
    successThreshold: 3,     // Successes needed in HALF_OPEN to close
    timeout: 30_000,          // 30s before HALF_OPEN
    halfOpenMaxRequests: 3,
  },
  model_provider: {
    failureThreshold: 3,      // Model providers: faster trip
    successThreshold: 2,
    timeout: 15_000,
    halfOpenMaxRequests: 1,
  },
  communication: {
    failureThreshold: 10,     // Slack: more lenient
    successThreshold: 3,
    timeout: 60_000,
    halfOpenMaxRequests: 5,
  },
  // ... other categories
};
```

---

## 7. Event System & Webhooks

### 7.1 Event Bus Architecture

```typescript
// infrastructure/events/event-bus.ts

/**
 * Internal event bus for connector-generated events.
 * Supports both sync (in-process) and async (queue-backed) delivery.
 */
interface IEventBus {
  /**
   * Publish an event from a connector.
   */
  publish(event: IntegrationEvent): Promise<void>;

  /**
   * Subscribe to events by type pattern.
   * Returns unsubscribe function.
   */
  subscribe(
    pattern: string,   // e.g., 'github.*', 'jira.issue.created', '*'
    handler: EventHandler,
    options?: SubscribeOptions,
  ): Unsubscribe;

  /**
   * Replay events from history (for catch-up after outage).
   */
  replay(
    pattern: string,
    since: string,
    handler: EventHandler,
  ): Promise<number>;
}

interface IntegrationEvent {
  id: string;                      // UUID
  type: string;                    // 'github.pr.opened', 'jira.issue.created'
  source: string;                  // Connector instance ID
  tenantId: string;
  timestamp: string;
  /** Event payload (connector-specific) */
  data: Record<string, unknown>;
  /** Correlation ID for tracing */
  correlationId: string;
  /** Metadata */
  metadata: {
    connectorId: string;
    connectorVersion: string;
    deliveryAttempt: number;
    originalWebhookId?: string;
  };
}

type EventHandler = (event: IntegrationEvent) => Promise<void>;

interface SubscribeOptions {
  /** Process events in order (vs parallel) */
  ordered?: boolean;
  /** Max concurrent event processing */
  concurrency?: number;
  /** Dead-letter after N failures */
  maxRetries?: number;
  /** Filter by tenant */
  tenantId?: string;
}
```

### 7.2 Webhook Ingestion

```typescript
// infrastructure/webhooks/webhook-receiver.ts

/**
 * Receives webhooks from external services.
 * Each connector type registers its webhook handlers.
 */
interface WebhookReceiver {
  /**
   * Register a webhook endpoint for a connector instance.
   * Returns the webhook URL to configure in the external service.
   */
  register(
    instanceId: string,
    events: string[],
  ): Promise<WebhookEndpoint>;

  /**
   * Process an incoming webhook.
   * 1. Verify signature
   * 2. Identify connector instance
   * 3. Parse payload
   * 4. Publish to event bus
   */
  process(request: IncomingWebhook): Promise<WebhookProcessResult>;

  /**
   * Deregister webhook (called during uninstall).
   */
  deregister(instanceId: string): Promise<void>;
}

interface WebhookEndpoint {
  url: string;           // https://hooks.cuervo.dev/v1/webhooks/<instanceId>/<secret>
  secret: string;        // For HMAC signature verification
  events: string[];
  createdAt: string;
}

interface IncomingWebhook {
  /** Raw HTTP request data */
  headers: Record<string, string>;
  body: Buffer;
  method: string;
  path: string;
  /** Extracted from URL path */
  instanceId: string;
  webhookSecret: string;
}

interface WebhookProcessResult {
  accepted: boolean;
  eventId: string | null;
  eventType: string | null;
  error: string | null;
}
```

### 7.3 Outgoing Webhooks (User-Defined)

```typescript
// infrastructure/webhooks/webhook-dispatcher.ts

/**
 * Dispatches events to user-configured webhook endpoints.
 * Part of the extensibility layer (users subscribe to Cuervo events).
 */
interface WebhookDispatcher {
  /**
   * Register an outgoing webhook subscription.
   */
  createSubscription(sub: WebhookSubscription): Promise<void>;

  /**
   * Dispatch an event to all matching subscriptions.
   * Handles retries, failure recording, and circuit breaking.
   */
  dispatch(event: IntegrationEvent): Promise<DispatchResult>;

  /**
   * List recent deliveries for a subscription (for debugging).
   */
  getDeliveryHistory(subscriptionId: string, limit: number): Promise<WebhookDelivery[]>;
}

interface WebhookSubscription {
  id: string;
  tenantId: string;
  url: string;                     // Target endpoint
  events: string[];                // Event type patterns
  secret: string;                  // For HMAC signing outgoing requests
  active: boolean;
  headers: Record<string, string>; // Custom headers
  retryPolicy: {
    maxRetries: number;            // Default: 3
    backoffMs: number[];           // [1000, 5000, 30000]
  };
  createdBy: string;
  createdAt: string;
}

interface WebhookDelivery {
  id: string;
  subscriptionId: string;
  eventId: string;
  eventType: string;
  url: string;
  requestBody: string;
  responseStatus: number | null;
  responseBody: string | null;
  durationMs: number;
  attempt: number;
  success: boolean;
  error: string | null;
  deliveredAt: string;
}
```

---

## 8. Connector SDK (For Third-Party Development)

### 8.1 SDK Structure

```
@cuervo/connector-sdk/
├── src/
│   ├── index.ts                    # Public API exports
│   ├── base-connector.ts           # Abstract base class
│   ├── decorators.ts               # @Action, @Webhook, @Capability decorators
│   ├── types.ts                    # Shared types
│   ├── testing/
│   │   ├── mock-gateway.ts         # Mock gateway for testing
│   │   ├── mock-credential-store.ts
│   │   └── test-harness.ts         # Integration test helpers
│   ├── validation/
│   │   ├── schema-validator.ts     # JSON Schema validation
│   │   └── permission-checker.ts   # Permission validation
│   └── utils/
│       ├── http-client.ts          # Pre-configured HTTP client
│       ├── retry.ts                # Retry utilities
│       └── pagination.ts           # Pagination helpers
├── templates/
│   ├── basic-connector/            # Scaffold for new connectors
│   └── oauth2-connector/          # Scaffold with OAuth2 flow
├── package.json
├── tsconfig.json
└── README.md
```

### 8.2 Base Connector Class

```typescript
// @cuervo/connector-sdk/src/base-connector.ts

/**
 * Abstract base class that simplifies connector development.
 * Handles boilerplate: credential management, health checks, error handling.
 * Connector authors only implement domain-specific logic.
 */
abstract class BaseConnector implements IConnector {
  abstract readonly id: string;
  abstract readonly version: string;
  abstract readonly metadata: ConnectorMetadata;
  abstract readonly capabilities: ConnectorCapability[];
  abstract readonly requiredPermissions: ConnectorPermission[];
  abstract readonly configSchema: JSONSchema;

  protected config!: ConnectorConfig;
  protected credentials!: CredentialStore;
  protected httpClient!: ConnectorHttpClient;
  protected logger!: ConnectorLogger;

  // ─── Template methods (override in subclass) ───

  /** Called after initialization. Set up client, validate config. */
  protected abstract onInitialize(config: ConnectorConfig): Promise<void>;

  /** Called on shutdown. Clean up resources. */
  protected abstract onShutdown(): Promise<void>;

  /** Register action handlers. Called during initialization. */
  protected abstract registerActions(): ActionRegistry;

  /** Perform health check against external service. */
  protected abstract onHealthCheck(): Promise<HealthCheckResult>;

  // ─── Final methods (not overridable) ───

  async initialize(config: ConnectorConfig): Promise<void> {
    this.config = config;
    this.credentials = new CredentialStore(config.tenantId, config.instanceId);
    this.httpClient = new ConnectorHttpClient({
      baseUrl: this.getBaseUrl(),
      timeout: 30_000,
      retries: 3,
    });
    this.logger = new ConnectorLogger(this.id, config.instanceId);

    await this.onInitialize(config);
    this.logger.info('Connector initialized', { version: this.version });
  }

  async execute(action: ConnectorAction): Promise<ConnectorResult> {
    const handler = this.registerActions().get(action.action);
    if (!handler) {
      return {
        success: false,
        data: null,
        error: { code: 'UNKNOWN_ACTION', message: `Action ${action.action} not supported`, retryable: false },
        metadata: { durationMs: 0, apiCalls: 0, cached: false },
      };
    }

    const start = performance.now();
    try {
      const result = await handler(action.params, action.context);
      return {
        success: true,
        data: result,
        metadata: { durationMs: performance.now() - start, apiCalls: this.httpClient.callCount, cached: false },
      };
    } catch (error) {
      return this.handleError(error, start);
    }
  }

  // ... additional helper methods
}

/**
 * Decorator-based action registration (alternative to registerActions).
 */
function Action(actionId: string, options?: { cacheTtl?: number; weight?: number }) {
  return function (target: any, propertyKey: string, descriptor: PropertyDescriptor) {
    // Register method as action handler
    Reflect.defineMetadata(`action:${actionId}`, {
      handler: descriptor.value,
      ...options,
    }, target.constructor);
  };
}

// Example usage:
class GitHubConnector extends BaseConnector {
  readonly id = 'github';
  readonly version = '2.3.0';
  // ...

  @Action('repo.list', { cacheTtl: 300 })
  async listRepos(params: { page: number; perPage: number }) {
    const response = await this.httpClient.get('/user/repos', {
      params: { page: params.page, per_page: params.perPage },
    });
    return response.data.map(this.mapRepository);
  }

  @Action('pr.create')
  async createPR(params: CreatePRInput) {
    const response = await this.httpClient.post(
      `/repos/${params.owner}/${params.repo}/pulls`,
      { title: params.title, body: params.body, head: params.head, base: params.base },
    );
    return this.mapPullRequest(response.data);
  }
}
```

---

## 9. Security & Sandboxing

### 9.1 Connector Permission Model

```typescript
// domain/entities/connector-permissions.ts

/**
 * Each connector declares what it needs; the admin grants/denies.
 * Principle of least privilege: connectors get ONLY what they declare.
 */
interface ConnectorPermissionGrant {
  connectorId: string;
  instanceId: string;
  tenantId: string;

  /** Granted capabilities (subset of connector's declared capabilities) */
  grantedCapabilities: string[];

  /** Denied capabilities (explicitly blocked) */
  deniedCapabilities: string[];

  /** Data access scope */
  dataScope: {
    /** Which projects this connector can access */
    projects: string[] | 'all';
    /** Which data types it can read/write */
    dataTypes: ('code' | 'issues' | 'messages' | 'configs' | 'secrets')[];
    /** Whether it can access user PII */
    piiAccess: boolean;
  };

  /** Network scope (for custom connectors) */
  networkScope: {
    /** Allowed outbound domains */
    allowedDomains: string[];
    /** Blocked domains */
    blockedDomains: string[];
  };

  grantedBy: string;
  grantedAt: string;
  reviewedAt: string | null;
  expiresAt: string | null;
}
```

### 9.2 Third-Party Connector Sandboxing

```typescript
// infrastructure/sandbox/connector-sandbox.ts

/**
 * Third-party (non-first-party) connectors run in a sandboxed environment.
 * This limits their access to system resources and enforces security policies.
 */
interface ConnectorSandbox {
  /**
   * Execute a connector action within the sandbox.
   * The sandbox enforces:
   * - Network restrictions (only allowed domains)
   * - Memory limits (256MB default)
   * - CPU time limits (30s per action)
   * - No filesystem access (only via provided APIs)
   * - No access to other connectors' data
   * - No access to system secrets (only own credentials)
   */
  execute(
    connector: IConnector,
    action: ConnectorAction,
    constraints: SandboxConstraints,
  ): Promise<ConnectorResult>;
}

interface SandboxConstraints {
  maxMemoryMB: number;         // Default: 256
  maxCpuTimeMs: number;        // Default: 30000
  maxNetworkCalls: number;     // Default: 100 per action
  allowedDomains: string[];    // Outbound HTTP whitelist
  maxResponseSizeBytes: number; // Default: 10MB
  /** Whether to log all outbound requests (for audit) */
  logOutboundRequests: boolean;
}
```

---

## 10. Connector API

```
BASE: /api/v1/connectors

┌────────────────────────────────────────────────────────────────────────────┐
│ Endpoint                              │ Method │ Description               │
├────────────────────────────────────────────────────────────────────────────┤
│ /registry                             │ GET    │ List available connectors  │
│ /registry/:connectorId                │ GET    │ Get connector details      │
│ /instances                            │ GET    │ List installed instances   │
│ /instances                            │ POST   │ Install a connector        │
│ /instances/:instanceId                │ GET    │ Get instance details       │
│ /instances/:instanceId                │ PATCH  │ Update instance config     │
│ /instances/:instanceId                │ DELETE │ Uninstall connector        │
│ /instances/:instanceId/auth           │ POST   │ Start auth flow            │
│ /instances/:instanceId/auth/callback  │ GET    │ OAuth2 callback            │
│ /instances/:instanceId/execute        │ POST   │ Execute connector action   │
│ /instances/:instanceId/health         │ GET    │ Health check               │
│ /instances/:instanceId/sync           │ POST   │ Trigger sync               │
│ /instances/:instanceId/sync/status    │ GET    │ Get sync status            │
│ /instances/:instanceId/events         │ GET    │ List connector events      │
│ /instances/:instanceId/permissions    │ GET    │ Get permissions            │
│ /instances/:instanceId/permissions    │ PUT    │ Update permissions         │
│ /instances/:instanceId/metrics        │ GET    │ Get usage metrics          │
│ /instances/:instanceId/pause          │ POST   │ Pause connector            │
│ /instances/:instanceId/resume         │ POST   │ Resume connector           │
│ /webhooks                             │ GET    │ List webhook subscriptions │
│ /webhooks                             │ POST   │ Create webhook sub         │
│ /webhooks/:id                         │ DELETE │ Delete webhook sub         │
│ /webhooks/:id/deliveries              │ GET    │ Delivery history           │
│ /webhooks/:id/test                    │ POST   │ Send test webhook          │
└────────────────────────────────────────────────────────────────────────────┘
```

---

## 11. Connector Database Schema

```sql
-- Available connectors (platform-managed registry)
CREATE TABLE connector_registry (
    id TEXT PRIMARY KEY,            -- 'github', 'jira', etc.
    name TEXT NOT NULL,
    description TEXT,
    category TEXT NOT NULL,
    vendor TEXT NOT NULL,
    latest_version TEXT NOT NULL,
    icon_url TEXT,
    documentation_url TEXT,
    config_schema JSONB NOT NULL,
    capabilities JSONB NOT NULL,
    required_permissions JSONB NOT NULL,
    is_first_party BOOLEAN NOT NULL DEFAULT false,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Installed connector instances (per tenant)
CREATE TABLE connector_instances (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    connector_id TEXT NOT NULL REFERENCES connector_registry(id),
    display_name TEXT NOT NULL,
    version TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'installed',
    config JSONB NOT NULL DEFAULT '{}',
    auth_type TEXT NOT NULL,
    granted_capabilities JSONB NOT NULL DEFAULT '[]',
    denied_capabilities JSONB NOT NULL DEFAULT '[]',
    data_scope JSONB NOT NULL DEFAULT '{}',
    rate_limits JSONB NOT NULL DEFAULT '{}',
    features JSONB NOT NULL DEFAULT '{"syncEnabled":false,"webhooksEnabled":false}',

    -- Health tracking
    last_health_check_at TIMESTAMPTZ,
    last_health_status TEXT,
    last_error_at TIMESTAMPTZ,
    last_error TEXT,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,

    -- Circuit breaker state
    circuit_state TEXT NOT NULL DEFAULT 'closed',
    circuit_opened_at TIMESTAMPTZ,
    circuit_next_retry_at TIMESTAMPTZ,

    -- Lifecycle
    installed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    installed_by UUID NOT NULL,
    last_activity_at TIMESTAMPTZ,

    -- Usage
    total_requests BIGINT NOT NULL DEFAULT 0,
    total_errors BIGINT NOT NULL DEFAULT 0,
    total_latency_ms BIGINT NOT NULL DEFAULT 0,

    CONSTRAINT valid_status CHECK (status IN (
        'installed','authenticating','active','paused',
        'error','degraded','uninstalling','uninstalled'
    ))
);

ALTER TABLE connector_instances ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON connector_instances
    USING (tenant_id = current_setting('app.current_tenant')::UUID);

-- Connector credentials (encrypted)
CREATE TABLE connector_credentials (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    instance_id UUID NOT NULL REFERENCES connector_instances(id) ON DELETE CASCADE,
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    credential_type TEXT NOT NULL,    -- 'oauth2_token', 'api_key', 'bearer', etc.
    encrypted_data BYTEA NOT NULL,
    encryption_key_id TEXT NOT NULL,
    expires_at TIMESTAMPTZ,
    refreshed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT valid_credential_type CHECK (credential_type IN (
        'oauth2_token','api_key','bearer_token','basic_auth',
        'service_account','mtls_cert'
    ))
);

ALTER TABLE connector_credentials ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON connector_credentials
    USING (tenant_id = current_setting('app.current_tenant')::UUID);

-- Webhook subscriptions (outgoing)
CREATE TABLE webhook_subscriptions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    url TEXT NOT NULL,
    events JSONB NOT NULL,           -- Event type patterns
    secret_hash TEXT NOT NULL,
    active BOOLEAN NOT NULL DEFAULT true,
    custom_headers JSONB DEFAULT '{}',
    retry_policy JSONB NOT NULL DEFAULT '{"maxRetries":3}',
    created_by UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_delivery_at TIMESTAMPTZ,
    total_deliveries BIGINT NOT NULL DEFAULT 0,
    total_failures BIGINT NOT NULL DEFAULT 0
);

ALTER TABLE webhook_subscriptions ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON webhook_subscriptions
    USING (tenant_id = current_setting('app.current_tenant')::UUID);

-- Webhook delivery log
CREATE TABLE webhook_deliveries (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    subscription_id UUID NOT NULL REFERENCES webhook_subscriptions(id) ON DELETE CASCADE,
    tenant_id UUID NOT NULL,
    event_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    url TEXT NOT NULL,
    request_headers JSONB,
    request_body TEXT,
    response_status INTEGER,
    response_body TEXT,
    duration_ms INTEGER,
    attempt INTEGER NOT NULL DEFAULT 1,
    success BOOLEAN NOT NULL,
    error TEXT,
    delivered_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Partition by month for efficient cleanup
CREATE INDEX idx_webhook_deliveries_sub ON webhook_deliveries(subscription_id, delivered_at DESC);

-- Integration events log
CREATE TABLE integration_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL,
    type TEXT NOT NULL,
    source TEXT NOT NULL,
    data JSONB NOT NULL,
    correlation_id TEXT,
    connector_id TEXT NOT NULL,
    connector_version TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_integration_events_type ON integration_events(tenant_id, type, created_at DESC);
CREATE INDEX idx_integration_events_source ON integration_events(source, created_at DESC);
```

---

## 12. Observabilidad por Conector

### 12.1 Métricas

| Métrica | Tipo | Labels | Descripción |
|---------|------|--------|-------------|
| `connector_requests_total` | Counter | tenant, connector, action, status | Total requests por conector |
| `connector_request_duration_ms` | Histogram | tenant, connector, action | Latencia de requests |
| `connector_errors_total` | Counter | tenant, connector, action, error_code | Total errores |
| `connector_circuit_state` | Gauge | tenant, connector | 0=closed, 1=half_open, 2=open |
| `connector_rate_limit_remaining` | Gauge | tenant, connector | Tokens restantes |
| `connector_credential_expiry_seconds` | Gauge | tenant, connector | Tiempo hasta expiración |
| `connector_webhook_deliveries_total` | Counter | tenant, subscription, status | Webhooks entregados |
| `connector_webhook_delivery_duration_ms` | Histogram | tenant, subscription | Latencia de entrega |
| `connector_sync_duration_ms` | Histogram | tenant, connector | Duración de sync |
| `connector_sync_items_total` | Counter | tenant, connector, direction | Items sincronizados |

---

## 13. Decisiones Arquitectónicas

| # | Decisión | Alternativa Descartada | Justificación |
|---|----------|----------------------|---------------|
| ADR-IF01 | **Adapter pattern (interfaces uniformes por categoría)** | Connector-specific APIs expuestas al dominio | El dominio no debe conocer si usamos GitHub o GitLab. Permite swap sin cambiar lógica de negocio. |
| ADR-IF02 | **Per-connector circuit breaker** | Global circuit breaker | Un conector caído no debe afectar a otros. Aislamiento de fallos es crítico. |
| ADR-IF03 | **Connector SDK como package separado** | Connectors como código monolítico | SDK permite terceros crear connectors. Facilita testing independiente. Reduce acoplamiento. |
| ADR-IF04 | **Webhook signature verification (HMAC-SHA256)** | IP allowlisting | HMAC es más seguro y funciona con CDNs/proxies. Estándar de la industria (GitHub, Stripe, Slack). |
| ADR-IF05 | **Credential isolation (per-tenant-per-instance)** | Shared credentials pool | Compromiso de una credencial no afecta a otros tenants. Revocación granular. |
| ADR-IF06 | **Event bus para integración entre conectores** | Direct connector-to-connector calls | Desacoplamiento, replay capability, audit trail, dead-letter handling. |
| ADR-IF07 | **Sandboxing para third-party connectors** | Trust all connectors equally | Third-party code es untrusted. Sandbox previene exfiltración de datos y abuso de recursos. |

---

## 14. Plan de Implementación (Integration Fabric)

| Sprint | Entregable | Dependencias |
|--------|-----------|-------------|
| S1-S2 | Connector framework base (IConnector, BaseConnector, registry) | TypeScript project setup |
| S3-S4 | Connector Gateway (auth injection, rate limiting, circuit breaker) | Framework base |
| S5-S6 | Credential vault (encrypted storage, OAuth2 token refresh) | Encryption service (from IAM) |
| S7-S8 | GitHub connector (repos, PRs, issues, actions) | Gateway, credential vault |
| S9-S10 | Model provider connectors (Anthropic, OpenAI, Ollama) | Gateway |
| S11-S12 | Event bus + webhook receiver | Gateway |
| S13-S14 | Jira/Linear connector + Slack connector | Gateway, event bus |
| S15-S16 | Outgoing webhooks + delivery tracking | Event bus |
| S17-S18 | Connector SDK + documentation + templates | All above |
| S19-S20 | GitLab + Bitbucket connectors | SDK |
| S21-S22 | Cloud provider connectors (AWS, GCP) | SDK |
| S23-S24 | Third-party connector sandboxing | SDK, security |
| S25-S26 | Connector marketplace backend | SDK, registry |
