//! FASE 7 — Model Quirks Framework
//!
//! Replaces hardcoded provider-specific workarounds with a composable trait-based registry.
//! Each `ModelQuirk` matches against (provider, model) and can:
//! - Inject post-delegation text (e.g., anti-redo directives)
//! - Filter synthesis text (e.g., strip XML tool artifacts)

use std::borrow::Cow;

/// Context passed to quirk methods so they can make provider-aware decisions.
pub struct QuirkContext<'a> {
    /// Provider name (e.g., "deepseek", "anthropic").
    pub provider_name: &'a str,
    /// Model ID (e.g., "deepseek-chat", "claude-sonnet-4-6").
    pub model: &'a str,
    /// Tool names that were successfully executed by sub-agents this round.
    pub delegated_ok_tools: &'a [String],
    /// Whether tools were sent in the current round request.
    pub has_tools_in_request: bool,
}

/// A composable model-specific workaround.
///
/// Quirks are stateless and matched against (provider, model). Multiple quirks
/// can match the same provider — the registry composes their effects.
pub trait ModelQuirk: Send + Sync {
    /// Human-readable name for logging.
    fn name(&self) -> &str;

    /// Whether this quirk should activate for the given provider/model pair.
    fn matches(&self, provider: &str, model: &str) -> bool;

    /// Optional text to inject after sub-agent delegation results.
    ///
    /// Returns `None` to skip injection. Called by the coordinator after
    /// collecting sub-agent outputs, before the synthesis round.
    fn post_delegation_injection(&self, ctx: &QuirkContext<'_>) -> Option<String> {
        let _ = ctx;
        None
    }

    /// Optional filter applied to synthesis text before it reaches the user.
    ///
    /// Returns `Cow::Borrowed(text)` when no filtering is needed (zero-alloc fast path).
    fn filter_synthesis_text<'a>(&self, text: &'a str, ctx: &QuirkContext<'_>) -> Cow<'a, str> {
        let _ = ctx;
        Cow::Borrowed(text)
    }
}

/// Registry that composes multiple quirks.
pub struct QuirkRegistry {
    quirks: Vec<Box<dyn ModelQuirk>>,
}

impl QuirkRegistry {
    pub fn new() -> Self {
        Self { quirks: Vec::new() }
    }

    pub fn register(&mut self, quirk: Box<dyn ModelQuirk>) {
        self.quirks.push(quirk);
    }

    /// Collect all post-delegation injection texts for matching quirks.
    pub fn post_delegation_injections(&self, ctx: &QuirkContext<'_>) -> Vec<String> {
        self.quirks
            .iter()
            .filter(|q| q.matches(ctx.provider_name, ctx.model))
            .filter_map(|q| q.post_delegation_injection(ctx))
            .collect()
    }

    /// Apply all matching quirk filters to synthesis text, chaining them.
    pub fn filter_synthesis_text<'a>(&self, text: &'a str, ctx: &QuirkContext<'_>) -> Cow<'a, str> {
        let mut result: Cow<'a, str> = Cow::Borrowed(text);
        for quirk in &self.quirks {
            if quirk.matches(ctx.provider_name, ctx.model) {
                // Need to convert to owned if the quirk wants to modify
                let filtered = quirk.filter_synthesis_text(&result, ctx);
                if let Cow::Owned(owned) = filtered {
                    result = Cow::Owned(owned);
                }
            }
        }
        result
    }
}

// ── Built-in Quirks ──────────────────────────────────────────────────────────

/// Prevents non-Anthropic models from re-executing destructive tools that
/// sub-agents already completed (file_write, bash, shell, patch_apply).
///
/// Root cause: deepseek-chat hallucinates second file_write calls containing
/// the full file content even after sub-agents already wrote the file.
pub struct AntiRedoQuirk;

impl ModelQuirk for AntiRedoQuirk {
    fn name(&self) -> &str {
        "anti_redo"
    }

    fn matches(&self, provider: &str, _model: &str) -> bool {
        // Applies to non-Anthropic providers (deepseek, ollama, etc.)
        !matches!(provider, "anthropic" | "claude_code")
    }

