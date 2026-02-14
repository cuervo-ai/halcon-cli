//! 3-zone layout calculation for the TUI.
//!
//! Divides the terminal into three zones:
//! - **Prompt** (top): multiline text editor, min 3 lines, max 30% of height
//! - **Activity** (middle): scrollable agent output, takes remaining space
//! - **Status** (bottom): fixed 3 lines for stats + mini log

use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Layout zones for the 3-zone TUI.
#[derive(Debug, Clone, Copy)]
pub struct Zones {
    pub prompt: Rect,
    pub activity: Rect,
    pub status: Rect,
}

/// Minimum terminal dimensions for TUI mode.
#[allow(dead_code)]
pub const MIN_WIDTH: u16 = 80;
#[allow(dead_code)]
pub const MIN_HEIGHT: u16 = 24;

/// Fixed height of the status zone.
const STATUS_HEIGHT: u16 = 3;
/// Minimum height of the prompt zone.
const PROMPT_MIN_HEIGHT: u16 = 3;
/// Maximum fraction of terminal height for the prompt zone.
const PROMPT_MAX_FRACTION: f32 = 0.30;

/// Calculate the 3-zone layout from the available terminal area.
pub fn calculate_zones(area: Rect) -> Zones {
    let prompt_max = ((area.height as f32) * PROMPT_MAX_FRACTION) as u16;
    let prompt_height = prompt_max.max(PROMPT_MIN_HEIGHT).min(area.height.saturating_sub(STATUS_HEIGHT + 4));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(prompt_height),
            Constraint::Min(1),
            Constraint::Length(STATUS_HEIGHT),
        ])
        .split(area);

    Zones {
        prompt: chunks[0],
        activity: chunks[1],
        status: chunks[2],
    }
}

/// Check if terminal meets minimum dimensions.
#[allow(dead_code)]
pub fn is_terminal_large_enough(width: u16, height: u16) -> bool {
    width >= MIN_WIDTH && height >= MIN_HEIGHT
}

// ============================================================================
// Cockpit Layout (Phase 42C)
// ============================================================================

/// Minimum terminal width to show the side panel.
const PANEL_MIN_WIDTH: u16 = 100;
/// Default side panel width percentage.
const PANEL_WIDTH_PCT: u16 = 20;

/// Cockpit layout with optional side panel.
#[derive(Debug, Clone, Copy)]
pub struct CockpitZones {
    pub status: Rect,
    pub side_panel: Option<Rect>,
    pub activity: Rect,
    pub prompt: Rect,
}

/// Configuration for cockpit layout behavior.
#[derive(Debug, Clone)]
pub struct LayoutConfig {
    pub show_side_panel: bool,
    pub side_panel_width_pct: u16,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            show_side_panel: true,
            side_panel_width_pct: PANEL_WIDTH_PCT,
        }
    }
}

/// Calculate the cockpit layout: status bar at top, optional side panel, activity, prompt at bottom.
pub fn calculate_cockpit_zones(area: Rect, config: &LayoutConfig) -> CockpitZones {
    // Status bar at top (1 line + borders = 3)
    let status_height: u16 = 3;

    // Prompt at bottom
    let prompt_max = ((area.height as f32) * PROMPT_MAX_FRACTION) as u16;
    let prompt_height = prompt_max
        .max(PROMPT_MIN_HEIGHT)
        .min(area.height.saturating_sub(status_height + 4));

    // Vertical split: status | middle | prompt
    let v_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(status_height),
            Constraint::Min(1),
            Constraint::Length(prompt_height),
        ])
        .split(area);

    let status = v_chunks[0];
    let middle = v_chunks[1];
    let prompt = v_chunks[2];

    // Side panel: only when enabled AND terminal wide enough
    let show_panel = config.show_side_panel && area.width >= PANEL_MIN_WIDTH;

    let (side_panel, activity) = if show_panel {
        let panel_width = (area.width as u32 * config.side_panel_width_pct as u32 / 100) as u16;
        let panel_width = panel_width.max(20).min(area.width / 3);

        let h_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(panel_width),
                Constraint::Min(1),
            ])
            .split(middle);

        (Some(h_chunks[0]), h_chunks[1])
    } else {
        (None, middle)
    };

    CockpitZones {
        status,
        side_panel,
        activity,
        prompt,
    }
}

