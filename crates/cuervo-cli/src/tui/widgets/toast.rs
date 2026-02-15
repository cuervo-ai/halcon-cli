//! Toast notification widget — transient messages with auto-dismiss.
//!
//! Toasts appear as a floating stack in the bottom-right of the terminal,
//! above the footer. Each toast auto-dismisses after a configurable duration.

use std::time::{Duration, Instant};

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

/// Severity level for visual differentiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastLevel {
    Info,
    Success,
    Warning,
    Error,
}

impl ToastLevel {
    fn icon(self) -> &'static str {
        match self {
            ToastLevel::Info => "ℹ",
            ToastLevel::Success => "✓",
            ToastLevel::Warning => "⚠",
            ToastLevel::Error => "✗",
        }
    }

    /// Get the ratatui Color for this toast level (theme-aware).
    ///
    /// Phase 45A: Migrated from hardcoded Color::Cyan/Green/Yellow/Red
    /// to palette semantic tokens for theme compliance.
    fn color(self) -> Color {
        let p = &crate::render::theme::active().palette;
        // Phase 45A Task 2.2: Use cached accessors
        match self {
            ToastLevel::Info => p.accent_ratatui(),
            ToastLevel::Success => p.success_ratatui(),
            ToastLevel::Warning => p.warning_ratatui(),
            ToastLevel::Error => p.error_ratatui(),
        }
    }

    /// Get the ThemeColor for perceptual fade operations.
    ///
    /// Phase 45A: Used for OKLCH-aware darkening during toast fade-out.
    fn theme_color(self) -> crate::render::theme::ThemeColor {
        let p = &crate::render::theme::active().palette;
        match self {
            ToastLevel::Info => p.accent,
            ToastLevel::Success => p.success,
            ToastLevel::Warning => p.warning,
            ToastLevel::Error => p.error,
        }
    }
}

/// A single toast notification entry.
#[derive(Debug, Clone)]
pub struct Toast {
    pub message: String,
    pub level: ToastLevel,
    pub created: Instant,
    pub ttl: Duration,
}

impl Toast {
    pub fn new(message: impl Into<String>, level: ToastLevel) -> Self {
        Self {
            message: message.into(),
            level,
            created: Instant::now(),
            ttl: Duration::from_secs(4),
        }
    }

    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Whether the toast has expired.
    pub fn expired(&self) -> bool {
        self.created.elapsed() >= self.ttl
    }

    /// Remaining fraction (1.0 = fresh, 0.0 = expired).
    pub fn remaining_fraction(&self) -> f64 {
        let elapsed = self.created.elapsed().as_secs_f64();
        let total = self.ttl.as_secs_f64();
        (1.0 - elapsed / total).max(0.0)
    }
}

/// Toast stack: manages multiple active toasts.
pub struct ToastStack {
    toasts: Vec<Toast>,
    max_visible: usize,
    /// Timestamp of the last toast push, for rate limiting.
    last_push: Option<Instant>,
    /// Number of toasts suppressed due to rate limiting.
    suppressed: usize,
}

const MAX_TOAST_VISIBLE: usize = 5;
/// Minimum interval between toasts (rate limit).
const MIN_TOAST_INTERVAL: Duration = Duration::from_millis(200);

impl ToastStack {
    pub fn new() -> Self {
        Self {
            toasts: Vec::new(),
            max_visible: MAX_TOAST_VISIBLE,
            last_push: None,
            suppressed: 0,
        }
    }

    /// Push a new toast onto the stack.
    ///
    /// Rate-limited: if toasts are pushed faster than MIN_TOAST_INTERVAL,
    /// excess toasts are suppressed. Error toasts always pass through.
    pub fn push(&mut self, toast: Toast) {
        // Error-level toasts always pass through rate limiting.
        if toast.level != ToastLevel::Error {
            if let Some(last) = self.last_push {
                if last.elapsed() < MIN_TOAST_INTERVAL {
                    self.suppressed += 1;
                    return;
                }
            }
        }
        self.last_push = Some(Instant::now());
        self.toasts.push(toast);
        // If we exceed max, drop the oldest.
        while self.toasts.len() > self.max_visible * 2 {
            self.toasts.remove(0);
        }
    }

    /// Number of toasts suppressed by rate limiting.
    pub fn suppressed_count(&self) -> usize {
        self.suppressed
    }

    /// Remove expired toasts. Returns true if any were removed.
    pub fn gc(&mut self) -> bool {
        let before = self.toasts.len();
        self.toasts.retain(|t| !t.expired());
        self.toasts.len() < before
    }

    /// How many active (non-expired) toasts are in the stack.
    pub fn active_count(&self) -> usize {
        self.toasts.iter().filter(|t| !t.expired()).count()
    }

    /// Manually dismiss all active toasts.
    pub fn dismiss_all(&mut self) {
        self.toasts.clear();
    }

