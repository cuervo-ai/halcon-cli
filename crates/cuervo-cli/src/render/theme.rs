//! Terminal design system: color palette, semantic tokens, and theme singleton.
//!
//! The theme provides brand colors (neon blue, cyan, violet) and semantic tokens
//! (primary, accent, error, warning, success, muted) that the rest of the render
//! system uses for consistent visual output.
//!
//! When the `color-science` feature is enabled, colors are backed by momoto-core's
//! perceptual OKLCH color space. Without it, an approximate HSL-based conversion
//! is used instead.

use std::sync::OnceLock;

#[cfg(feature = "color-science")]
use momoto_core::{Color, OKLCH};

use super::color;

/// ANSI reset escape sequence.
pub const RESET: &str = "\x1b[0m";

// ============================================================================
// ThemeColor — momoto-backed (color-science feature enabled)
// ============================================================================

#[cfg(feature = "color-science")]
/// A perceptual color backed by momoto-core's OKLCH color science.
///
/// Wraps `momoto_core::Color` and provides ANSI 24-bit escape sequences
/// for terminal rendering. All palette definitions use OKLCH coordinates
/// for perceptual uniformity.
#[derive(Debug, Clone, Copy)]
pub struct ThemeColor {
    inner: Color,
}

#[cfg(feature = "color-science")]
impl ThemeColor {
    /// Create a theme color from sRGB values (0-255).
    pub fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self {
            inner: Color::from_srgb8(r, g, b),
        }
    }

    /// Create a theme color from OKLCH perceptual coordinates.
    ///
    /// - `l`: Lightness (0.0 = black, 1.0 = white)
    /// - `c`: Chroma (0.0 = gray, ~0.4 = vivid)
    /// - `h`: Hue (0.0 to 360.0 degrees)
    ///
    /// The resulting color is gamut-mapped to sRGB for terminal display.
    pub fn oklch(l: f64, c: f64, h: f64) -> Self {
        let oklch = OKLCH::new(l, c, h);
        let mapped = oklch.map_to_gamut();
        Self {
            inner: mapped.to_color(),
        }
    }

    /// Red channel (0-255).
    #[allow(dead_code)]
    pub fn r(&self) -> u8 {
        self.inner.to_srgb8()[0]
    }

    /// Green channel (0-255).
    #[allow(dead_code)]
    pub fn g(&self) -> u8 {
        self.inner.to_srgb8()[1]
    }

    /// Blue channel (0-255).
    #[allow(dead_code)]
    pub fn b(&self) -> u8 {
        self.inner.to_srgb8()[2]
    }

    /// Access the underlying momoto Color.
    pub fn color(&self) -> &Color {
        &self.inner
    }

    /// Convert to OKLCH perceptual coordinates.
    pub fn to_oklch(self) -> OKLCH {
        self.inner.to_oklch()
    }

    /// Return a lighter version of this color.
    pub fn lighten(&self, delta: f64) -> Self {
        let oklch = self.to_oklch().lighten(delta).map_to_gamut();
        Self {
            inner: oklch.to_color(),
        }
    }

    /// Return a darker version of this color.
    pub fn darken(&self, delta: f64) -> Self {
        let oklch = self.to_oklch().darken(delta).map_to_gamut();
        Self {
            inner: oklch.to_color(),
        }
    }

    /// Return a color with the hue rotated by the given degrees.
    #[allow(dead_code)]
    pub fn rotate_hue(&self, degrees: f64) -> Self {
        let oklch = self.to_oklch().rotate_hue(degrees).map_to_gamut();
        Self {
            inner: oklch.to_color(),
        }
    }

    /// Convert to a ratatui Color for TUI rendering.
    #[cfg(feature = "tui")]
    pub fn to_ratatui_color(&self) -> ratatui::style::Color {
        let [r, g, b] = self.inner.to_srgb8();
        ratatui::style::Color::Rgb(r, g, b)
    }

    /// Return the sRGB8 triple.
    pub fn srgb8(&self) -> [u8; 3] {
        self.inner.to_srgb8()
    }

    /// ANSI 24-bit foreground escape sequence.
    ///
    /// Returns an empty string when color is disabled.
    pub fn fg(&self) -> String {
        if !color::color_enabled() {
            return String::new();
        }
        let [r, g, b] = self.inner.to_srgb8();
        format!("\x1b[38;2;{r};{g};{b}m")
    }

    /// ANSI 24-bit background escape sequence.
    ///
    /// Returns an empty string when color is disabled.
    #[allow(dead_code)]
    pub fn bg(&self) -> String {
        if !color::color_enabled() {
            return String::new();
        }
        let [r, g, b] = self.inner.to_srgb8();
        format!("\x1b[48;2;{r};{g};{b}m")
    }
}

