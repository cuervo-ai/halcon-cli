# UX Implementation Specifications

**Project:** Cuervo CLI
**Date:** February 2026
**Author:** UX Research & Product Design Team
**Version:** 1.0

---

## 1. Navigation Map

### 1.1 Command Hierarchy

```
cuervo (default: chat)
├── chat [-p prompt] [--resume id] [--provider p] [--model m]
│   └── REPL Mode (interactive)
│       ├── /help, /h, /?           → Show categorized help
│       ├── /quit, /exit, /q        → Save session + exit
│       ├── /clear                  → Clear screen
│       ├── /model                  → Show current model
│       ├── /cost                   → Show session cost breakdown [NEW]
│       ├── /session
│       │   ├── list, ls            → List recent sessions
│       │   ├── show, info          → Show current session
│       │   └── name <title>        → Name current session [NEW]
│       ├── /test
│       │   ├── (empty), status     → Run all diagnostics
│       │   └── provider <name>     → Test specific provider
│       ├── /memory                 → [NEW] Memory commands from REPL
│       │   ├── list                → List memory entries
│       │   ├── search <query>      → Search memory
│       │   └── stats               → Memory statistics
│       └── /doctor                 → [NEW] Run doctor from REPL
│
├── config
│   ├── show                        → Display active config (TOML)
│   ├── get <key>                   → Get specific config value
│   ├── set <key> <value>           → Set config value [IMPLEMENT]
│   └── path                        → Show config file paths
│
├── init [--force]                  → Initialize project config
├── status                          → Show provider/model/security status
│
├── auth
│   ├── login <provider>            → Store API key in keychain
│   ├── logout <provider>           → Remove API key
│   └── status                      → Show key status for all providers
│
├── trace
│   ├── export <session_id>         → Export trace as JSON
│   └── replay <session_id>         → Replay a recorded session [MOVED]
│
├── memory
│   ├── list [--type t] [--limit n] → List entries
│   ├── search <query> [--limit n]  → FTS search
│   ├── prune [--dry-run]           → Remove old entries [ENHANCED]
│   └── stats                       → Memory statistics
│
├── doctor                          → Run health diagnostics
│
└── setup [NEW]                     → Interactive first-run wizard
```

### 1.2 State Machine: REPL Session

```
                    ┌──────────┐
                    │  LAUNCH  │
                    └────┬─────┘
                         │
                    ┌────▼─────┐     Config Error
                    │  CONFIG  ├─────────────────► EXIT (code 1)
                    │ VALIDATE │
                    └────┬─────┘
                         │ Valid
                    ┌────▼─────┐     No Key + First Run
                    │  CHECK   ├─────────────────► SETUP WIZARD
                    │   AUTH   │                       │
                    └────┬─────┘                       │
                         │ Has Key         ◄───────────┘
                    ┌────▼─────┐
                    │  CREATE/ │
                    │  RESUME  │
                    │ SESSION  │
                    └────┬─────┘
                         │
              ┌──────────▼──────────┐
              │    REPL LOOP        │
              │                     │
              │  ┌───────────────┐  │
              │  │ WAIT FOR      │  │
              │  │ USER INPUT    │◄─┤
              │  └───────┬───────┘  │
              │          │          │
              │  ┌───────▼───────┐  │
              │  │ CLASSIFY      │  │
              │  │ INPUT         │  │
              │  └───┬───┬───┬───┘  │
              │      │   │   │      │
              │  Slash│   │Text │    │
              │  Cmd  │   │Query│    │
              │      │   │   │      │
              │  ┌───▼─┐ │ ┌─▼───┐  │
              │  │HANDLE│ │ │AGENT│  │
              │  │ CMD  │ │ │LOOP │  │
              │  └──┬───┘ │ └──┬──┘  │
              │     │     │    │     │
              │     └─────┼────┘     │
              │           │          │
              │     ┌─────▼─────┐    │
              │     │ AUTO-SAVE │    │
              │     │ SESSION   │────┘
              │     └───────────┘
              │                     │
              └────────┬────────────┘
                       │ /quit or Ctrl+D
                  ┌────▼─────┐
                  │  SAVE &  │
                  │  EXIT    │
                  └──────────┘
```

