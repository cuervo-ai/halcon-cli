//! Regex-based intent classifier for model routing.
//!
//! Classifies a user query into a `TaskIntent` in < 1 µs using ordered
//! regex rules — no LLM call, no network I/O.  The result drives the
//! `IntentBasedStrategy` in the routing pipeline.
//!
//! Rules are evaluated in declaration order; the first match wins.
//! Add more specific rules before more general ones.

use std::sync::OnceLock;

/// Compiled rule: (intent, pattern string).  The regex is compiled once at
/// first access via `OnceLock` so construction is zero-cost at startup.
struct Rule {
    intent: TaskIntent,
    pattern: &'static str,
}

const RULES: &[Rule] = &[
    // ── High-cost tasks ──────────────────────────────────────────────────────
    // Match the imperative verb at or near the start of the query.
    // Keeping patterns simple avoids edge-cases with multi-word `.{0,N}` spans
    // that can silently fail in NFA engines when backtracking is limited.
    Rule {
        intent: TaskIntent::CodeGeneration,
        pattern: r"(?i)^\s*(?:write|implement|create|build|generate)\b",
    },
    Rule {
        intent: TaskIntent::CodeReview,
        pattern: r"(?i)^\s*(?:review|audit|critique|improve)\b",
    },
    Rule {
        intent: TaskIntent::Debugging,
        pattern: r"(?i)\b(?:fix|debug|error|bug|exception|traceback|crash|failing|panic|segfault)\b",
    },
    Rule {
        intent: TaskIntent::Refactoring,
        pattern: r"(?i)\b(?:refactor|restructure|clean up|simplify|extract|rename|reorganize)\b",
    },
    // ── Medium-cost tasks ────────────────────────────────────────────────────
    Rule {
        intent: TaskIntent::DataAnalysis,
        pattern: r"(?i)\b(?:analyze|analysis|statistics|metrics|chart|plot|trend|anomaly|correlation)\b",
    },
    Rule {
        intent: TaskIntent::Research,
        pattern: r"(?i)\b(?:explain|describe|what is|how does|compare|contrast|overview)\b",
    },
    Rule {
        intent: TaskIntent::Planning,
        pattern: r"(?i)\b(?:plan|design|architect|strategy|roadmap|approach for)\b",
    },
    // ── Low-cost tasks ───────────────────────────────────────────────────────
    Rule {
        intent: TaskIntent::Summarization,
        pattern: r"(?i)\b(?:summarize|summary|tl;?dr|recap|condense|shorten)\b",
    },
    Rule {
        intent: TaskIntent::Translation,
        pattern: r"(?i)\b(?:translate|translation|in spanish|in french|in german|en español)\b",
    },
    // Ends with a question mark → question/answer task.
    Rule {
        intent: TaskIntent::QuestionAnswer,
        pattern: r"[?？]\s*$",
    },
    // Conversation: short messages (≤ 60 chars) that matched none of the above.
    Rule {
        intent: TaskIntent::Conversation,
        pattern: r"(?s)^.{0,60}$",
    },
];

/// Coarse categories of user intent, mapped to routing tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskIntent {
    // High-cost — route to FLAGSHIP
    CodeGeneration,
    CodeReview,
    Debugging,
    Refactoring,
    // Medium-cost — route to BALANCED
    DataAnalysis,
    Research,
    Planning,
    QuestionAnswer,
    // Low-cost — route to ECONOMY
    Summarization,
    Translation,
    // Cheap — route to FAST
    Conversation,
    // Fallback
    Unknown,
}

impl TaskIntent {
    /// Human-readable label used in tracing and metrics.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CodeGeneration => "code_generation",
            Self::CodeReview     => "code_review",
            Self::Debugging      => "debugging",
            Self::Refactoring    => "refactoring",
            Self::DataAnalysis   => "data_analysis",
            Self::Research       => "research",
            Self::Planning       => "planning",
            Self::QuestionAnswer => "question_answer",
            Self::Summarization  => "summarization",
            Self::Translation    => "translation",
            Self::Conversation   => "conversation",
            Self::Unknown        => "unknown",
        }
    }
}

impl std::fmt::Display for TaskIntent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Result of intent classification.
#[derive(Debug, Clone)]
pub struct IntentResult {
    pub intent: TaskIntent,
    /// Confidence in [0.0, 1.0].  Regex matches return 0.85; unknown = 0.0.
    pub confidence: f32,
    /// How the classification was produced (`"regex"` or `"fallback"`).
    pub method: &'static str,
}

