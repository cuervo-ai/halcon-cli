# Engineering Tool Architecture

> Phase 3 deliverable for the Engineering Utility Layer design.
> Input: [ENGINEERING_UTILITIES_SOTA.md](./ENGINEERING_UTILITIES_SOTA.md) | [ENGINEERING_UTILITY_TAXONOMY.md](./ENGINEERING_UTILITY_TAXONOMY.md)

---

## 1. Existing Architecture (Baseline)

### 1.1 Core Trait

```rust
// crates/cuervo-core/src/traits/tool.rs
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn permission_level(&self) -> PermissionLevel;
    async fn execute(&self, input: ToolInput) -> Result<ToolOutput>;
    fn requires_confirmation(&self, _input: &ToolInput) -> bool { ... }
    fn input_schema(&self) -> serde_json::Value;
}
```

### 1.2 Types

```rust
pub enum PermissionLevel { ReadOnly, ReadWrite, Destructive }

pub struct ToolInput {
    pub tool_use_id: String,
    pub arguments: serde_json::Value,
    pub working_directory: String,
}

pub struct ToolOutput {
    pub tool_use_id: String,
    pub content: String,           // Human-readable result
    pub is_error: bool,
    pub metadata: Option<serde_json::Value>,  // Structured data
}
```

### 1.3 Registry & Default Population

```rust
// crates/cuervo-tools/src/lib.rs
pub fn default_registry(config: &ToolsConfig) -> ToolRegistry {
    // Registers 9 tools: file_read, file_write, file_edit, bash,
    // glob, grep, web_fetch, directory_tree, file_inspect
}
```

### 1.4 Cross-Cutting Infrastructure

| Module | Location | Reusable By |
|--------|----------|-------------|
| `path_security` | `cuervo-tools/src/path_security.rs` | All file/git tools |
| `sandbox` | `cuervo-tools/src/sandbox.rs` | bash, background tools |
| `syntax_check` | `cuervo-tools/src/syntax_check.rs` | file_write, file_edit |
| `ToolRegistry` | `cuervo-tools/src/registry.rs` | All tools |

### 1.5 Conventions

All existing tools follow these patterns:
1. Tool struct holds config (allowed_dirs, blocked_patterns, timeout, etc.)
2. `execute()` validates input, resolves paths, performs operation, returns structured output
3. Metadata includes operation-specific data (line counts, match counts, etc.)
4. Tests use `tempfile::TempDir` for filesystem isolation
5. Contract tests in `lib.rs` verify name, description, schema, uniqueness

---

## 2. New Tool Designs

### 2.1 Git Tools (5 tools)

All git tools operate via `std::process::Command` calling the `git` binary (same approach as bash tool but with structured output parsing).

#### 2.1.1 `git_status`

```rust
pub struct GitStatusTool;

impl Tool for GitStatusTool {
    fn name(&self) -> &str { "git_status" }
    fn permission_level(&self) -> PermissionLevel { PermissionLevel::ReadOnly }

    // Input schema:
    // { "short": bool (optional, default false) }

    // Output content (human-readable):
    // "On branch main\nChanges staged:\n  M src/lib.rs\nUntracked:\n  new_file.rs"

    // Metadata:
    // { "branch": "main", "staged": ["src/lib.rs"], "modified": [],
    //   "untracked": ["new_file.rs"], "is_clean": false }
}
```

**Key decisions**:
- ReadOnly: never modifies state
- Runs `git status --porcelain=v2 --branch` for structured parsing
- Falls back to `git status` if porcelain fails
- No path_security needed (reads repo state, not arbitrary files)
- Working directory from ToolInput determines which repo

#### 2.1.2 `git_diff`

```rust
pub struct GitDiffTool;

// Input schema:
// {
//   "staged": bool (optional, default false -- show unstaged changes),
//   "path": string (optional -- diff specific file),
//   "commit": string (optional -- diff against specific commit)
// }

// Permission: ReadOnly
// Output: unified diff text
// Metadata: { "files_changed": 3, "insertions": 42, "deletions": 15, "truncated": bool }
```

**Token budgeting**: large diffs truncated to ~4000 tokens. Shows stat summary even when diff is truncated.

#### 2.1.3 `git_log`

