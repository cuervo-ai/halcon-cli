//! Core activity feed types — data model without presentation logic.
//!
//! **P0.1A: Architecture Consolidation**
//!
//! Extracted from `activity.rs` to separate concerns:
//! - This module: core data types (ActivityLine, ToolResult, markdown helpers)
//! - activity_model.rs: storage + indexing + push_*() methods
//! - activity_renderer.rs: rendering logic
//!
//! This eliminates the legacy ActivityState that was causing architectural confusion.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Result of a completed tool execution.
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
    pub duration_ms: u64,
}

/// A single line/block in the activity feed.
#[derive(Debug, Clone)]
pub enum ActivityLine {
    /// User's submitted prompt text.
    UserPrompt(String),
    /// Accumulated streaming assistant response.
    AssistantText(String),
    /// Syntax-highlighted code block.
    CodeBlock { lang: String, code: String },
    /// Informational message (round separators, status, etc.).
    Info(String),
    /// Warning message with optional hint.
    Warning { message: String, hint: Option<String> },
    /// Error message with optional hint.
    Error { message: String, hint: Option<String> },
    /// Visual separator between agent rounds.
    RoundSeparator(usize),
    /// Tool execution — shows skeleton while loading, result when done.
    /// When `expanded` is true, shows full output; when false, shows compact summary.
    ToolExec {
        name: String,
        input_preview: String,
        result: Option<ToolResult>,
        expanded: bool,
    },
    /// Plan overview — shows the execution plan with step statuses.
    PlanOverview {
        goal: String,
        steps: Vec<crate::tui::events::PlanStepStatus>,
        current_step: usize,
    },
}

impl ActivityLine {
    /// Classify this line as conversational (user/assistant) or system (info/warning/tool).
    ///
    /// Phase 3.3: Used for filtering system events when user wants conversation-only view.
    pub fn is_conversational(&self) -> bool {
        matches!(
            self,
            ActivityLine::UserPrompt(_)
                | ActivityLine::AssistantText(_)
                | ActivityLine::CodeBlock { .. }
        )
    }

    /// Check if this line is a system event (info/warning/error/tool/round/plan).
    pub fn is_system(&self) -> bool {
        !self.is_conversational()
    }

    /// Extract the searchable text content of this line.
    pub fn text_content(&self) -> String {
        match self {
            ActivityLine::UserPrompt(s) => s.clone(),
            ActivityLine::AssistantText(s) => s.clone(),
            ActivityLine::CodeBlock { lang, code } => format!("{lang}\n{code}"),
            ActivityLine::Info(s) => s.clone(),
            ActivityLine::Warning { message, hint } => {
                let mut s = message.clone();
                if let Some(h) = hint {
                    s.push(' ');
                    s.push_str(h);
                }
                s
            }
            ActivityLine::Error { message, hint } => {
                let mut s = message.clone();
                if let Some(h) = hint {
                    s.push(' ');
                    s.push_str(h);
                }
                s
            }
            ActivityLine::RoundSeparator(n) => format!("Round {n}"),
            ActivityLine::ToolExec { name, input_preview, result, .. } => {
                let mut s = format!("{name} {input_preview}");
                if let Some(r) = result {
                    s.push(' ');
                    s.push_str(&r.content);
                }
                s
            }
            ActivityLine::PlanOverview { goal, .. } => goal.clone(),
        }
    }
}

// ── Markdown rendering helpers ──

