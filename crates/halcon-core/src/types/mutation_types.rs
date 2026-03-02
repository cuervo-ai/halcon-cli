//! Types for retry mutation strategy.

use serde::{Deserialize, Serialize};

/// Individual mutation axis applied during retry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MutationAxis {
    /// Removed tools that repeatedly failed.
    ToolExposureReduced { removed: Vec<String> },
    /// Increased temperature to diversify LLM output.
    TemperatureIncreased { from: f32, to: f32 },
    /// Reduced plan depth to force replanning.
    PlanDepthReduced { from: u32, to: u32 },
    /// Switched to a different model.
    ModelFallback { from: String, to: String },
}

/// Record of what changed between retry attempts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationRecord {
    pub mutations: Vec<MutationAxis>,
    pub retry_number: u32,
}

/// Mutable parameters that can be adjusted between retries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryParams {
    pub temperature: f32,
    pub plan_depth: u32,
    pub model_name: String,
    pub available_tools: Vec<String>,
}

/// Tool failure record from a previous attempt (used by mutation strategy).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFailureRecord {
    pub tool_name: String,
    pub failure_count: u32,
}
