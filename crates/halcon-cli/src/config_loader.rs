use std::collections::HashMap;
use std::path::{Path, PathBuf};

use halcon_core::types::{AppConfig, McpServerConfig};

// ── Filesystem safety utilities ─────────────────────────────────────────────

/// Guard against running as root / under `sudo`.
///
/// Running as root causes all created files to be owned by root, which breaks
/// subsequent non-root runs. This is the #1 cause of "Permission denied"
/// cascading failures.
///
/// **Behavior:**
/// - `HALCON_ALLOW_ROOT=1` → skip the guard entirely (for Docker/systemd use cases).
/// - Running via `sudo` (SUDO_USER set) → warn with actionable fix.
/// - Running as root directly → warn.
///
/// Never blocks startup — the warning is informational. The real protection comes
/// from `safe_write_file`'s ownership pre-check and XDG fallback.
pub fn warn_if_sudo() {
    #[cfg(unix)]
    {
        let euid = unsafe { libc::geteuid() };
        if euid != 0 {
            return;
        }

        // Escape hatch: HALCON_ALLOW_ROOT=1 silences the warning.
        // Used in Docker containers and systemd services where root is expected.
        if std::env::var("HALCON_ALLOW_ROOT")
            .map(|v| v == "1")
            .unwrap_or(false)
        {
            tracing::debug!("Running as root with HALCON_ALLOW_ROOT=1 — guard suppressed");
            return;
        }

        let dir_display = dirs_path().display().to_string();

        if let Ok(real_user) = std::env::var("SUDO_USER") {
            eprintln!();
            eprintln!("WARNING: halcon is running as root (via sudo). Files created this session");
            eprintln!("  will be owned by root, which will cause 'Permission denied' errors");
            eprintln!("  when you next run halcon as '{real_user}'.");
            eprintln!();
            eprintln!("  Options:");
            eprintln!("    1. Run without sudo:  halcon chat");
            eprintln!("    2. Fix existing files: sudo chown -R {real_user} {dir_display}");
            eprintln!("    3. Silence this:      HALCON_ALLOW_ROOT=1 halcon chat");
            eprintln!();
            tracing::warn!(
                real_user = %real_user,
                halcon_dir = %dir_display,
                "Running under sudo — files will be root-owned"
            );
        } else {
            // Running as root directly (not via sudo) — likely Docker or systemd.
            tracing::info!(
                "Running as root (no SUDO_USER). Set HALCON_ALLOW_ROOT=1 to suppress warnings."
            );
        }
    }
}

/// Check that `~/.halcon/` and its critical files are writable by the current user.
///
/// Detects root-owned files left behind by accidental `sudo halcon` invocations.
/// Prints an actionable fix to stderr. Best-effort: never errors or blocks startup.
pub fn check_data_dir_permissions() {
    let halcon_dir = dirs_path();
    if !halcon_dir.exists() {
        return;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let current_uid = unsafe { libc::getuid() };

        // Don't warn when running as root — root can write to anything.
        if current_uid == 0 {
            return;
        }

        // Critical files that halcon must be able to write during normal operation.
        let critical_paths = [
            halcon_dir.join("config.toml"),
            halcon_dir.join("cenzontle-models.json"),
            halcon_dir.join("workspace-trust.json"),
            halcon_dir.join("config-trust.json"),
            halcon_dir.join("mcp-trust.json"),
        ];

        let mut root_owned: Vec<String> = Vec::new();

        // Check the directory itself.
        if let Ok(meta) = std::fs::metadata(&halcon_dir) {
            if meta.uid() == 0 {
                root_owned.push(halcon_dir.display().to_string());
            }
        }

        for path in &critical_paths {
            if let Ok(meta) = std::fs::metadata(path) {
                if meta.uid() == 0 {
                    root_owned.push(path.display().to_string());
                }
            }
        }

        if root_owned.is_empty() {
            return;
        }

        let dir_display = halcon_dir.display();
        eprintln!();
        eprintln!(
            "WARNING: {} file(s) in {dir_display} are owned by root.",
            root_owned.len()
        );
        eprintln!("  This causes 'Permission denied' errors when writing config and cache files,");
        eprintln!("  which can make providers appear unavailable.");
        eprintln!();
        eprintln!("  Fix with:");
        eprintln!("    sudo chown -R $(whoami) {dir_display}");
        eprintln!();
        tracing::warn!(
            count = root_owned.len(),
            dir = %dir_display,
            files = ?root_owned,
            "~/.halcon/ contains root-owned files — run: sudo chown -R $(whoami) {dir_display}"
        );
    }
}

