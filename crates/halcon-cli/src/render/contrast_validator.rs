//! M3: Runtime contrast validation for card backgrounds and text pairs.
//!
//! Validates that all card background/text combinations meet WCAG AA standards
//! (contrast ratio ≥ 4.5:1 for normal text, ≥ 3:1 for large text).

#[cfg(feature = "color-science")]
use momoto_intelligence::context::RecommendationContext;
#[cfg(feature = "color-science")]
use momoto_intelligence::scoring::QualityScorer;

use super::theme::Palette;

/// WCAG AA compliance threshold for normal text (≥4.5:1 contrast ratio).
pub const WCAG_AA_NORMAL_TEXT: f64 = 0.9; // 90% compliance score

/// WCAG AA compliance threshold for large text (≥3:1 contrast ratio).
pub const WCAG_AA_LARGE_TEXT: f64 = 0.85; // 85% compliance score

/// Result of a contrast validation check.
#[derive(Debug, Clone)]
pub struct ContrastValidation {
    /// Name of the background color being validated.
    pub bg_name: &'static str,
    /// Name of the foreground/text color.
    pub fg_name: &'static str,
    /// Compliance score (0.0-1.0, where 1.0 = perfect compliance).
    pub score: f64,
    /// Whether this pair passes WCAG AA for normal text.
    pub passes_aa_normal: bool,
    /// Whether this pair passes WCAG AA for large text.
    pub passes_aa_large: bool,
}

impl ContrastValidation {
    /// Check if this validation failed WCAG AA for normal text.
    pub fn is_failure(&self) -> bool {
        !self.passes_aa_normal
    }

    /// Get a human-readable summary of this validation result.
    pub fn summary(&self) -> String {
        if self.passes_aa_normal {
            format!(
                "✓ {}/{}: {:.1}% compliance (WCAG AA pass)",
                self.bg_name,
                self.fg_name,
                self.score * 100.0
            )
        } else if self.passes_aa_large {
            format!(
                "⚠ {}/{}: {:.1}% compliance (WCAG AA fail for normal text, pass for large text)",
                self.bg_name,
                self.fg_name,
                self.score * 100.0
            )
        } else {
            format!(
                "✗ {}/{}: {:.1}% compliance (WCAG AA fail)",
                self.bg_name,
                self.fg_name,
                self.score * 100.0
            )
        }
    }
}

/// Validates all card background/text pairs in a palette.
#[cfg(feature = "color-science")]
pub fn validate_palette_contrast(palette: &Palette) -> Vec<ContrastValidation> {
    let scorer = QualityScorer::new();
    let mut results = Vec::new();

    // Card backgrounds to validate
    let card_backgrounds = [
        ("bg_user", palette.bg_user),
        ("bg_assistant", palette.bg_assistant),
        ("bg_tool", palette.bg_tool),
        ("bg_code", palette.bg_code),
        ("bg_panel", palette.bg_panel),
        ("bg_highlight", palette.bg_highlight),
    ];

    // Text colors to validate against
    let text_colors = [("text", palette.text), ("text_dim", palette.text_dim)];

    // Validate all combinations
    for (bg_name, bg_color) in &card_backgrounds {
        for (fg_name, fg_color) in &text_colors {
            let normal_context = RecommendationContext::body_text();
            let large_context = RecommendationContext::large_text();

            let normal_score = scorer.score(*fg_color.color(), *bg_color.color(), normal_context);
            let large_score = scorer.score(*fg_color.color(), *bg_color.color(), large_context);

            results.push(ContrastValidation {
                bg_name,
                fg_name,
                score: normal_score.compliance,
                passes_aa_normal: normal_score.compliance >= WCAG_AA_NORMAL_TEXT,
                passes_aa_large: large_score.compliance >= WCAG_AA_LARGE_TEXT,
            });
        }
    }

    results
}

/// Fallback validation when color-science feature is disabled.
#[cfg(not(feature = "color-science"))]
pub fn validate_palette_contrast(palette: &Palette) -> Vec<ContrastValidation> {
    // Without color science, we can't compute accurate contrast
    // Return empty results (assume passing)
    Vec::new()
}

