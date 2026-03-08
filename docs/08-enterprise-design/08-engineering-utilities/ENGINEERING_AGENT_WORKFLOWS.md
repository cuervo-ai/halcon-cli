# Engineering Agent Workflows

> Phase 6 deliverable for the Engineering Utility Layer design.
> Input: All prior deliverables.

---

## 1. Overview

This document defines how the expanded tool set (23+ tools) integrates into Cuervo's agent loop, REPL, and multi-agent orchestrator. It covers tool composition patterns, agent workflow recipes, and UX considerations.

---

## 2. Tool Composition Patterns

### 2.1 Sequential Chains

The agent naturally chains tools in sequence. The expanded tool set enables new chains:

| Chain | Tools | Use Case |
|-------|-------|----------|
| **Code Change** | `symbol_search` -> `file_read` -> `file_edit` -> `git_add` -> `git_commit` | Find symbol, read context, edit, stage, commit |
| **Research** | `web_search` -> `web_fetch` -> `file_write` | Search docs, fetch page, save findings |
| **Debug** | `git_diff` -> `grep` -> `file_read` -> `bash` | See changes, find related code, read context, run tests |
| **Explore** | `directory_tree` -> `fuzzy_find` -> `file_read` -> `symbol_search` | Understand structure, find file, read it, find definitions |
| **Deploy** | `git_status` -> `git_add` -> `git_commit` -> `bash(tests)` -> `bash(deploy)` | Stage, commit, test, deploy |
| **Background** | `background_start` -> work on other things -> `background_output` | Start long process, continue working, check results |

### 2.2 Parallel Composition

Some tool calls can be parallelized when they have no data dependencies:

| Pattern | Parallel Tools | Condition |
|---------|---------------|-----------|
| **Multi-search** | `grep` + `symbol_search` + `fuzzy_find` | Different search strategies for same query |
| **Context gathering** | `git_status` + `git_log` + `directory_tree` | Independent repo state queries |
| **Multi-file read** | `file_read(a)` + `file_read(b)` + `file_read(c)` | Independent file reads |
| **Background check** | `background_output(bg-1)` + `background_output(bg-2)` | Independent job status checks |

Cuervo's agent loop already supports parallel tool execution (multiple ToolUse blocks in a single assistant message). No changes needed.

### 2.3 Conditional Composition

The agent decides which tools to use based on previous results:

```
git_status -> if clean: "no changes"
           -> if dirty: git_diff -> analyze changes -> git_add -> git_commit
```

```
symbol_search("auth") -> if found: file_read(match.file, match.line)
                       -> if not found: web_search("authentication pattern {language}")
```

This is natural agent behavior -- no infrastructure changes needed.

---

## 3. Workflow Recipes

### 3.1 Feature Implementation Workflow

```
Agent receives: "Add a rate limiter to the API handler"

1. symbol_search(query="api_handler", kind="function")
   -> Found: src/api/handler.rs:42

2. file_read(path="src/api/handler.rs")
   -> Read full file for context

3. web_search(query="rate limiter implementation Rust tokio")
   -> Found: tower-http rate limiting docs

4. web_fetch(url="https://docs.rs/tower-http/latest/rate_limit")
   -> Read implementation examples

5. file_edit(path="src/api/handler.rs", old_string="...", new_string="...")
   -> Add rate limiter middleware

6. file_edit(path="Cargo.toml", old_string="...", new_string="...")
   -> Add tower-http dependency

7. bash(command="cargo check")
   -> Verify compilation

8. git_add(paths=["src/api/handler.rs", "Cargo.toml"])
   -> Stage changes

9. git_commit(message="Add rate limiter to API handler")
   -> Commit with descriptive message
```

### 3.2 Bug Fix Workflow

