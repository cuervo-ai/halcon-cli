# Cuervo CLI — Provider Integration Test Report

**Date**: 2026-02-08T10:00:00Z
**Build**: release (5.3MB)
**Tests**: 1116 unit tests passing, 28 routing tests passing
**Prompt**: "Responde en una sola linea: Cual es la capital de Francia?" / "What is 15 * 37?"

---

## Phase 0: Critical Bug Fixes Applied

### Bug 1: Model Selector Provider Mismatch (FIXED)
- **Symptom**: When `model_selection.enabled = true`, the selector picked a model from another provider (e.g., `gemini-2.0-flash`) but the request was still sent to the original provider (deepseek), causing "Model Not Exist" errors.
- **Root Cause**: `agent.rs:520-536` only extracted `selection.model_id` but never switched the `provider` object.
- **Fix**: Added `registry: Option<&ProviderRegistry>` to `AgentContext`. When model selection picks a model from a different provider, the agent loop now looks up and switches to the correct provider via the registry.
- **Files Modified**: `agent.rs`, `mod.rs`, `orchestrator.rs`, `replay_runner.rs`

### Bug 2: Fallback Provider Model Mismatch (FIXED)
- **Symptom**: When fallback triggered, all fallback providers received the same model name (e.g., `gemini-2.0-flash`) regardless of their supported models, causing cascading failures.
- **Root Cause**: `invoke_with_fallback()` and `router.rs` passed the request model directly to fallback providers without checking compatibility.
- **Fix**:
  - `agent.rs:invoke_with_fallback()`: Before each fallback attempt, checks if the provider supports the model. If not, uses the provider's first supported model.
  - `router.rs`: Same logic applied in the ModelRouter fallback chain.
- **Files Modified**: `agent.rs`, `router.rs`

### Verification
- **1116 unit tests**: ALL PASS
- **28 routing tests**: ALL PASS (router, speculative, model_selector)
- **Clippy**: Clean (0 cuervo warnings)

---

## Phase 1: Individual Model Tests

| # | Provider | Model | Status | Latency (ms) | Response |
|---|----------|-------|--------|-------------|----------|
| 1 | deepseek | deepseek-chat | OK | 7,877 | "Paris." |
| 2 | deepseek | deepseek-coder | OK | 8,351 | Python hello world (comprehensive) |
| 3 | deepseek | deepseek-reasoner | OK | 4,059 | "15 x 37 = 555." |
| 4 | openai | gpt-4o-mini | OK | 855 | "La capital de Francia es Paris." |
| 5 | openai | gpt-4o | OK | 1,311 | "La capital de Francia es Paris." |
| 6 | openai | o1 | OK | 1,411 | "15 multiplied by 37 equals 555." |
| 7 | openai | o3-mini | OK | 1,510 | "15 * 37 = 555" (with step-by-step) |
| 8 | gemini | gemini-2.0-flash | FAIL (429) | 28,645 | Rate limited (free tier quota) |
| 9 | anthropic | claude-sonnet-4-5 | FAIL (no credits) | 362 | Provider rejected |
| 10 | ollama | deepseek-coder-v2 | OK | 1,078 | "La capital de Francia es Paris." |

### Summary
- **Total models tested**: 10
- **Passed**: 8/10 (80%)
- **Failed**: 2/10 (account issues, not code bugs)
  - **Gemini**: Free tier rate limit (429) — need paid tier or wait
  - **Anthropic**: No API credits — need account top-up

---

## Phase 2: Provider Latency Rankings

| Rank | Provider/Model | Latency (ms) | Cost Estimate |
|------|---------------|-------------|---------------|
| 1 | openai/gpt-4o-mini | 855 | ~$0.0001 |
| 2 | ollama/deepseek-coder-v2 | 1,078 | $0.00 (local) |
| 3 | openai/gpt-4o | 1,311 | ~$0.001 |
| 4 | openai/o1 | 1,411 | ~$0.005 |
| 5 | openai/o3-mini | 1,510 | ~$0.002 |
| 6 | deepseek/deepseek-reasoner | 4,059 | ~$0.001 |
| 7 | deepseek/deepseek-chat | 7,877 | ~$0.0001 |
| 8 | deepseek/deepseek-coder | 8,351 | ~$0.0001 |

**Observations**:
- OpenAI models are consistently fastest (855-1510ms)
- Ollama local inference is competitive (1078ms) with zero cost
- DeepSeek models are slowest (4-8s) but cheapest for cloud
- Reasoning models (o1, o3-mini, deepseek-reasoner) correctly receive no temperature parameter

