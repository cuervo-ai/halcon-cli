//! Activity interaction controller.
//!
//! **Phase A1: Foundation — Activity Controller**
//!
//! Handles user interactions and translates them into navigation actions.
//! Pure logic layer — no state mutation, returns ControlAction enum.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use super::activity_model::ActivityModel;
use super::activity_navigator::ActivityNavigator;
use super::activity_types::ActivityLine;

/// Actions that can be triggered from activity interactions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlAction {
    /// No action.
    None,

    /// Toggle expand/collapse on a tool execution.
    ToggleExpand(usize),

    /// Copy a line's content to clipboard.
    CopyOutput(usize),

    /// Jump to the plan step associated with a tool.
    JumpToPlanStep(usize),

    /// Open the inspector for a specific target.
    OpenInspector(InspectTarget),

    /// Filter activity by tool name.
    FilterByTool(String),

    /// Execute a slash command.
    SlashCommand(String),
}

/// Inspection targets for the inspector panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InspectTarget {
    ToolResult { line_idx: usize },
    PlanSteps,
    ContextPipeline,
    Metrics,
}

/// Activity interaction controller.
///
/// Stateless handler that dispatches keyboard and mouse events
/// to appropriate navigation/action methods.
pub struct ActivityController;

impl ActivityController {
    pub fn new() -> Self {
        Self
    }

    /// Handle a keyboard event while activity zone is focused.
    ///
    /// Returns a ControlAction describing the effect.
    pub fn handle_key(
        &self,
        key: KeyEvent,
        nav: &mut ActivityNavigator,
        model: &ActivityModel,
    ) -> ControlAction {
        use KeyCode::*;

        // Search mode intercepts most keys
        if nav.is_searching() {
            return self.handle_search_key(key, nav, model);
        }

        // Phase 2 NAV-001: Handle multi-character sequences (g prefix)
        if let Some(pending) = nav.get_pending_key() {
            nav.clear_pending_key(); // Clear after checking

            if pending == 'g' {
                match key.code {
                    Char('u') | Char('U') => {
                        nav.jump_to_next_user(model);
                        return ControlAction::None;
                    }
                    Char('t') | Char('T') => {
                        nav.jump_to_next_tool(model);
                        return ControlAction::None;
                    }
                    Char('e') | Char('E') => {
                        nav.jump_to_next_error(model);
                        return ControlAction::None;
                    }
                    _ => {
                        // Non-matching key after 'g', fall through to normal handling
                    }
                }
            }
        }

        // Normal mode key bindings
        match (key.code, key.modifiers) {
            // g: prefix for jump commands (gu/gt/ge)
            (Char('g') | Char('G'), _) => {
                nav.set_pending_key('g');
                ControlAction::None
            }

            // J/K navigation (vim-style)
            (Char('j') | Char('J') | Down, _) => {
                nav.select_next(model);
                ControlAction::None
            }
            (Char('k') | Char('K') | Up, _) => {
                nav.select_prev(model);
                ControlAction::None
            }

            // Enter: toggle expand on selected line (if tool or code block)
            (Enter, _) => self.handle_enter_on_selected(nav, model),

            // Esc: clear selection
            (Esc, _) => {
                nav.clear_selection();
                ControlAction::None
            }

            // /: enter search mode
            (Char('/'), _) => {
                // This will be handled by overlay in app.rs
                // Just a placeholder here
                ControlAction::None
            }

            // y: copy (yank) selected line
            (Char('y') | Char('Y'), _) => {
                if let Some(idx) = nav.selected() {
                    ControlAction::CopyOutput(idx)
                } else {
                    ControlAction::None
                }
            }

            // i: open inspector on selected line
            (Char('i') | Char('I'), _) => {
                if let Some(idx) = nav.selected() {
                    ControlAction::OpenInspector(InspectTarget::ToolResult { line_idx: idx })
                } else {
                    ControlAction::None
                }
            }

            // x: expand all tools
            (Char('x') | Char('X'), _) => {
                nav.expand_all_tools(model);
                ControlAction::None
            }

            // z: collapse all tools
            (Char('z') | Char('Z'), _) => {
                nav.collapse_all_tools();
                ControlAction::None
            }

            // p: jump to plan step (if on tool line)
            (Char('p') | Char('P'), _) => {
                if let Some(idx) = nav.selected() {
                    if let Some(step) = model.metadata.step_for_line(idx) {
                        ControlAction::JumpToPlanStep(step)
                    } else {
                        ControlAction::None
                    }
                } else {
                    ControlAction::None
                }
            }

            // PageUp/PageDown: scroll by page
            (PageUp, _) => {
                nav.scroll_up(10); // 10 lines per page
                ControlAction::None
            }
            (PageDown, _) => {
                nav.scroll_down(10);
                ControlAction::None
            }

            // Home/End: scroll to top/bottom
            (Home, _) => {
                nav.scroll_offset = 0;
                nav.auto_scroll = false;
                ControlAction::None
            }
            (End, _) => {
                nav.scroll_to_bottom();
                ControlAction::None
            }

            _ => ControlAction::None,
        }
    }

