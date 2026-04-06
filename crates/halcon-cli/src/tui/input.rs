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
    /// Pause or resume the running agent (programmatic — triggered via /pause slash command).
    PauseAgent,
    /// Execute one agent step then pause (programmatic — triggered via /step slash command).
    StepAgent,
    /// Approve the pending destructive action (programmatic — triggered by permission overlay).
    ApproveAction,
    /// Reject the pending destructive action (programmatic — triggered by permission overlay).
    RejectAction,
    /// Approve always (global permission) — triggered by permission overlay advanced options.
    ApproveAlways,
    /// Approve for this directory only — triggered by permission overlay advanced options.
    ApproveDirectory,
    /// Approve for this session only — triggered by permission overlay advanced options.
    ApproveSession,
    /// Approve for this pattern only — triggered by permission overlay advanced options.
    ApprovePattern,
    /// Deny for this directory — triggered by permission overlay advanced options.
    DenyDirectory,
    /// Open the help overlay (F1).
    OpenHelp,
    /// Open the command palette (Ctrl+P).
    OpenCommandPalette,
    /// Open the search overlay (Ctrl+F).
    OpenSearch,
    /// Dismiss all active toast notifications (Ctrl+T).
    DismissToasts,
    /// Toggle conversation filter (show only user/assistant text).
    ToggleConversationFilter,
    /// Insert a newline at the cursor position (Shift+Enter).
    InsertNewline,
    /// Open the Context Servers overlay (Ctrl+S).
    OpenContextServers,
    /// Paste clipboard content into the prompt at cursor position (Ctrl+V).
    PasteFromClipboard,
    /// Open the session browser overlay (F6).
    OpenSessionList,

    // --- Phase 93: Cross-Platform SOTA ---
    /// Open native file picker for media attachment (Ctrl+O).
    ///
    /// In terminal context: shows a toast with drag-and-drop instructions.
    OpenFilePicker,
    /// Remove the last pending media attachment (Ctrl+Backspace).
    RemoveLastAttachment,

    // Activity Navigation actions — these are NOT returned by dispatch_key().
    // They are handled by handle_action() for direct/programmatic invocation (tests, etc.).
    // Actual activity navigation goes through ActivityController → ControlAction.
    /// Select next line in activity (programmatic use; keyboard routes via ActivityController).
    SelectNextLine,
    /// Select previous line in activity (programmatic use; keyboard routes via ActivityController).
    SelectPrevLine,
    /// Toggle expand/collapse on selected line (programmatic use).
    ToggleExpand,
    /// Copy selected line content to clipboard (programmatic use; keyboard 'y' → ActivityController).
    CopySelected,
    /// Open inspector for selected line (programmatic use).
    InspectSelected,
    /// Expand all tool executions (programmatic use).
    ExpandAllTools,
    /// Collapse all tool executions (programmatic use).
    CollapseAllTools,
    /// Jump to plan step for selected tool (programmatic use; keyboard 'p' → ActivityController).
    JumpToPlan,
    /// Navigate to next search match (programmatic use).
    SearchNext,
    /// Navigate to previous search match (programmatic use).
    SearchPrev,
    /// Clear activity selection (programmatic use).
    ClearSelection,

    /// Toggle sub-agent detail view: collapsed pills ↔ expanded tool list + summary (Ctrl+B).
    ToggleSubAgentDetail,

    /// Open the model selector overlay (Ctrl+M).
    OpenModelSelector,

    /// Open the settings overlay (F7).
    OpenSettings,

    /// Open the LSP status overlay (F8).
    OpenLspStatus,

    /// Copy selected activity line or prompt selection to clipboard (Ctrl+Shift+C).
    CopyToClipboard,

    /// Cut prompt selection to clipboard (Ctrl+X).
    CutToClipboard,

    /// Select all text in the prompt (Ctrl+A).
    SelectAll,

    /// Pass the key to the currently focused widget (e.g. tui-textarea).
    ForwardToWidget(KeyEvent),
}

