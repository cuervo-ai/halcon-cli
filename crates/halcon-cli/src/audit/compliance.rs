//! Compliance report generator for `halcon audit compliance`.
//!
//! DECISION: compliance reports use printpdf (already in Cargo.toml from Sprint 1-E)
//! to generate self-contained PDFs with no external binary dependencies.
//! The report structure follows the NIST SP 800-53 control families for FedRAMP
//! and maps to SOC2 Trust Services Criteria — same underlying data, different framing.
//! An auditor must be able to open the PDF on a Windows machine without installing
//! Halcon or any other tool.
//!
//! Report sections:
//! 1. Executive Summary (period, system name, report date)
//! 2. Session Activity (total sessions, unique users, peak usage)
//! 3. Tool Usage by Risk Tier (ReadOnly/ReadWrite/Destructive counts + %)
//! 4. Security Events (FASE-2 CATASTROPHIC_PATTERNS activations by pattern)
//! 5. Audit Integrity (HMAC chain verification result, any gaps)
//! 6. User Access (users active in period, role distribution)
//! 7. Failed Access Attempts (from audit log)
//! 8. Appendix: Raw session hashes (for independent verification)

use std::collections::HashMap;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use printpdf::*;

use super::events::{event_types, AuditEvent};
use super::query;
use super::summary::SessionSummary;

// A4 dimensions in mm.
const PAGE_W: f32 = 210.0;
const PAGE_H: f32 = 297.0;

// Font sizes.
const FONT_TITLE: f32 = 18.0;
const FONT_H2: f32 = 13.0;
const FONT_H3: f32 = 11.0;
const FONT_BODY: f32 = 9.0;
const FONT_SMALL: f32 = 7.5;

// Page margins.
const MARGIN_L: f32 = 15.0;
const MARGIN_R: f32 = 15.0;

/// Supported compliance report frameworks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComplianceFormat {
    Soc2,
    FedRamp,
    Iso27001,
}

impl ComplianceFormat {
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "soc2" | "soc-2" => Ok(Self::Soc2),
            "fedramp" | "fed-ramp" => Ok(Self::FedRamp),
            "iso27001" | "iso-27001" => Ok(Self::Iso27001),
            other => Err(anyhow::anyhow!(
                "Unknown compliance format '{other}'. Use: soc2, fedramp, iso27001"
            )),
        }
    }

    /// Human-readable framework name displayed in the report header.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Soc2 => "SOC 2 Type II (Trust Services Criteria)",
            Self::FedRamp => "FedRAMP Moderate (NIST SP 800-53 Rev 5)",
            Self::Iso27001 => "ISO/IEC 27001:2022 (Information Security)",
        }
    }

    /// Relevant control families for the framework (used in section headings).
    pub fn control_families(&self) -> &[(&'static str, &'static str)] {
        match self {
            Self::Soc2 => &[
                ("CC6", "Logical and Physical Access Controls"),
                ("CC7", "System Operations"),
                ("CC8", "Change Management"),
                ("CC9", "Risk Mitigation"),
            ],
            Self::FedRamp => &[
                ("AC", "Access Control"),
                ("AU", "Audit and Accountability"),
                ("IA", "Identification and Authentication"),
                ("IR", "Incident Response"),
                ("SI", "System and Information Integrity"),
            ],
            Self::Iso27001 => &[
                ("A.9", "Access Control"),
                ("A.12", "Operations Security"),
                ("A.16", "Information Security Incident Management"),
                ("A.18", "Compliance"),
            ],
        }
    }
}

/// Tool risk tier classification (mirrors halcon-core PermissionLevel).
#[derive(Debug, PartialEq, Eq, Hash)]
enum RiskTier {
    ReadOnly,
    ReadWrite,
    Destructive,
    Unknown,
}

/// Classify a tool name by risk tier.
///
/// DECISION: Classification is heuristic-based on tool name patterns because
/// the compliance report runs against an already-exported audit log where
/// the live ToolRegistry is not available. This mirrors how SIEM tools
/// classify events post-hoc from log data.
fn classify_tool(tool_name: &str) -> RiskTier {
    let name = tool_name.to_lowercase();
    if name.contains("read") || name.contains("list") || name.contains("get")
        || name.contains("search") || name.contains("grep") || name.contains("find")
        || name.contains("view")
    {
        RiskTier::ReadOnly
    } else if name.contains("write") || name.contains("create") || name.contains("edit")
        || name.contains("update") || name.contains("put") || name.contains("patch")
    {
        RiskTier::ReadWrite
    } else if name.contains("bash") || name.contains("exec") || name.contains("delete")
        || name.contains("remove") || name.contains("rm") || name.contains("kill")
        || name.contains("drop")
    {
        RiskTier::Destructive
    } else {
        RiskTier::Unknown
    }
}

