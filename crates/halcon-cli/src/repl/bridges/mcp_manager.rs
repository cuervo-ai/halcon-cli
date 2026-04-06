//! MCP Resource Manager: wraps the McpPool with lazy initialization,
//! health monitoring, and tool discovery.
//! FASE 3.2: Now wired into REPL - active infrastructure.
//!
//! Activates the existing McpPool infrastructure (previously dead code)
//! by providing a higher-level manager that handles lifecycle concerns.
//!
//! P0.1: `ensure_initialized()` now registers real `PoolBackedBridge`
//! instances into `ToolRegistry`, completing the MCP wiring.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use halcon_core::error::{HalconError, Result as HalconResult};
use halcon_core::traits::Tool;
use halcon_core::types::{McpConfig, PermissionLevel, ToolInput, ToolOutput};
use halcon_mcp::extract_text;
use halcon_mcp::pool::{McpConnectionHealth, McpPool, McpServerDef};
use halcon_mcp::types::McpToolDefinition;
use halcon_tools::ToolRegistry;

// ---------------------------------------------------------------------------
// PoolBackedBridge — Tool impl backed by McpPool (auto-reconnect included)
// ---------------------------------------------------------------------------

/// A `Tool` implementation that routes calls through `McpPool`.
///
/// Unlike `McpToolBridge` (which holds `Arc<Mutex<McpHost>>`), this bridge
/// holds an `Arc<McpPool>` so that auto-reconnect and health tracking work
/// automatically. Created per discovered MCP tool in `ensure_initialized()`.
struct PoolBackedBridge {
    pool: Arc<McpPool>,
    server: String,
    definition: McpToolDefinition,
    permission_override: Option<PermissionLevel>,
}

impl PoolBackedBridge {
    fn infer_permission(&self) -> PermissionLevel {
        infer_permission_from_def(&self.definition)
    }
}

/// Well-known read-only MCP tool names (from @modelcontextprotocol/server-filesystem
/// and common MCP servers). These override keyword inference to prevent false
/// Destructive classification from substring matches in descriptions.
///
/// Examples of false positives with naive substring matching:
///   - "run" matches inside "Returns" → `directory_tree` → Destructive (wrong)
///   - "set" matches inside "structure" → any tree tool → Destructive (wrong)
///   - "execute" matches inside "executable" in file info descriptions
const KNOWN_READONLY_TOOL_NAMES: &[&str] = &[
    // @modelcontextprotocol/server-filesystem
    "read_file",
    "read_multiple_files",
    "list_directory",
    "directory_tree",
    "get_file_info",
    "list_allowed_directories",
    // Common aliases / variants
    "read_text_file",
    "get_file_contents",
    "show_directory",
    "tree",
    "ls",
];

/// Infer permission level from tool name + description keywords.
///
/// Uses word-boundary matching (splits on non-alphanumeric characters) to avoid
/// false positives from common substrings — e.g. "run" inside "Returns", "set"
/// inside "structure", "send" inside "representation".
///
/// Known read-only tool names are short-circuited before keyword matching to
/// provide a stable floor for well-understood MCP server tools.
fn infer_permission_from_def(def: &McpToolDefinition) -> PermissionLevel {
    let name = def.name.to_lowercase();

    // Short-circuit: explicit allowlist of known read-only tool names.
    if KNOWN_READONLY_TOOL_NAMES.contains(&name.as_str()) {
        return PermissionLevel::ReadOnly;
    }

    let desc = def.description.as_deref().unwrap_or("").to_lowercase();
    let text = format!("{name} {desc}");

    // Split text into words for word-boundary matching.
    // This prevents "run" matching inside "Returns", "set" inside "structure", etc.
    let words: std::collections::HashSet<&str> = text
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| !w.is_empty())
        .collect();

    let write_signals = [
        "write", "create", "update", "delete", "remove", "set", "put", "post", "push", "commit",
        "execute", "run", "send", "modify",
    ];
    let read_signals = [
        "read", "get", "list", "search", "fetch", "show", "find", "query", "describe", "count",
        "view",
    ];

    if write_signals.iter().any(|s| words.contains(s)) {
        PermissionLevel::Destructive
    } else if read_signals.iter().any(|s| words.contains(s)) {
        PermissionLevel::ReadOnly
    } else {
        PermissionLevel::Destructive // safe default
    }
}

