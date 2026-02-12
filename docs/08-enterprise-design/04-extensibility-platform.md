# FASE 4 — Extensibilidad y Plataforma

> **Versión**: 1.0.0 | **Fecha**: 2026-02-06
> **Autores**: Staff Engineer, Developer Tooling
> **Estado**: Design Complete — Ready for Implementation Review

---

## 1. Visión General

La plataforma de extensibilidad de Cuervo permite que desarrolladores internos, partners y terceros extiendan las capacidades del sistema sin modificar el core. El diseño se inspira en plataformas probadas (VS Code extensions, GitHub Actions, Shopify Apps, Atlassian Connect) pero adaptado a las necesidades específicas de una herramienta de desarrollo AI-powered.

### 1.1 Principios

| Principio | Implementación |
|-----------|---------------|
| **Open by default** | APIs públicas bien documentadas; todo lo que el UI puede hacer, la API puede hacer |
| **Safe by design** | Plugins ejecutan en sandbox; no pueden escapar de su scope de permisos |
| **Backwards compatible** | API versionada; breaking changes solo en major versions con 12 meses de deprecation |
| **Developer-first DX** | CLI tools, SDK con tipos, playground, templates, hot-reload en dev |
| **Metered & auditable** | Toda extensión se mide (uso, costo, errores) y se audita |

---

## 2. Plugin System

### 2.1 Plugin Architecture

```
┌──────────────────────────────────────────────────────────────────────────┐
│                        PLUGIN ARCHITECTURE                               │
├──────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  ┌────────────────────────────────────────────────────────────────────┐  │
│  │                       PLUGIN HOST                                  │  │
│  │                                                                    │  │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐            │  │
│  │  │ Plugin       │  │ Plugin       │  │ Sandbox      │            │  │
│  │  │ Registry     │  │ Lifecycle    │  │ Runtime      │            │  │
│  │  │              │  │ Manager      │  │              │            │  │
│  │  │ • Discover   │  │ • Install    │  │ • Isolate    │            │  │
│  │  │ • Validate   │  │ • Enable     │  │ • Resource   │            │  │
│  │  │ • Version    │  │ • Disable    │  │   limits     │            │  │
│  │  │ • Resolve    │  │ • Uninstall  │  │ • Permissions│            │  │
│  │  │   deps       │  │ • Upgrade    │  │ • Network    │            │  │
│  │  └──────────────┘  └──────────────┘  └──────────────┘            │  │
│  │                                                                    │  │
│  │  ┌──────────────────────────────────────────────────────────────┐  │  │
│  │  │                     EXTENSION POINTS                         │  │  │
│  │  │                                                              │  │  │
│  │  │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐       │  │  │
│  │  │  │Commands  │ │Tools     │ │Agents    │ │Providers │       │  │  │
│  │  │  │          │ │          │ │          │ │          │       │  │  │
│  │  │  │Slash     │ │Custom    │ │Custom    │ │Model     │       │  │  │
│  │  │  │commands  │ │tools for │ │agent     │ │providers,│       │  │  │
│  │  │  │and CLI   │ │AI agents │ │behaviors │ │storage,  │       │  │  │
│  │  │  │extensions│ │to use    │ │& roles   │ │formatters│       │  │  │
│  │  │  └──────────┘ └──────────┘ └──────────┘ └──────────┘       │  │  │
│  │  │                                                              │  │  │
│  │  │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐       │  │  │
│  │  │  │Hooks     │ │Formatters│ │Themes    │ │Analyzers │       │  │  │
│  │  │  │          │ │          │ │          │ │          │       │  │  │
│  │  │  │Pre/post  │ │Output    │ │UI themes │ │Code      │       │  │  │
│  │  │  │event     │ │renderers │ │for CLI   │ │analysis  │       │  │  │
│  │  │  │hooks     │ │and       │ │          │ │and       │       │  │  │
│  │  │  │          │ │templates │ │          │ │linters   │       │  │  │
│  │  │  └──────────┘ └──────────┘ └──────────┘ └──────────┘       │  │  │
│  │  └──────────────────────────────────────────────────────────────┘  │  │
│  └────────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  ┌────────────────────────────────────────────────────────────────────┐  │
│  │                       PLUGIN API SURFACE                           │  │
│  │                                                                    │  │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐            │  │
│  │  │ Context API  │  │ Storage API  │  │ UI API       │            │  │
│  │  │ (read ctx)   │  │ (plugin KV)  │  │ (render,     │            │  │
│  │  │              │  │              │  │  prompts)    │            │  │
│  │  └──────────────┘  └──────────────┘  └──────────────┘            │  │
│  │                                                                    │  │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐            │  │
│  │  │ Event API    │  │ Network API  │  │ Secret API   │            │  │
│  │  │ (pub/sub)    │  │ (http, ws)   │  │ (vault)      │            │  │
│  │  └──────────────┘  └──────────────┘  └──────────────┘            │  │
│  └────────────────────────────────────────────────────────────────────┘  │
│                                                                          │
└──────────────────────────────────────────────────────────────────────────┘
```

