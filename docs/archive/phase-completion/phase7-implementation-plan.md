# Phase 7: Production-Grade Extension — Implementation Plan

**Date**: February 7, 2026
**Baseline**: 553 tests, 5.0MB binary, 23,134 LOC, 9 crates
**Execution**: 4 sub-phases, each compiles + tests independently

---

## Execution Order & Rationale

```
7.1 Safety & Security Hardening    (P0, independent)
7.2 Context Compaction + Cost Guard (P0, builds on agent loop)
7.3 Enhanced PII + Observability    (P1, independent)
7.4 Test Coverage Expansion         (P1, last — validates everything)
```

---

## Sub-Phase 7.1: Safety & Security Hardening

### Problem
- `mcp/host.rs:94`: `server_info.as_ref().unwrap()` — production panic risk
- PII detector misses SSH private keys, JWT tokens, PEM content, GitHub tokens
- No output sanitization before feeding tool results back to model
- AgentLimits.max_total_tokens defaults to 0 (unlimited) — unbounded API spend risk

### Changes

**1. Fix mcp/host.rs unwrap** — `crates/cuervo-mcp/src/host.rs:94`
```rust
// Before:
Ok(self.server_info.as_ref().unwrap())
// After:
Ok(self.server_info.as_ref().ok_or(McpError::NotInitialized)?)
```
One-line fix. Safe because `initialize()` sets `self.server_info = Some(result)` on line 82 before reaching line 94, but the defensive check prevents panics if code paths change.

**2. Enhanced PII patterns** — `crates/cuervo-security/src/pii.rs`

Add 5 new patterns to `default_patterns()`:
```rust
// SSH private key header
(r"-----BEGIN (?:RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----", "ssh_private_key"),
// JWT token (3 base64 segments)
(r"eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}", "jwt_token"),
// GitHub personal access token (new format)
(r"ghp_[A-Za-z0-9]{36}", "github_token"),
// GitHub fine-grained PAT
(r"github_pat_[A-Za-z0-9]{22}_[A-Za-z0-9]{59}", "github_fine_grained_token"),
// Anthropic API key
(r"sk-ant-api\d{2}-[A-Za-z0-9_-]{80,}", "anthropic_api_key"),
```

Update existing tests + add 5 new tests (one per pattern).

**3. Tool output sanitization** — `crates/cuervo-cli/src/repl/executor.rs`

Add `sanitize_tool_output()` function called after tool execution, before feeding results back to the model:
```rust
fn sanitize_tool_output(output: &str, pii_detector: &PiiDetector) -> String {
    if pii_detector.contains_pii(output) {
        let detected = pii_detector.detect(output);
        tracing::warn!(types = ?detected, "PII detected in tool output — sanitizing");
        // Redact the specific PII matches (replace with [REDACTED:<type>])
        // Use the regex patterns from PiiDetector to locate and replace matches
    }
    output.to_string()
}
```
- Wire into `execute_parallel_batch()` and `execute_sequential_tool()` result paths
- Controlled by `config.security.pii_detection` and `config.security.pii_action`
- Action "redact" → replace matches; "warn" → log + pass through; "block" → error

**4. Config validation for cost limits** — `crates/cuervo-core/src/types/config.rs`

Add warning to `validate_config()`:
```rust
// Warn if no token budget AND no duration budget — unbounded spend risk
if config.agent.limits.max_total_tokens == 0 && config.agent.limits.max_duration_secs == 0 {
    issues.push(ConfigIssue {
        level: IssueLevel::Warning,
        field: "agent.limits".into(),
        message: "no token or duration budget set — API spend is unbounded".into(),
        suggestion: Some("Set max_total_tokens or max_duration_secs for cost control".into()),
    });
}
```

