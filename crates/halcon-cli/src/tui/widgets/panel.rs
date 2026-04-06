//! Side panel widget for the cockpit TUI layout.
//!
//! Displays plan steps, metrics, and context tier usage in a collapsible panel.
//! Phase 3.1: Added real-time updates with transition effects and threshold-based highlights.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::render::theme;
use crate::tui::events::{CircuitBreakerState, PlanStepDisplayStatus, PlanStepStatus};
use crate::tui::highlight::HighlightManager;
use crate::tui::state::PanelSection;
use crate::tui::transition_engine::TransitionEngine;
use crate::tui::widgets::context_viz::{ContextVisualization, TierData};

/// Metrics displayed in the side panel.
#[derive(Debug, Clone, Default)]
pub struct PanelMetrics {
    pub total_tokens: u64,
    pub total_cost: f64,
    pub tool_count: u32,
    pub round: usize,
    pub elapsed_ms: u64,
    pub cache_hits: u32,
    pub cache_misses: u32,
    // Phase 2 metrics
    pub delegation_success_rate: Option<f64>,
    pub delegation_trigger_rate: Option<f64>,
    pub plan_success_rate: Option<f64>,
    pub ucb1_agreement_rate: Option<f64>,
}

/// Context tier usage data from the context pipeline.
#[derive(Debug, Clone, Default)]
pub struct ContextTiers {
    pub l0_pct: u8,
    pub l1_pct: u8,
    pub l2_pct: u8,
    pub l3_pct: u8,
    pub l4_pct: u8,
    /// Raw token/entry counts for detail display.
    pub l0_tokens: u32,
    pub l0_capacity: u32,
    pub l1_tokens: u32,
    pub l1_entries: usize,
    pub l2_entries: usize,
    pub l3_entries: usize,
    pub l4_entries: usize,
    pub total_tokens: u32,
}

/// Reasoning engine status for panel display.
#[derive(Debug, Clone, Default)]
pub struct ReasoningInfo {
    pub strategy: String,
    pub task_type: String,
    pub complexity: String,
}

/// Per-provider circuit breaker state for panel display.
#[derive(Debug, Clone)]
pub struct BreakerInfo {
    pub provider: String,
    pub state: CircuitBreakerState,
    pub failure_count: u32,
}

/// Side panel state and rendering.
///
/// Phase 3.1: Extended with TransitionEngine and HighlightManager for
/// smooth metric updates and threshold-based visual alerts.
pub struct SidePanel {
    pub plan_steps: Vec<PlanStepStatus>,
    pub current_step: usize,
    pub metrics: PanelMetrics,
    pub context_tiers: ContextTiers,
    pub reasoning: ReasoningInfo,
    pub breakers: Vec<BreakerInfo>,
    /// Phase 3.1: Transition engine for smooth metric updates.
    transitions: TransitionEngine,
    /// Phase 3.1: Highlight manager for threshold-based alerts.
    highlights: HighlightManager,
    /// Phase 3.1: Previous L0 percentage for detecting overflow.
    last_l0_pct: u8,
    /// Phase 3.1: Previous total cost for detecting threshold crossings.
    last_cost: f64,
    /// Phase 3.2: Context visualization with animated gauges.
    context_viz: ContextVisualization,
    /// Scroll offset for panel content (0 = top).
    scroll_offset: u16,
}

impl SidePanel {
    pub fn new() -> Self {
        Self {
            plan_steps: Vec::new(),
            current_step: 0,
            metrics: PanelMetrics::default(),
            context_tiers: ContextTiers::default(),
            reasoning: ReasoningInfo::default(),
            breakers: Vec::new(),
            transitions: TransitionEngine::new(),
            highlights: HighlightManager::new(),
            last_l0_pct: 0,
            last_cost: 0.0,
            context_viz: ContextVisualization::new(),
            scroll_offset: 0,
        }
    }

    /// Scroll panel content up (decrease offset).
    pub fn scroll_up(&mut self, amount: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }

    /// Scroll panel content down (increase offset).
    pub fn scroll_down(&mut self, amount: u16, max_lines: u16, viewport_height: u16) {
        if max_lines > viewport_height {
            let max_offset = max_lines.saturating_sub(viewport_height);
            self.scroll_offset = (self.scroll_offset + amount).min(max_offset);
        }
    }

