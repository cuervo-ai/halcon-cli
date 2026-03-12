//! Theme generation, optimization, and accessibility audit commands.

#[cfg(feature = "color-science")]
use crate::render::adaptive_optimizer::{AdaptivePaletteOptimizer, OptimizationConfig};
#[cfg(feature = "color-science")]
use crate::render::intelligent_theme::{IntelligentPaletteBuilder, QualityThresholds};
use anyhow::{Context, Result};
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct ThemeArgs {
    #[command(subcommand)]
    pub command: ThemeCommand,
}

#[derive(Debug, Subcommand)]
pub enum ThemeCommand {
    /// Optimize a palette using adaptive pipeline
    Optimize(OptimizeArgs),
    /// Full accessibility audit: CVD + APCA + harmony score
    Audit(AuditArgs),
    /// Activate CVD simulation mode (affects adaptive palette)
    Cvd(CvdArgs),
    /// Generate palette with harmony type + optional constraint solver + HCT
    Generate(GenerateArgs),
    /// Preview a harmony type and its quality score
    Harmony(HarmonyArgs),
}

#[derive(Debug, Args)]
pub struct OptimizeArgs {
    /// Base hue in degrees (0-360)
    pub hue: f64,

    /// Optimization config preset: fast, default, high-quality
    #[arg(long, default_value = "default")]
    pub config: String,

    /// Enable verbose output showing optimization steps
    #[arg(long, short)]
    pub verbose: bool,

    /// Target quality threshold (0.0-1.0)
    #[arg(long)]
    pub target: Option<f64>,

    /// Maximum iterations
    #[arg(long)]
    pub max_iterations: Option<usize>,

    /// Show detailed weak pair diagnostics
    #[arg(long)]
    pub show_weak_pairs: bool,
}

#[derive(Debug, Args)]
pub struct AuditArgs {
    /// Base hue in degrees (0-360) for palette generation
    #[arg(long, default_value_t = 210.0)]
    pub hue: f64,
}

#[derive(Debug, Args)]
pub struct CvdArgs {
    /// CVD type to simulate: deuteranopia, protanopia, tritanopia, off
    pub cvd_type: String,
}

#[derive(Debug, Args)]
pub struct GenerateArgs {
    /// Base hue in degrees (0-360)
    pub hue: f64,

    /// Harmony type: complementary, analogous, triadic, tetradic, split-complementary, monochromatic
    #[arg(long, default_value = "analogous")]
    pub harmony: String,

    /// Apply ConstraintSolver for formal WCAG/APCA guarantees
    #[arg(long)]
    pub solver: bool,

    /// Use HCT (Material Design 3) tonal system instead of OKLCH
    #[arg(long)]
    pub hct: bool,
}

#[derive(Debug, Args)]
pub struct HarmonyArgs {
    /// Harmony type: complementary, analogous, triadic, tetradic, split-complementary, monochromatic
    pub harmony_type: String,

    /// Base hue in degrees (0-360)
    #[arg(long, default_value_t = 210.0)]
    pub hue: f64,
}

// ============================================================================
// Dispatch
// ============================================================================

#[cfg(feature = "color-science")]
pub fn run(args: ThemeArgs) -> Result<()> {
    match args.command {
        ThemeCommand::Optimize(opt_args) => optimize(opt_args),
        ThemeCommand::Audit(audit_args) => audit(audit_args),
        ThemeCommand::Cvd(cvd_args) => cvd(cvd_args),
        ThemeCommand::Generate(gen_args) => generate(gen_args),
        ThemeCommand::Harmony(harm_args) => harmony(harm_args),
    }
}

#[cfg(not(feature = "color-science"))]
pub fn run(_args: ThemeArgs) -> Result<()> {
    anyhow::bail!(
        "Theme commands require the 'color-science' feature. Rebuild with --features color-science"
    )
}

// ============================================================================
// optimize
// ============================================================================