// ============================================================================
// ThemeColor — simple RGB fallback (color-science feature disabled)
// ============================================================================

#[cfg(not(feature = "color-science"))]
/// A terminal color backed by sRGB values.
///
/// When the `color-science` feature is disabled, OKLCH coordinates are
/// approximated via HSL conversion. Perceptual uniformity is not guaranteed.
#[derive(Debug, Clone, Copy)]
pub struct ThemeColor {
    rgb: [u8; 3],
}

#[cfg(not(feature = "color-science"))]
impl ThemeColor {
    /// Create a theme color from sRGB values (0-255).
    pub fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { rgb: [r, g, b] }
    }

    /// Approximate OKLCH → sRGB conversion using HSL as intermediary.
    ///
    /// - `l`: Lightness (0.0 = black, 1.0 = white)
    /// - `c`: Chroma (0.0 = gray, ~0.4 = vivid) — mapped to HSL saturation
    /// - `h`: Hue (0.0 to 360.0 degrees)
    pub fn oklch(l: f64, c: f64, h: f64) -> Self {
        // Map OKLCH chroma to HSL saturation (0..1). Max chroma ~0.4 → S=1.0.
        let s = (c / 0.4).min(1.0).max(0.0);
        let (r, g, b) = hsl_to_rgb(h, s, l);
        Self { rgb: [r, g, b] }
    }

    /// Red channel (0-255).
    #[allow(dead_code)]
    pub fn r(&self) -> u8 {
        self.rgb[0]
    }

    /// Green channel (0-255).
    #[allow(dead_code)]
    pub fn g(&self) -> u8 {
        self.rgb[1]
    }

    /// Blue channel (0-255).
    #[allow(dead_code)]
    pub fn b(&self) -> u8 {
        self.rgb[2]
    }

    /// Return a lighter version of this color.
    #[allow(dead_code)]
    pub fn lighten(&self, delta: f64) -> Self {
        let scale = (1.0 + delta).min(2.0);
        Self {
            rgb: [
                (self.rgb[0] as f64 * scale).min(255.0) as u8,
                (self.rgb[1] as f64 * scale).min(255.0) as u8,
                (self.rgb[2] as f64 * scale).min(255.0) as u8,
            ],
        }
    }

    /// Return a darker version of this color.
    #[allow(dead_code)]
    pub fn darken(&self, delta: f64) -> Self {
        let scale = (1.0 - delta).max(0.0);
        Self {
            rgb: [
                (self.rgb[0] as f64 * scale) as u8,
                (self.rgb[1] as f64 * scale) as u8,
                (self.rgb[2] as f64 * scale) as u8,
            ],
        }
    }

    /// Return a color with the hue rotated by the given degrees.
    #[allow(dead_code)]
    pub fn rotate_hue(&self, _degrees: f64) -> Self {
        // Without color science, return self as a simple fallback.
        *self
    }

    /// Convert to a ratatui Color for TUI rendering.
    pub fn to_ratatui_color(&self) -> ratatui::style::Color {
        let [r, g, b] = self.rgb;
        ratatui::style::Color::Rgb(r, g, b)
    }

    /// Return the sRGB8 triple.
    pub fn srgb8(&self) -> [u8; 3] {
        self.rgb
    }

    /// ANSI 24-bit foreground escape sequence.
    pub fn fg(&self) -> String {
        if !color::color_enabled() {
            return String::new();
        }
        let [r, g, b] = self.rgb;
        format!("\x1b[38;2;{r};{g};{b}m")
    }

    /// ANSI 24-bit background escape sequence.
    #[allow(dead_code)]
    pub fn bg(&self) -> String {
        if !color::color_enabled() {
            return String::new();
        }
        let [r, g, b] = self.rgb;
        format!("\x1b[48;2;{r};{g};{b}m")
    }
}

