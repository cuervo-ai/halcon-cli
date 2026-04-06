//! SOTA 2026++ Universal Project Intelligence Engine.
//!
//! Five-wave parallel orchestrator implementing Phases 103-120:
//!
//! ```text
//! Wave 0  │ root_detection (sync) + cache probe
//! Wave R  │ system_profile ─────────────────────────┐
//!         │ tool_versions ──────────────────────────┤
//!         │ ide_context ────────────────────────────┤  Resource Discovery
//!         │ agent_capabilities ─────────────────────┤  (tokio::join!)
//!         │ runtime_profile ────────────────────────┘
//! Wave 1  │ filesystem_scanner ─┐
//!         │ type_detector      ─┘  Project Structure
//! Wave 2  │ metadata_reader ─────┐
//!         │ git_intelligence ────┤
//!         │ cicd_detector ───────┤  Deep Analysis + Language Intel
//!         │ docker_detector ─────┤  (7 parallel tools)
//!         │ security_scanner ────┤
//!         │ test_coverage_est ───┤
//!         │ language_intel ──────┘  Ph 110-112 (languages, monorepo, scale)
//! Wave 3  │ dependency_analyzer ─┐
//!         │ architecture_detect ─┤  Risk + Architecture + Distributed Intel
//!         │ arch_intel ──────────┘  Ph 113 (microservices, events, mesh, K8s)
//! Wave 4  │ health_score (Ph 107)          ┐
//!         │ agent_readiness (Ph 107)       ┤
//!         │ env_compat (Ph 107)            ┤  Synthesis (pure functions)
//!         │ arch_quality (Ph 117)          ┤
//!         │ scalability (Ph 117)           ┤
//!         │ maintainability (Ph 117)       ┤
//!         │ technical_debt (Ph 117)        ┤
//!         │ dev_ex (Ph 117)                ┤
//!         │ ai_readiness (Ph 117)          ┤
//!         │ distributed_maturity (Ph 117)  ┤
//!         │ agent_mode_suggestion (Ph 119) ┘
//! ```
//!
//! All wave outputs merge into [`ProjectContext`] via the sparse
//! [`ToolOutput::merge_into`] accumulator pattern.

pub mod architecture_intelligence;
pub mod halcon_md;
pub mod language_intelligence;
pub mod resource_intelligence;
pub mod tools;

use std::path::PathBuf;
use std::time::Instant;

use sha2::{Digest, Sha256};

use crate::tui::events::UiEvent;
use architecture_intelligence::{
    architecture_intelligence_scanner, compute_ai_readiness_score,
    compute_architecture_quality_score, compute_dev_ex_score, compute_distributed_maturity_score,
    compute_maintainability_score, compute_scalability_score, compute_technical_debt_score,
    suggest_agent_configuration,
};
use language_intelligence::language_intelligence_scanner;
use resource_intelligence::{
    agent_capabilities_scanner, compute_agent_readiness_score,
    compute_environment_compatibility_score, ide_context_scanner, runtime_profile_scanner,
    system_profile_scanner, tool_versions_scanner,
};
use tools::{
    // Phase 122: AI context file discovery
    ai_context_file_scanner,
    architecture_detector,
    // Wave 2
    cicd_detector,
    // Wave 3
    dependency_analyzer,
    docker_detector,
    // Wave 1
    filesystem_scanner,
    // Wave 0
    find_project_root,
    git_intelligence,
    // Wave 4
    health_score_calculator,
    metadata_reader,
    security_scanner,
    test_coverage_estimator,
    type_detector,
    ProjectContext,
};

// ─── Public entry point ───────────────────────────────────────────────────────