### 2.2 Plugin Manifest

```typescript
// Plugin manifest: cuervo-plugin.json (at plugin root)

interface PluginManifest {
  /** Unique identifier (reverse-domain: com.acme.my-plugin) */
  id: string;

  /** Display name */
  name: string;

  /** Semantic version */
  version: string;

  /** Plugin description */
  description: string;

  /** Author information */
  author: {
    name: string;
    email: string;
    url?: string;
  };

  /** Publisher (org or individual) */
  publisher: string;

  /** License (SPDX) */
  license: string;

  /** Minimum Cuervo platform version */
  engines: {
    cuervo: string;  // semver range, e.g., ">=1.0.0"
  };

  /** Plugin category for marketplace */
  categories: PluginCategory[];

  /** Keywords for search */
  keywords: string[];

  /** Entry point */
  main: string;  // e.g., "./dist/index.js"

  /** Extension points this plugin provides */
  contributes: {
    /** Slash commands */
    commands?: CommandContribution[];
    /** Tools for AI agents */
    tools?: ToolContribution[];
    /** Custom agent types */
    agents?: AgentContribution[];
    /** Event hooks */
    hooks?: HookContribution[];
    /** Output formatters */
    formatters?: FormatterContribution[];
    /** Configuration schema */
    configuration?: ConfigurationContribution;
    /** Code analyzers */
    analyzers?: AnalyzerContribution[];
    /** Model providers */
    modelProviders?: ModelProviderContribution[];
  };

  /** Required permissions */
  permissions: PluginPermissionRequest[];

  /** Plugin dependencies */
  dependencies?: Record<string, string>;

  /** Activation events (when to load the plugin) */
  activationEvents: string[];

  /** Icon and visual assets */
  icon?: string;
  banner?: string;

  /** Repository URL */
  repository?: string;

  /** Pricing (for marketplace) */
  pricing?: {
    model: 'free' | 'paid' | 'freemium';
    plans?: PricingPlan[];
  };
}

type PluginCategory =
  | 'languages'
  | 'frameworks'
  | 'testing'
  | 'deployment'
  | 'security'
  | 'productivity'
  | 'ai-models'
  | 'integrations'
  | 'themes'
  | 'other';

interface CommandContribution {
  command: string;          // e.g., '/deploy'
  title: string;
  description: string;
  /** Arguments schema */
  args?: JSONSchema;
  /** When to show this command */
  when?: string;            // Condition expression
}

interface ToolContribution {
  name: string;             // Tool name for AI agents
  description: string;      // Description shown to AI
  /** Input schema (JSON Schema) */
  inputSchema: JSONSchema;
  /** Output schema */
  outputSchema?: JSONSchema;
  /** Permission level required */
  permissionLevel: 'read' | 'write' | 'destructive';
  /** Requires human confirmation? */
  requiresConfirmation: boolean;
}

interface HookContribution {
  event: string;            // e.g., 'pre:commit', 'post:model_invoke', 'on:error'
  handler: string;          // Export name of handler function
  /** Priority (lower = earlier) */
  priority?: number;
}

interface PluginPermissionRequest {
  scope: string;
  reason: string;           // Shown to user during install
  required: boolean;
}
```

### 2.3 Plugin API Surface

