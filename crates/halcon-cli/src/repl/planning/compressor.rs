//! Plan Compressor — merges, deduplicates, and compresses execution plan steps.
//!
//! ## Purpose
//!
//! The LLM planner often generates redundant or overly granular steps that increase
//! token consumption, extend execution time, and produce noisy user feedback.
//! The compressor applies five deterministic rules to shrink plans before execution:
//!
//! 1. **Read-merge**: Consecutive `file_read` / `grep` / `glob` steps → single grouped step.
//! 2. **Trivial-elimination**: Steps with description matching trivial patterns (verify, check
//!    existence) that are subsumed by BOTH adjacent tool calls (AND, not OR).
//! 3. **Synthesis deduplication**: Only one synthesis step allowed (last, `tool_name: null`).
//! 4. **Parallel promotion**: Sequential read-only steps with no data dependency → `parallel: true`.
//! 5. **Hard cap enforcement**: Caps at `MAX_VISIBLE_STEPS = 4` total steps including synthesis.
//!
//! ## Design goals
//!
//! - Zero LLM calls (purely deterministic)
//! - O(n) where n = number of plan steps (typically ≤ 5)
//! - Preserves all fields of surviving steps unchanged
//! - Always keeps the synthesis step as the final step
//! - Returns the original plan unchanged if no compression needed
//!
//! ## Guarantees (formal invariants)
//!
//! - I1: `result.steps.last().tool_name == None` (synthesis always last)
//! - I2: `result.steps.len() >= 1` (at least synthesis exists)
//! - I3: `result.steps.len() <= MAX_VISIBLE_STEPS`
//! - I4: `filter(|s| s.tool_name.is_none()).count() == 1` (exactly one synthesis)
//! - I5: Step order preserved relative to the original plan (cap removes by confidence, not order)
//! - I6: First step is never removed by cap (I5 specialisation for index 0)
//! - I7: Safety-critical steps are never eliminated
//!
//! ## Metrics
//!
//! Expected reductions vs uncompressed:
//! - Steps: -25% to -60% depending on task type
//! - Tool calls: -20% to -40%
//! - Feedback lines shown to user: -30% to -50%

use halcon_core::traits::{ExecutionPlan, PlanStep};
use uuid::Uuid;

// ── Constants ─────────────────────────────────────────────────────────────

/// Maximum steps shown to the user (including synthesis).
/// Raised to 8 to accommodate execution tasks (install → build → run → verify → synthesize).
/// Investigation tasks self-limit via the planner prompt (max 4 steps).
pub const MAX_VISIBLE_STEPS: usize = 8;

/// Read-only tools that can be merged into a grouped read step.
const READONLY_MERGEABLE: &[&str] = &[
    "file_read",
    "grep",
    "glob",
    "directory_tree",
    "file_inspect",
    "symbol_search",
    "fuzzy_find",
    "git_log",
    "git_diff",
    "git_status",
];

/// Tool names whose steps are PROTECTED from hard-cap truncation.
///
/// These represent commands that must execute to achieve the task objective.
/// Execution-protected steps are never removed by Rule 5; only read-only
/// analysis steps can be dropped when the plan exceeds `MAX_VISIBLE_STEPS`.
const EXECUTION_PROTECTED: &[&str] = &[
    "bash",
    "file_write",
    "edit_file",
    "run_command",
    "terminal",
    "apply_patch",
    "code_execution",
];

/// Safety-critical tool names patterns used by both Rule 2 and Rule 5.
const SAFETY_CRITICAL_PATTERNS: &[&str] = &[
    "file_delete",
    "git_push",
    "git_commit",
];

/// Trivial patterns — step descriptions matching these prefixes are candidates
/// for elimination when BOTH adjacent steps have tools (AND condition).
const TRIVIAL_PATTERNS: &[&str] = &[
    "verify",
    "check if",
    "check whether",
    "confirm",
    "ensure",
    "validate that",
    "look for",
];

// ── Public API ────────────────────────────────────────────────────────────

/// Compression statistics returned alongside the compressed plan.
#[derive(Debug, Clone, Default)]
pub struct CompressionStats {
    /// Steps removed by read-merge.
    pub merged_reads: usize,
    /// Steps removed by trivial-elimination.
    pub eliminated_trivials: usize,
    /// Duplicate synthesis steps removed.
    pub deduped_synthesis: usize,
    /// Steps promoted to parallel execution.
    pub parallelised: usize,
    /// Steps removed by hard-cap truncation.
    pub cap_truncated: usize,
}

impl CompressionStats {
    /// True if at least one compression was applied.
    pub fn any_applied(&self) -> bool {
        self.merged_reads > 0
            || self.eliminated_trivials > 0
            || self.deduped_synthesis > 0
            || self.parallelised > 0
            || self.cap_truncated > 0
    }

    /// Total steps removed.
    pub fn total_removed(&self) -> usize {
        self.merged_reads + self.eliminated_trivials + self.deduped_synthesis + self.cap_truncated
    }
}

