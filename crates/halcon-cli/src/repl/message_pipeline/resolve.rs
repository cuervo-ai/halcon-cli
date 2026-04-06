//! Stage 3: Provider Resolution — provider lookup, planner, model selection, plugin init, UCB1 seeding.
//!
//! # Xiyo Comparison
//!
//! Xiyo resolves the model in `submitMessage()` (QueryEngine.ts:274-276) with a
//! simple `parseUserSpecifiedModel()` call. Halcon's resolution is richer:
//! - **Fallback chain**: Multiple providers with priority ordering
//! - **PlaybookPlanner**: Zero-LLM-cost plan matching before falling back to LlmPlanner
//! - **ModelSelector**: UCB1-based quality routing with cross-session learning
//! - **Plugin system**: Lazy discovery and registration of plugin proxy tools
//! - **Reasoning engine**: Task analysis + strategy selection (SOTA 2026)
//!
//! # Side Effects
//!
//! - Database reads for quality stats, plugin metrics, UCB1 experience (all load-once)
//! - Plugin filesystem discovery (one-time)
//! - Plugin registry mutation (tool registration)

use std::sync::{Arc, Mutex};

use halcon_core::traits::ModelProvider;
use halcon_core::types::{AgentLimits, AppConfig};
use halcon_storage::AsyncDatabase;
use halcon_tools::ToolRegistry;

use crate::repl::agent;
use crate::repl::agent_types;
use crate::repl::cache_state::ReplCacheState;
use crate::repl::compaction;
use crate::repl::feature_bundle::ReplFeatures;
use crate::repl::model_selector;
use crate::repl::planner;
use crate::repl::plugins;
use crate::repl::runtime_control::RuntimeGuards;
use crate::repl::strategy_selector;
use crate::repl::task_analyzer;
use crate::repl::task_bridge;

/// Output of the Provider Resolution stage.
pub struct ResolveOutput<'a> {
    /// Fully assembled AgentContext ready for `run_agent_loop`.
    pub agent_context: agent::AgentContext<'a>,
    /// Resolved provider for the primary model.
    pub provider: Arc<dyn ModelProvider>,
    /// Fallback providers (name, Arc<Provider>) pairs.
    pub fallback_providers: Vec<(String, Arc<dyn ModelProvider>)>,
    /// Optional LLM planner (when adaptive planning enabled).
    pub llm_planner: Option<planner::LlmPlanner>,
    /// Optional model selector (when model selection enabled).
    pub selector: Option<model_selector::ModelSelector>,
    /// Compactor for context compression.
    pub compactor: compaction::ContextCompactor,
    /// Guardrails slice.
    pub guardrails: &'a [Box<dyn halcon_security::Guardrail>],
    /// Strategy context for UCB1 routing.
    pub strategy_context: Option<agent_types::StrategyContext>,
    /// Critic provider/model for post-loop evaluation.
    pub critic_provider: Option<Arc<dyn ModelProvider>>,
    pub critic_model: Option<String>,
    /// Reasoning analysis (for post-loop evaluation).
    pub reasoning_analysis: Option<crate::repl::reasoning_engine::PreLoopAnalysis>,
    /// Agent limits (possibly adjusted by reasoning engine).
    pub agent_limits: AgentLimits,
    /// Task bridge instance.
    pub task_bridge: Option<task_bridge::TaskBridge>,
}

/// Stage 3: Provider Resolution.
///
/// Resolves the provider, planner, model selector, initializes plugins,
/// loads cross-session UCB1 data, runs reasoning engine pre-loop analysis,
/// and assembles the full AgentContext.
pub struct ResolveStage;

