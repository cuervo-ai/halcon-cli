# ARCHITECTURE_TRACE.md — Phase K1
# Runtime Flow Map: halcon v0.3.0

Generated: 2026-02-22 | Branch: feature/sota-intent-architecture

---

## LAYER MAP

```
User Input
  │
  ▼
[CLI Layer] halcon-cli/src/commands/chat.rs
  │  Input:  raw &str from stdin/TUI
  │  Output: ChatRequest { user_msg, provider, model, flags }
  │  Mutation: none — pure routing
  │
  ▼
[REPL / Reasoning Router] repl/mod.rs + application/reasoning_engine.rs
  │  Input:  ChatRequest
  │  Output: PreLoopAnalysis { strategy, adjusted_limits, plan }
  │  Key fn: engine.pre_loop(user_query, base_limits, provider_models)
  │  Budget:  owns max_rounds cap via adjusted_limits
  │  Mutation: StrategySelector UCB1 state updated post-loop
  │
  │  ┌─ IntentScorer::score(query) → IntentProfile
  │  │   Scope: Conversational|SingleArtifact|LocalContext|ProjectWide|SystemWide
  │  │   Depth: None|Light|Deep|Exhaustive
  │  │   suggested_max_rounds(): Conversational→2, SingleArtifact+Light→4, etc.
  │  │
  │  └─ StrategySelector::select() → DirectExecution|PlanExecuteReflect
  │      plan.max_rounds = min(strategy_rounds, profile_max)
  │      adjusted_limits.max_rounds = min(plan.max_rounds, base_limits.max_rounds)
  │
  ▼
[Planner] repl/planner.rs (called when strategy == PlanExecuteReflect)
  │  Input:  user_msg, tool_defs
  │  Output: ExecutionPlan { goal, steps: Vec<PlanStep> }
  │  Budget:  owns planning_config.timeout_secs (async bounded)
  │  Mutation: none — pure LLM call
  │  Failure: timeout → skip plan; empty plan → DirectExecution fallback
  │
  ▼
[Agent Loop] repl/agent/mod.rs  ← COORDINATOR
  │  Input:  messages, session, limits, plan (opt), tools
  │  Output: AgentLoopResult { full_text, stop_condition, round_evaluations, critic_verdict }
  │  Budget:  max_rounds (from adjusted_limits), token budget, duration, cost
  │  Mutation: LoopState (all mutable session state), messages, session.total_usage
  │
  │  Per-round sub-phases:
  │  ├─ round_setup.rs     → build ModelRequest, apply tool_decision signal
  │  ├─ provider_round.rs  → LLM API call → StreamChunks → round_text, tool_calls
  │  ├─ post_batch.rs      → execute tools, update ExecutionTracker, early_convergence check
  │  └─ convergence_phase.rs → ConvergenceController, RoundScorer, TerminationOracle, LoopGuard
  │
  ▼
[Orchestrator] repl/orchestrator.rs (when plan has delegatable steps)
  │  Input:  plan steps, tool_defs, allowed_tools (filtered subset)
  │  Output: sub-agent results → injected as User messages into coordinator context
  │  Budget:  sub_agent_timeout_secs (capped at SUB_AGENT_MAX_TIMEOUT_SECS=120s)
  │  Mutation: coordinator.messages.push(sub_agent_output_text)
  │
  ▼
[Sub-agent] repl/agent/mod.rs (nested run_agent_loop, is_sub_agent=true)
  │  Input:  focused instruction, sub-agent tool subset, sub-agent ConvergenceController
  │  Output: output_text (injected back to coordinator)
  │  Budget:  sub-agent max_rounds=6, stagnation_window=2, goal_coverage_threshold=0.10
  │  Contract: MUST execute assigned tool; MUST NOT ask clarification
  │  Failure mode: returns meta-question → coordinator cannot synthesize
  │
  ▼
[Tool Execution] repl/executor.rs + halcon-tools/*
  │  Input:  ToolCall { name, args }
  │  Output: ToolResult { content, is_error }
  │  Budget:  tool_timeout (per-tool, from config)
  │  Mutation: filesystem, shell, network (depending on tool)
  │
  ▼
[Critic / LoopCritic] repl/supervisor.rs
  │  Input:  final_response_text, original_goal
  │  Output: CriticVerdict { achieved: bool, confidence: f32, reasoning: String }
  │  Budget:  1 extra LLM call per session (always, regardless of task type)
  │  Mutation: feeds into post_loop() reward calculation
  │  State:   ALWAYS runs — no bypass for ConversationalSimple
  │
  ▼
[Evaluation / Reward Pipeline] application/reasoning_engine.rs + evaluator.rs
  │  Input:  AgentLoopResult, CriticVerdict (opt)
  │  Output: PostLoopEvaluation { score: f64, success: bool }
  │  Formula: score = 0.6×trajectory + 0.4×critic_signal
  │  Threshold: success_threshold = 0.6 (from ReasoningConfig::default())
  │
  ▼
[Memory Consolidation] repl/memory_consolidator.rs
  │  Input:  session reflections from DB
  │  Output: merged/pruned reflection count
  │  Mutation: DB write (async, fire-and-forget every 5 rounds)
  │  Risk:   runs post-loop; does NOT affect plan state
  │
  ▼
[FSM State Transition] repl/mod.rs state machine
  │  States: Idle → Executing → Planning → Executing → Complete
  │  Emits:  DomainEvent::* → TUI activity model
  │  Mutation: session saved to DB; UCB1 experience updated
```

