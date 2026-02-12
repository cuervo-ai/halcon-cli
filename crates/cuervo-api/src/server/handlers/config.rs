use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;

use crate::server::state::AppState;
use crate::types::config::*;
use crate::types::ws::WsServerEvent;

/// GET /api/v1/system/config — returns the full runtime config as DTO.
pub async fn get_config(
    State(state): State<AppState>,
) -> Result<Json<RuntimeConfigResponse>, StatusCode> {
    let config = state.config.read().await;
    Ok(Json(config_to_dto(&config)))
}

/// PUT /api/v1/system/config — applies a partial update and returns the new config.
pub async fn update_config(
    State(state): State<AppState>,
    Json(update): Json<UpdateConfigRequest>,
) -> Result<Json<RuntimeConfigResponse>, (StatusCode, Json<serde_json::Value>)> {
    let mut config = state.config.write().await;
    let changed_sections = apply_dto_update(&mut config, &update);

    // Validate after applying.
    let dto = config_to_dto(&config);
    let issues = validate_config_dto(&dto);
    if !issues.is_empty() {
        let error_fields: Vec<_> = issues.iter().map(|i| i.message.clone()).collect();
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "validation_failed",
                "issues": error_fields,
            })),
        ));
    }

    // Broadcast change events.
    for section in changed_sections {
        state.broadcast(WsServerEvent::ConfigChanged { section });
    }

    Ok(Json(dto))
}

/// Convert an `AppConfig` (cuervo-core) to the API DTO.
pub fn config_to_dto(
    config: &cuervo_core::types::AppConfig,
) -> RuntimeConfigResponse {
    let providers = config
        .models
        .providers
        .iter()
        .map(|(name, p)| {
            (
                name.clone(),
                ProviderConfigDto {
                    enabled: p.enabled,
                    api_base: p.api_base.clone(),
                    api_key_env: p.api_key_env.clone(),
                    default_model: p.default_model.clone(),
                    http: HttpConfigDto {
                        connect_timeout_secs: p.http.connect_timeout_secs,
                        request_timeout_secs: p.http.request_timeout_secs,
                        max_retries: p.http.max_retries,
                        retry_base_delay_ms: p.http.retry_base_delay_ms,
                    },
                },
            )
        })
        .collect();

    RuntimeConfigResponse {
        general: GeneralConfigDto {
            default_provider: config.general.default_provider.clone(),
            default_model: config.general.default_model.clone(),
            temperature: config.general.temperature,
            max_tokens: config.general.max_tokens,
        },
        providers,
        agent_limits: AgentLimitsDto {
            max_rounds: config.agent.limits.max_rounds,
            max_total_tokens: config.agent.limits.max_total_tokens,
            max_duration_secs: config.agent.limits.max_duration_secs,
            tool_timeout_secs: config.agent.limits.tool_timeout_secs,
            provider_timeout_secs: config.agent.limits.provider_timeout_secs,
            max_parallel_tools: config.agent.limits.max_parallel_tools,
            max_tool_output_chars: config.agent.limits.max_tool_output_chars,
        },
        routing: RoutingConfigDto {
            strategy: config.agent.routing.strategy.clone(),
            mode: config.agent.routing.mode.clone(),
            fallback_models: config.agent.routing.fallback_models.clone(),
            max_retries: config.agent.routing.max_retries,
        },
        tools: ToolsConfigDto {
            confirm_destructive: config.tools.confirm_destructive,
            timeout_secs: config.tools.timeout_secs,
            allowed_directories: config
                .tools
                .allowed_directories
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
            blocked_patterns: config.tools.blocked_patterns.clone(),
            dry_run: config.tools.dry_run,
            sandbox: SandboxConfigDto {
                enabled: config.tools.sandbox.enabled,
                max_output_bytes: config.tools.sandbox.max_output_bytes,
                max_memory_mb: config.tools.sandbox.max_memory_mb,
                max_cpu_secs: config.tools.sandbox.max_cpu_secs,
                max_file_size_bytes: config.tools.sandbox.max_file_size_bytes,
            },
        },
        security: SecurityConfigDto {
            pii_detection: config.security.pii_detection,
            pii_action: config.security.pii_action.clone(),
            audit_enabled: config.security.audit_enabled,
            guardrails_enabled: config.security.guardrails.enabled,
            guardrails_builtins: config.security.guardrails.builtins,
            tbac_enabled: config.security.tbac_enabled,
        },
        memory: MemoryConfigDto {
            enabled: config.memory.enabled,
            max_entries: config.memory.max_entries,
            auto_summarize: config.memory.auto_summarize,
            episodic: config.memory.episodic,
            decay_half_life_days: config.memory.decay_half_life_days,
        },
        resilience: ResilienceConfigDto {
            enabled: config.resilience.enabled,
            circuit_breaker: CircuitBreakerConfigDto {
                failure_threshold: config.resilience.circuit_breaker.failure_threshold,
                window_secs: config.resilience.circuit_breaker.window_secs,
                open_duration_secs: config.resilience.circuit_breaker.open_duration_secs,
                half_open_probes: config.resilience.circuit_breaker.half_open_probes,
            },
            health: HealthConfigDto {
                window_minutes: config.resilience.health.window_minutes,
                degraded_threshold: config.resilience.health.degraded_threshold,
                unhealthy_threshold: config.resilience.health.unhealthy_threshold,
            },
            backpressure: BackpressureConfigDto {
                max_concurrent_per_provider: config
                    .resilience
                    .backpressure
                    .max_concurrent_per_provider,
                queue_timeout_secs: config.resilience.backpressure.queue_timeout_secs,
            },
        },
    }
}