#[cfg(feature = "color-science")]
fn optimize(args: OptimizeArgs) -> Result<()> {
    use std::time::Instant;

    if !(0.0..=360.0).contains(&args.hue) {
        anyhow::bail!("Hue must be between 0 and 360 degrees");
    }

    let thresholds = QualityThresholds {
        min_overall: 0.3,
        min_compliance: 0.4,
        min_perceptual: 0.2,
        min_confidence: 0.3,
    };

    let builder = IntelligentPaletteBuilder::with_thresholds(thresholds);

    let mut config = match args.config.as_str() {
        "fast" => OptimizationConfig::fast(),
        "default" => OptimizationConfig::default(),
        "high-quality" | "high_quality" => OptimizationConfig::high_quality(),
        other => anyhow::bail!("Unknown config preset: {}", other),
    };

    if args.verbose {
        config.verbose = true;
    }
    if let Some(target) = args.target {
        if !(0.0..=1.0).contains(&target) {
            anyhow::bail!("Target quality must be between 0.0 and 1.0");
        }
        config.target_quality = target;
    }
    if let Some(max_iter) = args.max_iterations {
        config.max_iterations = max_iter;
    }

    println!("\n  Adaptive Palette Optimization");
    println!("══════════════════════════════════════");
    println!("  Base hue:         {:.1}°", args.hue);
    println!("  Config preset:    {}", args.config);
    println!("  Target quality:   {:.2}", config.target_quality);
    println!("  Max iterations:   {}", config.max_iterations);
    println!();

    let start = Instant::now();
    let mut optimizer = AdaptivePaletteOptimizer::with_config(builder, config);
    let result = optimizer
        .optimize_from_hue(args.hue)
        .context("Failed to optimize palette")?;
    let elapsed = start.elapsed();

    println!("\n✓ Optimization Complete");
    println!("══════════════════════════════════════");
    println!("  Iterations:          {}", result.iterations);
    println!("  Quality improvement: {:.4}", result.quality_improvement);
    println!(
        "  Final quality:       {:.4}",
        result.final_palette.quality_report.average_overall()
    );
    println!("  Convergence:         {}", result.convergence_status);
    println!("  Duration:            {:.2}s", elapsed.as_secs_f64());
    println!();

    let palette = &result.final_palette.palette;
    println!("  Final Palette:");
    println!("  ──────────────────────────────────────");
    print_color_token("text", &palette.text);
    print_color_token("primary", &palette.primary);
    print_color_token("accent", &palette.accent);
    print_color_token("warning", &palette.warning);
    print_color_token("error", &palette.error);
    print_color_token("success", &palette.success);
    println!();

    let report = &result.final_palette.quality_report;
    println!("  Quality Metrics:");
    println!("  ──────────────────────────────────────");
    println!("  Overall:          {:.3}", report.average_overall());
    println!("  Weak pairs:       {}", report.weak_pairs().len());
    if !report.advanced_scores.is_empty() {
        let (compliance, perceptual, priority, confidence) = report.average_advanced_metrics();
        println!("  Compliance:       {:.3}", compliance);
        println!("  Perceptual:       {:.3}", perceptual);
        println!("  Priority:         {:.3}", priority);
        println!("  Confidence:       {:.3}", confidence);
    }
    println!();

    if args.show_weak_pairs {
        let weak_pairs = report.weak_pairs();
        if weak_pairs.is_empty() {
            println!("  ✓ No weak pairs detected");
        } else {
            println!("  ⚠ Weak Pairs ({}):", weak_pairs.len());
            println!("  ──────────────────────────────────────");
            for (token_name, score) in &weak_pairs {
                println!("    {:<15} Overall: {:.3}", token_name, score.overall);
            }
        }
        println!();
    }

    Ok(())
}

// ============================================================================
// audit
// ============================================================================

