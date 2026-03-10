//! Task complexity and type analysis — SOTA 2026 Scored Multi-Rule Classifier (SMRC).
//!
//! ## Architecture
//!
//! Replaces the legacy "first-match-wins" keyword scanner with a scored
//! multi-rule classifier where every matching signal contributes to a
//! per-type score bucket, and the winner is the bucket with the highest total.
//!
//! ### How it works
//!
//! ```text
//! query ──► normalise ──► for each ClassifierRule:
//!                            for each keyword:
//!                               if matches(query, kw):
//!                                  scores[rule.task_type] += rule.base_score
//!                                  signals.push(kw)
//!           ──► winner  = argmax(scores)
//!           ──► confidence = winner_score / Σ(all positive scores)
//!           ──► if confidence < CONFIDENCE_FLOOR → General
//! ```
//!
//! ### Why this beats first-match-wins
//!
//! | Query | Legacy result | SMRC result |
//! |-------|---------------|-------------|
//! | "create a security audit report" | CodeGeneration | Research (audit×3 > create×1) |
//! | "fix the vulnerability in auth" | Debugging | Research (vulnerability×3 > fix×2) |
//! | "git commit with message 'fix bug'" | GitOperation | GitOperation (git commit×4 > fix+bug×3) |
//! | "verify SOC2 compliance controls" | General (after P1-C) | Research (soc2×3 + compliance×3 + verify×1) |
//!
//! ### Fixes vs legacy implementation
//!
//! | ID | Bug | Legacy | Fixed |
//! |----|-----|--------|-------|
//! | STAT-PANIC-006 | `start = pos + 1` breaks UTF-8 char boundaries | panic on accented chars | char-boundary aware |
//! | STAT-LOGIC-001 | Operator precedence ambiguity in `before_ok` | accidental correctness | explicit parentheses |
//! | SOTA-CLASSIFY-001 | First-match priority collision | wrong type on mixed signals | score-based resolution |
//! | SOTA-HASH-001 | SHA-256 of raw text — every phrasing is a cold-start | UCB1 never learns | semantic normalization |
//!
//! ### Extensibility
//!
//! Add keywords by editing `CLASSIFIER_RULES`. No logic changes needed.
//! Add a new `TaskType` by:
//! 1. Adding the variant to the enum.
//! 2. Adding `as_str()` / `from_str()` entries.
//! 3. Adding one or more `ClassifierRule` entries in `CLASSIFIER_RULES`.

use sha2::{Digest, Sha256};

// ─── Constants ────────────────────────────────────────────────────────────────

/// Minimum confidence to accept a classification.  Below this threshold the
/// winning type does not have enough signal mass to be trustworthy and the
/// result is `General`.
pub const CONFIDENCE_FLOOR: f32 = 0.30;

// ─── Types ────────────────────────────────────────────────────────────────────

/// Task complexity derived from query length and keyword presence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskComplexity {
    /// Short query (< 10 words), no complex keywords.
    Simple,
    /// Medium query (10–35 words) or analysis verbs present.
    Moderate,
    /// Long query (> 35 words) or architectural/systemic keywords.
    Complex,
}

/// Task type classification for UCB1 strategy selection.
///
/// Variants are ordered from most-specific to most-general.
/// `as_str()` returns a stable snake_case key for DB persistence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskType {
    /// Write new code, create functions/classes/modules.
    CodeGeneration,
    /// Modify existing code — refactor, rename, update, restructure.
    CodeModification,
    /// Fix bugs, resolve errors, diagnose crashes and panics.
    Debugging,
    /// Investigate, analyse, search, compliance, security audits.
    Research,
    /// File and directory operations.
    FileManagement,
    /// Git operations (commit, status, diff, merge…).
    GitOperation,
    /// Explain concepts, describe behaviour, answer "how does X work".
    Explanation,
    /// Configure tools, manage settings, install dependencies.
    Configuration,
    /// Signals too weak or contradictory to classify confidently.
    General,
}

impl TaskType {
    /// Stable snake_case key for database storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskType::CodeGeneration  => "code_generation",
            TaskType::CodeModification => "code_modification",
            TaskType::Debugging        => "debugging",
            TaskType::Research         => "research",
            TaskType::FileManagement   => "file_management",
            TaskType::GitOperation     => "git_operation",
            TaskType::Explanation      => "explanation",
            TaskType::Configuration    => "configuration",
            TaskType::General          => "general",
        }
    }

    /// Parse from stable snake_case key (DB round-trip).
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "code_generation"   => Some(TaskType::CodeGeneration),
            "code_modification" => Some(TaskType::CodeModification),
            "debugging"         => Some(TaskType::Debugging),
            "research"          => Some(TaskType::Research),
            "file_management"   => Some(TaskType::FileManagement),
            "git_operation"     => Some(TaskType::GitOperation),
            "explanation"       => Some(TaskType::Explanation),
            "configuration"     => Some(TaskType::Configuration),
            "general"           => Some(TaskType::General),
            _ => None,
        }
    }
}

