//! Bridge between the Momoto-backed theme system and ratatui styles.
//!
//! Provides semantic style constructors that map agent visual states and UI
//! elements to ratatui `Style` values using the active palette. This keeps
//! color logic centralized and prevents widgets from accessing the palette
//! directly with ad-hoc color choices.

use ratatui::style::{Color, Modifier, Style};

use crate::render::theme::{self, ThemeColor};

/// Convert a `ThemeColor` to a `ratatui::style::Color`.
pub fn color(tc: &ThemeColor) -> Color {
    tc.to_ratatui_color()
}

/// Agent visual state for semantic color mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentVisualState {
    Idle,
    Thinking,
    Planning,
    Executing,
    ToolSuccess,
    ToolError,
    ToolRetrying,
    Destructive,
    Cached,
    Paused,
    Reflecting,
    Delegated,
}

/// Return a foreground style for the given agent visual state.
pub fn style_for_state(state: AgentVisualState) -> Style {
    let p = &theme::active().palette;
    let fg = match state {
        AgentVisualState::Idle => color(&p.text_dim),
        AgentVisualState::Thinking => color(&p.cyan),
        AgentVisualState::Planning => color(&p.planning),
        AgentVisualState::Executing => color(&p.running),
        AgentVisualState::ToolSuccess => color(&p.success),
        AgentVisualState::ToolError => color(&p.error),
        AgentVisualState::ToolRetrying => color(&p.retrying),
        AgentVisualState::Destructive => color(&p.destructive),
        AgentVisualState::Cached => color(&p.cached),
        AgentVisualState::Paused => color(&p.warning),
        AgentVisualState::Reflecting => color(&p.reasoning),
        AgentVisualState::Delegated => color(&p.delegated),
    };
    Style::default().fg(fg)
}

/// Style for the status bar background.
pub fn status_bar_style() -> Style {
    let p = &theme::active().palette;
    Style::default()
        .fg(color(&p.text))
        .bg(color(&p.bg_panel))
}

/// Border style based on focus state.
pub fn border_style(focused: bool) -> Style {
    let p = &theme::active().palette;
    if focused {
        Style::default().fg(color(&p.primary))
    } else {
        Style::default().fg(color(&p.border))
    }
}

/// Style for panel section headers.
pub fn panel_header_style() -> Style {
    let p = &theme::active().palette;
    Style::default()
        .fg(color(&p.accent))
        .add_modifier(Modifier::BOLD)
}

/// Style for muted/secondary text.
pub fn muted_style() -> Style {
    let p = &theme::active().palette;
    Style::default().fg(color(&p.muted))
}

/// Style for labels in the panel.
pub fn label_style() -> Style {
    let p = &theme::active().palette;
    Style::default().fg(color(&p.text_label))
}

/// Style for success indicators.
pub fn success_style() -> Style {
    let p = &theme::active().palette;
    Style::default().fg(color(&p.success))
}

/// Style for error indicators.
pub fn error_style() -> Style {
    let p = &theme::active().palette;
    Style::default().fg(color(&p.error))
}

/// Style for warning indicators.
pub fn warning_style() -> Style {
    let p = &theme::active().palette;
    Style::default().fg(color(&p.warning))
}

/// Style for the primary brand color.
pub fn primary_style() -> Style {
    let p = &theme::active().palette;
    Style::default().fg(color(&p.primary))
}

/// Style for the spinner animation.
pub fn spinner_style() -> Style {
    let p = &theme::active().palette;
    Style::default().fg(color(&p.spinner_color))
}

/// Style for dry-run warning banner: warning text on highlighted bg.
pub fn dry_run_banner_style() -> Style {
    let p = &theme::active().palette;
    Style::default()
        .fg(color(&p.warning))
        .bg(color(&p.bg_highlight))
        .add_modifier(Modifier::BOLD)
}

/// Style for the footer keybinding hints.
pub fn footer_hint_style() -> Style {
    let p = &theme::active().palette;
    Style::default().fg(color(&p.text_dim))
}

/// Style for the footer keybinding key labels.
pub fn footer_key_style() -> Style {
    let p = &theme::active().palette;
    Style::default()
        .fg(color(&p.accent))
        .add_modifier(Modifier::BOLD)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_agent_states_produce_valid_styles() {
        // Ensure no panics when accessing palette for each state.
        let states = [
            AgentVisualState::Idle,
            AgentVisualState::Thinking,
            AgentVisualState::Planning,
            AgentVisualState::Executing,
            AgentVisualState::ToolSuccess,
            AgentVisualState::ToolError,
            AgentVisualState::ToolRetrying,
            AgentVisualState::Destructive,
            AgentVisualState::Cached,
            AgentVisualState::Paused,
            AgentVisualState::Reflecting,
            AgentVisualState::Delegated,
        ];
        for state in states {
            let style = style_for_state(state);
            assert!(style.fg.is_some(), "state {:?} should have fg color", state);
        }
    }

    #[test]
    fn status_bar_style_has_bg() {
        let style = status_bar_style();
        assert!(style.fg.is_some());
        assert!(style.bg.is_some());
    }

    #[test]
    fn border_focused_differs_from_unfocused() {
        let focused = border_style(true);
        let unfocused = border_style(false);
        assert_ne!(focused.fg, unfocused.fg);
    }

    #[test]
    fn dry_run_banner_has_bold() {
        let style = dry_run_banner_style();
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn color_conversion_roundtrip() {
        let tc = ThemeColor::rgb(100, 200, 50);
        let c = color(&tc);
        assert!(matches!(c, Color::Rgb(100, 200, 50)));
    }
}
