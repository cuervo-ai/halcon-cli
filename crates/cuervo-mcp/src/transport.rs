//! MCP stdio transport: spawns a child process and communicates
//! via newline-delimited JSON on stdin/stdout.

use std::collections::HashMap;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use crate::error::{McpError, McpResult};
use crate::types::{JsonRpcRequest, JsonRpcResponse};

/// Stdio transport for MCP communication with a child process.
///
/// Cannot derive Debug due to Mutex<Child>/ChildStdin/ChildStdout.
pub struct StdioTransport {
    child: Mutex<Child>,
    stdin: Mutex<tokio::process::ChildStdin>,
    reader: Mutex<BufReader<tokio::process::ChildStdout>>,
}

impl std::fmt::Debug for StdioTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdioTransport").finish_non_exhaustive()
    }
}

impl StdioTransport {
    /// Spawn a child process and create a transport.
    pub fn spawn(command: &str, args: &[String], env: &HashMap<String, String>) -> McpResult<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);

        for (key, val) in env {
            cmd.env(key, val);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| McpError::ProcessStart(format!("Failed to spawn '{command}': {e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Transport("Failed to open stdin".into()))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Transport("Failed to open stdout".into()))?;

        Ok(Self {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            reader: Mutex::new(BufReader::new(stdout)),
        })
    }

    /// Send a JSON-RPC request as a newline-delimited JSON message.
    pub async fn send(&self, request: &JsonRpcRequest) -> McpResult<()> {
        let mut line = serde_json::to_string(request)?;
        line.push('\n');

        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| McpError::Transport(format!("Write failed: {e}")))?;
        stdin
            .flush()
            .await
            .map_err(|e| McpError::Transport(format!("Flush failed: {e}")))?;

        Ok(())
    }

    /// Read the next JSON-RPC response (newline-delimited JSON).
    pub async fn receive(&self) -> McpResult<JsonRpcResponse> {
        let mut reader = self.reader.lock().await;
        let mut line = String::new();

        reader
            .read_line(&mut line)
            .await
            .map_err(|e| McpError::Transport(format!("Read failed: {e}")))?;

        if line.is_empty() {
            return Err(McpError::Transport("Server closed connection".into()));
        }

        let response: JsonRpcResponse = serde_json::from_str(line.trim())?;
        Ok(response)
    }

    /// Kill the child process and clean up.
    pub async fn close(&self) -> McpResult<()> {
        let mut child = self.child.lock().await;
        let _ = child.kill().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spawn_nonexistent_command_fails() {
        let result = StdioTransport::spawn("nonexistent_command_xyz_12345", &[], &HashMap::new());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, McpError::ProcessStart(_)));
    }

    #[tokio::test]
    async fn spawn_echo_server_and_close() {
        // Use cat as a simple echo: it reads stdin and writes to stdout.
        let transport = StdioTransport::spawn("cat", &[], &HashMap::new());
        if let Ok(t) = transport {
            t.close().await.unwrap();
        }
    }

    #[tokio::test]
    async fn send_and_receive_via_cat() {
        // cat echoes stdin to stdout — we can use it as a trivial "MCP server".
        let transport = StdioTransport::spawn("cat", &[], &HashMap::new()).unwrap();

        let request = JsonRpcRequest::new(1, "test/method", None);
        transport.send(&request).await.unwrap();

        // cat echoes it back, but as the *request* JSON (not a response).
        // We parse what comes back — it won't be a valid response, but we can
        // verify the transport round-trip works.
        let mut reader = transport.reader.lock().await;
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        drop(reader);

        assert!(line.contains("test/method"));
        assert!(line.contains("\"jsonrpc\":\"2.0\""));

        transport.close().await.unwrap();
    }
}
