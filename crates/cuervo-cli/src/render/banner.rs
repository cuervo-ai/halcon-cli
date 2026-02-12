//! Startup banner: ASCII logo, environment info, and tips.

use std::io::{self, Write};

use super::{color, components, theme};

/// Block-letter CUERVO logo for wide terminals.
const LOGO_NEON: &str = "\
 ██████╗██╗   ██╗███████╗██████╗ ██╗   ██╗ ██████╗
██╔════╝██║   ██║██╔════╝██╔══██╗██║   ██║██╔═══██╗
██║     ██║   ██║█████╗  ██████╔╝██║   ██║██║   ██║
██║     ██║   ██║██╔══╝  ██╔══██╗╚██╗ ██╔╝██║   ██║
╚██████╗╚██████╔╝███████╗██║  ██║ ╚████╔╝ ╚██████╔╝
 ╚═════╝ ╚═════╝ ╚══════╝╚═╝  ╚═╝  ╚═══╝   ╚═════╝";

/// Compact logo for narrow terminals.
const LOGO_COMPACT: &str = "CUERVO";

/// Small geometric crow silhouette.
const CROW_ART: &str = "\
    ▄▀▀▀▄
   █  ●  █▄
   █     ██▀
    ▀▄▄▄▀▀
 ▀▀▀▀▀▀▀▀▀▀▀";

/// Startup tips pool.
const TIPS: &[&str] = &[
    "Type /help for available commands",
    "Press Alt+Enter for multi-line input",
    "Use /quit or Ctrl+D to exit",
    "Try /test status for diagnostics",
    "Use --resume <id> to continue a session",
];

/// Routing chain display info for multi-model configurations.
pub struct RoutingDisplay {
    pub mode: String,
    pub strategy: String,
    pub fallback_chain: Vec<String>,
}

/// Whether to show the banner, respecting env var and config.
pub fn should_show(config_show: bool) -> bool {
    if std::env::var_os("CUERVO_NO_BANNER").is_some() {
        return false;
    }
    config_show
}

/// Render the full startup banner to stderr.
#[allow(clippy::too_many_arguments)]
pub fn render_startup(
    version: &str,
    provider: &str,
    provider_connected: bool,
    model: &str,
    session_id: &str,
    session_type: &str,
    tip_index: usize,
    routing: Option<&RoutingDisplay>,
) {
    let mut out = io::stderr().lock();
    let t = theme::active();
    let r = theme::reset();
    let width = color::terminal_width() as usize;

    let _ = writeln!(out);

    if width >= 60 && color::unicode_enabled() {
        // Wide layout: block letters + crow art + info.
        let primary = t.palette.primary.fg();

        // Render crow art in violet.
        if width >= 80 {
            let violet = t.palette.violet.fg();
            for line in CROW_ART.lines() {
                let _ = writeln!(out, "  {violet}{line}{r}");
            }
        }

        // Render block logo in primary color.
        for line in LOGO_NEON.lines() {
            let _ = writeln!(out, "  {primary}{line}{r}");
        }
    } else {
        // Narrow/plain layout.
        let primary = t.palette.primary.fg();
        let _ = writeln!(out, "  {primary}{LOGO_COMPACT}{r}");
    }

    // Version tagline.
    let accent = t.palette.accent.fg();
    let dim = t.palette.text_dim.fg();
    let _ = writeln!(
        out,
        "  {accent}v{version}{r}  {dim}AI-powered CLI for software development{r}",
    );

    // Horizontal rule.
    let rule_width = width.min(54);
    components::hr(rule_width, &mut out);

    // Environment info as key-value table.
    let provider_status = if provider_connected {
        "connected"
    } else {
        "not configured"
    };
    let provider_val = format!("{provider} ({provider_status})");
    let session_val = format!("{session_id} ({session_type})");

    let routing_val = routing
        .filter(|r| !r.fallback_chain.is_empty())
        .map(|r| format!("{}: {}", r.mode, r.fallback_chain.join(" → ")));

    let mut kv: Vec<(&str, &str)> = vec![
        ("Provider", &provider_val),
        ("Model", model),
        ("Session", &session_val),
    ];
    if let Some(ref rv) = routing_val {
        kv.push(("Routing", rv));
    }
    components::kv_table(&kv, 2, &mut out);

    // Tip line.
    let tip = TIPS[tip_index % TIPS.len()];
    let muted = t.palette.muted.fg();
    let _ = writeln!(out, "\n  {muted}{tip}{r}\n");
}

/// Render a minimal startup line (used when banner is disabled but info is still needed).
pub fn render_minimal(version: &str, provider: &str, model: &str, fallback_count: Option<usize>) {
    let t = theme::active();
    let r = theme::reset();
    let accent = t.palette.accent.fg();
    let dim = t.palette.text_dim.fg();
    let suffix = match fallback_count {
        Some(n) if n > 0 => format!(" | failover: {n} fallbacks"),
        _ => String::new(),
    };
    eprintln!(
        "{accent}cuervo{r} {dim}v{version}{r} | {provider}/{model}{suffix}",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_show_respects_default() {
        // When env var is not set, should follow config.
        // (In test env, CUERVO_NO_BANNER might or might not be set.)
        let _ = should_show(true);
        let _ = should_show(false);
    }

    #[test]
    fn logo_neon_is_multi_line() {
        assert!(LOGO_NEON.lines().count() >= 5);
    }

    #[test]
    fn crow_art_is_multi_line() {
        assert!(CROW_ART.lines().count() >= 4);
    }

    #[test]
    fn tips_non_empty() {
        assert!(!TIPS.is_empty());
        for tip in TIPS {
            assert!(!tip.is_empty());
        }
    }

    #[test]
    fn render_startup_does_not_panic() {
        render_startup(
            "0.1.0",
            "anthropic",
            true,
            "claude-sonnet",
            "abc12345",
            "new",
            0,
            None,
        );
    }

    #[test]
    fn render_startup_all_tip_indices() {
        for i in 0..TIPS.len() + 2 {
            render_startup("0.1.0", "echo", false, "echo", "00000000", "new", i, None);
        }
    }

    #[test]
    fn render_minimal_does_not_panic() {
        render_minimal("0.1.0", "anthropic", "claude-sonnet", None);
    }

    #[test]
    fn render_startup_with_routing() {
        let routing = RoutingDisplay {
            mode: "failover".into(),
            strategy: "balanced".into(),
            fallback_chain: vec![
                "anthropic".into(),
                "deepseek".into(),
                "openai".into(),
            ],
        };
        render_startup(
            "0.1.0", "anthropic", true, "claude-sonnet",
            "abc12345", "new", 0, Some(&routing),
        );
    }

    #[test]
    fn render_startup_no_routing() {
        let routing = RoutingDisplay {
            mode: "failover".into(),
            strategy: "balanced".into(),
            fallback_chain: vec![],
        };
        // Empty chain should not add a Routing row — same as None.
        render_startup(
            "0.1.0", "echo", false, "echo",
            "00000000", "new", 0, Some(&routing),
        );
    }

    #[test]
    fn render_minimal_with_fallbacks() {
        render_minimal("0.1.0", "anthropic", "claude-sonnet", Some(3));
        // Zero fallbacks should not add suffix.
        render_minimal("0.1.0", "echo", "echo", Some(0));
        render_minimal("0.1.0", "echo", "echo", None);
    }

    #[test]
    fn logo_compact_non_empty() {
        assert!(!LOGO_COMPACT.is_empty());
    }
}
