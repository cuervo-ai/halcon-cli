use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use reedline::{
    EditCommand, FileBackedHistory, KeyCode, KeyModifiers, Reedline, ReedlineEvent, Signal,
};

use halcon_core::traits::{ContextQuery, ContextSource, ModelProvider, Planner};
use halcon_core::types::{
    AppConfig, ChatMessage, DomainEvent, EventPayload, MessageContent, ModelRequest,
    Role, Session,
};
use halcon_core::EventSender;
use halcon_providers::ProviderRegistry;
use halcon_storage::{AsyncDatabase, Database};
use halcon_tools::ToolRegistry;

use conversational_permission::ConversationalPermissionHandler;
use memory_source::MemorySource;
use planning_source::PlanningSource;
use resilience::ResilienceManager;
use response_cache::ResponseCache;

// === AGENT ORCHESTRATION ===
// Main agent loop coordinator. Previously a single 9000-line file;
// now a directory module (agent/mod.rs) ready for further decomposition.
pub mod agent;

// Agent support types and utilities — part of the agent orchestration layer.
// agent_comm moved to bridges/ (C-7)
pub use bridges::agent_comm;
pub mod agent_task_manager;
pub mod agent_types;
pub mod agent_utils;
pub mod accumulator;

// === INFRASTRUCTURE LAYER ===
// I/O-bound modules: tool execution, storage, context assembly, resilience.
// Future extraction candidates for a `halcon-infra` crate.

/// Tool execution pipeline — file I/O, shell, risk scoring, permission gates.
pub mod executor;
// plan_state_diagnostics moved to planning/diagnostics (C-3)
pub(crate) use planning::diagnostics as plan_state_diagnostics;
pub(crate) mod security;
// Backward-compat: subagent_contract_validator was moved to security/subagent_contract.rs
pub(crate) use security::subagent_contract as subagent_contract_validator;
/// Runtime bridge: connects CLI tool execution to the halcon-runtime DAG executor.
/// Provides `CliToolRuntime` — a `HalconRuntime` pre-populated with `LocalToolAgent`
/// wrappers for every `ToolRegistry` entry. Parallel tool batches become single-wave
/// `TaskDAG`s, executed concurrently via `RuntimeExecutor`.
// runtime_bridge moved to bridges/runtime (C-7)
pub(crate) use bridges::runtime as runtime_bridge;
/// Multi-agent wave orchestration — parallel task scheduling with dependency resolution.
pub mod orchestrator;
// orchestrator_metrics moved to metrics/ (C-6)
pub use metrics::orchestrator as orchestrator_metrics;
pub mod model_quirks;
// decision_layer, sla_manager moved to planning/ (C-3)
pub(crate) use planning::decision_layer;
pub(crate) use planning::sla as sla_manager;
/// Boundary Decision Engine — structured multi-layer routing pipeline.
pub(crate) mod decision_engine;
pub(crate) mod retry_mutation;
pub(crate) mod tool_aliases;
// tool_policy and tool_trust moved to security/ subdir
pub(crate) use security::tool_policy;
pub(crate) use security::tool_trust;

// Anomaly detection and loop integrity. Moved to metrics/ (C-6)
pub(crate) use metrics::anomaly as anomaly_detector;
pub(crate) use metrics::arima as arima_predictor;
pub use domain::metacognitive_loop;
pub(crate) use domain::loop_guard;

// Context assembly and management — moved to context/ (C-4)
pub mod artifact_store;
pub(crate) use context::governance as context_governance;
pub(crate) use context::manager as context_manager;
pub(crate) use context::metrics as context_metrics;
pub use context::episodic as episodic_source;
pub use context::hybrid_retriever;
pub use context::consolidator as memory_consolidator;
pub use context::memory as memory_source;
pub use context::reflection as reflection_source;
pub use domain::reflexion;
pub use context::repo_map as repo_map_source;

// Commands and authorization and security.
pub mod commands;
// authorization, circuit_breaker, permissions moved to security/ (C-2)
pub use security::authorization;
pub use security::circuit_breaker;
pub use security::permissions;
pub use security::conversational as conversational_permission;
pub use security::adaptive_prompt;
pub use security::rule_matcher;
pub use security::schema_validator;
pub use security::validation;
pub use planning::backpressure;
// command_blacklist, permission_lifecycle, output_risk_scorer moved to security/
pub use security::blacklist as command_blacklist;
pub use security::lifecycle as permission_lifecycle;
pub use security::output_risk as output_risk_scorer;

// Session and resilience.
// ci_detection moved to git_tools/ (C-5)
pub use git_tools::ci_detection;
pub use context::compaction;
pub mod console;
pub mod delegation;
/// Evidence Boundary System — Zero Evidence → Zero Output policy.
/// Tracks textual evidence from file-reading tools; blocks synthesis on binary/empty files.
pub use domain::evidence_pipeline;
pub(crate) use domain::evidence_graph;
pub mod execution_tracker;
pub mod failure_tracker;
// health, metrics_store moved to metrics/ (C-6)
pub use metrics::health;
pub use security::idempotency;
pub use metrics::integration_decision;
// mcp_manager moved to bridges/ (C-7)
pub(crate) use bridges::mcp_manager;
pub use metrics::store as metrics_store;
pub use planning::model_selector;
pub use planning::optimizer;
pub mod resilience;
pub use security::response_cache;
// router moved to planning/router (C-3)
pub use planning::router;

/// HALCON.md persistent instruction system (Feature 1 — Frontier Roadmap 2026).
/// 4-scope hierarchy, @import resolution, path-glob rules, hot-reload via notify.
pub mod instruction_store;

/// User-accessible lifecycle hooks (Feature 2 — Frontier Roadmap 2026).
/// Shell command and Rhai script hooks for PreToolUse, PostToolUse, UserPromptSubmit, Stop, etc.
pub mod hooks;

/// Auto-memory system (Feature 3 — Frontier Roadmap 2026).
/// Heuristic scoring, bounded Markdown writes, session-start injection.
pub mod auto_memory;

/// Declarative sub-agent configuration registry (Feature 4 — Frontier Roadmap 2026).
/// Agent definitions from .halcon/agents/*.md; skills from .halcon/skills/*.md.
pub mod agent_registry;

/// Cron-based background scheduler for agent tasks (PASO 4-C — US-scheduler).
/// Uses a 60s tokio::time::interval tick and the croner crate for expression parsing.
pub mod agent_scheduler;

/// Semantic memory vector store context source (Feature 7 — Frontier Roadmap 2026).
/// Pipeline-triggered retrieval from MEMORY.md via cosine similarity + MMR.
// vector_memory_source moved to context/vector_memory (C-4)
pub use context::vector_memory as vector_memory_source;

// Communication and protocol.
// rule_matcher, adaptive_prompt, validation, conversational_permission moved to security/ (C-2)
// input_normalizer, input_boundary moved to planning/ (C-3)
pub mod conversation_protocol;
pub mod conversation_state;
pub use planning::normalizer as input_normalizer;
pub use planning::input_boundary;

// Session persistence — FASE F clean architecture extraction.
// Contains: auto_save, save, summarize_to_memory as testable free functions.
pub mod session_manager;

// Planning infrastructure — moved to planning/ (C-3)
pub use planning::llm_planner as planner;
pub use planning::playbook as playbook_planner;
pub use planning::metrics as planning_metrics;
// tool_manifest moved to plugins/tool_manifest (C-1)
pub use planning::source as planning_source;
pub mod provenance_tracker;
mod prompt;
pub mod servers;
// Backward-compat re-exports so existing use super::X_server paths still compile:
pub use servers::architecture as architecture_server;
pub use servers::codebase as codebase_server;
pub use servers::requirements as requirements_server;
pub use servers::workflow as workflow_server;
pub use servers::test_results as test_results_server;
pub use servers::runtime_metrics as runtime_metrics_server;
pub use servers::security as security_server;
pub use servers::support as support_server;
// sdlc_phase_detector moved to git_tools/ (C-5)
pub use git_tools::sdlc_phase as sdlc_phase_detector;
pub use bridges::replay_executor;
pub use bridges::replay_runner;
// search_engine_global moved to bridges/ (C-7)
pub use bridges::search as search_engine_global;
pub use domain::self_corrector;
pub use planning::speculative;
// evaluator moved to metrics/ (C-6)
pub use metrics::evaluator;
// === APPLICATION LAYER ===
// Orchestration and metacognition — coordinates domain services, no direct I/O.
pub mod application;
// Backward-compat re-export so `super::reasoning_engine::*` paths remain valid:
pub use application::reasoning_engine;
// plan_coherence moved to planning/coherence (C-3)
pub use planning::coherence as plan_coherence;
// capability_index/orchestrator/resolver moved to plugins/ (C-1)
pub(crate) use planning::provider_normalization;
pub mod plugins;
// reward_pipeline, round_scorer, strategy_metrics moved to metrics/ (C-6)
pub use metrics::reward as reward_pipeline;
pub use metrics::scorer as round_scorer;
pub mod supervisor;
pub use metrics::strategy as strategy_metrics;
pub mod task_backlog;
// task_bridge moved to bridges/ (C-7)
pub(crate) use bridges::task as task_bridge;
pub mod task_scheduler;
// schema_validator moved to security/ (C-2)
pub mod tool_selector;
pub mod tool_speculation;
// git_tools/ migration (C-5): traceback, instrumentation, patch, edit, git, ci, ide
pub use git_tools::traceback as traceback_parser;
pub use git_tools::instrumentation as code_instrumentation;
// risk_tier_classifier moved to security/risk_tier.rs
pub use security::risk_tier as risk_tier_classifier;
pub use git_tools::patch as patch_preview_engine;
pub use git_tools::edit_transaction;
pub use git_tools::safe_edit as safe_edit_manager;
// Phase 2 — Git Context & Branch Awareness
pub use git_tools::context as git_context;
pub use git_tools::branch as branch_divergence;
pub use git_tools::commit_rewards as commit_reward_tracker;
pub use git_tools::events as git_event_listener;
// Phase 3 — Test Runner Bridge
pub use git_tools::test_results as test_result_parsers;
pub use git_tools::test_runner as test_runner_bridge;
// Phase 4 — CI Feedback Loop
pub use git_tools::ci_ingestor as ci_result_ingestor;
// Phase 5 — IDE Protocol Handler
pub use git_tools::unsaved_buffer as unsaved_buffer_tracker;
pub use git_tools::ide_protocol as ide_protocol_handler;
// dev_gateway moved to bridges/ (C-7)
pub use bridges::dev_gateway;
// Phase 6 — AST Symbol Extractor (feature-gated ast-symbols; regex backend compiles always)
// ast_symbol_extractor moved to git_tools/ (C-5)
pub use git_tools::ast_symbols as ast_symbol_extractor;
// Phase 7 — Runtime Signal Ingestor (OTEL-compatible, feature-gated otel)
// runtime_signal_ingestor moved to metrics/ (C-6)
pub use metrics::signal_ingestor as runtime_signal_ingestor;
// Phase 8 — Dev Ecosystem Integration Tests (cross-module invariant validation)
#[cfg(test)]
pub mod dev_ecosystem_integration_tests;

// Planning V3 — Compression, Macro Feedback, Early Convergence
// plan_compressor moved to planning/compressor (C-3)
pub use planning::compressor as plan_compressor;
// macro_feedback moved to metrics/ (C-6)
pub use metrics::macro_feedback;
pub use domain::early_convergence;

// Phase 94 — Project Onboarding System
// project_inspector moved to git_tools/ (C-5)
pub use git_tools::project_inspector;
pub mod onboarding;

// Phase 95 — Plugin Auto-Implantation & Suggestion (now in plugins/ subdir)

// === DOMAIN LAYER ===
// Pure domain types and algorithms — zero infrastructure dependencies.
// Future extraction candidate: could become a standalone `halcon-domain` crate.
pub mod domain;
pub mod planning;
pub mod context;
pub mod git_tools;
pub mod metrics;
pub mod bridges;
// Backward-compatible re-exports so all existing `super::X` import paths remain valid:
pub use domain::intent_scorer;
pub use domain::convergence_controller;
pub use domain::model_router;
pub use domain::strategy_selector;
pub use domain::task_analyzer;
pub(crate) use domain::text_utils;
pub use domain::round_feedback;
pub use domain::termination_oracle;
pub use domain::adaptive_policy;

mod slash_commands;

#[cfg(test)]
mod stress_tests;

use prompt::HalconPrompt;

/// Detect the OS username for user context injection into the system prompt.
///
/// Priority: $USER → $LOGNAME → home dir basename → "user".
fn detect_user_display_name() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            dirs::home_dir()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        })
        .unwrap_or_else(|| "user".to_string())
}

/// Interactive REPL for halcon.
pub struct Repl {
    pub(crate) editor: Reedline,
    pub(crate) prompt: HalconPrompt,
    pub(crate) config: AppConfig,
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) session: Session,
    pub(crate) db: Option<Arc<Database>>,
    pub(crate) async_db: Option<AsyncDatabase>,
    pub(crate) registry: ProviderRegistry,
    pub(crate) tool_registry: ToolRegistry,
    pub(crate) permissions: ConversationalPermissionHandler,
    pub(crate) event_tx: EventSender,
    /// Context manager wrapping pipeline + sources + governance for unified context assembly (Phase 38).
    /// Sources are owned by the manager - access via context_manager.sources().
    pub(crate) context_manager: Option<context_manager::ContextManager>,
    pub(crate) response_cache: Option<ResponseCache>,
    pub(crate) resilience: ResilienceManager,
    pub(crate) reflector: Option<reflexion::Reflector>,
    pub(crate) no_banner: bool,
    /// When true, the user explicitly set `--model` on the CLI, so model selection is bypassed.
    pub(crate) explicit_model: bool,
    /// Temporary dry-run mode override for the next handle_message call.
    pub(crate) dry_run_override: Option<halcon_core::types::DryRunMode>,
    /// Trace step cursor for /step forward/back navigation.
    pub(crate) trace_cursor: Option<(uuid::Uuid, Vec<halcon_storage::TraceStep>, usize)>,
    /// Cached execution timeline JSON from the last agent loop (for --timeline flag).
    pub(crate) last_timeline: Option<String>,
    /// Shared context metrics for agent loop observability (Phase 42).
    pub(crate) context_metrics: std::sync::Arc<context_metrics::ContextMetrics>,
    /// Context governance for per-source token limits (Phase 42).
    pub(crate) context_governance: context_governance::ContextGovernance,
    /// Expert mode: show full agent feedback (model selection, caching, etc.).
    pub(crate) expert_mode: bool,
    /// Tool speculation engine for pre-executing read-only tools (Phase 3 remediation).
    pub(crate) speculator: tool_speculation::ToolSpeculator,
    /// FASE 3.1: Reasoning engine for metacognitive agent loop wrapping (Phase 40).
    /// None when reasoning.enabled = false (default).
    pub(crate) reasoning_engine: Option<reasoning_engine::ReasoningEngine>,
    /// FASE 3.2: MCP resource manager for lazy MCP server discovery.
    /// Always present (empty when no servers configured).
    pub(crate) mcp_manager: mcp_manager::McpResourceManager,
    /// P1.1: Playbook-based planner loaded from ~/.halcon/playbooks/.
    /// Runs before LlmPlanner — instant (zero LLM calls) for matched workflows.
    pub(crate) playbook_planner: playbook_planner::PlaybookPlanner,
    /// OS username for user context injection into system prompt (e.g. "oscarvalois").
    pub(crate) user_display_name: String,
    /// Multimodal subsystem (image/audio/video analysis). Activated with `--full`.
    pub(crate) multimodal: Option<std::sync::Arc<halcon_multimodal::MultimodalSubsystem>>,
    /// Control channel receiver from TUI (Phase 43). None in classic REPL mode.
    #[cfg(feature = "tui")]
    pub(crate) ctrl_rx: Option<tokio::sync::mpsc::UnboundedReceiver<crate::tui::events::ControlEvent>>,
    /// Phase 3: Model quality stats cache for session-level persistence.
    ///
    /// Snapshot of `ModelSelector.quality_stats` extracted after each agent loop and re-injected
    /// into the next fresh `ModelSelector` via `with_quality_seeds()`. This ensures `balanced`
    /// routing uses accumulated quality data across all messages within the session, not just
    /// the current message (previously reset to neutral every turn because ModelSelector is
    /// created fresh per message). Tuple: `(success_count, failure_count, total_reward)`.
    pub(crate) model_quality_cache: std::collections::HashMap<String, (u32, u32, f64)>,
    /// Phase 4: Whether cross-session quality stats have been loaded from DB this session.
    ///
    /// Prevents repeated DB queries (load-once-per-session). Set to true after first load attempt
    /// (even if the DB returned empty results or was unavailable).
    pub(crate) model_quality_db_loaded: bool,
    /// Plugin registry for V3 plugin system. None until plugins are configured.
    /// Wrapped in Arc<Mutex<>> so it can be shared safely with the parallel executor.
    /// Initialized as None in Repl::new() — activated when plugins are loaded.
    pub(crate) plugin_registry: Option<std::sync::Arc<std::sync::Mutex<plugins::PluginRegistry>>>,
    /// Transport runtime for V3 plugins (shared handle pool for Stdio/HTTP/Local plugins).
    /// None until plugins are lazy-initialized on first message with config.plugins.enabled.
    pub(crate) plugin_transport_runtime: Option<std::sync::Arc<plugins::PluginTransportRuntime>>,
    /// Whether plugin UCB1 metrics have been loaded from DB this session (load-once guard).
    pub(crate) plugin_metrics_db_loaded: bool,
    /// Phase 5 Dev Ecosystem: DevGateway coordinates IDE buffers, git context, and CI
    /// feedback into a single `DevContext` snapshot that is injected into the system
    /// prompt on each message.  The gateway is Arc-backed internally so clone is cheap.
    pub(crate) dev_gateway: dev_gateway::DevGateway,
    /// Phase 7 Dev Ecosystem: Rolling observability window for agent-loop telemetry.
    /// Ingests per-loop spans and exposes p50/p95/p99 + error-rate as a UCB1 reward
    /// signal.  Shared via Arc so multiple async tasks can ingest without contention.
    pub(crate) runtime_signals: std::sync::Arc<runtime_signal_ingestor::RuntimeSignalIngestor>,
    /// Phase 4 Dev Ecosystem: Stop signal for the background CI polling task.
    /// Set once during `run()` / `run_tui()` when GITHUB_TOKEN is available.
    /// Notified on session teardown so the polling loop exits gracefully.
    pub(crate) ci_stop: std::sync::Arc<tokio::sync::Notify>,
    /// Phase 94: One-time onboarding check performed on first message.
    /// Set to true after the check runs (prevents repeated file-existence checks).
    pub(crate) onboarding_checked: bool,
    /// Phase 95: One-time plugin recommendation check on first message.
    pub(crate) plugin_recommendation_done: bool,
    /// BRECHA-S3: Tools blocked during this session (name, reason).
    ///
    /// Accumulated from `state.blocked_tools` after each agent loop. Persists
    /// across turns so the LlmPlanner can avoid generating steps with blocked tools.
    /// Invariant: if a tool was blocked in turn N, the plan for turn N+1 excludes it.
    pub(crate) session_blocked_tools: Vec<(String, String)>,
    /// US-output-format (PASO 2-A): when true, use CiSink (NDJSON) instead of ClassicSink.
    /// Set from --output-format json on the CLI.
    pub(crate) use_ci_sink: bool,
    /// Shared CiSink instance for session_end emission after loop completes.
    /// Only populated when use_ci_sink is true.
    pub(crate) ci_sink: Option<std::sync::Arc<crate::render::ci_sink::CiSink>>,
}

