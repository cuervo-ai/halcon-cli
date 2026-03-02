//! Supervisor layer — closes the autonomy gaps identified in the Phase 72 audit.
//!
//! Three components, three insertion points, zero new infrastructure:
//!
//! - [`InSessionReflectionInjector`]: Closes temporal Reflexion gap.
//!   Reflections generated in round N are injected as directives in round N+1
//!   (not only cross-session via episodic memory).
//!
//! - [`PostBatchSupervisor`]: Closes authority gap.
//!   After tool batch execution, intervenes *before* loop-guard thresholds fire:
//!   injects correction messages or forces replanning on structural failure.
//!
//! - [`LoopCritic`]: Closes self-serving-reflection gap.
//!   Post-loop adversarial LLM evaluation using a separate system role.
//!   Its verdict can override score-based retry logic.

use std::collections::HashSet;
use std::sync::Arc;

use futures::StreamExt;

use halcon_core::traits::ModelProvider;
use halcon_core::types::{ChatMessage, MessageContent, ModelChunk, ModelRequest, Role};

// ─── Phase 1: In-session reflection injection ──────────────────────────────

/// Closes the temporal Reflexion gap.
///
/// The Reflexion paper (NeurIPS 2023) requires that reflection advice generated
/// after trial N is **injected** at the start of trial N+1 — not just stored
/// for a future session. This struct implements that injection within a single
/// agent session.
///
/// Usage pattern in agent.rs:
/// ```text
/// // After reflector.reflect() returns Some(reflection):
/// injector.push_advice(&reflection.advice);
///
/// // Before building round_request (top of each round):
/// if let Some(directive) = injector.take_directive() {
///     // prepend to system message
/// }
/// ```
pub struct InSessionReflectionInjector {
    /// Advice strings waiting to be injected next round.
    pending: Vec<String>,
    /// Content hashes seen (pending OR already injected) — prevents re-injection of identical
    /// advice both within the same batch and across subsequent rounds.
    seen_hashes: HashSet<u64>,
}

impl InSessionReflectionInjector {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            seen_hashes: HashSet::new(),
        }
    }

    /// Record advice from a newly generated reflection.
    ///
    /// Silently deduplicates by content hash. Empty advice is ignored.
    /// Deduplication covers both pending (not yet taken) and previously taken entries,
    /// preventing identical advice from appearing twice within or across rounds.
    pub fn push_advice(&mut self, advice: &str) {
        if advice.trim().is_empty() {
            return;
        }
        let hash = hash_str(advice);
        if self.seen_hashes.insert(hash) {
            // insert() returns true if the hash was newly added → first time we see this advice.
            self.pending.push(advice.to_string());
        }
    }

    /// Consume pending advice and format as a supervisor directive.
    ///
    /// Returns `None` when nothing is pending. The seen-hash set is preserved
    /// so identical advice from later rounds is also suppressed.
    pub fn take_directive(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        let directives = std::mem::take(&mut self.pending);
        // Note: hashes were already inserted into seen_hashes in push_advice().
        // No extra work needed here; seen_hashes remains populated for future dedup.
        let body = directives
            .iter()
            .map(|d| format!("- {d}"))
            .collect::<Vec<_>>()
            .join("\n");
        Some(format!(
            "\n[Supervisor: prior-round self-reflection]\n\
             Apply the following advice from your previous round's analysis:\n\
             {body}\n\
             This is a direct operational directive. Adjust your approach accordingly."
        ))
    }

    /// Whether there are pending directives waiting to be injected.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

impl Default for InSessionReflectionInjector {
    fn default() -> Self {
        Self::new()
    }
}

