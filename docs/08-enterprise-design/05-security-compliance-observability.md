# FASE 5 — Seguridad, Compliance y Observabilidad

> **Versión**: 1.0.0 | **Fecha**: 2026-02-06
> **Autores**: Compliance & Privacy Lead + Security Architecture Team
> **Estado**: Design Complete — Ready for Implementation Review

---

## 1. Visión General

Este documento define las fundaciones de seguridad, cumplimiento regulatorio y observabilidad que permean toda la plataforma Cuervo. No son features añadidas post-hoc: son **propiedades del sistema** que se diseñan desde el primer día y se verifican continuamente.

### 1.1 Objetivos de Seguridad

| Propiedad | Objetivo | Medida |
|-----------|----------|--------|
| **Confidencialidad** | Datos de un tenant nunca accesibles por otro | RLS + application-level + penetration testing |
| **Integridad** | Audit logs tamper-proof, código no modificado sin traza | Hash chain, signed artifacts |
| **Disponibilidad** | 99.9% uptime para servicios cloud | Multi-AZ, failover, DRP tested |
| **Non-repudiation** | Toda acción es atribuible a un principal | Audit log con firma cryptográfica |
| **Privacy** | Datos personales protegidos, minimizados, controlados | GDPR/LGPD compliance, PII detection |

---

## 2. Auditoría Completa

### 2.1 Audit Event Schema

```typescript
// domain/entities/audit.ts

/**
 * Immutable audit event. Stored in append-only log with hash chain.
 * Designed for SOC 2, ISO 27001, and EU AI Act compliance.
 */
interface AuditEvent {
  /** Globally unique event ID (UUID v7, time-sortable) */
  readonly id: string;

  /** Tenant boundary */
  readonly tenantId: string;

  /** Event classification */
  readonly category: AuditCategory;
  readonly action: string;
  readonly severity: AuditSeverity;

  /** Who performed the action */
  readonly actor: {
    type: 'user' | 'service_account' | 'agent' | 'system' | 'plugin';
    id: string;
    email?: string;
    ipAddress?: string;
    userAgent?: string;
    deviceId?: string;
    sessionId?: string;
  };

  /** What was acted upon */
  readonly resource: {
    type: string;           // 'project', 'model', 'tool', 'user', 'org', etc.
    id: string;
    name?: string;
  };

  /** Action outcome */
  readonly outcome: 'success' | 'failure' | 'denied' | 'error';
  readonly outcomeReason?: string;

  /** Event-specific data (varies by action) */
  readonly data: Record<string, unknown>;

  /** Context at time of event */
  readonly context: {
    projectId?: string;
    sessionId?: string;
    agentType?: string;
    modelUsed?: string;
    tokensUsed?: number;
    costUSD?: number;
    durationMs?: number;
  };

  /** Timestamp (ISO 8601, microsecond precision) */
  readonly timestamp: string;

  /** Hash chain for tamper detection */
  readonly sequenceNumber: number;
  readonly previousHash: string | null;
  readonly eventHash: string;

  /** Digital signature (optional, for critical events) */
  readonly signature?: {
    algorithm: 'Ed25519';
    keyId: string;
    value: string;
  };

  /** Data classification */
  readonly classification: 'public' | 'internal' | 'confidential' | 'restricted';

  /** Retention category (determines how long to keep) */
  readonly retentionClass: 'standard' | 'compliance' | 'legal_hold';
}

type AuditCategory =
  | 'auth'           // Login, logout, MFA, password changes
  | 'authz'          // Permission checks, role changes
  | 'data'           // CRUD on data resources
  | 'ai'             // Model invocations, agent actions
  | 'tool'           // Tool executions
  | 'security'       // Security events, anomalies
  | 'privacy'        // PII access, data subject requests
  | 'admin'          // Org settings, member management
  | 'billing'        // Plan changes, payments
  | 'integration'    // Connector operations
  | 'plugin'         // Plugin lifecycle
  | 'system';        // System events, maintenance

type AuditSeverity =
  | 'critical'       // Security breach, data loss
  | 'high'           // Auth failures, permission denials, destructive ops
  | 'medium'         // Config changes, user management
  | 'low'            // Normal operations
  | 'info';          // Informational (reads, queries)
```

### 2.2 Audit Event Catalog

| Category | Action | Severity | Description |
|----------|--------|----------|-------------|
| **auth** | `auth.login.success` | low | Successful login |
| **auth** | `auth.login.failure` | high | Failed login attempt |
| **auth** | `auth.login.locked` | critical | Account locked after failures |
| **auth** | `auth.mfa.enrolled` | medium | MFA method added |
| **auth** | `auth.mfa.verified` | low | MFA challenge passed |
| **auth** | `auth.token.issued` | info | Access token issued |
| **auth** | `auth.token.revoked` | medium | Token revoked |
| **auth** | `auth.session.expired` | info | Session expired |
| **auth** | `auth.password.changed` | medium | Password changed |
| **auth** | `auth.sso.login` | low | SSO login |
| **authz** | `authz.check.denied` | high | Authorization denied |
| **authz** | `authz.role.assigned` | medium | Role assigned to user |
| **authz** | `authz.role.removed` | medium | Role removed from user |
| **authz** | `authz.policy.changed` | high | Security policy updated |
| **data** | `data.project.created` | medium | Project created |
| **data** | `data.project.deleted` | high | Project deleted |
| **data** | `data.context.updated` | low | Context entry updated |
| **data** | `data.export.requested` | high | Data export requested |
| **data** | `data.deletion.requested` | high | Data deletion requested (GDPR) |
| **ai** | `ai.model.invoked` | info | Model invocation |
| **ai** | `ai.model.error` | medium | Model invocation failed |
| **ai** | `ai.agent.started` | low | Agent task started |
| **ai** | `ai.agent.completed` | low | Agent task completed |
| **ai** | `ai.injection.detected` | critical | Prompt injection detected |
| **ai** | `ai.pii.detected` | high | PII detected in AI input/output |
| **ai** | `ai.budget.exceeded` | high | Token/cost budget exceeded |
| **tool** | `tool.executed` | low | Tool execution |
| **tool** | `tool.destructive.executed` | high | Destructive tool (delete, overwrite) |
| **tool** | `tool.bash.executed` | medium | Bash command execution |
| **tool** | `tool.denied` | high | Tool execution denied |
| **security** | `security.anomaly.detected` | critical | Behavioral anomaly |
| **security** | `security.brute_force.detected` | critical | Brute force attack |
| **security** | `security.token_theft.detected` | critical | Token replay detected |
| **security** | `security.cross_tenant.attempted` | critical | Cross-tenant access attempt |
| **privacy** | `privacy.pii.accessed` | high | PII data accessed |
| **privacy** | `privacy.consent.updated` | medium | User consent changed |
| **privacy** | `privacy.dsr.received` | high | Data subject request received |
| **privacy** | `privacy.dsr.completed` | high | Data subject request completed |
| **admin** | `admin.org.settings.updated` | medium | Org settings changed |
| **admin** | `admin.member.invited` | medium | Member invited |
| **admin** | `admin.member.removed` | high | Member removed |
| **admin** | `admin.sso.configured` | high | SSO configuration changed |
| **admin** | `admin.scim.synced` | low | SCIM sync completed |
| **integration** | `integration.connector.installed` | medium | Connector installed |
| **integration** | `integration.connector.error` | medium | Connector error |
| **plugin** | `plugin.installed` | medium | Plugin installed |
| **plugin** | `plugin.sandbox.violation` | critical | Plugin sandbox escape attempt |
| **billing** | `billing.plan.changed` | high | Plan upgrade/downgrade |
| **billing** | `billing.payment.failed` | high | Payment failed |

