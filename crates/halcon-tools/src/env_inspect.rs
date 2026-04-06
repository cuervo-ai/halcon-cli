//! `env_inspect` tool: inspect environment variables.
//!
//! Lists environment variables matching an optional prefix or pattern.
//! Sensitive values (keys containing TOKEN, SECRET, KEY, PASSWORD, etc.)
//! are masked by default to prevent accidental credential exposure.

use async_trait::async_trait;
use serde_json::json;

use halcon_core::error::Result;
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

#[allow(unused_imports)]
use tracing::instrument;

pub struct EnvInspectTool;

impl EnvInspectTool {
    pub fn new() -> Self {
        Self
    }

    fn is_sensitive(key: &str) -> bool {
        let upper = key.to_uppercase();
        // Generic patterns
        upper.contains("TOKEN")
            || upper.contains("SECRET")
            || upper.contains("PASSWORD")
            || upper.contains("PASSWD")
            || upper.contains("API_KEY")
            || upper.contains("APIKEY")
            || upper.contains("PRIVATE")
            || upper.contains("CREDENTIAL")
            || upper.contains("AUTH")
            // Database connection strings and passwords
            || upper.contains("DATABASE_URL")
            || upper.contains("DB_PASSWORD")
            || upper.contains("DB_PASS")
            || upper.contains("PGPASSWORD")
            || upper.contains("PGPASS")
            || upper.contains("MYSQL_PWD")
            || upper.contains("REDIS_URL")
            || upper.contains("REDIS_PASSWORD")
            || upper.contains("REDIS_PASS")
            // Messaging and communication
            || upper.contains("SLACK_TOKEN")
            || upper.contains("DISCORD_TOKEN")
            || upper.contains("SMTP_PASSWORD")
            || upper.contains("SMTP_PASS")
            || upper.contains("MAIL_PASSWORD")
            // Cloud providers
            || upper.contains("AWS_SECRET")
            || upper.contains("CLOUDFLARE")
            || upper.contains("STRIPE_KEY")
            || upper.contains("STRIPE_SECRET")
            || upper.contains("TWILIO")
            // Signing / encryption
            || upper.contains("SIGNING_KEY")
            || upper.contains("ENCRYPTION_KEY")
            || upper.contains("HMAC")
    }

    fn mask_value(value: &str) -> String {
        if value.len() <= 4 {
            return "****".to_string();
        }
        let visible = &value[..4];
        let stars = "*".repeat(value.len().min(20) - 4);
        format!("{visible}{stars}")
    }
}

impl Default for EnvInspectTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for EnvInspectTool {
    fn name(&self) -> &str {
        "env_inspect"
    }

