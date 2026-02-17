//! Activity navigation state management.
//!
//! **Phase A1: Foundation — Activity Navigator**
//!
//! Manages navigation state independently from data model:
//! - J/K line selection (vim-style)
//! - Expand/collapse state per tool
//! - Search mode with match navigation (n/N)
//! - Scroll offset for virtual scrolling

use std::collections::HashSet;
use super::activity_model::ActivityModel;

/// Navigation state for the activity zone.
///
/// Separation of concerns: this owns navigation state,
/// ActivityModel owns data, ActivityRenderer owns presentation.
#[derive(Debug, Clone)]
pub struct ActivityNavigator {
    /// Currently selected line index (for J/K navigation).
    /// None = no selection (initial state).
    pub selected_index: Option<usize>,

    /// Scroll offset for virtual scrolling.
    /// Top line index currently visible in viewport.
    pub scroll_offset: usize,

    /// Set of expanded tool execution line indices.
    /// Tools not in this set are collapsed (preview mode).
    pub expanded_tools: HashSet<usize>,

    /// Whether search mode is active (/ key pressed).
    pub search_active: bool,

    /// Current search query.
    pub search_query: String,

    /// Search match indices (from ActivityModel.search()).
    pub search_matches: Vec<usize>,

    /// Current match index in search_matches (for n/N navigation).
    pub search_current: usize,

    /// Auto-scroll to bottom when new lines arrive.
    pub auto_scroll: bool,

    /// Cached max scroll value from last render (for clamping).
    pub(crate) last_max_scroll: usize,

    /// Phase B4: Currently hovered line index (for mouse hover effects).
    /// None = no hover (mouse outside activity zone or not moved yet).
    pub hovered_line: Option<usize>,

    /// Phase 1 Remediation: Cached viewport height from last render.
    /// Used by ensure_selected_visible() for proper centering.
    /// Updated each frame by app.rs after render().
    pub(crate) viewport_height: Option<usize>,

    /// Phase 2 NAV-001: Pending key for multi-character sequences (e.g., 'g' for gu/gt/ge).
    /// Cleared on non-matching key or after successful command execution.
    pub(crate) pending_key: Option<char>,

    /// Phase 3 SRCH-001: Fuzzy search mode enabled (Levenshtein distance tolerance).
    /// When true, search tolerates typos within max_distance (default 2).
    pub fuzzy_mode: bool,

    /// Phase 3 SRCH-001: Maximum edit distance for fuzzy matching (default 2).
    pub fuzzy_max_distance: usize,

    /// Phase 3 SRCH-002: Regex search mode enabled.
    /// When true, query is treated as a regular expression pattern.
    pub regex_mode: bool,
}

impl ActivityNavigator {
    pub fn new() -> Self {
        Self {
            selected_index: None,
            scroll_offset: 0,
            expanded_tools: HashSet::new(),
            search_active: false,
            search_query: String::new(),
            search_matches: Vec::new(),
            search_current: 0,
            auto_scroll: true,
            last_max_scroll: 0,
            hovered_line: None, // Phase B4
            viewport_height: None, // Phase 1 Remediation
            pending_key: None, // Phase 2 NAV-001
            fuzzy_mode: false, // Phase 3 SRCH-001
            fuzzy_max_distance: 2, // Phase 3 SRCH-001
            regex_mode: false, // Phase 3 SRCH-002
        }
    }

    /// Select the next line (J key — vim-style down).
    pub fn select_next(&mut self, model: &ActivityModel) {
        let total = model.len();
        if total == 0 {
            return;
        }

        self.selected_index = Some(match self.selected_index {
            Some(idx) if idx + 1 < total => idx + 1,
            Some(idx) => idx, // At last line, stay there
            None => 0, // No selection → select first line
        });

        // Disable auto-scroll when selecting
        self.auto_scroll = false;

        // Update scroll if selection moved off-screen
        self.ensure_selected_visible();
    }

    /// Select the previous line (K key — vim-style up).
    pub fn select_prev(&mut self, model: &ActivityModel) {
        if model.is_empty() {
            return;
        }

        self.selected_index = Some(match self.selected_index {
            Some(idx) if idx > 0 => idx - 1,
            Some(idx) => idx, // At first line, stay there
            None => 0, // No selection → select first line
        });

        self.auto_scroll = false;
        self.ensure_selected_visible();
    }

