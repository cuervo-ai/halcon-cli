//! Auto-memory: agent writes session learnings to structured Markdown files.
//!
//! # Architecture
//!
//! ```text
//! result_assembly::build()
//!        │
//!        ▼ (tokio::spawn — never blocks response)
//! AutoMemory::record_session()
//!        │
//!        ├── scorer::classify_trigger()  → MemoryTrigger or skip
//!        ├── scorer::score()             → importance: f32
//!        │
//!        │  (importance < threshold → return early)
//!        │
//!        ├── build_session_summary()
//!        ├── writer::write_project_memory()   → .halcon/memory/MEMORY.md
//!        └── writer::write_user_memory()      → ~/.halcon/memory/<repo>/MEMORY.md
//! ```
//!
//! # Memory file locations
//!
//! - **Project**: `.halcon/memory/MEMORY.md` + `.halcon/memory/<topic>.md`
//! - **User**:    `~/.halcon/memory/<repo-name>/MEMORY.md`
//!
//! # Injection (session start, round 1 only)
//!
//! ```text
//! injector::build_injection() → "## Agent Memory\n\n<first 200 lines>"
//! ```
//! Inserted between the HALCON.md block and the user message at session start.
//!
//! # References
//!
//! - Sumers et al. (2024) CoALA: Cognitive Architectures for Language Agents, arXiv:2309.02427
//! - Packer et al. (2023) MemGPT, arXiv:2310.08560

pub mod injector;
pub mod scorer;
pub mod writer;

use std::path::Path;
use std::sync::Arc;

use chrono::Utc;
use halcon_core::types::PolicyConfig;

use crate::repl::agent_types::AgentLoopResult;

/// Lightweight snapshot of `AgentLoopResult` fields needed for memory scoring.
///
/// Used to avoid cloning the full `AgentLoopResult` (which contains non-Clone fields
/// like `ControlReceiver`).  Populated before `result_assembly::build()` returns.
#[derive(Debug, Clone)]
pub struct MemoryResultSnapshot {
    pub rounds: usize,
    pub stop_condition: crate::repl::agent_types::StopCondition,
    pub critic_verdict: Option<crate::repl::agent_types::CriticVerdictSummary>,
    pub tool_trust_failures: Vec<crate::repl::retry_mutation::ToolFailureRecord>,
    pub tools_executed: Vec<String>,
}

/// Variant of `record_session` that operates on a `MemoryResultSnapshot`.
///
/// Called from `tokio::spawn` after `result_assembly::build()` — fire-and-forget.
pub fn record_session_snapshot(
    snapshot: &MemoryResultSnapshot,
    user_goal: &str,
    working_dir: &str,
    repo_name: &str,
    policy: &Arc<PolicyConfig>,
) {
    if !policy.enable_auto_memory {
        return;
    }

    let trigger = match classify_trigger_snapshot(snapshot) {
        Some(t) => t,
        None => {
            tracing::debug!("auto_memory: no trigger classified, skipping write");
            return;
        }
    };

    let importance = score_snapshot(snapshot, &trigger);
    if importance < policy.memory_importance_threshold {
        tracing::debug!(
            "auto_memory: importance {importance:.2} below threshold {:.2}, skipping",
            policy.memory_importance_threshold
        );
        return;
    }

    let summary = build_summary_from_snapshot(snapshot, user_goal, trigger, importance);
    let working_path = Path::new(working_dir);

    if let Some(halcon_dir) = find_halcon_dir(working_path) {
        writer::write_project_memory(&halcon_dir, &summary);
    } else {
        let halcon_dir = working_path.join(".halcon");
        if std::fs::create_dir_all(&halcon_dir).is_ok() {
            writer::write_project_memory(&halcon_dir, &summary);
        }
    }

    writer::write_user_memory(repo_name, &summary);

    tracing::debug!(
        "auto_memory: wrote memory entry (trigger={}, importance={:.2})",
        summary.trigger_tag,
        summary.importance
    );
}

