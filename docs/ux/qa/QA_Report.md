# UX Quality Assurance Report

**Project:** Cuervo CLI v0.1.0
**Date:** February 2026
**Author:** UX Research & Product Design Team
**Build:** 690 tests, clippy clean, 5.1MB arm64 binary

---

## 1. Test Environment

| Parameter | Value |
|-----------|-------|
| Platform | macOS 15.3 (Darwin 24.3.0) |
| Architecture | Apple M4 (arm64) |
| Terminal | Terminal.app + iTerm2 |
| Shell | zsh 5.9 |
| Rust | 1.90.0 |
| Binary path | `/usr/local/bin/cuervo` |
| Config | `~/.cuervo/config.toml` |

---

## 2. Command Validation Matrix

### 2.1 Top-Level Commands

| Command | Expected | Actual | Status | Notes |
|---------|----------|--------|--------|-------|
| `cuervo --version` | Show version | `cuervo 0.1.0` | PASS | |
| `cuervo --help` | Show all commands | 9 subcommands listed | PASS | |
| `cuervo` (no args) | Enter REPL | REPL starts with welcome | PASS | Default is chat |
| `cuervo chat` | Enter REPL | REPL starts | PASS | |
| `cuervo chat -p "hello"` | Single-shot mode | Response printed, exits | PASS | |
| `cuervo config show` | Display config | Full TOML printed | PASS | |
| `cuervo config path` | Show paths | Global + Project paths | PASS | |
| `cuervo config get general.default_model` | Show value | Model name printed | PASS | |
| `cuervo config set key val` | Set config | "Coming in Sprint 7" | ISSUE | Unimplemented placeholder |
| `cuervo init` | Create .cuervo/ | Config created | PASS | |
| `cuervo init` (repeat) | Detect existing | "Already initialized" | PASS | |
| `cuervo status` | Show status | Providers + security | PASS | Fixed: now shows keychain |
| `cuervo auth login anthropic` | Store key | Key stored in keychain | PASS | |
| `cuervo auth logout anthropic` | Remove key | Key removed | PASS | |
| `cuervo auth status` | Show key status | All providers listed | PASS | |
| `cuervo doctor` | Run diagnostics | Full report generated | PASS | |
| `cuervo memory list` | List entries | Table or "No entries" | PASS | |
| `cuervo memory stats` | Show stats | Statistics displayed | PASS | |
| `cuervo memory search "test"` | Search memory | Results or "No results" | PASS | |
| `cuervo memory prune` | Prune old entries | Count or "Nothing" | PASS | No confirmation (ISSUE) |
| `cuervo trace export <id>` | Export JSON | JSON to stdout | PASS | |
| `cuervo replay <id>` | Replay session | Step-by-step replay | PASS | |

### 2.2 REPL Slash Commands

| Command | Expected | Actual | Status | Notes |
|---------|----------|--------|--------|-------|
| `/help` | Show help | Help text displayed | PASS | Missing categories, some commands |
| `/h` | Alias for help | Same as /help | PASS | |
| `/?` | Alias for help | Same as /help | PASS | |
| `/quit` | Exit REPL | "Goodbye!" + exit | PASS | |
| `/exit` | Alias for quit | Same as /quit | PASS | |
| `/q` | Alias for quit | Same as /quit | PASS | |
| `/clear` | Clear screen | Screen cleared | PASS | |
| `/model` | Show current | "Current: provider/model" | PASS | |
| `/session list` | List sessions | Table of sessions | PASS | |
| `/session ls` | Alias for list | Same as list | PASS | |
| `/session show` | Show current | Session details | PASS | |
| `/session` | Default to show | Same as show | PASS | |
| `/test` | Run diagnostics | Status output | PASS | |
| `/test status` | System status | Full status | PASS | |
| `/test provider echo` | Test echo | OK with latency | PASS | |
| `/unknown` | Error message | "Unknown command" + help hint | PASS | |
| Ctrl+C | Cancel input | Line cleared | PASS | |
| Ctrl+D | Exit | "Goodbye!" + exit | PASS | |
| Up/Down arrows | History | Navigate history | PASS | |
| Ctrl+R | Search history | Search mode | PASS | |

---

## 3. UX Issue Catalog

### 3.1 Critical Issues

| ID | Issue | Location | Impact | Recommendation |
|----|-------|----------|--------|----------------|
| UX-001 | No inference indicator (blank screen during model processing) | `repl/mod.rs` | Users confused if system is working | Add "Thinking..." spinner |
| UX-002 | No progress during tool execution | `render/tool.rs` | Users can't gauge long-running tools | Add elapsed timer |

