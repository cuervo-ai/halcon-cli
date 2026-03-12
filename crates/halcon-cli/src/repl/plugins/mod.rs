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

pub mod auto_bootstrap;
pub mod circuit_breaker;
pub mod cost_tracker;
pub mod loader;
pub mod manifest;
pub mod permission_gate;
pub mod proxy_tool;
pub mod recommendation;
pub mod registry;
pub mod transport;

// Re-exports to maintain backward compatibility for callers outside plugins/
pub use auto_bootstrap::{AutoPluginBootstrap, BootstrapOptions, BootstrapResult};
pub use cost_tracker::PluginCostSnapshot;
pub use loader::PluginLoader;
pub use proxy_tool::PluginProxyTool;
pub use recommendation::{PluginRecommendationEngine, RecommendationTier};
pub use registry::{InvokeGateResult, PluginRegistry};
pub use transport::PluginTransportRuntime;

// C-1: capability files migrated from repl/ root
pub mod capability_index;
pub mod capability_orchestrator;
pub mod capability_resolver;
pub mod tool_manifest;

// MIGRATION-2026: tool files migrated from repl/ root (C-8f)
pub(crate) mod tool_aliases;
pub mod tool_selector;
pub mod tool_speculation;

// Re-exports for capability types
// capability_index, capability_orchestrator, capability_resolver: pub(crate) types — access via module path
