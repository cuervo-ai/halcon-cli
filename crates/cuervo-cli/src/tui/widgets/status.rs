//! Status bar widget for the TUI bottom zone.
//!
//! Displays session info: ID, provider/model, round, token breakdown,
//! cost, elapsed time, and tool invocation count.

use std::time::Instant;

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

/// State for the status bar zone.
pub struct StatusState {
    session_id: String,
    provider: String,
    model: String,
    round: usize,
    input_tokens: u32,
    output_tokens: u32,
    cost: f64,
    elapsed_ms: u64,
    tool_count: u32,
    /// Wall-clock start time for live elapsed display.
    start_time: Instant,
    /// Optional plan step indicator (e.g. "Step 2/5: Edit auth").
    pub plan_step: Option<String>,
}

impl StatusState {
    pub fn new() -> Self {
        Self {
            session_id: String::new(),
            provider: String::new(),
            model: String::new(),
            round: 0,
            input_tokens: 0,
            output_tokens: 0,
            cost: 0.0,
            elapsed_ms: 0,
            tool_count: 0,
            start_time: Instant::now(),
            plan_step: None,
        }
    }

    /// Update the status bar fields. Only overwrites fields that are `Some`.
    pub fn update(
        &mut self,
        provider: Option<String>,
        model: Option<String>,
        round: Option<usize>,
        _tokens: Option<u64>,
        cost: Option<f64>,
        session_id: Option<String>,
        elapsed_ms: Option<u64>,
        tool_count: Option<u32>,
        input_tokens: Option<u32>,
        output_tokens: Option<u32>,
    ) {
        if let Some(p) = provider {
            self.provider = p;
        }
        if let Some(m) = model {
            self.model = m;
        }
        if let Some(r) = round {
            self.round = r;
        }
        if let Some(c) = cost {
            self.cost = c;
        }
        if let Some(s) = session_id {
            self.session_id = s;
        }
        if let Some(e) = elapsed_ms {
            self.elapsed_ms = e;
        }
        if let Some(t) = tool_count {
            self.tool_count = t;
        }
        if let Some(i) = input_tokens {
            self.input_tokens = i;
        }
        if let Some(o) = output_tokens {
            self.output_tokens = o;
        }
    }

    /// Format token count with K suffix for large numbers.
    fn fmt_tokens(n: u32) -> String {
        if n >= 10_000 {
            format!("{:.1}k", n as f64 / 1000.0)
        } else {
            format!("{n}")
        }
    }

    /// Format elapsed time from milliseconds.
    fn fmt_elapsed(ms: u64) -> String {
        if ms < 1_000 {
            format!("{ms}ms")
        } else if ms < 60_000 {
            format!("{:.1}s", ms as f64 / 1000.0)
        } else {
            let mins = ms / 60_000;
            let secs = (ms % 60_000) / 1000;
            format!("{mins}m{secs:02}s")
        }
    }

    /// Get live elapsed time (wall clock since session start).
    fn live_elapsed(&self) -> u64 {
        let wall = self.start_time.elapsed().as_millis() as u64;
        // Use the larger of wall clock and reported elapsed
        wall.max(self.elapsed_ms)
    }

    /// Render the status bar.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let sep = Span::styled(" │ ", Style::default().fg(Color::DarkGray));

        let status_line = if self.provider.is_empty() {
            Line::from(Span::styled(
                " Waiting for connection...",
                Style::default().fg(Color::DarkGray),
            ))
        } else {
            let elapsed = Self::fmt_elapsed(self.live_elapsed());
            let total_tok = self.input_tokens + self.output_tokens;
            let mut spans = vec![
                // Session ID
                Span::styled(" ", Style::default()),
                Span::styled(
                    "SESSION ",
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    &self.session_id,
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                ),
                sep.clone(),
                // Provider/model
                Span::styled(
                    &self.provider,
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ),
                Span::styled("/", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    &self.model,
                    Style::default().fg(Color::White),
                ),
                sep.clone(),
                // Round
                Span::styled(
                    format!("R{}", self.round),
                    Style::default().fg(Color::Yellow),
                ),
                sep.clone(),
                // Token breakdown: ↑input ↓output (total)
                Span::styled(
                    format!("↑{}", Self::fmt_tokens(self.input_tokens)),
                    Style::default().fg(Color::Green),
                ),
                Span::styled(" ", Style::default()),
                Span::styled(
                    format!("↓{}", Self::fmt_tokens(self.output_tokens)),
                    Style::default().fg(Color::Magenta),
                ),
                Span::styled(
                    format!(" ({})", Self::fmt_tokens(total_tok)),
                    Style::default().fg(Color::DarkGray),
                ),
                sep.clone(),
                // Cost
                Span::styled(
                    format!("${:.4}", self.cost),
                    Style::default().fg(Color::Yellow),
                ),
                sep.clone(),
                // Elapsed time
                Span::styled(
                    elapsed,
                    Style::default().fg(Color::Cyan),
                ),
            ];

            // Tool count (only show if > 0)
            if self.tool_count > 0 {
                spans.push(sep.clone());
                spans.push(Span::styled(
                    format!("{} tools", self.tool_count),
                    Style::default().fg(Color::Green),
                ));
            }

            // Plan step indicator
            if let Some(ref step_text) = self.plan_step {
                spans.push(sep);
                spans.push(Span::styled(
                    step_text.clone(),
                    Style::default().fg(Color::Cyan),
                ));
            }

            Line::from(spans)
        };