```typescript
// @cuervo/plugin-api/src/index.ts

/**
 * The API object provided to plugins at activation.
 * This is the ONLY interface between plugins and the host.
 */
interface CuervoPluginAPI {
  /** Plugin's own identity and config */
  readonly plugin: PluginContext;

  /** Context reading (read-only) */
  readonly context: ContextAPI;

  /** Plugin-scoped key-value storage */
  readonly storage: StorageAPI;

  /** Event publish/subscribe */
  readonly events: EventAPI;

  /** CLI output rendering */
  readonly ui: UIAPI;

  /** HTTP client (sandboxed) */
  readonly http: HttpAPI;

  /** Secret management */
  readonly secrets: SecretAPI;

  /** Register extension contributions */
  readonly register: RegistrationAPI;

  /** Logging */
  readonly log: LogAPI;

  /** Disposable management */
  readonly disposables: DisposableAPI;
}

// ─── Context API ──────────────────────────────────

interface ContextAPI {
  /** Get resolved context value */
  get<T>(key: string): Promise<T | undefined>;

  /** Get current user info */
  getUser(): Promise<{ id: string; email: string; roles: string[] }>;

  /** Get current project info */
  getProject(): Promise<{ id: string; path: string; language: string } | null>;

  /** Get current session info */
  getSession(): Promise<{ id: string; messages: number }>;

  /** Get current git state */
  getGitState(): Promise<{ branch: string; isClean: boolean } | null>;

  /** Get current working directory */
  getCwd(): string;

  /** Read file (sandboxed to project directory) */
  readFile(path: string): Promise<string>;

  /** List files (sandboxed) */
  listFiles(pattern: string): Promise<string[]>;
}

// ─── Storage API ──────────────────────────────────

interface StorageAPI {
  /**
   * Plugin-scoped key-value store.
   * Isolated per plugin + per tenant.
   * 10MB quota per plugin per tenant.
   */
  get<T>(key: string): Promise<T | undefined>;
  set<T>(key: string, value: T): Promise<void>;
  delete(key: string): Promise<void>;
  keys(): Promise<string[]>;

  /** Global storage (shared across projects, same tenant) */
  readonly global: {
    get<T>(key: string): Promise<T | undefined>;
    set<T>(key: string, value: T): Promise<void>;
    delete(key: string): Promise<void>;
  };
}

// ─── Event API ────────────────────────────────────

interface EventAPI {
  /** Subscribe to platform events */
  on(event: string, handler: (data: unknown) => void | Promise<void>): Disposable;

  /** Emit custom events (namespaced to plugin) */
  emit(event: string, data: unknown): Promise<void>;
}

// Well-known events:
// 'session:start', 'session:end'
// 'message:user', 'message:assistant'
// 'tool:before_execute', 'tool:after_execute'
// 'model:before_invoke', 'model:after_invoke'
// 'agent:task_start', 'agent:task_complete'
// 'git:commit', 'git:push'
// 'file:change', 'file:create', 'file:delete'
// 'error:model', 'error:tool', 'error:plugin'

// ─── UI API ───────────────────────────────────────

interface UIAPI {
  /** Show an info message in the CLI */
  showInfo(message: string): void;

  /** Show a warning */
  showWarning(message: string): void;

  /** Show an error */
  showError(message: string): void;

  /** Render markdown content */
  renderMarkdown(content: string): void;

  /** Show a progress indicator */
  showProgress(title: string, task: () => Promise<void>): Promise<void>;

  /** Prompt user for input */
  prompt(options: PromptOptions): Promise<string | string[] | boolean>;

  /** Render a table */
  renderTable(headers: string[], rows: string[][]): void;

  /** Render a diff */
  renderDiff(oldContent: string, newContent: string, filename?: string): void;
}

interface PromptOptions {
  type: 'input' | 'confirm' | 'select' | 'multiselect';
  message: string;
  choices?: { label: string; value: string }[];
  default?: string | boolean;
}

// ─── Registration API ─────────────────────────────

interface RegistrationAPI {
  /** Register a slash command handler */
  registerCommand(command: string, handler: CommandHandler): Disposable;

  /** Register a tool for AI agents */
  registerTool(tool: ToolDefinition, handler: ToolHandler): Disposable;

  /** Register an event hook */
  registerHook(event: string, handler: HookHandler): Disposable;

  /** Register an output formatter */
  registerFormatter(name: string, formatter: OutputFormatter): Disposable;

  /** Register a code analyzer */
  registerAnalyzer(analyzer: CodeAnalyzer): Disposable;

  /** Register a model provider */
  registerModelProvider(provider: ModelProviderRegistration): Disposable;
}

type CommandHandler = (args: Record<string, unknown>, api: CuervoPluginAPI) => Promise<void>;
type ToolHandler = (input: Record<string, unknown>) => Promise<unknown>;
type HookHandler = (event: unknown) => Promise<void | { modified: unknown }>;

interface ToolDefinition {
  name: string;
  description: string;
  inputSchema: JSONSchema;
  permissionLevel: 'read' | 'write' | 'destructive';
  requiresConfirmation: boolean;
}
```

### 2.4 Plugin Lifecycle

