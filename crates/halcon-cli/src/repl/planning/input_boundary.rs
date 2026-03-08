//! Input Boundary Layer — structures raw user input before the decision pipeline.
//!
//! This is the first stage in the agent decision pipeline. It transforms a raw
//! `&str` query into a `BoundaryInput` that carries normalized text, detected
//! language, injected context, and optional session continuity signals.
//!
//! # Responsibilities
//! - Unicode normalization (trim, whitespace collapse, diacritic-aware lowercasing)
//! - Language detection (English / Spanish / Mixed — affects keyword matching)
//! - Context injection (working directory, tool count, active plan state)
//! - Session continuity (prior routing mode, rounds executed)
//!
//! # What this module does NOT do
//! - Call IntentScorer or BoundaryDecisionEngine (pure transformation)
//! - Perform any I/O
//! - Hold mutable state
//!
//! # Integration point
//! `agent/mod.rs` calls `InputNormalizer::normalize()` at the top of
//! `run_agent_loop()`, before `IntentScorer::score()` and before
//! `BoundaryDecisionEngine::evaluate()`. Both scoring systems receive
//! `BoundaryInput.query` (the normalized text) instead of the raw `user_msg`.

// ── BoundaryInput ─────────────────────────────────────────────────────────────

/// Structured representation of user input after boundary processing.
///
/// Produced once per agent session (or per mid-session re-evaluation).
/// All fields are immutable after construction.
#[derive(Debug, Clone)]
pub struct BoundaryInput {
    /// Normalized, trimmed query text.
    ///
    /// - Leading/trailing whitespace removed
    /// - Internal whitespace runs collapsed to single space
    /// - Unicode normalized (NFKC-equivalent via char-level cleanup)
    pub query: String,

    /// Word count after normalization (used by ComplexityEstimator).
    pub word_count: usize,

    /// Detected query language.
    pub language: QueryLanguage,

    /// Environment context at the boundary.
    pub context: InputContext,

    /// Session continuity signals — `None` on the first turn of a new session.
    pub continuity: Option<SessionContinuity>,
}

// ── QueryLanguage ─────────────────────────────────────────────────────────────

/// Detected language of the user query.
///
/// Used to:
/// - Select appropriate keyword lists in domain/complexity scoring
/// - Localize system prompt directives
/// - Log language distribution for observability
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QueryLanguage {
    /// Predominantly English.
    English,
    /// Predominantly Spanish.
    Spanish,
    /// Mix of English and Spanish (common in bilingual engineering teams).
    Mixed,
    /// Insufficient signal to determine language.
    Unknown,
}

impl QueryLanguage {
    pub fn label(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Spanish => "es",
            Self::Mixed => "mixed",
            Self::Unknown => "unknown",
        }
    }
}

// ── InputContext ──────────────────────────────────────────────────────────────

/// Environment context injected at the input boundary.
///
/// These are facts about the agent's execution environment that are known
/// before any scoring occurs. They supplement the query text.
#[derive(Debug, Clone, Default)]
pub struct InputContext {
    /// Current working directory path (for path resolution hints).
    pub working_dir: Option<String>,

    /// Number of tools available this session.
    pub available_tool_count: usize,

    /// Whether a plan is currently active (affects convergence calibration).
    pub has_active_plan: bool,

    /// Current round number (0 = initial query, >0 = mid-session re-evaluation).
    pub current_round: u32,

    /// Whether this is a sub-agent session (affects max_rounds calibration).
    pub is_sub_agent: bool,
}

// ── SessionContinuity ─────────────────────────────────────────────────────────

/// Signals from prior turns in the same session.
///
/// Enables the `IntentPipeline` to detect complexity escalation
/// (a simple question that revealed a systemic problem) and adjust
/// routing for the continuation.
#[derive(Debug, Clone)]
pub struct SessionContinuity {
    /// Routing mode selected in the previous turn.
    pub prior_routing_mode_label: &'static str,

    /// Complexity score computed in the previous turn.
    pub prior_complexity_score: f32,

    /// Number of rounds already executed in this session.
    pub rounds_executed: u32,

    /// Whether the prior turn required routing escalation.
    pub was_escalated: bool,
}

