//! Modal overlay system for the TUI.
//!
//! Overlays render as centered floating boxes on top of the main layout.
//! While an overlay is active, it captures all keyboard input.
//! Pressing Esc dismisses the active overlay.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::render::theme;
use crate::tui::constants;

/// The kind of overlay currently displayed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayKind {
    /// Help overlay showing keybinding reference (F1).
    Help,
    /// Command palette with substring search (Ctrl+P).
    CommandPalette,
    /// Search overlay for activity zone (Ctrl+F).
    Search,
    /// Permission approval prompt for a tool.
    PermissionPrompt { tool: String },
}

/// State for the overlay system.
#[derive(Debug, Clone)]
pub struct OverlayState {
    /// Currently active overlay, or None.
    pub active: Option<OverlayKind>,
    /// Text input for search/command palette.
    pub input: String,
    /// Cursor position within input.
    pub cursor: usize,
    /// Selected index in a list (command palette).
    pub selected: usize,
    /// Filtered items for command palette.
    pub filtered_items: Vec<OverlayItem>,
}

/// An item in the command palette list.
#[derive(Debug, Clone)]
pub struct OverlayItem {
    pub label: String,
    pub description: String,
    pub action: String,
}

impl OverlayState {
    pub fn new() -> Self {
        Self {
            active: None,
            input: String::new(),
            cursor: 0,
            selected: 0,
            filtered_items: Vec::new(),
        }
    }

    /// Open an overlay of the given kind.
    pub fn open(&mut self, kind: OverlayKind) {
        self.active = Some(kind);
        self.input.clear();
        self.cursor = 0;
        self.selected = 0;
        self.filtered_items.clear();
    }

    /// Close the active overlay.
    pub fn close(&mut self) {
        self.active = None;
        self.input.clear();
        self.cursor = 0;
        self.selected = 0;
        self.filtered_items.clear();
    }

    /// Whether an overlay is currently active.
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// Type a character into the overlay input.
    pub fn type_char(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Delete the character before cursor.
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            let prev = self.input[..self.cursor]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.input.drain(prev..self.cursor);
            self.cursor = prev;
        }
    }

    /// Move selection up in a list.
    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down in a list.
    pub fn select_next(&mut self, max: usize) {
        if self.selected + 1 < max {
            self.selected += 1;
        }
    }
}

impl Default for OverlayState {
    fn default() -> Self {
        Self::new()
    }
}

/// Get the default set of command palette items.
pub fn default_commands() -> Vec<OverlayItem> {
    vec![
        OverlayItem {
            label: "/help".into(),
            description: "Show help and keybindings".into(),
            action: "help".into(),
        },
        OverlayItem {
            label: "/model".into(),
            description: "Change the active model".into(),
            action: "model".into(),
        },
        OverlayItem {
            label: "/mode".into(),
            description: "Cycle UI mode (Minimal/Standard/Expert)".into(),
            action: "mode".into(),
        },
        OverlayItem {
            label: "/plan".into(),
            description: "Show or manage the current plan".into(),
            action: "plan".into(),
        },
        OverlayItem {
            label: "/clear".into(),
            description: "Clear the activity zone".into(),
            action: "clear".into(),
        },
        OverlayItem {
            label: "/quit".into(),
            description: "Exit the TUI".into(),
            action: "quit".into(),
        },
        OverlayItem {
            label: "/panel".into(),
            description: "Toggle the side panel".into(),
            action: "panel".into(),
        },
        OverlayItem {
            label: "/search".into(),
            description: "Search activity text (Ctrl+F)".into(),
            action: "search".into(),
        },
    ]
}

/// Filter command items by query using case-insensitive substring matching.
pub fn filter_commands(items: &[OverlayItem], query: &str) -> Vec<OverlayItem> {
    if query.is_empty() {
        return items.to_vec();
    }
    let q = query.to_lowercase();
    items
        .iter()
        .filter(|item| {
            item.label.to_lowercase().contains(&q)
                || item.description.to_lowercase().contains(&q)
        })
        .cloned()
        .collect()
}

/// Calculate a centered rectangle within the given area.
pub fn centered_rect(area: Rect, width_pct: u16, height_pct: u16) -> Rect {
    let v_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - height_pct) / 2),
            Constraint::Percentage(height_pct),
            Constraint::Percentage((100 - height_pct) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - width_pct) / 2),
            Constraint::Percentage(width_pct),
            Constraint::Percentage((100 - width_pct) / 2),
        ])
        .split(v_chunks[1])[1]
}

/// Render the help overlay.
/// Build a help section with header and keybinding list.
fn build_help_section<'a>(
    header: &'a str,
    bindings: &'a [(&'a str, &'a str)],
    c_accent: ratatui::style::Color,
    c_text: ratatui::style::Color,
) -> Vec<Line<'a>> {
    let mut lines = vec![];
    lines.push(Line::from(Span::styled(
        header,
        Style::default().fg(c_accent).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
    for &(key, desc) in bindings {
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<12}", key), Style::default().fg(c_accent)),
            Span::styled(desc, Style::default().fg(c_text)),
        ]));
    }
    lines.push(Line::from(""));
    lines
}

