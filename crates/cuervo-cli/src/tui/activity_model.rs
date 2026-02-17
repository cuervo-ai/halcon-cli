//! Activity data model with O(1) search indexing.
//!
//! **Phase A1: Foundation — Activity Model**
//!
//! Separates data storage from presentation logic. Provides:
//! - InvertedIndex for O(1) keyword lookup (vs O(n) linear scan)
//! - LineMetadata for cross-references (line → plan step, round, tool_id)
//! - Filter state management (conversation, tools, errors, system, plans)

use std::collections::{HashMap, HashSet};
use super::activity_types::ActivityLine;

// Phase 3 SRCH-002: Regex pattern matching support
use regex::Regex;

/// Filter flags for activity lines (bitflags pattern).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivityFilter {
    bits: u8,
}

impl ActivityFilter {
    pub const CONVERSATION: Self = Self { bits: 0b00001 };
    pub const TOOLS: Self = Self { bits: 0b00010 };
    pub const ERRORS: Self = Self { bits: 0b00100 };
    pub const SYSTEM: Self = Self { bits: 0b01000 };
    pub const PLANS: Self = Self { bits: 0b10000 };
    pub const ALL: Self = Self { bits: 0b11111 };
    pub const NONE: Self = Self { bits: 0b00000 };

    /// Check if a specific filter is enabled.
    pub fn contains(self, other: Self) -> bool {
        (self.bits & other.bits) == other.bits
    }

    /// Enable a filter.
    pub fn insert(&mut self, other: Self) {
        self.bits |= other.bits;
    }

    /// Disable a filter.
    pub fn remove(&mut self, other: Self) {
        self.bits &= !other.bits;
    }

    /// Toggle a filter.
    pub fn toggle(&mut self, other: Self) {
        self.bits ^= other.bits;
    }

    /// Check if any filters are active (not ALL).
    pub fn is_filtering(self) -> bool {
        self.bits != Self::ALL.bits
    }
}

impl Default for ActivityFilter {
    fn default() -> Self {
        Self::ALL
    }
}

/// Inverted index for O(1) search lookup.
/// Maps normalized words → line indices containing them.
#[derive(Debug, Clone, Default)]
pub struct InvertedIndex {
    /// word (lowercase) → set of line indices
    index: HashMap<String, HashSet<usize>>,
    /// Phase 3 SRCH-001: All indexed words for fuzzy matching
    all_words: HashSet<String>,
}

impl InvertedIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Index a line by extracting and normalizing words.
    pub fn index_line(&mut self, line_idx: usize, text: &str) {
        for word in Self::tokenize(text) {
            let normalized = word.to_lowercase();
            self.index
                .entry(normalized.clone())
                .or_insert_with(HashSet::new)
                .insert(line_idx);
            // Phase 3 SRCH-001: Track all words for fuzzy matching
            self.all_words.insert(normalized);
        }
    }

    /// Search for lines matching ALL query words (AND semantics).
    /// Returns sorted line indices in ascending order.
    pub fn search(&self, query: &str) -> Vec<usize> {
        if query.trim().is_empty() {
            return Vec::new();
        }

        let query_words: Vec<String> = Self::tokenize(query)
            .into_iter()
            .map(|w| w.to_lowercase())
            .collect();

        if query_words.is_empty() {
            return Vec::new();
        }

        // Get line sets for each query word
        let mut word_sets: Vec<&HashSet<usize>> = query_words
            .iter()
            .filter_map(|w| self.index.get(w))
            .collect();

        if word_sets.is_empty() {
            return Vec::new();
        }

        // Intersect all sets (AND operation)
        let mut result: HashSet<usize> = word_sets[0].clone();
        for set in word_sets.iter().skip(1) {
            result = result.intersection(set).copied().collect();
        }

        // Sort and return
        let mut indices: Vec<usize> = result.into_iter().collect();
        indices.sort_unstable();
        indices
    }

    /// Phase 3 SRCH-001: Fuzzy search with Levenshtein distance tolerance.
    /// Matches words within `max_distance` edits (default 2 for typo tolerance).
    /// Returns sorted line indices in ascending order.
    pub fn fuzzy_search(&self, query: &str, max_distance: usize) -> Vec<usize> {
        if query.trim().is_empty() {
            return Vec::new();
        }

        let query_words: Vec<String> = Self::tokenize(query)
            .into_iter()
            .map(|w| w.to_lowercase())
            .collect();

        if query_words.is_empty() {
            return Vec::new();
        }

        let mut word_sets: Vec<HashSet<usize>> = Vec::new();

        // For each query word, find all indexed words within edit distance
        for query_word in &query_words {
            let mut fuzzy_matches = HashSet::new();

            // First check exact match (O(1))
            if let Some(lines) = self.index.get(query_word) {
                fuzzy_matches.extend(lines);
            }

            // Then check fuzzy matches if max_distance > 0
            if max_distance > 0 {
                for indexed_word in &self.all_words {
                    if Self::levenshtein_distance(query_word, indexed_word) <= max_distance {
                        if let Some(lines) = self.index.get(indexed_word) {
                            fuzzy_matches.extend(lines);
                        }
                    }
                }
            }

            word_sets.push(fuzzy_matches);
        }

        if word_sets.is_empty() {
            return Vec::new();
        }

        // Intersect all sets (AND operation)
        let mut result: HashSet<usize> = word_sets[0].clone();
        for set in word_sets.iter().skip(1) {
            result = result.intersection(set).copied().collect();
        }

        // Sort and return
        let mut indices: Vec<usize> = result.into_iter().collect();
        indices.sort_unstable();
        indices
    }

    /// Compute Levenshtein distance between two strings.
    /// Returns minimum number of single-character edits (insertions, deletions, substitutions).
    fn levenshtein_distance(a: &str, b: &str) -> usize {
        let a_chars: Vec<char> = a.chars().collect();
        let b_chars: Vec<char> = b.chars().collect();
        let a_len = a_chars.len();
        let b_len = b_chars.len();

        if a_len == 0 {
            return b_len;
        }
        if b_len == 0 {
            return a_len;
        }

        // Dynamic programming matrix: dp[i][j] = distance between a[0..i] and b[0..j]
        let mut dp = vec![vec![0usize; b_len + 1]; a_len + 1];

        // Initialize first row and column
        for i in 0..=a_len {
            dp[i][0] = i;
        }
        for j in 0..=b_len {
            dp[0][j] = j;
        }

        // Fill matrix
        for i in 1..=a_len {
            for j in 1..=b_len {
                let cost = if a_chars[i - 1] == b_chars[j - 1] { 0 } else { 1 };
                dp[i][j] = (dp[i - 1][j] + 1) // deletion
                    .min(dp[i][j - 1] + 1) // insertion
                    .min(dp[i - 1][j - 1] + cost); // substitution
            }
        }

        dp[a_len][b_len]
    }

    /// Tokenize text into words (alphanumeric + underscores, min 2 chars).
    fn tokenize(text: &str) -> Vec<String> {
        text.split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|w| w.len() >= 2)
            .map(|w| w.to_string())
            .collect()
    }

    /// Clear the entire index.
    pub fn clear(&mut self) {
        self.index.clear();
        self.all_words.clear(); // Phase 3 SRCH-001
    }

    /// Rebuild the index from scratch given all lines.
    pub fn rebuild(&mut self, lines: &[ActivityLine]) {
        self.clear();
        for (idx, line) in lines.iter().enumerate() {
            self.index_line(idx, &line.text_content());
        }
    }
}

