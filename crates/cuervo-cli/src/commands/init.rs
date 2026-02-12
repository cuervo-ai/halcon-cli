use anyhow::Result;
use std::path::Path;

/// Initialize Cuervo in the current project directory.
///
/// Creates a `.cuervo/` directory with a project-level config file.
pub async fn run(force: bool) -> Result<()> {
    let project_dir = Path::new(".cuervo");

    if project_dir.exists() && !force {
        println!("Cuervo is already initialized in this directory.");
        println!("Use --force to re-initialize.");
        return Ok(());
    }

    std::fs::create_dir_all(project_dir)?;

    let config_path = project_dir.join("config.toml");
    if !config_path.exists() || force {
        let default_config = r#"# Cuervo project configuration
# Values here override global config (~/.cuervo/config.toml)

[general]
# default_provider = "anthropic"
# default_model = "claude-sonnet-4-5-20250929"

[tools]
# allowed_directories = ["."]
# confirm_destructive = true

[security]
# pii_detection = true
"#;
        std::fs::write(&config_path, default_config)?;
        println!("Initialized Cuervo in .cuervo/");
        println!("Config: {}", config_path.display());
    }

    Ok(())
}