/// Render a single line of text with markdown formatting.
/// Accepts palette colors to avoid hardcoded Color:: values.
pub fn render_md_line(text: &str, c_text: Color, c_accent: Color, c_warning: Color, c_muted: Color) -> Line<'static> {
    // Headers
    if let Some(rest) = text.strip_prefix("### ") {
        return Line::from(Span::styled(
            rest.to_string(),
            Style::default()
                .fg(c_text)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if let Some(rest) = text.strip_prefix("## ") {
        return Line::from(Span::styled(
            rest.to_string(),
            Style::default()
                .fg(c_accent)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if let Some(rest) = text.strip_prefix("# ") {
        return Line::from(Span::styled(
            rest.to_string(),
            Style::default()
                .fg(c_accent)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        ));
    }

    // Horizontal rule
    let trimmed = text.trim();
    if (trimmed == "---" || trimmed == "***" || trimmed == "___") && trimmed.len() >= 3 {
        return Line::from(Span::styled(
            "────────────────────────────────────────",
            Style::default().fg(c_muted),
        ));
    }

    // Blockquote
    if let Some(rest) = text.strip_prefix("> ") {
        let mut spans = vec![Span::styled("│ ", Style::default().fg(c_muted))];
        spans.extend(parse_md_spans(rest, c_warning).into_iter().map(|s| {
            Span::styled(
                s.content,
                s.style
                    .fg(c_muted)
                    .add_modifier(Modifier::ITALIC),
            )
        }));
        return Line::from(spans);
    }

    // Unordered list
    if text.starts_with("- ") || text.starts_with("* ") {
        let rest = &text[2..];
        let mut spans = vec![Span::styled("  • ", Style::default().fg(c_accent))];
        spans.extend(parse_md_spans(rest, c_warning));
        return Line::from(spans);
    }

    // Numbered list (e.g. "1. item", "12. item")
    if let Some(dot_pos) = text.find(". ") {
        let prefix = &text[..dot_pos];
        if !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()) {
            let rest = &text[dot_pos + 2..];
            let mut spans = vec![Span::styled(
                format!("  {prefix}. "),
                Style::default().fg(c_accent),
            )];
            spans.extend(parse_md_spans(rest, c_warning));
            return Line::from(spans);
        }
    }

    // Regular text with inline formatting
    Line::from(parse_md_spans(text, c_warning))
}

/// Parse inline markdown: **bold**, *italic*, `code`.
/// `c_code` is the color used for inline code spans.
pub fn parse_md_spans(text: &str, c_code: Color) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut buf = String::new();
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Bold: **text**
        if i + 1 < len && chars[i] == '*' && chars[i + 1] == '*' {
            if !buf.is_empty() {
                spans.push(Span::raw(std::mem::take(&mut buf)));
            }
            i += 2;
            while i + 1 < len && !(chars[i] == '*' && chars[i + 1] == '*') {
                buf.push(chars[i]);
                i += 1;
            }
            if i + 1 < len {
                i += 2; // skip closing **
            }
            if !buf.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut buf),
                    Style::default().add_modifier(Modifier::BOLD),
                ));
            }
        }
        // Inline code: `text`
        else if chars[i] == '`' {
            if !buf.is_empty() {
                spans.push(Span::raw(std::mem::take(&mut buf)));
            }
            i += 1;
            while i < len && chars[i] != '`' {
                buf.push(chars[i]);
                i += 1;
            }
            if i < len {
                i += 1; // skip closing `
            }
            if !buf.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut buf),
                    Style::default().fg(c_code),
                ));
            }
        }
        // Italic: *text* (single *, not followed by another *)
        else if chars[i] == '*' && (i + 1 >= len || chars[i + 1] != '*') {
            if !buf.is_empty() {
                spans.push(Span::raw(std::mem::take(&mut buf)));
            }
            i += 1;
            while i < len && chars[i] != '*' {
                buf.push(chars[i]);
                i += 1;
            }
            if i < len {
                i += 1; // skip closing *
            }
            if !buf.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut buf),
                    Style::default().add_modifier(Modifier::ITALIC),
                ));
            }
        } else {
            buf.push(chars[i]);
            i += 1;
        }
    }

    if !buf.is_empty() {
        spans.push(Span::raw(buf));
    }

    if spans.is_empty() {
        spans.push(Span::raw(String::new()));
    }

    spans
}
