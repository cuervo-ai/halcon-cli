//! Semantic cycle detection — normalized tool-operation deduplication.
//!
//! Unlike `LoopGuard`'s `(tool_name, args_hash)` approach, this detector
//! normalizes tool names via `tool_aliases::canonicalize()`, normalizes paths,
//! and detects synonym-based exploration loops.
//!
//! Pure business logic — no I/O.

use std::collections::{HashSet, VecDeque};

/// Severity of detected cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CycleSeverity {
    None,
    Low,
    Medium,
    High,
}

impl CycleSeverity {
    /// Convert to a f32 score [0.0, 1.0] for use in feedback signals.
    pub fn as_f32(&self) -> f32 {
        match self {
            Self::None => 0.0,
            Self::Low => 0.33,
            Self::Medium => 0.66,
            Self::High => 1.0,
        }
    }
}

/// Pattern type of the detected cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CyclePattern {
    /// Same tool + same normalized path, different raw args.
    DisguisedRetry,
    /// Same tool + same semantic hash (sorted words), different raw args.
    EquivalentArgs,
    /// Search tools + synonym overlap ≥ threshold.
    ExplorationLoop,
}

/// A record of a detected cycle.
#[derive(Debug, Clone)]
pub struct CycleRecord {
    pub round: usize,
    pub pattern: CyclePattern,
    pub tool_name: String,
}

/// Normalized operation stored in the sliding window.
#[derive(Debug, Clone)]
struct NormalizedOp {
    round: usize,
    canonical_tool: String,
    normalized_path: Option<String>,
    semantic_hash: u64,
    search_words: HashSet<String>,
    is_search_tool: bool,
}

/// Semantic cycle detector with sliding window.
#[derive(Debug)]
pub struct SemanticCycleDetector {
    history: VecDeque<NormalizedOp>,
    window_size: usize,
    cycles: Vec<CycleRecord>,
    synonym_overlap_threshold: f64,
    medium_threshold: usize,
    high_threshold: usize,
}

/// Synonym groups for exploration loop detection (EN/ES bilingual).
const SYNONYM_GROUPS: &[&[&str]] = &[
    &["search", "find", "look", "query", "buscar", "encontrar"],
    &["read", "view", "show", "display", "leer", "ver", "mostrar"],
    &["list", "enumerate", "directory", "listar", "directorio"],
    &["check", "verify", "validate", "inspect", "verificar", "inspeccionar"],
    &["create", "make", "generate", "build", "crear", "generar"],
    &["delete", "remove", "drop", "clean", "eliminar", "borrar"],
    &["update", "modify", "change", "edit", "actualizar", "modificar"],
    &["test", "check", "assert", "probar", "verificar"],
    &["config", "setup", "configure", "settings", "configurar"],
    &["deploy", "publish", "release", "ship", "desplegar", "publicar"],
    &["debug", "trace", "log", "diagnose", "depurar", "diagnosticar"],
    &["analyze", "inspect", "examine", "review", "analizar", "examinar"],
];

impl SemanticCycleDetector {
    /// Create a new detector from PolicyConfig fields.
    pub fn new(
        window_size: usize,
        synonym_overlap_threshold: f64,
        medium_threshold: usize,
        high_threshold: usize,
    ) -> Self {
        Self {
            history: VecDeque::with_capacity(window_size + 1),
            window_size,
            cycles: Vec::new(),
            synonym_overlap_threshold,
            medium_threshold,
            high_threshold,
        }
    }

    /// Create from PolicyConfig.
    pub fn from_policy(policy: &halcon_core::types::PolicyConfig) -> Self {
        Self::new(
            policy.semantic_cycle_window,
            policy.cycle_synonym_overlap_threshold,
            policy.cycle_medium_threshold,
            policy.cycle_high_threshold,
        )
    }

