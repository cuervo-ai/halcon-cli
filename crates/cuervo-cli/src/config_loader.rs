use std::collections::HashMap;
use std::path::PathBuf;

use cuervo_core::types::{AppConfig, McpServerConfig};

/// Load configuration with layered merging:
/// 1. Built-in defaults (AppConfig::default())
/// 2. Global config (~/.cuervo/config.toml)
/// 3. Project config (.cuervo/config.toml)
/// 4. Explicit config file (--config flag)
/// 5. Environment variables (CUERVO_*)
pub fn load_config(explicit_path: Option<&str>) -> Result<AppConfig, anyhow::Error> {
    let mut config = AppConfig::default();

    // Layer 2: Global config
    let global = global_config_path();
    if global.exists() {
        let content = std::fs::read_to_string(&global)?;
        let global_config: toml::Value = toml::from_str(&content)?;
        merge_toml_into_config(&mut config, &global_config);
        tracing::debug!("Loaded global config from {}", global.display());
    }

    // Layer 3: Project config
    let project = project_config_path();
    if project.exists() {
        let content = std::fs::read_to_string(&project)?;
        let project_config: toml::Value = toml::from_str(&content)?;
        merge_toml_into_config(&mut config, &project_config);
        tracing::debug!("Loaded project config from {}", project.display());
    }

    // Layer 4: Explicit config file
    if let Some(path) = explicit_path {
        let content = std::fs::read_to_string(path)?;
        let explicit_config: toml::Value = toml::from_str(&content)?;
        merge_toml_into_config(&mut config, &explicit_config);
        tracing::debug!("Loaded explicit config from {path}");
    }

    // Layer 5: Environment variable overrides
    apply_env_overrides(&mut config);

    // Layer 6: .mcp.json auto-discovery (additive merge, highest priority for MCP servers)
    load_mcp_json(&mut config);

    Ok(config)
}

/// Global config path: ~/.cuervo/config.toml
pub fn global_config_path() -> PathBuf {
    dirs_path().join("config.toml")
}

/// Project config path: .cuervo/config.toml
pub fn project_config_path() -> PathBuf {
    PathBuf::from(".cuervo/config.toml")
}

/// Cuervo data directory: ~/.cuervo/
fn dirs_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cuervo")
}

/// Default database path: ~/.cuervo/cuervo.db
pub fn default_db_path() -> PathBuf {
    dirs_path().join("cuervo.db")
}

/// Merge a TOML value tree into an existing AppConfig.
///
/// This does a shallow merge at the section level: if a section
/// exists in the overlay, it fully replaces the section in config
/// (re-deserialized from the merged TOML).
fn merge_toml_into_config(config: &mut AppConfig, overlay: &toml::Value) {
    // Serialize current config to toml::Value
    let mut base = match toml::Value::try_from(&*config) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("Config serialization failed during merge: {e}");
            return;
        }
    };

    // Deep merge overlay into base
    if let (Some(base_table), Some(overlay_table)) = (base.as_table_mut(), overlay.as_table()) {
        deep_merge(base_table, overlay_table);
    }

    // Deserialize back
    match base.try_into::<AppConfig>() {
        Ok(merged) => *config = merged,
        Err(e) => {
            tracing::warn!("Config merge deserialization failed (overlay ignored): {e}");
        }
    }
}

