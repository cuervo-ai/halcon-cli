use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;

use super::animations;
use super::theme;

/// Lightweight async spinner for inference waiting feedback.
///
/// Shows a themed spinner frame + "{label} ({elapsed}s)" on stderr,
/// updating every 80ms. Appears only after an initial delay (default 200ms)
/// to avoid flicker on fast responses.
pub struct Spinner {
    active: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Spinner {
    /// Start a spinner with a 200ms delay before first display.
    ///
    /// If `stop()` is called before the delay elapses, nothing is printed.
    pub fn start(label: &str) -> Self {
        Self::start_delayed(label, Duration::from_millis(200))
    }

    /// Start a spinner with a custom delay before first display.
    pub fn start_delayed(label: &str, delay: Duration) -> Self {
        let active = Arc::new(AtomicBool::new(true));
        let active_clone = Arc::clone(&active);
        let label = label.to_string();

        let invoke_start = Instant::now();

        let handle = tokio::spawn(async move {
            // Wait for the delay before showing anything.
            tokio::time::sleep(delay).await;
            if !active_clone.load(Ordering::Relaxed) {
                return;
            }

            let frames = animations::spinner_frames();
            let t = theme::active();
            let r = theme::reset();
            let primary = t.palette.primary.fg();
            let dim = t.palette.text_dim.fg();
            let mut idx: usize = 0;

            loop {
                if !active_clone.load(Ordering::Relaxed) {
                    break;
                }
                let frame = frames[idx % frames.len()];
                idx += 1;

                // Show total elapsed since invoke started (not since spinner appeared).
                let elapsed = invoke_start.elapsed().as_secs_f64();
                {
                    let mut out = io::stderr().lock();
                    let _ = write!(
                        out,
                        "\r  {primary}{frame}{r} {label} {dim}({elapsed:.1}s){r}",
                    );
                    let _ = out.flush();
                }
                tokio::time::sleep(Duration::from_millis(80)).await;
            }
        });

        Self {
            active,
            handle: Some(handle),
        }
    }

    /// Stop the spinner and clear the line.
    pub fn stop(&self) {
        if self.active.swap(false, Ordering::Relaxed) {
            // Abort the spawned task so the runtime doesn't wait for it.
            if let Some(ref h) = self.handle {
                h.abort();
            }
            let mut out = io::stderr().lock();
            if super::color::color_enabled() {
                let _ = write!(out, "\r\x1b[K");
            } else {
                // Fallback: overwrite with spaces and carriage return.
                let _ = write!(out, "\r{:>60}\r", "");
            }
            let _ = out.flush();
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spinner_starts_and_stops_without_panic() {
        let spinner = Spinner::start_delayed("Thinking...", Duration::from_millis(10));
        tokio::time::sleep(Duration::from_millis(50)).await;
        spinner.stop();
    }

    #[tokio::test]
    async fn spinner_stop_before_delay_no_output() {
        let spinner = Spinner::start_delayed("Thinking...", Duration::from_secs(10));
        // Stop immediately — spinner should never have printed.
        spinner.stop();
    }

    #[tokio::test]
    async fn spinner_drop_cleans_up() {
        let spinner = Spinner::start_delayed("Thinking...", Duration::from_millis(10));
        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(spinner);
        // Should not panic or leave artifacts.
    }

    #[tokio::test]
    async fn spinner_abort_allows_runtime_exit() {
        // Verify the spawned task is aborted and doesn't prevent runtime shutdown.
        let spinner = Spinner::start_delayed("Testing...", Duration::from_millis(10));
        tokio::time::sleep(Duration::from_millis(50)).await;
        spinner.stop();
        // If the task weren't aborted, this test would hang.
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