### 2.3 Hash Chain Implementation

```typescript
// infrastructure/audit/hash-chain.ts

import { createHash, sign, verify } from 'node:crypto';

/**
 * Computes the hash for an audit event, creating a tamper-evident chain.
 * If any event is modified or deleted, the chain breaks.
 */
function computeEventHash(event: {
  id: string;
  tenantId: string;
  category: string;
  action: string;
  actor: Record<string, unknown>;
  resource: Record<string, unknown>;
  outcome: string;
  data: Record<string, unknown>;
  timestamp: string;
  sequenceNumber: number;
  previousHash: string | null;
}): string {
  const canonical = JSON.stringify({
    id: event.id,
    t: event.tenantId,
    c: event.category,
    a: event.action,
    ac: event.actor,
    r: event.resource,
    o: event.outcome,
    d: event.data,
    ts: event.timestamp,
    seq: event.sequenceNumber,
    prev: event.previousHash,
  });

  return createHash('sha256').update(canonical).digest('hex');
}

/**
 * Verify integrity of the audit chain for a tenant.
 * Walks backwards from the latest event, verifying each link.
 */
async function verifyAuditChain(
  tenantId: string,
  repository: IAuditRepository,
): Promise<ChainVerificationResult> {
  let currentSeq = await repository.getLatestSequenceNumber(tenantId);
  let expectedPrevHash: string | null = null;
  let eventsVerified = 0;
  let firstBrokenAt: number | null = null;

  while (currentSeq > 0) {
    const event = await repository.getBySequenceNumber(tenantId, currentSeq);
    if (!event) {
      return { valid: false, eventsVerified, brokenAt: currentSeq, reason: 'missing_event' };
    }

    const computedHash = computeEventHash(event);
    if (computedHash !== event.eventHash) {
      return { valid: false, eventsVerified, brokenAt: currentSeq, reason: 'hash_mismatch' };
    }

    if (expectedPrevHash !== null && event.eventHash !== expectedPrevHash) {
      return { valid: false, eventsVerified, brokenAt: currentSeq, reason: 'chain_broken' };
    }

    expectedPrevHash = event.previousHash;
    eventsVerified++;
    currentSeq--;
  }

  return { valid: true, eventsVerified, brokenAt: null, reason: null };
}

interface ChainVerificationResult {
  valid: boolean;
  eventsVerified: number;
  brokenAt: number | null;
  reason: 'missing_event' | 'hash_mismatch' | 'chain_broken' | null;
}
```

### 2.4 Audit Storage

```sql
-- Append-only audit log (no UPDATE or DELETE operations allowed)
CREATE TABLE audit_events (
    id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL,
    category TEXT NOT NULL,
    action TEXT NOT NULL,
    severity TEXT NOT NULL,

    -- Actor
    actor_type TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    actor_email TEXT,
    actor_ip TEXT,
    actor_user_agent TEXT,
    actor_device_id TEXT,
    actor_session_id TEXT,

    -- Resource
    resource_type TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    resource_name TEXT,

    -- Outcome
    outcome TEXT NOT NULL,
    outcome_reason TEXT,

    -- Data
    event_data JSONB NOT NULL DEFAULT '{}',

    -- Context
    context_project_id TEXT,
    context_session_id TEXT,
    context_agent_type TEXT,
    context_model TEXT,
    context_tokens INTEGER,
    context_cost_usd NUMERIC(10,6),
    context_duration_ms INTEGER,

    -- Timestamp
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- Hash chain
    sequence_number BIGINT NOT NULL,
    previous_hash TEXT,
    event_hash TEXT NOT NULL,

    -- Signature (for critical events)
    signature_algorithm TEXT,
    signature_key_id TEXT,
    signature_value TEXT,

    -- Classification
    classification TEXT NOT NULL DEFAULT 'internal',
    retention_class TEXT NOT NULL DEFAULT 'standard',

    CONSTRAINT valid_severity CHECK (severity IN ('critical','high','medium','low','info')),
    CONSTRAINT valid_outcome CHECK (outcome IN ('success','failure','denied','error')),
    CONSTRAINT valid_classification CHECK (classification IN ('public','internal','confidential','restricted'))
);

-- Prevent modifications (application-level enforcement + database trigger)
CREATE OR REPLACE FUNCTION prevent_audit_modification() RETURNS TRIGGER AS $$
BEGIN
    RAISE EXCEPTION 'Audit log entries cannot be modified or deleted';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER no_update_audit
    BEFORE UPDATE OR DELETE ON audit_events
    FOR EACH ROW EXECUTE FUNCTION prevent_audit_modification();

-- Indexes for common query patterns
CREATE INDEX idx_audit_tenant_time ON audit_events(tenant_id, created_at DESC);
CREATE INDEX idx_audit_actor ON audit_events(tenant_id, actor_id, created_at DESC);
CREATE INDEX idx_audit_category ON audit_events(tenant_id, category, action, created_at DESC);
CREATE INDEX idx_audit_severity ON audit_events(tenant_id, severity, created_at DESC)
    WHERE severity IN ('critical', 'high');
CREATE INDEX idx_audit_resource ON audit_events(tenant_id, resource_type, resource_id, created_at DESC);

-- Partitioning by month for efficient retention management
-- (partitioned table creation would go here for production)

-- RLS
ALTER TABLE audit_events ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON audit_events
    USING (tenant_id = current_setting('app.current_tenant')::UUID);
```

