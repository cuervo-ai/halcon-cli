#[allow(dead_code)]
pub mod animations;
#[allow(dead_code)]
pub mod banner;
pub mod color;
#[cfg(feature = "color-science")]
pub mod color_science;
pub mod components;
#[allow(dead_code)]
pub mod diff;
pub mod feedback;
pub mod markdown;
pub mod spinner;
pub mod stream;
pub mod sink;
pub mod syntax;
pub mod theme;
pub mod tool;

use std::sync::OnceLock;

use syntax::Highlighter;

fn highlighter() -> &'static Highlighter {
    static INSTANCE: OnceLock<Highlighter> = OnceLock::new();
    INSTANCE.get_or_init(Highlighter::new)
}

/// Render a complete assistant response to the terminal.
///
/// Fenced code blocks (` ```lang ... ``` `) get syntax highlighting via syntect.
/// Everything else is rendered as markdown via termimad.
///
/// Note: streaming responses use `StreamRenderer` instead.
#[cfg(test)]
pub fn render_response(text: &str) {
    if text.is_empty() {
        return;
    }

    let hl = highlighter();
    let mut remaining = text;

    while let Some(fence_start) = remaining.find("```") {
        // Prose before the fence.
        let prose = &remaining[..fence_start];
        if !prose.is_empty() {
            markdown::render(prose);
        }

        let after_fence = &remaining[fence_start + 3..];
        let lang_end = after_fence.find('\n').unwrap_or(after_fence.len());
        let lang = after_fence[..lang_end].trim();

        let code_region = if lang_end < after_fence.len() {
            &after_fence[lang_end + 1..]
        } else {
            ""
        };

        if let Some(close) = code_region.find("```") {
            let code = &code_region[..close];
            if !code.is_empty() {
                let lang = if lang.is_empty() { "txt" } else { lang };
                print!("{}", hl.highlight(code, lang));
            }
            remaining = &code_region[close + 3..];
            // Skip optional newline after closing fence.
            if remaining.starts_with('\n') {
                remaining = &remaining[1..];
            }
        } else {
            // No closing fence — render everything remaining as prose.
            markdown::render(remaining);
            return;
        }
    }

    if !remaining.is_empty() {
        markdown::render(remaining);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_response_does_not_panic_on_empty() {
        render_response("");
    }

    #[test]
    fn render_response_handles_plain_text() {
        render_response("Hello, world!");
    }

    #[test]
    fn render_response_with_code_block() {
        let md = "Some text\n```rust\nfn main() {}\n```\nMore text";
        render_response(md);
    }

    #[test]
    fn render_response_with_unclosed_fence() {
        let md = "text\n```python\nprint('hi')\n";
        render_response(md);
    }

    #[test]
    fn render_response_multiple_code_blocks() {
        let md = "A\n```json\n{}\n```\nB\n```sh\necho hi\n```\nC";
        render_response(md);
    }
}
