//! CompactionBudgetCalculator: centralized budget logic for semantic compaction.
//!
//! Owns the budget equation: S + P + K + R ≤ B
//! Provides trigger policy, cap computation, and post-compaction verification.

use halcon_core::types::CompactionConfig;

use super::compaction::ContextCompactor;

/// Pre-computed budget parameters for a compaction event.
#[derive(Debug, Clone)]
pub struct CompactionBudget {
    /// Pipeline budget: W × U.
    pub pipeline_budget: usize,
    /// Trigger threshold: B - C_reserve.
    pub trigger_threshold: usize,
    /// Maximum summary tokens (S_max).
    pub max_summary_tokens: usize,
    /// Maximum protected context tokens (P_max).
    pub max_protected_tokens: usize,
    /// Nominal keep count (messages).
    pub keep_count: usize,
    /// Extended keep count for degraded level.
    pub extended_keep_count: usize,
    /// Reserve for next turn (R = max_output_tokens).
    pub reserve: usize,
}

/// Result of post-compaction budget verification.
#[derive(Debug, PartialEq)]
pub enum PostCompactionCheck {
    /// Budget satisfied.
    Ok,
    /// Summary must be truncated to fit.
    SummaryTruncationNeeded { target_tokens: usize },
    /// Keep window must be reduced (summary already at 0).
    KeepReductionNeeded { target_keep: usize },
}

/// Centralized budget computation. All functions are pure.
pub struct CompactionBudgetCalculator;

impl CompactionBudgetCalculator {
    /// Compute the full budget from pipeline parameters.
    pub fn compute(
        pipeline_budget: usize,
        max_output_tokens: u32,
        config: &CompactionConfig,
        message_count: usize,
        avg_tokens_per_recent_message: usize,
    ) -> CompactionBudget {
        let uf = config.utilization_factor.clamp(0.5, 1.0);
        if (uf - config.utilization_factor).abs() > 0.001 {
            tracing::warn!(
                original = config.utilization_factor,
                clamped = uf,
                "utilization_factor clamped to [0.5, 1.0]"
            );
        }

        let b = pipeline_budget;
        let r = max_output_tokens as usize;

        // S_max = clamp(B * proportion, floor, cap)
        let s_max = {
            let raw = (b as f64 * config.summary_proportion as f64) as usize;
            raw.max(config.summary_floor as usize)
                .min(config.summary_cap as usize)
        };

        let p_max = config.protected_context_cap as usize;
        let c_reserve = s_max + p_max + r;

        // Trigger threshold: B - C_reserve
        let trigger = if c_reserve < b {
            b - c_reserve
        } else {
            // Guard: C_reserve exceeds budget — use fallback
            tracing::error!(
                c_reserve,
                pipeline_budget = b,
                "C_reserve exceeds pipeline_budget, using 60% fallback"
            );
            (b as f64 * 0.60) as usize
        };

        // Nominal keep: reuse existing adaptive formula
        let keep_count = ContextCompactor::adaptive_keep_recent(pipeline_budget as u32);

        // Extended keep for degraded level:
        // keep_count + (S_max / avg_msg_tokens), capped at 2/3 of messages
        let extended = if avg_tokens_per_recent_message > 0 {
            let extra = s_max / avg_tokens_per_recent_message;
            keep_count + extra
        } else {
            keep_count
        };
        let extended_keep_count = extended.min(message_count * 2 / 3).max(keep_count);

        CompactionBudget {
            pipeline_budget: b,
            trigger_threshold: trigger,
            max_summary_tokens: s_max,
            max_protected_tokens: p_max,
            keep_count,
            extended_keep_count,
            reserve: r,
        }
    }

    /// Should compaction trigger?
    pub fn should_compact(estimated_tokens: usize, budget: &CompactionBudget) -> bool {
        estimated_tokens >= budget.trigger_threshold
    }

    /// Verify the post-compaction state fits within budget.
    pub fn verify_post_compaction(
        tokens_after: usize,
        budget: &CompactionBudget,
    ) -> PostCompactionCheck {
        if tokens_after + budget.reserve <= budget.pipeline_budget {
            return PostCompactionCheck::Ok;
        }

        // How much do we need to shed?
        let overage = (tokens_after + budget.reserve).saturating_sub(budget.pipeline_budget);

        // Try truncating summary first
        if overage <= budget.max_summary_tokens {
            let target = budget.max_summary_tokens.saturating_sub(overage);
            PostCompactionCheck::SummaryTruncationNeeded {
                target_tokens: target,
            }
        } else {
            // Even removing the summary entirely isn't enough — reduce keep
            let needed_reduction = overage.saturating_sub(budget.max_summary_tokens);
            let current_keep = budget.keep_count;
            // Rough: 1 message ≈ some avg tokens — can't know exactly here
            // Return a target keep that's smaller
            let target_keep = current_keep.saturating_sub(
                (needed_reduction / 500).max(1), // rough estimate: 500 tokens/msg
            );
            PostCompactionCheck::KeepReductionNeeded {
                target_keep: target_keep.max(2), // never go below 2 messages
            }
        }
    }

