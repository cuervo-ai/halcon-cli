//! Multiline prompt editor widget using tui-textarea.

use crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use tui_textarea::TextArea;

use super::super::input_state::InputState;

/// State for the multiline prompt editor.
pub struct PromptState {
    textarea: TextArea<'static>,
    history: Vec<String>,
    history_index: Option<usize>,
    /// Current input state (idle/queued/sending/locked). Phase 2.1.
    pub input_state: InputState,
}

impl PromptState {
    pub fn new() -> Self {
        let mut textarea = TextArea::default();
        let p = &crate::render::theme::active().palette;

        textarea.set_block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Prompt ")
                .border_style(Style::default().fg(p.border_ratatui())),
        );

        // Enhanced cursor: block style with theme color
        textarea.set_cursor_style(
            Style::default()
                .bg(p.accent_ratatui())
                .fg(p.bg_panel_ratatui())
                .add_modifier(Modifier::BOLD),
        );

        // Subtle cursor line highlight
        textarea.set_cursor_line_style(Style::default().bg(p.bg_highlight_ratatui()));

        textarea
            .set_placeholder_text("Type your message... (Enter to send, Shift+Enter for new line)");

        Self {
            textarea,
            history: Vec::new(),
            history_index: None,
            input_state: InputState::default(),
        }
    }

    /// Render the prompt widget with enhanced visual feedback.
    ///
    /// # Arguments
    /// * `typing` - Whether to show typing indicator (Phase 44C).
    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool, typing: bool) {
        let p = &crate::render::theme::active().palette;

        // Count chars and lines for status display
        let text = self.text();
        let char_count = text.chars().count();
        let line_count = self.textarea.lines().len();

        let (border_color, title) = if focused {
            // Phase 2.1: State-aware color and label
            let state_icon = self.input_state.icon();
            let state_label = self.input_state.label();
            let state_color = self.input_state.semantic_color(p);

            let title_text = if typing && char_count == 0 {
                format!(" ✍ typing... · {} {} ", state_icon, state_label)
            } else if char_count > 0 {
                format!(
                    " ✎ Prompt ({} chars, {} lines) · {} {} · Enter→send",
                    char_count, line_count, state_icon, state_label
                )
            } else {
                format!(" ✎ Prompt · {} {} · Enter→send", state_icon, state_label)
            };
            (state_color.to_ratatui_color(), title_text)
        } else {
            (p.border_ratatui(), " Prompt ".to_string())
        };

        self.textarea.set_block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(border_color).add_modifier(if focused {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                })),
        );

        frame.render_widget(&self.textarea, area);
    }

    /// Render the prompt widget in **compact mode** (Phase I2 + Button Fix).
    ///
    /// Layout: textarea | button (14 cols) on last line, separator with metadata.
    ///
    /// # Arguments
    /// * `typing` - Whether to show typing indicator.
    ///
    /// # Returns
    /// * `usize` - Number of content lines (for dynamic height)
    /// * `Option<Rect>` - Button area if rendered
    pub fn render_compact(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        focused: bool,
        typing: bool,
    ) -> (usize, Option<Rect>) {
        let p = &crate::render::theme::active().palette;

        // Split: [textarea (n lines)] + [separator (1 line)]
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(area);

        let input_area = chunks[0];
        let separator_area = chunks[1];

        // Count lines for dynamic height
        let line_count = self.textarea.lines().len();
        let char_count = self.text().chars().count();

        // Phase I2: State-aware background color at 5-8% opacity
        let (state_icon, state_label, state_color) = if focused {
            let icon = self.input_state.icon();
            let label = self.input_state.label();
            let color = self.input_state.semantic_color(p);
            (icon, label, color)
        } else {
            ("✎", "idle", p.border)
        };

        // Background: subtle semantic color hint (5% opacity approximation via dim)
        let bg_style = if focused {
            // Approximate 5% opacity by using a very dim version of the semantic color
            // In terminal, we can't do true opacity, so we just use the border color dimmed
            Style::default().bg(p.bg_panel_ratatui())
        } else {
            Style::default()
        };

        // Remove all borders, use background style
        self.textarea
            .set_block(Block::default().borders(Borders::NONE).style(bg_style));

        // Enhanced cursor: block style with theme color
        self.textarea.set_cursor_style(
            Style::default()
                .bg(state_color.to_ratatui_color())
                .fg(p.bg_panel_ratatui())
                .add_modifier(Modifier::BOLD),
        );

        // Subtle cursor line highlight
        self.textarea
            .set_cursor_line_style(Style::default().bg(p.bg_highlight_ratatui()));

        // Compact placeholder (no keybinding hints)
        let placeholder = if typing && char_count == 0 {
            "✍ typing..."
        } else {
            "Type your message..."
        };
        self.textarea.set_placeholder_text(placeholder);

        // SCROLL FIX: Calculate if scroll is needed and render with scrollbar
        let available_height = input_area.height as usize;
        let total_lines = self.textarea.lines().len();
        let needs_scroll = total_lines > available_height;

        if needs_scroll && focused {
            // Split: [textarea] | [scrollbar (1 col)]
            let h_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(1), Constraint::Length(1)])
                .split(input_area);

            let textarea_area = h_chunks[0];
            let scrollbar_area = h_chunks[1];

            // Render textarea
            frame.render_widget(&self.textarea, textarea_area);

            // Render scrollbar indicator
            self.render_scrollbar(frame, scrollbar_area, available_height, total_lines);
        } else {
            // No scroll needed, use full width
            frame.render_widget(&self.textarea, input_area);
        }

        // Phase I2 Fix: Split separator into [metadata line] + [button (14 cols)]
        // ALWAYS show button if there's enough width (min 34 cols total)
        let button_width = 14u16;
        let show_button = separator_area.width >= (button_width + 20);

        let (sep_area, button_area) = if show_button {
            let h_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(1), Constraint::Length(button_width)])
                .split(separator_area);
            (h_chunks[0], Some(h_chunks[1]))
        } else {
            (separator_area, None)
        };

        // Calculate scroll info for separator
        let available_height = input_area.height as usize;
        let needs_scroll = line_count > available_height;
        let scroll_info = if needs_scroll {
            let cursor_row = self.textarea.cursor().0 + 1; // 1-indexed for display
            Some((cursor_row, line_count))
        } else {
            None
        };

        // Render separator line with inline metadata
        self.render_separator(
            frame,
            sep_area,
            focused,
            state_icon,
            state_label,
            state_color,
            char_count,
            scroll_info,
        );

        // Return content line count + button area for external rendering
        (line_count, button_area)
    }

    /// Render a vertical scrollbar indicator (SCROLL FIX).
    ///
    /// Shows position in scrollable content with visual track.
    fn render_scrollbar(
        &self,
        frame: &mut Frame,
        area: Rect,
        visible_height: usize,
        total_lines: usize,
    ) {
        let p = &crate::render::theme::active().palette;

        // Calculate scrollbar thumb position
        // tui-textarea cursor is at self.textarea.cursor().0 (row, col)
        let cursor_row = self.textarea.cursor().0;
        let scroll_position = cursor_row.saturating_sub(visible_height / 2);

        let thumb_height = (visible_height * visible_height / total_lines).max(1);
        let thumb_position = (scroll_position * area.height as usize / total_lines.max(1)) as u16;

        // Render scrollbar track
        for y in 0..area.height {
            let is_thumb = y >= thumb_position && y < thumb_position + thumb_height as u16;
            let symbol = if is_thumb { "█" } else { "│" };
            let style = if is_thumb {
                Style::default().fg(p.accent_ratatui())
            } else {
                Style::default()
                    .fg(p.border_ratatui())
                    .add_modifier(Modifier::DIM)
            };

            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(symbol, style))),
                Rect {
                    x: area.x,
                    y: area.y + y,
                    width: 1,
                    height: 1,
                },
            );
        }
    }

    /// Render the separator line with inline metadata (Phase I2 + Scroll Fix).
    ///
    /// Format: `─────────────── ✓ ready · 142 chars · ↕ 5/12 · Enter───────────────`
    fn render_separator(
        &self,
        frame: &mut Frame,
        area: Rect,
        focused: bool,
        state_icon: &str,
        state_label: &str,
        state_color: crate::render::theme::ThemeColor,
        char_count: usize,
        scroll_info: Option<(usize, usize)>, // (cursor_line, total_lines)
    ) {
        let p = &crate::render::theme::active().palette;

        if !focused {
            // Unfocused: simple thin line
            let line = Line::from(vec![Span::styled(
                "─".repeat(area.width as usize),
                Style::default()
                    .fg(p.border_ratatui())
                    .add_modifier(Modifier::DIM),
            )]);
            frame.render_widget(Paragraph::new(line), area);
            return;
        }

        // SCROLL FIX: Add scroll indicator to metadata
        let scroll_suffix = if let Some((cursor, total)) = scroll_info {
            format!(" · ↕ {}/{}", cursor, total)
        } else {
            String::new()
        };

        let metadata = if char_count > 0 {
            format!(
                " {} {} · {} chars{} · Enter",
                state_icon, state_label, char_count, scroll_suffix
            )
        } else {
            format!(" {} {}{} · Enter", state_icon, state_label, scroll_suffix)
        };

        let metadata_len = metadata.chars().count();
        let available = area.width as usize;

        // Calculate padding: (total - metadata) / 2 for each side
        let line_text = if metadata_len + 6 <= available {
            let padding = (available.saturating_sub(metadata_len)) / 2;
            let left_pad = "─".repeat(padding);
            let right_pad = "─".repeat(available.saturating_sub(metadata_len + padding));
            format!("{}{}{}", left_pad, metadata, right_pad)
        } else {
            // Too narrow: just show metadata, truncate if needed
            metadata.chars().take(available).collect()
        };

        let line = Line::from(vec![Span::styled(
            line_text,
            Style::default().fg(state_color.to_ratatui_color()),
        )]);

        frame.render_widget(Paragraph::new(line), area);
    }

    /// Take the current text and clear the editor. Returns the text.
    pub fn take_text(&mut self) -> String {
        let lines: Vec<String> = self
            .textarea
            .lines()
            .iter()
            .map(|l| l.to_string())
            .collect();
        let text = lines.join("\n");
        if !text.trim().is_empty() {
            self.history.push(text.clone());
        }
        self.history_index = None;
        // Clear textarea.
        self.textarea = TextArea::default();
        self.textarea
            .set_placeholder_text("Type your message... (Enter to send, Shift+Enter for new line)");
        text
    }

    /// Clear the prompt text.
    pub fn clear(&mut self) {
        self.textarea = TextArea::default();
        self.textarea
            .set_placeholder_text("Type your message... (Enter to send, Shift+Enter for new line)");
        self.history_index = None;
    }

    /// Insert a newline at cursor (triggered by Shift+Enter).
    pub fn insert_newline(&mut self) {
        self.textarea.insert_newline();
    }

    /// Insert a string at the current cursor position (e.g. from clipboard paste).
    /// Handles multi-line text by inserting newlines at each line boundary.
    pub fn insert_str(&mut self, text: &str) {
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                self.textarea.insert_newline();
            }
            self.textarea.insert_str(line);
        }
    }

    /// Forward a key event to the textarea.
    pub fn handle_key(&mut self, key: KeyEvent) {
        self.textarea.input(key);
    }

    /// Whether the cursor is on the first line.
    /// Used to decide if Up arrow should navigate history or move cursor.
    pub fn is_on_first_line(&self) -> bool {
        self.textarea.cursor().0 == 0
    }

    /// Whether the cursor is on the last line.
    /// Used to decide if Down arrow should navigate history or move cursor.
    pub fn is_on_last_line(&self) -> bool {
        let last = self.textarea.lines().len().saturating_sub(1);
        self.textarea.cursor().0 >= last
    }

    /// Navigate history backward.
    pub fn history_back(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let idx = match self.history_index {
            Some(0) => 0,
            Some(i) => i - 1,
            None => self.history.len() - 1,
        };
        self.history_index = Some(idx);
        self.load_history_entry(idx);
    }

    /// Navigate history forward.
    pub fn history_forward(&mut self) {
        let Some(current) = self.history_index else {
            return;
        };
        if current + 1 < self.history.len() {
            let idx = current + 1;
            self.history_index = Some(idx);
            self.load_history_entry(idx);
        } else {
            // Past end → clear to empty.
            self.history_index = None;
            self.clear();
        }
    }

    fn load_history_entry(&mut self, idx: usize) {
        if let Some(entry) = self.history.get(idx) {
            let lines: Vec<&str> = entry.lines().collect();
            self.textarea = TextArea::new(lines.iter().map(|l| l.to_string()).collect());
            self.textarea.set_placeholder_text(
                "Type your message... (Enter to send, Shift+Enter for new line)",
            );
        }
    }

    /// Get current text without taking it.
    pub fn text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    /// Select all text in the textarea.
    ///
    /// Uses tui-textarea's built-in select_all when available,
    /// otherwise moves cursor to start then shifts-selects to end.
    pub fn select_all(&mut self) {
        self.textarea.select_all();
    }

    /// Set the input state (Phase 2.1).
    pub fn set_input_state(&mut self, state: InputState) {
        self.input_state = state;
    }

    /// Get the current input state (Phase 2.1).
    pub fn input_state(&self) -> InputState {
        self.input_state
    }
}

