//! Background job management: spawn, monitor, and kill long-running processes.

pub mod kill;
pub mod output;
pub mod start;

pub use kill::BackgroundKillTool;
pub use output::BackgroundOutputTool;
pub use start::BackgroundStartTool;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use tokio::process::Child;

/// A background process tracked by the registry.
pub struct BackgroundProcess {
    pub job_id: String,
    pub command: String,
    pub child: Option<Child>,
    pub started_at: std::time::Instant,
    pub stdout_buf: String,
    pub stderr_buf: String,
    pub exit_code: Option<i32>,
    pub finished: bool,
}

/// Registry of background processes with max-concurrent enforcement.
pub struct ProcessRegistry {
    processes: Mutex<HashMap<String, BackgroundProcess>>,
    counter: AtomicU32,
    max_concurrent: usize,
}

impl ProcessRegistry {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            processes: Mutex::new(HashMap::new()),
            counter: AtomicU32::new(0),
            max_concurrent,
        }
    }

    /// Generate a new unique job ID.
    pub fn next_id(&self) -> String {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        format!("bg-{n}")
    }

    /// Register a new background process. Returns error if at capacity.
    pub fn register(&self, process: BackgroundProcess) -> Result<(), String> {
        let mut procs = self.processes.lock().unwrap_or_else(|e| e.into_inner());

        // Clean up finished processes first.
        procs.retain(|_, p| !p.finished);

        let active_count = procs.values().filter(|p| !p.finished).count();
        if active_count >= self.max_concurrent {
            return Err(format!(
                "maximum concurrent jobs ({}) reached",
                self.max_concurrent
            ));
        }

        procs.insert(process.job_id.clone(), process);
        Ok(())
    }

    /// Get a reference to a process, updating its buffers from the child.
    pub fn get_output(&self, job_id: &str) -> Option<(String, String, bool, Option<i32>, u64)> {
        let procs = self.processes.lock().unwrap_or_else(|e| e.into_inner());
        procs.get(job_id).map(|p| {
            (
                p.stdout_buf.clone(),
                p.stderr_buf.clone(),
                p.finished,
                p.exit_code,
                p.started_at.elapsed().as_secs(),
            )
        })
    }

    /// Update a process's output buffers and finished status.
    pub fn update_output(
        &self,
        job_id: &str,
        stdout: &str,
        stderr: &str,
        finished: bool,
        exit_code: Option<i32>,
    ) {
        let mut procs = self.processes.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = procs.get_mut(job_id) {
            p.stdout_buf.push_str(stdout);
            p.stderr_buf.push_str(stderr);
            if finished {
                p.finished = true;
                p.exit_code = exit_code;
            }
        }
    }

    /// Kill a background process. Returns (was_running, exit_code).
    pub fn kill(&self, job_id: &str) -> Option<(bool, Option<i32>)> {
        let mut procs = self.processes.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = procs.get_mut(job_id) {
            let was_running = !p.finished;
            if let Some(ref mut child) = p.child {
                let _ = child.start_kill();
            }
            p.finished = true;
            Some((was_running, p.exit_code))
        } else {
            None
        }
    }

    /// Take the child process out for async waiting.
    pub fn take_child(&self, job_id: &str) -> Option<Child> {
        let mut procs = self.processes.lock().unwrap_or_else(|e| e.into_inner());
        procs.get_mut(job_id).and_then(|p| p.child.take())
    }

    /// List all job IDs with their running status.
    pub fn list(&self) -> Vec<(String, bool, u64)> {
        let procs = self.processes.lock().unwrap_or_else(|e| e.into_inner());
        procs
            .values()
            .map(|p| {
                (
                    p.job_id.clone(),
                    p.finished,
                    p.started_at.elapsed().as_secs(),
                )
            })
            .collect()
    }

    /// Remove finished processes.
    pub fn cleanup(&self) -> usize {
        let mut procs = self.processes.lock().unwrap_or_else(|e| e.into_inner());
        let before = procs.len();
        procs.retain(|_, p| !p.finished);
        before - procs.len()
    }
}

impl Drop for ProcessRegistry {
    fn drop(&mut self) {
        // Kill all remaining child processes.
        let mut procs = self.processes.lock().unwrap_or_else(|e| e.into_inner());
        for p in procs.values_mut() {
            if let Some(ref mut child) = p.child {
                let _ = child.start_kill();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_id_generation() {
        let reg = ProcessRegistry::new(5);
        assert_eq!(reg.next_id(), "bg-0");
        assert_eq!(reg.next_id(), "bg-1");
        assert_eq!(reg.next_id(), "bg-2");
    }

    #[test]
    fn registry_max_concurrent() {
        let reg = ProcessRegistry::new(2);

        for i in 0..2 {
            let p = BackgroundProcess {
                job_id: format!("bg-{i}"),
                command: "sleep 10".into(),
                child: None,
                started_at: std::time::Instant::now(),
                stdout_buf: String::new(),
                stderr_buf: String::new(),
                exit_code: None,
                finished: false,
            };
            assert!(reg.register(p).is_ok());
        }

        // Third should fail.
        let p = BackgroundProcess {
            job_id: "bg-2".into(),
            command: "sleep 10".into(),
            child: None,
            started_at: std::time::Instant::now(),
            stdout_buf: String::new(),
            stderr_buf: String::new(),
            exit_code: None,
            finished: false,
        };
        assert!(reg.register(p).is_err());
    }

    #[test]
    fn registry_cleanup() {
        let reg = ProcessRegistry::new(5);

        // Register both as running.
        let p1 = BackgroundProcess {
            job_id: "bg-0".into(),
            command: "echo".into(),
            child: None,
            started_at: std::time::Instant::now(),
            stdout_buf: String::new(),
            stderr_buf: String::new(),
            exit_code: None,
            finished: false,
        };
        let p2 = BackgroundProcess {
            job_id: "bg-1".into(),
            command: "sleep".into(),
            child: None,
            started_at: std::time::Instant::now(),
            stdout_buf: String::new(),
            stderr_buf: String::new(),
            exit_code: None,
            finished: false,
        };
        reg.register(p1).unwrap();
        reg.register(p2).unwrap();
        assert_eq!(reg.list().len(), 2);

        // Mark bg-0 as finished.
        reg.update_output("bg-0", "done\n", "", true, Some(0));

        // Cleanup should remove bg-0.
        assert_eq!(reg.cleanup(), 1);
        assert_eq!(reg.list().len(), 1);
    }

    #[test]
    fn registry_update_and_get_output() {
        let reg = ProcessRegistry::new(5);
        let p = BackgroundProcess {
            job_id: "bg-0".into(),
            command: "echo".into(),
            child: None,
            started_at: std::time::Instant::now(),
            stdout_buf: String::new(),
            stderr_buf: String::new(),
            exit_code: None,
            finished: false,
        };
        reg.register(p).unwrap();

        reg.update_output("bg-0", "hello\n", "", true, Some(0));
        let (stdout, _, finished, exit_code, _) = reg.get_output("bg-0").unwrap();
        assert_eq!(stdout, "hello\n");
        assert!(finished);
        assert_eq!(exit_code, Some(0));
    }

    #[test]
    fn registry_unknown_job() {
        let reg = ProcessRegistry::new(5);
        assert!(reg.get_output("bg-999").is_none());
        assert!(reg.kill("bg-999").is_none());
    }
}