### 2.5 Audit Export & SIEM Integration

```typescript
// infrastructure/audit/export.ts

interface AuditExportService {
  /**
   * Export audit events to external destination.
   * Supports multiple formats and destinations for enterprise SIEM integration.
   */
  export(params: AuditExportParams): Promise<AuditExportResult>;

  /**
   * Stream audit events in real-time to external destination.
   * Uses persistent connection with at-least-once delivery.
   */
  startStreaming(params: AuditStreamParams): Promise<AuditStreamHandle>;

  /**
   * Generate compliance report from audit data.
   */
  generateReport(params: ComplianceReportParams): Promise<ComplianceReport>;
}

interface AuditExportParams {
  tenantId: string;
  /** Time range */
  startDate: string;
  endDate: string;
  /** Filters */
  categories?: AuditCategory[];
  severities?: AuditSeverity[];
  actors?: string[];
  /** Output format */
  format: 'json' | 'csv' | 'cef' | 'leef';
  /** Destination */
  destination:
    | { type: 'download' }
    | { type: 's3'; bucket: string; prefix: string; region: string }
    | { type: 'gcs'; bucket: string; prefix: string }
    | { type: 'azure_blob'; container: string; prefix: string }
    | { type: 'webhook'; url: string; headers: Record<string, string> };
  /** Include hash chain verification metadata */
  includeVerification: boolean;
}

interface AuditStreamParams {
  tenantId: string;
  destination:
    | { type: 'syslog'; host: string; port: number; protocol: 'tcp' | 'udp' | 'tls' }
    | { type: 'kafka'; brokers: string[]; topic: string }
    | { type: 'webhook'; url: string; batchSize: number; flushIntervalMs: number };
  /** Filter which events to stream */
  filter?: {
    minSeverity?: AuditSeverity;
    categories?: AuditCategory[];
  };
}

interface ComplianceReportParams {
  tenantId: string;
  framework: 'soc2' | 'iso27001' | 'gdpr' | 'eu_ai_act' | 'lgpd';
  period: string;        // '2026-Q1', '2026-01'
  includeEvidence: boolean;
}

interface ComplianceReport {
  framework: string;
  period: string;
  generatedAt: string;
  summary: {
    totalEvents: number;
    criticalEvents: number;
    securityIncidents: number;
    policyViolations: number;
    dataSubjectRequests: number;
  };
  controls: {
    id: string;
    name: string;
    status: 'compliant' | 'partial' | 'non_compliant' | 'not_applicable';
    evidence: string[];
    findings: string[];
  }[];
  recommendations: string[];
}
```

---

## 3. Structured Logging

### 3.1 Log Schema

```typescript
// infrastructure/logging/log-schema.ts

interface StructuredLog {
  /** Log level */
  level: 'debug' | 'info' | 'warn' | 'error' | 'fatal';

  /** Structured message */
  message: string;

  /** Service identifier */
  service: string;

  /** Correlation and tracing */
  traceId: string;
  spanId: string;
  parentSpanId: string | null;

  /** Tenant context */
  tenantId: string | null;
  userId: string | null;

  /** Timing */
  timestamp: string;        // ISO 8601 with microseconds
  durationMs?: number;

  /** Error details */
  error?: {
    name: string;
    message: string;
    stack: string;
    code: string;
  };

  /** Arbitrary structured data */
  data?: Record<string, unknown>;

  /** Environment */
  environment: 'development' | 'staging' | 'production';
  version: string;
  hostname: string;
}
```

### 3.2 PII Redaction Pipeline

```typescript
// infrastructure/logging/pii-redactor.ts

interface PIIRedactor {
  /**
   * Scan and redact PII from log data before storage/transmission.
   * Ensures no personal data leaks into logs, metrics, or external services.
   */
  redact(data: unknown): unknown;

  /**
   * Scan text for PII patterns.
   * Returns detected PII with locations (for audit, not storage).
   */
  scan(text: string): PIIDetection[];
}

interface PIIDetection {
  type: PIIType;
  value: string;          // Redacted placeholder
  originalLength: number;
  startIndex: number;
  endIndex: number;
  confidence: number;     // 0.0 - 1.0
  detector: 'regex' | 'ner' | 'secret_scanner';
}

type PIIType =
  | 'email'
  | 'phone'
  | 'ssn'
  | 'credit_card'
  | 'ip_address'
  | 'person_name'
  | 'physical_address'
  | 'date_of_birth'
  | 'api_key'
  | 'password'
  | 'token'
  | 'private_key'
  | 'aws_access_key'
  | 'github_token'
  | 'jwt';

// Redaction strategies per PII type
const REDACTION_RULES: Record<PIIType, RedactionStrategy> = {
  email:           { strategy: 'mask', format: '[EMAIL]' },
  phone:           { strategy: 'mask', format: '[PHONE]' },
  ssn:             { strategy: 'mask', format: '[SSN]' },
  credit_card:     { strategy: 'mask', format: '[CC]' },
  ip_address:      { strategy: 'hash', prefix: 'ip_' },      // Hashed for correlation
  person_name:     { strategy: 'mask', format: '[NAME]' },
  physical_address:{ strategy: 'mask', format: '[ADDRESS]' },
  date_of_birth:   { strategy: 'mask', format: '[DOB]' },
  api_key:         { strategy: 'mask', format: '[API_KEY]' },
  password:        { strategy: 'mask', format: '[REDACTED]' },
  token:           { strategy: 'mask', format: '[TOKEN]' },
  private_key:     { strategy: 'mask', format: '[PRIVATE_KEY]' },
  aws_access_key:  { strategy: 'mask', format: '[AWS_KEY]' },
  github_token:    { strategy: 'mask', format: '[GH_TOKEN]' },
  jwt:             { strategy: 'mask', format: '[JWT]' },
};

type RedactionStrategy =
  | { strategy: 'mask'; format: string }
  | { strategy: 'hash'; prefix: string }
  | { strategy: 'truncate'; keepChars: number };
```

