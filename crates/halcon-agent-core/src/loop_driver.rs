//! `run_gdem_loop` — the GDEM (Goal-Driven Execution Model) agent loop.
//!
//! ## Architecture
//!
//! This is the central orchestration function that wires all GDEM layers together:
//!
//! ```text
//! run_gdem_loop(GdemContext)
//! │
//! ├─ L0  GoalSpecParser        → GoalSpec + VerifiableCriteria
//! ├─ L1  AdaptivePlanner       → PlanTree (Tree-of-Thoughts)
//! ├─ L2  SemanticToolRouter    → selected tools for this round
//! ├─ L3  [SandboxedExecutor]   → tool results (via caller's ToolExecutor)
//! ├─ L4  StepVerifier          → VerifierDecision (Achieved | Continue | Insufficient)
//! ├─ L5  InLoopCritic          → CriticSignal (Continue | InjectHint | Replan | Terminate)
//! ├─ L6  FormalAgentFSM        → validated state transitions
//! ├─ L7  VectorMemory          → episode storage + retrieval
//! ├─ L8  UCB1StrategyLearner   → strategy selection and reward recording
//! └─ L9  [DagOrchestrator]     → multi-agent sub-task execution (optional)
//! ```
//!
//! ## Design invariants (from lib.rs)
//!
//! 1. Loop exits when `GoalVerificationEngine::evaluate() >= threshold`, NOT when tools stagnate.
//! 2. `InLoopCritic` runs after every tool batch — never post-hoc.
//! 3. Tool selection is embedding-based (SemanticToolRouter), no keyword tables.
//! 4. All FSM transitions are validated by `AgentFsm`.
//! 5. Tool execution is delegated to the caller's `ToolExecutor` (sandboxed in production).

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::time::Instant;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::{
    critic::{CriticConfig, CriticSignal, InLoopCritic, RoundMetrics},
    fsm::{AgentFsm, AgentState},
    goal::{Evidence, GoalSpec, GoalSpecParser},
    memory::{Episode, MemoryConfig, VectorMemory},
    planner::{AdaptivePlanner, PlannerConfig, PlanTree},
    router::{EmbeddingProvider, RouterConfig, SemanticToolRouter},
    strategy::{StrategyLearner, StrategyLearnerConfig},
    telemetry::ToolTelemetry,
    verifier::{StepVerifier, VerifierConfig},
};

// ─── ToolExecutor trait ───────────────────────────────────────────────────────

/// Caller-provided bridge to the actual tool execution system.
///
/// In production this delegates to `halcon-sandbox::SandboxedExecutor` or
/// the existing `halcon-tools` registry. In tests a mock can be supplied.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Execute a single tool call. Returns the output text.
    async fn execute_tool(&self, tool_name: &str, input: &str) -> Result<ToolCallResult>;
}

/// Result from one tool invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub tool_name: String,
    pub output: String,
    pub is_error: bool,
    pub tokens_consumed: u32,
    pub latency_ms: u64,
}

// ─── LlmClient trait ─────────────────────────────────────────────────────────

/// Caller-provided LLM client for generating plan text and final synthesis.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Generate a completion given a system prompt and user message.
    /// Returns the assistant text and approximate token count.
    async fn complete(&self, system: &str, user: &str) -> Result<(String, u32)>;
}

// ─── GdemConfig ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct GdemConfig {
    /// Maximum rounds the loop may run. Overridden by GoalSpec if present.
    pub max_rounds: u32,
    /// Goal confidence threshold for early exit.
    pub completion_threshold: f32,
    /// How many tools to select per round.
    pub tools_per_round: usize,
    /// Whether to persist episodes to VectorMemory.
    pub enable_memory: bool,
    /// Whether to use UCB1 strategy learning.
    pub enable_strategy_learning: bool,
    /// Whether to log verbose critic/verifier decisions.
    pub verbose: bool,
    /// How many similar past episodes to inject as context.
    pub memory_top_k: usize,
}

impl Default for GdemConfig {
    fn default() -> Self {
        Self {
            max_rounds: 20,
            completion_threshold: 0.85,
            tools_per_round: 6,
            enable_memory: true,
            enable_strategy_learning: true,
            verbose: false,
            memory_top_k: 3,
        }
    }
}

// ─── GdemContext ──────────────────────────────────────────────────────────────

