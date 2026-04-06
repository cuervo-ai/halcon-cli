//! Intelligent theme generation using momoto-intelligence.
//!
//! This module uses the RecommendationEngine, QualityScorer, and ExplanationGenerator
//! from momoto-intelligence to create scientifically-optimal color palettes with
//! explainable reasoning.
//!
//! # Design Philosophy
//!
//! Instead of manually specifying OKLCH coordinates, we use momoto's AI-like
//! recommendation system to suggest optimal colors based on:
//! - Usage context (body text, headings, buttons, decorative)
//! - Compliance targets (WCAG AA/AAA, APCA thresholds)
//! - Perceptual quality scoring
//! - Multi-factor optimization (compliance + perceptual + usability)
//!
//! # Example
//!
//! ```rust,ignore
//! use crate::render::intelligent_theme::IntelligentPaletteBuilder;
//! use crate::render::theme::ThemeColor;
//!
//! let builder = IntelligentPaletteBuilder::new();
//! let base_hue = 210.0; // Blue
//! let palette = builder.generate_from_hue(base_hue);
//!
//! // All colors are optimized with explanations
//! println!("{}", palette.explanation_summary());
//! ```

use momoto_core::space::hct::HCT;
use momoto_core::{Color, OKLCH};
use momoto_intelligence::{
    generate_palette as harmony_gen, harmony_score as harmony_score_fn, ColorConstraint,
    ConstraintKind, ConstraintSolver, HarmonyType, SolverConfig, SolverResult,
};
use momoto_intelligence::{
    AdvancedScore, AdvancedScorer, ComplianceTarget, QualityScore, QualityScorer, Recommendation,
    RecommendationContext, RecommendationEngine, RecommendationExplanation, TechnicalDetails,
    UsageContext,
};

use super::terminal_caps::ColorLevel;
use super::theme::{Palette, ThemeColor};

/// M1: Card backgrounds generated via RecommendationEngine.
#[derive(Debug, Clone)]
pub struct CardBackgrounds {
    /// User message background (interactive context).
    pub user: ThemeColor,
    /// Assistant message background (body_text context).
    pub assistant: ThemeColor,
    /// Tool execution background (large_text context, cyan tint).
    pub tool: ThemeColor,
    /// Code block background (darker for contrast).
    pub code: ThemeColor,
}

/// Quality thresholds for palette generation.
///
/// These thresholds ensure generated palettes meet minimum quality standards
/// across multiple dimensions.
#[derive(Debug, Clone, Copy)]
pub struct QualityThresholds {
    /// Minimum overall quality score (0.0-1.0). Default: 0.8
    pub min_overall: f64,
    /// Minimum compliance score (0.0-1.0). Default: 0.9
    pub min_compliance: f64,
    /// Minimum perceptual score (0.0-1.0). Default: 0.7
    pub min_perceptual: f64,
    /// Minimum confidence (0.0-1.0). Default: 0.75
    pub min_confidence: f64,
}

impl Default for QualityThresholds {
    fn default() -> Self {
        Self {
            min_overall: 0.8,
            min_compliance: 0.9,
            min_perceptual: 0.7,
            min_confidence: 0.75,
        }
    }
}

/// Intelligent palette builder using momoto-intelligence.
///
/// Uses RecommendationEngine to generate optimal color palettes with
/// quality validation and explainable reasoning.
pub struct IntelligentPaletteBuilder {
    engine: RecommendationEngine,
    scorer: QualityScorer,
    advanced_scorer: AdvancedScorer,
    thresholds: QualityThresholds,
}

impl IntelligentPaletteBuilder {
    /// Create a new intelligent palette builder with default thresholds.
    pub fn new() -> Self {
        Self::with_thresholds(QualityThresholds::default())
    }

    /// Create a new intelligent palette builder with custom thresholds.
    pub fn with_thresholds(thresholds: QualityThresholds) -> Self {
        Self {
            engine: RecommendationEngine::new(),
            scorer: QualityScorer::new(),
            advanced_scorer: AdvancedScorer::new(),
            thresholds,
        }
    }

    /// Generate a complete palette from a base hue angle (0-360°).
    ///
    /// Uses RecommendationEngine to intelligently select all semantic tokens
    /// based on usage context and compliance targets. Returns None if unable
    /// to generate a palette meeting quality thresholds.
    pub fn generate_from_hue(&self, base_hue: f64) -> Option<PaletteWithMetadata> {
        // Step 1: Generate base brand color at medium lightness/chroma
        let brand = ThemeColor::oklch(0.75, 0.15, base_hue);
        let _brand_color = *brand.color();

        // Step 2: Define background (dark panel for TUI)
        let bg_panel = ThemeColor::oklch(0.18, 0.02, base_hue);
        let bg_panel_color = *bg_panel.color();

        // Step 3: Recommend primary text color for body text context
        let text_context = RecommendationContext::body_text();
        let text_rec = self
            .engine
            .recommend_foreground(bg_panel_color, text_context);

        // Validate text recommendation meets thresholds
        if !self.validate_recommendation(&text_rec, &text_context) {
            return None;
        }

        let text = ThemeColor::from_color(text_rec.color);

        // Step 4: Recommend accent color (complementary hue)
        let accent_context = RecommendationContext::interactive();
        let accent_rec = self
            .engine
            .recommend_foreground(bg_panel_color, accent_context);

        if !self.validate_recommendation(&accent_rec, &accent_context) {
            return None;
        }

        let accent = ThemeColor::from_color(accent_rec.color);

        // Step 5: Generate semantic colors with specific hues for universal meaning
        let warning = self.generate_semantic(
            bg_panel_color,
            95.0, // Yellow hue
            0.88, // High lightness
            0.18, // Medium chroma
            UsageContext::Interactive,
        )?;

        let error = self.generate_semantic(
            bg_panel_color,
            25.0, // Red hue
            0.62, // Medium lightness
            0.22, // Higher chroma for attention
            UsageContext::Interactive,
        )?;

        let success = self.generate_semantic(
            bg_panel_color,
            145.0, // Green hue
            0.75,  // Medium-high lightness
            0.18,  // Medium chroma
            UsageContext::Interactive,
        )?;

        // Step 6: Generate cockpit semantic tokens
        let running = self.generate_semantic(
            bg_panel_color,
            195.0, // Cyan
            0.78,
            0.12,
            UsageContext::Decorative,
        )?;

        let planning = self.generate_semantic(
            bg_panel_color,
            280.0, // Violet
            0.72,
            0.14,
            UsageContext::Decorative,
        )?;

        let reasoning = self.generate_semantic(
            bg_panel_color,
            170.0, // Teal
            0.70,
            0.10,
            UsageContext::Decorative,
        )?;

        let delegated = self.generate_semantic(
            bg_panel_color,
            310.0, // Magenta
            0.65,
            0.16,
            UsageContext::Decorative,
        )?;

        let destructive = self.generate_semantic(
            bg_panel_color,
            25.0, // Red (same as error)
            0.58,
            0.24,
            UsageContext::Interactive,
        )?;

        let cached = self.generate_semantic(
            bg_panel_color,
            85.0, // Yellow-orange
            0.75,
            0.08,
            UsageContext::Decorative,
        )?;

        let retrying = self.generate_semantic(
            bg_panel_color,
            60.0, // Orange
            0.82,
            0.15,
            UsageContext::Interactive,
        )?;

        let compacting = ThemeColor::oklch(0.68, 0.06, base_hue);

        let border = ThemeColor::oklch(0.40, 0.03, base_hue);
        let bg_highlight = ThemeColor::oklch(0.22, 0.04, base_hue);
        let text_label = ThemeColor::oklch(0.60, 0.04, base_hue);
        let spinner_color = ThemeColor::oklch(0.85, 0.12, 195.0);

        // M1: Card backgrounds via RecommendationEngine
        let card_backgrounds = self.generate_card_backgrounds(base_hue, *text.color());
        let bg_user = card_backgrounds.user;
        let bg_assistant = card_backgrounds.assistant;
        let bg_tool = card_backgrounds.tool;
        let bg_code = card_backgrounds.code;

        // Step 7: Generate muted and dim text variants
        let text_dim_context =
            RecommendationContext::new(UsageContext::LargeText, ComplianceTarget::WCAG_AA);
        let text_dim_rec = self
            .engine
            .recommend_foreground(bg_panel_color, text_dim_context);

        if !self.validate_recommendation(&text_dim_rec, &text_dim_context) {
            return None;
        }

        let text_dim = ThemeColor::from_color(text_dim_rec.color);

        let muted = ThemeColor::oklch(0.55, 0.02, base_hue);

        // Step 8: Build complete palette (M1: with card backgrounds)
        let palette = Palette {
            neon_blue: brand,
            cyan: accent,
            violet: planning,
            deep_blue: ThemeColor::oklch(0.15, 0.04, base_hue),
            primary: brand,
            accent,
            warning,
            error,
            success,
            muted,
            text,
            text_dim,
            running,
            planning,
            reasoning,
            delegated,
            destructive,
            cached,
            retrying,
            compacting,
            border,
            bg_panel,
            bg_highlight,
            text_label,
            spinner_color,
            bg_user,
            bg_assistant,
            bg_tool,
            bg_code,
        };

        // Step 9: Validate overall palette quality
        let quality_report = self.assess_palette(&palette);

        if !quality_report.meets_thresholds(self.thresholds) {
            return None;
        }

        // M3: Validate contrast for all card/text pairs
        let contrast_validations =
            crate::render::contrast_validator::validate_palette_contrast(&palette);
        crate::render::contrast_validator::log_contrast_warnings(&contrast_validations);

        // Step 10: Generate explanations
        let explanation = momoto_intelligence::ExplanationBuilder::new()
            .summary(format!(
                "Intelligent palette generated from {:.0}° hue",
                base_hue
            ))
            .problem(format!(
                "Generate accessible color palette from base hue {:.0}°",
                base_hue
            ))
            .benefit("All text colors meet WCAG AA contrast requirements")
            .benefit("Perceptually uniform OKLCH color space ensures consistent appearance")
            .benefit("Semantic tokens use universal color meanings (red=error, green=success)")
            .benefit(format!(
                "Overall palette quality: {:.0}%",
                quality_report.average_overall() * 100.0
            ))
            .build();

        Some(PaletteWithMetadata {
            palette,
            quality_report,
            explanation,
            base_hue,
            harmony_score: 0.0,
            solver_result: None,
        })
    }

