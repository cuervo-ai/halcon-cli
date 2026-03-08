//! # HICON Phase 6: Metacognitive Loop with Φ Coherence
//!
//! Complete metacognitive cycle implementing self-awareness, adaptation, and system
//! integration monitoring. Uses IIT (Integrated Information Theory) Φ metric to
//! measure coherence quality.

use super::anomaly_detector::{AgentAnomaly, AnomalySeverity};
use super::self_corrector::CorrectionStats;
use std::collections::HashMap;

/// Metacognitive cycle phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MetacognitivePhase {
    /// Phase 1: Monitoring — Gather metrics from all subsystems.
    Monitoring,
    /// Phase 2: Analysis — Identify patterns and anomalies.
    Analysis,
    /// Phase 3: Adaptation — Apply corrections and adjustments.
    Adaptation,
    /// Phase 4: Reflection — Evaluate effectiveness of actions.
    Reflection,
    /// Phase 5: Integration — Update system-wide understanding.
    Integration,
}

/// System component being monitored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SystemComponent {
    /// Bayesian anomaly detector.
    AnomalyDetector,
    /// Self-correction strategy system.
    SelfCorrector,
    /// ARIMA resource predictor.
    ResourcePredictor,
    /// Loop guard (tool loop termination).
    LoopGuard,
    /// Context pipeline (L0-L4 memory).
    ContextPipeline,
}

/// Metacognitive observation from a system component.
#[derive(Debug, Clone)]
pub(crate) struct ComponentObservation {
    /// Which component produced this observation.
    pub component: SystemComponent,
    /// Observation timestamp (round number).
    pub round: usize,
    /// Component-specific metrics.
    pub metrics: HashMap<String, f64>,
    /// Component health status (0.0-1.0).
    pub health: f64,
}

/// IIT Φ (phi) coherence metric.
///
/// Measures integration and differentiation of information across system components.
/// Higher Φ indicates better system coherence and conscious processing.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PhiCoherence {
    /// Integration score (0.0-1.0) — components working together.
    pub integration: f64,
    /// Differentiation score (0.0-1.0) — components have distinct roles.
    pub differentiation: f64,
    /// Overall Φ value (geometric mean of integration × differentiation).
    pub phi: f64,
    /// Timestamp of calculation.
    pub round: usize,
}

impl PhiCoherence {
    /// Calculate Φ from integration and differentiation scores.
    pub(crate) fn new(round: usize, integration: f64, differentiation: f64) -> Self {
        // Φ = sqrt(integration * differentiation)
        // Geometric mean ensures both dimensions are required
        let phi = (integration * differentiation).sqrt();

        Self {
            integration,
            differentiation,
            phi,
            round,
        }
    }

    /// Check if Φ meets target threshold (>0.7).
    pub(crate) fn meets_target(&self) -> bool {
        self.phi > 0.7
    }

    /// Get quality rating based on Φ value.
    pub(crate) fn quality(&self) -> PhiQuality {
        if self.phi >= 0.9 {
            PhiQuality::Excellent
        } else if self.phi >= 0.7 {
            PhiQuality::Good
        } else if self.phi >= 0.5 {
            PhiQuality::Fair
        } else {
            PhiQuality::Poor
        }
    }
}

/// Quality rating for Φ coherence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PhiQuality {
    /// Φ ≥ 0.9 — Excellent coherence.
    Excellent,
    /// Φ ≥ 0.7 — Good coherence (target achieved).
    Good,
    /// Φ ≥ 0.5 — Fair coherence (needs improvement).
    Fair,
    /// Φ < 0.5 — Poor coherence (intervention needed).
    Poor,
}

/// Metacognitive loop orchestrator.
///
/// Implements the 5-phase cycle: Monitoring → Analysis → Adaptation → Reflection → Integration.
pub(crate) struct MetacognitiveLoop {
    /// Current phase of the cycle.
    current_phase: MetacognitivePhase,

    /// Observations collected during monitoring phase.
    observations: Vec<ComponentObservation>,

    /// Φ coherence history (last 10 calculations).
    phi_history: Vec<PhiCoherence>,

    /// Component interaction matrix (for integration calculation).
    ///
    /// interaction[i][j] = strength of connection from component i to j.
    interaction_matrix: HashMap<SystemComponent, HashMap<SystemComponent, f64>>,

