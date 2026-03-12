# CI Validation Policy — HALCON Workspace

> Version: 1.0 | Created: 2026-03-12 | Branch: feature/sota-intent-architecture

---

## Policy Summary

| Gate | Requirement | Block Merge? |
|------|-------------|--------------|
| `cargo check --workspace` | 0 errors, 0 warnings (deny) | YES |
| `cargo build --workspace` | Clean build | YES |
| `cargo test --workspace` | 0 failures | YES |
| New flaky test detected | Suite must be stable ≥3 runs | YES |
| Coverage regression | No crate loses >5% coverage | WARN |
| Ignored test added without comment | New `#[ignore]` must have documented reason | YES |

---

## Stage 1 — Compilation Gate

**Command**:
```bash
RUSTFLAGS="-D warnings" cargo check --workspace --exclude halcon-desktop
```

**Requirements**:
- 0 compiler errors
- 0 compiler warnings (warnings-as-errors enforced via `RUSTFLAGS`)
- Must complete within 120 seconds on standard CI runners

**Exceptions**:
- `halcon-desktop` may be excluded on non-macOS runners (requires macOS SDK)
- `#[allow(dead_code)]` is permitted only for stubs documented as Phase 2 placeholders

**Failure action**: Block merge immediately. No bypasses permitted.

---

## Stage 2 — Build Gate

**Command**:
```bash
cargo build --workspace --exclude halcon-desktop
```

**Requirements**:
- Clean release build succeeds
- Binary `halcon` is produced and executable
- No linker errors

**Failure action**: Block merge. Investigate linker/dependency issues before proceeding.

---

## Stage 3 — Test Gate

**Command**:
```bash
cargo test --workspace --exclude halcon-desktop -- --test-threads=4
```

**Requirements**:
- **0 test failures** — any failure blocks merge
- **Pass rate ≥ 99%** of non-ignored tests
- Suite must complete within 300 seconds (5 minutes)

**Current baseline** (2026-03-12):
```
passed=12,670  failed=0  ignored=31
```

**Ignored test policy**:
- Tests marked `#[ignore]` must include a comment explaining the reason
- Accepted reasons: requires display server, requires live API key, requires specific runtime environment
- New `#[ignore]` without documented reason blocks merge
- Maximum 40 ignored tests before requiring a cleanup sprint

**Failure action**: Block merge. Newly failing tests must be fixed or explicitly documented as known-broken with a linked issue.

---

## Stage 4 — Flaky Test Detection

**Trigger**: Any PR that modifies test files or async/timing-sensitive code.

**Command**:
```bash
for i in 1 2 3; do
  cargo test --workspace --exclude halcon-desktop -- --test-threads=1 2>&1 | tail -3
done
```

**Requirements**:
- Same pass/fail count across all 3 runs
- Any test that fails in ≥1 of 3 runs is classified as flaky
- Flaky tests **block merge** until stabilized

**Known timing-sensitive tests** (monitored, not blocked):
- `repl::agent::agent_scheduler::test_is_not_due_just_ran` — sub-millisecond window; passes in isolation

**Flaky test remediation policy**:
1. Identify root cause (global state, timing, filesystem race)
2. Preferred fix: use deterministic instance methods instead of global singletons
3. If unfixable without major refactor: add `#[ignore]` with documented reason + linked issue
4. Never use `sleep()` to paper over timing races — fix the race

---

## Stage 5 — Coverage Regression Check

**Trigger**: PRs modifying files in coverage-tracked modules.

**Tracked modules with coverage thresholds**:

| Module | Minimum Coverage | Current |
|--------|-----------------|---------|
| `repl::security` | 75% | ~80% |
| `repl::domain` | 80% | ~85% |
| `halcon-agent-core` | 70% | ~75% |
| `halcon-tools` | 65% | ~70% |
| `halcon-storage` | 70% | ~75% |

**Command** (when `cargo-llvm-cov` available):
```bash
cargo llvm-cov --workspace --exclude halcon-desktop \
  --ignore-filename-regex '(tests?\.rs|_test\.rs|benches?)' \
  --json --output-path coverage.json
```

**Requirements**:
- No crate drops more than 5% below its previous coverage baseline
- Coverage drop ≥5% triggers WARN (not block) with mandatory PR comment explaining the reduction

**Exception**: Test-only files (`tests.rs`, `_tests.rs`) excluded from coverage calculation.

---

## Stage 6 — Documentation Gate

**Trigger**: PRs that add new `#[ignore]` tests or modify `docs/remediation/`.

**Requirements**:
- Every new `#[ignore]` must have a comment: `// IGNORED: <reason> — see <issue/doc>`
- `docs/remediation/` documents must be updated when coverage gaps are closed
- `TEST_SUITE_MAP.md` total counts must match actual `cargo test` output (verified quarterly)

---

## Merge Requirements

All of the following must be satisfied before merge to `main`:

```
[ ] Stage 1: cargo check — 0 errors, 0 warnings
[ ] Stage 2: cargo build — clean build
[ ] Stage 3: cargo test — 0 failures
[ ] Stage 4: Flaky detection — stable across 3 runs (for async/timing changes)
[ ] Stage 5: Coverage — no crate regresses >5% (for logic changes)
[ ] Stage 6: Docs — #[ignore] tests documented
```

---

## Emergency Bypass Policy

In exceptional circumstances (e.g., broken dependency release blocking CI), a bypass may be granted by:
1. Lead engineer approval in PR comments
2. Linked issue filed with ≤72 hour remediation commitment
3. Maximum 1 active bypass at a time

**Never bypass for**:
- Test failures caused by the PR's own code changes
- New `#[ignore]` tests added without documentation
- `RUSTFLAGS="-D warnings"` failures

---

## Known Deferred Tests (Phase 2+ targets)

The following test categories are deferred pending infrastructure work:

| Test Category | Count | Blocking | Target Phase |
|---------------|-------|----------|--------------|
| Live provider tests (`halcon-providers::live_*`) | 8 | No | Separate CI job with secrets |
| Runtime environment tests (`halcon-runtime`) | 8 | No | Dedicated test environment |
| Clipboard tests | 3 | No | Display server in CI |
| OnceLock-dependent terminal tests | 2 | No | Process isolation refactor |
| GDEM integration tests | 0 (pending) | No | Phase 2 |

---

## CI Configuration Template

```yaml
# .github/workflows/ci.yml (reference)
name: CI

on: [push, pull_request]

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: Check (warnings as errors)
        run: RUSTFLAGS="-D warnings" cargo check --workspace --exclude halcon-desktop

  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: Run tests
        run: cargo test --workspace --exclude halcon-desktop -- --test-threads=4
      - name: Verify 0 failures
        run: |
          cargo test --workspace --exclude halcon-desktop 2>&1 | \
            grep -E "^test result" | \
            awk '{if ($6 != "0") exit 1}'

  flaky-detect:
    runs-on: ubuntu-latest
    if: contains(github.event.pull_request.changed_files, 'tests')
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: Run suite 3x for flaky detection
        run: |
          for i in 1 2 3; do
            cargo test --workspace --exclude halcon-desktop -- --test-threads=1
          done
```

---

## Revision History

| Date | Change | Author |
|------|--------|--------|
| 2026-03-12 | Initial policy — Phase 1 remediation | halcon-remediation |
