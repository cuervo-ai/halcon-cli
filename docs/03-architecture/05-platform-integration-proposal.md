# Halcon Platform Integration Proposal
## Surfaces · Providers · Protocols · Agent Network

**Fecha**: 2026-03-08
**Baseline**: 19 crates, ~77K LOC, 4,344 tests passing
**Audiencia**: Equipo de ingeniería + arquitectura

---

## 0. Resumen Ejecutivo

Halcon CLI es hoy un agente terminal de producción con 9 superficies de integración funcionales. Este documento propone expandirlo a una **plataforma completa de desarrollo asistido por IA** que cubra 11 superficies de usuario, 6 proveedores de modelos (incluyendo Bedrock, Vertex AI y Foundry), MCP bidireccional, integraciones CI/CD nativas, y una red de agentes con roles lead/teammate.

El análisis forense del código confirma que **la arquitectura actual soporta la expansión** sin reescrituras — todos los gaps son aditivos dentro de patrones ya establecidos.

---

## 1. Estado Actual vs. Objetivo

```
ESTADO HOY (implementado)           OBJETIVO PROPUESTO
─────────────────────────           ─────────────────────────────────────
Terminal CLI          ✅ PROD       Terminal CLI             ✅ mantener
VS Code extension     ✅ PROD       VS Code extension        ✅ mantener
MCP server/client     ✅ PROD       JetBrains plugin         🔵 NUEVO
LSP server            🟡 alpha     Cursor IDE               🔵 NUEVO
Desktop app           🟡 alpha     Desktop app              🟢 completar
JSON-RPC bridge       ✅ PROD       Web (browser)            🔵 NUEVO
API server            ✅ PROD       iOS app                  🔵 NUEVO
                                   Slack integration        🔵 NUEVO
Proveedores:                        Chrome extension         🔵 NUEVO
  Anthropic           ✅ PROD       Voice mode               🔵 NUEVO
  OpenAI              ✅ PROD       Remote Control           🔵 NUEVO
  Ollama              ✅ PROD
  DeepSeek            ✅ PROD       Proveedores:
  Gemini              ✅ PROD         AWS Bedrock            🔵 NUEVO
  ClaudeCode          ✅ PROD         Google Vertex AI       🔵 NUEVO
  OpenAI-compat       ✅ PROD         Microsoft Foundry      🔵 NUEVO
                                     LLM Gateway (proxy)    🔵 NUEVO
Multi-agente:                         Corporate proxy        🔵 NUEVO
  Sub-agent spawning  ✅ PROD
  Orchestrator DAG    ✅ PROD       Agent Network:
  Agent registry      ✅ PROD         Lead/Teammate roles    🔵 NUEVO
  Delegation router   ✅ PROD         Mailbox P2P            🔵 NUEVO
  Shared budget       ✅ PROD         Agent Teams API        🔵 NUEVO
                                     GitHub Actions hook    🔵 NUEVO
CI/CD:                                GitLab CI integration  🔵 NUEVO
  MCP serve headless  ✅ PROD         Scheduled tasks        🔵 NUEVO
  JSON-RPC headless   ✅ PROD
  ci_logs tool        ✅ PROD
  output-format       ❌ missing
```

---

## 2. Arquitectura Objetivo

```
┌──────────────────────────────────────────────────────────────────────────┐
│                        SUPERFICIES DE USUARIO                            │
│                                                                          │
│  Terminal CLI   VS Code   JetBrains   Cursor   Desktop   Web Browser    │
│  iOS App        Slack     Chrome Ext  Voice    Remote Control            │
└─────────────────────────────┬────────────────────────────────────────────┘
                              │
              ┌───────────────▼───────────────────┐
              │         PROTOCOL GATEWAY           │
              │                                   │
              │  JSON-RPC stdin/stdout  (CLI/IDE) │
              │  ACP (Agent Control Protocol)     │
              │  WebSocket ws://                   │
              │  HTTPS REST + SSE                 │
              │  Native Messaging (Chrome)         │
              │  STT/TTS audio stream              │
              └───────────────┬───────────────────┘
                              │
┌─────────────────────────────▼────────────────────────────────────────────┐
│                          HALCON CORE ENGINE                              │
│                                                                          │
│  Agent Loop · Boundary Decision Engine · TerminationOracle              │
│  FASE-2 Security Gate · 60+ Tools · HALCON.md · Auto-Memory             │
│  Lifecycle Hooks · Sub-Agent Registry · Audit Log                       │
│                                                                          │
│  ┌────────────────────────────────────────────────────────────────┐     │
│  │                    AGENT NETWORK LAYER                         │     │
│  │  Lead Agent ↔ Teammate Agents (mailbox P2P)                   │     │
│  │  Sub-agent spawning · Shared budget · Topological DAG         │     │
│  │  Agent Teams API · SDK consumer interface                      │     │
│  └────────────────────────────────────────────────────────────────┘     │
└──────┬──────────────────┬──────────────────────────────┬────────────────┘
       │                  │                              │
┌──────▼──────┐  ┌────────▼──────────────┐  ┌──────────▼────────────┐
│  PROVEEDORES│  │    MCP ECOSYSTEM      │  │    CI/CD LAYER        │
│             │  │                       │  │                       │
│ Anthropic   │  │ COMO CLIENTE:         │  │ GitHub Actions        │
│ OpenAI      │  │   stdio JSON-RPC      │  │ GitLab CI/CD          │
│ Ollama      │  │   HTTP+SSE Bearer     │  │ Scheduled tasks       │
│ DeepSeek    │  │   OAuth 2.1+PKCE      │  │ Headless API server   │
│ Gemini      │  │   GitHub/Jira/Notion  │  │ --output-format flag  │
│ ClaudeCode  │  │                       │  │                       │
│ ─────────── │  │ COMO SERVIDOR:        │  └───────────────────────┘
│ Bedrock 🔵  │  │   stdio + HTTP/axum   │
│ Vertex AI 🔵│  │   Bearer + TTL        │
│ Foundry 🔵  │  │                       │
│ LLM Gw 🔵   │  └───────────────────────┘
└─────────────┘
```

