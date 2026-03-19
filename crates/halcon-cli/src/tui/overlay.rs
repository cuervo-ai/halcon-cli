//! Modal overlay system for the TUI.
//!
//! Overlays render as centered floating boxes on top of the main layout.
//! While an overlay is active, it captures all keyboard input.
//! Pressing Esc dismisses the active overlay.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::render::theme;
use crate::tui::constants;
use crate::tui::events::{PluginSuggestionItem, SessionInfo};

/// The kind of overlay currently displayed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayKind {
    /// Help overlay showing keybinding reference (F1).
    Help,
    /// Command palette with substring search (Ctrl+P).
    CommandPalette,
    /// Search overlay for activity zone (Ctrl+F).
    Search,
    /// Permission approval prompt for a tool.
    PermissionPrompt { tool: String },
    /// Context Servers management overlay (Ctrl+S).
    ContextServers,
    /// Session browser overlay (F6).
    SessionList,
    /// System password (sudo) entry — shown after permission approved for a sudo command.
    ///
    /// Provides masked password input (●●●●) with optional 5-minute session cache.
    SudoPasswordEntry { tool: String, command: String },
    /// Project onboarding wizard — multi-step HALCON.md generator.
    ///
    /// Steps:
    /// - 0 = analyzing project
    /// - 1 = review detected info
    /// - 2 = preview generated content
    /// - 3 = confirm save
    /// - 4 = done
    InitWizard {
        step: u8,
        preview: String,
        save_path: String,
        dry_run: bool,
    },
    /// Plugin recommendation overlay — tiered suggestion list.
    PluginSuggest {
        suggestions: Vec<PluginSuggestionItem>,
        selected: usize,
        dry_run: bool,
    },
    /// Update-available notification overlay.
    ///
    /// Shown at TUI startup when `get_pending_update_info()` returns Some.
    /// Enter = install now + quit; Esc = dismiss + toast reminder.
    UpdateAvailable {
        current: String,
        remote: String,
        notes: Option<String>,
        published_at: Option<String>,
        size_bytes: u64,
    },
}

/// State for the overlay system.
#[derive(Debug, Clone)]
pub struct OverlayState {
    /// Currently active overlay, or None.
    pub active: Option<OverlayKind>,
    /// Text input for search/command palette.
    pub input: String,
    /// Cursor position within input.
    pub cursor: usize,
    /// Selected index in a list (command palette).
    pub selected: usize,
    /// Filtered items for command palette.
    pub filtered_items: Vec<OverlayItem>,
    /// Whether advanced permission options are shown (Phase 6: Progressive disclosure).
    pub show_advanced_permissions: bool,
    /// Deadline for the active permission modal countdown (TUI-side enforcement).
    /// When `Some`, the TUI auto-denies at this instant and closes the modal.
    pub permission_deadline: Option<std::time::Instant>,
    /// Total seconds for the current permission countdown (for progress bar fraction).
    pub permission_total_secs: u64,
}

/// An item in the command palette list.
#[derive(Debug, Clone)]
pub struct OverlayItem {
    pub label: String,
    pub description: String,
    pub action: String,
}

impl OverlayState {
    pub fn new() -> Self {
        Self {
            active: None,
            input: String::new(),
            cursor: 0,
            selected: 0,
            filtered_items: Vec::new(),
            show_advanced_permissions: false,
            permission_deadline: None,
            permission_total_secs: 0,
        }
    }

    /// Open an overlay of the given kind.
    pub fn open(&mut self, kind: OverlayKind) {
        self.active = Some(kind);
        self.input.clear();
        self.cursor = 0;
        self.selected = 0;
        self.filtered_items.clear();
    }

    /// Close the active overlay.
    pub fn close(&mut self) {
        self.active = None;
        self.input.clear();
        self.cursor = 0;
        self.selected = 0;
        self.filtered_items.clear();
        self.permission_deadline = None;
        self.permission_total_secs = 0;
    }

    /// Set the permission modal countdown deadline (for external/programmatic use).
    ///
    /// In normal TUI interactive mode, `permission_deadline` is left as `None` so
    /// the agent waits indefinitely for user input. This method exists as an escape
    /// hatch for non-interactive scenarios where a hard deadline is needed.
    pub fn set_permission_deadline(&mut self, timeout_secs: u64) {
        if timeout_secs > 0 {
            self.permission_deadline = Some(
                std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs),
            );
            self.permission_total_secs = timeout_secs;
        }
    }

    /// Whether an overlay is currently active.
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// Type a character into the overlay input.
    pub fn type_char(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Delete the character before cursor.
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            let prev = self.input[..self.cursor]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.input.drain(prev..self.cursor);
            self.cursor = prev;
        }
    }

    /// Move selection up in a list.
    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down in a list.
    pub fn select_next(&mut self, max: usize) {
        if self.selected + 1 < max {
            self.selected += 1;
        }
    }
}

impl Default for OverlayState {
    fn default() -> Self {
        Self::new()
    }
}