    /// Whether there are any toasts to display.
    pub fn is_empty(&self) -> bool {
        self.active_count() == 0
    }

    /// Render the toast stack as a floating overlay.
    ///
    /// Skips rendering if terminal is too small (< 60 cols or < 12 rows).
    pub fn render(&self, frame: &mut Frame, terminal_area: Rect) {
        let active: Vec<&Toast> = self.toasts.iter().filter(|t| !t.expired()).collect();
        if active.is_empty() {
            return;
        }
        // Terminal size awareness: skip toasts when too small.
        if terminal_area.width < 60 || terminal_area.height < 12 {
            return;
        }

        let toast_width = 40u16.min(terminal_area.width.saturating_sub(4));
        let visible = &active[active.len().saturating_sub(self.max_visible)..];

        // Stack from bottom-right, above footer.
        let mut y_offset = terminal_area.height.saturating_sub(3); // 1 for footer, 2 margin

        for toast in visible.iter().rev() {
            if y_offset < 2 {
                break;
            }
            let area = Rect::new(
                terminal_area.width.saturating_sub(toast_width + 2),
                y_offset.saturating_sub(2),
                toast_width,
                3,
            );

            let icon = toast.level.icon();

            // Phase 45A: Perceptual fade using OKLCH darken instead of Modifier::DIM.
            // Last 700ms (30% of 4s default TTL): fade from full brightness to 30% darker.
            let frac = toast.remaining_fraction();
            let fade_threshold = 0.3;

            let fg_color = if frac > fade_threshold {
                // Full brightness
                toast.level.color()
            } else {
                // Fade progress: 0.0 (at 30%) → 1.0 (at 0%)
                let fade_progress = (fade_threshold - frac) / fade_threshold;
                let base_color = toast.level.theme_color();
                let faded_color = base_color.darken(fade_progress * 0.3);
                faded_color.to_ratatui_color()
            };

            let style = Style::default().fg(fg_color);

            let line = Line::from(vec![
                Span::styled(format!(" {icon} "), style.add_modifier(Modifier::BOLD)),
                Span::styled(&toast.message, style),
            ]);
            let paragraph = Paragraph::new(line)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(style),
                )
                .wrap(Wrap { trim: true });

            frame.render_widget(paragraph, area);
            y_offset = y_offset.saturating_sub(3);
        }
    }
}

impl Default for ToastStack {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_not_expired_initially() {
        let toast = Toast::new("test", ToastLevel::Info);
        assert!(!toast.expired());
    }

    #[test]
    fn toast_remaining_fraction_starts_near_one() {
        let toast = Toast::new("test", ToastLevel::Info);
        assert!(toast.remaining_fraction() > 0.99);
    }

    #[test]
    fn toast_with_zero_ttl_expired_immediately() {
        let toast = Toast::new("test", ToastLevel::Info).with_ttl(Duration::ZERO);
        assert!(toast.expired());
    }

    #[test]
    fn toast_stack_starts_empty() {
        let stack = ToastStack::new();
        assert!(stack.is_empty());
        assert_eq!(stack.active_count(), 0);
    }

    #[test]
    fn toast_stack_push_increments() {
        let mut stack = ToastStack::new();
        stack.push(Toast::new("msg", ToastLevel::Info));
        assert_eq!(stack.active_count(), 1);
        assert!(!stack.is_empty());
    }

    #[test]
    fn toast_stack_gc_removes_expired() {
        let mut stack = ToastStack::new();
        // Use Error level to bypass rate limiting.
        stack.push(Toast::new("expired", ToastLevel::Error).with_ttl(Duration::ZERO));
        stack.push(Toast::new("active", ToastLevel::Error));
        let removed = stack.gc();
        assert!(removed);
        assert_eq!(stack.active_count(), 1);
    }

    #[test]
    fn toast_level_icons() {
        assert_eq!(ToastLevel::Info.icon(), "ℹ");
        assert_eq!(ToastLevel::Success.icon(), "✓");
        assert_eq!(ToastLevel::Warning.icon(), "⚠");
        assert_eq!(ToastLevel::Error.icon(), "✗");
    }

    #[test]
    fn toast_level_colors_use_palette() {
        // Phase 45A: Validate colors come from palette, not hardcoded.
        let p = &crate::render::theme::active().palette;

        assert_eq!(ToastLevel::Info.color(), p.accent_ratatui());
        assert_eq!(ToastLevel::Success.color(), p.success_ratatui());
        assert_eq!(ToastLevel::Warning.color(), p.warning_ratatui());
        assert_eq!(ToastLevel::Error.color(), p.error_ratatui());
    }

    #[test]
    fn toast_stack_gc_no_change_when_all_active() {
        let mut stack = ToastStack::new();
        // Use Error level to bypass rate limiting in tests.
        stack.push(Toast::new("msg1", ToastLevel::Error));
        stack.push(Toast::new("msg2", ToastLevel::Error));
        let removed = stack.gc();
        assert!(!removed);
        assert_eq!(stack.active_count(), 2);
    }

