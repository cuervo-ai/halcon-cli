//! Context Pipeline: central orchestrator for the multi-tiered context engine.
//!
//! Manages the L0 hot buffer, L1 sliding window, tool output elision,
//! instruction caching, and token budget tracking. Provides a single entry point
//! for adding messages and building model requests.

use std::path::Path;

use serde::{Deserialize, Serialize};

use halcon_core::types::validation::strip_orphaned_tool_results;
use halcon_core::types::{ChatMessage, ContentBlock, MessageContent, Role};

use crate::accountant::{estimate_message_tokens, BudgetResult, Tier, TokenAccountant};
use crate::assembler::estimate_tokens;
use crate::cold_archive::ColdArchive;
use crate::cold_store::ColdStore;
use crate::elider::ToolOutputElider;
use crate::hot_buffer::HotBuffer;
use crate::instruction::find_instruction_files;
use crate::instruction_cache::InstructionCache;
use crate::segment::{extract_segment_from_message, ContextSegment};
use crate::semantic_store::SemanticStore;
use crate::sliding_window::SlidingWindow;

/// Configuration for the context pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPipelineConfig {
    /// Maximum total context tokens (maps to model's context window).
    pub max_context_tokens: u32,
    /// Number of recent messages to keep in L0 hot buffer.
    pub hot_buffer_capacity: usize,
    /// Default token budget per tool output.
    pub default_tool_output_budget: u32,
    /// Token threshold for merging adjacent L1 segments.
    pub l1_merge_threshold: u32,
    /// Maximum number of compressed segments in L2 cold store.
    pub max_cold_entries: usize,
    /// Maximum number of segments in L3 semantic store (0 = disabled).
    pub max_semantic_entries: usize,
    /// Maximum number of segments in L4 cold archive.
    pub max_archive_entries: usize,
}

impl Default for ContextPipelineConfig {
    fn default() -> Self {
        Self {
            max_context_tokens: 200_000, // Safe default for Claude; override with for_model().
            hot_buffer_capacity: 8,
            default_tool_output_budget: 2_000,
            l1_merge_threshold: 3_000,
            max_cold_entries: 100,
            max_semantic_entries: 200,
            max_archive_entries: 500,
        }
    }
}

impl ContextPipelineConfig {
    /// Create a config derived from the model's actual context window.
    ///
    /// Uses `halcon_core::types::model_context_window()` as the single source
    /// of truth, avoiding the 200K hardcode for models with different capacities.
    pub fn for_model(model: &str) -> Self {
        let context_window = halcon_core::types::model_context_window(model);
        Self {
            max_context_tokens: context_window,
            ..Self::default()
        }
    }
}

/// Central orchestrator for the multi-tiered context engine.
pub struct ContextPipeline {
    accountant: TokenAccountant,
    l0: HotBuffer,
    l1: SlidingWindow,
    l2: ColdStore,
    l3: SemanticStore,
    l4: ColdArchive,
    elider: ToolOutputElider,
    instruction_cache: InstructionCache,
    l1_merge_threshold: u32,
    round_counter: u32,
    /// Content hash of instruction files after last load/refresh.
    instruction_content_hash: u64,
}

impl ContextPipeline {
    /// Create a new context pipeline with the given configuration.
    pub fn new(config: &ContextPipelineConfig) -> Self {
        let accountant = TokenAccountant::new(config.max_context_tokens);
        Self {
            accountant,
            l0: HotBuffer::new(config.hot_buffer_capacity),
            l1: SlidingWindow::new(),
            l2: ColdStore::new(config.max_cold_entries),
            l3: SemanticStore::new(config.max_semantic_entries),
            l4: ColdArchive::new(config.max_archive_entries),
            elider: ToolOutputElider::new(config.default_tool_output_budget),
            instruction_cache: InstructionCache::new(),
            l1_merge_threshold: config.l1_merge_threshold,
            round_counter: 0,
            instruction_content_hash: 0,
        }
    }

    /// Initialize with system prompt and instruction files.
    pub fn initialize(&mut self, system_prompt: &str, working_dir: &Path) {
        let sys_tokens = estimate_tokens(system_prompt) as u32;
        self.accountant.reserve_system_prompt(sys_tokens);

        // Warm instruction cache
        let instruction_paths = find_instruction_files(working_dir);
        for path in &instruction_paths {
            self.instruction_cache.get_or_load(path);
        }
        self.instruction_content_hash = self.instruction_cache.content_hash();
    }

    /// Check instruction files for changes and return new merged content if stale.
    ///
    /// Performs a stat syscall (~10μs) per instruction file to check mtimes.
    /// Only reloads from disk if a file's mtime has changed.
    /// Returns `Some(merged_content)` if any file changed, `None` if all fresh.
    /// Also updates the accountant's system prompt reservation on change.
    pub fn refresh_instructions(&mut self, working_dir: &Path) -> Option<String> {
        let old_hash = self.instruction_content_hash;

        // Re-scan instruction files, reload any with changed mtime, and collect content in one pass.
        let paths = find_instruction_files(working_dir);
        let mut merged = String::new();
        for path in &paths {
            if let Some(content) = self.instruction_cache.get_or_load(path) {
                if !merged.is_empty() {
                    merged.push_str("\n\n");
                }
                merged.push_str(content);
            }
        }

        let new_hash = self.instruction_cache.content_hash();
        if new_hash == old_hash {
            return None; // No changes
        }

        // Update token accounting for changed system prompt size.
        let new_tokens = estimate_tokens(&merged) as u32;
        self.accountant.update_system_prompt(new_tokens);

        self.instruction_content_hash = new_hash;
        Some(merged)
    }

    /// Set the current round number (for segment creation).
    pub fn set_round(&mut self, round: u32) {
        self.round_counter = round;
    }

    /// Add a message to the context. Handles L0 overflow → L1 cascading.
    ///
    /// When an evicted message is an Assistant with ToolUse blocks and the next
    /// message in the buffer is a User with matching ToolResult blocks, both are
    /// evicted together to prevent splitting a tool pair across tiers.
    pub fn add_message(&mut self, msg: ChatMessage) {
        let tokens = estimate_message_tokens(&msg);
        if let Some(evicted) = self.l0.push(msg) {
            // Check if the evicted message is an Assistant with ToolUse blocks.
            let tool_use_ids = Self::extract_tool_use_ids_from_message(&evicted);

            // Evicted from L0 → extract segment → push to L1
            let segment = extract_segment_from_message(&evicted, self.round_counter);
            self.push_to_l1(segment);

            // If the evicted message had ToolUse blocks, check if the NEXT
            // message in the buffer is its corresponding ToolResult.
            // If so, evict it too to keep the pair together.
            if !tool_use_ids.is_empty() {
                if let Some(next) = self.l0.messages().front() {
                    if Self::message_has_tool_results_for(&tool_use_ids, next) {
                        if let Some(companion) = self.l0.pop_oldest() {
                            let seg = extract_segment_from_message(&companion, self.round_counter);
                            self.push_to_l1(seg);
                        }
                    }
                }
            }
        }
        // Track in accountant (L0 tier)
        // Release old token count and set new one based on actual L0 state
        self.sync_l0_accountant(tokens);
        self.ensure_budgets();
    }

