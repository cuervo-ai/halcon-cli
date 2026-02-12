# UX Metrics Dashboard & Report

**Project:** Cuervo CLI v0.1.0
**Date:** February 2026
**Author:** UX Research & Product Design Team

---

## 1. KPI Dashboard — Current State

### 1.1 Primary Metrics

| Metric | Current | Target (3mo) | Target (6mo) | Method |
|--------|---------|--------------|--------------|--------|
| **SUS Score** | 57.5 | 75 | 82+ | Expert evaluation → user survey |
| **Task Success Rate** | 62.5% | 85% | 91% | Scenario testing |
| **Error Rate** | 17.5% | 8% | <5% | Error message catalog audit |
| **Time-to-First-Value** | 25-45 min | 3 min | <60s | Setup flow timing |
| **NPS** | Unknown | 30+ | 50+ | User survey (post-implementation) |
| **CSAT** | Unknown | 3.5/5 | 4.2/5 | Post-interaction rating |

### 1.2 Secondary Metrics

| Metric | Current | Target | Notes |
|--------|---------|--------|-------|
| Command discoverability | 6/10 | 8/10 | /help coverage audit |
| Accessibility score | 3/10 | 7/10 | WCAG 2.2 compliance |
| Consistency score | 5/10 | 8/10 | Error format + terminology |
| Feedback coverage | 4/10 | 9/10 | States with visible feedback |
| Scriptability | 3/10 | 7/10 | JSON, quiet, exit codes |

---

## 2. Performance UX Metrics

### 2.1 Latency Metrics (Measured)

| Operation | p50 | p95 | p99 | Target p95 |
|-----------|-----|-----|-----|-----------|
| Startup (--version) | 3ms | 5ms | 8ms | <100ms |
| Startup (REPL) | 50ms | 80ms | 120ms | <500ms |
| Agent loop (echo) | 27ms | 35ms | 50ms | <200ms |
| Cache hit | 66us | 120us | 250us | <1ms |
| Memory search | 2ms | 5ms | 10ms | <50ms |
| Session save | 3ms | 8ms | 15ms | <50ms |
| Doctor report | 45ms | 80ms | 150ms | <500ms |

### 2.2 Resource Metrics (Measured)

| Metric | Value | Limit |
|--------|-------|-------|
| Binary size | 5.1MB | <10MB |
| RSS at startup | 4.7MB | <20MB |
| RSS after 10 rounds | 5.2MB | <50MB |
| SQLite DB per 100 sessions | ~500KB | <10MB |
| Config file size | ~1KB | <10KB |

### 2.3 Perceived Performance

| Scenario | Current Perception | Target Perception |
|----------|-------------------|-------------------|
| Model inference start | Blank screen (2-5s) — feels broken | Spinner — feels responsive |
| Tool execution | No progress — feels uncertain | Timer — feels transparent |
| Context loading | Invisible | Progress indicator — feels informative |
| Cache hit vs miss | Indistinguishable | Verbose mode indicates cache source |

---

## 3. Error Analysis

### 3.1 Error Message Quality Distribution

| Quality Level | Count | % | Definition |
|---------------|-------|---|------------|
| Excellent | 4 | 12% | What + Why + Fix command |
| Good | 8 | 24% | What + partial guidance |
| Fair | 10 | 30% | What only, no guidance |
| Poor | 11 | 33% | Vague or missing context |
| **Total** | **33** | | |

### 3.2 Error Recovery Rate by Category

| Category | Errors | Self-Recoverable | Rate |
|----------|--------|-------------------|------|
| Authentication | 3 | 3 | 100% |
| Configuration | 5 | 2 | 40% |
| Provider/Network | 6 | 1 | 17% |
| Tool Execution | 8 | 5 | 63% |
| Session/Memory | 4 | 2 | 50% |
| MCP Server | 3 | 0 | 0% |
| **Total** | **29** | **13** | **45%** |

**Target: 80% self-recoverable**

### 3.3 Error Prefix Inconsistency

| Pattern | Occurrences | Standard |
|---------|-------------|----------|
| `Error: message` | 8 | TARGET |
| `[ERROR] message` | 4 | Migrate |
| `Config error [field]: msg` | 3 | Migrate |
| `Warning: message` | 5 | Keep (separate level) |
| `[WARN] message` | 2 | Migrate to `Warning:` |
| `eprintln!("...error...")` | 6 | Migrate |

---

## 4. Competitive Metrics

### 4.1 Feature Parity Score

