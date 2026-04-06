/// Classification of a single agent loop round for cross-type oscillation detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoundType {
    /// Round had at least one tool call.
    Tool,
    /// Round was text-only (model produced output without calling tools).
    Text,
    /// Round produced neither tools nor useful text (empty).
    Empty,
}

/// What action the tool loop guard recommends after a tool round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoopAction {
    /// Normal — proceed to next round.
    Continue,
    /// Consecutive tool rounds >= synthesis_threshold → inject synthesis directive.
    InjectSynthesis,
    /// Consecutive tool rounds >= force_threshold → remove tools from request.
    ForceNoTools,
    /// Oscillation detected or plan complete → stop now.
    Break,
    /// Stagnation detected (read saturation + no progress) → regenerate plan (Sprint 3).
    ///
    /// Fired when:
    /// - 3+ consecutive read-only tool rounds (detect_read_saturation)
    /// - Plan exists and has 0% completion after 3+ rounds
    /// - No recent write/modify operations showing progress
    ReplanRequired,
}

/// HICON Phase 4: Anomaly detection result for self-correction.
#[derive(Debug, Clone)]
pub(crate) struct AnomalyResult {
    pub action: LoopAction,
    pub anomaly: super::super::anomaly_detector::AgentAnomaly,
    pub severity: super::super::anomaly_detector::AnomalySeverity,
}

/// Snapshot of plan execution progress (for plan-aware heuristics).
#[derive(Debug, Clone, Copy)]
struct PlanProgress {
    completed: usize,
    total: usize,
    /// Elapsed milliseconds since plan start.
    elapsed_ms: u64,
}

/// Intelligent multi-layered tool loop termination guard.
///
/// Replaces the blunt `consecutive_tool_rounds >= 5` counter with pattern
/// detection: oscillation (A→B→A→B), read saturation (3+ rounds of only
/// ReadOnly tools), deduplication, and graduated escalation (synthesis
/// directive → forced tool withdrawal → break).
///
/// **HICON Integration**: Uses Bayesian anomaly detection for advanced
/// pattern recognition (ToolCycle, TokenExplosion, PlanOscillation).
pub(crate) struct ToolLoopGuard {
    /// Per-round tool call log: Vec<Vec<(tool_name, args_hash)>>.
    history: Vec<Vec<(String, u64)>>,
    /// Consecutive tool-use rounds.
    consecutive_rounds: usize,
    /// Threshold for synthesis directive injection (default 3).
    synthesis_threshold: usize,
    /// Threshold for forced tool withdrawal (default 4).
    force_threshold: usize,
    /// Whether plan completion has been signaled.
    plan_complete: bool,
    /// Track plan progress for context-aware dynamic thresholds (Sprint 2).
    plan_progress: Option<PlanProgress>,
    /// Bayesian anomaly detector (HICON integration).
    anomaly_detector: super::super::anomaly_detector::BayesianAnomalyDetector,
    /// Token tracking for explosion detection.
    recent_token_counts: Vec<(u64, u64, u64)>, // (input, output, total)
    /// Recent errors for stagnation analysis.
    recent_errors: Vec<String>,
    /// Current plan hash for oscillation detection.
    current_plan_hash: Option<u64>,
    /// Last detected anomaly (for self-correction in agent loop).
    last_anomaly: Option<AnomalyResult>,
    /// Sliding window of recent round types for cross-type oscillation detection.
    /// Window size controlled by `oscillation_window` (default: 8, from PolicyConfig).
    round_types: std::collections::VecDeque<RoundType>,
    /// Max entries in `round_types` sliding window (PolicyConfig.oscillation_window).
    oscillation_window: usize,
    /// Minimum synthesis threshold used by `set_tightness` (PolicyConfig.loop_guard_min_synthesis).
    min_synthesis: usize,
    /// Minimum force threshold used by `set_tightness` (PolicyConfig.loop_guard_min_force).
    min_force: usize,
}

/// Known read-only tool names. Tools in this set gather information but don't
/// modify state — sustained use signals the model is exploring without converging.
const READ_ONLY_TOOLS: &[&str] = &[
    "file_read",
    "glob",
    "grep",
    "directory_tree",
    "git_status",
    "git_diff",
    "git_log",
    "fuzzy_find",
    "symbol_search",
    "file_inspect",
    "web_search",
    "web_fetch",
];

impl ToolLoopGuard {
    pub(crate) fn new() -> Self {
        Self {
            history: Vec::new(),
            consecutive_rounds: 0,
            // Raised thresholds: complex tasks (website creation, multi-file work) need
            // more rounds than the old 3/4 defaults. synthesis=6 injects a hint first,
            // force=10 only removes tools as a last resort after 10 pure-tool rounds.
            synthesis_threshold: 6,
            force_threshold: 10,
            plan_complete: false,
            plan_progress: None,
            anomaly_detector: super::super::anomaly_detector::BayesianAnomalyDetector::new(),
            recent_token_counts: Vec::new(),
            recent_errors: Vec::new(),
            current_plan_hash: None,
            last_anomaly: None,
            round_types: std::collections::VecDeque::new(),
            oscillation_window: 8,
            min_synthesis: 3,
            min_force: 5,
        }
    }

