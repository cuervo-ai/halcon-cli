//! Reusable UI components: panels, badges, tables, alerts.
//!
//! All output functions accept `&mut impl Write` for testability
//! (write to `Vec<u8>` in tests, `stderr` in production).

use std::io::Write;

use super::color;
use super::theme;

/// Badge severity levels for status indicators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum BadgeLevel {
    Success,
    Warning,
    Error,
    Info,
    Muted,
}

/// Render a titled panel with bordered content.
///
/// ```text
/// ╭─ Title ───────────╮
/// │ line 1             │
/// │ line 2             │
/// ╰───────────────────╯
/// ```
#[allow(dead_code)]
pub fn panel(title: &str, lines: &[&str], out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();
    let tl = color::box_top_left();
    let tr = color::box_top_right();
    let bl = color::box_bottom_left();
    let br = color::box_bottom_right();
    let h = color::box_horiz();
    let v = color::box_vert();

    // Calculate content width.
    let title_len = title.chars().count();
    let max_content = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let inner_width = title_len.max(max_content).max(20) + 4;

    let muted = t.palette.muted.fg();
    let accent = t.palette.accent.fg();

    // Top border.
    let _ = writeln!(
        out,
        "  {muted}{tl}{h} {accent}{title}{muted} {}{tr}{r}",
        h.repeat(inner_width.saturating_sub(title_len + 3)),
    );

    // Content lines.
    for line in lines {
        let padding = inner_width.saturating_sub(line.chars().count() + 1);
        let _ = writeln!(out, "  {muted}{v}{r} {line}{:>pad$}{muted}{v}{r}", "", pad = padding);
    }

    // Bottom border.
    let _ = writeln!(
        out,
        "  {muted}{bl}{}{br}{r}",
        h.repeat(inner_width + 1),
    );
}

/// Create a colored status badge string.
///
/// Returns e.g. `"[OK]"` colored green, `"[WARN]"` colored yellow.
pub fn badge(label: &str, level: BadgeLevel) -> String {
    let t = theme::active();
    let r = theme::reset();
    let color = match level {
        BadgeLevel::Success => t.palette.success.fg(),
        BadgeLevel::Warning => t.palette.warning.fg(),
        BadgeLevel::Error => t.palette.error.fg(),
        BadgeLevel::Info => t.palette.primary.fg(),
        BadgeLevel::Muted => t.palette.muted.fg(),
    };
    format!("{color}[{label}]{r}")
}

/// Render a key-value table with aligned columns.
pub fn kv_table(pairs: &[(&str, &str)], indent: usize, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();
    let key_color = t.palette.text_dim.fg();
    let val_color = t.palette.text.fg();
    let pad_str = " ".repeat(indent);

    let max_key = pairs.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(0);

    for (key, val) in pairs {
        let key_pad = max_key - key.chars().count();
        let _ = writeln!(
            out,
            "{pad_str}{key_color}{key}{:>kp$}  {val_color}{val}{r}",
            "",
            kp = key_pad,
        );
    }
}

/// Render a horizontal rule.
pub fn hr(width: usize, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();
    let h = color::box_horiz();
    let dim = t.palette.muted.fg();
    let bright = t.palette.primary.fg();

    if width < 6 {
        let _ = writeln!(out, "  {dim}{}{r}", h.repeat(width));
        return;
    }

    // Gradient simulation: dim-bright-dim.
    let edge = width / 4;
    let center = width - edge * 2;
    let _ = writeln!(
        out,
        "  {dim}{}{bright}{}{dim}{}{r}",
        h.repeat(edge),
        h.repeat(center),
        h.repeat(edge),
    );
}

/// Render a section header.
pub fn section_header(title: &str, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();
    let color = t.palette.accent.fg();
    let bold = "\x1b[1m";
    if color::color_enabled() {
        let _ = writeln!(out, "\n  {bold}{color}{title}{r}");
    } else {
        let _ = writeln!(out, "\n  {title}");
    }
}

/// Render an alert box (error or warning).
#[allow(dead_code)]
pub fn alert(level: BadgeLevel, message: &str, hint: Option<&str>, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();

    let (icon, color) = match level {
        BadgeLevel::Error => ("✖", t.palette.error.fg()),
        BadgeLevel::Warning => ("▲", t.palette.warning.fg()),
        _ => ("●", t.palette.primary.fg()),
    };
    let icon_char = if color::unicode_enabled() { icon } else { "!" };

    let _ = writeln!(out, "  {color}{icon_char} {message}{r}");
    if let Some(h) = hint {
        let hint_color = t.palette.muted.fg();
        let _ = writeln!(out, "    {hint_color}Hint: {h}{r}");
    }
}

/// Format a progress fraction (e.g. "3/5").
#[allow(dead_code)]
pub fn progress_fraction(current: usize, total: usize) -> String {
    let t = theme::active();
    let r = theme::reset();
    let color = if current == total {
        t.palette.success.fg()
    } else {
        t.palette.primary.fg()
    };
    format!("{color}{current}/{total}{r}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture() -> Vec<u8> {
        Vec::new()
    }

    #[test]
    fn panel_renders_title() {
        let mut buf = capture();
        panel("Test Panel", &["line 1", "line 2"], &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("Test Panel"));
        assert!(output.contains("line 1"));
        assert!(output.contains("line 2"));
    }

    #[test]
    fn panel_empty_content() {
        let mut buf = capture();
        panel("Empty", &[], &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("Empty"));
    }

    #[test]
    fn badge_contains_label() {
        let b = badge("OK", BadgeLevel::Success);
        assert!(b.contains("OK"));
    }

    #[test]
    fn badge_all_levels() {
        for level in [BadgeLevel::Success, BadgeLevel::Warning, BadgeLevel::Error, BadgeLevel::Info, BadgeLevel::Muted] {
            let b = badge("X", level);
            assert!(b.contains("[X]"));
        }
    }

    #[test]
    fn kv_table_renders_pairs() {
        let mut buf = capture();
        kv_table(&[("Key", "Value"), ("Longer Key", "Val")], 4, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("Key"));
        assert!(output.contains("Value"));
        assert!(output.contains("Longer Key"));
    }

    #[test]
    fn hr_renders_line() {
        let mut buf = capture();
        hr(40, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(!output.is_empty());
    }

    #[test]
    fn hr_short_width() {
        let mut buf = capture();
        hr(3, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(!output.is_empty());
    }

    #[test]
    fn section_header_renders_title() {
        let mut buf = capture();
        section_header("Test Section", &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("Test Section"));
    }

    #[test]
    fn alert_error_renders() {
        let mut buf = capture();
        alert(BadgeLevel::Error, "something broke", Some("try again"), &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("something broke"));
        assert!(output.contains("try again"));
    }

    #[test]
    fn alert_warning_no_hint() {
        let mut buf = capture();
        alert(BadgeLevel::Warning, "watch out", None, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("watch out"));
        assert!(!output.contains("Hint"));
    }

    #[test]
    fn progress_fraction_format() {
        let s = progress_fraction(3, 5);
        assert!(s.contains("3/5"));
    }

    #[test]
    fn progress_fraction_complete() {
        let s = progress_fraction(5, 5);
        assert!(s.contains("5/5"));
    }
}
