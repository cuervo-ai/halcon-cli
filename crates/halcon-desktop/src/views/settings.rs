use egui::{CollapsingHeader, RichText, Ui};
use tokio::sync::mpsc;

use crate::config::AppConfig;
use crate::state::{AppState, ConnectionState};
use crate::theme::HalconTheme;
use crate::workers::UiCommand;
use halcon_api::types::config::*;

pub fn render(
    ui: &mut Ui,
    state: &mut AppState,
    config: &mut AppConfig,
    cmd_tx: &mpsc::Sender<UiCommand>,
) {
    ui.heading("Settings");
    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| {
        // ── Desktop Settings (always visible) ───────────────────
        CollapsingHeader::new(RichText::new("Desktop Settings").strong())
            .default_open(true)
            .show(ui, |ui| {
                render_desktop_settings(ui, config, cmd_tx);
            });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        // ── Runtime Configuration (only when connected + loaded) ─
        let is_connected = state.connection == ConnectionState::Connected;
        let has_config = state.runtime_config.is_some();

        if !is_connected {
            ui.colored_label(
                HalconTheme::TEXT_MUTED,
                "Connect to a runtime to manage configuration.",
            );
        } else if !has_config {
            ui.colored_label(HalconTheme::TEXT_MUTED, "Loading runtime configuration...");
        } else {
            ui.label(RichText::new("Runtime Configuration").strong().size(14.0));
            ui.add_space(4.0);

            // We clone the config for editing, then put it back.
            let mut cfg = state.runtime_config.clone().unwrap();
            let original = cfg.clone();

            render_general_section(ui, &mut cfg.general, state);
            render_providers_section(ui, &mut cfg.providers, state);
            render_agent_limits_section(ui, &mut cfg.agent_limits, state);
            render_routing_section(ui, &mut cfg.routing, state);
            render_tools_section(ui, &mut cfg.tools, state);
            render_security_section(ui, &mut cfg.security, state);
            render_memory_section(ui, &mut cfg.memory, state);
            render_resilience_section(ui, &mut cfg.resilience, state);

            // Check if anything changed.
            if cfg != original {
                state.config_dirty = true;
            }
            state.runtime_config = Some(cfg);

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(4.0);

            // Save / Reload buttons.
            ui.horizontal(|ui| {
                let save_btn = ui.add_enabled(
                    state.config_dirty,
                    egui::Button::new(
                        RichText::new("Save Changes")
                            .color(if state.config_dirty {
                                HalconTheme::ACCENT
                            } else {
                                HalconTheme::TEXT_MUTED
                            }),
                    ),
                );
                if save_btn.clicked() {
                    if let Some(ref cfg) = state.runtime_config {
                        let update = build_update_request(cfg);
                        let _ = cmd_tx.try_send(UiCommand::UpdateConfig(Box::new(update)));
                    }
                }

                if ui.button("Reload").clicked() {
                    let _ = cmd_tx.try_send(UiCommand::RefreshConfig);
                }

                if state.config_dirty {
                    ui.colored_label(HalconTheme::WARNING, "Unsaved changes");
                }
            });

            // Error display.
            if let Some(ref err) = state.config_error {
                ui.add_space(4.0);
                ui.colored_label(HalconTheme::ERROR, err);
            }

            // Validation warnings.
            if let Some(ref cfg) = state.runtime_config {
                let issues = validate_config_dto(cfg);
                if !issues.is_empty() {
                    ui.add_space(4.0);
                    for issue in &issues {
                        ui.colored_label(
                            HalconTheme::WARNING,
                            format!("{}: {}", issue.field, issue.message),
                        );
                    }
                }
            }
        }

        ui.add_space(16.0);
        ui.separator();
        ui.add_space(4.0);

        // ── Danger Zone ─────────────────────────────────────────
        ui.label(RichText::new("Danger Zone").strong().color(HalconTheme::ERROR));
        ui.add_space(4.0);
        if ui
            .button(RichText::new("Shutdown Runtime").color(HalconTheme::ERROR))
            .clicked()
        {
            let _ = cmd_tx.try_send(UiCommand::Shutdown { graceful: true });
        }
    });
}

// ── Desktop Settings ────────────────────────────────────────────────