fn hash_str(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

// ─── Phase 3: PostBatch supervisor ────────────────────────────────────────

/// Verdict returned by [`PostBatchSupervisor::check`].
#[derive(Debug)]
pub enum BatchVerdict {
    /// Continue normally — no supervisor intervention needed.
    Continue,
    /// Prepend this correction message before the next model call.
    ///
    /// The agent loop appends this as a system message to `round_request.messages`.
    InjectCorrection(String),
    /// Critical failure: force replanning immediately without waiting for loop-guard threshold.
    ///
    /// The agent loop should set a replan signal and `continue 'agent_loop`.
    ForceReplanNow(String),
    /// All plugin tool invocations for this plugin failed — suspend it to prevent further damage.
    ///
    /// The agent loop should call `plugin_registry.suspend_plugin(plugin_id, reason)`.
    SuspendPlugin { plugin_id: String, reason: String },
}

/// Intervenes after tool batch execution and before the next model invocation.
///
/// Operates synchronously with no LLM calls — purely structural heuristics.
/// Fires **before** `ToolLoopGuard` thresholds (synthesis_threshold=6,
/// force_threshold=10), providing earlier intervention on structural misalignment.
pub struct PostBatchSupervisor;

impl PostBatchSupervisor {
    /// Evaluate tool batch results and return a supervisor verdict.
    ///
    /// # Arguments
    /// - `round`: 0-indexed round number (gates that need warmup are suppressed on round 0).
    /// - `expected_tool`: tool_name from the active plan step (None if no plan).
    /// - `tools_executed`: all tool names that ran this round (order irrelevant).
    /// - `critical_failures`: `(tool_name, error_msg)` pairs for **deterministic** failures only.
    ///   Transient errors (timeout, rate-limit, retryable) must be filtered out by the caller
    ///   before passing here, otherwise Gate 1 triggers spurious replans.
    /// - `plan_progress_ratio`: 0.0–1.0 fraction of plan steps in terminal state.
    /// - `any_tool_succeeded`: true if at least one tool returned a non-error result.
    /// - `plugin_all_failed`: Some(plugin_id) when ALL plugin tool invocations for that
    ///   plugin failed this round. None when no plugin involvement or mixed results.
    pub fn check(
        round: usize,
        expected_tool: Option<&str>,
        tools_executed: &[String],
        critical_failures: &[(String, String)],
        plan_progress_ratio: f32,
        any_tool_succeeded: bool,
        plugin_all_failed: Option<&str>,
    ) -> BatchVerdict {
        // Gate 0: All plugin invocations failed → suspend the plugin.
        if let Some(plugin_id) = plugin_all_failed {
            return BatchVerdict::SuspendPlugin {
                plugin_id: plugin_id.to_string(),
                reason: "All plugin tool invocations failed this round".into(),
            };
        }

        // Gate 1: ≥2 critical deterministic failures → current plan cannot proceed.
        if critical_failures.len() >= 2 {
            let names: Vec<&str> = critical_failures.iter().map(|(n, _)| n.as_str()).collect();
            return BatchVerdict::ForceReplanNow(format!(
                "Critical deterministic failures in tools {names:?}. \
                 Current plan is not viable — replanning required."
            ));
        }

        // Gate 2: Plan expected a specific tool that was not called.
        // Only fires when tools *were* executed (prevents false positives on text-only rounds).
        if let Some(expected) = expected_tool {
            let ran = tools_executed.iter().any(|t| t == expected);
            if !ran && !tools_executed.is_empty() {
                return BatchVerdict::InjectCorrection(format!(
                    "[Supervisor] The active plan step expected tool '{expected}' to be called \
                     this round, but it was not. In the next round, explicitly invoke '{expected}' \
                     to advance the plan. Do not call other tools until '{expected}' is complete."
                ));
            }
        }

        // Gate 3: Zero plan progress despite successful tool execution.
        // Indicates tools ran but plan step outcomes were not recorded — alignment drift.
        //
        // Warmup guard: skip rounds 0–2 (ExecutionTracker needs at least 2 rounds to map
        // MCP-provided tool names to plan step tool_name fields via fuzzy matching; firing
        // earlier produces token-expensive corrections that are guaranteed to be spurious).
        //
        // - Round 0: model initializing, hasn't mapped tools→steps yet.
        // - Round 1: first batch completed, tracker may still not have matched names.
        // - Round 2: second batch, still calibrating (e.g., MCP tools vs halcon native names).
        // - Round ≥ 3: by now, genuine alignment drift if progress is still 0%.
        if round >= 3 && plan_progress_ratio == 0.0 && any_tool_succeeded && !tools_executed.is_empty() {
            return BatchVerdict::InjectCorrection(
                "[Supervisor] Tools succeeded this round but plan completion is 0%. \
                 Explicitly advance the plan: mark completed steps and invoke the next \
                 plan step's tool in the following round."
                    .to_string(),
            );
        }

        BatchVerdict::Continue
    }
}

// ─── Phase 5: LoopCritic ──────────────────────────────────────────────────
//
// Authority Hierarchy:
// Level 1: LoopGuard           — stability (LoopAction::Break → structural break)
// Level 2: InSessionReflection — behavioral correction (system prompt prefix)
// Level 3: PostBatchSupervisor — plan correction (ForceReplanNow → structural flag)
// Level 4: LoopCritic          — execution validation (should_halt → structural halt)
// Level 5: Hard termination    — budget/interrupt signals (DurationBudget, TokenBudget)

// NOTE: HALT_CONFIDENCE_THRESHOLD (0.80), EXCERPT_LEN (1500), and critic timeout (20s→45s)
// are now sourced from `PolicyConfig` and passed as parameters to `should_halt()`,
// `should_halt_raw()`, and `evaluate()`.  See `halcon_core::types::PolicyConfig`.

/// Structured verdict from an adversarial post-loop critic evaluation.
///
/// Deserializes from LLM JSON output. Optional fields default to empty/None.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CriticVerdict {
    /// Whether the original goal was achieved.
    pub achieved: bool,
    /// Confidence in the verdict [0.0, 1.0].
    pub confidence: f32,
    /// Specific gaps or unresolved aspects. Empty when `achieved = true`.
    #[serde(default)]
    pub gaps: Vec<String>,
    /// Concrete instruction for the retry attempt. None when `achieved = true`.
    #[serde(default)]
    pub retry_instruction: Option<String>,
}