    /// Handle keyboard events in search mode.
    fn handle_search_key(
        &self,
        key: KeyEvent,
        nav: &mut ActivityNavigator,
        _model: &ActivityModel,
    ) -> ControlAction {
        use KeyCode::*;

        match key.code {
            // n: next match
            Char('n') => {
                nav.search_next();
                ControlAction::None
            }
            // N: previous match
            Char('N') => {
                nav.search_prev();
                ControlAction::None
            }
            // Phase 3 SRCH-001: f: toggle fuzzy search mode
            Char('f') | Char('F') => {
                nav.toggle_fuzzy_mode();
                // Re-run search with new mode
                let query = nav.search_query.clone();
                nav.enter_search(query, _model);
                ControlAction::None
            }
            // Phase 3 SRCH-002: r: toggle regex search mode
            Char('r') | Char('R') => {
                nav.toggle_regex_mode();
                // Re-run search with new mode
                let query = nav.search_query.clone();
                nav.enter_search(query, _model);
                ControlAction::None
            }
            // Enter: select current match and exit search
            Enter => {
                nav.exit_search();
                ControlAction::None
            }
            // Esc: exit search without selection
            Esc => {
                nav.exit_search();
                ControlAction::None
            }
            _ => ControlAction::None,
        }
    }

    /// Handle Enter key on selected line.
    fn handle_enter_on_selected(
        &self,
        nav: &mut ActivityNavigator,
        model: &ActivityModel,
    ) -> ControlAction {
        if let Some(idx) = nav.selected() {
            if let Some(line) = model.get(idx) {
                match line {
                    ActivityLine::ToolExec { .. } | ActivityLine::CodeBlock { .. } => {
                        nav.toggle_expand(idx);
                        ControlAction::ToggleExpand(idx)
                    }
                    ActivityLine::PlanOverview { .. } => {
                        ControlAction::OpenInspector(InspectTarget::PlanSteps)
                    }
                    _ => ControlAction::None,
                }
            } else {
                ControlAction::None
            }
        } else {
            ControlAction::None
        }
    }

    /// Handle a mouse event in the activity zone.
    ///
    /// Returns a ControlAction describing the effect.
    pub fn handle_mouse(
        &self,
        mouse: MouseEvent,
        area: Rect,
        nav: &mut ActivityNavigator,
        model: &ActivityModel,
        viewport_height: usize,
    ) -> ControlAction {
        // Check if mouse is within activity area
        if mouse.column < area.x
            || mouse.column >= area.x + area.width
            || mouse.row < area.y
            || mouse.row >= area.y + area.height
        {
            return ControlAction::None;
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Calculate which line was clicked
                let relative_row = mouse.row.saturating_sub(area.y + 1); // +1 for border
                let line_idx = nav.scroll_offset + relative_row as usize;

                if line_idx < model.len() {
                    // Select the clicked line
                    nav.selected_index = Some(line_idx);

                    // If it's a tool or code block, toggle expand
                    if let Some(line) = model.get(line_idx) {
                        match line {
                            ActivityLine::ToolExec { .. } | ActivityLine::CodeBlock { .. } => {
                                nav.toggle_expand(line_idx);
                                return ControlAction::ToggleExpand(line_idx);
                            }
                            _ => {}
                        }
                    }
                }
                ControlAction::None
            }

            MouseEventKind::ScrollUp => {
                self.handle_scroll(-3, nav) // 3 lines per scroll tick
            }

            MouseEventKind::ScrollDown => {
                self.handle_scroll(3, nav) // 3 lines per scroll tick
            }

            // Phase B4: Hover tracking
            MouseEventKind::Moved => {
                // Calculate which line is hovered
                let relative_row = mouse.row.saturating_sub(area.y + 1); // +1 for border
                let line_idx = nav.scroll_offset + relative_row as usize;

                if line_idx < model.len() {
                    nav.set_hover(Some(line_idx));
                } else {
                    nav.clear_hover();
                }
                ControlAction::None
            }

            _ => {
                // Mouse left activity zone or other event
                nav.clear_hover();
                ControlAction::None
            }
        }
    }

    /// Handle scroll wheel delta.
    ///
    /// Positive delta = scroll down, negative = scroll up.
    pub fn handle_scroll(&self, delta: i16, nav: &mut ActivityNavigator) -> ControlAction {
        if delta < 0 {
            nav.scroll_up(delta.unsigned_abs() as usize);
        } else {
            nav.scroll_down(delta as usize);
        }
        ControlAction::None
    }

    /// Get contextual actions available for the currently selected line.
    ///
    /// Used for rendering hint text in status bar.
    pub fn contextual_actions(
        &self,
        nav: &ActivityNavigator,
        model: &ActivityModel,
    ) -> Vec<(&'static str, &'static str)> {
        if let Some(idx) = nav.selected() {
            if let Some(line) = model.get(idx) {
                match line {
                    ActivityLine::ToolExec { .. } => vec![
                        ("Enter", "expand/collapse"),
                        ("y", "copy"),
                        ("i", "inspect"),
                        ("p", "jump to plan"),
                    ],
                    ActivityLine::CodeBlock { .. } => vec![
                        ("Enter", "expand/collapse"),
                        ("y", "copy"),
                    ],
                    ActivityLine::PlanOverview { .. } => vec![
                        ("Enter", "view plan"),
                        ("i", "inspect"),
                    ],
                    ActivityLine::UserPrompt(_) | ActivityLine::AssistantText(_) => vec![
                        ("y", "copy"),
                    ],
                    _ => vec![],
                }
            } else {
                vec![]
            }
        } else {
            // No selection — show general navigation hints
            vec![
                ("J/K", "navigate"),
                ("gu/gt/ge", "jump to user/tool/error"),
                ("/", "search"),
                ("x", "expand all"),
                ("z", "collapse all"),
            ]
        }
    }
}