/// Dispatch a key event to an action.
///
/// Architecture notes:
/// - Input is NEVER blocked by agent state — user can always type.
/// - Most keys → ForwardToWidget; special combos → named actions.
/// - Activity navigation (j/k/y/p/i etc.) routes via ActivityController, NOT this dispatch.
/// - Agent control (/pause /resume /step /cancel) routes via slash commands, NOT bare keys.
/// - Programmatic InputAction variants (PauseAgent, SelectNextLine, etc.) exist for
///   direct handle_action() calls but are never returned by this function.
pub fn dispatch_key(key: KeyEvent) -> InputAction {
    match (key.modifiers, key.code) {
        // Ctrl+Enter: submit prompt (backward-compat; Enter alone submits when Prompt-focused)
        (m, KeyCode::Enter) if m.contains(KeyModifiers::CONTROL) => InputAction::SubmitPrompt,
        // Shift+Enter: insert newline (multi-line input without submitting)
        (m, KeyCode::Enter) if m.contains(KeyModifiers::SHIFT) => InputAction::InsertNewline,
        // Ctrl+S: open Context Servers overlay (was hardcoded before dispatch_key)
        (m, KeyCode::Char('s')) if m.contains(KeyModifiers::CONTROL) => {
            InputAction::OpenContextServers
        }
        // Ctrl+V: paste from clipboard into prompt
        (m, KeyCode::Char('v')) if m.contains(KeyModifiers::CONTROL) => {
            InputAction::PasteFromClipboard
        }
        // Ctrl+K: clear prompt
        (m, KeyCode::Char('k')) if m.contains(KeyModifiers::CONTROL) => InputAction::ClearPrompt,
        // Ctrl+Up: history back
        (m, KeyCode::Up) if m.contains(KeyModifiers::CONTROL) => InputAction::HistoryBack,
        // Ctrl+Down: history forward
        (m, KeyCode::Down) if m.contains(KeyModifiers::CONTROL) => InputAction::HistoryForward,
        // Ctrl+P: open command palette
        (m, KeyCode::Char('p')) if m.contains(KeyModifiers::CONTROL) => {
            InputAction::OpenCommandPalette
        }
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
        // F5: toggle conversation filter
        (_, KeyCode::F(5)) => InputAction::ToggleConversationFilter,
        // F6: session browser
        (_, KeyCode::F(6)) => InputAction::OpenSessionList,
        // Ctrl+O: open file picker / show attachment instructions
        (m, KeyCode::Char('o')) if m.contains(KeyModifiers::CONTROL) => InputAction::OpenFilePicker,
        // Ctrl+Backspace: remove last attachment chip
        (m, KeyCode::Backspace) if m.contains(KeyModifiers::CONTROL) => {
            InputAction::RemoveLastAttachment
        }
        // Ctrl+B: toggle sub-agent detail view (collapsed pills ↔ tool list + summary)
        (m, KeyCode::Char('b')) if m.contains(KeyModifiers::CONTROL) => {
            InputAction::ToggleSubAgentDetail
        }
        // Ctrl+M: open model selector overlay
        (m, KeyCode::Char('m')) if m.contains(KeyModifiers::CONTROL) => {
            InputAction::OpenModelSelector
        }
        // Ctrl+Shift+C: copy to clipboard (activity line or prompt text)
        (m, KeyCode::Char('C'))
            if m.contains(KeyModifiers::CONTROL) && m.contains(KeyModifiers::SHIFT) =>
        {
            InputAction::CopyToClipboard
        }
        // Ctrl+X: cut selection to clipboard
        (m, KeyCode::Char('x')) if m.contains(KeyModifiers::CONTROL) => InputAction::CutToClipboard,
        // Ctrl+A: select all in prompt
        (m, KeyCode::Char('a')) if m.contains(KeyModifiers::CONTROL) => InputAction::SelectAll,
        // F7: settings overlay
        (_, KeyCode::F(7)) => InputAction::OpenSettings,
        // F8: LSP status overlay
        (_, KeyCode::F(8)) => InputAction::OpenLspStatus,
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
        let action = dispatch_key(key(KeyModifiers::CONTROL, KeyCode::Enter));
        assert_eq!(action, InputAction::SubmitPrompt);
    }

    #[test]
    fn esc_forwards_to_widget() {
        // Esc is now ALWAYS forwarded to widget (no agent cancellation via key)
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::Esc));
        assert!(matches!(action, InputAction::ForwardToWidget(_)));
    }

    #[test]
    fn ctrl_c_quits() {
        let action = dispatch_key(key(KeyModifiers::CONTROL, KeyCode::Char('c')));
        assert_eq!(action, InputAction::Quit);
    }

    #[test]
    fn ctrl_d_quits() {
        let action = dispatch_key(key(KeyModifiers::CONTROL, KeyCode::Char('d')));
        assert_eq!(action, InputAction::Quit);
    }

    #[test]
    fn tab_cycles_focus() {
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::Tab));
        assert_eq!(action, InputAction::CycleFocus);
    }

    #[test]
    fn page_up_scrolls() {
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::PageUp));
        assert_eq!(action, InputAction::ScrollUp);
    }

    #[test]
    fn shift_up_scrolls() {
        let action = dispatch_key(key(KeyModifiers::SHIFT, KeyCode::Up));
        assert_eq!(action, InputAction::ScrollUp);
    }

    #[test]
    fn ctrl_k_clears() {
        let action = dispatch_key(key(KeyModifiers::CONTROL, KeyCode::Char('k')));
        assert_eq!(action, InputAction::ClearPrompt);
    }

    #[test]
    fn ctrl_up_history_back() {
        let action = dispatch_key(key(KeyModifiers::CONTROL, KeyCode::Up));
        assert_eq!(action, InputAction::HistoryBack);
    }

    #[test]
    fn f1_opens_help() {
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::F(1)));
        assert_eq!(action, InputAction::OpenHelp);
    }

    #[test]
    fn f2_toggles_panel() {
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::F(2)));
        assert_eq!(action, InputAction::TogglePanel);
    }

    #[test]
    fn f4_cycles_panel_section() {
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::F(4)));
        assert_eq!(action, InputAction::CyclePanelSection);
    }

    #[test]
    fn ctrl_p_opens_command_palette() {
        let action = dispatch_key(key(KeyModifiers::CONTROL, KeyCode::Char('p')));
        assert_eq!(action, InputAction::OpenCommandPalette);
    }

    #[test]
    fn ctrl_f_opens_search() {
        let action = dispatch_key(key(KeyModifiers::CONTROL, KeyCode::Char('f')));
        assert_eq!(action, InputAction::OpenSearch);
    }

    #[test]
    fn space_forwards_to_widget() {
        // Space is now ALWAYS forwarded to widget (can type space)
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::Char(' ')));
        assert!(matches!(action, InputAction::ForwardToWidget(_)));
    }

    #[test]
    fn n_forwards_to_widget() {
        // 'n' is now ALWAYS forwarded to widget (can type 'n')
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::Char('n')));
        assert!(matches!(action, InputAction::ForwardToWidget(_)));
    }

    #[test]
    fn y_forwards_to_widget() {
        // 'y' is now ALWAYS forwarded to widget (can type 'y')
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::Char('y')));
        assert!(matches!(action, InputAction::ForwardToWidget(_)));
    }

    #[test]
    fn f3_cycles_ui_mode() {
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::F(3)));
        assert_eq!(action, InputAction::CycleUiMode);
    }

    #[test]
    fn regular_char_forwards() {
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::Char('a')));
        assert!(matches!(action, InputAction::ForwardToWidget(_)));
    }

    #[test]
    fn enter_forwards_to_widget() {
        // Plain Enter is forwarded to widget; handle_action decides submit vs newline based on focus.
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::Enter));
        assert!(matches!(action, InputAction::ForwardToWidget(_)));
    }

    #[test]
    fn shift_enter_inserts_newline() {
        // Shift+Enter always inserts a newline (multi-line input, no submit).
        let action = dispatch_key(key(KeyModifiers::SHIFT, KeyCode::Enter));
        assert_eq!(action, InputAction::InsertNewline);
    }

    #[test]
    fn ctrl_s_opens_context_servers() {
        // Ctrl+S opens the Context Servers overlay (moved from hardcoded check in app.rs).
        let action = dispatch_key(key(KeyModifiers::CONTROL, KeyCode::Char('s')));
        assert_eq!(action, InputAction::OpenContextServers);
    }

    #[test]
    fn ctrl_t_dismisses_toasts() {
        let action = dispatch_key(key(KeyModifiers::CONTROL, KeyCode::Char('t')));
        assert_eq!(action, InputAction::DismissToasts);
    }

    #[test]
    fn ctrl_v_pastes_from_clipboard() {
        let action = dispatch_key(key(KeyModifiers::CONTROL, KeyCode::Char('v')));
        assert_eq!(action, InputAction::PasteFromClipboard);
    }

    #[test]
    fn f5_toggles_conversation_filter() {
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::F(5)));
        assert_eq!(action, InputAction::ToggleConversationFilter);
    }

    #[test]
    fn f6_opens_session_list() {
        let action = dispatch_key(key(KeyModifiers::NONE, KeyCode::F(6)));
        assert_eq!(action, InputAction::OpenSessionList);
    }

    // --- Phase 93: Cross-Platform SOTA key tests ---

    #[test]
    fn ctrl_o_opens_file_picker() {
        let action = dispatch_key(key(KeyModifiers::CONTROL, KeyCode::Char('o')));
        assert_eq!(action, InputAction::OpenFilePicker);
    }

    #[test]
    fn ctrl_backspace_removes_last_attachment() {
        let action = dispatch_key(key(KeyModifiers::CONTROL, KeyCode::Backspace));
        assert_eq!(action, InputAction::RemoveLastAttachment);
    }

    #[test]
    fn ctrl_b_toggles_sub_agent_detail() {
        let action = dispatch_key(key(KeyModifiers::CONTROL, KeyCode::Char('b')));
        assert_eq!(action, InputAction::ToggleSubAgentDetail);
    }
}