/// Result type for [`safe_write_file`].
#[derive(Debug)]
pub enum WriteResult {
    /// File written successfully.
    Ok,
    /// Primary path failed but file was written to a fallback location.
    /// The caller should continue normally — data is persisted.
    /// Inspired by Xiyo's `createFallbackStorage(primary, secondary)` pattern.
    FallbackUsed {
        primary_path: PathBuf,
        fallback_path: PathBuf,
        reason: String,
    },
    /// Write failed due to permission denied (root-owned file or directory).
    PermissionDenied {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Write failed for a non-permission reason (disk full, I/O error, etc.).
    OtherError {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl WriteResult {
    /// Returns `true` if the data was successfully persisted (either primary or fallback).
    pub fn is_ok(&self) -> bool {
        matches!(self, WriteResult::Ok | WriteResult::FallbackUsed { .. })
    }

    /// Log the error at warn level with an actionable fix hint.
    pub fn log_on_failure(&self, context: &str) {
        match self {
            WriteResult::Ok => {}
            WriteResult::FallbackUsed {
                primary_path,
                fallback_path,
                reason,
            } => {
                tracing::warn!(
                    primary = %primary_path.display(),
                    fallback = %fallback_path.display(),
                    reason = reason,
                    context = context,
                    "Write used fallback location (primary unwritable). \
                     Fix: sudo chown -R $(whoami) {}",
                    primary_path.parent().map(|p| p.display().to_string()).unwrap_or_default()
                );
            }
            WriteResult::PermissionDenied { path, source } => {
                tracing::warn!(
                    error = %source,
                    path = %path.display(),
                    context = context,
                    "Permission denied writing file. \
                     Fix: sudo chown $(whoami) {}",
                    path.display()
                );
            }
            WriteResult::OtherError { path, source } => {
                tracing::warn!(
                    error = %source,
                    path = %path.display(),
                    context = context,
                    "Failed to write file"
                );
            }
        }
    }
}

/// XDG-compliant fallback path for when `~/.halcon/` is unwritable.
///
/// Returns `$XDG_DATA_HOME/halcon/<filename>` (typically `~/.local/share/halcon/<filename>`).
/// This mirrors the credential store fallback in halcon-auth/file_store.rs.
pub fn xdg_fallback_path(filename: &str) -> Option<PathBuf> {
    let base = std::env::var("XDG_DATA_HOME")
        .ok()
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("share")))?;
    Some(base.join("halcon").join(filename))
}

/// Safely write `content` to `path` using atomic tmp+rename with correct permissions.
///
/// Guarantees:
/// - **Atomic**: readers never see a partial file (write to `.tmp`, then `rename`).
/// - **Permission-safe**: checks ownership before writing; returns `PermissionDenied`
///   with an actionable message instead of a cryptic OS error.
/// - **Secure**: file is created with mode `0600` (owner rw only) on Unix.
/// - **Directory creation**: parent directory is created with `0700` if missing.
/// - **Self-healing fallback**: when the primary path is unwritable (root-owned),
///   automatically writes to XDG fallback (`~/.local/share/halcon/`) and returns
///   `FallbackUsed` so the caller knows data is persisted (inspired by Xiyo's
///   `createFallbackStorage(primary, secondary)` pattern).
///
/// This function is the single chokepoint for all config/cache writes in halcon-cli.
/// Use it instead of raw `std::fs::write` for any file in `~/.halcon/`.
pub fn safe_write_file(path: &Path, content: &[u8]) -> WriteResult {
    let result = atomic_write_inner(path, content);

    // Xiyo-inspired fallback: if the primary path fails with PermissionDenied,
    // try writing to the XDG data directory instead. This ensures the CLI keeps
    // working even when ~/.halcon/ is root-owned — the user can fix ownership
    // later without losing their session.
    if let WriteResult::PermissionDenied { .. } = &result {
        if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
            if let Some(fallback) = xdg_fallback_path(filename) {
                // Don't fallback to the same path.
                if fallback != path {
                    let fb_result = atomic_write_inner(&fallback, content);
                    if fb_result.is_ok() {
                        return WriteResult::FallbackUsed {
                            primary_path: path.to_path_buf(),
                            fallback_path: fallback,
                            reason: "primary path owned by root".to_string(),
                        };
                    }
                }
            }
        }
    }