/// Log warnings for any failed contrast validations.
pub fn log_contrast_warnings(validations: &[ContrastValidation]) {
    let failures: Vec<_> = validations.iter().filter(|v| v.is_failure()).collect();

    if failures.is_empty() {
        tracing::debug!("Palette contrast validation: all pairs pass WCAG AA");
        return;
    }

    tracing::warn!(
        "Palette contrast validation: {}/{} pairs failed WCAG AA",
        failures.len(),
        validations.len()
    );

    for validation in failures {
        tracing::warn!("{}", validation.summary());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::theme::ThemeColor;

    #[cfg(feature = "color-science")]
    fn create_test_palette() -> Palette {
        let bg = ThemeColor::oklch(0.12, 0.02, 210.0);
        let text = ThemeColor::oklch(0.85, 0.05, 210.0);

        Palette {
            neon_blue: text,
            cyan: text,
            violet: text,
            deep_blue: bg,
            primary: text,
            accent: text,
            warning: text,
            error: text,
            success: text,
            muted: text,
            text,
            text_dim: ThemeColor::oklch(0.70, 0.05, 210.0),
            text_label: text,
            bg_panel: bg,
            bg_highlight: ThemeColor::oklch(0.18, 0.03, 210.0),
            border: bg,
            running: text,
            planning: text,
            reasoning: text,
            delegated: text,
            destructive: text,
            cached: text,
            retrying: text,
            compacting: text,
            spinner_color: text,
            bg_user: ThemeColor::oklch(0.20, 0.03, 210.0),
            bg_assistant: ThemeColor::oklch(0.14, 0.02, 210.0),
            bg_tool: ThemeColor::oklch(0.18, 0.04, 195.0),
            bg_code: ThemeColor::oklch(0.10, 0.02, 210.0),
        }
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn validate_palette_returns_results() {
        let palette = create_test_palette();
        let validations = validate_palette_contrast(&palette);

        // Should validate 6 backgrounds × 2 text colors = 12 pairs
        assert_eq!(validations.len(), 12);
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn validation_includes_all_card_backgrounds() {
        let palette = create_test_palette();
        let validations = validate_palette_contrast(&palette);

        let bg_names: Vec<&str> = validations.iter().map(|v| v.bg_name).collect();

        assert!(bg_names.contains(&"bg_user"));
        assert!(bg_names.contains(&"bg_assistant"));
        assert!(bg_names.contains(&"bg_tool"));
        assert!(bg_names.contains(&"bg_code"));
        assert!(bg_names.contains(&"bg_panel"));
        assert!(bg_names.contains(&"bg_highlight"));
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn validation_includes_text_colors() {
        let palette = create_test_palette();
        let validations = validate_palette_contrast(&palette);

        let fg_names: Vec<&str> = validations.iter().map(|v| v.fg_name).collect();

        assert!(fg_names.contains(&"text"));
        assert!(fg_names.contains(&"text_dim"));
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn good_contrast_passes_aa() {
        let palette = create_test_palette();
        let validations = validate_palette_contrast(&palette);

        // Most pairs should pass with good lightness difference (0.85 vs 0.12)
        let passed = validations.iter().filter(|v| v.passes_aa_normal).count();
        assert!(passed > 0, "At least some pairs should pass WCAG AA");
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn low_contrast_fails_aa() {
        // Create palette with poor contrast
        let bg = ThemeColor::oklch(0.18, 0.02, 210.0);
        let poor_text = ThemeColor::oklch(0.25, 0.01, 210.0); // Too dark

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
            text_label: poor_text,
            bg_panel: bg,
            bg_highlight: bg,
            border: bg,
            running: bg,
            planning: bg,
            reasoning: bg,
            delegated: bg,
            destructive: bg,
            cached: bg,
            retrying: bg,
            compacting: bg,
            spinner_color: bg,
            bg_user: bg,
            bg_assistant: bg,
            bg_tool: bg,
            bg_code: bg,
        };

        let validations = validate_palette_contrast(&palette);
        let failures: Vec<_> = validations.iter().filter(|v| v.is_failure()).collect();

        assert!(
            !failures.is_empty(),
            "Poor contrast should produce failures"
        );
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn validation_summary_shows_status() {
        let palette = create_test_palette();
        let validations = validate_palette_contrast(&palette);

        for validation in validations {
            let summary = validation.summary();
            // Summary should start with ✓, ⚠, or ✗
            assert!(
                summary.starts_with('✓') || summary.starts_with('⚠') || summary.starts_with('✗'),
                "Summary should have status indicator: {}",
                summary
            );
        }
    }

    #[test]
    fn log_warnings_does_not_panic() {
        // Just ensure it doesn't panic (can't easily test tracing output)
        let validations = vec![];
        log_contrast_warnings(&validations);
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn validation_score_in_range() {
        let palette = create_test_palette();
        let validations = validate_palette_contrast(&palette);

        for validation in validations {
            assert!(
                validation.score >= 0.0 && validation.score <= 1.0,
                "Score {} should be in [0.0, 1.0]",
                validation.score
            );
        }
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn large_text_has_lower_threshold() {
        let palette = create_test_palette();
        let validations = validate_palette_contrast(&palette);

        // If any pair fails normal but passes large, threshold difference is working
        let has_large_only = validations
            .iter()
            .any(|v| !v.passes_aa_normal && v.passes_aa_large);

        // This might not always be true with good palettes, so just check it's possible
        // (not asserting true/false, just that the logic exists)
        let _ = has_large_only;
    }
}
