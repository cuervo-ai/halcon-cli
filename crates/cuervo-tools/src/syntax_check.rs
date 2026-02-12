//! Lightweight syntax verification for edited/written files.
//!
//! Checks balanced delimiters, unclosed strings, and JSON validity.
//! No external dependencies — operates on raw text in a single O(n) pass.

use std::fmt;

/// Severity of a syntax issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Warning,
    Error,
}

/// A syntax issue detected in file content.
#[derive(Debug, Clone)]
pub struct SyntaxWarning {
    pub line: usize,
    pub column: usize,
    pub message: String,
    pub severity: Severity,
}

impl fmt::Display for SyntaxWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sev = match self.severity {
            Severity::Warning => "warning",
            Severity::Error => "error",
        };
        write!(f, "  L{}:{}: [{}] {}", self.line, self.column, sev, self.message)
    }
}

/// Check file content for syntax issues based on file extension.
///
/// Returns an empty vec for unsupported file types.
pub fn check_syntax(content: &str, path: &str) -> Vec<SyntaxWarning> {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "rs" | "c" | "cpp" | "h" | "hpp" | "java" | "go" | "swift" => {
            check_c_family_delimiters(content)
        }
        "js" | "ts" | "jsx" | "tsx" | "mjs" | "mts" => {
            check_c_family_delimiters(content)
        }
        "py" => check_python_syntax(content),
        "json" => check_json_syntax(content),
        "toml" => check_toml_syntax(content),
        "yaml" | "yml" => check_balanced_quotes(content),
        _ => Vec::new(),
    }
}

/// Format syntax warnings into a string suitable for appending to tool output.
pub fn format_warnings(warnings: &[SyntaxWarning]) -> String {
    if warnings.is_empty() {
        return String::new();
    }

    let errors = warnings.iter().filter(|w| w.severity == Severity::Error).count();
    let warns = warnings.len() - errors;

    let mut out = String::from("\n\n⚠ Syntax check:");
    if errors > 0 {
        out.push_str(&format!(" {} error(s)", errors));
    }
    if warns > 0 {
        if errors > 0 {
            out.push(',');
        }
        out.push_str(&format!(" {} warning(s)", warns));
    }
    out.push('\n');

    // Limit to 10 most important issues (errors first).
    let mut sorted: Vec<&SyntaxWarning> = warnings.iter().collect();
    sorted.sort_by_key(|w| match w.severity {
        Severity::Error => 0,
        Severity::Warning => 1,
    });
    for w in sorted.iter().take(10) {
        out.push_str(&w.to_string());
        out.push('\n');
    }
    if warnings.len() > 10 {
        out.push_str(&format!("  ... and {} more issues\n", warnings.len() - 10));
    }

    out
}