    result
}

/// Core atomic write: tmp file → chmod 0600 → rename. No fallback logic.
fn atomic_write_inner(path: &Path, content: &[u8]) -> WriteResult {
    // 1. Ensure parent directory exists.
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return classify_io_error(path, e);
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
            }
        }
    }

    // 2. Pre-flight ownership check (Unix only): detect root-owned files early
    //    so we return PermissionDenied with an actionable message.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let current_uid = unsafe { libc::getuid() };
        if current_uid != 0 {
            if let Ok(meta) = std::fs::metadata(path) {
                if meta.uid() == 0 {
                    return WriteResult::PermissionDenied {
                        path: path.to_path_buf(),
                        source: std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            format!(
                                "file owned by root (uid 0), current uid is {current_uid}. \
                                 Fix: sudo chown $(whoami) {}",
                                path.display()
                            ),
                        ),
                    };
                }
            }
            if let Some(parent) = path.parent() {
                if let Ok(meta) = std::fs::metadata(parent) {
                    if meta.uid() == 0 {
                        return WriteResult::PermissionDenied {
                            path: path.to_path_buf(),
                            source: std::io::Error::new(
                                std::io::ErrorKind::PermissionDenied,
                                format!(
                                    "directory {} owned by root. \
                                     Fix: sudo chown -R $(whoami) {}",
                                    parent.display(),
                                    parent.display()
                                ),
                            ),
                        };
                    }
                }
            }
        }
    }

    // 3. Write to a sibling tmp file (atomic — readers never see partial content).
    let tmp = path.with_extension(format!(
        "tmp.{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
    ));

    if let Err(e) = std::fs::write(&tmp, content) {
        let _ = std::fs::remove_file(&tmp);
        return classify_io_error(path, e);
    }

    // 4. Set permissions to 0600 BEFORE rename (file is never world-readable).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)) {
            let _ = std::fs::remove_file(&tmp);
            return WriteResult::OtherError {
                path: path.to_path_buf(),
                source: e,
            };
        }
    }

    // 5. Atomic rename.
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return classify_io_error(path, e);
    }

    WriteResult::Ok
}

/// Classify an IO error into the appropriate WriteResult variant.
fn classify_io_error(path: &Path, e: std::io::Error) -> WriteResult {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        WriteResult::PermissionDenied {
            path: path.to_path_buf(),
            source: e,
        }
    } else {
        WriteResult::OtherError {
            path: path.to_path_buf(),
            source: e,
        }
    }
}

/// Read a file from the primary path, falling back to the XDG data directory.
///
/// This is the read counterpart to [`safe_write_file`]'s fallback behavior.
/// When `safe_write_file` writes to `~/.local/share/halcon/<file>` because
/// `~/.halcon/<file>` is unwritable, `safe_read_file` checks both locations.
///
/// Priority: primary path first, then XDG fallback.
pub fn safe_read_file(path: &Path) -> Option<Vec<u8>> {
    // Try primary path first.
    if let Ok(content) = std::fs::read(path) {
        return Some(content);
    }
    // Try XDG fallback.
    if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
        if let Some(fallback) = xdg_fallback_path(filename) {
            if fallback != path {
                if let Ok(content) = std::fs::read(&fallback) {
                    tracing::debug!(
                        primary = %path.display(),
                        fallback = %fallback.display(),
                        "Read file from XDG fallback (primary unreadable)"
                    );
                    return Some(content);
                }
            }
        }
    }
    None
}

