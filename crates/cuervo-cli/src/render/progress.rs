//! Progress bars — visual indicators for long-running operations.
//!
//! Supports:
//! - Determinate progress bars with percentage
//! - Indeterminate progress bars (spinner-style)
//! - Multi-step progress with step labels
//! - Budget gauge (cost/token limit tracking)

use std::io::Write;
use std::time::Instant;

use super::color;
use super::theme;

/// Style for a progress bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressStyle {
    /// Standard filled bar: [████░░░░░░] 42%
    Bar,
    /// Compact: ██▒▒ 42%
    Compact,
    /// Minimal dot: ●●●○○
    Dots,
}

/// A determinate progress bar state.
#[derive(Debug, Clone)]
pub struct ProgressBar {
    pub label: String,
    pub current: u64,
    pub total: u64,
    pub style: ProgressStyle,
    pub started_at: Instant,
}

impl ProgressBar {
    pub fn new(label: &str, total: u64) -> Self {
        Self {
            label: label.to_string(),
            current: 0,
            total,
            style: ProgressStyle::Bar,
            started_at: Instant::now(),
        }
    }

    pub fn with_style(mut self, style: ProgressStyle) -> Self {
        self.style = style;
        self
    }

    pub fn set(&mut self, current: u64) {
        self.current = current.min(self.total);
    }

    pub fn increment(&mut self) {
        self.current = (self.current + 1).min(self.total);
    }

    pub fn fraction(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.current as f64 / self.total as f64
    }

    pub fn is_complete(&self) -> bool {
        self.current >= self.total
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }
}

/// A multi-step progress tracker.
#[derive(Debug, Clone)]
pub struct StepProgress {
    pub steps: Vec<StepInfo>,
    pub current_step: usize,
}

/// Info about a single step.
#[derive(Debug, Clone)]
pub struct StepInfo {
    pub label: String,
    pub status: StepStatus,
}

/// Status of a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Pending,
    Running,
    Complete,
    Failed,
    Skipped,
}

impl StepProgress {
    pub fn new(labels: &[&str]) -> Self {
        Self {
            steps: labels
                .iter()
                .map(|l| StepInfo {
                    label: l.to_string(),
                    status: StepStatus::Pending,
                })
                .collect(),
            current_step: 0,
        }
    }

    pub fn advance(&mut self) {
        if self.current_step < self.steps.len() {
            self.steps[self.current_step].status = StepStatus::Complete;
            self.current_step += 1;
            if self.current_step < self.steps.len() {
                self.steps[self.current_step].status = StepStatus::Running;
            }
        }
    }

    pub fn fail_current(&mut self) {
        if self.current_step < self.steps.len() {
            self.steps[self.current_step].status = StepStatus::Failed;
        }
    }

    pub fn start(&mut self) {
        if !self.steps.is_empty() {
            self.steps[0].status = StepStatus::Running;
        }
    }

    pub fn completed_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| s.status == StepStatus::Complete)
            .count()
    }

    pub fn total(&self) -> usize {
        self.steps.len()
    }
}

/// A budget gauge for cost/token tracking.
#[derive(Debug, Clone)]
pub struct BudgetGauge {
    pub label: String,
    pub used: f64,
    pub limit: f64,
    pub unit: String,
}

impl BudgetGauge {
    pub fn new(label: &str, limit: f64, unit: &str) -> Self {
        Self {
            label: label.to_string(),
            used: 0.0,
            limit,
            unit: unit.to_string(),
        }
    }

    pub fn set_used(&mut self, used: f64) {
        self.used = used;
    }

    pub fn fraction(&self) -> f64 {
        if self.limit <= 0.0 {
            return 0.0;
        }
        (self.used / self.limit).min(1.0)
    }

    pub fn is_over_budget(&self) -> bool {
        self.limit > 0.0 && self.used >= self.limit
    }

    pub fn is_warning(&self) -> bool {
        self.fraction() >= 0.8
    }
}

// ── Rendering ──────────────────────────────────────────────────

/// Render a progress bar to a writer.
pub fn render_progress_bar(bar: &ProgressBar, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();
    let muted = t.palette.text_dim.fg();

    match bar.style {
        ProgressStyle::Bar => render_standard_bar(bar, out),
        ProgressStyle::Compact => render_compact_bar(bar, out),
        ProgressStyle::Dots => render_dots_bar(bar, out),
    }

    // Elapsed time.
    let elapsed = bar.elapsed_ms();
    if elapsed > 1000 {
        let _ = write!(out, " {muted}{:.1}s{r}", elapsed as f64 / 1000.0);
    }

    let _ = writeln!(out);
}

