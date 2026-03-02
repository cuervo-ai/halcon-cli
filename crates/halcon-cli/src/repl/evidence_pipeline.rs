//! Evidence Boundary System (EBS) — "Zero Evidence → Zero Output" policy.
//!
//! Tracks textual evidence extracted from file-reading tools across all loop rounds.
//! When investigation tasks attempt file content reading but extract insufficient
//! readable text (e.g. binary PDFs, empty files, permission errors), the
//! `EvidenceGate` injects an explicit limitation notice instead of allowing
//! the coordinator to synthesize fabricated content from prior knowledge.
//!
//! ## Policy
//! - Zero Evidence → Zero Output: no synthesis of document content without real data.
//! - Binary files (PDF, images) are detected and reported explicitly.
//! - Gate applies only when content-reading tools were attempted and returned < threshold.
//! - Soft path: warning injected alongside synthesis message.
//! - Hard path: synthesis message replaced with limitation report directive.
//!
//! ## Integration Points
//! - `post_batch.rs`: calls `EvidenceBundle::record_tool_result()` for each success.
//! - `convergence_phase.rs`: checks `evidence_gate_fires()` before synthesis injection.
//! - `loop_state.rs`: owns `EvidenceBundle` as a field on `LoopState`.

// ── Constants ──────────────────────────────────────────────────────────────────

/// Minimum meaningful text bytes extracted to consider content "readable".
///
/// 30 bytes is roughly "a short sentence." Tool results below this threshold
/// when reading files indicate binary content, empty files, or permission errors.
pub const MIN_EVIDENCE_BYTES: usize = 30;

// Content-read tool detection is now centralised in `tool_aliases::is_content_read_tool()`.
// This covers all known aliases: file_read, read_file, read_text_file, read_multiple_files, etc.

/// Substrings in tool output that indicate binary or unreadable file content.
pub(crate) const BINARY_INDICATORS: &[&str] = &[
    "%PDF-",             // PDF magic header bytes
    "Binary file",       // grep binary-file detection message
    "binary file",       // lowercase variant
    "is a binary file",  // extended grep message
    "cannot process binary file",
    "\x00\x00\x00",      // null-byte sequence typical in binary formats
];

// ── EvidenceBundle ─────────────────────────────────────────────────────────────

/// Aggregate evidence state collected from tool results across all loop rounds.
///
/// Tracks both quantitative (byte count) and qualitative (binary indicators)
/// signals to decide whether synthesis should proceed or be replaced by a
/// limitation report.
#[derive(Debug, Clone, Default)]
pub struct EvidenceBundle {
    /// Total printable text bytes extracted across all content-reading tool results.
    pub text_bytes_extracted: usize,

    /// Number of calls to content-reading tools (read_file, read_multiple_files, etc.).
    pub content_read_attempts: usize,

    /// Number of tool results that contained binary-content indicators.
    pub binary_file_count: usize,

    /// Short indicator strings that triggered binary detection (for diagnostics).
    pub unreadable_indicators: Vec<String>,

    /// Whether the evidence gate fired and synthesis was replaced with a limitation notice.
    pub synthesis_blocked: bool,
}

impl EvidenceBundle {
    // ── Gate Decision ─────────────────────────────────────────────────────────

    /// Returns `true` when the evidence gate should fire.
    ///
    /// Gate fires when:
    /// - At least one content-read was attempted (read_file, read_multiple_files), AND
    /// - Less than `MIN_EVIDENCE_BYTES` of readable text was extracted in total.
    ///
    /// This indicates the files are binary, empty, or inaccessible.
    /// When the gate fires, synthesis should be replaced with an explicit limitation report.
    pub fn evidence_gate_fires(&self) -> bool {
        self.content_read_attempts > 0 && self.text_bytes_extracted < MIN_EVIDENCE_BYTES
    }

    /// Returns `true` when there is sufficient evidence to proceed with synthesis.
    pub fn has_sufficient_evidence(&self) -> bool {
        !self.evidence_gate_fires()
    }