/// HSL to RGB conversion (hue in degrees, s and l in 0..1).
#[cfg(not(feature = "color-science"))]
fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (u8, u8, u8) {
    let l = l.clamp(0.0, 1.0);
    let s = s.clamp(0.0, 1.0);
    if s < 1e-10 {
        let v = (l * 255.0).round() as u8;
        return (v, v, v);
    }
    let h = ((h % 360.0) + 360.0) % 360.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r1, g1, b1) = match h as u32 {
        0..=59 => (c, x, 0.0),
        60..=119 => (x, c, 0.0),
        120..=179 => (0.0, c, x),
        180..=239 => (0.0, x, c),
        240..=299 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

// ============================================================================
// Palette and Theme (shared — no feature gate needed)
// ============================================================================

/// The complete color palette for a theme.
#[derive(Debug, Clone)]
pub struct Palette {
    // Brand colors (set during construction; not directly read in production)
    #[allow(dead_code)]
    pub neon_blue: ThemeColor,
    #[allow(dead_code)]
    pub cyan: ThemeColor,
    #[allow(dead_code)]
    pub violet: ThemeColor,
    #[allow(dead_code)]
    pub deep_blue: ThemeColor,

    // Semantic colors
    pub primary: ThemeColor,
    pub accent: ThemeColor,
    pub warning: ThemeColor,
    pub error: ThemeColor,
    pub success: ThemeColor,
    pub muted: ThemeColor,
    pub text: ThemeColor,
    pub text_dim: ThemeColor,

    // Cockpit semantic tokens (Phase 42A)
    pub running: ThemeColor,
    pub planning: ThemeColor,
    pub reasoning: ThemeColor,
    pub delegated: ThemeColor,
    pub destructive: ThemeColor,
    pub cached: ThemeColor,
    pub retrying: ThemeColor,
    pub compacting: ThemeColor,
    pub border: ThemeColor,
    pub bg_panel: ThemeColor,
    pub bg_highlight: ThemeColor,
    pub text_label: ThemeColor,
    pub spinner_color: ThemeColor,
}

impl Palette {
    /// Returns the 8 semantic foreground tokens as (name, color) pairs.
    ///
    /// Used by the doctor accessibility section to evaluate contrast
    /// against the terminal background.
    pub fn semantic_pairs(&self) -> Vec<(&'static str, &ThemeColor)> {
        vec![
            ("primary", &self.primary),
            ("accent", &self.accent),
            ("warning", &self.warning),
            ("error", &self.error),
            ("success", &self.success),
            ("muted", &self.muted),
            ("text", &self.text),
            ("text_dim", &self.text_dim),
            ("running", &self.running),
            ("planning", &self.planning),
            ("reasoning", &self.reasoning),
            ("delegated", &self.delegated),
            ("destructive", &self.destructive),
            ("cached", &self.cached),
            ("retrying", &self.retrying),
            ("compacting", &self.compacting),
            ("text_label", &self.text_label),
            ("spinner_color", &self.spinner_color),
        ]
    }
}

/// A named theme with a palette.
#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    pub palette: Palette,
}

/// Global theme singleton.
static THEME: OnceLock<Theme> = OnceLock::new();

/// Initialize the theme system with the given theme name and optional brand color.
///
/// Valid names: "neon" (default), "minimal", "plain".
/// When `brand_hex` is provided (e.g. "#0066cc"), the palette is generated
/// from that hue using OKLCH color science (requires `color-science` feature).
/// Call once at startup; subsequent calls are no-ops.
pub fn init(theme_name: &str, brand_hex: Option<&str>) {
    THEME.get_or_init(|| {
        let palette = if let Some(hex) = brand_hex {
            brand_palette(hex).unwrap_or_else(|| match theme_name {
                "minimal" => minimal_palette(),
                "plain" => plain_palette(),
                _ => neon_palette(),
            })
        } else {
            match theme_name {
                "minimal" => minimal_palette(),
                "plain" => plain_palette(),
                _ => neon_palette(),
            }
        };
        Theme {
            name: theme_name.to_string(),
            palette,
        }
    });
}

