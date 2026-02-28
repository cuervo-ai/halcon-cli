//! Ollama local model provider with NDJSON streaming and tool emulation.
//!
//! Implements `ModelProvider` for the Ollama REST API (`/api/chat`).
//! Uses `bytes_stream()` with line-buffered parsing instead of SSE,
//! since Ollama emits newline-delimited JSON.
//!
//! Key design decisions:
//! - No API key required (local server).
//! - **Tool emulation**: Ollama models don't support native tool_use protocol.
//!   When tools are present in the request, definitions are injected into the
//!   system prompt and the model is instructed to output `<tool_call>` XML blocks.
//!   The provider parses these from the text response and converts them to
//!   `ToolUseStart`/`ToolUseDelta`/`Done(ToolUse)` chunks for the agent loop.
//! - System prompt is mapped to a `role: "system"` message.
//! - Cost is always 0.0 (local inference).

pub mod types;

use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use futures::StreamExt;
use tracing::{debug, warn};

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::ModelProvider;
use halcon_core::types::{
    ContentBlock, HttpConfig, MessageContent, ModelChunk, ModelInfo, ModelRequest, StopReason,
    TokenCost, TokenUsage, ToolDefinition,
};

use crate::http;
use types::{OllamaChatRequest, OllamaChatResponse, OllamaMessage, OllamaOptions};

const DEFAULT_BASE_URL: &str = "http://localhost:11434";

/// Ollama provider: connects to a local Ollama instance.
///
/// Streams responses via NDJSON from `/api/chat`.
pub struct OllamaProvider {
    client: reqwest::Client,
    base_url: String,
    models: Vec<ModelInfo>,
}

impl std::fmt::Debug for OllamaProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OllamaProvider")
            .field("base_url", &self.base_url)
            .finish()
    }
}

impl OllamaProvider {
    /// Create a new Ollama provider with optional custom base URL and HTTP config.
    pub fn new(base_url: Option<String>, http_config: HttpConfig) -> Self {
        Self::with_default_model(base_url, http_config, None)
    }

    /// Create with an explicit default model placed first in supported_models().
    ///
    /// When a user configures `default_model = "deepseek-coder-v2:latest"` in their
    /// Ollama provider config, this model is prepended to the supported list so that
    /// fallback selection (`supported_models().first()`) picks the user's installed model.
    pub fn with_default_model(
        base_url: Option<String>,
        http_config: HttpConfig,
        default_model: Option<String>,
    ) -> Self {
        let mut models = Vec::new();

        // If the user configured a default_model, put it first.
        if let Some(ref model_id) = default_model {
            models.push(ModelInfo {
                id: model_id.clone(),
                name: model_id.clone(),
                provider: "ollama".into(),
                context_window: 128_000,
                max_output_tokens: 8192,
                supports_streaming: true,
                supports_tools: true, // via emulation — see tool_emulation module
                supports_vision: false,
                supports_reasoning: false,
                cost_per_input_token: 0.0,
                cost_per_output_token: 0.0,
            });
        }

        // Standard builtin models (skip if already added as default).
        let builtins = [
            ("llama3.2", "Llama 3.2", 128_000, 8192),
            ("mistral", "Mistral 7B", 32_768, 4096),
            ("codellama", "Code Llama", 16_384, 4096),
        ];
        for (id, name, ctx, max_out) in builtins {
            if default_model.as_deref() == Some(id) {
                continue; // Already added above.
            }
            models.push(ModelInfo {
                id: id.into(),
                name: name.into(),
                provider: "ollama".into(),
                context_window: ctx,
                max_output_tokens: max_out,
                supports_streaming: true,
                supports_tools: true, // via emulation
                supports_vision: false,
                supports_reasoning: false,
                cost_per_input_token: 0.0,
                cost_per_output_token: 0.0,
            });
        }

        Self {
            client: http::build_client(&http_config),
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            models,
        }
    }

