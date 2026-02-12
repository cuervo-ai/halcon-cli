# FASE 2 — Identidad, Login y Autorización (IAM)

> **Versión**: 1.0.0 | **Fecha**: 2026-02-06
> **Autores**: Security/IAM Architecture Team
> **Estado**: Design Complete — Ready for Implementation Review

---

## 1. Visión General

El sistema IAM de Cuervo implementa un modelo **Zero Trust** completo donde ningún actor (usuario, servicio, agente, plugin) tiene acceso implícito a ningún recurso. Cada request se autentica, autoriza, y audita independientemente. El sistema escala desde un desarrollador individual (sin auth, API keys locales) hasta organizaciones enterprise con miles de usuarios, SSO corporativo, provisioning automático, y cumplimiento regulatorio estricto.

### 1.1 Principios Zero Trust

| Principio | Implementación |
|-----------|---------------|
| **Never trust, always verify** | Cada API call requiere token válido; tokens son efímeros (15min) |
| **Least privilege** | RBAC + ABAC; permisos granulares por recurso, acción y contexto |
| **Assume breach** | Segmentación por tenant, encryption E2E, audit completo, anomaly detection |
| **Verify explicitly** | MFA, device trust, IP reputation, behavioral analysis |
| **Limit blast radius** | Service accounts con scope mínimo, token revocation instantánea, session isolation |

### 1.2 Modos de Operación

```
┌────────────────────────────────────────────────────────────────────────┐
│                        IAM OPERATING MODES                             │
├────────────────────────────────────────────────────────────────────────┤
│                                                                        │
│  MODE 1: LOCAL (Standalone CLI)                                        │
│  ┌──────────────────────────────────────────────┐                      │
│  │  • No auth server required                    │                      │
│  │  • API keys in OS keychain                    │                      │
│  │  • Implicit full-access (single user)         │                      │
│  │  • Tools permission via .cuervo/config.yml    │                      │
│  │  • No network dependency                      │                      │
│  └──────────────────────────────────────────────┘                      │
│                                                                        │
│  MODE 2: CLOUD (SaaS / Hybrid)                                         │
│  ┌──────────────────────────────────────────────┐                      │
│  │  • cuervo-auth-service (JWT + RBAC)           │                      │
│  │  • OAuth2/OIDC login via browser              │                      │
│  │  • Device authorization for CLI               │                      │
│  │  • Short-lived access tokens (15min)          │                      │
│  │  • Refresh tokens (7 days, rotated)           │                      │
│  │  • MFA enforcement (org policy)               │                      │
│  └──────────────────────────────────────────────┘                      │
│                                                                        │
│  MODE 3: ENTERPRISE (Self-hosted / SSO)                                │
│  ┌──────────────────────────────────────────────┐                      │
│  │  • All of Cloud mode +                        │                      │
│  │  • SAML 2.0 / OIDC with corporate IdP        │                      │
│  │  • SCIM 2.0 provisioning                      │                      │
│  │  • Custom RBAC + ABAC policies                │                      │
│  │  • Customer-managed encryption keys           │                      │
│  │  • IP allowlisting, device trust              │                      │
│  │  • WebAuthn/Passkeys, hardware tokens         │                      │
│  │  • Conditional access policies                │                      │
│  └──────────────────────────────────────────────┘                      │
│                                                                        │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Modelo de Identidad

### 2.1 Tipos de Identidad (Principals)

```typescript
// domain/entities/identity.ts

/**
 * Discriminated union of all identity types in the system.
 * Every action is performed by a Principal.
 */
type Principal =
  | UserPrincipal
  | ServiceAccountPrincipal
  | AgentPrincipal
  | SystemPrincipal;

interface UserPrincipal {
  readonly type: 'user';
  readonly id: string;           // UUID
  readonly tenantId: string;
  readonly email: string;
  readonly roles: Role[];
  readonly attributes: UserAttributes;
  readonly authMethod: AuthMethod;
  readonly sessionId: string;
  readonly deviceId: string;
  readonly mfaVerified: boolean;
  readonly ipAddress: string;
  readonly lastAuthenticatedAt: string;
}

interface ServiceAccountPrincipal {
  readonly type: 'service_account';
  readonly id: string;
  readonly tenantId: string;
  readonly name: string;
  readonly scopes: string[];      // Fine-grained OAuth2 scopes
  readonly createdBy: string;     // User who created this SA
  readonly expiresAt: string | null;
  readonly ipAllowlist: string[] | null;
}

interface AgentPrincipal {
  readonly type: 'agent';
  readonly id: string;
  readonly tenantId: string;
  readonly agentType: 'explorer' | 'planner' | 'executor' | 'reviewer';
  readonly parentSessionId: string;
  readonly delegatedBy: string;   // User who initiated the session
  readonly permissions: AgentPermission[];
  readonly toolAllowlist: string[];
  readonly tokenBudget: number;
}

interface SystemPrincipal {
  readonly type: 'system';
  readonly id: 'cuervo-system';
  readonly component: string;     // Which system component (scheduler, sync, etc.)
}

type AuthMethod =
  | 'password'
  | 'oauth2_google'
  | 'oauth2_github'
  | 'oidc'
  | 'saml'
  | 'passkey'
  | 'api_key'
  | 'device_code'
  | 'refresh_token';
```

### 2.2 User Entity (Full)

```typescript
// domain/entities/user.ts

interface User {
  readonly id: string;             // UUID v7 (time-sortable)
  readonly tenantId: string;

  /** Identity */
  email: string;
  emailVerified: boolean;
  displayName: string;
  avatarUrl: string | null;
  locale: string;                  // BCP 47 (e.g., 'es-MX', 'pt-BR')
  timezone: string;                // IANA (e.g., 'America/Mexico_City')

  /** Authentication */
  passwordHash: string | null;     // bcrypt(12) — null if SSO-only
  mfaMethods: MFAMethod[];
  passkeys: PasskeyCredential[];
  lastPasswordChangeAt: string | null;
  passwordExpiresAt: string | null;
  failedLoginAttempts: number;
  lockedUntil: string | null;

  /** Authorization */
  roles: UserRoleBinding[];
  directPermissions: PermissionGrant[];

  /** External identity links */
  externalIdentities: ExternalIdentity[];

  /** Provisioning */
  provisionedVia: 'manual' | 'scim' | 'jit_sso' | 'invitation';
  scimExternalId: string | null;

  /** Lifecycle */
  status: 'active' | 'suspended' | 'deprovisioned' | 'pending_activation';
  createdAt: string;
  updatedAt: string;
  lastLoginAt: string | null;
  deactivatedAt: string | null;
  deactivatedBy: string | null;
  deactivationReason: string | null;
}

interface MFAMethod {
  id: string;
  type: 'totp' | 'webauthn' | 'sms' | 'email' | 'recovery_codes';
  verified: boolean;
  enrolledAt: string;
  lastUsedAt: string | null;
  /** TOTP-specific */
  totpSecret?: string;        // Encrypted
  /** WebAuthn-specific */
  credentialId?: string;
  publicKey?: string;
  /** Recovery codes */
  hashedCodes?: string[];     // bcrypt hashed
  usedCodes?: number;
}

interface PasskeyCredential {
  id: string;
  credentialId: string;       // Base64url
  publicKey: string;          // COSE key, base64
  counter: number;            // Signature counter for clone detection
  transports: ('usb' | 'ble' | 'nfc' | 'internal')[];
  deviceName: string;
  createdAt: string;
  lastUsedAt: string | null;
  backedUp: boolean;
}

