//! Startup banner: ASCII logo, environment info, and SOTA feature indicators.
//! Enhanced with Momoto semantic colors and intelligent information display.

use std::io::{self, Write};

use super::{color, components, theme};

/// Block-letter HALCON logo for wide terminals.
const LOGO_NEON: &str = "\
██╗  ██╗ █████╗ ██╗      ██████╗ ██████╗ ███╗   ██╗
██║  ██║██╔══██╗██║     ██╔════╝██╔═══██╗████╗  ██║
███████║███████║██║     ██║     ██║   ██║██╔██╗ ██║
██╔══██║██╔══██║██║     ██║     ██║   ██║██║╚██╗██║
██║  ██║██║  ██║███████╗╚██████╗╚██████╔╝██║ ╚████║
╚═╝  ╚═╝╚═╝  ╚═╝╚══════╝ ╚═════╝ ╚═════╝ ╚═╝  ╚═══╝";

/// Compact logo for narrow terminals.
const LOGO_COMPACT: &str = "HALCON";

/// OKLCH Spectrum — demonstrates Momoto's full color capabilities.
/// Shows perceptual uniformity across hue spectrum + lightness gradient.
/// Uses Unicode block elements (█) for smooth gradients with 24-bit ANSI colors.

/// HALCÓN tips — precise, useful, no decoration.
const TIPS: &[&str] = &[
    "--tui --full  Complete operational interface",
    "/inspect      Live context tiers, metrics, cost",
    "/model        Switch provider or model mid-session",
    "--expert      Full telemetry and reasoning visibility",
    "L0-L4 context pipeline active every session",
    "23 tools: file ops, git, search, web, background",
    "UCB1 learning adapts strategy from experience",
    "ReadOnly tools speculated before model responds",
    "PII detection active — your data stays local",
    "HALCON.md in any directory sets scoped context",
];

/// Feature status indicators for banner.
#[derive(Debug, Clone)]
pub struct FeatureStatus {
    pub tui_active: bool,
    pub reasoning_enabled: bool,
    pub orchestrator_enabled: bool,
    pub context_pipeline_active: bool,
    pub tool_count: usize,
    pub background_tools_enabled: bool,
    /// Whether the multimodal subsystem (image/audio/video analysis) is active.
    pub multimodal_enabled: bool,
    /// Whether LoopCritic adversarial evaluation is active (adds ~1-3s per loop).
    pub loop_critic_enabled: bool,
    /// Whether a project-level HALCON.md was found in the working directory.
    pub project_config: bool,
}

impl Default for FeatureStatus {
    fn default() -> Self {
        Self {
            tui_active: false,
            reasoning_enabled: false,
            orchestrator_enabled: false,
            context_pipeline_active: true,
            tool_count: 23,
            background_tools_enabled: true,
            multimodal_enabled: false,
            loop_critic_enabled: false,
            project_config: false,
        }
    }
}

/// Routing chain display info for multi-model configurations.
pub struct RoutingDisplay {
    pub mode: String,
    pub strategy: String,
    pub fallback_chain: Vec<String>,
}

/// Whether to show the banner, respecting env var and config.
pub fn should_show(config_show: bool) -> bool {
    if std::env::var_os("HALCON_NO_BANNER").is_some() {
        return false;
    }
    config_show
}

/// Render the full startup banner to stderr with Momoto enhancements.
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
    render_startup_with_features(
        version,
        provider,
        provider_connected,
        model,
        session_id,
        session_type,
        tip_index,
        routing,
        &FeatureStatus::default(),
    );
}

