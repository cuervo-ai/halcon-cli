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
    /// Toggle the side panel visibility.
    TogglePanel,
    /// Cycle the side panel section (Plan → Metrics → Context → All).
    CyclePanelSection,
    /// Cycle UI mode: Minimal → Standard → Expert.
    CycleUiMode,
    /// Pause or resume the running agent.
    PauseAgent,
    /// Execute one agent step then pause.
    StepAgent,
    /// Approve the pending destructive action.
    ApproveAction,
    /// Reject the pending destructive action.
    RejectAction,
    /// Open the help overlay (F1).
    OpenHelp,
    /// Open the command palette (Ctrl+P).
    OpenCommandPalette,
    /// Open the search overlay (Ctrl+F).
    OpenSearch,
    /// Dismiss all active toast notifications (Ctrl+T).
    DismissToasts,
    /// Pass the key to the currently focused widget (e.g. tui-textarea).
    ForwardToWidget(KeyEvent),
}

/// Dispatch a key event to an action based on current state.
pub fn dispatch_key(key: KeyEvent, agent_running: bool) -> InputAction {
    match (key.modifiers, key.code) {
        // Ctrl+Enter: submit prompt (keyboard-first submit)
        (m, KeyCode::Enter) if m.contains(KeyModifiers::CONTROL) => InputAction::SubmitPrompt,
        // Ctrl+K: clear prompt
        (m, KeyCode::Char('k')) if m.contains(KeyModifiers::CONTROL) => InputAction::ClearPrompt,
        // Ctrl+Up: history back
        (m, KeyCode::Up) if m.contains(KeyModifiers::CONTROL) => InputAction::HistoryBack,
        // Ctrl+Down: history forward
        (m, KeyCode::Down) if m.contains(KeyModifiers::CONTROL) => InputAction::HistoryForward,
        // Esc: cancel running agent
        (_, KeyCode::Esc) if agent_running => InputAction::CancelAgent,
        // Space: pause/resume agent
        (KeyModifiers::NONE, KeyCode::Char(' ')) if agent_running => InputAction::PauseAgent,
        // N: step one then pause
        (KeyModifiers::NONE, KeyCode::Char('n')) if agent_running => InputAction::StepAgent,
        // Y: approve pending action
        (KeyModifiers::NONE, KeyCode::Char('y')) if agent_running => InputAction::ApproveAction,
        // Shift+N: reject pending action
        (KeyModifiers::SHIFT, KeyCode::Char('N')) if agent_running => InputAction::RejectAction,
        // Ctrl+P: open command palette
        (m, KeyCode::Char('p')) if m.contains(KeyModifiers::CONTROL) => InputAction::OpenCommandPalette,
        // Ctrl+F: open search overlay
        (m, KeyCode::Char('f')) if m.contains(KeyModifiers::CONTROL) => InputAction::OpenSearch,
        // Ctrl+T: dismiss active toasts
        (m, KeyCode::Char('t')) if m.contains(KeyModifiers::CONTROL) => InputAction::DismissToasts,
        // Ctrl+D on empty: quit
        (m, KeyCode::Char('d')) if m.contains(KeyModifiers::CONTROL) => InputAction::Quit,
        // Ctrl+C: quit
        (m, KeyCode::Char('c')) if m.contains(KeyModifiers::CONTROL) => InputAction::Quit,
        // F1: help overlay
        (_, KeyCode::F(1)) => InputAction::OpenHelp,
        // F2: toggle side panel
        (_, KeyCode::F(2)) => InputAction::TogglePanel,
        // F3: cycle UI mode (Minimal → Standard → Expert)
        (_, KeyCode::F(3)) => InputAction::CycleUiMode,
        // F4: cycle panel section
        (_, KeyCode::F(4)) => InputAction::CyclePanelSection,
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
    fn ctrl_enter_submits_prompt() {
        let action = dispatch_key(key(KeyModifiers::CONTROL, KeyCode::Enter), false);
        assert_eq!(action, InputAction::SubmitPrompt);
    }

    #[test]
    fn ctrl_enter_submits_even_when_agent_running() {
        // Ctrl+Enter should submit regardless of agent state (queues next prompt).
        let action = dispatch_key(key(KeyModifiers::CONTROL, KeyCode::Enter), true);
        assert_eq!(action, InputAction::SubmitPrompt);
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
    fn f1_opens_help() {
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::F(1)), false);
        assert_eq!(action, InputAction::OpenHelp);
    }

    #[test]
    fn f1_opens_help_during_agent() {
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::F(1)), true);
        assert_eq!(action, InputAction::OpenHelp);
    }

    #[test]
    fn f2_toggles_panel() {
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::F(2)), false);
        assert_eq!(action, InputAction::TogglePanel);
    }

    #[test]
    fn f4_cycles_panel_section() {
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::F(4)), false);
        assert_eq!(action, InputAction::CyclePanelSection);
    }

    #[test]
    fn ctrl_p_opens_command_palette() {
        let action = dispatch_key(key(KeyModifiers::CONTROL, KeyCode::Char('p')), false);
        assert_eq!(action, InputAction::OpenCommandPalette);
    }

    #[test]
    fn ctrl_f_opens_search() {
        let action = dispatch_key(key(KeyModifiers::CONTROL, KeyCode::Char('f')), false);
        assert_eq!(action, InputAction::OpenSearch);
    }

    #[test]
    fn space_pauses_agent() {
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::Char(' ')), true);
        assert_eq!(action, InputAction::PauseAgent);
    }

    #[test]
    fn space_idle_forwards_to_widget() {
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::Char(' ')), false);
        assert!(matches!(action, InputAction::ForwardToWidget(_)));
    }

    #[test]
    fn n_steps_agent() {
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::Char('n')), true);
        assert_eq!(action, InputAction::StepAgent);
    }

    #[test]
    fn y_approves_action() {
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::Char('y')), true);
        assert_eq!(action, InputAction::ApproveAction);
    }

    #[test]
    fn f3_cycles_ui_mode() {
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::F(3)), false);
        assert_eq!(action, InputAction::CycleUiMode);
    }

    #[test]
    fn f3_cycles_ui_mode_during_agent() {
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::F(3)), true);
        assert_eq!(action, InputAction::CycleUiMode);
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

    #[test]
    fn ctrl_t_dismisses_toasts() {
        let action = dispatch_key(key(KeyModifiers::CONTROL, KeyCode::Char('t')), false);
        assert_eq!(action, InputAction::DismissToasts);
    }
}
