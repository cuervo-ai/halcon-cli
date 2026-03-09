# Tool Execution Diagnostics — Phase 1 Report

**Date**: 2026-03-08
**Branch**: feature/sota-intent-architecture
**Scope**: Analysis of which commands are blocked by CATASTROPHIC_PATTERNS, DANGEROUS_COMMAND_PATTERNS, and the G7 permission gate

---

## 1. Blocked Tool Analysis

### 1.1 CATASTROPHIC_PATTERNS (halcon-core/src/security.rs)

These patterns block commands at **runtime** in `bash.rs` (`execute()`) — AFTER permission was granted.

| Pattern | Blocks | Analysis Tool Impact |
|---------|--------|----------------------|
| `^rm\s+(-[rfivRF]+\s+)+/$` | `rm -rf /` | ✅ No impact |
| `^rm\s+(-[rfivRF]+\s+)+/\*+$` | `rm -rf /*` | ✅ No impact |
| `^rm\s+.*/(bin\|etc\|usr\|...)` | `rm -rf /etc` | ✅ No impact |
| `:(\)\{:\|:&\};:` | Fork bomb | ✅ No impact |
| `^mkfs\.` | Disk format | ✅ No impact |
| `dd\s+.*\s+of=/dev/[sh]d` | dd to disk | ✅ No impact |
| `(curl\|wget).*\|\s*(ba)?sh` | `curl \| bash` | ⚠️ Blocks `curl URL \| bash` pattern |
| `(curl\|wget).*\|\s*python` | `curl \| python` | ⚠️ Blocks `curl URL \| python` |
| `^chmod\s+(-R\s+)?[0-7]+\s+/$` | `chmod 777 /` | ✅ No impact |
| `^chown\s+(-R\s+)?.*\s+/$` | `chown -R user /` | ✅ No impact |
| `^systemctl\s+stop\s+(sshd\|network)` | Stop critical services | ✅ No impact |
| `^kill\s+-9\s+1\b` | Kill init | ✅ No impact |
| `^\s*>\s*/dev/(null\|zero)\s*$` | Standalone redirect only | ✅ No impact (anchored) |

**Result**: `grep -r`, `find .`, `cat file`, `npm audit`, `cargo audit` are **NOT blocked** by CATASTROPHIC_PATTERNS.

### 1.2 DANGEROUS_COMMAND_PATTERNS (G7 Hard Veto)

Applied at the permission gate **before** execution. None of the 12 patterns target analysis commands (`grep`, `find`, `cat`, `npm audit`, `cargo audit`).

**Result**: Analysis tools pass the G7 veto gate cleanly.

---

## 2. Real Blockers for Analysis Tasks

The actual cause of analysis tool failures is **not** the blacklists. The root causes are:

### 2.1 BashTool Permission Level

```rust
// crates/halcon-tools/src/bash.rs:136
fn permission_level(&self) -> PermissionLevel {
    PermissionLevel::Destructive
}
```

With `confirm_destructive = true` (default in `config/default.toml`), **every bash call** requires interactive user confirmation. In agent loops running 10–20 tool calls, this creates confirmation fatigue and stalls.

**Fix**: Add `[security.analysis_mode]` config with an allow-list of safe bash command prefixes that auto-approve without prompting (Phase 4).

### 2.2 GovernanceRescue Fires Before Confirmation Loop Completes

When `consecutive_stalls >= stall_threshold` (default 3), `ProgressPolicy` triggers `GovernanceRescue → SynthesisTrigger`. If the agent was waiting on user confirmations, the stall counter increments incorrectly.

### 2.3 ReplanRequired Without Tool Failure Context

When `ReadSaturation` is detected and `LlmPlanner.plan()` is called (convergence_phase.rs:1443), the replan prompt **does not include the tool failures** from the current round — only blocked_tools from the evidence bundle and context from recent assistant messages. This causes the planner to generate similar plans without understanding why the previous tools failed.

**Fix**: Phase 3 — inject `tool_failures` into the replan prompt.

### 2.4 LoopCritic Disabled in Production

`enable_loop_critic = false` (config.rs:544) means synthesis quality is never adversarially evaluated. Analysis tasks with `synthesis_origin = SupervisorFailure` complete with unverified fabricated output.

**Fix**: Phase 2 — enable LoopCritic with `critic_timeout_secs = 30`.

---

## 3. Tool Failure Classification

| Error Pattern | `ToolFailureTracker` Key | Circuit Trip Threshold |
|--------------|--------------------------|------------------------|
| `no such file or directory` | `not_found` | 3 |
| `permission denied` | `permission_denied` | 3 |
| `blocked by security` | `security_blocked` | 3 |
| `mcp pool call failed` | `mcp_unavailable` | 3 |
| `unknown tool` | `unknown_tool` | 3 |
| `denied by task context` | `tbac_denied` | 3 |

The circuit breaker currently only injects a fallback directive for `file_read → file_inspect`. No alt-tool suggestions exist for `bash` commands (`grep → rg`, `find → glob`, `cat → file_read`).

**Fix**: Phase 5 — extend `failure_tracker` to return failure count, and inject count-graduated directives.

---

## 4. Remediation Summary

| Phase | What | Files Changed |
|-------|------|---------------|
| 1 | This report (no code change) | `docs/audit/tool-execution-diagnostics.md` |
| 2 | Enable LoopCritic | `config/default.toml` |
| 3 | Pass `tool_failures` to planner | `convergence_phase.rs` |
| 4 | Analysis mode config + auto-approve whitelist | `config/default.toml`, `config.rs` |
| 5 | Graduated retry/mutation directives | `failure_tracker.rs`, `post_batch.rs` |
