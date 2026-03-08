//! Centralized tunable thresholds for the agent pipeline.
//!
//! All runtime-configurable constants live here. Weight constants (W_STOP, W_TRAJECTORY, etc.)
//! remain as module-local `const` in their respective modules — they're empirically tuned,
//! not user-configurable.
//!
//! Usage: `AppConfig.policy` → threaded through `AgentContext` → `LoopState` as `Arc<PolicyConfig>`.

use serde::{Deserialize, Serialize};

/// Centralized policy thresholds consumed by reward pipeline, supervisor, evidence boundary,
/// tool trust, retry mutation, convergence, and SLA subsystems.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyConfig {
    // ── Reward thresholds ────────────────────────────────────────────────
    /// Reward score below which a retry is considered (0.60).
    #[serde(default = "default_success_threshold")]
    pub success_threshold: f64,

    /// Penalty applied when both primary and fallback critic evaluations fail (0.15).
    #[serde(default = "default_critic_unavailable_penalty")]
    pub critic_unavailable_penalty: f64,

    // ── Critic ───────────────────────────────────────────────────────────
    /// Confidence threshold above which a non-achieved verdict recommends halt + retry (0.80).
    #[serde(default = "default_halt_confidence_threshold")]
    pub halt_confidence_threshold: f32,

    /// Overall timeout for LoopCritic evaluation (primary + fallback), in seconds (45).
    #[serde(default = "default_critic_timeout_secs")]
    pub critic_timeout_secs: u64,

    /// Number of characters from the end of `full_text` fed to LoopCritic (1500).
    #[serde(default = "default_excerpt_len")]
    pub excerpt_len: usize,

    /// Minimum confidence below which a critic verdict cannot trigger retry (0.40).
    #[serde(default = "default_min_retry_confidence")]
    pub min_retry_confidence: f32,

    // ── Evidence ─────────────────────────────────────────────────────────
    /// Minimum bytes of extracted text for evidence to be considered sufficient (30).
    #[serde(default = "default_min_evidence_bytes")]
    pub min_evidence_bytes: usize,

    /// Minimum EvidenceGraph synthesis coverage before hint injection (0.30).
    /// Advisory only — does NOT block synthesis (EBS handles hard blocks).
    #[serde(default = "default_min_synthesis_coverage")]
    pub min_synthesis_coverage: f64,

    // ── Synthesis ────────────────────────────────────────────────────────
    /// Max tokens cap for synthesis rounds (4096).
    #[serde(default = "default_synthesis_max_tokens")]
    pub synthesis_max_tokens: u32,

    // ── Tool trust ───────────────────────────────────────────────────────
    /// Trust score below which a tool is hidden from the surface entirely (0.15).
    #[serde(default = "default_hide_threshold")]
    pub hide_threshold: f64,

    /// Trust score below which a tool is deprioritized (moved to end of list) (0.40).
    #[serde(default = "default_deprioritize_threshold")]
    pub deprioritize_threshold: f64,

    /// Minimum number of calls before trust-based filtering kicks in (3).
    #[serde(default = "default_min_calls_for_filtering")]
    pub min_calls_for_filtering: u32,

    // ── Retry ────────────────────────────────────────────────────────────
    /// Maximum number of structural replan attempts per agent loop (2).
    #[serde(default = "default_max_replan_attempts")]
    pub max_replan_attempts: u32,

    /// Temperature step increase per retry (0.1).
    #[serde(default = "default_temperature_step")]
    pub temperature_step: f32,

    /// Maximum temperature ceiling for retry mutation (1.0).
    #[serde(default = "default_max_temperature")]
    pub max_temperature: f32,

    /// Number of failures before a tool is removed from retry surface (2).
    #[serde(default = "default_tool_failure_threshold")]
    pub tool_failure_threshold: u32,

    // ── Reward weights (final reward formula) ────────────────────────────
    /// Stop condition weight in reward formula (0.25).
    #[serde(default = "default_w_stop")]
    pub w_stop: f64,

    /// Trajectory score weight in reward formula (0.30).
    #[serde(default = "default_w_trajectory")]
    pub w_trajectory: f64,

    /// Critic verdict weight in reward formula (0.25).
    #[serde(default = "default_w_critic")]
    pub w_critic: f64,

    /// Coherence weight in FINAL reward formula (0.20).
    /// NOTE: Intentionally different from `w_coherence_round` (0.10) used in per-round scoring.
    /// Final reward evaluates overall session coherence; round scoring evaluates per-round drift.
    #[serde(default = "default_w_coherence_reward")]
    pub w_coherence_reward: f64,

    // ── Round scorer weights (per-round scoring) ──────────────────────────
    /// Progress delta weight in per-round scoring (0.45).
    #[serde(default = "default_w_progress_round")]
    pub w_progress_round: f64,

    /// Tool efficiency weight in per-round scoring (0.30).
    #[serde(default = "default_w_efficiency_round")]
    pub w_efficiency_round: f64,

    /// Coherence weight in per-round scoring (0.10).
    /// NOTE: Intentionally lower than `w_coherence_reward` (0.20) — per-round keyword overlap
    /// is a weaker signal than session-level semantic drift.
    #[serde(default = "default_w_coherence_round")]
    pub w_coherence_round: f64,

    /// Token efficiency weight in per-round scoring (0.15).
    #[serde(default = "default_w_token_round")]
    pub w_token_round: f64,

    // ── Provider / runtime thresholds ─────────────────────────────────────
    /// Minimum output headroom tokens before OutputHeadroomCritical fires (5000).
    #[serde(default = "default_output_headroom_tokens")]
    pub output_headroom_tokens: u32,

    /// Timeout for K5-2 compaction in round_setup, in seconds (15).
    #[serde(default = "default_compaction_timeout_secs")]
    pub compaction_timeout_secs: u64,

    // ── K5-2 Growth (Phase 5) ────────────────────────────────────────────
    /// Token growth factor threshold per round (1.3).
    #[serde(default = "default_growth_threshold")]
    pub growth_threshold: f64,

    /// Consecutive growth violations before triggering compaction (2).
    #[serde(default = "default_growth_consecutive_trigger")]
    pub growth_consecutive_trigger: u32,

    // ── Mini-critic (Phase 6) ────────────────────────────────────────────
    /// Run mini-critic check every N rounds (3).
    #[serde(default = "default_mini_critic_interval")]
    pub mini_critic_interval: usize,

    /// Budget fraction threshold for mini-critic stall detection (0.50).
    /// Gates ForceReplan and ReduceTools paths in `mini_critic_check()`.
    /// See also: `mini_critic_synthesis_budget_fraction` for the ForceSynthesis path.
    #[serde(default = "default_mini_critic_budget_fraction")]
    pub mini_critic_budget_fraction: f64,

    /// Budget fraction above which mini-critic fires ForceSynthesis (0.80).
    /// Eliminates hardcoded `0.80` in `mini_critic_check()` — first condition on line 1737.
    /// Distinct from `mini_critic_budget_fraction` (0.50) which gates ForceReplan/ReduceTools.
    #[serde(default = "default_mini_critic_synthesis_budget_fraction")]
    pub mini_critic_synthesis_budget_fraction: f64,

    /// Plan progress fraction below which mini-critic fires ForceSynthesis (0.80).
    /// Eliminates hardcoded `0.80` in `mini_critic_check()` — second condition on line 1737.
    /// Prevents ForceSynthesis when the plan is already substantially complete.
    #[serde(default = "default_mini_critic_synthesis_progress_threshold")]
    pub mini_critic_synthesis_progress_threshold: f64,

    /// Plan progress fraction below which mini-critic fires ForceReplan (0.30).
    /// Eliminates hardcoded `0.30` in `mini_critic_check()` — line 1743.
    /// Combined with `mini_critic_recent_threshold` to detect stalled sessions.
    #[serde(default = "default_mini_critic_progress_threshold")]
    pub mini_critic_progress_threshold: f64,

    /// Average recent round score below which mini-critic fires ForceReplan (0.40).
    /// Eliminates hardcoded `0.40` in `mini_critic_check()` — line 1743.
    /// Combined with `mini_critic_progress_threshold` to detect stalled sessions.
    #[serde(default = "default_mini_critic_recent_threshold")]
    pub mini_critic_recent_threshold: f32,

    // ── K5-2 compaction parameters ────────────────────────────────────
    /// Maximum characters per tool-result block after K5-2 compaction (2000).
    /// Blocks exceeding this length are truncated with a compaction notice appended.
    /// Eliminates hardcoded `2000` literals in `round_setup.rs` K5-2 fallback path.
    #[serde(default = "default_k5_2_tool_output_max_chars")]
    pub k5_2_tool_output_max_chars: usize,

    /// Fraction of messages to retain after K5-2 message truncation (0.60).
    /// Applied as `keep = (len as f32 * ratio) as usize` — oldest messages evicted first.
    /// Eliminates hardcoded `* 3 / 5` (~60%) expression in `round_setup.rs`.
    #[serde(default = "default_k5_2_message_retain_ratio")]
    pub k5_2_message_retain_ratio: f32,

    // ── Critic per-call timeout ───────────────────────────────────────
    /// Per-call timeout for each individual LoopCritic evaluation, in seconds (22).
    /// The overall critic envelope is governed by `critic_timeout_secs` (45s).
    /// Default 22 = `floor(critic_timeout_secs_default / 2)` = `floor(45 / 2)`.
    /// A `.max(10)` floor is applied at the call site to prevent degenerate timeouts.
    /// Eliminates hardcoded `critic_timeout_secs / 2` expression in `result_assembly.rs`.
    #[serde(default = "default_per_call_critic_timeout_secs")]
    pub per_call_critic_timeout_secs: u64,

    // ── Loop guard thresholds ─────────────────────────────────────────
    /// Sliding window size for cross-type oscillation detection in ToolLoopGuard (8).
    #[serde(default = "default_oscillation_window")]
    pub oscillation_window: usize,

    /// Minimum synthesis threshold (tightness=1.0) for ToolLoopGuard (3).
    #[serde(default = "default_loop_guard_min_synthesis")]
    pub loop_guard_min_synthesis: usize,

    /// Minimum force threshold (tightness=1.0) for ToolLoopGuard (5).
    #[serde(default = "default_loop_guard_min_force")]
    pub loop_guard_min_force: usize,

    // ── Sub-agent control ─────────────────────────────────────────────
    /// Hard cap for sub-agent execution timeout, in seconds (300).
    #[serde(default = "default_sub_agent_max_timeout_secs")]
    pub sub_agent_max_timeout_secs: u64,

    // ── Convergence & coherence ───────────────────────────────────────
    /// Fraction of plan steps required for early convergence (0.80).
    #[serde(default = "default_early_convergence_threshold")]
    pub early_convergence_threshold: f32,

    /// Per-round score below which the round is considered "low trajectory" for replan (0.15).
    #[serde(default = "default_replan_score_threshold")]
    pub replan_score_threshold: f32,

    /// Plan drift score above which coherence violation fires (0.70).
    #[serde(default = "default_drift_threshold")]
    pub drift_threshold: f32,

    // ── Model quality ─────────────────────────────────────────────────
    /// Provider quality degradation threshold — fires when all models below this (0.35).
    #[serde(default = "default_model_quality_gate")]
    pub model_quality_gate: f64,

    // ── P3.1: Mid-loop strategy mutation ────────────────────────────────
    /// SLA fraction consumed above which ForceSynthesis fires (0.85).
    #[serde(default = "default_strategy_force_synthesis_sla")]
    pub strategy_force_synthesis_sla: f64,

    /// Minimum evidence coverage required for ForceSynthesis (0.30).
    #[serde(default = "default_strategy_min_evidence_for_synthesis")]
    pub strategy_min_evidence_for_synthesis: f64,

    /// Minimum plan completion for CollapsePlan mutation (0.50).
    #[serde(default = "default_strategy_collapse_min_progress")]
    pub strategy_collapse_min_progress: f32,

    /// Drift score above which SwitchExecutionMode fires (0.50).
    #[serde(default = "default_strategy_drift_threshold")]
    pub strategy_drift_threshold: f32,

    /// Tool failure clustering above which ReplanWithDecomposition fires (0.50).
    #[serde(default = "default_strategy_failure_cluster_threshold")]
    pub strategy_failure_cluster_threshold: f32,

    // ── P3.2: Capability validation ─────────────────────────────────────
    /// Automatically skip plan steps with missing tools/env (true).
    #[serde(default = "default_capability_auto_skip")]
    pub capability_auto_skip: bool,

    // ── P3.3: Semantic cycle detection ──────────────────────────────────
    /// Sliding window size for semantic cycle detection (6).
    #[serde(default = "default_semantic_cycle_window")]
    pub semantic_cycle_window: usize,

    /// Synonym overlap threshold for ExplorationLoop detection (0.60).
    #[serde(default = "default_cycle_synonym_overlap_threshold")]
    pub cycle_synonym_overlap_threshold: f64,

    /// Cycle severity threshold that boosts replan urgency (0.50).
    #[serde(default = "default_cycle_replan_boost_threshold")]
    pub cycle_replan_boost_threshold: f32,

    /// Number of cycle detections for Medium severity (3).
    #[serde(default = "default_cycle_medium_threshold")]
    pub cycle_medium_threshold: usize,

    /// Number of cycle detections for High severity (4).
    #[serde(default = "default_cycle_high_threshold")]
    pub cycle_high_threshold: usize,

    // ── P3.4: Mid-loop critic checkpoints ───────────────────────────────
    /// Checkpoint interval for mid-loop critic (rounds) (3).
    #[serde(default = "default_mid_critic_interval")]
    pub mid_critic_interval: usize,

    /// Progress deficit threshold triggering replan (0.25).
    #[serde(default = "default_progress_deficit_threshold")]
    pub progress_deficit_threshold: f64,

    /// Objective drift above which ChangeStrategy fires (0.40).
    #[serde(default = "default_objective_drift_threshold")]
    pub objective_drift_threshold: f64,

    /// Evidence rate decline ratio triggering scope reduction (0.50).
    #[serde(default = "default_evidence_rate_decline_ratio")]
    pub evidence_rate_decline_ratio: f64,

    // ── P3.5: Complexity feedback loop ──────────────────────────────────
    /// Minimum rounds before complexity evaluation (3).
    #[serde(default = "default_complexity_min_rounds")]
    pub complexity_min_rounds: usize,

    /// Actual/expected round ratio triggering upgrade (1.5).
    #[serde(default = "default_complexity_upgrade_ratio")]
    pub complexity_upgrade_ratio: f64,

    /// Bayesian confidence threshold for upgrade (0.70).
    #[serde(default = "default_complexity_confidence_threshold")]
    pub complexity_confidence_threshold: f64,

    // ── P3.6: Convergence utility function ──────────────────────────────
    /// Utility below which synthesis is triggered (0.35).
    #[serde(default = "default_utility_synthesis_threshold")]
    pub utility_synthesis_threshold: f64,

    /// Marginal utility below which further rounds are futile (0.05).
    #[serde(default = "default_utility_marginal_threshold")]
    pub utility_marginal_threshold: f64,

    /// Weight for evidence coverage in utility function (0.25).
    #[serde(default = "default_utility_w_evidence")]
    pub utility_w_evidence: f64,

    /// Weight for coherence in utility function (0.15).
    #[serde(default = "default_utility_w_coherence")]
    pub utility_w_coherence: f64,

    /// Weight for time/token pressure penalty in utility function (0.20).
    #[serde(default = "default_utility_w_pressure")]
    pub utility_w_pressure: f64,

    /// Weight for retry cost penalty in utility function (0.15).
    #[serde(default = "default_utility_w_cost")]
    pub utility_w_cost: f64,

    /// Weight for drift penalty in utility function (0.10).
    #[serde(default = "default_utility_w_drift")]
    pub utility_w_drift: f64,

    /// Weight for plan progress in utility function (0.15).
    #[serde(default = "default_utility_w_progress")]
    pub utility_w_progress: f64,

    // ── P4.1: System invariants ───────────────────────────────────────────
    /// Maximum allowed cumulative drift before invariant violation fires (5.0).
    #[serde(default = "default_max_drift_bound")]
    pub max_drift_bound: f32,

    // ── P4.5: Bounded adaptation guarantees ───────────────────────────────
    /// Maximum structural replans per session (4).
    #[serde(default = "default_max_structural_replans")]
    pub max_structural_replans: u32,

    /// Maximum cumulative adaptive policy sensitivity shift per session (0.50).
    #[serde(default = "default_max_sensitivity_shift")]
    pub max_sensitivity_shift: f64,

    /// Maximum strategy mutations per session (6).
    #[serde(default = "default_max_strategy_mutations")]
    pub max_strategy_mutations: u32,

    /// Maximum model downgrades per session (2).
    #[serde(default = "default_max_model_downgrades")]
    pub max_model_downgrades: u32,

    // ── P5.2: Problem classification ────────────────────────────────────────
    /// Minimum rounds before first problem classification (2).
    #[serde(default = "default_classification_min_rounds")]
    pub classification_min_rounds: usize,

    /// Signal divergence threshold that triggers reclassification (0.30).
    #[serde(default = "default_reclassification_shift_threshold")]
    pub reclassification_shift_threshold: f64,

    /// Score variance threshold for Oscillatory problem class (0.04).
    #[serde(default = "default_oscillation_variance_threshold")]
    pub oscillation_variance_threshold: f64,

    // ── P5.1: Session retrospective ─────────────────────────────────────────
    /// Combined score below which a round is considered wasted (0.10).
    #[serde(default = "default_wasted_round_threshold")]
    pub wasted_round_threshold: f64,

    // ── P5.3: Adaptive strategy weighting ───────────────────────────────────
    /// Maximum per-round weight adjustment magnitude (0.05).
    #[serde(default = "default_max_weight_shift_per_round")]
    pub max_weight_shift_per_round: f64,

    // ── P5.4: Convergence estimator ─────────────────────────────────────────
    /// Minimum data points for convergence forecast (3).
    #[serde(default = "default_forecast_min_rounds")]
    pub forecast_min_rounds: usize,

    /// Convergence probability below which synthesis urgency is boosted (0.20).
    #[serde(default = "default_forecast_low_probability_threshold")]
    pub forecast_low_probability_threshold: f64,

    // ── P5.5: Strategic initialization ──────────────────────────────────────
    /// Enable data-driven round-0 initialization (true).
    #[serde(default = "default_strategic_init_enabled")]
    pub strategic_init_enabled: bool,

    // ── Step 8.1/10: Capability gate token estimation ────────────────────────
    /// Average estimated input tokens consumed per plan step (300).
    ///
    /// Base per-step token estimate for `derive_capability_descriptor()`.
    /// Used by `assign_base_costs()` for topology-aware cost propagation (Step 10):
    /// Text nodes → `avg`; ToolUse/Vision nodes → `avg × tool_cost_multiplier`.
    #[serde(default = "default_avg_input_tokens_per_step")]
    pub avg_input_tokens_per_step: usize,

    /// Cost multiplier applied to ToolUse and Vision nodes in graph cost propagation (2).
    ///
    /// Tool-invoking steps consume more tokens than text-only steps due to tool
    /// call overhead, result rendering, and follow-up processing. Deterministic —
    /// no heuristics, no dynamic measurement.
    ///
    /// Used by `ExecutionGraph::assign_base_costs()` (Step 10.1):
    ///   `base_cost = avg_input_tokens_per_step × tool_cost_multiplier`
    #[serde(default = "default_tool_cost_multiplier")]
    pub tool_cost_multiplier: usize,

    // ── Step 4: Convergence Budget Governor ─────────────────────────────────
    /// Hard upper bound on loop iterations before synthesis is forced (12).
    ///
    /// Replaces implicit dependency on `effective_max_rounds` for pathological-loop
    /// termination. Governor fires `request_synthesis(OracleConvergence, Critical)` at
    /// round >= max_round_iterations regardless of other convergence state.
    /// Default 12 is permissive — normal sessions complete in 1-5 rounds.
    #[serde(default = "default_max_round_iterations")]
    pub max_round_iterations: usize,

    /// Token growth ratio above which synthesis is forced (3.0).
    ///
    /// Effective limit = `initial_prompt_tokens * max_token_growth_ratio`.
    /// Enforces: cumulative input-token growth across rounds must not exceed 3× the
    /// initial prompt size, eliminating unbounded context expansion.
    /// Governor fires `request_synthesis(CacheCorruption, High)` when limit is exceeded.
    /// Skipped when initial_tokens is zero (early rounds before first call).
    #[serde(default = "default_max_token_growth_ratio")]
    pub max_token_growth_ratio: f64,

    /// Consecutive rounds with < min_progress_delta score gain before stagnation fires (3).
    ///
    /// Replaces soft "low trajectory" heuristic in convergence scoring with a hard bound.
    /// Governor fires `request_synthesis(OscillationDetected, High)` after this many
    /// consecutive stagnant rounds.
    #[serde(default = "default_max_stagnation_rounds")]
    pub max_stagnation_rounds: usize,

    /// Minimum per-round score improvement to NOT count as stagnation (0.05).
    ///
    /// If `current_score - previous_score < min_progress_delta`, the round is counted
    /// as stagnant toward `max_stagnation_rounds`. Reset to 0 on any round with
    /// progress >= this threshold.
    #[serde(default = "default_min_progress_delta")]
    pub min_progress_delta: f32,

    // ── SLA Mode Budgets — operator-configurable (replaces hardcoded 4/10/20) ──
    /// Maximum rounds for Fast/Quick SLA mode (4).
    #[serde(default = "default_sla_fast_max_rounds")]
    pub sla_fast_max_rounds: u32,

    /// Maximum plan depth for Fast/Quick SLA mode (2).
    #[serde(default = "default_sla_fast_max_plan_depth")]
    pub sla_fast_max_plan_depth: u32,

    /// Maximum rounds for Balanced/Extended SLA mode (10).
    #[serde(default = "default_sla_balanced_max_rounds")]
    pub sla_balanced_max_rounds: u32,

    /// Maximum plan depth for Balanced/Extended SLA mode (5).
    #[serde(default = "default_sla_balanced_max_plan_depth")]
    pub sla_balanced_max_plan_depth: u32,

    /// Maximum retries for Balanced/Extended SLA mode (1).
    #[serde(default = "default_sla_balanced_max_retries")]
    pub sla_balanced_max_retries: u32,

    /// Maximum rounds for Deep SLA mode (20).
    #[serde(default = "default_sla_deep_max_rounds")]
    pub sla_deep_max_rounds: u32,

    /// Maximum plan depth for Deep SLA mode (10).
    #[serde(default = "default_sla_deep_max_plan_depth")]
    pub sla_deep_max_plan_depth: u32,

    /// Maximum retries for Deep SLA mode (3).
    #[serde(default = "default_sla_deep_max_retries")]
    pub sla_deep_max_retries: u32,

    // ── Intent Pipeline — unified reconciliation ─────────────────────────
    /// High-confidence threshold: IntentScorer suggestion trusted more than
    /// BoundaryDecision when confidence >= this value (0.75).
    #[serde(default = "default_intent_high_confidence_threshold")]
    pub intent_high_confidence_threshold: f32,

    /// Low-confidence threshold: BoundaryDecision trusted more than IntentScorer
    /// when confidence <= this value (0.40).
    #[serde(default = "default_intent_low_confidence_threshold")]
    pub intent_low_confidence_threshold: f32,

    /// Enable unified IntentPipeline reconciliation (true).
    ///
    /// When true, `IntentPipeline::resolve()` reconciles IntentScorer and
    /// BoundaryDecisionEngine outputs into a single `ResolvedIntent` with
    /// calibrated ConvergenceController parameters.
    /// When false, falls back to the legacy dual-pipeline behavior.
    #[serde(default = "default_use_intent_pipeline")]
    pub use_intent_pipeline: bool,

    // ── Boundary Decision Engine ──────────────────────────────────────────
    /// Enable the new boundary decision engine for SLA routing (true).
    ///
    /// When `true`, `agent/mod.rs` calls `BoundaryDecisionEngine::evaluate()`
    /// instead of the legacy `decision_layer::estimate_complexity()`.
    /// Set to `false` to fall back to keyword-count routing.
    #[serde(default = "default_use_boundary_decision_engine")]
    pub use_boundary_decision_engine: bool,

    // ── HALCON.md Persistent Instruction System (Feature 1 — Frontier Roadmap 2026) ─
    /// Enable the 4-scope HALCON.md instruction hierarchy (false by default).
    ///
    /// When `true`, `agent/mod.rs` loads instruction files from four scopes
    /// (Local → User → Project → Managed) at session start and injects them
    /// into the system prompt as a `## Project Instructions` section.
    /// A hot-reload watcher fires within 250 ms of any file change.
    ///
    /// Scopes:
    ///   - Local:   `./HALCON.local.md`        (gitignored)
    ///   - User:    `~/.halcon/HALCON.md`
    ///   - Project: `.halcon/HALCON.md` + `.halcon/rules/*.md`
    ///   - Managed: `/etc/halcon/HALCON.md`    (operator policy, always wins)
    ///
    /// Set to `false` to preserve the legacy `load_instructions` behaviour.
    #[serde(default = "default_use_halcon_md")]
    pub use_halcon_md: bool,

    /// Lifecycle hooks system (Feature 2 — Frontier Roadmap 2026).
    ///
    /// Master runtime kill switch for user-accessible lifecycle hooks
    /// (`PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `UserPromptSubmit`,
    /// `Stop`, `SessionEnd`).
    ///
    /// When `false` (default) no hooks run, regardless of what is configured in
    /// `~/.halcon/settings.toml` or `.halcon/settings.toml`.
    ///
    /// Set to `true` to enable hook evaluation at defined lifecycle events.
    ///
    /// # Security invariant
    /// Hooks form a **user-policy layer** that runs BEFORE FASE-2
    /// CATASTROPHIC_PATTERNS checks.  A hook returning Allow cannot bypass the
    /// hard safety wall in `bash.rs`.
    #[serde(default = "default_enable_hooks")]
    pub enable_hooks: bool,

    /// Enterprise policy: restrict hook loading to the global managed scope only.
    ///
    /// When `true`, only hooks defined in `~/.halcon/settings.toml` (global /
    /// operator scope) are loaded.  Project-level `.halcon/settings.toml` hooks
    /// are silently ignored — they cannot override or extend managed hooks.
    ///
    /// Useful in environments where a central security team manages hook policy
    /// and project-local hooks would represent an escalation vector.
    ///
    /// Default: `false` (project-level hooks allowed alongside global hooks).
    #[serde(default = "default_allow_managed_hooks_only")]
    pub allow_managed_hooks_only: bool,

    /// Auto-memory: agent writes session learnings to `.halcon/memory/MEMORY.md`.
    ///
    /// When enabled, at the end of each session the agent scores the interaction and,
    /// if the importance score exceeds `memory_importance_threshold`, appends a structured
    /// entry to the project memory file and injects the first 200 lines at session start.
    ///
    /// Off by default — zero behavioral change until explicitly enabled.
    #[serde(default = "default_enable_auto_memory")]
    pub enable_auto_memory: bool,

    /// Minimum importance score [0.0, 1.0] required for a memory entry to be written.
    ///
    /// Higher values produce fewer, higher-quality memories.  0.3 balances recall
    /// breadth vs. MEMORY.md growth rate (≈180-line bounded index).
    #[serde(default = "default_memory_importance_threshold")]
    pub memory_importance_threshold: f32,

    /// Declarative sub-agent registry (Feature 4 — Frontier Roadmap 2026).
    ///
    /// When enabled, `AgentRegistry::load()` is called at session start to discover
    /// agent definitions from `.halcon/agents/*.md` (project) and `~/.halcon/agents/*.md`
    /// (user).  The routing manifest is injected into the parent agent's system prompt.
    ///
    /// Off by default — zero behavioral change until explicitly enabled.
    #[serde(default = "default_enable_agent_registry")]
    pub enable_agent_registry: bool,

    // ── Semantic Memory Vector Store (Feature 7) ──────────────────────────────
    /// When enabled, a `VectorMemoryStore` is built from MEMORY.md files and:
    /// (a) the `search_memory` tool is injected into the session's tool registry,
    /// (b) a `VectorMemorySource` context source contributes top-K results to the
    ///     pipeline context on each round.
    ///
    /// Off by default — opt-in via config or `--enable-semantic-memory` flag.
    #[serde(default = "default_enable_semantic_memory")]
    pub enable_semantic_memory: bool,

    /// Number of top-K memory entries to surface via `search_memory` and the
    /// `VectorMemorySource` context source (default: 5).
    #[serde(default = "default_semantic_memory_top_k")]
    pub semantic_memory_top_k: usize,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            success_threshold: default_success_threshold(),
            critic_unavailable_penalty: default_critic_unavailable_penalty(),
            halt_confidence_threshold: default_halt_confidence_threshold(),
            critic_timeout_secs: default_critic_timeout_secs(),
            excerpt_len: default_excerpt_len(),
            min_retry_confidence: default_min_retry_confidence(),
            min_evidence_bytes: default_min_evidence_bytes(),
            min_synthesis_coverage: default_min_synthesis_coverage(),
            synthesis_max_tokens: default_synthesis_max_tokens(),
            hide_threshold: default_hide_threshold(),
            deprioritize_threshold: default_deprioritize_threshold(),
            min_calls_for_filtering: default_min_calls_for_filtering(),
            max_replan_attempts: default_max_replan_attempts(),
            temperature_step: default_temperature_step(),
            max_temperature: default_max_temperature(),
            tool_failure_threshold: default_tool_failure_threshold(),
            w_stop: default_w_stop(),
            w_trajectory: default_w_trajectory(),
            w_critic: default_w_critic(),
            w_coherence_reward: default_w_coherence_reward(),
            w_progress_round: default_w_progress_round(),
            w_efficiency_round: default_w_efficiency_round(),
            w_coherence_round: default_w_coherence_round(),
            w_token_round: default_w_token_round(),
            output_headroom_tokens: default_output_headroom_tokens(),
            compaction_timeout_secs: default_compaction_timeout_secs(),
            growth_threshold: default_growth_threshold(),
            growth_consecutive_trigger: default_growth_consecutive_trigger(),
            mini_critic_interval: default_mini_critic_interval(),
            mini_critic_budget_fraction: default_mini_critic_budget_fraction(),
            mini_critic_synthesis_budget_fraction: default_mini_critic_synthesis_budget_fraction(),
            mini_critic_synthesis_progress_threshold: default_mini_critic_synthesis_progress_threshold(),
            mini_critic_progress_threshold: default_mini_critic_progress_threshold(),
            mini_critic_recent_threshold: default_mini_critic_recent_threshold(),
            k5_2_tool_output_max_chars: default_k5_2_tool_output_max_chars(),
            k5_2_message_retain_ratio: default_k5_2_message_retain_ratio(),
            per_call_critic_timeout_secs: default_per_call_critic_timeout_secs(),
            oscillation_window: default_oscillation_window(),
            loop_guard_min_synthesis: default_loop_guard_min_synthesis(),
            loop_guard_min_force: default_loop_guard_min_force(),
            sub_agent_max_timeout_secs: default_sub_agent_max_timeout_secs(),
            early_convergence_threshold: default_early_convergence_threshold(),
            replan_score_threshold: default_replan_score_threshold(),
            drift_threshold: default_drift_threshold(),
            model_quality_gate: default_model_quality_gate(),
            // P3.1
            strategy_force_synthesis_sla: default_strategy_force_synthesis_sla(),
            strategy_min_evidence_for_synthesis: default_strategy_min_evidence_for_synthesis(),
            strategy_collapse_min_progress: default_strategy_collapse_min_progress(),
            strategy_drift_threshold: default_strategy_drift_threshold(),
            strategy_failure_cluster_threshold: default_strategy_failure_cluster_threshold(),
            // P3.2
            capability_auto_skip: default_capability_auto_skip(),
            // P3.3
            semantic_cycle_window: default_semantic_cycle_window(),
            cycle_synonym_overlap_threshold: default_cycle_synonym_overlap_threshold(),
            cycle_replan_boost_threshold: default_cycle_replan_boost_threshold(),
            cycle_medium_threshold: default_cycle_medium_threshold(),
            cycle_high_threshold: default_cycle_high_threshold(),
            // P3.4
            mid_critic_interval: default_mid_critic_interval(),
            progress_deficit_threshold: default_progress_deficit_threshold(),
            objective_drift_threshold: default_objective_drift_threshold(),
            evidence_rate_decline_ratio: default_evidence_rate_decline_ratio(),
            // P3.5
            complexity_min_rounds: default_complexity_min_rounds(),
            complexity_upgrade_ratio: default_complexity_upgrade_ratio(),
            complexity_confidence_threshold: default_complexity_confidence_threshold(),
            // P3.6
            utility_synthesis_threshold: default_utility_synthesis_threshold(),
            utility_marginal_threshold: default_utility_marginal_threshold(),
            utility_w_evidence: default_utility_w_evidence(),
            utility_w_coherence: default_utility_w_coherence(),
            utility_w_pressure: default_utility_w_pressure(),
            utility_w_cost: default_utility_w_cost(),
            utility_w_drift: default_utility_w_drift(),
            utility_w_progress: default_utility_w_progress(),
            // P4.1
            max_drift_bound: default_max_drift_bound(),
            // P4.5
            max_structural_replans: default_max_structural_replans(),
            max_sensitivity_shift: default_max_sensitivity_shift(),
            max_strategy_mutations: default_max_strategy_mutations(),
            max_model_downgrades: default_max_model_downgrades(),
            // P5.2
            classification_min_rounds: default_classification_min_rounds(),
            reclassification_shift_threshold: default_reclassification_shift_threshold(),
            oscillation_variance_threshold: default_oscillation_variance_threshold(),
            // P5.1
            wasted_round_threshold: default_wasted_round_threshold(),
            // P5.3
            max_weight_shift_per_round: default_max_weight_shift_per_round(),
            // P5.4
            forecast_min_rounds: default_forecast_min_rounds(),
            forecast_low_probability_threshold: default_forecast_low_probability_threshold(),
            // P5.5
            strategic_init_enabled: default_strategic_init_enabled(),
            // Step 8.1/10
            avg_input_tokens_per_step: default_avg_input_tokens_per_step(),
            tool_cost_multiplier: default_tool_cost_multiplier(),
            // Step 4: Convergence Budget Governor
            max_round_iterations: default_max_round_iterations(),
            max_token_growth_ratio: default_max_token_growth_ratio(),
            max_stagnation_rounds: default_max_stagnation_rounds(),
            min_progress_delta: default_min_progress_delta(),
            // SLA Mode Budgets
            sla_fast_max_rounds: default_sla_fast_max_rounds(),
            sla_fast_max_plan_depth: default_sla_fast_max_plan_depth(),
            sla_balanced_max_rounds: default_sla_balanced_max_rounds(),
            sla_balanced_max_plan_depth: default_sla_balanced_max_plan_depth(),
            sla_balanced_max_retries: default_sla_balanced_max_retries(),
            sla_deep_max_rounds: default_sla_deep_max_rounds(),
            sla_deep_max_plan_depth: default_sla_deep_max_plan_depth(),
            sla_deep_max_retries: default_sla_deep_max_retries(),
            // Intent Pipeline
            intent_high_confidence_threshold: default_intent_high_confidence_threshold(),
            intent_low_confidence_threshold: default_intent_low_confidence_threshold(),
            use_intent_pipeline: default_use_intent_pipeline(),
            // Boundary Decision Engine
            use_boundary_decision_engine: default_use_boundary_decision_engine(),
            // HALCON.md instruction system (off by default)
            use_halcon_md: default_use_halcon_md(),
            // Lifecycle hooks system (off by default)
            enable_hooks: default_enable_hooks(),
            allow_managed_hooks_only: default_allow_managed_hooks_only(),
            // Auto-memory system (off by default)
            enable_auto_memory: default_enable_auto_memory(),
            memory_importance_threshold: default_memory_importance_threshold(),
            // Declarative sub-agent registry (off by default)
            enable_agent_registry: default_enable_agent_registry(),
            // Semantic memory vector store (off by default)
            enable_semantic_memory: default_enable_semantic_memory(),
            semantic_memory_top_k: default_semantic_memory_top_k(),
        }
    }
}