    /// Create a guard configured from PolicyConfig thresholds.
    pub(crate) fn with_policy(policy: &halcon_core::types::PolicyConfig) -> Self {
        let mut guard = Self::new();
        guard.oscillation_window = policy.oscillation_window;
        guard.min_synthesis = policy.loop_guard_min_synthesis;
        guard.min_force = policy.loop_guard_min_force;
        guard
    }

    /// Record a completed tool round and return the recommended action.
    ///
    /// **HICON Integration**: Uses Bayesian anomaly detection BEFORE
    /// rule-based heuristics for early pattern recognition.
    pub(crate) fn record_round(&mut self, tools: &[(String, u64)]) -> LoopAction {
        self.history.push(tools.to_vec());
        self.consecutive_rounds += 1;

        // Track round type for cross-type oscillation detection.
        self.round_types.push_back(RoundType::Tool);
        if self.round_types.len() > self.oscillation_window {
            self.round_types.pop_front();
        }

        // Cross-type oscillation check BEFORE Bayesian anomaly detection.
        if self.detect_cross_type_oscillation() {
            tracing::warn!(
                round = self.consecutive_rounds,
                "Loop guard: cross-type Tool↔Text oscillation detected — forcing break"
            );
            return LoopAction::Break;
        }

        // HICON: Run Bayesian anomaly detection FIRST
        let bayesian_result = self.check_bayesian_anomalies(tools);
        if let Some(result) = bayesian_result {
            // Store for self-correction in agent loop
            self.last_anomaly = Some(result.clone());
            return result.action;
        }

        // Plan complete → force synthesis immediately.
        if self.plan_complete {
            return LoopAction::Break;
        }

        // Oscillation detection takes priority — stop immediately.
        if self.detect_oscillation() {
            return LoopAction::Break;
        }

        // Sprint 3: Stagnation detection (read saturation + no progress) → suggest replan.
        // Check BEFORE escalation thresholds to enable recovery before ForceNoTools.
        if self.detect_stagnation() {
            return LoopAction::ReplanRequired;
        }

        // Sprint 2: Dynamic thresholds based on plan progress
        let (synthesis_threshold, force_threshold) = self.compute_dynamic_thresholds();

        // Graduated escalation based on consecutive rounds (using dynamic thresholds).
        if self.consecutive_rounds >= force_threshold {
            return LoopAction::ForceNoTools;
        }
        if self.consecutive_rounds >= synthesis_threshold {
            // Read saturation amplifies urgency — but still InjectSynthesis at this stage.
            return LoopAction::InjectSynthesis;
        }

        LoopAction::Continue
    }

    /// Detect oscillation patterns: A→B→A→B or A→A→A (3+ identical rounds).
    pub(crate) fn detect_oscillation(&self) -> bool {
        let len = self.history.len();
        if len < 3 {
            return false;
        }

        // Check A→A→A: 3 consecutive identical tool sets.
        let last = &self.history[len - 1];
        let prev1 = &self.history[len - 2];
        let prev2 = &self.history[len - 3];
        if last == prev1 && prev1 == prev2 {
            return true;
        }

        // Check A→B→A→B: alternating pattern over 4 rounds.
        if len >= 4 {
            let prev3 = &self.history[len - 4];
            if last == prev2 && prev1 == prev3 && last != prev1 {
                return true;
            }
        }

        false
    }

    /// Detect read saturation: 3+ consecutive rounds using only read-only tools.
    pub(crate) fn detect_read_saturation(&self) -> bool {
        if self.history.len() < 3 {
            return false;
        }
        let recent = &self.history[self.history.len().saturating_sub(3)..];
        recent.iter().all(|round| {
            !round.is_empty()
                && round
                    .iter()
                    .all(|(name, _)| READ_ONLY_TOOLS.contains(&name.as_str()))
        })
    }

    /// Check if this exact (tool_name, args_hash) was already executed in any prior round.
    pub(crate) fn is_duplicate(&self, tool_name: &str, args_hash: u64) -> bool {
        // Check all rounds except the current one being built (last element, if any).
        // At the point of dedup checking, the current round hasn't been recorded yet,
        // so we check all of self.history.
        self.history.iter().any(|round| {
            round
                .iter()
                .any(|(name, hash)| name == tool_name && *hash == args_hash)
        })
    }

    /// Signal that all plan steps have been completed.
    pub(crate) fn force_synthesis(&mut self) {
        self.plan_complete = true;
    }

    /// Get the current consecutive tool round count.
    pub(crate) fn consecutive_rounds(&self) -> usize {
        self.consecutive_rounds
    }

    /// Whether plan_complete was signaled.
    pub(crate) fn plan_complete(&self) -> bool {
        self.plan_complete
    }

