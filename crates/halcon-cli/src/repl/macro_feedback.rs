//! Macro Feedback — user-facing plan progress in [N/M] format.
//!
//! ## Purpose
//!
//! Instead of exposing every micro-step and internal tool call to the user,
//! this module maps the compressed execution plan to a small set of **macro steps**
//! (≤ 4) and emits clean, informative progress lines.
//!
//! ## User Experience
//!
//! ```text
//! [1/3] Analizando estructura del proyecto...
//! [2/3] Ejecutando cambios...
//! [3/3] Sintetizando hallazgos...
//! ```
//!
//! ## Display Modes
//!
//! Three display modes controlled by the rendering sink:
//!
//! - **Compact** (default): `[N/M] Description`  — single line per macro step.
//! - **Verbose**: `[N/M] Description (tool: name, duration: Xs)` — for `--expert` mode.
//! - **Silent**: No output — for sub-agents and tests.
//!
//! ## Architecture
//!
//! `MacroPlanView` wraps the compressed `ExecutionPlan` and maintains current-step state.
//! The agent loop calls `advance(step_index)` after each step completes, triggering
//! a feedback emission via the `RenderSink`.

use halcon_core::traits::{ExecutionPlan, PlanStep};

// ── Display Mode ───────────────────────────────────────────────────────────

/// Feedback verbosity for macro step display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FeedbackMode {
    /// `[N/M] Description` — clean, minimal (default).
    #[default]
    Compact,
    /// `[N/M] Description (tool: name)` — for expert/verbose terminals.
    Verbose,
    /// No output — for sub-agents and test harnesses.
    Silent,
}

// ── MacroStep ─────────────────────────────────────────────────────────────

/// A single macro-step shown to the user.
#[derive(Debug, Clone)]
pub struct MacroStep {
    /// 1-based index (for display: "1/3").
    pub index: usize,
    /// Total number of macro steps in this plan.
    pub total: usize,
    /// Human-readable description (cleaned up from plan step).
    pub description: String,
    /// Tool name (if any) — shown in verbose mode.
    pub tool_name: Option<String>,
    /// Whether this step is the synthesis/final step.
    pub is_synthesis: bool,
    /// Whether this step can run in parallel with others.
    pub is_parallel: bool,
}

impl MacroStep {
    /// Format for compact display: `[1/3] Analysing project structure...`
    pub fn compact_line(&self) -> String {
        format!("[{}/{}] {}", self.index, self.total, self.description)
    }

    /// Format for verbose display: `[1/3] Analysing project structure... (file_read)`
    pub fn verbose_line(&self) -> String {
        match &self.tool_name {
            Some(tool) => format!(
                "[{}/{}] {} ({})",
                self.index, self.total, self.description, tool
            ),
            None => self.compact_line(),
        }
    }

    /// Format as "done" indicator: `[1/3] ✓ Analysing project structure`
    pub fn done_line(&self) -> String {
        format!("[{}/{}] ✓ {}", self.index, self.total, self.description)
    }

    /// Format as "failed" indicator: `[1/3] ✗ Analysing project structure`
    pub fn failed_line(&self) -> String {
        format!("[{}/{}] ✗ {}", self.index, self.total, self.description)
    }

    /// Format as "running" indicator with spinner character.
    pub fn running_line(&self, spinner_frame: usize) -> String {
        const FRAMES: &[char] = &['⠁', '⠃', '⠇', '⠧', '⠷', '⠿', '⠾', '⠼', '⠸', '⠰'];
        let frame = FRAMES[spinner_frame % FRAMES.len()];
        format!("[{}/{}] {} {}", self.index, self.total, frame, self.description)
    }
}

// ── MacroPlanView ─────────────────────────────────────────────────────────

/// Maps a compressed execution plan to macro steps for user display.
///
/// Create once after plan compression, then call `start_step` / `complete_step`
/// as the agent loop progresses.
#[derive(Debug, Clone)]
pub struct MacroPlanView {
    /// The macro steps derived from the plan.
    pub steps: Vec<MacroStep>,
    /// Current step index (0-based internal, 1-based displayed).
    current_idx: usize,
    /// Display mode.
    mode: FeedbackMode,
}

