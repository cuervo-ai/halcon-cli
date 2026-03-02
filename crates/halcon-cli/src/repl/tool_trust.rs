//! Dynamic tool trust scoring — runtime reliability tracking for tool selection.
//!
//! Each tool accumulates success/failure/latency metrics across the session.
//! Trust score is computed as:
//!
//! ```text
//! trust = α × success_rate + β × latency_score + γ × recency_bonus
//! ```
//!
//! Tools with trust below `HIDE_THRESHOLD` are removed from the tool surface.
//! Tools with trust below `DEPRIORITIZE_THRESHOLD` are moved to the end of the list.
//!
//! This prevents broken tools (ci_logs 0%, dep_check 0%, search_files 0%) from
//! polluting the tool surface and wasting sub-agent rounds.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use halcon_core::types::PolicyConfig;

/// Weights for trust score components (internal formula, not user-tunable).
const ALPHA_SUCCESS: f64 = 0.60;   // Success rate weight (dominant)
const BETA_LATENCY: f64 = 0.25;    // Latency efficiency weight
const GAMMA_RECENCY: f64 = 0.15;   // Recency bonus weight

// NOTE: HIDE_THRESHOLD, DEPRIORITIZE_THRESHOLD, MIN_CALLS_FOR_FILTERING
// are now read from PolicyConfig (hide_threshold, deprioritize_threshold, min_calls_for_filtering).

/// Maximum latency (ms) used for normalization. Calls slower than this get 0 latency score.
const MAX_ACCEPTABLE_LATENCY_MS: f64 = 30_000.0;

/// Per-tool runtime metrics.
#[derive(Debug, Clone)]
pub(crate) struct ToolMetrics {
    pub success_count: u32,
    pub failure_count: u32,
    pub total_latency_ms: u64,
    pub last_used: Option<Instant>,
    pub last_error: Option<String>,
}

impl ToolMetrics {
    fn new() -> Self {
        Self {
            success_count: 0,
            failure_count: 0,
            total_latency_ms: 0,
            last_used: None,
            last_error: None,
        }
    }

    fn total_calls(&self) -> u32 {
        self.success_count + self.failure_count
    }

    fn success_rate(&self) -> f64 {
        if self.total_calls() == 0 {
            1.0 // Optimistic prior: untested tools get full trust
        } else {
            self.success_count as f64 / self.total_calls() as f64
        }
    }

    fn avg_latency_ms(&self) -> f64 {
        if self.total_calls() == 0 {
            0.0
        } else {
            self.total_latency_ms as f64 / self.total_calls() as f64
        }
    }

    /// Latency score: 1.0 = instant, 0.0 = exceeds MAX_ACCEPTABLE_LATENCY_MS.
    fn latency_score(&self) -> f64 {
        let avg = self.avg_latency_ms();
        (1.0 - avg / MAX_ACCEPTABLE_LATENCY_MS).max(0.0)
    }

    /// Recency bonus: 1.0 if used recently, decays toward 0.0 over 5 minutes.
    fn recency_bonus(&self) -> f64 {
        match self.last_used {
            Some(t) => {
                let elapsed = t.elapsed().as_secs_f64();
                (1.0 - elapsed / 300.0).max(0.0) // 5-minute decay window
            }
            None => 0.5, // Untested tools get neutral recency
        }
    }
}

/// Dynamic tool trust scoring engine.
#[derive(Debug, Clone)]
pub(crate) struct ToolTrustScorer {
    metrics: HashMap<String, ToolMetrics>,
    policy: Arc<PolicyConfig>,
}

/// Trust decision for a single tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrustDecision {
    /// Tool is trusted — include normally.
    Include,
    /// Tool has low trust — include but deprioritize (move to end).
    Deprioritize,
    /// Tool has very low trust — hide from tool surface.
    Hide,
}

impl ToolTrustScorer {
    pub fn new(policy: Arc<PolicyConfig>) -> Self {
        Self {
            metrics: HashMap::new(),
            policy,
        }
    }

    /// Record a successful tool execution.
    pub fn record_success(&mut self, tool_name: &str, latency_ms: u64) {
        let m = self.metrics.entry(tool_name.to_string()).or_insert_with(ToolMetrics::new);
        m.success_count += 1;
        m.total_latency_ms += latency_ms;
        m.last_used = Some(Instant::now());
    }

    /// Record a failed tool execution.
    pub fn record_failure(&mut self, tool_name: &str, latency_ms: u64, error: Option<&str>) {
        let m = self.metrics.entry(tool_name.to_string()).or_insert_with(ToolMetrics::new);
        m.failure_count += 1;
        m.total_latency_ms += latency_ms;
        m.last_used = Some(Instant::now());
        m.last_error = error.map(|e| e.to_string());
    }

    /// Compute trust score for a tool. Range: [0.0, 1.0].
    pub fn trust_score(&self, tool_name: &str) -> f64 {
        match self.metrics.get(tool_name) {
            Some(m) => {
                ALPHA_SUCCESS * m.success_rate()
                    + BETA_LATENCY * m.latency_score()
                    + GAMMA_RECENCY * m.recency_bonus()
            }
            None => 1.0, // Unknown tools get full trust (optimistic prior)
        }
    }