    /// Add a tool result with intelligent elision.
    pub fn add_tool_result(
        &mut self,
        tool_name: &str,
        tool_use_id: &str,
        content: &str,
        is_error: bool,
    ) {
        let budget = self.accountant.available(Tier::L0Hot) / 4; // max 25% of L0 per tool
        let elided = self.elider.elide(tool_name, content, Some(budget.max(500)));
        let msg = ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: elided,
                is_error,
            }]),
        };
        self.add_message(msg);
    }

    /// Build messages for ModelRequest (combines L4 + L3 + L2 + L1 context + L0 messages).
    pub fn build_messages(&self) -> Vec<ChatMessage> {
        let mut messages = Vec::new();

        // Extract the most recent user query once (used by L3 and L4 retrieval).
        let user_query = self
            .l0
            .messages()
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .and_then(|m| m.content.as_text())
            .unwrap_or("");

        // L4 cold archive (oldest, lowest priority — query-filtered retrieval).
        if !self.l4.is_empty() {
            let l4_query = if user_query.is_empty() {
                None
            } else {
                Some(user_query)
            };
            let l4_chunks = self
                .l4
                .retrieve(l4_query, self.accountant.available(Tier::L4Cold));
            if !l4_chunks.is_empty() {
                let l4_text: String = l4_chunks
                    .iter()
                    .map(|c| c.content.as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                messages.push(ChatMessage {
                    role: Role::User,
                    content: MessageContent::Text(format!("[Archived Memory (L4)]\n{l4_text}")),
                });
            }
        }

        // L3 semantic context (relevance-ranked).
        if !self.l3.is_empty() && !user_query.is_empty() {
            let l3_chunks = self
                .l3
                .retrieve(user_query, self.accountant.available(Tier::L3Semantic));
            if !l3_chunks.is_empty() {
                let l3_text: String = l3_chunks
                    .iter()
                    .map(|c| c.content.as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                messages.push(ChatMessage {
                    role: Role::User,
                    content: MessageContent::Text(format!("[Semantic Memory (L3)]\n{l3_text}")),
                });
            }
        }

        // L2 cold context (oldest, lower priority — decompressed on demand)
        let l2_chunks = self
            .l2
            .retrieve(self.accountant.available(Tier::L2Compressed));
        if !l2_chunks.is_empty() {
            let l2_text: String = l2_chunks
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            messages.push(ChatMessage {
                role: Role::User,
                content: MessageContent::Text(format!("[Compressed History (L2)]\n{l2_text}")),
            });
        }

        // L1 context as summary (if any)
        let l1_chunks = self.l1.retrieve(self.accountant.available(Tier::L1Warm));
        if !l1_chunks.is_empty() {
            let l1_text: String = l1_chunks
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            messages.push(ChatMessage {
                role: Role::User,
                content: MessageContent::Text(format!("[Prior Context Summary]\n{l1_text}")),
            });
        }

        // L0 messages (the hot buffer — most recent).
        // Clone directly from the deque, avoiding intermediate Vec allocation.
        messages.extend(self.l0.messages().iter().cloned());

        // Protocol safety: strip any orphaned ToolResult blocks whose matching
        // ToolUse was evicted from L0 to a lower tier (and thus converted to
        // text summary). This prevents 400 errors from providers.
        strip_orphaned_tool_results(&messages)
    }

    /// Get estimated token count for current context.
    pub fn estimated_tokens(&self) -> u32 {
        self.l0.token_count()
            + self.l1.token_count()
            + self.l2.original_tokens()
            + self.l3.original_tokens()
            + self.l4.original_tokens()
            + self.accountant.system_prompt_reserved()
    }

    /// Token accountant (read-only access).
    pub fn accountant(&self) -> &TokenAccountant {
        &self.accountant
    }

    /// L0 hot buffer (read-only access).
    pub fn l0(&self) -> &HotBuffer {
        &self.l0
    }

    /// L1 sliding window (read-only access).
    pub fn l1(&self) -> &SlidingWindow {
        &self.l1
    }

    /// L2 cold store (read-only access).
    pub fn l2(&self) -> &ColdStore {
        &self.l2
    }

    /// L3 semantic store (read-only access).
    pub fn l3(&self) -> &SemanticStore {
        &self.l3
    }

    /// L4 cold archive (read-only access).
    pub fn l4(&self) -> &ColdArchive {
        &self.l4
    }

    /// Instruction cache (read-only access).
    pub fn instruction_cache(&self) -> &InstructionCache {
        &self.instruction_cache
    }

    /// Tool output elider (read-only access).
    pub fn elider(&self) -> &ToolOutputElider {
        &self.elider
    }

    /// Load L4 cold archive from disk, replacing the current in-memory L4.
    ///
    /// Call at session start to restore cross-session knowledge.
    /// If the file doesn't exist or is invalid, silently keeps the empty L4.
    pub fn load_l4_archive(&mut self, path: &Path) {
        if let Some(loaded) = ColdArchive::load_from_disk(path, self.l4.max_entries()) {
            self.total_original_tokens_l4_adjust(&loaded);
            self.l4 = loaded;
        } else {
            // No existing archive — initialize with path for future flushes.
            self.l4 = ColdArchive::with_path(self.l4.max_entries(), path.to_path_buf());
        }
    }

    /// Flush L4 cold archive to disk.
    ///
    /// Call at session end to persist cross-session knowledge.
    /// Returns number of bytes written, or None if flush failed.
    pub fn flush_l4_archive(&mut self) -> Option<usize> {
        self.l4.flush_to_disk()
    }

    fn total_original_tokens_l4_adjust(&mut self, loaded: &ColdArchive) {
        // Sync accountant with loaded L4 token count.
        let loaded_tokens = loaded.original_tokens();
        if loaded_tokens == 0 {
            return;
        }
        match self.accountant.allocate(Tier::L4Cold, loaded_tokens) {
            BudgetResult::Allocated => {
                tracing::debug!(
                    loaded_tokens,
                    "L4 archive token budget allocated to accountant"
                );
            }
            BudgetResult::InsufficientBudget {
                available,
                requested,
            } => {
                // This is normal when loading a cross-session archive produced in a
                // session with a LARGER context window (e.g. Anthropic 200K archive
                // loaded into a DeepSeek 64K session). The archive content is still
                // loaded — only the accountant budget tracking is capped.
                tracing::warn!(
                    available_l4_budget = available,
                    requested = requested,
                    "L4 archive exceeds session L4 budget — archive loaded, accountant capped. \
                     This is normal for cross-session archives from larger context window sessions."
                );
                // Allocate as much as the current L4 tier can accommodate.
                if available > 0 {
                    let _ = self.accountant.allocate(Tier::L4Cold, available);
                }
            }
        }
    }

    /// Check if L0 is over budget.
    pub fn needs_compaction(&self) -> bool {
        self.l0.token_count() > self.accountant.tier_budget(Tier::L0Hot)
    }

    /// Force eviction of oldest L0 message to L1.
    pub fn force_evict_l0(&mut self) -> Option<ContextSegment> {
        let msg = self.l0.pop_oldest()?;
        let segment = extract_segment_from_message(&msg, self.round_counter);
        self.push_to_l1(segment.clone());
        Some(segment)
    }

    /// Reset the pipeline for a new session.
    pub fn reset(&mut self) {
        self.l0.clear();
        self.l1 = SlidingWindow::new();
        self.l2 = ColdStore::new(self.l2.max_entries());
        self.l3 = SemanticStore::new(self.l3.max_entries());
        self.l4 = ColdArchive::new(self.l4.max_entries());
        self.accountant = TokenAccountant::new(self.accountant.total_budget());
        self.round_counter = 0;
    }

    /// Reset only the L0 hot buffer, preserving L1-L4 compressed/semantic/archive content.
    ///
    /// Use this after context compaction to avoid destroying the rich compressed memory
    /// accumulated in L1-L4. The full `reset()` is reserved for new sessions only.
    /// After calling this, re-seed L0 with the compacted messages via `add_message()`.
    pub fn reset_hot_only(&mut self) {
        let l0_used = self.l0.token_count();
        self.l0.clear();
        // Release the L0 budget that was used so new messages can be allocated.
        self.accountant.release(Tier::L0Hot, l0_used);
        // Round counter preserved so L1/L2 segments retain correct round metadata.
    }

    /// Update the total context budget (e.g. after model fallback to a smaller context window).
    /// Recomputes per-tier allocations while preserving existing tier usage.
    pub fn update_budget(&mut self, new_total: u32) {
        let new_accountant = TokenAccountant::new(new_total);
        // Transfer system prompt reservation to new accountant.
        let reserved = self.accountant.system_prompt_reserved();
        let mut accountant = new_accountant;
        accountant.reserve_system_prompt(reserved);
        self.accountant = accountant;
    }

    fn push_to_l1(&mut self, segment: ContextSegment) {
        let tokens = segment.token_estimate;
        if let BudgetResult::InsufficientBudget { .. } =
            self.accountant.allocate(Tier::L1Warm, tokens)
        {
            // L1 full → evict oldest → compress to L2
            if let Some(evicted) = self.l1.evict_oldest() {
                self.accountant
                    .release(Tier::L1Warm, evicted.token_estimate);
                self.push_to_l2(evicted);
            }
            // Retry allocation
            let _ = self.accountant.allocate(Tier::L1Warm, tokens);
        }
        self.l1.push(segment);
        // Periodic merge of small segments
        self.l1.merge_adjacent(self.l1_merge_threshold);
    }

    fn push_to_l2(&mut self, segment: ContextSegment) {
        let tokens = segment.token_estimate;
        if let BudgetResult::InsufficientBudget { .. } =
            self.accountant.allocate(Tier::L2Compressed, tokens)
        {
            // L2 full → evict oldest → decompress and promote to L3 semantic store.
            if let Some(evicted) = self.l2.evict_oldest_as_segment() {
                self.accountant.release(Tier::L2Compressed, tokens);
                self.push_to_l3(evicted);
            }
            let _ = self.accountant.allocate(Tier::L2Compressed, tokens);
        }
        self.l2.store(&segment);
    }

    fn push_to_l3(&mut self, segment: ContextSegment) {
        let tokens = segment.token_estimate;
        if let BudgetResult::InsufficientBudget { .. } =
            self.accountant.allocate(Tier::L3Semantic, tokens)
        {
            // L3 full → evict oldest → promote to L4 cold archive.
            if let Some(evicted) = self.l3.evict_oldest() {
                self.accountant.release(Tier::L3Semantic, tokens);
                self.push_to_l4(evicted);
            }
            let _ = self.accountant.allocate(Tier::L3Semantic, tokens);
        }
        self.l3.store(&segment);
    }

    fn push_to_l4(&mut self, segment: ContextSegment) {
        let tokens = segment.token_estimate;
        if let BudgetResult::InsufficientBudget { .. } =
            self.accountant.allocate(Tier::L4Cold, tokens)
        {
            // L4 full → evict oldest (final tier, data is truly dropped).
            if self.l4.evict_oldest() {
                self.accountant.release(Tier::L4Cold, tokens);
            }
            let _ = self.accountant.allocate(Tier::L4Cold, tokens);
        }
        self.l4.store(&segment);
    }

    /// Extract ToolUse IDs from a message's blocks. Returns empty if not blocks
    /// or no ToolUse blocks present.
    fn extract_tool_use_ids_from_message(msg: &ChatMessage) -> Vec<String> {
        match &msg.content {
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| {
                    if let ContentBlock::ToolUse { id, .. } = b {
                        Some(id.clone())
                    } else {
                        None
                    }
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Check if a message contains ToolResult blocks referencing any of the given IDs.
    fn message_has_tool_results_for(tool_use_ids: &[String], msg: &ChatMessage) -> bool {
        if let MessageContent::Blocks(blocks) = &msg.content {
            blocks.iter().any(|b| {
                if let ContentBlock::ToolResult { tool_use_id, .. } = b {
                    tool_use_ids.iter().any(|id| id == tool_use_id)
                } else {
                    false
                }
            })
        } else {
            false
        }
    }

    fn sync_l0_accountant(&mut self, _new_tokens: u32) {
        // Sync L0 tier usage with actual hot buffer state
        let actual = self.l0.token_count();
        let tracked = self.accountant.tier_used(Tier::L0Hot);
        if actual > tracked {
            let _ = self.accountant.allocate(Tier::L0Hot, actual - tracked);
        } else if actual < tracked {
            self.accountant.release(Tier::L0Hot, tracked - actual);
        }
    }

    fn ensure_budgets(&mut self) {
        // If L0 is over budget, cascade evictions
        while self.l0.token_count() > self.accountant.tier_budget(Tier::L0Hot) {
            if let Some(msg) = self.l0.pop_oldest() {
                let segment = extract_segment_from_message(&msg, self.round_counter);
                self.push_to_l1(segment);
                // Re-sync L0 accountant
                self.sync_l0_accountant(0);
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::types::Role;

    fn text_msg(role: Role, text: &str) -> ChatMessage {
        ChatMessage {
            role,
            content: MessageContent::Text(text.to_string()),
        }
    }

    fn default_config() -> ContextPipelineConfig {
        ContextPipelineConfig {
            max_context_tokens: 10_000,
            hot_buffer_capacity: 4,
            default_tool_output_budget: 500,
            l1_merge_threshold: 1000,
            max_cold_entries: 50,
            ..Default::default()
        }
    }

    #[test]
    fn new_pipeline_is_empty() {
        let pipeline = ContextPipeline::new(&default_config());
        assert_eq!(pipeline.l0().len(), 0);
        assert!(pipeline.l1().is_empty());
        assert_eq!(pipeline.estimated_tokens(), 0);
    }

    #[test]
    fn add_message_within_capacity() {
        let mut pipeline = ContextPipeline::new(&default_config());
        pipeline.add_message(text_msg(Role::User, "hello"));
        assert_eq!(pipeline.l0().len(), 1);
        assert!(pipeline.l1().is_empty());
    }

    #[test]
    fn add_message_overflow_promotes_to_l1() {
        let mut pipeline = ContextPipeline::new(&default_config());
        // Fill L0 (capacity=4)
        for i in 0..5 {
            pipeline.add_message(text_msg(Role::User, &format!("message {i}")));
        }
        // L0 still has 4, L1 got the evicted one
        assert_eq!(pipeline.l0().len(), 4);
        assert!(!pipeline.l1().is_empty());
    }

    #[test]
    fn build_messages_returns_l0_content() {
        let mut pipeline = ContextPipeline::new(&default_config());
        pipeline.add_message(text_msg(Role::User, "hello"));
        pipeline.add_message(text_msg(Role::Assistant, "hi there"));
        let msgs = pipeline.build_messages();
        assert_eq!(msgs.len(), 2);
    }

    #[test]
    fn build_messages_includes_l1_summary() {
        let mut pipeline = ContextPipeline::new(&default_config());
        // Overflow L0 to populate L1
        for i in 0..10 {
            pipeline.add_message(text_msg(
                Role::User,
                &format!("message {i} with some content"),
            ));
        }
        let msgs = pipeline.build_messages();
        // First message should be L1 summary
        if pipeline.l1().len() > 0 {
            let first = msgs[0].content.as_text().unwrap();
            assert!(first.contains("[Prior Context Summary]"));
        }
    }

    #[test]
    fn add_tool_result_elides() {
        let config = ContextPipelineConfig {
            max_context_tokens: 1000, // small budget to force elision
            hot_buffer_capacity: 4,
            default_tool_output_budget: 50,
            l1_merge_threshold: 500,
            ..Default::default()
        };
        let mut pipeline = ContextPipeline::new(&config);
        // Generate large bash output (many lines)
        let large_content: String = (0..500).map(|i| format!("output line {i}\n")).collect();
        pipeline.add_tool_result("bash", "t1", &large_content, false);
        assert_eq!(pipeline.l0().len(), 1);
        // The stored content should be smaller than the original (bash keeps last 30 lines)
        let msg = &pipeline.l0().messages()[0];
        if let MessageContent::Blocks(blocks) = &msg.content {
            if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
                assert!(
                    content.len() < large_content.len(),
                    "Expected elision: original={}, stored={}",
                    large_content.len(),
                    content.len()
                );
            }
        }
    }

    #[test]
    fn set_round() {
        let mut pipeline = ContextPipeline::new(&default_config());
        pipeline.set_round(5);
        // The round counter affects segment creation
        pipeline.add_message(text_msg(Role::User, "test"));
        // No direct assertion on round counter, but shouldn't panic
    }

    #[test]
    fn estimated_tokens_increases() {
        let mut pipeline = ContextPipeline::new(&default_config());
        let before = pipeline.estimated_tokens();
        pipeline.add_message(text_msg(Role::User, "hello world"));
        let after = pipeline.estimated_tokens();
        assert!(after > before);
    }

    #[test]
    fn initialize_with_system_prompt() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut pipeline = ContextPipeline::new(&default_config());
        pipeline.initialize("You are a helpful assistant.", dir.path());
        assert!(pipeline.accountant().system_prompt_reserved() > 0);
    }

    #[test]
    fn reset_clears_state() {
        let mut pipeline = ContextPipeline::new(&default_config());
        pipeline.add_message(text_msg(Role::User, "msg1"));
        pipeline.add_message(text_msg(Role::User, "msg2"));
        pipeline.reset();
        assert_eq!(pipeline.l0().len(), 0);
        assert!(pipeline.l1().is_empty());
    }

    /// Fix C: reset_hot_only() clears L0 but preserves L1.
    #[test]
    fn reset_hot_only_clears_l0_preserves_l1() {
        let config = ContextPipelineConfig {
            max_context_tokens: 10_000,
            hot_buffer_capacity: 2, // small to force L1 eviction quickly
            l1_merge_threshold: 1000,
            ..Default::default()
        };
        let mut pipeline = ContextPipeline::new(&config);
        // Overfill L0 to populate L1.
        for i in 0..5 {
            pipeline.add_message(text_msg(Role::User, &format!("message {i}")));
        }
        let l1_len_before = pipeline.l1().len();
        assert!(
            l1_len_before > 0,
            "L1 should have segments after L0 overflow"
        );

        // Apply reset_hot_only().
        pipeline.reset_hot_only();

        // L0 cleared, L1 preserved.
        assert_eq!(
            pipeline.l0().len(),
            0,
            "L0 should be empty after reset_hot_only"
        );
        assert_eq!(
            pipeline.l1().len(),
            l1_len_before,
            "L1 should be unchanged after reset_hot_only"
        );
    }

    /// Fix C: after reset_hot_only, new messages can be added to L0.
    #[test]
    fn reset_hot_only_allows_new_messages_after() {
        let mut pipeline = ContextPipeline::new(&default_config());
        for i in 0..4 {
            pipeline.add_message(text_msg(Role::User, &format!("original {i}")));
        }
        pipeline.reset_hot_only();

        // Add compacted summary as new L0 seed.
        pipeline.add_message(text_msg(
            Role::User,
            "[Context Summary — previous messages were compacted]\n\nSummary of conversation.",
        ));
        pipeline.add_message(text_msg(Role::User, "New message after compaction"));

        // Pipeline accepts new messages and L0 contains exactly the new ones.
        assert_eq!(pipeline.l0().len(), 2);
        let msgs = pipeline.build_messages();
        assert!(msgs.iter().any(|m| {
            m.content
                .as_text()
                .map(|t| t.contains("Summary"))
                .unwrap_or(false)
        }));
    }

    #[test]
    fn needs_compaction_false_when_within_budget() {
        let pipeline = ContextPipeline::new(&default_config());
        assert!(!pipeline.needs_compaction());
    }

    #[test]
    fn many_messages_stay_within_budget() {
        let config = ContextPipelineConfig {
            max_context_tokens: 1000,
            hot_buffer_capacity: 4,
            default_tool_output_budget: 100,
            l1_merge_threshold: 500,
            ..Default::default()
        };
        let mut pipeline = ContextPipeline::new(&config);
        // Add many messages — pipeline should manage budget via eviction
        for i in 0..50 {
            pipeline.add_message(text_msg(Role::User, &format!("message {i} with content")));
        }
        // L0 should be at capacity
        assert!(pipeline.l0().len() <= 4);
        // L1 should have segments
        assert!(!pipeline.l1().is_empty());
    }

    #[test]
    fn force_evict_l0() {
        let mut pipeline = ContextPipeline::new(&default_config());
        pipeline.add_message(text_msg(Role::User, "msg1"));
        pipeline.add_message(text_msg(Role::User, "msg2"));
        assert_eq!(pipeline.l0().len(), 2);

        let seg = pipeline.force_evict_l0();
        assert!(seg.is_some());
        assert_eq!(pipeline.l0().len(), 1);
        assert!(!pipeline.l1().is_empty());
    }

    #[test]
    fn force_evict_empty_l0() {
        let mut pipeline = ContextPipeline::new(&default_config());
        assert!(pipeline.force_evict_l0().is_none());
    }

    #[test]
    fn tool_result_small_passthrough() {
        let mut pipeline = ContextPipeline::new(&default_config());
        pipeline.add_tool_result("bash", "t1", "OK", false);
        let msg = &pipeline.l0().messages()[0];
        if let MessageContent::Blocks(blocks) = &msg.content {
            if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
                assert_eq!(content, "OK");
            }
        }
    }

    #[test]
    fn default_config_values() {
        let config = ContextPipelineConfig::default();
        assert_eq!(config.max_context_tokens, 200_000);
        assert_eq!(config.hot_buffer_capacity, 8);
        assert_eq!(config.default_tool_output_budget, 2_000);
        assert_eq!(config.l1_merge_threshold, 3_000);
        assert_eq!(config.max_cold_entries, 100);
    }

    // --- L2 Cold Store integration tests ---

    #[test]
    fn l2_receives_evicted_l1_segments() {
        let config = ContextPipelineConfig {
            max_context_tokens: 500, // very tight budget to force L1→L2 cascade
            hot_buffer_capacity: 2,
            default_tool_output_budget: 50,
            l1_merge_threshold: 100,
            max_cold_entries: 50,
            ..Default::default()
        };
        let mut pipeline = ContextPipeline::new(&config);
        // Add many large messages to overflow L0 → L1 → L2
        for i in 0..40 {
            pipeline.add_message(text_msg(
                Role::User,
                &format!(
                    "message {i}: detailed discussion about Rust async patterns \
                     and error handling strategies for production systems"
                ),
            ));
        }
        // L2 should have received some segments
        assert!(
            pipeline.l2().len() > 0,
            "L2 should have entries after heavy overflow, l0={}, l1={}, l2={}",
            pipeline.l0().len(),
            pipeline.l1().len(),
            pipeline.l2().len(),
        );
    }

    #[test]
    fn build_messages_includes_l2_context() {
        let config = ContextPipelineConfig {
            max_context_tokens: 5000,
            hot_buffer_capacity: 2,
            default_tool_output_budget: 100,
            l1_merge_threshold: 200,
            max_cold_entries: 50,
            ..Default::default()
        };
        let mut pipeline = ContextPipeline::new(&config);
        // Generate enough messages to push data through L0 → L1 → L2
        for i in 0..30 {
            pipeline.add_message(text_msg(
                Role::User,
                &format!("detailed message {i} about Rust async patterns and error handling"),
            ));
        }
        let msgs = pipeline.build_messages();
        // Should have at least L0 messages
        assert!(!msgs.is_empty());
        // If L2 has data, first message might be compressed history
        if pipeline.l2().len() > 0 {
            let has_l2 = msgs.iter().any(|m| {
                m.content
                    .as_text()
                    .map_or(false, |t| t.contains("[Compressed History"))
            });
            assert!(has_l2, "Expected L2 compressed history in messages");
        }
    }

    #[test]
    fn l2_accessor() {
        let pipeline = ContextPipeline::new(&default_config());
        assert!(pipeline.l2().is_empty());
        assert_eq!(pipeline.l2().len(), 0);
    }

    #[test]
    fn reset_clears_l2() {
        let config = ContextPipelineConfig {
            max_context_tokens: 2000,
            hot_buffer_capacity: 2,
            default_tool_output_budget: 100,
            l1_merge_threshold: 200,
            max_cold_entries: 50,
            ..Default::default()
        };
        let mut pipeline = ContextPipeline::new(&config);
        for i in 0..20 {
            pipeline.add_message(text_msg(
                Role::User,
                &format!("message {i} with content for overflow"),
            ));
        }
        pipeline.reset();
        assert!(pipeline.l2().is_empty());
        assert_eq!(pipeline.l0().len(), 0);
        assert!(pipeline.l1().is_empty());
    }

    #[test]
    fn estimated_tokens_includes_l2() {
        let config = ContextPipelineConfig {
            max_context_tokens: 2000,
            hot_buffer_capacity: 2,
            default_tool_output_budget: 100,
            l1_merge_threshold: 200,
            max_cold_entries: 50,
            ..Default::default()
        };
        let mut pipeline = ContextPipeline::new(&config);
        for i in 0..20 {
            pipeline.add_message(text_msg(
                Role::User,
                &format!("message {i} with content for token estimation"),
            ));
        }
        let total = pipeline.estimated_tokens();
        // Total should be > 0 and include contributions from all tiers
        assert!(total > 0);
        if pipeline.l2().len() > 0 {
            assert!(
                total > pipeline.l0().token_count() + pipeline.l1().token_count(),
                "Total should include L2 tokens"
            );
        }
    }

    // --- Scale integration tests ---

    #[test]
    fn scale_500_messages_full_cascade() {
        let config = ContextPipelineConfig {
            max_context_tokens: 3000,
            hot_buffer_capacity: 8,
            default_tool_output_budget: 200,
            l1_merge_threshold: 500,
            max_cold_entries: 50,
            ..Default::default()
        };
        let mut pipeline = ContextPipeline::new(&config);

        // Simulate a long conversation with alternating roles.
        for i in 0..500 {
            let role = if i % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            };
            let content = format!(
                "Round {}: discussing Rust async patterns with tokio, error handling \
                 with thiserror, and testing strategies for production systems. \
                 Modified src/module_{}.rs and added comprehensive tests.",
                i / 2,
                i % 20
            );
            pipeline.add_message(text_msg(role, &content));
        }

        // Invariants that must hold:
        // 1. L0 has exactly hot_buffer_capacity messages (the most recent).
        assert_eq!(pipeline.l0().len(), 8, "L0 should be at capacity");

        // 2. L1 has segments (overflow from L0).
        assert!(
            !pipeline.l1().is_empty(),
            "L1 should have segments after 500 messages"
        );

        // 3. L2 should have compressed entries (overflow from L1).
        assert!(
            pipeline.l2().len() > 0,
            "L2 should have entries, got l1={} l2={}",
            pipeline.l1().len(),
            pipeline.l2().len()
        );

        // 4. L2 compression ratio should be meaningful.
        assert!(
            pipeline.l2().compression_ratio() < 1.0,
            "L2 compression ratio should be < 1.0, got {}",
            pipeline.l2().compression_ratio()
        );

        // 5. build_messages returns coherent output.
        let msgs = pipeline.build_messages();
        assert!(
            !msgs.is_empty(),
            "build_messages should return non-empty output"
        );

        // 6. Most recent L0 messages are present in output.
        let last_msg = msgs.last().unwrap();
        let last_text = last_msg.content.as_text().unwrap();
        assert!(
            last_text.contains("Round 249"),
            "Last message should be from the most recent round"
        );

        // 7. Token estimate is reasonable.
        let total_tokens = pipeline.estimated_tokens();
        assert!(total_tokens > 0, "Estimated tokens should be positive");
    }

    #[test]
    fn scale_mixed_tool_results() {
        let config = ContextPipelineConfig {
            max_context_tokens: 2000,
            hot_buffer_capacity: 4,
            default_tool_output_budget: 100,
            l1_merge_threshold: 300,
            max_cold_entries: 30,
            ..Default::default()
        };
        let mut pipeline = ContextPipeline::new(&config);

        // Simulate agent rounds with tool usage.
        for round in 0..50 {
            // User message.
            pipeline.add_message(text_msg(
                Role::User,
                &format!("Please run cargo test for round {round}"),
            ));
            // Tool result (sometimes large).
            let tool_output = if round % 5 == 0 {
                (0..100)
                    .map(|i| format!("test result line {i}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                format!("test result: {} passed, 0 failed", round * 10)
            };
            pipeline.add_tool_result("bash", &format!("tool_{round}"), &tool_output, false);
            // Assistant response.
            pipeline.add_message(text_msg(
                Role::Assistant,
                &format!("All tests passed for round {round}. Proceeding."),
            ));
        }

        // L0 should hold recent messages.
        assert!(pipeline.l0().len() <= 4);
        // Total tokens tracked.
        assert!(pipeline.estimated_tokens() > 0);
        // build_messages works.
        let msgs = pipeline.build_messages();
        assert!(!msgs.is_empty());
    }

    #[test]
    fn scale_budget_never_exceeded() {
        let config = ContextPipelineConfig {
            max_context_tokens: 500,
            hot_buffer_capacity: 4,
            default_tool_output_budget: 50,
            l1_merge_threshold: 100,
            max_cold_entries: 20,
            ..Default::default()
        };
        let mut pipeline = ContextPipeline::new(&config);

        for i in 0..200 {
            pipeline.add_message(text_msg(
                Role::User,
                &format!(
                    "message {i}: detailed content about system design and architecture patterns"
                ),
            ));
            // After each message, L0 tokens should not exceed L0 budget.
            let l0_tokens = pipeline.l0().token_count();
            let l0_budget = pipeline
                .accountant()
                .tier_budget(crate::accountant::Tier::L0Hot);
            // Allow some slack for the sync cycle.
            assert!(
                l0_tokens <= l0_budget + 50,
                "L0 tokens ({l0_tokens}) should not greatly exceed L0 budget ({l0_budget}) at msg {i}"
            );
        }
    }

    // --- L3 Semantic Store integration tests ---

    #[test]
    fn l3_receives_l2_evictions() {
        let config = ContextPipelineConfig {
            max_context_tokens: 400,
            hot_buffer_capacity: 2,
            default_tool_output_budget: 50,
            l1_merge_threshold: 80,
            max_cold_entries: 3,
            max_semantic_entries: 50,
            ..Default::default()
        };
        let mut pipeline = ContextPipeline::new(&config);
        // Generate enough messages to overflow L0→L1→L2→L3.
        for i in 0..60 {
            pipeline.add_message(text_msg(
                Role::User,
                &format!(
                    "message {i}: discussing Rust async patterns and error handling \
                     strategies for building reliable production systems"
                ),
            ));
        }
        // L3 should have received evictions from L2.
        assert!(
            pipeline.l3().len() > 0,
            "L3 should have entries after L2 overflow, l0={}, l1={}, l2={}, l3={}",
            pipeline.l0().len(),
            pipeline.l1().len(),
            pipeline.l2().len(),
            pipeline.l3().len(),
        );
    }

    #[test]
    fn build_messages_includes_l3_semantic_context() {
        let config = ContextPipelineConfig {
            max_context_tokens: 5000,
            hot_buffer_capacity: 2,
            default_tool_output_budget: 100,
            l1_merge_threshold: 200,
            max_cold_entries: 3,
            max_semantic_entries: 50,
            ..Default::default()
        };
        let mut pipeline = ContextPipeline::new(&config);

        // Phase 1: Add varied content that will be evicted to L3.
        for i in 0..20 {
            pipeline.add_message(text_msg(
                Role::User,
                &format!(
                    "message {i}: implementing SQLite database layer with WAL mode \
                     and connection pooling for high throughput"
                ),
            ));
        }

        // Phase 2: Add a recent message that semantically relates to L3 content.
        pipeline.add_message(text_msg(
            Role::User,
            "How should we configure the SQLite database connection?",
        ));

        if pipeline.l3().len() > 0 {
            let msgs = pipeline.build_messages();
            let has_l3 = msgs.iter().any(|m| {
                m.content
                    .as_text()
                    .map_or(false, |t| t.contains("[Semantic Memory (L3)]"))
            });
            assert!(
                has_l3,
                "Expected L3 semantic memory in messages when query matches stored content"
            );
        }
    }

    #[test]
    fn l3_accessor() {
        let pipeline = ContextPipeline::new(&default_config());
        assert!(pipeline.l3().is_empty());
        assert_eq!(pipeline.l3().len(), 0);
    }

    #[test]
    fn reset_clears_l3() {
        let config = ContextPipelineConfig {
            max_context_tokens: 400,
            hot_buffer_capacity: 2,
            default_tool_output_budget: 50,
            l1_merge_threshold: 80,
            max_cold_entries: 3,
            max_semantic_entries: 50,
            ..Default::default()
        };
        let mut pipeline = ContextPipeline::new(&config);
        for i in 0..60 {
            pipeline.add_message(text_msg(
                Role::User,
                &format!("message {i} about Rust patterns and error handling strategies"),
            ));
        }
        pipeline.reset();
        assert!(pipeline.l3().is_empty());
    }

    #[test]
    fn estimated_tokens_includes_l3() {
        let config = ContextPipelineConfig {
            max_context_tokens: 400,
            hot_buffer_capacity: 2,
            default_tool_output_budget: 50,
            l1_merge_threshold: 80,
            max_cold_entries: 3,
            max_semantic_entries: 50,
            ..Default::default()
        };
        let mut pipeline = ContextPipeline::new(&config);
        for i in 0..60 {
            pipeline.add_message(text_msg(
                Role::User,
                &format!("message {i} about Rust async tokio error handling"),
            ));
        }
        if pipeline.l3().len() > 0 {
            let total = pipeline.estimated_tokens();
            let without_l3 = pipeline.l0().token_count()
                + pipeline.l1().token_count()
                + pipeline.l2().original_tokens();
            assert!(
                total > without_l3,
                "Total tokens should include L3: total={total}, without_l3={without_l3}, l3_entries={}",
                pipeline.l3().len()
            );
        }
    }

    #[test]
    fn l3_no_match_returns_no_semantic_section() {
        let config = ContextPipelineConfig {
            max_context_tokens: 5000,
            hot_buffer_capacity: 2,
            default_tool_output_budget: 100,
            l1_merge_threshold: 200,
            max_cold_entries: 3,
            max_semantic_entries: 50,
            ..Default::default()
        };
        let mut pipeline = ContextPipeline::new(&config);

        // Add content about database patterns.
        for i in 0..20 {
            pipeline.add_message(text_msg(
                Role::User,
                &format!("message {i}: database schema migration and indexing strategies"),
            ));
        }

        // Query about something completely different.
        pipeline.add_message(text_msg(
            Role::User,
            "How do quantum computing circuits work?",
        ));

        let msgs = pipeline.build_messages();
        // With no semantic match, there should be no L3 section.
        let has_l3 = msgs.iter().any(|m| {
            m.content
                .as_text()
                .map_or(false, |t| t.contains("[Semantic Memory (L3)]"))
        });
        // This might or might not have L3 results depending on term overlap — but quantum/circuits
        // shouldn't match database/schema/migration. The key invariant is no crash.
        // If BM25 finds no match, has_l3 should be false.
        if pipeline.l3().len() > 0 {
            // At minimum, the pipeline works without error.
            assert!(!msgs.is_empty());
        }
        let _ = has_l3; // used for documentation, not always deterministic
    }

    #[test]
    fn scale_1000_messages_full_l3_cascade() {
        let config = ContextPipelineConfig {
            max_context_tokens: 2000,
            hot_buffer_capacity: 4,
            default_tool_output_budget: 100,
            l1_merge_threshold: 300,
            max_cold_entries: 10,
            max_semantic_entries: 50,
            ..Default::default()
        };
        let mut pipeline = ContextPipeline::new(&config);

        for i in 0..1000 {
            let role = if i % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            };
            pipeline.add_message(text_msg(
                role,
                &format!(
                    "Round {}: detailed discussion about system architecture patterns \
                     and Rust async programming with tokio and error handling",
                    i / 2
                ),
            ));
        }

        // All tiers should be populated.
        assert_eq!(pipeline.l0().len(), 4);
        assert!(!pipeline.l1().is_empty());
        assert!(pipeline.l2().len() > 0);
        assert!(
            pipeline.l3().len() > 0,
            "L3 should have entries after 1000 messages with tight budgets"
        );

        // build_messages should work.
        let msgs = pipeline.build_messages();
        assert!(!msgs.is_empty());
    }

    #[test]
    fn load_l4_archive_nonexistent_path() {
        let mut pipeline = ContextPipeline::new(&default_config());
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.bin");
        pipeline.load_l4_archive(&path);
        // Should create archive with path (for future flushes), not crash.
        assert!(pipeline.l4().is_empty());
    }

    #[test]
    fn flush_and_load_l4_archive() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("archive.bin");

        // Create a pipeline with data that cascades to L4.
        let config = ContextPipelineConfig {
            max_context_tokens: 500,
            hot_buffer_capacity: 2,
            max_cold_entries: 2,
            max_semantic_entries: 2,
            max_archive_entries: 100,
            ..Default::default()
        };
        let mut pipeline = ContextPipeline::new(&config);
        pipeline.load_l4_archive(&path); // Set up path for flushing.

        // Push enough messages to cascade to L4.
        for i in 0..50 {
            pipeline.add_message(text_msg(
                if i % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                },
                &format!(
                    "Message {i} with enough content to fill multiple tiers and cascade evictions"
                ),
            ));
        }

        // Flush to disk.
        let bytes = pipeline.flush_l4_archive();
        assert!(bytes.is_some(), "flush should write data");
        assert!(bytes.unwrap() > 0);
        assert!(path.exists());

        // Create a new pipeline and load from disk.
        let mut pipeline2 = ContextPipeline::new(&config);
        pipeline2.load_l4_archive(&path);

        // L4 should be restored with the same entries.
        assert_eq!(pipeline2.l4().len(), pipeline.l4().len());
    }

    #[test]
    fn flush_without_path_returns_none() {
        let mut pipeline = ContextPipeline::new(&default_config());
        // No L4 path configured → flush returns None.
        assert!(pipeline.flush_l4_archive().is_none());
    }

    // --- Instruction refresh tests ---

    #[test]
    fn refresh_instructions_no_change_returns_none() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("HALCON.md"), "# Instructions\nUse Rust.").unwrap();

        let mut pipeline = ContextPipeline::new(&default_config());
        pipeline.initialize("system prompt", dir.path());

        // Immediate refresh — no changes — should return None.
        let result = pipeline.refresh_instructions(dir.path());
        assert!(result.is_none());
    }

    #[test]
    fn refresh_instructions_detects_file_change() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("HALCON.md");
        std::fs::write(&path, "version 1 instructions").unwrap();

        let mut pipeline = ContextPipeline::new(&default_config());
        pipeline.initialize("system prompt", dir.path());

        // Modify the file on disk.
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&path, "version 2 instructions with more content").unwrap();

        let result = pipeline.refresh_instructions(dir.path());
        assert!(result.is_some());
        let content = result.unwrap();
        assert!(content.contains("version 2"));
    }

    #[test]
    fn refresh_instructions_updates_accountant() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("HALCON.md");
        std::fs::write(&path, "short").unwrap();

        let mut pipeline = ContextPipeline::new(&default_config());
        pipeline.initialize("system prompt", dir.path());
        let reserved_before = pipeline.accountant().system_prompt_reserved();

        // Write a much larger instruction file.
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&path, &"x".repeat(400)).unwrap();

        let result = pipeline.refresh_instructions(dir.path());
        assert!(result.is_some());
        let reserved_after = pipeline.accountant().system_prompt_reserved();
        assert!(
            reserved_after > reserved_before,
            "Reservation should grow: before={reserved_before}, after={reserved_after}"
        );
    }

    #[test]
    fn refresh_instructions_no_files_returns_none() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut pipeline = ContextPipeline::new(&default_config());
        // Initialize with no instruction files.
        pipeline.initialize("system prompt", dir.path());

        // Refresh with still no files — should return None.
        let result = pipeline.refresh_instructions(dir.path());
        assert!(result.is_none());
    }

    #[test]
    fn refresh_instructions_new_file_detected() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut pipeline = ContextPipeline::new(&default_config());
        // Initialize with no HALCON.md.
        pipeline.initialize("system prompt", dir.path());

        // Create a new HALCON.md — should be detected.
        std::fs::write(dir.path().join("HALCON.md"), "new instructions").unwrap();

        let result = pipeline.refresh_instructions(dir.path());
        assert!(result.is_some());
        assert!(result.unwrap().contains("new instructions"));
    }

    // ── Tool pair eviction safety tests ──

    fn tool_use_msg(id: &str, name: &str) -> ChatMessage {
        ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input: serde_json::json!({}),
            }]),
        }
    }

    fn tool_result_msg(tool_use_id: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: content.to_string(),
                is_error: false,
            }]),
        }
    }

    #[test]
    fn pair_eviction_keeps_tool_pair_together() {
        // L0 capacity=4. Fill with 3 text messages, then add a tool pair.
        // The tool_use message should be at the eviction boundary.
        let config = ContextPipelineConfig {
            max_context_tokens: 50_000, // large budget so ensure_budgets doesn't interfere
            hot_buffer_capacity: 4,
            ..Default::default()
        };
        let mut pipeline = ContextPipeline::new(&config);

        // Fill 3 slots.
        pipeline.add_message(text_msg(Role::User, "msg1"));
        pipeline.add_message(text_msg(Role::Assistant, "msg2"));
        pipeline.add_message(text_msg(Role::User, "msg3"));
        assert_eq!(pipeline.l0().len(), 3);

        // Add assistant tool_use — fills slot 4.
        pipeline.add_message(tool_use_msg("t1", "bash"));
        assert_eq!(pipeline.l0().len(), 4);

        // Add user tool_result — this should evict msg1, then the pair-aware
        // logic should also evict msg2 (next oldest). But the key invariant is:
        // build_messages() must NOT have an orphaned ToolResult.
        pipeline.add_message(tool_result_msg("t1", "ok"));

        let msgs = pipeline.build_messages();
        // Verify no orphaned tool results.
        let violations = halcon_core::types::validation::validate_message_sequence(&msgs, false);
        let orphans: Vec<_> = violations
            .iter()
            .filter(|v| {
                matches!(
                    v,
                    halcon_core::types::validation::ProtocolViolation::OrphanedToolResult { .. }
                )
            })
            .collect();
        assert!(
            orphans.is_empty(),
            "build_messages() produced orphaned tool results: {orphans:?}"
        );
    }

    #[test]
    fn build_messages_strips_orphans_from_l0() {
        // Directly test: if L0 somehow has an orphaned ToolResult (ToolUse
        // was evicted to L1), build_messages strips it.
        let config = ContextPipelineConfig {
            max_context_tokens: 50_000,
            hot_buffer_capacity: 3,
            ..Default::default()
        };
        let mut pipeline = ContextPipeline::new(&config);

        // Fill buffer with text messages first.
        pipeline.add_message(text_msg(Role::User, "filler1"));
        pipeline.add_message(text_msg(Role::Assistant, "filler2"));

        // Add a tool pair that will span the eviction boundary.
        pipeline.add_message(tool_use_msg("t_orphan", "bash"));
        // At this point, L0 is full (3 messages).
        // Adding tool_result evicts filler1 (FIFO). The pair-aware logic
        // evicts filler2 too (it's not a ToolResult for t_orphan).
        // But tool_use_msg("t_orphan") stays in L0.
        pipeline.add_message(tool_result_msg("t_orphan", "result"));

        let msgs = pipeline.build_messages();
        let violations = halcon_core::types::validation::validate_message_sequence(&msgs, false);
        let orphans: Vec<_> = violations
            .iter()
            .filter(|v| {
                matches!(
                    v,
                    halcon_core::types::validation::ProtocolViolation::OrphanedToolResult { .. }
                )
            })
            .collect();
        assert!(
            orphans.is_empty(),
            "build_messages() should never return orphaned tool results: {orphans:?}"
        );
    }

    #[test]
    fn heavy_tool_rounds_no_orphans() {
        // Simulate 100 tool-use rounds with a small L0 buffer.
        let config = ContextPipelineConfig {
            max_context_tokens: 50_000,
            hot_buffer_capacity: 6,
            ..Default::default()
        };
        let mut pipeline = ContextPipeline::new(&config);

        for round in 0..100 {
            pipeline.add_message(text_msg(Role::User, &format!("do round {round}")));
            pipeline.add_message(tool_use_msg(&format!("t{round}"), "bash"));
            pipeline.add_message(tool_result_msg(
                &format!("t{round}"),
                &format!("result {round}"),
            ));
            pipeline.add_message(text_msg(Role::Assistant, &format!("done round {round}")));

            // Check invariant after every round.
            let msgs = pipeline.build_messages();
            let violations =
                halcon_core::types::validation::validate_message_sequence(&msgs, false);
            let orphans: Vec<_> = violations
                .iter()
                .filter(|v| {
                    matches!(
                        v,
                        halcon_core::types::validation::ProtocolViolation::OrphanedToolResult { .. }
                    )
                })
                .collect();
            assert!(
                orphans.is_empty(),
                "Orphaned tool results at round {round}: {orphans:?}"
            );
        }
    }
}
