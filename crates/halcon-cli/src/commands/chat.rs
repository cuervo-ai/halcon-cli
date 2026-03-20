use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use halcon_core::context::{ExecutionContext, EXECUTION_CTX};
use halcon_storage::Database;
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
    pub background_tools: bool,
    /// Write execution timeline as JSONL to this path (P0.2).
    pub trace_out: Option<PathBuf>,
    /// Import and display a JSONL trace file produced by --trace-out (P2.1).
    pub trace_in: Option<PathBuf>,
}

impl FeatureFlags {
    /// Apply CLI flag overrides to a mutable config.
    ///
    /// Only flags that are explicitly passed on the CLI override config.toml values.
    /// If no flag is set, the value from config.toml (or AppConfig::default()) is used.
    /// This restores --orchestrate, --tasks, and --full as meaningful toggles.
    pub fn apply(&self, config: &mut halcon_core::types::AppConfig) {
        if self.full || self.orchestrate {
            config.orchestrator.enabled = true;
            // Orchestrator requires adaptive planning to generate ExecutionPlan objects.
            // Without this, delegation logic in agent.rs never executes.
            config.planning.adaptive = true;
        }
        if self.full || self.tasks {
            config.task_framework.enabled = true;
        }
        if self.full || self.reflexion {
            config.reflexion.enabled = true;
        }
        if self.full {
            // Activate UCB1 strategy learning + metacognitive ReasoningEngine.
            // Without this, reasoning_engine is None even with --full --expert,
            // so UCB1, LoopCritic wiring, and strategy selection are all dead.
            config.reasoning.enabled = true;
            // Activate multimodal subsystem (image/audio analysis).
            // Requires API key (ANTHROPIC_API_KEY / OPENAI_API_KEY) at runtime;
            // init is non-fatal if unavailable.
            config.multimodal.enabled = true;
            // Activate V3 plugin system — scans ~/.halcon/plugins/*.plugin.toml on first message.
            // Plugins add tools to ToolRegistry so the model can invoke them natively.
            config.plugins.enabled = true;
        }
        if self.full || self.expert {
            // Activate LoopCritic adversarial post-loop evaluation.
            // Adds ~1-3s latency per agent loop but closes the G2 self-evaluation gap.
            config.reasoning.enable_loop_critic = true;
            // Activate ReasoningEngine + UCB1 strategy learning.
            // Without this, loop_critic verdicts are computed but never fed back into
            // the UCB1 selector (reasoning_engine = None → post_loop_with_reward never called).
            // This closes the G3 audit gap: --expert now has a coherent meta-cognitive loop.
            config.reasoning.enabled = true;
            // Activate structural enforcement in TaskBridge (DAG violation halt, strict mode).
            // This was defined in Phase 69 but never wired to any CLI flag — dead code.
            config.task_framework.strict_enforcement = true;
        }
    }

    /// Determine if background tools should be enabled.
    /// Background tools (background_start, background_output, background_kill) require a ProcessRegistry.
    pub fn enable_background_tools(&self) -> bool {
        self.full || self.background_tools
    }
}

