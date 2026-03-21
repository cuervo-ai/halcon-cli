//! # Goal Specification Engine
//!
//! Parses a free-text user intent into a typed `GoalSpec` with machine-checkable
//! `VerifiableCriterion` entries. The agent loop exits ONLY when the
//! `GoalVerificationEngine` confirms that confidence >= threshold.
//!
//! ## Design Contract
//!
//! - `GoalSpec` is immutable after construction.
//! - `VerifiableCriterion::verify()` is a pure function: same output for same state.
//! - `GoalVerificationEngine::evaluate()` accumulates evidence across all criteria
//!   and returns a scalar `ConfidenceScore` in [0.0, 1.0].
//! - The agent loop polls `evaluate()` after every tool batch; the loop controller
//!   decides when to halt based on the returned score.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ── Confidence Score ─────────────────────────────────────────────────────────

/// Scalar confidence that a goal has been achieved. Range: [0.0, 1.0].
///
/// 0.0 = no evidence the goal was achieved.
/// 1.0 = all verifiable criteria satisfied with maximum confidence.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ConfidenceScore(f32);

impl ConfidenceScore {
    pub const ZERO: Self = Self(0.0);
    pub const FULL: Self = Self(1.0);

    /// Construct a confidence score, clamping to [0.0, 1.0].
    pub fn new(v: f32) -> Self {
        Self(v.clamp(0.0, 1.0))
    }

    pub fn value(self) -> f32 {
        self.0
    }

    pub fn meets(self, threshold: f32) -> bool {
        self.0 >= threshold
    }
}

impl fmt::Display for ConfidenceScore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.1}%", self.0 * 100.0)
    }
}

// ── Criterion Types ───────────────────────────────────────────────────────────

/// The kind of evidence a criterion requires.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CriterionKind {
    /// A regex pattern that must appear in at least one tool output.
    PatternMatch { pattern: String },

    /// A specific tool must have been called successfully at least `min_calls` times.
    ToolInvoked { tool_name: String, min_calls: usize },

    /// A tool output must contain a JSON field with the given path and optional value.
    JsonField {
        field_path: String,
        expected_value: Option<serde_json::Value>,
    },

    /// The accumulated assistant text must contain all of these keywords.
    KeywordPresence { keywords: Vec<String> },

    /// An LLM-evaluated criterion: the verifier sends tool outputs to a small model
    /// and asks whether the stated requirement is satisfied.
    /// Use sparingly — has latency overhead.
    LlmJudge { requirement: String },

    /// A compound criterion: all sub-criteria must be satisfied.
    All { sub: Vec<VerifiableCriterion> },

    /// A compound criterion: at least one sub-criterion must be satisfied.
    Any { sub: Vec<VerifiableCriterion> },

    /// The external exit code of a tool invocation must be 0.
    ExitCodeZero { tool_name: String },

    /// A file at the given path must exist after execution.
    FileExists { path_pattern: String },
}

/// A single verifiable criterion that contributes to goal satisfaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifiableCriterion {
    /// Human-readable description (shown in UI).
    pub description: String,
    /// Weight of this criterion in the overall confidence calculation. Weights are
    /// normalised so they sum to 1.0 across all criteria in a `GoalSpec`.
    pub weight: f32,
    /// The actual check to perform.
    pub kind: CriterionKind,
    /// Minimum confidence for this criterion to be counted as satisfied.
    pub threshold: f32,
}

impl VerifiableCriterion {
    /// Construct a pattern-match criterion.
    pub fn pattern(description: impl Into<String>, pattern: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            weight: 1.0,
            kind: CriterionKind::PatternMatch {
                pattern: pattern.into(),
            },
            threshold: 0.8,
        }
    }

    /// Construct a tool-invoked criterion.
    pub fn tool_invoked(description: impl Into<String>, tool_name: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            weight: 1.0,
            kind: CriterionKind::ToolInvoked {
                tool_name: tool_name.into(),
                min_calls: 1,
            },
            threshold: 1.0,
        }
    }

    /// Construct an LLM-judge criterion.
    pub fn llm_judge(requirement: impl Into<String>) -> Self {
        let req = requirement.into();
        Self {
            description: req.clone(),
            weight: 1.0,
            kind: CriterionKind::LlmJudge { requirement: req },
            threshold: 0.7,
        }
    }
}

