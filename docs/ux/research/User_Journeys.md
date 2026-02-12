# User Journey Maps & Empathy Maps

**Project:** Cuervo CLI
**Date:** February 2026
**Author:** UX Research & Product Design Team
**Version:** 1.0

---

## 1. User Personas

### 1.1 Persona: Alex — The Senior Backend Engineer

| Attribute | Detail |
|-----------|--------|
| **Role** | Senior Software Engineer, 8 years experience |
| **Tech Stack** | Rust, Go, Python; Linux/macOS terminal daily |
| **AI Usage** | Uses ChatGPT/Claude web UI daily, wants CLI integration |
| **Goals** | Code faster, automate repetitive tasks, reduce context switching |
| **Pain Points** | Web UI breaks flow; copy-paste between browser and terminal is slow |
| **Terminal Skill** | Expert — uses tmux, vim, custom shell scripts |
| **Configuration Tolerance** | High — prefers explicit config over magic |
| **Trust Level** | Cautious — wants to review before AI executes anything |

**Empathy Map:**

```
         THINKS                          FEELS
  "I need to stay in           Frustrated by context-
   my terminal flow"            switching to browser
  "I want control over         Cautious about AI running
   what AI executes"            commands on my system
  "Config should be            Excited about productivity
   version-controlled"          gains from AI assistance

         SAYS                            DOES
  "Just give me the            Reads man pages before
   code, I'll review it"       using new tools
  "I don't trust AI to         Checks git diff after
   run rm -rf"                  every AI edit
  "Show me what you're         Uses --verbose flag on
   going to do first"          everything
```

---

### 1.2 Persona: Maya — The Full-Stack Developer

| Attribute | Detail |
|-----------|--------|
| **Role** | Mid-level full-stack dev, 4 years experience |
| **Tech Stack** | TypeScript, React, Node.js; VS Code primary |
| **AI Usage** | GitHub Copilot in IDE, occasional ChatGPT |
| **Goals** | Learn new codebases faster, get unstuck on bugs |
| **Pain Points** | Intimidated by complex CLI tools; prefers GUI |
| **Terminal Skill** | Intermediate — uses basic commands, git, npm |
| **Configuration Tolerance** | Low — wants sensible defaults |
| **Trust Level** | Moderate — willing to let AI help if clearly explained |

**Empathy Map:**

```
         THINKS                          FEELS
  "I wish there was a          Overwhelmed by tools
   tutorial for this"           with too many options
  "What does this config       Anxious when CLI shows
   option even do?"             cryptic error messages
  "I just want it to           Relieved when things
   work out of the box"         just work

         SAYS                            DOES
  "How do I set this up?"      Googles error messages
  "What command do I           Tries --help first,
   use for X?"                  then Stack Overflow
  "Can you explain what        Copies example commands
   that error means?"           from documentation
```

---

### 1.3 Persona: Jordan — The DevOps Engineer

| Attribute | Detail |
|-----------|--------|
| **Role** | DevOps/Platform Engineer, 6 years experience |
| **Tech Stack** | Terraform, Docker, K8s, Python; terminal-first |
| **AI Usage** | AI for script generation, IaC troubleshooting |
| **Goals** | Automate infrastructure tasks, generate configs, debug systems |
| **Pain Points** | Needs scriptable output; hates tools that only work interactively |
| **Terminal Skill** | Expert — pipes, redirections, cron, CI/CD |
| **Configuration Tolerance** | High — wants env vars and machine-readable output |
| **Trust Level** | Variable — trusts AI for suggestions, reviews for execution |

**Empathy Map:**

```
         THINKS                          FEELS
  "I need to pipe this         Frustrated when tools
   output to jq"                require interaction
  "Can I use this in           Annoyed by colored
   my CI pipeline?"             output that breaks logs
  "Cost tracking matters       Satisfied by precise,
   for my team budget"          scriptable interfaces

         SAYS                            DOES
  "Does it support             Wraps tools in bash
   --json output?"              scripts for automation
  "How much does this          Monitors API costs
   cost per query?"             closely
  "Can I disable the           Sets NO_COLOR in
   spinner in scripts?"         all pipelines
```

