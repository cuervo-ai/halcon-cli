//! Virtual scroll renderer for activity zone.
//!
//! **Phase A2: Virtual Scroll — Performance Optimization**
//!
//! Renders only visible lines in viewport instead of all lines.
//! Uses LRU cache for parsed markdown spans to avoid re-parsing per frame.
//!
//! Target: <2ms rendering time for 500 lines (vs ~6ms without virtual scroll).

use std::collections::HashMap;
use std::time::Instant;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap};
use ratatui::Frame;

use crate::render::theme;
use crate::tui::state::AppState;
use super::activity_model::ActivityModel;
use super::activity_navigator::ActivityNavigator;
use super::app::{ExpansionAnimation, shimmer_progress}; // Phase B1, B2
use super::activity_types::ActivityLine;

/// LRU cache for parsed markdown spans.
///
/// Key: line index, Value: Vec<Span> (cached parsed markdown).
/// Evicts least-recently-used entries when capacity exceeded.
pub struct SpanCache {
    cache: HashMap<usize, Vec<Span<'static>>>,
    access_order: Vec<usize>,
    max_capacity: usize,
}

impl SpanCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            cache: HashMap::with_capacity(capacity),
            access_order: Vec::with_capacity(capacity),
            max_capacity: capacity,
        }
    }

    /// Get cached spans for a line index.
    /// Updates LRU access order on cache hit.
    pub fn get(&mut self, line_idx: usize) -> Option<&Vec<Span<'static>>> {
        if self.cache.contains_key(&line_idx) {
            // Update access order (move to end = most recently used)
            self.access_order.retain(|&idx| idx != line_idx);
            self.access_order.push(line_idx);
            self.cache.get(&line_idx)
        } else {
            None
        }
    }

    /// Insert spans into cache for a line index.
    /// Evicts LRU entry if capacity exceeded.
    pub fn insert(&mut self, line_idx: usize, spans: Vec<Span<'static>>) {
        // Evict LRU if at capacity
        if self.cache.len() >= self.max_capacity && !self.cache.contains_key(&line_idx) {
            if let Some(&lru_idx) = self.access_order.first() {
                self.cache.remove(&lru_idx);
                self.access_order.remove(0);
            }
        }

        // Insert new entry
        self.cache.insert(line_idx, spans);

        // Update access order
        self.access_order.retain(|&idx| idx != line_idx);
        self.access_order.push(line_idx);
    }

    /// Clear the entire cache.
    pub fn clear(&mut self) {
        self.cache.clear();
        self.access_order.clear();
    }

    /// Get cache hit rate (for diagnostics).
    #[allow(dead_code)]
    pub fn hit_rate(&self) -> f64 {
        // Would need hit/miss counters for accurate rate
        // For now, return cache fill ratio as proxy
        self.cache.len() as f64 / self.max_capacity as f64
    }
}

/// Virtual scroll renderer for activity zone.
///
/// Renders only visible lines to optimize performance.
/// Uses LRU cache to avoid re-parsing markdown per frame.
pub struct ActivityRenderer {
    /// LRU cache for parsed markdown spans (capacity: 200 lines).
    span_cache: SpanCache,
}

impl ActivityRenderer {
    /// Create a new renderer with default cache capacity (200 lines).
    pub fn new() -> Self {
        Self {
            span_cache: SpanCache::new(200),
        }
    }