/// Cross-reference metadata for activity lines.
#[derive(Debug, Clone, Default)]
pub struct LineMetadata {
    /// tool_use_id → line index
    pub tool_to_line: HashMap<String, usize>,
    /// line index → plan step index
    pub line_to_step: HashMap<usize, usize>,
    /// round number → line indices in that round
    pub round_to_lines: HashMap<usize, Vec<usize>>,
}

impl LineMetadata {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a tool execution line.
    pub fn record_tool(&mut self, line_idx: usize, tool_use_id: String) {
        self.tool_to_line.insert(tool_use_id, line_idx);
    }

    /// Link a line to a plan step.
    pub fn link_to_step(&mut self, line_idx: usize, step_idx: usize) {
        self.line_to_step.insert(line_idx, step_idx);
    }

    /// Add a line to a round's line list.
    pub fn add_to_round(&mut self, round: usize, line_idx: usize) {
        self.round_to_lines
            .entry(round)
            .or_insert_with(Vec::new)
            .push(line_idx);
    }

    /// Get plan step for a given line.
    pub fn step_for_line(&self, line_idx: usize) -> Option<usize> {
        self.line_to_step.get(&line_idx).copied()
    }

    /// Get line index for a given tool.
    pub fn line_for_tool(&self, tool_use_id: &str) -> Option<usize> {
        self.tool_to_line.get(tool_use_id).copied()
    }

    /// Get all lines in a given round.
    pub fn lines_in_round(&self, round: usize) -> &[usize] {
        self.round_to_lines.get(&round).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Clear all metadata.
    pub fn clear(&mut self) {
        self.tool_to_line.clear();
        self.line_to_step.clear();
        self.round_to_lines.clear();
    }
}

/// Data model for activity feed.
///
/// Separates data storage from rendering logic. Provides:
/// - O(1) search via InvertedIndex
/// - Cross-reference metadata
/// - Filter state management
pub struct ActivityModel {
    /// All activity lines (ordered chronologically).
    lines: Vec<ActivityLine>,
    /// Inverted index for fast search.
    index: InvertedIndex,
    /// Cross-reference metadata.
    pub metadata: LineMetadata,
    /// Active filter flags.
    pub filters: ActivityFilter,
    /// P0.4B: Temporary scroll state (will be extracted in future refactor).
    pub(crate) scroll_offset: usize,
    /// P0.4B: Auto-scroll flag (scroll to bottom on new lines).
    pub(crate) auto_scroll: bool,
    /// P0.4B: Cached max scroll from last render.
    pub(crate) last_max_scroll: usize,
}

impl ActivityModel {
    pub fn new() -> Self {
        Self {
            lines: Vec::new(),
            index: InvertedIndex::new(),
            metadata: LineMetadata::new(),
            filters: ActivityFilter::ALL,
            scroll_offset: 0,
            auto_scroll: true,
            last_max_scroll: 0,
        }
    }