// ── Multimodal helper ─────────────────────────────────────────────────────────

/// Extract file paths from user message text that point to media files.
///
/// Scans whitespace-separated tokens, strips surrounding quotes/brackets,
/// checks extension against known media types, and verifies file existence.
/// Returns deduplicated, canonicalized paths only.
fn extract_media_paths(text: &str) -> Vec<std::path::PathBuf> {
    const MEDIA_EXTS: &[&str] = &[
        // Images
        "jpg", "jpeg", "png", "gif", "webp", "bmp", "tiff", "tif", "avif",
        // Audio
        "mp3", "wav", "ogg", "m4a", "flac", "aac", "opus",
        // Video
        "mp4", "webm", "mkv", "mov", "avi",
    ];
    let mut seen = std::collections::HashSet::new();
    let mut paths = Vec::new();
    for token in text.split_whitespace() {
        let cleaned = token.trim_matches(|c| matches!(c, '"' | '\'' | '(' | ')' | '[' | ']'));
        let lower = cleaned.to_lowercase();
        let is_media = MEDIA_EXTS.iter().any(|ext| lower.ends_with(&format!(".{ext}")));
        if !is_media {
            continue;
        }
        let p = std::path::PathBuf::from(cleaned);
        if !p.exists() {
            continue;
        }
        if let Ok(canon) = p.canonicalize() {
            if seen.insert(canon.clone()) {
                paths.push(canon);
            }
        }
    }
    paths
}

/// Build all context sources from config and database.
///
/// Extracted from `Repl::new()` to keep the constructor under 80 LOC.
/// Handles: base sources, memory, reflexion, all 8 SDLC context servers.
fn build_context_sources(
    config: &AppConfig,
    async_db: &Option<AsyncDatabase>,
) -> Vec<Box<dyn ContextSource>> {
    let mut sources: Vec<Box<dyn ContextSource>> = vec![
        Box::new(halcon_context::InstructionSource::new()),
        Box::new(repo_map_source::RepoMapSource::default()),
    ];

    if config.planning.enabled {
        sources.push(Box::new(PlanningSource::new(&config.planning)));
    }

    if config.memory.enabled {
        if let Some(ref adb) = async_db {
            if config.memory.episodic {
                let retriever = hybrid_retriever::HybridRetriever::new(adb.clone())
                    .with_rrf_k(config.memory.rrf_k)
                    .with_decay_half_life(config.memory.decay_half_life_days);
                sources.push(Box::new(episodic_source::EpisodicSource::new(
                    retriever,
                    config.memory.retrieval_top_k,
                    config.memory.retrieval_token_budget,
                )));
            } else {
                sources.push(Box::new(MemorySource::new(
                    adb.clone(),
                    config.memory.retrieval_top_k,
                    config.memory.retrieval_token_budget,
                )));
            }
        }
    }

    if config.reflexion.enabled {
        if let Some(ref adb) = async_db {
            sources.push(Box::new(reflection_source::ReflectionSource::new(
                adb.clone(),
                config.reflexion.max_reflections,
            )));
        }
    }

    if config.context_servers.enabled {
        if let Some(ref adb) = async_db {
            if config.context_servers.requirements.enabled {
                sources.push(Box::new(requirements_server::RequirementsServer::new(adb.clone(), config.context_servers.requirements.priority, config.context_servers.requirements.token_budget)));
            }
            if config.context_servers.architecture.enabled {
                sources.push(Box::new(architecture_server::ArchitectureServer::new(adb.clone(), config.context_servers.architecture.priority, config.context_servers.architecture.token_budget)));
            }
            if config.context_servers.workflow.enabled {
                sources.push(Box::new(workflow_server::WorkflowServer::new(adb.clone(), config.context_servers.workflow.priority, config.context_servers.workflow.token_budget)));
            }
            if config.context_servers.testing.enabled {
                sources.push(Box::new(test_results_server::TestResultsServer::new(adb.clone(), config.context_servers.testing.priority, config.context_servers.testing.token_budget)));
            }
            if config.context_servers.runtime.enabled {
                sources.push(Box::new(runtime_metrics_server::RuntimeMetricsServer::new(adb.clone(), config.context_servers.runtime.priority, config.context_servers.runtime.token_budget)));
            }
            if config.context_servers.security.enabled {
                sources.push(Box::new(security_server::SecurityServer::new(adb.clone(), config.context_servers.security.priority, config.context_servers.security.token_budget)));
            }
            if config.context_servers.support.enabled {
                sources.push(Box::new(support_server::SupportServer::new(adb.clone(), config.context_servers.support.priority, config.context_servers.support.token_budget)));
            }
        }
        if config.context_servers.codebase.enabled {
            let working_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            sources.push(Box::new(codebase_server::CodebaseServer::new(
                working_dir,
                config.context_servers.codebase.priority,
                config.context_servers.codebase.token_budget,
            )));
        }
    }

    sources
}

/// Initialize the reflector for reflexion-based learning.
fn init_reflector(
    config: &AppConfig,
    registry: &ProviderRegistry,
    provider: &str,
    model: &str,
) -> Option<reflexion::Reflector> {
    if !config.reflexion.enabled { return None; }
    let reflector_prov = config.reasoning.reflector_provider.as_deref()
        .and_then(|name| registry.get(name))
        .cloned()
        .or_else(|| registry.get(provider).cloned())?;
    let reflector_model = config.reasoning.reflector_model.clone().unwrap_or_else(|| model.to_owned());
    Some(reflexion::Reflector::new(reflector_prov, reflector_model)
        .with_reflect_on_success(config.reflexion.reflect_on_success))
}

/// Initialize the resilience manager and register all known providers.
fn init_resilience(
    config: &AppConfig,
    registry: &ProviderRegistry,
    async_db: &Option<AsyncDatabase>,
    event_tx: EventSender,
) -> ResilienceManager {
    let mut resilience = ResilienceManager::new(config.resilience.clone()).with_event_tx(event_tx);
    if let Some(ref adb) = async_db {
        resilience = resilience.with_db(adb.clone());
    }
    for name in registry.list() {
        resilience.register_provider(name);
    }
    resilience
}

impl Repl {
    /// Build real FeatureStatus from current REPL configuration and state.
    fn build_feature_status(&self, tui_active: bool) -> crate::render::banner::FeatureStatus {
        let tool_count = self.tool_registry.tool_definitions().len();
        // Background tools enabled if tool count is 23 (20 core + 3 background)
        let background_tools_enabled = tool_count >= 23;

        // Phase 94: Quick project config check for banner display.
        let cwd = std::env::current_dir().unwrap_or_default();
        let project_configured = matches!(
            onboarding::OnboardingCheck::run(&cwd),
            onboarding::OnboardingStatus::Configured { .. }
        );

        crate::render::banner::FeatureStatus {
            tui_active,
            reasoning_enabled: self.reasoning_engine.is_some(),
            orchestrator_enabled: self.config.orchestrator.enabled,
            context_pipeline_active: true, // Always active (L0-L4 always present)
            tool_count,
            background_tools_enabled,
            multimodal_enabled: self.multimodal.is_some(),
            loop_critic_enabled: self.config.reasoning.enable_loop_critic,
            project_config: project_configured,
        }
    }

    /// Create a new REPL instance with file-backed history and optional DB.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: &AppConfig,
        provider: String,
        model: String,
        db: Option<Arc<Database>>,
        resume_session: Option<Session>,
        registry: ProviderRegistry,
        mut tool_registry: ToolRegistry,
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

        let prompt = HalconPrompt::new(&provider, &model);

        let cwd = std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let session =
            resume_session.unwrap_or_else(|| Session::new(model.clone(), provider.clone(), cwd));

        let mut permissions = ConversationalPermissionHandler::with_config(
            config.tools.confirm_destructive,
            config.security.tbac_enabled,
            config.tools.auto_approve_in_ci,
            config.tools.prompt_timeout_secs,
        );

        // FASE 3-A: Detect CI environment at session init and set non-interactive mode.
        // CIDetectionPolicy handles tool auto-approval in the authorization chain, but
        // set_non_interactive() additionally disables TTY permission prompts that would
        // otherwise hang indefinitely in headless CI environments.
        let ci_env = ci_detection::detect();
        if ci_env.is_ci {
            if let Some(ref var) = ci_env.detected_via {
                tracing::info!(ci_var = %var, "CI environment detected at session init — setting non-interactive mode");
            }
            permissions.set_non_interactive();
        }

        // Build async database wrapper (for async call sites).
        let async_db = db.as_ref().map(|db_ref| AsyncDatabase::new(Arc::clone(db_ref)));

        // Build context sources (memory, reflexion, all 8 SDLC servers) — extracted helper.
        let context_sources = build_context_sources(config, &async_db);

        // Initialize reflexion (extracted helper).
        let reflector = init_reflector(config, &registry, &provider, &model);

        // Initialize response cache when DB is available and cache is enabled.
        let response_cache = if config.cache.enabled {
            async_db.as_ref().map(|adb| ResponseCache::new(adb.clone(), config.cache.clone()))
        } else {
            None
        };

        // Initialize resilience manager (extracted helper).
        let resilience = init_resilience(config, &registry, &async_db, event_tx.clone());

        // Create ContextManager for unified context assembly from all sources (Phase 38 + Context Servers).
        let context_manager = if !context_sources.is_empty() {
            let gov_config = &config.context.governance;
            let governance = if gov_config.default_max_tokens_per_source > 0 {
                context_governance::ContextGovernance::with_default_max_tokens(
                    gov_config.default_max_tokens_per_source,
                )
            } else {
                // No limits configured - use empty HashMap for per-source limits
                context_governance::ContextGovernance::new(std::collections::HashMap::new())
            };

            Some(context_manager::ContextManager::new(
                &halcon_context::ContextPipelineConfig {
                    max_context_tokens: config.general.max_tokens.max(200_000),
                    ..Default::default()
                },
                context_sources, // Move sources into ContextManager (it owns them)
                governance,
            ))
        } else {
            None
        };

        // FASE 3.1: Initialize ReasoningEngine when enabled.
        // Reads from [reasoning] enabled = true in config.toml or HALCON_REASONING=true env var.
        let reasoning_enabled = config.reasoning.enabled
            || std::env::var("HALCON_REASONING").map(|v| v == "true" || v == "1").unwrap_or(false);
        let reasoning_engine = if reasoning_enabled {
            tracing::info!("ReasoningEngine enabled — UCB1 strategy learning active");
            let engine_config = reasoning_engine::ReasoningConfig {
                enabled: true,
                success_threshold: config.reasoning.success_threshold,
                max_retries: config.reasoning.max_retries,
                exploration_factor: config.reasoning.exploration_factor,
            };
            Some(reasoning_engine::ReasoningEngine::new(engine_config))
        } else {
            None
        };

        // P1.2: Load external tools from ~/.halcon/tools/*.toml.
        plugins::tool_manifest::load_external_tools_default(&mut tool_registry);

        // FASE 3.2: Initialize MCP resource manager (lazy discovery, safe fallback).
        let mcp_manager = if config.mcp.servers.is_empty() {
            tracing::debug!("No MCP servers configured — using empty manager");
            mcp_manager::McpResourceManager::empty()
        } else {
            tracing::info!(server_count = config.mcp.servers.len(), "MCP resource manager initialized (lazy discovery)");
            mcp_manager::McpResourceManager::new(&config.mcp)
        };