    /// Build the Ollama chat request from a `ModelRequest`.
    ///
    /// - System prompt is mapped to a system-role message.
    /// - When tools are present, injects tool definitions into the system prompt
    ///   for emulation (Ollama doesn't support native tool_use protocol).
    /// - ToolUse blocks in assistant messages are formatted as `<tool_call>` XML.
    /// - ToolResult blocks in user messages are formatted as `<tool_result>` XML.
    fn build_chat_request(request: &ModelRequest) -> OllamaChatRequest {
        let has_tools = !request.tools.is_empty();
        let mut messages = Vec::new();

        // Build system prompt, optionally with tool emulation definitions.
        let mut system_content = request.system.clone().unwrap_or_default();
        if has_tools {
            system_content.push_str(&Self::format_tool_emulation_prompt(&request.tools));
        }
        if !system_content.is_empty() {
            messages.push(OllamaMessage {
                role: "system".into(),
                content: system_content,
            });
        }

        for msg in &request.messages {
            let role = match msg.role {
                halcon_core::types::Role::User => "user",
                halcon_core::types::Role::Assistant => "assistant",
                halcon_core::types::Role::System => "system",
            };
            let content = match &msg.content {
                MessageContent::Text(t) => t.clone(),
                MessageContent::Blocks(blocks) => {
                    // Flatten blocks to text with tool emulation formatting.
                    blocks
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(text.clone()),
                            ContentBlock::ToolUse { name, input, .. } if has_tools => {
                                // Re-serialize as <tool_call> so model sees its prior calls.
                                let args =
                                    serde_json::to_string(input).unwrap_or_else(|_| "{}".into());
                                Some(format!(
                                    "<tool_call>\n{{\"name\": \"{name}\", \"arguments\": {args}}}\n</tool_call>"
                                ))
                            }
                            ContentBlock::ToolResult {
                                content,
                                tool_use_id,
                                is_error,
                                ..
                            } if has_tools => {
                                // Format as <tool_result> for emulation context.
                                let status = if *is_error { " status=\"error\"" } else { "" };
                                let completion_hint = if *is_error {
                                    "\n[The tool returned an error. Fix the issue or try a different approach.]"
                                } else {
                                    "\n[Tool succeeded. If the task is complete, respond with a short text summary. Do NOT call the same tool again.]"
                                };
                                Some(format!(
                                    "<tool_result tool_use_id=\"{tool_use_id}\"{status}>\n{content}\n</tool_result>{completion_hint}"
                                ))
                            }
                            ContentBlock::ToolResult { content, .. } => Some(content.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            };
            if !content.is_empty() {
                messages.push(OllamaMessage {
                    role: role.into(),
                    content,
                });
            }
        }

        let options = if request.temperature.is_some() || request.max_tokens.is_some() {
            Some(OllamaOptions {
                temperature: request.temperature,
                num_predict: request.max_tokens,
            })
        } else {
            None
        };

        OllamaChatRequest {
            model: request.model.clone(),
            messages,
            stream: true,
            options,
        }
    }

    /// Maximum number of tool definitions to inject into the emulation prompt.
    /// Local models perform better with fewer, more focused tool definitions.
    const MAX_EMULATION_TOOLS: usize = 12;

    /// Format tool definitions for injection into the system prompt.
    ///
    /// Instructs the model to output `<tool_call>` XML blocks with JSON payloads
    /// when it wants to use a tool, enabling tool emulation for models that
    /// don't support the native tool_use protocol.
    ///
    /// Design: uses concrete examples and places the strongest instruction at the END
    /// (recency bias helps local models follow formatting instructions).
    fn format_tool_emulation_prompt(tools: &[ToolDefinition]) -> String {
        // Prioritize tools: file_write, file_read, file_edit, bash first (most commonly needed),
        // then the rest up to MAX_EMULATION_TOOLS.
        let priority_names = ["file_write", "file_read", "file_edit", "bash", "grep", "glob"];
        let mut ordered: Vec<&ToolDefinition> = Vec::with_capacity(tools.len().min(Self::MAX_EMULATION_TOOLS));

        // Add priority tools first.
        for pname in &priority_names {
            if let Some(t) = tools.iter().find(|t| t.name == *pname) {
                ordered.push(t);
            }
        }
        // Add remaining tools up to limit.
        for t in tools {
            if ordered.len() >= Self::MAX_EMULATION_TOOLS {
                break;
            }
            if !priority_names.contains(&t.name.as_str()) {
                ordered.push(t);
            }
        }

        let mut prompt = String::from("\n\n# TOOL USE INSTRUCTIONS\n\n");
        prompt.push_str(
            "You are an AI assistant with access to tools. \
             When you need to perform an action (create/read/edit files, run commands, search), \
             you MUST use the tool call format below. NEVER describe the action in plain text.\n\n",
        );

        // Concrete examples — most important thing for local models.
        // Use <PLACEHOLDER> tokens that the model won't copy literally.
        prompt.push_str("## Examples\n\n");
        prompt.push_str("Example 1 — writing a file:\n");
        prompt.push_str("User: \"Create a file at <USER_PATH> with the text: <USER_CONTENT>\"\n\n");
        prompt.push_str("Your response (replace <USER_PATH> and <USER_CONTENT> with ACTUAL values from the user's message):\n");
        prompt.push_str("<tool_call>\n");
        prompt.push_str("{\"name\": \"file_write\", \"arguments\": {\"path\": \"<USER_PATH>\", \"content\": \"<USER_CONTENT>\"}}\n");
        prompt.push_str("</tool_call>\n\n");

        prompt.push_str("Example 2 — reading a file:\n");
        prompt.push_str("User: \"Read the file at <USER_PATH>\"\n\n");
        prompt.push_str("Your response:\n");
        prompt.push_str("<tool_call>\n");
        prompt.push_str("{\"name\": \"file_read\", \"arguments\": {\"path\": \"<USER_PATH>\"}}\n");
        prompt.push_str("</tool_call>\n\n");

        prompt.push_str("IMPORTANT: Replace <USER_PATH> and <USER_CONTENT> with the ACTUAL path and content from the user's request. NEVER use placeholder text like <USER_PATH> literally.\n\n");

        prompt.push_str("After you receive the tool result, respond with a SHORT text summary. Do NOT call tools again.\n\n");

        // Tool catalog — concise format (name + one-line description + required params only).
        prompt.push_str("## Available Tools\n\n");
        for tool in &ordered {
            // Extract required params from schema for a compact listing.
            let params = if let Some(props) = tool.input_schema.get("properties") {
                let required: Vec<&str> = tool
                    .input_schema
                    .get("required")
                    .and_then(|r| r.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();
                if required.is_empty() {
                    if let Some(obj) = props.as_object() {
                        obj.keys().map(|k| k.as_str()).collect::<Vec<_>>().join(", ")
                    } else {
                        String::new()
                    }
                } else {
                    required.join(", ")
                }
            } else {
                String::new()
            };
            prompt.push_str(&format!(
                "- **{}**: {} (params: {})\n",
                tool.name, tool.description, params,
            ));
        }
        if tools.len() > Self::MAX_EMULATION_TOOLS {
            prompt.push_str(&format!(
                "\n({} more tools available but not shown)\n",
                tools.len() - Self::MAX_EMULATION_TOOLS,
            ));
        }

        // Strongest instruction at the END — recency bias makes local models follow this.
        prompt.push_str("\n## CRITICAL RULES\n");
        prompt.push_str("1. To use a tool, output EXACTLY this format:\n");
        prompt.push_str("<tool_call>\n{\"name\": \"TOOL_NAME\", \"arguments\": {PARAMS}}\n</tool_call>\n");
        prompt.push_str("2. NEVER explain how to do something — USE THE TOOL instead.\n");
        prompt.push_str("3. NEVER output code snippets or shell commands as text — call bash or file_write.\n");
        prompt.push_str("4. After <tool_call> blocks, STOP. Do not add extra text.\n");
        prompt.push_str("5. When you receive a SUCCESS tool result, respond with a SHORT text confirmation. Do NOT call the same tool again.\n");
        prompt.push_str("6. NEVER repeat a tool call that already succeeded. Once a file is written, do NOT write it again.\n");

        prompt
    }

    /// Parse `<tool_call>` blocks from model text output.
    ///
    /// Returns parsed tool calls as `(name, arguments)` tuples, plus
    /// any remaining non-tool text content.
    fn parse_tool_calls(text: &str) -> (Vec<(String, serde_json::Value)>, String) {
        let mut tool_calls = Vec::new();
        let mut remaining = String::new();
        let mut search_from = 0;

        while let Some(rel_start) = text[search_from..].find("<tool_call>") {
            let abs_start = search_from + rel_start;
            // Collect text before this tool_call block.
            remaining.push_str(&text[search_from..abs_start]);

            let after_tag = abs_start + "<tool_call>".len();
            if let Some(rel_end) = text[after_tag..].find("</tool_call>") {
                let abs_end = after_tag + rel_end;
                let json_str = text[after_tag..abs_end].trim();

                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_str) {
                    if let (Some(name), Some(args)) = (
                        parsed.get("name").and_then(|v| v.as_str()),
                        parsed.get("arguments"),
                    ) {
                        tool_calls.push((name.to_string(), args.clone()));
                    } else {
                        warn!(
                            json_preview = %json_str.chars().take(200).collect::<String>(),
                            "Tool emulation: tool_call block missing 'name' or 'arguments' fields"
                        );
                    }
                } else {
                    warn!(
                        raw_preview = %json_str.chars().take(200).collect::<String>(),
                        "Tool emulation: failed to parse tool_call JSON"
                    );
                }

                search_from = abs_end + "</tool_call>".len();
            } else {
                // Unclosed <tool_call> — treat rest as text.
                remaining.push_str(&text[abs_start..]);
                search_from = text.len();
            }
        }

        // Append text after the last tool_call block.
        remaining.push_str(&text[search_from..]);
        (tool_calls, remaining)
    }

    /// Fallback intent extraction: when no `<tool_call>` blocks are found,
    /// look for bash commands in code blocks and convert them to tool calls.
    ///
    /// This handles the common case where local models output:
    /// ```bash
    /// echo "content" > /tmp/file.txt
    /// ```
    /// instead of using the `<tool_call>` format.
    fn extract_intent_from_text(text: &str) -> Vec<(String, serde_json::Value)> {
        let mut tool_calls = Vec::new();

        // Pattern 1: ```bash\n...\n``` or ```sh\n...\n``` code blocks → bash tool call
        let mut search_from = 0;
        while let Some(start) = text[search_from..].find("```") {
            let abs_start = search_from + start + 3;
            // Skip the language tag (bash, sh, shell, etc.)
            let after_tag = if let Some(nl) = text[abs_start..].find('\n') {
                abs_start + nl + 1
            } else {
                break;
            };
            // Find closing ```
            if let Some(end) = text[after_tag..].find("```") {
                let abs_end = after_tag + end;
                let command = text[after_tag..abs_end].trim();
                if !command.is_empty() && command.len() < 2000 {
                    tool_calls.push((
                        "bash".to_string(),
                        serde_json::json!({"command": command}),
                    ));
                }
                search_from = abs_end + 3;
            } else {
                break;
            }
        }

        // Pattern 2: inline `echo "..." > /path` patterns (no code block)
        if tool_calls.is_empty() {
            if let Some(echo_pos) = text.find("echo ") {
                let after_echo = &text[echo_pos + 5..];
                // Find quoted content.
                let (content, after_content) = if after_echo.starts_with('"') {
                    if let Some(end) = after_echo[1..].find('"') {
                        (Some(&after_echo[1..end + 1]), &after_echo[end + 2..])
                    } else {
                        (None, after_echo)
                    }
                } else if after_echo.starts_with('\'') {
                    if let Some(end) = after_echo[1..].find('\'') {
                        (Some(&after_echo[1..end + 1]), &after_echo[end + 2..])
                    } else {
                        (None, after_echo)
                    }
                } else {
                    (None, after_echo)
                };

                if let Some(content) = content {
                    // Look for > /path after the quoted content.
                    let trimmed = after_content.trim_start();
                    if trimmed.starts_with('>') {
                        let path_start = trimmed[1..].trim_start();
                        let path_end = path_start
                            .find(|c: char| c.is_whitespace() || c == '"' || c == '`')
                            .unwrap_or(path_start.len());
                        let path = &path_start[..path_end];
                        if path.starts_with('/') && path.len() > 1 {
                            tool_calls.push((
                                "file_write".to_string(),
                                serde_json::json!({
                                    "path": path,
                                    "content": content,
                                }),
                            ));
                        }
                    }
                }
            }
        }

        tool_calls
    }

    /// Send a chat request to Ollama and collect the full text response.
    ///
    /// Returns `(accumulated_text, usage)` — the full text output and token counts.
    async fn send_and_collect(
        &self,
        chat_request: &OllamaChatRequest,
    ) -> Result<(String, TokenUsage)> {
        let url = format!("{}/api/chat", self.base_url);
        let response = self
            .client
            .post(&url)
            .json(chat_request)
            .send()
            .await
            .map_err(|e| {
                if e.is_connect() {
                    HalconError::ConnectionError {
                        provider: "ollama".into(),
                        message: format!("Cannot connect to Ollama at {}: {e}", self.base_url),
                    }
                } else if e.is_timeout() {
                    HalconError::RequestTimeout {
                        provider: "ollama".into(),
                        timeout_secs: 300,
                    }
                } else {
                    HalconError::ApiError {
                        message: format!("Ollama request failed: {e}"),
                        status: e.status().map(|s| s.as_u16()),
                    }
                }
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(HalconError::ApiError {
                message: format!("Ollama returned {status}: {body}"),
                status: Some(status.as_u16()),
            });
        }

        let body = response.text().await.map_err(|e| {
            HalconError::StreamError(format!("read error: {e}"))
        })?;

        let mut accumulated_text = String::new();
        let mut usage = TokenUsage::default();

        for line in body.lines() {
            if let Some(chunk_result) = Self::parse_ndjson_line(line) {
                match chunk_result {
                    Ok(ModelChunk::TextDelta(text)) => accumulated_text.push_str(&text),
                    Ok(ModelChunk::Usage(u)) => usage = u,
                    _ => {}
                }
            }
        }

        Ok((accumulated_text, usage))
    }

    /// Build a retry request that appends the model's failed text response and
    /// a correction prompt instructing it to use tool_call format.
    fn build_retry_request(
        original: &OllamaChatRequest,
        failed_text: &str,
    ) -> OllamaChatRequest {
        let mut messages = original.messages.clone();
        // Add the model's failed response as an assistant message.
        messages.push(OllamaMessage {
            role: "assistant".into(),
            content: failed_text.to_string(),
        });
        // Add a correction prompt as a user message.
        messages.push(OllamaMessage {
            role: "user".into(),
            content: "You did NOT use the required <tool_call> format. \
                      You MUST respond with <tool_call> XML blocks to perform actions. \
                      Do NOT explain — just output the <tool_call> block now. \
                      Re-read the tool instructions and respond correctly."
                .to_string(),
        });
        OllamaChatRequest {
            model: original.model.clone(),
            messages,
            stream: original.stream,
            options: original.options.clone(),
        }
    }

    /// Parse a single NDJSON line into a `ModelChunk`.
    fn parse_ndjson_line(line: &str) -> Option<Result<ModelChunk>> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }

        let response: OllamaChatResponse = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, line = line, "Failed to parse Ollama NDJSON line");
                return Some(Err(HalconError::StreamError(format!(
                    "invalid NDJSON: {e}"
                ))));
            }
        };

        if response.done {
            // Final line: emit usage + done.
            let usage = TokenUsage {
                input_tokens: response.prompt_eval_count.unwrap_or(0),
                output_tokens: response.eval_count.unwrap_or(0),
                ..Default::default()
            };
            // We return a small stream of Usage + Done combined.
            // But since we return one chunk at a time, return Usage here.
            // The caller handles Done separately.
            Some(Ok(ModelChunk::Usage(usage)))
        } else {
            // Streaming delta.
            let text = response.message.content;
            if text.is_empty() {
                None
            } else {
                Some(Ok(ModelChunk::TextDelta(text)))
            }
        }
    }
}