fn render_standard_bar(bar: &ProgressBar, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();
    let muted = t.palette.text_dim.fg();

    let width = 30;
    let filled = (bar.fraction() * width as f64) as usize;
    let empty = width - filled;
    let pct = (bar.fraction() * 100.0) as u32;

    let bar_color = if bar.is_complete() {
        t.palette.success.fg()
    } else {
        t.palette.primary.fg()
    };

    let fill_char = if color::unicode_enabled() { "█" } else { "#" };
    let empty_char = if color::unicode_enabled() { "░" } else { "-" };

    let _ = write!(
        out,
        "  {muted}{}{r} [{bar_color}{}{muted}{}{r}] {bar_color}{pct}%{r}",
        bar.label,
        fill_char.repeat(filled),
        empty_char.repeat(empty),
    );
}

fn render_compact_bar(bar: &ProgressBar, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();
    let muted = t.palette.text_dim.fg();

    let width = 15;
    let filled = (bar.fraction() * width as f64) as usize;
    let empty = width - filled;
    let pct = (bar.fraction() * 100.0) as u32;

    let bar_color = if bar.is_complete() {
        t.palette.success.fg()
    } else {
        t.palette.primary.fg()
    };

    let fill_char = if color::unicode_enabled() { "██" } else { "##" };
    let empty_char = if color::unicode_enabled() { "▒▒" } else { ".." };

    // In compact mode, halve the chars so 1 unit = 2 display chars.
    let fill_str = (if color::unicode_enabled() { "█" } else { "#" }).repeat(filled);
    let empty_str = (if color::unicode_enabled() { "▒" } else { "." }).repeat(empty);

    let _ = write!(
        out,
        "  {muted}{}{r} {bar_color}{fill_str}{muted}{empty_str}{r} {bar_color}{pct}%{r}",
        bar.label,
    );

    // Suppress unused variable warnings.
    let _ = fill_char;
    let _ = empty_char;
}

fn render_dots_bar(bar: &ProgressBar, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();
    let muted = t.palette.text_dim.fg();

    let total_dots = 10;
    let filled = (bar.fraction() * total_dots as f64) as usize;
    let empty = total_dots - filled;

    let bar_color = if bar.is_complete() {
        t.palette.success.fg()
    } else {
        t.palette.primary.fg()
    };

    let filled_dot = if color::unicode_enabled() { "●" } else { "*" };
    let empty_dot = if color::unicode_enabled() { "○" } else { "." };

    let _ = write!(
        out,
        "  {muted}{}{r} {bar_color}{}{muted}{}{r}",
        bar.label,
        filled_dot.repeat(filled),
        empty_dot.repeat(empty),
    );
}

/// Render a multi-step progress tracker.
pub fn render_step_progress(progress: &StepProgress, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();

    for (i, step) in progress.steps.iter().enumerate() {
        let (icon, color) = match step.status {
            StepStatus::Pending => {
                let icon = if color::unicode_enabled() { "○" } else { " " };
                (icon, t.palette.muted.fg())
            }
            StepStatus::Running => {
                let icon = if color::unicode_enabled() { "◉" } else { ">" };
                (icon, t.palette.primary.fg())
            }
            StepStatus::Complete => {
                let icon = if color::unicode_enabled() { "●" } else { "x" };
                (icon, t.palette.success.fg())
            }
            StepStatus::Failed => {
                let icon = if color::unicode_enabled() { "✖" } else { "!" };
                (icon, t.palette.error.fg())
            }
            StepStatus::Skipped => {
                let icon = if color::unicode_enabled() { "⊘" } else { "-" };
                (icon, t.palette.muted.fg())
            }
        };

        // Connector line between steps.
        if i > 0 {
            let conn_color = if step.status == StepStatus::Complete || step.status == StepStatus::Running {
                t.palette.primary.fg()
            } else {
                t.palette.muted.fg()
            };
            let _ = writeln!(out, "  {conn_color}│{r}");
        }

        let _ = writeln!(out, "  {color}{icon} {}{r}", step.label);
    }
}

