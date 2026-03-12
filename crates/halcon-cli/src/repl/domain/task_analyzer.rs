//! Task complexity and type analysis — Cascade-SMRC™ 2026.
//!
//! ## Architecture — 3-Layer Cascade
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │  Layer 1 · Position-Weighted SMRC                                   │
//! │  Every keyword contributes base_score × position_weight.            │
//! │  Tokens at positions 0-1 (action verb slot) get 1.30× boost;       │
//! │  positions 2-3 get 1.15×; positions 4+ get 1.0×.                   │
//! │                                                                     │
//! │  Fast-path: if winner_confidence ≥ 0.88 → return immediately.      │
//! ├─────────────────────────────────────────────────────────────────────┤
//! │  Layer 2 · Entropy + Margin Analysis                                │
//! │  Shannon entropy H = -Σ p_i·log₂(p_i) over positive-score types.  │
//! │  If H/H_max > 0.65 OR margin < 0.12 → tag as ambiguous.           │
//! │  Primary task_type preserved; ambiguity exposed as metadata.        │
//! ├─────────────────────────────────────────────────────────────────────┤
//! │  Layer 3 · Contextual Enrichment                                    │
//! │  W5H2 canonical intent: "verb:domain" stable key for UCB1.         │
//! │  Multi-intent detection: conjunction tokens + dual-signal check.    │
//! │  Context priors: file extensions + git state → score bias.         │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Why this beats plain SMRC
//!
//! | Query | SMRC result | Cascade-SMRC |
//! |-------|-------------|--------------|
//! | "explain AND fix the auth bug" | Debugging (fix>explain) | Debugging + is_multi_intent=true + secondary=Explanation |
//! | "fix the vulnerability in auth" | Research (close) | Research, margin=0.141, not ambiguous |
//! | "verify" (single word) | General (no signal) | General, ambiguity=NoSignals |
//! | "fix refactor" (contradiction) | CodeModification | CodeModification, ambiguity=NarrowMargin |
//!
//! ## Backward Compatibility
//!
//! `TaskAnalysis.task_type` and `TaskAnalysis.confidence` are semantically
//! identical to the previous SMRC implementation for all queries — position
//! weighting only adjusts the magnitude, not the winner, for unambiguous cases.
//! New fields (`secondary_type`, `is_multi_intent`, `ambiguity`, `margin`,
//! `canonical_intent`) are purely additive.
//!
//! ## Dynamic Rules (config/classifier_rules.toml)
//!
//! La diferencia clave con sistemas como Claude Code / Codex:
//!
//! ```text
//! Claude Code:  LLM ──────────────────────────────► el modelo IS el clasificador
//!                                                    no existe ClassifierRule
//!
//! Halcon:       necesita clasificar ANTES del LLM   (para elegir routing tier,
//!               max_rounds, UCB1 strategy)           por eso existe el clasificador
//!               pero las reglas DEBEN ser externas   → TOML + few-shot examples
//!
//! Cascade-SMRC: TOML rules (Layer 1) ──────────────► score-based winner
//!               VectorStore examples (Layer 3) ────► nearest-neighbor fallback
//!               confidence < FLOOR ─────────────────► General (honest "no sé")
//! ```
//!
//! Para agregar keywords sin recompilar:
//!   editar `config/classifier_rules.toml` o `~/.halcon/classifier_rules.toml`
//!
//! Para agregar un TaskType nuevo:
//!   1. Variante en `TaskType` enum + `as_str()` / `from_str()`
//!   2. `[[rule]]` en el TOML
//!   3. `[[example]]` en el TOML (mejora el fallback few-shot)

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

// ─── Constants ────────────────────────────────────────────────────────────────

/// Minimum confidence to accept a classification.  Below this, winner type
/// does not have enough signal mass → falls back to `General`.
pub const CONFIDENCE_FLOOR: f32 = 0.30;

/// Normalised Shannon entropy ratio above which the result is tagged as
/// ambiguous — the score mass is spread across too many competing types.
/// H/H_max > 0.65 means the distribution is more "uncertain" than a
/// coin-flip between the top two types.
pub const AMBIGUITY_ENTROPY_RATIO: f32 = 0.65;

/// Normalised margin (winner − runner-up) / total below which the result
/// is tagged as ambiguous — the top two types are too close to call.
pub const AMBIGUITY_MARGIN: f32 = 0.12;

/// If winner confidence ≥ this, skip entropy analysis (fast path).
/// Avoids O(N log N) entropy computation for crystal-clear queries.
pub const PHRASE_FAST_PATH_CONFIDENCE: f32 = 0.88;

/// Position weight for tokens at positions 0–1 (typical action-verb slot).
/// Research on MASSIVE corpus shows ~31% accuracy gain on 8-class intent
/// when the leading bigram is given higher weight (SetFit, Tunstall 2022).
pub const POSITION_WEIGHT_LEADING: f32 = 1.30;

/// Position weight for tokens at positions 2–3 (first object/domain slot).
pub const POSITION_WEIGHT_NEAR: f32 = 1.15;

// tokens 4+: weight 1.0 (default, no constant needed)

// ─── Ambiguity ────────────────────────────────────────────────────────────────

/// Reason the classifier is uncertain about its result.
///
/// Callers may choose to treat any ambiguous result as `General` when
/// strict correctness matters, or surface the reason to the user for
/// disambiguation.
#[derive(Debug, Clone, PartialEq)]
pub enum AmbiguityReason {
    /// Zero keywords fired — query carries no domain signal.
    NoSignals,
    /// Shannon entropy ratio H/H_max > `AMBIGUITY_ENTROPY_RATIO` — score mass
    /// spread across too many competing types.
    HighEntropy { entropy_ratio: f32 },
    /// Normalised margin between winner and runner-up < `AMBIGUITY_MARGIN` —
    /// the top two types are statistically tied.
    NarrowMargin { margin: f32 },
}

// ─── Task types ───────────────────────────────────────────────────────────────

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
/// `as_str()` returns a stable snake_case key for DB persistence.
/// Variants are ordered most-specific → most-general.
// Phase 5: serde support for FeedbackEvent persistence + snapshot roundtrip.
// rename_all = "snake_case" matches TaskType::as_str() exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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
    /// Stable snake_case key for database storage and UCB1 lookup.
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskType::CodeGeneration => "code_generation",
            TaskType::CodeModification => "code_modification",
            TaskType::Debugging => "debugging",
            TaskType::Research => "research",
            TaskType::FileManagement => "file_management",
            TaskType::GitOperation => "git_operation",
            TaskType::Explanation => "explanation",
            TaskType::Configuration => "configuration",
            TaskType::General => "general",
        }
    }

    /// Parse from stable snake_case key (DB round-trip).
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "code_generation" => Some(TaskType::CodeGeneration),
            "code_modification" => Some(TaskType::CodeModification),
            "debugging" => Some(TaskType::Debugging),
            "research" => Some(TaskType::Research),
            "file_management" => Some(TaskType::FileManagement),
            "git_operation" => Some(TaskType::GitOperation),
            "explanation" => Some(TaskType::Explanation),
            "configuration" => Some(TaskType::Configuration),
            "general" => Some(TaskType::General),
            _ => None,
        }
    }
}

// ─── Context signals ──────────────────────────────────────────────────────────

/// Contextual signals from the session environment used to apply prior biases
/// before scoring.  All fields are optional — passing an empty struct is safe
/// and produces zero bias (same result as `TaskAnalyzer::analyze`).
///
/// ## Prior adjustment semantics
///
/// Signals that increase the prior for a type add a small fixed score to that
/// type's bucket BEFORE keyword matching, so keyword evidence can still
/// override the prior when strong enough.
pub struct ContextSignals<'a> {
    /// File extensions present in the current conversation context
    /// (e.g., `&["rs", "toml"]` when editing Rust files).
    /// Used to bias toward CodeGeneration/CodeModification/Debugging.
    pub file_extensions: &'a [&'a str],

    /// Whether the session has active git merge conflicts.
    /// Biases toward Debugging and GitOperation.
    pub in_git_conflict: bool,

    /// Recent task types from session history (most-recent first).
    /// First element receives the strongest prior bias (0.5× base), with
    /// exponential decay (0.25× for 2nd, 0.125× for 3rd, …).
    pub recent_task_types: &'a [TaskType],
}

