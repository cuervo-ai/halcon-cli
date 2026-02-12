//! Token budget tracker with per-tier allocation.
//!
//! The `TokenAccountant` maintains a budget split across 5 memory tiers (L0-L4),
//! tracking token usage per tier and supporting dynamic rebalancing when a tier
//! overflows.

use crate::assembler::estimate_tokens;
use cuervo_core::types::{ChatMessage, ContentBlock, MessageContent};

/// Tier identifier for the 5-level memory hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Tier {
    /// L0: Hot buffer — most recent messages.
    L0Hot = 0,
    /// L1: Sliding window — compacted summaries.
    L1Warm = 1,
    /// L2: Compressed store — zstd-compressed segments.
    L2Compressed = 2,
    /// L3: Semantic index — BM25 + embedding retrieval.
    L3Semantic = 3,
    /// L4: Cold archive — disk-backed persistence.
    L4Cold = 4,
}

/// Result of a budget allocation attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetResult {
    /// Tokens were successfully allocated.
    Allocated,
    /// Insufficient budget in the requested tier.
    InsufficientBudget {
        available: u32,
        requested: u32,
    },
}

/// Default budget fractions: L0=40%, L1=25%, L2=15%, L3=15%, L4=5%.
const DEFAULT_FRACTIONS: [f32; 5] = [0.40, 0.25, 0.15, 0.15, 0.05];

/// Safety margin: 5% of total budget reserved to prevent overflow.
const SAFETY_FRACTION: f32 = 0.05;

/// Number of tiers.
const TIER_COUNT: usize = 5;

/// Bit-packed token budget tracker with per-tier allocation.
pub struct TokenAccountant {
    total_budget: u32,
    tier_budgets: [u32; TIER_COUNT],
    tier_used: [u32; TIER_COUNT],
    system_prompt_reserved: u32,
    safety_margin: u32,
}

impl TokenAccountant {
    /// Create a new accountant with the given total token budget.
    pub fn new(total_budget: u32) -> Self {
        let safety = (total_budget as f32 * SAFETY_FRACTION) as u32;
        let usable = total_budget.saturating_sub(safety);
        let tier_budgets = DEFAULT_FRACTIONS.map(|f| (usable as f32 * f) as u32);
        Self {
            total_budget,
            tier_budgets,
            tier_used: [0; TIER_COUNT],
            system_prompt_reserved: 0,
            safety_margin: safety,
        }
    }

    /// Create an accountant with custom tier fractions.
    pub fn with_fractions(total_budget: u32, fractions: [f32; TIER_COUNT]) -> Self {
        let safety = (total_budget as f32 * SAFETY_FRACTION) as u32;
        let usable = total_budget.saturating_sub(safety);
        let tier_budgets = fractions.map(|f| (usable as f32 * f) as u32);
        Self {
            total_budget,
            tier_budgets,
            tier_used: [0; TIER_COUNT],
            system_prompt_reserved: 0,
            safety_margin: safety,
        }
    }

    /// Reserve tokens for the system prompt (deducted from L0).
    pub fn reserve_system_prompt(&mut self, tokens: u32) {
        self.system_prompt_reserved = tokens;
        self.tier_budgets[Tier::L0Hot as usize] =
            self.tier_budgets[Tier::L0Hot as usize].saturating_sub(tokens);
    }

    /// Update system prompt reservation to a new value.
    ///
    /// Restores L0 budget from the old reservation, then deducts the new amount.
    /// Used when instruction files change mid-session.
    pub fn update_system_prompt(&mut self, new_tokens: u32) {
        // Restore L0 budget from old reservation.
        self.tier_budgets[Tier::L0Hot as usize] += self.system_prompt_reserved;
        // Apply new reservation.
        self.system_prompt_reserved = new_tokens;
        self.tier_budgets[Tier::L0Hot as usize] =
            self.tier_budgets[Tier::L0Hot as usize].saturating_sub(new_tokens);
    }

    /// Attempt to allocate tokens in a tier.
    pub fn allocate(&mut self, tier: Tier, tokens: u32) -> BudgetResult {
        let idx = tier as usize;
        if self.tier_used[idx] + tokens <= self.tier_budgets[idx] {
            self.tier_used[idx] += tokens;
            BudgetResult::Allocated
        } else {
            BudgetResult::InsufficientBudget {
                available: self.tier_budgets[idx].saturating_sub(self.tier_used[idx]),
                requested: tokens,
            }
        }
    }

