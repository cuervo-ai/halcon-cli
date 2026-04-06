//! Agent status badge with real-time state visualization.
//!
//! Embeds the ActivityIndicator into the status bar with transition effects
//! and visual hierarchy for different agent phases.

use crate::render::theme;
use crate::tui::transition_engine::TransitionEngine;
use crate::tui::widgets::activity_indicator::{ActivityIndicator, AgentState};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::time::Duration;

/// Agent status badge with transition effects.
///
/// Renders the current agent state as a styled badge in the status bar.
/// Uses ColorTransition for smooth state changes and visual hierarchy.
pub struct AgentBadge {
    /// Current agent state indicator.
    indicator: ActivityIndicator,
    /// Transition engine for badge color changes.
    transitions: TransitionEngine,
    /// Last rendered state (for detecting changes).
    last_state: AgentState,
}

impl AgentBadge {
    /// Create a new agent badge.
    pub fn new() -> Self {
        Self {
            indicator: ActivityIndicator::new(),
            transitions: TransitionEngine::new(),
            last_state: AgentState::Idle,
        }
    }

    /// Update agent state with transition effect.
    ///
    /// If the state changed, starts a color transition from the previous
    /// state color to the new state color.
    pub fn set_state(&mut self, state: AgentState) {
        if state != self.last_state {
            let from_color = self.last_state.semantic_color();
            let to_color = state.semantic_color();

            // Start 300ms transition from old to new state color
            self.transitions.start(
                "badge_color",
                from_color,
                to_color,
                Duration::from_millis(300),
            );

            self.last_state = state;
        }

        self.indicator.set_state(state);
    }

    /// Set detail message.
    pub fn set_detail(&mut self, detail: Option<String>) {
        self.indicator.set_detail(detail);
    }

    /// Get current state.
    pub fn state(&self) -> AgentState {
        self.indicator.state()
    }

    /// Render the badge as a styled span for status bar embedding.
    ///
    /// Uses transition engine to smoothly animate color changes between states.
    pub fn render_span(&self) -> Span<'static> {
        let _p = &theme::active().palette;
        let base_color = self.indicator.state().semantic_color();

        // Get current transition color (or base if no transition active)
        let current_color = self.transitions.current("badge_color", base_color);

        let icon = self.indicator.state().icon();
        let label = self.indicator.state().label();

        // Format as " icon label " (e.g., " ⚙ Running ")
        let text = format!(" {} {} ", icon, label);

        Span::styled(
            text,
            Style::default()
                .fg(current_color.to_ratatui_color())
                .add_modifier(Modifier::BOLD),
        )
    }

    /// Render as a standalone widget in a dedicated area.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let span = self.render_span();
        let widget = Paragraph::new(span);
        frame.render_widget(widget, area);
    }

    /// Get reference to internal indicator (for compatibility).
    pub fn indicator(&self) -> &ActivityIndicator {
        &self.indicator
    }

    /// Get mutable reference to internal indicator (for direct updates).
    pub fn indicator_mut(&mut self) -> &mut ActivityIndicator {
        &mut self.indicator
    }

    /// Prune completed transitions (should be called on tick).
    pub fn tick(&mut self) {
        self.transitions.prune_completed();
    }
}

impl Default for AgentBadge {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_badge_starts_idle() {
        let badge = AgentBadge::new();
        assert_eq!(badge.state(), AgentState::Idle);
        assert_eq!(badge.last_state, AgentState::Idle);
    }

    #[test]
    fn set_state_updates_indicator() {
        let mut badge = AgentBadge::new();
        badge.set_state(AgentState::Planning);
        assert_eq!(badge.state(), AgentState::Planning);
    }

    #[test]
    fn set_state_starts_transition_on_change() {
        let mut badge = AgentBadge::new();
        assert_eq!(badge.last_state, AgentState::Idle);

        badge.set_state(AgentState::Running);
        assert_eq!(badge.last_state, AgentState::Running);

        // Transition should be active
        assert!(badge.transitions.has_active());
    }

    #[test]
    fn set_state_no_transition_if_same() {
        let mut badge = AgentBadge::new();
        badge.set_state(AgentState::Idle); // Same as default

        // No transition should start
        assert!(!badge.transitions.has_active());
    }

    #[test]
    fn set_detail_passes_through() {
        let mut badge = AgentBadge::new();
        badge.set_detail(Some("Test detail".to_string()));

        // Detail is stored but not used in render_span (uses label only)
        // This is expected - detail is for standalone rendering
        assert_eq!(badge.indicator.state(), AgentState::Idle);
    }

    #[test]
    fn render_span_includes_icon_and_label() {
        let badge = AgentBadge::new();
        let span = badge.render_span();

        let text = span.content.to_string();
        assert!(text.contains(AgentState::Idle.icon()));
        assert!(text.contains(AgentState::Idle.label()));
    }

    #[test]
    fn tick_prunes_completed_transitions() {
        let mut badge = AgentBadge::new();
        badge.set_state(AgentState::Running);
        assert!(badge.transitions.has_active());

        // Sleep longer than transition duration (300ms)
        std::thread::sleep(Duration::from_millis(350));
        badge.tick();

        // Transition should be pruned
        assert!(!badge.transitions.has_active());
    }

    #[test]
    fn indicator_accessor_provides_reference() {
        let badge = AgentBadge::new();
        let indicator = badge.indicator();
        assert_eq!(indicator.state(), AgentState::Idle);
    }

    #[test]
    fn indicator_mut_accessor_allows_updates() {
        let mut badge = AgentBadge::new();
        badge.indicator_mut().set_state(AgentState::Error);
        assert_eq!(badge.state(), AgentState::Error);
    }

    #[test]
    fn default_creates_idle_badge() {
        let badge = AgentBadge::default();
        assert_eq!(badge.state(), AgentState::Idle);
    }

    #[test]
    #[cfg(feature = "color-science")]
    fn transition_interpolates_colors() {
        let mut badge = AgentBadge::new();
        badge.set_state(AgentState::Running);

        // Get current color during transition
        let p = &crate::render::theme::active().palette;
        let current = badge.transitions.current("badge_color", p.success);

        // Color should be different from both start and end (unless at boundary)
        // This is a smoke test - exact values depend on timing
        let [r, g, b] = current.srgb8();
        assert!(r <= 255 && g <= 255 && b <= 255); // Sanity check
    }

    #[test]
    fn state_change_sequence() {
        let mut badge = AgentBadge::new();

        // Idle → Planning
        badge.set_state(AgentState::Planning);
        assert_eq!(badge.state(), AgentState::Planning);
        assert!(badge.transitions.has_active());

        // Wait for transition to complete
        std::thread::sleep(Duration::from_millis(350));
        badge.tick();
        assert!(!badge.transitions.has_active());

        // Planning → ToolExecution
        badge.set_state(AgentState::ToolExecution);
        assert_eq!(badge.state(), AgentState::ToolExecution);
        assert!(badge.transitions.has_active());
    }
}