fn render_desktop_settings(
    ui: &mut Ui,
    config: &mut AppConfig,
    cmd_tx: &mpsc::Sender<UiCommand>,
) {
    ui.horizontal(|ui| {
        ui.label("Server URL:");
        ui.text_edit_singleline(&mut config.server_url);
    });
    ui.horizontal(|ui| {
        ui.label("Auth Token:");
        ui.add(egui::TextEdit::singleline(&mut config.auth_token).password(true));
    });
    ui.horizontal(|ui| {
        if ui.button("Connect").clicked() {
            let _ = cmd_tx.try_send(UiCommand::Connect {
                url: config.server_url.clone(),
                token: config.auth_token.clone(),
            });
        }
        if ui.button("Disconnect").clicked() {
            let _ = cmd_tx.try_send(UiCommand::Disconnect);
        }
        if ui.button("Save").on_hover_text("Save URL + token to ~/.halcon/desktop.toml").clicked() {
            if let Err(e) = config.save() {
                tracing::warn!(error = %e, "failed to save desktop config");
            }
        }
    });

    ui.add_space(4.0);
    ui.checkbox(&mut config.dark_mode, "Dark mode");

    ui.horizontal(|ui| {
        ui.label("Poll interval (secs):");
        ui.add(egui::DragValue::new(&mut config.poll_interval_secs).range(1..=60));
    });
    ui.horizontal(|ui| {
        ui.label("Max log entries:");
        ui.add(egui::DragValue::new(&mut config.max_log_entries).range(100..=100_000));
    });
    ui.horizontal(|ui| {
        ui.label("Max events:");
        ui.add(egui::DragValue::new(&mut config.max_events).range(100..=50_000));
    });
}

// ── Runtime config section renderers ────────────────────────────────

fn render_general_section(ui: &mut Ui, g: &mut GeneralConfigDto, state: &mut AppState) {
    CollapsingHeader::new("General")
        .default_open(false)
        .show(ui, |ui| {
            let mut dirty = false;
            ui.horizontal(|ui| {
                ui.label("Default provider:");
                dirty |= ui.text_edit_singleline(&mut g.default_provider).changed();
            });
            ui.horizontal(|ui| {
                ui.label("Default model:");
                dirty |= ui.text_edit_singleline(&mut g.default_model).changed();
            });
            ui.horizontal(|ui| {
                ui.label("Temperature:");
                dirty |= ui
                    .add(egui::Slider::new(&mut g.temperature, 0.0..=2.0).step_by(0.05))
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Max tokens:");
                dirty |= ui
                    .add(egui::DragValue::new(&mut g.max_tokens).range(1..=1_000_000))
                    .changed();
            });
            if dirty {
                state.config_dirty = true;
            }
        });
}

fn render_providers_section(
    ui: &mut Ui,
    providers: &mut std::collections::HashMap<String, ProviderConfigDto>,
    state: &mut AppState,
) {
    CollapsingHeader::new("Providers & API Keys")
        .default_open(false)
        .show(ui, |ui| {
            // Sort provider names for deterministic display.
            let mut names: Vec<_> = providers.keys().cloned().collect();
            names.sort();

            for name in names {
                if let Some(p) = providers.get_mut(&name) {
                    let mut dirty = false;
                    CollapsingHeader::new(&name)
                        .default_open(false)
                        .id_salt(format!("provider_{name}"))
                        .show(ui, |ui| {
                            dirty |= ui.checkbox(&mut p.enabled, "Enabled").changed();

                            // api_base
                            let mut base =
                                p.api_base.clone().unwrap_or_default();
                            ui.horizontal(|ui| {
                                ui.label("API Base:");
                                if ui.text_edit_singleline(&mut base).changed() {
                                    p.api_base = if base.is_empty() {
                                        None
                                    } else {
                                        Some(base.clone())
                                    };
                                    dirty = true;
                                }
                            });

                            // api_key_env
                            let mut key_env =
                                p.api_key_env.clone().unwrap_or_default();
                            ui.horizontal(|ui| {
                                ui.label("API Key Env:");
                                if ui.text_edit_singleline(&mut key_env).changed() {
                                    p.api_key_env = if key_env.is_empty() {
                                        None
                                    } else {
                                        Some(key_env.clone())
                                    };
                                    dirty = true;
                                }
                            });

                            // default_model
                            let mut model =
                                p.default_model.clone().unwrap_or_default();
                            ui.horizontal(|ui| {
                                ui.label("Default model:");
                                if ui.text_edit_singleline(&mut model).changed() {
                                    p.default_model = if model.is_empty() {
                                        None
                                    } else {
                                        Some(model.clone())
                                    };
                                    dirty = true;
                                }
                            });

                            // HTTP config
                            ui.label(RichText::new("HTTP").small());
                            ui.horizontal(|ui| {
                                ui.label("Connect timeout:");
                                dirty |= ui
                                    .add(
                                        egui::DragValue::new(
                                            &mut p.http.connect_timeout_secs,
                                        )
                                        .range(0..=300)
                                        .suffix("s"),
                                    )
                                    .changed();
                            });
                            ui.horizontal(|ui| {
                                ui.label("Request timeout:");
                                dirty |= ui
                                    .add(
                                        egui::DragValue::new(
                                            &mut p.http.request_timeout_secs,
                                        )
                                        .range(0..=3600)
                                        .suffix("s"),
                                    )
                                    .changed();
                            });
                            ui.horizontal(|ui| {
                                ui.label("Max retries:");
                                dirty |= ui
                                    .add(
                                        egui::DragValue::new(&mut p.http.max_retries)
                                            .range(0..=20),
                                    )
                                    .changed();
                            });
                            ui.horizontal(|ui| {
                                ui.label("Retry base delay:");
                                dirty |= ui
                                    .add(
                                        egui::DragValue::new(
                                            &mut p.http.retry_base_delay_ms,
                                        )
                                        .range(0..=30_000)
                                        .suffix("ms"),
                                    )
                                    .changed();
                            });
                        });
                    if dirty {
                        state.config_dirty = true;
                    }
                }
            }
        });
}

