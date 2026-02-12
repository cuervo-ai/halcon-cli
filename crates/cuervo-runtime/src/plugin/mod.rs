//! TOML manifest-based plugin discovery and loading.

pub mod loader;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::agent::{AgentCapability, AgentKind};

/// A plugin manifest (loaded from `plugin.toml`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub agent_kind: AgentKind,
    pub transport: PluginTransport,
    #[serde(default)]
    pub capabilities: Vec<AgentCapability>,
    #[serde(default)]
    pub config: HashMap<String, serde_json::Value>,
}

/// Transport configuration for a plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginTransport {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
    },
    Http {
        endpoint: String,
        #[serde(default)]
        auth_header: Option<String>,
    },
    UnixSocket {
        path: String,
    },
}

impl PluginManifest {
    /// Validate the manifest has required fields.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("name is required".to_string());
        }
        if self.version.is_empty() {
            return Err("version is required".to_string());
        }
        match &self.transport {
            PluginTransport::Stdio { command, .. } => {
                if command.is_empty() {
                    return Err("stdio transport requires a command".to_string());
                }
            }
            PluginTransport::Http { endpoint, .. } => {
                if endpoint.is_empty() {
                    return Err("http transport requires an endpoint".to_string());
                }
            }
            PluginTransport::UnixSocket { path } => {
                if path.is_empty() {
                    return Err("unix_socket transport requires a path".to_string());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parse_stdio() {
        let manifest = PluginManifest {
            name: "my-tool".to_string(),
            version: "1.0.0".to_string(),
            description: "A cool tool".to_string(),
            agent_kind: AgentKind::CliProcess,
            transport: PluginTransport::Stdio {
                command: "/usr/bin/my-tool".to_string(),
                args: vec!["--json".to_string()],
                env: HashMap::new(),
            },
            capabilities: vec![AgentCapability::ShellExecution],
            config: HashMap::new(),
        };

        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn manifest_parse_http() {
        let manifest = PluginManifest {
            name: "remote-api".to_string(),
            version: "2.0.0".to_string(),
            description: "Remote API agent".to_string(),
            agent_kind: AgentKind::HttpEndpoint,
            transport: PluginTransport::Http {
                endpoint: "https://api.example.com/agent".to_string(),
                auth_header: Some("Bearer token123".to_string()),
            },
            capabilities: vec![AgentCapability::WebSearch, AgentCapability::Research],
            config: HashMap::new(),
        };

        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn manifest_serde_roundtrip() {
        let manifest = PluginManifest {
            name: "test".to_string(),
            version: "0.1.0".to_string(),
            description: "Test plugin".to_string(),
            agent_kind: AgentKind::Plugin,
            transport: PluginTransport::Stdio {
                command: "echo".to_string(),
                args: vec![],
                env: HashMap::new(),
            },
            capabilities: vec![],
            config: HashMap::new(),
        };

        let json = serde_json::to_string(&manifest).unwrap();
        let parsed: PluginManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test");
        assert_eq!(parsed.version, "0.1.0");
    }

    #[test]
    fn manifest_validate_empty_name() {
        let manifest = PluginManifest {
            name: "".to_string(),
            version: "1.0".to_string(),
            description: "".to_string(),
            agent_kind: AgentKind::Plugin,
            transport: PluginTransport::Stdio {
                command: "echo".to_string(),
                args: vec![],
                env: HashMap::new(),
            },
            capabilities: vec![],
            config: HashMap::new(),
        };
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn manifest_validate_empty_version() {
        let manifest = PluginManifest {
            name: "test".to_string(),
            version: "".to_string(),
            description: "".to_string(),
            agent_kind: AgentKind::Plugin,
            transport: PluginTransport::Stdio {
                command: "echo".to_string(),
                args: vec![],
                env: HashMap::new(),
            },
            capabilities: vec![],
            config: HashMap::new(),
        };
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn manifest_validate_empty_command() {
        let manifest = PluginManifest {
            name: "test".to_string(),
            version: "1.0".to_string(),
            description: "".to_string(),
            agent_kind: AgentKind::Plugin,
            transport: PluginTransport::Stdio {
                command: "".to_string(),
                args: vec![],
                env: HashMap::new(),
            },
            capabilities: vec![],
            config: HashMap::new(),
        };
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn manifest_validate_empty_endpoint() {
        let manifest = PluginManifest {
            name: "test".to_string(),
            version: "1.0".to_string(),
            description: "".to_string(),
            agent_kind: AgentKind::HttpEndpoint,
            transport: PluginTransport::Http {
                endpoint: "".to_string(),
                auth_header: None,
            },
            capabilities: vec![],
            config: HashMap::new(),
        };
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn manifest_validate_unix_socket() {
        let manifest = PluginManifest {
            name: "test".to_string(),
            version: "1.0".to_string(),
            description: "".to_string(),
            agent_kind: AgentKind::Plugin,
            transport: PluginTransport::UnixSocket {
                path: "/tmp/agent.sock".to_string(),
            },
            capabilities: vec![],
            config: HashMap::new(),
        };
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn manifest_with_config() {
        let mut config = HashMap::new();
        config.insert("max_retries".to_string(), serde_json::json!(3));
        config.insert("verbose".to_string(), serde_json::json!(true));

        let manifest = PluginManifest {
            name: "test".to_string(),
            version: "1.0".to_string(),
            description: "".to_string(),
            agent_kind: AgentKind::Plugin,
            transport: PluginTransport::Stdio {
                command: "echo".to_string(),
                args: vec![],
                env: HashMap::new(),
            },
            capabilities: vec![],
            config,
        };
        assert_eq!(manifest.config["max_retries"], 3);
    }

    #[test]
    fn manifest_multiple_capabilities() {
        let manifest = PluginManifest {
            name: "multi".to_string(),
            version: "1.0".to_string(),
            description: "".to_string(),
            agent_kind: AgentKind::Llm,
            transport: PluginTransport::Http {
                endpoint: "http://localhost:8080".to_string(),
                auth_header: None,
            },
            capabilities: vec![
                AgentCapability::CodeGeneration,
                AgentCapability::CodeReview,
                AgentCapability::Testing,
            ],
            config: HashMap::new(),
        };
        assert_eq!(manifest.capabilities.len(), 3);
    }
}