---

### 1.4 Persona: Sam — The Student/Learner

| Attribute | Detail |
|-----------|--------|
| **Role** | CS student, 1 year coding experience |
| **Tech Stack** | Python, JavaScript; learning Rust |
| **AI Usage** | Heavy ChatGPT user for learning, code explanations |
| **Goals** | Learn to code better, understand existing codebases, get help with homework |
| **Pain Points** | Doesn't understand technical jargon; needs clear explanations |
| **Terminal Skill** | Beginner — knows `cd`, `ls`, `git clone` |
| **Configuration Tolerance** | None — wants it to just work |
| **Trust Level** | High — trusts AI output, doesn't always verify |

**Empathy Map:**

```
         THINKS                          FEELS
  "What does 'provider'        Confused by config
   mean in this context?"       files and TOML syntax
  "I just want to ask          Intimidated by error
   a coding question"           messages with stack traces
  "How do I install this?"     Excited when AI
                                explains things clearly

         SAYS                            DOES
  "It's not working,           Copies entire error
   help!"                       messages to Google
  "What's a REPL?"             Follows YouTube
                                tutorials step by step
  "How do I exit this?"        Presses Ctrl+C
                                repeatedly when stuck
```

---

## 2. User Journey Maps

### 2.1 Journey: First-Time Setup (All Personas)

```
STAGE        DISCOVER         INSTALL          CONFIGURE        FIRST USE        EVALUATE
             ─────────────    ─────────────    ─────────────    ─────────────    ─────────────

ACTIONS      Search for AI    cargo install    Get API key      Run first        Assess if
             CLI tools        cuervo-cli       from provider    query            worth keeping

TOUCHPOINTS  GitHub/crates.io Terminal         Provider         REPL             REPL output
             README                            website, REPL

THINKING     "Will this work  "How long will   "Where do I      "What should     "Is this better
             for my use       this take?"      put my key?"     I type?"         than ChatGPT?"
             case?"

FEELING      Curious          Neutral          Confused         Uncertain        Critical
             ●●●○○            ●●○○○            ●○○○○            ●●○○○            ●●●●○

PAIN POINTS  - No demo/video  - Build time     - No setup       - Blank REPL     - No comparison
             - README lacks     for Rust         wizard           is daunting      baseline
               quick example  - Binary size    - config.toml    - No example     - Unclear what
                               not clear         syntax           queries          it can do
                                                 unclear                           beyond chat

OPPORTUN.    Quick-start      Pre-built        Interactive      Example          Success metrics
             video/GIF in     binaries for     `cuervo setup`   commands on      after first
             README           major platforms  wizard           first launch     session
```

#### Journey Metrics (Current)
| Metric | Alex | Maya | Jordan | Sam |
|--------|------|------|--------|-----|
| Time to install | 3 min | 5 min | 2 min | 10 min |
| Time to configure | 2 min | 15 min | 3 min | 30+ min |
| Time to first value | 5 min | 20 min | 5 min | 45+ min |
| Dropout risk | Low | High | Low | Very High |
| Satisfaction | 6/10 | 3/10 | 7/10 | 2/10 |

---

### 2.2 Journey: Daily Coding Session (Alex — Senior Engineer)

```
STAGE        LAUNCH           CONTEXT          QUERY            TOOL USE         WRAP UP
             ─────────────    ─────────────    ─────────────    ─────────────    ─────────────

ACTIONS      Open terminal    Resume session   Ask coding       AI runs tools    Save/exit
             Run cuervo       Set context      question         Review results   session

TOUCHPOINTS  Terminal,        REPL, session    REPL prompt      Tool output,     /quit,
             shell alias      resume                            permission       auto-save

THINKING     "Continue        "Does it         "Will it         "What is it      "Did it save
             where I left     remember our     understand my    running on my    my session?"
             off"             context?"        codebase?"       system?"

FEELING      Efficient        Curious          Engaged          Cautious         Satisfied
             ●●●●○            ●●●○○            ●●●●○            ●●○○○            ●●●○○

PAIN POINTS  - No quick       - Resume loads   - No codebase    - No preview     - No session
               resume alias     messages only    indexing         before exec      summary
             - Startup shows  - Memory          (just memory)  - Permission      - No cost
               too much info    context not    - Context          prompt is        total for
                                visible          window not       y/n only         session
                                                 shown

OPPORTUN.    `cuervo -r`      Show "loaded     Codebase        Tool preview     Session summary
             quick-resume     3 memories,      indexing +       with diff        with total
             last session     12 messages"     RAG              before exec      cost/tokens
```