```rust
pub struct GitLogTool;

// Input schema:
// {
//   "count": integer (optional, default 10, max 50),
//   "path": string (optional -- log for specific file),
//   "oneline": bool (optional, default true)
// }

// Permission: ReadOnly
// Output: formatted commit log
// Metadata: { "commit_count": 10, "oldest_hash": "abc123", "newest_hash": "def456" }
```

#### 2.1.4 `git_add`

```rust
pub struct GitAddTool;

// Input schema:
// { "paths": string[] (required -- files to stage) }

// Permission: ReadWrite
// Output: "Staged 3 files"
// Metadata: { "staged_paths": ["src/lib.rs", "src/main.rs", "Cargo.toml"] }
```

**Safety**: validates each path exists. No `git add .` or `git add -A` (too dangerous). Explicit paths only.

#### 2.1.5 `git_commit`

```rust
pub struct GitCommitTool;

// Input schema:
// { "message": string (required) }

// Permission: Destructive
// requires_confirmation() -> true (always)
// Output: "Created commit abc1234: Fix authentication bug"
// Metadata: { "hash": "abc1234", "message": "Fix auth bug", "files_changed": 3 }
```

**Safety**: always requires confirmation. No `--amend` support (too dangerous without explicit user intent). No `--no-verify` (respects hooks).

#### Git Tool Module Layout

```
crates/cuervo-tools/src/
├── git/
│   ├── mod.rs          // pub use of all git tools
│   ├── status.rs       // GitStatusTool
│   ├── diff.rs         // GitDiffTool
│   ├── log.rs          // GitLogTool
│   ├── add.rs          // GitAddTool
│   ├── commit.rs       // GitCommitTool
│   └── helpers.rs      // Shared: run_git_command(), parse_porcelain(), is_git_repo()
```

#### Shared Git Helpers

```rust
// crates/cuervo-tools/src/git/helpers.rs

/// Run a git command in the given working directory.
/// Returns (stdout, stderr, exit_code).
pub fn run_git_command(
    working_dir: &str,
    args: &[&str],
    timeout_secs: u64,
) -> Result<GitCommandResult>;

/// Check if the working directory is inside a git repository.
pub fn is_git_repo(working_dir: &str) -> bool;

/// Parse `git status --porcelain=v2 --branch` output.
pub fn parse_porcelain_v2(output: &str) -> GitStatusInfo;

/// Parse `git diff --stat` output.
pub fn parse_diff_stat(output: &str) -> DiffStat;

pub struct GitCommandResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

pub struct GitStatusInfo {
    pub branch: String,
    pub staged: Vec<String>,
    pub modified: Vec<String>,
    pub untracked: Vec<String>,
    pub is_clean: bool,
}

pub struct DiffStat {
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
}
```

---

### 2.2 Web Search Tool

```rust
pub struct WebSearchTool {
    provider: SearchProvider,
}

pub enum SearchProvider {
    Brave { api_key: String },
    Google { api_key: String, cx: String },
    SearXNG { base_url: String },
}

// Input schema:
// {
//   "query": string (required, minLength 2),
//   "count": integer (optional, default 5, max 10),
//   "domain_filter": string (optional -- restrict to domain)
// }

// Permission: ReadOnly
// Output: "1. Title - URL\n   Snippet...\n2. Title - URL\n   Snippet..."
// Metadata: { "result_count": 5, "provider": "brave",
//             "results": [{"title": "...", "url": "...", "snippet": "..."}] }
```

**Provider abstraction**: `SearchProvider` trait with implementations for multiple backends. Default: Brave Search (cheapest, no Google overhead). API key from config or environment variable.

**Module**: `crates/cuervo-tools/src/web_search.rs`

---

### 2.3 Background Job Tools (3 tools)

These tools manage long-running shell commands. A process registry persists across tool invocations within a session.

```rust
// Shared state (held in Arc<Mutex<ProcessRegistry>>)
pub struct ProcessRegistry {
    processes: HashMap<String, BackgroundProcess>,
    next_id: u32,
}

pub struct BackgroundProcess {
    child: tokio::process::Child,
    stdout_buffer: Vec<u8>,
    stderr_buffer: Vec<u8>,
    started_at: std::time::Instant,
    command: String,
}
```

#### 2.3.1 `background_start`

