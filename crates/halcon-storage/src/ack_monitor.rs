//! ACK timeout monitoring and automatic escalation.
//!
//! Monitors events stuck in 'sent' status and escalates:
//! - 5 min → warning log
//! - 15 min → move to DLQ

use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::{DeadLetterQueue, PersistentEventBuffer};

/// Configuration for ACK timeout monitor.
#[derive(Debug, Clone)]
pub struct AckMonitorConfig {
    /// Warning threshold (seconds).
    pub warning_threshold_secs: u64,
    /// DLQ escalation threshold (seconds).
    pub dlq_threshold_secs: u64,
    /// Check interval (seconds).
    pub check_interval_secs: u64,
}

impl Default for AckMonitorConfig {
    fn default() -> Self {
        Self {
            warning_threshold_secs: 300,  // 5 min
            dlq_threshold_secs: 900,      // 15 min
            check_interval_secs: 60,      // 1 min
        }
    }
}

/// ACK timeout monitor.
pub struct AckMonitor {
    buffer: Arc<Mutex<PersistentEventBuffer>>,
    dlq: Arc<Mutex<DeadLetterQueue>>,
    config: AckMonitorConfig,
}

impl AckMonitor {
    /// Create new ACK monitor.
    pub fn new(
        buffer: Arc<Mutex<PersistentEventBuffer>>,
        dlq: Arc<Mutex<DeadLetterQueue>>,
        config: AckMonitorConfig,
    ) -> Self {
        Self { buffer, dlq, config }
    }

    /// Start monitoring loop (spawns background task).
    pub fn start(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            self.run().await;
        })
    }

    /// Main monitoring loop.
    async fn run(&self) {
        let mut interval = tokio::time::interval(Duration::from_secs(
            self.config.check_interval_secs,
        ));

        loop {
            interval.tick().await;

            if let Err(e) = self.check_timeouts().await {
                warn!(error = %e, "ACK timeout check failed");
            }
        }
    }

    /// Check for timed-out events and escalate.
    async fn check_timeouts(&self) -> Result<()> {
        let now = current_timestamp();

        let buffer = self.buffer.lock().await;
        let stats = buffer.stats()?;

        if stats.sent == 0 {
            debug!("No events in 'sent' status, skipping timeout check");
            return Ok(());
        }

        // Get all sent events with their ages
        let sent_events = buffer.get_sent()?;
        drop(buffer); // Release lock early

        for event in sent_events {
            let age_secs = now.saturating_sub(event.sent_at.unwrap_or(event.created_at));

            if age_secs >= self.config.dlq_threshold_secs {
                // Escalate to DLQ
                warn!(
                    seq = event.seq,
                    age_secs,
                    "ACK timeout exceeded, moving to DLQ"
                );

                let mut dlq = self.dlq.lock().await;
                let _ = dlq.add_failure(
                    &format!("event-seq-{}", event.seq),
                    event.payload.clone(),
                    format!("ACK timeout after {}s", age_secs),
                    3, // max_retries
                );
                drop(dlq);

                // Mark as failed in buffer (delete from sent state)
                let mut buffer = self.buffer.lock().await;
                let _ = buffer.mark_failed(event.seq);
                drop(buffer);

                info!(
                    seq = event.seq,
                    age_secs,
                    "Event moved to DLQ due to ACK timeout"
                );
            } else if age_secs >= self.config.warning_threshold_secs {
                // Warning log
                warn!(
                    seq = event.seq,
                    age_secs,
                    "ACK timeout warning: event waiting for ACK"
                );
            }
        }

        Ok(())
    }
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DeadLetterQueue, PersistentEventBuffer};
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_ack_timeout_escalation() {
        let buffer_file = NamedTempFile::new().unwrap();
        let dlq_file = NamedTempFile::new().unwrap();

        let mut buffer = PersistentEventBuffer::open(buffer_file.path()).unwrap();
        let dlq = DeadLetterQueue::open(dlq_file.path()).unwrap();

        // Add event and mark as sent (old timestamp)
        buffer.push(1, r#"{"test":true}"#.to_string()).unwrap();
        buffer.mark_sent(1).unwrap();

        // Manually set sent_at to 20 minutes ago
        let conn = buffer.conn_mut();
        let old_timestamp = current_timestamp() - 1200; // 20 min ago
        conn.execute(
            "UPDATE event_buffer SET sent_at = ?1 WHERE seq = 1",
            rusqlite::params![old_timestamp],
        )
        .unwrap();

        let buffer = Arc::new(Mutex::new(buffer));
        let dlq = Arc::new(Mutex::new(dlq));

        let config = AckMonitorConfig {
            warning_threshold_secs: 300,
            dlq_threshold_secs: 900,
            check_interval_secs: 1,
        };

        let monitor = Arc::new(AckMonitor::new(buffer.clone(), dlq.clone(), config));

        // Run check once
        monitor.check_timeouts().await.unwrap();

        // Verify event moved to DLQ
        let dlq_guard = dlq.lock().await;
        let stats = dlq_guard.stats().unwrap();
        assert_eq!(stats.pending, 1);
    }
}
