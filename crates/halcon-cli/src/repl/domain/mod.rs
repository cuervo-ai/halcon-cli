//! Domain layer — pure business logic with zero infrastructure dependencies.
//!
//! These modules contain only domain types, algorithms, and decision logic.
//! They do not depend on I/O, storage, HTTP, or any external services.
//! They can be safely extracted into a separate crate in the future.

/// Multi-signal intent profiling — SOTA 2026 replacement for keyword-based task analysis.
pub mod intent_scorer;

/// Adaptive loop termination with semantic progress tracking.
pub mod convergence_controller;

/// Dynamic model routing based on IntentProfile.
pub mod model_router;

/// UCB1 multi-armed bandit strategy selection.
pub mod strategy_selector;

/// Task complexity and type classification.
pub mod task_analyzer;

/// Shared text analysis utilities (keyword extraction, stopwords).
pub(crate) mod text_utils;

/// Per-round intelligence aggregate — bridges scoring signals to termination/policy decisions.
pub mod round_feedback;

/// Unified loop termination authority — explicit precedence over 4 independent control systems.
pub mod termination_oracle;

/// Within-session adaptive policy — the L6 enabler: real-time parameter self-adjustment.
pub mod adaptive_policy;

/// Intent-to-tool graph for declarative tool selection (Phase 2, feature = "intent-graph").
///
/// Covers 25/61 tools in Phase 2. Phase 4 expands to all 61.
/// ToolSelector consults this graph first, falls back to keyword logic when no match.
pub mod intent_graph;

// ── Phase 3: Strategy Evolution & Capability Awareness ──────────────────────

/// Mid-loop structural strategy mutation — decides HOW to replan (P3.1).
pub mod mid_loop_strategy;

/// Pre-execution capability validation — validates plan feasibility (P3.2).
pub mod capability_validator;

/// Semantic cycle detection — normalized tool-op deduplication (P3.3).
pub mod semantic_cycle;

/// Mid-loop critic checkpoints — progress-aware evaluation (P3.4).
pub mod mid_loop_critic;

/// Complexity feedback loop — runtime complexity upgrades (P3.5).
pub mod complexity_feedback;

/// Convergence utility function — optimal synthesis timing (P3.6).
pub mod convergence_utility;

// ── Phase 4: System Integrity & Observability ────────────────────────────────

/// System invariants — formalized global correctness properties (P4.1).
pub mod system_invariants;

/// Decision traceability — per-round decision audit trail (P4.2).
pub mod decision_trace;

/// Structured observability — per-round and session metrics (P4.3).
pub mod system_metrics;

/// Signal conflict resolution — deterministic arbitration of contradictory signals (P4.4).
pub mod signal_arbitrator;

/// Bounded adaptation guarantees — formal limits on runtime self-modification (P4.5).
pub mod adaptation_bounds;

// ── Phase 5: Meta-Cognitive Intelligence & Strategic Self-Evolution ──────────

/// Problem classification layer — runtime task classification for adaptive strategy (P5.2).
pub mod problem_classifier;

/// Session retrospective analyzer — post-session diagnostic analysis (P5.1).
pub mod session_retrospective;

/// Adaptive strategy weighting — intra-session utility weight adjustment (P5.3).
pub mod strategy_weights;

/// Predictive convergence estimator — forecasts convergence probability (P5.4).
pub mod convergence_estimator;

/// Strategic initialization engine — data-driven round-0 configuration (P5.5).
pub mod strategic_init;
