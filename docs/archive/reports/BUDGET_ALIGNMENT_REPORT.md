# BUDGET_ALIGNMENT_REPORT.md — Phase K5
# Budget & Termination Alignment Audit

Generated: 2026-02-22 | Branch: feature/sota-intent-architecture

---

## ISSUE B1 — max_rounds = 2 by Default for 3-Word Analysis Queries

**Observed:** `Warning: max rounds reached: 2`
**Source:** `intent_scorer.rs:314-315`

```rust
// Fallback: use word count as weak signal
match word_count {
    0..=4 if !q.contains('/') && !q.contains('.') => TaskScope::Conversational,
    ...
}
```

"analiza mi implementacion" = 3 words, no '/', no '.' → `Conversational` → `max_rounds=2`

### The Invariant Being Violated

```
INVARIANT: max_rounds ≥ plan.total_steps + critic_retries

Observed values:
  max_rounds = 2
  plan.total_steps = 2
  critic_retries = 1 (ReasoningConfig::max_retries=1)
  required = 2 + 1 + 1 (synthesis) = 4

  2 < 4 → INVARIANT VIOLATED
```

### Root Cause in IntentScorer

The word-count fallback `0..=4 → Conversational` is too aggressive.
"analiza mi implementacion" contains the word "analiza" which signals
a research/analysis intent but no explicit scope keyword from the keyword lists.

Correct behavior: fallback should be `SingleArtifact` (not `Conversational`) for
queries that do not match conversational greeting patterns.

### Fix for IntentScorer

```rust
// domain/intent_scorer.rs:313-319 — replace fallback:
//
// CURRENT (too aggressive):
match word_count {
    0..=4 if !q.contains('/') && !q.contains('.') => TaskScope::Conversational,
    0..=10 => TaskScope::SingleArtifact,
    ...
}
//
// PROPOSED (word count alone is insufficient; require conversational keyword match):
// Conversational scope requires BOTH: (a) short query AND (b) conversational keyword match.
// Without (b), short queries are treated as SingleArtifact, not Conversational.
match word_count {
    0..=4 if !q.contains('/') && !q.contains('.')
          && Self::contains_any(q, CONVERSATIONAL) => TaskScope::Conversational,
    0..=4 => TaskScope::SingleArtifact,  // short but non-greeting → artifact scope
    0..=10 => TaskScope::SingleArtifact,
    ...
}
```

This ensures "analiza mi implementacion" → `SingleArtifact+Light → max_rounds=4`
which satisfies the invariant for a 2-step plan with 1 critic retry.

---

## ISSUE B2 — max_rounds Not Re-Calculated on Critic Retry

**Observed:** Critic retry generates new plan but uses same `max_rounds=2`
**Source:** `repl/mod.rs` critic retry path — reuses `adjusted_limits` from first `pre_loop()`

### Expected Behavior

On critic retry, the ReasoningEngine should:
1. Re-run `pre_loop()` with hint `complexity_override = Complex` (critic signals complexity was underestimated)
2. OR add `max_critic_retries` to the remaining rounds budget

### Current State

```
First attempt: adjusted_limits.max_rounds = 2
Critic fires: retry uses same adjusted_limits → max_rounds = 2
New plan: 2 steps → needs 4 rounds → hits 2 → Step 2 orphaned again
```

### Fix

```rust
// In mod.rs, at critic retry site:
let retry_limits = AgentLimits {
    // On retry, grant additional rounds equal to plan.total_steps + 1
    max_rounds: adjusted_limits.max_rounds + new_plan.steps.len() + 1,
    ..adjusted_limits.clone()
};
// Use retry_limits for the retry agent loop, not adjusted_limits
```

---

## ISSUE B3 — Convergence_synthesize Triggers at Round 2 (Too Early)

**Observed:** `[guard] convergence_synthesize: stagnation detected` at Round 2
**Cause:** ConvergenceController.max_rounds = 2 (same as adjusted_limits.max_rounds)

### Decision Tree in ConvergenceController.observe_round()

```rust
// convergence_controller.rs:276
if round + 1 >= self.max_rounds {
    return ConvergenceAction::Synthesize;
}
```

At round=1 (Round 2 in 1-indexed display): `1 + 1 = 2 >= max_rounds(2)` → Synthesize

This fires before:
- Step 2 has been attempted
- Any synthesis directive has been injected
- The coordinator has had a chance to produce text output

### Fix

```
ConvergenceController.max_rounds should be set AFTER plan creation:
  conv_ctrl.cap_max_rounds(plan.total_steps * 2 + max_critic_retries)

This gives each plan step 2 rounds of budget before convergence fires.
```

---

## ISSUE B4 — TokenHeadroom Uses Incompatible Metric

**Observed:** `[convergence] token budget low — synthesising to avoid truncation`
**Even though** actual API context usage is 24504/64000 = 38% (well within bounds)

### Root Cause