    /// Reset the consecutive rounds counter when the model generates text without tools.
    ///
    /// Called when `StopReason::EndTurn` (text-only round). This prevents false
    /// positives when the agent alternates between tool use and synthesis.
    ///
    /// **Preserves** `history` for oscillation detection (cross-round pattern matching).
    /// **Resets** only `consecutive_rounds` (sliding window of tool-only rounds).
    pub(crate) fn reset_on_text_round(&mut self) {
        if self.consecutive_rounds > 0 {
            tracing::debug!(
                previous_consecutive = self.consecutive_rounds,
                "Loop guard reset: model generated text without tools"
            );
            self.consecutive_rounds = 0;
        }
    }

    /// Record a text-only round and track its type for cross-type oscillation detection.
    ///
    /// Call this INSTEAD OF `reset_on_text_round()` at all text-round sites in agent.rs.
    /// Internally calls `reset_on_text_round()` to preserve existing counter-reset semantics,
    /// and additionally records `RoundType::Text` in the sliding window.
    ///
    /// Use `detect_cross_type_oscillation()` AFTER this call to check for Tool↔Text alternation.
    pub(crate) fn record_text_round(&mut self) {
        self.round_types.push_back(RoundType::Text);
        if self.round_types.len() > self.oscillation_window {
            self.round_types.pop_front();
        }
        self.reset_on_text_round(); // existing behavior preserved
    }

    /// Detect cross-type oscillation: Tool↔Text alternation pattern over the last 4 rounds.
    ///
    /// Pattern matches: `Tool→Text→Tool→Text` OR `Text→Tool→Text→Tool` (and any 4-entry
    /// window where all adjacent entries alternate AND both Tool and Text appear).
    ///
    /// Returns `false` when fewer than 4 round-type samples have been collected.
    ///
    /// **PlanExecuteReflect exception**: The strategy inherently produces `Tool→Text→Tool→Text`
    /// alternation (tool round → synthesis/reflection text round → next tool round → ...).
    /// This is NOT an oscillation bug — it is the correct working pattern. The detector
    /// therefore suppresses the alarm when `plan_progress.completed > 0`, meaning at least
    /// one plan step has been recorded as done. A plan that is actively advancing must not be
    /// terminated on the very pattern that proves it is working.
    pub(crate) fn detect_cross_type_oscillation(&self) -> bool {
        if self.round_types.len() < 4 {
            return false;
        }

        // PlanExecuteReflect guard: if any plan step has been completed, the Tool↔Text
        // alternation is the expected control-flow pattern, not a stuck loop.
        // Only treat it as a problem when the plan has made zero progress AND we have an
        // active plan tracker (plan_progress == None means no plan at all; without a plan,
        // alternation is always suspicious because there is nothing to drive convergence).
        if let Some(progress) = self.plan_progress {
            if progress.completed > 0 {
                tracing::trace!(
                    completed = progress.completed,
                    total = progress.total,
                    "Loop guard: cross-type oscillation suppressed — plan is advancing"
                );
                return false;
            }
        }

        // Collect last 4 entries (most recent last → rev().take(4) = most-recent-first).
        let last4: Vec<&RoundType> = self.round_types.iter().rev().take(4).collect();
        // All adjacent pairs must differ (alternating pattern).
        let alternates = last4.windows(2).all(|w| w[0] != w[1]);
        // Pattern must span both Tool and Text (not just Empty alternating).
        let has_tool = last4.iter().any(|&&t| t == RoundType::Tool);
        let has_text = last4.iter().any(|&&t| t == RoundType::Text);
        alternates && has_tool && has_text
    }

    /// Scale `synthesis_threshold` and `force_threshold` based on UCB1 `StrategyContext` tightness.
    ///
    /// - `tightness = 0.0`: thresholds at base values (6, 10) — relaxed, max rounds before action
    /// - `tightness = 1.0`: thresholds at minimum values (3, 5) — tight, early synthesis/withdrawal
    ///
    /// Linear interpolation: `value = (1 - t) * base + t * min`
    pub(crate) fn set_tightness(&mut self, tightness: f32) {
        const BASE_SYNTHESIS: usize = 6;
        const BASE_FORCE: usize = 10;
        let min_synthesis = self.min_synthesis;
        let min_force = self.min_force;
        let scale = tightness.clamp(0.0, 1.0) as f64;
        self.synthesis_threshold =
            ((1.0 - scale) * BASE_SYNTHESIS as f64 + scale * min_synthesis as f64) as usize;
        self.force_threshold =
            ((1.0 - scale) * BASE_FORCE as f64 + scale * min_force as f64) as usize;
        tracing::debug!(
            tightness,
            synthesis_threshold = self.synthesis_threshold,
            force_threshold = self.force_threshold,
            "ToolLoopGuard thresholds scaled by UCB1 strategy tightness"
        );
    }