/// Get the default set of command palette items.
pub fn default_commands() -> Vec<OverlayItem> {
    vec![
        // --- Agent Control ---
        OverlayItem {
            label: "/pause".into(),
            description: "Pause the running agent".into(),
            action: "pause".into(),
        },
        OverlayItem {
            label: "/resume".into(),
            description: "Resume a paused agent".into(),
            action: "resume".into(),
        },
        OverlayItem {
            label: "/step".into(),
            description: "Execute one agent step then pause".into(),
            action: "step".into(),
        },
        OverlayItem {
            label: "/cancel".into(),
            description: "Cancel the running agent".into(),
            action: "cancel".into(),
        },
        // --- Session Info ---
        OverlayItem {
            label: "/status".into(),
            description: "Show current provider, model, and session status".into(),
            action: "status".into(),
        },
        OverlayItem {
            label: "/session".into(),
            description: "Show session ID and connection info".into(),
            action: "session".into(),
        },
        OverlayItem {
            label: "/metrics".into(),
            description: "Show token usage and cost metrics".into(),
            action: "metrics".into(),
        },
        OverlayItem {
            label: "/context".into(),
            description: "Show context tier usage (L0-L4)".into(),
            action: "context".into(),
        },
        OverlayItem {
            label: "/cost".into(),
            description: "Show session cost breakdown".into(),
            action: "cost".into(),
        },
        OverlayItem {
            label: "/history".into(),
            description: "Show conversation history count".into(),
            action: "history".into(),
        },
        OverlayItem {
            label: "/why".into(),
            description: "Show current reasoning strategy".into(),
            action: "why".into(),
        },
        // --- UI Control ---
        OverlayItem {
            label: "/help".into(),
            description: "Show help and keybindings (F1)".into(),
            action: "help".into(),
        },
        OverlayItem {
            label: "/model".into(),
            description: "Show active provider and model info".into(),
            action: "model".into(),
        },
        OverlayItem {
            label: "/mode".into(),
            description: "Cycle UI mode (Minimal/Standard/Expert)".into(),
            action: "mode".into(),
        },
        OverlayItem {
            label: "/plan".into(),
            description: "Switch side panel to Plan view".into(),
            action: "plan".into(),
        },
        OverlayItem {
            label: "/panel".into(),
            description: "Toggle the side panel (F2)".into(),
            action: "panel".into(),
        },
        OverlayItem {
            label: "/search".into(),
            description: "Search activity text (Ctrl+F)".into(),
            action: "search".into(),
        },
        OverlayItem {
            label: "/clear".into(),
            description: "Clear the activity zone".into(),
            action: "clear".into(),
        },
        OverlayItem {
            label: "/quit".into(),
            description: "Exit the TUI".into(),
            action: "quit".into(),
        },
        // --- Extended / Setup ---
        OverlayItem {
            label: "/inspect".into(),
            description: "Inspect full session state (provider, model, cost, metrics)".into(),
            action: "inspect".into(),
        },
        OverlayItem {
            label: "/init".into(),
            description: "Open project setup wizard (generates HALCON.md)".into(),
            action: "init".into(),
        },
        OverlayItem {
            label: "/tools".into(),
            description: "Show tool usage statistics for this session".into(),
            action: "tools".into(),
        },
        OverlayItem {
            label: "/plugins".into(),
            description: "Show loaded plugin information".into(),
            action: "plugins".into(),
        },
        OverlayItem {
            label: "/dry-run".into(),
            description: "Toggle dry-run mode — destructive tools are skipped".into(),
            action: "dry-run".into(),
        },
        OverlayItem {
            label: "/reasoning".into(),
            description: "Show reasoning engine and UCB1 strategy status".into(),
            action: "reasoning".into(),
        },
    ]
}

/// Filter command items by query using case-insensitive substring matching.
pub fn filter_commands(items: &[OverlayItem], query: &str) -> Vec<OverlayItem> {
    if query.is_empty() {
        return items.to_vec();
    }
    let q = query.to_lowercase();
    items
        .iter()
        .filter(|item| {
            item.label.to_lowercase().contains(&q)
                || item.description.to_lowercase().contains(&q)
        })
        .cloned()
        .collect()
}

/// Calculate a centered rectangle within the given area.
pub fn centered_rect(area: Rect, width_pct: u16, height_pct: u16) -> Rect {
    let v_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - height_pct) / 2),
            Constraint::Percentage(height_pct),
            Constraint::Percentage((100 - height_pct) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - width_pct) / 2),
            Constraint::Percentage(width_pct),
            Constraint::Percentage((100 - width_pct) / 2),
        ])
        .split(v_chunks[1])[1]
}

/// Render the help overlay.
/// Build a help section with header and keybinding list.
fn build_help_section<'a>(
    header: &'a str,
    bindings: &'a [(&'a str, &'a str)],
    c_accent: ratatui::style::Color,
    c_text: ratatui::style::Color,
) -> Vec<Line<'a>> {
    let mut lines = vec![];
    lines.push(Line::from(Span::styled(
        header,
        Style::default().fg(c_accent).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
    for &(key, desc) in bindings {
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<12}", key), Style::default().fg(c_accent)),
            Span::styled(desc, Style::default().fg(c_text)),
        ]));
    }
    lines.push(Line::from(""));
    lines
}