    /// Record a tool call and check for cycles.
    ///
    /// Returns the detected cycle pattern (if any) for this specific call.
    pub fn record(
        &mut self,
        round: usize,
        tool_name: &str,
        args_text: &str,
    ) -> Option<CyclePattern> {
        let canonical = crate::repl::tool_aliases::canonicalize(tool_name).to_string();
        let normalized_path = extract_and_normalize_path(args_text);
        let semantic_hash = compute_semantic_hash(args_text);
        let search_words = extract_search_words(args_text);
        let is_search = is_search_tool(&canonical);

        let op = NormalizedOp {
            round,
            canonical_tool: canonical.clone(),
            normalized_path: normalized_path.clone(),
            semantic_hash,
            search_words: search_words.clone(),
            is_search_tool: is_search,
        };

        // Check against window BEFORE inserting
        let detected = self.detect_cycle(&op);

        // Insert and maintain window
        self.history.push_back(op);
        while self.history.len() > self.window_size {
            self.history.pop_front();
        }

        if let Some(pattern) = detected {
            self.cycles.push(CycleRecord {
                round,
                pattern,
                tool_name: canonical,
            });
        }

        detected
    }

    /// Record all tool calls from a round batch.
    ///
    /// Returns `true` if any cycle was detected in this round.
    pub fn record_round(&mut self, round: usize, tool_calls: &[(String, String)]) -> bool {
        let mut any_cycle = false;
        for (name, args) in tool_calls {
            if self.record(round, name, args).is_some() {
                any_cycle = true;
            }
        }
        any_cycle
    }

    /// Get the current cycle severity based on accumulated detections.
    pub fn severity(&self) -> CycleSeverity {
        let count = self.cycles.len();
        if count >= self.high_threshold {
            CycleSeverity::High
        } else if count >= self.medium_threshold {
            CycleSeverity::Medium
        } else if count >= 2 {
            CycleSeverity::Low
        } else {
            CycleSeverity::None
        }
    }

    /// Check if any cycle has been detected.
    pub fn has_cycle(&self) -> bool {
        !self.cycles.is_empty()
    }

    /// Get the number of detected cycles.
    pub fn cycle_count(&self) -> usize {
        self.cycles.len()
    }

    /// Reset the detector (e.g., after a replan).
    pub fn reset(&mut self) {
        self.history.clear();
        self.cycles.clear();
    }

    /// Detect cycle against existing window entries.
    fn detect_cycle(&self, op: &NormalizedOp) -> Option<CyclePattern> {
        for prev in self.history.iter().rev() {
            if prev.canonical_tool != op.canonical_tool {
                continue;
            }

            // Check DisguisedRetry: same tool + same normalized path
            if let (Some(ref prev_path), Some(ref op_path)) =
                (&prev.normalized_path, &op.normalized_path)
            {
                if prev_path == op_path && prev.semantic_hash != op.semantic_hash {
                    return Some(CyclePattern::DisguisedRetry);
                }
            }

            // Check EquivalentArgs: same semantic hash
            if prev.semantic_hash == op.semantic_hash && prev.round != op.round {
                return Some(CyclePattern::EquivalentArgs);
            }

            // Check ExplorationLoop: search tools with synonym overlap
            if prev.is_search_tool && op.is_search_tool {
                let overlap = synonym_overlap(&prev.search_words, &op.search_words);
                if overlap >= self.synonym_overlap_threshold {
                    return Some(CyclePattern::ExplorationLoop);
                }
            }
        }
        None
    }
}

// ── Normalization helpers ────────────────────────────────────────────────────

/// Extract and normalize a file path from tool args.
fn extract_and_normalize_path(args: &str) -> Option<String> {
    // Heuristic: look for path-like strings
    let path = args
        .split(|c: char| c == '"' || c == '\'' || c == ',' || c == ':')
        .find(|s| s.contains('/') || s.contains('\\') || s.ends_with(".rs") || s.ends_with(".ts"))?;

    let normalized = path
        .trim()
        .to_lowercase()
        .replace('\\', "/")
        .replace("//", "/")
        .trim_start_matches("./")
        .trim_end_matches('/')
        .to_string();

    // Resolve simple `..` patterns
    let parts: Vec<&str> = normalized.split('/').collect();
    let mut resolved = Vec::new();
    for part in parts {
        if part == ".." {
            resolved.pop();
        } else if !part.is_empty() && part != "." {
            resolved.push(part);
        }
    }

    let result = resolved.join("/");
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

/// Compute a semantic hash of args: lowercase, sort words, hash.
fn compute_semantic_hash(args: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut words: Vec<&str> = args
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .collect();
    words.sort_unstable();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for w in &words {
        w.to_lowercase().hash(&mut hasher);
    }
    hasher.finish()
}

/// Extract search-related words from args for synonym matching.
fn extract_search_words(args: &str) -> HashSet<String> {
    args.split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|w| w.len() >= 3)
        .collect()
}

