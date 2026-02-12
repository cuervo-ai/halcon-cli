//! Diff renderer — color-coded unified diffs, inline hunks, change markers.
//!
//! Supports:
//! - Unified diff format with color-coded +/- lines
//! - Inline hunk preview (before/after with context)
//! - Change markers (added=green, modified=yellow, deleted=red, ai=purple)
//! - Summary statistics

use std::io::Write;

use super::color;
use super::theme;

/// Type of line change for rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Context,
    /// AI-generated edit (purple accent).
    AiGenerated,
}

/// A single line in a diff hunk.
#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: ChangeKind,
    pub line_number: Option<usize>,
    pub content: String,
}

/// A diff hunk with context.
#[derive(Debug, Clone)]
pub struct DiffHunk {
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    pub lines: Vec<DiffLine>,
}

/// A complete file diff.
#[derive(Debug, Clone)]
pub struct FileDiff {
    pub path: String,
    pub hunks: Vec<DiffHunk>,
    pub added: usize,
    pub deleted: usize,
    pub modified: usize,
}

/// Gutter marker character for a change kind.
fn gutter_char(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Added => "+",
        ChangeKind::Deleted => "-",
        ChangeKind::Modified => "~",
        ChangeKind::Context => " ",
        ChangeKind::AiGenerated => "▸",
    }
}

/// Gutter color for a change kind.
fn gutter_color(kind: ChangeKind) -> String {
    let t = theme::active();
    match kind {
        ChangeKind::Added => t.palette.success.fg(),
        ChangeKind::Deleted => t.palette.error.fg(),
        ChangeKind::Modified => t.palette.warning.fg(),
        ChangeKind::Context => t.palette.muted.fg(),
        ChangeKind::AiGenerated => t.palette.accent.fg(),
    }
}

/// Render a unified diff for a single file.
pub fn render_file_diff(diff: &FileDiff, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();
    let muted = t.palette.muted.fg();
    let accent = t.palette.accent.fg();
    let bold = if color::color_enabled() { "\x1b[1m" } else { "" };
    let h = color::box_horiz();

    // File header.
    let _ = writeln!(out, "\n  {muted}{h}{h}{h} {bold}{accent}{}{r}", diff.path);

    // Summary line.
    let added_str = if diff.added > 0 {
        let c = t.palette.success.fg();
        format!("{c}+{}{r}", diff.added)
    } else {
        String::new()
    };
    let deleted_str = if diff.deleted > 0 {
        let c = t.palette.error.fg();
        format!("{c}-{}{r}", diff.deleted)
    } else {
        String::new()
    };
    let modified_str = if diff.modified > 0 {
        let c = t.palette.warning.fg();
        format!("{c}~{}{r}", diff.modified)
    } else {
        String::new()
    };

    let parts: Vec<&str> = [added_str.as_str(), deleted_str.as_str(), modified_str.as_str()]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect();
    if !parts.is_empty() {
        let _ = writeln!(out, "  {muted}    {}{r}", parts.join(" "));
    }

    // Hunks.
    for hunk in &diff.hunks {
        render_hunk(hunk, out);
    }
}

/// Render a single hunk.
fn render_hunk(hunk: &DiffHunk, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();
    let muted = t.palette.muted.fg();

    // Hunk header (@@ -old,count +new,count @@).
    let _ = writeln!(
        out,
        "  {muted}@@ -{},{} +{},{} @@{r}",
        hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
    );

    // Lines.
    for line in &hunk.lines {
        render_diff_line(line, out);
    }
}

/// Render a single diff line with gutter marker and color.
fn render_diff_line(line: &DiffLine, out: &mut impl Write) {
    let r = theme::reset();
    let color = gutter_color(line.kind);
    let marker = gutter_char(line.kind);

    let line_num = match line.line_number {
        Some(n) => format!("{n:>4}"),
        None => "    ".to_string(),
    };

    let t = theme::active();
    let num_color = t.palette.text_dim.fg();

    // For deleted lines, use strikethrough if terminal supports it.
    let (style_start, style_end) = if line.kind == ChangeKind::Deleted && color::color_enabled() {
        ("\x1b[9m", "\x1b[29m") // strikethrough
    } else {
        ("", "")
    };

    // For AI-generated, use italic.
    let (ai_start, ai_end) = if line.kind == ChangeKind::AiGenerated && color::color_enabled() {
        ("\x1b[3m", "\x1b[23m") // italic
    } else {
        ("", "")
    };

    let _ = writeln!(
        out,
        "  {num_color}{line_num} {color}{marker}{r} {style_start}{ai_start}{color}{}{ai_end}{style_end}{r}",
        line.content
    );
}

