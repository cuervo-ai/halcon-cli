//! Slash command handler methods extracted from the Repl struct.

use std::io::Write;
use std::sync::Arc;

use futures::StreamExt;

use cuervo_core::traits::Planner;
use cuervo_core::types::{
    ChatMessage, MessageContent, ModelChunk, ModelRequest, Role, Session,
};

use super::{commands, console, orchestrator, planner, replay_runner, Repl};

impl Repl {
    pub(crate) fn list_sessions(&self) {
        let Some(db) = &self.db else {
            println!("(No database configured)");
            return;
        };
        match db.list_sessions(10) {
            Ok(sessions) if sessions.is_empty() => {
                println!("No saved sessions.");
            }
            Ok(sessions) => {
                println!("Recent sessions:");
                for s in &sessions {
                    let id_short = &s.id.to_string()[..8];
                    let title = s.title.as_deref().unwrap_or("(untitled)");
                    let msgs = s.messages.len();
                    let date = s.updated_at.format("%Y-%m-%d %H:%M");
                    println!("  {id_short}  {title:<30} {msgs} msgs  {date}");
                }
            }
            Err(e) => {
                crate::render::feedback::user_error(
                    &format!("failed to list sessions — {e}"),
                    None,
                );
            }
        }
    }

    pub(crate) fn show_session(&self) {
        let id_short = &self.session.id.to_string()[..8];
        println!("Session:  {id_short}");
        println!("Model:    {}/{}", self.provider, self.model);
        println!("Messages: {}", self.session.messages.len());
        println!(
            "Tokens:   {} in / {} out",
            self.session.total_usage.input_tokens, self.session.total_usage.output_tokens,
        );
        println!("Tools:    {} invocations", self.session.tool_invocations);
        println!("Rounds:   {}", self.session.agent_rounds);
        println!("Latency:  {}ms total", self.session.total_latency_ms);
        if self.session.estimated_cost_usd > 0.0 {
            println!("Cost:     ${:.6}", self.session.estimated_cost_usd);
        }
        println!(
            "Started:  {}",
            self.session.created_at.format("%Y-%m-%d %H:%M:%S")
        );
    }

    pub(crate) fn show_trace_info(&self) {
        let id_short = &self.session.id.to_string()[..8];
        println!("--- Trace Context ---");
        println!("Session:    {id_short}");
        println!("Messages:   {}", self.session.messages.len());
        println!("Rounds:     {}", self.session.agent_rounds);
        if let Some(ref fp) = self.session.execution_fingerprint {
            println!("Fingerprint: {}", &fp[..16.min(fp.len())]);
        } else {
            println!("Fingerprint: (none)");
        }
    }

    pub(crate) fn show_state_info(&self) {
        let id_short = &self.session.id.to_string()[..8];
        println!("--- Agent State ---");
        println!("Session:   {id_short}");
        println!("Rounds:    {}", self.session.agent_rounds);
        println!("Tools:     {} invocations", self.session.tool_invocations);
        let state = if self.session.messages.is_empty() {
            "Idle"
        } else {
            "Complete"
        };
        println!("State:     {state}");
    }

    pub(crate) async fn run_test(&self, kind: &commands::TestKind) {
        use commands::TestKind;
        match kind {
            TestKind::Status => {
                println!("--- cuervo diagnostics ---");
                println!("Version:  {}", env!("CARGO_PKG_VERSION"));
                println!("Provider: {}", self.provider);
                println!("Model:    {}", self.model);
                println!(
                    "Database: {}",
                    if self.db.is_some() {
                        "connected"
                    } else {
                        "not configured"
                    }
                );
                println!("Session:  {}", &self.session.id.to_string()[..8]);
                println!("Messages: {}", self.session.messages.len());

                // Check registered providers.
                let providers = self.registry.list();
                println!("Registered providers: {}", providers.join(", "));

                // Check if current provider is available.
                match self.registry.get(&self.provider) {
                    Some(p) => {
                        let available = p.is_available().await;
                        println!(
                            "Provider '{}': {}",
                            self.provider,
                            if available {
                                "available"
                            } else {
                                "not available (missing API key?)"
                            }
                        );
                    }
                    None => {
                        println!("Provider '{}': not registered", self.provider);
                    }
                }
                println!("--- end diagnostics ---");
            }
            TestKind::Provider(name) => {
                print!("Testing provider '{name}'... ");
                match self.registry.get(name) {
                    None => {
                        println!("NOT FOUND");
                        println!("  Provider '{name}' is not registered.");
                        let available = self.registry.list();
                        println!("  Available: {}", available.join(", "));
                    }
                    Some(p) => {
                        // 1. Availability check.
                        let available = p.is_available().await;
                        if !available {
                            println!("NOT AVAILABLE");
                            println!(
                                "  Provider is registered but not reachable (missing API key?)."
                            );
                            return;
                        }

                        // 2. Send a tiny probe request.
                        let probe = ModelRequest {
                            model: if name == "echo" {
                                "echo".to_string()
                            } else {
                                self.model.clone()
                            },
                            messages: vec![ChatMessage {
                                role: Role::User,
                                content: MessageContent::Text("Say OK".to_string()),
                            }],
                            tools: vec![],
                            max_tokens: Some(16),
                            temperature: Some(0.0),
                            system: None,
                            stream: true,
                        };

                        let start = std::time::Instant::now();
                        match p.invoke(&probe).await {
                            Ok(mut stream) => {
                                let mut text = String::new();
                                let mut tokens = 0u32;
                                while let Some(chunk) = stream.next().await {
                                    match chunk {
                                        Ok(ModelChunk::TextDelta(t)) => text.push_str(&t),
                                        Ok(ModelChunk::Usage(u)) => {
                                            tokens += u.input_tokens + u.output_tokens;
                                        }
                                        Ok(ModelChunk::Done(_)) => break,
                                        Ok(ModelChunk::Error(e)) => {
                                            println!("ERROR");
                                            println!("  Stream error: {e}");
                                            return;
                                        }
                                        Err(e) => {
                                            println!("ERROR");
                                            println!("  {e}");
                                            return;
                                        }
                                        _ => {}
                                    }
                                }
                                let elapsed = start.elapsed();
                                println!("OK ({:.0}ms)", elapsed.as_millis());
                                let preview: String = text.chars().take(60).collect();
                                println!("  Response: {preview}");
                                println!("  Tokens: {tokens}");
                                println!("  Latency: {:.0}ms", elapsed.as_millis());
                            }
                            Err(e) => {
                                let elapsed = start.elapsed();
                                println!("FAILED ({:.0}ms)", elapsed.as_millis());
                                println!("  Error: {e}");
                            }
                        }
                    }
                }
            }
        }
    }