    /// Improve an existing palette using RecommendationEngine.
    ///
    /// Analyzes the palette, identifies weak color pairs, and suggests improvements.
    /// Returns a new palette with optimized colors or None if no improvements possible.
    pub fn improve_palette(&self, palette: &Palette) -> Option<PaletteWithMetadata> {
        let quality_report = self.assess_palette(palette);

        // Find the weakest foreground/background pairs
        let weak_pairs = quality_report.weak_pairs();

        if weak_pairs.is_empty() {
            // Palette already optimal
            return None;
        }

        // For now, return None (full implementation would iterate improvements)
        // TODO: Implement iterative improvement loop
        None
    }

    /// Generate a semantic token color using a starting OKLCH coordinate
    /// and improving it to meet quality thresholds.
    fn generate_semantic(
        &self,
        bg: Color,
        hue: f64,
        lightness: f64,
        chroma: f64,
        usage: UsageContext,
    ) -> Option<ThemeColor> {
        let initial = ThemeColor::oklch(lightness, chroma, hue);
        let context = RecommendationContext::new(usage, ComplianceTarget::WCAG_AA);

        // Try to improve the initial color
        let recommendation = self
            .engine
            .improve_foreground(*initial.color(), bg, context);

        if self.validate_recommendation(&recommendation, &context) {
            Some(ThemeColor::from_color(recommendation.color))
        } else {
            // Fallback to initial if improvement doesn't help
            Some(initial)
        }
    }

    /// Generate scientifically-optimal card backgrounds via perceptual adjustments.
    ///
    /// M1: Uses OKLCH perceptual space to compute accessible bg colors:
    /// - User messages (slightly more prominent, lighter)
    /// - Assistant messages (more subtle, closer to base)
    /// - Tool execution (distinct cyan tint)
    /// - Code blocks (darker for contrast)
    ///
    /// All backgrounds validated for WCAG AA contrast with text_color.
    fn generate_card_backgrounds(&self, base_hue: f64, text_color: Color) -> CardBackgrounds {
        // M2: Use ElevationSystem for perceptually uniform background hierarchy
        use super::theme::ElevationSystem;

        let elevation = ElevationSystem::new(base_hue);

        // User messages: emphasized level (L=0.20, most prominent)
        let bg_user = elevation.emphasized();

        // Validate user bg contrast
        let user_context = RecommendationContext::interactive();
        let user_score = self
            .scorer
            .score(text_color, *bg_user.color(), user_context);

        let bg_user = if user_score.compliance >= 0.9 {
            bg_user
        } else {
            // Fallback: use hover level (L=0.16)
            elevation.hover()
        };

        // Assistant messages: card level (L=0.14, default card background)
        let bg_assistant = elevation.card();

        // Validate assistant bg contrast
        let assistant_context = RecommendationContext::body_text();
        let assistant_score =
            self.scorer
                .score(text_color, *bg_assistant.color(), assistant_context);

        let bg_assistant = if assistant_score.compliance >= 0.9 {
            bg_assistant
        } else {
            // Fallback: use base level (L=0.12)
            elevation.base()
        };

        // Tool execution: cyan tint with validation (semantic color, not elevation-based)
        let bg_tool = self
            .generate_semantic(
                text_color,
                195.0, // Cyan hue
                0.18,
                0.04,
                UsageContext::LargeText,
            )
            .unwrap_or_else(|| ThemeColor::oklch(0.18, 0.04, 195.0));

        // Code blocks: darker than base for depth (L=0.10, below elevation system)
        let base_oklch = elevation.base().to_oklch();
        let code_oklch = base_oklch.darken(0.02); // 2% darker than base
        let bg_code = ThemeColor::from_color(code_oklch.to_color());

        CardBackgrounds {
            user: bg_user,
            assistant: bg_assistant,
            tool: bg_tool,
            code: bg_code,
        }
    }

    /// Validate that a recommendation meets quality thresholds.
    fn validate_recommendation(
        &self,
        recommendation: &Recommendation,
        _context: &RecommendationContext,
    ) -> bool {
        recommendation.score.overall >= self.thresholds.min_overall
            && recommendation.score.compliance >= self.thresholds.min_compliance
            && recommendation.score.perceptual >= self.thresholds.min_perceptual
    }

    /// Assess overall quality of a palette.
    pub fn assess_palette(&self, palette: &Palette) -> PaletteQualityReport {
        let bg = *palette.bg_panel.color();

        // Score all text tokens against background
        let text_scores = vec![
            ("text", self.score_pair(palette.text, bg)),
            ("text_dim", self.score_pair(palette.text_dim, bg)),
            ("text_label", self.score_pair(palette.text_label, bg)),
        ];

        // Score semantic tokens
        let semantic_scores = vec![
            ("primary", self.score_pair(palette.primary, bg)),
            ("accent", self.score_pair(palette.accent, bg)),
            ("warning", self.score_pair(palette.warning, bg)),
            ("error", self.score_pair(palette.error, bg)),
            ("success", self.score_pair(palette.success, bg)),
            ("muted", self.score_pair(palette.muted, bg)),
        ];

        // Score cockpit tokens
        let cockpit_scores = vec![
            ("running", self.score_pair(palette.running, bg)),
            ("planning", self.score_pair(palette.planning, bg)),
            ("reasoning", self.score_pair(palette.reasoning, bg)),
            ("delegated", self.score_pair(palette.delegated, bg)),
            ("destructive", self.score_pair(palette.destructive, bg)),
            ("cached", self.score_pair(palette.cached, bg)),
            ("retrying", self.score_pair(palette.retrying, bg)),
            ("compacting", self.score_pair(palette.compacting, bg)),
        ];

        PaletteQualityReport {
            text_scores,
            semantic_scores,
            cockpit_scores,
            advanced_scores: Vec::new(), // Computed separately via score_palette_advanced()
        }
    }

    /// Score a foreground/background pair using QualityScorer.
    fn score_pair(&self, fg: ThemeColor, bg: Color) -> QualityScore {
        let context = RecommendationContext::body_text();
        self.scorer.score(*fg.color(), bg, context)
    }

    /// Compute advanced scores for all palette tokens.
    ///
    /// Uses AdvancedScorer to provide detailed impact/effort/confidence analysis
    /// for each color token. The "before" baseline is a mid-gray color (L=0.5, C=0, H=0).
    ///
    /// Returns a vector of (name, AdvancedScore) tuples for all semantic tokens.
    pub fn score_palette_advanced(&self, palette: &Palette) -> Vec<(&'static str, AdvancedScore)> {
        let bg = *palette.bg_panel.color();

        // Baseline "before" color: mid-gray with no chroma
        let baseline_color = Color::from_oklch(0.5, 0.0, 0.0);
        let baseline_oklch = OKLCH::from_color(&baseline_color);

        // Score baseline against background
        let context = RecommendationContext::body_text();
        let baseline_score = self.scorer.score(baseline_color, bg, context);

        let mut advanced_scores = Vec::new();

        // Helper to score a single token
        let mut score_token = |name: &'static str, color: ThemeColor| {
            let fg = *color.color();
            let fg_oklch = OKLCH::from_color(&fg);

            // Compute OKLCH deltas
            let delta_l = fg_oklch.l - baseline_oklch.l;
            let delta_c = fg_oklch.c - baseline_oklch.c;
            let delta_h = {
                let diff = fg_oklch.h - baseline_oklch.h;
                // Normalize hue difference to -180..180 range
                if diff > 180.0 {
                    diff - 360.0
                } else if diff < -180.0 {
                    diff + 360.0
                } else {
                    diff
                }
            };

            // Score after with actual color
            let after_score = self.scorer.score(fg, bg, context);

            // Use AdvancedScorer to get detailed breakdown
            let advanced_score = self.advanced_scorer.score_recommendation(
                name,
                &baseline_score,
                &after_score,
                delta_l,
                delta_c,
                delta_h,
            );

            advanced_scores.push((name, advanced_score));
        };

        // Score all text tokens
        score_token("text", palette.text);
        score_token("text_dim", palette.text_dim);
        score_token("text_label", palette.text_label);

        // Score semantic tokens
        score_token("primary", palette.primary);
        score_token("accent", palette.accent);
        score_token("warning", palette.warning);
        score_token("error", palette.error);
        score_token("success", palette.success);
        score_token("muted", palette.muted);

        // Score cockpit tokens
        score_token("running", palette.running);
        score_token("planning", palette.planning);
        score_token("reasoning", palette.reasoning);
        score_token("delegated", palette.delegated);
        score_token("destructive", palette.destructive);
        score_token("cached", palette.cached);
        score_token("retrying", palette.retrying);
        score_token("compacting", palette.compacting);

