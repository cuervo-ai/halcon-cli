//! AgentContext sub-struct decomposition (B1 migration step).
//!
//! The flat `AgentContext` (40 fields) is decomposed into three logical groups:
//! - `AgentInfrastructure`: provider chain, tool registry, response cache, resilience
//! - `AgentPolicyContext`: limits, routing, planning, orchestrator config, policy
//! - `AgentOptional`: compactor, planner, guardrails, reflector, optional subsystems
//!
//! The flat `AgentContext` struct in `mod.rs` remains the canonical type used at all
//! callsites. This module provides the sub-struct definitions and the `from_parts()`
//! constructor to enable gradual migration of construction sites (strangler fig).
//!
//! ## Borrow Checker Constraint
//!
//! `AgentContext` contains multiple `&'a mut` fields (`session`, `permissions`,
//! `resilience`, `task_bridge`, `context_manager`). Rust's exclusivity rules prevent
//! these from being split across sub-structs that are stored as values inside a
//! parent struct — doing so would require multiple mutable borrows of the same
//! lifetime simultaneously. For this reason, the sub-structs defined here do NOT
//! hold the mutable references; those remain directly on `AgentContext`. The sub-structs
//! group the immutable (shared-reference and owned) fields.

use std::sync::Arc;

use halcon_core::traits::{ModelProvider, Planner};
use halcon_core::types::{
    AgentLimits, OrchestratorConfig, Phase14Context, PlanningConfig, RoutingConfig,
};
use halcon_core::EventSender;
use halcon_providers::ProviderRegistry;
use halcon_storage::AsyncDatabase;
use halcon_tools::ToolRegistry;

use crate::render::sink::RenderSink;

use super::super::agent_types::StrategyContext;

/// Infrastructure fields: provider chain, registries, caches, event bus.
///
/// These are the "plumbing" fields that connect the agent loop to external
/// systems. All fields are immutable references or owned types.
pub struct AgentInfrastructure<'a> {
    pub provider: &'a Arc<dyn ModelProvider>,
    pub tool_registry: &'a ToolRegistry,
    pub trace_db: Option<&'a AsyncDatabase>,
    pub response_cache: Option<&'a super::super::response_cache::ResponseCache>,
    pub fallback_providers: &'a [(String, Arc<dyn ModelProvider>)],
    pub event_tx: &'a EventSender,
    pub render_sink: &'a dyn RenderSink,
    pub registry: Option<&'a ProviderRegistry>,
    pub speculator: &'a super::super::tool_speculation::ToolSpeculator,
}

/// Policy fields: limits, routing, planning, orchestrator config, security policy.
///
/// These govern HOW the agent loop behaves: max rounds, routing mode, planning
/// timeouts, and all PolicyConfig thresholds. All references are immutable.
pub struct AgentPolicyContext<'a> {
    pub limits: &'a AgentLimits,
    pub routing_config: &'a RoutingConfig,
    pub planning_config: &'a PlanningConfig,
    pub orchestrator_config: &'a OrchestratorConfig,
    pub policy: Arc<halcon_core::types::PolicyConfig>,
    pub security_config: &'a halcon_core::types::SecurityConfig,
    pub phase14: Phase14Context,
    pub tool_selection_enabled: bool,
    pub is_sub_agent: bool,
    pub requested_provider: Option<String>,
    pub episode_id: Option<uuid::Uuid>,
}

/// Optional subsystem fields: compactor, planner, reflector, model selector, critic, plugins.
///
/// These are feature-gated subsystems that may or may not be present in a given session.
/// All are `Option<_>` or equivalent.
pub struct AgentOptional<'a> {
    pub compactor: Option<&'a super::super::compaction::ContextCompactor>,
    pub planner: Option<&'a dyn Planner>,
    pub guardrails: &'a [Box<dyn halcon_security::Guardrail>],
    pub reflector: Option<&'a super::super::reflexion::Reflector>,
    pub replay_tool_executor: Option<&'a super::super::replay_executor::ReplayToolExecutor>,
    pub model_selector: Option<&'a super::super::model_selector::ModelSelector>,
    pub critic_provider: Option<Arc<dyn ModelProvider>>,
    pub critic_model: Option<String>,
    pub plugin_registry: Option<
        std::sync::Arc<
            std::sync::Mutex<super::super::plugin_registry::PluginRegistry>,
        >,
    >,
    pub strategy_context: Option<StrategyContext>,
}
