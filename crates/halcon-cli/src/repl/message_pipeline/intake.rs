//! Stage 1: Intake — Guard checks, plugin resume, message recording, media extraction.
//!
//! # Xiyo Comparison
//!
//! Xiyo performs these as inline checks at the top of `query()` (lines 307-340).
//! Halcon extracts them into a typed stage with explicit output, enabling:
//! - Independent testing of guard logic
//! - Clear separation of one-time-per-session vs per-message operations
//!
//! # Side Effects
//!
//! - Mutates `RuntimeGuards` (one-time flags)
//! - Locks `PluginRegistry` briefly for resume
//! - Writes user message to `Session`
//! - Reads filesystem for media path validation
//! - Calls `RenderSink` for UI feedback

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;

use halcon_core::types::{AppConfig, ChatMessage, MessageContent, Role, Session};

use crate::render::sink::RenderSink;
use crate::repl::plugins;
use crate::repl::runtime_control::RuntimeGuards;

use super::super::application::onboarding;
use super::super::git_tools::project_inspector;

/// Output of the Intake stage.
#[derive(Debug)]
pub struct IntakeOutput {
    /// Media file paths extracted from user message (not yet analyzed).
    pub media_paths: Vec<PathBuf>,
    /// Whether the multimodal subsystem is available for media analysis.
    pub has_multimodal: bool,
}

/// Stage 1: Intake — one-time guards, plugin resume, message recording, media extraction.
///
/// This stage handles all pre-processing before context assembly begins.
/// Guard checks are idempotent: each runs at most once per session.
pub struct IntakeStage;

impl IntakeStage {
    /// Execute the intake stage.
    ///
    /// # Arguments
    /// - `input`: Raw user message text
    /// - `session`: Mutable session for recording the user message
    /// - `guards`: Mutable runtime guards for one-time checks
    /// - `plugin_registry`: Optional plugin registry for resume/recommendation
    /// - `config`: Application config for plugin settings
    /// - `has_multimodal`: Whether multimodal subsystem is available
    /// - `sink`: Render sink for UI feedback
    pub async fn execute(
        input: &str,
        session: &mut Session,
        guards: &mut RuntimeGuards,
        plugin_registry: Option<&Arc<Mutex<plugins::PluginRegistry>>>,
        config: &AppConfig,
        has_multimodal: bool,
        sink: &dyn RenderSink,
    ) -> Result<IntakeOutput> {
        // ── Phase 94: One-time onboarding check (file existence, <1ms) ──
        if !guards.onboarding_checked {
            guards.onboarding_checked = true;
            let cwd = std::env::current_dir().unwrap_or_default();
            match onboarding::OnboardingCheck::run(&cwd) {
                onboarding::OnboardingStatus::Configured { path } => {
                    sink.project_config_loaded(&path.to_string_lossy());
                }
                onboarding::OnboardingStatus::NotConfigured { root, project_type } => {
                    sink.onboarding_suggestion(&root.to_string_lossy(), &project_type);
                }
                onboarding::OnboardingStatus::Unknown => {}
            }
        }

        // ── Phase 95: Auto-resume plugins with expired cooling periods ──
        if let Some(arc_pr) = plugin_registry {
            if let Ok(mut reg) = arc_pr.lock() {
                reg.maybe_resume_plugins();
            }
        }

        // ── Phase 95: One-time plugin recommendation on first message ──
        if !guards.plugin_recommendation_done && config.plugins.enabled {
            guards.plugin_recommendation_done = true;
            if let Some(arc_pr) = plugin_registry {
                if let Ok(reg) = arc_pr.lock() {
                    let cwd = std::env::current_dir().unwrap_or_default();
                    let analysis = project_inspector::ProjectInspector::analyze(&cwd);
                    let loaded: std::collections::HashSet<String> =
                        reg.loaded_plugin_ids().map(|s| s.to_string()).collect();
                    let rewards = reg.ucb1_rewards_snapshot();
                    drop(reg); // release lock before calling sink
                    let recs = plugins::PluginRecommendationEngine::recommend(
                        &analysis, &loaded, &rewards,
                    );
                    let total_new: usize = recs.iter().filter(|r| !r.already_installed).count();
                    let essential: usize = recs
                        .iter()
                        .filter(|r| {
                            !r.already_installed && r.tier == plugins::RecommendationTier::Essential
                        })
                        .count();
                    if total_new > 0 {
                        sink.plugin_suggestion(total_new, essential);
                    }
                }
            }
        }

        // ── Record user message in session ──
        session.add_message(ChatMessage {
            role: Role::User,
            content: MessageContent::Text(input.to_string()),
        });

        // ── Extract media paths from user message ──
        let media_paths = if has_multimodal {
            super::super::extract_media_paths(input)
        } else {
            Vec::new()
        };

        Ok(IntakeOutput {
            media_paths,
            has_multimodal,
        })
    }
}