/// State machine for tracking delimiters in C-family languages (Rust, JS, Go, etc.).
///
/// Handles:
/// - `{}`, `[]`, `()` — balanced pairs
/// - String literals (`"..."`, `'...'` for non-Rust)
/// - Line comments (`//`)
/// - Block comments (`/* ... */`)
/// - Raw strings (`r"..."`, `r#"..."#` for Rust)
fn check_c_family_delimiters(content: &str) -> Vec<SyntaxWarning> {
    let mut warnings = Vec::new();
    let mut stack: Vec<(char, usize, usize)> = Vec::new(); // (opener, line, col)
    let chars: Vec<char> = content.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut line = 1usize;
    let mut col = 1usize;

    while i < len {
        let ch = chars[i];

        // Track position.
        if ch == '\n' {
            line += 1;
            col = 0; // Will be incremented at end.
            i += 1;
            col += 1;
            continue;
        }

        // Skip line comments.
        if ch == '/' && i + 1 < len && chars[i + 1] == '/' {
            while i < len && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }

        // Skip block comments.
        if ch == '/' && i + 1 < len && chars[i + 1] == '*' {
            i += 2;
            col += 2;
            let comment_line = line;
            let comment_col = col - 2;
            while i < len {
                if chars[i] == '\n' {
                    line += 1;
                    col = 0;
                } else if chars[i] == '*' && i + 1 < len && chars[i + 1] == '/' {
                    i += 2;
                    col += 2;
                    break;
                }
                i += 1;
                col += 1;
            }
            if i >= len {
                warnings.push(SyntaxWarning {
                    line: comment_line,
                    column: comment_col,
                    message: "unclosed block comment `/*`".to_string(),
                    severity: Severity::Error,
                });
            }
            continue;
        }

        // Skip string literals.
        if ch == '"' || ch == '\'' {
            let quote = ch;
            let str_line = line;
            let str_col = col;
            i += 1;
            col += 1;
            while i < len {
                if chars[i] == '\\' {
                    // Skip escaped character.
                    i += 2;
                    col += 2;
                    continue;
                }
                if chars[i] == '\n' {
                    line += 1;
                    col = 0;
                }
                if chars[i] == quote {
                    i += 1;
                    col += 1;
                    break;
                }
                i += 1;
                col += 1;
            }
            if i > len {
                warnings.push(SyntaxWarning {
                    line: str_line,
                    column: str_col,
                    message: format!("unclosed string literal `{}`", quote),
                    severity: Severity::Error,
                });
            }
            continue;
        }

        // Skip Rust raw strings: r"..." or r#"..."#
        if ch == 'r' && i + 1 < len && (chars[i + 1] == '"' || chars[i + 1] == '#') {
            let mut hashes = 0usize;
            let mut j = i + 1;
            while j < len && chars[j] == '#' {
                hashes += 1;
                j += 1;
            }
            if j < len && chars[j] == '"' {
                // Raw string — skip to closing "###
                j += 1;
                let raw_line = line;
                let raw_col = col;
                let mut found = false;
                while j < len {
                    if chars[j] == '\n' {
                        line += 1;
                    }
                    if chars[j] == '"' {
                        let mut closing_hashes = 0;
                        let mut k = j + 1;
                        while k < len && chars[k] == '#' && closing_hashes < hashes {
                            closing_hashes += 1;
                            k += 1;
                        }
                        if closing_hashes == hashes {
                            j = k;
                            found = true;
                            break;
                        }
                    }
                    j += 1;
                }
                if !found {
                    warnings.push(SyntaxWarning {
                        line: raw_line,
                        column: raw_col,
                        message: "unclosed raw string literal".to_string(),
                        severity: Severity::Error,
                    });
                }
                i = j;
                col = 1; // Approximate after raw string.
                continue;
            }
        }

        // Backtick template literals (JS/TS).
        if ch == '`' {
            let tpl_line = line;
            let tpl_col = col;
            i += 1;
            col += 1;
            let mut depth = 0usize; // Track ${...} nesting.
            while i < len {
                if chars[i] == '\\' {
                    i += 2;
                    col += 2;
                    continue;
                }
                if chars[i] == '\n' {
                    line += 1;
                    col = 0;
                }
                if chars[i] == '$' && i + 1 < len && chars[i + 1] == '{' {
                    depth += 1;
                    i += 2;
                    col += 2;
                    continue;
                }
                if chars[i] == '{' && depth > 0 {
                    depth += 1;
                }
                if chars[i] == '}' && depth > 0 {
                    depth -= 1;
                }
                if chars[i] == '`' && depth == 0 {
                    i += 1;
                    col += 1;
                    break;
                }
                i += 1;
                col += 1;
            }
            if i > len {
                warnings.push(SyntaxWarning {
                    line: tpl_line,
                    column: tpl_col,
                    message: "unclosed template literal".to_string(),
                    severity: Severity::Error,
                });
            }
            continue;
        }

        // Delimiter tracking.
        match ch {
            '{' | '[' | '(' => {
                stack.push((ch, line, col));
            }
            '}' | ']' | ')' => {
                let expected = match ch {
                    '}' => '{',
                    ']' => '[',
                    ')' => '(',
                    _ => unreachable!(),
                };
                match stack.last() {
                    Some(&(opener, _, _)) if opener == expected => {
                        stack.pop();
                    }
                    Some(&(opener, open_line, open_col)) => {
                        warnings.push(SyntaxWarning {
                            line,
                            column: col,
                            message: format!(
                                "mismatched `{}` — expected closing for `{}` opened at L{}:{}",
                                ch, opener, open_line, open_col
                            ),
                            severity: Severity::Error,
                        });
                        // Pop the mismatched opener to avoid cascading.
                        stack.pop();
                    }
                    None => {
                        warnings.push(SyntaxWarning {
                            line,
                            column: col,
                            message: format!("unexpected closing `{}` with no matching opener", ch),
                            severity: Severity::Error,
                        });
                    }
                }
            }
            _ => {}
        }

        i += 1;
        col += 1;
    }

    // Report unclosed delimiters.
    for (opener, open_line, open_col) in stack.iter().rev() {
        let closer = match opener {
            '{' => '}',
            '[' => ']',
            '(' => ')',
            _ => '?',
        };
        warnings.push(SyntaxWarning {
            line: *open_line,
            column: *open_col,
            message: format!("unclosed `{}` — expected `{}`", opener, closer),
            severity: Severity::Error,
        });
    }

    warnings
}

