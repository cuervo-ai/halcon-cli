# Engineering Tool Implementation Plan

> Phase 4 deliverable for the Engineering Utility Layer design.
> Input: [ENGINEERING_TOOL_ARCHITECTURE.md](./ENGINEERING_TOOL_ARCHITECTURE.md)

---

## 1. Implementation Phases

### Overview

| Phase | Tools | Est. Tests | Priority | Depends On |
|-------|-------|-----------|----------|------------|
| I-A | Git Status, Diff, Log | ~20 | Tier 1 (P15) | Helpers module |
| I-B | Git Add, Commit | ~15 | Tier 1 (P15) | Phase I-A |
| I-C | Background Start/Output/Kill | ~18 | Tier 1 (P12) | ProcessRegistry |
| I-D | Web Search | ~12 | Tier 1 (P12) | SearchProvider |
| I-E | File Delete, HTTP Request | ~21 | Tier 2 (P6) | None |
| I-F | Task Tracking, Fuzzy Find | ~24 | Tier 2 (P6) | None |
| I-G | Symbol Search | ~12 | Tier 2 (P6) | repo_map module |
| I-H | Registry Update + Contract Tests | ~10 | All | All above |

Total: ~132 new tests. Target: 1797 + 132 = ~1929 workspace tests.

---

## 2. Phase I-A: Git Read Tools (Status, Diff, Log)

### Files to Create

| File | Purpose | Lines (est.) |
|------|---------|-------------|
| `crates/cuervo-tools/src/git/mod.rs` | Module declarations + re-exports | 20 |
| `crates/cuervo-tools/src/git/helpers.rs` | `run_git_command()`, `is_git_repo()`, `parse_porcelain_v2()`, `parse_diff_stat()` | 180 |
| `crates/cuervo-tools/src/git/status.rs` | `GitStatusTool` + tests | 200 |
| `crates/cuervo-tools/src/git/diff.rs` | `GitDiffTool` + tests | 180 |
| `crates/cuervo-tools/src/git/log.rs` | `GitLogTool` + tests | 150 |

### Files to Modify

| File | Change |
|------|--------|
| `crates/cuervo-tools/src/lib.rs` | Add `pub mod git;` |
| `crates/cuervo-tools/Cargo.toml` | No changes (uses `std::process::Command`) |

### Implementation Steps

1. Create `git/helpers.rs`:
   - `run_git_command(working_dir, args, timeout)` -- wraps `std::process::Command` with timeout via `tokio::time::timeout`
   - `is_git_repo(working_dir)` -- runs `git rev-parse --is-inside-work-tree`
   - `parse_porcelain_v2(output)` -- parses `git status --porcelain=v2 --branch` into `GitStatusInfo`
   - `parse_diff_stat(output)` -- parses `git diff --stat` summary line
   - Tests: 5 (parse porcelain, parse stat, is_git_repo true/false, command timeout)

2. Create `git/status.rs`:
   - Implement `Tool` trait for `GitStatusTool`
   - Runs `git status --porcelain=v2 --branch`
   - Parses into structured output (branch, staged, modified, untracked)
   - Human-readable content + JSON metadata
   - Tests: 5 (clean repo, staged file, untracked file, not a repo, modified file)

3. Create `git/diff.rs`:
   - Implement `Tool` trait for `GitDiffTool`
   - Input: `staged` (bool), `path` (optional), `commit` (optional)
   - Runs `git diff` or `git diff --staged` or `git diff <commit>`
   - Truncates large diffs (>16K chars) with stat summary preserved
   - Tests: 5 (no changes, unstaged change, staged change, specific file, truncation)

4. Create `git/log.rs`:
   - Implement `Tool` trait for `GitLogTool`
   - Input: `count` (default 10), `path` (optional), `oneline` (default true)
   - Runs `git log --oneline -N` or `git log --format=...`
   - Tests: 5 (basic log, with count, specific file, oneline vs full, empty repo)

### Verification
```bash
cargo test -p cuervo-tools -- git
cargo clippy -p cuervo-tools
```

---

## 3. Phase I-B: Git Write Tools (Add, Commit)

### Files to Create