```
Agent receives: "Fix the failing test in auth module"

1. bash(command="cargo test --lib auth 2>&1 | tail -30")
   -> See failing test output

2. grep(pattern="test.*auth.*fail", path="src/auth/")
   -> Find test file and function

3. file_read(path="src/auth/tests.rs", offset=42, limit=30)
   -> Read failing test

4. symbol_search(query="authenticate", path="src/auth/")
   -> Find the implementation being tested

5. file_read(path="src/auth/mod.rs", offset=100, limit=50)
   -> Read implementation

6. file_edit(path="src/auth/mod.rs", old_string="...", new_string="...")
   -> Fix the bug

7. bash(command="cargo test --lib auth")
   -> Verify fix

8. git_diff(staged=false)
   -> Review changes

9. git_add(paths=["src/auth/mod.rs"])
10. git_commit(message="Fix authentication test: handle empty token case")
```

### 3.3 Code Review Workflow

```
Agent receives: "Review the changes on the current branch"

1. git_status()
   -> See overall state

2. git_log(count=5)
   -> See recent commits

3. git_diff(commit="main")
   -> See all changes from main branch

4. For each changed file:
   a. file_read(path=changed_file)
   b. symbol_search(query=modified_function)
   c. grep(pattern=potential_issues, path=changed_file)

5. task_track(action="add", content="Review: src/api/handler.rs - looks good")
6. task_track(action="add", content="Review: src/auth/mod.rs - edge case concern at line 45")

7. Provide summary with findings
```

### 3.4 Long-Running Task Workflow

```
Agent receives: "Run the full test suite and fix any failures"

1. background_start(command="cargo test --workspace 2>&1")
   -> Started bg-1

2. task_track(action="add", content="Run full test suite", status="in_progress")

3. While waiting, explore codebase:
   a. directory_tree(path="src/")
   b. fuzzy_find(query="config")

4. background_output(job_id="bg-1")
   -> Check test progress

5. If tests finished:
   a. Parse failures
   b. For each failure: file_read -> file_edit -> fix
   c. background_start(command="cargo test --workspace 2>&1")
   d. Repeat until green

6. task_track(action="update", task_index=0, status="completed")
7. git_add + git_commit
```

### 3.5 Project Setup Workflow

```
Agent receives: "Set up a new Rust web service"

1. bash(command="cargo init web-service")
2. file_write(path="web-service/Cargo.toml", content="...")
3. file_write(path="web-service/src/main.rs", content="...")
4. file_write(path="web-service/src/routes.rs", content="...")

5. bash(command="cd web-service && cargo check")
   -> Verify project builds

6. web_search(query="axum web service best practices 2026")
   -> Research current patterns

7. Apply patterns from research

8. bash(command="cd web-service && cargo test")
9. git_add(paths=["web-service/"])
10. git_commit(message="Initialize web service with axum")
```

---

## 4. REPL Integration

### 4.1 Tool-Aware System Prompt

The system prompt should describe all available tools with concise descriptions. When tool count exceeds 20, organize by category:

```
## Available Tools

### File Operations
- file_read: Read file contents with line numbers
- file_write: Create or overwrite files
- file_edit: Search-and-replace editing
- file_delete: Delete a single file
- file_inspect: Detect format, extract text, show metadata

### Search & Navigation
- glob: Find files by pattern
- grep: Search file contents with regex
- directory_tree: Show directory structure
- fuzzy_find: Fuzzy file path search
- symbol_search: Find function/class/struct definitions

### Shell & Process
- bash: Execute shell commands
- background_start: Start long-running command
- background_output: Check background job output
- background_kill: Terminate background job

### Version Control
- git_status: Show working tree status
- git_diff: Show file changes
- git_log: Show commit history
- git_add: Stage files for commit
- git_commit: Create a git commit

### Network
- web_fetch: Fetch URL content
- web_search: Search the web
- http_request: Send HTTP POST/PUT/DELETE/PATCH

### Agent
- task_track: Manage task checklist
```

### 4.2 REPL Commands for Tool Management

Existing REPL commands that interact with tools:

| Command | Purpose | New Behavior |
|---------|---------|-------------|
| `/help` | Show available commands | Include tool categories in help |
| `/model` | Switch model | No change |
| `/dry-run` | Preview without executing | Applies to all new tools |

No new REPL commands needed. Tools are invoked by the agent, not by the user directly.

### 4.3 Tool Confirmation UX

For Destructive tools requiring confirmation, the existing confirmation dialog works:

```
[cuervo] The agent wants to run:
  git_commit -m "Fix authentication bug"

  Allow? [y/N]
```

Consider enhancing with tool-specific context:
```
[cuervo] The agent wants to create a git commit:
  Message: "Fix authentication bug"
  Staged files: src/auth/mod.rs, src/auth/tests.rs

  Allow? [y/N]
```

This can be done by extending `requires_confirmation()` to return a `ConfirmationContext` struct instead of `bool` (future enhancement, not blocking).

---

## 5. Multi-Agent Orchestrator Integration

### 5.1 Tool Assignment per Sub-Agent

The orchestrator assigns tools to sub-agents based on task type:

| Task Type | Assigned Tools | Rationale |
|-----------|---------------|-----------|
| Research | web_search, web_fetch, file_read, grep, symbol_search | Read-only research tools |
| Code modification | file_read, file_edit, file_write, file_delete, bash, symbol_search | Full code editing + validation |
| Testing | bash, background_start, background_output, file_read, grep | Run and analyze tests |
| Git operations | git_status, git_diff, git_log, git_add, git_commit | Version control tasks |
| Planning | file_read, directory_tree, symbol_search, fuzzy_find, task_track | Understand codebase + plan |

### 5.2 Shared State Across Agents

| State | Sharing Model | Implementation |
|-------|--------------|---------------|
| ProcessRegistry | Shared `Arc<Mutex<ProcessRegistry>>` | All agents share one registry |
| Task list (task_track) | Per-agent (isolated) | Each agent has its own task list |
| Git state | Shared filesystem | Agents operate on same repo |
| File system | Shared filesystem | Path security per-agent |

### 5.3 Orchestrator Workflow Example

```
User: "Refactor the database module and add comprehensive tests"

Orchestrator decomposes into 3 sub-agents:

Agent 1 (Research):
  - symbol_search("database", kind="module")
  - file_read each module file
  - directory_tree("src/database/")
  - Report findings to orchestrator

Agent 2 (Refactor):
  - Receives findings from Agent 1
  - file_edit (multiple files)
  - bash("cargo check")
  - git_add + git_commit("Refactor database module")

Agent 3 (Testing):
  - file_write("src/database/tests.rs", test content)
  - bash("cargo test --lib database")
  - Fix any failures
  - git_add + git_commit("Add database module tests")
```

---

## 6. Tool Discovery & Documentation

### 6.1 In-Session Tool Discovery

The agent's system prompt includes all tool descriptions. When tool count exceeds ~25, implement **tool search** (load tools on demand):

1. At session start, only load critical tools (file_read/write/edit, bash, grep, glob)
2. When the agent requests a tool not in the loaded set, search available tools by name/description
3. Load matched tool dynamically

This mirrors Claude Code's MCP tool search pattern. **Not needed until tool count > 25.**

### 6.2 Model-Facing Documentation

Each tool's `description()` should be concise but informative:

| Quality | Example |
|---------|---------|
| **Good** | "Search file contents with regex patterns. Returns matching lines with file paths and line numbers." |
| **Bad** | "grep tool for searching" |
| **Good** | "Show working tree status: staged, modified, and untracked files." |
| **Bad** | "git status" |

Guideline: 1-2 sentences. First sentence = what it does. Second sentence = what it returns.

---

## 7. Error Recovery Patterns

### 7.1 Agent Self-Correction with New Tools

The expanded tool set enables better error recovery:

| Error | Recovery Pattern |
|-------|----------------|
| Compilation error after edit | `bash("cargo check")` -> parse error -> `file_read` error location -> `file_edit` fix |
| Test failure after change | `bash("cargo test")` -> parse failure -> `git_diff` to see what changed -> fix |
| Git commit rejected (hooks) | `git_status` -> see what's wrong -> fix -> `git_add` -> `git_commit` |
| File not found | `fuzzy_find(partial_name)` -> find correct path -> `file_read` |
| Symbol not found | `symbol_search(approx_name)` -> find correct name/file |
| Background job failed | `background_output` -> see error -> fix and restart |

### 7.2 Self-Correction Context Injection

Cuervo's existing self-correction system (Phase 18 B.1) injects context about tool failures. The new tools participate automatically because the system operates at the ToolOutput level:

```
When tool_failures is non-empty:
  Inject: "Previous tool failures: {tool}: {error_message}"
  Agent sees failures and adjusts strategy
```

---

## 8. Performance Optimization Strategies

### 8.1 Tool Speculation with New Tools

Cuervo's tool speculation system (Phase 8C) can predict new tool calls:

| Trigger | Predicted Tool | Confidence |
|---------|---------------|-----------|
| User mentions "commit" | `git_status` | 0.7 |
| Agent edits a file | `bash("cargo check")` or `bash("cargo test")` | 0.6 |
| User mentions "search for" | `grep` or `symbol_search` | 0.7 |
| Agent reads a git diff | `file_read` for changed files | 0.8 |
| User mentions "background" | `background_output` for running jobs | 0.6 |

Only ReadOnly tools are speculated (safety constraint preserved).

### 8.2 Parallel Tool Execution

When the model returns multiple tool_use blocks in a single response, execute them in parallel using `tokio::join!`. This is already implemented in the agent loop. The new tools benefit automatically.

---

## 9. Future Considerations

### 9.1 Tool Hooks (Pre/Post Execution)

Similar to Claude Code's hook system, add lifecycle hooks for tools:

```rust
pub trait ToolHook: Send + Sync {
    /// Called before tool execution. Can modify input or block execution.
    async fn pre_execute(&self, tool_name: &str, input: &mut ToolInput) -> HookDecision;

    /// Called after tool execution. Can modify output or log.
    async fn post_execute(&self, tool_name: &str, output: &ToolOutput);
}

pub enum HookDecision {
    Allow,
    Deny(String),
    Modify,
}
```

This enables:
- Custom approval workflows for specific tools
- Input sanitization before execution
- Output filtering (redact sensitive data)
- Audit logging beyond default

### 9.2 Tool Versioning

As tools evolve, maintain backward compatibility:
- Tool names are stable (never rename)
- Input schemas are additive (new optional params, never remove)
- Output format changes are backward compatible

### 9.3 MCP Tool Parity

Every native tool should be exposable as an MCP tool for external consumption. This enables Cuervo to act as an MCP server, not just a client.

---

## 10. Summary

### What Changes

| Component | Change |
|-----------|--------|
| Tool count | 9 -> 23+ |
| Tool categories | 3 (file, shell, web) -> 6 (+ git, search, agent) |
| New modules | 8 new files + 2 submodules (git/, background/) |
| Config types | +3 new config structs (SearchConfig, GitToolsConfig, BackgroundConfig) |
| `default_registry()` signature | +1 parameter (ProcessRegistry) |
| Contract tests | Updated count + new confirmation checks |

### What Does NOT Change

| Component | Status |
|-----------|--------|
| Tool trait | Unchanged |
| ToolInput/ToolOutput | Unchanged |
| PermissionLevel | Unchanged |
| Agent loop | Unchanged (tools register into existing registry) |
| REPL | Unchanged (tools are agent-invoked, not user-invoked) |
| MCP bridge | Unchanged (new tools are native, not MCP) |
| Path security | Reused by new tools |
| Sandbox | Reused by background tools |
| Syntax check | Reused by file tools (existing) |
| Event bus | Unchanged (existing audit captures new tools) |
| Replay system | Unchanged (tool calls are traced automatically) |
