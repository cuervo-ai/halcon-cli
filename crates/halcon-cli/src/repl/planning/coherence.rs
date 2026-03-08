//! Plan Coherence Checker — detects goal drift after plan regeneration.
//!
//! Closes **G6** (no PlanCoherenceCheck): after every structural replan, the checker
//! computes keyword-overlap between the original user goal and the new plan's step
//! descriptions. If drift is above threshold, it signals the agent loop to inject a
//! goal-restoration message so the model realigns.

use std::collections::HashSet;

use halcon_core::traits::ExecutionPlan;

use super::text_utils::extract_keywords;

/// Threshold above which plan drift is considered critical (1.0 - jaccard > threshold).
const DEFAULT_DRIFT_THRESHOLD: f32 = 0.70;

/// Report produced by [`PlanCoherenceChecker::check`].
#[derive(Debug, Clone)]
pub struct CoherenceReport {
    /// Jaccard similarity between goal keywords and plan keywords [0, 1].
    pub semantic_overlap: f32,
    /// `1.0 - semantic_overlap` — how far the plan has drifted from the goal.
    pub drift_score: f32,
    /// True when `drift_score > drift_threshold`.
    pub drift_detected: bool,
    /// Goal keywords that do NOT appear in any plan step description.
    pub missing_keywords: Vec<String>,
}

/// Checks new plans for semantic drift from the original user goal.
///
/// Instantiated once before the agent loop and reused across all replan events.
/// Keyword extraction is done at construction time (O(G) where G = goal word count),
/// and each `check()` call is O(P) where P = total words across all plan steps.
pub struct PlanCoherenceChecker {
    goal_keywords: HashSet<String>,
    original_goal: String,
    drift_threshold: f32,
}

impl PlanCoherenceChecker {
    /// Create a checker with default drift threshold (0.70).
    pub fn new(goal: &str) -> Self {
        Self::new_with_threshold(goal, DEFAULT_DRIFT_THRESHOLD)
    }

    /// Create a checker with a custom drift threshold.
    pub fn new_with_threshold(goal: &str, threshold: f32) -> Self {
        Self {
            goal_keywords: extract_keywords(goal),
            original_goal: goal.to_string(),
            drift_threshold: threshold,
        }
    }

    /// Check a new execution plan for semantic drift from the original goal.
    ///
    /// Returns a [`CoherenceReport`] with:
    /// - `semantic_overlap`: Jaccard similarity [0, 1]
    /// - `drift_score`: 1.0 - overlap
    /// - `drift_detected`: whether drift_score exceeds the threshold
    /// - `missing_keywords`: keywords from the goal absent from the plan
    pub fn check(&self, plan: &ExecutionPlan) -> CoherenceReport {
        // Collect all keywords from all step descriptions.
        let mut plan_keywords: HashSet<String> = HashSet::new();
        for step in &plan.steps {
            plan_keywords.extend(extract_keywords(&step.description));
            // Also include the goal field of the plan itself.
        }
        plan_keywords.extend(extract_keywords(&plan.goal));

        // Jaccard similarity: |A ∩ B| / |A ∪ B|.
        let intersection = self.goal_keywords.intersection(&plan_keywords).count();
        let union = self.goal_keywords.union(&plan_keywords).count();

        let semantic_overlap = if union == 0 {
            1.0f32 // both empty — trivially coherent
        } else {
            intersection as f32 / union as f32
        };

        let drift_score = 1.0 - semantic_overlap;
        let drift_detected = drift_score > self.drift_threshold;

        // Find which goal keywords are absent from the plan.
        let missing_keywords: Vec<String> = self
            .goal_keywords
            .iter()
            .filter(|kw| !plan_keywords.contains(*kw))
            .cloned()
            .collect();

        CoherenceReport {
            semantic_overlap,
            drift_score,
            drift_detected,
            missing_keywords,
        }
    }