| File | Purpose | Lines (est.) |
|------|---------|-------------|
| `crates/cuervo-tools/src/git/add.rs` | `GitAddTool` + tests | 140 |
| `crates/cuervo-tools/src/git/commit.rs` | `GitCommitTool` + tests | 160 |

### Implementation Steps

1. Create `git/add.rs`:
   - Input: `paths` (string array, required)
   - Validates each path exists relative to working directory
   - Runs `git add -- <path1> <path2> ...`
   - No `git add .` or `git add -A` allowed (explicit paths only)
   - Permission: ReadWrite
   - Tests: 5 (add single file, add multiple, nonexistent file, already staged, not a repo)

2. Create `git/commit.rs`:
   - Input: `message` (string, required)
   - Runs `git commit -m "<message>"`
   - Permission: Destructive
   - `requires_confirmation()` -> always true
   - Returns commit hash in metadata
   - Tests: 5 (basic commit, empty staging area, message with special chars, commit hash in output, requires confirmation)

3. Update `git/mod.rs` with new pub uses.

### Verification
```bash
cargo test -p cuervo-tools -- git
```

---

## 4. Phase I-C: Background Job Management

### Files to Create

| File | Purpose | Lines (est.) |
|------|---------|-------------|
| `crates/cuervo-tools/src/background/mod.rs` | `ProcessRegistry` + re-exports | 100 |
| `crates/cuervo-tools/src/background/start.rs` | `BackgroundStartTool` + tests | 180 |
| `crates/cuervo-tools/src/background/output.rs` | `BackgroundOutputTool` + tests | 120 |
| `crates/cuervo-tools/src/background/kill.rs` | `BackgroundKillTool` + tests | 120 |

### Implementation Steps

1. Create `background/mod.rs`:
   - `ProcessRegistry` with `HashMap<String, BackgroundProcess>`
   - `start()` -> spawns process, generates ID (`bg-{n}`), stores in registry
   - `get_output(id)` -> reads accumulated stdout/stderr
   - `kill(id)` -> sends SIGKILL, removes from registry
   - `cleanup_finished()` -> removes completed processes
   - Max concurrent limit enforcement
   - Tests: 4 (registry lifecycle, max concurrent, cleanup, ID generation)

2. Create `background/start.rs`:
   - Spawns via `tokio::process::Command`
   - Applies sandbox rlimits (same as bash tool)
   - Returns job_id in metadata
   - Permission: Destructive, requires confirmation
   - Tests: 5 (start simple command, start with timeout, start when at max capacity, sandbox applied, job_id format)

3. Create `background/output.rs`:
   - Reads stdout/stderr buffer for given job_id
   - Returns running status, elapsed time
   - Tests: 4 (running job output, completed job output, unknown job_id, empty output)

4. Create `background/kill.rs`:
   - Sends SIGTERM then SIGKILL after 5s grace period
   - Returns exit code if available
   - Tests: 5 (kill running job, kill already finished, kill unknown id, confirm required, kill returns exit code)

### Verification
```bash
cargo test -p cuervo-tools -- background
```

---

## 5. Phase I-D: Web Search

### Files to Create

| File | Purpose | Lines (est.) |
|------|---------|-------------|
| `crates/cuervo-tools/src/web_search.rs` | `WebSearchTool` + `SearchProvider` + tests | 250 |

### Implementation Steps

1. Define `SearchProvider` enum:
   - `Brave { api_key: String }` -- Brave Search API (primary)
   - `Configurable { base_url: String, api_key: Option<String> }` -- generic REST search

2. Implement `WebSearchTool`:
   - Input: `query` (required), `count` (default 5, max 10), `domain_filter` (optional)
   - Uses `reqwest` (already a dep) to call search API
   - Parses JSON response into structured results
   - Formats as numbered list with title, URL, snippet
   - Permission: ReadOnly

3. Brave Search API integration:
   - Endpoint: `https://api.search.brave.com/res/v1/web/search`
   - Headers: `X-Subscription-Token: <api_key>`
   - Query params: `q`, `count`, `search_lang=en`

4. Tests:
   - Schema validation (2 tests)
   - Missing query error (1 test)
   - Permission level (1 test)
   - Result formatting (2 tests -- use mock response)
   - Domain filter (1 test)
   - Count limiting (1 test)
   - Empty results (1 test)
   - API key missing error (1 test)
   - Provider selection (2 tests)