/// Fast, pure-Rust intent classifier.
///
/// Uses ordered regex rules compiled once at first call.  Suitable for the
/// hot-path of the routing pipeline.
pub struct IntentClassifier {
    _private: (),
}

// Compiled regexes: OnceLock<Vec<(TaskIntent, Regex)>>
fn compiled_rules() -> &'static Vec<(TaskIntent, regex::Regex)> {
    static COMPILED: OnceLock<Vec<(TaskIntent, regex::Regex)>> = OnceLock::new();
    COMPILED.get_or_init(|| {
        RULES
            .iter()
            .map(|r| {
                (
                    r.intent,
                    regex::Regex::new(r.pattern)
                        .unwrap_or_else(|e| panic!("invalid intent regex for {:?}: {e}", r.intent)),
                )
            })
            .collect()
    })
}

impl IntentClassifier {
    pub fn new() -> Self {
        // Eagerly compile rules at construction so the first `classify()` call
        // does not pay the compilation cost.
        let _ = compiled_rules();
        Self { _private: () }
    }

    /// Classify `text` and return the matching intent with confidence.
    ///
    /// Runs in < 1 µs for typical query lengths.  Never allocates on the heap
    /// (the regex match itself may, but the result does not).
    pub fn classify(&self, text: &str) -> IntentResult {
        for (intent, re) in compiled_rules() {
            if re.is_match(text) {
                return IntentResult {
                    intent: *intent,
                    confidence: 0.85,
                    method: "regex",
                };
            }
        }
        IntentResult {
            intent: TaskIntent::Unknown,
            confidence: 0.0,
            method: "fallback",
        }
    }
}

impl Default for IntentClassifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(text: &str) -> TaskIntent {
        IntentClassifier::new().classify(text).intent
    }

    #[test]
    fn code_generation_detected() {
        // Pattern: starts with an imperative verb
        assert_eq!(classify("Write a Rust function that parses JSON"), TaskIntent::CodeGeneration);
        assert_eq!(classify("Implement a REST API endpoint in Python"), TaskIntent::CodeGeneration);
        assert_eq!(classify("Create a React component for the login form"), TaskIntent::CodeGeneration);
        assert_eq!(classify("Build a CLI tool"), TaskIntent::CodeGeneration);
        assert_eq!(classify("Generate the migration script"), TaskIntent::CodeGeneration);
    }

    #[test]
    fn debugging_detected() {
        assert_eq!(classify("Fix this error: thread panicked at index out of bounds"), TaskIntent::Debugging);
        assert_eq!(classify("There is a bug in the parser"), TaskIntent::Debugging);
        assert_eq!(classify("Getting an exception in production"), TaskIntent::Debugging);
    }

    #[test]
    fn summarization_detected() {
        assert_eq!(classify("Summarize this article in 3 bullet points"), TaskIntent::Summarization);
        assert_eq!(classify("tl;dr of the following text"), TaskIntent::Summarization);
        assert_eq!(classify("Give me a recap of this document"), TaskIntent::Summarization);
    }

    #[test]
    fn short_message_is_conversation() {
        // Short strings that don't match any keyword → Conversation
        assert_eq!(classify("Hello there"), TaskIntent::Conversation);
        assert_eq!(classify("Thanks"), TaskIntent::Conversation);
    }

    #[test]
    fn question_mark_is_question() {
        // "What is" / "How does" match Research first (correct: both route to BALANCED).
        // Pure question-mark queries without Research keywords hit QuestionAnswer.
        assert_eq!(classify("What is Rust's ownership model?"), TaskIntent::Research);
        assert_eq!(classify("How does tokio work?"), TaskIntent::Research);
        // A plain question that doesn't contain Research keywords → QuestionAnswer
        assert_eq!(classify("Is this correct?"), TaskIntent::QuestionAnswer);
    }

    #[test]
    fn unknown_for_long_unmatched() {
        // > 60 chars and no keyword → Unknown
        let result = IntentClassifier::new().classify(
            "Xyzzy frobnicate the quux through the blargh and wibble the flibber in a cyclic manner here"
        );
        assert_eq!(result.intent, TaskIntent::Unknown);
        assert_eq!(result.confidence, 0.0);
        assert_eq!(result.method, "fallback");
    }

    #[test]
    fn intent_display() {
        assert_eq!(TaskIntent::CodeGeneration.to_string(), "code_generation");
        assert_eq!(TaskIntent::Unknown.to_string(), "unknown");
    }
}
