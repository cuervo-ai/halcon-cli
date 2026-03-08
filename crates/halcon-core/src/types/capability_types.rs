//! Capability type system — plan execution requirements for the Step 7 hard gate.
//!
//! `CapabilityDescriptor` is embedded in `ExecutionPlan` to declare what the plan needs.
//! `CapabilityValidator` (in halcon-cli domain) compares requirements against runtime state.
//!
//! # Design
//! - `CapabilityDescriptor` defaults to all-empty fields → always passes validation.
//! - Plans with no declared requirements have zero-drift behavior through the gate.
//! - Only plans with explicit tool/modality requirements trigger gate evaluation.

use serde::{Deserialize, Serialize};

/// Interaction modalities a plan step or execution environment may support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    /// Textual input/output — always supported.
    Text,
    /// Tool invocation capability — requires ≥1 available tool in the session.
    ToolUse,
    /// Vision / image analysis — requires a multimodal model and provider.
    Vision,
}

/// Declared execution requirements attached to an `ExecutionPlan`.
///
/// Populated by `agent/mod.rs` post-parse via `ExecutionPlan::derive_capability_descriptor()`.
/// All fields default to empty/zero — gate always passes for legacy/undecorated plans.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapabilityDescriptor {
    /// Tool names that the plan explicitly requires at execution time.
    /// Checked against `EnvironmentCapabilities::available_tools` at the gate.
    #[serde(default)]
    pub required_tools: Vec<String>,
    /// Interaction modalities required by the plan.
    /// Checked against `EnvironmentCapabilities::supported_modalities` at the gate.
    #[serde(default)]
    pub required_modalities: Vec<Modality>,
    /// Estimated cumulative token cost for the plan. 0 = unknown — skips budget check.
    #[serde(default)]
    pub estimated_token_cost: usize,
}