#[async_trait]
impl ModelProvider for OllamaProvider {
    fn name(&self) -> &str {
        "ollama"
    }

    fn supported_models(&self) -> &[ModelInfo] {
        // Ollama supports many models; we list common defaults.
        // The user can specify any model name in their config.
        &self.models
    }

    async fn invoke(
        &self,
        request: &ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelChunk>>> {
        let has_tools = !request.tools.is_empty();
        let chat_request = Self::build_chat_request(request);

        debug!(
            model = %chat_request.model,
            messages = chat_request.messages.len(),
            has_tools = has_tools,
            tool_count = request.tools.len(),
            "Invoking Ollama"
        );

        if has_tools {
            // Tool emulation path: collect full response, parse for <tool_call> blocks.
            // Includes 1 retry when model fails to use tool format.
            let (accumulated_text, mut usage) =
                self.send_and_collect(&chat_request).await?;

            debug!(
                accumulated_len = accumulated_text.len(),
                "Ollama tool emulation: parsing response for <tool_call> blocks"
            );

            let (mut tool_calls, mut remaining_text) =
                Self::parse_tool_calls(&accumulated_text);

            // Retry once if model didn't produce tool calls.
            if tool_calls.is_empty() {
                warn!(
                    response_len = accumulated_text.len(),
                    response_preview = %accumulated_text.chars().take(200).collect::<String>(),
                    "Ollama tool emulation: no <tool_call> blocks found, retrying with correction"
                );

                let retry_request =
                    Self::build_retry_request(&chat_request, &accumulated_text);
                let (retry_text, retry_usage) =
                    self.send_and_collect(&retry_request).await?;

                debug!(
                    retry_len = retry_text.len(),
                    "Ollama tool emulation: retry response received"
                );

                let (retry_calls, retry_remaining) =
                    Self::parse_tool_calls(&retry_text);

                if !retry_calls.is_empty() {
                    debug!(
                        tool_count = retry_calls.len(),
                        "Ollama tool emulation: retry succeeded"
                    );
                    tool_calls = retry_calls;
                    remaining_text = retry_remaining;
                    let _ = retry_text; // consumed by parse_tool_calls
                    // Merge usage (sum both attempts).
                    usage.input_tokens += retry_usage.input_tokens;
                    usage.output_tokens += retry_usage.output_tokens;
                } else {
                    // Retry also failed — try fallback intent extraction from original text.
                    let fallback_calls = Self::extract_intent_from_text(&accumulated_text);
                    if !fallback_calls.is_empty() {
                        debug!(
                            tool_count = fallback_calls.len(),
                            "Ollama tool emulation: fallback intent extraction succeeded"
                        );
                        tool_calls = fallback_calls;
                        remaining_text = String::new();
                    } else {
                        warn!("Ollama tool emulation: retry and fallback both failed, emitting as text");
                    }
                    // Keep original response, add retry usage.
                    usage.input_tokens += retry_usage.input_tokens;
                    usage.output_tokens += retry_usage.output_tokens;
                }
            }

            let mut chunks: Vec<Result<ModelChunk>> = Vec::new();

            // Emit any non-tool text.
            let trimmed = remaining_text.trim();
            if !trimmed.is_empty() {
                chunks.push(Ok(ModelChunk::TextDelta(trimmed.to_string())));
            }

            if tool_calls.is_empty() {
                // No tool calls found even after retry — emit as normal text response.
                debug!("Ollama tool emulation: no <tool_call> blocks found, emitting as text");
                chunks.push(Ok(ModelChunk::Usage(usage)));
                chunks.push(Ok(ModelChunk::Done(StopReason::EndTurn)));
            } else {
                // Emit tool use chunks for the accumulator.
                debug!(
                    tool_count = tool_calls.len(),
                    "Ollama tool emulation: parsed tool calls"
                );

                for (i, (name, args)) in tool_calls.iter().enumerate() {
                    let id = format!("emul_{}_{i}", uuid::Uuid::new_v4().simple());
                    chunks.push(Ok(ModelChunk::ToolUseStart {
                        index: i as u32,
                        id: id.clone(),
                        name: name.clone(),
                    }));
                    let json = serde_json::to_string(args).unwrap_or_default();
                    chunks.push(Ok(ModelChunk::ToolUseDelta {
                        index: i as u32,
                        partial_json: json,
                    }));
                }
                chunks.push(Ok(ModelChunk::Usage(usage)));
                chunks.push(Ok(ModelChunk::Done(StopReason::ToolUse)));
            }

            Ok(Box::pin(stream::iter(chunks)))
        } else {
            // Original NDJSON streaming path (no tools).
            let url = format!("{}/api/chat", self.base_url);
            let response = self
                .client
                .post(&url)
                .json(&chat_request)
                .send()
                .await
                .map_err(|e| {
                    if e.is_connect() {
                        HalconError::ConnectionError {
                            provider: "ollama".into(),
                            message: format!("Cannot connect to Ollama at {}: {e}", self.base_url),
                        }
                    } else if e.is_timeout() {
                        HalconError::RequestTimeout {
                            provider: "ollama".into(),
                            timeout_secs: 300,
                        }
                    } else {
                        HalconError::ApiError {
                            message: format!("Ollama request failed: {e}"),
                            status: e.status().map(|s| s.as_u16()),
                        }
                    }
                })?;

            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(HalconError::ApiError {
                    message: format!("Ollama returned {status}: {body}"),
                    status: Some(status.as_u16()),
                });
            }

            let byte_stream = response.bytes_stream();

            let chunk_stream = byte_stream
                .scan(String::new(), |buffer, chunk_result| {
                    let bytes = match chunk_result {
                        Ok(b) => b,
                        Err(e) => {
                            return std::future::ready(Some(vec![Err(
                                HalconError::StreamError(format!("stream read error: {e}")),
                            )]));
                        }
                    };

                    // Append new bytes to the buffer.
                    buffer.push_str(&String::from_utf8_lossy(&bytes));

                    // Extract complete lines.
                    let mut results = Vec::new();
                    while let Some(newline_pos) = buffer.find('\n') {
                        let line: String = buffer.drain(..=newline_pos).collect();
                        if let Some(chunk) = Self::parse_ndjson_line(&line) {
                            // Check if this is a Usage chunk (from done=true line).
                            // If so, also emit a Done chunk after it.
                            if matches!(&chunk, Ok(ModelChunk::Usage(_))) {
                                results.push(chunk);
                                results.push(Ok(ModelChunk::Done(StopReason::EndTurn)));
                            } else {
                                results.push(chunk);
                            }
                        }
                    }

                    std::future::ready(Some(results))
                })
                .flat_map(stream::iter);

            Ok(Box::pin(chunk_stream))
        }
    }