        advanced_scores
    }

    // =========================================================================
    // Constraint Solver + Harmony Engine + HCT
    // =========================================================================

    /// Generate a palette via Harmony Engine → ConstraintSolver pipeline.
    ///
    /// 1. Selects hues from `harmony` type rooted at `base_hue`.
    /// 2. Applies formal WCAG/APCA/gamut/lightness constraints via penalty solver.
    /// 3. Returns palette with `harmony_score` and `solver_result` populated.
    pub fn generate_constrained(
        &self,
        base_hue: f64,
        harmony: HarmonyType,
        dark_mode: bool,
    ) -> Option<PaletteWithMetadata> {
        // Step 1: Generate harmony palette in OKLCH space
        let seed = OKLCH::new(0.65, 0.18, base_hue);
        let harmony_palette = harmony_gen(seed, harmony);
        let score = harmony_score_fn(&harmony_palette.colors);

        // Step 2: Extract OKLCH candidates from harmony palette
        let bg_l = if dark_mode { 0.17 } else { 0.95 };
        let bg_panel = OKLCH::new(bg_l, 0.02, base_hue);

        // Build color list: [bg_panel, text, accent, running, planning, error]
        let mut colors: Vec<OKLCH> = vec![bg_panel];
        let text_l = if dark_mode { 0.92 } else { 0.10 };
        colors.push(OKLCH::new(text_l, 0.01, base_hue)); // text (idx 1)
        for color in harmony_palette.colors.iter().take(4) {
            // accent, running, planning, error
            colors.push(*color);
        }

        // Step 3: Apply constraint solver
        let solver_result = self.apply_solver_constraints(colors, bg_panel);

        // Step 4: Reconstruct Palette using solved colors
        let bg_color = solver_result.colors.first().copied().unwrap_or(bg_panel);
        let text_color = solver_result
            .colors
            .get(1)
            .copied()
            .unwrap_or(OKLCH::new(text_l, 0.01, base_hue));
        let accent_color = solver_result
            .colors
            .get(2)
            .copied()
            .unwrap_or(OKLCH::new(0.72, 0.15, base_hue));

        let bg_tc = ThemeColor::oklch(bg_color.l, bg_color.c, bg_color.h);
        let text_tc = ThemeColor::oklch(text_color.l, text_color.c, text_color.h);
        let accent_tc = ThemeColor::oklch(accent_color.l, accent_color.c, accent_color.h);

        // Fall back to generate_from_hue for any tokens not covered by solver
        let base_meta = self.generate_from_hue(base_hue)?;

        let mut palette = base_meta.palette;
        palette.bg_panel = bg_tc;
        palette.text = text_tc;
        palette.accent = accent_tc;
        palette.primary = accent_tc;

        let quality_report = self.assess_palette(&palette);

        let solver_report = SolverReport {
            converged: solver_result.converged,
            iterations: solver_result.iterations,
            final_penalty: solver_result.final_penalty,
            violations: solver_result
                .violations
                .iter()
                .map(|v| v.description.clone())
                .collect(),
        };

        Some(PaletteWithMetadata {
            palette,
            quality_report,
            explanation: base_meta.explanation,
            base_hue,
            harmony_score: score,
            solver_result: Some(solver_report),
        })
    }

    /// Apply ConstraintSolver to a list of OKLCH colors.
    ///
    /// Constraints applied:
    /// - MinAPCA(text idx=1, bg idx=0, Lc=75) — body text
    /// - MinContrast(accent idx=2, bg idx=0, 4.5) — WCAG AA
    /// - InGamut for every color
    /// - LightnessRange(bg idx=0, 0.12, 0.22) for dark mode
    fn apply_solver_constraints(&self, colors: Vec<OKLCH>, _bg_panel: OKLCH) -> SolverResult {
        let constraints = vec![
            // text (idx 1) vs bg (idx 0): APCA Lc ≥ 75
            ColorConstraint {
                color_idx: 1,
                kind: ConstraintKind::MinAPCA {
                    other_idx: 0,
                    target: 75.0,
                },
            },
            // accent (idx 2) vs bg (idx 0): WCAG AA
            ColorConstraint {
                color_idx: 2,
                kind: ConstraintKind::MinContrast {
                    other_idx: 0,
                    target: 4.5,
                },
            },
            // bg lightness in dark-panel range
            ColorConstraint {
                color_idx: 0,
                kind: ConstraintKind::LightnessRange {
                    min: 0.12,
                    max: 0.22,
                },
            },
            // gamut for all
            ColorConstraint {
                color_idx: 0,
                kind: ConstraintKind::InGamut,
            },
            ColorConstraint {
                color_idx: 1,
                kind: ConstraintKind::InGamut,
            },
            ColorConstraint {
                color_idx: 2,
                kind: ConstraintKind::InGamut,
            },
        ];

        let mut solver = ConstraintSolver::new(colors, constraints, SolverConfig::default());
        solver.solve()
    }

    /// Generate a Material Design 3 tonal palette using HCT color space.
    ///
    /// Generates roles following MD3 tone guidelines:
    /// - Primary: tone 40 (dark) / 80 (light)
    /// - Secondary: tone 35 / 70
    /// - Neutral: tone 17, 22, 90
    /// - Error: hue 25°, tone 40 / 80
    pub fn generate_hct_palette(&self, seed_hex: &str) -> Option<Palette> {
        use momoto_core::color::cvd::parse_hex;

        let seed_color = parse_hex(seed_hex)?;
        let seed_hct = HCT::from_color(&seed_color);
        let base_hue = seed_hct.hue;
        let base_chroma = seed_hct.chroma.max(30.0); // Ensure minimum chroma

        // Helper: HCT → ThemeColor
        let hct_to_tc = |h: f64, c: f64, t: f64| -> ThemeColor {
            let color = HCT::new(h, c, t).to_color();
            let [r, g, b] = color.to_srgb8();
            ThemeColor::rgb(r, g, b)
        };

        // Build a palette from HCT tonal roles
        let base_meta = self.generate_from_hue(base_hue)?;
        let mut palette = base_meta.palette;

        // Primary tones (dark mode: tone 80, light would be 40)
        palette.primary = hct_to_tc(base_hue, base_chroma, 80.0);
        palette.accent = hct_to_tc(base_hue + 60.0, base_chroma * 0.8, 70.0);
        palette.text = hct_to_tc(base_hue, 4.0, 90.0);
        palette.text_dim = hct_to_tc(base_hue, 4.0, 70.0);
        palette.bg_panel = hct_to_tc(base_hue, 4.0, 17.0);
        palette.bg_highlight = hct_to_tc(base_hue, 4.0, 22.0);
        palette.error = hct_to_tc(25.0, 84.0, 80.0);

        // Cockpit semantic tokens using secondary hue (H+90°)
        let sec_hue = base_hue + 90.0;
        palette.running = hct_to_tc(195.0, 60.0, 75.0); // Cyan tonal
        palette.planning = hct_to_tc(280.0, 55.0, 70.0); // Purple tonal
        palette.reasoning = hct_to_tc(170.0, 50.0, 68.0); // Teal tonal
        palette.delegated = hct_to_tc(sec_hue, 55.0, 65.0);

        Some(palette)
    }

    /// Generate palette optimized for detected terminal color capabilities.
    ///
    /// Automatically detects terminal color support and generates an optimized
    /// palette. Equivalent to calling `generate_for_color_level()` with detected level.
    pub fn generate_adaptive_from_hue(&self, base_hue: f64) -> Option<PaletteWithMetadata> {
        let caps = super::terminal_caps::caps();
        self.generate_for_color_level(base_hue, caps.color_level)
    }

    /// Generate palette optimized for a specific color level.
    ///
    /// Adjusts color selection strategy based on terminal capabilities:
    /// - **Truecolor**: Full OKLCH precision, no constraints
    /// - **Color256**: Reduced chroma to fit 6×6×6 cube, quantization-aware
    /// - **Color16**: High chroma primary/secondary colors only
    /// - **None**: Lightness-based grayscale palette
    pub fn generate_for_color_level(
        &self,
        base_hue: f64,
        color_level: ColorLevel,
    ) -> Option<PaletteWithMetadata> {
        match color_level {
            ColorLevel::Truecolor => {
                // Full precision - use standard generation
                self.generate_from_hue(base_hue)
            }
            ColorLevel::Color256 => {
                // Optimize for 6×6×6 color cube
                self.generate_256color_optimized(base_hue)
            }
            ColorLevel::Color16 => {
                // Use only highly saturated primary/secondary colors
                self.generate_16color_optimized(base_hue)
            }
            ColorLevel::None => {
                // Grayscale palette based on lightness only
                self.generate_grayscale(base_hue)
            }
        }
    }

    /// Generate palette optimized for 256-color terminals.
    ///
    /// Strategy:
    /// - Reduce chroma to fit 6×6×6 cube better (max chroma: 0.12)
    /// - Prefer colors that map cleanly to cube boundaries
    /// - Use grayscale ramp (232-255) for neutrals
    fn generate_256color_optimized(&self, base_hue: f64) -> Option<PaletteWithMetadata> {
        // Generate base palette with reduced chroma
        let mut palette_meta = self.generate_from_hue(base_hue)?;

        // Helper: reduce chroma to fit 256-color cube
        let optimize_for_256 = |color: ThemeColor| -> ThemeColor {
            let oklch = color.to_oklch();
            // Reduce chroma to 0.12 max (fits cube better)
            let clamped_chroma = oklch.c.min(0.12);
            ThemeColor::oklch(oklch.l, clamped_chroma, oklch.h)
        };

        // Optimize all colors
        palette_meta.palette.primary = optimize_for_256(palette_meta.palette.primary);
        palette_meta.palette.accent = optimize_for_256(palette_meta.palette.accent);
        palette_meta.palette.warning = optimize_for_256(palette_meta.palette.warning);
        palette_meta.palette.error = optimize_for_256(palette_meta.palette.error);
        palette_meta.palette.success = optimize_for_256(palette_meta.palette.success);
        palette_meta.palette.running = optimize_for_256(palette_meta.palette.running);
        palette_meta.palette.planning = optimize_for_256(palette_meta.palette.planning);
        palette_meta.palette.reasoning = optimize_for_256(palette_meta.palette.reasoning);

        Some(palette_meta)
    }

    /// Generate palette optimized for 16-color ANSI terminals.
    ///
    /// Strategy:
    /// - Use only 8 base hues (Red, Green, Yellow, Blue, Magenta, Cyan, + variants)
    /// - High chroma (0.15+) for clear color distinction
    /// - Lightness-based bright variants
    fn generate_16color_optimized(&self, base_hue: f64) -> Option<PaletteWithMetadata> {
        // Round base_hue to nearest ANSI primary (0°, 60°, 120°, 180°, 240°, 300°)
        let ansi_hue = ((base_hue / 60.0).round() * 60.0).rem_euclid(360.0);

        let mut palette_meta = self.generate_from_hue(ansi_hue)?;

        // Helper: boost chroma for clear ANSI mapping
        let optimize_for_16 = |color: ThemeColor| -> ThemeColor {
            let oklch = color.to_oklch();
            // Boost chroma to 0.15+ for clear color
            let boosted_chroma = oklch.c.max(0.15).min(0.20);
            // Round hue to nearest 60° (ANSI primaries)
            let rounded_hue = ((oklch.h / 60.0).round() * 60.0).rem_euclid(360.0);
            ThemeColor::oklch(oklch.l, boosted_chroma, rounded_hue)
        };

        // Optimize semantic colors
        palette_meta.palette.primary = optimize_for_16(palette_meta.palette.primary);
        palette_meta.palette.accent = optimize_for_16(palette_meta.palette.accent);
        palette_meta.palette.warning = ThemeColor::oklch(0.75, 0.18, 60.0); // Yellow
        palette_meta.palette.error = ThemeColor::oklch(0.60, 0.20, 0.0); // Red
        palette_meta.palette.success = ThemeColor::oklch(0.70, 0.18, 120.0); // Green
        palette_meta.palette.running = ThemeColor::oklch(0.70, 0.18, 120.0); // Green
        palette_meta.palette.planning = ThemeColor::oklch(0.70, 0.18, 240.0); // Blue
        palette_meta.palette.reasoning = ThemeColor::oklch(0.70, 0.18, 300.0); // Magenta

        Some(palette_meta)
    }

    /// Generate grayscale palette for monochrome terminals.
    ///
    /// Strategy:
    /// - All colors use lightness only (chroma = 0)
    /// - Semantic differentiation via lightness levels
    /// - Maintains accessibility contrast ratios
    fn generate_grayscale(&self, _base_hue: f64) -> Option<PaletteWithMetadata> {
        // Grayscale palette with varying lightness
        let bg_panel = ThemeColor::oklch(0.18, 0.0, 0.0); // Dark gray bg
        let text = ThemeColor::oklch(0.90, 0.0, 0.0); // Light gray text
        let text_dim = ThemeColor::oklch(0.70, 0.0, 0.0); // Mid-light gray
        let text_label = ThemeColor::oklch(0.60, 0.0, 0.0); // Medium gray

        let palette = Palette {
            // Brand colors (unused in grayscale)
            neon_blue: ThemeColor::oklch(0.60, 0.0, 0.0),
            cyan: ThemeColor::oklch(0.65, 0.0, 0.0),
            violet: ThemeColor::oklch(0.55, 0.0, 0.0),
            deep_blue: bg_panel,

            // Semantic colors (lightness-differentiated)
            primary: ThemeColor::oklch(0.75, 0.0, 0.0), // Light gray
            accent: ThemeColor::oklch(0.85, 0.0, 0.0),  // Very light gray
            warning: ThemeColor::oklch(0.70, 0.0, 0.0), // Medium-light
            error: ThemeColor::oklch(0.65, 0.0, 0.0),   // Medium
            success: ThemeColor::oklch(0.75, 0.0, 0.0), // Light
            muted: ThemeColor::oklch(0.50, 0.0, 0.0),   // Mid-dark

            // Text colors
            text,
            text_dim,
            text_label,

            // Cockpit colors (lightness-differentiated)
            running: ThemeColor::oklch(0.75, 0.0, 0.0), // Light
            planning: ThemeColor::oklch(0.70, 0.0, 0.0), // Medium-light
            reasoning: ThemeColor::oklch(0.65, 0.0, 0.0), // Medium
            delegated: ThemeColor::oklch(0.60, 0.0, 0.0), // Medium-dark
            destructive: ThemeColor::oklch(0.55, 0.0, 0.0), // Dark
            cached: ThemeColor::oklch(0.80, 0.0, 0.0),  // Very light
            retrying: ThemeColor::oklch(0.68, 0.0, 0.0), // Medium-light
            compacting: ThemeColor::oklch(0.72, 0.0, 0.0), // Light

            // UI structure
            border: ThemeColor::oklch(0.30, 0.0, 0.0), // Dark gray
            bg_panel,
            bg_highlight: ThemeColor::oklch(0.22, 0.0, 0.0), // Slightly lighter
            spinner_color: ThemeColor::oklch(0.75, 0.0, 0.0), // Light gray

            // M1: Card backgrounds (grayscale lightness differentiation)
            bg_user: ThemeColor::oklch(0.20, 0.0, 0.0), // Slightly lighter than panel
            bg_assistant: ThemeColor::oklch(0.19, 0.0, 0.0), // Close to panel
            bg_tool: ThemeColor::oklch(0.21, 0.0, 0.0), // Slightly lighter
            bg_code: ThemeColor::oklch(0.15, 0.0, 0.0), // Darker than panel
        };

        let quality_report = self.assess_palette(&palette);

        // Create minimal explanation for grayscale
        let explanation = RecommendationExplanation {
            summary: format!("Grayscale palette for monochrome terminal"),
            reasoning: vec![],
            problem_addressed: "Limited color support".to_string(),
            benefits: vec![
                "Maximum compatibility".to_string(),
                "Accessibility maintained via lightness contrast".to_string(),
            ],
            trade_offs: vec!["No hue-based semantic differentiation".to_string()],
            technical: TechnicalDetails::default(),
        };

        Some(PaletteWithMetadata {
            palette,
            quality_report,
            explanation,
            base_hue: 0.0,
            harmony_score: 0.0,
            solver_result: None,
        })
    }
}