    /// Reset scroll to top.
    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = 0;
    }

    /// Update metrics from round-ended event data.
    ///
    /// Phase 3.1: Detects cost threshold crossings ($0.10, $1.00, $10.00) and starts alert pulse.
    pub fn update_metrics(
        &mut self,
        round: usize,
        input_tokens: u32,
        output_tokens: u32,
        cost: f64,
        duration_ms: u64,
    ) {
        self.metrics.round = round;
        self.metrics.total_tokens += (input_tokens + output_tokens) as u64;
        self.metrics.total_cost += cost;
        self.metrics.elapsed_ms = duration_ms;

        // Phase 3.1: Detect cost threshold crossings and pulse
        let thresholds = [0.10, 1.0, 10.0];
        for &threshold in &thresholds {
            if self.last_cost < threshold && self.metrics.total_cost >= threshold {
                // Crossed threshold - start strong pulse
                let p = &theme::active().palette;
                let color = if threshold >= 10.0 {
                    p.destructive // Red for $10+
                } else if threshold >= 1.0 {
                    p.warning // Yellow for $1+
                } else {
                    p.accent // Blue for $0.10+
                };
                self.highlights.start_strong("cost_threshold", color);
            }
        }
        self.last_cost = self.metrics.total_cost;
    }

    /// Update plan steps from plan progress event.
    pub fn update_plan(&mut self, steps: Vec<PlanStepStatus>, current_step: usize) {
        self.plan_steps = steps;
        self.current_step = current_step;
    }

    /// Update context tier data from pipeline metrics.
    ///
    /// Phase 3.1: Detects L0 overflow (>90%) and starts warning highlight.
    pub fn update_context(
        &mut self,
        l0_tokens: u32,
        l0_capacity: u32,
        l1_tokens: u32,
        l1_entries: usize,
        l2_entries: usize,
        l3_entries: usize,
        l4_entries: usize,
        total_tokens: u32,
    ) {
        self.context_tiers.l0_tokens = l0_tokens;
        self.context_tiers.l0_capacity = l0_capacity;
        self.context_tiers.l1_tokens = l1_tokens;
        self.context_tiers.l1_entries = l1_entries;
        self.context_tiers.l2_entries = l2_entries;
        self.context_tiers.l3_entries = l3_entries;
        self.context_tiers.l4_entries = l4_entries;
        self.context_tiers.total_tokens = total_tokens;
        // Compute percentages relative to total.
        if total_tokens > 0 {
            self.context_tiers.l0_pct =
                ((l0_tokens as f64 / total_tokens as f64) * 100.0).min(100.0) as u8;
            self.context_tiers.l1_pct =
                ((l1_tokens as f64 / total_tokens as f64) * 100.0).min(100.0) as u8;

            // FIX: Compute L2-L4 percentages based on entry counts
            // L2-L4 don't have direct token counts, estimate based on entries
            let l2_estimated = l2_entries as u32 * 200; // ~200 tokens per compressed entry
            let l3_estimated = l3_entries as u32 * 150; // ~150 tokens per semantic entry
            let l4_estimated = l4_entries as u32 * 100; // ~100 tokens per archived entry

            self.context_tiers.l2_pct =
                ((l2_estimated as f64 / total_tokens as f64) * 100.0).min(100.0) as u8;
            self.context_tiers.l3_pct =
                ((l3_estimated as f64 / total_tokens as f64) * 100.0).min(100.0) as u8;
            self.context_tiers.l4_pct =
                ((l4_estimated as f64 / total_tokens as f64) * 100.0).min(100.0) as u8;

            // Phase 3.1: Detect L0 overflow (>90% capacity) and start warning pulse
            if self.context_tiers.l0_pct > 90 && self.last_l0_pct <= 90 {
                let p = &theme::active().palette;
                self.highlights.start_medium("l0_overflow", p.warning);
            } else if self.context_tiers.l0_pct <= 90 && self.last_l0_pct > 90 {
                // L0 recovered below threshold - stop pulse
                self.highlights.stop("l0_overflow");
            }
            self.last_l0_pct = self.context_tiers.l0_pct;

            // Phase 3.2: Update context visualization with tier data
            let tier_data = TierData {
                l0_tokens,
                l0_capacity,
                l0_pct: self.context_tiers.l0_pct,
                l1_tokens,
                l1_entries,
                l1_pct: self.context_tiers.l1_pct,
                l2_entries,
                l2_pct: self.context_tiers.l2_pct,
                l3_entries,
                l3_pct: self.context_tiers.l3_pct,
                l4_entries,
                l4_pct: self.context_tiers.l4_pct,
                total_tokens,
            };
            self.context_viz.update(tier_data);
        }
    }

    /// Update or insert circuit breaker state for a provider.
    pub fn update_breaker(
        &mut self,
        provider: String,
        state: CircuitBreakerState,
        failure_count: u32,
    ) {
        if let Some(b) = self.breakers.iter_mut().find(|b| b.provider == provider) {
            b.state = state;
            b.failure_count = failure_count;
        } else {
            self.breakers.push(BreakerInfo {
                provider,
                state,
                failure_count,
            });
        }
    }

    /// Record a cache hit or miss.
    pub fn record_cache(&mut self, hit: bool) {
        if hit {
            self.metrics.cache_hits += 1;
        } else {
            self.metrics.cache_misses += 1;
        }
    }

    /// Update reasoning engine info.
    pub fn update_reasoning(&mut self, strategy: String, task_type: String, complexity: String) {
        self.reasoning = ReasoningInfo {
            strategy,
            task_type,
            complexity,
        };
    }

    /// Update Phase 2 metrics (orchestrator, planning, strategy).
    pub fn update_phase2_metrics(
        &mut self,
        delegation_success_rate: Option<f64>,
        delegation_trigger_rate: Option<f64>,
        plan_success_rate: Option<f64>,
        ucb1_agreement_rate: Option<f64>,
    ) {
        self.metrics.delegation_success_rate = delegation_success_rate;
        self.metrics.delegation_trigger_rate = delegation_trigger_rate;
        self.metrics.plan_success_rate = plan_success_rate;
        self.metrics.ucb1_agreement_rate = ucb1_agreement_rate;
    }

    /// Tick the panel to prune completed transitions and update animations.
    ///
    /// Phase 3.1: Should be called on each render loop tick (100ms interval).
    /// Phase 3.2: Also ticks context visualization for gauge animations.
    /// One-line metrics summary for /metrics command.
    pub fn metrics_summary(&self) -> String {
        let m = &self.metrics;
        let cache_rate = if m.cache_hits + m.cache_misses > 0 {
            let rate = m.cache_hits as f64 / (m.cache_hits + m.cache_misses) as f64 * 100.0;
            format!("  cache {:.0}%", rate)
        } else {
            String::new()
        };
        format!(
            "Round {}  {} tokens  ${:.4}  {} tools{}",
            m.round, m.total_tokens, m.total_cost, m.tool_count, cache_rate
        )
    }

    /// One-line context tier summary for /context command.
    pub fn context_summary(&self) -> String {
        let t = &self.context_tiers;
        format!(
            "L0 {}% ({}/{} tok)  L1 {}% ({} ent)  L2 {}%  L3 {}%  L4 {}%",
            t.l0_pct,
            t.l0_tokens,
            t.l0_capacity,
            t.l1_pct,
            t.l1_entries,
            t.l2_pct,
            t.l3_pct,
            t.l4_pct
        )
    }

    /// One-line reasoning summary for /why command.
    pub fn reasoning_summary(&self) -> String {
        let r = &self.reasoning;
        if r.strategy.is_empty() {
            "No reasoning data available yet".to_string()
        } else {
            format!(
                "Strategy: {}  Task: {}  Complexity: {}",
                r.strategy, r.task_type, r.complexity
            )
        }
    }

    pub fn tick(&mut self) {
        self.transitions.prune_completed();
        self.context_viz.tick();
        // Highlights auto-expire after 3 seconds
        // Note: HighlightManager doesn't have auto-expire yet, but pulses continue indefinitely
        // We rely on stop() being called when thresholds return to normal
    }

    /// Render the side panel.
    pub fn render(&self, frame: &mut Frame, area: Rect, section: PanelSection) {
        let p = &theme::active().palette;
        let c_success = p.success_ratatui();
        let c_error = p.error_ratatui();
        let c_running = p.running_ratatui();
        let c_muted = p.muted_ratatui();
        let c_border = p.border_ratatui();
        let c_text_label = p.text_label_ratatui();
        let c_bg_panel = p.bg_panel_ratatui();

        let mut lines: Vec<Line<'_>> = Vec::new();

        let show_plan = matches!(section, PanelSection::Plan | PanelSection::All);
        let show_metrics = matches!(section, PanelSection::Metrics | PanelSection::All);
        let show_context = matches!(section, PanelSection::Context | PanelSection::All);
        let show_reasoning = matches!(section, PanelSection::Reasoning | PanelSection::All);

        // Plan section
        if show_plan {
            lines.push(Line::from(Span::styled(
                "── Plan ──",
                Style::default()
                    .fg(c_text_label)
                    .add_modifier(Modifier::BOLD),
            )));
            if self.plan_steps.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  (no plan)",
                    Style::default().fg(c_muted),
                )));
            } else {
                for (i, step) in self.plan_steps.iter().enumerate() {
                    let icon = match step.status {
                        PlanStepDisplayStatus::Succeeded => "✓",
                        PlanStepDisplayStatus::Failed => "✗",
                        PlanStepDisplayStatus::InProgress => "▸",
                        PlanStepDisplayStatus::Skipped => "−",
                        PlanStepDisplayStatus::Pending => "○",
                    };
                    let style = match step.status {
                        PlanStepDisplayStatus::Succeeded => Style::default().fg(c_success),
                        PlanStepDisplayStatus::Failed => Style::default().fg(c_error),
                        PlanStepDisplayStatus::InProgress => Style::default().fg(c_running),
                        PlanStepDisplayStatus::Skipped => Style::default().fg(c_muted),
                        PlanStepDisplayStatus::Pending => Style::default().fg(c_border),
                    };
                    // Truncate description to fit panel width — char-safe (no byte-slice).
                    let max_desc = (area.width as usize).saturating_sub(6);
                    let desc = if step.description.chars().count() > max_desc {
                        let truncated: String = step
                            .description
                            .chars()
                            .take(max_desc.saturating_sub(1))
                            .collect();
                        format!("{truncated}…")
                    } else {
                        step.description.clone()
                    };
                    let prefix = if i == self.current_step
                        && step.status == PlanStepDisplayStatus::Pending
                    {
                        "▸"
                    } else {
                        icon
                    };
                    lines.push(Line::from(Span::styled(format!(" {prefix} {desc}"), style)));
                }
            }
            lines.push(Line::from(""));
        }

        // Metrics section
        if show_metrics {
            lines.push(Line::from(Span::styled(
                "── Metrics ──",
                Style::default()
                    .fg(c_text_label)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(format!("  Round: {}", self.metrics.round)));
            lines.push(Line::from(format!(
                "  Tokens: {}",
                fmt_tokens(self.metrics.total_tokens)
            )));
            lines.push(Line::from(format!(
                "  Cost: ${:.4}",
                self.metrics.total_cost
            )));
            lines.push(Line::from(format!("  Tools: {}", self.metrics.tool_count)));
            if self.metrics.elapsed_ms > 0 {
                lines.push(Line::from(format!(
                    "  Time: {}",
                    fmt_elapsed(self.metrics.elapsed_ms)
                )));
            }
            // Cache hit rate (only show if any cache events recorded)
            let cache_total = self.metrics.cache_hits + self.metrics.cache_misses;
            if cache_total > 0 {
                let rate = (self.metrics.cache_hits as f64 / cache_total as f64) * 100.0;
                lines.push(Line::from(format!(
                    "  Cache: {}/{} ({:.0}%)",
                    self.metrics.cache_hits, cache_total, rate
                )));
            }

            // Phase 2 metrics (solo mostrar si hay datos)
            if self.metrics.delegation_success_rate.is_some()
                || self.metrics.plan_success_rate.is_some()
                || self.metrics.ucb1_agreement_rate.is_some()
            {
                lines.push(Line::from("")); // Separador
                lines.push(Line::from(Span::styled(
                    "  -- Phase 2 --",
                    Style::default().fg(c_muted).add_modifier(Modifier::DIM),
                )));

                if let Some(rate) = self.metrics.delegation_success_rate {
                    lines.push(Line::from(format!("  Delegation: {:.0}%", rate * 100.0)));
                }
                if let Some(rate) = self.metrics.delegation_trigger_rate {
                    lines.push(Line::from(format!("  Trigger: {:.1}%", rate * 100.0)));
                }
                if let Some(rate) = self.metrics.plan_success_rate {
                    lines.push(Line::from(format!("  Plan success: {:.0}%", rate * 100.0)));
                }
                if let Some(rate) = self.metrics.ucb1_agreement_rate {
                    let divergence = (1.0 - rate) * 100.0;
                    lines.push(Line::from(format!("  UCB1 ∆: {:.0}%", divergence)));
                }
            }

            // Circuit breaker states (only show non-Closed breakers)
            for b in &self.breakers {
                let (icon, color) = match b.state {
                    CircuitBreakerState::Closed => continue,
                    CircuitBreakerState::Open => ("○ OPEN", c_error),
                    CircuitBreakerState::HalfOpen => ("◐ HALF", p.warning_ratatui()),
                };
                lines.push(Line::from(Span::styled(
                    format!("  {} {} ({})", icon, b.provider, b.failure_count),
                    Style::default().fg(color),
                )));
            }
            lines.push(Line::from(""));
        }

        // Context section (Phase 3.2: Using ContextVisualization with animated gauges)
        if show_context {
            lines.push(Line::from(Span::styled(
                "── Context ──",
                Style::default()
                    .fg(c_text_label)
                    .add_modifier(Modifier::BOLD),
            )));
            if self.context_viz.data().total_tokens > 0 {
                // Render context visualization with animated gauges
                lines.extend(self.context_viz.render_lines());
            } else {
                lines.push(Line::from(Span::styled(
                    "  (no data)",
                    Style::default().fg(c_muted),
                )));
            }
            lines.push(Line::from(""));
        }

        // Reasoning section
        if show_reasoning {
            lines.push(Line::from(Span::styled(
                "── Reasoning ──",
                Style::default()
                    .fg(c_text_label)
                    .add_modifier(Modifier::BOLD),
            )));
            if self.reasoning.strategy.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  (no data)",
                    Style::default().fg(c_muted),
                )));
            } else {
                lines.push(Line::from(format!(
                    "  Strategy:   {}",
                    self.reasoning.strategy
                )));
                lines.push(Line::from(format!(
                    "  Task Type:  {}",
                    self.reasoning.task_type
                )));
                lines.push(Line::from(format!(
                    "  Complexity: {}",
                    self.reasoning.complexity
                )));
            }
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Panel ")
            .border_style(Style::default().fg(c_border))
            .style(Style::default().bg(c_bg_panel));

        // Apply scroll offset
        let total_lines = lines.len() as u16;
        let inner_area = block.inner(area);
        let viewport_height = inner_area.height;

        // Calculate visible range with scroll
        let start = self.scroll_offset.min(total_lines.saturating_sub(1));
        let end = (start + viewport_height).min(total_lines);
        let visible_lines: Vec<Line> = lines
            .into_iter()
            .skip(start as usize)
            .take((end - start) as usize)
            .collect();

        let paragraph = Paragraph::new(visible_lines)
            .block(block)
            .wrap(Wrap { trim: true });

        frame.render_widget(paragraph, area);

        // Render scrollbar if content overflows
        if total_lines > viewport_height {
            use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState};

            let scrollbar = Scrollbar::default()
                .orientation(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓"));

            let mut scrollbar_state =
                ScrollbarState::new(total_lines.saturating_sub(viewport_height) as usize)
                    .position(self.scroll_offset as usize);

            frame.render_stateful_widget(scrollbar, inner_area, &mut scrollbar_state);
        }
    }
}

