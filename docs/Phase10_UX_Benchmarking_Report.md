# Cuervo CLI: Competitive UX Benchmarking Report

**Date**: February 7, 2026
**Scope**: 11 products across 3 categories, 9 UX dimensions, 6 academic/design frameworks
**Purpose**: Identify best-in-class patterns, score competitors, and generate actionable UX improvements for Cuervo CLI

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Methodology](#2-methodology)
3. [Product Benchmarks: AI Agent CLIs](#3-product-benchmarks-ai-agent-clis)
4. [Product Benchmarks: Developer CLI Tools](#4-product-benchmarks-developer-cli-tools)
5. [Product Benchmarks: AI/Conversational Products](#5-product-benchmarks-aiconversational-products)
6. [UX Dimension Cross-Analysis](#6-ux-dimension-cross-analysis)
7. [Academic & Framework Research](#7-academic--framework-research)
8. [Cuervo CLI Current State Assessment](#8-cuervo-cli-current-state-assessment)
9. [Competitive Scorecard](#9-competitive-scorecard)
10. [Actionable Recommendations for Cuervo CLI](#10-actionable-recommendations-for-cuervo-cli)
11. [Sources](#11-sources)

---

## 1. Executive Summary

The AI-powered CLI tool market in 2025-2026 has coalesced around several clear UX patterns. Claude Code dominates the category, generating $1B+ annualized revenue by November 2025 and serving as the template for competitors including GitHub Copilot CLI, Codex CLI, and others. The key differentiators are not features but **trust signals**, **progressive disclosure**, and **streaming feedback quality**.

Cuervo CLI has strong technical foundations (685 tests, 9-crate workspace, multi-provider support, resilience layer, episodic memory) but its UX layer remains utilitarian. This report identifies 47 specific improvements across 9 dimensions, prioritized by impact and implementation effort.

### Key Findings

1. **Onboarding gap is critical**: The top 3 AI CLIs all invest heavily in guided first-run experiences. Cuervo's `init` command is 40 lines of boilerplate creation.
2. **Trust/transparency is the #1 differentiator**: Developer trust in AI tool accuracy has fallen to 29% (2025 Stack Overflow Survey). Products that show their reasoning win.
3. **Streaming UX is table stakes**: All competitive products now use real-time streaming with progressive rendering. Cuervo's StreamRenderer handles this adequately but lacks progress state indicators.
4. **Permission UX determines adoption**: Both Claude Code and Codex CLI have evolved sophisticated, multi-tier permission models with session memory. Cuervo's is functional but lacks the "approve for session" batch pattern.
5. **Color/accessibility is universally poor**: Only Warp Terminal makes meaningful accessibility investment. This is an opportunity for differentiation.

---

## 2. Methodology

### Scoring Criteria (1-10 scale)

| Score | Meaning |
|-------|---------|
| 1-2 | Absent or broken |
| 3-4 | Basic/minimal implementation |
| 5-6 | Functional, meets expectations |
| 7-8 | Well-designed, above average |
| 9-10 | Best-in-class, industry-leading |

### UX Dimensions Evaluated

1. **Onboarding** -- First-run experience, setup, authentication
2. **Command Structure** -- Hierarchy, naming, discoverability
3. **Output Formatting** -- Results presentation, color, structure
4. **Error Handling** -- Error display, actionability, recovery suggestions
5. **Progress/Feedback** -- Spinners, progress bars, streaming indicators
6. **Help System** -- Inline help, contextual hints, documentation
7. **Configuration** -- Settings management, defaults, overrides
8. **Accessibility** -- Screen reader support, color-blind modes, contrast
9. **Trust/Transparency** -- AI decision explanation, confidence indicators

---

## 3. Product Benchmarks: AI Agent CLIs

### 3.1 Claude Code (Anthropic)

**Category**: Direct competitor (market leader)
**Revenue**: $1B+ annualized (Nov 2025)
**Platform**: Node.js (TypeScript)

| Dimension | Score | Analysis |
|-----------|-------|----------|
| Onboarding | 9 | CLAUDE.md auto-injection per directory. First run detects project type, suggests config. `claude` with no args drops into REPL immediately. OAuth browser flow for auth. |
| Command Structure | 9 | Slash commands (`/help`, `/compact`, `/doctor`, `/context`, `/model`, `/review`) merged with skills system. Extensible via `.claude/commands/` directories. |
| Output Formatting | 8 | Markdown rendering with syntax highlighting. Code diffs with intra-line highlighting. Tool use shown inline with `[tool: name]` markers. Status line customizable. |
| Error Handling | 8 | Permission prompt UX refined (Tab hint in footer, Y/N labels). Config errors with suggestions. MCP server failures shown as warnings, non-fatal. |
| Progress/Feedback | 8 | "Reading..." -> "Read" progress states. Spinner during first token wait. Tool execution progress shown in real-time. Customizable status line. |
| Help System | 9 | `/help` shows all commands including custom skills. SKILL.md frontmatter with `when:` clauses for auto-suggestion. `/doctor` for installation health. |
| Configuration | 8 | CLAUDE.md (project) + `~/.claude/` (global). Layered config. Environment variables. Provider-specific settings. |
| Accessibility | 4 | No explicit accessibility features documented. Relies on terminal emulator accessibility. No color-blind mode. |
| Trust/Transparency | 7 | Shows tool invocations before execution. Permission prompts with command preview. Cost tracking per session. No confidence scores. |

**Key UX Patterns to Adopt**:
- CLAUDE.md-equivalent auto-injection (`CUERVO.md`)
- Skills/commands extensibility via directory convention
- Customizable status line (git branch, cost, model)
- `/compact` for context window management
- `/doctor` with unreachable permission rule detection

### 3.2 GitHub Copilot CLI

**Category**: Direct competitor (enterprise-backed)
**Release**: Public preview Sep 2025, active development through Jan 2026
**Platform**: Go

| Dimension | Score | Analysis |
|-----------|-------|----------|
| Onboarding | 8 | `gh extension install github/copilot-cli` leverages existing GitHub CLI auth. OAuth flow via browser. Automatic context from git repo. |
| Command Structure | 8 | Slash commands: `/model`, `/cwd`, `/add-dir`, `/review`, `/context`, `/permissions`, `/experimental`. Plan mode via Shift+Tab toggle. Cloud handoff with `&` prefix. |
| Output Formatting | 9 | Intra-line syntax highlighting in diffs. Git pager integration. Token usage breakdown via `/context`. Better heredoc handling in shell commands. |
| Error Handling | 7 | Nonzero exit codes on LLM backend failures (auth, quota, network). Better error messages for unsupported models. Unsupported model message via `/model`. |
| Progress/Feedback | 7 | Plan mode shows analysis steps, clarifying questions, structured plan. Session approval batching ("approve for session" auto-approves parallel requests). |
| Help System | 7 | `/experimental` shows experimental features help. Configuration system with precedence rules. Custom agents via `~/.copilot/agents`. |
| Configuration | 8 | `~/.copilot/config` with precedence rules. Custom agents in `~/.copilot/agents` or `.github/agents`. MCP server configs. Model selection per-session. |
| Accessibility | 3 | No documented accessibility features. Standard terminal output. |
| Trust/Transparency | 8 | Plan mode shows reasoning before implementation. `/context` shows detailed token usage. Permission tiers (auto-approve for session). `/review` for code change analysis. |

**Key UX Patterns to Adopt**:
- Plan mode (Shift+Tab toggle) -- analyze before implementing
- `&` prefix for async/background delegation
- "Approve for session" batch permission pattern
- `/context` showing token usage breakdown
- `/review` for staged/unstaged change analysis

### 3.3 Aider

**Category**: Direct competitor (open-source)
**Platform**: Python
**Stars**: 25k+ GitHub

| Dimension | Score | Analysis |
|-----------|-------|----------|
| Onboarding | 7 | `pip install aider-chat && aider` immediate start. Auto-detects git repo. Model onboarding wizard for free vs paid tiers. `--auto-commits` flag for git integration. |
| Command Structure | 6 | `/add`, `/drop`, `/run`, `/settings`, `/undo`, `/diff`. File-centric: add files to chat context explicitly. Less discoverable than slash-command heavy competitors. |
| Output Formatting | 7 | Color-coded output: errors (#FF2222), warnings (#FFA500), assistant (#0088ff). Customizable via env vars. Pretty/plain modes. Dark/light terminal support. |
| Error Handling | 6 | "Failed to apply edit" messages when LLM disobeys format. Edit format validation. AutoCompleter requires 3 chars to reduce noise. Basic error recovery. |
| Progress/Feedback | 5 | Basic streaming output. No explicit progress indicators for multi-file operations. Relies on terminal scrolling for feedback. |
| Help System | 6 | `/help` command. Extensive online docs (aider.chat). `/settings` shows active model metadata. Shell tab completion for file paths and options. |
| Configuration | 8 | YAML config file (`.aider.conf.yml`). `.env` file support. Environment variables for all settings. Model-specific advanced settings. Editor configuration. |
| Accessibility | 4 | Customizable colors (AIDER_TOOL_ERROR_COLOR etc.). Dark/light mode. No screen reader specific features. |
| Trust/Transparency | 5 | Shows diffs before applying. `/undo` for reversal. Git commit history for traceability. No confidence scores or reasoning display. |

**Key UX Patterns to Adopt**:
- Color customization via environment variables
- Dark/light mode auto-detection
- `/undo` for reversal of AI actions
- File-explicit context management (`/add`, `/drop`)
- Shell tab completion for arguments and options

### 3.4 Cursor (Hybrid CLI/GUI)

**Category**: Hybrid competitor
**Platform**: Electron + Rust CLI (beta, Aug 2025)
**Revenue**: $100M+ ARR

| Dimension | Score | Analysis |
|-----------|-------|----------|
| Onboarding | 8 | IDE-native onboarding. CLI via `cursor` command. Agent modes (Plan, Ask, Edit). Layout system (agent, editor, zen, browser). |
| Command Structure | 7 | Plan/Ask/Edit modes rather than granular commands. Subagents for parallel task decomposition. `.cursorrules` for project-specific AI behavior. MCP integration for context. |
| Output Formatting | 8 | Tabbed file viewing. File tree. Syntax highlighting. Agent-centric interface redesign (Cursor 2.0). |
| Error Handling | 5 | Agent mode can hang with loading spinner. Terminal commands sometimes marked as success without feedback. Known issues with terminal hanging. |
| Progress/Feedback | 6 | Plan mode shows analysis and to-do generation. Subagent parallel progress. Known spinner issues when using certain models with YOLO mode. |
| Help System | 6 | Documentation at cursor.com/docs. Agent modes discoverable via UI. Less command-line help since it is editor-first. |
| Configuration | 7 | `.cursorrules` project config. Figma MCP + Code MCP integration. Custom subagent prompts and tool access. |
| Accessibility | 3 | Electron-based, inherits system accessibility. No terminal-specific accessibility features. |
| Trust/Transparency | 7 | Plan mode shows reasoning before execution. Subagent task breakdown visible. Editable plans with file paths and code references. |

**Key UX Patterns to Adopt**:
- Agent modes (Plan/Ask/Edit) as first-class concepts
- Subagents for parallel task decomposition with visible progress
- `.cursorrules`-equivalent project config (already have `CUERVO.md` potential)
- Editable plans with code references

### 3.5 OpenAI Codex CLI

**Category**: Direct competitor
**Platform**: Node.js (TypeScript)
**Release**: April 2025

| Dimension | Score | Analysis |
|-----------|-------|----------|
| Onboarding | 8 | `codex` launches TUI immediately. OAuth browser flow or API key. `--cd` flag for directory targeting. Auto-detection of repo context. |
| Command Structure | 7 | Three approval modes: Auto, Read-only, Full Access. `/permissions` to switch modes in-session. Minimal slash commands, focuses on natural language. |
| Output Formatting | 7 | TUI-based rendering. File diffs with context. Tool use display. Esc-Esc to navigate message history. |
| Error Handling | 7 | Sandbox enforcement with clear error messages. Two-layer security model (sandbox + approval policy) provides clear error context. |
| Progress/Feedback | 7 | Web search indicators. Tool execution progress. Cached vs live browsing modes. |
| Help System | 6 | `--help` with comprehensive flag reference. In-session `/permissions` command. Online documentation. |
| Configuration | 7 | Config files with basic and advanced options. Three-tier approval model as primary config. BYOK support. |
| Accessibility | 3 | TUI-based, limited accessibility. No documented screen reader support. |
| Trust/Transparency | 9 | **Best-in-class permission model**: OS-enforced sandbox + approval policy (two independent layers). Clear scope boundaries (working directory, network). Explicit web search caching to reduce prompt injection. |

**Key UX Patterns to Adopt**:
- Two-layer security model (sandbox + approval policy)
- Three-tier approval modes (Auto/Read-only/Full Access)
- `/permissions` mode switching in-session
- OS-enforced sandbox boundaries with clear scope display
- Cached web search for safety

---

## 4. Product Benchmarks: Developer CLI Tools

### 4.1 Homebrew

**Category**: Package manager (UX benchmark)
**Platform**: Ruby

| Dimension | Score | Analysis |
|-----------|-------|----------|
| Onboarding | 5 | One-line install script. But: PATH configuration buried in output wall. `brew: command not found` is #1 beginner issue. Apple Silicon makes this worse. |
| Command Structure | 9 | Verb-noun pattern: `brew install`, `brew upgrade`, `brew search`. Consistent, predictable. `brew` alone shows help. Tab completion. |
| Output Formatting | 7 | Progress bars for downloads. Checkmarks for success. Warnings in yellow. Errors in red. Cask vs formula distinction. |
| Error Handling | 6 | `brew doctor` for self-diagnosis. But: sudo brew creates cascading file permission errors. Error messages don't always suggest fixes. |
| Progress/Feedback | 8 | Download progress bars. X of Y pattern for multi-package installs. Clear "Already installed" vs "Installing" vs "Upgrading" states. |
| Help System | 7 | `brew help`, `brew help <command>`. Man pages. `brew doctor` diagnostic. Online docs. |
| Configuration | 5 | Environment variables (HOMEBREW_*). No config file. Limited customization surface. |
| Accessibility | 3 | Terminal-dependent. Color used for status (red/yellow/green). No alternative indicators. |
| Trust/Transparency | 5 | Shows what is being installed. No AI decisions to explain. Cask auditing for security. |

**Key UX Patterns to Adopt**:
- `brew doctor` pattern (Cuervo already has this)
- Verb-noun command naming consistency
- "Already done" vs "In progress" vs "Upgrading" state communication
- Download/progress bar pattern for long operations
- One-command diagnostic self-check

### 4.2 npm/pnpm

**Category**: Package manager (UX benchmark)

| Dimension | Score | Analysis |
|-----------|-------|----------|
| Onboarding | 7 | npm comes with Node.js. pnpm: `npm install -g pnpm`. Immediate usability. `npm init` / `pnpm init` for project setup. |
| Command Structure | 8 | Verb-noun: `npm install`, `npm run`, `npm test`. pnpm validates all options (rejects unknown flags). pnpm shorthand: `pnpm lint` = `pnpm run lint`. |
| Output Formatting | 7 | Dependency tree visualization. Audit report formatting. pnpm recursive output with workspace prefixing. Customizable prefix templates. |
| Error Handling | 7 | pnpm strict option validation. Peer dependency warnings vs errors. `npm audit` with severity levels. Clear "fix available" suggestions. |
| Progress/Feedback | 7 | Progress bars for large installs. Package count tracking. pnpm shows concurrent install progress per package. |
| Help System | 7 | `npm help <command>`. Man pages. `npm help-search`. pnpm comprehensive online docs. |
| Configuration | 8 | `.npmrc` layered (project > user > global). pnpm `pnpm-workspace.yaml`. Config Dependencies for monorepos. Environment variable overrides. |
| Accessibility | 3 | Standard terminal output. `--no-color` flag. No screen reader specific features. |
| Trust/Transparency | 6 | `npm audit` security reporting. Lock file transparency. pnpm strict mode. Supply chain security features. |

**Key UX Patterns to Adopt**:
- Strict option validation (reject unknown flags)
- Layered configuration (project > user > global) -- Cuervo already has this
- Audit/security reporting pattern
- Shorthand command aliases (`pnpm lint` = `pnpm run lint`)
- `--no-color` flag for plain output

### 4.3 Vercel CLI

**Category**: Deployment platform (UX benchmark)

| Dimension | Score | Analysis |
|-----------|-------|----------|
| Onboarding | 8 | `vercel` in project directory starts interactive linking. OAuth via GitHub/GitLab/Bitbucket/Email/SAML. Auto-detects framework. Post-link prompts for env var pull. |
| Command Structure | 7 | Verb-noun: `vercel deploy`, `vercel dev`, `vercel env`. `vercel` alone triggers deploy. Project linking with interactive selector (<100 projects). |
| Output Formatting | 7 | Deployment URLs highlighted. Build log streaming. Environment summary. Clean status output. |
| Error Handling | 6 | Error list documentation. Build error troubleshooting guide. But: Windows support issues (breaks without prompting on some versions). |
| Progress/Feedback | 8 | Build streaming with step-by-step progress. Deployment URL shown immediately. Preview vs production distinction clear. |
| Help System | 6 | `vercel --help`. Online documentation. Knowledge base articles for common workflows. |
| Configuration | 7 | `vercel.json` project config. Environment variables via dashboard and CLI. `vercel pull` for sync. `vercel env` management. |
| Accessibility | 3 | Standard terminal output. No documented accessibility features. |
| Trust/Transparency | 6 | Deployment previews before production. Build logs visible. Environment variable scoping (development/preview/production). |

**Key UX Patterns to Adopt**:
- Interactive project linking flow on first run
- Framework auto-detection
- Post-action prompts ("pull env vars?")
- Deployment/action URLs shown immediately
- Interactive selector for disambiguation

### 4.4 Railway CLI

**Category**: Cloud platform (UX benchmark)
**Platform**: Rewritten from Go to Rust (2025)

| Dimension | Score | Analysis |
|-----------|-------|----------|
| Onboarding | 7 | `railway login` with browser or `--browserless` pairing code. Token-based CI/CD auth. Quick start tutorial. |
| Command Structure | 7 | `railway up`, `railway link`, `railway logs`. Verb pattern. Service-centric operations. |
| Output Formatting | 6 | Clean deploy output. Log streaming. Basic formatting. |
| Error Handling | 5 | Token confusion between RAILWAY_TOKEN and RAILWAY_API_TOKEN. "Unauthorized" errors without clear guidance on which token type needed. Many help station questions about auth errors. |
| Progress/Feedback | 6 | Deploy progress. Log streaming. Basic status indicators. |
| Help System | 5 | CLI reference documentation. Help station community forum. GitHub README. |
| Configuration | 6 | Environment-based. Two token types. Project-level settings via dashboard. |
| Accessibility | 2 | No documented accessibility features. |
| Trust/Transparency | 5 | Deploy logs. Service status. No AI-specific trust signals. |

**Key UX Patterns (Anti-patterns to Avoid)**:
- **Avoid**: Multiple token types without clear documentation of when to use each
- **Avoid**: "Unauthorized" errors without specifying which auth method is expected
- **Learn**: `--browserless` pairing code for environments without a browser (useful for SSH)

---

## 5. Product Benchmarks: AI/Conversational Products

### 5.1 ChatGPT / OpenAI Codex CLI

*Covered in Section 3.5 (Codex CLI). For the ChatGPT API interface:*

| Dimension | Score | Analysis |
|-----------|-------|----------|
| Onboarding | 7 | Multiple community CLIs (kardolus/chatgpt-cli, others). Official Codex CLI launched April 2025. OAuth browser flow or API key. |
| Command Structure | 6 | Varies by implementation. Official Codex CLI minimal. Community CLIs vary widely. |
| Output Formatting | 7 | Streaming text output. Markdown rendering varies by client. |
| Error Handling | 6 | Rate limit handling. Token exhaustion messages. Network error recovery. |
| Progress/Feedback | 6 | Streaming text as primary feedback. Multi-provider switching (`--target` flag). |
| Help System | 5 | CLI-specific documentation. `--help` flags. |
| Configuration | 6 | API key env vars. Model selection. Multi-provider targets. |
| Accessibility | 2 | Depends entirely on client implementation. |
| Trust/Transparency | 5 | Token counting. Model selection. No inline confidence or reasoning display. |

### 5.2 Perplexity

**Category**: AI search/retrieval (UX benchmark for trust patterns)

| Dimension | Score | Analysis |
|-----------|-------|----------|
| Onboarding | 8 | Web-first, no CLI. Immediate question-answer flow. No setup required for basic usage. Pro for advanced features. |
| Command Structure | N/A | Web interface, not CLI. |
| Output Formatting | 9 | **Best-in-class**: Inline citations on every claim. Source links with one-click verification. Structured answers with clear formatting. |
| Error Handling | 7 | Source quality indicators. Fallback to different sources. Clear "I don't know" patterns. |
| Progress/Feedback | 8 | 40% faster response speed (2025). Search progress indicators. Source crawling visualization. |
| Help System | 6 | Pro Search, Research Mode, Labs for different depths. Context-dependent suggestions. |
| Configuration | 5 | Pro vs free tiers. Model selection in Pro. |
| Accessibility | 5 | Web-based with standard browser accessibility. |
| Trust/Transparency | 10 | **Industry gold standard**: Every claim has inline citations. 20-50 sources evaluated per query. Authority/recency/relevance scoring visible. One-click source verification. |

**Key UX Patterns to Adopt**:
- **Inline citations on AI-generated claims** (critical for trust)
- Source quality indicators
- "Verify this" one-click links
- Progressive search depth (quick answer -> deep research)
- Authority/recency scoring transparency

### 5.3 Warp Terminal

**Category**: AI-native terminal (UX benchmark)

| Dimension | Score | Analysis |
|-----------|-------|----------|
| Onboarding | 8 | Download and launch. Account creation. AI features discoverable inline. Four interaction modes. |
| Command Structure | 7 | Four modes: traditional CLI, AI completions, agent coding, collaborative Drive. Warp Code for prompt-to-production. |
| Output Formatting | 9 | GPU-rendered terminal. Block-based output (each command in its own block). Syntax highlighting. Code review with tabbed file viewing. |
| Error Handling | 7 | AI-powered error explanation. Command suggestion on typos. Error block highlighting. |
| Progress/Feedback | 8 | Real-time agent feedback. Inline AI completions (GitHub Copilot-like). Warp Code step-by-step execution. |
| Help System | 7 | Command palette. AI-powered command search. Warp Drive for sharing. |
| Configuration | 8 | Custom themes (YAML-based). OS light/dark sync. Photo backgrounds. BYOK for AI features. Extensive settings. |
| Accessibility | 6 | **Best-in-class for terminals**: VoiceOver support (macOS, WIP). Voice input. Adjustable verbosity. Command palette. But: No screen reader support on Linux/Windows. |
| Trust/Transparency | 6 | AI suggestions clearly marked. Agent actions visible. SWE-bench scores published. |

**Key UX Patterns to Adopt**:
- Block-based output (each command result as discrete unit)
- AI-powered error explanation
- Command palette for discoverability
- Theme system with YAML definitions and OS sync
- Voice input as alternative input method
- Adjustable verbosity for accessibility

---

## 6. UX Dimension Cross-Analysis

### 6.1 Onboarding: Cross-Product Best Practices

| Pattern | Products Using It | Priority for Cuervo |
|---------|-------------------|---------------------|
| Zero-config first run (just type command name) | Claude Code, Aider, Codex | HIGH |
| Browser-based OAuth | Claude Code, Copilot, Vercel, Codex | MEDIUM |
| Browserless/CLI-only auth fallback | Railway, Aider (API key) | HIGH |
| Project-type auto-detection | Claude Code, Vercel | HIGH |
| Guided first-run wizard | Aider (model selection), Vercel (project linking) | HIGH |
| CUERVO.md/CLAUDE.md auto-context | Claude Code | HIGH |

**Cuervo Gap Analysis**: Current `cuervo init` creates a `.cuervo/` directory with a boilerplate config. It does not detect project type, suggest providers, check for existing API keys, or guide the user through model selection. The first-run experience needs a guided wizard.

### 6.2 Command Structure: Naming Convention Analysis

| Product | Pattern | Example |
|---------|---------|---------|
| Claude Code | Slash commands + skills | `/compact`, `/doctor`, `/review` |
| Copilot CLI | Slash commands + mode toggles | `/model`, `/context`, Shift+Tab |
| Homebrew | Verb-noun | `brew install`, `brew doctor` |
| pnpm | Verb-noun + shorthand | `pnpm install`, `pnpm lint` |
| Vercel | Verb-noun | `vercel deploy`, `vercel dev` |
| Cuervo | Subcommands + slash commands | `cuervo chat`, `/help`, `/model` |

**Cuervo Gap Analysis**: Cuervo's command structure is solid (Clap-based subcommands + REPL slash commands) but the REPL slash commands are limited (7 commands vs Claude Code's 10+ including skills). Missing key commands: `/compact`, `/review`, `/context`, `/undo`.

### 6.3 Trust/Transparency: The #1 Differentiator

The 2025 Stack Overflow Developer Survey revealed that **only 29% of developers trust AI accuracy** (down from 40%). Trust is the most important UX dimension for AI CLI tools.

| Trust Signal | Products Using It | Implementation |
|--------------|-------------------|----------------|
| Inline citations/sources | Perplexity (10/10) | Every claim linked to source |
| Plan before execute | Copilot CLI, Cursor, Codex | Show reasoning before implementation |
| Permission tiers | Claude Code, Codex | Auto/Read-only/Full Access |
| Cost tracking per session | Claude Code | Status line + round summary |
| Tool invocation preview | Claude Code, Codex, Copilot | Show command before execution |
| Confidence scores | None (emerging) | Models can self-assess uncertainty |
| Undo/revert capability | Aider, Cursor | Git-based rollback |
| Sandbox boundaries display | Codex CLI | Clear scope (directory, network) |

**Cuervo Gap Analysis**: Cuervo has permission checking, cost tracking (in metrics), and tool invocation display. It lacks: plan mode, confidence scores, inline citations for AI claims, `/undo`, sandbox boundary display, and token usage breakdown.

---

## 7. Academic & Framework Research

### 7.1 Nielsen Norman Group: AI UX Guidelines (2024-2026)

**Key Findings** (Source: [NN/G State of UX 2026](https://www.nngroup.com/articles/state-of-ux-2026/)):

1. **Post-hype AI era (2025)**: "The tech improved and found valuable new use cases (such as coding and search agents), but its limitations remain (inconsistency, hallucinations, edge-case failures, and the ongoing need for human oversight)."
2. **AI features fatigue**: "Lazy AI features and AI slop are now ubiquitous, and the shine is fading fast."
3. **Design implication**: Tools must deliver genuine user value, not just AI integration for its own sake.
4. **Recommendation**: Use AI to augment existing workflows, not replace them. Provide clear escape hatches.

**Application to Cuervo**: Focus on genuinely useful AI features (code explanation, error diagnosis, project understanding) rather than AI novelty. Every AI feature should have a non-AI fallback.

### 7.2 Google PAIR People + AI Guidebook (3rd Edition, April 2025)

**Key Principles** (Source: [PAIR Guidebook](https://pair.withgoogle.com/guidebook/)):

1. **Explain in-the-moment**: Provide reasons for a given inference, recommendation, or suggestion at the point of display.
2. **Set expectations**: Communicate what the AI can and cannot do.
3. **Support user control**: Allow users to correct, override, or undo AI actions.
4. **Data transparency**: Tell users where data is used to eliminate suspicion.

**Application to Cuervo**:
- Show reasoning for tool selections ("Using `bash` because the task requires file system changes")
- Display model limitations ("Note: I may not have access to files outside the working directory")
- Provide `/undo` for reversal of AI actions
- Show what data is sent to the model (token count, context sources)

### 7.3 WCAG 2.2 Terminal Accessibility

**Key Standards** (Source: [ACM CLI Accessibility Research](https://dl.acm.org/doi/fullHtml/10.1145/3411764.3445544)):

1. **Color contrast**: 4.5:1 ratio for normal text, 3:1 for large text/icons (WCAG 2.2 Level AA).
2. **Do not rely solely on color**: "Users with color blindness or low vision may not distinguish between certain hues, such as red and green."
3. **CLIs have inherent advantages**: Text-based and keyboard-operable, but this only covers a small subset of WCAG criteria.
4. **Screen reader challenges**: CLIs present unique challenges for screen readers, particularly around streaming output and progress indicators.
5. **European Accessibility Act (EAA)**: In force since June 28, 2025. WCAG 2.2 Level AA compliance is increasingly mandatory.

**Application to Cuervo**:
- Add `--no-color` / `NO_COLOR` environment variable support
- Use shape indicators alongside color (checkmark, X, warning triangle in ASCII)
- Ensure 4.5:1 contrast ratios in all color choices
- Provide `--plain` output mode for screen readers
- Test with VoiceOver/NVDA for streaming output accessibility

### 7.4 Cognitive Load Theory in CLI Design

**Key Principles** (Source: [Cognitive Load in Developer Tools](https://www.zigpoll.com/content/how-can-cognitive-load-theory-be-applied-to-improve-the-usability-of-developer-tools), [GitHub zakirullin/cognitive-load](https://github.com/zakirullin/cognitive-load)):

1. **Intrinsic load**: The inherent complexity of the task. Cannot be reduced but can be managed through chunking.
2. **Extraneous load**: Caused by poor design. Must be minimized. Examples: unclear error messages, inconsistent naming, unnecessary flags.
3. **Germane load**: Productive mental effort for learning. Can be optimized through progressive disclosure.

**Strategies**:
- **Chunking**: Group related output (collapsible sections, distinct visual blocks)
- **Progressive disclosure**: Show simple output first, expand on demand
- **Wizard-style flows**: Scaffold complex workflows (onboarding, config setup)
- **Consistent naming**: Same flags for same purposes across all subcommands

**Application to Cuervo**:
- Group `cuervo doctor` output into collapsible sections (already partially done with box drawing)
- Use progressive disclosure in error messages (brief message first, `--verbose` for stack trace)
- Implement wizard flow for `cuervo init` (detect project, suggest model, test connectivity)
- Ensure flag names are consistent (`--limit` everywhere, not `-l` in one place and `--max` in another)

### 7.5 Progressive Disclosure in Developer Tools

**Key Patterns** (Source: [IxDF Progressive Disclosure](https://www.interaction-design.org/literature/topics/progressive-disclosure), [Claude-Mem Docs](https://docs.claude-mem.ai/progressive-disclosure)):

1. **Level 1**: Show only necessary information upfront
2. **Level 2**: Reveal advanced details on demand
3. **Level 3**: Deep-dive available via explicit request

**Examples in CLI context**:
| Level | Error Message Pattern |
|-------|----------------------|
| 1 | `Error: API key not found for 'anthropic'` |
| 2 | `Run 'cuervo auth login anthropic' to configure, or set ANTHROPIC_API_KEY` |
| 3 | `Use --verbose for full API response details` |

**Application to Cuervo**: Most error paths in Cuervo currently provide Level 1 only. The `eprintln!` calls in `chat.rs` and `doctor.rs` should be enhanced with Level 2 (actionable suggestion) and Level 3 (verbose debugging).

### 7.6 Apple HIG Principles Applied to CLI

While Apple does not publish terminal-specific guidelines, the core principles (Clarity, Deference, Depth) from the [Human Interface Guidelines](https://developer.apple.com/design/human-interface-guidelines) apply:

1. **Clarity**: Output should be legible and precise. Avoid wall-of-text output. Use whitespace and structure.
2. **Deference**: The CLI should help users focus on their task, not on the tool itself. Minimize chrome, maximize signal.
3. **Depth**: Use layered information (summary -> detail -> debug) to convey hierarchy.

**Application to Cuervo**: The `cuervo doctor` output uses box-drawing characters which is good structure. But the main REPL experience could benefit from clearer visual hierarchy (section headers, indentation levels, whitespace between logical blocks).

---

## 8. Cuervo CLI Current State Assessment

Based on analysis of the codebase at `/Users/oscarvalois/Documents/Github/cuervo-cli`:

### 8.1 Onboarding (Current Score: 4/10)

**What exists**:
- `cuervo init` creates `.cuervo/config.toml` with commented defaults (40 lines, `crates/cuervo-cli/src/commands/init.rs`)
- `cuervo auth login <provider>` stores API key in OS keychain
- Default chat mode (no subcommand = REPL)

**What is missing**:
- No guided first-run wizard
- No project type detection (Rust, Node, Python, etc.)
- No API key validation during login
- No "first time? run `cuervo init`" suggestion when config is absent
- No model recommendation based on use case
- No connectivity test during setup

### 8.2 Command Structure (Current Score: 6/10)

**What exists** (`crates/cuervo-cli/src/main.rs`):
- Top-level: `chat`, `config`, `init`, `status`, `auth`, `trace`, `replay`, `memory`, `doctor`
- REPL slash: `/help`, `/quit`, `/clear`, `/model`, `/session`, `/test`
- Clap-based with `--model`, `--provider`, `--verbose`, `--config` global flags

**What is missing**:
- No `/compact` (context compression -- exists internally as ContextCompactor but not exposed)
- No `/context` (show token usage)
- No `/review` (code change analysis)
- No `/undo` (reverse last AI action)
- No `/plan` (enter plan mode)
- No `/cost` (show session cost)
- No extensible commands directory (like Claude Code's `.claude/commands/`)

### 8.3 Output Formatting (Current Score: 5/10)

**What exists** (`crates/cuervo-cli/src/render/`):
- `StreamRenderer`: Prose/CodeBlock state machine with syntax highlighting via syntect
- `markdown.rs`: Markdown rendering via termimad
- `tool.rs`: Tool result formatting
- `syntax.rs`: Syntax highlighting

**What is missing**:
- No color theming system
- No diff display with intra-line highlighting
- No block-based output (each response as discrete visual unit)
- No cost/token display per response
- No status line / progress bar during operations

### 8.4 Error Handling (Current Score: 5/10)

**What exists**:
- anyhow for error propagation
- Config validation with `IssueLevel::Error` / `IssueLevel::Warning` + suggestions
- MCP server failures shown as warnings (non-fatal)
- Provider fallback on error (resilience layer)

**What is missing**:
- No progressive disclosure (brief -> detailed -> debug)
- No "did you mean?" suggestions for unknown commands (`CommandResult::Unknown` just returns the string)
- No recovery suggestions for common errors (API key expired, network timeout, rate limit)
- Error colors/formatting not differentiated from normal output

### 8.5 Progress/Feedback (Current Score: 4/10)

**What exists**:
- Streaming text output (StreamRenderer)
- `[tool: name]` markers during tool invocations
- `[error: msg]` inline error display
- `[interrupted]` on Ctrl+C

**What is missing**:
- No spinner during model response wait
- No progress indicators for multi-step operations
- No "Reading..." -> "Read" state transitions
- No tool execution progress (time elapsed, step count)
- No cost display per round
- No token count display

### 8.6 Help System (Current Score: 5/10)

**What exists** (`crates/cuervo-cli/src/repl/commands.rs`):
- `/help` listing 7 commands with shortcuts
- Clap-generated `--help` for CLI
- `/test` with usage hints for unknown subcommands

**What is missing**:
- No contextual hints ("Type /help for commands" on first REPL entry)
- No "did you mean?" for typos in slash commands
- No examples in help text (just descriptions)
- No `/help <command>` for detailed per-command help
- No command auto-completion in REPL

### 8.7 Configuration (Current Score: 7/10)

**What exists**:
- `config.toml` with layered loading (global -> project)
- `config show`, `config get`, `config set`, `config path` subcommands
- Environment variable overrides (`CUERVO_MODEL`, `CUERVO_PROVIDER`, `CUERVO_LOG`, `CUERVO_CONFIG`)
- `config_loader.rs` with default paths

**What is missing**:
- No interactive config editor
- No config migration on version upgrade
- No `CUERVO.md` (project context file, equivalent to CLAUDE.md)
- Limited env var support (only 4 variables vs Aider's comprehensive set)

### 8.8 Accessibility (Current Score: 2/10)

**What exists**:
- Text-based output (inherently keyboard-navigable)
- No color-only information encoding (most status uses text labels)

**What is missing**:
- No `--no-color` / `NO_COLOR` env var support
- No `--plain` output mode
- No configurable color themes
- No high-contrast mode
- No screen reader testing
- No WCAG 2.2 compliance consideration
- Color used in syntax highlighting without text alternatives

### 8.9 Trust/Transparency (Current Score: 4/10)

**What exists**:
- Permission prompting for destructive tools (y/n/a)
- TBAC authorization with scoped contexts
- Safety guardrails (PII detection, command allowlists)
- Reflexion self-improvement (evaluates round outcomes)
- Doctor command with health diagnostics

**What is missing**:
- No plan mode (show reasoning before executing)
- No confidence indicators
- No inline citations for AI claims
- No cost display per interaction
- No token usage breakdown
- No sandbox boundary display
- No "what data is being sent" transparency
- No undo capability

---

## 9. Competitive Scorecard

### 9.1 Overall Scores (out of 90 possible)

| Product | Onb | Cmd | Out | Err | Prog | Help | Conf | A11y | Trust | **Total** |
|---------|-----|-----|-----|-----|------|------|------|------|-------|-----------|
| **Claude Code** | 9 | 9 | 8 | 8 | 8 | 9 | 8 | 4 | 7 | **70** |
| **Copilot CLI** | 8 | 8 | 9 | 7 | 7 | 7 | 8 | 3 | 8 | **65** |
| **Codex CLI** | 8 | 7 | 7 | 7 | 7 | 6 | 7 | 3 | 9 | **61** |
| **Warp** | 8 | 7 | 9 | 7 | 8 | 7 | 8 | 6 | 6 | **66** |
| **Cursor** | 8 | 7 | 8 | 5 | 6 | 6 | 7 | 3 | 7 | **57** |
| **Aider** | 7 | 6 | 7 | 6 | 5 | 6 | 8 | 4 | 5 | **54** |
| **Vercel CLI** | 8 | 7 | 7 | 6 | 8 | 6 | 7 | 3 | 6 | **58** |
| **Homebrew** | 5 | 9 | 7 | 6 | 8 | 7 | 5 | 3 | 5 | **55** |
| **npm/pnpm** | 7 | 8 | 7 | 7 | 7 | 7 | 8 | 3 | 6 | **60** |
| **Railway** | 7 | 7 | 6 | 5 | 6 | 5 | 6 | 2 | 5 | **49** |
| **Perplexity** | 8 | N/A | 9 | 7 | 8 | 6 | 5 | 5 | 10 | **58** |
| **Cuervo CLI** | **4** | **6** | **5** | **5** | **4** | **5** | **7** | **2** | **4** | **42** |

### 9.2 Gap Analysis (Cuervo vs Market Leader Claude Code)

| Dimension | Claude Code | Cuervo | Gap | Priority |
|-----------|-------------|--------|-----|----------|
| Onboarding | 9 | 4 | -5 | P0 (Critical) |
| Trust/Transparency | 7 | 4 | -3 | P0 (Critical) |
| Progress/Feedback | 8 | 4 | -4 | P0 (Critical) |
| Help System | 9 | 5 | -4 | P1 (High) |
| Output Formatting | 8 | 5 | -3 | P1 (High) |
| Error Handling | 8 | 5 | -3 | P1 (High) |
| Command Structure | 9 | 6 | -3 | P1 (High) |
| Accessibility | 4 | 2 | -2 | P2 (Opportunity) |
| Configuration | 8 | 7 | -1 | P3 (Maintain) |

---

## 10. Actionable Recommendations for Cuervo CLI

### P0: Critical (Address First -- Maximum Impact)

#### R1: Guided Onboarding Wizard
**Gap**: Onboarding score 4 vs market average 7.5
**Effort**: Medium (2-3 days)
**Implementation**:
```
cuervo init
  -> Detect project type (Cargo.toml? package.json? pyproject.toml?)
  -> "Detected Rust project. Recommended model: claude-sonnet-4-5"
  -> "Enter API key or run 'cuervo auth login anthropic':"
  -> Test provider connectivity
  -> Create .cuervo/config.toml with detected settings
  -> Create CUERVO.md template with project-specific context
  -> "Ready! Run 'cuervo' to start chatting."
```
**File**: `/Users/oscarvalois/Documents/Github/cuervo-cli/crates/cuervo-cli/src/commands/init.rs`

#### R2: Plan Mode
**Gap**: No plan-before-execute capability
**Effort**: Medium (leverages existing PlanningSource + AdaptivePlanner)
**Implementation**:
- Add `/plan` slash command to enter plan mode
- Show structured plan before execution
- Allow user to approve/reject/modify plan
- Display reasoning for tool selections
**Files**: `commands.rs`, `agent.rs`, `planner.rs`

#### R3: Streaming Progress Indicators
**Gap**: Progress score 4 vs market average 7
**Effort**: Low-Medium (1-2 days)
**Implementation**:
- Spinner during model response wait (before first token)
- "Thinking..." -> "Writing..." state transitions
- Tool execution: `[bash: running 'cargo build'...]` with elapsed time
- Round summary: tokens used, cost, duration
**File**: `/Users/oscarvalois/Documents/Github/cuervo-cli/crates/cuervo-cli/src/render/stream.rs`

#### R4: Cost and Token Transparency
**Gap**: No per-interaction cost/token display
**Effort**: Low (data already collected via InvocationMetric)
**Implementation**:
- Display after each response: `[sonnet] 1,234 tokens | $0.0045 | 2.1s`
- Add `/cost` command showing session total
- Add `/context` command showing context window usage
**Files**: `agent.rs`, `commands.rs`, `stream.rs`

### P1: High Priority (Address Next)

#### R5: Enhanced Error Messages with Progressive Disclosure
**Effort**: Low (1 day)
**Implementation**:
```
Level 1: "Error: Provider 'anthropic' not available"
Level 2: "Run 'cuervo auth login anthropic' or set ANTHROPIC_API_KEY"
Level 3: "Use --verbose for connection diagnostics"
```
- Apply to all `eprintln!` calls in chat.rs, doctor.rs, auth.rs
- Add "did you mean?" for unknown commands using edit distance
**Files**: All `commands/*.rs`, `repl/commands.rs`

#### R6: Expanded Slash Commands
**Effort**: Medium (2-3 days total)
**New commands**:
| Command | Description | Leverages |
|---------|-------------|-----------|
| `/compact` | Compress context window | Existing ContextCompactor |
| `/context` | Show token usage breakdown | Existing assembler |
| `/cost` | Show session cost summary | Existing InvocationMetric |
| `/review` | Analyze staged git changes | Existing git tools |
| `/undo` | Revert last AI file change | git stash/checkout |
| `/plan` | Enter/exit plan mode | Existing PlanningSource |
| `/permissions` | Show/change approval mode | Existing PermissionChecker |

**File**: `/Users/oscarvalois/Documents/Github/cuervo-cli/crates/cuervo-cli/src/repl/commands.rs`

#### R7: Output Formatting Improvements
**Effort**: Medium (2-3 days)
**Implementation**:
- Add box-drawing separators between AI responses and user input
- Show tool results in collapsible blocks (summary line, expand on request)
- Add diff display for file modifications (leveraging syntect for highlighting)
- Color-code by message type: AI response (default), tool output (dimmed), errors (red), warnings (yellow)
**Files**: `render/*.rs`

#### R8: CUERVO.md Project Context
**Effort**: Low (1 day)
**Implementation**:
- Auto-load `CUERVO.md` from project root (like CLAUDE.md)
- Inject into system prompt on every session
- Create template during `cuervo init`
- Document format and best practices
**Files**: `cuervo-context/`, `init.rs`

### P2: Differentiation Opportunities

#### R9: Accessibility Foundation
**Effort**: Medium (2-3 days)
**Implementation**:
- Support `NO_COLOR` environment variable (standard: https://no-color.org/)
- Add `--no-color` flag
- Add `--plain` flag for screen-reader-friendly output (no box drawing, no ANSI)
- Ensure all color-conveyed information has text equivalent
- Document accessibility features
**Files**: `main.rs`, `render/*.rs`, `doctor.rs`

#### R10: Confidence Indicators (Novel Differentiator)
**Effort**: High (3-5 days)
**Implementation**:
- Add self-assessed confidence to AI responses: `[confidence: high]` / `[confidence: uncertain -- verify this]`
- Use model's own uncertainty estimation (ask model to rate its confidence)
- Flag statements that should be verified
- This would be **unique in the market** -- no competitor currently does this
**Files**: `agent.rs`, `stream.rs`, core types

#### R11: Interactive Config Wizard
**Effort**: Medium (2 days)
**Implementation**:
- `cuervo config wizard` interactive setup
- Step through: provider, model, tools, security, memory settings
- Validate each setting as entered
- Write to config file with comments
**Files**: `commands/config.rs`

#### R12: Theme System
**Effort**: Medium (2-3 days)
**Implementation**:
- TOML-based theme definitions (colors for prompt, AI output, errors, tools, code)
- Built-in themes: default, dark, light, high-contrast, solarized
- `cuervo config set theme high-contrast`
- Environment variable: `CUERVO_THEME`
**Files**: New `render/theme.rs`, config types

### P3: Maintenance / Future Considerations

#### R13: Extensible Commands Directory
**Effort**: Medium (2-3 days)
**Implementation**:
- `.cuervo/commands/` for project-specific slash commands
- `~/.cuervo/commands/` for global custom commands
- YAML/TOML frontmatter with `when:` conditions
- Auto-discovery in `/help` listing

#### R14: Command Auto-completion in REPL
**Effort**: Medium (reedline supports custom completers)
**Implementation**:
- Tab completion for slash commands
- File path completion for tool arguments
- Model name completion for `/model` command

#### R15: Session Replay UX
**Effort**: Low (enhance existing trace/replay)
**Implementation**:
- `cuervo replay <id>` with time-compressed playback
- Show diff between original and replay output
- Allow branching from any point in a session

---

## 11. Sources

### Product Documentation & Research
- [Claude Code CLI Reference (2025 Guide)](https://www.eesel.ai/blog/claude-code-cli-reference)
- [Claude Code Slash Commands](https://code.claude.com/docs/en/slash-commands)
- [Claude Code Best Practices](https://code.claude.com/docs/en/best-practices)
- [Claude Code Status Line](https://code.claude.com/docs/en/statusline)
- [GitHub Copilot CLI Features](https://github.com/features/copilot/cli)
- [Copilot CLI Enhanced Agents (Jan 2026)](https://github.blog/changelog/2026-01-14-github-copilot-cli-enhanced-agents-context-management-and-new-ways-to-install/)
- [Copilot CLI Plan Mode (Jan 2026)](https://github.blog/changelog/2026-01-21-github-copilot-cli-plan-before-you-build-steer-as-you-go/)
- [Aider Documentation](https://aider.chat/docs/)
- [Aider Configuration Options](https://aider.chat/docs/config/options.html)
- [Cursor Features](https://cursor.com/features)
- [Cursor CLI Agent Modes (Jan 2026)](https://cursor.com/changelog/cli-jan-16-2026)
- [Codex CLI Features](https://developers.openai.com/codex/cli/features/)
- [Codex CLI Security Model](https://developers.openai.com/codex/security/)
- [Homebrew Documentation](https://docs.brew.sh/)
- [pnpm CLI Documentation](https://pnpm.io/pnpm-cli)
- [pnpm in 2025](https://pnpm.io/blog/2025/12/29/pnpm-in-2025)
- [Vercel CLI Overview](https://vercel.com/docs/cli)
- [Railway CLI Documentation](https://docs.railway.com/guides/cli)
- [Warp Terminal 2025 in Review](https://www.warp.dev/blog/2025-in-review)
- [Warp Accessibility](https://docs.warp.dev/terminal/more-features/accessibility)
- [Warp Themes](https://docs.warp.dev/terminal/appearance/themes)
- [Perplexity AI Guide 2026](https://notiongraffiti.com/perplexity-ai-guide-2026/)
- [Perplexity Architecture](https://www.frugaltesting.com/blog/behind-perplexitys-architecture-how-ai-search-handles-real-time-web-data)

### UX Frameworks & Academic Research
- [Nielsen Norman Group: State of UX 2026](https://www.nngroup.com/articles/state-of-ux-2026/)
- [NN/G: AI Work Study Guide](https://www.nngroup.com/articles/ai-work-study-guide/)
- [NN/G: Top 10 UX Articles of 2025](https://www.nngroup.com/articles/top-articles-2025/)
- [Google PAIR People + AI Guidebook](https://pair.withgoogle.com/guidebook/)
- [PAIR Guidebook Patterns](https://pair.withgoogle.com/guidebook/patterns)
- [Apple Human Interface Guidelines](https://developer.apple.com/design/human-interface-guidelines)
- [ACM: Accessibility of Command Line Interfaces](https://dl.acm.org/doi/fullHtml/10.1145/3411764.3445544)
- [WCAG 2.2 Color Contrast Guide](https://www.allaccessible.org/blog/color-contrast-accessibility-wcag-guide-2025)
- [Cognitive Load in Developer Tools](https://www.zigpoll.com/content/how-can-cognitive-load-theory-be-applied-to-improve-the-usability-of-developer-tools)
- [Cognitive Load Theory (GitHub)](https://github.com/zakirullin/cognitive-load)
- [IxDF: Progressive Disclosure](https://www.interaction-design.org/literature/topics/progressive-disclosure)
- [CLI UX Progress Display Patterns (Evil Martians)](https://evilmartians.com/chronicles/cli-ux-best-practices-3-patterns-for-improving-progress-displays)
- [Command Line Interface Guidelines](https://clig.dev/)
- [UX Patterns for CLI Tools](https://www.lucasfcosta.com/blog/ux-patterns-cli-tools)
- [PatternFly CLI Handbook](https://www.patternfly.org/developer-resources/cli-handbook/)
- [Atlassian: 10 Design Principles for Delightful CLIs](https://www.atlassian.com/blog/it-teams/10-design-principles-for-delightful-clis)

### Trust & Transparency Research
- [Stack Overflow 2025 Developer Survey: AI Trust](https://survey.stackoverflow.co/2025/ai)
- [Smashing Magazine: Psychology of Trust in AI](https://www.smashingmagazine.com/2025/09/psychology-trust-ai-guide-measuring-designing-user-confidence/)
- [Explainable AI Design Guidelines (Springer)](https://link.springer.com/article/10.1007/s10462-025-11363-y)
- [Supernova: AI Guidelines in Design Systems](https://www.supernova.io/blog/top-6-examples-of-ai-guidelines-in-design-systems)
- [Perplexity Citation-Forward Design](https://www.unusual.ai/blog/perplexity-platform-guide-design-for-citation-forward-answers)

---

## Appendix A: Cuervo CLI File Inventory (UX-Relevant)

| File | Purpose | UX Score |
|------|---------|----------|
| `crates/cuervo-cli/src/main.rs` | CLI entry, Clap command structure | 6/10 |
| `crates/cuervo-cli/src/commands/init.rs` | Project initialization | 4/10 |
| `crates/cuervo-cli/src/commands/chat.rs` | REPL + single-shot execution | 5/10 |
| `crates/cuervo-cli/src/commands/doctor.rs` | Health diagnostics | 7/10 |
| `crates/cuervo-cli/src/repl/commands.rs` | Slash command dispatcher | 5/10 |
| `crates/cuervo-cli/src/repl/prompt.rs` | REPL prompt rendering | 6/10 |
| `crates/cuervo-cli/src/repl/permissions.rs` | Permission prompts | 6/10 |
| `crates/cuervo-cli/src/render/stream.rs` | Streaming response renderer | 5/10 |
| `crates/cuervo-cli/src/render/markdown.rs` | Markdown rendering | 5/10 |
| `crates/cuervo-cli/src/render/syntax.rs` | Syntax highlighting | 6/10 |
| `crates/cuervo-cli/src/render/tool.rs` | Tool result formatting | 5/10 |
| `crates/cuervo-cli/src/config_loader.rs` | Config loading/layering | 7/10 |

## Appendix B: Implementation Priority Matrix

```
                    HIGH IMPACT
                        |
         R1 (Wizard)    |  R2 (Plan Mode)
         R3 (Progress)  |  R10 (Confidence)
                        |
  LOW EFFORT -----+-----+----- HIGH EFFORT
                        |
         R4 (Cost)      |  R13 (Ext Commands)
         R5 (Errors)    |  R14 (Autocomplete)
         R8 (CUERVO.md) |  R12 (Themes)
         R9 (NO_COLOR)  |
                        |
                    LOW IMPACT
```

**Recommended implementation order**:
1. R4 (Cost/Token display) + R8 (CUERVO.md) + R9 (NO_COLOR) -- quick wins, 2-3 days total
2. R1 (Onboarding wizard) + R5 (Error messages) -- biggest gap closure, 3-4 days
3. R3 (Progress indicators) + R6 (Slash commands) -- streaming UX parity, 3-4 days
4. R2 (Plan mode) + R7 (Output formatting) -- competitive differentiation, 4-5 days
5. R10 (Confidence) + R12 (Themes) -- novel differentiation, 5-7 days