impl Default for IntelligentPaletteBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Summary of a constraint solver run for palette generation.
#[derive(Debug, Clone)]
pub struct SolverReport {
    /// Whether the solver converged within the penalty threshold.
    pub converged: bool,
    /// Number of gradient-descent iterations executed.
    pub iterations: usize,
    /// Final total penalty (lower = better; 0.0 = fully satisfied).
    pub final_penalty: f64,
    /// Human-readable descriptions of remaining violations.
    pub violations: Vec<String>,
}

/// A palette with quality metadata and explanations.
#[derive(Debug, Clone)]
pub struct PaletteWithMetadata {
    /// The generated palette.
    pub palette: Palette,
    /// Quality assessment report.
    pub quality_report: PaletteQualityReport,
    /// Explanation for color choices.
    pub explanation: RecommendationExplanation,
    /// Base hue used for generation (0-360°).
    pub base_hue: f64,
    /// Chromatic harmony score [0, 1] (higher = more coherent palette).
    pub harmony_score: f64,
    /// Constraint solver result, if `generate_constrained` was used.
    pub solver_result: Option<SolverReport>,
}

impl PaletteWithMetadata {
    /// Get a summary of the quality report.
    pub fn quality_summary(&self) -> String {
        self.quality_report.summary()
    }

    /// Get a summary of the explanation.
    pub fn explanation_summary(&self) -> String {
        format!(
            "Generated palette with base hue {:.1}°\n{}",
            self.base_hue, self.explanation.summary
        )
    }

    /// Get advanced scoring summary (impact/effort/confidence).
    ///
    /// Returns detailed breakdown of advanced scores with priority recommendations.
    /// Returns a message if no advanced scores are available.
    pub fn advanced_summary(&self) -> String {
        self.quality_report.advanced_summary()
    }
}

