//! Color science utilities built on momoto-core and momoto-metrics.
//!
//! Provides WCAG/APCA contrast evaluation, accessible palette generation,
//! and perceptual color distance calculations.

use momoto_core::ContrastMetric;
use momoto_metrics::{APCAMetric, WCAGMetric};

use super::theme::{Palette, ThemeColor};

/// Calculate the WCAG 2.1 contrast ratio between two theme colors.
///
/// Returns a value between 1.0 (identical) and 21.0 (black-on-white).
pub fn contrast_ratio(fg: &ThemeColor, bg: &ThemeColor) -> f64 {
    let wcag = WCAGMetric::new();
    let result = wcag.evaluate(*fg.color(), *bg.color());
    result.value
}

/// Calculate the APCA lightness contrast (Lc) between foreground and background.
///
/// Returns a signed value (-108 to +106). Magnitude indicates contrast strength.
pub fn apca_contrast(fg: &ThemeColor, bg: &ThemeColor) -> f64 {
    let apca = APCAMetric::new();
    let result = apca.evaluate(*fg.color(), *bg.color());
    result.value
}

/// Compute the perceptual distance (delta-E) between two colors in OKLCH space.
pub fn perceptual_distance(a: &ThemeColor, b: &ThemeColor) -> f64 {
    let oklch_a = a.to_oklch();
    let oklch_b = b.to_oklch();
    oklch_a.delta_e(&oklch_b)
}

/// Generate a complete palette from a single OKLCH hue angle (0-360).
///
/// Builds a harmonious color scheme using perceptual color relationships:
/// complementary accent (hue+180), analogous violet (hue+120).
/// Functional colors (warning, error, success) use fixed hues for universal meaning.
pub fn palette_from_hue(hue: f64) -> Palette {
    let brand = ThemeColor::oklch(0.75, 0.15, hue);
    let accent = ThemeColor::oklch(0.85, 0.12, hue + 180.0);

    Palette {
        neon_blue: brand,
        cyan: accent,
        violet: ThemeColor::oklch(0.60, 0.18, hue + 120.0),
        deep_blue: ThemeColor::oklch(0.15, 0.04, hue),

        primary: brand,
        accent,
        warning: ThemeColor::oklch(0.88, 0.18, 95.0),
        error: ThemeColor::oklch(0.62, 0.22, 25.0),
        success: ThemeColor::oklch(0.75, 0.18, 145.0),
        muted: ThemeColor::oklch(0.55, 0.02, hue),
        text: ThemeColor::oklch(0.93, 0.01, hue),
        text_dim: ThemeColor::oklch(0.68, 0.02, hue),
    }
}

/// Adjust a foreground color's lightness to meet a minimum WCAG contrast ratio
/// against the given background.
///
/// Returns the adjusted color. If already accessible, returns a copy of the original.
pub fn ensure_accessible(fg: &ThemeColor, bg: &ThemeColor, min_ratio: f64) -> ThemeColor {
    let current = contrast_ratio(fg, bg);
    if current >= min_ratio {
        return *fg;
    }

    // Determine direction: if bg is dark, lighten fg; if bg is light, darken fg.
    let bg_l = bg.to_oklch().l;
    let mut candidate = *fg;

    for _ in 0..50 {
        candidate = if bg_l < 0.5 {
            candidate.lighten(0.02)
        } else {
            candidate.darken(0.02)
        };
        if contrast_ratio(&candidate, bg) >= min_ratio {
            return candidate;
        }
    }

    // Fallback: if we couldn't reach the target, return best effort.
    candidate
}

/// WCAG conformance level badge for a given contrast ratio.
pub fn wcag_badge(ratio: f64) -> &'static str {
    if ratio >= 7.0 {
        "AAA"
    } else if ratio >= 4.5 {
        "AA"
    } else if ratio >= 3.0 {
        "AA-Lg"
    } else {
        "FAIL"
    }
}