/// Compute a simple line-level diff between two texts.
///
/// Returns a `FileDiff` with hunks generated from the differences.
/// Uses a simple LCS-based approach suitable for inline preview.
pub fn compute_diff(path: &str, old_text: &str, new_text: &str) -> FileDiff {
    compute_diff_with_kind(path, old_text, new_text, false)
}

/// Like `compute_diff`, but marks additions as AI-generated.
pub fn compute_ai_diff(path: &str, old_text: &str, new_text: &str) -> FileDiff {
    compute_diff_with_kind(path, old_text, new_text, true)
}

fn compute_diff_with_kind(path: &str, old_text: &str, new_text: &str, ai_generated: bool) -> FileDiff {
    let old_lines: Vec<&str> = old_text.lines().collect();
    let new_lines: Vec<&str> = new_text.lines().collect();

    let add_kind = if ai_generated {
        ChangeKind::AiGenerated
    } else {
        ChangeKind::Added
    };

    let mut hunks = Vec::new();
    let mut added = 0usize;
    let mut deleted = 0usize;

    // Simple sequential diff: walk both line arrays.
    let mut oi = 0;
    let mut ni = 0;
    let context_lines = 3;

    while oi < old_lines.len() || ni < new_lines.len() {
        // Find next difference.
        if oi < old_lines.len() && ni < new_lines.len() && old_lines[oi] == new_lines[ni] {
            oi += 1;
            ni += 1;
            continue;
        }

        // Found a difference — build a hunk.
        let hunk_old_start = oi.saturating_sub(context_lines);
        let hunk_new_start = ni.saturating_sub(context_lines);
        let mut lines = Vec::new();

        // Leading context.
        let ctx_start_old = oi.saturating_sub(context_lines);
        let ctx_start_new = ni.saturating_sub(context_lines);
        for (offset, line) in old_lines[ctx_start_old..oi].iter().enumerate() {
            lines.push(DiffLine {
                kind: ChangeKind::Context,
                line_number: Some(ctx_start_new + offset + 1),
                content: line.to_string(),
            });
        }

        // Consume differing lines.
        while oi < old_lines.len() || ni < new_lines.len() {
            let same = oi < old_lines.len()
                && ni < new_lines.len()
                && old_lines[oi] == new_lines[ni];

            if same {
                // Check if we have enough trailing context.
                let mut trail = 0;
                while trail < context_lines
                    && oi + trail < old_lines.len()
                    && ni + trail < new_lines.len()
                    && old_lines[oi + trail] == new_lines[ni + trail]
                {
                    trail += 1;
                }

                // Add trailing context.
                for t in 0..trail {
                    lines.push(DiffLine {
                        kind: ChangeKind::Context,
                        line_number: Some(ni + t + 1),
                        content: old_lines[oi + t].to_string(),
                    });
                }
                oi += trail;
                ni += trail;
                break;
            }

            // Emit deleted lines.
            if oi < old_lines.len()
                && (ni >= new_lines.len() || !new_lines[ni..].contains(&old_lines[oi]))
            {
                lines.push(DiffLine {
                    kind: ChangeKind::Deleted,
                    line_number: Some(oi + 1),
                    content: old_lines[oi].to_string(),
                });
                deleted += 1;
                oi += 1;
                continue;
            }

            // Emit added lines.
            if ni < new_lines.len()
                && (oi >= old_lines.len() || !old_lines[oi..].contains(&new_lines[ni]))
            {
                lines.push(DiffLine {
                    kind: add_kind,
                    line_number: Some(ni + 1),
                    content: new_lines[ni].to_string(),
                });
                added += 1;
                ni += 1;
                continue;
            }

            // Both have lines but they differ — treat as delete+add.
            if oi < old_lines.len() {
                lines.push(DiffLine {
                    kind: ChangeKind::Deleted,
                    line_number: Some(oi + 1),
                    content: old_lines[oi].to_string(),
                });
                deleted += 1;
                oi += 1;
            }
            if ni < new_lines.len() {
                lines.push(DiffLine {
                    kind: add_kind,
                    line_number: Some(ni + 1),
                    content: new_lines[ni].to_string(),
                });
                added += 1;
                ni += 1;
            }
        }

        if !lines.is_empty() {
            let old_count = lines
                .iter()
                .filter(|l| l.kind == ChangeKind::Deleted || l.kind == ChangeKind::Context)
                .count();
            let new_count = lines
                .iter()
                .filter(|l| {
                    l.kind == ChangeKind::Added
                        || l.kind == ChangeKind::AiGenerated
                        || l.kind == ChangeKind::Context
                })
                .count();

            hunks.push(DiffHunk {
                old_start: hunk_old_start + 1,
                old_count,
                new_start: hunk_new_start + 1,
                new_count,
                lines,
            });
        }
    }

    FileDiff {
        path: path.to_string(),
        hunks,
        added,
        deleted,
        modified: 0,
    }
}