    /// Toggle expand/collapse state for the currently selected tool.
    pub fn toggle_expand(&mut self, line_idx: usize) {
        if self.expanded_tools.contains(&line_idx) {
            self.expanded_tools.remove(&line_idx);
        } else {
            self.expanded_tools.insert(line_idx);
        }
    }

    /// Check if a specific line is expanded.
    pub fn is_expanded(&self, line_idx: usize) -> bool {
        self.expanded_tools.contains(&line_idx)
    }

    /// Expand all tool executions.
    pub fn expand_all_tools(&mut self, model: &ActivityModel) {
        use super::activity_types::ActivityLine;
        for (idx, line) in model.all_lines().iter().enumerate() {
            if matches!(line, ActivityLine::ToolExec { .. }) {
                self.expanded_tools.insert(idx);
            }
        }
    }

    /// Collapse all tool executions.
    pub fn collapse_all_tools(&mut self) {
        self.expanded_tools.clear();
    }

    /// Clear the current selection.
    pub fn clear_selection(&mut self) {
        self.selected_index = None;
    }

    /// Enter search mode with a query.
    /// Phase 3 SRCH-001: Uses fuzzy search if fuzzy_mode is enabled.
    /// Phase 3 SRCH-002: Uses regex search if regex_mode is enabled (takes priority).
    pub fn enter_search(&mut self, query: String, model: &ActivityModel) {
        self.search_active = true;
        self.search_query = query.clone();

        // Phase 3: Choose search mode (priority: regex > fuzzy > exact)
        self.search_matches = if self.regex_mode {
            model.regex_search(&query)
        } else if self.fuzzy_mode {
            model.fuzzy_search(&query, self.fuzzy_max_distance)
        } else {
            model.search(&query)
        };

        self.search_current = 0;

        // Auto-select first match
        if !self.search_matches.is_empty() {
            self.selected_index = Some(self.search_matches[0]);
            self.ensure_selected_visible();
        }
    }

    /// Navigate to the next search match (n key).
    pub fn search_next(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }

        self.search_current = (self.search_current + 1) % self.search_matches.len();
        self.selected_index = Some(self.search_matches[self.search_current]);
        self.ensure_selected_visible();
    }

    /// Navigate to the previous search match (N key).
    pub fn search_prev(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }

        self.search_current = if self.search_current == 0 {
            self.search_matches.len() - 1
        } else {
            self.search_current - 1
        };
        self.selected_index = Some(self.search_matches[self.search_current]);
        self.ensure_selected_visible();
    }

    /// Exit search mode.
    pub fn exit_search(&mut self) {
        self.search_active = false;
        self.search_query.clear();
        self.search_matches.clear();
        self.search_current = 0;
    }

    /// Scroll up by `n` lines.
    pub fn scroll_up(&mut self, n: usize) {
        if self.auto_scroll {
            // Switching from auto-scroll: start from bottom
            self.scroll_offset = self.last_max_scroll;
        }
        self.auto_scroll = false;
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    /// Scroll down by `n` lines.
    pub fn scroll_down(&mut self, n: usize) {
        self.auto_scroll = false;
        self.scroll_offset = self.scroll_offset.saturating_add(n);

        // Re-enable auto-scroll when reaching bottom
        if self.scroll_offset >= self.last_max_scroll {
            self.scroll_to_bottom();
        }
    }

    /// Scroll to the bottom and re-enable auto-scroll.
    pub fn scroll_to_bottom(&mut self) {
        self.auto_scroll = true;
        self.scroll_offset = self.last_max_scroll;
    }

    /// Ensure the selected line is visible in the viewport.
    /// Adjusts scroll_offset if needed.
    ///
    /// **Phase 1 Remediation**: Now uses cached viewport_height for proper centering.
    /// Centers selection in middle of viewport instead of hard-coded 5-line offset.
    fn ensure_selected_visible(&mut self) {
        if let Some(selected) = self.selected_index {
            // Use cached viewport_height if available, otherwise fallback to 20-line estimate
            let viewport = self.viewport_height.unwrap_or(20);
            let half_viewport = viewport / 2;

            // Center selection in viewport
            let target_scroll = selected.saturating_sub(half_viewport);

            // Clamp to valid range [0, last_max_scroll]
            self.scroll_offset = target_scroll.min(self.last_max_scroll);
        }
    }

    /// Update scroll offset to center the given line index.
    /// Called by renderer when viewport height is known.
    pub fn scroll_to_line(&mut self, line_idx: usize, viewport_height: usize) {
        let half_viewport = viewport_height / 2;
        self.scroll_offset = line_idx.saturating_sub(half_viewport);
        self.auto_scroll = false;
    }

    /// Get the currently selected line index.
    pub fn selected(&self) -> Option<usize> {
        self.selected_index
    }

    /// Check if search mode is active.
    pub fn is_searching(&self) -> bool {
        self.search_active
    }

    /// Get search match count.
    pub fn match_count(&self) -> usize {
        self.search_matches.len()
    }

    /// Get current match position (1-indexed for display).
    pub fn current_match_position(&self) -> Option<usize> {
        if self.search_matches.is_empty() {
            None
        } else {
            Some(self.search_current + 1)
        }
    }

    /// Phase 3 SRCH-003: Get search mode description for UI display.
    /// Returns "Exact", "Fuzzy", or "Regex" based on active mode.
    pub fn search_mode_label(&self) -> &'static str {
        if self.regex_mode {
            "Regex"
        } else if self.fuzzy_mode {
            "Fuzzy"
        } else {
            "Exact"
        }
    }

    /// Phase 3 SRCH-003: Get all search match indices (for highlighting).
    pub fn all_matches(&self) -> &[usize] {
        &self.search_matches
    }

    // Phase B4: Hover state management

    /// Set the currently hovered line index.
    pub fn set_hover(&mut self, line_idx: Option<usize>) {
        self.hovered_line = line_idx;
    }

    /// Get the currently hovered line index.
    pub fn hovered(&self) -> Option<usize> {
        self.hovered_line
    }

    /// Check if a specific line is hovered.
    pub fn is_hovered(&self, idx: usize) -> bool {
        self.hovered_line == Some(idx)
    }

    /// Clear hover state (mouse left activity zone).
    pub fn clear_hover(&mut self) {
        self.hovered_line = None;
    }

    // Phase 2 NAV-001: Smart jump-to-context navigation

    /// Jump to the next user message (UserPrompt) from current position.
    /// Wraps around if no match found after current position.
    pub fn jump_to_next_user(&mut self, model: &ActivityModel) {
        use super::activity_types::ActivityLine;

        let total = model.len();
        if total == 0 {
            return;
        }

        let start = self.selected_index.map(|i| i + 1).unwrap_or(0);

        // Search from current position forward
        for idx in start..total {
            if matches!(model.get(idx), Some(ActivityLine::UserPrompt(_))) {
                self.selected_index = Some(idx);
                self.auto_scroll = false;
                self.ensure_selected_visible();
                return;
            }
        }

        // Wrap around: search from start to current position
        for idx in 0..start {
            if matches!(model.get(idx), Some(ActivityLine::UserPrompt(_))) {
                self.selected_index = Some(idx);
                self.auto_scroll = false;
                self.ensure_selected_visible();
                return;
            }
        }
    }

    /// Jump to the next tool execution (ToolExec) from current position.
    /// Wraps around if no match found after current position.
    pub fn jump_to_next_tool(&mut self, model: &ActivityModel) {
        use super::activity_types::ActivityLine;

        let total = model.len();
        if total == 0 {
            return;
        }

        let start = self.selected_index.map(|i| i + 1).unwrap_or(0);

        // Search from current position forward
        for idx in start..total {
            if matches!(model.get(idx), Some(ActivityLine::ToolExec { .. })) {
                self.selected_index = Some(idx);
                self.auto_scroll = false;
                self.ensure_selected_visible();
                return;
            }
        }

        // Wrap around
        for idx in 0..start {
            if matches!(model.get(idx), Some(ActivityLine::ToolExec { .. })) {
                self.selected_index = Some(idx);
                self.auto_scroll = false;
                self.ensure_selected_visible();
                return;
            }
        }
    }

    /// Jump to the next error (Error) from current position.
    /// Wraps around if no match found after current position.
    pub fn jump_to_next_error(&mut self, model: &ActivityModel) {
        use super::activity_types::ActivityLine;

        let total = model.len();
        if total == 0 {
            return;
        }

        let start = self.selected_index.map(|i| i + 1).unwrap_or(0);

        // Search from current position forward
        for idx in start..total {
            if matches!(model.get(idx), Some(ActivityLine::Error { .. })) {
                self.selected_index = Some(idx);
                self.auto_scroll = false;
                self.ensure_selected_visible();
                return;
            }
        }

        // Wrap around
        for idx in 0..start {
            if matches!(model.get(idx), Some(ActivityLine::Error { .. })) {
                self.selected_index = Some(idx);
                self.auto_scroll = false;
                self.ensure_selected_visible();
                return;
            }
        }
    }

    /// Set pending key for multi-character sequences.
    pub fn set_pending_key(&mut self, key: char) {
        self.pending_key = Some(key);
    }

    /// Clear pending key (on non-matching input or successful command).
    pub fn clear_pending_key(&mut self) {
        self.pending_key = None;
    }

    /// Get pending key if set.
    pub fn get_pending_key(&self) -> Option<char> {
        self.pending_key
    }

    // Phase 3 SRCH-001: Fuzzy search mode management

    /// Toggle fuzzy search mode on/off.
    pub fn toggle_fuzzy_mode(&mut self) {
        self.fuzzy_mode = !self.fuzzy_mode;
    }

    /// Check if fuzzy search mode is enabled.
    pub fn is_fuzzy_mode(&self) -> bool {
        self.fuzzy_mode
    }

    /// Set fuzzy search maximum edit distance.
    pub fn set_fuzzy_distance(&mut self, distance: usize) {
        self.fuzzy_max_distance = distance;
    }

    // Phase 3 SRCH-002: Regex search mode management

    /// Toggle regex search mode on/off.
    /// When enabled, disables fuzzy mode (mutually exclusive).
    pub fn toggle_regex_mode(&mut self) {
        self.regex_mode = !self.regex_mode;
        if self.regex_mode {
            self.fuzzy_mode = false; // Regex and fuzzy are mutually exclusive
        }
    }

    /// Check if regex search mode is enabled.
    pub fn is_regex_mode(&self) -> bool {
        self.regex_mode
    }
}

