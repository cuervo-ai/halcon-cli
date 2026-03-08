## Summary

<!-- One paragraph: what does this PR do and why? -->

## Changes

<!-- Bullet list of concrete changes -->
-

## Type of Change

- [ ] Bug fix (non-breaking, fixes an issue)
- [ ] New feature (non-breaking, adds functionality)
- [ ] Breaking change (existing behavior changes)
- [ ] Refactor (no functional change)
- [ ] Documentation
- [ ] CI/CD
- [ ] Security fix

## Testing

- [ ] New tests added
- [ ] Existing tests pass (`cargo test --workspace`)
- [ ] Clippy clean (`cargo clippy --workspace -- -D warnings`)
- [ ] Formatted (`cargo fmt --all -- --check`)

## Security Checklist

- [ ] No credentials, API keys, or tokens in the diff
- [ ] Destructive operations require explicit user confirmation
- [ ] New tools go through FASE-2 gate in `executor.rs`
- [ ] No new `unsafe` without `// SAFETY:` justification
- [ ] See [SECURITY.md](docs/security/SECURITY.md) for full guidelines

## Affected Subsystem (if touching `repl/`)

- [ ] `agent/` — agent loop core
- [ ] `security/` — permissions, auth, blacklist
- [ ] `planning/` — planner, router, SLA
- [ ] `context/` — memory sources, vector store
- [ ] `plugins/` — plugin system
- [ ] `git_tools/` — git, CI, IDE
- [ ] `metrics/` — reward, scorer, health
- [ ] `bridges/` — MCP, task, runtime
- [ ] `domain/` — strategy, convergence
- [ ] `decision_engine/` — BDE pipeline

## Related Issues

<!-- Closes #N / Fixes #N -->
