//! Multi-signal intent profiling — SOTA 2026 replacement for keyword-based task analysis.
//!
//! Replaces fragile word-count + static keyword lists with confidence-weighted multi-dimensional
//! scoring. Five independent signals combine into an `IntentProfile`:
//!
//! | Signal             | Weight | What it measures                          |
//! |--------------------|--------|-------------------------------------------|
//! | Scope              |  30%   | Project-wide vs single-file vs chat       |
//! | Depth              |  25%   | Reasoning depth required                  |
//! | Intent             |  25%   | Task category (code/debug/explain/…)      |
//! | Language           |  10%   | Detected query language                   |
//! | Ambiguity          |  10%   | How well-specified the request is         |
//!
//! The output `IntentProfile` drives:
//! - `ModelRouter` — picks the right model tier (fast / balanced / reasoner)
//! - `StrategySelector` — chooses DirectExecution vs PlanExecuteReflect
//! - `ConvergenceController` — sets dynamic loop bounds
//! - Agent context injection — injects [CONVERSATIONAL MODE] or tool restrictions

use std::collections::HashSet;

use super::task_analyzer::{TaskComplexity, TaskType};

// ── Enums ──────────────────────────────────────────────────────────────────

/// How wide is the scope of the request?
///
/// Ordinal matters: higher value = more rounds / planning needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TaskScope {
    /// Pure chat — no files, no tools, no state.
    Conversational = 0,
    /// Single artifact (one file, one function, one class).
    SingleArtifact = 1,
    /// Local context (a few related files, a module, a test suite).
    LocalContext = 2,
    /// Whole project / repository scan.
    ProjectWide = 3,
    /// Cross-system (multiple repos, infra, external APIs).
    SystemWide = 4,
}

/// How much reasoning is required?
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReasoningDepth {
    /// Direct answer, no analysis needed.
    None = 0,
    /// Light analysis — 1-2 reasoning steps.
    Light = 1,
    /// Deep analysis — multi-step, plan required.
    Deep = 2,
    /// Exhaustive — chain-of-thought, reflection, critique.
    Exhaustive = 3,
}

/// How quickly must we respond?
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LatencyTolerance {
    /// Sub-second response expected (chat replies, status queries).
    Instant,
    /// 1–5 s — quick code snippet or lookup.
    Fast,
    /// 5–15 s — medium analysis or refactor.
    Balanced,
    /// 15 s+ — full project scan, deep reasoning.
    Patient,
}

/// Detected language of the query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QueryLanguage {
    English,
    Spanish,
    Mixed,
    Unknown,
}

// ── IntentProfile ──────────────────────────────────────────────────────────

/// Rich multi-dimensional intent profile replacing flat TaskAnalysis.
///
/// All fields are public so callers can inspect/log the full profile,
/// but construction always goes through `IntentScorer::score()`.
#[derive(Debug, Clone)]
pub struct IntentProfile {
    // ── Core classification (backward-compatible with StrategySelector) ──
    /// Primary task category (mirrors TaskType for UCB1 experience lookup).
    pub task_type: TaskType,
    /// Complexity tier (mirrors TaskComplexity for max_rounds lookup).
    pub complexity: TaskComplexity,

    // ── Multi-dimensional signals ──
    /// Confidence in the classification: 0.0 (noise) – 1.0 (certain).
    pub confidence: f32,
    /// Spatial scope of the task.
    pub scope: TaskScope,
    /// Required reasoning depth.
    pub reasoning_depth: ReasoningDepth,
    /// Whether structured planning is required.
    pub requires_planning: bool,
    /// Whether reflection (post-loop critique) is beneficial.
    pub requires_reflection: bool,

    // ── Resource budget hints ──
    /// Conservative estimate of required tool calls.
    pub estimated_tool_calls: u8,
    /// Approximate input-context tokens needed (from scope × depth).
    pub estimated_context_tokens: u32,
    /// Acceptable response latency class.
    pub latency_tolerance: LatencyTolerance,

