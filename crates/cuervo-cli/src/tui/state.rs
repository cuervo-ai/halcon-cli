//! UI state management for the TUI application.

/// Which zone currently has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusZone {
    Prompt,
    Activity,
}

/// Which section of the side panel is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelSection {
    Plan,
    Metrics,
    Context,
    Reasoning,
    All,
}

impl PanelSection {
    /// Cycle to the next section.
    pub fn next(self) -> Self {
        match self {
            PanelSection::Plan => PanelSection::Metrics,
            PanelSection::Metrics => PanelSection::Context,
            PanelSection::Context => PanelSection::Reasoning,
            PanelSection::Reasoning => PanelSection::All,
            PanelSection::All => PanelSection::Plan,
        }
    }
}

/// Display mode controlling progressive disclosure of UI elements.
///
/// Cycle: Minimal → Standard → Expert → Minimal (F3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiMode {
    /// Chat + status bar only. No side panel.
    Minimal,
    /// Chat + status bar + side panel (plan, metrics, health).
    Standard,
    /// Full observability: extended status, side panel, inspector.
    Expert,
}

impl UiMode {
    /// Cycle to the next mode.
    pub fn next(self) -> Self {
        match self {
            UiMode::Minimal => UiMode::Standard,
            UiMode::Standard => UiMode::Expert,
            UiMode::Expert => UiMode::Minimal,
        }
    }

    /// Human-readable label for the status bar.
    pub fn label(self) -> &'static str {
        match self {
            UiMode::Minimal => "Minimal",
            UiMode::Standard => "Standard",
            UiMode::Expert => "Expert",
        }
    }
}

/// Agent execution control state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentControl {
    Running,
    Paused,
    StepMode,
    WaitingApproval,
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

    // Phase 42C: Cockpit state
    /// Whether the side panel is visible.
    pub panel_visible: bool,
    /// Which panel section is active.
    pub panel_section: PanelSection,
    /// UI display mode (simple vs expert).
    pub ui_mode: UiMode,
    /// Agent control state (Phase 42D).
    pub agent_control: AgentControl,

    // Phase 44A: Observability state
    /// Whether dry-run mode is active (destructive tools skipped).
    pub dry_run_active: bool,
    /// Token budget tracking.
    pub token_budget: TokenBudget,

    // Phase C: Overlay state
    /// Overlay system state (command palette, search, help, permissions).
    pub overlay: super::overlay::OverlayState,

    /// Persisted agent execution state (FSM). Updated on AgentStateTransition events.
    pub agent_state: super::events::AgentState,
}

/// Token budget usage for the status bar progress indicator.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokenBudget {
    /// Tokens used so far.
    pub used: u64,
    /// Token limit (0 = unlimited).
    pub limit: u64,
    /// Current burn rate in tokens per minute.
    pub rate_per_minute: f64,
}

impl TokenBudget {
    /// Returns the usage fraction (0.0 - 1.0), or None if unlimited.
    pub fn fraction(&self) -> Option<f64> {
        if self.limit == 0 {
            None
        } else {
            Some((self.used as f64 / self.limit as f64).min(1.0))
        }
    }

    /// Returns a compact display string like "45%" or "∞".
    pub fn display(&self) -> String {
        match self.fraction() {
            Some(f) => format!("{}%", (f * 100.0) as u32),
            None => "∞".to_string(),
        }
    }
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
            panel_visible: false,
            panel_section: PanelSection::All,
            ui_mode: UiMode::Standard,
            agent_control: AgentControl::Running,
            dry_run_active: false,
            token_budget: TokenBudget::default(),
            overlay: super::overlay::OverlayState::new(),
            agent_state: super::events::AgentState::Idle,
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

    // --- Phase 42C: Cockpit state tests ---

    #[test]
    fn panel_section_cycle() {
        assert_eq!(PanelSection::Plan.next(), PanelSection::Metrics);
        assert_eq!(PanelSection::Metrics.next(), PanelSection::Context);
        assert_eq!(PanelSection::Context.next(), PanelSection::Reasoning);
        assert_eq!(PanelSection::Reasoning.next(), PanelSection::All);
        assert_eq!(PanelSection::All.next(), PanelSection::Plan);
    }

    #[test]
    fn panel_toggle_state() {
        let mut state = AppState::new();
        assert!(!state.panel_visible);
        state.panel_visible = true;
        assert!(state.panel_visible);
        state.panel_visible = false;
        assert!(!state.panel_visible);
    }

    #[test]
    fn ui_mode_default_is_standard() {
        let state = AppState::new();
        assert_eq!(state.ui_mode, UiMode::Standard);
        assert!(!state.panel_visible);
    }

    #[test]
    fn ui_mode_cycles_minimal_standard_expert() {
        assert_eq!(UiMode::Minimal.next(), UiMode::Standard);
        assert_eq!(UiMode::Standard.next(), UiMode::Expert);
        assert_eq!(UiMode::Expert.next(), UiMode::Minimal);
    }

    #[test]
    fn ui_mode_labels() {
        assert_eq!(UiMode::Minimal.label(), "Minimal");
        assert_eq!(UiMode::Standard.label(), "Standard");
        assert_eq!(UiMode::Expert.label(), "Expert");
    }

    #[test]
    fn token_budget_fraction_limited() {
        let budget = TokenBudget { used: 500, limit: 1000, rate_per_minute: 0.0 };
        assert_eq!(budget.fraction(), Some(0.5));
    }

    #[test]
    fn token_budget_fraction_unlimited() {
        let budget = TokenBudget { used: 500, limit: 0, rate_per_minute: 0.0 };
        assert_eq!(budget.fraction(), None);
    }

    #[test]
    fn token_budget_fraction_capped_at_one() {
        let budget = TokenBudget { used: 2000, limit: 1000, rate_per_minute: 0.0 };
        assert_eq!(budget.fraction(), Some(1.0));
    }

    #[test]
    fn token_budget_display_limited() {
        let budget = TokenBudget { used: 450, limit: 1000, rate_per_minute: 0.0 };
        assert_eq!(budget.display(), "45%");
    }

    #[test]
    fn token_budget_display_unlimited() {
        let budget = TokenBudget::default();
        assert_eq!(budget.display(), "∞");
    }

    #[test]
    fn agent_control_variants() {
        assert_eq!(AgentControl::Running, AgentControl::Running);
        assert_ne!(AgentControl::Paused, AgentControl::Running);
        assert_ne!(AgentControl::StepMode, AgentControl::Paused);
        assert_ne!(AgentControl::WaitingApproval, AgentControl::Running);
    }

    #[test]
    fn agent_state_defaults_to_idle() {
        let state = AppState::new();
        assert_eq!(state.agent_state, super::super::events::AgentState::Idle);
    }

    #[test]
    fn agent_state_can_be_updated() {
        let mut state = AppState::new();
        state.agent_state = super::super::events::AgentState::Executing;
        assert_eq!(state.agent_state, super::super::events::AgentState::Executing);
    }
}