    /// Add a new line to the model.
    /// Automatically indexes it for search.
    pub fn push(&mut self, line: ActivityLine) {
        let idx = self.lines.len();
        self.index.index_line(idx, &line.text_content());
        self.lines.push(line);
    }

    /// Get total line count.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Get a specific line by index.
    pub fn get(&self, idx: usize) -> Option<&ActivityLine> {
        self.lines.get(idx)
    }

    /// Get a mutable reference to a specific line.
    pub fn get_mut(&mut self, idx: usize) -> Option<&mut ActivityLine> {
        self.lines.get_mut(idx)
    }

    /// Search for lines matching query (O(1) via inverted index).
    pub fn search(&self, query: &str) -> Vec<usize> {
        self.index.search(query)
    }

    /// Phase 3 SRCH-001: Fuzzy search with Levenshtein distance tolerance.
    /// Matches words within `max_distance` edits (default 2).
    pub fn fuzzy_search(&self, query: &str, max_distance: usize) -> Vec<usize> {
        self.index.fuzzy_search(query, max_distance)
    }

    /// Phase 3 SRCH-002: Regex pattern search.
    /// Searches line text content with regular expression pattern.
    /// Returns sorted line indices where pattern matches.
    /// Returns empty vec if pattern is invalid.
    pub fn regex_search(&self, pattern: &str) -> Vec<usize> {
        // Try to compile regex
        let re = match Regex::new(pattern) {
            Ok(r) => r,
            Err(_) => return Vec::new(), // Invalid pattern → no results
        };

        let mut matches = Vec::new();

        for (idx, line) in self.lines.iter().enumerate() {
            if re.is_match(&line.text_content()) {
                matches.push(idx);
            }
        }

        matches
    }

    /// Clear all lines and metadata.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.index.clear();
        self.metadata.clear();
    }

    /// Get an iterator over lines matching the active filter.
    pub fn filter_active(&self) -> impl Iterator<Item = (usize, &ActivityLine)> {
        self.lines
            .iter()
            .enumerate()
            .filter(move |(_, line)| self.filter_matches(line))
    }

    /// Check if a line matches the active filter.
    fn filter_matches(&self, line: &ActivityLine) -> bool {
        // If ALL filters enabled, show everything
        if self.filters.contains(ActivityFilter::ALL) {
            return true;
        }

        // Match line type against enabled filters
        use super::activity_types::ActivityLine as AL;
        match line {
            AL::UserPrompt(_) | AL::AssistantText(_) | AL::CodeBlock { .. } => {
                self.filters.contains(ActivityFilter::CONVERSATION)
            }
            AL::ToolExec { .. } => {
                self.filters.contains(ActivityFilter::TOOLS)
            }
            AL::Error { .. } => {
                self.filters.contains(ActivityFilter::ERRORS)
            }
            AL::Info(_) | AL::Warning { .. } | AL::RoundSeparator(_) => {
                self.filters.contains(ActivityFilter::SYSTEM)
            }
            AL::PlanOverview { .. } => {
                self.filters.contains(ActivityFilter::PLANS)
            }
        }
    }

    /// Get all lines (unfiltered).
    pub fn all_lines(&self) -> &[ActivityLine] {
        &self.lines
    }

    /// Find a tool execution line by tool name (last occurrence).
    pub fn find_last_tool(&self, tool_name: &str) -> Option<usize> {
        self.lines
            .iter()
            .enumerate()
            .rev()
            .find(|(_, line)| {
                if let ActivityLine::ToolExec { name, .. } = line {
                    name == tool_name
                } else {
                    false
                }
            })
            .map(|(idx, _)| idx)
    }

    /// Rebuild the inverted index from scratch.
    /// Useful after bulk modifications.
    pub fn rebuild_index(&mut self) {
        self.index.rebuild(&self.lines);
    }

    /// Complete a tool execution by filling in the result on the matching entry.
    /// Finds the last ToolExec with matching name and no result, then updates it.
    /// Phase A3: Enables ToolOutput dual-write.
    pub fn complete_tool(&mut self, tool_name: &str, content: String, is_error: bool, duration_ms: u64) {
        use super::activity_types::{ActivityLine, ToolResult};

        // Find the last ToolExec with matching name and no result
        for line in self.lines.iter_mut().rev() {
            if let ActivityLine::ToolExec {
                name,
                result: ref mut r,
                ..
            } = line
            {
                if name == tool_name && r.is_none() {
                    *r = Some(ToolResult {
                        content,
                        is_error,
                        duration_ms,
                    });
                    break;
                }
            }
        }
    }