// ── Verification Result ───────────────────────────────────────────────────────

/// Result of evaluating one criterion against collected evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    pub criterion: String,
    pub satisfied: bool,
    pub confidence: ConfidenceScore,
    /// Evidence snippets that led to this result (for audit / display).
    pub evidence: Vec<String>,
    /// Gaps: what evidence is missing to satisfy this criterion.
    pub gaps: Vec<String>,
}

// ── Evidence Accumulator ──────────────────────────────────────────────────────

/// Accumulated evidence from tool executions within one agent session.
///
/// The verifier checks criteria against this accumulated state — not against
/// individual tool outputs. This ensures criteria that require multiple tool
/// calls (e.g., "file exists after creation") are correctly evaluated.
#[derive(Debug, Clone, Default)]
pub struct Evidence {
    /// Raw text outputs from each tool invocation, keyed by tool name.
    pub tool_outputs: Vec<(String, String)>,
    /// Tools that were called successfully at least once, with call counts.
    pub tools_called: std::collections::HashMap<String, usize>,
    /// Accumulated assistant response text across all rounds.
    pub assistant_text: String,
    /// Structured JSON outputs where available.
    pub json_outputs: Vec<serde_json::Value>,
    /// Files known to exist (from file_read / file_inspect successes).
    pub known_files: Vec<String>,
}

impl Evidence {
    pub fn record_tool_success(&mut self, tool_name: &str, output: &str) {
        *self.tools_called.entry(tool_name.to_string()).or_insert(0) += 1;
        self.tool_outputs
            .push((tool_name.to_string(), output.to_string()));

        // Attempt JSON parse for structured output.
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(output) {
            self.json_outputs.push(v);
        }
    }

    pub fn record_assistant_text(&mut self, text: &str) {
        if !text.is_empty() {
            self.assistant_text.push_str(text);
            self.assistant_text.push('\n');
        }
    }
}

// ── Criterion Evaluator ───────────────────────────────────────────────────────