```rust
// post_batch.rs:390-392 (BUGGY):
let tokens_remaining =
    (state.pipeline_budget as u64).saturating_sub(state.call_input_tokens);
//                ^^^^^^^^^^^^^^^^^^^^              ^^^^^^^^^^^^^^^^^^^
//  L0 context injection budget (~14895)    vs   total API input (~24504)
//  These are DIFFERENT quantities!
```

`pipeline_budget` = tokens the L0-L4 context pipeline can inject into the system prompt.
`call_input_tokens` = total tokens consumed by the API call (system + history + tools + injected).

When sub-agent results are injected as conversation messages, `call_input_tokens` grows
beyond `pipeline_budget` because conversation history is NOT bounded by the pipeline budget.

### Impact

```
pipeline_budget = 14895
call_input_tokens after sub-agent injection = 20492-24504
tokens_remaining = 14895 - 20492 = saturating_sub → 0
0 < MIN_SYNTHESIS_HEADROOM(4000) → TokenHeadroom fires every round after injection
```

### Fix

```rust
// post_batch.rs:390-392 (CORRECTED):
// Use session total against the provider's configured context window.
let provider_context_window = state.pipeline_budget as u64 * 4; // approx: pipeline ≈ 25% of window
let tokens_remaining = provider_context_window.saturating_sub(state.call_input_tokens);
// Better: read actual context window from provider config:
// let provider_context_window = session.model_context_window_tokens; // new field
```

---

## ISSUE B5 — Oscillation Detection Fires on PlanExecuteReflect Pattern

**Observed:** `auto-stopped after 2 consecutive tool state.rounds`
**Expected:** Tool→Tool pattern for coordinator reading files is NOT oscillation

### Current LoopGuard

`detect_cross_type_oscillation()` requires Tool→Text→Tool→Text over 4 rounds.
This does NOT fire with only 2 tool rounds.

However, the Halt path fires via:
1. ConvergenceController::Synthesize (B3 above)
2. TokenHeadroom (B4 above)
These combine in TerminationOracle → Halt → render "auto-stopped after 2 rounds"

The message is misleading — it implies oscillation but the actual cause is
premature convergence from B3+B4.

### Fix

```
Display the actual termination reason, not the generic "oscillation" message:
  "auto-stopped: convergence_controller max_rounds(2) reached at round 2"
  "auto-stopped: token_headroom fired (metrics mismatch — see B4)"
```

---

## ISSUE B6 — max_rounds Termination Before Plan Steps Evaluated

**Required invariant (Phase K5):**

```
max_rounds ≥ plan.total_steps + critic_retries
```

### Enforcement Gap

This invariant is NEVER checked at plan creation time.

The plan is generated AFTER `adjusted_limits` is set. By the time the Planner
returns a 2-step plan, it's too late — `max_rounds=2` is already locked in.

### Fix: Check at Plan Creation

```rust
// In agent/mod.rs, after plan creation:
if let Some(ref plan) = active_plan {
    if let Err(corrected) = BudgetInvariantChecker::check_max_rounds_invariant(
        limits.max_rounds,
        plan.steps.len(),
        reasoning_config.max_retries,
    ) {
        tracing::warn!(
            corrected_max_rounds = corrected,
            original_max_rounds = limits.max_rounds,
            "Budget invariant violated: expanding max_rounds"
        );
        limits.max_rounds = corrected;
        state.conv_ctrl.cap_max_rounds(corrected);
    }
}
```

---

## BUDGET TERMINATION AUDIT TABLE

| Signal | Location | Threshold | Correct? | Fix Required? |
|--------|---------|-----------|---------|---------------|
| `max_rounds` | agent/mod.rs outer loop | adjusted_limits.max_rounds | YES (value wrong) | YES — increase per plan |
| `ConvergenceController::Synthesize` | convergence_controller.rs:276 | max_rounds | YES (value wrong) | YES — cap after plan known |
| `TokenHeadroom` | post_batch.rs:391 | pipeline_budget - call_input | NO (wrong metric) | CRITICAL — fix formula |
| `TokenBudget` | budget_guards.rs:35 | max_total_tokens | YES | NO |
| `LoopGuard::Break` | loop_guard.rs | oscillation patterns | YES | NO |
| `should_inject_synthesis` | round_scorer.rs:245 | 2 regressions | YES | NO |
| `CriticRetry` | mod.rs | halt_confidence=0.80 | YES | Needs plan-aware check |

---

## INVARIANT ENFORCEMENT SUMMARY

```
INVARIANT K5-1: max_rounds ≥ plan.total_steps + critic_retries
  Status: VIOLATED for "analiza mi implementacion" (2 < 4)
  Enforcement: NONE currently
  Fix: BudgetInvariantChecker::check_max_rounds_invariant() at plan creation

INVARIANT K5-2: token_growth_rate < linear_bound (1.5× per round)
  Status: VIOLATED (R0=4836 → R1=20492 = 4.24× growth)
  Enforcement: NONE currently
  Fix: add growth check in post_batch.rs

INVARIANT K5-3: TokenHeadroom uses session_tokens vs provider_context_window
  Status: VIOLATED (uses pipeline_budget vs call_input_tokens)
  Enforcement: broken
  Fix: correct formula in post_batch.rs:391
```
