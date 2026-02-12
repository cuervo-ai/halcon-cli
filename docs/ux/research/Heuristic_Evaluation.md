# Heuristic Evaluation Report

**Project:** Cuervo CLI
**Date:** February 2026
**Author:** UX Research & Product Design Team
**Version:** 1.0
**Methodology:** Nielsen's 10 Usability Heuristics + Shneiderman's 8 Golden Rules + CLI-Specific Heuristics

---

## Executive Summary

This heuristic evaluation examines Cuervo CLI against 24 heuristics across three established frameworks. The evaluation is based on a comprehensive code audit of 28,451 LOC across 9 crates, cataloging 100+ user-facing messages and testing all 9 top-level commands.

**Overall Score: 58/100** (Moderate — functional but needs significant UX investment)

| Category | Score | Critical Issues |
|----------|-------|-----------------|
| Nielsen's 10 Heuristics | 56/100 | Visibility, Error Prevention, Help |
| Shneiderman's 8 Rules | 62/100 | Feedback, Reversal, Error Handling |
| CLI-Specific Heuristics | 55/100 | Scriptability, Progressive Disclosure |

**Severity Distribution:**
- Catastrophic (4): 2 issues
- Major (3): 8 issues
- Minor (2): 14 issues
- Cosmetic (1): 9 issues

---

## Part 1: Nielsen's 10 Usability Heuristics

### H1: Visibility of System Status (Score: 4/10)

The system should always keep users informed about what is going on, through appropriate feedback within reasonable time.

| # | Finding | Severity | Location |
|---|---------|----------|----------|
| H1.1 | No inference indicator — user sees blank screen for 2-5s during model processing | 4 (Catastrophic) | `repl/mod.rs` (agent loop) |
| H1.2 | No progress indicator during tool execution | 3 (Major) | `render/tool.rs:11` |
| H1.3 | Session auto-save is fire-and-forget — no confirmation or failure notification visible to user | 2 (Minor) | `repl/mod.rs:469` |
| H1.4 | Context assembly happens silently — user doesn't know which sources are being loaded | 2 (Minor) | `cuervo-context/assembler.rs` |
| H1.5 | No indication of remaining context window capacity | 2 (Minor) | `repl/mod.rs` |
| H1.6 | Cost display exists per-round but no running session total | 2 (Minor) | `repl/mod.rs` (after agent loop) |

**Evidence:** When a user types a query, the REPL returns to a blinking cursor with zero visual feedback until the first streaming token arrives. For complex queries requiring context assembly + model inference, this can be 3-5 seconds of silence.

**Recommendation:**
1. Add spinner with elapsed time: `Thinking... (2.1s)`
2. Show context loading: `Loading context [3/5 sources]...`
3. Show token/cost after each response: `[423 tokens | $0.003 | 1.2s]`

---

### H2: Match Between System and Real World (Score: 7/10)

The system should speak the users' language, with words, phrases and concepts familiar to the user.

| # | Finding | Severity | Location |
|---|---------|----------|----------|
| H2.1 | "Provider" is a developer abstraction — users think "Claude" or "GPT" not "anthropic provider" | 2 (Minor) | `commands/status.rs`, welcome message |
| H2.2 | "Session" semantics unclear — does resume load messages? memory? tool state? | 2 (Minor) | `commands/chat.rs:195` |
| H2.3 | `memory_entries` type names are database-oriented: `session_summary`, `project_meta` | 1 (Cosmetic) | `commands/memory.rs:27` |
| H2.4 | "Invocation metrics" in doctor is internal jargon | 1 (Cosmetic) | `commands/doctor.rs:145` |

**Strengths:**
- Good model name shortening in prompt (`claude-sonnet-4-5-20250929` → `sonnet`)
- Clear command verbs: `auth login`, `memory search`, `memory prune`
- Doctor uses plain-language recommendations

**Recommendation:** Add user-facing glossary for technical terms, use "model" over "provider" in user-facing output.

---

### H3: User Control and Freedom (Score: 6/10)

