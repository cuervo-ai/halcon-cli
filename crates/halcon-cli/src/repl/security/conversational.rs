//! Conversational Permission System — integration layer for executor.
//!
//! Phase I-6 of Questionnaire SOTA Audit (Feb 14, 2026)
//!
//! This module provides a conversational wrapper around the existing PermissionChecker,
//! enabling multi-turn dialogue for tool permissions while maintaining backwards compatibility.
//!
//! ## Architecture
//!
//! ```text
//! Executor
//!   ↓
//! ConversationalPermissionHandler
//!   ├─→ InputNormalizer (parse user input)
//!   ├─→ PermissionValidator (validate modifications)
//!   ├─→ ConversationState FSM (track dialogue)
//!   ├─→ AdaptivePromptBuilder (generate prompts)
//!   └─→ PermissionChecker (final authorization)
//! ```

use super::{
    adaptive_prompt::{AdaptivePromptBuilder, RiskLevel},
    conversation_protocol::{DetailAspect, PermissionMessage},
    conversation_state::ConversationState,
    input_normalizer::InputNormalizer,
    permissions::PermissionChecker,
    validation::{PermissionValidator, ValidationError},
};
use halcon_core::types::{ChatMessage, MessageContent, PermissionDecision, Role, ToolInput};
use serde_json::Value;
use std::time::Instant;

/// Result of a conversational permission request.
#[derive(Debug, Clone)]
pub enum ConversationalResult {
    /// User approved the operation (terminal state).
    Approved,
    /// User rejected the operation (terminal state).
    Denied,
    /// User asked a question — agent should respond.
    NeedsAgentResponse {
        question: String,
        /// Conversation state to resume after agent answers.
        resume_state: Box<ConversationState>,
    },
    /// User requested modification — validator checked, needs agent to replan.
    NeedsModification {
        clarification: String,
        validation_result: Result<(), ValidationError>,
        /// Conversation state to resume after modification.
        resume_state: Box<ConversationState>,
    },
    /// User deferred decision — operation paused.
    Deferred { reason: Option<String> },
}

/// Conversational permission handler.
///
/// Wraps the existing PermissionChecker with conversational capabilities.
pub struct ConversationalPermissionHandler {
    /// Underlying permission checker for final authorization.
    checker: PermissionChecker,
    /// Input normalizer for parsing user responses.
    normalizer: InputNormalizer,
    /// Validator for modifications and alternatives.
    validator: PermissionValidator,
}

impl ConversationalPermissionHandler {
    /// Create a new conversational permission handler.
    pub fn new(confirm: bool) -> Self {
        Self {
            checker: PermissionChecker::new(confirm),
            normalizer: InputNormalizer::new(),
            validator: PermissionValidator::new(),
        }
    }

    /// Create a conversational handler with TBAC support.
    pub fn with_tbac(confirm: bool, tbac_enabled: bool) -> Self {
        Self {
            checker: PermissionChecker::with_tbac(confirm, tbac_enabled),
            normalizer: InputNormalizer::new(),
            validator: PermissionValidator::new(),
        }
    }

    /// Create a conversational handler with full configuration.
    ///
    /// Matches `PermissionChecker::with_config()` API for backwards compatibility.
    pub fn with_config(
        confirm_destructive: bool,
        tbac_enabled: bool,
        auto_approve_in_ci: bool,
        prompt_timeout_secs: u64,
    ) -> Self {
        Self {
            checker: PermissionChecker::with_config(
                confirm_destructive,
                tbac_enabled,
                auto_approve_in_ci,
                prompt_timeout_secs,
            ),
            normalizer: InputNormalizer::new(),
            validator: PermissionValidator::new(),
        }
    }

    /// Set non-interactive mode (auto-approves all destructive operations).
    pub fn set_non_interactive(&mut self) {
        self.checker.set_non_interactive();
    }

