use anyhow::Result;
use cuervo_core::types::AppConfig;

use crate::config_loader;

/// Show the full configuration.
pub fn show(config: &AppConfig) -> Result<()> {
    let toml_str =
        toml::to_string_pretty(config).map_err(|e| anyhow::anyhow!("serialize config: {e}"))?;
    println!("{toml_str}");
    Ok(())
}

/// Get a specific configuration value by dot-separated key.
pub fn get(config: &AppConfig, key: &str) -> Result<()> {
    // Serialize to toml::Value for dynamic key lookup
    let value =
        toml::Value::try_from(config).map_err(|e| anyhow::anyhow!("serialize config: {e}"))?;

    let result = key.split('.').try_fold(&value, |acc, part| acc.get(part));

    match result {
        Some(v) => println!("{v}"),
        None => println!("Key '{key}' not found"),
    }
    Ok(())
}

/// Set a configuration value (writes to project-level config).
pub fn set(key: &str, value: &str) -> Result<()> {
    println!("Setting {key} = {value}");
    println!("(Config write coming in Sprint 7)");
    let _ = (key, value);
    Ok(())
}

/// Show the configuration file paths.
pub fn path() -> Result<()> {
    let global = config_loader::global_config_path();
    let project = config_loader::project_config_path();
    println!("Global:  {}", global.display());
    println!("Project: {}", project.display());
    Ok(())
}