/// Apply a partial update to an `AppConfig`. Returns the list of section names that changed.
pub fn apply_dto_update(
    config: &mut cuervo_core::types::AppConfig,
    update: &UpdateConfigRequest,
) -> Vec<String> {
    let mut changed = Vec::new();

    if let Some(ref g) = update.general {
        config.general.default_provider = g.default_provider.clone();
        config.general.default_model = g.default_model.clone();
        config.general.temperature = g.temperature;
        config.general.max_tokens = g.max_tokens;
        changed.push("general".into());
    }

    if let Some(ref providers) = update.providers {
        for (name, dto) in providers {
            let entry = config
                .models
                .providers
                .entry(name.clone())
                .or_insert_with(|| cuervo_core::types::ProviderConfig {
                    enabled: false,
                    api_base: None,
                    api_key_env: None,
                    default_model: None,
                    http: Default::default(),
                    oauth: None,
                    extra: Default::default(),
                });
            entry.enabled = dto.enabled;
            entry.api_base = dto.api_base.clone();
            entry.api_key_env = dto.api_key_env.clone();
            entry.default_model = dto.default_model.clone();
            entry.http.connect_timeout_secs = dto.http.connect_timeout_secs;
            entry.http.request_timeout_secs = dto.http.request_timeout_secs;
            entry.http.max_retries = dto.http.max_retries;
            entry.http.retry_base_delay_ms = dto.http.retry_base_delay_ms;
        }
        changed.push("providers".into());
    }

    if let Some(ref l) = update.agent_limits {
        config.agent.limits.max_rounds = l.max_rounds;
        config.agent.limits.max_total_tokens = l.max_total_tokens;
        config.agent.limits.max_duration_secs = l.max_duration_secs;
        config.agent.limits.tool_timeout_secs = l.tool_timeout_secs;
        config.agent.limits.provider_timeout_secs = l.provider_timeout_secs;
        config.agent.limits.max_parallel_tools = l.max_parallel_tools;
        config.agent.limits.max_tool_output_chars = l.max_tool_output_chars;
        changed.push("agent_limits".into());
    }

    if let Some(ref r) = update.routing {
        config.agent.routing.strategy = r.strategy.clone();
        config.agent.routing.mode = r.mode.clone();
        config.agent.routing.fallback_models = r.fallback_models.clone();
        config.agent.routing.max_retries = r.max_retries;
        changed.push("routing".into());
    }

    if let Some(ref t) = update.tools {
        config.tools.confirm_destructive = t.confirm_destructive;
        config.tools.timeout_secs = t.timeout_secs;
        config.tools.allowed_directories =
            t.allowed_directories.iter().map(Into::into).collect();
        config.tools.blocked_patterns = t.blocked_patterns.clone();
        config.tools.dry_run = t.dry_run;
        config.tools.sandbox.enabled = t.sandbox.enabled;
        config.tools.sandbox.max_output_bytes = t.sandbox.max_output_bytes;
        config.tools.sandbox.max_memory_mb = t.sandbox.max_memory_mb;
        config.tools.sandbox.max_cpu_secs = t.sandbox.max_cpu_secs;
        config.tools.sandbox.max_file_size_bytes = t.sandbox.max_file_size_bytes;
        changed.push("tools".into());
    }

    if let Some(ref s) = update.security {
        config.security.pii_detection = s.pii_detection;
        config.security.pii_action = s.pii_action.clone();
        config.security.audit_enabled = s.audit_enabled;
        config.security.guardrails.enabled = s.guardrails_enabled;
        config.security.guardrails.builtins = s.guardrails_builtins;
        config.security.tbac_enabled = s.tbac_enabled;
        changed.push("security".into());
    }

    if let Some(ref m) = update.memory {
        config.memory.enabled = m.enabled;
        config.memory.max_entries = m.max_entries;
        config.memory.auto_summarize = m.auto_summarize;
        config.memory.episodic = m.episodic;
        config.memory.decay_half_life_days = m.decay_half_life_days;
        changed.push("memory".into());
    }

    if let Some(ref r) = update.resilience {
        config.resilience.enabled = r.enabled;
        config.resilience.circuit_breaker.failure_threshold = r.circuit_breaker.failure_threshold;
        config.resilience.circuit_breaker.window_secs = r.circuit_breaker.window_secs;
        config.resilience.circuit_breaker.open_duration_secs =
            r.circuit_breaker.open_duration_secs;
        config.resilience.circuit_breaker.half_open_probes = r.circuit_breaker.half_open_probes;
        config.resilience.health.window_minutes = r.health.window_minutes;
        config.resilience.health.degraded_threshold = r.health.degraded_threshold;
        config.resilience.health.unhealthy_threshold = r.health.unhealthy_threshold;
        config.resilience.backpressure.max_concurrent_per_provider =
            r.backpressure.max_concurrent_per_provider;
        config.resilience.backpressure.queue_timeout_secs = r.backpressure.queue_timeout_secs;
        changed.push("resilience".into());
    }

    changed
}
