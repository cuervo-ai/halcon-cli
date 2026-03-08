# RUNTIME_FAILURE_ANALYSIS.md — Phase K2
# Root Cause Analysis of Observed Runtime Failures

Generated: 2026-02-22 | Branch: feature/sota-intent-architecture

---

## FAILURE F1 — Score 0.56 for "hola" (ConversationalSimple)

**Observed:** `[evaluation] Score: 0.56 — Below threshold`
**Expected:** Score ≥ 0.6 (success) for a correct greeting response

### Causal Chain

```
"hola" → IntentScorer → scope=Conversational → DirectExecution
Agent loop → deepseek-reasoner → "¡Hola! ¿En qué puedo ayudarte hoy?"
StopCondition::EndTurn → base_score = 1.0

RoundScorer.score_round(round=0, tools_succeeded=0, tools_total=0,
  output_tokens=179, input_tokens=4384, plan_progress_ratio=0.0):
  - tool_efficiency = 0.5  (neutral: no tools)
  - token_efficiency = min(179/4384, 1.0) = 0.041  ← PENALIZES SHORT RESPONSES
  - progress_score = max(0.0, 0.0) = 0.0
  - coherence_score ≈ 0.5  (no extractable keywords from "hola")
  - combined_score = 0.45×0.0 + 0.30×0.5 + 0.10×0.5 + 0.15×0.041 = 0.206

trajectory_score = 0.5×1.0 + 0.5×0.206 = 0.603

LoopCritic runs (NO bypass for ConversationalSimple) → API call #2
  → achieved=true, confidence≈0.5 (model uncertainty on greeting task)
  critic_signal = 0.5 (achieved=true path)

score = 0.6×0.603 + 0.4×0.5 = 0.362 + 0.200 = 0.562 ≈ 0.56
success = 0.562 < 0.60 → false → "Below threshold"
```

### Root Causes

**RC-F1-A: Token efficiency metric (output/input ratio) is wrong for conversational responses.**
- File: `round_scorer.rs:163-167`
- Formula: `token_efficiency = min(output_tokens/input_tokens, 1.0)`
- A short correct reply (179 tokens) to a large system-prompt context (4384 tokens) scores 0.041
- This is NOT inefficiency — it is the correct behavior for conversational tasks
- Weight W_TOKEN=0.15 pulls combined_score from 0.256 → 0.206

**RC-F1-B: LoopCritic runs unconditionally for ALL tasks.**
- File: `repl/mod.rs` critic invocation site (no scope bypass)
- Runs on "hola" greeting, consuming a full LLM API call
- Low confidence (≈0.5) from critic on trivial task pulls score: 0.603 → 0.562
- Net effect: UCB1 learns DirectExecution is suboptimal for greetings → escalates next session

**RC-F1-C: success_threshold=0.6 is too high when critic is uncertain.**
- File: `application/reasoning_engine.rs:19,28`
- Default threshold: 0.6 (ReasoningConfig::default)
- A perfect EndTurn response with no tools should always score ≥ 0.6
- The combined formula allows critic uncertainty to override a perfect stop condition

---

## FAILURE F2 — Plan Step 2 (Synthesize) Remains Pending

**Observed:** `Plan: 1/2 steps in 17.1s, 1 delegated` + Step 2 status = Pending
**Expected:** Step 2 completes with synthesis

### Causal Chain

```
"analiza mi implementacion" (3 words, no path separators)
→ IntentScorer:
    - SYSTEM_WIDE: no match
    - PROJECT_WIDE: no match ("proyecto" keyword absent)
    - LOCAL_CONTEXT: no match ("implementacion" not in LOCAL_CONTEXT keyword list)
    - CONVERSATIONAL && word_count<12: depends on CONVERSATIONAL list
    - FALLBACK: word_count=3 → 0..=4 without '/' or '.' → TaskScope::Conversational ← BUG

scope=Conversational → suggested_max_rounds=2 → adjusted_limits.max_rounds=2

Plan generated: 2 steps
  Step 1: Read and analyse (glob) → delegated to sub-agent
  Step 2: Synthesize findings → COORDINATOR must do this

After sub-agent completes Step 1 (8.8s):
  coordinator context: sub-agent result injected as User message
  Round 1 starts: coordinator → asks clarification (meta-question, not synthesis)

Critic fires: "goal not fully achieved (95% confidence)"
→ critic retry: new plan generated, new agent loop started

New plan: 2 steps (same structure)
  Sub-agent 2 completes Step 1 (7.7s)
  Coordinator Round 1: calls read_multiple_files (tool)
  Coordinator Round 2: calls read_multiple_files again (tool)

At Round 2 (round=1, 0-indexed):
  convergence_controller: round+1=2 >= max_rounds=2 → Synthesize ← FIRES
  early_convergence: pipeline_budget(14895) - call_input_tokens(24504) = 0 → TokenHeadroom ← FIRES
  loop_guard: consecutive_rounds=2, plan_progress.completed=0
    → detect_cross_type_oscillation? No (4 rounds needed, only 2 tool rounds)
    → RoundScorer.should_inject_synthesis()? regression_flag requires negative progress_delta; progress=0 → false
    → check_bayesian_anomalies? TokenExplosion might fire → ForceNoTools
  TerminationOracle: Halt → auto-stop

Step 2 never gets a round to execute.
```

