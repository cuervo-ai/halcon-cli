use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ── Top-level request / response ────────────────────────────────────

/// Full runtime configuration snapshot returned by the API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeConfigResponse {
    pub general: GeneralConfigDto,
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfigDto>,
    pub agent_limits: AgentLimitsDto,
    pub routing: RoutingConfigDto,
    pub tools: ToolsConfigDto,
    pub security: SecurityConfigDto,
    pub memory: MemoryConfigDto,
    pub resilience: ResilienceConfigDto,
}

/// Partial update request — all fields are optional.
/// Only provided sections are applied; `None` sections are left unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct UpdateConfigRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub general: Option<GeneralConfigDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub providers: Option<HashMap<String, ProviderConfigDto>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_limits: Option<AgentLimitsDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<RoutingConfigDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsConfigDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security: Option<SecurityConfigDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryConfigDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resilience: Option<ResilienceConfigDto>,
}

// ── Section DTOs ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GeneralConfigDto {
    pub default_provider: String,
    pub default_model: String,
    pub temperature: f32,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderConfigDto {
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default)]
    pub http: HttpConfigDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HttpConfigDto {
    pub connect_timeout_secs: u64,
    pub request_timeout_secs: u64,
    pub max_retries: u32,
    pub retry_base_delay_ms: u64,
}