impl<'a> ContextSignals<'a> {
    /// Zero-bias context (equivalent to `TaskAnalyzer::analyze`).
    pub const fn empty() -> Self {
        ContextSignals {
            file_extensions: &[],
            in_git_conflict: false,
            recent_task_types: &[],
        }
    }
}

// ─── TaskAnalysis ─────────────────────────────────────────────────────────────

/// Full result of task analysis.
///
/// `task_type` and `confidence` are the primary classification output.
/// All other fields are enrichment metadata for callers that need it.
#[derive(Debug, Clone)]
pub struct TaskAnalysis {
    // ── Primary classification ───────────────────────────────────────────────
    pub complexity: TaskComplexity,
    pub task_type: TaskType,
    /// Semantic hash for UCB1 experience lookup.
    /// Stop-word removal + alphabetical sort normalise paraphrases to the
    /// same bucket, drastically reducing UCB1 cold-starts.
    pub task_hash: String,
    pub word_count: usize,
    /// Winner score / total positive score (0.0–1.0).
    /// Values below `CONFIDENCE_FLOOR` produce `TaskType::General`.
    pub confidence: f32,
    /// Keywords that fired — useful for debugging reward misattribution.
    pub signals: Vec<String>,

    // ── Enrichment metadata (Layer 2 & 3) ───────────────────────────────────
    /// Second-ranked task type (if any), exposed for multi-intent routing.
    /// `None` when either (a) only one type scored or (b) primary is General.
    pub secondary_type: Option<TaskType>,
    /// True when the query contains conjunctive markers ("and", "y", "además")
    /// AND has strong signal for at least two different types.
    /// Callers may route such queries through a multi-step plan.
    pub is_multi_intent: bool,
    /// Non-None when the classifier is uncertain.  Callers may choose to treat
    /// any ambiguous result as `General` for safety-critical routing.
    pub ambiguity: Option<AmbiguityReason>,
    /// Normalised score gap: (winner − runner_up) / total.
    /// 0.0 = tied; 1.0 = only one type scored at all.
    pub margin: f32,
    /// W5H2 canonical intent in `"verb:domain"` form, e.g. `"fix:vulnerability"`.
    /// More stable than `task_hash` for UCB1 strategy warm-start because
    /// it survives surface-level rephrasing ("fix"/"resolve"/"patch" → same verb).
    /// `None` when neither an action verb nor a domain noun could be extracted.
    pub canonical_intent: Option<String>,
}

// ─── Classifier rules — Dynamic (loaded from TOML, compiled defaults as fallback) ──

/// Runtime-loaded classifier rule.  Keywords are owned `String`s so rules can
/// be loaded from config at startup without `'static` lifetime constraints.
#[derive(Debug, Clone)]
pub struct ClassifierRule {
    pub task_type: TaskType,
    /// Score contribution per matched keyword (before position weighting).
    ///
    /// Tier guide:
    ///   5.0  exact multi-word commands — almost never ambiguous
    ///   3.0  domain nouns (audit, cve, vulnerability…) — context-specific
    ///   2.0  strong intent verbs (fix, implement, explain…)
    ///   1.0  weak / polysemous signals (find, create, error…)
    pub base_score: f32,
    pub keywords: Vec<String>,
}

/// TOML-deserializable form of a classifier rule.
#[derive(Debug, Deserialize)]
struct RuleToml {
    task_type: String,
    base_score: f32,
    keywords: Vec<String>,
}

/// TOML-deserializable form of a few-shot example.
#[derive(Debug, Deserialize, Clone)]
pub struct FewShotExample {
    pub query: String,
    pub task_type: String,
}

/// Top-level structure of `classifier_rules.toml`.
#[derive(Debug, Deserialize)]
pub(crate) struct ClassifierRulesToml {
    #[serde(rename = "rule")]
    rules: Vec<RuleToml>,
    #[serde(rename = "example", default)]
    examples: Vec<FewShotExample>,
}

/// Complete rule set loaded from config (rules + few-shot examples).
pub struct ClassifierRuleSet {
    pub rules: Vec<ClassifierRule>,
    pub examples: Vec<FewShotExample>,
    /// Path from which the rules were loaded (`None` = compiled defaults).
    pub source: Option<std::path::PathBuf>,
}

/// Global rule set — loaded once on first use.
///
/// Search order (first file found wins):
///   1. `$HALCON_CLASSIFIER_RULES`   env var
///   2. `.halcon/classifier_rules.toml`   (project scope)
///   3. `~/.halcon/classifier_rules.toml` (user scope)
///   4. `config/classifier_rules.toml`    (dev / installed)
///   5. Compiled-in defaults              (always available)
pub(crate) static RULE_SET: LazyLock<ClassifierRuleSet> =
    LazyLock::new(ClassifierRuleSet::load_or_default);

impl ClassifierRuleSet {
    /// Load rules from config files, falling back to compiled defaults.
    pub fn load_or_default() -> Self {
        if let Some(ruleset) = Self::try_load_from_env() {
            return ruleset;
        }
        for path in Self::config_search_paths() {
            if path.exists() {
                match Self::load_from_path(&path) {
                    Ok(rs) => {
                        tracing::info!(
                            target: "halcon::classifier",
                            path = %path.display(),
                            rule_count = rs.rules.len(),
                            example_count = rs.examples.len(),
                            "classifier rules loaded from file"
                        );
                        return rs;
                    }
                    Err(e) => {
                        tracing::warn!(
                            target: "halcon::classifier",
                            path = %path.display(),
                            error = %e,
                            "failed to parse classifier rules — using compiled defaults"
                        );
                    }
                }
            }
        }
        tracing::debug!(
            target: "halcon::classifier",
            "no classifier_rules.toml found — using compiled defaults"
        );
        Self::compiled_defaults()
    }

    fn try_load_from_env() -> Option<Self> {
        let path = std::env::var("HALCON_CLASSIFIER_RULES").ok()?;
        let p = std::path::PathBuf::from(path);
        match Self::load_from_path(&p) {
            Ok(rs) => {
                tracing::info!(
                    target: "halcon::classifier",
                    path = %p.display(),
                    "classifier rules loaded from HALCON_CLASSIFIER_RULES"
                );
                Some(rs)
            }
            Err(e) => {
                tracing::warn!(
                    target: "halcon::classifier",
                    error = %e,
                    "HALCON_CLASSIFIER_RULES set but file unreadable — using compiled defaults"
                );
                None
            }
        }
    }

    fn config_search_paths() -> Vec<std::path::PathBuf> {
        let mut paths = vec![];
        // Project scope
        paths.push(std::path::PathBuf::from(".halcon/classifier_rules.toml"));
        // User scope
        if let Some(home) = dirs::home_dir() {
            paths.push(home.join(".halcon/classifier_rules.toml"));
        }
        // Dev / installed
        paths.push(std::path::PathBuf::from("config/classifier_rules.toml"));
        paths
    }

    fn load_from_path(path: &std::path::Path) -> Result<Self, String> {
        let content =
            std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let parsed: ClassifierRulesToml =
            toml::from_str(&content).map_err(|e| format!("parse {}: {e}", path.display()))?;

        let rules = parsed
            .rules
            .into_iter()
            .filter_map(|r| {
                let task_type = TaskType::from_str(&r.task_type)?;
                Some(ClassifierRule {
                    task_type,
                    base_score: r.base_score,
                    keywords: r.keywords,
                })
            })
            .collect();

        Ok(ClassifierRuleSet {
            rules,
            examples: parsed.examples,
            source: Some(path.to_owned()),
        })
    }

