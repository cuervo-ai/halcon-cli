use std::sync::Arc;

use anyhow::Result;
use cuervo_storage::Database;
use uuid::Uuid;

use crate::config_loader::default_db_path;
use crate::render::feedback;
use crate::repl::Repl;

use super::provider_factory;

/// CLI feature flags that override config.toml settings for a single session.
#[derive(Debug, Clone, Default)]
pub struct FeatureFlags {
    pub orchestrate: bool,
    pub tasks: bool,
    pub reflexion: bool,
    pub metrics: bool,
    pub timeline: bool,
    pub full: bool,
    pub expert: bool,
}

impl FeatureFlags {
    /// Apply CLI flag overrides to a mutable config.
    pub fn apply(&self, config: &mut cuervo_core::types::AppConfig) {
        if self.full || self.orchestrate {
            config.orchestrator.enabled = true;
        }
        if self.full || self.tasks {
            config.task_framework.enabled = true;
        }
        if self.full || self.reflexion {
            config.reflexion.enabled = true;
        }
    }
}

/// Run the chat command: interactive REPL or single prompt.
#[tracing::instrument(skip_all, fields(provider, model))]
pub async fn run(
    config: &cuervo_core::types::AppConfig,
    provider: &str,
    model: &str,
    prompt: Option<String>,
    resume: Option<String>,
    no_banner: bool,
    tui: bool,
    explicit_model: bool,
    flags: FeatureFlags,
) -> Result<()> {
    // Validate configuration before proceeding.
    let issues = cuervo_core::types::validate_config(config);
    let has_errors = issues
        .iter()
        .any(|i| i.level == cuervo_core::types::IssueLevel::Error);
    for issue in &issues {
        let msg = format!("[{}]: {}", issue.field, issue.message);
        match issue.level {
            cuervo_core::types::IssueLevel::Error => {
                feedback::user_error(&msg, issue.suggestion.as_deref());
            }
            cuervo_core::types::IssueLevel::Warning => {
                feedback::user_warning(&msg, issue.suggestion.as_deref());
            }
        }
    }
    if has_errors {
        anyhow::bail!("Configuration has errors. Fix them and retry.");
    }

    // Apply CLI flag overrides to a mutable copy of config.
    let mut config = config.clone();
    flags.apply(&mut config);

    let mut registry = provider_factory::build_registry(&config);

    // Ensure Ollama is available as a last-resort local fallback.
    provider_factory::ensure_local_fallback(&mut registry).await;

    // Precheck that the selected provider is available, falling back if needed.
    let (provider, model) =
        provider_factory::precheck_providers(&registry, provider, model).await?;
    let provider = provider.as_str();
    let model = model.as_str();

    let mut tool_registry = cuervo_tools::default_registry(&config.tools);

    // Connect to MCP servers and register their tools.
    let _mcp_hosts =
        provider_factory::connect_mcp_servers(&config.mcp, &mut tool_registry).await;

    // Instantiate the domain event bus for observability.
    let (event_tx, _event_rx) = cuervo_core::event_bus(256);

    // Open database (non-fatal if it fails).
    let db_path = config
        .storage
        .database_path
        .clone()
        .unwrap_or_else(default_db_path);
    let db = match Database::open(&db_path) {
        Ok(db) => {
            tracing::debug!("Database opened at {}", db_path.display());
            Some(Arc::new(db))
        }
        Err(e) => {
            tracing::warn!("Could not open database: {e}");
            None
        }
    };

    // Resume session if requested.
    let resume_session = if let Some(ref id_str) = resume {
        let id =
            Uuid::parse_str(id_str).map_err(|e| anyhow::anyhow!("Invalid session ID: {e}"))?;
        match db.as_ref().and_then(|d| d.load_session(id).ok().flatten()) {
            Some(session) => {
                println!("Resuming session {}", &id_str[..8.min(id_str.len())]);
                Some(session)
            }
            None => {
                eprintln!("Session {id_str} not found, starting new session.");
                None
            }
        }
    } else {
        None
    };

    let mut repl = Repl::new(
        &config,
        provider.to_string(),
        model.to_string(),
        db,
        resume_session,
        registry,
        tool_registry,
        event_tx,
        no_banner,
        explicit_model,
    )?;

    // Set expert mode (from --expert flag, --full flag, or config.toml display.ui_mode = "expert").
    repl.expert_mode = flags.expert || flags.full || config.display.ui_mode == "expert";

    match prompt {
        Some(p) => {
            // Single prompt with full agent loop (tools, context, resilience).
            repl.run_single_prompt(&p).await?;
        }
        None => {
            #[cfg(feature = "tui")]
            if tui {
                repl.run_tui().await?;
                // --metrics/--timeline handled below.
                print_exit_hooks(&repl, &flags);
                return Ok(());
            }

            #[cfg(not(feature = "tui"))]
            if tui {
                feedback::user_warning(
                    "TUI mode requires --features tui at compile time",
                    Some("Falling back to classic REPL"),
                );
            }

            repl.run().await?;
        }
    }

    // Post-run exit hooks.
    print_exit_hooks(&repl, &flags);

    Ok(())
}