    /// Reset state after successful plan regeneration — fresh start with new strategy (Sprint 3).
    ///
    /// Called when `LoopAction::ReplanRequired` triggers and replan succeeds.
    /// Clears all state to give the new plan a clean slate.
    ///
    /// **Clears**:
    /// - `consecutive_rounds` → 0 (fresh escalation tracking)
    /// - `history` → empty (new plan = new context, clear oscillation history)
    /// - `plan_complete` → false (new plan not complete yet)
    ///
    /// **Preserves**:
    /// - `plan_progress` (will be updated by tracker on next round)
    pub(crate) fn reset_on_replan(&mut self) {
        tracing::info!(
            previous_consecutive = self.consecutive_rounds,
            previous_history_len = self.history.len(),
            "Loop guard reset: plan regenerated"
        );
        self.consecutive_rounds = 0;
        self.history.clear(); // New plan = new context, clear oscillation history
        self.plan_complete = false;
        // Keep plan_progress (tracker will update it next round)
    }

    /// Update plan progress for context-aware dynamic thresholds (Sprint 2).
    ///
    /// Called after `ExecutionTracker::record_tool_results()` to provide
    /// real-time plan completion metrics.
    pub(crate) fn update_plan_progress(&mut self, completed: usize, total: usize, elapsed_ms: u64) {
        self.plan_progress = Some(PlanProgress {
            completed,
            total,
            elapsed_ms,
        });
        tracing::trace!(
            completed,
            total,
            elapsed_ms,
            "Loop guard: plan progress updated"
        );
    }

    /// Update token counts for Bayesian anomaly detection.
    ///
    /// Called after each model invocation to track token growth rate.
    pub(crate) fn update_token_counts(&mut self, input: u64, output: u64, total: u64) {
        self.recent_token_counts.push((input, output, total));

        // Keep last 10 rounds
        if self.recent_token_counts.len() > 10 {
            self.recent_token_counts.remove(0);
        }
    }

    /// Record an error for stagnation pattern detection.
    ///
    /// Called when a tool fails or replan fails.
    pub(crate) fn record_error(&mut self, error: &str) {
        self.recent_errors.push(error.to_string());

        // Keep last 10 errors
        if self.recent_errors.len() > 10 {
            self.recent_errors.remove(0);
        }
    }

    /// Update current plan hash for oscillation detection.
    ///
    /// Called after planning or replanning.
    pub(crate) fn update_plan_hash(&mut self, plan_hash: u64) {
        self.current_plan_hash = Some(plan_hash);
    }

    /// HICON Phase 4: Retrieve last detected anomaly for self-correction.
    ///
    /// Takes ownership of the stored anomaly (one-time retrieval).
    pub(crate) fn take_last_anomaly(&mut self) -> Option<AnomalyResult> {
        self.last_anomaly.take()
    }

    /// Detect stagnation: sustained read-only activity with zero plan progress (Sprint 3).
    ///
    /// **Criteria**:
    /// - 3+ consecutive read-only tool rounds (`detect_read_saturation()`)
    /// - Plan exists and has 0% completion after 3+ rounds
    /// - No recent successful write/modify operations
    ///
    /// **Purpose**: Trigger plan regeneration before resorting to ForceNoTools.
    pub(crate) fn detect_stagnation(&self) -> bool {
        // Must have read saturation first
        if !self.detect_read_saturation() {
            return false;
        }

        // Must have a plan with zero progress
        if let Some(progress) = self.plan_progress {
            if progress.completed == 0 && self.consecutive_rounds >= 3 {
                tracing::warn!(
                    consecutive_rounds = self.consecutive_rounds,
                    plan_total = progress.total,
                    elapsed_ms = progress.elapsed_ms,
                    "Stagnation detected: read saturation with 0% plan progress"
                );
                return true;
            }
        }

        false
    }