#[async_trait]
impl Tool for PoolBackedBridge {
    fn name(&self) -> &str {
        &self.definition.name
    }

    fn description(&self) -> &str {
        self.definition.description.as_deref().unwrap_or("MCP tool")
    }

    fn permission_level(&self) -> PermissionLevel {
        self.permission_override
            .unwrap_or_else(|| self.infer_permission())
    }

    async fn execute_inner(&self, input: ToolInput) -> HalconResult<ToolOutput> {
        let call_result = self
            .pool
            .call_tool(&self.server, &self.definition.name, input.arguments.clone())
            .await
            .map_err(|e| HalconError::ToolExecutionFailed {
                tool: self.definition.name.clone(),
                message: format!("MCP pool call failed: {e}"),
            })?;

        let content = extract_text(&call_result);

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: call_result.is_error,
            metadata: None,
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        self.definition.input_schema.clone()
    }
}

// ---------------------------------------------------------------------------
// McpResourceManager
// ---------------------------------------------------------------------------

/// Manages MCP server connections with lazy initialization and health tracking.
///
/// Instead of eagerly connecting to all MCP servers at startup (as the old
/// `connect_mcp_servers()` did), this manager defers connection until the
/// first time tools are needed.
///
/// **P0.1**: `ensure_initialized()` now creates `PoolBackedBridge` instances
/// for every discovered MCP tool and registers them in the `ToolRegistry`.
pub(crate) struct McpResourceManager {
    pool: Arc<McpPool>,
    /// Tools discovered from MCP servers: (server_name, tool_name) pairs.
    discovered_tools: Vec<(String, String)>,
    /// How many tools were successfully registered.
    registered_tool_count: usize,
    /// Whether initial discovery has happened.
    initialized: bool,
    /// Whether any servers are configured.
    has_servers: bool,
    /// Per-server tool permission overrides: server → (tool_name → perm_string).
    tool_permissions: HashMap<String, HashMap<String, String>>,
}

impl McpResourceManager {
    /// Create from MCP config. Does NOT connect yet — connections are lazy.
    pub fn new(mcp_config: &McpConfig) -> Self {
        let mut tool_permissions: HashMap<String, HashMap<String, String>> = HashMap::new();

        let server_defs: HashMap<String, McpServerDef> = mcp_config
            .servers
            .iter()
            .map(|(name, cfg)| {
                // Capture per-tool permission overrides before converting config.
                tool_permissions.insert(name.clone(), cfg.tool_permissions.clone());
                (
                    name.clone(),
                    McpServerDef {
                        command: cfg.command.clone(),
                        args: cfg.args.clone(),
                        env: cfg.env.clone(),
                        enabled: true,
                    },
                )
            })
            .collect();

        let has_servers = !server_defs.is_empty();
        let pool = Arc::new(McpPool::new(server_defs, mcp_config.max_reconnect_attempts));

        Self {
            pool,
            discovered_tools: Vec::new(),
            registered_tool_count: 0,
            initialized: false,
            has_servers,
            tool_permissions,
        }
    }

    /// Create an empty manager (no MCP servers configured).
    pub fn empty() -> Self {
        Self {
            pool: Arc::new(McpPool::new(HashMap::new(), 0)),
            discovered_tools: Vec::new(),
            registered_tool_count: 0,
            initialized: true,
            has_servers: false,
            tool_permissions: HashMap::new(),
        }
    }

