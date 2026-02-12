# Engineering Utility Taxonomy

> Phase 2 deliverable for the Engineering Utility Layer design.
> Input: [ENGINEERING_UTILITIES_SOTA.md](./ENGINEERING_UTILITIES_SOTA.md)

---

## 1. Taxonomy Overview

Six capability domains, each containing concrete tool categories. Every tool is scored on three axes:

- **SOTA Coverage** (0-5): how many of the 6 reference agents provide this capability
- **Cuervo Gap** (0-3): 0 = already implemented, 1 = partially present, 2 = missing but infrastructure exists, 3 = missing entirely
- **Implementation Complexity** (S/M/L/XL): estimated effort in Cuervo's Rust architecture

Priority = SOTA Coverage * Cuervo Gap. Higher score = higher priority.

---

## 2. Domain A: File & Code Operations

### A.1 File Read (line-based, offset/limit)
- **SOTA Coverage**: 6/6 (all agents)
- **Cuervo Gap**: 0 (file_read exists)
- **Priority**: 0
- **Status**: Complete

### A.2 File Write (create/overwrite)
- **SOTA Coverage**: 6/6
- **Cuervo Gap**: 0 (file_write exists)
- **Priority**: 0
- **Status**: Complete

### A.3 File Edit (search-and-replace)
- **SOTA Coverage**: 6/6
- **Cuervo Gap**: 0 (file_edit exists)
- **Priority**: 0
- **Status**: Complete

### A.4 File Delete
- **SOTA Coverage**: 3/6 (Cursor, Devin, Replit)
- **Cuervo Gap**: 2 (infrastructure exists, no tool)
- **Priority**: 6
- **Complexity**: S
- **Notes**: Straightforward `std::fs::remove_file` with path_security. Cursor has explicit `delete_file`. Cuervo can use `bash rm` but a dedicated tool provides safer guardrails (confirmation, audit logging, blocked patterns).

### A.5 File Move/Rename
- **SOTA Coverage**: 2/6 (Devin, OpenHands via bash)
- **Cuervo Gap**: 2
- **Priority**: 4
- **Complexity**: S
- **Notes**: `std::fs::rename` with path_security for both source and destination. Low priority -- bash fallback works.

### A.6 Multi-File Edit (atomic batch)
- **SOTA Coverage**: 2/6 (Cursor MultiEdit, Aider multi-file diff)
- **Cuervo Gap**: 2
- **Priority**: 4
- **Complexity**: M
- **Notes**: Apply multiple edits atomically. Rollback on any failure. Reduces round-trips.

### A.7 File Inspect (format detection + metadata)
- **SOTA Coverage**: 2/6 (Cuervo is ahead here)
- **Cuervo Gap**: 0 (file_inspect exists, 12 handlers)
- **Priority**: 0
- **Status**: Complete (Phase 6 validated)

### A.8 Notebook Edit (Jupyter .ipynb)
- **SOTA Coverage**: 1/6 (Claude Code only)
- **Cuervo Gap**: 3
- **Priority**: 3
- **Complexity**: M
- **Notes**: Cell-level CRUD on .ipynb JSON. Growing demand for data science. Claude Code's NotebookEdit supports replace/insert/delete by cell_number or cell_id.

### A.9 Read-Before-Write Enforcement
- **SOTA Coverage**: 1/6 (Claude Code)
- **Cuervo Gap**: 3
- **Priority**: 3
- **Complexity**: S
- **Notes**: Track which files have been read in the session. Block Write/Edit on unread files. Prevents blind overwrites. Simple HashSet in agent loop state.

---

## 3. Domain B: Search & Navigation

### B.1 Glob (file pattern matching)
- **SOTA Coverage**: 6/6
- **Cuervo Gap**: 0 (glob tool exists)
- **Priority**: 0
- **Status**: Complete

### B.2 Grep (regex content search)
- **SOTA Coverage**: 6/6
- **Cuervo Gap**: 0 (grep tool exists)
- **Priority**: 0
- **Status**: Complete

### B.3 Directory Tree
- **SOTA Coverage**: 5/6
- **Cuervo Gap**: 0 (directory_tree exists)
- **Priority**: 0
- **Status**: Complete

### B.4 Fuzzy File Search
- **SOTA Coverage**: 2/6 (Cursor file_search, Replit search_filesystem)
- **Cuervo Gap**: 3
- **Priority**: 6
- **Complexity**: M
- **Notes**: Partial path matching (e.g., "auth/login" matches "src/auth/login.rs"). Levenshtein/subsequence matching on file paths. Cuervo could use the glob tool with wildcards, but dedicated fuzzy search is faster for agents.