fn classify_trigger_snapshot(snapshot: &MemoryResultSnapshot) -> Option<MemoryTrigger> {
    if !snapshot.tool_trust_failures.is_empty() {
        return Some(MemoryTrigger::ErrorRecovery);
    }
    if matches!(
        snapshot.stop_condition,
        crate::repl::agent_types::StopCondition::EndTurn
    ) {
        if let Some(ref v) = snapshot.critic_verdict {
            if v.achieved && v.confidence >= 0.6 {
                return Some(MemoryTrigger::TaskSuccess);
            }
        }
        let distinct: std::collections::HashSet<_> = snapshot.tools_executed.iter().collect();
        if distinct.len() >= 3 {
            return Some(MemoryTrigger::ToolPatternDiscovered);
        }
    }
    None
}

fn score_snapshot(snapshot: &MemoryResultSnapshot, trigger: &MemoryTrigger) -> f32 {
    let mut score: f32 = 0.0;
    match trigger {
        MemoryTrigger::UserCorrection => return 1.0,
        MemoryTrigger::ErrorRecovery => {
            score += 0.5;
            let extra = snapshot.tool_trust_failures.len().saturating_sub(1);
            score += (extra as f32 * 0.08).min(0.25);
        }
        MemoryTrigger::ToolPatternDiscovered => score += 0.6,
        MemoryTrigger::TaskSuccess => score += 0.2,
    }
    if let Some(ref v) = snapshot.critic_verdict {
        if v.achieved {
            score += 0.1 + v.confidence * 0.2;
        }
    }
    if snapshot.rounds >= 5 {
        score += 0.1;
    }
    let distinct: std::collections::HashSet<_> = snapshot.tools_executed.iter().collect();
    if distinct.len() >= 3 {
        score += 0.1;
    }
    if matches!(
        snapshot.stop_condition,
        crate::repl::agent_types::StopCondition::EndTurn
    ) {
        score += 0.05;
    }
    score.clamp(0.0, 1.0)
}

fn build_summary_from_snapshot(
    snapshot: &MemoryResultSnapshot,
    user_goal: &str,
    trigger: MemoryTrigger,
    importance: f32,
) -> SessionSummary {
    let timestamp = Utc::now().format("%Y-%m-%dT%H:%MZ").to_string();
    let trigger_tag = trigger.to_string();
    let goal_excerpt: String = user_goal.chars().take(80).collect();
    let goal_excerpt = if user_goal.len() > 80 {
        format!("{goal_excerpt}…")
    } else {
        goal_excerpt
    };

    let one_liner = match &trigger {
        MemoryTrigger::UserCorrection => format!("user correction during: {goal_excerpt}"),
        MemoryTrigger::ErrorRecovery => {
            let tools: Vec<&str> = snapshot
                .tool_trust_failures
                .iter()
                .map(|f| f.tool_name.as_str())
                .collect();
            format!(
                "recovered from {} failure(s) [{}] during: {goal_excerpt}",
                tools.len(),
                tools.join(", ")
            )
        }
        MemoryTrigger::ToolPatternDiscovered => {
            let distinct: std::collections::HashSet<_> = snapshot.tools_executed.iter().collect();
            let sample: Vec<_> = distinct.into_iter().take(3).collect();
            format!(
                "effective tool pattern [{}] for: {goal_excerpt}",
                sample
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        MemoryTrigger::TaskSuccess => {
            format!(
                "{} rounds, critic={} during: {goal_excerpt}",
                snapshot.rounds,
                snapshot
                    .critic_verdict
                    .as_ref()
                    .map(|v| v.achieved)
                    .unwrap_or(false),
            )
        }
    };

    let details = match &trigger {
        MemoryTrigger::ErrorRecovery => {
            let lines: Vec<String> = snapshot
                .tool_trust_failures
                .iter()
                .map(|f| format!("- `{}` failed {} time(s)", f.tool_name, f.failure_count))
                .collect();
            if lines.is_empty() {
                None
            } else {
                Some(format!(
                    "**Tool failures recovered:**\n\n{}",
                    lines.join("\n")
                ))
            }
        }
        MemoryTrigger::ToolPatternDiscovered => {
            let mut sorted: Vec<_> = snapshot.tools_executed.iter().collect();
            sorted.sort();
            sorted.dedup();
            let lines: Vec<String> = sorted.iter().map(|t| format!("- `{t}`")).collect();
            Some(format!("**Tools used:**\n\n{}", lines.join("\n")))
        }
        MemoryTrigger::TaskSuccess => {
            let achieved = snapshot
                .critic_verdict
                .as_ref()
                .map(|v| v.achieved)
                .unwrap_or(false);
            let conf = snapshot
                .critic_verdict
                .as_ref()
                .map(|v| v.confidence)
                .unwrap_or(0.0);
            Some(format!(
                "**Session stats:** rounds={}, critic_achieved={achieved}, confidence={conf:.2}",
                snapshot.rounds
            ))
        }
        MemoryTrigger::UserCorrection => None,
    };

    SessionSummary {
        timestamp,
        trigger_tag,
        importance,
        one_liner,
        details,
    }
}

/// Memory write trigger classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryTrigger {
    /// User explicitly corrected the agent's answer — highest signal.
    UserCorrection,
    /// Agent encountered tool failures but recovered (e.g., path errors).
    ErrorRecovery,
    /// Agent discovered an effective tool pattern not used before.
    ToolPatternDiscovered,
    /// Task completed successfully with critic confirmation.
    TaskSuccess,
}

impl std::fmt::Display for MemoryTrigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryTrigger::UserCorrection => write!(f, "UserCorrection"),
            MemoryTrigger::ErrorRecovery => write!(f, "ErrorRecovery"),
            MemoryTrigger::ToolPatternDiscovered => write!(f, "ToolPatternDiscovered"),
            MemoryTrigger::TaskSuccess => write!(f, "TaskSuccess"),
        }
    }
}