    /// Add or accumulate assistant text (streaming-aware).
    ///
    /// P0.2: Fix stream chunk divergence. If the last line is AssistantText,
    /// appends to it. Otherwise, creates a new line.
    /// This matches legacy ActivityState behavior for consistent line counts.
    pub fn push_assistant_text(&mut self, text: String) {
        // Calculate idx before mutable borrow
        let last_idx = if self.lines.is_empty() {
            None
        } else {
            Some(self.lines.len() - 1)
        };

        // Check if last line is AssistantText
        let should_accumulate = matches!(
            self.lines.last(),
            Some(ActivityLine::AssistantText(_))
        );

        if should_accumulate {
            if let Some(ActivityLine::AssistantText(ref mut existing)) = self.lines.last_mut() {
                existing.push_str(&text);
                // Rebuild index for the updated line
                if let Some(idx) = last_idx {
                    self.index.index_line(idx, existing);
                }
            }
        } else {
            // Create new AssistantText line
            self.push(ActivityLine::AssistantText(text));
        }
    }

    /// Set or replace the plan overview.
    ///
    /// P0.2: Fix missing PlanProgress dual-write. If a PlanOverview already exists,
    /// replaces it. Otherwise, appends a new one.
    /// This prevents duplicate plan displays in the activity feed.
    pub fn set_plan_overview(
        &mut self,
        goal: String,
        steps: Vec<crate::tui::events::PlanStepStatus>,
        current_step: usize,
    ) {
        // Find existing PlanOverview index
        let existing_idx = self.lines
            .iter()
            .enumerate()
            .rev()
            .find(|(_, line)| matches!(line, ActivityLine::PlanOverview { .. }))
            .map(|(idx, _)| idx);

        if let Some(idx) = existing_idx {
            // Replace existing
            self.lines[idx] = ActivityLine::PlanOverview {
                goal: goal.clone(),
                steps,
                current_step,
            };
            // Rebuild index for updated line
            self.index.index_line(idx, &goal);
        } else {
            // No existing PlanOverview found, push new one
            let idx = self.lines.len();
            self.index.index_line(idx, &goal);
            self.lines.push(ActivityLine::PlanOverview {
                goal,
                steps,
                current_step,
            });
        }
    }

    /// Add a round separator.
    ///
    /// P0.2: Fix missing RoundStart dual-write. Adds visual separator between rounds.
    pub fn push_round_separator(&mut self, round: usize) {
        self.push(ActivityLine::RoundSeparator(round));
    }

    /// Push an informational message.
    ///
    /// P0.4A: Convenience wrapper for Info variant, matches ActivityState API.
    pub fn push_info(&mut self, text: &str) {
        self.push(ActivityLine::Info(text.to_string()));
    }

    /// Push a warning message with optional hint.
    ///
    /// P0.4A: Convenience wrapper for Warning variant, matches ActivityState API.
    pub fn push_warning(&mut self, message: &str, hint: Option<&str>) {
        self.push(ActivityLine::Warning {
            message: message.to_string(),
            hint: hint.map(|s| s.to_string()),
        });
    }

    /// Push an error message with optional hint.
    ///
    /// P0.4A: Convenience wrapper for Error variant, matches ActivityState API.
    pub fn push_error(&mut self, message: &str, hint: Option<&str>) {
        self.push(ActivityLine::Error {
            message: message.to_string(),
            hint: hint.map(|s| s.to_string()),
        });
    }

    /// Push a user prompt line.
    ///
    /// P0.4A: Convenience wrapper for UserPrompt variant, matches ActivityState API.
    pub fn push_user_prompt(&mut self, text: &str) {
        self.push(ActivityLine::UserPrompt(text.to_string()));
    }

    /// Push a code block with syntax highlighting.
    ///
    /// P0.4A: Convenience wrapper for CodeBlock variant, matches ActivityState API.
    pub fn push_code_block(&mut self, lang: &str, code: &str) {
        self.push(ActivityLine::CodeBlock {
            lang: lang.to_string(),
            code: code.to_string(),
        });
    }

    /// Push a tool execution start (skeleton/loading state).
    ///
    /// P0.4A: Convenience wrapper for ToolExec variant, matches ActivityState API.
    pub fn push_tool_start(&mut self, name: &str, input_preview: &str) {
        self.push(ActivityLine::ToolExec {
            name: name.to_string(),
            input_preview: input_preview.to_string(),
            result: None,
            expanded: false,
        });
    }

    /// Get total line count (matches ActivityState::line_count).
    ///
    /// P0.4A: Alias for len() to match legacy API.
    pub fn line_count(&self) -> usize {
        self.len()
    }

    /// Check if there are any loading tools (ToolExec with result=None).
    ///
    /// P0.4A: Port from ActivityState for spinner detection.
    pub fn has_loading_tools(&self) -> bool {
        self.lines.iter().any(|line| {
            matches!(line, ActivityLine::ToolExec { result: None, .. })
        })
    }

    // --- P0.4B: Temporary scroll methods (will be refactored to ScrollState) ---

