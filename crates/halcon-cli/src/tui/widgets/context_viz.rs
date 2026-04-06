//! Context tier visualization widget with animated gauges.
//!
//! Displays L0-L4 memory tiers with:
//! - Live token/entry counts
//! - Animated percentage gauges with transition effects
//! - Color-coded capacity indicators (green→yellow→red)
//! - Highlight pulses on overflow warnings

use crate::render::theme;
use crate::tui::highlight::HighlightManager;
use crate::tui::transition_engine::TransitionEngine;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use std::time::Duration;

/// Context tier data for visualization.
#[derive(Debug, Clone, Default)]
pub struct TierData {
    pub l0_tokens: u32,
    pub l0_capacity: u32,
    pub l0_pct: u8,
    pub l1_tokens: u32,
    pub l1_entries: usize,
    pub l1_pct: u8,
    pub l2_entries: usize,
    pub l2_pct: u8,
    pub l3_entries: usize,
    pub l3_pct: u8,
    pub l4_entries: usize,
    pub l4_pct: u8,
    pub total_tokens: u32,
}

/// Context visualization widget with animated gauges.
pub struct ContextVisualization {
    /// Current tier data.
    data: TierData,
    /// Transition engine for smooth gauge animations.
    transitions: TransitionEngine,
    /// Highlight manager for overflow warnings.
    highlights: HighlightManager,
    /// Previous L0 percentage for transition detection.
    last_l0_pct: u8,
}

impl ContextVisualization {
    /// Create a new context visualization widget.
    pub fn new() -> Self {
        Self {
            data: TierData::default(),
            transitions: TransitionEngine::new(),
            highlights: HighlightManager::new(),
            last_l0_pct: 0,
        }
    }

    /// Update tier data with transition effects.
    ///
    /// Automatically starts gauge transitions for changed percentages.
    pub fn update(&mut self, data: TierData) {
        // Start L0 gauge transition if percentage changed
        if data.l0_pct != self.last_l0_pct {
            let _p = &theme::active().palette;
            let from_color = self.gauge_color(self.last_l0_pct);
            let to_color = self.gauge_color(data.l0_pct);

            self.transitions
                .start("l0_gauge", from_color, to_color, Duration::from_millis(500));
        }

        // Detect L0 overflow and start warning pulse
        if data.l0_pct > 90 && self.last_l0_pct <= 90 {
            let p = &theme::active().palette;
            self.highlights.start_medium("l0_overflow", p.warning);
        } else if data.l0_pct <= 90 && self.last_l0_pct > 90 {
            self.highlights.stop("l0_overflow");
        }

        self.last_l0_pct = data.l0_pct;
        self.data = data;
    }

    /// Get gauge color based on capacity percentage.
    ///
    /// - 0-70%: success (green)
    /// - 71-90%: accent (blue)
    /// - 91-100%: warning (yellow)
    fn gauge_color(&self, pct: u8) -> crate::render::theme::ThemeColor {
        let p = &theme::active().palette;
        if pct > 90 {
            p.warning
        } else if pct > 70 {
            p.accent
        } else {
            p.success
        }
    }

    /// Render a horizontal gauge bar.
    ///
    /// Returns a styled string like: "[████████░░] 80%"
    fn render_gauge(&self, pct: u8, width: usize) -> Vec<Span<'static>> {
        let _p = &theme::active().palette;
        let gauge_width = width.saturating_sub(2); // Account for [ ]
        let filled = (gauge_width * pct as usize / 100).min(gauge_width);
        let empty = gauge_width.saturating_sub(filled);

        let color = self.gauge_color(pct);
        let bar = format!("[{}{}]", "█".repeat(filled), "░".repeat(empty));