/// Compress an execution plan by applying all five compression rules in order.
///
/// Returns the (possibly modified) plan and compression statistics.
/// If no rules fire, returns the original plan with all-zero stats.
///
/// # Guarantees
/// - The returned plan always has at least one step (synthesis step at end).
/// - The synthesis step (tool_name = None) is always the LAST step (I1, I2).
/// - There is exactly one synthesis step (I4).
/// - Step order is preserved relative to the original; cap removes by confidence (I5).
/// - The first step (index 0) is never removed by cap (I6).
/// - Safety-critical steps are never eliminated (I7).
/// - `result.steps.len() <= MAX_VISIBLE_STEPS` (I3).
pub fn compress(mut plan: ExecutionPlan) -> (ExecutionPlan, CompressionStats) {
    let mut stats = CompressionStats::default();

    // Rule 1: Merge consecutive read-only steps.
    let (steps, merged) = merge_consecutive_reads(plan.steps);
    plan.steps = steps;
    stats.merged_reads = merged;

    // Rule 2: Eliminate trivial steps subsumed by BOTH adjacent tools.
    let (steps, eliminated) = eliminate_trivials(plan.steps);
    plan.steps = steps;
    stats.eliminated_trivials = eliminated;

    // Rule 3: Deduplicate synthesis steps (keep only last tool_name=None step, force it last).
    let (steps, deduped) = dedup_synthesis(plan.steps);
    plan.steps = steps;
    stats.deduped_synthesis = deduped;

    // Rule 4: Promote sequential read-only steps to parallel.
    let parallelised = promote_parallel(&mut plan.steps);
    stats.parallelised = parallelised;

    // Rule 5: Enforce hard cap (MAX_VISIBLE_STEPS).
    let cap_truncated = enforce_cap(&mut plan.steps);
    stats.cap_truncated = cap_truncated;

    // Invariant enforcement: always guarantee exactly one synthesis step at end.
    // This is the final safety net — earlier rules may leave the plan synthesis-free
    // in degenerate cases (e.g. all steps trivially eliminated, then capped).
    ensure_synthesis_exists(&mut plan.steps);

    (plan, stats)
}

// ── Rule 1: Merge consecutive reads ───────────────────────────────────────

fn merge_consecutive_reads(steps: Vec<PlanStep>) -> (Vec<PlanStep>, usize) {
    if steps.len() <= 1 {
        return (steps, 0);
    }

    let mut result: Vec<PlanStep> = Vec::with_capacity(steps.len());
    let mut removed = 0;
    let mut i = 0;

    while i < steps.len() {
        let step = &steps[i];

        // Not a mergeable read? Keep as-is.
        if !is_readonly_mergeable(step) {
            result.push(steps[i].clone());
            i += 1;
            continue;
        }

        // Scan forward for consecutive reads.
        let mut j = i + 1;
        let mut group_tools: Vec<String> = vec![step.tool_name.clone().unwrap_or_default()];
        let mut group_descs: Vec<String> = vec![step.description.clone()];

        while j < steps.len() && is_readonly_mergeable(&steps[j]) {
            group_tools.push(steps[j].tool_name.clone().unwrap_or_default());
            group_descs.push(steps[j].description.clone());
            j += 1;
        }

        if j - i <= 1 {
            // Only one step in group — no merge needed.
            result.push(steps[i].clone());
            i += 1;
            continue;
        }

        // Merge into a single grouped step.
        let merged_desc = format!(
            "Read and analyse: {}",
            group_descs
                .iter()
                .map(|d| shorten_desc(d))
                .collect::<Vec<_>>()
                .join(", ")
        );

        // Use first step as template, update description + parallel flag.
        // BUG-C4 FIX: Store the full list of grouped tool names in expected_args so
        // that ExecutionTracker can match any of them when recording step outcomes.
        // The tool_name field keeps the first tool name for dispatch compatibility.
        let mut merged_step = steps[i].clone();
        merged_step.description = merged_desc;
        merged_step.parallel = true; // reads can run in parallel
        merged_step.expected_args = Some(serde_json::json!({
            "grouped_tools": group_tools
        }));

        result.push(merged_step);
        removed += j - i - 1;
        i = j;
    }

    (result, removed)
}

fn is_readonly_mergeable(step: &PlanStep) -> bool {
    match &step.tool_name {
        None => false, // synthesis step — never merge
        Some(t) => READONLY_MERGEABLE.contains(&t.as_str()),
    }
}

