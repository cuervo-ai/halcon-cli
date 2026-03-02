//! Retry Mutation Strategy — prevents identical retries from wasting tokens.
//!
//! When the reward pipeline triggers a retry, this module mutates at least one
//! parameter to ensure the next attempt differs meaningfully from the previous one.
//!
//! Mutation axes (applied in priority order):
//!
//! 1. **Tool exposure**: Remove tools that failed > `policy.tool_failure_threshold`× in previous attempt
//! 2. **Temperature**: Increase by `policy.temperature_step` (max `policy.max_temperature`)
//! 3. **Plan depth**: Reduce by 1 step (forces replanning)
//! 4. **Model fallback**: Switch to a different model tier
//!
//! A retry without mutation is blocked — the agent must either mutate or accept
//! the current result.
//!
//! All thresholds are read from `PolicyConfig` — no local constants.

use halcon_core::types::PolicyConfig;

/// Record of what changed between retry attempts.
#[derive(Debug, Clone)]
pub(crate) struct MutationRecord {
    pub mutations: Vec<MutationAxis>,
    pub retry_number: u32,
}

/// Individual mutation axis applied.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum MutationAxis {
    /// Removed tools that repeatedly failed.
    ToolExposureReduced { removed: Vec<String> },
    /// Increased temperature to diversify LLM output.
    TemperatureIncreased { from: f32, to: f32 },
    /// Reduced plan depth to force replanning.
    PlanDepthReduced { from: u32, to: u32 },
    /// Switched to a different model.
    ModelFallback { from: String, to: String },
}

/// Mutable parameters that can be adjusted between retries.
#[derive(Debug, Clone)]
pub(crate) struct RetryParams {
    pub temperature: f32,
    pub plan_depth: u32,
    pub model_name: String,
    pub available_tools: Vec<String>,
}

/// Tool failure record from the previous attempt.
#[derive(Debug, Clone)]
pub(crate) struct ToolFailureRecord {
    pub tool_name: String,
    pub failure_count: u32,
}

/// Compute mutations for a retry attempt.
///
/// Returns `None` if no mutation is possible (caller should accept current result).
/// All thresholds (temperature step, max temperature, failure threshold) read from `policy`.
pub(crate) fn compute_mutation(
    params: &RetryParams,
    retry_number: u32,
    tool_failures: &[ToolFailureRecord],
    fallback_models: &[String],
    policy: &PolicyConfig,
) -> Option<MutationRecord> {
    let mut mutations = Vec::new();

    // Axis 1: Remove repeatedly-failing tools
    let tools_to_remove: Vec<String> = tool_failures.iter()
        .filter(|tf| tf.failure_count >= policy.tool_failure_threshold)
        .map(|tf| tf.tool_name.clone())
        .collect();

    if !tools_to_remove.is_empty() {
        mutations.push(MutationAxis::ToolExposureReduced {
            removed: tools_to_remove,
        });
    }

    // Axis 2: Increase temperature
    if params.temperature < policy.max_temperature {
        let new_temp = (params.temperature + policy.temperature_step).min(policy.max_temperature);
        mutations.push(MutationAxis::TemperatureIncreased {
            from: params.temperature,
            to: new_temp,
        });
    }

    // Axis 3: Reduce plan depth (only if > 1)
    if params.plan_depth > 1 {
        mutations.push(MutationAxis::PlanDepthReduced {
            from: params.plan_depth,
            to: params.plan_depth - 1,
        });
    }

    // Axis 4: Model fallback (cycle through available fallbacks)
    if !fallback_models.is_empty() {
        let idx = (retry_number as usize) % fallback_models.len();
        let candidate = &fallback_models[idx];
        if candidate != &params.model_name {
            mutations.push(MutationAxis::ModelFallback {
                from: params.model_name.clone(),
                to: candidate.clone(),
            });
        }
    }

    if mutations.is_empty() {
        None
    } else {
        Some(MutationRecord {
            mutations,
            retry_number,
        })
    }
}