    /// HICON Phase 3: Check for Bayesian-detected anomalies and return early action if critical.
    ///
    /// Builds AgentSnapshot from current state and runs Bayesian detector.
    /// Returns Some(AnomalyResult) if critical/high anomaly detected, None otherwise.
    fn check_bayesian_anomalies(&mut self, _tools: &[(String, u64)]) -> Option<AnomalyResult> {
        use super::super::anomaly_detector::AgentSnapshot;

        // Build recent_tool_history from last 3 rounds (max, for performance)
        let recent_tool_history: Vec<Vec<(String, u64, Option<String>)>> = self
            .history
            .iter()
            .rev()
            .take(3)
            .rev()
            .map(|round| {
                round
                    .iter()
                    .map(|(name, args_hash)| {
                        // No target extraction yet — set to None
                        (name.clone(), *args_hash, None)
                    })
                    .collect()
            })
            .collect();

        // Extract plan progress if available
        let plan_progress = self.plan_progress.map(|p| (p.completed, p.total));

        // Get latest token counts (or default)
        let token_counts = self
            .recent_token_counts
            .last()
            .copied()
            .unwrap_or((0, 0, 0));

        // Calculate elapsed_ms from plan_progress (if available)
        let elapsed_ms = self.plan_progress.map(|p| p.elapsed_ms).unwrap_or(0);

        // Build snapshot with all available metrics
        let snapshot = AgentSnapshot {
            round: self.consecutive_rounds,
            recent_tool_history,
            plan_progress,
            token_counts,
            recent_errors: self.recent_errors.clone(),
            plan_hash: self.current_plan_hash,
            elapsed_ms,
        };

        // Run Bayesian detector
        let detection_results = self.anomaly_detector.detect(&snapshot);

        // If no anomalies detected, return early
        if detection_results.is_empty() {
            return None;
        }

        // Process anomalies by severity (critical/high trigger immediate action)
        for result in &detection_results {
            match result.severity {
                super::super::anomaly_detector::AnomalySeverity::Critical => {
                    // Critical anomalies require immediate action
                    tracing::error!(
                        anomaly_type = ?result.anomaly,
                        confidence = result.confidence,
                        round = self.consecutive_rounds,
                        "CRITICAL Bayesian anomaly detected - triggering strong action"
                    );

                    // Determine action based on anomaly type
                    let (dyn_synth, dyn_force) = self.compute_dynamic_thresholds();
                    let action = match result.anomaly {
                        super::super::anomaly_detector::AgentAnomaly::ToolCycle { .. } => {
                            LoopAction::Break
                        }
                        super::super::anomaly_detector::AgentAnomaly::PlanOscillation {
                            ..
                        } => LoopAction::Break,
                        super::super::anomaly_detector::AgentAnomaly::TokenExplosion { .. } => {
                            LoopAction::ForceNoTools
                        }
                        _ => {
                            // StagnantProgress/ReadSaturation: use dynamic thresholds
                            if self.consecutive_rounds >= dyn_force {
                                LoopAction::ForceNoTools
                            } else if self.consecutive_rounds >= dyn_synth {
                                LoopAction::InjectSynthesis
                            } else {
                                LoopAction::InjectSynthesis
                            }
                        }
                    };

                    return Some(AnomalyResult {
                        action,
                        anomaly: result.anomaly.clone(),
                        severity: result.severity,
                    });
                }
                super::super::anomaly_detector::AnomalySeverity::High => {
                    tracing::warn!(
                        anomaly_type = ?result.anomaly,
                        confidence = result.confidence,
                        round = self.consecutive_rounds,
                        "HIGH severity Bayesian anomaly detected - injecting synthesis"
                    );

                    // High severity gets synthesis injection
                    return Some(AnomalyResult {
                        action: LoopAction::InjectSynthesis,
                        anomaly: result.anomaly.clone(),
                        severity: result.severity,
                    });
                }
                super::super::anomaly_detector::AnomalySeverity::Medium => {
                    tracing::info!(
                        anomaly_type = ?result.anomaly,
                        confidence = result.confidence,
                        round = self.consecutive_rounds,
                        "MEDIUM severity Bayesian anomaly detected - monitoring"
                    );
                    // Medium severity: log but continue
                }
                super::super::anomaly_detector::AnomalySeverity::Low => {
                    // Low severity: silent monitoring
                }
            }
        }

        // If we have any high/critical anomalies, we would have returned above
        // If only medium/low, continue normal flow
        None
    }

    /// Compute dynamic thresholds based on plan size and progress rate (Sprint 2).
    ///
    /// **Heuristics**:
    /// - Large plans (8+ steps): Allow more rounds (+2)
    /// - Good progress (>50% done): Be lenient (+1)
    /// - Stalled progress (0% after 3 rounds): Be strict (-1)
    ///
    /// Returns `(synthesis_threshold, force_threshold)`.
    fn compute_dynamic_thresholds(&self) -> (usize, usize) {
        if let Some(progress) = self.plan_progress {
            let total = progress.total.max(1);
            let progress_ratio = progress.completed as f64 / total as f64;
            let remaining_steps = total.saturating_sub(progress.completed);

            // B5 remediation: Plan-aware dynamic thresholds.
            //
            // Strategy: the synthesis threshold should give the agent enough rounds
            // to complete remaining plan steps. A plan with 12 steps and 50% done
            // needs at least 6 more rounds, not the default 6 synthesis threshold.
            //
            // Formula:
            //   synthesis = max(base, remaining_steps + 2)   — headroom of 2
            //   force     = synthesis + 3                     — always 3 rounds after synthesis hint
            //
            // Guards:
            //   - synth never below min_synthesis (from PolicyConfig)
            //   - force never below min_force (from PolicyConfig)
            //   - Stalled plans (0% after 3+ rounds) reduce thresholds to accelerate exit
            //   - Absolute cap: synth <= 20, force <= 25 (prevent infinite loops)

            let mut synth = self.synthesis_threshold; // base: 6
            let mut force = self.force_threshold; // base: 10

            // Adjustment 1: Extend thresholds based on remaining plan steps.
            // If there are 8 remaining steps, synthesis should be at least 10.
            let plan_aware_synth = remaining_steps + 2;
            if plan_aware_synth > synth {
                synth = plan_aware_synth;
                force = synth + 3;
            }

            // Adjustment 2: Good progress (>70%) — agent is converging, give more room.
            if progress_ratio > 0.7 {
                force += 2;
            }

            // Adjustment 3: Stalled (0% after 3+ rounds) — tighten to exit earlier.
            if self.consecutive_rounds >= 3 && progress.completed == 0 {
                synth = self.synthesis_threshold.saturating_sub(1);
                force = self.force_threshold.saturating_sub(1);
            }

            // Absolute caps to prevent runaway loops.
            synth = synth.min(20).max(self.min_synthesis);
            force = force.min(25).max(self.min_force);

            tracing::debug!(
                synth,
                force,
                plan_total = total,
                plan_completed = progress.completed,
                remaining_steps,
                progress_ratio,
                consecutive_rounds = self.consecutive_rounds,
                "B5: dynamic thresholds computed"
            );

            (synth, force)
        } else {
            // No plan → use default thresholds
            (self.synthesis_threshold, self.force_threshold)
        }
    }
}

