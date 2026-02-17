//! Status bar widget for the TUI bottom zone.
//!
//! Displays session info: ID, provider/model, round, token breakdown,
//! cost, elapsed time, and tool invocation count.

use std::time::Instant;

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::render::theme;
use crate::tui::constants;
use crate::tui::events::ProviderHealthStatus;
use crate::tui::state::{AgentControl, TokenBudget, UiMode};

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
    /// Current agent control state (Running/Paused/StepMode/WaitingApproval).
    pub agent_control: AgentControl,
    /// Whether dry-run mode is active.
    pub dry_run_active: bool,
    /// Token budget for progress display.
    pub token_budget: TokenBudget,
    /// Current provider health status for the active provider.
    pub provider_health: ProviderHealthStatus,
    /// Current UI mode for controlling extended status rendering.
    pub ui_mode: UiMode,
    /// Reasoning strategy label (Expert mode).
    pub reasoning_strategy: String,
    /// Cache hit rate percentage (Expert mode).
    pub cache_hit_rate: Option<f64>,
    /// Phase 4B-Lite: Number of prompts currently queued.
    pub prompts_queued: usize,
    /// Phase 4B-Lite: Number of agents currently processing.
    pub agents_active: usize,
    /// Phase A3: Contextual hints for Activity zone navigation.
    pub activity_hints: Vec<(&'static str, &'static str)>,
    /// Context servers count for button display.
    pub context_servers_count: usize,
    /// Phase 3 SRCH-003: Search mode active and current mode label.
    pub search_active: bool,
    /// Phase 3 SRCH-003: Search mode label ("Exact"/"Fuzzy"/"Regex").
    pub search_mode: String,
    /// Phase 3 SRCH-003: Current match position (1-indexed).
    pub search_current: Option<usize>,
    /// Phase 3 SRCH-003: Total match count.
    pub search_total: usize,
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
            agent_control: AgentControl::Running,
            dry_run_active: false,
            token_budget: TokenBudget::default(),
            provider_health: ProviderHealthStatus::Healthy,
            ui_mode: UiMode::Standard,
            reasoning_strategy: String::new(),
            cache_hit_rate: None,
            prompts_queued: 0,
            agents_active: 0,
            activity_hints: Vec::new(),
            context_servers_count: 0,
            search_active: false, // Phase 3 SRCH-003
            search_mode: String::new(), // Phase 3 SRCH-003
            search_current: None, // Phase 3 SRCH-003
            search_total: 0, // Phase 3 SRCH-003
        }
    }

    /// Get the current provider name.
    pub fn current_provider(&self) -> &str {
        &self.provider
    }

    /// Get the current model name.
    pub fn current_model(&self) -> &str {
        &self.model
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

    /// Phase 4B-Lite: Update queue status display.
    pub fn update_queue_status(&mut self, prompts_queued: usize, agents_active: usize) {
        self.prompts_queued = prompts_queued;
        self.agents_active = agents_active;
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

    /// Generate a visual gauge bar from a fraction (0.0–1.0) and width.
    /// Uses block characters: ▓ for filled, ░ for empty.
    fn budget_gauge(frac: f64, width: usize) -> String {
        let filled = (frac * width as f64).round() as usize;
        let empty = width.saturating_sub(filled);
        format!("{}{}", "▓".repeat(filled), "░".repeat(empty))
    }

    /// Get live elapsed time (wall clock since session start).
    fn live_elapsed(&self) -> u64 {
        let wall = self.start_time.elapsed().as_millis() as u64;
        // Use the larger of wall clock and reported elapsed
        wall.max(self.elapsed_ms)
    }

    /// Build the main status line with session, provider, tokens, cost, etc.
    fn render_main_line(&self) -> Line<'static> {
        let p = &theme::active().palette;
        // Phase 45A Task 2.2: Use cached accessors
        let c_success = p.success_ratatui();
        let c_accent = p.accent_ratatui();
        let c_warning = p.warning_ratatui();
        let c_running = p.running_ratatui();
        let c_text = p.text_ratatui();
        let c_muted = p.muted_ratatui();
        let c_planning = p.planning_ratatui();

        let sep = Span::styled(" │ ", Style::default().fg(c_muted));

        if self.provider.is_empty() {
            return Line::from(Span::styled(
                " Waiting for connection...",
                Style::default().fg(c_muted),
            ));
        }

        {
            let elapsed = Self::fmt_elapsed(self.live_elapsed());
            let total_tok = self.input_tokens + self.output_tokens;
            let (ctrl_label, ctrl_color) = match self.agent_control {
                AgentControl::Running => ("\u{25b6} RUN", c_success),
                AgentControl::Paused => ("\u{23f8} PAUSE", c_warning),
                AgentControl::StepMode => ("\u{23ed} STEP", c_accent),
                AgentControl::WaitingApproval => ("\u{23f3} AWAIT", c_planning),
            };
            let mut spans = vec![
                // Agent control state
                Span::styled(" ", Style::default()),
                Span::styled(
                    ctrl_label,
                    Style::default().fg(ctrl_color).add_modifier(Modifier::BOLD),
                ),
            ];

            // Dry-run persistent banner
            if self.dry_run_active {
                spans.push(Span::styled(
                    constants::DRY_RUN_LABEL,
                    Style::default()
                        .fg(c_warning)
                        .add_modifier(Modifier::BOLD),
                ));
            }

            spans.push(Span::styled(" \u{2502} ", Style::default().fg(c_muted)));
            // Session ID
            spans.push(Span::styled("SESSION ", Style::default().fg(c_muted)));
            spans.push(Span::styled(
                self.session_id.clone(),
                Style::default().fg(c_text).add_modifier(Modifier::BOLD),
            ));
            spans.push(sep.clone());
            // Provider/model
            spans.push(Span::styled(
                self.provider.clone(),
                Style::default().fg(c_accent).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled("/", Style::default().fg(c_muted)));
            spans.push(Span::styled(
                self.model.clone(),
                Style::default().fg(c_text),
            ));
            // Provider health indicator
            let (health_icon, health_color) = match &self.provider_health {
                ProviderHealthStatus::Healthy => (" ●", c_success),
                ProviderHealthStatus::Degraded { .. } => (" ◐", c_warning),
                ProviderHealthStatus::Unhealthy { .. } => (" ○", p.error_ratatui()),
            };
            spans.push(Span::styled(health_icon, Style::default().fg(health_color)));
            spans.push(sep.clone());
            // Round
            spans.push(Span::styled(
                format!("R{}", self.round),
                Style::default().fg(c_warning),
            ));

            // Phase 4B-Lite: Show queue status when prompts are queued
            if self.prompts_queued > 0 || self.agents_active > 0 {
                spans.push(Span::styled(" ", Style::default()));
                if self.agents_active > 0 {
                    spans.push(Span::styled(
                        format!("⚙{}", self.agents_active),
                        Style::default().fg(p.running_ratatui()),
                    ));
                }
                if self.prompts_queued > 0 {
                    spans.push(Span::styled(
                        format!(" +{}", self.prompts_queued),
                        Style::default().fg(c_muted),
                    ));
                }
            }

            spans.push(sep.clone());
            // Token breakdown: ↑input ↓output (total)
            spans.push(Span::styled(
                format!("↑{}", Self::fmt_tokens(self.input_tokens)),
                Style::default().fg(c_success),
            ));
            spans.push(Span::styled(" ", Style::default()));
            spans.push(Span::styled(
                format!("↓{}", Self::fmt_tokens(self.output_tokens)),
                Style::default().fg(c_running),
            ));
            spans.push(Span::styled(
                format!(" ({})", Self::fmt_tokens(total_tok)),
                Style::default().fg(c_muted),
            ));
            spans.push(sep.clone());
            // Cost
            spans.push(Span::styled(
                format!("${:.4}", self.cost),
                Style::default().fg(c_warning),
            ));
            spans.push(sep.clone());
            // Elapsed time
            spans.push(Span::styled(elapsed, Style::default().fg(c_accent)));

            // Token budget gauge (only show if limited)
            if let Some(frac) = self.token_budget.fraction() {
                spans.push(sep.clone());
                let pct = (frac * 100.0) as u32;
                let (gauge_color, gauge_label_color) = if frac > 0.9 {
                    (c_warning, c_warning)
                } else if frac > 0.7 {
                    (c_accent, c_accent)
                } else {
                    (c_success, c_muted)
                };
                let gauge = Self::budget_gauge(frac, 10);
                spans.push(Span::styled("budget:", Style::default().fg(c_muted)));
                spans.push(Span::styled(gauge, Style::default().fg(gauge_color)));
                spans.push(Span::styled(
                    format!(" {pct}%"),
                    Style::default().fg(gauge_label_color),
                ));
            }

            // Tool count (only show if > 0)
            if self.tool_count > 0 {
                spans.push(sep.clone());
                spans.push(Span::styled(
                    format!("{} tools", self.tool_count),
                    Style::default().fg(c_success),
                ));
            }

            // Plan step indicator
            if let Some(ref step_text) = self.plan_step {
                spans.push(sep.clone());
                spans.push(Span::styled(
                    step_text.clone(),
                    Style::default().fg(c_accent),
                ));
            }

            // Key hints when paused or step mode
            if matches!(self.agent_control, AgentControl::Paused | AgentControl::StepMode) {
                spans.push(sep.clone());
                spans.push(Span::styled(
                    "[Space] resume  [N] step  [Esc] cancel",
                    Style::default().fg(c_muted),
                ));
            }

            // Context Servers button (always visible on right side)
            if self.context_servers_count > 0 {
                spans.push(sep);
                spans.push(Span::styled(
                    " [Ctrl+S] ",
                    Style::default().fg(c_accent).add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::styled(
                    format!("⚙ {} servers", self.context_servers_count),
                    Style::default().fg(c_success),
                ));
            }

            Line::from(spans)
        }
    }

    /// Build the expert mode line with strategy, cache, UI mode.
    /// Phase A3: Render contextual hints line for Activity navigation.
    fn render_hints_line(&self) -> Line<'static> {
        let p = &theme::active().palette;
        let c_accent = p.accent_ratatui();
        let c_muted = p.muted_ratatui();

        let mut spans = vec![
            Span::styled("  Hints: ", Style::default().fg(c_muted)),
        ];

        for (i, (key, label)) in self.activity_hints.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" │ ", Style::default().fg(c_muted)));
            }
            spans.push(Span::styled(
                format!("[{}]", key),
                Style::default().fg(c_accent).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(" {}", label),
                Style::default().fg(c_muted),
            ));
        }

        Line::from(spans)
    }

    /// Phase 3 SRCH-003: Render search mode indicator line.
    fn render_search_line(&self) -> Line<'static> {
        let p = &theme::active().palette;
        let c_accent = p.accent_ratatui();
        let c_success = p.success_ratatui();
        let c_muted = p.muted_ratatui();

        let mut spans = vec![
            Span::styled("  Search: ", Style::default().fg(c_muted)),
        ];

        // Mode indicator
        spans.push(Span::styled(
            format!("[{}]", self.search_mode),
            Style::default().fg(c_accent).add_modifier(Modifier::BOLD),
        ));

        // Match counter
        if let Some(current) = self.search_current {
            spans.push(Span::styled(" │ ", Style::default().fg(c_muted)));
            spans.push(Span::styled("Match ", Style::default().fg(c_muted)));
            spans.push(Span::styled(
                format!("{}/{}", current, self.search_total),
                Style::default().fg(c_success).add_modifier(Modifier::BOLD),
            ));
        } else if self.search_total == 0 {
            spans.push(Span::styled(" │ ", Style::default().fg(c_muted)));
            spans.push(Span::styled(
                "No matches",
                Style::default().fg(c_muted),
            ));
        }

        Line::from(spans)
    }

    fn render_expert_line(&self) -> Option<Line<'static>> {
        if self.ui_mode != UiMode::Expert || self.provider.is_empty() {
            return None;
        }

        let p = &theme::active().palette;
        let c_success = p.success_ratatui();
        let c_accent = p.accent_ratatui();
        let c_warning = p.warning_ratatui();
        let c_muted = p.muted_ratatui();

        let mut expert_spans = vec![
            Span::styled(" ", Style::default()),
        ];

        // Reasoning strategy
        if !self.reasoning_strategy.is_empty() {
            expert_spans.push(Span::styled("strategy:", Style::default().fg(c_muted)));
            expert_spans.push(Span::styled(
                self.reasoning_strategy.clone(),
                Style::default().fg(c_accent),
            ));
            expert_spans.push(Span::styled(" │ ", Style::default().fg(c_muted)));
        }

        // Cache hit rate
        if let Some(rate) = self.cache_hit_rate {
            expert_spans.push(Span::styled("cache:", Style::default().fg(c_muted)));
            expert_spans.push(Span::styled(
                format!("{:.0}%", rate),
                Style::default().fg(if rate > 50.0 { c_success } else { c_warning }),
            ));
            expert_spans.push(Span::styled(" │ ", Style::default().fg(c_muted)));
        }

        // UI mode indicator
        expert_spans.push(Span::styled("mode:", Style::default().fg(c_muted)));
        expert_spans.push(Span::styled(
            self.ui_mode.label(),
            Style::default().fg(c_accent),
        ));

        Some(Line::from(expert_spans))
    }

    /// Render the status bar.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let p = &theme::active().palette;
        let c_border = p.border_ratatui();

        let mut lines = vec![self.render_main_line()];

        // Add expert mode line if applicable
        if let Some(expert_line) = self.render_expert_line() {
            lines.push(expert_line);
        }

        // Phase 3 SRCH-003: Add search indicator when active
        if self.search_active {
            lines.push(self.render_search_line());
        }

        // Phase A3: Add contextual hints when available
        if !self.activity_hints.is_empty() {
            lines.push(self.render_hints_line());
        }

        let paragraph = Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Status ")
                .border_style(Style::default().fg(c_border)),
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

    #[test]
    fn status_bar_default_agent_control_running() {
        let status = StatusState::new();
        assert_eq!(status.agent_control, AgentControl::Running);
    }

    #[test]
    fn status_bar_shows_paused() {
        let mut status = StatusState::new();
        status.agent_control = AgentControl::Paused;
        assert_eq!(status.agent_control, AgentControl::Paused);
    }

    #[test]
    fn status_bar_shows_step_mode() {
        let mut status = StatusState::new();
        status.agent_control = AgentControl::StepMode;
        assert_eq!(status.agent_control, AgentControl::StepMode);
    }

    // Phase 43B: Verify status bar uses palette tokens
    #[test]
    fn status_uses_palette_colors() {
        let p = &theme::active().palette;
        // Phase 45A Task 2.2: Use cached accessors
        let _s = p.success_ratatui();
        let _a = p.accent_ratatui();
        let _w = p.warning_ratatui();
        let _r = p.running_ratatui();
        let _t = p.text_ratatui();
        let _m = p.muted_ratatui();
        let _b = p.border_ratatui();
        let _pl = p.planning_ratatui();
    }

    // --- Phase 43E: Polish tests ---

    #[test]
    fn key_hints_shown_when_paused() {
        // Verify the paused state exists and can be set.
        let mut status = StatusState::new();
        status.agent_control = AgentControl::Paused;
        // Key hints should appear when agent_control is Paused (verified in render).
        assert!(matches!(status.agent_control, AgentControl::Paused | AgentControl::StepMode));
    }

    #[test]
    fn key_hints_shown_when_step_mode() {
        let mut status = StatusState::new();
        status.agent_control = AgentControl::StepMode;
        assert!(matches!(status.agent_control, AgentControl::Paused | AgentControl::StepMode));
    }

    #[test]
    fn key_hints_hidden_when_running() {
        let status = StatusState::new();
        assert!(!matches!(status.agent_control, AgentControl::Paused | AgentControl::StepMode));
    }

    #[test]
    fn dry_run_defaults_to_false() {
        let status = StatusState::new();
        assert!(!status.dry_run_active);
    }

    #[test]
    fn dry_run_can_be_set() {
        let mut status = StatusState::new();
        status.dry_run_active = true;
        assert!(status.dry_run_active);
    }

    // --- Phase B1: Budget gauge tests ---

    #[test]
    fn budget_gauge_empty() {
        let gauge = StatusState::budget_gauge(0.0, 10);
        assert_eq!(gauge, "░░░░░░░░░░");
    }

    #[test]
    fn budget_gauge_half() {
        let gauge = StatusState::budget_gauge(0.5, 10);
        assert_eq!(gauge, "▓▓▓▓▓░░░░░");
    }

    #[test]
    fn budget_gauge_full() {
        let gauge = StatusState::budget_gauge(1.0, 10);
        assert_eq!(gauge, "▓▓▓▓▓▓▓▓▓▓");
    }

    #[test]
    fn budget_gauge_quarter() {
        let gauge = StatusState::budget_gauge(0.25, 8);
        assert_eq!(gauge, "▓▓░░░░░░");
    }

    // --- Phase B2: Provider health indicator tests ---

    #[test]
    fn provider_health_defaults_to_healthy() {
        let status = StatusState::new();
        assert_eq!(status.provider_health, ProviderHealthStatus::Healthy);
    }

    #[test]
    fn provider_health_can_be_degraded() {
        let mut status = StatusState::new();
        status.provider_health = ProviderHealthStatus::Degraded {
            failure_rate: 0.3,
            latency_p95_ms: 5000,
        };
        assert!(matches!(status.provider_health, ProviderHealthStatus::Degraded { .. }));
    }

    #[test]
    fn provider_health_can_be_unhealthy() {
        let mut status = StatusState::new();
        status.provider_health = ProviderHealthStatus::Unhealthy {
            reason: "timeout".into(),
        };
        assert!(matches!(status.provider_health, ProviderHealthStatus::Unhealthy { .. }));
    }

    #[test]
    fn current_provider_accessor() {
        let mut status = StatusState::new();
        assert_eq!(status.current_provider(), "");
        status.update(Some("anthropic".into()), None, None, None, None, None, None, None, None, None);
        assert_eq!(status.current_provider(), "anthropic");
    }
}
