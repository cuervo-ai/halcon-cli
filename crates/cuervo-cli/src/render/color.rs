//! Color and Unicode output detection.
//!
//! Respects the [NO_COLOR](https://no-color.org/) standard:
//! - `NO_COLOR` env var set (any value) → no colors
//! - `TERM=dumb` → no colors, no Unicode
//! - Non-TTY stderr → no colors
//!
//! Extended capabilities: truecolor, animations, terminal width, TTY detection.

use std::sync::OnceLock;

/// Cached output capabilities (computed once on first access).
static CAPS: OnceLock<OutputCaps> = OnceLock::new();

/// Output capability flags.
#[allow(dead_code)]
struct OutputCaps {
    color: bool,
    unicode: bool,
    truecolor: bool,
    animations: bool,
    terminal_width: u16,
    is_tty: bool,
}

fn detect() -> OutputCaps {
    let no_color = std::env::var_os("NO_COLOR").is_some();
    let term = std::env::var("TERM").unwrap_or_default();
    let dumb = term == "dumb";

    // TTY detection on stderr (where we render UI).
    let is_tty = {
        use crossterm::tty::IsTty;
        std::io::stderr().is_tty()
    };

    // Truecolor: COLORTERM=truecolor or COLORTERM=24bit.
    let colorterm = std::env::var("COLORTERM").unwrap_or_default();
    let truecolor = matches!(colorterm.as_str(), "truecolor" | "24bit");

    // Animations: disabled in CI, non-TTY, or explicit opt-out.
    let ci = std::env::var_os("CI").is_some() || std::env::var_os("GITHUB_ACTIONS").is_some();
    let no_animations = std::env::var_os("CUERVO_NO_ANIMATIONS").is_some();
    let animations = is_tty && !ci && !no_animations && !dumb;

    // Terminal width: try crossterm, fallback to 80.
    let terminal_width = crossterm::terminal::size().map(|(w, _)| w).unwrap_or(80);

    OutputCaps {
        color: !no_color && !dumb,
        unicode: !dumb,
        truecolor,
        animations,
        terminal_width,
        is_tty,
    }
}

fn caps() -> &'static OutputCaps {
    CAPS.get_or_init(detect)
}

/// Returns true if color output is enabled.
pub fn color_enabled() -> bool {
    caps().color
}

/// Returns true if Unicode box-drawing characters are safe to use.
pub fn unicode_enabled() -> bool {
    caps().unicode
}

/// Returns true if the terminal supports 24-bit truecolor.
#[allow(dead_code)]
pub fn truecolor_enabled() -> bool {
    caps().truecolor
}

/// Returns true if animations (spinner frames) should be shown.
pub fn animations_enabled() -> bool {
    caps().animations
}

/// Returns the detected terminal width in columns (default 80).
pub fn terminal_width() -> u16 {
    caps().terminal_width
}

/// Returns true if stderr is connected to a TTY.
#[allow(dead_code)]
pub fn is_tty() -> bool {
    caps().is_tty
}

/// Returns true if the terminal is narrower than `threshold` columns.
#[allow(dead_code)]
pub fn is_compact(threshold: u16) -> bool {
    terminal_width() < threshold
}

// --- Box-drawing character accessors ---

/// Top-left corner: `╭` or `+`
pub fn box_top_left() -> &'static str {
    if unicode_enabled() { "╭" } else { "+" }
}

/// Bottom-left corner: `╰` or `+`
pub fn box_bottom_left() -> &'static str {
    if unicode_enabled() { "╰" } else { "+" }
}

/// Vertical bar: `│` or `|`
pub fn box_vert() -> &'static str {
    if unicode_enabled() { "│" } else { "|" }
}

/// Horizontal bar: `─` or `-`
pub fn box_horiz() -> &'static str {
    if unicode_enabled() { "─" } else { "-" }
}

/// Top-right corner: `╮` or `+`
pub fn box_top_right() -> &'static str {
    if unicode_enabled() { "╮" } else { "+" }
}

/// Bottom-right corner: `╯` or `+`
pub fn box_bottom_right() -> &'static str {
    if unicode_enabled() { "╯" } else { "+" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_returns_consistent_value() {
        let c1 = color_enabled();
        let c2 = color_enabled();
        assert_eq!(c1, c2);
    }

    #[test]
    fn unicode_returns_consistent_value() {
        let u1 = unicode_enabled();
        let u2 = unicode_enabled();
        assert_eq!(u1, u2);
    }

    #[test]
    fn box_chars_are_non_empty() {
        assert!(!box_top_left().is_empty());
        assert!(!box_bottom_left().is_empty());
        assert!(!box_vert().is_empty());
        assert!(!box_horiz().is_empty());
    }

    #[test]
    fn terminal_width_is_positive() {
        assert!(terminal_width() > 0);
    }

    #[test]
    fn truecolor_returns_consistent_value() {
        let t1 = truecolor_enabled();
        let t2 = truecolor_enabled();
        assert_eq!(t1, t2);
    }

    #[test]
    fn animations_returns_consistent_value() {
        let a1 = animations_enabled();
        let a2 = animations_enabled();
        assert_eq!(a1, a2);
    }

    #[test]
    fn is_compact_with_high_threshold() {
        // 10000 is wider than any terminal — should always be compact.
        assert!(is_compact(10000));
    }

    #[test]
    fn is_tty_returns_consistent_value() {
        let t1 = is_tty();
        let t2 = is_tty();
        assert_eq!(t1, t2);
    }
}