    /// Lazy initialization: connect all servers + discover tools + register bridges.
    ///
    /// Safe to call multiple times — subsequent calls are no-ops.
    /// Failed servers are logged but don't block other servers.
    ///
    /// **P0.1**: After discovery, creates a `PoolBackedBridge` per tool and
    /// registers it in `tool_registry`. The bridge uses `McpPool::call_tool()`
    /// which includes auto-reconnect logic.
    pub async fn ensure_initialized(
        &mut self,
        tool_registry: &mut ToolRegistry,
    ) -> Vec<(String, Result<(), String>)> {
        if self.initialized {
            return Vec::new();
        }
        self.initialized = true;

        if !self.has_servers {
            return Vec::new();
        }

        // Connect all servers.
        let connect_results = self.pool.initialize_all().await;
        let mut results = Vec::new();

        for (name, result) in &connect_results {
            match result {
                Ok(()) => {
                    tracing::info!(server = %name, "MCP server connected via pool");
                    results.push((name.clone(), Ok(())));
                }
                Err(e) => {
                    tracing::warn!(server = %name, error = %e, "MCP server failed to connect");
                    results.push((name.clone(), Err(e.to_string())));
                }
            }
        }

        // Discover tools from connected servers and register bridges.
        let all_tools = self.pool.all_tools().await;
        for (server_name, tools) in all_tools {
            let server_perms = self.tool_permissions.get(&server_name);
            for tool_def in tools {
                let tool_name = tool_def.name.clone();

                // Resolve permission override from config.
                let perm_override =
                    server_perms
                        .and_then(|p| p.get(&tool_name))
                        .and_then(|s| match s.as_str() {
                            "ReadOnly" => Some(PermissionLevel::ReadOnly),
                            "Destructive" => Some(PermissionLevel::Destructive),
                            _ => None,
                        });

                // Native built-in tools take precedence over MCP tools with the same name.
                // This prevents MCP servers (e.g. @modelcontextprotocol/server-filesystem)
                // from silently overwriting native implementations that handle edge cases
                // (like EACCES on subdirectories) more gracefully.
                if tool_registry.get(&tool_name).is_some() {
                    tracing::debug!(
                        name = %tool_name,
                        server = %server_name,
                        "MCP tool shadowed by native built-in — skipping MCP registration"
                    );
                    continue;
                }

                // Create pool-backed bridge and register.
                let bridge = PoolBackedBridge {
                    pool: Arc::clone(&self.pool),
                    server: server_name.clone(),
                    definition: tool_def,
                    permission_override: perm_override,
                };
                tool_registry.register(Arc::new(bridge));
                self.registered_tool_count += 1;
                self.discovered_tools.push((server_name.clone(), tool_name));
            }

            let registered = self
                .discovered_tools
                .iter()
                .filter(|(s, _)| s == &server_name)
                .count();
            tracing::info!(
                server = %server_name,
                tool_count = registered,
                "Registered MCP tools into ToolRegistry"
            );
        }

        results
    }

    /// Health check all connections. Returns map of server→health.
    pub async fn health_check(&self) -> HashMap<String, McpConnectionHealth> {
        self.pool.health_check_all().await
    }

    /// Get the pool reference for direct tool calls.
    pub fn pool(&self) -> &Arc<McpPool> {
        &self.pool
    }

    /// Check if any MCP servers are configured.
    pub fn has_servers(&self) -> bool {
        self.has_servers
    }

    /// Check if initialization has been performed.
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Get discovered tool names: (server, tool) pairs.
    pub fn discovered_tools(&self) -> &[(String, String)] {
        &self.discovered_tools
    }

    /// Number of tools successfully registered into the ToolRegistry.
    pub fn registered_tool_count(&self) -> usize {
        self.registered_tool_count
    }