/// Analyze the project rooted at `cwd` and emit progress + completion events.
///
/// Same public API as before — all call sites in app.rs remain unchanged.
/// Internally uses 5-wave parallel execution with resource discovery (Ph 103–109).
pub async fn analyze_and_emit(tx: crate::tui::events::BoundedUiSender, cwd: PathBuf) {
    macro_rules! info {
        ($msg:expr) => {
            tx.send(UiEvent::Info($msg));
        };
    }

    let started = Instant::now();
    info!("[init] ⟳ Iniciando Context-Aware System Bootstrap Engine…".to_string());

    // ── Wave 0: Find project root ─────────────────────────────────────────────
    let root = find_project_root(&cwd).unwrap_or_else(|| cwd.clone());
    info!(format!("[init] ◈ Raíz: {}", root.display()));

    // ── Check cache ───────────────────────────────────────────────────────────
    let cache_key = compute_cache_key(&root);
    if let Some(cached) = try_load_cache(&cache_key).await {
        info!(format!(
            "[init] ◈ Cache hit — análisis previo < 24h (health: {}/100 · agent: {}/100)",
            cached.health_score, cached.agent_readiness_score
        ));
        let preview = halcon_md::generate(&cached);
        let save_path = root
            .join(".halcon")
            .join("HALCON.md")
            .to_string_lossy()
            .to_string();
        tx.send(UiEvent::ProjectHealthCalculated {
            score: cached.health_score,
            issues: cached.health_issues.clone(),
            recommendations: cached.health_recommendations.clone(),
        });
        tx.send(UiEvent::ProjectAnalysisComplete {
            root: root.to_string_lossy().to_string(),
            project_type: cached.project_type.clone(),
            package_name: cached.package_name.clone(),
            has_git: cached.branch.is_some(),
            preview,
            save_path,
        });
        return;
    }

    let mut ctx = ProjectContext {
        root: root.to_string_lossy().to_string(),
        ..Default::default()
    };
    let mut tools_run: u32 = 0;

    // ── Wave R: Resource Discovery (5 probes in parallel) ────────────────────
    info!("[init] ⟳ Wave R: Resource discovery — sistema, IDE, agente…".to_string());
    let res_started = Instant::now();
    let root_r = root.clone();
    let root_r2 = root.clone();
    let (sys_out, tool_ver_out, ide_out, agent_out, runtime_out) = tokio::join!(
        system_profile_scanner(),
        tool_versions_scanner(),
        ide_context_scanner(),
        agent_capabilities_scanner(&root_r),
        runtime_profile_scanner(&root_r2),
    );
    sys_out.merge_into(&mut ctx);
    tool_ver_out.merge_into(&mut ctx);
    ide_out.merge_into(&mut ctx);
    agent_out.merge_into(&mut ctx);
    runtime_out.merge_into(&mut ctx);
    tools_run += 5;
    let res_ms = res_started.elapsed().as_millis() as u64;

    // Emit resource discovery summary
    info!(format!(
        "[init] ◈ Sistema: {} · {} cores · {}MB RAM",
        ctx.sys_os,
        ctx.sys_cpu_cores,
        ctx.sys_ram_mb / 1024
    ));
    if let Some(ref ide) = ctx.ide_detected {
        let lsp = if ctx.ide_lsp_connected {
            " · LSP✓"
        } else {
            ""
        };
        info!(format!("[init] ◈ IDE: {ide}{lsp}"));
    }
    if let Some(ref model) = ctx.agent_model_name {
        let tier = ctx.agent_model_tier.as_deref().unwrap_or("Balanced");
        info!(format!("[init] ◈ Agente: {model} [{tier}]"));
    }
    if !ctx.agent_mcp_servers.is_empty() {
        info!(format!(
            "[init] ◈ MCP: {} servers",
            ctx.agent_mcp_servers.len()
        ));
    }
    if ctx.agent_hicon_active {
        info!("[init] ◈ HICON: activo ✓".to_string());
    }

    // ── Wave 1: Filesystem scan + type detection + AI context files ───────────
    info!("[init] ⟳ Wave 1: Escaneando estructura y detectando tipo…".to_string());
    let (fs_out, type_out, ai_ctx_out) = tokio::join!(
        filesystem_scanner(&root),
        async { type_detector(&root) },
        ai_context_file_scanner(&root),
    );
    fs_out.merge_into(&mut ctx);
    type_out.merge_into(&mut ctx);
    ai_ctx_out.merge_into(&mut ctx);
    tools_run += 3;

    info!(format!(
        "[init] ◈ Tipo: {} · {} dirs · {} archivos",
        ctx.project_type,
        ctx.top_dirs.len(),
        ctx.files_scanned
    ));

    // ── Wave 2: Deep analysis (7 tools in parallel, +language_intelligence) ──
    info!("[init] ⟳ Wave 2: Análisis profundo del entorno + lenguajes…".to_string());
    let project_type_clone = ctx.project_type.clone();
    let (meta_out, git_out, ci_out, docker_out, sec_out, test_out, lang_out) = tokio::join!(
        metadata_reader(&root, &project_type_clone),
        git_intelligence(&root),
        cicd_detector(&root),
        docker_detector(&root),
        security_scanner(&root),
        test_coverage_estimator(&root, &project_type_clone),
        language_intelligence_scanner(&root),
    );
    meta_out.merge_into(&mut ctx);
    git_out.merge_into(&mut ctx);
    ci_out.merge_into(&mut ctx);
    docker_out.merge_into(&mut ctx);
    sec_out.merge_into(&mut ctx);
    test_out.merge_into(&mut ctx);
    lang_out.merge_into(&mut ctx);
    tools_run += 7;

    // Emit language intelligence summary
    if !ctx.primary_language.is_empty() {
        let polyglot = if ctx.is_polyglot { " (poliglota)" } else { "" };
        let scale = if ctx.project_scale.is_empty() {
            ""
        } else {
            &ctx.project_scale
        };
        info!(format!(
            "[init] ◈ Lenguaje: {}{polyglot} · Escala: {scale} · {} archivos",
            ctx.primary_language, ctx.total_file_count
        ));
    }
    if let Some(ref fw) = ctx.frontend_framework {
        info!(format!("[init] ◈ Frontend: {fw}"));
    }
    if let Some(ref fw) = ctx.mobile_framework {
        info!(format!("[init] ◈ Mobile: {fw}"));
    }
    if ctx.is_monorepo {
        let tool = ctx.monorepo_tool.as_deref().unwrap_or("monorepo");
        info!(format!(
            "[init] ◈ Monorepo: {tool} · {} sub-proyectos",
            ctx.sub_project_count
        ));
    }

    if let Some(ref n) = ctx.package_name {
        let ver = ctx.version.as_deref().unwrap_or("?");
        info!(format!("[init] ◈ Paquete: {n} v{ver}"));
    }
    if !ctx.members.is_empty() {
        info!(format!(
            "[init] ◈ Workspace: {} crates/paquetes",
            ctx.members.len()
        ));
    }
    if let Some(ref b) = ctx.branch {
        info!(format!(
            "[init] ◈ Git: branch={b}, commits={}",
            ctx.total_commits.unwrap_or(0)
        ));
    }
    if ctx.has_security_policy {
        info!("[init] ◈ Security policy: ✓".to_string());
    }
    if ctx.has_tests {
        let cov = ctx
            .test_coverage_est
            .map(|c| format!(" ~{c}%"))
            .unwrap_or_default();
        info!(format!("[init] ◈ Tests detectados{cov}"));
    }

    // ── Wave 3: Architecture + dependencies + distributed detection ───────────
    info!("[init] ⟳ Wave 3: Arquitectura + dependencias + sistemas distribuidos…".to_string());
    let members_clone = ctx.members.clone();
    let project_type_clone2 = ctx.project_type.clone();
    let (dep_out, arch_out, dist_out) = tokio::join!(
        dependency_analyzer(&root, &project_type_clone2),
        architecture_detector(&root, &members_clone),
        architecture_intelligence_scanner(&root),
    );
    dep_out.merge_into(&mut ctx);
    arch_out.merge_into(&mut ctx);
    dist_out.merge_into(&mut ctx);
    tools_run += 3;

    if let Some(ref style) = ctx.architecture_style {
        info!(format!("[init] ◈ Arquitectura: {style}"));
    }
    if !ctx.architecture_patterns.is_empty() {
        info!(format!(
            "[init] ◈ Patrones: {}",
            ctx.architecture_patterns.join(", ")
        ));
    }
    if ctx.has_message_broker {
        let broker = ctx.message_broker_type.as_deref().unwrap_or("broker");
        info!(format!("[init] ◈ Message broker: {broker}"));
    }
    if ctx.has_observability_stack {
        info!("[init] ◈ Observability stack: ✓".to_string());
    }

    // ── Wave 4: Synthesis — all scores (pure functions, Phases 107 + 117-119) ─
    let (score, issues, recommendations) = health_score_calculator(&ctx);
    ctx.health_score = score;
    ctx.health_issues = issues.clone();
    ctx.health_recommendations = recommendations.clone();
    ctx.agent_readiness_score = compute_agent_readiness_score(&ctx);
    ctx.environment_compatibility_score = compute_environment_compatibility_score(&ctx);

    // Phase 117: Advanced composite scores
    ctx.architecture_quality_score = compute_architecture_quality_score(&ctx);
    ctx.scalability_score = compute_scalability_score(&ctx);
    ctx.maintainability_score = compute_maintainability_score(&ctx);
    ctx.technical_debt_score = compute_technical_debt_score(&ctx);
    ctx.dev_ex_score = compute_dev_ex_score(&ctx);
    ctx.ai_readiness_score = compute_ai_readiness_score(&ctx);
    ctx.distributed_maturity_score = compute_distributed_maturity_score(&ctx);

    // Phase 119: Auto-mode suggestion
    let suggestion = suggest_agent_configuration(&ctx);
    ctx.suggested_model_tier = suggestion.model_tier;
    ctx.suggested_agent_flags = suggestion.agent_flags.clone();
    ctx.suggested_planning_strategy = suggestion.planning_strategy;
    ctx.activate_reasoning_deep = suggestion.activate_reasoning_deep;
    ctx.activate_multimodal_for_init = suggestion.activate_multimodal;
    ctx.use_fast_mode = suggestion.use_fast_mode;
    ctx.agent_mode_rationale = Some(suggestion.rationale.clone());

    ctx.tools_run = tools_run + 10; // +10 for score calculators + suggestion
    ctx.analysis_duration_ms = started.elapsed().as_millis() as u64;
    ctx.resource_detection_time_ms = res_ms;

    info!(format!(
        "[init] ◈ Salud: {}/100 · Agente: {}/100 · Entorno: {}/100 ({} issues)",
        ctx.health_score,
        ctx.agent_readiness_score,
        ctx.environment_compatibility_score,
        issues.len()
    ));
    info!(format!(
        "[init] ◈ Calidad: {}/100 · Deuda: {}/100 · DevEx: {}/100 · IA: {}/100",
        ctx.architecture_quality_score,
        ctx.technical_debt_score,
        ctx.dev_ex_score,
        ctx.ai_readiness_score
    ));
    if !suggestion.agent_flags.is_empty() {
        info!(format!(
            "[init] ◈ Sugerencia: halcon chat {} ({})",
            suggestion.agent_flags.join(" "),
            suggestion.rationale
        ));
    }

    // ── Cache: persist result asynchronously ──────────────────────────────────
    let ctx_for_cache = ctx.clone();
    let cache_key_clone = cache_key.clone();
    tokio::spawn(async move {
        save_cache(&cache_key_clone, &ctx_for_cache).await;
    });

    // ── Generate HALCON.md ────────────────────────────────────────────────────
    info!("[init] ⟳ Generando HALCON.md contextual…".to_string());
    let preview = halcon_md::generate(&ctx);
    let save_path = root
        .join(".halcon")
        .join("HALCON.md")
        .to_string_lossy()
        .to_string();

    info!("[init] ✓ Bootstrap completo — análisis contextual listo".to_string());

    tx.send(UiEvent::ProjectHealthCalculated {
        score,
        issues: issues.clone(),
        recommendations: recommendations.clone(),
    });

    let has_git = ctx.branch.is_some();
    tx.send(UiEvent::ProjectAnalysisComplete {
        root: ctx.root.clone(),
        project_type: ctx.project_type.clone(),
        package_name: ctx.package_name.clone(),
        has_git,
        preview,
        save_path,
    });
}

