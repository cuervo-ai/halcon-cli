# Implementation Progress

## Sprint 1 — Security Foundation (Weeks 1-2)

| Paso | Story | Status | Commit | Tests Added | Bugs Fixed |
|------|-------|--------|--------|-------------|------------|
| 1-A  | SynthesisGate ordering fix | ✅ DONE | fix(convergence)... | 2 | BUG-001, BUG-002 |
| 1-B  | tokio::sync::Mutex migration | ✅ DONE (no changes needed) | — | 0 | — |
| 1-C  | Ctrl-C cancellation | ✅ DONE (already implemented) | — | 0 | — |
| 1-D  | Feature flags defaults | ✅ DONE (already true) | — | 0 | — |
| 1-E  | Audit export PDF/CSV/JSONL | ✅ DONE (already implemented) | — | 0 | — |

## Sprint 2 — Providers + CI/CD (Weeks 3-4)

| Paso | Story | Status | Tests Added |
|------|-------|--------|-------------|
| 2-A  | --output-format json | ✅ DONE | 5 (CiSink) |
| 2-B  | AWS Bedrock provider | ✅ DONE | 6 |
| 2-C  | Vertex AI provider | ✅ DONE | 5 |
| 2-D  | Azure Foundry provider | ✅ DONE | 4 |
| 2-E  | GitHub Actions action.yml | ✅ DONE | — |

## Sprint 3 — Analytics + Compliance (Weeks 5-6)

| Paso | Story | Status | Tests Added |
|------|-------|--------|-------------|
| 3-A  | Admin analytics API | ✅ DONE | 14 |
| 3-B  | RBAC 4 roles | ✅ DONE | 22 |
| 3-C  | Compliance reports | ✅ DONE | — |
| 3-D  | Air-gap mode | ✅ DONE | 1 |

## Sprint 4 — Agent Network (Weeks 7-10)

| Paso | Story | Status | Commit | Tests Added | Files Changed |
|------|-------|--------|--------|-------------|---------------|
| 4-A  | Mailbox P2P for agent-to-agent communication | ✅ DONE | `feat(agents): mailbox P2P...` | 4 (mailbox tests) | `halcon-storage/src/mailbox.rs` (NEW), `migrations.rs` (M37), `lib.rs` |
| 4-B  | Lead/Teammate/Specialist/Observer roles | ✅ DONE | `feat(agents): Lead/Teammate...` | 6 (AgentRole tests) | `halcon-core/src/types/orchestrator.rs`, `repl/orchestrator.rs`, `slash_commands.rs`, `delegation.rs` |
| 4-C  | Cron-based scheduled agent tasks | ✅ DONE | `feat(scheduler): cron-based...` | 4+3 (scheduler+schedule tests) | `repl/agent_scheduler.rs` (NEW), `commands/schedule.rs` (NEW), `migrations.rs` (M38), `main.rs` |
| 4-D  | Wire PlaybookPlanner before LlmPlanner | ✅ DONE | `feat(planning): wire PlaybookPlanner...` | 0 (wiring fix) | `repl/mod.rs` (retry path fix) |

### Sprint 4 Technical Details

#### PASO 4-A: Mailbox P2P
- **New file**: `crates/halcon-storage/src/mailbox.rs`
- **Migration 37**: `mailbox_messages` table with composite index on `(team_id, to_agent, consumed, expires_at)`
- **API**: `send()`, `receive()`, `broadcast()`, `mark_consumed()`, `purge_expired()`
- **Design decisions**: SQLite over in-memory channels for durability + audit trail; WAL mode for concurrent reads
- **Tests**: broadcast delivery, direct P2P reply, expired TTL filtering, mark_consumed idempotency

