use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Persistent application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Server connection URL.
    pub server_url: String,
    /// Authentication token.
    pub auth_token: String,
    /// Dark mode enabled.
    pub dark_mode: bool,
    /// Metric polling interval in seconds.
    pub poll_interval_secs: u64,
    /// Maximum log entries to retain in memory.
    pub max_log_entries: usize,
    /// Maximum events to retain in memory.
    pub max_events: usize,
    /// Window width.
    pub window_width: f32,
    /// Window height.
    pub window_height: f32,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server_url: std::env::var("CUERVO_SERVER_URL").unwrap_or_else(|_| {
                format!(
                    "http://{}:{}",
                    cuervo_api::DEFAULT_BIND,
                    cuervo_api::DEFAULT_PORT
                )
            }),
            auth_token: std::env::var("CUERVO_API_TOKEN").unwrap_or_default(),
            dark_mode: true,
            poll_interval_secs: 5,
            max_log_entries: 10_000,
            max_events: 5_000,
            window_width: 1280.0,
            window_height: 800.0,
        }
    }
}

impl AppConfig {
    /// Returns true if both server_url and auth_token are non-empty
    /// (set via env vars), enabling auto-connect on startup.
    pub fn has_auto_connect(&self) -> bool {
        !self.auth_token.is_empty()
    }

    /// Path to the desktop config file: `~/.cuervo/desktop.toml`.
    pub fn config_path() -> Option<PathBuf> {
        #[cfg(target_os = "windows")]
        let home = std::env::var("USERPROFILE").ok();
        #[cfg(not(target_os = "windows"))]
        let home = std::env::var("HOME").ok();

        home.map(|h| PathBuf::from(h).join(".cuervo").join("desktop.toml"))
    }

    /// Load config from `~/.cuervo/desktop.toml`, falling back to defaults.
    /// Environment variables still override persisted values for server_url and auth_token.
    pub fn load() -> Self {
        let mut config = if let Some(path) = Self::config_path() {
            if path.exists() {
                match std::fs::read_to_string(&path) {
                    Ok(contents) => match toml::from_str::<AppConfig>(&contents) {
                        Ok(c) => c,
                        Err(e) => {
                            tracing::warn!(error = %e, "failed to parse desktop config, using defaults");
                            AppConfig::default()
                        }
                    },
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to read desktop config, using defaults");
                        AppConfig::default()
                    }
                }
            } else {
                AppConfig::default()
            }
        } else {
            AppConfig::default()
        };

        // Env vars override persisted values.
        if let Ok(url) = std::env::var("CUERVO_SERVER_URL") {
            config.server_url = url;
        }
        if let Ok(token) = std::env::var("CUERVO_API_TOKEN") {
            config.auth_token = token;
        }

        config
    }

    /// Save config to `~/.cuervo/desktop.toml`.
    #[allow(dead_code)]
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let path = Self::config_path().ok_or("could not determine config path")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let contents = toml::to_string_pretty(self)?;
        std::fs::write(&path, contents)?;
        Ok(())
    }
}
