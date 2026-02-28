//! Sprint 2 — ProviderNormalizationAdapter
//!
//! Canonical, per-provider description of how `ToolDefinition`s from halcon-core are
//! transformed before reaching a provider's HTTP wire format.  The actual byte-level
//! transformation is still performed inside each provider crate (Anthropic, openai_compat,
//! Gemini, Ollama); this module provides:
//!
//! * A **typed enum** (`ProviderToolFormat`) that names every distinct wire format.
//! * A **validation layer** that checks tool schemas for format-specific incompatibilities.
//! * **Structured tracing** so every round logs which format will be used.
//!
//! # Wire formats in use
//!
//! | Provider          | Field name          | Wrapper struct          | Notes                      |
//! |-------------------|---------------------|-------------------------|----------------------------|
//! | Anthropic         | `input_schema`      | none                    | direct passthrough         |
//! | OpenAI / DeepSeek | `parameters`        | `{ type, function: … }` | renamed field              |
//! | Gemini            | `parameters`        | `{ functionDeclarations: […] }` | all tools in one wrapper |
//! | Ollama            | N/A (system prompt) | N/A                     | XML `<tool_call>` emulation |

use halcon_core::types::{ToolDefinition, ToolFormat};

// ── ProviderToolFormat ────────────────────────────────────────────────────────

/// Backward-compatible alias — all logic now lives in `halcon_core::types::ToolFormat`.
pub(crate) type ProviderToolFormat = ToolFormat;

/// String-based detection for backward compat with callers that don't have a
/// `&dyn ModelProvider` reference. Prefer `provider.tool_format()` when available.
pub(crate) fn detect_tool_format(provider_name: &str) -> ToolFormat {
    match provider_name {
        "anthropic" | "claude_code" => ToolFormat::AnthropicInputSchema,
        "openai" | "deepseek" => ToolFormat::OpenAIFunctionObject,
        "gemini" => ToolFormat::GeminiFunctionDeclarations,
        "ollama" => ToolFormat::OllamaXmlEmulation,
        _ => ToolFormat::Unknown,
    }
}

// ── NormalizationWarning ──────────────────────────────────────────────────────

/// A non-fatal compatibility issue detected between a tool's schema and the target
/// provider format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NormalizationWarning {
    /// The schema uses a JSON type (`"array"`, `"number"`, etc.) that is not explicitly
    /// listed as supported in the official provider documentation for this format.
    UnsupportedSchemaType {
        tool: String,
        schema_type: String,
        format: ProviderToolFormat,
    },

    /// Tool definitions are sent via system-prompt XML emulation rather than the provider's
    /// native tool_use protocol.  Output formatting depends on the model's instruction
    /// following quality.
    OllamaEmulationMode { tool_count: usize },

    /// The tool has no `"type"` key in its `input_schema`, which violates the minimum
    /// structural requirement expected by this format.
    MissingTypeField { tool: String },

    /// A field listed in `"required"` is absent from `"properties"`.  This would be caught
    /// by `schema_validator::preflight_validate` but is included here for defence-in-depth.
    RequiredFieldMissing { tool: String, field: String },
}

impl NormalizationWarning {
    /// Terse description for tracing.
    pub(crate) fn message(&self) -> String {
        match self {
            Self::UnsupportedSchemaType {
                tool,
                schema_type,
                format,
            } => {
                format!(
                    "tool '{}' uses schema type '{}' which may not be supported by {} format",
                    tool,
                    schema_type,
                    format.label()
                )
            }
            Self::OllamaEmulationMode { tool_count } => {
                format!(
                    "{} tool(s) will be sent via Ollama XML emulation — model output quality may vary",
                    tool_count
                )
            }
            Self::MissingTypeField { tool } => {
                format!("tool '{}' input_schema is missing the required 'type' field", tool)
            }
            Self::RequiredFieldMissing { tool, field } => {
                format!(
                    "tool '{}' lists '{}' as required but it is absent from 'properties'",
                    tool, field
                )
            }
        }
    }
}