#[cfg(feature = "color-science")]
fn audit(args: AuditArgs) -> Result<()> {
    use crate::render::color_science::{
        harmony_score_for_palette, palette_from_hue, validate_all_cvd, validate_with_apca_usage,
    };

    if !(0.0..=360.0).contains(&args.hue) {
        anyhow::bail!("Hue must be between 0 and 360 degrees");
    }

    let palette = palette_from_hue(args.hue);

    println!("\n  Theme Accessibility Audit — hue {:.1}°", args.hue);
    println!("══════════════════════════════════════════════");
    println!();

    // --- CVD Validation ---
    println!("  CVD (Color Vision Deficiency) Validation");
    println!("  ──────────────────────────────────────────");
    let cvd_report = validate_all_cvd(&palette);
    let protan_ok = cvd_report.protan_failures.is_empty();
    let deutan_ok = cvd_report.deutan_failures.is_empty();
    let tritan_ok = cvd_report.tritan_failures.is_empty();
    println!(
        "  Protanopia:   {}",
        if protan_ok {
            "✓ safe"
        } else {
            "✗ failures"
        }
    );
    if !protan_ok {
        for (a, b, de) in &cvd_report.protan_failures {
            println!("    - {} / {}: ΔE={:.1} (< 15.0)", a, b, de);
        }
    }
    println!(
        "  Deuteranopia: {}",
        if deutan_ok {
            "✓ safe"
        } else {
            "✗ failures"
        }
    );
    if !deutan_ok {
        for (a, b, de) in &cvd_report.deutan_failures {
            println!("    - {} / {}: ΔE={:.1} (< 15.0)", a, b, de);
        }
    }
    println!(
        "  Tritanopia:   {}",
        if tritan_ok {
            "✓ safe"
        } else {
            "✗ failures"
        }
    );
    if !tritan_ok {
        for (a, b, de) in &cvd_report.tritan_failures {
            println!("    - {} / {}: ΔE={:.1} (< 15.0)", a, b, de);
        }
    }
    println!();

    // --- APCA Validation ---
    println!("  APCA Semantic Contrast Validation");
    println!("  ──────────────────────────────────────────");
    let apca_results = validate_with_apca_usage(&palette);
    let mut any_apca_fail = false;
    for (name, lc, required, passes) in &apca_results {
        let icon = if *passes { "✓" } else { "✗" };
        println!(
            "  {} {:<14} Lc={:>6.1}  (req ≥ {:.0})",
            icon, name, lc, required
        );
        if !passes {
            any_apca_fail = true;
        }
    }
    if !any_apca_fail {
        println!("\n  ✓ All APCA semantic checks passed");
    }
    println!();

    // --- Harmony Score ---
    println!("  Color Harmony Score");
    println!("  ──────────────────────────────────────────");
    let h_score = harmony_score_for_palette(&palette);
    let harmony_label = if h_score >= 0.8 {
        "Excellent"
    } else if h_score >= 0.6 {
        "Good"
    } else if h_score >= 0.4 {
        "Fair"
    } else {
        "Poor"
    };
    println!("  Score: {:.3}  ({})", h_score, harmony_label);
    println!();

    // --- Summary ---
    let overall_pass = cvd_report.all_safe && !any_apca_fail;
    println!("══════════════════════════════════════════════");
    if overall_pass {
        println!("  ✓ Audit PASSED — palette is fully accessible");
    } else {
        println!("  ✗ Audit FAILED — see failures above");
        println!(
            "    Tip: run `halcon theme generate --hue {:.0} --harmony analogous --solver`",
            args.hue
        );
    }
    println!();

    Ok(())
}

// ============================================================================
// cvd
// ============================================================================

