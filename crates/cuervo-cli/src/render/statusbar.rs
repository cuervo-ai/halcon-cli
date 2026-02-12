//! Sticky status bar — persistent bottom-line display.
//!
//! Shows: provider | model | latency | tokens | cost | health
//! Updates in-place using cursor movement (no scroll pollution).

use std::io::Write;

use super::color;
use super::theme;

/// Session statistics for the status bar.
#[derive(Debug, Clone, Default)]
pub struct SessionStats {
    pub provider: String,
    pub model: String,
    pub round: u32,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub last_latency_ms: u64,
    pub health_score: u8,
    pub streaming: bool,
    pub tool_active: Option<String>,
    pub tokens_per_sec: f64,
}

/// Render the status bar to a writer (for testing) or stderr (for production).
pub fn render_status_bar(stats: &SessionStats, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();
    let width = color::terminal_width() as usize;
    let bold = if color::color_enabled() { "\x1b[1m" } else { "" };

    // Build segments.
    let mut segments = Vec::new();

    // Provider + model.
    let primary = t.palette.primary.fg();
    let provider_model = if stats.model.is_empty() {
        stats.provider.clone()
    } else {
        format!("{}/{}", stats.provider, stats.model)
    };
    segments.push(format!("{bold}{primary}{provider_model}{r}"));

    // Round.
    let muted = t.palette.text_dim.fg();
    segments.push(format!("{muted}R{}{r}", stats.round));

    // Latency.
    let latency_color = if stats.last_latency_ms < 500 {
        t.palette.success.fg()
    } else if stats.last_latency_ms < 2000 {
        t.palette.warning.fg()
    } else {
        t.palette.error.fg()
    };
    segments.push(format!("{latency_color}{}ms{r}", stats.last_latency_ms));

    // Tokens.
    let token_str = format_compact_number(stats.total_tokens);
    segments.push(format!("{muted}{token_str} tok{r}"));

    // Cost.
    if stats.total_cost_usd > 0.0 {
        segments.push(format!("{muted}${:.4}{r}", stats.total_cost_usd));
    }

    // Streaming indicator.
    if stats.streaming {
        let accent = t.palette.accent.fg();
        let rate = if stats.tokens_per_sec > 0.0 {
            format!(" {:.0}t/s", stats.tokens_per_sec)
        } else {
            String::new()
        };
        segments.push(format!("{accent}▶ streaming{rate}{r}"));
    }

    // Tool active.
    if let Some(tool) = &stats.tool_active {
        let warn = t.palette.warning.fg();
        segments.push(format!("{warn}⚙ {tool}{r}"));
    }

    // Health indicator.
    let health_str = match stats.health_score {
        90..=100 => format!("{}●{r}", t.palette.success.fg()),
        70..=89 => format!("{}●{r}", t.palette.warning.fg()),
        _ => format!("{}●{r}", t.palette.error.fg()),
    };
    segments.push(health_str);

    // Join with separator.
    let sep = format!(" {muted}│{r} ");
    let line = segments.join(&sep);

    // Background bar.
    let bg = if color::color_enabled() {
        "\x1b[48;5;236m" // dark gray background
    } else {
        ""
    };
    let bg_reset = if color::color_enabled() {
        "\x1b[49m"
    } else {
        ""
    };

    // Pad to full width.
    // Strip ANSI codes to calculate visible length.
    let visible_len = strip_ansi_len(&line);
    let padding = width.saturating_sub(visible_len + 2);

    let _ = write!(
        out,
        "{bg} {line}{:>pad$} {bg_reset}{r}",
        "",
        pad = padding,
    );
}

/// Render the status bar directly to stderr (production use).
pub fn print_status_bar(stats: &SessionStats) {
    if !color::is_tty() {
        return;
    }
    let mut out = std::io::stderr().lock();
    // Save cursor, move to bottom, render, restore.
    let _ = write!(out, "\x1b[s\x1b[999;1H"); // save + move to last line
    render_status_bar(stats, &mut out);
    let _ = write!(out, "\x1b[u"); // restore cursor
    let _ = out.flush();
}