    /// Last round when full cycle completed.
    last_cycle_round: usize,

    /// Cycle interval (rounds between full cycles).
    cycle_interval: usize,
}

impl MetacognitiveLoop {
    /// Create new metacognitive loop.
    pub(crate) fn new() -> Self {
        Self {
            current_phase: MetacognitivePhase::Monitoring,
            observations: Vec::new(),
            phi_history: Vec::new(),
            interaction_matrix: HashMap::new(),
            last_cycle_round: 0,
            cycle_interval: 10, // Full cycle every 10 rounds
        }
    }

    /// Check if it's time to run a full metacognitive cycle.
    pub(crate) fn should_run_cycle(&self, round: usize) -> bool {
        round >= self.last_cycle_round + self.cycle_interval
    }

    /// Phase 1: Monitoring — Collect observations from all components.
    pub(crate) fn monitor(&mut self, observation: ComponentObservation) {
        self.observations.push(observation);
        self.current_phase = MetacognitivePhase::Monitoring;
    }

    /// Phase 2: Analysis — Analyze collected observations for patterns.
    pub(crate) fn analyze(&mut self, round: usize) -> AnalysisResult {
        self.current_phase = MetacognitivePhase::Analysis;

        if self.observations.is_empty() {
            return AnalysisResult {
                round,
                component_health: HashMap::new(),
                detected_patterns: Vec::new(),
                integration_score: 0.0,
                differentiation_score: 0.0,
            };
        }

        // Aggregate health scores per component
        let mut component_health: HashMap<SystemComponent, f64> = HashMap::new();
        for obs in &self.observations {
            component_health.insert(obs.component, obs.health);
        }

        // Detect patterns in observations
        let patterns = self.detect_patterns();

        // Calculate integration score (components working together)
        let integration = self.calculate_integration(&component_health);

        // Calculate differentiation score (components have distinct roles)
        let differentiation = self.calculate_differentiation();

        AnalysisResult {
            round,
            component_health,
            detected_patterns: patterns,
            integration_score: integration,
            differentiation_score: differentiation,
        }
    }

    /// Phase 3: Adaptation — Apply adjustments based on analysis.
    pub(crate) fn adapt(&mut self, analysis: &AnalysisResult) -> AdaptationPlan {
        self.current_phase = MetacognitivePhase::Adaptation;

        let mut adjustments = Vec::new();

        // Low health components need attention
        for (component, &health) in &analysis.component_health {
            if health < 0.5 {
                adjustments.push(Adjustment {
                    component: *component,
                    action: AdjustmentAction::IncreaseMonitoring,
                    reason: format!("Low health: {:.2}", health),
                });
            }
        }

        // Low integration → increase communication
        if analysis.integration_score < 0.6 {
            adjustments.push(Adjustment {
                component: SystemComponent::ContextPipeline,
                action: AdjustmentAction::EnhanceCommunication,
                reason: format!("Low integration: {:.2}", analysis.integration_score),
            });
        }

        // Low differentiation → clarify roles
        if analysis.differentiation_score < 0.6 {
            adjustments.push(Adjustment {
                component: SystemComponent::SelfCorrector,
                action: AdjustmentAction::ClarifyRole,
                reason: format!("Low differentiation: {:.2}", analysis.differentiation_score),
            });
        }

        AdaptationPlan {
            round: analysis.round,
            adjustments,
        }
    }

    /// Phase 4: Reflection — Evaluate effectiveness of adaptations.
    pub(crate) fn reflect(&mut self, _plan: &AdaptationPlan) -> ReflectionInsight {
        self.current_phase = MetacognitivePhase::Reflection;

        // Calculate Φ from latest analysis
        let phi = if let Some(latest_analysis) = self.get_latest_analysis() {
            PhiCoherence::new(
                latest_analysis.round,
                latest_analysis.integration_score,
                latest_analysis.differentiation_score,
            )
        } else {
            PhiCoherence::new(0, 0.0, 0.0)
        };

        // Store in history
        self.phi_history.push(phi);
        if self.phi_history.len() > 10 {
            self.phi_history.remove(0);
        }

        // Trend analysis
        let trend = if self.phi_history.len() >= 2 {
            let latest = self.phi_history.last().unwrap().phi;
            let previous = self.phi_history[self.phi_history.len() - 2].phi;
            let delta = latest - previous;

            if delta > 0.05 {
                PhiTrend::Improving
            } else if delta < -0.05 {
                PhiTrend::Declining
            } else {
                PhiTrend::Stable
            }
        } else {
            PhiTrend::Insufficient
        };

        ReflectionInsight {
            phi,
            trend,
            meets_target: phi.meets_target(),
        }
    }