### Config Changes

Add to `crates/cuervo-core/src/types/config.rs`:
```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchConfig {
    pub provider: String,
    pub api_key: String,
}
```

### Verification
```bash
cargo test -p cuervo-tools -- web_search
```

---

## 6. Phase I-E: File Delete + HTTP Request

### Files to Create

| File | Purpose | Lines (est.) |
|------|---------|-------------|
| `crates/cuervo-tools/src/file_delete.rs` | `FileDeleteTool` + tests | 150 |
| `crates/cuervo-tools/src/http_request.rs` | `HttpRequestTool` + tests | 200 |

### Implementation Steps

1. `file_delete.rs`:
   - Path validation via `path_security::resolve_and_validate()`
   - `std::fs::remove_file()` (no recursive delete)
   - Returns file size in metadata (stat before delete)
   - Permission: Destructive, always requires confirmation
   - Tests: 9 (delete existing, nonexistent, blocked pattern, outside allowed, directory rejected, confirm required, relative path, permission level, schema)

2. `http_request.rs`:
   - Input: url, method (POST/PUT/DELETE/PATCH), headers (object), body (string), timeout
   - Uses `reqwest::Client` with configured method
   - Response: status code, body (truncated to 1MB), headers
   - Permission: Destructive (all write methods require confirmation)
   - Tests: 12 (each method, headers, body, timeout, error status, large response truncation, confirm required, schema, missing url, invalid method, content-type detection, redirect handling)

### Verification
```bash
cargo test -p cuervo-tools -- file_delete
cargo test -p cuervo-tools -- http_request
```

---

## 7. Phase I-F: Task Tracking + Fuzzy Find

### Files to Create

| File | Purpose | Lines (est.) |
|------|---------|-------------|
| `crates/cuervo-tools/src/task_track.rs` | `TaskTrackTool` + tests | 200 |
| `crates/cuervo-tools/src/fuzzy_find.rs` | `FuzzyFindTool` + tests | 180 |

### Implementation Steps

1. `task_track.rs`:
   - In-memory `Vec<AgentTask>` with `{content, status, active_form}`
   - Actions: add (appends), update (by index), list (all)
   - Constraint: only one `in_progress` at a time
   - Permission: ReadOnly (internal agent state)
   - Tests: 13 (add task, update status, list empty, list populated, only one in_progress, update nonexistent index, add multiple, complete all, schema, permission, status transitions, active_form display, boundary index)

2. `fuzzy_find.rs`:
   - Walk directory tree (skip hidden, node_modules, target, .git)
   - Score each path by subsequence match against query
   - Return top-N results sorted by score
   - Permission: ReadOnly
   - Tests: 11 (exact match, subsequence, no match, nested directories, hidden files skipped, max results, empty query, base path, relative paths, score ordering, large directory)

### Verification
```bash
cargo test -p cuervo-tools -- task_track
cargo test -p cuervo-tools -- fuzzy_find
```

---

## 8. Phase I-G: Symbol Search

### Files to Create

| File | Purpose | Lines (est.) |
|------|---------|-------------|
| `crates/cuervo-tools/src/symbol_search.rs` | `SymbolSearchTool` + tests | 180 |

### Dependencies

Depends on `cuervo-cli/src/repo_map.rs` module. Since cuervo-tools cannot depend on cuervo-cli (circular), the symbol extraction logic needs to be:

**Option A**: Move `repo_map.rs` to `cuervo-core` (breaks zero-I/O rule)
**Option B**: Move `repo_map.rs` to a new `cuervo-analysis` crate
**Option C**: Duplicate the regex-based extraction in cuervo-tools (simple, ~60 lines)
**Option D**: Create the tool in `cuervo-cli` instead of `cuervo-tools`

**Recommended: Option C**. The regex extraction is ~60 lines of pure logic. Duplicating avoids architectural changes. The patterns are stable (Phase 8B, 32 tests).

### Implementation Steps

1. Add symbol extraction functions directly in `symbol_search.rs`:
   - Regex patterns for Rust, Python, JS, TS, Go (copied from repo_map.rs)
   - `extract_symbols(path) -> Vec<Symbol>`
   - `search_symbols(dir, query, kind_filter) -> Vec<SymbolMatch>`