---

### 2.3 Journey: Debugging a Production Issue (Maya — Full-Stack Dev)

```
STAGE        PANIC            SEEK HELP        DESCRIBE BUG     ITERATE          RESOLVE
             ─────────────    ─────────────    ─────────────    ─────────────    ─────────────

ACTIONS      See error in     Launch cuervo    Paste error +    Follow AI        Apply fix,
             production       or ask in REPL   describe          suggestions      verify

TOUCHPOINTS  Logs, terminal   Terminal         REPL prompt      Tool output,     Git, deploy
                                                                file edits

THINKING     "What broke?     "Can AI help     "I hope it       "Wait, what      "Did that
             I need help      me debug          understands      file is it       actually
             fast"            this?"           the stack trace"  editing?"        fix it?"

FEELING      Anxious          Hopeful          Uncertain        Cautious         Relieved
             ●○○○○            ●●●○○            ●●○○○            ●●●○○            ●●●●○

PAIN POINTS  - Long error     - No "debug      - Long pastes    - File edits     - No way to
               messages get     mode" or         truncated in     happen without   verify fix
               truncated        context          context          showing diff     before deploy
             - No error       - Model starts  - Loses context  - No undo for    - No "test
               pattern          from scratch     on large         tool actions      this fix"
               recognition                       codebases                         command

OPPORTUN.    Error pattern    Debug-focused    Smart error      Show diffs       Integrated
             recognition +    session mode     parsing +        before applying  test runner
             auto-categorize  with log         context from     changes          + deploy
                              analysis         git blame                          verification
```

---

### 2.4 Journey: Scripting & Automation (Jordan — DevOps)

```
STAGE        SETUP            SCRIPT           EXECUTE          PARSE            INTEGRATE
             ─────────────    ─────────────    ─────────────    ─────────────    ─────────────

ACTIONS      Configure env    Write script     Run cuervo       Parse output     Add to CI/CD
             vars             using cuervo     in pipeline      for automation   pipeline

TOUCHPOINTS  .env, config     Shell script     CI runner        jq, grep,        Dockerfile,
                                                                awk              GitHub Actions

THINKING     "Can I use       "Does it         "Will this       "How do I        "Is this
             env vars for     support          work non-        get just the     reliable
             everything?"     single-shot?"    interactively?"  code output?"    enough?"

FEELING      Hopeful          Frustrated       Anxious          Very Frustrated  Cautious
             ●●●○○            ●●○○○            ●●○○○            ●○○○○            ●●○○○

PAIN POINTS  - Config needs   - No --json      - REPL prompt    - No structured  - No retry
               file, not just   output mode      blocks           output mode      logic built
               env vars       - No --quiet       pipeline       - ANSI codes       in
             - API key via      mode           - Spinner/         pollute logs   - Cost unpre-
               keychain not   - Exit codes       color breaks   - No exit          dictable
               CI-friendly      not documented   CI logs          code docs

OPPORTUN.    Env-only mode    --json, --quiet  Non-interactive  Machine-readable Exit code
             (no config file  flags for all    mode with stdin  output formats   docs + retry
             required)        commands         pipe support     (JSON, CSV)      strategies
```

---

### 2.5 Journey: Learning a New Codebase (Sam — Student)

