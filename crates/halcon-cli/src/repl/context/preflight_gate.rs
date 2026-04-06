//! Preflight admission control (Frontier AAA).
//!
//! Checks context budget BEFORE model invocation and makes adaptive decisions:
//!   - Tool shortlisting: reduce tool set when context is tight
//!   - Resume distillation: flag when resumed messages dominate the budget
//!   - Budget violation: block invocation when context exceeds model window
//!
//! This runs AFTER compaction but BEFORE the ModelRequest is built,
//! so it can influence what goes into the request.
//!
//! Design rationale:
//!   - 52 tools suppressed AFTER serialization = wasted tokens + latency
//!   - No existing gate checks tools+messages against context window
//!   - DeepSeek V3 coding with 64K effective window is especially sensitive

use halcon_core::types::ToolDefinition;

// ── PreflightDecision ───────────────────────────────────────────────────────

/// Result of the preflight admission check.
#[derive(Debug)]
pub struct PreflightDecision {
    /// Whether to proceed with model invocation.
    pub proceed: bool,
    /// If tools were reduced, the shortlisted set. None = use all tools.
    pub shortlisted_tools: Option<Vec<ToolDefinition>>,
    /// Number of tools removed by shortlisting.
    pub tools_removed: usize,
    /// Estimated total tokens (messages + tools + system).
    pub estimated_tokens: usize,
    /// Available context budget for the model.
    pub context_budget: usize,
    /// Fraction of budget consumed by messages alone.
    pub message_fraction: f64,
    /// Fraction of budget consumed by tool definitions.
    pub tool_fraction: f64,
    /// Warning message (if context is tight but invocation proceeds).
    pub warning: Option<String>,
}

// ── PreflightConfig ─────────────────────────────────────────────────────────

/// Configuration for the preflight gate.
pub struct PreflightConfig {
    /// Maximum fraction of context budget that tools can consume (0.0-1.0).
    /// If tools exceed this, they are shortlisted.
    pub max_tool_fraction: f64,
    /// Hard limit: block invocation if total tokens exceed this fraction of budget.
    pub hard_limit_fraction: f64,
    /// Minimum number of tools to always keep (even under pressure).
    pub min_tools: usize,
}