---

## 4. Observability Stack

### 4.1 Metrics (Prometheus)

```typescript
// infrastructure/observability/metrics.ts

/**
 * Key metrics exposed via Prometheus-compatible endpoint.
 * /metrics endpoint (pull model) or OTLP push.
 */

// ── Request Metrics ────────────────────────────────
// cuervo_api_requests_total{method, path, status, tenant}
// cuervo_api_request_duration_seconds{method, path, tenant} (histogram)
// cuervo_api_request_size_bytes{method, path} (histogram)
// cuervo_api_response_size_bytes{method, path} (histogram)

// ── Model Metrics ──────────────────────────────────
// cuervo_model_invocations_total{provider, model, tenant, status}
// cuervo_model_tokens_total{provider, model, tenant, direction=input|output}
// cuervo_model_cost_usd_total{provider, model, tenant}
// cuervo_model_latency_seconds{provider, model, tenant} (histogram)
// cuervo_model_circuit_state{provider} (gauge: 0=closed, 1=half_open, 2=open)

// ── Agent Metrics ──────────────────────────────────
// cuervo_agent_tasks_total{agent_type, status, tenant}
// cuervo_agent_task_duration_seconds{agent_type, tenant} (histogram)
// cuervo_agent_tool_calls_total{tool, agent_type, status}

// ── Connector Metrics ──────────────────────────────
// cuervo_connector_requests_total{connector, action, status, tenant}
// cuervo_connector_latency_seconds{connector, action} (histogram)
// cuervo_connector_circuit_state{connector, instance} (gauge)

// ── Context Metrics ────────────────────────────────
// cuervo_context_cache_hits_total{layer=l1|l2|l3}
// cuervo_context_cache_misses_total{layer}
// cuervo_context_resolution_duration_seconds (histogram)
// cuervo_semantic_search_duration_seconds (histogram)

// ── Auth Metrics ───────────────────────────────────
// cuervo_auth_login_total{method, status}
// cuervo_auth_token_issued_total{type=access|refresh|api_key}
// cuervo_auth_mfa_challenges_total{method, status}

// ── Tenant Metrics ─────────────────────────────────
// cuervo_tenant_active_sessions{tenant} (gauge)
// cuervo_tenant_storage_bytes{tenant, type} (gauge)
// cuervo_tenant_daily_cost_usd{tenant} (gauge)
// cuervo_tenant_daily_tokens{tenant} (gauge)
// cuervo_tenant_quota_usage_ratio{tenant, quota_type} (gauge: 0.0-1.0)

// ── System Metrics ─────────────────────────────────
// cuervo_process_cpu_seconds_total
// cuervo_process_memory_bytes
// cuervo_db_connections_active{database}
// cuervo_db_query_duration_seconds{database, operation} (histogram)
// cuervo_queue_depth{queue_name} (gauge)
// cuervo_queue_processing_duration_seconds{queue_name} (histogram)
```

### 4.2 Distributed Tracing (OpenTelemetry)

```typescript
// infrastructure/observability/tracing.ts

/**
 * Trace spans for key operations.
 * Uses OpenTelemetry SDK for vendor-neutral tracing.
 */

// Span naming conventions:
// <service>.<component>.<operation>

// Example trace for a user message:
//
// [cuervo-api.http.post_message]                    (root span)
//   ├── [cuervo-auth.jwt.validate]                  (auth check)
//   ├── [cuervo-context.resolver.resolve_all]       (context resolution)
//   │   ├── [cuervo-context.cache.l1_get]           (cache check)
//   │   └── [cuervo-context.db.query]               (DB fallback)
//   ├── [cuervo-agent.orchestrator.process]          (agent processing)
//   │   ├── [cuervo-model.gateway.invoke]            (model call)
//   │   │   ├── [cuervo-model.anthropic.chat]        (external API)
//   │   │   └── [cuervo-model.cache.semantic_check]  (semantic cache)
//   │   ├── [cuervo-tool.executor.read_file]         (tool execution)
//   │   └── [cuervo-tool.executor.write_file]        (tool execution)
//   ├── [cuervo-audit.logger.record]                 (audit logging)
//   └── [cuervo-context.session.update]              (session update)

interface TracingConfig {
  /** OTLP endpoint for trace export */
  endpoint: string;
  /** Sampling rate (0.0-1.0). 1.0 = trace everything */
  samplingRate: number;
  /** Always trace these operations regardless of sampling */
  alwaysTrace: string[];
  /** Propagation format */
  propagation: 'w3c' | 'b3' | 'jaeger';
  /** Max attributes per span */
  maxAttributes: number;
  /** Max events per span */
  maxEvents: number;
}

// Default configuration
const DEFAULT_TRACING_CONFIG: TracingConfig = {
  endpoint: 'http://otel-collector:4318',
  samplingRate: 0.1,          // 10% in production
  alwaysTrace: [
    'auth.login',
    'security.*',
    'billing.*',
    'ai.model.invoke',        // Always trace model calls (cost tracking)
  ],
  propagation: 'w3c',
  maxAttributes: 64,
  maxEvents: 128,
};
```

### 4.3 SLOs and SLIs