/// Errors from goal verification.
#[derive(Debug, Error)]
pub enum GoalError {
    #[error("invalid pattern: {0}")]
    InvalidPattern(#[from] regex::Error),
    #[error("LLM judge unavailable: {0}")]
    JudgeUnavailable(String),
    #[error("JSON path error: {0}")]
    JsonPath(String),
}

/// Evaluates a single `VerifiableCriterion` against accumulated `Evidence`.
///
/// This is synchronous for all deterministic criteria (PatternMatch, ToolInvoked,
/// KeywordPresence, FileExists). LlmJudge variants require an async call and are
/// handled by the outer `GoalVerificationEngine`.
pub struct CriterionEvaluator;

impl CriterionEvaluator {
    /// Synchronous evaluation for all non-LLM criteria.
    /// Returns `None` if the criterion requires async LLM evaluation.
    pub fn evaluate_sync(
        criterion: &VerifiableCriterion,
        evidence: &Evidence,
    ) -> Result<Option<VerificationResult>, GoalError> {
        match &criterion.kind {
            CriterionKind::PatternMatch { pattern } => {
                let re = regex::Regex::new(pattern)?;
                let mut found_in = Vec::new();
                for (tool, output) in &evidence.tool_outputs {
                    if re.is_match(output) {
                        found_in.push(format!("Found in {tool}"));
                    }
                }
                // Also check assistant text
                if re.is_match(&evidence.assistant_text) {
                    found_in.push("Found in assistant response".to_string());
                }
                let satisfied = !found_in.is_empty();
                Ok(Some(VerificationResult {
                    criterion: criterion.description.clone(),
                    satisfied,
                    confidence: if satisfied {
                        ConfidenceScore::FULL
                    } else {
                        ConfidenceScore::ZERO
                    },
                    evidence: found_in,
                    gaps: if satisfied {
                        vec![]
                    } else {
                        vec![format!("Pattern `{pattern}` not found in any tool output")]
                    },
                }))
            }

            CriterionKind::ToolInvoked {
                tool_name,
                min_calls,
            } => {
                let count = evidence
                    .tools_called
                    .get(tool_name.as_str())
                    .copied()
                    .unwrap_or(0);
                let satisfied = count >= *min_calls;
                Ok(Some(VerificationResult {
                    criterion: criterion.description.clone(),
                    satisfied,
                    confidence: if satisfied {
                        ConfidenceScore::FULL
                    } else {
                        ConfidenceScore::new(count as f32 / *min_calls as f32)
                    },
                    evidence: if satisfied {
                        vec![format!("`{tool_name}` called {count} time(s)")]
                    } else {
                        vec![]
                    },
                    gaps: if satisfied {
                        vec![]
                    } else {
                        vec![format!(
                            "`{tool_name}` needs {min_calls} call(s), got {count}"
                        )]
                    },
                }))
            }

            CriterionKind::KeywordPresence { keywords } => {
                let text_lower = evidence.assistant_text.to_lowercase();
                let mut found = Vec::new();
                let mut missing = Vec::new();
                for kw in keywords {
                    if text_lower.contains(&kw.to_lowercase()) {
                        found.push(kw.clone());
                    } else {
                        missing.push(kw.clone());
                    }
                }
                let ratio = found.len() as f32 / keywords.len().max(1) as f32;
                Ok(Some(VerificationResult {
                    criterion: criterion.description.clone(),
                    satisfied: missing.is_empty(),
                    confidence: ConfidenceScore::new(ratio),
                    evidence: found
                        .iter()
                        .map(|k| format!("keyword `{k}` present"))
                        .collect(),
                    gaps: missing
                        .iter()
                        .map(|k| format!("keyword `{k}` missing"))
                        .collect(),
                }))
            }

            CriterionKind::FileExists { path_pattern } => {
                let re = regex::Regex::new(path_pattern)?;
                let found: Vec<_> = evidence
                    .known_files
                    .iter()
                    .filter(|f| re.is_match(f))
                    .cloned()
                    .collect();
                let satisfied = !found.is_empty();
                Ok(Some(VerificationResult {
                    criterion: criterion.description.clone(),
                    satisfied,
                    confidence: if satisfied {
                        ConfidenceScore::FULL
                    } else {
                        ConfidenceScore::ZERO
                    },
                    evidence: found.clone(),
                    gaps: if satisfied {
                        vec![]
                    } else {
                        vec![format!("No file matching `{path_pattern}` was observed")]
                    },
                }))
            }

            CriterionKind::ExitCodeZero { tool_name } => {
                let called = evidence.tools_called.contains_key(tool_name.as_str());
                // We infer exit-code-zero from the tool being in `tools_called` (successes only).
                Ok(Some(VerificationResult {
                    criterion: criterion.description.clone(),
                    satisfied: called,
                    confidence: if called {
                        ConfidenceScore::FULL
                    } else {
                        ConfidenceScore::ZERO
                    },
                    evidence: if called {
                        vec![format!("`{tool_name}` succeeded")]
                    } else {
                        vec![]
                    },
                    gaps: if called {
                        vec![]
                    } else {
                        vec![format!("`{tool_name}` has not been called or failed")]
                    },
                }))
            }

            CriterionKind::JsonField {
                field_path,
                expected_value,
            } => {
                let parts: Vec<&str> = field_path.split('.').collect();
                let mut found_values = Vec::new();
                for json in &evidence.json_outputs {
                    if let Some(v) = Self::json_traverse(json, &parts) {
                        if let Some(expected) = expected_value {
                            if *v == *expected {
                                found_values.push(v.to_string());
                            }
                        } else {
                            found_values.push(v.to_string());
                        }
                    }
                }
                let satisfied = !found_values.is_empty();
                Ok(Some(VerificationResult {
                    criterion: criterion.description.clone(),
                    satisfied,
                    confidence: if satisfied {
                        ConfidenceScore::FULL
                    } else {
                        ConfidenceScore::ZERO
                    },
                    evidence: found_values,
                    gaps: if satisfied {
                        vec![]
                    } else {
                        vec![format!("JSON field `{field_path}` not found or mismatched")]
                    },
                }))
            }

            // Async criteria — caller must handle.
            CriterionKind::LlmJudge { .. }
            | CriterionKind::All { .. }
            | CriterionKind::Any { .. } => Ok(None),
        }
    }

