# Engineering Tool Safety & Performance Validation

> Phase 5 deliverable for the Engineering Utility Layer design.
> Input: [ENGINEERING_TOOL_ARCHITECTURE.md](./ENGINEERING_TOOL_ARCHITECTURE.md)

---

## 1. Safety Analysis

### 1.1 Threat Model

Each new tool is evaluated against five threat categories:

| Threat | Description | Affected Tools |
|--------|------------|---------------|
| **T1: Path Traversal** | Access files outside allowed directories | file_delete |
| **T2: Command Injection** | Inject shell commands via user-controlled input | git tools, background tools |
| **T3: Credential Exposure** | Leak API keys, tokens, or secrets | web_search, http_request |
| **T4: Resource Exhaustion** | Spawn unbounded processes or consume memory | background tools, fuzzy_find |
| **T5: Data Destruction** | Irreversible deletion or corruption | file_delete, git_commit, git_add |

### 1.2 Per-Tool Safety Analysis

#### Git Status / Diff / Log (ReadOnly)

| Threat | Risk | Mitigation |
|--------|------|-----------|
| T2 | **Low** | Arguments are not interpolated into shell. Uses `Command::arg()` (not `Command::args(shell_string)`). Each argument is passed as a separate OS string. |
| T5 | **None** | Read-only operations. |

**Verification**: test that `git_status` with a working directory containing `"; rm -rf /"` in the path does not execute the injected command.

#### Git Add (ReadWrite)

| Threat | Risk | Mitigation |
|--------|------|-----------|
| T1 | **Medium** | Validate each path exists and is within working directory. Reject absolute paths outside working dir. |
| T2 | **Low** | Paths passed via `Command::arg()`, not shell interpolation. |
| T5 | **Medium** | Staging wrong files could lead to bad commits. Mitigation: explicit path list only, no `git add .` or `git add -A`. |

**Verification**: test that `git_add` with path `../../etc/passwd` fails validation.

#### Git Commit (Destructive)

| Threat | Risk | Mitigation |
|--------|------|-----------|
| T2 | **Low** | Commit message passed via `Command::arg("-m").arg(message)`, not shell-interpreted. |
| T5 | **High** | Creates permanent git history. Mitigation: always requires confirmation, no `--amend`, no `--no-verify`. |

**Verification**: test that `requires_confirmation()` returns true. Test that `--amend` flag is not present in any code path.

#### Background Start (Destructive)

| Threat | Risk | Mitigation |
|--------|------|-----------|
| T2 | **High** | Executes arbitrary commands (same risk as bash tool). Mitigation: same sandbox rlimits as bash, requires confirmation. |
| T4 | **High** | Could spawn unbounded processes. Mitigation: `max_concurrent` limit (default 5), timeout enforcement, `ProcessRegistry::Drop` kills all children. |

**Verification**: test max_concurrent enforcement. Test that ProcessRegistry Drop impl terminates children.

#### File Delete (Destructive)

| Threat | Risk | Mitigation |
|--------|------|-----------|
| T1 | **High** | Could delete files outside project. Mitigation: `path_security::resolve_and_validate()` (same as file_read/write/edit). Blocked patterns enforced. |
| T5 | **Critical** | Irreversible data loss. Mitigation: always requires confirmation, no recursive delete, single file only, size reported before deletion. |

**Verification**: test blocked patterns (.env, *.key). Test path traversal rejection. Test that directories are rejected (no `rm -rf`).

#### Web Search (ReadOnly)

| Threat | Risk | Mitigation |
|--------|------|-----------|
| T3 | **Medium** | API key sent to search provider. Mitigation: API key loaded from config/env, never included in tool output or metadata. Key validated at registration time. |

**Verification**: test that API key does not appear in ToolOutput content or metadata.

#### HTTP Request (Destructive)

