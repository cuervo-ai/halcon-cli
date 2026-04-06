# Halcon Remediation Prompts — Xiyo-Benchmarked

Per-phase prompts for systematic architectural remediation of Halcon CLI.
Each prompt is self-contained and designed for a single Claude Code session.

## Execution Order

```
QW (Quick Wins)  ─────────────────────────────────────────→ [1 week]
    │
    ▼
Phase 2 (REPL Decomposition) ────────────────────────────→ [2-3 weeks]
    │                              ┌─ Phase 5 (Memory)    → [2 weeks]  ──┐
    ▼                              ├─ Phase 6 (Tooling)   → [1 week]  ──┤
Phase 3 (Execution Model) ───────→├─ Phase 7 (Security)  → [1 week]  ──┤
    │                              └──────────────────────────────────────┘
    ▼                                          (parallelizable)
Phase 4 (Orchestration) ⚠️ HIGH RISK ────────────────────→ [2 weeks]
    │
    ▼
Phase 8 (Synthesis) ─────────────────────────────────────→ [3 days]
```

## Files

| File | Phase | Risk | Effort |
|------|-------|------|--------|
| [PHASE_QW_QUICK_WINS.md](PHASE_QW_QUICK_WINS.md) | Quick Wins (7 items) | Low | 1 week |
| [PHASE_2_REPL_DECOMPOSITION.md](PHASE_2_REPL_DECOMPOSITION.md) | REPL god-file decomposition | Medium | 2-3 weeks |
| [PHASE_3_EXECUTION_MODEL.md](PHASE_3_EXECUTION_MODEL.md) | FeedbackArbiter + loop simplification | Medium | 2 weeks |
| [PHASE_4_ORCHESTRATION.md](PHASE_4_ORCHESTRATION.md) | 3→1 orchestrator consolidation | **High** | 2 weeks |
| [PHASE_5_CONTEXT_MEMORY.md](PHASE_5_CONTEXT_MEMORY.md) | 6→2 memory consolidation + compaction | Medium | 3 weeks |
| [PHASE_6_TOOLING_PROVIDERS.md](PHASE_6_TOOLING_PROVIDERS.md) | build_tool() factory + Skills | Low | 1 week |
| [PHASE_7_SECURITY.md](PHASE_7_SECURITY.md) | Guardrails pipeline + hardening | Low | 1 week |
| [PHASE_8_SYNTHESIS.md](PHASE_8_SYNTHESIS.md) | Validation + architecture doc | Low | 3 days |

## Dependencies

- Phase 2 → Phase 3 (executor must exist before loop simplification)
- Phase 3 → Phase 4 (execution model must be stable before orchestrator work)
- Phase 2b → Phase 7a (permission pipeline must exist for GuardrailsGate)
- Phases 5, 6, 7 are parallelizable with 2-4

## Usage

Copy the content of each `.md` file into a fresh Claude Code session on the Halcon repo:
```bash
cd ~/Documents/Github/cuervo-cli
# Paste prompt content into Claude Code
```

## Xiyo Reference

All prompts reference Xiyo (Claude Code, 512K LOC TypeScript) as the architectural benchmark.
Xiyo source is at ~/Documents/Github/xiyo for cross-reference.

Key Xiyo patterns transferred:
1. Async generator loop with explicit State + typed transitions
2. Single feedback authority (model's stop_reason)
3. buildTool() factory with fail-closed defaults
4. 5-layer compaction hierarchy
5. Model-ranked memory (no vector embeddings)
6. Skills system (user-defined prompts from disk)
7. Error withholding waterfall
8. Permission pipeline (checkPermissions → allow|deny|ask)