```rust
pub struct BackgroundStartTool {
    registry: Arc<Mutex<ProcessRegistry>>,
    sandbox: SandboxConfig,
}

// Input schema:
// {
//   "command": string (required),
//   "timeout_secs": integer (optional, default 300, max 3600)
// }

// Permission: Destructive (same as bash)
// requires_confirmation() -> true
// Output: "Started background job bg-1: npm run dev"
// Metadata: { "job_id": "bg-1", "pid": 12345, "command": "npm run dev" }
```

#### 2.3.2 `background_output`

```rust
pub struct BackgroundOutputTool {
    registry: Arc<Mutex<ProcessRegistry>>,
}

// Input schema:
// { "job_id": string (required) }

// Permission: ReadOnly
// Output: captured stdout/stderr since last check
// Metadata: { "job_id": "bg-1", "running": true, "elapsed_secs": 42,
//             "stdout_bytes": 1234, "stderr_bytes": 56 }
```

#### 2.3.3 `background_kill`

```rust
pub struct BackgroundKillTool {
    registry: Arc<Mutex<ProcessRegistry>>,
}

// Input schema:
// { "job_id": string (required) }

// Permission: Destructive
// Output: "Killed job bg-1 (pid 12345)"
// Metadata: { "job_id": "bg-1", "exit_code": null, "killed": true }
```

**Module layout**:
```
crates/cuervo-tools/src/
├── background/
│   ├── mod.rs          // ProcessRegistry + pub use
│   ├── start.rs        // BackgroundStartTool
│   ├── output.rs       // BackgroundOutputTool
│   └── kill.rs         // BackgroundKillTool
```

---

### 2.4 File Delete Tool

```rust
pub struct FileDeleteTool {
    allowed_dirs: Vec<PathBuf>,
    blocked_patterns: Vec<String>,
}

// Input schema:
// { "path": string (required) }

// Permission: Destructive
// requires_confirmation() -> true (always)
// Output: "Deleted: /path/to/file.txt (1234 bytes)"
// Metadata: { "path": "/path/to/file.txt", "size_bytes": 1234 }
```

**Safety**: path_security validation, confirmation required, blocked patterns enforced. No recursive delete (no `rm -rf`). Single file only.

**Module**: `crates/cuervo-tools/src/file_delete.rs`

---

### 2.5 HTTP Request Tool (extended web_fetch)

Rather than creating a new tool, extend the existing `web_fetch` tool:

```rust
pub struct WebFetchTool;

// Extended input schema (backward compatible):
// {
//   "url": string (required),
//   "method": string (optional, default "GET" -- "GET"|"POST"|"PUT"|"DELETE"|"PATCH"),
//   "headers": object (optional -- {"Authorization": "Bearer ...", "Content-Type": "..."}),
//   "body": string (optional -- request body for POST/PUT/PATCH),
//   "timeout": integer (optional, default 30)
// }

// Permission: ReadOnly for GET, ReadWrite for POST/PUT/PATCH, Destructive for DELETE
// (Override permission_level based on method — new pattern)
```

**New pattern**: `permission_level()` reads from the input arguments. Requires a new trait method or the existing `requires_confirmation(input)` can gate this.

Actually, better approach: keep `web_fetch` as ReadOnly for GET. Create `http_request` for methods that modify:

```rust
pub struct HttpRequestTool;

// Input schema:
// {
//   "url": string (required),
//   "method": string (required -- "POST"|"PUT"|"DELETE"|"PATCH"),
//   "headers": object (optional),
//   "body": string (optional),
//   "timeout": integer (optional, default 30)
// }

// Permission: Destructive (all write methods)
// requires_confirmation() -> true
```

**Module**: `crates/cuervo-tools/src/http_request.rs`

---

### 2.6 Task Tracking Tool

```rust
pub struct TaskTrackTool;

// Input schema:
// {
//   "action": string (required -- "add"|"update"|"list"),
//   "content": string (required for "add", optional for "update"),
//   "task_index": integer (required for "update"),
//   "status": string (required for "update" -- "pending"|"in_progress"|"completed")
// }

// Permission: ReadOnly (managing internal agent state, not modifying user files)
// Output: formatted task list
// Metadata: { "task_count": 5, "pending": 2, "in_progress": 1, "completed": 2 }
```

**State**: tasks stored in `Vec<AgentTask>` held by the tool instance. Not persisted to DB (session-local). This mirrors Claude Code's TodoWrite which tracks tasks within the conversation.