/// Result of task analysis.
#[derive(Debug, Clone)]
pub struct TaskAnalysis {
    pub complexity:  TaskComplexity,
    pub task_type:   TaskType,
    /// Semantic hash for UCB1 experience lookup.
    /// Normalised (stop-word removal + sort) before hashing so that
    /// "fix auth bug" and "fix the authentication bug" land in closer
    /// buckets than raw SHA-256 of the query text would allow.
    pub task_hash:   String,
    pub word_count:  usize,
    /// Score mass captured by the winning type divided by total signal mass.
    /// Range 0.0–1.0.  Values below `CONFIDENCE_FLOOR` produce `General`.
    pub confidence:  f32,
    /// Keywords that fired during classification — useful for debugging
    /// UCB1 reward misattribution and strategy selection reasoning.
    pub signals:     Vec<String>,
}

// ─── Classifier rules ─────────────────────────────────────────────────────────

/// One rule in the scored multi-rule classifier.
///
/// All keywords in the same rule are awarded the same `base_score` per match.
/// A query can match multiple rules for the same `TaskType` (scores accumulate)
/// and multiple rules for *different* types (winner = max score).
struct ClassifierRule {
    task_type:  TaskType,
    /// Score contribution per matched keyword.
    /// Higher = more specific / less ambiguous.
    ///
    /// Tier guide:
    ///   5.0  exact multi-word git/file commands — almost never ambiguous
    ///   3.0  domain nouns (audit, cve, vulnerability, stacktrace…) — context-specific
    ///   2.0  strong intent verbs (fix, implement, explain, configure…)
    ///   1.0  weak / polysemous signals (find, create, check, error…)
    base_score: f32,
    keywords:   &'static [&'static str],
}

/// The full rule table.  Rules for the same `TaskType` accumulate.
/// Order within the slice does NOT affect results — scoring is additive.
static CLASSIFIER_RULES: &[ClassifierRule] = &[
    // ── Tier 5: exact multi-word git commands ─────────────────────────────────
    ClassifierRule {
        task_type:  TaskType::GitOperation,
        base_score: 5.0,
        keywords: &[
            "git commit", "git status", "git diff", "git log",
            "git add", "git push", "git pull", "git fetch",
            "git branch", "git merge", "git rebase", "git stash",
            "git checkout", "git cherry-pick", "git bisect",
            "commit changes", "stage files", "push changes",
            "pull request", "merge request",
        ],
    },
    // ── Tier 5: exact multi-word file operations ──────────────────────────────
    ClassifierRule {
        task_type:  TaskType::FileManagement,
        base_score: 5.0,
        keywords: &[
            "delete file", "remove file", "rename file",
            "move file", "copy file", "create directory",
            "create folder", "list files", "show files",
            "find files", "search files", "delete directory",
            "remove directory", "file permissions",
        ],
    },

    // ── Tier 3: security / compliance / audit domain nouns ───────────────────
    ClassifierRule {
        task_type:  TaskType::Research,
        base_score: 3.0,
        keywords: &[
            // Audit
            "audit", "auditar", "auditoria", "auditoría",
            // Compliance & standards
            "compliance", "cumplimiento",
            "soc2", "soc 2", "sox", "gdpr", "hipaa", "hipaa",
            "iso27001", "iso 27001", "pci-dss", "pci dss",
            "nist", "fips",
            // Vulnerability & offensive security
            "vulnerability", "vulnerabilidad", "vulnerabilities",
            "cve", "cvss", "exploit", "zero-day", "0day",
            "pentest", "pen test", "penetration test", "penetration testing",
            "red team", "blue team",
            // SAST / DAST / scanning
            "sast", "dast", "sonarqube", "sonar", "checkmarx",
            "snyk", "trivy", "grype", "semgrep",
            "attack surface", "threat model", "threat modeling",
            "security assessment", "risk assessment",
        ],
    },

    // ── Tier 3: precise debugging signals ────────────────────────────────────
    ClassifierRule {
        task_type:  TaskType::Debugging,
        base_score: 3.0,
        keywords: &[
            "stacktrace", "stack trace", "traceback",
            "segfault", "segmentation fault",
            "null pointer", "null reference", "nullpointerexception", "npe",
            "deadlock", "race condition", "livelock",
            "memory leak", "memory corruption", "use after free",
            "buffer overflow", "heap corruption",
            "undefined behavior", "ub", "asan",
            "panic at", "thread panicked",
            "core dump", "crash dump",
        ],
    },

    // ── Tier 2: research / analysis verbs (EN + ES) ──────────────────────────
    ClassifierRule {
        task_type:  TaskType::Research,
        base_score: 2.0,
        keywords: &[
            // English
            "analyze", "analyse", "investigate", "compare",
            "examine", "review", "inspect", "survey", "assess",
            "benchmark", "profile", "measure",
            // Spanish verbs (present imperative + infinitive)
            "analiza", "analizar",
            "investiga", "investigar",
            "revisa", "revisar",
            "examina", "examinar",
            "diagnostica", "diagnosticar",
            "evalua", "evaluar",
            "inspecciona", "inspeccionar",
            "compara", "comparar",
        ],
    },

    // ── Tier 2: explanation verbs (EN + ES) ───────────────────────────────────
    ClassifierRule {
        task_type:  TaskType::Explanation,
        base_score: 2.0,
        keywords: &[
            // English
            "explain", "describe", "walk me through", "tell me about",
            "how does", "what is", "what are", "why does", "why is",
            "when should", "can you clarify", "clarify",
            // Spanish
            "explica", "explicar",
            "como funciona", "cómo funciona",
            "que es", "qué es", "que son", "qué son",
            "por que", "por qué",
            "cuando usar", "cuándo usar",
        ],
    },

    // ── Tier 2: code generation — specific intent verbs ──────────────────────
    ClassifierRule {
        task_type:  TaskType::CodeGeneration,
        base_score: 2.0,
        keywords: &[
            "implement", "scaffold", "generate code", "bootstrap",
            "add function", "add method", "add class", "add struct",
            "add feature", "add endpoint", "add route",
            "write a function", "write a class", "write a test",
            "write a script", "write a module",
            "create a function", "create a class", "create a struct",
            "create a module", "create a service", "create an endpoint",
            "build a", "develop a",
        ],
    },

    // ── Tier 2: debugging — intent verbs (EN + ES) ───────────────────────────
    ClassifierRule {
        task_type:  TaskType::Debugging,
        base_score: 2.0,
        keywords: &[
            // English
            "fix", "debug", "diagnose", "troubleshoot", "resolve",
            "not working", "broken", "crash", "why doesn't", "why doesn't",
            "not compiling", "fails to", "throwing",
            // Spanish
            "arregla", "arreglar",
            "corrige", "corregir",
            "depura", "depurar",
            "soluciona", "solucionar",
            "no funciona", "no compila",
        ],
    },

    // ── Tier 2: code modification — intent verbs (EN + ES) ───────────────────
    ClassifierRule {
        task_type:  TaskType::CodeModification,
        base_score: 2.0,
        keywords: &[
            // English
            "modify", "change", "update", "edit", "refactor",
            "rename", "replace", "rewrite", "restructure",
            "extract", "inline", "migrate", "port",
            "optimize", "simplify", "clean up",
            // Spanish
            "modifica", "modificar",
            "cambia", "cambiar",
            "actualiza", "actualizar",
            "refactoriza", "refactorizar",
            "simplifica", "simplificar",
        ],
    },

    // ── Tier 2: configuration — intent verbs (EN + ES) ───────────────────────
    ClassifierRule {
        task_type:  TaskType::Configuration,
        base_score: 2.0,
        keywords: &[
            // English
            "configure", "setup", "set up", "install",
            "initialize", "initialise", "settings", "configuration",
            "enable", "disable", "activate", "deactivate",
            // Spanish
            "configura", "configurar",
            "instala", "instalar",
            "inicializa", "inicializar",
            "habilita", "deshabilita",
            "ajustes",
        ],
    },

    // ── Tier 1: weak / polysemous signals ────────────────────────────────────
    // These contribute to a type but are easily overridden by tier-3 signals.

    ClassifierRule {
        task_type:  TaskType::CodeGeneration,
        base_score: 1.0,
        keywords: &[
            "write", "create", "build", "make", "develop",
            // Spanish
            "escribe", "escribir", "crea", "crear", "construye",
        ],
    },

    ClassifierRule {
        task_type:  TaskType::Debugging,
        base_score: 1.0,
        keywords: &[
            "bug", "error", "issue", "problem", "fails",
            "failure", "wrong", "incorrect", "unexpected",
            // Spanish
            "fallo", "falla", "problema", "erróneo",
        ],
    },

    ClassifierRule {
        task_type:  TaskType::Research,
        base_score: 1.0,
        keywords: &[
            "find", "search", "look up", "lookup", "research",
            "scan", "verify", "validate", "check",
            // Spanish
            "busca", "buscar", "encuentra", "verificar", "validar",
            "comprobar", "revisar",
        ],
    },

    ClassifierRule {
        task_type:  TaskType::FileManagement,
        base_score: 1.0,
        keywords: &[
            "delete", "remove", "move", "copy", "list", "show",
        ],
    },
];