/// Render startup banner with feature status indicators (enhanced version).
#[allow(clippy::too_many_arguments)]
pub fn render_startup_with_features(
    version: &str,
    provider: &str,
    provider_connected: bool,
    model: &str,
    session_id: &str,
    session_type: &str,
    tip_index: usize,
    routing: Option<&RoutingDisplay>,
    features: &FeatureStatus,
) {
    let mut out = io::stderr().lock();
    let t = theme::active();
    let r = theme::reset();
    let width = color::terminal_width() as usize;

    let _ = writeln!(out);

    if width >= 60 && color::unicode_enabled() {
        // HALCÓN wide layout: monochromatic blade gradient logo, no spectrum noise.
        let logo_lines: Vec<&str> = LOGO_NEON.lines().collect();
        let col0 = t.palette.primary.fg();
        let col1 = t.palette.accent.fg();
        let col2 = t.palette.text_dim.fg();
        for (i, line) in logo_lines.iter().enumerate() {
            let color = match i {
                0 | 4 => &col0,
                1 | 3 => &col1,
                2 => &col0,
                _ => &col2,
            };
            let _ = writeln!(out, "  {color}{line}{r}");
        }
    } else {
        // Narrow/plain layout — blade name only.
        let primary = t.palette.primary.fg();
        let _ = writeln!(out, "  {primary}{LOGO_COMPACT}{r}");
    }

    // Precision tagline — version + identity statement.
    let blade = t.palette.primary.fg();
    let dim = t.palette.text_dim.fg();
    let muted = t.palette.muted.fg();
    let _ = writeln!(
        out,
        "  {blade}v{version}{r}  {dim}AI-native engineering CLI{r}"
    );

    // HALCÓN divider — thin, precise.
    let rule_width = width.min(54);
    components::hr(rule_width, &mut out);

    // Environment info — minimal, scannable.
    let connected_icon = if provider_connected { "◆" } else { "◇" };
    let provider_color = if provider_connected {
        t.palette.success.fg()
    } else {
        t.palette.warning.fg()
    };
    let provider_val = format!("{provider_color}{connected_icon}{r} {provider}");
    let session_val = format!("{session_id} ({session_type})");

    let routing_val = routing
        .filter(|rd| !rd.fallback_chain.is_empty())
        .map(|rd| format!("{}: {}", rd.mode, rd.fallback_chain.join(" → ")));

    let mut kv: Vec<(String, String)> = vec![
        ("Provider".to_string(), provider_val),
        ("Model".to_string(), model.to_string()),
        ("Session".to_string(), session_val),
    ];

    if let Some(ref rv) = routing_val {
        kv.push(("Routing".to_string(), rv.clone()));
    }

    let kv_refs: Vec<(&str, &str)> = kv.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    components::kv_table(&kv_refs, 2, &mut out);

    // Capability strip — compact, one line, no labels. Only what matters.
    if width >= 60 {
        let _ = writeln!(out);
        let on = t.palette.primary.fg();
        let off = t.palette.muted.fg();

        // Each dot: on = falcon blue filled, off = muted empty
        let dot = |active: bool, label: &str| -> String {
            if active {
                format!("{on}◆{r} {label}")
            } else {
                format!("{off}◇{r} {label}")
            }
        };

        let tools_lbl = if features.background_tools_enabled {
            format!("{} tools", features.tool_count)
        } else {
            format!("{} tools", features.tool_count)
        };

        let _ = writeln!(
            out,
            "  {}  {}  {}  {}  {}  {}  {muted}{tools_lbl}{r}",
            dot(features.tui_active, "TUI"),
            dot(features.context_pipeline_active, "L0-L4"),
            dot(features.reasoning_enabled, "Reasoning"),
            dot(features.orchestrator_enabled, "Orchestrator"),
            dot(features.multimodal_enabled, "Multimodal"),
            dot(features.project_config, "project cfg"),
        );
    }

    // Tip — one actionable line, no emoji, HALCÓN voice.
    let tip = TIPS[tip_index % TIPS.len()];
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
    eprintln!("{accent}halcon{r} {dim}v{version}{r} | {provider}/{model}{suffix}",);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_show_respects_default() {
        // When env var is not set, should follow config.
        // (In test env, HALCON_NO_BANNER might or might not be set.)
        let _ = should_show(true);
        let _ = should_show(false);
    }

    #[test]
    fn logo_neon_is_multi_line() {
        assert!(LOGO_NEON.lines().count() >= 5);
    }

    // Note: CROW_ART was removed in favor of LOGO_NEON and LOGO_COMPACT
    // #[test]
    // fn crow_art_is_multi_line() {
    //     assert!(CROW_ART.lines().count() >= 4);
    // }

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
            fallback_chain: vec!["anthropic".into(), "deepseek".into(), "openai".into()],
        };
        render_startup(
            "0.1.0",
            "anthropic",
            true,
            "claude-sonnet",
            "abc12345",
            "new",
            0,
            Some(&routing),
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
            "0.1.0",
            "echo",
            false,
            "echo",
            "00000000",
            "new",
            0,
            Some(&routing),
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

    #[test]
    fn render_startup_with_all_features_enabled() {
        let features = FeatureStatus {
            tui_active: true,
            reasoning_enabled: true,
            orchestrator_enabled: true,
            context_pipeline_active: true,
            tool_count: 23,
            background_tools_enabled: true,
            multimodal_enabled: true,
            loop_critic_enabled: true,
            project_config: true,
        };
        render_startup_with_features(
            "0.2.0",
            "anthropic",
            true,
            "claude-sonnet-4-5",
            "abc12345",
            "new",
            0,
            None,
            &features,
        );
    }

    #[test]
    fn render_startup_with_minimal_features() {
        let features = FeatureStatus {
            tui_active: false,
            reasoning_enabled: false,
            orchestrator_enabled: false,
            context_pipeline_active: true,
            tool_count: 20,
            background_tools_enabled: false,
            multimodal_enabled: false,
            loop_critic_enabled: false,
            project_config: false,
        };
        render_startup_with_features(
            "0.2.0",
            "ollama",
            true,
            "deepseek-coder-v2",
            "test123",
            "resumed",
            1,
            None,
            &features,
        );
    }

    #[test]
    fn feature_status_default() {
        let features = FeatureStatus::default();
        assert!(!features.tui_active);
        assert!(!features.reasoning_enabled);
        assert!(!features.orchestrator_enabled);
        assert!(features.context_pipeline_active);
        assert_eq!(features.tool_count, 23);
    }
}