    // ── Meta ──
    /// Detected query language (for prompt localization decisions).
    pub detected_language: QueryLanguage,
    /// Ambiguity score: 0.0 (crystal-clear) – 1.0 (totally ambiguous).
    pub ambiguity_score: f32,
    /// Deterministic SHA-256 hash for UCB1 experience lookup.
    pub task_hash: String,
    /// Raw word count (for logs / heuristic fallback).
    pub word_count: usize,
}

impl IntentProfile {
    /// Returns true when the task is purely conversational (no tools needed).
    pub fn is_conversational(&self) -> bool {
        self.scope == TaskScope::Conversational
    }

    /// Returns a model routing tier hint:
    /// - "fast"     → instant/conversational
    /// - "balanced" → single-artifact light analysis
    /// - "deep"     → project-wide or exhaustive reasoning
    pub fn routing_tier(&self) -> &'static str {
        match (self.scope, self.reasoning_depth) {
            (TaskScope::Conversational, _) => "fast",
            (_, ReasoningDepth::None | ReasoningDepth::Light) => "balanced",
            (TaskScope::SingleArtifact, ReasoningDepth::Deep) => "balanced",
            _ => "deep",
        }
    }

    /// Suggested max_rounds derived from scope+depth (override StrategySelector static table).
    pub fn suggested_max_rounds(&self) -> u32 {
        match (self.scope, self.reasoning_depth) {
            (TaskScope::Conversational, _) => 2,
            (TaskScope::SingleArtifact, ReasoningDepth::None | ReasoningDepth::Light) => 4,
            (TaskScope::SingleArtifact, _) => 7,
            (TaskScope::LocalContext, ReasoningDepth::None | ReasoningDepth::Light) => 6,
            (TaskScope::LocalContext, _) => 10,
            (TaskScope::ProjectWide, ReasoningDepth::Deep) => 12,
            (TaskScope::ProjectWide, ReasoningDepth::Exhaustive) => 16,
            (TaskScope::SystemWide, _) => 20,
            _ => 8,
        }
    }
}

// ── IntentScorer ───────────────────────────────────────────────────────────

/// Stateless multi-signal intent classifier.
///
/// All methods are pure functions over the query string. No I/O, no state.
/// Typical call latency: < 200 µs.
pub struct IntentScorer;

impl IntentScorer {
    /// Score a user query and return a full IntentProfile.
    pub fn score(query: &str) -> IntentProfile {
        let q = query.trim();
        let q_lower = q.to_lowercase();
        let word_count = q.split_whitespace().count();

        let language = Self::detect_language(&q_lower);
        let scope = Self::score_scope(&q_lower, word_count);
        let depth = Self::score_depth(&q_lower, scope);
        let (task_type, type_confidence) = Self::score_intent(&q_lower);
        let ambiguity = Self::score_ambiguity(q, word_count, type_confidence);

        // Weighted confidence: scope+depth are the strongest signals.
        let scope_confidence = match scope {
            TaskScope::Conversational => 0.90,
            TaskScope::SingleArtifact => 0.75,
            TaskScope::LocalContext => 0.70,
            TaskScope::ProjectWide => 0.80,
            TaskScope::SystemWide => 0.65,
        };
        let confidence = (0.30 * scope_confidence
            + 0.25 * Self::depth_confidence(depth)
            + 0.25 * type_confidence as f64
            + 0.10 * Self::language_confidence(language)
            + 0.10 * (1.0 - ambiguity as f64)) as f32;
        debug_assert!(
            confidence.is_finite() && (0.0..=1.0).contains(&confidence),
            "intent confidence {confidence} out of [0,1] — check scorer inputs"
        );

        let complexity = Self::derive_complexity(scope, depth, word_count);
        let requires_planning = scope >= TaskScope::LocalContext
            || depth >= ReasoningDepth::Deep
            || matches!(
                task_type,
                TaskType::CodeGeneration | TaskType::CodeModification | TaskType::Debugging
            ) && scope > TaskScope::SingleArtifact;
        let requires_reflection =
            depth == ReasoningDepth::Exhaustive || scope >= TaskScope::ProjectWide;
        let estimated_tool_calls = Self::estimate_tool_calls(scope, task_type);
        let estimated_context_tokens = Self::estimate_context_tokens(scope, depth);
        let latency_tolerance = Self::derive_latency(scope, depth);

        let task_hash = Self::hash_query(q);

        IntentProfile {
            task_type,
            complexity,
            confidence,
            scope,
            reasoning_depth: depth,
            requires_planning,
            requires_reflection,
            estimated_tool_calls,
            estimated_context_tokens,
            latency_tolerance,
            detected_language: language,
            ambiguity_score: ambiguity,
            task_hash,
            word_count,
        }
    }