// ─── Stop words for semantic hash normalisation ───────────────────────────────

const STOP_WORDS: &[&str] = &[
    // English
    "a", "an", "the", "this", "that", "these", "those",
    "is", "are", "was", "were", "be", "been", "being",
    "have", "has", "had", "do", "does", "did",
    "will", "would", "shall", "should", "may", "might", "must", "can", "could",
    "to", "of", "in", "for", "on", "with", "at", "by", "from", "as", "into",
    "through", "during", "it", "its", "i", "me", "my", "you", "your",
    "we", "our", "they", "their", "and", "or", "but", "if", "because",
    "while", "after", "before", "so", "not", "no", "nor",
    "all", "some", "any", "each", "every", "both", "few",
    "more", "most", "other", "such", "only", "own", "same",
    "than", "too", "very", "just", "now", "then", "here", "there",
    "when", "where", "who", "which", "how", "what", "why",
    // Spanish
    "el", "la", "los", "las", "un", "una", "unos", "unas",
    "de", "del", "al", "en", "con", "por", "para", "que",
    "es", "son", "fue", "era", "ser", "estar",
    "mi", "tu", "su", "sus", "nos", "vos",
    "se", "me", "te", "le", "les",
    "y", "e", "o", "u", "pero", "si", "no", "mas", "más", "muy",
    "este", "esta", "esto", "ese", "esa", "eso",
    "hay", "bien", "mal", "ya", "aún", "ahora",
];

// ─── Complexity keywords ──────────────────────────────────────────────────────