    /// Authorize a tool operation with conversational dialogue loop.
    ///
    /// This is the main executor-facing API. It internally loops calling
    /// `authorize_conversational()` until a terminal state is reached.
    ///
    /// **Phase I-6B**: Multi-turn loop with placeholder responses.
    /// **Phase I-7**: Real agent answers and modification handling.
    ///
    /// # Returns
    ///
    /// - `PermissionDecision::Allowed`: User approved
    /// - `PermissionDecision::Denied`: User rejected or operation deferred
    pub async fn authorize(
        &mut self,
        tool: &str,
        level: halcon_core::types::PermissionLevel,
        input: &ToolInput,
    ) -> PermissionDecision {
        use std::io::Write;
        use tokio::io::AsyncBufReadExt as _;

        // G7 HARD VETO: blacklisted commands are denied unconditionally before any prompt.
        // The user cannot override this by pressing 'y' — it is a non-interactive hard block.
        if tool == "bash" {
            if let Some(cmd) = input.arguments.get("command").and_then(|v| v.as_str()) {
                let analysis = super::command_blacklist::analyze_command(cmd);
                if analysis.is_blacklisted {
                    let pattern_name = analysis
                        .matched_pattern
                        .as_ref()
                        .map(|p| p.name)
                        .unwrap_or("unknown");
                    let reason = analysis
                        .matched_pattern
                        .as_ref()
                        .map(|p| p.reason)
                        .unwrap_or("dangerous command");
                    eprintln!(
                        "\n🚫 HARD VETO: Command blocked by security policy.\n   Pattern: {}\n   Reason: {}",
                        pattern_name, reason
                    );
                    tracing::error!(
                        tool = tool,
                        command = cmd,
                        pattern = pattern_name,
                        reason = reason,
                        "G7 hard veto: blacklisted command blocked unconditionally"
                    );
                    return PermissionDecision::Denied;
                }
            }
        }

        let mut user_response: Option<String> = None;
        let mut iteration_count = 0;
        const MAX_ITERATIONS: usize = 10; // Prevent infinite loops

        loop {
            iteration_count += 1;
            if iteration_count > MAX_ITERATIONS {
                eprintln!("\n⚠️  Conversational loop exceeded max iterations. Denying operation.");
                return PermissionDecision::Denied;
            }

            let result = self
                .authorize_conversational(
                    tool,
                    level,
                    &input.arguments,
                    user_response.as_deref(),
                )
                .await;

            match result {
                ConversationalResult::Approved => return PermissionDecision::Allowed,
                ConversationalResult::Denied => return PermissionDecision::Denied,
                ConversationalResult::Deferred { .. } => {
                    // Deferred = operation cancelled for now
                    return PermissionDecision::Denied;
                }
                ConversationalResult::NeedsAgentResponse { question, .. } => {
                    // Phase I-6B: Show placeholder response
                    // Phase I-7: Will inject question into agent context for real answer
                    eprintln!("\n💬 User asked: {}", question);
                    eprintln!("🤖 [Placeholder response: I don't have enough context to answer that question right now.]");
                    eprintln!();

                    // Re-prompt for decision
                    eprint!("Tool '{}' requires permission. [y]es [n]o [?]details: ", tool);
                    std::io::stderr().flush().ok();

                    let mut buffer = String::new();
                    let mut reader = tokio::io::BufReader::new(tokio::io::stdin());
                    match reader.read_line(&mut buffer).await {
                        Ok(_) => {
                            user_response = Some(buffer.trim().to_string());
                        }
                        Err(e) => {
                            eprintln!("\n⚠️  Failed to read user input: {}", e);
                            return PermissionDecision::Denied;
                        }
                    }
                }
                ConversationalResult::NeedsModification {
                    clarification,
                    validation_result,
                    ..
                } => {
                    // Phase I-6B: Show placeholder response for modification requests
                    // Phase I-7: Will inject modification into agent loop for replanning
                    if let Err(e) = validation_result {
                        eprintln!("\n❌ Modification rejected: {}", e);
                    } else {
                        eprintln!("\n💬 User requested modification: {}", clarification);
                        eprintln!("🤖 [Placeholder: I cannot modify parameters in this mode. Please approve or deny the current operation.]");
                    }
                    eprintln!();

                    // Re-prompt for decision
                    eprint!("Tool '{}' requires permission. [y]es [n]o: ", tool);
                    std::io::stderr().flush().ok();

                    let mut buffer = String::new();
                    let mut reader = tokio::io::BufReader::new(tokio::io::stdin());
                    match reader.read_line(&mut buffer).await {
                        Ok(_) => {
                            user_response = Some(buffer.trim().to_string());
                        }
                        Err(e) => {
                            eprintln!("\n⚠️  Failed to read user input: {}", e);
                            return PermissionDecision::Denied;
                        }
                    }
                }
            }
        }
    }

