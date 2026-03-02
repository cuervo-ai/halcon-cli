//! Types for the SLA / BudgetManager trait interface.

use serde::{Deserialize, Serialize};

/// SLA performance tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlaMode {
    /// Optimised for speed. No orchestration, minimal planning.
    Fast,
    /// Default mode. Moderate planning and optional orchestration.
    Balanced,
    /// Thoroughness over speed. Full orchestration, deep planning, retries.
    Deep,
}