    /// Phase 5: Integration — Update system-wide understanding.
    pub(crate) fn integrate(&mut self, insight: &ReflectionInsight, round: usize) {
        self.current_phase = MetacognitivePhase::Integration;

        // Update interaction matrix based on observations
        self.update_interaction_matrix();

        // Mark cycle as complete
        self.last_cycle_round = round;

        // Clear observations for next cycle
        self.observations.clear();

        tracing::info!(
            round,
            phi = insight.phi.phi,
            quality = ?insight.phi.quality(),
            trend = ?insight.trend,
            "Metacognitive cycle complete"
        );
    }

    /// Get current Φ coherence value.
    pub(crate) fn current_phi(&self) -> Option<f64> {
        self.phi_history.last().map(|p| p.phi)
    }

    /// Get average Φ over history.
    pub(crate) fn average_phi(&self) -> Option<f64> {
        if self.phi_history.is_empty() {
            None
        } else {
            let sum: f64 = self.phi_history.iter().map(|p| p.phi).sum();
            Some(sum / self.phi_history.len() as f64)
        }
    }

    // === Private Methods ===

    /// Detect patterns in observations.
    fn detect_patterns(&self) -> Vec<String> {
        let mut patterns = Vec::new();

        // Pattern: All components have low health
        let avg_health: f64 = self.observations.iter()
            .map(|obs| obs.health)
            .sum::<f64>() / self.observations.len() as f64;

        if avg_health < 0.5 {
            patterns.push("System-wide low health".to_string());
        }

        // Pattern: Specific component always failing
        let mut component_failures: HashMap<SystemComponent, usize> = HashMap::new();
        for obs in &self.observations {
            if obs.health < 0.3 {
                *component_failures.entry(obs.component).or_insert(0) += 1;
            }
        }

        for (component, count) in component_failures {
            if count >= 3 {
                patterns.push(format!("{:?} consistently failing", component));
            }
        }

        patterns
    }

    /// Calculate integration score (0.0-1.0).
    ///
    /// Measures how well components work together via interaction strength.
    fn calculate_integration(&self, health: &HashMap<SystemComponent, f64>) -> f64 {
        if health.len() < 2 {
            return 0.0;
        }

        // Integration = average health × average interaction strength
        let avg_health: f64 = health.values().sum::<f64>() / health.len() as f64;

        let interaction_strength = if !self.interaction_matrix.is_empty() {
            let total: f64 = self.interaction_matrix.values()
                .flat_map(|inner| inner.values())
                .sum();
            let count = self.interaction_matrix.values()
                .map(|inner| inner.len())
                .sum::<usize>() as f64;
            if count > 0.0 {
                total / count
            } else {
                0.5 // Default if no interactions recorded
            }
        } else {
            0.5 // Default
        };

        (avg_health * interaction_strength).min(1.0)
    }

    /// Calculate differentiation score (0.0-1.0).
    ///
    /// Measures how distinct component roles are via metric variance.
    fn calculate_differentiation(&self) -> f64 {
        if self.observations.len() < 2 {
            return 0.0;
        }

        // Differentiation = variance in component metrics
        // High variance = components doing different things (good)

        let mut all_metric_values: Vec<f64> = Vec::new();
        for obs in &self.observations {
            all_metric_values.extend(obs.metrics.values());
        }

        if all_metric_values.is_empty() {
            return 0.5; // Default
        }

        let mean = all_metric_values.iter().sum::<f64>() / all_metric_values.len() as f64;
        let variance = all_metric_values.iter()
            .map(|v| (v - mean).powi(2))
            .sum::<f64>() / all_metric_values.len() as f64;

        // Normalize variance to [0, 1] range (use sigmoid)
        let normalized = 1.0 - (-variance).exp();
        normalized.min(1.0)
    }