Users often choose system functions by mistake and will need a clearly marked "emergency exit" to leave the unwanted state.

| # | Finding | Severity | Location |
|---|---------|----------|----------|
| H3.1 | No undo for tool execution (file edits, bash commands) | 3 (Major) | `repl/executor.rs` |
| H3.2 | `memory prune` has no preview or confirmation — immediately deletes | 3 (Major) | `commands/memory.rs:89` |
| H3.3 | Ctrl+C during streaming shows only `[interrupted]` — no information about state | 2 (Minor) | `commands/chat.rs:286` |
| H3.4 | No way to cancel a tool execution in progress | 2 (Minor) | `repl/executor.rs` |

**Strengths:**
- Multiple exit options: `/quit`, `/exit`, `/q`, Ctrl+D
- Permission system gates destructive tool execution
- Stream cancellation via Ctrl+C works

**Recommendation:**
1. Add `memory prune --dry-run` to preview what would be pruned
2. Show operation context on interrupt: `[interrupted: round 3, tool: bash]`
3. Git integration for undo: show `git diff` after AI file edits

---

### H4: Consistency and Standards (Score: 5/10)

Users should not have to wonder whether different words, situations, or actions mean the same thing.

| # | Finding | Severity | Location |
|---|---------|----------|----------|
| H4.1 | Error prefix inconsistency: `"Error:"`, `"[ERROR]"`, `"Config error"`, `"WARN"`, `"Warning:"` | 3 (Major) | Multiple files |
| H4.2 | Timestamp format inconsistency: `%Y-%m-%d %H:%M` vs `%H:%M:%S` — no timezone | 2 (Minor) | `memory.rs`, `doctor.rs` |
| H4.3 | Output channel inconsistency: tool rendering to stderr, model output to stdout | 2 (Minor) | `render/tool.rs`, `render/stream.rs` |
| H4.4 | Auth status hardcodes providers `["anthropic", "openai", "ollama"]`, doesn't match config | 2 (Minor) | `commands/auth.rs:57` |
| H4.5 | `trace export` and `replay` are separate top-level commands (should be subcommands of trace) | 1 (Cosmetic) | `main.rs` |
| H4.6 | Config env vars: `CUERVO_LOG_LEVEL` vs `CUERVO_LOG` inconsistency | 2 (Minor) | `config_loader.rs`, `main.rs:30` |

**Evidence:**

Error format comparison across surfaces:
```
Auth:     eprintln!("Error reading key: {e}")     (raw eprintln)
Config:   println!("Config error [{}]: {}")        (bracketed field)
Doctor:   "│    [ERROR] {}: {}"                    (boxed + bracket)
REPL:     "Error: unknown tool '{}'"               (prefix colon)
Executor: "Error: tool '{}' timed out after {}s"   (prefix colon)
Stream:   "[error: {msg}]"                         (inline bracket)
```

**Recommendation:** Create a unified `UserMessage` type with standardized formatting:
```rust
enum MessageLevel { Error, Warning, Info, Success }
fn format_user_message(level: MessageLevel, what: &str, context: &str, fix: Option<&str>)
```

---

### H5: Error Prevention (Score: 5/10)

Even better than good error messages is a careful design which prevents a problem from occurring in the first place.

| # | Finding | Severity | Location |
|---|---------|----------|----------|
| H5.1 | Silent config file parse failures — invalid TOML ignored instead of warned | 3 (Major) | `config_loader.rs:18, 28` |
| H5.2 | `memory prune` has no confirmation step | 3 (Major) | `commands/memory.rs:89` |
| H5.3 | Empty API key input silently aborts login | 2 (Minor) | `commands/auth.rs:27` |
| H5.4 | `config set` command is visible but unimplemented | 2 (Minor) | `commands/config.rs:31` |
| H5.5 | Session ID format (UUID) not validated before database query | 1 (Cosmetic) | `commands/chat.rs` |

**Strengths:**
- Config validation catches errors before REPL startup
- Permission system prevents accidental destructive operations
- TBAC limits tool access scope

