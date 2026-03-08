//! Phase 95: Plugin Auto-Bootstrap Engine
//!
//! Writes `.plugin.toml` manifest files for recommended plugins so that
//! PluginLoader picks them up on the next HALCON startup.
//!
//! All I/O is synchronous and intentionally simple — this module is called
//! from a `spawn_blocking` context in the TUI dispatch.

use std::path::PathBuf;

use super::recommendation::{PluginRecommendation, RecommendationTier};

// ─── Bootstrap Options ────────────────────────────────────────────────────────

/// Configuration for a bootstrap run.
pub struct BootstrapOptions {
    /// If true, calculate what would be installed but don't write files.
    pub dry_run: bool,
    /// Directory where `.plugin.toml` manifests are written.
    pub plugin_dir: PathBuf,
    /// Which tiers to install (default: Essential + Recommended).
    pub tiers: Vec<RecommendationTier>,
}

impl Default for BootstrapOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            plugin_dir: dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".halcon/plugins"),
            tiers: vec![
                RecommendationTier::Essential,
                RecommendationTier::Recommended,
            ],
        }
    }
}

// ─── Bootstrap Result ─────────────────────────────────────────────────────────

/// Result of a bootstrap run.
pub struct BootstrapResult {
    /// Plugin IDs that were written (or would be written in dry-run).
    pub installed: Vec<String>,
    /// Plugin IDs that were skipped (already installed or wrong tier).
    pub skipped: Vec<String>,
    /// (plugin_id, error_message) pairs for failed installs.
    pub failed: Vec<(String, String)>,
    /// Whether this was a dry-run.
    pub dry_run: bool,
}

// ─── Engine ───────────────────────────────────────────────────────────────────

/// Auto-installs plugins by writing `.plugin.toml` manifest files.
pub struct AutoPluginBootstrap;

impl AutoPluginBootstrap {
    /// Install plugins matching `opts.tiers` from the recommendation list.
    ///
    /// For each recommendation:
    /// 1. Skip if `already_installed`.
    /// 2. Skip if `tier` not in `opts.tiers`.
    /// 3. Check `opts.plugin_dir/<plugin_id>/` exists.
    /// 4. Write `opts.plugin_dir/<plugin_id>.plugin.toml` manifest.
    /// 5. On any FS error, rollback all files written so far in this call.
    pub fn bootstrap(
        recommendations: &[PluginRecommendation],
        opts: &BootstrapOptions,
    ) -> BootstrapResult {
        let mut installed: Vec<String> = Vec::new();
        let mut skipped: Vec<String> = Vec::new();
        let mut failed: Vec<(String, String)> = Vec::new();
        let mut written_files: Vec<PathBuf> = Vec::new();

        for rec in recommendations {
            // Skip if already installed
            if rec.already_installed {
                skipped.push(rec.plugin_id.clone());
                continue;
            }

            // Skip if tier not requested
            if !opts.tiers.contains(&rec.tier) {
                skipped.push(rec.plugin_id.clone());
                continue;
            }

            // In dry-run mode, record as installed without writing
            if opts.dry_run {
                installed.push(rec.plugin_id.clone());
                continue;
            }

            // Check that the plugin source directory exists
            let plugin_src_dir = opts.plugin_dir.join(&rec.plugin_id);
            if !plugin_src_dir.exists() {
                failed.push((
                    rec.plugin_id.clone(),
                    format!("plugin directory not found: {}", plugin_src_dir.display()),
                ));
                continue;
            }

            // Write the manifest
            let manifest_path = opts.plugin_dir.join(format!("{}.plugin.toml", rec.plugin_id));
            let main_py = plugin_src_dir.join("main.py");
            let manifest_content = Self::manifest_toml(rec, &main_py);

            match std::fs::write(&manifest_path, &manifest_content) {
                Ok(()) => {
                    written_files.push(manifest_path);
                    installed.push(rec.plugin_id.clone());
                }
                Err(e) => {
                    // Rollback all previously written files
                    for path in &written_files {
                        if let Err(rm_err) = std::fs::remove_file(path) {
                            tracing::warn!(
                                "plugin bootstrap rollback: failed to remove {}: {}",
                                path.display(),
                                rm_err
                            );
                        }
                    }
                    // Update installed list to remove rolled-back plugins
                    let rolled_back_count = written_files.len();
                    let written_ids: Vec<String> = installed
                        .drain(installed.len().saturating_sub(rolled_back_count)..)
                        .collect();
                    for id in written_ids {
                        skipped.push(id);
                    }
                    written_files.clear();

                    failed.push((rec.plugin_id.clone(), format!("write error: {e}")));
                }
            }
        }

        BootstrapResult {
            installed,
            skipped,
            failed,
            dry_run: opts.dry_run,
        }
    }

