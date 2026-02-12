# Engineering Utility Layer: SOTA Research (2025-2026)

> Phase 1 deliverable for the Engineering Utility Layer design.
> Research date: February 2026

---

## 1. Executive Summary

This document captures the state-of-the-art in AI coding agent tooling as of February 2026, covering 6 production systems (Claude Code, OpenHands, Cursor, Replit Agent, Devin, Aider), the Cuervo CLI baseline, and cross-cutting infrastructure patterns (sandbox isolation, retry strategies, DevOps MCP servers, browser automation). The goal is to extract what tools are provided, how they are exposed to agents, what capabilities are table stakes in 2026, and what capabilities differentiate expert-level agents.

---

## 2. Cuervo CLI Baseline (Phase 18)

### 2.1 Tool Inventory (9 tools)

| Tool | Permission | Key Feature |
|------|-----------|-------------|
| `file_read` | ReadOnly | Line-based reading with offset/limit, numbered output |
| `file_write` | ReadWrite | Auto-creates parent dirs, syntax validation on write |
| `file_edit` | ReadWrite | Exact string replacement (not regex), uniqueness check |
| `file_inspect` | ReadOnly | Universal file intelligence (12 format handlers via cuervo-files) |
| `bash` | Destructive | Sandboxed (rlimits), timeout, output truncation (head 60% + tail 30%) |
| `glob` | ReadOnly | Glob patterns, sorted results, max 500 |
| `grep` | ReadOnly | Full regex, context lines, max 200 matches |
| `directory_tree` | ReadOnly | ASCII tree, depth limit 10, max 2000 entries |
| `web_fetch` | ReadOnly | HTTP GET, HTML stripping, max 1MB, 5 redirects |

### 2.2 Cross-Cutting Infrastructure

| Component | Coverage |
|-----------|----------|
| **Path Security** | Traversal prevention, blocked patterns, allowed dirs, pre-compiled patterns |
| **Sandbox** | CPU/file-size rlimits (Unix), output truncation, configurable limits |
| **Syntax Check** | C-family bracket balance, Python indentation, JSON/TOML/YAML validation |
| **MCP Bridge** | External tools bridged as first-class `Tool` trait implementors |
| **Tool Registry** | `HashMap<String, Arc<dyn Tool>>`, unified `ToolInput`/`ToolOutput` |

### 2.3 Architecture Strengths

1. **Unified Tool trait**: all tools share the same interface (name, description, permission_level, execute, input_schema, requires_confirmation)
2. **Feature-gated file intelligence**: 12 handlers, zero binary impact when disabled
3. **Token budgeting**: file_inspect respects budgets, truncates gracefully
4. **Memory efficiency**: streaming grep, iterator-based file_read
5. **MCP extensibility**: any external MCP server becomes a native tool

### 2.4 Gaps vs. SOTA

| Gap | Impact | SOTA Reference |
|-----|--------|---------------|
| No git integration | Cannot query repo state, make commits, diff branches | Claude Code, Aider, Devin |
| No code intelligence (AST/symbols) | Cannot navigate large codebases efficiently | Cursor (semantic RAG), Aider (tree-sitter repo map) |
| No semantic search | Only regex + glob, no fuzzy/meaning-based | Cursor (codebase_search), Devin (Devin Search) |
| No database tools | Cannot inspect schemas or run read-only SQL | DevOps MCP pattern |
| No container execution | bash runs on host, not isolated | OpenHands (Docker), Devin (cloud VM) |
| No auth helpers | web_fetch has no auth headers, cookies, OAuth | General gap across all agents |
| No async job tracking | Cannot spawn/cancel/monitor background tasks | Claude Code (BashOutput, KillShell) |
| No browser automation | Cannot interact with web UIs | OpenHands (BrowserGym), Devin (Chrome), Claude (Computer Use) |
| No notebook support | Cannot edit Jupyter cells | Claude Code (NotebookEdit) |
| No web search | Only web_fetch (GET), no search engine | Claude Code (WebSearch) |