    /// Authorize a tool operation using conversational dialogue.
    ///
    /// This is the main entry point for the conversational permission system.
    /// It handles multi-turn dialogue by returning `ConversationalResult` variants
    /// that indicate whether the agent needs to respond or replan.
    ///
    /// # Returns
    ///
    /// - `Approved`: User approved, proceed with execution
    /// - `Denied`: User rejected, abort operation
    /// - `NeedsAgentResponse`: User asked question, agent should answer
    /// - `NeedsModification`: User requested change, agent should replan
    /// - `Deferred`: User paused decision, operation on hold
    pub async fn authorize_conversational(
        &mut self,
        tool: &str,
        level: halcon_core::types::PermissionLevel,
        input: &Value,
        user_response: Option<&str>,
    ) -> ConversationalResult {
        // If no user response yet, this is the initial request.
        // Use the existing PermissionChecker to determine if we need to prompt.
        if user_response.is_none() {
            let tool_input = ToolInput {
                tool_use_id: format!("perm_{}", tool),
                arguments: input.clone(),
                working_directory: ".".to_string(),
            };
            let decision = self.checker.authorize(tool, level, &tool_input).await;
            match decision {
                PermissionDecision::Allowed
                | PermissionDecision::AllowedAlways
                | PermissionDecision::AllowedForDirectory
                | PermissionDecision::AllowedForRepository
                | PermissionDecision::AllowedForPattern
                | PermissionDecision::AllowedThisSession => {
                    return ConversationalResult::Approved
                }
                PermissionDecision::Denied
                | PermissionDecision::DeniedForDirectory
                | PermissionDecision::DeniedForPattern => return ConversationalResult::Denied,
            }
        }

        // User provided a response — normalize and handle.
        if let Some(response) = user_response {
            let message = self.normalizer.normalize(response);

            // Validate the message.
            if let Err(e) = self.validator.validate(&message, tool, input) {
                // Validation failed — return error embedded in NeedsAgentResponse.
                return ConversationalResult::NeedsAgentResponse {
                    question: format!("Validation error: {}", e),
                    resume_state: Box::new(ConversationState::Prompting {
                        tool: tool.to_string(),
                        args: input.clone(),
                        started_at: Instant::now(),
                    }),
                };
            }

            // Handle validated message.
            match message {
                PermissionMessage::Approve => ConversationalResult::Approved,
                PermissionMessage::Reject => ConversationalResult::Denied,
                PermissionMessage::AskQuestion { question } => {
                    ConversationalResult::NeedsAgentResponse {
                        question: question.clone(),
                        resume_state: Box::new(ConversationState::RespondingToQuestion {
                            tool: tool.to_string(),
                            args: input.clone(),
                            question,
                        }),
                    }
                }
                PermissionMessage::ModifyParameters { clarification } => {
                    ConversationalResult::NeedsModification {
                        clarification: clarification.clone(),
                        validation_result: Ok(()),
                        resume_state: Box::new(ConversationState::ValidatingModification {
                            tool: tool.to_string(),
                            original_args: input.clone(),
                            requested_change: clarification,
                        }),
                    }
                }
                PermissionMessage::Defer { reason } => ConversationalResult::Deferred { reason },
                PermissionMessage::RequestDetails { aspect } => {
                    // Generate detail view and return as agent response.
                    let details = AdaptivePromptBuilder::build_detail_view(tool, input, aspect.clone());
                    let detail_text = format!("{}\n\n{}", details.title, details.summary);
                    ConversationalResult::NeedsAgentResponse {
                        question: detail_text,
                        resume_state: Box::new(ConversationState::ShowingDetails {
                            tool: tool.to_string(),
                            args: input.clone(),
                            aspect,
                        }),
                    }
                }
                PermissionMessage::SuggestAlternative { suggestion } => {
                    ConversationalResult::NeedsModification {
                        clarification: suggestion.clone(),
                        validation_result: Ok(()),
                        resume_state: Box::new(ConversationState::ValidatingModification {
                            tool: tool.to_string(),
                            original_args: input.clone(),
                            requested_change: suggestion,
                        }),
                    }
                }
                PermissionMessage::ConditionalApprove { condition } => {
                    // Treat conditional approval as modification request.
                    ConversationalResult::NeedsModification {
                        clarification: condition.clone(),
                        validation_result: Ok(()),
                        resume_state: Box::new(ConversationState::ValidatingModification {
                            tool: tool.to_string(),
                            original_args: input.clone(),
                            requested_change: condition,
                        }),
                    }
                }
                PermissionMessage::RequestPreview => {
                    // Preview requested — return detail view.
                    let details =
                        AdaptivePromptBuilder::build_detail_view(tool, input, DetailAspect::WhatItDoes);
                    ConversationalResult::NeedsAgentResponse {
                        question: details.summary,
                        resume_state: Box::new(ConversationState::Prompting {
                            tool: tool.to_string(),
                            args: input.clone(),
                            started_at: Instant::now(),
                        }),
                    }
                }
                PermissionMessage::BatchApprove { .. } => {
                    // Batch approval — treat as regular approval for now.
                    // TODO: Phase I-7 — implement batch approval state tracking.
                    ConversationalResult::Approved
                }
            }
        } else {
            // No response provided — shouldn't happen, return denied.
            ConversationalResult::Denied
        }
    }