        vec![
            Span::styled(
                bar,
                Style::default()
                    .fg(color.to_ratatui_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" {}%", pct)),
        ]
    }

    /// Render the context visualization.
    ///
    /// Displays L0-L4 tiers with animated gauges and counts.
    pub fn render_lines(&self) -> Vec<Line<'static>> {
        let p = &theme::active().palette;
        let mut lines = Vec::new();

        // L0 Hot Buffer
        let l0_color = if self.highlights.is_pulsing("l0_overflow") {
            self.highlights.current("l0_overflow", p.warning)
        } else {
            self.transitions
                .current("l0_gauge", self.gauge_color(self.data.l0_pct))
        };

        lines.push(Line::from(vec![
            Span::styled(
                "L0 Hot:  ",
                Style::default()
                    .fg(l0_color.to_ratatui_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "{}tok / {}tok",
                self.data.l0_tokens, self.data.l0_capacity
            )),
        ]));
        lines.push(Line::from(self.render_gauge(self.data.l0_pct, 20)));

        // L1 Sliding Window
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                "L1 Warm: ",
                Style::default()
                    .fg(self.gauge_color(self.data.l1_pct).to_ratatui_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "{}tok / {} seg",
                self.data.l1_tokens, self.data.l1_entries
            )),
        ]));
        lines.push(Line::from(self.render_gauge(self.data.l1_pct, 20)));

        // L2 Cold Store
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                "L2 Cold: ",
                Style::default().fg(self.gauge_color(self.data.l2_pct).to_ratatui_color()),
            ),
            Span::raw(format!("{} entries", self.data.l2_entries)),
        ]));
        lines.push(Line::from(self.render_gauge(self.data.l2_pct, 20)));

        // L3 Semantic
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                "L3 Sem:  ",
                Style::default().fg(self.gauge_color(self.data.l3_pct).to_ratatui_color()),
            ),
            Span::raw(format!("{} entries", self.data.l3_entries)),
        ]));
        lines.push(Line::from(self.render_gauge(self.data.l3_pct, 20)));

        // L4 Archive
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                "L4 Arch: ",
                Style::default().fg(self.gauge_color(self.data.l4_pct).to_ratatui_color()),
            ),
            Span::raw(format!("{} entries", self.data.l4_entries)),
        ]));
        lines.push(Line::from(self.render_gauge(self.data.l4_pct, 20)));

        // Total
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("Total:   ", Style::default().fg(p.text_label_ratatui())),
            Span::styled(
                format!("{}tok", self.data.total_tokens),
                Style::default()
                    .fg(p.accent_ratatui())
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        lines
    }

    /// Tick to prune completed transitions and update highlights.
    pub fn tick(&mut self) {
        self.transitions.prune_completed();
    }

    /// Get current tier data.
    pub fn data(&self) -> &TierData {
        &self.data
    }
}

impl Default for ContextVisualization {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_viz_empty() {
        let viz = ContextVisualization::new();
        assert_eq!(viz.data.l0_tokens, 0);
        assert_eq!(viz.data.total_tokens, 0);
    }

    #[test]
    fn update_sets_data() {
        let mut viz = ContextVisualization::new();
        let data = TierData {
            l0_tokens: 500,
            l0_capacity: 1000,
            l0_pct: 50,
            l1_tokens: 200,
            l1_entries: 5,
            l1_pct: 20,
            l2_entries: 3,
            l2_pct: 10,
            l3_entries: 2,
            l3_pct: 5,
            l4_entries: 1,
            l4_pct: 5,
            total_tokens: 1000,
        };

        viz.update(data.clone());
        assert_eq!(viz.data.l0_tokens, 500);
        assert_eq!(viz.data.l0_pct, 50);
    }

    #[test]
    fn gauge_color_green_below_70() {
        let viz = ContextVisualization::new();
        let color = viz.gauge_color(50);
        let p = &theme::active().palette;
        assert_eq!(color.srgb8(), p.success.srgb8());
    }

    #[test]
    fn gauge_color_blue_71_to_90() {
        let viz = ContextVisualization::new();
        let color = viz.gauge_color(80);
        let p = &theme::active().palette;
        assert_eq!(color.srgb8(), p.accent.srgb8());
    }

    #[test]
    fn gauge_color_yellow_above_90() {
        let viz = ContextVisualization::new();
        let color = viz.gauge_color(95);
        let p = &theme::active().palette;
        assert_eq!(color.srgb8(), p.warning.srgb8());
    }