---

## 3. Claude Code (Anthropic) - v2.1.37

### 3.1 Tool Inventory (18 built-in + MCP dynamic)

| Tool | Category | Auto-Approved |
|------|----------|---------------|
| Read | File I/O (read) | Yes |
| Write | File I/O (modify) | No |
| Edit | File I/O (modify) | No |
| Bash | Shell execution | No |
| BashOutput | Shell output retrieval | Yes |
| KillShell | Shell management | Yes |
| Glob | File search | Yes |
| Grep | Content search (ripgrep) | Yes |
| WebFetch | Network (URL fetch + AI processing) | No |
| WebSearch | Network (web search) | Yes |
| Task | Subagent spawning | Yes |
| NotebookEdit | Jupyter notebook modification | No |
| TodoWrite | Task tracking | Yes |
| AskUserQuestion | User interaction | Yes |
| Skill | Skill invocation | Yes |
| SlashCommand | Custom command execution | No |
| EnterPlanMode | Mode switch | Yes |
| ExitPlanMode | Mode switch | Yes |

MCP tools appear dynamically as `mcp__<server>__<tool>`. Additional tools: ListMcpResources, ReadMcpResource.

### 3.2 Key Architectural Patterns

**Read-before-write constraint**: Edit and Write tools enforce that the file must have been read via Read at least once in the conversation. Prevents blind overwrites.

**Edit tool**: exact string replacement (same as Cuervo). `old_string` must be unique unless `replace_all: true`. Uses `// ... existing code ...` markers conceptually but the actual tool is search-and-replace.

**Grep**: built on ripgrep. Three output modes (files_with_matches, content, count). Supports multiline matching, file type filtering, pagination (head_limit + offset).

**WebFetch**: not just HTTP GET -- processes fetched content with a small/fast AI model using the provided prompt. 15-minute cache. Converts HTML to markdown.

**WebSearch**: domain filtering (allowed/blocked), returns search result blocks with markdown hyperlinks.

**Background execution**: `Bash(run_in_background: true)` + `BashOutput(task_id)` + `KillShell(task_id)` for async job lifecycle.

### 3.3 Permission Model (5 modes)

| Mode | Behavior |
|------|----------|
| `default` | Prompts for modifications and bash |
| `acceptEdits` | Auto-approves file Write/Edit; bash still prompts |
| `plan` | Read-only; no tool execution |
| `dontAsk` | Auto-denies all; use with explicit allow rules |
| `bypassPermissions` | Auto-approves everything (YOLO mode) |

Rules configured in `.claude/settings.json` with deny/ask/allow tiers. Deny always wins. Rule format: `Tool(glob_specifier)`.

Processing pipeline: `PreToolUse Hook -> Deny Rules -> Allow Rules -> Ask Rules -> Permission Mode Check -> canUseTool Callback -> PostToolUse Hook`.

### 3.4 Sandboxing (OS-native)

| Platform | Technology |
|----------|-----------|
| macOS | Seatbelt (`sandbox-exec`) |
| Linux | Bubblewrap (`bwrap`) |

- **Filesystem**: read access everywhere (deny-specific), write access denied everywhere except cwd (allow-specific)
- **Network**: all network denied by default; routed through proxy server outside sandbox
- **Escape hatch**: `dangerouslyDisableSandbox` param on Bash tool
- **Impact**: 84% reduction in permission prompts (Anthropic internal data)
- **Open source**: `@anthropic-ai/sandbox-runtime` on npm

### 3.5 Subagent Architecture

| Agent Type | Tools Available | Purpose |
|------------|----------------|---------|
| general-purpose | ALL | Complex multi-step tasks |
| Explore | Glob, Grep, Read, Bash | Fast read-only codebase search |
| Plan | Read-only tools | Architecture planning |
| claude-code-guide | Read-only | Documentation lookup |
| statusline-setup | Read, Edit | Status line configuration |

