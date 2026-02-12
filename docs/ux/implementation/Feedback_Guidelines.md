# Feedback & Error Messaging Guidelines

**Project:** Cuervo CLI
**Date:** February 2026
**Author:** UX Research & Product Design Team
**Version:** 1.0

---

## 1. Tone of Voice

### 1.1 Principles

| Principle | Do | Don't |
|-----------|----|----|
| **Clear** | "Provider 'openai' not configured" | "Provider error occurred" |
| **Helpful** | "Run `cuervo auth login openai` to set up" | "Check your configuration" |
| **Respectful** | "No results found for 'search term'" | "Invalid search query" |
| **Concise** | "Pruned 5 entries (3 expired, 2 over limit)" | "The pruning operation has been completed successfully and removed a total of 5 entries from the memory database" |
| **Technical but accessible** | "Connection timed out after 30s" | "ETIMEDOUT: connect ECONNREFUSED" |

### 1.2 Voice Characteristics

| Context | Voice | Example |
|---------|-------|---------|
| Success | Confident, brief | "Key stored securely." |
| Error | Direct, helpful | "Error: provider not found.\n  To fix: cuervo auth login <provider>" |
| Warning | Informative, non-alarming | "Warning: MCP server 'x' unavailable. Some tools may not work." |
| Prompt | Clear, patient | "Allow bash [rm -rf build/]? [y]es [n]o [a]lways [?]explain" |
| Information | Factual, structured | "23 entries | 156 hits | 87% hit rate" |
| Goodbye | Friendly, brief | "Goodbye!" |

### 1.3 Language Rules

1. **Active voice**: "Cuervo pruned 5 entries" not "5 entries were pruned by the system"
2. **Second person for actions**: "Run `cuervo auth login`" not "The user should run..."
3. **Present tense for status**: "Provider is connected" not "Provider was connected"
4. **Avoid jargon**: "API service" over "provider" in user-facing text (technical name in parens)
5. **No exclamation marks** (except "Goodbye!")
6. **No emojis** in default output (configurable in future)

---

## 2. Error Message Framework

### 2.1 Structure

Every error follows this structure:

```
{Level}: {What happened}

  {Why it happened — one sentence explanation}

  To fix:
    {Actionable command or steps}

  Related: {cross-reference to relevant command}
```

### 2.2 Error Levels

| Level | Prefix | Color | Behavior | Example |
|-------|--------|-------|----------|---------|
| Error | `Error:` | Red | Blocks operation | Config invalid, provider unavailable |
| Warning | `Warning:` | Yellow | Continues with degradation | MCP server failed, slow provider |
| Info | `Note:` | Blue | Informational only | Cache hit, session resumed |

### 2.3 Error Categories

#### Category A: User-Fixable Errors

The user can resolve these with a specific action.

```
Error: No API key configured for 'anthropic'

  Cuervo checks for API keys in this order:
  1. Environment variable: $ANTHROPIC_API_KEY (not set)
  2. OS keychain: anthropic (not found)

  To fix:
    cuervo auth login anthropic

  Related: cuervo auth status (check all provider keys)
```

```
Error: Configuration value invalid

  In ~/.cuervo/config.toml:
  [models.providers.anthropic] timeout_ms = -1
  Value must be a positive integer (milliseconds).

  To fix:
    cuervo config set models.providers.anthropic.timeout_ms 30000

  Suggestion: 30000 (30 seconds) is a good default for Claude models.
```

#### Category B: Transient Errors

Temporary issues that may resolve on retry.

```
Error: Connection to 'anthropic' timed out after 30s

  The API server did not respond within the timeout period.
  This is usually a temporary network issue.

  To fix:
    Try again in a few seconds.
    If persistent, check: cuervo doctor

  Tip: Increase timeout with:
    cuervo config set models.providers.anthropic.timeout_ms 60000
```

```
Warning: Rate limited by 'anthropic', retry after 5s

  You've exceeded the API rate limit. Cuervo will automatically
  retry after the cooldown period.
```

#### Category C: System Errors

