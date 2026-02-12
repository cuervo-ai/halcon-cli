/// Build the default terminal skin for markdown rendering.
#[cfg(test)]
fn make_skin() -> termimad::MadSkin {
    use termimad::crossterm::style::Color;
    use termimad::MadSkin;
    let mut skin = MadSkin::default();
    // Use terminal width for wrapping. Default skin already handles
    // bold, italic, strikethrough, code, headers, tables, lists.
    // We only override where needed.
    skin.set_headers_fg(Color::Cyan);
    skin.bold.set_fg(Color::White);
    skin.italic.set_fg(Color::Yellow);
    skin.inline_code.set_bg(Color::DarkGrey);
    skin
}

/// Render a markdown string to stdout with terminal formatting.
#[cfg(test)]
pub fn render(text: &str) {
    if text.is_empty() {
        return;
    }
    let skin = make_skin();
    // print_text handles wrapping to terminal width automatically.
    skin.print_text(text);
}

/// Render markdown to a String (for testing / non-TTY).
#[cfg(test)]
fn render_to_string(text: &str, width: usize) -> String {
    if text.is_empty() {
        return String::new();
    }
    let skin = make_skin();
    let area = termimad::Area::new(0, 0, width as u16, u16::MAX);
    let text = termimad::FmtText::from(&skin, text, Some(area.width as usize));
    format!("{text}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_empty_string() {
        assert_eq!(render_to_string("", 80), "");
    }

    #[test]
    fn renders_plain_text() {
        let out = render_to_string("Hello world", 80);
        assert!(out.contains("Hello world"));
    }

    #[test]
    fn renders_bold() {
        let out = render_to_string("**bold text**", 80);
        // termimad wraps bold text in ANSI escape codes
        assert!(out.contains("bold text"));
    }

    #[test]
    fn renders_code_block() {
        let md = "```\nfn main() {}\n```";
        let out = render_to_string(md, 80);
        assert!(out.contains("fn main()"));
    }

    #[test]
    fn renders_header() {
        let out = render_to_string("# Title", 80);
        assert!(out.contains("Title"));
    }

    #[test]
    fn renders_bullet_list() {
        let md = "* item one\n* item two";
        let out = render_to_string(md, 80);
        assert!(out.contains("item one"));
        assert!(out.contains("item two"));
    }

    #[test]
    fn renders_table() {
        let md = "|col1|col2|\n|-|-|\n|a|b|";
        let out = render_to_string(md, 80);
        assert!(out.contains("col1"));
        assert!(out.contains("a"));
    }

    #[test]
    fn respects_width() {
        let long = "word ".repeat(50);
        let narrow = render_to_string(&long, 40);
        // Should contain line breaks due to wrapping
        assert!(narrow.lines().count() > 1);
    }
}
