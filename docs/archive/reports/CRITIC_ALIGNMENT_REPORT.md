# CRITIC_ALIGNMENT_REPORT.md — Phase K4
# Critic / Evaluation Alignment Audit

Generated: 2026-02-22 | Branch: feature/sota-intent-architecture

---

## ISSUE C1 — LoopCritic Runs for ConversationalSimple Tasks

**Observed:** LoopCritic makes an extra LLM API call for "hola"
**Evidence:** Session tokens = 9126 vs round tokens = 4563 (2× overhead)

### Current Behavior (supervisor.rs)

The LoopCritic is invoked in `repl/mod.rs` unconditionally whenever
`enable_loop_critic = true`. There is no bypass condition based on task class.

### Effect on Scoring

```
For "hola" → EndTurn response:
  critic_signal = achieved=true, confidence≈0.50
  (LLM is uncertain about whether a greeting counts as "goal achieved")
  score = 0.6 × trajectory(0.603) + 0.4 × critic_signal(0.50)
         = 0.362 + 0.200 = 0.562
  success = false (below 0.60 threshold)

Expected:
  ConversationalSimple → bypass critic
  score = trajectory_score = 0.603
  success = 0.603 ≥ 0.60 → true
```

### UCB1 Feedback Corruption

```
Current: UCB1.update(task=General, strategy=DirectExecution, reward=0.562)
  → UCB1 learns DirectExecution is suboptimal for General tasks
  → Next session: UCB1 might select PlanExecuteReflect for "hola"
  → Observed in Session 2 (TUI): 2 rounds, 19.7K tokens for a greeting

Correct: UCB1.update(task=General, strategy=DirectExecution, reward=0.85+)
  → UCB1 learns DirectExecution is correct for simple tasks
```

### Fix

```rust
// In mod.rs, at LoopCritic invocation site:
let should_run_critic = reasoning_config.enable_loop_critic
    && !profile.is_conversational()  // Add this guard
    && result.full_text.len() > 100; // Skip for trivial responses
```

---

## ISSUE C2 — GoalSpec Incorrectly Applied to Conversational Context

**Observed:** "[critic] goal not fully achieved (95% confidence)" for "analiza mi implementacion"
**Context:** sub-agent returned a meta-question; critic correctly identifies failure

### This is CORRECT behavior for the "analiza" case.

The critic correctly identified that:
- The sub-agent returned clarification questions, not analysis
- No code quality, architecture, or technical content was produced

However, the critic's response is to trigger a FULL plan retry, not to:
1. Mark Step 1 as failed
2. Allow Step 2 to still execute with what was gathered

### Effect on Flow

```
Critic fires at confidence=0.95 → critic_halt=true → retry triggered
Retry: new plan created from scratch (plan_id changes)
New plan also hits max_rounds=2 → Step 2 orphaned again

Root issue: critic retry is always a FULL RESET, not targeted continuation
```

### Fix

```
When critic fires and plan.completed_steps >= 1:
  Option A: do NOT retry — synthesize from what was gathered
  Option B: retry only Step 1 (not the full plan)
  Option C: increase max_rounds for retry (not reset to same 2)
```

---

## ISSUE C3 — Evaluation Threshold Miscalibrated for Low-Output Rounds

**Observed:** Score: 0.56 — Below threshold (success_threshold=0.60)
**Root cause:** Token efficiency formula penalizes short conversational responses

### Token Efficiency Formula (round_scorer.rs:163-167)

```rust
let token_efficiency = if input_tokens == 0 {
    0.5 // neutral
} else {
    (output_tokens as f32 / input_tokens as f32).min(1.0)
};
```

For "hola": `179 / 4384 = 0.041` (4% efficiency → very low)

This metric was designed for tool-use rounds where output should be
proportional to input processing. For conversational rounds, a short
correct reply is not inefficient — it is correct behavior.

### Effect on Combined Score

```
combined_score contribution from token_efficiency:
  W_TOKEN × token_efficiency = 0.15 × 0.041 = 0.006
  vs expected: 0.15 × 0.5 (neutral) = 0.075
  delta: -0.069 points

Without this bug:
  combined_score = 0.206 + 0.069 = 0.275
  trajectory = 0.5 × 1.0 + 0.5 × 0.275 = 0.638
  score (no critic) = 0.638 → success ✓
```

