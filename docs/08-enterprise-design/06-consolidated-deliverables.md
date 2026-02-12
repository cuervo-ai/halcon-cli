# FASE 6 — Entregables Consolidados

> **Versión**: 1.0.0 | **Fecha**: 2026-02-06
> **Autores**: Full Architecture Team
> **Estado**: Design Complete — Ready for Implementation Review

---

## 1. Diagrama de Arquitectura Completo

```
┌─────────────────────────────────────────────────────────────────────────────────────┐
│                                                                                     │
│                          CUERVO PLATFORM — FULL ARCHITECTURE                        │
│                                                                                     │
│  ┌───────────────────────────────────────────────────────────────────────────────┐  │
│  │                              CLIENTS                                          │  │
│  │                                                                               │  │
│  │  ┌──────────┐  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐  │  │
│  │  │ CLI      │  │ Web App      │  │ IDE Plugin   │  │ CI/CD / Automation   │  │  │
│  │  │ (cuervo) │  │ (SPA)        │  │ (VS Code,    │  │ (GitHub Actions,     │  │  │
│  │  │          │  │              │  │  JetBrains)  │  │  Jenkins, etc.)      │  │  │
│  │  └────┬─────┘  └──────┬───────┘  └──────┬───────┘  └──────────┬───────────┘  │  │
│  └───────┼───────────────┼──────────────────┼─────────────────────┼──────────────┘  │
│          │               │                  │                     │                  │
│          └───────────────┴──────────────────┴─────────────────────┘                  │
│                                     │                                                │
│                              TLS 1.3 │ JWT / API Key                                 │
│                                     ▼                                                │
│  ┌───────────────────────────────────────────────────────────────────────────────┐  │
│  │                           API GATEWAY LAYER                                   │  │
│  │                                                                               │  │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────────┐   │  │
│  │  │ WAF      │  │ Rate     │  │ Auth     │  │ Request  │  │ Load         │   │  │
│  │  │          │  │ Limiter  │  │ Validator│  │ Router   │  │ Balancer     │   │  │
│  │  └──────────┘  └──────────┘  └──────────┘  └──────────┘  └──────────────┘   │  │
│  └──────────────────────────────────┬────────────────────────────────────────────┘  │
│                                     │                                                │
│  ┌──────────────────────────────────┼────────────────────────────────────────────┐  │
│  │                                  ▼                                             │  │
│  │                        CORE SERVICES LAYER                                    │  │
│  │                                                                               │  │
│  │  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐            │  │
│  │  │  AUTH SERVICE     │  │  CONTEXT SERVICE  │  │  SESSION SERVICE │            │  │
│  │  │                  │  │                  │  │                  │            │  │
│  │  │  • OAuth2/OIDC   │  │  • Resolution    │  │  • Conversations │            │  │
│  │  │  • SAML SSO      │  │  • Versioning    │  │  • State mgmt   │            │  │
│  │  │  • SCIM 2.0      │  │  • Sync          │  │  • Working set  │            │  │
│  │  │  • RBAC + ABAC   │  │  • Agent memory  │  │  • Streaming    │            │  │
│  │  │  • MFA/Passkeys  │  │  • Semantic index│  │                  │            │  │
│  │  │  • API Keys      │  │  • Caching (L1-3)│  │                  │            │  │
│  │  └──────────────────┘  └──────────────────┘  └──────────────────┘            │  │
│  │                                                                               │  │
│  │  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐            │  │
│  │  │  MODEL GATEWAY    │  │  AGENT ENGINE    │  │  TOOL EXECUTOR   │            │  │
│  │  │                  │  │                  │  │                  │            │  │
│  │  │  • Multi-provider│  │  • Orchestrator  │  │  • File ops      │            │  │
│  │  │  • Routing       │  │  • Explorer      │  │  • Bash sandbox  │            │  │
│  │  │  • Circuit break │  │  • Planner       │  │  • Git ops       │            │  │
│  │  │  • Cost tracking │  │  • Executor      │  │  • Search/Grep   │            │  │
│  │  │  • Semantic cache│  │  • Reviewer      │  │  • Web fetch     │            │  │
│  │  │  • Fallback chain│  │  • Custom agents │  │  • Custom tools  │            │  │
│  │  └──────────────────┘  └──────────────────┘  └──────────────────┘            │  │
│  │                                                                               │  │
│  │  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐            │  │
│  │  │ INTEGRATION HUB   │  │ PLUGIN HOST      │  │ AUTOMATION ENGINE│            │  │
│  │  │                  │  │                  │  │                  │            │  │
│  │  │  • Conn. Gateway │  │  • Registry      │  │  • Event triggers│            │  │
│  │  │  • Adapters      │  │  • Lifecycle mgr │  │  • Cron triggers │            │  │
│  │  │  • Credential    │  │  • Sandbox (V8)  │  │  • Webhook trig  │            │  │
│  │  │    vault         │  │  • Marketplace   │  │  • Step executor │            │  │
│  │  │  • Event bus     │  │  • Extension pts │  │  • Condition eval│            │  │
│  │  │  • Webhook mgr   │  │  • Metering      │  │                  │            │  │
│  │  └──────────────────┘  └──────────────────┘  └──────────────────┘            │  │
│  │                                                                               │  │
│  │  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐            │  │
│  │  │ AUDIT SERVICE     │  │ COST TRACKER     │  │ ANOMALY DETECTOR │            │  │
│  │  │                  │  │                  │  │                  │            │  │
│  │  │  • Hash chain    │  │  • Per-tenant    │  │  • Rule-based    │            │  │
│  │  │  • SIEM export   │  │  • Per-model     │  │  • Behavioral    │            │  │
│  │  │  • Compliance    │  │  • Budget enforce│  │  • Kill switches │            │  │
│  │  │    reports       │  │  • Projections   │  │  • Alert routing │            │  │
│  │  └──────────────────┘  └──────────────────┘  └──────────────────┘            │  │
│  └───────────────────────────────────────────────────────────────────────────────┘  │
│                                     │                                                │
│  ┌──────────────────────────────────┼────────────────────────────────────────────┐  │
│  │                                  ▼                                             │  │
│  │                         DATA LAYER                                            │  │
│  │                                                                               │  │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐      │  │
│  │  │ PostgreSQL   │  │ Redis        │  │ Qdrant       │  │ S3 / GCS     │      │  │
│  │  │ + pgvector   │  │ (cache,      │  │ (vectors,    │  │ (objects,    │      │  │
│  │  │ (primary DB, │  │  sessions,   │  │  semantic    │  │  backups,    │      │  │
│  │  │  RLS, audit) │  │  rate limit) │  │  search)     │  │  archives)   │      │  │
│  │  └──────────────┘  └──────────────┘  └──────────────┘  └──────────────┘      │  │
│  │                                                                               │  │
│  │  LOCAL (CLI):   SQLite + SQLite vec + Hnswlib + OS Keychain                   │  │
│  └───────────────────────────────────────────────────────────────────────────────┘  │
│                                     │                                                │
│  ┌──────────────────────────────────┼────────────────────────────────────────────┐  │
│  │                                  ▼                                             │  │
│  │                      EXTERNAL SERVICES                                        │  │
│  │                                                                               │  │
│  │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐            │  │
│  │  │Anthropic │ │OpenAI    │ │Google AI │ │Ollama    │ │DeepSeek  │            │  │
│  │  │(Claude)  │ │(GPT)     │ │(Gemini)  │ │(local)   │ │          │            │  │
│  │  └──────────┘ └──────────┘ └──────────┘ └──────────┘ └──────────┘            │  │
│  │                                                                               │  │
│  │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐            │  │
│  │  │GitHub    │ │Jira      │ │Slack     │ │AWS/GCP   │ │Corporate │            │  │
│  │  │GitLab    │ │Linear    │ │Teams     │ │Azure     │ │IdP (SSO) │            │  │
│  │  └──────────┘ └──────────┘ └──────────┘ └──────────┘ └──────────┘            │  │
│  └───────────────────────────────────────────────────────────────────────────────┘  │
│                                                                                     │
│  ┌───────────────────────────────────────────────────────────────────────────────┐  │
│  │                        OBSERVABILITY                                          │  │
│  │                                                                               │  │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐      │  │
│  │  │ Prometheus   │  │ Grafana      │  │ Jaeger       │  │ ELK / Loki   │      │  │
│  │  │ (metrics)    │  │ (dashboards) │  │ (traces)     │  │ (logs)       │      │  │
│  │  └──────────────┘  └──────────────┘  └──────────────┘  └──────────────┘      │  │
│  └───────────────────────────────────────────────────────────────────────────────┘  │
│                                                                                     │
└─────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Lista de Componentes Principales

| # | Componente | Tipo | Responsabilidad | Dependencias Críticas |
|---|-----------|------|----------------|----------------------|
| 1 | **API Gateway** | Infrastructure | Rate limiting, WAF, routing, auth validation | Auth Service |
| 2 | **Auth Service** | Core Service | OAuth2/OIDC/SAML, JWT, MFA, SCIM, RBAC+ABAC | PostgreSQL, Redis, KMS |
| 3 | **Context Service** | Core Service | Context CRUD, versioning, resolution, caching | PostgreSQL/SQLite, Redis, Qdrant |
| 4 | **Session Service** | Core Service | Conversation management, state, streaming | Redis, PostgreSQL |
| 5 | **Model Gateway** | Core Service | Multi-provider routing, fallback, cost tracking | External model APIs |
| 6 | **Agent Engine** | Core Service | Agent orchestration (Explorer, Planner, Executor, Reviewer) | Model Gateway, Tool Executor |
| 7 | **Tool Executor** | Core Service | Sandboxed tool execution, permissions | File system, Git |
| 8 | **Integration Hub** | Core Service | Connector gateway, credentials, lifecycle | Credential Vault, Event Bus |
| 9 | **Plugin Host** | Core Service | Plugin lifecycle, sandbox runtime, API surface | V8 Isolates |
| 10 | **Automation Engine** | Core Service | Event/cron/webhook triggers, step execution | Event Bus |
| 11 | **Audit Service** | Cross-cutting | Tamper-proof logging, hash chain, export | PostgreSQL (append-only) |
| 12 | **Cost Tracker** | Cross-cutting | Per-tenant cost tracking, budget enforcement | PostgreSQL, Redis |
| 13 | **Anomaly Detector** | Cross-cutting | Behavioral analysis, kill switches | Audit events, Metrics |
| 14 | **Semantic Index** | Data Service | Embedding generation, RAG search | Qdrant/pgvector, SQLite vec |
| 15 | **Credential Vault** | Security | Encrypted credential storage, rotation | KMS, OS Keychain |
| 16 | **Event Bus** | Infrastructure | Pub/sub, webhook dispatch, dead letter queue | Redis/Kafka |
| 17 | **Marketplace** | Platform | Plugin discovery, review, distribution | Plugin Host |

---

## 3. Definición de Interfaces Consolidada (TypeScript)

```typescript
// ═══════════════════════════════════════════════════════════════
// CONSOLIDATED INTERFACES — CUERVO PLATFORM
// ═══════════════════════════════════════════════════════════════