2. Implement `SymbolSearchTool`:
   - Input: `query`, `kind` (optional filter), `path` (optional directory)
   - Walks directory, extracts symbols, filters by query (case-insensitive substring)
   - Returns formatted matches with file:line and signature
   - Permission: ReadOnly
   - Tests: 12 (rust function, python class, js function, filter by kind, no match, directory scope, multiple matches, case insensitive, signature display, schema, permission, empty directory)

### Verification
```bash
cargo test -p cuervo-tools -- symbol_search
```

---

## 9. Phase I-H: Registry Update + Contract Tests

### Files to Modify

| File | Change |
|------|--------|
| `crates/cuervo-tools/src/lib.rs` | Add all new modules, update `default_registry()`, update contract tests |
| `crates/cuervo-tools/Cargo.toml` | No new deps needed |

### Implementation Steps

1. Add module declarations:
   ```rust
   pub mod git;
   pub mod background;
   pub mod file_delete;
   pub mod web_search;
   pub mod http_request;
   pub mod task_track;
   pub mod symbol_search;
   pub mod fuzzy_find;
   ```

2. Update `default_registry()`:
   - Add new `process_registry` parameter
   - Register all 14 new tools (or 15 with web_search if configured)
   - Update function signature

3. Update `all_tools()` in contract tests:
   - Include all new tools
   - Update tool count assertion (9 -> 23)

4. Add new contract tests:
   - Destructive tools require confirmation (git_commit, file_delete, background_start, http_request)
   - All new tool names are unique
   - All new schemas are valid

5. Update agent loop call site:
   - `default_registry()` now needs `ProcessRegistry` -- create in REPL before agent loop

### Tests: ~10

### Verification
```bash
cargo test --workspace
cargo clippy --workspace
```

---

## 10. Implementation Schedule

```
Phase I-A (Git Read)        ██████████░░░░░░░░░░░░░░  ~20 tests
Phase I-B (Git Write)       ░░░░░░░░░░██████░░░░░░░░  ~15 tests, depends on I-A
Phase I-C (Background)      ░░░░░░░░░░░░░░░░████████  ~18 tests, independent
Phase I-D (Web Search)      ████████████░░░░░░░░░░░░  ~12 tests, independent
Phase I-E (Delete+HTTP)     ░░░░░░████████████░░░░░░  ~21 tests, independent
Phase I-F (Task+Fuzzy)      ░░░░░░░░░░████████████░░  ~24 tests, independent
Phase I-G (Symbol Search)   ░░░░░░░░░░░░░░░░████████  ~12 tests, independent
Phase I-H (Registry)        ░░░░░░░░░░░░░░░░░░░░████  ~10 tests, depends on all
```

Phases I-A/I-D/I-E/I-F can run in parallel. I-B depends on I-A. I-H depends on all.

### Parallelization Strategy

```
Batch 1 (parallel):  I-A + I-D + I-E (first half)
Batch 2 (parallel):  I-B + I-C + I-E (second half) + I-F
Batch 3 (parallel):  I-G
Batch 4 (sequential): I-H (integration)
```

---

## 11. Risk Mitigation

| Risk | Impact | Mitigation |
|------|--------|-----------|
| `git` binary not installed | Git tools fail | `is_git_repo()` check first, clear error message |
| Search API rate limiting | Web search fails | Retry with backoff, cache recent queries |
| Background process leak | Orphaned processes | `Drop` impl on `ProcessRegistry` kills all children |
| Large git diffs OOM | Memory spike | Stream-based truncation, stat-only fallback |
| Symbol extraction regex miss | Incomplete results | Test against real codebase files, accept imperfection |
| Tool count > 20 | Context window pressure | Document tool descriptions concisely, consider tool search |

---

## 12. Success Criteria

Per-phase:
- All tests pass (`cargo test -p cuervo-tools`)
- Clippy clean (`cargo clippy -p cuervo-tools`)
- No new dependencies added to Cargo.toml (except config types in cuervo-core)

Final:
- `cargo test --workspace` passes (~1929 tests)
- `cargo clippy --workspace` clean
- Release binary size <= 6.0MB
- All 23+ tools registered and functional
- Contract tests validate all tools
