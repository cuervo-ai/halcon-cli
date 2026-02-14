use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use reedline::{
    EditCommand, FileBackedHistory, KeyCode, KeyModifiers, Reedline, ReedlineEvent, Signal,
};

use cuervo_core::traits::{ContextQuery, ContextSource, ModelProvider, Planner};
use cuervo_core::types::{
    AppConfig, ChatMessage, DomainEvent, EventPayload, MessageContent, ModelRequest,
    Role, Session,
};
use cuervo_core::EventSender;
use cuervo_providers::ProviderRegistry;
use cuervo_storage::{AsyncDatabase, Database};
use cuervo_tools::ToolRegistry;

use memory_source::MemorySource;
use permissions::PermissionChecker;
use planning_source::PlanningSource;
use resilience::ResilienceManager;
use response_cache::ResponseCache;

pub mod accumulator;
pub mod agent;
pub mod agent_comm;
pub mod agent_types;
pub mod agent_utils;
pub mod artifact_store;
pub mod authorization;
pub mod backpressure;
pub mod circuit_breaker;
pub mod commands;
pub mod compaction;
pub mod console;
pub mod context_governance;
pub mod context_manager;
pub mod context_metrics;
pub mod delegation;
pub mod episodic_source;
pub mod execution_tracker;
pub mod executor;
pub mod failure_tracker;
pub mod health;
pub mod idempotency;
pub mod hybrid_retriever;
pub mod loop_guard;
pub mod mcp_manager;
pub mod memory_consolidator;
pub mod memory_source;
pub mod model_selector;
pub mod optimizer;
pub mod orchestrator;
pub mod permissions;
pub mod reasoning_engine;
pub mod planner;
pub mod planning_source;
pub mod provenance_tracker;
mod prompt;
pub mod reflection_source;
pub mod reflexion;
pub mod repo_map_source;
pub mod replay_executor;
pub mod replay_runner;
pub mod resilience;
pub mod response_cache;
pub mod router;
pub mod speculative;
pub mod strategy_selector;
pub mod task_analyzer;
pub mod task_backlog;
pub mod task_evaluator;
pub mod task_bridge;
pub mod task_scheduler;
pub mod tool_selector;
pub mod tool_speculation;

mod slash_commands;

#[cfg(test)]
mod stress_tests;

use prompt::CuervoPrompt;

/// Interactive REPL for cuervo.
pub struct Repl {
    pub(crate) editor: Reedline,
    pub(crate) prompt: CuervoPrompt,
    pub(crate) config: AppConfig,
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) session: Session,
    pub(crate) db: Option<Arc<Database>>,
    pub(crate) async_db: Option<AsyncDatabase>,
    pub(crate) registry: ProviderRegistry,
    pub(crate) tool_registry: ToolRegistry,
    pub(crate) permissions: PermissionChecker,
    pub(crate) event_tx: EventSender,
    pub(crate) context_sources: Vec<Box<dyn ContextSource>>,
    pub(crate) response_cache: Option<ResponseCache>,
    pub(crate) resilience: ResilienceManager,
    pub(crate) reflector: Option<reflexion::Reflector>,
    pub(crate) no_banner: bool,
    /// When true, the user explicitly set `--model` on the CLI, so model selection is bypassed.
    pub(crate) explicit_model: bool,
    /// Temporary dry-run mode override for the next handle_message call.
    pub(crate) dry_run_override: Option<cuervo_core::types::DryRunMode>,
    /// Trace step cursor for /step forward/back navigation.
    pub(crate) trace_cursor: Option<(uuid::Uuid, Vec<cuervo_storage::TraceStep>, usize)>,
    /// Adaptive reasoning engine (enabled by default).
    pub(crate) reasoning_engine: Option<reasoning_engine::ReasoningEngine>,
    /// Cached execution timeline JSON from the last agent loop (for --timeline flag).
    pub(crate) last_timeline: Option<String>,
    /// Shared context metrics for agent loop observability (Phase 42).
    pub(crate) context_metrics: std::sync::Arc<context_metrics::ContextMetrics>,
    /// Context governance for per-source token limits (Phase 42).
    pub(crate) context_governance: context_governance::ContextGovernance,
    /// Expert mode: show full agent feedback (model selection, caching, etc.).
    pub(crate) expert_mode: bool,
    /// Control channel receiver from TUI (Phase 43). None in classic REPL mode.
    #[cfg(feature = "tui")]
    pub(crate) ctrl_rx: Option<tokio::sync::mpsc::UnboundedReceiver<crate::tui::events::ControlEvent>>,
}

