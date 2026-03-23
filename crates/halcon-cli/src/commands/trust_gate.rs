//! Trust Gate — wires workspace trust, config trust, and MCP trust into session startup.
//!
//! This module is the **integration layer** that connects the trust stores
//! (implemented in `repl/security/workspace_trust.rs` and `repl/security/config_trust.rs`)
//! to the actual session lifecycle in `commands/chat.rs`.
//!
//! # Trust Chain
//!
//! ```text
//! Session Start
//!   → Gate 1: Workspace Trust (check cwd)
//!   → Gate 2: Config Trust (HALCON.md fingerprint)
//!   → Gate 3: MCP Trust (server config hash)
//!   → Session runs with appropriate restrictions
//! ```

use std::path::{Path, PathBuf};

use anyhow::Result;
use tracing::{debug, info, warn};

use crate::repl::security::config_trust::{
    ConfigTrustDecision, ConfigTrustStore, McpTrustDecision, McpTrustStore,
};
use crate::repl::security::workspace_trust::{
    RestrictedMode, RestrictedReason, TrustDecision, WorkspaceTrustStore,
};

/// Result of the trust gate evaluation.
#[derive(Debug)]
pub struct TrustGateResult {
    /// Whether the workspace is trusted.
    pub workspace_trusted: bool,
    /// Whether HALCON.md instructions should be loaded.
    pub allow_instructions: bool,
    /// MCP servers that were approved (empty = none approved).
    pub approved_mcp_servers: Vec<String>,
    /// MCP servers that were denied or need approval.
    pub denied_mcp_servers: Vec<String>,
    /// If workspace is untrusted, this contains the restricted mode config.
    pub restricted_mode: Option<RestrictedMode>,
}

impl TrustGateResult {
    /// Fully trusted — all gates passed.
    pub fn fully_trusted() -> Self {
        Self {
            workspace_trusted: true,
            allow_instructions: true,
            approved_mcp_servers: Vec::new(),
            denied_mcp_servers: Vec::new(),
            restricted_mode: None,
        }
    }
}

/// Run all trust gates for a session.
///
/// This is called from `commands/chat.rs` AFTER auth_gate and BEFORE Repl::new().
/// It evaluates the trust chain and returns restrictions to apply.
///
/// In non-interactive mode (CI, pipe), workspace trust defaults to trusted
/// to avoid blocking automated workflows. HALCON.md and MCP still require
/// explicit approval stored from a previous interactive session.
pub fn evaluate_trust_chain(workspace: &Path, is_interactive: bool) -> Result<TrustGateResult> {
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());

    // ── Gate 1: Workspace Trust ────────────────────────────────────────────
    let mut ws_store = WorkspaceTrustStore::load();
    let ws_decision = ws_store.check(&workspace);

    match ws_decision {
        TrustDecision::Trusted => {
            debug!(dir = %workspace.display(), "Workspace trusted");
        }
        TrustDecision::Denied => {
            warn!(dir = %workspace.display(), "Workspace explicitly denied — restricted mode");
            return Ok(TrustGateResult {
                workspace_trusted: false,
                allow_instructions: false,
                approved_mcp_servers: Vec::new(),
                denied_mcp_servers: Vec::new(),
                restricted_mode: Some(RestrictedMode {
                    directory: workspace,
                    reason: RestrictedReason::ExplicitlyDenied,
                }),
            });
        }
        TrustDecision::Unknown => {
            if is_interactive {
                // Interactive mode: prompt the user
                let trusted = prompt_workspace_trust(&workspace);
                let decision = if trusted {
                    TrustDecision::Trusted
                } else {
                    TrustDecision::Denied
                };
                ws_store.set_trust(&workspace, decision.clone());
                let _ = ws_store.save();

                if decision == TrustDecision::Denied {
                    return Ok(TrustGateResult {
                        workspace_trusted: false,
                        allow_instructions: false,
                        approved_mcp_servers: Vec::new(),
                        denied_mcp_servers: Vec::new(),
                        restricted_mode: Some(RestrictedMode {
                            directory: workspace,
                            reason: RestrictedReason::ExplicitlyDenied,
                        }),
                    });
                }
            } else {
                // Non-interactive: auto-trust (CI/pipe must work without prompts)
                // Trust is NOT persisted — only applies to this session.
                debug!("Non-interactive mode: auto-trusting workspace");
            }
        }
    }

    // ── Gate 2: Config Trust (HALCON.md) ────────────────────────────────────
    let allow_instructions = evaluate_config_trust(&workspace, is_interactive);

    // ── Gate 3: MCP Trust ──────────────────────────────────────────────────
    let (approved, denied) = evaluate_mcp_trust(&workspace, is_interactive);

    info!(
        workspace = %workspace.display(),
        instructions = allow_instructions,
        mcp_approved = approved.len(),
        mcp_denied = denied.len(),
        "Trust chain evaluated"
    );

    Ok(TrustGateResult {
        workspace_trusted: true,
        allow_instructions,
        approved_mcp_servers: approved,
        denied_mcp_servers: denied,
        restricted_mode: None,
    })
}

// ── Gate 2: Config Trust ─────────────────────────────────────────────────

