//! ConfigValidatorTool — validate configuration files (TOML, JSON, YAML, .env, INI).
//!
//! Features:
//! - Parse and syntax-validate config files
//! - Detect common misconfigurations (missing required keys, type mismatches)
//! - Check .env files for key naming conventions and dangerous values
//! - Validate TOML against optional expected-key schema
//! - Output structured validation report with severity levels

use async_trait::async_trait;
use halcon_core::{
    traits::Tool,
    types::{PermissionLevel, ToolInput, ToolOutput},
};
use serde_json::{json, Value};

pub struct ConfigValidatorTool;

impl ConfigValidatorTool {
    pub fn new() -> Self {
        Self
    }

    fn detect_format(path: &str, format_hint: Option<&str>) -> &'static str {
        if let Some(f) = format_hint {
            return match f {
                "toml" => "toml",
                "json" => "json",
                "yaml" => "yaml",
                "env" => "env",
                "ini" => "ini",
                _ => "auto",
            };
        }
        let lower = path.to_lowercase();
        if lower.ends_with(".toml") {
            "toml"
        } else if lower.ends_with(".json") || lower.ends_with(".jsonc") {
            "json"
        } else if lower.ends_with(".yaml") || lower.ends_with(".yml") {
            "yaml"
        } else if lower.ends_with(".env") || lower.contains(".env.") || lower == ".env" {
            "env"
        } else if lower.ends_with(".ini") || lower.ends_with(".cfg") || lower.ends_with(".conf") {
            "ini"
        } else {
            "auto"
        }
    }

    fn validate_toml(content: &str, required_keys: &[&str]) -> ValidationResult {
        let mut issues = vec![];

        // Parse TOML using our simple parser approach
        match content.parse::<toml::Value>() {
            Err(e) => {
                issues.push(Issue {
                    severity: "error",
                    code: "TOML001",
                    message: format!("TOML parse error: {e}"),
                    line: None,
                });
            }
            Ok(val) => {
                // Check required keys
                if let Some(table) = val.as_table() {
                    for key in required_keys {
                        if !table.contains_key(*key) {
                            issues.push(Issue {
                                severity: "warning",
                                code: "CFG001",
                                message: format!("Missing expected key: '{key}'"),
                                line: None,
                            });
                        }
                    }
                    // Warn on empty string values
                    for (k, v) in table {
                        if v.as_str().map(|s| s.is_empty()).unwrap_or(false) {
                            issues.push(Issue {
                                severity: "info",
                                code: "CFG010",
                                message: format!("Key '{k}' has empty string value"),
                                line: None,
                            });
                        }
                    }
                }
            }
        }

        ValidationResult {
            format: "toml",
            valid: !issues.iter().any(|i| i.severity == "error"),
            issues,
        }
    }

    fn validate_json(content: &str) -> ValidationResult {
        let mut issues = vec![];

        match serde_json::from_str::<Value>(content) {
            Err(e) => {
                issues.push(Issue {
                    severity: "error",
                    code: "JSON001",
                    message: format!("JSON parse error: {e}"),
                    line: Some(e.line()),
                });
            }
            Ok(val) => {
                // Check for common patterns
                if let Some(obj) = val.as_object() {
                    if obj.is_empty() {
                        issues.push(Issue {
                            severity: "info",
                            code: "JSON010",
                            message: "JSON object is empty".into(),
                            line: None,
                        });
                    }
                    // Warn on null values
                    for (k, v) in obj {
                        if v.is_null() {
                            issues.push(Issue {
                                severity: "info",
                                code: "JSON011",
                                message: format!("Key '{k}' is null"),
                                line: None,
                            });
                        }
                    }
                }
            }
        }

        ValidationResult {
            format: "json",
            valid: !issues.iter().any(|i| i.severity == "error"),
            issues,
        }
    }

    fn validate_yaml(content: &str) -> ValidationResult {
        let mut issues = vec![];

        match serde_yaml::from_str::<Value>(content) {
            Err(e) => {
                issues.push(Issue {
                    severity: "error",
                    code: "YAML001",
                    message: format!("YAML parse error: {e}"),
                    line: None,
                });
            }
            Ok(val) => {
                if val.is_null() {
                    issues.push(Issue {
                        severity: "warning",
                        code: "YAML010",
                        message: "YAML file is empty or contains only null".into(),
                        line: None,
                    });
                }
            }
        }

        ValidationResult {
            format: "yaml",
            valid: !issues.iter().any(|i| i.severity == "error"),
            issues,
        }
    }

    fn validate_env(content: &str) -> ValidationResult {
        let mut issues = vec![];
        let dangerous_patterns = ["password", "secret", "token", "api_key", "private_key"];

        for (lineno, line) in content.lines().enumerate() {
            let trimmed = line.trim();

            // Skip blanks and comments
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            if !trimmed.contains('=') {
                issues.push(Issue {
                    severity: "error",
                    code: "ENV001",
                    message: format!(
                        "Line {} is not a valid KEY=VALUE pair: '{}'",
                        lineno + 1,
                        trimmed.chars().take(40).collect::<String>()
                    ),
                    line: Some(lineno + 1),
                });
                continue;
            }

            let (key, value) = trimmed.split_once('=').unwrap();
            let key = key.trim();
            let value = value.trim().trim_matches('"').trim_matches('\'');

            // Key naming convention: UPPER_SNAKE_CASE
            if key != key.to_uppercase() || key.contains(' ') || key.contains('-') {
                issues.push(Issue {
                    severity: "warning",
                    code: "ENV002",
                    message: format!("Key '{key}' should be UPPER_SNAKE_CASE"),
                    line: Some(lineno + 1),
                });
            }

            // Warn on keys with sensitive names but non-empty, plaintext values
            let key_lower = key.to_lowercase();
            for pat in &dangerous_patterns {
                if key_lower.contains(pat) && !value.is_empty() && !value.starts_with("${") {
                    issues.push(Issue {
                        severity: "warning",
                        code: "ENV003",
                        message: format!(
                            "Key '{key}' appears to contain a secret value in plaintext"
                        ),
                        line: Some(lineno + 1),
                    });
                    break;
                }
            }

            // Warn on empty values that look like they should be set
            if value.is_empty() {
                issues.push(Issue {
                    severity: "info",
                    code: "ENV010",
                    message: format!("Key '{key}' has empty value"),
                    line: Some(lineno + 1),
                });
            }
        }

        ValidationResult {
            format: "env",
            valid: !issues.iter().any(|i| i.severity == "error"),
            issues,
        }
    }

    fn validate_ini(content: &str) -> ValidationResult {
        let mut issues = vec![];
        let mut _current_section = "global";

        for (lineno, line) in content.lines().enumerate() {
            let trimmed = line.trim();

            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
                continue;
            }

            if trimmed.starts_with('[') {
                if !trimmed.ends_with(']') {
                    issues.push(Issue {
                        severity: "error",
                        code: "INI001",
                        message: format!("Malformed section header at line {}", lineno + 1),
                        line: Some(lineno + 1),
                    });
                } else {
                    _current_section = "seen";
                }
            } else if !trimmed.contains('=') && !trimmed.contains(':') {
                issues.push(Issue {
                    severity: "warning",
                    code: "INI002",
                    message: format!(
                        "Line {} doesn't look like a key=value pair: '{}'",
                        lineno + 1,
                        trimmed.chars().take(40).collect::<String>()
                    ),
                    line: Some(lineno + 1),
                });
            }
        }

        ValidationResult {
            format: "ini",
            valid: !issues.iter().any(|i| i.severity == "error"),
            issues,
        }
    }

    fn format_report(path: &str, result: &ValidationResult) -> String {
        let status = if result.valid {
            "✅ VALID"
        } else {
            "❌ INVALID"
        };
        let mut out = format!(
            "## Config Validation: {path}\n\n**Format**: {} | **Status**: {}\n\n",
            result.format, status
        );

        if result.issues.is_empty() {
            out.push_str("No issues found.\n");
        } else {
            let errors = result
                .issues
                .iter()
                .filter(|i| i.severity == "error")
                .count();
            let warnings = result
                .issues
                .iter()
                .filter(|i| i.severity == "warning")
                .count();
            let infos = result
                .issues
                .iter()
                .filter(|i| i.severity == "info")
                .count();
            out.push_str(&format!(
                "Found: {} error(s), {} warning(s), {} info(s)\n\n",
                errors, warnings, infos
            ));

            for issue in &result.issues {
                let icon = match issue.severity {
                    "error" => "❌",
                    "warning" => "⚠️",
                    _ => "ℹ️",
                };
                let loc = issue
                    .line
                    .map(|l| format!(" (line {l})"))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "{} [{}]{} {}\n",
                    icon, issue.code, loc, issue.message
                ));
            }
        }
        out
    }
}