interface ExternalIdentity {
  provider: string;           // 'google', 'github', 'okta', 'azure_ad', etc.
  externalId: string;         // Provider's user ID
  email: string;
  profile: Record<string, unknown>;
  linkedAt: string;
  lastSyncedAt: string;
}
```

### 2.3 Organization & Team Model

```typescript
// domain/entities/organization.ts

interface Organization {
  readonly id: string;
  readonly slug: string;

  name: string;
  plan: OrgPlan;
  billingEmail: string;

  /** Security settings */
  securityPolicy: OrgSecurityPolicy;

  /** SSO Configuration */
  ssoConfig: SSOConfiguration | null;

  /** SCIM Configuration */
  scimConfig: SCIMConfiguration | null;

  /** Members & Teams */
  memberCount: number;
  teamCount: number;

  /** Lifecycle */
  status: 'active' | 'suspended' | 'trial' | 'cancelled';
  createdAt: string;
  trialEndsAt: string | null;
}

type OrgPlan = 'free' | 'team' | 'business' | 'enterprise';

interface OrgSecurityPolicy {
  /** Authentication requirements */
  mfaRequired: boolean;
  mfaGracePeriodDays: number;          // Days before MFA enforcement
  allowedAuthMethods: AuthMethod[];
  ssoEnforced: boolean;                 // Disable password if true

  /** Password policy (if passwords allowed) */
  passwordPolicy: {
    minLength: number;                  // Default: 12
    requireUppercase: boolean;
    requireLowercase: boolean;
    requireNumbers: boolean;
    requireSpecialChars: boolean;
    maxAgeDays: number;                 // 0 = no expiry
    historyCount: number;              // Prevent reuse of last N passwords
    preventCommonPasswords: boolean;
  };

  /** Session policy */
  sessionPolicy: {
    maxDurationMinutes: number;         // Default: 720 (12h)
    idleTimeoutMinutes: number;         // Default: 60
    maxConcurrentSessions: number;      // Default: 5
    requireReauthForSensitive: boolean; // Step-up auth
    bindToIP: boolean;                  // Session invalidated on IP change
    bindToDevice: boolean;              // Session bound to device fingerprint
  };

  /** Network policy */
  networkPolicy: {
    ipAllowlist: string[] | null;       // CIDR ranges
    ipBlocklist: string[];
    geoRestrictions: string[] | null;   // ISO 3166-1 alpha-2 country codes
    requireVPN: boolean;
    vpnCIDRs: string[];
  };

  /** Data policy */
  dataPolicy: {
    dataResidency: 'us' | 'eu' | 'latam' | 'ap';
    zeroRetention: boolean;
    customerManagedKeys: boolean;
    auditLogRetentionDays: number;
    allowDataExport: boolean;
    allowedModelProviders: string[] | null;
    blockedModelProviders: string[];
  };

  /** Conditional access rules */
  conditionalAccess: ConditionalAccessRule[];
}

interface ConditionalAccessRule {
  id: string;
  name: string;
  enabled: boolean;
  /** Conditions that trigger this rule */
  conditions: {
    userGroups?: string[];
    ipRanges?: string[];
    deviceTrust?: 'any' | 'managed' | 'compliant';
    riskLevel?: 'low' | 'medium' | 'high';
    locations?: string[];             // Country codes
    timeWindows?: TimeWindow[];
  };
  /** Actions when conditions match */
  actions: {
    grant: 'allow' | 'deny' | 'require_mfa' | 'require_approval';
    sessionDuration?: number;
    restrictedScopes?: string[];
    notifyAdmins?: boolean;
  };
  priority: number;
}

interface TimeWindow {
  daysOfWeek: number[];  // 0=Sun, 6=Sat
  startHour: number;     // 0-23
  endHour: number;       // 0-23
  timezone: string;
}

/** Teams within an organization */
interface Team {
  readonly id: string;
  readonly orgId: string;

  name: string;
  slug: string;
  description: string | null;

  /** Team-level permissions */
  defaultRole: string;
  permissions: PermissionGrant[];

  /** Projects accessible to this team */
  projectBindings: TeamProjectBinding[];

  memberCount: number;
  createdAt: string;
  createdBy: string;
}

interface TeamProjectBinding {
  projectId: string;
  role: string;           // Team's role on this project
  grantedAt: string;
  grantedBy: string;
}
```

---

## 3. RBAC + ABAC Authorization Model

### 3.1 Role Hierarchy

```
┌──────────────────────────────────────────────────────────────────────┐
│                        ROLE HIERARCHY                                │
├──────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ORGANIZATION LEVEL                                                  │
│  ┌─────────────────────────────────────────────┐                     │
│  │  org:owner                                   │                     │
│  │  ├── org:admin                               │                     │
│  │  │   ├── org:member                          │                     │
│  │  │   │   ├── org:viewer                      │                     │
│  │  │   │   └── org:billing_admin               │                     │
│  │  │   └── org:security_admin                  │                     │
│  │  └── org:service_account_admin               │                     │
│  └─────────────────────────────────────────────┘                     │
│                                                                      │
│  PROJECT LEVEL                                                       │
│  ┌─────────────────────────────────────────────┐                     │
│  │  project:admin                               │                     │
│  │  ├── project:maintainer                      │                     │
│  │  │   ├── project:developer                   │                     │
│  │  │   │   └── project:viewer                  │                     │
│  │  │   └── project:ci_bot                      │                     │
│  │  └── project:security_reviewer               │                     │
│  └─────────────────────────────────────────────┘                     │
│                                                                      │
│  PLATFORM LEVEL (Internal)                                           │
│  ┌─────────────────────────────────────────────┐                     │
│  │  platform:super_admin                        │                     │
│  │  ├── platform:support                        │                     │
│  │  ├── platform:billing                        │                     │
│  │  └── platform:security                       │                     │
│  └─────────────────────────────────────────────┘                     │
│                                                                      │
└──────────────────────────────────────────────────────────────────────┘
```

### 3.2 Permission Model

```typescript
// domain/entities/authorization.ts

/**
 * Permission is a triple: resource_type:action:scope
 * Examples:
 *   "project:read:*"
 *   "model:invoke:claude-sonnet-4-5"
 *   "tool:execute:bash"
 *   "org:manage:members"
 */
interface Permission {
  /** Resource type (project, model, tool, org, billing, audit, etc.) */
  resource: string;
  /** Action on the resource */
  action: string;
  /** Scope/target (specific ID, wildcard, or pattern) */
  scope: string;
}

interface Role {
  id: string;
  name: string;
  description: string;
  level: 'org' | 'project' | 'platform';
  /** Is this a built-in role (non-deletable) */
  builtIn: boolean;
  /** Permissions granted by this role */
  permissions: Permission[];
  /** Roles this role inherits from */
  inherits: string[];
}

interface UserRoleBinding {
  userId: string;
  roleId: string;
  /** Scope of the binding */
  scope: {
    type: 'org' | 'project' | 'team';
    id: string;
  };
  grantedAt: string;
  grantedBy: string;
  expiresAt: string | null;  // Temporary role grants
  condition: RoleCondition | null;
}

/**
 * ABAC: Attribute-based conditions on role bindings.
 * Role is active only when conditions are met.
 */