```typescript
// infrastructure/observability/slos.ts

interface SLODefinition {
  name: string;
  description: string;
  /** Service Level Indicator */
  sli: SLI;
  /** Service Level Objective */
  target: number;          // e.g., 0.999 for 99.9%
  /** Measurement window */
  window: '7d' | '28d' | '30d' | '90d';
  /** Error budget burn rate alerts */
  alerts: SLOAlert[];
}

type SLI =
  | { type: 'availability'; service: string }
  | { type: 'latency'; service: string; percentile: number; threshold_ms: number }
  | { type: 'error_rate'; service: string; error_codes: string[] }
  | { type: 'throughput'; service: string; min_rps: number };

interface SLOAlert {
  name: string;
  burnRate: number;           // e.g., 14.4 = 14.4x faster than budget
  window: string;             // e.g., '1h'
  severity: 'warning' | 'critical' | 'page';
  channel: string;            // 'slack:#platform-alerts', 'pagerduty:platform'
}

// ── Defined SLOs ─────────────────────────────────

const PLATFORM_SLOS: SLODefinition[] = [
  {
    name: 'API Availability',
    description: 'Public API returns non-5xx responses',
    sli: { type: 'availability', service: 'cuervo-api' },
    target: 0.999,            // 99.9% = ~8.7h downtime/year
    window: '30d',
    alerts: [
      { name: '1h_fast_burn', burnRate: 14.4, window: '1h', severity: 'page', channel: 'pagerduty:platform' },
      { name: '6h_slow_burn', burnRate: 6, window: '6h', severity: 'critical', channel: 'slack:#platform-critical' },
      { name: '3d_budget_warn', burnRate: 1, window: '3d', severity: 'warning', channel: 'slack:#platform-alerts' },
    ],
  },
  {
    name: 'API Latency (p99)',
    description: '99th percentile API response time < 2s',
    sli: { type: 'latency', service: 'cuervo-api', percentile: 99, threshold_ms: 2000 },
    target: 0.99,
    window: '30d',
    alerts: [
      { name: 'latency_spike', burnRate: 10, window: '30m', severity: 'critical', channel: 'slack:#platform-critical' },
    ],
  },
  {
    name: 'Model Invocation Success',
    description: 'Model invocations complete without error',
    sli: { type: 'error_rate', service: 'model-gateway', error_codes: ['5xx', 'timeout'] },
    target: 0.995,            // 99.5%
    window: '7d',
    alerts: [
      { name: 'model_errors', burnRate: 10, window: '1h', severity: 'critical', channel: 'pagerduty:platform' },
    ],
  },
  {
    name: 'Auth Service Availability',
    description: 'Authentication service is available',
    sli: { type: 'availability', service: 'cuervo-auth' },
    target: 0.9999,           // 99.99% (auth is critical path)
    window: '30d',
    alerts: [
      { name: 'auth_down', burnRate: 14.4, window: '5m', severity: 'page', channel: 'pagerduty:security' },
    ],
  },
  {
    name: 'Audit Log Completeness',
    description: 'All auditable events are logged',
    sli: { type: 'error_rate', service: 'audit-logger', error_codes: ['drop', 'failure'] },
    target: 0.9999,           // 99.99% (compliance requirement)
    window: '30d',
    alerts: [
      { name: 'audit_gap', burnRate: 50, window: '5m', severity: 'page', channel: 'pagerduty:security' },
    ],
  },
];
```

### 4.4 Alerting Rules

```yaml
# alerts/platform-alerts.yml

groups:
  - name: security_alerts
    rules:
      - alert: BruteForceDetected
        expr: rate(cuervo_auth_login_total{status="failure"}[5m]) > 10
        for: 2m
        labels:
          severity: critical
          category: security
        annotations:
          summary: "Brute force attack detected"
          description: "More than 10 failed logins/min for tenant {{ $labels.tenant }}"

      - alert: CrossTenantAttempt
        expr: cuervo_security_cross_tenant_attempts_total > 0
        labels:
          severity: critical
          category: security
        annotations:
          summary: "Cross-tenant access attempt detected"

      - alert: TokenTheftDetected
        expr: cuervo_auth_token_theft_detected_total > 0
        labels:
          severity: critical
          category: security
        annotations:
          summary: "Token theft detected for user {{ $labels.user_id }}"

  - name: operational_alerts
    rules:
      - alert: TenantQuotaExceeded
        expr: cuervo_tenant_quota_usage_ratio > 0.95
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "Tenant {{ $labels.tenant }} at {{ $value }}% quota"

      - alert: HighModelCost
        expr: cuervo_tenant_daily_cost_usd > 100
        labels:
          severity: warning
        annotations:
          summary: "Tenant {{ $labels.tenant }} daily cost ${{ $value }}"

      - alert: ModelProviderDown
        expr: cuervo_model_circuit_state == 2
        for: 5m
        labels:
          severity: critical
        annotations:
          summary: "Model provider {{ $labels.provider }} circuit OPEN"

      - alert: AuditLogGap
        expr: rate(cuervo_audit_events_dropped_total[5m]) > 0
        labels:
          severity: critical
          category: compliance
        annotations:
          summary: "Audit events being dropped - compliance risk"

  - name: cost_alerts
    rules:
      - alert: TenantDailyCostSpike
        expr: cuervo_tenant_daily_cost_usd > (cuervo_tenant_avg_daily_cost_usd * 3)
        for: 30m
        labels:
          severity: warning
        annotations:
          summary: "Tenant {{ $labels.tenant }} cost 3x above average"
```

---

## 5. Cost Monitoring Per Tenant

### 5.1 Cost Tracking Architecture