    /// Update interaction matrix based on recent observations.
    fn update_interaction_matrix(&mut self) {
        // Simple heuristic: components observed together have stronger interaction
        for i in 0..self.observations.len() {
            for j in (i + 1)..self.observations.len() {
                let comp_i = self.observations[i].component;
                let comp_j = self.observations[j].component;

                // Increase interaction strength
                let current = self.interaction_matrix
                    .entry(comp_i)
                    .or_insert_with(HashMap::new)
                    .entry(comp_j)
                    .or_insert(0.5);

                *current = (*current * 0.9 + 0.1).min(1.0); // EMA increase
            }
        }
    }

    /// Get latest analysis from cache (simplified, would need storage).
    fn get_latest_analysis(&self) -> Option<AnalysisResult> {
        // In real implementation, would cache analysis results
        // For now, reconstruct from observations
        if self.observations.is_empty() {
            return None;
        }

        let round = self.observations.last()?.round;
        let mut component_health = HashMap::new();
        for obs in &self.observations {
            component_health.insert(obs.component, obs.health);
        }

        let integration = self.calculate_integration(&component_health);
        let differentiation = self.calculate_differentiation();

        Some(AnalysisResult {
            round,
            component_health,
            detected_patterns: Vec::new(),
            integration_score: integration,
            differentiation_score: differentiation,
        })
    }
}

/// Result of analysis phase.
#[derive(Debug, Clone)]
pub(crate) struct AnalysisResult {
    pub round: usize,
    pub component_health: HashMap<SystemComponent, f64>,
    pub detected_patterns: Vec<String>,
    pub integration_score: f64,
    pub differentiation_score: f64,
}

/// Adaptation plan with recommended adjustments.
#[derive(Debug, Clone)]
pub(crate) struct AdaptationPlan {
    pub round: usize,
    pub adjustments: Vec<Adjustment>,
}

/// Single adjustment to a component.
#[derive(Debug, Clone)]
pub(crate) struct Adjustment {
    pub component: SystemComponent,
    pub action: AdjustmentAction,
    pub reason: String,
}

/// Action to take for adaptation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AdjustmentAction {
    /// Increase monitoring frequency for component.
    IncreaseMonitoring,
    /// Enhance communication between components.
    EnhanceCommunication,
    /// Clarify component's role in system.
    ClarifyRole,
    /// Reset component state.
    Reset,
}

/// Reflection insight from Φ analysis.
#[derive(Debug, Clone)]
pub(crate) struct ReflectionInsight {
    /// Current Φ coherence.
    pub phi: PhiCoherence,
    /// Trend in Φ values.
    pub trend: PhiTrend,
    /// Whether target Φ > 0.7 achieved.
    pub meets_target: bool,
}

