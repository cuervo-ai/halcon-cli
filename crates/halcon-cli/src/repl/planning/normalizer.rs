//! Input normalization layer — parse free-text user input into structured PermissionMessage.
//!
//! Phase I-2 of Questionnaire SOTA Audit (Feb 14, 2026)
//!
//! This module provides intent classification for user's natural language input during
//! permission dialogues. It supports:
//! - Classic Y/N/A/D keyboard shortcuts (backwards compatible)
//! - Natural language questions ("what does this do?")
//! - Modification requests ("use /tmp/output.txt instead")
//! - Detail requests ("show me the parameters")
//! - Fuzzy matching for typo tolerance

use super::conversation_protocol::{BatchScope, DetailAspect, PermissionMessage};

/// Classify user's free-text input into a PermissionMessage.
///
/// Uses keyword-based intent classification with fuzzy matching.
/// Falls back to AskQuestion if intent is unclear.
pub struct InputNormalizer {
    /// Keyword patterns for intent classification.
    patterns: IntentPatterns,
}

impl InputNormalizer {
    pub fn new() -> Self {
        Self {
            patterns: IntentPatterns::default(),
        }
    }

    /// Parse free-text input into PermissionMessage.
    ///
    /// # Examples
    /// ```
    /// use halcon_cli::repl::input_normalizer::InputNormalizer;
    /// use halcon_cli::repl::conversation_protocol::PermissionMessage;
    ///
    /// let normalizer = InputNormalizer::new();
    ///
    /// // Classic shortcuts
    /// assert!(matches!(normalizer.normalize("y"), PermissionMessage::Approve));
    /// assert!(matches!(normalizer.normalize("n"), PermissionMessage::Reject));
    ///
    /// // Natural language
    /// let msg = normalizer.normalize("what does this do?");
    /// assert!(matches!(msg, PermissionMessage::AskQuestion { .. }));
    /// ```
    pub fn normalize(&self, input: &str) -> PermissionMessage {
        let input_lower = input.trim().to_lowercase();
        if input_lower.is_empty() {
            return PermissionMessage::Reject; // Empty input = reject (fail-safe)
        }

        // Short-circuit for classic Y/N/A/D shortcuts.
        match input_lower.as_str() {
            "y" | "yes" => return PermissionMessage::Approve,
            "n" | "no" => return PermissionMessage::Reject,
            "a" | "always" => {
                return PermissionMessage::BatchApprove {
                    scope: BatchScope::Pattern {
                        tool: "*".into(),
                        pattern: "*".into(),
                    },
                }
            }
            "d" | "deny" => return PermissionMessage::Reject,
            _ => {}
        }

        // Intent classification with keyword matching.
        // Order matters! More specific patterns first.
        if self.patterns.is_approval(&input_lower) {
            PermissionMessage::Approve
        } else if self.patterns.is_rejection(&input_lower) {
            PermissionMessage::Reject
        } else if self.patterns.is_detail_request(&input_lower) {
            // Check detail requests BEFORE questions (more specific)
            let aspect = self.patterns.classify_detail_aspect(&input_lower);
            PermissionMessage::RequestDetails { aspect }
        } else if self.patterns.is_safer_alternative(&input_lower) {
            // Check safer alternatives BEFORE modifications (more specific)
            PermissionMessage::SuggestAlternative {
                suggestion: input.to_string(),
            }
        } else if self.patterns.is_defer(&input_lower) {
            // Check defer BEFORE questions (more specific)
            PermissionMessage::Defer {
                reason: Some(input.to_string()),
            }
        } else if self.patterns.is_modification(&input_lower) {
            PermissionMessage::ModifyParameters {
                clarification: input.to_string(),
            }
        } else if self.patterns.is_question(&input_lower) {
            // Questions last (most general pattern - starts with what/why/etc)
            PermissionMessage::AskQuestion {
                question: input.to_string(),
            }
        } else {
            // Fallback: treat ambiguous input as question.
            PermissionMessage::AskQuestion {
                question: input.to_string(),
            }
        }
    }

