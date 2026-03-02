//! Types for task complexity estimation (Decision Layer).

use serde::{Deserialize, Serialize};

/// Task complexity tier — determines orchestration strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TaskComplexity {
    /// Single tool call or direct answer. No orchestration needed.
    Simple,
    /// Multi-step but single-domain. Optional orchestration.
    Structured,
    /// Cross-domain work requiring parallel execution.
    MultiDomain,
    /// Extended investigation requiring deep planning.
    LongHorizon,
}