#[cfg(feature = "color-science")]
fn cvd(args: CvdArgs) -> Result<()> {
    use momoto_core::color::cvd::CVDType;

    match args.cvd_type.to_lowercase().as_str() {
        "off" | "none" => {
            println!("\n  CVD mode: OFF");
            println!("  Set HALCON_CVD_MODE=off or unset to disable CVD simulation.");
        }
        t => {
            let cvd = CVDType::from_str(t).ok_or_else(|| {
                anyhow::anyhow!(
                    "Unknown CVD type '{}'. Use: deuteranopia, protanopia, tritanopia, off",
                    t
                )
            })?;
            let label = match cvd {
                CVDType::Protanopia => "Protanopia  (L-cone absent, red-green confusion)",
                CVDType::Deuteranopia => "Deuteranopia (M-cone absent, most common red-green)",
                CVDType::Tritanopia => "Tritanopia   (S-cone absent, blue-yellow, rare)",
            };
            println!("\n  CVD mode: {}", label);
            println!();
            println!("  To activate in the TUI, set the environment variable:");
            println!("    export HALCON_CVD_MODE={}", t);
            println!("    halcon chat --tui");
            println!();
            println!("  The adaptive palette will use CVD-safe hues (blue/orange)");
            println!("  instead of red/green in critical health states.");
        }
    }

    println!();
    Ok(())
}

// ============================================================================
// generate
// ============================================================================

#[cfg(feature = "color-science")]
fn generate(args: GenerateArgs) -> Result<()> {
    use momoto_intelligence::HarmonyType;

    if !(0.0..=360.0).contains(&args.hue) {
        anyhow::bail!("Hue must be between 0 and 360 degrees");
    }

    let thresholds = QualityThresholds {
        min_overall: 0.3,
        min_compliance: 0.4,
        min_perceptual: 0.2,
        min_confidence: 0.3,
    };
    let builder = IntelligentPaletteBuilder::with_thresholds(thresholds);

    println!("\n  Palette Generation");
    println!("══════════════════════════════════════");
    println!("  Base hue:  {:.1}°", args.hue);
    println!("  Harmony:   {}", args.harmony);
    println!(
        "  Solver:    {}",
        if args.solver { "enabled" } else { "disabled" }
    );
    println!(
        "  HCT:       {}",
        if args.hct { "enabled" } else { "disabled" }
    );
    println!();

    if args.hct {
        // Generate using HCT tonal system
        let seed_hex = format!(
            "#{:02x}{:02x}{:02x}",
            (args.hue.to_radians().cos() * 127.0 + 127.0) as u8,
            (args.hue.to_radians().sin() * 127.0 + 127.0) as u8,
            100u8,
        );
        let palette = builder
            .generate_hct_palette(&seed_hex)
            .context("Failed to generate HCT palette")?;

        println!("  HCT Tonal Palette:");
        println!("  ──────────────────────────────────────");
        print_color_token("primary", &palette.primary);
        print_color_token("accent", &palette.accent);
        print_color_token("text", &palette.text);
        print_color_token("text_dim", &palette.text_dim);
        print_color_token("bg_panel", &palette.bg_panel);
        print_color_token("running", &palette.running);
        print_color_token("planning", &palette.planning);
        print_color_token("error", &palette.error);
        println!();
        return Ok(());
    }

    if args.solver {
        // Parse harmony type
        let harmony = parse_harmony_type(&args.harmony)?;

        let result = builder
            .generate_constrained(args.hue, harmony, true)
            .context("Constraint solver failed to generate palette")?;

        println!("  Constrained Palette:");
        println!("  ──────────────────────────────────────");
        let palette = &result.palette;
        print_color_token("primary", &palette.primary);
        print_color_token("accent", &palette.accent);
        print_color_token("text", &palette.text);
        print_color_token("bg_panel", &palette.bg_panel);
        print_color_token("error", &palette.error);
        println!();

        println!("  Harmony Score: {:.3}", result.harmony_score);
        if let Some(ref sr) = result.solver_result {
            println!(
                "  Solver: {} in {} iterations (penalty={:.4})",
                if sr.converged {
                    "converged"
                } else {
                    "max iterations"
                },
                sr.iterations,
                sr.final_penalty,
            );
            if !sr.violations.is_empty() {
                println!("  Remaining violations ({}):", sr.violations.len());
                for v in &sr.violations {
                    println!("    - {}", v);
                }
            }
        }
    } else {
        // Standard generation
        let result = builder
            .generate_from_hue(args.hue)
            .context("Failed to generate palette")?;

        let palette = &result.palette;
        println!("  Generated Palette:");
        println!("  ──────────────────────────────────────");
        print_color_token("primary", &palette.primary);
        print_color_token("accent", &palette.accent);
        print_color_token("text", &palette.text);
        print_color_token("bg_panel", &palette.bg_panel);
        print_color_token("error", &palette.error);
        print_color_token("success", &palette.success);
        print_color_token("warning", &palette.warning);
        println!();
        println!("  Quality: {:.3}", result.quality_report.average_overall());
    }

    println!();
    Ok(())
}