    /// Normalize with fuzzy matching for typos (future enhancement).
    ///
    /// For now, delegates to normalize(). Future: Levenshtein distance.
    pub fn normalize_fuzzy(&self, input: &str) -> PermissionMessage {
        // TODO: Add fuzzy matching with Levenshtein distance.
        // For common typos like "appove" → "approve", "rejct" → "reject".
        self.normalize(input)
    }
}

impl Default for InputNormalizer {
    fn default() -> Self {
        Self::new()
    }
}

/// Keyword patterns for intent classification.
#[derive(Clone)]
struct IntentPatterns {
    approval: Vec<&'static str>,
    rejection: Vec<&'static str>,
    question: Vec<&'static str>,
    modification: Vec<&'static str>,
    defer: Vec<&'static str>,
    detail_request: Vec<&'static str>,
    safer_alternative: Vec<&'static str>,
}

impl Default for IntentPatterns {
    fn default() -> Self {
        Self {
            approval: vec![
                "approve",
                "ok",
                "okay",
                "proceed",
                "go ahead",
                "do it",
                "yes",
                "y",
                "sure",
                "fine",
                "accept",
            ],
            rejection: vec![
                "reject",
                "no",
                "n",
                "cancel",
                "abort",
                "stop",
                "deny",
                "nope",
                "nah",
                "refuse",
            ],
            question: vec![
                "what", "why", "how", "when", "where", "which", "who", "explain", "tell me",
                "can you", "could you", "would you", "is this", "does this",
            ],
            modification: vec![
                "use",
                "change",
                "instead",
                "modify",
                "update",
                "replace",
                "swap",
                "switch",
                "different",
            ],
            defer: vec!["wait", "defer", "later", "check", "let me", "pause", "hold"],
            detail_request: vec!["show", "details", "parameters", "params", "more info", "info"],
            safer_alternative: vec![
                "safer",
                "add --dry-run",
                "add --interactive",
                "--dry-run",
                "--interactive",
                "-i",
                "interactive mode",
            ],
        }
    }
}

impl IntentPatterns {
    /// Check if input is an approval intent.
    fn is_approval(&self, input: &str) -> bool {
        self.approval.iter().any(|kw| {
            input == *kw
                || input.starts_with(&format!("{} ", kw))
                || input.ends_with(&format!(" {}", kw))  // Word boundary before
                || input.trim_end_matches(|c: char| !c.is_alphanumeric()) == *kw
        })
    }

    /// Check if input is a rejection intent.
    fn is_rejection(&self, input: &str) -> bool {
        self.rejection.iter().any(|kw| {
            input == *kw
                || input.starts_with(&format!("{} ", kw))
                || input.ends_with(&format!(" {}", kw))  // Word boundary before
                || input.trim_end_matches(|c: char| !c.is_alphanumeric()) == *kw
        })
    }

    /// Check if input is a question intent (starts with question word).
    fn is_question(&self, input: &str) -> bool {
        self.question.iter().any(|kw| input.starts_with(kw))
    }

    /// Check if input is a modification request.
    fn is_modification(&self, input: &str) -> bool {
        self.modification.iter().any(|kw| input.contains(kw))
    }

    /// Check if input is a defer request.
    fn is_defer(&self, input: &str) -> bool {
        self.defer.iter().any(|kw| input.contains(kw))
    }

    /// Check if input is a detail request.
    fn is_detail_request(&self, input: &str) -> bool {
        self.detail_request.iter().any(|kw| input.contains(kw))
    }

    /// Check if input suggests a safer alternative.
    fn is_safer_alternative(&self, input: &str) -> bool {
        self.safer_alternative.iter().any(|kw| input.contains(kw))
    }

