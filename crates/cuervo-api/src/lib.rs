pub mod error;
pub mod types;

#[cfg(feature = "server")]
pub mod server;

/// API version string.
pub const API_VERSION: &str = "v1";

/// Default server port.
pub const DEFAULT_PORT: u16 = 9849;

/// Default bind address (localhost only for security).
pub const DEFAULT_BIND: &str = "127.0.0.1";