    // ── Evidence Recording ────────────────────────────────────────────────────

    /// Record evidence from a successful tool result.
    ///
    /// Called in `post_batch.rs` for each non-error tool result.
    /// Only content-reading tools contribute to evidence tracking;
    /// search/listing tools (grep, ls, glob) are intentionally excluded because
    /// they return file *names* rather than file *content*.
    pub fn record_tool_result(&mut self, tool_name: &str, content: &str) {
        let is_content_tool = super::tool_aliases::is_content_read_tool(tool_name);

        if !is_content_tool {
            return;
        }

        self.content_read_attempts += 1;

        // Detect binary-file indicators in the output.
        for indicator in BINARY_INDICATORS {
            if content.contains(indicator) {
                self.binary_file_count += 1;
                // Record first matching indicator for diagnostics (one per result).
                self.unreadable_indicators.push(indicator.to_string());
                // Don't count bytes from binary-indicator lines as real text.
                return;
            }
        }

        // Count printable text bytes (excludes control characters except newline/tab).
        let text_bytes = count_printable_bytes(content);
        self.text_bytes_extracted += text_bytes;
    }

    // ── Gate Message ──────────────────────────────────────────────────────────

    /// Build the synthesis-replacement directive injected when the gate fires.
    ///
    /// This message asks the model to honestly report the limitation instead of
    /// synthesizing content that was never extracted from the files.
    pub fn gate_message(&self) -> String {
        if self.binary_file_count > 0 {
            format!(
                "[System — Evidence Gate ACTIVE] {attempt}s file reading attempt(s) were \
                 made but only {bytes} bytes of readable text were extracted. \
                 {binary} file(s) appear to be in binary format (PDF or similar) and \
                 cannot be read by text tools. \
                 IMPORTANT: Do NOT fabricate or infer document content. \
                 Instead, respond to the user with a clear explanation: \
                 the requested files exist but are binary (likely PDF) and require \
                 a PDF-to-text conversion tool (e.g. pdftotext) to be read. \
                 Suggest how the user can extract the text and retry.",
                attempt = self.content_read_attempts,
                bytes = self.text_bytes_extracted,
                binary = self.binary_file_count,
            )
        } else {
            format!(
                "[System — Evidence Gate ACTIVE] {attempt}s file reading attempt(s) were \
                 made but only {bytes} bytes of readable text were extracted. \
                 The files may be empty, inaccessible, or in a non-text format. \
                 IMPORTANT: Do NOT fabricate or infer document content. \
                 Instead, respond to the user honestly: the files could not be read \
                 and no content is available to analyze. Describe what was found \
                 (file names, paths) and what was NOT found (readable content).",
                attempt = self.content_read_attempts,
                bytes = self.text_bytes_extracted,
            )
        }
    }

    /// Build a compact summary string for tracing/logs.
    pub fn summary(&self) -> String {
        format!(
            "evidence_bundle(attempts={}, text_bytes={}, binary={}, gate={})",
            self.content_read_attempts,
            self.text_bytes_extracted,
            self.binary_file_count,
            if self.evidence_gate_fires() { "FIRES" } else { "pass" },
        )
    }
}

// ── Operational claim detection ────────────────────────────────────────────────

/// Detects if text contains operational claims about code/files/data
/// that would require tool evidence to be verifiable.
///
/// Returns `true` if an unverified notice should be appended (BRECHA-A mini-gate).
///
/// # Heuristic
/// Fires when the text simultaneously:
/// - References quantitative properties (line counts, byte counts, character counts), OR
///   references code constructs (function, struct, impl, fn, pub), AND
/// - Makes a claim about a file or code entity ("the file", "the code", "returns", "contains").
///
/// Intentionally conservative — only fires on combinations that strongly indicate
/// the model is asserting facts about code/file content without having read it.
pub fn detect_operational_claim(text: &str) -> bool {
    if text.len() < 20 {
        return false;
    }
    let lower = text.to_lowercase();
    // Quantitative properties about files or code.
    let has_count = lower.contains(" lines")
        || lower.contains(" line ")
        || lower.contains(" bytes")
        || lower.contains(" characters");
    // Code construct references (function/struct existence claims).
    let has_code_ref = lower.contains("function ")
        || lower.contains("struct ")
        || lower.contains("impl ")
        || lower.contains(" fn ")
        || lower.contains("pub ");
    // File or code content claims.
    let has_file_claim = lower.contains("the file")
        || lower.contains("the code")
        || lower.contains("returns ")
        || lower.contains("contains ");
    (has_count || has_code_ref) && has_file_claim
}