    fn description(&self) -> &str {
        "Inspect environment variables. \
         List all env vars or filter by prefix (e.g. 'HALCON_', 'PATH'). \
         Sensitive variables (API_KEY, TOKEN, SECRET, PASSWORD, etc.) are masked by default. \
         Use show_sensitive=true to reveal full values (requires explicit opt-in)."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn requires_confirmation(&self, _input: &ToolInput) -> bool {
        false
    }

    #[tracing::instrument(skip(self), fields(tool = "env_inspect"))]
    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let prefix = input
            .arguments
            .get("prefix")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_uppercase();

        let show_sensitive = input
            .arguments
            .get("show_sensitive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if show_sensitive {
            tracing::warn!(
                "env_inspect: show_sensitive=true requested — sensitive env var values will be exposed"
            );
        }

        let mut vars: Vec<(String, String)> = std::env::vars()
            .filter(|(k, _)| {
                if prefix.is_empty() {
                    true
                } else {
                    k.to_uppercase().starts_with(&prefix) || k.to_uppercase() == prefix
                }
            })
            .collect();

        vars.sort_by(|a, b| a.0.cmp(&b.0));

        if vars.is_empty() {
            let msg = if prefix.is_empty() {
                "No environment variables found.".to_string()
            } else {
                format!("No environment variables matching prefix '{prefix}'.")
            };
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: msg,
                is_error: false,
                metadata: Some(json!({ "count": 0, "prefix": prefix })),
            });
        }

        let mut masked_count = 0usize;
        let mut lines: Vec<String> = Vec::with_capacity(vars.len() + 2);
        lines.push(format!(
            "Environment Variables{}:\n",
            if prefix.is_empty() {
                String::new()
            } else {
                format!(" (prefix: {prefix})")
            }
        ));

        for (key, value) in &vars {
            let display_value = if Self::is_sensitive(key) && !show_sensitive {
                masked_count += 1;
                Self::mask_value(value)
            } else {
                value.clone()
            };
            lines.push(format!("  {key}={display_value}"));
        }

        if masked_count > 0 {
            lines.push(format!(
                "\n({masked_count} sensitive value(s) masked. Use show_sensitive=true to reveal.)"
            ));
        }

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content: lines.join("\n"),
            is_error: false,
            metadata: Some(json!({
                "count": vars.len(),
                "masked": masked_count,
                "prefix": prefix,
            })),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "prefix": {
                    "type": "string",
                    "description": "Optional prefix to filter variables (case-insensitive). E.g. 'PATH', 'HALCON_', 'RUST_'."
                },
                "show_sensitive": {
                    "type": "boolean",
                    "description": "If true, reveals values of sensitive variables (API keys, tokens, passwords). Default: false."
                }
            },
            "required": []
        })
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_sensitive_detects_tokens() {
        assert!(EnvInspectTool::is_sensitive("MY_API_KEY"));
        assert!(EnvInspectTool::is_sensitive("OPENAI_TOKEN"));
        assert!(EnvInspectTool::is_sensitive("DB_PASSWORD"));
        assert!(EnvInspectTool::is_sensitive("GITHUB_AUTH_TOKEN"));
        assert!(!EnvInspectTool::is_sensitive("HOME"));
        assert!(!EnvInspectTool::is_sensitive("PATH"));
        assert!(!EnvInspectTool::is_sensitive("RUST_LOG"));
    }

    #[test]
    fn mask_value_short() {
        assert_eq!(EnvInspectTool::mask_value("abc"), "****");
    }

    #[test]
    fn mask_value_long() {
        let masked = EnvInspectTool::mask_value("sk-ant-api03-XXXX");
        assert!(masked.starts_with("sk-a"));
        assert!(masked.contains('*'));
        assert!(!masked.contains("api03"));
    }

    #[test]
    fn name_and_schema() {
        let tool = EnvInspectTool::new();
        assert_eq!(tool.name(), "env_inspect");
        assert_eq!(tool.permission_level(), PermissionLevel::ReadOnly);
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
    }

    #[tokio::test]
    async fn lists_path_variable() {
        // PATH should always be set on Unix/macOS
        let tool = EnvInspectTool::new();
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({ "prefix": "PATH" }),
            working_directory: "/tmp".into(),
        };
        let out = tool.execute(input).await.unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("PATH"), "content: {}", out.content);
    }

    #[tokio::test]
    async fn empty_prefix_lists_all() {
        let tool = EnvInspectTool::new();
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({}),
            working_directory: "/tmp".into(),
        };
        let out = tool.execute(input).await.unwrap();
        assert!(!out.is_error);
        let count = out.metadata.as_ref().unwrap()["count"]
            .as_u64()
            .unwrap_or(0);
        assert!(count > 0, "should find at least one env var");
    }

    #[tokio::test]
    async fn nonexistent_prefix_returns_no_match() {
        let tool = EnvInspectTool::new();
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({ "prefix": "ZZZZ_NONEXISTENT_PREFIX_XYZ" }),
            working_directory: "/tmp".into(),
        };
        let out = tool.execute(input).await.unwrap();
        assert!(!out.is_error);
        assert!(
            out.content.contains("No environment variables"),
            "content: {}",
            out.content
        );
    }

    /// Extended sensitive patterns: database and cloud credentials.
    #[test]
    fn is_sensitive_extended_patterns() {
        // Database
        assert!(EnvInspectTool::is_sensitive("DATABASE_URL"));
        assert!(EnvInspectTool::is_sensitive("PGPASSWORD"));
        assert!(EnvInspectTool::is_sensitive("PGPASS_FILE"));
        assert!(EnvInspectTool::is_sensitive("DB_PASSWORD"));
        assert!(EnvInspectTool::is_sensitive("DB_PASS"));
        assert!(EnvInspectTool::is_sensitive("MYSQL_PWD"));
        assert!(EnvInspectTool::is_sensitive("REDIS_URL"));
        assert!(EnvInspectTool::is_sensitive("REDIS_PASSWORD"));
        assert!(EnvInspectTool::is_sensitive("REDIS_PASS"));
        // Communication
        assert!(EnvInspectTool::is_sensitive("SLACK_TOKEN"));
        assert!(EnvInspectTool::is_sensitive("DISCORD_TOKEN"));
        assert!(EnvInspectTool::is_sensitive("SMTP_PASSWORD"));
        assert!(EnvInspectTool::is_sensitive("SMTP_PASS"));
        assert!(EnvInspectTool::is_sensitive("MAIL_PASSWORD"));
        // Cloud
        assert!(EnvInspectTool::is_sensitive("AWS_SECRET_ACCESS_KEY"));
        assert!(EnvInspectTool::is_sensitive("CLOUDFLARE_API_KEY"));
        assert!(EnvInspectTool::is_sensitive("STRIPE_SECRET_KEY"));
        assert!(EnvInspectTool::is_sensitive("STRIPE_KEY"));
        assert!(EnvInspectTool::is_sensitive("TWILIO_AUTH_TOKEN"));
        // Signing/encryption
        assert!(EnvInspectTool::is_sensitive("SIGNING_KEY"));
        assert!(EnvInspectTool::is_sensitive("ENCRYPTION_KEY"));
        assert!(EnvInspectTool::is_sensitive("HMAC_SECRET"));
        // Negative cases — should NOT be masked
        assert!(!EnvInspectTool::is_sensitive("HOME"));
        assert!(!EnvInspectTool::is_sensitive("REDIS_HOST")); // host is not sensitive
        assert!(!EnvInspectTool::is_sensitive("DB_HOST")); // host is not sensitive
        assert!(!EnvInspectTool::is_sensitive("SMTP_HOST"));
        assert!(!EnvInspectTool::is_sensitive("LANG"));
        assert!(!EnvInspectTool::is_sensitive("TERM"));
    }
}
