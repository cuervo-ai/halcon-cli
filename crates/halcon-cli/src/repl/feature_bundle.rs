//! Optional feature subsystems bundle.
//!
//! Phase 3.1: Groups all optional capabilities (reflection, reasoning, MCP, plugins,
//! multimodal) into a single cohesive unit. 6 top-level fields from original Repl.

use std::sync::{Arc, Mutex};

use super::application::reasoning_engine::ReasoningEngine;
use super::bridges::mcp_manager::McpResourceManager;
use super::domain::reflexion::Reflector;
use super::planning::playbook::PlaybookPlanner;
use super::plugins::{PluginRegistry, PluginTransportRuntime};

/// Plugin system wrapper (registry + transport runtime).
///
/// Groups the two plugin-related fields into a single optional unit.
pub struct PluginSystem {
    /// V3 plugin registry for capability orchestration.
    pub registry: Arc<Mutex<PluginRegistry>>,
    /// Transport runtime for Stdio/HTTP/Local plugin communication.
    pub transport_runtime: Arc<PluginTransportRuntime>,
}

impl PluginSystem {
    /// Construct plugin system with both components.
    pub fn new(
        registry: Arc<Mutex<PluginRegistry>>,
        transport_runtime: Arc<PluginTransportRuntime>,
    ) -> Self {
        Self {
            registry,
            transport_runtime,
        }
    }
}

/// Optional feature subsystems for the REPL.
///
/// All advanced capabilities that may be disabled or lazy-initialized.
/// Most fields are Option<> to support feature flags and lazy loading.
pub struct ReplFeatures {
    /// Reflection subsystem for memory consolidation and learning.
    pub reflector: Option<Reflector>,

    /// FASE 3.1: Reasoning engine for metacognitive agent loop wrapping.
    /// None when reasoning.enabled = false (default).
    pub reasoning_engine: Option<ReasoningEngine>,

    /// FASE 3.2: MCP resource manager for lazy MCP server discovery.
    /// Always present (empty when no servers configured).
    pub mcp_manager: McpResourceManager,

    /// P1.1: Playbook-based planner loaded from ~/.halcon/playbooks/.
    /// Runs before LlmPlanner — instant (zero LLM calls) for matched workflows.
    pub playbook_planner: PlaybookPlanner,

    /// Multimodal subsystem (image/audio/video analysis). Activated with `--full`.
    pub multimodal: Option<Arc<halcon_multimodal::MultimodalSubsystem>>,

    /// V3 plugin system (registry + transport). None until plugins are configured.
    pub plugin_system: Option<PluginSystem>,

    /// Plugin registry for V3 plugin system. None until plugins are configured.
    /// Wrapped in Arc<Mutex<>> so it can be shared safely with the parallel executor.
    /// TODO(Phase 3.3): Migrate callers to use plugin_system instead.
    pub plugin_registry: Option<Arc<Mutex<PluginRegistry>>>,

    /// Transport runtime for V3 plugins (shared handle pool for Stdio/HTTP/Local plugins).
    /// None until plugins are lazy-initialized on first message with config.plugins.enabled.
    /// TODO(Phase 3.3): Migrate callers to use plugin_system instead.
    pub plugin_transport_runtime: Option<Arc<PluginTransportRuntime>>,

    /// Paloma FFR (formally-verified routing engine).
    /// When Some, round_setup consults Paloma for model/provider selection before ModelSelector.
    /// Initialized from the provider registry at REPL creation time.
    pub paloma_router: Option<halcon_providers::PalomaRouter>,
}

impl ReplFeatures {
    /// Construct feature bundle with all components.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        reflector: Option<Reflector>,
        reasoning_engine: Option<ReasoningEngine>,
        mcp_manager: McpResourceManager,
        playbook_planner: PlaybookPlanner,
        multimodal: Option<Arc<halcon_multimodal::MultimodalSubsystem>>,
        plugin_system: Option<PluginSystem>,
    ) -> Self {
        Self {
            reflector,
            reasoning_engine,
            mcp_manager,
            playbook_planner,
            multimodal,
            plugin_system,
            plugin_registry: None,
            plugin_transport_runtime: None,
            paloma_router: None,
        }
    }

    /// Check if reflection is enabled.
    pub fn has_reflection(&self) -> bool {
        self.reflector.is_some()
    }

    /// Check if reasoning engine is active.
    pub fn has_reasoning(&self) -> bool {
        self.reasoning_engine.is_some()
    }

    /// Check if plugins are loaded.
    pub fn has_plugins(&self) -> bool {
        self.plugin_system.is_some()
    }

    /// Check if multimodal is available.
    pub fn has_multimodal(&self) -> bool {
        self.multimodal.is_some()
    }
}

impl Default for ReplFeatures {
    fn default() -> Self {
        Self {
            reflector: None,
            reasoning_engine: None,
            mcp_manager: McpResourceManager::new(&Default::default()), // Empty McpConfig
            playbook_planner: PlaybookPlanner::empty(),                // No playbooks loaded
            multimodal: None,
            plugin_system: None,
            plugin_registry: None,
            plugin_transport_runtime: None,
            paloma_router: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_bundle_default_has_no_optional_features() {
        let features = ReplFeatures::default();

        assert!(!features.has_reflection());
        assert!(!features.has_reasoning());
        assert!(!features.has_plugins());
        assert!(!features.has_multimodal());
    }

    #[test]
    fn feature_bundle_tracks_enabled_features() {
        let mut features = ReplFeatures::default();

        // Initially none
        assert!(!features.has_reflection());

        // Enable reflection
        features.reflector = Some(Reflector::new(
            Arc::new(halcon_providers::EchoProvider::new()),
            "echo".to_string(),
        ));
        assert!(features.has_reflection());
    }
}
