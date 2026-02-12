//! MCP (Model Context Protocol) runtime for Cuervo CLI.
//!
//! Provides both MCP Host (client) and MCP Server capabilities:
//! - **Host**: stdio transport, tool discovery, tool execution, bridging
//! - **Server**: exposes cuervo tools via JSON-RPC over stdin/stdout

pub mod bridge;
pub mod error;
pub mod host;
pub mod pool;
pub mod server;
pub mod transport;
pub mod types;

pub use bridge::McpToolBridge;
pub use error::{McpError, McpResult};
pub use host::{extract_text, McpHost};
pub use pool::{McpConnectionHealth, McpPool, McpServerDef};
pub use server::McpServer;
pub use transport::StdioTransport;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_test() {
        assert!(!version().is_empty());
    }
}
