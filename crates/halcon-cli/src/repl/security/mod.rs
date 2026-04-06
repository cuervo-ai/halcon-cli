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
pub mod denial_tracker;
pub mod lifecycle;
pub mod output_risk;
pub(crate) mod permission_pipeline;
pub mod risk_tier;
pub mod subagent_contract;
pub(crate) mod tool_policy;
pub(crate) mod tool_trust;

// Trust chain gates (Gate 1-3)
pub mod config_trust;
pub mod workspace_trust;

// Re-exports for public types used by callers outside security/

// C-2: files migrated from repl/ root
pub mod adaptive_prompt;
pub mod authorization;
pub mod circuit_breaker;
pub mod conversation_protocol;
pub mod conversation_state;
pub mod conversational;
pub mod idempotency;
pub mod permissions;
pub mod resilience;
pub mod response_cache;
pub mod retry_mutation;
pub mod rule_matcher;
pub mod schema_validator;
pub mod validation;

// Re-exports — preserve API surface for callers outside security/