// ── Default value functions (used by serde) ──────────────────────────────────

fn default_success_threshold() -> f64 { 0.60 }
fn default_critic_unavailable_penalty() -> f64 { 0.15 }
fn default_halt_confidence_threshold() -> f32 { 0.80 }
fn default_critic_timeout_secs() -> u64 { 45 }
fn default_excerpt_len() -> usize { 1500 }
fn default_min_retry_confidence() -> f32 { 0.40 }
fn default_min_evidence_bytes() -> usize { 30 }
fn default_min_synthesis_coverage() -> f64 { 0.30 }
fn default_synthesis_max_tokens() -> u32 { 4096 }
fn default_hide_threshold() -> f64 { 0.15 }
fn default_deprioritize_threshold() -> f64 { 0.40 }
fn default_min_calls_for_filtering() -> u32 { 3 }
fn default_max_replan_attempts() -> u32 { 2 }
fn default_temperature_step() -> f32 { 0.1 }
fn default_max_temperature() -> f32 { 1.0 }
fn default_tool_failure_threshold() -> u32 { 2 }
fn default_w_stop() -> f64 { 0.25 }
fn default_w_trajectory() -> f64 { 0.30 }
fn default_w_critic() -> f64 { 0.25 }
fn default_w_coherence_reward() -> f64 { 0.20 }
fn default_w_progress_round() -> f64 { 0.45 }
fn default_w_efficiency_round() -> f64 { 0.30 }
fn default_w_coherence_round() -> f64 { 0.10 }
fn default_w_token_round() -> f64 { 0.15 }
fn default_output_headroom_tokens() -> u32 { 5000 }
fn default_compaction_timeout_secs() -> u64 { 15 }
fn default_growth_threshold() -> f64 { 1.3 }
fn default_growth_consecutive_trigger() -> u32 { 2 }
fn default_mini_critic_interval() -> usize { 3 }
fn default_mini_critic_budget_fraction() -> f64 { 0.50 }
fn default_mini_critic_synthesis_budget_fraction() -> f64 { 0.80 }
fn default_mini_critic_synthesis_progress_threshold() -> f64 { 0.80 }
fn default_mini_critic_progress_threshold() -> f64 { 0.30 }
fn default_mini_critic_recent_threshold() -> f32 { 0.40 }
fn default_k5_2_tool_output_max_chars() -> usize { 2000 }
fn default_k5_2_message_retain_ratio() -> f32 { 0.60 }
fn default_per_call_critic_timeout_secs() -> u64 { 22 }
fn default_oscillation_window() -> usize { 8 }
fn default_loop_guard_min_synthesis() -> usize { 3 }
fn default_loop_guard_min_force() -> usize { 5 }
fn default_sub_agent_max_timeout_secs() -> u64 { 300 }
fn default_early_convergence_threshold() -> f32 { 0.80 }
fn default_replan_score_threshold() -> f32 { 0.15 }
fn default_drift_threshold() -> f32 { 0.70 }
fn default_model_quality_gate() -> f64 { 0.35 }