| Dimension | Cuervo | Avg Competitor | Gap |
|-----------|--------|---------------|-----|
| Core functionality | 9/10 | 8/10 | +1 |
| Onboarding | 3/10 | 7/10 | -4 |
| Error handling | 5/10 | 7.5/10 | -2.5 |
| Visual feedback | 4/10 | 7/10 | -3 |
| Accessibility | 3/10 | 5.5/10 | -2.5 |
| Scriptability | 3/10 | 6/10 | -3 |
| **Average** | **4.5/10** | **6.8/10** | **-2.3** |

### 4.2 Unique Advantage Score

Features where Cuervo leads or is unique:

| Feature | Cuervo | Best Competitor | Advantage |
|---------|--------|----------------|-----------|
| Multi-provider routing | Yes (speculative) | Aider (manual) | Significant |
| Circuit breaker/resilience | Yes | None | Unique |
| Response cache (L1+L2) | Yes | None | Unique |
| Episodic memory | Yes | Claude Code (project) | Differentiated |
| TBAC permissions | Yes | Claude Code (basic) | Differentiated |
| Doctor diagnostics | Comprehensive | Claude Code (/doctor) | Comparable |
| Cost optimizer | Yes (3 strategies) | None | Unique |
| Binary size | 5.1MB | Cursor: 200MB+ | Significant |
| Startup time | 3ms | Claude Code: ~500ms | Significant |

---

## 5. Trend Tracking

### 5.1 Metric History (to be filled as versions progress)

| Version | Date | Tests | SUS | Task Success | Error Rate | Issues |
|---------|------|-------|-----|-------------|-----------|--------|
| v0.1.0 | 2026-02 | 690 | 57.5 | 62.5% | 17.5% | 33 open |
| v0.2.0 | TBD | — | Target: 75 | Target: 85% | Target: 8% | — |
| v0.3.0 | TBD | — | Target: 82 | Target: 91% | Target: 5% | — |

### 5.2 Issue Resolution Velocity

| Sprint | Opened | Closed | Net | Backlog |
|--------|--------|--------|-----|---------|
| v0.1.0 | 33 | 0 | +33 | 33 |
| v0.2.0 | TBD | TBD | TBD | TBD |

---

## 6. Measurement Plan

### 6.1 Automated Metrics (Instrument in Code)

| Metric | How to Measure | Where |
|--------|----------------|-------|
| Time to first token | timestamp(query_sent) → timestamp(first_chunk) | `repl/mod.rs` |
| Tool execution duration | Duration in tool chrome | `render/tool.rs` |
| Cache hit rate | hits / (hits + misses) per session | `cache.rs` |
| Error frequency by type | Count CuervoError variants per session | `agent.rs` |
| Session duration | started_at → last_activity | `sessions.rs` |
| Rounds per session | round counter | `agent.rs` |
| Commands used | Count slash commands per session | `commands.rs` |

### 6.2 Survey Metrics (Post-Implementation)

| Metric | Instrument | Frequency |
|--------|-----------|-----------|
| SUS Score | 10-question survey after 1 week of use | Quarterly |
| NPS | "How likely to recommend?" (0-10) | Quarterly |
| CSAT | "How satisfied?" (1-5) after each session | Continuous |
| Task success | Moderated testing with 5 users per persona | Per major release |

### 6.3 Analytics Events (Future)

| Event | Payload | Privacy |
|-------|---------|---------|
| `session_start` | provider, model, is_resume | No PII |
| `round_complete` | tokens, cost, latency, cache_hit | No PII |
| `error_occurred` | error_type (enum, not message) | No PII |
| `command_used` | command_name | No PII |
| `tool_executed` | tool_name, duration, status | No PII |
| `session_end` | total_rounds, total_cost, duration | No PII |

**Note:** All analytics must be opt-in, with clear disclosure. Never collect query content, code, or file paths.

---

## 7. Goals & OKRs

### Q1 2026 (Current Quarter)

**Objective:** Make Cuervo CLI usable without frustration for intermediate developers.

| Key Result | Baseline | Target | Metric |
|------------|----------|--------|--------|
| KR1: SUS score | 57.5 | 75 | Expert evaluation |
| KR2: Error rate | 17.5% | <8% | Error audit |
| KR3: Time-to-first-value | 25 min | <3 min | Setup flow timing |
| KR4: Open UX issues | 33 | <10 | Issue tracker |

### Q2 2026

**Objective:** Enable Cuervo CLI for CI/CD and scripting workflows.

| Key Result | Baseline | Target | Metric |
|------------|----------|--------|--------|
| KR1: --json coverage | 0% | 100% | Command audit |
| KR2: CI/CD task success | 30% | 80% | Scenario testing |
| KR3: WCAG AA compliance | 42% | 80% | Compliance audit |
| KR4: NPS | Unknown | 30+ | User survey |

---

*This dashboard should be updated monthly. Automated metrics should be added as instrumentation is implemented.*