```typescript
// infrastructure/billing/cost-tracker.ts

interface CostTracker {
  /**
   * Record a cost event (model invocation, storage, API call).
   */
  recordCost(event: CostEvent): Promise<void>;

  /**
   * Get current cost for a tenant in a period.
   */
  getCurrentCost(tenantId: string, period: 'day' | 'month'): Promise<TenantCost>;

  /**
   * Check if a new operation would exceed budget.
   * Called BEFORE model invocations.
   */
  checkBudget(tenantId: string, estimatedCost: number): Promise<BudgetCheckResult>;

  /**
   * Get cost breakdown by dimension.
   */
  getCostBreakdown(tenantId: string, period: string): Promise<CostBreakdown>;
}

interface CostEvent {
  tenantId: string;
  userId: string;
  type: 'model_invocation' | 'storage' | 'connector_call' | 'data_transfer';
  amount: number;           // USD
  details: {
    model?: string;
    provider?: string;
    inputTokens?: number;
    outputTokens?: number;
    connector?: string;
    storageType?: string;
    bytes?: number;
  };
  timestamp: string;
}

interface TenantCost {
  tenantId: string;
  period: string;
  totalCost: number;
  budget: number;
  usagePercent: number;
  byCategory: Record<string, number>;
  byUser: Record<string, number>;
  byModel: Record<string, number>;
}

interface BudgetCheckResult {
  allowed: boolean;
  currentCost: number;
  budget: number;
  remainingBudget: number;
  estimatedNewTotal: number;
  /** If not allowed, why */
  reason?: 'daily_limit' | 'monthly_limit' | 'per_request_limit' | 'org_suspended';
}

interface CostBreakdown {
  tenantId: string;
  period: string;
  total: number;
  byDimension: {
    models: { model: string; provider: string; cost: number; tokens: number; invocations: number }[];
    users: { userId: string; email: string; cost: number }[];
    projects: { projectId: string; name: string; cost: number }[];
    connectors: { connectorId: string; cost: number; apiCalls: number }[];
    storage: { type: string; sizeGB: number; cost: number }[];
  };
  trends: {
    dailyCosts: { date: string; cost: number }[];
    weekOverWeekChange: number;
    projectedMonthEnd: number;
  };
}
```

---

## 6. Kill Switches

### 6.1 Kill Switch System

```typescript
// infrastructure/safety/kill-switches.ts

/**
 * Kill switches provide emergency controls to immediately stop
 * operations when security or cost thresholds are breached.
 */
interface KillSwitchService {
  /**
   * Activate a kill switch. Takes effect immediately.
   */
  activate(killSwitch: KillSwitchActivation): Promise<void>;

  /**
   * Deactivate a kill switch. Requires elevated permissions.
   */
  deactivate(killSwitchId: string, deactivatedBy: string, reason: string): Promise<void>;

  /**
   * Check if any kill switch blocks a specific operation.
   */
  check(context: { tenantId: string; operation: string }): Promise<KillSwitchCheck>;

  /**
   * List all active kill switches.
   */
  listActive(tenantId?: string): Promise<KillSwitch[]>;
}

interface KillSwitch {
  id: string;
  type: KillSwitchType;
  scope: 'global' | 'tenant' | 'user' | 'model' | 'connector';
  scopeId: string | null;     // null for global
  reason: string;
  activatedBy: string;
  activatedAt: string;
  expiresAt: string | null;   // null = manual deactivation required
  autoTrigger: AutoTriggerConfig | null;
}

type KillSwitchType =
  | 'model_invocation_halt'     // Stop all model calls
  | 'tenant_suspend'            // Suspend entire tenant
  | 'user_suspend'              // Suspend specific user
  | 'connector_disable'         // Disable specific connector
  | 'tool_disable'              // Disable specific tool
  | 'write_operations_halt'     // Read-only mode
  | 'external_calls_halt'       // Block all outbound traffic
  | 'plugin_disable_all'        // Disable all plugins
  | 'cost_limit_enforce'        // Hard-stop on cost limit
  | 'rate_limit_emergency';     // Emergency rate limit reduction

interface AutoTriggerConfig {
  condition: string;           // Prometheus-style expression
  threshold: number;
  window: string;              // '5m', '1h'
  cooldown: string;            // Minimum time between activations
}

interface KillSwitchCheck {
  blocked: boolean;
  activeKillSwitches: {
    id: string;
    type: KillSwitchType;
    reason: string;
    activatedAt: string;
  }[];
}

// Auto-triggered kill switches
const AUTO_KILL_SWITCHES: AutoTriggerConfig[] = [
  {
    // Stop model calls if cost spike detected
    condition: 'cuervo_tenant_hourly_cost_usd',
    threshold: 50,              // $50/hour = something is wrong
    window: '1h',
    cooldown: '30m',
  },
  {
    // Suspend tenant if brute force detected
    condition: 'rate(cuervo_auth_login_total{status="failure"}[5m])',
    threshold: 100,             // 100 failures in 5 min
    window: '5m',
    cooldown: '1h',
  },
  {
    // Halt writes if audit logging is down
    condition: 'up{job="audit-logger"}',
    threshold: 0,
    window: '1m',
    cooldown: '5m',
  },
];
```

---

## 7. Data Residency & Encryption

### 7.1 Data Residency Configuration

```typescript
// infrastructure/compliance/data-residency.ts

interface DataResidencyConfig {
  /** Available regions */
  regions: DataRegion[];

  /** Default region for new tenants */
  defaultRegion: string;

  /** Region-specific infrastructure */
  infrastructure: Record<string, RegionInfrastructure>;
}

interface DataRegion {
  id: string;              // 'us-east', 'eu-west', 'latam-east'
  name: string;
  country: string;         // Primary country
  compliance: string[];    // ['gdpr', 'soc2']
  available: boolean;
}

interface RegionInfrastructure {
  database: {
    host: string;
    replicas: string[];
  };
  vectorDb: {
    host: string;
  };
  cache: {
    host: string;
  };
  objectStorage: {
    bucket: string;
    region: string;
  };
  kms: {
    keyArn: string;
  };
}

// Data residency regions
const DATA_REGIONS: DataRegion[] = [
  { id: 'us-east', name: 'US East (Virginia)', country: 'US', compliance: ['soc2', 'hipaa'], available: true },
  { id: 'eu-west', name: 'EU West (Ireland)', country: 'IE', compliance: ['gdpr', 'soc2'], available: true },
  { id: 'eu-central', name: 'EU Central (Frankfurt)', country: 'DE', compliance: ['gdpr', 'soc2'], available: true },
  { id: 'latam-east', name: 'LATAM East (São Paulo)', country: 'BR', compliance: ['lgpd', 'soc2'], available: true },
  { id: 'ap-southeast', name: 'Asia Pacific (Singapore)', country: 'SG', compliance: ['pdpa', 'soc2'], available: false },
];
```

---

## 8. Disaster Recovery

### 8.1 Backup Strategy