**Constraint**: only ONE task can be `in_progress` at a time (Claude Code pattern).

**Module**: `crates/cuervo-tools/src/task_track.rs`

---

### 2.7 Symbol Search Tool

Extends the existing `repo_map.rs` module (Phase 8B) with a queryable tool interface.

```rust
pub struct SymbolSearchTool;

// Input schema:
// {
//   "query": string (required -- symbol name or partial match),
//   "kind": string (optional -- "function"|"struct"|"enum"|"trait"|"class"|"interface"|"constant"),
//   "path": string (optional -- restrict search to directory)
// }

// Permission: ReadOnly
// Output: "Found 3 matches:\n  src/lib.rs:42 fn compute_fingerprint(messages: &[Message])\n  ..."
// Metadata: { "match_count": 3, "symbols": [{ "name": "...", "kind": "function",
//             "file": "src/lib.rs", "line": 42, "signature": "fn ..." }] }
```

**Implementation**: uses `cuervo_cli::repo_map::build_repo_map()` + filter by query. Regex-based symbol extraction already exists (Rust, Python, JS, TS, Go). No new dependencies.

**Module**: `crates/cuervo-tools/src/symbol_search.rs`

---

### 2.8 Fuzzy File Search Tool

```rust
pub struct FuzzyFindTool;

// Input schema:
// {
//   "query": string (required -- partial path or filename),
//   "path": string (optional -- base directory, default cwd),
//   "max_results": integer (optional, default 20, max 50)
// }

// Permission: ReadOnly
// Output: ranked list of matching file paths
// Metadata: { "match_count": 15, "truncated": false }
```

**Algorithm**: subsequence matching on path components. Score = length of longest common subsequence / query length. No external dependencies -- implement directly.

**Module**: `crates/cuervo-tools/src/fuzzy_find.rs`

---

### 2.9 User Question Tool

```rust
pub struct AskUserTool {
    sender: tokio::sync::mpsc::Sender<UserQuestion>,
    receiver: Arc<Mutex<tokio::sync::oneshot::Receiver<String>>>,
}

pub struct UserQuestion {
    pub question: String,
    pub options: Vec<String>,
    pub response_channel: tokio::sync::oneshot::Sender<String>,
}

// Input schema:
// {
//   "question": string (required),
//   "options": string[] (optional -- predefined choices)
// }

// Permission: ReadOnly (asking a question doesn't modify anything)
// Output: user's response text
// Metadata: { "question": "...", "selected_option": 0 }
```

**Integration**: the REPL loop handles the `UserQuestion` by displaying the question, collecting input, and sending the response back through the oneshot channel.

**Module**: `crates/cuervo-tools/src/ask_user.rs`

---

## 3. Registration Changes

### 3.1 Updated `default_registry()`

```rust
pub fn default_registry(
    config: &ToolsConfig,
    process_registry: Arc<Mutex<ProcessRegistry>>,  // NEW
) -> ToolRegistry {
    let mut reg = ToolRegistry::new();

    // Existing tools (unchanged)
    reg.register(Arc::new(file_read::FileReadTool::new(...)));
    reg.register(Arc::new(file_write::FileWriteTool::new(...)));
    reg.register(Arc::new(file_edit::FileEditTool::new(...)));
    reg.register(Arc::new(bash::BashTool::new(...)));
    reg.register(Arc::new(glob_tool::GlobTool::new()));
    reg.register(Arc::new(grep::GrepTool::new()));
    reg.register(Arc::new(web_fetch::WebFetchTool::new()));
    reg.register(Arc::new(directory_tree::DirectoryTreeTool::new()));
    reg.register(Arc::new(file_inspect::FileInspectTool::new(...)));

    // NEW: Git tools
    reg.register(Arc::new(git::GitStatusTool::new()));
    reg.register(Arc::new(git::GitDiffTool::new()));
    reg.register(Arc::new(git::GitLogTool::new()));
    reg.register(Arc::new(git::GitAddTool::new()));
    reg.register(Arc::new(git::GitCommitTool::new()));

    // NEW: Web search (if API key configured)
    if let Some(search_config) = &config.search {
        reg.register(Arc::new(web_search::WebSearchTool::new(search_config.clone())));
    }

    // NEW: Background job management
    reg.register(Arc::new(background::BackgroundStartTool::new(
        process_registry.clone(), config.sandbox.clone(),
    )));
    reg.register(Arc::new(background::BackgroundOutputTool::new(
        process_registry.clone(),
    )));
    reg.register(Arc::new(background::BackgroundKillTool::new(
        process_registry.clone(),
    )));

    // NEW: File operations
    reg.register(Arc::new(file_delete::FileDeleteTool::new(
        config.allowed_directories.clone(), config.blocked_patterns.clone(),
    )));

    // NEW: HTTP request (write methods)
    reg.register(Arc::new(http_request::HttpRequestTool::new()));

    // NEW: Task tracking
    reg.register(Arc::new(task_track::TaskTrackTool::new()));

    // NEW: Code intelligence
    reg.register(Arc::new(symbol_search::SymbolSearchTool::new()));
    reg.register(Arc::new(fuzzy_find::FuzzyFindTool::new()));

    reg
}
```