fn render_agent_limits_section(
    ui: &mut Ui,
    l: &mut AgentLimitsDto,
    state: &mut AppState,
) {
    CollapsingHeader::new("Agent Limits")
        .default_open(false)
        .show(ui, |ui| {
            let mut dirty = false;
            ui.horizontal(|ui| {
                ui.label("Max rounds:");
                dirty |= ui
                    .add(egui::DragValue::new(&mut l.max_rounds).range(1..=1000))
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Max total tokens (0=unlimited):");
                dirty |= ui
                    .add(egui::DragValue::new(&mut l.max_total_tokens).range(0..=10_000_000))
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Max duration (0=unlimited):");
                dirty |= ui
                    .add(
                        egui::DragValue::new(&mut l.max_duration_secs)
                            .range(0..=86400)
                            .suffix("s"),
                    )
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Tool timeout:");
                dirty |= ui
                    .add(
                        egui::DragValue::new(&mut l.tool_timeout_secs)
                            .range(1..=3600)
                            .suffix("s"),
                    )
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Provider timeout:");
                dirty |= ui
                    .add(
                        egui::DragValue::new(&mut l.provider_timeout_secs)
                            .range(0..=3600)
                            .suffix("s"),
                    )
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Max parallel tools:");
                dirty |= ui
                    .add(egui::DragValue::new(&mut l.max_parallel_tools).range(0..=100))
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Max tool output chars:");
                dirty |= ui
                    .add(
                        egui::DragValue::new(&mut l.max_tool_output_chars)
                            .range(0..=10_000_000),
                    )
                    .changed();
            });
            if dirty {
                state.config_dirty = true;
            }
        });
}

fn render_routing_section(ui: &mut Ui, r: &mut RoutingConfigDto, state: &mut AppState) {
    CollapsingHeader::new("Routing Strategy")
        .default_open(false)
        .show(ui, |ui| {
            let mut dirty = false;

            ui.horizontal(|ui| {
                ui.label("Strategy:");
                let strategies = ["balanced", "fast", "cheap"];
                egui::ComboBox::from_id_salt("routing_strategy")
                    .selected_text(&r.strategy)
                    .show_ui(ui, |ui| {
                        for s in &strategies {
                            if ui
                                .selectable_label(r.strategy == *s, *s)
                                .clicked()
                            {
                                r.strategy = s.to_string();
                                dirty = true;
                            }
                        }
                    });
            });

            ui.horizontal(|ui| {
                ui.label("Mode:");
                let modes = ["failover", "speculative"];
                egui::ComboBox::from_id_salt("routing_mode")
                    .selected_text(&r.mode)
                    .show_ui(ui, |ui| {
                        for m in &modes {
                            if ui.selectable_label(r.mode == *m, *m).clicked() {
                                r.mode = m.to_string();
                                dirty = true;
                            }
                        }
                    });
            });

            ui.horizontal(|ui| {
                ui.label("Max retries:");
                dirty |= ui
                    .add(egui::DragValue::new(&mut r.max_retries).range(0..=20))
                    .changed();
            });

            // Fallback models as a comma-separated text field.
            let mut fallback_text = r.fallback_models.join(", ");
            ui.horizontal(|ui| {
                ui.label("Fallback models:");
                if ui.text_edit_singleline(&mut fallback_text).changed() {
                    r.fallback_models = fallback_text
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    dirty = true;
                }
            });

            if dirty {
                state.config_dirty = true;
            }
        });
}