// ── NormalizationResult ───────────────────────────────────────────────────────

/// Summary produced by [`ProviderNormalizationAdapter::validate`].
#[derive(Debug)]
pub(crate) struct NormalizationResult {
    /// Wire format that will be used for this provider.
    pub format: ProviderToolFormat,
    /// Number of tools that passed validation.
    pub tool_count: usize,
    /// Non-fatal warnings accumulated during validation.
    pub warnings: Vec<NormalizationWarning>,
}

impl NormalizationResult {
    /// `true` when there are zero warnings.
    pub(crate) fn is_clean(&self) -> bool {
        self.warnings.is_empty()
    }
}

// ── ProviderNormalizationAdapter ──────────────────────────────────────────────

/// Validates and describes tool normalization for a specific provider wire format.
///
/// The adapter does **not** perform byte-level transformation — that remains inside each
/// provider crate.  Its responsibilities are:
///
/// 1. **Classification** — identify the wire format from the provider name.
/// 2. **Validation** — produce [`NormalizationWarning`]s for schema incompatibilities.
/// 3. **Structured tracing** — emit a single `DEBUG` span per round so distributed traces
///    show which format was used.
///
/// # Usage
///
/// ```rust,ignore
/// let adapter = ProviderNormalizationAdapter::for_provider(effective_provider.name());
/// let result  = adapter.validate(&round_request.tools);
/// adapter.trace_result(&result, round);
/// ```
pub(crate) struct ProviderNormalizationAdapter {
    format: ProviderToolFormat,
    provider_name: String,
}

impl ProviderNormalizationAdapter {
    /// Build an adapter for the given provider `name()`.
    pub(crate) fn for_provider(provider_name: &str) -> Self {
        Self {
            format: detect_tool_format(provider_name),
            provider_name: provider_name.to_owned(),
        }
    }

    /// The detected wire format.
    pub(crate) fn format(&self) -> ProviderToolFormat {
        self.format
    }

    /// Validate `tools` for compatibility with this adapter's wire format.
    ///
    /// Validation is **O(n × fields)** — no I/O, no LLM calls.  All results are returned as
    /// structured [`NormalizationWarning`]s rather than hard errors; callers decide how to
    /// surface them.
    pub(crate) fn validate(&self, tools: &[ToolDefinition]) -> NormalizationResult {
        let mut warnings: Vec<NormalizationWarning> = Vec::new();

        // Special-case: Ollama uses XML emulation — warn once about quality variability.
        if self.format.uses_system_prompt_injection() && !tools.is_empty() {
            warnings.push(NormalizationWarning::OllamaEmulationMode {
                tool_count: tools.len(),
            });
        }

        for tool in tools {
            let schema = &tool.input_schema;

            // Rule 1: schema must be an object.
            let Some(obj) = schema.as_object() else {
                // Non-object schemas are already caught by preflight_validate; skip here
                // to avoid duplicate noise.
                continue;
            };

            // Rule 2: must have a "type" key.
            if !obj.contains_key("type") {
                warnings.push(NormalizationWarning::MissingTypeField {
                    tool: tool.name.clone(),
                });
            }

            // Rule 3: required fields must be in properties.
            if let (Some(required), Some(properties)) = (
                obj.get("required").and_then(|r| r.as_array()),
                obj.get("properties").and_then(|p| p.as_object()),
            ) {
                for item in required {
                    if let Some(field_name) = item.as_str() {
                        if !properties.contains_key(field_name) {
                            warnings.push(NormalizationWarning::RequiredFieldMissing {
                                tool: tool.name.clone(),
                                field: field_name.to_owned(),
                            });
                        }
                    }
                }
            }

            // Rule 4: Gemini has historically struggled with nested `$defs` / `$ref`.
            // Emit a warning when the schema contains reference keywords.
            if self.format == ProviderToolFormat::GeminiFunctionDeclarations {
                if obj.contains_key("$ref") || obj.contains_key("$defs") {
                    warnings.push(NormalizationWarning::UnsupportedSchemaType {
                        tool: tool.name.clone(),
                        schema_type: "$ref/$defs (JSON Schema reference)".to_owned(),
                        format: self.format,
                    });
                }
            }
        }

        NormalizationResult {
            format: self.format,
            tool_count: tools.len(),
            warnings,
        }
    }