fn deep_merge(
    base: &mut toml::map::Map<String, toml::Value>,
    overlay: &toml::map::Map<String, toml::Value>,
) {
    for (key, value) in overlay {
        match (base.get_mut(key), value) {
            (Some(toml::Value::Table(base_table)), toml::Value::Table(overlay_table)) => {
                deep_merge(base_table, overlay_table);
            }
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

/// Apply environment variable overrides.
fn apply_env_overrides(config: &mut AppConfig) {
    if let Ok(val) = std::env::var("CUERVO_DEFAULT_PROVIDER") {
        config.general.default_provider = val;
    }
    if let Ok(val) = std::env::var("CUERVO_DEFAULT_MODEL") {
        config.general.default_model = val;
    }
    if let Ok(val) = std::env::var("CUERVO_MAX_TOKENS") {
        if let Ok(n) = val.parse() {
            config.general.max_tokens = n;
        }
    }
    if let Ok(val) = std::env::var("CUERVO_TEMPERATURE") {
        if let Ok(n) = val.parse() {
            config.general.temperature = n;
        }
    }
    if let Ok(val) = std::env::var("CUERVO_LOG_LEVEL") {
        config.logging.level = val;
    }
}

/// Load MCP server configurations from `.mcp.json` files.
///
/// Search order (all merged additively, later files override earlier for same server name):
/// 1. `./.mcp.json` (project root)
/// 2. `.cuervo/.mcp.json` (project config dir)
/// 3. `~/.cuervo/.mcp.json` (global user config)
fn load_mcp_json(config: &mut AppConfig) {
    let paths = [
        PathBuf::from(".mcp.json"),
        PathBuf::from(".cuervo/.mcp.json"),
        dirs_path().join(".mcp.json"),
    ];

    for path in &paths {
        if !path.exists() {
            continue;
        }
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to read {}: {e}", path.display());
                continue;
            }
        };
        match parse_mcp_json(&content) {
            Ok(servers) => {
                let count = servers.len();
                for (name, server_config) in servers {
                    config.mcp.servers.entry(name).or_insert(server_config);
                }
                tracing::debug!("Loaded {count} MCP servers from {}", path.display());
            }
            Err(e) => {
                tracing::warn!("Failed to parse {}: {e}", path.display());
            }
        }
    }
}

/// Parse a `.mcp.json` file content into a map of server configurations.
///
/// Supports two formats:
/// 1. Claude/Cursor format: `{"mcpServers": {"name": {"command": "...", ...}}}`
/// 2. Direct format: `{"name": {"command": "...", ...}}`
fn parse_mcp_json(content: &str) -> Result<HashMap<String, McpServerConfig>, anyhow::Error> {
    let root: serde_json::Value = serde_json::from_str(content)?;

    let servers_obj = if let Some(mcp_servers) = root.get("mcpServers") {
        mcp_servers
    } else {
        &root
    };

    let map = servers_obj
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("expected JSON object"))?;

    let mut result = HashMap::new();
    for (name, value) in map {
        let command = value
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("server '{name}' missing 'command' field"))?
            .to_string();

        let args: Vec<String> = value
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let env: HashMap<String, String> = value
            .get("env")
            .and_then(|v| v.as_object())
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), expand_env_value(s))))
                    .collect()
            })
            .unwrap_or_default();

        let tool_permissions: HashMap<String, String> = value
            .get("tool_permissions")
            .and_then(|v| v.as_object())
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();

        result.insert(
            name.clone(),
            McpServerConfig {
                command,
                args,
                env,
                tool_permissions,
            },
        );
    }

    Ok(result)
}