    /// Decide whether to include, deprioritize, or hide a tool.
    ///
    /// Filtering uses `success_rate` directly (not the composite trust score)
    /// because recency bonus should not rescue a persistently broken tool.
    pub fn decide(&self, tool_name: &str) -> TrustDecision {
        let m = match self.metrics.get(tool_name) {
            Some(m) => m,
            None => return TrustDecision::Include, // Unknown = trusted
        };

        // Don't filter until we have enough data
        if m.total_calls() < self.policy.min_calls_for_filtering {
            return TrustDecision::Include;
        }

        let rate = m.success_rate();
        if rate < self.policy.hide_threshold {
            TrustDecision::Hide
        } else if rate < self.policy.deprioritize_threshold {
            TrustDecision::Deprioritize
        } else {
            TrustDecision::Include
        }
    }

    /// Filter a tool list based on trust scores.
    /// Returns (included_tools, hidden_count).
    pub fn filter_tools(
        &self,
        tools: Vec<halcon_core::types::ToolDefinition>,
    ) -> (Vec<halcon_core::types::ToolDefinition>, usize) {
        let mut included = Vec::new();
        let mut deprioritized = Vec::new();
        let mut hidden_count = 0;

        for tool in tools {
            match self.decide(&tool.name) {
                TrustDecision::Include => included.push(tool),
                TrustDecision::Deprioritize => deprioritized.push(tool),
                TrustDecision::Hide => {
                    tracing::info!(
                        tool = %tool.name,
                        score = self.trust_score(&tool.name),
                        "ToolTrust: hiding low-trust tool from surface"
                    );
                    hidden_count += 1;
                }
            }
        }

        // Deprioritized tools go at the end
        included.extend(deprioritized);
        (included, hidden_count)
    }

    /// Get metrics snapshot for a tool (for observability).
    pub fn get_metrics(&self, tool_name: &str) -> Option<&ToolMetrics> {
        self.metrics.get(tool_name)
    }

    /// Get failure records for retry mutation — tools with at least one failure.
    pub fn failure_records(&self) -> Vec<super::retry_mutation::ToolFailureRecord> {
        self.metrics.iter()
            .filter(|(_, m)| m.failure_count > 0)
            .map(|(name, m)| super::retry_mutation::ToolFailureRecord {
                tool_name: name.clone(),
                failure_count: m.failure_count,
            })
            .collect()
    }

    /// Get all tools with their current trust scores.
    pub fn all_scores(&self) -> Vec<(&str, f64)> {
        self.metrics.iter()
            .map(|(name, _)| (name.as_str(), self.trust_score(name)))
            .collect()
    }
}

// ── Trait implementation ──────────────────────────────────────────────────────

impl halcon_core::traits::ToolTrust for ToolTrustScorer {
    fn record_success(&mut self, tool_name: &str, latency_ms: u64) {
        self.record_success(tool_name, latency_ms);
    }

    fn record_failure(&mut self, tool_name: &str, latency_ms: u64, error: Option<&str>) {
        self.record_failure(tool_name, latency_ms, error);
    }

    fn trust_score(&self, tool_name: &str) -> f64 {
        self.trust_score(tool_name)
    }

    fn decide(&self, tool_name: &str) -> halcon_core::types::ToolTrustDecision {
        match self.decide(tool_name) {
            TrustDecision::Include => halcon_core::types::ToolTrustDecision::Include,
            TrustDecision::Deprioritize => halcon_core::types::ToolTrustDecision::Deprioritize,
            TrustDecision::Hide => halcon_core::types::ToolTrustDecision::Hide,
        }
    }

    fn filter_tools(
        &self,
        tools: Vec<halcon_core::types::ToolDefinition>,
    ) -> (Vec<halcon_core::types::ToolDefinition>, usize) {
        self.filter_tools(tools)
    }

    fn get_metrics(&self, tool_name: &str) -> Option<halcon_core::types::ToolTrustMetrics> {
        self.get_metrics(tool_name).map(|m| halcon_core::types::ToolTrustMetrics {
            tool_name: tool_name.to_string(),
            success_rate: m.success_rate(),
            avg_latency_ms: m.avg_latency_ms(),
            call_count: m.total_calls(),
            failure_count: m.failure_count,
        })
    }

    fn failure_records(&self) -> Vec<halcon_core::types::ToolFailureInfo> {
        self.failure_records()
            .into_iter()
            .map(|r| halcon_core::types::ToolFailureInfo {
                tool_name: r.tool_name,
                failure_count: r.failure_count,
            })
            .collect()
    }