Internal issues requiring investigation.

```
Error: Database error: disk full

  Cuervo could not write to the database at ~/.cuervo/cuervo.db.
  This usually means your disk is full.

  To fix:
    Free disk space, then retry.
    To check database: cuervo doctor

  If the issue persists, please report it:
    https://github.com/cuervo-cli/cuervo/issues
```

#### Category D: Degraded Operation Warnings

The system continues but with reduced capability.

```
Warning: MCP server 'github' failed to start: connection refused

  Tools from the 'github' MCP server will not be available this session.
  Other tools and AI functionality are not affected.

  To fix:
    Check if the MCP server is running at localhost:3001
    Or disable it: [mcp.servers.github] enabled = false in config
```

```
Warning: Response cache unavailable (database not configured)

  Cuervo will work normally but responses won't be cached.
  This increases API costs for repeated queries.

  To fix:
    Ensure database path is set in config (default: ~/.cuervo/cuervo.db)
```

---

## 3. Success Message Patterns

### 3.1 Operation Confirmations

| Operation | Message |
|-----------|---------|
| Auth login | `API key for 'anthropic' stored in OS keychain.` |
| Auth logout | `API key for 'anthropic' removed from OS keychain.` |
| Init project | `Initialized Cuervo in .cuervo/\n  Config: .cuervo/config.toml` |
| Memory prune | `Pruned 5 entries (3 expired, 2 over limit).` |
| Session save | (silent — auto-save, only warn on failure) |
| Config set | `Set models.providers.anthropic.timeout_ms = 30000` |

### 3.2 Completion Summaries

| Context | Format |
|---------|--------|
| Agent response | `[423 tokens | $0.003 | 1.2s | round 3]` |
| Session exit | `Session saved. Goodbye!` |
| Trace export | `Exported 45 steps to stdout.` |
| Replay complete | `Replay complete (45 steps).` |
| Doctor clean | `All systems nominal. No issues detected.` |

---

## 4. Interactive Prompt Patterns

### 4.1 Confirmation Prompts

```
{Description of action}? [{default}]:
```

| Prompt | Default | Context |
|--------|---------|---------|
| `Prune 5 memory entries older than 30d? [y/N]:` | No | Destructive |
| `Save key to OS keychain? [Y/n]:` | Yes | Safe action |
| `Overwrite existing config? [y/N]:` | No | Destructive |
| `Resume session a1b2c3d4? [Y/n]:` | Yes | Safe action |

### 4.2 Permission Prompts

```
  +-  bash(rm -rf node_modules)
  |   Deletes node_modules/ directory
  |   Working dir: /Users/alex/project
  |
  |   Allow?  [y]es once  [n]o  [a]lways for bash  [?]explain
```

When user types `?`:
```
  |   Options:
  |     y - Allow this one time
  |     n - Deny (AI will be told the tool was denied)
  |     a - Allow all future 'bash' calls this session
  |
  |   Allow?  [y]es once  [n]o  [a]lways for bash  [?]explain
```

### 4.3 Selection Prompts (Setup Wizard)

```
? Select your AI provider:
  > Anthropic (Claude) — Best for code analysis and generation
    OpenAI (GPT-4) — General purpose, fast
    Ollama (Local) — No API key needed, private
```

Arrow keys to navigate, Enter to select.

---

## 5. Contextual Feedback

### 5.1 Startup Feedback

| Condition | Message |
|-----------|---------|
| Normal start | `cuervo v0.1.0 — AI-powered CLI for software development` |
| Resume session | `Resuming session a1b2c3d4 (12 messages loaded)` |
| Session not found | `Session a1b2c3d4 not found. Starting new session.\n  Tip: /session list to see available sessions` |
| No API key | `Warning: no API key for 'anthropic'.\n  Run: cuervo auth login anthropic` |
| Config warning | `Warning: [field] description\n  Suggestion: fix suggestion` |
| Config error | `Error: [field] description\n  Fix and retry, or: cuervo --skip-validation` |

### 5.2 During Operation