// ============================================================================
// Mode-aware Layout Engine (Phase 44A)
// ============================================================================

use super::state::UiMode;

/// Minimum terminal width for expert inspector panel.
const INSPECTOR_MIN_WIDTH: u16 = 140;
/// Inspector panel width percentage.
const INSPECTOR_WIDTH_PCT: u16 = 25;
/// Footer height (1 line for keybinding hints).
const FOOTER_HEIGHT: u16 = 1;

/// Result of mode-aware layout calculation.
#[derive(Debug, Clone, Copy)]
pub struct ModeLayout {
    pub status: Rect,
    pub side_panel: Option<Rect>,
    pub activity: Rect,
    pub inspector: Option<Rect>,
    pub prompt: Rect,
    pub footer: Rect,
}

/// Calculate layout zones based on the current UI mode.
///
/// - **Minimal**: status + activity + prompt + footer (no panels)
/// - **Standard**: status + [side_panel | activity] + prompt + footer
/// - **Expert**: status + [side_panel | activity | inspector] + prompt + footer
pub fn calculate_mode_layout(area: Rect, mode: UiMode, panel_visible: bool) -> ModeLayout {
    let status_height: u16 = 3;
    let prompt_max = ((area.height as f32) * PROMPT_MAX_FRACTION) as u16;
    let prompt_height = prompt_max
        .max(PROMPT_MIN_HEIGHT)
        .min(area.height.saturating_sub(status_height + FOOTER_HEIGHT + 4));

    // Vertical split: status | middle | prompt | footer
    let v_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(status_height),
            Constraint::Min(1),
            Constraint::Length(prompt_height),
            Constraint::Length(FOOTER_HEIGHT),
        ])
        .split(area);

    let status = v_chunks[0];
    let middle = v_chunks[1];
    let prompt = v_chunks[2];
    let footer = v_chunks[3];

    match mode {
        UiMode::Minimal => ModeLayout {
            status,
            side_panel: None,
            activity: middle,
            inspector: None,
            prompt,
            footer,
        },
        UiMode::Standard => {
            let show_panel = panel_visible && area.width >= PANEL_MIN_WIDTH;
            let (side_panel, activity) = if show_panel {
                let pw = panel_width(area.width, PANEL_WIDTH_PCT);
                let h = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Length(pw), Constraint::Min(1)])
                    .split(middle);
                (Some(h[0]), h[1])
            } else {
                (None, middle)
            };
            ModeLayout {
                status,
                side_panel,
                activity,
                inspector: None,
                prompt,
                footer,
            }
        }
        UiMode::Expert => {
            let show_panel = panel_visible && area.width >= PANEL_MIN_WIDTH;
            let show_inspector = area.width >= INSPECTOR_MIN_WIDTH;

            let (side_panel, rest) = if show_panel {
                let pw = panel_width(area.width, PANEL_WIDTH_PCT);
                let h = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Length(pw), Constraint::Min(1)])
                    .split(middle);
                (Some(h[0]), h[1])
            } else {
                (None, middle)
            };

            let (activity, inspector) = if show_inspector {
                let iw = panel_width(rest.width, INSPECTOR_WIDTH_PCT);
                let h = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Min(1), Constraint::Length(iw)])
                    .split(rest);
                (h[0], Some(h[1]))
            } else {
                (rest, None)
            };

            ModeLayout {
                status,
                side_panel,
                activity,
                inspector,
                prompt,
                footer,
            }
        }
    }
}

/// Calculate panel width from terminal width and percentage, clamped.
fn panel_width(terminal_width: u16, pct: u16) -> u16 {
    let pw = (terminal_width as u32 * pct as u32 / 100) as u16;
    pw.max(20).min(terminal_width / 3)
}