### 1.3 State Machine: Agent Loop

```
              ┌──────────────┐
              │ RECEIVE      │
              │ USER QUERY   │
              └──────┬───────┘
                     │
              ┌──────▼───────┐
              │ ASSEMBLE     │──► Show: "Loading context [X/N]..."
              │ CONTEXT      │
              └──────┬───────┘
                     │
              ┌──────▼───────┐
              │ CHECK CACHE  │──► Cache Hit → Return cached response
              └──────┬───────┘
                     │ Cache Miss
              ┌──────▼───────┐
              │ MODEL        │──► Show: "Thinking... (Xs)"
              │ INFERENCE    │──► Stream tokens to terminal
              └──────┬───────┘
                     │
              ┌──────▼───────┐
              │ CLASSIFY     │
              │ RESPONSE     │
              └──┬────┬──────┘
                 │    │
            Text │    │ ToolUse
                 │    │
          ┌──────▼──┐ │ ┌────────▼───────┐
          │ DISPLAY │ │ │ PERMISSION     │
          │ + SAVE  │ │ │ CHECK          │
          └─────────┘ │ └────┬───┬───────┘
                      │      │   │
              Show footer:   │Allowed │Denied
        [tokens|cost|time]   │   │
                      │ ┌────▼───┘ ┌────▼────┐
                      │ │ EXECUTE  │ │ REPORT  │
                      │ │ TOOL     │ │ DENIED  │
                      │ └────┬─────┘ └────┬────┘
                      │      │            │
                      │      └────┬───────┘
                      │           │
                      │    ┌──────▼───────┐
                      │    │ CONTINUE?    │
                      │    │ (max rounds) │
                      │    └──┬────┬──────┘
                      │       │    │
                      │  More │    │ Done/Max
                      │       │    │
                      │    ┌──▼─┐  │
                      │    │NEXT│  │
                      │    │ROUND  │
                      └────┤    │  │
                           └────┘  │
                                   ▼
                            ┌──────────┐
                            │ COMPLETE │──► Show response footer
                            └──────────┘
```

---

## 2. UI State Tables

### 2.1 REPL Prompt States

| State | Display | Trigger |
|-------|---------|---------|
| Ready | `cuervo [model] > ` | Idle, awaiting input |
| Vi Normal | `cuervo [model] : ` | ESC in vi mode |
| Multiline | `... ` | Alt+Enter |
| History Search | `(search: 'term') > ` | Ctrl+R |
| History Search (failed) | `(failed) (search: 'term') > ` | No match found |

### 2.2 Inference States

| State | Display | Duration |
|-------|---------|----------|
| Idle | (nothing) | — |
| Context Loading | `Loading context [X/N]...` | 10-500ms |
| Cache Checking | (invisible, <1ms) | — |
| Thinking | `Thinking... (Xs)` | 500ms-30s |
| Streaming | (raw model text) | 1s-60s |
| Tool Request | Tool chrome appears | — |
| Complete | `[tokens \| cost \| time \| round]` | — |
| Interrupted | `[interrupted: round N]` | — |

### 2.3 Tool Execution States

| State | Display | Trigger |
|-------|---------|---------|
| Requested | `+-  tool(summary)` | Model requests tool |
| Preview | `\|   Description of action` | For destructive tools |
| Awaiting Permission | `\|   Allow? [y]es [n]o [a]lways [?]explain` | Permission needed |
| Executing | `\|   ...output...` | Permission granted |
| Success | `+- [OK] (Xs)` | Execution completed |
| Error | `+- [ERROR] message (Xs)` | Execution failed |
| Denied | `+- [DENIED]` | User denied permission |
| Timeout | `+- [TIMEOUT] after Xs` | Execution timed out |