/// Render a compact inline diff (for chat/REPL output).
///
/// Shows added/deleted lines only, no context, with gutter markers.
pub fn render_inline_diff(old_text: &str, new_text: &str, out: &mut impl Write) {
    let diff = compute_diff("", old_text, new_text);
    let r = theme::reset();

    for hunk in &diff.hunks {
        for line in &hunk.lines {
            if line.kind == ChangeKind::Context {
                continue;
            }
            let color = gutter_color(line.kind);
            let marker = gutter_char(line.kind);
            let _ = writeln!(out, "  {color}{marker} {}{r}", line.content);
        }
    }
}

/// Render a diff summary bar (e.g., "+15 -3 ~2").
pub fn render_diff_summary(added: usize, deleted: usize, modified: usize, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();

    let mut parts = Vec::new();
    if added > 0 {
        parts.push(format!("{}+{added}{r}", t.palette.success.fg()));
    }
    if deleted > 0 {
        parts.push(format!("{}-{deleted}{r}", t.palette.error.fg()));
    }
    if modified > 0 {
        parts.push(format!("{}~{modified}{r}", t.palette.warning.fg()));
    }
    if parts.is_empty() {
        let muted = t.palette.muted.fg();
        let _ = write!(out, "{muted}no changes{r}");
    } else {
        let _ = write!(out, "{}", parts.join(" "));
    }
}