    #[test]
    fn l0_overflow_starts_highlight() {
        let mut viz = ContextVisualization::new();
        let data = TierData {
            l0_pct: 95,
            ..Default::default()
        };

        viz.update(data);
        assert!(viz.highlights.is_pulsing("l0_overflow"));
    }

    #[test]
    fn l0_recovery_stops_highlight() {
        let mut viz = ContextVisualization::new();

        // First, trigger overflow
        let overflow_data = TierData {
            l0_pct: 95,
            ..Default::default()
        };
        viz.update(overflow_data);
        assert!(viz.highlights.is_pulsing("l0_overflow"));

        // Then, recover
        let recovery_data = TierData {
            l0_pct: 85,
            ..Default::default()
        };
        viz.update(recovery_data);
        assert!(!viz.highlights.is_pulsing("l0_overflow"));
    }

    #[test]
    fn l0_pct_change_starts_transition() {
        let mut viz = ContextVisualization::new();

        let data1 = TierData {
            l0_pct: 50,
            ..Default::default()
        };
        viz.update(data1);

        let data2 = TierData {
            l0_pct: 80,
            ..Default::default()
        };
        viz.update(data2);

        assert!(viz.transitions.has_active());
    }

    #[test]
    fn render_gauge_format() {
        let viz = ContextVisualization::new();
        let spans = viz.render_gauge(50, 20);

        // Should have 2 spans: bar + percentage
        assert_eq!(spans.len(), 2);

        // Second span should be percentage
        assert!(spans[1].content.contains("50%"));
    }

    #[test]
    fn render_gauge_filled_bars() {
        let viz = ContextVisualization::new();
        let spans = viz.render_gauge(100, 12);

        // Bar should be fully filled
        let bar = &spans[0].content;
        assert!(bar.contains("█"));
        assert!(!bar.contains("░")); // No empty bars at 100%
    }

    #[test]
    fn render_gauge_empty_bars() {
        let viz = ContextVisualization::new();
        let spans = viz.render_gauge(0, 12);

        // Bar should be fully empty
        let bar = &spans[0].content;
        assert!(!bar.contains("█")); // No filled bars at 0%
        assert!(bar.contains("░"));
    }

    #[test]
    fn render_lines_includes_all_tiers() {
        let mut viz = ContextVisualization::new();
        let data = TierData {
            l0_tokens: 500,
            l0_capacity: 1000,
            l0_pct: 50,
            l1_tokens: 200,
            l1_entries: 5,
            l1_pct: 20,
            l2_entries: 3,
            l2_pct: 10,
            l3_entries: 2,
            l3_pct: 5,
            l4_entries: 1,
            l4_pct: 5,
            total_tokens: 1000,
        };
        viz.update(data);

        let lines = viz.render_lines();

        // Should have lines for all tiers + gauges + spacing + total
        // L0 (2) + blank + L1 (2) + blank + L2 (2) + blank + L3 (2) + blank + L4 (2) + blank + Total = 15
        assert!(lines.len() >= 15);
    }

    #[test]
    fn tick_prunes_transitions() {
        let mut viz = ContextVisualization::new();

        let data1 = TierData {
            l0_pct: 50,
            ..Default::default()
        };
        viz.update(data1);

        let data2 = TierData {
            l0_pct: 80,
            ..Default::default()
        };
        viz.update(data2);

        assert!(viz.transitions.has_active());

        // Sleep longer than transition duration (500ms)
        std::thread::sleep(Duration::from_millis(550));
        viz.tick();

        assert!(!viz.transitions.has_active());
    }

    #[test]
    fn data_accessor() {
        let mut viz = ContextVisualization::new();
        let data = TierData {
            l0_tokens: 123,
            ..Default::default()
        };
        viz.update(data);

        assert_eq!(viz.data().l0_tokens, 123);
    }

    #[test]
    fn default_creates_empty_viz() {
        let viz = ContextVisualization::default();
        assert_eq!(viz.data.l0_tokens, 0);
    }
}
