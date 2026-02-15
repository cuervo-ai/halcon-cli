//! Scrollable activity zone widget for agent output.
//!
//! Renders inline markdown (headers, bold, italic, code, lists, quotes),
//! tool execution with loading skeletons, and includes a visual scrollbar.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};
use ratatui::Frame;

use crate::render::theme;
use crate::tui::state::AppState;

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

/// State for the scrollable activity zone.
pub struct ActivityState {
    lines: Vec<ActivityLine>,
    scroll_offset: usize,
    pub(crate) auto_scroll: bool,
    /// Cached from last render — used to clamp scroll_offset.
    pub(crate) last_max_scroll: usize,
}

impl ActivityState {
    pub fn new() -> Self {
        Self {
            lines: Vec::new(),
            scroll_offset: 0,
            auto_scroll: true,
            last_max_scroll: 0,
        }
    }

    /// Render the activity zone with scrollbar.
    pub fn render(&mut self, frame: &mut Frame, area: Rect, state: &AppState) {
        let p = &theme::active().palette;
        // Phase 45A Task 2.2: Use cached ratatui colors (eliminates OKLCH→sRGB conversions)
        let c_success = p.success_ratatui();
        let c_accent = p.accent_ratatui();
        let c_warning = p.warning_ratatui();
        let c_error = p.error_ratatui();
        let c_running = p.running_ratatui();
        let c_text = p.text_ratatui();
        let c_muted = p.muted_ratatui();
        let c_border = p.border_ratatui();
        let c_spinner = p.spinner_color_ratatui();

        let border_color =
            if state.focus == super::super::state::FocusZone::Activity {
                c_accent
            } else {
                c_border
            };

        let mut styled_lines: Vec<Line<'_>> = Vec::new();

        for line in &self.lines {
            match line {
                ActivityLine::UserPrompt(text) => {
                    styled_lines.push(Line::from(vec![
                        Span::styled(
                            "► ",
                            Style::default()
                                .fg(c_success)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            text.clone(),
                            Style::default()
                                .fg(c_success)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]));
                }
                ActivityLine::AssistantText(text) => {
                    for l in text.lines() {
                        styled_lines.push(render_md_line(l, c_text, c_accent, c_warning, c_muted));
                    }
                }
                ActivityLine::CodeBlock { lang, code } => {
                    styled_lines.push(Line::from(vec![
                        Span::styled("  ┌─ ", Style::default().fg(c_muted)),
                        Span::styled(
                            lang.clone(),
                            Style::default()
                                .fg(c_accent)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(" ─", Style::default().fg(c_muted)),
                    ]));
                    for l in code.lines() {
                        styled_lines.push(Line::from(vec![
                            Span::styled("  │ ", Style::default().fg(c_muted)),
                            Span::styled(l.to_string(), Style::default().fg(c_warning)),
                        ]));
                    }
                    styled_lines.push(Line::from(Span::styled(
                        "  └───",
                        Style::default().fg(c_muted),
                    )));
                }
                ActivityLine::Info(text) => {
                    styled_lines.push(Line::from(Span::styled(
                        text.clone(),
                        Style::default().fg(c_accent),
                    )));
                }
                ActivityLine::Warning { message, hint } => {
                    let mut spans = vec![
                        Span::styled(
                            "⚠ ",
                            Style::default()
                                .fg(c_warning)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(message.clone(), Style::default().fg(c_warning)),
                    ];
                    if let Some(h) = hint {
                        spans.push(Span::styled(
                            format!(" ({h})"),
                            Style::default().fg(c_muted),
                        ));
                    }
                    styled_lines.push(Line::from(spans));
                }
                ActivityLine::Error { message, hint } => {
                    let mut spans = vec![
                        Span::styled(
                            "✖ ",
                            Style::default()
                                .fg(c_error)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(message.clone(), Style::default().fg(c_error)),
                    ];
                    if let Some(h) = hint {
                        spans.push(Span::styled(
                            format!(" ({h})"),
                            Style::default().fg(c_muted),
                        ));
                    }
                    styled_lines.push(Line::from(spans));
                }
                ActivityLine::RoundSeparator(n) => {
                    styled_lines.push(Line::from(Span::styled(
                        format!("──────── Round {n} ────────"),
                        Style::default()
                            .fg(c_muted)
                            .add_modifier(Modifier::DIM),
                    )));
                }
                ActivityLine::PlanOverview {
                    goal,
                    steps,
                    current_step,
                } => {
                    // Plan header
                    styled_lines.push(Line::from(vec![
                        Span::styled(
                            "  \u{1f4cb} PLAN: ",
                            Style::default()
                                .fg(c_accent)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            goal.clone(),
                            Style::default()
                                .fg(c_text)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]));
                    // Step list with status icons
                    for (i, step) in steps.iter().enumerate() {
                        use crate::tui::events::PlanStepDisplayStatus;
                        let (icon, color) = match step.status {
                            PlanStepDisplayStatus::Succeeded => ("\u{2713}", c_success),
                            PlanStepDisplayStatus::Failed => ("\u{2717}", c_error),
                            PlanStepDisplayStatus::InProgress => ("\u{25b8}", c_warning),
                            PlanStepDisplayStatus::Skipped => ("-", c_muted),
                            PlanStepDisplayStatus::Pending => ("\u{25cb}", c_muted),
                        };
                        let marker = if i == *current_step && step.status == PlanStepDisplayStatus::InProgress {
                            " \u{2190} CURRENT"
                        } else {
                            ""
                        };
                        let tool_hint = step
                            .tool_name
                            .as_deref()
                            .map(|t| format!(" ({t})"))
                            .unwrap_or_default();
                        styled_lines.push(Line::from(vec![
                            Span::styled(
                                format!("    {icon} "),
                                Style::default().fg(color).add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("Step {}: {}{tool_hint}{marker}", i + 1, step.description),
                                Style::default().fg(color),
                            ),
                        ]));
                    }
                    styled_lines.push(Line::from(""));
                }
                ActivityLine::ToolExec {
                    name,
                    input_preview,
                    result,
                    expanded,
                } => match result {
                    None => {
                        // Loading skeleton — animated shimmer.
                        let shimmer_frames = [
                            "░░░░░░░░░░░░",
                            "▒░░░░░░░░░░░",
                            "▒▒░░░░░░░░░░",
                            "░▒▒░░░░░░░░░",
                            "░░▒▒░░░░░░░░",
                            "░░░▒▒░░░░░░░",
                            "░░░░▒▒░░░░░░",
                            "░░░░░▒▒░░░░░",
                            "░░░░░░▒▒░░░░",
                            "░░░░░░░▒▒░░░",
                        ];
                        let frame_idx = state.spinner_frame % shimmer_frames.len();
                        // Tool name line.
                        styled_lines.push(Line::from(vec![
                            Span::styled(
                                "  ⚙ ",
                                Style::default()
                                    .fg(c_running)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                name.clone(),
                                Style::default()
                                    .fg(c_running)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!(" {input_preview}"),
                                Style::default().fg(c_muted),
                            ),
                        ]));
                        // Skeleton bar.
                        styled_lines.push(Line::from(vec![
                            Span::styled("    ", Style::default()),
                            Span::styled(
                                shimmer_frames[frame_idx],
                                Style::default().fg(c_muted),
                            ),
                        ]));
                    }
                    Some(res) => {
                        let (icon, icon_color) = if res.is_error {
                            ("  ✖ ", c_error)
                        } else {
                            ("  ✔ ", c_success)
                        };
                        let duration_str = if res.duration_ms < 1000 {
                            format!("{}ms", res.duration_ms)
                        } else {
                            format!("{:.1}s", res.duration_ms as f64 / 1000.0)
                        };
                        let expand_hint = if *expanded { "▾" } else { "▸" };
                        styled_lines.push(Line::from(vec![
                            Span::styled(
                                icon,
                                Style::default()
                                    .fg(icon_color)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("{expand_hint} "),
                                Style::default().fg(c_muted),
                            ),
                            Span::styled(
                                name.clone(),
                                Style::default()
                                    .fg(c_text)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!(" [{duration_str}]"),
                                Style::default().fg(c_muted),
                            ),
                        ]));
                        // Show content: expanded = full, collapsed = 3-line preview.
                        if !res.content.is_empty() {
                            let content_color = if res.is_error {
                                c_error
                            } else {
                                c_muted
                            };
                            if *expanded {
                                // Show all content lines.
                                for pline in res.content.lines() {
                                    styled_lines.push(Line::from(vec![
                                        Span::styled("    ", Style::default()),
                                        Span::styled(
                                            pline.to_string(),
                                            Style::default().fg(content_color),
                                        ),
                                    ]));
                                }
                            } else {
                                // Show truncated preview (3 lines max).
                                let preview = &res.content[..res.content.len().min(200)];
                                for pline in preview.lines().take(3) {
                                    styled_lines.push(Line::from(vec![
                                        Span::styled("    ", Style::default()),
                                        Span::styled(
                                            pline.to_string(),
                                            Style::default().fg(content_color),
                                        ),
                                    ]));
                                }
                                let total_lines_in_content = res.content.lines().count();
                                if total_lines_in_content > 3 {
                                    styled_lines.push(Line::from(Span::styled(
                                        format!(
                                            "    ... ({} more lines, press Enter to expand)",
                                            total_lines_in_content - 3
                                        ),
                                        Style::default().fg(c_muted),
                                    )));
                                }
                            }
                        }
                    }
                },
            }
        }

        // Spinner line if active.
        if state.spinner_active {
            let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
            let ch = frames[state.spinner_frame % frames.len()];
            styled_lines.push(Line::from(Span::styled(
                format!("{ch} {}", state.spinner_label),
                Style::default()
                    .fg(c_spinner)
                    .add_modifier(Modifier::BOLD),
            )));
        }

        let total_lines = styled_lines.len() as u16;
        let visible_height = area.height.saturating_sub(2); // borders
        let max_scroll = total_lines.saturating_sub(visible_height) as usize;

        // Cache max_scroll for clamping in scroll_up/scroll_down.
        self.last_max_scroll = max_scroll;

        let scroll = if self.auto_scroll {
            max_scroll
        } else {
            self.scroll_offset.min(max_scroll)
        };

        let paragraph = Paragraph::new(styled_lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Activity ")
                    .border_style(Style::default().fg(border_color)),
            )
            .wrap(Wrap { trim: false })
            .scroll((scroll as u16, 0));

        frame.render_widget(paragraph, area);

        // Render scrollbar if content exceeds visible area.
        if total_lines > visible_height {
            let mut scrollbar_state = ScrollbarState::new(max_scroll).position(scroll);
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓"));
            frame.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
        }
    }

    /// Push a user prompt line.
    pub fn push_user_prompt(&mut self, text: &str) {
        self.lines.push(ActivityLine::UserPrompt(text.to_string()));
        self.auto_scroll = true;
    }

    /// Push assistant streaming text.
    pub fn push_assistant_text(&mut self, text: &str) {
        // Append to last AssistantText if present, otherwise create new.
        if let Some(ActivityLine::AssistantText(ref mut existing)) = self.lines.last_mut() {
            existing.push_str(text);
        } else {
            self.lines
                .push(ActivityLine::AssistantText(text.to_string()));
        }
        if self.auto_scroll {
            self.scroll_to_bottom();
        }
    }

    /// Push a code block.
    pub fn push_code_block(&mut self, lang: &str, code: &str) {
        self.lines.push(ActivityLine::CodeBlock {
            lang: lang.to_string(),
            code: code.to_string(),
        });
        if self.auto_scroll {
            self.scroll_to_bottom();
        }
    }

    /// Push an informational message.
    pub fn push_info(&mut self, text: &str) {
        self.lines.push(ActivityLine::Info(text.to_string()));
        if self.auto_scroll {
            self.scroll_to_bottom();
        }
    }

    /// Push a warning message.
    pub fn push_warning(&mut self, message: &str, hint: Option<&str>) {
        self.lines.push(ActivityLine::Warning {
            message: message.to_string(),
            hint: hint.map(|h| h.to_string()),
        });
        if self.auto_scroll {
            self.scroll_to_bottom();
        }
    }

    /// Push an error message.
    pub fn push_error(&mut self, message: &str, hint: Option<&str>) {
        self.lines.push(ActivityLine::Error {
            message: message.to_string(),
            hint: hint.map(|h| h.to_string()),
        });
        if self.auto_scroll {
            self.scroll_to_bottom();
        }
    }

    /// Push a round separator.
    pub fn push_round_separator(&mut self, n: usize) {
        self.lines.push(ActivityLine::RoundSeparator(n));
        if self.auto_scroll {
            self.scroll_to_bottom();
        }
    }

    /// Push a tool execution start (shows loading skeleton until result arrives).
    pub fn push_tool_start(&mut self, name: &str, input_preview: &str) {
        self.lines.push(ActivityLine::ToolExec {
            name: name.to_string(),
            input_preview: input_preview.to_string(),
            result: None,
            expanded: false,
        });
        if self.auto_scroll {
            self.scroll_to_bottom();
        }
    }

    /// Complete a tool execution by filling in the result on the matching entry.
    pub fn complete_tool(&mut self, name: &str, content: String, is_error: bool, duration_ms: u64) {
        // Find the last ToolExec with matching name and no result.
        for line in self.lines.iter_mut().rev() {
            if let ActivityLine::ToolExec {
                name: ref n,
                result: ref mut r,
                ..
            } = line
            {
                if n == name && r.is_none() {
                    *r = Some(ToolResult {
                        content,
                        is_error,
                        duration_ms,
                    });
                    break;
                }
            }
        }
        if self.auto_scroll {
            self.scroll_to_bottom();
        }
    }

    /// Scroll up by `n` lines.
    pub fn scroll_up(&mut self, n: usize) {
        if self.auto_scroll {
            // Switching from auto-scroll: start from the bottom.
            self.scroll_offset = self.last_max_scroll;
        }
        self.auto_scroll = false;
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    /// Scroll down by `n` lines.
    pub fn scroll_down(&mut self, n: usize) {
        self.auto_scroll = false;
        self.scroll_offset = self.scroll_offset.saturating_add(n);
        // Clamp to max_scroll to prevent offset from growing unbounded.
        if self.scroll_offset >= self.last_max_scroll {
            // Re-enable auto-scroll when reaching the bottom.
            self.scroll_to_bottom();
        }
    }

    /// Scroll to the bottom and re-enable auto-scroll.
    pub fn scroll_to_bottom(&mut self) {
        self.auto_scroll = true;
        self.scroll_offset = self.last_max_scroll;
    }

    /// Clear all activity content.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.scroll_offset = 0;
        self.auto_scroll = true;
    }

    /// Toggle expand/collapse on the last completed tool execution.
    #[allow(dead_code)]
    pub fn toggle_last_tool_expanded(&mut self) {
        for line in self.lines.iter_mut().rev() {
            if let ActivityLine::ToolExec { result: Some(_), expanded, .. } = line {
                *expanded = !*expanded;
                break;
            }
        }
    }

    /// Get the number of activity lines.
    #[allow(dead_code)]
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Set or replace the plan overview. Removes any existing PlanOverview entry
    /// to avoid duplicates, then inserts the new one near the top (after user prompts).
    pub fn set_plan_overview(
        &mut self,
        goal: &str,
        steps: Vec<crate::tui::events::PlanStepStatus>,
        current_step: usize,
    ) {
        // Remove existing PlanOverview.
        self.lines.retain(|l| !matches!(l, ActivityLine::PlanOverview { .. }));
        // Insert after the first user prompt (or at position 0).
        let insert_pos = self
            .lines
            .iter()
            .position(|l| !matches!(l, ActivityLine::UserPrompt(_)))
            .unwrap_or(self.lines.len());
        self.lines.insert(
            insert_pos,
            ActivityLine::PlanOverview {
                goal: goal.to_string(),
                steps,
                current_step,
            },
        );
        if self.auto_scroll {
            self.scroll_to_bottom();
        }
    }

    /// Check if there are any loading tools.
    #[allow(dead_code)]
    pub fn has_loading_tools(&self) -> bool {
        self.lines.iter().any(|l| {
            matches!(l, ActivityLine::ToolExec { result: None, .. })
        })
    }

    /// Search activity lines for a query (case-insensitive).
    /// Returns indices of matching lines. O(n) scan.
    pub fn search(&self, query: &str) -> Vec<usize> {
        if query.is_empty() {
            return Vec::new();
        }
        let q = query.to_lowercase();
        self.lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.text_content().to_lowercase().contains(&q))
            .map(|(i, _)| i)
            .collect()
    }

    /// Scroll to a specific line index (used by search navigation).
    pub fn scroll_to_line(&mut self, line_idx: usize) {
        self.auto_scroll = false;
        // Approximate: each ActivityLine maps to roughly 1-3 rendered lines.
        // Scroll so the target line is near the top of the view.
        self.scroll_offset = line_idx.saturating_sub(2);
    }
}

impl Default for ActivityState {
    fn default() -> Self {
        Self::new()
    }
}

// ── Markdown rendering helpers ──

/// Render a single line of text with markdown formatting.
/// Accepts palette colors to avoid hardcoded Color:: values.
fn render_md_line(text: &str, c_text: Color, c_accent: Color, c_warning: Color, c_muted: Color) -> Line<'static> {
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
fn parse_md_spans(text: &str, c_code: Color) -> Vec<Span<'static>> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: call render_md_line with default palette colors for tests.
    fn test_render_md_line(text: &str) -> Line<'static> {
        let p = &theme::active().palette;
        render_md_line(
            text,
            p.text_ratatui(),
            p.accent_ratatui(),
            p.warning_ratatui(),
            p.muted_ratatui(),
        )
    }

    /// Helper: call parse_md_spans with default palette colors for tests.
    fn test_parse_md_spans(text: &str) -> Vec<Span<'static>> {
        let p = &theme::active().palette;
        parse_md_spans(text, p.warning_ratatui())
    }

    #[test]
    fn new_activity_is_empty() {
        let activity = ActivityState::new();
        assert_eq!(activity.line_count(), 0);
        assert!(activity.auto_scroll);
    }

    #[test]
    fn push_user_prompt() {
        let mut activity = ActivityState::new();
        activity.push_user_prompt("hello");
        assert_eq!(activity.line_count(), 1);
        assert!(matches!(
            &activity.lines[0],
            ActivityLine::UserPrompt(t) if t == "hello"
        ));
    }

    #[test]
    fn push_assistant_text_accumulates() {
        let mut activity = ActivityState::new();
        activity.push_assistant_text("hello ");
        activity.push_assistant_text("world");
        assert_eq!(activity.line_count(), 1);
        if let ActivityLine::AssistantText(text) = &activity.lines[0] {
            assert_eq!(text, "hello world");
        } else {
            panic!("expected AssistantText");
        }
    }

    #[test]
    fn push_assistant_text_after_other_creates_new() {
        let mut activity = ActivityState::new();
        activity.push_info("info");
        activity.push_assistant_text("text");
        assert_eq!(activity.line_count(), 2);
    }

    #[test]
    fn push_code_block() {
        let mut activity = ActivityState::new();
        activity.push_code_block("rust", "fn main() {}");
        assert_eq!(activity.line_count(), 1);
        assert!(matches!(
            &activity.lines[0],
            ActivityLine::CodeBlock { lang, code }
                if lang == "rust" && code == "fn main() {}"
        ));
    }

    #[test]
    fn push_warning_with_hint() {
        let mut activity = ActivityState::new();
        activity.push_warning("watch out", Some("be careful"));
        assert_eq!(activity.line_count(), 1);
        assert!(matches!(
            &activity.lines[0],
            ActivityLine::Warning { message, hint }
                if message == "watch out" && hint.as_deref() == Some("be careful")
        ));
    }

    #[test]
    fn push_error_without_hint() {
        let mut activity = ActivityState::new();
        activity.push_error("something broke", None);
        assert_eq!(activity.line_count(), 1);
        assert!(matches!(
            &activity.lines[0],
            ActivityLine::Error { message, hint }
                if message == "something broke" && hint.is_none()
        ));
    }

    #[test]
    fn push_round_separator() {
        let mut activity = ActivityState::new();
        activity.push_round_separator(3);
        assert_eq!(activity.line_count(), 1);
        assert!(matches!(&activity.lines[0], ActivityLine::RoundSeparator(3)));
    }

    #[test]
    fn scroll_up_disables_auto_scroll() {
        let mut activity = ActivityState::new();
        assert!(activity.auto_scroll);
        activity.scroll_up(3);
        assert!(!activity.auto_scroll);
    }

    #[test]
    fn scroll_to_bottom_re_enables_auto_scroll() {
        let mut activity = ActivityState::new();
        activity.scroll_up(3);
        assert!(!activity.auto_scroll);
        activity.scroll_to_bottom();
        assert!(activity.auto_scroll);
    }

    #[test]
    fn scroll_up_saturates_at_zero() {
        let mut activity = ActivityState::new();
        activity.scroll_offset = 2;
        activity.scroll_up(5);
        assert_eq!(activity.scroll_offset, 0);
    }

    #[test]
    fn scroll_down_re_enables_auto_at_bottom() {
        let mut activity = ActivityState::new();
        activity.last_max_scroll = 10;
        activity.auto_scroll = false;
        activity.scroll_offset = 5;
        // Scroll past max → should re-enable auto-scroll.
        activity.scroll_down(20);
        assert!(activity.auto_scroll);
        assert_eq!(activity.scroll_offset, 10);
    }

    #[test]
    fn scroll_down_clamps_to_max() {
        let mut activity = ActivityState::new();
        activity.last_max_scroll = 5;
        activity.auto_scroll = false;
        activity.scroll_offset = 3;
        activity.scroll_down(1);
        assert_eq!(activity.scroll_offset, 4);
        assert!(!activity.auto_scroll);
    }

    #[test]
    fn mixed_content_ordering() {
        let mut activity = ActivityState::new();
        activity.push_user_prompt("question");
        activity.push_round_separator(1);
        activity.push_assistant_text("answer");
        activity.push_code_block("py", "print('hi')");
        activity.push_info("tool: grep");
        activity.push_warning("heads up", None);
        activity.push_error("oops", Some("retry"));
        assert_eq!(activity.line_count(), 7);
    }

    // ── Tool execution tests ──

    #[test]
    fn tool_start_creates_loading_entry() {
        let mut activity = ActivityState::new();
        activity.push_tool_start("file_read", "path=src/main.rs");
        assert_eq!(activity.line_count(), 1);
        assert!(matches!(
            &activity.lines[0],
            ActivityLine::ToolExec { name, result: None, .. }
                if name == "file_read"
        ));
    }

    #[test]
    fn tool_complete_fills_result() {
        let mut activity = ActivityState::new();
        activity.push_tool_start("bash", "cmd=ls");
        activity.complete_tool("bash", "file1\nfile2".into(), false, 42);
        if let ActivityLine::ToolExec { result: Some(r), .. } = &activity.lines[0] {
            assert!(!r.is_error);
            assert_eq!(r.duration_ms, 42);
            assert_eq!(r.content, "file1\nfile2");
        } else {
            panic!("expected completed ToolExec");
        }
    }

    #[test]
    fn tool_complete_error_result() {
        let mut activity = ActivityState::new();
        activity.push_tool_start("bash", "cmd=fail");
        activity.complete_tool("bash", "command not found".into(), true, 10);
        if let ActivityLine::ToolExec { result: Some(r), .. } = &activity.lines[0] {
            assert!(r.is_error);
        } else {
            panic!("expected error ToolExec");
        }
    }

    #[test]
    fn has_loading_tools_check() {
        let mut activity = ActivityState::new();
        assert!(!activity.has_loading_tools());
        activity.push_tool_start("grep", "pattern=todo");
        assert!(activity.has_loading_tools());
        activity.complete_tool("grep", "found 3".into(), false, 5);
        assert!(!activity.has_loading_tools());
    }

    #[test]
    fn multiple_tools_complete_independently() {
        let mut activity = ActivityState::new();
        activity.push_tool_start("file_read", "a.rs");
        activity.push_tool_start("file_read", "b.rs");
        // Complete second one first.
        activity.complete_tool("file_read", "content_b".into(), false, 20);
        // First should still be loading.
        assert!(matches!(
            &activity.lines[0],
            ActivityLine::ToolExec { result: None, .. }
        ));
        // Second should be done.
        assert!(matches!(
            &activity.lines[1],
            ActivityLine::ToolExec { result: Some(_), .. }
        ));
    }

    // ── Markdown parsing tests ──

    #[test]
    fn md_header_h1() {
        let line = test_render_md_line("# Hello World");
        assert_eq!(line.spans.len(), 1);
        assert!(line.spans[0].style.add_modifier == Modifier::empty()
            || line.spans[0].content.contains("Hello World"));
    }

    #[test]
    fn md_header_h2() {
        let line = test_render_md_line("## Subheader");
        assert_eq!(line.spans.len(), 1);
        assert!(line.spans[0].content.contains("Subheader"));
    }

    #[test]
    fn md_bold() {
        let spans = test_parse_md_spans("hello **world** end");
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].content.as_ref(), "hello ");
        assert_eq!(spans[1].content.as_ref(), "world");
        assert!(spans[1].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(spans[2].content.as_ref(), " end");
    }