// ─── Core Domain Ports ──────────────────────────────────────

export interface IAuthService {
  authenticate(credentials: AuthCredentials): Promise<AuthResult>;
  authorize(request: AuthorizationRequest): Promise<AuthorizationDecision>;
  issueToken(userId: string, scopes: string[]): Promise<TokenPair>;
  refreshToken(refreshToken: string): Promise<TokenPair>;
  revokeToken(tokenId: string): Promise<void>;
  validateToken(token: string): Promise<TokenClaims>;
}

export interface IContextService {
  resolve(params: ResolutionParams): Promise<ResolvedContext>;
  get<T>(tenantId: string, scope: ContextScope, key: string): Promise<ContextEntry<T> | null>;
  set<T>(tenantId: string, scope: ContextScope, key: string, value: T, createdBy: string): Promise<ContextEntry<T>>;
  delete(tenantId: string, scope: ContextScope, key: string): Promise<void>;
  search(projectId: string, query: string, options: SearchOptions): Promise<SearchResult[]>;
  sync(tenantId: string): Promise<SyncReport>;
}

export interface ISessionService {
  create(params: CreateSessionParams): Promise<Session>;
  get(sessionId: string): Promise<Session | null>;
  sendMessage(sessionId: string, content: string, options?: MessageOptions): Promise<AssistantMessage>;
  streamMessage(sessionId: string, content: string): AsyncIterable<StreamEvent>;
  end(sessionId: string): Promise<void>;
}