### 3.2 Tool Count Progression

| Phase | Tool Count |
|-------|-----------|
| Phase 18 (current) | 9 |
| + Git tools | 14 |
| + Web search | 15 |
| + Background jobs | 18 |
| + File delete | 19 |
| + HTTP request | 20 |
| + Task tracking | 21 |
| + Symbol search | 22 |
| + Fuzzy find | 23 |
| + Ask user (later) | 24 |

When tool count exceeds ~20, consider implementing tool search (on-demand loading) similar to Claude Code's MCP tool search pattern.

---

## 4. Config Extensions

### 4.1 ToolsConfig Changes

```rust
pub struct ToolsConfig {
    // Existing fields (unchanged)
    pub confirm_destructive: bool,
    pub timeout_secs: u64,
    pub allowed_directories: Vec<PathBuf>,
    pub blocked_patterns: Vec<String>,
    pub sandbox: SandboxConfig,

    // NEW
    pub search: Option<SearchConfig>,
    pub git: GitToolsConfig,
    pub background: BackgroundConfig,
}

#[derive(Default)]
pub struct SearchConfig {
    pub provider: String,    // "brave" | "google" | "searxng"
    pub api_key: String,
    pub custom_search_cx: Option<String>,  // Google-specific
    pub base_url: Option<String>,          // SearXNG-specific
}

#[derive(Default)]
pub struct GitToolsConfig {
    pub enabled: bool,       // default true
    pub allow_push: bool,    // default false (require explicit enable)
    pub allow_commit: bool,  // default true
}

#[derive(Default)]
pub struct BackgroundConfig {
    pub max_concurrent: usize,     // default 5
    pub default_timeout_secs: u64, // default 300
    pub max_timeout_secs: u64,     // default 3600
}
```

### 4.2 TOML Config Example

```toml
[tools]
confirm_destructive = true
timeout_secs = 120

[tools.git]
enabled = true
allow_push = false
allow_commit = true

[tools.search]
provider = "brave"
api_key = "${BRAVE_API_KEY}"

[tools.background]
max_concurrent = 5
default_timeout_secs = 300
```

---

## 5. Error Handling Patterns

### 5.1 Git-Specific Errors

```rust
// All git tools use this pattern:
fn execute_git_tool(&self, input: ToolInput, args: &[&str]) -> Result<ToolOutput> {
    // 1. Check if working directory is a git repo
    if !git::helpers::is_git_repo(&input.working_directory) {
        return Err(CuervoError::ToolExecutionFailed {
            tool: self.name().into(),
            message: "not a git repository".into(),
        });
    }

    // 2. Run git command with timeout
    let result = git::helpers::run_git_command(
        &input.working_directory, args, self.timeout_secs,
    )?;

    // 3. Check exit code
    if result.exit_code != 0 {
        return Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content: format!("git error: {}", result.stderr),
            is_error: true,
            metadata: Some(json!({"exit_code": result.exit_code})),
        });
    }

    // 4. Parse and return structured output
    Ok(ToolOutput { ... })
}
```

### 5.2 Non-fatal Error Pattern

Git tools return `is_error: true` ToolOutput instead of `Err(...)` for expected failures (unstaged files, merge conflicts, etc.). This lets the agent see the error and decide how to proceed, rather than stopping execution.

---

## 6. Testing Strategy

### 6.1 Per-Tool Testing

