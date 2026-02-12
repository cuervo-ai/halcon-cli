# Cuervo CLI — Design System Specification

**Project:** Cuervo CLI
**Date:** February 2026
**Author:** UX Research & Product Design Team
**Version:** 1.0

---

## 1. Design Principles

### 1.1 Core Principles

| # | Principle | Description | Application |
|---|-----------|-------------|-------------|
| P1 | **Terminal-Native** | Embrace the terminal as the medium. No web-envy — design for monospace, streaming, and keyboard. | Use box-drawing, ASCII art only when it adds clarity. Avoid gratuitous decoration. |
| P2 | **Progressive Disclosure** | Show essentials first, details on demand. Beginners see simplicity, experts see power. | Default output is concise. `--verbose` reveals internals. /help shows basics, /?cmd shows specifics. |
| P3 | **Predictable Feedback** | Every action gets a response. Every wait gets an indicator. No silent states. | Spinners for inference, timers for tools, confirmations for operations, summaries for sessions. |
| P4 | **Error as Guidance** | Errors are teaching moments. Every error message is a mini-tutorial. | What happened + Why + How to fix. Always include a runnable recovery command. |
| P5 | **Composable** | Output is machine-readable. Behavior is scriptable. Integrates into workflows. | `--json`, `--quiet`, `NO_COLOR`, documented exit codes, stdin/stdout pipes. |
| P6 | **Accessible by Default** | Works for everyone: colorblind users, screen readers, slow terminals, pipes. | Color is never the only signal. ASCII fallbacks. Semantic structure. |
| P7 | **Trust Through Transparency** | Show what the AI is doing, what it costs, what it will execute. | Tool previews, cost displays, context indicators, permission explanations. |

### 1.2 Design Tenets (Decision Tiebreakers)

When principles conflict, resolve in this order:

1. **Correctness over speed** — Never sacrifice data integrity for UX polish
2. **Clarity over brevity** — A slightly longer message that's clear beats a terse one that's cryptic
3. **Defaults over configuration** — Work out of the box; make customization possible, not required
4. **Convention over invention** — Follow established CLI patterns (git, cargo, npm) before inventing new ones

---

## 2. Terminal Color System

### 2.1 Semantic Color Palette

| Token | Purpose | ANSI Code | Hex (ref) | Fallback (no color) |
|-------|---------|-----------|-----------|---------------------|
| `color.success` | Successful operations | Green (32) | #22c55e | `[OK]` prefix |
| `color.error` | Errors | Red (31) | #ef4444 | `[ERROR]` prefix |
| `color.warning` | Warnings | Yellow (33) | #eab308 | `[WARN]` prefix |
| `color.info` | Informational | Blue (34) | #3b82f6 | `[INFO]` prefix |
| `color.muted` | Secondary text | Dark Gray (90) | #6b7280 | No styling |
| `color.accent` | Highlights, prompts | Cyan (36) | #06b6d4 | No styling |
| `color.code` | Code/commands | White Bold (1;37) | #f9fafb | Backtick wrap |
| `color.model` | AI-generated content | Default (0) | — | No styling |

### 2.2 Color Rules

