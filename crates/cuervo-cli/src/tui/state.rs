//! UI state management for the TUI application.

/// Which zone currently has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusZone {
    Prompt,
    Activity,
}

/// Top-level TUI application state.
pub struct AppState {
    /// Which zone has keyboard focus.
    pub focus: FocusZone,
    /// Whether the agent loop is currently running.
    pub agent_running: bool,
    /// Whether the user has requested quit.
    pub should_quit: bool,
    /// Spinner frame index (cycles during inference).
    pub spinner_frame: usize,
    /// Whether the spinner is active.
    pub spinner_active: bool,
    /// Spinner label text.
    pub spinner_label: String,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            focus: FocusZone::Prompt,
            agent_running: false,
            should_quit: false,
            spinner_frame: 0,
            spinner_active: false,
            spinner_label: String::new(),
        }
    }

    /// Cycle focus to the next zone.
    pub fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            FocusZone::Prompt => FocusZone::Activity,
            FocusZone::Activity => FocusZone::Prompt,
        };
    }

    /// Advance the spinner animation frame.
    pub fn tick_spinner(&mut self) {
        if self.spinner_active {
            self.spinner_frame = (self.spinner_frame + 1) % 10;
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_state() {
        let state = AppState::new();
        assert_eq!(state.focus, FocusZone::Prompt);
        assert!(!state.agent_running);
        assert!(!state.should_quit);
    }

    #[test]
    fn cycle_focus_prompt_to_activity() {
        let mut state = AppState::new();
        assert_eq!(state.focus, FocusZone::Prompt);
        state.cycle_focus();
        assert_eq!(state.focus, FocusZone::Activity);
    }

    #[test]
    fn cycle_focus_activity_to_prompt() {
        let mut state = AppState::new();
        state.focus = FocusZone::Activity;
        state.cycle_focus();
        assert_eq!(state.focus, FocusZone::Prompt);
    }

    #[test]
    fn spinner_tick_advances() {
        let mut state = AppState::new();
        state.spinner_active = true;
        state.tick_spinner();
        assert_eq!(state.spinner_frame, 1);
        state.tick_spinner();
        assert_eq!(state.spinner_frame, 2);
    }

    #[test]
    fn spinner_tick_wraps() {
        let mut state = AppState::new();
        state.spinner_active = true;
        state.spinner_frame = 9;
        state.tick_spinner();
        assert_eq!(state.spinner_frame, 0);
    }

    #[test]
    fn spinner_tick_inactive_no_advance() {
        let mut state = AppState::new();
        state.spinner_active = false;
        state.tick_spinner();
        assert_eq!(state.spinner_frame, 0);
    }
}