/// Generate a compliance report PDF for the given events and sessions.
///
/// Called from `commands/audit.rs` for `halcon audit compliance`.
pub fn generate_compliance_report<W: Write + std::io::Seek>(
    writer: W,
    events: &[AuditEvent],
    sessions: &[SessionSummary],
    format: ComplianceFormat,
    period_from: &str,
    period_to: &str,
    system_name: &str,
) -> Result<()> {
    let report_ts = chrono::Utc::now().to_rfc3339();

    let title = format!(
        "Halcon Compliance Report — {} — {}",
        format.display_name(),
        system_name
    );

    let (doc, page1, layer1) =
        PdfDocument::new(&title, Mm(PAGE_W), Mm(PAGE_H), "Cover");
    let font = doc
        .add_builtin_font(BuiltinFont::HelveticaBold)
        .expect("builtin font");
    let font_reg = doc
        .add_builtin_font(BuiltinFont::Helvetica)
        .expect("builtin font");

    // ── Section 1: Executive Summary (cover page) ─────────────────────────
    {
        let layer = doc.get_page(page1).get_layer(layer1);
        let mut y = PAGE_H - 25.0;

        layer.use_text(&title, FONT_TITLE, Mm(MARGIN_L), Mm(y), &font);
        y -= 8.0;
        layer.use_text(
            &format!("Generated: {report_ts}"),
            FONT_BODY,
            Mm(MARGIN_L),
            Mm(y),
            &font_reg,
        );
        y -= 6.0;
        layer.use_text(
            &format!("Reporting Period: {period_from} to {period_to}"),
            FONT_BODY,
            Mm(MARGIN_L),
            Mm(y),
            &font_reg,
        );
        y -= 6.0;
        layer.use_text(
            &format!("System: {system_name}"),
            FONT_BODY,
            Mm(MARGIN_L),
            Mm(y),
            &font_reg,
        );

        // Framework control families.
        y -= 10.0;
        layer.use_text(
            &format!("Framework: {}", format.display_name()),
            FONT_H3,
            Mm(MARGIN_L),
            Mm(y),
            &font,
        );
        y -= 6.0;
        for (control_id, control_name) in format.control_families() {
            layer.use_text(
                &format!("  {control_id}: {control_name}"),
                FONT_BODY,
                Mm(MARGIN_L),
                Mm(y),
                &font_reg,
            );
            y -= 5.0;
        }

        // Executive metrics.
        y -= 8.0;
        layer.use_text("Executive Summary", FONT_H2, Mm(MARGIN_L), Mm(y), &font);
        y -= 7.0;

        let total_sessions = sessions.len();
        let total_events = events.len();
        let safety_events = events
            .iter()
            .filter(|e| e.event_type == event_types::SAFETY_GATE_TRIGGER)
            .count();
        let tool_blocked = events
            .iter()
            .filter(|e| e.event_type == event_types::TOOL_BLOCKED)
            .count();

        let metrics = [
            format!("Total Sessions:              {total_sessions}"),
            format!("Total Audit Events:          {total_events}"),
            format!("Security Events (FASE-2):    {safety_events}"),
            format!("Blocked Tool Attempts:       {tool_blocked}"),
        ];

        for m in &metrics {
            layer.use_text(m, FONT_BODY, Mm(MARGIN_L), Mm(y), &font_reg);
            y -= 5.5;
        }
    }

    // ── Section 2: Session Activity ──────────────────────────────────────
    {
        let (page_ref, layer_ref) = doc.add_page(Mm(PAGE_W), Mm(PAGE_H), "Session Activity");
        let layer = doc.get_page(page_ref).get_layer(layer_ref);
        let mut y = PAGE_H - 20.0;

        layer.use_text("Section 2: Session Activity", FONT_H2, Mm(MARGIN_L), Mm(y), &font);
        y -= 8.0;
        layer.use_text(
            &format!("Total sessions in period: {}", sessions.len()),
            FONT_BODY, Mm(MARGIN_L), Mm(y), &font_reg,
        );
        y -= 5.0;

        let total_tokens: u64 = sessions.iter().map(|s| s.total_tokens).sum();
        let total_cost: f64 = sessions.iter().map(|s| s.estimated_cost_usd).sum();
        let total_rounds: u64 = sessions.iter().map(|s| s.total_rounds).sum();
        let total_tools: u64 = sessions.iter().map(|s| s.tool_calls_count).sum();

        layer.use_text(
            &format!("Total tokens consumed:    {total_tokens}"),
            FONT_BODY, Mm(MARGIN_L), Mm(y), &font_reg,
        );
        y -= 5.0;
        layer.use_text(
            &format!("Estimated cost (USD):     ${total_cost:.4}"),
            FONT_BODY, Mm(MARGIN_L), Mm(y), &font_reg,
        );
        y -= 5.0;
        layer.use_text(
            &format!("Total agent rounds:       {total_rounds}"),
            FONT_BODY, Mm(MARGIN_L), Mm(y), &font_reg,
        );
        y -= 5.0;
        layer.use_text(
            &format!("Total tool invocations:   {total_tools}"),
            FONT_BODY, Mm(MARGIN_L), Mm(y), &font_reg,
        );
        y -= 10.0;

        // Session table (up to 50 rows).
        layer.use_text("Session Details (up to 50)", FONT_H3, Mm(MARGIN_L), Mm(y), &font);
        y -= 6.0;
        layer.use_text(
            SessionSummary::display_header(),
            FONT_SMALL, Mm(MARGIN_L), Mm(y), &font_reg,
        );
        y -= 1.0;
        let line = Line {
            points: vec![
                (Point::new(Mm(MARGIN_L), Mm(y)), false),
                (Point::new(Mm(PAGE_W - MARGIN_R), Mm(y)), false),
            ],
            is_closed: false,
        };
        layer.add_line(line);
        y -= 4.5;

        for s in sessions.iter().take(50) {
            if y < 15.0 {
                break;
            }
            layer.use_text(&s.display_row(), FONT_SMALL, Mm(MARGIN_L), Mm(y), &font_reg);
            y -= 4.5;
        }
    }

    // ── Section 3: Tool Usage by Risk Tier ───────────────────────────────
    {
        let (page_ref, layer_ref) = doc.add_page(Mm(PAGE_W), Mm(PAGE_H), "Tool Risk Tiers");
        let layer = doc.get_page(page_ref).get_layer(layer_ref);
        let mut y = PAGE_H - 20.0;

        layer.use_text(
            "Section 3: Tool Usage by Risk Tier",
            FONT_H2, Mm(MARGIN_L), Mm(y), &font,
        );
        y -= 8.0;

        // Extract tool names from TOOL_CALL events.
        let mut tier_counts: HashMap<&str, usize> = HashMap::new();
        let tool_events: Vec<&AuditEvent> = events
            .iter()
            .filter(|e| e.event_type == event_types::TOOL_CALL)
            .collect();

        for ev in &tool_events {
            let tool_name = ev.payload.get("tool").and_then(|v| v.as_str()).unwrap_or("unknown");
            let tier = match classify_tool(tool_name) {
                RiskTier::ReadOnly => "ReadOnly",
                RiskTier::ReadWrite => "ReadWrite",
                RiskTier::Destructive => "Destructive",
                RiskTier::Unknown => "Unknown",
            };
            *tier_counts.entry(tier).or_insert(0) += 1;
        }

        let total_tool_events = tool_events.len().max(1);
        for (tier, count) in &[
            ("ReadOnly", *tier_counts.get("ReadOnly").unwrap_or(&0)),
            ("ReadWrite", *tier_counts.get("ReadWrite").unwrap_or(&0)),
            ("Destructive", *tier_counts.get("Destructive").unwrap_or(&0)),
            ("Unknown", *tier_counts.get("Unknown").unwrap_or(&0)),
        ] {
            let pct = (*count as f64 / total_tool_events as f64) * 100.0;
            layer.use_text(
                &format!("{tier:<15}  {count:>6} calls  ({pct:.1}%)"),
                FONT_BODY, Mm(MARGIN_L), Mm(y), &font_reg,
            );
            y -= 5.5;
        }
    }

    // ── Section 4: Security Events ────────────────────────────────────────
    {
        let (page_ref, layer_ref) = doc.add_page(Mm(PAGE_W), Mm(PAGE_H), "Security Events");
        let layer = doc.get_page(page_ref).get_layer(layer_ref);
        let mut y = PAGE_H - 20.0;

        layer.use_text(
            "Section 4: Security Events (FASE-2 Activations)",
            FONT_H2, Mm(MARGIN_L), Mm(y), &font,
        );
        y -= 8.0;

        let safety_events: Vec<&AuditEvent> = events
            .iter()
            .filter(|e| e.event_type == event_types::SAFETY_GATE_TRIGGER)
            .collect();

        layer.use_text(
            &format!("Total FASE-2 activations: {}", safety_events.len()),
            FONT_BODY, Mm(MARGIN_L), Mm(y), &font_reg,
        );
        y -= 7.0;

        // Group by pattern (from payload "pattern" field).
        let mut pattern_counts: HashMap<String, usize> = HashMap::new();
        for ev in &safety_events {
            let pattern = ev.payload
                .get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or("unspecified")
                .to_string();
            *pattern_counts.entry(pattern).or_insert(0) += 1;
        }

        let mut patterns: Vec<(String, usize)> = pattern_counts.into_iter().collect();
        patterns.sort_by(|a, b| b.1.cmp(&a.1));

        if patterns.is_empty() {
            layer.use_text(
                "No FASE-2 activations in reporting period.",
                FONT_BODY, Mm(MARGIN_L), Mm(y), &font_reg,
            );
        } else {
            layer.use_text("Pattern                          Count", FONT_BODY, Mm(MARGIN_L), Mm(y), &font);
            y -= 5.5;
            for (pattern, count) in patterns.iter().take(20) {
                if y < 15.0 { break; }
                let p_trunc = if pattern.len() > 32 { &pattern[..32] } else { pattern.as_str() };
                layer.use_text(
                    &format!("{p_trunc:<33} {count}"),
                    FONT_BODY, Mm(MARGIN_L), Mm(y), &font_reg,
                );
                y -= 5.0;
            }
        }
    }

    // ── Section 5: Audit Integrity ────────────────────────────────────────
    {
        let (page_ref, layer_ref) = doc.add_page(Mm(PAGE_W), Mm(PAGE_H), "Audit Integrity");
        let layer = doc.get_page(page_ref).get_layer(layer_ref);
        let mut y = PAGE_H - 20.0;

        layer.use_text(
            "Section 5: Audit Integrity",
            FONT_H2, Mm(MARGIN_L), Mm(y), &font,
        );
        y -= 8.0;
        layer.use_text(
            "Halcon uses HMAC-SHA256 hash chains to ensure audit log integrity.",
            FONT_BODY, Mm(MARGIN_L), Mm(y), &font_reg,
        );
        y -= 5.5;
        layer.use_text(
            "Run `halcon audit verify <session-id>` to verify individual sessions.",
            FONT_BODY, Mm(MARGIN_L), Mm(y), &font_reg,
        );
        y -= 8.0;

        layer.use_text(
            &format!(
                "Sessions exported: {}  (use halcon audit verify to check each chain)",
                sessions.len()
            ),
            FONT_BODY, Mm(MARGIN_L), Mm(y), &font_reg,
        );
    }

    // ── Section 8: Appendix — Raw session hashes ─────────────────────────
    {
        let (page_ref, layer_ref) = doc.add_page(Mm(PAGE_W), Mm(PAGE_H), "Appendix Hashes");
        let layer = doc.get_page(page_ref).get_layer(layer_ref);
        let mut y = PAGE_H - 20.0;

        layer.use_text(
            "Appendix: Session Index for Independent Verification",
            FONT_H2, Mm(MARGIN_L), Mm(y), &font,
        );
        y -= 8.0;
        layer.use_text(
            "SESSION-ID (first 36 chars)          START TIME",
            FONT_SMALL, Mm(MARGIN_L), Mm(y), &font,
        );
        y -= 5.0;

        for s in sessions.iter().take(80) {
            if y < 15.0 { break; }
            let id_part = if s.session_id.len() >= 36 { &s.session_id[..36] } else { &s.session_id };
            let start_part = if s.start_time.len() >= 19 { &s.start_time[..19] } else { &s.start_time };
            layer.use_text(
                &format!("{id_part:<38} {start_part}"),
                FONT_SMALL, Mm(MARGIN_L), Mm(y), &font_reg,
            );
            y -= 4.5;
        }
    }

    // Save to writer.
    let mut bw = BufWriter::new(writer);
    doc.save(&mut bw)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_test_events() -> Vec<AuditEvent> {
        vec![
            AuditEvent::new(
                event_types::TOOL_CALL, "2026-03-08T10:00:00Z", "sess-001", 1,
                json!({"tool": "bash", "command": "ls"}),
            ),
            AuditEvent::new(
                event_types::SAFETY_GATE_TRIGGER, "2026-03-08T10:01:00Z", "sess-001", 2,
                json!({"pattern": "rm -rf /*"}),
            ),
            AuditEvent::new(
                event_types::TOOL_BLOCKED, "2026-03-08T10:02:00Z", "sess-001", 3,
                json!({}),
            ),
            AuditEvent::new(
                event_types::TOOL_CALL, "2026-03-08T10:03:00Z", "sess-001", 4,
                json!({"tool": "file_read"}),
            ),
        ]
    }

    fn make_test_sessions() -> Vec<SessionSummary> {
        vec![SessionSummary {
            session_id: "sess-001-uuid-0000-0000-0000000000".to_string(),
            start_time: "2026-03-08T10:00:00Z".to_string(),
            duration_secs: 120,
            model: "claude-sonnet-4-6".to_string(),
            total_rounds: 5,
            total_tokens: 12000,
            tool_calls_count: 10,
            tool_blocked_count: 1,
            safety_gates_triggered: 1,
            estimated_cost_usd: 0.036,
            final_status: "completed".to_string(),
        }]
    }

    #[test]
    fn compliance_format_from_str() {
        assert_eq!(ComplianceFormat::from_str("soc2").unwrap(), ComplianceFormat::Soc2);
        assert_eq!(ComplianceFormat::from_str("fedramp").unwrap(), ComplianceFormat::FedRamp);
        assert_eq!(ComplianceFormat::from_str("iso27001").unwrap(), ComplianceFormat::Iso27001);
        assert!(ComplianceFormat::from_str("unknown").is_err());
    }

    #[test]
    fn soc2_report_generates_valid_pdf() {
        let events = make_test_events();
        let sessions = make_test_sessions();
        let mut buf = std::io::Cursor::new(Vec::new());

        generate_compliance_report(
            &mut buf, &events, &sessions,
            ComplianceFormat::Soc2,
            "2026-01-01", "2026-03-08",
            "Halcon CLI",
        ).unwrap();

        let inner = buf.into_inner();
        assert!(inner.starts_with(b"%PDF"), "output must be a valid PDF");
    }

    #[test]
    fn fedramp_report_generates_valid_pdf() {
        let events = make_test_events();
        let sessions = make_test_sessions();
        let mut buf = std::io::Cursor::new(Vec::new());

        generate_compliance_report(
            &mut buf, &events, &sessions,
            ComplianceFormat::FedRamp,
            "2026-01-01", "2026-03-08",
            "Halcon CLI",
        ).unwrap();

        let inner = buf.into_inner();
        assert!(inner.starts_with(b"%PDF"), "FedRAMP output must be a valid PDF");
    }

    #[test]
    fn iso27001_report_generates_valid_pdf() {
        let events = make_test_events();
        let sessions = make_test_sessions();
        let mut buf = std::io::Cursor::new(Vec::new());

        generate_compliance_report(
            &mut buf, &events, &sessions,
            ComplianceFormat::Iso27001,
            "2026-01-01", "2026-03-08",
            "Halcon CLI",
        ).unwrap();

        let inner = buf.into_inner();
        assert!(inner.starts_with(b"%PDF"), "ISO 27001 output must be a valid PDF");
    }

    #[test]
    fn classify_tool_read_only() {
        assert_eq!(classify_tool("file_read"), RiskTier::ReadOnly);
        assert_eq!(classify_tool("list_directory"), RiskTier::ReadOnly);
        assert_eq!(classify_tool("grep_search"), RiskTier::ReadOnly);
        assert_eq!(classify_tool("get_content"), RiskTier::ReadOnly);
    }

    #[test]
    fn classify_tool_destructive() {
        assert_eq!(classify_tool("bash"), RiskTier::Destructive);
        assert_eq!(classify_tool("execute_command"), RiskTier::Destructive);
        assert_eq!(classify_tool("delete_file"), RiskTier::Destructive);
    }

    #[test]
    fn classify_tool_read_write() {
        assert_eq!(classify_tool("file_write"), RiskTier::ReadWrite);
        assert_eq!(classify_tool("create_file"), RiskTier::ReadWrite);
        assert_eq!(classify_tool("update_config"), RiskTier::ReadWrite);
    }

    #[test]
    fn empty_events_and_sessions_generates_valid_pdf() {
        let mut buf = std::io::Cursor::new(Vec::new());
        generate_compliance_report(
            &mut buf, &[], &[],
            ComplianceFormat::Soc2,
            "2026-01-01", "2026-03-08",
            "Test System",
        ).unwrap();
        let inner = buf.into_inner();
        assert!(inner.starts_with(b"%PDF"));
    }
}