/// Check Python for balanced delimiters and unclosed strings.
///
/// Python uses `#` for comments and triple-quoted strings.
fn check_python_syntax(content: &str) -> Vec<SyntaxWarning> {
    let mut warnings = Vec::new();
    let mut stack: Vec<(char, usize, usize)> = Vec::new();
    let chars: Vec<char> = content.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut line = 1usize;
    let mut col = 1usize;

    while i < len {
        let ch = chars[i];

        if ch == '\n' {
            line += 1;
            col = 0;
            i += 1;
            col += 1;
            continue;
        }

        // Line comments.
        if ch == '#' {
            while i < len && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }

        // Triple-quoted strings.
        if (ch == '"' || ch == '\'')
            && i + 2 < len
            && chars[i + 1] == ch
            && chars[i + 2] == ch
        {
            let quote = ch;
            let str_line = line;
            let str_col = col;
            i += 3;
            col += 3;
            let mut found = false;
            while i + 2 < len {
                if chars[i] == '\n' {
                    line += 1;
                    col = 0;
                }
                if chars[i] == quote && chars[i + 1] == quote && chars[i + 2] == quote {
                    i += 3;
                    col += 3;
                    found = true;
                    break;
                }
                i += 1;
                col += 1;
            }
            if !found {
                // Consume remaining chars.
                while i < len {
                    if chars[i] == '\n' {
                        line += 1;
                    }
                    i += 1;
                }
                warnings.push(SyntaxWarning {
                    line: str_line,
                    column: str_col,
                    message: "unclosed triple-quoted string".to_string(),
                    severity: Severity::Error,
                });
            }
            continue;
        }

        // Single/double quoted strings.
        if ch == '"' || ch == '\'' {
            let quote = ch;
            let str_line = line;
            let str_col = col;
            i += 1;
            col += 1;
            while i < len && chars[i] != '\n' {
                if chars[i] == '\\' {
                    i += 2;
                    col += 2;
                    continue;
                }
                if chars[i] == quote {
                    i += 1;
                    col += 1;
                    break;
                }
                i += 1;
                col += 1;
            }
            if i <= len && (i >= len || chars[i - 1] != quote) {
                // Might be unclosed, but Python allows same-line only for non-triple.
                // This is heuristic — only flag if we hit newline.
                if i < len && chars[i] == '\n' {
                    warnings.push(SyntaxWarning {
                        line: str_line,
                        column: str_col,
                        message: "unclosed string literal (newline before closing quote)"
                            .to_string(),
                        severity: Severity::Warning,
                    });
                }
            }
            continue;
        }

        // Delimiter tracking (same as C-family).
        match ch {
            '{' | '[' | '(' => stack.push((ch, line, col)),
            '}' | ']' | ')' => {
                let expected = match ch {
                    '}' => '{',
                    ']' => '[',
                    ')' => '(',
                    _ => unreachable!(),
                };
                match stack.last() {
                    Some(&(opener, _, _)) if opener == expected => {
                        stack.pop();
                    }
                    Some(&(opener, ol, oc)) => {
                        warnings.push(SyntaxWarning {
                            line,
                            column: col,
                            message: format!(
                                "mismatched `{}` — expected closing for `{}` at L{}:{}",
                                ch, opener, ol, oc
                            ),
                            severity: Severity::Error,
                        });
                        stack.pop();
                    }
                    None => {
                        warnings.push(SyntaxWarning {
                            line,
                            column: col,
                            message: format!(
                                "unexpected closing `{}` with no matching opener",
                                ch
                            ),
                            severity: Severity::Error,
                        });
                    }
                }
            }
            _ => {}
        }

        i += 1;
        col += 1;
    }

    for (opener, ol, oc) in stack.iter().rev() {
        let closer = match opener {
            '{' => '}',
            '[' => ']',
            '(' => ')',
            _ => '?',
        };
        warnings.push(SyntaxWarning {
            line: *ol,
            column: *oc,
            message: format!("unclosed `{}` — expected `{}`", opener, closer),
            severity: Severity::Error,
        });
    }

    warnings
}

/// Validate JSON by attempting to parse it.
fn check_json_syntax(content: &str) -> Vec<SyntaxWarning> {
    match serde_json::from_str::<serde_json::Value>(content) {
        Ok(_) => Vec::new(),
        Err(e) => {
            vec![SyntaxWarning {
                line: e.line(),
                column: e.column(),
                message: format!("JSON parse error: {}", e),
                severity: Severity::Error,
            }]
        }
    }
}

