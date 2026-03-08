//! Agent task manager for non-blocking concurrent agent execution.
//!
//! Phase 4: Allows multiple agent loops to run concurrently without blocking
//! the TUI message loop. Provides concurrency limits, task tracking, and cleanup.

use std::collections::HashMap;
use tokio::task::JoinHandle;
use uuid::Uuid;

/// Manages concurrent agent loop tasks.
///
/// Prevents TUI blocking by spawning each agent loop in a separate tokio task.
/// Tracks active tasks, enforces concurrency limits, and provides cleanup.
pub struct AgentTaskManager {
    /// Active agent tasks (task_id -> JoinHandle).
    active_tasks: HashMap<Uuid, JoinHandle<()>>,
    /// Maximum number of concurrent agent tasks.
    max_concurrent: usize,
    /// Total tasks spawned (for metrics).
    total_spawned: usize,
    /// Total tasks completed (for metrics).
    total_completed: usize,
}

impl AgentTaskManager {
    /// Create a new task manager with concurrency limit.
    ///
    /// # Arguments
    /// * `max_concurrent` - Maximum number of concurrent agent tasks (default: 3)
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            active_tasks: HashMap::new(),
            max_concurrent: max_concurrent.max(1), // At least 1
            total_spawned: 0,
            total_completed: 0,
        }
    }

    /// Spawn a new agent task if under concurrency limit.
    ///
    /// Returns `Ok(task_id)` if spawned, `Err(message)` if limit reached.
    pub fn spawn_task<F>(&mut self, future: F) -> Result<Uuid, String>
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        // Check concurrency limit.
        if self.active_tasks.len() >= self.max_concurrent {
            return Err(format!(
                "Maximum concurrent agents reached ({}/{})",
                self.active_tasks.len(),
                self.max_concurrent
            ));
        }

        let task_id = Uuid::new_v4();
        let handle = tokio::spawn(future);
        self.active_tasks.insert(task_id, handle);
        self.total_spawned += 1;

        tracing::debug!(
            task_id = %task_id,
            active = self.active_tasks.len(),
            max = self.max_concurrent,
            "Spawned agent task"
        );

        Ok(task_id)
    }

    /// Check if a task has completed and remove it from tracking.
    ///
    /// Returns `true` if the task was completed and removed.
    pub fn poll_task(&mut self, task_id: &Uuid) -> bool {
        if let Some(handle) = self.active_tasks.get(task_id) {
            if handle.is_finished() {
                self.active_tasks.remove(task_id);
                self.total_completed += 1;
                tracing::debug!(
                    task_id = %task_id,
                    active = self.active_tasks.len(),
                    completed = self.total_completed,
                    "Agent task completed"
                );
                return true;
            }
        }
        false
    }

    /// Poll all active tasks and remove completed ones.
    ///
    /// Returns the number of tasks that were completed.
    pub fn poll_all(&mut self) -> usize {
        let task_ids: Vec<Uuid> = self.active_tasks.keys().copied().collect();
        let mut completed = 0;

        for task_id in task_ids {
            if self.poll_task(&task_id) {
                completed += 1;
            }
        }

        completed
    }

    /// Abort a specific task.
    ///
    /// Returns `true` if the task was found and aborted.
    pub fn abort_task(&mut self, task_id: &Uuid) -> bool {
        if let Some(handle) = self.active_tasks.remove(task_id) {
            handle.abort();
            tracing::debug!(task_id = %task_id, "Agent task aborted");
            true
        } else {
            false
        }
    }

    /// Abort all active tasks.
    ///
    /// Returns the number of tasks that were aborted.
    pub fn abort_all(&mut self) -> usize {
        let count = self.active_tasks.len();
        for (task_id, handle) in self.active_tasks.drain() {
            handle.abort();
            tracing::debug!(task_id = %task_id, "Agent task aborted (cleanup)");
        }
        count
    }

    /// Get the number of active tasks.
    pub fn active_count(&self) -> usize {
        self.active_tasks.len()
    }

    /// Check if under concurrency limit.
    pub fn can_spawn(&self) -> bool {
        self.active_tasks.len() < self.max_concurrent
    }

    /// Get metrics snapshot.
    pub fn metrics(&self) -> AgentTaskMetrics {
        AgentTaskMetrics {
            active: self.active_tasks.len(),
            total_spawned: self.total_spawned,
            total_completed: self.total_completed,
            max_concurrent: self.max_concurrent,
        }
    }
}

impl Default for AgentTaskManager {
    fn default() -> Self {
        Self::new(3) // Default: 3 concurrent agents
    }
}

impl Drop for AgentTaskManager {
    fn drop(&mut self) {
        let aborted = self.abort_all();
        if aborted > 0 {
            tracing::warn!(aborted, "AgentTaskManager dropped with active tasks, all aborted");
        }
    }
}