/// Render a budget gauge.
pub fn render_budget_gauge(gauge: &BudgetGauge, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();
    let muted = t.palette.text_dim.fg();

    let width = 20;
    let filled = (gauge.fraction() * width as f64) as usize;
    let empty = width - filled;

    let bar_color = if gauge.is_over_budget() {
        t.palette.error.fg()
    } else if gauge.is_warning() {
        t.palette.warning.fg()
    } else {
        t.palette.success.fg()
    };

    let fill_char = if color::unicode_enabled() { "█" } else { "#" };
    let empty_char = if color::unicode_enabled() { "░" } else { "-" };

    let _ = writeln!(
        out,
        "  {muted}{}{r} [{bar_color}{}{muted}{}{r}] {bar_color}{:.2}{r}/{:.2} {}",
        gauge.label,
        fill_char.repeat(filled),
        empty_char.repeat(empty),
        gauge.used,
        gauge.limit,
        gauge.unit,
    );
}

// ── Token rate rendering ───────────────────────────────────────

/// Render a token rate indicator.
pub fn render_token_rate(tokens_per_sec: f64, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();

    let color = if tokens_per_sec > 50.0 {
        t.palette.success.fg()
    } else if tokens_per_sec > 20.0 {
        t.palette.primary.fg()
    } else if tokens_per_sec > 5.0 {
        t.palette.warning.fg()
    } else {
        t.palette.error.fg()
    };

    let _ = write!(out, "{color}{:.0} tok/s{r}", tokens_per_sec);
}