    pub(crate) async fn run_orchestrate(&mut self, instruction: &str) {
        if !self.config.orchestrator.enabled {
            crate::render::feedback::user_error(
                "orchestrator is disabled",
                Some("Set [orchestrator] enabled = true in config.toml"),
            );
            return;
        }

        let provider = match self.registry.get(&self.provider).cloned() {
            Some(p) => p,
            None => {
                crate::render::feedback::user_error(
                    &format!("provider '{}' not available", self.provider),
                    Some("Configure a provider before using /orchestrate"),
                );
                return;
            }
        };

        // Create a single SubAgentTask from the instruction.
        // (When a planner is available, it would decompose into multiple tasks.)
        use cuervo_core::types::{AgentType, SubAgentTask};
        use std::collections::HashSet;

        let tasks = vec![SubAgentTask {
            task_id: uuid::Uuid::new_v4(),
            instruction: instruction.to_string(),
            agent_type: AgentType::Chat,
            model: None,
            provider: None,
            allowed_tools: HashSet::new(),
            limits_override: None,
            depends_on: vec![],
            priority: 0,
        }];

        let orchestrator_id = uuid::Uuid::new_v4();
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

        let fallback_providers: Vec<(String, Arc<dyn cuervo_core::traits::ModelProvider>)> = self
            .config
            .agent
            .routing
            .fallback_models
            .iter()
            .filter_map(|name| {
                self.registry.get(name).cloned().map(|p| (name.clone(), p))
            })
            .collect();

        let guardrails: &[Box<dyn cuervo_security::Guardrail>] =
            if self.config.security.guardrails.enabled && self.config.security.guardrails.builtins {
                cuervo_security::builtin_guardrails()
            } else {
                &[]
            };

        eprintln!("Orchestrating: {}", instruction);
        let start = std::time::Instant::now();

        match orchestrator::run_orchestrator(
            orchestrator_id,
            tasks,
            &provider,
            &self.tool_registry,
            &self.event_tx,
            &self.config.agent.limits,
            &self.config.orchestrator,
            &self.config.agent.routing,
            self.async_db.as_ref(),
            self.response_cache.as_ref(),
            &fallback_providers,
            &self.model,
            &working_dir,
            None,
            guardrails,
            self.config.tools.confirm_destructive,
            self.config.security.tbac_enabled,
        )
        .await
        {
            Ok(result) => {
                let elapsed = start.elapsed();
                eprintln!(
                    "\nOrchestration complete: {}/{} tasks succeeded | {:.1}s | ${:.4}",
                    result.success_count,
                    result.total_count,
                    elapsed.as_secs_f64(),
                    result.total_cost_usd,
                );
                // Print per-task summaries.
                for sub in &result.sub_results {
                    let status = if sub.success { "OK" } else { "FAIL" };
                    let preview: String = sub.output_text.chars().take(80).collect();
                    eprintln!(
                        "  [{}] {} ({} rounds, {}ms) {}",
                        status,
                        &sub.task_id.to_string()[..8],
                        sub.rounds,
                        sub.latency_ms,
                        if preview.is_empty() {
                            sub.error.as_deref().unwrap_or("").to_string()
                        } else {
                            preview
                        },
                    );
                }
            }
            Err(e) => {
                crate::render::feedback::user_error(
                    &format!("orchestration failed — {e}"),
                    None,
                );
            }
        }
    }

    // --- Phase 19: Agent Operating Console handlers ---

    pub(crate) async fn handle_live_status(&self) {
        let session_short = &self.session.id.to_string()[..8];
        let tokens = self.session.total_usage.input_tokens as u64
            + self.session.total_usage.output_tokens as u64;

        let diagnostics: Vec<(String, String, usize)> = self
            .resilience
            .diagnostics()
            .iter()
            .map(|d| {
                let state = format!("{:?}", d.breaker_state).to_lowercase();
                (d.provider.clone(), state, d.failure_count)
            })
            .collect();

        let models: Vec<String> = self.registry.list().iter().map(|s| s.to_string()).collect();

        let mut out = std::io::stderr().lock();
        let info = console::StatusInfo {
            session_id: session_short,
            rounds: self.session.agent_rounds,
            tokens,
            cost: self.session.estimated_cost_usd,
            provider: &self.provider,
            model: &self.model,
            provider_diagnostics: &diagnostics,
            registered_models: &models,
        };
        console::render_status(&info, &mut out);
    }

