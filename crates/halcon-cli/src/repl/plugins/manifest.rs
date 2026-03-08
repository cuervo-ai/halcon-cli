//! Plugin manifest — declarative metadata for HALCON V3 plugins.
//!
//! Each plugin ships a manifest describing its capabilities, permission requirements,
//! sandbox contract and supervisor policy. The registry validates and stores these at
//! registration time; no I/O occurs during execution.

use serde::{Deserialize, Serialize};

// ─── Risk Tier ────────────────────────────────────────────────────────────────

/// Ordered risk classification for plugin capabilities.
///
/// Used by [`PluginPermissionGate`] to gate invocations against the session's
/// configured `global_max_risk` ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskTier {
    Low,
    Medium,
    High,
    Critical,
}

impl Default for RiskTier {
    fn default() -> Self {
        RiskTier::Medium
    }
}

impl std::fmt::Display for RiskTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RiskTier::Low => write!(f, "low"),
            RiskTier::Medium => write!(f, "medium"),
            RiskTier::High => write!(f, "high"),
            RiskTier::Critical => write!(f, "critical"),
        }
    }
}

// ─── Transport ────────────────────────────────────────────────────────────────

/// Wire protocol used to invoke the plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum PluginTransport {
    /// Subprocess launched with `command` + `args`; communicates via stdio JSON-RPC.
    Stdio { command: String, args: Vec<String> },
    /// Remote HTTP service; agent POSTs tool calls to `base_url/invoke`.
    Http { base_url: String },
    /// In-process Rust plugin (loaded via FFI — Phase 8 feature, not yet enforced).
    InProcess,
    /// Local tool bridge (wraps an existing ToolRegistry entry — test/demo only).
    Local,
}

impl Default for PluginTransport {
    fn default() -> Self {
        PluginTransport::Local
    }
}

// ─── Category ─────────────────────────────────────────────────────────────────

/// Broad classification for plugin discovery and routing hints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginCategory {
    Development,
    DataProcessing,
    Communication,
    Integration,
    Security,
    Custom(String),
}

impl Default for PluginCategory {
    fn default() -> Self {
        PluginCategory::Custom("general".into())
    }
}

// ─── Capability Descriptor ────────────────────────────────────────────────────

/// Metadata for one tool exposed by a plugin.
///
/// The capability index uses `name` + `description` for BM25 search.
/// The permission gate uses `risk_tier` + `permission_level` for access control.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCapabilityDescriptor {
    /// Tool identifier (must match what the plugin routes on internally).
    pub name: String,
    /// Human-readable description used for BM25 capability search.
    pub description: String,
    /// Risk classification for this specific tool.
    #[serde(default)]
    pub risk_tier: RiskTier,
    /// Whether repeated calls with the same arguments produce identical results.
    #[serde(default)]
    pub idempotent: bool,
    /// HALCON permission level (maps to existing PermissionChecker flow).
    #[serde(default = "default_permission_level")]
    pub permission_level: halcon_core::types::PermissionLevel,
    /// Expected token budget consumed by a single call (0 = unknown).
    #[serde(default)]
    pub budget_tokens_per_call: u32,
}

fn default_permission_level() -> halcon_core::types::PermissionLevel {
    halcon_core::types::PermissionLevel::ReadOnly
}

// ─── Sandbox Contract ─────────────────────────────────────────────────────────

/// Declared resource boundaries for the plugin.
///
/// Enforcement is deferred to Phase 8 (WASM sandbox); at Phase 7 this is metadata
/// only — used by the permission gate and audit log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxContract {
    /// Allowed outbound network domains (empty = no network access declared).
    #[serde(default)]
    pub network_allowed: Vec<String>,
    /// Paths the plugin may read from the filesystem.
    #[serde(default)]
    pub fs_read: Vec<String>,
    /// Whether the plugin may spawn subprocesses.
    #[serde(default)]
    pub subprocess_allowed: bool,
    /// Maximum resident memory in megabytes (0 = no limit declared).
    #[serde(default)]
    pub max_memory_mb: u32,
    /// Per-invocation timeout in milliseconds (0 = use global default).
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_timeout_ms() -> u64 {
    30_000
}

impl Default for SandboxContract {
    fn default() -> Self {
        Self {
            network_allowed: vec![],
            fs_read: vec![],
            subprocess_allowed: false,
            max_memory_mb: 0,
            timeout_ms: default_timeout_ms(),
        }
    }
}

// ─── Supervisor Policy ────────────────────────────────────────────────────────