### B.5 Semantic Code Search (RAG/embeddings)
- **SOTA Coverage**: 4/6 (Cursor, Devin, OpenHands via IPython, Replit)
- **Cuervo Gap**: 3
- **Priority**: 12 (highest in domain)
- **Complexity**: XL
- **Notes**: Vector embeddings for meaning-based search. Cursor's `codebase_search` is the gold standard. Requires embedding model, vector store, incremental indexing. Consider: local-only (no API dependency) vs. provider-backed. Alternative: BM25/TF-IDF (no embeddings needed) as stepping stone.

### B.6 Repo Map (structural codebase overview)
- **SOTA Coverage**: 2/6 (Aider, Cuervo)
- **Cuervo Gap**: 1 (repo_map exists as context source, not queryable tool)
- **Priority**: 2
- **Complexity**: S
- **Notes**: Cuervo already has repo_map.rs (Phase 8B, 32 tests) with tree-sitter-like regex extraction. Currently wired as a ContextSource. Could be exposed as a tool for on-demand querying (`/repo_map` or as agent tool).

### B.7 Symbol Search (go-to-definition, find-references)
- **SOTA Coverage**: 2/6 (Cursor via LSP, Aider via tree-sitter)
- **Cuervo Gap**: 3
- **Priority**: 6
- **Complexity**: L
- **Notes**: Find function/class/struct definitions and references. tree-sitter parsing gives definitions. Full LSP integration gives references too. Cuervo's repo_map already extracts symbols -- extend with search capability.

---

## 4. Domain C: Shell & Process Management

### C.1 Shell Execution (bash -c)
- **SOTA Coverage**: 6/6
- **Cuervo Gap**: 0 (bash tool exists)
- **Priority**: 0
- **Status**: Complete

### C.2 Background Job Management
- **SOTA Coverage**: 4/6 (Claude Code BashOutput/KillShell, Cursor is_background, Devin persistent shell, Replit)
- **Cuervo Gap**: 3
- **Priority**: 12 (highest in domain)
- **Complexity**: M
- **Notes**: Three operations: spawn-in-background, check-output, kill. Claude Code pattern: Bash(run_in_background) returns task_id, BashOutput(task_id) retrieves output, KillShell(task_id) terminates. Requires process registry (HashMap<String, Child>) in tool state.

### C.3 Package Management
- **SOTA Coverage**: 2/6 (Replit packager_tool, Devin via shell)
- **Cuervo Gap**: 3
- **Priority**: 6
- **Complexity**: M
- **Notes**: Dedicated install/uninstall for npm, pip, cargo. Replit's `packager_tool` detects language and uses correct package manager. Safer than raw bash because it validates package names. Lower priority -- bash fallback works for experienced users.

### C.4 Runtime Environment Setup
- **SOTA Coverage**: 1/6 (Replit programming_language_install_tool)
- **Cuervo Gap**: 3
- **Priority**: 3
- **Complexity**: L
- **Notes**: Install Python 3.12, Node 20, etc. Cloud-hosted agent feature. Not critical for local CLI.

---

## 5. Domain D: Version Control & Collaboration

### D.1 Git Status/Diff/Log
- **SOTA Coverage**: 5/6 (all except Aider which integrates natively)
- **Cuervo Gap**: 3
- **Priority**: 15 (HIGHEST OVERALL)
- **Complexity**: M
- **Notes**: `git_status`, `git_diff`, `git_log` tools. Read-only permission level. Provides repo state context without running full bash. Structured output (JSON metadata) vs. raw git output. Claude Code delegates to Bash, but structured tools are more efficient for agent reasoning.

### D.2 Git Add/Commit
- **SOTA Coverage**: 5/6
- **Cuervo Gap**: 3
- **Priority**: 15 (HIGHEST OVERALL, tied with D.1)
- **Complexity**: M
- **Notes**: `git_add` (ReadWrite), `git_commit` (Destructive). Aider auto-commits after every edit. Replit auto-commits at every major step. Dedicated tools prevent malformed commits and provide metadata (hash, diff stats).

### D.3 Git Branch/Checkout/Merge
- **SOTA Coverage**: 3/6 (Devin, Claude Code via bash, Aider via git)
- **Cuervo Gap**: 3
- **Priority**: 9
- **Complexity**: M
- **Notes**: Branch management tools. Higher risk (Destructive permission). Consider: separate tools vs. single `git` tool with subcommands.

### D.4 Git Push/Pull
- **SOTA Coverage**: 3/6
- **Cuervo Gap**: 3
- **Priority**: 9
- **Complexity**: S
- **Notes**: Network operations. Destructive permission level. Requires explicit user confirmation (force push = catastrophic).

