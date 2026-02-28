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
}
