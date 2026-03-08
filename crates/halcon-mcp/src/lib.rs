//! MCP (Model Context Protocol) runtime for Halcon CLI.
//!
//! Provides both MCP Host (client) and MCP Server capabilities:
//! - **Host**: stdio transport, tool discovery, tool execution, bridging
//! - **Server**: exposes halcon tools via JSON-RPC over stdin/stdout
//! - **OAuth**: OAuth 2.1 + PKCE S256 authorization for HTTP MCP servers
//! - **Scope**: 3-scope TOML config (local > project > user)
//! - **Tool Search**: deferred tool loading with nucleo fuzzy search
//! - **HTTP**: Streamable-HTTP / SSE transport for remote MCP servers

pub mod bridge;
pub mod error;
pub mod host;
pub mod http_server;
pub mod http_transport;
pub mod oauth;
pub mod pool;
pub mod scope;
pub mod server;
pub mod tool_search;
pub mod transport;
pub mod types;

pub use bridge::McpToolBridge;
pub use error::{McpError, McpResult};
pub use host::{extract_text, McpHost};
pub use http_transport::HttpTransport;
pub use oauth::{McpToken, OAuthError, OAuthManager};
pub use pool::{McpConnectionHealth, McpPool, McpServerDef};
pub use scope::{
    expand_env, expand_spec_env, remove_server, write_server, McpScope, McpServerSpec,
    McpTransport, MergedMcpConfig,
};
pub use http_server::McpHttpServer;
pub use server::McpServer;
pub use tool_search::{
    format_search_results, search_tools_definition, IndexedTool, ToolSearchIndex,
};
pub use transport::StdioTransport;
pub use types::JsonRpcNotification;

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