impl Default for PromptState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_prompt_is_empty() {
        let prompt = PromptState::new();
        assert_eq!(prompt.text(), "");
    }

    #[test]
    fn take_text_clears_and_returns() {
        let mut prompt = PromptState::new();
        prompt.textarea.insert_str("hello world");
        let text = prompt.take_text();
        assert_eq!(text, "hello world");
        assert_eq!(prompt.text(), "");
    }

    #[test]
    fn take_text_adds_to_history() {
        let mut prompt = PromptState::new();
        prompt.textarea.insert_str("first message");
        prompt.take_text();
        prompt.textarea.insert_str("second message");
        prompt.take_text();
        assert_eq!(prompt.history.len(), 2);
    }

    #[test]
    fn empty_submit_not_in_history() {
        let mut prompt = PromptState::new();
        prompt.take_text();
        assert!(prompt.history.is_empty());
    }

    #[test]
    fn history_back_loads_previous() {
        let mut prompt = PromptState::new();
        prompt.textarea.insert_str("msg1");
        prompt.take_text();
        prompt.textarea.insert_str("msg2");
        prompt.take_text();

        prompt.history_back();
        assert_eq!(prompt.text(), "msg2");
        prompt.history_back();
        assert_eq!(prompt.text(), "msg1");
    }

    #[test]
    fn history_forward_past_end_clears() {
        let mut prompt = PromptState::new();
        prompt.textarea.insert_str("msg1");
        prompt.take_text();

        prompt.history_back();
        assert_eq!(prompt.text(), "msg1");
        prompt.history_forward();
        assert_eq!(prompt.text(), "");
    }

    #[test]
    fn history_back_on_empty_is_noop() {
        let mut prompt = PromptState::new();
        prompt.history_back(); // Should not panic.
        assert_eq!(prompt.text(), "");
    }

    #[test]
    fn clear_resets_text() {
        let mut prompt = PromptState::new();
        prompt.textarea.insert_str("some text");
        prompt.clear();
        assert_eq!(prompt.text(), "");
    }

    #[test]
    fn insert_newline_adds_line() {
        let mut prompt = PromptState::new();
        prompt.textarea.insert_str("line1");
        prompt.insert_newline();
        prompt.textarea.insert_str("line2");
        let text = prompt.text();
        assert!(text.contains("line1"));
        assert!(text.contains("line2"));
    }

    #[test]
    fn history_back_at_beginning_stays() {
        let mut prompt = PromptState::new();
        prompt.textarea.insert_str("only msg");
        prompt.take_text();

        prompt.history_back();
        assert_eq!(prompt.text(), "only msg");
        prompt.history_back(); // Already at 0, should stay.
        assert_eq!(prompt.text(), "only msg");
    }

    // --- Phase 2.1: InputState integration tests ---

    #[test]
    fn new_prompt_starts_idle() {
        let prompt = PromptState::new();
        assert_eq!(prompt.input_state, InputState::Idle);
    }

    #[test]
    fn set_input_state_updates_state() {
        let mut prompt = PromptState::new();
        prompt.set_input_state(InputState::Sending);
        assert_eq!(prompt.input_state(), InputState::Sending);
    }

    #[test]
    fn input_state_persists_across_operations() {
        let mut prompt = PromptState::new();
        prompt.set_input_state(InputState::Sending);
        prompt.textarea.insert_str("test");
        assert_eq!(prompt.input_state(), InputState::Sending);
        prompt.clear();
        // State should persist even after clear
        assert_eq!(prompt.input_state(), InputState::Sending);
    }

    // --- History navigation position tests ---

    #[test]
    fn single_line_prompt_is_on_first_and_last_line() {
        let mut prompt = PromptState::new();
        prompt.textarea.insert_str("hello");
        assert!(prompt.is_on_first_line());
        assert!(prompt.is_on_last_line());
    }

    #[test]
    fn empty_prompt_is_on_first_and_last_line() {
        let prompt = PromptState::new();
        assert!(prompt.is_on_first_line());
        assert!(prompt.is_on_last_line());
    }

    #[test]
    fn multiline_first_line_detection() {
        let mut prompt = PromptState::new();
        prompt.textarea.insert_str("line one");
        prompt.insert_newline();
        prompt.textarea.insert_str("line two");
        // Cursor is now on second line — not first, IS last.
        assert!(!prompt.is_on_first_line());
        assert!(prompt.is_on_last_line());
    }

    #[test]
    fn history_back_restores_previous_prompt() {
        let mut prompt = PromptState::new();
        prompt.textarea.insert_str("first prompt");
        prompt.take_text();
        prompt.textarea.insert_str("second prompt");
        prompt.take_text();

        // After submitting twice, history has 2 entries.
        prompt.history_back(); // load "second prompt"
        assert_eq!(prompt.text(), "second prompt");
        prompt.history_back(); // load "first prompt"
        assert_eq!(prompt.text(), "first prompt");
    }

    #[test]
    fn history_forward_after_back_advances() {
        let mut prompt = PromptState::new();
        prompt.textarea.insert_str("msg one");
        prompt.take_text();
        prompt.textarea.insert_str("msg two");
        prompt.take_text();

        prompt.history_back(); // "msg two"
        prompt.history_back(); // "msg one"
        prompt.history_forward(); // back to "msg two"
        assert_eq!(prompt.text(), "msg two");
    }
}