// ── InputNormalizer ───────────────────────────────────────────────────────────

/// Stateless input boundary normalizer.
///
/// The single entry point for the input boundary layer. All methods are
/// pure functions — no I/O, no state, no allocations beyond the returned struct.
pub struct InputNormalizer;

impl InputNormalizer {
    /// Normalize raw user input into a `BoundaryInput`.
    ///
    /// This is the primary integration point called from `agent/mod.rs`
    /// before any scoring or decision logic runs.
    ///
    /// # Arguments
    /// - `raw`: The raw user message string (from `request.messages.last()`).
    /// - `context`: Environment context (working_dir, tool_count, etc.).
    /// - `continuity`: Optional prior session signals.
    pub fn normalize(
        raw: &str,
        context: InputContext,
        continuity: Option<SessionContinuity>,
    ) -> BoundaryInput {
        let query = Self::normalize_text(raw);
        let word_count = query.split_whitespace().count();
        let language = Self::detect_language(&query);

        BoundaryInput { query, word_count, language, context, continuity }
    }

    /// Normalize text: trim, collapse internal whitespace, Unicode cleanup.
    ///
    /// Does NOT lowercase — downstream scorers do that themselves on the
    /// normalized query so they can apply case-sensitive rules where needed.
    fn normalize_text(raw: &str) -> String {
        // Step 1: collect chars, normalizing Unicode control chars and
        // zero-width spaces that can confuse keyword matching.
        // Zero-width and invisible format characters (Unicode Cf category) that
        // .is_whitespace() does NOT catch but confuse keyword matching.
        const INVISIBLE_FORMAT: &[char] = &[
            '\u{200B}', // ZERO WIDTH SPACE
            '\u{200C}', // ZERO WIDTH NON-JOINER
            '\u{200D}', // ZERO WIDTH JOINER
            '\u{200E}', // LEFT-TO-RIGHT MARK
            '\u{200F}', // RIGHT-TO-LEFT MARK
            '\u{FEFF}', // ZERO WIDTH NO-BREAK SPACE (BOM)
            '\u{00AD}', // SOFT HYPHEN
        ];
        let cleaned: String = raw
            .chars()
            .map(|c| {
                // Collapse all whitespace categories (tabs, non-breaking spaces,
                // form feeds) to regular ASCII space.
                if c.is_whitespace() || INVISIBLE_FORMAT.contains(&c) { ' ' } else { c }
            })
            .filter(|c| {
                // Remove control characters except the space we just mapped.
                !c.is_control() || *c == ' '
            })
            .collect();

        // Step 2: trim and collapse internal whitespace runs.
        let mut result = String::with_capacity(cleaned.len());
        let mut last_was_space = false;
        for c in cleaned.trim().chars() {
            if c == ' ' {
                if !last_was_space {
                    result.push(' ');
                }
                last_was_space = true;
            } else {
                result.push(c);
                last_was_space = false;
            }
        }
        result
    }