// ── Top-level gate enforcement ─────────────────────────────────────────────────

/// Enforce the Evidence Boundary at any synthesis injection point.
///
/// Centralised helper called by ALL 12 synthesis paths (EBS-R1 coverage).
///
/// # Returns
/// - `None` — evidence is sufficient or no content-read tools were attempted.
///   The caller should proceed with its normal synthesis directive.
/// - `Some(gate_msg)` — the gate fired. The caller **MUST**:
///   1. Use `gate_msg` as the synthesis directive (replaces normal text).
///   2. Set `synthesis_origin = SynthesisOrigin::SupervisorFailure`.
///   3. Emit a `tracing::warn!` with the path label for audit trail.
///
/// The function sets `synthesis_blocked = true` on the bundle when it fires.
pub fn enforce_evidence_boundary(bundle: &mut EvidenceBundle) -> Option<String> {
    if bundle.evidence_gate_fires() {
        bundle.synthesis_blocked = true;
        Some(bundle.gate_message())
    } else {
        None
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

/// Count printable/meaningful bytes in a string.
///
/// Counts char by char; includes newlines and tabs (structural whitespace) but
/// excludes other control characters that appear in binary output. This prevents
/// binary-file bytes from inflating the evidence counter.
fn count_printable_bytes(content: &str) -> usize {
    content
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t' || *c == '\r')
        .count()
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Gate fires correctly ──────────────────────────────────────────────────

    #[test]
    fn gate_does_not_fire_when_no_content_read_attempted() {
        // No read_file calls → gate should NOT fire (nothing was attempted).
        let bundle = EvidenceBundle::default();
        assert!(
            !bundle.evidence_gate_fires(),
            "gate must not fire when no content read attempted"
        );
    }

    #[test]
    fn gate_fires_when_read_file_returned_empty() {
        let mut bundle = EvidenceBundle::default();
        bundle.record_tool_result("read_file", "");
        assert!(
            bundle.evidence_gate_fires(),
            "empty read_file result should trigger gate"
        );
        assert_eq!(bundle.content_read_attempts, 1);
        assert_eq!(bundle.text_bytes_extracted, 0);
    }

    #[test]
    fn gate_does_not_fire_when_sufficient_text_extracted() {
        let mut bundle = EvidenceBundle::default();
        // > MIN_EVIDENCE_BYTES of real text
        bundle.record_tool_result(
            "read_file",
            "This is a valid text document with sufficient content to satisfy the evidence gate.",
        );
        assert!(
            !bundle.evidence_gate_fires(),
            "real text content should NOT trigger gate"
        );
        assert!(bundle.text_bytes_extracted >= MIN_EVIDENCE_BYTES);
    }

    // ── Binary PDF detection ──────────────────────────────────────────────────

    #[test]
    fn binary_pdf_header_detected() {
        let mut bundle = EvidenceBundle::default();
        bundle.record_tool_result("read_file", "%PDF-1.4\x00\x00garbage binary content");
        assert_eq!(bundle.binary_file_count, 1, "PDF header must trigger binary count");
        assert!(bundle.evidence_gate_fires(), "PDF binary should trigger gate");
        assert_eq!(bundle.text_bytes_extracted, 0, "binary result must not add text bytes");
    }

    #[test]
    fn grep_binary_file_message_detected() {
        let mut bundle = EvidenceBundle::default();
        bundle.record_tool_result(
            "read_multiple_files",
            "Binary file /path/to/document.pdf matches",
        );
        assert_eq!(bundle.binary_file_count, 1);
        assert!(bundle.evidence_gate_fires());
    }

    // ── Non-content tools are ignored ─────────────────────────────────────────

    #[test]
    fn grep_search_tool_does_not_affect_evidence() {
        let mut bundle = EvidenceBundle::default();
        // grep returning filenames is NOT a content-read tool
        bundle.record_tool_result("bash", "/path/to/file1.pdf\n/path/to/file2.pdf\n");
        bundle.record_tool_result("grep", "/path/to/file1.pdf\n/path/to/file2.pdf\n");
        assert_eq!(bundle.content_read_attempts, 0, "grep/bash must not count as content reads");
        assert!(!bundle.evidence_gate_fires(), "no content read attempt → gate must not fire");
    }

    // ── Multiple reads accumulate ─────────────────────────────────────────────

    #[test]
    fn multiple_read_file_calls_accumulate_bytes() {
        let mut bundle = EvidenceBundle::default();
        bundle.record_tool_result("read_file", "Content fragment one.");
        bundle.record_tool_result("read_multiple_files", "Content fragment two and more.");
        assert_eq!(bundle.content_read_attempts, 2);
        assert!(bundle.text_bytes_extracted > MIN_EVIDENCE_BYTES);
        assert!(!bundle.evidence_gate_fires());
    }

    // ── Gate message contains useful info ─────────────────────────────────────

    #[test]
    fn gate_message_mentions_binary_when_detected() {
        let mut bundle = EvidenceBundle {
            content_read_attempts: 2,
            binary_file_count: 2,
            text_bytes_extracted: 0,
            ..Default::default()
        };
        let msg = bundle.gate_message();
        assert!(msg.contains("binary"), "gate message must mention binary format");
        assert!(msg.contains("PDF"), "gate message must mention PDF");
        assert!(msg.contains("pdftotext"), "gate message must suggest pdftotext");
    }

    #[test]
    fn gate_message_no_fabrication_directive_present() {
        let bundle = EvidenceBundle {
            content_read_attempts: 1,
            binary_file_count: 0,
            text_bytes_extracted: 5,
            ..Default::default()
        };
        let msg = bundle.gate_message();
        assert!(
            msg.contains("Do NOT fabricate"),
            "gate message must contain anti-fabrication directive"
        );
    }

    // ── summary() is informative ──────────────────────────────────────────────

    #[test]
    fn summary_includes_gate_status() {
        let mut bundle = EvidenceBundle::default();
        bundle.record_tool_result("read_file", "");
        let s = bundle.summary();
        assert!(s.contains("FIRES"), "summary must indicate gate fires");
    }

    // ── enforce_evidence_boundary (EBS-B2 helper) ─────────────────────────────

    /// TEST A — EBS-B2: binary PDF read → enforce_evidence_boundary returns Some(gate_msg).
    /// Simulates: read_file("invoice.pdf") → %PDF- header → binary detected.
    /// Expected: gate fires, synthesis_blocked set, returns limitation notice.
    #[test]
    fn ebs_b2_test_a_binary_pdf_returns_gate_message() {
        let mut bundle = EvidenceBundle::default();
        bundle.record_tool_result("read_file", "%PDF-1.7 binary garbage\x00\x00");

        assert!(bundle.evidence_gate_fires(), "pre-condition: gate must fire after binary PDF read");
        assert!(!bundle.synthesis_blocked, "pre-condition: synthesis not yet blocked");

        let result = enforce_evidence_boundary(&mut bundle);

        assert!(result.is_some(), "EBS-B2: gate fired → must return Some(gate_msg)");
        let msg = result.unwrap();
        assert!(msg.contains("Do NOT fabricate"), "gate_msg must have anti-fabrication directive");
        assert!(bundle.synthesis_blocked, "EBS-B2: synthesis_blocked must be set after enforcement");
    }

    /// TEST B — EBS-B2: investigation with no tool calls → gate does NOT fire.
    /// Simulates: model-initiated EndTurn on a session that never called read_file.
    /// Expected: gate does not fire (no content_read_attempts), enforce returns None.
    #[test]
    fn ebs_b2_test_b_no_tool_calls_gate_does_not_fire() {
        let mut bundle = EvidenceBundle::default();
        // No read_file calls — content_read_attempts == 0

        assert!(!bundle.evidence_gate_fires(), "gate must NOT fire when no content reads attempted");

        let result = enforce_evidence_boundary(&mut bundle);
        assert!(result.is_none(), "EBS-B2: no content reads → enforce returns None (normal synthesis)");
        assert!(!bundle.synthesis_blocked, "synthesis_blocked must remain false");
    }

    /// TEST C — EBS-B2: real evidence read → gate does NOT fire → normal synthesis.
    /// Simulates: read_file returns actual text content (≥ MIN_EVIDENCE_BYTES).
    /// Expected: enforce returns None, synthesis proceeds normally.
    #[test]
    fn ebs_b2_test_c_real_evidence_gate_does_not_fire() {
        let mut bundle = EvidenceBundle::default();
        bundle.record_tool_result(
            "read_file",
            "INVOICE #2024-001\nClient: Acme Corp\nAmount: $1,500.00\nDue: 2024-12-31",
        );

        assert!(!bundle.evidence_gate_fires(), "gate must NOT fire when real text was extracted");

        let result = enforce_evidence_boundary(&mut bundle);
        assert!(result.is_none(), "EBS-B2: sufficient evidence → enforce returns None");
        assert!(!bundle.synthesis_blocked, "synthesis_blocked must remain false for valid evidence");
    }

    /// TEST D — EBS-B2: synthesis_blocked already set → enforce_evidence_boundary is idempotent.
    /// Simulates: EBS-1/EBS-2 already fired and set synthesis_blocked=true.
    /// The caller (EBS-B2 gate) checks synthesis_blocked BEFORE calling enforce,
    /// but this test validates the bundle's state is consistent post-EBS-1/EBS-2.
    #[test]
    fn ebs_b2_test_d_already_blocked_enforce_is_idempotent() {
        let mut bundle = EvidenceBundle {
            content_read_attempts: 1,
            text_bytes_extracted: 0,
            binary_file_count: 1,
            synthesis_blocked: true, // already set by EBS-1/EBS-2
            ..Default::default()
        };

        // Gate still fires (evidence insufficient)
        assert!(bundle.evidence_gate_fires());
        // enforce still returns Some (idempotent — safe to call again)
        let result = enforce_evidence_boundary(&mut bundle);
        assert!(result.is_some(), "enforce must return Some even when already blocked (idempotent)");
        // synthesis_blocked stays true (was already true)
        assert!(bundle.synthesis_blocked);
    }

    // ── BRECHA-A: detect_operational_claim tests ──────────────────────────────

    /// TEST: "The file has 42 lines" → operational claim → true.
    #[test]
    fn detect_operational_claim_file_lines() {
        let text = "The file has 42 lines of Rust code.";
        assert!(
            detect_operational_claim(text),
            "line-count claim about a file must be detected"
        );
    }

    /// TEST: "The function returns a Vec" → operational claim → true.
    #[test]
    fn detect_operational_claim_function_return() {
        let text = "The code contains a function that returns a Vec<String> to the caller.";
        assert!(
            detect_operational_claim(text),
            "function-return claim about code must be detected"
        );
    }

    /// TEST: "Sure, I can help!" → conversational → false.
    #[test]
    fn detect_operational_claim_conversational() {
        let text = "Sure, I can help! Let me know what you need.";
        assert!(
            !detect_operational_claim(text),
            "generic conversational text must NOT trigger claim detection"
        );
    }

    /// TEST: empty string → false (below length threshold).
    #[test]
    fn detect_operational_claim_short() {
        assert!(
            !detect_operational_claim(""),
            "empty string must NOT trigger claim detection"
        );
        assert!(
            !detect_operational_claim("Hi!"),
            "very short text must NOT trigger claim detection"
        );
    }
}