/// Clear the status bar from the screen.
pub fn clear_status_bar() {
    if !color::is_tty() {
        return;
    }
    let mut out = std::io::stderr().lock();
    let _ = write!(out, "\x1b[s\x1b[999;1H\x1b[2K\x1b[u"); // save, goto bottom, clear line, restore
    let _ = out.flush();
}

/// Format a number compactly (e.g., 1234 → "1.2k", 1234567 → "1.2M").
fn format_compact_number(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

/// Calculate visible length of a string (stripping ANSI codes).
fn strip_ansi_len(s: &str) -> usize {
    let mut len = 0;
    let mut in_escape = false;
    for c in s.chars() {
        if c == '\x1b' {
            in_escape = true;
        } else if in_escape {
            if c == 'm' {
                in_escape = false;
            }
        } else {
            len += 1;
        }
    }
    len
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture() -> Vec<u8> {
        Vec::new()
    }

    fn output_str(buf: &[u8]) -> String {
        String::from_utf8_lossy(buf).to_string()
    }

    #[test]
    fn status_bar_default_stats() {
        let mut buf = capture();
        render_status_bar(&SessionStats::default(), &mut buf);
        let out = output_str(&buf);
        // Should contain round and token count.
        assert!(out.contains("R0"));
        assert!(out.contains("tok"));
    }

    #[test]
    fn status_bar_with_provider() {
        let stats = SessionStats {
            provider: "deepseek".into(),
            model: "chat".into(),
            round: 5,
            total_tokens: 12345,
            total_cost_usd: 0.0042,
            last_latency_ms: 350,
            health_score: 95,
            ..Default::default()
        };
        let mut buf = capture();
        render_status_bar(&stats, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("deepseek/chat"));
        assert!(out.contains("R5"));
        assert!(out.contains("350ms"));
        assert!(out.contains("12.3k"));
        assert!(out.contains("$0.0042"));
    }

    #[test]
    fn status_bar_streaming() {
        let stats = SessionStats {
            streaming: true,
            tokens_per_sec: 42.5,
            ..Default::default()
        };
        let mut buf = capture();
        render_status_bar(&stats, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("streaming"));
        assert!(out.contains("43t/s") || out.contains("42t/s"));
    }

    #[test]
    fn status_bar_tool_active() {
        let stats = SessionStats {
            tool_active: Some("file_read".into()),
            ..Default::default()
        };
        let mut buf = capture();
        render_status_bar(&stats, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("file_read"));
    }

    #[test]
    fn status_bar_health_levels() {
        for (score, _expected) in [(95, "green"), (75, "yellow"), (30, "red")] {
            let stats = SessionStats {
                health_score: score,
                ..Default::default()
            };
            let mut buf = capture();
            render_status_bar(&stats, &mut buf);
            let out = output_str(&buf);
            assert!(out.contains("●"), "health indicator missing for score {score}");
        }
    }

    // ── format_compact_number ─────────────────────────────────

    #[test]
    fn compact_number_small() {
        assert_eq!(format_compact_number(42), "42");
        assert_eq!(format_compact_number(999), "999");
    }

    #[test]
    fn compact_number_thousands() {
        assert_eq!(format_compact_number(1500), "1.5k");
        assert_eq!(format_compact_number(12345), "12.3k");
    }

    #[test]
    fn compact_number_millions() {
        assert_eq!(format_compact_number(1_500_000), "1.5M");
    }

    // ── strip_ansi_len ────────────────────────────────────────

    #[test]
    fn strip_ansi_plain() {
        assert_eq!(strip_ansi_len("hello"), 5);
    }

    #[test]
    fn strip_ansi_with_codes() {
        assert_eq!(strip_ansi_len("\x1b[31mred\x1b[0m"), 3);
    }

    #[test]
    fn strip_ansi_empty() {
        assert_eq!(strip_ansi_len(""), 0);
    }

    #[test]
    fn strip_ansi_multiple_codes() {
        assert_eq!(strip_ansi_len("\x1b[1m\x1b[38;2;0;0;0mhi\x1b[0m"), 2);
    }
}