    pub(crate) async fn handle_metrics(&self) {
        let mut out = std::io::stderr().lock();

        let Some(db) = &self.db else {
            crate::render::feedback::user_warning("no database configured", None);
            return;
        };

        let sys = match db.system_metrics() {
            Ok(s) => s,
            Err(e) => {
                crate::render::feedback::user_error(&format!("failed to load metrics: {e}"), None);
                return;
            }
        };

        let cache = db.cache_stats().ok();
        let memory = db.memory_stats().ok();

        console::render_metrics(&sys, cache.as_ref(), memory.as_ref(), &mut out);
    }

    pub(crate) async fn handle_logs(&self, filter: Option<&str>) {
        let mut out = std::io::stderr().lock();

        let Some(db) = &self.db else {
            crate::render::feedback::user_warning("no database configured", None);
            return;
        };

        let steps = match db.load_trace_steps(self.session.id) {
            Ok(s) => s,
            Err(e) => {
                crate::render::feedback::user_error(&format!("failed to load logs: {e}"), None);
                return;
            }
        };

        console::render_logs(&steps, filter, &mut out);
    }

    pub(crate) async fn handle_inspect(&self, target: &commands::InspectTarget) {
        let mut out = std::io::stderr().lock();

        match target {
            commands::InspectTarget::Runtime => {
                console::inspect_runtime(&mut out);
            }
            commands::InspectTarget::Memory => {
                let (stats, episodes) = if let Some(db) = &self.db {
                    let ms = db.memory_stats().ok();
                    let ep = db.count_episodes().unwrap_or(0);
                    (ms, ep)
                } else {
                    (None, 0)
                };
                console::inspect_memory(stats.as_ref(), episodes, &mut out);
            }
            commands::InspectTarget::Db => {
                let info = self.db.as_ref().and_then(|db| {
                    let conn = db.conn().ok()?;
                    let mut pairs = Vec::new();
                    for pragma in &["page_count", "page_size", "journal_mode"] {
                        let val = conn
                            .query_row(&format!("PRAGMA {pragma}"), [], |row| {
                                // Try integer first (page_count, page_size), then string (journal_mode).
                                row.get::<_, i64>(0)
                                    .map(|v| v.to_string())
                                    .or_else(|_| row.get::<_, String>(0))
                            })
                            .unwrap_or_else(|_| "unknown".into());
                        pairs.push((pragma.to_string(), val));
                    }
                    Some(pairs)
                });
                console::inspect_db(info.as_deref(), &mut out);
            }
            commands::InspectTarget::Traces => {
                let traces = if let Some(db) = &self.db {
                    let sessions = db.list_sessions(10).unwrap_or_default();
                    sessions
                        .iter()
                        .filter_map(|s| {
                            let steps = db.load_trace_steps(s.id).ok()?;
                            let id_short = s.id.to_string()[..8].to_string();
                            let date = s.updated_at.format("%Y-%m-%d %H:%M").to_string();
                            Some((id_short, steps.len(), date))
                        })
                        .collect()
                } else {
                    vec![]
                };
                console::inspect_traces(&traces, &mut out);
            }
            commands::InspectTarget::Context => {
                let _ = writeln!(out, "--- Context Pipeline ---");
                let _ = writeln!(out, "Dynamic tool selection: {}", self.config.context.dynamic_tool_selection);
                let _ = writeln!(out, "Max context tokens:    {}", self.config.general.max_tokens);
                let _ = writeln!(out, "Context sources:       {}", self.context_sources.len());
                for src in &self.context_sources {
                    let _ = writeln!(out, "  - {} (priority: {})", src.name(), src.priority());
                }
                let _ = writeln!(out, "Governance:");
                let _ = writeln!(out, "  Max tokens/source: {}", self.config.context.governance.default_max_tokens_per_source);
                let _ = writeln!(out, "  TTL (secs):        {}", self.config.context.governance.default_ttl_secs);
                let snap = self.context_metrics.snapshot();
                let _ = writeln!(out, "Context Metrics:");
                let _ = writeln!(out, "  Assemblies:      {} | Total tokens: {:.1}K",
                    snap.assemblies,
                    snap.total_tokens_assembled as f64 / 1000.0);
                let _ = writeln!(out, "  Tools filtered:  {} | Governance truncations: {}",
                    snap.tools_filtered,
                    snap.governance_truncations);
            }
            commands::InspectTarget::Tasks => {
                let _ = writeln!(out, "--- Task Framework ---");
                if self.config.task_framework.enabled {
                    let _ = writeln!(out, "Status:      enabled");
                    let _ = writeln!(out, "Persist:     {}", self.config.task_framework.persist_tasks);
                    let _ = writeln!(out, "Max retries: {}", self.config.task_framework.default_max_retries);
                    if let Some(ref timeline) = self.last_timeline {
                        let _ = writeln!(out, "Last timeline: {} bytes", timeline.len());
                    } else {
                        let _ = writeln!(out, "Last timeline: (none)");
                    }
                } else {
                    let _ = writeln!(out, "Status: disabled");
                    let _ = writeln!(out, "  Enable with: --tasks flag or [task_framework] enabled = true in config.toml");
                }
            }
            commands::InspectTarget::Mcp => {
                let _ = writeln!(out, "--- MCP Servers ---");
                if self.config.mcp.servers.is_empty() {
                    let _ = writeln!(out, "No MCP servers configured.");
                    let _ = writeln!(out, "  Add servers in [mcp.servers] section of config.toml");
                } else {
                    let _ = writeln!(out, "Configured servers: {}", self.config.mcp.servers.len());
                    for (name, server) in &self.config.mcp.servers {
                        let _ = writeln!(out, "  - {name}: {}", server.command);
                    }
                }
                let tool_count = self.tool_registry.tool_definitions().len();
                let _ = writeln!(out, "Total registered tools: {tool_count}");
            }
            commands::InspectTarget::Reasoning => {
                let _ = writeln!(out, "--- Reasoning Engine ---");
                let _ = writeln!(out, "Reasoning engine has been removed (dead code).");
            }
            commands::InspectTarget::Orchestration => {
                let _ = writeln!(out, "--- Orchestration ---");
                let _ = writeln!(out, "Enabled:          {}", self.config.orchestrator.enabled);
                if self.config.orchestrator.enabled {
                    let _ = writeln!(out, "Max concurrent:   {}", self.config.orchestrator.max_concurrent_agents);
                    let _ = writeln!(out, "Sub-agent timeout: {}s", self.config.orchestrator.sub_agent_timeout_secs);
                    let _ = writeln!(out, "Shared budget:    {}", self.config.orchestrator.shared_budget);
                    let _ = writeln!(out, "Communication:    {}", self.config.orchestrator.enable_communication);
                    let _ = writeln!(out, "Min delegation confidence: {:.2}", self.config.orchestrator.min_delegation_confidence);
                }
            }
            commands::InspectTarget::Resilience => {
                let _ = writeln!(out, "--- Resilience ---");
                let _ = writeln!(out, "Enabled: {}", self.config.resilience.enabled);
                if self.config.resilience.enabled {
                    let diag = self.resilience.diagnostics();
                    let _ = writeln!(out, "Registered providers: {}", diag.len());
                    for d in &diag {
                        let _ = writeln!(out, "  - {} ({:?}, failures: {})", d.provider, d.breaker_state, d.failure_count);
                    }
                    let _ = writeln!(out, "Circuit breaker:");
                    let _ = writeln!(out, "  Failure threshold: {}", self.config.resilience.circuit_breaker.failure_threshold);
                    let _ = writeln!(out, "  Window:            {}s", self.config.resilience.circuit_breaker.window_secs);
                    let _ = writeln!(out, "  Open duration:     {}s", self.config.resilience.circuit_breaker.open_duration_secs);
                    let _ = writeln!(out, "Health scoring:");
                    let _ = writeln!(out, "  Window:    {} min", self.config.resilience.health.window_minutes);
                    let _ = writeln!(out, "  Degraded:  <= {}", self.config.resilience.health.degraded_threshold);
                    let _ = writeln!(out, "  Unhealthy: <= {}", self.config.resilience.health.unhealthy_threshold);
                    let _ = writeln!(out, "Backpressure:");
                    let _ = writeln!(out, "  Max concurrent/provider: {}", self.config.resilience.backpressure.max_concurrent_per_provider);
                    let _ = writeln!(out, "  Queue timeout:           {}s", self.config.resilience.backpressure.queue_timeout_secs);
                }
            }
            commands::InspectTarget::CostMetrics => {
                let _ = writeln!(out, "--- Cost Metrics ---");
                let _ = writeln!(out, "Session: {}", &self.session.id.to_string()[..8]);
                let _ = writeln!(out, "Provider: {}", self.provider);
                let _ = writeln!(out, "Model:    {}", self.model);
                let _ = writeln!(out, "Budget limits:");
                let _ = writeln!(out, "  Max rounds:      {}", self.config.agent.limits.max_rounds);
                let _ = writeln!(out, "  Max tool output: {} chars", self.config.agent.limits.max_tool_output_chars);
            }
        }
    }