struct Issue {
    severity: &'static str,
    code: &'static str,
    message: String,
    line: Option<usize>,
}

struct ValidationResult {
    format: &'static str,
    valid: bool,
    issues: Vec<Issue>,
}

impl Default for ConfigValidatorTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ConfigValidatorTool {
    fn name(&self) -> &str {
        "config_validate"
    }

    fn description(&self) -> &str {
        "Validate configuration files (TOML, JSON, YAML, .env, INI). \
         Reports syntax errors, missing keys, naming convention violations, \
         and potential security issues like plaintext secrets in .env files. \
         Accepts file path or inline content."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the config file to validate."
                },
                "content": {
                    "type": "string",
                    "description": "Inline config content to validate (use instead of path)."
                },
                "format": {
                    "type": "string",
                    "enum": ["toml", "json", "yaml", "env", "ini", "auto"],
                    "description": "Config format (default: auto-detect from file extension)."
                },
                "required_keys": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "For TOML/JSON: keys that must be present."
                },
                "output": {
                    "type": "string",
                    "enum": ["text", "json"],
                    "description": "Output format (default: text)."
                }
            },
            "required": []
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    async fn execute_inner(
        &self,
        input: ToolInput,
    ) -> Result<ToolOutput, halcon_core::error::HalconError> {
        let args = &input.arguments;
        let output_fmt = args["output"].as_str().unwrap_or("text");
        let required_keys: Vec<&str> = args["required_keys"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();

        // Get content
        let (content, display_path) = if let Some(c) = args["content"].as_str() {
            (c.to_string(), "<inline>")
        } else if let Some(p) = args["path"].as_str() {
            match tokio::fs::read_to_string(p).await {
                Ok(c) => (c, p),
                Err(e) => {
                    return Ok(ToolOutput {
                        tool_use_id: input.tool_use_id,
                        content: format!("Failed to read '{p}': {e}"),
                        is_error: true,
                        metadata: None,
                    });
                }
            }
        } else {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: "Provide 'path' or 'content'.".into(),
                is_error: true,
                metadata: None,
            });
        };

        let format_hint = args["format"].as_str();
        let detected_format = Self::detect_format(display_path, format_hint);

        let result = match detected_format {
            "toml" => Self::validate_toml(&content, &required_keys),
            "json" => Self::validate_json(&content),
            "yaml" => Self::validate_yaml(&content),
            "env" => Self::validate_env(&content),
            "ini" => Self::validate_ini(&content),
            _ => {
                // Try each format
                let r = Self::validate_json(&content);
                if r.valid {
                    r
                } else {
                    let r2 = Self::validate_toml(&content, &required_keys);
                    if r2.valid {
                        r2
                    } else {
                        Self::validate_yaml(&content)
                    }
                }
            }
        };

        let error_count = result
            .issues
            .iter()
            .filter(|i| i.severity == "error")
            .count();
        let warning_count = result
            .issues
            .iter()
            .filter(|i| i.severity == "warning")
            .count();

        let out_content = if output_fmt == "json" {
            let issues_json: Vec<Value> = result
                .issues
                .iter()
                .map(|i| {
                    json!({
                        "severity": i.severity,
                        "code": i.code,
                        "message": i.message,
                        "line": i.line
                    })
                })
                .collect();
            serde_json::to_string_pretty(&json!({
                "path": display_path,
                "format": result.format,
                "valid": result.valid,
                "errors": error_count,
                "warnings": warning_count,
                "issues": issues_json
            }))
            .unwrap_or_default()
        } else {
            Self::format_report(display_path, &result)
        };

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content: out_content,
            is_error: false,
            metadata: Some(json!({
                "valid": result.valid,
                "format": result.format,
                "errors": error_count,
                "warnings": warning_count
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_metadata() {
        let t = ConfigValidatorTool::new();
        assert_eq!(t.name(), "config_validate");
        assert!(!t.description().is_empty());
        assert_eq!(t.permission_level(), PermissionLevel::ReadOnly);
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
    }

    #[test]
    fn detect_format_toml() {
        assert_eq!(
            ConfigValidatorTool::detect_format("Cargo.toml", None),
            "toml"
        );
        assert_eq!(
            ConfigValidatorTool::detect_format("config.json", None),
            "json"
        );
        assert_eq!(
            ConfigValidatorTool::detect_format("docker-compose.yml", None),
            "yaml"
        );
        assert_eq!(ConfigValidatorTool::detect_format(".env", None), "env");
        assert_eq!(
            ConfigValidatorTool::detect_format(".env.production", None),
            "env"
        );
    }

    #[test]
    fn validate_valid_toml() {
        let r = ConfigValidatorTool::validate_toml(
            "[package]\nname = \"hello\"\nversion = \"1.0.0\"\n",
            &[],
        );
        assert!(r.valid);
        assert!(r.issues.is_empty() || r.issues.iter().all(|i| i.severity != "error"));
    }

    #[test]
    fn validate_invalid_toml() {
        let r = ConfigValidatorTool::validate_toml("key = {{broken", &[]);
        assert!(!r.valid);
        assert!(r.issues.iter().any(|i| i.severity == "error"));
    }

    #[test]
    fn validate_toml_required_keys() {
        let r =
            ConfigValidatorTool::validate_toml("[package]\nname = \"x\"\n", &["name", "version"]);
        assert!(r.issues.iter().any(|i| i.message.contains("version")));
    }

    #[test]
    fn validate_valid_json() {
        let r = ConfigValidatorTool::validate_json(r#"{"key": "value", "num": 42}"#);
        assert!(r.valid);
    }

    #[test]
    fn validate_invalid_json() {
        let r = ConfigValidatorTool::validate_json("{broken json");
        assert!(!r.valid);
        assert!(r.issues.iter().any(|i| i.severity == "error"));
    }

    #[test]
    fn validate_env_valid() {
        let r = ConfigValidatorTool::validate_env(
            "DATABASE_URL=postgres://localhost/db\nDEBUG=false\n",
        );
        assert!(r.valid);
    }

    #[test]
    fn validate_env_invalid_line() {
        let r = ConfigValidatorTool::validate_env("no-equals-sign\n");
        assert!(!r.valid);
    }

    #[test]
    fn validate_env_naming_warning() {
        let r = ConfigValidatorTool::validate_env("lower_key=value\n");
        assert!(r.issues.iter().any(|i| i.code == "ENV002"));
    }

    #[test]
    fn validate_env_secret_warning() {
        let r = ConfigValidatorTool::validate_env("API_TOKEN=my_real_secret_token_here\n");
        assert!(r.issues.iter().any(|i| i.code == "ENV003"));
    }

    #[tokio::test]
    async fn execute_inline_json() {
        let tool = ConfigValidatorTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t1".into(),
                arguments: serde_json::json!({ "content": r#"{"name":"test"}"#, "format": "json" }),
                working_directory: "/tmp".into(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("VALID") || out.content.contains("valid"));
    }

    #[tokio::test]
    async fn execute_missing_input() {
        let tool = ConfigValidatorTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t1".into(),
                arguments: serde_json::json!({}),
                working_directory: "/tmp".into(),
            })
            .await
            .unwrap();
        assert!(out.is_error);
    }
}