```typescript
// Plugin entry point contract

/**
 * Every plugin must export an `activate` function.
 * This is called when the plugin is loaded (based on activationEvents).
 */
export function activate(api: CuervoPluginAPI): void | Promise<void> {
  // Register contributions
  api.register.registerCommand('/my-command', async (args) => {
    const project = await api.context.getProject();
    api.ui.showInfo(`Running on ${project?.path}`);
  });

  api.register.registerTool(
    {
      name: 'my_tool',
      description: 'Does something useful',
      inputSchema: { type: 'object', properties: { query: { type: 'string' } } },
      permissionLevel: 'read',
      requiresConfirmation: false,
    },
    async (input) => {
      // Tool implementation
      return { result: 'done' };
    },
  );
}

/**
 * Optional: cleanup when plugin is deactivated.
 */
export function deactivate(): void | Promise<void> {
  // Cleanup resources
}
```

---

## 3. Public API

### 3.1 REST API Design

```
BASE: https://api.cuervo.dev/v1

Authentication: Bearer <access_token> or API key (ck_live_...)

┌──────────────────────────────────────────────────────────────────────────────┐
│                             PUBLIC REST API                                  │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  SESSIONS                                                                    │
│  POST   /sessions                      Create new session                    │
│  GET    /sessions                      List sessions                         │
│  GET    /sessions/:id                  Get session details                   │
│  DELETE /sessions/:id                  End session                           │
│                                                                              │
│  MESSAGES (within a session)                                                 │
│  POST   /sessions/:id/messages         Send message (trigger agent)          │
│  GET    /sessions/:id/messages         List messages                         │
│  POST   /sessions/:id/messages/stream  Send message (SSE streaming)          │
│                                                                              │
│  CONTEXT                                                                     │
│  GET    /context/resolve               Resolve context                       │
│  GET    /context/entries               List entries                          │
│  POST   /context/entries               Create entry                          │
│  PUT    /context/entries/:id           Update entry                          │
│  DELETE /context/entries/:id           Delete entry                          │
│  POST   /context/search               Semantic search                        │
│                                                                              │
│  PROJECTS                                                                    │
│  GET    /projects                      List projects                         │
│  POST   /projects                      Create project                        │
│  GET    /projects/:id                  Get project details                   │
│  PATCH  /projects/:id                  Update project                        │
│  DELETE /projects/:id                  Delete project                        │
│  POST   /projects/:id/index           Trigger codebase indexing              │
│  GET    /projects/:id/index/status     Get indexing status                   │
│                                                                              │
│  MODELS                                                                      │
│  GET    /models                        List available models                  │
│  GET    /models/:id                    Get model details                     │
│  POST   /models/invoke                 Direct model invocation               │
│  POST   /models/invoke/stream          Streaming model invocation            │
│                                                                              │
│  TOOLS                                                                       │
│  GET    /tools                         List available tools                   │
│  POST   /tools/:name/execute           Execute a tool                        │
│                                                                              │
│  CONNECTORS                                                                  │
│  GET    /connectors                    List installed connectors              │
│  POST   /connectors/:id/execute        Execute connector action              │
│                                                                              │
│  ORGANIZATION                                                                │
│  GET    /org                           Get org details                        │
│  GET    /org/members                   List members                          │
│  GET    /org/teams                     List teams                            │
│  GET    /org/usage                     Get usage statistics                   │
│                                                                              │
│  PLUGINS                                                                     │
│  GET    /plugins                       List installed plugins                 │
│  POST   /plugins/install               Install plugin                        │
│  DELETE /plugins/:id                   Uninstall plugin                      │
│  GET    /plugins/marketplace           Browse marketplace                     │
│                                                                              │
│  WEBHOOKS                                                                    │
│  GET    /webhooks                      List subscriptions                     │
│  POST   /webhooks                      Create subscription                   │
│  DELETE /webhooks/:id                  Delete subscription                   │
│                                                                              │
│  AUDIT                                                                       │
│  GET    /audit/events                  List audit events (paginated)          │
│  GET    /audit/events/:id              Get event details                     │
│  POST   /audit/export                  Export audit log                       │
│                                                                              │
└──────────────────────────────────────────────────────────────────────────────┘
```

### 3.2 API Versioning Strategy

