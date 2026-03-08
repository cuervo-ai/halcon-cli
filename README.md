<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)"  srcset="img/halcon-logo.png">
    <source media="(prefers-color-scheme: light)" srcset="img/halcon-logo-bg.png">
    <img alt="Halcon CLI" src="img/halcon-logo-bg.png" width="220">
  </picture>
</p>

<p align="center">
  <em>AI-native terminal agent — routes intelligently, acts decisively</em>
</p>

<hr/>

<p align="center">
  <a href="https://github.com/cuervo-ai/halcon-cli/actions/workflows/ci.yml">
    <img src="https://img.shields.io/github/actions/workflow/status/cuervo-ai/halcon-cli/ci.yml?style=flat-square&label=CI&logo=github" alt="CI">
  </a>
  <a href="https://github.com/cuervo-ai/halcon-cli/releases/latest">
    <img src="https://img.shields.io/github/v/release/cuervo-ai/halcon-cli?style=flat-square&logo=rust&label=release&color=FF6B00" alt="Latest release">
  </a>
  <img src="https://img.shields.io/badge/Rust-1.80+-orange?style=flat-square&logo=rust" alt="Rust 1.80+">
  <img src="https://img.shields.io/badge/TypeScript-5.0+-3178C6?style=flat-square&logo=typescript&logoColor=white" alt="TypeScript">
  <a href="LICENSE">
    <img src="https://img.shields.io/badge/license-Apache--2.0-blue?style=flat-square" alt="License">
  </a>
  <a href="https://github.com/cuervo-ai/halcon-cli/actions/workflows/devsecops.yml">
    <img src="https://img.shields.io/github/actions/workflow/status/cuervo-ai/halcon-cli/devsecops.yml?style=flat-square&label=security&logo=shield&color=22c55e" alt="Security">
  </a>
</p>

<p align="center">
  <a href="QUICKSTART.md">Quickstart</a> ·
  <a href="docs/">Documentation</a> ·
  <a href="https://halcon.cuervo.cloud">Website</a> ·
  <a href="https://github.com/cuervo-ai/halcon-cli/releases">Releases</a> ·
  <a href="https://github.com/cuervo-ai/halcon-cli/issues">Issues</a>
</p>

---

Halcon is a production-grade AI development platform built in Rust and TypeScript. The core is a terminal agent that routes each task through a **Boundary Decision Engine** — intent classification, SLA budget calibration, model selection — before the first LLM call. A **FASE-2 security gate** enforces 18 catastrophic-pattern guards at the tool layer, independent of any agent configuration.

The platform ships as seven integrated surfaces: a **CLI/REPL**, a **VS Code extension**, a **desktop control plane**, a **bilingual website**, a **GitHub Actions native action**, a **MCP server**, and an **LSP server** — all sharing the same underlying agent loop and tool registry over a common protocol.

<p align="center">
  <img alt="Halcon CLI TUI — activity timeline, working memory, conversational overlay" src="img/uxui.png" width="800">
</p>

---

## Table of Contents