interface RoleCondition {
  /** Time-based restrictions */
  timeWindow?: TimeWindow;
  /** IP-based restrictions */
  ipRanges?: string[];
  /** Device trust level */
  deviceTrust?: 'any' | 'managed' | 'compliant';
  /** MFA must be verified */
  requireMFA?: boolean;
  /** Risk score threshold */
  maxRiskScore?: number;
}
```

### 3.3 Permission Tables (Built-in Roles)

#### Organization Roles

| Permission | owner | admin | security_admin | member | viewer | billing_admin |
|-----------|:-----:|:-----:|:--------------:|:------:|:------:|:-------------:|
| org:manage:settings | **W** | **W** | R | R | R | R |
| org:manage:members | **W** | **W** | R | - | - | - |
| org:manage:teams | **W** | **W** | R | R | - | - |
| org:manage:security | **W** | **W** | **W** | - | - | - |
| org:manage:sso | **W** | **W** | **W** | - | - | - |
| org:manage:scim | **W** | **W** | **W** | - | - | - |
| org:manage:billing | **W** | **W** | - | - | - | **W** |
| org:view:audit_logs | **W** | **W** | **W** | - | - | - |
| org:export:audit_logs | **W** | - | **W** | - | - | - |
| org:manage:service_accounts | **W** | **W** | - | - | - | - |
| org:manage:api_keys | **W** | **W** | **W** | - | - | - |
| org:view:usage | **W** | **W** | R | R | R | **W** |
| org:manage:integrations | **W** | **W** | R | - | - | - |
| org:delete | **W** | - | - | - | - | - |

#### Project Roles

| Permission | admin | maintainer | developer | viewer | ci_bot | security_reviewer |
|-----------|:-----:|:---------:|:---------:|:------:|:------:|:-----------------:|
| project:read:code | **R** | **R** | **R** | **R** | **R** | **R** |
| project:write:code | **W** | **W** | **W** | - | **W** | - |
| project:delete:code | **W** | **W** | - | - | - | - |
| project:manage:settings | **W** | **W** | - | - | - | - |
| project:manage:members | **W** | **W** | - | - | - | - |
| project:manage:integrations | **W** | **W** | - | - | - | - |
| model:invoke:* | **W** | **W** | **W** | - | **W** | **W** |
| model:invoke:expensive | **W** | **W** | - | - | - | - |
| tool:execute:read_only | **W** | **W** | **W** | **W** | **W** | **W** |
| tool:execute:write | **W** | **W** | **W** | - | **W** | - |
| tool:execute:destructive | **W** | **W** | - | - | - | - |
| tool:execute:bash | **W** | **W** | **W** | - | **W** | - |
| agent:spawn:* | **W** | **W** | **W** | - | **W** | **W** |
| agent:spawn:multi | **W** | **W** | - | - | - | - |
| audit:view:project | **W** | **W** | R | - | - | **W** |
| security:review:code | **W** | **W** | - | - | - | **W** |

### 3.4 ABAC Policy Engine

```typescript
// domain/services/authorization-engine.ts

interface AuthorizationEngine {
  /**
   * Evaluate whether a principal can perform an action on a resource.
   * Combines RBAC (roles) + ABAC (attributes/conditions) + org policies.
   */
  evaluate(request: AuthorizationRequest): Promise<AuthorizationDecision>;

  /**
   * Batch evaluation for multiple permissions (e.g., UI rendering).
   */
  evaluateBatch(
    principal: Principal,
    requests: { resource: string; action: string; scope: string }[],
  ): Promise<Map<string, AuthorizationDecision>>;

  /**
   * Get effective permissions for a principal (for UI/debugging).
   */
  getEffectivePermissions(principal: Principal): Promise<EffectivePermissions>;
}

interface AuthorizationRequest {
  principal: Principal;
  resource: {
    type: string;         // 'project', 'model', 'tool', etc.
    id: string;           // Specific resource ID
    tenantId: string;     // Tenant owning the resource
  };
  action: string;         // 'read', 'write', 'invoke', 'execute', etc.
  context: {
    ipAddress: string;
    deviceId: string;
    deviceTrust: 'unknown' | 'managed' | 'compliant';
    timestamp: string;
    riskScore: number;    // 0.0 - 1.0
    mfaVerified: boolean;
    sessionAge: number;   // minutes
  };
}

interface AuthorizationDecision {
  allowed: boolean;
  reason: string;
  /** Which policy/role granted or denied */
  decidedBy: {
    type: 'role' | 'abac_policy' | 'org_policy' | 'conditional_access' | 'explicit_deny';
    id: string;
    name: string;
  };
  /** Conditions that must be met (e.g., step-up auth) */
  conditions?: {
    requireMFA?: boolean;
    requireApproval?: boolean;
    approverRoles?: string[];
    maxDuration?: number;
  };
  /** Audit metadata */
  evaluationId: string;
  evaluatedAt: string;
  durationUs: number;
}

interface EffectivePermissions {
  principal: { type: string; id: string };
  permissions: {
    resource: string;
    action: string;
    scope: string;
    source: string;       // Role or policy that grants this
    conditions: RoleCondition | null;
  }[];
  denials: {
    resource: string;
    action: string;
    scope: string;
    reason: string;
  }[];
}
```

### 3.5 Policy Evaluation Order

```
┌─────────────────────────────────────────────────────┐
│             POLICY EVALUATION PIPELINE               │
├─────────────────────────────────────────────────────┤
│                                                      │
│  1. EXPLICIT DENY (org-level blocklist)              │
│     └── If matched → DENY (short-circuit)            │
│                                                      │
│  2. CONDITIONAL ACCESS RULES                         │
│     └── Evaluate conditions (IP, device, time, risk) │
│     └── If deny rule matches → DENY                  │
│     └── If require_mfa → check MFA status            │
│                                                      │
│  3. ORG SECURITY POLICY                              │
│     └── Check org-wide restrictions                  │
│     └── Model allowlist/blocklist                    │
│     └── Tool restrictions                            │
│                                                      │
│  4. RBAC ROLE CHECK                                  │
│     └── Resolve user roles (org + project + team)    │
│     └── Expand role inheritance                      │
│     └── Check permission match                       │
│                                                      │
│  5. ABAC CONDITION CHECK                             │
│     └── For matching roles with conditions           │
│     └── Evaluate time, IP, device, MFA conditions    │
│                                                      │
│  6. DEFAULT DENY                                     │
│     └── If no explicit allow → DENY                  │
│                                                      │
│  Result: ALLOW | DENY | REQUIRE_STEP_UP              │
│                                                      │
└─────────────────────────────────────────────────────┘
```

---

## 4. Flujos de Autenticación

### 4.1 Flujo CLI — Device Authorization (RFC 8628)

```
┌──────────┐                    ┌──────────────┐              ┌─────────────┐
│  CLI     │                    │ Auth Service  │              │  Browser    │
│  (cuervo │                    │ (cuervo-auth) │              │  (User)     │
│   login) │                    │               │              │             │
└────┬─────┘                    └──────┬────────┘              └──────┬──────┘
     │                                 │                              │
     │  1. POST /oauth/device/code     │                              │
     │  { client_id, scope }           │                              │
     │────────────────────────────────>│                              │
     │                                 │                              │
     │  2. { device_code,              │                              │
     │       user_code: "ABCD-1234",   │                              │
     │       verification_uri,         │                              │
     │       expires_in: 900,          │                              │
     │       interval: 5 }             │                              │
     │<────────────────────────────────│                              │
     │                                 │                              │
     │  3. Display to user:            │                              │
     │  "Visit https://cuervo.dev/     │                              │
     │   activate                      │                              │
     │   Enter code: ABCD-1234"        │                              │
     │  (+ auto-open browser)          │                              │
     │                                 │     4. User visits URL       │
     │                                 │<─────────────────────────────│
     │                                 │                              │
     │                                 │     5. User enters code      │
     │                                 │<─────────────────────────────│
     │                                 │                              │
     │                                 │     6. OAuth2 login flow     │
     │                                 │     (password/SSO/passkey)   │
     │                                 │<────────────────────────────>│
     │                                 │                              │
     │                                 │     7. MFA challenge         │
     │                                 │     (if required)            │
     │                                 │<────────────────────────────>│
     │                                 │                              │
     │                                 │     8. "Device authorized"   │
     │                                 │─────────────────────────────>│
     │                                 │                              │
     │  9. Poll POST /oauth/token      │                              │
     │  { device_code, grant_type:     │                              │
     │    device_code }                │                              │
     │────────────────────────────────>│                              │
     │                                 │                              │
     │  10. { access_token (JWT),      │                              │
     │        refresh_token,           │                              │
     │        expires_in: 900,         │                              │
     │        token_type: "Bearer",    │                              │
     │        scope }                  │                              │
     │<────────────────────────────────│                              │
     │                                 │                              │
     │  11. Store tokens in OS         │                              │
     │      keychain (encrypted)       │                              │
     │                                 │                              │
     │  12. "Logged in as user@org"    │                              │
     │                                 │                              │