// ─── Cache ────────────────────────────────────────────────────────────────────

/// Bump this when ProjectContext fields change to automatically invalidate stale caches.
const CACHE_SCHEMA_VERSION: &str = "v3";

fn compute_cache_key(root: &std::path::Path) -> String {
    let mut hasher = Sha256::new();
    // Schema version prefix — changes here invalidate ALL existing caches
    hasher.update(CACHE_SCHEMA_VERSION.as_bytes());
    for fname in &["Cargo.toml", "package.json", "go.mod", "pyproject.toml"] {
        if let Ok(content) = std::fs::read(root.join(fname)) {
            hasher.update(&content);
        }
    }
    hasher.update(root.to_string_lossy().as_bytes());
    hex::encode(hasher.finalize())
}

fn cache_dir() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".halcon").join("project_cache"))
}

async fn try_load_cache(key: &str) -> Option<ProjectContext> {
    let dir = cache_dir()?;
    let path = dir.join(format!("{key}.json"));
    if let Ok(meta) = tokio::fs::metadata(&path).await {
        if let Ok(modified) = meta.modified() {
            let age = std::time::SystemTime::now()
                .duration_since(modified)
                .unwrap_or(std::time::Duration::MAX);
            if age > std::time::Duration::from_secs(24 * 3600) {
                return None;
            }
        }
    }
    let content = tokio::fs::read_to_string(&path).await.ok()?;
    serde_json::from_str(&content).ok()
}