---

## 3. Superficies de Usuario

### 3.1 Superficies Existentes (mantener + mejorar)

#### Terminal CLI — PROD ✅
Sin cambios. El motor central.

#### VS Code Extension — PROD ✅
Mejoras menores:
- Añadir `SubAgentSpawned` / `SubAgentCompleted` como eventos JSON-RPC
- Mostrar árbol de sub-agentes en el panel lateral
- Actualizar a xterm.js 5.5 cuando esté disponible

#### MCP Server — PROD ✅
Completar SSE bidireccional (Phase 2 pendiente en `http_server.rs`):
- `GET /mcp` debe mantener conexión SSE viva y emitir `tool_result` events en tiempo real
- `Mcp-Session-Id` ya implementado — sólo falta el stream handler

#### Desktop App — ALPHA 🟡 → completar a PROD
Las vistas existen como skeleton. Prioridad:
1. `dashboard.rs` — wiring con `halcon-api` `/status` endpoint
2. `agents.rs` — lista desde API + spawn interactivo
3. `tasks.rs` — timeline desde WebSocket events
4. `metrics.rs` — gráficas de tokens/latencia con egui_plot

---

### 3.2 Nuevas Superficies

#### JetBrains Plugin — 🔵 NUEVO
**Protocolo**: ACP (Agent Communication Protocol) — mismo JSON-RPC que VS Code con adaptador.

**Implementación**:
```
halcon-vscode/              # ya existe
halcon-jetbrains/           # NUEVO — Kotlin/IntelliJ SDK
├── src/main/kotlin/
│   ├── HalconPlugin.kt     # Plugin entry point (plugin.xml)
│   ├── HalconProcess.kt    # Spawn halcon --mode json-rpc (misma lógica que TS)
│   ├── HalconToolWindow.kt # Tool window con JCEF (Chromium embed) o Swing terminal
│   ├── ContextCollector.kt # IDE context: active file, PSI diagnostics, VCS
│   └── DiffApplier.kt      # com.intellij.diff.DiffManager integration
└── plugin.xml
```

**Esfuerzo estimado**: 3-4 semanas (reutiliza el protocolo JSON-RPC — sólo adaptar el host).

**Halcon CLI no requiere cambios** — ya expone `--mode json-rpc`.

---

#### Cursor IDE — 🔵 NUEVO
Cursor ya soporta extensiones VS Code. La extensión `halcon-vscode` funciona tal cual si se publica como `.vsix`.

**Mejora específica para Cursor**: Modo `Agent` de Cursor puede invocar Halcon como herramienta MCP.
```json
// .cursor/mcp.json
{
  "mcpServers": {
    "halcon": {
      "command": "halcon",
      "args": ["mcp", "serve"]
    }
  }
}
```

**Esfuerzo estimado**: 0 semanas para instalación básica. 1-2 semanas para integración nativa en Cursor Agent mode.

---

#### Web Browser Interface — 🔵 NUEVO
Interfaz web que se conecta al `halcon-api` HTTP server vía WebSocket.

```
website/                    # ya existe (Astro 5)
├── src/pages/app.astro     # NUEVO — React SPA dentro de Astro
└── src/components/
    ├── ChatPanel.tsx        # xterm.js en browser (misma librería que VS Code)
    ├── AgentTree.tsx        # árbol de sub-agentes activos
    └── ToolFeed.tsx         # stream de tool calls en tiempo real
```

**Protocolo**: WebSocket `ws://localhost:9849/api/v1/ws` con Bearer token.

**halcon-api** ya expone este endpoint — sólo falta el frontend.

**Esfuerzo estimado**: 2-3 semanas (reutiliza halcon-api + Astro website existente).

---

#### iOS App — 🔵 NUEVO
App nativa Swift que consume el `halcon-api` REST + WebSocket.

```
halcon-ios/                 # NUEVO
├── HalconApp.swift         # SwiftUI App entry
├── Views/
│   ├── ChatView.swift      # conversación principal
│   ├── AgentView.swift     # sub-agentes activos
│   └── SettingsView.swift  # server URL + API token
├── Services/
│   ├── HalconAPIClient.swift  # URLSession + Codable para /api/v1/
│   └── WebSocketClient.swift  # URLSessionWebSocketTask para streaming
└── Models/
    └── AgentEvent.swift    # decodifica eventos: token/tool_call/done
```

**Auth**: Bearer token almacenado en Keychain.
**Requisito servidor**: `halcon serve --port 9849` con `HALCON_API_TOKEN`.
**Esfuerzo estimado**: 4-5 semanas.

