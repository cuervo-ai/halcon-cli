//! Conversational permission protocol — replaces bool channel with structured messages.
//!
//! Phase I-1 of Questionnaire SOTA Audit (Feb 14, 2026)
//!
//! This module defines the message protocol for multi-turn permission dialogues
//! between the TUI and the PermissionChecker. Instead of a simple bool (approve/reject),
//! users can now ask questions, request modifications, defer decisions, and more.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Message from TUI to PermissionChecker during tool approval dialogue.
///
/// Replaces the `bool` channel with a structured enum that supports:
/// - Free-text clarifications
/// - Parameter modifications
/// - Progressive disclosure requests
/// - Batch approvals
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PermissionMessage {
    /// User approves the tool call as-is.
    Approve,

    /// User rejects outright (no clarification).
    Reject,

    /// User wants to modify tool parameters.
    ///
    /// Text contains natural language description of change.
    ///
    /// # Examples
    /// - "use /tmp/output.txt instead of /tmp/test.txt"
    /// - "add --dry-run flag"
    /// - "change rm to rm -i for interactive mode"
    ModifyParameters { clarification: String },

    /// User asks a question about the tool call.
    ///
    /// # Examples
    /// - "What file is this?"
    /// - "Why delete it?"
    /// - "Is this safe?"
    AskQuestion { question: String },

    /// User defers decision (wants to check something first).
    ///
    /// Timeout will eventually handle this as rejection (fail-safe).
    Defer { reason: Option<String> },

    /// User approves with condition.
    ///
    /// # Examples
    /// - "approve if file size < 1MB"
    /// - "approve if no errors in previous step"
    ConditionalApprove { condition: String },

    /// User suggests safer alternative.
    ///
    /// # Examples
    /// - "add --dry-run flag"
    /// - "use --interactive mode"
    /// - "run in a sandbox first"
    SuggestAlternative { suggestion: String },

    /// User wants more details before deciding.
    ///
    /// Triggers progressive disclosure (show parameters, risk, history, etc).
    RequestDetails { aspect: DetailAspect },

    /// User wants to see preview/diff again with changes.
    RequestPreview,

    /// User approves all similar operations for limited time/count.
    ///
    /// # Examples
    /// - "approve next 5 file_write"
    /// - "approve file_write for 5 minutes"
    /// - "approve all file operations on /tmp/*"
    BatchApprove { scope: BatchScope },
}

/// What aspect of the tool call to show details for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetailAspect {
    /// Show all parameters with values.
    Parameters,

    /// Show what the tool will do (human-readable explanation).
    WhatItDoes,

    /// Show risk assessment (file size, destructiveness, reversibility).
    RiskAssessment,

    /// Show previous similar operations and their outcomes.
    History,
}

impl DetailAspect {
    /// Human-readable label for UI.
    pub fn label(&self) -> &'static str {
        match self {
            DetailAspect::Parameters => "Parameters",
            DetailAspect::WhatItDoes => "What It Does",
            DetailAspect::RiskAssessment => "Risk Assessment",
            DetailAspect::History => "History",
        }
    }
}

/// Scope for batch approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BatchScope {
    /// Approve next N operations of this tool.
    Count { tool: String, count: u32 },

    /// Approve operations of this tool for N seconds.
    Duration { tool: String, seconds: u64 },

    /// Approve all operations matching pattern.
    ///
    /// Pattern syntax: glob-style matching on tool name.
    /// Examples: "file_*", "bash", "*"
    Pattern { tool: String, pattern: String },
}

impl BatchScope {
    /// Check if this scope covers the given tool at the current time.
    pub fn covers(&self, tool: &str, elapsed: Duration, consumed: u32) -> bool {
        match self {
            BatchScope::Count { tool: scope_tool, count } => {
                tool == scope_tool && consumed < *count
            }
            BatchScope::Duration { tool: scope_tool, seconds } => {
                tool == scope_tool && elapsed.as_secs() < *seconds
            }
            BatchScope::Pattern { pattern, .. } => {
                Self::matches_glob(tool, pattern)
            }
        }
    }