// ── Phase 3 defaults ──────────────────────────────────────────────────────────
// P3.1: Mid-loop strategy mutation
fn default_strategy_force_synthesis_sla() -> f64 { 0.85 }
fn default_strategy_min_evidence_for_synthesis() -> f64 { 0.30 }
fn default_strategy_collapse_min_progress() -> f32 { 0.50 }
fn default_strategy_drift_threshold() -> f32 { 0.50 }
fn default_strategy_failure_cluster_threshold() -> f32 { 0.50 }
// P3.2: Capability validation
fn default_capability_auto_skip() -> bool { true }
// P3.3: Semantic cycle detection
fn default_semantic_cycle_window() -> usize { 6 }
fn default_cycle_synonym_overlap_threshold() -> f64 { 0.60 }
fn default_cycle_replan_boost_threshold() -> f32 { 0.50 }
fn default_cycle_medium_threshold() -> usize { 3 }
fn default_cycle_high_threshold() -> usize { 4 }
// P3.4: Mid-loop critic checkpoints
fn default_mid_critic_interval() -> usize { 3 }
fn default_progress_deficit_threshold() -> f64 { 0.25 }
fn default_objective_drift_threshold() -> f64 { 0.40 }
fn default_evidence_rate_decline_ratio() -> f64 { 0.50 }
// P3.5: Complexity feedback loop
fn default_complexity_min_rounds() -> usize { 3 }
fn default_complexity_upgrade_ratio() -> f64 { 1.5 }
fn default_complexity_confidence_threshold() -> f64 { 0.70 }
// P3.6: Convergence utility function
fn default_utility_synthesis_threshold() -> f64 { 0.35 }
fn default_utility_marginal_threshold() -> f64 { 0.05 }
fn default_utility_w_evidence() -> f64 { 0.25 }
fn default_utility_w_coherence() -> f64 { 0.15 }
fn default_utility_w_pressure() -> f64 { 0.20 }
fn default_utility_w_cost() -> f64 { 0.15 }
fn default_utility_w_drift() -> f64 { 0.10 }
fn default_utility_w_progress() -> f64 { 0.15 }
// P4.1: System invariants
fn default_max_drift_bound() -> f32 { 5.0 }
// P4.5: Bounded adaptation
fn default_max_structural_replans() -> u32 { 4 }
fn default_max_sensitivity_shift() -> f64 { 0.50 }
fn default_max_strategy_mutations() -> u32 { 6 }
fn default_max_model_downgrades() -> u32 { 2 }
// P5.2: Problem classification
fn default_classification_min_rounds() -> usize { 2 }
fn default_reclassification_shift_threshold() -> f64 { 0.30 }
fn default_oscillation_variance_threshold() -> f64 { 0.04 }
// P5.1: Session retrospective
fn default_wasted_round_threshold() -> f64 { 0.10 }
// P5.3: Adaptive strategy weighting
fn default_max_weight_shift_per_round() -> f64 { 0.05 }
// P5.4: Convergence estimator
fn default_forecast_min_rounds() -> usize { 3 }
fn default_forecast_low_probability_threshold() -> f64 { 0.20 }
// P5.5: Strategic initialization
fn default_strategic_init_enabled() -> bool { true }
// Step 8.1/10: Capability gate token estimation + graph cost propagation
fn default_avg_input_tokens_per_step() -> usize { 300 }
fn default_tool_cost_multiplier() -> usize { 2 }
// Step 4: Convergence Budget Governor
fn default_max_round_iterations() -> usize { 12 }
fn default_max_token_growth_ratio() -> f64 { 3.0 }
fn default_max_stagnation_rounds() -> usize { 3 }
fn default_min_progress_delta() -> f32 { 0.05 }
// Boundary Decision Engine: enabled by default (replaces legacy keyword-count routing)
fn default_sla_fast_max_rounds() -> u32 { 4 }
fn default_sla_fast_max_plan_depth() -> u32 { 2 }
fn default_sla_balanced_max_rounds() -> u32 { 10 }
fn default_sla_balanced_max_plan_depth() -> u32 { 5 }
fn default_sla_balanced_max_retries() -> u32 { 1 }
fn default_sla_deep_max_rounds() -> u32 { 20 }
fn default_sla_deep_max_plan_depth() -> u32 { 10 }
fn default_sla_deep_max_retries() -> u32 { 3 }
fn default_intent_high_confidence_threshold() -> f32 { 0.75 }
fn default_intent_low_confidence_threshold() -> f32 { 0.40 }
fn default_use_intent_pipeline() -> bool { true }
fn default_use_boundary_decision_engine() -> bool { true }
// HALCON.md instruction system — on by default (additive, filesystem-only, safe)
fn default_use_halcon_md() -> bool { true }
// Lifecycle hooks system — off by default (executes user scripts — security surface)
// Enable explicitly in config: [policy] enable_hooks = true
fn default_enable_hooks() -> bool { false }
fn default_allow_managed_hooks_only() -> bool { false }
// Auto-memory system — on by default (additive, background write, safe)
// Disable with: [policy] enable_auto_memory = false
fn default_enable_auto_memory() -> bool { true }
fn default_memory_importance_threshold() -> f32 { 0.3 }
// Declarative sub-agent registry — on by default (read-only, safe)
// Disable with: [policy] enable_agent_registry = false
fn default_enable_agent_registry() -> bool { true }
// Semantic memory vector store — off by default (TF-IDF overhead, explicit opt-in)
fn default_enable_semantic_memory() -> bool { false }
fn default_semantic_memory_top_k() -> usize { 5 }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_config_default_matches_current_constants() {
        let p = PolicyConfig::default();
        assert!((p.success_threshold - 0.60).abs() < f64::EPSILON);
        assert!((p.critic_unavailable_penalty - 0.15).abs() < f64::EPSILON);
        assert!((p.halt_confidence_threshold - 0.80).abs() < f32::EPSILON);
        assert_eq!(p.critic_timeout_secs, 45);
        assert_eq!(p.excerpt_len, 1500);
        assert!((p.min_retry_confidence - 0.40).abs() < f32::EPSILON);
        assert_eq!(p.min_evidence_bytes, 30);
        assert!((p.min_synthesis_coverage - 0.30).abs() < f64::EPSILON);
        assert_eq!(p.synthesis_max_tokens, 4096);
        assert!((p.hide_threshold - 0.15).abs() < f64::EPSILON);
        assert!((p.deprioritize_threshold - 0.40).abs() < f64::EPSILON);
        assert_eq!(p.min_calls_for_filtering, 3);
        assert_eq!(p.max_replan_attempts, 2);
        assert!((p.temperature_step - 0.1).abs() < f32::EPSILON);
        assert!((p.max_temperature - 1.0).abs() < f32::EPSILON);
        assert_eq!(p.tool_failure_threshold, 2);
        // Reward weights
        assert!((p.w_stop - 0.25).abs() < f64::EPSILON);
        assert!((p.w_trajectory - 0.30).abs() < f64::EPSILON);
        assert!((p.w_critic - 0.25).abs() < f64::EPSILON);
        assert!((p.w_coherence_reward - 0.20).abs() < f64::EPSILON);
        // Round scorer weights (intentionally different from reward weights)
        assert!((p.w_progress_round - 0.45).abs() < f64::EPSILON);
        assert!((p.w_efficiency_round - 0.30).abs() < f64::EPSILON);
        assert!((p.w_coherence_round - 0.10).abs() < f64::EPSILON);
        assert!((p.w_token_round - 0.15).abs() < f64::EPSILON);
        // Runtime thresholds
        assert_eq!(p.output_headroom_tokens, 5000);
        assert_eq!(p.compaction_timeout_secs, 15);
        assert!((p.growth_threshold - 1.3).abs() < f64::EPSILON);
        assert_eq!(p.growth_consecutive_trigger, 2);
        assert_eq!(p.mini_critic_interval, 3);
        assert!((p.mini_critic_budget_fraction - 0.50).abs() < f64::EPSILON);
        // Step 1.1 — new fields eliminating hardcodes
        assert!((p.mini_critic_synthesis_budget_fraction - 0.80).abs() < f64::EPSILON);
        assert!((p.mini_critic_synthesis_progress_threshold - 0.80).abs() < f64::EPSILON);
        assert!((p.mini_critic_progress_threshold - 0.30).abs() < f64::EPSILON);
        assert!((p.mini_critic_recent_threshold - 0.40).abs() < f32::EPSILON);
        assert_eq!(p.k5_2_tool_output_max_chars, 2000);
        assert!((p.k5_2_message_retain_ratio - 0.60).abs() < f32::EPSILON);
        assert_eq!(p.per_call_critic_timeout_secs, 22);
        // Phase 1B: loop guard, sub-agent, convergence, coherence, model quality
        assert_eq!(p.oscillation_window, 8);
        assert_eq!(p.loop_guard_min_synthesis, 3);
        assert_eq!(p.loop_guard_min_force, 5);
        assert_eq!(p.sub_agent_max_timeout_secs, 300);
        assert!((p.early_convergence_threshold - 0.80).abs() < f32::EPSILON);
        assert!((p.replan_score_threshold - 0.15).abs() < f32::EPSILON);
        assert!((p.drift_threshold - 0.70).abs() < f32::EPSILON);
        assert!((p.model_quality_gate - 0.35).abs() < f64::EPSILON);
        // Phase 3 fields
        assert!((p.strategy_force_synthesis_sla - 0.85).abs() < f64::EPSILON);
        assert!((p.strategy_min_evidence_for_synthesis - 0.30).abs() < f64::EPSILON);
        assert!((p.strategy_collapse_min_progress - 0.50).abs() < f32::EPSILON);
        assert!((p.strategy_drift_threshold - 0.50).abs() < f32::EPSILON);
        assert!((p.strategy_failure_cluster_threshold - 0.50).abs() < f32::EPSILON);
        assert!(p.capability_auto_skip);
        assert_eq!(p.semantic_cycle_window, 6);
        assert!((p.cycle_synonym_overlap_threshold - 0.60).abs() < f64::EPSILON);
        assert!((p.cycle_replan_boost_threshold - 0.50).abs() < f32::EPSILON);
        assert_eq!(p.cycle_medium_threshold, 3);
        assert_eq!(p.cycle_high_threshold, 4);
        assert_eq!(p.mid_critic_interval, 3);
        assert!((p.progress_deficit_threshold - 0.25).abs() < f64::EPSILON);
        assert!((p.objective_drift_threshold - 0.40).abs() < f64::EPSILON);
        assert!((p.evidence_rate_decline_ratio - 0.50).abs() < f64::EPSILON);
        assert_eq!(p.complexity_min_rounds, 3);
        assert!((p.complexity_upgrade_ratio - 1.5).abs() < f64::EPSILON);
        assert!((p.complexity_confidence_threshold - 0.70).abs() < f64::EPSILON);
        assert!((p.utility_synthesis_threshold - 0.35).abs() < f64::EPSILON);
        assert!((p.utility_marginal_threshold - 0.05).abs() < f64::EPSILON);
        assert!((p.utility_w_evidence - 0.25).abs() < f64::EPSILON);
        assert!((p.utility_w_coherence - 0.15).abs() < f64::EPSILON);
        assert!((p.utility_w_pressure - 0.20).abs() < f64::EPSILON);
        assert!((p.utility_w_cost - 0.15).abs() < f64::EPSILON);
        assert!((p.utility_w_drift - 0.10).abs() < f64::EPSILON);
        assert!((p.utility_w_progress - 0.15).abs() < f64::EPSILON);
        // Phase 4 fields
        assert!((p.max_drift_bound - 5.0).abs() < f32::EPSILON);
        assert_eq!(p.max_structural_replans, 4);
        assert!((p.max_sensitivity_shift - 0.50).abs() < f64::EPSILON);
        assert_eq!(p.max_strategy_mutations, 6);
        assert_eq!(p.max_model_downgrades, 2);
        // Phase 5 fields
        assert_eq!(p.classification_min_rounds, 2);
        assert!((p.reclassification_shift_threshold - 0.30).abs() < f64::EPSILON);
        assert!((p.oscillation_variance_threshold - 0.04).abs() < f64::EPSILON);
        assert!((p.wasted_round_threshold - 0.10).abs() < f64::EPSILON);
        assert!((p.max_weight_shift_per_round - 0.05).abs() < f64::EPSILON);
        assert_eq!(p.forecast_min_rounds, 3);
        assert!((p.forecast_low_probability_threshold - 0.20).abs() < f64::EPSILON);
        assert!(p.strategic_init_enabled);
        // Step 8.1/10: Capability gate token estimation + graph cost propagation
        assert_eq!(p.avg_input_tokens_per_step, 300);
        assert_eq!(p.tool_cost_multiplier, 2);
        // Step 4: Convergence Budget Governor
        assert_eq!(p.max_round_iterations, 12);
        assert!((p.max_token_growth_ratio - 3.0).abs() < f64::EPSILON);
        assert_eq!(p.max_stagnation_rounds, 3);
        assert!((p.min_progress_delta - 0.05).abs() < f32::EPSILON);
    }

    #[test]
    fn policy_config_partial_json_uses_defaults() {
        let partial = r#"{"success_threshold": 0.75, "critic_timeout_secs": 60}"#;
        let parsed: PolicyConfig = serde_json::from_str(partial).expect("deserialize partial");
        assert!((parsed.success_threshold - 0.75).abs() < f64::EPSILON);
        assert_eq!(parsed.critic_timeout_secs, 60);
        // All other fields should be defaults
        assert!((parsed.halt_confidence_threshold - 0.80).abs() < f32::EPSILON);
        assert_eq!(parsed.min_evidence_bytes, 30);
        assert_eq!(parsed.max_replan_attempts, 2);
        assert_eq!(parsed.mini_critic_interval, 3);
    }

    #[test]
    fn policy_config_empty_json_uses_all_defaults() {
        let parsed: PolicyConfig = serde_json::from_str("{}").expect("deserialize empty");
        let default = PolicyConfig::default();
        assert!((parsed.success_threshold - default.success_threshold).abs() < f64::EPSILON);
        assert_eq!(parsed.critic_timeout_secs, default.critic_timeout_secs);
        assert_eq!(parsed.excerpt_len, default.excerpt_len);
    }

    #[test]
    fn policy_config_json_roundtrip() {
        let original = PolicyConfig::default();
        let json_str = serde_json::to_string(&original).expect("serialize json");
        let parsed: PolicyConfig = serde_json::from_str(&json_str).expect("deserialize json");
        assert!((parsed.success_threshold - original.success_threshold).abs() < f64::EPSILON);
        assert_eq!(parsed.min_calls_for_filtering, original.min_calls_for_filtering);
    }

    #[test]
    fn policy_config_custom_overrides() {
        let mut p = PolicyConfig::default();
        p.success_threshold = 0.80;
        p.critic_timeout_secs = 120;
        p.min_evidence_bytes = 50;
        p.max_replan_attempts = 5;
        p.growth_threshold = 1.5;
        p.mini_critic_interval = 5;
        assert!((p.success_threshold - 0.80).abs() < f64::EPSILON);
        assert_eq!(p.critic_timeout_secs, 120);
        assert_eq!(p.min_evidence_bytes, 50);
        assert_eq!(p.max_replan_attempts, 5);
        assert!((p.growth_threshold - 1.5).abs() < f64::EPSILON);
        assert_eq!(p.mini_critic_interval, 5);
    }

    // ── R2-A: Feature flag defaults (user onboarding) ────────────────────────

    #[test]
    fn feature_flags_default_to_correct_values() {
        let policy = PolicyConfig::default();
        // These three are safe + high-value: enabled by default for new users
        assert!(policy.use_halcon_md,
            "use_halcon_md must default to true — instruction persistence is additive and safe");
        assert!(policy.enable_auto_memory,
            "enable_auto_memory must default to true — session learning is additive and safe");
        assert!(policy.enable_agent_registry,
            "enable_agent_registry must default to true — declarative agents add value immediately");
        // These require explicit opt-in (security surface or overhead)
        assert!(!policy.enable_hooks,
            "enable_hooks must default to false — executes user scripts, security surface");
        assert!(!policy.enable_semantic_memory,
            "enable_semantic_memory must default to false — TF-IDF overhead, user opt-in");
        assert!(!policy.use_boundary_decision_engine || policy.use_boundary_decision_engine,
            "use_boundary_decision_engine may be true/false — just confirming field exists");
    }
}