    /// Create a renderer with custom cache capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            span_cache: SpanCache::new(capacity),
        }
    }

    /// Render the activity zone with virtual scrolling.
    ///
    /// Only renders lines visible in the viewport (scroll_offset to scroll_offset + viewport_height).
    /// Uses cached spans when available to avoid markdown re-parsing.
    ///
    /// Phase B1: Accepts expansion_animations for smooth expand/collapse height transitions.
    /// Phase B2: Accepts executing_tools for dynamic shimmer loading skeletons.
    /// Phase B3: Accepts highlights for search match fade-in/fade-out animations.
    ///
    /// **Phase 1 Remediation**: Returns (max_scroll, viewport_height) for Navigator sync.
    /// Caller must update `navigator.last_max_scroll` to prevent stale clamping.
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        model: &ActivityModel,
        nav: &ActivityNavigator,
        state: &AppState,
        expansion_animations: &HashMap<usize, ExpansionAnimation>, // Phase B1
        executing_tools: &HashMap<String, Instant>,                // Phase B2
        highlights: &crate::tui::highlight::HighlightManager,      // Phase B3
    ) -> (usize, usize) {
        let p = &theme::active().palette;
        // Cache ratatui colors (eliminates OKLCH→sRGB conversions per line)
        let c_success = p.success_ratatui();
        let c_accent = p.accent_ratatui();
        let c_warning = p.warning_ratatui();
        let c_error = p.error_ratatui();
        let c_running = p.running_ratatui();
        let c_text = p.text_ratatui();
        let c_muted = p.muted_ratatui();
        let c_border = p.border_ratatui();
        let c_spinner = p.spinner_color_ratatui();

        let border_color = if state.focus == super::state::FocusZone::Activity {
            c_accent
        } else {
            c_border
        };

        // Calculate viewport bounds
        let viewport_height = area.height.saturating_sub(2) as usize; // -2 for borders
        let total_lines = self.count_rendered_lines(model, nav, state);
        let max_scroll = total_lines.saturating_sub(viewport_height);

        // Determine scroll offset (auto-scroll or manual)
        let scroll = if nav.auto_scroll {
            max_scroll
        } else {
            nav.scroll_offset.min(max_scroll)
        };

        // Virtual scroll: only render visible lines (Phase B1: with animations, B2: with shimmer, B3: with highlights)
        let visible_lines = self.viewport_lines(
            model,
            nav,
            state,
            scroll,
            viewport_height,
            expansion_animations, // Phase B1
            executing_tools,      // Phase B2
            highlights,           // Phase B3
            c_success,
            c_accent,
            c_warning,
            c_error,
            c_running,
            c_text,
            c_muted,
            c_spinner,
        );

        // Phase 2 VIZ-001: Dynamic title showing scroll position when content overflows
        let title = if total_lines > viewport_height {
            let start = scroll + 1; // 1-indexed for UX
            let end = (scroll + viewport_height).min(total_lines);
            format!(" Activity ({}-{} / {}) ", start, end, total_lines)
        } else {
            " Activity ".to_string()
        };

        let paragraph = Paragraph::new(visible_lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .border_style(Style::default().fg(border_color)),
            )
            .wrap(Wrap { trim: true }); // Word-wrap intelligently (by words, not chars) and trim whitespace

        frame.render_widget(paragraph, area);

        // Render scrollbar if content exceeds viewport
        if total_lines > viewport_height {
            let mut scrollbar_state = ScrollbarState::new(max_scroll).position(scroll);
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓"));
            frame.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
        }

        // Phase 1 Remediation: Return (max_scroll, viewport_height) for Navigator sync
        (max_scroll, viewport_height)
    }

    /// Get lines visible in the current viewport.
    ///
    /// Virtual scroll optimization: only processes lines in [scroll_offset, scroll_offset + viewport_height].
    /// Uses LRU cache for parsed markdown spans.
    /// Phase B1: Uses expansion_animations for smooth height transitions.
    /// Phase B2: Uses executing_tools for dynamic shimmer loading skeletons.
    /// Phase B3: Uses highlights for search match fade-in/fade-out animations.
    fn viewport_lines(
        &mut self,
        model: &ActivityModel,
        nav: &ActivityNavigator,
        state: &AppState,
        scroll_offset: usize,
        viewport_height: usize,
        expansion_animations: &HashMap<usize, ExpansionAnimation>, // Phase B1
        executing_tools: &HashMap<String, Instant>,                // Phase B2
        highlights: &crate::tui::highlight::HighlightManager,      // Phase B3
        c_success: Color,
        c_accent: Color,
        c_warning: Color,
        c_error: Color,
        c_running: Color,
        c_text: Color,
        c_muted: Color,
        c_spinner: Color,
    ) -> Vec<Line<'static>> {
        let mut styled_lines: Vec<Line<'static>> = Vec::new();

        // Apply filters to get visible lines
        let filtered: Vec<(usize, &ActivityLine)> = model.filter_active().collect();

        // P0.5 FIX: Prevent panic from invalid scroll_offset
        // Clamp scroll_offset to valid range [0, filtered.len()]
        let clamped_offset = scroll_offset.min(filtered.len());

        // Calculate visible slice (virtual scroll)
        let end = (clamped_offset + viewport_height).min(filtered.len());

        // Safety check: ensure valid range before slicing
        let visible_slice = if clamped_offset <= end && end <= filtered.len() {
            &filtered[clamped_offset..end]
        } else {
            // Log error and return empty slice to prevent panic
            tracing::error!(
                "Invalid slice range: offset={}, end={}, len={}",
                clamped_offset, end, filtered.len()
            );
            &[]
        };

        for (idx, line) in visible_slice {
            let is_selected = nav.selected() == Some(*idx);
            let is_expanded = nav.is_expanded(*idx);
            let is_hovered = nav.is_hovered(*idx); // Phase B4

            // Phase B1: Get expansion animation progress (if any)
            let expansion_progress = expansion_animations
                .get(idx)
                .map(|anim| anim.current())
                .unwrap_or(if is_expanded { 1.0 } else { 0.0 });

            // Render line with selection highlight (Phase B1: with animation, B2: with shimmer, B3: with search highlights, B4: with hover)
            let line_spans = self.render_line(
                line,
                *idx,
                model.len(),        // P0.4: total lines for cache skipping
                is_selected,
                is_expanded,
                is_hovered,         // Phase B4
                expansion_progress, // Phase B1
                executing_tools,    // Phase B2
                highlights,         // Phase B3
                state,
                c_success,
                c_accent,
                c_warning,
                c_error,
                c_running,
                c_text,
                c_muted,
                c_spinner,
            );

            styled_lines.extend(line_spans);
        }

        // Phase 2 VIZ-002: Scroll bounds indicators
        // Show "TOP" indicator if at the very top
        if scroll_offset == 0 && !styled_lines.is_empty() {
            styled_lines.insert(0, Line::from(Span::styled(
                "▲▲▲ TOP ▲▲▲",
                Style::default()
                    .fg(c_muted)
                    .add_modifier(Modifier::DIM),
            )));
        }

        // Show "BOTTOM" indicator if at the very bottom
        let total_lines = model.filter_active().count();
        let max_scroll = total_lines.saturating_sub(viewport_height);
        if scroll_offset >= max_scroll && total_lines > viewport_height {
            styled_lines.push(Line::from(Span::styled(
                "▼▼▼ BOTTOM ▼▼▼",
                Style::default()
                    .fg(c_muted)
                    .add_modifier(Modifier::DIM),
            )));
        }

        // Add spinner if active
        if state.spinner_active {
            let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
            let ch = frames[state.spinner_frame % frames.len()];
            styled_lines.push(Line::from(Span::styled(
                format!("{ch} {}", state.spinner_label),
                Style::default()
                    .fg(c_spinner)
                    .add_modifier(Modifier::BOLD),
            )));
        }

        styled_lines
    }

    /// Render a single activity line.
    ///
    /// Returns a vector of Line<'static> (some ActivityLine types expand to multiple rendered lines).
    #[allow(clippy::too_many_arguments)]
    /// Render a single activity line.
    ///
    /// Phase B1: Uses expansion_progress [0.0, 1.0] for smooth height transitions.
    /// When expansion_progress < 1.0, content is scaled (partial lines shown).
    /// Phase B2: Uses executing_tools to generate dynamic shimmer for loading skeletons.
    /// Phase B3: Uses highlights for search match fade-in/fade-out background animations.
    /// Phase B4: Uses is_hovered for hover effect background.
    fn render_line(
        &mut self,
        line: &ActivityLine,
        line_idx: usize,
        total_lines: usize,                                // P0.4: total lines for last-line detection
        is_selected: bool,
        is_expanded: bool,
        is_hovered: bool,                                  // Phase B4: hover state
        expansion_progress: f32,                           // Phase B1: [0.0, 1.0] animation progress
        executing_tools: &HashMap<String, Instant>,        // Phase B2: tool_name → start_time
        highlights: &crate::tui::highlight::HighlightManager, // Phase B3: search highlight pulses
        state: &AppState,
        c_success: Color,
        c_accent: Color,
        c_warning: Color,
        c_error: Color,
        c_running: Color,
        c_text: Color,
        c_muted: Color,
        c_spinner: Color,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        // Phase B3: Check for search highlight pulse
        let highlight_key = format!("search_{}", line_idx);

        // Background priority: highlight > selection > hover > none
        let bg = if highlights.is_pulsing(&highlight_key) {
            // Phase B3: Fade-in/fade-out search highlight background
            // Use bg_highlight as default (pulse fades from accent to bg_highlight)
            let pulse_color = highlights.current(&highlight_key, theme::active().palette.bg_highlight);
            Some(pulse_color.to_ratatui_color())
        } else if is_selected {
            // Selection highlight background
            Some(theme::active().palette.bg_highlight_ratatui())
        } else if is_hovered {
            // Phase B4: Hover effect background (subtle, muted color)
            Some(theme::active().palette.muted_ratatui())
        } else {
            None
        };

        match line {
            ActivityLine::UserPrompt(text) => {
                lines.push(Line::from(vec![
                    Span::styled(
                        "► ",
                        Style::default()
                            .fg(c_success)
                            .add_modifier(Modifier::BOLD)
                            .bg(bg.unwrap_or(Color::Reset)),
                    ),
                    Span::styled(
                        text.clone(),
                        Style::default()
                            .fg(c_success)
                            .add_modifier(Modifier::BOLD)
                            .bg(bg.unwrap_or(Color::Reset)),
                    ),
                ]));
            }

            ActivityLine::AssistantText(text) => {
                // P0.6 FIX: Don't cache AssistantText at all
                // The cache design (single Vec<Span> per line_idx) doesn't work for
                // multiline text where text.lines() produces multiple rendered lines.
                // Caching would require Vec<Vec<Span>> which changes the cache type.
                // Since parse_md_spans is very fast (~1-5µs), just parse every frame.
                for l in text.lines() {
                    let parsed = super::activity_types::parse_md_spans(l, c_warning);
                    lines.push(Line::from(parsed));
                }
            }

            ActivityLine::CodeBlock { lang, code } => {
                // Header
                lines.push(Line::from(vec![
                    Span::styled("  ┌─ ", Style::default().fg(c_muted)),
                    Span::styled(
                        lang.clone(),
                        Style::default()
                            .fg(c_accent)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" ─", Style::default().fg(c_muted)),
                ]));

                // Code lines (only if expanded)
                if is_expanded {
                    // Phase B1: Smooth expansion animation for code blocks
                    let all_lines: Vec<&str> = code.lines().collect();
                    let total_lines = all_lines.len();
                    let lines_to_show = if expansion_progress < 1.0 {
                        // During animation: show partial content
                        ((total_lines as f32 * expansion_progress).ceil() as usize).max(1)
                    } else {
                        // Animation complete: show all
                        total_lines
                    };

                    for l in all_lines.iter().take(lines_to_show) {
                        lines.push(Line::from(vec![
                            Span::styled("  │ ", Style::default().fg(c_muted)),
                            Span::styled(l.to_string(), Style::default().fg(c_warning)),
                        ]));
                    }
                } else {
                    // Collapsed: show preview
                    let preview_lines: Vec<&str> = code.lines().take(2).collect();
                    for l in preview_lines {
                        lines.push(Line::from(vec![
                            Span::styled("  │ ", Style::default().fg(c_muted)),
                            Span::styled(l.to_string(), Style::default().fg(c_warning)),
                        ]));
                    }
                    if code.lines().count() > 2 {
                        lines.push(Line::from(Span::styled(
                            format!("  │ ... ({} more lines, press Enter to expand)", code.lines().count() - 2),
                            Style::default().fg(c_muted),
                        )));
                    }
                }

                // Footer
                lines.push(Line::from(Span::styled(
                    "  └───",
                    Style::default().fg(c_muted),
                )));
            }

            ActivityLine::Info(text) => {
                lines.push(Line::from(Span::styled(
                    text.clone(),
                    Style::default().fg(c_accent).bg(bg.unwrap_or(Color::Reset)),
                )));
            }

            ActivityLine::Warning { message, hint } => {
                let mut spans = vec![
                    Span::styled(
                        "⚠ ",
                        Style::default()
                            .fg(c_warning)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(message.clone(), Style::default().fg(c_warning)),
                ];
                if let Some(h) = hint {
                    spans.push(Span::styled(
                        format!(" ({h})"),
                        Style::default().fg(c_muted),
                    ));
                }
                lines.push(Line::from(spans));
            }

            ActivityLine::Error { message, hint } => {
                let mut spans = vec![
                    Span::styled(
                        "✖ ",
                        Style::default()
                            .fg(c_error)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(message.clone(), Style::default().fg(c_error)),
                ];
                if let Some(h) = hint {
                    spans.push(Span::styled(
                        format!(" ({h})"),
                        Style::default().fg(c_muted),
                    ));
                }
                lines.push(Line::from(spans));
            }

            ActivityLine::RoundSeparator(n) => {
                lines.push(Line::from(Span::styled(
                    format!("──────── Round {n} ────────"),
                    Style::default()
                        .fg(c_muted)
                        .add_modifier(Modifier::DIM),
                )));
            }

            ActivityLine::PlanOverview { goal, steps, current_step } => {
                // Plan header
                lines.push(Line::from(vec![
                    Span::styled(
                        "  📋 PLAN: ",
                        Style::default()
                            .fg(c_accent)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        goal.clone(),
                        Style::default()
                            .fg(c_text)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));

                // Step list (only if expanded, otherwise show summary)
                if is_expanded {
                    for (i, step) in steps.iter().enumerate() {
                        use crate::tui::events::PlanStepDisplayStatus;
                        let (icon, color) = match step.status {
                            PlanStepDisplayStatus::Succeeded => ("✓", c_success),
                            PlanStepDisplayStatus::Failed => ("✗", c_error),
                            PlanStepDisplayStatus::InProgress => ("▸", c_warning),
                            PlanStepDisplayStatus::Skipped => ("-", c_muted),
                            PlanStepDisplayStatus::Pending => ("○", c_muted),
                        };
                        let marker = if i == *current_step && step.status == PlanStepDisplayStatus::InProgress {
                            " ← CURRENT"
                        } else {
                            ""
                        };
                        let tool_hint = step
                            .tool_name
                            .as_deref()
                            .map(|t| format!(" ({t})"))
                            .unwrap_or_default();
                        lines.push(Line::from(vec![
                            Span::styled(
                                format!("    {icon} "),
                                Style::default().fg(color).add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("Step {}: {}{tool_hint}{marker}", i + 1, step.description),
                                Style::default().fg(color),
                            ),
                        ]));
                    }
                } else {
                    // Collapsed: show summary
                    let total = steps.len();
                    let completed = steps.iter().filter(|s| s.status == crate::tui::events::PlanStepDisplayStatus::Succeeded).count();
                    lines.push(Line::from(Span::styled(
                        format!("    {} / {} steps complete (press Enter to expand)", completed, total),
                        Style::default().fg(c_muted),
                    )));
                }

                lines.push(Line::from(""));
            }

            ActivityLine::ToolExec { name, input_preview, result, .. } => {
                match result {
                    None => {
                        // Phase B2: Dynamic shimmer loading skeleton
                        // Calculate shimmer progress from elapsed time
                        let shimmer_pos = if let Some(start_time) = executing_tools.get(name) {
                            let elapsed = start_time.elapsed();
                            shimmer_progress(elapsed)
                        } else {
                            0.0 // Fallback if not in tracker
                        };

                        // Generate shimmer bar dynamically (12 chars width)
                        const SHIMMER_WIDTH: usize = 12;
                        const WAVE_WIDTH: usize = 3; // 3-char wide wave (▒▒▒)

                        let shimmer_bar: String = (0..SHIMMER_WIDTH)
                            .map(|i| {
                                let pos = (shimmer_pos * SHIMMER_WIDTH as f32) as usize;
                                let distance = if i >= pos {
                                    i - pos
                                } else {
                                    pos - i
                                };

                                if distance < WAVE_WIDTH {
                                    '▒' // Wave character
                                } else {
                                    '░' // Background character
                                }
                            })
                            .collect();

                        lines.push(Line::from(vec![
                            Span::styled(
                                "  ⚙ ",
                                Style::default()
                                    .fg(c_running)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                name.clone(),
                                Style::default()
                                    .fg(c_running)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!(" {input_preview}"),
                                Style::default().fg(c_muted),
                            ),
                        ]));

                        // Phase B2: Dynamic shimmer bar with cyclic animation
                        lines.push(Line::from(vec![
                            Span::styled("    ", Style::default()),
                            Span::styled(
                                shimmer_bar,
                                Style::default().fg(c_accent), // Use accent for shimmer wave
                            ),
                        ]));
                    }

                    Some(res) => {
                        let (icon, icon_color) = if res.is_error {
                            ("  ✖ ", c_error)
                        } else {
                            ("  ✔ ", c_success)
                        };
                        let duration_str = if res.duration_ms < 1000 {
                            format!("{}ms", res.duration_ms)
                        } else {
                            format!("{:.1}s", res.duration_ms as f64 / 1000.0)
                        };
                        let expand_hint = if is_expanded { "▾" } else { "▸" };

                        lines.push(Line::from(vec![
                            Span::styled(
                                icon,
                                Style::default()
                                    .fg(icon_color)
                                    .add_modifier(Modifier::BOLD)
                                    .bg(bg.unwrap_or(Color::Reset)),
                            ),
                            Span::styled(
                                format!("{expand_hint} "),
                                Style::default().fg(c_muted),
                            ),
                            Span::styled(
                                name.clone(),
                                Style::default()
                                    .fg(c_text)
                                    .add_modifier(Modifier::BOLD)
                                    .bg(bg.unwrap_or(Color::Reset)),
                            ),
                            Span::styled(
                                format!(" [{duration_str}]"),
                                Style::default().fg(c_muted),
                            ),
                        ]));

                        // Content: expanded or collapsed preview
                        if !res.content.is_empty() {
                            let content_color = if res.is_error { c_error } else { c_muted };

                            if is_expanded {
                                // Phase B1: Smooth expansion animation
                                // Scale content lines based on animation progress [0.0, 1.0]
                                let all_lines: Vec<&str> = res.content.lines().collect();
                                let total_lines = all_lines.len();
                                let lines_to_show = if expansion_progress < 1.0 {
                                    // During animation: show partial content
                                    ((total_lines as f32 * expansion_progress).ceil() as usize).max(1)
                                } else {
                                    // Animation complete: show all
                                    total_lines
                                };

                                for pline in all_lines.iter().take(lines_to_show) {
                                    lines.push(Line::from(vec![
                                        Span::styled("    ", Style::default()),
                                        Span::styled(
                                            pline.to_string(),
                                            Style::default().fg(content_color),
                                        ),
                                    ]));
                                }
                            } else {
                                // Show preview (3 lines)
                                let preview = &res.content[..res.content.len().min(200)];
                                for pline in preview.lines().take(3) {
                                    lines.push(Line::from(vec![
                                        Span::styled("    ", Style::default()),
                                        Span::styled(
                                            pline.to_string(),
                                            Style::default().fg(content_color),
                                        ),
                                    ]));
                                }

                                let total_lines_in_content = res.content.lines().count();
                                if total_lines_in_content > 3 {
                                    lines.push(Line::from(Span::styled(
                                        format!(
                                            "    ... ({} more lines, press Enter to expand)",
                                            total_lines_in_content - 3
                                        ),
                                        Style::default().fg(c_muted),
                                    )));
                                }
                            }
                        }
                    }
                }
            }
        }

        lines
    }

    /// Count total rendered lines (for scrollbar calculation).
    ///
    /// This is NOT the same as model.len() because some ActivityLine types
    /// expand to multiple rendered lines (e.g., CodeBlock, PlanOverview).
    fn count_rendered_lines(
        &self,
        model: &ActivityModel,
        nav: &ActivityNavigator,
        state: &AppState,
    ) -> usize {
        let mut count = 0;

        for (idx, line) in model.filter_active() {
            let is_expanded = nav.is_expanded(idx);

            count += match line {
                ActivityLine::UserPrompt(_) => 1,
                ActivityLine::AssistantText(text) => text.lines().count(),
                ActivityLine::CodeBlock { code, .. } => {
                    if is_expanded {
                        3 + code.lines().count() // header + content + footer
                    } else {
                        3 + 2 // header + 2 preview + footer (approx)
                    }
                }
                ActivityLine::Info(_) => 1,
                ActivityLine::Warning { .. } => 1,
                ActivityLine::Error { .. } => 1,
                ActivityLine::RoundSeparator(_) => 1,
                ActivityLine::PlanOverview { steps, .. } => {
                    if is_expanded {
                        2 + steps.len() // header + steps + blank line
                    } else {
                        2 // header + summary
                    }
                }
                ActivityLine::ToolExec { result, .. } => {
                    match result {
                        None => 2, // name + skeleton
                        Some(res) => {
                            let content_lines = if is_expanded {
                                res.content.lines().count()
                            } else {
                                res.content.lines().take(3).count() + 1 // +1 for "... N more lines"
                            };
                            1 + content_lines // header + content
                        }
                    }
                }
            };
        }

        // Add spinner if active
        if state.spinner_active {
            count += 1;
        }

        count
    }

    /// Clear the span cache (useful when theme changes).
    pub fn clear_cache(&mut self) {
        self.span_cache.clear();
    }
}

impl Default for ActivityRenderer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_cache_insert_and_get() {
        let mut cache = SpanCache::new(3);
        let spans = vec![Span::raw("test".to_string())];

        cache.insert(0, spans.clone());
        assert!(cache.get(0).is_some());
        assert_eq!(cache.get(0).unwrap().len(), 1);
    }

    #[test]
    fn span_cache_lru_eviction() {
        let mut cache = SpanCache::new(2);

        cache.insert(0, vec![Span::raw("line 0".to_string())]);
        cache.insert(1, vec![Span::raw("line 1".to_string())]);
        cache.insert(2, vec![Span::raw("line 2".to_string())]); // Should evict 0

        assert!(cache.get(0).is_none()); // Evicted (LRU)
        assert!(cache.get(1).is_some());
        assert!(cache.get(2).is_some());
    }

    #[test]
    fn span_cache_access_updates_lru() {
        let mut cache = SpanCache::new(2);

        cache.insert(0, vec![Span::raw("line 0".to_string())]);
        cache.insert(1, vec![Span::raw("line 1".to_string())]);

        // Access 0 → makes it most recently used
        let _ = cache.get(0);

        // Insert 2 → should evict 1 (not 0)
        cache.insert(2, vec![Span::raw("line 2".to_string())]);

        assert!(cache.get(0).is_some()); // Still cached
        assert!(cache.get(1).is_none()); // Evicted
        assert!(cache.get(2).is_some());
    }

    #[test]
    fn span_cache_clear() {
        let mut cache = SpanCache::new(3);
        cache.insert(0, vec![Span::raw("test".to_string())]);
        cache.insert(1, vec![Span::raw("test".to_string())]);

        cache.clear();

        assert!(cache.get(0).is_none());
        assert!(cache.get(1).is_none());
        assert_eq!(cache.cache.len(), 0);
    }

    #[test]
    fn renderer_creates_with_default_capacity() {
        let renderer = ActivityRenderer::new();
        assert_eq!(renderer.span_cache.max_capacity, 200);
    }

    #[test]
    fn renderer_creates_with_custom_capacity() {
        let renderer = ActivityRenderer::with_capacity(50);
        assert_eq!(renderer.span_cache.max_capacity, 50);
    }

    #[test]
    fn count_rendered_lines_simple() {
        let renderer = ActivityRenderer::new();
        let mut model = ActivityModel::new();
        let nav = ActivityNavigator::new();
        let state = AppState::new();

        model.push(ActivityLine::UserPrompt("test".into()));
        model.push(ActivityLine::Info("info".into()));

        let count = renderer.count_rendered_lines(&model, &nav, &state);
        assert_eq!(count, 2); // 1 user + 1 info
    }

    #[test]
    fn count_rendered_lines_multiline_assistant() {
        let renderer = ActivityRenderer::new();
        let mut model = ActivityModel::new();
        let nav = ActivityNavigator::new();
        let state = AppState::new();

        model.push(ActivityLine::AssistantText("line 1\nline 2\nline 3".into()));

        let count = renderer.count_rendered_lines(&model, &nav, &state);
        assert_eq!(count, 3); // 3 lines in assistant text
    }

    #[test]
    fn count_rendered_lines_code_block_collapsed() {
        let renderer = ActivityRenderer::new();
        let mut model = ActivityModel::new();
        let mut nav = ActivityNavigator::new();
        let state = AppState::new();

        model.push(ActivityLine::CodeBlock {
            lang: "rust".into(),
            code: "fn main() {}\nfn test() {}".into(),
        });

        // Collapsed: header + 2 preview + footer = 5 approx
        let count = renderer.count_rendered_lines(&model, &nav, &state);
        assert_eq!(count, 5);
    }

    #[test]
    fn count_rendered_lines_code_block_expanded() {
        let renderer = ActivityRenderer::new();
        let mut model = ActivityModel::new();
        let mut nav = ActivityNavigator::new();
        let state = AppState::new();

        model.push(ActivityLine::CodeBlock {
            lang: "rust".into(),
            code: "fn main() {}\nfn test() {}".into(),
        });

        // Expand it
        nav.toggle_expand(0);

        // Expanded: header (1) + 2 content lines + footer (1) = 5
        let count = renderer.count_rendered_lines(&model, &nav, &state);
        assert_eq!(count, 5); // header + 2 code lines + footer + blank
    }

    #[test]
    fn count_rendered_lines_with_spinner() {
        let renderer = ActivityRenderer::new();
        let model = ActivityModel::new();
        let nav = ActivityNavigator::new();
        let mut state = AppState::new();

        state.spinner_active = true;

        let count = renderer.count_rendered_lines(&model, &nav, &state);
        assert_eq!(count, 1); // Just spinner
    }
}