    /// Generate the TOML manifest content for a plugin.
    fn manifest_toml(rec: &PluginRecommendation, main_py: &std::path::Path) -> String {
        format!(
            "# Auto-generated by HALCON /plugins auto\n\
             [meta]\n\
             id = \"{}\"\n\
             name = \"{}\"\n\
             version = \"1.0.0\"\n\
             \n\
             [transport]\n\
             type = \"stdio\"\n\
             command = \"python3\"\n\
             args = [\"{}\"]\n\
             \n\
             [permissions]\n\
             risk_tier = \"low\"\n",
            rec.plugin_id,
            rec.display_name,
            main_py.display(),
        )
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repl::plugins::recommendation::{PluginRecommendation, RecommendationTier};
    use std::collections::HashSet;

    fn make_rec(id: &str, tier: RecommendationTier, installed: bool) -> PluginRecommendation {
        PluginRecommendation {
            plugin_id: id.to_string(),
            display_name: format!("Plugin {id}"),
            rationale: format!("Rationale for {id}"),
            tier,
            already_installed: installed,
            prior_reward: None,
        }
    }

    #[test]
    fn dry_run_returns_plan_without_writing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let opts = BootstrapOptions {
            dry_run: true,
            plugin_dir: dir.path().to_path_buf(),
            tiers: vec![RecommendationTier::Essential, RecommendationTier::Recommended],
        };
        let recs = vec![make_rec("my-plugin", RecommendationTier::Essential, false)];

        let result = AutoPluginBootstrap::bootstrap(&recs, &opts);
        assert_eq!(result.installed, vec!["my-plugin".to_string()]);
        assert!(result.dry_run);
        // No file written
        let manifest = dir.path().join("my-plugin.plugin.toml");
        assert!(!manifest.exists(), "dry-run should not write files");
    }

    #[test]
    fn bootstrap_writes_manifest_for_matching_tier() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Create plugin source directory
        let plugin_dir = dir.path().join("my-plugin");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(plugin_dir.join("main.py"), "# stub").unwrap();

        let opts = BootstrapOptions {
            dry_run: false,
            plugin_dir: dir.path().to_path_buf(),
            tiers: vec![RecommendationTier::Essential],
        };
        let recs = vec![make_rec("my-plugin", RecommendationTier::Essential, false)];

        let result = AutoPluginBootstrap::bootstrap(&recs, &opts);
        assert_eq!(result.installed, vec!["my-plugin".to_string()]);
        assert!(result.failed.is_empty());

        // Verify manifest was written
        let manifest = dir.path().join("my-plugin.plugin.toml");
        assert!(manifest.exists(), "manifest file should be written");
        let content = std::fs::read_to_string(&manifest).unwrap();
        assert!(content.contains("id = \"my-plugin\""), "manifest should contain plugin id");
        assert!(content.contains("type = \"stdio\""), "manifest should specify stdio transport");
    }

    #[test]
    fn skip_already_installed_plugin() {
        let dir = tempfile::tempdir().expect("tempdir");
        let opts = BootstrapOptions {
            dry_run: false,
            plugin_dir: dir.path().to_path_buf(),
            tiers: vec![RecommendationTier::Essential],
        };
        let recs = vec![make_rec("installed-plugin", RecommendationTier::Essential, true)];

        let result = AutoPluginBootstrap::bootstrap(&recs, &opts);
        assert!(result.installed.is_empty(), "already installed should be skipped");
        assert_eq!(result.skipped, vec!["installed-plugin".to_string()]);
    }

    #[test]
    fn skip_plugin_when_directory_not_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        let opts = BootstrapOptions {
            dry_run: false,
            plugin_dir: dir.path().to_path_buf(),
            tiers: vec![RecommendationTier::Essential],
        };
        // No plugin source dir created
        let recs = vec![make_rec("missing-plugin", RecommendationTier::Essential, false)];

        let result = AutoPluginBootstrap::bootstrap(&recs, &opts);
        assert!(result.installed.is_empty());
        assert_eq!(result.failed.len(), 1);
        assert!(result.failed[0].0 == "missing-plugin");
        assert!(result.failed[0].1.contains("not found"), "error should mention 'not found'");
    }

    #[test]
    fn wrong_tier_skipped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let opts = BootstrapOptions {
            dry_run: false,
            plugin_dir: dir.path().to_path_buf(),
            tiers: vec![RecommendationTier::Essential], // only Essential
        };
        // Recommended plugin — not in tiers list
        let recs = vec![make_rec("optional-plugin", RecommendationTier::Optional, false)];

        let result = AutoPluginBootstrap::bootstrap(&recs, &opts);
        assert!(result.installed.is_empty());
        assert_eq!(result.skipped, vec!["optional-plugin".to_string()]);
    }

    // Verify BootstrapOptions::default() compiles and has expected tiers.
    #[test]
    fn default_options_has_essential_and_recommended_tiers() {
        let opts = BootstrapOptions::default();
        assert!(!opts.dry_run);
        let tier_set: HashSet<_> = opts.tiers.iter().collect();
        assert!(tier_set.contains(&RecommendationTier::Essential));
        assert!(tier_set.contains(&RecommendationTier::Recommended));
        assert!(!tier_set.contains(&RecommendationTier::Optional));
    }
}
