//! Three-scope MCP server configuration system.
//!
//! # Scopes (precedence: local > project > user)
//!
//! | Scope   | File path                                      | Git? | Description            |
//! |---------|------------------------------------------------|------|------------------------|
//! | user    | `~/.halcon/mcp.toml`                           | No   | Cross-project private  |
//! | project | `.halcon/mcp.toml` (in project root)           | Yes  | Team-shared            |
//! | local   | `~/.halcon/local/<project-hash>/mcp.toml`      | No   | Project-specific priv  |
//!
//! Each scope is a separate TOML file.  Entries are merged at startup;
//! name collisions resolved by precedence: local > project > user.
//!
//! # Transport
//!
//! ```toml
//! [servers.github]
//! url = "https://api.githubcopilot.com/mcp/"          # HTTP/SSE transport
//!
//! [servers.filesystem]
//! command = "npx"                                       # stdio transport
//! args    = ["@modelcontextprotocol/server-filesystem"]
//! env     = { ALLOWED_DIRS = "${HOME}/projects" }
//! ```
//!
//! # Environment-variable expansion
//!
//! `${VAR}` and `${VAR:-default}` are expanded at **connection time**, not at load time.
//! This keeps secrets out of in-memory config until they are needed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Which scope owns this server entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpScope {
    Local,
    Project,
    User,
}

impl std::fmt::Display for McpScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            McpScope::Local => write!(f, "local"),
            McpScope::Project => write!(f, "project"),
            McpScope::User => write!(f, "user"),
        }
    }
}

/// Transport discriminant for an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum McpTransport {
    /// HTTP + SSE / Streamable-HTTP transport. Requires OAuth for protected endpoints.
    Http {
        /// Server base URL (e.g. `https://api.githubcopilot.com/mcp/`).
        url: String,
    },
    /// Child-process stdio transport. The same as the existing `McpServerConfig`.
    Stdio {
        /// Executable to spawn (e.g. `npx`, `uvx`, `docker`).
        command: String,
        #[serde(default)]
        args: Vec<String>,
        /// Environment variables.  Values may contain `${VAR}` or `${VAR:-default}`.
        #[serde(default)]
        env: HashMap<String, String>,
    },
}

/// A single MCP server entry across any scope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerSpec {
    #[serde(flatten)]
    pub transport: McpTransport,
    /// Per-tool permission overrides: `"tool_name"` → `"ReadOnly"` | `"Destructive"`.
    #[serde(default)]
    pub tool_permissions: HashMap<String, String>,
    /// Which scope this entry was loaded from (populated after merge, not stored in TOML).
    #[serde(skip)]
    pub scope: Option<McpScope>,
}

/// Top-level structure of a scope TOML file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ScopeFile {
    #[serde(default)]
    servers: HashMap<String, McpServerSpec>,
}

/// Merged MCP configuration from all three scopes.
#[derive(Debug, Clone, Default)]
pub struct MergedMcpConfig {
    /// Servers indexed by name; scope precedence already applied.
    pub servers: HashMap<String, McpServerSpec>,
}

impl MergedMcpConfig {
    /// Load and merge all three scopes for `working_dir`.
    ///
    /// Precedence: local > project > user (local wins on name collision).
    pub fn load(working_dir: &Path) -> Self {
        let user_path = user_scope_path();
        let project_path = find_project_halcon_dir(working_dir).map(|d| d.join("mcp.toml"));
        let local_path = local_scope_path(working_dir);

        let user = user_path.as_deref().and_then(|p| load_scope_file(p, McpScope::User));
        let project = project_path.as_deref().and_then(|p| load_scope_file(p, McpScope::Project));
        let local = local_path.as_deref().and_then(|p| load_scope_file(p, McpScope::Local));

        let mut merged: HashMap<String, McpServerSpec> = HashMap::new();

        // Lower-precedence scopes first.
        for (name, spec) in user.into_iter().flat_map(|m| m.into_iter()) {
            merged.insert(name, spec);
        }
        for (name, spec) in project.into_iter().flat_map(|m| m.into_iter()) {
            merged.insert(name, spec);
        }
        for (name, spec) in local.into_iter().flat_map(|m| m.into_iter()) {
            merged.insert(name, spec);
        }

        MergedMcpConfig { servers: merged }
    }

    /// All servers for a given scope (for display).
    pub fn servers_for_scope(&self, scope: McpScope) -> Vec<(&str, &McpServerSpec)> {
        self.servers
            .iter()
            .filter(|(_, s)| s.scope == Some(scope))
            .map(|(n, s)| (n.as_str(), s))
            .collect()
    }
}