Key properties:
- Each subagent gets its **own 200K context window**
- Up to **10 concurrent tasks** with intelligent queuing
- **Single-level nesting only** (subagents cannot spawn subagents)
- Custom agents defined as `.claude/agents/*.md` files with YAML frontmatter

**Agent Teams (experimental, Feb 2026)**: multi-agent coordination with peer-to-peer messaging via `SendMessage`, shared task list with file-locking for race conditions. One team per session, no nested teams. Token cost: 5-person team = 5x consumption.

### 3.6 Context Management

- **CLAUDE.md hierarchy**: parent dirs (global) -> cwd (project) -> child dirs (on-demand) -> `.local.md` (private)
- **Auto memory**: `~/.claude/projects/<project>/memory/`, MEMORY.md first 200 lines loaded into system prompt
- **Context compression**: auto at ~95% capacity, manual at `/compact`. Context editing clears stale tool calls. 84% token reduction in 100-turn evaluations
- **Context buffer**: ~33K tokens reserved (16.5% of 200K), usable ~167K tokens

### 3.7 Hook System (14 events)

| Event | Can Block? | Key Use |
|-------|-----------|---------|
| PreToolUse | Yes (allow/deny/ask) | Input modification, policy enforcement |
| PostToolUse | No | Feedback, logging |
| UserPromptSubmit | Yes | Prompt validation |
| Stop | Yes | Output validation |
| SubagentStart | No | Context injection |

Three hook types: command (shell), prompt (single-turn LLM), agent (multi-turn with tools).

### 3.8 MCP Integration

- **Transports**: stdio (local), SSE (remote), streamable-HTTP (remote)
- **Scopes**: user (`~/.claude.json`), project (`.mcp.json`)
- **Tool search**: auto-enabled when MCP descriptions exceed 10% context window; tools loaded on-demand
- **Token limits**: warning at 10K tokens per output, max 25K (overridable via `MAX_MCP_OUTPUT_TOKENS`)

---

## 4. OpenHands (formerly OpenDevin)

### 4.1 Action/Observation Architecture

OpenHands uses an **event-stream architecture** rather than function-calling:

**Actions (~15 types):**

| Action | Purpose |
|--------|---------|
| CmdRunAction | Bash commands in Docker sandbox |
| IPythonRunCellAction | Python code in Jupyter kernel |
| FileReadAction | Read file contents |
| FileWriteAction | Write new content |
| FileEditAction | Edit existing file (V1) |
| BrowseURLAction | Navigate to URL |
| BrowseInteractiveAction | BrowserGym DSL for interactive browsing |
| AgentDelegateAction | Delegate subtask to another agent |
| AgentThinkAction | Internal reasoning scratchpad |
| AgentFinishAction | Signal completion |
| AgentRejectAction | Signal rejection |
| AddTaskAction / ModifyTaskAction | Plan management |
| MCPAction | MCP tool invocation |

**Observations**: CmdOutputObservation, FileReadObservation, ErrorObservation, NullObservation, AgentThinkObservation, TaskTrackingObservation, UserRejectObservation.

### 4.2 Sandbox Model

- Each session spins up an **isolated Docker container** with full Linux OS, bash, IPython, Chromium
- Runtime communicates via **REST API server inside the container**
- V1 SDK: `Workspace` abstraction (LocalWorkspace, RemoteWorkspace, DockerWorkspace)
- Container torn down post-session

### 4.3 Browser Automation

- **BrowserGym** (ServiceNow) + **Playwright + Chromium**
- Declarative action primitives: navigation, clicking, typing, scrolling, DOM manipulation
- Observations: HTML, DOM, **accessibility tree (AXTree)**, screenshots, open tabs
- Each DOM element gets unique `bid` (BrowserGym ID) for precise targeting

### 4.4 Permission Model (V1 SDK)

- Actions assigned risk level (LOW/MEDIUM/HIGH/UNKNOWN) by LLM policy or programmatic rules
- Actions above user threshold require explicit confirmation

### 4.5 Multi-Agent

- CodeAct paradigm: LLM generates executable code as its "action"
- AgentDelegateAction for coding->browser agent delegation
- Event-sourced state with deterministic replay