| Threat | Risk | Mitigation |
|--------|------|-----------|
| T3 | **High** | User-specified headers could contain credentials. Auth headers in tool output could leak secrets. Mitigation: redact `Authorization` and `Cookie` headers from output. Log request URL but not headers/body. |
| T2 | **Low** | URL is passed directly to reqwest, not shell-interpreted. |

**Verification**: test that Authorization header is redacted in output. Test that response body > 1MB is truncated.

#### Task Track (ReadOnly)

| Threat | Risk | Mitigation |
|--------|------|-----------|
| All | **None** | Pure in-memory state management. No filesystem, network, or process operations. |

#### Symbol Search / Fuzzy Find (ReadOnly)

| Threat | Risk | Mitigation |
|--------|------|-----------|
| T4 | **Low** | Could walk very large directory trees. Mitigation: max_results limit, skip hidden/vendor directories, max depth limit (10). |
| T1 | **Low** | Only reads, never modifies. Path scoping via working directory. |

**Verification**: test that hidden directories and node_modules are skipped. Test max_results enforcement.

---

## 2. Permission Level Assignments

| Tool | Permission | Confirmation | Justification |
|------|-----------|-------------|---------------|
| `git_status` | ReadOnly | No | Reads repo state only |
| `git_diff` | ReadOnly | No | Reads diff only |
| `git_log` | ReadOnly | No | Reads log only |
| `git_add` | ReadWrite | No | Stages files (reversible via `git reset`) |
| `git_commit` | Destructive | **Yes** | Creates permanent history |
| `background_start` | Destructive | **Yes** | Executes arbitrary commands |
| `background_output` | ReadOnly | No | Reads process output |
| `background_kill` | Destructive | No | Terminates owned process (agent-initiated) |
| `file_delete` | Destructive | **Yes** | Irreversible file removal |
| `web_search` | ReadOnly | No | Reads search results |
| `http_request` | Destructive | **Yes** | Sends data to external servers |
| `task_track` | ReadOnly | No | Internal agent state |
| `symbol_search` | ReadOnly | No | Reads source files |
| `fuzzy_find` | ReadOnly | No | Reads directory listing |

**Destructive tools requiring confirmation: 4** (git_commit, background_start, file_delete, http_request)

---

## 3. Input Validation Rules

### 3.1 String Injection Prevention

All tools must follow this rule: **never construct shell command strings from user input**. Always use `Command::arg()` for each argument separately.

```rust
// CORRECT: each argument is a separate OS string
Command::new("git")
    .arg("commit")
    .arg("-m")
    .arg(&message)  // message can contain any characters safely

// WRONG: shell injection possible
Command::new("sh")
    .arg("-c")
    .arg(format!("git commit -m '{message}'"))  // VULNERABLE
```

### 3.2 Path Validation

All tools accepting file paths must call `path_security::resolve_and_validate()`:

| Tool | Path Params | Validation Required |
|------|------------|-------------------|
| `file_delete` | `path` | Yes -- full validation |
| `git_add` | `paths[]` | Yes -- validate each path exists in working dir |
| `git_diff` | `path` (optional) | Partial -- validate relative to repo root |
| `git_log` | `path` (optional) | Partial -- validate relative to repo root |
| `symbol_search` | `path` (optional) | Yes -- scoped to working dir |
| `fuzzy_find` | `path` (optional) | Yes -- scoped to working dir |

### 3.3 Integer Bounds

| Tool | Param | Min | Max | Default |
|------|-------|-----|-----|---------|
| `git_log` | `count` | 1 | 50 | 10 |
| `web_search` | `count` | 1 | 10 | 5 |
| `background_start` | `timeout_secs` | 1 | 3600 | 300 |
| `fuzzy_find` | `max_results` | 1 | 50 | 20 |

### 3.4 URL Validation

| Tool | Param | Rules |
|------|-------|-------|
| `http_request` | `url` | Must start with `http://` or `https://`. No `file://` or `javascript://`. |
| `web_search` | (internal) | API URL hardcoded, not user-controllable. |