// ── TOML persistence ──────────────────────────────────────────────────────────

/// Add or update a server entry in the given scope file.
///
/// Creates the file and parent directories if they do not exist.
pub fn write_server(scope: McpScope, working_dir: &Path, name: &str, spec: McpServerSpec) -> std::io::Result<()> {
    let path = scope_file_path(scope, working_dir)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "cannot resolve scope path"))?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut file: ScopeFile = path.exists()
        .then(|| std::fs::read_to_string(&path).ok().and_then(|s| toml::from_str(&s).ok()))
        .flatten()
        .unwrap_or_default();

    file.servers.insert(name.to_string(), spec);

    let toml_str = toml::to_string_pretty(&file)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    std::fs::write(&path, toml_str)
}

/// Remove a server entry from the given scope file.
pub fn remove_server(scope: McpScope, working_dir: &Path, name: &str) -> std::io::Result<bool> {
    let path = scope_file_path(scope, working_dir)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "cannot resolve scope path"))?;

    if !path.exists() {
        return Ok(false);
    }

    let mut file: ScopeFile = toml::from_str(&std::fs::read_to_string(&path)?)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;

    let removed = file.servers.remove(name).is_some();
    if removed {
        let toml_str = toml::to_string_pretty(&file)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        std::fs::write(&path, toml_str)?;
    }
    Ok(removed)
}

// ── Environment-variable expansion ───────────────────────────────────────────

/// Expand `${VAR}` and `${VAR:-default}` in a string at call time.
///
/// This runs at **connection time**, not at config load time, so secrets remain
/// out of in-memory config until they are actually needed.
pub fn expand_env(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '$' && chars.peek() == Some(&'{') {
            chars.next(); // consume '{'
            let mut var = String::new();
            for ch in chars.by_ref() {
                if ch == '}' { break; }
                var.push(ch);
            }
            // Split on ":-" for default value.
            let (var_name, default) = if let Some(idx) = var.find(":-") {
                (&var[..idx], Some(&var[idx + 2..]))
            } else {
                (var.as_str(), None)
            };
            let value = std::env::var(var_name)
                .ok()
                .or_else(|| default.map(|d| d.to_string()))
                .unwrap_or_default();
            result.push_str(&value);
        } else {
            result.push(c);
        }
    }
    result
}

/// Expand all env-var references in a `McpServerSpec`'s mutable fields.
pub fn expand_spec_env(spec: &McpServerSpec) -> McpServerSpec {
    let transport = match &spec.transport {
        McpTransport::Http { url } => McpTransport::Http {
            url: expand_env(url),
        },
        McpTransport::Stdio { command, args, env } => McpTransport::Stdio {
            command: expand_env(command),
            args: args.iter().map(|a| expand_env(a)).collect(),
            env: env.iter().map(|(k, v)| (k.clone(), expand_env(v))).collect(),
        },
    };
    McpServerSpec {
        transport,
        tool_permissions: spec.tool_permissions.clone(),
        scope: spec.scope,
    }
}

// ── Path helpers ──────────────────────────────────────────────────────────────

fn scope_file_path(scope: McpScope, working_dir: &Path) -> Option<PathBuf> {
    match scope {
        McpScope::User => user_scope_path(),
        McpScope::Project => find_project_halcon_dir(working_dir).map(|d| d.join("mcp.toml")),
        McpScope::Local => local_scope_path(working_dir),
    }
}

fn user_scope_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".halcon").join("mcp.toml"))
}

fn local_scope_path(working_dir: &Path) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    // Hash the canonical working_dir to get a stable per-project key.
    let canonical = working_dir.canonicalize().unwrap_or_else(|_| working_dir.to_path_buf());
    let hash = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        canonical.hash(&mut h);
        h.finish()
    };
    Some(home.join(".halcon").join("local").join(format!("{hash:016x}")).join("mcp.toml"))
}

fn find_project_halcon_dir(working_dir: &Path) -> Option<PathBuf> {
    let mut current = working_dir;
    loop {
        let candidate = current.join(".halcon");
        if candidate.is_dir() {
            return Some(candidate);
        }
        current = current.parent()?;
    }
}

