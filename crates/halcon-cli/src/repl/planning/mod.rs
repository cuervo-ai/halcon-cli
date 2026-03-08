// planning/ — planificación, routing, normalización de input
// MIGRATION-2026: archivos movidos desde repl/ raíz

pub mod decision_layer;
pub mod input_boundary;
pub mod normalizer;
pub mod coherence;
pub mod compressor;
pub(crate) mod diagnostics;
pub mod metrics;
pub mod source;
pub mod playbook;
pub mod llm_planner;
pub mod router;
pub(crate) mod sla;

// Re-exports — preserve API surface for callers in repl/
pub use decision_layer::{BoundaryDecisionEngine, BoundaryDecisionResult, DecisionLayer};
pub use input_boundary::{InputBoundary, InputContext, InputNormalizer};
pub use normalizer::InputNormalizer as Normalizer;
pub use coherence::PlanCoherenceChecker;
pub use llm_planner::LlmPlanner;
pub use playbook::PlaybookPlanner;
pub use router::ModelRouter;
pub(crate) use sla::{SlaBudget, SlaMode};
pub use source::PlanningSource;
pub use metrics::PlanningMetrics;
pub use compressor::PlanCompressor;
pub(crate) use diagnostics::PlanStateDiagnostics;
