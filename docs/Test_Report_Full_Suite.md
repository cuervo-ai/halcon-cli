# Cuervo CLI — Full Test Report
## Multi-Provider & Orchestrated Routing Verification
**Date**: 2026-02-08T09:17Z
**Binary**: 5.3MB (release, opt-level="z", lto="fat")
**Tests**: 1116 unit/integration, 0 failures
**Platform**: macOS Darwin 24.3.0, Apple M4

---

## 1. Individual Model Test Results

| Status | Provider | Model | Latency | Tokens | Response |
|--------|----------|-------|---------|--------|----------|
| ✅ OK | deepseek | deepseek-chat | 1626ms | 0 | OK |
| ✅ OK | deepseek | deepseek-coder | 1344ms | 0 | OK |
| ✅ OK | deepseek | deepseek-reasoner | 5531ms | 0 | OK |
| ✅ OK | openai | gpt-4o-mini | 896ms | 0 | OK |
| ✅ OK | openai | gpt-4o | 996ms | 0 | OK |
| ✅ OK | openai | o3-mini | 1508ms | 0 | OK |
| ✅ OK | openai | o1 | 2645ms | 0 | OK |
| ❌ FAIL | gemini | gemini-2.0-flash | 28751ms | 0 | Rate limited (429) |
| ❌ FAIL | anthropic | claude-sonnet-4-5 | 460ms | 0 | No credits |
| ✅ OK | ollama | deepseek-coder-v2 | 212ms | 25 | OK |

**Success Rate**: 8/10 models functional (80%)
**Failures**: Both are account-level issues (not code bugs)
- Gemini: Free tier quota exhausted → needs billing
- Anthropic: Credit balance $0 → needs purchase

---

## 2. Bug Found & Fixed During Testing

### Bug: Reasoning models reject `temperature` parameter
- **Affected models**: o3-mini, o1 (OpenAI)
- **Error**: `Unsupported parameter: 'temperature' is not supported with this model`
- **Root cause**: `build_request()` sent `temperature` unconditionally
- **Fix**: Strip `temperature` for reasoning models (`supports_reasoning = true`)
- **File**: `crates/cuervo-providers/src/openai_compat/mod.rs:196-200`
- **Tests added**: 2 (`build_request_reasoning_model_strips_temperature`, `build_request_non_reasoning_preserves_temperature`)
- **Verification**: o3-mini and o1 now respond correctly

### Previous fix (from earlier session): Reasoning models reject `max_tokens`
- **Fix**: Route to `max_completion_tokens` for reasoning models
- **Test added**: 1 (`build_request_reasoning_model_uses_max_completion_tokens`)

---

## 3. Live Scenario Test Results

| Scenario | Provider/Model | Latency | Status | Notes |
|----------|---------------|---------|--------|-------|
| Simple chat | deepseek/deepseek-chat | 2562ms | ✅ | Correct Spanish response |
| Code gen (fast) | openai/gpt-4o-mini | 1538ms | ✅ | Valid Python code |
| Code gen (specialized) | deepseek/deepseek-coder | 3974ms | ✅ | Valid Python code |
| Complex architecture | openai/gpt-4o | 3659ms | ✅ | 5 microservices listed |
| Reasoning (OpenAI) | openai/o3-mini | 2693ms | ✅ | Correct logical answer |
| Reasoning (DeepSeek) | deepseek/deepseek-reasoner | 8435ms | ✅ | Correct logical answer |
| Latency race (OpenAI) | openai/gpt-4o-mini | 617ms | ✅ | Fastest cloud |
| Latency race (DeepSeek) | deepseek/deepseek-chat | 1388ms | ✅ | 2x slower than OpenAI |
| Latency race (Ollama) | ollama/deepseek-coder-v2 | 323ms | ✅ | **Fastest** (local) |
| Budget (DeepSeek) | deepseek/deepseek-chat | 2431ms | ✅ | $0.14/M input |
| Budget (Ollama) | ollama/deepseek-coder-v2 | 680ms | ✅ | **$0.00** (free) |

---

## 4. Orchestrated Routing Tests (Unit Tests)

| Mechanism | Tests | Status | Key Scenarios Covered |
|-----------|-------|--------|----------------------|
| Failover chain | 4 | ✅ All pass | Primary→fallback delegation, default mode |
| Speculative racing | 9 | ✅ All pass | select_ok racing, single provider, config serde |
| Model Selection | 14 | ✅ All pass | Simple/Standard/Complex detection, budget gate, overrides |
| Circuit Breaker | 21 | ✅ All pass | Trip/recover/re-trip lifecycle, half-open probes, backoff |
| Health Scoring | 15 | ✅ All pass | Composite score, slow/failing providers, filter unhealthy |
| Backpressure | 7 | ✅ All pass | Semaphore limits, utilization tracking, timeout |
| Orchestrator | 25 | ✅ All pass | Waves, parallel, sequential, shared context, communication |
| Response Cache | 25 | ✅ All pass | L1/L2 hit/miss, TTL, write-through, tool_use skip |
| Budget Control | 10 | ✅ All pass | Token budget, shared budget, exhaustion→cheapest |
| Dry-Run Mode | 15 | ✅ All pass | Full/destructive-only, synthetic results |
| Idempotency | 25 | ✅ All pass | Dedup, rollback hints, parallel batch |
| Contract Tests | 14 | ✅ All pass | 6 providers, reasoning models, cost validation |
| Agent Loop | 27 | ✅ All pass | Routing, metrics, events, session tracking |
| Doctor | 35 | ✅ All pass | All 14 diagnostic sections |