```

### 4.2 Flujo Web — OAuth2 Authorization Code + PKCE

```
┌──────────┐         ┌──────────────┐        ┌──────────────┐       ┌─────────┐
│ Browser  │         │ Cuervo Web   │        │ Auth Service  │       │ IdP     │
│ (User)   │         │ App (SPA)    │        │ (cuervo-auth) │       │ (Okta/  │
│          │         │              │        │               │       │  Azure) │
└────┬─────┘         └──────┬───────┘        └──────┬────────┘       └────┬────┘
     │                      │                       │                     │
     │  1. Click "Login"    │                       │                     │
     │─────────────────────>│                       │                     │
     │                      │                       │                     │
     │                      │  2. Generate PKCE     │                     │
     │                      │  code_verifier +      │                     │
     │                      │  code_challenge       │                     │
     │                      │                       │                     │
     │  3. Redirect to:     │                       │                     │
     │  /oauth/authorize?   │                       │                     │
     │  client_id=...&      │                       │                     │
     │  redirect_uri=...&   │                       │                     │
     │  code_challenge=...& │                       │                     │
     │  scope=...&state=... │                       │                     │
     │<─────────────────────│                       │                     │
     │                      │                       │                     │
     │  4. GET /oauth/authorize                     │                     │
     │──────────────────────────────────────────────>│                     │
     │                      │                       │                     │
     │                      │                       │  5. IF SSO configured│
     │                      │                       │  redirect to IdP    │
     │                      │                       │────────────────────>│
     │                      │                       │                     │
     │                      │         6. SAML/OIDC flow                   │
     │<───────────────────────────────────────────────────────────────────>│
     │                      │                       │                     │
     │                      │                       │  7. IdP callback    │
     │                      │                       │  with assertion     │
     │                      │                       │<────────────────────│
     │                      │                       │                     │
     │  8. Redirect to app with auth code           │                     │
     │  redirect_uri?code=AUTH_CODE&state=...        │                     │
     │<──────────────────────────────────────────────│                     │
     │                      │                       │                     │
     │  9. App receives code│                       │                     │
     │─────────────────────>│                       │                     │
     │                      │                       │                     │
     │                      │  10. POST /oauth/token│                     │
     │                      │  { code, verifier,    │                     │
     │                      │    redirect_uri }     │                     │
     │                      │──────────────────────>│                     │
     │                      │                       │                     │
     │                      │  11. { access_token,  │                     │
     │                      │    refresh_token,     │                     │
     │                      │    id_token }         │                     │
     │                      │<──────────────────────│                     │
     │                      │                       │                     │
     │  12. Authenticated   │                       │                     │
     │<─────────────────────│                       │                     │
```

### 4.3 Flujo Machine-to-Machine — Client Credentials

```
┌──────────────┐                    ┌──────────────┐
│ CI/CD        │                    │ Auth Service  │
│ / Automation │                    │ (cuervo-auth) │
│ / Service    │                    │               │
└──────┬───────┘                    └──────┬────────┘
       │                                   │
       │  1. POST /oauth/token             │
       │  {                                │
       │    grant_type: client_credentials,│
       │    client_id: "sa_...",           │
       │    client_secret: "sk_...",       │
       │    scope: "project:read model:invoke:haiku"
       │  }                                │
       │──────────────────────────────────>│
       │                                   │
       │  2. Validate credentials          │
       │  3. Check IP allowlist            │
       │  4. Check scope against SA grants │
       │  5. Check org quotas              │
       │                                   │
       │  6. {                             │
       │    access_token (JWT, 1h TTL),    │
       │    token_type: "Bearer",          │
       │    expires_in: 3600,              │
       │    scope: "project:read model:invoke:haiku"
       │  }                                │
       │<──────────────────────────────────│
       │                                   │
       │  7. Use token for API calls       │
       │  Authorization: Bearer <jwt>      │
       │──────────────────────────────────>│
       │                                   │
```

### 4.4 Flujo SAML SSO

```
┌──────────┐        ┌──────────────┐       ┌─────────────┐
│ User     │        │ Cuervo       │       │ Corporate   │
│ Browser  │        │ Auth Service │       │ IdP (SAML)  │
│          │        │ (SP)         │       │ (Okta/ADFS) │
└────┬─────┘        └──────┬───────┘       └──────┬──────┘
     │                     │                      │
     │  1. GET /login?     │                      │
     │  org=acme-corp      │                      │
     │────────────────────>│                      │
     │                     │                      │
     │                     │  2. Lookup SSO config │
     │                     │  for org "acme-corp"  │
     │                     │                      │
     │  3. Redirect:       │                      │
     │  SAML AuthnRequest  │                      │
     │  (signed, deflated) │                      │
     │<────────────────────│                      │
     │                     │                      │
     │  4. POST to IdP SSO │                      │
     │  endpoint           │                      │
     │─────────────────────────────────────────── >│
     │                     │                      │
     │                     │  5. IdP authenticates │
     │                     │  (password, MFA, etc) │
     │<────────────────────────────────────────────│
     │                     │                      │
     │  6. Submit form     │                      │
     │  (SAML Response)    │                      │
     │────────────────────>│                      │
     │                     │                      │
     │                     │  7. Validate SAML     │
     │                     │  - Verify signature   │
     │                     │  - Check assertions   │
     │                     │  - Extract attributes │
     │                     │  - Check conditions   │
     │                     │  - Map to user        │
     │                     │                      │
     │                     │  8. JIT provisioning  │
     │                     │  (if new user)        │
     │                     │  - Create user record │
     │                     │  - Assign default role│
     │                     │  - Apply group mapping│
     │                     │                      │
     │  9. Set session +   │                      │
     │  redirect to app    │                      │
     │<────────────────────│                      │
```

---

## 5. Token Architecture

### 5.1 JWT Structure

```typescript
// infrastructure/auth/jwt.ts

/**
 * Access Token (short-lived, 15 minutes)
 */
interface AccessTokenClaims {
  /** Standard claims */
  iss: string;                // 'https://auth.cuervo.dev'
  sub: string;                // User ID or Service Account ID
  aud: string[];              // ['https://api.cuervo.dev']
  exp: number;                // Expiration (Unix timestamp)
  iat: number;                // Issued at
  nbf: number;               // Not before
  jti: string;                // Unique token ID (for revocation)

  /** Cuervo-specific claims */
  tenant_id: string;
  principal_type: 'user' | 'service_account';
  email?: string;
  roles: string[];            // Compacted role IDs
  scopes: string[];           // OAuth2 scopes granted

  /** Security context */
  mfa_verified: boolean;
  auth_method: AuthMethod;
  device_id: string;
  session_id: string;