    /// Simple glob matching (supports * wildcard only).
    fn matches_glob(tool: &str, pattern: &str) -> bool {
        if pattern == "*" {
            return true;
        }
        if pattern.contains('*') {
            let parts: Vec<&str> = pattern.split('*').collect();
            if parts.len() == 2 {
                tool.starts_with(parts[0]) && tool.ends_with(parts[1])
            } else {
                tool == pattern
            }
        } else {
            tool == pattern
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_message_serialize_roundtrip() {
        let msg = PermissionMessage::ModifyParameters {
            clarification: "use /tmp/output.txt".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: PermissionMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn detail_aspect_labels() {
        assert_eq!(DetailAspect::Parameters.label(), "Parameters");
        assert_eq!(DetailAspect::WhatItDoes.label(), "What It Does");
        assert_eq!(DetailAspect::RiskAssessment.label(), "Risk Assessment");
        assert_eq!(DetailAspect::History.label(), "History");
    }

    #[test]
    fn batch_scope_count_coverage() {
        let scope = BatchScope::Count {
            tool: "file_write".into(),
            count: 3,
        };
        assert!(scope.covers("file_write", Duration::from_secs(0), 0));
        assert!(scope.covers("file_write", Duration::from_secs(0), 2));
        assert!(!scope.covers("file_write", Duration::from_secs(0), 3));
        assert!(!scope.covers("file_read", Duration::from_secs(0), 0));
    }

    #[test]
    fn batch_scope_duration_coverage() {
        let scope = BatchScope::Duration {
            tool: "bash".into(),
            seconds: 60,
        };
        assert!(scope.covers("bash", Duration::from_secs(30), 0));
        assert!(scope.covers("bash", Duration::from_secs(59), 0));
        assert!(!scope.covers("bash", Duration::from_secs(60), 0));
        assert!(!scope.covers("file_write", Duration::from_secs(30), 0));
    }

    #[test]
    fn batch_scope_pattern_wildcard() {
        let scope = BatchScope::Pattern {
            tool: "*".into(),
            pattern: "*".into(),
        };
        assert!(scope.covers("bash", Duration::from_secs(0), 0));
        assert!(scope.covers("file_write", Duration::from_secs(0), 0));
        assert!(scope.covers("anything", Duration::from_secs(0), 0));
    }

    #[test]
    fn batch_scope_pattern_prefix() {
        let scope = BatchScope::Pattern {
            tool: "file_*".into(),
            pattern: "file_*".into(),
        };
        assert!(scope.covers("file_write", Duration::from_secs(0), 0));
        assert!(scope.covers("file_read", Duration::from_secs(0), 0));
        assert!(!scope.covers("bash", Duration::from_secs(0), 0));
    }

    #[test]
    fn batch_scope_pattern_exact() {
        let scope = BatchScope::Pattern {
            tool: "bash".into(),
            pattern: "bash".into(),
        };
        assert!(scope.covers("bash", Duration::from_secs(0), 0));
        assert!(!scope.covers("file_write", Duration::from_secs(0), 0));
    }

    #[test]
    fn permission_message_variants() {
        // Ensure all variants are constructible.
        let _ = PermissionMessage::Approve;
        let _ = PermissionMessage::Reject;
        let _ = PermissionMessage::ModifyParameters {
            clarification: "test".into(),
        };
        let _ = PermissionMessage::AskQuestion {
            question: "why?".into(),
        };
        let _ = PermissionMessage::Defer { reason: None };
        let _ = PermissionMessage::ConditionalApprove {
            condition: "if safe".into(),
        };
        let _ = PermissionMessage::SuggestAlternative {
            suggestion: "use --dry-run".into(),
        };
        let _ = PermissionMessage::RequestDetails {
            aspect: DetailAspect::Parameters,
        };
        let _ = PermissionMessage::RequestPreview;
        let _ = PermissionMessage::BatchApprove {
            scope: BatchScope::Count {
                tool: "test".into(),
                count: 5,
            },
        };
    }
}
