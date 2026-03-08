//! # HICON Phase 4: Agent Self-Correction Strategies
//!
//! Adaptive self-correction system that responds to Bayesian-detected anomalies
//! with targeted interventions. Each strategy addresses specific failure patterns
//! detected by the anomaly detector.

use super::anomaly_detector::{AgentAnomaly, AnomalySeverity};
use halcon_core::types::{ChatMessage, ContentBlock, MessageContent, Role};
use std::collections::HashMap;

/// Self-correction strategy to apply when anomaly detected.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CorrectionStrategy {
    /// Inject clarifying context into system prompt.
    ///
    /// Used when: Model appears confused or stuck.
    /// Effect: Adds focused guidance about current situation.
    InjectContext { context: String },

    /// Reduce plan complexity by simplifying steps.
    ///
    /// Used when: TokenExplosion or PlanOscillation detected.
    /// Effect: Condenses multi-step plan into fewer, clearer actions.
    SimplifyPlan { max_steps: usize },

    /// Replace problematic tool with alternative.
    ///
    /// Used when: ToolCycle detected (same tool failing repeatedly).
    /// Effect: Suggests alternative tool or approach.
    ChangeTools {
        avoid_tool: String,
        suggestion: String,
    },

    /// Force explicit reflection round.
    ///
    /// Used when: StagnantProgress or ReadSaturation detected.
    /// Effect: Injects reflection prompt to reassess approach.
    ForceReflection { reflection_prompt: String },

    /// Emergency plan regeneration.
    ///
    /// Used when: Critical failures across multiple dimensions.
    /// Effect: Signals complete plan reset needed.
    EmergencyReplan { reason: String },
}

/// Tracks correction history and effectiveness.
#[derive(Debug, Clone)]
struct CorrectionRecord {
    strategy: CorrectionStrategy,
    round_applied: usize,
    anomaly_severity: AnomalySeverity,
    /// Whether the correction succeeded (anomaly didn't recur in next 3 rounds).
    success: Option<bool>,
}

/// Agent self-corrector with adaptive strategy selection.
///
/// Analyzes detected anomalies and selects appropriate correction strategies.
/// Tracks effectiveness to improve future selections (reinforcement learning).
pub(crate) struct AgentSelfCorrector {
    /// History of applied corrections.
    correction_history: Vec<CorrectionRecord>,

    /// Effectiveness scores per strategy (success rate 0.0-1.0).
    strategy_effectiveness: HashMap<String, f64>,

    /// Number of corrections applied this session.
    corrections_applied: usize,

    /// Maximum corrections allowed per session (safety limit).
    max_corrections: usize,
}

impl AgentSelfCorrector {
    /// Create new self-corrector with default limits.
    pub(crate) fn new() -> Self {
        let mut strategy_effectiveness = HashMap::new();

        // Initialize with neutral effectiveness (0.5)
        strategy_effectiveness.insert("inject_context".to_string(), 0.5);
        strategy_effectiveness.insert("simplify_plan".to_string(), 0.5);
        strategy_effectiveness.insert("change_tools".to_string(), 0.5);
        strategy_effectiveness.insert("force_reflection".to_string(), 0.5);
        strategy_effectiveness.insert("emergency_replan".to_string(), 0.5);

        Self {
            correction_history: Vec::new(),
            strategy_effectiveness,
            corrections_applied: 0,
            max_corrections: 10, // Safety limit
        }
    }

