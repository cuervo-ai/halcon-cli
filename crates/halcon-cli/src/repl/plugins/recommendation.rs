//! Phase 95: Plugin Recommendation Engine
//!
//! Analyses a `ProjectAnalysis` snapshot and generates a tiered list of plugin
//! recommendations. All logic is deterministic and runs in < 1 ms — no I/O,
//! no network, no subprocess execution.

use std::collections::{HashMap, HashSet};

use super::super::project_inspector::ProjectAnalysis;

// ─── Recommendation Tier ─────────────────────────────────────────────────────

/// Urgency tier for a plugin recommendation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RecommendationTier {
    /// Must-have for this stack — high productivity loss if absent.
    Essential,
    /// High-value, covers common workflows for this stack.
    Recommended,
    /// Useful but not critical.
    Optional,
    /// Cutting-edge / early access.
    Experimental,
}

impl RecommendationTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            RecommendationTier::Essential => "Essential",
            RecommendationTier::Recommended => "Recommended",
            RecommendationTier::Optional => "Optional",
            RecommendationTier::Experimental => "Experimental",
        }
    }

    /// Upgrade one level (Optional → Recommended → Essential; Essential stays).
    fn upgrade(self) -> Self {
        match self {
            RecommendationTier::Experimental => RecommendationTier::Optional,
            RecommendationTier::Optional => RecommendationTier::Recommended,
            RecommendationTier::Recommended => RecommendationTier::Essential,
            RecommendationTier::Essential => RecommendationTier::Essential,
        }
    }
}

// ─── Recommendation ───────────────────────────────────────────────────────────

/// A single plugin recommendation.
#[derive(Debug, Clone)]
pub struct PluginRecommendation {
    pub plugin_id: String,
    pub display_name: String,
    pub rationale: String,
    pub tier: RecommendationTier,
    pub already_installed: bool,
    /// UCB1 avg_reward from prior sessions (None if never used).
    pub prior_reward: Option<f64>,
}

// ─── Engine ───────────────────────────────────────────────────────────────────

/// Stateless recommendation engine — call `recommend()` with project metadata.
pub struct PluginRecommendationEngine;

/// Descriptor for a candidate recommendation (before personalisation).
struct Candidate {
    plugin_id: &'static str,
    display_name: &'static str,
    rationale: &'static str,
    tier: RecommendationTier,
}

