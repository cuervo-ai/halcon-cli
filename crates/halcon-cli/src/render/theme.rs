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
#[derive(Debug, Clone, Copy, PartialEq)]
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

    /// Create a theme color from sRGB8 array [r, g, b].
    pub fn from_srgb8(rgb: [u8; 3]) -> Self {
        Self {
            inner: Color::from_srgb8(rgb[0], rgb[1], rgb[2]),
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

    /// M5: Calculate perceptual mid-point between two colors for borders.
    ///
    /// Returns a color that is perceptually halfway between `self` and `other`
    /// in OKLCH space. This creates borders that are visually balanced and
    /// maintain consistent perceived separation from both colors.
    ///
    /// ## Parameters
    /// - `other`: The second color to interpolate with
    /// - `chroma_factor`: Multiplier for chroma (0.5 = subtle, 1.0 = clear)
    ///
    /// ## Example
    /// ```ignore
    /// let bg = ThemeColor::oklch(0.12, 0.02, 210.0);
    /// let text = ThemeColor::oklch(0.85, 0.05, 210.0);
    ///
    /// let border = bg.border_between(&text, 1.0); // Clear border
    /// let subtle = bg.border_between(&text, 0.5); // Subtle border
    /// ```
    pub fn border_between(&self, other: &ThemeColor, chroma_factor: f64) -> Self {
        let oklch1 = self.to_oklch();
        let oklch2 = other.to_oklch();

        // Average lightness and hue
        let mid_l = (oklch1.l + oklch2.l) / 2.0;
        let mid_h = (oklch1.h + oklch2.h) / 2.0;

        // Average chroma, then apply factor (for subtle vs. clear variants)
        let mid_c = ((oklch1.c + oklch2.c) / 2.0) * chroma_factor;

        let border_oklch = OKLCH::new(mid_l, mid_c, mid_h).map_to_gamut();

        Self {
            inner: border_oklch.to_color(),
        }
    }

    /// M5: Create a subtle border (50% chroma).
    pub fn border_subtle(&self, other: &ThemeColor) -> Self {
        self.border_between(other, 0.5)
    }

    /// M5: Create a clear border (100% chroma).
    pub fn border_clear(&self, other: &ThemeColor) -> Self {
        self.border_between(other, 1.0)
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

    /// Create a theme color from sRGB8 array [r, g, b].
    pub fn from_srgb8(rgb: [u8; 3]) -> Self {
        Self { rgb }
    }

    /// Approximate OKLCH → sRGB conversion using HSL as intermediary.
    ///
    /// - `l`: Lightness (0.0 = black, 1.0 = white)
    /// - `c`: Chroma (0.0 = gray, ~0.4 = vivid) — mapped to HSL saturation
    /// - `h`: Hue (0.0 to 360.0 degrees)
    pub fn oklch(l: f64, c: f64, h: f64) -> Self {
        // Map OKLCH chroma to HSL saturation (0..1). Max chroma ~0.4 → S=1.0.
        let s = (c / 0.4).clamp(0.0, 1.0);
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
    #[cfg(feature = "tui")]
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
// M2: Perceptual Elevation System
// ============================================================================

#[cfg(feature = "color-science")]
/// Perceptual elevation system providing visually uniform background hierarchy.
///
/// Uses OKLCH lightness steps to create 5 elevation levels (0-4) where each
/// level is perceptually +0.02 lightness from the previous. This ensures
/// consistent visual separation regardless of base hue.
///
/// ## Elevation Levels
/// - **Level 0** (base): L=0.12 — Activity zone background (deepest)
/// - **Level 1** (card): L=0.14 — Default card background
/// - **Level 2** (hover): L=0.16 — Hover state
/// - **Level 3** (selected): L=0.18 — Selection highlight
/// - **Level 4** (emphasized): L=0.20 — User messages (highest prominence)
///
/// ## Usage
/// ```text
/// let elevation = ElevationSystem::new(210.0); // Blue base hue
/// let bg_panel = elevation.base();      // Darkest level
/// let bg_card = elevation.card();       // Default card
/// let bg_user = elevation.emphasized(); // Most prominent
/// ```
#[derive(Debug, Clone, Copy)]
pub struct ElevationSystem {
    base_hue: f64,
    base_lightness: f64,
    step: f64,
}

#[cfg(feature = "color-science")]
impl ElevationSystem {
    /// Create elevation system from base hue.
    ///
    /// M7: Base lightness is 0.08, with +0.05 steps per level (more pronounced).
    pub fn new(base_hue: f64) -> Self {
        Self {
            base_hue,
            base_lightness: 0.08,
            step: 0.05,
        }
    }

    /// Create elevation system with custom base lightness and step.
    pub fn with_params(base_hue: f64, base_lightness: f64, step: f64) -> Self {
        Self {
            base_hue,
            base_lightness,
            step,
        }
    }

    /// Get color at elevation level (0-4).
    ///
    /// Each level adds `step` lightness to the base. Level 0 returns the base.
    pub fn level(&self, n: u8) -> ThemeColor {
        let lightness = self.base_lightness + (n as f64 * self.step);
        ThemeColor::oklch(lightness, 0.02, self.base_hue)
    }

    // Semantic accessors for common elevation levels

    /// Level 0: Base panel (L=0.08) — deepest background.
    pub fn base(&self) -> ThemeColor {
        self.level(0)
    }

    /// Level 1: Default card (L=0.13) — standard card background (assistant messages).
    pub fn card(&self) -> ThemeColor {
        self.level(1)
    }

    /// Level 2: Hover state (L=0.18) — interactive hover feedback.
    pub fn hover(&self) -> ThemeColor {
        self.level(2)
    }

    /// Level 3: Selection (L=0.23) — selected item highlight.
    pub fn selected(&self) -> ThemeColor {
        self.level(3)
    }

    /// Level 4: Emphasized (L=0.28) — most prominent (user messages, tools).
    pub fn emphasized(&self) -> ThemeColor {
        self.level(4)
    }
}

#[cfg(not(feature = "color-science"))]
/// Fallback elevation system when color-science feature is disabled.
///
/// Provides approximate lightness steps using direct RGB manipulation.
/// Perceptual uniformity is not guaranteed.
#[derive(Debug, Clone, Copy)]
pub struct ElevationSystem {
    base_rgb: [u8; 3],
    step_value: u8,
}

#[cfg(not(feature = "color-science"))]
impl ElevationSystem {
    /// Create elevation system from base hue (ignored in fallback).
    /// M7: Darker base with larger steps for more pronounced elevation.
    pub fn new(_base_hue: f64) -> Self {
        Self {
            base_rgb: [20, 20, 25], // Darker base for more contrast
            step_value: 13,         // Larger steps (was 5)
        }
    }

    /// Create elevation system with custom base (hue ignored).
    pub fn with_params(_base_hue: f64, _base_lightness: f64, _step: f64) -> Self {
        Self::new(0.0)
    }

    /// Get color at elevation level (0-4).
    ///
    /// Adds `step_value` to each RGB component per level (approximate).
    pub fn level(&self, n: u8) -> ThemeColor {
        let delta = n * self.step_value;
        ThemeColor::rgb(
            self.base_rgb[0].saturating_add(delta),
            self.base_rgb[1].saturating_add(delta),
            self.base_rgb[2].saturating_add(delta),
        )
    }

    pub fn base(&self) -> ThemeColor {
        self.level(0)
    }

    pub fn card(&self) -> ThemeColor {
        self.level(1)
    }

    pub fn hover(&self) -> ThemeColor {
        self.level(2)
    }

    pub fn selected(&self) -> ThemeColor {
        self.level(3)
    }

    pub fn emphasized(&self) -> ThemeColor {
        self.level(4)
    }
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

    // M1: Card backgrounds (scientifically optimal via RecommendationEngine)
    pub bg_user: ThemeColor,
    pub bg_assistant: ThemeColor,
    pub bg_tool: ThemeColor,
    pub bg_code: ThemeColor,
}

impl Palette {
    // Phase 45A: Cached ratatui Color accessors for 60-120 FPS hot path.
    // OnceCell cache stored in RATATUI_CACHE static (preserves Clone on Palette).

    /// Get primary color as ratatui Color (cached).
    #[cfg(all(feature = "tui", feature = "color-science"))]
    pub fn primary_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE
            .primary
            .get_or_init(|| super::terminal_caps::caps().downgrade_color(&self.primary))
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    pub fn primary_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE.primary.get_or_init(|| {
            let [r, g, b] = self.primary.srgb8();
            super::terminal_caps::caps().downgrade_rgb(r, g, b)
        })
    }

    /// Get accent color as ratatui Color (cached).
    #[cfg(all(feature = "tui", feature = "color-science"))]
    pub fn accent_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE
            .accent
            .get_or_init(|| super::terminal_caps::caps().downgrade_color(&self.accent))
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    pub fn accent_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE.accent.get_or_init(|| {
            let [r, g, b] = self.accent.srgb8();
            super::terminal_caps::caps().downgrade_rgb(r, g, b)
        })
    }

    /// Get warning color as ratatui Color (cached).
    #[cfg(all(feature = "tui", feature = "color-science"))]
    pub fn warning_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE
            .warning
            .get_or_init(|| super::terminal_caps::caps().downgrade_color(&self.warning))
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    pub fn warning_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE.warning.get_or_init(|| {
            let [r, g, b] = self.warning.srgb8();
            super::terminal_caps::caps().downgrade_rgb(r, g, b)
        })
    }

    /// Get error color as ratatui Color (cached).
    #[cfg(all(feature = "tui", feature = "color-science"))]
    pub fn error_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE
            .error
            .get_or_init(|| super::terminal_caps::caps().downgrade_color(&self.error))
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    pub fn error_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE.error.get_or_init(|| {
            let [r, g, b] = self.error.srgb8();
            super::terminal_caps::caps().downgrade_rgb(r, g, b)
        })
    }

    /// Get success color as ratatui Color (cached).
    #[cfg(all(feature = "tui", feature = "color-science"))]
    pub fn success_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE
            .success
            .get_or_init(|| super::terminal_caps::caps().downgrade_color(&self.success))
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    pub fn success_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE.success.get_or_init(|| {
            let [r, g, b] = self.success.srgb8();
            super::terminal_caps::caps().downgrade_rgb(r, g, b)
        })
    }

    /// Get muted color as ratatui Color (cached).
    #[cfg(all(feature = "tui", feature = "color-science"))]
    pub fn muted_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE
            .muted
            .get_or_init(|| super::terminal_caps::caps().downgrade_color(&self.muted))
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    pub fn muted_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE.muted.get_or_init(|| {
            let [r, g, b] = self.muted.srgb8();
            super::terminal_caps::caps().downgrade_rgb(r, g, b)
        })
    }

    /// Get text color as ratatui Color (cached).
    #[cfg(all(feature = "tui", feature = "color-science"))]
    pub fn text_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE
            .text
            .get_or_init(|| super::terminal_caps::caps().downgrade_color(&self.text))
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    pub fn text_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE.text.get_or_init(|| {
            let [r, g, b] = self.text.srgb8();
            super::terminal_caps::caps().downgrade_rgb(r, g, b)
        })
    }

    /// Get text_dim color as ratatui Color (cached).
    #[cfg(all(feature = "tui", feature = "color-science"))]
    pub fn text_dim_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE
            .text_dim
            .get_or_init(|| super::terminal_caps::caps().downgrade_color(&self.text_dim))
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    pub fn text_dim_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE.text_dim.get_or_init(|| {
            let [r, g, b] = self.text_dim.srgb8();
            super::terminal_caps::caps().downgrade_rgb(r, g, b)
        })
    }

    /// Get running color as ratatui Color (cached).
    #[cfg(all(feature = "tui", feature = "color-science"))]
    pub fn running_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE
            .running
            .get_or_init(|| super::terminal_caps::caps().downgrade_color(&self.running))
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    pub fn running_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE.running.get_or_init(|| {
            let [r, g, b] = self.running.srgb8();
            super::terminal_caps::caps().downgrade_rgb(r, g, b)
        })
    }

    /// Get planning color as ratatui Color (cached).
    #[cfg(all(feature = "tui", feature = "color-science"))]
    pub fn planning_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE
            .planning
            .get_or_init(|| super::terminal_caps::caps().downgrade_color(&self.planning))
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    pub fn planning_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE.planning.get_or_init(|| {
            let [r, g, b] = self.planning.srgb8();
            super::terminal_caps::caps().downgrade_rgb(r, g, b)
        })
    }

    /// Get reasoning color as ratatui Color (cached).
    #[cfg(all(feature = "tui", feature = "color-science"))]
    pub fn reasoning_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE
            .reasoning
            .get_or_init(|| super::terminal_caps::caps().downgrade_color(&self.reasoning))
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    pub fn reasoning_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE.reasoning.get_or_init(|| {
            let [r, g, b] = self.reasoning.srgb8();
            super::terminal_caps::caps().downgrade_rgb(r, g, b)
        })
    }

    /// Get delegated color as ratatui Color (cached).
    #[cfg(all(feature = "tui", feature = "color-science"))]
    pub fn delegated_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE
            .delegated
            .get_or_init(|| super::terminal_caps::caps().downgrade_color(&self.delegated))
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    pub fn delegated_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE.delegated.get_or_init(|| {
            let [r, g, b] = self.delegated.srgb8();
            super::terminal_caps::caps().downgrade_rgb(r, g, b)
        })
    }

    /// Get destructive color as ratatui Color (cached).
    #[cfg(all(feature = "tui", feature = "color-science"))]
    pub fn destructive_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE
            .destructive
            .get_or_init(|| super::terminal_caps::caps().downgrade_color(&self.destructive))
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    pub fn destructive_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE.destructive.get_or_init(|| {
            let [r, g, b] = self.destructive.srgb8();
            super::terminal_caps::caps().downgrade_rgb(r, g, b)
        })
    }

    /// Get cached color as ratatui Color (cached).
    #[cfg(all(feature = "tui", feature = "color-science"))]
    pub fn cached_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE
            .cached
            .get_or_init(|| super::terminal_caps::caps().downgrade_color(&self.cached))
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    pub fn cached_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE.cached.get_or_init(|| {
            let [r, g, b] = self.cached.srgb8();
            super::terminal_caps::caps().downgrade_rgb(r, g, b)
        })
    }

    /// Get retrying color as ratatui Color (cached).
    #[cfg(all(feature = "tui", feature = "color-science"))]
    pub fn retrying_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE
            .retrying
            .get_or_init(|| super::terminal_caps::caps().downgrade_color(&self.retrying))
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    pub fn retrying_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE.retrying.get_or_init(|| {
            let [r, g, b] = self.retrying.srgb8();
            super::terminal_caps::caps().downgrade_rgb(r, g, b)
        })
    }

    /// Get compacting color as ratatui Color (cached).
    #[cfg(all(feature = "tui", feature = "color-science"))]
    pub fn compacting_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE
            .compacting
            .get_or_init(|| super::terminal_caps::caps().downgrade_color(&self.compacting))
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    pub fn compacting_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE.compacting.get_or_init(|| {
            let [r, g, b] = self.compacting.srgb8();
            super::terminal_caps::caps().downgrade_rgb(r, g, b)
        })
    }

    /// Get border color as ratatui Color (cached).
    #[cfg(all(feature = "tui", feature = "color-science"))]
    pub fn border_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE
            .border
            .get_or_init(|| super::terminal_caps::caps().downgrade_color(&self.border))
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    pub fn border_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE.border.get_or_init(|| {
            let [r, g, b] = self.border.srgb8();
            super::terminal_caps::caps().downgrade_rgb(r, g, b)
        })
    }

    /// Get bg_panel color as ratatui Color (cached).
    #[cfg(all(feature = "tui", feature = "color-science"))]
    pub fn bg_panel_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE
            .bg_panel
            .get_or_init(|| super::terminal_caps::caps().downgrade_color(&self.bg_panel))
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    pub fn bg_panel_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE.bg_panel.get_or_init(|| {
            let [r, g, b] = self.bg_panel.srgb8();
            super::terminal_caps::caps().downgrade_rgb(r, g, b)
        })
    }

    /// Get bg_highlight color as ratatui Color (cached).
    #[cfg(all(feature = "tui", feature = "color-science"))]
    pub fn bg_highlight_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE
            .bg_highlight
            .get_or_init(|| super::terminal_caps::caps().downgrade_color(&self.bg_highlight))
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    pub fn bg_highlight_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE.bg_highlight.get_or_init(|| {
            let [r, g, b] = self.bg_highlight.srgb8();
            super::terminal_caps::caps().downgrade_rgb(r, g, b)
        })
    }

    /// Get text_label color as ratatui Color (cached).
    #[cfg(all(feature = "tui", feature = "color-science"))]
    pub fn text_label_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE
            .text_label
            .get_or_init(|| super::terminal_caps::caps().downgrade_color(&self.text_label))
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    pub fn text_label_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE.text_label.get_or_init(|| {
            let [r, g, b] = self.text_label.srgb8();
            super::terminal_caps::caps().downgrade_rgb(r, g, b)
        })
    }

    /// Get spinner_color as ratatui Color (cached).
    #[cfg(all(feature = "tui", feature = "color-science"))]
    pub fn spinner_color_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE
            .spinner_color
            .get_or_init(|| super::terminal_caps::caps().downgrade_color(&self.spinner_color))
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    pub fn spinner_color_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE.spinner_color.get_or_init(|| {
            let [r, g, b] = self.spinner_color.srgb8();
            super::terminal_caps::caps().downgrade_rgb(r, g, b)
        })
    }

    // M1: Card background ratatui accessors (cached)
    #[cfg(all(feature = "tui", feature = "color-science"))]
    pub fn bg_user_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE
            .bg_user
            .get_or_init(|| super::terminal_caps::caps().downgrade_color(&self.bg_user))
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    pub fn bg_user_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE.bg_user.get_or_init(|| {
            let [r, g, b] = self.bg_user.srgb8();
            super::terminal_caps::caps().downgrade_rgb(r, g, b)
        })
    }

    #[cfg(all(feature = "tui", feature = "color-science"))]
    pub fn bg_assistant_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE
            .bg_assistant
            .get_or_init(|| super::terminal_caps::caps().downgrade_color(&self.bg_assistant))
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    pub fn bg_assistant_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE.bg_assistant.get_or_init(|| {
            let [r, g, b] = self.bg_assistant.srgb8();
            super::terminal_caps::caps().downgrade_rgb(r, g, b)
        })
    }

    #[cfg(all(feature = "tui", feature = "color-science"))]
    pub fn bg_tool_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE
            .bg_tool
            .get_or_init(|| super::terminal_caps::caps().downgrade_color(&self.bg_tool))
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    pub fn bg_tool_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE.bg_tool.get_or_init(|| {
            let [r, g, b] = self.bg_tool.srgb8();
            super::terminal_caps::caps().downgrade_rgb(r, g, b)
        })
    }

    #[cfg(all(feature = "tui", feature = "color-science"))]
    pub fn bg_code_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE
            .bg_code
            .get_or_init(|| super::terminal_caps::caps().downgrade_color(&self.bg_code))
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    pub fn bg_code_ratatui(&self) -> ratatui::style::Color {
        *RATATUI_CACHE.bg_code.get_or_init(|| {
            let [r, g, b] = self.bg_code.srgb8();
            super::terminal_caps::caps().downgrade_rgb(r, g, b)
        })
    }

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