/// Trend in Φ coherence over time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PhiTrend {
    /// Φ increasing (good).
    Improving,
    /// Φ stable (acceptable).
    Stable,
    /// Φ decreasing (intervention needed).
    Declining,
    /// Insufficient data for trend.
    Insufficient,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phi_coherence_calculation() {
        let phi = PhiCoherence::new(1, 0.8, 0.9);
        assert!((phi.phi - 0.8485).abs() < 0.001); // sqrt(0.8 * 0.9) ≈ 0.8485
        assert_eq!(phi.quality(), PhiQuality::Good);
    }

    #[test]
    fn test_phi_meets_target() {
        let good_phi = PhiCoherence::new(1, 0.9, 0.9);
        assert!(good_phi.meets_target());

        let bad_phi = PhiCoherence::new(1, 0.5, 0.6);
        assert!(!bad_phi.meets_target());
    }

    #[test]
    fn test_metacognitive_loop_creation() {
        let loop_ = MetacognitiveLoop::new();
        assert_eq!(loop_.current_phase, MetacognitivePhase::Monitoring);
        assert_eq!(loop_.observations.len(), 0);
    }

    #[test]
    fn test_monitor_phase() {
        let mut loop_ = MetacognitiveLoop::new();

        let obs = ComponentObservation {
            component: SystemComponent::AnomalyDetector,
            round: 1,
            metrics: HashMap::new(),
            health: 0.9,
        };

        loop_.monitor(obs);
        assert_eq!(loop_.observations.len(), 1);
        assert_eq!(loop_.current_phase, MetacognitivePhase::Monitoring);
    }

    #[test]
    fn test_analysis_phase() {
        let mut loop_ = MetacognitiveLoop::new();

        // Add observations
        loop_.monitor(ComponentObservation {
            component: SystemComponent::AnomalyDetector,
            round: 1,
            metrics: [("accuracy".to_string(), 0.95)].into_iter().collect(),
            health: 0.9,
        });

        loop_.monitor(ComponentObservation {
            component: SystemComponent::SelfCorrector,
            round: 1,
            metrics: [("success_rate".to_string(), 0.85)].into_iter().collect(),
            health: 0.8,
        });

        let analysis = loop_.analyze(1);
        assert_eq!(analysis.component_health.len(), 2);
        assert!(analysis.integration_score > 0.0);
        assert!(analysis.differentiation_score > 0.0);
    }

    #[test]
    fn test_adaptation_low_health() {
        let mut loop_ = MetacognitiveLoop::new();

        loop_.monitor(ComponentObservation {
            component: SystemComponent::LoopGuard,
            round: 1,
            metrics: HashMap::new(),
            health: 0.3, // Low health
        });

        let analysis = loop_.analyze(1);
        let plan = loop_.adapt(&analysis);

        assert!(!plan.adjustments.is_empty());
        assert!(plan.adjustments.iter().any(|adj|
            matches!(adj.action, AdjustmentAction::IncreaseMonitoring)
        ));
    }

    #[test]
    fn test_reflection_phase() {
        let mut loop_ = MetacognitiveLoop::new();

        loop_.monitor(ComponentObservation {
            component: SystemComponent::AnomalyDetector,
            round: 1,
            metrics: HashMap::new(),
            health: 0.9,
        });

        let analysis = loop_.analyze(1);
        let plan = loop_.adapt(&analysis);
        let insight = loop_.reflect(&plan);

        assert!(insight.phi.phi >= 0.0 && insight.phi.phi <= 1.0);
    }

    #[test]
    fn test_integration_phase() {
        let mut loop_ = MetacognitiveLoop::new();

        loop_.monitor(ComponentObservation {
            component: SystemComponent::AnomalyDetector,
            round: 10,
            metrics: HashMap::new(),
            health: 0.9,
        });

        let analysis = loop_.analyze(10);
        let plan = loop_.adapt(&analysis);
        let insight = loop_.reflect(&plan);

        loop_.integrate(&insight, 10);

        assert_eq!(loop_.last_cycle_round, 10);
        assert_eq!(loop_.observations.len(), 0); // Cleared
        assert_eq!(loop_.current_phase, MetacognitivePhase::Integration);
    }

    #[test]
    fn test_phi_history_tracking() {
        let mut loop_ = MetacognitiveLoop::new();

        for round in 1..=5 {
            loop_.monitor(ComponentObservation {
                component: SystemComponent::AnomalyDetector,
                round,
                metrics: HashMap::new(),
                health: 0.9,
            });

            let analysis = loop_.analyze(round);
            let plan = loop_.adapt(&analysis);
            let insight = loop_.reflect(&plan);
            loop_.integrate(&insight, round);
        }

        assert_eq!(loop_.phi_history.len(), 5);
        assert!(loop_.average_phi().is_some());
    }

    #[test]
    fn test_should_run_cycle() {
        let loop_ = MetacognitiveLoop::new();

        assert!(!loop_.should_run_cycle(5)); // Too early
        assert!(loop_.should_run_cycle(10)); // Exactly at interval
        assert!(loop_.should_run_cycle(15)); // Past interval
    }

    #[test]
    fn test_pattern_detection_system_low_health() {
        let mut loop_ = MetacognitiveLoop::new();

        // All components low health
        for component in [
            SystemComponent::AnomalyDetector,
            SystemComponent::SelfCorrector,
            SystemComponent::LoopGuard,
        ] {
            loop_.monitor(ComponentObservation {
                component,
                round: 1,
                metrics: HashMap::new(),
                health: 0.3,
            });
        }

        let patterns = loop_.detect_patterns();
        assert!(patterns.iter().any(|p| p.contains("System-wide low health")));
    }

    #[test]
    fn test_pattern_detection_component_failure() {
        let mut loop_ = MetacognitiveLoop::new();

        // Same component failing repeatedly
        for _ in 0..5 {
            loop_.monitor(ComponentObservation {
                component: SystemComponent::ResourcePredictor,
                round: 1,
                metrics: HashMap::new(),
                health: 0.2,
            });
        }

        let patterns = loop_.detect_patterns();
        assert!(patterns.iter().any(|p| p.contains("ResourcePredictor consistently failing")));
    }
}