/// Check whether the current process can write to `path` (or its parent directory).
///
/// Returns `true` if the write is expected to succeed. Returns `false` with a
/// structured log when a permission issue is detected.
///
/// This is a cheap pre-flight check — it does NOT guarantee the write will succeed
/// (the filesystem can change between check and write), but it catches the common
/// case of root-owned files without attempting a write.
#[cfg(unix)]
pub fn is_writable(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    let current_uid = unsafe { libc::getuid() };
    if current_uid == 0 {
        return true; // root can write anything
    }

    // Check the file itself (if it exists).
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.uid() == 0 {
            tracing::debug!(
                path = %path.display(),
                file_uid = 0,
                current_uid = current_uid,
                "Pre-flight write check failed: file owned by root"
            );
            return false;
        }
    }

    // Check the parent directory.
    if let Some(parent) = path.parent() {
        if let Ok(meta) = std::fs::metadata(parent) {
            if meta.uid() == 0 {
                tracing::debug!(
                    path = %path.display(),
                    dir = %parent.display(),
                    dir_uid = 0,
                    current_uid = current_uid,
                    "Pre-flight write check failed: directory owned by root"
                );
                return false;
            }
        }
    }

    true
}

#[cfg(not(unix))]
pub fn is_writable(_path: &Path) -> bool {
    true
}

/// Migrates ~/.cuervo/ → ~/.halcon/ on first Halcon run (legacy user migration).
pub fn migrate_legacy_dir() {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let old = home.join(".cuervo");
    let new = home.join(".halcon");
    if old.exists() && !new.exists() && std::fs::rename(&old, &new).is_ok() {
        let _ = std::fs::rename(new.join("cuervo.db"), new.join("halcon.db"));
        let _ = std::fs::rename(new.join("CUERVO.md"), new.join("HALCON.md"));
        tracing::info!("Migrated ~/.cuervo/ → ~/.halcon/");
    }
}

/// Load configuration with layered merging:
/// 1. Built-in defaults (AppConfig::default())
/// 2. Global config (~/.halcon/config.toml)
/// 3. Project config (.halcon/config.toml)
/// 4. Explicit config file (--config flag)
/// 5. Environment variables (HALCON_*)
pub fn load_config(explicit_path: Option<&str>) -> Result<AppConfig, anyhow::Error> {
    let mut config = AppConfig::default();

    // Layer 2: Global config
    let global = global_config_path();
    if global.exists() {
        let content = std::fs::read_to_string(&global)?;
        let global_config: toml::Value = toml::from_str(&content)?;
        merge_toml_into_config(&mut config, &global_config);
        tracing::debug!("Loaded global config from {}", global.display());
    }

    // Layer 3: Project config
    let project = project_config_path();
    if project.exists() {
        let content = std::fs::read_to_string(&project)?;
        let project_config: toml::Value = toml::from_str(&content)?;
        merge_toml_into_config(&mut config, &project_config);
        tracing::debug!("Loaded project config from {}", project.display());
    }

    // Layer 4: Explicit config file
    if let Some(path) = explicit_path {
        let content = std::fs::read_to_string(path)?;
        let explicit_config: toml::Value = toml::from_str(&content)?;
        merge_toml_into_config(&mut config, &explicit_config);
        tracing::debug!("Loaded explicit config from {path}");
    }

    // Layer 5: Environment variable overrides
    apply_env_overrides(&mut config);

    // Layer 6: .mcp.json auto-discovery (additive merge, highest priority for MCP servers)
    load_mcp_json(&mut config);

    Ok(config)
}

/// Global config path: ~/.halcon/config.toml
pub fn global_config_path() -> PathBuf {
    dirs_path().join("config.toml")
}

/// Project config path: .halcon/config.toml
pub fn project_config_path() -> PathBuf {
    PathBuf::from(".halcon/config.toml")
}

/// Halcon data directory: ~/.halcon/
fn dirs_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".halcon")
}

/// Default database path: ~/.halcon/halcon.db
pub fn default_db_path() -> PathBuf {
    dirs_path().join("halcon.db")
}

