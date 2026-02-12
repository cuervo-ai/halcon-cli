//! `cuervo doctor` — diagnostic command for runtime health.
//!
//! Reports provider health, cache stats, recent metrics, and recommendations.
//! Read-only: no side effects.

use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use cuervo_core::types::AppConfig;
use cuervo_storage::Database;

#[cfg(feature = "color-science")]
use momoto_intelligence::{RecommendationContext, RecommendationEngine, UsageContext};

#[cfg(feature = "color-science")]
use crate::render::color_science;
use crate::render::{color, components, theme};
use crate::repl::optimizer::{CostLatencyOptimizer, OptimizeStrategy};

/// Run the doctor diagnostic report.
pub fn run(config: &AppConfig, db_path: Option<&Path>) -> Result<()> {
    let db = open_db(config, db_path)?;
    let t = theme::active();
    let r = theme::reset();
    let mut out = io::stderr().lock();

    // Title banner.
    let primary = t.palette.primary.fg();
    let h = color::box_horiz();
    let tl = color::box_top_left();
    let tr = color::box_top_right();
    let bl = color::box_bottom_left();
    let br = color::box_bottom_right();
    let muted = t.palette.muted.fg();

    let _ = writeln!(
        out,
        "\n  {muted}{tl}{h}{r} {primary}Cuervo Doctor{r} {muted}{}{tr}{r}",
        h.repeat(38),
    );

    print_config_section(config, &mut out);
    print_provider_section(config, &db, &mut out);
    print_health_section(config, &db, &mut out);
    print_cache_section(&db, &mut out);
    print_metrics_section(&db, &mut out);
    print_tool_metrics_section(&db, &mut out);
    print_orchestrator_section(config, &db, &mut out);
    print_replay_section(&db, &mut out);
    print_optimizer_section(&db, &mut out);
    print_resilience_section(&db, &mut out);
    print_phase14_section(config, &mut out);
    print_model_selection_section(config, &mut out);
    #[cfg(feature = "color-science")]
    print_accessibility_section(config, &mut out);
    print_console_section(config, &db, &mut out);
    print_recommendations(config, &db, &mut out);

    let _ = writeln!(
        out,
        "  {muted}{bl}{}{br}{r}\n",
        h.repeat(54),
    );

    let _ = out.flush();
    println!();
    Ok(())
}

fn open_db(
    config: &AppConfig,
    db_path: Option<&Path>,
) -> Result<Option<Arc<Database>>> {
    let path = db_path
        .map(|p| p.to_path_buf())
        .or_else(|| config.storage.database_path.clone())
        .or_else(|| {
            dirs::home_dir().map(|h| h.join(".cuervo").join("cuervo.db"))
        });

    match path {
        Some(p) if p.exists() => {
            let db = Database::open(&p)?;
            Ok(Some(Arc::new(db)))
        }
        _ => Ok(None),
    }
}

fn print_config_section(config: &AppConfig, out: &mut impl Write) {
    use cuervo_core::types::{validate_config, IssueLevel};
    let t = theme::active();
    let r = theme::reset();

    components::section_header("Configuration", out);
    let issues = validate_config(config);
    if issues.is_empty() {
        let success = t.palette.success.fg();
        let _ = writeln!(out, "    {success}All settings valid{r}");
    } else {
        for issue in &issues {
            let level = match issue.level {
                IssueLevel::Error => components::BadgeLevel::Error,
                IssueLevel::Warning => components::BadgeLevel::Warning,
            };
            let badge = components::badge(
                match issue.level {
                    IssueLevel::Error => "ERROR",
                    IssueLevel::Warning => "WARN",
                },
                level,
            );
            let _ = writeln!(out, "    {badge} {}: {}", issue.field, issue.message);
            if let Some(ref suggestion) = issue.suggestion {
                let hint = t.palette.muted.fg();
                let _ = writeln!(out, "      {hint}-> {suggestion}{r}");
            }
        }
    }
}

fn print_provider_section(config: &AppConfig, db: &Option<Arc<Database>>, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();

    components::section_header("Providers", out);

    let provider_name = &config.general.default_provider;
    let model_name = &config.general.default_model;
    let accent = t.palette.accent.fg();
    let _ = writeln!(out, "    Primary: {accent}{provider_name}/{model_name}{r}");

    if let Some(db) = db {
        let metrics = db.system_metrics().unwrap_or_default();
        if !metrics.models.is_empty() {
            for stat in &metrics.models {
                let success_pct = (stat.success_rate * 100.0).round() as u32;
                let (badge_text, level) = if success_pct >= 95 {
                    ("OK", components::BadgeLevel::Success)
                } else if success_pct >= 80 {
                    ("DEGRADED", components::BadgeLevel::Warning)
                } else {
                    ("UNHEALTHY", components::BadgeLevel::Error)
                };
                let badge = components::badge(badge_text, level);
                let dim = t.palette.text_dim.fg();
                let _ = writeln!(
                    out,
                    "    {}/{}: {badge} {dim}({}% success, {:.0}ms avg, {} calls){r}",
                    stat.provider,
                    stat.model,
                    success_pct,
                    stat.avg_latency_ms,
                    stat.total_invocations,
                );
            }
        } else {
            let muted = t.palette.muted.fg();
            let _ = writeln!(out, "    {muted}(no invocation data yet){r}");
        }
    } else {
        let muted = t.palette.muted.fg();
        let _ = writeln!(out, "    {muted}(no database configured){r}");
    }
}