    /// Emit a `DEBUG` tracing event summarising tool normalization for this round.
    ///
    /// Called after `validate()`.  The event is emitted unconditionally at DEBUG level so
    /// it is invisible in production but visible under `RUST_LOG=halcon_cli::repl=debug`.
    pub(crate) fn trace_result(&self, result: &NormalizationResult, round: u32) {
        tracing::debug!(
            round,
            provider = %self.provider_name,
            format   = result.format.label(),
            tools    = result.tool_count,
            warnings = result.warnings.len(),
            schema_field = result.format.schema_field_name(),
            injection = result.format.uses_system_prompt_injection(),
            "ProviderNormalization"
        );

        for w in &result.warnings {
            tracing::debug!(
                round,
                provider = %self.provider_name,
                warning  = %w.message(),
                "ProviderNormalization::warning"
            );
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn tool(name: &str, schema: serde_json::Value) -> ToolDefinition {
        ToolDefinition {
            name: name.to_owned(),
            description: format!("Tool {name}"),
            input_schema: schema,
        }
    }

    fn valid_schema() -> serde_json::Value {
        json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        })
    }

    // ── ProviderToolFormat::detect ────────────────────────────────────────────

    #[test]
    fn detect_anthropic() {
        assert_eq!(
            detect_tool_format("anthropic"),
            ProviderToolFormat::AnthropicInputSchema
        );
    }

    #[test]
    fn detect_openai() {
        assert_eq!(
            detect_tool_format("openai"),
            ProviderToolFormat::OpenAIFunctionObject
        );
    }

    #[test]
    fn detect_deepseek() {
        assert_eq!(
            detect_tool_format("deepseek"),
            ProviderToolFormat::OpenAIFunctionObject
        );
    }

    #[test]
    fn detect_gemini() {
        assert_eq!(
            detect_tool_format("gemini"),
            ProviderToolFormat::GeminiFunctionDeclarations
        );
    }

    #[test]
    fn detect_ollama() {
        assert_eq!(
            detect_tool_format("ollama"),
            ProviderToolFormat::OllamaXmlEmulation
        );
    }

    #[test]
    fn detect_unknown_provider_falls_through() {
        assert_eq!(
            detect_tool_format("my-custom-provider"),
            ProviderToolFormat::Unknown
        );
    }

    #[test]
    fn detect_echo_is_unknown() {
        assert_eq!(detect_tool_format("echo"), ProviderToolFormat::Unknown);
    }

    // ── ProviderToolFormat property methods ───────────────────────────────────

    #[test]
    fn anthropic_schema_field_is_input_schema() {
        assert_eq!(
            ProviderToolFormat::AnthropicInputSchema.schema_field_name(),
            "input_schema"
        );
    }

    #[test]
    fn openai_schema_field_is_parameters() {
        assert_eq!(
            ProviderToolFormat::OpenAIFunctionObject.schema_field_name(),
            "parameters"
        );
    }

    #[test]
    fn gemini_schema_field_is_parameters() {
        assert_eq!(
            ProviderToolFormat::GeminiFunctionDeclarations.schema_field_name(),
            "parameters"
        );
    }

    #[test]
    fn ollama_uses_system_prompt_injection() {
        assert!(ProviderToolFormat::OllamaXmlEmulation.uses_system_prompt_injection());
    }

    #[test]
    fn anthropic_does_not_use_system_prompt_injection() {
        assert!(!ProviderToolFormat::AnthropicInputSchema.uses_system_prompt_injection());
    }

    #[test]
    fn openai_uses_function_wrapper() {
        assert!(ProviderToolFormat::OpenAIFunctionObject.uses_function_wrapper());
    }