### 2.4 Provider Connection States

| State | Display (status cmd) | Display (welcome) |
|-------|---------------------|-------------------|
| Connected | `provider: enabled, key set` | `Provider: name (connected)` |
| No Key | `provider: enabled, key missing` | `Warning: no API key...` |
| No Key Needed | `provider: enabled, no key needed` | `Provider: name (connected)` |
| Disabled | `provider: disabled` | (not shown) |
| Unhealthy | `provider: enabled, key set` | `Provider: name (degraded)` |

### 2.5 Session States

| State | Display | Condition |
|-------|---------|-----------|
| New | `Session: a1b2c3d4 (new)` | Fresh session created |
| Resumed | `Session: a1b2c3d4 (resumed, 12 messages)` | `--resume` used |
| Not Found | `Session x not found. Try /session list` | Invalid resume ID |
| Saving | (invisible) | Auto-save after each round |
| Save Failed | `Warning: failed to save session: reason` | DB error |

---

## 3. Annotated Implementation Specs

### 3.1 Spec: Inference Spinner

**Files to modify:**
- `crates/cuervo-cli/src/repl/mod.rs` — add spinner start/stop around agent loop call
- `crates/cuervo-cli/src/render/spinner.rs` (new) — spinner implementation

**Behavior:**
1. After user presses Enter, start spinner on stderr: `Thinking... (0.0s)`
2. Update elapsed time every 100ms
3. On first streaming chunk received, clear spinner line with `\r` + spaces + `\r`
4. If `--quiet` mode, skip spinner entirely
5. On Ctrl+C, stop spinner before printing `[interrupted]`

**Implementation notes:**
- Use `tokio::select!` with interval timer for elapsed updates
- Spinner runs on stderr (model output goes to stdout)
- Detect TTY: only show spinner when stderr is a terminal

### 3.2 Spec: Response Footer

**Files to modify:**
- `crates/cuervo-cli/src/repl/mod.rs` — add footer after agent loop completes

**Format:** `[{tokens} tokens | ${cost} | {latency}s | round {n}]`

**Behavior:**
1. After model response completes (streaming done), print footer
2. Footer in muted color (dark gray)
3. Tokens: sum of input + output tokens for this round
4. Cost: estimated cost for this round in USD, 4 decimal places
5. Latency: total round-trip time from query to last chunk
6. Round: current round number in agent loop
7. In `--verbose` mode: also show `cache: hit/miss`, input/output token split

### 3.3 Spec: Standardized Error Format

**Files to create:**
- `crates/cuervo-cli/src/render/error.rs` (new) — error formatting utilities

**Types:**
```rust
pub enum ErrorLevel {
    Error,   // Red prefix, may exit
    Warning, // Yellow prefix, continues
    Info,    // Blue prefix, informational
}

pub struct UserError {
    pub level: ErrorLevel,
    pub what: String,       // One-line summary
    pub why: Option<String>, // Explanation
    pub fix: Option<String>, // Recovery command or steps
    pub related: Option<String>, // Cross-reference
}

impl UserError {
    pub fn render(&self) -> String { ... }
}
```

**Migration:** Replace all `eprintln!("Error: ...")`, `println!("Config error ...")`, etc. with `UserError::new(...)`.render()`.

### 3.4 Spec: NO_COLOR Support

**Files to modify:**
- `crates/cuervo-cli/src/render/color.rs` (new) — centralized color control
- `crates/cuervo-cli/src/main.rs` — add `--color` flag
- All files using ANSI codes — route through color module

**Behavior:**
1. Check `NO_COLOR` env var (any value = no color)
2. Check `--color=never` flag
3. Check `TERM=dumb`
4. Check `!atty::is(Stream::Stderr)` for chrome
5. If any: disable all ANSI escape codes, use text-only fallbacks

### 3.5 Spec: Categorized /help

**Files to modify:**
- `crates/cuervo-cli/src/repl/commands.rs` — replace `print_help()` function

**New help output:**
```
Commands:

  Chat
    /help, /h, /?        Show this help
    /quit, /exit, /q     Exit cuervo
    /clear               Clear the screen
    /model               Show current model
    /cost                Show session cost breakdown

  Session
    /session list         List recent sessions
    /session show         Show current session
    /session name <t>     Name this session

  Memory
    /memory list          List memory entries
    /memory search <q>    Search memory
    /memory stats         Memory statistics

  Diagnostics
    /test                 Run self-diagnostics
    /test provider <n>    Test a provider
    /doctor               Run health check

  Shortcuts
    Ctrl+C               Cancel current request
    Ctrl+D               Exit
    Ctrl+R               Search history
    Alt+Enter            New line (multi-line input)
    Up/Down              Navigate history