impl Repl {
    /// Create a new REPL instance with file-backed history and optional DB.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: &AppConfig,
        provider: String,
        model: String,
        db: Option<Arc<Database>>,
        resume_session: Option<Session>,
        registry: ProviderRegistry,
        tool_registry: ToolRegistry,
        event_tx: EventSender,
        no_banner: bool,
        explicit_model: bool,
    ) -> Result<Self> {
        let mut keybindings = reedline::default_emacs_keybindings();
        // Alt+Enter inserts a newline (multi-line input).
        keybindings.add_binding(
            KeyModifiers::ALT,
            KeyCode::Enter,
            ReedlineEvent::Edit(vec![EditCommand::InsertNewline]),
        );

        let mut editor =
            Reedline::create().with_edit_mode(Box::new(reedline::Emacs::new(keybindings)));

        if let Some(path) = Self::history_path() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let history = Box::new(
                FileBackedHistory::with_file(1000, path)
                    .map_err(|e| anyhow::anyhow!("Failed to init history: {e}"))?,
            );
            editor = editor.with_history(history);
        }

        let prompt = CuervoPrompt::new(&provider, &model);

        let cwd = std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let session =
            resume_session.unwrap_or_else(|| Session::new(model.clone(), provider.clone(), cwd));

        let permissions = PermissionChecker::with_config(
            config.tools.confirm_destructive,
            config.security.tbac_enabled,
            config.tools.prompt_timeout_secs,
        );

        // Build async database wrapper (for async call sites).
        let async_db = db.as_ref().map(|db_ref| AsyncDatabase::new(Arc::clone(db_ref)));

        // Build context sources.
        let mut context_sources: Vec<Box<dyn ContextSource>> = vec![
            Box::new(cuervo_context::InstructionSource::new()),
            Box::new(repo_map_source::RepoMapSource::default()),
        ];

        if config.planning.enabled {
            context_sources.push(Box::new(PlanningSource::new(&config.planning)));
        }

        if config.memory.enabled {
            if let Some(ref adb) = async_db {
                if config.memory.episodic {
                    // Episodic memory with hybrid retrieval (BM25 + RRF + temporal decay).
                    let retriever = hybrid_retriever::HybridRetriever::new(adb.clone())
                        .with_rrf_k(config.memory.rrf_k)
                        .with_decay_half_life(config.memory.decay_half_life_days);
                    context_sources.push(Box::new(episodic_source::EpisodicSource::new(
                        retriever,
                        config.memory.retrieval_top_k,
                        config.memory.retrieval_token_budget,
                    )));
                } else {
                    // Legacy BM25-only memory source.
                    context_sources.push(Box::new(MemorySource::new(
                        adb.clone(),
                        config.memory.retrieval_top_k,
                        config.memory.retrieval_token_budget,
                    )));
                }
            }
        }

        // Initialize reflexion: Reflector + ReflectionSource.
        let reflector = if config.reflexion.enabled {
            registry
                .get(&provider)
                .cloned()
                .map(|p| {
                    reflexion::Reflector::new(p, model.clone())
                        .with_reflect_on_success(config.reflexion.reflect_on_success)
                })
        } else {
            None
        };

        if config.reflexion.enabled {
            if let Some(ref adb) = async_db {
                context_sources.push(Box::new(reflection_source::ReflectionSource::new(
                    adb.clone(),
                    config.reflexion.max_reflections,
                )));
            }
        }

        // Initialize response cache when DB is available and cache is enabled.
        let response_cache = if config.cache.enabled {
            async_db.as_ref().map(|adb| {
                ResponseCache::new(adb.clone(), config.cache.clone())
            })
        } else {
            None
        };

        // Initialize resilience manager and register ALL providers from the registry.
        let mut resilience =
            ResilienceManager::new(config.resilience.clone()).with_event_tx(event_tx.clone());
        if let Some(ref adb) = async_db {
            resilience = resilience.with_db(adb.clone());
        }
        for name in registry.list() {
            resilience.register_provider(name);
        }

        // Initialize reasoning engine if enabled.
        let reasoning_engine = if config.reasoning.enabled {
            let mut engine = reasoning_engine::ReasoningEngine::new(&config.reasoning);
            // Load experience from DB for cross-session learning.
            if let Some(ref adb) = async_db {
                // Use sync inner() since we're in a sync constructor.
                if let Ok(rows) = adb.inner().load_all_experience() {
                    let records = reasoning_engine::rows_to_records(rows);
                    engine.load_experience(records);
                }
            }
            Some(engine)
        } else {
            None
        };

        Ok(Self {
            editor,
            prompt,
            config: config.clone(),
            provider,
            model,
            session,
            db,
            async_db,
            registry,
            tool_registry,
            permissions,
            event_tx,
            context_sources,
            response_cache,
            resilience,
            reflector,
            no_banner,
            explicit_model,
            dry_run_override: None,
            trace_cursor: None,
            reasoning_engine,
            last_timeline: None,
            context_metrics: std::sync::Arc::new(context_metrics::ContextMetrics::default()),
            context_governance: {
                let gov_config = &config.context.governance;
                if gov_config.default_max_tokens_per_source > 0 {
                    context_governance::ContextGovernance::with_default_max_tokens(
                        gov_config.default_max_tokens_per_source,
                    )
                } else {
                    context_governance::ContextGovernance::new(std::collections::HashMap::new())
                }
            },
            expert_mode: false,
            #[cfg(feature = "tui")]
            ctrl_rx: None,
        })
    }

    /// Execute a single prompt through the full agent loop (with tools), then exit.
    ///
    /// This gives inline prompts (`cuervo chat "do X"`) the same capabilities as
    /// the interactive REPL: tool execution, context assembly, resilience, etc.
    pub async fn run_single_prompt(&mut self, prompt: &str) -> Result<()> {
        // Non-interactive mode: auto-approve tools since there's no TTY for prompts.
        self.permissions.set_non_interactive();

        // Warm L1 cache from L2 on startup.
        if let Some(ref cache) = self.response_cache {
            cache.warm_l1().await;
        }

        // Emit SessionStarted event.
        let _ = self.event_tx.send(DomainEvent::new(EventPayload::SessionStarted {
            session_id: self.session.id,
        }));

        // Run the prompt through handle_message (full agent loop with tools).
        self.handle_message(prompt).await?;

        // Save session.
        self.auto_save_session().await;
        self.save_session();

        Ok(())
    }

    /// Run the interactive REPL loop.
    pub async fn run(&mut self) -> Result<()> {
        // Warm L1 cache from L2 on startup.
        if let Some(ref cache) = self.response_cache {
            cache.warm_l1().await;
        }

        self.print_welcome();

        // Emit SessionStarted event.
        let _ = self.event_tx.send(DomainEvent::new(EventPayload::SessionStarted {
            session_id: self.session.id,
        }));

        loop {
            let sig = self.editor.read_line(&self.prompt);

            match sig {
                Ok(Signal::Success(line)) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Some(cmd) = trimmed.strip_prefix('/') {
                        match commands::handle(cmd, &self.provider, &self.model) {
                            commands::CommandResult::Continue => continue,
                            commands::CommandResult::Exit => break,
                            commands::CommandResult::Unknown(c) => {
                                crate::render::feedback::user_error(
                                    &format!("unknown command '/{c}'"),
                                    Some("Type /help for available commands"),
                                );
                                continue;
                            }
                            commands::CommandResult::SessionList => {
                                self.list_sessions();
                                continue;
                            }
                            commands::CommandResult::SessionShow => {
                                self.show_session();
                                continue;
                            }
                            commands::CommandResult::TestRun(kind) => {
                                self.run_test(&kind).await;
                                continue;
                            }
                            commands::CommandResult::Orchestrate(instruction) => {
                                self.run_orchestrate(&instruction).await;
                                continue;
                            }
                            commands::CommandResult::DryRun(prompt) => {
                                self.handle_message_dry_run(&prompt).await?;
                                continue;
                            }
                            commands::CommandResult::TraceInfo => {
                                self.show_trace_info();
                                continue;
                            }
                            commands::CommandResult::StateInfo => {
                                self.show_state_info();
                                continue;
                            }

                            // --- Phase 19: Agent Operating Console ---
                            commands::CommandResult::Research(query) => {
                                self.handle_research(&query).await;
                                continue;
                            }
                            commands::CommandResult::Inspect(target) => {
                                self.handle_inspect(&target).await;
                                continue;
                            }
                            commands::CommandResult::Plan(goal) => {
                                self.handle_plan(&goal).await;
                                continue;
                            }
                            commands::CommandResult::RunPlan(plan_id) => {
                                self.handle_run_plan(&plan_id).await;
                                continue;
                            }
                            commands::CommandResult::Resume(session_id) => {
                                self.handle_resume(&session_id).await;
                                continue;
                            }
                            commands::CommandResult::Cancel(task_id) => {
                                self.handle_cancel(&task_id).await;
                                continue;
                            }
                            commands::CommandResult::LiveStatus => {
                                self.handle_live_status().await;
                                continue;
                            }
                            commands::CommandResult::Logs(filter) => {
                                self.handle_logs(filter.as_deref()).await;
                                continue;
                            }
                            commands::CommandResult::Metrics => {
                                self.handle_metrics().await;
                                continue;
                            }
                            commands::CommandResult::TraceBrowse(session_id) => {
                                self.handle_trace_browse(session_id.as_deref()).await;
                                continue;
                            }
                            commands::CommandResult::Replay(session_id) => {
                                self.handle_replay(&session_id).await;
                                continue;
                            }
                            commands::CommandResult::Step(direction) => {
                                self.handle_step(&direction).await;
                                continue;
                            }
                            commands::CommandResult::Snapshot => {
                                self.handle_snapshot().await;
                                continue;
                            }
                            commands::CommandResult::Diff(a, b) => {
                                self.handle_diff(&a, &b).await;
                                continue;
                            }
                            commands::CommandResult::Benchmark(workload) => {
                                self.handle_benchmark(&workload).await;
                                continue;
                            }
                            commands::CommandResult::Optimize => {
                                self.handle_optimize().await;
                                continue;
                            }
                            commands::CommandResult::Analyze => {
                                self.handle_analyze().await;
                                continue;
                            }
                        }
                    }
                    self.handle_message(trimmed).await?;
                    // Auto-save session after each message exchange.
                    self.auto_save_session().await;
                }
                Ok(Signal::CtrlC) => {
                    continue;
                }
                Ok(Signal::CtrlD) => {
                    println!("\nGoodbye!");
                    break;
                }
                Err(err) => {
                    crate::render::feedback::user_error(
                        &format!("input failed — {err}"),
                        None,
                    );
                    break;
                }
            }
        }

        self.save_session();
        self.print_session_summary();
        Ok(())
    }

    /// Run the TUI-based interactive loop.
    ///
    /// Spawns a ratatui 3-zone TUI (prompt / activity / status) and bridges
    /// the agent loop through `TuiSink` ↔ UiEvent channel.
    #[cfg(feature = "tui")]
    pub async fn run_tui(&mut self) -> Result<()> {
        use crate::render::sink::TuiSink;
        use crate::tui::app::TuiApp;
        use crate::tui::events::UiEvent;
        use tokio::sync::mpsc as tokio_mpsc;

        // Warm L1 cache from L2 on startup.
        if let Some(ref cache) = self.response_cache {
            cache.warm_l1().await;
        }

        // Emit SessionStarted event.
        let _ = self.event_tx.send(DomainEvent::new(EventPayload::SessionStarted {
            session_id: self.session.id,
        }));

        // Create channels: UiEvents (agent → TUI) and prompts (TUI → agent).
        let (ui_tx, ui_rx) = tokio_mpsc::channel::<UiEvent>(1024);
        let (prompt_tx, mut prompt_rx) = tokio_mpsc::unbounded_channel::<String>();

        let tui_sink = TuiSink::new(ui_tx.clone());

        // Gather banner info for TUI startup display.
        let banner_version = env!("CARGO_PKG_VERSION").to_string();
        let banner_provider = self.provider.clone();
        let banner_provider_connected = self.registry.get(&self.provider).is_some();
        let banner_model = self.model.clone();
        let banner_session_id = self.session.id.to_string()[..8].to_string();
        let banner_session_type = if self.session.messages.is_empty() {
            "new".to_string()
        } else {
            "resumed".to_string()
        };

        // Build routing display info for TUI banner.
        let banner_routing = if !self.config.agent.routing.fallback_models.is_empty() {
            Some(crate::render::banner::RoutingDisplay {
                mode: self.config.agent.routing.mode.clone(),
                strategy: self.config.agent.routing.strategy.clone(),
                fallback_chain: std::iter::once(self.provider.clone())
                    .chain(self.config.agent.routing.fallback_models.clone())
                    .collect(),
            })
        } else {
            None
        };

        // Control channel: TUI → agent (pause/step/cancel).
        let (ctrl_tx, ctrl_rx) = tokio::sync::mpsc::unbounded_channel();
        self.ctrl_rx = Some(ctrl_rx);

        // Permission approval channel: TUI → PermissionChecker (approve/reject).
        // Dedicated channel ensures permission decisions reach the executor even
        // while the agent loop is blocked on tool execution.
        let (perm_tx, perm_rx) = tokio::sync::mpsc::unbounded_channel::<bool>();
        self.permissions.set_tui_channel(perm_rx);

        // Determine initial UI mode from expert_mode flag.
        let initial_mode = if self.expert_mode {
            crate::tui::state::UiMode::Expert
        } else {
            // Map config string to UiMode.
            match self.config.display.ui_mode.as_str() {
                "minimal" => crate::tui::state::UiMode::Minimal,
                "expert" => crate::tui::state::UiMode::Expert,
                _ => crate::tui::state::UiMode::Standard,
            }
        };

        // Spawn TUI render loop in a separate task.
        let tui_handle = tokio::spawn(async move {
            let mut app = TuiApp::with_mode(ui_rx, prompt_tx, ctrl_tx, perm_tx, initial_mode);
            app.push_banner(
                &banner_version,
                &banner_provider,
                banner_provider_connected,
                &banner_model,
                &banner_session_id,
                &banner_session_type,
                banner_routing.as_ref(),
            );
            app.run().await
        });

        // Send initial status update with session info.
        let session_id_short = self.session.id.to_string()[..8].to_string();
        let _ = ui_tx.send(UiEvent::StatusUpdate {
            provider: Some(self.provider.clone()),
            model: Some(self.model.clone()),
            round: Some(0),
            tokens: None,
            cost: Some(0.0),
            session_id: Some(session_id_short.clone()),
            elapsed_ms: Some(0),
            tool_count: Some(0),
            input_tokens: Some(0),
            output_tokens: Some(0),
        });

        let session_start = std::time::Instant::now();

        // Agent message loop: wait for prompts from TUI, process each.
        loop {
            let text = match prompt_rx.recv().await {
                Some(t) => t,
                None => break, // TUI closed the channel.
            };

            // Route slash commands before sending to agent.
            let trimmed = text.trim();
            if let Some(cmd) = trimmed.strip_prefix('/') {
                match commands::handle(cmd, &self.provider, &self.model) {
                    commands::CommandResult::Exit => {
                        let _ = ui_tx.send(UiEvent::Quit);
                        break;
                    }
                    commands::CommandResult::Continue => {
                        // Commands that printed to stdout (help, model, clear)
                        // already wrote to stdout which is captured by alternate screen.
                        // Re-render the help text in the activity zone instead.
                        let (c, _) = cmd.split_once(' ').unwrap_or((cmd, ""));
                        match c {
                            "help" | "h" | "?" => {
                                let _ = ui_tx.send(UiEvent::Info("─── Help ───".into()));
                                let _ = ui_tx.send(UiEvent::Info("/help        Show this help".into()));
                                let _ = ui_tx.send(UiEvent::Info("/model       Show current provider/model".into()));
                                let _ = ui_tx.send(UiEvent::Info("/session     Show session info".into()));
                                let _ = ui_tx.send(UiEvent::Info("/status      Live system status".into()));
                                let _ = ui_tx.send(UiEvent::Info("/metrics     Token/cost metrics".into()));
                                let _ = ui_tx.send(UiEvent::Info("/clear       Clear activity zone".into()));
                                let _ = ui_tx.send(UiEvent::Info("/quit        Exit cuervo".into()));
                                let _ = ui_tx.send(UiEvent::Info("────────────".into()));
                            }
                            "model" => {
                                let _ = ui_tx.send(UiEvent::Info(format!(
                                    "Current: {}/{}",
                                    self.provider, self.model
                                )));
                            }
                            _ => {}
                        }
                    }
                    commands::CommandResult::Unknown(c) => {
                        let _ = ui_tx.send(UiEvent::Warning {
                            message: format!("Unknown command '/{c}'"),
                            hint: Some("Type /help for available commands".into()),
                        });
                    }
                    commands::CommandResult::SessionShow => {
                        let info = format!(
                            "Session {} | {} rounds | ↑{} ↓{} tokens | ${:.4} | {} tools",
                            &self.session.id.to_string()[..8],
                            self.session.agent_rounds,
                            self.session.total_usage.input_tokens,
                            self.session.total_usage.output_tokens,
                            self.session.estimated_cost_usd,
                            self.session.tool_invocations,
                        );
                        let _ = ui_tx.send(UiEvent::Info(info));
                    }
                    commands::CommandResult::LiveStatus => {
                        let info = format!(
                            "Provider: {} | Model: {} | Rounds: {} | Cost: ${:.4}",
                            self.provider, self.model,
                            self.session.agent_rounds,
                            self.session.estimated_cost_usd,
                        );
                        let _ = ui_tx.send(UiEvent::Info(info));
                    }
                    commands::CommandResult::Metrics => {
                        let total = self.session.total_usage.total();
                        let info = format!(
                            "Tokens: {} total (↑{} ↓{}) | Cost: ${:.4} | Latency: {:.1}s",
                            total,
                            self.session.total_usage.input_tokens,
                            self.session.total_usage.output_tokens,
                            self.session.estimated_cost_usd,
                            self.session.total_latency_ms as f64 / 1000.0,
                        );
                        let _ = ui_tx.send(UiEvent::Info(info));
                    }
                    _ => {
                        let _ = ui_tx.send(UiEvent::Warning {
                            message: format!("Command '/{cmd}' not available in TUI mode"),
                            hint: Some("Use classic mode (cuervo chat) for full command access".into()),
                        });
                    }
                }
                let _ = ui_tx.send(UiEvent::AgentDone);
                continue;
            }

            // Send RoundStart to TUI.
            let _ = ui_tx.send(UiEvent::RoundStart((self.session.agent_rounds + 1) as usize));

            // Handle the message through the full agent loop with TuiSink.
            if let Err(e) = self.handle_message_tui(&text, &tui_sink).await {
                let _ = ui_tx.send(UiEvent::Error {
                    message: format!("Agent error: {e}"),
                    hint: None,
                });
            }

            // Send post-round status update with accumulated session metrics.
            let _ = ui_tx.send(UiEvent::StatusUpdate {
                provider: Some(self.provider.clone()),
                model: Some(self.model.clone()),
                round: Some(self.session.agent_rounds as usize),
                tokens: Some(self.session.total_usage.total() as u64),
                cost: Some(self.session.estimated_cost_usd),
                session_id: None, // Already set.
                elapsed_ms: Some(session_start.elapsed().as_millis() as u64),
                tool_count: Some(self.session.tool_invocations),
                input_tokens: Some(self.session.total_usage.input_tokens),
                output_tokens: Some(self.session.total_usage.output_tokens),
            });

            // Signal agent done.
            let _ = ui_tx.send(UiEvent::AgentDone);

            // Auto-save session.
            self.auto_save_session().await;
        }

        // Wait for TUI task to finish.
        let _ = tui_handle.await;

        // Emit SessionEnded event (matches classic REPL behavior).
        let _ = self.event_tx.send(DomainEvent::new(EventPayload::SessionEnded {
            session_id: self.session.id,
            total_usage: self.session.total_usage.clone(),
        }));

        self.save_session();
        Ok(())
    }

    /// Handle a message using a TuiSink (same as handle_message but with custom sink).
    #[cfg(feature = "tui")]
    async fn handle_message_tui(
        &mut self,
        input: &str,
        tui_sink: &crate::render::sink::TuiSink,
    ) -> Result<()> {
        self.handle_message_with_sink(input, tui_sink).await
    }

    /// Print a brief session summary on exit.
    fn print_session_summary(&self) {
        // Emit SessionEnded event.
        let _ = self.event_tx.send(DomainEvent::new(EventPayload::SessionEnded {
            session_id: self.session.id,
            total_usage: self.session.total_usage.clone(),
        }));

        if self.session.agent_rounds == 0 {
            return; // No interactions, nothing to summarize.
        }

        let t = crate::render::theme::active();
        let r = crate::render::theme::reset();
        let dim = t.palette.text_dim.fg();
        let accent = t.palette.accent.fg();

        let latency = self.session.total_latency_ms as f64 / 1000.0;
        let cost_str = if self.session.estimated_cost_usd > 0.0 {
            format!(" | ${:.4}", self.session.estimated_cost_usd)
        } else {
            String::new()
        };
        let tools_str = if self.session.tool_invocations > 0 {
            format!(" | {} tools", self.session.tool_invocations)
        } else {
            String::new()
        };
        eprintln!(
            "\n{dim}Session:{r} {} rounds | {:.1}s{}{} | {accent}{}{r}",
            self.session.agent_rounds,
            latency,
            cost_str,
            tools_str,
            &self.session.id.to_string()[..8],
        );
    }

    fn print_welcome(&self) {
        let provider_connected = self.registry.get(&self.provider).is_some();
        let session_short = &self.session.id.to_string()[..8];
        let session_type = if self.session.messages.is_empty() {
            "new"
        } else {
            "resumed"
        };

        let routing = if !self.config.agent.routing.fallback_models.is_empty() {
            Some(crate::render::banner::RoutingDisplay {
                mode: self.config.agent.routing.mode.clone(),
                strategy: self.config.agent.routing.strategy.clone(),
                fallback_chain: std::iter::once(self.provider.clone())
                    .chain(self.config.agent.routing.fallback_models.clone())
                    .collect(),
            })
        } else {
            None
        };

        let show = !self.no_banner
            && crate::render::banner::should_show(self.config.display.show_banner);

        if show {
            // Deterministic tip index from session ID.
            let tip_index = self.session.id.as_u128() as usize;
            crate::render::banner::render_startup(
                env!("CARGO_PKG_VERSION"),
                &self.provider,
                provider_connected,
                &self.model,
                session_short,
                session_type,
                tip_index,
                routing.as_ref(),
            );
        } else {
            let fallback_count = routing.as_ref().map(|r| r.fallback_chain.len());
            crate::render::banner::render_minimal(
                env!("CARGO_PKG_VERSION"),
                &self.provider,
                &self.model,
                fallback_count,
            );
        }

        // Warn if primary provider is not available.
        if !provider_connected {
            crate::render::feedback::user_warning(
                &format!("no API key configured for '{}'", self.provider),
                Some(&format!("Run `cuervo auth login {}` to set up", self.provider)),
            );
        }
    }

    /// Handle a /dry-run command: routes through the agent loop with DestructiveOnly mode.
    async fn handle_message_dry_run(&mut self, input: &str) -> Result<()> {
        use crate::render::sink::RenderSink;
        let sink = crate::render::sink::ClassicSink::with_expert(self.expert_mode);
        sink.info("[dry-run] Destructive tools will be skipped.");
        self.dry_run_override = Some(cuervo_core::types::DryRunMode::DestructiveOnly);
        self.handle_message(input).await
    }

    async fn handle_message(&mut self, input: &str) -> Result<()> {
        let classic_sink = crate::render::sink::ClassicSink::with_expert(self.expert_mode);
        self.handle_message_with_sink(input, &classic_sink).await?;
        println!();
        Ok(())
    }

    /// Unified message handler — runs the full agent loop with any RenderSink.
    ///
    /// Both classic REPL and TUI modes delegate here. The sink parameter
    /// abstracts away the rendering backend.
    async fn handle_message_with_sink(
        &mut self,
        input: &str,
        sink: &dyn crate::render::sink::RenderSink,
    ) -> Result<()> {
        // Record user message in session.
        self.session.add_message(ChatMessage {
            role: Role::User,
            content: MessageContent::Text(input.to_string()),
        });

        // Assemble context from all sources (instructions + memory).
        let working_dir = self
            .config
            .general
            .working_directory
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            });

        let context_query = ContextQuery {
            working_directory: working_dir.clone(),
            user_message: Some(input.to_string()),
            token_budget: self.config.general.max_tokens as usize,
        };

        let raw_chunks =
            cuervo_context::assemble_context(&self.context_sources, &context_query).await;

        // Apply governance (per-source token limits + provenance tracking).
        let (chunks, _provenance) = self.context_governance.apply(raw_chunks);
        // Record governance truncations in metrics.
        let truncations = _provenance
            .iter()
            .filter(|p| p.token_count > 0)
            .count();
        if truncations > 0 {
            self.context_metrics.record_source_invocations(truncations as u64);
        }

        let system_prompt = if chunks.is_empty() {
            None
        } else {
            Some(cuervo_context::chunks_to_system_prompt(&chunks))
        };

        // Build the model request from session history.
        let tool_defs = self.tool_registry.tool_definitions();
        let request = ModelRequest {
            model: self.model.clone(),
            messages: self.session.messages.clone(),
            tools: tool_defs,
            max_tokens: Some(self.config.general.max_tokens),
            temperature: Some(self.config.general.temperature),
            system: system_prompt,
            stream: true,
        };

        // Look up the active provider.
        let provider: Option<Arc<dyn ModelProvider>> = self.registry.get(&self.provider).cloned();

        // Pre-loop reasoning: analyze query and select strategy.
        let _strategy_plan = if let Some(engine) = &mut self.reasoning_engine {
            engine.reset_retries();
            let plan = engine.pre_loop(input);
            sink.info(&format!(
                "  [reasoning] strategy: {:?}, planner: {}",
                plan.kind, if plan.use_planner { "yes" } else { "no" },
            ));
            // Phase 43D: Emit reasoning info for TUI panel.
            if let Some(record) = engine.history().last() {
                sink.reasoning_update(
                    &format!("{:?}", record.strategy),
                    &format!("{:?}", record.analysis.task_type),
                    &format!("{:?}", record.analysis.complexity),
                );
            }
            Some(plan)
        } else {
            None
        };

        match provider {
            Some(p) => {
                // Build fallback providers from routing config.
                let fallback_providers: Vec<(String, Arc<dyn ModelProvider>)> = self
                    .config
                    .agent
                    .routing
                    .fallback_models
                    .iter()
                    .filter_map(|name| {
                        self.registry
                            .get(name)
                            .cloned()
                            .map(|p| (name.clone(), p))
                    })
                    .collect();

                let compactor = compaction::ContextCompactor::new(
                    self.config.agent.compaction.clone(),
                );
                let guardrails: &[Box<dyn cuervo_security::Guardrail>] =
                    if self.config.security.guardrails.enabled
                        && self.config.security.guardrails.builtins
                    {
                        cuervo_security::builtin_guardrails()
                    } else {
                        &[]
                    };

                let llm_planner = if self.config.planning.adaptive {
                    Some(planner::LlmPlanner::new(
                        Arc::clone(&p),
                        self.model.clone(),
                    ).with_max_replans(self.config.planning.max_replans))
                } else {
                    None
                };

                // Skip model selection when user explicitly set --model on the CLI.
                let selector = if self.config.agent.model_selection.enabled && !self.explicit_model {
                    Some(model_selector::ModelSelector::new(
                        self.config.agent.model_selection.clone(),
                        &self.registry,
                    ).with_provider_scope(p.name()))
                } else {
                    None
                };

                // Create task bridge when structured task framework is enabled.
                let mut task_bridge_inst = if self.config.task_framework.enabled {
                    Some(task_bridge::TaskBridge::new(&self.config.task_framework))
                } else {
                    None
                };

                let ctx = agent::AgentContext {
                    provider: &p,
                    session: &mut self.session,
                    request: &request,
                    tool_registry: &self.tool_registry,
                    permissions: &mut self.permissions,
                    working_dir: &working_dir,
                    event_tx: &self.event_tx,
                    trace_db: self.async_db.as_ref(),
                    limits: &self.config.agent.limits,
                    response_cache: self.response_cache.as_ref(),
                    resilience: &mut self.resilience,
                    fallback_providers: &fallback_providers,
                    routing_config: &self.config.agent.routing,
                    compactor: Some(&compactor),
                    planner: llm_planner.as_ref().map(|p| p as &dyn Planner),
                    guardrails,
                    reflector: self.reflector.as_ref(),
                    render_sink: sink,
                    replay_tool_executor: None,
                    phase14: cuervo_core::types::Phase14Context {
                        dry_run_mode: self.dry_run_override.take().unwrap_or_default(),
                        ..Default::default()
                    },
                    model_selector: selector.as_ref(),
                    registry: Some(&self.registry),
                    episode_id: Some(uuid::Uuid::new_v4()),
                    planning_config: &self.config.planning,
                    orchestrator_config: &self.config.orchestrator,
                    tool_selection_enabled: self.config.context.dynamic_tool_selection,
                    task_bridge: task_bridge_inst.as_mut(),
                    reasoning_config: self.reasoning_engine.as_ref().map(|e| e.config()),
                    context_metrics: Some(&self.context_metrics),
                    // Phase 43: pass control channel receiver from TUI (if present).
                    #[cfg(feature = "tui")]
                    ctrl_rx: self.ctrl_rx.take(),
                    #[cfg(not(feature = "tui"))]
                    ctrl_rx: None,
                };
                let mut result = agent::run_agent_loop(ctx).await?;

                // Phase 43: restore control channel receiver for reuse across TUI messages.
                #[cfg(feature = "tui")]
                {
                    self.ctrl_rx = result.ctrl_rx.take();
                }

                // Cache timeline for --timeline exit hook.
                self.last_timeline = result.timeline_json.clone();

                // Post-loop reasoning: evaluate result and persist experience.
                if let Some(engine) = &mut self.reasoning_engine {
                    let eval = engine.post_loop(&result);
                    sink.reasoning_status(
                        &engine.history().last().map(|r| format!("{:?}", r.analysis.task_type)).unwrap_or_default(),
                        &engine.history().last().map(|r| format!("{:?}", r.analysis.complexity)).unwrap_or_default(),
                        &engine.history().last().map(|r| format!("{:?}", r.strategy)).unwrap_or_default(),
                        eval.score,
                        eval.success,
                    );
                    // Persist experience to DB.
                    if self.config.reasoning.learning_enabled {
                        if let Some(ref adb) = self.async_db {
                            for record in engine.experience() {
                                let _ = adb.inner().save_experience(
                                    &format!("{:?}", record.task_type),
                                    &format!("{:?}", record.strategy),
                                    record.avg_score,
                                    record.uses,
                                    record.last_score,
                                    None,
                                );
                            }
                        }
                    }
                }

                // Display result summary via sink.
                let total_tokens = result.input_tokens + result.output_tokens;
                if total_tokens > 0 || result.latency_ms > 0 {
                    let cost_str = if result.cost_usd > 0.0 {
                        format!(" | ${:.4}", result.cost_usd)
                    } else {
                        String::new()
                    };
                    let rounds_str = if result.rounds > 0 {
                        format!(
                            " | {} tool {}",
                            result.rounds,
                            if result.rounds == 1 { "round" } else { "rounds" },
                        )
                    } else {
                        String::new()
                    };
                    sink.info(&format!(
                        "  [{} tokens | {:.1}s{}{}]",
                        total_tokens,
                        result.latency_ms as f64 / 1000.0,
                        cost_str,
                        rounds_str,
                    ));
                }

                // Auto-consolidate reflections after each agent interaction.
                if let Some(ref adb) = self.async_db {
                    sink.consolidation_status("consolidating reflections...");
                    memory_consolidator::maybe_consolidate(adb).await;
                }
            }
            None => {
                sink.error(
                    &format!("provider '{}' not configured", self.provider),
                    Some("Set API key or check config"),
                );
                // Add placeholder to session so it's visible in session history.
                self.session.add_message(ChatMessage {
                    role: Role::Assistant,
                    content: MessageContent::Text(format!(
                        "[provider '{}' not configured] — Set API key or check config.",
                        self.provider,
                    )),
                });
            }
        }

        Ok(())
    }

    /// Fire-and-forget async session save (crash protection after each message).
    async fn auto_save_session(&self) {
        if self.session.messages.is_empty() {
            return;
        }
        if let Some(ref adb) = self.async_db {
            if let Err(e) = adb.save_session(&self.session).await {
                tracing::warn!("Auto-save session failed: {e}");
                crate::render::feedback::user_warning(
                    &format!("session auto-save failed — {e}"),
                    Some("Session data may be lost if process exits"),
                );
            }
        }
    }

    fn save_session(&self) {
        if self.session.messages.is_empty() {
            return;
        }
        if let Some(db) = &self.db {
            if let Err(e) = db.save_session(&self.session) {
                crate::render::feedback::user_warning(
                    &format!("failed to save session — {e}"),
                    None,
                );
            } else {
                tracing::debug!("Session {} saved", self.session.id);
            }

            // Auto-summarize session to memory (extractive, no LLM call).
            if self.config.memory.enabled && self.config.memory.auto_summarize {
                self.summarize_session_to_memory(db);
            }
        }
    }

    fn summarize_session_to_memory(&self, db: &Database) {
        use cuervo_storage::{MemoryEntry, MemoryEntryType};
        use sha2::{Digest, Sha256};

        // Build an extractive summary from user messages.
        let user_messages: Vec<&str> = self
            .session
            .messages
            .iter()
            .filter(|m| m.role == Role::User)
            .filter_map(|m| match &m.content {
                MessageContent::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();

        if user_messages.is_empty() {
            return;
        }

        let topic_preview: String = user_messages
            .iter()
            .take(3)
            .map(|m| {
                let trimmed: String = m.chars().take(100).collect();
                trimmed.replace('\n', " ")
            })
            .collect::<Vec<_>>()
            .join("; ");

        let summary = format!(
            "Session {}: {} messages, {} user turns. Topics: {}",
            &self.session.id.to_string()[..8],
            self.session.messages.len(),
            user_messages.len(),
            topic_preview,
        );

        let hash = hex::encode(Sha256::digest(summary.as_bytes()));

        let entry = MemoryEntry {
            entry_id: uuid::Uuid::new_v4(),
            session_id: Some(self.session.id),
            entry_type: MemoryEntryType::SessionSummary,
            content: summary,
            content_hash: hash,
            metadata: serde_json::json!({
                "model": self.model,
                "provider": self.provider,
                "message_count": self.session.messages.len(),
                "tokens": self.session.total_usage.input_tokens + self.session.total_usage.output_tokens,
            }),
            created_at: chrono::Utc::now(),
            expires_at: self.config.memory.default_ttl_days.map(|days| {
                chrono::Utc::now() + chrono::Duration::days(days as i64)
            }),
            relevance_score: 0.8,
        };

        match db.insert_memory(&entry) {
            Ok(true) => {
                tracing::debug!("Session summary stored in memory");
            }
            Ok(false) => {
                tracing::debug!("Session summary already exists (duplicate hash)");
            }
            Err(e) => {
                tracing::warn!("Failed to store session summary: {e}");
            }
        }
    }


    fn history_path() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".cuervo").join("history.txt"))
    }

    /// Return the last execution timeline as JSON, if a plan was generated.
    ///
    /// Used by `--timeline` flag. Returns None if no plan was executed in this session.
    pub fn last_timeline_json(&self) -> Option<String> {
        self.last_timeline.clone()
    }

    /// Expose session ID for testing.
    #[cfg(test)]
    pub fn session_id(&self) -> uuid::Uuid {
        self.session.id
    }

    /// Expose session message count for testing.
    #[cfg(test)]
    pub fn message_count(&self) -> usize {
        self.session.messages.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cuervo_core::types::AppConfig;

    fn test_db() -> Arc<Database> {
        Arc::new(Database::open_in_memory().unwrap())
    }

    fn test_config() -> AppConfig {
        AppConfig::default()
    }

    fn test_registry() -> ProviderRegistry {
        ProviderRegistry::new()
    }

    fn test_tool_registry() -> ToolRegistry {
        ToolRegistry::new()
    }

    fn test_event_tx() -> EventSender {
        cuervo_core::event_bus(16).0
    }

    #[test]
    fn repl_creates_session() {
        let config = test_config();
        let repl = Repl::new(
            &config,
            "test".into(),
            "test-model".into(),
            Some(test_db()),
            None,
            test_registry(),
            test_tool_registry(),
            test_event_tx(),
            false,
            false,
        )
        .unwrap();
        assert_eq!(repl.message_count(), 0);
    }

    #[test]
    fn session_save_and_load_roundtrip() {
        let config = test_config();
        let db = test_db();

        // Create REPL, add a message, save.
        let mut session = Session::new("test-model".into(), "test".into(), "/tmp".into());
        session.add_message(ChatMessage {
            role: Role::User,
            content: MessageContent::Text("hello".into()),
        });
        let id = session.id;
        db.save_session(&session).unwrap();

        // Reload via resume.
        let repl = Repl::new(
            &config,
            "test".into(),
            "test-model".into(),
            Some(test_db()),
            Some(db.load_session(id).unwrap().unwrap()),
            test_registry(),
            test_tool_registry(),
            test_event_tx(),
            false,
            false,
        )
        .unwrap();
        assert_eq!(repl.session_id(), id);
        assert_eq!(repl.message_count(), 1);
    }

    #[test]
    fn session_not_saved_if_empty() {
        let config = test_config();
        let db = test_db();
        let repl = Repl::new(
            &config,
            "test".into(),
            "test-model".into(),
            Some(db),
            None,
            test_registry(),
            test_tool_registry(),
            test_event_tx(),
            false,
            false,
        )
        .unwrap();
        // save_session should be a no-op (no messages).
        repl.save_session();
        // Verify no sessions in DB.
        let db2 = test_db();
        let sessions = db2.list_sessions(10).unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn list_sessions_without_db() {
        let config = test_config();
        let repl = Repl::new(
            &config,
            "test".into(),
            "model".into(),
            None,
            None,
            test_registry(),
            test_tool_registry(),
            test_event_tx(),
            false,
            false,
        )
        .unwrap();
        // Should not panic.
        repl.list_sessions();
    }

    #[test]
    fn show_session_info() {
        let config = test_config();
        let repl = Repl::new(
            &config,
            "anthropic".into(),
            "sonnet".into(),
            None,
            None,
            test_registry(),
            test_tool_registry(),
            test_event_tx(),
            false,
            false,
        )
        .unwrap();
        // Should not panic.
        repl.show_session();
    }

    #[tokio::test]
    async fn handle_message_with_echo_provider() {
        let config = test_config();
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(cuervo_providers::EchoProvider::new()));

        let mut repl = Repl::new(
            &config,
            "echo".into(),
            "echo".into(),
            None,
            None,
            registry,
            test_tool_registry(),
            test_event_tx(),
            false,
            false,
        )
        .unwrap();

        repl.handle_message("hello world").await.unwrap();

        // Session should have 2 messages (user + assistant).
        assert_eq!(repl.message_count(), 2);
        // Token usage should be updated.
        assert!(repl.session.total_usage.output_tokens > 0);
    }

    #[tokio::test]
    async fn handle_message_no_provider_shows_placeholder() {
        let config = test_config();
        let registry = ProviderRegistry::new();

        let mut repl = Repl::new(
            &config,
            "missing".into(),
            "some-model".into(),
            None,
            None,
            registry,
            test_tool_registry(),
            test_event_tx(),
            false,
            false,
        )
        .unwrap();

        repl.handle_message("test").await.unwrap();

        // Session should have 2 messages (user + placeholder).
        assert_eq!(repl.message_count(), 2);
    }

    #[test]
    fn session_auto_summarize_to_memory() {
        let config = test_config();
        let db = test_db();

        let mut repl = Repl::new(
            &config,
            "test".into(),
            "test-model".into(),
            Some(Arc::clone(&db)),
            None,
            test_registry(),
            test_tool_registry(),
            test_event_tx(),
            false,
            false,
        )
        .unwrap();

        // Add some messages so save_session triggers summarization.
        repl.session.add_message(ChatMessage {
            role: Role::User,
            content: MessageContent::Text("What is Rust?".into()),
        });
        repl.session.add_message(ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Text("Rust is a systems programming language.".into()),
        });

        repl.save_session();

        // Memory should have a session summary.
        let stats = db.memory_stats().unwrap();
        assert_eq!(stats.total_entries, 1);

        let entries = db
            .list_memories(Some(cuervo_storage::MemoryEntryType::SessionSummary), 10)
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].content.contains("What is Rust?"));
        assert!(entries[0].session_id.is_some());
    }

    #[test]
    fn session_summarize_disabled_when_config_off() {
        let mut config = test_config();
        config.memory.auto_summarize = false;
        let db = test_db();

        let mut repl = Repl::new(
            &config,
            "test".into(),
            "test-model".into(),
            Some(Arc::clone(&db)),
            None,
            test_registry(),
            test_tool_registry(),
            test_event_tx(),
            false,
            false,
        )
        .unwrap();

        repl.session.add_message(ChatMessage {
            role: Role::User,
            content: MessageContent::Text("test".into()),
        });

        repl.save_session();

        // No memory entry should be created.
        let stats = db.memory_stats().unwrap();
        assert_eq!(stats.total_entries, 0);
    }

    #[test]
    fn context_sources_include_memory_when_enabled() {
        let config = test_config();
        let db = test_db();

        let repl = Repl::new(
            &config,
            "test".into(),
            "test-model".into(),
            Some(db),
            None,
            test_registry(),
            test_tool_registry(),
            test_event_tx(),
            false,
            false,
        )
        .unwrap();

        // Should have 5 context sources: instructions + repo_map + planning + episodic_memory + reflections.
        // (episodic=true and reflexion.enabled=true by default)
        assert_eq!(repl.context_sources.len(), 5);
        assert_eq!(repl.context_sources[0].name(), "instructions");
        assert_eq!(repl.context_sources[1].name(), "repo_map");
        assert_eq!(repl.context_sources[2].name(), "planning");
        assert_eq!(repl.context_sources[3].name(), "episodic_memory");
        assert_eq!(repl.context_sources[4].name(), "reflections");
    }

    #[test]
    fn context_sources_no_memory_when_disabled() {
        let mut config = test_config();
        config.memory.enabled = false;
        config.reflexion.enabled = false;
        let db = test_db();

        let repl = Repl::new(
            &config,
            "test".into(),
            "test-model".into(),
            Some(db),
            None,
            test_registry(),
            test_tool_registry(),
            test_event_tx(),
            false,
            false,
        )
        .unwrap();

        // Should have 3 context sources: instructions + repo_map + planning (no memory, no reflections).
        assert_eq!(repl.context_sources.len(), 3);
        assert_eq!(repl.context_sources[0].name(), "instructions");
        assert_eq!(repl.context_sources[1].name(), "repo_map");
        assert_eq!(repl.context_sources[2].name(), "planning");
    }

    #[test]
    fn context_sources_no_memory_without_db() {
        let config = test_config();

        let repl = Repl::new(
            &config,
            "test".into(),
            "test-model".into(),
            None,
            None,
            test_registry(),
            test_tool_registry(),
            test_event_tx(),
            false,
            false,
        )
        .unwrap();

        // No DB => no memory or reflections source. Still has instructions + repo_map + planning.
        assert_eq!(repl.context_sources.len(), 3);
        assert_eq!(repl.context_sources[0].name(), "instructions");
        assert_eq!(repl.context_sources[1].name(), "repo_map");
        assert_eq!(repl.context_sources[2].name(), "planning");
    }

    #[test]
    fn context_sources_no_planning_when_disabled() {
        let mut config = test_config();
        config.planning.enabled = false;
        config.memory.enabled = false;
        config.reflexion.enabled = false;

        let repl = Repl::new(
            &config,
            "test".into(),
            "test-model".into(),
            None,
            None,
            test_registry(),
            test_tool_registry(),
            test_event_tx(),
            false,
            false,
        )
        .unwrap();

        // Instructions + repo_map when planning, memory, and reflexion disabled.
        assert_eq!(repl.context_sources.len(), 2);
        assert_eq!(repl.context_sources[0].name(), "instructions");
        assert_eq!(repl.context_sources[1].name(), "repo_map");
    }

    // --- Phase 4C: Integration wiring tests ---

    #[test]
    fn resilience_registers_all_providers() {
        let config = test_config();
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(cuervo_providers::EchoProvider::new()));

        let repl = Repl::new(
            &config,
            "echo".into(),
            "echo".into(),
            Some(test_db()),
            None,
            registry,
            test_tool_registry(),
            test_event_tx(),
            false,
            false,
        )
        .unwrap();

        // Resilience diagnostics should list all registered providers.
        let diag = repl.resilience.diagnostics();
        let names: Vec<&str> = diag.iter().map(|d| d.provider.as_str()).collect();
        assert!(
            names.contains(&"echo"),
            "resilience should register 'echo' provider: {names:?}"
        );
    }

    #[test]
    fn resilience_registers_multiple_providers() {
        let config = test_config();
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(cuervo_providers::EchoProvider::new()));
        registry.register(Arc::new(cuervo_providers::OllamaProvider::new(
            None,
            cuervo_core::types::HttpConfig::default(),
        )));

        let repl = Repl::new(
            &config,
            "echo".into(),
            "echo".into(),
            None,
            None,
            registry,
            test_tool_registry(),
            test_event_tx(),
            false,
            false,
        )
        .unwrap();

        let diag = repl.resilience.diagnostics();
        let names: Vec<&str> = diag.iter().map(|d| d.provider.as_str()).collect();
        assert!(
            names.contains(&"echo"),
            "resilience should register echo: {names:?}"
        );
        assert!(
            names.contains(&"ollama"),
            "resilience should register ollama: {names:?}"
        );
    }

    #[tokio::test]
    async fn end_to_end_failover_to_echo() {
        use cuervo_core::types::{BackpressureConfig, CircuitBreakerConfig, ResilienceConfig};

        let config = test_config();
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(cuervo_providers::EchoProvider::new()));

        let mut repl = Repl::new(
            &config,
            "echo".into(),
            "echo".into(),
            None,
            None,
            registry,
            test_tool_registry(),
            test_event_tx(),
            false,
            false,
        )
        .unwrap();

        // Override resilience to test failover path.
        let mut resilience = ResilienceManager::new(ResilienceConfig {
            enabled: true,
            circuit_breaker: CircuitBreakerConfig {
                failure_threshold: 100, // high threshold so echo won't trip
                ..Default::default()
            },
            health: Default::default(),
            backpressure: BackpressureConfig::default(),
        });
        resilience.register_provider("echo");
        repl.resilience = resilience;

        // Should succeed with echo provider even through resilience.
        repl.handle_message("integration test").await.unwrap();
        assert_eq!(repl.message_count(), 2); // user + assistant
    }
}