  /** Org context */
  org_plan: OrgPlan;
  org_features: string[];     // Feature flags

  /** Operational context */
  rate_limit_tier: string;    // 'free' | 'team' | 'business' | 'enterprise'
}

/**
 * Refresh Token (long-lived, 7 days, rotated on use)
 */
interface RefreshTokenRecord {
  id: string;                 // Token ID (stored, not the token itself)
  userId: string;
  tenantId: string;
  tokenHash: string;          // SHA-256 hash of the actual token
  familyId: string;           // Token family for rotation detection
  deviceId: string;
  ipAddress: string;
  userAgent: string;
  expiresAt: string;
  createdAt: string;
  rotatedAt: string | null;
  revokedAt: string | null;
  /** If this token was used after rotation = token theft detected */
  compromised: boolean;
}

/**
 * API Key (long-lived, for integrations)
 */
interface APIKey {
  id: string;
  tenantId: string;
  createdBy: string;
  name: string;               // Human-readable label
  prefix: string;             // First 8 chars for identification: 'ck_live_'
  keyHash: string;            // SHA-256 of the full key
  scopes: string[];           // Granular permissions
  ipAllowlist: string[] | null;
  expiresAt: string | null;
  lastUsedAt: string | null;
  lastUsedIP: string | null;
  usageCount: number;
  rateLimit: number;          // Requests per minute
  status: 'active' | 'revoked' | 'expired';
  createdAt: string;
  revokedAt: string | null;
  revokedBy: string | null;
}
```

### 5.2 Token Lifecycle

```
┌─────────────────────────────────────────────────────────────────────┐
│                     TOKEN LIFECYCLE                                  │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  ACCESS TOKEN (JWT)                                                 │
│  ┌─────────┐    ┌─────────┐    ┌─────────┐    ┌─────────┐         │
│  │ Issue   │───>│ Active  │───>│ Expired │    │ Revoked │         │
│  │ (login) │    │ (15min) │    │         │    │ (force) │         │
│  └─────────┘    └────┬────┘    └─────────┘    └─────────┘         │
│                      │                                              │
│                      │ approaching expiry                           │
│                      ▼                                              │
│  REFRESH TOKEN                                                      │
│  ┌─────────┐    ┌─────────┐    ┌─────────┐    ┌─────────┐         │
│  │ Issue   │───>│ Active  │───>│ Rotated │───>│ Expired │         │
│  │         │    │ (7days) │    │ (new    │    │         │         │
│  └─────────┘    └────┬────┘    │  issued)│    └─────────┘         │
│                      │         └─────────┘                         │
│                      │                                              │
│                      │ reuse after rotation                         │
│                      ▼                                              │
│                 ┌──────────┐                                        │
│                 │ THEFT    │ → Revoke entire token family            │
│                 │ DETECTED │ → Force re-login                       │
│                 │          │ → Alert security admin                  │
│                 │          │ → Log security event                    │
│                 └──────────┘                                        │
│                                                                     │
│  API KEY                                                            │
│  ┌─────────┐    ┌─────────┐    ┌─────────┐    ┌─────────┐         │
│  │ Create  │───>│ Active  │───>│ Expired │    │ Revoked │         │
│  │         │    │         │    │ (if TTL)│    │ (manual)│         │
│  └─────────┘    └─────────┘    └─────────┘    └─────────┘         │
│                                                                     │
│  Rotation: Managed via org policy (e.g., every 90 days)            │
│  Grace period: Old key valid for 24h after new key generated       │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

### 5.3 Token Storage (CLI)

```typescript
// infrastructure/auth/token-store.ts

interface TokenStore {
  /**
   * Store tokens securely in OS keychain.
   * macOS: Keychain Access
   * Linux: libsecret (GNOME Keyring / KDE Wallet)
   * Windows: Credential Manager
   */
  saveTokens(tokens: {
    accessToken: string;
    refreshToken: string;
    expiresAt: string;
    tenantId: string;
    userId: string;
  }): Promise<void>;

  /**
   * Retrieve current tokens. Auto-refresh if access token expired.
   */
  getAccessToken(): Promise<string | null>;

  /**
   * Clear all stored tokens (logout).
   */
  clearTokens(): Promise<void>;

  /**
   * Check if user is authenticated.
   */
  isAuthenticated(): Promise<boolean>;

  /**
   * Get token metadata without exposing token values.
   */
  getTokenInfo(): Promise<{
    userId: string;
    tenantId: string;
    expiresAt: string;
    authMethod: AuthMethod;
    scopes: string[];
  } | null>;
}
```

---

## 6. SCIM 2.0 Provisioning

### 6.1 SCIM Endpoints

```
BASE: /scim/v2

┌────────────────────────────────────────────────────────────────────┐
│ Endpoint                    │ Method │ Description                 │
├────────────────────────────────────────────────────────────────────┤
│ /Users                      │ GET    │ List users (filtered)       │
│ /Users                      │ POST   │ Create user                 │
│ /Users/:id                  │ GET    │ Get user                    │
│ /Users/:id                  │ PUT    │ Replace user                │
│ /Users/:id                  │ PATCH  │ Update user attributes      │
│ /Users/:id                  │ DELETE │ Deprovision user            │
│ /Groups                     │ GET    │ List groups (teams)         │
│ /Groups                     │ POST   │ Create group (team)         │
│ /Groups/:id                 │ GET    │ Get group                   │
│ /Groups/:id                 │ PUT    │ Replace group               │
│ /Groups/:id                 │ PATCH  │ Update group membership     │
│ /Groups/:id                 │ DELETE │ Delete group                │
│ /Schemas                    │ GET    │ SCIM schemas                │
│ /ServiceProviderConfig      │ GET    │ SP capabilities             │
│ /ResourceTypes              │ GET    │ Supported resource types    │
│ /Bulk                       │ POST   │ Bulk operations             │
└────────────────────────────────────────────────────────────────────┘
```

### 6.2 SCIM User Schema

```typescript
// infrastructure/scim/schemas.ts

interface SCIMUser {
  schemas: ['urn:ietf:params:scim:schemas:core:2.0:User'];
  id: string;
  externalId: string;          // IdP's user ID
  userName: string;            // Typically email
  name: {
    givenName: string;
    familyName: string;
    formatted: string;
  };
  displayName: string;
  emails: {
    value: string;
    type: 'work' | 'home';
    primary: boolean;
  }[];
  active: boolean;
  groups: {
    value: string;             // Group ID
    display: string;
    $ref: string;              // URI to group
  }[];
  roles: {
    value: string;
    display: string;
    type: string;
    primary: boolean;
  }[];
  meta: {
    resourceType: 'User';
    created: string;
    lastModified: string;
    location: string;
    version: string;
  };
}

interface SCIMGroup {
  schemas: ['urn:ietf:params:scim:schemas:core:2.0:Group'];
  id: string;
  externalId?: string;
  displayName: string;
  members: {
    value: string;
    display: string;
    $ref: string;
    type: 'User';
  }[];
  meta: {
    resourceType: 'Group';
    created: string;
    lastModified: string;
    location: string;
  };
}
```

### 6.3 SCIM Provisioning Behavior

| IdP Action | Cuervo Response |
|-----------|----------------|
| Create user | Create Cuervo user with `provisionedVia: 'scim'`, assign default role |
| Update user | Update email, name, attributes. Re-evaluate group→role mapping |
| Deactivate user | Set `status: 'suspended'`, revoke all tokens, keep data |
| Delete user | Set `status: 'deprovisioned'`, revoke tokens, schedule data deletion per policy |
| Add to group | Map IdP group to Cuervo team + role. Apply binding |
| Remove from group | Remove Cuervo team binding. Revoke project access if no other binding |
| Re-activate user | Set `status: 'active'`, restore previous role bindings |