### D.5 PR/Issue Management
- **SOTA Coverage**: 2/6 (Claude Code via gh CLI/MCP, Devin via Slack/Linear integration)
- **Cuervo Gap**: 3
- **Priority**: 6
- **Complexity**: L
- **Notes**: Create/review PRs, manage issues. Best done via GitHub MCP server rather than native tools. Cuervo's MCP bridge already supports this path.

---

## 6. Domain E: Network & Web

### E.1 HTTP Fetch (GET)
- **SOTA Coverage**: 6/6
- **Cuervo Gap**: 0 (web_fetch exists)
- **Priority**: 0
- **Status**: Complete

### E.2 Web Search (search engine)
- **SOTA Coverage**: 4/6 (Claude Code, Cursor, Devin, OpenHands via browser)
- **Cuervo Gap**: 3
- **Priority**: 12 (highest in domain)
- **Complexity**: M
- **Notes**: Query a search engine, return structured results. Claude Code uses a dedicated WebSearch tool with domain filtering. Options: Google Custom Search API, Brave Search API, SearXNG (self-hosted). Cuervo has web_fetch but no search. ReadOnly permission.

### E.3 HTTP POST/PUT/DELETE
- **SOTA Coverage**: 3/6 (Devin, OpenHands via browser, Claude Code via bash/curl)
- **Cuervo Gap**: 2 (web_fetch is GET-only)
- **Priority**: 6
- **Complexity**: S
- **Notes**: Extend web_fetch with method parameter. Add request body and headers. ReadWrite or Destructive depending on method.

### E.4 Browser Automation
- **SOTA Coverage**: 3/6 (OpenHands BrowserGym, Devin Chrome, Claude Computer Use)
- **Cuervo Gap**: 3
- **Priority**: 9
- **Complexity**: XL
- **Notes**: Headless Chromium via Playwright or browser-use. DOM extraction, accessibility tree, screenshot-based interaction. Very high complexity for CLI tool. Better as MCP server integration.

### E.5 API Client (authenticated requests)
- **SOTA Coverage**: 2/6 (Devin, OpenHands)
- **Cuervo Gap**: 3
- **Priority**: 6
- **Complexity**: M
- **Notes**: web_fetch with auth headers (Bearer, API key, Basic). Cookie jar. OAuth flows. Currently Cuervo's web_fetch has no auth support.

---

## 7. Domain F: Observability & Self-Improvement

### F.1 Task Tracking (TodoWrite)
- **SOTA Coverage**: 3/6 (Claude Code TodoWrite, OpenHands TaskTracking, Devin Planner)
- **Cuervo Gap**: 2 (agent_tasks table exists, no tool exposure)
- **Priority**: 6
- **Complexity**: S
- **Notes**: Cuervo has agent_tasks in the database and orchestrator task management. Missing: dedicated tool for the agent to manage its own task list during execution. Claude Code's TodoWrite: array of {content, status, activeForm}, only one in_progress at a time.

### F.2 Plan Generation
- **SOTA Coverage**: 4/6 (Claude Code EnterPlanMode, Cursor Plan mode, Devin Planner, OpenHands CodeAct)
- **Cuervo Gap**: 1 (LlmPlanner exists, not fully wired)
- **Priority**: 4
- **Complexity**: S
- **Notes**: Cuervo has planner.rs (Phase 8). Needs better tool exposure and wiring into the agent loop.

### F.3 Self-Correction Feedback
- **SOTA Coverage**: 4/6 (Replit reflection, Devin test-debug-fix, Aider lint+fix, Cuervo reflexion)
- **Cuervo Gap**: 1 (reflexion + self-correction context injection exists)
- **Priority**: 4
- **Complexity**: S
- **Notes**: Cuervo already has self-correction context injection (Phase 18 B.1), reflexion (Phase 8), confidence feedback loop (Phase 9). Further improvement: autorater pattern from production agents.

### F.4 User Interaction (ask question)
- **SOTA Coverage**: 3/6 (Claude Code AskUserQuestion, Replit "ask human", Devin clarification)
- **Cuervo Gap**: 2 (REPL has user input, no tool for agent to ask structured questions)
- **Priority**: 6
- **Complexity**: M
- **Notes**: Tool that pauses execution and asks the user a structured question with options. Claude Code: {question, options, multiSelect}. Cuervo's REPL can prompt the user, but the agent has no tool to request user input mid-execution.

---

## 8. Priority Matrix

### Tier 1: Critical (Priority >= 12)

