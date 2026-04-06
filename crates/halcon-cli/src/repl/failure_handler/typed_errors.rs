//! Typed Tool Failure Taxonomy — compiler-enforced error classification.
//!
//! # Motivation
//!
//! Replaces string-matching in `is_transient_error()` / `is_deterministic_error()`
//! with a typed enum. Benefits:
//! - **Exhaustive matching**: Rust `match` ensures all variants are handled
//! - **Structured context**: Each variant carries domain-specific fields
//! - **Observability**: Variant names become structured tracing fields
//! - **Zero runtime cost**: Enum discriminant + fields, no regex/contains
//!
//! # Xiyo Comparison
//!
//! Xiyo's toolExecution.ts uses `classifyToolError()` (line 1643) which returns
//! a string tag for analytics. Halcon's typed enum provides:
//! - Compiler-enforced exhaustiveness (Xiyo's string tags can drift)
//! - Structured context per variant (Xiyo only logs the string)
//! - Direct mapping to retry/repair/surface actions (Xiyo has no retry)
//!
//! # Migration Path
//!
//! 1. New code uses `ToolFailureKind::classify()` (returns typed enum)
//! 2. Old code continues using `is_transient_error()` / `is_deterministic_error()`
//! 3. Gradually replace string-matching call sites with typed enum
//! 4. Remove string-matching functions when all callers migrated

use std::path::PathBuf;

/// Typed tool failure classification.
///
/// Every tool failure maps to exactly one variant. The variant determines
/// the retry/repair/surface action in the failure waterfall.
#[derive(Debug, Clone)]
pub enum ToolFailureKind {
    // ═══ TRANSIENT (may succeed on retry) ═══
    /// Network timeout (provider or MCP server).
    NetworkTimeout {
        provider: Option<String>,
        timeout_ms: Option<u64>,
    },

    /// Rate limit hit (HTTP 429 or provider-specific).
    RateLimit {
        provider: Option<String>,
        retry_after_ms: Option<u64>,
    },

    /// Server error (HTTP 5xx).
    ServerError {
        status: u16,
        provider: Option<String>,
    },

    /// Connection reset / refused / broken pipe.
    ConnectionError { detail: String },

    /// Resource contention (cargo lock, file lock, EAGAIN).
    ResourceContention { resource: String },

    /// MCP transport/pool failure (connection dropped, channel closed).
    McpTransport {
        server: Option<String>,
        detail: String,
    },

    /// Provider overloaded (Anthropic 529, generic overload).
    ProviderOverloaded { provider: Option<String> },

    // ═══ DETERMINISTIC (will never succeed on retry) ═══
    /// File or directory not found.
    FileNotFound { path: Option<PathBuf> },

    /// Permission denied (filesystem).
    PermissionDenied { path: Option<PathBuf> },

    /// Path type mismatch (is a directory / not a directory).
    PathTypeMismatch {
        path: Option<PathBuf>,
        expected: &'static str,
    },

    /// Path traversal or security block.
    SecurityBlocked { reason: String },

    /// Tool not found in registry.
    UnknownTool { name: String },

    /// TBAC denied (task context whitelist).
    TbacDenied { context_id: Option<String> },

    /// Invalid schema or missing required fields.
    SchemaError { detail: String },

    /// Authentication failure (invalid API key, unauthorized, insufficient quota).
    AuthFailure {
        provider: Option<String>,
        detail: String,
    },

    /// MCP server initialization failure (process start failed, not initialized).
    McpInitFailure {
        server: Option<String>,
        detail: String,
    },

    // ═══ UNKNOWN (cannot classify) ═══
    /// Unrecognized error — default to surface.
    Unclassified { message: String },
}