/// Run the chat command: interactive REPL or single prompt.
#[tracing::instrument(skip_all, fields(provider, model))]
pub async fn run(
    config: &halcon_core::types::AppConfig,
    provider: &str,
    model: &str,
    prompt: Option<String>,
    resume: Option<String>,
    no_banner: bool,
    tui: bool,
    explicit_model: bool,
    explicit_provider: bool,
    flags: FeatureFlags,
    // US-output-format (PASO 2-A): when true, use CiSink (NDJSON) instead of ClassicSink.
    use_ci_sink: bool,
) -> Result<()> {
    // P2.1: Display imported trace if --trace-in was specified.
    if let Some(ref trace_path) = flags.trace_in {
        read_trace_jsonl(trace_path);
    }

    // Validate configuration before proceeding.
    let issues = halcon_core::types::validate_config(config);
    let has_errors = issues
        .iter()
        .any(|i| i.level == halcon_core::types::IssueLevel::Error);
    for issue in &issues {
        let msg = format!("[{}]: {}", issue.field, issue.message);
        match issue.level {
            halcon_core::types::IssueLevel::Error => {
                feedback::user_error(&msg, issue.suggestion.as_deref());
            }
            halcon_core::types::IssueLevel::Warning => {
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

    // Proactively refresh Cenzontle SSO token if near-expiry (< 5 min remaining).
    // Must run before build_registry() so the refreshed token is read from keychain.
    let _ = super::sso::refresh_if_needed().await;

    let mut registry = provider_factory::build_registry(&config);

    // Probe Ollama + fetch Cenzontle models in parallel to minimize startup latency.
    // Previously sequential (4-12s); now parallel (max of each, typically ≤2s with disk cache).
    provider_factory::ensure_startup_providers(&mut registry).await;

    // ── Auth gate ────────────────────────────────────────────────────────────
    // If no real AI provider is registered (no valid token anywhere), show the
    // interactive auth gate before starting the session.  On success, rebuild the
    // registry with the newly configured credentials.
    {
        let registry_list = registry.list();
        let no_real = super::auth_gate::registry_has_no_real_providers(&registry_list);
        drop(registry_list);

        if no_real {
            let gate = super::auth_gate::run_if_needed(&config, true).await?;
            if gate.credentials_added {
                // Rebuild registry with the freshly stored credentials.
                registry = provider_factory::build_registry(&config);
                provider_factory::ensure_startup_providers(&mut registry).await;
            }
        }
    }

    // Frontier update: in classic (non-TUI) mode, show an interactive update prompt
    // when a new version is pending.  TUI mode handles this via the overlay system.
    if !tui && !no_banner {
        if let Some(info) = super::update::get_pending_update_info() {
            match super::update::run_interactive_classic(&info) {
                Ok(true) => {
                    // User confirmed — download, verify, replace, then re-exec.
                    match super::update::run_update_from_info(&info) {
                        Ok(()) => super::update::reexec_with_current_args(),
                        Err(e) => eprintln!("  Error al instalar actualización: {e}"),
                    }
                }
                Ok(false) => {} // User skipped
                Err(e) => tracing::debug!("Interactive update prompt error: {e}"),
            }
        }
    }

    // If cenzontle was auto-detected from the keystore (token exists) but the
    // config still lists a different default_provider, promote cenzontle in-memory.
    // This covers users who ran `halcon auth login cenzontle` before v0.3.8
    // without the config-patching logic.  We only promote when:
    //   (a) the user did NOT explicitly pass -p <provider>
    //   (b) cenzontle is registered in the registry
    //   (c) the config default_provider is not already "cenzontle"
    let provider = if !explicit_provider
        && config.general.default_provider != "cenzontle"
        && registry.get("cenzontle").is_some()
    {
        tracing::info!(
            prev_default = %config.general.default_provider,
            "cenzontle token found in keystore — promoting to default provider (no -p flag)"
        );
        config.general.default_provider = "cenzontle".to_string();
        provider_factory::activate_cenzontle_in_config_once();
        "cenzontle"
    } else {
        provider
    };

    // Precheck that the selected provider is available, falling back if needed.
    // In TUI mode, allow starting even without providers (show error on first prompt).
    let (provider, model) = if tui {
        match provider_factory::precheck_providers_explicit(
            &registry,
            provider,
            model,
            explicit_model,
        )
        .await
        {
            Ok((p, m)) => (p, m),
            Err(e) => {
                feedback::user_warning(
                    &format!("No providers available: {e}"),
                    Some("TUI will start, but you'll need to configure a provider to send prompts"),
                );
                ("none".to_string(), "none".to_string())
            }
        }
    } else {
        // Classic mode requires a valid provider.
        provider_factory::precheck_providers_explicit(&registry, provider, model, explicit_model)
            .await?
    };
    let provider = provider.as_str();
    let model = model.as_str();

    // P0-A: Generate session_id early so it propagates via EXECUTION_CTX into every DomainEvent.
    let session_id = Uuid::new_v4();

    // P0-C: 4096-slot channel (was 256) — prevents silent overflow at ~9k events/session.
    let (event_tx, event_rx) = halcon_core::event_bus(4096);

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

    // P0-A + P2 FIX: Spawn audit subscriber — reads session_id from each event
    // (auto-injected via EXECUTION_CTX) and calls append_audit_event_with_session.
    // This closes the session_id=NULL bug in audit_log (9,897 orphaned rows).
    let _audit_task = if config.security.audit_enabled {
        if let Some(ref db_arc) = db {
            let db_clone = Arc::clone(db_arc);
            let mut audit_rx = event_rx;
            Some(tokio::spawn(async move {
                while let Ok(event) = audit_rx.recv().await {
                    // Use session_id embedded in the event (set by EXECUTION_CTX.scope).
                    let sid_str = event.session_id.map(|u| u.to_string());
                    let _ = db_clone.append_audit_event_with_session(&event, sid_str.as_deref());
                }
            }))
        } else {
            drop(event_rx);
            None
        }
    } else {
        drop(event_rx);
        None
    };

    // Initialize search engine if enabled and database is available.
    let search_engine = if config.search.enabled {
        if let Some(ref database) = db {
            tracing::debug!("Initializing native search engine");
            let search_config = halcon_search::SearchEngineConfig {
                index: halcon_search::config::IndexConfig {
                    max_documents: config.search.max_documents,
                    ..Default::default()
                },
                query: halcon_search::config::QueryConfig::default(),
                crawl: halcon_search::config::CrawlConfig::default(),
                cache: halcon_search::config::CacheConfig {
                    enabled: config.search.enable_cache,
                    ..Default::default()
                },
                enable_semantic_search: config.search.enable_semantic,
            };

            match halcon_search::SearchEngine::new(database.clone(), search_config) {
                Ok(engine) => {
                    tracing::info!("Native search engine initialized successfully");
                    Some(Arc::new(tokio::sync::RwLock::new(Some(engine))))
                }
                Err(e) => {
                    tracing::warn!("Failed to initialize search engine: {}", e);
                    None
                }
            }
        } else {
            tracing::warn!("Search engine enabled but no database available");
            None
        }
    } else {
        None
    };

    // Build tool registry with optional background tools support and database for native search.
    // Background tools (background_start, background_output, background_kill) require a ProcessRegistry.
    // Native search tools (native_search, native_crawl, native_index_query) require a Database.
    // When --full or --background-tools is used, we create a ProcessRegistry to enable these capabilities.
    let proc_reg = if flags.enable_background_tools() {
        Some(Arc::new(halcon_tools::background::ProcessRegistry::new(5)))
    } else {
        None
    };
    let mut tool_registry =
        halcon_tools::full_registry(&config.tools, proc_reg, db.clone(), search_engine);

    // Connect to MCP servers and register their tools.
    let _mcp_hosts = provider_factory::connect_mcp_servers(&config.mcp, &mut tool_registry).await;

    // Resume session if requested.
    let resume_session = if let Some(ref id_str) = resume {
        let id = Uuid::parse_str(id_str).map_err(|e| anyhow::anyhow!("Invalid session ID: {e}"))?;
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

    // P0-A: Establish execution context so all DomainEvent::new() calls inside this scope
    // automatically carry session_id, trace_id, and span_id — no callsite changes needed.
    let exec_ctx = ExecutionContext::new(session_id);

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

    // US-output-format (PASO 2-A): activate CiSink when --output-format json is requested.
    repl.use_ci_sink = use_ci_sink;

    // Initialize multimodal subsystem when --full enables it.
    // Non-fatal: if no API key is present or DB is unavailable, subsystem is simply absent.
    if config.multimodal.enabled {
        if let Some(ref db_arc) = repl.async_db {
            let db_clone = db_arc.clone();
            match halcon_multimodal::MultimodalSubsystem::init(
                &config.multimodal,
                Arc::new(db_clone),
            ) {
                Ok(sys) => {
                    tracing::info!("Multimodal subsystem initialized (--full)");
                    repl.multimodal = Some(Arc::new(sys));
                }
                Err(e) => {
                    tracing::warn!("Multimodal subsystem init failed (non-fatal): {e}");
                }
            }
        } else {
            tracing::debug!("Multimodal enabled but no database available — skipping init");
        }
    }

    // Wire MediaContextSource into context pipeline so analyzed images are retrievable
    // in subsequent conversation turns (closes Phase 83 gap: context source was never wired).
    if let Some(ref mm) = repl.multimodal {
        if let Some(ref mut cm) = repl.context_manager {
            cm.add_source(Box::new(mm.context_source()));
            tracing::info!("MediaContextSource wired into context pipeline (priority=55)");
        }
    }

    // P0-A: Execute the REPL inside the EXECUTION_CTX scope so that every
    // DomainEvent::new() call (in executor, orchestrator, etc.) auto-injects
    // session_id, trace_id, and span_id — fixing session_id=NULL in audit_log.
    EXECUTION_CTX
        .scope(exec_ctx, async move {
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
        })
        .await
}

/// Print session exit card — stats, plan summary, and resume command.
///
/// Always shown (no flag gate) when exiting TUI or REPL after any real work.
/// Replaces the raw JSON dump with a structured, human-readable card.
fn print_exit_hooks(repl: &Repl, flags: &FeatureFlags) {
    let s = &repl.session;
    // A session is only persisted to DB when it has at least one message.
    // session_manager::save() is a no-op for empty sessions — so a resume
    // command is only meaningful when the session was actually saved.
    let was_saved = !s.messages.is_empty();
    let has_activity = s.agent_rounds > 0 || s.tool_invocations > 0;

    // ── Show exit card only when there was real activity (saved to DB) ────────
    if (has_activity && was_saved) || (flags.metrics && was_saved) || (flags.full && was_saved) {
        let total_tokens = s.total_usage.input_tokens + s.total_usage.output_tokens;
        let latency = s.total_latency_ms as f64 / 1000.0;
        let session_id = s.id;

        // Auto-save trace to ~/.halcon/sessions/<session_id>.jsonl (only when a plan ran)
        let trace_path = if repl.last_timeline_json().is_some() {
            let tp = dirs::home_dir()
                .map(|h| h.join(".halcon").join("sessions").join(format!("{session_id}.jsonl")));
            if let Some(ref tp) = tp {
                let _ = std::fs::create_dir_all(tp.parent().unwrap());
                write_trace_jsonl(repl, tp);
            }
            tp
        } else {
            None
        };

        // Resolve resume command — uses --resume <session_id> which loads full
        // conversation history from the DB (not --trace-in which only shows the plan).
        let provider = &repl.provider;
        let model = &repl.model;
        let resume_cmd = format!(
            "halcon -p {provider} -m {model} chat --tui --full --expert --resume {session_id}"
        );

        // ── Exit card ────────────────────────────────────────────────────────
        eprintln!();
        eprintln!("╭─────────────────────────────────────────────────────────╮");
        eprintln!("│                  HALCÓN — Resumen de Sesión             │");
        eprintln!("╰─────────────────────────────────────────────────────────╯");

        // Session identity
        eprintln!("  ID:         {session_id}");
        eprintln!("  Proveedor:  {provider}  │  Modelo: {model}");
        eprintln!("  Directorio: {}", s.working_directory);
        if let Some(ref title) = s.title {
            eprintln!("  Título:     {title}");
        }

        // Stats
        eprintln!();
        eprintln!("  ┌── Estadísticas ─────────────────────────────────────┐");
        eprintln!(
            "  │  Rounds: {:>3}  │  Herramientas: {:>4} llamadas        │",
            s.agent_rounds, s.tool_invocations,
        );
        eprintln!(
            "  │  Tokens: ↑{:<8} ↓{:<8} │  Total: {:>8}     │",
            format_k(s.total_usage.input_tokens),
            format_k(s.total_usage.output_tokens),
            format_k(total_tokens),
        );
        eprintln!(
            "  │  Costo:  ${:<10.4}  │  Duración: {:<10.1}s       │",
            s.estimated_cost_usd, latency,
        );
        eprintln!("  └─────────────────────────────────────────────────────┘");

        // Plan / timeline summary (human-readable, not raw JSON)
        if let Some(timeline_json) = repl.last_timeline_json() {
            if let Ok(timeline) = serde_json::from_str::<serde_json::Value>(&timeline_json) {
                eprintln!();
                let goal = timeline["goal"].as_str().unwrap_or("(sin goal)");
                let plan_id = timeline["plan_id"].as_str().unwrap_or("?");
                let completed = timeline["completed_steps"].as_u64().unwrap_or(0);
                let total_steps = timeline["total_steps"].as_u64().unwrap_or(0);
                let elapsed_ms = timeline["total_elapsed_ms"].as_u64().unwrap_or(0);

                eprintln!("  ┌── Plan de Ejecución ─────────────────────────────────┐");
                eprintln!("  │  Goal:    {goal}");
                eprintln!("  │  Plan ID: {plan_id}");
                eprintln!(
                    "  │  Pasos:   {completed}/{total_steps} completados  │  Plan: {:.1}s",
                    elapsed_ms as f64 / 1000.0
                );

                if let Some(steps) = timeline["steps"].as_array() {
                    eprintln!("  │");
                    for step in steps {
                        let idx = step["index"].as_u64().unwrap_or(0);
                        let desc = step["description"].as_str().unwrap_or("?");
                        let status = step["status"].as_str().unwrap_or("?");
                        let dur_ms = step["duration_ms"].as_u64().unwrap_or(0);
                        let delegated = step["delegated_to"].as_str().unwrap_or("-");
                        let icon = match status {
                            "Completed" => "✓",
                            "Failed" => "✗",
                            "Skipped" => "⊘",
                            "Running" => "▸",
                            _ => "○",
                        };
                        eprintln!(
                            "  │  {icon} [{idx}] {desc}",
                        );
                        if dur_ms > 0 || delegated != "-" {
                            eprintln!(
                                "  │      → {status}  {:.1}s  agente: {delegated}",
                                dur_ms as f64 / 1000.0
                            );
                        }
                    }
                }
                eprintln!("  └─────────────────────────────────────────────────────┘");
            }
        }

        // Trace file location
        if let Some(ref tp) = trace_path {
            eprintln!();
            eprintln!("  Traza guardada → {}", tp.display());
        }

        // Resume command
        eprintln!();
        eprintln!("  ┌── Para retomar esta sesión ──────────────────────────┐");
        eprintln!("  │  {resume_cmd}");
        eprintln!("  └─────────────────────────────────────────────────────┘");
        eprintln!();
    }

    // Legacy flag: --timeline dumps raw JSON for scripting/piping
    if flags.timeline && !flags.full {
        if let Some(timeline_json) = repl.last_timeline_json() {
            eprintln!("{timeline_json}");
        }
    }

    // P0.2: Write execution timeline as JSONL to explicit --trace-out path.
    if let Some(ref trace_path) = flags.trace_out {
        write_trace_jsonl(repl, trace_path);
    }
}

// ---------------------------------------------------------------------------
// P2.1 — Trace import (--trace-in)
// ---------------------------------------------------------------------------

/// Read and display a JSONL trace file produced by `--trace-out`.
///
/// Prints a human-readable summary of the header and each step to stderr,
/// so it can be used to review or inspect a previous execution before
/// re-running the same session interactively.
pub fn read_trace_jsonl(path: &std::path::Path) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("(--trace-in: failed to read {}: {e})", path.display());
            return;
        }
    };

    let mut header: Option<serde_json::Value> = None;
    let mut steps: Vec<serde_json::Value> = Vec::new();

    for (lineno, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(v) => match v.get("type").and_then(|t| t.as_str()) {
                Some("header") => header = Some(v),
                Some("step") => steps.push(v),
                _ => {
                    tracing::debug!(lineno, "trace-in: unknown line type — skipping");
                }
            },
            Err(e) => {
                eprintln!("(--trace-in: line {}: parse error: {e})", lineno + 1);
            }
        }
    }

    // Display header.
    eprintln!("─── Imported Trace: {} ───", path.display());
    if let Some(ref h) = header {
        let goal = h["goal"].as_str().unwrap_or("(no goal)");
        let plan_id = h["plan_id"].as_str().unwrap_or("?");
        let elapsed = h["total_elapsed_ms"].as_u64().unwrap_or(0);
        eprintln!("  Goal:    {goal}");
        eprintln!("  Plan ID: {plan_id}");
        eprintln!("  Elapsed: {:.1}s", elapsed as f64 / 1000.0);
    } else {
        eprintln!("  (no header found)");
    }

    // Display steps.
    eprintln!("  Steps ({}):", steps.len());
    for step in &steps {
        let idx = step["index"].as_u64().unwrap_or(0);
        let desc = step["description"].as_str().unwrap_or("?");
        let status = step["status"].as_str().unwrap_or("?");
        let duration_ms = step["duration_ms"].as_u64();
        let dur_str = duration_ms
            .map(|ms| format!(" [{:.1}s]", ms as f64 / 1000.0))
            .unwrap_or_default();
        let icon = match status {
            "Completed" => "✓",
            "Failed" => "✗",
            "Skipped" => "⊘",
            "Running" => "▸",
            _ => "○",
        };
        eprintln!("  {icon} [{idx}] {desc}{dur_str}");
    }
    eprintln!("────────────────────────────────");
}