/// Minimum dimensions for degraded operation.
pub const COMPACT_MIN_WIDTH: u16 = 40;
pub const COMPACT_MIN_HEIGHT: u16 = 10;

/// Check if terminal is too small for any meaningful TUI display.
pub fn is_too_small(width: u16, height: u16) -> bool {
    width < COMPACT_MIN_WIDTH || height < COMPACT_MIN_HEIGHT
}

/// Effective UI mode after considering terminal size constraints.
///
/// Progressive degradation:
/// - Expert → Standard if too narrow for inspector (< 140 cols)
/// - Standard → Minimal if too narrow for side panel (< 100 cols)
pub fn effective_mode(width: u16, mode: UiMode) -> UiMode {
    match mode {
        UiMode::Expert if width < PANEL_MIN_WIDTH => UiMode::Minimal,
        UiMode::Expert if width < INSPECTOR_MIN_WIDTH => UiMode::Standard,
        UiMode::Standard if width < PANEL_MIN_WIDTH => UiMode::Minimal,
        _ => mode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zones_standard_terminal() {
        let area = Rect::new(0, 0, 120, 40);
        let zones = calculate_zones(area);
        assert!(zones.prompt.height >= PROMPT_MIN_HEIGHT);
        assert_eq!(zones.status.height, STATUS_HEIGHT);
        assert!(zones.activity.height >= 1);
        assert_eq!(
            zones.prompt.height + zones.activity.height + zones.status.height,
            area.height
        );
    }

    #[test]
    fn zones_minimum_terminal() {
        let area = Rect::new(0, 0, MIN_WIDTH, MIN_HEIGHT);
        let zones = calculate_zones(area);
        assert!(zones.prompt.height >= PROMPT_MIN_HEIGHT);
        assert_eq!(zones.status.height, STATUS_HEIGHT);
        assert!(zones.activity.height >= 1);
    }

    #[test]
    fn zones_small_terminal() {
        let area = Rect::new(0, 0, 80, 10);
        let zones = calculate_zones(area);
        assert_eq!(zones.status.height, STATUS_HEIGHT);
        assert!(zones.prompt.height >= 3);
    }

    #[test]
    fn zones_tall_terminal() {
        let area = Rect::new(0, 0, 120, 100);
        let zones = calculate_zones(area);
        // Prompt should be at most 30% of 100 = 30 lines
        assert!(zones.prompt.height <= 30);
        assert_eq!(zones.status.height, STATUS_HEIGHT);
    }

    #[test]
    fn prompt_never_exceeds_30_percent() {
        for h in 24..=200 {
            let area = Rect::new(0, 0, 80, h);
            let zones = calculate_zones(area);
            let max_allowed = ((h as f32) * PROMPT_MAX_FRACTION) as u16 + 1;
            assert!(
                zones.prompt.height <= max_allowed,
                "height={h}: prompt={} > max={}",
                zones.prompt.height,
                max_allowed
            );
        }
    }

    #[test]
    fn terminal_size_check() {
        assert!(is_terminal_large_enough(80, 24));
        assert!(is_terminal_large_enough(120, 40));
        assert!(!is_terminal_large_enough(79, 24));
        assert!(!is_terminal_large_enough(80, 23));
        assert!(!is_terminal_large_enough(40, 10));
    }

    #[test]
    fn zones_sum_to_total_height() {
        for h in 10..=100 {
            let area = Rect::new(0, 0, 80, h);
            let zones = calculate_zones(area);
            assert_eq!(
                zones.prompt.height + zones.activity.height + zones.status.height,
                h,
                "height={h}: prompt={} + activity={} + status={}",
                zones.prompt.height,
                zones.activity.height,
                zones.status.height
            );
        }
    }

    #[test]
    fn zones_positions_are_contiguous() {
        let area = Rect::new(0, 0, 100, 50);
        let zones = calculate_zones(area);
        assert_eq!(zones.prompt.y, 0);
        assert_eq!(zones.activity.y, zones.prompt.y + zones.prompt.height);
        assert_eq!(zones.status.y, zones.activity.y + zones.activity.height);
    }

    // --- Phase 42C: Cockpit layout tests ---

    #[test]
    fn cockpit_zones_wide_terminal() {
        let area = Rect::new(0, 0, 120, 40);
        let config = LayoutConfig::default();
        let zones = calculate_cockpit_zones(area, &config);
        assert!(zones.side_panel.is_some(), "panel visible at 120 cols");
    }

    #[test]
    fn cockpit_zones_narrow_terminal() {
        let area = Rect::new(0, 0, 80, 40);
        let config = LayoutConfig::default();
        let zones = calculate_cockpit_zones(area, &config);
        assert!(zones.side_panel.is_none(), "panel hidden at 80 cols");
    }

    #[test]
    fn cockpit_zones_sum_to_total() {
        let area = Rect::new(0, 0, 120, 40);
        let config = LayoutConfig::default();
        let zones = calculate_cockpit_zones(area, &config);
        // Vertical: status + middle + prompt = total height
        let total_h = zones.status.height + zones.activity.height + zones.prompt.height;
        // Side panel shares middle row, so just check with panel
        if let Some(panel) = zones.side_panel {
            assert_eq!(panel.height, zones.activity.height);
            assert_eq!(panel.width + zones.activity.width, area.width);
            assert_eq!(total_h, area.height);
        }
    }

    #[test]
    fn cockpit_panel_disabled() {
        let area = Rect::new(0, 0, 120, 40);
        let config = LayoutConfig {
            show_side_panel: false,
            side_panel_width_pct: 20,
        };
        let zones = calculate_cockpit_zones(area, &config);
        assert!(zones.side_panel.is_none(), "panel hidden when disabled");
    }

    #[test]
    fn cockpit_status_at_top() {
        let area = Rect::new(0, 0, 120, 40);
        let config = LayoutConfig::default();
        let zones = calculate_cockpit_zones(area, &config);
        assert_eq!(zones.status.y, 0, "status bar at top");
        assert_eq!(zones.status.height, 3);
    }

    // --- Phase 44A: Mode-aware layout tests ---

    #[test]
    fn mode_layout_minimal_has_no_panels() {
        let area = Rect::new(0, 0, 160, 50);
        let layout = calculate_mode_layout(area, UiMode::Minimal, true);
        assert!(layout.side_panel.is_none(), "minimal never shows side panel");
        assert!(layout.inspector.is_none(), "minimal never shows inspector");
    }

    #[test]
    fn mode_layout_standard_shows_side_panel() {
        let area = Rect::new(0, 0, 120, 40);
        let layout = calculate_mode_layout(area, UiMode::Standard, true);
        assert!(layout.side_panel.is_some(), "standard shows side panel at 120 cols");
        assert!(layout.inspector.is_none(), "standard never shows inspector");
    }

    #[test]
    fn mode_layout_standard_no_panel_when_narrow() {
        let area = Rect::new(0, 0, 80, 40);
        let layout = calculate_mode_layout(area, UiMode::Standard, true);
        assert!(layout.side_panel.is_none(), "panel hidden when < 100 cols");
    }

    #[test]
    fn mode_layout_standard_no_panel_when_hidden() {
        let area = Rect::new(0, 0, 120, 40);
        let layout = calculate_mode_layout(area, UiMode::Standard, false);
        assert!(layout.side_panel.is_none(), "panel hidden when panel_visible=false");
    }

    #[test]
    fn mode_layout_expert_shows_inspector_wide() {
        let area = Rect::new(0, 0, 160, 50);
        let layout = calculate_mode_layout(area, UiMode::Expert, true);
        assert!(layout.side_panel.is_some(), "expert shows side panel");
        assert!(layout.inspector.is_some(), "expert shows inspector at 160 cols");
    }

    #[test]
    fn mode_layout_expert_no_inspector_narrow() {
        let area = Rect::new(0, 0, 120, 40);
        let layout = calculate_mode_layout(area, UiMode::Expert, true);
        assert!(layout.side_panel.is_some(), "expert shows side panel at 120");
        assert!(layout.inspector.is_none(), "inspector hidden when < 140 cols");
    }

    #[test]
    fn mode_layout_footer_is_1_line() {
        let area = Rect::new(0, 0, 120, 40);
        let layout = calculate_mode_layout(area, UiMode::Standard, false);
        assert_eq!(layout.footer.height, FOOTER_HEIGHT, "footer is 1 line");
    }

    #[test]
    fn mode_layout_zones_sum_to_total_height() {
        for mode in [UiMode::Minimal, UiMode::Standard, UiMode::Expert] {
            let area = Rect::new(0, 0, 160, 50);
            let layout = calculate_mode_layout(area, mode, true);
            let total = layout.status.height
                + layout.activity.height
                + layout.prompt.height
                + layout.footer.height;
            assert_eq!(
                total, area.height,
                "mode={:?}: {} + {} + {} + {} = {} != {}",
                mode,
                layout.status.height,
                layout.activity.height,
                layout.prompt.height,
                layout.footer.height,
                total,
                area.height,
            );
        }
    }

    #[test]
    fn mode_layout_vertical_positions_contiguous() {
        let area = Rect::new(0, 0, 120, 40);
        let layout = calculate_mode_layout(area, UiMode::Standard, false);
        assert_eq!(layout.status.y, 0);
        assert_eq!(layout.activity.y, layout.status.y + layout.status.height);
        assert_eq!(layout.prompt.y, layout.activity.y + layout.activity.height);
        assert_eq!(layout.footer.y, layout.prompt.y + layout.prompt.height);
    }

    #[test]
    fn mode_layout_side_panel_shares_middle_row() {
        let area = Rect::new(0, 0, 120, 40);
        let layout = calculate_mode_layout(area, UiMode::Standard, true);
        if let Some(panel) = layout.side_panel {
            assert_eq!(panel.height, layout.activity.height, "panel and activity same height");
            assert_eq!(panel.width + layout.activity.width, area.width, "panel+activity = full width");
        }
    }

    // --- Phase F5: Small terminal tests ---

    #[test]
    fn is_too_small_below_minimum() {
        assert!(is_too_small(30, 5));
        assert!(is_too_small(39, 15));
        assert!(is_too_small(80, 9));
    }

    #[test]
    fn is_too_small_at_minimum() {
        assert!(!is_too_small(COMPACT_MIN_WIDTH, COMPACT_MIN_HEIGHT));
    }

    #[test]
    fn is_too_small_above_minimum() {
        assert!(!is_too_small(120, 40));
    }

    #[test]
    fn effective_mode_expert_very_narrow_downgrades_to_minimal() {
        assert_eq!(effective_mode(80, UiMode::Expert), UiMode::Minimal);
    }

    #[test]
    fn effective_mode_expert_medium_downgrades_to_standard() {
        // 100-139 cols: too narrow for inspector, but wide enough for panel → Standard
        assert_eq!(effective_mode(120, UiMode::Expert), UiMode::Standard);
    }

    #[test]
    fn effective_mode_standard_narrow_downgrades() {
        assert_eq!(effective_mode(80, UiMode::Standard), UiMode::Minimal);
    }

    #[test]
    fn effective_mode_expert_wide_unchanged() {
        assert_eq!(effective_mode(150, UiMode::Expert), UiMode::Expert);
    }

    #[test]
    fn effective_mode_minimal_always_unchanged() {
        assert_eq!(effective_mode(40, UiMode::Minimal), UiMode::Minimal);
        assert_eq!(effective_mode(200, UiMode::Minimal), UiMode::Minimal);
    }

    #[test]
    fn effective_mode_progressive_degradation_chain() {
        // Wide → Expert (unchanged)
        assert_eq!(effective_mode(160, UiMode::Expert), UiMode::Expert);
        // Medium → Standard (no inspector)
        assert_eq!(effective_mode(110, UiMode::Expert), UiMode::Standard);
        // Narrow → Minimal (no panels)
        assert_eq!(effective_mode(80, UiMode::Expert), UiMode::Minimal);
    }
}