impl MacroPlanView {
    /// Build a macro view from a (compressed) execution plan.
    ///
    /// Each plan step maps to one macro step, with the description cleaned
    /// for user consumption (removes implementation noise).
    pub fn from_plan(plan: &ExecutionPlan, mode: FeedbackMode) -> Self {
        let total = plan.steps.len();
        let steps: Vec<MacroStep> = plan
            .steps
            .iter()
            .enumerate()
            .map(|(i, step)| MacroStep {
                index: i + 1,
                total,
                description: clean_description(&step.description),
                tool_name: step.tool_name.clone(),
                is_synthesis: step.tool_name.is_none(),
                is_parallel: step.parallel,
            })
            .collect();

        Self {
            steps,
            current_idx: 0,
            mode,
        }
    }

    /// Returns the total number of macro steps.
    pub fn total_steps(&self) -> usize {
        self.steps.len()
    }

    /// Returns the current step index (0-based).
    pub fn current_idx(&self) -> usize {
        self.current_idx
    }

    /// Advance to the next step. Returns the completed step for display.
    /// Returns `None` if already at the end.
    pub fn advance(&mut self) -> Option<&MacroStep> {
        if self.current_idx < self.steps.len() {
            let step = &self.steps[self.current_idx];
            self.current_idx += 1;
            Some(step)
        } else {
            None
        }
    }

    /// Get the current step without advancing.
    pub fn current(&self) -> Option<&MacroStep> {
        self.steps.get(self.current_idx)
    }

    /// Get the step at a specific plan step index (0-based).
    pub fn step_at(&self, idx: usize) -> Option<&MacroStep> {
        self.steps.get(idx)
    }

    /// Returns true if all steps have been completed.
    pub fn is_complete(&self) -> bool {
        self.current_idx >= self.steps.len()
    }

    /// Format the "starting step N" line for emission to the render sink.
    pub fn format_start(&self, step_idx: usize) -> Option<String> {
        if self.mode == FeedbackMode::Silent {
            return None;
        }
        let step = self.steps.get(step_idx)?;
        Some(match self.mode {
            FeedbackMode::Compact => step.compact_line(),
            FeedbackMode::Verbose => step.verbose_line(),
            FeedbackMode::Silent => unreachable!(),
        })
    }

    /// Format the "step N complete" line.
    pub fn format_done(&self, step_idx: usize) -> Option<String> {
        if self.mode == FeedbackMode::Silent {
            return None;
        }
        Some(self.steps.get(step_idx)?.done_line())
    }

    /// Format the "step N failed" line.
    pub fn format_failed(&self, step_idx: usize) -> Option<String> {
        if self.mode == FeedbackMode::Silent {
            return None;
        }
        Some(self.steps.get(step_idx)?.failed_line())
    }

    /// Format a summary of all steps for plan display at start.
    ///
    /// ```text
    /// Plan: Analyse project, apply fixes, synthesise report
    /// ```
    pub fn format_plan_summary(&self) -> String {
        let descriptions: Vec<_> = self.steps.iter().map(|s| s.description.as_str()).collect();
        format!("Plan: {}", descriptions.join(" → "))
    }

    /// Format a progress bar string for status display.
    ///
    /// ```text
    /// ██░░░ 2/5
    /// ```
    pub fn format_progress_bar(&self, width: usize) -> String {
        let total = self.steps.len();
        if total == 0 {
            return String::new();
        }
        let completed = self.current_idx.min(total);
        // BUG-M6 FIX: Integer division `(completed * width) / total` rounds down to 0
        // for any completed < total/width (e.g. 1/5 with width=4 → (1*4)/5 = 0).
        // Use float division and round to nearest, then clamp to [0, width].
        let filled = ((completed as f64 / total as f64) * width as f64).round() as usize;
        let filled = filled.min(width);
        let empty = width - filled;
        format!(
            "{}{} {}/{}",
            "█".repeat(filled),
            "░".repeat(empty),
            completed,
            total
        )
    }
}