#[cfg(feature = "color-science")]
/// Global adaptive palette for provider health-based color adjustment.
///
/// Phase 45C: Dynamically adjusts palette colors based on provider health state.
/// Initialized with the base theme palette, updated via `set_adaptive_health()`.
static ADAPTIVE_PALETTE: OnceLock<std::sync::RwLock<super::adaptive_palette::AdaptivePalette>> =
    OnceLock::new();

#[cfg(feature = "tui")]
/// Cached ratatui::style::Color conversions for 60-120 FPS hot path.
///
/// Phase 45A: Converts ThemeColor → ratatui::Color once per session,
/// eliminating repeated OKLCH→sRGB conversions in render loops.
struct RatatuiCache {
    primary: OnceLock<ratatui::style::Color>,
    accent: OnceLock<ratatui::style::Color>,
    warning: OnceLock<ratatui::style::Color>,
    error: OnceLock<ratatui::style::Color>,
    success: OnceLock<ratatui::style::Color>,
    muted: OnceLock<ratatui::style::Color>,
    text: OnceLock<ratatui::style::Color>,
    text_dim: OnceLock<ratatui::style::Color>,
    running: OnceLock<ratatui::style::Color>,
    planning: OnceLock<ratatui::style::Color>,
    reasoning: OnceLock<ratatui::style::Color>,
    delegated: OnceLock<ratatui::style::Color>,
    destructive: OnceLock<ratatui::style::Color>,
    cached: OnceLock<ratatui::style::Color>,
    retrying: OnceLock<ratatui::style::Color>,
    compacting: OnceLock<ratatui::style::Color>,
    border: OnceLock<ratatui::style::Color>,
    bg_panel: OnceLock<ratatui::style::Color>,
    bg_highlight: OnceLock<ratatui::style::Color>,
    text_label: OnceLock<ratatui::style::Color>,
    spinner_color: OnceLock<ratatui::style::Color>,
    // M1: Card background cache fields
    bg_user: OnceLock<ratatui::style::Color>,
    bg_assistant: OnceLock<ratatui::style::Color>,
    bg_tool: OnceLock<ratatui::style::Color>,
    bg_code: OnceLock<ratatui::style::Color>,
}

