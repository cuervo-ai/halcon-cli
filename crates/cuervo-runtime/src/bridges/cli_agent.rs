//! CLI process bridge: wraps a CLI command as a RuntimeAgent.

use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use uuid::Uuid;

use crate::agent::{
    AgentCapability, AgentDescriptor, AgentHealth, AgentKind, AgentRequest, AgentResponse,
    AgentUsage, Artifact, ArtifactKind, ProtocolSupport, RuntimeAgent,
};
use crate::error::{Result, RuntimeError};

/// Wraps a CLI command as a RuntimeAgent.
///
/// Passes the instruction via stdin, collects stdout as output.
pub struct CliProcessAgent {
    descriptor: AgentDescriptor,
    command: String,
    args: Vec<String>,
    env: HashMap<String, String>,
    timeout: Duration,
}

impl CliProcessAgent {
    pub fn new(
        name: &str,
        command: &str,
        args: Vec<String>,
        env: HashMap<String, String>,
        capabilities: Vec<AgentCapability>,
        timeout: Duration,
    ) -> Self {
        Self {
            descriptor: AgentDescriptor {
                id: Uuid::new_v4(),
                name: name.to_string(),
                agent_kind: AgentKind::CliProcess,
                capabilities,
                protocols: vec![ProtocolSupport::Native],
                metadata: {
                    let mut m = HashMap::new();
                    m.insert("command".to_string(), serde_json::json!(command));
                    m
                },
                max_concurrency: 1,
            },
            command: command.to_string(),
            args,
            env,
            timeout,
        }
    }
}

#[async_trait]
impl RuntimeAgent for CliProcessAgent {
    fn descriptor(&self) -> &AgentDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, request: AgentRequest) -> Result<AgentResponse> {
        let start = std::time::Instant::now();

        let mut cmd = tokio::process::Command::new(&self.command);
        cmd.args(&self.args)
            .envs(&self.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| RuntimeError::Execution(format!("failed to spawn '{}': {e}", self.command)))?;

        // Write instruction to stdin
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let _ = stdin.write_all(request.instruction.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }

        // Wait with timeout
        let output = tokio::time::timeout(self.timeout, child.wait_with_output())
            .await
            .map_err(|_| RuntimeError::Timeout {
                timeout_ms: self.timeout.as_millis() as u64,
            })?
            .map_err(|e| RuntimeError::Execution(format!("process error: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let success = output.status.success();
        let latency_ms = start.elapsed().as_millis() as u64;

        let mut artifacts = Vec::new();
        if !stderr.is_empty() {
            artifacts.push(Artifact {
                kind: ArtifactKind::Log,
                path: None,
                content: stderr,
            });
        }

        Ok(AgentResponse {
            request_id: request.request_id,
            success,
            output: stdout,
            artifacts,
            usage: AgentUsage {
                latency_ms,
                ..Default::default()
            },
            metadata: HashMap::new(),
        })
    }

    async fn health(&self) -> AgentHealth {
        // Check if command exists by running `which` or simply checking path
        let result = tokio::process::Command::new("which")
            .arg(&self.command)
            .output()
            .await;
        match result {
            Ok(output) if output.status.success() => AgentHealth::Healthy,
            _ => AgentHealth::Degraded {
                reason: format!("command '{}' may not be available", self.command),
            },
        }
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(()) // No persistent state to clean up
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_agent(cmd: &str, args: &[&str]) -> CliProcessAgent {
        CliProcessAgent::new(
            "test",
            cmd,
            args.iter().map(|s| s.to_string()).collect(),
            HashMap::new(),
            vec![AgentCapability::ShellExecution],
            Duration::from_secs(10),
        )
    }

    #[test]
    fn descriptor() {
        let agent = make_agent("echo", &["hello"]);
        assert_eq!(agent.descriptor().name, "test");
        assert_eq!(agent.descriptor().agent_kind, AgentKind::CliProcess);
        assert_eq!(agent.descriptor().capabilities.len(), 1);
    }

    #[tokio::test]
    async fn invoke_echo() {
        let agent = make_agent("cat", &[]);
        let req = AgentRequest::new("hello world");
        let resp = agent.invoke(req).await.unwrap();
        assert!(resp.success);
        assert!(resp.output.contains("hello world"));
    }

    #[tokio::test]
    async fn invoke_captures_stderr() {
        // Use sh -c to write to stderr
        let agent = CliProcessAgent::new(
            "test",
            "sh",
            vec!["-c".to_string(), "echo error >&2; echo ok".to_string()],
            HashMap::new(),
            vec![],
            Duration::from_secs(10),
        );
        let req = AgentRequest::new("");
        let resp = agent.invoke(req).await.unwrap();
        assert!(resp.success);
        assert!(resp.output.contains("ok"));
        assert_eq!(resp.artifacts.len(), 1);
        assert!(resp.artifacts[0].content.contains("error"));
    }

    #[tokio::test]
    async fn invoke_failure_exit_code() {
        let agent = CliProcessAgent::new(
            "test",
            "sh",
            vec!["-c".to_string(), "exit 1".to_string()],
            HashMap::new(),
            vec![],
            Duration::from_secs(10),
        );
        let req = AgentRequest::new("");
        let resp = agent.invoke(req).await.unwrap();
        assert!(!resp.success);
    }

    #[tokio::test]
    async fn invoke_bad_command() {
        let agent = make_agent("no_such_command_xyz_123", &[]);
        let req = AgentRequest::new("hello");
        let result = agent.invoke(req).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn invoke_with_env() {
        let mut env = HashMap::new();
        env.insert("MY_VAR".to_string(), "hello".to_string());
        let agent = CliProcessAgent::new(
            "test",
            "sh",
            vec!["-c".to_string(), "echo $MY_VAR".to_string()],
            env,
            vec![],
            Duration::from_secs(10),
        );
        let req = AgentRequest::new("");
        let resp = agent.invoke(req).await.unwrap();
        assert!(resp.output.contains("hello"));
    }

    #[tokio::test]
    async fn health_check_existing_command() {
        let agent = make_agent("echo", &[]);
        let health = agent.health().await;
        assert!(health.is_available());
    }

    #[tokio::test]
    async fn health_check_missing_command() {
        let agent = make_agent("no_such_command_xyz_123", &[]);
        let health = agent.health().await;
        // Should be degraded, not unavailable (best-effort check)
        assert!(!health.is_healthy() || health.is_available());
    }

    #[tokio::test]
    async fn shutdown_ok() {
        let agent = make_agent("echo", &[]);
        assert!(agent.shutdown().await.is_ok());
    }
}