impl Default for SidePanel {
    fn default() -> Self {
        Self::new()
    }
}

fn fmt_tokens(n: u64) -> String {
    if n >= 10_000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        format!("{n}")
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_panel_empty() {
        let panel = SidePanel::new();
        assert!(panel.plan_steps.is_empty());
        assert_eq!(panel.metrics.round, 0);
        assert_eq!(panel.metrics.total_cost, 0.0);
    }

    #[test]
    fn panel_metrics_update_from_round_ended() {
        let mut panel = SidePanel::new();
        panel.update_metrics(1, 500, 200, 0.003, 1500);
        assert_eq!(panel.metrics.round, 1);
        assert_eq!(panel.metrics.total_tokens, 700);
        assert!((panel.metrics.total_cost - 0.003).abs() < f64::EPSILON);
    }

    #[test]
    fn panel_plan_update_from_plan_progress() {
        let mut panel = SidePanel::new();
        let steps = vec![
            PlanStepStatus {
                description: "Read file".into(),
                tool_name: Some("file_read".into()),
                status: PlanStepDisplayStatus::Succeeded,
                duration_ms: Some(100),
            },
            PlanStepStatus {
                description: "Edit file".into(),
                tool_name: Some("file_edit".into()),
                status: PlanStepDisplayStatus::InProgress,
                duration_ms: None,
            },
        ];
        panel.update_plan(steps, 1);
        assert_eq!(panel.plan_steps.len(), 2);
        assert_eq!(panel.current_step, 1);
    }

    #[test]
    fn fmt_tokens_formatting() {
        assert_eq!(fmt_tokens(42), "42");
        assert_eq!(fmt_tokens(10_000), "10.0k");
        assert_eq!(fmt_tokens(100_000), "100.0k");
    }

    #[test]
    fn fmt_elapsed_formatting() {
        assert_eq!(fmt_elapsed(500), "500ms");
        assert_eq!(fmt_elapsed(2500), "2.5s");
        assert_eq!(fmt_elapsed(90_000), "1m30s");
    }

    // Phase 43B: Verify panel uses palette tokens
    #[test]
    fn panel_uses_palette_colors() {
        let p = &theme::active().palette;
        // Verify all cockpit tokens used in panel are loadable.
        let _s = p.success_ratatui();
        let _e = p.error_ratatui();
        let _r = p.running_ratatui();
        let _m = p.muted_ratatui();
        let _b = p.border_ratatui();
        let _tl = p.text_label_ratatui();
        let _bp = p.bg_panel_ratatui();
    }

    // --- Phase 43D: Live panel data tests ---

    #[test]
    fn panel_context_update_sets_real_data() {
        let mut panel = SidePanel::new();
        panel.update_context(500, 2000, 300, 5, 10, 8, 3, 1200);
        assert_eq!(panel.context_tiers.l0_tokens, 500);
        assert_eq!(panel.context_tiers.l0_capacity, 2000);
        assert_eq!(panel.context_tiers.l1_tokens, 300);
        assert_eq!(panel.context_tiers.l1_entries, 5);
        assert_eq!(panel.context_tiers.l2_entries, 10);
        assert_eq!(panel.context_tiers.l3_entries, 8);
        assert_eq!(panel.context_tiers.l4_entries, 3);
        assert_eq!(panel.context_tiers.total_tokens, 1200);
    }

    #[test]
    fn panel_context_computes_percentages() {
        let mut panel = SidePanel::new();
        panel.update_context(600, 2000, 400, 5, 0, 0, 0, 1000);
        assert_eq!(panel.context_tiers.l0_pct, 60);
        assert_eq!(panel.context_tiers.l1_pct, 40);
    }

    #[test]
    fn panel_context_zero_total_no_division_error() {
        let mut panel = SidePanel::new();
        panel.update_context(0, 2000, 0, 0, 0, 0, 0, 0);
        assert_eq!(panel.context_tiers.l0_pct, 0);
        assert_eq!(panel.context_tiers.l1_pct, 0);
    }

    #[test]
    fn panel_reasoning_update() {
        let mut panel = SidePanel::new();
        assert!(panel.reasoning.strategy.is_empty());
        panel.update_reasoning(
            "PlanExecuteReflect".into(),
            "CodeModification".into(),
            "Complex".into(),
        );
        assert_eq!(panel.reasoning.strategy, "PlanExecuteReflect");
        assert_eq!(panel.reasoning.task_type, "CodeModification");
        assert_eq!(panel.reasoning.complexity, "Complex");
    }

    #[test]
    fn panel_reasoning_empty_when_new() {
        let panel = SidePanel::new();
        assert!(panel.reasoning.strategy.is_empty());
        assert!(panel.reasoning.task_type.is_empty());
        assert!(panel.reasoning.complexity.is_empty());
    }

    // --- Phase B3: Cache stats tests ---

    #[test]
    fn panel_cache_defaults_to_zero() {
        let panel = SidePanel::new();
        assert_eq!(panel.metrics.cache_hits, 0);
        assert_eq!(panel.metrics.cache_misses, 0);
    }

    #[test]
    fn panel_record_cache_hit() {
        let mut panel = SidePanel::new();
        panel.record_cache(true);
        panel.record_cache(true);
        panel.record_cache(false);
        assert_eq!(panel.metrics.cache_hits, 2);
        assert_eq!(panel.metrics.cache_misses, 1);
    }

    #[test]
    fn panel_cache_hit_rate() {
        let mut panel = SidePanel::new();
        for _ in 0..7 {
            panel.record_cache(true);
        }
        for _ in 0..3 {
            panel.record_cache(false);
        }
        let total = panel.metrics.cache_hits + panel.metrics.cache_misses;
        let rate = (panel.metrics.cache_hits as f64 / total as f64) * 100.0;
        assert!((rate - 70.0).abs() < f64::EPSILON);
    }

    // --- Phase B4: Circuit breaker tests ---

    #[test]
    fn panel_breakers_empty_by_default() {
        let panel = SidePanel::new();
        assert!(panel.breakers.is_empty());
    }

    #[test]
    fn panel_update_breaker_inserts() {
        let mut panel = SidePanel::new();
        panel.update_breaker("anthropic".into(), CircuitBreakerState::Open, 5);
        assert_eq!(panel.breakers.len(), 1);
        assert_eq!(panel.breakers[0].provider, "anthropic");
        assert_eq!(panel.breakers[0].state, CircuitBreakerState::Open);
        assert_eq!(panel.breakers[0].failure_count, 5);
    }

    #[test]
    fn panel_update_breaker_updates_existing() {
        let mut panel = SidePanel::new();
        panel.update_breaker("anthropic".into(), CircuitBreakerState::Open, 3);
        panel.update_breaker("anthropic".into(), CircuitBreakerState::HalfOpen, 3);
        assert_eq!(panel.breakers.len(), 1);
        assert_eq!(panel.breakers[0].state, CircuitBreakerState::HalfOpen);
    }

    #[test]
    fn panel_multiple_breakers() {
        let mut panel = SidePanel::new();
        panel.update_breaker("anthropic".into(), CircuitBreakerState::Open, 5);
        panel.update_breaker("deepseek".into(), CircuitBreakerState::Closed, 0);
        assert_eq!(panel.breakers.len(), 2);
    }

    // METRICS FIX TEST: Verify L2-L4 percentages are computed
    #[test]
    fn panel_context_l2_l4_percentages_computed() {
        let mut panel = SidePanel::new();

        // L0: 100 tokens, L1: 50 tokens
        // L2: 10 entries * 200 = 2000 tokens estimated
        // L3: 5 entries * 150 = 750 tokens estimated
        // L4: 2 entries * 100 = 200 tokens estimated
        // Total: 100 + 50 + 2000 + 750 + 200 = 3100 tokens
        panel.update_context(
            100,  // l0_tokens
            500,  // l0_capacity
            50,   // l1_tokens
            5,    // l1_entries
            10,   // l2_entries
            5,    // l3_entries
            2,    // l4_entries
            3100, // total_tokens
        );

        // Verify L0 and L1 percentages (existing behavior)
        assert!(
            panel.context_tiers.l0_pct > 0,
            "L0 percentage should be computed"
        );
        assert!(
            panel.context_tiers.l1_pct > 0,
            "L1 percentage should be computed"
        );

        // FIX VERIFICATION: L2-L4 percentages should now be computed (was always 0 before)
        assert!(
            panel.context_tiers.l2_pct > 0,
            "L2 percentage should be computed (FIX)"
        );
        assert!(
            panel.context_tiers.l3_pct > 0,
            "L3 percentage should be computed (FIX)"
        );
        assert!(
            panel.context_tiers.l4_pct > 0,
            "L4 percentage should be computed (FIX)"
        );

        // Verify approximate distribution (L2 should be ~64%, L3 ~24%, L4 ~6%)
        assert!(
            panel.context_tiers.l2_pct >= 60 && panel.context_tiers.l2_pct <= 70,
            "L2 should be ~64% (2000/3100)"
        );
        assert!(
            panel.context_tiers.l3_pct >= 20 && panel.context_tiers.l3_pct <= 30,
            "L3 should be ~24% (750/3100)"
        );
        assert!(
            panel.context_tiers.l4_pct >= 5 && panel.context_tiers.l4_pct <= 10,
            "L4 should be ~6% (200/3100)"
        );
    }
}