```
STAGE        DISCOVER         SETUP            EXPLORE          LEARN            APPLY
             ─────────────    ─────────────    ─────────────    ─────────────    ─────────────

ACTIONS      Clone repo       Install cuervo   Ask "what does   Follow AI        Make first
             to learn         in project       this do?"        explanations     contribution

TOUCHPOINTS  GitHub, git      Terminal         REPL prompt      REPL output      Git, editor

THINKING     "This codebase   "Why is setup    "How does the    "That makes      "Did I
             is huge, where   so complicated?" auth module       sense now!"      understand
             do I start?"                       work?"                            it correctly?"

FEELING      Overwhelmed      Frustrated       Curious          Enlightened      Proud
             ●○○○○            ●○○○○            ●●●○○            ●●●●○            ●●●●●

PAIN POINTS  - No "explore    - Config jargon  - AI can't see   - Explanations   - No way to
               this repo"       is confusing     the full         reference files   verify
               command        - Error messages   codebase         user hasn't      understanding
             - No suggested     assume CLI     - "Memory" and     opened        - No quiz or
               first steps      expertise        "context" are  - No code          challenge
                                                 abstract         navigation       mode

OPPORTUN.    `cuervo explore` Simplified       Codebase         Interactive      Learning mode
             auto-indexes     setup for        indexing with    explanation      with exercises
             and summarizes   beginners        file navigation  with links       and checkpoints
```

---

## 3. Critical Moments of Truth

### 3.1 Moment: The Blank REPL (All Personas)

**Trigger:** User launches `cuervo` for the first time and sees an empty prompt.

**Current Experience:**
```
cuervo v0.1.0 — AI-powered CLI for software development
Provider: anthropic (connected)  Model: claude-sonnet-4-5  Session: a1b2c3d4 (new)
Type /help for commands, /quit to exit.

cuervo [sonnet] >
```

**User Thought:** "What do I type? What can it do?"

**Impact:** High dropout risk for Maya and Sam personas.

**Recommended Fix:**
```
cuervo v0.1.0 — AI-powered CLI for software development
Provider: anthropic (connected)  Model: claude-sonnet-4-5  Session: a1b2c3d4 (new)

Try these to get started:
  "Explain this codebase"          Analyze the current project
  "Fix the failing tests"          Debug and fix test failures
  "Write a function that..."       Generate code from description

Type /help for all commands, /quit to exit.

cuervo [sonnet] >
```

---

### 3.2 Moment: The Cryptic Error (Maya, Sam)

**Trigger:** Configuration error blocks startup.

**Current Experience:**
```
Config error [models.providers.anthropic.timeout_ms]: value must be > 0
Configuration has errors. Fix them and retry.
```

**User Thought:** "Where is this config file? What should I set it to?"

**Impact:** Complete blocker — user cannot use the tool.

**Recommended Fix:**
```
Config error in ~/.cuervo/config.toml:
  [models.providers.anthropic] timeout_ms must be > 0 (current: -1)
  Suggestion: Set timeout_ms = 30000 (30 seconds, recommended default)

  To fix:  cuervo config set models.providers.anthropic.timeout_ms 30000
  To skip: cuervo chat --skip-validation
```

---

### 3.3 Moment: The Silent Wait (All Personas)

**Trigger:** User sends a query and sees nothing for 2-5 seconds while the model processes.

**Current Experience:**
```
cuervo [sonnet] > Explain the authentication module in this project
_
```
(Blank screen, cursor blinking, no feedback)

**User Thought:** "Did it crash? Is it working? Should I wait or cancel?"

**Impact:** Perceived performance issue; users may interrupt valid requests.

**Recommended Fix:**
```
cuervo [sonnet] > Explain the authentication module in this project
Thinking... (2.1s)
```
Then when streaming begins:
```
cuervo [sonnet] > Explain the authentication module in this project
The authentication module is located in...
```

---

### 3.4 Moment: The Permission Question (Alex, Maya)

**Trigger:** AI wants to run a command and asks for permission.

**Current Experience:**
```
  ╭─ bash(rm -rf node_modules && npm install)

Allow bash [rm -rf node_modules && npm install]? (y/n/a):
```

**User Thought (Alex):** "Wait, what exactly will this delete? Is it safe?"
**User Thought (Maya):** "What does 'a' mean? Allow always? For what commands?"