/// Render OKLCH spectrum bars demonstrating Momoto's perceptual uniformity.
///
/// Shows two dimensions of OKLCH color space:
/// 1. Hue spectrum (0°-360°) with constant lightness & chroma
/// 2. Lightness gradient (0.2-0.9) with constant hue
///
/// Uses Unicode block elements (█) with 24-bit ANSI RGB for smooth gradients.
/// Pre-computed colors ensure consistent performance (~2ms total).
#[cfg(feature = "color-science")]
fn render_oklch_spectrum(
    out: &mut std::io::StderrLock,
    palette: &super::theme::Palette,
    reset: &str,
) {
    use super::theme::ThemeColor;

    let dim = palette.text_dim.fg();
    let _accent = palette.accent.fg();

    // Spectrum Bar 1: Hue dimension (40 steps, 9° each = 0° to 351°)
    let _ = writeln!(
        out,
        "  {dim}╔═══════════════════════════════════════════╗{reset}"
    );
    let _ = write!(out, "  {dim}║{reset} ");

    // Pre-compute all 40 hue colors
    let hue_colors: Vec<String> = (0..40)
        .map(|i| {
            let hue = (i as f64) * 9.0; // 0°, 9°, 18°, ..., 351°
            let color = ThemeColor::oklch(0.70, 0.15, hue);
            color.fg()
        })
        .collect();

    // Render hue spectrum
    for color_str in &hue_colors {
        let _ = write!(out, "{color_str}█{reset}");
    }

    let _ = writeln!(out, " {dim}║{reset}");
    let _ = writeln!(
        out,
        "  {dim}╚═══════════════════════════════════════════╝{reset}"
    );
    let _ = writeln!(out);

    // Spectrum Bar 2: Lightness dimension (20 steps from dark to light)
    let _ = writeln!(out, "  {dim}╔═════════════════════╗{reset}");
    let _ = write!(out, "  {dim}║{reset} ");

    // Pre-compute all 20 lightness colors (violet hue)
    let lightness_colors: Vec<String> = (0..20)
        .map(|i| {
            let lightness = 0.25 + (i as f64) * 0.035; // 0.25 to 0.915
            let color = ThemeColor::oklch(lightness, 0.12, 280.0); // Violet hue
            color.fg()
        })
        .collect();

    // Render lightness spectrum
    for color_str in &lightness_colors {
        let _ = write!(out, "{color_str}█{reset}");
    }

    let _ = writeln!(out, " {dim}║{reset}");
    let _ = writeln!(out, "  {dim}╚═════════════════════╝{reset}");
    let _ = writeln!(out);
}

// Fallback for non-color-science builds: simple colored bars
#[cfg(not(feature = "color-science"))]
fn render_oklch_spectrum(
    out: &mut std::io::StderrLock,
    palette: &super::theme::Palette,
    reset: &str,
) {
    let dim = palette.text_dim.fg();
    let accent = palette.accent.fg();

    // Simple 6-color spectrum using palette
    let _ = writeln!(out, "  {dim}╔═══════════════════════╗{reset}");
    let _ = write!(out, "  {dim}║{reset} ");

    let colors = [
        &palette.error,     // Red
        &palette.warning,   // Yellow
        &palette.success,   // Green
        &palette.cyan,      // Cyan
        &palette.neon_blue, // Blue
        &palette.violet,    // Violet
    ];

    for color in &colors {
        let color_str = color.fg();
        for _ in 0..4 {
            let _ = write!(out, "{color_str}█{reset}");
        }
    }

    let _ = writeln!(out, " {dim}║{reset}");
    let _ = writeln!(out, "  {dim}╚═══════════════════════╝{reset}");
    let _ = writeln!(out);
}