    /// Scroll up by N lines.
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
        self.auto_scroll = false;
    }

    /// Scroll down by N lines.
    pub fn scroll_down(&mut self, n: usize) {
        self.scroll_offset = (self.scroll_offset + n).min(self.last_max_scroll);
        // Re-enable auto-scroll if we've reached the bottom
        if self.scroll_offset >= self.last_max_scroll {
            self.auto_scroll = true;
        }
    }

    /// Scroll to a specific line index.
    pub fn scroll_to_line(&mut self, line_idx: usize) {
        self.scroll_offset = line_idx;
        self.auto_scroll = false;
    }

    /// Scroll to the bottom (most recent activity).
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = self.last_max_scroll;
        self.auto_scroll = true;
    }

    /// Toggle conversation filter (show only conversational lines, hide system events).
    ///
    /// P0.4B: Port from ActivityState. Returns new filter state (true = conversation only).
    pub fn toggle_conversation_filter(&mut self) -> bool {
        if self.filters.contains(ActivityFilter::CONVERSATION) {
            // Currently showing conversation → remove to show all
            self.filters = ActivityFilter::ALL;
            false
        } else {
            // Currently showing all or something else → show only conversation
            self.filters = ActivityFilter::CONVERSATION;
            true
        }
    }

    /// Check if conversation filter is active (only conversational lines visible).
    ///
    /// P0.4B: Port from ActivityState.
    pub fn is_conversation_only(&self) -> bool {
        self.filters.contains(ActivityFilter::CONVERSATION)
            && !self.filters.contains(ActivityFilter::SYSTEM)
    }
}

impl Default for ActivityModel {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_filter_contains() {
        let filter = ActivityFilter::CONVERSATION;
        assert!(filter.contains(ActivityFilter::CONVERSATION));
        assert!(!filter.contains(ActivityFilter::TOOLS));
    }

    #[test]
    fn activity_filter_toggle() {
        let mut filter = ActivityFilter::ALL;
        filter.toggle(ActivityFilter::CONVERSATION);
        assert!(!filter.contains(ActivityFilter::CONVERSATION));
        assert!(filter.contains(ActivityFilter::TOOLS));
    }

    #[test]
    fn inverted_index_search_single_word() {
        let mut index = InvertedIndex::new();
        index.index_line(0, "hello world");
        index.index_line(1, "hello rust");
        index.index_line(2, "goodbye world");

        let results = index.search("hello");
        assert_eq!(results, vec![0, 1]);
    }

    #[test]
    fn inverted_index_search_multiple_words_and() {
        let mut index = InvertedIndex::new();
        index.index_line(0, "hello world");
        index.index_line(1, "hello rust");
        index.index_line(2, "hello world rust");

        // "hello world" → lines containing BOTH words
        let results = index.search("hello world");
        assert_eq!(results, vec![0, 2]);
    }

    #[test]
    fn inverted_index_case_insensitive() {
        let mut index = InvertedIndex::new();
        index.index_line(0, "Hello World");
        index.index_line(1, "HELLO WORLD");

        let results = index.search("hello world");
        assert_eq!(results, vec![0, 1]);
    }

    #[test]
    fn inverted_index_min_word_length() {
        let mut index = InvertedIndex::new();
        index.index_line(0, "a bb ccc");

        // Only words >= 2 chars are indexed
        assert_eq!(index.search("a"), Vec::<usize>::new()); // Too short
        assert_eq!(index.search("bb"), vec![0]);
        assert_eq!(index.search("ccc"), vec![0]);
    }

    #[test]
    fn inverted_index_empty_query() {
        let index = InvertedIndex::new();
        assert_eq!(index.search(""), Vec::<usize>::new());
        assert_eq!(index.search("   "), Vec::<usize>::new());
    }

    #[test]
    fn line_metadata_tool_lookup() {
        let mut meta = LineMetadata::new();
        meta.record_tool(42, "tool_abc123".to_string());

        assert_eq!(meta.line_for_tool("tool_abc123"), Some(42));
        assert_eq!(meta.line_for_tool("nonexistent"), None);
    }

    #[test]
    fn line_metadata_step_linkage() {
        let mut meta = LineMetadata::new();
        meta.link_to_step(10, 3);

        assert_eq!(meta.step_for_line(10), Some(3));
        assert_eq!(meta.step_for_line(99), None);
    }

    #[test]
    fn line_metadata_round_grouping() {
        let mut meta = LineMetadata::new();
        meta.add_to_round(1, 5);
        meta.add_to_round(1, 6);
        meta.add_to_round(2, 10);

        assert_eq!(meta.lines_in_round(1), &[5, 6]);
        assert_eq!(meta.lines_in_round(2), &[10]);
        assert_eq!(meta.lines_in_round(99), &[] as &[usize]);
    }

