//! SUBAGENT_CONTRACT_VALIDATOR — Phase K6
//!
//! Validates that sub-agent execution results fulfill their assigned contracts.
//!
//! A sub-agent is assigned a specific task (e.g., "read and analyze files").
//! The contract defines what the output MUST contain to be considered valid.
//!
//! ## Rejection Criteria
//!
//! A sub-agent result is REJECTED (and flagged for re-execution or escalation) if:
//!
//! 1. **Meta-question pattern**: output contains clarification requests when the plan
//!    step requires analysis (not dialogue).
//! 2. **No file reference**: for analysis steps that listed specific files to inspect,
//!    the output does not mention any of the expected files.
//! 3. **No synthesis when required**: step type is Synthesis but output contains no
//!    substantive text (too short, or only questions).
//! 4. **Low content ratio**: output is mostly filler text with no technical content.
//!
//! ## Integration
//!
//! Call `SubAgentContractValidator::validate()` in `orchestrator.rs` after each
//! sub-agent returns its output, before injecting into coordinator context.
//!
//! If validation fails:
//! - Log the rejection with reason
//! - Optionally re-run the sub-agent with a corrective prompt
//! - If re-run also fails: inject a "sub-agent failed" notice so coordinator
//!   can attempt the step directly or synthesize from partial results.

// ── Contract ──────────────────────────────────────────────────────────────────

/// Contract for a sub-agent execution.
///
/// Derived from the plan step that was delegated.
#[derive(Debug, Clone)]
pub struct SubAgentContract {
    /// Step description (used for pattern matching).
    pub step_description: String,
    /// Type of step — determines what the output must contain.
    pub step_type: StepType,
    /// Files or targets explicitly mentioned in the step instruction.
    /// If non-empty, at least one must appear in the output.
    pub expected_references: Vec<String>,
    /// Whether clarification is allowed (false for most analysis/synthesis steps).
    pub allow_clarification: bool,
    /// Minimum substantive content length (characters).
    pub min_content_chars: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepType {
    /// Read/inspect files and return findings.
    Analysis,
    /// Combine gathered findings into a coherent response.
    Synthesis,
    /// Execute shell command and return output.
    Execution,
    /// Search for files/content matching a pattern.
    Search,
    /// No specific requirement — accept any non-empty output.
    Generic,
}

impl SubAgentContract {
    /// Derive a contract from a plan step description and tool name.
    pub fn from_step(description: &str, tool_name: Option<&str>) -> Self {
        let step_type = Self::infer_step_type(description, tool_name);
        let allow_clarification = step_type == StepType::Generic;
        let min_content_chars = match step_type {
            StepType::Synthesis => 200,
            StepType::Analysis => 50,
            StepType::Execution => 1,
            StepType::Search => 1,
            StepType::Generic => 10,
        };

        Self {
            step_description: description.to_string(),
            step_type,
            expected_references: Vec::new(),
            allow_clarification,
            min_content_chars,
        }
    }

    pub fn with_expected_references(mut self, refs: Vec<String>) -> Self {
        self.expected_references = refs;
        self
    }

    fn infer_step_type(description: &str, tool_name: Option<&str>) -> StepType {
        let d = description.to_lowercase();
        if d.contains("synthesi")
            || d.contains("sintetiz")
            || d.contains("summarize")
            || d.contains("responde")
            || d.contains("respond")
        {
            return StepType::Synthesis;
        }
        if d.contains("analiz")
            || d.contains("analy")
            || d.contains("inspect")
            || d.contains("read")
            || d.contains("lee")
            || d.contains("revisa")
        {
            return StepType::Analysis;
        }
        if d.contains("search")
            || d.contains("busca")
            || d.contains("find")
            || d.contains("glob")
            || d.contains("grep")
        {
            return StepType::Search;
        }
        if d.contains("run") || d.contains("execute") || d.contains("bash") || d.contains("executa")
        {
            return StepType::Execution;
        }
        match tool_name {
            Some("glob") | Some("grep") | Some("fuzzy_find") => StepType::Search,
            Some("file_read") | Some("file_inspect") | Some("read_multiple_files") => {
                StepType::Analysis
            }
            Some("bash") | Some("shell") => StepType::Execution,
            _ => StepType::Generic,
        }
    }
}

// ── Validation Result ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationStatus {
    /// Output satisfies the contract.
    Valid,
    /// Output is rejected — reason provided.
    Rejected(RejectionReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectionReason {
    /// Output is a clarification question when analysis was required.
    MetaQuestion,
    /// Synthesis step produced no substantive content.
    InsufficientSynthesis,
    /// Analysis step produced no reference to expected files/targets.
    NoFileReference,
    /// Output is below the minimum content length.
    TooShort,
    /// Output contains only generic filler without technical content.
    NoSubstantiveContent,
}

impl RejectionReason {
    pub fn description(&self) -> &'static str {
        match self {
            Self::MetaQuestion => {
                "sub-agent sent a meta-question (clarification request) when analysis was required"
            }
            Self::InsufficientSynthesis => {
                "synthesis step produced no substantive content (too short or only questions)"
            }
            Self::NoFileReference => "analysis step did not reference any expected files",
            Self::TooShort => "sub-agent output below minimum content threshold",
            Self::NoSubstantiveContent => {
                "output contains no technical content — only generic text"
            }
        }
    }

    /// Returns true if re-running the sub-agent may recover.
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::MetaQuestion | Self::NoFileReference | Self::TooShort
        )
    }
}

