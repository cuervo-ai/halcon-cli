use std::borrow::Cow;

use reedline::{Prompt, PromptEditMode, PromptHistorySearch, PromptHistorySearchStatus};

/// Custom prompt for the cuervo REPL.
///
/// Renders as: `cuervo [sonnet] > `
pub struct CuervoPrompt {
    left: String,
}

impl CuervoPrompt {
    pub fn new(provider: &str, model: &str) -> Self {
        let short = Self::shorten_model(model);
        let _ = provider; // available for future use
        Self {
            left: format!("cuervo [{short}]"),
        }
    }

    /// Produce a short display name for common models.
    pub fn shorten_model(model: &str) -> String {
        let m = model.to_lowercase();
        if m.contains("sonnet") {
            return "sonnet".into();
        }
        if m.contains("opus") {
            return "opus".into();
        }
        if m.contains("haiku") {
            return "haiku".into();
        }
        if m.contains("gpt-4o") {
            return "gpt-4o".into();
        }
        // Keep short names as-is (llama3.2, mistral, etc.)
        if model.len() <= 20 {
            return model.to_string();
        }
        model[..20].to_string()
    }
}

impl Prompt for CuervoPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.left)
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_indicator(&self, edit_mode: PromptEditMode) -> Cow<'_, str> {
        match edit_mode {
            PromptEditMode::Default | PromptEditMode::Emacs => Cow::Borrowed(" > "),
            PromptEditMode::Vi(vi_mode) => match vi_mode {
                reedline::PromptViMode::Normal => Cow::Borrowed(" : "),
                reedline::PromptViMode::Insert => Cow::Borrowed(" > "),
            },
            PromptEditMode::Custom(_) => Cow::Borrowed(" > "),
        }
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed("... ")
    }

    fn render_prompt_history_search_indicator(
        &self,
        history_search: PromptHistorySearch,
    ) -> Cow<'_, str> {
        let prefix = match history_search.status {
            PromptHistorySearchStatus::Passing => "",
            PromptHistorySearchStatus::Failing => "(failed) ",
        };
        Cow::Owned(format!("{prefix}(search: '{}') > ", history_search.term))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shorten_sonnet() {
        assert_eq!(
            CuervoPrompt::shorten_model("claude-sonnet-4-5-20250929"),
            "sonnet"
        );
    }

    #[test]
    fn shorten_opus() {
        assert_eq!(CuervoPrompt::shorten_model("claude-opus-4-6"), "opus");
    }

    #[test]
    fn shorten_haiku() {
        assert_eq!(
            CuervoPrompt::shorten_model("claude-haiku-4-5-20251001"),
            "haiku"
        );
    }

    #[test]
    fn shorten_gpt4o() {
        assert_eq!(CuervoPrompt::shorten_model("gpt-4o"), "gpt-4o");
    }

    #[test]
    fn shorten_short_name() {
        assert_eq!(CuervoPrompt::shorten_model("llama3.2"), "llama3.2");
    }

    #[test]
    fn shorten_long_name_truncates() {
        let long = "a-very-long-model-name-that-exceeds-twenty-chars";
        assert_eq!(CuervoPrompt::shorten_model(long).len(), 20);
    }

    #[test]
    fn prompt_left_contains_model() {
        let p = CuervoPrompt::new("anthropic", "claude-sonnet-4-5-20250929");
        assert_eq!(p.render_prompt_left().as_ref(), "cuervo [sonnet]");
    }
}