    /// Select appropriate correction strategy for detected anomaly.
    ///
    /// Uses pattern matching + effectiveness scores to choose best strategy.
    pub(crate) fn select_strategy(
        &self,
        anomaly: &AgentAnomaly,
        severity: AnomalySeverity,
        round: usize,
    ) -> Option<CorrectionStrategy> {
        // Safety check: don't exceed correction limit
        if self.corrections_applied >= self.max_corrections {
            tracing::warn!(
                corrections_applied = self.corrections_applied,
                max = self.max_corrections,
                "Correction limit reached — skipping further interventions"
            );
            return None;
        }

        // Match anomaly type to appropriate strategy
        let strategy = match anomaly {
            AgentAnomaly::ToolCycle {
                tool,
                target,
                occurrences,
            } => {
                tracing::info!(
                    tool = %tool,
                    target = %target,
                    occurrences = occurrences,
                    round,
                    "ToolCycle detected — suggesting tool change"
                );
                CorrectionStrategy::ChangeTools {
                    avoid_tool: tool.clone(),
                    suggestion: format!(
                        "Tool '{}' has been called {} times on '{}' without progress. \
                         Consider using a different approach or tool.",
                        tool, occurrences, target
                    ),
                }
            }

            AgentAnomaly::PlanOscillation {
                plan_a_hash,
                plan_b_hash,
                switches,
            } => {
                tracing::warn!(
                    plan_a = plan_a_hash,
                    plan_b = plan_b_hash,
                    switches,
                    round,
                    "PlanOscillation detected — simplifying plan"
                );
                // For high severity, force emergency replan
                if matches!(severity, AnomalySeverity::Critical) {
                    CorrectionStrategy::EmergencyReplan {
                        reason: format!(
                            "Plan oscillation detected ({} switches between strategies). \
                             Regenerating with clearer goal.",
                            switches
                        ),
                    }
                } else {
                    CorrectionStrategy::SimplifyPlan { max_steps: 3 }
                }
            }

            AgentAnomaly::ReadSaturation {
                consecutive_rounds,
                probability,
            } => {
                tracing::info!(
                    consecutive_rounds,
                    probability,
                    round,
                    "ReadSaturation detected — forcing reflection"
                );
                CorrectionStrategy::ForceReflection {
                    reflection_prompt: format!(
                        "You've been reading/exploring for {} consecutive rounds. \
                         Reflect: What information are you still missing? \
                         What concrete action can you take with what you already know?",
                        consecutive_rounds
                    ),
                }
            }

            AgentAnomaly::TokenExplosion {
                growth_rate,
                projected_overflow,
                current_tokens,
            } => {
                tracing::warn!(
                    growth_rate,
                    projected_overflow,
                    current_tokens,
                    round,
                    "TokenExplosion detected — injecting budget awareness"
                );
                CorrectionStrategy::InjectContext {
                    context: format!(
                        "⚠️ Token budget constraint: You have {} tokens currently, \
                         growing at {:.1}% per round. Projected overflow in {} rounds. \
                         Be concise and focus on essential actions only.",
                        current_tokens, growth_rate * 100.0, projected_overflow
                    ),
                }
            }

            AgentAnomaly::StagnantProgress {
                rounds_without_progress,
                repeated_errors,
            } => {
                tracing::warn!(
                    rounds_without_progress,
                    repeated_errors_count = repeated_errors.len(),
                    round,
                    "StagnantProgress detected — forcing reflection"
                );
                let error_summary = if !repeated_errors.is_empty() {
                    let top_errors: Vec<_> = repeated_errors
                        .iter()
                        .take(3)
                        .map(|(err, count)| format!("'{}' ({} times)", err, count))
                        .collect();
                    format!("Repeated errors: {}", top_errors.join(", "))
                } else {
                    "No progress for multiple rounds".to_string()
                };

                // For critical severity, trigger emergency replan
                if matches!(severity, AnomalySeverity::Critical) {
                    CorrectionStrategy::EmergencyReplan {
                        reason: format!(
                            "Stagnant progress ({} rounds). {}. Regenerating plan with different approach.",
                            rounds_without_progress, error_summary
                        ),
                    }
                } else {
                    CorrectionStrategy::ForceReflection {
                        reflection_prompt: format!(
                            "You've made no progress for {} rounds. {}. \
                             Reflect: What assumptions might be wrong? \
                             What completely different approach could you try?",
                            rounds_without_progress, error_summary
                        ),
                    }
                }
            }
        };

        Some(strategy)
    }