    /// Convert a conversational result to agent feedback (ChatMessage).
    ///
    /// This creates a system message that can be injected into the agent's context
    /// to inform it about the user's decision or question.
    pub fn to_agent_feedback(
        &self,
        result: &ConversationalResult,
        tool: &str,
        input: &Value,
    ) -> Option<ChatMessage> {
        match result {
            ConversationalResult::Approved => Some(ChatMessage {
                role: Role::User,
                content: MessageContent::Text(format!(
                    "[User approved {} with args: {}]",
                    tool, input
                )),
            }),
            ConversationalResult::Denied => Some(ChatMessage {
                role: Role::User,
                content: MessageContent::Text(format!(
                    "[User rejected {} — do not retry this operation]",
                    tool
                )),
            }),
            ConversationalResult::NeedsAgentResponse { question, .. } => Some(ChatMessage {
                role: Role::User,
                content: MessageContent::Text(format!(
                    "[User asked about {}: \"{}\"]",
                    tool, question
                )),
            }),
            ConversationalResult::NeedsModification { clarification, .. } => Some(ChatMessage {
                role: Role::User,
                content: MessageContent::Text(format!(
                    "[User requested modification to {}: \"{}\"]",
                    tool, clarification
                )),
            }),
            ConversationalResult::Deferred { reason } => {
                let reason_text = reason.as_deref().unwrap_or("no reason given");
                Some(ChatMessage {
                    role: Role::User,
                    content: MessageContent::Text(format!(
                        "[User deferred decision on {} — reason: {}]",
                        tool, reason_text
                    )),
                })
            }
        }
    }

    /// Assess risk level for a tool operation.
    pub fn assess_risk_level(
        &self,
        tool: &str,
        level: halcon_core::types::PermissionLevel,
        input: &Value,
    ) -> RiskLevel {
        // Phase 7: Check command blacklist FIRST (highest priority).
        // Blacklisted commands are ALWAYS Critical risk, regardless of tool type.
        if tool == "bash" {
            if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
                let analysis = super::command_blacklist::analyze_command(cmd);
                if analysis.is_blacklisted {
                    tracing::warn!(
                        tool = tool,
                        command = cmd,
                        pattern = analysis.matched_pattern.as_ref().map(|p| p.name),
                        reason = analysis.matched_pattern.as_ref().map(|p| p.reason),
                        "Blacklisted command detected — escalated to Critical risk"
                    );
                    return RiskLevel::Critical;
                }
            }
        }

        // Destructive tools are always high risk.
        if level == halcon_core::types::PermissionLevel::Destructive {
            return RiskLevel::High;
        }

        // ReadWrite tools are medium risk.
        if level == halcon_core::types::PermissionLevel::ReadWrite {
            return RiskLevel::Medium;
        }