pub fn render_help(frame: &mut Frame, area: Rect) {
    let p = &theme::active().palette;
    let c_border = p.border_ratatui();
    let c_accent = p.accent_ratatui();
    let c_text = p.text_ratatui();
    let c_muted = p.muted_ratatui();

    let rect = centered_rect(area, 60, 70);
    frame.render_widget(Clear, rect);

    let mut lines = Vec::new();
    lines.extend(build_help_section(
        constants::HELP_HEADER_NAVIGATION,
        constants::HELP_SECTION_NAVIGATION,
        c_accent,
        c_text,
    ));
    // Phase 2 NAV-001: Activity zone keybindings section
    lines.extend(build_help_section(
        constants::HELP_HEADER_ACTIVITY,
        constants::HELP_SECTION_ACTIVITY,
        c_accent,
        c_text,
    ));
    lines.extend(build_help_section(
        constants::HELP_HEADER_PANELS,
        constants::HELP_SECTION_PANELS,
        c_accent,
        c_text,
    ));
    lines.extend(build_help_section(
        constants::HELP_HEADER_AGENT,
        constants::HELP_SECTION_AGENT,
        c_accent,
        c_text,
    ));
    lines.extend(build_help_section(
        constants::HELP_HEADER_GENERAL,
        constants::HELP_SECTION_GENERAL,
        c_accent,
        c_text,
    ));

    lines.push(Line::from(Span::styled(
        "  Slash Commands: /help /model /mode /plan /panel /clear /search /quit",
        Style::default().fg(c_muted),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Press Esc to close",
        Style::default().fg(c_muted),
    )));

    let help = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Help (F1) ")
                .border_style(Style::default().fg(c_border)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(help, rect);
}

/// Render the permission approval overlay.
pub fn render_permission_prompt(frame: &mut Frame, area: Rect, tool: &str) {
    let p = &theme::active().palette;
    let c_border = p.border_ratatui();
    let c_warning = p.warning_ratatui();
    let c_text = p.text_ratatui();
    let c_accent = p.accent_ratatui();

    let rect = centered_rect(area, 50, 30);
    frame.render_widget(Clear, rect);

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Tool requires approval:",
            Style::default().fg(c_warning).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("    {tool}"),
            Style::default().fg(c_text).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  [Y] ", Style::default().fg(c_accent).add_modifier(Modifier::BOLD)),
            Span::styled("Approve", Style::default().fg(c_text)),
            Span::styled("    ", Style::default()),
            Span::styled("[N] ", Style::default().fg(c_warning).add_modifier(Modifier::BOLD)),
            Span::styled("Reject", Style::default().fg(c_text)),
            Span::styled("    ", Style::default()),
            Span::styled("[Esc] ", Style::default().fg(c_text)),
            Span::styled("Dismiss", Style::default().fg(c_text)),
        ]),
    ];

    let prompt = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Permission Required ")
                .border_style(Style::default().fg(c_warning)),
        )
        .alignment(Alignment::Left);

    frame.render_widget(prompt, rect);
}