impl ToolFailureKind {
    /// Whether this failure may succeed on retry.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::NetworkTimeout { .. }
                | Self::RateLimit { .. }
                | Self::ServerError { .. }
                | Self::ConnectionError { .. }
                | Self::ResourceContention { .. }
                | Self::McpTransport { .. }
                | Self::ProviderOverloaded { .. }
        )
    }

    /// Whether this failure will never succeed on retry.
    pub fn is_deterministic(&self) -> bool {
        matches!(
            self,
            Self::FileNotFound { .. }
                | Self::PermissionDenied { .. }
                | Self::PathTypeMismatch { .. }
                | Self::SecurityBlocked { .. }
                | Self::UnknownTool { .. }
                | Self::TbacDenied { .. }
                | Self::SchemaError { .. }
                | Self::AuthFailure { .. }
                | Self::McpInitFailure { .. }
        )
    }

    /// Whether the failure is unclassified.
    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unclassified { .. })
    }

    /// Canonical name for structured tracing / metrics.
    pub fn name(&self) -> &'static str {
        match self {
            Self::NetworkTimeout { .. } => "network_timeout",
            Self::RateLimit { .. } => "rate_limit",
            Self::ServerError { .. } => "server_error",
            Self::ConnectionError { .. } => "connection_error",
            Self::ResourceContention { .. } => "resource_contention",
            Self::McpTransport { .. } => "mcp_transport",
            Self::ProviderOverloaded { .. } => "provider_overloaded",
            Self::FileNotFound { .. } => "file_not_found",
            Self::PermissionDenied { .. } => "permission_denied",
            Self::PathTypeMismatch { .. } => "path_type_mismatch",
            Self::SecurityBlocked { .. } => "security_blocked",
            Self::UnknownTool { .. } => "unknown_tool",
            Self::TbacDenied { .. } => "tbac_denied",
            Self::SchemaError { .. } => "schema_error",
            Self::AuthFailure { .. } => "auth_failure",
            Self::McpInitFailure { .. } => "mcp_init_failure",
            Self::Unclassified { .. } => "unclassified",
        }
    }

    /// Map to the legacy `ErrorKind` for backward compatibility.
    pub fn to_error_kind(&self) -> super::classifier::ErrorKind {
        if self.is_transient() {
            super::classifier::ErrorKind::Transient
        } else if self.is_deterministic() {
            super::classifier::ErrorKind::Deterministic
        } else {
            super::classifier::ErrorKind::Unknown
        }
    }

    /// Classify an error string into a typed variant.
    ///
    /// This is the migration bridge: wraps the existing string-matching logic
    /// but returns a typed enum instead. As call sites migrate to passing
    /// structured error types, this function will shrink.
    pub fn classify(error: &str) -> Self {
        let lower = error.to_lowercase();

        // ── Transient patterns ──
        if lower.contains("timeout") || lower.contains("timed out") {
            return Self::NetworkTimeout {
                provider: None,
                timeout_ms: None,
            };
        }
        if lower.contains("rate limit") || lower.contains("rate_limit") || lower.contains("429") {
            return Self::RateLimit {
                provider: None,
                retry_after_ms: None,
            };
        }
        if lower.contains("529")
            || lower.contains("overloaded")
            || lower.contains("retryable error:")
        {
            return Self::ProviderOverloaded { provider: None };
        }
        if lower.contains("500")
            || lower.contains("502 bad gateway")
            || lower.contains("503 service unavailable")
            || lower.contains("504 gateway timeout")
        {
            let status = if lower.contains("500") {
                500
            } else if lower.contains("502") {
                502
            } else if lower.contains("503") {
                503
            } else {
                504
            };
            return Self::ServerError {
                status,
                provider: None,
            };
        }
        if lower.contains("connection reset")
            || lower.contains("connection refused")
            || lower.contains("broken pipe")
            || lower.contains("network error")
        {
            return Self::ConnectionError {
                detail: error.to_string(),
            };
        }
        if lower.contains("mcp pool call failed")
            || lower.contains("failed to call")
            || lower.contains("transport error")
            || lower.contains("channel closed")
        {
            return Self::McpTransport {
                server: None,
                detail: error.to_string(),
            };
        }
        if lower.contains(".cargo-lock")
            || lower.contains("cargo-lock")
            || lower.contains("could not acquire package cache lock")
            || lower.contains("file lock")
            || lower.contains("resource temporarily unavailable")
            || lower.contains("eagain")
        {
            return Self::ResourceContention {
                resource: error.to_string(),
            };
        }
        if lower.contains("temporary") {
            return Self::ConnectionError {
                detail: error.to_string(),
            };
        }

        // ── Deterministic patterns ──
        if lower.contains("no such file or directory")
            || lower.contains("not found")
            || lower.contains("does not exist")
        {
            return Self::FileNotFound { path: None };
        }
        if lower.contains("permission denied") {
            return Self::PermissionDenied { path: None };
        }
        if lower.contains("is a directory") || lower.contains("not a directory") {
            return Self::PathTypeMismatch {
                path: None,
                expected: if lower.contains("is a directory") {
                    "file"
                } else {
                    "directory"
                },
            };
        }
        if lower.contains("path traversal") || lower.contains("blocked by security") {
            return Self::SecurityBlocked {
                reason: error.to_string(),
            };
        }
        if lower.contains("unknown tool") {
            return Self::UnknownTool {
                name: error.to_string(),
            };
        }
        if lower.contains("denied by task context") {
            return Self::TbacDenied { context_id: None };
        }
        if lower.contains("schema") || lower.contains("missing required") {
            return Self::SchemaError {
                detail: error.to_string(),
            };
        }
        if lower.contains("credit balance")
            || lower.contains("invalid_api_key")
            || lower.contains("authentication")
            || lower.contains("unauthorized")
            || lower.contains("insufficient_quota")
        {
            return Self::AuthFailure {
                provider: None,
                detail: error.to_string(),
            };
        }
        if (lower.contains("mcp server is not initialized")
            || lower.contains("process start")
            || lower.contains("process failed"))
            && (lower.contains("mcp") || lower.contains("server"))
        {
            return Self::McpInitFailure {
                server: None,
                detail: error.to_string(),
            };
        }
        if lower.contains("invalid path") {
            return Self::SecurityBlocked {
                reason: error.to_string(),
            };
        }

        Self::Unclassified {
            message: error.to_string(),
        }
    }
}