/// Render change gutter markers for a range of lines.
///
/// Used by the IDE to show which lines have been added/modified/deleted.
pub fn render_gutter(changes: &[(usize, ChangeKind)], out: &mut impl Write) {
    let r = theme::reset();
    for (line_num, kind) in changes {
        let color = gutter_color(*kind);
        let marker = match kind {
            ChangeKind::Added => "▎",
            ChangeKind::Modified => "▎",
            ChangeKind::Deleted => "▁",
            ChangeKind::AiGenerated => "▎",
            ChangeKind::Context => " ",
        };
        let marker = if color::unicode_enabled() {
            marker
        } else {
            gutter_char(*kind)
        };
        let _ = writeln!(out, "{color}{line_num:>4} {marker}{r}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture() -> Vec<u8> {
        Vec::new()
    }

    fn output_str(buf: &[u8]) -> String {
        String::from_utf8_lossy(buf).to_string()
    }

    // ── compute_diff ──────────────────────────────────────────

    #[test]
    fn diff_identical_texts() {
        let diff = compute_diff("test.rs", "hello\nworld\n", "hello\nworld\n");
        assert!(diff.hunks.is_empty());
        assert_eq!(diff.added, 0);
        assert_eq!(diff.deleted, 0);
    }

    #[test]
    fn diff_added_lines() {
        let diff = compute_diff("test.rs", "line1\n", "line1\nline2\n");
        assert_eq!(diff.added, 1);
        assert_eq!(diff.deleted, 0);
        assert!(!diff.hunks.is_empty());
        let has_added = diff.hunks.iter().flat_map(|h| &h.lines).any(|l| l.kind == ChangeKind::Added);
        assert!(has_added);
    }

    #[test]
    fn diff_deleted_lines() {
        let diff = compute_diff("test.rs", "line1\nline2\n", "line1\n");
        assert_eq!(diff.deleted, 1);
        assert_eq!(diff.added, 0);
    }

    #[test]
    fn diff_modified_lines() {
        let diff = compute_diff("test.rs", "old line\n", "new line\n");
        // Modified = deleted old + added new.
        assert!(diff.added > 0 || diff.deleted > 0);
    }

    #[test]
    fn diff_ai_generated() {
        let diff = compute_ai_diff("test.rs", "old\n", "new\n");
        let has_ai = diff
            .hunks
            .iter()
            .flat_map(|h| &h.lines)
            .any(|l| l.kind == ChangeKind::AiGenerated);
        assert!(has_ai);
    }

    #[test]
    fn diff_empty_to_content() {
        let diff = compute_diff("test.rs", "", "hello\nworld\n");
        assert_eq!(diff.added, 2);
    }

    #[test]
    fn diff_content_to_empty() {
        let diff = compute_diff("test.rs", "hello\nworld\n", "");
        assert_eq!(diff.deleted, 2);
    }

    // ── render_file_diff ──────────────────────────────────────

    #[test]
    fn render_diff_shows_path() {
        let diff = compute_diff("src/main.rs", "old\n", "new\n");
        let mut buf = capture();
        render_file_diff(&diff, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("src/main.rs"));
    }

    #[test]
    fn render_diff_shows_hunk_header() {
        let diff = compute_diff("test.rs", "a\n", "b\n");
        let mut buf = capture();
        render_file_diff(&diff, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("@@"));
    }

    #[test]
    fn render_diff_shows_markers() {
        let diff = compute_diff("test.rs", "old\n", "new\n");
        let mut buf = capture();
        render_file_diff(&diff, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("+") || out.contains("-"));
    }

    // ── render_inline_diff ────────────────────────────────────

    #[test]
    fn inline_diff_no_context() {
        let mut buf = capture();
        render_inline_diff("old\n", "new\n", &mut buf);
        let out = output_str(&buf);
        // Should have markers but no context lines.
        assert!(out.contains("old") || out.contains("new"));
    }

    #[test]
    fn inline_diff_empty_change() {
        let mut buf = capture();
        render_inline_diff("same\n", "same\n", &mut buf);
        let out = output_str(&buf);
        // No changes → empty output.
        assert!(out.is_empty());
    }

    // ── render_diff_summary ───────────────────────────────────

    #[test]
    fn summary_with_changes() {
        let mut buf = capture();
        render_diff_summary(5, 3, 2, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("+5"));
        assert!(out.contains("-3"));
        assert!(out.contains("~2"));
    }

    #[test]
    fn summary_no_changes() {
        let mut buf = capture();
        render_diff_summary(0, 0, 0, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("no changes"));
    }

    #[test]
    fn summary_added_only() {
        let mut buf = capture();
        render_diff_summary(10, 0, 0, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("+10"));
        assert!(!out.contains("-"));
        assert!(!out.contains("~"));
    }

    // ── render_gutter ─────────────────────────────────────────

    #[test]
    fn gutter_renders_markers() {
        let changes = vec![
            (1, ChangeKind::Added),
            (5, ChangeKind::Deleted),
            (10, ChangeKind::Modified),
        ];
        let mut buf = capture();
        render_gutter(&changes, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("1"));
        assert!(out.contains("5"));
        assert!(out.contains("10"));
    }

    #[test]
    fn gutter_empty() {
        let mut buf = capture();
        render_gutter(&[], &mut buf);
        let out = output_str(&buf);
        assert!(out.is_empty());
    }

    // ── ChangeKind ────────────────────────────────────────────

    #[test]
    fn gutter_chars() {
        assert_eq!(gutter_char(ChangeKind::Added), "+");
        assert_eq!(gutter_char(ChangeKind::Deleted), "-");
        assert_eq!(gutter_char(ChangeKind::Modified), "~");
        assert_eq!(gutter_char(ChangeKind::Context), " ");
        assert_eq!(gutter_char(ChangeKind::AiGenerated), "▸");
    }
}