### Root Causes

**RC-F2-A: IntentScorer word-count fallback misclassifies short analysis queries.**
- File: `domain/intent_scorer.rs:313-319`
- `0..=4 words without path separator → Conversational`
- "analiza mi implementacion" = 3 words → Conversational → max_rounds=2
- Invariant violated: `max_rounds(2) < plan.total_steps(2) + 1_synthesis_round`

**RC-F2-B: max_rounds is a flat counter, not plan-aware.**
- File: `application/reasoning_engine.rs:138`
- `adjusted_limits.max_rounds = plan.max_rounds.min(base_limits.max_rounds)`
- No enforcement of: `max_rounds ≥ plan.total_steps + critic_retries`
- A 2-step plan REQUIRES at minimum 2 rounds (1 per step) + 1 synthesis round

**RC-F2-C: Critic retry does not carry over plan state.**
- File: `repl/mod.rs:2920+` critic retry path
- On critic retry, the entire agent loop restarts from round=0 with max_rounds=2 (same cap)
- The orphaned Step 2 from the first plan is never attempted again
- Retry == full reset; not targeted continuation

**RC-F2-D: early_convergence TokenHeadroom uses incompatible metric.**
- File: `agent/post_batch.rs:390-392`
- `tokens_remaining = pipeline_budget(14895) - call_input_tokens(24504)`
- saturating_sub → 0 → 0 < MIN_SYNTHESIS_HEADROOM(4000) → fires prematurely
- pipeline_budget is the L0 context injection budget, NOT the API context window

---

## FAILURE F3 — auto-stopped after 2 consecutive tool rounds

**Observed:** `Warning: auto-stopped after 2 consecutive tool state.rounds (pattern detected)`
**Expected:** Loop should continue — synthesis_threshold=6 requires 6 consecutive rounds

### Causal Chain

```
TerminationOracle::Halt path fires when is_loop_guard_break=true
LoopGuard.record_round() returns Break when plan_complete=true

How plan_complete gets set:
  RoundScorer.should_inject_synthesis() needs 2 consecutive regression rounds
  regression_flag = progress_delta < -0.001

  Plan tracker progress: completed=0/total=2 → ratio=0.0 all rounds
  progress_delta = 0.0 - 0.0 = 0.0 → regression_flag=false ← does NOT fire this path

Alternative Break path:
  LoopGuard.check_bayesian_anomalies() → TokenExplosion detected → ForceNoTools
  (input tokens: 20492→24504 = 19.6% growth, possible explosion threshold)

  OR ConvergenceController.observe_round() → Synthesize fires (round+1 ≥ max_rounds=2)
  → state.convergence_directive_injected=true
  → Oracle receives ConvergenceControllerSynthesizeAction → InjectSynthesis
  → ForcedSynthesis next round

  Then next round: loop_guard.force_synthesis() sets plan_complete=true
  On record_round: plan_complete=true → Break → is_loop_guard_break=true

Render output:
  "auto-stopped after 2 consecutive tool state.rounds"
  = consecutive_rounds=2 in the Break path display message
```

### Root Causes

**RC-F3-A: ConvergenceController max_rounds fires at Round 2, forcing synthesis chain.**
- Synthesize → InjectSynthesis → ForcedByOracle → next tool-free round
- But next round hits max_rounds=2 → outer agent loop terminates

