# Halcon Audit & Compliance Export Capability

> Feature 8 · Implemented 2026-03-08

## Overview

`halcon audit` provides a compliance-ready export of all agent activity stored in
the local SQLite database.  It is designed for SOC 2 Type II audits, internal
security reviews, and SIEM integration.

No new instrumentation is required.  All data is sourced from tables that have
been written since Halcon's initial release:

| Table | Content |
|---|---|
| `audit_log` | HMAC-SHA256 chain of domain events (tamper-evident) |
| `invocation_metrics` | Per-round model calls (provider, latency, tokens, cost) |
| `tool_execution_metrics` | Per-tool timing and success data |
| `sessions` | Session metadata (model, working directory, totals) |
| `execution_loop_events` | Agent-loop events (convergence, guard fires, replan) |
| `policy_decisions` | TBAC allow/deny decisions per tool call |
| `resilience_events` | Circuit breaker trips and provider fallbacks |

## CLI Reference

### `halcon audit list`

Lists all sessions with compliance summary statistics.

```
SESSION   START                 DURATION  MODEL                     ROUNDS  TOKENS   TOOLS  BLOCKED  GATES
────────────────────────────────────────────────────────────────────────────────────────────────────────
a1b2c3d4  2026-03-08T10:00:00   120s  claude-sonnet-4-6         8       42300    24     1        0
```

Options:
- `--json` — emit JSON array instead of table
- `--db PATH` — override database path (default: `~/.halcon/halcon.db`)

### `halcon audit export`

Export session audit events as JSONL, CSV, or PDF.

```bash
# Export one session as JSONL (stdout)
halcon audit export --session <UUID>

# Export to CSV file
halcon audit export --session <UUID> --format csv --output audit.csv

# Export a time range as JSONL
halcon audit export --since 2026-03-01T00:00:00Z --format jsonl --output march.jsonl

# Full export with raw tool I/O (use cautiously — may include sensitive data)
halcon audit export --session <UUID> --include-tool-inputs --include-tool-outputs
```

Options:
- `--session UUID` — export one session (exclusive with `--since`)
- `--since ISO-8601` — export all sessions since timestamp (exclusive with `--session`)
- `--format jsonl|csv|pdf` — output format (default: `jsonl`)
- `--output PATH` — write to file instead of stdout
- `--include-tool-inputs` — include raw tool inputs in payload
- `--include-tool-outputs` — include raw tool outputs in payload
- `--db PATH` — override database path

### `halcon audit verify <session-id>`

Verifies the HMAC-SHA256 hash chain for a session.  Exits with code 1 if
tampering is detected.

```bash
halcon audit verify a1b2c3d4-...
# Session:     a1b2c3d4-...
# Total rows:  47
# Passed:      47
# Failed:      0
# Chain:       INTACT ✓
```

## SOC 2 Event Taxonomy

All exported events use the following normalized `event_type` labels:

| Label | Source |
|---|---|
| `AGENT_SESSION_START` | `session_started`, `agent_started`, `orchestrator_started` |
| `AGENT_SESSION_END` | `session_ended`, `agent_completed`, `orchestrator_completed` |
| `TOOL_CALL` | `tool_executed` + `policy_decisions` allowed |
| `TOOL_BLOCKED` | `permission_denied` + `policy_decisions` denied |
| `SAFETY_GATE_TRIGGER` | `guardrail_triggered`, `pii_detected`, `guard_fired` |
| `CIRCUIT_BREAKER_ACTIVATION` | `circuit_breaker_tripped` |
| `TERMINATION_ORACLE_DECISION` | `convergence_decided`, `policy_decision` |
| `REPLAN_TRIGGERED` | `plan_generated`, `intent_rescored`, `plan_replanned` |
| `MEMORY_WRITE` | `memory_retrieved`, `episode_created`, `experience_recorded` |

## Output Formats

### JSONL (recommended for SIEM)

One JSON object per line.  Each object has:

```json
{
  "event_type": "TOOL_CALL",
  "timestamp_utc": "2026-03-08T10:05:23Z",
  "session_id": "a1b2c3d4-...",
  "sequence_number": 12,
  "payload": { "tool_name": "bash", "chain_hash": "3f9a1c..." }
}
```

### CSV

Fixed column schema for spreadsheet / SQL ingestion:

```
sequence_number,event_type,timestamp_utc,session_id,payload_json
1,AGENT_SESSION_START,2026-03-08T10:00:00Z,a1b2c3d4,...,"{...}"
```

### PDF

Structured 3-section report:
1. **Cover page** — session metadata, event counts, session summary table
2. **Event timeline** — all events in chronological order (paginated)
3. **Breakdown** — event-type counts + safety gate count

## Hash Chain Integrity

Each row in `audit_log` is signed with `HMAC-SHA256(per_db_key, prev_hash ‖ event_id ‖ timestamp ‖ payload)`.

The per-database key is stored in the `audit_hmac_key` table and is generated once on
first open.  An adversary who obtains a copy of the database without the key
cannot forge new valid chain entries.

`halcon audit verify` recomputes all HMACs and reports any mismatches.  This is
suitable as a pre-audit check before submitting logs to a compliance reviewer.

## Privacy Controls

By default, raw tool inputs and outputs are **not** included in exports to
minimize PII surface.  Use `--include-tool-inputs` / `--include-tool-outputs`
only when required by the audit scope.

In TOOL_BLOCKED events, the `tool_name` field is redacted unless
`--include-tool-inputs` is set.
