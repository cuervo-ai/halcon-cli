//! Resilience event persistence: circuit breaker trips, health changes,
//! saturation events, and provider fallbacks.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A resilience event record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResilienceEvent {
    /// Provider name.
    pub provider: String,
    /// Event type: "breaker_trip", "health_change", "saturation", "fallback", "recovery".
    pub event_type: String,
    /// Previous state (for breaker transitions).
    pub from_state: Option<String>,
    /// New state (for breaker transitions).
    pub to_state: Option<String>,
    /// Health score (for health_change events).
    pub score: Option<u32>,
    /// Additional details (free-form text).
    pub details: Option<String>,
    /// When this event occurred.
    pub created_at: DateTime<Utc>,
}
