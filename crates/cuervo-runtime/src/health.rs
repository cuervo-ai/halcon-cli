//! Background health tracking for registered agents.

use std::collections::HashMap;
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::agent::AgentHealth;

/// Tracks health state for all registered agents.
#[derive(Debug)]
pub struct AgentHealthTracker {
    states: RwLock<HashMap<Uuid, HealthState>>,
}

#[derive(Debug, Clone)]
pub struct HealthState {
    pub current: AgentHealth,
    pub last_check: DateTime<Utc>,
    pub failure_count: u32,
    pub success_count: u32,
}

impl HealthState {
    fn new() -> Self {
        Self {
            current: AgentHealth::Healthy,
            last_check: Utc::now(),
            failure_count: 0,
            success_count: 0,
        }
    }
}

impl AgentHealthTracker {
    pub fn new() -> Self {
        Self {
            states: RwLock::new(HashMap::new()),
        }
    }

    /// Initialize tracking for an agent.
    pub fn track(&self, id: Uuid) {
        let mut states = self.states.write().unwrap();
        states.entry(id).or_insert_with(HealthState::new);
    }

    /// Remove tracking for an agent.
    pub fn untrack(&self, id: &Uuid) {
        let mut states = self.states.write().unwrap();
        states.remove(id);
    }

    /// Record a successful health check or invocation.
    pub fn record_success(&self, id: &Uuid) {
        let mut states = self.states.write().unwrap();
        if let Some(state) = states.get_mut(id) {
            state.success_count += 1;
            state.last_check = Utc::now();
            state.current = AgentHealth::Healthy;
        }
    }

    /// Record a failed health check or invocation.
    pub fn record_failure(&self, id: &Uuid, reason: &str) {
        let mut states = self.states.write().unwrap();
        if let Some(state) = states.get_mut(id) {
            state.failure_count += 1;
            state.last_check = Utc::now();
            // 3+ consecutive failures = Unavailable, 1-2 = Degraded
            if state.failure_count >= 3 {
                state.current = AgentHealth::Unavailable {
                    reason: reason.to_string(),
                };
            } else {
                state.current = AgentHealth::Degraded {
                    reason: reason.to_string(),
                };
            }
        }
    }

    /// Get the current health for a given agent.
    pub fn current(&self, id: &Uuid) -> AgentHealth {
        let states = self.states.read().unwrap();
        states
            .get(id)
            .map(|s| s.current.clone())
            .unwrap_or(AgentHealth::Unavailable {
                reason: "not tracked".to_string(),
            })
    }

    /// Get all tracked health states.
    pub fn all_states(&self) -> HashMap<Uuid, AgentHealth> {
        let states = self.states.read().unwrap();
        states.iter().map(|(k, v)| (*k, v.current.clone())).collect()
    }

    /// Get detailed state for an agent.
    pub fn state(&self, id: &Uuid) -> Option<HealthState> {
        let states = self.states.read().unwrap();
        states.get(id).cloned()
    }
}

impl Default for AgentHealthTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn track_and_current() {
        let tracker = AgentHealthTracker::new();
        let a = id(1);
        tracker.track(a);
        assert_eq!(tracker.current(&a), AgentHealth::Healthy);
    }

    #[test]
    fn untracked_agent_unavailable() {
        let tracker = AgentHealthTracker::new();
        let result = tracker.current(&id(999));
        assert!(matches!(result, AgentHealth::Unavailable { .. }));
    }

    #[test]
    fn record_success_stays_healthy() {
        let tracker = AgentHealthTracker::new();
        let a = id(1);
        tracker.track(a);
        tracker.record_success(&a);
        assert_eq!(tracker.current(&a), AgentHealth::Healthy);
        let state = tracker.state(&a).unwrap();
        assert_eq!(state.success_count, 1);
    }

    #[test]
    fn single_failure_degraded() {
        let tracker = AgentHealthTracker::new();
        let a = id(1);
        tracker.track(a);
        tracker.record_failure(&a, "timeout");
        assert!(matches!(tracker.current(&a), AgentHealth::Degraded { .. }));
    }

    #[test]
    fn three_failures_unavailable() {
        let tracker = AgentHealthTracker::new();
        let a = id(1);
        tracker.track(a);
        tracker.record_failure(&a, "error 1");
        tracker.record_failure(&a, "error 2");
        tracker.record_failure(&a, "error 3");
        assert!(matches!(
            tracker.current(&a),
            AgentHealth::Unavailable { .. }
        ));
    }

    #[test]
    fn success_resets_to_healthy() {
        let tracker = AgentHealthTracker::new();
        let a = id(1);
        tracker.track(a);
        tracker.record_failure(&a, "temp");
        assert!(matches!(tracker.current(&a), AgentHealth::Degraded { .. }));
        tracker.record_success(&a);
        assert_eq!(tracker.current(&a), AgentHealth::Healthy);
    }

    #[test]
    fn all_states() {
        let tracker = AgentHealthTracker::new();
        tracker.track(id(1));
        tracker.track(id(2));
        tracker.record_failure(&id(2), "slow");
        let states = tracker.all_states();
        assert_eq!(states.len(), 2);
        assert_eq!(states[&id(1)], AgentHealth::Healthy);
        assert!(matches!(states[&id(2)], AgentHealth::Degraded { .. }));
    }

    #[test]
    fn untrack_removes() {
        let tracker = AgentHealthTracker::new();
        let a = id(1);
        tracker.track(a);
        tracker.untrack(&a);
        assert!(tracker.state(&a).is_none());
        assert!(tracker.all_states().is_empty());
    }

    #[test]
    fn failure_count_tracks_correctly() {
        let tracker = AgentHealthTracker::new();
        let a = id(1);
        tracker.track(a);
        tracker.record_failure(&a, "f1");
        tracker.record_failure(&a, "f2");
        let state = tracker.state(&a).unwrap();
        assert_eq!(state.failure_count, 2);
        assert_eq!(state.success_count, 0);
    }
}