    pub(crate) async fn handle_trace_browse(&self, session_id_str: Option<&str>) {
        let mut out = std::io::stderr().lock();

        let Some(db) = &self.db else {
            crate::render::feedback::user_warning("no database configured", None);
            return;
        };

        let session_id = if let Some(id_str) = session_id_str {
            match uuid::Uuid::parse_str(id_str) {
                Ok(id) => id,
                Err(_) => {
                    // Try prefix match against recent sessions.
                    let sessions = db.list_sessions(50).unwrap_or_default();
                    match sessions.iter().find(|s| s.id.to_string().starts_with(id_str)) {
                        Some(s) => s.id,
                        None => {
                            crate::render::feedback::user_error(
                                &format!("session not found: {id_str}"),
                                Some("Use /session list to see available sessions"),
                            );
                            return;
                        }
                    }
                }
            }
        } else {
            self.session.id
        };

        let steps = match db.load_trace_steps(session_id) {
            Ok(s) => s,
            Err(e) => {
                crate::render::feedback::user_error(&format!("failed to load trace: {e}"), None);
                return;
            }
        };

        console::browse_trace(&steps, &mut out);
    }

    pub(crate) async fn handle_plan(&mut self, goal: &str) {
        let provider = match self.registry.get(&self.provider).cloned() {
            Some(p) => p,
            None => {
                crate::render::feedback::user_error(
                    &format!("provider '{}' not available", self.provider),
                    None,
                );
                return;
            }
        };

        let planner = planner::LlmPlanner::new(provider, self.model.clone())
            .with_max_replans(self.config.planning.max_replans);

        let tool_defs = self.tool_registry.tool_definitions();
        eprintln!("Planning: {goal}");

        match planner.plan(goal, &tool_defs).await {
            Ok(Some(plan)) => {
                let plan_id_str = plan.plan_id.to_string()[..8].to_string();
                let steps: Vec<(String, Option<String>, f64)> = plan
                    .steps
                    .iter()
                    .map(|s| (s.description.clone(), s.tool_name.clone(), s.confidence))
                    .collect();

                let mut out = std::io::stderr().lock();
                console::render_plan(&plan_id_str, &plan.goal, &steps, &mut out);

                // Save plan to DB.
                if let Some(ref adb) = self.async_db {
                    if let Err(e) = adb.save_plan_steps(&self.session.id, &plan).await {
                        crate::render::feedback::user_warning(
                            &format!("failed to save plan: {e}"),
                            None,
                        );
                    } else {
                        let _ = writeln!(out, "\n    Plan saved. Run with: /run {plan_id_str}");
                    }
                }
            }
            Ok(None) => {
                crate::render::feedback::user_warning("planner returned no plan", None);
            }
            Err(e) => {
                crate::render::feedback::user_error(&format!("planning failed: {e}"), None);
            }
        }
    }