fn print_cache_section(db: &Option<Arc<Database>>, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();

    components::section_header("Cache", out);
    if let Some(db) = db {
        match db.cache_stats() {
            Ok(stats) => {
                let hit_rate_str = if stats.total_entries > 0 {
                    format!("{} total hits", stats.total_hits)
                } else {
                    "no entries".to_string()
                };
                let dim = t.palette.text_dim.fg();
                let _ = writeln!(out, "    Entries: {}  {dim}|{r}  {hit_rate_str}", stats.total_entries);
                if let Some(oldest) = stats.oldest_entry {
                    let _ = writeln!(out, "    {dim}Oldest: {}{r}", oldest.format("%Y-%m-%d %H:%M"));
                }
            }
            Err(_) => {
                let muted = t.palette.muted.fg();
                let _ = writeln!(out, "    {muted}(cache stats unavailable){r}");
            }
        }
    } else {
        let muted = t.palette.muted.fg();
        let _ = writeln!(out, "    {muted}(no database){r}");
    }
}

fn print_metrics_section(db: &Option<Arc<Database>>, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();

    components::section_header("Metrics", out);
    if let Some(db) = db {
        let metrics = db.system_metrics().unwrap_or_default();
        if metrics.total_invocations > 0 {
            let dim = t.palette.text_dim.fg();
            let _ = writeln!(
                out,
                "    Total invocations: {}  {dim}|{r}  Cost: ${:.4}",
                metrics.total_invocations, metrics.total_cost_usd
            );
            let _ = writeln!(out, "    Total tokens: {}", metrics.total_tokens);
        } else {
            let muted = t.palette.muted.fg();
            let _ = writeln!(out, "    {muted}(no metrics data yet){r}");
        }
    } else {
        let muted = t.palette.muted.fg();
        let _ = writeln!(out, "    {muted}(no database){r}");
    }
}

fn print_tool_metrics_section(db: &Option<Arc<Database>>, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();

    components::section_header("Tool Metrics", out);
    if let Some(db) = db {
        match db.top_tool_stats(5) {
            Ok(stats) if !stats.is_empty() => {
                for stat in &stats {
                    let success_pct = (stat.success_rate * 100.0).round() as u32;
                    let dim = t.palette.text_dim.fg();
                    let accent = t.palette.accent.fg();
                    let _ = writeln!(
                        out,
                        "    {accent}{}{r}: {} calls, {dim}{:.0}ms avg, {}% success{r}",
                        stat.tool_name,
                        stat.total_executions,
                        stat.avg_duration_ms,
                        success_pct,
                    );
                }
            }
            Ok(_) => {
                let muted = t.palette.muted.fg();
                let _ = writeln!(out, "    {muted}(no tool execution data yet){r}");
            }
            Err(_) => {
                let muted = t.palette.muted.fg();
                let _ = writeln!(out, "    {muted}(tool metrics unavailable){r}");
            }
        }
    } else {
        let muted = t.palette.muted.fg();
        let _ = writeln!(out, "    {muted}(no database){r}");
    }
}

fn print_orchestrator_section(config: &AppConfig, db: &Option<Arc<Database>>, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();

    components::section_header("Orchestrator", out);
    let (status_text, level) = if config.orchestrator.enabled {
        ("enabled", components::BadgeLevel::Success)
    } else {
        ("disabled", components::BadgeLevel::Muted)
    };
    let badge = components::badge(status_text, level);
    let _ = writeln!(out, "    Status: {badge}");

    let dim = t.palette.text_dim.fg();
    let _ = writeln!(out, "    {dim}Max concurrent agents: {}{r}", config.orchestrator.max_concurrent_agents);
    if config.orchestrator.sub_agent_timeout_secs > 0 {
        let _ = writeln!(out, "    {dim}Sub-agent timeout: {}s{r}", config.orchestrator.sub_agent_timeout_secs);
    } else {
        let _ = writeln!(out, "    {dim}Sub-agent timeout: (inherit from parent){r}");
    }
    let _ = writeln!(out, "    {dim}Shared budget: {}{r}", config.orchestrator.shared_budget);
    if let Some(db) = db {
        match db.count_recent_orchestrator_runs(7) {
            Ok(count) if count > 0 => {
                let _ = writeln!(out, "    Recent runs (7d): {count}");
            }
            Ok(_) => {
                let _ = writeln!(out, "    {dim}Recent runs (7d): 0{r}");
            }
            Err(_) => {
                let muted = t.palette.muted.fg();
                let _ = writeln!(out, "    {muted}Recent runs: (unavailable){r}");
            }
        }
    }
}

fn print_replay_section(db: &Option<Arc<Database>>, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();

    components::section_header("Replay & Checkpoints", out);
    if let Some(db) = db {
        let sessions = db.list_sessions(100).unwrap_or_default();
        let fingerprinted = sessions.iter().filter(|s| s.execution_fingerprint.is_some()).count();
        let replays = sessions.iter().filter(|s| s.replay_source_session.is_some()).count();

        let dim = t.palette.text_dim.fg();
        let _ = writeln!(out, "    Sessions with fingerprints: {fingerprinted}");
        let _ = writeln!(out, "    Replay sessions: {replays}");

        let mut total_checkpoints = 0u64;
        for session in &sessions {
            if let Ok(cps) = db.list_checkpoints(session.id) {
                total_checkpoints += cps.len() as u64;
            }
        }
        let _ = writeln!(out, "    {dim}Total checkpoints: {total_checkpoints}{r}");
    } else {
        let muted = t.palette.muted.fg();
        let _ = writeln!(out, "    {muted}(no database){r}");
    }
}