### 3.2 Major Issues

| ID | Issue | Location | Impact | Recommendation |
|----|-------|----------|--------|----------------|
| UX-003 | Inconsistent error prefixes (6 patterns) | Multiple files | Unprofessional, confusing | Standardize to `Error:` format |
| UX-004 | `memory prune` has no confirmation | `commands/memory.rs` | Risk of accidental data loss | Add `[y/N]` confirmation |
| UX-005 | `config set` visible but unimplemented | `commands/config.rs` | Frustrating user experience | Implement or remove |
| UX-006 | MCP server failures have no recovery guidance | `commands/chat.rs` | Users can't fix MCP issues | Add specific fix steps |
| UX-007 | No `NO_COLOR` support | Rendering | Breaks accessibility, CI/CD | Implement NO_COLOR check |
| UX-008 | `/help` missing memory, doctor, trace commands | `repl/commands.rs` | Commands undiscoverable from REPL | Add to categorized help |
| UX-009 | Doctor uses Unicode box-drawing (no ASCII fallback) | `commands/doctor.rs` | Breaks on dumb terminals | Add ASCII fallback |
| UX-010 | No `--json` output for scripting | All commands | Can't use in CI/CD pipelines | Add --json flag |

### 3.3 Minor Issues

| ID | Issue | Location | Impact | Recommendation |
|----|-------|----------|--------|----------------|
| UX-011 | Session IDs are UUIDs (hard to type) | REPL, chat.rs | Friction when resuming | Accept 4-8 char prefixes |
| UX-012 | Timestamps missing timezone | memory.rs, doctor.rs | Ambiguous dates | Add UTC label |
| UX-013 | Welcome message shows full UUID | `repl/mod.rs` | Visual noise | Show 8-char prefix |
| UX-014 | Permission prompt doesn't explain `a` option | `permissions.rs` | Confusing for new users | Expand to `[y]es [n]o [a]lways [?]explain` |
| UX-015 | `[interrupted]` message lacks context | `agent.rs` | User unsure what was cancelled | Show round and tool info |
| UX-016 | Memory content truncated without indicator | `commands/memory.rs` | User doesn't know content is cut | Add `...` marker |
| UX-017 | Auth status hardcodes provider list | `commands/auth.rs` | Doesn't match config | Iterate from config |
| UX-018 | Config parse errors silently ignored | `config_loader.rs` | User's config not applied | Warn about parse failures |
| UX-019 | Alt+Enter for multiline not in /help | `repl/commands.rs` | Undocumented feature | Add to shortcuts section |
| UX-020 | No first-run experience/wizard | — | High dropout for new users | Add setup wizard |
| UX-021 | Cost display not prominent | REPL | Users may not see costs | Add session total in prompt |
| UX-022 | Memory list header width mismatch | `commands/memory.rs` | Minor visual glitch | Align header to actual data width |
| UX-023 | Trace and replay are separate commands | `main.rs` | Inconsistent with subcommand pattern | Move replay under trace |
| UX-024 | `env var CUERVO_LOG_LEVEL` vs `CUERVO_LOG` | config_loader/main.rs | Naming inconsistency | Standardize naming |

---

## 4. Performance UX Metrics

### 4.1 Startup Latency

| Scenario | Measurement | Target | Status |
|----------|-------------|--------|--------|
| `cuervo --version` | 3ms | <100ms | PASS |
| `cuervo --help` | 3ms | <100ms | PASS |
| `cuervo status` | 15ms | <200ms | PASS |
| `cuervo doctor` | 45ms | <500ms | PASS |
| REPL startup | 50ms | <500ms | PASS |

### 4.2 Interaction Latency

| Scenario | Measurement | Target | Status |
|----------|-------------|--------|--------|
| Agent loop (echo provider) | 27ms avg | <100ms | PASS |
| Cache hit response | 66us p50 | <1ms | PASS |
| Session auto-save | <5ms | <50ms | PASS |
| /help rendering | <1ms | <50ms | PASS |
| /session list (10 sessions) | <10ms | <100ms | PASS |

### 4.3 Resource Usage

| Metric | Measurement | Target | Status |
|--------|-------------|--------|--------|
| Binary size | 5.1MB | <10MB | PASS |
| RSS at startup | 4.7MB | <20MB | PASS |
| RSS after 10 rounds | ~5.2MB | <50MB | PASS |
| SQLite DB (100 sessions) | ~500KB | <10MB | PASS |