impl ResolveStage {
    /// Loads cross-session model quality stats from DB (one-time per session).
    pub async fn load_model_quality(
        guards: &mut RuntimeGuards,
        async_db: &Option<AsyncDatabase>,
        provider: &dyn ModelProvider,
        cache: &mut ReplCacheState,
    ) {
        if guards.model_quality_db_loaded {
            return;
        }
        guards.model_quality_db_loaded = true;
        if let Some(ref adb) = async_db {
            match adb.load_model_quality_stats(provider.name()).await {
                Ok(prior_stats) if !prior_stats.is_empty() => {
                    for (model_id, success, failure, reward) in prior_stats {
                        let cached = cache
                            .model_quality
                            .entry(model_id)
                            .or_insert((0u32, 0u32, 0.0f64));
                        if success > cached.0 {
                            *cached = (success, failure, reward);
                        }
                    }
                    tracing::info!(
                        models = cache.model_quality.len(),
                        provider = provider.name(),
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

    /// Initializes the plugin system (one-time per session).
    ///
    /// Discovers *.plugin.toml manifests and registers PluginProxyTool instances.
    pub fn init_plugins(
        features: &mut ReplFeatures,
        tool_registry: &mut ToolRegistry,
        config: &AppConfig,
    ) {
        let plugins_should_run = config.plugins.enabled || {
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

        if features.plugin_registry.is_some() || !plugins_should_run {
            return;
        }

        let loader = if let Some(ref dir) = config.plugins.plugin_dir {
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
                "Phase 8-A: Plugin system initialised"
            );
            let runtime_arc = Arc::new(runtime);

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
                    tool_registry.register(Arc::new(proxy));
                    proxy_count += 1;
                }
            }
            tracing::info!(
                proxy_tools = proxy_count,
                "Phase 8-A: Plugin proxy tools registered in ToolRegistry"
            );

            features.plugin_transport_runtime = Some(runtime_arc);
            features.plugin_registry = Some(Arc::new(Mutex::new(registry)));
        } else {
            tracing::debug!(
                skipped_invalid = load_result.skipped_invalid,
                "Phase 8-A: No plugins loaded"
            );
        }
    }

    /// Loads plugin UCB1 metrics from DB (one-time per session).
    pub async fn load_plugin_metrics(
        guards: &mut RuntimeGuards,
        async_db: &Option<AsyncDatabase>,
        plugin_registry: &Option<Arc<Mutex<plugins::PluginRegistry>>>,
    ) {
        if guards.plugin_metrics_db_loaded {
            return;
        }
        guards.plugin_metrics_db_loaded = true;
        if let (Some(ref adb), Some(ref arc_reg)) = (async_db, plugin_registry) {
            match adb.load_plugin_metrics().await {
                Ok(records) if !records.is_empty() => {
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
                Ok(_) => tracing::debug!("Phase 8-E: no prior plugin metrics in DB"),
                Err(e) => {
                    tracing::warn!(error = %e, "Phase 8-E: failed to load plugin metrics from DB")
                }
            }
        }
    }

    /// Loads cross-session UCB1 reasoning experience (one-time per session).
    pub async fn load_reasoning_experience(
        features: &mut ReplFeatures,
        async_db: &Option<AsyncDatabase>,
    ) {
        let engine = match features.reasoning_engine.as_mut() {
            Some(e) if !e.is_experience_loaded() => e,
            _ => return,
        };

        fn pascal_to_snake(s: &str) -> String {
            let mut out = String::with_capacity(s.len() + 4);
            for (i, c) in s.chars().enumerate() {
                if c.is_uppercase() && i > 0 {
                    out.push('_');
                }
                out.extend(c.to_lowercase());
            }
            out
        }

        if let Some(ref adb) = async_db {
            match adb.load_all_reasoning_experiences().await {
                Ok(exps) => {
                    let parsed: Vec<_> = exps
                        .iter()
                        .filter_map(|e| {
                            let tt =
                                task_analyzer::TaskType::from_str(&pascal_to_snake(&e.task_type))?;
                            let st = strategy_selector::ReasoningStrategy::from_str(
                                &pascal_to_snake(&e.strategy),
                            )?;
                            Some((tt, st, e.avg_score, e.uses))
                        })
                        .collect();
                    let count = parsed.len();
                    engine.load_experience(parsed);
                    tracing::info!(entries = count, "UCB1: cross-session experience loaded");
                }
                Err(e) => {
                    tracing::warn!("UCB1 load_experience failed: {e}");
                    engine.mark_experience_loaded();
                }
            }
        } else {
            engine.mark_experience_loaded();
        }
    }
}