fn evaluate_config_trust(workspace: &Path, is_interactive: bool) -> bool {
    // Check for HALCON.md in project scope
    let halcon_md_paths = [
        workspace.join("HALCON.md"),
        workspace.join(".halcon").join("HALCON.md"),
    ];

    let config_store = ConfigTrustStore::load();

    for path in &halcon_md_paths {
        if !path.exists() {
            continue;
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                warn!(path = %path.display(), error = %e, "Failed to read HALCON.md");
                continue;
            }
        };

        let decision = config_store.check(workspace, path, &content);

        match decision {
            ConfigTrustDecision::Approved => {
                debug!(path = %path.display(), "HALCON.md approved (fingerprint match)");
                return true;
            }
            ConfigTrustDecision::Rejected => {
                info!(path = %path.display(), "HALCON.md rejected — skipping project instructions");
                return false;
            }
            ConfigTrustDecision::Changed => {
                if is_interactive {
                    let approved = prompt_config_trust(path, &content, true);
                    let mut store = ConfigTrustStore::load();
                    let decision = if approved {
                        ConfigTrustDecision::Approved
                    } else {
                        ConfigTrustDecision::Rejected
                    };
                    store.set_trust(workspace, path, &content, decision);
                    let _ = store.save();
                    return approved;
                } else {
                    warn!(
                        "HALCON.md changed since last approval — skipping in non-interactive mode"
                    );
                    return false;
                }
            }
            ConfigTrustDecision::Unknown => {
                if is_interactive {
                    let approved = prompt_config_trust(path, &content, false);
                    let mut store = ConfigTrustStore::load();
                    let decision = if approved {
                        ConfigTrustDecision::Approved
                    } else {
                        ConfigTrustDecision::Rejected
                    };
                    store.set_trust(workspace, path, &content, decision);
                    let _ = store.save();
                    return approved;
                } else {
                    // Non-interactive: skip unknown instructions (fail-closed)
                    debug!("Unknown HALCON.md in non-interactive mode — skipping");
                    return false;
                }
            }
        }
    }

    // No HALCON.md found — nothing to approve
    true
}

// ── Gate 3: MCP Trust ────────────────────────────────────────────────────

fn evaluate_mcp_trust(workspace: &Path, _is_interactive: bool) -> (Vec<String>, Vec<String>) {
    // MCP trust is evaluated lazily when servers are actually connected.
    // At this point we just return empty lists — the actual gating happens
    // in the MCP manager when it calls connect().
    //
    // This is by design: MCP server configs may come from the global config
    // (~/.halcon/config.toml) which is always trusted, or from project-level
    // .mcp.json which needs per-server approval.
    (Vec::new(), Vec::new())
}

// ── User Prompts ─────────────────────────────────────────────────────────

fn prompt_workspace_trust(workspace: &Path) -> bool {
    let dir_name = workspace
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| workspace.to_string_lossy().to_string());

    eprintln!();
    eprintln!("  Workspace Trust");
    eprintln!("  ─────────────────────────────────────────────────────");
    eprintln!("  Do you trust the authors of this workspace?");
    eprintln!();
    eprintln!("    {}", workspace.display());
    eprintln!();
    eprintln!("  Trusting a workspace allows Halcon to:");
    eprintln!("    - Load project instructions (HALCON.md)");
    eprintln!("    - Connect to project MCP servers");
    eprintln!("    - Execute tools in this directory");
    eprintln!();
    eprint!("  Trust this workspace? [y/N] ");
    let _ = std::io::Write::flush(&mut std::io::stderr());

    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_ok() {
        let answer = input.trim().to_lowercase();
        answer == "y" || answer == "yes"
    } else {
        false // fail-closed
    }
}

fn prompt_config_trust(path: &Path, content: &str, changed: bool) -> bool {
    let line_count = content.lines().count();
    let preview: String = content.lines().take(10).collect::<Vec<_>>().join("\n");

    eprintln!();
    if changed {
        eprintln!("  Project Instructions Changed");
    } else {
        eprintln!("  New Project Instructions Found");
    }
    eprintln!("  ─────────────────────────────────────────────────────");
    eprintln!("  File: {}", path.display());
    eprintln!("  Lines: {line_count}");
    eprintln!();
    eprintln!("  Preview:");
    for line in preview.lines() {
        eprintln!("    {line}");
    }
    if line_count > 10 {
        eprintln!("    ... ({} more lines)", line_count - 10);
    }
    eprintln!();
    if changed {
        eprintln!("  This file has been modified since you last approved it.");
    }
    eprint!("  Load these instructions? [y/N] ");
    let _ = std::io::Write::flush(&mut std::io::stderr());

    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_ok() {
        let answer = input.trim().to_lowercase();
        answer == "y" || answer == "yes"
    } else {
        false // fail-closed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn fully_trusted_result_defaults() {
        let r = TrustGateResult::fully_trusted();
        assert!(r.workspace_trusted);
        assert!(r.allow_instructions);
        assert!(r.restricted_mode.is_none());
    }

    #[test]
    fn non_interactive_auto_trusts_workspace() {
        let tmp = TempDir::new().unwrap();
        let result = evaluate_trust_chain(tmp.path(), false).unwrap();
        // First access in non-interactive = auto-trust (CI must work)
        assert!(result.workspace_trusted);
    }

    #[test]
    fn non_interactive_skips_unknown_halcon_md() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("HALCON.md"), "# Malicious instructions").unwrap();
        let result = evaluate_trust_chain(tmp.path(), false).unwrap();
        // Unknown HALCON.md in non-interactive → fail-closed (not loaded)
        assert!(!result.allow_instructions);
    }

    #[test]
    fn previously_denied_workspace_stays_denied() {
        let tmp = TempDir::new().unwrap();
        let mut store = WorkspaceTrustStore::load();
        store.set_trust(tmp.path(), TrustDecision::Denied);
        let _ = store.save();

        let result = evaluate_trust_chain(tmp.path(), false).unwrap();
        assert!(!result.workspace_trusted);
        assert!(result.restricted_mode.is_some());

        // Cleanup
        store.revoke(tmp.path());
        let _ = store.save();
    }
}