#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub status: ValidationStatus,
    /// Signals to include in the coordinator's injected context.
    pub coordinator_notice: Option<String>,
}

impl ValidationResult {
    pub fn valid() -> Self {
        Self {
            status: ValidationStatus::Valid,
            coordinator_notice: None,
        }
    }

    pub fn rejected(reason: RejectionReason) -> Self {
        let notice = format!(
            "[SubAgent Contract Violation]: {}. The coordinator should attempt \
             this step directly or synthesize from available context.",
            reason.description()
        );
        Self {
            status: ValidationStatus::Rejected(reason),
            coordinator_notice: Some(notice),
        }
    }
}

// ── Validator ─────────────────────────────────────────────────────────────────

/// Validates sub-agent output against its assigned contract.
pub struct SubAgentContractValidator;

/// Patterns that indicate a meta-question / clarification request.
const META_QUESTION_PATTERNS: &[&str] = &[
    "¿qué",
    "¿cuál",
    "¿cuáles",
    "¿qué módulo",
    "¿qué archivo",
    "¿qué aspecto",
    "¿prefieres",
    "para darte",
    "necesito que me indiques",
    "necesito saber",
    "could you clarify",
    "could you specify",
    "what specific",
    "which module",
    "which file",
    "please specify",
    "please clarify",
    "what would you like",
    "what do you want",
];

/// Words that indicate substantive technical content.
const TECHNICAL_CONTENT_MARKERS: &[&str] = &[
    "fn ",
    "impl ",
    "struct ",
    "enum ",
    "trait ",
    "pub ",
    "use ",
    "cargo",
    "rust",
    "tokio",
    "async",
    "await",
    "mod ",
    "crate",
    ".rs",
    "Cargo.toml",
    "src/",
    "crates/",
    "error",
    "warning",
    "bug",
    "issue",
    "fix",
    "architecture",
    "design",
    "pattern",
    "module",
    "función",
    "módulo",
    "archivo",
    "estructura",
    "implementación",
];