    fn all_scores(&self) -> Vec<(String, f64)> {
        self.all_scores()
            .into_iter()
            .map(|(name, score)| (name.to_string(), score))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_policy() -> Arc<PolicyConfig> {
        Arc::new(PolicyConfig::default())
    }

    #[test]
    fn unknown_tool_gets_full_trust() {
        let scorer = ToolTrustScorer::new(default_policy());
        assert_eq!(scorer.trust_score("unknown_tool"), 1.0);
        assert_eq!(scorer.decide("unknown_tool"), TrustDecision::Include);
    }

    #[test]
    fn perfect_tool_gets_high_trust() {
        let mut scorer = ToolTrustScorer::new(default_policy());
        for _ in 0..10 {
            scorer.record_success("file_read", 15);
        }
        let score = scorer.trust_score("file_read");
        assert!(score > 0.80, "perfect tool should have high trust, got {score}");
        assert_eq!(scorer.decide("file_read"), TrustDecision::Include);
    }

    #[test]
    fn zero_success_tool_gets_hidden() {
        let mut scorer = ToolTrustScorer::new(default_policy());
        for _ in 0..5 {
            scorer.record_failure("ci_logs", 24, Some("not found"));
        }
        // decide() uses success_rate directly: 0% < HIDE_THRESHOLD (0.15)
        assert_eq!(scorer.decide("ci_logs"), TrustDecision::Hide);
    }

    #[test]
    fn low_success_tool_gets_deprioritized() {
        let mut scorer = ToolTrustScorer::new(default_policy());
        scorer.record_success("dep_check", 1500);
        scorer.record_failure("dep_check", 1500, None);
        scorer.record_failure("dep_check", 1500, None);
        scorer.record_failure("dep_check", 1500, None);
        // 25% success rate: above HIDE_THRESHOLD (0.15) but below DEPRIORITIZE_THRESHOLD (0.40)
        assert_eq!(scorer.decide("dep_check"), TrustDecision::Deprioritize);
    }

    #[test]
    fn fewer_than_min_calls_not_filtered() {
        let mut scorer = ToolTrustScorer::new(default_policy());
        scorer.record_failure("new_tool", 100, None);
        scorer.record_failure("new_tool", 100, None);
        // Only 2 calls < MIN_CALLS_FOR_FILTERING (3)
        assert_eq!(scorer.decide("new_tool"), TrustDecision::Include);
    }

    #[test]
    fn filter_tools_separates_and_hides() {
        let mut scorer = ToolTrustScorer::new(default_policy());
        // Good tool
        for _ in 0..5 {
            scorer.record_success("file_read", 10);
        }
        // Bad tool
        for _ in 0..5 {
            scorer.record_failure("ci_logs", 24, None);
        }
        // Medium tool
        scorer.record_success("dep_check", 1500);
        scorer.record_failure("dep_check", 1500, None);
        scorer.record_failure("dep_check", 1500, None);
        scorer.record_failure("dep_check", 1500, None);

        let tools = vec![
            halcon_core::types::ToolDefinition {
                name: "file_read".into(),
                description: "Read file".into(),
                input_schema: serde_json::json!({}),
            },
            halcon_core::types::ToolDefinition {
                name: "ci_logs".into(),
                description: "CI logs".into(),
                input_schema: serde_json::json!({}),
            },
            halcon_core::types::ToolDefinition {
                name: "dep_check".into(),
                description: "Dep check".into(),
                input_schema: serde_json::json!({}),
            },
        ];

        let (filtered, hidden) = scorer.filter_tools(tools);
        assert_eq!(hidden, 1, "ci_logs should be hidden");
        assert_eq!(filtered.len(), 2, "file_read + dep_check should remain");
        assert_eq!(filtered[0].name, "file_read", "trusted tool first");
        // dep_check should be second (deprioritized to end)
    }

    #[test]
    fn success_rate_calculation() {
        let mut scorer = ToolTrustScorer::new(default_policy());
        scorer.record_success("bash", 100);
        scorer.record_success("bash", 200);
        scorer.record_failure("bash", 300, None);
        let metrics = scorer.get_metrics("bash").unwrap();
        assert_eq!(metrics.total_calls(), 3);
        let rate = metrics.success_rate();
        assert!((rate - 0.6667).abs() < 0.01, "expected ~0.667, got {rate}");
    }

    #[test]
    fn high_latency_reduces_trust() {
        let mut scorer = ToolTrustScorer::new(default_policy());
        for _ in 0..5 {
            scorer.record_success("slow_tool", 25_000); // 25s per call
        }
        let score = scorer.trust_score("slow_tool");
        // Success rate = 1.0 (full α), but latency_score ≈ 0.17 (25000/30000 → 0.17 remaining)
        assert!(score < 0.90, "high latency should reduce trust, got {score}");
    }

    #[test]
    fn all_scores_returns_tracked_tools() {
        let mut scorer = ToolTrustScorer::new(default_policy());
        scorer.record_success("a", 10);
        scorer.record_success("b", 20);
        let scores = scorer.all_scores();
        assert_eq!(scores.len(), 2);
    }

    #[test]
    fn failure_records_captures_failures() {
        let mut s = ToolTrustScorer::new(default_policy());
        s.record_failure("ci_logs", 100, None);
        s.record_failure("ci_logs", 200, None);
        s.record_success("bash", 50);
        let r = s.failure_records();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].failure_count, 2);
    }
}