**Recommendation:**
1. Warn on config parse failures (don't silently ignore)
2. Add confirmation for destructive operations: `Prune 5 entries? [y/N]`
3. Hide unimplemented commands from help output

---

### H6: Recognition Rather Than Recall (Score: 6/10)

Minimize the user's memory load by making objects, actions, and options visible or easily retrievable.

| # | Finding | Severity | Location |
|---|---------|----------|----------|
| H6.1 | Session IDs are UUIDs — impossible to remember or type | 2 (Minor) | `repl/mod.rs:569` |
| H6.2 | Memory type names must be typed exactly: `fact`, `session_summary`, etc. | 2 (Minor) | `commands/memory.rs:27` |
| H6.3 | Top-level commands (`memory`, `doctor`, `trace`) not accessible from REPL slash commands | 2 (Minor) | `repl/commands.rs` |
| H6.4 | Config key paths must be known (no autocomplete or listing) | 1 (Cosmetic) | `commands/config.rs` |

**Strengths:**
- Short model names in prompt (`sonnet`, `opus`, `haiku`)
- `/help` available as `/h` or `/?`
- `/session show` displays all current session info

**Recommendation:**
1. Accept short session ID prefixes (4-8 chars, like git)
2. Add tab completion for slash commands, providers, types
3. Expose top-level commands as `/memory`, `/doctor`, `/trace` in REPL

---

### H7: Flexibility and Efficiency of Use (Score: 7/10)

Accelerators — unseen by the novice user — may often speed up the interaction for the expert user.

| # | Finding | Severity | Location |
|---|---------|----------|----------|
| H7.1 | No keyboard shortcuts beyond Ctrl+C/D/R and arrow keys | 2 (Minor) | `repl/commands.rs:102-106` |
| H7.2 | No aliases for common operations (e.g., quick resume) | 1 (Cosmetic) | `main.rs` |
| H7.3 | No `--json` output mode for scripting | 2 (Minor) | All commands |
| H7.4 | No `--quiet` mode to suppress decorative output | 2 (Minor) | All commands |

**Strengths:**
- Command aliases: `/quit`, `/exit`, `/q` all work
- Short flags: `-p` for prompt, `-m` for model
- Single-shot mode: `cuervo chat -p "question"` avoids REPL
- Config layering (global → project → env → flags) supports different workflows

**Recommendation:**
1. Add `--json` and `--quiet` flags for scriptability
2. Add quick-resume: `cuervo -r` or `cuervo --resume`
3. Support shell aliases in documentation

---

### H8: Aesthetic and Minimalist Design (Score: 7/10)

Dialogues should not contain information which is irrelevant or rarely needed.

| # | Finding | Severity | Location |
|---|---------|----------|----------|
| H8.1 | Welcome message shows session UUID — irrelevant to most users | 1 (Cosmetic) | `repl/mod.rs:294` |
| H8.2 | Doctor output has decorative Unicode box that adds visual noise | 1 (Cosmetic) | `commands/doctor.rs:20` |
| H8.3 | Memory list shows 36-char ID column header but only 8-char IDs | 1 (Cosmetic) | `commands/memory.rs:40` |

**Strengths:**
- Clean prompt design: `cuervo [sonnet] >`
- Tool output is compact: `╭─ name(args)` / `╰─ [OK] result`
- Minimal REPL chrome — focuses on conversation

**Recommendation:** Reduce welcome message to essentials; make session ID optional (--verbose).

---

### H9: Help Users Recognize, Diagnose, and Recover from Errors (Score: 4/10)

Error messages should be expressed in plain language (no codes), precisely indicate the problem, and constructively suggest a solution.

| # | Finding | Severity | Location |
|---|---------|----------|----------|
| H9.1 | MCP server failure messages provide no recovery guidance | 3 (Major) | `commands/chat.rs:84, 92, 102` |
| H9.2 | Config validation says "Fix them and retry" with no specific fix | 3 (Major) | `commands/chat.rs:155` |
| H9.3 | `"Session x not found, starting new session"` doesn't suggest `/session list` | 2 (Minor) | `commands/chat.rs:199` |
| H9.4 | Database errors surface raw technical messages | 2 (Minor) | `CuervoError::DatabaseError` |
| H9.5 | Tool timeout message doesn't suggest increasing timeout | 1 (Cosmetic) | `CuervoError::RequestTimeout` |

**Evidence:** Error messages that lack recovery guidance:
```
"Warning: MCP server 'github' failed to start: connection refused"
  → User doesn't know: Is server running? Is config wrong? How to fix?

"Configuration has errors. Fix them and retry."
  → User doesn't know: Which file? What's the correct value?

"[interrupted]"
  → User doesn't know: Is my session saved? Can I resume?
```

**Recommendation:** Every error should follow: `What happened → Why → How to fix`:
```
MCP server 'github' failed to connect: connection refused
  The MCP server at localhost:3001 is not responding.
  Fix: Check if the server is running, or disable it in config:
       [mcp.servers.github] enabled = false
```

---

### H10: Help and Documentation (Score: 5/10)

Even though it is better if the system can be used without documentation, it may be necessary to provide help and documentation.

| # | Finding | Severity | Location |
|---|---------|----------|----------|
| H10.1 | No contextual help — all commands show same /help text | 2 (Minor) | `repl/commands.rs:89` |
| H10.2 | No man page or `--help` examples with actual usage | 2 (Minor) | `main.rs` (clap) |
| H10.3 | No link to online documentation from CLI | 1 (Cosmetic) | All commands |
| H10.4 | `/help` doesn't mention memory, doctor, or trace commands | 2 (Minor) | `repl/commands.rs:92-107` |
| H10.5 | Tool permission prompt `(y/n/a)` doesn't explain what `a` means | 2 (Minor) | `permissions.rs:150` |

**Strengths:**
- `cuervo --help` is clear and well-structured (clap)
- `/help` in REPL is concise and scannable
- `/test` command provides diagnostic information
- `cuervo doctor` is a comprehensive diagnostic tool

**Recommendation:**
1. Add examples to `--help`: `cuervo chat -p "fix the tests" --model opus`
2. Expand `/help` with categories and all available commands
3. Add `/?` contextual help: `/?session` shows session-specific help
4. Link to docs: `For more info: https://cuervo.dev/docs`

---

## Part 2: Shneiderman's 8 Golden Rules

### S1: Strive for Consistency (Score: 5/10)

*See H4 findings above — same issues apply.*

Additional findings:

| # | Finding | Severity |
|---|---------|----------|
| S1.1 | Tool rendering uses `╭─`/`╰─` (Unicode) but doctor uses `│` | 1 (Cosmetic) |
| S1.2 | Some commands print to stdout, others to stderr, no clear pattern | 2 (Minor) |
| S1.3 | Date display varies: some UTC, some local, none labeled | 2 (Minor) |

---

### S2: Enable Frequent Users to Use Shortcuts (Score: 7/10)

**Strengths:**
- Command aliases everywhere (/q, /h, /?)
- Single-shot mode bypasses REPL
- Config layering allows expert customization

**Gaps:**
- No keyboard accelerators beyond basics
- No macro/alias system for repeated queries
- No `--resume` shorthand

---

### S3: Offer Informative Feedback (Score: 4/10)

*See H1 findings — this is the weakest area.*

| # | Finding | Severity |
|---|---------|----------|
| S3.1 | No inference indicator (blank screen during processing) | 4 (Catastrophic) |
| S3.2 | Tool execution shows no progress during run | 3 (Major) |
| S3.3 | Context loading happens invisibly | 2 (Minor) |
| S3.4 | Cache hit/miss not indicated to user | 1 (Cosmetic) |

---

### S4: Design Dialogs to Yield Closure (Score: 6/10)

| # | Finding | Severity |
|---|---------|----------|
| S4.1 | Memory prune gives count but no summary of what was pruned | 2 (Minor) |
| S4.2 | Session save has no visible confirmation | 2 (Minor) |
| S4.3 | Init command says "Initialized" but no next-step guidance | 2 (Minor) |
| S4.4 | Auth login confirms storage but not connection test | 2 (Minor) |

**Strengths:**
- "Goodbye!" on exit provides closure
- Doctor report ends with recommendations
- Replay ends with "Replay complete (N steps)"

---

### S5: Offer Simple Error Handling (Score: 5/10)

*See H5 and H9 — combined weaknesses in prevention and recovery.*

---

### S6: Permit Easy Reversal of Actions (Score: 4/10)

| # | Finding | Severity |
|---|---------|----------|
| S6.1 | No undo for file operations performed by tools | 3 (Major) |
| S6.2 | Memory prune is irreversible with no confirmation | 3 (Major) |
| S6.3 | Auth logout deletes key immediately | 2 (Minor) |
| S6.4 | No session delete with undo/trash concept | 1 (Cosmetic) |

**Recommendation:** For file operations, show `git diff` after changes. For destructive operations, add `--dry-run` and confirmation prompts.

---

### S7: Support Internal Locus of Control (Score: 7/10)

**Strengths:**
- User controls tool execution via permission system
- Config is explicit (no hidden magic)
- Model and provider selectable at runtime
- Single-shot mode for scripting

**Gaps:**
- Auto-compaction happens without user control
- MCP server failures continue silently
- Context sources loaded without visibility

---

### S8: Reduce Short-Term Memory Load (Score: 6/10)

*See H6 findings — UUID session IDs and exact type names increase cognitive load.*

---

## Part 3: CLI-Specific Heuristics

### C1: Respect Unix Philosophy (Score: 6/10)

| Principle | Status | Notes |
|-----------|--------|-------|
| Do one thing well | Partial | Chat + tools + memory + diagnostics in one binary |
| Compose with pipes | Poor | No `--json` output, no stdin pipe support |
| Text streams | Partial | Stdout for model output, stderr for chrome |
| Exit codes | Undocumented | Exit codes not documented for scripting |

---

### C2: Progressive Disclosure (Score: 4/10)

| # | Finding | Severity |
|---|---------|----------|
| C2.1 | All config options visible at once (no defaults-first approach) | 2 (Minor) |
| C2.2 | Doctor shows all 8 sections regardless of relevance | 2 (Minor) |
| C2.3 | No --verbose flag for detailed output (only CUERVO_LOG for tracing) | 2 (Minor) |
| C2.4 | No beginner/expert mode distinction | 2 (Minor) |

**Recommendation:** Show summary by default, details on `--verbose`. Doctor should highlight issues, not dump everything.

---

### C3: Scriptability (Score: 3/10)

| # | Finding | Severity |
|---|---------|----------|
| C3.1 | No `--json` output for any command | 3 (Major) |
| C3.2 | No `--quiet` mode to suppress interactive elements | 3 (Major) |
| C3.3 | ANSI colors not disabled in pipe/non-TTY context | 2 (Minor) |
| C3.4 | Exit codes not documented | 2 (Minor) |
| C3.5 | No `NO_COLOR` support | 2 (Minor) |

---

### C4: Predictable Behavior (Score: 7/10)

**Strengths:**
- Config layering is predictable (global < project < env < flags)
- Permission system is consistent
- Session persistence is automatic and reliable

**Gaps:**
- MCP failures silently degrade capabilities
- Config parse failures silently ignored
- Compaction may alter session state unexpectedly

---

### C5: Graceful Degradation (Score: 6/10)

**Strengths:**
- Circuit breaker prevents cascading failures
- Fallback providers on primary failure
- Backpressure prevents overload
- Cache serves when providers are down

**Gaps:**
- MCP server failure messages are warnings (continues without tools)
- No explicit "degraded mode" indicator
- User doesn't know which capabilities are unavailable

---

### C6: Discoverability (Score: 5/10)

| # | Finding | Severity |
|---|---------|----------|
| C6.1 | No tab completion for any commands or arguments | 3 (Major) |
| C6.2 | REPL /help doesn't mention CLI-level commands (memory, doctor) | 2 (Minor) |
| C6.3 | No suggested commands on empty input or after errors | 2 (Minor) |
| C6.4 | No `cuervo commands` to list everything | 1 (Cosmetic) |

---

## Severity Summary

### Catastrophic Issues (Must Fix)

1. **No inference indicator** (H1.1/S3.1) — Users see blank screen during 2-5s processing. Causes confusion, premature cancellation, perceived crashes. Fix: Add "Thinking..." spinner.

2. **No progress during tool execution** (H1.2/S3.2) — Long-running tools (bash, file operations) show no feedback. Fix: Add elapsed timer to tool chrome.

### Major Issues (Should Fix)

1. **Inconsistent error formatting** (H4.1) — 6 different error prefix patterns across codebase
2. **No undo for tool operations** (H3.1/S6.1) — File changes by tools are irreversible
3. **Memory prune has no confirmation** (H3.2/S6.2) — Destructive operation without preview
4. **Silent config parse failures** (H5.1) — Invalid TOML files silently ignored
5. **MCP failure messages lack recovery** (H9.1) — Warnings without actionable guidance
6. **Config validation lacks specific fixes** (H9.2) — "Fix them and retry" is not helpful
7. **No scriptable output** (C3.1/C3.2) — Missing `--json` and `--quiet` modes
8. **No tab completion** (C6.1) — Critical for discoverability

### Heuristic Score Summary

| Heuristic | Score | Rating |
|-----------|-------|--------|
| H1: Visibility of System Status | 4/10 | Poor |
| H2: Match System & Real World | 7/10 | Good |
| H3: User Control & Freedom | 6/10 | Fair |
| H4: Consistency & Standards | 5/10 | Fair |
| H5: Error Prevention | 5/10 | Fair |
| H6: Recognition vs. Recall | 6/10 | Fair |
| H7: Flexibility & Efficiency | 7/10 | Good |
| H8: Aesthetic & Minimal Design | 7/10 | Good |
| H9: Error Recognition & Recovery | 4/10 | Poor |
| H10: Help & Documentation | 5/10 | Fair |
| **Nielsen Average** | **5.6/10** | **Fair** |
| S1-S8: Shneiderman Average | 5.5/10 | Fair |
| C1-C6: CLI-Specific Average | 5.2/10 | Fair |
| **Overall Weighted Score** | **5.4/10** | **Fair** |

---

## Prioritized Action Plan

### Phase 1: Critical Fixes (Week 1-2)

| # | Action | Addresses | Effort |
|---|--------|-----------|--------|
| 1 | Add inference spinner ("Thinking... Xs") | H1.1, S3.1 | Small |
| 2 | Add tool execution elapsed timer | H1.2, S3.2 | Small |
| 3 | Standardize error message format | H4.1, S1 | Medium |
| 4 | Add `NO_COLOR` / `--no-color` support | C3.5 | Small |

### Phase 2: Major Improvements (Week 3-6)

| # | Action | Addresses | Effort |
|---|--------|-----------|--------|
| 5 | Structured error recovery messages | H9.1, H9.2, S5 | Medium |
| 6 | Confirmation for destructive ops | H3.2, H5.2, S6.2 | Small |
| 7 | Warn on config parse failures | H5.1 | Small |
| 8 | Tab completion for slash commands | C6.1, H6.2, H10 | Medium |
| 9 | Categorized /help with all commands | H10.4, C6.2 | Small |
| 10 | `--json` output mode for data commands | C3.1, H7.3 | Medium |

### Phase 3: Enhancement (Week 7-12)

| # | Action | Addresses | Effort |
|---|--------|-----------|--------|
| 11 | Interactive first-run setup wizard | H10, S4 | Large |
| 12 | ASCII fallback for doctor output | C5, S1 | Small |
| 13 | Round separators with metadata | H1.5, S3 | Small |
| 14 | Session naming and short IDs | H6.1, S8 | Medium |
| 15 | Contextual help system | H10.1, C2.4, C6.3 | Large |

---

*Next evaluation scheduled after Phase 1 implementation to measure improvement.*