/// Analysis verbs that upgrade short queries from Simple → Moderate.
/// A 3-word query like "analiza mi proyecto" is never truly Simple —
/// it implies multi-file scanning across many agent rounds.
const ANALYSIS_VERBS: &[&str] = &[
    "analiza",    "analizar",
    "revisa",     "revisar",
    "examina",    "examinar",
    "investiga",  "investigar",
    "inspecciona","inspeccionar",
    "diagnostica","diagnosticar",
    "evalua",     "evaluar",
    "assess",     "investigate",
    "examine",    "review",
];

/// Keywords that force Complex regardless of word count.
const COMPLEX_KEYWORDS: &[&str] = &[
    "refactor", "optimize", "optimise", "migrate",
    "integrate", "architecture", "design pattern",
    "performance", "scale", "scalability",
    "distributed", "microservice", "microservices",
    "concurrent", "parallelism", "zero-downtime",
    "backwards compatible", "breaking change",
    // Spanish
    "arquitectura", "escalabilidad",
    "distribuido", "refactorizar",
];

// ─── TaskAnalyzer ─────────────────────────────────────────────────────────────

/// Classifies user queries by complexity, type, and confidence.
pub struct TaskAnalyzer;

impl TaskAnalyzer {
    /// Analyse a user query and return a full classification.
    pub fn analyze(query: &str) -> TaskAnalysis {
        let word_count = query.split_whitespace().count();
        let complexity = Self::classify_complexity(query, word_count);
        let (task_type, confidence, signals) = Self::classify_type_scored(query);
        let task_hash = Self::compute_semantic_hash(query);

        TaskAnalysis {
            complexity,
            task_type,
            task_hash,
            word_count,
            confidence,
            signals,
        }
    }

    // ── Complexity ────────────────────────────────────────────────────────────

    fn classify_complexity(query: &str, word_count: usize) -> TaskComplexity {
        let lower = query.to_lowercase();

        // Architectural / systemic keywords → always Complex.
        if COMPLEX_KEYWORDS.iter().any(|kw| lower.contains(kw)) {
            return TaskComplexity::Complex;
        }

        // Analysis verbs on short queries → at least Moderate.
        // Word boundary aware to avoid "reanalizando" → Moderate.
        if word_count < 10 {
            if ANALYSIS_VERBS.iter().any(|v| Self::contains_word_safe(&lower, v)) {
                return TaskComplexity::Moderate;
            }
        }

        match word_count {
            0..=9   => TaskComplexity::Simple,
            10..=35 => TaskComplexity::Moderate,
            _       => TaskComplexity::Complex,
        }
    }

    // ── Type (scored) ─────────────────────────────────────────────────────────

    /// Returns `(TaskType, confidence, matched_signals)`.
    fn classify_type_scored(query: &str) -> (TaskType, f32, Vec<String>) {
        let lower = query.to_lowercase();

        // Per-type score accumulator.  Using parallel arrays keyed by
        // `TaskType::as_str()` would require alloc; a fixed-size array is simpler.
        let mut scores = [0f32; 9]; // one slot per TaskType variant
        let mut signals: Vec<String> = Vec::new();

        let type_index = |t: TaskType| match t {
            TaskType::CodeGeneration   => 0,
            TaskType::CodeModification => 1,
            TaskType::Debugging        => 2,
            TaskType::Research         => 3,
            TaskType::FileManagement   => 4,
            TaskType::GitOperation     => 5,
            TaskType::Explanation      => 6,
            TaskType::Configuration    => 7,
            TaskType::General          => 8,
        };

        let type_from_index = |i: usize| match i {
            0 => TaskType::CodeGeneration,
            1 => TaskType::CodeModification,
            2 => TaskType::Debugging,
            3 => TaskType::Research,
            4 => TaskType::FileManagement,
            5 => TaskType::GitOperation,
            6 => TaskType::Explanation,
            7 => TaskType::Configuration,
            _ => TaskType::General,
        };

        for rule in CLASSIFIER_RULES {
            for kw in rule.keywords {
                let matched = if kw.contains(' ') {
                    lower.contains(*kw)
                } else {
                    Self::contains_word_safe(&lower, kw)
                };
                if matched {
                    scores[type_index(rule.task_type)] += rule.base_score;
                    signals.push((*kw).to_string());
                }
            }
        }

        let total: f32 = scores.iter().sum();

        if total == 0.0 {
            return (TaskType::General, 0.0, signals);
        }

        let (winner_idx, &winner_score) = scores
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
            .unwrap();

        let confidence = winner_score / total;

        let task_type = if confidence >= CONFIDENCE_FLOOR {
            type_from_index(winner_idx)
        } else {
            TaskType::General
        };

        (task_type, confidence, signals)
    }

    // ── Semantic hash ─────────────────────────────────────────────────────────

    /// SHA-256 of the semantically normalised query.
    ///
    /// Normalisation pipeline:
    /// 1. Lowercase + trim
    /// 2. Replace non-alphanumeric chars with spaces
    /// 3. Split into words, remove stop words
    /// 4. Sort alphabetically (order-independent)
    /// 5. Deduplicate
    /// 6. Join → SHA-256
    ///
    /// This groups "fix the authentication bug" and "fix auth bug" into
    /// much closer buckets than SHA-256 of the raw query text, reducing
    /// UCB1 cold-starts for paraphrased repetitions of the same task.
    fn compute_semantic_hash(query: &str) -> String {
        let cleaned: String = query
            .trim()
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() || c.is_whitespace() { c } else { ' ' })
            .collect();