/// All dependencies injected into `run_gdem_loop`.
pub struct GdemContext {
    pub session_id: Uuid,
    pub config: GdemConfig,
    pub tool_executor: Arc<dyn ToolExecutor>,
    pub llm_client: Arc<dyn LlmClient>,
    pub embedding_provider: Arc<dyn EmbeddingProvider>,
    /// Optional pre-existing strategy learner (loaded from storage for cross-session learning).
    pub strategy_learner: Option<StrategyLearner>,
    /// Optional pre-existing vector memory (loaded from storage).
    pub memory: Option<VectorMemory>,
    /// Pre-registered tool registry: (name, description) pairs.
    pub tool_registry: Vec<(String, String)>,
}

// ─── GdemResult ───────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct GdemResult {
    pub session_id: Uuid,
    /// The final synthesised response to the user.
    pub response: String,
    /// Final goal confidence achieved.
    pub final_confidence: f32,
    /// Whether the goal was verified as achieved.
    pub goal_achieved: bool,
    /// Total rounds executed.
    pub rounds: u32,
    /// Total tokens consumed.
    pub tokens_consumed: u64,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
    /// Strategy that was chosen for this session.
    pub strategy_used: String,
    /// State the FSM was in when the loop exited.
    pub final_state: AgentState,
    /// Serialised strategy learner for persistence (if enabled).
    pub updated_strategy_learner_json: Option<String>,
    /// Serialised memory for persistence (if enabled).
    pub updated_memory_bytes: Option<Vec<u8>>,
}

// ─── run_gdem_loop ────────────────────────────────────────────────────────────