---

## 5. Cursor (Anysphere)

### 5.1 Tool Inventory (~9 tools)

| Tool | Purpose |
|------|---------|
| codebase_search | **Semantic search** (vector embeddings/RAG) -- finds code by meaning |
| grep_search | Regex-based exact pattern matching |
| file_search | Fuzzy file name search (partial path) |
| read_file | Read file (up to 250 lines; 750 in max mode) |
| edit_file | Minimal context edits with `// ... existing code ...` markers |
| list_dir | Directory listing |
| delete_file | File removal with safety checks |
| run_terminal_command | Shell with `is_background`, `required_permissions` |
| search_replace / MultiEdit | Atomic multi-edit within single file |

### 5.2 Differentiating Features

**Codebase indexing**: automatic on project open. Hybrid RAG with vector embeddings. Enables `@Codebase` natural language queries across entire project.

**Apply model**: proprietary Cursor 2.0 Composer -- mixture-of-experts trained via RL inside real codebases. 4x faster than comparably intelligent models.

**Subagents (v2.4)**: independent agents for discrete subtasks, parallel execution with own context/tools.

**Sandboxed terminals (GA)**: macOS/Linux seatbelt. Read/write scoped to workspace + `/tmp/`. Network blocked by default. Non-allowlist commands sandboxed.

**Parallel tool calling**: multiple tools invoked simultaneously in one turn.

---

## 6. Replit Agent

### 6.1 Multi-Agent Architecture

| Agent | Role |
|-------|------|
| Manager | Orchestrates overall workflow |
| Editor | Code modifications and file operations |
| Verifier | Tests app via screenshots, runs static checks, validates progress |

**Key design decision**: Replit does NOT use standard function-calling APIs. The LLM generates code to invoke tools, which proved more reliable at scale.

### 6.2 Tool Inventory (~6 tools)

| Tool | Purpose |
|------|---------|
| search_filesystem | Code search across project |
| packager_tool | Package management (npm, pip, etc.) |
| programming_language_install_tool | Runtime environment setup |
| bash | Shell execution |
| suggest_deploy | Deployment suggestion (requires user confirmation) |
| restart_workflow | Workflow management |

### 6.3 Differentiating Features

**Self-healing loop (Agent 3)**: AI tests apps it builds in a live browser, takes screenshots, validates UI, fixes issues autonomously.

**Auto-commit**: every major step creates a git commit, enabling time-travel/rollback.

**Deployment pipeline**: one-click deploy, database provisioning, auth, domain purchasing, payments (Stripe), 30+ connectors.

**Reflection**: every 5 steps, agent reflects on progress, can roll back + retry with randomized exploration.

**Memory**: long trajectories compressed with LLMs to retain relevant information only.

**200-minute autonomous runs** (Agent 3), ~90% tool invocation success rate at production scale.

---

## 7. Devin (Cognition Labs)

### 7.1 Core Tools (4 simultaneous interfaces)

| Tool | Description |
|------|-------------|
| Shell | Full bash terminal for env setup, git, tests, builds |
| Editor | VS Code-like IDE with full capabilities |
| Browser | Built-in Chromium for docs, testing web apps, interactive browsing |
| Planner | Breaks tasks into sequential steps before coding |

Additional features: **Devin Search** (agentic codebase exploration with cited code answers), **Devin Wiki** (auto-generated repo documentation updated hourly), **Devin API** (programmatic session creation for CI/CD).

### 7.2 Differentiating Features

**Cloud VM per session**: fresh sandboxed VM with Shell + Editor + Browser running simultaneously.

**Persistent memory**: maintains running to-do list across hours/days. Vectorized codebase snapshots + full replay timeline.

**Self-correction loop**: test-debug-fix cycle. Runs tests, explores console logs, adds debugging statements, fixes, re-runs until passing.

**Devin 2.0**: parallel instances in separate VMs, interactive planning, 67% PR merge rate (up from 34%), $73M ARR.