    /// Compute utility ratio: net benefit of compaction.
    ///
    /// utility = (T_freed - T_added) / T_freed
    /// where T_freed = tokens_before - keep_tokens, T_added = summary + protected
    pub fn utility_ratio(
        tokens_before: usize,
        keep_tokens: usize,
        summary_tokens: usize,
        protected_tokens: usize,
    ) -> f64 {
        let freed = tokens_before.saturating_sub(keep_tokens);
        if freed == 0 {
            return 0.0;
        }
        let added = summary_tokens + protected_tokens;
        (freed as f64 - added as f64) / freed as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> CompactionConfig {
        CompactionConfig::default()
    }

    #[test]
    fn deepseek_64k_budget() {
        let b = (64_000.0 * 0.80) as usize; // 51_200
        let budget = CompactionBudgetCalculator::compute(b, 4096, &default_config(), 100, 1000);

        assert_eq!(budget.pipeline_budget, 51_200);
        // S_max = clamp(51200 * 0.05, 1000, 4000) = clamp(2560, 1000, 4000) = 2560
        assert_eq!(budget.max_summary_tokens, 2560);
        // C_reserve = 2560 + 500 + 4096 = 7156
        // trigger = 51200 - 7156 = 44044
        assert_eq!(budget.trigger_threshold, 44044);
        assert_eq!(budget.reserve, 4096);
    }

    #[test]
    fn claude_200k_budget() {
        let b = (200_000.0 * 0.80) as usize; // 160_000
        let budget = CompactionBudgetCalculator::compute(b, 4096, &default_config(), 100, 1000);

        assert_eq!(budget.pipeline_budget, 160_000);
        // S_max = clamp(160000 * 0.05, 1000, 4000) = clamp(8000, 1000, 4000) = 4000
        assert_eq!(budget.max_summary_tokens, 4000);
        // C_reserve = 4000 + 500 + 4096 = 8596
        // trigger = 160000 - 8596 = 151404
        assert_eq!(budget.trigger_threshold, 151404);
    }

    #[test]
    fn minimum_32k_budget() {
        let b = (32_000.0 * 0.80) as usize; // 25_600
        let budget = CompactionBudgetCalculator::compute(b, 4096, &default_config(), 50, 500);

        // S_max = clamp(25600 * 0.05, 1000, 4000) = clamp(1280, 1000, 4000) = 1280
        assert_eq!(budget.max_summary_tokens, 1280);
    }

    #[test]
    fn should_compact_above_threshold() {
        let budget = CompactionBudgetCalculator::compute(51200, 4096, &default_config(), 100, 1000);
        assert!(CompactionBudgetCalculator::should_compact(50000, &budget));
    }

    #[test]
    fn should_compact_below_threshold() {
        let budget = CompactionBudgetCalculator::compute(51200, 4096, &default_config(), 100, 1000);
        assert!(!CompactionBudgetCalculator::should_compact(30000, &budget));
    }

    #[test]
    fn utility_ratio_positive() {
        // 100K tokens, keep 10K, add 5K summary + 500 protected
        let u = CompactionBudgetCalculator::utility_ratio(100_000, 10_000, 5000, 500);
        // freed = 90K, added = 5.5K, utility = 84.5K / 90K ≈ 0.939
        assert!(u > 0.9);
    }

    #[test]
    fn utility_ratio_negative() {
        // freed = 1K, added = 5K — negative utility
        let u = CompactionBudgetCalculator::utility_ratio(11_000, 10_000, 4500, 500);
        assert!(u < 0.0);
    }

    #[test]
    fn utility_ratio_zero_freed() {
        let u = CompactionBudgetCalculator::utility_ratio(10_000, 10_000, 1000, 500);
        assert_eq!(u, 0.0);
    }

    #[test]
    fn verify_post_compaction_ok() {
        let budget =
            CompactionBudgetCalculator::compute(160_000, 4096, &default_config(), 100, 1000);
        // tokens_after = 20K, reserve = 4096, total = 24096 < 160K
        assert_eq!(
            CompactionBudgetCalculator::verify_post_compaction(20_000, &budget),
            PostCompactionCheck::Ok
        );
    }

    #[test]
    fn verify_post_compaction_truncation_needed() {
        let budget =
            CompactionBudgetCalculator::compute(51_200, 4096, &default_config(), 100, 1000);
        // tokens_after = 48K, reserve = 4096 → 52096 > 51200, overage = 896
        let check = CompactionBudgetCalculator::verify_post_compaction(48_000, &budget);
        match check {
            PostCompactionCheck::SummaryTruncationNeeded { target_tokens } => {
                assert!(target_tokens < budget.max_summary_tokens);
            }
            other => panic!("Expected SummaryTruncationNeeded, got {:?}", other),
        }
    }

    #[test]
    fn extended_keep_count_basic() {
        let budget = CompactionBudgetCalculator::compute(51200, 4096, &default_config(), 100, 1000);
        // keep_count = adaptive(51200) = 5
        // extended = 5 + (2560 / 1000) = 5 + 2 = 7
        assert_eq!(budget.keep_count, 5);
        assert!(budget.extended_keep_count >= budget.keep_count);
    }

    #[test]
    fn extended_keep_count_capped() {
        // With very small avg (5 tokens/msg), extension would be huge
        let budget = CompactionBudgetCalculator::compute(51200, 4096, &default_config(), 30, 5);
        // extended = 5 + (2560/5) = 5 + 512 = 517, capped at 30*2/3 = 20
        assert!(budget.extended_keep_count <= 20);
        assert!(budget.extended_keep_count >= budget.keep_count);
    }

    #[test]
    fn extended_keep_count_floor() {
        let budget = CompactionBudgetCalculator::compute(51200, 4096, &default_config(), 3, 10000);
        // extended = 5 + 0 = 5, but capped at 3*2/3=2, floored at keep_count=5 → 5
        assert_eq!(budget.extended_keep_count, budget.keep_count);
    }
}