/// The primary GDEM agent loop entry point.
///
/// This function is the SOTA replacement for the existing `run_agent_loop` in
/// `crates/halcon-cli/src/repl/agent/mod.rs`. It wires all 10 GDEM layers and
/// guarantees goal-driven termination.
///
/// # Arguments
/// - `user_message`: the raw user input / task description
/// - `ctx`: injected dependencies (executor, LLM, memory, etc.)
///
/// # Returns
/// A [`GdemResult`] with the response, confidence score, and persistence state.
pub async fn run_gdem_loop(user_message: &str, mut ctx: GdemContext) -> Result<GdemResult> {
    let start = Instant::now();
    let session_id = ctx.session_id;

    info!(session = %session_id, intent = user_message, "GDEM loop starting");

    // ── L0: Parse goal spec ────────────────────────────────────────────────
    let parser = GoalSpecParser::default();
    let goal = parser.parse(user_message);
    let max_rounds = ctx.config.max_rounds.min(goal.max_rounds as u32);

    debug!(
        goal_id = %goal.id,
        criteria = goal.criteria.len(),
        max_rounds = max_rounds,
        threshold = goal.completion_threshold,
        "GoalSpec parsed"
    );

    // ── L6: FSM ────────────────────────────────────────────────────────────
    let mut fsm = AgentFsm::new();

    // ── L8: Strategy selection ─────────────────────────────────────────────
    let mut strategy_learner = ctx
        .strategy_learner
        .take()
        .unwrap_or_else(|| StrategyLearner::new(StrategyLearnerConfig::default()));
    let strategy_used = if ctx.config.enable_strategy_learning {
        strategy_learner.select().to_string()
    } else {
        "goal_driven".to_string()
    };
    debug!(strategy = %strategy_used, "UCB1 strategy selected");

    // ── L1: Plan ───────────────────────────────────────────────────────────
    fsm.transition(AgentState::Planning)?;
    let planner = AdaptivePlanner::new(PlannerConfig::default());
    let mut plan_tree: PlanTree = planner.plan(&goal);
    debug!(branches = plan_tree.branches.len(), "Initial plan generated");

    // ── L2: Router ─────────────────────────────────────────────────────────
    let router = SemanticToolRouter::new(
        ctx.embedding_provider.clone(),
        RouterConfig {
            top_k: ctx.config.tools_per_round,
            ..Default::default()
        },
    );
    router.register_batch(ctx.tool_registry.iter().cloned());

    // ── L4: Verifier ───────────────────────────────────────────────────────
    let verifier_config = VerifierConfig {
        verbose: ctx.config.verbose,
        ..Default::default()
    };
    let mut verifier = StepVerifier::new(goal.clone(), verifier_config);

    // ── L5: Critic ─────────────────────────────────────────────────────────
    let mut critic = InLoopCritic::new(CriticConfig::default());

    // ── Telemetry ──────────────────────────────────────────────────────────
    let telemetry = ToolTelemetry::new(session_id);

    // ── L7: Memory ─────────────────────────────────────────────────────────
    let memory = ctx
        .memory
        .take()
        .unwrap_or_else(|| VectorMemory::new(MemoryConfig::default()));

    // ── L4: Evidence accumulator ───────────────────────────────────────────
    let mut evidence = Evidence::default();
    evidence.record_assistant_text(user_message);

    // ── Loop state ─────────────────────────────────────────────────────────
    let mut round = 0u32;
    let mut total_tokens: u64 = 0;
    let mut response = String::new();
    let mut hint_injection: Option<String> = None;
    let mut pre_confidence = 0.0f32;

    // ── Main loop ──────────────────────────────────────────────────────────
    'agent: loop {
        if round >= max_rounds {
            warn!(round = round, max = max_rounds, "Max rounds reached — forcing synthesis");
            fsm.try_transition_or_terminate(AgentState::Terminating);
            break 'agent;
        }

        if plan_tree.all_exhausted() {
            warn!("All plan branches exhausted — terminating");
            fsm.try_transition_or_terminate(AgentState::Terminating);
            break 'agent;
        }

        round += 1;

        // ── Transition to Executing ────────────────────────────────────────
        if !matches!(fsm.state(), AgentState::Executing) {
            let _ = fsm.transition(AgentState::Executing);
        }

        // ── Get current plan step ─────────────────────────────────────────
        let active_steps = plan_tree.active_steps();
        let step_idx = ((round - 1) as usize).min(active_steps.len().saturating_sub(1));
        let current_step = active_steps.get(step_idx).map(|s| s.description.as_str()).unwrap_or(user_message);

        debug!(round = round, step = current_step, "Executing plan step");

        // ── L2: Route tools for this step ─────────────────────────────────
        let intent_for_routing = if let Some(hint) = &hint_injection {
            format!("{} — HINT: {}", current_step, hint)
        } else {
            current_step.to_string()
        };
        hint_injection = None;

        let tool_candidates = match router.query(&intent_for_routing).await {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "Router query failed — using empty tool set");
                Vec::new()
            }
        };

        // ── L3: Execute tools ─────────────────────────────────────────────
        let mut round_tools_used: Vec<String> = Vec::new();
        let mut had_errors = false;

        for candidate in &tool_candidates {
            let telem_id = telemetry.record_start(&candidate.name, &intent_for_routing, pre_confidence, round);
            let call_start = Instant::now();

            let result = ctx.tool_executor.execute_tool(&candidate.name, current_step).await;

            let latency_ms = call_start.elapsed().as_millis() as u64;

            match result {
                Ok(tool_result) => {
                    let is_error = tool_result.is_error;
                    total_tokens += tool_result.tokens_consumed as u64;
                    telemetry.record_end(telem_id, pre_confidence, latency_ms, is_error, tool_result.tokens_consumed);

                    if is_error {
                        had_errors = true;
                        // Record as a failed tool output (not via record_tool_success to avoid counting it)
                        evidence.tool_outputs.push((
                            tool_result.tool_name.clone(),
                            format!("[ERROR]: {}", tool_result.output),
                        ));
                    } else {
                        evidence.record_tool_success(&tool_result.tool_name, &tool_result.output);
                        round_tools_used.push(tool_result.tool_name);
                    }
                }
                Err(e) => {
                    had_errors = true;
                    telemetry.record_end(telem_id, pre_confidence, latency_ms, true, 0);
                    warn!(tool = %candidate.name, error = %e, "Tool execution failed");
                }
            }
        }

        // ── Generate LLM round response ────────────────────────────────────
        let system_prompt = build_system_prompt(&goal, round, max_rounds, &plan_tree);
        let user_context = build_user_context(&evidence, current_step, round);

        match ctx.llm_client.complete(&system_prompt, &user_context).await {
            Ok((text, tokens)) => {
                total_tokens += tokens as u64;
                evidence.record_assistant_text(&text);
                response = text;
            }
            Err(e) => {
                warn!(error = %e, "LLM completion failed in round {}", round);
            }
        }

        // ── L4: Verify ────────────────────────────────────────────────────
        let _ = fsm.transition(AgentState::Verifying);
        let verifier_decision = verifier.verify(&evidence);
        let post_confidence = verifier_decision.confidence().unwrap_or(pre_confidence);

        debug!(
            round = round,
            pre = pre_confidence,
            post = post_confidence,
            decision = ?verifier_decision.is_achieved(),
            "StepVerifier decision"
        );

        if verifier_decision.is_achieved() {
            info!(round = round, confidence = post_confidence, "Goal ACHIEVED — exiting loop");
            let _ = fsm.transition(AgentState::Converged);
            break 'agent;
        }

        // ── L5: Critic ────────────────────────────────────────────────────
        let round_metrics = RoundMetrics {
            pre_confidence,
            post_confidence,
            tools_invoked: round_tools_used.clone(),
            had_errors,
            round,
            max_rounds,
        };
        let critic_signal = critic.evaluate(&round_metrics, &goal);

        debug!(signal = critic_signal.label(), "InLoopCritic signal");

        match critic_signal {
            CriticSignal::Continue => {
                let _ = fsm.transition(AgentState::Executing);
            }
            CriticSignal::InjectHint { hint, .. } => {
                hint_injection = Some(hint);
                let _ = fsm.transition(AgentState::Executing);
            }
            CriticSignal::Replan { reason, .. } => {
                warn!(round = round, reason = %reason, "Critic triggering replan");
                let _ = fsm.transition(AgentState::Replanning);
                plan_tree = planner.replan(&goal, &plan_tree);
                critic.reset_stall();
                let _ = fsm.transition(AgentState::Planning);
                let _ = fsm.transition(AgentState::Executing);
            }
            CriticSignal::Terminate { reason } => {
                warn!(round = round, reason = %reason, "Critic terminating loop");
                let _ = fsm.transition(AgentState::Terminating);
                break 'agent;
            }
        }

        pre_confidence = post_confidence;
    }

    // ── Synthesis if not already done ─────────────────────────────────────
    if response.is_empty() || matches!(fsm.state(), AgentState::Terminating) {
        let synth_prompt = format!(
            "Based on the evidence gathered (confidence: {:.1}%), synthesise the best possible \
             answer for the goal: '{}'",
            verifier.current_confidence() * 100.0,
            goal.intent
        );
        if let Ok((synth_text, tokens)) = ctx.llm_client.complete(
            "You are a helpful assistant. Synthesise the findings.",
            &synth_prompt,
        ).await {
            total_tokens += tokens as u64;
            if !synth_text.is_empty() {
                response = synth_text;
            }
        }
    }

    let final_confidence = verifier.current_confidence();
    let goal_achieved = verifier.ever_achieved() || matches!(fsm.state(), AgentState::Converged);

    // ── L8: Record UCB1 outcome ────────────────────────────────────────────
    if ctx.config.enable_strategy_learning {
        strategy_learner.record_outcome(&strategy_used, final_confidence as f64, session_id);
    }

    // ── L7: Store episode in memory ────────────────────────────────────────
    let tools_used: Vec<String> = telemetry.all_records().into_iter().map(|r| r.tool_name).collect();
    let _episode = Episode::new(
        session_id,
        &goal.intent,
        tools_used.clone(),
        format!("Round {}/{}, confidence {:.2}", round, max_rounds, final_confidence),
        final_confidence,
        goal_achieved,
    );
    // Note: embedding is injected by the caller (requires async provider access).
    // The caller should call memory.store(episode, embedding) after this returns.

    // ── Prepare persistence data ───────────────────────────────────────────
    let updated_strategy_learner_json = if ctx.config.enable_strategy_learning {
        strategy_learner.to_json().ok()
    } else {
        None
    };

    let updated_memory_bytes = if ctx.config.enable_memory {
        memory.to_bytes().ok()
    } else {
        None
    };

    let duration_ms = start.elapsed().as_millis() as u64;

    info!(
        session = %session_id,
        rounds = round,
        confidence = final_confidence,
        achieved = goal_achieved,
        duration_ms = duration_ms,
        tokens = total_tokens,
        strategy = %strategy_used,
        "GDEM loop completed"
    );

    Ok(GdemResult {
        session_id,
        response,
        final_confidence,
        goal_achieved,
        rounds: round,
        tokens_consumed: total_tokens,
        duration_ms,
        strategy_used,
        final_state: fsm.state().clone(),
        updated_strategy_learner_json,
        updated_memory_bytes,
    })
}

