use async_trait::async_trait;

use crate::error::Result;

/// A JSON-RPC message for the MCP protocol.
///
/// This is an opaque container — the actual JSON-RPC structure
/// is defined in cuervo-mcp. The core trait only needs to know
/// it can send and receive JSON values.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct JsonRpcMessage(pub serde_json::Value);

/// Transport layer for MCP communication.
///
/// Implementations handle the physical transport: stdio pipes,
/// HTTP+SSE, or other future transports.
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send a JSON-RPC message to the MCP server.
    async fn send(&self, message: &JsonRpcMessage) -> Result<()>;

    /// Receive the next JSON-RPC message from the MCP server.
    async fn receive(&self) -> Result<JsonRpcMessage>;

    /// Close the transport and clean up resources.
    async fn close(&self) -> Result<()>;
}