#[cfg(feature = "tui")]
impl RatatuiCache {
    const fn new() -> Self {
        Self {
            primary: OnceLock::new(),
            accent: OnceLock::new(),
            warning: OnceLock::new(),
            error: OnceLock::new(),
            success: OnceLock::new(),
            muted: OnceLock::new(),
            text: OnceLock::new(),
            text_dim: OnceLock::new(),
            running: OnceLock::new(),
            planning: OnceLock::new(),
            reasoning: OnceLock::new(),
            delegated: OnceLock::new(),
            destructive: OnceLock::new(),
            cached: OnceLock::new(),
            retrying: OnceLock::new(),
            compacting: OnceLock::new(),
            border: OnceLock::new(),
            bg_panel: OnceLock::new(),
            bg_highlight: OnceLock::new(),
            text_label: OnceLock::new(),
            spinner_color: OnceLock::new(),
            // M1: Card background cache initialization
            bg_user: OnceLock::new(),
            bg_assistant: OnceLock::new(),
            bg_tool: OnceLock::new(),
            bg_code: OnceLock::new(),
        }
    }
}

#[cfg(feature = "tui")]
/// Global ratatui color cache singleton.
static RATATUI_CACHE: RatatuiCache = RatatuiCache::new();

/// Initialize the theme system with the given theme name and optional brand color.
///
/// Valid names: "neon" (default), "minimal", "plain".
/// When `brand_hex` is provided (e.g. "#0066cc"), the palette is generated
/// from that hue using OKLCH color science (requires `color-science` feature).
/// Call once at startup; subsequent calls are no-ops.
///
/// **Progressive Enhancement (Phase 45)**: Auto-detects terminal capabilities
/// via environment variables (COLORTERM, TERM) and applies color downgrades
/// for limited terminals (256/16/None).
pub fn init(theme_name: &str, brand_hex: Option<&str>) {
    // Auto-detect terminal capabilities (progressive enhancement)
    let _caps = super::terminal_caps::caps();

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

#[cfg(feature = "color-science")]
/// Initialize the adaptive palette system with the current theme's base palette.
///
/// Should be called once after theme initialization. Subsequent calls are no-ops.
/// The adaptive palette starts in Healthy state (using the base palette as-is).
pub fn init_adaptive() {
    ADAPTIVE_PALETTE.get_or_init(|| {
        let base_palette = active().palette.clone();
        let adaptive = super::adaptive_palette::AdaptivePalette::new(base_palette);
        std::sync::RwLock::new(adaptive)
    });
}

#[cfg(feature = "color-science")]
/// Update the adaptive palette based on provider health level.
///
/// This dynamically adjusts the color palette to provide visual feedback:
/// - Healthy: Uses base palette unchanged
/// - Degraded: Applies warning tint (hue shift towards yellow, reduced chroma)
/// - Unhealthy: Applies critical palette (monochrome red-scale)
///
/// Thread-safe: can be called from health monitoring background tasks.
pub fn set_adaptive_health(level: crate::repl::health::HealthLevel) {
    if let Some(adaptive_lock) = ADAPTIVE_PALETTE.get() {
        if let Ok(mut adaptive) = adaptive_lock.write() {
            adaptive.set_health(level);
        }
    }
}

#[cfg(feature = "color-science")]
/// Get the current adaptive palette based on provider health.
///
/// Returns the adjusted palette if health is degraded/unhealthy, or the base
/// palette if healthy or if adaptive palette is not initialized.
///
/// Thread-safe: uses RwLock read guard for concurrent access.
pub fn adaptive_palette() -> Palette {
    if let Some(adaptive_lock) = ADAPTIVE_PALETTE.get() {
        if let Ok(adaptive) = adaptive_lock.read() {
            return adaptive.palette().clone();
        }
    }
    // Fallback to base palette if adaptive not initialized or lock poisoned
    active().palette.clone()
}

#[cfg(all(test, feature = "color-science"))]
/// Reset adaptive palette to Healthy state (test-only helper).
///
/// Needed because ADAPTIVE_PALETTE is a global singleton that persists across
/// tests. Call this at the start of each adaptive palette test to ensure clean state.
pub fn reset_adaptive_for_test() {
    use crate::repl::health::HealthLevel;
    // Initialize if not yet done
    init_adaptive();
    // Reset to Healthy
    set_adaptive_health(HealthLevel::Healthy);
}

// --- Palette constructors ---

fn neon_palette() -> Palette {
    // HALCÓN Identity Palette — Precision. Focus. Predatory Clarity. Silent Power.
    //
    // Design system:
    //   Falcon Blue  — the blade, primary action, sharp focus
    //   Void Black   — deep space backgrounds, no noise
    //   Amber Eye    — the falcon's warm gaze, decisive secondary
    //   Frost        — cool white text, effortless clarity
    //   Carbon       — muted steel surfaces, structural
    //   Emerald      — confident success, alive
    //   Blood        — decisive error, no ambiguity

    // Core identity anchors — HALCÓN design tokens
    // Falcon Blue: the blade, sharp execution, primary action
    let falcon_blue = ThemeColor::oklch(0.80, 0.18, 207.0); // Primary — bright blade (L=0.80 for panel separation)
                                                            // Bright Blade: electric highlight, accent on interaction
    let blade_light = ThemeColor::oklch(0.88, 0.14, 194.0); // Accent — electric, distinct from running
                                                            // Amber Eye: warm decisive secondary, the falcon's gaze
    let amber_eye = ThemeColor::oklch(0.76, 0.17, 62.0); // Delegated — warm, golden
    let frost = ThemeColor::oklch(0.93, 0.006, 220.0); // Text — cool white clarity
    let carbon = ThemeColor::oklch(0.48, 0.022, 232.0); // Muted — subdued steel

    Palette {
        // Legacy field names preserved for compatibility
        neon_blue: falcon_blue,
        cyan: blade_light,
        violet: ThemeColor::oklch(0.58, 0.22, 285.0), // Deep violet — planning
        deep_blue: ThemeColor::oklch(0.07, 0.013, 240.0), // Void Black — deepest space

        // Semantic tokens
        primary: falcon_blue,
        accent: blade_light,
        warning: ThemeColor::oklch(0.85, 0.16, 82.0), // Gold — caution, bright (H=82 far from blade)
        error: ThemeColor::oklch(0.58, 0.26, 27.0),   // Blood — decisive, no ambiguity
        success: ThemeColor::oklch(0.58, 0.22, 142.0), // Emerald — confident, alive (H=142)
        muted: carbon,
        text: frost,
        text_dim: ThemeColor::oklch(0.62, 0.012, 225.0), // Dimmed frost — structural

        // Cockpit semantic tokens — HALCÓN operational states
        // Wide hue coverage (207°/285°/155°/62°) + large ΔL for perceptual separation ≥ 0.3
        running: falcon_blue, // L=0.80, H=207 — blade active
        planning: ThemeColor::oklch(0.58, 0.22, 285.0), // L=0.58, H=285 — deep violet, strategy
        reasoning: ThemeColor::oklch(0.52, 0.20, 155.0), // L=0.52, H=155 — dark teal, intelligence
        delegated: amber_eye, // L=0.76, H=62  — amber warm, handoff
        destructive: ThemeColor::oklch(0.58, 0.26, 27.0), // Blood — danger acknowledged
        cached: ThemeColor::oklch(0.60, 0.06, 222.0), // Steel blue — settled, stored
        retrying: ThemeColor::oklch(0.82, 0.16, 62.0), // Warm amber retrying — persistence
        compacting: ThemeColor::oklch(0.50, 0.04, 235.0), // Blue-grey — background work
        border: ThemeColor::oklch(0.28, 0.022, 237.0), // Carbon border — structural
        bg_panel: ThemeColor::oklch(0.12, 0.016, 240.0), // Carbon panel — elevated surface
        bg_highlight: ThemeColor::oklch(0.18, 0.040, 210.0), // Blue micro-highlight
        text_label: ThemeColor::oklch(0.62, 0.022, 232.0), // Carbon label — ≥4.5:1 on bg_panel
        spinner_color: falcon_blue, // Blade color — active motion

        // Card backgrounds — void hierarchy (darker = deeper)
        bg_user: ThemeColor::oklch(0.11, 0.020, 240.0), // Elevated void — user space
        bg_assistant: ThemeColor::oklch(0.09, 0.015, 240.0), // Deep void — falcon origin
        bg_tool: ThemeColor::oklch(0.13, 0.030, 210.0), // Blue-carbon — tool surface
        bg_code: ThemeColor::oklch(0.08, 0.012, 240.0), // Deepest void — code sanctum
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
        // M1: Card backgrounds (softer than neon)
        bg_user: ThemeColor::oklch(0.15, 0.02, 250.0),
        bg_assistant: ThemeColor::oklch(0.13, 0.01, 250.0),
        bg_tool: ThemeColor::oklch(0.17, 0.03, 195.0),
        bg_code: ThemeColor::oklch(0.12, 0.01, 250.0),
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
        // M1: Card backgrounds (neutral/grayscale)
        bg_user: ThemeColor::oklch(0.16, 0.0, 0.0),
        bg_assistant: ThemeColor::oklch(0.14, 0.0, 0.0),
        bg_tool: ThemeColor::oklch(0.17, 0.0, 0.0),
        bg_code: ThemeColor::oklch(0.13, 0.0, 0.0),
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

    // ===== Progressive Enhancement Tests (Phase 45 - FASE 3.3) =====

    #[test]
    fn progressive_enhancement_initializes_on_theme_init() {
        init("neon", None);

        // Terminal capabilities should be auto-detected
        let caps = super::super::terminal_caps::caps();

        // Should have detected SOME color level (at minimum Color16)
        assert!(matches!(
            caps.color_level,
            super::super::terminal_caps::ColorLevel::Truecolor
                | super::super::terminal_caps::ColorLevel::Color256
                | super::super::terminal_caps::ColorLevel::Color16
                | super::super::terminal_caps::ColorLevel::None
        ));
    }

    #[cfg(all(feature = "tui", feature = "color-science"))]
    #[test]
    fn progressive_enhancement_downgrades_for_limited_terminals() {
        // Use TerminalCapabilities::with_color_level directly to avoid the
        // OnceLock singleton, which may already be initialized with a different
        // color level when other tests run first (test-order dependency fixed).
        let caps_256 = super::super::terminal_caps::TerminalCapabilities::with_color_level(
            super::super::terminal_caps::ColorLevel::Color256,
        );

        let neon_blue = ThemeColor::oklch(0.80, 0.15, 210.0);

        // Downgrade a color and verify it's Indexed, not RGB
        let downgraded = caps_256.downgrade_color(&neon_blue);

        assert!(
            matches!(downgraded, ratatui::style::Color::Indexed(_)),
            "Color256 terminal should downgrade OkLCh to Indexed, got: {:?}",
            downgraded
        );
    }

    #[cfg(all(feature = "tui", feature = "color-science"))]
    #[test]
    fn progressive_enhancement_respects_no_color() {
        // Test that ColorLevel::None downgrades to Reset
        // Note: Cannot force terminal caps due to OnceLock, so test the logic directly
        let caps_none = super::super::terminal_caps::TerminalCapabilities::with_color_level(
            super::super::terminal_caps::ColorLevel::None,
        );

        let red = ThemeColor::rgb(255, 0, 0);

        let downgraded = caps_none.downgrade_color(&red);

        // Should downgrade to Reset (monochrome)
        assert_eq!(downgraded, ratatui::style::Color::Reset);
    }

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
        assert!(
            hue > 200.0 && hue < 280.0,
            "brand hue should be blue-ish, got {hue}"
        );
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
        assert!(failures.is_empty(), "WCAG failures: {failures:?}");
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

    // --- Phase 45A Task 2.1: Ratatui Color Cache Tests ---

    #[cfg(all(feature = "tui", feature = "color-science"))]
    #[test]
    fn ratatui_cache_primary_returns_correct_color() {
        crate::render::terminal_caps::init(); // Ensure caps are initialized
        let p = neon_palette();
        let cached = p.primary_ratatui();
        let direct = crate::render::terminal_caps::caps().downgrade_color(&p.primary);
        assert_eq!(
            cached, direct,
            "cached color should match downgraded conversion"
        );
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    #[test]
    fn ratatui_cache_primary_returns_correct_color() {
        crate::render::terminal_caps::init();
        let p = neon_palette();
        let cached = p.primary_ratatui();
        let [r, g, b] = p.primary.srgb8();
        let direct = crate::render::terminal_caps::caps().downgrade_rgb(r, g, b);
        assert_eq!(
            cached, direct,
            "cached color should match downgraded conversion"
        );
    }

    #[cfg(feature = "tui")]
    #[test]
    fn ratatui_cache_all_21_accessors_work() {
        let p = neon_palette();

        // 8 semantic colors
        let _ = p.primary_ratatui();
        let _ = p.accent_ratatui();
        let _ = p.warning_ratatui();
        let _ = p.error_ratatui();
        let _ = p.success_ratatui();
        let _ = p.muted_ratatui();
        let _ = p.text_ratatui();
        let _ = p.text_dim_ratatui();

        // 13 cockpit colors
        let _ = p.running_ratatui();
        let _ = p.planning_ratatui();
        let _ = p.reasoning_ratatui();
        let _ = p.delegated_ratatui();
        let _ = p.destructive_ratatui();
        let _ = p.cached_ratatui();
        let _ = p.retrying_ratatui();
        let _ = p.compacting_ratatui();
        let _ = p.border_ratatui();
        let _ = p.bg_panel_ratatui();
        let _ = p.bg_highlight_ratatui();
        let _ = p.text_label_ratatui();
        let _ = p.spinner_color_ratatui();
    }

    #[cfg(all(feature = "tui", feature = "color-science"))]
    #[test]
    fn ratatui_cache_returns_same_instance() {
        crate::render::terminal_caps::init();
        let p = neon_palette();

        // Call twice — should get identical values
        let first = p.accent_ratatui();
        let second = p.accent_ratatui();

        assert_eq!(first, second, "cache should return same value");

        // Verify it matches the downgraded color
        let direct = crate::render::terminal_caps::caps().downgrade_color(&p.accent);
        assert_eq!(first, direct);
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    #[test]
    fn ratatui_cache_returns_same_instance() {
        crate::render::terminal_caps::init();
        let p = neon_palette();

        // Call twice — should get identical values
        let first = p.accent_ratatui();
        let second = p.accent_ratatui();

        assert_eq!(first, second, "cache should return same value");

        // Verify it matches the downgraded RGB
        let [r, g, b] = p.accent.srgb8();
        let direct = crate::render::terminal_caps::caps().downgrade_rgb(r, g, b);
        assert_eq!(first, direct);
    }

    #[cfg(all(feature = "tui", feature = "color-science"))]
    #[test]
    fn ratatui_cache_uses_active_theme() {
        crate::render::terminal_caps::init();
        let theme = active();
        let cached_primary = theme.palette.primary_ratatui();
        let direct_primary =
            crate::render::terminal_caps::caps().downgrade_color(&theme.palette.primary);

        assert_eq!(
            cached_primary, direct_primary,
            "cached color should match active theme's downgraded color"
        );

        // Calling again should return the same cached value
        let cached_again = theme.palette.primary_ratatui();
        assert_eq!(cached_primary, cached_again);
    }

    #[cfg(all(feature = "tui", not(feature = "color-science")))]
    #[test]
    fn ratatui_cache_uses_active_theme() {
        crate::render::terminal_caps::init();
        let theme = active();
        let cached_primary = theme.palette.primary_ratatui();
        let [r, g, b] = theme.palette.primary.srgb8();
        let direct_primary = crate::render::terminal_caps::caps().downgrade_rgb(r, g, b);

        assert_eq!(
            cached_primary, direct_primary,
            "cached color should match active theme's downgraded color"
        );

        // Calling again should return the same cached value
        let cached_again = theme.palette.primary_ratatui();
        assert_eq!(cached_primary, cached_again);
    }

    #[cfg(feature = "tui")]
    #[test]
    fn ratatui_cache_toast_colors() {
        // Phase 45A: Validate that the colors used by toast.rs are cached
        let p = neon_palette();

        // Toast levels use: accent (info), success, warning, error
        let info = p.accent_ratatui();
        let success = p.success_ratatui();
        let warning = p.warning_ratatui();
        let error = p.error_ratatui();

        // Verify they're all different
        assert_ne!(info, success);
        assert_ne!(success, warning);
        assert_ne!(warning, error);
    }

    #[cfg(feature = "tui")]
    #[test]
    #[ignore] // TODO: Race condition with static TERMINAL_CAPS initialization in parallel tests
    fn ratatui_cache_tui_widget_colors() {
        // Phase 45A: Validate cockpit colors are cached for TUI widgets
        // Initialize with Truecolor to ensure RGB output
        crate::render::terminal_caps::init_with_level(
            crate::render::terminal_caps::ColorLevel::Truecolor,
        );
        let p = neon_palette();

        // Activity widget uses these
        let running = p.running_ratatui();
        let planning = p.planning_ratatui();
        let reasoning = p.reasoning_ratatui();

        // Status bar uses these
        let text = p.text_ratatui();
        let border = p.border_ratatui();

        // Panel uses these
        let bg_panel = p.bg_panel_ratatui();
        let bg_highlight = p.bg_highlight_ratatui();

        // All should be valid ratatui RGB colors (since we initialized with Truecolor)
        assert!(matches!(running, ratatui::style::Color::Rgb(_, _, _)));
        assert!(matches!(planning, ratatui::style::Color::Rgb(_, _, _)));
        assert!(matches!(reasoning, ratatui::style::Color::Rgb(_, _, _)));
        assert!(matches!(text, ratatui::style::Color::Rgb(_, _, _)));
        assert!(matches!(border, ratatui::style::Color::Rgb(_, _, _)));
        assert!(matches!(bg_panel, ratatui::style::Color::Rgb(_, _, _)));
        assert!(matches!(bg_highlight, ratatui::style::Color::Rgb(_, _, _)));
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn adaptive_palette_initializes_with_base() {
        init("neon", None);
        reset_adaptive_for_test(); // Ensure clean state

        let palette = adaptive_palette();
        let base = active().palette.clone();

        // Should start with base palette (Healthy state)
        assert_eq!(palette.running.srgb8(), base.running.srgb8());
        assert_eq!(palette.planning.srgb8(), base.planning.srgb8());
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn adaptive_palette_changes_on_degraded() {
        use crate::repl::health::HealthLevel;

        init("neon", None);
        reset_adaptive_for_test(); // Ensure clean state

        let base = active().palette.clone();
        set_adaptive_health(HealthLevel::Degraded);
        let degraded = adaptive_palette();

        // Degraded palette should differ from base (hue shifted, chroma reduced)
        let base_h = base.running.to_oklch().h;
        let degraded_h = degraded.running.to_oklch().h;

        assert_ne!(base.running.srgb8(), degraded.running.srgb8());
        assert!(
            (degraded_h - base_h).abs() > 5.0,
            "Hue should shift in degraded state"
        );
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn adaptive_palette_critical_uses_red_monochrome() {
        use crate::repl::health::HealthLevel;

        init("neon", None);
        reset_adaptive_for_test(); // Ensure clean state

        set_adaptive_health(HealthLevel::Unhealthy);
        let unhealthy = adaptive_palette();

        // All cockpit colors should be red-scale (H≈25°)
        let colors = [
            unhealthy.running.to_oklch().h,
            unhealthy.planning.to_oklch().h,
            unhealthy.reasoning.to_oklch().h,
            unhealthy.delegated.to_oklch().h,
        ];

        for h in colors {
            assert!(
                (h - 25.0).abs() < 10.0,
                "Unhealthy palette should use red hue (25°), got {:.1}°",
                h
            );
        }
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn adaptive_palette_reverts_to_base_when_healthy_again() {
        use crate::repl::health::HealthLevel;

        init("neon", None);
        init_adaptive();

        let base = active().palette.clone();

        // Go through health cycle
        set_adaptive_health(HealthLevel::Unhealthy);
        set_adaptive_health(HealthLevel::Healthy);

        let final_palette = adaptive_palette();

        // Should return to base
        assert_eq!(final_palette.running.srgb8(), base.running.srgb8());
        assert_eq!(final_palette.planning.srgb8(), base.planning.srgb8());
    }

    #[cfg(feature = "color-science")]
    #[test]
    #[ignore] // Ignored: tests uninitialized state, but global statics are shared across tests
    fn adaptive_palette_fallback_when_not_initialized() {
        init("neon", None);
        // Don't call init_adaptive()

        let palette = adaptive_palette();
        let base = active().palette.clone();

        // Should fallback to base palette
        assert_eq!(palette.running.srgb8(), base.running.srgb8());
    }

    // ========================================================================
    // M2: Perceptual Elevation System Tests
    // ========================================================================

    #[cfg(feature = "color-science")]
    #[test]
    fn elevation_system_creates_5_levels() {
        let elevation = ElevationSystem::new(210.0);

        let level0 = elevation.level(0);
        let level1 = elevation.level(1);
        let level2 = elevation.level(2);
        let level3 = elevation.level(3);
        let level4 = elevation.level(4);

        // All levels should be distinct
        let levels = [level0, level1, level2, level3, level4];
        for i in 0..5 {
            for j in (i + 1)..5 {
                assert_ne!(
                    levels[i].srgb8(),
                    levels[j].srgb8(),
                    "Level {} should differ from level {}",
                    i,
                    j
                );
            }
        }
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn elevation_levels_have_uniform_lightness_steps() {
        let elevation = ElevationSystem::new(210.0);

        let mut lightness_values = Vec::new();
        for i in 0..5 {
            let oklch = elevation.level(i).to_oklch();
            lightness_values.push(oklch.l);
        }

        // Check that each step is approximately +0.05 lightness (M7: base=0.08, step=0.05)
        for i in 1..5 {
            let delta = lightness_values[i] - lightness_values[i - 1];
            assert!(
                (delta - 0.05).abs() < 0.001,
                "Lightness step {} → {} should be ~0.05, got {:.4}",
                i - 1,
                i,
                delta
            );
        }
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn elevation_base_has_correct_lightness() {
        let elevation = ElevationSystem::new(210.0);
        let base_oklch = elevation.base().to_oklch();

        // M7: base_lightness = 0.08
        assert!(
            (base_oklch.l - 0.08).abs() < 0.001,
            "Base lightness should be 0.08, got {:.4}",
            base_oklch.l
        );
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn elevation_emphasized_has_correct_lightness() {
        let elevation = ElevationSystem::new(210.0);
        let emphasized_oklch = elevation.emphasized().to_oklch();

        // Level 4: 0.08 + (4 * 0.05) = 0.28 (M7 values)
        assert!(
            (emphasized_oklch.l - 0.28).abs() < 0.001,
            "Emphasized lightness should be 0.28, got {:.4}",
            emphasized_oklch.l
        );
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn elevation_semantic_accessors_match_levels() {
        let elevation = ElevationSystem::new(210.0);

        assert_eq!(elevation.base().srgb8(), elevation.level(0).srgb8());
        assert_eq!(elevation.card().srgb8(), elevation.level(1).srgb8());
        assert_eq!(elevation.hover().srgb8(), elevation.level(2).srgb8());
        assert_eq!(elevation.selected().srgb8(), elevation.level(3).srgb8());
        assert_eq!(elevation.emphasized().srgb8(), elevation.level(4).srgb8());
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn elevation_colors_are_monotonically_increasing() {
        let elevation = ElevationSystem::new(210.0);

        for i in 0..4 {
            let current = elevation.level(i).to_oklch();
            let next = elevation.level(i + 1).to_oklch();

            assert!(
                next.l > current.l,
                "Level {} lightness {:.3} should be < level {} lightness {:.3}",
                i,
                current.l,
                i + 1,
                next.l
            );
        }
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn elevation_with_custom_params() {
        let elevation = ElevationSystem::with_params(180.0, 0.15, 0.03);

        let base_oklch = elevation.base().to_oklch();
        let level1_oklch = elevation.level(1).to_oklch();

        // Base should be 0.15
        assert!(
            (base_oklch.l - 0.15).abs() < 0.001,
            "Custom base lightness should be 0.15, got {:.4}",
            base_oklch.l
        );

        // Step should be 0.03
        let delta = level1_oklch.l - base_oklch.l;
        assert!(
            (delta - 0.03).abs() < 0.001,
            "Custom step should be 0.03, got {:.4}",
            delta
        );
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn elevation_preserves_hue_across_levels() {
        let elevation = ElevationSystem::new(210.0);

        for i in 0..5 {
            let oklch = elevation.level(i).to_oklch();
            let hue_diff = (oklch.h - 210.0).abs();

            // Allow small floating-point variation
            assert!(
                hue_diff < 1.0,
                "Level {} hue {:.1}° should be close to base hue 210°",
                i,
                oklch.h
            );
        }
    }

    // ========================================================================
    // M5: Perceptual Border Colors Tests
    // ========================================================================

    #[cfg(feature = "color-science")]
    #[test]
    fn m5_border_between_is_midpoint() {
        let dark = ThemeColor::oklch(0.2, 0.02, 210.0);
        let light = ThemeColor::oklch(0.8, 0.04, 210.0);

        let border = dark.border_between(&light, 1.0);
        let border_oklch = border.to_oklch();

        // Should be approximately halfway in lightness
        let expected_l = (0.2 + 0.8) / 2.0;
        assert!(
            (border_oklch.l - expected_l).abs() < 0.05,
            "Border lightness {:.3} should be near {:.3}",
            border_oklch.l,
            expected_l
        );

        // Should average chroma (with factor 1.0)
        let expected_c = (0.02 + 0.04) / 2.0;
        assert!(
            (border_oklch.c - expected_c).abs() < 0.01,
            "Border chroma {:.3} should be near {:.3}",
            border_oklch.c,
            expected_c
        );
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn m5_border_subtle_has_reduced_chroma() {
        let dark = ThemeColor::oklch(0.2, 0.02, 210.0);
        let light = ThemeColor::oklch(0.8, 0.04, 210.0);

        let clear = dark.border_clear(&light);
        let subtle = dark.border_subtle(&light);

        let clear_oklch = clear.to_oklch();
        let subtle_oklch = subtle.to_oklch();

        // Subtle should have less chroma than clear
        assert!(
            subtle_oklch.c < clear_oklch.c,
            "Subtle chroma {:.3} should be < clear chroma {:.3}",
            subtle_oklch.c,
            clear_oklch.c
        );

        // Subtle should be approximately half the chroma
        assert!(
            (subtle_oklch.c - clear_oklch.c * 0.5).abs() < 0.01,
            "Subtle chroma should be ~50% of clear"
        );
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn m5_border_preserves_hue() {
        let color1 = ThemeColor::oklch(0.3, 0.05, 195.0); // Cyan
        let color2 = ThemeColor::oklch(0.7, 0.08, 195.0); // Same hue

        let border = color1.border_between(&color2, 1.0);
        let border_oklch = border.to_oklch();

        // Hue should be preserved (both colors have same hue)
        assert!(
            (border_oklch.h - 195.0).abs() < 5.0,
            "Border hue {:.1}° should be near 195°",
            border_oklch.h
        );
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn m5_border_between_different_hues() {
        let blue = ThemeColor::oklch(0.5, 0.1, 210.0);
        let green = ThemeColor::oklch(0.5, 0.1, 150.0);

        let border = blue.border_between(&green, 1.0);
        let border_oklch = border.to_oklch();

        // Hue should be averaged
        let expected_h = (210.0 + 150.0) / 2.0;
        assert!(
            (border_oklch.h - expected_h).abs() < 5.0,
            "Border hue {:.1}° should be near {:.1}°",
            border_oklch.h,
            expected_h
        );
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn m5_border_lightness_between_extremes() {
        let black = ThemeColor::oklch(0.05, 0.0, 0.0);
        let white = ThemeColor::oklch(0.95, 0.0, 0.0);

        let border = black.border_between(&white, 1.0);
        let border_oklch = border.to_oklch();

        // Should be gray-ish (mid lightness)
        assert!(
            border_oklch.l > 0.4 && border_oklch.l < 0.6,
            "Border lightness {:.3} should be in [0.4, 0.6]",
            border_oklch.l
        );
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn m5_border_variants_have_same_lightness() {
        let dark = ThemeColor::oklch(0.2, 0.04, 210.0);
        let light = ThemeColor::oklch(0.8, 0.08, 210.0);

        let subtle = dark.border_subtle(&light);
        let clear = dark.border_clear(&light);

        let subtle_oklch = subtle.to_oklch();
        let clear_oklch = clear.to_oklch();

        // Both should have same lightness (only chroma differs)
        assert!(
            (subtle_oklch.l - clear_oklch.l).abs() < 0.01,
            "Subtle and clear should have same lightness"
        );
    }
}
