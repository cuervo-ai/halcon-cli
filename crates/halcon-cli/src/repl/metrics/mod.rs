// metrics/ — métricas, reward, observabilidad
// MIGRATION-2026: archivos movidos desde repl/ raíz

pub(crate) mod anomaly;
pub(crate) mod arima;
pub mod evaluator;
pub mod health;
pub mod macro_feedback;
pub mod store;
pub mod orchestrator;
pub mod reward;
pub mod scorer;
pub mod signal_ingestor;
pub mod strategy;

// Re-exports
pub use reward::{RewardBreakdown, RewardComputation, RawRewardSignals};
// NOTE: RewardPipeline was removed from reward.rs — stale re-export deleted (BUG-mailbox-pre-existing-001)
pub use scorer::RoundScorer;
pub use store::MetricsStore;
pub use orchestrator::OrchestratorMetrics;
pub use strategy::StrategyMetrics;
pub use health::{HealthLevel, HealthReport, HealthScorer};
pub use evaluator::{CompositeEvaluator, AgentLoopOutcome};
pub use macro_feedback::MacroStep;
pub use signal_ingestor::RuntimeSignal;
pub(crate) use anomaly::{BayesianAnomalyDetector, AgentAnomaly};
pub(crate) use arima::ArimaPredictor;
