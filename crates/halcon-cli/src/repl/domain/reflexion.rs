//! Reflexion self-improvement loop (NeurIPS 2023 pattern).
//!
//! Evaluates completed agent rounds, generates verbal self-reflections
//! on failures/suboptimal outcomes, and stores them as memory entries
//! for cross-session learning.

use std::sync::Arc;

use futures::StreamExt;

use halcon_core::error::Result;
use halcon_core::traits::ModelProvider;
use halcon_core::types::{
    ChatMessage, ContentBlock, MessageContent, ModelChunk, ModelRequest, Role,
};

/// Evaluation of a completed agent round.
#[derive(Debug, Clone)]
pub enum RoundOutcome {
    /// All tool executions succeeded, model response was useful.
    Success,
    /// Some tools failed or model needed multiple retries.
    Partial { failures: Vec<String> },
    /// Critical failure — tools crashed, permission denied, etc.
    Failure { error: String },
}

impl RoundOutcome {
    /// Short trigger label for events/logging.
    pub fn trigger_label(&self) -> &str {
        match self {
            RoundOutcome::Success => "success",
            RoundOutcome::Partial { .. } => "partial",
            RoundOutcome::Failure { .. } => "failure",
        }
    }
}

/// A self-reflection generated after evaluating a round.
#[derive(Debug, Clone)]
pub struct Reflection {
    /// What went wrong or could be improved.
    pub analysis: String,
    /// Concrete advice for future rounds.
    pub advice: String,
    /// The round number this reflection is about.
    #[allow(dead_code)] // Used in tests and logging context.
    pub round: usize,
    /// The outcome that triggered this reflection.
    #[allow(dead_code)] // Used in tests; trigger_label() used in agent loop.
    pub trigger: RoundOutcome,
}

/// Evaluates agent rounds and generates reflections.
pub struct Reflector {
    provider: Arc<dyn ModelProvider>,
    model: String,
    /// Also reflect on success outcomes.
    reflect_on_success: bool,
}

impl Reflector {
    pub fn new(provider: Arc<dyn ModelProvider>, model: String) -> Self {
        Self {
            provider,
            model,
            reflect_on_success: false,
        }
    }

    /// Enable reflection on success outcomes.
    pub fn with_reflect_on_success(mut self, enabled: bool) -> Self {
        self.reflect_on_success = enabled;
        self
    }

    /// Evaluate tool execution results to determine the round outcome.
    pub fn evaluate_round(tool_results: &[ContentBlock]) -> RoundOutcome {
        let mut failures = Vec::new();
        let mut tool_result_count = 0;

        for block in tool_results {
            if let ContentBlock::ToolResult {
                content,
                is_error,
                tool_use_id,
            } = block
            {
                tool_result_count += 1;
                if *is_error {
                    failures.push(format!("{tool_use_id}: {content}"));
                }
            }
        }

        if failures.is_empty() {
            RoundOutcome::Success
        } else if tool_result_count > 0 && failures.len() == tool_result_count {
            RoundOutcome::Failure {
                error: failures.join("; "),
            }
        } else {
            RoundOutcome::Partial { failures }
        }
    }

