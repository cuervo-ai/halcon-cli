//! CLI command handlers for `halcon audit`.
//!
//! Subcommands:
//!   export     — export session audit events as JSONL, CSV, or PDF
//!   list       — list all sessions with compliance summary stats
//!   verify     — verify the HMAC-SHA256 hash chain for a session
//!   compliance — generate a compliance report (SOC 2 / FedRAMP / ISO 27001)

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::audit::{compliance::ComplianceFormat, AuditExporter, ExportFormat, ExportOptions};
use crate::config_loader::default_db_path;

/// `halcon audit export`
pub fn export(
    session: Option<String>,
    since: Option<String>,
    format: &str,
    output: Option<PathBuf>,
    include_tool_inputs: bool,
    include_tool_outputs: bool,
    db_path: Option<PathBuf>,
) -> Result<()> {
    let fmt = ExportFormat::from_str(format)?;
    let db = db_path.unwrap_or_else(default_db_path);

    if session.is_none() && since.is_none() {
        return Err(anyhow::anyhow!(
            "Specify --session <UUID> to export one session, or --since <ISO-8601> to export a time range."
        ));
    }

    let exporter = AuditExporter::new(db);
    let opts = ExportOptions {
        session_id: session,
        since,
        format: fmt,
        output,
        include_tool_inputs,
        include_tool_outputs,
    };
    exporter.export(&opts)
}

/// `halcon audit list`
pub fn list(db_path: Option<PathBuf>, json: bool) -> Result<()> {
    let db = db_path.unwrap_or_else(default_db_path);
    let exporter = AuditExporter::new(db);
    let summaries = exporter.list()?;

    if summaries.is_empty() {
        println!("No sessions found.");
        return Ok(());
    }

    if json {
        let s = serde_json::to_string_pretty(&summaries)?;
        println!("{s}");
    } else {
        println!("{}", crate::audit::summary::SessionSummary::display_header());
        println!("{}", "─".repeat(110));
        for s in &summaries {
            println!("{}", s.display_row());
        }
        println!("\n{} session(s) total.", summaries.len());
    }
    Ok(())
}

/// `halcon audit verify <session-id>`
pub fn verify(session_id: &str, db_path: Option<PathBuf>) -> Result<()> {
    let db = db_path.unwrap_or_else(default_db_path);
    let exporter = AuditExporter::new(db);
    let report = exporter.verify(session_id)?;
    report.print_summary();
    if !report.chain_intact {
        std::process::exit(1);
    }
    Ok(())
}

/// `halcon audit compliance --format soc2|fedramp|iso27001 --output /path/to/report.pdf`
///
/// Generates a self-contained PDF compliance report from existing audit data.
/// No new instrumentation required — reads from the existing halcon.db.
pub fn compliance(
    format_str: &str,
    output: Option<PathBuf>,
    from: Option<String>,
    to: Option<String>,
    db_path: Option<PathBuf>,
) -> Result<()> {
    let fmt = ComplianceFormat::from_str(format_str)?;
    let db = db_path.unwrap_or_else(default_db_path);

    // Default date range: last 30 days.
    let from_date = from.unwrap_or_else(|| {
        (chrono::Utc::now() - chrono::Duration::days(30))
            .format("%Y-%m-%d")
            .to_string()
    });
    let to_date = to.unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());

    // Default output path.
    let out_path = output.unwrap_or_else(|| {
        PathBuf::from(format!(
            "halcon-compliance-{}-{}.pdf",
            format_str.to_lowercase(),
            to_date
        ))
    });

    eprintln!(
        "Generating {} compliance report for period {} to {}...",
        fmt.display_name(), from_date, to_date
    );

    // Collect events in date range using existing query infrastructure.
    let conn = crate::audit::query::open_db(&db)?;
    let events = crate::audit::query::collect_events_since(&conn, &from_date, false, false)?;
    let sessions = crate::audit::query::list_sessions(&conn)?;

    eprintln!("Found {} events across {} sessions.", events.len(), sessions.len());

    let file = std::fs::File::create(&out_path)
        .with_context(|| format!("Cannot create output file: {}", out_path.display()))?;

    crate::audit::compliance::generate_compliance_report(
        std::io::BufWriter::new(file),
        &events,
        &sessions,
        fmt,
        &from_date,
        &to_date,
        "Halcon CLI",
    )?;

    eprintln!("Compliance report written to: {}", out_path.display());
    Ok(())
}
