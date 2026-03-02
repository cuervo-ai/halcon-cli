//! Types for the EvidenceTracker trait interface.

use serde::{Deserialize, Serialize};

/// Quality classification of a tool result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvidenceQuality {
    /// Sufficient readable text evidence.
    Good,
    /// Some text but below minimum threshold.
    Partial,
    /// Empty or error result.
    Empty,
    /// Binary content detected.
    Binary,
}