export interface IModelGateway {
  invoke(request: UnifiedModelRequest): Promise<UnifiedModelResponse>;
  stream(request: UnifiedModelRequest): AsyncIterable<ModelStreamChunk>;
  listModels(): Promise<ModelDefinition[]>;
  getCapabilities(modelId: string): Promise<ModelCapabilities>;
  checkAvailability(provider: string): Promise<ProviderHealth>;
}

export interface IAgentEngine {
  createTask(params: AgentTaskParams): Promise<AgentTask>;
  executeTask(taskId: string): Promise<AgentTaskResult>;
  getTaskStatus(taskId: string): Promise<AgentTaskStatus>;
  cancelTask(taskId: string): Promise<void>;
}

export interface IToolExecutor {
  execute(tool: string, input: Record<string, unknown>, context: ToolContext): Promise<ToolResult>;
  listTools(): Promise<ToolDefinition[]>;
  checkPermission(tool: string, principal: Principal): Promise<boolean>;
}

export interface IConnectorService {
  install(tenantId: string, connectorId: string, config: Record<string, unknown>): Promise<ConnectorInstance>;
  execute(instanceId: string, action: ConnectorAction): Promise<ConnectorResult>;
  uninstall(instanceId: string): Promise<void>;
  healthCheck(instanceId: string): Promise<HealthCheckResult>;
  listInstalled(tenantId: string): Promise<ConnectorInstance[]>;
}

