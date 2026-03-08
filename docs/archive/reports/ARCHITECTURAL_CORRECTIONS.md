# ARCHITECTURAL_CORRECTIONS.md — Phase K
# Recommended Architectural Corrections

Generated: 2026-02-22 | Branch: feature/sota-intent-architecture
Synthesizes: K1 (Architecture Trace) · K2 (Plan Diagnostics) · K3 (Token Audit)
           · K4 (Critic Alignment) · K5 (Budget Alignment) · K6 (Sub-agent Contract)

---

## Correction Priority Schema

| Severity | Meaning |
|----------|---------|
| **CRITICAL** | Correctness broken; invariant violated; observed runtime failure |
| **HIGH** | Material overhead; UCB1 learning corrupted; partial completion masked |
| **MEDIUM** | Metric inaccuracy; misleading output; long-term score drift |

---

## CORRECTION 1 — TokenHeadroom Uses Incompatible Metric

**Severity**: CRITICAL
**Layer**: Agent Loop → EarlyConvergence guard
**File**: `crates/halcon-cli/src/repl/agent/post_batch.rs` lines 390-392
**Invariant violated**: INVARIANT K5-3 (`TokenHeadroom uses session_tokens vs provider_context_window`)

### Problem

```rust
// CURRENT (post_batch.rs:390-392):
let tokens_remaining =
    (state.pipeline_budget as u64).saturating_sub(state.call_input_tokens);
```

`state.pipeline_budget` is the L0 context injection budget (~14 895 tokens for deepseek-chat).
`state.call_input_tokens` is the total API call input (system + history + injected sub-agent output).

After one sub-agent delegation, `call_input_tokens` (~24 504) exceeds `pipeline_budget` (~14 895).
`saturating_sub` returns 0. `0 < MIN_SYNTHESIS_HEADROOM(4 000)` → TokenHeadroom fires every
round after the first sub-agent completes — even though the provider context window (64 000 tokens)
is only 38% full.

### Required Correction

```rust
// post_batch.rs:390-392 — corrected:
// Use the provider's actual context window, not the pipeline injection budget.
let context_window_tokens: u64 = state
    .provider_context_window  // new field on LoopState (see Correction 7)
    .unwrap_or(64_000);       // safe default for deepseek-chat
let tokens_remaining = context_window_tokens
    .saturating_sub(state.call_input_tokens);
```

If `provider_context_window` is not yet a field, a safe interim value is:

```rust
// Interim (acceptable): pipeline_budget is ~25% of the provider window.
let context_window_tokens = (state.pipeline_budget as u64).saturating_mul(4);
let tokens_remaining = context_window_tokens.saturating_sub(state.call_input_tokens);
```

### Impact of Correction

Eliminates spurious `[convergence] token budget low — synthesising to avoid truncation` at round 2
when actual context usage is 38%. Removes the primary premature-synthesis trigger for the
"analiza mi implementacion" case.

---

## CORRECTION 2 — IntentScorer Word-Count Fallback Too Aggressive

**Severity**: CRITICAL
**Layer**: Reasoning Router → IntentScorer
**File**: `crates/halcon-cli/src/repl/domain/intent_scorer.rs` lines 313-319
**Invariant violated**: INVARIANT K5-1 (`max_rounds ≥ plan.total_steps + critic_retries`)

### Problem

```rust
// CURRENT (intent_scorer.rs:313-319):
match word_count {
    0..=4 if !q.contains('/') && !q.contains('.') => TaskScope::Conversational,
    0..=10 => TaskScope::SingleArtifact,
    ...
}
```

"analiza mi implementacion" = 3 words, no path separators → `Conversational` → `max_rounds = 2`.
The word-count alone is insufficient to determine conversational intent. Short queries that contain
task-oriented verbs ("analiza", "implementa", "revisa", "busca") are not greetings.

### Required Correction

```rust
// intent_scorer.rs:313-319 — corrected:
// ConversationalScope requires BOTH a short query AND a match in the CONVERSATIONAL keyword list.
// Without an explicit conversational keyword, short queries default to SingleArtifact.
match word_count {
    0..=4 if !q.contains('/') && !q.contains('.')
          && Self::contains_any(q, CONVERSATIONAL) => TaskScope::Conversational,
    0..=4  => TaskScope::SingleArtifact,   // short but task-oriented
    0..=10 => TaskScope::SingleArtifact,
    // existing arms follow unchanged
}
```

