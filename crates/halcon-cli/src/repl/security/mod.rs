//! Security subsystem for Halcon.
//!
//! Provides risk classification, command blacklisting, permission lifecycle,
//! tool trust scoring, and sub-agent contract validation:
//! - blacklist: Runtime command pattern matching against catastrophic/dangerous patterns
//! - output_risk: Scores agent outputs for risk signals (PII, secrets, etc.)
//! - risk_tier: Classifies file edits and writes into risk tiers (Low/Medium/High/Critical)
//! - lifecycle: Permission rule lifecycle — load, reload, cleanup for ConversationalPermission
//! - tool_policy: Classifies tools as ReadOnly / Execution / Analysis / External (pub(crate))
//! - tool_trust: Per-tool reputation scoring (success/failure history) within a session (pub(crate))
//! - subagent_contract: Validates sub-agent results against their assigned contracts

pub mod blacklist;
pub mod output_risk;
pub mod risk_tier;
pub mod lifecycle;
pub(crate) mod tool_policy;
pub(crate) mod tool_trust;
pub mod subagent_contract;

// Re-exports for public types used by callers outside security/
pub use blacklist::{analyze_command, DangerousPattern, SafetyAnalysis};
pub use output_risk::{OutputRiskFlag, OutputRiskReport, score_tool_args, score_model_output};
pub use risk_tier::{RiskTier, RiskTierClassifier};
pub use lifecycle::PermissionLifecycle;
pub(crate) use tool_policy::{ToolCategory, classify as classify_tool, tools_to_remove};
pub(crate) use tool_trust::{ToolMetrics, ToolTrustScorer, TrustDecision};
pub use subagent_contract::{
    SubAgentContract, SubAgentContractValidator, ValidationResult, ValidationStatus,
    RejectionReason, StepType,
};

// C-2: files migrated from repl/ root
pub mod adaptive_prompt;
pub mod authorization;
pub mod circuit_breaker;
pub mod conversational;
pub mod idempotency;
pub mod permissions;
pub mod response_cache;
pub mod rule_matcher;
pub mod schema_validator;
pub mod validation;

// Re-exports — preserve API surface for callers outside security/
pub use adaptive_prompt::AdaptivePromptBuilder;
pub use authorization::{AuthorizationMiddleware, AuthorizationPolicy, AuthorizationState};
pub use circuit_breaker::{BreakerState, ProviderBreaker};
pub use conversational::ConversationalPermissionHandler;
pub use permissions::PermissionChecker;
pub use rule_matcher::RuleMatcher;
pub(crate) use schema_validator::preflight_validate;
pub use validation::PermissionValidator;
