//! Render performance benchmarks — measure and validate rendering latency.
//!
//! Provides:
//! - Timing harness for render operations
//! - Performance report rendering
//! - Threshold validation (target: <50ms initial, <16ms incremental)

use std::io::Write;
use std::time::{Duration, Instant};

use super::color;
use super::theme;

/// Performance thresholds (in microseconds for precision).
pub const INITIAL_RENDER_TARGET_US: u64 = 50_000; // 50ms
pub const INCREMENTAL_RENDER_TARGET_US: u64 = 16_000; // 16ms (~60fps)
pub const STREAMING_CHUNK_TARGET_US: u64 = 50_000; // 50ms per chunk

/// Result of a single timing measurement.
#[derive(Debug, Clone)]
pub struct TimingResult {
    pub label: String,
    pub duration: Duration,
    pub target_us: u64,
}

impl TimingResult {
    pub fn passes(&self) -> bool {
        self.duration.as_micros() as u64 <= self.target_us
    }

    pub fn ratio(&self) -> f64 {
        if self.target_us == 0 {
            return 0.0;
        }
        self.duration.as_micros() as f64 / self.target_us as f64
    }
}

/// Aggregate benchmark results.
#[derive(Debug, Clone, Default)]
pub struct BenchmarkReport {
    pub results: Vec<TimingResult>,
}

impl BenchmarkReport {
    pub fn new() -> Self {
        Self {
            results: Vec::new(),
        }
    }

    pub fn add(&mut self, label: &str, duration: Duration, target_us: u64) {
        self.results.push(TimingResult {
            label: label.to_string(),
            duration,
            target_us,
        });
    }

    pub fn all_pass(&self) -> bool {
        self.results.iter().all(|r| r.passes())
    }

    pub fn pass_count(&self) -> usize {
        self.results.iter().filter(|r| r.passes()).count()
    }

    pub fn total_count(&self) -> usize {
        self.results.len()
    }
}

/// A simple timing scope for measuring render operations.
pub struct RenderTimer {
    start: Instant,
    label: String,
    target_us: u64,
}

impl RenderTimer {
    /// Start timing an initial render.
    pub fn initial(label: &str) -> Self {
        Self {
            start: Instant::now(),
            label: label.to_string(),
            target_us: INITIAL_RENDER_TARGET_US,
        }
    }

    /// Start timing an incremental render.
    pub fn incremental(label: &str) -> Self {
        Self {
            start: Instant::now(),
            label: label.to_string(),
            target_us: INCREMENTAL_RENDER_TARGET_US,
        }
    }

    /// Start timing a streaming chunk.
    pub fn streaming(label: &str) -> Self {
        Self {
            start: Instant::now(),
            label: label.to_string(),
            target_us: STREAMING_CHUNK_TARGET_US,
        }
    }

    /// Stop timer and return result.
    pub fn stop(self) -> TimingResult {
        TimingResult {
            label: self.label,
            duration: self.start.elapsed(),
            target_us: self.target_us,
        }
    }
}

/// Time a closure and return its result plus timing.
pub fn time_render<F, R>(label: &str, target_us: u64, f: F) -> (R, TimingResult)
where
    F: FnOnce() -> R,
{
    let start = Instant::now();
    let result = f();
    let timing = TimingResult {
        label: label.to_string(),
        duration: start.elapsed(),
        target_us,
    };
    (result, timing)
}

// ── Rendering ──────────────────────────────────────────────────

/// Render a benchmark report.
pub fn render_benchmark_report(report: &BenchmarkReport, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();
    let muted = t.palette.text_dim.fg();
    let accent = t.palette.accent.fg();
    let bold = if color::color_enabled() { "\x1b[1m" } else { "" };

    let _ = writeln!(out, "\n  {bold}{accent}Render Performance{r}");

    // Header.
    let _ = writeln!(
        out,
        "  {muted}{:<24} {:>10} {:>10} {:>8}{r}",
        "Operation", "Time", "Target", "Status"
    );
    let rule = "─".repeat(56);
    let _ = writeln!(out, "  {muted}{rule}{r}");

    for result in &report.results {
        let time_str = format_duration_precise(result.duration);
        let target_str = format_us(result.target_us);

        let (status_icon, status_color) = if result.passes() {
            let icon = if color::unicode_enabled() { "✓" } else { "OK" };
            (icon, t.palette.success.fg())
        } else {
            let icon = if color::unicode_enabled() { "✖" } else { "FAIL" };
            (icon, t.palette.error.fg())
        };

        let ratio = result.ratio();
        let time_color = if ratio <= 0.5 {
            t.palette.success.fg()
        } else if ratio <= 1.0 {
            t.palette.warning.fg()
        } else {
            t.palette.error.fg()
        };

        let _ = writeln!(
            out,
            "  {muted}{:<24}{r} {time_color}{:>10}{r} {muted}{:>10}{r} {status_color}{:>8}{r}",
            result.label, time_str, target_str, status_icon,
        );
    }

    // Summary.
    let _ = writeln!(out, "  {muted}{rule}{r}");
    let pass = report.pass_count();
    let total = report.total_count();
    let summary_color = if report.all_pass() {
        t.palette.success.fg()
    } else {
        t.palette.warning.fg()
    };
    let _ = writeln!(out, "  {summary_color}{pass}/{total} passed{r}");
}

