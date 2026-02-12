# Phase 8 Research Report: Evolving Cuervo to State-of-the-Art LLM Agent Architecture

**Date**: February 7, 2026
**Author**: Principal AI Systems Engineer
**Baseline**: 593 tests, 5.0MB binary, 9 crates, 21k+ LOC, 7 migrations, clippy clean

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Current Architecture Audit](#2-current-architecture-audit)
3. [Gap Analysis: 8 Capability Areas](#3-gap-analysis-8-capability-areas)
4. [Tradeoff Matrix](#4-tradeoff-matrix)
5. [Storage Layer Assessment](#5-storage-layer-assessment)
6. [Proposed Phase 8 Scope](#6-proposed-phase-8-scope)
7. [Risk Register](#7-risk-register)
8. [References](#8-references)

---

## 1. Executive Summary

Cuervo CLI has reached production-grade maturity through 7 phases: a complete REPL, Claude + Ollama providers, tool system with sandboxing, context engine, MCP runtime, resilience layer (circuit breaker, health scoring, backpressure), response caching (L1+L2), parallel execution, cost/latency optimization, context compaction, and PII redaction.

**Key finding**: Cuervo already has latent extension points that anticipate Phase 8 capabilities. The `Planner` trait exists but is unused. The `EmbeddingProvider` trait is defined with no implementations. The `Connector` trait exists with no implementations. `AgentType` already has `Orchestrator` variant. These represent pre-built seams that reduce implementation cost significantly.

**Recommendation**: Phase 8 should focus on 5 high-leverage capabilities in priority order:
1. Adaptive Planner with Re-Planning (lowest effort, highest immediate ROI)
2. Reflexion Self-Improvement Loop (low effort, builds on existing traces + memory)
3. TBAC Authorization Layer (medium effort, critical for safety)
4. Episodic Memory with Hybrid Retrieval (medium-high effort, highest long-term impact)
5. Multi-Agent Orchestration (highest effort, transformative capability)

Deferred to Phase 9+: Decentralized collaboration (A2A protocol still evolving), full GCC-style versioned context (experimental), LLM-powered guardrails (requires classifier model).

---

## 2. Current Architecture Audit

### 2.1 Module Coupling Map

**agent.rs (~1700 LOC) is the coordination hub** — it imports from 6+ modules and holds the 14-parameter `run_agent_loop()` function. This is the primary bottleneck for extensibility.

```
agent.rs ←→ resilience.rs     [TIGHT] — pre/post invoke gating, fallback logic
agent.rs ←→ response_cache.rs [TIGHT] — inline cache lookup/store
agent.rs ←→ executor.rs       [MODERATE] — clean plan_execution() separation
agent.rs ←→ compaction.rs     [MODERATE] — optional side effect, can disable
agent.rs ←→ speculative.rs    [MODERATE] — delegates via invoke_with_fallback()
```

**Refactoring opportunity**: Extract `cacheable_invoke()` helper that encapsulates resilience check → cache lookup → provider.invoke() → cache store → metric persist. This would reduce agent.rs coupling by ~200 lines.

### 2.2 Trait Abstraction Readiness

| Trait | Defined | Implemented | Used in Agent Loop |
|-------|---------|------------|-------------------|
| `ModelProvider` | cuervo-core | Anthropic, Ollama, Echo | Yes — via ProviderRegistry |
| `Planner` | cuervo-core | None | **No** — PlanningSource exists but doesn't use trait |
| `ContextSource` | cuervo-core | InstructionSource, PlanningSource, MemorySource | Yes — Vec<Box<dyn ContextSource>> |
| `EmbeddingProvider` | cuervo-core | **None** | No |
| `Tool` | cuervo-core | file_ops, bash, git, search | Yes — via ToolRegistry |
| `Connector` | cuervo-core | **None** | No |

**Key insight**: `Planner`, `EmbeddingProvider`, and `Connector` traits are architectural placeholders ready for Phase 8 activation.

### 2.3 Type System Extension Points

- `AgentType` enum: `Chat | Coder | Reviewer | Orchestrator` — only `Chat` currently used
- `EventPayload`: 14 variants including `AgentStarted`/`AgentCompleted` — ready for multi-agent events
- `PermissionLevel`: `ReadOnly | ReadWrite | Destructive` — needs expansion for TBAC
- `PlanStep` / `ExecutionPlan`: defined in `planner.rs` with confidence field — ready for adaptive planning

### 2.4 Memory Architecture (3 Tiers)

| Tier | Implementation | Capacity | Limitations |
|------|---------------|----------|-------------|
| L1 In-Memory | `Mutex<LruCache<String, CacheEntry>>` | min(config, 100) | Response-level only, no semantic grouping |
| L2 SQLite Cache | `response_cache` table, SHA-256 keys | Bounded by TTL + max_entries | No semantic search, keyword-based dedup only |
| L3 Semantic Memory | `memory_entries` + FTS5 (BM25) | Unbounded (manual prune) | No embeddings, no episodic grouping, no temporal decay |

**Critical gap**: No embedding-based retrieval. FTS5 BM25 misses semantic similarity. No episodic isolation (memories from Project A leak into Project B searches). No confidence decay (old memories treated same as recent).

---

## 3. Gap Analysis: 8 Capability Areas

### 3.1 Multi-Agent Orchestration

**Current State**: Single-agent loop in `agent.rs`. `AgentType::Orchestrator` variant defined but unused.

**State of the Art (2025-2026)**:
- **LangGraph 1.0** (LangChain): Graph-based execution with nodes as agents, edges as handoff conditions. Supports cycles, conditional branching, sub-graphs. Production-deployed at scale.
- **OpenAI Agents SDK**: Lightweight handoff protocol between specialized agents. Each agent has tools, instructions, guardrails. Handoff = function call returning target agent.
- **AutoGen 0.4** (Microsoft): Event-driven multi-agent with Selector/RoundRobin/Swarm group patterns. Supports custom agent classes, nested teams.
- **CrewAI**: Role-based agents with task delegation. YAML-driven configuration.

**Gap Assessment**:
| Aspect | Cuervo | SOTA | Gap |
|--------|--------|------|-----|
| Agent types | 4 enums (unused) | N specialized agents | Need orchestrator + at least 2 specialized agents |
| Handoff protocol | None | Function-call based | Need Agent trait with handoff() |
| State sharing | Single session | Per-agent + shared memory | Need agent-scoped memory namespaces |
| Execution graph | Linear loop | DAG with cycles | Need graph execution engine |
| Concurrency | Sequential | Parallel agent execution | Need tokio task spawning per agent |

**Recommended approach**: OpenAI SDK-style handoff (simpler than LangGraph DAG). Define `Agent` trait with `run()` + `handoff()`. Orchestrator delegates to specialized agents (Coder, Reviewer). State passed via shared `Session` with agent-scoped memory.

**Effort**: ~2500-3500 LOC | **Binary impact**: +200-500KB

### 3.2 Decentralized Collaboration

**Current State**: No inter-agent communication beyond single process.

**State of the Art**:
- **Google A2A Protocol** (April 2025, Linux Foundation): HTTP/JSON-based agent-to-agent communication. Agent Cards for discovery, task lifecycle management. 50+ technology partners.
- **Cisco AGNTCY Platform**: Agent discovery, identity verification, TBAC for inter-agent auth.
- **MCP**: Already implemented in Cuervo for agent-to-tool communication.

**Gap Assessment**: A2A protocol is still evolving rapidly (specification updates ongoing). Purely decentralized (no central orchestrator) P2P systems remain largely experimental. Most production deployments use hybrid: centralized planning + decentralized execution.

**Recommendation**: **DEFER to Phase 9+**. A2A spec needs to stabilize. Instead, focus on intra-process multi-agent orchestration first. The A2A client/server is ~1500-2500 LOC when ready.

### 3.3 Episodic + Hierarchical Memory

**Current State**: FTS5 BM25 search only. No embedding retrieval. No episodic grouping. `memory_entries` table has unused `embedding` BLOB column and `embedding_model` TEXT column.

**State of the Art**:
- **Zep / Graphiti** (Jan 2025): Temporal knowledge graph with 4 parallel retrieval paths (cosine, BM25, graph BFS, temporal). Reciprocal Rank Fusion + cross-encoder reranking. P95: 300ms.
- **Mem0** (Apr 2025): Extraction + CRUD update pipeline. 26% improvement over OpenAI baseline, 91% lower P95 latency, 90% token savings.
- **MemGPT / Letta**: LLM-managed virtual memory paging (core/recall/archival tiers).
- **MemR3** (Dec 2025): "Memory Retrieval via Reflective Reasoning" — agent reasons about what memories it needs before retrieving.

**Gap Assessment**:
| Aspect | Cuervo | SOTA | Gap |
|--------|--------|------|-----|
| Search | FTS5 BM25 only | Hybrid (BM25 + embedding + graph + temporal) | Need embedding retrieval + RRF fusion |
| Grouping | Flat (all entries searchable) | Episodic (per-session/task) | Need `memory_episodes` table + junction |
| Temporal | `created_at` exists, unused in ranking | Recency-weighted scoring | Need temporal decay factor |
| Confidence | Manual `relevance_score` | Computed confidence with decay | Need auto-scoring |
| Proactive recall | Only on user_message | Agent reasons about what to recall | Need MemR3-style reflective retrieval |

**Recommended approach**: Pragmatic hybrid — keep FTS5 BM25, add embedding-based cosine retrieval (via provider API call or local model), fuse with Reciprocal Rank Fusion. Add `memory_episodes` table for episodic grouping. Add temporal decay to ranking. Skip graph-based memory (too complex for CLI agent).

**Effort**: ~2000-3500 LOC | **Binary impact**: +300-600KB (petgraph if graph added)

### 3.4 Versioned Memory / Context Store

**Current State**: No versioning. Sessions have `messages_json` (mutable). No snapshots. No rollback.

**State of the Art**:
- **Git Context Controller (GCC)** (Jul 2025): Version-controlled file system for agent memory. COMMIT/BRANCH/MERGE/CONTEXT operations. 48% on SWE-Bench-Lite. Structured behaviors (writing tests, committing) emerged spontaneously from framing.
- **LanceDB / Lance Context** (production at Uber): Versioned multimodal context with HNSW indexing.
- **git-notes-memory-manager** (Dec 2025): Git-native semantic memory with progressive hydration.

**Gap Assessment**: Cuervo has SQLite + git2 — both sufficient for a snapshot DAG. The main question is granularity: per-round vs per-task vs per-session.

**Recommendation**: **Partially defer**. Implement lightweight context snapshots (SQLite DAG with parent-child) for rollback support. Full GCC-style BRANCH/MERGE is experimental and adds significant prompt complexity. Start with COMMIT + ROLLBACK only.

**Effort**: ~1000-1500 LOC | **Binary impact**: +100-200KB (no new deps)

### 3.5 Adaptive Planner with Dynamic Re-Planning

**Current State**: `PlanningSource` (priority=90) injects plan-execute instructions. `Planner` trait defined but unused. No replanning on failure.

**State of the Art**:
- **ReAct**: Iterative think-act-observe. Flexible but expensive (LLM call per action).
- **Plan-and-Execute**: Separate planner/executor. Efficient but brittle without replanning.
- **LATS** (ICML 2024): Monte Carlo tree search over reasoning steps. Enables backtracking.
- **ReAcTree** (Nov 2025): Hierarchical task decomposition with dynamic tree expansion. 61% on WAH-NL (2x ReAct).

**Production consensus**: Hybrid Plan-Execute as default + failure-triggered replanning + ReAct-style reasoning within individual steps.

**Gap Assessment**:
| Aspect | Cuervo | SOTA | Gap |
|--------|--------|------|-----|
| Planning | PlanningSource prompt injection | Planner trait with PlanStep | Need to wire Planner trait |
| Failure detection | InvocationMetric records success/failure | Auto-detect + classify failure | Need failure classifier |
| Replanning | None | Failure-triggered replan | Need replan loop in agent.rs |
| Step tracking | Trace steps (append-only) | Plan steps with outcomes | Need planning_steps table |
| Backtracking | None | Tree search (LATS) | Defer — too complex for CLI |

**Recommended approach**: Wire existing `Planner` trait. Implement `DefaultPlanner` that decomposes tasks into `PlanStep`s. Add failure-triggered replanning (on tool error or agent loop failure, re-invoke planner with failure context). Track plan steps in new DB table.

**Effort**: ~800-1500 LOC | **Binary impact**: +50-150KB (no new deps)

### 3.6 Task-Based Authorization (TBAC)

**Current State**: `PermissionChecker` with `needs_prompt()` / `auto_decide()` / `apply_answer()`. Three-level `PermissionLevel` (ReadOnly/ReadWrite/Destructive). No task context. No temporal bounds. No scoped tokens.

**State of the Art**:
- **Cisco AGNTCY TBAC**: Evaluates Task + Tool + Transaction dimensions. Scoped tokens per task with tool allowlists, parameter constraints, monetary limits, temporal expiry.
- **Uncertainty-Aware TBAC** (Oct 2025): LLM as autonomous risk-aware authorization judge.
- **1Password / CyberArk**: Per-agent identities with scoped token issuance.

**Gap Assessment**:
| Aspect | Cuervo | SOTA | Gap |
|--------|--------|------|-----|
| Permission model | 3 static levels | Task-scoped dynamic permissions | Need TaskContext propagation |
| Tool scoping | All tools available | Tool allowlist per task | Need task → tool mapping |
| Parameter constraints | Path security only | Arbitrary parameter constraints | Need constraint expressions |
| Temporal bounds | None | TTL on elevated permissions | Need expiry on decisions |
| Audit trail | EventPayload::ToolDenied | Full TBAC audit | Need policy_decisions table |

**Recommended approach**: Extend `PermissionChecker` with `TaskContext` struct (task_id, allowed_tools, parameter_constraints, ttl). Propagate through executor. Store decisions in new `policy_decisions` table. Skip LLM-judged TBAC (non-deterministic, adds latency).

**Effort**: ~1000-2000 LOC | **Binary impact**: +50-100KB

### 3.7 Formal Safety Constraints

**Current State**: PII regex redaction (12 patterns), SandboxConfig with rlimits, PermissionChecker (interactive prompts), audit trail (hash-chained), circuit breaker + backpressure.

**State of the Art**:
- **OWASP Top 10 for Agentic Applications 2026**: Canonical risk framework (goal hijack, rogue agents, tool misuse, privilege escalation, memory poisoning, supply chain, data exfiltration, unbounded consumption, prompt leakage, misinformation).
- **Anthropic Constitutional Classifiers**: Two-stage probe + classifier. 0.05% false refusal rate. 1% compute overhead.
- **NVIDIA NeMo Guardrails**: Colang DSL for programmable input/output guardrails.
- **OpenAI Agents SDK**: Input + output guardrails (parallel or blocking execution).

**Defense-in-depth stack** (production pattern):
```
Layer 1: Input Sanitization     ← Cuervo HAS (PII regex)
Layer 2: Pre-Invocation Guard   ← Cuervo MISSING
Layer 3: Tool Permission Gate   ← Cuervo HAS (PermissionChecker)
Layer 4: Sandbox Execution      ← Cuervo HAS (rlimits)
Layer 5: Output Validation      ← Cuervo MISSING
Layer 6: Audit Trail            ← Cuervo HAS (hash-chained audit_log)
Layer 7: Circuit Breaker        ← Cuervo HAS (resilience layer)
```

**Gap**: Layers 2 (pre-invocation guardrail) and 5 (output validation). Both can start as regex-based rules, upgrading to LLM-powered classifiers later.

**Recommended approach**: Add `Guardrail` trait with `check_input()` / `check_output()`. Implement `RegexGuardrail` first (jailbreak patterns, output policy). Wire into agent loop: input guard before `provider.invoke()`, output guard after response. Parallel execution by default.

**Effort**: ~800-1500 LOC | **Binary impact**: +50-150KB

### 3.8 Self-Improvement / Meta-Optimization

**Current State**: InvocationMetric records success/failure. Trace recording/replay exists. Response cache with hit_count. CostLatencyOptimizer ranks models. No self-reflection loop.

**State of the Art**:
- **Reflexion** (NeurIPS 2023): Actor + Evaluator + Self-Reflection. Verbal feedback stored in episodic memory. +22% AlfWorld, +20% HotPotQA, +11% HumanEval.
- **AgentDevel** (Jan 2026): Self-improvement as release engineering. Implementation-blind LLM critic + script-based diagnosis + flip-centered gating.
- **Self-Debugging** (ICLR 2024): Execution trace feedback for iterative code fixing.

**Gap Assessment**:
| Aspect | Cuervo | SOTA | Gap |
|--------|--------|------|-----|
| Execution traces | Full trace recording (Sprint 6) | Same | None — already captured |
| Failure memory | Metrics table (success bool) | Verbal reflections stored in memory | Need reflection generation + storage |
| Self-evaluation | CostLatencyOptimizer (model-level) | Task-level evaluator | Need task outcome evaluation |
| Reflection retrieval | MemorySource (BM25) | Context-aware reflection injection | Need reflection as ContextSource |

**Recommended approach**: Implement Reflexion pattern. After task failure (tool error chain or unsatisfactory outcome): (1) generate self-reflection prompt from execution trace, (2) store reflection in `memory_entries` with type `Reflection`, (3) create `ReflectionSource` (priority=85) that retrieves relevant past reflections. Skip AgentDevel (too complex for CLI, requires benchmark infrastructure).

**Effort**: ~800-1500 LOC | **Binary impact**: +50-100KB

---

## 4. Tradeoff Matrix

| # | Capability | Complexity | Benefit | Risk | LOC Estimate | Binary Impact | Dependencies | Priority |
|---|-----------|-----------|---------|------|-------------|--------------|-------------|----------|
| 1 | Adaptive Planner | **Low** | **High** — structured task execution, failure recovery | Low — builds on existing PlanningSource | 800-1500 | +50-150KB | None new | **P0** |
| 2 | Reflexion Loop | **Low** | **High** — continuous improvement from failures | Low — verbal feedback, no weight updates | 800-1500 | +50-100KB | None new | **P0** |
| 3 | TBAC Authorization | **Medium** | **High** — security-critical for production | Medium — PermissionChecker refactor needed | 1000-2000 | +50-100KB | None new | **P1** |
| 4 | Safety Guardrails | **Low-Med** | **High** — fills defense-in-depth gaps | Low — additive, doesn't change existing flow | 800-1500 | +50-150KB | None new | **P1** |
| 5 | Episodic Memory | **Med-High** | **Very High** — enables contextual recall | Medium — schema changes, hybrid retrieval | 2000-3500 | +300-600KB | petgraph (optional) | **P2** |
| 6 | Multi-Agent | **High** | **Very High** — transformative capability | High — agent.rs major refactor, concurrency | 2500-3500 | +200-500KB | None new | **P2** |
| 7 | Versioned Context | **Medium** | **Medium** — rollback/audit capability | Low — additive snapshot table | 1000-1500 | +100-200KB | None new | **P3** |
| 8 | Decentralized Collab | **Medium** | **Medium** — cross-agent interop | High — A2A spec still evolving | 1500-2500 | +100-300KB | None new | **Defer** |

**Priority Key**: P0 = implement first (Stages 3-4), P1 = implement second, P2 = implement third, P3 = lightweight version, Defer = Phase 9+

---

## 5. Storage Layer Assessment

### 5.1 Current Schema (7 Migrations, 7 Tables)

| Table | Rows/Month (est) | Growth Risk | Index Coverage |
|-------|-----------------|-------------|---------------|
| `sessions` (14 cols) | 30-300 | Unbounded (no TTL) | Good — updated_at DESC |
| `audit_log` (hash-chained) | 30k-300k | Unbounded, immutable | Good — timestamp, event_type |
| `trace_steps` (append-only) | 300k-3M | **HIGH** — no rollover | Good — (session_id, step_index) |
| `memory_entries` (11 cols) + FTS5 | 3k-30k | Moderate — FTS index bloat at 1M+ | Good — type, session, hash |
| `response_cache` (9 cols) | 1.5k-15k | Bounded by TTL | Good — cache_key, expires_at |
| `invocation_metrics` (10 cols) | 30k-300k | **HIGH** — N+1 in system_metrics() | Partial — no prune index |
| `resilience_events` (7 cols) | 3k-30k | Low | Good — provider, type, created_at |

### 5.2 Performance Concerns

1. **`system_metrics()` N+1 pattern**: Fetches all (provider, model) pairs, then loops calling `model_stats()` for each — 400+ queries if 100 models exist. Solution: aggregate by day/week or use SQLite window functions.
2. **`prune_memories()`/`prune_cache()`**: Full table sort before DELETE — no WHERE filter index. Add `(relevance_score, created_at)` index.
3. **P95 latency calculation**: Manual OFFSET-based calculation — O(n) scan. Use `NTILE()` window function (SQLite 3.30+).
4. **Trace steps explosion**: Long sessions can reach 10k+ steps. No rollover mechanism.

### 5.3 Required Schema Changes for Phase 8

| Migration | Tables | Purpose | Backward Compat |
|-----------|--------|---------|-----------------|
| 008 | `planning_steps` | Plan step tracking with outcomes | Additive — no existing data affected |
| 009 | `memory_episodes` + `memory_entry_episodes` | Episodic memory grouping | Additive — junction table |
| 010 | `policy_decisions` | TBAC audit trail | Additive — separate from audit_log |
| 011 | `context_snapshots` | Versioned context (lightweight) | Additive — snapshot DAG |
| 012 | ALTER `memory_entries` ADD `agent_id` | Multi-agent memory scoping | DEFAULT NULL — backward compat |

### 5.4 AsyncDatabase Gaps

All hot-path methods are wrapped. Missing async wrappers needed for Phase 8:
- `list_memories()` — needed for background memory sweeps
- `prune_memories()` — needed if called from agent loop
- New methods for planning_steps, policy_decisions, episodes

### 5.5 FTS5 Limitations

- Single content field — no field-specific weighting
- No semantic/vector search — pure BM25 lexical matching
- BLOB `embedding` column exists but unused (not FTS-searchable)
- No phrase search or proximity ranking

**Phase 8 upgrade path**: Keep FTS5 for lexical retrieval. Add embedding-based retrieval via API call (AnthropicProvider or dedicated embedding endpoint). Fuse results with Reciprocal Rank Fusion. The existing `embedding` + `embedding_model` columns in `memory_entries` are ready to store vectors.

---

## 6. Proposed Phase 8 Scope

### Stage 3: Development (5 Sub-Phases)

**Sub-Phase 8.1: Adaptive Planner** (P0, ~800-1500 LOC)
- Wire existing `Planner` trait with `DefaultPlanner` implementation
- Add `planning_steps` table (migration 008) for step tracking
- Implement failure-triggered replanning in agent loop
- Track plan step outcomes (success/failure/skipped)
- Update PlanningSource to use Planner output instead of generic prompt

**Sub-Phase 8.2: Reflexion Self-Improvement** (P0, ~800-1500 LOC)
- Add `Reflection` entry type to memory_entries
- Implement reflection generation from execution traces (on task failure)
- Create `ReflectionSource` (priority=85) as new ContextSource
- Wire into agent loop: after failed round → generate reflection → store
- On subsequent attempts, relevant reflections injected into context

**Sub-Phase 8.3: TBAC Authorization** (P1, ~1000-2000 LOC)
- Define `TaskContext` struct (task_id, allowed_tools, parameter_constraints, ttl)
- Extend PermissionChecker to accept TaskContext
- Add `policy_decisions` table (migration 010)
- Propagate task context through executor
- Wire into tool execution pipeline

**Sub-Phase 8.4: Safety Guardrails** (P1, ~800-1500 LOC)
- Define `Guardrail` trait with `check_input()` / `check_output()`
- Implement `RegexGuardrail` (jailbreak patterns, output policy)
- Wire into agent loop: input guard pre-invoke, output guard post-response
- Parallel execution (tokio::select with agent invocation)
- Config-driven enable/disable

**Sub-Phase 8.5: Episodic Memory + Hybrid Retrieval** (P2, ~2000-3000 LOC)
- Add `memory_episodes` + junction table (migration 009)
- Implement embedding retrieval via provider API
- Implement Reciprocal Rank Fusion (BM25 + embedding scores)
- Add temporal decay factor to memory ranking
- Add episode auto-creation (group memories by session + time window)
- Optional: Add `agent_id` column for multi-agent scoping (migration 012)

### Deferred to Phase 9+

- **Multi-Agent Orchestration**: Requires agent.rs refactor to support re-entrant agent loop. Design Agent trait with handoff(). Graph execution engine. Substantial effort (~2500-3500 LOC) and testing burden.
- **Versioned Context**: Lightweight snapshot DAG only if needed by planner rollback. Full GCC-style branching is experimental.
- **Decentralized Collaboration**: Wait for A2A protocol stabilization. Current MCP runtime provides tool-level interop.
- **LLM-Powered Guardrails**: Start with regex, upgrade to classifier when evaluation infrastructure exists.

### Estimated Totals

| Metric | Estimate |
|--------|---------|
| New LOC | 5,400-9,500 |
| New tests | 60-100 |
| New migrations | 3-5 |
| Binary impact | +500KB-1.5MB (from 5.0MB → 5.5-6.5MB) |
| New dependencies | 0-1 (petgraph optional) |
| Total tests after | 653-693 |

---

## 7. Risk Register

### High Risk

| Risk | Impact | Mitigation |
|------|--------|-----------|
| agent.rs parameter explosion | 14 params → 18+ params, unmaintainable | Extract `AgentContext` struct to bundle related params |
| Planner integration breaks existing tests | ~17 test call sites for run_agent_loop | Planner is optional (None); existing tests pass unchanged |
| Memory schema changes break sessions | Existing memory_entries rows may lose compatibility | All migrations use ALTER TABLE ADD COLUMN with DEFAULT values |
| Reflection loop adds latency | Extra LLM call on failure adds 1-5s | Make reflection async, fire-and-forget; cache reflections |

### Medium Risk

| Risk | Impact | Mitigation |
|------|--------|-----------|
| TBAC too restrictive | Over-scoped permissions hurt agent effectiveness | Default permissive; explicit deny-list rather than allow-list |
| Embedding API cost | Embedding retrieval requires API call per memory search | Batch embeddings, cache computed embeddings in DB |
| FTS5 + embedding fusion complexity | RRF scoring may not be better than BM25 alone | A/B test with existing FTS5 as control; make hybrid optional |
| N+1 query degradation at scale | system_metrics() slows as invocation_metrics grows | Add daily aggregation migration; pre-compute model rankings |

### Low Risk

| Risk | Impact | Mitigation |
|------|--------|-----------|
| New tables increase DB size | ~10-50KB per table initially | SQLite handles this effortlessly; existing WAL mode helps |
| Guardrail false positives | Regex-based guardrails may block legitimate queries | Start with high-confidence patterns only; user can disable |
| Plan step tracking overhead | Extra DB writes per plan step | Fire-and-forget async writes; batch when possible |

---

## 8. References

### Multi-Agent Orchestration
- [LangGraph 1.0 Multi-Agent Patterns](https://langchain-ai.github.io/langgraph/concepts/multi_agent/)
- [OpenAI Agents SDK](https://openai.github.io/openai-agents-python/)
- [AutoGen 0.4 Framework](https://microsoft.github.io/autogen/stable/)
- [CrewAI Documentation](https://docs.crewai.com/)

### Decentralized Collaboration
- [Google A2A Protocol](https://a2a-protocol.org/latest/specification/)
- [MCP Security Risks — Red Hat](https://www.redhat.com/en/blog/model-context-protocol-mcp-understanding-security-risks-and-controls)
- [Cisco AGNTCY Platform](https://outshift.cisco.com/blog/tool-transaction-task-based-access-control-tbac)

### Episodic + Hierarchical Memory
- [Zep / Graphiti: Temporal Knowledge Graphs](https://arxiv.org/abs/2501.13956)
- [Mem0: Production-Ready Long-Term Memory](https://arxiv.org/abs/2504.19413)
- [MemGPT / Letta: LLMs as Operating Systems](https://arxiv.org/abs/2310.08560)
- [MemR3: Memory Retrieval via Reflective Reasoning](https://arxiv.org/html/2512.20237v1)

### Versioned Context
- [Git Context Controller Paper](https://arxiv.org/html/2508.00031v1)
- [LanceDB Versioned Context Memory](https://tessl.io/blog/lancedb-brings-versioned-context-memory-to-multimodal-ai-agents/)
- [Git-Native Semantic Memory](https://zircote.com/blog/2025/12/git-native-semantic-memory/)

### Adaptive Planning
- [LATS: Language Agent Tree Search (ICML 2024)](https://arxiv.org/abs/2310.04406)
- [ReAcTree: Hierarchical LLM Agent Trees](https://arxiv.org/abs/2511.02424)
- [LangChain Planning Agents](https://blog.langchain.com/planning-agents/)

### TBAC
- [Cisco TBAC for Agentic AI](https://outshift.cisco.com/blog/tool-transaction-task-based-access-control-tbac)
- [Uncertainty-Aware TBAC](https://arxiv.org/abs/2510.11414)
- [Thomas & Sandhu TBAC Original Paper](https://profsandhu.com/confrnc/ifip/i97tbac.pdf)

### Safety & Guardrails
- [OWASP Top 10 for Agentic Applications 2026](https://genai.owasp.org/resource/owasp-top-10-for-agentic-applications-for-2026/)
- [Anthropic Constitutional Classifiers](https://www.anthropic.com/research/next-generation-constitutional-classifiers)
- [NVIDIA NeMo Guardrails](https://github.com/NVIDIA-NeMo/Guardrails)
- [OpenAI Agents SDK Guardrails](https://openai.github.io/openai-agents-python/guardrails/)

### Self-Improvement
- [Reflexion: Verbal Reinforcement Learning (NeurIPS 2023)](https://arxiv.org/abs/2303.11366)
- [AgentDevel: Self-Evolving Agents as Release Engineering](https://arxiv.org/abs/2601.04620)
- [Self-Debugging (ICLR 2024)](https://proceedings.iclr.cc/paper_files/paper/2024/file/2460396f2d0d421885997dd1612ac56b-Paper-Conference.pdf)