        // For bash, check command content.
        if tool == "bash" {
            if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
                let lower = cmd.to_lowercase();
                if lower.contains("rm") || lower.contains("delete") || lower.contains("sudo") {
                    return RiskLevel::High;
                }
                if lower.contains("mv") || lower.contains("cp") || lower.contains("chmod") {
                    return RiskLevel::Medium;
                }
            }
        }

        // Default: low risk.
        RiskLevel::Low
    }

    /// Get access to the underlying PermissionChecker (for backwards compatibility).
    pub fn checker(&self) -> &PermissionChecker {
        &self.checker
    }

    /// Get mutable access to the underlying PermissionChecker.
    pub fn checker_mut(&mut self) -> &mut PermissionChecker {
        &mut self.checker
    }

    // Delegation methods for TBAC (Task-Based Access Control)

    /// Push a task context onto the TBAC stack.
    pub fn push_context(&mut self, ctx: halcon_core::types::TaskContext) {
        self.checker.push_context(ctx);
    }

    /// Pop the current task context from the TBAC stack.
    pub fn pop_context(&mut self) {
        self.checker.pop_context();
    }

    /// Get the currently active task context (if any).
    pub fn active_context(&self) -> Option<&halcon_core::types::TaskContext> {
        self.checker.active_context()
    }

    /// Check TBAC authorization for a tool call.
    pub fn check_tbac(
        &mut self,
        tool: &str,
        arguments: &serde_json::Value,
    ) -> halcon_core::types::AuthzDecision {
        self.checker.check_tbac(tool, arguments)
    }

    /// Set the TUI channel for permission prompts (Phase 43).
    #[cfg(feature = "tui")]
    pub fn set_tui_channel(
        &mut self,
        rx: tokio::sync::mpsc::UnboundedReceiver<halcon_core::types::PermissionDecision>,
    ) {
        self.checker.set_tui_channel(rx);
    }

    /// Set the sudo password channel (TUI → executor elevation flow).
    #[cfg(feature = "tui")]
    pub fn set_sudo_channel(
        &mut self,
        rx: tokio::sync::mpsc::UnboundedReceiver<Option<String>>,
    ) {
        self.checker.set_sudo_channel(rx);
    }

    /// Set the TUI notification channel for permission timeout events.
    ///
    /// When set, `authorize()` sends a `UiEvent::Warning` to the TUI activity panel
    /// whenever the permission timeout fires. Wire this with the same `ui_tx` used
    /// by `TuiSink` so the user sees a visible explanation for denied tools.
    #[cfg(feature = "tui")]
    pub fn set_notification_tx(
        &mut self,
        tx: tokio::sync::mpsc::UnboundedSender<crate::tui::events::UiEvent>,
    ) {
        self.checker.set_notification_tx(tx);
    }

    /// Await the sudo password from the TUI modal.
    ///
    /// Blocks for up to `timeout_secs` seconds. Returns `None` on cancel/timeout.
    /// Call this AFTER `authorize()` returns `Allowed` for a sudo bash command.
    #[cfg(feature = "tui")]
    pub async fn get_sudo_password(&self, timeout_secs: u64) -> Option<String> {
        self.checker.get_sudo_password(timeout_secs).await
    }

    /// Returns true if the TUI has a cached sudo password available.
    #[cfg(feature = "tui")]
    pub fn has_cached_sudo_password(&self) -> bool {
        self.checker.has_cached_sudo_password()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn authorize_conversational_no_response_auto_approve() {
        let mut handler = ConversationalPermissionHandler::new(false); // Auto-approve
        let result = handler
            .authorize_conversational(
                "file_read",
                halcon_core::types::PermissionLevel::ReadOnly,
                &json!({"path": "/tmp/test.txt"}),
                None,
            )
            .await;
        assert!(matches!(result, ConversationalResult::Approved));
    }

    #[tokio::test]
    async fn authorize_conversational_approve_response() {
        let mut handler = ConversationalPermissionHandler::new(true);
        let result = handler
            .authorize_conversational(
                "bash",
                halcon_core::types::PermissionLevel::Destructive,
                &json!({"command": "ls"}),
                Some("yes"),
            )
            .await;
        assert!(matches!(result, ConversationalResult::Approved));
    }

    #[tokio::test]
    async fn authorize_conversational_reject_response() {
        let mut handler = ConversationalPermissionHandler::new(true);
        let result = handler
            .authorize_conversational(
                "file_delete",
                halcon_core::types::PermissionLevel::Destructive,
                &json!({"path": "/tmp/test.txt"}),
                Some("no"),
            )
            .await;
        assert!(matches!(result, ConversationalResult::Denied));
    }

    #[tokio::test]
    async fn authorize_conversational_question_response() {
        let mut handler = ConversationalPermissionHandler::new(true);
        let result = handler
            .authorize_conversational(
                "bash",
                halcon_core::types::PermissionLevel::Destructive,
                &json!({"command": "rm -rf /tmp/*.txt"}),
                Some("what files will be deleted?"),
            )
            .await;
        if let ConversationalResult::NeedsAgentResponse { question, .. } = result {
            assert!(question.contains("what files"));
        } else {
            panic!("Expected NeedsAgentResponse");
        }
    }

    #[tokio::test]
    async fn authorize_conversational_modification_response() {
        let mut handler = ConversationalPermissionHandler::new(true);
        let result = handler
            .authorize_conversational(
                "file_write",
                halcon_core::types::PermissionLevel::Destructive,
                &json!({"path": "/tmp/test.txt", "content": "test"}),
                Some("use /tmp/output.txt as the path"),
            )
            .await;
        if let ConversationalResult::NeedsModification { clarification, .. } = result {
            assert!(clarification.contains("/tmp/output.txt"));
        } else {
            panic!("Expected NeedsModification");
        }
    }

    #[tokio::test]
    async fn authorize_conversational_defer_response() {
        let mut handler = ConversationalPermissionHandler::new(true);
        let result = handler
            .authorize_conversational(
                "file_delete",
                halcon_core::types::PermissionLevel::Destructive,
                &json!({"path": "/tmp/test.txt"}),
                Some("wait, let me check first"),
            )
            .await;
        if let ConversationalResult::Deferred { reason } = result {
            assert!(reason.is_some());
        } else {
            panic!("Expected Deferred");
        }
    }

    #[tokio::test]
    async fn authorize_conversational_detail_request() {
        let mut handler = ConversationalPermissionHandler::new(true);
        let result = handler
            .authorize_conversational(
                "bash",
                halcon_core::types::PermissionLevel::Destructive,
                &json!({"command": "rm -rf /tmp/*.txt"}),
                Some("show risk assessment"),
            )
            .await;
        if let ConversationalResult::NeedsAgentResponse { question, .. } = result {
            // Should contain risk assessment details.
            assert!(!question.is_empty());
        } else {
            panic!("Expected NeedsAgentResponse for detail request");
        }
    }

    #[tokio::test]
    async fn authorize_conversational_invalid_modification_blocked() {
        let mut handler = ConversationalPermissionHandler::new(true);
        let result = handler
            .authorize_conversational(
                "bash",
                halcon_core::types::PermissionLevel::Destructive,
                &json!({"command": "ls"}),
                Some("modify please"), // "modify" keyword but no clear parameter mention
            )
            .await;
        // "modify" triggers ModifyParameters classification, but validator rejects
        // because no parameter is mentioned (path, file, command, etc.).
        if let ConversationalResult::NeedsAgentResponse { question, .. } = result {
            assert!(question.contains("Validation error"));
            assert!(question.contains("unclear") || question.contains("specify parameter"));
        } else {
            panic!("Expected NeedsAgentResponse with validation error, got: {:?}", result);
        }
    }

    #[tokio::test]
    async fn authorize_conversational_unsafe_modification_blocked() {
        let mut handler = ConversationalPermissionHandler::new(true);
        let result = handler
            .authorize_conversational(
                "bash",
                halcon_core::types::PermissionLevel::Destructive,
                &json!({"command": "ls"}),
                Some("use rm -rf / instead"), // Unsafe
            )
            .await;
        if let ConversationalResult::NeedsAgentResponse { question, .. } = result {
            assert!(question.contains("Validation error"));
            assert!(question.contains("unsafe") || question.contains("Unsafe"));
        } else {
            panic!("Expected NeedsAgentResponse with unsafe validation error");
        }
    }

    #[test]
    fn to_agent_feedback_approve() {
        let handler = ConversationalPermissionHandler::new(true);
        let feedback = handler.to_agent_feedback(
            &ConversationalResult::Approved,
            "bash",
            &json!({"command": "ls"}),
        );
        assert!(feedback.is_some());
        if let Some(msg) = feedback {
            assert_eq!(msg.role, Role::User);
            if let MessageContent::Text(text) = msg.content {
                assert!(text.contains("approved"));
            }
        }
    }

    #[test]
    fn to_agent_feedback_denied() {
        let handler = ConversationalPermissionHandler::new(true);
        let feedback = handler.to_agent_feedback(
            &ConversationalResult::Denied,
            "file_delete",
            &json!({"path": "/tmp/test.txt"}),
        );
        assert!(feedback.is_some());
        if let Some(msg) = feedback {
            if let MessageContent::Text(text) = msg.content {
                assert!(text.contains("rejected"));
                assert!(text.contains("do not retry"));
            }
        }
    }

    #[test]
    fn to_agent_feedback_question() {
        let handler = ConversationalPermissionHandler::new(true);
        let feedback = handler.to_agent_feedback(
            &ConversationalResult::NeedsAgentResponse {
                question: "what files will be deleted?".to_string(),
                resume_state: Box::new(ConversationState::Idle),
            },
            "bash",
            &json!({"command": "rm -rf /tmp/*.txt"}),
        );
        assert!(feedback.is_some());
        if let Some(msg) = feedback {
            if let MessageContent::Text(text) = msg.content {
                assert!(text.contains("asked"));
                assert!(text.contains("what files"));
            }
        }
    }

    #[test]
    fn assess_risk_level_destructive_high() {
        let handler = ConversationalPermissionHandler::new(true);
        let risk = handler.assess_risk_level(
            "file_delete",
            halcon_core::types::PermissionLevel::Destructive,
            &json!({"path": "/tmp/test.txt"}),
        );
        assert_eq!(risk, RiskLevel::High);
    }

    #[test]
    fn assess_risk_level_readwrite_medium() {
        let handler = ConversationalPermissionHandler::new(true);
        let risk = handler.assess_risk_level(
            "file_write",
            halcon_core::types::PermissionLevel::ReadWrite,
            &json!({"path": "/tmp/test.txt"}),
        );
        assert_eq!(risk, RiskLevel::Medium);
    }

    #[test]
    fn assess_risk_level_readonly_low() {
        let handler = ConversationalPermissionHandler::new(true);
        let risk = handler.assess_risk_level(
            "file_read",
            halcon_core::types::PermissionLevel::ReadOnly,
            &json!({"path": "/tmp/test.txt"}),
        );
        assert_eq!(risk, RiskLevel::Low);
    }

    #[test]
    fn assess_risk_level_bash_rm_high() {
        let handler = ConversationalPermissionHandler::new(true);
        let risk = handler.assess_risk_level(
            "bash",
            halcon_core::types::PermissionLevel::Destructive,
            &json!({"command": "rm -rf /tmp/*.txt"}),
        );
        assert_eq!(risk, RiskLevel::High);
    }

    #[test]
    fn assess_risk_level_bash_ls_low() {
        let handler = ConversationalPermissionHandler::new(true);
        let risk = handler.assess_risk_level(
            "bash",
            halcon_core::types::PermissionLevel::Destructive,
            &json!({"command": "ls -la"}),
        );
        // bash is Destructive level, so always high even for safe commands.
        assert_eq!(risk, RiskLevel::High);
    }

    // --- Phase 7: Command blacklist integration tests ---

    #[test]
    fn blacklist_rm_rf_root_escalates_to_critical() {
        let handler = ConversationalPermissionHandler::new(true);
        let risk = handler.assess_risk_level(
            "bash",
            halcon_core::types::PermissionLevel::Destructive,
            &json!({"command": "rm -rf /"}),
        );
        assert_eq!(risk, RiskLevel::Critical);
    }

    #[test]
    fn blacklist_dd_disk_wipe_escalates_to_critical() {
        let handler = ConversationalPermissionHandler::new(true);
        let risk = handler.assess_risk_level(
            "bash",
            halcon_core::types::PermissionLevel::Destructive,
            &json!({"command": "dd if=/dev/zero of=/dev/sda"}),
        );
        assert_eq!(risk, RiskLevel::Critical);
    }

    #[test]
    fn blacklist_fork_bomb_escalates_to_critical() {
        let handler = ConversationalPermissionHandler::new(true);
        let risk = handler.assess_risk_level(
            "bash",
            halcon_core::types::PermissionLevel::Destructive,
            &json!({"command": ":(){ :|:& };:"}),
        );
        assert_eq!(risk, RiskLevel::Critical);
    }

    #[test]
    fn blacklist_chmod_777_root_escalates_to_critical() {
        let handler = ConversationalPermissionHandler::new(true);
        let risk = handler.assess_risk_level(
            "bash",
            halcon_core::types::PermissionLevel::Destructive,
            &json!({"command": "chmod -R 777 /"}),
        );
        assert_eq!(risk, RiskLevel::Critical);
    }

    #[test]
    fn blacklist_mkfs_escalates_to_critical() {
        let handler = ConversationalPermissionHandler::new(true);
        let risk = handler.assess_risk_level(
            "bash",
            halcon_core::types::PermissionLevel::Destructive,
            &json!({"command": "mkfs.ext4 /dev/sdb"}),
        );
        assert_eq!(risk, RiskLevel::Critical);
    }

    #[test]
    fn blacklist_safe_rm_not_escalated() {
        let handler = ConversationalPermissionHandler::new(true);
        let risk = handler.assess_risk_level(
            "bash",
            halcon_core::types::PermissionLevel::Destructive,
            &json!({"command": "rm -rf /tmp/test"}),
        );
        // Safe rm is still High (Destructive level), not Critical
        assert_eq!(risk, RiskLevel::High);
    }

    // ── G7 hard veto tests ──────────────────────────────────────────────────

    fn make_tool_input(args: serde_json::Value) -> ToolInput {
        ToolInput {
            tool_use_id: "test-id".into(),
            arguments: args,
            working_directory: "/tmp".into(),
        }
    }

    #[tokio::test]
    async fn g7_hard_veto_rm_rf_root_denied_without_prompt() {
        // Even with interactive=true, blacklisted command is denied immediately.
        let mut handler = ConversationalPermissionHandler::new(true);
        let input = make_tool_input(json!({"command": "rm -rf /"}));
        let result = handler
            .authorize("bash", halcon_core::types::PermissionLevel::Destructive, &input)
            .await;
        assert_eq!(result, PermissionDecision::Denied);
    }

    #[tokio::test]
    async fn g7_hard_veto_fork_bomb_denied() {
        let mut handler = ConversationalPermissionHandler::new(false); // non-interactive
        let input = make_tool_input(json!({"command": ":(){ :|:& };:"}));
        let result = handler
            .authorize("bash", halcon_core::types::PermissionLevel::Destructive, &input)
            .await;
        assert_eq!(result, PermissionDecision::Denied);
    }

    #[tokio::test]
    async fn g7_hard_veto_mkfs_denied() {
        let mut handler = ConversationalPermissionHandler::new(true);
        let input = make_tool_input(json!({"command": "mkfs.ext4 /dev/sda1"}));
        let result = handler
            .authorize("bash", halcon_core::types::PermissionLevel::Destructive, &input)
            .await;
        assert_eq!(result, PermissionDecision::Denied);
    }

    #[tokio::test]
    async fn g7_safe_command_not_vetoed() {
        // Safe bash commands bypass the hard veto path entirely.
        let mut handler = ConversationalPermissionHandler::new(false); // auto-approve
        let input = make_tool_input(json!({"command": "echo hello"}));
        let result = handler
            .authorize("bash", halcon_core::types::PermissionLevel::ReadOnly, &input)
            .await;
        // Non-interactive auto-approves safe commands
        assert_eq!(result, PermissionDecision::Allowed);
    }

    #[tokio::test]
    async fn g7_non_bash_tool_not_vetoed() {
        // Hard veto only applies to the "bash" tool.
        let mut handler = ConversationalPermissionHandler::new(false);
        let input = make_tool_input(json!({"path": "/etc/passwd"}));
        let result = handler
            .authorize("file_read", halcon_core::types::PermissionLevel::ReadOnly, &input)
            .await;
        assert_eq!(result, PermissionDecision::Allowed);
    }

    #[tokio::test]
    async fn g7_dd_disk_wipe_denied() {
        let mut handler = ConversationalPermissionHandler::new(true);
        let input = make_tool_input(json!({"command": "dd if=/dev/zero of=/dev/sda"}));
        let result = handler
            .authorize("bash", halcon_core::types::PermissionLevel::Destructive, &input)
            .await;
        assert_eq!(result, PermissionDecision::Denied);
    }
}