    /// Classify which detail aspect is being requested.
    fn classify_detail_aspect(&self, input: &str) -> DetailAspect {
        if input.contains("param") {
            DetailAspect::Parameters
        } else if input.contains("risk") || input.contains("safe") || input.contains("danger") {
            DetailAspect::RiskAssessment
        } else if input.contains("history") || input.contains("previous") || input.contains("before") {
            DetailAspect::History
        } else {
            DetailAspect::WhatItDoes
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_classic_shortcuts() {
        let n = InputNormalizer::new();
        assert!(matches!(n.normalize("y"), PermissionMessage::Approve));
        assert!(matches!(n.normalize("yes"), PermissionMessage::Approve));
        assert!(matches!(n.normalize("n"), PermissionMessage::Reject));
        assert!(matches!(n.normalize("no"), PermissionMessage::Reject));
        assert!(matches!(
            n.normalize("a"),
            PermissionMessage::BatchApprove { .. }
        ));
        assert!(matches!(
            n.normalize("always"),
            PermissionMessage::BatchApprove { .. }
        ));
        assert!(matches!(n.normalize("d"), PermissionMessage::Reject));
        assert!(matches!(n.normalize("deny"), PermissionMessage::Reject));
    }

    #[test]
    fn normalize_approval_variations() {
        let n = InputNormalizer::new();
        assert!(matches!(n.normalize("ok"), PermissionMessage::Approve));
        assert!(matches!(n.normalize("okay"), PermissionMessage::Approve));
        assert!(matches!(n.normalize("proceed"), PermissionMessage::Approve));
        assert!(matches!(n.normalize("go ahead"), PermissionMessage::Approve));
        assert!(matches!(n.normalize("do it"), PermissionMessage::Approve));
        assert!(matches!(n.normalize("sure"), PermissionMessage::Approve));
    }

    #[test]
    fn normalize_rejection_variations() {
        let n = InputNormalizer::new();
        assert!(matches!(n.normalize("reject"), PermissionMessage::Reject));
        assert!(matches!(n.normalize("cancel"), PermissionMessage::Reject));
        assert!(matches!(n.normalize("abort"), PermissionMessage::Reject));
        assert!(matches!(n.normalize("stop"), PermissionMessage::Reject));
        assert!(matches!(n.normalize("nope"), PermissionMessage::Reject));
    }

    #[test]
    fn normalize_questions() {
        let n = InputNormalizer::new();

        let msg = n.normalize("what does this do?");
        assert!(matches!(msg, PermissionMessage::AskQuestion { .. }));

        let msg = n.normalize("why delete this?");
        assert!(matches!(msg, PermissionMessage::AskQuestion { .. }));

        let msg = n.normalize("how does this work?");
        assert!(matches!(msg, PermissionMessage::AskQuestion { .. }));

        let msg = n.normalize("is this safe?");
        assert!(matches!(msg, PermissionMessage::AskQuestion { .. }));
    }

    #[test]
    fn normalize_modifications() {
        let n = InputNormalizer::new();

        let msg = n.normalize("use /tmp/output.txt instead");
        assert!(matches!(msg, PermissionMessage::ModifyParameters { .. }));

        let msg = n.normalize("change the path to /tmp/test.txt");
        assert!(matches!(msg, PermissionMessage::ModifyParameters { .. }));

        let msg = n.normalize("replace with different file");
        assert!(matches!(msg, PermissionMessage::ModifyParameters { .. }));
    }

    #[test]
    fn normalize_defer() {
        let n = InputNormalizer::new();

        let msg = n.normalize("wait, let me check first");
        assert!(matches!(msg, PermissionMessage::Defer { .. }));

        let msg = n.normalize("defer this decision");
        assert!(matches!(msg, PermissionMessage::Defer { .. }));

        let msg = n.normalize("let me investigate");
        assert!(matches!(msg, PermissionMessage::Defer { .. }));
    }

    #[test]
    fn normalize_detail_requests() {
        let n = InputNormalizer::new();

        let msg = n.normalize("show me the parameters");
        if let PermissionMessage::RequestDetails { aspect } = msg {
            assert_eq!(aspect, DetailAspect::Parameters);
        } else {
            panic!("Expected RequestDetails");
        }

        let msg = n.normalize("show details");
        assert!(matches!(msg, PermissionMessage::RequestDetails { .. }));

        let msg = n.normalize("more info please");
        assert!(matches!(msg, PermissionMessage::RequestDetails { .. }));
    }

    #[test]
    fn normalize_detail_aspect_classification() {
        let n = InputNormalizer::new();

        let msg = n.normalize("show params");
        if let PermissionMessage::RequestDetails { aspect } = msg {
            assert_eq!(aspect, DetailAspect::Parameters);
        } else {
            panic!("Expected RequestDetails");
        }

        let msg = n.normalize("is this safe?");
        // This is a question, not a detail request, so should be AskQuestion
        assert!(matches!(msg, PermissionMessage::AskQuestion { .. }));

        let msg = n.normalize("show risk assessment");
        if let PermissionMessage::RequestDetails { aspect } = msg {
            assert_eq!(aspect, DetailAspect::RiskAssessment);
        } else {
            panic!("Expected RequestDetails");
        }

        let msg = n.normalize("show previous history");
        if let PermissionMessage::RequestDetails { aspect } = msg {
            assert_eq!(aspect, DetailAspect::History);
        } else {
            panic!("Expected RequestDetails");
        }
    }

    #[test]
    fn normalize_safer_alternatives() {
        let n = InputNormalizer::new();

        let msg = n.normalize("add --dry-run flag");
        assert!(matches!(msg, PermissionMessage::SuggestAlternative { .. }));

        let msg = n.normalize("use interactive mode");
        assert!(matches!(msg, PermissionMessage::SuggestAlternative { .. }));

        let msg = n.normalize("make it safer");
        assert!(matches!(msg, PermissionMessage::SuggestAlternative { .. }));
    }

    #[test]
    fn normalize_empty_input_failsafe() {
        let n = InputNormalizer::new();
        assert!(matches!(n.normalize(""), PermissionMessage::Reject));
        assert!(matches!(n.normalize("   "), PermissionMessage::Reject));
    }

    #[test]
    fn normalize_ambiguous_fallback() {
        let n = InputNormalizer::new();

        // Ambiguous input should fall back to AskQuestion
        let msg = n.normalize("hmm, not sure about this");
        assert!(matches!(msg, PermissionMessage::AskQuestion { .. }));

        let msg = n.normalize("maybe later");
        // "later" triggers defer
        assert!(matches!(msg, PermissionMessage::Defer { .. }));
    }

    #[test]
    fn normalize_case_insensitive() {
        let n = InputNormalizer::new();
        assert!(matches!(n.normalize("YES"), PermissionMessage::Approve));
        assert!(matches!(n.normalize("No"), PermissionMessage::Reject));
        assert!(matches!(
            n.normalize("PROCEED"),
            PermissionMessage::Approve
        ));
    }

    #[test]
    fn normalize_whitespace_trimming() {
        let n = InputNormalizer::new();
        assert!(matches!(
            n.normalize("  y  "),
            PermissionMessage::Approve
        ));
        assert!(matches!(
            n.normalize("\n\nyes\n\n"),
            PermissionMessage::Approve
        ));
    }

    #[test]
    fn normalize_with_punctuation() {
        let n = InputNormalizer::new();

        let msg = n.normalize("what does this do?");
        assert!(matches!(msg, PermissionMessage::AskQuestion { .. }));

        let msg = n.normalize("ok!");
        assert!(matches!(msg, PermissionMessage::Approve));

        let msg = n.normalize("no!!!");
        assert!(matches!(msg, PermissionMessage::Reject));
    }

    #[test]
    fn fuzzy_normalize_delegates_to_normalize() {
        let n = InputNormalizer::new();
        // For now, fuzzy matching just delegates to normalize
        let msg1 = n.normalize("approve");
        let msg2 = n.normalize_fuzzy("approve");
        assert_eq!(
            std::mem::discriminant(&msg1),
            std::mem::discriminant(&msg2)
        );
    }

    #[test]
    fn patterns_approval_matching() {
        let p = IntentPatterns::default();
        assert!(p.is_approval("approve"));
        assert!(p.is_approval("ok"));
        assert!(p.is_approval("proceed"));
        assert!(!p.is_approval("maybe"));
    }

    #[test]
    fn patterns_question_starts_with() {
        let p = IntentPatterns::default();
        assert!(p.is_question("what is this?"));
        assert!(p.is_question("why delete?"));
        assert!(!p.is_question("i don't know what to do"));
    }

    #[test]
    fn patterns_modification_contains() {
        let p = IntentPatterns::default();
        assert!(p.is_modification("please use different file"));
        assert!(p.is_modification("change the path"));
        assert!(!p.is_modification("approve this"));
    }
}