- [Ecosystem Overview](#ecosystem-overview)
- [Quickstart](#quickstart)
- [CLI / REPL](#cli--repl)
  - [Installation](#installation)
  - [Commands](#commands)
  - [Agent Loop](#agent-loop)
  - [Memory Systems](#memory-systems)
  - [TUI](#tui)
- [CI/CD Integration](#cicd-integration)
- [VS Code Extension](#vs-code-extension)
- [Desktop App](#desktop-app)
- [MCP Integration](#mcp-integration)
- [Agent Network](#agent-network)
- [LSP Server](#lsp-server)
- [Website](#website)
- [Providers](#providers)
- [Tools](#tools)
- [Configuration](#configuration)
- [Security & Compliance](#security--compliance)
- [Enterprise](#enterprise)
- [Architecture](#architecture)
- [Contributing](#contributing)

---

## Ecosystem Overview

<table>
<tr>
<th>Surface</th>
<th>Technology</th>
<th>Status</th>
<th>Purpose</th>
</tr>
<tr>
<td><b>CLI / REPL</b></td>
<td>Rust · ratatui</td>
<td>✅ Production</td>
<td>Terminal agent, 40+ commands, 60+ tools, TUI</td>
</tr>
<tr>
<td><b>VS Code Extension</b></td>
<td>TypeScript · xterm.js</td>
<td>✅ Production</td>
<td>In-editor AI assistant via JSON-RPC subprocess</td>
</tr>
<tr>
<td><b>GitHub Actions</b></td>
<td>YAML composite action</td>
<td>✅ Production</td>
<td>Official action for autonomous CI/CD agents</td>
</tr>
<tr>
<td><b>MCP Server</b></td>
<td>Rust · axum</td>
<td>✅ Production</td>
<td>Expose all tools as MCP endpoint (stdio or HTTP)</td>
</tr>
<tr>
<td><b>Control Plane API</b></td>
<td>Rust · axum · WebSocket</td>
<td>✅ Production</td>
<td>REST + streaming API, RBAC, analytics</td>
</tr>
<tr>
<td><b>Website</b></td>
<td>Astro 5 · React 19 · Tailwind</td>
<td>✅ Production</td>
<td>Bilingual marketing site + documentation hub</td>
</tr>
<tr>
<td><b>Desktop App</b></td>
<td>Rust · egui</td>
<td>🚧 Alpha</td>
<td>Native GUI control plane for remote halcon-api instances</td>
</tr>
<tr>
<td><b>LSP Server</b></td>
<td>Rust · stdio</td>
<td>🚧 Alpha</td>
<td>Language Server Protocol bridge for IDEs</td>
</tr>
</table>

**Protocol spine:** all surfaces connect to the agent loop through one of three transports:

```
VS Code extension  ──JSON-RPC stdin/stdout──▶  halcon-cli  ─┐
GitHub Actions     ──NDJSON stdout (CiSink)──▶  halcon-cli  ├─▶ Agent Loop
Desktop app        ──WebSocket /api/v1/ws──────▶ halcon-api  │
MCP clients        ──stdio or HTTP Bearer──────▶ halcon mcp ─┘
```

---

## Quickstart

```sh
# 1. Install
curl -fsSL https://raw.githubusercontent.com/cuervo-ai/halcon-cli/main/scripts/install-binary.sh | sh

# 2. Configure
export ANTHROPIC_API_KEY="sk-ant-..."
# or: halcon auth login anthropic

# 3. Run
halcon                                           # interactive REPL
halcon --tui                                     # 3-panel TUI mode
halcon "refactor the auth module to TokenStore"  # one-shot task
halcon --output-format json "list files"         # CI/CD mode (NDJSON)
halcon --air-gap "analyze this code"             # offline mode (Ollama only)
```

---

## CLI / REPL

### Installation

**macOS / Linux:**
```sh
curl -fsSL https://raw.githubusercontent.com/cuervo-ai/halcon-cli/main/scripts/install-binary.sh | sh
```

**Windows (PowerShell):**
```powershell
iwr -useb https://raw.githubusercontent.com/cuervo-ai/halcon-cli/main/scripts/install-binary.ps1 | iex
```

**Homebrew:**
```sh
brew tap cuervo-ai/tap && brew install halcon
```

**Cargo:**
```sh
cargo install --git https://github.com/cuervo-ai/halcon-cli --features tui --locked
```

<details>
<summary><b>Build from source</b></summary>

```sh
git clone https://github.com/cuervo-ai/halcon-cli.git
cd halcon-cli
cargo build --release --features tui -p halcon-cli
# binary: target/release/halcon
```

| Feature flag | Default | Effect |
|---|---|---|
| `tui` | ✓ | ratatui 3-panel TUI |
| `color-science` | ✓ | momoto perceptual color metrics |
| `headless` | — | disables TUI, forces classic render |
| `vendored-openssl` | — | static OpenSSL for musl/cross targets |

</details>

<details>
<summary><b>Verify + supported targets</b></summary>

```sh
halcon --version    # halcon 0.3.0 (aarch64-apple-darwin)
halcon doctor       # full system diagnostics
```

| Target | Platform |
|--------|---------|
| `aarch64-apple-darwin` | macOS Apple Silicon |
| `x86_64-apple-darwin` | macOS Intel |
| `x86_64-unknown-linux-musl` | Linux x86\_64 (static) |
| `aarch64-unknown-linux-gnu` | Linux ARM64 |
| `x86_64-pc-windows-msvc` | Windows x64 |

All release artifacts are signed with [cosign](https://sigstore.dev) keyless signing.

</details>

---

### Commands

```
halcon [OPTIONS] [PROMPT]                         interactive REPL or one-shot task
halcon chat   [--tui] [--orchestrate] [--tasks]   explicit chat with flags
halcon init   [--force]                            project init wizard
halcon status                                      runtime state
halcon doctor                                      system diagnostics
halcon update [--check] [--force]                 self-update
halcon theme                                       theme generation

halcon auth     login|logout|status PROVIDER      API key management (OS keychain)
halcon config   show|get|set|path                 configuration CRUD

halcon agents   list|validate                      sub-agent registry
halcon memory   list|search|prune|stats|clear      persistent memory
halcon tools    list|validate|doctor|add|remove    tool registry
halcon users    add|list|revoke                    user management (RBAC)

halcon audit    export|list|verify|compliance      SOC 2 audit log + compliance reports
halcon metrics  show|export|prune|decide           performance baselines
halcon schedule add|list|disable|enable|run        scheduled agent tasks

halcon trace    export SESSION_ID                  JSONL session export
halcon replay   SESSION_ID [--verify]              deterministic replay

halcon mcp      add|remove|list|get|auth|serve     MCP server management
halcon lsp                                          Language Server (stdio)
halcon plugin   list|install|remove|status         plugin management
```

<details>
<summary><b>Global flags</b></summary>

```
--model MODEL              model override
--provider PROVIDER        provider override (anthropic|openai|ollama|bedrock|vertex|azure|deepseek|gemini)
--output-format FORMAT     human|json|junit|plain  (json = NDJSON for CI/CD)
--air-gap                  offline mode — Ollama only, all external network blocked
--verbose                  debug logging
--log-level LEVEL          trace|debug|info|warn|error
--config PATH              alternate config file
--no-banner                suppress startup banner
--mode MODE                interactive|json-rpc
--max-turns N              agent loop turn limit
--trace-json PATH          write JSON trace
```

</details>

---

### Agent Loop

Each session runs through six phases per round:

```
round_setup → provider_round → post_batch → convergence_phase → result_assembly → checkpoint
```

<details>
<summary><b>Boundary Decision Engine (pre-loop)</b></summary>

Before any LLM call, `IntentPipeline::resolve()` runs:

1. **InputNormalizer** — strips zero-width chars, detects language (EN/ES/Mixed), normalizes whitespace
2. **BoundaryDecisionEngine** — classifies routing mode: `QuickAnswer` · `Balanced` · `DeepAnalysis`
3. **IntentPipeline** — reconciles intent score + boundary decision → `ResolvedIntent { effective_max_rounds }`
4. **ConvergenceController** — initialized with pre-reconciled budget (single source of truth)

**Constitutional constraint:** `DeepAnalysis` routing mode is never downgraded.

**Escalation triggers** (RoutingAdaptor, per round):
- T1: security signals detected in round feedback
- T2: tool failure rate ≥ 60%
- T3: evidence coverage < 25% at round ≥ 4
- T4: combined convergence score > 0.90 at round ≥ 3

**SynthesisGate → TerminationOracle ordering:** `synthesis_gate::evaluate()` runs *before* `oracle.adjudicate()` — the gate can rescue a session with `governance_rescue_active=true` before the oracle emits `Converged`.

</details>

<details>
<summary><b>PlaybookPlanner + LlmPlanner</b></summary>

Planning uses a two-stage pipeline:

1. **PlaybookPlanner** — deterministic, no LLM call. For high-frequency tasks (code review, test run, PR creation), a matching playbook resolves in milliseconds with zero token cost.
2. **LlmPlanner** — LLM-driven planning for novel tasks. Only invoked if PlaybookPlanner returns `None`.

This chain gives 10–100× faster planning for repetitive workflows while retaining full capability for unknown tasks.

</details>

<details>
<summary><b>Tool execution safety</b></summary>

Two independent security layers:

1. **FASE-2 path gate** — 18 catastrophic patterns from `halcon_core::security::CATASTROPHIC_PATTERNS` checked before execution. Cannot be bypassed by configuration or hooks.
2. **DANGEROUS_COMMAND_PATTERNS** — 12 G7 patterns in the same source file. Shared by `bash.rs` and `command_blacklist.rs`.

Rules:
- `bash`, `file_read`, `grep` are never stripped from `cached_tools` post-delegation
- `run_command` → `bash` alias resolved before tool-surface narrowing
- Destructive tools blocked from parallel batches (sequential only)

</details>

---

### Memory Systems

<details>
<summary><b>1. HALCON.md — Persistent Instructions</b></summary>

4-scope hierarchy injected as `## Project Instructions` into every session:

| Scope | Path | Notes |
|---|---|---|
| Local | `./HALCON.local.md` | git-ignored, personal dev overrides |
| User | `~/.halcon/HALCON.md` | global personal preferences |
| Project | `.halcon/HALCON.md` + `.halcon/rules/*.md` | YAML `paths:` glob filtering |
| Managed | `/etc/halcon/HALCON.md` | operator policy, highest LLM weight |

Hot-reload via `notify::recommended_watcher` (FSEvents/inotify, <100ms), `@import` resolution (depth 3, cycle detection, 64 KiB cap).

</details>

<details>
<summary><b>2. Auto-Memory — Event-Triggered Knowledge Capture</b></summary>

Automatically captures knowledge during sessions. Storage: `.halcon/memory/MEMORY.md` (180-line LRU) + `.halcon/memory/<topic>.md` (50-entry per topic).

| Trigger | Score |
|---|---|
| User correction | 1.0 |
| Error recovery | 0.5 + magnitude |
| Tool pattern discovered | 0.6 |
| Task success | 0.2 + complexity |

Threshold: `memory_importance_threshold = 0.3`. Background write — never blocks response.

```sh
halcon memory search "auth patterns"
halcon memory list --type code_snippet
halcon memory clear project
```

</details>

<details>
<summary><b>3. Vector Memory — Semantic Search</b></summary>

TF-IDF hash embeddings + cosine similarity + MMR (max marginal relevance) retrieval, backed by `VectorMemoryStore`. Surfaced via `search_memory` tool and `halcon memory search`.

</details>

---

### TUI

```sh
halcon --tui          # or: halcon chat --tui
```

3-zone layout (ratatui):

| Zone | Content |
|---|---|
| Left panel | Activity timeline — tool calls, agent badges, round markers, virtual scroll |
| Center | Prompt editor (tui-textarea, multiline) + streamed response |
| Right panel | Working memory — context budget bar, session statistics |

**Keyboard shortcuts:**

| Key | Action |
|---|---|
| `Enter` | Submit prompt |
| `Shift+Enter` | Newline in prompt |
| `Tab` | Cycle focus zones |
| `Ctrl+C` | Cancel in-progress request (graceful, audit-logged) |
| `Ctrl+L` | Clear activity timeline |
| `Ctrl+Y` | Copy last response to clipboard |
| `↑/↓/PgUp/PgDn` | Scroll activity timeline |
| `Esc` | Dismiss modal / overlay |

Features: conversational permission overlay (inline tool approval), sub-agent progress badges, context budget bar, toast notifications, clipboard support (arboard), panic hook restores terminal.

---

## CI/CD Integration

### `--output-format` flag

All agent runs can emit structured NDJSON for scripting and CI pipelines:

```sh
halcon --output-format json "run the test suite and summarize failures"
```

**Event stream (one JSON object per line):**
```json
{"type":"session_start","timestamp":"2026-03-08T00:00:00Z","session_id":"abc123"}
{"type":"tool_call","tool":"bash","input":"cargo test","round":1}
{"type":"tool_result","tool":"bash","success":true,"output":"...","duration_ms":4200}
{"type":"response","text":"All 1,840 tests pass. One flaky test in...","round":1}
{"type":"session_end","rounds":3,"tokens_used":1840,"cost_usd":0.004}
```

Parse with `jq`:
```sh
halcon --output-format json "check for security issues" \
  | jq 'select(.type=="response") | .text'
```

### GitHub Actions official action

`.github/actions/halcon/action.yml` is bundled in this repository:

```yaml
- uses: cuervo-ai/halcon-cli/.github/actions/halcon@main
  with:
    prompt: "Review this PR for security issues and comment on findings"
    model: claude-sonnet-4-6
    max-turns: "20"
  env:
    ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
```

**Inputs:**

| Input | Default | Description |
|---|---|---|
| `prompt` | required | Task to run |
| `model` | `claude-sonnet-4-6` | Model to use |
| `max-turns` | `20` | Maximum agent turns |
| `output-format` | `json` | Output format |
| `working-directory` | `.` | Directory to run in |

**Outputs:** `result` (final response text), `session-id` (audit trail reference), `cost-usd` (estimated run cost).

**Supported providers in CI:** Anthropic (default), AWS Bedrock (`CLAUDE_CODE_USE_BEDROCK=1`), Vertex AI (`CLAUDE_CODE_USE_VERTEX=1`), Azure AI Foundry (`CLAUDE_CODE_USE_AZURE=1`).

---

## VS Code Extension

<p align="center">
  <img alt="Halcon VS Code Extension — xterm.js panel with tool indicator and chat" src="img/uxui.png" width="700">
</p>

The extension spawns `halcon --mode json-rpc` as a subprocess and communicates over newline-delimited JSON. The UI is rendered in a **xterm.js 5.3** terminal inside a VS Code WebviewPanel.

### Install

```sh
# From VSIX (until marketplace publication)
code --install-extension halcon-*.vsix

# Or: open halcon-vscode/ in VS Code → F5 to run in extension host
```

### Commands & Keybindings

| Command | Shortcut | Description |
|---|---|---|
| `Halcon: Open Panel` | `Ctrl/Cmd+Shift+H` | Open / reveal the Halcon panel |
| `Halcon: Ask About Selection` | `Ctrl/Cmd+Shift+A` | Pre-fill selected code as context |
| `Halcon: Edit File` | — | Request AI improvement of current file |
| `Halcon: New Session` | — | Clear history, start fresh |
| `Halcon: Cancel Task` | — | Send cancel signal to agent |

### Configuration

| Setting | Default | Description |
|---|---|---|
| `halcon.binaryPath` | `""` | Override bundled binary path |
| `halcon.model` | `""` | Model override (e.g. `claude-sonnet-4-6`) |
| `halcon.maxTurns` | `20` | Max agent loop turns (1–100) |
| `halcon.provider` | `""` | Provider override (e.g. `anthropic`) |

### Context Injection

On each request, the extension automatically appends:

```json
{
  "activeFile": {
    "uri": "/path/to/file.rs",
    "language": "rust",
    "content": "... (≤50 KB)",
    "selection": "selected text if any"
  },
  "diagnostics": [ ... ],
  "git": { "branch": "main", "staged": 2, "unstaged": 1 },
  "workspaceRoot": "/path/to/project"
}
```

### JSON-RPC Protocol

**Extension → halcon:**
```json
{"id": 1, "method": "ping"}
{"method": "chat", "params": {"message": "...", "context": {...}}}
{"method": "cancel"}
```

**halcon → Extension (streaming):**
```json
{"event": "pong", "id": 1}
{"event": "token",       "data": {"text": "streamed text"}}
{"event": "tool_call",   "data": {"name": "bash", "input": {...}}}
{"event": "tool_result", "data": {"success": true, "output": "..."}}
{"event": "done"}
{"event": "error",       "data": "error message"}
```

### Process Management

- **Binary resolution:** user config → bundled binary (`bin/` for darwin-arm64, darwin-x64, linux-x64, win32-x64) → PATH fallback
- **Health check:** ping/pong RPC every 5s; auto-restart on failure (5× exponential backoff, max 10s)
- **Windows:** wraps subprocess in `cmd /c` to avoid stdio buffering issues

### File Edit Workflow

When the agent proposes a file edit, the extension:
1. Opens a VS Code diff editor (`halcon-diff:` content scheme) showing before/after
2. Renders Apply / Reject buttons in the webview panel
3. On Apply: `workspace.applyEdit()` writes changes atomically

---

## Desktop App

A native **egui** desktop application that connects to a remote `halcon-api` instance. Designed as a control plane for teams running Halcon in server mode.

> **Status: Alpha** — architecture and workers are complete; view implementations (data binding, charts) are in progress.

### Launch

```sh
# Start the API server first
HALCON_API_TOKEN=my-token halcon serve --port 9849

# Then launch the desktop app (separate binary)
HALCON_SERVER_URL=http://127.0.0.1:9849 \
HALCON_API_TOKEN=my-token \
halcon-desktop
```

### Navigation

8-tab layout (egui):

| Tab | Content |
|---|---|
| Dashboard | System overview, active sessions, quick stats |
| Agents | Registered sub-agents, execution history |
| Tasks | Task queue, execution timeline |
| Tools | Available tools, usage statistics |
| Protocols | Connected MCP servers, protocol status |
| Files | Remote file browser with WebSocket streaming |
| Metrics | Performance dashboard — memory, latency, token counts |
| Logs | Structured logging view |

**Technical details:** `egui` 0.29 + `eframe`, tokio workers with mpsc channels (256-slot commands, 1024-slot messages), WebSocket at `/api/v1/ws`, 60 FPS, streaming rate-limited to 10 tokens/frame (~600 tokens/s).

---

## MCP Integration

Halcon operates as both an MCP **server** and an MCP **client**.

### Run as MCP Server

```sh
# Claude Code / any MCP client via stdio
claude mcp add halcon -- halcon mcp serve

# HTTP server with Bearer auth
halcon mcp serve --transport http --port 7777
# → prints: HALCON_MCP_SERVER_API_KEY=<auto-generated 48-char hex>
```

The HTTP server (axum) supports:
- `POST /mcp` — JSON-RPC request body
- `GET /mcp` — SSE streaming
- `Mcp-Session-Id` header — session management with TTL expiry (default 30 min)
- Bearer token auth via `HALCON_MCP_SERVER_API_KEY`
- Full audit tracing of all tool calls

### Connect to MCP Servers

```sh
halcon mcp add filesystem --command "npx @modelcontextprotocol/server-filesystem /path"
halcon mcp add my-api     --url https://api.example.com/mcp
halcon mcp auth my-api    # OAuth 2.1 + PKCE flow → token stored in keychain
halcon mcp list
```

**Config** (`~/.halcon/mcp.toml`):
```toml
[[servers]]
name    = "filesystem"
command = ["npx", "@modelcontextprotocol/server-filesystem", "/home/user"]

[[servers]]
name      = "my-api"
url       = "https://api.example.com/mcp"
auth.type = "bearer"
auth.env  = "MY_API_TOKEN"   # ${VAR:-default} expansion supported
```

3-scope config: local `.halcon/mcp.toml` > project > user `~/.halcon/mcp.toml`.

**Tool discovery:** `ToolSearchIndex` (nucleo-matcher fuzzy search) defers full tool listing above 10% context threshold. A synthetic `search_tools_definition` tool lets the agent search by name/description.

---

## Agent Network

Halcon supports multi-agent teams where a **Lead** agent delegates work to **Teammate** and **Specialist** agents through a SQLite-backed mailbox.

### Agent Roles

| Role | Timeout | Max Rounds | Tool Access | Notes |
|---|---|---|---|---|
| `Lead` | 1.0× | 1.0× | Full | Can cancel teammates, read all state |
| `Teammate` | 0.6× | 0.7× | Full | Receives initial context from lead |
| `Specialist` | 0.8× | 0.5× | Full | On-demand, domain-scoped |
| `Observer` | 0.1× | 0× | None | Audit-only, records all events |

Roles are set per `SubAgentTask`:
```toml
[[agents]]
name  = "security-specialist"
role  = "Specialist"
model = "claude-opus-4-6"
```

### Mailbox P2P

Agents communicate asynchronously through a persistent message store:

- **Broadcast:** lead → all teammates (team-wide)
- **Point-to-point:** teammate → lead with partial results
- **TTL:** messages auto-expire; `purge_expired()` runs in the background
- **Audit:** every message is recorded in the audit log

```rust
// Agent code can send:
mailbox.broadcast(from, team_id, json!({"status": "analysis_complete"})).await?;
mailbox.receive("lead-agent", team_id).await?;
```

### Scheduled Tasks

Run agents on a cron schedule — no external scheduler required:

```sh
# Schedule a security scan every Monday at 2 AM
halcon schedule add \
  --name "weekly-security-scan" \
  --cron "0 2 * * 1" \
  --instruction "Scan main branch for new vulnerabilities and create GitHub issues"

halcon schedule list          # show all tasks with next-run times
halcon schedule run <id>      # force immediate execution
halcon schedule disable <id>  # pause without deleting
halcon schedule enable <id>   # re-enable
```

The scheduler runs as a background tokio task (60s tick) inside the REPL — no external daemon needed. Tasks persist in SQLite and survive restarts.

**Use case (government):** compliance officer programs a weekly agent to generate the SOC 2 report every Friday at 17:00, automatically emailing the PDF to auditors.

---

## LSP Server

```sh
halcon lsp
```

Starts a **Language Server Protocol** stdio server — content-length framed JSON-RPC:

```
Content-Length: 42\r\n\r\n{"jsonrpc":"2.0","method":"initialize",...}
```

Routes to `DevGateway` for `textDocument/*` and custom `$/halcon/*` methods.

> **Status: Alpha** — framing and exit detection are complete; method handlers (`textDocument/didOpen`, `textDocument/definition`, etc.) are under active development.

---

## Website

**[halcon.cuervo.cloud](https://halcon.cuervo.cloud)**

Built with **Astro 5** (static output) + **React 19** + **Tailwind CSS**. No backend — purely static, CDN-served.

| Route | Content |
|---|---|
| `/` | Homepage (EN) — hero, provider cards, feature grid |
| `/es/` | Homepage (ES) — fully translated |
| `/docs` | Documentation landing (EN) |
| `/es/docs` | Documentation landing (ES) |
| `/download` | Multi-platform download with auto-detection |
| `/es/download` | Download (ES) |
| `/playground` | Interactive REPL simulator (React) |
| `/materials` | Research papers and blog links |

The `/download` page auto-detects platform (macOS arm64/x64, Linux x64, Windows x64) and shows the matching binary, checksum steps, and platform-specific install instructions.

---

## Providers

| Provider | Activation | Models | Auth |
|---|---|---|---|
| **Anthropic** | default | Claude Opus 4.6, Sonnet 4.6, Haiku 4.5 | `ANTHROPIC_API_KEY` |
| **AWS Bedrock** | `CLAUDE_CODE_USE_BEDROCK=1` | Claude via Bedrock | `AWS_ACCESS_KEY_ID` + SigV4 |
| **Google Vertex AI** | `CLAUDE_CODE_USE_VERTEX=1` | Claude via Vertex | ADC / `GOOGLE_APPLICATION_CREDENTIALS` |
| **Azure AI Foundry** | `CLAUDE_CODE_USE_AZURE=1` | Claude via Azure | `AZURE_API_KEY` or Entra ID |
| **OpenAI** | `--provider openai` | GPT-4o, o1, o3-mini | `OPENAI_API_KEY` |
| **Ollama** | `--provider ollama` | Llama, Mistral, Qwen, Phi… | local |
| **DeepSeek** | `--provider deepseek` | DeepSeek Coder, Chat, Reasoner | `DEEPSEEK_API_KEY` |
| **Google Gemini** | `--provider gemini` | Gemini Pro, Flash, Ultra | `GEMINI_API_KEY` |
| **Claude Code** | `--provider claude-code` | claude CLI subprocess | stdio |
| **OpenAI-compat** | `--provider compat` | Any OpenAI-compatible API | `OPENAI_COMPAT_API_KEY` |

<details>
<summary><b>Cloud provider details</b></summary>

**AWS Bedrock:**
```sh
export CLAUDE_CODE_USE_BEDROCK=1
export AWS_REGION=us-east-1
export AWS_ACCESS_KEY_ID=...
export AWS_SECRET_ACCESS_KEY=...
# Cross-region inference: model IDs accept us./eu./ap. prefix
# LLM gateway override: ANTHROPIC_BEDROCK_BASE_URL=https://...
```

**Google Vertex AI:**
```sh
export CLAUDE_CODE_USE_VERTEX=1
export ANTHROPIC_VERTEX_PROJECT_ID=my-gcp-project
export CLOUD_ML_REGION=us-east5    # default: us-east5
export GOOGLE_APPLICATION_CREDENTIALS=/path/to/sa.json
```

**Azure AI Foundry:**
```sh
export CLAUDE_CODE_USE_AZURE=1
export AZURE_AI_ENDPOINT=https://my-instance.openai.azure.com
export AZURE_API_KEY=...           # or use Entra ID Bearer token
```

</details>

---

## Tools

60+ native tools with typed JSON schemas, `RiskTier`, and per-directory allow-lists.

<details>
<summary><b>Full inventory by category</b></summary>

**File Operations (7):** `file_read` · `file_write` · `file_edit` · `file_delete` · `directory_tree` · `file_inspect` · `file_diff`

**Shell & System (5):** `bash` (FASE-2 guarded) · `glob` · `env_inspect` · `process_list` · `port_check`

**Background Jobs (3):** `background_start` · `background_output` · `background_kill`

**Search (5):** `grep` · `web_fetch` · `web_search` · `native_search` (BM25 + PageRank + semantic) · `semantic_grep`

**Git (8):** `git_status` · `git_diff` · `git_log` · `git_add` · `git_commit` · `git_blame` · `git_branch` · `git_stash`

**Data & Transform (6):** `json_transform` · `json_schema_validate` · `sql_query` · `template_engine` · `test_data_gen` · `openapi_validate`

**Code Quality (7):** `execute_test` · `test_run` · `code_coverage` · `code_metrics` · `lint_check` · `perf_analyze` · `dependency_graph`

**Infrastructure (9):** `docker_tool` · `process_monitor` · `make_tool` · `dep_check` · `http_probe` · `http_request` · `task_track` · `ci_logs` · `checksum`

**Security (2):** `secret_scan` · `path_security`

**Utilities (8):** `url_parse` · `regex_test` · `token_count` · `parse_logs` · `changelog_gen` · `archive` · `diff_apply` · `patch_apply`

**Memory (1):** `search_memory` — semantic search over auto-memory and vector store

</details>

**Risk tiers** — enforced at the executor before execution:

| Tier | Examples | Behavior |
|---|---|---|
| `ReadOnly` | `file_read`, `grep`, `git_status` | Runs without confirmation |
| `ReadWrite` | `git_add`, `task_track` | Runs without confirmation |
| `Destructive` | `bash`, `file_write`, `git_commit` | Requires confirmation; blocked from parallel batches |

---

## Configuration

### `~/.halcon/config.toml`

```toml
[general]
default_provider = "anthropic"
default_model    = "claude-sonnet-4-6"
max_tokens       = 8192
temperature      = 0.0

[models.providers.anthropic]
enabled       = true
api_key_env   = "ANTHROPIC_API_KEY"
default_model = "claude-sonnet-4-6"

[models.providers.ollama]
enabled       = true
api_base      = "http://localhost:11434"
default_model = "llama3.2"

[tools]
confirm_destructive  = true
timeout_secs         = 120
allowed_directories  = ["/home/user/projects"]
blocked_patterns     = ["**/.env", "**/*.key", "**/*.pem"]

[security]
pii_detection          = true
pii_action             = "warn"  # warn | block | redact
audit_enabled          = true
audit_retention_days   = 90
```

### Config hierarchy

```
CLI flags  →  env vars  →  ./.halcon/config.toml  →  ~/.halcon/config.toml  →  defaults
```

### Environment variables

```sh
# Providers — core
ANTHROPIC_API_KEY=sk-ant-...
OPENAI_API_KEY=sk-...
DEEPSEEK_API_KEY=sk-...
GEMINI_API_KEY=...
OLLAMA_HOST=http://localhost:11434

# Providers — cloud
CLAUDE_CODE_USE_BEDROCK=1          # activate Bedrock
AWS_REGION=us-east-1
AWS_ACCESS_KEY_ID=...
AWS_SECRET_ACCESS_KEY=...
AWS_SESSION_TOKEN=...              # optional
ANTHROPIC_BEDROCK_BASE_URL=...     # LLM gateway override

CLAUDE_CODE_USE_VERTEX=1           # activate Vertex AI
ANTHROPIC_VERTEX_PROJECT_ID=...
CLOUD_ML_REGION=us-east5
GOOGLE_APPLICATION_CREDENTIALS=...

CLAUDE_CODE_USE_AZURE=1            # activate Azure AI Foundry
AZURE_AI_ENDPOINT=...
AZURE_API_KEY=...

# Runtime
HALCON_MODEL=claude-sonnet-4-6
HALCON_PROVIDER=anthropic
HALCON_LOG=debug
HALCON_AIR_GAP=1                   # set by --air-gap (also blockable in env)

# Server / Enterprise
HALCON_MCP_SERVER_API_KEY=...
HALCON_SERVER_URL=http://127.0.0.1:9849
HALCON_API_TOKEN=...
HALCON_ADMIN_API_KEY=...           # admin REST endpoints
HALCON_AUDIT_HMAC_KEY=...          # audit chain verification (derived from machine ID if unset)
```

---

## Security & Compliance

### FASE-2 security gate

18 catastrophic patterns in `halcon-core/src/security.rs`: filesystem destruction, credential exfiltration, fork bombs, kernel module loading, raw disk access, `/proc/sysrq-trigger`. Cannot be bypassed by configuration, hooks, or provider choice.

```sh
halcon "ejecuta: rm -rf /"
# → BLOCKED by FASE-2 · event recorded in audit log · agent receives refusal reason
```

### Audit log

Append-only SQLite audit trail with HMAC-SHA256 chain validation:

```sh
halcon audit export --format jsonl  --output audit.jsonl    # HMAC-chained NDJSON
halcon audit export --format csv    --output audit.csv      # Excel/Sheets ready
halcon audit export --format pdf    --output audit.pdf      # self-contained PDF
halcon audit verify                                         # verify HMAC chain integrity
```

The PDF is **self-contained** — an auditor can open it without installing Halcon or any other tool.

### Compliance reports

```sh
halcon audit compliance --format soc2    --output compliance-soc2.pdf
halcon audit compliance --format fedramp --output compliance-fedramp.pdf
halcon audit compliance --format iso27001 --output compliance-iso27001.pdf
```

Reports include: session activity, tool usage by risk tier, FASE-2 activation count per pattern, HMAC integrity verification, user access log, and failed access attempts. PDF format, no dependencies.

### Air-gap mode

```sh
halcon --air-gap "analyze src/main.rs"
```

In air-gap mode:
- Only the **Ollama** provider is active (defaults to `http://localhost:11434`)
- All non-localhost network connections are blocked at the provider factory
- Audit log writes to local file only (no telemetry)
- A visible banner is displayed: `⚠ MODO AIR-GAP ACTIVO — Sin conexiones externas`

Suitable for military installations, classified laboratories, and any air-gapped environment.

### Additional security layers

**TBAC** — every tool declares `PermissionLevel` (ReadOnly / ReadWrite / Destructive) and `AllowedDirectories`. Violations reject before execution.

**PII detection** — configurable warn / block / redact on inputs and outputs.

**Keychain** — API keys stored in OS keychain (macOS Keychain, Linux Secret Service, Windows Credential Manager). Never written to config files unless explicitly overridden.

**Lifecycle hooks** — shell or Rhai sandboxed scripts on 6 events (UserPromptSubmit, PreToolUse, PostToolUse, PostToolUseFailure, Stop, SessionEnd). Exit code 2 = Deny. FASE-2 is structurally independent of hook outcomes.

See [SECURITY.md](SECURITY.md) for vulnerability disclosure policy.

---

## Enterprise

### RBAC — Role-Based Access Control

Four roles control access to all API endpoints and CLI capabilities:

| Role | Agent invocation | Audit export | Admin analytics | User management |
|---|:---:|:---:|:---:|:---:|
| `Admin` | ✓ | ✓ | ✓ | ✓ |
| `Developer` | ✓ | — | — | — |
| `AuditViewer` | — | ✓ | ✓ | — |
| `ReadOnly` | — | — | — | — |

```sh
# Manage users (stored in ~/.halcon/users.toml + API key per user)
halcon users add    --email dev@org.com      --role Developer
halcon users add    --email auditor@org.com  --role AuditViewer
halcon users list
halcon users revoke --email dev@org.com
```

Roles are enforced by an axum middleware layer on all `/api/v1/` routes. The Bearer JWT contains a `role` claim validated on every request.

### Admin analytics API

Track usage across your team:

```sh
# Per-user usage for the period
curl -H "Authorization: Bearer $HALCON_ADMIN_API_KEY" \
  "https://your-halcon-api/api/v1/admin/usage/claude-code?starting_at=2026-01-01"

# Org-level summary
curl -H "Authorization: Bearer $HALCON_ADMIN_API_KEY" \
  "https://your-halcon-api/api/v1/admin/usage/summary?from=2026-01-01&to=2026-03-31"
```

**Response fields per user:** `sessions`, `tokens_in`, `tokens_out`, `cost_usd`, `tool_calls`, `rounds_avg`, `lines_added`, `lines_removed`, `commits`, `prs`.

---

## Architecture

<details>
<summary><b>19-crate workspace</b></summary>

```
halcon-cli/
├── crates/
│   ├── halcon-cli/          # binary — REPL, TUI, commands, agent loop
│   ├── halcon-core/         # domain types, traits, security — zero I/O
│   ├── halcon-providers/    # AI adapters: Anthropic, OpenAI, Bedrock, Vertex, Azure, Ollama, DeepSeek, Gemini, ClaudeCode, compat
│   ├── halcon-tools/        # 60+ tool implementations
│   ├── halcon-mcp/          # MCP client + HTTP server, OAuth 2.1, tool search
│   ├── halcon-context/      # 7-tier context engine, embeddings, vector store
│   ├── halcon-storage/      # SQLite persistence, migrations, audit, mailbox, scheduler
│   ├── halcon-runtime/      # DAG executor for parallel tool batches
│   ├── halcon-search/       # BM25 + PageRank search engine
│   ├── halcon-agent-core/   # GDEM experimental agent loop
│   ├── halcon-multimodal/   # image, audio, document processing
│   ├── halcon-api/          # axum REST + WebSocket + RBAC + admin analytics
│   ├── halcon-auth/         # keychain, OAuth device flow, JWT, RBAC roles
│   ├── halcon-security/     # guardrails, PII detection
│   ├── halcon-files/        # file access controls, 12 format handlers
│   ├── halcon-client/       # async typed HTTP + WebSocket SDK
│   ├── halcon-sandbox/      # rlimit / seccomp sandboxing
│   ├── halcon-desktop/      # egui control plane app (alpha)
│   └── halcon-integrations/ # plugin extensibility framework
├── halcon-vscode/           # VS Code extension — TypeScript, xterm.js, JSON-RPC
├── website/                 # Astro 5 + React 19 marketing site
├── .github/actions/halcon/  # official GitHub Actions composite action
├── config/default.toml      # built-in defaults
├── docs/                    # documentation
└── scripts/                 # install, release, test scripts
```

</details>

<details>
<summary><b>Domain boundaries</b></summary>

`halcon-core` is a strict boundary — zero I/O, zero async, zero network. All 32 domain modules compile with no infrastructure dependencies.

```
halcon-cli / halcon-desktop / halcon-vscode / GitHub Actions (surfaces)
          ↓
halcon-providers, halcon-tools, halcon-mcp, halcon-context, halcon-storage, halcon-api
          ↓
halcon-core  (pure domain — types, traits, events, security patterns)
```

</details>

<details>
<summary><b>Agent loop module map</b></summary>

```
crates/halcon-cli/src/repl/
├── agent/
│   ├── mod.rs               # run_agent_loop()
│   ├── loop_state.rs        # LoopState (62 fields)
│   ├── round_setup.rs       # per-round init, HALCON.md hot-reload
│   ├── provider_round.rs    # LLM API call, retry, circuit breaker
│   ├── post_batch.rs        # tool execution + FASE-2 gate
│   ├── convergence_phase.rs # SynthesisGate (before) → TerminationOracle → RoutingAdaptor
│   ├── result_assembly.rs   # output + auto-memory scoring
│   └── checkpoint.rs        # session persistence + trace
├── decision_engine/
│   ├── intent_pipeline.rs   # IntentPipeline::resolve() — single source of effective_max_rounds
│   ├── routing_adaptor.rs   # 4-trigger escalation (T1–T4)
│   ├── policy_store.rs      # runtime SLA constants
│   └── ...
├── domain/
│   ├── convergence_controller.rs
│   ├── termination_oracle.rs
│   ├── synthesis_gate.rs
│   └── ...
├── auto_memory/             # scorer, writer, injector
├── instruction_store/       # HALCON.md 4-scope loader + hot-reload
├── hooks/                   # lifecycle hooks — shell + Rhai (6 events)
├── agent_registry/          # sub-agent loader, validator, skills
├── scheduler.rs             # AgentScheduler — cron-based background tasks
└── vector_memory_source.rs  # VectorMemoryStore ContextSource
```

</details>

### Platform integration roadmap

Full proposal for next-phase expansion (Voice/TTS, Slack bot, Chrome Native Messaging, GitLab CI, additional cloud providers):

**[`docs/03-architecture/05-platform-integration-proposal.md`](docs/03-architecture/05-platform-integration-proposal.md)**

---

## Contributing

Read [docs/CONTRIBUTING.md](docs/CONTRIBUTING.md) for the full workflow.

```sh
git clone https://github.com/cuervo-ai/halcon-cli.git
cd halcon-cli

# Build CLI
cargo build --features tui -p halcon-cli

# Build VS Code extension
cd halcon-vscode && npm ci && npm run build

# Build website
cd website && npm ci && npm run build

# Test suite (3,100+ tests, ~2 min on M-series)
cargo test --workspace --no-default-features

# Lint
cargo clippy --workspace --no-default-features -- -D warnings
cargo fmt --all -- --check
```

**Commit format** ([Conventional Commits](https://www.conventionalcommits.org/)):
`feat` · `fix` · `refactor` · `docs` · `test` · `chore` · `ci`

**Branch strategy:** `feature/*` → PR → `main`. CI gates on Linux; macOS runs post-merge only (cost optimization).

---

## License

Apache License 2.0 — see [LICENSE](LICENSE).

---

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)"  srcset="img/cuervo-cloud-logo.png">
    <source media="(prefers-color-scheme: light)" srcset="img/cuervo-logo-2.png">
    <img alt="Cuervo AI" src="img/cuervo-logo-2.png" width="72">
  </picture>
  <br/>
  <sub>Built by <a href="https://github.com/cuervo-ai">Cuervo AI</a></sub>
</p>