/// Check if a canonical tool name is a search tool.
fn is_search_tool(canonical: &str) -> bool {
    matches!(
        canonical,
        "grep" | "glob" | "web_search" | "directory_tree"
    )
}

/// Compute synonym overlap between two word sets.
fn synonym_overlap(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }

    let mut synonym_matches = 0usize;
    let total_groups_checked = SYNONYM_GROUPS.len();

    for group in SYNONYM_GROUPS {
        let a_has = a.iter().any(|w| group.contains(&w.as_str()));
        let b_has = b.iter().any(|w| group.contains(&w.as_str()));
        if a_has && b_has {
            synonym_matches += 1;
        }
    }

    if total_groups_checked == 0 {
        return 0.0;
    }

    // Overlap = matched groups / max possible matches (min of groups that either set touches)
    let a_groups = SYNONYM_GROUPS
        .iter()
        .filter(|g| a.iter().any(|w| g.contains(&w.as_str())))
        .count();
    let b_groups = SYNONYM_GROUPS
        .iter()
        .filter(|g| b.iter().any(|w| g.contains(&w.as_str())))
        .count();
    let max_possible = a_groups.max(b_groups).max(1);

    synonym_matches as f64 / max_possible as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_detector() -> SemanticCycleDetector {
        SemanticCycleDetector::new(6, 0.60, 3, 4)
    }

    #[test]
    fn phase3_cycle_no_cycle_different_tools() {
        let mut det = test_detector();
        assert!(det.record(0, "file_read", r#"{"path": "/a.rs"}"#).is_none());
        assert!(det.record(1, "grep", r#"{"pattern": "foo"}"#).is_none());
        assert_eq!(det.severity(), CycleSeverity::None);
    }

    #[test]
    fn phase3_cycle_equivalent_args() {
        let mut det = test_detector();
        det.record(0, "file_read", r#"{"path": "/src/main.rs"}"#);
        let result = det.record(1, "read_file", r#"{"path": "/src/main.rs"}"#);
        // Both canonicalize to "file_read", same semantic hash → EquivalentArgs
        assert_eq!(result, Some(CyclePattern::EquivalentArgs));
    }

    #[test]
    fn phase3_cycle_disguised_retry_same_path() {
        let mut det = test_detector();
        det.record(0, "file_read", r#"{"path": "/src/lib.rs", "offset": 0}"#);
        let result = det.record(1, "file_read", r#"{"path": "/src/lib.rs", "offset": 100}"#);
        // Same canonical tool, same normalized path, different semantic hash → DisguisedRetry
        assert_eq!(result, Some(CyclePattern::DisguisedRetry));
    }

    #[test]
    fn phase3_cycle_exploration_loop_synonyms() {
        let mut det = test_detector();
        det.record(0, "grep", r#"search find error in logs"#);
        let result = det.record(1, "grep", r#"look buscar error in logs"#);
        // Both are search tools + synonym overlap (search≈look≈buscar, find≈encontrar)
        assert_eq!(result, Some(CyclePattern::ExplorationLoop));
    }

    #[test]
    fn phase3_cycle_severity_escalation() {
        let mut det = test_detector();
        // Force 4 cycles
        for i in 0..5 {
            det.record(i, "file_read", r#"{"path": "/same.rs"}"#);
        }
        // First call has nothing to compare. 2nd-5th detect EquivalentArgs.
        assert!(det.cycle_count() >= 4);
        assert_eq!(det.severity(), CycleSeverity::High);
    }

    #[test]
    fn phase3_cycle_severity_none_to_low() {
        let mut det = test_detector();
        assert_eq!(det.severity(), CycleSeverity::None);
        det.record(0, "file_read", r#"{"path": "/a.rs"}"#);
        det.record(1, "file_read", r#"{"path": "/a.rs"}"#); // 1 cycle
        assert_eq!(det.severity(), CycleSeverity::None); // need ≥2 for Low
        det.record(2, "file_read", r#"{"path": "/a.rs"}"#); // 2 cycles
        assert_eq!(det.severity(), CycleSeverity::Low);
    }

    #[test]
    fn phase3_cycle_severity_medium() {
        let mut det = test_detector();
        for i in 0..4 {
            det.record(i, "file_read", r#"{"path": "/a.rs"}"#);
        }
        // 3 cycles (2nd, 3rd, 4th all detect EquivalentArgs with the first)
        assert!(det.cycle_count() >= 3);
        assert!(det.severity() >= CycleSeverity::Medium);
    }

    #[test]
    fn phase3_cycle_path_normalization() {
        let p1 = extract_and_normalize_path(r#"{"path": "./src//lib.rs"}"#);
        let p2 = extract_and_normalize_path(r#"{"path": "src/lib.rs"}"#);
        assert_eq!(p1, p2);
    }

    #[test]
    fn phase3_cycle_path_dotdot_resolution() {
        let p = extract_and_normalize_path(r#"{"path": "src/foo/../lib.rs"}"#);
        assert_eq!(p, Some("src/lib.rs".to_string()));
    }

    #[test]
    fn phase3_cycle_reset_clears_state() {
        let mut det = test_detector();
        det.record(0, "file_read", r#"{"path": "/a.rs"}"#);
        det.record(1, "file_read", r#"{"path": "/a.rs"}"#);
        assert!(det.has_cycle());
        det.reset();
        assert!(!det.has_cycle());
        assert_eq!(det.cycle_count(), 0);
        assert_eq!(det.severity(), CycleSeverity::None);
    }

    #[test]
    fn phase3_cycle_window_eviction() {
        let mut det = SemanticCycleDetector::new(2, 0.60, 3, 4);
        det.record(0, "file_read", r#"{"path": "/a.rs"}"#);
        det.record(1, "grep", r#"{"pattern": "foo"}"#);
        det.record(2, "bash", r#"ls -la"#);
        // Window=2, so /a.rs should be evicted
        let result = det.record(3, "file_read", r#"{"path": "/a.rs"}"#);
        assert!(result.is_none(), "evicted entry should not trigger cycle");
    }

    #[test]
    fn phase3_cycle_record_round_batch() {
        let mut det = test_detector();
        let batch = vec![
            ("file_read".to_string(), r#"{"path": "/a.rs"}"#.to_string()),
        ];
        det.record_round(0, &batch);
        let detected = det.record_round(1, &batch);
        assert!(detected);
    }

    #[test]
    fn phase3_cycle_semantic_hash_order_invariant() {
        let h1 = compute_semantic_hash("hello world foo");
        let h2 = compute_semantic_hash("foo world hello");
        assert_eq!(h1, h2, "word order should not matter");
    }

    #[test]
    fn phase3_cycle_severity_as_f32() {
        assert!((CycleSeverity::None.as_f32()).abs() < f32::EPSILON);
        assert!((CycleSeverity::Low.as_f32() - 0.33).abs() < f32::EPSILON);
        assert!((CycleSeverity::Medium.as_f32() - 0.66).abs() < f32::EPSILON);
        assert!((CycleSeverity::High.as_f32() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn phase3_cycle_from_policy() {
        let policy = halcon_core::types::PolicyConfig::default();
        let det = SemanticCycleDetector::from_policy(&policy);
        assert_eq!(det.window_size, 6);
        assert!((det.synonym_overlap_threshold - 0.60).abs() < f64::EPSILON);
        assert_eq!(det.medium_threshold, 3);
        assert_eq!(det.high_threshold, 4);
    }

    #[test]
    fn phase3_cycle_no_false_positive_different_paths() {
        let mut det = test_detector();
        det.record(0, "file_read", r#"{"path": "/src/a.rs"}"#);
        let result = det.record(1, "file_read", r#"{"path": "/src/b.rs"}"#);
        // Different normalized paths AND different semantic hash → no cycle
        assert!(result.is_none());
    }

    #[test]
    fn phase3_cycle_synonym_overlap_computation() {
        let a: HashSet<String> = ["search", "error", "logs"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let b: HashSet<String> = ["find", "error", "logs"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let overlap = synonym_overlap(&a, &b);
        // "search" and "find" are in the same synonym group
        assert!(overlap > 0.0);
    }
}