    fn json_traverse<'v>(
        value: &'v serde_json::Value,
        path: &[&str],
    ) -> Option<&'v serde_json::Value> {
        if path.is_empty() {
            return Some(value);
        }
        match value {
            serde_json::Value::Object(map) => map
                .get(path[0])
                .and_then(|v| Self::json_traverse(v, &path[1..])),
            _ => None,
        }
    }
}

// ── Goal Specification ────────────────────────────────────────────────────────

/// A fully specified, machine-checkable goal.
///
/// Created from a user message by `GoalSpecParser`. Immutable after construction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalSpec {
    /// UUID for this goal (used in telemetry and memory).
    pub id: uuid::Uuid,
    /// The original user intent text.
    pub intent: String,
    /// Verifiable criteria that together define goal satisfaction.
    /// Each criterion has a weight; weights are normalised at construction.
    pub criteria: Vec<VerifiableCriterion>,
    /// Minimum aggregate confidence to declare the goal achieved.
    pub completion_threshold: f32,
    /// Maximum agent loop rounds allowed (derived from task complexity).
    pub max_rounds: usize,
    /// Whether the goal is time-sensitive (influences model selection).
    pub latency_sensitive: bool,
}

impl GoalSpec {
    /// Construct a GoalSpec, normalising criterion weights.
    pub fn new(
        intent: impl Into<String>,
        mut criteria: Vec<VerifiableCriterion>,
        completion_threshold: f32,
        max_rounds: usize,
    ) -> Self {
        // Normalise weights so they sum to 1.0.
        let total_weight: f32 = criteria.iter().map(|c| c.weight).sum();
        if total_weight > 0.0 {
            for c in &mut criteria {
                c.weight /= total_weight;
            }
        }
        Self {
            id: uuid::Uuid::new_v4(),
            intent: intent.into(),
            criteria,
            completion_threshold: completion_threshold.clamp(0.0, 1.0),
            max_rounds,
            latency_sensitive: false,
        }
    }

    /// A minimal GoalSpec with no verifiable criteria (conversational tasks).
    /// Exits after the model's first non-tool response.
    pub fn conversational(intent: impl Into<String>) -> Self {
        Self::new(intent, vec![], 0.0, 3)
    }
}

// ── Goal Verification Engine ──────────────────────────────────────────────────

/// Evaluates accumulated evidence against the GoalSpec criteria.
///
/// Called after every tool batch inside the agent loop. The loop controller
/// calls `evaluate()` and breaks when the returned score meets the threshold.
#[derive(Debug)]
pub struct GoalVerificationEngine {
    spec: GoalSpec,
    /// History of confidence scores per round (for trend analysis).
    history: Vec<ConfidenceScore>,
    /// Cached results from the previous evaluation round.
    last_results: Vec<VerificationResult>,
}

impl GoalVerificationEngine {
    pub fn new(spec: GoalSpec) -> Self {
        Self {
            spec,
            history: Vec::new(),
            last_results: Vec::new(),
        }
    }