---

#### Slack Integration — 🔵 NUEVO
Responder a menciones `@halcon` en canales Slack.

**Arquitectura**:
```
Slack Events API (HTTP POST) → halcon-integrations SlackProvider → Agent Loop
```

**Implementación en `halcon-integrations/src/providers/slack.rs`**:
```rust
pub struct SlackProvider {
    bot_token:       String,   // SLACK_BOT_TOKEN
    signing_secret:  String,   // SLACK_SIGNING_SECRET
    verification:    HmacSha256Verifier,
}

impl IntegrationProvider for SlackProvider {
    async fn handle_event(&self, event: InboundEvent) -> Result<OutboundEvent> {
        // 1. Verificar HMAC-SHA256 del request header X-Slack-Signature
        // 2. Extraer mensaje de event.payload["text"]
        // 3. Enviar al agent loop via run_json_rpc_turn() o handle_message()
        // 4. Responder con chat.postMessage a event.channel_id
    }
}
```

**Desafíos**:
- Slack requiere respuesta en <3s → respuesta inmediata "Procesando…" + stream posterior
- Manejo de threads Slack (`thread_ts`)
- Rate limits: Tier 3 (50 req/min)

**Esfuerzo estimado**: 3-4 semanas.

---

#### Chrome Extension — 🔵 NUEVO
Asistente IA en cualquier página web vía Native Messaging.

**Arquitectura**:
```
Chrome Extension (JS) → Native Messaging Host → halcon-cli JSON-RPC
```

```
halcon-chrome/              # NUEVO
├── manifest.json           # Manifest V3
├── background.js           # Service worker + native messaging port
├── content.js              # Inyecta panel lateral en página
├── panel.html              # xterm.js panel (misma UI que VS Code)
└── native-host/
    └── com.cuervo.halcon.json  # Native messaging host manifest
                                # apunta a: halcon --mode json-rpc
```

**Native Messaging Host** (`com.cuervo.halcon.json`):
```json
{
  "name": "com.cuervo.halcon",
  "description": "Halcon AI Agent",
  "path": "/usr/local/bin/halcon",
  "type": "stdio",
  "allowed_origins": ["chrome-extension://<ID>/"]
}
```

**halcon-cli no requiere cambios** — mismo `--mode json-rpc`.

**Esfuerzo estimado**: 2-3 semanas.

---

#### Voice Mode — 🔵 NUEVO
Pipeline STT → Agent Loop → TTS.

**Arquitectura**:
```
Micrófono → Whisper STT → halcon agent loop → TTS → Altavoces
```

**Implementación**:
```
crates/halcon-voice/        # NUEVO
├── src/
│   ├── stt/
│   │   ├── whisper.rs      # Local: whisper-rs (GGML bindings)
│   │   └── openai_stt.rs   # API: POST /v1/audio/transcriptions
│   ├── tts/
│   │   ├── openai_tts.rs   # API: POST /v1/audio/speech (alloy/echo/fable)
│   │   └── local_tts.rs    # Local: tts-rs (Coqui TTS bindings)
│   ├── audio_capture.rs    # cpal crate: cross-platform audio capture
│   ├── vad.rs              # Voice Activity Detection (silero-vad bindings)
│   └── voice_pipeline.rs   # STT → normalize → agent → TTS pipeline
```

**CLI**:
```sh
halcon --voice                    # activa modo voz continuo
halcon voice                      # subcomando explícito
halcon voice --stt openai         # fuerza STT via API
halcon voice --tts local          # fuerza TTS local
```

**`/voice` slash command** dentro del REPL:
```
/voice on           # activa escucha continua
/voice off          # desactiva
/voice status       # estado STT/TTS
```

**Integración con agent loop**:
- Input: audio transcripción inyectada como mensaje de usuario
- Output: texto de respuesta sintetizado y reproducido
- VAD determina fin de utterance (silencio >800ms)

**Esfuerzo estimado**: 5-6 semanas (TTS es el componente más complejo).

---

#### Remote Control — 🔵 NUEVO
Panel de control remoto vía HTTPS polling + WebSocket.

**Implementación**: Es el mismo `halcon-api` + `halcon-desktop` expuesto remotamente.

```
HTTPS_PROXY / mTLS → halcon serve --port 9849 --bind 0.0.0.0
                   → halcon-desktop conecta remotamente
```