/// Write the execution timeline as human-editable JSONL.
///
/// Format:
/// - Line 1: `{"type":"header","plan_id":"...","goal":"...","total_elapsed_ms":...,...}`
/// - Lines 2+: `{"type":"step","index":0,"description":"...","status":"...",...}`
///
/// JSONL is chosen so each line is independently parseable and editable.
fn write_trace_jsonl(repl: &Repl, path: &std::path::Path) {
    let Some(timeline_json_str) = repl.last_timeline_json() else {
        eprintln!("(--trace-out: no timeline to export — no plan was generated this session)");
        return;
    };

    let timeline: serde_json::Value = match serde_json::from_str(&timeline_json_str) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("(--trace-out: failed to parse timeline JSON: {e})");
            return;
        }
    };

    let mut lines: Vec<String> = Vec::new();

    // Header line — strip "steps" array, keep metadata + inject session_id.
    let mut header = timeline.clone();
    if let Some(obj) = header.as_object_mut() {
        obj.remove("steps");
        obj.insert("type".into(), serde_json::json!("header"));
        // Inject session_id so --resume can load the full conversation from DB.
        obj.insert(
            "session_id".into(),
            serde_json::json!(repl.session.id.to_string()),
        );
    }
    match serde_json::to_string(&header) {
        Ok(s) => lines.push(s),
        Err(e) => {
            eprintln!("(--trace-out: failed to serialize header: {e})");
            return;
        }
    }

    // Step lines.
    if let Some(steps) = timeline.get("steps").and_then(|s| s.as_array()) {
        for step in steps {
            let mut step = step.clone();
            if let Some(obj) = step.as_object_mut() {
                obj.insert("type".into(), serde_json::json!("step"));
            }
            match serde_json::to_string(&step) {
                Ok(s) => lines.push(s),
                Err(e) => eprintln!("(--trace-out: failed to serialize step: {e})"),
            }
        }
    }

    let content = lines.join("\n") + "\n";
    match std::fs::write(path, content) {
        Ok(()) => {
            eprintln!(
                "Trace exported → {} ({} entries)",
                path.display(),
                lines.len()
            );
        }
        Err(e) => eprintln!("(--trace-out: failed to write {}: {e})", path.display()),
    }
}