/// Per-plugin supervisor behaviour configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorPolicy {
    /// Number of consecutive failures that trigger circuit-breaker trip.
    #[serde(default = "default_halt_on_failures")]
    pub halt_on_failures: u32,
    /// Weight of this plugin's success rate in the final reward blend (0.0–1.0).
    #[serde(default = "default_reward_weight")]
    pub reward_weight: f64,
    /// When true, every invocation must be explicitly approved by the user.
    #[serde(default)]
    pub requires_explicit_approval: bool,
}

fn default_halt_on_failures() -> u32 {
    3
}
fn default_reward_weight() -> f64 {
    1.0
}

impl Default for SupervisorPolicy {
    fn default() -> Self {
        Self {
            halt_on_failures: default_halt_on_failures(),
            reward_weight: default_reward_weight(),
            requires_explicit_approval: false,
        }
    }
}

// ─── Permissions ─────────────────────────────────────────────────────────────

/// What the plugin declares it needs from the HALCON environment.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginPermissions {
    /// Whether the plugin may read environment variables.
    #[serde(default)]
    pub env_read: bool,
    /// Whether the plugin may write to the halcon database.
    #[serde(default)]
    pub db_write: bool,
}

// ─── Plugin Meta ─────────────────────────────────────────────────────────────

/// Core identity of a plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMeta {
    /// Stable identifier (kebab-case, e.g. "git-enhanced").
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// SemVer string.
    pub version: String,
    /// How to invoke this plugin.
    #[serde(default)]
    pub transport: PluginTransport,
    /// Broad category for discovery.
    #[serde(default)]
    pub category: PluginCategory,
    /// Optional SHA-256 checksum for integrity verification (Phase 5 Hub integration).
    #[serde(default)]
    pub checksum: Option<String>,
}

// ─── Root Manifest ────────────────────────────────────────────────────────────

/// Complete plugin specification — serializable to/from TOML or JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub meta: PluginMeta,
    /// All tools this plugin exposes.
    pub capabilities: Vec<ToolCapabilityDescriptor>,
    #[serde(default)]
    pub permissions: PluginPermissions,
    #[serde(default)]
    pub sandbox: SandboxContract,
    #[serde(default)]
    pub supervisor_policy: SupervisorPolicy,
}

impl PluginManifest {
    /// Convenience constructor for tests / local registration.
    pub fn new_local(id: &str, name: &str, version: &str, capabilities: Vec<ToolCapabilityDescriptor>) -> Self {
        Self {
            meta: PluginMeta {
                id: id.to_string(),
                name: name.to_string(),
                version: version.to_string(),
                transport: PluginTransport::Local,
                category: PluginCategory::default(),
                checksum: None,
            },
            capabilities,
            permissions: PluginPermissions::default(),
            sandbox: SandboxContract::default(),
            supervisor_policy: SupervisorPolicy::default(),
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest() -> PluginManifest {
        PluginManifest::new_local(
            "test-plugin",
            "Test Plugin",
            "1.0.0",
            vec![ToolCapabilityDescriptor {
                name: "test_tool".into(),
                description: "A test tool for unit testing".into(),
                risk_tier: RiskTier::Low,
                idempotent: true,
                permission_level: halcon_core::types::PermissionLevel::ReadOnly,
                budget_tokens_per_call: 100,
            }],
        )
    }

    #[test]
    fn manifest_serde_roundtrip() {
        let manifest = sample_manifest();
        let json = serde_json::to_string(&manifest).expect("serialize");
        let back: PluginManifest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.meta.id, "test-plugin");
        assert_eq!(back.capabilities.len(), 1);
        assert_eq!(back.capabilities[0].name, "test_tool");
    }

    #[test]
    fn risk_tier_ordering_low_lt_critical() {
        assert!(RiskTier::Low < RiskTier::Medium);
        assert!(RiskTier::Medium < RiskTier::High);
        assert!(RiskTier::High < RiskTier::Critical);
    }

    #[test]
    fn sandbox_default_disallows_network_and_subprocess() {
        let s = SandboxContract::default();
        assert!(s.network_allowed.is_empty());
        assert!(!s.subprocess_allowed);
    }

    #[test]
    fn supervisor_policy_defaults() {
        let p = SupervisorPolicy::default();
        assert_eq!(p.halt_on_failures, 3);
        assert!((p.reward_weight - 1.0).abs() < 1e-9);
        assert!(!p.requires_explicit_approval);
    }

    #[test]
    fn manifest_with_http_transport() {
        let mut manifest = sample_manifest();
        manifest.meta.transport = PluginTransport::Http {
            base_url: "https://example.com".into(),
        };
        let json = serde_json::to_string(&manifest).expect("serialize");
        let back: PluginManifest = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(back.meta.transport, PluginTransport::Http { .. }));
    }

    #[test]
    fn risk_tier_display() {
        assert_eq!(RiskTier::Low.to_string(), "low");
        assert_eq!(RiskTier::Critical.to_string(), "critical");
    }
}