The `CONVERSATIONAL` keyword list already exists and covers greetings in Spanish and English
("hola", "hi", "hello", "gracias", "thanks", "ok", "sí", "no", etc.).

### Impact of Correction

"analiza mi implementacion" → `SingleArtifact+Light` → `max_rounds = 4`.
Invariant INVARIANT K5-1 (max_rounds ≥ 2 + 1 + 1 = 4) now satisfied.
"hola" → still `Conversational` (contains "hola" in CONVERSATIONAL list).

---

## CORRECTION 3 — Budget Invariant Not Enforced at Plan Creation

**Severity**: CRITICAL
**Layer**: Agent Loop → Plan Creation
**File**: `crates/halcon-cli/src/repl/agent/mod.rs` (after plan returned by Planner)
**Invariant violated**: INVARIANT K5-1 (`max_rounds ≥ plan.total_steps + critic_retries`)

### Problem

`adjusted_limits.max_rounds` is set before the Planner runs. The Planner may return a plan with
more steps than `max_rounds` can service. No check is performed at plan creation time.

### Required Correction

```rust
// agent/mod.rs — after Planner returns active_plan:
if let Some(ref plan) = active_plan {
    if let Err(corrected) = BudgetInvariantChecker::check_max_rounds_invariant(
        limits.max_rounds,
        plan.steps.len(),
        reasoning_config.max_retries,
    ) {
        tracing::warn!(
            corrected_max_rounds = corrected,
            original_max_rounds  = limits.max_rounds,
            plan_steps           = plan.steps.len(),
            max_retries          = reasoning_config.max_retries,
            "Budget invariant violated — expanding max_rounds to satisfy K5-1"
        );
        limits.max_rounds = corrected;
        state.conv_ctrl.cap_max_rounds(corrected);
    }
}
```

`BudgetInvariantChecker` is already implemented in
`crates/halcon-cli/src/repl/plan_state_diagnostics.rs` (Phase K2 deliverable).
`ConvergenceController::cap_max_rounds()` must be added (see Correction 4).

---

## CORRECTION 4 — ConvergenceController cap_max_rounds Not Called After Plan Creation

**Severity**: CRITICAL
**Layer**: Agent Loop → ConvergenceController
**File**: `crates/halcon-cli/src/repl/agent/mod.rs` — call site after Planner returns
**Invariant violated**: INVARIANT K5-1

### Problem

```rust
// convergence_controller.rs:276:
if round + 1 >= self.max_rounds {
    return ConvergenceAction::Synthesize;
}
```

When `max_rounds = 2` (Conversational intent), this fires at round index 1 (Round 2 in 1-indexed
display): `1 + 1 = 2 >= 2` → Synthesize. This precedes any plan step execution or synthesis
directive injection.

`ConvergenceController::cap_max_rounds()` **already exists** at
`convergence_controller.rs:192-194` and is already called once during agent initialization at
`agent/mod.rs:1066` to align with the reasoning engine's initial adjusted limits. However, it is
**not called again** after the Planner returns a plan — by which point the budget invariant
violation is known and `limits.max_rounds` has been corrected (Correction 3).

### Required Correction

No new method is needed. Correction 3's call to `state.conv_ctrl.cap_max_rounds(corrected)`
is sufficient — this line already references the existing method. The only requirement is that
Correction 3's block executes **after** the Planner returns and **before** the agent loop begins:

```rust
// agent/mod.rs — after Planner returns active_plan (EXISTING method, new call site):
// cap_max_rounds() is already called at line 1066 during initialization.
// This second call applies after the plan's step count is known.
limits.max_rounds = corrected;          // outer loop bound
state.conv_ctrl.cap_max_rounds(corrected); // inner semantic bound (existing method)
```

The two-layer enforcement (outer `for round in 0..limits.max_rounds` + inner
ConvergenceController check) must both be updated for the invariant to hold end-to-end.

---

## CORRECTION 5 — LoopCritic Runs Unconditionally for All Task Classes

**Severity**: HIGH
**Layer**: Reasoning Engine → LoopCritic
**File**: `crates/halcon-cli/src/repl/mod.rs` at LoopCritic invocation site
**Issue reference**: CRITIC_ALIGNMENT_REPORT C1, C6