| Data Class | RPO | RTO | Backup Method | Frequency | Retention |
|-----------|-----|-----|--------------|-----------|-----------|
| **Audit logs** | 0 (no loss) | 4h | Streaming replication + WAL archiving | Continuous | 7 years |
| **User data** | 1h | 4h | pg_dump + WAL | Hourly | 30 days (daily), 1 year (weekly) |
| **Tenant config** | 1h | 2h | pg_dump + git-tracked | Hourly | 90 days |
| **Embeddings** | 24h | 24h | Full snapshot | Daily | 7 days |
| **Object storage** | 1h | 4h | Cross-region replication | Continuous | 90 days |
| **Encryption keys** | 0 | 1h | KMS auto-replication | Continuous | Indefinite |
| **Redis cache** | N/A | 15m | No backup (ephemeral) | N/A | N/A |

### 8.2 DRP Procedures

```typescript
// infrastructure/disaster-recovery/drp.ts

interface DisasterRecoveryPlan {
  /** Incident classification */
  incidents: {
    level1: 'Single service degradation';     // RTO: 1h
    level2: 'Multi-service outage';           // RTO: 4h
    level3: 'Complete region failure';         // RTO: 8h
    level4: 'Data breach / security incident'; // Immediate response
  };

  /** Recovery procedures */
  procedures: {
    database_failover: {
      trigger: 'Primary PostgreSQL unreachable for >5 minutes';
      steps: [
        '1. Verify primary is actually down (not network issue)',
        '2. Promote read replica to primary',
        '3. Update connection strings via config management',
        '4. Verify data consistency (check WAL position)',
        '5. Notify affected tenants',
        '6. Begin investigation of root cause',
      ];
      estimatedTime: '15-30 minutes';
      dataLossRisk: 'Minimal (async replication lag, typically <1s)';
    };

    region_failover: {
      trigger: 'Entire region unreachable for >15 minutes';
      steps: [
        '1. Activate DNS failover to secondary region',
        '2. Verify secondary databases are caught up',
        '3. Warm up caches in secondary region',
        '4. Redirect traffic via load balancer',
        '5. Verify service health in secondary region',
        '6. Communicate status to all tenants',
      ];
      estimatedTime: '1-4 hours';
      dataLossRisk: 'Possible loss of up to 1h of data (cross-region replication lag)';
    };

    data_breach_response: {
      trigger: 'Confirmed unauthorized data access';
      steps: [
        '1. ACTIVATE KILL SWITCH: external_calls_halt',
        '2. Isolate affected systems',
        '3. Preserve forensic evidence (do NOT restart services)',
        '4. Notify security team + legal',
        '5. Assess scope of breach (which tenants, which data)',
        '6. Rotate all affected credentials',
        '7. Notify affected tenants within 72 hours (GDPR)',
        '8. Notify supervisory authority if EU data affected',
        '9. Engage incident response retainer',
        '10. Post-incident review within 7 days',
      ];
      estimatedTime: 'Immediate + ongoing';
      legalRequirements: [
        'GDPR Art. 33: Notify supervisory authority within 72 hours',
        'GDPR Art. 34: Notify data subjects if high risk',
        'LGPD Art. 48: Notify ANPD in reasonable time',
        'SOC 2: Document incident and remediation',
      ];
    };
  };

  /** Testing schedule */
  testing: {
    tabletop: 'Quarterly';
    failover_drill: 'Semi-annually';
    full_dr_test: 'Annually';
    backup_restoration: 'Monthly (random sample)';
  };
}
```

---

## 9. Compliance Framework Mapping

### 9.1 SOC 2 Control Mapping

| Trust Service Criteria | Cuervo Control | Evidence |
|----------------------|----------------|---------|
| CC1.1 COSO Integrity | Code of conduct, security policy | Policy documents, training records |
| CC2.1 Internal communications | Structured logging, alerting | Log aggregation, alert configs |
| CC3.1 Risk assessment | Threat model, annual pentest | Threat model doc, pentest reports |
| CC4.1 Monitoring | SLOs, metrics, alerting | Grafana dashboards, PagerDuty |
| CC5.1 Control activities | RBAC, least privilege | Authorization engine, role configs |
| CC6.1 Logical access | JWT + MFA + RBAC + ABAC | Auth service, audit logs |
| CC6.2 Access provisioning | SCIM, JIT provisioning | SCIM sync logs, user lifecycle |
| CC6.3 Access removal | SCIM deprovisioning, token revocation | Audit trail of deprovisioning |
| CC6.6 Encryption | AES-256-GCM at rest, TLS 1.3 transit | Encryption service, TLS configs |
| CC6.7 Transmission security | TLS 1.3 everywhere | Certificate management, HSTS |
| CC6.8 Vulnerability mgmt | Dependency scanning, pentests | Snyk reports, pentest schedule |
| CC7.1 Incident detection | Anomaly detection, alerts | Security monitoring, kill switches |
| CC7.2 Incident response | DRP, runbooks | DRP document, drill reports |
| CC7.3 Incident communication | Status page, tenant notification | Communication templates |
| CC8.1 Change management | Git-based, PR review, CI/CD | Git history, deployment logs |
| A1.2 Recovery | Backups, failover | Backup reports, DR drill results |

### 9.2 EU AI Act Compliance

```typescript
// infrastructure/compliance/eu-ai-act.ts

interface EUAIActCompliance {
  /** System classification */
  classification: {
    /** Cuervo CLI core: Limited Risk (Art. 52) */
    core: 'limited_risk';
    /** Cuervo in hiring/HR context: Could be High Risk (Annex III) */
    hrContext: 'high_risk';
    /** GPAI obligations if training own models (Art. 53) */
    gpaiApplicable: boolean;
  };

  /** Transparency requirements (Art. 52) */
  transparency: {
    /** Users must know they interact with AI */
    aiDisclosure: 'Banner shown on every session start';
    /** AI-generated content must be labeled */
    contentLabeling: 'All AI outputs prefixed with source model';
    /** Model information available */
    modelTransparency: 'Model name, provider, version shown per response';
  };

  /** Technical documentation (Art. 11) */
  documentation: {
    /** System description */
    systemDescription: string;
    /** Risk management system */
    riskManagement: string;
    /** Data governance */
    dataGovernance: string;
    /** Technical specifications */
    technicalSpecs: string;
    /** Performance metrics */
    performanceMetrics: string;
  };

  /** Explainability (for high-risk deployments) */
  explainability: {
    /** Decision logging */
    decisionLogging: 'All agent decisions logged with reasoning traces';
    /** Attribution */
    attribution: 'Source files cited for code suggestions';
    /** Confidence scoring */
    confidenceScoring: 'Confidence level per suggestion (when available from model)';
    /** Human override */
    humanOverride: 'Destructive operations require explicit approval';
  };

  /** Incident reporting (Art. 62) */
  incidentReporting: {
    /** Report serious incidents to authorities */
    reportingProcess: string;
    /** Timeline: without undue delay, within 15 days of awareness */
    timeline: '15 days';
    /** Who to report to */
    authority: 'National market surveillance authority of member state';
  };
}
```