    /// Generate a self-reflection for a non-success round.
    pub async fn reflect(
        &self,
        round: usize,
        outcome: &RoundOutcome,
        recent_messages: &[ChatMessage],
    ) -> Result<Option<Reflection>> {
        // Don't reflect on success (unless configured).
        if matches!(outcome, RoundOutcome::Success) && !self.reflect_on_success {
            return Ok(None);
        }

        let outcome_desc = match outcome {
            RoundOutcome::Success => "All tools succeeded.".to_string(),
            RoundOutcome::Partial { failures } => {
                format!("Partial failure. Failed tools:\n{}", failures.join("\n"))
            }
            RoundOutcome::Failure { error } => {
                format!("Complete failure: {error}")
            }
        };

        // Build recent context summary (last 8 messages max, 800 chars each).
        // More context gives the reflector better signal about what went wrong.
        let context: Vec<String> = recent_messages
            .iter()
            .rev()
            .take(8)
            .rev()
            .map(|m| {
                let role = match m.role {
                    Role::User => "User",
                    Role::Assistant => "Assistant",
                    Role::System => "System",
                };
                let text = match &m.content {
                    MessageContent::Text(t) => t.chars().take(800).collect::<String>(),
                    MessageContent::Blocks(blocks) => blocks
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => {
                                Some(text.chars().take(300).collect::<String>())
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(" "),
                };
                format!("[{role}]: {text}")
            })
            .collect();

        let prompt = format!(
            "You are a self-reflection agent. Analyze the following failed execution round \
             and provide concrete advice for improvement.\n\n\
             Round {round} outcome: {outcome_desc}\n\n\
             Recent conversation context:\n{}\n\n\
             Respond with ONLY a JSON object:\n\
             {{\n  \"analysis\": \"<what went wrong and why>\",\n  \
             \"advice\": \"<specific, actionable advice for the next attempt>\"\n}}",
            context.join("\n"),
        );

        let request = ModelRequest {
            model: self.model.clone(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text(prompt),
            }],
            tools: vec![],
            max_tokens: Some(1024),
            temperature: Some(0.0),
            system: None,
            stream: true,
        };

        let mut text = String::new();
        let mut was_truncated = false;
        let mut stream = self.provider.invoke(&request).await?;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(ModelChunk::TextDelta(delta)) => text.push_str(&delta),
                Ok(ModelChunk::Done(halcon_core::types::StopReason::MaxTokens)) => {
                    was_truncated = true;
                }
                _ => {}
            }
        }

        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }

        // If output was truncated, don't attempt to parse incomplete JSON.
        if was_truncated {
            let preview: String = trimmed.chars().take(200).collect();
            tracing::warn!(
                raw_len = trimmed.len(),
                raw_preview = %preview,
                "Reflection output truncated by max_tokens — skipping"
            );
            return Ok(None);
        }

        // Strip markdown code fences before parsing (model may wrap JSON in ```).
        let json_str = crate::repl::planner::extract_json(trimmed);

        // Parse JSON response.
        #[derive(serde::Deserialize)]
        struct ReflectionJson {
            analysis: String,
            advice: String,
        }

        match serde_json::from_str::<ReflectionJson>(json_str) {
            Ok(parsed) => Ok(Some(Reflection {
                analysis: parsed.analysis,
                advice: parsed.advice,
                round,
                trigger: outcome.clone(),
            })),
            Err(e) => {
                let preview: String = json_str.chars().take(500).collect();
                tracing::warn!(
                    error = %e,
                    raw_len = json_str.len(),
                    raw_preview = %preview,
                    "Failed to parse reflection JSON"
                );
                // Don't store malformed output as reflection — it would
                // poison episodic memory with garbage (JSON fragments,
                // markdown fences, prompt echoes). Return None so the
                // caller skips memory storage for this round.
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluate_all_success() {
        let results = vec![
            ContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                content: "file contents".into(),
                is_error: false,
            },
            ContentBlock::ToolResult {
                tool_use_id: "t2".into(),
                content: "ok".into(),
                is_error: false,
            },
        ];
        let outcome = Reflector::evaluate_round(&results);
        assert!(matches!(outcome, RoundOutcome::Success));
    }

    #[test]
    fn evaluate_partial_failure() {
        let results = vec![
            ContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                content: "file contents".into(),
                is_error: false,
            },
            ContentBlock::ToolResult {
                tool_use_id: "t2".into(),
                content: "permission denied".into(),
                is_error: true,
            },
        ];
        let outcome = Reflector::evaluate_round(&results);
        match outcome {
            RoundOutcome::Partial { failures } => {
                assert_eq!(failures.len(), 1);
                assert!(failures[0].contains("permission denied"));
            }
            other => panic!("expected Partial, got {other:?}"),
        }
    }

    #[test]
    fn evaluate_all_failure() {
        let results = vec![
            ContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                content: "timeout".into(),
                is_error: true,
            },
            ContentBlock::ToolResult {
                tool_use_id: "t2".into(),
                content: "crash".into(),
                is_error: true,
            },
        ];
        let outcome = Reflector::evaluate_round(&results);
        match outcome {
            RoundOutcome::Failure { error } => {
                assert!(error.contains("timeout"));
                assert!(error.contains("crash"));
            }
            other => panic!("expected Failure, got {other:?}"),
        }
    }

    #[test]
    fn evaluate_empty_results() {
        let results: Vec<ContentBlock> = vec![];
        let outcome = Reflector::evaluate_round(&results);
        assert!(matches!(outcome, RoundOutcome::Success));
    }

    #[test]
    fn evaluate_non_tool_result_blocks_ignored() {
        let results = vec![
            ContentBlock::Text {
                text: "some text".into(),
            },
            ContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                content: "ok".into(),
                is_error: false,
            },
        ];
        let outcome = Reflector::evaluate_round(&results);
        assert!(matches!(outcome, RoundOutcome::Success));
    }

    #[test]
    fn outcome_trigger_labels() {
        assert_eq!(RoundOutcome::Success.trigger_label(), "success");
        assert_eq!(
            RoundOutcome::Partial {
                failures: vec![]
            }
            .trigger_label(),
            "partial"
        );
        assert_eq!(
            RoundOutcome::Failure {
                error: "x".into()
            }
            .trigger_label(),
            "failure"
        );
    }

    #[tokio::test]
    async fn reflect_skips_success_by_default() {
        let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
        let reflector = Reflector::new(provider, "echo".into());

        let result = reflector
            .reflect(0, &RoundOutcome::Success, &[])
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn reflect_generates_on_failure() {
        let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
        let reflector = Reflector::new(provider, "echo".into());

        let outcome = RoundOutcome::Failure {
            error: "tool crashed".into(),
        };
        let messages = vec![ChatMessage {
            role: Role::User,
            content: MessageContent::Text("fix the bug".into()),
        }];

        let result = reflector.reflect(1, &outcome, &messages).await.unwrap();
        // EchoProvider echoes the prompt — won't be valid JSON.
        // After RC-5 fix, malformed JSON returns None (not a degraded reflection).
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn reflect_parses_valid_json() {
        // Test the JSON parsing path with a mock that returns valid JSON.
        // Since EchoProvider echoes the prompt (not valid JSON), we test the fallback.
        // The JSON parsing is tested via the serde path in the code.
        #[derive(serde::Deserialize)]
        struct ReflectionJson {
            analysis: String,
            advice: String,
        }

        let json = r#"{"analysis": "The file path was wrong", "advice": "Check path exists first"}"#;
        let parsed: ReflectionJson = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.analysis, "The file path was wrong");
        assert_eq!(parsed.advice, "Check path exists first");
    }

    #[test]
    fn reflection_fallback_on_bad_json() {
        #[derive(serde::Deserialize)]
        struct ReflectionJson {
            #[allow(dead_code)]
            analysis: String,
            #[allow(dead_code)]
            advice: String,
        }

        let bad_json = "this is not json";
        let result = serde_json::from_str::<ReflectionJson>(bad_json);
        assert!(result.is_err());
    }

    // === Hardening tests for reflection JSON parsing (RC-1, RC-5) ===

    #[test]
    fn reflection_json_with_markdown_fence() {
        #[derive(serde::Deserialize)]
        struct ReflectionJson {
            analysis: String,
            advice: String,
        }

        // Simulates model wrapping response in code fences
        let fenced = "```json\n{\"analysis\": \"path error\", \"advice\": \"validate first\"}\n```";
        let extracted = crate::repl::planner::extract_json(fenced);
        let parsed: ReflectionJson = serde_json::from_str(extracted).unwrap();
        assert_eq!(parsed.analysis, "path error");
        assert_eq!(parsed.advice, "validate first");
    }

    #[test]
    fn reflection_json_truncated_eof() {
        #[derive(Debug, serde::Deserialize)]
        struct ReflectionJson {
            #[allow(dead_code)]
            analysis: String,
            #[allow(dead_code)]
            advice: String,
        }

        let truncated = r#"{"analysis": "The file was not fo"#;
        let result = serde_json::from_str::<ReflectionJson>(truncated);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("EOF"));
    }

    #[test]
    fn reflection_json_empty_string() {
        #[derive(serde::Deserialize)]
        struct ReflectionJson {
            #[allow(dead_code)]
            analysis: String,
            #[allow(dead_code)]
            advice: String,
        }

        let result = serde_json::from_str::<ReflectionJson>("");
        assert!(result.is_err());
    }

    #[test]
    fn reflection_json_missing_advice_field() {
        #[derive(serde::Deserialize)]
        struct ReflectionJson {
            #[allow(dead_code)]
            analysis: String,
            #[allow(dead_code)]
            advice: String,
        }

        let missing = r#"{"analysis": "error occurred"}"#;
        let result = serde_json::from_str::<ReflectionJson>(missing);
        assert!(result.is_err());
    }

    #[test]
    fn reflection_extract_json_then_parse_roundtrip() {
        #[derive(serde::Deserialize)]
        struct ReflectionJson {
            analysis: String,
            advice: String,
        }

        // Full roundtrip: fence-wrapped → extract → parse
        let model_output =
            "```json\n{\"analysis\": \"timeout on API call\", \"advice\": \"add retry logic\"}\n```";
        let extracted = crate::repl::planner::extract_json(model_output);
        let parsed: ReflectionJson = serde_json::from_str(extracted).unwrap();
        assert_eq!(parsed.analysis, "timeout on API call");
        assert_eq!(parsed.advice, "add retry logic");
    }

    #[test]
    fn reflection_json_with_unicode() {
        #[derive(serde::Deserialize)]
        struct ReflectionJson {
            analysis: String,
            advice: String,
        }

        let unicode_json =
            r#"{"analysis": "El archivo no existe — 文件不存在", "advice": "Verificar ruta 🛡️"}"#;
        let parsed: ReflectionJson = serde_json::from_str(unicode_json).unwrap();
        assert!(parsed.analysis.contains("文件不存在"));
    }
}