### Tests (10 new)
- `mcp_host_initialize_returns_result_not_panic` — verify no unwrap
- `detect_ssh_private_key` — SSH key header detected
- `detect_jwt_token` — JWT 3-part token detected
- `detect_github_pat` — GitHub PAT detected
- `detect_github_fine_grained_pat` — Fine-grained PAT detected
- `detect_anthropic_key` — Anthropic API key detected
- `no_false_positive_on_base64_code` — Base64 strings in code not flagged as JWT
- `sanitize_tool_output_redacts_pii` — Tool output with PII gets redacted
- `sanitize_tool_output_passes_clean` — Clean output passes through
- `validate_warns_unbounded_budget` — No budget → warning

### Files Modified
- `crates/cuervo-mcp/src/host.rs` (1 line — unwrap → ok_or)
- `crates/cuervo-security/src/pii.rs` (5 new patterns + 5 tests)
- `crates/cuervo-cli/src/repl/executor.rs` (sanitize_tool_output + wiring)
- `crates/cuervo-core/src/types/config.rs` (1 new validation rule)

---

## Sub-Phase 7.2: Context Compaction + Cost Guard

### Problem
- Long sessions exceed model context window — messages grow unbounded
- No summarization when context is large — just fails or truncates
- Token budget is enforced but not visible to the user until exceeded
- No per-message cost estimate printed

### Changes

**1. Context compaction module** — `crates/cuervo-cli/src/repl/compaction.rs` (NEW)

Implements rolling summarization when messages exceed a configurable token threshold:

```rust
pub struct ContextCompactor {
    /// Threshold as fraction of max context (e.g., 0.80 = 80%)
    threshold_fraction: f32,
    /// Maximum context tokens for the current model
    max_context_tokens: u32,
}

impl ContextCompactor {
    /// Check if compaction is needed based on current token usage
    pub fn needs_compaction(&self, session: &Session) -> bool {
        let current = estimate_message_tokens(&session.messages);
        let threshold = (self.max_context_tokens as f32 * self.threshold_fraction) as u32;
        current >= threshold
    }

    /// Generate a compaction prompt for the model to summarize context
    pub fn compaction_prompt(&self, messages: &[ChatMessage]) -> String {
        // Generates a system prompt asking the model to produce a structured
        // summary preserving: key decisions, file modifications, pending tasks,
        // and critical context. Output wrapped in <summary> tags.
    }

    /// Replace old messages with compacted summary, preserving most recent N messages
    pub fn apply_compaction(
        &self,
        messages: &mut Vec<ChatMessage>,
        summary: &str,
        keep_recent: usize,
    ) {
        // Keep first message (system), last `keep_recent` messages
        // Replace middle with a single summary message
    }
}
```

**2. Wire compaction into agent loop** — `crates/cuervo-cli/src/repl/agent.rs`

At the START of each round (before building `round_request`), check if compaction is needed:

```rust
// Context compaction check (before building request)
if let Some(compactor) = compactor {
    if compactor.needs_compaction(session) {
        tracing::info!("Context compaction triggered");
        eprintln!("\n[compacting context...]");
        // Build compaction request using the provider
        let summary = run_compaction(provider, &messages, compactor, request).await?;
        compactor.apply_compaction(&mut messages, &summary, 4);
        // Update session messages to match
        session.messages = messages.clone();
    }
}
```

**3. Cost display per round** — `crates/cuervo-cli/src/repl/agent.rs`

After each round, print a subtle cost indicator:
```rust
// After line 432 (session.estimated_cost_usd += round_cost...)
if round_cost.estimated_cost_usd > 0.0 {
    tracing::debug!(
        cost = format!("${:.4}", round_cost.estimated_cost_usd),
        cumulative = format!("${:.4}", session.estimated_cost_usd),
        "Round cost"
    );
}
```

**4. Compaction config** — `crates/cuervo-core/src/types/config.rs`

Add to `AgentConfig`:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    /// Enable automatic context compaction
    pub enabled: bool,
    /// Trigger compaction at this fraction of max context (0.0–1.0)
    pub threshold_fraction: f32,
    /// Number of recent messages to always preserve during compaction
    pub keep_recent: usize,
    /// Max context window tokens (0 = auto-detect from model)
    pub max_context_tokens: u32,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold_fraction: 0.80,
            keep_recent: 4,
            max_context_tokens: 200_000, // Claude default
        }
    }
}
```

Add `compaction: CompactionConfig` to `AgentConfig` with `#[serde(default)]`.