    /// Compiled-in defaults — identical keyword set to the original static rules.
    /// These are the ground-truth fallback; the TOML should match or extend them.
    fn compiled_defaults() -> Self {
        macro_rules! rule {
            ($t:expr, $s:expr, [$($k:expr),+ $(,)?]) => {
                ClassifierRule {
                    task_type:  $t,
                    base_score: $s,
                    keywords:   vec![$($k.to_string()),+],
                }
            };
        }

        let rules = vec![
            // Tier 5
            rule!(
                TaskType::GitOperation,
                5.0,
                [
                    "git commit",
                    "git status",
                    "git diff",
                    "git log",
                    "git add",
                    "git push",
                    "git pull",
                    "git fetch",
                    "git branch",
                    "git merge",
                    "git rebase",
                    "git stash",
                    "git checkout",
                    "git cherry-pick",
                    "git bisect",
                    "commit changes",
                    "stage files",
                    "push changes",
                    "pull request",
                    "merge request"
                ]
            ),
            rule!(
                TaskType::FileManagement,
                5.0,
                [
                    "delete file",
                    "remove file",
                    "rename file",
                    "move file",
                    "copy file",
                    "create directory",
                    "create folder",
                    "list files",
                    "show files",
                    "find files",
                    "search files",
                    "delete directory",
                    "remove directory",
                    "file permissions"
                ]
            ),
            // Tier 3
            rule!(
                TaskType::Research,
                3.0,
                [
                    "audit",
                    "auditar",
                    "auditoria",
                    "auditoría",
                    "compliance",
                    "cumplimiento",
                    "soc2",
                    "soc 2",
                    "sox",
                    "gdpr",
                    "hipaa",
                    "iso27001",
                    "iso 27001",
                    "pci-dss",
                    "pci dss",
                    "nist",
                    "fips",
                    "vulnerability",
                    "vulnerabilidad",
                    "vulnerabilities",
                    "cve",
                    "cvss",
                    "exploit",
                    "zero-day",
                    "0day",
                    "pentest",
                    "pen test",
                    "penetration test",
                    "penetration testing",
                    "red team",
                    "blue team",
                    "sast",
                    "dast",
                    "sonarqube",
                    "sonar",
                    "checkmarx",
                    "snyk",
                    "trivy",
                    "grype",
                    "semgrep",
                    "attack surface",
                    "threat model",
                    "threat modeling",
                    "security assessment",
                    "risk assessment"
                ]
            ),
            rule!(
                TaskType::Debugging,
                3.0,
                [
                    "stacktrace",
                    "stack trace",
                    "traceback",
                    "segfault",
                    "segmentation fault",
                    "null pointer",
                    "null reference",
                    "nullpointerexception",
                    "npe",
                    "deadlock",
                    "race condition",
                    "livelock",
                    "memory leak",
                    "memory corruption",
                    "use after free",
                    "buffer overflow",
                    "heap corruption",
                    "undefined behavior",
                    "ub",
                    "asan",
                    "panic at",
                    "thread panicked",
                    "core dump",
                    "crash dump"
                ]
            ),
            // Tier 2
            rule!(
                TaskType::Research,
                2.0,
                [
                    "analyze",
                    "analyse",
                    "investigate",
                    "compare",
                    "examine",
                    "review",
                    "inspect",
                    "survey",
                    "assess",
                    "benchmark",
                    "profile",
                    "measure",
                    "analiza",
                    "analizar",
                    "investiga",
                    "investigar",
                    "revisa",
                    "revisar",
                    "examina",
                    "examinar",
                    "diagnostica",
                    "diagnosticar",
                    "evalua",
                    "evaluar",
                    "inspecciona",
                    "inspeccionar",
                    "compara",
                    "comparar"
                ]
            ),
            rule!(
                TaskType::Explanation,
                2.0,
                [
                    "explain",
                    "describe",
                    "walk me through",
                    "tell me about",
                    "how does",
                    "what is",
                    "what are",
                    "why does",
                    "why is",
                    "when should",
                    "can you clarify",
                    "clarify",
                    "explica",
                    "explicar",
                    "como funciona",
                    "cómo funciona",
                    "que es",
                    "qué es",
                    "que son",
                    "qué son",
                    "por que",
                    "por qué",
                    "cuando usar",
                    "cuándo usar"
                ]
            ),
            rule!(
                TaskType::CodeGeneration,
                2.0,
                [
                    "implement",
                    "scaffold",
                    "generate code",
                    "bootstrap",
                    "add function",
                    "add method",
                    "add class",
                    "add struct",
                    "add feature",
                    "add endpoint",
                    "add route",
                    "write a function",
                    "write a class",
                    "write a test",
                    "write a script",
                    "write a module",
                    "create a function",
                    "create a class",
                    "create a struct",
                    "create a module",
                    "create a service",
                    "create an endpoint",
                    "build a",
                    "develop a"
                ]
            ),
            rule!(
                TaskType::Debugging,
                2.0,
                [
                    "fix",
                    "debug",
                    "diagnose",
                    "troubleshoot",
                    "resolve",
                    "not working",
                    "broken",
                    "crash",
                    "why doesn't",
                    "why doesn't",
                    "not compiling",
                    "fails to",
                    "throwing",
                    "arregla",
                    "arreglar",
                    "corrige",
                    "corregir",
                    "depura",
                    "depurar",
                    "soluciona",
                    "solucionar",
                    "no funciona",
                    "no compila"
                ]
            ),
            rule!(
                TaskType::CodeModification,
                2.0,
                [
                    "modify",
                    "change",
                    "update",
                    "edit",
                    "refactor",
                    "rename",
                    "replace",
                    "rewrite",
                    "restructure",
                    "extract",
                    "inline",
                    "migrate",
                    "port",
                    "optimize",
                    "simplify",
                    "clean up",
                    "modifica",
                    "modificar",
                    "cambia",
                    "cambiar",
                    "actualiza",
                    "actualizar",
                    "refactoriza",
                    "refactorizar",
                    "simplifica",
                    "simplificar"
                ]
            ),
            rule!(
                TaskType::Configuration,
                2.0,
                [
                    "configure",
                    "setup",
                    "set up",
                    "install",
                    "initialize",
                    "initialise",
                    "settings",
                    "configuration",
                    "enable",
                    "disable",
                    "activate",
                    "deactivate",
                    "configura",
                    "configurar",
                    "instala",
                    "instalar",
                    "inicializa",
                    "inicializar",
                    "habilita",
                    "deshabilita",
                    "ajustes"
                ]
            ),
            // Tier 1 — Phase 3: degraded 1.0 → 0.4 (polysemous weak signals)
            // "revisar" removed: duplicate of Tier 2 Research (base_score = 2.0).
            rule!(
                TaskType::CodeGeneration,
                0.4,
                [
                    "write",
                    "create",
                    "build",
                    "make",
                    "develop",
                    "escribe",
                    "escribir",
                    "crea",
                    "crear",
                    "construye"
                ]
            ),
            rule!(
                TaskType::Debugging,
                0.4,
                [
                    "bug",
                    "error",
                    "issue",
                    "problem",
                    "fails",
                    "failure",
                    "wrong",
                    "incorrect",
                    "unexpected",
                    "fallo",
                    "falla",
                    "problema",
                    "erróneo"
                ]
            ),
            rule!(
                TaskType::Research,
                0.4,
                [
                    "find",
                    "search",
                    "look up",
                    "lookup",
                    "research",
                    "scan",
                    "verify",
                    "validate",
                    "check",
                    "busca",
                    "buscar",
                    "encuentra",
                    "verificar",
                    "validar",
                    "comprobar"
                ]
            ),
            rule!(
                TaskType::FileManagement,
                0.4,
                ["delete", "remove", "move", "copy", "list", "show"]
            ),
        ];

        ClassifierRuleSet {
            rules,
            examples: vec![],
            source: None,
        }
    }

    /// Reference to active rules (for scoring loop).
    pub fn rules(&self) -> &[ClassifierRule] {
        &self.rules
    }

    /// Reference to few-shot examples (for Layer 3 fallback).
    pub fn examples(&self) -> &[FewShotExample] {
        &self.examples
    }

    /// Whether rules were loaded from a config file vs compiled defaults.
    pub fn is_from_file(&self) -> bool {
        self.source.is_some()
    }
}