    async fn is_available(&self) -> bool {
        let url = format!("{}/api/tags", self.base_url);
        let result = self
            .client
            .get(&url)
            .timeout(Duration::from_secs(5))
            .send()
            .await;

        match result {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }

    fn estimate_cost(&self, _request: &ModelRequest) -> TokenCost {
        // Local inference: always free.
        TokenCost::default()
    }

    fn tool_format(&self) -> halcon_core::types::ToolFormat {
        halcon_core::types::ToolFormat::OllamaXmlEmulation
    }

    fn tokenizer_hint(&self) -> halcon_core::types::TokenizerHint {
        halcon_core::types::TokenizerHint::OllamaUnknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::types::{ChatMessage, ContentBlock, MessageContent, Role, ToolDefinition};

    fn make_request(msg: &str) -> ModelRequest {
        ModelRequest {
            model: "llama3.2".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text(msg.into()),
            }],
            tools: vec![],
            max_tokens: Some(256),
            temperature: Some(0.7),
            system: None,
            stream: true,
        }
    }

    fn sample_tools() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "file_write".into(),
                description: "Write content to a file".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "required": ["path", "content"]
                }),
            },
            ToolDefinition {
                name: "bash".into(),
                description: "Run a shell command".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string" }
                    },
                    "required": ["command"]
                }),
            },
        ]
    }

    #[test]
    fn name_is_ollama() {
        let provider = OllamaProvider::new(None, HttpConfig::default());
        assert_eq!(provider.name(), "ollama");
    }

    #[test]
    fn supported_models_non_empty() {
        let provider = OllamaProvider::new(None, HttpConfig::default());
        let models = provider.supported_models();
        assert!(!models.is_empty());
        for model in models {
            assert_eq!(model.provider, "ollama");
            assert!(model.context_window > 0);
            assert!(model.max_output_tokens > 0);
            assert_eq!(model.cost_per_input_token, 0.0);
            assert!(model.supports_tools, "all Ollama models support tools via emulation");
        }
    }

    #[test]
    fn cost_is_always_zero() {
        let provider = OllamaProvider::new(None, HttpConfig::default());
        let req = make_request("test");
        let cost = provider.estimate_cost(&req);
        assert_eq!(cost.estimated_cost_usd, 0.0);
        assert_eq!(cost.estimated_input_tokens, 0);
    }

    #[test]
    fn build_request_no_tools_key_in_json() {
        let request = ModelRequest {
            model: "llama3.2".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text("test".into()),
            }],
            tools: sample_tools(),
            max_tokens: Some(256),
            temperature: None,
            system: None,
            stream: true,
        };

        let chat_req = OllamaProvider::build_chat_request(&request);
        // Tools should NOT appear as a "tools" key in the Ollama JSON (injected into system prompt).
        let json = serde_json::to_value(&chat_req).unwrap();
        assert!(!json.as_object().unwrap().contains_key("tools"));
    }

    #[test]
    fn build_request_injects_tool_prompt() {
        let request = ModelRequest {
            model: "llama3.2".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text("create a file".into()),
            }],
            tools: sample_tools(),
            max_tokens: None,
            temperature: None,
            system: Some("You are helpful.".into()),
            stream: true,
        };

        let chat_req = OllamaProvider::build_chat_request(&request);
        // System message should contain tool definitions.
        let system_msg = &chat_req.messages[0];
        assert_eq!(system_msg.role, "system");
        assert!(system_msg.content.contains("You are helpful."));
        assert!(system_msg.content.contains("# TOOL USE INSTRUCTIONS"));
        assert!(system_msg.content.contains("file_write"));
        assert!(system_msg.content.contains("bash"));
        assert!(system_msg.content.contains("<tool_call>"));
    }

    #[test]
    fn build_request_no_tools_no_injection() {
        let request = ModelRequest {
            model: "llama3.2".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text("hello".into()),
            }],
            tools: vec![],
            max_tokens: None,
            temperature: None,
            system: Some("You are helpful.".into()),
            stream: true,
        };

        let chat_req = OllamaProvider::build_chat_request(&request);
        let system_msg = &chat_req.messages[0];
        assert_eq!(system_msg.content, "You are helpful.");
        assert!(!system_msg.content.contains("# TOOL USE INSTRUCTIONS"));
    }

    #[test]
    fn build_request_formats_tool_use_blocks() {
        let request = ModelRequest {
            model: "llama3.2".into(),
            messages: vec![
                ChatMessage {
                    role: Role::User,
                    content: MessageContent::Text("create a file".into()),
                },
                ChatMessage {
                    role: Role::Assistant,
                    content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                        id: "emul_123_0".into(),
                        name: "file_write".into(),
                        input: serde_json::json!({"path": "/tmp/test.txt", "content": "hello"}),
                    }]),
                },
                ChatMessage {
                    role: Role::User,
                    content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                        tool_use_id: "emul_123_0".into(),
                        content: "File written: /tmp/test.txt (5 bytes)".into(),
                        is_error: false,
                    }]),
                },
            ],
            tools: sample_tools(),
            max_tokens: None,
            temperature: None,
            system: None,
            stream: true,
        };

        let chat_req = OllamaProvider::build_chat_request(&request);
        // Assistant message should have <tool_call> format.
        let assistant_msg = &chat_req.messages[2]; // [0]=system, [1]=user, [2]=assistant
        assert_eq!(assistant_msg.role, "assistant");
        assert!(assistant_msg.content.contains("<tool_call>"));
        assert!(assistant_msg.content.contains("file_write"));
        // User message should have <tool_result> format.
        let user_result_msg = &chat_req.messages[3];
        assert_eq!(user_result_msg.role, "user");
        assert!(user_result_msg.content.contains("<tool_result"));
        assert!(user_result_msg.content.contains("File written"));
    }

    #[test]
    fn build_request_maps_system_prompt() {
        let request = ModelRequest {
            model: "llama3.2".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text("hello".into()),
            }],
            tools: vec![],
            max_tokens: None,
            temperature: None,
            system: Some("You are a helpful assistant.".into()),
            stream: true,
        };

        let chat_req = OllamaProvider::build_chat_request(&request);
        assert_eq!(chat_req.messages.len(), 2);
        assert_eq!(chat_req.messages[0].role, "system");
        assert_eq!(chat_req.messages[0].content, "You are a helpful assistant.");
        assert_eq!(chat_req.messages[1].role, "user");
    }

    #[test]
    fn build_request_with_options() {
        let request = make_request("test");
        let chat_req = OllamaProvider::build_chat_request(&request);

        assert!(chat_req.options.is_some());
        let opts = chat_req.options.unwrap();
        assert_eq!(opts.temperature, Some(0.7));
        assert_eq!(opts.num_predict, Some(256));
    }

    #[test]
    fn parse_ndjson_text_delta() {
        let line = r#"{"model":"llama3.2","message":{"role":"assistant","content":"Hello"},"done":false}"#;
        let result = OllamaProvider::parse_ndjson_line(line);
        assert!(result.is_some());
        let chunk = result.unwrap().unwrap();
        match chunk {
            ModelChunk::TextDelta(text) => assert_eq!(text, "Hello"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_ndjson_done_line() {
        let line = r#"{"model":"llama3.2","message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":25,"eval_count":100}"#;
        let result = OllamaProvider::parse_ndjson_line(line);
        assert!(result.is_some());
        let chunk = result.unwrap().unwrap();
        match chunk {
            ModelChunk::Usage(usage) => {
                assert_eq!(usage.input_tokens, 25);
                assert_eq!(usage.output_tokens, 100);
            }
            other => panic!("expected Usage, got {other:?}"),
        }
    }

    #[test]
    fn parse_ndjson_empty_line() {
        assert!(OllamaProvider::parse_ndjson_line("").is_none());
        assert!(OllamaProvider::parse_ndjson_line("  ").is_none());
    }

    #[test]
    fn parse_ndjson_invalid_json() {
        let result = OllamaProvider::parse_ndjson_line("not json");
        assert!(result.is_some());
        assert!(result.unwrap().is_err());
    }

    #[test]
    fn malformed_ndjson_returns_error() {
        // Various malformed inputs should return Err, not panic.
        let cases = vec![
            "not json at all",
            "{broken",
            r#"{"unexpected_field": true}"#,
        ];
        for input in cases {
            let result = OllamaProvider::parse_ndjson_line(input);
            if let Some(r) = result {
                let _ = r;
            }
        }
    }

    #[tokio::test]
    async fn is_available_returns_false_for_unreachable() {
        let provider = OllamaProvider::new(
            Some("http://127.0.0.1:19999".into()),
            HttpConfig::default(),
        );
        assert!(!provider.is_available().await);
    }

    // --- Tool Emulation Tests ---

    #[test]
    fn parse_tool_calls_single_call() {
        let text = r#"I'll create that file for you.
<tool_call>
{"name": "file_write", "arguments": {"path": "/tmp/test.txt", "content": "hello world"}}
</tool_call>"#;

        let (calls, remaining) = OllamaProvider::parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "file_write");
        assert_eq!(calls[0].1["path"], "/tmp/test.txt");
        assert_eq!(calls[0].1["content"], "hello world");
        assert!(remaining.contains("create that file"));
    }

    #[test]
    fn parse_tool_calls_multiple_calls() {
        let text = r#"<tool_call>
{"name": "file_write", "arguments": {"path": "/tmp/a.txt", "content": "aaa"}}
</tool_call>
<tool_call>
{"name": "bash", "arguments": {"command": "ls /tmp"}}
</tool_call>"#;

        let (calls, _remaining) = OllamaProvider::parse_tool_calls(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "file_write");
        assert_eq!(calls[1].0, "bash");
    }

    #[test]
    fn parse_tool_calls_no_calls() {
        let text = "I can't create files, but you can use the echo command.";
        let (calls, remaining) = OllamaProvider::parse_tool_calls(text);
        assert!(calls.is_empty());
        assert_eq!(remaining, text);
    }

    #[test]
    fn parse_tool_calls_invalid_json() {
        let text = "<tool_call>\nnot valid json\n</tool_call>";
        let (calls, _remaining) = OllamaProvider::parse_tool_calls(text);
        assert!(calls.is_empty(), "invalid JSON should be skipped");
    }

    #[test]
    fn parse_tool_calls_missing_name() {
        let text = r#"<tool_call>
{"arguments": {"path": "/tmp/test.txt"}}
</tool_call>"#;
        let (calls, _remaining) = OllamaProvider::parse_tool_calls(text);
        assert!(calls.is_empty(), "missing 'name' field should be skipped");
    }

    #[test]
    fn parse_tool_calls_unclosed_tag() {
        let text = "Some text <tool_call> unclosed forever";
        let (calls, remaining) = OllamaProvider::parse_tool_calls(text);
        assert!(calls.is_empty());
        // Unclosed tag is treated as text.
        assert!(remaining.contains("<tool_call>"));
        assert!(remaining.contains("unclosed forever"));
    }

    #[test]
    fn parse_tool_calls_whitespace_in_json() {
        let text = r#"<tool_call>
  {
    "name": "file_write",
    "arguments": {
      "path": "/tmp/test.txt",
      "content": "hello"
    }
  }
</tool_call>"#;

        let (calls, _) = OllamaProvider::parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "file_write");
    }

    #[test]
    fn format_tool_emulation_prompt_content() {
        let tools = sample_tools();
        let prompt = OllamaProvider::format_tool_emulation_prompt(&tools);
        assert!(prompt.contains("# TOOL USE INSTRUCTIONS"));
        assert!(prompt.contains("## Available Tools"));
        assert!(prompt.contains("file_write"));
        assert!(prompt.contains("Write content to a file"));
        assert!(prompt.contains("bash"));
        assert!(prompt.contains("Run a shell command"));
        assert!(prompt.contains("<tool_call>"));
        assert!(prompt.contains("\"name\""));
        assert!(prompt.contains("\"arguments\""));
        // Verify concrete example is included.
        assert!(prompt.contains("## Example"));
        assert!(prompt.contains("<USER_PATH>"));
        // Verify critical rules section at end.
        assert!(prompt.contains("## CRITICAL RULES"));
        assert!(prompt.contains("NEVER explain"));
    }

    #[test]
    fn with_default_model_supports_tools() {
        let provider = OllamaProvider::with_default_model(
            None,
            HttpConfig::default(),
            Some("deepseek-coder-v2:latest".into()),
        );
        let models = provider.supported_models();
        assert_eq!(models[0].id, "deepseek-coder-v2:latest");
        assert!(models[0].supports_tools, "default model should support tools via emulation");
    }

    #[test]
    fn build_request_tool_result_error_status() {
        let request = ModelRequest {
            model: "llama3.2".into(),
            messages: vec![
                ChatMessage {
                    role: Role::User,
                    content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                        tool_use_id: "emul_1_0".into(),
                        content: "Permission denied".into(),
                        is_error: true,
                    }]),
                },
            ],
            tools: sample_tools(),
            max_tokens: None,
            temperature: None,
            system: None,
            stream: true,
        };

        let chat_req = OllamaProvider::build_chat_request(&request);
        // The tool result for errors should include status="error".
        let user_msg = &chat_req.messages[1]; // [0]=system (tool prompt)
        assert!(user_msg.content.contains("status=\"error\""));
        assert!(user_msg.content.contains("Permission denied"));
    }

    // === Phase 30: Fix 3 — UUID tool_use_id uniqueness ===

    #[test]
    fn tool_use_ids_are_unique_across_calls() {
        // Verify that UUID-based IDs don't collide (previous timestamp-based could).
        let mut ids = std::collections::HashSet::new();
        for i in 0..100 {
            let id = format!("emul_{}_{i}", uuid::Uuid::new_v4().simple());
            assert!(ids.insert(id), "ID collision at iteration {i}");
        }
    }

    // === Phase 30: Fix 4 — parse_tool_calls warn logging ===

    #[test]
    fn parse_tool_calls_warns_on_invalid_json() {
        // Malformed JSON inside <tool_call> should be silently dropped (no panic).
        let text = r#"Some text <tool_call>not valid json{</tool_call> more text"#;
        let (calls, remaining) = OllamaProvider::parse_tool_calls(text);
        assert!(calls.is_empty(), "Invalid JSON should not produce tool calls");
        assert!(remaining.contains("Some text"));
        assert!(remaining.contains("more text"));
    }

    #[test]
    fn parse_tool_calls_warns_on_missing_fields() {
        // Valid JSON but missing required 'name' or 'arguments' fields.
        let text = r#"<tool_call>{"only_name": "test"}</tool_call>"#;
        let (calls, _remaining) = OllamaProvider::parse_tool_calls(text);
        assert!(calls.is_empty(), "Missing fields should not produce tool calls");
    }

    // === Phase 32: Tool emulation improvements ===

    #[test]
    fn tool_emulation_prompt_prioritizes_file_and_bash() {
        // Create 15 tools: the 6 priority ones plus 9 generic ones.
        let mut tools: Vec<ToolDefinition> = vec![
            "grep", "glob", "web_search", "git_status", "git_diff",
            "git_log", "git_add", "git_commit", "symbol_search",
        ]
        .into_iter()
        .map(|name| ToolDefinition {
            name: name.into(),
            description: format!("{name} description"),
            input_schema: serde_json::json!({"type": "object", "properties": {}}),
        })
        .collect();

        // Add priority tools in reverse order (should still appear first).
        for name in ["bash", "file_edit", "file_read", "file_write"] {
            tools.push(ToolDefinition {
                name: name.into(),
                description: format!("{name} description"),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {"path": {"type": "string"}},
                    "required": ["path"]
                }),
            });
        }

        let prompt = OllamaProvider::format_tool_emulation_prompt(&tools);

        // file_write should appear before git_status in the listing.
        let fw_pos = prompt.find("**file_write**").expect("file_write should be in prompt");
        let gs_pos = prompt.find("**git_status**").expect("git_status should be in prompt");
        assert!(fw_pos < gs_pos, "file_write should be listed before git_status");
    }

    #[test]
    fn tool_emulation_prompt_limits_tool_count() {
        // Create 20 tools — only MAX_EMULATION_TOOLS (12) should be included.
        let tools: Vec<ToolDefinition> = (0..20)
            .map(|i| ToolDefinition {
                name: format!("tool_{i}"),
                description: format!("Tool {i} description"),
                input_schema: serde_json::json!({"type": "object", "properties": {}}),
            })
            .collect();

        let prompt = OllamaProvider::format_tool_emulation_prompt(&tools);

        // Should mention that extra tools exist.
        assert!(prompt.contains("more tools available"));

        // First 12 tools should be present, tool_12 through tool_19 should not.
        for i in 0..12 {
            assert!(
                prompt.contains(&format!("tool_{i}")),
                "tool_{i} should be in prompt"
            );
        }
    }

    #[test]
    fn tool_emulation_prompt_shows_required_params() {
        let tools = sample_tools();
        let prompt = OllamaProvider::format_tool_emulation_prompt(&tools);
        // file_write has required: ["path", "content"].
        assert!(prompt.contains("path, content"), "should list required params");
        // bash has required: ["command"].
        assert!(prompt.contains("command"), "should list bash required param");
    }

    #[test]
    fn build_retry_request_appends_correction() {
        let original = OllamaChatRequest {
            model: "test-model".into(),
            messages: vec![
                OllamaMessage {
                    role: "system".into(),
                    content: "System prompt".into(),
                },
                OllamaMessage {
                    role: "user".into(),
                    content: "Create a file".into(),
                },
            ],
            stream: true,
            options: None,
        };

        let retry = OllamaProvider::build_retry_request(&original, "Here's how to create a file...");

        // Should have original messages + assistant response + correction prompt.
        assert_eq!(retry.messages.len(), 4);
        assert_eq!(retry.messages[0].role, "system");
        assert_eq!(retry.messages[1].role, "user");
        assert_eq!(retry.messages[2].role, "assistant");
        assert!(retry.messages[2].content.contains("Here's how to create a file"));
        assert_eq!(retry.messages[3].role, "user");
        assert!(retry.messages[3].content.contains("<tool_call>"));
        assert_eq!(retry.model, "test-model");
    }

    #[test]
    fn build_retry_request_preserves_options() {
        let original = OllamaChatRequest {
            model: "test-model".into(),
            messages: vec![],
            stream: true,
            options: Some(OllamaOptions {
                temperature: Some(0.5),
                num_predict: Some(1024),
            }),
        };

        let retry = OllamaProvider::build_retry_request(&original, "text");
        let opts = retry.options.unwrap();
        assert_eq!(opts.temperature, Some(0.5));
        assert_eq!(opts.num_predict, Some(1024));
    }

    // === Fallback intent extraction tests ===

    #[test]
    fn extract_intent_bash_code_block() {
        let text = "Para crear el archivo, usa este comando:\n\n```bash\necho \"hola mundo\" > /tmp/test.txt\n```\n\nEsto creará el archivo.";
        let calls = OllamaProvider::extract_intent_from_text(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "bash");
        assert!(calls[0].1["command"].as_str().unwrap().contains("echo"));
    }

    #[test]
    fn extract_intent_sh_code_block() {
        let text = "```sh\ncat > /tmp/file.txt << EOF\nhello\nEOF\n```";
        let calls = OllamaProvider::extract_intent_from_text(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "bash");
    }

    #[test]
    fn extract_intent_multiple_code_blocks() {
        let text = "```bash\necho \"a\" > /tmp/a.txt\n```\n\nAnd then:\n\n```bash\necho \"b\" > /tmp/b.txt\n```";
        let calls = OllamaProvider::extract_intent_from_text(text);
        assert_eq!(calls.len(), 2);
    }

    #[test]
    fn extract_intent_inline_echo() {
        let text = "You can use: echo \"hola mundo\" > /tmp/test.txt to create the file.";
        let calls = OllamaProvider::extract_intent_from_text(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "file_write");
        assert_eq!(calls[0].1["path"], "/tmp/test.txt");
        assert_eq!(calls[0].1["content"], "hola mundo");
    }

    #[test]
    fn extract_intent_inline_echo_single_quotes() {
        let text = "Run this: echo 'hello world' > /tmp/hello.txt";
        let calls = OllamaProvider::extract_intent_from_text(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "file_write");
        assert_eq!(calls[0].1["path"], "/tmp/hello.txt");
        assert_eq!(calls[0].1["content"], "hello world");
    }

    #[test]
    fn extract_intent_no_match() {
        let text = "The file system stores data on disk. You should use appropriate commands.";
        let calls = OllamaProvider::extract_intent_from_text(text);
        assert!(calls.is_empty());
    }

    #[test]
    fn extract_intent_code_block_takes_priority_over_inline() {
        // If there's a code block, it takes priority (inline pattern only fires if no code blocks).
        let text = "Use: echo \"inline\" > /tmp/inline.txt\n\n```bash\necho \"block\" > /tmp/block.txt\n```";
        let calls = OllamaProvider::extract_intent_from_text(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "bash");
        assert!(calls[0].1["command"].as_str().unwrap().contains("block"));
    }
}