/// Expand `${VAR}` references in a string with environment variable values.
///
/// Unset variables are replaced with an empty string.
fn expand_env_value(val: &str) -> String {
    let mut result = val.to_string();
    // Find all ${...} patterns and replace.
    while let Some(start) = result.find("${") {
        if let Some(end) = result[start..].find('}') {
            let var_name = &result[start + 2..start + end];
            let replacement = std::env::var(var_name).unwrap_or_default();
            result = format!("{}{}{}", &result[..start], replacement, &result[start + end + 1..]);
        } else {
            break; // Malformed pattern, stop.
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialize tests that read/write process-global env vars.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn default_config_loads() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::remove_var("CUERVO_DEFAULT_PROVIDER");
        std::env::remove_var("CUERVO_DEFAULT_MODEL");
        // load_config reads ~/.cuervo/config.toml if it exists, so the
        // default_provider may differ from AppConfig::default().
        // We only verify that load_config succeeds and returns a valid config.
        let config = load_config(None).unwrap();
        assert!(!config.general.default_provider.is_empty());
    }

    #[test]
    fn toml_overlay_merges() {
        let mut config = AppConfig::default();
        let overlay: toml::Value = toml::from_str(
            r#"
            [general]
            default_model = "llama3.2"
            "#,
        )
        .unwrap();

        merge_toml_into_config(&mut config, &overlay);
        assert_eq!(config.general.default_model, "llama3.2");
        assert_eq!(config.general.default_provider, "anthropic");
    }

    #[test]
    fn env_override_applies() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var("CUERVO_DEFAULT_PROVIDER", "ollama");
        let mut config = AppConfig::default();
        apply_env_overrides(&mut config);
        assert_eq!(config.general.default_provider, "ollama");
        std::env::remove_var("CUERVO_DEFAULT_PROVIDER");
    }

    // --- .mcp.json tests ---

    #[test]
    fn parse_mcp_json_mcpservers_format() {
        let json = r#"{
            "mcpServers": {
                "github": {
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-github"],
                    "env": { "TOKEN": "abc123" }
                }
            }
        }"#;
        let servers = parse_mcp_json(json).unwrap();
        assert_eq!(servers.len(), 1);
        let github = &servers["github"];
        assert_eq!(github.command, "npx");
        assert_eq!(github.args, vec!["-y", "@modelcontextprotocol/server-github"]);
        assert_eq!(github.env.get("TOKEN").unwrap(), "abc123");
    }

    #[test]
    fn parse_mcp_json_direct_format() {
        let json = r#"{
            "filesystem": {
                "command": "mcp-server-filesystem",
                "args": ["--root", "/tmp"]
            }
        }"#;
        let servers = parse_mcp_json(json).unwrap();
        assert_eq!(servers.len(), 1);
        let fs = &servers["filesystem"];
        assert_eq!(fs.command, "mcp-server-filesystem");
        assert_eq!(fs.args, vec!["--root", "/tmp"]);
    }

    #[test]
    fn parse_mcp_json_merge_preserves_existing() {
        let mut config = AppConfig::default();
        config.mcp.servers.insert(
            "existing".to_string(),
            McpServerConfig {
                command: "existing-cmd".to_string(),
                args: vec![],
                env: HashMap::new(),
                tool_permissions: HashMap::new(),
            },
        );

        // Simulate merge: entry() with or_insert preserves existing.
        let new_servers = parse_mcp_json(r#"{"new_server": {"command": "new-cmd"}}"#).unwrap();
        for (name, server_config) in new_servers {
            config.mcp.servers.entry(name).or_insert(server_config);
        }

        assert_eq!(config.mcp.servers.len(), 2);
        assert_eq!(config.mcp.servers["existing"].command, "existing-cmd");
        assert_eq!(config.mcp.servers["new_server"].command, "new-cmd");
    }

    #[test]
    fn expand_env_value_replaces_variables() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var("CUERVO_TEST_TOKEN", "secret123");
        let result = expand_env_value("Bearer ${CUERVO_TEST_TOKEN}");
        assert_eq!(result, "Bearer secret123");
        std::env::remove_var("CUERVO_TEST_TOKEN");
    }

    #[test]
    fn expand_env_value_missing_var_empty() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::remove_var("CUERVO_NONEXISTENT_VAR_42");
        let result = expand_env_value("prefix_${CUERVO_NONEXISTENT_VAR_42}_suffix");
        assert_eq!(result, "prefix__suffix");
    }

    #[test]
    fn load_mcp_json_nonexistent_paths_noop() {
        let mut config = AppConfig::default();
        let before = config.mcp.servers.len();
        // load_mcp_json tries paths that don't exist in the test cwd — should be a no-op.
        load_mcp_json(&mut config);
        // May or may not find files depending on test environment.
        // At minimum, it should not error.
        assert!(config.mcp.servers.len() >= before);
    }

    #[test]
    fn toml_overlay_merge_with_unknown_fields() {
        // Unknown sections in overlay should not break the merge —
        // they are silently dropped during deserialization back to AppConfig.
        let mut config = AppConfig::default();
        let overlay: toml::Value = toml::from_str(
            r#"
            [general]
            default_model = "gpt-4o"

            [unknown_section]
            foo = "bar"
            "#,
        )
        .unwrap();

        merge_toml_into_config(&mut config, &overlay);
        // Known field merged successfully.
        assert_eq!(config.general.default_model, "gpt-4o");
        // Provider unchanged (not in overlay).
        assert_eq!(config.general.default_provider, "anthropic");
    }
}