    fn post_delegation_injection(&self, ctx: &QuirkContext<'_>) -> Option<String> {
        let destructive_tools: Vec<&String> = ctx
            .delegated_ok_tools
            .iter()
            .filter(|t| matches!(t.as_str(), "file_write" | "bash" | "shell" | "patch_apply"))
            .collect();

        if destructive_tools.is_empty() {
            return None;
        }

        let tool_list: Vec<&str> = destructive_tools.iter().map(|s| s.as_str()).collect();
        Some(format!(
            "\n⚠️  CRITICAL: The following tools were already executed by sub-agents \
             and must NOT be called again: [{}]. \
             Your ONLY task now is to synthesize the results and confirm to the user \
             what was created. Do NOT regenerate or re-write any files.\n",
            tool_list.join(", ")
        ))
    }
}

/// Strips XML tool-call artifacts (`<function_calls>`, `<invoke>`, `<halcon::tool_call>`)
/// from synthesis text when the model was not provided tools in the request.
///
/// Root cause: non-Anthropic models occasionally hallucinate XML tool syntax in
/// synthesis rounds even when tools=[] in the request.
pub struct XmlArtifactFilterQuirk;

impl ModelQuirk for XmlArtifactFilterQuirk {
    fn name(&self) -> &str {
        "xml_artifact_filter"
    }

    fn matches(&self, _provider: &str, _model: &str) -> bool {
        // Applies to all providers — even Anthropic can produce stale XML artifacts
        // in synthesis rounds when tools were previously available but withdrawn.
        true
    }

    fn filter_synthesis_text<'a>(&self, text: &'a str, ctx: &QuirkContext<'_>) -> Cow<'a, str> {
        // Only filter when tools are NOT in the request (synthesis round).
        if ctx.has_tools_in_request {
            return Cow::Borrowed(text);
        }
        strip_tool_xml_artifacts(text)
    }
}

/// Strip XML tool-call artifacts from synthesis round text.
///
/// Moved from `provider_round.rs` to be shared by the quirks framework.
/// Returns the sanitized text. If no artifacts are found, returns the input
/// unchanged without any allocation.
pub fn strip_tool_xml_artifacts(text: &str) -> Cow<'_, str> {
    // Fast path: avoid allocation when no XML markers present.
    if !text.contains("<function_calls>")
        && !text.contains("<invoke ")
        && !text.contains("<halcon::tool_call>")
    {
        return Cow::Borrowed(text);
    }

    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    let mut stripped = false;

    const PATTERNS: &[(&str, &str)] = &[
        ("<function_calls>", "</function_calls>"),
        ("<halcon::tool_call>", "</halcon::tool_call>"),
        ("<invoke ", "</invoke>"),
    ];

    'outer: loop {
        for (open, close) in PATTERNS {
            if let Some(start) = rest.find(open) {
                result.push_str(&rest[..start]);
                rest = &rest[start..];
                if let Some(end_rel) = rest.find(close) {
                    let end_abs = end_rel + close.len();
                    rest = &rest[end_abs..];
                    stripped = true;
                    continue 'outer;
                } else {
                    stripped = true;
                    rest = "";
                    break 'outer;
                }
            }
        }
        result.push_str(rest);
        break;
    }

    if stripped {
        let trimmed = result.trim_matches('\n');
        Cow::Owned(trimmed.to_string())
    } else {
        Cow::Borrowed(text)
    }
}

/// Returns `true` if the text contains XML tool-call artifacts.
pub fn contains_tool_xml_artifacts(text: &str) -> bool {
    text.contains("<function_calls>")
        || text.contains("<invoke ")
        || text.contains("<halcon::tool_call>")
}

// ── Alternative Tool-Call Format Recovery ──────────────────────────────────

/// A recovered tool call extracted from alternative text-based formats.
///
/// Some providers emit tool calls as XML-like text instead of structured
/// `ToolUseStart`/`ToolUseDelta` chunks. This struct represents a tool call
/// recovered from such text so it can be injected into the normal execution
/// pipeline as if it were a native tool call.
#[derive(Debug, Clone)]
pub struct RecoveredToolCall {
    pub name: String,
    pub input: serde_json::Value,
}