    /// Release tokens from a tier.
    pub fn release(&mut self, tier: Tier, tokens: u32) {
        let idx = tier as usize;
        self.tier_used[idx] = self.tier_used[idx].saturating_sub(tokens);
    }

    /// Available tokens in a tier.
    pub fn available(&self, tier: Tier) -> u32 {
        let idx = tier as usize;
        self.tier_budgets[idx].saturating_sub(self.tier_used[idx])
    }

    /// Budget for a tier.
    pub fn tier_budget(&self, tier: Tier) -> u32 {
        self.tier_budgets[tier as usize]
    }

    /// Used tokens in a tier.
    pub fn tier_used(&self, tier: Tier) -> u32 {
        self.tier_used[tier as usize]
    }

    /// Total tokens used across all tiers.
    pub fn total_used(&self) -> u32 {
        self.tier_used.iter().sum()
    }

    /// Total budget.
    pub fn total_budget(&self) -> u32 {
        self.total_budget
    }

    /// Safety margin.
    pub fn safety_margin(&self) -> u32 {
        self.safety_margin
    }

    /// System prompt reservation.
    pub fn system_prompt_reserved(&self) -> u32 {
        self.system_prompt_reserved
    }

    /// Rebalance: steal budget from underused tiers for overflowing ones.
    pub fn rebalance(&mut self) {
        let min_per_tier = self.total_budget / 20; // 5% floor per tier
        for i in 0..TIER_COUNT {
            if self.tier_used[i] > self.tier_budgets[i] {
                let deficit = self.tier_used[i] - self.tier_budgets[i];
                // Find donor: tier with most available
                let donor = (0..TIER_COUNT)
                    .filter(|&j| j != i)
                    .max_by_key(|&j| self.tier_budgets[j].saturating_sub(self.tier_used[j]));
                if let Some(d) = donor {
                    let donor_available =
                        self.tier_budgets[d].saturating_sub(self.tier_used[d]);
                    let donor_min_headroom =
                        self.tier_budgets[d].saturating_sub(min_per_tier);
                    let steal = deficit.min(donor_available).min(donor_min_headroom);
                    self.tier_budgets[d] -= steal;
                    self.tier_budgets[i] += steal;
                }
            }
        }
    }
}