/// Validate TOML by attempting to parse it.
fn check_toml_syntax(content: &str) -> Vec<SyntaxWarning> {
    // Use basic delimiter check — toml crate may not be available in this crate.
    // TOML uses `#` comments and `"` / `'` strings.
    check_balanced_quotes(content)
}

/// Simple balanced-quotes check for config files.
fn check_balanced_quotes(content: &str) -> Vec<SyntaxWarning> {
    let mut warnings = Vec::new();
    for (line_num, line_text) in content.lines().enumerate() {
        let line_num = line_num + 1;
        // Skip comment-only lines.
        let trimmed = line_text.trim();
        if trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }
        // Count unescaped double quotes.
        let dquotes = count_unescaped(line_text, '"');
        if dquotes % 2 != 0 {
            warnings.push(SyntaxWarning {
                line: line_num,
                column: 1,
                message: "odd number of double quotes on this line".to_string(),
                severity: Severity::Warning,
            });
        }
    }
    warnings
}

fn count_unescaped(text: &str, target: char) -> usize {
    let mut count = 0;
    let mut prev_backslash = false;
    for ch in text.chars() {
        if ch == target && !prev_backslash {
            count += 1;
        }
        prev_backslash = ch == '\\' && !prev_backslash;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Balanced delimiters ---

    #[test]
    fn balanced_braces_ok() {
        let code = "fn main() {\n    println!(\"hello\");\n}\n";
        let warnings = check_syntax(code, "main.rs");
        assert!(warnings.is_empty(), "expected no warnings: {:?}", warnings);
    }

    #[test]
    fn unclosed_brace() {
        let code = "fn main() {\n    let x = 1;\n";
        let warnings = check_syntax(code, "main.rs");
        assert!(!warnings.is_empty());
        assert!(warnings[0].message.contains("unclosed `{`"));
        assert_eq!(warnings[0].severity, Severity::Error);
    }

    #[test]
    fn extra_closing_brace() {
        let code = "fn main() {\n}\n}\n";
        let warnings = check_syntax(code, "main.rs");
        assert!(!warnings.is_empty());
        assert!(warnings[0].message.contains("unexpected closing `}`"));
    }

    #[test]
    fn mismatched_delimiters() {
        let code = "fn f() {\n    let x = [1, 2);\n}\n";
        let warnings = check_syntax(code, "main.rs");
        assert!(!warnings.is_empty());
        assert!(warnings[0].message.contains("mismatched"));
    }

    #[test]
    fn nested_delimiters_ok() {
        let code = "fn f() {\n    let x = vec![(1, 2), (3, 4)];\n    if x.is_empty() { return; }\n}\n";
        let warnings = check_syntax(code, "main.rs");
        assert!(warnings.is_empty(), "expected no warnings: {:?}", warnings);
    }

    // --- String handling ---

    #[test]
    fn string_with_braces_inside() {
        let code = "fn f() {\n    let s = \"{hello}\";\n}\n";
        let warnings = check_syntax(code, "main.rs");
        assert!(warnings.is_empty(), "braces inside strings should be ignored: {:?}", warnings);
    }

    #[test]
    fn escaped_quote_in_string() {
        let code = r#"fn f() {
    let s = "hello \"world\"";
}
"#;
        let warnings = check_syntax(code, "main.rs");
        assert!(warnings.is_empty(), "escaped quotes should not confuse checker: {:?}", warnings);
    }

    // --- Comments ---

    #[test]
    fn line_comment_with_braces() {
        let code = "fn f() {\n    // this { is a comment\n}\n";
        let warnings = check_syntax(code, "main.rs");
        assert!(warnings.is_empty(), "braces in line comments should be ignored: {:?}", warnings);
    }

    #[test]
    fn block_comment_with_braces() {
        let code = "fn f() {\n    /* { unmatched in comment */ \n}\n";
        let warnings = check_syntax(code, "main.rs");
        assert!(warnings.is_empty(), "braces in block comments should be ignored: {:?}", warnings);
    }

    #[test]
    fn unclosed_block_comment() {
        let code = "fn f() {\n    /* unclosed comment\n}\n";
        let warnings = check_syntax(code, "main.rs");
        assert!(!warnings.is_empty());
        assert!(warnings.iter().any(|w| w.message.contains("unclosed block comment")));
    }

    // --- Raw strings ---

    #[test]
    fn rust_raw_string_with_braces() {
        let code = "fn f() {\n    let s = r#\"{ not a brace }\"#;\n}\n";
        let warnings = check_syntax(code, "main.rs");
        assert!(warnings.is_empty(), "braces in raw strings should be ignored: {:?}", warnings);
    }

    // --- JSON validation ---

    #[test]
    fn valid_json() {
        let content = r#"{"name": "test", "value": [1, 2, 3]}"#;
        let warnings = check_syntax(content, "config.json");
        assert!(warnings.is_empty());
    }

    #[test]
    fn invalid_json() {
        let content = r#"{"name": "test", "value": [1, 2, 3}"#;
        let warnings = check_syntax(content, "config.json");
        assert!(!warnings.is_empty());
        assert!(warnings[0].message.contains("JSON parse error"));
    }

    // --- Python ---

    #[test]
    fn python_balanced() {
        let code = "def f():\n    x = [1, 2]\n    return {\"a\": x}\n";
        let warnings = check_syntax(code, "main.py");
        assert!(warnings.is_empty(), "expected no warnings: {:?}", warnings);
    }

    #[test]
    fn python_unclosed_bracket() {
        let code = "def f():\n    x = [1, 2\n    return x\n";
        let warnings = check_syntax(code, "main.py");
        assert!(!warnings.is_empty());
        assert!(warnings[0].message.contains("unclosed `[`"));
    }

    #[test]
    fn python_comment_ignored() {
        let code = "def f():\n    # { this is fine\n    return 1\n";
        let warnings = check_syntax(code, "main.py");
        assert!(warnings.is_empty());
    }

    #[test]
    fn python_triple_quoted_string() {
        let code = "s = \"\"\"{\n    unmatched brace in triple string\n}\"\"\"\n";
        let warnings = check_syntax(code, "main.py");
        assert!(warnings.is_empty(), "braces in triple-quoted strings should be ignored: {:?}", warnings);
    }

    // --- JS/TS ---

    #[test]
    fn js_template_literal() {
        let code = "const s = `hello ${name} { not a brace }`;\n";
        let warnings = check_syntax(code, "app.js");
        assert!(warnings.is_empty(), "template literals should be handled: {:?}", warnings);
    }

    #[test]
    fn typescript_balanced() {
        let code = "function f(x: number): { a: number } {\n    return { a: x };\n}\n";
        let warnings = check_syntax(code, "app.ts");
        assert!(warnings.is_empty());
    }

    // --- Format warnings ---

    #[test]
    fn format_empty_warnings() {
        let result = format_warnings(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn format_with_warnings() {
        let warnings = vec![
            SyntaxWarning {
                line: 10,
                column: 5,
                message: "unclosed `{`".to_string(),
                severity: Severity::Error,
            },
            SyntaxWarning {
                line: 3,
                column: 1,
                message: "odd quotes".to_string(),
                severity: Severity::Warning,
            },
        ];
        let result = format_warnings(&warnings);
        assert!(result.contains("1 error(s)"));
        assert!(result.contains("1 warning(s)"));
        assert!(result.contains("unclosed `{`"));
    }

    #[test]
    fn format_truncates_beyond_10() {
        let warnings: Vec<SyntaxWarning> = (0..15)
            .map(|i| SyntaxWarning {
                line: i + 1,
                column: 1,
                message: format!("issue {}", i),
                severity: Severity::Warning,
            })
            .collect();
        let result = format_warnings(&warnings);
        assert!(result.contains("5 more issues"));
    }

    // --- Unsupported extension ---

    #[test]
    fn unsupported_extension_returns_empty() {
        let warnings = check_syntax("random content { [ (", "data.bin");
        assert!(warnings.is_empty());
    }

    // --- Edge cases ---

    #[test]
    fn empty_file() {
        let warnings = check_syntax("", "main.rs");
        assert!(warnings.is_empty());
    }

    #[test]
    fn deeply_nested_ok() {
        let code = "fn f() {\n    if true {\n        match x {\n            Some(v) => {\n                vec![(v, 1)]\n            }\n            None => vec![]\n        }\n    }\n}\n";
        let warnings = check_syntax(code, "main.rs");
        assert!(warnings.is_empty(), "deeply nested should be fine: {:?}", warnings);
    }

    #[test]
    fn multiple_errors_reported() {
        let code = "fn f() {\n    let x = [1, 2;\n    let y = (3, 4;\n";
        let warnings = check_syntax(code, "main.rs");
        // Should report multiple issues.
        assert!(warnings.len() >= 2, "expected multiple issues: {:?}", warnings);
    }
}