---

## 7. Service Accounts & API Keys

### 7.1 Service Account Model

```typescript
// domain/entities/service-account.ts

interface ServiceAccount {
  readonly id: string;           // 'sa_' prefixed UUID
  readonly tenantId: string;

  name: string;
  description: string;

  /** Who owns/manages this SA */
  ownerId: string;               // User who created it

  /** OAuth2 credentials */
  clientId: string;              // Public identifier
  clientSecretHash: string;      // bcrypt hash of secret

  /** Scopes define what this SA can do */
  scopes: string[];

  /** Security constraints */
  ipAllowlist: string[] | null;
  maxTokenTTL: number;           // seconds

  /** Lifecycle */
  status: 'active' | 'suspended' | 'revoked';
  expiresAt: string | null;
  lastUsedAt: string | null;
  createdAt: string;
  rotatedAt: string | null;

  /** Audit */
  lastRotatedBy: string | null;
  usageCount: number;
}
```

### 7.2 API Key Format

```
Format: ck_<environment>_<random_42_chars>

Examples:
  ck_live_a1b2c3d4e5f6g7h8i9j0k1l2m3n4o5p6q7r8s9t0uv
  ck_test_x9y8z7w6v5u4t3s2r1q0p9o8n7m6l5k4j3i2h1g0fe

Prefix meanings:
  ck_live_  → Production API key
  ck_test_  → Sandbox/test API key
  ck_ci_    → CI/CD specific key (auto-rotating)

Storage:
  - Full key shown ONCE at creation (never stored in plaintext)
  - SHA-256 hash stored in database
  - Prefix stored separately for identification
  - Last 4 chars stored for UI display ("...t0uv")
```

---

## 8. Threat Model

### 8.1 Trust Boundaries

```
┌──────────────────────────────────────────────────────────────────────────┐
│                        TRUST BOUNDARY DIAGRAM                            │
├──────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  BOUNDARY 0: USER'S MACHINE (Highest Trust)                              │
│  ┌────────────────────────────────────────────────────────────────────┐  │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐            │  │
│  │  │ CLI Process  │  │ OS Keychain  │  │ Local SQLite │            │  │
│  │  │ (cuervo)     │  │ (secrets)    │  │ (context.db) │            │  │
│  │  └──────────────┘  └──────────────┘  └──────────────┘            │  │
│  │  ┌──────────────┐  ┌──────────────┐                               │  │
│  │  │ .cuervo/     │  │ Ollama       │                               │  │
│  │  │ (project)    │  │ (local LLM)  │                               │  │
│  │  └──────────────┘  └──────────────┘                               │  │
│  └───────────────────────────┬────────────────────────────────────────┘  │
│                              │ TLS 1.3                                    │
│  BOUNDARY 1: CUERVO PLATFORM ▼ (Medium Trust)                            │
│  ┌────────────────────────────────────────────────────────────────────┐  │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐            │  │
│  │  │ API Gateway  │  │ Auth Service │  │ Context Svc  │            │  │
│  │  │ (rate limit, │  │ (JWT, SSO)   │  │ (data store) │            │  │
│  │  │  WAF)        │  │              │  │              │            │  │
│  │  └──────────────┘  └──────────────┘  └──────────────┘            │  │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐            │  │
│  │  │ Model Proxy  │  │ PostgreSQL   │  │ Redis        │            │  │
│  │  │ (routing)    │  │ (RLS)        │  │ (sessions)   │            │  │
│  │  └──────────────┘  └──────────────┘  └──────────────┘            │  │
│  └───────────────────────────┬────────────────────────────────────────┘  │
│                              │ TLS 1.3 + API keys                        │
│  BOUNDARY 2: EXTERNAL        ▼ (Low Trust)                               │
│  ┌────────────────────────────────────────────────────────────────────┐  │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐            │  │
│  │  │ Anthropic    │  │ OpenAI       │  │ Google       │            │  │
│  │  │ API          │  │ API          │  │ AI API       │            │  │
│  │  └──────────────┘  └──────────────┘  └──────────────┘            │  │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐            │  │
│  │  │ GitHub/      │  │ Corporate    │  │ Third-party  │            │  │
│  │  │ GitLab       │  │ IdP (SSO)    │  │ Plugins      │            │  │
│  │  └──────────────┘  └──────────────┘  └──────────────┘            │  │
│  └────────────────────────────────────────────────────────────────────┘  │
│                                                                          │
└──────────────────────────────────────────────────────────────────────────┘
```

### 8.2 Threat Catalog

| ID | Threat | STRIDE | Severity | Attack Vector | Mitigation |
|----|--------|--------|----------|--------------|-----------|
| T-01 | Stolen refresh token | Spoofing | Critical | Malware on user machine, clipboard sniffing | Token family rotation + theft detection, OS keychain storage, device binding |
| T-02 | JWT forging | Spoofing | Critical | Key compromise | RS256 with key rotation (every 90d), JWKS endpoint with caching, key stored in HSM/KMS |
| T-03 | Session hijacking | Spoofing | High | XSS, network sniff | HttpOnly cookies, SameSite=Strict, TLS only, IP binding (optional), device fingerprint |
| T-04 | SCIM token compromise | Spoofing | High | IdP misconfiguration | Dedicated bearer token per org, IP allowlist, rate limit, audit all SCIM ops |
| T-05 | Privilege escalation | Elevation | Critical | RBAC bypass, role assignment flaw | Server-side authz check on every request, no client-trust, admin actions require step-up auth |
| T-06 | Cross-tenant data leak | Info Disclosure | Critical | RLS bypass, query injection | RLS + application-level tenant check, prepared statements, no raw SQL, penetration testing |
| T-07 | API key leak in logs | Info Disclosure | High | Accidental logging | Key prefix-only in logs, PII redaction pipeline, secret scanning in CI |
| T-08 | Brute force login | Spoofing | Medium | Automated password guessing | Rate limiting (5 attempts/15min), account lockout, CAPTCHA, IP reputation |
| T-09 | SSO misconfiguration | Spoofing | High | Open redirect, assertion replay | SAML response validation (audience, timestamps, replay cache), signature verification, assertion encryption |
| T-10 | Refresh token replay | Spoofing | High | Token stolen after rotation | Token family tracking: if rotated token reused → revoke entire family + force re-login |
| T-11 | Service account over-privilege | Elevation | Medium | SA with admin scope | Scope minimization enforcement, SA audit, usage monitoring, expiration requirement |
| T-12 | Agent permission escape | Elevation | High | Prompt injection → agent bypasses tool restrictions | Agent permissions enforced at tool execution layer (not prompt), allow-list approach, sandbox |

### 8.3 Security Controls Matrix

| Control | Implementation | Layer |
|---------|---------------|-------|
| **Authentication** | OAuth2/OIDC/SAML + MFA + Passkeys | Boundary 0-1 |
| **Authorization** | RBAC + ABAC + conditional access | Application |
| **Transport encryption** | TLS 1.3 (min TLS 1.2) | Network |
| **Data encryption at rest** | AES-256-GCM (OS keychain local, KMS cloud) | Storage |
| **Secret management** | OS keychain (local), HashiCorp Vault / AWS KMS (cloud) | Infrastructure |
| **Token security** | Short-lived JWT (15min), rotation, revocation | Application |
| **Rate limiting** | Token bucket per tenant/user/IP | API Gateway |
| **Anomaly detection** | Login velocity, geo-impossible travel, behavioral | Monitoring |
| **Audit logging** | All auth events, tamper-proof hash chain | Monitoring |
| **Input validation** | Schema validation on all inputs, parameterized queries | Application |
| **Dependency scanning** | Snyk/Dependabot, lock file integrity | CI/CD |
| **Penetration testing** | Annual third-party pentest + continuous bug bounty | Process |