```typescript
/**
 * API versioning via URL path: /v1/, /v2/
 *
 * Versioning policy:
 * - Minor additions (new fields, new endpoints) = no version bump
 * - Breaking changes (removed fields, changed semantics) = major version bump
 * - Deprecation: 12 months notice before removal
 * - Max 2 concurrent API versions supported
 * - Sunset header on deprecated versions: Sunset: Sat, 01 Feb 2028 00:00:00 GMT
 */

// API response envelope
interface APIResponse<T> {
  data: T;
  meta: {
    requestId: string;
    timestamp: string;
    /** API version */
    version: 'v1';
    /** Deprecation warnings */
    deprecations?: {
      field: string;
      message: string;
      removeBy: string;
      replacement: string;
    }[];
  };
  /** Pagination (if applicable) */
  pagination?: {
    page: number;
    perPage: number;
    total: number;
    totalPages: number;
    hasNext: boolean;
    hasPrev: boolean;
  };
}

// Error response
interface APIError {
  error: {
    code: string;            // Machine-readable: 'RATE_LIMIT_EXCEEDED'
    message: string;         // Human-readable
    details?: Record<string, unknown>;
    requestId: string;
    /** Link to documentation */
    docUrl?: string;
  };
}

// Standard HTTP status codes used:
// 200 OK, 201 Created, 204 No Content
// 400 Bad Request, 401 Unauthorized, 403 Forbidden
// 404 Not Found, 409 Conflict, 422 Unprocessable Entity
// 429 Too Many Requests (with Retry-After header)
// 500 Internal Server Error, 503 Service Unavailable
```

### 3.3 Streaming API (Server-Sent Events)

```typescript
// POST /v1/sessions/:id/messages/stream
// Content-Type: application/json
// Accept: text/event-stream

// Request body:
interface StreamMessageRequest {
  content: string;
  /** Override model for this message */
  model?: string;
  /** Stream events to include */
  streamEvents?: ('thinking' | 'content' | 'tool_call' | 'tool_result' | 'done')[];
}

// SSE Response:
// event: thinking
// data: {"content": "Let me analyze this..."}
//
// event: content
// data: {"content": "Here's my analysis:", "delta": "Here's"}
//
// event: tool_call
// data: {"tool": "read_file", "input": {"path": "src/main.ts"}}
//
// event: tool_result
// data: {"tool": "read_file", "result": "...", "duration_ms": 5}
//
// event: content
// data: {"content": "Based on the file...", "delta": "Based on"}
//
// event: usage
// data: {"input_tokens": 1500, "output_tokens": 340, "cost_usd": 0.012}
//
// event: done
// data: {"message_id": "msg_abc123", "finish_reason": "end_turn"}

type StreamEvent =
  | { event: 'thinking'; data: { content: string } }
  | { event: 'content'; data: { content: string; delta: string } }
  | { event: 'tool_call'; data: { tool: string; input: Record<string, unknown> } }
  | { event: 'tool_result'; data: { tool: string; result: unknown; duration_ms: number } }
  | { event: 'usage'; data: { input_tokens: number; output_tokens: number; cost_usd: number } }
  | { event: 'error'; data: { code: string; message: string } }
  | { event: 'done'; data: { message_id: string; finish_reason: string } };
```

---

## 4. SDK Público

### 4.1 SDK Structure

```
@cuervo/sdk/
├── src/
│   ├── index.ts                     # Main exports
│   ├── client.ts                    # CuervoClient class
│   ├── resources/
│   │   ├── sessions.ts              # Session management
│   │   ├── messages.ts              # Message sending
│   │   ├── context.ts               # Context operations
│   │   ├── models.ts                # Model invocation
│   │   ├── tools.ts                 # Tool execution
│   │   ├── connectors.ts           # Connector operations
│   │   ├── projects.ts             # Project management
│   │   ├── plugins.ts              # Plugin management
│   │   ├── webhooks.ts             # Webhook management
│   │   ├── org.ts                  # Organization management
│   │   └── audit.ts               # Audit log access
│   ├── streaming.ts                 # SSE streaming support
│   ├── auth.ts                      # Authentication helpers
│   ├── errors.ts                    # Error types
│   └── types.ts                     # Shared types
├── package.json
├── tsconfig.json
└── README.md
```

### 4.2 SDK Usage Examples