---

## 8. Aider

### 8.1 Edit Format System

| Format | Mechanism | Best For |
|--------|-----------|----------|
| whole | Returns entire updated file | Small files, weak models |
| diff (search/replace) | `<<<<<<< SEARCH ... >>>>>>> REPLACE` | Most models (default for GPT-4o) |
| diff-fenced | Variant with path inside fence | Gemini models |
| udiff | Standard unified diff format | GPT-4 Turbo (reduces "lazy coding") |
| editor | Simplified for architect/editor pipeline | Two-model workflows |

The search/replace format was adopted by Cline and RooCode. OpenAI's patch format also supported.

### 8.2 Repo Map (tree-sitter + PageRank)

The pipeline:
1. **tree-sitter parsing**: language-specific `tags.scm` query files, dozens of languages
2. **Symbol extraction**: definitions (classes, functions, methods) + cross-file references
3. **Graph construction**: files = nodes, cross-file references = directed edges
4. **PageRank ranking**: identifies most important symbols relative to current chat context
5. **Token optimization**: binary search trims map to fit `--map-tokens` budget (default 1K)
6. **Context injection**: optimized map sent alongside chat messages

### 8.3 Key Architectural Decisions

**No built-in sandbox**: operates directly on user's filesystem. Safety via **git integration** -- auto-commits after each edit, enabling `git diff` / `git revert`.

**tree-sitter linting**: AST-aware error detection + auto-fix after every edit.

**Explicit file control**: user adds files to chat with `/add`. Only those files can be modified. Repo map provides read-only context from broader codebase.

**Streaming**: LiteLLM backend (any provider), Rich library for live markdown rendering with syntax highlighting.

**Architect/editor mode**: two-model pipeline. Architect model reasons about changes, editor model produces syntactically correct diffs.

---

## 9. DevOps & Infrastructure MCP Servers

### 9.1 Kubernetes MCP Server

- **Language**: Go
- **Key safety feature**: `--disable-destructive` flag blocks delete/scale/rollout operations
- **Read-only mode**: list pods, describe deployments, get logs -- no mutations
- **RBAC**: respects Kubernetes RBAC; agent operates under configured service account

### 9.2 Terraform MCP Server (HashiCorp Official)

| Tool | Purpose |
|------|---------|
| resolveProviderDocID | Find provider documentation |
| getProviderDocs | Fetch complete provider docs (markdown) |
| searchModules | Search Terraform Registry for modules |
| create_run | Plan-and-apply or refresh-state operations |

**Safety**: destructive operations disabled by default. Require `ENABLE_TF_OPERATIONS=true`.

**Dual transport**: stdio + streamable-HTTP. Docker image available.

### 9.3 Claude Computer Use (Beta)

**Three tools**: Computer (mouse/keyboard), Text Editor (file ops), Bash (system commands).

**Mechanism**: screenshot capture -> pixel coordinate mapping -> action execution -> new screenshot -> loop.

**Limitations**: beta quality, coordinate accuracy issues, high latency, resolution sensitivity.

**Safety**: automatic prompt injection classifiers on screenshots, recommend running in VMs/containers.

---

## 10. Cross-Cutting Patterns

### 10.1 Sandbox Isolation (2026 Landscape)

| Approach | Used By | Cold Start | Isolation Level |
|----------|---------|-----------|----------------|
| Seatbelt (macOS) | Claude Code, Cursor | <10ms | Process-level |
| Bubblewrap (Linux) | Claude Code | <10ms | Namespace-level |
| Docker container | OpenHands | ~500ms | Container-level |
| Cloud VM | Devin | ~2s | VM-level |
| Firecracker microVM | K8s Agent Sandbox | 100-125ms | Hardware-level |
| rlimits | Cuervo CLI | 0ms | Resource limits only |
| None (git safety) | Aider | 0ms | None |

**Kubernetes Agent Sandbox** (Google, 2025): formal subproject of K8s SIG Apps. Core APIs: Sandbox, SandboxTemplate, SandboxClaim. Standardizing secure agent execution on K8s.