    #[test]
    fn toast_stack_overflow_drops_oldest() {
        let mut stack = ToastStack::new();
        for i in 0..20 {
            // Use Error level to bypass rate limiting.
            stack.push(Toast::new(format!("msg {i}"), ToastLevel::Error));
        }
        // Should be capped at max_visible * 2 = 10.
        assert!(stack.toasts.len() <= MAX_TOAST_VISIBLE * 2);
    }

    // --- Sprint 3: Hardening tests ---

    #[test]
    fn toast_dismiss_all_clears_stack() {
        let mut stack = ToastStack::new();
        // Use Error level to bypass rate limiting.
        stack.push(Toast::new("msg", ToastLevel::Error));
        stack.push(Toast::new("msg2", ToastLevel::Error));
        assert_eq!(stack.active_count(), 2);
        stack.dismiss_all();
        assert_eq!(stack.active_count(), 0);
    }

    #[test]
    fn toast_rate_limit_suppresses_burst() {
        let mut stack = ToastStack::new();
        // Push multiple non-error toasts immediately — only the first should pass.
        stack.push(Toast::new("first", ToastLevel::Info));
        stack.push(Toast::new("second", ToastLevel::Info));
        stack.push(Toast::new("third", ToastLevel::Warning));
        // First goes through; the rest are rate-limited.
        assert_eq!(stack.active_count(), 1);
        assert!(stack.suppressed_count() >= 2);
    }

    #[test]
    fn toast_rate_limit_allows_errors_through() {
        let mut stack = ToastStack::new();
        stack.push(Toast::new("first", ToastLevel::Info));
        stack.push(Toast::new("error", ToastLevel::Error));
        // Error toasts bypass rate limiting.
        assert_eq!(stack.active_count(), 2);
    }

    // --- Phase 45A: Theme compliance tests ---

    #[test]
    fn toast_respects_neon_theme() {
        // Initialize neon theme
        crate::render::theme::init("neon", None);
        let p = &crate::render::theme::active().palette;

        // Validate Info uses accent (cyan in neon)
        let info_color = ToastLevel::Info.color();
        assert_eq!(info_color, p.accent_ratatui());
    }

    #[test]
    fn toast_respects_minimal_theme() {
        // Minimal theme has softer colors
        crate::render::theme::init("minimal", None);
        let p = &crate::render::theme::active().palette;

        // Validate Success uses success (softer green in minimal)
        let success_color = ToastLevel::Success.color();
        assert_eq!(success_color, p.success_ratatui());
    }

    #[test]
    fn toast_respects_plain_theme() {
        // Plain theme has neutral colors
        crate::render::theme::init("plain", None);
        let p = &crate::render::theme::active().palette;

        // In plain mode, all semantic colors are neutral
        let warning_color = ToastLevel::Warning.color();
        assert_eq!(warning_color, p.warning_ratatui());
    }

    #[test]
    fn toast_perceptual_fade_darkens() {
        // Validate that fade uses darken() not Modifier::DIM
        let base_color = ToastLevel::Success.theme_color();

        // Simulate fade at 15% remaining (85% through fade window)
        let fade_progress = 0.85;
        let faded = base_color.darken(fade_progress * 0.3);

        // Faded color should be darker (lower L in OKLCH)
        #[cfg(feature = "color-science")]
        {
            let base_l = base_color.to_oklch().l;
            let faded_l = faded.to_oklch().l;
            assert!(
                faded_l < base_l,
                "Faded L ({faded_l:.3}) should be < base L ({base_l:.3})"
            );
        }

        // RGB values should be darker
        let base_rgb = base_color.srgb8();
        let faded_rgb = faded.srgb8();
        assert!(
            faded_rgb[0] <= base_rgb[0] && faded_rgb[1] <= base_rgb[1] && faded_rgb[2] <= base_rgb[2],
            "Faded RGB should be <= base RGB"
        );
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn toast_wcag_compliance_against_bg_panel() {
        use crate::render::color_science::contrast_ratio;

        // Validate all toast levels meet WCAG AA (3.0:1) against bg_panel
        let p = &crate::render::theme::active().palette;
        let bg = &p.bg_panel;

        let levels = [
            ("Info", ToastLevel::Info.theme_color()),
            ("Success", ToastLevel::Success.theme_color()),
            ("Warning", ToastLevel::Warning.theme_color()),
            ("Error", ToastLevel::Error.theme_color()),
        ];

        for (name, fg) in levels {
            let ratio = contrast_ratio(&fg, bg);
            assert!(
                ratio >= 3.0,
                "Toast {name}: WCAG ratio {ratio:.2} should be >= 3.0"
            );
        }
    }
}