    pub(crate) async fn handle_run_plan(&mut self, plan_id_str: &str) {
        let Some(db) = &self.db else {
            crate::render::feedback::user_warning("no database configured", None);
            return;
        };

        // Load plan steps from DB.
        let plan_steps = match db.load_plan_steps(plan_id_str) {
            Ok(steps) if steps.is_empty() => {
                crate::render::feedback::user_error(
                    &format!("no plan found with id: {plan_id_str}"),
                    Some("Use /plan <goal> to create a plan first"),
                );
                return;
            }
            Ok(steps) => steps,
            Err(e) => {
                crate::render::feedback::user_error(&format!("failed to load plan: {e}"), None);
                return;
            }
        };

        // Build sub-agent tasks from plan steps.
        use cuervo_core::types::{AgentType, SubAgentTask};
        use std::collections::HashSet;

        let tasks: Vec<SubAgentTask> = plan_steps
            .iter()
            .map(|step| SubAgentTask {
                task_id: uuid::Uuid::new_v4(),
                instruction: step.description.clone(),
                agent_type: AgentType::Chat,
                model: None,
                provider: None,
                allowed_tools: HashSet::new(),
                limits_override: None,
                depends_on: vec![],
                priority: step.step_index,
            })
            .collect();

        // Run via orchestrator (reuse existing orchestrator wiring).
        eprintln!("Executing plan {plan_id_str} ({} steps)", tasks.len());
        self.run_orchestrate(&format!("Execute plan steps for: {}", plan_steps.first().map(|s| s.goal.as_str()).unwrap_or("unknown")))
            .await;
    }

    pub(crate) async fn handle_resume(&mut self, session_id_str: &str) {
        let session_id = match uuid::Uuid::parse_str(session_id_str) {
            Ok(id) => id,
            Err(_) => {
                // Try prefix match.
                if let Some(db) = &self.db {
                    let sessions = db.list_sessions(50).unwrap_or_default();
                    match sessions.iter().find(|s| s.id.to_string().starts_with(session_id_str)) {
                        Some(s) => s.id,
                        None => {
                            crate::render::feedback::user_error(
                                &format!("session not found: {session_id_str}"),
                                None,
                            );
                            return;
                        }
                    }
                } else {
                    crate::render::feedback::user_error("no database configured", None);
                    return;
                }
            }
        };

        let Some(ref adb) = self.async_db else {
            crate::render::feedback::user_warning("no database configured", None);
            return;
        };

        // Load latest checkpoint.
        match adb.load_latest_checkpoint(session_id).await {
            Ok(Some(checkpoint)) => {
                // Deserialize messages from checkpoint.
                match serde_json::from_str::<Vec<ChatMessage>>(&checkpoint.messages_json) {
                    Ok(messages) => {
                        self.session.messages = messages;
                        self.session.agent_rounds = checkpoint.round;
                        eprintln!(
                            "Resumed session {} from round {} ({} messages)",
                            &session_id.to_string()[..8],
                            checkpoint.round,
                            self.session.messages.len(),
                        );
                    }
                    Err(e) => {
                        crate::render::feedback::user_error(
                            &format!("failed to deserialize checkpoint: {e}"),
                            None,
                        );
                    }
                }
            }
            Ok(None) => {
                // No checkpoint, try loading session directly.
                match adb.load_session(session_id).await {
                    Ok(Some(session)) => {
                        self.session = session;
                        eprintln!(
                            "Resumed session {} ({} messages, no checkpoint)",
                            &session_id.to_string()[..8],
                            self.session.messages.len(),
                        );
                    }
                    Ok(None) => {
                        crate::render::feedback::user_error(
                            &format!("session not found: {}", &session_id.to_string()[..8]),
                            None,
                        );
                    }
                    Err(e) => {
                        crate::render::feedback::user_error(
                            &format!("failed to load session: {e}"),
                            None,
                        );
                    }
                }
            }
            Err(e) => {
                crate::render::feedback::user_error(
                    &format!("failed to load checkpoint: {e}"),
                    None,
                );
            }
        }
    }

    pub(crate) async fn handle_cancel(&self, task_id_str: &str) {
        let Some(ref adb) = self.async_db else {
            crate::render::feedback::user_warning("no database configured", None);
            return;
        };

        match adb
            .update_agent_task_status(task_id_str, "cancelled", 0, 0, 0.0, 0, 0, Some("Cancelled by user"), None)
            .await
        {
            Ok(()) => {
                eprintln!("Task {task_id_str} cancelled.");
            }
            Err(e) => {
                crate::render::feedback::user_error(&format!("failed to cancel task: {e}"), None);
            }
        }
    }