#### PASO 4-B: Agent Roles
- **New type**: `AgentRole` enum (`Lead | Teammate | Specialist | Observer`) in `halcon-core`
- **Timeout multipliers**: Lead=1.0×, Teammate=0.6×, Specialist=0.8×, Observer=0.1×
- **Rounds multipliers**: Lead=1.0×, Teammate=0.7×, Specialist=0.5×, Observer=0.0 (no tool execution)
- **New fields on `SubAgentTask`**: `role: AgentRole`, `team_id: Option<Uuid>`, `mailbox_id: Option<Uuid>`
- **Backward compat**: all new fields `#[serde(default)]`, old JSON deserializes to `AgentRole::Lead`
- **Wired in orchestrator**: role multipliers applied to limits before sub-agent launch

#### PASO 4-C: Scheduled Tasks
- **New file**: `crates/halcon-cli/src/repl/agent_scheduler.rs`
- **New file**: `crates/halcon-cli/src/commands/schedule.rs`
- **Migration 38**: `scheduled_tasks` table
- **CLI**: `halcon schedule add|list|disable|enable|run`
- **Scheduler**: `AgentScheduler::start(cancel)` — 60s tokio::time::interval background task
- **Cron parsing**: `croner` v2.2.0 — pure Rust, 5-field + optional seconds
- **Design**: Scheduler does NOT execute agents directly; it marks `last_run_at` and logs the task for the event bridge to dispatch

#### PASO 4-D: PlaybookPlanner Wiring
- **Gap fixed**: The retry `AgentContext` at `repl/mod.rs:3287` was using only `LlmPlanner`, bypassing `PlaybookPlanner`
- **Fix**: Added the same `find_match()` check to the retry path as exists in the primary invocation path
- **Effect**: Repeated tasks (code review, git commit, test run) now use deterministic zero-LLM plans on both first and retry invocations

### Pre-existing Bugs Found and Fixed (Sprint 4 session)

| Bug | Location | Fix |
|-----|----------|-----|
| Stale `pub use reward::RewardPipeline` re-export (type removed from reward.rs) | `repl/metrics/mod.rs:17` | Removed stale re-export line |
| Migration count assertions hard-coded to 36 | `migrations.rs:1279,1293` | Updated to 38 (after M37 mailbox + M38 scheduled_tasks) |

---

## Post-Sprint Architecture Improvements (2026-03-08)

Identified by principal audit and applied as follow-up hardening commits.

| # | Improvement | Status | Commit | Benefit |
|---|-------------|--------|--------|---------|
| 1 | Unify synthesis logic into `SynthesisController` struct | 🔲 TODO | — | Eliminates 3 scattered synthesis guard sites |
| 2 | Whitelist-based synthesis summary detection | ✅ DONE | `a01fab5` | SC-2 robustness: 6 keywords instead of 1 |
| 3 | `summary_preview` in zero-tool-drift warn log | ✅ DONE | `a01fab5` | Operator observability without extra overhead |
| 4 | Extract `run_agent_loop` into sub-functions | 🔲 TODO | — | Reduces cyclomatic complexity from ~80 to <20 |
| 5 | Provider-specific tool fallback registry | ✅ AUDITED | — | All providers already use correct format; `normalize_schema_for_openai()` handles bare-object schemas |

### Improvement #2 Details — Synthesis Summary Whitelist

**File**: `crates/halcon-cli/src/repl/orchestrator.rs`

Before: `!summary.to_lowercase().contains("synthesis")`

After:
```rust
const SYNTHESIS_SUMMARY_KEYWORDS: &[&str] = &[
    "synthesis", "summariz", "conclus", "final", "review", "analysis complete"
];
fn is_synthesis_summary(summary: &str) -> bool {
    let lower = summary.to_lowercase();
    SYNTHESIS_SUMMARY_KEYWORDS.iter().any(|kw| lower.contains(kw))
}
```

**New tests** (`synthesis_whitelist_covers_all_variants`): 8 cases — 5 positive matches, 3 negative.

---

_Last updated: 2026-03-08 (post-sprint hardening)_