### Problem

The LoopCritic makes an extra LLM API call unconditionally for every completed loop, including
simple conversational tasks ("hola"). For "hola":

- Doubles the token cost (4 563 → 9 126 tokens)
- Produces a low-confidence critic verdict (achieved=true, confidence≈0.50)
- Pulls the final score: `0.6×0.603 + 0.4×0.5 = 0.562` (below success_threshold=0.60)
- Corrupts UCB1: `DirectExecution` learns a reward of 0.562 instead of 0.85+ for simple tasks

### Required Correction

```rust
// mod.rs — at LoopCritic invocation site:
let should_run_critic = reasoning_config.enable_loop_critic
    && !profile.is_conversational()          // no critic for greetings
    && result.full_text.len() > 100;         // no critic for trivial one-liners

if should_run_critic {
    // existing critic invocation code
}
```

`IntentProfile::is_conversational()` returns `self.scope == TaskScope::Conversational`.
This method already exists (used in ConvergenceController).

### Bypass Matrix (Phase K4 requirement)

| Task Class | Plan? | Critic? | Token Efficiency Neutral? |
|------------|-------|---------|--------------------------|
| ConversationalSimple | No | **No** | **Yes** |
| SingleArtifact+Light | Optional | Yes (post-loop) | No |
| LocalContext+ | Yes | Yes (post-loop) | No |
| ProjectWide+ | Yes | Yes (in-loop) | No |

---

## CORRECTION 6 — Token Efficiency Formula Penalizes Short Correct Responses

**Severity**: HIGH
**Layer**: Agent Loop → RoundScorer
**File**: `crates/halcon-cli/src/repl/round_scorer.rs` lines 163-167
**Issue reference**: CRITIC_ALIGNMENT_REPORT C3, C6

### Problem

```rust
// CURRENT (round_scorer.rs:163-167):
let token_efficiency = if input_tokens == 0 {
    0.5
} else {
    (output_tokens as f32 / input_tokens as f32).min(1.0)
};
```

For "hola": `179 / 4 384 = 0.041`. This metric was designed for tool-execution rounds where
output volume should scale with input processing. For text-only rounds, a concise correct reply
is the correct behavior — not inefficiency.

Contribution to combined_score: `W_TOKEN × 0.041 = 0.15 × 0.041 = 0.006` vs
neutral contribution `0.15 × 0.5 = 0.075`. Delta: −0.069 points, causing the 0.56 score.

### Required Correction

```rust
// round_scorer.rs:163-167 — corrected:
// Text-only rounds (no tool calls) use neutral efficiency.
// Short correct answers are not inefficient; the metric is only meaningful for tool rounds.
let token_efficiency = if input_tokens == 0 || tools_total == 0 {
    0.5  // neutral: text-only round, short output is correct behavior
} else {
    (output_tokens as f32 / input_tokens as f32).min(1.0)
};
```

`tools_total` is already computed earlier in `score_round()` as the sum of all tool call counts.

### Impact of Correction

For "hola" (tools_total=0): `token_efficiency = 0.5`.
`combined_score = 0.45×0.0 + 0.30×0.5 + 0.10×0.5 + 0.15×0.5 = 0.275`.
`trajectory = 0.5×1.0 + 0.5×0.275 = 0.638`.
Without critic: `score = 0.638 ≥ 0.60` → success. With Correction 5 (skip critic): confirmed.

---

## CORRECTION 7 — Critic Retry Reuses Same max_rounds Budget

**Severity**: HIGH
**Layer**: Agent Loop → Critic Retry path
**File**: `crates/halcon-cli/src/repl/mod.rs` at critic retry invocation site
**Issue reference**: BUDGET_ALIGNMENT_REPORT B2, CRITIC_ALIGNMENT_REPORT C2, C5

### Problem

```
First attempt: adjusted_limits.max_rounds = 2
Critic fires (confidence=0.95 > HALT_CONFIDENCE_THRESHOLD=0.80)
Retry: new plan generated — but uses same adjusted_limits → max_rounds = 2
New plan also has 2 steps → Step 2 orphaned again
```

The retry always performs a full plan reset (new plan_id, step_index=0) with the same round budget
that already proved insufficient.