impl Default for HttpConfigDto {
    fn default() -> Self {
        Self {
            connect_timeout_secs: 10,
            request_timeout_secs: 300,
            max_retries: 3,
            retry_base_delay_ms: 1000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentLimitsDto {
    pub max_rounds: usize,
    pub max_total_tokens: u32,
    pub max_duration_secs: u64,
    pub tool_timeout_secs: u64,
    pub provider_timeout_secs: u64,
    pub max_parallel_tools: usize,
    pub max_tool_output_chars: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoutingConfigDto {
    pub strategy: String,
    pub mode: String,
    pub fallback_models: Vec<String>,
    pub max_retries: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolsConfigDto {
    pub confirm_destructive: bool,
    pub timeout_secs: u64,
    pub allowed_directories: Vec<String>,
    pub blocked_patterns: Vec<String>,
    pub dry_run: bool,
    pub sandbox: SandboxConfigDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SandboxConfigDto {
    pub enabled: bool,
    pub max_output_bytes: usize,
    pub max_memory_mb: u64,
    pub max_cpu_secs: u64,
    pub max_file_size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SecurityConfigDto {
    pub pii_detection: bool,
    pub pii_action: String,
    pub audit_enabled: bool,
    pub guardrails_enabled: bool,
    pub guardrails_builtins: bool,
    pub tbac_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryConfigDto {
    pub enabled: bool,
    pub max_entries: u32,
    pub auto_summarize: bool,
    pub episodic: bool,
    pub decay_half_life_days: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResilienceConfigDto {
    pub enabled: bool,
    pub circuit_breaker: CircuitBreakerConfigDto,
    pub health: HealthConfigDto,
    pub backpressure: BackpressureConfigDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CircuitBreakerConfigDto {
    pub failure_threshold: u32,
    pub window_secs: u64,
    pub open_duration_secs: u64,
    pub half_open_probes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HealthConfigDto {
    pub window_minutes: u64,
    pub degraded_threshold: u32,
    pub unhealthy_threshold: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackpressureConfigDto {
    pub max_concurrent_per_provider: u32,
    pub queue_timeout_secs: u64,
}

// ── Validation ──────────────────────────────────────────────────────

/// A validation issue found in a config update.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConfigValidationIssue {
    pub field: String,
    pub message: String,
}

/// Validate a `RuntimeConfigResponse` and return any issues.
pub fn validate_config_dto(config: &RuntimeConfigResponse) -> Vec<ConfigValidationIssue> {
    let mut issues = Vec::new();

    if config.general.temperature < 0.0 || config.general.temperature > 2.0 {
        issues.push(ConfigValidationIssue {
            field: "general.temperature".into(),
            message: format!(
                "temperature {} out of range [0.0, 2.0]",
                config.general.temperature
            ),
        });
    }
    if config.general.max_tokens == 0 {
        issues.push(ConfigValidationIssue {
            field: "general.max_tokens".into(),
            message: "max_tokens must be > 0".into(),
        });
    }
    if config.tools.timeout_secs == 0 {
        issues.push(ConfigValidationIssue {
            field: "tools.timeout_secs".into(),
            message: "tool timeout must be > 0".into(),
        });
    }
    if config.agent_limits.max_rounds == 0 {
        issues.push(ConfigValidationIssue {
            field: "agent_limits.max_rounds".into(),
            message: "max_rounds must be > 0".into(),
        });
    }

    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> RuntimeConfigResponse {
        RuntimeConfigResponse {
            general: GeneralConfigDto {
                default_provider: "anthropic".into(),
                default_model: "claude-sonnet-4-5-20250929".into(),
                temperature: 0.7,
                max_tokens: 8192,
            },
            providers: {
                let mut m = HashMap::new();
                m.insert(
                    "anthropic".into(),
                    ProviderConfigDto {
                        enabled: true,
                        api_base: Some("https://api.anthropic.com".into()),
                        api_key_env: Some("ANTHROPIC_API_KEY".into()),
                        default_model: Some("claude-sonnet-4-5-20250929".into()),
                        http: HttpConfigDto::default(),
                    },
                );
                m
            },
            agent_limits: AgentLimitsDto {
                max_rounds: 25,
                max_total_tokens: 0,
                max_duration_secs: 0,
                tool_timeout_secs: 120,
                provider_timeout_secs: 300,
                max_parallel_tools: 10,
                max_tool_output_chars: 100_000,
            },
            routing: RoutingConfigDto {
                strategy: "balanced".into(),
                mode: "failover".into(),
                fallback_models: vec![],
                max_retries: 1,
            },
            tools: ToolsConfigDto {
                confirm_destructive: true,
                timeout_secs: 120,
                allowed_directories: vec![],
                blocked_patterns: vec!["**/.env".into()],
                dry_run: false,
                sandbox: SandboxConfigDto {
                    enabled: true,
                    max_output_bytes: 100_000,
                    max_memory_mb: 512,
                    max_cpu_secs: 60,
                    max_file_size_bytes: 50_000_000,
                },
            },
            security: SecurityConfigDto {
                pii_detection: true,
                pii_action: "warn".into(),
                audit_enabled: true,
                guardrails_enabled: true,
                guardrails_builtins: true,
                tbac_enabled: false,
            },
            memory: MemoryConfigDto {
                enabled: true,
                max_entries: 10000,
                auto_summarize: true,
                episodic: false,
                decay_half_life_days: 30.0,
            },
            resilience: ResilienceConfigDto {
                enabled: true,
                circuit_breaker: CircuitBreakerConfigDto {
                    failure_threshold: 5,
                    window_secs: 60,
                    open_duration_secs: 30,
                    half_open_probes: 2,
                },
                health: HealthConfigDto {
                    window_minutes: 60,
                    degraded_threshold: 50,
                    unhealthy_threshold: 30,
                },
                backpressure: BackpressureConfigDto {
                    max_concurrent_per_provider: 5,
                    queue_timeout_secs: 30,
                },
            },
        }
    }

    #[test]
    fn runtime_config_response_serde_roundtrip() {
        let config = sample_config();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: RuntimeConfigResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(config, parsed);
    }

    #[test]
    fn update_config_request_serde_roundtrip() {
        let req = UpdateConfigRequest {
            general: Some(GeneralConfigDto {
                default_provider: "openai".into(),
                default_model: "gpt-4o".into(),
                temperature: 0.5,
                max_tokens: 4096,
            }),
            ..Default::default()
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: UpdateConfigRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, parsed);
    }

    #[test]
    fn update_config_empty_roundtrip() {
        let req = UpdateConfigRequest::default();
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, "{}");
        let parsed: UpdateConfigRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, parsed);
    }

    #[test]
    fn general_config_dto_roundtrip() {
        let dto = GeneralConfigDto {
            default_provider: "deepseek".into(),
            default_model: "deepseek-chat".into(),
            temperature: 1.0,
            max_tokens: 2048,
        };
        let json = serde_json::to_string(&dto).unwrap();
        let parsed: GeneralConfigDto = serde_json::from_str(&json).unwrap();
        assert_eq!(dto, parsed);
    }

    #[test]
    fn provider_config_dto_roundtrip() {
        let dto = ProviderConfigDto {
            enabled: true,
            api_base: Some("https://api.openai.com/v1".into()),
            api_key_env: Some("OPENAI_API_KEY".into()),
            default_model: Some("gpt-4o".into()),
            http: HttpConfigDto {
                connect_timeout_secs: 15,
                request_timeout_secs: 120,
                max_retries: 5,
                retry_base_delay_ms: 500,
            },
        };
        let json = serde_json::to_string(&dto).unwrap();
        let parsed: ProviderConfigDto = serde_json::from_str(&json).unwrap();
        assert_eq!(dto, parsed);
    }

    #[test]
    fn provider_config_dto_minimal() {
        // Only required field
        let json = r#"{"enabled":false}"#;
        let parsed: ProviderConfigDto = serde_json::from_str(json).unwrap();
        assert!(!parsed.enabled);
        assert!(parsed.api_base.is_none());
        assert!(parsed.api_key_env.is_none());
        assert!(parsed.default_model.is_none());
    }

    #[test]
    fn http_config_dto_roundtrip() {
        let dto = HttpConfigDto::default();
        let json = serde_json::to_string(&dto).unwrap();
        let parsed: HttpConfigDto = serde_json::from_str(&json).unwrap();
        assert_eq!(dto, parsed);
    }

    #[test]
    fn agent_limits_dto_roundtrip() {
        let dto = AgentLimitsDto {
            max_rounds: 50,
            max_total_tokens: 100_000,
            max_duration_secs: 600,
            tool_timeout_secs: 60,
            provider_timeout_secs: 120,
            max_parallel_tools: 5,
            max_tool_output_chars: 50_000,
        };
        let json = serde_json::to_string(&dto).unwrap();
        let parsed: AgentLimitsDto = serde_json::from_str(&json).unwrap();
        assert_eq!(dto, parsed);
    }

    #[test]
    fn routing_config_dto_roundtrip() {
        let dto = RoutingConfigDto {
            strategy: "fast".into(),
            mode: "speculative".into(),
            fallback_models: vec!["gpt-4o".into(), "deepseek-chat".into()],
            max_retries: 3,
        };
        let json = serde_json::to_string(&dto).unwrap();
        let parsed: RoutingConfigDto = serde_json::from_str(&json).unwrap();
        assert_eq!(dto, parsed);
    }

    #[test]
    fn tools_config_dto_roundtrip() {
        let dto = ToolsConfigDto {
            confirm_destructive: false,
            timeout_secs: 60,
            allowed_directories: vec!["/home/user".into()],
            blocked_patterns: vec!["**/.env".into(), "**/*.key".into()],
            dry_run: true,
            sandbox: SandboxConfigDto {
                enabled: false,
                max_output_bytes: 50_000,
                max_memory_mb: 256,
                max_cpu_secs: 30,
                max_file_size_bytes: 10_000_000,
            },
        };
        let json = serde_json::to_string(&dto).unwrap();
        let parsed: ToolsConfigDto = serde_json::from_str(&json).unwrap();
        assert_eq!(dto, parsed);
    }

    #[test]
    fn security_config_dto_roundtrip() {
        let dto = SecurityConfigDto {
            pii_detection: false,
            pii_action: "block".into(),
            audit_enabled: false,
            guardrails_enabled: false,
            guardrails_builtins: false,
            tbac_enabled: true,
        };
        let json = serde_json::to_string(&dto).unwrap();
        let parsed: SecurityConfigDto = serde_json::from_str(&json).unwrap();
        assert_eq!(dto, parsed);
    }

    #[test]
    fn memory_config_dto_roundtrip() {
        let dto = MemoryConfigDto {
            enabled: false,
            max_entries: 500,
            auto_summarize: false,
            episodic: true,
            decay_half_life_days: 14.0,
        };
        let json = serde_json::to_string(&dto).unwrap();
        let parsed: MemoryConfigDto = serde_json::from_str(&json).unwrap();
        assert_eq!(dto, parsed);
    }

    #[test]
    fn resilience_config_dto_roundtrip() {
        let dto = ResilienceConfigDto {
            enabled: false,
            circuit_breaker: CircuitBreakerConfigDto {
                failure_threshold: 10,
                window_secs: 120,
                open_duration_secs: 60,
                half_open_probes: 3,
            },
            health: HealthConfigDto {
                window_minutes: 30,
                degraded_threshold: 40,
                unhealthy_threshold: 20,
            },
            backpressure: BackpressureConfigDto {
                max_concurrent_per_provider: 10,
                queue_timeout_secs: 60,
            },
        };
        let json = serde_json::to_string(&dto).unwrap();
        let parsed: ResilienceConfigDto = serde_json::from_str(&json).unwrap();
        assert_eq!(dto, parsed);
    }

    #[test]
    fn circuit_breaker_config_dto_roundtrip() {
        let dto = CircuitBreakerConfigDto {
            failure_threshold: 3,
            window_secs: 30,
            open_duration_secs: 15,
            half_open_probes: 1,
        };
        let json = serde_json::to_string(&dto).unwrap();
        let parsed: CircuitBreakerConfigDto = serde_json::from_str(&json).unwrap();
        assert_eq!(dto, parsed);
    }

    #[test]
    fn sandbox_config_dto_roundtrip() {
        let dto = SandboxConfigDto {
            enabled: true,
            max_output_bytes: 200_000,
            max_memory_mb: 1024,
            max_cpu_secs: 120,
            max_file_size_bytes: 100_000_000,
        };
        let json = serde_json::to_string(&dto).unwrap();
        let parsed: SandboxConfigDto = serde_json::from_str(&json).unwrap();
        assert_eq!(dto, parsed);
    }

    #[test]
    fn validate_good_config() {
        let config = sample_config();
        let issues = validate_config_dto(&config);
        assert!(issues.is_empty(), "expected no issues: {issues:?}");
    }

    #[test]
    fn validate_bad_temperature() {
        let mut config = sample_config();
        config.general.temperature = 3.0;
        let issues = validate_config_dto(&config);
        assert!(issues.iter().any(|i| i.field.contains("temperature")));
    }

    #[test]
    fn validate_zero_max_tokens() {
        let mut config = sample_config();
        config.general.max_tokens = 0;
        let issues = validate_config_dto(&config);
        assert!(issues.iter().any(|i| i.field.contains("max_tokens")));
    }

    #[test]
    fn validate_zero_tool_timeout() {
        let mut config = sample_config();
        config.tools.timeout_secs = 0;
        let issues = validate_config_dto(&config);
        assert!(issues.iter().any(|i| i.field.contains("timeout")));
    }

    #[test]
    fn validate_zero_max_rounds() {
        let mut config = sample_config();
        config.agent_limits.max_rounds = 0;
        let issues = validate_config_dto(&config);
        assert!(issues.iter().any(|i| i.field.contains("max_rounds")));
    }

    #[test]
    fn partial_update_with_only_general() {
        let req = UpdateConfigRequest {
            general: Some(GeneralConfigDto {
                default_provider: "ollama".into(),
                default_model: "llama3.2".into(),
                temperature: 0.0,
                max_tokens: 4096,
            }),
            ..Default::default()
        };
        // Verify only general is serialized.
        let json = serde_json::to_string(&req).unwrap();
        let val: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(val.get("general").is_some());
        assert!(val.get("providers").is_none());
        assert!(val.get("agent_limits").is_none());
    }
}