| Tool | Test Approach | Fixture |
|------|--------------|---------|
| Git tools | `git init` in TempDir, create commits, test tool output | TempDir + git init |
| Web search | Mock HTTP server (mockito or local) | Mock responses |
| Background jobs | `sleep 1` command, check output/kill | Real processes with short timeout |
| File delete | TempDir files, verify removal | TempDir |
| HTTP request | httpbin.org or mock server | Mock responses |
| Task tracking | In-memory state, verify task lifecycle | None (pure state) |
| Symbol search | Create source files in TempDir, query symbols | TempDir + .rs/.py/.ts files |
| Fuzzy find | Create directory tree in TempDir, query paths | TempDir |

### 6.2 Contract Test Updates

`lib.rs::contract_tests` must be updated:
1. `all_tools()` vec includes all new tools
2. `default_registry_has_all_tools` assertion count updated
3. New `destructive_tools_require_confirmation` checks for git_commit, file_delete, background_start

### 6.3 Test Count Estimate

| Tool Category | Unit Tests | Integration Tests | Total |
|---------------|-----------|------------------|-------|
| Git (5 tools) | 25 | 10 | 35 |
| Web search | 8 | 4 | 12 |
| Background (3 tools) | 12 | 6 | 18 |
| File delete | 6 | 3 | 9 |
| HTTP request | 8 | 4 | 12 |
| Task tracking | 10 | 3 | 13 |
| Symbol search | 8 | 4 | 12 |
| Fuzzy find | 8 | 3 | 11 |
| Contract tests update | 5 | 0 | 5 |
| **Total** | **90** | **37** | **~127** |

Target: 1797 + 127 = ~1924 tests

---

## 7. Dependency Impact

### 7.1 New Dependencies

| Dependency | Crate | Required By | Feature-Gated? |
|-----------|-------|-------------|----------------|
| None | cuervo-tools | Git tools | No (uses `std::process::Command` + `git` binary) |
| None | cuervo-tools | File delete | No (uses `std::fs::remove_file`) |
| None | cuervo-tools | Task tracking | No (pure state) |
| None | cuervo-tools | Symbol search | No (reuses repo_map) |
| None | cuervo-tools | Fuzzy find | No (custom implementation) |
| None | cuervo-tools | Background jobs | No (uses `tokio::process`) |
| reqwest (already dep) | cuervo-tools | HTTP request | No |
| reqwest (already dep) | cuervo-tools | Web search | No |

**Binary impact**: ZERO new dependencies. All new tools use existing deps or `std`. This is by design -- every tool is implemented with minimal footprint.

### 7.2 Feature Gating

Only `web_search` should be feature-gated (requires API key configuration). All other tools are unconditionally available.

```toml
[features]
default = []
web-search = []  # Enables WebSearchTool registration
```

---

## 8. Module Layout (Final)

```
crates/cuervo-tools/src/
├── lib.rs                    // default_registry() + contract tests
├── registry.rs               // ToolRegistry
├── path_security.rs          // Path validation
├── sandbox.rs                // Process sandboxing
├── syntax_check.rs           // Code syntax validation
│
├── # Existing tools
├── file_read.rs
├── file_write.rs
├── file_edit.rs
├── file_inspect.rs
├── bash.rs
├── glob_tool.rs
├── grep.rs
├── directory_tree.rs
├── web_fetch.rs
│
├── # NEW: Git tools
├── git/
│   ├── mod.rs
│   ├── helpers.rs            // Shared git command helpers
│   ├── status.rs             // GitStatusTool
│   ├── diff.rs               // GitDiffTool
│   ├── log.rs                // GitLogTool
│   ├── add.rs                // GitAddTool
│   └── commit.rs             // GitCommitTool
│
├── # NEW: Background job management
├── background/
│   ├── mod.rs                // ProcessRegistry + pub use
│   ├── start.rs              // BackgroundStartTool
│   ├── output.rs             // BackgroundOutputTool
│   └── kill.rs               // BackgroundKillTool
│
├── # NEW: Individual tools
├── file_delete.rs            // FileDeleteTool
├── web_search.rs             // WebSearchTool
├── http_request.rs           // HttpRequestTool
├── task_track.rs             // TaskTrackTool
├── symbol_search.rs          // SymbolSearchTool
└── fuzzy_find.rs             // FuzzyFindTool
```
