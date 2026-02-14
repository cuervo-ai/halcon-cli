//! Multiline prompt editor widget using tui-textarea.

use crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;
use tui_textarea::TextArea;

/// State for the multiline prompt editor.
pub struct PromptState {
    textarea: TextArea<'static>,
    history: Vec<String>,
    history_index: Option<usize>,
}

impl PromptState {
    pub fn new() -> Self {
        let mut textarea = TextArea::default();
        textarea.set_block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Prompt ")
                .border_style(Style::default().fg(Color::DarkGray)),
        );
        textarea.set_cursor_line_style(Style::default());
        textarea.set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));
        textarea.set_placeholder_text("Type your message here...");

        Self {
            textarea,
            history: Vec::new(),
            history_index: None,
        }
    }

    /// Render the prompt widget.
    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        let border_color = if focused { Color::Cyan } else { Color::DarkGray };
        let title = if focused {
            " Prompt (Ctrl+Enter to send) "
        } else {
            " Prompt "
        };
        self.textarea.set_block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(border_color)),
        );
        frame.render_widget(&self.textarea, area);
    }

    /// Take the current text and clear the editor. Returns the text.
    pub fn take_text(&mut self) -> String {
        let lines: Vec<String> = self.textarea.lines().iter().map(|l| l.to_string()).collect();
        let text = lines.join("\n");
        if !text.trim().is_empty() {
            self.history.push(text.clone());
        }
        self.history_index = None;
        // Clear textarea.
        self.textarea = TextArea::default();
        self.textarea.set_placeholder_text("Type your message here...");
        text
    }

    /// Clear the prompt text.
    pub fn clear(&mut self) {
        self.textarea = TextArea::default();
        self.textarea.set_placeholder_text("Type your message here...");
        self.history_index = None;
    }

    /// Insert a newline at cursor.
    #[allow(dead_code)]
    pub fn insert_newline(&mut self) {
        self.textarea.insert_newline();
    }

    /// Forward a key event to the textarea.
    pub fn handle_key(&mut self, key: KeyEvent) {
        self.textarea.input(key);
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
        let Some(current) = self.history_index else { return };
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
            self.textarea.set_placeholder_text("Type your message here...");
        }
    }

    /// Get current text without taking it.
    #[allow(dead_code)]
    pub fn text(&self) -> String {
        self.textarea.lines().join("\n")
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
}