async fn save_cache(key: &str, ctx: &ProjectContext) {
    let Some(dir) = cache_dir() else { return };
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        tracing::warn!("Failed to create project cache dir: {e}");
        return;
    }
    let path = dir.join(format!("{key}.json"));
    match serde_json::to_string_pretty(ctx) {
        Ok(json) => {
            if let Err(e) = tokio::fs::write(&path, json).await {
                tracing::warn!("Failed to write project cache: {e}");
            }
        }
        Err(e) => tracing::warn!("Failed to serialize project cache: {e}"),
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn analyze_and_emit_completes_on_rust_project() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"test-proj\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        analyze_and_emit(
            crate::tui::events::BoundedUiSender::new(tx),
            tmp.path().to_path_buf(),
        )
        .await;

        let mut got_complete = false;
        let mut got_health = false;
        let mut info_count = 0;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                UiEvent::Info(_) => info_count += 1,
                UiEvent::ProjectAnalysisComplete {
                    package_name,
                    preview,
                    ..
                } => {
                    got_complete = true;
                    assert_eq!(package_name, Some("test-proj".to_string()));
                    assert!(preview.contains("test-proj"));
                    assert!(preview.contains("HALCON"));
                }
                UiEvent::ProjectHealthCalculated { score, .. } => {
                    got_health = true;
                    assert!(score <= 100, "Health score must be 0-100");
                }
                _ => {}
            }
        }
        assert!(got_complete, "ProjectAnalysisComplete must be emitted");
        assert!(got_health, "ProjectHealthCalculated must be emitted");
        assert!(
            info_count >= 3,
            "Must emit at least 3 progress messages, got {info_count}"
        );
    }

    #[tokio::test]
    async fn no_panic_on_empty_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        analyze_and_emit(
            crate::tui::events::BoundedUiSender::new(tx),
            tmp.path().to_path_buf(),
        )
        .await;
        let mut events = vec![];
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        let has_complete = events
            .iter()
            .any(|e| matches!(e, UiEvent::ProjectAnalysisComplete { .. }));
        assert!(
            has_complete,
            "Must emit ProjectAnalysisComplete even for empty dir"
        );
    }

    #[tokio::test]
    async fn emits_resource_discovery_info_messages() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]\nname=\"x\"").unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        analyze_and_emit(
            crate::tui::events::BoundedUiSender::new(tx),
            tmp.path().to_path_buf(),
        )
        .await;
        let infos: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|ev| {
                if let UiEvent::Info(s) = ev {
                    Some(s)
                } else {
                    None
                }
            })
            .collect();
        // Must contain a Wave R resource info message
        let has_resource = infos
            .iter()
            .any(|s| s.contains("Wave R") || s.contains("Sistema") || s.contains("cores"));
        assert!(
            has_resource,
            "Must emit resource discovery info. Got: {infos:?}"
        );
    }

    #[tokio::test]
    async fn context_includes_agent_scores() {
        // Test via cache JSON round-trip: ProjectContext serializes scores
        let ctx = ProjectContext {
            agent_readiness_score: 75,
            environment_compatibility_score: 60,
            sys_cpu_cores: 8,
            sys_ram_mb: 16384,
            ..Default::default()
        };
        let json = serde_json::to_string(&ctx).unwrap();
        let parsed: ProjectContext = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.agent_readiness_score, 75);
        assert_eq!(parsed.environment_compatibility_score, 60);
        assert_eq!(parsed.sys_cpu_cores, 8);
        assert_eq!(parsed.sys_ram_mb, 16384);
    }

    #[test]
    fn compute_cache_key_deterministic() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]\nname=\"a\"").unwrap();
        let k1 = compute_cache_key(tmp.path());
        let k2 = compute_cache_key(tmp.path());
        assert_eq!(k1, k2);
        assert_eq!(k1.len(), 64);
    }

    #[test]
    fn compute_cache_key_differs_for_different_content() {
        let tmp1 = tempfile::tempdir().unwrap();
        let tmp2 = tempfile::tempdir().unwrap();
        std::fs::write(tmp1.path().join("Cargo.toml"), "[package]\nname=\"a\"").unwrap();
        std::fs::write(tmp2.path().join("Cargo.toml"), "[package]\nname=\"b\"").unwrap();
        assert_ne!(
            compute_cache_key(tmp1.path()),
            compute_cache_key(tmp2.path())
        );
    }

    #[tokio::test]
    async fn cache_round_trip_with_resource_fields() {
        let ctx = ProjectContext {
            package_name: Some("cached-proj".to_string()),
            health_score: 77,
            agent_readiness_score: 40,
            sys_os: "linux aarch64".to_string(),
            tool_git_version: Some("2.43.0".to_string()),
            ide_detected: Some("VS Code".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&ctx).unwrap();
        let parsed: ProjectContext = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.package_name, Some("cached-proj".to_string()));
        assert_eq!(parsed.health_score, 77);
        assert_eq!(parsed.agent_readiness_score, 40);
        assert_eq!(parsed.sys_os, "linux aarch64");
        assert_eq!(parsed.tool_git_version, Some("2.43.0".to_string()));
        assert_eq!(parsed.ide_detected, Some("VS Code".to_string()));
    }
}