fn load_scope_file(path: &Path, scope: McpScope) -> Option<HashMap<String, McpServerSpec>> {
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(path).ok()?;
    let mut file: ScopeFile = toml::from_str(&content).ok()?;
    for spec in file.servers.values_mut() {
        spec.scope = Some(scope);
    }
    Some(file.servers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn http_spec(url: &str) -> McpServerSpec {
        McpServerSpec {
            transport: McpTransport::Http { url: url.to_string() },
            tool_permissions: HashMap::new(),
            scope: None,
        }
    }

    fn stdio_spec(cmd: &str) -> McpServerSpec {
        McpServerSpec {
            transport: McpTransport::Stdio {
                command: cmd.to_string(),
                args: vec![],
                env: HashMap::new(),
            },
            tool_permissions: HashMap::new(),
            scope: None,
        }
    }

    #[test]
    fn write_and_load_project_scope() {
        let dir = TempDir::new().unwrap();
        let halcon_dir = dir.path().join(".halcon");
        std::fs::create_dir(&halcon_dir).unwrap();

        write_server(McpScope::Project, dir.path(), "github", http_spec("https://example.com/mcp/")).unwrap();

        let merged = MergedMcpConfig::load(dir.path());
        assert!(merged.servers.contains_key("github"), "should load project server");
        assert_eq!(merged.servers["github"].scope, Some(McpScope::Project));
    }

    #[test]
    fn local_wins_over_project() {
        let dir = TempDir::new().unwrap();
        let halcon_dir = dir.path().join(".halcon");
        std::fs::create_dir(&halcon_dir).unwrap();

        write_server(McpScope::Project, dir.path(), "files", stdio_spec("npx")).unwrap();
        write_server(McpScope::Local, dir.path(), "files", stdio_spec("uvx")).unwrap();

        let merged = MergedMcpConfig::load(dir.path());
        if let McpTransport::Stdio { command, .. } = &merged.servers["files"].transport {
            assert_eq!(command, "uvx", "local scope must win over project");
        } else {
            panic!("expected stdio transport");
        }
    }

    #[test]
    fn env_var_expansion_basic() {
        std::env::set_var("TEST_MCP_URL", "https://test.example.com");
        let expanded = expand_env("${TEST_MCP_URL}/mcp/");
        assert_eq!(expanded, "https://test.example.com/mcp/");
        std::env::remove_var("TEST_MCP_URL");
    }

    #[test]
    fn env_var_expansion_with_default() {
        std::env::remove_var("NONEXISTENT_VAR_XYZ");
        let expanded = expand_env("${NONEXISTENT_VAR_XYZ:-https://fallback.example.com}");
        assert_eq!(expanded, "https://fallback.example.com");
    }

    #[test]
    fn env_var_expansion_no_braces_passthrough() {
        let s = "https://static.example.com/mcp/";
        assert_eq!(expand_env(s), s);
    }

    #[test]
    fn remove_server_from_scope() {
        let dir = TempDir::new().unwrap();
        let halcon_dir = dir.path().join(".halcon");
        std::fs::create_dir(&halcon_dir).unwrap();

        write_server(McpScope::Project, dir.path(), "github", http_spec("https://example.com/mcp/")).unwrap();
        let removed = remove_server(McpScope::Project, dir.path(), "github").unwrap();
        assert!(removed);

        let merged = MergedMcpConfig::load(dir.path());
        assert!(!merged.servers.contains_key("github"), "server should be removed");
    }

    #[test]
    fn serialize_http_spec_roundtrip() {
        let spec = http_spec("https://api.example.com/mcp/");
        let toml_str = toml::to_string(&spec).unwrap();
        let parsed: McpServerSpec = toml::from_str(&toml_str).unwrap();
        if let McpTransport::Http { url } = parsed.transport {
            assert_eq!(url, "https://api.example.com/mcp/");
        } else {
            panic!("wrong transport type");
        }
    }

    #[test]
    fn serialize_stdio_spec_roundtrip() {
        let mut env = HashMap::new();
        env.insert("PATH".to_string(), "/usr/bin".to_string());
        let spec = McpServerSpec {
            transport: McpTransport::Stdio {
                command: "npx".to_string(),
                args: vec!["--yes".to_string(), "@modelcontextprotocol/server-filesystem".to_string()],
                env,
            },
            tool_permissions: HashMap::new(),
            scope: None,
        };
        let toml_str = toml::to_string(&spec).unwrap();
        let parsed: McpServerSpec = toml::from_str(&toml_str).unwrap();
        if let McpTransport::Stdio { command, args, .. } = parsed.transport {
            assert_eq!(command, "npx");
            assert_eq!(args[1], "@modelcontextprotocol/server-filesystem");
        } else {
            panic!("wrong transport type");
        }
    }
}