/// Merge a TOML value tree into an existing AppConfig.
///
/// This does a shallow merge at the section level: if a section
/// exists in the overlay, it fully replaces the section in config
/// (re-deserialized from the merged TOML).
fn merge_toml_into_config(config: &mut AppConfig, overlay: &toml::Value) {
    // Serialize current config to toml::Value
    let mut base = match toml::Value::try_from(&*config) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("Config serialization failed during merge: {e}");
            return;
        }
    };

    // Deep merge overlay into base
    if let (Some(base_table), Some(overlay_table)) = (base.as_table_mut(), overlay.as_table()) {
        deep_merge(base_table, overlay_table);
    }

    // Deserialize back
    match base.try_into::<AppConfig>() {
        Ok(merged) => *config = merged,
        Err(e) => {
            tracing::warn!("Config merge deserialization failed (overlay ignored): {e}");
        }
    }
}

fn deep_merge(
    base: &mut toml::map::Map<String, toml::Value>,
    overlay: &toml::map::Map<String, toml::Value>,
) {
    for (key, value) in overlay {
        match (base.get_mut(key), value) {
            (Some(toml::Value::Table(base_table)), toml::Value::Table(overlay_table)) => {
                deep_merge(base_table, overlay_table);
            }
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

/// Apply environment variable overrides.
fn apply_env_overrides(config: &mut AppConfig) {
    if let Ok(val) = std::env::var("HALCON_DEFAULT_PROVIDER") {
        config.general.default_provider = val;
    }
    if let Ok(val) = std::env::var("HALCON_DEFAULT_MODEL") {
        config.general.default_model = val;
    }
    if let Ok(val) = std::env::var("HALCON_MAX_TOKENS") {
        if let Ok(n) = val.parse() {
            config.general.max_tokens = n;
        }
    }
    if let Ok(val) = std::env::var("HALCON_TEMPERATURE") {
        if let Ok(n) = val.parse() {
            config.general.temperature = n;
        }
    }
    if let Ok(val) = std::env::var("HALCON_LOG_LEVEL") {
        config.logging.level = val;
    }
}

/// Load MCP server configurations from `.mcp.json` files.
///
/// Search order (all merged additively, later files override earlier for same server name):
/// 1. `./.mcp.json` (project root)
/// 2. `.halcon/.mcp.json` (project config dir)
/// 3. `~/.halcon/.mcp.json` (global user config)
fn load_mcp_json(config: &mut AppConfig) {
    let paths = [
        PathBuf::from(".mcp.json"),
        PathBuf::from(".halcon/.mcp.json"),
        dirs_path().join(".mcp.json"),
    ];

    for path in &paths {
        if !path.exists() {
            continue;
        }
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to read {}: {e}", path.display());
                continue;
            }
        };
        match parse_mcp_json(&content) {
            Ok(servers) => {
                let count = servers.len();
                for (name, server_config) in servers {
                    config.mcp.servers.entry(name).or_insert(server_config);
                }
                tracing::debug!("Loaded {count} MCP servers from {}", path.display());
            }
            Err(e) => {
                tracing::warn!("Failed to parse {}: {e}", path.display());
            }
        }
    }
}

/// Parse a `.mcp.json` file content into a map of server configurations.
///
/// Supports two formats:
/// 1. Claude/Cursor format: `{"mcpServers": {"name": {"command": "...", ...}}}`
/// 2. Direct format: `{"name": {"command": "...", ...}}`
fn parse_mcp_json(content: &str) -> Result<HashMap<String, McpServerConfig>, anyhow::Error> {
    let root: serde_json::Value = serde_json::from_str(content)?;

    let servers_obj = if let Some(mcp_servers) = root.get("mcpServers") {
        mcp_servers
    } else {
        &root
    };

    let map = servers_obj
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("expected JSON object"))?;

    let mut result = HashMap::new();
    for (name, value) in map {
        let command = value
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("server '{name}' missing 'command' field"))?
            .to_string();

        let args: Vec<String> = value
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let env: HashMap<String, String> = value
            .get("env")
            .and_then(|v| v.as_object())
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), expand_env_value(s))))
                    .collect()
            })
            .unwrap_or_default();

        let tool_permissions: HashMap<String, String> = value
            .get("tool_permissions")
            .and_then(|v| v.as_object())
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();

        result.insert(
            name.clone(),
            McpServerConfig {
                command,
                args,
                env,
                tool_permissions,
            },
        );
    }

    Ok(result)
}

