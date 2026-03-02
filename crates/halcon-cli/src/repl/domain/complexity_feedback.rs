//! Complexity feedback loop — runtime complexity upgrades.
//!
//! Monitors actual execution signals and upgrades task complexity when the
//! observed behavior exceeds what the initial classification predicted.
//! Only upgrades, never downgrades. Single upgrade per session.
//!
//! Pure business logic — no I/O.

use std::sync::Arc;

use halcon_core::types::PolicyConfig;

use crate::repl::decision_layer::TaskComplexity;

/// Observation of actual execution behavior for one evaluation.
#[derive(Debug, Clone)]
pub struct ComplexityObservation {
    pub rounds_used: usize,
    pub replans_triggered: usize,
    pub distinct_tools_used: usize,
    pub domains_touched: usize,
    pub elapsed_secs: f64,
    pub orchestration_used: bool,
    pub tool_errors: usize,
}

/// Result of complexity evaluation — whether an upgrade is recommended.
#[derive(Debug, Clone)]
pub struct ComplexityAdjustment {
    pub original: TaskComplexity,
    pub adjusted: TaskComplexity,
    pub was_upgraded: bool,
    pub sla_refresh_needed: bool,
    pub confidence: f64,
    pub reason: &'static str,
}

/// Stateful tracker for runtime complexity feedback.
#[derive(Debug)]
pub struct ComplexityTracker {
    original_complexity: TaskComplexity,
    current_complexity: TaskComplexity,
    expected_rounds: usize,
    already_upgraded: bool,
    policy: Arc<PolicyConfig>,
}

impl ComplexityTracker {
    /// Create a new tracker with initial complexity classification.
    pub fn new(complexity: TaskComplexity, expected_rounds: usize, policy: Arc<PolicyConfig>) -> Self {
        Self {
            original_complexity: complexity.clone(),
            current_complexity: complexity,
            expected_rounds: expected_rounds.max(1),
            already_upgraded: false,
            policy,
        }
    }

    /// Evaluate current observations and decide if an upgrade is needed.
    ///
    /// Returns `None` if evaluation conditions aren't met (too few rounds, already upgraded).
    pub fn evaluate(&mut self, obs: &ComplexityObservation) -> Option<ComplexityAdjustment> {
        // Gate: already upgraded
        if self.already_upgraded {
            return None;
        }

        // Gate: minimum rounds
        if obs.rounds_used < self.policy.complexity_min_rounds {
            return None;
        }

        // Gate: no room to upgrade
        let next = next_complexity(&self.current_complexity);
        let next = match next {
            Some(n) => n,
            None => return None, // already at LongHorizon
        };

        // Check: actual rounds significantly exceed expected
        let ratio = obs.rounds_used as f64 / self.expected_rounds as f64;
        if ratio < self.policy.complexity_upgrade_ratio {
            return Some(ComplexityAdjustment {
                original: self.original_complexity.clone(),
                adjusted: self.current_complexity.clone(),
                was_upgraded: false,
                sla_refresh_needed: false,
                confidence: 0.0,
                reason: "ratio below upgrade threshold",
            });
        }

        // Bayesian confidence estimate
        let confidence = compute_upgrade_confidence(obs, &self.current_complexity, ratio);
        if confidence < self.policy.complexity_confidence_threshold {
            return Some(ComplexityAdjustment {
                original: self.original_complexity.clone(),
                adjusted: self.current_complexity.clone(),
                was_upgraded: false,
                sla_refresh_needed: false,
                confidence,
                reason: "confidence below threshold",
            });
        }

        // Upgrade!
        self.current_complexity = next.clone();
        self.already_upgraded = true;

        Some(ComplexityAdjustment {
            original: self.original_complexity.clone(),
            adjusted: next,
            was_upgraded: true,
            sla_refresh_needed: true,
            confidence,
            reason: "complexity upgrade triggered",
        })
    }

    /// Get the current (possibly upgraded) complexity.
    pub fn current(&self) -> &TaskComplexity {
        &self.current_complexity
    }

