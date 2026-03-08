//! Adaptive prompt builder — context-aware permission prompts with risk assessment.
//!
//! Phase I-3 of Questionnaire SOTA Audit (Feb 14, 2026)
//!
//! This module generates permission prompts that adapt to:
//! - Tool risk level (Low/Medium/High)
//! - Conversation state (initial, clarification, detail view)
//! - Progressive disclosure (summary → parameters → risk → history)

use super::conversation_protocol::DetailAspect;

/// Build prompts that adapt to conversation state and tool risk.
pub struct AdaptivePromptBuilder;

impl AdaptivePromptBuilder {
    /// Build initial permission prompt with progressive disclosure.
    pub fn build_initial_prompt(
        tool: &str,
        args: &serde_json::Value,
        risk_level: RiskLevel,
    ) -> PromptContent {
        let summary = Self::summarize_tool(tool, args);
        let risk_indicator = Self::risk_indicator(risk_level);

        let quick_options = vec![
            "[Y] Approve".into(),
            "[N] Reject".into(),
            "[?] Details".into(),
            "[M] Modify".into(),
        ];

        let verbose_hint = match risk_level {
            RiskLevel::Low => "Type 'y' to approve or ask questions naturally".into(),
            RiskLevel::Medium => "⚠️  Consider reviewing parameters with '?' before approving".into(),
            RiskLevel::High => "🔴 HIGH RISK — Review details carefully before approving".into(),
            RiskLevel::Critical => "🚨 CRITICAL — Dangerous operation detected! Type 'no' to reject".into(),
        };

        PromptContent {
            title: format!("{} {} wants approval", risk_indicator, tool),
            summary,
            quick_options,
            verbose_hint: Some(verbose_hint),
            detail_view: None,
        }
    }

    /// Build clarification response prompt (after user asks question).
    pub fn build_clarification_response(question: &str, answer: &str) -> PromptContent {
        PromptContent {
            title: "Clarification".into(),
            summary: format!("Q: {}\nA: {}", question, answer),
            quick_options: vec!["[Y] Approve now".into(), "[N] Still reject".into()],
            verbose_hint: Some("Ask more questions or approve/reject".into()),
            detail_view: None,
        }
    }

    /// Build detail view prompt (progressive disclosure).
    pub fn build_detail_view(
        tool: &str,
        args: &serde_json::Value,
        aspect: DetailAspect,
    ) -> PromptContent {
        let detail_view = match aspect {
            DetailAspect::Parameters => Self::render_parameters(args),
            DetailAspect::WhatItDoes => Self::explain_tool_action(tool, args),
            DetailAspect::RiskAssessment => Self::assess_risk(tool, args),
            DetailAspect::History => Self::show_history(tool),
        };

        PromptContent {
            title: format!("{} — {}", tool, aspect.label()),
            summary: String::new(),
            quick_options: vec!["[Y] Approve".into(), "[N] Reject".into(), "[Back]".into()],
            verbose_hint: None,
            detail_view: Some(detail_view),
        }
    }

    /// Summarize tool call for initial prompt.
    fn summarize_tool(tool: &str, args: &serde_json::Value) -> String {
        match tool {
            "bash" => {
                let cmd = args["command"].as_str().unwrap_or("(unknown)");
                if cmd.len() > 60 {
                    format!("Command: {}...", &cmd[..{ let mut _fcb = (57).min(cmd.len()); while _fcb > 0 && !cmd.is_char_boundary(_fcb) { _fcb -= 1; } _fcb }])
                } else {
                    format!("Command: {}", cmd)
                }
            }
            "file_write" | "file_edit" | "file_delete" | "file_read" => {
                format!("File: {}", args["path"].as_str().unwrap_or("(unknown)"))
            }
            "grep" => format!(
                "Pattern: {} in {}",
                args["pattern"].as_str().unwrap_or("(unknown)"),
                args["path"].as_str().unwrap_or("(unknown)")
            ),
            "glob" => format!(
                "Pattern: {}",
                args["pattern"].as_str().unwrap_or("(unknown)")
            ),
            _ => {
                let json = serde_json::to_string(args).unwrap_or_default();
                if json.len() > 60 {
                    format!("{}...", &json[..{ let mut _fcb = (57).min(json.len()); while _fcb > 0 && !json.is_char_boundary(_fcb) { _fcb -= 1; } _fcb }])
                } else {
                    json
                }
            }
        }
    }