---

## 9. Key Management

### 9.1 Key Hierarchy

```
┌─────────────────────────────────────────────────────────┐
│                   KEY HIERARCHY                          │
├─────────────────────────────────────────────────────────┤
│                                                          │
│  ROOT KEY (HSM / Cloud KMS)                              │
│  ├── Master Encryption Key (MEK)                         │
│  │   ├── Tenant Encryption Key (TEK) — per tenant        │
│  │   │   ├── Data Encryption Key (DEK) — per data class  │
│  │   │   │   ├── Context encryption                      │
│  │   │   │   ├── Secret encryption                       │
│  │   │   │   └── Backup encryption                       │
│  │   │   └── Search Encryption Key (SEK)                 │
│  │   │       └── Encrypted search indexes                │
│  │   └── Token Signing Key (TSK)                         │
│  │       ├── JWT signing (RS256, rotated 90d)            │
│  │       └── SAML assertion verification                 │
│  └── SCIM Bearer Tokens (per org)                        │
│                                                          │
│  CUSTOMER-MANAGED KEYS (Enterprise tier)                 │
│  ├── Customer provides KMS ARN / key reference           │
│  ├── Cuervo wraps TEK with customer key                  │
│  └── Customer can revoke access = instant data lockout   │
│                                                          │
└─────────────────────────────────────────────────────────┘
```

### 9.2 Key Rotation Schedule