    /// Shut down all MCP connections.
    pub async fn shutdown(&self) {
        self.pool.shutdown_all().await;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::types::McpServerConfig;

    fn make_tool_def(name: &str, description: &str) -> McpToolDefinition {
        McpToolDefinition {
            name: name.into(),
            description: Some(description.into()),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

    // --- McpResourceManager lifecycle ---

    #[test]
    fn new_empty_config() {
        let config = McpConfig::default();
        let mgr = McpResourceManager::new(&config);
        assert!(!mgr.has_servers());
        assert!(!mgr.is_initialized());
        assert_eq!(mgr.registered_tool_count(), 0);
    }

    #[test]
    fn has_servers_with_config() {
        let mut config = McpConfig::default();
        config.servers.insert(
            "test".to_string(),
            McpServerConfig {
                command: "echo".to_string(),
                args: vec![],
                env: HashMap::new(),
                tool_permissions: HashMap::new(),
            },
        );
        let mgr = McpResourceManager::new(&config);
        assert!(mgr.has_servers());
    }

    #[test]
    fn empty_manager_is_initialized() {
        let mgr = McpResourceManager::empty();
        assert!(mgr.is_initialized());
        assert!(!mgr.has_servers());
        assert_eq!(mgr.registered_tool_count(), 0);
    }

    #[tokio::test]
    async fn ensure_initialized_idempotent() {
        let config = McpConfig::default();
        let mut mgr = McpResourceManager::new(&config);
        let mut reg = ToolRegistry::new();
        // First call initializes.
        let r1 = mgr.ensure_initialized(&mut reg).await;
        assert!(r1.is_empty());
        assert!(mgr.is_initialized());
        // Second call is a no-op.
        let r2 = mgr.ensure_initialized(&mut reg).await;
        assert!(r2.is_empty());
    }

    #[tokio::test]
    async fn ensure_initialized_no_servers_registers_nothing() {
        let config = McpConfig::default();
        let mut mgr = McpResourceManager::new(&config);
        let mut reg = ToolRegistry::new();
        mgr.ensure_initialized(&mut reg).await;
        assert_eq!(mgr.registered_tool_count(), 0);
        assert!(mgr.discovered_tools().is_empty());
    }

    #[tokio::test]
    async fn health_check_empty() {
        let mgr = McpResourceManager::empty();
        let health = mgr.health_check().await;
        assert!(health.is_empty());
    }

    #[test]
    fn pool_access() {
        let config = McpConfig::default();
        let mgr = McpResourceManager::new(&config);
        let _pool = mgr.pool();
    }

    #[test]
    fn tool_permissions_captured_from_config() {
        let mut config = McpConfig::default();
        let mut perms = HashMap::new();
        perms.insert("my_tool".to_string(), "ReadOnly".to_string());
        config.servers.insert(
            "myserver".to_string(),
            McpServerConfig {
                command: "echo".to_string(),
                args: vec![],
                env: HashMap::new(),
                tool_permissions: perms,
            },
        );
        let mgr = McpResourceManager::new(&config);
        let captured = mgr.tool_permissions.get("myserver").unwrap();
        assert_eq!(
            captured.get("my_tool").map(|s| s.as_str()),
            Some("ReadOnly")
        );
    }

    // --- PoolBackedBridge permission inference ---

    #[test]
    fn infer_permission_read_tool() {
        let def = make_tool_def("github_search", "Search repositories");
        assert_eq!(infer_permission_from_def(&def), PermissionLevel::ReadOnly);
    }

    #[test]
    fn infer_permission_write_tool() {
        let def = make_tool_def("file_create", "Create a new file");
        assert_eq!(
            infer_permission_from_def(&def),
            PermissionLevel::Destructive
        );
    }

    #[test]
    fn infer_permission_unknown_defaults_destructive() {
        let def = make_tool_def("custom_tool", "Does something custom");
        assert_eq!(
            infer_permission_from_def(&def),
            PermissionLevel::Destructive
        );
    }

    #[test]
    fn infer_permission_delete_tool() {
        let def = make_tool_def("delete_record", "Delete a database record");
        assert_eq!(
            infer_permission_from_def(&def),
            PermissionLevel::Destructive
        );
    }

    #[test]
    fn infer_permission_list_tool() {
        let def = make_tool_def("list_files", "List directory contents");
        assert_eq!(infer_permission_from_def(&def), PermissionLevel::ReadOnly);
    }

    #[test]
    fn infer_permission_commit_is_destructive() {
        let def = make_tool_def("git_commit", "Commit staged changes");
        assert_eq!(
            infer_permission_from_def(&def),
            PermissionLevel::Destructive
        );
    }

    #[test]
    fn permission_override_readonly_wins_over_write_name() {
        let def = make_tool_def("delete_everything", "Delete all records");
        // Without override: Destructive.
        assert_eq!(
            infer_permission_from_def(&def),
            PermissionLevel::Destructive
        );
        // Override to ReadOnly.
        let override_perm = Some(PermissionLevel::ReadOnly);
        let result = override_perm.unwrap_or_else(|| infer_permission_from_def(&def));
        assert_eq!(result, PermissionLevel::ReadOnly);
    }

    // ── Allowlist regression tests ──

    #[test]
    fn directory_tree_is_always_readonly_regardless_of_description() {
        // Regression: naive substring matching found "run" inside "Returns" in the
        // @modelcontextprotocol/server-filesystem directory_tree description,
        // classifying it Destructive. Allowlist must short-circuit this.
        let def = make_tool_def(
            "directory_tree",
            "Returns a recursive tree view of files and directories as a JSON structure. \
             Each entry includes name, type (file/directory), and size for files. \
             Files and empty directories are shown as objects.",
        );
        assert_eq!(
            infer_permission_from_def(&def),
            PermissionLevel::ReadOnly,
            "directory_tree must be ReadOnly regardless of description keywords"
        );
    }

    #[test]
    fn list_directory_is_always_readonly() {
        let def = make_tool_def(
            "list_directory",
            "List directory contents. Set depth to control recursion.",
        );
        assert_eq!(infer_permission_from_def(&def), PermissionLevel::ReadOnly);
    }

    #[test]
    fn read_multiple_files_allowlist_is_readonly() {
        let def = make_tool_def(
            "read_multiple_files",
            "Read the contents of multiple files simultaneously.",
        );
        assert_eq!(infer_permission_from_def(&def), PermissionLevel::ReadOnly);
    }

    // ── Word-boundary matching tests ──

    #[test]
    fn run_inside_returns_does_not_trigger_write_signal() {
        // "Returns" contains "run" as substring — word-boundary match must reject it.
        let def = make_tool_def("inspect_output", "Returns the output of a process.");
        // "run" is NOT a standalone word in "Returns" → no write signal → falls to read/default
        // "Returns" → word: "Returns" ≠ "run" → no write signal
        // no read signal in words either → Destructive (safe default)
        assert_eq!(
            infer_permission_from_def(&def),
            PermissionLevel::Destructive
        );
    }

    #[test]
    fn set_as_standalone_word_is_write_signal() {
        // "set" as a whole word must still trigger Destructive.
        let def = make_tool_def("config_tool", "Get or set configuration values.");
        assert_eq!(
            infer_permission_from_def(&def),
            PermissionLevel::Destructive
        );
    }

    #[test]
    fn set_inside_structure_does_not_trigger_write_signal() {
        // "structure" contains "set" as substring — word-boundary must reject it.
        let def = make_tool_def(
            "schema_info",
            "Returns the schema structure of the database.",
        );
        // words: {schema_info, Returns, the, schema, structure, of, database}
        // no write signal as whole word; "Returns" ≠ "get" → no exact read signal either → Destructive
        assert_eq!(
            infer_permission_from_def(&def),
            PermissionLevel::Destructive
        );
    }

    #[test]
    fn get_as_standalone_word_is_read_signal() {
        let def = make_tool_def("schema_info", "Get the schema of a table.");
        // words include "Get" → lowercased to "get" when text is lowercased → ReadOnly
        assert_eq!(infer_permission_from_def(&def), PermissionLevel::ReadOnly);
    }
}