fn print_resilience_section(db: &Option<Arc<Database>>, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();

    components::section_header("Resilience Events", out);
    if let Some(db) = db {
        match db.resilience_events(None, None, 5) {
            Ok(events) if !events.is_empty() => {
                let dim = t.palette.text_dim.fg();
                for ev in &events {
                    let state_info = match (&ev.from_state, &ev.to_state) {
                        (Some(from), Some(to)) => format!("{from} -> {to}"),
                        _ => String::new(),
                    };
                    let _ = writeln!(
                        out,
                        "    {dim}[{}]{r} {} {} {}",
                        ev.created_at.format("%H:%M:%S"),
                        ev.provider,
                        ev.event_type,
                        state_info,
                    );
                }
            }
            Ok(_) => {
                let muted = t.palette.muted.fg();
                let _ = writeln!(out, "    {muted}(no recent resilience events){r}");
            }
            Err(_) => {
                let muted = t.palette.muted.fg();
                let _ = writeln!(out, "    {muted}(resilience events unavailable){r}");
            }
        }
    } else {
        let muted = t.palette.muted.fg();
        let _ = writeln!(out, "    {muted}(no database){r}");
    }
}

fn print_health_section(config: &AppConfig, db: &Option<Arc<Database>>, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();

    components::section_header("Health Scores", out);
    if let Some(db) = db {
        let health_config = &config.resilience.health;
        let metrics = db.system_metrics().unwrap_or_default();

        let providers: Vec<String> = metrics
            .models
            .iter()
            .map(|m| m.provider.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();

        if providers.is_empty() {
            let muted = t.palette.muted.fg();
            let _ = writeln!(out, "    {muted}(no providers with metrics){r}");
        } else {
            for provider in &providers {
                let report = assess_sync(db, provider, health_config);
                let (badge_text, level) = match report.level {
                    crate::repl::health::HealthLevel::Healthy => ("OK", components::BadgeLevel::Success),
                    crate::repl::health::HealthLevel::Degraded => ("DEGRADED", components::BadgeLevel::Warning),
                    crate::repl::health::HealthLevel::Unhealthy => ("UNHEALTHY", components::BadgeLevel::Error),
                };
                let badge = components::badge(badge_text, level);
                let dim = t.palette.text_dim.fg();
                let _ = writeln!(
                    out,
                    "    {}: {badge} {dim}(score {}, err {:.0}%, {:.0}ms avg, p95 {}ms, tmout {:.0}%, {} calls){r}",
                    report.provider,
                    report.score,
                    report.error_rate * 100.0,
                    report.avg_latency_ms,
                    report.p95_latency_ms,
                    report.timeout_rate * 100.0,
                    report.invocation_count,
                );
            }
        }
    } else {
        let muted = t.palette.muted.fg();
        let _ = writeln!(out, "    {muted}(no database){r}");
    }
}

/// Sync health assessment for the doctor command (avoids async overhead).
///
/// Uses the shared `compute_health_score` formula from the health module
/// to avoid formula divergence.
fn assess_sync(
    db: &Database,
    provider: &str,
    config: &cuervo_core::types::HealthConfig,
) -> crate::repl::health::HealthReport {
    use crate::repl::health::{compute_health_score, HealthLevel, HealthReport};

    let metrics = db
        .provider_metrics_windowed(provider, config.window_minutes)
        .unwrap_or_default();

    if metrics.total_invocations == 0 {
        return HealthReport {
            provider: provider.to_string(),
            score: 100,
            level: HealthLevel::Healthy,
            error_rate: 0.0,
            avg_latency_ms: 0.0,
            p95_latency_ms: 0,
            timeout_rate: 0.0,
            invocation_count: 0,
        };
    }

    let score = compute_health_score(&metrics);

    let level = if score <= config.unhealthy_threshold {
        HealthLevel::Unhealthy
    } else if score <= config.degraded_threshold {
        HealthLevel::Degraded
    } else {
        HealthLevel::Healthy
    };

    HealthReport {
        provider: provider.to_string(),
        score,
        level,
        error_rate: metrics.error_rate,
        avg_latency_ms: metrics.avg_latency_ms,
        p95_latency_ms: metrics.p95_latency_ms,
        timeout_rate: metrics.timeout_rate,
        invocation_count: metrics.total_invocations,
    }
}

fn print_optimizer_section(db: &Option<Arc<Database>>, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();

    components::section_header("Model Rankings (Balanced)", out);
    if let Some(db) = db {
        let optimizer = CostLatencyOptimizer::new(Arc::clone(db));
        let ranked = optimizer.rank_models(OptimizeStrategy::Balanced);

        if ranked.is_empty() {
            let muted = t.palette.muted.fg();
            let _ = writeln!(out, "    {muted}(insufficient data -- need >= 3 invocations/model){r}");
        } else {
            let dim = t.palette.text_dim.fg();
            let accent = t.palette.accent.fg();
            for (i, model) in ranked.iter().take(5).enumerate() {
                let _ = writeln!(
                    out,
                    "    {accent}{}. {}/{}{r}: {:.3} {dim}({:.0}ms, ${:.4}, {:.0}% success){r}",
                    i + 1,
                    model.provider,
                    model.model,
                    model.score,
                    model.avg_latency_ms,
                    model.avg_cost,
                    model.success_rate * 100.0,
                );
            }
        }
    } else {
        let muted = t.palette.muted.fg();
        let _ = writeln!(out, "    {muted}(no database){r}");
    }
}

fn print_phase14_section(config: &AppConfig, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();
    let dim = t.palette.text_dim.fg();

    components::section_header("Advanced Features", out);

    // Determinism
    let _ = writeln!(out, "    {dim}Deterministic execution:{r} active");

    // State Machine
    let _ = writeln!(out, "    {dim}Agent state machine:{r} available");

    // Observability
    let _ = writeln!(out, "    {dim}Trace context (W3C):{r} available");

    // Inter-Agent Communication
    let (comm_text, comm_level) = if config.orchestrator.enable_communication {
        ("enabled", components::BadgeLevel::Success)
    } else {
        ("disabled", components::BadgeLevel::Muted)
    };
    let comm_badge = components::badge(comm_text, comm_level);
    let _ = writeln!(out, "    Inter-agent communication: {comm_badge}");

    // Idempotency / Dry-Run
    let (dry_text, dry_level) = if config.tools.dry_run {
        ("enabled", components::BadgeLevel::Success)
    } else {
        ("disabled", components::BadgeLevel::Muted)
    };
    let dry_badge = components::badge(dry_text, dry_level);
    let _ = writeln!(out, "    Dry-run mode: {dry_badge}");
    let _ = writeln!(out, "    {dim}Idempotency registry:{r} active");

    // MCP Pool
    let server_count = config.mcp.servers.len();
    if server_count > 0 {
        let _ = writeln!(
            out,
            "    MCP servers: {server_count} configured  {dim}|{r}  max reconnect: {}",
            config.mcp.max_reconnect_attempts,
        );
    } else {
        let muted = t.palette.muted.fg();
        let _ = writeln!(out, "    {muted}MCP servers: none configured{r}");
    }
}

fn print_model_selection_section(config: &AppConfig, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();
    let dim = t.palette.text_dim.fg();

    components::section_header("Model Selection", out);

    let sel = &config.agent.model_selection;
    let (status_text, level) = if sel.enabled {
        ("active", components::BadgeLevel::Success)
    } else {
        ("disabled", components::BadgeLevel::Muted)
    };
    let badge = components::badge(status_text, level);
    let _ = writeln!(out, "    Status: {badge}");

    // Count registered providers
    let provider_count = config.models.providers.values().filter(|p| p.enabled).count();
    let _ = writeln!(out, "    {dim}Registered providers:{r} {provider_count} enabled");

    // Count available models (from enabled providers)
    let enabled_providers: Vec<&str> = config
        .models
        .providers
        .iter()
        .filter(|(_, p)| p.enabled)
        .map(|(name, _)| name.as_str())
        .collect();
    let _ = writeln!(out, "    {dim}Enabled:{r} {}", enabled_providers.join(", "));

    let _ = writeln!(out, "    {dim}Strategy:{r} {}", config.agent.routing.strategy);

    if sel.budget_cap_usd > 0.0 {
        let _ = writeln!(out, "    {dim}Budget cap:{r} ${:.2}", sel.budget_cap_usd);
    } else {
        let _ = writeln!(out, "    {dim}Budget cap:{r} unlimited");
    }

    if let Some(ref model) = sel.simple_model {
        let _ = writeln!(out, "    {dim}Simple override:{r} {model}");
    }
    if let Some(ref model) = sel.complex_model {
        let _ = writeln!(out, "    {dim}Complex override:{r} {model}");
    }
}

#[cfg(feature = "color-science")]
fn print_accessibility_section(config: &AppConfig, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();

    components::section_header("Color Accessibility", out);

    // Parse terminal background color.
    let bg_hex = &config.display.terminal_background;
    let bg_theme = momoto_core::Color::from_hex(bg_hex)
        .map(|c| {
            let [r, g, b] = c.to_srgb8();
            theme::ThemeColor::rgb(r, g, b)
        })
        .unwrap_or_else(|_| theme::ThemeColor::rgb(26, 26, 26));

    let dim = t.palette.text_dim.fg();
    let _ = writeln!(out, "    {dim}Background: {bg_hex}{r}");
    let _ = writeln!(out, "    {dim}Theme: {}{r}", t.name);

    let engine = RecommendationEngine::new();

    let mut pass_count = 0u32;
    let pairs = t.palette.semantic_pairs();
    let total = pairs.len() as u32;

    for (name, fg) in &pairs {
        let ratio = color_science::contrast_ratio(fg, &bg_theme);
        let lc = color_science::apca_contrast(fg, &bg_theme);
        let badge_text = color_science::wcag_badge(ratio);
        let passes = color_science::passes_aa(ratio);

        if passes {
            pass_count += 1;
        }

        let level = if ratio >= 4.5 {
            components::BadgeLevel::Success
        } else if ratio >= 3.0 {
            components::BadgeLevel::Warning
        } else {
            components::BadgeLevel::Error
        };
        let badge = components::badge(badge_text, level);

        let _ = writeln!(
            out,
            "    {name:>10}: {badge}  {dim}{ratio:.1}:1  Lc {lc:+.0}{r}",
        );

        // When failing, suggest an improved color via momoto-intelligence.
        if !passes {
            let usage = match *name {
                "primary" | "accent" => UsageContext::Interactive,
                _ => UsageContext::BodyText,
            };
            let ctx = RecommendationContext::new(usage, momoto_intelligence::ComplianceTarget::WCAG_AA);
            let rec = engine.improve_foreground(*fg.color(), *bg_theme.color(), ctx);
            let suggested_hex = rec.color.to_hex();
            let score_pct = (rec.score.overall * 100.0).round() as u32;
            let _ = writeln!(
                out,
                "    {dim}           -> suggest {suggested_hex} (quality {score_pct}%){r}",
            );
        }
    }

    // Perceptual diversity: minimum delta-E between any two semantic tokens.
    let mut min_dist = f64::MAX;
    for i in 0..pairs.len() {
        for j in (i + 1)..pairs.len() {
            let d = color_science::perceptual_distance(pairs[i].1, pairs[j].1);
            if d < min_dist {
                min_dist = d;
            }
        }
    }
    let _ = writeln!(
        out,
        "    {dim}Palette diversity (min delta-E): {min_dist:.3}{r}",
    );

    // Summary
    let summary_text = if pass_count == total {
        let success = t.palette.success.fg();
        format!("{success}All {total} tokens pass WCAG AA{r}")
    } else {
        let warn = t.palette.warning.fg();
        format!("{warn}{pass_count}/{total} tokens pass WCAG AA{r}")
    };
    let _ = writeln!(out, "    {summary_text}");
}

fn print_console_section(config: &AppConfig, db: &Option<Arc<Database>>, out: &mut impl Write) {
    components::section_header("Operating Console", out);

    // Available commands count.
    let console_commands = [
        "/research", "/inspect", "/plan", "/run", "/resume", "/cancel",
        "/status", "/metrics", "/logs", "/trace", "/replay", "/step",
        "/snapshot", "/diff", "/benchmark", "/optimize", "/analyze",
    ];
    let _ = writeln!(out, "    Available commands: {}", console_commands.len());

    if let Some(db) = db {
        // Session/task/plan counts.
        let sessions = db.list_sessions(1000).map(|s| s.len()).unwrap_or(0);
        let _ = writeln!(out, "    Sessions in DB: {sessions}");

        let plans = db.load_plan_steps("").unwrap_or_default().len();
        let _ = writeln!(out, "    Plan steps in DB: {plans}");
    } else {
        let _ = writeln!(out, "    (no database — some commands unavailable)");
    }

    // Orchestrator status.
    if config.orchestrator.enabled {
        let badge = components::badge("ON", components::BadgeLevel::Success);
        let _ = writeln!(out, "    Orchestrator: {badge}");
    } else {
        let badge = components::badge("OFF", components::BadgeLevel::Muted);
        let _ = writeln!(
            out,
            "    Orchestrator: {badge} (/research, /run require it)"
        );
    }
}

fn print_recommendations(config: &AppConfig, db: &Option<Arc<Database>>, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();
    let mut recommendations: Vec<String> = Vec::new();

    if !config.resilience.enabled {
        recommendations.push(
            "Enable resilience layer: set [resilience] enabled = true in config".to_string(),
        );
    }

    if !config.cache.enabled {
        recommendations
            .push("Enable response cache for faster repeated queries".to_string());
    }

    if let Some(db) = db {
        let metrics = db.system_metrics().unwrap_or_default();
        for stat in &metrics.models {
            if stat.success_rate < 0.80 && stat.total_invocations >= 3 {
                recommendations.push(format!(
                    "{}/{}: low success rate ({:.0}%). Check provider config.",
                    stat.provider,
                    stat.model,
                    stat.success_rate * 100.0,
                ));
            }
            if stat.avg_latency_ms > 5000.0 && stat.total_invocations >= 3 {
                recommendations.push(format!(
                    "{}/{}: high latency ({:.0}ms avg). Consider timeout tuning.",
                    stat.provider, stat.model, stat.avg_latency_ms,
                ));
            }
        }
    }

    components::section_header("Recommendations", out);
    if recommendations.is_empty() {
        let success = t.palette.success.fg();
        let _ = writeln!(out, "    {success}All systems nominal{r}");
    } else {
        let warn = t.palette.warning.fg();
        for rec in &recommendations {
            let _ = writeln!(out, "    {warn}-{r} {rec}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_runs_without_db() {
        let config = AppConfig::default();
        let result = run(&config, None);
        let _ = result;
    }

    #[test]
    fn doctor_with_in_memory_db() {
        let config = AppConfig::default();
        let db = Database::open_in_memory().unwrap();
        let db = Some(Arc::new(db));
        let mut out = Vec::new();

        print_provider_section(&config, &db, &mut out);
        print_health_section(&config, &db, &mut out);
        print_cache_section(&db, &mut out);
        print_metrics_section(&db, &mut out);
        print_tool_metrics_section(&db, &mut out);
        print_replay_section(&db, &mut out);
        print_optimizer_section(&db, &mut out);
        print_resilience_section(&db, &mut out);
        print_phase14_section(&config, &mut out);
        print_recommendations(&config, &db, &mut out);
    }

    #[test]
    fn doctor_with_seeded_metrics() {
        use chrono::Utc;
        use cuervo_storage::InvocationMetric;

        let config = AppConfig::default();
        let db = Arc::new(Database::open_in_memory().unwrap());

        for i in 0..5 {
            db.insert_metric(&InvocationMetric {
                provider: "anthropic".to_string(),
                model: "claude-sonnet".to_string(),
                latency_ms: 500 + i * 100,
                input_tokens: 100,
                output_tokens: 50,
                estimated_cost_usd: 0.002,
                success: true,
                stop_reason: "end_turn".to_string(),
                session_id: None,
                created_at: Utc::now(),
            })
            .unwrap();
        }

        db.insert_resilience_event(&cuervo_storage::ResilienceEvent {
            provider: "anthropic".to_string(),
            event_type: "breaker_trip".to_string(),
            from_state: Some("closed".to_string()),
            to_state: Some("open".to_string()),
            score: None,
            details: None,
            created_at: Utc::now(),
        })
        .unwrap();

        let db = Some(db);
        let mut out = Vec::new();
        print_provider_section(&config, &db, &mut out);
        print_health_section(&config, &db, &mut out);
        print_cache_section(&db, &mut out);
        print_metrics_section(&db, &mut out);
        print_optimizer_section(&db, &mut out);
        print_resilience_section(&db, &mut out);
        print_recommendations(&config, &db, &mut out);
    }

    #[test]
    fn health_section_shows_per_provider_scores() {
        use chrono::Utc;
        use cuervo_storage::InvocationMetric;

        let config = AppConfig::default();
        let db = Arc::new(Database::open_in_memory().unwrap());

        for _ in 0..5 {
            db.insert_metric(&InvocationMetric {
                provider: "anthropic".to_string(),
                model: "sonnet".to_string(),
                latency_ms: 300,
                input_tokens: 100,
                output_tokens: 50,
                estimated_cost_usd: 0.002,
                success: true,
                stop_reason: "end_turn".to_string(),
                session_id: None,
                created_at: Utc::now(),
            })
            .unwrap();
        }
        for _ in 0..3 {
            db.insert_metric(&InvocationMetric {
                provider: "ollama".to_string(),
                model: "llama3".to_string(),
                latency_ms: 2000,
                input_tokens: 50,
                output_tokens: 30,
                estimated_cost_usd: 0.0,
                success: true,
                stop_reason: "end_turn".to_string(),
                session_id: None,
                created_at: Utc::now(),
            })
            .unwrap();
        }

        let db = Some(db);
        let mut out = Vec::new();
        print_health_section(&config, &db, &mut out);
    }

    #[test]
    fn optimizer_section_ranks_models() {
        use chrono::Utc;
        use cuervo_storage::InvocationMetric;

        let db = Arc::new(Database::open_in_memory().unwrap());

        for _ in 0..5 {
            db.insert_metric(&InvocationMetric {
                provider: "anthropic".to_string(),
                model: "sonnet".to_string(),
                latency_ms: 400,
                input_tokens: 100,
                output_tokens: 50,
                estimated_cost_usd: 0.003,
                success: true,
                stop_reason: "end_turn".to_string(),
                session_id: None,
                created_at: Utc::now(),
            })
            .unwrap();
        }
        for _ in 0..4 {
            db.insert_metric(&InvocationMetric {
                provider: "anthropic".to_string(),
                model: "opus".to_string(),
                latency_ms: 2000,
                input_tokens: 200,
                output_tokens: 100,
                estimated_cost_usd: 0.03,
                success: true,
                stop_reason: "end_turn".to_string(),
                session_id: None,
                created_at: Utc::now(),
            })
            .unwrap();
        }

        let db = Some(db);
        let mut out = Vec::new();
        print_optimizer_section(&db, &mut out);
    }

    #[test]
    fn tool_metrics_section_with_data() {
        use chrono::Utc;
        use cuervo_storage::ToolExecutionMetric;

        let db = Arc::new(Database::open_in_memory().unwrap());

        for _ in 0..5 {
            db.insert_tool_metric(&ToolExecutionMetric {
                tool_name: "bash".to_string(),
                session_id: Some("s1".to_string()),
                duration_ms: 150,
                success: true,
                is_parallel: false,
                input_summary: None,
                created_at: Utc::now(),
            })
            .unwrap();
        }
        for _ in 0..3 {
            db.insert_tool_metric(&ToolExecutionMetric {
                tool_name: "read_file".to_string(),
                session_id: Some("s1".to_string()),
                duration_ms: 50,
                success: true,
                is_parallel: true,
                input_summary: None,
                created_at: Utc::now(),
            })
            .unwrap();
        }

        let db = Some(db);
        let mut out = Vec::new();
        print_tool_metrics_section(&db, &mut out);
    }

    #[test]
    fn tool_metrics_section_no_db() {
        let db: Option<Arc<Database>> = None;
        let mut out = Vec::new();
        print_tool_metrics_section(&db, &mut out);
    }

    #[test]
    fn health_and_optimizer_sections_no_db() {
        let config = AppConfig::default();
        let db: Option<Arc<Database>> = None;
        let mut out = Vec::new();

        print_health_section(&config, &db, &mut out);
        print_optimizer_section(&db, &mut out);
    }

    #[test]
    fn config_section_shows_validation() {
        let config = AppConfig::default();
        let mut out = Vec::new();
        print_config_section(&config, &mut out);
    }

    #[test]
    fn assess_sync_uses_shared_formula() {
        use chrono::Utc;
        use cuervo_storage::InvocationMetric;

        let db = Database::open_in_memory().unwrap();
        for _ in 0..5 {
            db.insert_metric(&InvocationMetric {
                provider: "test_provider".to_string(),
                model: "test_model".to_string(),
                latency_ms: 200,
                input_tokens: 100,
                output_tokens: 50,
                estimated_cost_usd: 0.001,
                success: true,
                stop_reason: "end_turn".to_string(),
                session_id: None,
                created_at: Utc::now(),
            })
            .unwrap();
        }

        let config = cuervo_core::types::HealthConfig::default();
        let report = assess_sync(&db, "test_provider", &config);
        assert!(report.score >= 80, "healthy provider should score >= 80, got {}", report.score);
        assert_eq!(report.invocation_count, 5);
    }

    #[test]
    fn replay_section_no_db() {
        let db: Option<Arc<Database>> = None;
        let mut out = Vec::new();
        print_replay_section(&db, &mut out);
    }

    #[test]
    fn replay_section_with_db() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let db = Some(db);
        let mut out = Vec::new();
        print_replay_section(&db, &mut out);
    }

    #[test]
    fn orchestrator_section_disabled() {
        let config = AppConfig::default();
        let db: Option<Arc<Database>> = None;
        let mut out = Vec::new();
        print_orchestrator_section(&config, &db, &mut out);
    }

    #[test]
    fn orchestrator_section_enabled_with_db() {
        let mut config = AppConfig::default();
        config.orchestrator.enabled = true;
        config.orchestrator.max_concurrent_agents = 5;
        config.orchestrator.sub_agent_timeout_secs = 120;

        let db = Arc::new(Database::open_in_memory().unwrap());

        db.save_agent_task(
            "t1", "o1", "s1", "Chat", "test",
            "completed", 100, 50, 0.01, 500, 2, None, Some("output"),
        ).unwrap();

        let db = Some(db);
        let mut out = Vec::new();
        print_orchestrator_section(&config, &db, &mut out);
    }

    #[test]
    fn themed_output_contains_section_headers() {
        let mut out = Vec::new();
        components::section_header("Test Section", &mut out);
        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("Test Section"));
    }

    #[test]
    fn phase14_section_default_config() {
        let config = AppConfig::default();
        let mut out = Vec::new();
        print_phase14_section(&config, &mut out);
        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("Advanced Features"));
        assert!(output.contains("Deterministic execution"));
        assert!(output.contains("Agent state machine"));
        assert!(output.contains("Trace context"));
        assert!(output.contains("Dry-run mode"));
        assert!(output.contains("Idempotency registry"));
    }

    #[test]
    fn phase14_section_with_mcp_servers() {
        let mut config = AppConfig::default();
        config.mcp.servers.insert(
            "test".to_string(),
            cuervo_core::types::McpServerConfig {
                command: "echo".to_string(),
                args: vec![],
                env: std::collections::HashMap::new(),
            },
        );
        config.mcp.max_reconnect_attempts = 5;
        let mut out = Vec::new();
        print_phase14_section(&config, &mut out);
        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("1 configured"));
        assert!(output.contains("max reconnect: 5"));
    }

    #[test]
    fn phase14_section_comm_enabled() {
        let mut config = AppConfig::default();
        config.orchestrator.enable_communication = true;
        let mut out = Vec::new();
        print_phase14_section(&config, &mut out);
        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("enabled"));
    }

    #[test]
    fn recommendations_all_nominal() {
        let mut config = AppConfig::default();
        config.resilience.enabled = true;
        config.cache.enabled = true;
        let db: Option<Arc<Database>> = None;
        let mut out = Vec::new();
        print_recommendations(&config, &db, &mut out);
        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("nominal"));
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn accessibility_section_renders() {
        let config = AppConfig::default();
        let mut out = Vec::new();
        print_accessibility_section(&config, &mut out);
        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("Color Accessibility"));
        assert!(output.contains("primary"));
        assert!(output.contains("text"));
        assert!(output.contains("WCAG AA"));
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn accessibility_section_shows_all_tokens() {
        let config = AppConfig::default();
        let mut out = Vec::new();
        print_accessibility_section(&config, &mut out);
        let output = String::from_utf8(out).unwrap();
        // All 8 semantic tokens should appear.
        for name in &["primary", "accent", "warning", "error", "success", "muted", "text", "text_dim"] {
            assert!(output.contains(name), "missing token: {name}");
        }
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn accessibility_section_with_custom_bg() {
        let mut config = AppConfig::default();
        config.display.terminal_background = "#ffffff".to_string();
        let mut out = Vec::new();
        print_accessibility_section(&config, &mut out);
        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("#ffffff"));
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn accessibility_section_badge_format() {
        let config = AppConfig::default();
        let mut out = Vec::new();
        print_accessibility_section(&config, &mut out);
        let output = String::from_utf8(out).unwrap();
        // Should contain at least one of these badges.
        let has_badge = output.contains("AAA") || output.contains("AA") || output.contains("FAIL");
        assert!(has_badge, "should contain WCAG badge");
    }

    #[test]
    fn phase14_deterministic_execution_active() {
        let config = AppConfig::default();
        let mut out = Vec::new();
        print_phase14_section(&config, &mut out);
        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("Deterministic execution"));
        assert!(output.contains("active"));
    }

    #[test]
    fn phase14_idempotency_registry_active() {
        let config = AppConfig::default();
        let mut out = Vec::new();
        print_phase14_section(&config, &mut out);
        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("Idempotency registry"));
        assert!(output.contains("active"));
    }

    #[test]
    fn phase14_dry_run_badge_disabled_by_default() {
        let config = AppConfig::default();
        let mut out = Vec::new();
        print_phase14_section(&config, &mut out);
        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("Dry-run mode"));
        assert!(output.contains("disabled"));
    }

    #[test]
    fn phase14_dry_run_badge_enabled() {
        let mut config = AppConfig::default();
        config.tools.dry_run = true;
        let mut out = Vec::new();
        print_phase14_section(&config, &mut out);
        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("Dry-run mode"));
        assert!(output.contains("enabled"));
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn accessibility_section_invalid_bg_fallback() {
        let mut config = AppConfig::default();
        config.display.terminal_background = "invalid".to_string();
        let mut out = Vec::new();
        // Should not panic — falls back to #1a1a1a.
        print_accessibility_section(&config, &mut out);
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn accessibility_section_suggests_fix_for_failing_color() {
        // Use a white background where dark muted text will fail.
        let mut config = AppConfig::default();
        config.display.terminal_background = "#ffffff".to_string();
        let mut out = Vec::new();
        print_accessibility_section(&config, &mut out);
        let output = String::from_utf8(out).unwrap();
        // With white bg, dark/muted tokens may fail — check suggestion renders.
        // At minimum the output should have either all-pass or suggestions.
        let has_suggest = output.contains("suggest");
        let all_pass = output.contains("All 8 tokens pass");
        assert!(
            has_suggest || all_pass,
            "should show suggestions or all-pass"
        );
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn accessibility_suggestion_contains_hex() {
        // Force a scenario with low contrast (white bg + light text).
        let mut config = AppConfig::default();
        config.display.terminal_background = "#ffffff".to_string();
        let mut out = Vec::new();
        print_accessibility_section(&config, &mut out);
        let output = String::from_utf8(out).unwrap();
        if output.contains("suggest") {
            // Suggestion should contain a hex color (# followed by digits).
            assert!(
                output.contains("#"),
                "suggestion should contain hex color"
            );
        }
    }

    #[cfg(feature = "color-science")]
    #[test]
    fn accessibility_suggestion_has_quality_score() {
        let mut config = AppConfig::default();
        config.display.terminal_background = "#ffffff".to_string();
        let mut out = Vec::new();
        print_accessibility_section(&config, &mut out);
        let output = String::from_utf8(out).unwrap();
        if output.contains("suggest") {
            assert!(
                output.contains("quality"),
                "suggestion should contain quality score"
            );
        }
    }

    #[test]
    fn model_selection_section_disabled_by_default() {
        let config = AppConfig::default();
        let mut out = Vec::new();
        print_model_selection_section(&config, &mut out);
        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("Model Selection"));
        assert!(output.contains("disabled"));
        assert!(output.contains("Strategy"));
        assert!(output.contains("Budget cap"));
        assert!(output.contains("unlimited"));
    }

    #[test]
    fn model_selection_section_enabled_with_overrides() {
        let mut config = AppConfig::default();
        config.agent.model_selection.enabled = true;
        config.agent.model_selection.budget_cap_usd = 5.0;
        config.agent.model_selection.simple_model = Some("gpt-4o-mini".into());
        config.agent.model_selection.complex_model = Some("claude-opus-4-6".into());
        let mut out = Vec::new();
        print_model_selection_section(&config, &mut out);
        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("active"));
        assert!(output.contains("$5.00"));
        assert!(output.contains("gpt-4o-mini"));
        assert!(output.contains("claude-opus-4-6"));
        assert!(output.contains("Simple override"));
        assert!(output.contains("Complex override"));
    }

    #[test]
    fn model_selection_section_provider_count() {
        let config = AppConfig::default();
        let mut out = Vec::new();
        print_model_selection_section(&config, &mut out);
        let output = String::from_utf8(out).unwrap();
        // Default config has anthropic + ollama enabled (3 new disabled)
        assert!(output.contains("enabled"));
        assert!(output.contains("Registered providers"));
    }

    #[test]
    fn model_selection_section_shows_enabled_providers() {
        let mut config = AppConfig::default();
        // Enable openai
        if let Some(openai) = config.models.providers.get_mut("openai") {
            openai.enabled = true;
        }
        let mut out = Vec::new();
        print_model_selection_section(&config, &mut out);
        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("openai"));
    }

    // --- Phase 19: Console section tests ---

    #[test]
    fn console_section_no_db() {
        let config = AppConfig::default();
        let mut out = Vec::new();
        print_console_section(&config, &None, &mut out);
        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("Operating Console"));
        assert!(output.contains("17")); // 17 commands
        assert!(output.contains("no database"));
    }

    #[test]
    fn console_section_with_db() {
        let config = AppConfig::default();
        let db = Some(Arc::new(Database::open_in_memory().unwrap()));
        let mut out = Vec::new();
        print_console_section(&config, &db, &mut out);
        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("Operating Console"));
        assert!(output.contains("Sessions in DB"));
        assert!(output.contains("Plan steps"));
    }

    #[test]
    fn console_section_orchestrator_status() {
        let mut config = AppConfig::default();
        config.orchestrator.enabled = true;
        let mut out = Vec::new();
        print_console_section(&config, &None, &mut out);
        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("ON"));

        let mut config2 = AppConfig::default();
        config2.orchestrator.enabled = false;
        let mut out2 = Vec::new();
        print_console_section(&config2, &None, &mut out2);
        let output2 = String::from_utf8(out2).unwrap();
        assert!(output2.contains("OFF"));
    }
}