    // ── Language detection ───────────────────────────────────────────────

    fn detect_language(q: &str) -> QueryLanguage {
        // Compact language-marker word families (no external deps).
        const ES_MARKERS: &[&str] = &[
            "que",
            "qué",
            "cómo",
            "como",
            "por",
            "para",
            "cuál",
            "cual",
            "analiza",
            "analizar",
            "revisa",
            "revisar",
            "examina",
            "explica",
            "implementa",
            "crea",
            "busca",
            "encuentra",
            "muestra",
            "el",
            "la",
            "los",
            "las",
            "un",
            "una",
            "del",
            "con",
        ];
        const EN_MARKERS: &[&str] = &[
            "the",
            "and",
            "for",
            "with",
            "how",
            "what",
            "why",
            "which",
            "create",
            "find",
            "show",
            "implement",
            "analyze",
            "review",
            "fix",
            "write",
            "add",
            "remove",
            "update",
            "check",
        ];

        let es_hits = ES_MARKERS
            .iter()
            .filter(|&&m| Self::contains_word(q, m))
            .count();
        let en_hits = EN_MARKERS
            .iter()
            .filter(|&&m| Self::contains_word(q, m))
            .count();

        match (es_hits, en_hits) {
            (e, _) if e >= 2 && e > en_hits => QueryLanguage::Spanish,
            (_, n) if n >= 2 && n > es_hits => QueryLanguage::English,
            (e, n) if e >= 1 && n >= 1 => QueryLanguage::Mixed,
            _ => QueryLanguage::Unknown,
        }
    }

    fn language_confidence(lang: QueryLanguage) -> f64 {
        match lang {
            QueryLanguage::English | QueryLanguage::Spanish => 0.85,
            QueryLanguage::Mixed => 0.60,
            QueryLanguage::Unknown => 0.40,
        }
    }

    // ── Scope scoring ────────────────────────────────────────────────────

    fn score_scope(q: &str, word_count: usize) -> TaskScope {
        // SystemWide signals
        const SYSTEM_WIDE: &[&str] = &[
            "arquitectura",
            "architecture",
            "infraestructura",
            "infrastructure",
            "multiple repos",
            "cross-repo",
            "sistema completo",
            "entire system",
            "microservices",
            "microservicios",
            "deployment",
            "ci/cd",
        ];
        // ProjectWide signals
        const PROJECT_WIDE: &[&str] = &[
            "proyecto",
            "project",
            "codebase",
            "base de codigo",
            "todo el",
            "all files",
            "todos los archivos",
            "repositorio",
            "repository",
            "analiza el proyecto",
            "analyze the project",
            "audit",
            "current project",
            "proyecto actual",
            "estado del proyecto",
            "estado actual",
            "revisa el proyecto",
        ];
        // LocalContext signals (module / multiple files)
        const LOCAL_CONTEXT: &[&str] = &[
            "module",
            "modulo",
            "package",
            "paquete",
            "directory",
            "directorio",
            "folder",
            "carpeta",
            "multiple files",
            "varios archivos",
            "tests",
            "test suite",
            "all functions in",
            "entire module",
        ];
        // Conversational signals (no tool implied)
        const CONVERSATIONAL: &[&str] = &[
            "how are you",
            "hola",
            "hello",
            "gracias",
            "thanks",
            "thank you",
            "what is your name",
            "who are you",
            "cuéntame",
            "cuéntame sobre",
            "joke",
            "chiste",
            "humor",
            "chat",
            "conversa",
        ];

        if Self::contains_any(q, SYSTEM_WIDE) {
            return TaskScope::SystemWide;
        }
        if Self::contains_any(q, PROJECT_WIDE) {
            return TaskScope::ProjectWide;
        }
        if Self::contains_any(q, LOCAL_CONTEXT) {
            return TaskScope::LocalContext;
        }
        if Self::contains_any(q, CONVERSATIONAL) && word_count < 12 {
            return TaskScope::Conversational;
        }
        // Fallback: use word count as weak signal.
        // NOTE: Conversational scope is NOT assigned here — it requires an explicit
        // keyword match (checked above). Word count alone is insufficient: short
        // task-oriented queries like "analiza mi implementacion" (3 words) are
        // SingleArtifact, not Conversational (Phase L fix B1).
        match word_count {
            0..=10 => TaskScope::SingleArtifact,
            11..=25 => TaskScope::LocalContext,
            _ => TaskScope::ProjectWide,
        }
    }

