//! Canonical tool wire-format and tokenizer hint types.
//!
//! These types live in `halcon-core` so that provider crates can declare their
//! wire format and tokenizer characteristics via the `ModelProvider` trait,
//! and consumer crates (halcon-cli) can query them without string-based detection.

/// Describes how a provider expects tool definitions to be serialized on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolFormat {
    /// Anthropic Claude — `input_schema` field, no wrapper struct.
    AnthropicInputSchema,

    /// OpenAI-compatible (OpenAI, DeepSeek) — `parameters` field, each tool wrapped in
    /// `{ "type": "function", "function": { … } }`.
    OpenAIFunctionObject,

    /// Google Gemini — `parameters` field, **all** tools merged into a single
    /// `{ "functionDeclarations": [ … ] }` wrapper sent in the `tools` array.
    GeminiFunctionDeclarations,

    /// Ollama local models — tools are injected into the system prompt as `<tool_call>` XML
    /// instructions; the model's XML responses are parsed back into tool-use chunks.
    OllamaXmlEmulation,

    /// Unknown / custom provider — format cannot be determined statically.
    Unknown,
}

impl ToolFormat {
    /// Name of the JSON field used for the input schema in this format.
    ///
    /// Returns `"n/a"` for formats that use system-prompt injection instead.
    pub fn schema_field_name(self) -> &'static str {
        match self {
            Self::AnthropicInputSchema => "input_schema",
            Self::OpenAIFunctionObject | Self::GeminiFunctionDeclarations => "parameters",
            Self::OllamaXmlEmulation => "n/a (system-prompt injection)",
            Self::Unknown => "unknown",
        }
    }

    /// Whether tools are injected into the system prompt rather than sent in a `tools` field.
    pub fn uses_system_prompt_injection(self) -> bool {
        matches!(self, Self::OllamaXmlEmulation)
    }

    /// Whether the format wraps each tool in an outer function object (OpenAI-compat style).
    pub fn uses_function_wrapper(self) -> bool {
        matches!(
            self,
            Self::OpenAIFunctionObject | Self::GeminiFunctionDeclarations
        )
    }

    /// Whether all tools are collected into a single wrapper (Gemini style).
    pub fn uses_batch_wrapper(self) -> bool {
        matches!(self, Self::GeminiFunctionDeclarations)
    }

    /// Short human-readable label for tracing / UI.
    pub fn label(self) -> &'static str {
        match self {
            Self::AnthropicInputSchema => "anthropic/input_schema",
            Self::OpenAIFunctionObject => "openai/function_object",
            Self::GeminiFunctionDeclarations => "gemini/function_declarations",
            Self::OllamaXmlEmulation => "ollama/xml_emulation",
            Self::Unknown => "unknown",
        }
    }
}

/// Hint about the tokenizer family used by a provider's models.
///
/// Used for more accurate token estimation when computing context budgets,
/// output headroom, and reward normalization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenizerHint {
    /// Anthropic Claude BPE tokenizer (~3.5 chars/token).
    ClaudeBpe,

    /// OpenAI cl100k_base / o200k_base (~4.0 chars/token).
    TiktokenCl100k,

    /// DeepSeek BPE tokenizer (~4.5 chars/token).
    DeepSeekBpe,

    /// Google Gemini SentencePiece tokenizer (~5.0 chars/token).
    GeminiSentencePiece,

    /// Ollama local models — tokenizer unknown, use conservative estimate.
    OllamaUnknown,

    /// Unknown tokenizer — use conservative default.
    Unknown,
}

impl TokenizerHint {
    /// Approximate characters per token for this tokenizer family.
    ///
    /// Used for quick estimates when a real tokenizer is unavailable.
    pub fn chars_per_token(self) -> f32 {
        match self {
            Self::ClaudeBpe => 3.5,
            Self::TiktokenCl100k => 4.0,
            Self::DeepSeekBpe => 4.5,
            Self::GeminiSentencePiece => 5.0,
            Self::OllamaUnknown => 4.0,
            Self::Unknown => 4.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_format_schema_field_names() {
        assert_eq!(ToolFormat::AnthropicInputSchema.schema_field_name(), "input_schema");
        assert_eq!(ToolFormat::OpenAIFunctionObject.schema_field_name(), "parameters");
        assert_eq!(ToolFormat::GeminiFunctionDeclarations.schema_field_name(), "parameters");
    }

    #[test]
    fn tool_format_system_prompt_injection() {
        assert!(ToolFormat::OllamaXmlEmulation.uses_system_prompt_injection());
        assert!(!ToolFormat::AnthropicInputSchema.uses_system_prompt_injection());
        assert!(!ToolFormat::OpenAIFunctionObject.uses_system_prompt_injection());
    }

    #[test]
    fn tool_format_function_wrapper() {
        assert!(ToolFormat::OpenAIFunctionObject.uses_function_wrapper());
        assert!(ToolFormat::GeminiFunctionDeclarations.uses_function_wrapper());
        assert!(!ToolFormat::AnthropicInputSchema.uses_function_wrapper());
    }

    #[test]
    fn tool_format_batch_wrapper() {
        assert!(ToolFormat::GeminiFunctionDeclarations.uses_batch_wrapper());
        assert!(!ToolFormat::OpenAIFunctionObject.uses_batch_wrapper());
    }

    #[test]
    fn tool_format_labels_non_empty() {
        let formats = [
            ToolFormat::AnthropicInputSchema,
            ToolFormat::OpenAIFunctionObject,
            ToolFormat::GeminiFunctionDeclarations,
            ToolFormat::OllamaXmlEmulation,
            ToolFormat::Unknown,
        ];
        for f in formats {
            assert!(!f.label().is_empty(), "label for {f:?} must be non-empty");
        }
    }

    #[test]
    fn tokenizer_hint_chars_per_token_positive() {
        let hints = [
            TokenizerHint::ClaudeBpe,
            TokenizerHint::TiktokenCl100k,
            TokenizerHint::DeepSeekBpe,
            TokenizerHint::GeminiSentencePiece,
            TokenizerHint::OllamaUnknown,
            TokenizerHint::Unknown,
        ];
        for h in hints {
            assert!(h.chars_per_token() > 0.0, "chars_per_token for {h:?} must be positive");
        }
    }

    #[test]
    fn tokenizer_hint_ordering() {
        // Claude is densest (fewest chars/token), Gemini is sparsest
        assert!(TokenizerHint::ClaudeBpe.chars_per_token() < TokenizerHint::TiktokenCl100k.chars_per_token());
        assert!(TokenizerHint::TiktokenCl100k.chars_per_token() < TokenizerHint::DeepSeekBpe.chars_per_token());
        assert!(TokenizerHint::DeepSeekBpe.chars_per_token() < TokenizerHint::GeminiSentencePiece.chars_per_token());
    }
}