    /// Synchronous evaluation against all non-LLM criteria.
    ///
    /// Returns the weighted aggregate confidence. Async (LLM-judge) criteria
    /// are skipped and assumed to have the confidence from the previous round.
    pub fn evaluate(&mut self, evidence: &Evidence) -> ConfidenceScore {
        if self.spec.criteria.is_empty() {
            // Conversational goal — trivially satisfied by any assistant response.
            let satisfied = !evidence.assistant_text.is_empty();
            return if satisfied {
                ConfidenceScore::FULL
            } else {
                ConfidenceScore::ZERO
            };
        }

        let mut weighted_sum = 0.0f32;
        let mut results = Vec::new();

        for criterion in &self.spec.criteria {
            let result = match CriterionEvaluator::evaluate_sync(criterion, evidence) {
                Ok(Some(r)) => r,
                Ok(None) => {
                    // LLM-judge: carry forward previous confidence or 0.
                    let prev_conf = self
                        .last_results
                        .iter()
                        .find(|r| r.criterion == criterion.description)
                        .map(|r| r.confidence)
                        .unwrap_or(ConfidenceScore::ZERO);
                    VerificationResult {
                        criterion: criterion.description.clone(),
                        satisfied: prev_conf.meets(criterion.threshold),
                        confidence: prev_conf,
                        evidence: vec!["[Pending LLM evaluation]".to_string()],
                        gaps: vec![],
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, criterion = %criterion.description, "Criterion evaluation error");
                    VerificationResult {
                        criterion: criterion.description.clone(),
                        satisfied: false,
                        confidence: ConfidenceScore::ZERO,
                        evidence: vec![],
                        gaps: vec![format!("Evaluation error: {e}")],
                    }
                }
            };

            weighted_sum += criterion.weight * result.confidence.value();
            results.push(result);
        }

        let score = ConfidenceScore::new(weighted_sum);
        self.history.push(score);
        self.last_results = results;

        tracing::debug!(
            score = score.value(),
            threshold = self.spec.completion_threshold,
            criteria = self.spec.criteria.len(),
            "GoalVerificationEngine::evaluate"
        );

        score
    }

    /// Whether the goal is achieved at the configured threshold.
    pub fn is_achieved(&self) -> bool {
        self.history
            .last()
            .map(|s| s.meets(self.spec.completion_threshold))
            .unwrap_or(false)
    }

    /// Per-criterion results from the most recent evaluation (for UI display).
    pub fn last_results(&self) -> &[VerificationResult] {
        &self.last_results
    }

    /// Gaps from the most recent evaluation — used for replan directive injection.
    pub fn current_gaps(&self) -> Vec<String> {
        self.last_results
            .iter()
            .flat_map(|r| r.gaps.iter().cloned())
            .collect()
    }

    /// Confidence trend over last N rounds (positive = improving).
    pub fn trend(&self, window: usize) -> f32 {
        if self.history.len() < 2 {
            return 0.0;
        }
        let recent: Vec<f32> = self
            .history
            .iter()
            .rev()
            .take(window)
            .map(|s| s.value())
            .collect();
        if recent.len() < 2 {
            return 0.0;
        }
        recent[0] - recent[recent.len() - 1]
    }

    pub fn spec(&self) -> &GoalSpec {
        &self.spec
    }
}

// ── Goal Spec Parser ──────────────────────────────────────────────────────────

/// Parses a user message into a `GoalSpec`.
///
/// Uses the existing `IntentScorer` for complexity estimation, then maps
/// the detected domain to known criterion templates.
///
/// For tasks with no recognisable structure, falls back to a single
/// `LlmJudge` criterion with the original intent as the requirement.
pub struct GoalSpecParser {
    pub completion_threshold: f32,
}

impl Default for GoalSpecParser {
    fn default() -> Self {
        Self {
            completion_threshold: 0.75,
        }
    }
}