    /// Get risk indicator emoji.
    fn risk_indicator(level: RiskLevel) -> &'static str {
        match level {
            RiskLevel::Low => "✓",
            RiskLevel::Medium => "⚠️",
            RiskLevel::High => "🔴",
            RiskLevel::Critical => "🚨",
        }
    }

    /// Render all parameters in pretty format.
    fn render_parameters(args: &serde_json::Value) -> String {
        if let Some(obj) = args.as_object() {
            let mut lines = vec!["Parameters:".to_string(), String::new()];
            for (key, value) in obj {
                let value_str = match value {
                    serde_json::Value::String(s) => s.clone(),
                    _ => serde_json::to_string_pretty(value).unwrap_or_default(),
                };
                lines.push(format!("  {}: {}", key, value_str));
            }
            lines.join("\n")
        } else {
            format!("Parameters: {}", serde_json::to_string_pretty(args).unwrap_or_default())
        }
    }

    /// Explain what the tool will do (human-readable).
    fn explain_tool_action(tool: &str, args: &serde_json::Value) -> String {
        match tool {
            "bash" => {
                let cmd = args["command"].as_str().unwrap_or("(unknown)");
                format!(
                    "What it does:\n\n  Execute shell command: {}\n\n  \
                     This will run in your current shell environment with full system access.",
                    cmd
                )
            }
            "file_write" => {
                let path = args["path"].as_str().unwrap_or("(unknown)");
                format!(
                    "What it does:\n\n  Write content to file: {}\n\n  \
                     This will OVERWRITE the file if it exists, or create it if it doesn't.\n  \
                     Previous content will be LOST.",
                    path
                )
            }
            "file_edit" => {
                let path = args["path"].as_str().unwrap_or("(unknown)");
                format!(
                    "What it does:\n\n  Edit file: {}\n\n  \
                     This will modify the existing file by replacing specific text.\n  \
                     If the search text isn't found, the edit will fail.",
                    path
                )
            }
            "file_delete" => {
                let path = args["path"].as_str().unwrap_or("(unknown)");
                format!(
                    "What it does:\n\n  DELETE file: {}\n\n  \
                     This will permanently remove the file from disk.\n  \
                     This action CANNOT BE UNDONE.",
                    path
                )
            }
            "file_read" => {
                let path = args["path"].as_str().unwrap_or("(unknown)");
                format!(
                    "What it does:\n\n  Read file: {}\n\n  \
                     This will read the file's contents. No modifications will be made.",
                    path
                )
            }
            _ => format!(
                "What it does:\n\n  Tool: {}\n  Arguments: {}\n\n  \
                 (No detailed explanation available for this tool)",
                tool,
                serde_json::to_string_pretty(args).unwrap_or_default()
            ),
        }
    }

    /// Assess risk of the tool call.
    fn assess_risk(tool: &str, args: &serde_json::Value) -> String {
        let risk_level = RiskLevel::assess(tool, args);

        let mut lines = vec!["Risk Assessment:".to_string(), String::new()];

        lines.push(format!("  Overall Risk: {:?}", risk_level));
        lines.push(String::new());

        match tool {
            "bash" => {
                let cmd = args["command"].as_str().unwrap_or("");
                lines.push("  Destructiveness: High (executes arbitrary shell commands)".into());
                lines.push("  Reversibility: Low (depends on command)".into());

                if cmd.contains("rm -rf") {
                    lines.push(String::new());
                    lines.push("  🔴 WARNING: This command contains 'rm -rf' which can".into());
                    lines.push("     delete large amounts of data irreversibly.".into());
                }
                if cmd.contains("sudo") {
                    lines.push(String::new());
                    lines.push("  🔴 WARNING: This command uses 'sudo' which runs with".into());
                    lines.push("     elevated privileges.".into());
                }
            }
            "file_delete" => {
                let path = args["path"].as_str().unwrap_or("");
                lines.push("  Destructiveness: High (deletes file permanently)".into());
                lines.push("  Reversibility: None (file cannot be recovered)".into());

                if path.contains(".env") || path.contains("credentials") {
                    lines.push(String::new());
                    lines.push("  🔴 WARNING: This appears to be a sensitive file.".into());
                }
            }
            "file_write" | "file_edit" => {
                lines.push("  Destructiveness: Medium (modifies file)".into());
                lines.push("  Reversibility: Medium (can restore from git/backup)".into());
            }
            "file_read" | "grep" | "glob" => {
                lines.push("  Destructiveness: None (read-only operation)".into());
                lines.push("  Reversibility: N/A (no changes made)".into());
            }
            _ => {
                lines.push("  Destructiveness: Unknown".into());
                lines.push("  Reversibility: Unknown".into());
            }
        }

        lines.join("\n")
    }

    /// Show history of similar operations (placeholder for now).
    fn show_history(tool: &str) -> String {
        format!(
            "History:\n\n  Previous uses of '{}':\n\n  \
             (History tracking not yet implemented)\n\n  \
             This will show the last 5 times this tool was used,\n  \
             along with success/failure outcomes.",
            tool
        )
    }
}