/// A structured record of what the agent learned in a session.
#[derive(Debug)]
pub struct SessionSummary {
    /// ISO-8601 UTC timestamp of when the session ended.
    pub timestamp: String,
    /// String representation of the trigger type.
    pub trigger_tag: String,
    /// Importance score [0.0, 1.0].
    pub importance: f32,
    /// Single-line summary (≤120 chars) for the MEMORY.md index.
    pub one_liner: String,
    /// Optional multi-line detail block written to the topic file.
    pub details: Option<String>,
}

/// Entry point called from `result_assembly` in a `tokio::spawn` task.
///
/// This function is fire-and-forget: all errors are logged at debug level and
/// silently swallowed so that memory writes never affect session latency or stability.
pub fn record_session(
    result: &AgentLoopResult,
    user_goal: &str,
    working_dir: &str,
    repo_name: &str,
    policy: &Arc<PolicyConfig>,
) {
    if !policy.enable_auto_memory {
        return;
    }

    let trigger = match scorer::classify_trigger(result) {
        Some(t) => t,
        None => {
            tracing::debug!("auto_memory: no trigger classified, skipping write");
            return;
        }
    };

    let importance = scorer::score(result, &trigger);
    if importance < policy.memory_importance_threshold {
        tracing::debug!(
            "auto_memory: importance {importance:.2} below threshold {:.2}, skipping",
            policy.memory_importance_threshold
        );
        return;
    }

    let summary = build_summary(result, user_goal, trigger, importance);

    let working_path = Path::new(working_dir);

    // Locate the .halcon/ directory for project memory.
    if let Some(halcon_dir) = find_halcon_dir(working_path) {
        writer::write_project_memory(&halcon_dir, &summary);
    } else {
        // Create .halcon/ if it doesn't exist (first-time project setup).
        let halcon_dir = working_path.join(".halcon");
        if std::fs::create_dir_all(&halcon_dir).is_ok() {
            writer::write_project_memory(&halcon_dir, &summary);
        }
    }

    writer::write_user_memory(repo_name, &summary);

    tracing::debug!(
        "auto_memory: wrote memory entry (trigger={}, importance={:.2})",
        summary.trigger_tag,
        summary.importance
    );
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn build_summary(
    result: &AgentLoopResult,
    user_goal: &str,
    trigger: MemoryTrigger,
    importance: f32,
) -> SessionSummary {
    let timestamp = Utc::now().format("%Y-%m-%dT%H:%MZ").to_string();
    let trigger_tag = trigger.to_string();

    // Truncate goal to 80 chars for the one-liner.
    let goal_excerpt: String = user_goal.chars().take(80).collect();
    let goal_excerpt = if user_goal.len() > 80 {
        format!("{goal_excerpt}…")
    } else {
        goal_excerpt
    };

    let one_liner = match &trigger {
        MemoryTrigger::UserCorrection => {
            format!("user correction during: {goal_excerpt}")
        }
        MemoryTrigger::ErrorRecovery => {
            let failures: Vec<&str> = result
                .tool_trust_failures
                .iter()
                .map(|f| f.tool_name.as_str())
                .collect();
            let tool_list = failures.join(", ");
            format!(
                "recovered from {n} tool failure(s) [{tool_list}] during: {goal_excerpt}",
                n = result.tool_trust_failures.len()
            )
        }
        MemoryTrigger::ToolPatternDiscovered => {
            let distinct: std::collections::HashSet<_> = result.tools_executed.iter().collect();
            let sample: Vec<_> = distinct.into_iter().take(3).collect();
            format!(
                "effective tool pattern [{tools}] for: {goal_excerpt}",
                tools = sample
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        MemoryTrigger::TaskSuccess => {
            format!(
                "{rounds} rounds, critic={achieved} during: {goal_excerpt}",
                rounds = result.rounds,
                achieved = result
                    .critic_verdict
                    .as_ref()
                    .map(|v| v.achieved)
                    .unwrap_or(false),
            )
        }
    };

    let details = build_details(result, &trigger);

    SessionSummary {
        timestamp,
        trigger_tag,
        importance,
        one_liner,
        details,
    }
}

fn build_details(result: &AgentLoopResult, trigger: &MemoryTrigger) -> Option<String> {
    match trigger {
        MemoryTrigger::ErrorRecovery => {
            let lines: Vec<String> = result
                .tool_trust_failures
                .iter()
                .map(|f| format!("- `{}` failed {} time(s)", f.tool_name, f.failure_count))
                .collect();
            if lines.is_empty() {
                None
            } else {
                Some(format!(
                    "**Tool failures recovered:**\n\n{}",
                    lines.join("\n")
                ))
            }
        }
        MemoryTrigger::ToolPatternDiscovered => {
            let distinct: std::collections::HashSet<_> = result.tools_executed.iter().collect();
            let mut sorted: Vec<_> = distinct.into_iter().collect();
            sorted.sort();
            let lines: Vec<String> = sorted.iter().map(|t| format!("- `{t}`")).collect();
            Some(format!("**Tools used:**\n\n{}", lines.join("\n")))
        }
        MemoryTrigger::TaskSuccess => {
            let achieved = result
                .critic_verdict
                .as_ref()
                .map(|v| v.achieved)
                .unwrap_or(false);
            let confidence = result
                .critic_verdict
                .as_ref()
                .map(|v| v.confidence)
                .unwrap_or(0.0);
            Some(format!(
                "**Session stats:** rounds={}, critic_achieved={achieved}, confidence={confidence:.2}",
                result.rounds,
            ))
        }
        MemoryTrigger::UserCorrection => None,
    }
}

fn find_halcon_dir(working_dir: &Path) -> Option<std::path::PathBuf> {
    let mut current = working_dir;
    loop {
        let candidate = current.join(".halcon");
        if candidate.is_dir() {
            return Some(candidate);
        }
        current = current.parent()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repl::agent_types::{AgentLoopResult, CriticVerdictSummary, StopCondition};
    use std::fs;
    use tempfile::TempDir;

    fn base_result() -> AgentLoopResult {
        AgentLoopResult {
            full_text: String::new(),
            rounds: 3,
            stop_condition: StopCondition::EndTurn,
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
            latency_ms: 0,
            execution_fingerprint: String::new(),
            timeline_json: None,
            ctrl_rx: None,
            critic_verdict: None,
            round_evaluations: vec![],
            plan_completion_ratio: 0.0,
            avg_plan_drift: 0.0,
            oscillation_penalty: 0.0,
            last_model_used: None,
            plugin_cost_snapshot: vec![],
            tools_executed: vec!["bash".into(), "file_read".into(), "grep".into()],
            evidence_verified: true,
            content_read_attempts: 1,
            last_provider_used: None,
            blocked_tools: vec![],
            failed_sub_agent_steps: vec![],
            critic_unavailable: false,
            tool_trust_failures: vec![],
            sla_budget: None,
            evidence_coverage: 1.0,
            synthesis_kind: None,
            synthesis_trigger: None,
            routing_escalation_count: 0,
            response_trust: halcon_core::types::ResponseTrust::Unverified,
            decision_log: Vec::new(),
        }
    }

    fn policy_with_auto_memory(enabled: bool, threshold: f32) -> Arc<PolicyConfig> {
        let mut p = PolicyConfig::default();
        p.enable_auto_memory = enabled;
        p.memory_importance_threshold = threshold;
        Arc::new(p)
    }

    #[test]
    fn record_session_noop_when_disabled() {
        let dir = TempDir::new().unwrap();
        let halcon = dir.path().join(".halcon");
        fs::create_dir(&halcon).unwrap();

        let result = base_result();
        let policy = policy_with_auto_memory(false, 0.3);
        record_session(
            &result,
            "test goal",
            dir.path().to_str().unwrap(),
            "repo",
            &policy,
        );

        let memory = halcon.join("memory").join("MEMORY.md");
        assert!(!memory.exists(), "no memory file when feature disabled");
    }

    #[test]
    fn record_session_skips_below_threshold() {
        let dir = TempDir::new().unwrap();
        let halcon = dir.path().join(".halcon");
        fs::create_dir(&halcon).unwrap();

        // MaxRounds + no critic + no failures → None trigger → skip
        let mut result = base_result();
        result.stop_condition = StopCondition::MaxRounds;
        result.tools_executed = vec![];
        result.tool_trust_failures = vec![];
        result.critic_verdict = None;

        let policy = policy_with_auto_memory(true, 0.3);
        record_session(
            &result,
            "goal",
            dir.path().to_str().unwrap(),
            "repo",
            &policy,
        );

        // No trigger classified → no write
        let memory = halcon.join("memory").join("MEMORY.md");
        assert!(!memory.exists(), "should not write when no trigger");
    }

    #[test]
    fn record_session_writes_memory_on_task_success() {
        let dir = TempDir::new().unwrap();
        let halcon = dir.path().join(".halcon");
        fs::create_dir(&halcon).unwrap();

        let mut result = base_result();
        result.critic_verdict = Some(CriticVerdictSummary {
            achieved: true,
            confidence: 0.85,
            gaps: vec![],
            retry_instruction: None,
        });

        let policy = policy_with_auto_memory(true, 0.1); // low threshold to ensure write
        record_session(
            &result,
            "analyse security audit",
            dir.path().to_str().unwrap(),
            "repo",
            &policy,
        );

        let memory = halcon.join("memory").join("MEMORY.md");
        assert!(memory.exists(), "MEMORY.md should be created");
        let content = fs::read_to_string(&memory).unwrap();
        assert!(
            content.contains("TaskSuccess") || content.contains("ToolPatternDiscovered"),
            "should contain a trigger tag: {content}"
        );
    }

    #[test]
    fn build_summary_one_liner_truncates_long_goal() {
        let result = base_result();
        let long_goal = "a".repeat(200);
        let summary = build_summary(&result, &long_goal, MemoryTrigger::TaskSuccess, 0.5);
        assert!(
            summary.one_liner.len() <= 200,
            "one-liner should not be excessively long"
        );
        assert!(
            summary.one_liner.contains('…'),
            "should contain ellipsis for truncated goal"
        );
    }

    #[test]
    fn memory_trigger_display() {
        assert_eq!(MemoryTrigger::ErrorRecovery.to_string(), "ErrorRecovery");
        assert_eq!(MemoryTrigger::UserCorrection.to_string(), "UserCorrection");
        assert_eq!(MemoryTrigger::TaskSuccess.to_string(), "TaskSuccess");
        assert_eq!(
            MemoryTrigger::ToolPatternDiscovered.to_string(),
            "ToolPatternDiscovered"
        );
    }
}