impl Default for ActivityNavigator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::activity_types::ActivityLine;

    fn test_model_with_lines(count: usize) -> ActivityModel {
        let mut model = ActivityModel::new();
        for i in 0..count {
            model.push(ActivityLine::UserPrompt(format!("line {}", i)));
        }
        model
    }

    #[test]
    fn new_navigator_has_no_selection() {
        let nav = ActivityNavigator::new();
        assert_eq!(nav.selected_index, None);
        assert!(nav.auto_scroll);
    }

    #[test]
    fn select_next_from_none_selects_first() {
        let mut nav = ActivityNavigator::new();
        let model = test_model_with_lines(5);

        nav.select_next(&model);
        assert_eq!(nav.selected_index, Some(0));
    }

    #[test]
    fn select_next_advances() {
        let mut nav = ActivityNavigator::new();
        let model = test_model_with_lines(5);

        nav.select_next(&model);
        nav.select_next(&model);
        nav.select_next(&model);
        assert_eq!(nav.selected_index, Some(2));
    }

    #[test]
    fn select_next_stops_at_last() {
        let mut nav = ActivityNavigator::new();
        let model = test_model_with_lines(3);

        nav.select_next(&model); // 0
        nav.select_next(&model); // 1
        nav.select_next(&model); // 2
        nav.select_next(&model); // Still 2 (clamped)
        assert_eq!(nav.selected_index, Some(2));
    }

    #[test]
    fn select_prev_from_none_selects_first() {
        let mut nav = ActivityNavigator::new();
        let model = test_model_with_lines(5);

        nav.select_prev(&model);
        assert_eq!(nav.selected_index, Some(0));
    }

    #[test]
    fn select_prev_goes_backward() {
        let mut nav = ActivityNavigator::new();
        let model = test_model_with_lines(5);

        nav.selected_index = Some(3);
        nav.select_prev(&model);
        assert_eq!(nav.selected_index, Some(2));
    }

    #[test]
    fn select_prev_stops_at_first() {
        let mut nav = ActivityNavigator::new();
        let model = test_model_with_lines(5);

        nav.selected_index = Some(0);
        nav.select_prev(&model);
        assert_eq!(nav.selected_index, Some(0)); // Clamped at 0
    }

    #[test]
    fn select_disables_auto_scroll() {
        let mut nav = ActivityNavigator::new();
        let model = test_model_with_lines(5);

        assert!(nav.auto_scroll);
        nav.select_next(&model);
        assert!(!nav.auto_scroll);
    }

    #[test]
    fn toggle_expand_adds_and_removes() {
        let mut nav = ActivityNavigator::new();

        assert!(!nav.is_expanded(5));
        nav.toggle_expand(5);
        assert!(nav.is_expanded(5));
        nav.toggle_expand(5);
        assert!(!nav.is_expanded(5));
    }

    #[test]
    fn expand_all_tools() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::ToolExec {
            name: "bash".into(),
            input_preview: "ls".into(),
            result: None,
            expanded: false,
        });
        model.push(ActivityLine::UserPrompt("user".into()));
        model.push(ActivityLine::ToolExec {
            name: "grep".into(),
            input_preview: "pattern".into(),
            result: None,
            expanded: false,
        });

        let mut nav = ActivityNavigator::new();
        nav.expand_all_tools(&model);

        assert!(nav.is_expanded(0)); // First tool
        assert!(!nav.is_expanded(1)); // User prompt (not a tool)
        assert!(nav.is_expanded(2)); // Second tool
    }

    #[test]
    fn collapse_all_tools_clears_set() {
        let mut nav = ActivityNavigator::new();
        nav.expanded_tools.insert(0);
        nav.expanded_tools.insert(5);
        nav.expanded_tools.insert(10);

        nav.collapse_all_tools();
        assert!(nav.expanded_tools.is_empty());
    }

    #[test]
    fn enter_search_sets_state() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("hello world".into()));
        model.push(ActivityLine::UserPrompt("hello rust".into()));
        model.push(ActivityLine::UserPrompt("goodbye".into()));

        let mut nav = ActivityNavigator::new();
        nav.enter_search("hello".into(), &model);

        assert!(nav.search_active);
        assert_eq!(nav.search_query, "hello");
        assert_eq!(nav.search_matches, vec![0, 1]);
        assert_eq!(nav.selected_index, Some(0)); // First match selected
    }

    #[test]
    fn search_next_cycles_matches() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("match 1".into()));
        model.push(ActivityLine::UserPrompt("match 2".into()));
        model.push(ActivityLine::UserPrompt("match 3".into()));

        let mut nav = ActivityNavigator::new();
        nav.enter_search("match".into(), &model);

        assert_eq!(nav.selected_index, Some(0));

        nav.search_next();
        assert_eq!(nav.selected_index, Some(1));

        nav.search_next();
        assert_eq!(nav.selected_index, Some(2));

        nav.search_next(); // Wrap around
        assert_eq!(nav.selected_index, Some(0));
    }

    #[test]
    fn search_prev_cycles_backward() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("match 1".into()));
        model.push(ActivityLine::UserPrompt("match 2".into()));
        model.push(ActivityLine::UserPrompt("match 3".into()));

        let mut nav = ActivityNavigator::new();
        nav.enter_search("match".into(), &model);

        // Start at first match (0)
        nav.search_prev(); // Wrap to last (2)
        assert_eq!(nav.selected_index, Some(2));

        nav.search_prev();
        assert_eq!(nav.selected_index, Some(1));
    }

    #[test]
    fn exit_search_clears_state() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("hello".into()));

        let mut nav = ActivityNavigator::new();
        nav.enter_search("hello".into(), &model);

        assert!(nav.search_active);
        nav.exit_search();

        assert!(!nav.search_active);
        assert!(nav.search_query.is_empty());
        assert!(nav.search_matches.is_empty());
    }

    #[test]
    fn scroll_up_disables_auto_scroll() {
        let mut nav = ActivityNavigator::new();
        assert!(nav.auto_scroll);

        nav.scroll_up(3);
        assert!(!nav.auto_scroll);
    }

    #[test]
    fn scroll_down_past_bottom_re_enables_auto() {
        let mut nav = ActivityNavigator::new();
        nav.last_max_scroll = 10;
        nav.auto_scroll = false;
        nav.scroll_offset = 5;

        nav.scroll_down(20); // Scroll way past max
        assert!(nav.auto_scroll);
        assert_eq!(nav.scroll_offset, 10); // Clamped to max
    }

    #[test]
    fn match_count_returns_search_results() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("test 1".into()));
        model.push(ActivityLine::UserPrompt("test 2".into()));

        let mut nav = ActivityNavigator::new();
        nav.enter_search("test".into(), &model);

        assert_eq!(nav.match_count(), 2);
    }

    #[test]
    fn current_match_position_is_one_indexed() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("match".into()));
        model.push(ActivityLine::UserPrompt("match".into()));

        let mut nav = ActivityNavigator::new();
        nav.enter_search("match".into(), &model);

        assert_eq!(nav.current_match_position(), Some(1)); // First match = position 1

        nav.search_next();
        assert_eq!(nav.current_match_position(), Some(2)); // Second match = position 2
    }

    // Phase 2 NAV-001: Smart jump-to-context tests

    #[test]
    fn jump_to_next_user_finds_user_prompt() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::Info("info".into()));
        model.push(ActivityLine::UserPrompt("user 1".into())); // index 1
        model.push(ActivityLine::ToolExec {
            name: "test".into(),
            input_preview: "args".into(),
            result: None,
            expanded: false,
        });
        model.push(ActivityLine::UserPrompt("user 2".into())); // index 3

        let mut nav = ActivityNavigator::new();
        nav.viewport_height = Some(10);
        nav.last_max_scroll = 0;

        // From no selection, should jump to first user prompt
        nav.jump_to_next_user(&model);
        assert_eq!(nav.selected_index, Some(1));

        // From first user prompt, should jump to second
        nav.jump_to_next_user(&model);
        assert_eq!(nav.selected_index, Some(3));

        // From second user prompt, should wrap to first
        nav.jump_to_next_user(&model);
        assert_eq!(nav.selected_index, Some(1));
    }

    #[test]
    fn jump_to_next_tool_finds_tool_exec() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("user".into()));
        model.push(ActivityLine::ToolExec {
            name: "tool1".into(),
            input_preview: "args".into(),
            result: None,
            expanded: false,
        }); // index 1
        model.push(ActivityLine::Info("info".into()));
        model.push(ActivityLine::ToolExec {
            name: "tool2".into(),
            input_preview: "args".into(),
            result: None,
            expanded: false,
        }); // index 3

        let mut nav = ActivityNavigator::new();
        nav.viewport_height = Some(10);
        nav.last_max_scroll = 0;

        nav.jump_to_next_tool(&model);
        assert_eq!(nav.selected_index, Some(1));

        nav.jump_to_next_tool(&model);
        assert_eq!(nav.selected_index, Some(3));

        // Wrap around
        nav.jump_to_next_tool(&model);
        assert_eq!(nav.selected_index, Some(1));
    }

    #[test]
    fn jump_to_next_error_finds_error_lines() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("user".into()));
        model.push(ActivityLine::Error {
            message: "error 1".into(),
            hint: None,
        }); // index 1
        model.push(ActivityLine::Info("info".into()));
        model.push(ActivityLine::Error {
            message: "error 2".into(),
            hint: Some("hint".into()),
        }); // index 3

        let mut nav = ActivityNavigator::new();
        nav.viewport_height = Some(10);
        nav.last_max_scroll = 0;

        nav.jump_to_next_error(&model);
        assert_eq!(nav.selected_index, Some(1));

        nav.jump_to_next_error(&model);
        assert_eq!(nav.selected_index, Some(3));

        // Wrap around
        nav.jump_to_next_error(&model);
        assert_eq!(nav.selected_index, Some(1));
    }

    #[test]
    fn jump_to_next_user_handles_no_matches() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::Info("info".into()));
        model.push(ActivityLine::ToolExec {
            name: "tool".into(),
            input_preview: "args".into(),
            result: None,
            expanded: false,
        });

        let mut nav = ActivityNavigator::new();
        nav.viewport_height = Some(10);
        nav.last_max_scroll = 0;

        // No user prompts, selection should remain None
        nav.jump_to_next_user(&model);
        assert_eq!(nav.selected_index, None);
    }

    #[test]
    fn jump_commands_disable_auto_scroll() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("user".into()));

        let mut nav = ActivityNavigator::new();
        nav.viewport_height = Some(10);
        nav.last_max_scroll = 0;
        assert!(nav.auto_scroll); // Initially true

        nav.jump_to_next_user(&model);
        assert!(!nav.auto_scroll); // Should be disabled after jump
    }

    #[test]
    fn pending_key_lifecycle() {
        let mut nav = ActivityNavigator::new();

        assert_eq!(nav.get_pending_key(), None);

        nav.set_pending_key('g');
        assert_eq!(nav.get_pending_key(), Some('g'));

        nav.clear_pending_key();
        assert_eq!(nav.get_pending_key(), None);
    }

    // Phase 3 SRCH-001: Fuzzy search tests

    #[test]
    fn fuzzy_mode_starts_disabled() {
        let nav = ActivityNavigator::new();
        assert!(!nav.is_fuzzy_mode());
        assert_eq!(nav.fuzzy_max_distance, 2);
    }

    #[test]
    fn toggle_fuzzy_mode() {
        let mut nav = ActivityNavigator::new();

        assert!(!nav.is_fuzzy_mode());

        nav.toggle_fuzzy_mode();
        assert!(nav.is_fuzzy_mode());

        nav.toggle_fuzzy_mode();
        assert!(!nav.is_fuzzy_mode());
    }

    #[test]
    fn set_fuzzy_distance() {
        let mut nav = ActivityNavigator::new();

        nav.set_fuzzy_distance(3);
        assert_eq!(nav.fuzzy_max_distance, 3);

        nav.set_fuzzy_distance(1);
        assert_eq!(nav.fuzzy_max_distance, 1);
    }

    #[test]
    fn enter_search_uses_exact_when_fuzzy_disabled() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("hello world".into()));
        model.push(ActivityLine::UserPrompt("helo world".into())); // typo

        let mut nav = ActivityNavigator::new();
        nav.viewport_height = Some(10);
        nav.last_max_scroll = 0;

        // Fuzzy disabled: "helo" should only match line 1 (exact)
        nav.enter_search("helo".to_string(), &model);
        assert_eq!(nav.search_matches, vec![1]);
    }

    #[test]
    fn enter_search_uses_fuzzy_when_enabled() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("hello world".into()));
        model.push(ActivityLine::UserPrompt("helo world".into())); // typo

        let mut nav = ActivityNavigator::new();
        nav.viewport_height = Some(10);
        nav.last_max_scroll = 0;
        nav.fuzzy_mode = true;

        // Fuzzy enabled: "helo" should match both lines (within distance 1)
        nav.enter_search("helo".to_string(), &model);
        assert_eq!(nav.search_matches, vec![0, 1]);
    }

    // Phase 3 SRCH-002: Regex search tests

    #[test]
    fn regex_mode_starts_disabled() {
        let nav = ActivityNavigator::new();
        assert!(!nav.is_regex_mode());
    }

    #[test]
    fn toggle_regex_mode() {
        let mut nav = ActivityNavigator::new();

        assert!(!nav.is_regex_mode());

        nav.toggle_regex_mode();
        assert!(nav.is_regex_mode());

        nav.toggle_regex_mode();
        assert!(!nav.is_regex_mode());
    }

    #[test]
    fn toggle_regex_disables_fuzzy() {
        let mut nav = ActivityNavigator::new();

        // Enable both
        nav.fuzzy_mode = true;
        nav.regex_mode = true;

        // Toggle regex off then on should disable fuzzy
        nav.toggle_regex_mode(); // off
        nav.toggle_regex_mode(); // on → disables fuzzy
        assert!(nav.is_regex_mode());
        assert!(!nav.is_fuzzy_mode());
    }

    #[test]
    fn enter_search_uses_regex_when_enabled() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("error 123".into()));
        model.push(ActivityLine::UserPrompt("no errors".into()));
        model.push(ActivityLine::UserPrompt("error 456".into()));

        let mut nav = ActivityNavigator::new();
        nav.viewport_height = Some(10);
        nav.last_max_scroll = 0;
        nav.regex_mode = true;

        // Regex pattern "error \\d+" should match lines 0 and 2
        nav.enter_search(r"error \d+".to_string(), &model);
        assert_eq!(nav.search_matches, vec![0, 2]);
    }

    #[test]
    fn regex_takes_priority_over_fuzzy() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("test 123".into()));

        let mut nav = ActivityNavigator::new();
        nav.viewport_height = Some(10);
        nav.last_max_scroll = 0;
        nav.fuzzy_mode = true;
        nav.regex_mode = true;

        // Both modes enabled → regex takes priority
        nav.enter_search(r"\d+".to_string(), &model);
        assert_eq!(nav.search_matches, vec![0]); // Regex pattern matched
    }
}