---

## LAYER CONTRACT TABLE

| Layer | Input Contract | Output Contract | Budget Owner | State Authority |
|-------|---------------|-----------------|--------------|-----------------|
| IntentScorer | &str query | IntentProfile (scope, depth) | none | none |
| ReasoningEngine | query, base_limits | adjusted_limits (max_rounds capped) | max_rounds | UCB1 StrategySelector |
| Planner | user_msg, tools | ExecutionPlan or None | timeout_secs | none |
| Agent Loop (coord) | messages, limits, plan | AgentLoopResult | max_rounds, token, duration, cost | LoopState (all mutable) |
| Orchestrator | plan steps, tools | sub-agent output text | sub_agent_timeout | none |
| Sub-agent | instruction, sub-tools | output_text | max_rounds=6 (sub) | is_sub_agent flag |
| Tool Executor | ToolCall | ToolResult | tool_timeout | filesystem/shell |
| LoopCritic | response_text, goal | CriticVerdict | 1 LLM call | none |
| Evaluation | AgentLoopResult + CriticVerdict | PostLoopEvaluation | none | UCB1 update |

---

## STATE DIVERGENCE POINTS

### SD-1: plan_id not persisted through critic retry
- Location: `repl/mod.rs:2906` critic retry path
- The critic fires on plan_id `c33e056f`; retry generates plan_id `<new_uuid>`
- The original plan's pending Step 2 is never marked as failed — it simply orphans
- No plan lifecycle log spans both plan IDs

### SD-2: sub-agent token count vs coordinator context budget
- Location: `post_batch.rs:390-392`
- `tokens_remaining = pipeline_budget - call_input_tokens`
- `pipeline_budget` = L0 context pipeline budget (~14895 tokens)
- `call_input_tokens` = actual API input including sub-agent injected results (~20-24K)
- Result: `tokens_remaining = 0` (saturating_sub) → TokenHeadroom fires immediately

### SD-3: max_rounds applies to coordinator rounds, not plan steps
- Location: `intent_scorer.rs:144-154`
- `Conversational → max_rounds=2` (regardless of plan.total_steps)
- A 2-step plan needs ≥2 rounds just to complete Step 1 (delegation) + Step 2 (synthesis)
- Invariant violated: `max_rounds < plan.total_steps + 1`

### SD-4: LoopCritic runs unconditionally
- Location: `repl/mod.rs` critic call site
- Runs for ALL sessions including single-word greetings ("hola")
- No bypass condition for ConversationalSimple tasks
- Consumes 1 extra API call; pulls evaluation score down via low critic_signal

### SD-5: max_rounds not re-calculated on critic retry
- Location: `repl/mod.rs` critic retry path
- Retry uses same `adjusted_limits` from original `pre_loop()` call
- max_rounds = 2 (same as first attempt) → new plan also exhausted in 2 rounds

---

## WHERE TERMINATION IS EVALUATED (priority order)

1. `budget_guards.check()` — token/duration/cost hard limits (post-batch)
2. `early_convergence::ConvergenceDetector::check_with_cost()` — token headroom (post-batch)
3. `ConvergenceController::observe_round()` — max_rounds, stagnation (convergence_phase)
4. `ToolLoopGuard::record_round()` — oscillation, read saturation (convergence_phase)
5. `TerminationOracle::adjudicate()` — authoritative arbitration from all signals (convergence_phase)
6. `AgentLimits::max_rounds` check — outer loop counter (agent/mod.rs main loop)

---

## WHERE TOKEN BUDGET IS CALCULATED

| Location | Formula | Issue |
|----------|---------|-------|
| `post_batch.rs:391` | `pipeline_budget - call_input_tokens` | Compares incompatible metrics |
| `budget_guards.rs:35` | `session.total_usage.total() >= max_total_tokens` | Correct; uses session total |
| `convergence_controller.rs:276` | `round + 1 >= max_rounds` | Correct; round-based |
| `evaluator.rs:48` | `1 - rounds_used/max_rounds` | Efficiency only; not budget |

---

## WHERE CRITIC OVERRIDES EXECUTION

1. `convergence_phase.rs` LoopCritic fires AFTER loop completes (post-hoc)
2. `mod.rs:2905-2930`: critic_halt overrides score_says_retry at ≥80% confidence
3. critic retry path: drops current plan, regenerates new plan, re-runs agent loop
4. No mechanism to continue Step 2 of the original plan — retry is always full reset