**Zero-trust model**: all LLM-generated code treated as potentially malicious. MicroVM isolation preferred for production.

### 10.2 Retry & Self-Correction Patterns

| Pattern | Description | Used By |
|---------|------------|---------|
| Selective retry | Only retry transient errors (timeout, 429, connection reset) | Cuervo CLI (ToolRetryConfig) |
| Autorater self-correction | LLM-as-judge assesses each output, provides feedback for retry | Google, production agents |
| Sandbox escalation | Decision tree for safe execution + automatic escalation | Anthropic Codex patterns |
| Git-based rollback | Auto-commit + revert on failure | Aider, Replit |
| Reflection loops | Periodic self-assessment (every N steps) | Replit (5 steps), Cuervo (reflexion) |

**Anti-pattern**: UiPath advises against agent-level retries because output isn't deterministic. Instead, capture errors and handle within the tool.

### 10.3 Context Management Patterns

| Pattern | Description | Used By |
|---------|------------|---------|
| Instruction file hierarchy | Parent -> project -> subdirectory -> private | Claude Code (CLAUDE.md) |
| Auto memory persistence | Cross-session knowledge stored to disk | Claude Code, Cuervo (L4 archive) |
| Context compression | LLM-summarized compaction at capacity threshold | Claude Code (95%), Replit |
| Repo map | Structural codebase overview within token budget | Aider (tree-sitter), Cuervo (repo_map) |
| Semantic search | Vector embeddings for meaning-based code retrieval | Cursor (codebase_search) |
| Event-sourced state | Deterministic replay from action/observation stream | OpenHands, Cuervo (trace replay) |

---

## 11. Table Stakes vs. Differentiators (2026)

### 11.1 Table Stakes (every serious agent has these)

| Capability | Evidence |
|------------|----------|
| File read/write/edit | All 6 agents |
| Shell execution with timeout | All 6 agents |
| File search (glob/pattern) | All 6 agents |
| Content search (regex/grep) | All 6 agents (5/6 ripgrep-based) |
| Directory listing/tree | All 6 agents |
| Permission tiers (read-only / read-write / destructive) | All 6 agents |
| Streaming responses | All 6 agents |
| Output truncation / token budgeting | All 6 agents |
| Multi-step planning | All 6 agents |
| Auto-commit / checkpoint | 5/6 agents (not OpenHands natively) |

### 11.2 Near-Table-Stakes (4+ agents, Cuervo missing)

| Capability | Count | Cuervo Status |
|------------|-------|---------------|
| Web search (engine, not just fetch) | 4/6 | Missing |
| Git integration (status, diff, commit) | 5/6 | Missing |
| Background/async job management | 4/6 | Missing |
| Semantic/meaning-based code search | 4/6 | Missing |
| Sandbox isolation (OS-level, not just rlimits) | 4/6 | Partial (rlimits only) |

### 11.3 Differentiators (1-3 agents, competitive advantage)

| Capability | Who | Value |
|------------|-----|-------|
| Subagent spawning (parallel, isolated context) | Claude Code, Cursor, Replit | Distributes complex tasks across multiple contexts |
| Browser automation | OpenHands, Devin, Claude (Computer Use) | Web app testing, documentation browsing |
| Notebook editing | Claude Code | Data science workflows |
| Codebase semantic indexing (RAG) | Cursor | Natural language codebase queries |
| Tree-sitter repo map + PageRank | Aider | Structural awareness without embeddings |
| Self-healing UI testing | Replit | Autonomous QA loop |
| Agentic codebase search | Devin | Cited code answers from natural language |
| Infrastructure-as-Code tools (MCP) | Claude Code (via MCP) | Terraform/K8s integration |
| Hook system (14 lifecycle events) | Claude Code | Deterministic policy enforcement |
| Agent teams (peer-to-peer coordination) | Claude Code (experimental) | Multi-agent collaboration |
| Edit format diversity | Aider | Model-specific diff strategies |

---