/// Content of a permission prompt.
#[derive(Debug, Clone)]
pub struct PromptContent {
    pub title: String,
    pub summary: String,
    pub quick_options: Vec<String>,
    pub verbose_hint: Option<String>,
    pub detail_view: Option<String>,
}

/// Risk level classification for tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    Low,      // ReadOnly tools, safe operations
    Medium,   // ReadWrite, file edits
    High,     // Destructive, rm commands, system modifications
    Critical, // Phase 7: Blacklisted commands (rm -rf /, dd, fork bombs, etc.)
}

impl RiskLevel {
    /// Assess risk level based on tool and arguments.
    pub fn assess(tool: &str, args: &serde_json::Value) -> Self {
        match tool {
            "bash" => {
                let cmd = args["command"].as_str().unwrap_or("");
                if cmd.contains("rm -rf") || cmd.contains("sudo") || cmd.contains("dd if=")
                    || cmd.contains("> /dev/")
                {
                    RiskLevel::High
                } else if cmd.contains("rm ") || cmd.contains("mv ") || cmd.contains("cp ") {
                    RiskLevel::Medium
                } else {
                    RiskLevel::Low
                }
            }
            "file_delete" => RiskLevel::High,
            "file_write" | "file_edit" => {
                let path = args["path"].as_str().unwrap_or("");
                if path.contains(".env")
                    || path.contains("credentials")
                    || path.contains(".pem")
                    || path.contains(".key")
                    || path.starts_with("/etc/")
                    || path.starts_with("/sys/")
                {
                    RiskLevel::High
                } else {
                    RiskLevel::Medium
                }
            }
            "file_read" | "grep" | "glob" | "directory_tree" | "fuzzy_find" | "symbol_search" => {
                RiskLevel::Low
            }
            _ => RiskLevel::Medium, // Unknown tools default to medium
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risk_assessment_bash_dangerous() {
        let args = serde_json::json!({"command": "rm -rf /"});
        assert_eq!(RiskLevel::assess("bash", &args), RiskLevel::High);

        let args = serde_json::json!({"command": "sudo apt-get install malware"});
        assert_eq!(RiskLevel::assess("bash", &args), RiskLevel::High);
    }

    #[test]
    fn risk_assessment_bash_medium() {
        let args = serde_json::json!({"command": "rm test.txt"});
        assert_eq!(RiskLevel::assess("bash", &args), RiskLevel::Medium);

        let args = serde_json::json!({"command": "mv a.txt b.txt"});
        assert_eq!(RiskLevel::assess("bash", &args), RiskLevel::Medium);
    }

    #[test]
    fn risk_assessment_bash_safe() {
        let args = serde_json::json!({"command": "ls -la"});
        assert_eq!(RiskLevel::assess("bash", &args), RiskLevel::Low);

        let args = serde_json::json!({"command": "echo hello"});
        assert_eq!(RiskLevel::assess("bash", &args), RiskLevel::Low);
    }

    #[test]
    fn risk_assessment_file_delete_always_high() {
        let args = serde_json::json!({"path": "/tmp/test.txt"});
        assert_eq!(RiskLevel::assess("file_delete", &args), RiskLevel::High);
    }

    #[test]
    fn risk_assessment_file_write_sensitive() {
        let args = serde_json::json!({"path": ".env"});
        assert_eq!(RiskLevel::assess("file_write", &args), RiskLevel::High);

        let args = serde_json::json!({"path": "/etc/passwd"});
        assert_eq!(RiskLevel::assess("file_write", &args), RiskLevel::High);

        let args = serde_json::json!({"path": "credentials.json"});
        assert_eq!(RiskLevel::assess("file_write", &args), RiskLevel::High);
    }

    #[test]
    fn risk_assessment_file_write_normal() {
        let args = serde_json::json!({"path": "/tmp/test.txt"});
        assert_eq!(RiskLevel::assess("file_write", &args), RiskLevel::Medium);

        let args = serde_json::json!({"path": "src/main.rs"});
        assert_eq!(RiskLevel::assess("file_write", &args), RiskLevel::Medium);
    }

    #[test]
    fn risk_assessment_read_only_low() {
        let args = serde_json::json!({"path": "/tmp/test.txt"});
        assert_eq!(RiskLevel::assess("file_read", &args), RiskLevel::Low);

        let args = serde_json::json!({"pattern": "*.rs"});
        assert_eq!(RiskLevel::assess("grep", &args), RiskLevel::Low);
        assert_eq!(RiskLevel::assess("glob", &args), RiskLevel::Low);
    }

    #[test]
    fn risk_assessment_unknown_tool_medium() {
        let args = serde_json::json!({});
        assert_eq!(RiskLevel::assess("unknown_tool", &args), RiskLevel::Medium);
    }

    #[test]
    fn build_initial_prompt_low_risk() {
        let args = serde_json::json!({"path": "test.txt"});
        let prompt = AdaptivePromptBuilder::build_initial_prompt("file_read", &args, RiskLevel::Low);

        assert!(prompt.title.contains("✓"));
        assert!(prompt.title.contains("file_read"));
        assert!(prompt.summary.contains("File: test.txt"));
        assert_eq!(prompt.quick_options.len(), 4);
        assert!(prompt.verbose_hint.is_some());
    }

    #[test]
    fn build_initial_prompt_high_risk() {
        let args = serde_json::json!({"command": "rm -rf /"});
        let prompt = AdaptivePromptBuilder::build_initial_prompt("bash", &args, RiskLevel::High);

        assert!(prompt.title.contains("🔴"));
        assert!(prompt.verbose_hint.unwrap().contains("HIGH RISK"));
    }

    #[test]
    fn build_clarification_response() {
        let prompt = AdaptivePromptBuilder::build_clarification_response(
            "What does this do?",
            "It deletes the file.",
        );

        assert_eq!(prompt.title, "Clarification");
        assert!(prompt.summary.contains("Q: What does this do?"));
        assert!(prompt.summary.contains("A: It deletes the file."));
    }

    #[test]
    fn build_detail_view_parameters() {
        let args = serde_json::json!({"path": "test.txt", "content": "hello"});
        let prompt = AdaptivePromptBuilder::build_detail_view("file_write", &args, DetailAspect::Parameters);

        assert!(prompt.title.contains("Parameters"));
        assert!(prompt.detail_view.is_some());
        assert!(prompt.detail_view.unwrap().contains("path"));
    }

    #[test]
    fn build_detail_view_what_it_does() {
        let args = serde_json::json!({"path": "test.txt"});
        let prompt = AdaptivePromptBuilder::build_detail_view("file_delete", &args, DetailAspect::WhatItDoes);

        assert!(prompt.title.contains("What It Does"));
        let detail = prompt.detail_view.unwrap();
        assert!(detail.contains("DELETE"));
        assert!(detail.contains("CANNOT BE UNDONE"));
    }

    #[test]
    fn build_detail_view_risk_assessment() {
        let args = serde_json::json!({"command": "rm -rf /"});
        let prompt = AdaptivePromptBuilder::build_detail_view("bash", &args, DetailAspect::RiskAssessment);

        assert!(prompt.title.contains("Risk Assessment"));
        let detail = prompt.detail_view.unwrap();
        assert!(detail.contains("Risk Assessment"));
        assert!(detail.contains("rm -rf"));
    }

    #[test]
    fn summarize_tool_bash() {
        let args = serde_json::json!({"command": "ls -la"});
        let summary = AdaptivePromptBuilder::summarize_tool("bash", &args);
        assert_eq!(summary, "Command: ls -la");
    }

    #[test]
    fn summarize_tool_bash_long_command() {
        let long_cmd = "a".repeat(100);
        let args = serde_json::json!({"command": long_cmd});
        let summary = AdaptivePromptBuilder::summarize_tool("bash", &args);
        // "Command: " (9 chars) + 57 chars + "..." (3 chars) = 69 chars max
        assert!(summary.len() <= 70);
        assert!(summary.ends_with("..."));
    }

    #[test]
    fn summarize_tool_file_operations() {
        let args = serde_json::json!({"path": "test.txt"});
        assert_eq!(
            AdaptivePromptBuilder::summarize_tool("file_write", &args),
            "File: test.txt"
        );
        assert_eq!(
            AdaptivePromptBuilder::summarize_tool("file_read", &args),
            "File: test.txt"
        );
        assert_eq!(
            AdaptivePromptBuilder::summarize_tool("file_delete", &args),
            "File: test.txt"
        );
    }

    #[test]
    fn summarize_tool_grep() {
        let args = serde_json::json!({"pattern": "TODO", "path": "src/main.rs"});
        let summary = AdaptivePromptBuilder::summarize_tool("grep", &args);
        assert_eq!(summary, "Pattern: TODO in src/main.rs");
    }

    #[test]
    fn explain_tool_action_file_write() {
        let args = serde_json::json!({"path": "test.txt"});
        let explanation = AdaptivePromptBuilder::explain_tool_action("file_write", &args);
        assert!(explanation.contains("OVERWRITE"));
        assert!(explanation.contains("LOST"));
    }

    #[test]
    fn assess_risk_file_delete_warning() {
        let args = serde_json::json!({"path": ".env"});
        let assessment = AdaptivePromptBuilder::assess_risk("file_delete", &args);
        assert!(assessment.contains("WARNING"));
        assert!(assessment.contains("sensitive"));
    }

    #[test]
    fn assess_risk_bash_sudo_warning() {
        let args = serde_json::json!({"command": "sudo apt-get update"});
        let assessment = AdaptivePromptBuilder::assess_risk("bash", &args);
        assert!(assessment.contains("sudo"));
        assert!(assessment.contains("elevated privileges"));
    }

    #[test]
    fn render_parameters_object() {
        let args = serde_json::json!({"path": "test.txt", "content": "hello", "append": false});
        let rendered = AdaptivePromptBuilder::render_parameters(&args);
        assert!(rendered.contains("Parameters:"));
        assert!(rendered.contains("path"));
        assert!(rendered.contains("content"));
        assert!(rendered.contains("append"));
    }

    #[test]
    fn show_history_placeholder() {
        let history = AdaptivePromptBuilder::show_history("bash");
        assert!(history.contains("History"));
        assert!(history.contains("not yet implemented"));
    }
}