// ============================================================================
// harmony
// ============================================================================

#[cfg(feature = "color-science")]
fn harmony(args: HarmonyArgs) -> Result<()> {
    use momoto_core::OKLCH;
    use momoto_intelligence::{generate_palette, harmony_score, HarmonyType};

    if !(0.0..=360.0).contains(&args.hue) {
        anyhow::bail!("Hue must be between 0 and 360 degrees");
    }

    let harmony_type = parse_harmony_type(&args.harmony_type)?;
    let seed = OKLCH::new(0.65, 0.18, args.hue);
    let palette = generate_palette(seed, harmony_type.clone());
    let score = harmony_score(&palette.colors);

    println!("\n  Harmony Preview — {:?}", harmony_type);
    println!("══════════════════════════════════════");
    println!("  Base hue: {:.1}°", args.hue);
    println!("  Colors:   {}", palette.colors.len());
    println!(
        "  Score:    {:.3}  ({})",
        score,
        if score >= 0.8 {
            "Excellent"
        } else if score >= 0.6 {
            "Good"
        } else if score >= 0.4 {
            "Fair"
        } else {
            "Poor"
        }
    );
    println!();
    println!("  Palette Colors:");
    println!("  ──────────────────────────────────────");
    for (i, oklch) in palette.colors.iter().enumerate() {
        let name = format!("color_{}", i + 1);
        let tc = crate::render::theme::ThemeColor::oklch(oklch.l, oklch.c, oklch.h);
        println!(
            "  {:12} L={:.2} C={:.2} H={:>5.1}°  {}",
            format!("{}:", name),
            oklch.l,
            oklch.c,
            oklch.h,
            tc.fg()
        );
    }
    println!();

    Ok(())
}

// ============================================================================
// Helpers
// ============================================================================

#[cfg(feature = "color-science")]
fn parse_harmony_type(s: &str) -> Result<momoto_intelligence::HarmonyType> {
    use momoto_intelligence::HarmonyType;
    match s.to_lowercase().as_str() {
        "complementary" => Ok(HarmonyType::Complementary),
        "split-complementary" => Ok(HarmonyType::SplitComplementary),
        "triadic" => Ok(HarmonyType::Triadic),
        "tetradic" => Ok(HarmonyType::Tetradic),
        "analogous" => Ok(HarmonyType::Analogous { spread: 45.0 }),
        "monochromatic" => Ok(HarmonyType::Monochromatic { steps: 5 }),
        other => anyhow::bail!(
            "Unknown harmony type '{}'. Choose: complementary, split-complementary, \
             triadic, tetradic, analogous, monochromatic",
            other
        ),
    }
}

#[cfg(feature = "color-science")]
fn print_color_token(name: &str, color: &crate::render::theme::ThemeColor) {
    use momoto_core::OKLCH;
    let oklch = OKLCH::from_color(color.color());
    println!(
        "  {:12} L={:.2} C={:.2} H={:>5.1}°  {}",
        format!("{}:", name),
        oklch.l,
        oklch.c,
        oklch.h,
        color.fg(),
    );
}