## 12. Comparative Tool Matrix

| Dimension | Cuervo | Claude Code | OpenHands | Cursor | Replit | Devin | Aider |
|-----------|--------|-------------|-----------|--------|--------|-------|-------|
| **Tool count** | 9 | 18+ | ~15 | ~9 | ~6 | 4+3 | 5 formats |
| **Sandbox** | rlimits | OS-native (Seatbelt/bwrap) | Docker | Seatbelt | Cloud VM | Cloud VM | None (git) |
| **Browser** | None | None (MCP possible) | BrowserGym+Playwright | None | Live preview | Chrome | None |
| **Code search** | regex only | ripgrep | grep/find | Semantic RAG + regex | search_filesystem | Devin Search | tree-sitter repo map |
| **Git** | None | Via bash | Via bash | @Git context | Auto-commit | Full shell | Native integration |
| **Multi-agent** | Orchestrator | Task tool (10 concurrent) | AgentDelegate | Subagents (8 parallel) | Manager+Editor+Verifier | Parallel VMs | Single only |
| **Planning** | LlmPlanner | EnterPlanMode | CodeAct | Plan mode | Manager agent + reflection | Planner tool | Architect/editor |
| **MCP** | Native bridge | Native (stdio/SSE/HTTP) | V1 SDK | CLI config | Not documented | Not documented | Not documented |
| **Permission** | 3 levels | 5 modes + hooks | Risk levels | Sandbox + allowlist | "ask human" tool | VM isolation | User adds files |
| **Streaming** | SSE | SSE + fine-grained tool | WebSocket events | Token-by-token | UI updates | Timelapse replay | LiteLLM + Rich |
| **Memory** | L0-L4 + episodic | CLAUDE.md + auto memory | Event-sourced | Session-based | LLM compression | Vectorized snapshots | Git history |
| **Language** | Rust | TypeScript | Python | TypeScript | Internal | Proprietary | Python |

---

## 13. Key Insights for Cuervo Engineering Utility Layer

### 13.1 Highest-ROI Additions (close SOTA gaps)

1. **Git integration tools** -- every SOTA agent has this. Status, diff, log, add, commit, branch operations. Cuervo currently relies on bash fallback.

2. **Web search tool** -- 4/6 SOTA agents have dedicated search. Claude Code's WebSearch with domain filtering is the gold standard. Cuervo has web_fetch but no search engine integration.

3. **Background job management** -- Claude Code pattern: Bash(run_in_background) + BashOutput(task_id) + KillShell(task_id). Three tools for async lifecycle. Cuervo has nothing.

4. **Code intelligence / repo map** -- Aider's tree-sitter + PageRank approach is the most practical (no embedding infrastructure needed). Cuervo has repo_map module (Phase 8B, 32 tests) but it's only used as a context source, not as a queryable tool.

### 13.2 Architectural Patterns to Adopt

1. **Read-before-write enforcement** (Claude Code) -- prevent blind overwrites. Simple conversation state tracking.

2. **Permission rule DSL** (Claude Code) -- `Tool(glob)` pattern for deny/ask/allow rules. More expressive than Cuervo's current 3-level system.

3. **Parallel tool calling** (Cursor, Claude Code) -- already supported by Cuervo's agent loop, but not explicitly optimized.

4. **Input modification hooks** (Claude Code PreToolUse) -- hooks can modify tool inputs before execution. Cuervo has event bus but no pre-execution interception.

### 13.3 Patterns to Avoid

1. **Docker-per-session** (OpenHands) -- 500ms cold start per session is too slow for interactive CLI. Stick with rlimits/Seatbelt.

2. **No sandbox at all** (Aider) -- git-only safety net is insufficient for production. Cuervo's rlimits are minimum viable.

3. **Code-as-action** (OpenHands, Replit) -- generating executable code instead of function calls is fragile for Rust-based systems. Keep Tool trait.

4. **200-minute autonomous runs** (Replit) -- appropriate for cloud-hosted, not for local CLI. Keep human-in-the-loop by default.