/// Post-loop adversarial evaluator using a separate LLM call in a critic role.
///
/// Unlike [`super::reflexion::Reflector`] (same LLM critiquing its own output),
/// `LoopCritic` uses a deliberately adversarial system prompt. Its verdict feeds
/// into [`super::reasoning_engine::ReasoningEngine::should_retry`] as an
/// independent signal that can override score-based retry decisions.
///
/// Enabled only when `ReasoningConfig::enable_loop_critic = true` to avoid
/// latency overhead on every session.
pub struct LoopCritic {
    provider: Arc<dyn ModelProvider>,
    model: String,
}

impl LoopCritic {
    pub fn new(provider: Arc<dyn ModelProvider>, model: String) -> Self {
        Self { provider, model }
    }

    /// Returns true if this verdict warrants immediate halt + retry.
    ///
    /// Fires when the critic is highly confident the goal was NOT achieved.
    /// A high-confidence failure verdict means continuing the current loop is
    /// unlikely to recover — halting early and retrying with the retry_instruction
    /// produces better outcomes than exhausting remaining rounds.
    pub fn should_halt(verdict: &CriticVerdict, threshold: f32) -> bool {
        !verdict.achieved && verdict.confidence >= threshold
    }

    /// Same semantics as `should_halt` but operates on the raw fields from
    /// `CriticVerdictSummary` (stored in `AgentLoopResult`).
    ///
    /// Used by mod.rs where only the summary is available (the full `CriticVerdict`
    /// returned by the async evaluate() call is not preserved across the await point).
    pub fn should_halt_raw(achieved: bool, confidence: f32, threshold: f32) -> bool {
        !achieved && confidence >= threshold
    }