/// Compute a deterministic hash of a serde_json::Value for dedup purposes.
pub(crate) fn hash_tool_args(value: &serde_json::Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    // Canonical JSON string for deterministic hashing.
    let canonical = value.to_string();
    canonical.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
pub(crate) use tests::READ_ONLY_TOOLS_LIST;

#[cfg(test)]
mod tests {
    use super::*;

    /// Expose READ_ONLY_TOOLS for test assertions in other modules.
    pub(crate) const READ_ONLY_TOOLS_LIST: &[&str] = READ_ONLY_TOOLS;

    // === Sprint 1: Reset on text round tests ===

    #[test]
    fn reset_on_text_round_zeros_counter() {
        let mut guard = ToolLoopGuard::new();
        guard.record_round(&[("file_read".into(), 1)]);
        guard.record_round(&[("grep".into(), 2)]);
        assert_eq!(guard.consecutive_rounds(), 2);

        guard.reset_on_text_round();
        assert_eq!(guard.consecutive_rounds(), 0);
    }

    #[test]
    fn reset_preserves_history_for_oscillation() {
        let mut guard = ToolLoopGuard::new();
        let tools = vec![("file_read".into(), 42u64)];
        guard.record_round(&tools);
        guard.record_round(&tools);
        assert_eq!(guard.consecutive_rounds(), 2);

        guard.reset_on_text_round();

        // History still has 2 entries for oscillation detection
        assert_eq!(guard.history.len(), 2);
        // Counter reset
        assert_eq!(guard.consecutive_rounds(), 0);

        // Third identical call should still detect oscillation
        let action = guard.record_round(&tools);
        assert_eq!(action, LoopAction::Break);
        assert!(guard.detect_oscillation());
    }

    #[test]
    fn alternating_tool_text_tool_no_false_positive() {
        let mut guard = ToolLoopGuard::new();

        // Round 1: Tool
        guard.record_round(&[("file_read".into(), 1)]);
        assert_eq!(guard.consecutive_rounds(), 1);

        // Round 2: Text (simulated via reset)
        guard.reset_on_text_round();
        assert_eq!(guard.consecutive_rounds(), 0);

        // Round 3: Tool (should restart at 1, not continue from 2)
        let action = guard.record_round(&[("grep".into(), 2)]);
        assert_eq!(action, LoopAction::Continue); // Not InjectSynthesis
        assert_eq!(guard.consecutive_rounds(), 1);
    }

    #[test]
    fn reset_idempotent() {
        let mut guard = ToolLoopGuard::new();
        guard.record_round(&[("file_read".into(), 1)]);

        guard.reset_on_text_round();
        let count1 = guard.consecutive_rounds();
        guard.reset_on_text_round();
        let count2 = guard.consecutive_rounds();

        assert_eq!(count1, 0);
        assert_eq!(count2, 0); // Multiple resets safe
    }

    #[test]
    fn reset_noop_when_already_zero() {
        let mut guard = ToolLoopGuard::new();
        assert_eq!(guard.consecutive_rounds(), 0);

        // Should be safe to call even when counter is 0
        guard.reset_on_text_round();
        assert_eq!(guard.consecutive_rounds(), 0);
    }

    // === Sprint 2: Plan-aware dynamic thresholds tests ===

    #[test]
    fn large_plan_gets_extended_thresholds() {
        let mut guard = ToolLoopGuard::new();
        guard.update_plan_progress(2, 10, 5000); // 10-step plan, 20% progress

        // B5: remaining_steps = 8, plan_aware_synth = 8+2 = 10, force = 10+3 = 13
        // progress_ratio = 0.2 (no >0.7 bonus), not stalled (completed=2>0)

        // Rounds 1-9: Continue (consecutive_rounds < synthesis threshold 10)
        for i in 1..=9 {
            let action = guard.record_round(&[(format!("tool{i}"), i as u64)]);
            assert_eq!(action, LoopAction::Continue, "Round {i} should continue");
        }

        // Round 10: InjectSynthesis (consecutive_rounds 10 >= synthesis threshold 10)
        let action = guard.record_round(&[("tool10".into(), 10)]);
        assert_eq!(action, LoopAction::InjectSynthesis);

        // Rounds 11-12: Still InjectSynthesis (>= 10 synth, < 13 force)
        for i in 11..=12 {
            let action = guard.record_round(&[(format!("tool{i}"), i as u64)]);
            assert_eq!(
                action,
                LoopAction::InjectSynthesis,
                "Round {i} should inject synthesis"
            );
        }

        // Round 13: ForceNoTools (force threshold 13)
        let action = guard.record_round(&[("tool13".into(), 13)]);
        assert_eq!(action, LoopAction::ForceNoTools);
    }

    #[test]
    fn good_progress_delays_force() {
        let mut guard = ToolLoopGuard::new();
        guard.update_plan_progress(6, 9, 10000); // 67% done, 9 steps

        // B5: remaining_steps = 3, plan_aware_synth = 3+2 = 5 < base 6 -> synth stays 6
        // force stays 10. progress_ratio = 0.67 (not >0.7), no extra bonus.

        // Rounds 1-5: Continue
        for i in 1..=5 {
            let action = guard.record_round(&[(format!("tool{i}"), i as u64)]);
            assert_eq!(action, LoopAction::Continue, "Round {i} should continue");
        }

        // Round 6: InjectSynthesis (synthesis threshold 6)
        let action = guard.record_round(&[("tool6".into(), 6)]);
        assert_eq!(action, LoopAction::InjectSynthesis);

        // Rounds 7-9: Still InjectSynthesis (>= 6 synth, < 10 force)
        for i in 7..=9 {
            let action = guard.record_round(&[(format!("tool{i}"), i as u64)]);
            assert_eq!(
                action,
                LoopAction::InjectSynthesis,
                "Round {i} should inject synthesis"
            );
        }

        // Round 10: ForceNoTools (force threshold 10)
        let action = guard.record_round(&[("tool10".into(), 10)]);
        assert_eq!(action, LoopAction::ForceNoTools);
    }

    #[test]
    fn stalled_progress_triggers_early() {
        let mut guard = ToolLoopGuard::new();
        guard.update_plan_progress(0, 5, 8000); // 0% after time elapsed

        // Stall logic: after consecutive_rounds >= 3 AND completed == 0,
        // force = 10 - 1 = 9, synth = 6 - 1 = 5.
        // Rounds 1-2: Continue
        guard.record_round(&[("read1".into(), 1)]);
        guard.record_round(&[("read2".into(), 2)]);

        // Rounds 3-4: InjectSynthesis (stalled: synth 5, 3 >= 5? No — 3 < 5 still Continue)
        // After round 3, consecutive_rounds == 3 >= 3, so stall kicks in for round 4+ threshold calc.
        guard.record_round(&[("read3".into(), 3)]);
        guard.record_round(&[("read4".into(), 4)]);

        // Round 5: InjectSynthesis (synth threshold = 5, consecutive_rounds = 5 >= 5)
        let action = guard.record_round(&[("read5".into(), 5)]);
        assert_eq!(action, LoopAction::InjectSynthesis); // Early synthesis at round 5

        // Rounds 6-8: Still InjectSynthesis (>= 5 synth, < 9 force)
        for i in 6..=8 {
            let action = guard.record_round(&[(format!("read{i}"), i as u64)]);
            assert_eq!(
                action,
                LoopAction::InjectSynthesis,
                "Round {i} should inject synthesis"
            );
        }

        // Round 9: ForceNoTools (force threshold = 9)
        let action = guard.record_round(&[("read9".into(), 9)]);
        assert_eq!(action, LoopAction::ForceNoTools); // Early force at round 9
    }

    #[test]
    fn no_plan_uses_defaults() {
        let mut guard = ToolLoopGuard::new();
        // No update_plan_progress() call

        // Rounds 1-5: Continue (< synthesis threshold 6)
        for i in 1..=5 {
            let action = guard.record_round(&[(format!("tool{i}"), i as u64)]);
            assert_eq!(action, LoopAction::Continue, "Round {i} should continue");
        }

        // Round 6: InjectSynthesis (default threshold 6)
        let action = guard.record_round(&[("tool6".into(), 6)]);
        assert_eq!(action, LoopAction::InjectSynthesis);
    }

    // === Sprint 3: Self-healing loop tests ===

    #[test]
    fn detect_stagnation_read_saturation_zero_progress() {
        let mut guard = ToolLoopGuard::new();
        guard.update_plan_progress(0, 5, 10000); // 0% progress, plan exists

        // 3 rounds of read-only tools (triggers read saturation)
        guard.record_round(&[("file_read".into(), 1)]);
        guard.record_round(&[("grep".into(), 2)]);

        // Third round: should detect stagnation (read saturation + 0% progress)
        let action = guard.record_round(&[("glob".into(), 3)]);

        assert_eq!(action, LoopAction::ReplanRequired);
        assert!(guard.detect_stagnation());
    }

    #[test]
    fn no_stagnation_if_progress_exists() {
        let mut guard = ToolLoopGuard::new();
        guard.update_plan_progress(2, 5, 10000); // 40% progress

        // 3 rounds of read-only tools (read saturation)
        guard.record_round(&[("file_read".into(), 1)]);
        guard.record_round(&[("grep".into(), 2)]);
        let action = guard.record_round(&[("glob".into(), 3)]);

        // Should NOT trigger ReplanRequired (progress exists)
        assert_ne!(action, LoopAction::ReplanRequired);
        assert!(!guard.detect_stagnation());
    }

    #[test]
    fn no_stagnation_without_read_saturation() {
        let mut guard = ToolLoopGuard::new();
        guard.update_plan_progress(0, 5, 10000); // 0% progress

        // Mix of read and write tools (no read saturation)
        guard.record_round(&[("file_read".into(), 1)]);
        guard.record_round(&[("file_write".into(), 2)]); // Write tool breaks saturation
        let action = guard.record_round(&[("grep".into(), 3)]);

        assert_ne!(action, LoopAction::ReplanRequired);
        assert!(!guard.detect_stagnation());
    }

    #[test]
    fn reset_on_replan_clears_all_state() {
        let mut guard = ToolLoopGuard::new();
        guard.record_round(&[("tool1".into(), 1)]);
        guard.record_round(&[("tool2".into(), 2)]);
        guard.update_plan_progress(1, 5, 5000);

        assert_eq!(guard.consecutive_rounds(), 2);
        assert_eq!(guard.history.len(), 2);

        guard.reset_on_replan();

        // All state cleared
        assert_eq!(guard.consecutive_rounds(), 0);
        assert_eq!(guard.history.len(), 0);
        assert!(!guard.plan_complete());
        // plan_progress retained (tracker will update next round)
        assert!(guard.plan_progress.is_some());
    }

    #[test]
    fn stagnation_takes_priority_over_synthesis() {
        let mut guard = ToolLoopGuard::new();
        guard.update_plan_progress(0, 5, 10000); // 0% progress

        // 3 rounds of read-only tools
        guard.record_round(&[("file_read".into(), 1)]);
        guard.record_round(&[("grep".into(), 2)]);
        let action = guard.record_round(&[("glob".into(), 3)]);

        // Should be ReplanRequired, NOT InjectSynthesis
        // (even though consecutive_rounds == 3 == synthesis_threshold)
        assert_eq!(action, LoopAction::ReplanRequired);
    }

    // === PlanExecuteReflect oscillation suppression tests ===

    #[test]
    fn cross_type_oscillation_suppressed_when_plan_advancing() {
        // When at least one plan step is completed, the Tool→Text alternation is the
        // expected PlanExecuteReflect pattern and must NOT be flagged as oscillation.
        let mut guard = ToolLoopGuard::new();
        guard.update_plan_progress(1, 5, 3000); // 1 step completed → plan is advancing

        // Build Tool→Text→Tool→Text via public API (PlanExecuteReflect pattern).
        guard.record_round(&[("file_read".into(), 1u64)]);
        guard.record_text_round();
        guard.record_round(&[("grep".into(), 2u64)]);
        guard.record_text_round(); // round_types = [Tool, Text, Tool, Text]

        // With progress.completed > 0, oscillation must be suppressed.
        assert!(
            !guard.detect_cross_type_oscillation(),
            "PlanExecuteReflect Tool↔Text alternation must not be flagged when plan advances"
        );
    }

    #[test]
    fn cross_type_oscillation_fires_when_plan_has_zero_progress() {
        // When plan exists but no step has been completed (completed==0), the Tool↔Text
        // alternation is a stuck loop and SHOULD be flagged.
        let mut guard = ToolLoopGuard::new();
        guard.update_plan_progress(0, 5, 3000); // 0 steps completed → plan stalled

        guard.record_round(&[("file_read".into(), 1u64)]);
        guard.record_text_round();
        guard.record_round(&[("grep".into(), 2u64)]);
        guard.record_text_round(); // round_types = [Tool, Text, Tool, Text]

        assert!(
            guard.detect_cross_type_oscillation(),
            "Tool↔Text alternation with 0% plan progress must be flagged as oscillation"
        );
    }

    #[test]
    fn cross_type_oscillation_fires_when_no_plan_tracker() {
        // When there is no plan at all (plan_progress == None), Tool↔Text alternation
        // is always suspicious — nothing drives convergence.
        let mut guard = ToolLoopGuard::new();
        // No update_plan_progress() call.

        guard.record_round(&[("file_read".into(), 1u64)]);
        guard.record_text_round();
        guard.record_round(&[("grep".into(), 2u64)]);
        guard.record_text_round();

        assert!(
            guard.detect_cross_type_oscillation(),
            "Tool↔Text alternation without any plan tracker must still be flagged"
        );
    }
}