    /// Apply correction strategy to conversation context.
    ///
    /// Modifies system prompt or injects user message as needed.
    /// Returns: (modified_system_prompt, injected_message)
    pub(crate) fn apply_strategy(
        &mut self,
        strategy: CorrectionStrategy,
        current_system: &str,
        round: usize,
        severity: AnomalySeverity,
    ) -> (Option<String>, Option<ChatMessage>) {
        self.corrections_applied += 1;

        // Record correction
        self.correction_history.push(CorrectionRecord {
            strategy: strategy.clone(),
            round_applied: round,
            anomaly_severity: severity,
            success: None, // Will be evaluated after 3 rounds
        });

        tracing::info!(
            strategy = ?strategy,
            round,
            corrections_applied = self.corrections_applied,
            "Applying self-correction strategy"
        );

        match strategy {
            CorrectionStrategy::InjectContext { context } => {
                // Append context to system prompt
                let new_system = format!(
                    "{}\n\n## IMPORTANT CONSTRAINT\n{}\n",
                    current_system, context
                );
                (Some(new_system), None)
            }

            CorrectionStrategy::SimplifyPlan { max_steps } => {
                // Inject message suggesting plan simplification
                let msg = ChatMessage {
                    role: Role::User,
                    content: MessageContent::Blocks(vec![ContentBlock::Text {
                        text: format!(
                            "Your current plan appears too complex. Please regenerate with at most {} clear, focused steps. \
                             Prioritize the most essential actions only.",
                            max_steps
                        ),
                    }]),
                };
                (None, Some(msg))
            }

            CorrectionStrategy::ChangeTools {
                avoid_tool,
                suggestion,
            } => {
                // Inject message discouraging problematic tool
                let msg = ChatMessage {
                    role: Role::User,
                    content: MessageContent::Blocks(vec![ContentBlock::Text {
                        text: format!(
                            "⚠️ Avoid using the '{}' tool further. {}",
                            avoid_tool, suggestion
                        ),
                    }]),
                };
                (None, Some(msg))
            }

            CorrectionStrategy::ForceReflection { reflection_prompt } => {
                // Inject reflection prompt as user message
                let msg = ChatMessage {
                    role: Role::User,
                    content: MessageContent::Blocks(vec![ContentBlock::Text {
                        text: reflection_prompt,
                    }]),
                };
                (None, Some(msg))
            }

            CorrectionStrategy::EmergencyReplan { reason } => {
                // Both system prompt update AND user message
                let new_system = format!(
                    "{}\n\n## EMERGENCY REPLAN REQUIRED\n{}\n",
                    current_system, reason
                );
                let msg = ChatMessage {
                    role: Role::User,
                    content: MessageContent::Blocks(vec![ContentBlock::Text {
                        text: format!(
                            "🚨 Emergency replan required: {}. \
                             Regenerate a completely new plan with a different approach.",
                            reason
                        ),
                    }]),
                };
                (Some(new_system), Some(msg))
            }
        }
    }

    /// Update effectiveness score for a correction (called after observing outcome).
    pub(crate) fn record_outcome(&mut self, correction_index: usize, success: bool) {
        if let Some(record) = self.correction_history.get_mut(correction_index) {
            record.success = Some(success);

            // Update strategy effectiveness using exponential moving average
            let strategy_key = match &record.strategy {
                CorrectionStrategy::InjectContext { .. } => "inject_context",
                CorrectionStrategy::SimplifyPlan { .. } => "simplify_plan",
                CorrectionStrategy::ChangeTools { .. } => "change_tools",
                CorrectionStrategy::ForceReflection { .. } => "force_reflection",
                CorrectionStrategy::EmergencyReplan { .. } => "emergency_replan",
            };

            let current_score = self
                .strategy_effectiveness
                .get(strategy_key)
                .copied()
                .unwrap_or(0.5);

            // EMA: new_score = 0.8 * current + 0.2 * outcome
            let outcome_value = if success { 1.0 } else { 0.0 };
            let new_score = 0.8 * current_score + 0.2 * outcome_value;

            self.strategy_effectiveness
                .insert(strategy_key.to_string(), new_score);

            tracing::debug!(
                strategy = strategy_key,
                success,
                new_effectiveness = new_score,
                "Updated correction strategy effectiveness"
            );
        }
    }

    /// Get correction statistics for diagnostics.
    pub(crate) fn stats(&self) -> CorrectionStats {
        let total = self.correction_history.len();
        let successful = self
            .correction_history
            .iter()
            .filter(|r| r.success == Some(true))
            .count();

        CorrectionStats {
            total_corrections: total,
            successful_corrections: successful,
            success_rate: if total > 0 {
                successful as f64 / total as f64
            } else {
                0.0
            },
            strategy_effectiveness: self.strategy_effectiveness.clone(),
        }
    }

    /// Reset corrector state (for new session).
    pub(crate) fn reset(&mut self) {
        self.correction_history.clear();
        self.corrections_applied = 0;
    }
}

/// Statistics about correction effectiveness.
#[derive(Debug, Clone)]
pub(crate) struct CorrectionStats {
    pub total_corrections: usize,
    pub successful_corrections: usize,
    pub success_rate: f64,
    pub strategy_effectiveness: HashMap<String, f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_corrector_initialization() {
        let corrector = AgentSelfCorrector::new();
        assert_eq!(corrector.corrections_applied, 0);
        assert_eq!(corrector.correction_history.len(), 0);
        assert_eq!(corrector.strategy_effectiveness.len(), 5);
    }

    #[test]
    fn test_select_strategy_tool_cycle() {
        let corrector = AgentSelfCorrector::new();
        let anomaly = AgentAnomaly::ToolCycle {
            tool: "file_read".to_string(),
            target: "test.txt".to_string(),
            occurrences: 5,
        };

        let strategy = corrector.select_strategy(&anomaly, AnomalySeverity::High, 3);
        assert!(matches!(
            strategy,
            Some(CorrectionStrategy::ChangeTools { .. })
        ));
    }