    pub(crate) async fn handle_replay(&self, session_id_str: &str) {
        let session_id = match uuid::Uuid::parse_str(session_id_str) {
            Ok(id) => id,
            Err(_) => {
                // Try prefix match.
                if let Some(db) = &self.db {
                    let sessions = db.list_sessions(50).unwrap_or_default();
                    match sessions.iter().find(|s| s.id.to_string().starts_with(session_id_str)) {
                        Some(s) => s.id,
                        None => {
                            crate::render::feedback::user_error(
                                &format!("session not found: {session_id_str}"),
                                None,
                            );
                            return;
                        }
                    }
                } else {
                    crate::render::feedback::user_error("no database configured", None);
                    return;
                }
            }
        };

        let Some(ref adb) = self.async_db else {
            crate::render::feedback::user_warning("no database configured", None);
            return;
        };

        eprintln!("Replaying session {}...", &session_id.to_string()[..8]);

        match replay_runner::run_replay(
            session_id,
            adb,
            &self.tool_registry,
            &self.event_tx,
            true,
        )
        .await
        {
            Ok(result) => {
                let mut out = std::io::stderr().lock();
                let replay_info = console::ReplayInfo {
                    original_id: &result.original_session_id.to_string()[..8],
                    replay_id: &result.replay_session_id.to_string()[..8],
                    original_fp: result.original_fingerprint.as_deref(),
                    replay_fp: &result.replay_fingerprint,
                    fp_match: result.fingerprint_match,
                    rounds: result.rounds,
                    steps: result.steps_replayed,
                };
                console::render_replay_result(&replay_info, &mut out);
            }
            Err(e) => {
                crate::render::feedback::user_error(&format!("replay failed: {e}"), None);
            }
        }
    }

    pub(crate) async fn handle_step(&mut self, direction: &commands::StepDirection) {
        let Some(db) = &self.db else {
            crate::render::feedback::user_warning("no database configured", None);
            return;
        };

        // Load trace if no cursor exists.
        if self.trace_cursor.is_none() {
            let steps = match db.load_trace_steps(self.session.id) {
                Ok(s) if s.is_empty() => {
                    crate::render::feedback::user_warning(
                        "no trace steps for current session",
                        Some("Run a message first to generate trace data"),
                    );
                    return;
                }
                Ok(s) => s,
                Err(e) => {
                    crate::render::feedback::user_error(
                        &format!("failed to load trace: {e}"),
                        None,
                    );
                    return;
                }
            };
            self.trace_cursor = Some((self.session.id, steps, 0));
        }

        let (_, ref steps, ref mut index) = self.trace_cursor.as_mut().unwrap();

        match direction {
            commands::StepDirection::Forward => {
                if *index < steps.len().saturating_sub(1) {
                    *index += 1;
                }
            }
            commands::StepDirection::Back => {
                *index = index.saturating_sub(1);
            }
        }

        let idx = *index;
        let total = steps.len();
        let step = &steps[idx];

        let mut out = std::io::stderr().lock();
        console::render_trace_step(step, idx, total, &mut out);
    }

    pub(crate) async fn handle_snapshot(&self) {
        let Some(ref adb) = self.async_db else {
            crate::render::feedback::user_warning("no database configured", None);
            return;
        };

        let messages_json = match serde_json::to_string(&self.session.messages) {
            Ok(j) => j,
            Err(e) => {
                crate::render::feedback::user_error(
                    &format!("failed to serialize session: {e}"),
                    None,
                );
                return;
            }
        };

        let usage_json = serde_json::to_string(&self.session.total_usage).unwrap_or_default();
        let fingerprint = self
            .session
            .execution_fingerprint
            .clone()
            .unwrap_or_else(|| "manual_snapshot".to_string());

        let checkpoint = cuervo_storage::SessionCheckpoint {
            session_id: self.session.id,
            round: self.session.agent_rounds,
            step_index: 0,
            messages_json,
            usage_json,
            fingerprint,
            created_at: chrono::Utc::now(),
            agent_state: None,
        };

        match adb.save_checkpoint(&checkpoint).await {
            Ok(()) => {
                eprintln!(
                    "Snapshot saved at round {} for session {}",
                    self.session.agent_rounds,
                    &self.session.id.to_string()[..8],
                );
            }
            Err(e) => {
                crate::render::feedback::user_error(
                    &format!("failed to save snapshot: {e}"),
                    None,
                );
            }
        }
    }

    pub(crate) async fn handle_diff(&self, id_a: &str, id_b: &str) {
        let Some(db) = &self.db else {
            crate::render::feedback::user_warning("no database configured", None);
            return;
        };

        let sessions = db.list_sessions(100).unwrap_or_default();

        let resolve_id = |prefix: &str| -> Option<uuid::Uuid> {
            uuid::Uuid::parse_str(prefix).ok().or_else(|| {
                sessions.iter().find(|s| s.id.to_string().starts_with(prefix)).map(|s| s.id)
            })
        };

        let (sa_id, sb_id) = match (resolve_id(id_a), resolve_id(id_b)) {
            (Some(a), Some(b)) => (a, b),
            (None, _) => {
                crate::render::feedback::user_error(&format!("session not found: {id_a}"), None);
                return;
            }
            (_, None) => {
                crate::render::feedback::user_error(&format!("session not found: {id_b}"), None);
                return;
            }
        };

        let sa = match db.load_session(sa_id) {
            Ok(Some(s)) => s,
            _ => {
                crate::render::feedback::user_error(&format!("failed to load session: {id_a}"), None);
                return;
            }
        };
        let sb = match db.load_session(sb_id) {
            Ok(Some(s)) => s,
            _ => {
                crate::render::feedback::user_error(&format!("failed to load session: {id_b}"), None);
                return;
            }
        };

        let kv_a = session_to_kv(&sa);
        let kv_b = session_to_kv(&sb);

        let mut out = std::io::stderr().lock();
        console::diff_sessions(&kv_a, &kv_b, &mut out);
    }

