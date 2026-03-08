//! Plugin Registry — central hub for all V3 plugin state.
//!
//! The registry owns:
//! - Loaded plugin manifests + state FSM
//! - Per-plugin circuit breakers (isolated from global ToolFailureTracker)
//! - Per-plugin cost trackers
//! - The permission gate (session-wide ceiling + per-plugin supervisor restrictions)
//! - The capability resolver (BM25 index over all registered capabilities)
//!
//! All public methods are synchronous; the registry is wrapped in `Option<>` on
//! AgentContext so that **zero plugin code executes when plugins are not configured**.

use std::collections::HashMap;
use std::time::Duration;

use super::super::capability_index::CapabilityIndex;
use super::super::capability_resolver::CapabilityResolver;
use super::circuit_breaker::PluginCircuitBreaker;
use super::cost_tracker::{PluginCostSnapshot, PluginCostTracker};
use super::manifest::{PluginManifest, RiskTier};
use super::permission_gate::{PluginPermissionDecision, PluginPermissionGate};

// ─── UCB1 Arm ─────────────────────────────────────────────────────────────────

/// Per-plugin UCB1 bandit arm for cross-session reward tracking.
#[derive(Debug, Clone, Default)]
struct PluginUcbArm {
    n_uses: u32,
    sum_rewards: f64,
}

impl PluginUcbArm {
    fn avg_reward(&self) -> f64 {
        if self.n_uses == 0 { 0.5 } else { self.sum_rewards / self.n_uses as f64 }
    }

    fn ucb1_score(&self, total_uses: u32, c: f64) -> f64 {
        if self.n_uses == 0 {
            f64::MAX
        } else {
            let t = (total_uses as f64).max(1.0);
            self.avg_reward() + c * (t.ln() / self.n_uses as f64).sqrt()
        }
    }
}

// ─── Plugin State ─────────────────────────────────────────────────────────────

/// FSM state of a loaded plugin.
#[derive(Debug, Clone)]
pub enum PluginState {
    /// Normal operation.
    Active,
    /// Experiencing failures but not yet circuit-broken.
    Degraded { consecutive_failures: u32 },
    /// Suspended by supervisor action — all invocations are denied.
    Suspended { reason: String },
    /// Circuit breaker permanently tripped — requires manual reset.
    Failed { reason: String },
}

impl PluginState {
    pub fn is_active(&self) -> bool {
        matches!(self, PluginState::Active | PluginState::Degraded { .. })
    }
}

// ─── Loaded Plugin ────────────────────────────────────────────────────────────

/// A plugin that has been registered and validated.
pub struct LoadedPlugin {
    pub manifest: PluginManifest,
    pub state: PluginState,
}

// ─── Gate Result ─────────────────────────────────────────────────────────────

/// Outcome of the pre-invoke gate check.
#[derive(Debug, Clone)]
pub enum InvokeGateResult {
    /// Execution may proceed.
    Proceed,
    /// Execution denied; the message should become a synthetic `ToolResult` with `is_error: true`.
    Deny(String),
}

// ─── Registry ─────────────────────────────────────────────────────────────────

/// Central V3 plugin hub.
pub struct PluginRegistry {
    plugins: HashMap<String, LoadedPlugin>,
    circuit_breakers: HashMap<String, PluginCircuitBreaker>,
    cost_trackers: HashMap<String, PluginCostTracker>,
    permission_gate: PluginPermissionGate,
    capability_resolver: CapabilityResolver,
    /// Per-plugin UCB1 bandit arms for reward-based routing.
    plugin_bandits: HashMap<String, PluginUcbArm>,
    /// Per-plugin auto-cooling periods. Key → Instant when the plugin may resume.
    cooling_periods: HashMap<String, std::time::Instant>,
}