---

## 10. Anomaly Detection

```typescript
// infrastructure/security/anomaly-detection.ts

interface AnomalyDetectionService {
  /**
   * Evaluate a user action for anomalous behavior.
   * Uses rule-based + statistical detection.
   */
  evaluate(action: UserAction): Promise<AnomalyResult>;

  /**
   * Train baseline for a user/tenant (statistical model).
   */
  trainBaseline(tenantId: string, userId: string): Promise<void>;
}

interface UserAction {
  userId: string;
  tenantId: string;
  action: string;
  timestamp: string;
  ipAddress: string;
  deviceId: string;
  geoLocation: { country: string; city: string; lat: number; lon: number } | null;
  metadata: Record<string, unknown>;
}

interface AnomalyResult {
  riskScore: number;        // 0.0 - 1.0
  anomalies: AnomalyFlag[];
  recommendation: 'allow' | 'challenge_mfa' | 'block' | 'alert_admin';
}

interface AnomalyFlag {
  type: AnomalyType;
  description: string;
  confidence: number;
  severity: 'low' | 'medium' | 'high' | 'critical';
}

type AnomalyType =
  | 'impossible_travel'        // Login from two distant locations in short time
  | 'unusual_time'             // Login outside normal hours
  | 'new_device'               // First time device
  | 'new_ip_range'             // IP from unusual range
  | 'unusual_model_usage'      // Sudden spike in model invocations
  | 'cost_spike'               // Cost significantly above baseline
  | 'bulk_data_access'         // Accessing many files rapidly
  | 'privilege_escalation'     // Attempting admin operations without history
  | 'api_abuse'                // Unusual API call patterns
  | 'exfiltration_pattern';    // Large outbound data transfers

// Detection rules
const ANOMALY_RULES = [
  {
    type: 'impossible_travel' as const,
    check: (current: UserAction, previous: UserAction) => {
      if (!current.geoLocation || !previous.geoLocation) return null;
      const distanceKm = haversineDistance(current.geoLocation, previous.geoLocation);
      const timeDiffHours = (new Date(current.timestamp).getTime() - new Date(previous.timestamp).getTime()) / 3600000;
      const maxSpeedKmh = 1000; // Roughly max commercial flight speed
      if (distanceKm / timeDiffHours > maxSpeedKmh) {
        return { confidence: 0.9, severity: 'high' as const };
      }
      return null;
    },
  },
  {
    type: 'cost_spike' as const,
    check: (dailyCost: number, avgDailyCost: number) => {
      if (dailyCost > avgDailyCost * 5 && dailyCost > 10) {
        return { confidence: 0.8, severity: 'high' as const };
      }
      return null;
    },
  },
];
```

---

## 11. Decisiones Arquitectónicas

| # | Decisión | Alternativa Descartada | Justificación |
|---|----------|----------------------|---------------|
| ADR-S01 | **Hash-chain audit log (append-only)** | Mutable audit log with access controls | Tamper-evidence es requisito SOC 2. Hash chain permite verificación independiente. PostgreSQL trigger prevents modification. |
| ADR-S02 | **PII redaction at logging layer** | PII redaction at query time | Prevent PII from ever being stored in logs. Retroactive redaction is expensive and error-prone. |
| ADR-S03 | **OpenTelemetry for tracing** | Vendor-specific (Datadog, New Relic) | Vendor-neutral, wide ecosystem, avoids lock-in. Can export to any backend. |
| ADR-S04 | **Prometheus metrics + Grafana** | Cloud-native monitoring (CloudWatch, etc.) | Self-hostable (required for enterprise), well-understood, ecosystem. Cloud providers can consume Prometheus metrics. |
| ADR-S05 | **Kill switches with auto-triggers** | Manual-only emergency controls | Auto-triggers catch issues faster than humans. Cost spikes, audit failures, and brute force need immediate response. Manual deactivation prevents false-positive lockout. |
| ADR-S06 | **Data residency at database level** | Application-level routing | Database-level separation is more secure, easier to audit, and required by some regulations. Overhead is acceptable for enterprise tier. |
| ADR-S07 | **Rule-based anomaly detection** | ML-based anomaly detection | Rule-based is deterministic, explainable, and sufficient for known patterns. ML can be added later as enhancement but introduces complexity and false positive risk. |

---

## 12. Plan de Implementación (Security & Compliance)

| Sprint | Entregable | Dependencias |
|--------|-----------|-------------|
| S1-S2 | Audit event schema + append-only storage + hash chain | Database setup |
| S3-S4 | Structured logging + PII redaction pipeline | Audit schema |
| S5-S6 | Prometheus metrics instrumentation | Application services |
| S7-S8 | OpenTelemetry tracing integration | Application services |
| S9-S10 | SLO definitions + alerting rules | Metrics |
| S11-S12 | Cost tracking + budget enforcement | Billing infrastructure |
| S13-S14 | Kill switch system + auto-triggers | Metrics, cost tracking |
| S15-S16 | Anomaly detection (rule-based) | Audit events, metrics |
| S17-S18 | Audit export + SIEM integration | Audit storage |
| S19-S20 | Compliance report generation (SOC 2, GDPR) | All audit data |
| S21-S22 | Data residency implementation | Multi-region infrastructure |
| S23-S24 | Disaster recovery setup + testing | Backup infrastructure |
| S25-S26 | EU AI Act transparency implementation | Frontend changes |
| S27-S28 | Penetration testing + security audit | All systems deployed |
