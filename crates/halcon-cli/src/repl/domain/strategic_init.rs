//! Strategic Initialization Engine — Data-driven round-0 configuration (P5.5).
//!
//! Replaces hardcoded defaults with problem-aware initial configuration based on
//! task complexity, user message heuristics, and available tools. Runs once at
//! session start before the first round.
//!
//! Bilingual EN/ES heuristics consistent with `decision_layer.rs`.
//!
//! Pure business logic — no I/O.

use super::problem_classifier::ProblemClass;
use super::strategy_weights::StrategyWeights;

// ── PlanGranularity ────────────────────────────────────────────────────────

/// Plan depth hint for the planner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanGranularity {
    /// 2-3 steps (simple tasks).
    Coarse,
    /// 4-6 steps (most tasks).
    Standard,
    /// 7+ steps (complex multi-domain).
    Fine,
}

impl PlanGranularity {
    /// Short label for logging.
    pub fn label(self) -> &'static str {
        match self {
            Self::Coarse => "coarse",
            Self::Standard => "standard",
            Self::Fine => "fine",
        }
    }
}

// ── InitializationProfile ──────────────────────────────────────────────────

/// Data-driven initialization profile for the agent session.
#[derive(Debug, Clone)]
pub struct InitializationProfile {
    /// Initial problem class assumption.
    pub problem_class: ProblemClass,
    /// Baseline strategy weights.
    pub weights: StrategyWeights,
    /// Plan depth hint.
    pub granularity: PlanGranularity,
    /// Fraction of SLA budget reserved for exploration [0.0, 1.0].
    pub exploration_budget: f64,
    /// Initial sensitivity for AdaptivePolicy.
    pub initial_sensitivity: f32,
    /// MidLoopCritic checkpoint interval override.
    pub critic_interval: usize,
    /// Human-readable rationale for the profile.
    pub rationale: &'static str,
}

// ── TaskComplexity (re-export friendly) ────────────────────────────────────

/// Complexity tiers from decision_layer — re-enumerated here for decoupling.
/// In practice, `initialize()` receives the concrete variant from
/// `decision_layer::TaskComplexity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Complexity {
    Simple,
    Structured,
    MultiDomain,
    LongHorizon,
}

// ── Initialization Logic ───────────────────────────────────────────────────

/// Compute an initialization profile from task complexity and user message.
///
/// # Parameters
/// - `complexity`: Task complexity from DecisionLayer.
/// - `user_message`: Raw user message (for heuristic keyword overrides).
/// - `available_tools`: Names of tools available in the current session.
pub fn initialize(
    complexity: Complexity,
    user_message: &str,
    _available_tools: &[String],
) -> InitializationProfile {
    // Base profile from complexity tier
    let mut profile = base_profile(complexity);

    // Heuristic overrides from user message keywords
    let lower = user_message.to_lowercase();
    apply_keyword_overrides(&lower, &mut profile);

    profile
}

fn base_profile(complexity: Complexity) -> InitializationProfile {
    match complexity {
        Complexity::Simple => InitializationProfile {
            problem_class: ProblemClass::DeterministicLinear,
            weights: StrategyWeights::for_class(ProblemClass::DeterministicLinear),
            granularity: PlanGranularity::Coarse,
            exploration_budget: 0.10,
            initial_sensitivity: 0.20,
            critic_interval: 4,
            rationale: "simple task — minimal exploration, coarse plan",
        },
        Complexity::Structured => InitializationProfile {
            problem_class: ProblemClass::DeterministicLinear,
            weights: StrategyWeights::for_class(ProblemClass::DeterministicLinear),
            granularity: PlanGranularity::Standard,
            exploration_budget: 0.25,
            initial_sensitivity: 0.30,
            critic_interval: 3,
            rationale: "structured task — standard exploration, moderate plan",
        },
        Complexity::MultiDomain => InitializationProfile {
            problem_class: ProblemClass::HighExploration,
            weights: StrategyWeights::for_class(ProblemClass::HighExploration),
            granularity: PlanGranularity::Standard,
            exploration_budget: 0.40,
            initial_sensitivity: 0.35,
            critic_interval: 3,
            rationale: "multi-domain — high exploration, standard granularity",
        },
        Complexity::LongHorizon => InitializationProfile {
            problem_class: ProblemClass::HighExploration,
            weights: StrategyWeights::for_class(ProblemClass::HighExploration),
            granularity: PlanGranularity::Fine,
            exploration_budget: 0.50,
            initial_sensitivity: 0.40,
            critic_interval: 2,
            rationale: "long horizon — deep exploration, fine-grained plan",
        },
    }
}

