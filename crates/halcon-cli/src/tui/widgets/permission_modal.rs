//! High-contrast permission approval modal using momoto semantic colors.
//!
//! Provides clear visual hierarchy for permission decisions with:
//! - Risk-based color coding (OKLCH perceptual colors)
//! - Argument preview
//! - Impact explanation
//! - Clear approve/reject actions

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::render::theme;
use crate::tui::overlay::centered_rect;
use crate::tui::permission_context::{PermissionContext, RiskLevel};

/// Permission approval modal with risk-based visual hierarchy.
///
/// Renders as a centered floating box with:
/// - Risk icon + level header (color-coded by momoto)
/// - Tool name (bold, accent color)
/// - Risk description
/// - Arguments preview (first 3 keys)
/// - Approve/Reject actions (color-coded)
pub struct PermissionModal {
    /// Permission request context.
    context: PermissionContext,
}

impl PermissionModal {
    /// Create a new permission modal.
    pub fn new(context: PermissionContext) -> Self {
        Self { context }
    }

    /// Render the permission modal.
    ///
    /// Modal is centered at 60% width, 55% height with momoto-backed colors.
    ///
    /// # Arguments
    /// * `show_advanced` - Whether to show advanced permission options (AlwaysThisTool, ThisDirectory, etc.)
    /// * `remaining_secs` - Countdown remaining (None = no deadline set).
    /// * `total_secs` - Total countdown duration for progress bar fraction.
    pub fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        show_advanced: bool,
        remaining_secs: Option<u64>,
        total_secs: u64,
    ) {
        let p = &theme::active().palette;
        let risk_color = self.context.risk_level.color(p);

        // Centered modal (60% width, 55% height — extra row for countdown bar)
        let rect = centered_rect(area, 60, 55);
        frame.render_widget(Clear, rect);

        // Build content with momoto colors
        let mut lines = Vec::new();

        // Header: Risk icon + level (bold, large)
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} ", self.context.risk_level.icon()),
                Style::default(),
            ),
            Span::styled(
                format!(
                    "Permission Required: {} Risk",
                    self.context.risk_level.label().to_uppercase()
                ),
                Style::default()
                    .fg(risk_color.to_ratatui_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(""));

        // Tool name (prominent, accent color)
        lines.push(Line::from(vec![
            Span::styled("Tool: ", Style::default().fg(p.text_dim_ratatui())),
            Span::styled(
                &self.context.tool,
                Style::default()
                    .fg(p.accent_ratatui())
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(""));

        // Risk description (slightly lighter risk color for readability)
        #[cfg(feature = "color-science")]
        let desc_color = risk_color.lighten(0.1).to_ratatui_color();
        #[cfg(not(feature = "color-science"))]
        let desc_color = risk_color.to_ratatui_color();

        lines.push(Line::from(Span::styled(
            self.context.risk_level.description(),
            Style::default().fg(desc_color),
        )));
        lines.push(Line::from(""));

        // Arguments preview (first 3 keys)
        lines.push(Line::from(Span::styled(
            "Arguments:",
            Style::default().fg(p.text_dim_ratatui()),
        )));

        let args_summary = self.context.args_summary(3);
        if args_summary.is_empty() {
            lines.push(Line::from(Span::styled(
                "  (no arguments)",
                Style::default().fg(p.muted_ratatui()),
            )));
        } else {
            for (key, value) in args_summary {
                lines.push(Line::from(vec![
                    Span::styled("  • ", Style::default().fg(p.muted_ratatui())),
                    Span::styled(format!("{}: ", key), Style::default().fg(p.text_ratatui())),
                    Span::styled(value, Style::default().fg(p.text_dim_ratatui())),
                ]));
            }
        }
        lines.push(Line::from(""));

        // Actions (8-option grid layout)
        lines.push(Line::from(Span::styled(
            "Options:",
            Style::default()
                .fg(p.text_dim_ratatui())
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));

        let all_options = self.context.risk_level.available_options();
        let recommended = self.context.risk_level.recommended_option();

        // Phase 6: Progressive disclosure - filter advanced options if not shown
        let options: Vec<_> = if show_advanced {
            all_options
        } else {
            all_options
                .into_iter()
                .filter(|opt| !opt.is_advanced())
                .collect()
        };

        // Render in 2-column grid
        for chunk in options.chunks(2) {
            let mut spans = Vec::new();

            for (i, option) in chunk.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::raw("    ")); // Column separator
                }

                // Color-code by option type
                let color = match option {
                    crate::tui::permission_context::PermissionOption::Yes
                    | crate::tui::permission_context::PermissionOption::AlwaysThisTool
                    | crate::tui::permission_context::PermissionOption::ThisDirectory
                    | crate::tui::permission_context::PermissionOption::ThisSession
                    | crate::tui::permission_context::PermissionOption::ThisPattern => {
                        if self.context.risk_level == RiskLevel::Critical {
                            p.warning_ratatui() // Yellow for critical approvals
                        } else {
                            p.success_ratatui() // Green for approvals
                        }
                    }
                    crate::tui::permission_context::PermissionOption::No
                    | crate::tui::permission_context::PermissionOption::NeverThisDirectory => {
                        p.error_ratatui() // Red for denials
                    }
                    crate::tui::permission_context::PermissionOption::Cancel => {
                        p.muted_ratatui() // Gray for cancel
                    }
                };

                // Phase 6: Highlight recommended option with star marker
                let is_recommended = *option == recommended;
                let label = if is_recommended {
                    format!("★ [{}] {}", option.key(), option.label())
                } else {
                    format!("[{}] {}", option.key(), option.label())
                };

                let mut style = Style::default().fg(color).add_modifier(Modifier::BOLD);
                if is_recommended {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }

                spans.push(Span::styled(label, style));
            }

            lines.push(Line::from(spans));
        }

        // Phase 6: Help text for progressive disclosure
        if !show_advanced {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Press [F1] to show advanced options",
                Style::default()
                    .fg(p.muted_ratatui())
                    .add_modifier(Modifier::ITALIC),
            )));
        }

        // Hint for high/critical risk
        if matches!(
            self.context.risk_level,
            RiskLevel::High | RiskLevel::Critical
        ) {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "⚠ This operation cannot be easily undone. Review carefully.",
                Style::default()
                    .fg(p.warning_ratatui())
                    .add_modifier(Modifier::ITALIC),
            )));
        }

        // Static pause indicator — agent is blocked waiting for user decision.
        // No countdown: user has unlimited time to review and decide.
        let _ = (remaining_secs, total_secs); // fields retained in struct for API compatibility
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("⏸  ", Style::default().fg(p.warning_ratatui())),
            Span::styled(
                "Agent paused — approve or deny to continue",
                Style::default()
                    .fg(p.warning_ratatui())
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        let modal = Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(
                        " {} Permission Request ",
                        self.context.risk_level.icon()
                    ))
                    .border_style(
                        Style::default()
                            .fg(risk_color.to_ratatui_color())
                            .add_modifier(Modifier::BOLD),
                    ),
            )
            .wrap(Wrap { trim: false })
            .alignment(Alignment::Left);

        frame.render_widget(modal, rect);
    }

    /// Get the permission context.
    pub fn context(&self) -> &PermissionContext {
        &self.context
    }

    /// Get the tool name.
    pub fn tool_name(&self) -> &str {
        &self.context.tool
    }

    /// Get the risk level.
    pub fn risk_level(&self) -> RiskLevel {
        self.context.risk_level
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_modal_stores_context() {
        let ctx = PermissionContext::new(
            "bash".to_string(),
            serde_json::json!({"command": "ls"}),
            RiskLevel::Low,
        );
        let modal = PermissionModal::new(ctx.clone());
        assert_eq!(modal.tool_name(), "bash");
        assert_eq!(modal.risk_level(), RiskLevel::Low);
    }

    #[test]
    fn modal_context_accessor() {
        let ctx = PermissionContext::new(
            "file_write".to_string(),
            serde_json::json!({"path": "/tmp/test.txt"}),
            RiskLevel::Medium,
        );
        let modal = PermissionModal::new(ctx.clone());
        assert_eq!(modal.context().tool, "file_write");
    }

    #[test]
    fn modal_tool_name_accessor() {
        let ctx = PermissionContext::new(
            "git_commit".to_string(),
            serde_json::json!({}),
            RiskLevel::High,
        );
        let modal = PermissionModal::new(ctx);
        assert_eq!(modal.tool_name(), "git_commit");
    }

    #[test]
    fn modal_risk_level_accessor() {
        let ctx = PermissionContext::new(
            "bash".to_string(),
            serde_json::json!({}),
            RiskLevel::Critical,
        );
        let modal = PermissionModal::new(ctx);
        assert_eq!(modal.risk_level(), RiskLevel::Critical);
    }

    #[test]
    #[cfg(feature = "color-science")]
    fn modal_uses_risk_color_from_palette() {
        use crate::render::theme;

        theme::init("neon", None);
        let p = &theme::active().palette;

        let ctx_low =
            PermissionContext::new("test".to_string(), serde_json::json!({}), RiskLevel::Low);
        assert_eq!(ctx_low.risk_level.color(p).srgb8(), p.success.srgb8());

        let ctx_critical = PermissionContext::new(
            "test".to_string(),
            serde_json::json!({}),
            RiskLevel::Critical,
        );
        assert_eq!(
            ctx_critical.risk_level.color(p).srgb8(),
            p.destructive.srgb8()
        );
    }

    #[test]
    fn modal_with_empty_args() {
        let ctx = PermissionContext::new("tool".to_string(), serde_json::json!({}), RiskLevel::Low);
        let modal = PermissionModal::new(ctx);
        assert_eq!(modal.context().args_summary(3).len(), 0);
    }

    #[test]
    fn modal_with_multiple_args() {
        let ctx = PermissionContext::new(
            "tool".to_string(),
            serde_json::json!({
                "arg1": "value1",
                "arg2": "value2",
                "arg3": "value3",
            }),
            RiskLevel::Medium,
        );
        let modal = PermissionModal::new(ctx);
        assert_eq!(modal.context().args_summary(3).len(), 3);
    }
}