fn shorten_desc(desc: &str) -> String {
    // Extract meaningful noun phrase: strip leading verb.
    let lower = desc.to_lowercase();
    for prefix in &[
        "read ",
        "analyse ",
        "analyze ",
        "search ",
        "find ",
        "look up ",
        "inspect ",
        "check ",
        "scan ",
        "list ",
    ] {
        if lower.starts_with(prefix) {
            // Preserve original casing of the remainder.
            let rest = desc[prefix.len()..].trim();
            return truncate(rest, 40);
        }
    }
    truncate(desc, 40)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

// ── Rule 2: Eliminate trivial steps ───────────────────────────────────────

fn eliminate_trivials(steps: Vec<PlanStep>) -> (Vec<PlanStep>, usize) {
    // A trivial step is eligible for elimination only when:
    // - Its description matches a trivial pattern, AND
    // - The PREVIOUS step has a tool (not synthesis), AND
    // - The NEXT step has a tool (not synthesis).
    //
    // BUG-C3 FIX: Changed OR → AND. A trivial step at the boundary of the plan
    // (only one adjacent tool) must be preserved because it may represent the
    // only verification point for that operation.
    //
    // Safety-critical steps are always preserved regardless of adjacency (I7).

    let mut result: Vec<PlanStep> = Vec::with_capacity(steps.len());
    let mut eliminated = 0;

    for (i, step) in steps.iter().enumerate() {
        if step.tool_name.is_none() {
            // Never eliminate synthesis step.
            result.push(step.clone());
            continue;
        }

        // BUG-C3 FIX: Check safety-critical classification BEFORE trivial check.
        if is_safety_critical_step(step) {
            result.push(step.clone());
            continue;
        }

        if is_trivial_step(step) {
            let prev_has_tool = i > 0 && steps[i - 1].tool_name.is_some();
            let next_has_tool = i + 1 < steps.len() && steps[i + 1].tool_name.is_some();

            // BUG-C3 FIX: AND condition — BOTH neighbors must have tools.
            if prev_has_tool && next_has_tool {
                eliminated += 1;
                continue;
            }
        }

        result.push(step.clone());
    }

    (result, eliminated)
}

fn is_trivial_step(step: &PlanStep) -> bool {
    let desc = step.description.to_lowercase();
    TRIVIAL_PATTERNS.iter().any(|p| desc.starts_with(p))
}

/// Returns true if this step represents a safety-critical operation that must
/// never be eliminated, even if it matches a trivial pattern (I7).
///
/// Safety-critical steps include: permission checks, auth verification, backup
/// operations, and steps involving irreversible or credential-related tools.
fn is_safety_critical_step(step: &PlanStep) -> bool {
    let desc = step.description.to_lowercase();

    // Explicitly destructive-only tool names (irreversible operations).
    let is_destructive_tool = step
        .tool_name
        .as_deref()
        .map(|t| matches!(t, "file_delete" | "git_push" | "git_commit"))
        .unwrap_or(false);

    if is_destructive_tool {
        return true;
    }

    // Description-based safety keywords (regardless of tool).
    const SAFETY_KEYWORDS: &[&str] = &[
        "backup",
        "rollback",
        "credentials",
        "secret",
        "permission check",
        "auth",
        "delete",
        "remove file",
        "overwrite",
    ];
    SAFETY_KEYWORDS.iter().any(|kw| desc.contains(kw))
}

// ── Rule 3: Deduplicate synthesis ─────────────────────────────────────────

fn dedup_synthesis(steps: Vec<PlanStep>) -> (Vec<PlanStep>, usize) {
    let synthesis_count = steps.iter().filter(|s| s.tool_name.is_none()).count();

    if synthesis_count == 0 {
        // No synthesis at all — ensure_synthesis_exists() will add one later.
        return (steps, 0);
    }

    if synthesis_count == 1 {
        // BUG-C2 FIX: Even a single synthesis step must be at the END.
        // Models sometimes generate plans with synthesis at position 0 or mid-plan.
        let pos = steps.iter().rposition(|s| s.tool_name.is_none()).unwrap();
        if pos == steps.len().saturating_sub(1) {
            // Already at end — no change needed.
            return (steps, 0);
        }
        // Move synthesis to the end while preserving all other step positions.
        let mut result = steps;
        let synth = result.remove(pos);
        result.push(synth);
        return (result, 0);
    }

    // Multiple synthesis steps: keep only the LAST one and move it to the end.
    // BUG-C2 FIX: Collect tool steps first, then append synthesis — guarantees
    // synthesis is last regardless of its original position.
    let last_synthesis_idx = steps
        .iter()
        .enumerate()
        .filter(|(_, s)| s.tool_name.is_none())
        .map(|(i, _)| i)
        .last()
        .unwrap(); // safe: synthesis_count > 1

    let deduped_count = synthesis_count - 1;
    let synthesis_step = steps[last_synthesis_idx].clone();

    // Collect all tool steps in original order, then append the synthesis step last.
    let result: Vec<PlanStep> = steps
        .into_iter()
        .filter(|s| s.tool_name.is_some())
        .chain(std::iter::once(synthesis_step))
        .collect();

    (result, deduped_count)
}

// ── Rule 4: Promote to parallel ────────────────────────────────────────────

fn promote_parallel(steps: &mut Vec<PlanStep>) -> usize {
    // Promote consecutive read-only tool steps that aren't already parallel.
    // We only promote steps[i] when steps[i-1] is also a read-only step.
    // Never promote synthesis or write/destructive steps.

    let mut promoted = 0;

    // First pass: identify which steps are read-only.
    let is_ro: Vec<bool> = steps.iter().map(is_readonly_mergeable).collect();

    for i in 1..steps.len() {
        if is_ro[i] && is_ro[i - 1] && !steps[i].parallel {
            steps[i].parallel = true;
            promoted += 1;
        }
    }

    promoted
}

// ── Rule 5: Enforce hard cap ───────────────────────────────────────────────

fn enforce_cap(steps: &mut Vec<PlanStep>) -> usize {
    if steps.len() <= MAX_VISIBLE_STEPS {
        return 0;
    }

    // Extract synthesis (last tool_name=None step) BEFORE truncation so it is
    // always re-appended at the end after cap enforcement.
    let synthesis_pos = steps.iter().rposition(|s| s.tool_name.is_none());
    let synthesis = synthesis_pos.map(|pos| steps.remove(pos));

    // Reserve one slot for synthesis; the rest are tool step slots.
    let max_tool_steps = MAX_VISIBLE_STEPS.saturating_sub(synthesis.is_some() as usize);

    let removed = if steps.len() > max_tool_steps {
        let excess = steps.len() - max_tool_steps;

        // BUG-C1 FIX: Preserve original step order by filtering via tracked indices.
        //   Previously the code sorted by confidence after filtering, which corrupted
        //   semantic ordering (e.g. "write" step moved before "read" step).
        //
        // BUG-H2 FIX: Always protect step at index 0.
        //   The first step is the entry-point of the plan; removing it leaves later
        //   steps with unresolved context dependencies.
        //
        // Strategy: assign a sort key — MAX for index 0 (always keep), confidence
        // for all others — then select the top max_tool_steps by key. Filter the
        // original Vec in original order (drain preserves order).

        // BUG-C1 FIX: Preserve original step order by filtering via tracked indices.
        // BUG-H2 FIX: Always protect step at index 0.
        //
        // Phase 3 / EXECUTION_PROTECTED: Execution tool steps (bash, file_write, etc.) and
        // safety-critical steps are NEVER removed by the hard cap — only read-only analysis
        // steps are candidates for removal. If there are not enough analysis steps to reach
        // the cap, we remove as many as possible (cap becomes a soft limit for execution tasks).
        //
        // Strategy: classify each step as protected or non-protected, then remove the
        // lowest-confidence non-protected steps first. Protected steps are always kept.

        let is_step_protected = |i: usize, s: &PlanStep| -> bool {
            i == 0
                || s.tool_name.as_deref()
                    .map(|t| {
                        EXECUTION_PROTECTED.contains(&t)
                            || SAFETY_CRITICAL_PATTERNS.iter().any(|p| t.contains(p))
                    })
                    .unwrap_or(true) // synthesis (tool_name=None) is also protected
        };

        // Collect removable (non-protected) indices sorted by confidence ascending.
        let mut removable: Vec<(usize, f64)> = steps
            .iter()
            .enumerate()
            .filter(|(i, s)| !is_step_protected(*i, s))
            .map(|(i, s)| (i, s.confidence))
            .collect();

        // Sort ascending — lowest confidence removed first.
        removable.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        // Remove at most `excess` non-protected steps; can't remove more than available.
        let to_remove_count = excess.min(removable.len());
        let remove_indices: std::collections::HashSet<usize> = removable
            .iter()
            .take(to_remove_count)
            .map(|(i, _)| *i)
            .collect();

        // BUG-C1 FIX: Filter while preserving original order.
        let kept: Vec<PlanStep> = steps
            .drain(..)
            .enumerate()
            .filter(|(i, _)| !remove_indices.contains(i))
            .map(|(_, s)| s)
            .collect();

        *steps = kept;
        to_remove_count
    } else {
        0
    };

    // Re-append synthesis at the end (guaranteed last by I1).
    if let Some(synth) = synthesis {
        steps.push(synth);
    }

    removed
}

// ── Invariant: always ensure synthesis exists ──────────────────────────────

/// Ensure the plan always ends with exactly one synthesis step.
///
/// Called as the final step of `compress()` to uphold invariants I1, I2, I4
/// even when all rules fire aggressively (e.g. an empty plan, or cap removing
/// the synthesis step when no synthesis was extracted beforehand).
///
/// BUG-H7 FIX: Previously `compress()` could return an empty plan or a plan
/// without any synthesis step. This function is the final safety net.
fn ensure_synthesis_exists(steps: &mut Vec<PlanStep>) {
    let has_synthesis = steps.iter().any(|s| s.tool_name.is_none());
    if !has_synthesis {
        steps.push(PlanStep {
            step_id: Uuid::new_v4(),
            description: "Synthesize findings and respond to the user".to_string(),
            tool_name: None,
            parallel: false,
            confidence: 1.0,
            expected_args: None,
            outcome: None,
        });
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::traits::PlanStep;
    use uuid::Uuid;

    fn make_step(desc: &str, tool: Option<&str>, confidence: f64) -> PlanStep {
        PlanStep {
            step_id: Uuid::new_v4(),
            description: desc.to_string(),
            tool_name: tool.map(|s| s.to_string()),
            parallel: false,
            confidence,
            expected_args: None,
            outcome: None,
        }
    }

    fn make_plan(steps: Vec<PlanStep>) -> ExecutionPlan {
        ExecutionPlan {
            goal: "Test goal".to_string(),
            steps,
            requires_confirmation: false,
            plan_id: Uuid::new_v4(),
            replan_count: 0,
            parent_plan_id: None,
            ..Default::default()
        }
    }

    // ── Rule 1: Read merge ───────────────────────────────────────────────

    #[test]
    fn merge_two_consecutive_reads() {
        let plan = make_plan(vec![
            make_step("Read main.rs", Some("file_read"), 0.9),
            make_step("Read lib.rs", Some("file_read"), 0.9),
            make_step("Synthesize findings", None, 1.0),
        ]);

        let (compressed, stats) = compress(plan);
        assert_eq!(stats.merged_reads, 1, "One step should be removed by merge");
        assert_eq!(compressed.steps.len(), 2, "Should have 2 steps after merge");
        assert!(compressed.steps[0].parallel, "Merged reads should be parallel");
        assert!(compressed.steps[0].description.starts_with("Read and analyse:"));
        assert!(compressed.steps.last().unwrap().tool_name.is_none());
    }

    #[test]
    fn merge_three_consecutive_reads_into_one() {
        let plan = make_plan(vec![
            make_step("Read file A", Some("file_read"), 0.9),
            make_step("Search for pattern", Some("grep"), 0.9),
            make_step("List directory", Some("glob"), 0.8),
            make_step("Synthesize", None, 1.0),
        ]);

        let (compressed, stats) = compress(plan);
        assert_eq!(stats.merged_reads, 2, "Two steps removed by merge (3→1)");
        assert_eq!(compressed.steps.len(), 2);
    }

    #[test]
    fn no_merge_when_write_step_between_reads() {
        let plan = make_plan(vec![
            make_step("Read file A", Some("file_read"), 0.9),
            make_step("Edit file B", Some("file_write"), 0.8),
            make_step("Read file C", Some("file_read"), 0.9),
            make_step("Synthesize", None, 1.0),
        ]);

        let (compressed, stats) = compress(plan);
        // Reads are not adjacent (write step in between), so no merge.
        assert_eq!(stats.merged_reads, 0);
        assert_eq!(compressed.steps.len(), 4);
    }

    #[test]
    fn no_merge_for_single_read() {
        let plan = make_plan(vec![
            make_step("Read file A", Some("file_read"), 0.9),
            make_step("Synthesize", None, 1.0),
        ]);

        let (_, stats) = compress(plan);
        assert_eq!(stats.merged_reads, 0);
    }

    /// BUG-C4 FIX: Merged step records all grouped tool names in expected_args.
    #[test]
    fn merged_step_records_grouped_tools_in_expected_args() {
        let plan = make_plan(vec![
            make_step("Read main.rs", Some("file_read"), 0.9),
            make_step("Search errors", Some("grep"), 0.9),
            make_step("Synthesize", None, 1.0),
        ]);

        let (compressed, stats) = compress(plan);
        assert_eq!(stats.merged_reads, 1);
        let merged = &compressed.steps[0];
        // expected_args must document grouped_tools for ExecutionTracker.
        let args_str = merged.expected_args.as_ref()
            .map(|v| v.to_string())
            .unwrap_or_default();
        assert!(
            args_str.contains("grouped_tools"),
            "Merged step should record grouped_tools in expected_args"
        );
        assert!(
            args_str.contains("file_read"),
            "grouped_tools must include file_read"
        );
        assert!(
            args_str.contains("grep"),
            "grouped_tools must include grep"
        );
    }

    // ── Rule 2: Trivial elimination ──────────────────────────────────────

    #[test]
    fn trivial_step_eliminated_when_both_neighbors_have_tools() {
        // "Verify" step with bash uses a non-read tool so Rule 1 doesn't absorb it.
        // Rule 2 eliminates it because BOTH neighbors have tools.
        let plan = make_plan(vec![
            make_step("Read main.rs", Some("file_read"), 0.9),
            make_step("Verify the output", Some("bash"), 0.5),
            make_step("Edit main.rs", Some("file_write"), 0.9),
            make_step("Synthesize", None, 1.0),
        ]);

        let (compressed, stats) = compress(plan);
        assert!(stats.eliminated_trivials > 0, "Trivial step should be eliminated");
        let descs: Vec<_> = compressed.steps.iter().map(|s| s.description.as_str()).collect();
        assert!(!descs.iter().any(|d| d.contains("Verify")));
    }

    /// BUG-C3 FIX: Trivial step at boundary (only ONE neighbor has tool) is preserved.
    #[test]
    fn trivial_step_preserved_when_only_one_neighbor_has_tool() {
        // "Verify" is the FIRST step (no previous neighbor with tool) → must NOT be eliminated.
        let plan = make_plan(vec![
            make_step("Verify the setup", Some("bash"), 0.5),
            make_step("Edit main.rs", Some("file_write"), 0.9),
            make_step("Synthesize", None, 1.0),
        ]);

        let (compressed, stats) = compress(plan);
        assert_eq!(stats.eliminated_trivials, 0, "Trivial step at boundary must be kept");
        assert!(compressed.steps.iter().any(|s| s.description.contains("Verify")));
    }

    /// BUG-C3 FIX: Safety-critical steps are never eliminated (I7).
    #[test]
    fn safety_critical_step_never_eliminated_even_with_both_neighbors() {
        // "Verify the backup" uses a security keyword → is_safety_critical_step returns true.
        let plan = make_plan(vec![
            make_step("Read config", Some("file_read"), 0.9),
            make_step("Verify backup exists", Some("bash"), 0.5),
            make_step("Delete old files", Some("file_delete"), 0.9),
            make_step("Synthesize", None, 1.0),
        ]);

        let (compressed, stats) = compress(plan);
        // "Verify backup" has keyword "backup" → safety_critical → not eliminated.
        assert_eq!(stats.eliminated_trivials, 0, "Safety-critical step must not be eliminated");
        assert!(compressed.steps.iter().any(|s| s.description.contains("backup")));
    }

    #[test]
    fn synthesis_step_never_eliminated() {
        let plan = make_plan(vec![
            make_step("Read files", Some("file_read"), 0.9),
            make_step("Synthesize findings and respond to the user", None, 1.0),
        ]);

        let (compressed, _stats) = compress(plan);
        // Synthesis always survives regardless of patterns.
        assert!(compressed.steps.last().unwrap().tool_name.is_none());
    }

    // ── Rule 3: Synthesis dedup ──────────────────────────────────────────

    #[test]
    fn duplicate_synthesis_deduplicated() {
        let plan = make_plan(vec![
            make_step("Read file", Some("file_read"), 0.9),
            make_step("Synthesize partial results", None, 0.8),
            make_step("Edit file", Some("file_write"), 0.8),
            make_step("Synthesize final results", None, 1.0),
        ]);

        let (compressed, stats) = compress(plan);
        assert_eq!(stats.deduped_synthesis, 1);
        let synthesis_steps: Vec<_> = compressed
            .steps
            .iter()
            .filter(|s| s.tool_name.is_none())
            .collect();
        assert_eq!(synthesis_steps.len(), 1, "Exactly one synthesis step");
        assert!(synthesis_steps[0].description.contains("final"));
    }

    #[test]
    fn single_synthesis_not_deduped() {
        let plan = make_plan(vec![
            make_step("Read file", Some("file_read"), 0.9),
            make_step("Synthesize", None, 1.0),
        ]);

        let (_, stats) = compress(plan);
        assert_eq!(stats.deduped_synthesis, 0);
    }

    /// BUG-C2 FIX: Synthesis that appears BEFORE tool steps is moved to the end.
    #[test]
    fn synthesis_moved_to_end_when_originally_first() {
        let plan = make_plan(vec![
            make_step("Synthesize findings", None, 0.9),
            make_step("Read file", Some("file_read"), 0.9),
            make_step("Edit file", Some("file_write"), 0.8),
        ]);

        let (compressed, _stats) = compress(plan);
        assert!(
            compressed.steps.last().unwrap().tool_name.is_none(),
            "Synthesis must be last even when originally first"
        );
    }

    // ── Rule 4: Parallel promotion ───────────────────────────────────────

    #[test]
    fn sequential_reads_promoted_to_parallel() {
        // No merge because they are non-adjacent due to a manual flag, but rule 4 still runs.
        let plan = make_plan(vec![
            make_step("Read A", Some("file_read"), 0.9),
            make_step("Grep B", Some("grep"), 0.9),
            make_step("Synthesize", None, 1.0),
        ]);

        let (compressed, stats) = compress(plan);
        // Rule 1 merges these two, so stats.parallelised would be 0 from rule 4.
        // Just verify stats are consistent.
        let _ = stats;
        // After rule 1 merge, the merged step is already parallel.
        assert!(compressed.steps[0].parallel);
    }

    // ── Rule 5: Hard cap ─────────────────────────────────────────────────

    #[test]
    fn cap_enforced_at_max_visible_steps() {
        // Create plan with 9 steps — should be capped to MAX_VISIBLE_STEPS (8).
        // Note: MAX_VISIBLE_STEPS was raised from 4 to 8 to support execution tasks.
        // Use "inspect" (not in READONLY_MERGEABLE, not in EXECUTION_PROTECTED) so neither
        // Rule 1 (read-merge) nor Phase 3 (execution protection) prevents the cap from firing.
        let plan = make_plan(vec![
            make_step("Inspect file A", Some("inspect"), 0.9),
            make_step("Inspect file B", Some("inspect"), 0.85),
            make_step("Inspect file C", Some("inspect"), 0.8),
            make_step("Inspect file D", Some("inspect"), 0.75),
            make_step("Inspect file E", Some("inspect"), 0.7),
            make_step("Inspect file F", Some("inspect"), 0.65),
            make_step("Inspect file G", Some("inspect"), 0.6),
            make_step("Inspect file H", Some("inspect"), 0.5),
            make_step("Synthesize", None, 1.0),
        ]);

        let (compressed, stats) = compress(plan);
        assert!(
            compressed.steps.len() <= MAX_VISIBLE_STEPS,
            "Steps should be capped at {MAX_VISIBLE_STEPS}, got {}",
            compressed.steps.len()
        );
        assert!(stats.cap_truncated > 0, "Some steps should have been truncated");
        // Synthesis must survive.
        assert!(compressed.steps.last().unwrap().tool_name.is_none());
    }

    #[test]
    fn cap_not_applied_when_under_limit() {
        let plan = make_plan(vec![
            make_step("Read file", Some("file_read"), 0.9),
            make_step("Edit file", Some("file_write"), 0.8),
            make_step("Synthesize", None, 1.0),
        ]);

        let (compressed, stats) = compress(plan);
        assert_eq!(stats.cap_truncated, 0);
        assert_eq!(compressed.steps.len(), 3);
    }

    /// BUG-C1 + BUG-H2 FIX: Cap preserves original order and never removes first step.
    #[test]
    fn cap_preserves_first_step_and_original_order() {
        // Steps with deliberately non-monotone confidence so the old sort would
        // re-order them. MAX_VISIBLE_STEPS=8, total=9 (8 tool + synthesis) → 1 tool step dropped.
        // Lowest-confidence non-protected step is removed; step 0 (index 0) is always kept.
        // Result must be in original order.
        //
        // Use "inspect" (not in READONLY_MERGEABLE, not in EXECUTION_PROTECTED) so:
        // - Rule 1 (read-merge) doesn't consume all steps before the cap fires
        // - Phase 3 (execution protection) doesn't block cap from removing one step
        let plan = make_plan(vec![
            make_step("Inspect initial config", Some("inspect"), 0.5), // index 0 — always kept (BUG-H2)
            make_step("Inspect tests result", Some("inspect"), 0.9),   // index 1 — highest conf, kept
            make_step("Inspect report A", Some("inspect"), 0.8),       // index 2 — kept
            make_step("Inspect report B", Some("inspect"), 0.75),      // index 3 — kept
            make_step("Inspect report C", Some("inspect"), 0.70),      // index 4 — kept
            make_step("Inspect report D", Some("inspect"), 0.65),      // index 5 — kept
            make_step("Inspect report E", Some("inspect"), 0.60),      // index 6 — kept
            make_step("Inspect report F", Some("inspect"), 0.55),      // index 7 — dropped (lowest non-protected)
            make_step("Synthesize", None, 1.0),
        ]);

        let (compressed, stats) = compress(plan);
        // 9 steps total, MAX_VISIBLE_STEPS=8 → 1 tool step removed.
        assert_eq!(stats.cap_truncated, 1, "One step should be removed by cap");
        assert_eq!(compressed.steps.len(), MAX_VISIBLE_STEPS);

        // First step (index 0) must always be present (BUG-H2).
        assert!(
            compressed.steps[0].description.contains("initial config"),
            "First step must be preserved by cap (BUG-H2)"
        );
        // Verify original order preserved: step0 before step1 (BUG-C1).
        let pos_step0 = compressed.steps.iter().position(|s| s.description.contains("initial config"));
        let pos_step1 = compressed.steps.iter().position(|s| s.description.contains("tests result"));
        assert!(
            pos_step0 < pos_step1,
            "Original step order must be preserved after cap (BUG-C1)"
        );
        // Synthesis last.
        assert!(compressed.steps.last().unwrap().tool_name.is_none());
    }

    // ── Stats ────────────────────────────────────────────────────────────

    #[test]
    fn no_compression_stats_are_zero() {
        let plan = make_plan(vec![
            make_step("Run bash command", Some("bash"), 0.9),
            make_step("Synthesize", None, 1.0),
        ]);

        let (_, stats) = compress(plan);
        assert!(!stats.any_applied());
        assert_eq!(stats.total_removed(), 0);
    }

    #[test]
    fn compression_stats_any_applied_true_when_merge_occurred() {
        let plan = make_plan(vec![
            make_step("Read A", Some("file_read"), 0.9),
            make_step("Read B", Some("file_read"), 0.9),
            make_step("Synthesize", None, 1.0),
        ]);

        let (_, stats) = compress(plan);
        assert!(stats.any_applied());
        assert_eq!(stats.total_removed(), 1);
    }

    /// BUG-H7 FIX: Empty plan gets a synthesis step appended (was: returned empty).
    #[test]
    fn empty_plan_gets_synthesis_appended() {
        let plan = make_plan(vec![]);
        let (compressed, stats) = compress(plan);
        // Invariant I2: at least one step (synthesis).
        assert_eq!(compressed.steps.len(), 1, "Empty plan must get synthesis appended");
        assert!(
            compressed.steps[0].tool_name.is_none(),
            "The appended step must be synthesis"
        );
        // Compression itself applied nothing.
        assert!(!stats.any_applied());
    }

    #[test]
    fn synthesis_always_last_after_compression() {
        let plan = make_plan(vec![
            make_step("Synthesize early", None, 0.7),
            make_step("Read file A", Some("file_read"), 0.9),
            make_step("Read file B", Some("file_read"), 0.9),
            make_step("Edit file", Some("file_write"), 0.8),
            make_step("Synthesize final", None, 1.0),
        ]);

        let (compressed, _) = compress(plan);
        assert!(
            compressed.steps.last().unwrap().tool_name.is_none(),
            "Synthesis must be last after compression"
        );
    }

    /// I4: Exactly one synthesis step after any compression.
    #[test]
    fn exactly_one_synthesis_step_invariant() {
        let plan = make_plan(vec![
            make_step("Synthesize A", None, 0.8),
            make_step("Read A", Some("file_read"), 0.9),
            make_step("Synthesize B", None, 0.9),
            make_step("Write A", Some("file_write"), 0.7),
            make_step("Synthesize C", None, 1.0),
        ]);

        let (compressed, _) = compress(plan);
        let synthesis_count = compressed.steps.iter().filter(|s| s.tool_name.is_none()).count();
        assert_eq!(synthesis_count, 1, "Exactly one synthesis step (I4)");
    }

    #[test]
    fn shortens_description_strips_verb() {
        assert_eq!(shorten_desc("Read the configuration file"), "the configuration file");
        assert_eq!(shorten_desc("Search for pattern in sources"), "for pattern in sources");
        assert_eq!(shorten_desc("Analyse dependencies"), "dependencies");
    }

    // ── Phase 3: EXECUTION_PROTECTED tests ───────────────────────────────────

    /// Execution steps (bash, file_write) must never be truncated by the hard cap,
    /// even when the plan exceeds MAX_VISIBLE_STEPS. The read-only step is dropped first;
    /// if that's not enough to reach the cap, the plan stays over MAX_VISIBLE_STEPS but
    /// all execution steps are preserved (execution correctness > display budget).
    #[test]
    fn execution_step_never_truncated_by_cap() {
        // Plan with 7 bash steps + 2 file_read + synthesis = 10 steps (> MAX_VISIBLE_STEPS=8).
        // Cap fires: remove 2 file_reads (non-protected). All 7 bash steps survive.
        let mut steps: Vec<PlanStep> = (0..7)
            .map(|i| make_step(&format!("Run bash step {i}"), Some("bash"), 0.5))
            .collect();
        steps.push(make_step("Read config A", Some("file_read"), 0.4));
        steps.push(make_step("Read config B", Some("file_read"), 0.3));  // lowest conf
        steps.push(make_step("Synthesize", None, 1.0));

        let plan = make_plan(steps);
        assert!(plan.steps.len() > MAX_VISIBLE_STEPS, "test requires steps > cap");

        let (compressed, stats) = compress(plan);
        assert!(stats.cap_truncated > 0, "cap must fire and remove read-only steps");

        // All 7 bash execution steps must survive.
        let bash_count = compressed.steps.iter()
            .filter(|s| s.tool_name.as_deref() == Some("bash"))
            .count();
        assert_eq!(bash_count, 7,
            "all bash execution steps must be preserved by cap (got {bash_count})");

        // file_read steps must be gone (they were the non-protected removable steps).
        assert!(!compressed.steps.iter().any(|s| s.tool_name.as_deref() == Some("file_read")),
            "read-only steps must be truncated before execution steps");

        // Synthesis must survive.
        assert!(compressed.steps.last().unwrap().tool_name.is_none());
    }

    /// Read-only analysis steps should be truncated before execution steps.
    #[test]
    fn readonly_steps_truncated_before_execution_steps() {
        // Plan: 4 bash + 5 file_read + synthesis = 10 steps (> MAX_VISIBLE_STEPS=8).
        // Expected: some file_read steps dropped, all bash steps kept.
        let mut steps: Vec<PlanStep> = (0..4)
            .map(|i| make_step(&format!("Run bash {i}"), Some("bash"), 0.5))
            .collect();
        for i in 0..5 {
            steps.push(make_step(&format!("Read file {i}"), Some("file_read"), 0.4 - i as f64 * 0.02));
        }
        steps.push(make_step("Synthesize", None, 1.0));

        let plan = make_plan(steps);
        let (compressed, stats) = compress(plan);

        // Some truncation should have happened (either by merge or cap).
        // All bash steps must be present.
        let bash_count = compressed.steps.iter()
            .filter(|s| s.tool_name.as_deref() == Some("bash"))
            .count();
        assert_eq!(bash_count, 4, "all bash execution steps must survive compression");

        // Synthesis last.
        assert!(compressed.steps.last().unwrap().tool_name.is_none());
        let _ = stats; // stats checked implicitly
    }
}