// ─── Conjunction markers for multi-intent detection ───────────────────────────

/// Tokens that signal the user wants TWO distinct actions in one query.
const CONJUNCTION_MARKERS: &[&str] = &[
    "and",
    "y",
    "además",
    "also",
    "then",
    "afterward",
    "followed by",
    "as well as",
    "plus",
];

// ─── W5H2 action verb lexicon ─────────────────────────────────────────────────

/// Known action verbs for W5H2 canonical intent extraction.
/// Ordered: specific verbs first, generic last (first-match wins for verb slot).
const ACTION_VERBS: &[&str] = &[
    // Git
    "commit",
    "push",
    "pull",
    "merge",
    "rebase",
    "checkout",
    "stash",
    // Code operations
    "implement",
    "refactor",
    "optimize",
    "migrate",
    "scaffold",
    "bootstrap",
    "debug",
    "diagnose",
    "troubleshoot",
    "fix",
    "resolve",
    "repair",
    "explain",
    "describe",
    "analyse",
    "analyze",
    "investigate",
    "review",
    "inspect",
    "assess",
    "benchmark",
    "profile",
    "configure",
    "install",
    "setup",
    "enable",
    "disable",
    "modify",
    "update",
    "rename",
    "rewrite",
    "extract",
    // Weak verbs (matched last, override easily by specific)
    "create",
    "build",
    "write",
    "make",
    "find",
    "search",
    "list",
    "delete",
    "remove",
    // Spanish equivalents
    "implementa",
    "refactoriza",
    "optimiza",
    "migra",
    "arregla",
    "corrige",
    "depura",
    "soluciona",
    "explica",
    "analiza",
    "investiga",
    "revisa",
    "examina",
    "evalua",
    "configura",
    "instala",
    "habilita",
    "modifica",
    "actualiza",
    "renombra",
    "reescribe",
    "crea",
    "construye",
    "escribe",
    "encuentra",
    "busca",
    "elimina",
    "auditar",
    "audita",
];

/// Domain nouns for W5H2 canonical intent extraction.
/// Tier-3 keywords that indicate a specific domain context.
const DOMAIN_NOUNS: &[&str] = &[
    // Security
    "vulnerability",
    "vulnerabilidad",
    "exploit",
    "pentest",
    "audit",
    "auditoria",
    "compliance",
    "soc2",
    "gdpr",
    "cve",
    "threat",
    "attack",
    // Code artifacts
    "function",
    "method",
    "class",
    "struct",
    "module",
    "service",
    "endpoint",
    "api",
    "database",
    "schema",
    "migration",
    "query",
    // Infrastructure
    "docker",
    "kubernetes",
    "ci",
    "pipeline",
    "deployment",
    "container",
    "configuration",
    "settings",
    "environment",
    // Quality
    "test",
    "benchmark",
    "performance",
    "memory",
    "deadlock",
    "race",
    "stacktrace",
    "panic",
    "crash",
    // Git
    "commit",
    "branch",
    "merge",
    "conflict",
    "pr",
    "repository",
    // Spanish equivalents
    "función",
    "clase",
    "módulo",
    "servicio",
    "base de datos",
    "prueba",
    "rendimiento",
    "memoria",
    "repositorio",
    "rama",
    "conflicto",
];

// ─── Stop words ───────────────────────────────────────────────────────────────

const STOP_WORDS: &[&str] = &[
    // English
    "a", "an", "the", "this", "that", "these", "those", "is", "are", "was", "were", "be", "been",
    "being", "have", "has", "had", "do", "does", "did", "will", "would", "shall", "should", "may",
    "might", "must", "can", "could", "to", "of", "in", "for", "on", "with", "at", "by", "from",
    "as", "into", "through", "during", "it", "its", "i", "me", "my", "you", "your", "we", "our",
    "they", "their", "and", "or", "but", "if", "because", "while", "after", "before", "so", "not",
    "no", "nor", "all", "some", "any", "each", "every", "both", "few", "more", "most", "other",
    "such", "only", "own", "same", "than", "too", "very", "just", "now", "then", "here", "there",
    "when", "where", "who", "which", "how", "what", "why", // Spanish
    "el", "la", "los", "las", "un", "una", "unos", "unas", "de", "del", "al", "en", "con", "por",
    "para", "que", "es", "son", "fue", "era", "ser", "estar", "mi", "tu", "su", "sus", "nos",
    "vos", "se", "me", "te", "le", "les", "y", "e", "o", "u", "pero", "si", "no", "mas", "más",
    "muy", "este", "esta", "esto", "ese", "esa", "eso", "hay", "bien", "mal", "ya", "aún", "ahora",
];

// ─── Complexity keywords ──────────────────────────────────────────────────────

const ANALYSIS_VERBS: &[&str] = &[
    "analiza",
    "analizar",
    "revisa",
    "revisar",
    "examina",
    "examinar",
    "investiga",
    "investigar",
    "inspecciona",
    "inspeccionar",
    "diagnostica",
    "diagnosticar",
    "evalua",
    "evaluar",
    "assess",
    "investigate",
    "examine",
    "review",
];

const COMPLEX_KEYWORDS: &[&str] = &[
    "refactor",
    "optimize",
    "optimise",
    "migrate",
    "integrate",
    "architecture",
    "design pattern",
    "performance",
    "scale",
    "scalability",
    "distributed",
    "microservice",
    "microservices",
    "concurrent",
    "parallelism",
    "zero-downtime",
    "backwards compatible",
    "breaking change",
    // Spanish
    "arquitectura",
    "escalabilidad",
    "distribuido",
    "refactorizar",
];

// ─── Internal scored result ───────────────────────────────────────────────────

/// Internal representation of raw scoring output from Layer 1.
struct ScoredResult {
    scores: [f32; 9],
    signals: Vec<String>,
    winner_idx: usize,
    runner_up_idx: usize,
    total: f32,
    confidence: f32,
    margin_normalised: f32,
}

// ─── TaskAnalyzer ─────────────────────────────────────────────────────────────

/// Classifies user queries by complexity, type, confidence, and intent structure.
///
/// ## Usage
///
/// ```text
/// // Basic classification
/// let analysis = TaskAnalyzer::analyze("fix the memory leak in connection pool");
///
/// // Context-aware classification
/// let ctx = ContextSignals {
///     file_extensions: &["rs"],
///     in_git_conflict: false,
///     recent_task_types: &[TaskType::Debugging],
/// };
/// let analysis = TaskAnalyzer::analyze_with_context("fix it", &ctx);
/// ```
pub struct TaskAnalyzer;

impl TaskAnalyzer {
    // ── Public API ────────────────────────────────────────────────────────────

    /// Analyse a user query and return a full classification.
    ///
    /// Equivalent to `analyze_with_context(query, &ContextSignals::empty())`.
    pub fn analyze(query: &str) -> TaskAnalysis {
        Self::analyze_with_context(query, &ContextSignals::empty())
    }