impl SubAgentContractValidator {
    /// Validate sub-agent output against its contract.
    ///
    /// # Parameters
    /// - `output`: the text returned by the sub-agent
    /// - `contract`: the execution contract derived from the plan step
    pub fn validate(output: &str, contract: &SubAgentContract) -> ValidationResult {
        let trimmed = output.trim();
        let lower = trimmed.to_lowercase();

        // 1. Minimum content length check.
        if trimmed.len() < contract.min_content_chars {
            tracing::warn!(
                output_len = trimmed.len(),
                min_required = contract.min_content_chars,
                step_type = ?contract.step_type,
                "SubAgentContract: output too short"
            );
            return ValidationResult::rejected(RejectionReason::TooShort);
        }

        // 2. Meta-question detection (for non-clarification steps).
        if !contract.allow_clarification {
            let has_meta_question = META_QUESTION_PATTERNS.iter().any(|pat| lower.contains(pat));

            // Additional check: ends with question mark and is short (likely pure clarification).
            let is_short_question = trimmed.ends_with('?') && trimmed.len() < 500;

            if has_meta_question || is_short_question {
                // Check if there IS some substantive content alongside the question.
                // If the output is mostly questions (>50% of lines are questions), reject.
                let question_lines = trimmed.lines().filter(|l| l.trim().ends_with('?')).count();
                let total_lines = trimmed.lines().count().max(1);
                let question_ratio = question_lines as f64 / total_lines as f64;

                if question_ratio > 0.4 || (has_meta_question && trimmed.len() < 800) {
                    tracing::warn!(
                        question_ratio,
                        output_len = trimmed.len(),
                        step_type = ?contract.step_type,
                        "SubAgentContract: meta-question detected (clarification not allowed)"
                    );
                    return ValidationResult::rejected(RejectionReason::MetaQuestion);
                }
            }
        }

        // 3. Synthesis step: must have substantive content.
        if contract.step_type == StepType::Synthesis {
            let has_content = TECHNICAL_CONTENT_MARKERS
                .iter()
                .any(|marker| lower.contains(marker));
            let word_count = trimmed.split_whitespace().count();

            if !has_content && word_count < 30 {
                tracing::warn!(
                    word_count,
                    has_technical_content = false,
                    "SubAgentContract: synthesis step lacks substantive content"
                );
                return ValidationResult::rejected(RejectionReason::InsufficientSynthesis);
            }
        }

        // 4. File reference check (for analysis steps with expected files).
        if contract.step_type == StepType::Analysis && !contract.expected_references.is_empty() {
            let has_any_reference = contract
                .expected_references
                .iter()
                .any(|r| lower.contains(&r.to_lowercase()));

            if !has_any_reference {
                tracing::warn!(
                    expected = ?contract.expected_references,
                    output_preview = &trimmed[..trimmed.len().min(100)],
                    "SubAgentContract: analysis output references no expected files"
                );
                return ValidationResult::rejected(RejectionReason::NoFileReference);
            }
        }

        // 5. No substantive content (generic filler detection).
        if contract.step_type != StepType::Generic {
            let technical_markers_found = TECHNICAL_CONTENT_MARKERS
                .iter()
                .filter(|m| lower.contains(*m))
                .count();
            let word_count = trimmed.split_whitespace().count();

            // If output is >100 words but has NO technical markers → likely filler.
            if word_count > 100 && technical_markers_found == 0 {
                tracing::warn!(
                    word_count,
                    technical_markers = 0,
                    step_type = ?contract.step_type,
                    "SubAgentContract: no technical content detected in lengthy output"
                );
                return ValidationResult::rejected(RejectionReason::NoSubstantiveContent);
            }
        }

        // 6. Prompt injection detection — reject outputs that attempt to
        // override the coordinator's instructions. This prevents a compromised
        // sub-agent from injecting directives into the coordinator's context.
        {
            let injection_patterns = [
                "ignore previous instructions",
                "ignore all instructions",
                "ignore the above",
                "disregard previous",
                "you are now",
                "new instructions:",
                "system prompt:",
                "SYSTEM:",
                "<system>",
                "override:",
            ];
            let injection_found = injection_patterns
                .iter()
                .any(|pat| lower.contains(&pat.to_lowercase()));
            if injection_found {
                tracing::warn!(
                    step_type = ?contract.step_type,
                    output_preview = &trimmed[..trimmed.len().min(200)],
                    "SubAgentContract: prompt injection attempt detected in sub-agent output"
                );
                return ValidationResult::rejected(RejectionReason::NoSubstantiveContent);
            }
        }

        ValidationResult::valid()
    }