### Tests (8 new)
- `needs_compaction_below_threshold` — returns false when under 80%
- `needs_compaction_above_threshold` — returns true when over 80%
- `compaction_prompt_includes_instructions` — prompt asks for summary
- `apply_compaction_preserves_recent` — last N messages kept
- `apply_compaction_inserts_summary` — summary replaces old messages
- `compaction_config_defaults` — verify default values
- `validate_warns_compaction_high_threshold` — threshold > 0.95 → warning
- `estimate_message_tokens_basic` — token estimation sanity check

### Files Modified/Created
- `crates/cuervo-cli/src/repl/compaction.rs` (NEW — compaction module)
- `crates/cuervo-cli/src/repl/mod.rs` (add `pub mod compaction;`)
- `crates/cuervo-cli/src/repl/agent.rs` (wire compaction + cost display)
- `crates/cuervo-core/src/types/config.rs` (CompactionConfig + validation)

---

## Sub-Phase 7.3: Enhanced PII Redaction + Observability Foundation

### Problem
- PII `detect()` returns type names but can't locate/replace matches in text
- No structured tracing spans for agent loop (only trace_steps in DB)
- No way to export traces to external systems (OpenTelemetry)
- Duplicate health scoring code (doctor assess_sync vs HealthScorer::assess)

### Changes

**1. PII redaction capability** — `crates/cuervo-security/src/pii.rs`

Add individual `Regex` patterns alongside `RegexSet` for redaction:
```rust
pub struct PiiDetector {
    patterns: RegexSet,
    pattern_names: Vec<String>,
    individual_patterns: Vec<regex::Regex>,  // NEW — for find+replace
}

impl PiiDetector {
    /// Redact all PII in text, replacing matches with [REDACTED:<type>]
    pub fn redact(&self, text: &str) -> String {
        let mut result = text.to_string();
        let matches: Vec<usize> = self.patterns.matches(text).into_iter().collect();
        // Apply replacements from individual regexes (reverse order to preserve indices)
        for idx in matches.into_iter().rev() {
            let re = &self.individual_patterns[idx];
            let name = &self.pattern_names[idx];
            result = re.replace_all(&result, format!("[REDACTED:{name}]")).to_string();
        }
        result
    }
}
```

**2. Structured tracing spans** — `crates/cuervo-cli/src/repl/agent.rs`

Add semantic spans following OpenTelemetry GenAI conventions:
```rust
// Per-round span with GenAI attributes
let round_span = tracing::info_span!("gen_ai.agent.round",
    "gen_ai.request.model" = %request.model,
    "gen_ai.operation.name" = "agent_round",
    round = round,
);
let _guard = round_span.enter();
```

After streaming completes, record usage attributes:
```rust
tracing::Span::current().record("gen_ai.usage.input_tokens", round_usage.input_tokens);
tracing::Span::current().record("gen_ai.usage.output_tokens", round_usage.output_tokens);
```

**3. Remove duplicate health scoring** — `crates/cuervo-cli/src/commands/doctor.rs`

Replace `assess_sync()` standalone function with direct call to `HealthScorer::assess()` using a blocking context (the doctor command is sync):
```rust
// Use spawn_blocking to call async health scorer from sync context
let health = tokio::task::block_in_place(|| {
    tokio::runtime::Handle::current().block_on(async {
        scorer.assess(provider_name, &db).await
    })
});
```
Wait — doctor runs in sync context before tokio runtime. Keep assess_sync but extract shared formula into a pure function:
```rust
// In health.rs:
pub fn compute_score(reliability: f32, latency: f32, availability: f32, consistency: f32) -> u32
```
Both `assess()` and `assess_sync()` call `compute_score()`.

### Tests (6 new)
- `redact_replaces_email` — email → [REDACTED:email]
- `redact_replaces_ssh_key` — SSH header → [REDACTED:ssh_private_key]
- `redact_preserves_clean_text` — no PII → unchanged
- `redact_multiple_types` — multiple PII types in one string
- `compute_score_shared_formula` — same inputs give same score in both paths
- `gen_ai_span_attributes` — verify span names follow convention