### Required Correction

```rust
// mod.rs — at critic retry site:
// Option A (minimal): grant additional rounds proportional to new plan size.
let retry_limits = AgentLimits {
    max_rounds: adjusted_limits.max_rounds + new_plan.steps.len() + 1,
    ..adjusted_limits.clone()
};
// Use retry_limits for the retry agent loop, not adjusted_limits.

// Option B (suppression): if plan is ≥50% complete, do not retry — synthesize instead.
if plan.completed_steps >= plan.total_steps / 2 {
    tracing::info!(
        completed = plan.completed_steps,
        total     = plan.total_steps,
        "Critic halt suppressed: plan already ≥50% complete — synthesizing from gathered context"
    );
    // inject synthesis directive instead of reset
}
```

Option B is preferred when partial results exist (avoids token explosion on retry).
Option A is acceptable when completed_steps == 0 (true failure, not partial).

---

## CORRECTION 8 — Sub-agent Output Contract Not Validated

**Severity**: HIGH
**Layer**: Orchestrator → Sub-agent result handling
**File**: `crates/halcon-cli/src/repl/orchestrator.rs` at sub-agent result processing
**Issue reference**: K6 investigation — sub-agent returns meta-questions instead of analysis

### Problem

Sub-agents have no enforced output contract. A sub-agent that calls `glob`, receives a file list,
and then returns clarification questions ("¿qué módulo quieres que revise?") is accepted as a
completed step. No validation runs before injecting this output into coordinator context.

### Required Correction

Wire `SubAgentContractValidator` (Phase K6 deliverable,
`crates/halcon-cli/src/repl/subagent_contract_validator.rs`) at the sub-agent result site:

```rust
// orchestrator.rs — after sub-agent execution completes:
let contract = SubAgentContract::from_step(&plan_step, &sub_agent_result.tools_used);
let validation = SubAgentContractValidator::validate(&sub_agent_result.output_text, &contract);

if let ValidationStatus::Rejected(reason) = validation.status {
    if validation.is_recoverable() {
        // Inject corrective prompt into coordinator context
        let corrective = SubAgentContractValidator::corrective_prompt(
            &contract,
            &reason,
            &sub_agent_result.output_text,
        );
        coordinator_messages.push(Message::user(corrective));
        tracing::warn!(
            step    = plan_step.description,
            reason  = ?reason,
            "Sub-agent output rejected — injecting corrective prompt"
        );
    } else {
        tracing::error!(
            step   = plan_step.description,
            reason = ?reason,
            "Sub-agent output rejected (non-recoverable) — marking step failed"
        );
        plan.mark_step_failed(step_index, format!("{:?}", reason));
    }
}
```

---

## CORRECTION 9 — Sub-agent Output Injection Unbounded

**Severity**: HIGH
**Layer**: Agent Loop → Sub-agent output injection
**File**: `crates/halcon-cli/src/repl/agent/mod.rs` at sub-agent result injection site
**Issue reference**: TOKEN_AUDIT_REPORT — 4.24× token growth (4 836 → 20 492 tokens in one round)

### Problem

When a sub-agent completes, its full raw output is injected as a User message into the coordinator
conversation. A sub-agent that reads large files (e.g., 748-line Cargo.toml plus multiple `.rs`
files) injects 15 000+ tokens in a single round, causing super-linear token growth and triggering
the (already broken) TokenHeadroom guard.

### Required Correction

```rust
// agent/mod.rs — at sub-agent output injection:
const MAX_SUBAGENT_INJECTION_TOKENS: usize = 2_000; // ~8K characters

let injected_text = if sub_agent_output.len() > MAX_SUBAGENT_INJECTION_TOKENS * 4 {
    // Truncate with head+tail pattern (preserves both start and end context)
    let head = &sub_agent_output[..MAX_SUBAGENT_INJECTION_TOKENS * 3];
    let tail_start = sub_agent_output.len().saturating_sub(MAX_SUBAGENT_INJECTION_TOKENS);
    let tail = &sub_agent_output[tail_start..];
    format!(
        "{}\n\n[... {} chars omitted for context budget ...]\n\n{}",
        head,
        sub_agent_output.len() - MAX_SUBAGENT_INJECTION_TOKENS * 4,
        tail
    )
} else {
    sub_agent_output.clone()
};

coordinator_messages.push(Message::user(format!(
    "[Sub-agent step '{}' result]\n{}",
    plan_step.description, injected_text
)));
```