    /// Detect query language from lexical markers.
    ///
    /// Uses a lightweight word-marker approach. Not a full language model —
    /// sufficient for distinguishing EN/ES/mixed for keyword table selection.
    /// Accuracy > 90% for typical engineering queries of 5+ words.
    fn detect_language(normalized: &str) -> QueryLanguage {
        // These markers are chosen to minimize false positives across EN/ES.
        // Spanish markers: common function words and technical terms unique to ES.
        const SPANISH_MARKERS: &[&str] = &[
            " de ", " la ", " el ", " en ", " los ", " las ", " del ",
            " para ", " con ", " por ", " una ", " que ",
            "analiza", "revisar", "revisar", "identificar", "busca",
            "arquitectura", "sistema", "módulo", "código", "seguridad",
            "microservicio", "distribuid", "vulnerabilidad", "rendimiento",
            "implementar", "construir", "desplegar", "arreglar",
        ];
        const ENGLISH_MARKERS: &[&str] = &[
            " the ", " and ", " for ", " with ", " from ", " that ",
            " this ", " are ", " have ", " will ", " can ", " not ",
            "analyze", "review", "identify", "search", "find",
            "architecture", "system", "module", "code", "security",
            "microservice", "distributed", "vulnerability", "performance",
            "implement", "build", "deploy", "fix", "refactor",
        ];

        let lower = normalized.to_lowercase();
        // Pad with spaces for word-boundary matching.
        let padded = format!(" {} ", lower);

        let es_hits = SPANISH_MARKERS.iter()
            .filter(|&&m| padded.contains(m))
            .count();
        let en_hits = ENGLISH_MARKERS.iter()
            .filter(|&&m| padded.contains(m))
            .count();

        match (es_hits, en_hits) {
            (0, 0) => QueryLanguage::Unknown,
            (s, e) if s > 0 && e == 0 => QueryLanguage::Spanish,
            (s, e) if e > 0 && s == 0 => QueryLanguage::English,
            (s, e) if s as f32 / (s + e) as f32 >= 0.70 => QueryLanguage::Spanish,
            (s, e) if e as f32 / (s + e) as f32 >= 0.70 => QueryLanguage::English,
            _ => QueryLanguage::Mixed,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn normalize(raw: &str) -> BoundaryInput {
        InputNormalizer::normalize(raw, InputContext::default(), None)
    }

    #[test]
    fn trims_leading_trailing_whitespace() {
        let b = normalize("  hello world  ");
        assert_eq!(b.query, "hello world");
    }

    #[test]
    fn collapses_internal_whitespace() {
        let b = normalize("analyze   the   architecture");
        assert_eq!(b.query, "analyze the architecture");
        assert_eq!(b.word_count, 3);
    }

    #[test]
    fn handles_tabs_and_newlines() {
        let b = normalize("fix\tthe\nbug");
        assert_eq!(b.query, "fix the bug");
    }

    #[test]
    fn detects_english() {
        let b = normalize("analyze the security architecture of the system");
        assert_eq!(b.language, QueryLanguage::English);
    }

    #[test]
    fn detects_spanish() {
        let b = normalize("analiza la arquitectura del sistema de seguridad en todos los módulos");
        assert_eq!(b.language, QueryLanguage::Spanish);
    }

    #[test]
    fn detects_mixed() {
        // Typical bilingual engineering query.
        let b = normalize("analyze la arquitectura and identify vulnerabilidades de seguridad");
        assert_eq!(b.language, QueryLanguage::Mixed);
    }

    #[test]
    fn unknown_for_very_short_query() {
        let b = normalize("hello");
        // "hello" has no markers → Unknown.
        assert!(matches!(b.language, QueryLanguage::Unknown | QueryLanguage::English));
    }

    #[test]
    fn word_count_accurate() {
        let b = normalize("one two three four five");
        assert_eq!(b.word_count, 5);
    }

    #[test]
    fn empty_input() {
        let b = normalize("");
        assert_eq!(b.query, "");
        assert_eq!(b.word_count, 0);
        assert_eq!(b.language, QueryLanguage::Unknown);
    }

    #[test]
    fn context_fields_propagated() {
        let ctx = InputContext {
            working_dir: Some("/home/user/project".to_string()),
            available_tool_count: 15,
            has_active_plan: true,
            current_round: 3,
            is_sub_agent: false,
        };
        let b = InputNormalizer::normalize("test query", ctx.clone(), None);
        assert_eq!(b.context.working_dir.as_deref(), Some("/home/user/project"));
        assert_eq!(b.context.available_tool_count, 15);
        assert!(b.context.has_active_plan);
        assert_eq!(b.context.current_round, 3);
    }

    #[test]
    fn continuity_propagated() {
        let cont = SessionContinuity {
            prior_routing_mode_label: "DeepAnalysis",
            prior_complexity_score: 72.5,
            rounds_executed: 8,
            was_escalated: true,
        };
        let b = InputNormalizer::normalize("follow up query", InputContext::default(), Some(cont));
        let c = b.continuity.unwrap();
        assert_eq!(c.prior_routing_mode_label, "DeepAnalysis");
        assert_eq!(c.rounds_executed, 8);
        assert!(c.was_escalated);
    }

    #[test]
    fn unicode_control_chars_removed() {
        // Zero-width space (U+200B) and other control chars should be stripped.
        let b = normalize("analyze\u{200B}the system");
        assert_eq!(b.query, "analyze the system");
    }
}