    /// Whether an upgrade has already been performed.
    pub fn was_upgraded(&self) -> bool {
        self.already_upgraded
    }
}

/// Determine the next complexity tier (only upgrades).
fn next_complexity(current: &TaskComplexity) -> Option<TaskComplexity> {
    match current {
        TaskComplexity::SimpleExecution => Some(TaskComplexity::StructuredTask),
        TaskComplexity::StructuredTask => Some(TaskComplexity::MultiDomain),
        TaskComplexity::MultiDomain => Some(TaskComplexity::LongHorizon),
        TaskComplexity::LongHorizon => None,
    }
}

/// Compute Bayesian-style confidence for upgrade based on multiple signals.
fn compute_upgrade_confidence(
    obs: &ComplexityObservation,
    _current: &TaskComplexity,
    ratio: f64,
) -> f64 {
    let mut confidence = 0.0;

    // Signal 1: Round ratio (strongest signal)
    // ratio > 1.5 → +0.40, ratio > 2.0 → +0.50
    confidence += (ratio - 1.0).clamp(0.0, 1.0) * 0.50;

    // Signal 2: Replans triggered
    if obs.replans_triggered >= 2 {
        confidence += 0.15;
    } else if obs.replans_triggered >= 1 {
        confidence += 0.08;
    }

    // Signal 3: Tool diversity
    if obs.distinct_tools_used >= 5 {
        confidence += 0.10;
    }

    // Signal 4: Multi-domain evidence
    if obs.domains_touched >= 2 {
        confidence += 0.10;
    }

    // Signal 5: Orchestration actually used
    if obs.orchestration_used {
        confidence += 0.10;
    }

    // Signal 6: Negative — tool errors weaken confidence
    if obs.tool_errors >= 3 {
        confidence -= 0.10;
    }

    confidence.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_policy() -> Arc<PolicyConfig> {
        Arc::new(PolicyConfig::default())
    }

    fn basic_obs(rounds: usize) -> ComplexityObservation {
        ComplexityObservation {
            rounds_used: rounds,
            replans_triggered: 1,
            distinct_tools_used: 4,
            domains_touched: 1,
            elapsed_secs: 30.0,
            orchestration_used: false,
            tool_errors: 0,
        }
    }

    #[test]
    fn phase3_complexity_no_upgrade_below_min_rounds() {
        let mut tracker = ComplexityTracker::new(
            TaskComplexity::SimpleExecution,
            5,
            test_policy(),
        );
        let obs = basic_obs(2); // below min_rounds=3
        assert!(tracker.evaluate(&obs).is_none());
    }

    #[test]
    fn phase3_complexity_no_upgrade_below_ratio() {
        let mut tracker = ComplexityTracker::new(
            TaskComplexity::SimpleExecution,
            10,
            test_policy(),
        );
        let obs = basic_obs(5); // ratio=0.5, below 1.5
        let adj = tracker.evaluate(&obs).unwrap();
        assert!(!adj.was_upgraded);
        assert_eq!(adj.reason, "ratio below upgrade threshold");
    }

    #[test]
    fn phase3_complexity_upgrade_simple_to_structured() {
        let mut tracker = ComplexityTracker::new(
            TaskComplexity::SimpleExecution,
            3,
            test_policy(),
        );
        // ratio = 8/3 = 2.67 → triggers. Need high confidence too.
        let obs = ComplexityObservation {
            rounds_used: 8,
            replans_triggered: 2,
            distinct_tools_used: 6,
            domains_touched: 2,
            elapsed_secs: 60.0,
            orchestration_used: true,
            tool_errors: 0,
        };
        let adj = tracker.evaluate(&obs).unwrap();
        assert!(adj.was_upgraded);
        assert!(matches!(adj.adjusted, TaskComplexity::StructuredTask));
        assert!(adj.sla_refresh_needed);
    }

    #[test]
    fn phase3_complexity_single_upgrade_only() {
        let mut tracker = ComplexityTracker::new(
            TaskComplexity::SimpleExecution,
            3,
            test_policy(),
        );
        let obs = ComplexityObservation {
            rounds_used: 8,
            replans_triggered: 2,
            distinct_tools_used: 6,
            domains_touched: 2,
            elapsed_secs: 60.0,
            orchestration_used: true,
            tool_errors: 0,
        };
        let first = tracker.evaluate(&obs).unwrap();
        assert!(first.was_upgraded);

        // Second evaluation — should be None (already upgraded)
        assert!(tracker.evaluate(&obs).is_none());
        assert!(tracker.was_upgraded());
    }

    #[test]
    fn phase3_complexity_no_upgrade_at_long_horizon() {
        let mut tracker = ComplexityTracker::new(
            TaskComplexity::LongHorizon,
            3,
            test_policy(),
        );
        let obs = basic_obs(10);
        assert!(tracker.evaluate(&obs).is_none());
    }

    #[test]
    fn phase3_complexity_confidence_below_threshold() {
        let mut tracker = ComplexityTracker::new(
            TaskComplexity::SimpleExecution,
            3,
            test_policy(),
        );
        // Ratio = 5/3 ≈ 1.67 → triggers ratio check
        // But minimal supporting signals → low confidence
        let obs = ComplexityObservation {
            rounds_used: 5,
            replans_triggered: 0,
            distinct_tools_used: 2,
            domains_touched: 1,
            elapsed_secs: 20.0,
            orchestration_used: false,
            tool_errors: 3, // weakens confidence
        };
        let adj = tracker.evaluate(&obs).unwrap();
        assert!(!adj.was_upgraded);
        assert_eq!(adj.reason, "confidence below threshold");
    }

    #[test]
    fn phase3_complexity_upgrade_chain() {
        // Verify the upgrade chain: Simple → Structured → Multi → Long
        assert!(matches!(next_complexity(&TaskComplexity::SimpleExecution), Some(TaskComplexity::StructuredTask)));
        assert!(matches!(next_complexity(&TaskComplexity::StructuredTask), Some(TaskComplexity::MultiDomain)));
        assert!(matches!(next_complexity(&TaskComplexity::MultiDomain), Some(TaskComplexity::LongHorizon)));
        assert!(next_complexity(&TaskComplexity::LongHorizon).is_none());
    }

    #[test]
    fn phase3_complexity_confidence_signals() {
        let high_obs = ComplexityObservation {
            rounds_used: 10,
            replans_triggered: 3,
            distinct_tools_used: 8,
            domains_touched: 3,
            elapsed_secs: 120.0,
            orchestration_used: true,
            tool_errors: 0,
        };
        let conf = compute_upgrade_confidence(&high_obs, &TaskComplexity::SimpleExecution, 3.0);
        assert!(conf >= 0.70, "high-signal obs should have confidence ≥0.70, got {conf}");

        let low_obs = ComplexityObservation {
            rounds_used: 5,
            replans_triggered: 0,
            distinct_tools_used: 2,
            domains_touched: 1,
            elapsed_secs: 10.0,
            orchestration_used: false,
            tool_errors: 5,
        };
        let conf = compute_upgrade_confidence(&low_obs, &TaskComplexity::SimpleExecution, 1.6);
        assert!(conf < 0.70, "low-signal obs should have confidence <0.70, got {conf}");
    }

    #[test]
    fn phase3_complexity_current_tracks_upgrade() {
        let mut tracker = ComplexityTracker::new(
            TaskComplexity::SimpleExecution,
            3,
            test_policy(),
        );
        assert!(matches!(tracker.current(), TaskComplexity::SimpleExecution));

        let obs = ComplexityObservation {
            rounds_used: 8,
            replans_triggered: 2,
            distinct_tools_used: 6,
            domains_touched: 2,
            elapsed_secs: 60.0,
            orchestration_used: true,
            tool_errors: 0,
        };
        tracker.evaluate(&obs);
        assert!(matches!(tracker.current(), TaskComplexity::StructuredTask));
    }
}