    #[test]
    fn gemini_uses_batch_wrapper() {
        assert!(ProviderToolFormat::GeminiFunctionDeclarations.uses_batch_wrapper());
    }

    #[test]
    fn openai_does_not_use_batch_wrapper() {
        assert!(!ProviderToolFormat::OpenAIFunctionObject.uses_batch_wrapper());
    }

    #[test]
    fn labels_are_non_empty() {
        let formats = [
            ProviderToolFormat::AnthropicInputSchema,
            ProviderToolFormat::OpenAIFunctionObject,
            ProviderToolFormat::GeminiFunctionDeclarations,
            ProviderToolFormat::OllamaXmlEmulation,
            ProviderToolFormat::Unknown,
        ];
        for f in formats {
            assert!(!f.label().is_empty(), "label for {f:?} must be non-empty");
        }
    }

    // ── validate — clean inputs ───────────────────────────────────────────────

    #[test]
    fn validate_empty_tools_is_clean() {
        let adapter = ProviderNormalizationAdapter::for_provider("anthropic");
        let result = adapter.validate(&[]);
        assert!(result.is_clean());
        assert_eq!(result.tool_count, 0);
    }

    #[test]
    fn validate_valid_tool_anthropic_clean() {
        let adapter = ProviderNormalizationAdapter::for_provider("anthropic");
        let result = adapter.validate(&[tool("file_read", valid_schema())]);
        assert!(result.is_clean(), "unexpected warnings: {:?}", result.warnings);
        assert_eq!(result.tool_count, 1);
        assert_eq!(result.format, ProviderToolFormat::AnthropicInputSchema);
    }

    #[test]
    fn validate_valid_tool_openai_clean() {
        let adapter = ProviderNormalizationAdapter::for_provider("openai");
        let result = adapter.validate(&[tool("bash", valid_schema())]);
        assert!(result.is_clean());
        assert_eq!(result.format, ProviderToolFormat::OpenAIFunctionObject);
    }

    #[test]
    fn validate_valid_tool_gemini_clean() {
        let adapter = ProviderNormalizationAdapter::for_provider("gemini");
        let result = adapter.validate(&[tool("grep", valid_schema())]);
        assert!(result.is_clean());
        assert_eq!(result.format, ProviderToolFormat::GeminiFunctionDeclarations);
    }

    // ── validate — warnings ───────────────────────────────────────────────────

    #[test]
    fn validate_ollama_with_tools_warns_emulation() {
        let adapter = ProviderNormalizationAdapter::for_provider("ollama");
        let result = adapter.validate(&[tool("t1", valid_schema()), tool("t2", valid_schema())]);
        assert!(result.warnings.iter().any(|w| matches!(
            w,
            NormalizationWarning::OllamaEmulationMode { tool_count: 2 }
        )));
    }

    #[test]
    fn validate_ollama_without_tools_no_emulation_warning() {
        let adapter = ProviderNormalizationAdapter::for_provider("ollama");
        let result = adapter.validate(&[]);
        assert!(
            !result.warnings.iter().any(|w| matches!(
                w,
                NormalizationWarning::OllamaEmulationMode { .. }
            )),
            "no tools → no emulation warning"
        );
    }

    #[test]
    fn validate_missing_type_field_warns() {
        let schema = json!({ "properties": { "x": { "type": "string" } } });
        let adapter = ProviderNormalizationAdapter::for_provider("openai");
        let result = adapter.validate(&[tool("no_type", schema)]);
        assert!(result.warnings.iter().any(|w| matches!(
            w,
            NormalizationWarning::MissingTypeField { tool } if tool == "no_type"
        )));
    }

    #[test]
    fn validate_required_field_missing_from_properties_warns() {
        let schema = json!({
            "type": "object",
            "properties": {},
            "required": ["missing"]
        });
        let adapter = ProviderNormalizationAdapter::for_provider("deepseek");
        let result = adapter.validate(&[tool("bad_tool", schema)]);
        assert!(result.warnings.iter().any(|w| matches!(
            w,
            NormalizationWarning::RequiredFieldMissing { tool, field }
                if tool == "bad_tool" && field == "missing"
        )));
    }

