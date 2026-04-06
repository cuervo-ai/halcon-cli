//! Dynamic Tool Manifest Loader (P1.2).
//!
//! Loads user-defined tool definitions from `~/.halcon/tools/*.toml` and
//! registers them as `ExternalTool` instances in the `ToolRegistry`.
//!
//! This allows adding custom tools without recompiling — useful for
//! project-specific scripts, internal APIs, or CI/CD wrappers.
//!
//! # Manifest format
//!
//! ```toml
//! [tool]
//! name = "run_tests"
//! description = "Run the project test suite"
//! permission = "Destructive"  # "ReadOnly" | "Destructive"
//!
//! [command]
//! # Template: use {{arg_name}} placeholders for substitution.
//! template = "cargo test {{filter}}"
//! # Optional working directory (defaults to CWD when tool runs).
//! working_dir = "."
//!
//! # Optional: JSON schema for the tool's input arguments.
//! # If omitted, the tool accepts a single "args" string parameter.
//! [schema]
//! type = "object"
//! [schema.properties.filter]
//! type = "string"
//! description = "Test name filter (optional, leave empty to run all)"
//! ```

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use halcon_core::error::{HalconError, Result as HalconResult};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};
use halcon_tools::ToolRegistry;

// ---------------------------------------------------------------------------
// TOML schema
// ---------------------------------------------------------------------------

/// The `[tool]` section of a manifest file.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolSection {
    pub name: String,
    pub description: String,
    /// "ReadOnly" or "Destructive". Defaults to "Destructive" for safety.
    #[serde(default = "default_permission")]
    pub permission: String,
}

/// The `[command]` section of a manifest file.
#[derive(Debug, Clone, Deserialize)]
pub struct CommandSection {
    /// Command template. Use `{{arg_name}}` for argument substitution.
    pub template: String,
    /// Optional working directory for the command.
    #[serde(default)]
    pub working_dir: Option<String>,
}

/// A full tool manifest definition.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolManifest {
    pub tool: ToolSection,
    pub command: CommandSection,
    /// JSON schema for the tool's input arguments.
    /// If omitted, defaults to `{"type":"object","properties":{"args":{"type":"string"}}}`.
    #[serde(default)]
    pub schema: Option<serde_json::Value>,
}

fn default_permission() -> String {
    "Destructive".into()
}

// ---------------------------------------------------------------------------
// ExternalTool — Tool impl backed by a shell command template
// ---------------------------------------------------------------------------

/// A tool defined by a TOML manifest and executed via shell command.
///
/// Argument substitution: placeholders `{{name}}` in the template are replaced
/// by the corresponding key in `input.arguments`.
///
/// Example: template `"cargo test {{filter}}"` with `{"filter": "my_test"}`
/// produces `"cargo test my_test"`.
pub struct ExternalTool {
    name: String,
    description: String,
    permission: PermissionLevel,
    template: String,
    working_dir: Option<String>,
    schema: serde_json::Value,
}

impl ExternalTool {
    pub fn from_manifest(manifest: ToolManifest) -> Self {
        let permission = match manifest.tool.permission.as_str() {
            "ReadOnly" => PermissionLevel::ReadOnly,
            _ => PermissionLevel::Destructive,
        };

        let schema = manifest.schema.unwrap_or_else(|| {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "args": {
                        "type": "string",
                        "description": "Command arguments"
                    }
                }
            })
        });

        Self {
            name: manifest.tool.name,
            description: manifest.tool.description,
            permission,
            template: manifest.command.template,
            working_dir: manifest.command.working_dir,
            schema,
        }
    }

    /// Substitute `{{arg_name}}` placeholders in the template with values from `args`.
    fn render_command(&self, args: &serde_json::Value) -> String {
        let mut cmd = self.template.clone();
        if let Some(obj) = args.as_object() {
            for (key, val) in obj {
                let placeholder = format!("{{{{{key}}}}}");
                let value = match val {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                cmd = cmd.replace(&placeholder, &value);
            }
        }
        // Remove any remaining unsubstituted placeholders.
        while let Some(start) = cmd.find("{{") {
            if let Some(end) = cmd[start..].find("}}") {
                cmd.replace_range(start..start + end + 2, "");
            } else {
                break;
            }
        }
        cmd.trim().to_string()
    }
}