    // ── Depth scoring ────────────────────────────────────────────────────

    fn score_depth(q: &str, scope: TaskScope) -> ReasoningDepth {
        const EXHAUSTIVE: &[&str] = &[
            "audit",
            "audita",
            "comprehensive review",
            "revisión completa",
            "deep dive",
            "análisis profundo",
            "full analysis",
            "from scratch",
            "architecture review",
            "revisión de arquitectura",
            "diagnóstica",
            "profundo",
            "exhaustivo",
            "complete refactor",
        ];
        const DEEP: &[&str] = &[
            "analiza",
            "analyze",
            "diagnose",
            "diagostica",
            "investigate",
            "investiga",
            "refactor",
            "redesign",
            "why is",
            "por qué",
            "how does",
            "como funciona",
            "optimize",
            "optimiza",
            "performance",
            "rendimiento",
            "state",
            "estado",
        ];
        const LIGHT: &[&str] = &[
            "fix",
            "arregla",
            "add",
            "agrega",
            "remove",
            "elimina",
            "update",
            "actualiza",
            "rename",
            "check",
            "verifica",
            "show me",
            "muéstrame",
            "list",
            "lista",
        ];

        if Self::contains_any(q, EXHAUSTIVE) || scope >= TaskScope::ProjectWide {
            return ReasoningDepth::Exhaustive;
        }
        if Self::contains_any(q, DEEP) || scope == TaskScope::LocalContext {
            return ReasoningDepth::Deep;
        }
        if scope == TaskScope::Conversational {
            return ReasoningDepth::None;
        }
        if Self::contains_any(q, LIGHT) {
            return ReasoningDepth::Light;
        }
        ReasoningDepth::Light
    }

    fn depth_confidence(depth: ReasoningDepth) -> f64 {
        match depth {
            ReasoningDepth::None => 0.90,
            ReasoningDepth::Light => 0.75,
            ReasoningDepth::Deep => 0.70,
            ReasoningDepth::Exhaustive => 0.80,
        }
    }

    // ── Intent scoring ───────────────────────────────────────────────────