// ─── Prompt builders ──────────────────────────────────────────────────────────

fn build_system_prompt(goal: &GoalSpec, round: u32, max_rounds: u32, _plan: &PlanTree) -> String {
    format!(
        "You are a goal-driven AI agent. Your primary objective: {}\n\
         \n\
         Current progress: round {}/{}\n\
         \n\
         Success criteria:\n{}\n\
         \n\
         Focus all your analysis on advancing these criteria. Do not perform \
         unrelated actions. When you have gathered sufficient evidence, synthesise \
         a precise, factual response.",
        goal.intent,
        round,
        max_rounds,
        goal.criteria
            .iter()
            .map(|c| format!("  - {}", c.description))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

fn build_user_context(evidence: &Evidence, current_step: &str, round: u32) -> String {
    let recent_outputs: String = evidence
        .tool_outputs
        .iter()
        .rev()
        .take(3)
        .rev()
        .map(|(tool, output)| {
            let display = if output.len() > 2000 {
                let head = { let mut _fcb = (800).min(output.len()); while _fcb > 0 && !output.is_char_boundary(_fcb) { _fcb -= 1; } _fcb };
                let tail = { let mut _ccb = (output.len().saturating_sub(400)).min(output.len()); while _ccb < output.len() && !output.is_char_boundary(_ccb) { _ccb += 1; } _ccb };
                format!("{}...[truncated]...{}", &output[..head], &output[tail..])
            } else {
                output.clone()
            };
            format!("[{}]: {}", tool, display)
        })
        .collect::<Vec<_>>()
        .join("\n---\n");

    format!(
        "Round {} — Current step: {}\n\
         \n\
         Tool outputs from this round:\n{}\n\
         \n\
         Based on the evidence above, respond to the goal.",
        round, current_step, recent_outputs
    )
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::EmbeddingProvider;

    struct MockLlm;

    #[async_trait]
    impl LlmClient for MockLlm {
        async fn complete(&self, _system: &str, _user: &str) -> Result<(String, u32)> {
            Ok(("SUCCESS: goal achieved".to_string(), 50))
        }
    }

    struct MockToolExec;

    #[async_trait]
    impl ToolExecutor for MockToolExec {
        async fn execute_tool(&self, tool_name: &str, _input: &str) -> Result<ToolCallResult> {
            Ok(ToolCallResult {
                tool_name: tool_name.to_string(),
                output: "SUCCESS found".to_string(),
                is_error: false,
                tokens_consumed: 20,
                latency_ms: 5,
            })
        }
    }

    struct MockEmbedder;

    #[async_trait]
    impl EmbeddingProvider for MockEmbedder {
        async fn embed(&self, text: &str) -> Result<Vec<f32>> {
            let mut v = vec![0.0f32; 4];
            for (i, b) in text.bytes().enumerate() {
                v[i % 4] += b as f32 / 255.0;
            }
            Ok(v)
        }
        fn dimension(&self) -> usize { 4 }
    }

    fn make_ctx() -> GdemContext {
        GdemContext {
            session_id: Uuid::new_v4(),
            config: GdemConfig {
                max_rounds: 3,
                completion_threshold: 0.5,
                enable_memory: false,
                enable_strategy_learning: false,
                verbose: false,
                ..Default::default()
            },
            tool_executor: Arc::new(MockToolExec),
            llm_client: Arc::new(MockLlm),
            embedding_provider: Arc::new(MockEmbedder),
            strategy_learner: None,
            memory: None,
            tool_registry: vec![
                ("bash".into(), "Execute bash commands".into()),
                ("file_read".into(), "Read file contents".into()),
            ],
        }
    }

    #[tokio::test]
    async fn loop_completes_without_panic() {
        let ctx = make_ctx();
        let result = run_gdem_loop("find all files in the project", ctx).await;
        assert!(result.is_ok(), "loop should not error: {:?}", result.err());
        let r = result.unwrap();
        assert!(r.rounds > 0);
        assert!(!r.response.is_empty());
    }

    #[tokio::test]
    async fn gdem_result_has_session_id() {
        let sid = Uuid::new_v4();
        let mut ctx = make_ctx();
        ctx.session_id = sid;
        let result = run_gdem_loop("test goal", ctx).await.unwrap();
        assert_eq!(result.session_id, sid);
    }

    #[tokio::test]
    async fn max_rounds_respected() {
        let ctx = make_ctx(); // max_rounds = 3
        let result = run_gdem_loop("an impossible goal with no tools", ctx).await.unwrap();
        assert!(result.rounds <= 3);
    }

    #[tokio::test]
    async fn tokens_tracked() {
        let ctx = make_ctx();
        let result = run_gdem_loop("read a file", ctx).await.unwrap();
        // Should consume at least some tokens (tool calls + LLM completions)
        assert!(result.tokens_consumed > 0);
    }

    #[test]
    fn build_system_prompt_contains_intent() {
        use crate::goal::{CriterionKind, VerifiableCriterion};
        let goal = GoalSpec {
            id: Uuid::new_v4(),
            intent: "find API keys".into(),
            criteria: vec![VerifiableCriterion {
                description: "API keys located".into(),
                weight: 1.0,
                kind: CriterionKind::KeywordPresence {
                    keywords: vec!["API_KEY".into()],
                },
                threshold: 0.8,
            }],
            completion_threshold: 0.8,
            max_rounds: 10,
            latency_sensitive: false,
        };
        let planner = AdaptivePlanner::new(PlannerConfig::default());
        let plan = planner.plan(&goal);
        let prompt = build_system_prompt(&goal, 1, 10, &plan);
        assert!(prompt.contains("find API keys"));
        assert!(prompt.contains("API keys located"));
    }
}