/// Check whether a contrast ratio passes WCAG AA for normal text.
pub fn passes_aa(ratio: f64) -> bool {
    ratio >= 4.5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_white_contrast_near_21() {
        let black = ThemeColor::rgb(0, 0, 0);
        let white = ThemeColor::rgb(255, 255, 255);
        let ratio = contrast_ratio(&black, &white);
        assert!(
            (ratio - 21.0).abs() < 0.1,
            "black/white ratio should be ~21, got {ratio}"
        );
    }

    #[test]
    fn same_color_contrast_near_1() {
        let c = ThemeColor::rgb(128, 128, 128);
        let ratio = contrast_ratio(&c, &c);
        assert!(
            (ratio - 1.0).abs() < 0.01,
            "same color ratio should be ~1, got {ratio}"
        );
    }

    #[test]
    fn apca_black_on_white_positive() {
        let black = ThemeColor::rgb(0, 0, 0);
        let white = ThemeColor::rgb(255, 255, 255);
        let lc = apca_contrast(&black, &white);
        assert!(lc.abs() > 90.0, "APCA B/W should have high Lc, got {lc}");
    }

    #[test]
    fn palette_from_hue_produces_valid_colors() {
        let p = palette_from_hue(210.0); // blue hue
        // All colors should have valid sRGB values (no panics).
        let _ = p.primary.r();
        let _ = p.accent.g();
        let _ = p.warning.b();
        let _ = p.error.fg();
        let _ = p.success.fg();
    }

    #[test]
    fn ensure_accessible_fixes_low_contrast() {
        let dark_bg = ThemeColor::rgb(26, 26, 26); // #1a1a1a
        let low_contrast_fg = ThemeColor::rgb(50, 50, 50); // very dark gray
        let ratio_before = contrast_ratio(&low_contrast_fg, &dark_bg);
        assert!(ratio_before < 4.5, "should start below AA");

        let fixed = ensure_accessible(&low_contrast_fg, &dark_bg, 4.5);
        let ratio_after = contrast_ratio(&fixed, &dark_bg);
        assert!(
            ratio_after >= 4.5,
            "fixed color should pass AA, got {ratio_after}"
        );
    }

    #[test]
    fn ensure_accessible_preserves_passing_color() {
        let dark_bg = ThemeColor::rgb(26, 26, 26);
        let bright_fg = ThemeColor::rgb(230, 237, 243); // already high contrast
        let ratio = contrast_ratio(&bright_fg, &dark_bg);
        assert!(ratio >= 4.5, "should already pass AA");

        let fixed = ensure_accessible(&bright_fg, &dark_bg, 4.5);
        // Should be identical (no adjustment needed).
        assert_eq!(fixed.r(), bright_fg.r());
        assert_eq!(fixed.g(), bright_fg.g());
        assert_eq!(fixed.b(), bright_fg.b());
    }

    #[test]
    fn perceptual_distance_same_is_zero() {
        let c = ThemeColor::oklch(0.5, 0.1, 200.0);
        let d = perceptual_distance(&c, &c);
        assert!(d < 0.001, "same color distance should be ~0, got {d}");
    }

    #[test]
    fn perceptual_distance_different_is_positive() {
        let red = ThemeColor::oklch(0.5, 0.2, 25.0);
        let blue = ThemeColor::oklch(0.5, 0.2, 260.0);
        let d = perceptual_distance(&red, &blue);
        assert!(d > 0.1, "different colors should have distance > 0.1, got {d}");
    }

    #[test]
    fn wcag_badge_levels() {
        assert_eq!(wcag_badge(21.0), "AAA");
        assert_eq!(wcag_badge(7.0), "AAA");
        assert_eq!(wcag_badge(5.0), "AA");
        assert_eq!(wcag_badge(4.5), "AA");
        assert_eq!(wcag_badge(3.5), "AA-Lg");
        assert_eq!(wcag_badge(2.0), "FAIL");
    }

    #[test]
    fn passes_aa_threshold() {
        assert!(passes_aa(4.5));
        assert!(passes_aa(7.0));
        assert!(!passes_aa(4.4));
        assert!(!passes_aa(1.0));
    }
}