/// Expand `${VAR}` references in a string with environment variable values.
///
/// Unset variables are replaced with an empty string.
fn expand_env_value(val: &str) -> String {
    let mut result = val.to_string();
    // Find all ${...} patterns and replace.
    while let Some(start) = result.find("${") {
        if let Some(end) = result[start..].find('}') {
            let var_name = &result[start + 2..start + end];
            let replacement = std::env::var(var_name).unwrap_or_default();
            result = format!(
                "{}{}{}",
                &result[..start],
                replacement,
                &result[start + end + 1..]
            );
        } else {
            break; // Malformed pattern, stop.
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialize tests that read/write process-global env vars.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // ── safe_write_file tests ────────────────────────────────────────────────

    #[test]
    fn safe_write_file_creates_file_and_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("subdir").join("nested").join("test.json");
        let result = safe_write_file(&path, b"hello world");
        assert!(result.is_ok(), "safe_write_file should succeed");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello world");
    }

    #[test]
    fn safe_write_file_overwrites_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("overwrite.txt");
        safe_write_file(&path, b"first");
        safe_write_file(&path, b"second");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
    }

    #[test]
    fn safe_write_file_atomic_no_tmp_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("atomic.json");
        safe_write_file(&path, b"{\"ok\":true}");
        // No .tmp files should remain after a successful write.
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .map(|n| n.contains(".tmp"))
                    .unwrap_or(false)
            })
            .collect();
        assert!(entries.is_empty(), "tmp files should be cleaned up");
    }

    #[cfg(unix)]
    #[test]
    fn safe_write_file_sets_0600_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secure.txt");
        safe_write_file(&path, b"secret");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "file should be 0600, got {:o}",
            mode & 0o777
        );
    }

    #[cfg(unix)]
    #[test]
    fn safe_write_file_creates_parent_with_0700() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("new_subdir");
        let path = subdir.join("file.txt");
        safe_write_file(&path, b"content");
        let mode = std::fs::metadata(&subdir).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o700,
            "directory should be 0700, got {:o}",
            mode & 0o777
        );
    }

    #[cfg(unix)]
    #[test]
    fn safe_write_file_permission_denied_on_readonly_dir() {
        // Root can write anywhere, so skip this test when running as root.
        let euid = unsafe { libc::geteuid() };
        if euid == 0 {
            eprintln!("Skipping permission test (running as root)");
            return;
        }

        // Create a read-only directory so the write fails.
        let dir = tempfile::tempdir().unwrap();
        let ro_dir = dir.path().join("readonly");
        std::fs::create_dir(&ro_dir).unwrap();

        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ro_dir, std::fs::Permissions::from_mode(0o555)).unwrap();

        let path = ro_dir.join("should_fail.txt");
        let result = safe_write_file(&path, b"content");

        // Restore permissions so tempdir cleanup succeeds.
        std::fs::set_permissions(&ro_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(!result.is_ok(), "write to read-only dir should fail");
    }

    #[test]
    fn write_result_log_on_failure_noop_on_ok() {
        // Just verify log_on_failure doesn't panic on Ok.
        WriteResult::Ok.log_on_failure("test");
    }

    #[test]
    fn is_writable_returns_true_for_user_owned_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("user_file.txt");
        std::fs::write(&path, "content").unwrap();
        assert!(is_writable(&path));
    }

    #[test]
    fn is_writable_returns_true_for_nonexistent_file_in_user_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doesnt_exist.txt");
        assert!(is_writable(&path));
    }

    #[test]
    fn default_config_loads() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::remove_var("HALCON_DEFAULT_PROVIDER");
        std::env::remove_var("HALCON_DEFAULT_MODEL");
        // load_config reads ~/.halcon/config.toml if it exists, so the
        // default_provider may differ from AppConfig::default().
        // We only verify that load_config succeeds and returns a valid config.
        let config = load_config(None).unwrap();
        assert!(!config.general.default_provider.is_empty());
    }

    #[test]
    fn toml_overlay_merges() {
        let mut config = AppConfig::default();
        let overlay: toml::Value = toml::from_str(
            r#"
            [general]
            default_model = "llama3.2"
            "#,
        )
        .unwrap();

        merge_toml_into_config(&mut config, &overlay);
        assert_eq!(config.general.default_model, "llama3.2");
        assert_eq!(config.general.default_provider, "anthropic");
    }

    #[test]
    fn env_override_applies() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var("HALCON_DEFAULT_PROVIDER", "ollama");
        let mut config = AppConfig::default();
        apply_env_overrides(&mut config);
        assert_eq!(config.general.default_provider, "ollama");
        std::env::remove_var("HALCON_DEFAULT_PROVIDER");
    }

    // --- .mcp.json tests ---

    #[test]
    fn parse_mcp_json_mcpservers_format() {
        let json = r#"{
            "mcpServers": {
                "github": {
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-github"],
                    "env": { "TOKEN": "abc123" }
                }
            }
        }"#;
        let servers = parse_mcp_json(json).unwrap();
        assert_eq!(servers.len(), 1);
        let github = &servers["github"];
        assert_eq!(github.command, "npx");
        assert_eq!(
            github.args,
            vec!["-y", "@modelcontextprotocol/server-github"]
        );
        assert_eq!(github.env.get("TOKEN").unwrap(), "abc123");
    }

    #[test]
    fn parse_mcp_json_direct_format() {
        let json = r#"{
            "filesystem": {
                "command": "mcp-server-filesystem",
                "args": ["--root", "/tmp"]
            }
        }"#;
        let servers = parse_mcp_json(json).unwrap();
        assert_eq!(servers.len(), 1);
        let fs = &servers["filesystem"];
        assert_eq!(fs.command, "mcp-server-filesystem");
        assert_eq!(fs.args, vec!["--root", "/tmp"]);
    }

    #[test]
    fn parse_mcp_json_merge_preserves_existing() {
        let mut config = AppConfig::default();
        config.mcp.servers.insert(
            "existing".to_string(),
            McpServerConfig {
                command: "existing-cmd".to_string(),
                args: vec![],
                env: HashMap::new(),
                tool_permissions: HashMap::new(),
            },
        );

        // Simulate merge: entry() with or_insert preserves existing.
        let new_servers = parse_mcp_json(r#"{"new_server": {"command": "new-cmd"}}"#).unwrap();
        for (name, server_config) in new_servers {
            config.mcp.servers.entry(name).or_insert(server_config);
        }

        assert_eq!(config.mcp.servers.len(), 2);
        assert_eq!(config.mcp.servers["existing"].command, "existing-cmd");
        assert_eq!(config.mcp.servers["new_server"].command, "new-cmd");
    }

    #[test]
    fn expand_env_value_replaces_variables() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var("HALCON_TEST_TOKEN", "secret123");
        let result = expand_env_value("Bearer ${HALCON_TEST_TOKEN}");
        assert_eq!(result, "Bearer secret123");
        std::env::remove_var("HALCON_TEST_TOKEN");
    }

    #[test]
    fn expand_env_value_missing_var_empty() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::remove_var("HALCON_NONEXISTENT_VAR_42");
        let result = expand_env_value("prefix_${HALCON_NONEXISTENT_VAR_42}_suffix");
        assert_eq!(result, "prefix__suffix");
    }

    #[test]
    fn load_mcp_json_nonexistent_paths_noop() {
        let mut config = AppConfig::default();
        let before = config.mcp.servers.len();
        // load_mcp_json tries paths that don't exist in the test cwd — should be a no-op.
        load_mcp_json(&mut config);
        // May or may not find files depending on test environment.
        // At minimum, it should not error.
        assert!(config.mcp.servers.len() >= before);
    }

    #[test]
    fn toml_overlay_merge_with_unknown_fields() {
        // Unknown sections in overlay should not break the merge —
        // they are silently dropped during deserialization back to AppConfig.
        let mut config = AppConfig::default();
        let overlay: toml::Value = toml::from_str(
            r#"
            [general]
            default_model = "gpt-4o"

            [unknown_section]
            foo = "bar"
            "#,
        )
        .unwrap();

        merge_toml_into_config(&mut config, &overlay);
        // Known field merged successfully.
        assert_eq!(config.general.default_model, "gpt-4o");
        // Provider unchanged (not in overlay).
        assert_eq!(config.general.default_provider, "anthropic");
    }
}