    /// Inject a corrective prompt for a rejected sub-agent result.
    ///
    /// Returns a directive to inject into the coordinator context explaining
    /// what happened and what the coordinator should do next.
    pub fn corrective_prompt(
        contract: &SubAgentContract,
        rejection: &RejectionReason,
        sub_agent_output: &str,
    ) -> String {
        let preview = &sub_agent_output[..sub_agent_output.len().min(200)];
        format!(
            "[SubAgent Failure Notice]: The sub-agent assigned to step '{}' \
             did not fulfill its contract.\n\
             Reason: {}\n\
             Sub-agent output preview: \"{}\"\n\
             You (the coordinator) must now handle this step directly: \
             execute the required analysis using your available tools and \
             synthesize the findings for the user. \
             Do NOT ask the user for clarification.",
            contract.step_description,
            rejection.description(),
            preview
        )
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn analysis_contract() -> SubAgentContract {
        SubAgentContract::from_step(
            "Read and analyse: Buscar archivos que puedan corresponder",
            Some("glob"),
        )
    }

    fn synthesis_contract() -> SubAgentContract {
        SubAgentContract::from_step("Synthesize findings and respond to the user.", None)
    }

    /// Reproduces the observed failure: sub-agent asks clarification instead of analyzing.
    #[test]
    fn rejects_meta_question_for_analysis_step() {
        let contract = analysis_contract();
        let output = "Te pido que analices tu implementación, pero necesito saber de qué parte \
                      específica del proyecto estás hablando. ¿Qué módulo o archivo quieres que \
                      revise? ¿Un archivo específico de Rust (.rs)? ¿Qué aspectos te interesan más?";

        let result = SubAgentContractValidator::validate(output, &contract);
        assert_eq!(
            result.status,
            ValidationStatus::Rejected(RejectionReason::MetaQuestion),
            "Sub-agent asking clarification must be rejected for analysis steps"
        );
        assert!(result.coordinator_notice.is_some());
    }

    #[test]
    fn accepts_substantive_analysis_output() {
        let contract = analysis_contract();
        let output = "Found the following Rust files in crates/halcon-cli/src/repl/:\n\
                      - mod.rs (2500 lines) — main REPL coordinator\n\
                      - agent/mod.rs (1800 lines) — agent loop\n\
                      - orchestrator.rs (400 lines) — sub-agent delegation\n\
                      The architecture uses async Rust with tokio. Key structs: LoopState, \
                      AgentLimits, ExecutionPlan. The REPL uses impl Planner for plan generation.";

        let result = SubAgentContractValidator::validate(output, &contract);
        assert_eq!(result.status, ValidationStatus::Valid);
    }

    #[test]
    fn rejects_synthesis_with_only_questions() {
        let contract = synthesis_contract();
        // Too short (< 200 chars) and no technical markers.
        let output = "Sure, I can help. What would you like to know?";

        let result = SubAgentContractValidator::validate(output, &contract);
        assert!(
            matches!(
                result.status,
                ValidationStatus::Rejected(RejectionReason::TooShort)
                    | ValidationStatus::Rejected(RejectionReason::InsufficientSynthesis)
            ),
            "Synthesis with only generic text must be rejected"
        );
    }

    #[test]
    fn accepts_synthesis_with_technical_content() {
        let contract = synthesis_contract();
        let output = "Based on my analysis of the crates/halcon-cli/src/repl/ module:\n\n\
                      **Architecture**: The system uses a REPL pattern with an agent loop \
                      (agent/mod.rs). The orchestrator delegates plan steps to sub-agents. \
                      Key components: LoopState (mutable per-round state), ConvergenceController \
                      (termination logic), and RoundScorer (evaluation).\n\n\
                      **Issues found**: \n\
                      1. IntentScorer word-count fallback misclassifies short queries\n\
                      2. TokenHeadroom uses incompatible metrics (pipeline_budget vs call_input_tokens)\n\
                      3. LoopCritic runs for all tasks including greetings\n\n\
                      **Recommendation**: Fix the three issues above to resolve the runtime failures.";

        let result = SubAgentContractValidator::validate(output, &contract);
        assert_eq!(result.status, ValidationStatus::Valid);
    }

    #[test]
    fn rejects_output_too_short() {
        let contract = analysis_contract();
        let output = "OK";
        let result = SubAgentContractValidator::validate(output, &contract);
        assert_eq!(
            result.status,
            ValidationStatus::Rejected(RejectionReason::TooShort)
        );
    }

    #[test]
    fn corrective_prompt_contains_step_description() {
        let contract = analysis_contract();
        let prompt = SubAgentContractValidator::corrective_prompt(
            &contract,
            &RejectionReason::MetaQuestion,
            "¿Qué archivo quieres revisar?",
        );
        assert!(prompt.contains("SubAgent Failure Notice"));
        assert!(prompt.contains("meta-question"));
        assert!(prompt.contains("handle this step directly"));
    }

    #[test]
    fn recoverable_rejections_identified() {
        assert!(RejectionReason::MetaQuestion.is_recoverable());
        assert!(RejectionReason::NoFileReference.is_recoverable());
        assert!(RejectionReason::TooShort.is_recoverable());
        assert!(!RejectionReason::NoSubstantiveContent.is_recoverable());
        assert!(!RejectionReason::InsufficientSynthesis.is_recoverable());
    }

    #[test]
    fn generic_step_allows_clarification() {
        let contract = SubAgentContract::from_step("generic helper task", None);
        // Generic steps allow clarification — short questions should pass.
        // (they pass the meta-question check because allow_clarification=true)
        assert!(contract.allow_clarification);
    }

    #[test]
    fn analysis_step_does_not_allow_clarification() {
        let contract =
            SubAgentContract::from_step("analiza los archivos del proyecto", Some("file_read"));
        assert!(!contract.allow_clarification);
        assert_eq!(contract.step_type, StepType::Analysis);
    }

    #[test]
    fn synthesis_step_inferred_correctly() {
        let contract =
            SubAgentContract::from_step("Synthesize findings and respond to the user.", None);
        assert_eq!(contract.step_type, StepType::Synthesis);
        assert_eq!(contract.min_content_chars, 200);
    }

    #[test]
    fn search_step_inferred_from_tool_name() {
        let contract = SubAgentContract::from_step("find relevant files", Some("glob"));
        assert_eq!(contract.step_type, StepType::Search);
    }
}