    #[test]
    fn activity_model_push_and_search() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("hello world".into()));
        model.push(ActivityLine::AssistantText("hello rust".into()));
        model.push(ActivityLine::Info("goodbye".into()));

        assert_eq!(model.len(), 3);

        let results = model.search("hello");
        assert_eq!(results, vec![0, 1]);
    }

    #[test]
    fn activity_model_filter_conversation_only() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("user".into()));
        model.push(ActivityLine::Info("info".into()));
        model.push(ActivityLine::AssistantText("assistant".into()));

        // Enable only conversation filter
        model.filters = ActivityFilter::CONVERSATION;

        let filtered: Vec<usize> = model.filter_active().map(|(idx, _)| idx).collect();
        assert_eq!(filtered, vec![0, 2]); // user + assistant only
    }

    #[test]
    fn activity_model_filter_all_shows_everything() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("user".into()));
        model.push(ActivityLine::Info("info".into()));
        model.push(ActivityLine::AssistantText("assistant".into()));

        model.filters = ActivityFilter::ALL;

        let filtered: Vec<usize> = model.filter_active().map(|(idx, _)| idx).collect();
        assert_eq!(filtered, vec![0, 1, 2]); // All lines
    }

    #[test]
    fn activity_model_clear() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("test".into()));
        model.metadata.record_tool(0, "tool_1".into());

        model.clear();

        assert_eq!(model.len(), 0);
        assert_eq!(model.metadata.tool_to_line.len(), 0);
    }

    // --- P0.2: New method tests ---

    #[test]
    fn push_assistant_text_creates_new_line() {
        let mut model = ActivityModel::new();
        model.push_assistant_text("Hello".into());

        assert_eq!(model.len(), 1);
        assert!(matches!(model.get(0), Some(ActivityLine::AssistantText(s)) if s == "Hello"));
    }

    #[test]
    fn push_assistant_text_accumulates_to_last_line() {
        let mut model = ActivityModel::new();
        model.push_assistant_text("Hello".into());
        model.push_assistant_text(" world".into());

        assert_eq!(model.len(), 1); // Only 1 line, not 2
        assert!(matches!(model.get(0), Some(ActivityLine::AssistantText(s)) if s == "Hello world"));
    }

    #[test]
    fn push_assistant_text_creates_new_after_other_type() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::Info("separator".into()));
        model.push_assistant_text("New text".into());

        assert_eq!(model.len(), 2);
        assert!(matches!(model.get(1), Some(ActivityLine::AssistantText(s)) if s == "New text"));
    }

    #[test]
    fn set_plan_overview_creates_new() {
        use crate::tui::events::{PlanStepStatus, PlanStepDisplayStatus};

        let mut model = ActivityModel::new();
        let steps = vec![PlanStepStatus {
            description: "Step 1".into(),
            tool_name: Some("file_read".into()),
            status: PlanStepDisplayStatus::Pending,
            duration_ms: None,
        }];

        model.set_plan_overview("Goal: Fix bug".into(), steps.clone(), 0);

        assert_eq!(model.len(), 1);
        assert!(matches!(model.get(0), Some(ActivityLine::PlanOverview { goal, .. }) if goal == "Goal: Fix bug"));
    }

    #[test]
    fn set_plan_overview_replaces_existing() {
        use crate::tui::events::{PlanStepStatus, PlanStepDisplayStatus};

        let mut model = ActivityModel::new();
        let steps1 = vec![PlanStepStatus {
            description: "Old step".into(),
            tool_name: None,
            status: PlanStepDisplayStatus::Pending,
            duration_ms: None,
        }];
        let steps2 = vec![PlanStepStatus {
            description: "New step".into(),
            tool_name: None,
            status: PlanStepDisplayStatus::InProgress,
            duration_ms: None,
        }];

        model.set_plan_overview("Old goal".into(), steps1, 0);
        model.set_plan_overview("New goal".into(), steps2.clone(), 1);

        assert_eq!(model.len(), 1); // Still only 1 PlanOverview (replaced, not duplicated)
        assert!(matches!(model.get(0), Some(ActivityLine::PlanOverview { goal, current_step, .. })
            if goal == "New goal" && *current_step == 1));
    }

    #[test]
    fn push_round_separator() {
        let mut model = ActivityModel::new();
        model.push_round_separator(5);

        assert_eq!(model.len(), 1);
        assert!(matches!(model.get(0), Some(ActivityLine::RoundSeparator(n)) if *n == 5));
    }

    #[test]
    fn complete_tool_finds_last_matching() {
        use super::super::activity_types::ToolResult;

        let mut model = ActivityModel::new();

        // Add multiple tool execs with same name
        model.push(ActivityLine::ToolExec {
            name: "file_read".into(),
            input_preview: "test1.txt".into(),
            result: None,
            expanded: false,
        });
        model.push(ActivityLine::ToolExec {
            name: "file_read".into(),
            input_preview: "test2.txt".into(),
            result: None,
            expanded: false,
        });

        // Complete should target the LAST one
        model.complete_tool("file_read", "content".into(), false, 100);

        // First should still be None
        assert!(matches!(model.get(0), Some(ActivityLine::ToolExec { result: None, .. })));

        // Second should be completed
        assert!(matches!(model.get(1), Some(ActivityLine::ToolExec {
            result: Some(ToolResult { ref content, is_error, duration_ms }),
            ..
        }) if content == "content" && !is_error && *duration_ms == 100));
    }

    // --- P0.4A: Convenience wrapper tests ---

    #[test]
    fn push_info_creates_info_line() {
        let mut model = ActivityModel::new();
        model.push_info("Test info message");

        assert_eq!(model.len(), 1);
        assert!(matches!(model.get(0), Some(ActivityLine::Info(s)) if s == "Test info message"));
    }

    #[test]
    fn push_warning_with_hint() {
        let mut model = ActivityModel::new();
        model.push_warning("Warning message", Some("Try this fix"));

        assert_eq!(model.len(), 1);
        assert!(matches!(
            model.get(0),
            Some(ActivityLine::Warning { message, hint })
                if message == "Warning message" && hint.as_deref() == Some("Try this fix")
        ));
    }

    #[test]
    fn push_warning_without_hint() {
        let mut model = ActivityModel::new();
        model.push_warning("Warning message", None);

        assert_eq!(model.len(), 1);
        assert!(matches!(
            model.get(0),
            Some(ActivityLine::Warning { message, hint })
                if message == "Warning message" && hint.is_none()
        ));
    }

    #[test]
    fn push_error_with_hint() {
        let mut model = ActivityModel::new();
        model.push_error("Error occurred", Some("Check logs"));

        assert_eq!(model.len(), 1);
        assert!(matches!(
            model.get(0),
            Some(ActivityLine::Error { message, hint })
                if message == "Error occurred" && hint.as_deref() == Some("Check logs")
        ));
    }

    #[test]
    fn push_user_prompt_creates_prompt_line() {
        let mut model = ActivityModel::new();
        model.push_user_prompt("User input here");

        assert_eq!(model.len(), 1);
        assert!(matches!(model.get(0), Some(ActivityLine::UserPrompt(s)) if s == "User input here"));
    }

    #[test]
    fn push_code_block_creates_block() {
        let mut model = ActivityModel::new();
        model.push_code_block("rust", "fn main() {}");

        assert_eq!(model.len(), 1);
        assert!(matches!(
            model.get(0),
            Some(ActivityLine::CodeBlock { lang, code })
                if lang == "rust" && code == "fn main() {}"
        ));
    }

    #[test]
    fn push_tool_start_creates_skeleton() {
        let mut model = ActivityModel::new();
        model.push_tool_start("file_read", "test.txt");

        assert_eq!(model.len(), 1);
        assert!(matches!(
            model.get(0),
            Some(ActivityLine::ToolExec { name, input_preview, result, expanded })
                if name == "file_read" && input_preview == "test.txt" && result.is_none() && !expanded
        ));
    }

    #[test]
    fn line_count_matches_len() {
        let mut model = ActivityModel::new();
        model.push_info("Line 1");
        model.push_info("Line 2");

        assert_eq!(model.line_count(), 2);
        assert_eq!(model.line_count(), model.len());
    }

    #[test]
    fn has_loading_tools_detects_incomplete() {
        let mut model = ActivityModel::new();
        model.push_tool_start("bash", "ls -la");

        assert!(model.has_loading_tools());
    }

    #[test]
    fn has_loading_tools_false_when_all_complete() {
        let mut model = ActivityModel::new();
        model.push_tool_start("bash", "ls -la");
        model.complete_tool("bash", "output".into(), false, 100);

        assert!(!model.has_loading_tools());
    }

    #[test]
    fn has_loading_tools_false_when_no_tools() {
        let model = ActivityModel::new();
        assert!(!model.has_loading_tools());
    }

    // Phase 3 SRCH-001: Fuzzy search tests

    #[test]
    fn levenshtein_distance_identical() {
        assert_eq!(InvertedIndex::levenshtein_distance("hello", "hello"), 0);
    }

    #[test]
    fn levenshtein_distance_one_substitution() {
        assert_eq!(InvertedIndex::levenshtein_distance("hello", "hallo"), 1);
    }

    #[test]
    fn levenshtein_distance_one_insertion() {
        assert_eq!(InvertedIndex::levenshtein_distance("hello", "helllo"), 1);
    }

    #[test]
    fn levenshtein_distance_one_deletion() {
        assert_eq!(InvertedIndex::levenshtein_distance("hello", "helo"), 1);
    }

    #[test]
    fn levenshtein_distance_multiple_edits() {
        assert_eq!(InvertedIndex::levenshtein_distance("kitten", "sitting"), 3);
    }

    #[test]
    fn levenshtein_distance_empty_strings() {
        assert_eq!(InvertedIndex::levenshtein_distance("", ""), 0);
        assert_eq!(InvertedIndex::levenshtein_distance("hello", ""), 5);
        assert_eq!(InvertedIndex::levenshtein_distance("", "world"), 5);
    }

    #[test]
    fn fuzzy_search_exact_match() {
        let mut index = InvertedIndex::new();
        index.index_line(0, "hello world");
        index.index_line(1, "hello rust");

        // Exact match should work with fuzzy search
        let results = index.fuzzy_search("hello", 2);
        assert_eq!(results, vec![0, 1]);
    }

    #[test]
    fn fuzzy_search_one_typo() {
        let mut index = InvertedIndex::new();
        index.index_line(0, "hello world");
        index.index_line(1, "goodbye world");

        // "helo" (1 typo) should match "hello"
        let results = index.fuzzy_search("helo", 2);
        assert_eq!(results, vec![0]);
    }

    #[test]
    fn fuzzy_search_two_typos() {
        let mut index = InvertedIndex::new();
        index.index_line(0, "testing fuzzy search");
        index.index_line(1, "another line");

        // "testng" (2 typos: missing 'i', missing 's') should match "testing"
        let results = index.fuzzy_search("testng", 2);
        assert_eq!(results, vec![0]);
    }

    #[test]
    fn fuzzy_search_exceeds_distance() {
        let mut index = InvertedIndex::new();
        index.index_line(0, "hello world");

        // "xyz" is >2 edits away from "hello", should not match
        let results = index.fuzzy_search("xyz", 2);
        assert!(results.is_empty());
    }

    #[test]
    fn fuzzy_search_multiple_words_and() {
        let mut index = InvertedIndex::new();
        index.index_line(0, "hello world");
        index.index_line(1, "hello rust");
        index.index_line(2, "helo wrld"); // Both words have typos

        // "helo wrld" with distance 2 should match both line 0 and 2
        // Line 0: "hello" and "world" are within 1 edit of "helo" and "wrld"
        // Line 2: exact match for "helo" and "wrld"
        let results = index.fuzzy_search("helo wrld", 2);
        assert_eq!(results, vec![0, 2]);

        // "hello world" should match both line 0 (exact) and line 2 (fuzzy)
        // because "hello"/"helo" and "world"/"wrld" are within distance 2
        let results = index.fuzzy_search("hello world", 2);
        assert_eq!(results, vec![0, 2]);
    }

    #[test]
    fn fuzzy_search_zero_distance_is_exact() {
        let mut index = InvertedIndex::new();
        index.index_line(0, "hello world");
        index.index_line(1, "helo world");

        // distance=0 should only find exact matches
        let results = index.fuzzy_search("hello", 0);
        assert_eq!(results, vec![0]);
    }

    #[test]
    fn fuzzy_search_case_insensitive() {
        let mut index = InvertedIndex::new();
        index.index_line(0, "Hello World");

        // Should match despite case difference
        let results = index.fuzzy_search("helo", 2);
        assert_eq!(results, vec![0]);
    }

    #[test]
    fn activity_model_fuzzy_search_integration() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("testing fuzzy search".into()));
        model.push(ActivityLine::AssistantText("another line here".into()));
        model.push(ActivityLine::UserPrompt("testng typo example".into()));

        // "testng" should match both lines 0 and 2 (within distance 2)
        let results = model.fuzzy_search("testng", 2);
        assert_eq!(results, vec![0, 2]);
    }

    // Phase 3 SRCH-002: Regex search tests

    #[test]
    fn regex_search_simple_pattern() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("hello world".into()));
        model.push(ActivityLine::UserPrompt("hello rust".into()));
        model.push(ActivityLine::UserPrompt("goodbye world".into()));

        // Pattern "hello.*" should match lines 0 and 1
        let results = model.regex_search("hello.*");
        assert_eq!(results, vec![0, 1]);
    }

    #[test]
    fn regex_search_word_boundary() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("test testing tester".into()));
        model.push(ActivityLine::UserPrompt("test case".into()));

        // "\\btest\\b" should match exact word "test" (lines 0 and 1)
        let results = model.regex_search(r"\btest\b");
        assert_eq!(results, vec![0, 1]);
    }

    #[test]
    fn regex_search_digit_pattern() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("line 123".into()));
        model.push(ActivityLine::UserPrompt("no numbers here".into()));
        model.push(ActivityLine::UserPrompt("error code 456".into()));

        // "\\d+" should match lines with numbers
        let results = model.regex_search(r"\d+");
        assert_eq!(results, vec![0, 2]);
    }

    #[test]
    fn regex_search_case_insensitive() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("Hello World".into()));
        model.push(ActivityLine::UserPrompt("HELLO WORLD".into()));

        // "(?i)hello" should match both (case-insensitive)
        let results = model.regex_search("(?i)hello");
        assert_eq!(results, vec![0, 1]);
    }

    #[test]
    fn regex_search_invalid_pattern() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("some text".into()));

        // Invalid regex should return empty vec
        let results = model.regex_search("[invalid(");
        assert!(results.is_empty());
    }

    #[test]
    fn regex_search_empty_pattern() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("text 1".into()));
        model.push(ActivityLine::UserPrompt("text 2".into()));

        // Empty pattern matches all lines
        let results = model.regex_search("");
        assert_eq!(results, vec![0, 1]);
    }

    #[test]
    fn regex_search_no_matches() {
        let mut model = ActivityModel::new();
        model.push(ActivityLine::UserPrompt("hello world".into()));

        // Pattern that doesn't match
        let results = model.regex_search("xyz123");
        assert!(results.is_empty());
    }
}
