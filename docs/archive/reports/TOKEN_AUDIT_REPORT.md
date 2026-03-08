# TOKEN_AUDIT_REPORT.md — Phase K3
# Token Economy Audit

Generated: 2026-02-22 | Branch: feature/sota-intent-architecture

---

## OBSERVED TOKEN USAGE

### Session A: "hola" (DirectExecution)
```
Round 1: ↑4384 ↓179 = 4563 tokens
LoopCritic call: ≈4200 ↑ ≈178 ↓ = ~4378 tokens (extra API call)
Total: ↑8768 ↓357 ≈ 9126 tokens (confirmed: 8.8K ↑ 358 ↓)

Token attribution:
  tokens_system_prompt:    ~3800 (system + tool defs: 77 tools × ~50 tokens)
  tokens_user_message:     ~5    ("hola")
  tokens_conversation:     ~0    (first turn)
  tokens_model_response:   179   (greeting reply)
  tokens_critic:           ~4378 (LoopCritic API call — SHOULD NOT RUN)
  tokens_planning:         0     (DirectExecution, no plan)
  tokens_subagents:        0
  tokens_synthesis:        0
  tokens_memory:           ~104  (L0 context injection: 1 token from hot memory)
```

### Session B: "analiza mi implementacion" (PlanExecuteReflect + critic retry)
```
Total: ↑140.2K ↓1.2K = 141335 tokens

Estimated token attribution:
  tokens_planning (2 plans):       ~2400   (2 LLM planner calls)
  tokens_subagents (2 sub-agents): ~85000  (file reads + context)
    - Sub-agent 1 (Coder/glob):    ~8000
    - Sub-agent 2 (Chat):
        Round 1: ↑20492 ↓127      ~20619
        Round 2: ↑24504 ↓106      ~24610
        + sub-agent system/tools:  ~3850 × 2 rounds = ~7700
        Total sub-agent 2:         ~52929
  tokens_coordinator (2 attempts): ~10000
    - Attempt 1, Round 1:          ↑5167  ↓481
    - Attempt 2, Round 1:          ↑20492 included in sub-agent above
    - Attempt 2, Round 2:          ↑24504 included in sub-agent above
  tokens_critic (2 calls):         ~8000   (critic for attempt 1 + attempt 2)
  tokens_synthesis:                0       (never executed)
  tokens_memory:                   ~3300   (L0: 471 tokens × coordinator rounds)
  tokens_replay:                   0

Growth rate analysis:
  Coordinator input tokens by round: 4836 → 20492 → 24504
  Growth: round 1→2: 20492/4836 = 4.24× (super-linear ← VIOLATION)
  Cause: sub-agent result injection adds ~15K tokens per round
  Expected (linear): should grow by ~1K-2K tokens per round
```

---

## TOKEN GROWTH BOUND VIOLATION

```
Invariant: token_growth_rate < linear_bound

Observed: R0=4836, R1=20492, R2=24504
  Growth R0→R1: +15656 tokens (3.24× delta, super-linear)
  Growth R1→R2: +4012 tokens (linear after injection, acceptable)

Root cause: sub-agent result injection is O(file_content) not O(summary)
  A single read_multiple_files call can inject 10K-50K characters of file content
  This raw content is appended to coordinator.messages as a User message
  → Every subsequent coordinator API call includes this injected content

Corrective pattern: inject summary(sub_agent_result, max_tokens=500) not raw output
```

---

## TOKEN ATTRIBUTION PROPOSAL

The following fields should be tracked per-session for budget enforcement:

```rust
pub struct SessionTokenBreakdown {
    /// Tokens consumed by planner LLM calls.
    pub tokens_planning: u64,
    /// Tokens consumed by sub-agent execution (all sub-agent API calls combined).
    pub tokens_subagents: u64,
    /// Tokens consumed by coordinator synthesis rounds (tool-free).
    pub tokens_synthesis: u64,
    /// Tokens consumed by LoopCritic evaluation call.
    pub tokens_critic: u64,
    /// Tokens injected from memory context pipeline (L0-L4).
    pub tokens_memory: u64,
    /// Tokens from replayed history or compacted context.
    pub tokens_replay: u64,
    /// Tokens from tool execution results injected into context.
    pub tokens_tool_results: u64,
}
```

---

## TOKEN BUDGET GUARD AUDIT

### Current Guards

| Guard | Location | Formula | Trigger | Issue |
|-------|---------|---------|---------|-------|
| TokenBudget | budget_guards.rs:35 | `session.total_usage.total() >= max_total_tokens` | Hard stop | Correct |
| TokenHeadroom | post_batch.rs:391 | `pipeline_budget - call_input_tokens` | Early synth | **WRONG** |
| MaxRounds | convergence_controller.rs:276 | `round + 1 >= max_rounds` | Synthesize | Correct |
| CostBudget | budget_guards.rs:84 | `estimated_cost_usd >= max_cost_usd` | Hard stop | Correct |

### TokenHeadroom Bug (Critical)

```
File: agent/post_batch.rs:390-392

CURRENT (buggy):
    let tokens_remaining =
        (state.pipeline_budget as u64).saturating_sub(state.call_input_tokens);

PROBLEM:
    state.pipeline_budget = L0 context pipeline budget (~14895 tokens for deepseek-chat)
    state.call_input_tokens = total API input tokens (~20492 after sub-agent injection)

    14895 - 20492 = saturating_sub → 0
    0 < MIN_SYNTHESIS_HEADROOM (4000) → TokenHeadroom fires every round after sub-agent injection

CORRECT formula should be:
    let context_window = state.pipeline_budget as u64 + 50_000; // approx provider window
    let tokens_remaining = context_window.saturating_sub(state.call_input_tokens);
    // OR: use session.total_usage with provider's actual context window limit
```

### Hard Caps Proposal

```
Per-plan token budget:
  max_tokens_per_plan = 50_000 tokens
  → prevents token explosion from unbounded sub-agent reads

Per-step token budget:
  max_tokens_per_step = 10_000 tokens
  → limits sub-agent file read output injection

Per-critic-retry token budget:
  max_tokens_critic_retry = 5_000 tokens
  → critic should read summary, not full context
```

---

## LINEAR GROWTH ENFORCEMENT

```
Required invariant: call_input_tokens[round+1] ≤ call_input_tokens[round] × 1.5

Implementation: add to post_batch.rs after token tracking:
    if state.rounds >= 2 {
        let prev_tokens = state.call_input_tokens_prev_round;
        let growth_factor = state.call_input_tokens as f64 / prev_tokens as f64;
        if growth_factor > 1.5 {
            render_sink.warning(
                &format!("Token growth {:.1}× exceeds 1.5× linear bound", growth_factor),
                Some("Sub-agent output may be too large. Consider summarization."),
            );
            // Trigger early synthesis
        }
    }
```
