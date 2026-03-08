# Contributing to Halcon CLI

Thank you for your interest in contributing! This guide covers everything you need to get started.

## Table of Contents

- [Development Setup](#development-setup)
- [Project Structure](#project-structure)
- [Workflow](#workflow)
- [Testing](#testing)
- [Code Standards](#code-standards)
- [Security](#security)
- [Commit Messages](#commit-messages)

---

## Development Setup

```bash
# Prerequisites: Rust stable toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Clone and build
git clone https://github.com/cuervo-ai/halcon-cli.git
cd halcon-cli
cargo build --workspace

# Run tests
cargo test --workspace

# Run the CLI
cargo run -p halcon-cli -- "hello"
```

**Required tools:**
```bash
rustup component add rustfmt clippy
```

---

## Project Structure

```
crates/
├── halcon-cli/          # Main CLI binary + REPL
│   └── src/repl/        # Agent loop — organized into subdirectories:
│       ├── agent/       # Core agent loop
│       ├── bridges/     # MCP, task bridge, runtime
│       ├── context/     # Context sources (memory, vector, episodic)
│       ├── decision_engine/  # BDE pipeline
│       ├── domain/      # Strategy, convergence, termination
│       ├── git_tools/   # Git, CI, IDE integration
│       ├── hooks/       # Lifecycle hooks
│       ├── metrics/     # Reward, scorer, health
│       ├── planning/    # Planner, router, SLA
│       ├── plugins/     # Plugin system
│       ├── security/    # Auth, permissions, blacklist
│       └── servers/     # SDLC context servers
├── halcon-core/         # Shared types and traits
├── halcon-tools/        # Tool implementations (bash, file, search…)
├── halcon-context/      # Context pipeline + vector store
├── halcon-mcp/          # MCP server implementation
├── halcon-security/     # RBAC, sandboxing
├── halcon-storage/      # SQLite persistence
└── halcon-auth/         # OAuth 2.1 + PKCE
```

---

## Workflow

1. **Fork** the repository and create a branch from `main`:
   ```bash
   git checkout -b feat/my-feature
   ```

2. **Make changes** following the code standards below.

3. **Test** your changes:
   ```bash
   cargo test --workspace
   cargo clippy --workspace -- -D warnings
   cargo fmt --all -- --check
   ```

4. **Commit** using conventional commits (see [Commit Messages](#commit-messages)).

5. **Open a PR** against `main`. Fill out the PR template completely.

### Branch naming

| Type | Pattern | Example |
|------|---------|---------|
| Feature | `feat/description` | `feat/bedrock-provider` |
| Bug fix | `fix/description` | `fix/context-overflow` |
| Refactor | `refactor/description` | `refactor/repl-subdirs` |
| Docs | `docs/description` | `docs/mcp-guide` |

---

## Testing

Every PR must pass the full test suite:

```bash
# Full workspace tests
cargo test --workspace

# Single crate (faster iteration)
cargo test -p halcon-cli --lib

# With logging
RUST_LOG=debug cargo test -p halcon-cli --lib -- test_name
```

**Test requirements:**
- New features require new tests
- Bug fixes require a regression test
- No existing tests may be broken
- Security-sensitive changes require tests in `halcon-security/`

**Test files:** Place unit tests in the same file using `#[cfg(test)] mod tests { ... }`. Integration tests go in `crates/halcon-cli/tests/`.

---

## Code Standards

### Rust style
- `cargo fmt` before every commit — non-negotiable
- `cargo clippy -- -D warnings` must be clean
- No `unwrap()` in production paths — use `?` or explicit error handling
- No `std::sync::Mutex` in async functions — use `tokio::sync::Mutex`
- Prefer `tracing::` over `eprintln!`/`println!` in library code

### Security rules (enforced by CI)
- No new `unsafe` blocks without `// SAFETY:` comment and team review
- Destructive tool operations require explicit user confirmation
- No credentials, API keys, or tokens in source or tests
- New tool implementations must go through the FASE-2 security gate in `executor.rs`

### Adding a new tool
1. Implement `Tool` trait in `crates/halcon-tools/src/`
2. Register in `halcon_tools::full_registry()`
3. Add to `CATASTROPHIC_PATTERNS` check if the tool can delete/overwrite data
4. Write tests for both allowed and blocked cases

### Adding a new provider
1. Implement `ModelProvider` trait in `crates/halcon-core/src/traits/`
2. Add provider config to `crates/halcon-core/src/types/config.rs`
3. Register in `crates/halcon-cli/src/repl/provider_normalization.rs`
4. Add integration test in `crates/halcon-cli/tests/`

---

## Security

**Report vulnerabilities** privately via GitHub Security Advisories — do **not** open public issues for security bugs. See [SECURITY.md](docs/security/SECURITY.md).

**Security-sensitive paths** (require `@cuervo-ai/security` review):
- `crates/halcon-security/`
- `crates/halcon-auth/`
- `crates/halcon-sandbox/`
- `crates/halcon-tools/src/bash.rs` (CATASTROPHIC_PATTERNS)
- `crates/halcon-core/src/security.rs`

---

## Commit Messages

We follow [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <short description>

[optional body]

[optional footer]
```

**Types:** `feat`, `fix`, `refactor`, `docs`, `test`, `chore`, `perf`, `ci`

**Scopes:** `repl`, `tools`, `mcp`, `security`, `context`, `planning`, `cli`, `core`, `storage`, `async`

**Examples:**
```
feat(mcp): add OAuth 2.1 PKCE flow for HTTP servers
fix(async): replace std::sync::Mutex with tokio in model_selector
refactor(repl): migrate planning files to planning/ subdirectory
docs(contributing): add project structure and workflow sections
test(security): add CATASTROPHIC_PATTERNS regression tests
```

---

## Questions?

- Open a [Discussion](https://github.com/cuervo-ai/halcon-cli/discussions)
- Check existing [Issues](https://github.com/cuervo-ai/halcon-cli/issues)
- Read the architecture docs in `docs/03-architecture/`