    #[test]
    fn test_select_strategy_read_saturation() {
        let corrector = AgentSelfCorrector::new();
        let anomaly = AgentAnomaly::ReadSaturation {
            consecutive_rounds: 4,
            probability: 0.85,
        };

        let strategy = corrector.select_strategy(&anomaly, AnomalySeverity::Medium, 5);
        assert!(matches!(
            strategy,
            Some(CorrectionStrategy::ForceReflection { .. })
        ));
    }

    #[test]
    fn test_select_strategy_plan_oscillation_critical() {
        let corrector = AgentSelfCorrector::new();
        let anomaly = AgentAnomaly::PlanOscillation {
            plan_a_hash: 123,
            plan_b_hash: 456,
            switches: 4,
        };

        let strategy = corrector.select_strategy(&anomaly, AnomalySeverity::Critical, 6);
        assert!(matches!(
            strategy,
            Some(CorrectionStrategy::EmergencyReplan { .. })
        ));
    }

    #[test]
    fn test_apply_inject_context() {
        let mut corrector = AgentSelfCorrector::new();
        let strategy = CorrectionStrategy::InjectContext {
            context: "Be concise.".to_string(),
        };

        let (system, message) = corrector.apply_strategy(
            strategy,
            "Original system prompt",
            1,
            AnomalySeverity::Medium,
        );

        assert!(system.is_some());
        assert!(system.unwrap().contains("Be concise"));
        assert!(message.is_none());
        assert_eq!(corrector.corrections_applied, 1);
    }

    #[test]
    fn test_apply_force_reflection() {
        let mut corrector = AgentSelfCorrector::new();
        let strategy = CorrectionStrategy::ForceReflection {
            reflection_prompt: "Think harder.".to_string(),
        };

        let (system, message) =
            corrector.apply_strategy(strategy, "System", 2, AnomalySeverity::Low);

        assert!(system.is_none());
        assert!(message.is_some());
        let msg = message.unwrap();
        assert_eq!(msg.role, Role::User);
    }

    #[test]
    fn test_record_outcome_updates_effectiveness() {
        let mut corrector = AgentSelfCorrector::new();
        let strategy = CorrectionStrategy::SimplifyPlan { max_steps: 3 };

        corrector.apply_strategy(strategy, "System", 1, AnomalySeverity::Medium);

        let initial_score = corrector
            .strategy_effectiveness
            .get("simplify_plan")
            .copied()
            .unwrap();

        corrector.record_outcome(0, true); // First correction succeeded

        let updated_score = corrector
            .strategy_effectiveness
            .get("simplify_plan")
            .copied()
            .unwrap();

        assert!(updated_score > initial_score);
    }

    #[test]
    fn test_correction_limit_enforcement() {
        let mut corrector = AgentSelfCorrector::new();
        corrector.max_corrections = 2;

        let anomaly = AgentAnomaly::ReadSaturation {
            consecutive_rounds: 3,
            probability: 0.8,
        };

        // First two should succeed
        assert!(corrector
            .select_strategy(&anomaly, AnomalySeverity::Medium, 1)
            .is_some());
        corrector.apply_strategy(
            CorrectionStrategy::ForceReflection {
                reflection_prompt: "Reflect 1".to_string(),
            },
            "System",
            1,
            AnomalySeverity::Medium,
        );

        assert!(corrector
            .select_strategy(&anomaly, AnomalySeverity::Medium, 2)
            .is_some());
        corrector.apply_strategy(
            CorrectionStrategy::ForceReflection {
                reflection_prompt: "Reflect 2".to_string(),
            },
            "System",
            2,
            AnomalySeverity::Medium,
        );

        // Third should fail (limit reached)
        assert!(corrector
            .select_strategy(&anomaly, AnomalySeverity::Medium, 3)
            .is_none());
    }

    #[test]
    fn test_stats_calculation() {
        let mut corrector = AgentSelfCorrector::new();

        corrector.apply_strategy(
            CorrectionStrategy::SimplifyPlan { max_steps: 3 },
            "System",
            1,
            AnomalySeverity::Medium,
        );
        corrector.apply_strategy(
            CorrectionStrategy::InjectContext {
                context: "Context".to_string(),
            },
            "System",
            2,
            AnomalySeverity::Low,
        );

        corrector.record_outcome(0, true);
        corrector.record_outcome(1, false);

        let stats = corrector.stats();
        assert_eq!(stats.total_corrections, 2);
        assert_eq!(stats.successful_corrections, 1);
        assert_eq!(stats.success_rate, 0.5);
    }
}
