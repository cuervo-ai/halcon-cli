//! Plugin discovery and instantiation.

use std::path::PathBuf;
use std::sync::Arc;

use crate::agent::RuntimeAgent;
use crate::bridges::cli_agent::CliProcessAgent;
use crate::bridges::http_agent::HttpRemoteAgent;

use super::{PluginManifest, PluginTransport};

/// Discovers and loads plugins from configured search paths.
pub struct PluginLoader {
    search_paths: Vec<PathBuf>,
}

impl PluginLoader {
    pub fn new(paths: Vec<PathBuf>) -> Self {
        Self {
            search_paths: paths,
        }
    }

    /// Scan search paths for `plugin.toml` files.
    pub fn discover(&self) -> Vec<PluginManifest> {
        let mut manifests = Vec::new();

        for path in &self.search_paths {
            if !path.exists() || !path.is_dir() {
                continue;
            }

            if let Ok(entries) = std::fs::read_dir(path) {
                for entry in entries.flatten() {
                    let entry_path = entry.path();
                    if entry_path.is_dir() {
                        let manifest_path = entry_path.join("plugin.toml");
                        if manifest_path.exists() {
                            match self.load_manifest(&manifest_path) {
                                Ok(m) => manifests.push(m),
                                Err(e) => {
                                    tracing::warn!(
                                        path = %manifest_path.display(),
                                        error = %e,
                                        "skipping invalid plugin manifest"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        manifests
    }

    fn load_manifest(&self, path: &std::path::Path) -> Result<PluginManifest, String> {
        let content =
            std::fs::read_to_string(path).map_err(|e| format!("read error: {e}"))?;
        let manifest: PluginManifest =
            toml::from_str(&content).map_err(|e| format!("parse error: {e}"))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Load a single plugin from its manifest.
    pub fn load(
        &self,
        manifest: &PluginManifest,
    ) -> Result<Arc<dyn RuntimeAgent>, String> {
        match &manifest.transport {
            PluginTransport::Stdio {
                command,
                args,
                env,
            } => {
                let agent = CliProcessAgent::new(
                    &manifest.name,
                    command,
                    args.clone(),
                    env.clone(),
                    manifest.capabilities.clone(),
                    std::time::Duration::from_secs(120),
                );
                Ok(Arc::new(agent))
            }
            PluginTransport::Http {
                endpoint,
                auth_header,
            } => {
                let auth = auth_header
                    .as_ref()
                    .map(|h| ("Authorization".to_string(), h.clone()));
                let agent = HttpRemoteAgent::new(
                    &manifest.name,
                    endpoint,
                    auth,
                    manifest.capabilities.clone(),
                );
                Ok(Arc::new(agent))
            }
            PluginTransport::UnixSocket { path } => {
                // Unix sockets not yet implemented — treat as stdio with socat
                Err(format!(
                    "unix socket transport not yet implemented (path: {path})"
                ))
            }
        }
    }

    /// Discover and load all plugins.
    /// Result type for a plugin load attempt.
    #[allow(clippy::type_complexity)]
    pub fn load_all(&self) -> Vec<(PluginManifest, Result<Arc<dyn RuntimeAgent>, String>)> {
        self.discover()
            .into_iter()
            .map(|m| {
                let result = self.load(&m);
                (m, result)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_plugin_dir(parent: &std::path::Path, name: &str, toml_content: &str) {
        let plugin_dir = parent.join(name);
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(plugin_dir.join("plugin.toml"), toml_content).unwrap();
    }

    #[test]
    fn discover_valid_plugins() {
        let dir = TempDir::new().unwrap();
        create_plugin_dir(
            dir.path(),
            "my-plugin",
            r#"
name = "my-plugin"
version = "1.0.0"
agent_kind = "cli_process"

[transport]
type = "stdio"
command = "echo"
"#,
        );

        let loader = PluginLoader::new(vec![dir.path().to_path_buf()]);
        let manifests = loader.discover();
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].name, "my-plugin");
    }

    #[test]
    fn discover_skips_invalid() {
        let dir = TempDir::new().unwrap();
        create_plugin_dir(dir.path(), "bad", "invalid toml {{{}}}");
        create_plugin_dir(
            dir.path(),
            "good",
            r#"
name = "good"
version = "1.0.0"
agent_kind = "plugin"

[transport]
type = "stdio"
command = "echo"
"#,
        );

        let loader = PluginLoader::new(vec![dir.path().to_path_buf()]);
        let manifests = loader.discover();
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].name, "good");
    }

    #[test]
    fn discover_empty_dir() {
        let dir = TempDir::new().unwrap();
        let loader = PluginLoader::new(vec![dir.path().to_path_buf()]);
        let manifests = loader.discover();
        assert!(manifests.is_empty());
    }

    #[test]
    fn discover_nonexistent_path() {
        let loader = PluginLoader::new(vec![PathBuf::from("/nonexistent/path/xyz")]);
        let manifests = loader.discover();
        assert!(manifests.is_empty());
    }

    #[test]
    fn load_stdio_plugin() {
        let manifest = PluginManifest {
            name: "echo-agent".to_string(),
            version: "1.0".to_string(),
            description: "".to_string(),
            agent_kind: crate::agent::AgentKind::CliProcess,
            transport: PluginTransport::Stdio {
                command: "echo".to_string(),
                args: vec!["hello".to_string()],
                env: std::collections::HashMap::new(),
            },
            capabilities: vec![crate::agent::AgentCapability::ShellExecution],
            config: std::collections::HashMap::new(),
        };

        let loader = PluginLoader::new(vec![]);
        let result = loader.load(&manifest);
        assert!(result.is_ok());
        let agent = result.unwrap();
        assert_eq!(agent.descriptor().name, "echo-agent");
    }

    #[test]
    fn load_http_plugin() {
        let manifest = PluginManifest {
            name: "api-agent".to_string(),
            version: "1.0".to_string(),
            description: "".to_string(),
            agent_kind: crate::agent::AgentKind::HttpEndpoint,
            transport: PluginTransport::Http {
                endpoint: "https://api.example.com/agent".to_string(),
                auth_header: Some("Bearer token123".to_string()),
            },
            capabilities: vec![],
            config: std::collections::HashMap::new(),
        };

        let loader = PluginLoader::new(vec![]);
        let result = loader.load(&manifest);
        assert!(result.is_ok());
    }

    #[test]
    fn load_unix_socket_not_implemented() {
        let manifest = PluginManifest {
            name: "sock-agent".to_string(),
            version: "1.0".to_string(),
            description: "".to_string(),
            agent_kind: crate::agent::AgentKind::Plugin,
            transport: PluginTransport::UnixSocket {
                path: "/tmp/agent.sock".to_string(),
            },
            capabilities: vec![],
            config: std::collections::HashMap::new(),
        };

        let loader = PluginLoader::new(vec![]);
        let result = loader.load(&manifest);
        assert!(result.is_err());
    }

    #[test]
    fn search_path_ordering() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();

        create_plugin_dir(
            dir1.path(),
            "plugin-a",
            r#"
name = "from-dir1"
version = "1.0.0"
agent_kind = "plugin"

[transport]
type = "stdio"
command = "echo"
"#,
        );
        create_plugin_dir(
            dir2.path(),
            "plugin-b",
            r#"
name = "from-dir2"
version = "1.0.0"
agent_kind = "plugin"

[transport]
type = "stdio"
command = "echo"
"#,
        );

        let loader =
            PluginLoader::new(vec![dir1.path().to_path_buf(), dir2.path().to_path_buf()]);
        let manifests = loader.discover();
        assert_eq!(manifests.len(), 2);
        // First dir's plugins come first
        assert_eq!(manifests[0].name, "from-dir1");
        assert_eq!(manifests[1].name, "from-dir2");
    }

    #[test]
    fn load_all_mixed() {
        let dir = TempDir::new().unwrap();
        create_plugin_dir(
            dir.path(),
            "good",
            r#"
name = "good"
version = "1.0.0"
agent_kind = "cli_process"

[transport]
type = "stdio"
command = "echo"
"#,
        );
        create_plugin_dir(
            dir.path(),
            "sock",
            r#"
name = "sock"
version = "1.0.0"
agent_kind = "plugin"

[transport]
type = "unix_socket"
path = "/tmp/sock"
"#,
        );

        let loader = PluginLoader::new(vec![dir.path().to_path_buf()]);
        let results = loader.load_all();
        assert_eq!(results.len(), 2);
        // One should succeed (stdio), one should fail (unix socket)
        let successes = results.iter().filter(|(_, r)| r.is_ok()).count();
        let failures = results.iter().filter(|(_, r)| r.is_err()).count();
        assert_eq!(successes, 1);
        assert_eq!(failures, 1);
    }
}
