use anyhow::Result;
use cuervo_core::types::AppConfig;

use super::auth;

/// Show current Cuervo status.
pub async fn run(config: &AppConfig, provider: &str, model: &str) -> Result<()> {
    println!("Cuervo CLI v{}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("Provider: {provider}");
    println!("Model:    {model}");
    println!();

    // Check provider availability (env var + OS keychain)
    let providers = &config.models.providers;
    println!("Configured providers:");
    for (name, pc) in providers {
        let status = if pc.enabled { "enabled" } else { "disabled" };
        let needs_key = pc.api_key_env.is_some();
        let has_key = auth::resolve_api_key(name, pc.api_key_env.as_deref()).is_some();
        let key_status = if !needs_key {
            "no key needed"
        } else if has_key {
            "key set"
        } else {
            "key missing"
        };
        println!("  {name}: {status}, {key_status}");
    }

    println!();
    println!("Security:");
    println!(
        "  PII detection: {}",
        if config.security.pii_detection {
            "on"
        } else {
            "off"
        }
    );
    println!(
        "  Audit trail:   {}",
        if config.security.audit_enabled {
            "on"
        } else {
            "off"
        }
    );

    Ok(())
}