// ── Description Cleaner ────────────────────────────────────────────────────

/// Clean a plan step description for user display.
///
/// Applies these transformations:
/// 1. Strip internal prefixes ("Read and analyse: ").
/// 2. Capitalise first character.
/// 3. Trim trailing punctuation.
/// 4. Translate technical verbs to user-friendly language.
/// 5. Truncate to MAX_DISPLAY_CHARS.
const MAX_DISPLAY_CHARS: usize = 60;

fn clean_description(desc: &str) -> String {
    // ── Step 1: Handle merged-read prefixes from plan_compressor ──────────
    // BUG-H5 FIX: "Read and analyse: main.rs, lib.rs" → stripping the prefix
    // left "main.rs, lib.rs" which capitalised to "Main.rs, lib.rs" — meaningless.
    // When the prefix reveals multiple comma-separated sources, emit
    // "Analizando N fuentes" instead (short, informative, avoids filename noise).
    let primary: &str = if let Some(rest) = desc
        .strip_prefix("Read and analyse: ")
        .or_else(|| desc.strip_prefix("Read and analyze: "))
    {
        let sources: Vec<&str> = rest.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
        if sources.len() > 1 {
            // Multiple sources — emit compact count and return early.
            return format!("Analizando {} fuentes", sources.len());
        }
        // Single source — fall through with just the source name; translate_to_user_language
        // will treat it as an opaque string (passes through as-is then gets capitalised).
        rest
    } else {
        desc
    };

    // ── Step 2: Apply user-friendly translations ──────────────────────────
    let translated = translate_to_user_language(primary.trim());

    // ── Step 3: Capitalise first character ───────────────────────────────
    let mut chars = translated.chars();
    let capitalised = match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
    };

    // ── Step 4: Trim trailing punctuation ────────────────────────────────
    let trimmed = capitalised.trim_end_matches(['.', ';', ',']);

    // ── Step 5: Truncate ─────────────────────────────────────────────────
    if trimmed.chars().count() > MAX_DISPLAY_CHARS {
        format!("{}…", trimmed.chars().take(MAX_DISPLAY_CHARS).collect::<String>())
    } else {
        trimmed.to_string()
    }
}

fn translate_to_user_language(desc: &str) -> String {
    // Map technical descriptions to clearer user-facing language.
    let lower = desc.to_lowercase();

    // Synthesis step.
    if lower.contains("synthesize") || lower.contains("synthesise") || lower.contains("respond to the user") {
        return "Sintetizando y respondiendo".to_string();
    }
    if lower.contains("summarize") || lower.contains("summarise") {
        return "Resumiendo hallazgos".to_string();
    }

    // File operations.
    // BUG-M5 FIX: The original code used `lower.contains("edit file")` (case-insensitive)
    // to detect the pattern but then called `desc.replacen("edit ", ...)` on the original
    // `desc` (case-sensitive) — missing "Edit file" (uppercase E). Fix: use byte-position
    // from `lower.find()` to locate the keyword, then extract the remainder from `desc`
    // at the same byte position (safe because ASCII keywords have same byte length).
    if lower.contains("file_edit") || lower.contains("edit file") || lower.contains("modify file") {
        if lower.starts_with("file_edit") {
            // "file_edit main.rs" → "editar main.rs"
            return format!("editar{}", &desc["file_edit".len()..]);
        }
        if let Some(pos) = lower.find("edit ") {
            let after = &desc[pos + "edit ".len()..];
            return format!("Editar {after}");
        }
        if let Some(pos) = lower.find("modify ") {
            let after = &desc[pos + "modify ".len()..];
            return format!("Modificar {after}");
        }
        return desc.to_string();
    }
    if lower.contains("file_write") || lower.contains("write file") || lower.contains("create file") {
        if lower.starts_with("file_write") {
            return format!("crear{}", &desc["file_write".len()..]);
        }
        if let Some(pos) = lower.find("write ") {
            let after = &desc[pos + "write ".len()..];
            return format!("Crear {after}");
        }
        if let Some(pos) = lower.find("create ") {
            let after = &desc[pos + "create ".len()..];
            return format!("Crear {after}");
        }
        return desc.to_string();
    }

    // Bash/execution.
    if lower.starts_with("run ") || lower.starts_with("execute ") || lower.starts_with("bash") {
        return format!("Ejecutar: {}", desc.splitn(2, ' ').nth(1).unwrap_or(desc));
    }

    // Reading/analysis.
    if lower.starts_with("read ") || lower.starts_with("analyse ") || lower.starts_with("analyze ") {
        let rest = desc.splitn(2, ' ').nth(1).unwrap_or(desc);
        return format!("Analizando {}", rest.to_lowercase());
    }

    // Search.
    if lower.starts_with("search ") || lower.starts_with("grep ") || lower.starts_with("find ") {
        let rest = desc.splitn(2, ' ').nth(1).unwrap_or(desc);
        return format!("Buscando {}", rest.to_lowercase());
    }

    // Git operations.
    if lower.contains("git") {
        return format!("Operación Git: {}", desc);
    }

    // Default: pass through.
    desc.to_string()
}