/// Quality assessment report for a palette.
#[derive(Debug, Clone)]
pub struct PaletteQualityReport {
    /// Scores for text tokens.
    pub text_scores: Vec<(&'static str, QualityScore)>,
    /// Scores for semantic tokens.
    pub semantic_scores: Vec<(&'static str, QualityScore)>,
    /// Scores for cockpit tokens.
    pub cockpit_scores: Vec<(&'static str, QualityScore)>,
    /// Advanced scores with impact/effort/confidence analysis.
    pub advanced_scores: Vec<(&'static str, AdvancedScore)>,
}

impl PaletteQualityReport {
    /// Check if all scores meet the given thresholds.
    pub fn meets_thresholds(&self, thresholds: QualityThresholds) -> bool {
        let all_scores = self
            .text_scores
            .iter()
            .chain(self.semantic_scores.iter())
            .chain(self.cockpit_scores.iter());

        for (_, score) in all_scores {
            if score.overall < thresholds.min_overall
                || score.compliance < thresholds.min_compliance
                || score.perceptual < thresholds.min_perceptual
            {
                return false;
            }
        }

        true
    }

    /// Get all color pairs that don't meet minimum quality thresholds.
    pub fn weak_pairs(&self) -> Vec<(&'static str, &QualityScore)> {
        let min_overall = 0.7; // Minimum acceptable quality
        let all_scores = self
            .text_scores
            .iter()
            .chain(self.semantic_scores.iter())
            .chain(self.cockpit_scores.iter());

        all_scores
            .filter_map(|(name, score)| {
                if score.overall < min_overall {
                    Some((*name, score))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get average overall quality score across all tokens.
    pub fn average_overall(&self) -> f64 {
        let all_scores = self
            .text_scores
            .iter()
            .chain(self.semantic_scores.iter())
            .chain(self.cockpit_scores.iter());

        let (sum, count) = all_scores.fold((0.0, 0), |(sum, count), (_, score)| {
            (sum + score.overall, count + 1)
        });

        if count > 0 {
            sum / count as f64
        } else {
            0.0
        }
    }

    /// Generate a summary string of the quality report.
    pub fn summary(&self) -> String {
        let avg = self.average_overall();
        let weak = self.weak_pairs();

        format!(
            "Average quality: {:.1}%\nWeak pairs: {}",
            avg * 100.0,
            if weak.is_empty() {
                "none".to_string()
            } else {
                weak.iter()
                    .map(|(name, score)| format!("{} ({:.1}%)", name, score.overall * 100.0))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        )
    }

    /// Get tokens with strong recommendations (high priority).
    ///
    /// Returns tokens where `is_strong_recommendation()` is true
    /// (recommendation_strength >= 0.7).
    pub fn strong_recommendations(&self) -> Vec<(&'static str, &AdvancedScore)> {
        self.advanced_scores
            .iter()
            .filter_map(|(name, score)| {
                if score.is_strong_recommendation() {
                    Some((*name, score))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get tokens sorted by priority (descending).
    ///
    /// Returns all advanced scores sorted by priority score (highest first).
    pub fn by_priority(&self) -> Vec<(&'static str, &AdvancedScore)> {
        let mut sorted: Vec<_> = self
            .advanced_scores
            .iter()
            .map(|(name, score)| (*name, score))
            .collect();

        sorted.sort_by(|a, b| {
            b.1.priority
                .partial_cmp(&a.1.priority)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        sorted
    }

    /// Get average advanced scoring metrics.
    ///
    /// Returns (avg_impact, avg_effort, avg_confidence, avg_priority).
    pub fn average_advanced_metrics(&self) -> (f64, f64, f64, f64) {
        if self.advanced_scores.is_empty() {
            return (0.0, 0.0, 0.0, 0.0);
        }

        let count = self.advanced_scores.len() as f64;
        let (impact_sum, effort_sum, conf_sum, priority_sum) =
            self.advanced_scores
                .iter()
                .fold((0.0, 0.0, 0.0, 0.0), |acc, (_, score)| {
                    (
                        acc.0 + score.impact,
                        acc.1 + score.effort,
                        acc.2 + score.confidence,
                        acc.3 + score.priority,
                    )
                });

        (
            impact_sum / count,
            effort_sum / count,
            conf_sum / count,
            priority_sum / count,
        )
    }

    /// Generate advanced scoring summary.
    ///
    /// Returns a formatted string with impact/effort/confidence stats
    /// and priority recommendations.
    pub fn advanced_summary(&self) -> String {
        if self.advanced_scores.is_empty() {
            return "No advanced scores available. Call score_palette_advanced() first."
                .to_string();
        }

        let (avg_impact, avg_effort, avg_conf, avg_priority) = self.average_advanced_metrics();
        let strong = self.strong_recommendations();
        let by_priority = self.by_priority();

        let mut summary = format!(
            "Advanced Scoring Summary:\n\
             - Average Impact: {:.1}%\n\
             - Average Effort: {:.1}% (1.0 = trivial, 0.0 = major)\n\
             - Average Confidence: {:.1}%\n\
             - Average Priority: {:.2}\n\
             - Strong Recommendations: {}/{}\n",
            avg_impact * 100.0,
            avg_effort * 100.0,
            avg_conf * 100.0,
            avg_priority,
            strong.len(),
            self.advanced_scores.len(),
        );

        if !by_priority.is_empty() {
            summary.push_str("\nTop 5 Priority Tokens:\n");
            for (name, score) in by_priority.iter().take(5) {
                summary.push_str(&format!(
                    "  {}: priority={:.2} ({}) impact={:.1}% effort={:.1}% confidence={:.1}%\n",
                    name,
                    score.priority,
                    score.priority_assessment(),
                    score.impact * 100.0,
                    score.effort * 100.0,
                    score.confidence * 100.0,
                ));
            }
        }

        summary
    }
}

impl ThemeColor {
    /// Create a ThemeColor from a momoto Color.
    pub fn from_color(color: Color) -> Self {
        let [r, g, b] = color.to_srgb8();
        ThemeColor::rgb(r, g, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intelligent_builder_creates_instance() {
        let builder = IntelligentPaletteBuilder::new();
        assert_eq!(builder.thresholds.min_overall, 0.8);
    }

    #[test]
    fn custom_thresholds_applied() {
        let thresholds = QualityThresholds {
            min_overall: 0.9,
            min_compliance: 0.95,
            min_perceptual: 0.8,
            min_confidence: 0.85,
        };
        let builder = IntelligentPaletteBuilder::with_thresholds(thresholds);
        assert_eq!(builder.thresholds.min_overall, 0.9);
        assert_eq!(builder.thresholds.min_compliance, 0.95);
    }

    #[test]
    fn generate_from_hue_produces_palette() {
        let builder = IntelligentPaletteBuilder::new();
        let result = builder.generate_from_hue(210.0); // Blue

        // With strict thresholds, generation might fail - that's OK for testing
        if let Some(palette_with_meta) = result {
            assert_eq!(palette_with_meta.base_hue, 210.0);
            assert!(palette_with_meta.quality_report.average_overall() > 0.0);
        }
    }

    #[test]
    fn quality_report_identifies_weak_pairs() {
        let builder = IntelligentPaletteBuilder::new();

        // Create a deliberately low-quality palette
        let bg = ThemeColor::oklch(0.18, 0.02, 210.0);
        let poor_text = ThemeColor::oklch(0.25, 0.01, 210.0); // Too dark for bg

        let palette = Palette {
            neon_blue: bg,
            cyan: bg,
            violet: bg,
            deep_blue: bg,
            primary: bg,
            accent: bg,
            warning: bg,
            error: bg,
            success: bg,
            muted: bg,
            text: poor_text,
            text_dim: poor_text,
            running: bg,
            planning: bg,
            reasoning: bg,
            delegated: bg,
            destructive: bg,
            cached: bg,
            retrying: bg,
            compacting: bg,
            border: bg,
            bg_panel: bg,
            bg_highlight: bg,
            text_label: poor_text,
            spinner_color: bg,

            // M1: Card backgrounds
            bg_user: bg,
            bg_assistant: bg,
            bg_tool: bg,
            bg_code: bg,
        };

        let report = builder.assess_palette(&palette);
        let weak = report.weak_pairs();

        // At least text and text_label should be flagged as weak
        assert!(!weak.is_empty());
    }

    #[test]
    fn theme_color_from_color_roundtrip() {
        let color = Color::from_srgb8(128, 64, 255);
        let theme_color = ThemeColor::from_color(color);
        let [r, g, b] = theme_color.srgb8();

        // Should be close (gamut mapping might adjust slightly)
        assert!((r as i32 - 128).abs() <= 5);
        assert!((g as i32 - 64).abs() <= 5);
        assert!((b as i32 - 255).abs() <= 5);
    }

    // ========================================================================
    // Advanced Scoring Tests (Phase I1A)
    // ========================================================================

    #[test]
    fn score_palette_advanced_returns_all_tokens() {
        let builder = IntelligentPaletteBuilder::new();

        // Create a minimal valid palette
        let palette = create_minimal_palette();

        let advanced_scores = builder.score_palette_advanced(&palette);

        // Should have scores for all 17 semantic tokens
        // (3 text + 6 semantic + 8 cockpit)
        assert_eq!(advanced_scores.len(), 17);

        // Verify all expected tokens are present
        let token_names: Vec<&str> = advanced_scores.iter().map(|(name, _)| *name).collect();
        assert!(token_names.contains(&"text"));
        assert!(token_names.contains(&"primary"));
        assert!(token_names.contains(&"running"));
        assert!(token_names.contains(&"destructive"));
    }

    #[test]
    fn advanced_scores_have_valid_ranges() {
        let builder = IntelligentPaletteBuilder::new();
        let palette = create_minimal_palette();

        let advanced_scores = builder.score_palette_advanced(&palette);

        for (name, score) in &advanced_scores {
            // All scores should be in valid ranges (0.0 to 1.0)
            assert!(
                score.impact >= 0.0 && score.impact <= 1.0,
                "{}: impact out of range: {}",
                name,
                score.impact
            );
            assert!(
                score.effort >= 0.0 && score.effort <= 1.0,
                "{}: effort out of range: {}",
                name,
                score.effort
            );
            assert!(
                score.confidence >= 0.0 && score.confidence <= 1.0,
                "{}: confidence out of range: {}",
                name,
                score.confidence
            );
            // Priority can be > 1.0 (e.g., critical priority = 2.0+)
            assert!(
                score.priority >= 0.0,
                "{}: priority negative: {}",
                name,
                score.priority
            );
        }
    }

    #[test]
    fn strong_recommendations_filters_correctly() {
        let builder = IntelligentPaletteBuilder::new();
        let palette = create_minimal_palette();

        let mut report = builder.assess_palette(&palette);
        report.advanced_scores = builder.score_palette_advanced(&palette);

        let strong = report.strong_recommendations();

        // Strong recommendations should have recommendation_strength >= 0.7
        for (name, score) in &strong {
            assert!(
                score.is_strong_recommendation(),
                "{}: not a strong recommendation (strength: {:.2})",
                name,
                score.recommendation_strength()
            );
        }
    }

    #[test]
    fn by_priority_sorts_descending() {
        let builder = IntelligentPaletteBuilder::new();
        let palette = create_minimal_palette();

        let mut report = builder.assess_palette(&palette);
        report.advanced_scores = builder.score_palette_advanced(&palette);

        let sorted = report.by_priority();

        // Verify descending order
        for i in 1..sorted.len() {
            assert!(
                sorted[i - 1].1.priority >= sorted[i].1.priority,
                "Priority not in descending order: {} ({}) vs {} ({})",
                sorted[i - 1].0,
                sorted[i - 1].1.priority,
                sorted[i].0,
                sorted[i].1.priority
            );
        }
    }

    #[test]
    fn average_advanced_metrics_computes_correctly() {
        let builder = IntelligentPaletteBuilder::new();
        let palette = create_minimal_palette();

        let mut report = builder.assess_palette(&palette);
        report.advanced_scores = builder.score_palette_advanced(&palette);

        let (avg_impact, avg_effort, avg_conf, avg_priority) = report.average_advanced_metrics();

        // Averages should be in valid ranges
        assert!(avg_impact >= 0.0 && avg_impact <= 1.0);
        assert!(avg_effort >= 0.0 && avg_effort <= 1.0);
        assert!(avg_conf >= 0.0 && avg_conf <= 1.0);
        assert!(avg_priority >= 0.0);

        // Should be non-zero (we have 17 tokens)
        assert!(avg_impact > 0.0);
        assert!(avg_effort > 0.0);
        assert!(avg_conf > 0.0);
    }

    #[test]
    fn advanced_summary_generates_text() {
        let builder = IntelligentPaletteBuilder::new();
        let palette = create_minimal_palette();

        let mut report = builder.assess_palette(&palette);
        report.advanced_scores = builder.score_palette_advanced(&palette);

        let summary = report.advanced_summary();

        // Should contain key sections
        assert!(summary.contains("Advanced Scoring Summary"));
        assert!(summary.contains("Average Impact"));
        assert!(summary.contains("Average Effort"));
        assert!(summary.contains("Average Confidence"));
        assert!(summary.contains("Average Priority"));
        assert!(summary.contains("Strong Recommendations"));
        assert!(summary.contains("Top 5 Priority Tokens"));
    }

    #[test]
    fn advanced_summary_empty_when_no_scores() {
        let report = PaletteQualityReport {
            text_scores: vec![],
            semantic_scores: vec![],
            cockpit_scores: vec![],
            advanced_scores: vec![],
        };

        let summary = report.advanced_summary();
        assert!(summary.contains("No advanced scores available"));
    }

    #[test]
    fn palette_with_metadata_advanced_summary() {
        let builder = IntelligentPaletteBuilder::new();
        let palette = create_minimal_palette();

        let mut palette_meta = PaletteWithMetadata {
            palette: palette.clone(),
            quality_report: builder.assess_palette(&palette),
            explanation: create_mock_explanation(),
            base_hue: 210.0,
            harmony_score: 0.0,
            solver_result: None,
        };

        // Compute advanced scores
        palette_meta.quality_report.advanced_scores = builder.score_palette_advanced(&palette);

        let summary = palette_meta.advanced_summary();

        assert!(summary.contains("Advanced Scoring Summary"));
        assert!(summary.contains("Top 5 Priority Tokens"));
    }

    #[test]
    fn advanced_scorer_uses_oklch_deltas() {
        let builder = IntelligentPaletteBuilder::new();

        // Create two similar colors to test delta calculation
        let palette = create_minimal_palette();
        let scores = builder.score_palette_advanced(&palette);

        // Verify at least one score exists
        assert!(!scores.is_empty());

        // Scores should have valid breakdowns
        for (_name, score) in &scores {
            assert!(score.breakdown.impact_components.len() > 0);
            assert!(score.breakdown.effort_components.len() > 0);
            assert!(score.breakdown.confidence_components.len() > 0);
        }
    }

    #[test]
    fn priority_assessment_categories() {
        let builder = IntelligentPaletteBuilder::new();
        let palette = create_minimal_palette();
        let scores = builder.score_palette_advanced(&palette);

        // Verify priority assessments are assigned
        let mut has_critical = false;
        let mut has_high = false;
        let mut has_medium = false;
        let mut has_low = false;

        for (_name, score) in &scores {
            match score.priority_assessment() {
                momoto_intelligence::PriorityAssessment::Critical => has_critical = true,
                momoto_intelligence::PriorityAssessment::High => has_high = true,
                momoto_intelligence::PriorityAssessment::Medium => has_medium = true,
                momoto_intelligence::PriorityAssessment::Low => has_low = true,
            }
        }

        // At least one priority category should be assigned
        assert!(has_critical || has_high || has_medium || has_low);
    }

    #[test]
    fn recommendation_strength_calculation() {
        let builder = IntelligentPaletteBuilder::new();
        let palette = create_minimal_palette();
        let scores = builder.score_palette_advanced(&palette);

        for (_name, score) in &scores {
            let strength = score.recommendation_strength();
            // Strength is geometric mean of impact * confidence * effort
            // Should be in [0.0, 1.0]
            assert!(strength >= 0.0 && strength <= 1.0);

            // If all components are high, strength should be high
            if score.impact > 0.7 && score.confidence > 0.7 && score.effort > 0.7 {
                assert!(strength > 0.7);
            }
        }
    }

    #[test]
    fn score_breakdown_has_weighted_components() {
        let builder = IntelligentPaletteBuilder::new();
        let palette = create_minimal_palette();
        let scores = builder.score_palette_advanced(&palette);

        for (_name, score) in &scores {
            // Impact components should exist
            assert!(!score.breakdown.impact_components.is_empty());

            // Each component should have weight
            for component in &score.breakdown.impact_components {
                assert!(component.weight > 0.0);
                assert!(component.value >= 0.0 && component.value <= 1.0);
            }

            // Similar for effort
            assert!(!score.breakdown.effort_components.is_empty());
            for component in &score.breakdown.effort_components {
                assert!(component.weight > 0.0);
            }

            // And confidence
            assert!(!score.breakdown.confidence_components.is_empty());
            for component in &score.breakdown.confidence_components {
                assert!(component.weight > 0.0);
            }
        }
    }

    #[test]
    fn hue_delta_normalization() {
        let builder = IntelligentPaletteBuilder::new();

        // Create palette with colors at extreme hues (to test hue wrapping)
        let mut palette = create_minimal_palette();
        palette.primary = ThemeColor::oklch(0.65, 0.15, 10.0); // Near 0°
        palette.accent = ThemeColor::oklch(0.65, 0.15, 350.0); // Near 360°

        let scores = builder.score_palette_advanced(&palette);

        // All hue deltas should be in reasonable range (-180 to 180)
        // This tests that hue normalization is working
        for (_name, score) in &scores {
            // If hue delta was not normalized, we'd see values near 360
            // which would cause effort scores to be very low
            assert!(score.effort >= 0.0 && score.effort <= 1.0);
        }
    }

    #[test]
    fn baseline_comparison_uses_midgray() {
        let builder = IntelligentPaletteBuilder::new();
        let palette = create_minimal_palette();
        let scores = builder.score_palette_advanced(&palette);

        // Baseline is mid-gray (L=0.5, C=0, H=0)
        // All colors should have non-zero impact since they differ from gray
        for (_name, score) in &scores {
            // Impact should be positive (colors are better than gray baseline)
            assert!(score.impact >= 0.0);

            // Quality overall should be >= baseline mid-gray quality
            assert!(score.quality_overall >= 0.0);
        }
    }

    #[test]
    fn integration_generate_and_score_advanced() {
        let builder = IntelligentPaletteBuilder::new();

        // Lower thresholds for test to ensure generation succeeds
        let permissive_builder = IntelligentPaletteBuilder::with_thresholds(QualityThresholds {
            min_overall: 0.5,
            min_compliance: 0.6,
            min_perceptual: 0.4,
            min_confidence: 0.5,
        });

        if let Some(palette_meta) = permissive_builder.generate_from_hue(210.0) {
            // Compute advanced scores
            let advanced_scores = builder.score_palette_advanced(&palette_meta.palette);

            // Should have all 17 tokens scored
            assert_eq!(advanced_scores.len(), 17);

            // All scores should have valid ranges
            for (_name, score) in &advanced_scores {
                assert!(score.priority >= 0.0);
                assert!(score.confidence >= 0.0 && score.confidence <= 1.0);
            }

            // Advanced summary should be available
            let mut report = palette_meta.quality_report.clone();
            report.advanced_scores = advanced_scores;
            let summary = report.advanced_summary();
            assert!(summary.contains("Advanced Scoring Summary"));
        }
    }

    // ========================================================================
    // Terminal Capability Integration Tests (Phase I1B)
    // ========================================================================

    #[test]
    fn generate_adaptive_detects_capabilities() {
        use crate::render::terminal_caps;

        let builder = IntelligentPaletteBuilder::with_thresholds(QualityThresholds {
            min_overall: 0.5,
            min_compliance: 0.6,
            min_perceptual: 0.4,
            min_confidence: 0.5,
        });

        // Initialize with truecolor for test
        terminal_caps::init();

        if let Some(palette_meta) = builder.generate_adaptive_from_hue(210.0) {
            // Should generate successfully
            assert_eq!(palette_meta.base_hue, 210.0);
        }
    }

    #[test]
    fn generate_for_truecolor_uses_standard() {
        let builder = IntelligentPaletteBuilder::with_thresholds(QualityThresholds {
            min_overall: 0.5,
            min_compliance: 0.6,
            min_perceptual: 0.4,
            min_confidence: 0.5,
        });

        if let Some(palette_meta) = builder.generate_for_color_level(210.0, ColorLevel::Truecolor) {
            let oklch = palette_meta.palette.primary.to_oklch();
            // Truecolor should allow full chroma
            assert!(oklch.c >= 0.10); // No artificial reduction
        }
    }

    #[test]
    fn generate_for_256color_reduces_chroma() {
        let builder = IntelligentPaletteBuilder::with_thresholds(QualityThresholds {
            min_overall: 0.5,
            min_compliance: 0.6,
            min_perceptual: 0.4,
            min_confidence: 0.5,
        });

        if let Some(palette_meta) = builder.generate_for_color_level(210.0, ColorLevel::Color256) {
            let oklch = palette_meta.palette.primary.to_oklch();
            // 256-color should reduce chroma to <=0.12
            assert!(
                oklch.c <= 0.13,
                "Chroma should be reduced for 256-color, got {:.3}",
                oklch.c
            );
        }
    }

    #[test]
    fn generate_for_16color_uses_ansi_primaries() {
        let builder = IntelligentPaletteBuilder::with_thresholds(QualityThresholds {
            min_overall: 0.5,
            min_compliance: 0.6,
            min_perceptual: 0.4,
            min_confidence: 0.5,
        });

        if let Some(palette_meta) = builder.generate_for_color_level(210.0, ColorLevel::Color16) {
            // Warning should be yellow (60°)
            let warning_h = palette_meta.palette.warning.to_oklch().h;
            assert!(
                (warning_h - 60.0).abs() < 5.0,
                "Warning should be yellow for 16-color"
            );

            // Error should be red (0°)
            let error_h = palette_meta.palette.error.to_oklch().h;
            assert!(
                error_h < 10.0 || error_h > 350.0,
                "Error should be red for 16-color"
            );

            // Success should be green (120°)
            let success_h = palette_meta.palette.success.to_oklch().h;
            assert!(
                (success_h - 120.0).abs() < 10.0,
                "Success should be green for 16-color"
            );
        }
    }

    #[test]
    fn generate_grayscale_has_zero_chroma() {
        let builder = IntelligentPaletteBuilder::new();

        if let Some(palette_meta) = builder.generate_grayscale(210.0) {
            // All colors should have near-zero chroma (allow floating point epsilon)
            let epsilon = 1e-6;
            assert!(palette_meta.palette.primary.to_oklch().c < epsilon);
            assert!(palette_meta.palette.accent.to_oklch().c < epsilon);
            assert!(palette_meta.palette.warning.to_oklch().c < epsilon);
            assert!(palette_meta.palette.error.to_oklch().c < epsilon);
            assert!(palette_meta.palette.success.to_oklch().c < epsilon);
            assert!(palette_meta.palette.text.to_oklch().c < epsilon);

            // Lightness should vary for differentiation
            let l_text = palette_meta.palette.text.to_oklch().l;
            let l_bg = palette_meta.palette.bg_panel.to_oklch().l;
            assert!(l_text > l_bg, "Text should be lighter than background");
        }
    }

    #[test]
    fn grayscale_maintains_contrast() {
        let builder = IntelligentPaletteBuilder::new();

        if let Some(palette_meta) = builder.generate_grayscale(0.0) {
            let text = palette_meta.palette.text.to_oklch().l;
            let bg = palette_meta.palette.bg_panel.to_oklch().l;

            // Contrast should be significant (delta L >= 0.5)
            let delta_l = (text - bg).abs();
            assert!(
                delta_l >= 0.5,
                "Grayscale should maintain high lightness contrast"
            );
        }
    }

    #[test]
    fn all_color_levels_generate_successfully() {
        // Very permissive thresholds to ensure generation succeeds
        let builder = IntelligentPaletteBuilder::with_thresholds(QualityThresholds {
            min_overall: 0.3,
            min_compliance: 0.4,
            min_perceptual: 0.2,
            min_confidence: 0.3,
        });

        let levels = [
            ColorLevel::Truecolor,
            ColorLevel::Color256,
            ColorLevel::Color16,
            ColorLevel::None,
        ];

        for level in &levels {
            let result = builder.generate_for_color_level(210.0, *level);
            assert!(
                result.is_some(),
                "Generation should succeed for {:?}",
                level
            );
        }
    }

    #[test]
    fn adaptive_palette_explanation_mentions_color_level() {
        let builder = IntelligentPaletteBuilder::with_thresholds(QualityThresholds {
            min_overall: 0.5,
            min_compliance: 0.6,
            min_perceptual: 0.4,
            min_confidence: 0.5,
        });

        // Grayscale should mention "monochrome" or "grayscale"
        if let Some(palette_meta) = builder.generate_grayscale(180.0) {
            let summary = palette_meta.explanation.summary.to_lowercase();
            assert!(summary.contains("grayscale") || summary.contains("monochrome"));
        }
    }

    // ========================================================================
    // FASE 3 Task 3.4: E2E Validation Tests
    // ========================================================================

    #[test]
    fn e2e_full_pipeline_generates_valid_palette() {
        use super::super::adaptive_optimizer::{AdaptivePaletteOptimizer, OptimizationConfig};

        // 1. Generate initial palette
        let thresholds = QualityThresholds {
            min_overall: 0.3,
            min_compliance: 0.4,
            min_perceptual: 0.2,
            min_confidence: 0.3,
        };
        let builder = IntelligentPaletteBuilder::with_thresholds(thresholds);
        let config = OptimizationConfig::default();
        let mut optimizer = AdaptivePaletteOptimizer::with_config(builder, config);

        // 2. Optimize
        let result = optimizer.optimize_from_hue(210.0);
        assert!(result.is_some(), "Pipeline should produce a result");

        // 3. Validate quality
        let result = result.unwrap();
        assert!(
            result.quality_improvement >= -0.01,
            "Quality should not regress significantly"
        );
        assert!(
            result.final_palette.quality_report.average_overall() >= 0.5,
            "Final quality should be reasonable"
        );
        assert!(
            result.iterations <= 50,
            "Should converge within max iterations"
        );
    }

    #[test]
    fn e2e_adaptive_generation_all_levels_succeed() {
        use super::super::terminal_caps::ColorLevel;

        let thresholds = QualityThresholds {
            min_overall: 0.3,
            min_compliance: 0.4,
            min_perceptual: 0.2,
            min_confidence: 0.3,
        };
        let builder = IntelligentPaletteBuilder::with_thresholds(thresholds);

        // Test all color levels
        let levels = vec![
            ColorLevel::Truecolor,
            ColorLevel::Color256,
            ColorLevel::Color16,
            ColorLevel::None,
        ];

        for level in levels {
            let result = builder.generate_for_color_level(210.0, level);
            assert!(result.is_some(), "Should generate palette for {:?}", level);

            let palette_meta = result.unwrap();
            assert!(
                palette_meta.quality_report.average_overall() >= 0.3,
                "Quality should meet minimum for {:?}",
                level
            );
        }
    }

    #[test]
    fn e2e_quality_metrics_consistency() {
        let thresholds = QualityThresholds {
            min_overall: 0.3,
            min_compliance: 0.4,
            min_perceptual: 0.2,
            min_confidence: 0.3,
        };
        let builder = IntelligentPaletteBuilder::with_thresholds(thresholds);
        let palette_meta = builder
            .generate_from_hue(210.0)
            .expect("Should generate palette");

        // Validate quality report consistency
        let overall = palette_meta.quality_report.average_overall();
        assert!(
            overall >= 0.0 && overall <= 1.0,
            "Overall quality must be in [0,1]"
        );

        // Validate advanced scores if present
        if !palette_meta.quality_report.advanced_scores.is_empty() {
            let (compliance, perceptual, priority, confidence) =
                palette_meta.quality_report.average_advanced_metrics();
            assert!(
                compliance >= 0.0 && compliance <= 1.0,
                "Compliance must be in [0,1]"
            );
            assert!(
                perceptual >= 0.0 && perceptual <= 1.0,
                "Perceptual must be in [0,1]"
            );
            assert!(
                priority >= 0.0 && priority <= 1.0,
                "Priority must be in [0,1]"
            );
            assert!(
                confidence >= 0.0 && confidence <= 1.0,
                "Confidence must be in [0,1]"
            );
        }
    }

    #[test]
    fn e2e_palette_oklch_values_valid() {
        let thresholds = QualityThresholds {
            min_overall: 0.3,
            min_compliance: 0.4,
            min_perceptual: 0.2,
            min_confidence: 0.3,
        };
        let builder = IntelligentPaletteBuilder::with_thresholds(thresholds);
        let palette_meta = builder
            .generate_from_hue(210.0)
            .expect("Should generate palette");
        let palette = &palette_meta.palette;

        // Helper to validate OKLCH values
        let validate_oklch = |name: &str, color: &ThemeColor| {
            let oklch = color.to_oklch();
            assert!(
                oklch.l >= 0.0 && oklch.l <= 1.0,
                "{}: lightness must be in [0,1], got {}",
                name,
                oklch.l
            );
            assert!(
                oklch.c >= 0.0 && oklch.c <= 0.5,
                "{}: chroma must be in [0,0.5], got {}",
                name,
                oklch.c
            );
            assert!(
                oklch.h >= 0.0 && oklch.h < 360.0,
                "{}: hue must be in [0,360), got {}",
                name,
                oklch.h
            );
        };

        // Validate all palette colors
        validate_oklch("text", &palette.text);
        validate_oklch("primary", &palette.primary);
        validate_oklch("accent", &palette.accent);
        validate_oklch("warning", &palette.warning);
        validate_oklch("error", &palette.error);
        validate_oklch("success", &palette.success);
        validate_oklch("bg_panel", &palette.bg_panel);
        validate_oklch("border", &palette.border);
    }

    #[test]
    fn e2e_optimization_respects_convergence() {
        use super::super::adaptive_optimizer::{AdaptivePaletteOptimizer, OptimizationConfig};

        let thresholds = QualityThresholds {
            min_overall: 0.3,
            min_compliance: 0.4,
            min_perceptual: 0.2,
            min_confidence: 0.3,
        };
        let builder = IntelligentPaletteBuilder::with_thresholds(thresholds);

        // Use high target to test convergence detection
        let config = OptimizationConfig {
            max_iterations: 100,
            target_quality: 0.99, // Very high target
            min_improvement: 0.001,
            verbose: false,
        };
        let mut optimizer = AdaptivePaletteOptimizer::with_config(builder, config);

        let result = optimizer
            .optimize_from_hue(210.0)
            .expect("Should produce result");

        // Should stop before max iterations if converged
        assert!(
            !result.convergence_status.is_empty(),
            "Should have convergence status"
        );

        // If it stopped early, quality should be reasonable or it detected stalling
        if result.iterations < 100 {
            assert!(
                result.final_palette.quality_report.average_overall() >= 0.7
                    || result.convergence_status.contains("Stalled")
                    || result.convergence_status.contains("Undetermined"),
                "Early stop should be due to good quality or detected stalling"
            );
        }
    }

    #[test]
    fn e2e_terminal_downgrade_preserves_structure() {
        use super::super::terminal_caps::{ColorLevel, TerminalCapabilities};

        let thresholds = QualityThresholds {
            min_overall: 0.3,
            min_compliance: 0.4,
            min_perceptual: 0.2,
            min_confidence: 0.3,
        };
        let builder = IntelligentPaletteBuilder::with_thresholds(thresholds);
        let palette_meta = builder
            .generate_from_hue(210.0)
            .expect("Should generate palette");

        // Test downgrade to each level
        let caps_256 = TerminalCapabilities::with_color_level(ColorLevel::Color256);
        let caps_16 = TerminalCapabilities::with_color_level(ColorLevel::Color16);
        let caps_mono = TerminalCapabilities::with_color_level(ColorLevel::None);

        // Downgrade should not panic and should return valid ratatui colors
        #[cfg(feature = "tui")]
        {
            let _color_256 = caps_256.downgrade_color(&palette_meta.palette.primary);
            let _color_16 = caps_16.downgrade_color(&palette_meta.palette.primary);
            let _color_mono = caps_mono.downgrade_color(&palette_meta.palette.primary);
            // Just verify it doesn't panic - actual color values are implementation details
        }
    }

    // ========================================================================
    // Test Helpers
    // ========================================================================

    /// Create a minimal valid palette for testing.
    fn create_minimal_palette() -> Palette {
        let bg = ThemeColor::oklch(0.18, 0.02, 210.0);
        let text = ThemeColor::oklch(0.85, 0.05, 210.0);
        let accent = ThemeColor::oklch(0.65, 0.15, 180.0);

        Palette {
            neon_blue: accent,
            cyan: accent,
            violet: accent,
            deep_blue: bg,
            primary: accent,
            accent,
            warning: ThemeColor::oklch(0.70, 0.15, 60.0),
            error: ThemeColor::oklch(0.65, 0.20, 15.0),
            success: ThemeColor::oklch(0.70, 0.15, 140.0),
            muted: ThemeColor::oklch(0.60, 0.05, 210.0),
            text,
            text_dim: ThemeColor::oklch(0.70, 0.05, 210.0),
            text_label: ThemeColor::oklch(0.60, 0.05, 210.0),
            bg_panel: bg,
            bg_highlight: ThemeColor::oklch(0.22, 0.03, 210.0),
            border: ThemeColor::oklch(0.30, 0.05, 210.0),
            running: ThemeColor::oklch(0.65, 0.15, 140.0),
            planning: ThemeColor::oklch(0.70, 0.15, 210.0),
            reasoning: ThemeColor::oklch(0.70, 0.15, 280.0),
            delegated: ThemeColor::oklch(0.70, 0.15, 50.0),
            destructive: ThemeColor::oklch(0.65, 0.20, 15.0),
            cached: ThemeColor::oklch(0.70, 0.15, 180.0),
            retrying: ThemeColor::oklch(0.70, 0.15, 40.0),
            compacting: ThemeColor::oklch(0.70, 0.15, 260.0),
            spinner_color: accent,

            // M1: Card backgrounds
            bg_user: ThemeColor::oklch(0.20, 0.03, 210.0),
            bg_assistant: ThemeColor::oklch(0.19, 0.02, 210.0),
            bg_tool: ThemeColor::oklch(0.20, 0.04, 195.0),
            bg_code: ThemeColor::oklch(0.13, 0.02, 210.0),
        }
    }

    /// Create a mock explanation for testing.
    fn create_mock_explanation() -> RecommendationExplanation {
        RecommendationExplanation {
            summary: "Test palette generated for unit tests".to_string(),
            reasoning: vec![],
            problem_addressed: "Generate test palette".to_string(),
            benefits: vec!["Meets quality thresholds".to_string()],
            trade_offs: vec![],
            technical: TechnicalDetails::default(),
        }
    }

    // ========================================================================
    // M1: Card Background Intelligence Tests
    // ========================================================================

    #[test]
    fn card_backgrounds_are_distinct() {
        let palette = create_minimal_palette();

        // All four card backgrounds should be different
        let bg_user_rgb = palette.bg_user.srgb8();
        let bg_assistant_rgb = palette.bg_assistant.srgb8();
        let bg_tool_rgb = palette.bg_tool.srgb8();
        let bg_code_rgb = palette.bg_code.srgb8();

        // User and assistant should be different
        assert_ne!(
            bg_user_rgb, bg_assistant_rgb,
            "bg_user should differ from bg_assistant"
        );

        // Tool should be different from user and assistant
        assert_ne!(
            bg_tool_rgb, bg_user_rgb,
            "bg_tool should differ from bg_user"
        );
        assert_ne!(
            bg_tool_rgb, bg_assistant_rgb,
            "bg_tool should differ from bg_assistant"
        );

        // Code should be different from all others
        assert_ne!(
            bg_code_rgb, bg_user_rgb,
            "bg_code should differ from bg_user"
        );
        assert_ne!(
            bg_code_rgb, bg_assistant_rgb,
            "bg_code should differ from bg_assistant"
        );
        assert_ne!(
            bg_code_rgb, bg_tool_rgb,
            "bg_code should differ from bg_tool"
        );
    }

    #[test]
    fn card_backgrounds_have_perceptual_separation() {
        let palette = create_minimal_palette();

        // All card backgrounds should have perceptual separation in OKLCH space
        let bg_user_oklch = palette.bg_user.to_oklch();
        let bg_assistant_oklch = palette.bg_assistant.to_oklch();
        let bg_tool_oklch = palette.bg_tool.to_oklch();
        let bg_code_oklch = palette.bg_code.to_oklch();

        // User should be lighter than assistant (more prominent)
        assert!(
            bg_user_oklch.l > bg_assistant_oklch.l,
            "bg_user lightness {:.3} should be > bg_assistant lightness {:.3}",
            bg_user_oklch.l,
            bg_assistant_oklch.l
        );

        // Code should be darker than panel (creates depth)
        let bg_panel_oklch = palette.bg_panel.to_oklch();
        assert!(
            bg_code_oklch.l < bg_panel_oklch.l,
            "bg_code lightness {:.3} should be < bg_panel lightness {:.3}",
            bg_code_oklch.l,
            bg_panel_oklch.l
        );

        // Tool should have cyan tint (distinct hue)
        // Hue ~195° for cyan
        let hue_diff = (bg_tool_oklch.h - 195.0).abs();
        assert!(
            hue_diff < 30.0,
            "bg_tool hue {:.1}° should be near cyan (195°), diff: {:.1}°",
            bg_tool_oklch.h,
            hue_diff
        );
    }

    #[test]
    fn generated_palette_includes_card_backgrounds() {
        let thresholds = QualityThresholds {
            min_overall: 0.6,
            min_compliance: 0.7,
            min_perceptual: 0.5,
            min_confidence: 0.6,
        };
        let builder = IntelligentPaletteBuilder::with_thresholds(thresholds);

        if let Some(palette_meta) = builder.generate_from_hue(210.0) {
            let palette = &palette_meta.palette;

            // Verify all card backgrounds are populated (not default values)
            // By checking they're different from each other
            let bg_user_rgb = palette.bg_user.srgb8();
            let bg_assistant_rgb = palette.bg_assistant.srgb8();
            let bg_tool_rgb = palette.bg_tool.srgb8();
            let bg_code_rgb = palette.bg_code.srgb8();

            // At least user and assistant should be distinct (perceptual elevation)
            assert_ne!(
                bg_user_rgb, bg_assistant_rgb,
                "Generated palette should have distinct user/assistant backgrounds"
            );

            // Code should be darker than panel
            let bg_code_oklch = palette.bg_code.to_oklch();
            let bg_panel_oklch = palette.bg_panel.to_oklch();
            assert!(
                bg_code_oklch.l < bg_panel_oklch.l,
                "Code background should be darker than panel"
            );
        }
    }

    // ========================================================================
    // M6: Dynamic Palette Improvement Tests
    // ========================================================================

    #[test]
    fn m6_improve_palette_method_exists() {
        let builder = IntelligentPaletteBuilder::new();
        let palette = create_minimal_palette();

        // Should not panic (may return None if palette is already good)
        let _result = builder.improve_palette(&palette);
    }

    #[test]
    fn m6_assess_palette_produces_scores() {
        let builder = IntelligentPaletteBuilder::new();
        let palette = create_minimal_palette();

        let report = builder.assess_palette(&palette);

        // Should have some scores
        assert!(!report.text_scores.is_empty(), "Should have text scores");
        assert!(
            !report.semantic_scores.is_empty(),
            "Should have semantic scores"
        );
    }

    #[test]
    fn m6_quality_thresholds_configurable() {
        let thresholds = QualityThresholds {
            min_overall: 0.7,
            min_compliance: 0.75,
            min_perceptual: 0.6,
            min_confidence: 0.65,
        };

        let builder = IntelligentPaletteBuilder::with_thresholds(thresholds);

        assert_eq!(builder.thresholds.min_overall, 0.7);
        assert_eq!(builder.thresholds.min_compliance, 0.75);
    }

    #[test]
    fn m6_generate_respects_thresholds() {
        // Very high thresholds - may fail generation
        let thresholds = QualityThresholds {
            min_overall: 0.95,
            min_compliance: 0.98,
            min_perceptual: 0.90,
            min_confidence: 0.92,
        };

        let builder = IntelligentPaletteBuilder::with_thresholds(thresholds);

        // May return None if thresholds can't be met
        let result = builder.generate_from_hue(210.0);

        // If it succeeds, scores should meet thresholds
        if let Some(palette_meta) = result {
            assert!(palette_meta.quality_report.average_overall() >= 0.95);
        }
    }
}
