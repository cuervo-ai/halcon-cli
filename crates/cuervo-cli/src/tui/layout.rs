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
}
