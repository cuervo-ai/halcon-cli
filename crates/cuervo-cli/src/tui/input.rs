//! Key event dispatch for the TUI.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Actions that the TUI can perform in response to key events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputAction {
    /// Submit the current prompt text to the agent.
    SubmitPrompt,
    /// Clear the prompt text.
    ClearPrompt,
    /// Navigate prompt history backward.
    HistoryBack,
    /// Navigate prompt history forward.
    HistoryForward,
    /// Cancel the running agent.
    CancelAgent,
    /// Quit the TUI application.
    Quit,
    /// Cycle keyboard focus between zones.
    CycleFocus,
    /// Scroll the activity zone up.
    ScrollUp,
    /// Scroll the activity zone down.
    ScrollDown,
    /// Scroll to the bottom of the activity zone.
    ScrollToBottom,
    /// Pass the key to the currently focused widget (e.g. tui-textarea).
    ForwardToWidget(KeyEvent),
}

/// Dispatch a key event to an action based on current state.
pub fn dispatch_key(key: KeyEvent, agent_running: bool) -> InputAction {
    match (key.modifiers, key.code) {
        // Ctrl+K: clear prompt
        (m, KeyCode::Char('k')) if m.contains(KeyModifiers::CONTROL) => InputAction::ClearPrompt,
        // Ctrl+Up: history back
        (m, KeyCode::Up) if m.contains(KeyModifiers::CONTROL) => InputAction::HistoryBack,
        // Ctrl+Down: history forward
        (m, KeyCode::Down) if m.contains(KeyModifiers::CONTROL) => InputAction::HistoryForward,
        // Esc: cancel running agent
        (_, KeyCode::Esc) if agent_running => InputAction::CancelAgent,
        // Ctrl+D on empty: quit
        (m, KeyCode::Char('d')) if m.contains(KeyModifiers::CONTROL) => InputAction::Quit,
        // Ctrl+C: quit
        (m, KeyCode::Char('c')) if m.contains(KeyModifiers::CONTROL) => InputAction::Quit,
        // Tab: cycle focus
        (_, KeyCode::Tab) => InputAction::CycleFocus,
        // Shift+Up / PageUp: scroll activity up
        (m, KeyCode::Up) if m.contains(KeyModifiers::SHIFT) => InputAction::ScrollUp,
        (_, KeyCode::PageUp) => InputAction::ScrollUp,
        // Shift+Down / PageDown: scroll activity down
        (m, KeyCode::Down) if m.contains(KeyModifiers::SHIFT) => InputAction::ScrollDown,
        (_, KeyCode::PageDown) => InputAction::ScrollDown,
        // End: scroll to bottom
        (_, KeyCode::End) => InputAction::ScrollToBottom,
        // Everything else: forward to active widget
        _ => InputAction::ForwardToWidget(key),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    fn key(mods: KeyModifiers, code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    #[test]
    fn ctrl_enter_forwards_to_widget() {
        // Submit is button-only, Ctrl+Enter goes to textarea (inserts newline).
        let action = dispatch_key(key(KeyModifiers::CONTROL, KeyCode::Enter), false);
        assert!(matches!(action, InputAction::ForwardToWidget(_)));
    }

    #[test]
    fn esc_cancels_agent() {
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::Esc), true);
        assert_eq!(action, InputAction::CancelAgent);
    }

    #[test]
    fn esc_when_idle_forwards() {
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::Esc), false);
        assert!(matches!(action, InputAction::ForwardToWidget(_)));
    }

    #[test]
    fn ctrl_c_quits() {
        let action = dispatch_key(key(KeyModifiers::CONTROL, KeyCode::Char('c')), false);
        assert_eq!(action, InputAction::Quit);
    }

    #[test]
    fn ctrl_d_quits() {
        let action = dispatch_key(key(KeyModifiers::CONTROL, KeyCode::Char('d')), false);
        assert_eq!(action, InputAction::Quit);
    }

    #[test]
    fn tab_cycles_focus() {
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::Tab), false);
        assert_eq!(action, InputAction::CycleFocus);
    }

    #[test]
    fn page_up_scrolls() {
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::PageUp), false);
        assert_eq!(action, InputAction::ScrollUp);
    }

    #[test]
    fn shift_up_scrolls() {
        let action = dispatch_key(key(KeyModifiers::SHIFT, KeyCode::Up), false);
        assert_eq!(action, InputAction::ScrollUp);
    }

    #[test]
    fn ctrl_k_clears() {
        let action = dispatch_key(key(KeyModifiers::CONTROL, KeyCode::Char('k')), false);
        assert_eq!(action, InputAction::ClearPrompt);
    }

    #[test]
    fn ctrl_up_history_back() {
        let action = dispatch_key(key(KeyModifiers::CONTROL, KeyCode::Up), false);
        assert_eq!(action, InputAction::HistoryBack);
    }

    #[test]
    fn regular_char_forwards() {
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::Char('a')), false);
        assert!(matches!(action, InputAction::ForwardToWidget(_)));
    }

    #[test]
    fn enter_forwards_to_widget() {
        // Plain Enter inserts newline (forwarded to textarea)
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::Enter), false);
        assert!(matches!(action, InputAction::ForwardToWidget(_)));
    }
}