---

## Phase 3: Routing Infrastructure Tests

### Unit Test Results (28/28 pass)
```
router::primary_succeeds_no_fallback ............ OK
router::retries_before_giving_up ................ OK
router::default_routing_config .................. OK
router::fallback_models_tried_after_primary ..... OK
speculative::failover_mode_primary_succeeds ..... OK
speculative::failover_mode_is_default ........... OK
speculative::speculative_mode_with_single ....... OK
speculative::speculative_mode_races_providers ... OK
speculative::invocation_result_has_latency ...... OK
speculative::routing_config_serde_backward ...... OK
speculative::routing_config_with_speculative .... OK
model_selector::detect_simple_short_message ..... OK
model_selector::detect_complex_long_message ..... OK
model_selector::detect_standard_with_tools ...... OK
model_selector::detect_complex_multi_round ...... OK
model_selector::disabled_returns_none ........... OK
model_selector::budget_exceeded_forces_cheapest . OK
model_selector::simple_selects_cheap ............ OK
model_selector::complex_selects_capable ......... OK
model_selector::tools_filter_excludes_no_tools .. OK
model_selector::simple_model_override ........... OK
model_selector::complex_model_override .......... OK
model_selector::config_defaults ................. OK
model_selector::config_serde_roundtrip .......... OK
model_selector::reasoning_keywords_trigger ...... OK
+ 3 more routing config tests
```

### Doctor Diagnostics
```
Providers: 5 registered (anthropic, deepseek, gemini, ollama, openai)
Model Selection: active (balanced strategy, $5.00 budget cap)
  Simple override: deepseek-chat
  Complex override: gpt-4o
Orchestrator: enabled (4 concurrent agents, 300s timeout)
Resilience: enabled (circuit breaker + health scoring + backpressure)
Advanced Features: deterministic execution, W3C trace, inter-agent comm
```

---

## Phase 4: Known Issues & Recommendations

### Issue 1: Gemini Free Tier Rate Limiting
- **Status**: KNOWN (account issue)
- **Impact**: Gemini provider returns 429 on every request
- **Fix**: Upgrade to paid Gemini tier or use longer retry-after intervals
- **Workaround**: Gemini is disabled by default; routing automatically skips it

### Issue 2: Anthropic No Credits
- **Status**: KNOWN (account issue)
- **Impact**: Anthropic provider silently fails
- **Fix**: Add API credits to Anthropic account
- **Workaround**: Falls back to other providers in routing chain

### Issue 3: DeepSeek High Latency
- **Status**: EXPECTED (server-side)
- **Impact**: 4-8 second response times vs OpenAI's 0.8-1.5s
- **Recommendation**: Use DeepSeek for budget-limited tasks only; prefer OpenAI for latency-sensitive operations

### Issue 4: .env Variable Names
- **Status**: CONFIGURATION
- **Impact**: `.env` file uses `deepseek=`, `openai=`, `gemini=` but config expects `DEEPSEEK_API_KEY`, etc.
- **Fix**: Either rename .env vars or add dotenv support to cuervo

---

## Phase 5: Model Selection Config Validation

Current config (`~/.cuervo/config.toml`):
```toml
[agent.model_selection]
enabled = true
budget_cap_usd = 5.0
complexity_token_threshold = 1500
simple_model = "deepseek-chat"       # $0.14/1M — trivial tasks
complex_model = "gpt-4o"            # $2.50/1M — architecture/design
```

**Validation**:
- Simple tasks ("hola") -> deepseek-chat (correct, cheapest cloud)
- Standard tasks (with tools) -> balanced strategy picks optimal model
- Complex tasks (long context, reasoning keywords) -> gpt-4o (correct, most capable available)
- Budget gate at 90% of $5.00 -> forces cheapest model (correct)
- Provider switch on model selection -> NOW WORKING (Bug 1 fix)
- Fallback with correct models -> NOW WORKING (Bug 2 fix)

---

## Appendix: Test Environment

| Component | Value |
|-----------|-------|
| OS | macOS Darwin 24.3.0 |
| Rust | stable |
| Binary | 5.3MB (release, LTO fat) |
| Total tests | 1116 |
| Routing tests | 28 |
| Providers | 5 (deepseek, openai, gemini, anthropic, ollama) |
| Models tested | 10 |
| Operational | 8/10 (gemini: rate-limited, anthropic: no credits) |
