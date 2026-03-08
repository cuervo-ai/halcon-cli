//! Compliance and audit export package (Feature 8).
//!
//! Entry point for `halcon audit` subcommands.  Queries existing SQLite tables
//! (no new instrumentation) and produces SOC 2-compatible exports in three
//! formats: JSONL, CSV, and PDF.
//!
//! ## Architecture
//!
//! ```text
//! audit/
//!   events.rs          — AuditEvent struct + SOC2 event-type taxonomy
//!   query.rs           — read-only SQLite queries across all audit tables
//!   summary.rs         — SessionSummary for `halcon audit list`
//!   export_jsonl.rs    — JSONL writer (SIEM-ready)
//!   export_csv.rs      — CSV writer (fixed-schema, RFC 4180)
//!   export_pdf.rs      — PDF report generator (printpdf, pure Rust)
//!   integrity.rs       — HMAC-SHA256 hash chain verifier
//!   mod.rs             — AuditExporter public façade
//! ```

pub mod compliance;
pub mod events;
pub mod export_csv;
pub mod export_jsonl;
pub mod export_pdf;
pub mod integrity;
pub mod query;
pub mod summary;

use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use events::AuditEvent;
use summary::SessionSummary;

/// Output format for `halcon audit export`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Jsonl,
    Csv,
    Pdf,
}

impl ExportFormat {
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "jsonl" | "json" => Ok(Self::Jsonl),
            "csv" => Ok(Self::Csv),
            "pdf" => Ok(Self::Pdf),
            other => Err(anyhow::anyhow!(
                "Unknown format '{other}'. Use: jsonl, csv, pdf"
            )),
        }
    }

    pub fn default_extension(&self) -> &'static str {
        match self {
            Self::Jsonl => "jsonl",
            Self::Csv => "csv",
            Self::Pdf => "pdf",
        }
    }
}

/// Options for `halcon audit export`.
pub struct ExportOptions {
    /// Session UUID (exclusive with `since`).
    pub session_id: Option<String>,
    /// ISO-8601 timestamp — export all sessions starting after this time.
    pub since: Option<String>,
    /// Output format.
    pub format: ExportFormat,
    /// Output file path (None = stdout for JSONL/CSV; required for PDF).
    pub output: Option<PathBuf>,
    /// Include raw tool inputs in payload.
    pub include_tool_inputs: bool,
    /// Include raw tool outputs in payload.
    pub include_tool_outputs: bool,
}

/// High-level façade used by `commands/audit.rs`.
pub struct AuditExporter {
    db_path: PathBuf,
}

impl AuditExporter {
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    /// Export audit events according to `opts`.
    pub fn export(&self, opts: &ExportOptions) -> Result<()> {
        let conn = query::open_db(&self.db_path)?;

        let events: Vec<AuditEvent> = if let Some(sid) = &opts.session_id {
            query::collect_events_for_session(
                &conn,
                sid,
                opts.include_tool_inputs,
                opts.include_tool_outputs,
            )
            .with_context(|| format!("Failed to collect events for session {sid}"))?
        } else if let Some(since) = &opts.since {
            query::collect_events_since(
                &conn,
                since,
                opts.include_tool_inputs,
                opts.include_tool_outputs,
            )
            .with_context(|| format!("Failed to collect events since {since}"))?
        } else {
            return Err(anyhow::anyhow!(
                "Specify --session <ID> or --since <TIMESTAMP>"
            ));
        };

        if events.is_empty() {
            eprintln!("No audit events found matching the given criteria.");
            return Ok(());
        }

        match opts.format {
            ExportFormat::Jsonl => {
                if let Some(path) = &opts.output {
                    let file = fs::File::create(path)
                        .with_context(|| format!("Cannot create {}", path.display()))?;
                    let mut writer = BufWriter::new(file);
                    export_jsonl::write_jsonl(&mut writer, &events)?;
                    eprintln!("Exported {} events to {}", events.len(), path.display());
                } else {
                    let stdout = std::io::stdout();
                    let mut writer = stdout.lock();
                    export_jsonl::write_jsonl(&mut writer, &events)?;
                }
            }
            ExportFormat::Csv => {
                if let Some(path) = &opts.output {
                    let file = fs::File::create(path)
                        .with_context(|| format!("Cannot create {}", path.display()))?;
                    let mut writer = BufWriter::new(file);
                    export_csv::write_csv(&mut writer, &events)?;
                    eprintln!("Exported {} events to {}", events.len(), path.display());
                } else {
                    let stdout = std::io::stdout();
                    let mut writer = stdout.lock();
                    export_csv::write_csv(&mut writer, &events)?;
                }
            }
            ExportFormat::Pdf => {
                let path = opts.output.clone().unwrap_or_else(|| {
                    PathBuf::from(format!(
                        "halcon-audit-{}.pdf",
                        &opts
                            .session_id
                            .as_deref()
                            .unwrap_or("export")
                            .chars()
                            .take(8)
                            .collect::<String>()
                    ))
                });
                let file = fs::File::create(&path)
                    .with_context(|| format!("Cannot create {}", path.display()))?;

                let summaries: Vec<SessionSummary> = if let Some(sid) = &opts.session_id {
                    query::session_summary(&conn, sid)?
                        .into_iter()
                        .collect()
                } else {
                    query::list_sessions(&conn)?
                };

                let export_ts = chrono::Utc::now().to_rfc3339();
                let title = format!(
                    "Halcon Audit Report — {}",
                    opts.session_id.as_deref().unwrap_or("all sessions")
                );
                let writer = std::io::BufWriter::new(file);
                export_pdf::write_pdf(writer, &events, &summaries, &title, &export_ts)?;
                eprintln!("Exported {} events to {}", events.len(), path.display());
            }
        }

        Ok(())
    }

    /// List all sessions with summary stats.
    pub fn list(&self) -> Result<Vec<SessionSummary>> {
        let conn = query::open_db(&self.db_path)?;
        query::list_sessions(&conn)
    }

    /// Verify the hash chain for a session and return a report.
    pub fn verify(&self, session_id: &str) -> Result<integrity::VerifyReport> {
        let conn = query::open_db(&self.db_path)?;
        integrity::verify_chain(&conn, session_id, true)
    }

    /// Path to the database being used.
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }
}