**RC-F3-B: TokenHeadroom (RC-F2-D) fires simultaneously, triggering convergence_synthesize.**
- Dual-signal at Round 2 creates redundant termination paths

---

## FAILURE F4 — Token Explosion (141K tokens for simple analysis)

**Observed:** `Tokens: ↑140.2K ↓1.2K | Total tokens: 141335`
**Expected:** Analysis of a Rust workspace should use ≤ 20K tokens

### Causal Chain

```
Session breakdown (3 agent loops):
  1. Coordinator attempt 1: ↑5167 ↓481 (~5.6K)
  2. Sub-agent 1 (Coder): reads files with glob → ~8K
  3. Coordinator attempt 2 (critic retry):
     - Sub-agent 2 (Chat): reads Cargo.toml + 12 crates → ↑20492 ↓127 (~20.6K round 1)
                                                           ↑24504 ↓106 (~24.6K round 2)
  4. LoopCritic call (for attempt 1): ~4K
  5. System prompt × 3 agent loops × 77 tools definitions × ~50 tokens/tool = ~11K
  6. Sub-agent 2 reads large files: Cargo.toml (748 lines) + multiple crates = ~80K context

Root issue: sub-agent has no file size / token limit guard.
read_multiple_files reads Cargo.toml (full), halcon-cli/src/lib.rs (full), etc.
Each read returns thousands of tokens which are injected into coordinator context.
Coordinator then makes second call with 24504 input tokens (full injected history).
```

### Root Causes

**RC-F4-A: No per-step token budget for sub-agent reads.**
- Sub-agents have no `max_output_tokens_per_tool` limit
- Large file reads (748-line Cargo.toml) fully returned → context explosion

**RC-F4-B: Coordinator context grows unboundedly from sub-agent injection.**
- Sub-agent output injected as User message → part of coordinator's conversation history
- Next coordinator round: history + new prompt = 24504 tokens (sub-agent result dominates)

**RC-F4-C: System prompt (with 77 tool definitions) included in every sub-agent API call.**
- ~77 tools × ~50 tokens/tool ≈ 3850 tokens per sub-agent call overhead
- × multiple sub-agent rounds = significant baseline overhead

---

## FAILURE F5 — Evaluation Score 0.67 Marked as Success Despite Plan Incomplete

**Observed:** `[evaluation] Score: 0.67 — Success` but plan shows 1/2 steps
**Expected:** plan incomplete → score should not be "Success"

### Root Cause

**RC-F5-A: Evaluation score is independent of plan completion.**
- `CompositeEvaluator` uses: stop_condition (0.5), efficiency (0.2), has_output (0.3)
- `has_output=true` (model produced text) → full 0.3 weight
- Plan completion ratio NOT included in CompositeEvaluator formula
- A partial response with `EndTurn` can score 0.67 while Step 2 is Pending

---

## FAILURE F6 — Sub-agent Returns Meta-question Instead of Analysis

**Observed:** Sub-agent returns "¿Qué módulo o archivo quieres que revise?"
**Expected:** Sub-agent reads files and returns analysis

### Root Cause

**RC-F6-A: Sub-agent has no output contract validator.**
- Sub-agent instruction: "Read and analyse: Buscar archivos..."
- Sub-agent calls glob → gets file list → then asks clarification
- No post-execution check: "did sub-agent call required tool?"
- No check: "did sub-agent produce content or ask a question?"

**RC-F6-B: Sub-agent goal_keywords (multilingual) may not match glob output.**
- Glob returns filenames, not prose
- Coverage estimation may score low → sub-agent asks for clarification

---

## FAILURE SUMMARY TABLE

| ID | Failure | Layer | File(s) | Severity |
|----|---------|-------|---------|----------|
| F1 | Score 0.56 for greeting | Evaluation | round_scorer.rs, reasoning_engine.rs | HIGH |
| F2 | Step 2 never executes | Budget/Planner | intent_scorer.rs, reasoning_engine.rs | CRITICAL |
| F3 | auto-stop at 2 rounds | LoopGuard | convergence_phase.rs, convergence_controller.rs | HIGH |
| F4 | 141K token explosion | Sub-agent | orchestrator.rs, post_batch.rs | HIGH |
| F5 | Success despite incomplete plan | Evaluation | evaluator.rs | MEDIUM |
| F6 | Sub-agent meta-question | Sub-agent contract | orchestrator.rs, agent/mod.rs | HIGH |