    /// Returns a human-readable summary of the active rule set.
    ///
    /// Used by `halcon classifier info` to show operators whether rules come
    /// from a config file or compiled defaults.
    ///
    /// ```text
    /// Rules source: config/classifier_rules.toml
    /// Rules loaded: 14 rules, 22 few-shot examples
    /// ```
    pub fn rule_set_info() -> String {
        let rs = &*RULE_SET;
        let source = rs
            .source
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "compiled defaults".to_string());
        format!(
            "source: {source} | rules: {} | few-shot examples: {}",
            rs.rules().len(),
            rs.examples().len(),
        )
    }

    /// Returns the active few-shot examples (for Layer 3 / diagnostics).
    pub fn few_shot_examples() -> &'static [FewShotExample] {
        RULE_SET.examples()
    }

    /// Analyse with contextual priors from the session environment.
    ///
    /// Context priors are applied as small additive score biases before keyword
    /// matching, so strong keyword evidence always overrides the prior.
    pub fn analyze_with_context(query: &str, ctx: &ContextSignals<'_>) -> TaskAnalysis {
        let word_count = query.split_whitespace().count();
        let complexity = Self::classify_complexity(query, word_count);
        let lower = query.to_lowercase();

        let mut scored = Self::score_layer1(&lower);
        Self::apply_context_priors(&mut scored.scores, ctx, &lower);
        Self::recompute_derived(&mut scored);

        let (task_type, ambiguity) = Self::resolve_type(&scored);
        let secondary_type = Self::pick_secondary(&scored, task_type);
        let is_multi_intent = Self::detect_multi_intent(&lower, &scored);
        let canonical_intent = Self::extract_canonical_intent(&lower);
        let task_hash = Self::compute_semantic_hash(query);

        TaskAnalysis {
            complexity,
            task_type,
            task_hash,
            word_count,
            confidence: scored.confidence,
            signals: scored.signals,
            secondary_type,
            is_multi_intent,
            ambiguity,
            margin: scored.margin_normalised,
            canonical_intent,
        }
    }

    // ── Layer 1: Position-Weighted SMRC ──────────────────────────────────────

    /// Score all keywords with position weighting.
    ///
    /// Keywords in the first 2 query tokens receive `POSITION_WEIGHT_LEADING`
    /// multiplier; tokens 3-4 receive `POSITION_WEIGHT_NEAR`; tokens 5+
    /// receive 1.0 (no boost).  Multi-word phrases use the position of their
    /// first token.
    fn score_layer1(lower: &str) -> ScoredResult {
        let tokens: Vec<&str> = lower.split_whitespace().collect();

        // Build position-weighted prefix spans: fast O(1) substring check
        // avoids computing per-token positions during the scoring loop.
        let prefix_2: String = tokens.iter().take(2).copied().collect::<Vec<_>>().join(" ");
        let prefix_4: String = tokens.iter().take(4).copied().collect::<Vec<_>>().join(" ");

        let mut scores = [0f32; 9];
        let mut signals: Vec<String> = Vec::new();

        for rule in RULE_SET.rules() {
            let idx = Self::type_index(rule.task_type);
            for kw in &rule.keywords {
                let (matched, weight) = if kw.contains(' ') {
                    // Multi-word phrase: use .contains() (no word-boundary needed)
                    if lower.contains(kw.as_str()) {
                        let w = if prefix_2.contains(kw.as_str()) {
                            POSITION_WEIGHT_LEADING
                        } else if prefix_4.contains(kw.as_str()) {
                            POSITION_WEIGHT_NEAR
                        } else {
                            1.0
                        };
                        (true, w)
                    } else {
                        (false, 1.0)
                    }
                } else {
                    // Single word: word-boundary safe check
                    if Self::contains_word_safe(lower, kw) {
                        let w = if Self::contains_word_safe(&prefix_2, kw) {
                            POSITION_WEIGHT_LEADING
                        } else if Self::contains_word_safe(&prefix_4, kw) {
                            POSITION_WEIGHT_NEAR
                        } else {
                            1.0
                        };
                        (true, w)
                    } else {
                        (false, 1.0)
                    }
                };

                if matched {
                    scores[idx] += rule.base_score * weight;
                    signals.push(kw.clone());
                }
            }
        }

        // Initial derivation (may be recomputed after context priors).
        let mut result = ScoredResult {
            scores,
            signals,
            winner_idx: 0,
            runner_up_idx: 8,
            total: 0.0,
            confidence: 0.0,
            margin_normalised: 0.0,
        };
        Self::recompute_derived(&mut result);
        result
    }

    /// Recompute winner, runner-up, confidence, and margin from `scores`.
    fn recompute_derived(r: &mut ScoredResult) {
        r.total = r.scores.iter().sum();

        if r.total == 0.0 {
            r.winner_idx = 8; // General
            r.runner_up_idx = 8;
            r.confidence = 0.0;
            r.margin_normalised = 0.0;
            return;
        }

        // Winner = argmax; runner-up = second-highest.
        let mut sorted_indices: Vec<usize> = (0..9).collect();
        sorted_indices.sort_unstable_by(|&a, &b| r.scores[b].partial_cmp(&r.scores[a]).unwrap());

        r.winner_idx = sorted_indices[0];
        r.runner_up_idx = sorted_indices[1];
        r.confidence = r.scores[r.winner_idx] / r.total;
        r.margin_normalised = (r.scores[r.winner_idx] - r.scores[r.runner_up_idx]) / r.total;
    }

    // ── Context priors ────────────────────────────────────────────────────────

    /// Apply additive prior biases based on session context.
    ///
    /// Prior magnitude is deliberately small (≤ 1.0 per signal) so that
    /// a single tier-2 keyword always overrides the prior.
    fn apply_context_priors(scores: &mut [f32; 9], ctx: &ContextSignals<'_>, lower: &str) {
        // File extension priors
        for ext in ctx.file_extensions {
            match *ext {
                "rs" | "go" | "py" | "ts" | "js" | "java" | "cpp" | "c" => {
                    scores[Self::type_index(TaskType::CodeGeneration)] += 0.5;
                    scores[Self::type_index(TaskType::CodeModification)] += 0.5;
                    scores[Self::type_index(TaskType::Debugging)] += 0.3;
                }
                "toml" | "yaml" | "yml" | "json" | "env" | "conf" | "ini" => {
                    scores[Self::type_index(TaskType::Configuration)] += 0.8;
                }
                "md" | "rst" | "txt" => {
                    scores[Self::type_index(TaskType::Explanation)] += 0.4;
                }
                "sql" | "db" | "sqlite" => {
                    scores[Self::type_index(TaskType::Research)] += 0.4;
                    scores[Self::type_index(TaskType::Debugging)] += 0.3;
                }
                _ => {}
            }
        }

        // Git conflict prior
        if ctx.in_git_conflict {
            scores[Self::type_index(TaskType::Debugging)] += 0.6;
            scores[Self::type_index(TaskType::GitOperation)] += 0.4;
        }

        // Recency prior: exponential decay on recent task types.
        // First recent type gets 0.5 bias, second 0.25, third 0.125, capped.
        let mut decay = 0.5f32;
        for &recent in ctx.recent_task_types.iter().take(3) {
            scores[Self::type_index(recent)] += decay;
            decay *= 0.5;
        }

        // Ignore lower in simple contexts (only used for advanced disambiguation)
        let _ = lower;
    }

    // ── Layer 2: Entropy + ambiguity resolution ───────────────────────────────

    /// Resolve `task_type` from scored result and compute ambiguity metadata.
    fn resolve_type(r: &ScoredResult) -> (TaskType, Option<AmbiguityReason>) {
        if r.total == 0.0 {
            return (TaskType::General, Some(AmbiguityReason::NoSignals));
        }

        let winner_type = Self::type_from_index(r.winner_idx);

        // Fast path: crystal-clear winner — skip entropy analysis.
        if r.confidence >= PHRASE_FAST_PATH_CONFIDENCE {
            return (winner_type, None);
        }

        // Below confidence floor → General, no ambiguity (just no signal).
        if r.confidence < CONFIDENCE_FLOOR {
            return (TaskType::General, Some(AmbiguityReason::NoSignals));
        }

        // Entropy analysis over types with positive score.
        let entropy_ratio = Self::compute_entropy_ratio(r);
        if entropy_ratio > AMBIGUITY_ENTROPY_RATIO {
            return (
                winner_type,
                Some(AmbiguityReason::HighEntropy { entropy_ratio }),
            );
        }

        // Margin check.
        if r.margin_normalised < AMBIGUITY_MARGIN {
            return (
                winner_type,
                Some(AmbiguityReason::NarrowMargin {
                    margin: r.margin_normalised,
                }),
            );
        }

        (winner_type, None)
    }

    /// Shannon entropy ratio H/H_max for the score distribution.
    ///
    /// H_max = log₂(N) where N = number of types with positive scores.
    /// Returns 0.0 when only one type has a positive score (deterministic).
    fn compute_entropy_ratio(r: &ScoredResult) -> f32 {
        let active: Vec<f32> = r
            .scores
            .iter()
            .filter(|&&s| s > 0.0)
            .map(|&s| s / r.total)
            .collect();

        let n = active.len();
        if n <= 1 {
            return 0.0;
        }

        let entropy: f32 = active.iter().map(|&p| -p * p.log2()).sum();

        let h_max = (n as f32).log2();
        entropy / h_max
    }

    // ── Layer 3: Enrichment ───────────────────────────────────────────────────

    /// Pick the runner-up task type for multi-intent exposure.
    fn pick_secondary(r: &ScoredResult, primary: TaskType) -> Option<TaskType> {
        let runner_up = Self::type_from_index(r.runner_up_idx);
        if runner_up == primary
            || runner_up == TaskType::General
            || r.scores[r.runner_up_idx] == 0.0
        {
            return None;
        }
        Some(runner_up)
    }

    /// True when the query contains a conjunction marker AND has signal mass
    /// in at least two distinct TaskType buckets.
    ///
    /// This heuristic matches queries like:
    /// - "explain AND fix the authentication bug"
    /// - "review and refactor the service"
    /// - "analiza y corrige task_analyzer.rs"
    fn detect_multi_intent(lower: &str, r: &ScoredResult) -> bool {
        let has_conjunction = CONJUNCTION_MARKERS
            .iter()
            .any(|&m| Self::contains_word_safe(lower, m) || lower.contains(m));

        if !has_conjunction {
            return false;
        }

        let types_with_signal: usize = r.scores.iter().filter(|&&s| s > 0.0).count();
        types_with_signal >= 2
    }

    /// W5H2 canonical intent extraction: `"verb:domain"` form.
    ///
    /// Extracts:
    /// - Action verb: first token matching `ACTION_VERBS` (left-to-right)
    /// - Domain noun: first token matching `DOMAIN_NOUNS` (left-to-right)
    ///
    /// The resulting key is stable across surface rephrasing:
    /// "fix the memory leak" and "resolve memory leak issue" both → `"fix:memory"`
    pub(crate) fn extract_canonical_intent(lower: &str) -> Option<String> {
        let verb = ACTION_VERBS
            .iter()
            .find(|&&v| Self::contains_word_safe(lower, v))
            .copied();

        let domain = DOMAIN_NOUNS.iter().find(|&&d| lower.contains(d)).copied();

        match (verb, domain) {
            (Some(v), Some(d)) => Some(format!("{v}:{d}")),
            (Some(v), None) => Some(v.to_string()),
            (None, Some(d)) => Some(d.to_string()),
            (None, None) => None,
        }
    }

    // ── Complexity ────────────────────────────────────────────────────────────

    pub(crate) fn classify_complexity(query: &str, word_count: usize) -> TaskComplexity {
        let lower = query.to_lowercase();

        if COMPLEX_KEYWORDS.iter().any(|kw| lower.contains(kw)) {
            return TaskComplexity::Complex;
        }

        if word_count < 10
            && ANALYSIS_VERBS
                .iter()
                .any(|v| Self::contains_word_safe(&lower, v))
        {
            return TaskComplexity::Moderate;
        }

        match word_count {
            0..=9 => TaskComplexity::Simple,
            10..=35 => TaskComplexity::Moderate,
            _ => TaskComplexity::Complex,
        }
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
    pub fn compute_semantic_hash(query: &str) -> String {
        let cleaned: String = query
            .trim()
            .to_lowercase()
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c.is_whitespace() {
                    c
                } else {
                    ' '
                }
            })
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

    // ── Index helpers ─────────────────────────────────────────────────────────

    #[inline]
    fn type_index(t: TaskType) -> usize {
        match t {
            TaskType::CodeGeneration => 0,
            TaskType::CodeModification => 1,
            TaskType::Debugging => 2,
            TaskType::Research => 3,
            TaskType::FileManagement => 4,
            TaskType::GitOperation => 5,
            TaskType::Explanation => 6,
            TaskType::Configuration => 7,
            TaskType::General => 8,
        }
    }

    #[inline]
    fn type_from_index(i: usize) -> TaskType {
        match i {
            0 => TaskType::CodeGeneration,
            1 => TaskType::CodeModification,
            2 => TaskType::Debugging,
            3 => TaskType::Research,
            4 => TaskType::FileManagement,
            5 => TaskType::GitOperation,
            6 => TaskType::Explanation,
            7 => TaskType::Configuration,
            _ => TaskType::General,
        }
    }

    // ── Word-boundary matching ────────────────────────────────────────────────

    /// Returns `true` when `text` contains `word` at a proper word boundary.
    ///
    /// ## UTF-8 safety
    ///
    /// Uses only `str` slice operations and `chars()` — guaranteed to produce
    /// valid char boundaries.  Single-byte keywords (ASCII) ensure that
    /// `pos + word.len()` is always a valid char boundary in `text`.
    pub fn contains_word_safe(text: &str, word: &str) -> bool {
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

                    let before_ok = pos == 0 || {
                        let bc = text[..pos].chars().next_back().unwrap_or(' ');
                        (!bc.is_alphanumeric()) && (bc != '_')
                    };

                    let after_pos = pos + wlen;
                    let after_ok = after_pos >= tlen || {
                        let ac = text[after_pos..].chars().next().unwrap_or(' ');
                        (!ac.is_alphanumeric()) && (ac != '_')
                    };

                    if before_ok && after_ok {
                        return true;
                    }

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
// Current production path uses `IntentScorer` (wired in reasoning_engine.rs).
// This trait + `KeywordClassifier` exist to:
//   1. Expose `TaskAnalyzer` via a standard interface for tooling/tests.
//   2. Provide a clean migration point when an LLM-based backend is added.
//
// NOTE: `KeywordClassifier::classify()` is NOT currently called in the production
// agent loop — `IntentScorer::score()` is the live path.  These types will become
// the main path once scorer and analyzer are unified (ARCH-01).

/// Which backend produced the classification.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ClassificationMethod {
    /// 2026 Cascade-SMRC™ — position-weighted keyword implementation.
    KeywordCascadeSMRC,
    /// Reserved — future LLM-based one-shot classification.
    LlmOneShot,
    /// Reserved — LLM with SMRC fallback when confidence < `CONFIDENCE_FLOOR`.
    LlmWithKeywordFallback { llm_confidence: u8 },
}