| Tool | Domain | Priority | Complexity | Justification |
|------|--------|----------|------------|---------------|
| Git Status/Diff/Log | D.1 | 15 | M | 5/6 SOTA agents, foundational for version control awareness |
| Git Add/Commit | D.2 | 15 | M | 5/6 SOTA agents, enables autonomous code contribution |
| Web Search | E.2 | 12 | M | 4/6 SOTA agents, required for documentation/API lookup |
| Background Job Management | C.2 | 12 | M | 4/6 SOTA agents, enables async workflows |
| Semantic Code Search | B.5 | 12 | XL | 4/6 SOTA agents, transforms codebase navigation |

### Tier 2: Important (Priority 6-9)

| Tool | Domain | Priority | Complexity | Justification |
|------|--------|----------|------------|---------------|
| Git Branch/Checkout/Merge | D.3 | 9 | M | Full git workflow |
| Git Push/Pull | D.4 | 9 | S | Remote collaboration |
| Browser Automation | E.4 | 9 | XL | 3/6 SOTA, better via MCP |
| File Delete | A.4 | 6 | S | Quick win, safer than bash rm |
| Fuzzy File Search | B.4 | 6 | M | Agent navigation efficiency |
| Symbol Search | B.7 | 6 | L | Code intelligence |
| Package Management | C.3 | 6 | M | Safer than raw bash |
| PR/Issue Management | D.5 | 6 | L | Better via GitHub MCP |
| HTTP POST/PUT/DELETE | E.3 | 6 | S | Extend existing web_fetch |
| API Client (authenticated) | E.5 | 6 | M | Protected API access |
| Task Tracking Tool | F.1 | 6 | S | Agent self-management |
| User Question Tool | F.4 | 6 | M | Structured mid-execution queries |

### Tier 3: Nice-to-Have (Priority 1-5)

| Tool | Domain | Priority | Complexity | Justification |
|------|--------|----------|------------|---------------|
| Multi-File Edit | A.6 | 4 | M | Reduces round-trips |
| File Move/Rename | A.5 | 4 | S | bash fallback works |
| Plan Generation | F.2 | 4 | S | Already partially exists |
| Self-Correction | F.3 | 4 | S | Already partially exists |
| Notebook Edit | A.8 | 3 | M | Niche (data science) |
| Read-Before-Write | A.9 | 3 | S | Safety improvement |
| Runtime Environment | C.4 | 3 | L | Cloud-hosted feature |
| Repo Map Tool | B.6 | 2 | S | Already exists as context source |

---

## 9. Recommended Implementation Order

### Phase A: Git Integration (Tier 1)
1. `git_status` tool (ReadOnly) -- structured diff/untracked output
2. `git_diff` tool (ReadOnly) -- staged/unstaged changes with file list
3. `git_log` tool (ReadOnly) -- recent commits with metadata
4. `git_add` tool (ReadWrite) -- stage files
5. `git_commit` tool (Destructive) -- create commit with message

### Phase B: Search & Web (Tier 1)
6. `web_search` tool (ReadOnly) -- search engine integration
7. `background_start` / `background_output` / `background_kill` tools -- async job lifecycle

### Phase C: Quick Wins (Tier 2, small complexity)
8. `file_delete` tool (Destructive) -- safer than bash rm
9. `http_request` tool -- extend web_fetch with method/headers/body
10. `task_track` tool -- agent self-management (TodoWrite equivalent)

### Phase D: Code Intelligence (Tier 1-2, high complexity)
11. `symbol_search` tool -- extend repo_map with search capability
12. `fuzzy_find` tool -- partial path matching
13. Semantic search (if infrastructure permits)

### Phase E: Collaboration (Tier 2)
14. `git_branch` / `git_checkout` tools
15. `git_push` / `git_pull` tools
16. `ask_user` tool -- structured mid-execution questions

### Phase F: Polish
17. Read-before-write enforcement
18. Repo map as queryable tool
19. Multi-file edit
20. Notebook edit

---

## 10. Cross-Domain Infrastructure Requirements

| Requirement | Tools Affected | Notes |
|-------------|---------------|-------|
| Path security | All file tools | Already exists, extend to git tools |
| Permission levels | All new tools | ReadOnly for queries, ReadWrite for mutations, Destructive for git push/commit |
| Structured metadata | Git tools, search tools | JSON metadata alongside human-readable output |
| Token budgeting | Search tools, git diff | Large diffs/search results need truncation |
| Confirmation dialogs | Destructive tools | Git push, file delete, git reset |
| Process registry | Background job tools | HashMap<String, Child> + timeout management |
| Search provider | Web search | API key management, rate limiting |