/// Estimate tokens for a single message.
pub fn estimate_message_tokens(msg: &ChatMessage) -> u32 {
    match &msg.content {
        MessageContent::Text(t) => estimate_tokens(t) as u32,
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .map(|b| match b {
                ContentBlock::Text { text } => estimate_tokens(text) as u32,
                ContentBlock::ToolUse { input, .. } => {
                    estimate_tokens(&input.to_string()) as u32
                }
                ContentBlock::ToolResult { content, .. } => estimate_tokens(content) as u32,
            })
            .sum(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cuervo_core::types::Role;

    #[test]
    fn new_accountant_distributes_budget() {
        let acc = TokenAccountant::new(200_000);
        assert_eq!(acc.total_budget(), 200_000);
        // 5% safety margin = 10,000
        assert_eq!(acc.safety_margin(), 10_000);
        // Usable = 190,000
        // L0 = 40% of 190k = 76,000
        assert_eq!(acc.tier_budget(Tier::L0Hot), 76_000);
        // L1 = 25% of 190k = 47,500
        assert_eq!(acc.tier_budget(Tier::L1Warm), 47_500);
        // L2 = 15% of 190k = 28,500
        assert_eq!(acc.tier_budget(Tier::L2Compressed), 28_500);
    }

    #[test]
    fn allocate_within_budget() {
        let mut acc = TokenAccountant::new(100_000);
        assert_eq!(acc.allocate(Tier::L0Hot, 1000), BudgetResult::Allocated);
        assert_eq!(acc.tier_used(Tier::L0Hot), 1000);
        assert_eq!(acc.total_used(), 1000);
    }

    #[test]
    fn allocate_exceeding_budget() {
        let mut acc = TokenAccountant::new(1000);
        // L0 budget = 40% of 950 = 380
        let result = acc.allocate(Tier::L0Hot, 500);
        assert!(matches!(result, BudgetResult::InsufficientBudget { .. }));
    }

    #[test]
    fn release_tokens() {
        let mut acc = TokenAccountant::new(100_000);
        acc.allocate(Tier::L0Hot, 5000);
        assert_eq!(acc.tier_used(Tier::L0Hot), 5000);
        acc.release(Tier::L0Hot, 3000);
        assert_eq!(acc.tier_used(Tier::L0Hot), 2000);
    }

    #[test]
    fn release_saturating() {
        let mut acc = TokenAccountant::new(100_000);
        acc.allocate(Tier::L0Hot, 100);
        acc.release(Tier::L0Hot, 500); // release more than used
        assert_eq!(acc.tier_used(Tier::L0Hot), 0);
    }

    #[test]
    fn reserve_system_prompt() {
        let mut acc = TokenAccountant::new(100_000);
        let l0_before = acc.tier_budget(Tier::L0Hot);
        acc.reserve_system_prompt(5000);
        assert_eq!(acc.system_prompt_reserved(), 5000);
        assert_eq!(acc.tier_budget(Tier::L0Hot), l0_before - 5000);
    }

    #[test]
    fn available_decreases_after_allocation() {
        let mut acc = TokenAccountant::new(100_000);
        let initial = acc.available(Tier::L1Warm);
        acc.allocate(Tier::L1Warm, 1000);
        assert_eq!(acc.available(Tier::L1Warm), initial - 1000);
    }

    #[test]
    fn rebalance_steals_from_underused() {
        let mut acc = TokenAccountant::new(100_000);
        // Force L0 over budget
        let l0_budget = acc.tier_budget(Tier::L0Hot);
        acc.tier_used[Tier::L0Hot as usize] = l0_budget + 5000;
        // L3 is empty (donor)
        let l3_before = acc.tier_budget(Tier::L3Semantic);
        acc.rebalance();
        // L0 budget should increase, L3 (or other donor) should decrease
        assert!(acc.tier_budget(Tier::L0Hot) > l0_budget);
        // Some donor lost budget
        let total_donated: u32 = acc.tier_budget(Tier::L0Hot) - l0_budget;
        assert!(total_donated > 0);
        assert!(total_donated <= 5000);
        let _ = l3_before; // suppress unused warning
    }

    #[test]
    fn custom_fractions() {
        let acc = TokenAccountant::with_fractions(100_000, [0.50, 0.20, 0.10, 0.10, 0.10]);
        // Usable = 95,000. L0 = 50% = 47,500
        assert_eq!(acc.tier_budget(Tier::L0Hot), 47_500);
        assert_eq!(acc.tier_budget(Tier::L1Warm), 19_000);
    }

    #[test]
    fn estimate_text_message_tokens() {
        let msg = ChatMessage {
            role: Role::User,
            content: MessageContent::Text("hello world".to_string()),
        };
        // 11 chars / 4 = 3 tokens (ceil)
        assert_eq!(estimate_message_tokens(&msg), 3);
    }

    #[test]
    fn estimate_blocks_message_tokens() {
        let msg = ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![
                ContentBlock::Text {
                    text: "hello".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "id1".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"cmd": "ls"}),
                },
            ]),
        };
        let tokens = estimate_message_tokens(&msg);
        assert!(tokens > 0);
    }

    #[test]
    fn total_used_sums_all_tiers() {
        let mut acc = TokenAccountant::new(200_000);
        acc.allocate(Tier::L0Hot, 100);
        acc.allocate(Tier::L1Warm, 200);
        acc.allocate(Tier::L2Compressed, 300);
        assert_eq!(acc.total_used(), 600);
    }

    #[test]
    fn zero_budget_accountant() {
        let acc = TokenAccountant::new(0);
        assert_eq!(acc.total_budget(), 0);
        assert_eq!(acc.available(Tier::L0Hot), 0);
    }

    #[test]
    fn update_system_prompt_adjusts_l0_budget() {
        let mut acc = TokenAccountant::new(100_000);
        let l0_initial = acc.tier_budget(Tier::L0Hot);
        acc.reserve_system_prompt(5000);
        assert_eq!(acc.system_prompt_reserved(), 5000);
        assert_eq!(acc.tier_budget(Tier::L0Hot), l0_initial - 5000);

        // Update to larger reservation.
        acc.update_system_prompt(8000);
        assert_eq!(acc.system_prompt_reserved(), 8000);
        assert_eq!(acc.tier_budget(Tier::L0Hot), l0_initial - 8000);
    }

    #[test]
    fn update_system_prompt_shrink_restores_budget() {
        let mut acc = TokenAccountant::new(100_000);
        let l0_initial = acc.tier_budget(Tier::L0Hot);
        acc.reserve_system_prompt(10000);
        assert_eq!(acc.tier_budget(Tier::L0Hot), l0_initial - 10000);

        // Shrink reservation — L0 should recover.
        acc.update_system_prompt(2000);
        assert_eq!(acc.system_prompt_reserved(), 2000);
        assert_eq!(acc.tier_budget(Tier::L0Hot), l0_initial - 2000);
    }
}