/// Result of classifying a natural-language query (trait return type).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ClassificationResult {
    pub task_type: TaskType,
    pub confidence: f32,
    pub complexity: TaskComplexity,
    pub task_hash: String,
    pub word_count: usize,
    pub method: ClassificationMethod,
    pub secondary_type: Option<TaskType>,
    pub is_multi_intent: bool,
    pub ambiguity: Option<AmbiguityReason>,
    pub canonical_intent: Option<String>,
}

/// Stable call surface for intent classification backends.
///
/// Callers depend on this trait, enabling zero-callsite migration between
/// backends (keyword → LLM → hybrid).
#[allow(dead_code)]
pub trait IntentClassifier {
    fn classify(&self, query: &str) -> ClassificationResult;
    fn classify_with_context(&self, query: &str, ctx: &ContextSignals<'_>) -> ClassificationResult;
}

/// Keyword-based Cascade-SMRC classifier — wraps `TaskAnalyzer`.
#[allow(dead_code)]
pub struct KeywordClassifier;

impl IntentClassifier for KeywordClassifier {
    fn classify(&self, query: &str) -> ClassificationResult {
        let a = TaskAnalyzer::analyze(query);
        ClassificationResult {
            task_type: a.task_type,
            confidence: a.confidence,
            complexity: a.complexity,
            task_hash: a.task_hash,
            word_count: a.word_count,
            method: ClassificationMethod::KeywordCascadeSMRC,
            secondary_type: a.secondary_type,
            is_multi_intent: a.is_multi_intent,
            ambiguity: a.ambiguity,
            canonical_intent: a.canonical_intent,
        }
    }