#[async_trait]
impl Tool for ExternalTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn permission_level(&self) -> PermissionLevel {
        self.permission
    }

    fn input_schema(&self) -> serde_json::Value {
        self.schema.clone()
    }

    async fn execute_inner(&self, input: ToolInput) -> HalconResult<ToolOutput> {
        let rendered = self.render_command(&input.arguments);

        tracing::debug!(tool = %self.name, command = %rendered, "ExternalTool executing");

        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(&rendered);

        if let Some(ref wd) = self.working_dir {
            cmd.current_dir(wd);
        }

        // Capture both stdout and stderr.
        let output = cmd
            .output()
            .await
            .map_err(|e| HalconError::ToolExecutionFailed {
                tool: self.name.clone(),
                message: format!("Failed to spawn command '{rendered}': {e}"),
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let is_error = !output.status.success();

        let content = if stderr.is_empty() {
            stdout
        } else if stdout.is_empty() {
            stderr
        } else {
            format!("{stdout}\n---stderr---\n{stderr}")
        };

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error,
            metadata: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Loader
// ---------------------------------------------------------------------------

/// Load external tool manifests from `dir/*.toml` and register each as an
/// `ExternalTool` in `registry`.
///
/// * Missing directory → silent no-op.
/// * Parse errors → warning + skip.
/// * Name conflicts with existing tools → warning + skip (built-in tools take precedence).
pub fn load_external_tools(dir: &Path, registry: &mut ToolRegistry) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return, // Missing directory is a normal condition.
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "Failed to read tool manifest");
                continue;
            }
        };

        let manifest: ToolManifest = match toml::from_str(&content) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "Failed to parse tool manifest");
                continue;
            }
        };

        let tool_name = manifest.tool.name.clone();

        // Don't override built-in tools.
        if registry.get(&tool_name).is_some() {
            tracing::warn!(
                name = %tool_name,
                "External tool name conflicts with built-in — skipping"
            );
            continue;
        }

        let tool = ExternalTool::from_manifest(manifest);
        registry.register(Arc::new(tool));
        tracing::info!(name = %tool_name, "Registered external tool from tools.d");
    }
}

/// Load external tools from the default location: `~/.halcon/tools/`.
pub fn load_external_tools_default(registry: &mut ToolRegistry) {
    if let Some(home) = dirs::home_dir() {
        load_external_tools(&home.join(".halcon").join("tools"), registry);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_manifest(name: &str, template: &str, permission: &str) -> ToolManifest {
        ToolManifest {
            tool: ToolSection {
                name: name.into(),
                description: format!("{name} description"),
                permission: permission.into(),
            },
            command: CommandSection {
                template: template.into(),
                working_dir: None,
            },
            schema: None,
        }
    }

    // --- ExternalTool construction ---

    #[test]
    fn external_tool_name_and_description() {
        let t = ExternalTool::from_manifest(make_manifest("my_tool", "echo hi", "ReadOnly"));
        assert_eq!(t.name(), "my_tool");
        assert_eq!(t.description(), "my_tool description");
    }

    #[test]
    fn external_tool_readonly_permission() {
        let t = ExternalTool::from_manifest(make_manifest("t", "echo", "ReadOnly"));
        assert_eq!(t.permission_level(), PermissionLevel::ReadOnly);
    }

    #[test]
    fn external_tool_destructive_permission() {
        let t = ExternalTool::from_manifest(make_manifest("t", "echo", "Destructive"));
        assert_eq!(t.permission_level(), PermissionLevel::Destructive);
    }

    #[test]
    fn external_tool_unknown_permission_defaults_destructive() {
        let t = ExternalTool::from_manifest(make_manifest("t", "echo", "unknown"));
        assert_eq!(t.permission_level(), PermissionLevel::Destructive);
    }

    #[test]
    fn external_tool_default_schema_when_missing() {
        let t = ExternalTool::from_manifest(make_manifest("t", "echo", "ReadOnly"));
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["args"].is_object());
    }

    #[test]
    fn external_tool_custom_schema_preserved() {
        let mut manifest = make_manifest("t", "echo {{filter}}", "ReadOnly");
        manifest.schema = Some(serde_json::json!({
            "type": "object",
            "properties": {
                "filter": { "type": "string" }
            }
        }));
        let t = ExternalTool::from_manifest(manifest);
        assert!(t.input_schema()["properties"]["filter"].is_object());
    }

    // --- render_command ---

    #[test]
    fn render_command_no_placeholders() {
        let t = ExternalTool::from_manifest(make_manifest("t", "echo hello", "ReadOnly"));
        let cmd = t.render_command(&serde_json::json!({}));
        assert_eq!(cmd, "echo hello");
    }

    #[test]
    fn render_command_single_placeholder() {
        let t = ExternalTool::from_manifest(make_manifest("t", "echo {{msg}}", "ReadOnly"));
        let cmd = t.render_command(&serde_json::json!({"msg": "world"}));
        assert_eq!(cmd, "echo world");
    }

    #[test]
    fn render_command_multiple_placeholders() {
        let t = ExternalTool::from_manifest(make_manifest(
            "t",
            "{{cmd}} {{arg1}} {{arg2}}",
            "ReadOnly",
        ));
        let cmd =
            t.render_command(&serde_json::json!({"cmd": "ls", "arg1": "-la", "arg2": "/tmp"}));
        assert_eq!(cmd, "ls -la /tmp");
    }

    #[test]
    fn render_command_unmatched_placeholder_removed() {
        let t = ExternalTool::from_manifest(make_manifest("t", "echo {{missing}}", "ReadOnly"));
        let cmd = t.render_command(&serde_json::json!({}));
        assert_eq!(cmd, "echo");
    }

    #[test]
    fn render_command_number_arg() {
        let t = ExternalTool::from_manifest(make_manifest("t", "seq {{n}}", "ReadOnly"));
        let cmd = t.render_command(&serde_json::json!({"n": 5}));
        assert_eq!(cmd, "seq 5");
    }

    // --- load_external_tools ---

    #[test]
    fn load_missing_dir_is_noop() {
        let mut reg = ToolRegistry::new();
        load_external_tools(Path::new("/tmp/halcon_tools_nonexistent_xyz"), &mut reg);
        assert!(reg.get("any").is_none());
    }

    #[test]
    fn load_empty_dir_is_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let mut reg = ToolRegistry::new();
        load_external_tools(tmp.path(), &mut reg);
        // No tools registered.
        assert!(reg.tool_definitions().is_empty());
    }

    #[test]
    fn load_ignores_non_toml_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("tool.json"), "{}").unwrap();
        std::fs::write(tmp.path().join("tool.yaml"), "---").unwrap();
        let mut reg = ToolRegistry::new();
        load_external_tools(tmp.path(), &mut reg);
        assert!(reg.tool_definitions().is_empty());
    }

    #[test]
    fn load_valid_manifest_registers_tool() {
        let tmp = tempfile::tempdir().unwrap();
        let toml = r#"
[tool]
name = "my_script"
description = "Run my custom script"
permission = "ReadOnly"

[command]
template = "echo {{message}}"
"#;
        std::fs::write(tmp.path().join("my_script.toml"), toml).unwrap();
        let mut reg = ToolRegistry::new();
        load_external_tools(tmp.path(), &mut reg);
        assert!(reg.get("my_script").is_some());
    }

    #[test]
    fn load_skips_invalid_toml() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("bad.toml"), ":::invalid:::").unwrap();
        let valid = r#"