### Fix

```rust
// In round_scorer.rs:score_round():
// For text-only rounds (tools_total == 0), use neutral token efficiency
let token_efficiency = if input_tokens == 0 || tools_total == 0 {
    0.5 // neutral for conversational rounds: short output is correct
} else {
    (output_tokens as f32 / input_tokens as f32).min(1.0)
};
```

---

## ISSUE C4 — Evaluation Score Not Reflecting Plan Completion

**Observed:** Score: 0.67 — Success while plan shows 1/2 steps completed
**Root cause:** CompositeEvaluator does not include plan completion ratio

### Current CompositeEvaluator (evaluator.rs)

```
score = stop_condition(0.5) + efficiency(0.2) + has_output(0.3)
```

`has_output = true` → 0.3 even when Step 2 (synthesis) never ran.

### Proposed Addition

```rust
// Add plan_completion to CompositeEvaluator:
const W_PLAN_COMPLETION: f64 = 0.15;

// Reduce stop_condition weight slightly:
const W_STOP: f64 = 0.40; // was 0.50
const W_EFFICIENCY: f64 = 0.20;
const W_COMPLETION: f64 = 0.25; // was 0.30
const W_PLAN: f64 = 0.15;      // new

score = stop × 0.40 + efficiency × 0.20 + has_output × 0.25
      + plan_completion_ratio × 0.15
```

---

## ISSUE C5 — Confidence Threshold for Critic Halt Is Too Low

**Observed:** "[critic] goal not fully achieved (95% confidence)" triggers full retry
**File:** `repl/mod.rs:2907` — `HALT_CONFIDENCE_THRESHOLD = 0.80`

At 80% threshold, any critic confidence ≥ 0.80 triggers a full plan reset.

This is appropriate for high-confidence failures (truly wrong response).
But for partial completions (1/2 steps done), 95% confidence may be
triggering too aggressively since the plan WAS making progress.

### Recommendation

```
HALT_CONFIDENCE_THRESHOLD = 0.80 (keep)
But add check: if plan.completed_steps >= plan.total_steps × 0.5:
    → do not reset plan; instead synthesize from gathered context
    → log: "critic halt suppressed: plan already 50%+ complete"
```

---

## ISSUE C6 — Bypass Rule Required for ConversationalSimple

**Phase K4 requirement:**

```
If task_class == ConversationalSimple:
   bypass PlanExecuteReflect
```

### Current State

The IntentScorer correctly identifies "hola" as Conversational and routes to DirectExecution.
However, DirectExecution still runs the LoopCritic (which should not run).

### Required Bypass Matrix

| Task Class | Plan? | Critic? | Token Efficiency Check? |
|-----------|-------|---------|------------------------|
| ConversationalSimple | No | No | No |
| SingleArtifact+Light | Optional | Yes (post-loop) | Yes |
| LocalContext+ | Yes | Yes (post-loop) | Yes |
| ProjectWide+ | Yes | Yes (in-loop) | Yes |

### Implementation

```rust
// In mod.rs, after pre_loop() analysis:
let skip_critic = profile.is_conversational()
    || (profile.scope == TaskScope::SingleArtifact
        && profile.reasoning_depth == ReasoningDepth::None);

// In round_scorer.rs:score_round():
let token_efficiency = if tools_total == 0 {
    0.5 // always neutral for text-only rounds
} else {
    (output_tokens as f32 / input_tokens as f32).min(1.0)
};
```

---

## ALIGNMENT SUMMARY

| Issue | Impact | File | Status |
|-------|--------|------|--------|
| C1: Critic runs for greetings | HIGH: 2× API call overhead, UCB1 corruption | mod.rs | Open |
| C2: Critic retry is full reset | HIGH: Step 2 orphaned | mod.rs | Open |
| C3: Token efficiency formula | MEDIUM: score inflated-down for short replies | round_scorer.rs | Open |
| C4: Score ignores plan completion | MEDIUM: partial success marked as success | evaluator.rs | Open |
| C5: Confidence threshold too broad | MEDIUM: aggressive retry on partial plans | mod.rs | Open |
| C6: No ConversationalSimple bypass | HIGH: planning overhead for greetings | mod.rs | Open |