### 13.4 Future-Proofing Considerations

1. **MCP ecosystem growth**: Terraform, Kubernetes, GitHub, Postgres MCP servers exist. Cuervo's MCP bridge is a competitive advantage -- invest in MCP-first tool integration.

2. **Agent teams**: Claude Code's experimental agent teams show the direction. Cuervo's orchestrator is already positioned for this.

3. **Tool search**: when tool count exceeds ~20, context overhead becomes significant. Claude Code's on-demand tool loading (when MCP tools > 10% context) is the right approach.

4. **Notebook support**: growing demand for data science workflows. NotebookEdit is table stakes for full-stack agents.

---

## 14. Sources

### Agent Documentation
- [Claude Code Overview](https://code.claude.com/docs/en/overview)
- [Claude Code Permissions](https://code.claude.com/docs/en/permissions)
- [Claude Code Sandboxing](https://code.claude.com/docs/en/sandboxing)
- [Claude Code Sub-agents](https://code.claude.com/docs/en/sub-agents)
- [Claude Code Agent Teams](https://code.claude.com/docs/en/agent-teams)
- [Claude Code Hooks](https://code.claude.com/docs/en/hooks)
- [Claude Code MCP](https://code.claude.com/docs/en/mcp)
- [Claude Code Memory](https://code.claude.com/docs/en/memory)
- [Anthropic Engineering: Sandboxing](https://www.anthropic.com/engineering/claude-code-sandboxing)
- [Anthropic: Managing Context](https://www.anthropic.com/news/context-management)

### OpenHands
- [OpenHands ICLR 2025 Paper](https://arxiv.org/html/2407.16741v3)
- [OpenHands V1 SDK Paper](https://arxiv.org/html/2511.03690v1)
- [OpenHands GitHub](https://github.com/OpenHands/OpenHands)

### Cursor
- [Cursor Agent Tools Docs](https://docs.cursor.com/agent/tools)
- [How Cursor Shipped its Coding Agent (ByteByteGo)](https://blog.bytebytego.com/p/how-cursor-shipped-its-coding-agent)

### Replit
- [Replit Agent Architecture (LangChain)](https://www.langchain.com/breakoutagents/replit)
- [Replit Agent 3](https://blog.replit.com/introducing-agent-3-our-most-autonomous-agent-yet)

### Devin
- [Devin 2.0](https://cognition.ai/blog/devin-2)
- [Devin 2025 Performance Review](https://cognition.ai/blog/devin-annual-performance-review-2025)

### Aider
- [Aider Edit Formats](https://aider.chat/docs/more/edit-formats.html)
- [Aider Repo Map](https://aider.chat/docs/repomap.html)
- [Building a Better Repo Map with Tree-sitter](https://aider.chat/2023/10/22/repomap.html)

### DevOps & Infrastructure
- [Terraform MCP Server](https://github.com/hashicorp/terraform-mcp-server)
- [Kubernetes Agent Sandbox (Google)](https://opensource.googleblog.com/2025/11/unleashing-autonomous-ai-agents-why-kubernetes-needs-a-new-standard-for-agent-execution.html)
- [Claude Computer Use Tool](https://platform.claude.com/docs/en/agents-and-tools/tool-use/computer-use-tool)
- [Sandbox Runtime (GitHub)](https://github.com/anthropic-experimental/sandbox-runtime)
- [AI Sandbox Security (Blaxel)](https://blaxel.ai/blog/ai-sandbox)
- [Production AI Agent Best Practices (n8n)](https://blog.n8n.io/best-practices-for-deploying-ai-agents-in-production/)

### System Prompts & Internals
- [Piebald-AI Claude Code System Prompts](https://github.com/Piebald-AI/claude-code-system-prompts)
- [Internal Tools Implementation (Gist)](https://gist.github.com/bgauryy/0cdb9aa337d01ae5bd0c803943aa36bd)
- [Context Buffer Analysis](https://claudefa.st/blog/guide/mechanics/context-buffer-management)