pub fn render_help(frame: &mut Frame, area: Rect) {
    let p = &theme::active().palette;
    let c_border = p.border.to_ratatui_color();
    let c_accent = p.accent.to_ratatui_color();
    let c_text = p.text.to_ratatui_color();
    let c_muted = p.muted.to_ratatui_color();

    let rect = centered_rect(area, 60, 70);
    frame.render_widget(Clear, rect);

    let mut lines = Vec::new();
    lines.extend(build_help_section(
        constants::HELP_HEADER_NAVIGATION,
        constants::HELP_SECTION_NAVIGATION,
        c_accent,
        c_text,
    ));
    lines.extend(build_help_section(
        constants::HELP_HEADER_PANELS,
        constants::HELP_SECTION_PANELS,
        c_accent,
        c_text,
    ));
    lines.extend(build_help_section(
        constants::HELP_HEADER_AGENT,
        constants::HELP_SECTION_AGENT,
        c_accent,
        c_text,
    ));
    lines.extend(build_help_section(
        constants::HELP_HEADER_GENERAL,
        constants::HELP_SECTION_GENERAL,
        c_accent,
        c_text,
    ));

    lines.push(Line::from(Span::styled(
        "  Slash Commands: /help /model /mode /plan /panel /clear /search /quit",
        Style::default().fg(c_muted),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Press Esc to close",
        Style::default().fg(c_muted),
    )));

    let help = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Help (F1) ")
                .border_style(Style::default().fg(c_border)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(help, rect);
}

/// Render the permission approval overlay.
pub fn render_permission_prompt(frame: &mut Frame, area: Rect, tool: &str) {
    let p = &theme::active().palette;
    let c_border = p.border.to_ratatui_color();
    let c_warning = p.warning.to_ratatui_color();
    let c_text = p.text.to_ratatui_color();
    let c_accent = p.accent.to_ratatui_color();

    let rect = centered_rect(area, 50, 30);
    frame.render_widget(Clear, rect);

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Tool requires approval:",
            Style::default().fg(c_warning).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("    {tool}"),
            Style::default().fg(c_text).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  [Y] ", Style::default().fg(c_accent).add_modifier(Modifier::BOLD)),
            Span::styled("Approve", Style::default().fg(c_text)),
            Span::styled("    ", Style::default()),
            Span::styled("[N] ", Style::default().fg(c_warning).add_modifier(Modifier::BOLD)),
            Span::styled("Reject", Style::default().fg(c_text)),
            Span::styled("    ", Style::default()),
            Span::styled("[Esc] ", Style::default().fg(c_text)),
            Span::styled("Dismiss", Style::default().fg(c_text)),
        ]),
    ];

    let prompt = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Permission Required ")
                .border_style(Style::default().fg(c_warning)),
        )
        .alignment(Alignment::Left);

    frame.render_widget(prompt, rect);
}