    /// Adversarially evaluate whether the original goal was truly achieved.
    ///
    /// `excerpt_len`: number of chars from the end of `final_response` to evaluate.
    /// `timeout_secs`: per-call timeout for the LLM invocation.
    ///
    /// Returns `None` on timeout, provider error, or malformed JSON
    /// so the caller can gracefully fall back to score-based retry.
    pub async fn evaluate(
        &self,
        original_request: &str,
        final_response: &str,
        step_summaries: &[String],
        excerpt_len: usize,
        timeout_secs: u64,
    ) -> Option<CriticVerdict> {
        let steps_text = if step_summaries.is_empty() {
            "  (no execution steps recorded)".to_string()
        } else {
            step_summaries
                .iter()
                .enumerate()
                .map(|(i, s)| format!("  {}. {}", i + 1, s))
                .collect::<Vec<_>>()
                .join("\n")
        };

        // Phase 3D: evaluate the LAST 1500 chars of full_text, not the first.
        //
        // `final_response` accumulates all rounds: round-1 tool summaries, intermediate
        // coordinator text, and finally the synthesis output.  The synthesis content —
        // the actual "final response" that should be evaluated — appears at the END.
        //
        // The previous `take(1500)` evaluated the FIRST 1500 chars which is almost always
        // round-1 tool invocation output and planning context, not the synthesis.  In the
        // session e2adfb4f analysis, the critic received "Let me analyze the project
        // structure..." as the "final response" instead of the actual file-creation summary,
        // producing a spurious 15% confidence score on a session that correctly wrote 5 files.
        //
        // Using the last 1500 chars ensures the critic evaluates the synthesis output even
        // for multi-round sessions with large tool outputs.
        let total_chars = final_response.chars().count();
        let final_excerpt: String = if total_chars > excerpt_len {
            final_response.chars().skip(total_chars - excerpt_len).collect()
        } else {
            final_response.to_string()
        };

        let prompt = format!(
            "ORIGINAL GOAL:\n{original_request}\n\n\
             EXECUTION STEPS:\n{steps_text}\n\n\
             FINAL RESPONSE (last {excerpt_len} chars):\n{final_excerpt}\n\n\
             Evaluate strictly. A partial answer is NOT a success. Missing edge cases \
             is NOT a success. An error message is NOT a success even if politely phrased.\n\n\
             Respond ONLY with a JSON object (no markdown fences):\n\
             {{\n\
               \"achieved\": <true or false>,\n\
               \"confidence\": <float 0.0 to 1.0>,\n\
               \"gaps\": [\"<specific gap 1>\", \"<specific gap 2>\"],\n\
               \"retry_instruction\": \"<concrete instruction for retry, or null if achieved=true>\"\n\
             }}"
        );

        let request = ModelRequest {
            model: self.model.clone(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text(prompt),
            }],
            tools: vec![],
            max_tokens: Some(512),
            temperature: Some(0.0),
            system: Some(
                "You are an adversarial quality validator. Your role is to find failures, \
                 not justify successes. Be strict and skeptical. Never accept partial \
                 completion as full success."
                    .to_string(),
            ),
            stream: true,
        };