    /// The original goal string (for display and logging).
    pub fn original_goal(&self) -> &str {
        &self.original_goal
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::traits::{ExecutionPlan, PlanStep};
    use uuid::Uuid;

    fn make_plan(goal: &str, steps: &[&str]) -> ExecutionPlan {
        ExecutionPlan {
            plan_id: Uuid::new_v4(),
            goal: goal.to_string(),
            steps: steps
                .iter()
                .map(|desc| PlanStep {
                    step_id: Uuid::new_v4(),
                    description: desc.to_string(),
                    tool_name: None,
                    parallel: false,
                    confidence: 0.8,
                    expected_args: None,
                    outcome: None,
                })
                .collect(),
            replan_count: 0,
            parent_plan_id: None,
            requires_confirmation: false,
            ..Default::default()
        }
    }

    #[test]
    fn high_overlap_gives_low_drift() {
        let checker = PlanCoherenceChecker::new("implement file reading with error handling");
        let plan = make_plan(
            "implement file reading",
            &[
                "implement file reading function",
                "add error handling for file operations",
            ],
        );
        let report = checker.check(&plan);
        // "implement", "file", "reading", "error", "handling" should all be present.
        assert!(report.semantic_overlap > 0.3, "overlap={}", report.semantic_overlap);
        assert!(!report.drift_detected, "unexpected drift: score={}", report.drift_score);
    }

    #[test]
    fn low_overlap_gives_high_drift() {
        let checker = PlanCoherenceChecker::new("implement file reading with error handling");
        // Plan is about something completely different.
        let plan = make_plan(
            "database migration",
            &[
                "connect to postgres database",
                "run migration scripts",
                "verify table structure",
            ],
        );
        let report = checker.check(&plan);
        assert!(report.drift_detected, "drift should be detected: score={}", report.drift_score);
        assert!(report.drift_score > 0.5);
    }

    #[test]
    fn empty_plan_is_flagged() {
        let checker = PlanCoherenceChecker::new("implement file reading");
        let plan = make_plan("", &[]);
        let report = checker.check(&plan);
        // Zero plan keywords means union == goal_keywords.len() and intersection = 0.
        assert!(report.drift_detected);
        assert!((report.drift_score - 1.0).abs() < 0.01);
    }

    #[test]
    fn single_step_plan_partial_overlap() {
        let checker = PlanCoherenceChecker::new("read and parse configuration files");
        let plan = make_plan("parse config", &["read configuration file"]);
        let report = checker.check(&plan);
        // "read", "parse", "configuration", "files" — at least some overlap.
        assert!(report.semantic_overlap > 0.0);
    }

    #[test]
    fn custom_threshold_can_be_set() {
        // With threshold = 0.0, ANY drift is detected.
        let checker = PlanCoherenceChecker::new_with_threshold("implement feature", 0.0);
        // Plan that's similar but not identical.
        let plan = make_plan("implement features", &["add feature implementation"]);
        let report = checker.check(&plan);
        // drift_score > 0.0 → drift_detected.
        assert!(report.drift_detected);
    }

    #[test]
    fn missing_keywords_listed_correctly() {
        let checker = PlanCoherenceChecker::new("implement authentication with oauth tokens");
        // Plan mentions authentication but not oauth or tokens.
        let plan = make_plan(
            "authentication system",
            &["implement basic authentication"],
        );
        let report = checker.check(&plan);
        let missing_lower: Vec<String> =
            report.missing_keywords.iter().map(|k| k.to_lowercase()).collect();
        // "oauth" and "tokens" should be missing from the plan.
        assert!(
            missing_lower.contains(&"oauth".to_string())
                || missing_lower.contains(&"tokens".to_string()),
            "expected oauth/tokens in missing_keywords, got: {:?}",
            missing_lower
        );
    }

    #[test]
    fn identical_goal_and_plan_zero_drift() {
        let goal = "implement file reading and error handling";
        let checker = PlanCoherenceChecker::new(goal);
        let plan = make_plan(goal, &[goal]);
        let report = checker.check(&plan);
        assert!(!report.drift_detected);
    }

    #[test]
    fn original_goal_accessible() {
        let checker = PlanCoherenceChecker::new("my goal text");
        assert_eq!(checker.original_goal(), "my goal text");
    }

    #[test]
    fn drift_score_is_complement_of_overlap() {
        let checker = PlanCoherenceChecker::new("implement search functionality");
        let plan = make_plan("search implementation", &["implement search"]);
        let report = checker.check(&plan);
        let diff = (report.drift_score - (1.0 - report.semantic_overlap)).abs();
        assert!(diff < 0.001, "drift_score must equal 1.0 - semantic_overlap");
    }
}