/// Apply a mutation record to retry params, returning the updated params.
pub(crate) fn apply_mutation(params: &RetryParams, record: &MutationRecord) -> RetryParams {
    let mut result = params.clone();

    for mutation in &record.mutations {
        match mutation {
            MutationAxis::ToolExposureReduced { removed } => {
                result.available_tools.retain(|t| !removed.contains(t));
            }
            MutationAxis::TemperatureIncreased { to, .. } => {
                result.temperature = *to;
            }
            MutationAxis::PlanDepthReduced { to, .. } => {
                result.plan_depth = *to;
            }
            MutationAxis::ModelFallback { to, .. } => {
                result.model_name = to.clone();
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dp() -> PolicyConfig { PolicyConfig::default() }

    fn base_params() -> RetryParams {
        RetryParams {
            temperature: 0.7,
            plan_depth: 5,
            model_name: "claude-sonnet".into(),
            available_tools: vec!["bash".into(), "file_read".into(), "ci_logs".into()],
        }
    }

    #[test]
    fn removes_failing_tools() {
        let failures = vec![
            ToolFailureRecord { tool_name: "ci_logs".into(), failure_count: 3 },
        ];
        let record = compute_mutation(&base_params(), 1, &failures, &[], &dp()).unwrap();
        assert!(record.mutations.iter().any(|m| matches!(m,
            MutationAxis::ToolExposureReduced { removed } if removed.contains(&"ci_logs".to_string())
        )));
    }

    #[test]
    fn increases_temperature() {
        let record = compute_mutation(&base_params(), 1, &[], &[], &dp()).unwrap();
        assert!(record.mutations.iter().any(|m| matches!(m,
            MutationAxis::TemperatureIncreased { from, to } if (*to - *from - 0.1).abs() < 0.001
        )));
    }

    #[test]
    fn reduces_plan_depth() {
        let record = compute_mutation(&base_params(), 1, &[], &[], &dp()).unwrap();
        assert!(record.mutations.iter().any(|m| matches!(m,
            MutationAxis::PlanDepthReduced { from: 5, to: 4 }
        )));
    }

    #[test]
    fn model_fallback_on_retry() {
        let fallbacks = vec!["deepseek-chat".into()];
        let record = compute_mutation(&base_params(), 1, &[], &fallbacks, &dp()).unwrap();
        assert!(record.mutations.iter().any(|m| matches!(m,
            MutationAxis::ModelFallback { to, .. } if to == "deepseek-chat"
        )));
    }

    #[test]
    fn no_mutation_when_maxed_out() {
        let params = RetryParams {
            temperature: 1.0,
            plan_depth: 1,
            model_name: "claude-sonnet".into(),
            available_tools: vec![],
        };
        // No failing tools, max temp, min plan depth, no fallbacks
        assert!(compute_mutation(&params, 1, &[], &[], &dp()).is_none());
    }

    #[test]
    fn apply_mutation_updates_params() {
        let params = base_params();
        let failures = vec![
            ToolFailureRecord { tool_name: "ci_logs".into(), failure_count: 3 },
        ];
        let fallbacks = vec!["deepseek-chat".into()];
        let record = compute_mutation(&params, 1, &failures, &fallbacks, &dp()).unwrap();
        let updated = apply_mutation(&params, &record);

        assert!(!updated.available_tools.contains(&"ci_logs".to_string()));
        assert!(updated.temperature > params.temperature);
        assert!(updated.plan_depth < params.plan_depth);
        assert_eq!(updated.model_name, "deepseek-chat");
    }

    #[test]
    fn skip_fallback_when_same_model() {
        let params = base_params();
        let fallbacks = vec!["claude-sonnet".into()]; // same as current
        let record = compute_mutation(&params, 1, &[], &fallbacks, &dp()).unwrap();
        assert!(!record.mutations.iter().any(|m| matches!(m, MutationAxis::ModelFallback { .. })));
    }

    #[test]
    fn mutation_record_tracks_retry_number() {
        let record = compute_mutation(&base_params(), 3, &[], &[], &dp()).unwrap();
        assert_eq!(record.retry_number, 3);
    }

    #[test]
    fn below_threshold_tools_not_removed() {
        let failures = vec![
            ToolFailureRecord { tool_name: "ci_logs".into(), failure_count: 1 },
        ];
        let record = compute_mutation(&base_params(), 1, &failures, &[], &dp()).unwrap();
        assert!(!record.mutations.iter().any(|m| matches!(m,
            MutationAxis::ToolExposureReduced { removed } if removed.contains(&"ci_logs".to_string())
        )));
    }

    #[test]
    fn temperature_capped_at_max() {
        let mut params = base_params();
        params.temperature = 0.95;
        let record = compute_mutation(&params, 1, &[], &[], &dp()).unwrap();
        assert!(record.mutations.iter().any(|m| matches!(m,
            MutationAxis::TemperatureIncreased { to, .. } if (*to - 1.0).abs() < 0.001
        )));
    }

    #[test]
    fn custom_policy_thresholds_respected() {
        let mut policy = PolicyConfig::default();
        policy.tool_failure_threshold = 5; // higher threshold
        policy.temperature_step = 0.2;     // bigger step
        policy.max_temperature = 0.9;      // lower cap

        let failures = vec![
            ToolFailureRecord { tool_name: "ci_logs".into(), failure_count: 3 },
        ];
        let params = base_params();
        let record = compute_mutation(&params, 1, &failures, &[], &policy).unwrap();

        // ci_logs has 3 failures < 5 threshold → NOT removed
        assert!(!record.mutations.iter().any(|m| matches!(m,
            MutationAxis::ToolExposureReduced { .. }
        )));
        // Temperature step = 0.2, so 0.7 → 0.9 (capped at max 0.9)
        assert!(record.mutations.iter().any(|m| matches!(m,
            MutationAxis::TemperatureIncreased { to, .. } if (*to - 0.9).abs() < 0.001
        )));
    }
}