impl std::fmt::Display for ToolFailureKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NetworkTimeout { provider, .. } => {
                write!(f, "network timeout")?;
                if let Some(p) = provider {
                    write!(f, " (provider: {p})")?;
                }
                Ok(())
            }
            Self::RateLimit { provider, .. } => {
                write!(f, "rate limit")?;
                if let Some(p) = provider {
                    write!(f, " (provider: {p})")?;
                }
                Ok(())
            }
            Self::ServerError { status, provider } => {
                write!(f, "server error {status}")?;
                if let Some(p) = provider {
                    write!(f, " (provider: {p})")?;
                }
                Ok(())
            }
            Self::ConnectionError { detail } => write!(f, "connection error: {detail}"),
            Self::ResourceContention { resource } => write!(f, "resource contention: {resource}"),
            Self::McpTransport { server, detail } => {
                write!(f, "MCP transport error: {detail}")?;
                if let Some(s) = server {
                    write!(f, " (server: {s})")?;
                }
                Ok(())
            }
            Self::ProviderOverloaded { provider } => {
                write!(f, "provider overloaded")?;
                if let Some(p) = provider {
                    write!(f, " ({p})")?;
                }
                Ok(())
            }
            Self::FileNotFound { path } => {
                write!(f, "file not found")?;
                if let Some(p) = path {
                    write!(f, ": {}", p.display())?;
                }
                Ok(())
            }
            Self::PermissionDenied { path } => {
                write!(f, "permission denied")?;
                if let Some(p) = path {
                    write!(f, ": {}", p.display())?;
                }
                Ok(())
            }
            Self::PathTypeMismatch { path, expected } => {
                write!(f, "expected {expected}")?;
                if let Some(p) = path {
                    write!(f, " at {}", p.display())?;
                }
                Ok(())
            }
            Self::SecurityBlocked { reason } => write!(f, "security blocked: {reason}"),
            Self::UnknownTool { name } => write!(f, "unknown tool: {name}"),
            Self::TbacDenied { context_id } => {
                write!(f, "TBAC denied")?;
                if let Some(id) = context_id {
                    write!(f, " (context: {id})")?;
                }
                Ok(())
            }
            Self::SchemaError { detail } => write!(f, "schema error: {detail}"),
            Self::AuthFailure { provider, detail } => {
                write!(f, "auth failure: {detail}")?;
                if let Some(p) = provider {
                    write!(f, " (provider: {p})")?;
                }
                Ok(())
            }
            Self::McpInitFailure { server, detail } => {
                write!(f, "MCP init failure: {detail}")?;
                if let Some(s) = server {
                    write!(f, " (server: {s})")?;
                }
                Ok(())
            }
            Self::Unclassified { message } => write!(f, "{message}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_timeout() {
        let kind = ToolFailureKind::classify("Request timed out after 30s");
        assert!(kind.is_transient());
        assert_eq!(kind.name(), "network_timeout");
    }

    #[test]
    fn transient_rate_limit() {
        let kind = ToolFailureKind::classify("Rate limit exceeded (429)");
        assert!(kind.is_transient());
        assert_eq!(kind.name(), "rate_limit");
    }

    #[test]
    fn transient_mcp_pool() {
        let kind = ToolFailureKind::classify("mcp pool call failed: connection dropped");
        assert!(kind.is_transient());
        assert_eq!(kind.name(), "mcp_transport");
    }

    #[test]
    fn transient_cargo_lock() {
        let kind =
            ToolFailureKind::classify("could not acquire package cache lock for .cargo-lock");
        assert!(kind.is_transient());
        assert_eq!(kind.name(), "resource_contention");
    }

    #[test]
    fn transient_overloaded() {
        let kind = ToolFailureKind::classify("Anthropic API returned 529: overloaded");
        assert!(kind.is_transient());
        assert_eq!(kind.name(), "provider_overloaded");
    }

    #[test]
    fn deterministic_not_found() {
        let kind = ToolFailureKind::classify("No such file or directory: /foo/bar");
        assert!(kind.is_deterministic());
        assert_eq!(kind.name(), "file_not_found");
    }

    #[test]
    fn deterministic_permission() {
        let kind = ToolFailureKind::classify("Permission denied: /etc/shadow");
        assert!(kind.is_deterministic());
        assert_eq!(kind.name(), "permission_denied");
    }

    #[test]
    fn deterministic_unknown_tool() {
        let kind = ToolFailureKind::classify("Unknown tool: nonexistent_tool");
        assert!(kind.is_deterministic());
        assert_eq!(kind.name(), "unknown_tool");
    }

    #[test]
    fn deterministic_auth() {
        let kind = ToolFailureKind::classify("invalid_api_key: check your credentials");
        assert!(kind.is_deterministic());
        assert_eq!(kind.name(), "auth_failure");
    }

    #[test]
    fn deterministic_tbac() {
        let kind = ToolFailureKind::classify("denied by task context: tool not in allowlist");
        assert!(kind.is_deterministic());
        assert_eq!(kind.name(), "tbac_denied");
    }

    #[test]
    fn deterministic_mcp_init() {
        let kind = ToolFailureKind::classify("MCP server is not initialized: process start failed");
        assert!(kind.is_deterministic());
        assert_eq!(kind.name(), "mcp_init_failure");
    }

    #[test]
    fn unclassified_falls_through() {
        let kind = ToolFailureKind::classify("some completely novel error nobody anticipated");
        assert!(kind.is_unknown());
        assert_eq!(kind.name(), "unclassified");
    }

    #[test]
    fn all_variants_have_names() {
        // Ensure no variant returns empty string
        let variants = vec![
            ToolFailureKind::NetworkTimeout {
                provider: None,
                timeout_ms: None,
            },
            ToolFailureKind::RateLimit {
                provider: None,
                retry_after_ms: None,
            },
            ToolFailureKind::ServerError {
                status: 500,
                provider: None,
            },
            ToolFailureKind::ConnectionError { detail: "".into() },
            ToolFailureKind::ResourceContention {
                resource: "".into(),
            },
            ToolFailureKind::McpTransport {
                server: None,
                detail: "".into(),
            },
            ToolFailureKind::ProviderOverloaded { provider: None },
            ToolFailureKind::FileNotFound { path: None },
            ToolFailureKind::PermissionDenied { path: None },
            ToolFailureKind::PathTypeMismatch {
                path: None,
                expected: "file",
            },
            ToolFailureKind::SecurityBlocked { reason: "".into() },
            ToolFailureKind::UnknownTool { name: "".into() },
            ToolFailureKind::TbacDenied { context_id: None },
            ToolFailureKind::SchemaError { detail: "".into() },
            ToolFailureKind::AuthFailure {
                provider: None,
                detail: "".into(),
            },
            ToolFailureKind::McpInitFailure {
                server: None,
                detail: "".into(),
            },
            ToolFailureKind::Unclassified { message: "".into() },
        ];
        for v in &variants {
            assert!(!v.name().is_empty(), "variant {:?} has empty name", v);
        }
    }

    #[test]
    fn display_impl_works() {
        let kind = ToolFailureKind::ServerError {
            status: 502,
            provider: Some("anthropic".to_string()),
        };
        assert_eq!(format!("{kind}"), "server error 502 (provider: anthropic)");
    }

    #[test]
    fn error_kind_bridge() {
        let transient = ToolFailureKind::classify("timeout");
        assert_eq!(
            transient.to_error_kind(),
            super::super::classifier::ErrorKind::Transient
        );

        let deterministic = ToolFailureKind::classify("permission denied");
        assert_eq!(
            deterministic.to_error_kind(),
            super::super::classifier::ErrorKind::Deterministic
        );
    }
}