export interface IPluginHost {
  install(tenantId: string, pluginId: string, version: string): Promise<PluginInstance>;
  activate(instanceId: string): Promise<void>;
  deactivate(instanceId: string): Promise<void>;
  uninstall(instanceId: string): Promise<void>;
  executeCommand(command: string, args: Record<string, unknown>): Promise<void>;
}

export interface IAuditService {
  record(event: AuditEvent): Promise<void>;
  query(params: AuditQueryParams): Promise<PaginatedResult<AuditEvent>>;
  verifyChain(tenantId: string): Promise<ChainVerificationResult>;
  export(params: AuditExportParams): Promise<AuditExportResult>;
  generateReport(params: ComplianceReportParams): Promise<ComplianceReport>;
}

export interface ICostTracker {
  recordCost(event: CostEvent): Promise<void>;
  checkBudget(tenantId: string, estimatedCost: number): Promise<BudgetCheckResult>;
  getCurrentCost(tenantId: string, period: string): Promise<TenantCost>;
  getBreakdown(tenantId: string, period: string): Promise<CostBreakdown>;
}

export interface IKillSwitchService {
  activate(killSwitch: KillSwitchActivation): Promise<void>;
  deactivate(id: string, by: string, reason: string): Promise<void>;
  check(context: { tenantId: string; operation: string }): Promise<KillSwitchCheck>;
  listActive(tenantId?: string): Promise<KillSwitch[]>;
}
```

---

## 4. Flujos de Autenticación (Resumen)

| Flujo | Método | Token TTL | Use Case |
|-------|--------|----------|----------|
| **CLI Login** | Device Authorization (RFC 8628) | Access: 15min, Refresh: 7d | Developer using `cuervo login` |
| **Web Login** | OAuth2 Authorization Code + PKCE | Access: 15min, Refresh: 7d | Web application login |
| **SSO** | SAML 2.0 / OIDC via corporate IdP | Access: 15min, Refresh: org-policy | Enterprise single sign-on |
| **Machine-to-Machine** | Client Credentials | Access: 1h | CI/CD, automation, services |
| **API Key** | Bearer token (hashed) | Configurable (90d default) | Third-party integrations |
| **Local Mode** | None (OS keychain for provider keys) | N/A | Standalone CLI, no auth server |

---

## 5. Threat Model Resumido

### Top 10 Threats (by Risk Score)

| # | Threat | Risk | Impact | Probability | Controls |
|---|--------|------|--------|-------------|----------|
| 1 | Cross-tenant data leak | **Critical** | Catastrophic | Low | RLS + app check + pentest |
| 2 | JWT key compromise | **Critical** | Catastrophic | Very Low | HSM, 90d rotation, RS256 |
| 3 | Prompt injection → tool escape | **Critical** | High | Medium | Sandbox, allow-list, HITL |
| 4 | Stolen refresh token | **High** | High | Medium | Family rotation, device binding |
| 5 | SCIM token compromise | **High** | High | Low | IP allowlist, dedicated token, audit |
| 6 | Privilege escalation via role flaw | **High** | High | Low | Server-side authz, step-up auth |
| 7 | Plugin sandbox escape | **High** | High | Low | V8 isolate, resource limits |
| 8 | API key leak in third-party | **High** | Medium | Medium | Key rotation, PII redaction |
| 9 | SSO misconfiguration | **Medium** | High | Low | SAML validation, assertion checks |
| 10 | Cost abuse (denial-of-wallet) | **Medium** | Medium | Medium | Budget limits, kill switches |

---

## 6. Decisiones Arquitectónicas Consolidadas

### Decisiones de Alto Impacto

| ADR | Decision | Impact | Trade-off |
|-----|----------|--------|-----------|
| **ADR-001** | TypeScript monolanguage | High | Ecosystem consistency vs. performance for compute-heavy tasks. Mitigation: Rust/Go for specific hot paths (embedding generation). |
| **ADR-002** | Clean Architecture + DDD | High | Complexity of setup vs. long-term maintainability. Justified by multi-team development and plugin system requirements. |
| **ADR-003** | Offline-first with cloud sync | High | Sync complexity vs. user autonomy and privacy. Offline-first is a key differentiator and privacy requirement. |
| **ADR-004** | Multi-provider model gateway | High | Integration complexity vs. vendor lock-in prevention. Essential for enterprise adoption and cost optimization. |
| **ADR-005** | RLS + application-level tenant isolation | High | Performance overhead (~5-10% on queries) vs. security guarantees. Defense-in-depth is non-negotiable for enterprise. |
| **ADR-006** | Append-only audit with hash chain | Medium | Storage growth vs. tamper evidence. Required for SOC 2. Partitioning + archiving mitigates storage. |
| **ADR-007** | Device Authorization for CLI auth | Medium | Slightly more complex than localhost redirect vs. works in headless/SSH environments. |
| **ADR-008** | V8 isolates for plugin sandbox | Medium | Startup overhead vs. true isolation. V8 isolates are lightweight (~10ms) and battle-tested (Cloudflare Workers). |
| **ADR-009** | Event-driven integration fabric | Medium | Eventual consistency vs. decoupling. Event-driven enables reactive workflows and audit trail. |
| **ADR-010** | RBAC + ABAC hybrid authorization | Medium | Policy complexity vs. enterprise flexibility. RBAC alone insufficient for conditional access requirements. |

---

## 7. Trade-offs Técnicos

| Trade-off | Option A (Chosen) | Option B (Rejected) | Justification |
|-----------|-------------------|---------------------|---------------|
| **Local DB** | SQLite | PostgreSQL embedded | SQLite is truly zero-config, single-file, supports vec extension. PG embedded adds complexity. |
| **Vector search (local)** | SQLite vec + Hnswlib | Chroma, Weaviate embedded | Fewer dependencies, tighter integration, sufficient for project-scale indexes. |
| **Vector search (cloud)** | Qdrant | Pinecone, Weaviate | Self-hostable (required for enterprise), high performance, active OSS community. |
| **API format** | REST + SSE | GraphQL | REST is simpler, better caching, wider tooling. SSE sufficient for streaming. GraphQL later as option. |
| **Event bus** | Redis Streams (initial) → Kafka (scale) | NATS, RabbitMQ | Redis already in stack, Streams sufficient initially. Kafka for >10K events/sec. |
| **Secret storage (local)** | OS Keychain | Encrypted file | OS Keychain uses hardware security (Secure Enclave on macOS), better than any software solution. |
| **Plugin runtime** | V8 Isolates | Wasm, Docker | V8 has full JavaScript API, fastest startup, battle-tested isolation. Wasm limited API surface. |
| **Tracing** | OpenTelemetry | Vendor-specific SDK | Vendor-neutral, no lock-in, wide ecosystem. Can export to Jaeger, Datadog, Honeycomb, etc. |
| **Config format** | YAML | TOML, JSON | YAML has comments, human-readable, widely used in DevOps ecosystem. |
| **Encryption** | AES-256-GCM | ChaCha20-Poly1305 | AES has hardware acceleration on all modern CPUs. Both are AEAD and equally secure. |

---

## 8. Plan de Implementación por Fases

### Phase 0: Foundation (Sprints 1-4, ~8 weeks)

| Sprint | Deliverable | Team |
|--------|-----------|------|
| S1-S2 | Project scaffolding, CI/CD, linting, testing framework | Platform |
| S3-S4 | SQLite setup, domain entities, repository interfaces | Backend |

### Phase 1: Core MVP (Sprints 5-16, ~24 weeks)

| Sprint | Deliverable | Team |
|--------|-----------|------|
| S5-S6 | Context service (CRUD, versioning, resolution) | Backend |
| S7-S8 | Auth service (password, JWT, refresh tokens) | Security |
| S9-S10 | Model Gateway (Anthropic + Ollama providers) | AI |
| S11-S12 | Tool Executor (file ops, bash sandbox, git) | Platform |
| S13-S14 | Agent Engine (single agent, basic orchestration) | AI |
| S15-S16 | CLI REPL, slash commands, markdown rendering | Frontend |

**MVP Milestone**: Working CLI with single-model, basic auth, file tools.

### Phase 2: Enterprise Foundation (Sprints 17-28, ~24 weeks)

| Sprint | Deliverable | Team |
|--------|-----------|------|
| S17-S18 | Multi-provider model gateway (OpenAI, Google, DeepSeek) | AI |
| S19-S20 | RBAC engine, API keys, service accounts | Security |
| S21-S22 | Semantic indexing (Tree-sitter + embeddings + RAG) | AI |
| S23-S24 | Multi-agent orchestration (Explorer, Planner, Executor, Reviewer) | AI |
| S25-S26 | Audit service (hash chain, structured logging) | Security |
| S27-S28 | Cost tracking, budget enforcement, usage analytics | Platform |

**Beta Milestone**: Multi-model, multi-agent, basic audit, cost controls.

### Phase 3: Integration & Extensibility (Sprints 29-40, ~24 weeks)

| Sprint | Deliverable | Team |
|--------|-----------|------|
| S29-S30 | Connector framework + GitHub connector | Platform |
| S31-S32 | Plugin system (host, sandbox, API surface) | Platform |
| S33-S34 | OAuth2/OIDC login, Device Authorization (CLI) | Security |
| S35-S36 | Jira/Linear + Slack connectors | Platform |
| S37-S38 | Public REST API + SDK | Platform |
| S39-S40 | Webhook system (incoming + outgoing) | Platform |

**Integration Milestone**: Extensible platform with connectors and plugins.

### Phase 4: Enterprise Security (Sprints 41-52, ~24 weeks)

| Sprint | Deliverable | Team |
|--------|-----------|------|
| S41-S42 | SAML 2.0 SSO integration | Security |
| S43-S44 | SCIM 2.0 provisioning | Security |
| S45-S46 | ABAC conditional access engine | Security |
| S47-S48 | MFA: TOTP + WebAuthn/Passkeys | Security |
| S49-S50 | Anomaly detection, kill switches | Security |
| S51-S52 | Encryption service, key management, rotation | Security |

**Security Milestone**: Enterprise-grade IAM, Zero Trust, full audit.

### Phase 5: Compliance & Scale (Sprints 53-60, ~16 weeks)

| Sprint | Deliverable | Team |
|--------|-----------|------|
| S53-S54 | Data residency implementation | Platform |
| S55-S56 | SIEM integration, compliance reports (SOC 2, GDPR) | Security |
| S57-S58 | Marketplace backend, plugin review pipeline | Platform |
| S59-S60 | Penetration testing, DRP testing, SOC 2 Type I prep | Security |

**GA Milestone**: Compliance-certified, multi-region, marketplace.

### Total Timeline

```
2026 Q1-Q2: Phase 0 + Phase 1 (MVP)           — 32 weeks
2026 Q3-Q4: Phase 2 (Enterprise Foundation)    — 24 weeks
2027 Q1-Q2: Phase 3 (Integration & Ext.)       — 24 weeks
2027 Q3-Q4: Phase 4 (Enterprise Security)      — 24 weeks
2028 Q1:    Phase 5 (Compliance & Scale)        — 16 weeks
                                                ──────────
                                Total:          ~120 weeks (2.3 years)