```typescript
import { CuervoClient } from '@cuervo/sdk';

// Initialize client
const cuervo = new CuervoClient({
  apiKey: process.env.CUERVO_API_KEY!,   // or: accessToken: '...'
  baseUrl: 'https://api.cuervo.dev',      // or self-hosted URL
  // Optional: custom timeout, retry config, etc.
});

// ── Session & Messages ─────────────────────────────

// Create a session
const session = await cuervo.sessions.create({
  projectId: 'proj_abc123',
  model: 'claude-sonnet-4-5',
});

// Send a message and get response
const response = await cuervo.messages.send(session.id, {
  content: 'Explain the authentication flow in this project',
});
console.log(response.content);

// Stream a response
const stream = await cuervo.messages.stream(session.id, {
  content: 'Refactor the UserService to use dependency injection',
});

for await (const event of stream) {
  switch (event.event) {
    case 'content':
      process.stdout.write(event.data.delta);
      break;
    case 'tool_call':
      console.log(`\nUsing tool: ${event.data.tool}`);
      break;
    case 'done':
      console.log(`\nDone. Cost: $${event.data.cost_usd}`);
      break;
  }
}

// ── Context ─────────────────────────────────────────

// Resolve context
const ctx = await cuervo.context.resolve({
  keys: ['models.default', 'tools.permissions'],
});

// Semantic search
const results = await cuervo.context.search({
  projectId: 'proj_abc123',
  query: 'authentication middleware',
  limit: 10,
});

// ── Direct Model Invocation ─────────────────────────

const completion = await cuervo.models.invoke({
  model: 'claude-sonnet-4-5',
  messages: [
    { role: 'user', content: 'Generate a unit test for this function...' },
  ],
  maxTokens: 4096,
});

// ── Connectors ──────────────────────────────────────

// Execute a connector action
const prs = await cuervo.connectors.execute('github-instance-1', {
  action: 'pr.list',
  params: { owner: 'myorg', repo: 'myrepo', state: 'open' },
});

// ── Webhooks ────────────────────────────────────────

// Subscribe to events
await cuervo.webhooks.create({
  url: 'https://myapp.com/cuervo-webhook',
  events: ['session:end', 'agent:task_complete'],
  secret: 'whsec_...',
});
```

---

## 5. CLI Extensibility

### 5.1 CLI Plugin Discovery

```yaml
# ~/.cuervo/plugins.yml (user-level plugin config)
plugins:
  # From registry
  - id: com.cuervo.docker
    version: "^1.0.0"

  # From npm
  - id: npm:@acme/cuervo-plugin-terraform
    version: "2.1.0"

  # From local path (development)
  - id: local:./my-plugin
    dev: true

  # From git
  - id: git:https://github.com/user/cuervo-plugin-custom.git
    ref: main
```

### 5.2 CLI Plugin Commands

```bash
# Plugin management
cuervo plugin list                     # List installed plugins
cuervo plugin install <id>             # Install from registry
cuervo plugin install <npm-package>    # Install from npm
cuervo plugin install --dev <path>     # Install local (hot-reload)
cuervo plugin uninstall <id>           # Uninstall
cuervo plugin update <id>             # Update to latest
cuervo plugin update --all            # Update all plugins
cuervo plugin info <id>               # Show plugin details
cuervo plugin enable <id>             # Enable disabled plugin
cuervo plugin disable <id>            # Disable without uninstalling

# Plugin development
cuervo plugin init                     # Scaffold new plugin
cuervo plugin dev                      # Start dev mode (hot-reload)
cuervo plugin test                     # Run plugin tests
cuervo plugin package                  # Package for distribution
cuervo plugin publish                  # Publish to registry
```

---

## 6. Automation Scripts

### 6.1 Automation Engine

```typescript
// domain/entities/automation.ts

/**
 * Automations are user-defined workflows triggered by events.
 * Similar to GitHub Actions but for Cuervo operations.
 */
interface Automation {
  id: string;
  tenantId: string;
  name: string;
  description: string;

  /** Trigger conditions */
  trigger: AutomationTrigger;

  /** Steps to execute */
  steps: AutomationStep[];

  /** Whether this automation is active */
  enabled: boolean;

  /** Execution constraints */
  constraints: {
    maxDurationMs: number;
    maxTokenBudget: number;
    maxCostUSD: number;
    requireApproval: boolean;
  };

  createdBy: string;
  createdAt: string;
  lastRunAt: string | null;
  totalRuns: number;
  successRate: number;
}

type AutomationTrigger =
  | { type: 'event'; event: string; filter?: Record<string, unknown> }
  | { type: 'schedule'; cron: string; timezone: string }
  | { type: 'webhook'; path: string }
  | { type: 'manual' };

type AutomationStep =
  | { type: 'message'; content: string; model?: string }
  | { type: 'tool'; tool: string; input: Record<string, unknown> }
  | { type: 'connector'; connector: string; action: string; params: Record<string, unknown> }
  | { type: 'condition'; if: string; then: AutomationStep[]; else?: AutomationStep[] }
  | { type: 'parallel'; steps: AutomationStep[] }
  | { type: 'wait'; event: string; timeout: number }
  | { type: 'notify'; channel: string; message: string };
```

### 6.2 Automation Examples