fn render_tools_section(ui: &mut Ui, t: &mut ToolsConfigDto, state: &mut AppState) {
    CollapsingHeader::new("Tools & Sandbox")
        .default_open(false)
        .show(ui, |ui| {
            let mut dirty = false;

            dirty |= ui
                .checkbox(&mut t.confirm_destructive, "Confirm destructive ops")
                .changed();
            dirty |= ui.checkbox(&mut t.dry_run, "Dry-run mode").changed();

            ui.horizontal(|ui| {
                ui.label("Timeout:");
                dirty |= ui
                    .add(
                        egui::DragValue::new(&mut t.timeout_secs)
                            .range(1..=3600)
                            .suffix("s"),
                    )
                    .changed();
            });

            // Blocked patterns as comma-separated.
            let mut patterns_text = t.blocked_patterns.join(", ");
            ui.horizontal(|ui| {
                ui.label("Blocked patterns:");
                if ui.text_edit_singleline(&mut patterns_text).changed() {
                    t.blocked_patterns = patterns_text
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    dirty = true;
                }
            });

            // Allowed directories.
            let mut dirs_text = t.allowed_directories.join(", ");
            ui.horizontal(|ui| {
                ui.label("Allowed dirs:");
                if ui.text_edit_singleline(&mut dirs_text).changed() {
                    t.allowed_directories = dirs_text
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    dirty = true;
                }
            });

            ui.add_space(4.0);
            ui.label(RichText::new("Sandbox").small().strong());
            dirty |= ui.checkbox(&mut t.sandbox.enabled, "Sandbox enabled").changed();
            ui.horizontal(|ui| {
                ui.label("Max output bytes:");
                dirty |= ui
                    .add(
                        egui::DragValue::new(&mut t.sandbox.max_output_bytes)
                            .range(0..=10_000_000),
                    )
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Max memory:");
                dirty |= ui
                    .add(
                        egui::DragValue::new(&mut t.sandbox.max_memory_mb)
                            .range(0..=8192)
                            .suffix(" MB"),
                    )
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Max CPU time:");
                dirty |= ui
                    .add(
                        egui::DragValue::new(&mut t.sandbox.max_cpu_secs)
                            .range(0..=3600)
                            .suffix("s"),
                    )
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Max file size:");
                dirty |= ui
                    .add(
                        egui::DragValue::new(&mut t.sandbox.max_file_size_bytes)
                            .range(0..=1_000_000_000)
                            .suffix(" B"),
                    )
                    .changed();
            });

            if dirty {
                state.config_dirty = true;
            }
        });
}

fn render_security_section(ui: &mut Ui, s: &mut SecurityConfigDto, state: &mut AppState) {
    CollapsingHeader::new("Security & Guardrails")
        .default_open(false)
        .show(ui, |ui| {
            let mut dirty = false;

            dirty |= ui
                .checkbox(&mut s.pii_detection, "PII detection")
                .changed();

            ui.horizontal(|ui| {
                ui.label("PII action:");
                let actions = ["warn", "redact", "block"];
                egui::ComboBox::from_id_salt("pii_action")
                    .selected_text(&s.pii_action)
                    .show_ui(ui, |ui| {
                        for a in &actions {
                            if ui
                                .selectable_label(s.pii_action == *a, *a)
                                .clicked()
                            {
                                s.pii_action = a.to_string();
                                dirty = true;
                            }
                        }
                    });
            });

            dirty |= ui.checkbox(&mut s.audit_enabled, "Audit trail").changed();
            dirty |= ui
                .checkbox(&mut s.guardrails_enabled, "Guardrails enabled")
                .changed();
            dirty |= ui
                .checkbox(&mut s.guardrails_builtins, "Built-in guardrails")
                .changed();
            dirty |= ui.checkbox(&mut s.tbac_enabled, "TBAC enabled").changed();

            if dirty {
                state.config_dirty = true;
            }
        });
}