    pub(crate) async fn handle_research(&mut self, query: &str) {
        if !self.config.orchestrator.enabled {
            crate::render::feedback::user_error(
                "orchestrator is disabled (required for /research)",
                Some("Set [orchestrator] enabled = true in config.toml"),
            );
            return;
        }

        let provider = match self.registry.get(&self.provider).cloned() {
            Some(p) => p,
            None => {
                crate::render::feedback::user_error(
                    &format!("provider '{}' not available", self.provider),
                    None,
                );
                return;
            }
        };

        let decomposed = console::decompose_research(query);

        use cuervo_core::types::{AgentType, SubAgentTask};
        use std::collections::HashSet;

        let tasks: Vec<SubAgentTask> = decomposed
            .iter()
            .map(|(instruction, _)| SubAgentTask {
                task_id: uuid::Uuid::new_v4(),
                instruction: instruction.clone(),
                agent_type: AgentType::Chat,
                model: None,
                provider: None,
                allowed_tools: HashSet::new(),
                limits_override: None,
                depends_on: vec![],
                priority: 0,
            })
            .collect();

        let orchestrator_id = uuid::Uuid::new_v4();
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

        let fallback_providers: Vec<(String, Arc<dyn cuervo_core::traits::ModelProvider>)> = self
            .config
            .agent
            .routing
            .fallback_models
            .iter()
            .filter_map(|name| {
                self.registry.get(name).cloned().map(|p| (name.clone(), p))
            })
            .collect();

        let guardrails: &[Box<dyn cuervo_security::Guardrail>] =
            if self.config.security.guardrails.enabled && self.config.security.guardrails.builtins {
                cuervo_security::builtin_guardrails()
            } else {
                &[]
            };

        eprintln!("Researching: {query}");

        match orchestrator::run_orchestrator(
            orchestrator_id,
            tasks,
            &provider,
            &self.tool_registry,
            &self.event_tx,
            &self.config.agent.limits,
            &self.config.orchestrator,
            &self.config.agent.routing,
            self.async_db.as_ref(),
            self.response_cache.as_ref(),
            &fallback_providers,
            &self.model,
            &working_dir,
            None,
            guardrails,
            self.config.tools.confirm_destructive,
            self.config.security.tbac_enabled,
        )
        .await
        {
            Ok(result) => {
                let agent_results: Vec<(String, String, bool)> = result
                    .sub_results
                    .iter()
                    .zip(decomposed.iter())
                    .map(|(sub, (label, _))| {
                        (label.clone(), sub.output_text.clone(), sub.success)
                    })
                    .collect();

                let mut out = std::io::stderr().lock();
                console::render_research_report(
                    query,
                    &agent_results,
                    result.total_input_tokens + result.total_output_tokens,
                    result.total_cost_usd,
                    &mut out,
                );
            }
            Err(e) => {
                crate::render::feedback::user_error(&format!("research failed: {e}"), None);
            }
        }
    }

    pub(crate) async fn handle_benchmark(&self, workload: &str) {
        let mut out = std::io::stderr().lock();

        match workload {
            "inference" => {
                // Run 3 probe invocations and measure latency.
                let provider = match self.registry.get(&self.provider).cloned() {
                    Some(p) => p,
                    None => {
                        crate::render::feedback::user_error("no provider available", None);
                        return;
                    }
                };

                let probe = ModelRequest {
                    model: self.model.clone(),
                    messages: vec![ChatMessage {
                        role: Role::User,
                        content: MessageContent::Text("Say OK".to_string()),
                    }],
                    tools: vec![],
                    max_tokens: Some(16),
                    temperature: Some(0.0),
                    system: None,
                    stream: true,
                };

                let mut latencies = Vec::new();
                for _ in 0..3 {
                    let start = std::time::Instant::now();
                    if let Ok(mut stream) = provider.invoke(&probe).await {
                        while let Some(chunk) = stream.next().await {
                            match chunk {
                                Ok(ModelChunk::Done(_)) => break,
                                Err(_) => break,
                                _ => {}
                            }
                        }
                    }
                    latencies.push(start.elapsed().as_millis() as u64);
                }

                latencies.sort();
                let p50 = latencies.get(1).copied().unwrap_or(0);
                let p95 = latencies.last().copied().unwrap_or(0);
                let avg = if latencies.is_empty() {
                    0
                } else {
                    latencies.iter().sum::<u64>() / latencies.len() as u64
                };

                let results = vec![
                    ("Provider".into(), self.provider.clone()),
                    ("Model".into(), self.model.clone()),
                    ("Samples".into(), latencies.len().to_string()),
                    ("Avg latency".into(), format!("{avg}ms")),
                    ("P50 latency".into(), format!("{p50}ms")),
                    ("P95 latency".into(), format!("{p95}ms")),
                ];
                console::render_benchmark("inference", &results, &mut out);
            }
            "cache" => {
                let Some(db) = &self.db else {
                    crate::render::feedback::user_warning("no database configured", None);
                    return;
                };
                let stats = db.cache_stats().unwrap_or(cuervo_storage::CacheStats {
                    total_entries: 0,
                    total_hits: 0,
                    oldest_entry: None,
                    newest_entry: None,
                });
                let hit_rate = if stats.total_entries > 0 {
                    format!("{:.1}%", stats.total_hits as f64 / stats.total_entries as f64 * 100.0)
                } else {
                    "N/A".into()
                };
                let results = vec![
                    ("Entries".into(), stats.total_entries.to_string()),
                    ("Total hits".into(), stats.total_hits.to_string()),
                    ("Hit rate".into(), hit_rate),
                ];
                console::render_benchmark("cache", &results, &mut out);
            }
            "tools" | "full" => {
                let Some(db) = &self.db else {
                    crate::render::feedback::user_warning("no database configured", None);
                    return;
                };
                let tool_stats = db.top_tool_stats(10).unwrap_or_default();
                let results: Vec<(String, String)> = tool_stats
                    .iter()
                    .map(|ts| {
                        (
                            ts.tool_name.clone(),
                            format!(
                                "{} exec, {:.0}ms avg, {:.0}% success",
                                ts.total_executions,
                                ts.avg_duration_ms,
                                ts.success_rate * 100.0,
                            ),
                        )
                    })
                    .collect();
                console::render_benchmark(workload, &results, &mut out);
            }
            _ => {
                crate::render::feedback::user_error(
                    &format!("unknown workload: {workload}"),
                    Some("Available: inference, tools, cache, full"),
                );
            }
        }
    }

