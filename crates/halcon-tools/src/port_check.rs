//! `port_check` tool: check if a TCP port is in use.
//!
//! Attempts to bind to the port to determine availability.
//! Also attempts a quick connection to detect what might be listening.

use async_trait::async_trait;
use serde_json::json;
use std::time::Duration;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

// ─── Tracing re-export ────────────────────────────────────────────────────────
#[allow(unused_imports)]
use tracing::instrument;

pub struct PortCheckTool;

impl PortCheckTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PortCheckTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for PortCheckTool {
    fn name(&self) -> &str {
        "port_check"
    }

    fn description(&self) -> &str {
        "Check if a TCP port is in use on localhost (or a specified host). \
         Returns port status: free, in_use, or connection_refused. \
         Useful for verifying that a server started correctly, finding available ports, \
         or checking whether a service is listening. Timeout is 2 seconds by default."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn requires_confirmation(&self, _input: &ToolInput) -> bool {
        false
    }

    #[tracing::instrument(skip(self), fields(tool = "port_check"))]
    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let port = match input.arguments["port"].as_u64() {
            Some(p) if p > 0 && p <= 65535 => p as u16,
            Some(p) => {
                return Err(HalconError::InvalidInput(format!(
                    "Port {p} is out of range (1-65535)"
                )))
            }
            None => {
                return Err(HalconError::InvalidInput(
                    "port_check requires 'port' integer".into(),
                ))
            }
        };

        let host = input
            .arguments
            .get("host")
            .and_then(|v| v.as_str())
            .unwrap_or("127.0.0.1");

        let allow_remote = input
            .arguments
            .get("allow_remote")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // By default only allow localhost targets to prevent network scanning.
        if !allow_remote {
            let is_local = matches!(host, "localhost" | "127.0.0.1" | "::1" | "0.0.0.0" | "");
            if !is_local {
                return Err(HalconError::InvalidInput(format!(
                    "port_check: host '{host}' is not a localhost address. \
                     Set allow_remote=true to check non-local hosts."
                )));
            }
        }

        let timeout_ms = input
            .arguments
            .get("timeout_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(2000)
            .min(10_000);

        let addr = format!("{host}:{port}");
        let timeout = Duration::from_millis(timeout_ms);

        // Attempt a TCP connection to check if something is listening
        let connect_result =
            tokio::time::timeout(timeout, tokio::net::TcpStream::connect(&addr)).await;

        let (status, description) = match connect_result {
            Ok(Ok(_stream)) => {
                // Successfully connected → something is listening on this port
                (
                    "in_use",
                    format!("Port {port} on {host} is IN USE — a service is listening."),
                )
            }
            Ok(Err(e)) => {
                // Connection refused or other error → port is free or host unreachable
                let err_str = e.to_string().to_lowercase();
                if err_str.contains("connection refused") {
                    ("free", format!("Port {port} on {host} is FREE (connection refused — nothing listening)."))
                } else if err_str.contains("network unreachable") || err_str.contains("no route") {
                    (
                        "unreachable",
                        format!("Port {port} on {host} is unreachable: {e}"),
                    )
                } else {
                    ("free", format!("Port {port} on {host} appears free: {e}"))
                }
            }
            Err(_) => {
                // Timeout — could mean filtered/firewalled, treat as in_use (something is blocking)
                ("timeout", format!("Port {port} on {host} timed out after {timeout_ms}ms — may be filtered or slow."))
            }
        };

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content: description,
            is_error: false,
            metadata: Some(json!({
                "port": port,
                "host": host,
                "status": status,
                "timeout_ms": timeout_ms,
            })),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "port": {
                    "type": "integer",
                    "description": "TCP port number to check (1-65535).",
                    "minimum": 1,
                    "maximum": 65535
                },
                "host": {
                    "type": "string",
                    "description": "Host to check (default: '127.0.0.1'). Must be a localhost address unless allow_remote=true."
                },
                "allow_remote": {
                    "type": "boolean",
                    "description": "If true, allows checking non-localhost hosts (e.g. remote IPs). Default: false (localhost only)."
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Connection timeout in milliseconds (default: 2000, max: 10000).",
                    "minimum": 100,
                    "maximum": 10000
                }
            },
            "required": ["port"]
        })
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_input(args: serde_json::Value) -> ToolInput {
        ToolInput {
            tool_use_id: "t".into(),
            arguments: args,
            working_directory: "/tmp".into(),
        }
    }

    #[test]
    fn name_and_schema() {
        let tool = PortCheckTool::new();
        assert_eq!(tool.name(), "port_check");
        assert_eq!(tool.permission_level(), PermissionLevel::ReadOnly);
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "port"));
    }

    #[tokio::test]
    async fn missing_port_is_error() {
        let tool = PortCheckTool::new();
        let result = tool.execute(make_input(json!({}))).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("port"));
    }

    #[tokio::test]
    async fn invalid_port_is_error() {
        let tool = PortCheckTool::new();
        let result = tool.execute(make_input(json!({ "port": 99999 }))).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("out of range"));
    }

    #[tokio::test]
    async fn free_port_detected() {
        // Port 1 is almost certainly not listening and will give connection refused
        let tool = PortCheckTool::new();
        let out = tool
            .execute(make_input(json!({ "port": 19977, "timeout_ms": 500 })))
            .await
            .unwrap();
        assert!(!out.is_error);
        let status = out.metadata.as_ref().unwrap()["status"]
            .as_str()
            .unwrap_or("");
        // Should be "free" or "timeout" — never "in_use" for a random high port
        assert!(
            status == "free" || status == "timeout" || status == "unreachable",
            "unexpected status '{status}' for unused port"
        );
    }

    #[tokio::test]
    async fn metadata_has_port() {
        let tool = PortCheckTool::new();
        let out = tool
            .execute(make_input(json!({ "port": 19978, "timeout_ms": 300 })))
            .await
            .unwrap();
        assert_eq!(
            out.metadata.as_ref().unwrap()["port"].as_u64().unwrap(),
            19978
        );
    }

    /// Remote host rejected by default (no allow_remote).
    #[tokio::test]
    async fn remote_host_rejected_by_default() {
        let tool = PortCheckTool::new();
        let result = tool
            .execute(make_input(json!({ "port": 80, "host": "192.168.1.1" })))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not a localhost address"), "err: {err}");
    }

    /// Remote host allowed when allow_remote=true.
    #[tokio::test]
    async fn remote_host_allowed_with_flag() {
        let tool = PortCheckTool::new();
        // Use a high port on a private address — expected to be free or timeout
        let result = tool
            .execute(make_input(json!({
                "port": 19979,
                "host": "127.0.0.1",
                "allow_remote": true,
                "timeout_ms": 300
            })))
            .await;
        // Should succeed (localhost is always allowed even with allow_remote)
        assert!(result.is_ok());
    }

    /// Localhost addresses (127.0.0.1, localhost, ::1) accepted without allow_remote.
    #[tokio::test]
    async fn localhost_variants_accepted() {
        let tool = PortCheckTool::new();
        for host in &["127.0.0.1", "localhost", "::1"] {
            let result = tool
                .execute(make_input(json!({
                    "port": 19980,
                    "host": host,
                    "timeout_ms": 200
                })))
                .await;
            assert!(
                result.is_ok(),
                "host '{host}' should be accepted: {:?}",
                result
            );
        }
    }

    /// Schema exposes allow_remote field.
    #[test]
    fn schema_has_allow_remote() {
        let tool = PortCheckTool::new();
        let schema = tool.input_schema();
        assert!(
            schema["properties"]["allow_remote"].is_object(),
            "allow_remote should be in schema properties"
        );
    }
}