fn render_memory_section(ui: &mut Ui, m: &mut MemoryConfigDto, state: &mut AppState) {
    CollapsingHeader::new("Memory System")
        .default_open(false)
        .show(ui, |ui| {
            let mut dirty = false;

            dirty |= ui.checkbox(&mut m.enabled, "Memory enabled").changed();
            dirty |= ui
                .checkbox(&mut m.auto_summarize, "Auto-summarize")
                .changed();
            dirty |= ui.checkbox(&mut m.episodic, "Episodic memory").changed();

            ui.horizontal(|ui| {
                ui.label("Max entries:");
                dirty |= ui
                    .add(egui::DragValue::new(&mut m.max_entries).range(0..=1_000_000))
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Decay half-life:");
                dirty |= ui
                    .add(
                        egui::DragValue::new(&mut m.decay_half_life_days)
                            .range(1.0..=365.0)
                            .suffix(" days"),
                    )
                    .changed();
            });

            if dirty {
                state.config_dirty = true;
            }
        });
}

fn render_resilience_section(
    ui: &mut Ui,
    r: &mut ResilienceConfigDto,
    state: &mut AppState,
) {
    CollapsingHeader::new("Resilience")
        .default_open(false)
        .show(ui, |ui| {
            let mut dirty = false;

            dirty |= ui.checkbox(&mut r.enabled, "Resilience enabled").changed();

            ui.add_space(4.0);
            ui.label(RichText::new("Circuit Breaker").small().strong());
            ui.horizontal(|ui| {
                ui.label("Failure threshold:");
                dirty |= ui
                    .add(
                        egui::DragValue::new(&mut r.circuit_breaker.failure_threshold)
                            .range(1..=100),
                    )
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Window:");
                dirty |= ui
                    .add(
                        egui::DragValue::new(&mut r.circuit_breaker.window_secs)
                            .range(1..=3600)
                            .suffix("s"),
                    )
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Open duration:");
                dirty |= ui
                    .add(
                        egui::DragValue::new(&mut r.circuit_breaker.open_duration_secs)
                            .range(1..=3600)
                            .suffix("s"),
                    )
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Half-open probes:");
                dirty |= ui
                    .add(
                        egui::DragValue::new(&mut r.circuit_breaker.half_open_probes)
                            .range(1..=20),
                    )
                    .changed();
            });

            ui.add_space(4.0);
            ui.label(RichText::new("Health").small().strong());
            ui.horizontal(|ui| {
                ui.label("Window:");
                dirty |= ui
                    .add(
                        egui::DragValue::new(&mut r.health.window_minutes)
                            .range(1..=1440)
                            .suffix(" min"),
                    )
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Degraded threshold:");
                dirty |= ui
                    .add(
                        egui::DragValue::new(&mut r.health.degraded_threshold)
                            .range(0..=100),
                    )
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Unhealthy threshold:");
                dirty |= ui
                    .add(
                        egui::DragValue::new(&mut r.health.unhealthy_threshold)
                            .range(0..=100),
                    )
                    .changed();
            });

            ui.add_space(4.0);
            ui.label(RichText::new("Backpressure").small().strong());
            ui.horizontal(|ui| {
                ui.label("Max concurrent/provider:");
                dirty |= ui
                    .add(
                        egui::DragValue::new(
                            &mut r.backpressure.max_concurrent_per_provider,
                        )
                        .range(1..=100),
                    )
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Queue timeout:");
                dirty |= ui
                    .add(
                        egui::DragValue::new(&mut r.backpressure.queue_timeout_secs)
                            .range(0..=600)
                            .suffix("s"),
                    )
                    .changed();
            });

            if dirty {
                state.config_dirty = true;
            }
        });
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Build an `UpdateConfigRequest` with all sections populated from the current config.
fn build_update_request(cfg: &RuntimeConfigResponse) -> halcon_client::UpdateConfigRequest {
    halcon_client::UpdateConfigRequest {
        general: Some(cfg.general.clone()),
        providers: Some(cfg.providers.clone()),
        agent_limits: Some(cfg.agent_limits.clone()),
        routing: Some(cfg.routing.clone()),
        tools: Some(cfg.tools.clone()),
        security: Some(cfg.security.clone()),
        memory: Some(cfg.memory.clone()),
        resilience: Some(cfg.resilience.clone()),
    }
}