// ── Progress Emitter ───────────────────────────────────────────────────────

/// Format strings for step progress events.
///
/// Used by the agent loop to emit consistent step-progress messages to the RenderSink.
pub struct ProgressEmitter;

impl ProgressEmitter {
    /// Emit a "step starting" message to info sink.
    pub fn step_starting(view: &MacroPlanView, step_idx: usize) -> Option<String> {
        view.format_start(step_idx)
    }

    /// Emit a "step done" message to info sink.
    pub fn step_done(view: &MacroPlanView, step_idx: usize) -> Option<String> {
        view.format_done(step_idx)
    }

    /// Emit a "step failed" message to warning sink.
    pub fn step_failed(view: &MacroPlanView, step_idx: usize) -> Option<String> {
        view.format_failed(step_idx)
    }

    /// Emit plan summary at the start of execution.
    pub fn plan_start(view: &MacroPlanView) -> String {
        view.format_plan_summary()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::traits::PlanStep;
    use uuid::Uuid;

    fn make_plan(steps: Vec<(&str, Option<&str>)>) -> ExecutionPlan {
        ExecutionPlan {
            goal: "Test goal".to_string(),
            steps: steps
                .into_iter()
                .map(|(desc, tool)| PlanStep {
                    step_id: Uuid::new_v4(),
                    description: desc.to_string(),
                    tool_name: tool.map(|t| t.to_string()),
                    parallel: false,
                    confidence: 0.9,
                    expected_args: None,
                    outcome: None,
                })
                .collect(),
            requires_confirmation: false,
            plan_id: Uuid::new_v4(),
            replan_count: 0,
            parent_plan_id: None,
            ..Default::default()
        }
    }

    #[test]
    fn macro_step_compact_format() {
        let step = MacroStep {
            index: 2,
            total: 3,
            description: "Analizando código".to_string(),
            tool_name: Some("file_read".to_string()),
            is_synthesis: false,
            is_parallel: false,
        };
        assert_eq!(step.compact_line(), "[2/3] Analizando código");
    }

    #[test]
    fn macro_step_verbose_format_shows_tool() {
        let step = MacroStep {
            index: 1,
            total: 3,
            description: "Analizando código".to_string(),
            tool_name: Some("file_read".to_string()),
            is_synthesis: false,
            is_parallel: false,
        };
        assert_eq!(step.verbose_line(), "[1/3] Analizando código (file_read)");
    }

    #[test]
    fn macro_step_done_line() {
        let step = MacroStep {
            index: 1,
            total: 3,
            description: "Analizando".to_string(),
            tool_name: None,
            is_synthesis: true,
            is_parallel: false,
        };
        assert_eq!(step.done_line(), "[1/3] ✓ Analizando");
    }

    #[test]
    fn from_plan_creates_correct_count() {
        let plan = make_plan(vec![
            ("Read files", Some("file_read")),
            ("Edit config", Some("file_write")),
            ("Synthesize findings", None),
        ]);

        let view = MacroPlanView::from_plan(&plan, FeedbackMode::Compact);
        assert_eq!(view.total_steps(), 3);
        assert!(!view.is_complete());
    }

    #[test]
    fn advance_increments_index() {
        let plan = make_plan(vec![
            ("Read", Some("file_read")),
            ("Synthesize", None),
        ]);

        let mut view = MacroPlanView::from_plan(&plan, FeedbackMode::Compact);
        assert_eq!(view.current_idx(), 0);
        view.advance();
        assert_eq!(view.current_idx(), 1);
        view.advance();
        assert!(view.is_complete());
    }

    #[test]
    fn advance_past_end_returns_none() {
        let plan = make_plan(vec![("Synthesize", None)]);
        let mut view = MacroPlanView::from_plan(&plan, FeedbackMode::Compact);
        view.advance();
        assert!(view.advance().is_none());
    }

    #[test]
    fn silent_mode_format_start_returns_none() {
        let plan = make_plan(vec![("Read", Some("file_read")), ("Synthesize", None)]);
        let view = MacroPlanView::from_plan(&plan, FeedbackMode::Silent);
        assert!(view.format_start(0).is_none());
    }

    #[test]
    fn compact_mode_format_start_returns_some() {
        let plan = make_plan(vec![("Read file", Some("file_read")), ("Synthesize", None)]);
        let view = MacroPlanView::from_plan(&plan, FeedbackMode::Compact);
        let line = view.format_start(0).unwrap();
        assert!(line.starts_with("[1/2]"));
    }

    #[test]
    fn plan_summary_joins_descriptions() {
        let plan = make_plan(vec![
            ("Read files", Some("file_read")),
            ("Edit config", Some("file_write")),
            ("Synthesize", None),
        ]);
        let view = MacroPlanView::from_plan(&plan, FeedbackMode::Compact);
        let summary = view.format_plan_summary();
        assert!(summary.starts_with("Plan:"));
        assert!(summary.contains("→"));
    }

    #[test]
    fn progress_bar_filled_correctly() {
        let plan = make_plan(vec![
            ("Step A", Some("bash")),
            ("Step B", Some("bash")),
            ("Step C", Some("bash")),
            ("Synthesize", None),
        ]);
        let mut view = MacroPlanView::from_plan(&plan, FeedbackMode::Compact);
        let bar_zero = view.format_progress_bar(4);
        assert!(bar_zero.contains("0/4"));

        view.advance();
        view.advance();
        let bar_half = view.format_progress_bar(4);
        assert!(bar_half.contains("2/4"));
    }

    #[test]
    fn clean_description_capitalises_first_char() {
        assert!(clean_description("read the file").chars().next().unwrap().is_uppercase());
    }

    #[test]
    fn clean_description_synthesis_translated() {
        let desc = clean_description("Synthesize findings and respond to the user");
        assert!(desc.contains("Sintetizando"));
    }

    #[test]
    fn clean_description_strips_internal_prefix() {
        let desc = clean_description("Read and analyse: main.rs, lib.rs");
        assert!(!desc.contains("Read and analyse:"));
    }

    #[test]
    fn clean_description_truncates_long_text() {
        let long = "a".repeat(MAX_DISPLAY_CHARS + 10);
        let cleaned = clean_description(&long);
        assert!(cleaned.chars().count() <= MAX_DISPLAY_CHARS + 1); // +1 for ellipsis
    }

    #[test]
    fn macro_step_running_line_includes_spinner() {
        let step = MacroStep {
            index: 1,
            total: 3,
            description: "Working".to_string(),
            tool_name: None,
            is_synthesis: false,
            is_parallel: false,
        };
        let line = step.running_line(0);
        assert!(line.contains("[1/3]"));
        assert!(line.contains("Working"));
    }

    #[test]
    fn synthesis_step_correctly_identified() {
        let plan = make_plan(vec![
            ("Read", Some("file_read")),
            ("Synthesize", None),
        ]);
        let view = MacroPlanView::from_plan(&plan, FeedbackMode::Compact);
        assert!(!view.steps[0].is_synthesis);
        assert!(view.steps[1].is_synthesis);
    }

    // ── BUG-H5 regression: merged step prefix produces useful description ──

    #[test]
    fn clean_description_merged_step_shows_source_count() {
        // BUG-H5: "Read and analyse: main.rs, lib.rs" must NOT become "Main.rs, lib.rs".
        // With 2 comma-separated sources → "Analizando 2 fuentes".
        let desc = clean_description("Read and analyse: main.rs, lib.rs");
        assert_eq!(desc, "Analizando 2 fuentes");
    }

    #[test]
    fn clean_description_merged_step_three_sources() {
        let desc = clean_description("Read and analyze: main.rs, lib.rs, tests.rs");
        assert_eq!(desc, "Analizando 3 fuentes");
    }

    #[test]
    fn clean_description_single_source_after_prefix_falls_through() {
        // Single source: "Read and analyse: main.rs" → falls through to capitalize.
        let desc = clean_description("Read and analyse: main.rs");
        // Should NOT output "Analizando 1 fuentes" — should produce the filename capitalised.
        assert!(!desc.contains("fuentes"), "Single source should not emit 'N fuentes'");
    }

    // ── BUG-M5 regression: case-insensitive edit/write detection ──────────

    #[test]
    fn translate_mixed_case_edit_file_detected() {
        // BUG-M5: "Edit file main.rs" has uppercase E — original code's `desc.replacen("edit ", ...)`
        // couldn't find it (case-sensitive). Must now produce "Editar main.rs".
        let result = translate_to_user_language("Edit file main.rs");
        assert!(result.contains("Editar"), "Expected 'Editar', got: {result}");
        assert!(result.contains("main.rs"), "Must preserve filename");
    }

    #[test]
    fn translate_lowercase_edit_file_still_works() {
        // "edit file config.toml" contains "edit file" as substring → triggers the branch.
        // Lowercase — was working before BUG-M5 fix; must continue to work after.
        let result = translate_to_user_language("edit file config.toml");
        assert!(result.contains("Editar"), "lowercase 'edit file' must still work: {result}");
        assert!(result.contains("config.toml"), "Filename must be preserved: {result}");
    }

    #[test]
    fn translate_modify_keyword_detected() {
        // "modify file Cargo.toml" contains "modify file" as substring → triggers the branch.
        let result = translate_to_user_language("modify file Cargo.toml");
        assert!(result.contains("Modificar"), "Expected 'Modificar', got: {result}");
    }

    // ── BUG-M6 regression: progress bar partial completion not zero ────────

    #[test]
    fn progress_bar_partial_completion_not_zero() {
        // BUG-M6: with integer division, 1/5 steps * width=4 → (1*4)/5=0 → all empty.
        // With float division: (1/5)*4 = 0.8 → round → 1 filled block.
        let plan = make_plan(vec![
            ("Step A", Some("bash")),
            ("Step B", Some("bash")),
            ("Step C", Some("bash")),
            ("Step D", Some("bash")),
            ("Synthesize", None),
        ]);
        let mut view = MacroPlanView::from_plan(&plan, FeedbackMode::Compact);
        view.advance(); // 1/5 complete
        let bar = view.format_progress_bar(4);
        // With float division: round(1/5 * 4) = round(0.8) = 1 filled block.
        assert!(bar.contains('█'), "First step should produce at least 1 filled block: {bar}");
        assert!(bar.contains("1/5"));
    }

    #[test]
    fn progress_bar_full_completion_all_filled() {
        let plan = make_plan(vec![("Step A", Some("bash")), ("Synthesize", None)]);
        let mut view = MacroPlanView::from_plan(&plan, FeedbackMode::Compact);
        view.advance();
        view.advance();
        let bar = view.format_progress_bar(4);
        assert_eq!(bar, "████ 2/2", "Fully complete plan must have all blocks filled");
    }
}