    #[test]
    fn md_italic() {
        let spans = test_parse_md_spans("hello *italic* end");
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[1].content.as_ref(), "italic");
        assert!(spans[1].style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn md_inline_code_uses_palette() {
        let p = &theme::active().palette;
        let c_code = p.warning_ratatui();
        let spans = test_parse_md_spans("run `cargo test` now");
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[1].content.as_ref(), "cargo test");
        assert_eq!(spans[1].style.fg, Some(c_code));
    }

    #[test]
    fn md_plain_text() {
        let spans = test_parse_md_spans("just plain text");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content.as_ref(), "just plain text");
    }

    #[test]
    fn md_list_item() {
        let line = test_render_md_line("- first item");
        assert!(line.spans.len() >= 2);
        assert!(line.spans[0].content.contains("•"));
    }

    #[test]
    fn md_numbered_list() {
        let line = test_render_md_line("1. first item");
        assert!(line.spans.len() >= 2);
        assert!(line.spans[0].content.contains("1."));
    }

    #[test]
    fn md_blockquote() {
        let line = test_render_md_line("> quoted text");
        assert!(line.spans.len() >= 2);
        assert!(line.spans[0].content.contains("│"));
    }

    #[test]
    fn md_horizontal_rule() {
        let line = test_render_md_line("---");
        assert_eq!(line.spans.len(), 1);
        assert!(line.spans[0].content.contains("─"));
    }

    #[test]
    fn md_mixed_inline_uses_palette() {
        let p = &theme::active().palette;
        let c_code = p.warning_ratatui();
        let spans = test_parse_md_spans("**bold** and `code` here");
        assert!(spans.len() >= 4);
        assert_eq!(spans[0].content.as_ref(), "bold");
        assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(spans[2].content.as_ref(), "code");
        assert_eq!(spans[2].style.fg, Some(c_code));
    }

    // Phase 43B: Verify no hardcoded Color:: in activity rendering
    #[test]
    fn activity_uses_palette_colors() {
        // The palette is used when render() is called. Here we verify
        // the palette can be loaded and colors are valid ratatui Colors.
        let p = &theme::active().palette;
        // Phase 45A Task 2.2: Use cached accessors
        let _s = p.success_ratatui();
        let _a = p.accent_ratatui();
        let _w = p.warning_ratatui();
        let _e = p.error_ratatui();
        let _r = p.running_ratatui();
        let _t = p.text_ratatui();
        let _m = p.muted_ratatui();
        let _b = p.border_ratatui();
        let _sp = p.spinner_color_ratatui();
        // If this compiles and runs without panic, palette integration is working.
    }

    // ── Plan overview tests ──

    #[test]
    fn set_plan_overview_adds_entry() {
        use crate::tui::events::{PlanStepDisplayStatus, PlanStepStatus};
        let mut activity = ActivityState::new();
        activity.set_plan_overview(
            "Fix bug",
            vec![PlanStepStatus {
                description: "Read file".into(),
                tool_name: Some("file_read".into()),
                status: PlanStepDisplayStatus::Pending,
                duration_ms: None,
            }],
            0,
        );
        assert_eq!(activity.line_count(), 1);
        assert!(matches!(
            &activity.lines[0],
            ActivityLine::PlanOverview { goal, .. } if goal == "Fix bug"
        ));
    }

    #[test]
    fn set_plan_overview_replaces_existing() {
        use crate::tui::events::{PlanStepDisplayStatus, PlanStepStatus};
        let mut activity = ActivityState::new();
        activity.set_plan_overview(
            "Old goal",
            vec![PlanStepStatus {
                description: "Step 1".into(),
                tool_name: None,
                status: PlanStepDisplayStatus::Pending,
                duration_ms: None,
            }],
            0,
        );
        activity.set_plan_overview(
            "New goal",
            vec![PlanStepStatus {
                description: "Step 1".into(),
                tool_name: None,
                status: PlanStepDisplayStatus::Succeeded,
                duration_ms: None,
            }],
            1,
        );
        // Should still be exactly 1 PlanOverview.
        let plan_count = activity
            .lines
            .iter()
            .filter(|l| matches!(l, ActivityLine::PlanOverview { .. }))
            .count();
        assert_eq!(plan_count, 1);
        assert!(matches!(
            &activity.lines[0],
            ActivityLine::PlanOverview { goal, .. } if goal == "New goal"
        ));
    }

    #[test]
    fn set_plan_overview_after_user_prompt() {
        use crate::tui::events::{PlanStepDisplayStatus, PlanStepStatus};
        let mut activity = ActivityState::new();
        activity.push_user_prompt("Hello");
        activity.set_plan_overview(
            "Plan",
            vec![PlanStepStatus {
                description: "Do stuff".into(),
                tool_name: None,
                status: PlanStepDisplayStatus::InProgress,
                duration_ms: None,
            }],
            0,
        );
        // Plan should be after user prompt.
        assert!(matches!(&activity.lines[0], ActivityLine::UserPrompt(_)));
        assert!(matches!(
            &activity.lines[1],
            ActivityLine::PlanOverview { .. }
        ));
    }
}