impl PluginRecommendationEngine {
    /// Generate recommendations from project analysis + live plugin state.
    ///
    /// Fast (<1 ms) — no I/O, no subprocesses.
    pub fn recommend(
        analysis: &ProjectAnalysis,
        loaded_plugin_ids: &HashSet<String>,
        ucb1_rewards: &HashMap<String, f64>,
    ) -> Vec<PluginRecommendation> {
        let pt = analysis.project_type.as_str();
        let has_git = analysis.git_remote.is_some();
        let many_manifests = analysis.manifest_files.len() >= 5;

        // ── Candidate table ────────────────────────────────────────────────────
        // (plugin_id, display_name, rationale, base_tier, applies_when)
        let candidates: Vec<(&str, Candidate, bool)> = vec![
            // Rust
            (
                "rust",
                Candidate {
                    plugin_id: "halcon-dependency-auditor",
                    display_name: "HALCON Dependency Auditor",
                    rationale: "Cargo dependency security + license audit",
                    tier: RecommendationTier::Essential,
                },
                pt == "rust" || pt == "mixed",
            ),
            (
                "rust",
                Candidate {
                    plugin_id: "halcon-dev-sentinel",
                    display_name: "HALCON Dev Sentinel",
                    rationale: "Code quality: dead code, complexity, TODOs",
                    tier: RecommendationTier::Recommended,
                },
                pt == "rust" || pt == "go" || pt == "mixed",
            ),
            // Node / frontend
            (
                "node",
                Candidate {
                    plugin_id: "halcon-ui-inspector",
                    display_name: "HALCON UI Inspector",
                    rationale: "Component analysis, accessibility audit, design tokens",
                    tier: RecommendationTier::Essential,
                },
                pt == "node" || pt == "mixed",
            ),
            (
                "node",
                Candidate {
                    plugin_id: "halcon-perf-analyzer",
                    display_name: "HALCON Perf Analyzer",
                    rationale: "Bundle size, lazy loading, render-blocking resources",
                    tier: RecommendationTier::Recommended,
                },
                pt == "node" || pt == "mixed",
            ),
            // Python
            (
                "python",
                Candidate {
                    plugin_id: "halcon-schema-oracle",
                    display_name: "HALCON Schema Oracle",
                    rationale: "DB schema analysis, migration health, index advisor",
                    tier: RecommendationTier::Recommended,
                },
                pt == "python" || pt == "mixed",
            ),
            // Any project with a git remote
            (
                "any",
                Candidate {
                    plugin_id: "halcon-api-sculptor",
                    display_name: "HALCON API Sculptor",
                    rationale: "REST API inventory and health checks",
                    tier: RecommendationTier::Optional,
                },
                has_git,
            ),
            // Any project with many manifest files (polyglot / infra)
            (
                "any",
                Candidate {
                    plugin_id: "halcon-otel-tracer",
                    display_name: "HALCON OTEL Tracer",
                    rationale: "OpenTelemetry coverage, metric inventory, log patterns",
                    tier: RecommendationTier::Optional,
                },
                many_manifests || pt == "mixed",
            ),
        ];

        // ── Deduplicate + build final list ────────────────────────────────────
        let mut seen: HashSet<&str> = HashSet::new();
        let mut recs: Vec<PluginRecommendation> = Vec::new();

        for (_, candidate, applies) in candidates {
            if !applies {
                continue;
            }
            if !seen.insert(candidate.plugin_id) {
                continue; // skip duplicate (mixed can trigger multiple rules)
            }

            let prior_reward = ucb1_rewards.get(candidate.plugin_id).copied();

            // UCB1 tier upgrade: high prior reward (> 0.7) upgrades by one level.
            let tier = if prior_reward.unwrap_or(0.0) > 0.70 {
                candidate.tier.upgrade()
            } else {
                candidate.tier
            };

            let already_installed = loaded_plugin_ids.contains(candidate.plugin_id);

            recs.push(PluginRecommendation {
                plugin_id: candidate.plugin_id.to_string(),
                display_name: candidate.display_name.to_string(),
                rationale: candidate.rationale.to_string(),
                tier,
                already_installed,
                prior_reward,
            });
        }

        // Sort: Essential first, then Recommended, Optional, Experimental.
        recs.sort_by(|a, b| a.tier.cmp(&b.tier));
        recs
    }