### Files Modified
- `crates/cuervo-security/src/pii.rs` (add individual_patterns + redact method)
- `crates/cuervo-cli/src/repl/agent.rs` (GenAI span attributes)
- `crates/cuervo-cli/src/repl/health.rs` (extract compute_score pure fn)
- `crates/cuervo-cli/src/commands/doctor.rs` (use compute_score)

---

## Sub-Phase 7.4: Test Coverage Expansion

### Problem
- 35/97 files have zero test coverage (36%)
- Critical gaps in: registries, commands, rendering, context sources
- No edge case tests for path traversal attacks
- No fuzz-like boundary tests for SSE/NDJSON parsing

### Changes

**1. Registry tests** — `crates/cuervo-providers/src/registry.rs`
- `register_and_retrieve` — register provider, get by name
- `get_unknown_returns_none` — missing provider → None
- `list_returns_all` — list() shows all registered
- `default_provider_from_config` — build_registry uses config defaults

**2. Path security edge cases** — `crates/cuervo-tools/src/path_security.rs`
- `symlink_traversal_blocked` — /project/link -> /etc, access via link
- `double_dot_repeated` — `../../../../etc/passwd` deeply nested
- `null_byte_in_path` — path with \0 byte rejected
- `unicode_normalization` — different unicode forms for same path
- `empty_path_rejected` — empty string path

**3. Context source tests** — `crates/cuervo-context/src/instruction_source.rs`
- `missing_file_returns_empty` — no CUERVO.md → empty context
- `file_with_content` — CUERVO.md exists → returns content
- `respects_token_budget` — large file truncated to budget
- `multiple_instruction_files` — project + global merged

**4. Rendering tests** — `crates/cuervo-cli/src/render/`
- `markdown_renders_headings` — # heading → formatted
- `markdown_renders_code_blocks` — ```rust``` → highlighted
- `syntax_highlight_rust` — Rust code gets colors
- `stream_renderer_accumulates_text` — StreamRenderer.full_text()

**5. Command handler tests** — `crates/cuervo-cli/src/commands/`
- `doctor_runs_without_db` — doctor command handles no DB
- `status_shows_config` — status prints config info
- These test the handler functions directly (not CLI parsing)

### Tests (20+ new)
Target: bring coverage from 64% to 75%+ of files.

### Files Modified
- `crates/cuervo-providers/src/registry.rs` (add #[cfg(test)] module)
- `crates/cuervo-tools/src/path_security.rs` (add edge case tests)
- `crates/cuervo-context/src/instruction_source.rs` (add tests)
- `crates/cuervo-cli/src/render/markdown.rs` (add tests)
- `crates/cuervo-cli/src/render/syntax.rs` (add tests)
- `crates/cuervo-cli/src/commands/doctor.rs` (add tests)

---

## Quality Gates (per sub-phase)

- `cargo test --workspace` passes
- `cargo clippy --workspace -- -D warnings` clean
- Zero new `unwrap()` in production code
- All new code async-safe
- Backward compatible (serde(default), no breaking config changes)

## Final Deliverables

After all 4 sub-phases:
1. `cargo test --workspace` — target: **600+ tests** (from 553)
2. `cargo clippy --workspace` — zero warnings
3. `cargo build --release` — binary stays ~5.0MB
4. Manual test: PII patterns catch SSH keys, JWTs, GitHub tokens
5. Manual test: long conversation triggers compaction gracefully
6. Manual test: `cuervo doctor` shows consistent health scores

## Estimated Impact

| Metric | Before | After |
|--------|--------|-------|
| Tests | 553 | ~600+ |
| Production unwraps | 1 | 0 |
| PII patterns | 7 | 12 |
| File test coverage | 64% | ~75% |
| OWASP LLM gaps | 4 | 1 (prompt injection remains partial) |
| Binary size | 5.0MB | ~5.0MB (no new heavy deps) |