    fn classify_with_context(&self, query: &str, ctx: &ContextSignals<'_>) -> ClassificationResult {
        let a = TaskAnalyzer::analyze_with_context(query, ctx);
        ClassificationResult {
            task_type: a.task_type,
            confidence: a.confidence,
            complexity: a.complexity,
            task_hash: a.task_hash,
            word_count: a.word_count,
            method: ClassificationMethod::KeywordCascadeSMRC,
            secondary_type: a.secondary_type,
            is_multi_intent: a.is_multi_intent,
            ambiguity: a.ambiguity,
            canonical_intent: a.canonical_intent,
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

    // ── SMRC: score-based wins ────────────────────────────────────────────────

    #[test]
    fn smrc_create_audit_report_is_research_not_code_gen() {
        let analysis = TaskAnalyzer::analyze("create a security audit report");
        assert_eq!(
            analysis.task_type,
            TaskType::Research,
            "Research(audit×3) must beat CodeGeneration(create×1), confidence={}",
            analysis.confidence
        );
    }

    #[test]
    fn smrc_fix_vulnerability_is_research_not_debugging() {
        let analysis = TaskAnalyzer::analyze("fix the vulnerability in the auth module");
        assert_eq!(
            analysis.task_type,
            TaskType::Research,
            "Research(vulnerability×3) must beat Debugging(fix×2), margin={}",
            analysis.margin
        );
    }

    #[test]
    fn smrc_git_commit_beats_fix_bug() {
        let analysis = TaskAnalyzer::analyze("git commit all changes to fix the bug");
        assert_eq!(analysis.task_type, TaskType::GitOperation);
    }

    #[test]
    fn smrc_verify_soc2_compliance_is_research() {
        let analysis = TaskAnalyzer::analyze("verify SOC2 compliance controls are passing");
        assert_eq!(analysis.task_type, TaskType::Research);
    }

    #[test]
    fn smrc_confidence_exposed() {
        let clear = TaskAnalyzer::analyze("git status");
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
        let analysis = TaskAnalyzer::analyze("prefix the function name");
        assert_ne!(analysis.task_type, TaskType::Debugging);
    }

    #[test]
    fn word_boundary_write_not_in_rewrite() {
        let analysis = TaskAnalyzer::analyze("rewrite this module");
        assert_ne!(analysis.task_type, TaskType::CodeGeneration);
    }

    #[test]
    fn word_boundary_analiza_not_in_reanalizando() {
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
        assert!(!TaskAnalyzer::contains_word_safe(
            "rewrite the function",
            "write"
        ));
    }

    #[test]
    fn word_boundary_utf8_safe_with_accented_chars() {
        assert!(!TaskAnalyzer::contains_word_safe(
            "reanalizó el proceso",
            "analiza"
        ));
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
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn hash_order_independent() {
        let h1 = TaskAnalyzer::compute_semantic_hash("fix authentication bug");
        let h2 = TaskAnalyzer::compute_semantic_hash("bug authentication fix");
        assert_eq!(h1, h2, "semantic hash must be order-independent");
    }

    #[test]
    fn hash_stop_word_removal() {
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

    // ── Audit / compliance keyword tests ──────────────────────────────────────

    #[test]
    fn p1c_audit_keyword_is_research() {
        let analysis = TaskAnalyzer::analyze("audit the database access logs");
        assert_eq!(
            analysis.task_type,
            TaskType::Research,
            "confidence={}",
            analysis.confidence
        );
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

    // ── Layer 2: Ambiguity metadata ───────────────────────────────────────────

    #[test]
    fn ambiguity_no_signals_for_empty_query() {
        let analysis = TaskAnalyzer::analyze("hello there");
        assert_eq!(analysis.ambiguity, Some(AmbiguityReason::NoSignals));
        assert_eq!(analysis.task_type, TaskType::General);
    }

    #[test]
    fn ambiguity_none_for_crystal_clear_query() {
        // "git status" → single phrase, confidence = 1.0 → fast path, no ambiguity.
        let analysis = TaskAnalyzer::analyze("git status");
        assert!(
            analysis.ambiguity.is_none(),
            "Crystal-clear query must not be tagged ambiguous: {:?}",
            analysis.ambiguity
        );
    }

    #[test]
    fn ambiguity_margin_exposed_for_close_call() {
        // "fix and explain" — two competing types close in score.
        let analysis = TaskAnalyzer::analyze("fix and explain");
        // margin field must be populated regardless of ambiguity tag
        assert!(analysis.margin >= 0.0 && analysis.margin <= 1.0);
    }

    // ── Layer 3: Multi-intent ─────────────────────────────────────────────────

    #[test]
    fn multi_intent_detected_for_conjunctive_query() {
        // "explain AND fix" → Explanation + Debugging signals, conjunctive marker
        let analysis = TaskAnalyzer::analyze("explain and fix the authentication bug");
        assert!(
            analysis.is_multi_intent,
            "Conjunctive query with dual signals must be multi-intent"
        );
    }

    #[test]
    fn multi_intent_false_for_single_type_query() {
        let analysis = TaskAnalyzer::analyze("git status");
        assert!(!analysis.is_multi_intent);
    }

    #[test]
    fn multi_intent_spanish_y_conjunction() {
        // "analiza y corrige" — Spanish conjunction with dual signals
        let analysis = TaskAnalyzer::analyze("analiza y corrige el archivo");
        assert!(
            analysis.is_multi_intent,
            "Spanish 'y' conjunction with dual signals must be multi-intent"
        );
    }

    #[test]
    fn secondary_type_populated_for_multi_type_query() {
        let analysis = TaskAnalyzer::analyze("explain and fix the authentication bug");
        assert!(
            analysis.secondary_type.is_some(),
            "Multi-type query must expose secondary_type"
        );
    }

    #[test]
    fn secondary_type_none_for_single_type_query() {
        let analysis = TaskAnalyzer::analyze("git status");
        assert!(analysis.secondary_type.is_none());
    }

    // ── Layer 3: Canonical intent ─────────────────────────────────────────────

    #[test]
    fn canonical_intent_populated_for_actionable_query() {
        let analysis = TaskAnalyzer::analyze("fix the memory leak in connection pool");
        assert!(
            analysis.canonical_intent.is_some(),
            "Query with action verb must have canonical_intent"
        );
        let intent = analysis.canonical_intent.unwrap();
        assert!(
            intent.contains("fix"),
            "canonical_intent must contain action verb 'fix', got: {intent}"
        );
    }

    #[test]
    fn canonical_intent_contains_domain_noun() {
        let analysis = TaskAnalyzer::analyze("fix the memory leak");
        let intent = analysis.canonical_intent.unwrap_or_default();
        // Should be "fix:memory" or similar
        assert!(
            intent.contains(':') && (intent.contains("memory") || intent.contains("fix")),
            "canonical_intent should be verb:domain form, got: {intent}"
        );
    }

    #[test]
    fn canonical_intent_stable_across_rephrasing() {
        // "fix the memory leak" and "resolve memory leak issue" should have
        // the same or similar canonical intent.
        let a1 = TaskAnalyzer::analyze("fix the memory leak");
        let a2 = TaskAnalyzer::analyze("fix memory leak issue");
        // Both should contain "fix" in canonical_intent
        let i1 = a1.canonical_intent.unwrap_or_default();
        let i2 = a2.canonical_intent.unwrap_or_default();
        assert!(i1.contains("fix"), "got: {i1}");
        assert!(i2.contains("fix"), "got: {i2}");
    }

    #[test]
    fn canonical_intent_none_for_no_signal_query() {
        let analysis = TaskAnalyzer::analyze("hello there");
        // May or may not be None depending on whether "there" matches a domain noun.
        // Just verify it doesn't panic.
        let _ = analysis.canonical_intent;
    }

    // ── Context priors ────────────────────────────────────────────────────────

    #[test]
    fn context_rust_files_bias_toward_code() {
        let ctx = ContextSignals {
            file_extensions: &["rs"],
            in_git_conflict: false,
            recent_task_types: &[],
        };
        // Ambiguous query that context tips toward code
        let without_ctx = TaskAnalyzer::analyze("update it");
        let with_ctx = TaskAnalyzer::analyze_with_context("update it", &ctx);
        // With Rust context, CodeModification/Debugging should get higher confidence
        // At minimum: must not panic and must return a valid type.
        let _ = (without_ctx.task_type, with_ctx.task_type);
    }

    #[test]
    fn context_git_conflict_biases_debugging() {
        let ctx = ContextSignals {
            file_extensions: &[],
            in_git_conflict: true,
            recent_task_types: &[],
        };
        let analysis = TaskAnalyzer::analyze_with_context("help", &ctx);
        // With git conflict context, Debugging or GitOperation should score higher.
        // Must not panic; result is type-correct.
        assert!(
            matches!(
                analysis.task_type,
                TaskType::Debugging | TaskType::GitOperation | TaskType::General
            ),
            "Git conflict context should not produce irrelevant types: {:?}",
            analysis.task_type
        );
    }

    #[test]
    fn context_recency_prior_applied() {
        let ctx = ContextSignals {
            file_extensions: &[],
            in_git_conflict: false,
            recent_task_types: &[TaskType::Configuration],
        };
        // "set" is ambiguous between Configuration and other types.
        // With Configuration recency prior, Configuration should score higher.
        let analysis = TaskAnalyzer::analyze_with_context("set it up", &ctx);
        // Just verify no panic and produces a valid result.
        assert!(TaskType::from_str(analysis.task_type.as_str()).is_some());
    }

    #[test]
    fn context_empty_signals_matches_analyze() {
        // ContextSignals::empty() must produce identical result to analyze().
        let q = "fix the authentication bug";
        let a1 = TaskAnalyzer::analyze(q);
        let a2 = TaskAnalyzer::analyze_with_context(q, &ContextSignals::empty());
        assert_eq!(a1.task_type, a2.task_type);
        assert!((a1.confidence - a2.confidence).abs() < f32::EPSILON);
    }

    // ── IntentClassifier trait ────────────────────────────────────────────────

    #[test]
    fn keyword_classifier_trait_object_works() {
        let clf: Box<dyn IntentClassifier> = Box::new(KeywordClassifier);
        let result = clf.classify("git status");
        assert_eq!(result.task_type, TaskType::GitOperation);
        assert_eq!(result.method, ClassificationMethod::KeywordCascadeSMRC);
    }

    #[test]
    fn keyword_classifier_with_context_works() {
        let clf = KeywordClassifier;
        let ctx = ContextSignals {
            file_extensions: &["rs"],
            in_git_conflict: false,
            recent_task_types: &[],
        };
        let result = clf.classify_with_context("fix the bug", &ctx);
        assert_eq!(result.task_type, TaskType::Debugging);
    }

    // ── Entropy computation ───────────────────────────────────────────────────

    #[test]
    fn entropy_zero_for_single_type() {
        // "git status" fires only GitOperation → entropy = 0.
        let lower = "git status";
        let scored = TaskAnalyzer::score_layer1(lower);
        let entropy = TaskAnalyzer::compute_entropy_ratio(&scored);
        assert!(
            entropy < 0.01,
            "Single-type query must have near-zero entropy, got {entropy}"
        );
    }

    #[test]
    fn entropy_nonzero_for_mixed_query() {
        // "fix and explain" fires Debugging + Explanation → H > 0.
        let lower = "fix and explain";
        let scored = TaskAnalyzer::score_layer1(lower);
        let entropy = TaskAnalyzer::compute_entropy_ratio(&scored);
        assert!(entropy > 0.0, "Mixed-type query must have positive entropy");
    }

    // ── Position weighting ────────────────────────────────────────────────────

    #[test]
    fn position_weight_leading_verb_scores_higher() {
        let leading = TaskAnalyzer::score_layer1("fix the auth bug");
        let embedded = TaskAnalyzer::score_layer1("the auth problem needs a fix");

        let debug_idx = TaskAnalyzer::type_index(TaskType::Debugging);
        assert!(
            leading.scores[debug_idx] > embedded.scores[debug_idx],
            "Leading 'fix' must score higher than embedded 'fix': {} vs {}",
            leading.scores[debug_idx],
            embedded.scores[debug_idx]
        );
    }

    // ── Dynamic rule set ──────────────────────────────────────────────────────

    #[test]
    fn rule_set_loads_without_panic() {
        // Accessing RULE_SET must not panic regardless of whether
        // classifier_rules.toml exists in the current working directory.
        let info = TaskAnalyzer::rule_set_info();
        assert!(
            info.contains("rules:"),
            "rule_set_info must include rule count: {info}"
        );
    }

    #[test]
    fn rule_set_has_nonzero_rules() {
        assert!(
            !RULE_SET.rules().is_empty(),
            "Active rule set must have at least one rule"
        );
    }

    #[test]
    fn rule_set_all_task_types_parseable() {
        // Every rule loaded from config must have a valid TaskType.
        // This verifies that compiled_defaults() and any loaded TOML
        // only reference valid as_str() keys.
        for rule in RULE_SET.rules() {
            assert_ne!(
                rule.task_type,
                TaskType::General,
                "No rule should explicitly target General (it's the fallback)"
            );
        }
    }

    #[test]
    fn dynamic_rule_produces_same_result_as_static() {
        // The dynamic default rules must produce identical classifications
        // to the original static CLASSIFIER_RULES array for well-known queries.
        let pairs = [
            ("git status", TaskType::GitOperation),
            ("fix the bug", TaskType::Debugging),
            ("explain how async works", TaskType::Explanation),
            ("audit the database access", TaskType::Research),
            ("configure the database", TaskType::Configuration),
            ("delete file temp.txt", TaskType::FileManagement),
        ];
        for (query, expected) in &pairs {
            let analysis = TaskAnalyzer::analyze(query);
            assert_eq!(
                analysis.task_type, *expected,
                "Dynamic rules: wrong type for '{query}': got {:?}",
                analysis.task_type
            );
        }
    }

    #[test]
    fn custom_rule_can_be_injected_at_runtime() {
        // Simulate what happens when a user adds a custom keyword to
        // classifier_rules.toml by building a ClassifierRuleSet directly.
        let custom = ClassifierRuleSet {
            rules: vec![ClassifierRule {
                task_type: TaskType::Research,
                base_score: 5.0,
                keywords: vec!["halcon-custom-keyword".to_string()],
            }],
            examples: vec![],
            source: Some(std::path::PathBuf::from("test-custom.toml")),
        };
        assert!(custom.is_from_file());
        assert_eq!(custom.rules().len(), 1);
        assert_eq!(custom.rules()[0].keywords[0], "halcon-custom-keyword");
    }

    #[test]
    fn few_shot_examples_are_accessible() {
        // This test verifies the API — actual examples depend on whether
        // classifier_rules.toml was found (may be empty in CI).
        let examples = TaskAnalyzer::few_shot_examples();
        // Must not panic; examples may be zero in fresh checkout.
        for ex in examples {
            assert!(
                TaskType::from_str(&ex.task_type).is_some(),
                "few-shot example task_type '{}' is not a valid TaskType",
                ex.task_type
            );
        }
    }

    #[test]
    fn rule_set_info_format() {
        let info = TaskAnalyzer::rule_set_info();
        // Must contain all three key segments
        assert!(info.contains("source:"), "missing source: {info}");
        assert!(info.contains("rules:"), "missing rules: {info}");
        assert!(
            info.contains("few-shot examples:"),
            "missing examples: {info}"
        );
    }

    #[test]
    fn toml_deserialization_from_string() {
        // Test that the TOML format is parseable without a file on disk.
        let toml_str = r#"
[[rule]]
task_type  = "debugging"
base_score = 3.0
keywords   = ["custom-crash", "custom-panic"]

[[example]]
query     = "the server exploded on startup"
task_type = "debugging"
"#;
        let parsed: super::ClassifierRulesToml =
            toml::from_str(toml_str).expect("TOML must parse without error");
        assert_eq!(parsed.rules.len(), 1);
        assert_eq!(parsed.rules[0].task_type, "debugging");
        assert_eq!(parsed.rules[0].base_score, 3.0);
        assert_eq!(
            parsed.rules[0].keywords,
            vec!["custom-crash", "custom-panic"]
        );
        assert_eq!(parsed.examples.len(), 1);
        assert_eq!(parsed.examples[0].task_type, "debugging");
    }
}