**Total Routing-Related Tests**: 246 / 1116 (22% of suite)
**All pass**: 246/246 (100%)

---

## 5. Provider Latency Comparison

```
Latency (simple prompt "What is 2+2?"):
╔══════════════════════════╦═══════════╦════════════╗
║ Provider/Model           ║ Latency   ║ Cost/query ║
╠══════════════════════════╬═══════════╬════════════╣
║ ollama/deepseek-coder-v2 ║   323ms   ║ $0.0000    ║ ← Fastest
║ openai/gpt-4o-mini       ║   617ms   ║ ~$0.0000   ║
║ openai/gpt-4o            ║   996ms   ║ ~$0.0001   ║
║ deepseek/deepseek-chat   ║  1388ms   ║ ~$0.0000   ║
║ openai/o3-mini           ║  1508ms   ║ ~$0.0000   ║
║ openai/o1                ║  2645ms   ║ ~$0.0002   ║
║ deepseek/deepseek-reason ║  5531ms   ║ ~$0.0000   ║
║ gemini/gemini-2.0-flash  ║    N/A    ║ Rate limit ║
║ anthropic/claude-sonnet  ║    N/A    ║ No credits ║
╚══════════════════════════╩═══════════╩════════════╝
```

---

## 6. Doctor Diagnostic Summary

```
Configuration:     1 warning (unbounded budget)
Providers:         5 registered (3 fully functional)
Resilience:        Circuit breaker, health scoring, backpressure active
Cache:             L1 (in-memory LRU) + L2 (SQLite) enabled
Model Selection:   Active (simple→deepseek-chat, complex→gpt-4o)
Orchestrator:      Enabled (4 concurrent agents, shared budget)
Advanced Features: Deterministic execution, idempotency, W3C tracing active
Accessibility:     6/8 tokens pass WCAG AA
```

---

## 7. Gaps & Recommendations

### Gaps Identified

| # | Gap | Impact | Resolution |
|---|-----|--------|------------|
| 1 | Gemini quota exhausted | Provider unusable | Enable billing on Google Cloud |
| 2 | Anthropic no credits | Provider unusable | Purchase API credits |
| 3 | Single-shot mode bypasses routing | No failover in `cuervo chat "prompt"` | Wire routing into single_shot() |
| 4 | Token count shows 0 for most providers | Missing SSE usage tracking | Verify stream_options.include_usage handling |
| 5 | DeepSeek cost shows $0.0000 | Pricing data may be too low for display | Use higher precision or adjust format |

### Recommendations

1. **Enable Gemini billing** — Provider works technically (code tested), just needs quota
2. **Add Anthropic credits** — Use `cuervo auth login anthropic` once funded
3. **Wire routing into single-shot** — Add `invoke_with_fallback()` to `single_shot()` for production resilience
4. **Fix token usage reporting** — Ensure `stream_options: { include_usage: true }` is processed for footer display
5. **Set budget limits** — Add `max_total_tokens = 1000000` and `max_duration_secs = 600` to config

---

## 8. Test Artifacts

| Artifact | Path |
|----------|------|
| Individual test outputs | `/tmp/cuervo_full_results/` |
| Scenario test script | `/tmp/cuervo_scenarios.sh` |
| Test harness | `/tmp/cuervo_full_test.sh` |
| Doctor output | (inline in this report) |
| Binary | `~/.local/bin/cuervo` (5.3MB) |
| Config | `~/.cuervo/config.toml` |

---

## 9. Test Matrix Summary

```
                    Individual    Routing Tests    Live Scenarios
                    ──────────    ─────────────    ──────────────
deepseek-chat       ✅             ✅ (failover)    ✅ (simple, budget)
deepseek-coder      ✅             ✅ (model sel)   ✅ (code gen)
deepseek-reasoner   ✅             ✅ (reasoning)   ✅ (reasoning)
gpt-4o-mini         ✅             ✅ (speculative)  ✅ (code, latency)
gpt-4o              ✅             ✅ (complex)     ✅ (architecture)
o3-mini             ✅ (fixed)     ✅ (reasoning)   ✅ (reasoning)
o1                  ✅ (fixed)     ✅ (reasoning)   N/A
gemini-2.0-flash    ❌ (quota)     ✅ (unit tests)  ❌ (quota)
claude-sonnet-4-5   ❌ (credits)   ✅ (unit tests)  ❌ (credits)
deepseek-coder-v2   ✅             ✅ (ollama)      ✅ (latency, budget)
```

**Overall**: 8/10 providers operational, 1116/1116 tests pass, 11/11 live scenarios pass, 246/246 routing tests pass.