```yaml
# .cuervo/automations/auto-review.yml
name: "Auto-Review PRs"
trigger:
  type: event
  event: "github.pr.opened"
  filter:
    base: "main"

steps:
  - type: connector
    connector: github
    action: pr.get
    params:
      owner: "{{ event.repository.owner }}"
      repo: "{{ event.repository.name }}"
      number: "{{ event.pull_request.number }}"

  - type: message
    content: |
      Review this pull request and provide feedback:
      Title: {{ steps[0].result.title }}
      Description: {{ steps[0].result.body }}
      Files changed: {{ steps[0].result.changed_files }}
    model: claude-sonnet-4-5

  - type: connector
    connector: github
    action: pr.review
    params:
      owner: "{{ event.repository.owner }}"
      repo: "{{ event.repository.name }}"
      number: "{{ event.pull_request.number }}"
      body: "{{ steps[1].result }}"
      event: "COMMENT"

constraints:
  maxCostUSD: 0.50
  maxDurationMs: 60000
  requireApproval: false
```

---

## 7. Marketplace

### 7.1 Marketplace Architecture

```typescript
// infrastructure/marketplace/types.ts

interface MarketplaceListing {
  id: string;
  pluginId: string;
  publisher: PublisherInfo;
  name: string;
  description: string;
  longDescription: string;     // Markdown
  icon: string;
  screenshots: string[];
  categories: PluginCategory[];
  tags: string[];

  /** Versioning */
  latestVersion: string;
  versions: VersionInfo[];

  /** Metrics */
  installs: number;
  activeInstalls: number;
  rating: number;              // 0-5
  reviewCount: number;

  /** Trust & safety */
  verified: boolean;           // Publisher verified
  securityAudit: SecurityAuditStatus;

  /** Pricing */
  pricing: {
    model: 'free' | 'paid' | 'freemium';
    monthlyPrice?: number;
    annualPrice?: number;
    trialDays?: number;
  };

  /** Compliance */
  dataProcessing: {
    collectsData: boolean;
    dataTypes: string[];
    privacyPolicyUrl: string;
    gdprCompliant: boolean;
  };

  createdAt: string;
  updatedAt: string;
}

type SecurityAuditStatus =
  | 'not_audited'
  | 'self_assessed'
  | 'community_reviewed'
  | 'professionally_audited';

interface PublisherInfo {
  id: string;
  name: string;
  verified: boolean;
  type: 'individual' | 'organization';
  website: string;
  supportEmail: string;
}

interface VersionInfo {
  version: string;
  releaseDate: string;
  changelog: string;
  minPlatformVersion: string;
  downloadUrl: string;
  checksum: string;          // SHA-256
  size: number;              // bytes
}
```

### 7.2 Plugin Review Process

```
┌─────────────────────────────────────────────────────┐
│           PLUGIN PUBLICATION PIPELINE                │
├─────────────────────────────────────────────────────┤
│                                                      │
│  1. Developer submits plugin                         │
│     │                                                │
│     ▼                                                │
│  2. Automated checks                                 │
│     ├── Manifest validation                          │
│     ├── Dependency audit (npm audit)                 │
│     ├── Static analysis (no eval, no require)        │
│     ├── Permission scope review                      │
│     ├── Bundle size check (< 5MB)                    │
│     └── License compatibility check                  │
│     │                                                │
│     ▼                                                │
│  3. Security scan                                    │
│     ├── Known vulnerability check                    │
│     ├── Malware signature scan                       │
│     ├── Network behavior analysis                    │
│     └── Data exfiltration pattern detection          │
│     │                                                │
│     ▼                                                │
│  4. Human review (for paid / high-permission)        │
│     ├── Code review                                  │
│     ├── Permission justification review              │
│     └── Privacy policy review                        │
│     │                                                │
│     ▼                                                │
│  5. Published (available in marketplace)              │
│     │                                                │
│     ▼                                                │
│  6. Ongoing monitoring                               │
│     ├── Install/uninstall rates                      │
│     ├── Error rate tracking                          │
│     ├── User reports                                 │
│     └── Periodic re-audit (90 days)                  │
│                                                      │
└─────────────────────────────────────────────────────┘
```

---

## 8. Billing & Metering

### 8.1 Plugin Billing Model

```typescript
// infrastructure/billing/plugin-metering.ts

interface PluginMeteringService {
  /**
   * Record usage of a paid plugin.
   * Called by the plugin host after each action.
   */
  recordUsage(event: PluginUsageEvent): Promise<void>;

  /**
   * Get current usage for a tenant/plugin.
   */
  getUsage(tenantId: string, pluginId: string, period: string): Promise<PluginUsage>;

  /**
   * Check if tenant has exceeded plugin quota.
   */
  checkQuota(tenantId: string, pluginId: string): Promise<QuotaStatus>;
}

interface PluginUsageEvent {
  tenantId: string;
  pluginId: string;
  eventType: 'invocation' | 'api_call' | 'storage_write' | 'data_transfer';
  quantity: number;
  metadata: Record<string, unknown>;
  timestamp: string;
}

interface PluginUsage {
  tenantId: string;
  pluginId: string;
  period: string;             // '2026-02'
  invocations: number;
  apiCalls: number;
  storageBytes: number;
  dataTransferBytes: number;
  estimatedCost: number;
}
```

