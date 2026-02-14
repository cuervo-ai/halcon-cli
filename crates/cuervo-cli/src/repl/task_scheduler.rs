//! Task scheduler — wave-based scheduling with retry delay computation.

use std::time::Duration;

use uuid::Uuid;

use cuervo_core::types::StructuredTaskStatus;

use super::task_backlog::TaskBacklog;

/// Simple scheduler that produces waves of ready tasks.
pub(crate) struct TaskScheduler {
    pub max_concurrent: usize,
}

impl TaskScheduler {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            max_concurrent: max_concurrent.max(1),
        }
    }

    /// Get the next wave of ready tasks (up to max_concurrent).
    pub fn next_wave<'a>(&self, backlog: &'a TaskBacklog) -> Vec<&'a cuervo_core::types::StructuredTask> {
        backlog.ready_wave(self.max_concurrent)
    }

    /// Compute retry delay for a task using exponential backoff.
    pub fn retry_delay(task: &cuervo_core::types::StructuredTask) -> Duration {
        let base = task.retry_policy.base_delay_ms as f64;
        let multiplier = task.retry_policy.backoff_multiplier;
        let count = task.retry_count.saturating_sub(1) as f64; // retry_count is already incremented
        let delay = base * multiplier.powf(count);
        let capped = delay.min(task.retry_policy.max_delay_ms as f64);
        Duration::from_millis(capped as u64)
    }

    /// Whether there is any work left to do (Ready or Retrying tasks exist).
    pub fn has_work(backlog: &TaskBacklog) -> bool {
        backlog.tasks().any(|t| t.status.is_actionable())
    }

    /// Preview topological execution waves without modifying state.
    /// Returns groups of task IDs that can execute in parallel.
    pub fn preview_waves(backlog: &TaskBacklog) -> Vec<Vec<Uuid>> {
        let mut waves = Vec::new();
        let mut completed: std::collections::HashSet<Uuid> = std::collections::HashSet::new();

        // Collect tasks already in terminal states.
        for task in backlog.tasks() {
            if task.status.is_terminal() {
                completed.insert(task.task_id);
            }
        }

        loop {
            let wave: Vec<Uuid> = backlog
                .tasks()
                .filter(|t| {
                    !completed.contains(&t.task_id)
                        && !matches!(t.status, StructuredTaskStatus::Cancelled)
                        && t.depends_on.iter().all(|d| completed.contains(d))
                })
                .map(|t| t.task_id)
                .collect();

            if wave.is_empty() {
                break;
            }

            for id in &wave {
                completed.insert(*id);
            }
            waves.push(wave);
        }

        waves
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cuervo_core::types::{RetryPolicy, StructuredTask};

    fn make_task(title: &str, priority: u32) -> StructuredTask {
        StructuredTask {
            title: title.into(),
            description: title.into(),
            priority,
            ..Default::default()
        }
    }

    fn make_task_with_deps(title: &str, priority: u32, deps: Vec<Uuid>) -> StructuredTask {
        StructuredTask {
            title: title.into(),
            description: title.into(),
            priority,
            depends_on: deps,
            ..Default::default()
        }
    }

    #[test]
    fn next_wave_returns_ready_tasks() {
        let scheduler = TaskScheduler::new(3);
        let mut backlog = TaskBacklog::new();
        for i in 0..5 {
            backlog.add_task(make_task(&format!("T{i}"), i as u32)).unwrap();
        }

        let wave = scheduler.next_wave(&backlog);
        assert_eq!(wave.len(), 3);
    }

    #[test]
    fn retry_delay_exponential() {
        let mut task = make_task("T", 5);
        task.retry_policy = RetryPolicy {
            max_retries: 5,
            base_delay_ms: 1000,
            max_delay_ms: 60_000,
            backoff_multiplier: 2.0,
            idempotent: true,
        };

        // First retry (count=1, exponent=0).
        task.retry_count = 1;
        let d1 = TaskScheduler::retry_delay(&task);
        assert_eq!(d1.as_millis(), 1000);

        // Second retry (count=2, exponent=1).
        task.retry_count = 2;
        let d2 = TaskScheduler::retry_delay(&task);
        assert_eq!(d2.as_millis(), 2000);

        // Third retry (count=3, exponent=2).
        task.retry_count = 3;
        let d3 = TaskScheduler::retry_delay(&task);
        assert_eq!(d3.as_millis(), 4000);
    }

    #[test]
    fn retry_delay_capped() {
        let mut task = make_task("T", 5);
        task.retry_policy = RetryPolicy {
            max_retries: 10,
            base_delay_ms: 10_000,
            max_delay_ms: 30_000,
            backoff_multiplier: 2.0,
            idempotent: true,
        };
        task.retry_count = 5;

        let d = TaskScheduler::retry_delay(&task);
        assert_eq!(d.as_millis(), 30_000); // Capped.
    }

    #[test]
    fn has_work_checks_actionable() {
        let mut backlog = TaskBacklog::new();
        assert!(!TaskScheduler::has_work(&backlog));

        let task = make_task("T", 5);
        let id = task.task_id;
        backlog.add_task(task).unwrap();
        assert!(TaskScheduler::has_work(&backlog));

        backlog.transition(id, StructuredTaskStatus::Running).unwrap();
        assert!(!TaskScheduler::has_work(&backlog)); // Running is not actionable.

        backlog.complete_task(id, None, Vec::new()).unwrap();
        assert!(!TaskScheduler::has_work(&backlog));
    }

    #[test]
    fn preview_waves_topological_order() {
        let mut backlog = TaskBacklog::new();

        let t1 = make_task("T1", 5);
        let id1 = t1.task_id;
        backlog.add_task(t1).unwrap();

        let t2 = make_task("T2", 5);
        let id2 = t2.task_id;
        backlog.add_task(t2).unwrap();

        let t3 = make_task_with_deps("T3", 5, vec![id1, id2]);
        let id3 = t3.task_id;
        backlog.add_task(t3).unwrap();

        let t4 = make_task_with_deps("T4", 5, vec![id3]);
        let id4 = t4.task_id;
        backlog.add_task(t4).unwrap();

        let waves = TaskScheduler::preview_waves(&backlog);
        assert_eq!(waves.len(), 3);
        // Wave 0: T1 and T2 (no deps).
        assert!(waves[0].contains(&id1));
        assert!(waves[0].contains(&id2));
        // Wave 1: T3 (depends on T1, T2).
        assert_eq!(waves[1], vec![id3]);
        // Wave 2: T4 (depends on T3).
        assert_eq!(waves[2], vec![id4]);
    }

    #[test]
    fn scheduler_min_concurrent_is_one() {
        let scheduler = TaskScheduler::new(0);
        assert_eq!(scheduler.max_concurrent, 1);
    }
}