impl Default for ActivityController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_model_with_lines() -> ActivityModel {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("user 1".into()));
        model.push(ActivityLine::ToolExec {
            name: "bash".into(),
            input_preview: "ls".into(),
            result: None,
            expanded: false,
        });
        model.push(ActivityLine::CodeBlock {
            lang: "rust".into(),
            code: "fn main() {}".into(),
        });
        model.push(ActivityLine::AssistantText("assistant".into()));
        model
    }

    #[test]
    fn handle_key_j_selects_next() {
        let controller = ActivityController::new();
        let mut nav = ActivityNavigator::new();
        let model = test_model_with_lines();

        let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty());
        controller.handle_key(key, &mut nav, &model);

        assert_eq!(nav.selected(), Some(0));
    }

    #[test]
    fn handle_key_k_selects_prev() {
        let controller = ActivityController::new();
        let mut nav = ActivityNavigator::new();
        let model = test_model_with_lines();

        nav.selected_index = Some(2);

        let key = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::empty());
        controller.handle_key(key, &mut nav, &model);

        assert_eq!(nav.selected(), Some(1));
    }

    #[test]
    fn handle_key_enter_on_tool_toggles_expand() {
        let controller = ActivityController::new();
        let mut nav = ActivityNavigator::new();
        let model = test_model_with_lines();

        nav.selected_index = Some(1); // Tool line

        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::empty());
        let action = controller.handle_key(key, &mut nav, &model);

        assert_eq!(action, ControlAction::ToggleExpand(1));
        assert!(nav.is_expanded(1));
    }

    #[test]
    fn handle_key_enter_on_code_block_toggles_expand() {
        let controller = ActivityController::new();
        let mut nav = ActivityNavigator::new();
        let model = test_model_with_lines();

        nav.selected_index = Some(2); // Code block line

        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::empty());
        let action = controller.handle_key(key, &mut nav, &model);

        assert_eq!(action, ControlAction::ToggleExpand(2));
        assert!(nav.is_expanded(2));
    }

    #[test]
    fn handle_key_enter_on_user_prompt_no_action() {
        let controller = ActivityController::new();
        let mut nav = ActivityNavigator::new();
        let model = test_model_with_lines();

        nav.selected_index = Some(0); // User prompt

        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::empty());
        let action = controller.handle_key(key, &mut nav, &model);

        assert_eq!(action, ControlAction::None);
    }

    #[test]
    fn handle_key_esc_clears_selection() {
        let controller = ActivityController::new();
        let mut nav = ActivityNavigator::new();
        let model = test_model_with_lines();

        nav.selected_index = Some(1);

        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::empty());
        controller.handle_key(key, &mut nav, &model);

        assert_eq!(nav.selected(), None);
    }

    #[test]
    fn handle_key_y_copies_selected() {
        let controller = ActivityController::new();
        let mut nav = ActivityNavigator::new();
        let model = test_model_with_lines();

        nav.selected_index = Some(3);

        let key = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::empty());
        let action = controller.handle_key(key, &mut nav, &model);

        assert_eq!(action, ControlAction::CopyOutput(3));
    }

    #[test]
    fn handle_key_i_opens_inspector() {
        let controller = ActivityController::new();
        let mut nav = ActivityNavigator::new();
        let model = test_model_with_lines();

        nav.selected_index = Some(1);

        let key = KeyEvent::new(KeyCode::Char('i'), KeyModifiers::empty());
        let action = controller.handle_key(key, &mut nav, &model);

        assert_eq!(action, ControlAction::OpenInspector(InspectTarget::ToolResult { line_idx: 1 }));
    }

    #[test]
    fn handle_key_x_expands_all_tools() {
        let controller = ActivityController::new();
        let mut nav = ActivityNavigator::new();
        let model = test_model_with_lines();

        let key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::empty());
        controller.handle_key(key, &mut nav, &model);

        assert!(nav.is_expanded(1)); // Tool line should be expanded
    }

    #[test]
    fn handle_key_z_collapses_all_tools() {
        let controller = ActivityController::new();
        let mut nav = ActivityNavigator::new();
        let model = test_model_with_lines();

        nav.expanded_tools.insert(1);
        nav.expanded_tools.insert(2);

        let key = KeyEvent::new(KeyCode::Char('z'), KeyModifiers::empty());
        controller.handle_key(key, &mut nav, &model);

        assert!(nav.expanded_tools.is_empty());
    }

    #[test]
    fn handle_scroll_positive_scrolls_down() {
        let controller = ActivityController::new();
        let mut nav = ActivityNavigator::new();
        // Phase 1: scroll_down re-enables auto when reaching max
        nav.last_max_scroll = 100; // Set high max so auto_scroll doesn't re-enable

        controller.handle_scroll(5, &mut nav);

        assert_eq!(nav.scroll_offset, 5);
        assert!(!nav.auto_scroll);
    }

    #[test]
    fn handle_scroll_negative_scrolls_up() {
        let controller = ActivityController::new();
        let mut nav = ActivityNavigator::new();

        // Phase 1: scroll_up checks auto_scroll flag and resets offset if true
        // So we need to disable auto_scroll first
        nav.auto_scroll = false;
        nav.scroll_offset = 10;
        controller.handle_scroll(-3, &mut nav);

        assert_eq!(nav.scroll_offset, 7);
    }

    #[test]
    fn contextual_actions_for_tool_line() {
        let controller = ActivityController::new();
        let mut nav = ActivityNavigator::new();
        let model = test_model_with_lines();

        nav.selected_index = Some(1); // Tool line

        let actions = controller.contextual_actions(&nav, &model);

        assert!(actions.iter().any(|(k, _)| *k == "Enter"));
        assert!(actions.iter().any(|(k, _)| *k == "y"));
        assert!(actions.iter().any(|(k, _)| *k == "i"));
    }

    #[test]
    fn contextual_actions_for_no_selection() {
        let controller = ActivityController::new();
        let nav = ActivityNavigator::new();
        let model = test_model_with_lines();

        let actions = controller.contextual_actions(&nav, &model);

        assert!(actions.iter().any(|(k, _)| *k == "J/K"));
        assert!(actions.iter().any(|(k, _)| *k == "/"));
    }

    #[test]
    fn search_mode_n_advances_to_next_match() {
        let controller = ActivityController::new();
        let mut nav = ActivityNavigator::new();
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("match 1".into()));
        model.push(ActivityLine::UserPrompt("match 2".into()));

        nav.enter_search("match".into(), &model);
        assert_eq!(nav.selected(), Some(0));

        let key = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::empty());
        controller.handle_key(key, &mut nav, &model);

        assert_eq!(nav.selected(), Some(1));
    }

    #[test]
    fn search_mode_shift_n_goes_to_prev_match() {
        let controller = ActivityController::new();
        let mut nav = ActivityNavigator::new();
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("match 1".into()));
        model.push(ActivityLine::UserPrompt("match 2".into()));

        nav.enter_search("match".into(), &model);
        nav.search_next(); // Move to second match
        assert_eq!(nav.selected(), Some(1));

        let key = KeyEvent::new(KeyCode::Char('N'), KeyModifiers::empty());
        controller.handle_key(key, &mut nav, &model);

        assert_eq!(nav.selected(), Some(0));
    }

    #[test]
    fn search_mode_esc_exits_search() {
        let controller = ActivityController::new();
        let mut nav = ActivityNavigator::new();
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("test".into()));

        nav.enter_search("test".into(), &model);
        assert!(nav.is_searching());

        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::empty());
        controller.handle_key(key, &mut nav, &model);

        assert!(!nav.is_searching());
    }
}