```

### Team Composition Recommendation

| Role | Phase 0-1 | Phase 2-3 | Phase 4-5 |
|------|----------|----------|----------|
| Backend Engineers | 3 | 5 | 5 |
| AI/ML Engineers | 1 | 3 | 2 |
| Security Engineers | 1 | 2 | 3 |
| Platform Engineers | 1 | 2 | 3 |
| Frontend (CLI/Web) | 1 | 2 | 2 |
| DevOps/SRE | 1 | 1 | 2 |
| QA | 1 | 2 | 2 |
| **Total** | **9** | **17** | **19** |

---

## 9. Dependency Map

```
┌─────────────────────────────────────────────────────────────────────┐
│                   COMPONENT DEPENDENCY MAP                           │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│   Build order (topological sort):                                    │
│                                                                      │
│   Layer 0 (no dependencies):                                         │
│   ├── Domain entities & interfaces                                   │
│   ├── SQLite/PostgreSQL schemas                                      │
│   └── Encryption primitives                                          │
│                                                                      │
│   Layer 1 (depends on Layer 0):                                      │
│   ├── Auth Service (entities + DB)                                   │
│   ├── Context Repository (entities + DB)                             │
│   └── Audit Logger (entities + DB)                                   │
│                                                                      │
│   Layer 2 (depends on Layer 1):                                      │
│   ├── Context Resolver (Context Repo + Auth)                         │
│   ├── Model Gateway (Auth + Audit)                                   │
│   └── Tool Executor (Auth + Audit)                                   │
│                                                                      │
│   Layer 3 (depends on Layer 2):                                      │
│   ├── Agent Engine (Model Gateway + Tool Executor + Context)         │
│   ├── Session Service (Context + Auth)                               │
│   └── Integration Hub (Auth + Audit + Event Bus)                     │
│                                                                      │
│   Layer 4 (depends on Layer 3):                                      │
│   ├── Plugin Host (Agent Engine + Tool Executor + Context)           │
│   ├── Public API (Session + Auth + all services)                     │
│   ├── Automation Engine (Event Bus + Agent Engine)                   │
│   └── CLI REPL (Session + Agent Engine + all services)               │
│                                                                      │
│   Layer 5 (depends on Layer 4):                                      │
│   ├── SDK (Public API)                                               │
│   ├── Marketplace (Plugin Host)                                      │
│   ├── Compliance Reports (Audit + all services)                      │
│   └── Cost Analytics (Cost Tracker + all services)                   │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

---

## 10. Riesgos del Programa

| Risk | Probability | Impact | Mitigation |
|------|------------|--------|-----------|
| Scope creep in enterprise features | High | High | Strict MVP scoping, feature flags, incremental delivery |
| Model provider API changes | Medium | Medium | Adapter pattern isolates impact; monitor changelogs |
| Hiring security engineers (LATAM) | Medium | High | Remote-first, competitive compensation, begin recruiting early |
| SOC 2 audit timeline | Medium | High | Start evidence collection in Phase 2, engage auditor in Phase 4 |
| SQLite performance at scale | Low | Medium | Migration path to PostgreSQL documented; benchmarks at 100K files |
| Plugin security incidents | Medium | High | Sandbox, review process, kill switches, incident response plan |
| Multi-region complexity | Medium | High | Start with 2 regions (US + EU), add incrementally |
| Competitor feature parity | High | Medium | Focus on differentiators (multi-model, LATAM, self-hosted, open source) |

---

*Este documento concluye el diseño técnico empresarial de Cuervo CLI. Los 6 documentos de esta sección proporcionan la base arquitectónica completa para implementación.*