    /// Returns (TaskType, confidence 0.0–1.0).
    fn score_intent(q: &str) -> (TaskType, f32) {
        // Priority-ordered rules — first match wins.
        const DEBUGGING: &[&str] = &[
            "bug",
            "error",
            "fix",
            "broken",
            "crash",
            "fail",
            "arregla",
            "falla",
            "error en",
            "no funciona",
            "roto",
        ];
        const CODE_GEN: &[&str] = &[
            "create",
            "crea",
            "write",
            "escribe",
            "implement",
            "implementa",
            "build",
            "construye",
            "generate",
            "genera",
            "scaffold",
            "add function",
            "agrega función",
            "new class",
            "nueva clase",
        ];
        const CODE_MOD: &[&str] = &[
            "refactor",
            "refactoriza",
            "rename",
            "cambia nombre",
            "update",
            "actualiza",
            "modify",
            "modifica",
            "change",
            "cambia",
            "move",
            "mueve",
            "extract",
            "extrae",
        ];
        const GIT_OP: &[&str] = &[
            "commit",
            "branch",
            "rama",
            "merge",
            "rebase",
            "push",
            "pull",
            "status",
            "diff",
            "git log",
            "git status",
        ];
        const FILE_MGMT: &[&str] = &[
            "file",
            "archivo",
            "directory",
            "directorio",
            "folder",
            "carpeta",
            "delete",
            "elimina",
            "copy",
            "copia",
            "move file",
            "mueve archivo",
            "create file",
            "crea archivo",
        ];
        const CONFIG: &[&str] = &[
            "config",
            "configuración",
            "setup",
            "configura",
            "settings",
            "install",
            "instala",
            "environment",
            "entorno",
            "variable",
        ];
        const RESEARCH: &[&str] = &[
            "find",
            "busca",
            "search",
            "search for",
            "analyze",
            "analiza",
            "investigate",
            "investiga",
            "review",
            "revisa",
            "examine",
            "examina",
            "audit",
            "audita",
            "inspect",
            "diagnostica",
        ];
        const EXPLANATION: &[&str] = &[
            "explain",
            "explica",
            "how does",
            "como funciona",
            "what is",
            "qué es",
            "why does",
            "por qué",
            "describe",
            "tell me about",
        ];

        let rules: &[(&[&str], TaskType, f32)] = &[
            (DEBUGGING, TaskType::Debugging, 0.85),
            (GIT_OP, TaskType::GitOperation, 0.90),
            (CODE_GEN, TaskType::CodeGeneration, 0.80),
            (CODE_MOD, TaskType::CodeModification, 0.75),
            (FILE_MGMT, TaskType::FileManagement, 0.80),
            (CONFIG, TaskType::Configuration, 0.75),
            (RESEARCH, TaskType::Research, 0.70),
            (EXPLANATION, TaskType::Explanation, 0.75),
        ];

        for (kws, task_type, confidence) in rules {
            let hits: usize = kws.iter().filter(|&&kw| Self::contains_word(q, kw)).count();
            if hits >= 1 {
                // Boost confidence when multiple keywords match.
                let boosted = (confidence + 0.05 * (hits.saturating_sub(1) as f32)).min(0.95);
                return (*task_type, boosted);
            }
        }

        (TaskType::General, 0.40)
    }

    // ── Ambiguity scoring ────────────────────────────────────────────────

    fn score_ambiguity(q: &str, word_count: usize, type_confidence: f32) -> f32 {
        let mut score: f32 = 0.0;

        // Very short = ambiguous
        if word_count <= 2 {
            score += 0.4;
        } else if word_count <= 5 {
            score += 0.2;
        }

        // Pronouns without referent = ambiguous
        let pronoun_hits = ["it", "this", "that", "them", "those", "ese", "esto", "eso"]
            .iter()
            .filter(|&&p| {
                let q_low = q.to_lowercase();
                Self::contains_word(&q_low, p)
            })
            .count();
        score += 0.1 * pronoun_hits.min(3) as f32;

        // Low classification confidence = ambiguous
        if type_confidence < 0.50 {
            score += 0.3;
        } else if type_confidence < 0.70 {
            score += 0.15;
        }

        score.min(1.0)
    }

    // ── Derived fields ───────────────────────────────────────────────────