1. **Never use color as the only signal.** Every colored element must also have a text label, icon, or structural cue.
2. **Respect `NO_COLOR`** environment variable (https://no-color.org/).
3. **Respect `TERM=dumb`** — no ANSI codes when terminal doesn't support them.
4. **Detect pipe/non-TTY** — disable colors when stdout is not a terminal.
5. **Support `--color=always|auto|never`** flag override.

### 2.3 Implementation Pattern

```rust
// cuervo-cli/src/render/color.rs
pub fn is_color_enabled() -> bool {
    if std::env::var("NO_COLOR").is_ok() { return false; }
    if std::env::var("TERM").ok().as_deref() == Some("dumb") { return false; }
    atty::is(atty::Stream::Stdout)
}
```

---

## 3. Typography & Layout

### 3.1 Text Hierarchy

| Level | Usage | Format | Example |
|-------|-------|--------|---------|
| H1 | Section headers | Bold + Uppercase | `CONFIGURATION` |
| H2 | Sub-section headers | Bold | `Providers` |
| Body | Standard text | Default | Regular output |
| Caption | Metadata, timestamps | Dark Gray | Session info, costs |
| Code | Commands, paths | Bold White | `cuervo auth login anthropic` |
| Error | Error messages | Red | `Error: provider not found` |

### 3.2 Spacing Rules

| Context | Rule |
|---------|------|
| Between sections | 1 blank line |
| Between key-value pairs | No blank lines |
| After headers | No blank line (content follows immediately) |
| Before/after errors | 1 blank line above |
| Welcome message to prompt | 1 blank line |

### 3.3 Indentation

| Context | Indent |
|---------|--------|
| Top-level output | 0 spaces |
| List items | 2 spaces |
| Sub-items | 4 spaces |
| Error details/suggestions | 2 spaces |
| Tool output body | 3 spaces (under `╭─`) |

### 3.4 Column Alignment

```
Fixed-width fields align on consistent columns:

  Provider:  anthropic (connected)
  Model:     claude-sonnet-4-5
  Session:   a1b2c3d4 (new)
  Cost:      $0.0042
```

Use 2-space gap minimum between columns in tabular data.

---

## 4. Component Library

### 4.1 Prompt

```
cuervo [model] >_
cuervo [model] :_     (vi normal mode)
... _                  (multiline continuation)
(search: 'term') >_   (history search)
```

**Spec:**
- `cuervo` — literal, always present
- `[model]` — short model name in brackets (max 20 chars)
- ` > ` — indicator with space padding
- Multiline: `... ` with 4-char indent

---

### 4.2 Inference Indicator

```
Thinking... (2.1s)
```

**Spec:**
- Appears immediately after user presses Enter
- `Thinking...` in muted color
- `(Xs)` elapsed counter updates every 100ms
- Cleared when first streaming token arrives
- In `--quiet` mode: suppressed entirely

---

### 4.3 Response Footer

```
[423 tokens | $0.003 | 1.2s | round 3]
```

**Spec:**
- Appears after each model response completes
- Muted color (dark gray)
- Components: tokens, cost, latency, round number
- In `--quiet` mode: suppressed entirely
- In `--verbose` mode: adds input_tokens, output_tokens, cache hit

---

### 4.4 Tool Execution Chrome

```
  +-  bash(echo "hello world")
  |   hello world
  +- [OK] (0.1s)
```

With permission prompt:
```
  +-  bash(rm -rf node_modules)
  |   Deletes node_modules/ directory
  |   Working dir: /Users/alex/project
  |
  |   Allow?  [y]es  [n]o  [a]lways  [?]explain
  |
  |   ...output...
  +- [OK] (3.2s)
```

**Spec:**
- Header: `+-  tool_name(summary)` — 2-space indent, tool name bold
- Body: `|   ` prefix (pipe + 3 spaces)
- Footer: `+- [STATUS] (elapsed)` — status is OK/ERROR/DENIED
- Permission: inline within tool chrome, not separate
- Error: `+- [ERROR] message` in red

**ASCII fallback (NO_COLOR):**
```
  +-- bash(echo "hello world")
  |   hello world
  +-- [OK] (0.1s)
```

---

### 4.5 Round Separator

```
--- round 4 ---
```

**Spec:**
- Appears between agent rounds (not after the first)
- Muted color
- Centered in terminal width (or left-aligned if narrow)
- In `--quiet` mode: suppressed

---

### 4.6 Error Message

```
Error: Provider 'openai' not configured

  The provider 'openai' is not enabled or has no API key configured.

  To fix:
    cuervo auth login openai

  Related: cuervo status (shows all provider status)
```

**Spec:**
- Line 1: `Error:` in red, followed by concise description
- Line 2: Blank
- Line 3: Explanation in default color, 2-space indent
- Line 4: Blank
- Line 5: `To fix:` label, 2-space indent
- Line 6: Command in bold/code, 4-space indent
- Line 7: Blank
- Line 8: `Related:` optional cross-reference, 2-space indent

**Warning variant:**
```
Warning: MCP server 'github' failed to start

  Connection refused at localhost:3001.
  Tools from this server will not be available.

  To fix:
    Check if the server is running
    Or disable: [mcp.servers.github] enabled = false
```

---

### 4.7 Welcome Message

```
cuervo v0.1.0 — AI-powered CLI for software development
Provider: anthropic (connected)  Model: sonnet  Session: a1b2c3d4 (new)

Type /help for commands, /quit to exit.
```

**First-run variant:**
```
cuervo v0.1.0 — AI-powered CLI for software development
Provider: anthropic (connected)  Model: sonnet  Session: a1b2c3d4 (new)

Try:  "Explain this codebase"     Analyze the current project
      "Fix the failing tests"     Debug and fix test failures
      "Write a function that..."  Generate code

Type /help for all commands, /quit to exit.
```

**Spec:**
- Line 1: Version + tagline
- Line 2: Provider + model + session (one line, compact)
- Line 3: Blank
- Line 4+: Commands/tips (first-run only shows examples)
- Final: Help hint

---

### 4.8 Help Output

```
Commands:

  Chat
    /help, /h, /?        Show this help
    /quit, /exit, /q     Exit cuervo
    /clear               Clear the screen
    /model               Show current model

  Session
    /session list         List recent sessions
    /session show         Show current session info

  Diagnostics
    /test                 Run self-diagnostics
    /test provider <n>    Test a specific provider
    /doctor               Run full health check
    /memory stats         Show memory statistics

  Shortcuts
    Ctrl+C               Cancel current request
    Ctrl+D               Exit
    Ctrl+R               Search history
    Up/Down              Navigate history
    Alt+Enter            New line (multi-line input)
```

**Spec:**
- Grouped by function (not alphabetical)
- Category headers in bold
- 2-space indent for items
- Aligned descriptions at consistent column
- All available commands listed (including CLI-level ones accessible from REPL)

---

### 4.9 Doctor Output

```
Cuervo Doctor
=============

Configuration
  All settings valid.

Providers
  Primary: anthropic/claude-sonnet-4-5
  anthropic: [OK] 97.2% success, 1.2s avg, 45 calls
  ollama:    [--] no data (0 calls)

Health Scores
  anthropic: 92/100 (Healthy)
  ollama:    100/100 (Healthy, no data)

Cache
  Entries: 23 | Total hits: 156
  Oldest: 2026-01-15 14:32 UTC
  Newest: 2026-02-07 09:15 UTC

Metrics (last 30 days)
  Total invocations: 450
  Total cost: $1.2345
  Total tokens: 234,567

Recommendations
  - All systems nominal.
```

**Spec:**
- No box drawing (replaced with underline header)
- ASCII-safe (`[OK]`, `[--]`, not colored boxes)
- Sections always present but show "no data" states
- Timestamps always include UTC label
- Numbers formatted with commas for thousands

---

### 4.10 Progress Spinner

```
Thinking... (2.1s)
Loading context [3/5]...
Saving session...
```

**Spec:**
- Rendered to stderr (not pollute stdout)
- Cleared with `\r` + spaces when operation completes
- Shows elapsed time where applicable
- Shows progress fraction where applicable ([X/N])
- In `--quiet` mode: suppressed entirely

---

### 4.11 Confirmation Prompt

```
Prune 5 memory entries older than 30 days? [y/N]:
```

**Spec:**
- Question describes the action and scope
- Default in uppercase: `[y/N]` means default is No, `[Y/n]` means default is Yes
- Accepts: y/yes/Y (confirm), n/no/N/Enter (deny for [y/N])
- In non-interactive mode: use default

---

## 5. Output Modes

### 5.1 Mode Matrix

| Mode | Flag | Behavior |
|------|------|----------|
| **Interactive** | (default, TTY) | Full color, spinners, prompts, chrome |
| **Quiet** | `--quiet` or `-q` | Suppress spinners, chrome, metadata. Only model output. |
| **JSON** | `--json` | Machine-readable JSON output for all data commands |
| **Verbose** | `--verbose` or `-v` | Extra detail: cache hits, context sources, token breakdown |
| **No Color** | `NO_COLOR=1` or `--no-color` | All styling removed, ASCII fallbacks |
| **Pipe** | (non-TTY stdout) | Auto-detect: disable color, spinners, interactive prompts |

### 5.2 JSON Output Schema

For data commands (`status`, `doctor`, `memory list/stats`, `trace export`):

```json
{
  "version": "0.1.0",
  "command": "doctor",
  "timestamp": "2026-02-07T15:30:00Z",
  "data": {
    "configuration": { "valid": true, "issues": [] },
    "providers": [
      { "name": "anthropic", "model": "claude-sonnet-4-5", "status": "ok", "success_rate": 0.972 }
    ],
    "cache": { "entries": 23, "total_hits": 156 },
    "metrics": { "total_invocations": 450, "total_cost_usd": 1.2345 }
  }
}
```

---

## 6. Interaction Patterns

### 6.1 Permission Flow

```
User Query → Context Assembly → Model Inference → Tool Request
                                                       ↓
                                              Permission Check
                                              ├─ Auto-allowed → Execute
                                              ├─ Needs prompt → Display preview → User y/n/a
                                              │                                   ├─ y → Execute
                                              │                                   ├─ a → Execute + Remember
                                              │                                   └─ n → Skip + Tell AI
                                              └─ Denied by TBAC → Skip + Tell AI
```

### 6.2 Error Recovery Flow

```
Error Occurs → Classify Error Type
               ├─ UserFixable → Show: What + Why + Fix command
               ├─ Transient   → Auto-retry (1x) → If fail: Show + Suggest wait
               ├─ SystemError → Show: What + Suggest cuervo doctor
               └─ Fatal       → Show: What + Exit with code
```

### 6.3 Session Lifecycle

```
Launch → Load Config → Validate → Create/Resume Session → REPL Loop
                                                            ↓
                                                    User Input
                                                    ├─ /command → Handle slash command
                                                    ├─ Text    → Agent loop
                                                    └─ Ctrl+D  → Save + Exit
```

---

## 7. Prototypes

### 7.1 Prototype: First-Run Wizard

```
$ cuervo

  Welcome to Cuervo! Let's get you set up.

  ? Select your AI provider:
    > Anthropic (Claude)
      OpenAI (GPT-4)
      Ollama (Local — no API key needed)

  ? Enter your Anthropic API key: sk-ant-api03-****
    Testing connection... [OK] Connected (claude-sonnet-4-5)

  ? Save key to OS keychain? (recommended) [Y/n]: Y
    Key stored securely.

  ? Initialize project config in current directory? [Y/n]: Y
    Created .cuervo/config.toml

  Setup complete! Starting chat session.

  ------

  cuervo v0.1.0 — AI-powered CLI for software development
  Provider: anthropic (connected)  Model: sonnet  Session: a1b2c3d4 (new)

  Try:  "Explain this codebase"     Analyze the current project
        "Fix the failing tests"     Debug and fix test failures
        "Write a function that..."  Generate code

  Type /help for all commands, /quit to exit.

  cuervo [sonnet] >
```

### 7.2 Prototype: Enhanced Doctor

```
$ cuervo doctor

  Cuervo Doctor
  =============

  Configuration
    [OK] All settings valid
    Config: ~/.cuervo/config.toml + .cuervo/config.toml

  Providers
    Primary: anthropic/claude-sonnet-4-5
    anthropic: [OK] 97.2% success | 1.2s avg | 45 calls
    ollama:    [--] no invocation data yet

  Health
    anthropic: 92/100 Healthy
    ollama:    100/100 Healthy (no data)

  Cache
    23 entries | 156 hits | hit rate 87%
    Range: 2026-01-15 to 2026-02-07 UTC

  Metrics (30d)
    450 invocations | $1.23 total | 234k tokens
    Top model: anthropic/sonnet (420 calls, $1.15)

  Recommendations
    All systems nominal. No issues detected.
```

### 7.3 Prototype: Enhanced Error Messages

```
$ cuervo chat --provider openai

  Error: Provider 'openai' not configured

    No API key found for 'openai'. Keys are checked in order:
    1. Environment variable: $OPENAI_API_KEY (not set)
    2. OS keychain: openai (not found)

    To fix:
      cuervo auth login openai

    Or use a configured provider:
      cuervo chat --provider anthropic
```

### 7.4 Prototype: Tool Execution with Preview

```
  cuervo [sonnet] > Deploy to staging

  Thinking... (1.3s)

  I'll deploy the current branch to staging. This involves:
  1. Running tests
  2. Building the release binary
  3. Deploying via railway

  +-  bash(cargo test --workspace)
  |   Running 690 tests... all passed (12.3s)
  +- [OK] (12.3s)

  +-  bash(cargo build --release)
  |   Compiling cuervo v0.1.0
  |   Finished release target(s) in 45.2s
  +- [OK] (45.2s)

  +-  bash(railway up --environment staging)
  |   Deploying to staging...
  |
  |   Allow?  [y]es  [n]o  [a]lways  [?]explain
  |
  |   Deploying cuervo@0.1.0 to staging.railway.app
  |   Deploy live at: https://staging.cuervo.railway.app
  +- [OK] (23.1s)

  Deployment complete! Your app is live at staging.cuervo.railway.app

  [1,247 tokens | $0.0089 | 82.1s | round 1]
```

---

## 8. Migration Guide (Current → Target)

### 8.1 Breaking Changes (Require Major Version)

None — all changes are additive or backward-compatible.

### 8.2 Additive Changes (Non-Breaking)

| Change | Component | Priority |
|--------|-----------|----------|
| Inference spinner | `repl/mod.rs` | P0 |
| Response footer `[tokens\|cost\|time]` | `repl/mod.rs` | P0 |
| Round separators | `repl/mod.rs` | P1 |
| Enhanced tool chrome | `render/tool.rs` | P1 |
| Categorized /help | `repl/commands.rs` | P1 |
| `NO_COLOR` support | `render/color.rs` (new) | P0 |
| `--json` flag | `main.rs`, all commands | P1 |
| `--quiet` flag | `main.rs`, all commands | P1 |
| First-run wizard | `commands/setup.rs` (new) | P1 |
| Confirmation prompts | `commands/memory.rs` | P1 |
| Structured error format | `render/error.rs` (new) | P0 |

### 8.3 Deprecations

| Deprecated | Replacement | Timeline |
|------------|-------------|----------|
| Unicode box in doctor | ASCII underline headers | v0.2.0 |
| `config set` (placeholder) | Implement or remove | v0.2.0 |
| Inconsistent error prefixes | Unified `Error:` format | v0.2.0 |

---

*This design system should evolve as the product matures. Review quarterly.*