impl GoalSpecParser {
    /// Parse a user message into a GoalSpec.
    ///
    /// The domain detector maps well-known task patterns to structured criteria.
    /// Unknown tasks get a fallback LlmJudge criterion.
    pub fn parse(&self, user_message: &str) -> GoalSpec {
        let lower = user_message.to_lowercase();
        let mut criteria = Vec::new();
        let mut max_rounds = 20usize;

        // ── Domain: Security / Credentials ─────────────────────────────────
        if lower.contains("credential")
            || lower.contains("secret")
            || lower.contains("api key")
            || lower.contains("password")
            || lower.contains("token")
            || lower.contains("leak")
            || lower.contains("expose")
        {
            criteria.push(VerifiableCriterion {
                description: "secret_scan tool invoked".to_string(),
                weight: 2.0,
                kind: CriterionKind::ToolInvoked {
                    tool_name: "secret_scan".to_string(),
                    min_calls: 1,
                },
                threshold: 1.0,
            });
            criteria.push(VerifiableCriterion {
                description: "Report lists files scanned".to_string(),
                weight: 1.5,
                kind: CriterionKind::PatternMatch {
                    // Typical secret_scan output contains file paths
                    pattern: r"(?i)(scanned|found|detected|file|path)".to_string(),
                },
                threshold: 0.8,
            });
            criteria.push(VerifiableCriterion {
                description: "Response addresses credential findings".to_string(),
                weight: 1.0,
                kind: CriterionKind::LlmJudge {
                    requirement: format!(
                        "The response must report whether any credentials/secrets were found, \
                         listing specific files if any were detected. Original request: {user_message}"
                    ),
                },
                threshold: 0.7,
            });
            max_rounds = 15;
        }
        // ── Domain: Code Execution / Testing ────────────────────────────────
        else if lower.contains("test")
            || lower.contains("compile")
            || lower.contains("build")
            || lower.contains("run")
                && (lower.contains("cargo") || lower.contains("npm") || lower.contains("make"))
        {
            criteria.push(VerifiableCriterion {
                description: "Build/test tool invoked".to_string(),
                weight: 2.0,
                kind: CriterionKind::Any {
                    sub: vec![
                        VerifiableCriterion::tool_invoked("bash invoked", "bash"),
                        VerifiableCriterion::tool_invoked("test_run invoked", "test_run"),
                    ],
                },
                threshold: 1.0,
            });
            criteria.push(VerifiableCriterion {
                description: "Exit code zero or test pass output".to_string(),
                weight: 2.0,
                kind: CriterionKind::PatternMatch {
                    pattern: r"(?i)(test result|passed|ok|success|0 failed|all tests)".to_string(),
                },
                threshold: 0.8,
            });
            max_rounds = 12;
        }
        // ── Domain: File Operations ──────────────────────────────────────────
        else if lower.contains("create")
            || lower.contains("write")
            || lower.contains("generate")
            || lower.contains("edit")
            || lower.contains("modify")
        {
            criteria.push(VerifiableCriterion {
                description: "File write tool invoked".to_string(),
                weight: 2.0,
                kind: CriterionKind::Any {
                    sub: vec![
                        VerifiableCriterion::tool_invoked("file_write invoked", "file_write"),
                        VerifiableCriterion::tool_invoked("file_edit invoked", "file_edit"),
                    ],
                },
                threshold: 1.0,
            });
            criteria.push(VerifiableCriterion {
                description: "Response confirms file created/modified".to_string(),
                weight: 1.0,
                kind: CriterionKind::LlmJudge {
                    requirement: format!(
                        "The response must confirm that the requested file was created or \
                         modified successfully. Request: {user_message}"
                    ),
                },
                threshold: 0.7,
            });
            max_rounds = 10;
        }
        // ── Domain: Analysis / Explanation ──────────────────────────────────
        else if lower.contains("explain")
            || lower.contains("analyze")
            || lower.contains("analyse")
            || lower.contains("describe")
            || lower.contains("audit")
            || lower.contains("review")
        {
            criteria.push(VerifiableCriterion {
                description: "Sufficient information gathered (file reads)".to_string(),
                weight: 1.5,
                kind: CriterionKind::Any {
                    sub: vec![
                        VerifiableCriterion::tool_invoked("file_read used", "file_read"),
                        VerifiableCriterion::tool_invoked("grep used", "grep"),
                        VerifiableCriterion::tool_invoked("bash used", "bash"),
                    ],
                },
                threshold: 1.0,
            });
            criteria.push(VerifiableCriterion {
                description: "Response provides substantive analysis".to_string(),
                weight: 2.0,
                kind: CriterionKind::LlmJudge {
                    requirement: format!(
                        "The response must provide a substantive, specific analysis that \
                         directly addresses the user's request. Request: {user_message}"
                    ),
                },
                threshold: 0.7,
            });
            max_rounds = 20;
        }

        // ── Fallback: LLM-judged generic goal ──────────────────────────────
        if criteria.is_empty() {
            criteria.push(VerifiableCriterion::llm_judge(format!(
                "The agent's response fully and specifically addresses: {user_message}. \
                 It should not be vague, incomplete, or off-topic."
            )));
            max_rounds = 15;
        }

        GoalSpec::new(
            user_message,
            criteria,
            self.completion_threshold,
            max_rounds,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_credential_goal_has_secret_scan_criterion() {
        let parser = GoalSpecParser::default();
        let spec = parser.parse("search for exposed credentials in the codebase");
        let has_secret_scan = spec.criteria.iter().any(|c| {
            matches!(&c.kind, CriterionKind::ToolInvoked { tool_name, .. } if tool_name == "secret_scan")
        });
        assert!(has_secret_scan, "Credential goal must require secret_scan");
    }

    #[test]
    fn weights_normalised_to_one() {
        let parser = GoalSpecParser::default();
        let spec = parser.parse("find credentials");
        let total: f32 = spec.criteria.iter().map(|c| c.weight).sum();
        assert!(
            (total - 1.0).abs() < 1e-4,
            "Weights must sum to 1.0, got {total}"
        );
    }

    #[test]
    fn pattern_match_criterion_finds_match() {
        let mut evidence = Evidence::default();
        evidence.record_tool_success(
            "secret_scan",
            "Found 2 secrets: .env:3 API_KEY=sk-abc, config.yaml:12 password=hunter2",
        );
        let criterion = VerifiableCriterion {
            description: "Pattern found".to_string(),
            weight: 1.0,
            kind: CriterionKind::PatternMatch {
                pattern: r"(?i)(found|detected)\s+\d+\s+secret".to_string(),
            },
            threshold: 0.8,
        };
        let result = CriterionEvaluator::evaluate_sync(&criterion, &evidence)
            .unwrap()
            .unwrap();
        assert!(result.satisfied);
        assert!(result.confidence.value() > 0.9);
    }

    #[test]
    fn confidence_score_clamps_to_zero_one() {
        assert_eq!(ConfidenceScore::new(-1.0).value(), 0.0);
        assert_eq!(ConfidenceScore::new(2.0).value(), 1.0);
        assert_eq!(ConfidenceScore::new(0.5).value(), 0.5);
    }

    #[test]
    fn goal_verification_engine_detects_achieved() {
        let _parser = GoalSpecParser::default();
        // Build a simple spec with only a ToolInvoked criterion
        let spec = GoalSpec::new(
            "invoke grep",
            vec![VerifiableCriterion {
                description: "grep invoked".to_string(),
                weight: 1.0,
                kind: CriterionKind::ToolInvoked {
                    tool_name: "grep".to_string(),
                    min_calls: 1,
                },
                threshold: 1.0,
            }],
            0.9,
            10,
        );
        let mut engine = GoalVerificationEngine::new(spec);
        let mut evidence = Evidence::default();
        evidence.record_tool_success("grep", "found: foo.rs:42 secret=...");
        let score = engine.evaluate(&evidence);
        assert!(
            score.meets(0.9),
            "Score should meet 0.9 threshold, got {score}"
        );
        assert!(engine.is_achieved());
    }

    #[test]
    fn trend_positive_when_improving() {
        let spec = GoalSpec::conversational("hello");
        let mut engine = GoalVerificationEngine::new(spec);
        // Simulate improving history
        engine.history.push(ConfidenceScore::new(0.3));
        engine.history.push(ConfidenceScore::new(0.5));
        engine.history.push(ConfidenceScore::new(0.8));
        assert!(engine.trend(3) > 0.0);
    }
}
