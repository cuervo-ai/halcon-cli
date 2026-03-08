# Halcon as MCP Server

Halcon can operate as a **Model Context Protocol (MCP) server**, exposing its
built-in tools to any MCP client — Claude Code, the VS Code extension, other
agents, or your own tooling.

This makes Halcon the *trusted execution substrate* beneath other AI surfaces:
Claude Code's reasoning layer runs on top, while every tool call goes through
Halcon's FASE-2 path gate, `CATASTROPHIC_PATTERNS` check, TBAC validation, and
audit trail.

## Transport Modes

| Mode | Command | Use case |
|------|---------|----------|
| **stdio** | `halcon mcp serve` | Claude Code, IDE sidecars, subprocess embedding |
| **HTTP** | `halcon mcp serve --transport http --port 7777` | VS Code extension, remote clients, multi-tenant |

---

## Stdio Transport — Claude Code Integration

The stdio transport reads JSON-RPC from stdin and writes to stdout. This is
identical to how `claude mcp serve` works, making Halcon a drop-in peer.

### Add Halcon to Claude Code

```bash
# Add as a persistent MCP server
claude mcp add halcon --transport stdio -- halcon mcp serve

# Verify it appears
claude mcp list
```

Claude Code will now call `halcon mcp serve` as a subprocess whenever it needs
tools. All 61 of Halcon's built-in tools become available in Claude Code
sessions.

### What Claude Code can do with Halcon tools

- `bash` — execute shell commands with Halcon's FASE-2 safety gate
- `file_read`, `file_write`, `file_edit`, `file_delete` — file operations
- `glob`, `grep`, `directory_tree`, `file_inspect` — code exploration
- `git_status`, `git_diff`, `git_commit`, etc. — git operations
- `semantic_grep`, `dependency_graph`, `dep_check` — analysis
- `docker_tool`, `http_probe`, `ci_logs` — infrastructure
- ...and 40+ more

Every call goes through Halcon's guardrails — catastrophic commands are
blocked, TBAC enforces least-privilege, and every execution is audit-logged.

---

## HTTP Transport — Standalone Server

The HTTP transport starts an axum server that implements the MCP Streamable HTTP
spec (POST `/mcp`, GET `/mcp` SSE).

### Start the server

```bash
# No auth (development only)
HALCON_MCP_SERVER_API_KEY="" halcon mcp serve --transport http --port 7777

# With auto-generated API key (recommended)
halcon mcp serve --transport http --port 7777
# Prints: HALCON_MCP_SERVER_API_KEY=<48-char-hex-key>

# With a pre-set API key
export HALCON_MCP_SERVER_API_KEY=my-secure-key
halcon mcp serve --transport http --port 7777
```

### Configuration via `halcon.toml`

```toml
[mcp_server]
enabled = true
transport = "http"
port = 7777
expose_agents = true
require_auth = true
allowed_clients = []      # empty = allow all; list of client IDs to restrict
session_ttl_secs = 1800   # 30 minutes idle timeout
```

### Test with curl

```bash
# Initialize
curl -X POST http://localhost:7777/mcp \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $HALCON_MCP_SERVER_API_KEY" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","clientInfo":{"name":"test","version":"1.0"},"capabilities":{}}}'

# List tools
curl -X POST http://localhost:7777/mcp \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $HALCON_MCP_SERVER_API_KEY" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/list"}'

# Call bash tool
curl -X POST http://localhost:7777/mcp \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $HALCON_MCP_SERVER_API_KEY" \
  -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"bash","arguments":{"command":"echo hello"}}}'

# Health probe
curl http://localhost:7777/health
```

---

## VS Code Extension Integration

The Halcon VS Code extension spawns the binary in subprocess mode by default.
To use a shared HTTP server instead (useful for team setups where one server
serves multiple developers):

1. Start the server on a shared host: `halcon mcp serve --transport http --port 7777`
2. In VS Code settings, set `halcon.mcpServerUrl` to `http://<host>:7777/mcp`
   and `halcon.mcpServerApiKey` to the printed key.

> **Note**: Local `http://127.0.0.1:7777` is ready to use immediately. Remote
> hosts require TLS termination (nginx/caddy reverse proxy) for production.

---

## Security Architecture