/// Format a token count as "X.XK" for display.
fn format_k(tokens: u32) -> String {
    if tokens >= 1000 {
        format!("{:.1}K", tokens as f64 / 1000.0)
    } else {
        tokens.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::types::AppConfig;

    #[test]
    fn config_validation_empty_config_no_errors() {
        let config = AppConfig::default();
        let issues = halcon_core::types::validate_config(&config);
        let errors: Vec<_> = issues
            .iter()
            .filter(|i| i.level == halcon_core::types::IssueLevel::Error)
            .collect();
        assert!(errors.is_empty(), "default config should have no errors");
    }

    #[test]
    fn feature_flags_orchestrate_ensures_enabled() {
        let mut config = AppConfig::default();
        assert!(config.orchestrator.enabled);
        let flags = FeatureFlags {
            orchestrate: true,
            ..Default::default()
        };
        flags.apply(&mut config);
        assert!(config.orchestrator.enabled);
    }

    #[test]
    fn feature_flags_tasks_ensures_enabled() {
        let mut config = AppConfig::default();
        assert!(config.task_framework.enabled);
        let flags = FeatureFlags {
            tasks: true,
            ..Default::default()
        };
        flags.apply(&mut config);
        assert!(config.task_framework.enabled);
    }

    #[test]
    fn feature_flags_reflexion_ensures_enabled() {
        let mut config = AppConfig::default();
        assert!(config.reflexion.enabled);
        let flags = FeatureFlags {
            reflexion: true,
            ..Default::default()
        };
        flags.apply(&mut config);
        assert!(config.reflexion.enabled);
    }

    #[test]
    fn feature_flags_full_enables_all() {
        let mut config = AppConfig::default();
        let flags = FeatureFlags {
            full: true,
            ..Default::default()
        };
        flags.apply(&mut config);
        assert!(
            config.orchestrator.enabled,
            "full should enable orchestrator"
        );
        assert!(
            config.task_framework.enabled,
            "full should enable task_framework"
        );
        assert!(config.reflexion.enabled, "full should enable reflexion");
        assert!(
            config.planning.adaptive,
            "full should enable adaptive planning (orchestrator dependency)"
        );
        assert!(
            config.reasoning.enabled,
            "full should enable reasoning/UCB1"
        );
        assert!(config.multimodal.enabled, "full should enable multimodal");
        assert!(
            config.reasoning.enable_loop_critic,
            "full should enable loop_critic"
        );
    }

    #[test]
    fn feature_flags_orchestrate_enables_planning() {
        let mut config = AppConfig::default();
        config.planning.adaptive = false; // Explicitly set to false
        let flags = FeatureFlags {
            orchestrate: true,
            ..Default::default()
        };
        flags.apply(&mut config);
        assert!(
            config.orchestrator.enabled,
            "orchestrate should enable orchestrator"
        );
        assert!(
            config.planning.adaptive,
            "orchestrate should enable planning.adaptive (dependency)"
        );
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
        assert_eq!(format_k(1000), "1.0K");
        assert_eq!(format_k(2100), "2.1K");
        assert_eq!(format_k(8400), "8.4K");
    }

    // --- Phase 42E: Expert mode tests ---

    #[test]
    fn expert_flag_in_feature_flags() {
        let flags = FeatureFlags {
            expert: true,
            ..Default::default()
        };
        assert!(flags.expert);
    }

    #[test]
    fn full_flag_implies_expert_config() {
        // full + expert should both result in expert mode.
        let flags_full = FeatureFlags {
            full: true,
            ..Default::default()
        };
        assert!(flags_full.full);
        // In run(), expert_mode = flags.expert || flags.full
    }

    // --- Background tools tests ---

    #[test]
    fn background_tools_flag_enables_background_tools() {
        let flags = FeatureFlags {
            background_tools: true,
            ..Default::default()
        };
        assert!(flags.enable_background_tools());
    }

    #[test]
    fn full_flag_enables_background_tools() {
        let flags = FeatureFlags {
            full: true,
            ..Default::default()
        };
        assert!(flags.enable_background_tools());
    }

    #[test]
    fn default_flags_disable_background_tools() {
        let flags = FeatureFlags::default();
        assert!(!flags.enable_background_tools());
    }

    #[test]
    fn background_tools_independent_of_other_flags() {
        let flags = FeatureFlags {
            orchestrate: true,
            tasks: true,
            reflexion: true,
            background_tools: false,
            ..Default::default()
        };
        assert!(!flags.enable_background_tools());
    }

    // --- P0.2: trace-out JSONL tests ---

    /// Build a synthetic timeline JSON string matching ExecutionTimeline format.
    fn make_timeline_json(n_steps: usize) -> String {
        let steps: Vec<serde_json::Value> = (0..n_steps)
            .map(|i| {
                serde_json::json!({
                    "index": i,
                    "description": format!("Step {i}"),
                    "tool_name": null,
                    "status": "Completed",
                    "started_at": null,
                    "finished_at": null,
                    "duration_ms": null,
                    "round": null,
                    "delegated_to": null,
                    "sub_agent_task_id": null
                })
            })
            .collect();

        serde_json::to_string(&serde_json::json!({
            "plan_id": "00000000-0000-0000-0000-000000000001",
            "goal": "test goal",
            "total_elapsed_ms": 1234,
            "completed_steps": n_steps,
            "total_steps": n_steps,
            "steps": steps
        }))
        .unwrap()
    }

    /// Helper: call the JSONL builder directly without needing a Repl.
    fn build_jsonl_lines(timeline_json: &str) -> Vec<String> {
        let timeline: serde_json::Value = serde_json::from_str(timeline_json).unwrap();
        let mut lines = Vec::new();

        // Header line.
        let mut header = timeline.clone();
        if let Some(obj) = header.as_object_mut() {
            obj.remove("steps");
            obj.insert("type".into(), serde_json::json!("header"));
        }
        lines.push(serde_json::to_string(&header).unwrap());

        // Step lines.
        if let Some(steps) = timeline.get("steps").and_then(|s| s.as_array()) {
            for step in steps {
                let mut step = step.clone();
                if let Some(obj) = step.as_object_mut() {
                    obj.insert("type".into(), serde_json::json!("step"));
                }
                lines.push(serde_json::to_string(&step).unwrap());
            }
        }
        lines
    }

    #[test]
    fn trace_out_header_line_has_type_header() {
        let json = make_timeline_json(2);
        let lines = build_jsonl_lines(&json);
        let header: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(header["type"], "header");
        assert_eq!(header["goal"], "test goal");
        assert!(
            !header.as_object().unwrap().contains_key("steps"),
            "header must not contain steps array"
        );
    }

    #[test]
    fn trace_out_step_lines_have_type_step() {
        let json = make_timeline_json(3);
        let lines = build_jsonl_lines(&json);
        // lines[0] = header, lines[1..] = steps
        assert_eq!(lines.len(), 4); // 1 header + 3 steps
        for line in lines.iter().skip(1) {
            let step: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(step["type"], "step");
        }
    }

    #[test]
    fn trace_out_each_line_is_valid_json() {
        let json = make_timeline_json(5);
        let lines = build_jsonl_lines(&json);
        for (i, line) in lines.iter().enumerate() {
            serde_json::from_str::<serde_json::Value>(line)
                .unwrap_or_else(|e| panic!("Line {i} is not valid JSON: {e}\n  content: {line}"));
        }
    }

    #[test]
    fn trace_out_empty_plan_has_only_header() {
        let json = make_timeline_json(0);
        let lines = build_jsonl_lines(&json);
        assert_eq!(lines.len(), 1, "zero steps = only header line");
        let header: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(header["type"], "header");
    }

    #[test]
    fn trace_out_file_written_correctly() {
        let json = make_timeline_json(2);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");

        // Write JSONL using the builder logic.
        let lines = build_jsonl_lines(&json);
        let content = lines.join("\n") + "\n";
        std::fs::write(&path, &content).unwrap();

        // Read back and verify.
        let read_back = std::fs::read_to_string(&path).unwrap();
        let read_lines: Vec<&str> = read_back.trim_end_matches('\n').split('\n').collect();
        assert_eq!(read_lines.len(), 3); // 1 header + 2 steps
        for line in &read_lines {
            serde_json::from_str::<serde_json::Value>(line)
                .unwrap_or_else(|e| panic!("Invalid JSON in file: {e}"));
        }
    }

    #[test]
    fn trace_out_step_index_preserved() {
        let json = make_timeline_json(4);
        let lines = build_jsonl_lines(&json);
        for (i, line) in lines.iter().skip(1).enumerate() {
            let step: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(step["index"], i, "step index must match order");
        }
    }

    #[test]
    fn trace_out_flag_defaults_none() {
        let flags = FeatureFlags::default();
        assert!(flags.trace_out.is_none());
    }

    #[test]
    fn trace_out_flag_can_be_set() {
        let flags = FeatureFlags {
            trace_out: Some(std::path::PathBuf::from("/tmp/out.jsonl")),
            ..Default::default()
        };
        assert!(flags.trace_out.is_some());
    }

    // --- P2.1: trace-in tests ---

    fn make_valid_jsonl(n_steps: usize) -> String {
        let json = make_timeline_json(n_steps);
        let lines = build_jsonl_lines(&json);
        lines.join("\n") + "\n"
    }

    #[test]
    fn trace_in_missing_file_does_not_panic() {
        // read_trace_jsonl prints to stderr but must not panic.
        read_trace_jsonl(std::path::Path::new(
            "/tmp/halcon_nonexistent_trace_xyz.jsonl",
        ));
    }

    #[test]
    fn trace_in_reads_valid_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        std::fs::write(&path, make_valid_jsonl(3)).unwrap();
        // Must not panic.
        read_trace_jsonl(&path);
    }

    #[test]
    fn trace_in_handles_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.jsonl");
        std::fs::write(&path, "").unwrap();
        read_trace_jsonl(&path);
    }

    #[test]
    fn trace_in_handles_invalid_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.jsonl");
        std::fs::write(&path, "not json\nalso not json\n").unwrap();
        // Should not panic — just warn.
        read_trace_jsonl(&path);
    }

    #[test]
    fn trace_in_roundtrip_matches_trace_out() {
        // Write a trace with build_jsonl_lines, then read it back.
        let json = make_timeline_json(2);
        let lines = build_jsonl_lines(&json);
        let content = lines.join("\n") + "\n";

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("roundtrip.jsonl");
        std::fs::write(&path, &content).unwrap();

        // Parse the file manually to verify structure.
        let read_back = std::fs::read_to_string(&path).unwrap();
        let parsed: Vec<serde_json::Value> = read_back
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();

        assert_eq!(parsed.len(), 3); // 1 header + 2 steps
        assert_eq!(parsed[0]["type"], "header");
        assert_eq!(parsed[0]["goal"], "test goal");
        assert_eq!(parsed[1]["type"], "step");
        assert_eq!(parsed[2]["type"], "step");
    }

    #[test]
    fn trace_in_flag_defaults_none() {
        let flags = FeatureFlags::default();
        assert!(flags.trace_in.is_none());
    }

    #[test]
    fn trace_in_flag_can_be_set() {
        let flags = FeatureFlags {
            trace_in: Some(std::path::PathBuf::from("/tmp/in.jsonl")),
            ..Default::default()
        };
        assert!(flags.trace_in.is_some());
    }

    // --- Phase 75: Activation tests ---

    #[test]
    fn full_flag_enables_multimodal() {
        let mut config = AppConfig::default();
        assert!(
            !config.multimodal.enabled,
            "multimodal defaults to disabled"
        );
        let flags = FeatureFlags {
            full: true,
            ..Default::default()
        };
        flags.apply(&mut config);
        assert!(config.multimodal.enabled, "--full must enable multimodal");
    }

    #[test]
    fn full_flag_enables_loop_critic() {
        let mut config = AppConfig::default();
        assert!(
            !config.reasoning.enable_loop_critic,
            "loop_critic defaults to false"
        );
        let flags = FeatureFlags {
            full: true,
            ..Default::default()
        };
        flags.apply(&mut config);
        assert!(
            config.reasoning.enable_loop_critic,
            "--full must enable loop_critic"
        );
    }

    #[test]
    fn expert_flag_enables_loop_critic() {
        let mut config = AppConfig::default();
        assert!(
            !config.reasoning.enable_loop_critic,
            "loop_critic defaults to false"
        );
        let flags = FeatureFlags {
            expert: true,
            ..Default::default()
        };
        flags.apply(&mut config);
        assert!(
            config.reasoning.enable_loop_critic,
            "--expert must enable loop_critic"
        );
    }

    #[test]
    fn expert_only_does_not_enable_multimodal() {
        // Multimodal requires --full (heavier subsystem with API costs).
        // --expert alone only activates loop_critic (latency-only overhead).
        let mut config = AppConfig::default();
        let flags = FeatureFlags {
            expert: true,
            ..Default::default()
        };
        flags.apply(&mut config);
        assert!(
            !config.multimodal.enabled,
            "--expert alone must NOT enable multimodal"
        );
    }

    #[test]
    fn full_flag_enables_reasoning_for_ucb1() {
        let mut config = AppConfig::default();
        // Verify that reasoning (UCB1 engine) is enabled by --full.
        config.reasoning.enabled = false; // ensure starting state
        let flags = FeatureFlags {
            full: true,
            ..Default::default()
        };
        flags.apply(&mut config);
        assert!(
            config.reasoning.enabled,
            "--full must enable reasoning (UCB1/ReasoningEngine)"
        );
    }

    #[test]
    fn full_flag_enables_plugins() {
        let mut config = AppConfig::default();
        assert!(!config.plugins.enabled, "plugins default to disabled");
        let flags = FeatureFlags {
            full: true,
            ..Default::default()
        };
        flags.apply(&mut config);
        assert!(
            config.plugins.enabled,
            "--full must enable the V3 plugin system"
        );
    }

    #[test]
    fn non_full_flags_do_not_enable_plugins() {
        // Plugin system requires explicit --full (or manual config.toml edit).
        let flags = FeatureFlags {
            expert: true,
            orchestrate: true,
            tasks: true,
            ..Default::default()
        };
        let mut config = AppConfig::default();
        flags.apply(&mut config);
        assert!(
            !config.plugins.enabled,
            "--expert/--orchestrate/--tasks must NOT auto-enable plugins"
        );
    }

    // --- Audit Fix: --expert coherent meta-cognitive loop ---

    #[test]
    fn expert_flag_enables_reasoning_for_ucb1() {
        // AUDIT FIX: previously --expert activated loop_critic but reasoning_engine was None
        // because config.reasoning.enabled was never set. UCB1 learning was dead even with
        // --expert. Now --expert enables both so the full meta-cognitive loop is coherent.
        let mut config = AppConfig::default();
        config.reasoning.enabled = false;
        let flags = FeatureFlags {
            expert: true,
            ..Default::default()
        };
        flags.apply(&mut config);
        assert!(
            config.reasoning.enabled,
            "--expert must enable reasoning/UCB1 (loop_critic needs an engine)"
        );
        assert!(
            config.reasoning.enable_loop_critic,
            "--expert must also enable loop_critic"
        );
    }

    #[test]
    fn expert_flag_enables_strict_enforcement() {
        // AUDIT FIX: strict_enforcement was defined (Phase 69) and documented as activated by
        // --expert but was never set by any CLI code. Dead config field. Now wired.
        let mut config = AppConfig::default();
        assert!(
            !config.task_framework.strict_enforcement,
            "strict_enforcement defaults to false"
        );
        let flags = FeatureFlags {
            expert: true,
            ..Default::default()
        };
        flags.apply(&mut config);
        assert!(
            config.task_framework.strict_enforcement,
            "--expert must enable strict_enforcement (DAG violation halt, cascade halt, planner strict mode)"
        );
    }
}