/// Render the command palette overlay.
pub fn render_command_palette(
    frame: &mut Frame,
    area: Rect,
    input: &str,
    items: &[OverlayItem],
    selected: usize,
) {
    let p = &theme::active().palette;
    let c_border = p.border_ratatui();
    let c_accent = p.accent_ratatui();
    let c_text = p.text_ratatui();
    let c_muted = p.muted_ratatui();
    let c_running = p.running_ratatui();

    let rect = centered_rect(area, 60, 50);
    frame.render_widget(Clear, rect);

    let mut lines = vec![
        Line::from(vec![
            Span::styled("  > ", Style::default().fg(c_accent)),
            Span::styled(input, Style::default().fg(c_text)),
            Span::styled("_", Style::default().fg(c_accent)),
        ]),
        Line::from(""),
    ];

    for (i, item) in items.iter().enumerate() {
        let style = if i == selected {
            Style::default().fg(c_running).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(c_text)
        };
        let prefix = if i == selected { "  ▸ " } else { "    " };
        let desc = format!("  {}", item.description);
        lines.push(Line::from(vec![
            Span::styled(prefix, style),
            Span::styled(item.label.clone(), style),
            Span::styled(desc, Style::default().fg(c_muted)),
        ]));
    }

    if items.is_empty() {
        lines.push(Line::from(Span::styled(
            "    (no matches)",
            Style::default().fg(c_muted),
        )));
    }

    let palette = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Command Palette (Ctrl+P) ")
                .border_style(Style::default().fg(c_border)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(palette, rect);
}

/// Render the context servers overlay with real server status.
pub fn render_context_servers(
    frame: &mut Frame,
    area: Rect,
    servers: &[super::events::ContextServerInfo],
    total_count: usize,
    enabled_count: usize,
) {
    let p = &theme::active().palette;
    let c_border = p.border_ratatui();
    let c_accent = p.accent_ratatui();
    let c_text = p.text_ratatui();
    let c_muted = p.muted_ratatui();
    let c_success = p.success_ratatui();
    let c_warning = p.warning_ratatui();

    // Center the modal: 70% width, 60% height
    let rect = centered_rect(area, 70, 60);

    // Clear background
    frame.render_widget(Clear, rect);

    // Build content lines
    let mut lines = vec![];

    // Header
    lines.push(Line::from(vec![
        Span::styled("⚙ ", Style::default().fg(c_accent)),
        Span::styled(
            "Context Servers ",
            Style::default().fg(c_accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("({} / {} enabled)", enabled_count, total_count),
            Style::default().fg(c_muted),
        ),
    ]));
    lines.push(Line::from(""));

    if servers.is_empty() {
        lines.push(Line::from(Span::styled(
            "No context servers registered.",
            Style::default().fg(c_muted),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Context servers provide SDLC-aware context to the agent:",
            Style::default().fg(c_text),
        )));
        lines.push(Line::from(Span::styled(
            "  • Requirements, Architecture, Codebase",
            Style::default().fg(c_muted),
        )));
        lines.push(Line::from(Span::styled(
            "  • Workflows, Tests, Metrics",
            Style::default().fg(c_muted),
        )));
        lines.push(Line::from(Span::styled(
            "  • Security, Support",
            Style::default().fg(c_muted),
        )));
    } else {
        // Table header
        lines.push(Line::from(vec![
            Span::styled("NAME", Style::default().fg(c_accent).add_modifier(Modifier::BOLD)),
            Span::raw("            "),
            Span::styled("PRIORITY", Style::default().fg(c_accent).add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled("STATUS", Style::default().fg(c_accent).add_modifier(Modifier::BOLD)),
            Span::raw("      "),
            Span::styled("TOKENS", Style::default().fg(c_accent).add_modifier(Modifier::BOLD)),
            Span::raw("    "),
            Span::styled("QUERIES", Style::default().fg(c_accent).add_modifier(Modifier::BOLD)),
            Span::raw("    "),
            Span::styled("LAST QUERY", Style::default().fg(c_accent).add_modifier(Modifier::BOLD)),
        ]));
        lines.push(Line::from(Span::styled(
            "─".repeat(rect.width.saturating_sub(4) as usize),
            Style::default().fg(c_muted),
        )));

        // Server rows (sorted by priority descending)
        let mut sorted_servers = servers.to_vec();
        sorted_servers.sort_by(|a, b| b.priority.cmp(&a.priority));

        for server in sorted_servers.iter() {
            let status_span = if server.enabled {
                Span::styled("● ACTIVE", Style::default().fg(c_success))
            } else {
                Span::styled("○ DISABLED", Style::default().fg(c_warning))
            };

            let tokens_str = if server.total_tokens > 0 {
                if server.total_tokens >= 1000 {
                    format!("{:.1}K", server.total_tokens as f64 / 1000.0)
                } else {
                    server.total_tokens.to_string()
                }
            } else {
                "-".to_string()
            };

            let queries_str = if server.query_count > 0 {
                server.query_count.to_string()
            } else {
                "-".to_string()
            };

            let last_query_str = if let Some(ms) = server.last_query_ms {
                if ms < 1000 {
                    format!("{}ms", ms)
                } else if ms < 60000 {
                    format!("{:.1}s", ms as f64 / 1000.0)
                } else {
                    format!("{:.1}m", ms as f64 / 60000.0)
                }
            } else {
                "never".to_string()
            };

            lines.push(Line::from(vec![
                Span::styled(format!("{:<20}", server.name), Style::default().fg(c_text)),
                Span::raw("  "),
                Span::styled(format!("{:>3}", server.priority), Style::default().fg(c_accent)),
                Span::raw("  "),
                status_span,
                Span::raw("    "),
                Span::styled(format!("{:>6}", tokens_str), Style::default().fg(c_text)),
                Span::raw("    "),
                Span::styled(format!("{:>7}", queries_str), Style::default().fg(c_accent)),
                Span::raw("    "),
                Span::styled(format!("{:>10}", last_query_str), Style::default().fg(c_muted)),
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(""));

    // Footer hint
    lines.push(Line::from(vec![
        Span::styled("Press ", Style::default().fg(c_muted)),
        Span::styled("Esc", Style::default().fg(c_accent)),
        Span::styled(" to close", Style::default().fg(c_muted)),
    ]));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(c_border))
        .title(" Context Servers ");

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });

    frame.render_widget(paragraph, rect);
}

/// Render the search overlay with real match count and current position.
pub fn render_search(frame: &mut Frame, area: Rect, query: &str, match_count: usize, current: usize) {
    let p = &theme::active().palette;
    let c_border = p.border_ratatui();
    let c_accent = p.accent_ratatui();
    let c_text = p.text_ratatui();
    let c_muted = p.muted_ratatui();

    // Search bar at the top of the activity area.
    let search_area = Rect::new(area.x + 2, area.y, area.width.saturating_sub(4).min(60), 3);
    frame.render_widget(Clear, search_area);

    let match_label = if match_count > 0 {
        format!("  ({current}/{match_count})  ↑↓ navigate  Enter next")
    } else if query.is_empty() {
        String::new()
    } else {
        "  (no matches)".into()
    };

    let line = Line::from(vec![
        Span::styled(" / ", Style::default().fg(c_accent)),
        Span::styled(query, Style::default().fg(c_text)),
        Span::styled("_", Style::default().fg(c_accent)),
        Span::styled(&match_label, Style::default().fg(c_muted)),
    ]);

    let search = Paragraph::new(line).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Search (Ctrl+F) ")
            .border_style(Style::default().fg(c_border)),
    );

    frame.render_widget(search, search_area);
}

/// Render the session browser overlay (F6).
///
/// Shows a scrollable list of recent sessions with id, model, rounds, cost, date.
/// Navigation: Up/Down = move cursor; Enter = load session; Esc = close.
pub fn render_session_list(
    frame: &mut Frame,
    area: Rect,
    sessions: &[SessionInfo],
    selected: usize,
) {
    let p = &theme::active().palette;
    let c_border = p.border_ratatui();
    let c_accent = p.accent_ratatui();
    let c_text = p.text_ratatui();
    let c_muted = p.muted_ratatui();
    let c_highlight = p.bg_highlight_ratatui();
    let c_success = p.success_ratatui();

    // Center the overlay: 80% width, 70% height.
    let width = (area.width * 4 / 5).max(60).min(area.width.saturating_sub(4));
    let height = (area.height * 7 / 10).max(10).min(area.height.saturating_sub(4));
    let rect = Rect::new(
        area.x + (area.width.saturating_sub(width)) / 2,
        area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    );

    frame.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Sessions (F6) ")
        .border_style(Style::default().fg(c_border));

    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    if sessions.is_empty() {
        let msg = Paragraph::new(Line::from(vec![
            Span::styled("  No sessions found", Style::default().fg(c_muted)),
        ]));
        frame.render_widget(msg, inner);
        return;
    }

    // Header line + list rows + footer help.
    let list_height = inner.height.saturating_sub(2) as usize; // reserve 2 rows: header + footer

    let header_area = Rect::new(inner.x, inner.y, inner.width, 1);
    let list_area = Rect::new(inner.x, inner.y + 1, inner.width, inner.height.saturating_sub(2));
    let footer_area = Rect::new(inner.x, inner.y + inner.height.saturating_sub(1), inner.width, 1);

    // Header
    let header_line = Line::from(vec![
        Span::styled(
            format!("  {:<10} {:<25} {:>5}  {:>8}  date", "ID", "model", "R", "cost"),
            Style::default().fg(c_muted).add_modifier(Modifier::DIM),
        ),
    ]);
    frame.render_widget(Paragraph::new(header_line), header_area);

    // Viewport: show list_height rows starting from selected if beyond.
    let scroll = if selected >= list_height { selected - list_height + 1 } else { 0 };

    let mut rows: Vec<Line> = Vec::new();
    for (i, session) in sessions.iter().enumerate().skip(scroll).take(list_height) {
        let is_selected = i == selected;

        let short_id = if session.id.len() >= 8 { &session.id[..8] } else { &session.id };
        let model_str = if session.model.len() > 24 {
            format!("{:.24}", session.model)
        } else {
            session.model.clone()
        };
        let cost_str = format!("${:.4}", session.estimated_cost);
        // Shorten created_at: take first 10 chars (date part of ISO8601).
        let date_str: &str = if session.created_at.len() >= 10 {
            &session.created_at[..10]
        } else {
            &session.created_at
        };

        let prefix = if is_selected { "▶" } else { " " };
        let row_text = format!(
            "{} [{:<8}] {:<25} {:>5}  {:>8}  {}",
            prefix, short_id, model_str, session.agent_rounds, cost_str, date_str
        );

        let style = if is_selected {
            Style::default().fg(c_accent).bg(c_highlight).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(c_text)
        };
        rows.push(Line::from(vec![Span::styled(row_text, style)]));
    }

    // Pad remaining rows if fewer sessions than viewport.
    while rows.len() < list_height {
        rows.push(Line::from(vec![Span::raw("")]));
    }

    frame.render_widget(Paragraph::new(rows), list_area);

    // Footer help
    let footer = Line::from(vec![
        Span::styled("  ↑/↓ navigate  ", Style::default().fg(c_muted)),
        Span::styled("Enter", Style::default().fg(c_success)),
        Span::styled(" load  ", Style::default().fg(c_muted)),
        Span::styled("Esc", Style::default().fg(c_accent)),
        Span::styled(" close", Style::default().fg(c_muted)),
    ]);
    frame.render_widget(Paragraph::new(footer), footer_area);
}

/// Render the project onboarding init wizard overlay.
///
/// 4-zone centered overlay with step-by-step guidance:
/// - Step 0: analyzing
/// - Step 1: review detected project info
/// - Step 2: preview generated HALCON.md
/// - Step 3: confirm save
/// - Step 4: done
pub fn render_init_wizard(
    frame: &mut Frame,
    area: Rect,
    step: u8,
    preview: &str,
    save_path: &str,
    dry_run: bool,
    spinner_frame: usize,
) {
    let p = &theme::active().palette;
    let c_border = p.border_ratatui();
    let c_accent = p.accent.to_ratatui_color();
    let c_muted = p.text_label.to_ratatui_color();
    let c_success = p.success.to_ratatui_color();

    let popup_width = area.width.saturating_sub(8).min(72);
    let popup_height = area.height.saturating_sub(6).min(22);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect { x, y, width: popup_width, height: popup_height };

    frame.render_widget(Clear, popup_area);

    let title_text = if dry_run {
        format!(" ◈ HALCON — Configurar Proyecto  [dry-run]  Paso {}/4 ", step.min(4))
    } else {
        format!(" ◈ HALCON — Configurar Proyecto  Paso {}/4 ", step.min(4))
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(c_border))
        .title(Span::styled(title_text, Style::default().fg(c_accent).add_modifier(Modifier::BOLD)));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    // Build content lines based on step
    let lines: Vec<Line> = match step {
        0 => {
            let frames = ['⠁', '⠃', '⠇', '⠧', '⠷', '⠿', '⠾', '⠼', '⠸', '⠰'];
            let ch = frames[spinner_frame % frames.len()];
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {} Analizando proyecto…", ch),
                    Style::default().fg(c_accent).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "  Detectando tipo, metadata y estructura…",
                    Style::default().fg(c_muted),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "  ↳ Progreso en tiempo real en el feed de actividad",
                    Style::default().fg(c_muted),
                )),
            ]
        }
        1 => {
            // Parse the generated HALCON.md preview for key project metadata
            let mut proj_name = String::new();
            let mut proj_type = String::new();
            let mut proj_version = String::new();
            let mut git_branch = String::new();
            let mut description = String::new();
            let mut workspace_count = 0usize;

            for line in preview.lines() {
                let line = line.trim();
                if proj_name.is_empty() && line.starts_with("# HALCON — ") {
                    proj_name = line.trim_start_matches("# HALCON — ").to_string();
                } else if line.starts_with("**Tipo**: ") {
                    proj_type = line.trim_start_matches("**Tipo**: ").to_string();
                } else if line.starts_with("**Versión**: ") {
                    proj_version = line.trim_start_matches("**Versión**: ").to_string();
                } else if line.starts_with("**Branch**: ") {
                    git_branch = line.trim_start_matches("**Branch**: ").to_string();
                } else if line.starts_with("**Descripción**: ") {
                    description = line.trim_start_matches("**Descripción**: ").to_string();
                    if description.len() > 55 {
                        description = format!("{}…", &description[..{ let mut _fcb = (55).min(description.len()); while _fcb > 0 && !description.is_char_boundary(_fcb) { _fcb -= 1; } _fcb }]);
                    }
                } else if line.starts_with("- `") && line.ends_with('`') {
                    workspace_count += 1;
                }
            }

            let mut ls = vec![
                Line::from(Span::styled(
                    "  ✓ Análisis completo:",
                    Style::default().fg(c_success).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
            ];

            if !proj_name.is_empty() {
                let name_disp: String = proj_name.chars().take(50).collect();
                ls.push(Line::from(vec![
                    Span::styled("  Proyecto:  ", Style::default().fg(c_muted)),
                    Span::styled(name_disp, Style::default().fg(c_accent).add_modifier(Modifier::BOLD)),
                ]));
            }
            if !proj_type.is_empty() {
                ls.push(Line::from(vec![
                    Span::styled("  Tipo:      ", Style::default().fg(c_muted)),
                    Span::styled(proj_type.clone(), Style::default().fg(c_accent)),
                ]));
            }
            if workspace_count > 0 {
                ls.push(Line::from(vec![
                    Span::styled("  Crates:    ", Style::default().fg(c_muted)),
                    Span::styled(format!("{} miembros", workspace_count), Style::default().fg(c_muted)),
                ]));
            }
            if !proj_version.is_empty() && proj_version != "(desconocida)" {
                ls.push(Line::from(vec![
                    Span::styled("  Versión:   ", Style::default().fg(c_muted)),
                    Span::styled(proj_version, Style::default().fg(c_muted)),
                ]));
            }
            if !git_branch.is_empty() {
                ls.push(Line::from(vec![
                    Span::styled("  Branch:    ", Style::default().fg(c_muted)),
                    Span::styled(git_branch, Style::default().fg(c_muted)),
                ]));
            }
            if !description.is_empty() && description != "(sin descripción)" {
                ls.push(Line::from(vec![
                    Span::styled("  Desc:      ", Style::default().fg(c_muted)),
                    Span::styled(description, Style::default().fg(c_muted)),
                ]));
            }
            ls.push(Line::from(""));
            if !save_path.is_empty() {
                let path_display: String = save_path.chars().take(58).collect();
                ls.push(Line::from(vec![
                    Span::styled("  Guardar en: ", Style::default().fg(c_muted)),
                    Span::styled(path_display, Style::default().fg(c_success)),
                ]));
                ls.push(Line::from(""));
            }
            ls.push(Line::from(Span::styled(
                "  [Enter] Ver preview   [Esc] Cancelar",
                Style::default().fg(c_muted),
            )));
            ls
        }
        2 => {
            let mut ls = vec![
                Line::from(Span::styled("  Preview HALCON.md:", Style::default().fg(c_accent).add_modifier(Modifier::BOLD))),
                Line::from(""),
            ];
            let max_lines = inner.height.saturating_sub(4) as usize;
            for l in preview.lines().take(max_lines) {
                let display: String = l.chars().take(popup_width.saturating_sub(4) as usize).collect();
                ls.push(Line::from(Span::styled(format!("  {display}"), Style::default().fg(c_muted))));
            }
            ls.push(Line::from(""));
            let confirm_text = if dry_run {
                "  [Enter] Continuar (dry-run)   [Esc] Cancelar"
            } else {
                "  [Enter] Guardar   [Esc] Cancelar"
            };
            ls.push(Line::from(Span::styled(confirm_text, Style::default().fg(c_muted))));
            ls
        }
        3 => {
            let path_display: String = save_path.chars().take(60).collect();
            vec![
                Line::from(Span::styled("  Confirmar guardado:", Style::default().fg(c_accent).add_modifier(Modifier::BOLD))),
                Line::from(""),
                Line::from(vec![
                    Span::styled("  Ruta: ", Style::default().fg(c_muted)),
                    Span::styled(path_display, Style::default().fg(c_success)),
                ]),
                Line::from(""),
                Line::from(Span::styled(
                    if dry_run { "  [Enter] Simular (dry-run)   [Esc] Cancelar" }
                    else { "  [Enter] Escribir archivo   [Esc] Cancelar" },
                    Style::default().fg(c_muted),
                )),
            ]
        }
        _ => vec![
            Line::from(""),
            Line::from(Span::styled(
                "  ✓ ¡Listo! HALCON.md guardado.",
                Style::default().fg(c_success).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled("  [Enter/Esc] Cerrar", Style::default().fg(c_muted))),
        ],
    };

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Render the plugin recommendation overlay.
///
/// Shows tiered plugin suggestions with keyboard navigation:
/// - ↑/↓ — move selection
/// - [A] — show /plugins auto hint
/// - [Esc] — close
pub fn render_plugin_suggest(
    frame: &mut Frame,
    area: Rect,
    suggestions: &[PluginSuggestionItem],
    selected: usize,
    dry_run: bool,
) {
    let p = &theme::active().palette;
    let c_border = p.border_ratatui();
    let c_accent = p.accent_ratatui();
    let c_text = p.text_ratatui();
    let c_muted = p.muted_ratatui();
    let c_success = p.success_ratatui();
    let c_warning = p.warning_ratatui();
    let c_running = p.running_ratatui();

    let popup_width = area.width.saturating_sub(8).min(70);
    let popup_height = area.height.saturating_sub(6).min(24);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect { x, y, width: popup_width, height: popup_height };

    frame.render_widget(Clear, popup_area);

    let title = if dry_run {
        " ◈ HALCON — Plugin Recommendations [dry-run] "
    } else {
        " ◈ HALCON — Plugin Recommendations "
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(c_border))
        .title(Span::styled(
            title,
            Style::default().fg(c_accent).add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let mut lines: Vec<Line> = Vec::new();

    if suggestions.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  No plugins recommended for this project.",
            Style::default().fg(c_muted),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  [Esc] Close",
            Style::default().fg(c_muted),
        )));
    } else {
        // Group by tier
        let mut last_tier = String::new();
        for (i, item) in suggestions.iter().enumerate() {
            if item.tier != last_tier {
                // Tier header
                lines.push(Line::from(""));
                let tier_symbol = match item.tier.as_str() {
                    "Essential" => "◆",
                    "Recommended" => "◇",
                    "Optional" => "▷",
                    _ => "○",
                };
                let tier_color = match item.tier.as_str() {
                    "Essential" => c_running,
                    "Recommended" => c_accent,
                    "Optional" => c_muted,
                    _ => c_muted,
                };
                lines.push(Line::from(Span::styled(
                    format!("  {} {}", tier_symbol, item.tier),
                    Style::default().fg(tier_color).add_modifier(Modifier::BOLD),
                )));
                last_tier = item.tier.clone();
            }

            let is_selected = i == selected;
            let prefix = if is_selected { "  ❯ " } else { "    " };
            let name_style = if is_selected {
                Style::default().fg(c_accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(c_text)
            };

            lines.push(Line::from(vec![
                Span::styled(prefix, name_style),
                Span::styled(&item.plugin_id, name_style),
                if item.already_installed {
                    Span::styled(" [installed]", Style::default().fg(c_success))
                } else {
                    Span::raw("")
                },
            ]));
            // Rationale line
            let rationale_display: String = item.rationale.chars().take(
                popup_width.saturating_sub(6) as usize
            ).collect();
            lines.push(Line::from(Span::styled(
                format!("      {}", rationale_display),
                Style::default().fg(c_muted),
            )));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("  [A] Auto-install all  ", Style::default().fg(c_warning)),
            Span::styled("[↑↓] Navigate  ", Style::default().fg(c_muted)),
            Span::styled("[Esc] Close", Style::default().fg(c_muted)),
        ]));
    }

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        inner,
    );
}