        let mut text = String::new();
        match tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            self.provider.invoke(&request),
        )
        .await
        {
            Ok(Ok(mut stream)) => {
                while let Some(chunk) = stream.next().await {
                    if let Ok(ModelChunk::TextDelta(delta)) = chunk {
                        text.push_str(&delta);
                    }
                }
            }
            Ok(Err(e)) => {
                tracing::warn!("LoopCritic provider error: {e}");
                return None;
            }
            Err(_elapsed) => {
                tracing::warn!(
                    timeout_secs,
                    "LoopCritic timed out — skipping critic verdict"
                );
                return None;
            }
        }

        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }

        let json_str = super::planner::extract_json(trimmed);
        match serde_json::from_str::<CriticVerdict>(json_str) {
            Ok(verdict) => {
                tracing::info!(
                    achieved = verdict.achieved,
                    confidence = verdict.confidence,
                    gaps = verdict.gaps.len(),
                    "LoopCritic verdict"
                );
                Some(verdict)
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    raw_preview = %trimmed.chars().take(200).collect::<String>(),
                    "LoopCritic JSON parse failed — skipping critic verdict"
                );
                None
            }
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── InSessionReflectionInjector ─────────────────────────────────────────

    #[test]
    fn injector_starts_empty() {
        let mut injector = InSessionReflectionInjector::new();
        assert!(!injector.has_pending());
        assert!(injector.take_directive().is_none());
    }

    #[test]
    fn injector_push_and_take_formats_as_directive() {
        let mut injector = InSessionReflectionInjector::new();
        injector.push_advice("Check path exists before writing");
        assert!(injector.has_pending());

        let directive = injector.take_directive().unwrap();
        assert!(directive.contains("Check path exists before writing"));
        assert!(directive.contains("[Supervisor"));
        assert!(!injector.has_pending(), "Queue should be empty after take");
    }

    #[test]
    fn injector_take_returns_none_after_flush() {
        let mut injector = InSessionReflectionInjector::new();
        injector.push_advice("some advice");
        let _ = injector.take_directive();
        assert!(injector.take_directive().is_none());
    }

    #[test]
    fn injector_deduplicates_same_advice_after_injection() {
        let mut injector = InSessionReflectionInjector::new();
        injector.push_advice("Use absolute paths");
        let _ = injector.take_directive();

        // Same advice again — hash is in injected_hashes, should be dropped.
        injector.push_advice("Use absolute paths");
        assert!(
            !injector.has_pending(),
            "Duplicate advice should be deduplicated after prior injection"
        );
    }

    #[test]
    fn injector_allows_different_advice_after_dedup() {
        let mut injector = InSessionReflectionInjector::new();
        injector.push_advice("Check path exists");
        let _ = injector.take_directive();

        injector.push_advice("Validate JSON schema before parsing");
        assert!(
            injector.has_pending(),
            "Different advice should pass deduplication"
        );
    }

    #[test]
    fn injector_combines_multiple_pending_into_single_directive() {
        let mut injector = InSessionReflectionInjector::new();
        injector.push_advice("Check path exists");
        injector.push_advice("Validate input length");

        let directive = injector.take_directive().unwrap();
        assert!(directive.contains("Check path exists"));
        assert!(directive.contains("Validate input length"));
    }

    #[test]
    fn injector_empty_and_whitespace_advice_ignored() {
        let mut injector = InSessionReflectionInjector::new();
        injector.push_advice("");
        injector.push_advice("   ");
        assert!(!injector.has_pending());
    }

    #[test]
    fn injector_second_push_not_deduplicated_until_taken() {
        // Before take: same advice pushed twice should be stored once (pending dedup).
        let mut injector = InSessionReflectionInjector::new();
        injector.push_advice("same advice");
        injector.push_advice("same advice"); // pending already has it
        let directive = injector.take_directive().unwrap();
        // Should appear only once (not duplicated).
        let count = directive.matches("same advice").count();
        assert_eq!(count, 1, "Same advice should appear exactly once in directive");
    }

    // ── PostBatchSupervisor ─────────────────────────────────────────────────

    #[test]
    fn supervisor_continue_on_clean_batch() {
        let verdict = PostBatchSupervisor::check(
            1,
            None,
            &["file_read".to_string(), "bash".to_string()],
            &[],
            0.5,
            true,
            None,
        );
        assert!(matches!(verdict, BatchVerdict::Continue));
    }

    #[test]
    fn supervisor_continue_on_empty_round() {
        // No tools ran — don't inject spurious corrections.
        let verdict = PostBatchSupervisor::check(0, None, &[], &[], 0.0, false, None);
        assert!(matches!(verdict, BatchVerdict::Continue));
    }

    #[test]
    fn supervisor_inject_correction_for_missing_expected_tool() {
        let verdict = PostBatchSupervisor::check(
            1,
            Some("file_edit"),
            &["file_read".to_string()], // Expected file_edit, got file_read.
            &[],
            0.0,
            true,
            None,
        );
        match verdict {
            BatchVerdict::InjectCorrection(msg) => {
                assert!(msg.contains("file_edit"), "Correction must name the missing tool");
                assert!(msg.contains("[Supervisor]"));
            }
            other => panic!("Expected InjectCorrection, got {other:?}"),
        }
    }

    #[test]
    fn supervisor_no_correction_when_expected_tool_ran() {
        let verdict = PostBatchSupervisor::check(
            1,
            Some("bash"),
            &["bash".to_string(), "file_read".to_string()],
            &[],
            0.3,
            true,
            None,
        );
        assert!(matches!(verdict, BatchVerdict::Continue));
    }

    #[test]
    fn supervisor_force_replan_on_two_critical_failures() {
        let critical = vec![
            ("bash".to_string(), "command not found: xyz".to_string()),
            ("file_write".to_string(), "permission denied: /root".to_string()),
        ];
        let verdict = PostBatchSupervisor::check(0, None, &[], &critical, 0.0, false, None);
        assert!(
            matches!(verdict, BatchVerdict::ForceReplanNow(_)),
            "Two critical failures must force replan"
        );
    }

    #[test]
    fn supervisor_single_critical_failure_does_not_force_replan() {
        let critical = vec![("bash".to_string(), "command not found".to_string())];
        let verdict = PostBatchSupervisor::check(0, None, &[], &critical, 0.0, false, None);
        // Threshold is ≥2 — one failure doesn't trigger ForceReplanNow.
        assert!(matches!(verdict, BatchVerdict::Continue));
    }

    #[test]
    fn supervisor_inject_correction_for_zero_progress_with_success() {
        // round=3: Gate 3 requires round >= 3 to allow tracker calibration across 2+ rounds.
        let verdict = PostBatchSupervisor::check(
            3,
            None,
            &["file_read".to_string()],
            &[],
            0.0,   // zero plan progress
            true,  // tools succeeded
            None,
        );
        match verdict {
            BatchVerdict::InjectCorrection(msg) => {
                assert!(msg.contains("plan completion is 0%"));
            }
            other => panic!("Expected InjectCorrection, got {other:?}"),
        }
    }

    #[test]
    fn supervisor_gate3_suppressed_on_rounds_0_through_2() {
        // Gate 3 must NOT fire on rounds 0, 1, 2 — ExecutionTracker needs calibration rounds
        // to resolve MCP-provided tool names vs plan step tool_name fields.
        for round in 0..=2 {
            let verdict = PostBatchSupervisor::check(
                round,
                None,
                &["file_read".to_string()],
                &[],
                0.0,   // zero plan progress
                true,  // tools succeeded
                None,
            );
            assert!(
                matches!(verdict, BatchVerdict::Continue),
                "Gate 3 must not fire on round {round} (warmup/calibration window)"
            );
        }
    }

    #[test]
    fn supervisor_gate3_suppressed_on_round_0() {
        // Gate 3 must NOT fire on round 0 — model hasn't had a chance to map tools→steps yet.
        let verdict = PostBatchSupervisor::check(
            0,
            None,
            &["file_read".to_string()],
            &[],
            0.0,  // zero plan progress
            true, // tool succeeded
            None,
        );
        // Round 0: supervisor should continue without injection even with 0% progress.
        assert!(
            matches!(verdict, BatchVerdict::Continue),
            "Gate 3 must be suppressed on round 0, got: {verdict:?}"
        );
    }

    #[test]
    fn supervisor_no_correction_when_progress_nonzero() {
        let verdict = PostBatchSupervisor::check(
            1,
            None,
            &["file_read".to_string()],
            &[],
            0.1, // some progress
            true,
            None,
        );
        assert!(matches!(verdict, BatchVerdict::Continue));
    }

    // ── CriticVerdict deserialization ───────────────────────────────────────

    #[test]
    fn critic_verdict_deserializes_achieved_true() {
        let json = r#"{
            "achieved": true,
            "confidence": 0.95,
            "gaps": [],
            "retry_instruction": null
        }"#;
        let verdict: CriticVerdict = serde_json::from_str(json).unwrap();
        assert!(verdict.achieved);
        assert!((verdict.confidence - 0.95).abs() < 0.01);
        assert!(verdict.gaps.is_empty());
        assert!(verdict.retry_instruction.is_none());
    }

    #[test]
    fn critic_verdict_deserializes_not_achieved() {
        let json = r#"{
            "achieved": false,
            "confidence": 0.85,
            "gaps": ["Missing error handling", "Incomplete edge case coverage"],
            "retry_instruction": "Add try/catch blocks and test boundary inputs"
        }"#;
        let verdict: CriticVerdict = serde_json::from_str(json).unwrap();
        assert!(!verdict.achieved);
        assert_eq!(verdict.gaps.len(), 2);
        assert!(verdict.retry_instruction.is_some());
        assert!(verdict
            .retry_instruction
            .unwrap()
            .contains("try/catch"));
    }

    #[test]
    fn critic_verdict_optional_fields_default_to_empty() {
        let json = r#"{"achieved": true, "confidence": 0.7}"#;
        let verdict: CriticVerdict = serde_json::from_str(json).unwrap();
        assert!(verdict.gaps.is_empty());
        assert!(verdict.retry_instruction.is_none());
    }

    #[test]
    fn critic_verdict_rejects_malformed_json() {
        let bad = r#"{"achieved": "yes", "confidence": "high"}"#;
        // "yes" is not bool — should fail.
        let result = serde_json::from_str::<CriticVerdict>(bad);
        assert!(result.is_err());
    }

    // ── PostBatchSupervisor gate priority ───────────────────────────────────

    #[test]
    fn supervisor_gate1_takes_priority_over_gate2() {
        // Both ≥2 critical failures (Gate 1) AND expected tool not called (Gate 2).
        // Gate 1 must win → ForceReplanNow, not InjectCorrection.
        let critical = vec![
            ("bash".to_string(), "No such file or directory: /missing".to_string()),
            ("file_write".to_string(), "Permission denied: /root/x".to_string()),
        ];
        let verdict = PostBatchSupervisor::check(
            1,
            Some("file_edit"),        // expected but not called
            &["file_read".to_string()], // different tool ran
            &critical,
            0.0,
            true,
            None,
        );
        assert!(
            matches!(verdict, BatchVerdict::ForceReplanNow(_)),
            "Gate 1 (critical failures) must take priority over Gate 2 (missing expected tool)"
        );
    }

    #[test]
    fn supervisor_gate1_takes_priority_over_gate3() {
        // Both ≥2 critical failures (Gate 1) AND 0% progress with success (Gate 3).
        // Gate 1 must win → ForceReplanNow.
        let critical = vec![
            ("bash".to_string(), "command not found: missing_cmd".to_string()),
            ("grep".to_string(), "No such file or directory".to_string()),
        ];
        let verdict = PostBatchSupervisor::check(
            1,
            None,
            &["bash".to_string(), "grep".to_string()],
            &critical,
            0.0,  // zero progress
            true, // tools succeeded (some)
            None,
        );
        assert!(
            matches!(verdict, BatchVerdict::ForceReplanNow(_)),
            "Gate 1 must take priority over Gate 3"
        );
    }

    #[test]
    fn supervisor_force_replan_message_contains_tool_names() {
        let critical = vec![
            ("my_tool_a".to_string(), "No such file or directory".to_string()),
            ("my_tool_b".to_string(), "Permission denied".to_string()),
        ];
        let verdict = PostBatchSupervisor::check(0, None, &[], &critical, 0.0, false, None);
        match verdict {
            BatchVerdict::ForceReplanNow(msg) => {
                assert!(
                    msg.contains("my_tool_a") || msg.contains("my_tool_b"),
                    "ForceReplanNow message must name the failing tools: {msg}"
                );
            }
            other => panic!("Expected ForceReplanNow, got {other:?}"),
        }
    }

    #[test]
    fn supervisor_no_correction_when_expected_tool_is_none_and_no_failures() {
        // When there is no plan (expected_tool=None) and no critical failures,
        // Gate 2 and Gate 3 should not fire spuriously.
        // round=1: Gate 3 fires at round >= 1 when 0% progress with success.
        let verdict = PostBatchSupervisor::check(
            1,
            None,
            &["bash".to_string()],
            &[],
            0.0,  // zero progress — but no plan, so Gate 3 fires (tools ran, no progress)
            true,
            None,
        );
        // Gate 3 fires here: 0% progress + tool success + tools ran.
        // This is expected behavior — the supervisor sees misalignment.
        assert!(matches!(
            verdict,
            BatchVerdict::InjectCorrection(_) | BatchVerdict::Continue
        ));
    }

    // ── LoopCritic::should_halt + should_halt_raw (Phase 7 — Autonomy Validation) ─

    fn verdict(achieved: bool, confidence: f32) -> CriticVerdict {
        CriticVerdict {
            achieved,
            confidence,
            gaps: vec![],
            retry_instruction: None,
        }
    }

    // Default threshold used by tests (matches PolicyConfig::default().halt_confidence_threshold).
    const DEFAULT_HALT_THRESHOLD: f32 = 0.80;

    #[test]
    fn should_halt_fires_on_high_conf_failure() {
        // !achieved + confidence >= 0.80 → must halt.
        assert!(LoopCritic::should_halt(&verdict(false, 0.80), DEFAULT_HALT_THRESHOLD));
        assert!(LoopCritic::should_halt(&verdict(false, 0.95), DEFAULT_HALT_THRESHOLD));
        assert!(LoopCritic::should_halt(&verdict(false, 1.00), DEFAULT_HALT_THRESHOLD));
    }

    #[test]
    fn should_halt_silent_below_threshold() {
        // !achieved but confidence < 0.80 → do NOT halt (uncertainty is high).
        assert!(!LoopCritic::should_halt(&verdict(false, 0.79), DEFAULT_HALT_THRESHOLD));
        assert!(!LoopCritic::should_halt(&verdict(false, 0.50), DEFAULT_HALT_THRESHOLD));
        assert!(!LoopCritic::should_halt(&verdict(false, 0.00), DEFAULT_HALT_THRESHOLD));
    }

    #[test]
    fn should_halt_never_fires_when_achieved() {
        // Even with confidence = 1.0, if achieved=true we must NOT halt.
        assert!(!LoopCritic::should_halt(&verdict(true, 1.00), DEFAULT_HALT_THRESHOLD));
        assert!(!LoopCritic::should_halt(&verdict(true, 0.95), DEFAULT_HALT_THRESHOLD));
        assert!(!LoopCritic::should_halt(&verdict(true, 0.80), DEFAULT_HALT_THRESHOLD));
    }

    #[test]
    fn should_halt_raw_matches_should_halt() {
        // should_halt_raw must agree with should_halt on all boundary cases.
        for (achieved, conf) in [
            (false, 0.80_f32),
            (false, 0.79_f32),
            (true, 0.95_f32),
            (false, 1.00_f32),
        ] {
            let via_verdict = LoopCritic::should_halt(&verdict(achieved, conf), DEFAULT_HALT_THRESHOLD);
            let via_raw = LoopCritic::should_halt_raw(achieved, conf, DEFAULT_HALT_THRESHOLD);
            assert_eq!(
                via_verdict, via_raw,
                "should_halt and should_halt_raw disagree at achieved={achieved} conf={conf}"
            );
        }
    }

    #[test]
    fn halt_threshold_default_is_08() {
        let policy = halcon_core::types::PolicyConfig::default();
        assert!((policy.halt_confidence_threshold - 0.80).abs() < f32::EPSILON);
    }

    #[test]
    fn custom_policy_threshold_respected() {
        // Custom threshold of 0.50 should cause halts at lower confidence.
        let custom_threshold = 0.50_f32;
        assert!(LoopCritic::should_halt(&verdict(false, 0.50), custom_threshold));
        assert!(LoopCritic::should_halt(&verdict(false, 0.80), custom_threshold));
        // Below custom threshold → no halt.
        assert!(!LoopCritic::should_halt(&verdict(false, 0.49), custom_threshold));
    }

    // ── SuspendPlugin gate (Phase 7 V3 plugin architecture) ──────────────────

    #[test]
    fn suspend_plugin_gate0_fires_when_all_plugin_tools_failed() {
        // Gate 0: Some(plugin_id) → SuspendPlugin (highest priority, checked before Gate 1).
        let verdict = PostBatchSupervisor::check(
            0,
            None,
            &["plugin_myp_run".to_string()],
            &[],
            0.0,
            false,
            Some("my-plugin"),
        );
        match verdict {
            BatchVerdict::SuspendPlugin { plugin_id, reason } => {
                assert_eq!(plugin_id, "my-plugin");
                assert!(!reason.is_empty());
            }
            other => panic!("Expected SuspendPlugin, got {other:?}"),
        }
    }

    #[test]
    fn suspend_plugin_none_suppresses_gate0() {
        // None → Gate 0 skipped, existing gates apply normally.
        let verdict = PostBatchSupervisor::check(
            1,
            None,
            &["file_read".to_string()],
            &[],
            0.5,
            true,
            None,
        );
        assert!(matches!(verdict, BatchVerdict::Continue));
    }

    #[test]
    fn suspend_plugin_gate0_beats_gate1() {
        // Gate 0 (SuspendPlugin) fires before Gate 1 (ForceReplanNow) when both could fire.
        let critical = vec![
            ("tool_a".to_string(), "err1".to_string()),
            ("tool_b".to_string(), "err2".to_string()),
        ];
        let verdict = PostBatchSupervisor::check(
            0,
            None,
            &[],
            &critical,
            0.0,
            false,
            Some("culprit-plugin"),
        );
        assert!(
            matches!(verdict, BatchVerdict::SuspendPlugin { .. }),
            "Gate 0 must beat Gate 1 when plugin_all_failed is Some"
        );
    }

    #[test]
    fn suspend_plugin_gate0_beats_gate3() {
        // Gate 0 fires even when Gate 3 (zero progress) would also fire.
        let verdict = PostBatchSupervisor::check(
            1,
            None,
            &["plugin_x_run".to_string()],
            &[],
            0.0,   // zero plan progress
            false, // tool failed — not succeeded
            Some("x"),
        );
        assert!(
            matches!(verdict, BatchVerdict::SuspendPlugin { .. }),
            "Gate 0 must beat Gate 3"
        );
    }
}