    fn derive_complexity(
        scope: TaskScope,
        depth: ReasoningDepth,
        word_count: usize,
    ) -> TaskComplexity {
        // Scope + depth take priority over word count.
        match (scope, depth) {
            (TaskScope::Conversational, _) => TaskComplexity::Simple,
            (TaskScope::SingleArtifact, ReasoningDepth::None | ReasoningDepth::Light) => {
                if word_count > 35 {
                    TaskComplexity::Moderate
                } else {
                    TaskComplexity::Simple
                }
            }
            (TaskScope::SingleArtifact, _) => TaskComplexity::Moderate,
            (TaskScope::LocalContext, ReasoningDepth::None | ReasoningDepth::Light) => {
                TaskComplexity::Moderate
            }
            (TaskScope::LocalContext, _) => TaskComplexity::Complex,
            (TaskScope::ProjectWide | TaskScope::SystemWide, _) => TaskComplexity::Complex,
        }
    }

    fn estimate_tool_calls(scope: TaskScope, task_type: TaskType) -> u8 {
        let base: u8 = match scope {
            TaskScope::Conversational => 0,
            TaskScope::SingleArtifact => 2,
            TaskScope::LocalContext => 5,
            TaskScope::ProjectWide => 10,
            TaskScope::SystemWide => 15,
        };
        let type_bonus: u8 = match task_type {
            TaskType::Research => 3,
            TaskType::Debugging => 2,
            TaskType::CodeGeneration => 1,
            TaskType::CodeModification => 2,
            _ => 0,
        };
        base.saturating_add(type_bonus)
    }

    fn estimate_context_tokens(scope: TaskScope, depth: ReasoningDepth) -> u32 {
        let scope_k: u32 = match scope {
            TaskScope::Conversational => 1,
            TaskScope::SingleArtifact => 4,
            TaskScope::LocalContext => 12,
            TaskScope::ProjectWide => 32,
            TaskScope::SystemWide => 64,
        };
        let depth_mult: u32 = match depth {
            ReasoningDepth::None => 1,
            ReasoningDepth::Light => 2,
            ReasoningDepth::Deep => 3,
            ReasoningDepth::Exhaustive => 4,
        };
        scope_k * depth_mult * 1000
    }

    fn derive_latency(scope: TaskScope, depth: ReasoningDepth) -> LatencyTolerance {
        match (scope, depth) {
            (TaskScope::Conversational, _) => LatencyTolerance::Instant,
            (TaskScope::SingleArtifact, ReasoningDepth::None | ReasoningDepth::Light) => {
                LatencyTolerance::Fast
            }
            (TaskScope::SingleArtifact, _)
            | (TaskScope::LocalContext, ReasoningDepth::None | ReasoningDepth::Light) => {
                LatencyTolerance::Balanced
            }
            _ => LatencyTolerance::Patient,
        }
    }

    // ── Hash ─────────────────────────────────────────────────────────────

    fn hash_query(q: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(q.as_bytes());
        let result = hasher.finalize();
        format!("{:x}", result)[..16].to_string()
    }

    // ── Helpers ───────────────────────────────────────────────────────────

    /// Word-boundary aware match. Prevents "fix" matching inside "prefix".
    fn contains_word(text: &str, word: &str) -> bool {
        if word.is_empty() {
            return false;
        }
        // Multi-word phrases: use substring match (boundary doesn't apply to spaces).
        if word.contains(' ') {
            return text.contains(word);
        }
        let wlen = word.len();
        let tbytes = text.as_bytes();
        let _wbytes = word.as_bytes();

        let mut start = 0usize;
        while start + wlen <= tbytes.len() {
            if let Some(pos) = text[start..].find(word) {
                let abs = start + pos;
                let before_ok = abs == 0 || !tbytes[abs - 1].is_ascii_alphanumeric();
                let after_ok =
                    abs + wlen == tbytes.len() || !tbytes[abs + wlen].is_ascii_alphanumeric();
                if before_ok && after_ok {
                    return true;
                }
                start = abs + 1;
            } else {
                break;
            }
        }
        false
    }

    fn contains_any(text: &str, words: &[&str]) -> bool {
        words.iter().any(|w| Self::contains_word(text, w))
    }