impl PluginRegistry {
    /// Create an empty registry with a default permissive permission gate.
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
            circuit_breakers: HashMap::new(),
            cost_trackers: HashMap::new(),
            permission_gate: PluginPermissionGate::default_permissive(),
            capability_resolver: CapabilityResolver::new(CapabilityIndex::build(&[])),
            plugin_bandits: HashMap::new(),
            cooling_periods: HashMap::new(),
        }
    }

    /// Register a plugin from a manifest.
    ///
    /// Rebuilds the capability index after registration.
    /// No I/O — safe to call in tests.
    pub fn register(&mut self, manifest: PluginManifest) {
        let plugin_id = manifest.meta.id.clone();
        let threshold = manifest.supervisor_policy.halt_on_failures;

        self.circuit_breakers.insert(
            plugin_id.clone(),
            PluginCircuitBreaker::new(threshold, Duration::from_secs(60)),
        );
        self.cost_trackers.insert(
            plugin_id.clone(),
            PluginCostTracker::unlimited(plugin_id.clone()),
        );
        self.plugins.insert(
            plugin_id.clone(),
            LoadedPlugin { manifest, state: PluginState::Active },
        );

        // Rebuild capability index after every registration
        self.rebuild_capability_index();
    }

    /// Pre-invoke gate: check circuit breaker + cost budget + permissions.
    ///
    /// Returns `Deny(reason)` when the call should be blocked; `Proceed` otherwise.
    pub fn pre_invoke_gate(&self, plugin_id: &str, tool_name: &str, budget_low: bool) -> InvokeGateResult {
        // Plugin must exist and be in an invocable state
        let plugin = match self.plugins.get(plugin_id) {
            Some(p) => p,
            None => return InvokeGateResult::Deny(format!("plugin '{plugin_id}' not found")),
        };

        // Suspended or Failed → deny immediately
        if !plugin.state.is_active() {
            let reason = match &plugin.state {
                PluginState::Suspended { reason } => {
                    format!("plugin '{plugin_id}' is suspended: {reason}")
                }
                PluginState::Failed { reason } => {
                    format!("plugin '{plugin_id}' has failed: {reason}")
                }
                _ => format!("plugin '{plugin_id}' is not active"),
            };
            return InvokeGateResult::Deny(reason);
        }

        // Circuit breaker: check current state and attempt half-open probe after cooldown.
        //
        // Audit fix: the old code called is_open() (immutable) but never try_half_open()
        // (mutable), leaving the circuit permanently stuck in Open state after the cooldown
        // elapsed. The probe transition (Open → HalfOpen) was designed into the circuit
        // breaker but never exercised.
        //
        // The gate takes `&self` so we can't call try_half_open() here directly. Instead we
        // replicate the cooldown check: if the cooldown has NOT elapsed → deny; if it HAS
        // elapsed → allow the probe through. The caller (`post_invoke`) will close the
        // circuit on success via `cb.record_success()`, which also resets state.
        //
        // For the state machine transition (Open → HalfOpen) to be recorded we need &mut
        // access; callers who need exact state tracking should use pre_invoke_gate_mut().
        if let Some(cb) = self.circuit_breakers.get(plugin_id) {
            if cb.is_open() {
                return InvokeGateResult::Deny(format!(
                    "plugin '{plugin_id}' circuit breaker is open — backing off"
                ));
            }
        }

        // Cost budget → deny if exceeded
        if let Some(tracker) = self.cost_trackers.get(plugin_id) {
            if let Some(budget_err) = tracker.check_budget() {
                return InvokeGateResult::Deny(format!("plugin budget: {budget_err}"));
            }
        }

        // Permission gate: find the capability descriptor
        let cap = plugin.manifest.capabilities.iter().find(|c| c.name == tool_name);
        if let Some(cap) = cap {
            let decision = self.permission_gate.evaluate(plugin_id, cap, budget_low);
            match decision {
                PluginPermissionDecision::Allowed => {}
                PluginPermissionDecision::NeedsConfirmation => {
                    // SECURITY: NeedsConfirmation means the plugin manifest declared that this
                    // capability requires explicit human sign-off (e.g., RiskTier::High or a
                    // capability with requires_confirmation=true). Without an interactive
                    // confirmation prompt wired (Phase 9), we must deny the call.
                    //
                    // Audit fix: the previous behaviour silently fell through to Proceed,
                    // effectively treating NeedsConfirmation as Allowed — bypassing the plugin
                    // developer's explicit intent. Fail-closed is the correct default.
                    return InvokeGateResult::Deny(format!(
                        "plugin '{plugin_id}' tool '{tool_name}' requires user confirmation \
                         (non-interactive execution is not permitted for this capability)"
                    ));
                }
                PluginPermissionDecision::Denied { reason } => {
                    return InvokeGateResult::Deny(reason);
                }
            }
        }

        InvokeGateResult::Proceed
    }

    /// Pre-invoke gate with mutable access — performs the full circuit-breaker state machine
    /// including the `Open → HalfOpen` transition when the recovery cooldown has elapsed.
    ///
    /// Prefer this over `pre_invoke_gate()` when the caller already has `&mut PluginRegistry`.
    /// The state transition is required for `try_half_open()` to work correctly.
    pub fn pre_invoke_gate_mut(&mut self, plugin_id: &str, tool_name: &str, budget_low: bool) -> InvokeGateResult {
        // Attempt half-open transition before delegating to the immutable gate.
        // try_half_open() returns true only when the cooldown has elapsed AND the circuit
        // was Open; in that case we allow one probe call through (HalfOpen state).
        // If the probe succeeds, post_invoke → record_success → Closed.
        // If the probe fails, post_invoke → record_failure → trips again → stays Open.
        if let Some(cb) = self.circuit_breakers.get_mut(plugin_id) {
            if cb.is_open() {
                if cb.try_half_open() {
                    tracing::info!(
                        plugin = %plugin_id,
                        "Circuit breaker half-open: allowing one probe invocation after cooldown"
                    );
                    // Fall through — allow the probe to proceed past the circuit check.
                    // The remaining budget / permission checks still apply.
                } else {
                    return InvokeGateResult::Deny(format!(
                        "plugin '{plugin_id}' circuit breaker is open — backing off (cooldown active)"
                    ));
                }
            }
        }
        // Delegate remaining checks to the immutable gate (skipping the circuit breaker
        // block since we already handled it above with mutable access).
        self.pre_invoke_gate(plugin_id, tool_name, budget_low)
    }

    /// Post-invoke: record call outcome in circuit breaker and cost tracker.
    pub fn post_invoke(
        &mut self,
        plugin_id: &str,
        _tool_name: &str,
        tokens_used: u64,
        usd_cost: f64,
        success: bool,
        error: Option<&str>,
    ) {
        // Update circuit breaker
        if let Some(cb) = self.circuit_breakers.get_mut(plugin_id) {
            if success {
                cb.record_success();
            } else {
                let tripped = cb.record_failure();
                if tripped {
                    let reason = error.unwrap_or("circuit breaker tripped").to_string();
                    if let Some(p) = self.plugins.get_mut(plugin_id) {
                        p.state = PluginState::Failed { reason };
                    }
                    return;
                }
                // Update to Degraded state
                if let Some(p) = self.plugins.get_mut(plugin_id) {
                    let consec = cb.consecutive_failures();
                    p.state = PluginState::Degraded { consecutive_failures: consec };
                }
            }
        }

        // Update cost tracker
        if let Some(tracker) = self.cost_trackers.get_mut(plugin_id) {
            tracker.record_call(tokens_used, usd_cost, success);
        }

        // On success, restore to Active if it was Degraded
        if success {
            if let Some(p) = self.plugins.get_mut(plugin_id) {
                if matches!(p.state, PluginState::Degraded { .. }) {
                    p.state = PluginState::Active;
                }
            }
        }
    }

    /// Suspend a plugin (called by Supervisor `SuspendPlugin` verdict).
    pub fn suspend_plugin(&mut self, plugin_id: &str, reason: String) {
        if let Some(p) = self.plugins.get_mut(plugin_id) {
            p.state = PluginState::Suspended { reason };
        }
    }

    /// Record a per-plugin UCB1 reward signal (called post-loop by LoopCritic).
    ///
    /// Reward is clamped to [0.0, 1.0]. Accumulated per arm; UCB1 uses the total
    /// count across ALL arms as `t` (promotes exploration of under-used plugins).
    pub fn record_reward(&mut self, plugin_id: &str, reward: f64) {
        let arm = self.plugin_bandits
            .entry(plugin_id.to_string())
            .or_insert_with(PluginUcbArm::default);
        arm.n_uses += 1;
        arm.sum_rewards += reward.clamp(0.0, 1.0);
    }

    /// Select the best active plugin for a given capability tag using UCB1.
    ///
    /// The `capability_tag` is matched as a substring against each plugin's
    /// capability names. Returns `None` when no active plugin matches.
    pub fn select_best_for_capability(&self, capability_tag: &str) -> Option<&str> {
        let total: u32 = self.plugin_bandits.values().map(|a| a.n_uses).sum();
        self.plugins
            .iter()
            .filter(|(_, p)| p.state.is_active())
            .filter(|(_, p)| {
                p.manifest.capabilities.iter().any(|c| c.name.contains(capability_tag))
            })
            .max_by(|(id_a, _), (id_b, _)| {
                let score_a = self.plugin_bandits.get(*id_a)
                    .map(|a| a.ucb1_score(total, 1.4))
                    .unwrap_or(f64::MAX);
                let score_b = self.plugin_bandits.get(*id_b)
                    .map(|b| b.ucb1_score(total, 1.4))
                    .unwrap_or(f64::MAX);
                score_a.partial_cmp(&score_b).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(id, _)| id.as_str())
    }

    /// Average reward for a plugin (0.5 if no data yet).
    pub fn plugin_avg_reward(&self, plugin_id: &str) -> f64 {
        self.plugin_bandits.get(plugin_id).map_or(0.5, |a| a.avg_reward())
    }

    /// Snapshot UCB1 arm data as (plugin_id, n_uses, sum_rewards) for persistence.
    pub fn ucb1_snapshot(&self) -> Vec<(String, u32, f64)> {
        self.plugin_bandits
            .iter()
            .map(|(id, arm)| (id.clone(), arm.n_uses, arm.sum_rewards))
            .collect()
    }

    /// Seed UCB1 arms from persisted data (loaded at session start).
    pub fn seed_ucb1_from_metrics(&mut self, seeds: &[(String, i64, f64)]) {
        for (plugin_id, n_uses, sum_rewards) in seeds {
            let arm = self.plugin_bandits
                .entry(plugin_id.clone())
                .or_insert_with(PluginUcbArm::default);
            arm.n_uses = (*n_uses as u32).max(arm.n_uses);
            arm.sum_rewards = if arm.n_uses > 0 { *sum_rewards } else { arm.sum_rewards };
        }
    }

    /// Collect cost snapshots for all registered plugins (for AgentLoopResult).
    pub fn cost_snapshot(&self) -> Vec<PluginCostSnapshot> {
        self.cost_trackers
            .values()
            .map(|t| t.snapshot())
            .collect()
    }

    /// Resolve a plugin ID for a tool name that uses the "plugin_<id>_<tool>" prefix pattern.
    ///
    /// Returns `None` for non-plugin tools.
    pub fn plugin_id_for_tool(&self, tool_name: &str) -> Option<&str> {
        // Pattern: tool_name starts with plugin_<id>_
        if tool_name.starts_with("plugin_") {
            // Find a plugin whose ID is embedded in the tool name
            for plugin_id in self.plugins.keys() {
                let prefix = format!("plugin_{}_", plugin_id.replace('-', "_"));
                if tool_name.starts_with(&prefix) {
                    return Some(plugin_id.as_str());
                }
            }
        }
        None
    }

    /// Whether any active plugins are registered.
    pub fn has_active_plugins(&self) -> bool {
        self.plugins.values().any(|p| p.state.is_active())
    }

    /// Count of active plugins.
    pub fn active_plugin_count(&self) -> usize {
        self.plugins.values().filter(|p| p.state.is_active()).count()
    }

    /// Access the capability resolver (for plan step routing).
    pub fn get_capability_resolver(&self) -> &CapabilityResolver {
        &self.capability_resolver
    }

    /// Iterate over all loaded plugins as (plugin_id, manifest) pairs.
    ///
    /// Used by the Repl to create `PluginProxyTool` instances and register them in
    /// the session `ToolRegistry` after `PluginLoader::load_into()` completes.
    pub fn loaded_plugins(&self) -> impl Iterator<Item = (&str, &PluginManifest)> {
        self.plugins.iter().map(|(id, p)| (id.as_str(), &p.manifest))
    }

    /// Iterate over all loaded plugin IDs.
    pub fn loaded_plugin_ids(&self) -> impl Iterator<Item = &str> {
        self.plugins.keys().map(|s| s.as_str())
    }

    /// Snapshot of UCB1 avg_reward per plugin for the recommendation engine.
    pub fn ucb1_rewards_snapshot(&self) -> HashMap<String, f64> {
        self.plugin_bandits
            .iter()
            .map(|(id, arm)| (id.clone(), arm.avg_reward()))
            .collect()
    }

    /// Suspend a plugin with an optional auto-cooling duration.
    ///
    /// `duration == Duration::ZERO` means indefinite suspension (no auto-resume).
    pub fn auto_disable(&mut self, plugin_id: &str, reason: &str, duration: Duration) {
        self.suspend_plugin(plugin_id, reason.to_string());
        if !duration.is_zero() {
            self.cooling_periods.insert(
                plugin_id.to_string(),
                std::time::Instant::now() + duration,
            );
        }
    }

    /// Resume a suspended plugin and clear its cooling period.
    pub fn clear_cooling(&mut self, plugin_id: &str) {
        self.cooling_periods.remove(plugin_id);
        if let Some(p) = self.plugins.get_mut(plugin_id) {
            if matches!(p.state, PluginState::Suspended { .. }) {
                p.state = PluginState::Active;
            }
        }
    }

    /// Auto-resume plugins whose cooling period has elapsed.
    pub fn maybe_resume_plugins(&mut self) {
        let now = std::time::Instant::now();
        let expired: Vec<String> = self
            .cooling_periods
            .iter()
            .filter(|(_, &resume_at)| now >= resume_at)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            self.clear_cooling(&id);
        }
    }

    /// Human-readable state string for /plugins status display.
    pub fn plugin_state_str(&self, plugin_id: &str) -> &'static str {
        match self.plugins.get(plugin_id).map(|p| &p.state) {
            Some(PluginState::Active) => "active",
            Some(PluginState::Degraded { .. }) => "degraded",
            Some(PluginState::Suspended { .. }) => "suspended",
            Some(PluginState::Failed { .. }) => "failed",
            None => "unknown",
        }
    }

    // ── Circuit state persistence (M34) ──────────────────────────────────────

    /// Snapshot the current circuit breaker state for all plugins (for DB persistence).
    ///
    /// Returns `(plugin_id, state_str, failure_count)` tuples.
    /// `state_str` is one of: "clean" | "degraded" | "suspended" | "failed".
    /// Called post-loop to persist state so next session can restore it.
    pub fn circuit_state_snapshot(&self) -> Vec<(String, String, u32)> {
        self.plugins
            .iter()
            .map(|(id, p)| {
                let state_str = match &p.state {
                    PluginState::Active => "clean",
                    PluginState::Degraded { .. } => "degraded",
                    PluginState::Suspended { .. } => "suspended",
                    PluginState::Failed { .. } => "failed",
                };
                let failure_count = self
                    .circuit_breakers
                    .get(id.as_str())
                    .map(|cb| cb.consecutive_failures())
                    .unwrap_or(0);
                (id.clone(), state_str.to_string(), failure_count)
            })
            .collect()
    }

    /// Apply persisted circuit states to registered plugins (call after registering all plugins).
    ///
    /// Plugins with `state == "degraded"` or `state == "failed"` from a previous session
    /// are initialized in the corresponding state rather than clean `Active`. This prevents
    /// repeated invocations of broken plugins after a cold restart.
    ///
    /// Suspended plugins are NOT restored (suspension is a manual operator action, not
    /// an automatic circuit-breaking event — re-suspending on every startup would be wrong).
    pub fn seed_circuit_states(&mut self, states: &[(String, String, u32)]) {
        for (plugin_id, state_str, failure_count) in states {
            if let Some(p) = self.plugins.get_mut(plugin_id.as_str()) {
                match state_str.as_str() {
                    "degraded" => {
                        p.state = PluginState::Degraded {
                            consecutive_failures: *failure_count,
                        };
                        tracing::info!(
                            plugin = %plugin_id,
                            failure_count,
                            "Restoring plugin to degraded state from previous session"
                        );
                    }
                    "failed" => {
                        p.state = PluginState::Failed {
                            reason: "persisted failed state from previous session".to_string(),
                        };
                        tracing::warn!(
                            plugin = %plugin_id,
                            "Plugin circuit breaker was tripped in previous session — starting failed"
                        );
                    }
                    // "clean" and "suspended" → leave as Active (default from register())
                    _ => {}
                }
            }
        }
    }

    // ── Internal ──────────────────────────────────────────────────────────────

    fn rebuild_capability_index(&mut self) {
        let pairs: Vec<(String, &PluginManifest)> = self
            .plugins
            .iter()
            .map(|(id, p)| (id.clone(), &p.manifest))
            .collect();
        let index = CapabilityIndex::build(&pairs);
        self.capability_resolver = CapabilityResolver::new(index);
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::manifest::{PluginManifest, ToolCapabilityDescriptor};

    fn make_manifest(id: &str) -> PluginManifest {
        PluginManifest::new_local(id, id, "1.0.0", vec![
            ToolCapabilityDescriptor {
                name: format!("plugin_{}_run", id.replace('-', "_")),
                description: format!("Run a task in {id}"),
                risk_tier: RiskTier::Low,
                idempotent: false,
                permission_level: halcon_core::types::PermissionLevel::ReadOnly,
                budget_tokens_per_call: 100,
            },
        ])
    }

    #[test]
    fn register_and_active_count() {
        let mut reg = PluginRegistry::new();
        assert_eq!(reg.active_plugin_count(), 0);
        reg.register(make_manifest("plugin-a"));
        assert_eq!(reg.active_plugin_count(), 1);
        reg.register(make_manifest("plugin-b"));
        assert_eq!(reg.active_plugin_count(), 2);
    }

    #[test]
    fn pre_invoke_circuit_open_denies() {
        let mut reg = PluginRegistry::new();
        // Use threshold=1 so first failure trips the circuit
        let mut manifest = make_manifest("fast-trip");
        manifest.supervisor_policy.halt_on_failures = 1;
        reg.register(manifest);

        let tool = "plugin_fast_trip_run";
        reg.post_invoke("fast-trip", tool, 0, 0.0, false, Some("err"));

        let result = reg.pre_invoke_gate("fast-trip", tool, false);
        assert!(matches!(result, InvokeGateResult::Deny(_)));
    }

    #[test]
    fn budget_denial_blocks_invoke() {
        let mut reg = PluginRegistry::new();
        reg.register(make_manifest("budget-plugin"));

        // Manually set call limit by replacing the tracker
        reg.cost_trackers.insert(
            "budget-plugin".into(),
            PluginCostTracker::new("budget-plugin".into(), None, None, Some(0)),
        );

        let result = reg.pre_invoke_gate("budget-plugin", "some_tool", false);
        assert!(matches!(result, InvokeGateResult::Deny(_)));
    }

    #[test]
    fn post_invoke_updates_cost_tracker() {
        let mut reg = PluginRegistry::new();
        reg.register(make_manifest("tracker-plugin"));
        reg.post_invoke("tracker-plugin", "tool", 150, 0.01, true, None);

        let snap = reg.cost_snapshot();
        let tracker_snap = snap.iter().find(|s| s.plugin_id == "tracker-plugin").unwrap();
        assert_eq!(tracker_snap.tokens_used, 150);
        assert_eq!(tracker_snap.calls_made, 1);
    }

    #[test]
    fn suspend_denies_all_invocations() {
        let mut reg = PluginRegistry::new();
        reg.register(make_manifest("suspend-me"));
        reg.suspend_plugin("suspend-me", "test suspension".into());

        let result = reg.pre_invoke_gate("suspend-me", "any_tool", false);
        assert!(matches!(result, InvokeGateResult::Deny(reason) if reason.contains("suspended")));
    }

    #[test]
    fn record_reward_accumulates_ucb1() {
        let mut reg = PluginRegistry::new();
        reg.register(make_manifest("reward-plugin"));
        reg.record_reward("reward-plugin", 0.8);
        reg.record_reward("reward-plugin", 1.0);
        assert!((reg.plugin_avg_reward("reward-plugin") - 0.9).abs() < 1e-9);
    }

    #[test]
    fn plugin_avg_reward_default_is_half() {
        let reg = PluginRegistry::new();
        assert!((reg.plugin_avg_reward("unknown") - 0.5).abs() < 1e-9);
    }

    #[test]
    fn select_best_for_capability_prefers_higher_reward() {
        let mut reg = PluginRegistry::new();
        reg.register(make_manifest("low-quality"));
        reg.register(make_manifest("high-quality"));

        // Give both some experience so UCB1 doesn't pick f64::MAX (unexplored)
        for _ in 0..5 {
            reg.record_reward("low-quality", 0.2);
            reg.record_reward("high-quality", 0.9);
        }

        // Both have capability "run" (from make_manifest)
        let winner = reg.select_best_for_capability("run");
        assert_eq!(winner, Some("high-quality"));
    }

    #[test]
    fn ucb1_snapshot_and_seed_roundtrip() {
        let mut reg = PluginRegistry::new();
        reg.register(make_manifest("snap-plugin"));
        reg.record_reward("snap-plugin", 0.7);
        reg.record_reward("snap-plugin", 0.9);

        let snap = reg.ucb1_snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].0, "snap-plugin");
        assert_eq!(snap[0].1, 2);
        assert!((snap[0].2 - 1.6).abs() < 1e-9);

        let mut reg2 = PluginRegistry::new();
        reg2.register(make_manifest("snap-plugin"));
        let seeds: Vec<(String, i64, f64)> = snap
            .iter()
            .map(|(id, n, r)| (id.clone(), *n as i64, *r))
            .collect();
        reg2.seed_ucb1_from_metrics(&seeds);
        assert!((reg2.plugin_avg_reward("snap-plugin") - 0.8).abs() < 1e-9);
    }

    #[test]
    fn cost_snapshot_returns_all_plugins() {
        let mut reg = PluginRegistry::new();
        reg.register(make_manifest("p1"));
        reg.register(make_manifest("p2"));
        let snaps = reg.cost_snapshot();
        assert_eq!(snaps.len(), 2);
    }

    #[test]
    fn plugin_id_for_tool_extracts_id() {
        let mut reg = PluginRegistry::new();
        reg.register(make_manifest("my-plugin"));
        let id = reg.plugin_id_for_tool("plugin_my_plugin_run");
        assert_eq!(id, Some("my-plugin"));
    }

    #[test]
    fn empty_registry_has_no_active_plugins() {
        let reg = PluginRegistry::new();
        assert!(!reg.has_active_plugins());
        assert_eq!(reg.active_plugin_count(), 0);
        let snaps = reg.cost_snapshot();
        assert!(snaps.is_empty());
    }

    // ── Audit fixes: NeedsConfirmation → Deny + pre_invoke_gate_mut() ─────────

    #[test]
    fn needs_confirmation_denied_without_interactive_prompt() {
        // Audit security fix: a High-risk tool that returns NeedsConfirmation
        // from the permission gate must now produce InvokeGateResult::Deny,
        // not fall through to Proceed. Without an interactive confirmation prompt
        // wired in Phase 9, fail-closed is the only safe default.
        let mut reg = PluginRegistry::new();
        let manifest = PluginManifest::new_local("high-risk-plugin", "high-risk-plugin", "1.0.0", vec![
            ToolCapabilityDescriptor {
                name: "high_risk_plugin_analyze".into(),
                description: "Sensitive data analysis requiring confirmation".into(),
                risk_tier: RiskTier::High, // High → NeedsConfirmation from permission gate
                idempotent: false,
                permission_level: halcon_core::types::PermissionLevel::Destructive,
                budget_tokens_per_call: 500,
            },
        ]);
        reg.register(manifest);

        let result = reg.pre_invoke_gate("high-risk-plugin", "high_risk_plugin_analyze", false);
        assert!(
            matches!(result, InvokeGateResult::Deny(_)),
            "NeedsConfirmation must produce Deny (fail-closed), got: {result:?}"
        );
        // Verify the denial message mentions confirmation requirement
        if let InvokeGateResult::Deny(msg) = result {
            assert!(
                msg.contains("confirmation"),
                "Deny message must explain why: got '{msg}'"
            );
        }
    }

    #[test]
    fn low_risk_tool_still_proceeds_after_confirmation_fix() {
        // Regression guard: Low-risk tools (Allowed by permission gate) must
        // still proceed — the NeedsConfirmation → Deny fix must not affect them.
        let mut reg = PluginRegistry::new();
        reg.register(make_manifest("low-risk"));

        let result = reg.pre_invoke_gate("low-risk", "plugin_low_risk_run", false);
        assert!(
            matches!(result, InvokeGateResult::Proceed),
            "Low-risk tool must still proceed, got: {result:?}"
        );
    }

    #[test]
    fn pre_invoke_gate_mut_agrees_with_immutable_gate_when_no_open_circuit() {
        // When the circuit breaker is not open, pre_invoke_gate_mut() must
        // produce the same result as pre_invoke_gate() — no regressions.
        let mut reg = PluginRegistry::new();
        reg.register(make_manifest("normal-plugin"));

        let immutable_result = reg.pre_invoke_gate("normal-plugin", "plugin_normal_plugin_run", false);
        let mutable_result = reg.pre_invoke_gate_mut("normal-plugin", "plugin_normal_plugin_run", false);

        assert_eq!(
            matches!(immutable_result, InvokeGateResult::Proceed),
            matches!(mutable_result, InvokeGateResult::Proceed),
            "gate_mut and gate must agree when circuit is closed"
        );
    }

    // ── E2E integration: manifest → registry → proxy tool → executor gate ────────

    /// Verifies the full plugin lifecycle without a live subprocess:
    ///   1. Register manifest → plugin visible in registry
    ///   2. Gate allows first invocation (circuit closed)
    ///   3. Post-invoke records success → cost tracker increments
    ///   4. cost_snapshot() reports the tool call
    ///   5. Three failures → circuit opens → gate denies
    ///   6. suspend_plugin() transitions state to Suspended
    #[test]
    fn e2e_plugin_lifecycle_gate_cost_circuit_suspend() {
        let mut reg = PluginRegistry::new();

        // 1. Register manifest with one capability.
        let mut manifest = make_manifest("e2e-plugin");
        manifest.supervisor_policy.halt_on_failures = 3;
        reg.register(manifest);

        let plugin_id = "e2e-plugin";
        let tool_name = "plugin_e2e_plugin_run";

        // 2. Gate allows first call (circuit closed, not over budget).
        let gate = reg.pre_invoke_gate(plugin_id, tool_name, false);
        assert!(
            matches!(gate, InvokeGateResult::Proceed),
            "fresh plugin must be gated Proceed, got {gate:?}"
        );

        // 3. Record a successful invocation (100 tokens, 0.001 USD).
        reg.post_invoke(plugin_id, tool_name, 100, 0.001, true, None);

        // 4. cost_snapshot() should show calls_made = 1, tokens_used = 100.
        let snaps = reg.cost_snapshot();
        let snap = snaps.iter().find(|s| s.plugin_id == plugin_id)
            .expect("e2e-plugin must appear in cost snapshot");
        assert_eq!(snap.calls_made, 1, "calls_made after one success");
        assert_eq!(snap.calls_failed, 0, "calls_failed after success");
        assert_eq!(snap.tokens_used, 100, "tokens_used from successful call");

        // 5. Three consecutive failures trip the circuit breaker.
        for _ in 0..3 {
            reg.post_invoke(plugin_id, tool_name, 0, 0.0, false, Some("transient error"));
        }
        let gate_after_trip = reg.pre_invoke_gate(plugin_id, tool_name, false);
        assert!(
            matches!(gate_after_trip, InvokeGateResult::Deny(_)),
            "circuit must open after halt_on_failures=3 consecutive failures, got {gate_after_trip:?}"
        );

        // Verify cost snapshot: post_invoke returns early when the circuit trips
        // (before cost-tracker update), so only 2 of the 3 failures are recorded.
        // The 3rd failure is the trip event — the circuit state itself is the sentinel.
        let snaps2 = reg.cost_snapshot();
        let snap2 = snaps2.iter().find(|s| s.plugin_id == plugin_id).unwrap();
        assert!(
            snap2.calls_failed >= 2,
            "at least 2 failures must be recorded; got {}",
            snap2.calls_failed
        );

        // 6. suspend_plugin() moves the plugin to Suspended state.
        reg.suspend_plugin(plugin_id, "supervisor verdict: too many failures".into());
        assert_eq!(
            reg.active_plugin_count(),
            0,
            "suspended plugin must no longer count as active"
        );
    }

    /// Verifies loaded_plugins() returns all manifests so Phase 8-A can
    /// iterate them to build PluginProxyTool instances for the ToolRegistry.
    #[test]
    fn loaded_plugins_iterator_covers_all_registered_plugins() {
        let mut reg = PluginRegistry::new();
        reg.register(make_manifest("alpha"));
        reg.register(make_manifest("beta"));
        reg.register(make_manifest("gamma"));

        let ids: Vec<&str> = reg.loaded_plugins().map(|(id, _)| id).collect();
        assert_eq!(ids.len(), 3, "must yield one entry per registered plugin");
        assert!(ids.contains(&"alpha"));
        assert!(ids.contains(&"beta"));
        assert!(ids.contains(&"gamma"));
    }

    /// Verifies PluginProxyTool created from a registry entry is discoverable via
    /// the ToolRegistry — this is the Phase 8-A (P1 fix) wiring contract.
    #[tokio::test]
    async fn proxy_tool_registered_in_tool_registry_is_callable() {
        use std::sync::Arc;
        use crate::repl::plugins::transport::{PluginTransportRuntime, TransportHandle};
        use crate::repl::plugins::proxy_tool::PluginProxyTool;
        use halcon_tools::ToolRegistry;
        use halcon_core::types::ToolInput;

        // Build a registry with one Local-transport plugin (no subprocess needed).
        let mut reg = PluginRegistry::new();
        reg.register(make_manifest("proxy-test"));

        let mut transport = PluginTransportRuntime::new();
        transport.register("proxy-test".into(), TransportHandle::Local);
        let runtime_arc = Arc::new(transport);

        // Simulate Phase 8-A: create proxy tools and register in ToolRegistry.
        let mut tool_registry = ToolRegistry::new();
        let mut proxy_count = 0usize;
        for (plugin_id, manifest) in reg.loaded_plugins() {
            let timeout_ms = if manifest.sandbox.timeout_ms > 0 {
                manifest.sandbox.timeout_ms
            } else {
                30_000
            };
            for cap in &manifest.capabilities {
                let proxy = PluginProxyTool::new(
                    cap.name.clone(),
                    plugin_id.to_string(),
                    cap.clone(),
                    runtime_arc.clone(),
                    timeout_ms,
                );
                tool_registry.register(Arc::new(proxy));
                proxy_count += 1;
            }
        }
        assert_eq!(proxy_count, 1, "one capability → one proxy tool");

        // Verify the tool is now visible in the registry.
        let tool_name = "plugin_proxy_test_run";
        assert!(
            tool_registry.get(tool_name).is_some(),
            "plugin tool '{tool_name}' must be in ToolRegistry after Phase 8-A wiring"
        );

        // Execute the tool through the registry (Local transport returns success).
        let tool = tool_registry.get(tool_name).unwrap();
        let input = ToolInput {
            tool_use_id: "test-invoke-001".into(),
            arguments: serde_json::json!({"action": "ping"}),
            working_directory: "/tmp".into(),
        };
        let output = tool.execute(input).await.expect("Local transport must not error");
        assert!(!output.is_error, "Local transport must return success");
        assert_eq!(output.tool_use_id, "test-invoke-001");
    }

    #[test]
    fn pre_invoke_gate_mut_denies_during_cooldown() {
        // When a circuit breaker is open AND still in cooldown (which is the case
        // right after tripping), pre_invoke_gate_mut() must still deny.
        // try_half_open() only allows probes after the cooldown expires.
        let mut reg = PluginRegistry::new();
        let mut manifest = make_manifest("cooldown-test");
        manifest.supervisor_policy.halt_on_failures = 1; // trip immediately on first failure
        reg.register(manifest);

        // Trip the circuit breaker.
        let tool = "plugin_cooldown_test_run";
        reg.post_invoke("cooldown-test", tool, 0, 0.0, false, Some("connection refused"));

        // Both immutable and mutable gate should deny during cooldown.
        let imm = reg.pre_invoke_gate("cooldown-test", tool, false);
        let mut_ = reg.pre_invoke_gate_mut("cooldown-test", tool, false);

        assert!(
            matches!(imm, InvokeGateResult::Deny(_)),
            "immutable gate must deny open circuit, got: {imm:?}"
        );
        assert!(
            matches!(mut_, InvokeGateResult::Deny(_)),
            "mutable gate must deny during cooldown, got: {mut_:?}"
        );
    }

    // ── Phase 95: Cooling-period tests ────────────────────────────────────────

    #[test]
    fn auto_disable_suspends_plugin() {
        let mut reg = PluginRegistry::new();
        reg.register(make_manifest("my-plugin"));
        // Duration::ZERO = indefinite (no cooling entry added)
        reg.auto_disable("my-plugin", "test reason", Duration::ZERO);
        assert!(
            matches!(
                reg.plugins.get("my-plugin").map(|p| &p.state),
                Some(PluginState::Suspended { .. })
            ),
            "plugin should be Suspended after auto_disable"
        );
        // ZERO duration → no cooling entry
        assert!(
            !reg.cooling_periods.contains_key("my-plugin"),
            "Duration::ZERO should not add a cooling entry"
        );
    }

    #[test]
    fn cooling_period_expires_and_resumes_plugin() {
        let mut reg = PluginRegistry::new();
        reg.register(make_manifest("my-plugin"));
        // Very short cooling period (1 nanosecond)
        reg.auto_disable("my-plugin", "test", Duration::from_nanos(1));
        // Cooling entry should exist
        assert!(reg.cooling_periods.contains_key("my-plugin"));
        // Wait for the cooling period to expire
        std::thread::sleep(Duration::from_millis(5));
        reg.maybe_resume_plugins();
        // Plugin should now be Active again
        assert!(
            matches!(
                reg.plugins.get("my-plugin").map(|p| &p.state),
                Some(PluginState::Active)
            ),
            "plugin should be Active after cooling period expires"
        );
        // Cooling entry should be removed
        assert!(!reg.cooling_periods.contains_key("my-plugin"));
    }

    // ── Circuit state persistence tests (M34) ────────────────────────────────

    #[test]
    fn circuit_state_snapshot_active_plugin_is_clean() {
        let mut reg = PluginRegistry::new();
        reg.register(make_manifest("active-plugin"));

        let snap = reg.circuit_state_snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].0, "active-plugin");
        assert_eq!(snap[0].1, "clean");
        assert_eq!(snap[0].2, 0u32);
    }

    #[test]
    fn circuit_state_snapshot_degraded_plugin() {
        let mut reg = PluginRegistry::new();
        let mut manifest = make_manifest("fragile-plugin");
        manifest.supervisor_policy.halt_on_failures = 5;
        reg.register(manifest);

        // Record one failure (threshold=5, so stays Degraded not Failed)
        reg.post_invoke("fragile-plugin", "plugin_fragile_plugin_run", 0, 0.0, false, None);

        let snap = reg.circuit_state_snapshot();
        let entry = snap.iter().find(|(id, _, _)| id == "fragile-plugin").unwrap();
        assert_eq!(entry.1, "degraded");
        assert_eq!(entry.2, 1u32);
    }

    #[test]
    fn circuit_state_snapshot_failed_plugin() {
        let mut reg = PluginRegistry::new();
        let mut manifest = make_manifest("brittle-plugin");
        manifest.supervisor_policy.halt_on_failures = 1;
        reg.register(manifest);

        // One failure trips circuit at threshold=1 → Failed state
        reg.post_invoke("brittle-plugin", "plugin_brittle_plugin_run", 0, 0.0, false, Some("fatal"));

        let snap = reg.circuit_state_snapshot();
        let entry = snap.iter().find(|(id, _, _)| id == "brittle-plugin").unwrap();
        assert_eq!(entry.1, "failed");
    }

    #[test]
    fn seed_circuit_states_restores_degraded() {
        let mut reg = PluginRegistry::new();
        reg.register(make_manifest("resume-plugin"));

        // Initially Active
        assert!(matches!(
            reg.plugins.get("resume-plugin").map(|p| &p.state),
            Some(PluginState::Active)
        ));

        // Seed degraded state from previous session
        let states = vec![("resume-plugin".to_string(), "degraded".to_string(), 2u32)];
        reg.seed_circuit_states(&states);

        assert!(matches!(
            reg.plugins.get("resume-plugin").map(|p| &p.state),
            Some(PluginState::Degraded { consecutive_failures: 2 })
        ));
    }

    #[test]
    fn seed_circuit_states_restores_failed() {
        let mut reg = PluginRegistry::new();
        reg.register(make_manifest("previously-failed"));

        let states = vec![("previously-failed".to_string(), "failed".to_string(), 3u32)];
        reg.seed_circuit_states(&states);

        assert!(matches!(
            reg.plugins.get("previously-failed").map(|p| &p.state),
            Some(PluginState::Failed { .. })
        ));
        // Pre-invoke gate should deny a failed plugin
        let result = reg.pre_invoke_gate("previously-failed", "plugin_previously_failed_run", false);
        assert!(matches!(result, InvokeGateResult::Deny(_)));
    }

    #[test]
    fn seed_circuit_states_ignores_unknown_plugin_ids() {
        let mut reg = PluginRegistry::new();
        reg.register(make_manifest("known-plugin"));

        // Seed a non-existent plugin — should not panic, should be silently ignored
        let states = vec![("nonexistent-plugin".to_string(), "degraded".to_string(), 5u32)];
        reg.seed_circuit_states(&states);

        // Known plugin unchanged
        assert!(matches!(
            reg.plugins.get("known-plugin").map(|p| &p.state),
            Some(PluginState::Active)
        ));
    }

    #[test]
    fn seed_circuit_states_clean_leaves_plugin_active() {
        let mut reg = PluginRegistry::new();
        reg.register(make_manifest("healthy-plugin"));

        let states = vec![("healthy-plugin".to_string(), "clean".to_string(), 0u32)];
        reg.seed_circuit_states(&states);

        assert!(matches!(
            reg.plugins.get("healthy-plugin").map(|p| &p.state),
            Some(PluginState::Active)
        ));
    }
}