---

## 4. Performance Constraints

### 4.1 Latency Targets

| Tool | Target P50 | Target P99 | Bottleneck |
|------|-----------|-----------|-----------|
| `git_status` | <50ms | <200ms | `git` process spawn + porcelain parse |
| `git_diff` | <100ms | <500ms | Large diffs, truncation logic |
| `git_log` | <50ms | <200ms | `git log` with count limit |
| `git_add` | <50ms | <200ms | `git add` is fast |
| `git_commit` | <100ms | <500ms | Hook execution (if any) |
| `background_start` | <50ms | <100ms | Process spawn only (async) |
| `background_output` | <10ms | <50ms | Buffer read (in-memory) |
| `background_kill` | <50ms | <200ms | Signal + cleanup |
| `file_delete` | <10ms | <50ms | Path validation + unlink |
| `web_search` | <500ms | <2s | Network round-trip (API) |
| `http_request` | <500ms | <5s | Network (user-controlled timeout) |
| `task_track` | <1ms | <5ms | In-memory Vec operations |
| `symbol_search` | <100ms | <500ms | Directory walk + regex extraction |
| `fuzzy_find` | <50ms | <200ms | Directory walk + scoring |

### 4.2 Memory Constraints

| Tool | Max Memory | Mechanism |
|------|-----------|-----------|
| `git_diff` | 64KB output | Truncate diff to ~16K chars, preserve stat summary |
| `web_search` | 32KB response | Limit result count (max 10), truncate snippets |
| `http_request` | 1MB response | Same limit as web_fetch |
| `background_output` | 100KB per job | Same truncation as bash tool |
| `symbol_search` | ~1MB index | Max 2000 files scanned, max 500 symbols returned |
| `fuzzy_find` | ~1MB paths | Max 10000 files walked, max 50 results |

### 4.3 Process Limits

| Resource | Limit | Tool |
|----------|-------|------|
| Concurrent background jobs | 5 (configurable) | background_start |
| Background job timeout | 3600s max | background_start |
| Git command timeout | Same as tool timeout (120s default) | All git tools |
| Directory walk depth | 10 levels | symbol_search, fuzzy_find |
| Directory walk max entries | 10000 | fuzzy_find |

---

## 5. Audit & Observability

### 5.1 Tracing Instrumentation

All new tools should include `#[tracing::instrument]` on `execute()`:

```rust
#[tracing::instrument(skip(self, input), fields(
    tool = self.name(),
    working_dir = %input.working_directory,
    // tool-specific fields
))]
async fn execute(&self, input: ToolInput) -> Result<ToolOutput> { ... }
```

### 5.2 Event Bus Integration

Destructive tool executions emit events on the existing event bus:

```rust
// After successful execution of destructive tools:
if self.permission_level() >= PermissionLevel::Destructive {
    bus.send(Event::ToolExecution {
        tool: self.name().to_string(),
        success: !output.is_error,
        metadata: output.metadata.clone(),
    });
}
```

This is already handled by the agent loop -- no per-tool changes needed.

### 5.3 Audit Logging

All tool executions are already recorded in the audit table via `append_audit_event()` in the agent loop. No per-tool audit code needed. The existing infrastructure covers:
- Tool name
- Input arguments (sanitized)
- Output summary
- Timestamp
- Success/failure

---

## 6. Failure Mode Analysis

### 6.1 Graceful Degradation

| Scenario | Expected Behavior |
|----------|------------------|
| `git` binary not installed | `is_git_repo()` returns false, tool returns `is_error: true` with "git not found" |
| Search API unreachable | Tool returns `is_error: true` with "search API unavailable", agent falls back to web_fetch |
| Background process crashes | `background_output` returns exit code + stderr, tool reports is_error |
| File delete permission denied | OS error propagated as `is_error: true`, path included in message |
| Very large repository (>100K files) | fuzzy_find and symbol_search hit max limits, return truncated results |
| Git repo with no commits | git_log returns empty, git_diff returns empty, git_status shows initial state |