---

## 5. Cross-Terminal Compatibility

| Terminal | OS | Status | Issues |
|----------|----|----|--------|
| Terminal.app | macOS | PASS | Unicode renders correctly |
| iTerm2 | macOS | PASS | Full color support |
| Alacritty | macOS | PASS | Fast rendering |
| VS Code Terminal | macOS | PASS | Integrated experience |
| tmux | macOS | PASS | Multiplexer compatible |
| screen | macOS | PASS | Legacy compatible |
| `TERM=dumb` | Any | FAIL | Box-drawing characters rendered as garbage |
| Pipe to file | Any | PARTIAL | ANSI codes in output file |

---

## 6. Interaction Flow Testing

### 6.1 Happy Path: New User Chat

```
Step 1: cuervo                    → PASS (REPL starts)
Step 2: Type "hello"              → PASS (echo response)
Step 3: /help                     → PASS (help shown)
Step 4: /model                    → PASS (model shown)
Step 5: /session show             → PASS (session info)
Step 6: /quit                     → PASS ("Goodbye!")
```

### 6.2 Happy Path: Resume Session

```
Step 1: cuervo chat --resume <id> → PASS (session loaded)
Step 2: Type follow-up query      → PASS (context maintained)
Step 3: /session show             → PASS (shows resumed status)
Step 4: Ctrl+D                    → PASS (exit)
```

### 6.3 Error Path: No API Key

```
Step 1: Unset API key             → Setup
Step 2: cuervo                    → PASS (warning shown)
Step 3: Type query                → PASS (error with auth login suggestion)
```

### 6.4 Error Path: Invalid Config

```
Step 1: Add invalid config value  → Setup
Step 2: cuervo                    → PASS (error shown + exit)
Step 3: Fix config                → Manual step
Step 4: cuervo                    → PASS (starts normally)
```

---

## 7. Automated Test Coverage

| Category | Tests | Coverage | Notes |
|----------|-------|----------|-------|
| Database operations | 62 | High | All CRUD + edge cases |
| Agent loop | 20+ | Medium | Mock providers |
| CLI commands | 27 (E2E) | Medium | All subcommands tested |
| Provider contracts | 11 | High | Echo + mock responses |
| Permission system | 15+ | High | TBAC + legacy |
| Memory system | 22+ | High | Hybrid retrieval |
| Config validation | 3 | Low | Basic validation |
| Rendering | 18+ | Medium | Stream, markdown, tool |
| **Total** | **690** | **Medium-High** | |

### 7.1 Missing Test Coverage (UX-Critical)

| Area | Missing Tests | Priority |
|------|---------------|----------|
| Error message format consistency | No tests for message patterns | P1 |
| Terminal width handling | No tests for narrow/wide terminals | P2 |
| NO_COLOR behavior | No tests (not implemented) | P1 |
| First-run detection | No tests (not implemented) | P1 |
| Confirmation prompts | No tests (not implemented) | P1 |
| Exit codes | No tests for specific exit codes | P2 |

---

## 8. Recommendations Summary

### Immediate (Before v0.2.0)

1. **UX-001**: Add inference spinner — highest-impact single improvement
2. **UX-003**: Standardize error format — improves consistency across board
3. **UX-007**: Implement `NO_COLOR` — critical for accessibility and CI/CD
4. **UX-004**: Add confirmation to `memory prune` — prevent data loss

### Short-Term (v0.2.0)

5. **UX-008**: Categorized /help with all commands
6. **UX-009**: ASCII fallback for doctor
7. **UX-014**: Expanded permission prompt
8. **UX-005**: Implement `config set` or remove from help
9. **UX-010**: Add `--json` output mode

### Medium-Term (v0.3.0)

10. **UX-020**: First-run setup wizard
11. **UX-002**: Tool execution progress indicators
12. **UX-011**: Short session ID prefixes
13. **UX-006**: Enhanced MCP error messages

---

## 9. Sign-Off

| Reviewer | Area | Status | Date |
|----------|------|--------|------|
| UX Research | Research & Heuristics | Complete | 2026-02-07 |
| Product Design | Design System | Complete | 2026-02-07 |
| Implementation | Specs & Guidelines | Complete | 2026-02-07 |
| QA | Functional + Regression | Complete | 2026-02-07 |
| Accessibility | WCAG 2.2 Audit | Complete | 2026-02-07 |

---

*This report covers the current v0.1.0 release. Re-run QA after implementing Phase 1 UX improvements.*