---

## 9. Versioning Strategy

### 9.1 Plugin Versioning

| Component | Versioning | Compatibility |
|-----------|-----------|--------------|
| Plugin API (`@cuervo/plugin-api`) | Semantic versioning | Major = breaking change |
| Connector SDK (`@cuervo/connector-sdk`) | Semantic versioning | Major = breaking change |
| Public SDK (`@cuervo/sdk`) | Semantic versioning | Follows API version |
| REST API | URL path (`/v1/`) | Breaking = new version |
| Plugin manifest schema | Schema version field | Backwards compatible additions |
| Event schemas | Schema version + type field | Backwards compatible |

### 9.2 Deprecation Policy

```typescript
/**
 * API deprecation lifecycle:
 *
 * 1. ANNOUNCE: Add Sunset header + deprecation warning in response
 *    Sunset: Sat, 01 Feb 2028 00:00:00 GMT
 *    Deprecation: true
 *
 * 2. WARN: Log deprecation usage, notify plugin authors via email
 *    Duration: 6 months minimum
 *
 * 3. DISABLE NEW: Stop accepting new integrations on deprecated version
 *    Existing integrations continue to work
 *
 * 4. SUNSET: Return 410 Gone for deprecated endpoints
 *    Duration: 12 months after announce
 */
```

---

## 10. Decisiones Arquitectónicas

| # | Decisión | Alternativa Descartada | Justificación |
|---|----------|----------------------|---------------|
| ADR-E01 | **Plugin sandbox via V8 isolates (vm2/isolated-vm)** | Docker containers per plugin | V8 isolates son ligeros (~10ms startup vs ~1s), low memory, sufficient isolation para JavaScript. Docker overkill para CLI. |
| ADR-E02 | **REST API como API pública principal** | GraphQL-only | REST es más simple, mejor tooling, más accesible. GraphQL como futuro add-on, no como reemplazo. |
| ADR-E03 | **SSE para streaming** | WebSocket | SSE es unidireccional (suficiente para streaming respuestas), funciona sobre HTTP/2, mejor con proxies/firewalls. WebSocket overkill para nuestro caso. |
| ADR-E04 | **Activation events (lazy loading)** | Eager loading de todos los plugins | Lazy loading reduce startup time. Un plugin de Docker no se carga hasta que el usuario ejecuta un comando Docker. |
| ADR-E05 | **JSON Schema para validación** | TypeScript types solo | JSON Schema es runtime-validable, language-agnostic, auto-genera UI forms, usado por OpenAPI/JSON Schema standards. |
| ADR-E06 | **YAML para automations** | JavaScript/TypeScript automations | YAML es declarativo, más seguro (no es código ejecutable), más accesible para non-developers, versionable en git. |
| ADR-E07 | **Marketplace con review process** | Open publication | Security es crítica. Plugins untrusted pueden exfiltrar datos. Review process añade fricción pero protege usuarios. |

---

## 11. Plan de Implementación (Extensibility)

| Sprint | Entregable | Dependencias |
|--------|-----------|-------------|
| S1-S2 | Plugin manifest schema + plugin loader | Project setup |
| S3-S4 | Plugin host + V8 sandbox runtime | Plugin loader |
| S5-S6 | Plugin API surface (context, storage, events, UI) | Plugin host |
| S7-S8 | Command registration + tool registration | Plugin API |
| S9-S10 | Plugin lifecycle management (install, enable, disable, uninstall) | Plugin host |
| S11-S12 | Public REST API (sessions, messages, context) | Core platform |
| S13-S14 | SSE streaming API | REST API |
| S15-S16 | Public SDK (`@cuervo/sdk`) | REST API |
| S17-S18 | Connector SDK (`@cuervo/connector-sdk`) | Integration Fabric |
| S19-S20 | CLI plugin commands + dev mode (hot-reload) | Plugin lifecycle |
| S21-S22 | Automation engine + YAML parser | Event bus |
| S23-S24 | Webhook dispatcher (outgoing) | Event bus |
| S25-S26 | Marketplace backend + plugin submission pipeline | Plugin lifecycle |
| S27-S28 | Plugin billing/metering | Billing infrastructure |
| S29-S30 | Plugin security audit automation | Security team |