/// Get the active theme (initializes with "neon" if not yet set).
pub fn active() -> &'static Theme {
    THEME.get_or_init(|| Theme {
        name: "neon".to_string(),
        palette: neon_palette(),
    })
}

/// ANSI reset string. Returns empty when color is disabled.
pub fn reset() -> &'static str {
    if color::color_enabled() {
        RESET
    } else {
        ""
    }
}

// --- Palette constructors ---

fn neon_palette() -> Palette {
    // OKLCH coordinates calibrated from original RGB hex values.
    let neon_blue = ThemeColor::oklch(0.80, 0.15, 210.0); // #00d4ff-ish
    let cyan = ThemeColor::oklch(0.90, 0.15, 195.0);      // #00ffff-ish
    Palette {
        neon_blue,
        cyan,
        violet: ThemeColor::oklch(0.60, 0.20, 310.0),     // #bf5af2-ish
        deep_blue: ThemeColor::oklch(0.15, 0.05, 250.0),  // #0a1628-ish

        primary: neon_blue,
        accent: cyan,
        warning: ThemeColor::oklch(0.88, 0.18, 95.0),     // #ffcc00-ish
        error: ThemeColor::oklch(0.62, 0.22, 25.0),       // #ff3b30-ish
        success: ThemeColor::oklch(0.75, 0.18, 145.0),    // #30d158-ish
        muted: ThemeColor::oklch(0.55, 0.02, 250.0),      // #6e7681-ish
        text: ThemeColor::oklch(0.93, 0.01, 250.0),       // #e6edf3-ish
        text_dim: ThemeColor::oklch(0.68, 0.02, 250.0),   // #8b949e-ish

        // Cockpit semantic tokens
        running: ThemeColor::oklch(0.78, 0.12, 195.0),
        planning: ThemeColor::oklch(0.72, 0.14, 280.0),
        reasoning: ThemeColor::oklch(0.70, 0.10, 170.0),
        delegated: ThemeColor::oklch(0.65, 0.16, 310.0),
        destructive: ThemeColor::oklch(0.58, 0.24, 25.0),
        cached: ThemeColor::oklch(0.75, 0.08, 85.0),
        retrying: ThemeColor::oklch(0.82, 0.15, 60.0),
        compacting: ThemeColor::oklch(0.68, 0.06, 250.0),
        border: ThemeColor::oklch(0.40, 0.03, 250.0),
        bg_panel: ThemeColor::oklch(0.18, 0.02, 250.0),
        bg_highlight: ThemeColor::oklch(0.22, 0.04, 250.0),
        text_label: ThemeColor::oklch(0.60, 0.04, 250.0),
        spinner_color: ThemeColor::oklch(0.85, 0.12, 195.0),
    }
}

fn minimal_palette() -> Palette {
    let muted_blue = ThemeColor::oklch(0.66, 0.12, 255.0); // cornflower-ish
    let soft_cyan = ThemeColor::oklch(0.80, 0.06, 220.0);
    Palette {
        neon_blue: muted_blue,
        cyan: soft_cyan,
        violet: ThemeColor::oklch(0.62, 0.10, 300.0),
        deep_blue: ThemeColor::oklch(0.20, 0.03, 250.0),

        primary: muted_blue,
        accent: soft_cyan,
        warning: ThemeColor::oklch(0.80, 0.15, 85.0),
        error: ThemeColor::oklch(0.58, 0.18, 20.0),
        success: ThemeColor::oklch(0.70, 0.14, 145.0),
        muted: ThemeColor::oklch(0.58, 0.02, 250.0),
        text: ThemeColor::oklch(0.85, 0.01, 250.0),
        text_dim: ThemeColor::oklch(0.68, 0.01, 250.0),

        // Cockpit semantic tokens (softer than neon)
        running: ThemeColor::oklch(0.72, 0.08, 195.0),
        planning: ThemeColor::oklch(0.66, 0.10, 280.0),
        reasoning: ThemeColor::oklch(0.65, 0.07, 170.0),
        delegated: ThemeColor::oklch(0.60, 0.12, 310.0),
        destructive: ThemeColor::oklch(0.55, 0.18, 25.0),
        cached: ThemeColor::oklch(0.70, 0.06, 85.0),
        retrying: ThemeColor::oklch(0.76, 0.12, 60.0),
        compacting: ThemeColor::oklch(0.62, 0.04, 250.0),
        border: ThemeColor::oklch(0.38, 0.02, 250.0),
        bg_panel: ThemeColor::oklch(0.18, 0.01, 250.0),
        bg_highlight: ThemeColor::oklch(0.22, 0.03, 250.0),
        text_label: ThemeColor::oklch(0.58, 0.03, 250.0),
        spinner_color: ThemeColor::oklch(0.80, 0.08, 195.0),
    }
}