/// Render the update-available notification overlay.
///
/// Shows current vs new version, optional release notes, download size, and
/// two actions: [Enter] install now, [Esc] dismiss.
pub fn render_update_available(
    frame: &mut Frame,
    area: Rect,
    current: &str,
    remote: &str,
    notes: &Option<String>,
    published_at: &Option<String>,
    size_bytes: u64,
) {
    let p = &theme::active().palette;
    let c_border = p.border_ratatui();
    let c_accent = p.accent_ratatui();
    let c_text = p.text_ratatui();
    let c_muted = p.muted_ratatui();
    let c_success = p.success_ratatui();
    let c_warning = p.warning_ratatui();

    // Compute popup height: base + notes lines
    let notes_lines = notes.as_deref()
        .map(|n| n.lines().count().min(10) as u16)
        .unwrap_or(0);
    let popup_height = (10 + notes_lines).min(area.height.saturating_sub(4));
    let popup_width = area.width.saturating_sub(8).min(68);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect { x, y, width: popup_width, height: popup_height };

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(c_warning))
        .title(Span::styled(
            " ⚡ Actualización disponible ",
            Style::default().fg(c_warning).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));

    // Version info
    let date_str = published_at.as_deref()
        .and_then(|d| d.get(..10))
        .map(|d| format!("  (publicado {d})"))
        .unwrap_or_default();
    lines.push(Line::from(vec![
        Span::styled("  Versión actual:  ", Style::default().fg(c_muted)),
        Span::styled(format!("v{current}"), Style::default().fg(c_text)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Nueva versión:   ", Style::default().fg(c_muted)),
        Span::styled(format!("v{remote}"), Style::default().fg(c_success).add_modifier(Modifier::BOLD)),
        Span::styled(date_str, Style::default().fg(c_muted)),
    ]));

    if size_bytes > 0 {
        let mb = size_bytes as f64 / 1_048_576.0;
        lines.push(Line::from(vec![
            Span::styled("  Tamaño:          ", Style::default().fg(c_muted)),
            Span::styled(format!("{mb:.1} MB"), Style::default().fg(c_text)),
        ]));
    }

    lines.push(Line::from(""));

    // Release notes (capped)
    if let Some(ref note_text) = notes {
        lines.push(Line::from(Span::styled(
            "  Notas de versión:",
            Style::default().fg(c_accent).add_modifier(Modifier::BOLD),
        )));
        for note_line in note_text.lines().take(10) {
            lines.push(Line::from(Span::styled(
                format!("    {note_line}"),
                Style::default().fg(c_text),
            )));
        }
        lines.push(Line::from(""));
    }

    // Divider + actions
    lines.push(Line::from(Span::styled(
        "  ─────────────────────────────────────────",
        Style::default().fg(c_muted),
    )));
    lines.push(Line::from(vec![
        Span::styled("  [", Style::default().fg(c_muted)),
        Span::styled("Enter", Style::default().fg(c_success).add_modifier(Modifier::BOLD)),
        Span::styled("] Instalar ahora    [", Style::default().fg(c_muted)),
        Span::styled("Esc", Style::default().fg(c_muted).add_modifier(Modifier::BOLD)),
        Span::styled("] Posponer", Style::default().fg(c_muted)),
    ]));

    let paragraph = ratatui::widgets::Paragraph::new(lines)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_state_starts_inactive() {
        let state = OverlayState::new();
        assert!(!state.is_active());
        assert!(state.active.is_none());
    }

    #[test]
    fn overlay_open_sets_kind() {
        let mut state = OverlayState::new();
        state.open(OverlayKind::Help);
        assert!(state.is_active());
        assert_eq!(state.active, Some(OverlayKind::Help));
    }

    #[test]
    fn overlay_close_clears() {
        let mut state = OverlayState::new();
        state.open(OverlayKind::CommandPalette);
        state.type_char('t');
        state.close();
        assert!(!state.is_active());
        assert!(state.input.is_empty());
        assert_eq!(state.cursor, 0);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn overlay_type_char() {
        let mut state = OverlayState::new();
        state.open(OverlayKind::Search);
        state.type_char('h');
        state.type_char('e');
        state.type_char('l');
        assert_eq!(state.input, "hel");
        assert_eq!(state.cursor, 3);
    }

    #[test]
    fn overlay_backspace() {
        let mut state = OverlayState::new();
        state.open(OverlayKind::Search);
        state.type_char('a');
        state.type_char('b');
        state.backspace();
        assert_eq!(state.input, "a");
        assert_eq!(state.cursor, 1);
    }

    #[test]
    fn overlay_backspace_at_empty() {
        let mut state = OverlayState::new();
        state.backspace(); // Should not panic.
        assert!(state.input.is_empty());
    }

    #[test]
    fn overlay_select_navigation() {
        let mut state = OverlayState::new();
        assert_eq!(state.selected, 0);
        state.select_next(5);
        assert_eq!(state.selected, 1);
        state.select_next(5);
        state.select_next(5);
        state.select_next(5);
        assert_eq!(state.selected, 4);
        state.select_next(5); // At max, should not go beyond.
        assert_eq!(state.selected, 4);
        state.select_prev();
        assert_eq!(state.selected, 3);
    }

    #[test]
    fn overlay_select_prev_at_zero() {
        let mut state = OverlayState::new();
        state.select_prev(); // Should not underflow.
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn centered_rect_produces_valid_rect() {
        let area = Rect::new(0, 0, 120, 40);
        let rect = centered_rect(area, 60, 50);
        assert!(rect.width > 0);
        assert!(rect.height > 0);
        assert!(rect.x + rect.width <= area.width);
        assert!(rect.y + rect.height <= area.height);
    }

    #[test]
    fn overlay_kind_eq() {
        assert_eq!(OverlayKind::Help, OverlayKind::Help);
        assert_ne!(OverlayKind::Help, OverlayKind::Search);
        assert_eq!(
            OverlayKind::PermissionPrompt { tool: "bash".into() },
            OverlayKind::PermissionPrompt { tool: "bash".into() },
        );
    }

    #[test]
    fn overlay_open_resets_state() {
        let mut state = OverlayState::new();
        state.open(OverlayKind::CommandPalette);
        state.type_char('x');
        state.selected = 5;
        // Re-opening should reset.
        state.open(OverlayKind::Help);
        assert!(state.input.is_empty());
        assert_eq!(state.selected, 0);
        assert_eq!(state.cursor, 0);
    }

    // --- Phase C2: Command palette tests ---

    #[test]
    fn default_commands_not_empty() {
        let cmds = default_commands();
        assert!(cmds.len() >= 6);
    }

    #[test]
    fn filter_empty_query_returns_all() {
        let cmds = default_commands();
        let filtered = filter_commands(&cmds, "");
        assert_eq!(filtered.len(), cmds.len());
    }

    #[test]
    fn filter_by_label() {
        let cmds = default_commands();
        let filtered = filter_commands(&cmds, "help");
        assert!(filtered.iter().any(|i| i.label.contains("help")));
    }

    #[test]
    fn filter_by_description() {
        let cmds = default_commands();
        let filtered = filter_commands(&cmds, "keybind");
        assert!(filtered.iter().any(|i| i.description.to_lowercase().contains("keybind")));
    }

    #[test]
    fn filter_no_match_returns_empty() {
        let cmds = default_commands();
        let filtered = filter_commands(&cmds, "zzzzz_nonexistent");
        assert!(filtered.is_empty());
    }

    #[test]
    fn filter_case_insensitive() {
        let cmds = default_commands();
        let filtered_lower = filter_commands(&cmds, "quit");
        let filtered_upper = filter_commands(&cmds, "QUIT");
        assert_eq!(filtered_lower.len(), filtered_upper.len());
    }

    // --- Slash-autocomplete contract tests ---

    #[test]
    fn default_commands_contains_extended_commands() {
        let cmds = default_commands();
        let labels: Vec<&str> = cmds.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"/init"),    "missing /init");
        assert!(labels.contains(&"/tools"),   "missing /tools");
        assert!(labels.contains(&"/plugins"), "missing /plugins");
        assert!(labels.contains(&"/dry-run"), "missing /dry-run");
        assert!(labels.contains(&"/reasoning"), "missing /reasoning");
        assert!(labels.contains(&"/inspect"), "missing /inspect");
    }

    #[test]
    fn filter_slash_prefix_returns_matching_commands() {
        let cmds = default_commands();
        // Simulates what happens when the user types "/pa" — should match /pause and /panel
        let filtered = filter_commands(&cmds, "pa");
        assert!(filtered.iter().any(|i| i.label == "/pause"), "/pause must match 'pa'");
        assert!(filtered.iter().any(|i| i.label == "/panel"), "/panel must match 'pa'");
        // Should NOT match /quit
        assert!(!filtered.iter().any(|i| i.label == "/quit"), "/quit must not match 'pa'");
    }

    #[test]
    fn filter_empty_prefix_returns_all_commands() {
        let cmds = default_commands();
        // "/" with no suffix → empty query → all commands shown
        let filtered = filter_commands(&cmds, "");
        assert_eq!(filtered.len(), cmds.len(), "empty query must return all commands");
    }

    #[test]
    fn all_default_commands_have_non_empty_action() {
        for cmd in default_commands() {
            assert!(
                !cmd.action.is_empty(),
                "command '{}' has an empty action string",
                cmd.label,
            );
        }
    }

    #[test]
    fn all_default_commands_label_starts_with_slash() {
        for cmd in default_commands() {
            assert!(
                cmd.label.starts_with('/'),
                "command label '{}' does not start with '/'",
                cmd.label,
            );
        }
    }
}