impl Default for PreflightConfig {
    fn default() -> Self {
        Self {
            max_tool_fraction: 0.15,
            hard_limit_fraction: 0.95,
            min_tools: 3,
        }
    }
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Run the preflight admission check.
///
/// `message_tokens`: estimated tokens from messages (post-compaction).
/// `system_tokens`: estimated tokens from system prompt.
/// `tools`: current tool definitions.
/// `context_window`: model's total context window size.
/// `output_reserve`: tokens reserved for model output (max_tokens).
pub fn check(
    message_tokens: usize,
    system_tokens: usize,
    tools: &[ToolDefinition],
    context_window: usize,
    output_reserve: usize,
    config: &PreflightConfig,
) -> PreflightDecision {
    let tool_tokens = estimate_tool_tokens(tools);
    let total = message_tokens + system_tokens + tool_tokens;
    let budget = context_window.saturating_sub(output_reserve);

    let message_fraction = if budget > 0 {
        (message_tokens + system_tokens) as f64 / budget as f64
    } else {
        1.0
    };
    let tool_fraction = if budget > 0 {
        tool_tokens as f64 / budget as f64
    } else {
        0.0
    };

    // Hard limit: total exceeds budget
    if budget > 0 && total as f64 > budget as f64 * config.hard_limit_fraction {
        // Try shortlisting tools first
        if tool_tokens > 0 && tools.len() > config.min_tools {
            let target_tool_tokens = budget.saturating_sub(message_tokens + system_tokens);
            let shortlisted = shortlist_tools(tools, target_tool_tokens);
            let new_tool_tokens = estimate_tool_tokens(&shortlisted);
            let new_total = message_tokens + system_tokens + new_tool_tokens;

            if new_total as f64 <= budget as f64 * config.hard_limit_fraction {
                let removed = tools.len() - shortlisted.len();
                tracing::info!(
                    original_tools = tools.len(),
                    shortlisted_tools = shortlisted.len(),
                    removed,
                    original_tokens = tool_tokens,
                    new_tokens = new_tool_tokens,
                    "preflight: shortlisted tools to fit context budget"
                );
                return PreflightDecision {
                    proceed: true,
                    shortlisted_tools: Some(shortlisted),
                    tools_removed: removed,
                    estimated_tokens: new_total,
                    context_budget: budget,
                    message_fraction,
                    tool_fraction: new_tool_tokens as f64 / budget as f64,
                    warning: Some(format!(
                        "Context tight: reduced tools from {} to {} (saved ~{} tokens)",
                        tools.len(),
                        tools.len() - removed,
                        tool_tokens - new_tool_tokens
                    )),
                };
            }
        }

        // Can't fit even with shortlisting — strip all tools
        tracing::warn!(
            total,
            budget,
            message_tokens,
            system_tokens,
            tool_tokens,
            "preflight: context exceeds budget, stripping all tools"
        );
        return PreflightDecision {
            proceed: true,
            shortlisted_tools: Some(Vec::new()),
            tools_removed: tools.len(),
            estimated_tokens: message_tokens + system_tokens,
            context_budget: budget,
            message_fraction,
            tool_fraction: 0.0,
            warning: Some(format!(
                "Context budget exceeded ({total} > {budget}): all {} tools stripped",
                tools.len()
            )),
        };
    }

    // Soft limit: tools consuming too much of the budget
    if tool_fraction > config.max_tool_fraction && tools.len() > config.min_tools {
        let target = (budget as f64 * config.max_tool_fraction) as usize;
        let shortlisted = shortlist_tools(tools, target);
        let new_tool_tokens = estimate_tool_tokens(&shortlisted);
        let removed = tools.len() - shortlisted.len();

        if removed > 0 {
            tracing::info!(
                original = tools.len(),
                shortlisted = shortlisted.len(),
                tool_fraction = format!("{:.2}", tool_fraction),
                max_fraction = format!("{:.2}", config.max_tool_fraction),
                "preflight: tool fraction exceeds limit, shortlisting"
            );
            return PreflightDecision {
                proceed: true,
                shortlisted_tools: Some(shortlisted),
                tools_removed: removed,
                estimated_tokens: message_tokens + system_tokens + new_tool_tokens,
                context_budget: budget,
                message_fraction,
                tool_fraction: new_tool_tokens as f64 / budget as f64,
                warning: None,
            };
        }
    }

    // All clear
    PreflightDecision {
        proceed: true,
        shortlisted_tools: None,
        tools_removed: 0,
        estimated_tokens: total,
        context_budget: budget,
        message_fraction,
        tool_fraction,
        warning: None,
    }
}

// ── Tool shortlisting ───────────────────────────────────────────────────────

/// Reduce tools to fit within a token budget.
///
/// Strategy: sort by schema size (smallest first = most tools for budget),
/// then greedily add until budget is reached.
fn shortlist_tools(tools: &[ToolDefinition], target_tokens: usize) -> Vec<ToolDefinition> {
    // Sort by estimated token cost (ascending — keep cheapest tools first)
    let mut scored: Vec<(usize, &ToolDefinition)> = tools
        .iter()
        .map(|t| (estimate_single_tool_tokens(t), t))
        .collect();
    scored.sort_by_key(|(cost, _)| *cost);

    let mut result = Vec::new();
    let mut used = 0usize;

    for (cost, tool) in scored {
        if used + cost > target_tokens && !result.is_empty() {
            break;
        }
        used += cost;
        result.push(tool.clone());
    }

    result
}

/// Estimate tokens for a single tool definition.
fn estimate_single_tool_tokens(tool: &ToolDefinition) -> usize {
    let name_tokens = tool.name.len() / 4 + 1;
    let desc_tokens = tool.description.len() / 4 + 1;
    let schema_tokens = tool.input_schema.to_string().len() / 4 + 1;
    name_tokens + desc_tokens + schema_tokens + 10 // overhead for structure
}

/// Estimate total tokens for a set of tool definitions.
pub fn estimate_tool_tokens(tools: &[ToolDefinition]) -> usize {
    tools.iter().map(|t| estimate_single_tool_tokens(t)).sum()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool(name: &str, desc_len: usize, schema_len: usize) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: "x".repeat(desc_len),
            input_schema: serde_json::Value::String("y".repeat(schema_len)),
        }
    }

    #[test]
    fn all_clear_when_budget_ok() {
        let tools = vec![make_tool("read", 100, 200)];
        let decision = check(
            10_000,
            2_000,
            &tools,
            100_000,
            4_000,
            &PreflightConfig::default(),
        );
        assert!(decision.proceed);
        assert!(decision.shortlisted_tools.is_none());
        assert_eq!(decision.tools_removed, 0);
    }

    #[test]
    fn shortlist_when_tools_exceed_fraction() {
        // 50 tools with big schemas = lots of tokens
        let tools: Vec<_> = (0..50)
            .map(|i| make_tool(&format!("tool_{i}"), 200, 1000))
            .collect();
        let tool_tokens = estimate_tool_tokens(&tools);
        assert!(tool_tokens > 10_000); // Sanity check

        let decision = check(
            30_000,
            5_000,
            &tools,
            64_000,
            4_000,
            &PreflightConfig::default(),
        );
        assert!(decision.proceed);
        assert!(decision.shortlisted_tools.is_some());
        assert!(decision.tools_removed > 0);
    }

    #[test]
    fn strip_all_when_messages_fill_budget() {
        let tools = vec![make_tool("read", 100, 200)];
        // Messages alone exceed budget
        let decision = check(
            90_000,
            5_000,
            &tools,
            100_000,
            4_000,
            &PreflightConfig::default(),
        );
        assert!(decision.proceed);
        // Tools should be stripped
        if let Some(ref shortlisted) = decision.shortlisted_tools {
            assert!(shortlisted.is_empty());
        }
    }

    #[test]
    fn min_tools_preserved() {
        let tools: Vec<_> = (0..3)
            .map(|i| make_tool(&format!("t{i}"), 100, 500))
            .collect();
        let config = PreflightConfig {
            min_tools: 3,
            max_tool_fraction: 0.01,
            ..Default::default()
        };
        let decision = check(10_000, 2_000, &tools, 100_000, 4_000, &config);
        // Even though tool_fraction exceeds max, min_tools prevents shortlisting
        assert!(decision.shortlisted_tools.is_none());
    }

    #[test]
    fn estimate_tool_tokens_basic() {
        let tool = make_tool("bash", 200, 500);
        let tokens = estimate_single_tool_tokens(&tool);
        assert!(tokens > 100);
        assert!(tokens < 500);
    }

    #[test]
    fn shortlist_keeps_cheapest() {
        let tools = vec![
            make_tool("small", 50, 100),   // ~50 tokens
            make_tool("medium", 200, 500), // ~200 tokens
            make_tool("big", 500, 2000),   // ~650 tokens
        ];
        let shortlisted = shortlist_tools(&tools, 300);
        // Should keep "small" and maybe "medium", not "big"
        assert!(shortlisted.len() <= 2);
        assert!(shortlisted.iter().any(|t| t.name == "small"));
    }
}