        let mut words: Vec<String> = cleaned
            .split_whitespace()
            .filter(|w| !STOP_WORDS.contains(w))
            .map(|w| w.to_string())
            .collect();
        words.sort_unstable();
        words.dedup();

        let normalised = words.join(" ");

        let mut hasher = Sha256::new();
        hasher.update(normalised.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    // ── Word-boundary matching ────────────────────────────────────────────────

    /// Returns `true` when `text` contains `word` at a proper word boundary.
    ///
    /// A word boundary exists where an alphanumeric char (or `_`) is
    /// preceded / followed by a non-alphanumeric, non-underscore char,
    /// or the start / end of the string.
    ///
    /// ## UTF-8 safety
    ///
    /// The legacy implementation used `bytes[pos - 1]` with `start = pos + 1`
    /// which could land on a non-char-boundary with multibyte characters
    /// (STAT-PANIC-006).  This version only uses `str` slice operations and
    /// `chars()` which are guaranteed to produce valid char boundaries.
    ///
    /// All keywords in `CLASSIFIER_RULES` are ASCII, so `pos + word.len()`
    /// is always a valid char boundary in `text`.
    fn contains_word_safe(text: &str, word: &str) -> bool {
        let wlen = word.len();
        let tlen = text.len();
        if wlen > tlen {
            return false;
        }

        let mut search_from = 0usize;
        while search_from + wlen <= tlen {
            match text[search_from..].find(word) {
                None => break,
                Some(rel) => {
                    let pos = search_from + rel;

                    // Boundary before the match: last char of text[..pos].
                    // Using .chars().next_back() is char-boundary safe.
                    let before_ok = pos == 0 || {
                        let bc = text[..pos].chars().next_back().unwrap_or(' ');
                        // Explicit parentheses to document intended precedence (STAT-LOGIC-001 fix).
                        (!bc.is_alphanumeric()) && (bc != '_')
                    };

                    // Boundary after the match: first char of text[after_pos..].
                    // `pos + wlen` is a valid char boundary because `word` is ASCII.
                    let after_pos = pos + wlen;
                    let after_ok = after_pos >= tlen || {
                        let ac = text[after_pos..].chars().next().unwrap_or(' ');
                        (!ac.is_alphanumeric()) && (ac != '_')
                    };

                    if before_ok && after_ok {
                        return true;
                    }

                    // Advance by exactly one character (UTF-8 safe).
                    let step = text[pos..].chars().next().map_or(1, |c| c.len_utf8());
                    search_from = pos + step;
                }
            }
        }
        false
    }
}

// ─── P4-2: Classifier trait interface ─────────────────────────────────────────
//
// Stable abstraction for the intent classification backend.
// Current production path uses `IntentScorer` (multi-dimensional, wired in reasoning_engine.rs).
// This trait + `KeywordClassifier` exist to:
//   1. Expose `TaskAnalyzer::analyze()` via a standard interface for tooling/tests.
//   2. Provide a clean migration point when an LLM-based backend is added.
//
// NOTE: `KeywordClassifier::classify()` is NOT currently called in the production
// agent loop — `IntentScorer::score()` is the live path.  These types will become
// the main path once the scorer and analyzer are unified (tracked separately).

/// Result of classifying a natural-language query.
///
/// Used by `IntentClassifier` implementations as a stable return type.
/// In production, the equivalent data comes from `IntentProfile` (via `IntentScorer`).
#[derive(Debug, Clone)]
#[allow(dead_code)] // Used by KeywordClassifier and future LLM backends
pub struct ClassificationResult {
    pub task_type: TaskType,
    pub confidence: f32,
    pub complexity: TaskComplexity,
    pub task_hash: String,
    pub word_count: usize,
    pub method: ClassificationMethod,
}

/// Which backend produced the classification.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // Variants used by KeywordClassifier and reserved for future LLM backends
pub enum ClassificationMethod {
    /// SOTA 2026 Scored Multi-Rule Classifier — current SMRC keyword implementation.
    KeywordSMRC,
    /// Reserved — future LLM-based one-shot classification.
    LlmOneShot,
    /// Reserved — LLM with SMRC keyword fallback when confidence < `CONFIDENCE_FLOOR`.
    LlmWithKeywordFallback { llm_confidence: u8 },
}

/// Stable call surface for intent classification backends.
///
/// Callers depend on this trait, not on concrete types, enabling zero-callsite
/// migration between backends (keyword → LLM → hybrid).
#[allow(dead_code)] // Implemented by KeywordClassifier; will be consumed by orchestrator
pub trait IntentClassifier {
    fn classify(&self, query: &str) -> ClassificationResult;
}

/// Keyword-based SMRC classifier — wraps `TaskAnalyzer::analyze()`.
///
/// This is NOT the production agent path (which uses `IntentScorer`).
/// Use this for:
/// - CLI tooling: `halcon classify "my query"`
/// - Integration tests that need SMRC-specific signals
/// - Benchmarking SMRC vs IntentScorer agreement rates
#[allow(dead_code)] // Will be wired when scorer/analyzer unification is complete
pub struct KeywordClassifier;

impl IntentClassifier for KeywordClassifier {
    fn classify(&self, query: &str) -> ClassificationResult {
        let analysis = TaskAnalyzer::analyze(query);
        ClassificationResult {
            task_type: analysis.task_type,
            confidence: analysis.confidence,
            complexity: analysis.complexity,
            task_hash: analysis.task_hash,
            word_count: analysis.word_count,
            method: ClassificationMethod::KeywordSMRC,
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Complexity ────────────────────────────────────────────────────────────

    #[test]
    fn complexity_simple_short_query() {
        let analysis = TaskAnalyzer::analyze("list files");
        assert_eq!(analysis.complexity, TaskComplexity::Simple);
        assert_eq!(analysis.word_count, 2);
    }

    #[test]
    fn complexity_moderate_medium_query() {
        let analysis = TaskAnalyzer::analyze(
            "create a new function that takes a string and returns uppercase",
        );
        assert_eq!(analysis.complexity, TaskComplexity::Moderate);
        assert!(analysis.word_count >= 10 && analysis.word_count <= 35);
    }

    #[test]
    fn complexity_complex_long_query() {
        let long = "write a function that reads data from a database, processes it through \
            multiple transformations, validates the output against a schema, handles errors \
            gracefully, logs all operations, and returns a structured response with metadata \
            including timestamps and processing stats";
        let analysis = TaskAnalyzer::analyze(long);
        assert_eq!(analysis.complexity, TaskComplexity::Complex);
        assert!(analysis.word_count > 35);
    }

    #[test]
    fn complexity_complex_keyword_override() {
        // Only 3 words but "refactor" forces Complex.
        let analysis = TaskAnalyzer::analyze("refactor this code");
        assert_eq!(analysis.complexity, TaskComplexity::Complex);
    }

    // ── Task type — core cases ────────────────────────────────────────────────

    #[test]
    fn type_code_generation() {
        let analysis = TaskAnalyzer::analyze("write a new function to parse JSON");
        assert_eq!(analysis.task_type, TaskType::CodeGeneration);
    }

    #[test]
    fn type_debugging() {
        let analysis = TaskAnalyzer::analyze("fix the error in authentication");
        assert_eq!(analysis.task_type, TaskType::Debugging);
    }

    #[test]
    fn type_code_modification() {
        let analysis = TaskAnalyzer::analyze("modify the login function to use async");
        assert_eq!(analysis.task_type, TaskType::CodeModification);
    }

    #[test]
    fn type_file_management() {
        let analysis = TaskAnalyzer::analyze("delete file temp.txt");
        assert_eq!(analysis.task_type, TaskType::FileManagement);
    }

    #[test]
    fn type_git_operation() {
        let analysis = TaskAnalyzer::analyze("git commit with message 'fix bug'");
        assert_eq!(analysis.task_type, TaskType::GitOperation);
    }

    #[test]
    fn type_research() {
        let analysis = TaskAnalyzer::analyze("find all uses of this function");
        assert_eq!(analysis.task_type, TaskType::Research);
    }

    #[test]
    fn type_explanation() {
        let analysis = TaskAnalyzer::analyze("explain how async/await works");
        assert_eq!(analysis.task_type, TaskType::Explanation);
    }

    #[test]
    fn type_configuration() {
        let analysis = TaskAnalyzer::analyze("configure the database connection");
        assert_eq!(analysis.task_type, TaskType::Configuration);
    }

    #[test]
    fn type_general_fallback() {
        let analysis = TaskAnalyzer::analyze("hello there");
        assert_eq!(analysis.task_type, TaskType::General);
    }

    // ── SMRC: score-based wins (legacy first-match would fail these) ──────────

    #[test]
    fn smrc_create_audit_report_is_research_not_code_gen() {
        // "create" → CodeGeneration(1.0), "audit" → Research(3.0)
        // Research wins because domain noun outscores generic verb.
        let analysis = TaskAnalyzer::analyze("create a security audit report");
        assert_eq!(
            analysis.task_type,
            TaskType::Research,
            "SMRC: Research(audit×3) must beat CodeGeneration(create×1), confidence={}",
            analysis.confidence
        );
    }

    #[test]
    fn smrc_fix_vulnerability_is_research_not_debugging() {
        // "fix" → Debugging(2.0), "vulnerability" → Research(3.0)
        // Research wins: vulnerability analysis requires investigation, not just a patch.
        let analysis = TaskAnalyzer::analyze("fix the vulnerability in the auth module");
        assert_eq!(
            analysis.task_type,
            TaskType::Research,
            "SMRC: Research(vulnerability×3) must beat Debugging(fix×2)"
        );
    }

    #[test]
    fn smrc_git_commit_beats_fix_bug() {
        // "git commit" → GitOperation(5.0), "fix"→Debugging(2.0), "bug"→Debugging(1.0)
        // GitOperation wins (5.0 vs 3.0).
        let analysis = TaskAnalyzer::analyze("git commit all changes to fix the bug");
        assert_eq!(analysis.task_type, TaskType::GitOperation);
    }

    #[test]
    fn smrc_verify_soc2_compliance_is_research() {
        // "soc2"=3.0 + "compliance"=3.0 + "verify"=1.0 → Research(7.0)
        let analysis = TaskAnalyzer::analyze("verify SOC2 compliance controls are passing");
        assert_eq!(analysis.task_type, TaskType::Research);
    }

    #[test]
    fn smrc_confidence_exposed() {
        let clear = TaskAnalyzer::analyze("git status");
        // Only one signal "git status" → confidence = 5.0/5.0 = 1.0
        assert!(
            clear.confidence > 0.9,
            "High-signal query must have confidence > 0.9, got {}",
            clear.confidence
        );

        let general = TaskAnalyzer::analyze("hello there");
        assert_eq!(general.confidence, 0.0);
    }

    #[test]
    fn smrc_signals_populated_on_match() {
        let analysis = TaskAnalyzer::analyze("audit the database access logs");
        assert!(
            !analysis.signals.is_empty(),
            "signals must be populated when keywords match"
        );
        assert!(
            analysis.signals.iter().any(|s| s.contains("audit")),
            "signals must include 'audit'"
        );
    }

    // ── Word boundary: no false positives ────────────────────────────────────

    #[test]
    fn word_boundary_fix_not_in_prefix() {
        // "prefix" contains "fix" but word-boundary check must reject it.
        let analysis = TaskAnalyzer::analyze("prefix the function name");
        assert_ne!(analysis.task_type, TaskType::Debugging);
    }

    #[test]
    fn word_boundary_write_not_in_rewrite() {
        // "rewrite" contains "write" but word-boundary check must reject it.
        // "rewrite" does match CodeModification → that type is expected.
        let analysis = TaskAnalyzer::analyze("rewrite this module");
        assert_ne!(analysis.task_type, TaskType::CodeGeneration);
    }

    #[test]
    fn word_boundary_analiza_not_in_reanalizando() {
        // "reanalizando" — embedded "analiza" must not match at word boundary.
        // query is 3 words and no analysis verb fires → Simple complexity.
        let analysis = TaskAnalyzer::analyze("reanalizando el proceso");
        assert_eq!(analysis.complexity, TaskComplexity::Simple);
    }

    #[test]
    fn word_boundary_fix_whole_word() {
        assert!(TaskAnalyzer::contains_word_safe("fix the bug", "fix"));
    }

    #[test]
    fn word_boundary_fix_at_end() {
        assert!(TaskAnalyzer::contains_word_safe("please fix", "fix"));
    }

    #[test]
    fn word_boundary_fix_with_punctuation() {
        assert!(TaskAnalyzer::contains_word_safe("can you fix? yes", "fix"));
    }

    #[test]
    fn word_boundary_rejects_embedded() {
        assert!(!TaskAnalyzer::contains_word_safe("prefix this code", "fix"));
        assert!(!TaskAnalyzer::contains_word_safe("rewrite the function", "write"));
    }

    /// STAT-PANIC-006 regression: multibyte characters around the match must not panic.
    #[test]
    fn word_boundary_utf8_safe_with_accented_chars() {
        // "diagnosticar" contains "diagnos" prefix — must not panic.
        // "analiza" appears inside "reanalizó" — must not match.
        assert!(!TaskAnalyzer::contains_word_safe("reanalizó el proceso", "analiza"));
        // "fix" after an accented word boundary.
        assert!(TaskAnalyzer::contains_word_safe("¿puedes fix esto?", "fix"));
        // No panic on emoji (multibyte).
        let _ = TaskAnalyzer::contains_word_safe("🔥 fix the bug 🔥", "fix");
    }

    // ── Semantic hash ─────────────────────────────────────────────────────────

    #[test]
    fn hash_is_consistent() {
        let h1 = TaskAnalyzer::compute_semantic_hash("test query");
        let h2 = TaskAnalyzer::compute_semantic_hash("test query");
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_is_case_insensitive() {
        assert_eq!(
            TaskAnalyzer::compute_semantic_hash("Test Query"),
            TaskAnalyzer::compute_semantic_hash("test query"),
        );
    }

    #[test]
    fn hash_trims_whitespace() {
        assert_eq!(
            TaskAnalyzer::compute_semantic_hash("  test query  "),
            TaskAnalyzer::compute_semantic_hash("test query"),
        );
    }

    #[test]
    fn hash_is_sha256_hex() {
        let hash = TaskAnalyzer::compute_semantic_hash("test");
        assert_eq!(hash.len(), 64); // SHA-256 = 32 bytes = 64 hex chars
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn hash_order_independent() {
        // Semantic hash sorts words — order should not affect the result for
        // queries with identical content words.
        let h1 = TaskAnalyzer::compute_semantic_hash("fix authentication bug");
        let h2 = TaskAnalyzer::compute_semantic_hash("bug authentication fix");
        assert_eq!(h1, h2, "semantic hash must be order-independent");
    }

    #[test]
    fn hash_stop_word_removal() {
        // Adding stop words should not change the hash.
        let h1 = TaskAnalyzer::compute_semantic_hash("fix authentication bug");
        let h2 = TaskAnalyzer::compute_semantic_hash("fix the authentication bug");
        assert_eq!(h1, h2, "stop words must not affect the semantic hash");
    }

    // ── TaskType round-trip ───────────────────────────────────────────────────

    #[test]
    fn task_type_roundtrip() {
        let types = [
            TaskType::CodeGeneration,
            TaskType::CodeModification,
            TaskType::Debugging,
            TaskType::Research,
            TaskType::FileManagement,
            TaskType::GitOperation,
            TaskType::Explanation,
            TaskType::Configuration,
            TaskType::General,
        ];
        for ty in &types {
            let s = ty.as_str();
            let parsed = TaskType::from_str(s).unwrap();
            assert_eq!(*ty, parsed, "round-trip failed for {:?}", ty);
        }
    }

    #[test]
    fn task_type_from_str_invalid() {
        assert_eq!(TaskType::from_str("invalid"), None);
    }

    // ── Spanish keyword tests ─────────────────────────────────────────────────

    #[test]
    fn spanish_analiza_classified_as_research() {
        let analysis = TaskAnalyzer::analyze("analiza mi proyecto");
        assert_eq!(analysis.task_type, TaskType::Research);
    }

    #[test]
    fn spanish_revisa_classified_as_research() {
        let analysis = TaskAnalyzer::analyze("revisa el estado del proyecto");
        assert_eq!(analysis.task_type, TaskType::Research);
    }

    #[test]
    fn spanish_investiga_classified_as_research() {
        let analysis = TaskAnalyzer::analyze("investiga el proyecto");
        assert_eq!(analysis.task_type, TaskType::Research);
    }

    #[test]
    fn spanish_examina_classified_as_research() {
        let analysis = TaskAnalyzer::analyze("examina el codebase completo");
        assert_eq!(analysis.task_type, TaskType::Research);
    }

    #[test]
    fn spanish_analiza_short_query_is_moderate() {
        let analysis = TaskAnalyzer::analyze("analiza mi proyecto");
        assert_eq!(analysis.complexity, TaskComplexity::Moderate);
        assert!(analysis.word_count < 10);
    }

    #[test]
    fn spanish_analizar_infinitive_is_moderate() {
        let analysis = TaskAnalyzer::analyze("analizar el codigo");
        assert_eq!(analysis.complexity, TaskComplexity::Moderate);
    }

    #[test]
    fn spanish_revisa_short_is_moderate() {
        let analysis = TaskAnalyzer::analyze("revisa el estado");
        assert_eq!(analysis.complexity, TaskComplexity::Moderate);
        assert!(analysis.word_count < 10);
    }

    #[test]
    fn spanish_diagnostica_is_moderate() {
        let analysis = TaskAnalyzer::analyze("diagnostica el sistema");
        assert_eq!(analysis.complexity, TaskComplexity::Moderate);
    }

    #[test]
    fn spanish_evalua_is_moderate() {
        let analysis = TaskAnalyzer::analyze("evalua el rendimiento");
        assert_eq!(analysis.complexity, TaskComplexity::Moderate);
    }

    #[test]
    fn spanish_explica_classified_as_explanation() {
        let analysis = TaskAnalyzer::analyze("explica como funciona esto");
        assert_eq!(analysis.task_type, TaskType::Explanation);
    }

    #[test]
    fn spanish_user_query_project_analysis_is_research_moderate() {
        let analysis = TaskAnalyzer::analyze("analiza mi proyecto actual y el estado");
        assert_eq!(analysis.task_type, TaskType::Research);
        assert_eq!(analysis.complexity, TaskComplexity::Moderate);
    }

    #[test]
    fn non_spanish_short_query_still_simple() {
        let analysis = TaskAnalyzer::analyze("list files");
        assert_eq!(analysis.complexity, TaskComplexity::Simple);
    }

    // ── P1-C: Audit / compliance keyword tests ────────────────────────────────

    #[test]
    fn p1c_audit_keyword_is_research() {
        let analysis = TaskAnalyzer::analyze("audit the database access logs");
        assert_eq!(analysis.task_type, TaskType::Research, "confidence={}", analysis.confidence);
    }

    #[test]
    fn p1c_auditar_spanish_is_research() {
        let analysis = TaskAnalyzer::analyze("auditar los permisos del sistema");
        assert_eq!(analysis.task_type, TaskType::Research);
    }

    #[test]
    fn p1c_compliance_keyword_is_research() {
        let analysis = TaskAnalyzer::analyze("check SOC2 compliance for the API");
        assert_eq!(analysis.task_type, TaskType::Research);
    }

    #[test]
    fn p1c_vulnerability_keyword_is_research() {
        let analysis = TaskAnalyzer::analyze("scan for vulnerability in dependencies");
        assert_eq!(analysis.task_type, TaskType::Research);
    }

    #[test]
    fn p1c_pentest_keyword_is_research() {
        let analysis = TaskAnalyzer::analyze("pentest the authentication endpoint");
        assert_eq!(analysis.task_type, TaskType::Research);
    }

    #[test]
    fn p1c_assessment_keyword_is_research() {
        let analysis = TaskAnalyzer::analyze("security assessment of the codebase");
        assert_eq!(analysis.task_type, TaskType::Research);
    }

    #[test]
    fn p1c_soc2_keyword_is_research() {
        let analysis = TaskAnalyzer::analyze("verify SOC2 controls are passing");
        assert_eq!(analysis.task_type, TaskType::Research);
    }

    #[test]
    fn p1c_auditoria_spanish_is_research() {
        let analysis = TaskAnalyzer::analyze("realiza una auditoria de seguridad");
        assert_eq!(analysis.task_type, TaskType::Research);
    }

    // ── Debugging: direct keyword ─────────────────────────────────────────────

    #[test]
    fn type_debugging_direct_fix_keyword() {
        let analysis = TaskAnalyzer::analyze("fix this bug in the function");
        assert_eq!(analysis.task_type, TaskType::Debugging);
    }
}