### CATASTROPHIC_PATTERNS (FASE-2)

Commands matching the 18 catastrophic patterns are blocked **at the tool layer**,
not at the MCP layer. This means the safety guarantee holds regardless of how the
tool is invoked — directly, via the REPL, via JSON-RPC, or via MCP.

Blocked examples:

```bash
# All of these are blocked, no matter the caller:
rm -rf /
rm -rf /*
chmod -R 777 /
dd if=/dev/zero of=/dev/sda
```

The `bash` tool returns `is_error: true` with a clear rejection message.

### Bearer Token Auth (HTTP mode)

Every HTTP request must carry `Authorization: Bearer <key>`. Without it:
- `401 Unauthorized` is returned
- The request is never dispatched to any tool
- No audit log entry is written (the call never reached the tool layer)

### Session Isolation

Each client connection carries a `Mcp-Session-Id` header. Different session IDs
get different execution contexts — they cannot observe each other's working
directory or state. Sessions expire after `session_ttl_secs` of inactivity
(default 30 minutes).

### Audit Logging

Every tool call via the MCP server emits a structured `tracing::info!` event:

```
mcp_server.tool_call  tool=bash  transport=http
mcp_server.tool_result  tool=bash  success=true
```

When Halcon is configured with structured logging (`--trace-json`), these events
appear in the JSONL audit stream and can be exported with `halcon audit export`.

---

## Deployment Patterns

### Single Developer (Local)

```bash
# Start once, connect Claude Code
halcon mcp serve &
claude mcp add halcon -- halcon mcp serve
```

### Team Server (Shared HTTP)

```bash
# On a shared Linux server
export HALCON_MCP_SERVER_API_KEY="$(openssl rand -hex 24)"
echo "Key: $HALCON_MCP_SERVER_API_KEY"
halcon mcp serve --transport http --port 7777

# Each developer adds to their Claude Code config
claude mcp add halcon-team \
  --transport http \
  --url http://your-server:7777/mcp \
  --header "Authorization: Bearer $HALCON_MCP_SERVER_API_KEY"
```

### Docker

```dockerfile
FROM ubuntu:24.04
COPY halcon /usr/local/bin/halcon
ENV HALCON_MCP_SERVER_API_KEY=changeme
EXPOSE 7777
CMD ["halcon", "mcp", "serve", "--transport", "http", "--port", "7777"]
```

```bash
docker run -d -p 7777:7777 \
  -e HALCON_MCP_SERVER_API_KEY=my-secure-key \
  halcon-server
```

### systemd Service

```ini
[Unit]
Description=Halcon MCP Server
After=network.target

[Service]
Type=simple
User=halcon
Environment=HALCON_MCP_SERVER_API_KEY=<your-key>
ExecStart=/usr/local/bin/halcon mcp serve --transport http --port 7777
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

---

## API Reference

### POST /mcp

JSON-RPC 2.0 endpoint. Supported methods:

| Method | Description |
|--------|-------------|
| `initialize` | Handshake — returns server info and capabilities |
| `tools/list` | Returns all available tool definitions |
| `tools/call` | Execute a tool; `params.name` + `params.arguments` |
| `ping` | Liveness check — returns `{}` |

**Request headers:**
- `Authorization: Bearer <key>` — required if auth is enabled
- `Mcp-Session-Id: <uuid>` — optional, auto-assigned if omitted

**Response headers:**
- `Mcp-Session-Id: <uuid>` — echoed back (or the auto-assigned value)

### GET /mcp

SSE stream for server-initiated messages. Currently emits an `endpoint` event
pointing to `POST /mcp`. Full bidirectional SSE is a Phase 3.1 enhancement.

### GET /health

Plain-text liveness probe. Returns `ok` with status 200.

---

## Roadmap

| Phase | Feature | Status |
|-------|---------|--------|
| 3.0 | stdio transport + HTTP transport + auth + audit | ✅ Done |
| 3.1 | Agent tools (`agent_*`) via agent registry | Planned |
| 3.2 | Full OAuth 2.1 resource server validation | Planned |
| 3.3 | `list_changed` notifications when registry updates | Planned |
| 3.4 | TLS termination built-in (Let's Encrypt via `rcgen`) | Planned |