    /// Human-readable report for /plugins suggest ClassicSink output.
    pub fn format_report(recommendations: &[PluginRecommendation]) -> String {
        let mut out = String::new();
        out.push_str("Plugin Recommendations\n");
        out.push_str("══════════════════════\n");

        for tier in &[
            RecommendationTier::Essential,
            RecommendationTier::Recommended,
            RecommendationTier::Optional,
            RecommendationTier::Experimental,
        ] {
            let group: Vec<&PluginRecommendation> =
                recommendations.iter().filter(|r| &r.tier == tier).collect();
            if group.is_empty() {
                continue;
            }

            let header_icon = match tier {
                RecommendationTier::Essential => "◆",
                RecommendationTier::Recommended => "◇",
                RecommendationTier::Optional => "▷",
                RecommendationTier::Experimental => "○",
            };

            out.push_str(&format!(
                "\n{} {} ({})\n",
                header_icon,
                tier.as_str(),
                group.len()
            ));
            for rec in &group {
                let installed_tag = if rec.already_installed {
                    " [installed]"
                } else {
                    ""
                };
                out.push_str(&format!(
                    "  • {}{} — {}\n",
                    rec.plugin_id, installed_tag, rec.rationale
                ));
            }
        }

        out.push_str("\nRun /plugins auto to install Essential + Recommended.\n");
        out
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repl::project_inspector::ProjectAnalysis;

    fn empty_analysis(project_type: &str) -> ProjectAnalysis {
        ProjectAnalysis {
            project_type: project_type.to_string(),
            ..ProjectAnalysis::default()
        }
    }

    fn analysis_with_git(project_type: &str) -> ProjectAnalysis {
        ProjectAnalysis {
            project_type: project_type.to_string(),
            git_remote: Some("https://github.com/org/repo".to_string()),
            ..ProjectAnalysis::default()
        }
    }

    fn many_manifest_analysis(project_type: &str) -> ProjectAnalysis {
        ProjectAnalysis {
            project_type: project_type.to_string(),
            manifest_files: vec![
                "Cargo.toml".into(),
                "package.json".into(),
                "docker-compose.yml".into(),
                "Makefile".into(),
                "go.mod".into(),
            ],
            ..ProjectAnalysis::default()
        }
    }

    #[test]
    fn rust_project_recommends_dependency_auditor() {
        let analysis = empty_analysis("rust");
        let recs = PluginRecommendationEngine::recommend(
            &analysis,
            &HashSet::new(),
            &HashMap::new(),
        );
        let auditor = recs.iter().find(|r| r.plugin_id == "halcon-dependency-auditor");
        assert!(auditor.is_some(), "dependency auditor should be recommended for rust");
        assert_eq!(auditor.unwrap().tier, RecommendationTier::Essential);
    }

    #[test]
    fn node_project_recommends_ui_inspector() {
        let analysis = empty_analysis("node");
        let recs = PluginRecommendationEngine::recommend(
            &analysis,
            &HashSet::new(),
            &HashMap::new(),
        );
        let inspector = recs.iter().find(|r| r.plugin_id == "halcon-ui-inspector");
        assert!(inspector.is_some(), "ui-inspector should be recommended for node");
        assert_eq!(inspector.unwrap().tier, RecommendationTier::Essential);
    }

    #[test]
    fn already_installed_plugins_marked_correctly() {
        let analysis = empty_analysis("rust");
        let mut loaded: HashSet<String> = HashSet::new();
        loaded.insert("halcon-dependency-auditor".to_string());

        let recs = PluginRecommendationEngine::recommend(&analysis, &loaded, &HashMap::new());
        let auditor = recs.iter().find(|r| r.plugin_id == "halcon-dependency-auditor").unwrap();
        assert!(auditor.already_installed, "should be marked as installed");
    }

    #[test]
    fn ucb1_high_reward_upgrades_tier() {
        // halcon-api-sculptor starts as Optional (git_remote trigger)
        let analysis = analysis_with_git("rust");
        let mut rewards: HashMap<String, f64> = HashMap::new();
        rewards.insert("halcon-api-sculptor".to_string(), 0.85);

        let recs = PluginRecommendationEngine::recommend(&analysis, &HashSet::new(), &rewards);
        let sculptor = recs.iter().find(|r| r.plugin_id == "halcon-api-sculptor").unwrap();
        // Optional (0.85 > 0.70) → upgrades to Recommended
        assert_eq!(sculptor.tier, RecommendationTier::Recommended);
    }

    #[test]
    fn unknown_project_returns_otel_optional() {
        // unknown project + many manifests → otel-tracer should NOT appear
        // (many_manifests requires >= 5 files)
        let analysis = many_manifest_analysis("unknown");
        let recs = PluginRecommendationEngine::recommend(
            &analysis,
            &HashSet::new(),
            &HashMap::new(),
        );
        let otel = recs.iter().find(|r| r.plugin_id == "halcon-otel-tracer");
        assert!(otel.is_some(), "otel-tracer should appear when many_manifests");
        assert_eq!(otel.unwrap().tier, RecommendationTier::Optional);
    }

    #[test]
    fn format_report_contains_tier_headers() {
        let analysis = empty_analysis("rust");
        let recs = PluginRecommendationEngine::recommend(
            &analysis,
            &HashSet::new(),
            &HashMap::new(),
        );
        let report = PluginRecommendationEngine::format_report(&recs);
        assert!(report.contains("Essential"), "report should contain Essential tier");
        assert!(report.contains("Recommended"), "report should contain Recommended tier");
    }
}