**Diferencias vs. desktop local**:
- TLS cert (Let's Encrypt via `rustls` + `acme-client`)
- Auth: Bearer token + optional mTLS client cert
- Rate limiting: `tower::limit::RateLimitLayer`

**Esfuerzo estimado**: 1-2 semanas (reutiliza infraestructura existente).

---

## 4. Proveedores de Modelos

### 4.1 Patrón de implementación existente

Todos los proveedores implementan el trait `ModelProvider`:
```rust
#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn name(&self) -> &str;
    fn supports_tools(&self) -> bool;
    async fn complete(&self, req: ModelRequest) -> Result<ModelResponse>;
    async fn stream(&self, req: ModelRequest) -> Result<ModelStream>;
}
```

El `http.rs` de `halcon-providers` ya provee:
- `build_client()` — reqwest con timeouts, pooling, user-agent
- `backoff_delay_with_jitter()` — exponential backoff
- `is_retryable_status()` — 429, 5xx
- `parse_retry_after()` — header parsing

**Añadir un nuevo proveedor = ~300-500 líneas** siguiendo el patrón de `openai_compat.rs`.

---

### 4.2 AWS Bedrock — 🔵 NUEVO

**Variables de entorno**:
```sh
CLAUDE_CODE_USE_BEDROCK=1
AWS_REGION=us-east-1
AWS_ACCESS_KEY_ID=...          # o IAM role / OIDC
AWS_SECRET_ACCESS_KEY=...
```

**Implementación**:
```
crates/halcon-providers/src/bedrock/
├── mod.rs          # BedrockProvider: impl ModelProvider
├── auth.rs         # AWS SigV4 signing (aws-sigv4 crate)
├── request.rs      # Bedrock InvokeModel request format
└── stream.rs       # Bedrock streaming response (EventStream)
```

**Endpoint**: `https://bedrock-runtime.{region}.amazonaws.com/model/{model-id}/invoke-with-response-stream`

**Auth**: AWS Signature Version 4 — disponible vía `aws-sigv4` crate (ya en Cargo registry).

**Modelos soportados**:
- `anthropic.claude-3-5-sonnet-20241022-v2:0`
- `anthropic.claude-3-haiku-20240307-v1:0`
- `anthropic.claude-opus-4-5`

**Cross-region inference**: `us.anthropic.claude-*` (prefix `us.` para routing cross-region).

**Config**:
```toml
[models.providers.bedrock]
enabled      = true
region       = "us-east-1"
cross_region = true   # usar prefijo us./eu./ap.
```

**Esfuerzo estimado**: 2-3 semanas.

---

### 4.3 Google Vertex AI — 🔵 NUEVO

**Variables de entorno**:
```sh
CLAUDE_CODE_USE_VERTEX=1
CLOUD_ML_REGION=us-east5
ANTHROPIC_VERTEX_PROJECT_ID=my-project
```

**Implementación**:
```
crates/halcon-providers/src/vertex/
├── mod.rs          # VertexProvider: impl ModelProvider
├── auth.rs         # GCP Workload Identity / service account
├── request.rs      # Vertex AI API format (Anthropic Messages API compatible)
└── stream.rs       # SSE stream parsing
```

**Endpoint**: `https://{REGION}-aiplatform.googleapis.com/v1/projects/{PROJECT}/locations/{REGION}/publishers/anthropic/models/{MODEL}:streamRawPredict`

**Auth**: GCP Application Default Credentials (ADC) via `gcp-auth` crate o `google-oauth2` crate.

**Modelos**:
- `claude-3-5-sonnet-v2@20241022`
- `claude-3-5-haiku@20241022`
- `claude-opus-4@20250514`

**Config**:
```toml
[models.providers.vertex]
enabled    = true
project_id = "my-gcp-project"
region     = "us-east5"
```

**Esfuerzo estimado**: 2-3 semanas.

---

### 4.4 Microsoft Azure AI Foundry — 🔵 NUEVO

**Variables de entorno**:
```sh
CLAUDE_CODE_USE_FOUNDRY=1
AZURE_AI_ENDPOINT=https://my-hub.services.ai.azure.com/models
AZURE_AI_API_KEY=...           # o Entra ID managed identity
```

**Implementación**:
```
crates/halcon-providers/src/azure_foundry/
├── mod.rs          # AzureFoundryProvider: impl ModelProvider
├── auth.rs         # Entra ID token (azure-identity crate) o API key
└── request.rs      # Azure AI Inference API (OpenAI-compatible format)
```

**Endpoint**: `{AZURE_AI_ENDPOINT}/chat/completions?api-version=2024-05-01-preview`

**Auth**: `api-key` header o `Authorization: Bearer <EntraID token>`.

**Nota**: Azure AI Foundry usa el mismo formato que OpenAI — se puede implementar como wrapper de `openai_compat.rs` con headers adicionales.

**Esfuerzo estimado**: 1 semana (OpenAI-compat reutilizable).

---

### 4.5 LLM Gateway / Proxy — 🔵 NUEVO

Para rutas de corporate proxy o gateways internos:

**Variables de entorno**:
```sh
ANTHROPIC_BEDROCK_BASE_URL=https://my-llm-gateway.corp.com/v1
```

**Implementación**: Ya existe `openai_compat.rs` — sólo necesita:
1. Detectar `ANTHROPIC_BEDROCK_BASE_URL` en config
2. Override de `api_base` en `AnthropicProvider`
3. Soporte para `HTTPS_PROXY` via reqwest `ProxyBuilder`

**Corporate proxy con mTLS**:
```toml
[network]
https_proxy     = "https://proxy.corp.com:8080"
mtls_cert_path  = "/etc/halcon/client.pem"
mtls_key_path   = "/etc/halcon/client.key"
ca_bundle_path  = "/etc/halcon/corporate-ca.pem"
```

**Esfuerzo estimado**: 1 semana.

---

## 5. MCP Ecosystem (mejoras)

### 5.1 SSE Bidireccional Completo

**Archivo**: `crates/halcon-mcp/src/http_server.rs`

Actualmente el `GET /mcp` envía un único `endpoint` event y mantiene conexión viva pero vacía.

**Implementación completa**:
```rust
// En McpHttpServer::handle_sse()
// Subscribir al EventBus interno del servidor
// Por cada tool_call/tool_result que ocurra en sesiones activas:
//   → emitir SSE event: "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/call\",\"params\":{...}}\n\n"
```

**Esfuerzo**: 1 semana.

### 5.2 Nuevos MCP servers preconfigurados

```toml
# ~/.halcon/mcp-presets.toml — NUEVO
[[presets]]
name = "github"
url  = "https://api.githubcopilot.com/mcp/v1"
auth = { type = "oauth", provider = "github" }

[[presets]]
name = "linear"
command = ["npx", "@linear/mcp-server"]
auth = { type = "bearer", env = "LINEAR_API_KEY" }

[[presets]]
name = "notion"
command = ["npx", "@notionhq/mcp-server"]
auth = { type = "bearer", env = "NOTION_TOKEN" }
```

**Comando**:
```sh
halcon mcp preset add github   # instala preset con OAuth
halcon mcp preset list         # muestra disponibles
```

---

## 6. CI/CD Layer

### 6.1 `--output-format` flag — ALTA PRIORIDAD

**Archivos a modificar**:
- `crates/halcon-cli/src/main.rs` — añadir flag global
- `crates/halcon-cli/src/render/sink.rs` — nuevo `CiSink`

```rust
// main.rs
#[arg(long, value_enum, default_value = "human")]
output_format: OutputFormat,

#[derive(ValueEnum)]
enum OutputFormat {
    Human,   // ANSI color text (actual default)
    Json,    // newline-delimited JSON events
    Junit,   // JUnit XML (para CI test reporters)
    Plain,   // sin color, texto plano
}
```

**`CiSink`** emite eventos estructurados:
```json
{"type":"session_start","timestamp":"...","session_id":"..."}
{"type":"tool_call","tool":"bash","input":"cargo test"}
{"type":"tool_result","tool":"bash","success":true,"output":"...","duration_ms":4200}
{"type":"response","text":"All tests pass."}
{"type":"session_end","rounds":3,"tokens_used":1840,"cost_usd":0.004}
```

**Esfuerzo**: 1 semana.

---

### 6.2 GitHub Actions Integration

**Archivo nuevo**: `.github/actions/halcon/action.yml`

```yaml
name: Halcon AI Agent
description: Run Halcon AI agent in GitHub Actions
inputs:
  prompt:
    description: Task for the agent
    required: true
  model:
    description: Model override
    default: claude-sonnet-4-6
  max-turns:
    description: Maximum agent loop turns
    default: '20'
outputs:
  result:
    description: Agent response (JSON)
  session-id:
    description: Session ID for trace retrieval
runs:
  using: composite
  steps:
    - name: Install Halcon
      shell: bash
      run: |
        curl -fsSL https://raw.githubusercontent.com/cuervo-ai/halcon-cli/main/scripts/install-binary.sh | sh
    - name: Run agent
      shell: bash
      run: |
        halcon --output-format json \
               --max-turns ${{ inputs.max-turns }} \
               --model "${{ inputs.model }}" \
               "${{ inputs.prompt }}" \
          | tee /tmp/halcon-output.json
        # Parse result from last done event
        RESULT=$(cat /tmp/halcon-output.json | grep '"type":"response"' | tail -1 | jq -r '.text')
        echo "result=$RESULT" >> $GITHUB_OUTPUT
      env:
        ANTHROPIC_API_KEY: ${{ env.ANTHROPIC_API_KEY }}
```

**Uso en workflow**:
```yaml
- uses: cuervo-ai/halcon-cli/.github/actions/halcon@main
  with:
    prompt: "Review the PR diff and check for security issues"
  env:
    ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
```

**Esfuerzo**: 1 semana (depende del `--output-format` flag).

---

### 6.3 GitLab CI Integration

**Dockerfile** (`docker/halcon-ci.Dockerfile`):
```dockerfile
FROM debian:bookworm-slim
RUN curl -fsSL https://raw.githubusercontent.com/cuervo-ai/halcon-cli/main/scripts/install-binary.sh | sh
ENTRYPOINT ["halcon"]
```

**Uso en `.gitlab-ci.yml`**:
```yaml
halcon-review:
  image: ghcr.io/cuervo-ai/halcon-cli:latest
  script:
    - halcon --output-format json "Review staged changes for issues"
  variables:
    ANTHROPIC_API_KEY: $ANTHROPIC_API_KEY
  rules:
    - if: $CI_PIPELINE_SOURCE == "merge_request_event"
```

**Esfuerzo**: 1 semana (incluye Dockerfile + CI example).

---

### 6.4 Scheduled Tasks

**Schema en AsyncDatabase**:
```sql
CREATE TABLE scheduled_tasks (
    id          TEXT PRIMARY KEY,  -- UUID
    agent_name  TEXT NOT NULL,     -- referencia a .halcon/agents/
    instruction TEXT NOT NULL,
    cron_expr   TEXT NOT NULL,     -- "0 9 * * 1-5"
    last_run_at INTEGER,           -- Unix timestamp
    next_run_at INTEGER NOT NULL,
    enabled     BOOL DEFAULT TRUE,
    created_at  INTEGER NOT NULL
);
```

**Scheduler** (background tokio task en agent loop init):
```rust
// crates/halcon-cli/src/repl/scheduler.rs — NUEVO
pub struct AgentScheduler {
    db:       Arc<AsyncDatabase>,
    registry: Arc<AgentRegistry>,
}

impl AgentScheduler {
    pub async fn run(&self) {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            let due = self.db.get_due_tasks(Utc::now()).await?;
            for task in due {
                self.spawn_agent(task).await;
                self.db.update_next_run(task.id, next_cron_time(&task.cron_expr)).await?;
            }
        }
    }
}
```

**CLI**:
```sh
halcon schedule add --agent code-reviewer --cron "0 9 * * 1-5" \
                    --instruction "Review any open PRs for security issues"
halcon schedule list
halcon schedule disable <id>
halcon schedule run <id>    # ejecutar manualmente fuera de horario
```

**Cron parsing**: `cron` crate (ya en Cargo registry).

**Esfuerzo**: 2-3 semanas.

---

## 7. Agent Network Layer

### 7.1 Lead / Teammate Roles

**Estado actual**: todos los agentes son hijos del orchestrador, sin roles.

**Propuesta**:

```rust
// crates/halcon-core/src/types/orchestrator.rs — MODIFICAR
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentRole {
    Lead,        // coordina el equipo, acceso completo
    Teammate,    // trabaja en paralelo, alcance limitado
    Specialist,  // especialista en un dominio, invocado bajo demanda
    Observer,    // audit-only, no ejecuta herramientas
}

// crates/halcon-core/src/types/orchestrator.rs — MODIFICAR SubAgentTask
pub struct SubAgentTask {
    // ... campos existentes ...
    pub role: AgentRole,                    // NUEVO
    pub team_id: Option<Uuid>,              // NUEVO — agrupa agentes en equipo
    pub mailbox_id: Option<Uuid>,           // NUEVO — buzón de mensajes
}
```

**Comportamiento del Lead**:
- Puede leer el estado de todos los teammates
- Puede enviar mensajes a cualquier teammate
- Puede cancelar tareas de teammates
- Recibe resumen final de todos los resultados

**Comportamiento del Teammate**:
- Recibe contexto inicial del lead
- Puede enviar mensajes al lead (no a otros teammates directamente)
- Límites de herramientas más estrictos
- Timeout reducido (se hereda de lead con factor 0.6)

---

### 7.2 Mailbox P2P

```rust
// crates/halcon-storage/src/mailbox.rs — NUEVO
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailboxMessage {
    pub id:          Uuid,
    pub from_agent:  Uuid,
    pub to_agent:    Option<Uuid>,   // None = broadcast al equipo
    pub topic:       String,
    pub payload:     serde_json::Value,
    pub timestamp:   DateTime<Utc>,
    pub acked:       bool,
    pub ttl_secs:    u32,            // default 300
}

pub struct AsyncMailbox {
    db: Arc<AsyncDatabase>,
}

impl AsyncMailbox {
    pub async fn send(&self, msg: MailboxMessage) -> Result<()>;
    pub async fn poll(&self, agent_id: Uuid, timeout: Duration) -> Result<Vec<MailboxMessage>>;
    pub async fn ack(&self, msg_id: Uuid) -> Result<()>;
    pub async fn broadcast(&self, team_id: Uuid, msg: MailboxMessage) -> Result<()>;
}
```

**Integración en orchestrator.rs**:
```rust
// Al final de cada wave, antes de iniciar la siguiente:
// 1. Cada agente completado puede publicar mensajes a su mailbox
// 2. Los agentes de la siguiente wave pueden consumir mensajes relevantes
// 3. El lead agent siempre tiene acceso al mailbox global del equipo
```

**Esfuerzo**: 3-4 semanas.

---

### 7.3 Agent Teams API

**Endpoints HTTP** (extensión de `halcon-api`):

```
POST   /api/v1/teams                     # crear equipo
GET    /api/v1/teams/{id}                # estado del equipo
DELETE /api/v1/teams/{id}                # disolver equipo

POST   /api/v1/teams/{id}/agents         # añadir agente al equipo
GET    /api/v1/teams/{id}/agents         # listar agentes activos
GET    /api/v1/teams/{id}/agents/{aid}   # estado de agente específico
DELETE /api/v1/teams/{id}/agents/{aid}   # remover agente

GET    /api/v1/teams/{id}/context        # contexto compartido del equipo
PUT    /api/v1/teams/{id}/context        # actualizar contexto

POST   /api/v1/teams/{id}/messages       # enviar mensaje al equipo
GET    /api/v1/teams/{id}/messages       # leer mensajes (polling o SSE)

WS     /api/v1/teams/{id}/stream         # WebSocket: eventos en tiempo real
```

**SDK consumer** (Claude Code SDK pattern):
```rust
// crates/halcon-client/src/team.rs — NUEVO
pub struct HalconTeam {
    client: HalconClient,
    team_id: Uuid,
}

impl HalconTeam {
    pub async fn spawn_lead(&self, instruction: &str) -> Result<AgentHandle>;
    pub async fn spawn_teammate(&self, instruction: &str) -> Result<AgentHandle>;
    pub async fn broadcast(&self, message: &str) -> Result<()>;
    pub async fn wait_all(&self) -> Result<Vec<AgentResult>>;
    pub async fn context(&self) -> Result<serde_json::Value>;
}
```

**CLI**:
```sh
halcon team create --name "pr-review"
halcon team add-agent --team <id> --agent code-reviewer --role lead
halcon team add-agent --team <id> --agent security-scanner --role teammate
halcon team run --team <id>
halcon team status --team <id>
halcon team logs --team <id> --follow
```

**Esfuerzo**: 4-6 semanas.

---

## 8. Diagrama de Secuencia: Request Flow Completo

```
Usuario/IDE/Slack/iOS
     │
     ▼
PROTOCOL GATEWAY
[JSON-RPC / ACP / HTTPS / WS / Native Msg / STT]
     │
     ▼
     ┌─────────────────────────────────┐
     │     INPUT NORMALIZATION        │
     │  InputNormalizer.normalize()   │
     │  Language detection / ZW strip │
     └──────────────┬──────────────────┘
                    │
                    ▼
     ┌─────────────────────────────────┐
     │   BOUNDARY DECISION ENGINE     │
     │  IntentPipeline.resolve()      │
     │  → effective_max_rounds        │
     │  → routing_mode (Q/B/Deep)     │
     └──────────────┬──────────────────┘
                    │
          ┌─────────▼─────────┐
          │  AgentRole=Lead?  │
          └──┬─────────────┬──┘
             │ YES         │ NO
             ▼             ▼
     ┌──────────────┐ ┌──────────────┐
     │ TeamOrchest. │ │ AgentLoop    │
     │ spawn team   │ │ (existente)  │
     │ + mailbox    │ └──────┬───────┘
     └──────┬───────┘        │
            │                │
            ▼                ▼
     ┌─────────────────────────────────┐
     │   PROVIDER ROUND               │
     │  ModelProvider.stream()        │
     │  [Anthropic/Bedrock/Vertex/    │
     │   Foundry/OpenAI/Ollama/...]   │
     └──────────────┬──────────────────┘
                    │
                    ▼
     ┌─────────────────────────────────┐
     │   TOOL EXECUTION               │
     │  FASE-2 gate (18 patterns)     │
     │  executor.execute_batch()      │
     │  60+ tools with RiskTier       │
     └──────────────┬──────────────────┘
                    │
                    ▼
     ┌─────────────────────────────────┐
     │   CONVERGENCE                  │
     │  SynthesisGate → Oracle        │
     │  RoutingAdaptor (T1-T4)        │
     └──────────────┬──────────────────┘
                    │
                    ▼
OUTPUT LAYER
[ANSI stream / JSON-RPC events / TTS audio / Slack message / HTTP SSE]
```

---

## 9. Roadmap de Implementación

### Fase 1 — Fundación (4-6 semanas)
**Prioridad**: Unblocking para todas las fases siguientes.

| Item | Esfuerzo | Impacto |
|------|----------|---------|
| `--output-format json\|junit\|plain` | 1 sem | Habilita CI/CD y todas las superficies headless |
| AWS Bedrock provider | 2-3 sem | Clientes enterprise (AWS OIDC/IAM) |
| Google Vertex AI provider | 2-3 sem | Clientes GCP |
| Azure Foundry provider | 1 sem | Clientes Azure |
| MCP SSE bidireccional completo | 1 sem | MCP streaming correcto |

**Tests nuevos**: ~150 tests

---

### Fase 2 — Superficies IDE (4-5 semanas)
**Prioridad**: Alcance de desarrolladores.

| Item | Esfuerzo | Impacto |
|------|----------|---------|
| JetBrains plugin (Kotlin) | 3-4 sem | +30% developer reach |
| Cursor IDE (VSIX + MCP preset) | 1-2 sem | Usuarios Cursor (rápido) |
| GitHub Actions integration | 1 sem | CI/CD nativo |
| GitLab CI (Dockerfile + example) | 1 sem | CI/CD enterprise |

**Tests nuevos**: ~80 tests

---

### Fase 3 — Plataforma Web y Móvil (5-6 semanas)
**Prioridad**: Acceso ubicuo.

| Item | Esfuerzo | Impacto |
|------|----------|---------|
| Web browser interface (Astro SPA) | 2-3 sem | Uso sin instalación |
| Desktop app views completion | 2-3 sem | Control plane nativo |
| iOS app (SwiftUI) | 4-5 sem | Acceso móvil |
| Chrome extension | 2-3 sem | Contexto de navegador |

**Tests nuevos**: ~200 tests

---

### Fase 4 — Agent Network (6-8 semanas)
**Prioridad**: Diferenciador competitivo principal.

| Item | Esfuerzo | Impacto |
|------|----------|---------|
| Lead/Teammate roles | 2-3 sem | Base para todos los casos multi-agente |
| Mailbox P2P | 3-4 sem | Coordinación real entre agentes |
| Agent Teams API (REST + WS) | 3-4 sem | Integración externa con el equipo |
| Scheduled tasks (cron) | 2-3 sem | Automatización continua |
| `halcon team` CLI commands | 1 sem | UX para teams |

**Tests nuevos**: ~400 tests

---

### Fase 5 — Canales y Voz (5-6 semanas)
**Prioridad**: Integración en workflows existentes.

| Item | Esfuerzo | Impacto |
|------|----------|---------|
| Slack integration | 3-4 sem | Teams que ya usan Slack |
| Voice mode (STT+TTS) | 5-6 sem | Accesibilidad + manos libres |
| Remote control (TLS + auth) | 1-2 sem | Acceso remoto seguro |

**Tests nuevos**: ~250 tests

---

## 10. Gaps Técnicos Detallados

### Nuevas dependencias necesarias

```toml
# Proveedores cloud
aws-sigv4 = "1"            # Bedrock SigV4 signing
gcp-auth = "0.10"          # Vertex AI ADC
azure-identity = "0.20"    # Foundry Entra ID

# Voice
cpal = "0.15"              # cross-platform audio capture
whisper-rs = "0.10"        # local STT (GGML)
tts-rs = "0.4"             # local TTS (Coqui)

# Scheduled tasks
cron = "0.12"              # cron expression parsing
tokio-cron-scheduler = "0.10"

# JetBrains (Kotlin — proyecto separado)
# No afecta Cargo.toml

# Chrome extension (JS — proyecto separado)
# No afecta Cargo.toml
```

---

### Modificaciones de `PolicyConfig` necesarias

```rust
// crates/halcon-core/src/types/policy_config.rs — añadir campos
pub struct PolicyConfig {
    // ... campos existentes ...

    // Nuevos proveedores cloud
    #[serde(default)] pub enable_bedrock: bool,
    #[serde(default)] pub enable_vertex: bool,
    #[serde(default)] pub enable_azure_foundry: bool,

    // Voice
    #[serde(default)] pub enable_voice: bool,
    #[serde(default = "default_stt_provider")]
    pub stt_provider: SttProvider,          // Local | OpenAI
    #[serde(default = "default_tts_provider")]
    pub tts_provider: TtsProvider,          // Local | OpenAI | Azure

    // Agent teams
    #[serde(default)] pub enable_agent_teams: bool,
    #[serde(default = "default_max_team_size")]
    pub max_team_size: u8,                  // default 5
    #[serde(default = "default_mailbox_ttl")]
    pub mailbox_ttl_secs: u32,              // default 300

    // Scheduled tasks
    #[serde(default)] pub enable_scheduler: bool,
    #[serde(default = "default_scheduler_poll")]
    pub scheduler_poll_secs: u64,           // default 60
}
```

---

### Nuevas variables de entorno

```sh
# Bedrock
CLAUDE_CODE_USE_BEDROCK=1
AWS_REGION=us-east-1
AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY / AWS_SESSION_TOKEN
ANTHROPIC_BEDROCK_BASE_URL          # LLM gateway override

# Vertex AI
CLAUDE_CODE_USE_VERTEX=1
CLOUD_ML_REGION=us-east5
ANTHROPIC_VERTEX_PROJECT_ID=my-project
GOOGLE_APPLICATION_CREDENTIALS=/path/to/service-account.json

# Azure Foundry
CLAUDE_CODE_USE_FOUNDRY=1
AZURE_AI_ENDPOINT=https://...
AZURE_AI_API_KEY=... / AZURE_CLIENT_ID+AZURE_TENANT_ID (Entra ID)

# Voice
HALCON_STT_PROVIDER=openai|local
HALCON_TTS_PROVIDER=openai|local|azure
HALCON_VOICE_MODEL=whisper-1          # STT model
HALCON_TTS_VOICE=alloy                # TTS voice

# Agent Teams
HALCON_TEAM_API_TOKEN=...
HALCON_TEAM_SERVER=ws://localhost:9849/api/v1/teams

# Slack
SLACK_BOT_TOKEN=xoxb-...
SLACK_SIGNING_SECRET=...
SLACK_WEBHOOK_URL=https://hooks.slack.com/...

# Proxy
HTTPS_PROXY=https://proxy.corp.com:8080
HALCON_MTLS_CERT=/etc/halcon/client.pem
HALCON_MTLS_KEY=/etc/halcon/client.key
```

---

## 11. Invariantes de Seguridad — No Negociables

Independientemente de la superficie, protocolo, o proveedor, estos invariantes se mantienen:

1. **FASE-2 gate** — 18 patrones catastróficos bloqueados en el executor, no en el provider ni en el protocolo
2. **TBAC** — cada tool declara su RiskTier; las destructivas requieren confirmación (configurable)
3. **Audit log** — toda invocación de herramienta se registra en SQLite con HMAC-SHA256
4. **Keychain** — credenciales nunca en config files; siempre en OS keychain
5. **PII detection** — configurable warn/block/redact, independiente de la superficie
6. **Hooks FASE-2 independence** — los lifecycle hooks no pueden bypassear FASE-2
7. **Rate limiting** — todas las superficies HTTP heredan `tower::limit::RateLimitLayer`
8. **Session isolation** — cada sesión tiene UUID único; no hay cross-contaminación de contexto

---

## 12. Referencias

- Código base auditado: `crates/halcon-cli/src/repl/` (337 archivos)
- Tests actuales: 4,344 passing (halcon-cli: 4,307, halcon-mcp: 92, otros: 45+)
- Transport layer auditado: `json_rpc.rs`, `mcp_serve.rs`, `http_server.rs`, `lsp.rs`
- Provider layer auditado: `crates/halcon-providers/src/` (8 providers)
- Agent networking auditado: `orchestrator.rs`, `delegation.rs`, `supervisor.rs`
- Integration framework: `crates/halcon-integrations/src/` (framework, sin impls)
- Runtime federation: `crates/halcon-runtime/src/federation/` (definiciones, no integrado)