[tool]
name = "valid_ext"
description = "Valid"
permission = "ReadOnly"

[command]
template = "echo ok"
"#;
        std::fs::write(tmp.path().join("valid.toml"), valid).unwrap();
        let mut reg = ToolRegistry::new();
        load_external_tools(tmp.path(), &mut reg);
        assert!(reg.get("valid_ext").is_some());
        assert!(reg.get("bad").is_none());
    }

    #[test]
    fn load_does_not_override_existing_tool() {
        use halcon_core::error::Result as HalconResult;
        use halcon_core::traits::Tool;
        use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

        struct DummyTool;
        #[async_trait]
        impl Tool for DummyTool {
            fn name(&self) -> &str {
                "existing_tool"
            }
            fn description(&self) -> &str {
                "original"
            }
            fn permission_level(&self) -> PermissionLevel {
                PermissionLevel::ReadOnly
            }
            fn input_schema(&self) -> serde_json::Value {
                serde_json::json!({})
            }
            async fn execute_inner(&self, input: ToolInput) -> HalconResult<ToolOutput> {
                Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: "original".into(),
                    is_error: false,
                    metadata: None,
                })
            }
        }

        let tmp = tempfile::tempdir().unwrap();
        let toml = r#"
[tool]
name = "existing_tool"
description = "Overrider"
permission = "Destructive"

[command]
template = "rm -rf /"
"#;
        std::fs::write(tmp.path().join("overrider.toml"), toml).unwrap();

        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(DummyTool));
        load_external_tools(tmp.path(), &mut reg);

        // Built-in should remain.
        let tool = reg.get("existing_tool").unwrap();
        assert_eq!(tool.description(), "original");
    }

    // --- async execute ---

    #[tokio::test]
    async fn execute_simple_echo() {
        let t = ExternalTool::from_manifest(make_manifest("t", "echo hello_halcon", "ReadOnly"));
        let input = ToolInput {
            tool_use_id: "id1".into(),
            arguments: serde_json::json!({}),
            working_directory: String::new(),
        };
        let out = t.execute(input).await.unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("hello_halcon"));
    }

    #[tokio::test]
    async fn execute_with_argument_substitution() {
        let t = ExternalTool::from_manifest(make_manifest("t", "echo {{greeting}}", "ReadOnly"));
        let input = ToolInput {
            tool_use_id: "id2".into(),
            arguments: serde_json::json!({"greeting": "hello_world"}),
            working_directory: String::new(),
        };
        let out = t.execute(input).await.unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("hello_world"));
    }

    #[tokio::test]
    async fn execute_failed_command_sets_is_error() {
        let t = ExternalTool::from_manifest(make_manifest("t", "sh -c 'exit 1'", "ReadOnly"));
        let input = ToolInput {
            tool_use_id: "id3".into(),
            arguments: serde_json::json!({}),
            working_directory: String::new(),
        };
        let out = t.execute(input).await.unwrap();
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn execute_propagates_tool_use_id() {
        let t = ExternalTool::from_manifest(make_manifest("t", "echo x", "ReadOnly"));
        let input = ToolInput {
            tool_use_id: "my-id-xyz".into(),
            arguments: serde_json::json!({}),
            working_directory: String::new(),
        };
        let out = t.execute(input).await.unwrap();
        assert_eq!(out.tool_use_id, "my-id-xyz");
    }
}