fn plain_palette() -> Palette {
    let neutral = ThemeColor::oklch(0.76, 0.0, 0.0); // pure neutral
    Palette {
        neon_blue: neutral,
        cyan: neutral,
        violet: neutral,
        deep_blue: ThemeColor::oklch(0.25, 0.0, 0.0),

        primary: neutral,
        accent: neutral,
        warning: neutral,
        error: neutral,
        success: neutral,
        muted: ThemeColor::oklch(0.55, 0.0, 0.0),
        text: ThemeColor::oklch(0.83, 0.0, 0.0),
        text_dim: ThemeColor::oklch(0.63, 0.0, 0.0),

        // Cockpit tokens — all neutral in plain mode
        running: neutral,
        planning: neutral,
        reasoning: neutral,
        delegated: neutral,
        destructive: neutral,
        cached: neutral,
        retrying: neutral,
        compacting: neutral,
        border: ThemeColor::oklch(0.40, 0.0, 0.0),
        bg_panel: ThemeColor::oklch(0.18, 0.0, 0.0),
        bg_highlight: ThemeColor::oklch(0.22, 0.0, 0.0),
        text_label: ThemeColor::oklch(0.55, 0.0, 0.0),
        spinner_color: neutral,
    }
}

/// Generate a complete palette from a brand color hex string.
///
/// Requires `color-science` feature for OKLCH-based palette generation.
/// Without it, returns None (falls back to named palette).
#[cfg(feature = "color-science")]
fn brand_palette(hex: &str) -> Option<Palette> {
    let parsed = Color::from_hex(hex).ok()?;
    let hue = parsed.to_oklch().h;

    let mut palette = super::color_science::palette_from_hue(hue);

    // Ensure text tokens meet WCAG AA (4.5:1) against typical dark terminal bg.
    let dark_bg = ThemeColor::rgb(26, 26, 26); // #1a1a1a
    palette.text = super::color_science::ensure_accessible(&palette.text, &dark_bg, 4.5);
    palette.text_dim = super::color_science::ensure_accessible(&palette.text_dim, &dark_bg, 4.5);
    palette.muted = super::color_science::ensure_accessible(&palette.muted, &dark_bg, 3.0);

    Some(palette)
}