        let paragraph = Paragraph::new(status_line).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Status ")
                .border_style(Style::default().fg(Color::DarkGray)),
        );

        frame.render_widget(paragraph, area);
    }
}

impl Default for StatusState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_status_is_empty() {
        let status = StatusState::new();
        assert!(status.provider.is_empty());
        assert!(status.model.is_empty());
        assert!(status.session_id.is_empty());
        assert_eq!(status.round, 0);
        assert_eq!(status.input_tokens, 0);
        assert_eq!(status.output_tokens, 0);
        assert_eq!(status.cost, 0.0);
        assert_eq!(status.tool_count, 0);
    }

    #[test]
    fn update_sets_fields() {
        let mut status = StatusState::new();
        status.update(
            Some("deepseek".into()), Some("deepseek-chat".into()),
            Some(3), None, Some(0.0042),
            Some("abc12345".into()), Some(2500), Some(5),
            Some(1200), Some(450),
        );
        assert_eq!(status.provider, "deepseek");
        assert_eq!(status.model, "deepseek-chat");
        assert_eq!(status.session_id, "abc12345");
        assert_eq!(status.round, 3);
        assert_eq!(status.input_tokens, 1200);
        assert_eq!(status.output_tokens, 450);
        assert_eq!(status.tool_count, 5);
        assert!((status.cost - 0.0042).abs() < f64::EPSILON);
    }

    #[test]
    fn multiple_updates_overwrite() {
        let mut status = StatusState::new();
        status.update(
            Some("openai".into()), Some("gpt-4o".into()),
            Some(1), None, Some(0.01),
            Some("sess1".into()), Some(500), Some(2),
            Some(300), Some(100),
        );
        status.update(
            Some("deepseek".into()), Some("deepseek-coder".into()),
            Some(2), None, Some(0.002),
            None, Some(1500), Some(4),
            Some(800), Some(350),
        );
        assert_eq!(status.provider, "deepseek");
        assert_eq!(status.model, "deepseek-coder");
        assert_eq!(status.session_id, "sess1"); // Not overwritten (None).
        assert_eq!(status.round, 2);
        assert_eq!(status.input_tokens, 800);
        assert_eq!(status.output_tokens, 350);
    }

    #[test]
    fn default_matches_new() {
        let a = StatusState::new();
        let b = StatusState::default();
        assert_eq!(a.provider, b.provider);
        assert_eq!(a.round, b.round);
        assert_eq!(a.session_id, b.session_id);
    }

    #[test]
    fn fmt_tokens_small() {
        assert_eq!(StatusState::fmt_tokens(42), "42");
        assert_eq!(StatusState::fmt_tokens(9999), "9999");
    }

    #[test]
    fn fmt_tokens_large() {
        assert_eq!(StatusState::fmt_tokens(10_000), "10.0k");
        assert_eq!(StatusState::fmt_tokens(15_500), "15.5k");
        assert_eq!(StatusState::fmt_tokens(100_000), "100.0k");
    }

    #[test]
    fn fmt_elapsed_ms() {
        assert_eq!(StatusState::fmt_elapsed(42), "42ms");
        assert_eq!(StatusState::fmt_elapsed(999), "999ms");
    }

    #[test]
    fn fmt_elapsed_seconds() {
        assert_eq!(StatusState::fmt_elapsed(1000), "1.0s");
        assert_eq!(StatusState::fmt_elapsed(2500), "2.5s");
        assert_eq!(StatusState::fmt_elapsed(59999), "60.0s");
    }

    #[test]
    fn fmt_elapsed_minutes() {
        assert_eq!(StatusState::fmt_elapsed(60_000), "1m00s");
        assert_eq!(StatusState::fmt_elapsed(90_000), "1m30s");
        assert_eq!(StatusState::fmt_elapsed(125_000), "2m05s");
    }

    #[test]
    fn plan_step_defaults_to_none() {
        let status = StatusState::new();
        assert!(status.plan_step.is_none());
    }
}