    pub(crate) async fn handle_optimize(&self) {
        let mut out = std::io::stderr().lock();

        let Some(db) = &self.db else {
            crate::render::feedback::user_warning("no database configured", None);
            return;
        };

        let sys = match db.system_metrics() {
            Ok(s) => s,
            Err(e) => {
                crate::render::feedback::user_error(&format!("failed to load metrics: {e}"), None);
                return;
            }
        };

        let mut recommendations = Vec::new();

        for m in &sys.models {
            if m.p95_latency_ms > 2000 {
                recommendations.push((
                    format!("{}/{}: P95 latency {}ms", m.provider, m.model, m.p95_latency_ms),
                    "Consider a faster model or increase timeout".to_string(),
                ));
            }
            if m.success_rate < 0.9 && m.total_invocations > 5 {
                recommendations.push((
                    format!(
                        "{}/{}: success rate {:.0}%",
                        m.provider, m.model,
                        m.success_rate * 100.0
                    ),
                    "Check API key, rate limits, or switch provider".to_string(),
                ));
            }
        }

        let cache = db.cache_stats().ok();
        if let Some(cs) = &cache {
            if cs.total_entries > 0 && cs.total_hits == 0 {
                recommendations.push((
                    "Cache: 0 hits".to_string(),
                    "Cache is populated but never hit — check cache key generation".to_string(),
                ));
            }
        }

        console::render_optimize(&recommendations, &mut out);
    }

    pub(crate) async fn handle_analyze(&self) {
        let mut out = std::io::stderr().lock();

        let Some(db) = &self.db else {
            crate::render::feedback::user_warning("no database configured", None);
            return;
        };

        let sys = match db.system_metrics() {
            Ok(s) => s,
            Err(e) => {
                crate::render::feedback::user_error(&format!("failed to load metrics: {e}"), None);
                return;
            }
        };

        // Model rankings by cost (descending).
        let mut model_rankings: Vec<(String, u64, f64, f64)> = sys
            .models
            .iter()
            .map(|m| {
                (
                    format!("{}/{}", m.provider, m.model),
                    m.total_invocations,
                    m.total_cost_usd,
                    m.avg_latency_ms,
                )
            })
            .collect();
        model_rankings.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

        // Tool rankings by usage with bottleneck detection.
        let tool_stats = db.top_tool_stats(20).unwrap_or_default();
        let tool_rankings: Vec<(String, u64, f64, bool)> = tool_stats
            .iter()
            .map(|ts| {
                let is_bottleneck = ts.avg_duration_ms > 5000.0;
                (
                    ts.tool_name.clone(),
                    ts.total_executions,
                    ts.avg_duration_ms,
                    is_bottleneck,
                )
            })
            .collect();

        console::render_analyze(&model_rankings, &tool_rankings, &mut out);
    }
}

/// Convert a Session to key-value pairs for diffing.
fn session_to_kv(session: &Session) -> Vec<(String, String)> {
    vec![
        ("Session".into(), session.id.to_string()[..8].to_string()),
        ("Model".into(), format!("{}/{}", session.provider, session.model)),
        ("Messages".into(), session.messages.len().to_string()),
        ("Rounds".into(), session.agent_rounds.to_string()),
        ("Tools".into(), session.tool_invocations.to_string()),
        (
            "Tokens".into(),
            format!(
                "{}/{}",
                session.total_usage.input_tokens, session.total_usage.output_tokens
            ),
        ),
        ("Cost".into(), format!("${:.4}", session.estimated_cost_usd)),
        ("Latency".into(), format!("{}ms", session.total_latency_ms)),
        (
            "Fingerprint".into(),
            session
                .execution_fingerprint
                .as_deref()
                .unwrap_or("(none)")
                .to_string(),
        ),
    ]
}
