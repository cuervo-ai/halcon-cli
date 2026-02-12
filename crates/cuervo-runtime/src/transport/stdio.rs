//! Stdio transport for child process agents.
//!
//! Wraps a spawned process and communicates via JSON-delimited
//! messages over stdin/stdout.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use super::{AgentTransport, TransportMessage};
use crate::error::{Result, RuntimeError};

/// Stdio transport that wraps a child process.
pub struct StdioProcessTransport {
    child: Mutex<Child>,
    stdin: Mutex<tokio::process::ChildStdin>,
    stdout: Mutex<BufReader<tokio::process::ChildStdout>>,
    connected: AtomicBool,
}

impl StdioProcessTransport {
    /// Spawn a child process and wrap it in a transport.
    pub fn spawn(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| RuntimeError::Transport(format!("failed to spawn '{command}': {e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| RuntimeError::Transport("failed to capture stdin".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RuntimeError::Transport("failed to capture stdout".to_string()))?;

        Ok(Self {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            stdout: Mutex::new(BufReader::new(stdout)),
            connected: AtomicBool::new(true),
        })
    }
}

#[async_trait]
impl AgentTransport for StdioProcessTransport {
    async fn send(&self, message: TransportMessage) -> Result<()> {
        if !self.is_connected() {
            return Err(RuntimeError::Transport("process not connected".to_string()));
        }
        let mut json = serde_json::to_string(&message)
            .map_err(|e| RuntimeError::Transport(format!("serialize error: {e}")))?;
        json.push('\n');
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(json.as_bytes())
            .await
            .map_err(|e| RuntimeError::Transport(format!("stdin write error: {e}")))?;
        stdin
            .flush()
            .await
            .map_err(|e| RuntimeError::Transport(format!("stdin flush error: {e}")))?;
        Ok(())
    }

    async fn receive(&self) -> Result<TransportMessage> {
        if !self.is_connected() {
            return Err(RuntimeError::Transport("process not connected".to_string()));
        }
        let mut stdout = self.stdout.lock().await;
        let mut line = String::new();
        let n = stdout
            .read_line(&mut line)
            .await
            .map_err(|e| RuntimeError::Transport(format!("stdout read error: {e}")))?;
        if n == 0 {
            self.connected.store(false, Ordering::Release);
            return Err(RuntimeError::Transport("process stdout closed".to_string()));
        }
        serde_json::from_str(&line)
            .map_err(|e| RuntimeError::Transport(format!("deserialize error: {e}")))
    }

    async fn close(&self) -> Result<()> {
        self.connected.store(false, Ordering::Release);
        let mut child = self.child.lock().await;
        let _ = child.kill().await;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spawn_echo_process() {
        // Use cat as a simple echo server
        let transport = StdioProcessTransport::spawn("cat", &[], &HashMap::new());
        assert!(transport.is_ok());
        let transport = transport.unwrap();
        assert!(transport.is_connected());
        transport.close().await.unwrap();
        assert!(!transport.is_connected());
    }

    #[tokio::test]
    async fn spawn_bad_command() {
        let result =
            StdioProcessTransport::spawn("no_such_binary_xyz_123", &[], &HashMap::new());
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn send_and_receive_via_cat() {
        let transport = StdioProcessTransport::spawn("cat", &[], &HashMap::new()).unwrap();
        let msg = super::super::TransportMessage::new(
            super::super::TransportMessageKind::Request,
            serde_json::json!("test"),
        );
        let msg_id = msg.id;
        transport.send(msg).await.unwrap();
        let received = transport.receive().await.unwrap();
        assert_eq!(received.id, msg_id);
        transport.close().await.unwrap();
    }

    #[tokio::test]
    async fn send_after_close() {
        let transport = StdioProcessTransport::spawn("cat", &[], &HashMap::new()).unwrap();
        transport.close().await.unwrap();
        let msg = super::super::TransportMessage::new(
            super::super::TransportMessageKind::Request,
            serde_json::json!("test"),
        );
        let result = transport.send(msg).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn multiple_roundtrips() {
        let transport = StdioProcessTransport::spawn("cat", &[], &HashMap::new()).unwrap();
        for i in 0..3 {
            let msg = super::super::TransportMessage::new(
                super::super::TransportMessageKind::Request,
                serde_json::json!(i),
            );
            transport.send(msg).await.unwrap();
            let received = transport.receive().await.unwrap();
            assert_eq!(received.payload, serde_json::json!(i));
        }
        transport.close().await.unwrap();
    }

    #[tokio::test]
    async fn close_is_idempotent() {
        let transport = StdioProcessTransport::spawn("cat", &[], &HashMap::new()).unwrap();
        transport.close().await.unwrap();
        transport.close().await.unwrap(); // should not error
    }

    #[tokio::test]
    async fn spawn_with_env() {
        let mut env = HashMap::new();
        env.insert("MY_VAR".to_string(), "hello".to_string());
        let transport = StdioProcessTransport::spawn("cat", &[], &env);
        assert!(transport.is_ok());
        transport.unwrap().close().await.unwrap();
    }
}
