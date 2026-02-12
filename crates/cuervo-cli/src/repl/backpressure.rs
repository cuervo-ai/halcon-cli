//! Backpressure: per-provider concurrency limiting via tokio semaphores.
//!
//! Prevents saturation by limiting how many concurrent provider invocations
//! can be in-flight for each provider. Uses `tokio::sync::Semaphore` with
//! configurable max permits and acquire timeout.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use cuervo_core::types::BackpressureConfig;

/// Per-provider concurrency limiter.
pub struct BackpressureGuard {
    semaphores: HashMap<String, Arc<Semaphore>>,
    config: BackpressureConfig,
}

/// RAII permit — released on drop.
pub struct InvokePermit {
    _permit: OwnedSemaphorePermit,
    provider: String,
}

impl fmt::Debug for InvokePermit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InvokePermit")
            .field("provider", &self.provider)
            .finish()
    }
}

impl InvokePermit {
    /// The provider this permit is for.
    #[allow(dead_code)] // Convenience API for diagnostics
    pub fn provider(&self) -> &str {
        &self.provider
    }
}

/// Error returned when backpressure rejects a request.
#[derive(Debug, Clone)]
pub struct BackpressureFull {
    pub provider: String,
    pub max_concurrent: u32,
}

impl fmt::Display for BackpressureFull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "backpressure full for '{}' (max {} concurrent)",
            self.provider, self.max_concurrent
        )
    }
}

impl BackpressureGuard {
    pub fn new(config: BackpressureConfig) -> Self {
        Self {
            semaphores: HashMap::new(),
            config,
        }
    }

    /// Register a provider (creates a semaphore with max_concurrent permits).
    ///
    /// No-op if already registered.
    pub fn register(&mut self, provider: &str) {
        self.semaphores
            .entry(provider.to_string())
            .or_insert_with(|| {
                Arc::new(Semaphore::new(
                    self.config.max_concurrent_per_provider as usize,
                ))
            });
    }

    /// Acquire a permit, waiting up to `queue_timeout_secs`.
    ///
    /// Returns `Err(BackpressureFull)` if the timeout expires.
    pub async fn acquire(&self, provider: &str) -> Result<InvokePermit, BackpressureFull> {
        let sem = self.get_or_default(provider);

        if self.config.queue_timeout_secs == 0 {
            return self.try_acquire_inner(provider, &sem);
        }

        let timeout = Duration::from_secs(self.config.queue_timeout_secs);
        match tokio::time::timeout(timeout, sem.clone().acquire_owned()).await {
            Ok(Ok(permit)) => Ok(InvokePermit {
                _permit: permit,
                provider: provider.to_string(),
            }),
            Ok(Err(_closed)) => Err(BackpressureFull {
                provider: provider.to_string(),
                max_concurrent: self.config.max_concurrent_per_provider,
            }),
            Err(_timeout) => Err(BackpressureFull {
                provider: provider.to_string(),
                max_concurrent: self.config.max_concurrent_per_provider,
            }),
        }
    }

    /// Try to acquire without waiting. Returns Err if no permits available.
    #[allow(dead_code)] // Sync alternative to acquire() for non-async contexts
    pub fn try_acquire(&self, provider: &str) -> Result<InvokePermit, BackpressureFull> {
        let sem = self.get_or_default(provider);
        self.try_acquire_inner(provider, &sem)
    }

    /// Current utilization for a provider: (in_use, max).
    pub fn utilization(&self, provider: &str) -> (u32, u32) {
        let max = self.config.max_concurrent_per_provider;
        let sem = match self.semaphores.get(provider) {
            Some(s) => s,
            None => return (0, max),
        };
        let available = sem.available_permits() as u32;
        (max.saturating_sub(available), max)
    }

    fn get_or_default(&self, provider: &str) -> Arc<Semaphore> {
        self.semaphores
            .get(provider)
            .cloned()
            .unwrap_or_else(|| {
                Arc::new(Semaphore::new(
                    self.config.max_concurrent_per_provider as usize,
                ))
            })
    }

    fn try_acquire_inner(
        &self,
        provider: &str,
        sem: &Arc<Semaphore>,
    ) -> Result<InvokePermit, BackpressureFull> {
        match sem.clone().try_acquire_owned() {
            Ok(permit) => Ok(InvokePermit {
                _permit: permit,
                provider: provider.to_string(),
            }),
            Err(_) => Err(BackpressureFull {
                provider: provider.to_string(),
                max_concurrent: self.config.max_concurrent_per_provider,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(max: u32) -> BackpressureConfig {
        BackpressureConfig {
            max_concurrent_per_provider: max,
            queue_timeout_secs: 1,
        }
    }

    #[test]
    fn register_creates_semaphore() {
        let mut guard = BackpressureGuard::new(test_config(3));
        guard.register("anthropic");
        assert_eq!(guard.utilization("anthropic"), (0, 3));
    }

    #[tokio::test]
    async fn acquire_within_limit_succeeds() {
        let mut guard = BackpressureGuard::new(test_config(3));
        guard.register("anthropic");

        let p1 = guard.acquire("anthropic").await.unwrap();
        assert_eq!(p1.provider(), "anthropic");

        let _p2 = guard.acquire("anthropic").await.unwrap();

        // Both permits held → 2 in use.
        assert_eq!(guard.utilization("anthropic"), (2, 3));

        drop(p1);
    }

    #[tokio::test]
    async fn try_acquire_at_limit_fails() {
        let mut guard = BackpressureGuard::new(test_config(2));
        guard.register("test");

        let _p1 = guard.try_acquire("test").unwrap();
        let _p2 = guard.try_acquire("test").unwrap();

        // Third should fail — at limit.
        let err = guard.try_acquire("test");
        assert!(err.is_err());
        let full = err.unwrap_err();
        assert_eq!(full.provider, "test");
        assert_eq!(full.max_concurrent, 2);
    }

    #[tokio::test]
    async fn utilization_tracks_permits() {
        let mut guard = BackpressureGuard::new(test_config(3));
        guard.register("prov");

        assert_eq!(guard.utilization("prov"), (0, 3));

        let _p1 = guard.acquire("prov").await.unwrap();
        assert_eq!(guard.utilization("prov"), (1, 3));

        let _p2 = guard.acquire("prov").await.unwrap();
        assert_eq!(guard.utilization("prov"), (2, 3));

        // Drop p2 releases the permit.
        drop(_p2);
        assert_eq!(guard.utilization("prov"), (1, 3));
    }

    #[tokio::test]
    async fn acquire_timeout_returns_full() {
        let mut guard = BackpressureGuard::new(BackpressureConfig {
            max_concurrent_per_provider: 1,
            queue_timeout_secs: 1,
        });
        guard.register("slow");

        let _p1 = guard.acquire("slow").await.unwrap();

        // Second acquire should timeout after 1 second.
        let start = std::time::Instant::now();
        let result = guard.acquire("slow").await;
        let elapsed = start.elapsed();

        assert!(result.is_err());
        assert!(elapsed >= Duration::from_millis(900), "should have waited ~1s");
    }

    #[test]
    fn backpressure_full_display() {
        let full = BackpressureFull {
            provider: "anthropic".into(),
            max_concurrent: 5,
        };
        let s = format!("{full}");
        assert!(s.contains("anthropic"));
        assert!(s.contains("5"));
    }

    #[tokio::test]
    async fn unregistered_provider_uses_default() {
        let guard = BackpressureGuard::new(test_config(2));

        // Not registered, but should still work with a temporary semaphore.
        let p = guard.acquire("unknown").await;
        assert!(p.is_ok());
    }
}