This bounds injection at ~8K chars (≈2 000 tokens), keeping growth under the 1.5× linear bound.

---

## CORRECTION 10 — PlanLifecycleLog and BudgetInvariantChecker Not Registered

**Severity**: MEDIUM
**Layer**: Build / Module system
**File**: `crates/halcon-cli/src/repl/mod.rs`

### Problem

`plan_state_diagnostics.rs` and `subagent_contract_validator.rs` (Phase K deliverables) are not
yet declared as modules in `repl/mod.rs`. They compile in isolation but are not available to the
agent loop.

### Required Correction

```rust
// repl/mod.rs — add after existing pub(crate) mod declarations:
pub(crate) mod plan_state_diagnostics;
pub(crate) mod subagent_contract_validator;
```

---

## CORRECTION 11 — CompositeEvaluator Ignores Plan Completion Ratio

**Severity**: MEDIUM
**Layer**: Reasoning Engine → CompositeEvaluator
**File**: `crates/halcon-cli/src/repl/evaluator.rs`
**Issue reference**: CRITIC_ALIGNMENT_REPORT C4

### Problem

```
// CURRENT:
score = stop(0.5) + efficiency(0.2) + has_output(0.3)
```

`has_output = true` (0.3 points) when Step 2 (Synthesize) never ran. A plan that completes 1/2
steps and produces any text output receives 0.67 — marked as success even though synthesis did
not execute.

### Required Correction

```rust
// evaluator.rs — add plan_completion_ratio to CompositeEvaluator:
const W_STOP:   f64 = 0.40; // was 0.50
const W_EFFIC:  f64 = 0.20;
const W_OUTPUT: f64 = 0.25; // was 0.30
const W_PLAN:   f64 = 0.15; // new

let plan_ratio = match plan_progress {
    Some(p) if p.total_steps > 0 =>
        p.completed_steps as f64 / p.total_steps as f64,
    _ => 1.0, // no plan (DirectExecution) → treat as fully complete
};

score = stop_score    * W_STOP
      + effic_score   * W_EFFIC
      + has_output    * W_OUTPUT
      + plan_ratio    * W_PLAN;
```

With this correction, a 1/2-step completion scores `plan_ratio=0.5`:
`score = 0.4×0.4 + 0.2×0.8 + 0.25×1.0 + 0.15×0.5 = 0.160 + 0.160 + 0.250 + 0.075 = 0.645`
(still passes threshold). A 0/2-step plan would score:
`= 0.4×0.4 + 0.2×0.3 + 0.25×0.0 + 0.15×0.0 = 0.160 + 0.060 = 0.220` (correctly fails).

---

## CORRECTION 12 — Misleading Termination Message

**Severity**: MEDIUM
**Layer**: Agent Loop → Termination display
**File**: `crates/halcon-cli/src/repl/mod.rs` at BreakLoop render site
**Issue reference**: BUDGET_ALIGNMENT_REPORT B5

### Problem

The message "auto-stopped after 2 consecutive tool state.rounds (pattern detected)" implies
oscillation was detected by LoopGuard. The actual cause is premature convergence (Corrections 1+4
above) routed through TerminationOracle → Halt.

### Required Correction

Carry the `TerminationDecision` variant and the originating signal through to the render site:

```rust
// At BreakLoop render:
let stop_reason = match termination_cause {
    TerminationCause::ConvergenceMaxRounds { round, max } =>
        format!("max rounds reached ({}/{}) — convergence limit", round, max),
    TerminationCause::TokenHeadroom { used, window } =>
        format!("token budget ({}/{} tokens) — synthesising to avoid truncation", used, window),
    TerminationCause::OscillationDetected { pattern } =>
        format!("oscillation detected ({})", pattern),
    TerminationCause::BudgetExhausted =>
        "token budget exhausted".to_string(),
    // ...
};
render_sink.warning(&format!("auto-stopped: {}", stop_reason), None);
```

This requires threading a `TerminationCause` enum through `PhaseOutcome::BreakLoop(cause)`.

---

## IMPLEMENTATION ORDER