| Key Type | Rotation Period | Method | Downtime |
|---------|----------------|--------|---------|
| JWT signing key | 90 days | Dual-key (old + new valid during overlap) | Zero |
| Tenant encryption key | 365 days | Re-encrypt on read (lazy migration) | Zero |
| Data encryption key | 180 days | Background re-encryption job | Zero |
| SCIM bearer token | On-demand | Org admin regenerates | Brief (requires IdP update) |
| API keys | Configurable (90d default) | Grace period (24h overlap) | Zero |
| Service account secrets | 180 days | Automated rotation with notification | Zero (grace period) |
| TLS certificates | 90 days (Let's Encrypt) | Automated via cert-manager | Zero |

---

## 10. Encryption

### 10.1 Data Classification & Encryption

| Data Class | Examples | At Rest | In Transit | Key |
|-----------|---------|---------|-----------|-----|
| **Critical** | Passwords, API keys, tokens, SAML certs | AES-256-GCM (TEK) | TLS 1.3 | HSM-backed |
| **Sensitive** | PII, email, conversations, embeddings | AES-256-GCM (DEK) | TLS 1.3 | KMS-managed |
| **Internal** | Audit logs, usage metrics, configs | Disk encryption (LUKS/BitLocker) | TLS 1.3 | Volume-level |
| **Public** | Documentation, schemas, public configs | None | TLS 1.3 | N/A |

### 10.2 Encryption Implementation

```typescript
// infrastructure/crypto/encryption-service.ts

interface EncryptionService {
  /**
   * Encrypt a value for storage.
   * Uses envelope encryption: generate random DEK, encrypt data with DEK,
   * encrypt DEK with KEK (from KMS).
   */
  encrypt(
    plaintext: Buffer,
    context: { tenantId: string; dataClass: 'critical' | 'sensitive' },
  ): Promise<EncryptedPayload>;

  /**
   * Decrypt a stored value.
   */
  decrypt(
    payload: EncryptedPayload,
    context: { tenantId: string },
  ): Promise<Buffer>;

  /**
   * Rotate encryption key for a tenant.
   * Does NOT re-encrypt existing data (lazy migration on read).
   */
  rotateKey(tenantId: string): Promise<{ newKeyId: string; oldKeyId: string }>;

  /**
   * Re-encrypt a value with the current key (called on read if key is old).
   */
  reEncrypt(
    payload: EncryptedPayload,
    context: { tenantId: string },
  ): Promise<EncryptedPayload>;
}

interface EncryptedPayload {
  ciphertext: Buffer;
  iv: Buffer;            // 12 bytes for AES-256-GCM
  authTag: Buffer;       // 16 bytes
  keyId: string;         // Which KEK was used
  algorithm: 'aes-256-gcm';
  keyVersion: number;
}
```

---

## 11. Database Schema (IAM)

```sql
-- Users table
CREATE TABLE users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    email TEXT NOT NULL,
    email_verified BOOLEAN NOT NULL DEFAULT false,
    display_name TEXT NOT NULL,
    avatar_url TEXT,
    locale TEXT DEFAULT 'en',
    timezone TEXT DEFAULT 'UTC',

    -- Auth
    password_hash TEXT,
    failed_login_attempts INTEGER NOT NULL DEFAULT 0,
    locked_until TIMESTAMPTZ,
    last_password_change_at TIMESTAMPTZ,

    -- MFA
    mfa_methods JSONB NOT NULL DEFAULT '[]',
    passkeys JSONB NOT NULL DEFAULT '[]',

    -- External identities
    external_identities JSONB NOT NULL DEFAULT '[]',

    -- Provisioning
    provisioned_via TEXT NOT NULL DEFAULT 'manual',
    scim_external_id TEXT,

    -- Lifecycle
    status TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_login_at TIMESTAMPTZ,
    deactivated_at TIMESTAMPTZ,
    deactivated_by UUID,
    deactivation_reason TEXT,

    UNIQUE(tenant_id, email),
    CONSTRAINT valid_status CHECK (status IN ('active','suspended','deprovisioned','pending_activation'))
);

ALTER TABLE users ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON users
    USING (tenant_id = current_setting('app.current_tenant')::UUID);

-- Roles
CREATE TABLE roles (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID REFERENCES tenants(id),  -- NULL = system built-in role
    name TEXT NOT NULL,
    description TEXT,
    level TEXT NOT NULL,
    built_in BOOLEAN NOT NULL DEFAULT false,
    permissions JSONB NOT NULL DEFAULT '[]',
    inherits JSONB NOT NULL DEFAULT '[]',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    UNIQUE(tenant_id, name),
    CONSTRAINT valid_level CHECK (level IN ('org','project','platform'))
);

-- User-Role bindings
CREATE TABLE user_role_bindings (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role_id UUID NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    scope_type TEXT NOT NULL,
    scope_id UUID NOT NULL,
    granted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    granted_by UUID NOT NULL,
    expires_at TIMESTAMPTZ,
    conditions JSONB,

    UNIQUE(user_id, role_id, scope_type, scope_id),
    CONSTRAINT valid_scope_type CHECK (scope_type IN ('org','project','team'))
);

ALTER TABLE user_role_bindings ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON user_role_bindings
    USING (tenant_id = current_setting('app.current_tenant')::UUID);

-- Service Accounts
CREATE TABLE service_accounts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT,
    owner_id UUID NOT NULL REFERENCES users(id),
    client_id TEXT UNIQUE NOT NULL,
    client_secret_hash TEXT NOT NULL,
    scopes JSONB NOT NULL DEFAULT '[]',
    ip_allowlist JSONB,
    max_token_ttl INTEGER NOT NULL DEFAULT 3600,
    status TEXT NOT NULL DEFAULT 'active',
    expires_at TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    rotated_at TIMESTAMPTZ,
    usage_count BIGINT NOT NULL DEFAULT 0,

    CONSTRAINT valid_status CHECK (status IN ('active','suspended','revoked'))
);

ALTER TABLE service_accounts ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON service_accounts
    USING (tenant_id = current_setting('app.current_tenant')::UUID);

-- API Keys
CREATE TABLE api_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    created_by UUID NOT NULL REFERENCES users(id),
    name TEXT NOT NULL,
    prefix TEXT NOT NULL,
    key_hash TEXT NOT NULL,
    last_four TEXT NOT NULL,
    scopes JSONB NOT NULL DEFAULT '[]',
    ip_allowlist JSONB,
    rate_limit INTEGER NOT NULL DEFAULT 60,
    expires_at TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    last_used_ip TEXT,
    usage_count BIGINT NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ,
    revoked_by UUID,

    CONSTRAINT valid_status CHECK (status IN ('active','revoked','expired'))
);

ALTER TABLE api_keys ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON api_keys
    USING (tenant_id = current_setting('app.current_tenant')::UUID);

-- Refresh tokens
CREATE TABLE refresh_tokens (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL,
    family_id UUID NOT NULL,
    device_id TEXT NOT NULL,
    ip_address TEXT NOT NULL,
    user_agent TEXT,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    rotated_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    compromised BOOLEAN NOT NULL DEFAULT false
);

CREATE INDEX idx_refresh_tokens_family ON refresh_tokens(family_id);
CREATE INDEX idx_refresh_tokens_user ON refresh_tokens(user_id, revoked_at);

-- Sessions
CREATE TABLE auth_sessions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL,
    ip_address TEXT NOT NULL,
    user_agent TEXT,
    auth_method TEXT NOT NULL,
    mfa_verified BOOLEAN NOT NULL DEFAULT false,
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_activity_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    revocation_reason TEXT
);

CREATE INDEX idx_sessions_user ON auth_sessions(user_id, revoked_at);

-- Audit log (auth events)
CREATE TABLE auth_audit_log (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL,
    user_id UUID,
    event_type TEXT NOT NULL,
    event_data JSONB NOT NULL,
    ip_address TEXT,
    user_agent TEXT,
    device_id TEXT,
    risk_score REAL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- Hash chain for tamper detection
    prev_hash TEXT,
    entry_hash TEXT NOT NULL
);

CREATE INDEX idx_auth_audit_tenant ON auth_audit_log(tenant_id, created_at DESC);
CREATE INDEX idx_auth_audit_user ON auth_audit_log(user_id, created_at DESC);
CREATE INDEX idx_auth_audit_event ON auth_audit_log(event_type, created_at DESC);
```

---

## 12. Decisiones Arquitectónicas (IAM)

| # | Decisión | Alternativa Descartada | Justificación |
|---|----------|----------------------|---------------|
| ADR-I01 | **Device Authorization (RFC 8628) para CLI** | Browser redirect con localhost callback | Device auth es más robusto en entornos headless (SSH, containers, CI). No requiere puerto local. Estándar probado (GitHub CLI, Azure CLI lo usan). |
| ADR-I02 | **RS256 (RSA) para JWT signing** | HS256 (HMAC shared secret) | RS256 permite verificación sin compartir el secreto. Necesario para arquitectura de microservicios donde múltiples servicios validan tokens. |
| ADR-I03 | **15 minutos TTL para access tokens** | 1 hora | Minimiza ventana de compromiso. Refresh token automático es transparente para el usuario. |
| ADR-I04 | **Token family rotation detection** | Simple token rotation | Detecta token theft: si un refresh token rotado se reutiliza, toda la familia se invalida. Patrón de Auth0/Okta. |
| ADR-I05 | **RLS + application-level tenant check** | Solo RLS o solo application check | Defense-in-depth: RLS como safety net, application check como primary. Si uno falla, el otro protege. |
| ADR-I06 | **RBAC + ABAC hybrid** | Solo RBAC | RBAC para roles estáticos; ABAC para conditional access (IP, tiempo, riesgo). Enterprise necesita ambos. |
| ADR-I07 | **SCIM 2.0 para provisioning** | Custom sync API | SCIM es el estándar de la industria. Todos los IdPs (Okta, Azure AD, OneLogin) lo soportan. Reduce fricción de adopción enterprise. |
| ADR-I08 | **Passkeys/WebAuthn como MFA preferido** | Solo TOTP | Passkeys son phishing-resistant, mejor UX que TOTP, estándar FIDO2. Futuro de la autenticación. |
| ADR-I09 | **OS Keychain para token storage (CLI)** | Archivo encriptado, environment variables | OS Keychain es la forma más segura de almacenar secretos en desktop. Protegido por el OS (biometrics, secure enclave). Env vars son leakeables en logs. |

---

## 13. Recommended Access Policies

### 13.1 Baseline Policies (All Plans)

```yaml
# Minimum security baseline
baseline:
  password:
    min_length: 12
    require_complexity: true
    max_age_days: 0          # No expiry (NIST 800-63B recommendation)
    prevent_common: true

  sessions:
    max_duration: 12h
    idle_timeout: 60min
    max_concurrent: 5

  tokens:
    access_ttl: 15min
    refresh_ttl: 7d
    rotate_on_use: true

  rate_limits:
    login_attempts: 5/15min
    api_requests: 60/min     # Free tier
    lockout_duration: 30min
```

### 13.2 Enterprise Recommended Policies

```yaml
enterprise_recommended:
  authentication:
    sso_enforced: true
    mfa_required: true
    mfa_methods: [webauthn, totp]
    passkey_preferred: true
    password_login: disabled  # SSO only

  sessions:
    max_duration: 8h
    idle_timeout: 30min
    max_concurrent: 3
    bind_to_device: true
    require_reauth_for:
      - org_settings
      - security_settings
      - member_management
      - api_key_creation
      - service_account_creation

  conditional_access:
    - name: "Block non-corporate IPs"
      conditions:
        ip_ranges: ["!10.0.0.0/8", "!172.16.0.0/12"]
      actions:
        grant: require_mfa

    - name: "Block high-risk logins"
      conditions:
        risk_level: high
      actions:
        grant: deny
        notify_admins: true

    - name: "Off-hours restriction"
      conditions:
        time_windows:
          - days: [0, 6]  # Weekend
            hours: [0, 23]
      actions:
        grant: require_approval
        restricted_scopes: [tool:execute:destructive]

  data:
    zero_retention: true
    customer_managed_keys: true
    data_residency: eu
    audit_retention: 7years

  service_accounts:
    max_ttl: 1h
    require_ip_allowlist: true
    max_per_org: 50
    require_expiration: true
    max_expiration: 365d
```

---

## 14. Plan de Implementación (IAM)

| Sprint | Entregable | Dependencias |
|--------|-----------|-------------|
| S1-S2 | User entity + password auth + bcrypt | Database schema |
| S3-S4 | JWT issuance (RS256) + refresh tokens + rotation | User entity |
| S5-S6 | Device Authorization flow (CLI login) | JWT system |
| S7-S8 | RBAC engine + built-in roles + permission check | User entity |
| S9-S10 | OAuth2/OIDC provider integration (Google, GitHub) | JWT system |
| S11-S12 | API key management + service accounts | RBAC engine |
| S13-S14 | MFA: TOTP + WebAuthn/Passkeys | User entity |
| S15-S16 | SAML 2.0 SSO integration | OAuth2 system |
| S17-S18 | SCIM 2.0 provisioning | User entity, RBAC |
| S19-S20 | ABAC conditional access engine | RBAC engine |
| S21-S22 | Encryption service + key management + rotation | Infrastructure |
| S23-S24 | Auth audit logging + anomaly detection | All above |
| S25-S26 | Penetration testing + security review | All above |