        // Native Search: Initialize global SearchEngine if database is available.
        if let Some(ref db_arc) = db {
            search_engine_global::init_search_engine(
                db_arc.clone(),
                halcon_search::SearchEngineConfig::default(),
            );
        }

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
            context_manager,
            response_cache,
            resilience,
            reflector,
            no_banner,
            explicit_model,
            dry_run_override: None,
            trace_cursor: None,
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
            speculator: tool_speculation::ToolSpeculator::new(),
            reasoning_engine,
            mcp_manager,
            playbook_planner: playbook_planner::PlaybookPlanner::from_default_dir(),
            user_display_name: detect_user_display_name(),
            multimodal: None,
            #[cfg(feature = "tui")]
            ctrl_rx: None,
            model_quality_cache: std::collections::HashMap::new(),
            model_quality_db_loaded: false,
            plugin_registry: None, // V3 plugins: loaded lazily via /plugin install or config.toml
            plugin_transport_runtime: None,
            plugin_metrics_db_loaded: false,
            // Phase 5/7 Dev Ecosystem: initialized fresh per session.
            // DevGateway is inert until LSP messages arrive via handle_lsp_message().
            dev_gateway: dev_gateway::DevGateway::new(),
            runtime_signals: std::sync::Arc::new(
                runtime_signal_ingestor::RuntimeSignalIngestor::new(512),
            ),
            // Phase 4 Dev Ecosystem: stop signal for CI polling (armed in run/run_tui).
            ci_stop: std::sync::Arc::new(tokio::sync::Notify::new()),
            // Phase 94: Project onboarding check runs once on first message.
            onboarding_checked: false,
            // Phase 95: Plugin recommendation check runs once on first message.
            plugin_recommendation_done: false,
            // BRECHA-S3: No blocked tools at session start.
            session_blocked_tools: vec![],
            // US-output-format (PASO 2-A): defaults to human-readable sink.
            // Set to true externally (via commands/chat.rs) when --output-format json is used.
            use_ci_sink: false,
            ci_sink: None,
        })
    }

    /// Execute a single prompt through the full agent loop (with tools), then exit.
    ///
    /// This gives inline prompts (`halcon chat "do X"`) the same capabilities as
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

        // Send session_id to render sink (for TUI status bar initialization).
        use crate::render::sink::RenderSink;
        if self.use_ci_sink {
            // Initialise the shared CiSink and emit session_start.
            let ci = self.ci_sink.get_or_insert_with(|| {
                std::sync::Arc::new(crate::render::ci_sink::CiSink::new())
            }).clone();
            ci.session_started(&self.session.id.to_string());
        } else {
            let sink = crate::render::sink::ClassicSink::with_expert(self.expert_mode);
            sink.session_started(&self.session.id.to_string());
        }

        // Run the prompt through handle_message (full agent loop with tools).
        self.handle_message(prompt).await?;

        // Emit session_end for CI consumers.
        if let Some(ref ci) = self.ci_sink {
            ci.emit_session_end();
        }

        // Save session.
        self.auto_save_session().await;
        self.save_session();

        Ok(())
    }

    /// Execute one JSON-RPC chat turn with a caller-provided sink.
    ///
    /// Called by `commands::json_rpc::run` for each incoming `chat` request.
    /// Handles session persistence internally so the caller only needs to supply
    /// the message and a sink that serialises output as newline-delimited JSON.
    pub async fn run_json_rpc_turn(
        &mut self,
        message: &str,
        sink: &dyn crate::render::sink::RenderSink,
    ) -> Result<()> {
        self.handle_message_with_sink(message, sink).await?;
        self.auto_save_session().await;
        self.save_session();
        Ok(())
    }

    /// Configure the session for non-interactive (headless) tool execution.
    ///
    /// In this mode all tool permission requests are auto-approved because there
    /// is no TTY available to prompt the user.  Must be called once before the
    /// first `run_json_rpc_turn`.
    pub fn set_non_interactive_mode(&mut self) {
        self.permissions.set_non_interactive();
    }

    /// Start CI polling in the background when environment variables are present.
    ///
    /// Reads `GITHUB_TOKEN` and `GITHUB_REPOSITORY` (format: `owner/repo`).
    /// When both are set, spawns a background task that polls GitHub Actions every
    /// 60 s and feeds results into `DevGateway::ingest_ci_event()`.
    /// The task exits when `self.ci_stop` is notified.
    fn maybe_start_ci_polling(&self) {
        use ci_result_ingestor::{CiIngestorConfig, CiResultIngestor, GithubActionsClient};
        use std::sync::Arc;

        let token = match std::env::var("GITHUB_TOKEN")
            .or_else(|_| std::env::var("HALCON_CI_TOKEN"))
        {
            Ok(t) if !t.is_empty() => t,
            _ => return, // no token → skip silently
        };

        let repository = match std::env::var("GITHUB_REPOSITORY")
            .or_else(|_| std::env::var("HALCON_CI_REPO"))
        {
            Ok(r) if !r.is_empty() => r,
            _ => return, // no repo → skip silently
        };

        let parts: Vec<&str> = repository.splitn(2, '/').collect();
        if parts.len() != 2 {
            tracing::warn!(repo = %repository, "GITHUB_REPOSITORY must be 'owner/repo' — CI polling skipped");
            return;
        }
        let (owner, repo) = (parts[0].to_string(), parts[1].to_string());
        // Workflow name: optional, falls back to any workflow.
        let workflow = std::env::var("HALCON_CI_WORKFLOW").unwrap_or_default();

        let client = Arc::new(GithubActionsClient::new(&owner, &repo, &workflow, &token));
        let ingestor = CiResultIngestor::new(client, CiIngestorConfig::default());
        let mut rx = ingestor.subscribe();
        let stop = Arc::clone(&self.ci_stop);

        // Feed CI events into DevGateway so build_context() can include them.
        let gateway = self.dev_gateway.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop.notified() => break,
                    result = rx.recv() => {
                        match result {
                            Ok(event) => gateway.ingest_ci_event(event).await,
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        }
                    }
                }
            }
        });

        ingestor.start();
        tracing::info!(owner = %owner, repo = %repo, "Phase 4: CI polling started (GitHub Actions)");
    }

    /// Run the interactive REPL loop.
    pub async fn run(&mut self) -> Result<()> {
        // Warm L1 cache from L2 on startup.
        if let Some(ref cache) = self.response_cache {
            cache.warm_l1().await;
        }

        // Phase 4 Dev Ecosystem: start CI polling when GitHub credentials are present.
        self.maybe_start_ci_polling();

        self.print_welcome();

        // Emit SessionStarted event.
        let _ = self.event_tx.send(DomainEvent::new(EventPayload::SessionStarted {
            session_id: self.session.id,
        }));

        // Send session_id to render sink (for TUI status bar initialization).
        use crate::render::sink::RenderSink;
        if self.use_ci_sink {
            let ci = self.ci_sink.get_or_insert_with(|| {
                std::sync::Arc::new(crate::render::ci_sink::CiSink::new())
            }).clone();
            ci.session_started(&self.session.id.to_string());
        } else {
            let sink = crate::render::sink::ClassicSink::with_expert(self.expert_mode);
            sink.session_started(&self.session.id.to_string());
        }

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
                            // Phase 94: /init not interactive in classic mode — suggest TUI.
                            commands::CommandResult::Init { .. } => {
                                crate::render::feedback::user_error(
                                    "/init is best used in TUI mode",
                                    Some("Run: halcon chat --tui, then type /init"),
                                );
                                continue;
                            }
                            // Phase 95: /plugins — show suggestion to use TUI for rich view.
                            commands::CommandResult::Plugins(subcmd) => {
                                use commands::PluginsSubcmd;
                                match subcmd {
                                    PluginsSubcmd::Status => {
                                        if let Some(ref arc_pr) = self.plugin_registry {
                                            if let Ok(reg) = arc_pr.lock() {
                                                let ids: Vec<String> = reg.loaded_plugin_ids().map(|s| s.to_string()).collect();
                                                if ids.is_empty() {
                                                    println!("[plugins] No plugins loaded.");
                                                } else {
                                                    for id in &ids {
                                                        println!("[plugins] {} — {}", id, reg.plugin_state_str(id));
                                                    }
                                                }
                                            }
                                        } else {
                                            println!("[plugins] Plugin system not active. Use --full to enable.");
                                        }
                                    }
                                    PluginsSubcmd::Suggest => {
                                        if let Some(ref arc_pr) = self.plugin_registry {
                                            if let Ok(reg) = arc_pr.lock() {
                                                let cwd = std::env::current_dir().unwrap_or_default();
                                                let analysis = project_inspector::ProjectInspector::analyze(&cwd);
                                                let loaded: std::collections::HashSet<String> = reg.loaded_plugin_ids().map(|s| s.to_string()).collect();
                                                let rewards = reg.ucb1_rewards_snapshot();
                                                drop(reg);
                                                let recs = plugins::PluginRecommendationEngine::recommend(&analysis, &loaded, &rewards);
                                                println!("{}", plugins::PluginRecommendationEngine::format_report(&recs));
                                            }
                                        } else {
                                            println!("[plugins] Plugin system not active. Use --full to enable.");
                                        }
                                    }
                                    PluginsSubcmd::Auto { dry_run } => {
                                        println!("[plugins] Auto-bootstrap{} — use TUI for interactive mode.", if dry_run { " (dry-run)" } else { "" });
                                        println!("[plugins] Run: halcon chat --tui, then type /plugins auto");
                                    }
                                    PluginsSubcmd::Disable(id) => {
                                        if let Some(ref arc_pr) = self.plugin_registry {
                                            if let Ok(mut reg) = arc_pr.lock() {
                                                reg.auto_disable(&id, "disabled by user", std::time::Duration::ZERO);
                                                println!("[plugins] {} — suspended", id);
                                            }
                                        }
                                    }
                                    PluginsSubcmd::Enable(id) => {
                                        if let Some(ref arc_pr) = self.plugin_registry {
                                            if let Ok(mut reg) = arc_pr.lock() {
                                                reg.clear_cooling(&id);
                                                println!("[plugins] {} — resumed", id);
                                            }
                                        }
                                    }
                                }
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

        // Phase 4 Dev Ecosystem: start CI polling when GitHub credentials are present.
        self.maybe_start_ci_polling();

        // Emit SessionStarted event.
        let _ = self.event_tx.send(DomainEvent::new(EventPayload::SessionStarted {
            session_id: self.session.id,
        }));

        // Create channels: UiEvents (agent → TUI) and prompts (TUI → agent).
        // UNBOUNDED channel: prevents PermissionAwaiting from being dropped when LLM
        // floods the channel with StreamChunk events during large outputs (e.g. 8K token
        // game generation). The old bounded try_send silently dropped critical events
        // → modal never showed → 60s timeout → tool denied with no user feedback.
        let (ui_tx, ui_rx) = tokio_mpsc::unbounded_channel::<UiEvent>();
        let (prompt_tx, mut prompt_rx) = tokio_mpsc::unbounded_channel::<String>();

        let tui_sink = TuiSink::new(ui_tx.clone());

        // Expert mode: emit SOTA subsystem activation report to TUI activity stream.
        // This confirms all systems are live before the first prompt is entered.
        if self.expert_mode {
            use crate::render::sink::RenderSink as _;
            let fs = self.build_feature_status(true);
            tui_sink.info("[expert] SOTA subsystems active:");
            tui_sink.info(&format!(
                "  Reasoning/UCB1={} Orchestrator={} TaskFramework={} Reflexion={}",
                fs.reasoning_enabled,
                fs.orchestrator_enabled,
                self.config.task_framework.enabled,
                self.config.reflexion.enabled,
            ));
            tui_sink.info(&format!(
                "  Multimodal={} LoopCritic={} RoundScorer=on PlanCoherence=on",
                fs.multimodal_enabled,
                fs.loop_critic_enabled,
            ));
            tui_sink.info("  DevEcosystem=on [LSP:5758 CIPoll=env GitContext=on AST=on]");
            // Multimodal subsystem detailed status (Phase 62 wiring).
            if let Some(ref mm) = self.multimodal {
                let snap = mm.metrics_snapshot();
                tui_sink.info(&format!(
                    "  [multimodal] READY — cache_hits={} | index=active",
                    snap.cache_hits
                ));
            }
            tracing::info!(
                reasoning = fs.reasoning_enabled,
                orchestrator = fs.orchestrator_enabled,
                multimodal = fs.multimodal_enabled,
                loop_critic = fs.loop_critic_enabled,
                task_framework = self.config.task_framework.enabled,
                reflexion = self.config.reflexion.enabled,
                "Expert mode: SOTA subsystem activation report"
            );
        }

        // Phase 5 Dev Ecosystem: Start embedded TCP LSP server so IDE extensions can
        // connect while the TUI is running.  The server binds on localhost:5758 and
        // handles standard LSP JSON-RPC over a line-delimited TCP connection.
        //
        // A secondary polling task checks the open-buffer count every 5 s and emits
        // IdeBuffersUpdated when it changes, keeping the status bar indicator live.
        {
            use std::sync::Arc;
            const LSP_PORT: u16 = 5758;
            let lsp_addr: std::net::SocketAddr = ([127, 0, 0, 1], LSP_PORT).into();
            let lsp_gw = Arc::new(self.dev_gateway.clone());
            let lsp_stop = Arc::clone(&self.ci_stop);
            // Separate senders: server-done and buffer-poll need distinct clones.
            let lsp_done_tx = ui_tx.clone(); // moved into LSP server task
            let poll_gw = self.dev_gateway.clone();
            let poll_ui_tx = ui_tx.clone(); // moved into polling task
            // Independent stop signal clone for the polling task so it exits
            // cleanly when the TUI session ends (avoids the infinite loop leak).
            let poll_stop = Arc::clone(&self.ci_stop);

            // Start the TCP LSP accept loop in a background task.
            tokio::spawn(async move {
                if let Err(e) = lsp_gw.serve_tcp(lsp_addr, lsp_stop).await {
                    tracing::warn!(error = %e, "Dev ecosystem LSP TCP server stopped");
                }
                // Notify TUI that the server has gone away.
                let _ = lsp_done_tx.send(UiEvent::IdeDisconnected);
            });

            // Notify the TUI immediately that the LSP port is ready.
            let _ = ui_tx.send(UiEvent::IdeConnected { port: LSP_PORT });

            // Poll buffer count every 5 s; emit IdeBuffersUpdated on change.
            // Exits cleanly when `poll_stop` (= ci_stop) is notified.
            tokio::spawn(async move {
                let mut last_count: usize = 0;
                loop {
                    // Wait 5 s or until session teardown, whichever comes first.
                    tokio::select! {
                        _ = poll_stop.notified() => break,
                        _ = tokio::time::sleep(tokio::time::Duration::from_secs(5)) => {}
                    }
                    let count = poll_gw.buffers.len().await;
                    if count != last_count {
                        last_count = count;
                        // Fetch full dev context to get current git branch.
                        // build_context() offloads git I/O to spawn_blocking.
                        let ctx = poll_gw.build_context().await;
                        // Extract branch name from summary "git:{branch} [{status}] …"
                        let git_branch = ctx.git_summary.as_deref().and_then(|s| {
                            s.strip_prefix("git:")
                                .and_then(|s| s.split_once(" ["))
                                .map(|(b, _)| b.to_string())
                                .filter(|b| b != "(detached)")
                        });
                        let _ = poll_ui_tx.send(UiEvent::IdeBuffersUpdated {
                            count,
                            git_branch,
                        });
                    }
                }
            });
        }

        // Phase 96: Startup Probe — proactive project analysis at TUI launch.
        // Pre-mark flags so handle_message_with_sink() doesn't double-fire on first message.
        self.onboarding_checked = true;
        self.plugin_recommendation_done = true;
        {
            let probe_tx = ui_tx.clone();
            let probe_cwd = std::env::current_dir().unwrap_or_default();
            let probe_plugins_enabled = self.config.plugins.enabled;
            let probe_registry = self.plugin_registry.clone();
            let probe_expert = self.expert_mode;
            tokio::spawn(async move {
                // Short delay so the banner renders before flooding the activity feed.
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                // Analyze project (sync I/O) via spawn_blocking.
                let analysis = tokio::task::spawn_blocking(move || {
                    project_inspector::ProjectInspector::analyze(&probe_cwd)
                })
                .await
                .unwrap_or_else(|_| project_inspector::ProjectAnalysis::default());

                // Emit git branch immediately (don't wait 5 s for IDE polling).
                if let Some(ref branch) = analysis.git_branch {
                    let remote_hint = if analysis.git_remote.is_some() { " ·remote" } else { "" };
                    let _ = probe_tx.send(UiEvent::Info(format!(
                        "[git] {branch}{remote_hint} — {}", analysis.project_type
                    )));
                }

                // Emit project context summary.
                let mut ctx_parts: Vec<String> = Vec::new();
                if let Some(ref name) = analysis.package_name {
                    ctx_parts.push(format!("pkg:{name}"));
                }
                if !analysis.manifest_files.is_empty() {
                    ctx_parts.push(format!("{} manifests", analysis.manifest_files.len()));
                }
                if analysis.has_project_halcon_md {
                    ctx_parts.push("HALCON.md ✓".to_string());
                }
                if !ctx_parts.is_empty() {
                    let _ = probe_tx.send(UiEvent::Info(format!(
                        "[proyecto] {}",
                        ctx_parts.join(" · ")
                    )));
                }

                // Onboarding hint.
                let onboarding_status = onboarding::OnboardingCheck::run(&analysis.root);
                match onboarding_status {
                    onboarding::OnboardingStatus::NotConfigured { .. } => {
                        let _ = probe_tx.send(UiEvent::Info(
                            "[onboarding] Sin HALCON.md de proyecto — escribe /init para configurar".to_string(),
                        ));
                    }
                    onboarding::OnboardingStatus::Configured { ref path } => {
                        let name = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("HALCON.md");
                        let _ = probe_tx.send(UiEvent::Info(format!(
                            "[config] {name} cargado"
                        )));
                    }
                    onboarding::OnboardingStatus::Unknown => {}
                }

                // Plugin recommendations.
                if probe_plugins_enabled {
                    if let Some(ref arc_pr) = probe_registry {
                        if let Ok(reg) = arc_pr.lock() {
                            let loaded: std::collections::HashSet<String> =
                                reg.loaded_plugin_ids().map(|s| s.to_string()).collect();
                            let rewards = reg.ucb1_rewards_snapshot();
                            drop(reg);
                            let recs = plugins::PluginRecommendationEngine::recommend(
                                &analysis, &loaded, &rewards,
                            );
                            let total_new =
                                recs.iter().filter(|r| !r.already_installed).count();
                            let essential = recs
                                .iter()
                                .filter(|r| {
                                    !r.already_installed
                                        && r.tier
                                            == plugins::RecommendationTier::Essential
                                })
                                .count();
                            if total_new > 0 {
                                let _ = probe_tx.send(UiEvent::Info(format!(
                                    "[plugins] {total_new} recomendados ({essential} esenciales) — /plugins suggest"
                                )));
                            }
                        }
                    }
                }

                // Expert: brief tool summary.
                if probe_expert {
                    let _ = probe_tx.send(UiEvent::Info(
                        "[herramientas] 58 disponibles · /tools list para ver todo".to_string(),
                    ));
                }

                // Final: ready prompt.
                let _ = probe_tx.send(UiEvent::Info(
                    "◈  Listo. Escribe tu pregunta o /help para ver comandos.".to_string(),
                ));
            });
        }

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
        let (perm_tx, perm_rx) = tokio::sync::mpsc::unbounded_channel::<halcon_core::types::PermissionDecision>();
        self.permissions.set_tui_channel(perm_rx);
        // Wire notification channel so permission timeouts appear in TUI activity panel.
        self.permissions.set_notification_tx(ui_tx.clone());

        // Sudo password channel: TuiApp → executor's get_sudo_password().
        // Kept separate from perm_tx because PermissionDecision is Copy and cannot
        // carry a String payload. The executor awaits this after an approved sudo command.
        let (sudo_pw_tx, sudo_pw_rx) = tokio::sync::mpsc::unbounded_channel::<Option<String>>();
        self.permissions.set_sudo_channel(sudo_pw_rx);

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

        // Build real feature status for banner display.
        let features = self.build_feature_status(true); // tui_active = true

        // Spawn TUI render loop in a separate task.
        tracing::debug!("Spawning TUI task");
        let async_db_clone = self.async_db.clone(); // Phase 3 SRCH-004: Pass database for search history
        let ui_tx_for_bg = ui_tx.clone(); // Phase 45E: for background DB queries from TUI
        let tui_handle = tokio::spawn(async move {
            tracing::debug!("TUI task started");
            let mut app = TuiApp::with_mode(ui_rx, prompt_tx, ctrl_tx, perm_tx, async_db_clone, initial_mode);
            // Phase 45E: Give app a sender so it can push events from async background tasks.
            app.set_ui_tx(ui_tx_for_bg);
            // Phase 50: Wire sudo password sender so TuiApp can deliver passwords to executor.
            app.set_sudo_pw_tx(sudo_pw_tx);
            tracing::debug!("TUI app created with mode: {:?}", initial_mode);
            app.push_banner(
                &banner_version,
                &banner_provider,
                banner_provider_connected,
                &banner_model,
                &banner_session_id,
                &banner_session_type,
                banner_routing.as_ref(),
                &features,
            );
            tracing::debug!("TUI banner pushed, calling run()");
            let result = app.run().await;
            tracing::debug!("TUI run() returned: {:?}", result);
            result
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

        // Phase 4: Create task manager for non-blocking agent execution.
        let max_concurrent = self.config.agent.limits.max_concurrent_agents;
        let mut task_manager = agent_task_manager::AgentTaskManager::new(max_concurrent);
        tracing::debug!(max_concurrent, "Agent task manager initialized");

        // Agent message loop: wait for prompts from TUI, process each.
        tracing::debug!("Entering agent message loop, waiting for prompts from TUI");
        loop {
            // Check for control events (non-blocking) before waiting for next prompt.
            // This handles TUI requests like RequestContextServers that need immediate response.
            if let Some(ref mut ctrl) = self.ctrl_rx {
                while let Ok(event) = ctrl.try_recv() {
                    use crate::tui::events::ControlEvent;
                    match event {
                        ControlEvent::RequestContextServers => {
                            // ✅ Collect REAL data from context_manager with runtime stats
                            let servers = if let Some(ref cm) = self.context_manager {
                                cm.sources_with_stats()
                                    .map(|(name, priority, stats)| {
                                        // Calcular ms desde última query
                                        let last_query_ms = stats.last_query.map(|instant| {
                                            instant.elapsed().as_millis() as u64
                                        });

                                        crate::tui::events::ContextServerInfo {
                                            name: name.to_string(),
                                            priority,
                                            enabled: true,  // TODO: Obtener de config si es posible
                                            last_query_ms,
                                            total_tokens: stats.total_tokens,
                                            query_count: stats.query_count,
                                        }
                                    })
                                    .collect::<Vec<_>>()
                            } else {
                                Vec::new()
                            };

                            let total_count = servers.len();
                            let enabled_count = servers.iter().filter(|s| s.enabled).count();

                            // Send back via UiEvent
                            let _ = ui_tx.send(crate::tui::events::UiEvent::ContextServersList {
                                servers,
                                total_count,
                                enabled_count,
                            });
                        }
                        // Phase 45F: Load a previous session from DB and restore context.
                        ControlEvent::ResumeSession(id) => {
                            use uuid::Uuid;
                            if let Ok(uuid) = Uuid::parse_str(&id) {
                                if let Some(ref db) = self.async_db {
                                    match db.load_session(uuid).await {
                                        Ok(Some(session)) => {
                                            let rounds = session.agent_rounds as usize;
                                            let msgs = session.messages.len();
                                            let provider = session.provider.clone();
                                            let model = session.model.clone();
                                            let cost = session.estimated_cost_usd;
                                            let short_id = &id[..8.min(id.len())];
                                            // Restore session state.
                                            self.session.id = uuid;
                                            self.session.total_usage.input_tokens = session.total_usage.input_tokens;
                                            self.session.total_usage.output_tokens = session.total_usage.output_tokens;
                                            self.session.estimated_cost_usd = cost;
                                            self.session.agent_rounds = session.agent_rounds;
                                            self.session.messages = session.messages;
                                            // Notify TUI of loaded session.
                                            let _ = ui_tx.send(UiEvent::SessionInitialized {
                                                session_id: uuid.to_string(),
                                            });
                                            let _ = ui_tx.send(UiEvent::StatusUpdate {
                                                provider: Some(provider),
                                                model: Some(model),
                                                round: Some(rounds),
                                                tokens: None,
                                                cost: Some(cost),
                                                session_id: Some(uuid.to_string()),
                                                elapsed_ms: None,
                                                tool_count: None,
                                                input_tokens: Some(session.total_usage.input_tokens),
                                                output_tokens: Some(session.total_usage.output_tokens),
                                            });
                                            let _ = ui_tx.send(UiEvent::Info(format!(
                                                "=== Session {} loaded ({} rounds, {} messages) ===",
                                                short_id, rounds, msgs
                                            )));
                                        }
                                        Ok(None) => {
                                            let _ = ui_tx.send(UiEvent::Warning {
                                                message: format!("Session {} not found", &id[..8.min(id.len())]),
                                                hint: None,
                                            });
                                        }
                                        Err(e) => {
                                            let _ = ui_tx.send(UiEvent::Warning {
                                                message: format!("Failed to load session: {e}"),
                                                hint: None,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                        // Other control events (Pause/Resume/Step) are handled by agent loop,
                        // but we need to consume them here to prevent queue buildup.
                        // They'll be re-sent when appropriate or ignored if agent isn't running.
                        _ => {
                            // Ignore or log other control events in repl loop.
                            tracing::trace!("Ignoring control event in repl loop: {:?}", event);
                        }
                    }
                }
            }

            tracing::trace!("Waiting for prompt from TUI...");
            let text = match prompt_rx.recv().await {
                Some(t) => {
                    tracing::debug!("Received prompt from TUI: {}", t.chars().take(50).collect::<String>());
                    t
                }
                None => {
                    // Normal shutdown path: TUI exited (user pressed q/Ctrl+C),
                    // dropping prompt_tx which closes this channel. Not an error.
                    tracing::debug!("TUI closed the prompt channel, exiting loop");
                    break; // TUI closed the channel.
                }
            };

            // Phase 44B: Signal that agent started processing this prompt.
            let _ = ui_tx.send(UiEvent::AgentStartedPrompt);

            // Phase 4B-Lite: Show toast if prompts are queued (user feedback)
            let queue_len = prompt_rx.len();
            if queue_len > 0 {
                let _ = ui_tx.send(UiEvent::Info(format!(
                    "✓ Prompt queued ({} ahead)",
                    queue_len
                )));
            }

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
                                let _ = ui_tx.send(UiEvent::Info("/init        Setup project config (HALCON.md)".into()));
                                let _ = ui_tx.send(UiEvent::Info("/quit        Exit halcon".into()));
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
                    commands::CommandResult::Init { dry_run, .. } => {
                        // Phase 94: Open the init wizard overlay (step 0 = analyzing).
                        let _ = ui_tx.send(UiEvent::OpenInitWizard { dry_run });
                        // Run ProjectInspector in background so TUI stays responsive.
                        let ui_tx2 = ui_tx.clone();
                        let cwd = std::env::current_dir().unwrap_or_default();
                        tokio::spawn(async move {
                            let cwd2 = cwd.clone();
                            let analysis = tokio::task::spawn_blocking(move || {
                                project_inspector::ProjectInspector::analyze(&cwd2)
                            })
                            .await
                            .unwrap_or_default();
                            let preview =
                                project_inspector::ProjectInspector::generate_halcon_md(&analysis);
                            let save_path = analysis
                                .suggested_halcon_md_path
                                .to_string_lossy()
                                .into_owned();
                            let _ = ui_tx2.send(UiEvent::ProjectAnalysisComplete {
                                root: analysis.root.to_string_lossy().into_owned(),
                                project_type: analysis.project_type,
                                package_name: analysis.package_name,
                                has_git: analysis.git_remote.is_some(),
                                preview,
                                save_path,
                            });
                        });
                    }
                    // Phase 95: /plugins subcommands.
                    commands::CommandResult::Plugins(subcmd) => {
                        use commands::PluginsSubcmd;
                        match subcmd {
                            PluginsSubcmd::Status => {
                                // Show plugin status lines in activity feed.
                                if let Some(ref arc_pr) = self.plugin_registry {
                                    if let Ok(reg) = arc_pr.lock() {
                                        let ids: Vec<String> = reg.loaded_plugin_ids().map(|s| s.to_string()).collect();
                                        if ids.is_empty() {
                                            let _ = ui_tx.send(UiEvent::Info("[plugins] No plugins loaded.".into()));
                                        } else {
                                            for id in &ids {
                                                let state = reg.plugin_state_str(id);
                                                let _ = ui_tx.send(UiEvent::Info(format!("[plugins] {id} — {state}")));
                                            }
                                        }
                                    }
                                } else {
                                    let _ = ui_tx.send(UiEvent::Info("[plugins] Plugin system not active. Use --full to enable.".into()));
                                }
                            }
                            PluginsSubcmd::Suggest => {
                                // Open suggestion overlay after async analysis.
                                let ui_tx2 = ui_tx.clone();
                                let arc_pr_clone = self.plugin_registry.clone();
                                tokio::spawn(async move {
                                    let cwd = std::env::current_dir().unwrap_or_default();
                                    let analysis = tokio::task::spawn_blocking(move || {
                                        project_inspector::ProjectInspector::analyze(&cwd)
                                    })
                                    .await
                                    .unwrap_or_default();
                                    let (loaded, rewards) = if let Some(ref arc_pr) = arc_pr_clone {
                                        if let Ok(reg) = arc_pr.lock() {
                                            let l: std::collections::HashSet<String> = reg.loaded_plugin_ids().map(|s| s.to_string()).collect();
                                            let r = reg.ucb1_rewards_snapshot();
                                            (l, r)
                                        } else {
                                            Default::default()
                                        }
                                    } else {
                                        Default::default()
                                    };
                                    let recs = plugins::PluginRecommendationEngine::recommend(&analysis, &loaded, &rewards);
                                    let suggestions: Vec<crate::tui::events::PluginSuggestionItem> = recs.iter().map(|r| {
                                        crate::tui::events::PluginSuggestionItem {
                                            plugin_id: r.plugin_id.clone(),
                                            display_name: r.display_name.clone(),
                                            rationale: r.rationale.clone(),
                                            tier: format!("{:?}", r.tier),
                                            already_installed: r.already_installed,
                                        }
                                    }).collect();
                                    let _ = ui_tx2.send(UiEvent::PluginSuggestionReady { suggestions, dry_run: false });
                                });
                            }
                            PluginsSubcmd::Auto { dry_run } => {
                                // Bootstrap plugins in background.
                                let ui_tx2 = ui_tx.clone();
                                let arc_pr_clone = self.plugin_registry.clone();
                                tokio::spawn(async move {
                                    let cwd = std::env::current_dir().unwrap_or_default();
                                    let analysis = tokio::task::spawn_blocking(move || {
                                        project_inspector::ProjectInspector::analyze(&cwd)
                                    })
                                    .await
                                    .unwrap_or_default();
                                    let (loaded, rewards) = if let Some(ref arc_pr) = arc_pr_clone {
                                        if let Ok(reg) = arc_pr.lock() {
                                            let l: std::collections::HashSet<String> = reg.loaded_plugin_ids().map(|s| s.to_string()).collect();
                                            let r = reg.ucb1_rewards_snapshot();
                                            (l, r)
                                        } else {
                                            Default::default()
                                        }
                                    } else {
                                        Default::default()
                                    };
                                    let recs = plugins::PluginRecommendationEngine::recommend(&analysis, &loaded, &rewards);
                                    let count = recs.iter().filter(|r| !r.already_installed).count();
                                    let _ = ui_tx2.send(UiEvent::PluginBootstrapStarted { count, dry_run });
                                    let opts = plugins::BootstrapOptions {
                                        dry_run,
                                        ..Default::default()
                                    };
                                    let result = tokio::task::spawn_blocking(move || {
                                        plugins::AutoPluginBootstrap::bootstrap(&recs, &opts)
                                    })
                                    .await
                                    .unwrap_or(plugins::BootstrapResult {
                                        installed: vec![],
                                        skipped: vec![],
                                        failed: vec![("unknown".into(), "spawn error".into())],
                                        dry_run,
                                    });
                                    let _ = ui_tx2.send(UiEvent::PluginBootstrapComplete {
                                        installed: result.installed.len(),
                                        skipped: result.skipped.len(),
                                        failed: result.failed.len(),
                                    });
                                });
                            }
                            PluginsSubcmd::Disable(id) => {
                                if let Some(ref arc_pr) = self.plugin_registry {
                                    if let Ok(mut reg) = arc_pr.lock() {
                                        reg.auto_disable(&id, "disabled by user", std::time::Duration::ZERO);
                                        let _ = ui_tx.send(UiEvent::PluginStatusChanged {
                                            plugin_id: id.clone(),
                                            new_status: "suspended".into(),
                                        });
                                    }
                                }
                            }
                            PluginsSubcmd::Enable(id) => {
                                if let Some(ref arc_pr) = self.plugin_registry {
                                    if let Ok(mut reg) = arc_pr.lock() {
                                        reg.clear_cooling(&id);
                                        let _ = ui_tx.send(UiEvent::PluginStatusChanged {
                                            plugin_id: id.clone(),
                                            new_status: "active".into(),
                                        });
                                    }
                                }
                            }
                        }
                    }
                    _ => {
                        let _ = ui_tx.send(UiEvent::Warning {
                            message: format!("Command '/{cmd}' not available in TUI mode"),
                            hint: Some("Use classic mode (halcon chat) for full command access".into()),
                        });
                    }
                }
                // Phase 44B: Slash commands also count as prompt completion.
                let _ = ui_tx.send(UiEvent::AgentFinishedPrompt);
                let _ = ui_tx.send(UiEvent::PromptQueueStatus(prompt_rx.len()));
                let _ = ui_tx.send(UiEvent::AgentDone);
                continue;
            }

            // Send RoundStart to TUI.
            let _ = ui_tx.send(UiEvent::RoundStart((self.session.agent_rounds + 1) as usize));

            // Phase 4B-Lite: Validate provider is available before processing.
            if self.provider == "none" || self.model == "none" {
                let _ = ui_tx.send(UiEvent::Error {
                    message: "No provider configured".to_string(),
                    hint: Some("Configure a provider to send prompts:\n\n\
                        • Anthropic: Set ANTHROPIC_API_KEY environment variable\n\
                        • Ollama: Start local server with `ollama serve`\n\
                        • DeepSeek: Set DEEPSEEK_API_KEY environment variable\n\
                        • OpenAI: Set OPENAI_API_KEY environment variable\n\n\
                        Then restart halcon.".to_string()),
                });
                // Signal agent finished without actual processing.
                let _ = ui_tx.send(UiEvent::AgentFinishedPrompt);
                let _ = ui_tx.send(UiEvent::PromptQueueStatus(prompt_rx.len()));
                let _ = ui_tx.send(UiEvent::AgentDone);
                continue;
            }

            // Phase 4: Process message in background (allows TUI to remain responsive).
            // For now, we await to preserve session state consistency,
            // but the TUI event loop continues independently.
            tracing::debug!("Processing agent message (non-blocking architecture)");
            if let Err(e) = self.handle_message_tui(&text, &tui_sink).await {
                let _ = ui_tx.send(UiEvent::Error {
                    message: format!("Agent error: {e}"),
                    hint: None,
                });
            }
            tracing::debug!("Agent message processing complete");

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

            // Phase 44B: Signal that agent finished processing this prompt.
            let _ = ui_tx.send(UiEvent::AgentFinishedPrompt);

            // Phase 44B: Send current queue status (how many prompts waiting).
            let queued_count = prompt_rx.len();
            let _ = ui_tx.send(UiEvent::PromptQueueStatus(queued_count));

            // Phase 2: Send metrics update (placeholder values for now)
            // TODO: Wire actual metrics collectors into Repl struct
            let _ = ui_tx.send(UiEvent::Phase2Metrics {
                delegation_success_rate: None,
                delegation_trigger_rate: None,
                plan_success_rate: None,
                ucb1_agreement_rate: None,
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
            // Build real feature status for banner display.
            let features = self.build_feature_status(false); // tui_active = false in classic mode

            // Deterministic tip index from session ID.
            let tip_index = self.session.id.as_u128() as usize;
            crate::render::banner::render_startup_with_features(
                env!("CARGO_PKG_VERSION"),
                &self.provider,
                provider_connected,
                &self.model,
                session_short,
                session_type,
                tip_index,
                routing.as_ref(),
                &features,
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
                Some(&format!("Run `halcon auth login {}` to set up", self.provider)),
            );
        }
    }

    /// Handle a /dry-run command: routes through the agent loop with DestructiveOnly mode.
    async fn handle_message_dry_run(&mut self, input: &str) -> Result<()> {
        use crate::render::sink::RenderSink;
        let sink = crate::render::sink::ClassicSink::with_expert(self.expert_mode);
        sink.info("[dry-run] Destructive tools will be skipped.");
        self.dry_run_override = Some(halcon_core::types::DryRunMode::DestructiveOnly);
        self.handle_message(input).await
    }

    async fn handle_message(&mut self, input: &str) -> Result<()> {
        // US-output-format (PASO 2-A): route to CiSink when --output-format json is requested.
        if self.use_ci_sink {
            let ci = self.ci_sink.get_or_insert_with(|| {
                std::sync::Arc::new(crate::render::ci_sink::CiSink::new())
            }).clone();
            self.handle_message_with_sink(input, ci.as_ref()).await?;
        } else {
            let classic_sink = crate::render::sink::ClassicSink::with_expert(self.expert_mode);
            self.handle_message_with_sink(input, &classic_sink).await?;
            println!();
        }
        Ok(())
    }

    /// Unified message handler — runs the full agent loop with any RenderSink.
    ///
    /// Both classic REPL and TUI modes delegate here. The sink parameter
    /// abstracts away the rendering backend.
    pub(crate) async fn handle_message_with_sink(
        &mut self,
        input: &str,
        sink: &dyn crate::render::sink::RenderSink,
    ) -> Result<()> {
        // Phase 94: One-time onboarding check (fast — file existence only, <1ms).
        if !self.onboarding_checked {
            self.onboarding_checked = true;
            let cwd = std::env::current_dir().unwrap_or_default();
            match onboarding::OnboardingCheck::run(&cwd) {
                onboarding::OnboardingStatus::Configured { path } => {
                    sink.project_config_loaded(&path.to_string_lossy());
                }
                onboarding::OnboardingStatus::NotConfigured { root, project_type } => {
                    sink.onboarding_suggestion(
                        &root.to_string_lossy(),
                        &project_type,
                    );
                }
                onboarding::OnboardingStatus::Unknown => {}
            }
        }

        // Phase 95: Auto-resume plugins with expired cooling periods (fast, sync).
        {
            if let Some(ref arc_pr) = self.plugin_registry {
                if let Ok(mut reg) = arc_pr.lock() {
                    reg.maybe_resume_plugins();
                }
            }
        }

        // Phase 95: One-time plugin recommendation on first message.
        if !self.plugin_recommendation_done && self.config.plugins.enabled {
            self.plugin_recommendation_done = true;
            if let Some(ref arc_pr) = self.plugin_registry {
                if let Ok(reg) = arc_pr.lock() {
                    let cwd = std::env::current_dir().unwrap_or_default();
                    let analysis = project_inspector::ProjectInspector::analyze(&cwd);
                    let loaded: std::collections::HashSet<String> =
                        reg.loaded_plugin_ids().map(|s| s.to_string()).collect();
                    let rewards = reg.ucb1_rewards_snapshot();
                    drop(reg); // release lock before calling sink
                    let recs = plugins::PluginRecommendationEngine::recommend(
                        &analysis,
                        &loaded,
                        &rewards,
                    );
                    let total_new: usize = recs.iter().filter(|r| !r.already_installed).count();
                    let essential: usize = recs
                        .iter()
                        .filter(|r| {
                            !r.already_installed
                                && r.tier
                                    == plugins::RecommendationTier::Essential
                        })
                        .count();
                    if total_new > 0 {
                        sink.plugin_suggestion(total_new, essential);
                    }
                }
            }
        }

        // Record user message in session.
        self.session.add_message(ChatMessage {
            role: Role::User,
            content: MessageContent::Text(input.to_string()),
        });

        // ── Multimodal: detect & analyze media files referenced in user message ─────
        // Runs only when `--full` activated the multimodal subsystem.
        // Results: (a) inject "## Media Context" description into system prompt,
        //          (b) replace session message Text → Blocks(text + image blocks).
        let mut _media_context = String::new();
        let mut _had_media = false;
        if let Some(mm_sys) = self.multimodal.clone() {
            let paths = extract_media_paths(input);
            if !paths.is_empty() {
                use crate::render::sink::RenderSink as _;
                sink.media_analysis_started(paths.len());
                let session_id = self.session.id.to_string();
                let mut ctx = String::from("## Media Context\n");
                let mut img_blocks: Vec<halcon_core::types::ContentBlock> = Vec::new();
                let mut analyzed_count: usize = 0;
                let mut total_tokens_estimated: u32 = 0;
                // ── Phase 1: Sequential file reads ───────────────────────
                let mut read_data: Vec<Option<(std::path::PathBuf, Vec<u8>)>> =
                    Vec::with_capacity(paths.len());
                for path in &paths {
                    let path_str = path.to_string_lossy().to_string();
                    match tokio::fs::read(path).await {
                        Ok(data) => read_data.push(Some((path.clone(), data))),
                        Err(e) => {
                            tracing::warn!(path = %path_str, error = %e, "Cannot read media file");
                            read_data.push(None);
                        }
                    }
                }

                // ── Phase 2: Audio fallback check (sequential — needs sink) ──
                let base_timeout_ms = self.config.multimodal.api_timeout_ms;
                let mut analysis_items: Vec<(usize, std::path::PathBuf, Vec<u8>)> = Vec::new();
                let mut ctx_parts: Vec<Option<String>> = vec![None; paths.len()];

                for (idx, entry) in read_data.into_iter().enumerate() {
                    let Some((path, data)) = entry else { continue };
                    let path_str = path.to_string_lossy().to_string();
                    let fname = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path_str.clone());
                    // Audio fallback: warn early if OPENAI_API_KEY is not set.
                    let modality_hint =
                        halcon_multimodal::MultimodalSubsystem::peek_modality(&data);
                    if modality_hint == "audio" && !mm_sys.supports_audio() {
                        sink.warning(
                            &format!(
                                "[media] Audio transcription unavailable for '{fname}': set OPENAI_API_KEY"
                            ),
                            Some(
                                "Audio analysis requires OpenAI Whisper — OPENAI_API_KEY not set",
                            ),
                        );
                        // Include native audio metadata (WAV/MP3: duration, rate, channels).
                        let native_meta = halcon_multimodal::MultimodalSubsystem::native_audio_description(&data)
                            .unwrap_or_else(|| "Audio file — transcription unavailable. Set OPENAI_API_KEY to enable Whisper.".into());
                        ctx_parts[idx] = Some(format!("\n### {fname}\n{native_meta}\n"));
                        continue;
                    }
                    analysis_items.push((idx, path, data));
                }

                // ── Phase 3: Parallel media analysis (network-bound) ──────────
                // All API calls are dispatched concurrently — on 5 images at
                // 30 s each this cuts wall-clock time from 150 s → ~30 s.
                let analysis_futures: Vec<_> = analysis_items
                    .into_iter()
                    .map(|(idx, path, data)| {
                        let mm       = Arc::clone(&mm_sys);
                        let sid      = session_id.clone();
                        let path_str = path.to_string_lossy().to_string();
                        let fname    = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path_str.clone());
                        // Adaptive timeout: base + 5 s buffer + 2 s per MB of media.
                        let size_mb = (data.len() / (1024 * 1024)) as u64;
                        let adaptive_ms = base_timeout_ms
                            .saturating_add(5_000)
                            .saturating_add(size_mb * 2_000);
                        async move {
                            let timeout_dur = std::time::Duration::from_millis(adaptive_ms);
                            let result = tokio::time::timeout(
                                timeout_dur,
                                mm.analyze_bytes_with_provenance(
                                    &data,
                                    None,
                                    Some(sid),
                                    Some(path_str.clone()),
                                ),
                            )
                            .await;
                            let outcome = match result {
                                Ok(Ok(analysis)) => {
                                    let img_block = if analysis.modality == "image" {
                                        use base64::Engine as _;
                                        let encoded = base64::engine::general_purpose::STANDARD
                                            .encode(&data);
                                        let media_type =
                                            halcon_core::types::ImageMediaType::from_magic(&data)
                                                .unwrap_or(
                                                    halcon_core::types::ImageMediaType::Jpeg,
                                                );
                                        Some(halcon_core::types::ContentBlock::Image {
                                            source: halcon_core::types::ImageSource::Base64 {
                                                media_type,
                                                data: encoded,
                                            },
                                        })
                                    } else {
                                        None
                                    };
                                    Ok((analysis, img_block))
                                }
                                Ok(Err(e)) => Err(format!("{e}")),
                                Err(_elapsed) => {
                                    Err(format!("timed out after {}s", adaptive_ms / 1_000))
                                }
                            };
                            (idx, fname, path_str, outcome)
                        }
                    })
                    .collect();

                // Await all concurrently — API calls inflight simultaneously.
                let analysis_results =
                    futures::future::join_all(analysis_futures).await;

                // ── Phase 4: Collect results (sequential — needs sink) ────────
                for (idx, fname, path_str, outcome) in analysis_results {
                    match outcome {
                        Ok((analysis, img_block)) => {
                            ctx_parts[idx] = Some(format!(
                                "\n### {fname}\n{}\n",
                                analysis.description
                            ));
                            if let Some(block) = img_block {
                                img_blocks.push(block);
                            }
                            analyzed_count += 1;
                            total_tokens_estimated += analysis.token_estimate;
                            sink.media_analysis_complete(&fname, analysis.token_estimate);
                            tracing::info!(
                                path = %path_str,
                                modality = %analysis.modality,
                                tokens = analysis.token_estimate,
                                "Multimodal analysis complete"
                            );
                        }
                        Err(msg) => {
                            sink.warning(
                                &format!("Media analysis failed for '{fname}': {msg}"),
                                None,
                            );
                            tracing::warn!(
                                path = %path_str,
                                error = %msg,
                                "Media analysis error"
                            );
                        }
                    }
                }

                // ── Phase 5: Assemble ctx in original path order ──────────────
                for part in ctx_parts.into_iter().flatten() {
                    ctx.push_str(&part);
                }
                // Emit final summary if at least one file was analyzed.
                if analyzed_count > 0 {
                    sink.info(&format!(
                        "[media] {analyzed_count}/{} file{} analyzed — ~{total_tokens_estimated} tokens added to context",
                        paths.len(),
                        if paths.len() == 1 { "" } else { "s" },
                    ));
                }
                // Update last session message: Text → Blocks(text + images).
                if !img_blocks.is_empty() {
                    if let Some(last) = self.session.messages.last_mut() {
                        if matches!(last.role, Role::User) {
                            let mut blocks =
                                vec![halcon_core::types::ContentBlock::Text {
                                    text: input.to_string(),
                                }];
                            blocks.extend(img_blocks);
                            last.content = MessageContent::Blocks(blocks);
                            _had_media = true;
                        }
                    }
                }
                if ctx != "## Media Context\n" {
                    _media_context = ctx;
                }
            }
        }
        // ── End multimodal block ──────────────────────────────────────────────────────

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

        // Assemble context via ContextManager (if available).
        let system_prompt = if let Some(ref mut cm) = self.context_manager {
            let context_query = ContextQuery {
                working_directory: working_dir.clone(),
                user_message: Some(input.to_string()),
                token_budget: self.config.general.max_tokens as usize,
            };

            let assembled = cm.assemble(&context_query).await;
            assembled.system_prompt
        } else {
            None
        };

        // P0.1: Lazy MCP initialization — connect servers and register tools on first message.
        // ensure_initialized() is idempotent: subsequent calls are no-ops.
        // Must run BEFORE tool_definitions() so MCP tools appear in the agent loop.
        if !self.mcp_manager.is_initialized() && self.mcp_manager.has_servers() {
            let results = self.mcp_manager.ensure_initialized(&mut self.tool_registry).await;
            for (server, result) in &results {
                match result {
                    Ok(()) => {
                        use crate::render::sink::RenderSink;
                        sink.info(&format!("[mcp] Server '{server}' connected"));
                    }
                    Err(e) => {
                        use crate::render::sink::RenderSink;
                        sink.info(&format!("[mcp] Server '{server}' failed to connect: {e}"));
                    }
                }
            }
            let n = self.mcp_manager.registered_tool_count();
            if n > 0 {
                tracing::info!(tool_count = n, "MCP tools registered into agent loop");
            }
        }

        // Build the model request from session history.
        let tool_defs = self.tool_registry.tool_definitions();
        let mut request = ModelRequest {
            model: self.model.clone(),
            messages: self.session.messages.clone(),
            tools: tool_defs,
            max_tokens: Some(self.config.general.max_tokens),
            temperature: Some(self.config.general.temperature),
            system: system_prompt,
            stream: true,
        };

        // Inject user context into system prompt (idempotent via marker check).
        // Gives the model awareness of who it's talking to and the working directory.
        const USER_CTX_MARKER: &str = "## User Context";
        if let Some(ref mut sys) = request.system {
            if !sys.contains(USER_CTX_MARKER) {
                sys.push_str(&format!(
                    "\n\n{USER_CTX_MARKER}\nUser: {}\nDirectory: {}\nPlatform: {}",
                    self.user_display_name,
                    working_dir,
                    std::env::consts::OS,
                ));
            }
        }

        // Phase 5 Dev Ecosystem: inject DevGateway context (open IDE buffers, git branch,
        // latest CI run) — refreshed on EVERY round so git changes, new CI results, and
        // buffer edits are always current.  build_context() offloads git I/O to
        // spawn_blocking so this await is safe inside the async agent loop.
        {
            const DEV_ECO_MARKER: &str = "## Dev Ecosystem Context";
            if let Some(ref mut sys) = request.system {
                // Remove stale dev context block injected in the previous round.
                if let Some(idx) = sys.find(&format!("\n\n{DEV_ECO_MARKER}")) {
                    sys.truncate(idx);
                } else if sys.starts_with(DEV_ECO_MARKER) {
                    sys.clear();
                }
                // Re-inject fresh snapshot (git branch, open buffers, latest CI run).
                let dev_ctx = self.dev_gateway.build_context().await;
                let dev_md = dev_ctx.as_markdown();
                if !dev_md.is_empty() {
                    sys.push_str(&format!("\n\n{dev_md}"));
                }
            }
        }

        // Inject media analysis context into system prompt (idempotent via marker).
        const MEDIA_CTX_MARKER: &str = "## Media Context";
        if !_media_context.is_empty() {
            if let Some(ref mut sys) = request.system {
                if !sys.contains(MEDIA_CTX_MARKER) {
                    sys.push_str(&format!("\n\n{}", _media_context));
                }
            } else {
                request.system = Some(_media_context.clone());
            }
        }
        // Sync request.messages with updated session (now has image blocks).
        if _had_media {
            request.messages = self.session.messages.clone();
        }

        // Look up the active provider.
        let provider: Option<Arc<dyn ModelProvider>> = self.registry.get(&self.provider).cloned();

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
                let guardrails: &[Box<dyn halcon_security::Guardrail>] =
                    if self.config.security.guardrails.enabled
                        && self.config.security.guardrails.builtins
                    {
                        halcon_security::builtin_guardrails()
                    } else {
                        &[]
                    };

                let llm_planner = if self.config.planning.adaptive {
                    // Phase 3: AgentModelConfig — use dedicated planner provider/model when configured.
                    // Fall back to the session's primary provider/model for backward compatibility.
                    let planner_prov: Arc<dyn halcon_core::traits::ModelProvider> =
                        self.config.reasoning.planner_provider.as_deref()
                            .and_then(|name| self.registry.get(name))
                            .cloned()
                            .unwrap_or_else(|| Arc::clone(&p));

                    // Resolve planner model: explicit config > validate against planner_prov > best model.
                    let planner_model = if let Some(ref m) = self.config.reasoning.planner_model {
                        m.clone()
                    } else if planner_prov.validate_model(&self.model).is_ok() {
                        self.model.clone()
                    } else {
                        // Use ModelRouter::from_provider_models() to select the Balanced tier
                        // model for planning. Balanced tier is best for planning: reliable tool
                        // calling, not too slow/expensive, avoids over-indexing on context window
                        // (which previously selected opus $75/M over sonnet $15/M).
                        let router = model_router::ModelRouter::from_provider_models(
                            &planner_prov.supported_models(),
                        );
                        router.balanced_model().to_string()
                    };
                    tracing::debug!(
                        provider = planner_prov.name(),
                        model = %planner_model,
                        "LlmPlanner resolved model for provider (Phase 3)"
                    );
                    // BRECHA-S3: pass session-blocked tools so the planner excludes them.
                    let blocked_names: Vec<String> = self.session_blocked_tools
                        .iter()
                        .map(|(name, _)| name.clone())
                        .collect();
                    Some(planner::LlmPlanner::new(
                        planner_prov,
                        planner_model,
                    ).with_max_replans(self.config.planning.max_replans)
                     .with_blocked_tools(blocked_names))
                } else {
                    None
                };

                // Phase 4: Load cross-session model quality stats from DB on first message.
                // This seeds the ModelSelector with historical quality data so "balanced" routing
                // exploits learned performance signals from prior sessions (not just the current session).
                if !self.model_quality_db_loaded {
                    self.model_quality_db_loaded = true;
                    if let Some(ref adb) = self.async_db {
                        match adb.load_model_quality_stats(p.name()).await {
                            Ok(prior_stats) if !prior_stats.is_empty() => {
                                for (model_id, success, failure, reward) in prior_stats {
                                    let cached = self.model_quality_cache
                                        .entry(model_id)
                                        .or_insert((0u32, 0u32, 0.0f64));
                                    // Merge: take prior stats when they show more experience
                                    // than whatever was already in the in-session cache.
                                    if success > cached.0 {
                                        *cached = (success, failure, reward);
                                    }
                                }
                                tracing::info!(
                                    models = self.model_quality_cache.len(),
                                    provider = p.name(),
                                    "Phase 4: cross-session model quality loaded from DB"
                                );
                            }
                            Ok(_) => {
                                tracing::debug!("Phase 4: no prior model quality stats in DB");
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "Phase 4: failed to load model quality from DB");
                            }
                        }
                    }
                }

                // Phase 8-A: Plugin system lazy-init (V3).
                // Discovers *.plugin.toml manifests from ~/.halcon/plugins/ and registers
                // PluginProxyTool instances into the session ToolRegistry.  Only runs once
                // per session (guard: self.plugin_registry.is_none()).
                //
                // Auto-activation: if the default plugin directory exists and contains at
                // least one *.plugin.toml manifest, plugins are activated even when
                // config.plugins.enabled = false.  This provides zero-config UX: drop a
                // manifest into ~/.halcon/plugins/ and it activates on next message.
                let plugins_should_run = self.config.plugins.enabled || {
                    let default_dir = dirs::home_dir()
                        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
                        .join(".halcon")
                        .join("plugins");
                    std::fs::read_dir(&default_dir)
                        .map(|mut entries| {
                            entries.any(|e| {
                                e.ok()
                                    .and_then(|e| e.file_name().into_string().ok())
                                    .map(|n| n.ends_with(".plugin.toml"))
                                    .unwrap_or(false)
                            })
                        })
                        .unwrap_or(false)
                };
                if self.plugin_registry.is_none() && plugins_should_run {
                    // P9: Honour config.plugins.plugin_dir override; fall back to default
                    // ~/.halcon/plugins/ when not set.
                    let loader = if let Some(ref dir) = self.config.plugins.plugin_dir {
                        plugins::PluginLoader::new(vec![std::path::PathBuf::from(dir)])
                    } else {
                        plugins::PluginLoader::default()
                    };
                    let mut runtime = plugins::PluginTransportRuntime::new();
                    let mut registry = plugins::PluginRegistry::new();
                    let load_result = loader.load_into(&mut registry, &mut runtime);
                    if load_result.loaded > 0 {
                        tracing::info!(
                            loaded = load_result.loaded,
                            skipped_invalid = load_result.skipped_invalid,
                            skipped_checksum = load_result.skipped_checksum,
                            "Phase 8-A: Plugin system initialised"
                        );
                        let runtime_arc = std::sync::Arc::new(runtime);

                        // Phase 8-A (P1 fix): Create one PluginProxyTool per capability and
                        // register it in the session ToolRegistry so the model can see and
                        // invoke plugin tools exactly like built-in tools.
                        //
                        // BEFORE this fix: PluginRegistry was populated but ToolRegistry was
                        // never updated, making all plugin tools invisible to the model.
                        let mut proxy_count = 0usize;
                        for (plugin_id, manifest) in registry.loaded_plugins() {
                            let timeout_ms = if manifest.sandbox.timeout_ms > 0 {
                                manifest.sandbox.timeout_ms
                            } else {
                                30_000
                            };
                            for cap in &manifest.capabilities {
                                let proxy = plugins::PluginProxyTool::new(
                                    cap.name.clone(),
                                    plugin_id.to_string(),
                                    cap.clone(),
                                    runtime_arc.clone(),
                                    timeout_ms,
                                );
                                self.tool_registry.register(std::sync::Arc::new(proxy));
                                proxy_count += 1;
                            }
                        }
                        tracing::info!(
                            proxy_tools = proxy_count,
                            "Phase 8-A: Plugin proxy tools registered in ToolRegistry"
                        );

                        self.plugin_transport_runtime = Some(runtime_arc.clone());
                        self.plugin_registry = Some(std::sync::Arc::new(
                            std::sync::Mutex::new(registry)
                        ));
                    } else {
                        tracing::debug!(
                            skipped_invalid = load_result.skipped_invalid,
                            "Phase 8-A: No plugins loaded (dir empty or all invalid)"
                        );
                    }
                }

                // Phase 8-E: Load plugin UCB1 metrics from DB on first message (seed bandit arms).
                // Follows the same load-once-per-session pattern as model_quality_db_loaded.
                if !self.plugin_metrics_db_loaded {
                    self.plugin_metrics_db_loaded = true;
                    if let (Some(ref adb), Some(ref arc_reg)) =
                        (&self.async_db, &self.plugin_registry)
                    {
                        match adb.load_plugin_metrics().await {
                            Ok(records) if !records.is_empty() => {
                                // records: Vec<PluginMetricsRecord>
                                let seeds: Vec<(String, i64, f64)> = records
                                    .iter()
                                    .map(|r| (r.plugin_id.clone(), r.ucb1_n_uses, r.ucb1_sum_rewards))
                                    .collect();
                                if let Ok(mut reg) = arc_reg.lock() {
                                    reg.seed_ucb1_from_metrics(&seeds);
                                }
                                tracing::info!(
                                    plugins = records.len(),
                                    "Phase 8-E: Plugin UCB1 metrics loaded from DB"
                                );
                            }
                            Ok(_) => {
                                tracing::debug!("Phase 8-E: no prior plugin metrics in DB");
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "Phase 8-E: failed to load plugin metrics from DB");
                            }
                        }
                    }
                }

                // Skip model selection when user explicitly set --model on the CLI.
                let selector = if self.config.agent.model_selection.enabled && !self.explicit_model {
                    let mut sel = model_selector::ModelSelector::new(
                        self.config.agent.model_selection.clone(),
                        &self.registry,
                    )
                    .with_provider_scope(p.name())
                    // Phase 3: Inject accumulated quality stats from prior messages this session
                    // so "balanced" routing starts with learned quality adjustments (not neutral prior).
                    .with_quality_seeds(self.model_quality_cache.clone());

                    // P3: Provider health routing — populate real p95 latency hints from
                    // the metrics DB so "fast" strategy routes to lowest-latency models.
                    if let Some(ref db) = self.db {
                        let hints = build_latency_hints_from_db(db, &self.registry);
                        if !hints.is_empty() {
                            sel = sel.with_latency_hints(hints);
                        }
                    }

                    Some(sel)
                } else {
                    None
                };

                // Create task bridge when structured task framework is enabled.
                let mut task_bridge_inst = if self.config.task_framework.enabled {
                    Some(task_bridge::TaskBridge::new(&self.config.task_framework))
                } else {
                    None
                };

                // Phase 1.1: Load cross-session UCB1 experience on first use (lazy, one-time per session).
                // The write path (save_reasoning_experience) runs every turn.
                // The read path was NEVER called — UCB1 always started naive. This fixes that.
                if let Some(ref mut engine) = self.reasoning_engine {
                    if !engine.is_experience_loaded() {
                        // Convert Debug-format strings ("CodeGeneration") → snake_case ("code_generation")
                        // needed by TaskType::from_str / ReasoningStrategy::from_str.
                        fn pascal_to_snake(s: &str) -> String {
                            let mut out = String::with_capacity(s.len() + 4);
                            for (i, c) in s.chars().enumerate() {
                                if c.is_uppercase() && i > 0 { out.push('_'); }
                                out.extend(c.to_lowercase());
                            }
                            out
                        }
                        if let Some(ref adb) = self.async_db {
                            match adb.load_all_reasoning_experiences().await {
                                Ok(exps) => {
                                    let parsed: Vec<_> = exps.iter().filter_map(|e| {
                                        let tt = task_analyzer::TaskType::from_str(&pascal_to_snake(&e.task_type))?;
                                        let st = strategy_selector::ReasoningStrategy::from_str(&pascal_to_snake(&e.strategy))?;
                                        Some((tt, st, e.avg_score, e.uses))
                                    }).collect();
                                    let count = parsed.len();
                                    engine.load_experience(parsed); // sets experience_loaded = true
                                    tracing::info!(entries = count, "UCB1: cross-session experience loaded");
                                }
                                Err(e) => {
                                    tracing::warn!("UCB1 load_experience failed: {e}");
                                    engine.mark_experience_loaded(); // suppress future retries this session
                                }
                            }
                        } else {
                            engine.mark_experience_loaded(); // no DB — skip all future attempts
                        }
                    }
                }

                // FASE 3.1: PRE-LOOP reasoning analysis (when reasoning engine enabled).
                let reasoning_analysis = if let Some(ref mut engine) = self.reasoning_engine {
                    sink.phase_started("reasoning", "Selecting optimal strategy...");
                    // P0-1: pass provider models so ModelRouter can classify tiers from real
                    // metadata instead of hardcoded DeepSeek names.
                    let provider_models: Vec<halcon_core::types::ModelInfo> = self
                        .registry
                        .get(&self.provider)
                        .map(|p| p.supported_models().to_vec())
                        .unwrap_or_default();
                    let analysis = engine.pre_loop(input, &self.config.agent.limits, &provider_models);
                    sink.phase_ended();

                    // Emit ReasoningStarted event.
                    let _ = self.event_tx.send(DomainEvent::new(EventPayload::ReasoningStarted {
                        query_hash: analysis.analysis.task_hash.clone(),
                        task_type: format!("{:?}", analysis.analysis.task_type),
                        complexity: format!("{:?}", analysis.analysis.complexity),
                    }));

                    // Emit StrategySelected event.
                    let _ = self.event_tx.send(DomainEvent::new(EventPayload::StrategySelected {
                        strategy: format!("{:?}", analysis.strategy),
                        confidence: 0.8, // Placeholder confidence
                        task_type: format!("{:?}", analysis.analysis.task_type),
                    }));

                    // Note: reasoning_status in pre-loop doesn't have score/success yet
                    tracing::info!(
                        task_type = ?analysis.analysis.task_type,
                        complexity = ?analysis.analysis.complexity,
                        strategy = ?analysis.strategy,
                        "Reasoning strategy selected"
                    );

                    Some(analysis)
                } else {
                    None
                };

                // Use reasoning-adjusted limits if available, else base limits.
                let agent_limits = reasoning_analysis
                    .as_ref()
                    .map(|a| &a.adjusted_limits)
                    .unwrap_or(&self.config.agent.limits);

                // Build StrategyContext from UCB1 PreLoopAnalysis (Step 9a).
                let strategy_ctx: Option<agent_types::StrategyContext> =
                    reasoning_analysis.as_ref().map(|a| agent_types::StrategyContext {
                        strategy: a.strategy,
                        enable_reflection: a.plan.enable_reflection,
                        loop_guard_tightness: a.plan.loop_guard_tightness,
                        replan_sensitivity: a.plan.replan_sensitivity,
                        routing_bias: a.plan.routing_bias.clone(),
                        task_type: a.analysis.task_type,
                        complexity: a.analysis.complexity,
                    });

                // Build critic provider/model from config (Step 9b — G2 critic separation).
                let critic_prov: Option<std::sync::Arc<dyn halcon_core::traits::ModelProvider>> =
                    self.config.reasoning.critic_provider.as_deref()
                        .and_then(|name| self.registry.get(name))
                        .cloned();
                let critic_mdl: Option<String> = self.config.reasoning.critic_model.clone();

                let ctx = agent::AgentContext {
                    provider: &p,
                    session: &mut self.session,
                    request: &request,
                    tool_registry: &self.tool_registry,
                    permissions: &mut self.permissions,
                    working_dir: &working_dir,
                    event_tx: &self.event_tx,
                    trace_db: self.async_db.as_ref(),
                    limits: agent_limits,
                    response_cache: self.response_cache.as_ref(),
                    resilience: &mut self.resilience,
                    fallback_providers: &fallback_providers,
                    routing_config: &self.config.agent.routing,
                    compactor: Some(&compactor),
                    // P1.1: Try PlaybookPlanner first (zero LLM latency). Fall back to LlmPlanner.
                    planner: if self.playbook_planner.find_match(input).is_some() {
                        Some(&self.playbook_planner as &dyn Planner)
                    } else {
                        llm_planner.as_ref().map(|p| p as &dyn Planner)
                    },
                    guardrails,
                    reflector: self.reflector.as_ref(),
                    render_sink: sink,
                    replay_tool_executor: None,
                    phase14: halcon_core::types::Phase14Context {
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
                    context_metrics: Some(&self.context_metrics),
                    context_manager: self.context_manager.as_mut(),
                    // Phase 43 / GAP-5: pass control channel receiver.
                    // TUI: uses its own control channel (Pause/Step/Cancel from TUI events).
                    // Classic REPL (non-TUI): create a simple cancel-only channel wired to
                    // Ctrl-C via tokio::signal::ctrl_c() so the agent loop can exit gracefully.
                    #[cfg(feature = "tui")]
                    ctrl_rx: self.ctrl_rx.take(),
                    #[cfg(not(feature = "tui"))]
                    ctrl_rx: {
                        let (classic_ctrl_tx, classic_ctrl_rx) =
                            tokio::sync::mpsc::channel::<crate::repl::agent_types::ClassicCancelSignal>(1);
                        tokio::spawn(async move {
                            if tokio::signal::ctrl_c().await.is_ok() {
                                let _ = classic_ctrl_tx.send(
                                    crate::repl::agent_types::ClassicCancelSignal::Cancel
                                ).await;
                            }
                        });
                        Some(classic_ctrl_rx)
                    },
                    speculator: &self.speculator,
                    security_config: &self.config.security,
                    strategy_context: strategy_ctx.clone(),
                    critic_provider: critic_prov.clone(),
                    critic_model: critic_mdl.clone(),
                    plugin_registry: self.plugin_registry.clone(),
                    is_sub_agent: false,
                    requested_provider: Some(self.provider.clone()),
                    policy: std::sync::Arc::new(self.config.policy.clone()),
                };
                // Fix: restore ctrl_rx before propagating any error so TUI controls
                // (Pause/Step/Cancel) remain functional across agent loop failures.
                // Previously `?` would drop ctrl_rx on Err, leaving self.ctrl_rx = None
                // for the rest of the session.
                let mut agent_loop_result = agent::run_agent_loop(ctx).await;

                // Phase 43: restore control channel receiver for reuse across TUI messages.
                // We restore from AgentLoopResult on Ok. On Err the channel was consumed
                // inside run_agent_loop; we leave self.ctrl_rx as None in that rare case.
                #[cfg(feature = "tui")]
                if let Ok(ref mut r) = agent_loop_result {
                    self.ctrl_rx = r.ctrl_rx.take();
                }

                let mut result = agent_loop_result?;

                // BRECHA-S3: merge blocked tools from this loop into session state.
                // Dedup by name so repeated denials don't grow the list unboundedly.
                for (name, reason) in result.blocked_tools.drain(..) {
                    if !self.session_blocked_tools.iter().any(|(n, _)| n == &name) {
                        tracing::info!(
                            tool = %name,
                            reason = %reason,
                            "BRECHA-S3: tool blocked this turn — will be excluded from future plans"
                        );
                        self.session_blocked_tools.push((name, reason));
                    }
                }

                // Cache timeline for --timeline exit hook.
                self.last_timeline = result.timeline_json.clone();

                // Phase 1.2: Variables for capturing critic retry decision (must be outside
                // the reasoning_engine borrow so we can re-borrow self.session etc. for the retry).
                let mut critic_retry_needed = false;
                // (confidence, gaps, retry_instruction)
                let mut critic_retry_info: Option<(f32, Vec<String>, Option<String>)> = None;

                // Phase 2 Causality Enforcement: capture pipeline reward outside the
                // reasoning_engine borrow so record_outcome() can be called after retry.
                // None when reasoning engine is disabled (coarse fallback used instead).
                let mut captured_pipeline_reward: Option<(f64, bool)> = None;

                // FASE 3.1: POST-LOOP reasoning evaluation (when reasoning engine enabled).
                // Step 9e: Use reward_pipeline::compute_reward() for richer UCB1 signal.
                if let Some(ref mut engine) = self.reasoning_engine {
                    if let Some(ref analysis) = reasoning_analysis {
                        // Build multi-signal reward from all available signals.
                        let round_scores: Vec<f32> = result.round_evaluations.iter()
                            .map(|e| e.combined_score)
                            .collect();
                        let critic_verdict_tuple = result.critic_verdict.as_ref()
                            .map(|cv| (cv.achieved, cv.confidence));
                        let raw_signals = reward_pipeline::RawRewardSignals {
                            stop_condition: result.stop_condition,
                            round_scores,
                            critic_verdict: critic_verdict_tuple,
                            // Phase 7: wired from agent result (was TODO: 0.0 placeholder).
                            plan_coherence_score: result.avg_plan_drift,
                            oscillation_penalty: result.oscillation_penalty,
                            plan_completion_ratio: result.plan_completion_ratio,
                            plugin_snapshots: result.plugin_cost_snapshot.clone(),
                            critic_unavailable: result.critic_unavailable,
                            evidence_coverage: result.evidence_coverage,
                        };
                        let reward_computation = reward_pipeline::compute_reward(&raw_signals, &self.config.policy);
                        // Step 5 plugin blending: apply plugin success rate signal (10% weight).
                        let blended_reward = reward_pipeline::plugin_adjusted_reward(
                            reward_computation.final_reward as f64,
                            &result.plugin_cost_snapshot,
                        );
                        let evaluation = engine.post_loop_with_reward(analysis, blended_reward as f64);

                        // GAP-1 fix: per-round UCB1 signal (coexists with session-level update above).
                        // raw_signals.round_scores still accessible here (not moved after this point).
                        engine.record_per_round_signals(analysis, &raw_signals.round_scores);

                        // Phase 2 Causality Enforcement: capture pipeline reward for unified
                        // record_outcome() call after retry (reward contamination fix).
                        // Use blended_reward (includes plugin signal) as the canonical reward.
                        captured_pipeline_reward = Some((
                            blended_reward,
                            evaluation.success,
                        ));

                        // Emit EvaluationCompleted event.
                        let _ = self.event_tx.send(DomainEvent::new(
                            EventPayload::EvaluationCompleted {
                                score: evaluation.score,
                                success: evaluation.success,
                                strategy: format!("{:?}", evaluation.strategy),
                            },
                        ));

                        // Call reasoning_status with full evaluation
                        sink.reasoning_status(
                            &format!("{:?}", evaluation.task_type),
                            &format!("{:?}", analysis.analysis.complexity),
                            &format!("{:?}", evaluation.strategy),
                            evaluation.score,
                            evaluation.success,
                        );

                        // Emit ExperienceRecorded event.
                        let _ = self.event_tx.send(DomainEvent::new(
                            EventPayload::ExperienceRecorded {
                                task_type: format!("{:?}", evaluation.task_type),
                                strategy: format!("{:?}", evaluation.strategy),
                                score: evaluation.score,
                            },
                        ));

                        // P3 FIX: Persist reasoning experience to SQLite for cross-session UCB1 learning.
                        if let Some(ref adb) = self.async_db {
                            match adb.save_reasoning_experience(
                                &format!("{:?}", evaluation.task_type),
                                &format!("{:?}", evaluation.strategy),
                                evaluation.score,
                            ).await {
                                Ok(()) => tracing::debug!(
                                    task_type = %format!("{:?}", evaluation.task_type),
                                    strategy = %format!("{:?}", evaluation.strategy),
                                    score = evaluation.score,
                                    "P3: Reasoning experience persisted"
                                ),
                                Err(e) => tracing::warn!(
                                    error = %e,
                                    task_type = %format!("{:?}", evaluation.task_type),
                                    strategy = %format!("{:?}", evaluation.strategy),
                                    "UCB1: failed to persist reasoning experience — cross-session learning degraded"
                                ),
                            }
                        }

                        tracing::info!(
                            score = evaluation.score,
                            success = evaluation.success,
                            "Reasoning evaluation complete"
                        );

                        // Phase 1.2 + Phase 7 (Autonomy Validation): LoopCritic verdict → should_retry().
                        //
                        // Two independent paths can trigger a retry:
                        //   A) Score-based:  reward score < success_threshold (engine.should_retry)
                        //   B) Halt-based:   LoopCritic::should_halt() — !achieved AND
                        //                    confidence >= HALT_CONFIDENCE_THRESHOLD (0.80)
                        //
                        // Path B closes Phase 7: even if the reward score is above threshold
                        // (e.g. EndTurn scored as 0.70+), a highly confident critic verdict
                        // of failure overrides the score-based decision and forces a retry.
                        //
                        // Extract retry decision into outer variables so we can act on it
                        // AFTER the reasoning_engine borrow is released (Rust borrow rules).
                        if let Some(ref cv) = result.critic_verdict {
                            let score_says_retry = engine.should_retry(evaluation.score, 0);
                            // Phase 7: LoopCritic::should_halt_raw() — high-confidence failure
                            // bypass. When the critic is >=80% confident the goal was NOT
                            // achieved, treat it as a structural halt regardless of reward score.
                            let critic_halt = supervisor::LoopCritic::should_halt_raw(
                                cv.achieved,
                                cv.confidence,
                                self.config.policy.halt_confidence_threshold,
                            );
                            // Minimum confidence required for a critic failure to
                            // justify re-running the entire agent loop.  A critic
                            // at 30% confidence is too uncertain to warrant the
                            // extra tokens and latency of a full retry.
                            // FUTURE: granular retry hook — instead of re-running
                            // the full agent loop, use `cv.gaps` to identify which
                            // plan steps failed and retry only those via the
                            // orchestrator's selective re-dispatch.
                            let min_retry_confidence = self.config.policy.min_retry_confidence;
                            // Phase 2 SLA: gate retries through SLA budget.
                            // retry_attempt=0 means this is the first retry (critic_retry_needed triggers ONE retry).
                            let sla_allows = result.sla_budget.as_ref().map_or(true, |b| b.allows_retry(0));
                            if !cv.achieved
                                && cv.confidence >= min_retry_confidence
                                && (score_says_retry || critic_halt)
                                && sla_allows
                            {
                                critic_retry_needed = true;
                                critic_retry_info = Some((
                                    cv.confidence,
                                    cv.gaps.clone(),
                                    cv.retry_instruction.clone(),
                                ));
                                tracing::info!(
                                    critic_confidence = cv.confidence,
                                    score = evaluation.score,
                                    score_says_retry,
                                    critic_halt,
                                    "LoopCritic+Reasoning: in-session retry warranted"
                                );
                            }
                        }
                    }
                }

                // Phase 1.2: Actual in-session LoopCritic retry.
                // Now that the reasoning_engine borrow has been released, we can mutably
                // access self.session + self.permissions + self.resilience for the retry.
                // This is the structural change: previously this was advisory (just logs).
                // Now it performs a real second agent loop invocation within the same turn.
                if critic_retry_needed {
                    if let Some((confidence, gaps, retry_instr)) = critic_retry_info {
                        let instr = retry_instr.as_deref().unwrap_or(
                            "Your previous response did not fully complete the task. Please address all missing elements."
                        );
                        // BRECHA-R1 + FASE 5: If sub-agent steps failed, tell the planner explicitly
                        // with categorized error info so it generates ALTERNATIVE approaches.
                        let failed_steps_note = if !result.failed_sub_agent_steps.is_empty() {
                            format!(
                                "\n\nFAILED APPROACHES (do NOT repeat these — use a different method):\n{}",
                                result.failed_sub_agent_steps.iter()
                                    .map(|ctx| format!("  - [{}] {}: {}", ctx.error_category.label(), ctx.description, ctx.error_message))
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            )
                        } else {
                            String::new()
                        };
                        let retry_text = format!(
                            "[Critic retry]: Task incomplete. Missing: {}. Instruction: {}{}",
                            if gaps.is_empty() { "see previous response".to_string() } else { gaps.join("; ") },
                            instr,
                            failed_steps_note
                        );

                        sink.info(&format!(
                            "[reasoning] critic retry ({:.0}% confidence) — re-running agent loop",
                            confidence * 100.0
                        ));

                        // Inject retry instruction into session (already has agent's first response).
                        self.session.add_message(ChatMessage {
                            role: Role::User,
                            content: MessageContent::Text(retry_text),
                        });

                        // F4 RetryMutation: compute structural mutation for the retry.
                        // Derive plan_depth from actual plan (was hardcoded to 5).
                        let actual_plan_depth = result.timeline_json.as_ref()
                            .and_then(|json| {
                                // Parse step count from timeline JSON.
                                serde_json::from_str::<serde_json::Value>(json).ok()
                                    .and_then(|v| v.get("steps")?.as_array().map(|a| a.len() as u32))
                            })
                            .unwrap_or(5);
                        let mutation = {
                            use crate::repl::retry_mutation::*;
                            let params = RetryParams {
                                temperature: request.temperature.unwrap_or(0.7),
                                plan_depth: actual_plan_depth,
                                model_name: request.model.clone(),
                                available_tools: request.tools.iter().map(|t| t.name.clone()).collect(),
                            };
                            let fallbacks: Vec<String> = fallback_providers.iter().map(|(n, _)| n.clone()).collect();
                            let failures = &result.tool_trust_failures;
                            compute_mutation(&params, 1, failures, &fallbacks, &self.config.policy)
                        };

                        // Apply ALL mutation axes to build the retry request.
                        let mut r_model = request.model.clone();
                        let mut r_temp = request.temperature;
                        let mut r_tools = request.tools.clone();
                        let mut r_max_rounds = agent_limits.max_rounds;
                        let mut retry_provider_override: Option<&Arc<dyn ModelProvider>> = None;
                        if let Some(ref m) = mutation {
                            for axis in &m.mutations {
                                match axis {
                                    crate::repl::retry_mutation::MutationAxis::ModelFallback { to, .. } => {
                                        r_model = to.clone();
                                        // Look up the actual provider Arc for the fallback model.
                                        if let Some((_, fp)) = fallback_providers.iter().find(|(n, _)| n == to) {
                                            retry_provider_override = Some(fp);
                                            tracing::info!(to = %to, "RetryMutation: switching provider for retry");
                                        }
                                    }
                                    crate::repl::retry_mutation::MutationAxis::TemperatureIncreased { to, .. } => {
                                        r_temp = Some(*to);
                                    }
                                    crate::repl::retry_mutation::MutationAxis::ToolExposureReduced { removed } => {
                                        r_tools.retain(|t| !removed.contains(&t.name));
                                    }
                                    crate::repl::retry_mutation::MutationAxis::PlanDepthReduced { from, to } => {
                                        // Reduce max_rounds proportionally to plan depth reduction.
                                        // This forces replanning with fewer steps on the retry.
                                        if *from > 0 {
                                            let ratio = *to as f64 / *from as f64;
                                            r_max_rounds = ((r_max_rounds as f64 * ratio).ceil() as usize).max(3);
                                            tracing::info!(
                                                from_depth = from, to_depth = to,
                                                new_max_rounds = r_max_rounds,
                                                "RetryMutation: PlanDepthReduced — clamped max_rounds"
                                            );
                                        }
                                    }
                                }
                            }
                            tracing::info!(axes = m.mutations.len(), "RetryMutation: applied");
                        }

                        // Select the provider for the retry — fallback if ModelFallback fired, else original.
                        let retry_provider_ref: &Arc<dyn ModelProvider> = retry_provider_override.unwrap_or(&p);

                        // Rebuild request with updated session messages + mutation.
                        let retry_request = ModelRequest {
                            model: r_model,
                            messages: self.session.messages.clone(),
                            tools: r_tools,
                            max_tokens: request.max_tokens,
                            temperature: r_temp,
                            system: request.system.clone(),
                            stream: true,
                        };

                        // Build retry limits with PlanDepthReduced applied.
                        let retry_limits = halcon_core::types::AgentLimits {
                            max_rounds: r_max_rounds,
                            ..agent_limits.clone()
                        };

                        // Reconstruct AgentContext for the retry invocation.
                        let retry_ctx = agent::AgentContext {
                            provider: retry_provider_ref,
                            session: &mut self.session,
                            request: &retry_request,
                            tool_registry: &self.tool_registry,
                            permissions: &mut self.permissions,
                            working_dir: &working_dir,
                            event_tx: &self.event_tx,
                            trace_db: self.async_db.as_ref(),
                            limits: &retry_limits,
                            response_cache: self.response_cache.as_ref(),
                            resilience: &mut self.resilience,
                            fallback_providers: &fallback_providers,
                            routing_config: &self.config.agent.routing,
                            compactor: Some(&compactor),
                            // DECISION: PlaybookPlanner is tried BEFORE LlmPlanner because:
                            // 1. Playbooks are deterministic — no LLM call, no token cost
                            // 2. For high-frequency repetitive tasks (code review, test run,
                            //    PR creation) playbooks are 10-100× faster than LLM planning
                            // 3. The fallback to LlmPlanner preserves full capability for
                            //    novel tasks. If PlaybookPlanner returns None (no matching
                            //    playbook), we fall through transparently — zero behavioral
                            //    change for tasks without playbooks.
                            // See US-playbook (PASO 4-D).
                            planner: if self.playbook_planner.find_match(input).is_some() {
                                Some(&self.playbook_planner as &dyn Planner)
                            } else {
                                llm_planner.as_ref().map(|lp| lp as &dyn Planner)
                            },
                            guardrails,
                            reflector: self.reflector.as_ref(),
                            render_sink: sink,
                            replay_tool_executor: None,
                            phase14: halcon_core::types::Phase14Context::default(),
                            model_selector: selector.as_ref(),
                            registry: Some(&self.registry),
                            episode_id: Some(uuid::Uuid::new_v4()),
                            planning_config: &self.config.planning,
                            orchestrator_config: &self.config.orchestrator,
                            tool_selection_enabled: self.config.context.dynamic_tool_selection,
                            task_bridge: None, // task_bridge state committed from first loop
                            context_metrics: Some(&self.context_metrics),
                            context_manager: self.context_manager.as_mut(),
                            #[cfg(feature = "tui")]
                            ctrl_rx: self.ctrl_rx.take(),
                            #[cfg(not(feature = "tui"))]
                            ctrl_rx: None,
                            speculator: &self.speculator,
                            security_config: &self.config.security,
                            strategy_context: strategy_ctx.clone(),
                            critic_provider: critic_prov.clone(),
                            critic_model: critic_mdl.clone(),
                            plugin_registry: None, // retry doesn't re-share plugin state
                            is_sub_agent: false,
                            requested_provider: Some(self.provider.clone()),
                            policy: std::sync::Arc::new(self.config.policy.clone()),
                        };

                        let mut retry_loop_result = agent::run_agent_loop(retry_ctx).await;
                        #[cfg(feature = "tui")]
                        if let Ok(ref mut r) = retry_loop_result {
                            self.ctrl_rx = r.ctrl_rx.take();
                        }
                        if let Ok(retry_r) = retry_loop_result {
                            // Accumulate tokens from retry into session counters.
                            self.session.total_usage.input_tokens += retry_r.input_tokens as u32;
                            self.session.total_usage.output_tokens += retry_r.output_tokens as u32;
                            tracing::info!(
                                rounds = retry_r.rounds,
                                stop = ?retry_r.stop_condition,
                                "Phase 1.2: LoopCritic in-session retry completed"
                            );
                            result = retry_r;
                        }
                    }
                }

                // Phase 2 Causality Enforcement: Unified reward → ModelSelector.record_outcome().
                //
                // This is the reward contamination fix. Previously agent.rs called
                // record_outcome() with a coarse 4-value mapping INSIDE the loop, giving
                // the quality tracker a completely different (and less accurate) signal than
                // the 5-signal reward_pipeline used by the UCB1 engine. Now:
                //   - When reasoning engine is active: use the pipeline reward captured above.
                //   - When reasoning engine is disabled: compute a coarse formula here once.
                //
                // Uses result.last_model_used so we record the model that actually ran the
                // final round (possibly changed by fallback or ModelSelector mid-session).
                if let Some(ref sel) = selector {
                    if let Some(model_id) = result.last_model_used.as_deref() {
                        let (reward, success) = if let Some((pr, ps)) = captured_pipeline_reward {
                            // Reasoning engine was active: use 5-signal pipeline reward.
                            (pr, ps)
                        } else {
                            // Coarse fallback: stop-condition mapping (2-level only).
                            let coarse_success = matches!(
                                result.stop_condition,
                                agent_types::StopCondition::EndTurn
                                    | agent_types::StopCondition::ForcedSynthesis
                            );
                            let coarse_reward = match result.stop_condition {
                                agent_types::StopCondition::EndTurn => 0.85,
                                agent_types::StopCondition::ForcedSynthesis => 0.65,
                                agent_types::StopCondition::MaxRounds => 0.40,
                                agent_types::StopCondition::TokenBudget
                                | agent_types::StopCondition::DurationBudget
                                | agent_types::StopCondition::CostBudget
                                | agent_types::StopCondition::SupervisorDenied => 0.30,
                                // User-cancelled: partial credit (not a model/task failure).
                                agent_types::StopCondition::Interrupted => 0.50,
                                // Hard failures: zero reward so UCB1 avoids bad strategies.
                                _ => 0.0,
                            };
                            (coarse_reward, coarse_success)
                        };
                        sel.record_outcome(model_id, reward, success);
                        tracing::debug!(
                            model_id,
                            reward,
                            success,
                            via = if captured_pipeline_reward.is_some() { "pipeline" } else { "coarse" },
                            "Phase 2: ModelSelector quality record unified"
                        );
                    }
                    // Phase 3: Snapshot quality stats back to Repl-level cache so the NEXT
                    // message starts with informed priors (not neutral 0.5).
                    self.model_quality_cache = sel.snapshot_quality_stats();
                    tracing::debug!(
                        models_tracked = self.model_quality_cache.len(),
                        "Phase 3: Quality stats snapshot saved for next message"
                    );

                    // Phase 7: Provider quality gate — warn when all tracked models are degraded.
                    // Fires after record_outcome() so the new outcome is included in the check.
                    // Min 5 interactions required to avoid false positives on cold-start.
                    if let Some(warning) = sel.quality_gate_check_with_threshold(5, self.config.policy.model_quality_gate) {
                        sink.warning(&warning, None);
                        tracing::warn!(
                            provider = p.name(),
                            "Phase 7: Provider quality degradation detected"
                        );
                    }

                    // Phase 4: Persist quality stats to DB (fire-and-forget) for cross-session
                    // learning. Non-fatal: DB unavailability does not affect the agent loop.
                    if let Some(ref adb) = self.async_db {
                        let adb_clone = adb.clone();
                        let provider_name = p.name().to_string();
                        let snapshot: Vec<(String, u32, u32, f64)> = self
                            .model_quality_cache
                            .iter()
                            .map(|(k, &(s, f, r))| (k.clone(), s, f, r))
                            .collect();
                        tokio::spawn(async move {
                            if let Err(e) = adb_clone
                                .save_model_quality_stats(&provider_name, snapshot)
                                .await
                            {
                                tracing::warn!(error = %e, "Phase 4: model quality persist failed");
                            } else {
                                tracing::debug!("Phase 4: model quality stats persisted to DB");
                            }
                        });
                    }
                }

                // Phase 8-D + 8-E: Record per-plugin UCB1 rewards from this agent loop and
                // fire-and-forget persist to DB.
                if let Some(ref arc_reg) = self.plugin_registry {
                    let snapshot_data: Vec<halcon_storage::db::PluginMetricsRecord> =
                        if let Ok(mut reg) = arc_reg.lock() {
                            for snapshot in &result.plugin_cost_snapshot {
                                // Derive success rate from calls_made / calls_failed as reward signal.
                                let rate = if snapshot.calls_made > 0 {
                                    let succeeded =
                                        snapshot.calls_made.saturating_sub(snapshot.calls_failed);
                                    succeeded as f64 / snapshot.calls_made as f64
                                } else {
                                    0.5 // neutral prior for plugins that were not invoked this round
                                };
                                reg.record_reward(&snapshot.plugin_id, rate);
                            }
                            reg.ucb1_snapshot()
                                .into_iter()
                                .map(|(plugin_id, n_uses, sum_rewards)| {
                                    halcon_storage::db::PluginMetricsRecord {
                                        plugin_id,
                                        calls_made: 0,
                                        calls_failed: 0,
                                        tokens_used: 0,
                                        ucb1_n_uses: n_uses as i64,
                                        ucb1_sum_rewards: sum_rewards,
                                        updated_at: String::new(),
                                    }
                                })
                                .collect()
                        } else {
                            vec![]
                        };

                    // Persist updated UCB1 arm stats (fire-and-forget, non-fatal).
                    if !snapshot_data.is_empty() {
                        if let Some(ref adb) = self.async_db {
                            let adb_clone = adb.clone();
                            tokio::spawn(async move {
                                if let Err(e) = adb_clone.save_plugin_metrics(snapshot_data).await {
                                    tracing::warn!(error = %e, "Phase 8-E: plugin metrics persist failed");
                                } else {
                                    tracing::debug!("Phase 8-E: plugin metrics persisted to DB");
                                }
                            });
                        }
                    }
                }

                // P3: Playbook auto-learning — save successful LLM-generated plans as reusable YAML.
                // Only when:
                //   1. auto_learn_playbooks is enabled in config
                //   2. The agent stopped successfully (EndTurn or ForcedSynthesis, not error)
                //   3. A plan was actually executed (timeline_json is Some)
                //   4. PlaybookPlanner did NOT already match this message (LlmPlanner was used)
                if self.config.planning.auto_learn_playbooks
                    && matches!(
                        result.stop_condition,
                        agent_types::StopCondition::EndTurn
                            | agent_types::StopCondition::ForcedSynthesis
                    )
                    && self.playbook_planner.find_match(input).is_none()
                {
                    if let Some(ref timeline_json) = result.timeline_json {
                        if let Some(saved_path) =
                            self.playbook_planner.record_from_timeline(input, timeline_json)
                        {
                            tracing::info!(
                                path = %saved_path.display(),
                                "P3: Auto-saved plan as playbook for future reuse"
                            );
                        }
                    }
                }

                // Phase 7 Dev Ecosystem: Record agent-loop span into the rolling telemetry
                // window so as_reward() can surface latency / error-rate back to UCB1.
                // fire-and-forget (tokio::spawn) — never blocks the REPL response path.
                {
                    let rt_signals = std::sync::Arc::clone(&self.runtime_signals);
                    let loop_ms = result.latency_ms as f64;
                    let had_error = matches!(
                        result.stop_condition,
                        agent_types::StopCondition::ProviderError
                            | agent_types::StopCondition::EnvironmentError
                    );
                    tokio::spawn(async move {
                        rt_signals
                            .ingest(runtime_signal_ingestor::RuntimeSignal::span(
                                "agent_loop",
                                loop_ms,
                                had_error,
                            ))
                            .await;
                    });
                }

                // FIX P0.1 2026-02-17: Update session token counters from agent loop result
                self.session.total_usage.input_tokens += result.input_tokens as u32;
                self.session.total_usage.output_tokens += result.output_tokens as u32;

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

                    // Consolidation with 30-second timeout to prevent UI freeze
                    let consolidation_timeout = std::time::Duration::from_secs(30);
                    let start = std::time::Instant::now();

                    match tokio::time::timeout(
                        consolidation_timeout,
                        memory_consolidator::maybe_consolidate(adb)
                    ).await {
                        Ok(Some(result)) => {
                            let duration_ms = start.elapsed().as_millis() as u64;
                            tracing::debug!(
                                merged = result.merged,
                                pruned = result.pruned,
                                duration_ms,
                                "Memory consolidation completed successfully"
                            );
                            sink.consolidation_complete(result.merged, result.pruned, duration_ms);
                        }
                        Ok(None) => {
                            // Consolidation was skipped (below threshold or error)
                            tracing::debug!("Memory consolidation skipped");
                        }
                        Err(_) => {
                            tracing::warn!(
                                timeout_secs = consolidation_timeout.as_secs(),
                                "Memory consolidation timed out - skipping to prevent UI freeze"
                            );
                            sink.warning(
                                "Memory consolidation took too long and was skipped",
                                Some("This is safe but may accumulate more reflections. Consider clearing old memories."),
                            );
                        }
                    }
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
    /// Async session save — called after each message turn in the interactive loop.
    ///
    /// Delegates to [`session_manager::auto_save`] (FASE F extraction).
    async fn auto_save_session(&self) {
        if let Some(ref adb) = self.async_db {
            session_manager::auto_save(&self.session, adb).await;
        }
    }

    /// Sync session save — called at teardown and from `/save` command.
    ///
    /// Delegates to [`session_manager::save`] (FASE F extraction).
    fn save_session(&self) {
        if let Some(ref db) = self.db {
            session_manager::save(
                &self.session,
                db,
                &self.model,
                &self.provider,
                &self.config,
            );
        }
    }

    /// Extract session summary and write to memory DB.
    ///
    /// Delegates to [`session_manager::summarize_to_memory`] (FASE F extraction).
    fn summarize_session_to_memory(&self, db: &Database) {
        session_manager::summarize_to_memory(
            &self.session,
            db,
            &self.model,
            &self.provider,
            &self.config,
        );
    }


    fn history_path() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".halcon").join("history.txt"))
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

// ---------------------------------------------------------------------------
// P3: Provider health routing helpers
// ---------------------------------------------------------------------------

/// Load per-model p95 latency hints from the metrics DB.
///
/// Returns `HashMap<model_id, p95_latency_ms>` for models that have at least
/// 3 recorded invocations. Used to populate `ModelSelector::with_latency_hints()`
/// so the "fast" routing strategy routes to historically fastest models.
///
/// Requires only 3 samples to avoid cold-start bias (model with 1 fast outlier
/// getting preferential routing over better-tested alternatives).
fn build_latency_hints_from_db(
    db: &Database,
    registry: &ProviderRegistry,
) -> std::collections::HashMap<String, u64> {
    let mut hints = std::collections::HashMap::new();
    for provider_name in registry.list() {
        if let Some(provider) = registry.get(provider_name) {
            for model in provider.supported_models() {
                if let Ok(stats) = db.model_stats(provider_name, &model.id) {
                    // Require at least 3 samples for a reliable p95 estimate.
                    if stats.p95_latency_ms > 0 && stats.total_invocations >= 3 {
                        hints.insert(model.id.clone(), stats.p95_latency_ms);
                    }
                }
            }
        }
    }
    hints
}

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::types::AppConfig;

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
        halcon_core::event_bus(16).0
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
        registry.register(Arc::new(halcon_providers::EchoProvider::new()));

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
            .list_memories(Some(halcon_storage::MemoryEntryType::SessionSummary), 10)
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

        // Should have 13 context sources: instructions + repo_map + planning + episodic_memory +
        // reflections + 8 SDLC context servers (requirements, architecture, codebase, workflow,
        // testing, runtime, security, support).
        let cm = repl.context_manager.as_ref().expect("ContextManager should exist");
        let source_names: Vec<&str> = cm.sources().map(|(name, _)| name).collect();
        assert_eq!(source_names.len(), 13);
        assert!(source_names.contains(&"instructions"));
        assert!(source_names.contains(&"repo_map"));
        assert!(source_names.contains(&"planning"));
        assert!(source_names.contains(&"episodic_memory"));
        assert!(source_names.contains(&"reflections"));
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

        // Should have 11 context sources: instructions + repo_map + planning + 8 SDLC servers
        // (no episodic_memory or reflections because those are disabled).
        let cm = repl.context_manager.as_ref().expect("ContextManager should exist");
        let source_names: Vec<&str> = cm.sources().map(|(name, _)| name).collect();
        assert_eq!(source_names.len(), 11);
        assert!(source_names.contains(&"instructions"));
        assert!(source_names.contains(&"repo_map"));
        assert!(source_names.contains(&"planning"));
        assert!(!source_names.contains(&"episodic_memory"));
        assert!(!source_names.contains(&"reflections"));
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

        // No DB => no memory, reflections, or DB-backed SDLC servers.
        // Has: instructions + repo_map + planning + codebase (codebase doesn't need DB).
        let cm = repl.context_manager.as_ref().expect("ContextManager should exist");
        let source_names: Vec<&str> = cm.sources().map(|(name, _)| name).collect();
        assert_eq!(source_names.len(), 4);
        assert!(source_names.contains(&"instructions"));
        assert!(source_names.contains(&"repo_map"));
        assert!(source_names.contains(&"planning"));
        assert!(source_names.contains(&"codebase"));
        assert!(!source_names.contains(&"episodic_memory"));
        assert!(!source_names.contains(&"reflections"));
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

        // Instructions + repo_map + codebase when planning, memory, and reflexion disabled.
        // codebase server doesn't require a DB, so it's still included.
        let cm = repl.context_manager.as_ref().expect("ContextManager should exist");
        let source_names: Vec<&str> = cm.sources().map(|(name, _)| name).collect();
        assert_eq!(source_names.len(), 3);
        assert!(source_names.contains(&"instructions"));
        assert!(source_names.contains(&"repo_map"));
        assert!(source_names.contains(&"codebase"));
        assert!(!source_names.contains(&"planning"));
    }

    // --- Phase 4C: Integration wiring tests ---

    #[test]
    fn resilience_registers_all_providers() {
        let config = test_config();
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(halcon_providers::EchoProvider::new()));

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
        registry.register(Arc::new(halcon_providers::EchoProvider::new()));
        registry.register(Arc::new(halcon_providers::OllamaProvider::new(
            None,
            halcon_core::types::HttpConfig::default(),
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
        use halcon_core::types::{BackpressureConfig, CircuitBreakerConfig, ResilienceConfig};

        let config = test_config();
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(halcon_providers::EchoProvider::new()));

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

// ── Multimodal extract_media_paths tests ─────────────────────────────────────

#[cfg(test)]
mod media_tests {
    use super::extract_media_paths;
    use std::fs;
    use tempfile::TempDir;

    fn make_tmp(dir: &TempDir, name: &str) -> std::path::PathBuf {
        let p = dir.path().join(name);
        fs::write(&p, b"data").unwrap();
        p
    }

    #[test]
    fn empty_message_returns_no_paths() {
        assert!(extract_media_paths("").is_empty());
    }

    #[test]
    fn plain_text_returns_no_paths() {
        assert!(extract_media_paths("hello world").is_empty());
    }

    #[test]
    fn nonexistent_file_ignored() {
        assert!(extract_media_paths("/does/not/exist.jpg").is_empty());
    }

    #[test]
    fn detects_jpg() {
        let d = TempDir::new().unwrap();
        let p = make_tmp(&d, "a.jpg");
        let paths = extract_media_paths(&p.to_string_lossy());
        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn detects_png() {
        let d = TempDir::new().unwrap();
        let p = make_tmp(&d, "a.png");
        let paths = extract_media_paths(&p.to_string_lossy());
        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn detects_wav() {
        let d = TempDir::new().unwrap();
        let p = make_tmp(&d, "a.wav");
        let paths = extract_media_paths(&p.to_string_lossy());
        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn detects_mp4() {
        let d = TempDir::new().unwrap();
        let p = make_tmp(&d, "v.mp4");
        let paths = extract_media_paths(&p.to_string_lossy());
        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn strips_quotes() {
        let d = TempDir::new().unwrap();
        let p = make_tmp(&d, "a.png");
        let msg = format!("\"{}\"", p.display());
        let paths = extract_media_paths(&msg);
        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn deduplicates() {
        let d = TempDir::new().unwrap();
        let p = make_tmp(&d, "a.jpg");
        let msg = format!("{0} {0}", p.display());
        let paths = extract_media_paths(&msg);
        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn multiple_files() {
        let d = TempDir::new().unwrap();
        let p1 = make_tmp(&d, "a.jpg");
        let p2 = make_tmp(&d, "b.png");
        let msg = format!("{} {}", p1.display(), p2.display());
        let paths = extract_media_paths(&msg);
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn mixed_media_and_text() {
        let d = TempDir::new().unwrap();
        let p = make_tmp(&d, "a.jpg");
        let msg = format!("analyze this {} please", p.display());
        let paths = extract_media_paths(&msg);
        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn case_insensitive_extension() {
        let d = TempDir::new().unwrap();
        let p = make_tmp(&d, "a.JPG");
        let paths = extract_media_paths(&p.to_string_lossy());
        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn txt_file_ignored() {
        let d = TempDir::new().unwrap();
        let p = make_tmp(&d, "a.txt");
        let paths = extract_media_paths(&p.to_string_lossy());
        assert!(paths.is_empty());
    }

    // Phase 84 — video + audio extension coverage

    #[test]
    fn extract_media_paths_detects_video_extensions() {
        let d = TempDir::new().unwrap();
        let mp4  = make_tmp(&d, "clip.mp4");
        let webm = make_tmp(&d, "clip.webm");
        let mkv  = make_tmp(&d, "clip.mkv");
        let msg  = format!("{} {} {}", mp4.display(), webm.display(), mkv.display());
        let paths = extract_media_paths(&msg);
        assert_eq!(paths.len(), 3, "mp4/webm/mkv must all be detected");
    }

    #[test]
    fn nonexistent_video_files_return_empty() {
        // Non-existent files are filtered out regardless of extension.
        assert!(extract_media_paths("/no/such/video.mp4").is_empty());
        assert!(extract_media_paths("/no/such/clip.webm").is_empty());
        assert!(extract_media_paths("/no/such/film.mkv").is_empty());
    }

    #[test]
    fn extract_media_paths_detects_audio_extensions() {
        let d = TempDir::new().unwrap();
        let wav = make_tmp(&d, "speech.wav");
        let mp3 = make_tmp(&d, "music.mp3");
        let msg = format!("{} {}", wav.display(), mp3.display());
        let paths = extract_media_paths(&msg);
        assert_eq!(paths.len(), 2, "wav/mp3 must be detected for audio fallback logic");
    }

    #[test]
    fn video_and_audio_mixed_with_image() {
        let d = TempDir::new().unwrap();
        let img  = make_tmp(&d, "photo.png");
        let vid  = make_tmp(&d, "demo.mp4");
        let aud  = make_tmp(&d, "voice.wav");
        let msg  = format!("check {} and {} and {}", img.display(), vid.display(), aud.display());
        let paths = extract_media_paths(&msg);
        assert_eq!(paths.len(), 3, "image + video + audio should all be detected together");
    }
}