/// Metrics snapshot for agent task manager.
#[derive(Debug, Clone)]
pub struct AgentTaskMetrics {
    pub active: usize,
    pub total_spawned: usize,
    pub total_completed: usize,
    pub max_concurrent: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn new_manager_empty() {
        let manager = AgentTaskManager::new(3);
        assert_eq!(manager.active_count(), 0);
        assert!(manager.can_spawn());
    }

    #[tokio::test]
    async fn spawn_task_under_limit() {
        let mut manager = AgentTaskManager::new(3);
        let result = manager.spawn_task(async {
            tokio::time::sleep(Duration::from_millis(10)).await;
        });
        assert!(result.is_ok());
        assert_eq!(manager.active_count(), 1);
    }

    #[tokio::test]
    async fn spawn_task_at_limit_fails() {
        let mut manager = AgentTaskManager::new(2);

        // Spawn 2 tasks (max limit)
        manager.spawn_task(async {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }).unwrap();
        manager.spawn_task(async {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }).unwrap();

        // Third spawn should fail
        let result = manager.spawn_task(async {});
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Maximum concurrent"));
    }

    #[tokio::test]
    async fn poll_task_detects_completion() {
        let mut manager = AgentTaskManager::new(3);
        let task_id = manager.spawn_task(async {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }).unwrap();

        // Initially not finished
        assert_eq!(manager.active_count(), 1);

        // Wait for completion
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Poll should detect completion
        let completed = manager.poll_task(&task_id);
        assert!(completed);
        assert_eq!(manager.active_count(), 0);
    }

    #[tokio::test]
    async fn poll_all_removes_completed() {
        let mut manager = AgentTaskManager::new(5);

        // Spawn 3 tasks: 2 short, 1 long
        manager.spawn_task(async {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }).unwrap();
        manager.spawn_task(async {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }).unwrap();
        manager.spawn_task(async {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }).unwrap();

        assert_eq!(manager.active_count(), 3);

        // Wait for short tasks to complete
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Poll all should remove 2 completed tasks
        let completed = manager.poll_all();
        assert_eq!(completed, 2);
        assert_eq!(manager.active_count(), 1);
    }

    #[tokio::test]
    async fn abort_task_stops_execution() {
        let mut manager = AgentTaskManager::new(3);
        let task_id = manager.spawn_task(async {
            tokio::time::sleep(Duration::from_secs(10)).await;
        }).unwrap();

        assert_eq!(manager.active_count(), 1);

        // Abort task
        let aborted = manager.abort_task(&task_id);
        assert!(aborted);
        assert_eq!(manager.active_count(), 0);
    }

    #[tokio::test]
    async fn abort_all_clears_tasks() {
        let mut manager = AgentTaskManager::new(5);

        // Spawn 3 long-running tasks
        for _ in 0..3 {
            manager.spawn_task(async {
                tokio::time::sleep(Duration::from_secs(10)).await;
            }).unwrap();
        }

        assert_eq!(manager.active_count(), 3);

        // Abort all
        let aborted = manager.abort_all();
        assert_eq!(aborted, 3);
        assert_eq!(manager.active_count(), 0);
    }

    #[tokio::test]
    async fn can_spawn_respects_limit() {
        let mut manager = AgentTaskManager::new(2);

        assert!(manager.can_spawn());

        manager.spawn_task(async {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }).unwrap();
        assert!(manager.can_spawn());

        manager.spawn_task(async {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }).unwrap();
        assert!(!manager.can_spawn()); // At limit
    }

    #[tokio::test]
    async fn metrics_track_lifecycle() {
        let mut manager = AgentTaskManager::new(3);

        // Spawn 2 tasks
        let id1 = manager.spawn_task(async {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }).unwrap();
        manager.spawn_task(async {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }).unwrap();

        let metrics = manager.metrics();
        assert_eq!(metrics.active, 2);
        assert_eq!(metrics.total_spawned, 2);
        assert_eq!(metrics.total_completed, 0);

        // Wait for first task to complete
        tokio::time::sleep(Duration::from_millis(20)).await;
        manager.poll_task(&id1);

        let metrics = manager.metrics();
        assert_eq!(metrics.active, 1);
        assert_eq!(metrics.total_spawned, 2);
        assert_eq!(metrics.total_completed, 1);
    }

    #[tokio::test]
    async fn default_max_concurrent_is_3() {
        let manager = AgentTaskManager::default();
        assert_eq!(manager.max_concurrent, 3);
    }

    #[tokio::test]
    async fn min_concurrent_is_1() {
        let manager = AgentTaskManager::new(0); // Try to set 0
        assert_eq!(manager.max_concurrent, 1); // Clamped to 1
    }

    #[tokio::test]
    async fn drop_aborts_active_tasks() {
        let mut manager = AgentTaskManager::new(3);

        // Spawn long-running task
        manager.spawn_task(async {
            tokio::time::sleep(Duration::from_secs(10)).await;
        }).unwrap();

        assert_eq!(manager.active_count(), 1);

        // Drop manager (should abort all tasks)
        drop(manager);

        // Task should be aborted (can't verify directly, but no panic)
    }
}