/// Render a step timeline (horizontal).
pub fn render_step_timeline(
    steps: &[(String, u64, bool)], // (label, duration_ms, success)
    out: &mut impl Write,
) {
    let t = theme::active();
    let r = theme::reset();
    let muted = t.palette.text_dim.fg();

    for (i, (label, dur_ms, success)) in steps.iter().enumerate() {
        if i > 0 {
            let _ = write!(out, " {muted}→{r} ");
        }

        let color = if *success {
            t.palette.success.fg()
        } else {
            t.palette.error.fg()
        };

        let dur_str = if *dur_ms < 1000 {
            format!("{dur_ms}ms")
        } else {
            format!("{:.1}s", *dur_ms as f64 / 1000.0)
        };

        let _ = write!(out, "{color}{label}{r}{muted}({dur_str}){r}");
    }
    let _ = writeln!(out);
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

    // ── ProgressBar ─────────────────────────────────────────────

    #[test]
    fn progress_bar_fraction() {
        let mut bar = ProgressBar::new("test", 100);
        assert_eq!(bar.fraction(), 0.0);
        bar.set(50);
        assert!((bar.fraction() - 0.5).abs() < f64::EPSILON);
        bar.set(100);
        assert!((bar.fraction() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn progress_bar_increment() {
        let mut bar = ProgressBar::new("test", 3);
        bar.increment();
        assert_eq!(bar.current, 1);
        bar.increment();
        assert_eq!(bar.current, 2);
        bar.increment();
        assert_eq!(bar.current, 3);
        bar.increment(); // Should not exceed total.
        assert_eq!(bar.current, 3);
    }

    #[test]
    fn progress_bar_complete() {
        let mut bar = ProgressBar::new("test", 5);
        assert!(!bar.is_complete());
        bar.set(5);
        assert!(bar.is_complete());
    }

    #[test]
    fn progress_bar_zero_total() {
        let bar = ProgressBar::new("test", 0);
        assert_eq!(bar.fraction(), 0.0);
    }

    #[test]
    fn progress_bar_clamps_over_total() {
        let mut bar = ProgressBar::new("test", 10);
        bar.set(999);
        assert_eq!(bar.current, 10);
    }

    #[test]
    fn render_standard_bar() {
        let bar = ProgressBar::new("Loading", 100);
        let mut buf = capture();
        render_progress_bar(&bar, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("Loading"));
        assert!(out.contains("0%"));
    }

    #[test]
    fn render_standard_bar_half() {
        let mut bar = ProgressBar::new("Indexing", 100);
        bar.set(50);
        let mut buf = capture();
        render_progress_bar(&bar, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("50%"));
    }

    #[test]
    fn render_standard_bar_complete() {
        let mut bar = ProgressBar::new("Done", 10);
        bar.set(10);
        let mut buf = capture();
        render_progress_bar(&bar, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("100%"));
    }

    #[test]
    fn render_compact_style() {
        let bar = ProgressBar::new("Sync", 100).with_style(ProgressStyle::Compact);
        let mut buf = capture();
        render_progress_bar(&bar, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("Sync"));
        assert!(out.contains("0%"));
    }

    #[test]
    fn render_dots_style() {
        let mut bar = ProgressBar::new("Steps", 10).with_style(ProgressStyle::Dots);
        bar.set(5);
        let mut buf = capture();
        render_progress_bar(&bar, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("Steps"));
    }

    // ── StepProgress ────────────────────────────────────────────

    #[test]
    fn step_progress_advance() {
        let mut sp = StepProgress::new(&["Plan", "Execute", "Verify"]);
        sp.start();
        assert_eq!(sp.steps[0].status, StepStatus::Running);
        sp.advance();
        assert_eq!(sp.steps[0].status, StepStatus::Complete);
        assert_eq!(sp.steps[1].status, StepStatus::Running);
        assert_eq!(sp.completed_count(), 1);
    }

    #[test]
    fn step_progress_fail() {
        let mut sp = StepProgress::new(&["A", "B"]);
        sp.start();
        sp.fail_current();
        assert_eq!(sp.steps[0].status, StepStatus::Failed);
    }

    #[test]
    fn step_progress_all_complete() {
        let mut sp = StepProgress::new(&["X", "Y"]);
        sp.start();
        sp.advance();
        sp.advance();
        assert_eq!(sp.completed_count(), 2);
        assert_eq!(sp.total(), 2);
    }

    #[test]
    fn render_step_progress_output() {
        let mut sp = StepProgress::new(&["Plan", "Execute", "Verify"]);
        sp.start();
        sp.advance();
        let mut buf = capture();
        render_step_progress(&sp, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("Plan"));
        assert!(out.contains("Execute"));
        assert!(out.contains("Verify"));
    }

    // ── BudgetGauge ─────────────────────────────────────────────

    #[test]
    fn budget_gauge_fraction() {
        let mut gauge = BudgetGauge::new("Cost", 1.0, "USD");
        gauge.set_used(0.5);
        assert!((gauge.fraction() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn budget_gauge_over_budget() {
        let mut gauge = BudgetGauge::new("Tokens", 1000.0, "tok");
        assert!(!gauge.is_over_budget());
        gauge.set_used(1000.0);
        assert!(gauge.is_over_budget());
    }

    #[test]
    fn budget_gauge_warning() {
        let mut gauge = BudgetGauge::new("Cost", 1.0, "USD");
        gauge.set_used(0.79);
        assert!(!gauge.is_warning());
        gauge.set_used(0.80);
        assert!(gauge.is_warning());
    }

    #[test]
    fn budget_gauge_zero_limit() {
        let gauge = BudgetGauge::new("X", 0.0, "");
        assert_eq!(gauge.fraction(), 0.0);
        assert!(!gauge.is_over_budget());
    }

    #[test]
    fn render_budget_gauge_output() {
        let mut gauge = BudgetGauge::new("Cost", 10.0, "USD");
        gauge.set_used(3.5);
        let mut buf = capture();
        render_budget_gauge(&gauge, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("Cost"));
        assert!(out.contains("3.50"));
        assert!(out.contains("10.00"));
        assert!(out.contains("USD"));
    }

    // ── Token rate ──────────────────────────────────────────────

    #[test]
    fn render_token_rate_output() {
        let mut buf = capture();
        render_token_rate(42.5, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("43") || out.contains("42"));
        assert!(out.contains("tok/s"));
    }

    // ── Step timeline ───────────────────────────────────────────

    #[test]
    fn render_step_timeline_output() {
        let steps = vec![
            ("plan".into(), 100u64, true),
            ("exec".into(), 2500u64, true),
            ("verify".into(), 50u64, false),
        ];
        let mut buf = capture();
        render_step_timeline(&steps, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("plan"));
        assert!(out.contains("100ms"));
        assert!(out.contains("exec"));
        assert!(out.contains("2.5s"));
        assert!(out.contains("verify"));
    }

    #[test]
    fn render_empty_timeline() {
        let mut buf = capture();
        render_step_timeline(&[], &mut buf);
        let out = output_str(&buf);
        assert!(out.trim().is_empty() || out == "\n");
    }

    #[test]
    fn with_style_changes_style() {
        let bar = ProgressBar::new("test", 10).with_style(ProgressStyle::Dots);
        assert_eq!(bar.style, ProgressStyle::Dots);
    }
}