    #[test]
    fn validate_gemini_ref_schema_warns() {
        let schema = json!({
            "type": "object",
            "$ref": "#/$defs/MyInput",
            "$defs": { "MyInput": { "type": "string" } }
        });
        let adapter = ProviderNormalizationAdapter::for_provider("gemini");
        let result = adapter.validate(&[tool("ref_tool", schema)]);
        assert!(result.warnings.iter().any(|w| matches!(
            w,
            NormalizationWarning::UnsupportedSchemaType { tool, .. } if tool == "ref_tool"
        )));
    }

    #[test]
    fn validate_openai_ref_schema_no_ref_warning() {
        // OpenAI-compat does not warn about $ref — only Gemini does.
        let schema = json!({
            "type": "object",
            "$ref": "#/$defs/X"
        });
        let adapter = ProviderNormalizationAdapter::for_provider("openai");
        let result = adapter.validate(&[tool("ref_tool", schema)]);
        assert!(
            !result.warnings.iter().any(|w| matches!(
                w,
                NormalizationWarning::UnsupportedSchemaType { .. }
            )),
            "openai should not warn about $ref"
        );
    }

    #[test]
    fn validate_multiple_tools_collects_all_warnings() {
        let bad1 = json!({ "properties": {} }); // missing type
        let bad2 = json!({ "type": "object", "properties": {}, "required": ["x"] }); // required missing
        let good = valid_schema();

        let adapter = ProviderNormalizationAdapter::for_provider("anthropic");
        let result = adapter.validate(&[
            tool("bad1", bad1),
            tool("bad2", bad2),
            tool("good", good),
        ]);

        // At least one MissingTypeField and one RequiredFieldMissing.
        assert!(result.warnings.iter().any(|w| matches!(w, NormalizationWarning::MissingTypeField { .. })));
        assert!(result.warnings.iter().any(|w| matches!(w, NormalizationWarning::RequiredFieldMissing { .. })));
        assert_eq!(result.tool_count, 3);
    }

    // ── NormalizationResult helpers ───────────────────────────────────────────

    #[test]
    fn is_clean_true_when_no_warnings() {
        let result = NormalizationResult {
            format: ProviderToolFormat::AnthropicInputSchema,
            tool_count: 2,
            warnings: vec![],
        };
        assert!(result.is_clean());
    }

    #[test]
    fn is_clean_false_when_warnings_present() {
        let result = NormalizationResult {
            format: ProviderToolFormat::OllamaXmlEmulation,
            tool_count: 1,
            warnings: vec![NormalizationWarning::OllamaEmulationMode { tool_count: 1 }],
        };
        assert!(!result.is_clean());
    }

    // ── ProviderNormalizationAdapter construction ─────────────────────────────

    #[test]
    fn for_provider_returns_correct_format() {
        assert_eq!(
            ProviderNormalizationAdapter::for_provider("gemini").format(),
            ProviderToolFormat::GeminiFunctionDeclarations
        );
    }

    // ── NormalizationWarning messages ─────────────────────────────────────────

    #[test]
    fn warning_messages_are_non_empty() {
        let warnings = vec![
            NormalizationWarning::OllamaEmulationMode { tool_count: 3 },
            NormalizationWarning::MissingTypeField {
                tool: "t".to_owned(),
            },
            NormalizationWarning::RequiredFieldMissing {
                tool: "t".to_owned(),
                field: "f".to_owned(),
            },
            NormalizationWarning::UnsupportedSchemaType {
                tool: "t".to_owned(),
                schema_type: "$ref".to_owned(),
                format: ProviderToolFormat::GeminiFunctionDeclarations,
            },
        ];
        for w in &warnings {
            assert!(!w.message().is_empty(), "message for {w:?} must not be empty");
        }
    }
}