/// Render the command palette overlay.
pub fn render_command_palette(
    frame: &mut Frame,
    area: Rect,
    input: &str,
    items: &[OverlayItem],
    selected: usize,
) {
    let p = &theme::active().palette;
    let c_border = p.border.to_ratatui_color();
    let c_accent = p.accent.to_ratatui_color();
    let c_text = p.text.to_ratatui_color();
    let c_muted = p.muted.to_ratatui_color();
    let c_running = p.running.to_ratatui_color();

    let rect = centered_rect(area, 60, 50);
    frame.render_widget(Clear, rect);

    let mut lines = vec![
        Line::from(vec![
            Span::styled("  > ", Style::default().fg(c_accent)),
            Span::styled(input, Style::default().fg(c_text)),
            Span::styled("_", Style::default().fg(c_accent)),
        ]),
        Line::from(""),
    ];

    for (i, item) in items.iter().enumerate() {
        let style = if i == selected {
            Style::default().fg(c_running).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(c_text)
        };
        let prefix = if i == selected { "  ▸ " } else { "    " };
        let desc = format!("  {}", item.description);
        lines.push(Line::from(vec![
            Span::styled(prefix, style),
            Span::styled(item.label.clone(), style),
            Span::styled(desc, Style::default().fg(c_muted)),
        ]));
    }

    if items.is_empty() {
        lines.push(Line::from(Span::styled(
            "    (no matches)",
            Style::default().fg(c_muted),
        )));
    }

    let palette = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Command Palette (Ctrl+P) ")
                .border_style(Style::default().fg(c_border)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(palette, rect);
}

/// Render the search overlay with real match count and current position.
pub fn render_search(frame: &mut Frame, area: Rect, query: &str, match_count: usize, current: usize) {
    let p = &theme::active().palette;
    let c_border = p.border.to_ratatui_color();
    let c_accent = p.accent.to_ratatui_color();
    let c_text = p.text.to_ratatui_color();
    let c_muted = p.muted.to_ratatui_color();

    // Search bar at the top of the activity area.
    let search_area = Rect::new(area.x + 2, area.y, area.width.saturating_sub(4).min(60), 3);
    frame.render_widget(Clear, search_area);

    let match_label = if match_count > 0 {
        format!("  ({current}/{match_count})  ↑↓ navigate  Enter next")
    } else if query.is_empty() {
        String::new()
    } else {
        "  (no matches)".into()
    };

    let line = Line::from(vec![
        Span::styled(" / ", Style::default().fg(c_accent)),
        Span::styled(query, Style::default().fg(c_text)),
        Span::styled("_", Style::default().fg(c_accent)),
        Span::styled(&match_label, Style::default().fg(c_muted)),
    ]);

    let search = Paragraph::new(line).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Search (Ctrl+F) ")
            .border_style(Style::default().fg(c_border)),
    );

    frame.render_widget(search, search_area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_state_starts_inactive() {
        let state = OverlayState::new();
        assert!(!state.is_active());
        assert!(state.active.is_none());
    }

    #[test]
    fn overlay_open_sets_kind() {
        let mut state = OverlayState::new();
        state.open(OverlayKind::Help);
        assert!(state.is_active());
        assert_eq!(state.active, Some(OverlayKind::Help));
    }

    #[test]
    fn overlay_close_clears() {
        let mut state = OverlayState::new();
        state.open(OverlayKind::CommandPalette);
        state.type_char('t');
        state.close();
        assert!(!state.is_active());
        assert!(state.input.is_empty());
        assert_eq!(state.cursor, 0);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn overlay_type_char() {
        let mut state = OverlayState::new();
        state.open(OverlayKind::Search);
        state.type_char('h');
        state.type_char('e');
        state.type_char('l');
        assert_eq!(state.input, "hel");
        assert_eq!(state.cursor, 3);
    }

    #[test]
    fn overlay_backspace() {
        let mut state = OverlayState::new();
        state.open(OverlayKind::Search);
        state.type_char('a');
        state.type_char('b');
        state.backspace();
        assert_eq!(state.input, "a");
        assert_eq!(state.cursor, 1);
    }

    #[test]
    fn overlay_backspace_at_empty() {
        let mut state = OverlayState::new();
        state.backspace(); // Should not panic.
        assert!(state.input.is_empty());
    }

    #[test]
    fn overlay_select_navigation() {
        let mut state = OverlayState::new();
        assert_eq!(state.selected, 0);
        state.select_next(5);
        assert_eq!(state.selected, 1);
        state.select_next(5);
        state.select_next(5);
        state.select_next(5);
        assert_eq!(state.selected, 4);
        state.select_next(5); // At max, should not go beyond.
        assert_eq!(state.selected, 4);
        state.select_prev();
        assert_eq!(state.selected, 3);
    }

    #[test]
    fn overlay_select_prev_at_zero() {
        let mut state = OverlayState::new();
        state.select_prev(); // Should not underflow.
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn centered_rect_produces_valid_rect() {
        let area = Rect::new(0, 0, 120, 40);
        let rect = centered_rect(area, 60, 50);
        assert!(rect.width > 0);
        assert!(rect.height > 0);
        assert!(rect.x + rect.width <= area.width);
        assert!(rect.y + rect.height <= area.height);
    }

    #[test]
    fn overlay_kind_eq() {
        assert_eq!(OverlayKind::Help, OverlayKind::Help);
        assert_ne!(OverlayKind::Help, OverlayKind::Search);
        assert_eq!(
            OverlayKind::PermissionPrompt { tool: "bash".into() },
            OverlayKind::PermissionPrompt { tool: "bash".into() },
        );
    }

    #[test]
    fn overlay_open_resets_state() {
        let mut state = OverlayState::new();
        state.open(OverlayKind::CommandPalette);
        state.type_char('x');
        state.selected = 5;
        // Re-opening should reset.
        state.open(OverlayKind::Help);
        assert!(state.input.is_empty());
        assert_eq!(state.selected, 0);
        assert_eq!(state.cursor, 0);
    }

    // --- Phase C2: Command palette tests ---

    #[test]
    fn default_commands_not_empty() {
        let cmds = default_commands();
        assert!(cmds.len() >= 6);
    }

    #[test]
    fn filter_empty_query_returns_all() {
        let cmds = default_commands();
        let filtered = filter_commands(&cmds, "");
        assert_eq!(filtered.len(), cmds.len());
    }

    #[test]
    fn filter_by_label() {
        let cmds = default_commands();
        let filtered = filter_commands(&cmds, "help");
        assert!(filtered.iter().any(|i| i.label.contains("help")));
    }

    #[test]
    fn filter_by_description() {
        let cmds = default_commands();
        let filtered = filter_commands(&cmds, "keybind");
        assert!(filtered.iter().any(|i| i.description.to_lowercase().contains("keybind")));
    }

    #[test]
    fn filter_no_match_returns_empty() {
        let cmds = default_commands();
        let filtered = filter_commands(&cmds, "zzzzz_nonexistent");
        assert!(filtered.is_empty());
    }

    #[test]
    fn filter_case_insensitive() {
        let cmds = default_commands();
        let filtered_lower = filter_commands(&cmds, "quit");
        let filtered_upper = filter_commands(&cmds, "QUIT");
        assert_eq!(filtered_lower.len(), filtered_upper.len());
    }
}