| State | Feedback |
|-------|----------|
| Context assembly | `Loading context [3/5 sources]...` |
| Cache hit | (verbose only) `Cache hit: returning stored response` |
| Model thinking | `Thinking... (2.1s)` |
| Streaming | Direct text output (no chrome) |
| Tool requested | Tool chrome with name and summary |
| Tool executing | Elapsed time in tool chrome |
| Round complete | `[tokens | cost | time | round]` |
| Round separator | `--- round 4 ---` |

### 5.3 Exit Feedback

| Condition | Message |
|-----------|---------|
| /quit | `Session saved. Goodbye!` |
| Ctrl+D | `Session saved. Goodbye!` |
| Ctrl+C during input | (clear line, return to prompt) |
| Ctrl+C during stream | `[interrupted: round 3]` |

---

## 6. Doctor Recommendations Style

### 6.1 Recommendation Categories

| Category | Prefix | Example |
|----------|--------|---------|
| Performance | `Performance:` | "Enable response cache to reduce API calls and cost" |
| Reliability | `Reliability:` | "Enable resilience layer for automatic failover" |
| Security | `Security:` | "PII detection is disabled. Enable for sensitive codebases." |
| Cost | `Cost:` | "Model 'opus' averages $0.05/query. Consider 'sonnet' for routine tasks." |
| Health | `Health:` | "Provider 'anthropic' has 75% success rate. Check API key and network." |

### 6.2 Recommendation Format

```
Recommendations:
  Performance: Enable response cache for faster repeated queries
    cuervo config set cache.enabled true

  Health: anthropic/opus has high latency (5.2s avg)
    Consider: cuervo config set models.providers.anthropic.timeout_ms 10000
```

---

## 7. Message Catalog (Reference)

### 7.1 Standard Messages

| ID | Level | Message | Context |
|----|-------|---------|---------|
| M001 | Info | `cuervo v{} — AI-powered CLI for software development` | Welcome |
| M002 | Info | `Provider: {} ({})  Model: {}  Session: {} ({})` | Welcome |
| M003 | Info | `Type /help for commands, /quit to exit.` | Welcome |
| M004 | Info | `Goodbye!` | Exit |
| M005 | Info | `Current: {}/{}` | /model command |
| M006 | Info | `Unknown command: /{}. Type /help for commands.` | Invalid command |
| M010 | Success | `API key for '{}' stored in OS keychain.` | Auth login |
| M011 | Success | `API key for '{}' removed from OS keychain.` | Auth logout |
| M012 | Success | `Initialized Cuervo in .cuervo/` | Init |
| M013 | Success | `Pruned {} entries.` | Memory prune |
| M020 | Warning | `Warning: no API key configured for '{}'` | Welcome |
| M021 | Warning | `Warning: MCP server '{}' failed to start: {}` | Chat setup |
| M022 | Warning | `Warning: failed to save session: {}` | Auto-save |
| M030 | Error | `Error: provider '{}' not configured` | Chat |
| M031 | Error | `Error: configuration has errors` | Startup |
| M032 | Error | `Error: unknown tool '{}'` | Tool execution |
| M033 | Error | `Error: tool '{}' timed out after {}s` | Tool timeout |
| M034 | Error | `Error: user denied permission` | Tool denied |

---

## 8. Anti-Patterns (Do Not)

| Anti-Pattern | Why | Instead |
|--------------|-----|---------|
| `Something went wrong` | Vague, unhelpful | Specific error with context |
| `Error: null` | Technical leak | Translate to user language |
| `FATAL ERROR!!!` | Alarmist | `Error: {description}` |
| `Operation completed successfully` | Verbose for simple ops | Brief confirmation or silence |
| `Please try again later` | Vague, no actionable step | Specific retry guidance |
| Printing stack traces | Overwhelming | Log to file, show summary |
| `Error: error: The error was:` | Redundant | Single clear message |
| Silent failure | User assumes success | Always acknowledge outcome |

---

*These guidelines apply to all user-facing text in the cuervo-cli crate. Library crates should return typed errors that the CLI translates using these patterns.*