/// Debug/fix/error/bug → EvidenceSparse preset
/// Refactor/migrate/upgrade → ToolConstrained preset
/// Investigate/research/analyze → HighExploration preset
/// Quick/fast/rápido → SLAConstrained preset
fn apply_keyword_overrides(lower: &str, profile: &mut InitializationProfile) {
    // Debug/fix patterns (EN/ES)
    if contains_any(lower, &["debug", "fix", "error", "bug", "depurar", "arreglar", "corregir"]) {
        profile.problem_class = ProblemClass::EvidenceSparse;
        profile.weights = StrategyWeights::for_class(ProblemClass::EvidenceSparse);
        profile.rationale = "debug/fix heuristic — evidence-sparse preset";
        return;
    }

    // Refactor/migrate patterns (EN/ES)
    if contains_any(lower, &["refactor", "migrate", "upgrade", "refactorizar", "migrar"]) {
        profile.problem_class = ProblemClass::ToolConstrained;
        profile.weights = StrategyWeights::for_class(ProblemClass::ToolConstrained);
        profile.rationale = "refactor/migrate heuristic — tool-constrained preset";
        return;
    }

    // Investigation patterns (EN/ES)
    if contains_any(lower, &["investigate", "research", "analyze", "investigar", "analizar", "explorar"]) {
        profile.problem_class = ProblemClass::HighExploration;
        profile.weights = StrategyWeights::for_class(ProblemClass::HighExploration);
        profile.exploration_budget = 0.50;
        profile.rationale = "investigation heuristic — high exploration preset";
        return;
    }

    // Speed patterns (EN/ES)
    if contains_any(lower, &["quick", "fast", "rápido", "rapido", "urgente"]) {
        profile.problem_class = ProblemClass::SLAConstrained;
        profile.weights = StrategyWeights::for_class(ProblemClass::SLAConstrained);
        profile.exploration_budget = 0.10;
        profile.rationale = "speed heuristic — SLA-constrained preset";
    }
}

fn contains_any(text: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|p| text.contains(p))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase5_init_simple_complexity() {
        let profile = initialize(Complexity::Simple, "read this file", &[]);
        assert_eq!(profile.problem_class, ProblemClass::DeterministicLinear);
        assert_eq!(profile.granularity, PlanGranularity::Coarse);
        assert!((profile.exploration_budget - 0.10).abs() < 1e-4);
        assert_eq!(profile.critic_interval, 4);
    }

    #[test]
    fn phase5_init_structured_complexity() {
        let profile = initialize(Complexity::Structured, "find and update the config", &[]);
        assert_eq!(profile.problem_class, ProblemClass::DeterministicLinear);
        assert_eq!(profile.granularity, PlanGranularity::Standard);
        assert!((profile.exploration_budget - 0.25).abs() < 1e-4);
    }

    #[test]
    fn phase5_init_multi_domain_complexity() {
        let profile = initialize(Complexity::MultiDomain, "refactor auth and add tests", &[]);
        // Keyword "refactor" should override to ToolConstrained
        assert_eq!(profile.problem_class, ProblemClass::ToolConstrained);
    }

    #[test]
    fn phase5_init_long_horizon_complexity() {
        let profile = initialize(Complexity::LongHorizon, "full project audit", &[]);
        assert_eq!(profile.problem_class, ProblemClass::HighExploration);
        assert_eq!(profile.granularity, PlanGranularity::Fine);
        assert!((profile.exploration_budget - 0.50).abs() < 1e-4);
        assert_eq!(profile.critic_interval, 2);
    }

    #[test]
    fn phase5_init_debug_keyword_override() {
        let profile = initialize(Complexity::Structured, "debug the authentication bug", &[]);
        assert_eq!(profile.problem_class, ProblemClass::EvidenceSparse);
        assert!(profile.rationale.contains("debug"));
    }

    #[test]
    fn phase5_init_fix_keyword_override() {
        let profile = initialize(Complexity::Simple, "fix this error in the parser", &[]);
        assert_eq!(profile.problem_class, ProblemClass::EvidenceSparse);
    }

    #[test]
    fn phase5_init_refactor_keyword_override() {
        let profile = initialize(Complexity::Structured, "refactor the database layer", &[]);
        assert_eq!(profile.problem_class, ProblemClass::ToolConstrained);
    }

    #[test]
    fn phase5_init_investigate_keyword_override() {
        let profile = initialize(Complexity::Structured, "investigate why tests fail", &[]);
        assert_eq!(profile.problem_class, ProblemClass::HighExploration);
        assert!((profile.exploration_budget - 0.50).abs() < 1e-4);
    }

    #[test]
    fn phase5_init_quick_keyword_override() {
        let profile = initialize(Complexity::MultiDomain, "quick fix for the CI", &[]);
        // "quick" + "fix" → debug/fix wins (first match)
        assert_eq!(profile.problem_class, ProblemClass::EvidenceSparse);
    }

    #[test]
    fn phase5_init_spanish_keywords() {
        let profile = initialize(Complexity::Structured, "arreglar el error de autenticación", &[]);
        assert_eq!(profile.problem_class, ProblemClass::EvidenceSparse);

        let profile2 = initialize(Complexity::Structured, "investigar por qué falla", &[]);
        assert_eq!(profile2.problem_class, ProblemClass::HighExploration);

        let profile3 = initialize(Complexity::Structured, "hazlo rápido", &[]);
        assert_eq!(profile3.problem_class, ProblemClass::SLAConstrained);
    }

    #[test]
    fn phase5_init_granularity_labels_unique() {
        let granularities = [PlanGranularity::Coarse, PlanGranularity::Standard, PlanGranularity::Fine];
        let labels: Vec<&str> = granularities.iter().map(|g| g.label()).collect();
        let unique: std::collections::HashSet<&str> = labels.iter().copied().collect();
        assert_eq!(labels.len(), unique.len());
    }
}
