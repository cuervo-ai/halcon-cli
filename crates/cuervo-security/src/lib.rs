//! Cross-cutting security module for Cuervo CLI.
//!
//! Implements:
//! - PII detection via regex patterns (SIMD-accelerated RegexSet)
//! - Permission enforcement for tool execution
//! - Content sanitization before sending to external APIs
//!
//! Sprint 8 will implement the full PII detection and audit system.

pub mod guardrails;
pub mod pii;

pub use guardrails::{
    builtin_guardrails, has_blocking_violation, run_guardrails, Guardrail, GuardrailAction,
    GuardrailCheckpoint, GuardrailResult, GuardrailRuleConfig, GuardrailsConfig,
    RegexGuardrail,
};
pub use pii::PiiDetector;