### 6.2 Error Message Standards

All tool errors follow this format:
```
{tool_name} error: {specific_message}
```

Examples:
- `git_status error: not a git repository`
- `file_delete error: path blocked by pattern '.env'`
- `background_start error: maximum concurrent jobs (5) reached`
- `web_search error: API key not configured`

### 6.3 Non-Fatal vs Fatal

| Condition | Treatment | Rationale |
|-----------|-----------|-----------|
| Git command exits non-zero | Non-fatal (`is_error: true`) | Agent can see error and decide |
| Path validation fails | Fatal (`Err(CuervoError)`) | Security violation, stop execution |
| Network timeout | Non-fatal (`is_error: true`) | Agent can retry or use alternative |
| Process registry full | Non-fatal (`is_error: true`) | Agent can kill old jobs |
| Invalid input args | Fatal (`Err(CuervoError::InvalidInput)`) | Programming error, agent should fix |

---

## 7. Backward Compatibility

### 7.1 Breaking Changes

| Change | Impact | Migration |
|--------|--------|-----------|
| `default_registry()` signature change | All call sites | Add `process_registry` parameter (1 call site in REPL) |
| `ToolsConfig` new fields | Config deserialization | All new fields have `#[serde(default)]`, backward compatible |
| Contract test count change | Test assertion | Update `assert_eq!(defs.len(), 23)` |

### 7.2 Non-Breaking

- All existing tools unchanged
- All existing tool schemas unchanged
- Tool trait unchanged
- ToolInput/ToolOutput unchanged
- PermissionLevel unchanged
- Existing tests unmodified (except contract test count)

---

## 8. Security Testing Checklist

### Per-Tool Security Tests (included in unit tests)

- [ ] `git_add`: path traversal with `../../etc/passwd` rejected
- [ ] `git_commit`: `requires_confirmation()` returns true
- [ ] `git_commit`: no `--amend` or `--no-verify` flags in any code path
- [ ] `git_*`: command injection via working directory path
- [ ] `background_start`: max_concurrent enforced
- [ ] `background_start`: `requires_confirmation()` returns true
- [ ] `background_start`: sandbox rlimits applied
- [ ] `file_delete`: path traversal rejected
- [ ] `file_delete`: blocked patterns (.env, *.key) rejected
- [ ] `file_delete`: directories rejected (no recursive delete)
- [ ] `file_delete`: `requires_confirmation()` returns true
- [ ] `http_request`: Authorization header redacted in output
- [ ] `http_request`: `file://` URL rejected
- [ ] `http_request`: `requires_confirmation()` returns true
- [ ] `web_search`: API key not in output
- [ ] `fuzzy_find`: hidden directories skipped
- [ ] `symbol_search`: results scoped to working directory

### Integration Security Tests

- [ ] `default_registry` confirmation check for all Destructive tools
- [ ] Tool name uniqueness across all 23+ tools
- [ ] All input schemas have `required` array

---

## 9. Benchmark Plan

### 9.1 Git Tool Benchmarks

Add to `crates/cuervo-tools/benches/` or a new bench file:

```rust
// bench_git_status: TempDir with 100 files, 10 staged, 5 modified
// bench_git_diff: TempDir with 50-line diff
// bench_git_log: Repo with 50 commits
// bench_fuzzy_find: 1000-file directory tree
// bench_symbol_search: 50 Rust files with ~500 symbols total
```

### 9.2 Target Benchmarks

| Benchmark | Target |
|-----------|--------|
| `git_status_100_files` | <100ms |
| `git_diff_50_lines` | <50ms |
| `git_log_50_commits` | <100ms |
| `fuzzy_find_1000_files` | <200ms |
| `symbol_search_50_files` | <300ms |
| `task_track_100_tasks` | <1ms |