**Impact:** Breaks trust if user doesn't understand the options.

**Recommended Fix:**
```
  ╭─ bash(rm -rf node_modules && npm install)
  │  Deletes node_modules/ directory and reinstalls all dependencies
  │  Working directory: /Users/alex/project
  │
  │  Allow?  [y]es once  [n]o  [a]lways for this tool  [?] explain
```

---

### 3.5 Moment: The Cost Surprise (Jordan)

**Trigger:** User runs several queries and realizes significant API cost.

**Current Experience:**
- Cost shown only in `cuervo doctor` under Metrics section
- Per-round cost shown during session but easy to miss
- No running total visible

**User Thought:** "I've been running queries for an hour. How much did this cost?"

**Recommended Fix:**
```
cuervo [sonnet] > [Round 5 | $0.42 session total | 12.3k tokens]
```
Plus: `/cost` slash command showing detailed breakdown.

---

## 4. Journey-Based Requirements Matrix

| Requirement | Alex | Maya | Jordan | Sam | Priority |
|-------------|------|------|--------|-----|----------|
| First-run setup wizard | Nice | Critical | Nice | Critical | P0 |
| Example commands on empty REPL | Nice | Critical | Low | Critical | P0 |
| Inference spinner/indicator | Nice | Critical | Critical* | Critical | P0 |
| Structured error messages | Nice | Critical | Nice | Critical | P0 |
| `--json` output mode | Low | Low | Critical | Low | P1 |
| `NO_COLOR` support | Low | Low | Critical | Low | P1 |
| Session naming | Nice | Nice | Low | Low | P1 |
| Tab completion | Nice | Critical | Nice | Critical | P1 |
| Tool execution preview | Critical | Critical | Nice | Nice | P1 |
| Cost tracking display | Nice | Nice | Critical | Low | P1 |
| Codebase indexing | Nice | Nice | Nice | Critical | P2 |
| Learning mode | Low | Nice | Low | Critical | P2 |
| Non-interactive/pipe mode | Low | Low | Critical | Low | P2 |

*Jordan needs --quiet mode to suppress spinner in scripts

---

## 5. Emotional Journey Summary

```
SATISFACTION
    10 ─┐
       │
     8 ─┤                                    ╭─ Alex (daily use)
       │                              ╭─────╯
     6 ─┤                    ╭───────╯
       │              ╭─────╯              ╭─ Jordan (scripting)
     4 ─┤        ╭───╯                ╭───╯
       │   ╭────╯                ╭───╯
     2 ─┤──╯                ╭───╯          ╭─ Maya (debugging)
       │              ╭────╯          ╭───╯
     0 ─┤─────────────╯          ╭───╯
       │                    ╭───╯          ╭─ Sam (learning)
    -2 ─┤──────────────────╯          ╭───╯
       │                         ╭───╯
    -4 ─┤────────────────────────╯
       │
       └──┬──────┬──────┬──────┬──────┬──────
        Install  Config  First  Daily  Advanced
                         Query  Use    Features
```

**Key Insight:** The biggest satisfaction gap is between Install and Config for all personas. This is the critical friction point where most potential users drop off. An interactive setup wizard would flatten this curve significantly.

---

## 6. Recommended User Journey (Target State)

### First-Time Experience (Target: <60 seconds to first value)

```
$ cargo install cuervo-cli    # or: brew install cuervo

$ cuervo
Welcome to Cuervo! Let's get you set up.

? Select your AI provider:
  > Anthropic (Claude)
    OpenAI (GPT-4)
    Ollama (Local)

? Enter your Anthropic API key: sk-ant-api03-****
  Testing connection... Connected! (claude-sonnet-4-5)

? Save key to OS keychain? (recommended) [Y/n]: Y
  Key stored securely.

Ready! Here's what you can do:
  Ask a question:  "How does authentication work in this project?"
  Run a command:   "Run the tests and fix any failures"
  Get help:        /help

cuervo [sonnet] >
```

---

*This document should be updated as new user research data becomes available.*