/// Render a single timing result inline.
pub fn render_timing_inline(result: &TimingResult, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();
    let muted = t.palette.text_dim.fg();

    let time_str = format_duration_precise(result.duration);
    let color = if result.passes() {
        t.palette.success.fg()
    } else {
        t.palette.error.fg()
    };

    let _ = write!(
        out,
        "{muted}{}{r} {color}{time_str}{r}",
        result.label,
    );
}

fn format_duration_precise(d: Duration) -> String {
    let us = d.as_micros();
    if us < 1000 {
        format!("{us}µs")
    } else if us < 1_000_000 {
        format!("{:.2}ms", us as f64 / 1000.0)
    } else {
        format!("{:.2}s", us as f64 / 1_000_000.0)
    }
}

fn format_us(us: u64) -> String {
    if us < 1000 {
        format!("{us}µs")
    } else if us < 1_000_000 {
        format!("{:.0}ms", us as f64 / 1000.0)
    } else {
        format!("{:.1}s", us as f64 / 1_000_000.0)
    }
}

// ── Memory tracking ────────────────────────────────────────────

/// Simple memory usage snapshot (best-effort, not exact).
#[derive(Debug, Clone, Default)]
pub struct MemorySnapshot {
    pub label: String,
    pub estimated_bytes: usize,
}

impl MemorySnapshot {
    pub fn new(label: &str, estimated_bytes: usize) -> Self {
        Self {
            label: label.to_string(),
            estimated_bytes,
        }
    }

    pub fn formatted_size(&self) -> String {
        format_bytes(self.estimated_bytes)
    }
}