    /// Returns the distinct word set of a query (for ambiguity / overlap analysis).
    #[allow(dead_code)]
    fn word_set(text: &str) -> HashSet<&str> {
        text.split_whitespace().collect()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Scope detection ───────────────────────────────────────────────────

    #[test]
    fn scope_conversational_hola() {
        let p = IntentScorer::score("hola");
        assert_eq!(p.scope, TaskScope::Conversational);
    }

    #[test]
    fn scope_project_wide_analiza_proyecto() {
        let p = IntentScorer::score("analiza mi proyecto actual y el estado");
        assert_eq!(p.scope, TaskScope::ProjectWide, "scope was {:?}", p.scope);
    }

    #[test]
    fn scope_project_wide_english() {
        let p = IntentScorer::score("analyze the entire codebase and review its architecture");
        assert!(p.scope >= TaskScope::ProjectWide, "scope was {:?}", p.scope);
    }

    #[test]
    fn scope_single_artifact() {
        let p = IntentScorer::score("fix the bug in utils.rs");
        assert!(
            p.scope <= TaskScope::LocalContext,
            "scope was {:?}",
            p.scope
        );
    }

    #[test]
    fn scope_system_wide_architecture() {
        let p = IntentScorer::score("review the entire system architecture and microservices");
        assert_eq!(p.scope, TaskScope::SystemWide, "scope was {:?}", p.scope);
    }

    // ── Depth detection ───────────────────────────────────────────────────

    #[test]
    fn depth_exhaustive_for_project_audit() {
        let p = IntentScorer::score(
            "analiza el estado actual del proyecto, revisa la base de datos y los logs",
        );
        assert!(
            p.reasoning_depth >= ReasoningDepth::Deep,
            "depth was {:?}",
            p.reasoning_depth
        );
    }

    #[test]
    fn depth_none_for_greeting() {
        let p = IntentScorer::score("hola cómo estás");
        assert_eq!(p.reasoning_depth, ReasoningDepth::None);
    }

    #[test]
    fn depth_light_for_simple_fix() {
        let p = IntentScorer::score("fix the typo in README");
        assert!(
            p.reasoning_depth <= ReasoningDepth::Light,
            "depth was {:?}",
            p.reasoning_depth
        );
    }

    // ── Intent classification ─────────────────────────────────────────────

    #[test]
    fn intent_debugging_for_fix() {
        let p = IntentScorer::score("fix the crash in login.rs");
        assert_eq!(p.task_type, TaskType::Debugging);
    }

    #[test]
    fn intent_code_gen_create() {
        let p = IntentScorer::score("create a function to parse JSON");
        assert_eq!(p.task_type, TaskType::CodeGeneration);
    }

    #[test]
    fn intent_research_analiza() {
        let p = IntentScorer::score("analiza el rendimiento de la aplicación");
        assert_eq!(
            p.task_type,
            TaskType::Research,
            "type was {:?}",
            p.task_type
        );
    }

    #[test]
    fn intent_git_op() {
        let p = IntentScorer::score("git status and show me the diff");
        assert_eq!(p.task_type, TaskType::GitOperation);
    }

    #[test]
    fn intent_explanation_how_does() {
        let p = IntentScorer::score("explain how the cache works");
        assert_eq!(p.task_type, TaskType::Explanation);
    }

    #[test]
    fn intent_explanation_explica_spanish() {
        let p = IntentScorer::score("explica cómo funciona el sistema de plugins");
        assert_eq!(
            p.task_type,
            TaskType::Explanation,
            "type was {:?}",
            p.task_type
        );
    }

    // ── Language detection ────────────────────────────────────────────────

    #[test]
    fn language_spanish_detected() {
        let p = IntentScorer::score("analiza mi proyecto y revisa el código");
        assert_eq!(p.detected_language, QueryLanguage::Spanish);
    }

    #[test]
    fn language_english_detected() {
        let p = IntentScorer::score("create a new function and add tests for it");
        assert_eq!(p.detected_language, QueryLanguage::English);
    }

    // ── Complexity derivation ─────────────────────────────────────────────

    #[test]
    fn complexity_simple_for_greeting() {
        let p = IntentScorer::score("hola");
        assert_eq!(p.complexity, TaskComplexity::Simple);
    }

    #[test]
    fn complexity_complex_for_project_wide() {
        let p = IntentScorer::score("analiza el proyecto completo y revisa el estado actual");
        assert_eq!(
            p.complexity,
            TaskComplexity::Complex,
            "complexity was {:?}",
            p.complexity
        );
    }

    // ── Requires planning ─────────────────────────────────────────────────

    #[test]
    fn requires_planning_for_project_scope() {
        let p = IntentScorer::score("analiza el proyecto y dame un informe completo");
        assert!(p.requires_planning, "expected requires_planning=true");
    }

    #[test]
    fn no_planning_for_greeting() {
        let p = IntentScorer::score("hola");
        assert!(!p.requires_planning);
    }

    // ── Routing tier ─────────────────────────────────────────────────────

    #[test]
    fn routing_tier_fast_for_conversational() {
        let p = IntentScorer::score("hola");
        assert_eq!(p.routing_tier(), "fast");
    }

    #[test]
    fn routing_tier_deep_for_project_audit() {
        let p = IntentScorer::score("audita el proyecto completo y revisa la arquitectura");
        assert_eq!(p.routing_tier(), "deep");
    }

    // ── Suggested max_rounds ──────────────────────────────────────────────

    #[test]
    fn max_rounds_low_for_conversational() {
        let p = IntentScorer::score("hola");
        assert!(
            p.suggested_max_rounds() <= 3,
            "got {}",
            p.suggested_max_rounds()
        );
    }

    #[test]
    fn max_rounds_high_for_project_audit() {
        let p = IntentScorer::score(
            "analiza el proyecto completo, revisa todos los archivos y el estado actual",
        );
        assert!(
            p.suggested_max_rounds() >= 10,
            "got {}",
            p.suggested_max_rounds()
        );
    }

    // ── Ambiguity ─────────────────────────────────────────────────────────

    #[test]
    fn ambiguity_high_for_single_word() {
        let p = IntentScorer::score("it");
        assert!(
            p.ambiguity_score > 0.4,
            "expected high ambiguity, got {}",
            p.ambiguity_score
        );
    }

    #[test]
    fn ambiguity_low_for_specific_query() {
        let p = IntentScorer::score("fix the null pointer dereference in auth/login.rs line 42");
        assert!(
            p.ambiguity_score < 0.5,
            "expected low ambiguity, got {}",
            p.ambiguity_score
        );
    }

    // ── Backward compat ───────────────────────────────────────────────────

    #[test]
    fn backward_compat_task_type_and_complexity_present() {
        let p = IntentScorer::score("analyze the codebase structure and dependencies");
        // Must have both fields for integration with StrategySelector / UCB1.
        let _ = p.task_type;
        let _ = p.complexity;
        let _ = p.task_hash;
    }

    #[test]
    fn word_boundary_does_not_match_inside_word() {
        // "fix" must not match "prefix", "suffix", "fixture"
        assert!(!IntentScorer::contains_word("the prefix is wrong", "fix"));
        assert!(IntentScorer::contains_word("please fix the bug", "fix"));
    }

    #[test]
    fn contains_any_false_when_no_match() {
        assert!(!IntentScorer::contains_any("hello world", &["xyz", "abc"]));
    }

    #[test]
    fn confidence_is_in_range() {
        for q in &[
            "hola",
            "analyze the codebase",
            "fix bug in login.rs",
            "analiza mi proyecto actual y el estado",
            "it",
        ] {
            let p = IntentScorer::score(q);
            assert!(
                p.confidence >= 0.0 && p.confidence <= 1.0,
                "confidence {:.3} out of range for {:?}",
                p.confidence,
                q
            );
        }
    }

    #[test]
    fn is_conversational_true_for_greeting() {
        let p = IntentScorer::score("hola");
        assert!(p.is_conversational());
    }

    #[test]
    fn is_conversational_false_for_project_query() {
        let p = IntentScorer::score("analiza el proyecto actual");
        assert!(!p.is_conversational());
    }
}