```

### 3.6 Spec: Confirmation for Destructive Operations

**Files to modify:**
- `crates/cuervo-cli/src/commands/memory.rs` — add confirmation before prune

**Behavior:**
1. Before prune, count entries that would be pruned
2. Show: `Prune {N} memory entries? [y/N]: `
3. Default is No (uppercase N)
4. If `--yes` or `-y` flag: skip confirmation
5. If `--dry-run`: show count but don't delete
6. After prune: `Pruned {N} entries ({M} expired, {K} over limit).`

### 3.7 Spec: Tool Chrome Enhancement

**Files to modify:**
- `crates/cuervo-cli/src/render/tool.rs` — update rendering functions

**New format:**
```
  +-  bash(echo "hello")
  |   hello
  +- [OK] (0.1s)
```

**Changes:**
1. Replace `╭─`/`╰─` with `+-` (ASCII-safe by default, Unicode optional)
2. Add elapsed time to footer: `+- [OK] (Xs)`
3. Add description line for destructive tools
4. Permission prompt inline within tool chrome

### 3.8 Spec: First-Run Detection

**Files to modify:**
- `crates/cuervo-cli/src/commands/chat.rs` — detect first-run condition
- `crates/cuervo-cli/src/commands/setup.rs` (new) — wizard implementation

**Detection logic:**
1. No `~/.cuervo/config.toml` exists AND no API key in any keychain AND no env vars set
2. If first-run detected: launch wizard before REPL
3. Wizard stored as separate command: `cuervo setup` (also auto-triggered)
4. Wizard creates config, stores API key, tests connection

---

## 4. Exit Code Specification

| Code | Meaning | Example |
|------|---------|---------|
| 0 | Success | Normal exit, command completed |
| 1 | General error | Config error, provider failure |
| 2 | Usage error | Invalid arguments, unknown command |
| 3 | Auth error | No API key, auth failed |
| 4 | Provider error | All providers unavailable |
| 5 | Timeout | Request timed out |
| 130 | Interrupted | Ctrl+C (128 + SIGINT=2) |

---

## 5. Keyboard Shortcut Map

| Shortcut | Context | Action |
|----------|---------|--------|
| Enter | Prompt | Submit query |
| Alt+Enter | Prompt | New line (multi-line) |
| Ctrl+C | Prompt | Clear current input |
| Ctrl+C | Streaming | Interrupt model response |
| Ctrl+C | Tool execution | Cancel tool |
| Ctrl+D | Prompt (empty) | Exit REPL |
| Ctrl+R | Prompt | History search |
| Ctrl+W | Prompt | Delete word backward |
| Up/Down | Prompt | Navigate history |
| Tab | Prompt | Autocomplete (future) |

---

## 6. Responsive Design (Terminal Width)

| Width | Behavior |
|-------|----------|
| <40 cols | Minimal mode: no alignment, truncate long values |
| 40-79 cols | Compact mode: abbreviated headers, wrap at word boundaries |
| 80-120 cols | Standard mode: full formatting, aligned columns |
| >120 cols | Wide mode: same as standard (no extra use of width) |

---

*This specification serves as the contract between design and engineering. All implementations should reference this document.*