fn format_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Render a memory usage report.
pub fn render_memory_report(snapshots: &[MemorySnapshot], out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();
    let muted = t.palette.text_dim.fg();
    let accent = t.palette.accent.fg();
    let bold = if color::color_enabled() { "\x1b[1m" } else { "" };

    let _ = writeln!(out, "\n  {bold}{accent}Memory Usage{r}");

    for snap in snapshots {
        let size_str = snap.formatted_size();
        let size_color = if snap.estimated_bytes < 1024 * 1024 {
            t.palette.success.fg()
        } else if snap.estimated_bytes < 10 * 1024 * 1024 {
            t.palette.warning.fg()
        } else {
            t.palette.error.fg()
        };

        let _ = writeln!(
            out,
            "  {muted}{:<24}{r} {size_color}{:>10}{r}",
            snap.label, size_str,
        );
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

    // ── TimingResult ────────────────────────────────────────────

    #[test]
    fn timing_passes_under_target() {
        let r = TimingResult {
            label: "test".into(),
            duration: Duration::from_micros(100),
            target_us: 1000,
        };
        assert!(r.passes());
    }

    #[test]
    fn timing_fails_over_target() {
        let r = TimingResult {
            label: "test".into(),
            duration: Duration::from_millis(100),
            target_us: 1000,
        };
        assert!(!r.passes());
    }

    #[test]
    fn timing_ratio() {
        let r = TimingResult {
            label: "test".into(),
            duration: Duration::from_micros(500),
            target_us: 1000,
        };
        assert!((r.ratio() - 0.5).abs() < 0.01);
    }

    #[test]
    fn timing_ratio_zero_target() {
        let r = TimingResult {
            label: "test".into(),
            duration: Duration::from_millis(1),
            target_us: 0,
        };
        assert_eq!(r.ratio(), 0.0);
    }

    // ── BenchmarkReport ─────────────────────────────────────────

    #[test]
    fn report_all_pass() {
        let mut report = BenchmarkReport::new();
        report.add("a", Duration::from_micros(10), 1000);
        report.add("b", Duration::from_micros(100), 1000);
        assert!(report.all_pass());
        assert_eq!(report.pass_count(), 2);
        assert_eq!(report.total_count(), 2);
    }

    #[test]
    fn report_some_fail() {
        let mut report = BenchmarkReport::new();
        report.add("a", Duration::from_micros(10), 1000);
        report.add("b", Duration::from_millis(100), 1000);
        assert!(!report.all_pass());
        assert_eq!(report.pass_count(), 1);
    }

    #[test]
    fn report_empty() {
        let report = BenchmarkReport::new();
        assert!(report.all_pass()); // vacuously true
        assert_eq!(report.total_count(), 0);
    }

    // ── RenderTimer ─────────────────────────────────────────────

    #[test]
    fn render_timer_initial() {
        let timer = RenderTimer::initial("test");
        // Just ensure it doesn't panic and returns a result.
        let result = timer.stop();
        assert_eq!(result.label, "test");
        assert_eq!(result.target_us, INITIAL_RENDER_TARGET_US);
    }

    #[test]
    fn render_timer_incremental() {
        let timer = RenderTimer::incremental("delta");
        let result = timer.stop();
        assert_eq!(result.target_us, INCREMENTAL_RENDER_TARGET_US);
    }

    #[test]
    fn render_timer_streaming() {
        let timer = RenderTimer::streaming("chunk");
        let result = timer.stop();
        assert_eq!(result.target_us, STREAMING_CHUNK_TARGET_US);
    }

    // ── time_render ─────────────────────────────────────────────

    #[test]
    fn time_render_closure() {
        let (result, timing) = time_render("add", 1000, || 2 + 2);
        assert_eq!(result, 4);
        assert_eq!(timing.label, "add");
    }

    // ── render_benchmark_report ─────────────────────────────────

    #[test]
    fn render_report_output() {
        let mut report = BenchmarkReport::new();
        report.add("initial render", Duration::from_millis(12), INITIAL_RENDER_TARGET_US);
        report.add("incremental", Duration::from_micros(500), INCREMENTAL_RENDER_TARGET_US);
        report.add("streaming chunk", Duration::from_millis(80), STREAMING_CHUNK_TARGET_US);

        let mut buf = capture();
        render_benchmark_report(&report, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("Render Performance"));
        assert!(out.contains("initial render"));
        assert!(out.contains("incremental"));
        assert!(out.contains("streaming chunk"));
        assert!(out.contains("passed"));
    }

    #[test]
    fn render_report_empty() {
        let report = BenchmarkReport::new();
        let mut buf = capture();
        render_benchmark_report(&report, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("0/0"));
    }

    // ── render_timing_inline ────────────────────────────────────

    #[test]
    fn render_timing_inline_output() {
        let result = TimingResult {
            label: "render".into(),
            duration: Duration::from_micros(500),
            target_us: 1000,
        };
        let mut buf = capture();
        render_timing_inline(&result, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("render"));
    }

    // ── format_duration_precise ─────────────────────────────────

    #[test]
    fn format_microseconds() {
        assert_eq!(format_duration_precise(Duration::from_micros(500)), "500µs");
    }

    #[test]
    fn format_milliseconds() {
        let s = format_duration_precise(Duration::from_millis(42));
        assert!(s.contains("ms"));
    }

    #[test]
    fn format_seconds() {
        let s = format_duration_precise(Duration::from_secs(2));
        assert!(s.contains("s"));
    }

    // ── MemorySnapshot ──────────────────────────────────────────

    #[test]
    fn memory_bytes() {
        let snap = MemorySnapshot::new("test", 512);
        assert_eq!(snap.formatted_size(), "512B");
    }

    #[test]
    fn memory_kilobytes() {
        let snap = MemorySnapshot::new("test", 2048);
        assert_eq!(snap.formatted_size(), "2.0KB");
    }

    #[test]
    fn memory_megabytes() {
        let snap = MemorySnapshot::new("test", 5 * 1024 * 1024);
        assert_eq!(snap.formatted_size(), "5.0MB");
    }

    #[test]
    fn render_memory_report_output() {
        let snapshots = vec![
            MemorySnapshot::new("buffer", 4096),
            MemorySnapshot::new("cache", 2 * 1024 * 1024),
        ];
        let mut buf = capture();
        render_memory_report(&snapshots, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("Memory Usage"));
        assert!(out.contains("buffer"));
        assert!(out.contains("cache"));
    }

    // ── format_us ───────────────────────────────────────────────

    #[test]
    fn format_us_micro() {
        assert_eq!(format_us(500), "500µs");
    }

    #[test]
    fn format_us_milli() {
        assert_eq!(format_us(50_000), "50ms");
    }

    #[test]
    fn format_us_seconds() {
        let s = format_us(1_500_000);
        assert!(s.contains("1.5s"));
    }
}