The corrections above should be applied in the following order to avoid cascading test failures:

```
Phase 1 — Score and Critic Correctness (no architectural changes required):
  [6] Token efficiency neutral for text-only rounds  (round_scorer.rs, 1 line)
  [5] LoopCritic bypass for ConversationalSimple     (mod.rs, 3 lines)
  [2] IntentScorer word-count + keyword guard        (intent_scorer.rs, 2 lines)

Phase 2 — Budget Invariant Enforcement:
  [4] ConvergenceController::cap_max_rounds()        (convergence_controller.rs, new method)
  [3] Budget invariant check at plan creation         (agent/mod.rs, ~10 lines)
  [1] TokenHeadroom formula fix                      (post_batch.rs, 2 lines)

Phase 3 — Retry and Synthesis Correctness:
  [7] Critic retry extends max_rounds                (mod.rs, ~5 lines)
  [10] Register new modules                          (repl/mod.rs, 2 lines)

Phase 4 — Sub-agent Contract:
  [9] Sub-agent injection bounded                    (agent/mod.rs, ~15 lines)
  [8] SubAgentContractValidator wired                (orchestrator.rs, ~20 lines)

Phase 5 — Scoring and UX:
  [11] CompositeEvaluator plan completion ratio      (evaluator.rs, ~8 lines)
  [12] Accurate termination message                  (mod.rs, enum + render, ~30 lines)
```

---

## INVARIANT ENFORCEMENT AFTER ALL CORRECTIONS

```
INVARIANT K5-1: max_rounds ≥ plan.total_steps + critic_retries
  Before: VIOLATED (2 < 4 for "analiza mi implementacion")
  After Corrections 2+3+4: ENFORCED at plan creation; ConvergenceController updated

INVARIANT K5-2: token_growth_rate < 1.5× per round
  Before: VIOLATED (4836→20492 = 4.24×)
  After Correction 9: injection bounded at ~2000 tokens → growth ≤ 1.5×

INVARIANT K5-3: TokenHeadroom uses session_tokens vs provider_context_window
  Before: VIOLATED (pipeline_budget vs call_input_tokens — different quantities)
  After Correction 1: uses provider context window (64K for deepseek-chat)

INVARIANT K4-1: ConversationalSimple bypasses LoopCritic
  Before: VIOLATED (critic runs unconditionally)
  After Correction 5: bypassed when profile.is_conversational()

INVARIANT K4-2: token_efficiency neutral for text-only rounds
  Before: VIOLATED (0.041 for "hola" → score 0.56)
  After Correction 6: neutral (0.5) for tools_total == 0

INVARIANT K6-1: Sub-agent output passes contract validation before injection
  Before: VIOLATED (meta-questions accepted as completed steps)
  After Correction 8: rejected and corrective prompt injected

INVARIANT K6-2: Sub-agent output injection ≤ MAX_SUBAGENT_INJECTION_TOKENS
  Before: VIOLATED (unbounded; 15K+ tokens injected in one round)
  After Correction 9: bounded at ~2000 tokens
```

---

## REGRESSION RISK ASSESSMENT

| Correction | Risk | Reason |
|-----------|------|--------|
| 1 (TokenHeadroom) | LOW | Only affects guard threshold; does not change agent logic |
| 2 (IntentScorer) | LOW | "analiza" → SingleArtifact; "hola" → still Conversational (keyword match) |
| 3 (Budget invariant) | LOW | Only increases max_rounds; never decreases |
| 4 (cap_max_rounds) | LOW | Additive method; existing tests unaffected |
| 5 (skip critic) | MEDIUM | Changes UCB1 reward signal; rerun UCB1 convergence tests |
| 6 (token efficiency) | LOW | Neutral value replaces biased value for no-tool rounds |
| 7 (retry max_rounds) | MEDIUM | Changes retry loop budget; test critic retry path explicitly |
| 8 (contract validator) | LOW | New validation path; rejections add corrective prompts only |
| 9 (injection bound) | MEDIUM | Truncates sub-agent output; verify synthesis quality with bounded input |
| 10 (module registration) | LOW | Compile-only change |
| 11 (evaluator plan ratio) | MEDIUM | Changes score formula; calibrate success_threshold if needed |
| 12 (termination message) | LOW | Display-only change; requires TerminationCause enum threading |