#[cfg(not(feature = "color-science"))]
fn brand_palette(_hex: &str) -> Option<Palette> {
    // Without color-science, custom brand palettes are not supported.
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_active_returns_consistent_value() {
        let t1 = active();
        let t2 = active();
        assert_eq!(t1.name, t2.name);
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn neon_palette_has_expected_primary() {
        let p = neon_palette();
        let oklch = p.primary.to_oklch();
        assert!(oklch.l > 0.7, "primary should be bright, got L={}", oklch.l);
        assert!(
            oklch.h > 180.0 && oklch.h < 260.0,
            "primary should be blue-ish, got H={}",
            oklch.h
        );
    }

    #[test]
    fn fg_format_contains_rgb_values() {
        let c = ThemeColor::rgb(255, 128, 64);
        let fg = c.fg();
        if color::color_enabled() {
            assert!(fg.contains("255;128;64"));
        }
    }

    #[test]
    fn bg_format_contains_rgb_values() {
        let c = ThemeColor::rgb(10, 20, 30);
        let bg = c.bg();
        if color::color_enabled() {
            assert!(bg.contains("10;20;30"));
            assert!(bg.starts_with("\x1b[48;2;"));
        }
    }

    #[test]
    fn reset_consistent() {
        let r1 = reset();
        let r2 = reset();
        assert_eq!(r1, r2);
    }

    #[test]
    fn plain_palette_all_neutral() {
        let p = plain_palette();
        assert_eq!(p.primary.r(), p.accent.r());
        assert_eq!(p.primary.r(), p.warning.r());
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn minimal_palette_softer_than_neon() {
        let neon = neon_palette();
        let min = minimal_palette();
        let neon_c = neon.primary.to_oklch().c;
        let min_c = min.primary.to_oklch().c;
        assert!(
            min_c < neon_c,
            "minimal chroma ({min_c}) should be < neon ({neon_c})"
        );
    }

    #[test]
    fn color_disabled_returns_empty() {
        let c = ThemeColor::rgb(255, 0, 0);
        let _ = c.fg();
        let _ = c.bg();
    }

    #[test]
    fn rgb_roundtrip() {
        let c = ThemeColor::rgb(100, 200, 50);
        assert_eq!(c.r(), 100);
        assert_eq!(c.g(), 200);
        assert_eq!(c.b(), 50);
    }

    #[test]
    fn oklch_constructor_gamut_safe() {
        let c = ThemeColor::oklch(0.5, 0.4, 120.0);
        let _ = c.r();
        let _ = c.g();
        let _ = c.b();
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn oklch_black_and_white() {
        let black = ThemeColor::oklch(0.0, 0.0, 0.0);
        assert_eq!(black.r(), 0);
        assert_eq!(black.g(), 0);
        assert_eq!(black.b(), 0);

        let white = ThemeColor::oklch(1.0, 0.0, 0.0);
        assert_eq!(white.r(), 255);
        assert_eq!(white.g(), 255);
        assert_eq!(white.b(), 255);
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn lighten_increases_lightness() {
        let base = ThemeColor::oklch(0.5, 0.1, 200.0);
        let lighter = base.lighten(0.2);
        assert!(
            lighter.to_oklch().l > base.to_oklch().l,
            "lighter should have higher L"
        );
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn darken_decreases_lightness() {
        let base = ThemeColor::oklch(0.5, 0.1, 200.0);
        let darker = base.darken(0.2);
        assert!(
            darker.to_oklch().l < base.to_oklch().l,
            "darker should have lower L"
        );
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn rotate_hue_changes_hue() {
        let base = ThemeColor::oklch(0.5, 0.1, 100.0);
        let rotated = base.rotate_hue(90.0);
        let base_h = base.to_oklch().h;
        let rot_h = rotated.to_oklch().h;
        let diff = (rot_h - base_h).abs();
        assert!(
            (diff - 90.0).abs() < 5.0 || (diff - 270.0).abs() < 5.0,
            "hue should rotate ~90°, got {diff}"
        );
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn to_oklch_roundtrip() {
        let oklch_orig = OKLCH::new(0.6, 0.1, 200.0);
        let c = ThemeColor::oklch(0.6, 0.1, 200.0);
        let oklch_back = c.to_oklch();
        assert!((oklch_orig.l - oklch_back.l).abs() < 0.05);
        assert!((oklch_orig.h - oklch_back.h).abs() < 5.0);
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn color_accessor_returns_inner() {
        let c = ThemeColor::rgb(42, 100, 200);
        let inner = c.color();
        let srgb8 = inner.to_srgb8();
        assert_eq!(srgb8[0], 42);
        assert_eq!(srgb8[1], 100);
        assert_eq!(srgb8[2], 200);
    }

    #[test]
    fn semantic_pairs_includes_cockpit() {
        let p = neon_palette();
        let pairs = p.semantic_pairs();
        // 8 original + 10 cockpit text tokens (border/bg_panel/bg_highlight are non-text)
        assert_eq!(pairs.len(), 18);
        assert_eq!(pairs[0].0, "primary");
        assert_eq!(pairs[7].0, "text_dim");
        // Cockpit tokens start at index 8
        assert_eq!(pairs[8].0, "running");
        assert_eq!(pairs[17].0, "spinner_color");
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn brand_palette_valid_hex() {
        let p = brand_palette("#0066cc");
        assert!(p.is_some(), "valid hex should produce a palette");
        let p = p.unwrap();
        let hue = p.primary.to_oklch().h;
        assert!(hue > 200.0 && hue < 280.0, "brand hue should be blue-ish, got {hue}");
    }

    #[test]
    fn brand_palette_invalid_hex() {
        assert!(brand_palette("not-a-hex").is_none());
        assert!(brand_palette("").is_none());
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn init_with_brand_color() {
        let p = brand_palette("#ff6600").unwrap();
        let hue = p.primary.to_oklch().h;
        assert!(hue > 30.0 && hue < 90.0, "orange brand hue, got {hue}");
    }

    // --- Phase 42A: Cockpit semantic color tests ---

    #[test]
    fn neon_palette_has_cockpit_tokens() {
        let p = neon_palette();
        // All 13 cockpit fields should be accessible without panic.
        let _ = p.running.r();
        let _ = p.planning.g();
        let _ = p.reasoning.b();
        let _ = p.delegated.fg();
        let _ = p.destructive.r();
        let _ = p.cached.r();
        let _ = p.retrying.r();
        let _ = p.compacting.r();
        let _ = p.border.r();
        let _ = p.bg_panel.r();
        let _ = p.bg_highlight.r();
        let _ = p.text_label.r();
        let _ = p.spinner_color.r();
    }

    #[test]
    fn minimal_palette_has_cockpit_tokens() {
        let p = minimal_palette();
        let _ = p.running.r();
        let _ = p.planning.r();
        let _ = p.reasoning.r();
        let _ = p.delegated.r();
        let _ = p.destructive.r();
        let _ = p.cached.r();
        let _ = p.retrying.r();
        let _ = p.compacting.r();
        let _ = p.border.r();
        let _ = p.bg_panel.r();
        let _ = p.bg_highlight.r();
        let _ = p.text_label.r();
        let _ = p.spinner_color.r();
    }

    #[cfg(feature = "tui")]
    #[test]
    fn to_ratatui_color_roundtrip() {
        let c = ThemeColor::rgb(42, 128, 200);
        let rcolor = c.to_ratatui_color();
        assert!(matches!(rcolor, ratatui::style::Color::Rgb(42, 128, 200)));
    }

    #[test]
    fn no_color_returns_empty_escape() {
        // This tests the function exists and doesn't panic; actual behavior depends on NO_COLOR env.
        let c = ThemeColor::rgb(255, 0, 0);
        let _ = c.fg();
        let _ = c.bg();
    }

    #[test]
    fn state_color_mapping_consistent() {
        let p = neon_palette();
        // Running, success, and error should be visually distinct.
        let r = p.running.srgb8();
        let s = p.success.srgb8();
        let e = p.error.srgb8();
        assert_ne!(r, s, "running != success");
        assert_ne!(s, e, "success != error");
        assert_ne!(r, e, "running != error");
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn cockpit_palette_wcag_aa_compliance() {
        let p = neon_palette();
        let failures = crate::render::color_science::validate_cockpit_palette(&p);
        assert!(
            failures.is_empty(),
            "WCAG failures: {failures:?}"
        );
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn cockpit_palette_apca_compliance() {
        let p = neon_palette();
        let bg = &p.bg_panel;
        // Primary text tokens need higher APCA contrast, labels can be dimmer.
        let checks: &[(&str, &ThemeColor, f64)] = &[
            ("text", &p.text, 45.0),
            ("text_dim", &p.text_dim, 30.0),
            ("text_label", &p.text_label, 25.0),
        ];
        for &(name, color, min_lc) in checks {
            let lc = crate::render::color_science::apca_contrast(color, bg).abs();
            assert!(
                lc >= min_lc,
                "APCA {name}: Lc={lc:.1} should be >= {min_lc}"
            );
        }
    }
}
