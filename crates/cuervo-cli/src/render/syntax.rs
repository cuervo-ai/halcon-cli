use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::{as_24_bit_terminal_escaped, LinesWithEndings};

/// Syntax highlighter backed by syntect's default bundles.
pub struct Highlighter {
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
}

impl Highlighter {
    /// Create a highlighter with default syntax definitions and themes.
    pub fn new() -> Self {
        Self {
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
        }
    }

    /// Highlight a code block and return ANSI-escaped string.
    ///
    /// `lang` is the fence label (e.g. "rust", "python", "json").
    /// If the language is unknown, falls back to plain text.
    /// When `NO_COLOR` is set or `TERM=dumb`, returns raw code without ANSI escapes.
    pub fn highlight(&self, code: &str, lang: &str) -> String {
        if !super::color::color_enabled() {
            return code.to_string();
        }

        let syntax = self
            .resolve_syntax(lang)
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());

        let theme = &self.theme_set.themes["base16-ocean.dark"];
        let mut h = HighlightLines::new(syntax, theme);
        let mut output = String::with_capacity(code.len() * 2);

        for line in LinesWithEndings::from(code) {
            match h.highlight_line(line, &self.syntax_set) {
                Ok(ranges) => {
                    let escaped = as_24_bit_terminal_escaped(&ranges, false);
                    output.push_str(&escaped);
                }
                Err(_) => {
                    // Fallback: emit raw line on highlight error.
                    output.push_str(line);
                }
            }
        }
        // Reset terminal colors after highlighted block.
        output.push_str("\x1b[0m");
        output
    }

    /// Resolve a fence label to a syntect syntax definition.
    fn resolve_syntax(&self, lang: &str) -> Option<&syntect::parsing::SyntaxReference> {
        let lower = lang.to_lowercase();
        let normalized = match lower.as_str() {
            "rs" | "rust" => "Rust",
            "ts" | "typescript" => "TypeScript",
            "js" | "javascript" => "JavaScript",
            "py" | "python" => "Python",
            "sh" | "bash" | "shell" | "zsh" => "Bourne Again Shell (bash)",
            "json" => "JSON",
            "toml" => "TOML",
            "yaml" | "yml" => "YAML",
            "md" | "markdown" => "Markdown",
            "sql" => "SQL",
            "html" => "HTML",
            "css" => "CSS",
            "go" | "golang" => "Go",
            "rb" | "ruby" => "Ruby",
            "c" => "C",
            "cpp" | "c++" | "cxx" => "C++",
            "java" => "Java",
            "xml" => "XML",
            "diff" | "patch" => "Diff",
            other => other,
        };
        self.syntax_set.find_syntax_by_name(normalized).or_else(|| {
            self.syntax_set
                .find_syntax_by_extension(&lang.to_lowercase())
        })
    }
}

impl Default for Highlighter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_rust_code() {
        let hl = Highlighter::new();
        let code = "fn main() {\n    println!(\"hello\");\n}\n";
        let out = hl.highlight(code, "rust");
        // Output should contain ANSI escape codes.
        assert!(out.contains("\x1b["));
        assert!(out.contains("main"));
    }

    #[test]
    fn highlight_python_code() {
        let hl = Highlighter::new();
        let code = "def hello():\n    print('world')\n";
        let out = hl.highlight(code, "python");
        assert!(out.contains("\x1b["));
        assert!(out.contains("hello"));
    }

    #[test]
    fn highlight_json() {
        let hl = Highlighter::new();
        let code = "{\"key\": \"value\"}\n";
        let out = hl.highlight(code, "json");
        assert!(out.contains("key"));
    }

    #[test]
    fn highlight_alias_rs() {
        let hl = Highlighter::new();
        let code = "let x = 42;\n";
        let out = hl.highlight(code, "rs");
        assert!(out.contains("\x1b["));
    }

    #[test]
    fn highlight_alias_ts() {
        let hl = Highlighter::new();
        let code = "const x: number = 42;\n";
        let out = hl.highlight(code, "ts");
        assert!(out.contains("42"));
    }

    #[test]
    fn highlight_unknown_lang_fallback() {
        let hl = Highlighter::new();
        let code = "some random text\n";
        let out = hl.highlight(code, "nonexistent_lang");
        // Should still produce output (plain text fallback).
        assert!(out.contains("some random text"));
    }

    #[test]
    fn highlight_empty_code() {
        let hl = Highlighter::new();
        let out = hl.highlight("", "rust");
        // Should just have the reset code.
        assert!(out.contains("\x1b[0m"));
    }

    #[test]
    fn highlight_bash_code() {
        let hl = Highlighter::new();
        let code = "echo \"hello world\"\n";
        let out = hl.highlight(code, "bash");
        assert!(out.contains("echo"));
    }
}