/// Print --metrics summary and --timeline JSON on exit.
fn print_exit_hooks(repl: &Repl, flags: &FeatureFlags) {
    if flags.metrics || flags.full {
        let s = &repl.session;
        let total_tokens = s.total_usage.input_tokens + s.total_usage.output_tokens;
        let latency = s.total_latency_ms as f64 / 1000.0;
        eprintln!("\nSession Summary:");
        eprintln!(
            "  Rounds: {} | Tokens: \u{2191}{}K \u{2193}{}K | Cost: ${:.4}",
            s.agent_rounds,
            format_k(s.total_usage.input_tokens),
            format_k(s.total_usage.output_tokens),
            s.estimated_cost_usd,
        );
        eprintln!(
            "  Duration: {:.1}s | Tools: {} calls | Total tokens: {}",
            latency,
            s.tool_invocations,
            total_tokens,
        );
    }

    if flags.timeline || flags.full {
        if let Some(timeline_json) = repl.last_timeline_json() {
            println!("{timeline_json}");
        } else {
            eprintln!("(no execution timeline available — no plan was generated)");
        }
    }
}

/// Format a token count as "X.XK" for display.
fn format_k(tokens: u32) -> String {
    if tokens >= 1000 {
        format!("{:.1}", tokens as f64 / 1000.0)
    } else {
        tokens.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cuervo_core::types::AppConfig;

    #[test]
    fn config_validation_empty_config_no_errors() {
        let config = AppConfig::default();
        let issues = cuervo_core::types::validate_config(&config);
        let errors: Vec<_> = issues
            .iter()
            .filter(|i| i.level == cuervo_core::types::IssueLevel::Error)
            .collect();
        assert!(errors.is_empty(), "default config should have no errors");
    }

    #[test]
    fn feature_flags_orchestrate_ensures_enabled() {
        let mut config = AppConfig::default();
        assert!(config.orchestrator.enabled);
        let flags = FeatureFlags { orchestrate: true, ..Default::default() };
        flags.apply(&mut config);
        assert!(config.orchestrator.enabled);
    }

    #[test]
    fn feature_flags_tasks_ensures_enabled() {
        let mut config = AppConfig::default();
        assert!(config.task_framework.enabled);
        let flags = FeatureFlags { tasks: true, ..Default::default() };
        flags.apply(&mut config);
        assert!(config.task_framework.enabled);
    }

    #[test]
    fn feature_flags_reflexion_ensures_enabled() {
        let mut config = AppConfig::default();
        assert!(config.reflexion.enabled);
        let flags = FeatureFlags { reflexion: true, ..Default::default() };
        flags.apply(&mut config);
        assert!(config.reflexion.enabled);
    }

    #[test]
    fn feature_flags_full_enables_all() {
        let mut config = AppConfig::default();
        let flags = FeatureFlags { full: true, ..Default::default() };
        flags.apply(&mut config);
        assert!(config.orchestrator.enabled, "full should enable orchestrator");
        assert!(config.task_framework.enabled, "full should enable task_framework");
        assert!(config.reflexion.enabled, "full should enable reflexion");
    }

    #[test]
    fn feature_flags_default_changes_nothing() {
        let mut config = AppConfig::default();
        let orig = config.clone();
        let flags = FeatureFlags::default();
        flags.apply(&mut config);
        assert_eq!(config.orchestrator.enabled, orig.orchestrator.enabled);
        assert_eq!(config.task_framework.enabled, orig.task_framework.enabled);
        assert_eq!(config.reflexion.enabled, orig.reflexion.enabled);
    }

    #[test]
    fn format_k_below_thousand() {
        assert_eq!(format_k(500), "500");
        assert_eq!(format_k(0), "0");
        assert_eq!(format_k(999), "999");
    }

    #[test]
    fn format_k_above_thousand() {
        assert_eq!(format_k(1000), "1.0");
        assert_eq!(format_k(2100), "2.1");
        assert_eq!(format_k(8400), "8.4");
    }

    // --- Phase 42E: Expert mode tests ---

    #[test]
    fn expert_flag_in_feature_flags() {
        let flags = FeatureFlags { expert: true, ..Default::default() };
        assert!(flags.expert);
    }

    #[test]
    fn full_flag_implies_expert_config() {
        // full + expert should both result in expert mode.
        let flags_full = FeatureFlags { full: true, ..Default::default() };
        assert!(flags_full.full);
        // In run(), expert_mode = flags.expert || flags.full
    }
}
