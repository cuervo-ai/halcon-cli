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
// decision_layer: pub(crate) types — access via module path
pub use input_boundary::{BoundaryInput, InputContext, InputNormalizer};
pub use coherence::PlanCoherenceChecker;
pub use llm_planner::LlmPlanner;
pub use playbook::PlaybookPlanner;
pub(crate) use sla::{SlaBudget, SlaMode};
pub use source::PlanningSource;
pub use metrics::PlanningMetrics;
pub use compressor::{compress as compress_plan, CompressionStats};
