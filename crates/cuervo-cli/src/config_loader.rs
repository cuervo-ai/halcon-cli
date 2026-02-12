use std::path::PathBuf;

use cuervo_core::types::AppConfig;

/// Load configuration with layered merging:
/// 1. Built-in defaults (AppConfig::default())
/// 2. Global config (~/.cuervo/config.toml)
/// 3. Project config (.cuervo/config.toml)
/// 4. Explicit config file (--config flag)
/// 5. Environment variables (CUERVO_*)
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

    Ok(config)
}

/// Global config path: ~/.cuervo/config.toml
pub fn global_config_path() -> PathBuf {
    dirs_path().join("config.toml")
}

/// Project config path: .cuervo/config.toml
pub fn project_config_path() -> PathBuf {
    PathBuf::from(".cuervo/config.toml")
}

/// Cuervo data directory: ~/.cuervo/
fn dirs_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cuervo")
}

/// Default database path: ~/.cuervo/cuervo.db
pub fn default_db_path() -> PathBuf {
    dirs_path().join("cuervo.db")
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
    if let Ok(val) = std::env::var("CUERVO_DEFAULT_PROVIDER") {
        config.general.default_provider = val;
    }
    if let Ok(val) = std::env::var("CUERVO_DEFAULT_MODEL") {
        config.general.default_model = val;
    }
    if let Ok(val) = std::env::var("CUERVO_MAX_TOKENS") {
        if let Ok(n) = val.parse() {
            config.general.max_tokens = n;
        }
    }
    if let Ok(val) = std::env::var("CUERVO_TEMPERATURE") {
        if let Ok(n) = val.parse() {
            config.general.temperature = n;
        }
    }
    if let Ok(val) = std::env::var("CUERVO_LOG_LEVEL") {
        config.logging.level = val;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialize tests that read/write process-global env vars.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn default_config_loads() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::remove_var("CUERVO_DEFAULT_PROVIDER");
        std::env::remove_var("CUERVO_DEFAULT_MODEL");
        // load_config reads ~/.cuervo/config.toml if it exists, so the
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
        std::env::set_var("CUERVO_DEFAULT_PROVIDER", "ollama");
        let mut config = AppConfig::default();
        apply_env_overrides(&mut config);
        assert_eq!(config.general.default_provider, "ollama");
        std::env::remove_var("CUERVO_DEFAULT_PROVIDER");
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
