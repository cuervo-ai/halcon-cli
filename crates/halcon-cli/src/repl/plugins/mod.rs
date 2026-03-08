//! Plugin subsystem for Halcon.
//!
//! Provides the full lifecycle of external tool plugins:
//! - manifest: TOML-based plugin descriptor and metadata types
//! - registry: Central registry for tracking loaded plugins and their state
//! - loader: Discovers and loads plugins from disk into the registry
//! - transport: Runtime for executing plugin tools over stdio/HTTP transports
//! - proxy_tool: ToolDefinition wrapper that proxies calls to plugin transports
//! - permission_gate: Pre-invocation risk/permission check for plugin tools
//! - circuit_breaker: Per-plugin fault isolation with half-open recovery
//! - cost_tracker: Budget enforcement for metered plugin invocations
//! - recommendation: Heuristic engine for suggesting plugins based on project analysis
//! - auto_bootstrap: Automatic plugin installation from recommendation reports

pub mod registry;
pub mod loader;
pub mod manifest;
pub mod transport;
pub mod proxy_tool;
pub mod permission_gate;
pub mod circuit_breaker;
pub mod cost_tracker;
pub mod recommendation;
pub mod auto_bootstrap;

// Re-exports to maintain backward compatibility for callers outside plugins/
pub use registry::{PluginRegistry, PluginState, LoadedPlugin, InvokeGateResult};
pub use loader::{PluginLoader, PluginLoaderResult};
pub use manifest::{
    PluginManifest, PluginTransport, PluginCategory, ToolCapabilityDescriptor,
    SandboxContract, SupervisorPolicy, PluginPermissions, PluginMeta, RiskTier,
};
pub use transport::{PluginTransportRuntime, TransportHandle, PluginInvokeResult};
pub use proxy_tool::PluginProxyTool;
pub use permission_gate::{PluginPermissionGate, PluginPermissionDecision};
pub use circuit_breaker::{PluginCircuitBreaker, CircuitState};
pub use cost_tracker::{PluginCostTracker, PluginCostSnapshot, PluginBudgetError};
pub use recommendation::{PluginRecommendation, PluginRecommendationEngine, RecommendationTier};
pub use auto_bootstrap::{AutoPluginBootstrap, BootstrapOptions, BootstrapResult};

// C-1: capability files migrated from repl/ root
pub mod capability_index;
pub mod capability_orchestrator;
pub mod capability_resolver;
pub mod tool_manifest;

// Re-exports for capability types
pub use capability_index::CapabilityIndex;
pub use capability_orchestrator::{CapabilityOrchestrationLayer, OrchestrationDecision, SuppressReason};
pub use capability_resolver::{CapabilityResolver, CapabilitySource, ResolvedCapability};
pub use tool_manifest::{ToolManifest, ExternalTool, load_external_tools_default};