/// Marker for the format that was detected in text.
const DSML_MARKER: &str = "\u{ff5c}DSML\u{ff5c}";

/// Attempt to extract tool calls from alternative text-based formats.
///
/// Currently recognises:
/// - **DSML format**: `<｜DSML｜function_calls>...<｜DSML｜invoke name="...">...`
///
/// Returns `None` if no alternative format is detected, allowing the caller
/// to proceed with the normal text-only path. Returns `Some(vec)` with the
/// recovered tool calls if a known format was found.
///
/// This is **provider-agnostic**: any model that emits these patterns gets
/// the same treatment. P1-B validation still applies to the recovered calls.
pub fn try_recover_tool_calls_from_text(text: &str) -> Option<Vec<RecoveredToolCall>> {
    // Fast path: check for DSML marker (U+FF5C = fullwidth vertical bar ｜)
    if !text.contains(DSML_MARKER) {
        return None;
    }

    let mut calls = Vec::new();
    let mut rest = text;

    // Pattern: <｜DSML｜invoke name="TOOL_NAME">
    //          <｜DSML｜parameter name="PARAM" ...>VALUE<｜DSML｜/parameter>
    //          ...
    //          </｜DSML｜invoke>
    let invoke_open = format!("<{DSML_MARKER}invoke name=\"");
    let invoke_close = format!("</{DSML_MARKER}invoke>");
    let param_open = format!("<{DSML_MARKER}parameter name=\"");
    let param_close_prefix = format!("</{DSML_MARKER}parameter>");
    // Also handle self-closing: <｜DSML｜parameter>
    let param_close_alt = format!("{DSML_MARKER}parameter>");

    while let Some(invoke_start) = rest.find(&invoke_open) {
        let after_open = &rest[invoke_start + invoke_open.len()..];
        // Extract tool name (until closing quote)
        let name_end = match after_open.find('"') {
            Some(i) => i,
            None => break,
        };
        let tool_name = &after_open[..name_end];

        // Find the end of this invoke block
        let invoke_body_start = invoke_start + invoke_open.len() + name_end;
        let remaining = &rest[invoke_body_start..];
        let invoke_end = remaining.find(&invoke_close).unwrap_or(remaining.len());
        let invoke_body = &remaining[..invoke_end];

        // Extract parameters from the invoke body
        let mut params = serde_json::Map::new();
        let mut param_rest = invoke_body;
        while let Some(param_start) = param_rest.find(&param_open) {
            let after_param = &param_rest[param_start + param_open.len()..];
            let pname_end = match after_param.find('"') {
                Some(i) => i,
                None => break,
            };
            let param_name = &after_param[..pname_end];

            // Find the > that closes the opening tag (skip attributes like string="true")
            let tag_close = match after_param[pname_end..].find('>') {
                Some(i) => pname_end + i + 1,
                None => break,
            };

            let value_start = &after_param[tag_close..];
            // Value ends at the closing tag
            let value_end = value_start
                .find(&param_close_prefix)
                .or_else(|| value_start.find(&param_close_alt))
                .unwrap_or(value_start.len());
            let param_value = value_start[..value_end].trim();

            params.insert(
                param_name.to_string(),
                serde_json::Value::String(param_value.to_string()),
            );

            param_rest = &value_start[value_end..];
        }

        calls.push(RecoveredToolCall {
            name: tool_name.to_string(),
            input: serde_json::Value::Object(params),
        });

        // Advance past this invoke block
        rest = &rest[invoke_body_start + invoke_end + invoke_close.len().min(remaining.len() - invoke_end)..];
    }

    if calls.is_empty() {
        None
    } else {
        tracing::info!(
            count = calls.len(),
            tools = ?calls.iter().map(|c| &c.name).collect::<Vec<_>>(),
            "DSML format recovery: extracted tool calls from alternative text format"
        );
        Some(calls)
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(provider: &'a str, model: &'a str, tools: &'a [String], has_tools: bool) -> QuirkContext<'a> {
        QuirkContext {
            provider_name: provider,
            model,
            delegated_ok_tools: tools,
            has_tools_in_request: has_tools,
        }
    }

    #[test]
    fn anti_redo_matches_deepseek() {
        let quirk = AntiRedoQuirk;
        assert!(quirk.matches("deepseek", "deepseek-chat"));
        assert!(quirk.matches("ollama", "llama3.2"));
        assert!(quirk.matches("openai", "gpt-4o"));
    }

    #[test]
    fn anti_redo_no_match_anthropic() {
        let quirk = AntiRedoQuirk;
        assert!(!quirk.matches("anthropic", "claude-sonnet-4-6"));
        assert!(!quirk.matches("claude_code", "claude-sonnet-4-6"));
    }

    #[test]
    fn anti_redo_injects_when_destructive_tools() {
        let quirk = AntiRedoQuirk;
        let tools = vec!["file_write".to_string(), "grep".to_string(), "bash".to_string()];
        let c = ctx("deepseek", "deepseek-chat", &tools, false);
        let injection = quirk.post_delegation_injection(&c);
        assert!(injection.is_some());
        let text = injection.unwrap();
        assert!(text.contains("file_write"));
        assert!(text.contains("bash"));
        assert!(text.contains("must NOT be called again"));
    }

    #[test]
    fn anti_redo_no_injection_without_destructive_tools() {
        let quirk = AntiRedoQuirk;
        let tools = vec!["grep".to_string(), "read_file".to_string()];
        let c = ctx("deepseek", "deepseek-chat", &tools, false);
        assert!(quirk.post_delegation_injection(&c).is_none());
    }

    #[test]
    fn xml_filter_strips_function_calls() {
        let quirk = XmlArtifactFilterQuirk;
        let text = "Hello\n<function_calls>\n<invoke name=\"test\">\n</invoke>\n</function_calls>\nWorld";
        let c = ctx("deepseek", "deepseek-chat", &[], false);
        let filtered = quirk.filter_synthesis_text(text, &c);
        assert!(!filtered.contains("<function_calls>"));
        assert!(filtered.contains("Hello"));
        assert!(filtered.contains("World"));
    }

    #[test]
    fn xml_filter_passthrough_when_tools_present() {
        let quirk = XmlArtifactFilterQuirk;
        let text = "Hello <function_calls>test</function_calls>";
        let c = ctx("deepseek", "deepseek-chat", &[], true);
        let filtered = quirk.filter_synthesis_text(text, &c);
        // Should NOT filter because tools are present in the request
        assert!(filtered.contains("<function_calls>"));
    }

    #[test]
    fn registry_composes_multiple_quirks() {
        let mut registry = QuirkRegistry::new();
        registry.register(Box::new(AntiRedoQuirk));
        registry.register(Box::new(XmlArtifactFilterQuirk));

        let tools = vec!["file_write".to_string()];
        let c = ctx("deepseek", "deepseek-chat", &tools, false);

        // AntiRedoQuirk matches deepseek → should produce injection
        let injections = registry.post_delegation_injections(&c);
        assert_eq!(injections.len(), 1);
        assert!(injections[0].contains("file_write"));

        // XmlArtifactFilterQuirk matches all + has_tools=false → should filter
        let text = "Result: <function_calls>bad</function_calls> done";
        let filtered = registry.filter_synthesis_text(text, &c);
        assert!(!filtered.contains("<function_calls>"));
    }

    #[test]
    fn strip_tool_xml_artifacts_no_artifacts_zero_alloc() {
        let text = "Hello world, this is clean text";
        let result = strip_tool_xml_artifacts(text);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, text);
    }

    #[test]
    fn strip_tool_xml_artifacts_strips_invoke() {
        let text = "Before <invoke name=\"test\">\n<parameter>value</parameter>\n</invoke> After";
        let result = strip_tool_xml_artifacts(text);
        assert!(matches!(result, Cow::Owned(_)));
        assert!(!result.contains("<invoke"));
        assert!(result.contains("Before"));
        assert!(result.contains("After"));
    }

    #[test]
    fn contains_tool_xml_artifacts_detection() {
        assert!(contains_tool_xml_artifacts("<function_calls>test</function_calls>"));
        assert!(contains_tool_xml_artifacts("<invoke name=\"x\">"));
        assert!(contains_tool_xml_artifacts("<halcon::tool_call>x</halcon::tool_call>"));
        assert!(!contains_tool_xml_artifacts("clean text without xml"));
    }

    #[test]
    fn dsml_recovery_extracts_single_tool_call() {
        let text = "I need to list files.\n\
            <\u{ff5c}DSML\u{ff5c}function_calls>\n\
            <\u{ff5c}DSML\u{ff5c}invoke name=\"list_files\">\n\
            <\u{ff5c}DSML\u{ff5c}parameter name=\"path\" string=\"true\">.</\u{ff5c}DSML\u{ff5c}parameter>\n\
            </\u{ff5c}DSML\u{ff5c}invoke>\n\
            </\u{ff5c}DSML\u{ff5c}function_calls>";

        let result = try_recover_tool_calls_from_text(text);
        assert!(result.is_some());
        let calls = result.unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "list_files");
        assert_eq!(calls[0].input["path"], ".");
    }

    #[test]
    fn dsml_recovery_extracts_multiple_parameters() {
        let text = "<\u{ff5c}DSML\u{ff5c}function_calls>\n\
            <\u{ff5c}DSML\u{ff5c}invoke name=\"file_read\">\n\
            <\u{ff5c}DSML\u{ff5c}parameter name=\"path\">src/main.rs</\u{ff5c}DSML\u{ff5c}parameter>\n\
            <\u{ff5c}DSML\u{ff5c}parameter name=\"lines\">100</\u{ff5c}DSML\u{ff5c}parameter>\n\
            </\u{ff5c}DSML\u{ff5c}invoke>\n\
            </\u{ff5c}DSML\u{ff5c}function_calls>";

        let calls = try_recover_tool_calls_from_text(text).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "file_read");
        assert_eq!(calls[0].input["path"], "src/main.rs");
        assert_eq!(calls[0].input["lines"], "100");
    }

    #[test]
    fn dsml_recovery_returns_none_for_clean_text() {
        assert!(try_recover_tool_calls_from_text("Hello world, no DSML here").is_none());
        assert!(try_recover_tool_calls_from_text("").is_none());
    }

    #[test]
    fn dsml_recovery_returns_none_for_standard_xml() {
        // Standard halcon XML should NOT be recovered as DSML
        let text = "<function_calls><invoke name=\"test\"></invoke></function_calls>";
        assert!(try_recover_tool_calls_from_text(text).is_none());
    }

    // ── RP-1: DSML arg coercion tests ────────────────────────────────────────

    /// Verify that a DSML-recovered `paths` arg arrives as a string (the raw parse output).
    /// The actual coercion (string → array) happens in provider_round.rs after recovery,
    /// using the tool's input_schema. This test confirms the raw parse value for regression tracking.
    #[test]
    fn rp1_dsml_paths_arg_recovered_as_string() {
        let text = "<\u{ff5c}DSML\u{ff5c}function_calls>\n\
            <\u{ff5c}DSML\u{ff5c}invoke name=\"file_read\">\n\
            <\u{ff5c}DSML\u{ff5c}parameter name=\"paths\">/some/path</\u{ff5c}DSML\u{ff5c}parameter>\n\
            </\u{ff5c}DSML\u{ff5c}invoke>\n\
            </\u{ff5c}DSML\u{ff5c}function_calls>";
        let calls = try_recover_tool_calls_from_text(text).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "file_read");
        // Raw DSML parse: paths arrives as a string — coercion to array happens downstream (RP-1 fix).
        assert!(calls[0].input["paths"].is_string(), "raw DSML parse returns string for paths");
        assert_eq!(calls[0].input["paths"], "/some/path");
    }

    /// Simulate the RP-1 coercion logic: verify string→array transformation.
    #[test]
    fn rp1_string_to_array_coercion_logic() {
        // Simulate the coercion that happens in provider_round.rs after DSML recovery.
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "paths": {"type": "array", "items": {"type": "string"}},
                "command": {"type": "string"}
            }
        });
        let mut args = serde_json::json!({
            "paths": "/some/path",   // string — should be coerced
            "command": "ls"          // string — correct type, no coercion needed
        });

        // Apply the RP-1 coercion logic.
        if let Some(props_map) = schema.get("properties").and_then(|p| p.as_object()) {
            if let serde_json::Value::Object(ref mut arg_map) = args {
                let to_coerce: Vec<String> = props_map
                    .iter()
                    .filter(|(prop_name, prop_schema)| {
                        let expects_array = prop_schema
                            .get("type")
                            .and_then(|t| t.as_str())
                            .map_or(false, |t| t == "array");
                        expects_array && arg_map.get(prop_name.as_str()).map_or(false, |v| v.is_string())
                    })
                    .map(|(k, _)| k.clone())
                    .collect();
                for prop_name in to_coerce {
                    if let Some(val) = arg_map.get(&prop_name).cloned() {
                        arg_map.insert(prop_name, serde_json::Value::Array(vec![val]));
                    }
                }
            }
        }

        // paths should now be an array.
        assert!(args["paths"].is_array(), "paths should be coerced to array");
        assert_eq!(args["paths"][0], "/some/path");
        // command should remain a string (no coercion for non-array schema).
        assert!(args["command"].is_string(), "command should remain a string");
        assert_eq!(args["command"], "ls");
    }

    /// Verify coercion works for multiple array-type args in a single call.
    #[test]
    fn rp1_multiple_array_args_all_coerced() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "paths": {"type": "array"},
                "patterns": {"type": "array"},
                "flags": {"type": "string"}
            }
        });
        let mut args = serde_json::json!({
            "paths": "/dir",
            "patterns": "*.rs",
            "flags": "-r"
        });
        if let Some(props_map) = schema.get("properties").and_then(|p| p.as_object()) {
            if let serde_json::Value::Object(ref mut arg_map) = args {
                let to_coerce: Vec<String> = props_map
                    .iter()
                    .filter(|(prop_name, prop_schema)| {
                        let expects_array = prop_schema
                            .get("type").and_then(|t| t.as_str()).map_or(false, |t| t == "array");
                        expects_array && arg_map.get(prop_name.as_str()).map_or(false, |v| v.is_string())
                    })
                    .map(|(k, _)| k.clone())
                    .collect();
                for prop_name in to_coerce {
                    if let Some(val) = arg_map.get(&prop_name).cloned() {
                        arg_map.insert(prop_name, serde_json::Value::Array(vec![val]));
                    }
                }
            }
        }
        assert!(args["paths"].is_array());
        assert!(args["patterns"].is_array());
        assert!(args["flags"].is_string(), "non-array args must not be coerced");
    }

    /// Already-array arg should NOT be double-wrapped.
    #[test]
    fn rp1_already_array_not_double_wrapped() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {"paths": {"type": "array"}}
        });
        let mut args = serde_json::json!({"paths": ["/dir1", "/dir2"]});
        if let Some(props_map) = schema.get("properties").and_then(|p| p.as_object()) {
            if let serde_json::Value::Object(ref mut arg_map) = args {
                let to_coerce: Vec<String> = props_map
                    .iter()
                    .filter(|(prop_name, prop_schema)| {
                        let expects_array = prop_schema
                            .get("type").and_then(|t| t.as_str()).map_or(false, |t| t == "array");
                        expects_array && arg_map.get(prop_name.as_str()).map_or(false, |v| v.is_string())
                    })
                    .map(|(k, _)| k.clone())
                    .collect();
                for prop_name in to_coerce {
                    if let Some(val) = arg_map.get(&prop_name).cloned() {
                        arg_map.insert(prop_name, serde_json::Value::Array(vec![val]));
                    }
                }
            }
        }
        // The array ["/dir1", "/dir2"] should remain unchanged (filter excludes non-strings).
        assert_eq!(args["paths"].as_array().unwrap().len(), 2);
        assert_eq!(args["paths"][0], "/dir1");
    }
}
