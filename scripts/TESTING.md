# Testing & Validation Guide

## Pre-Release Checklist

Before creating a new release, validate the following:

### ✅ Scripts Validation

- [ ] `install-binary.sh` has no syntax errors (`bash -n scripts/install-binary.sh`)
- [ ] `install-binary.ps1` has no syntax errors
- [ ] Both scripts are executable on their respective platforms
- [ ] All required dependencies are checked (curl/wget, tar, sha256sum)
- [ ] HTTPS is enforced (no insecure HTTP URLs except localhost)
- [ ] No hardcoded credentials or tokens
- [ ] Error handling is robust (`set -euo pipefail` in bash)

### ✅ Build Validation

- [ ] Release profile is optimized (`Cargo.toml` has strip, lto, codegen-units=1)
- [ ] All workspace crates compile successfully
- [ ] Tests pass: `cargo test --workspace --all-features`
- [ ] Clippy passes: `cargo clippy --workspace --all-features -- -D warnings`
- [ ] Documentation builds: `cargo doc --workspace --no-deps`

### ✅ Cross-Compilation Testing

Test on each target platform:

#### Linux x86_64 (glibc)
```bash
cargo build --release --target x86_64-unknown-linux-gnu --features tui
./target/x86_64-unknown-linux-gnu/release/cuervo --version
```

#### Linux x86_64 (musl - static)
```bash
cross build --release --target x86_64-unknown-linux-musl --features tui
./target/x86_64-unknown-linux-musl/release/cuervo --version
```

#### macOS Intel
```bash
cargo build --release --target x86_64-apple-darwin --features tui
./target/x86_64-apple-darwin/release/cuervo --version
```

#### macOS Apple Silicon
```bash
cargo build --release --target aarch64-apple-darwin --features tui
./target/aarch64-apple-darwin/release/cuervo --version
```

#### Windows x64
```powershell
cargo build --release --target x86_64-pc-windows-msvc --features tui
.\target\x86_64-pc-windows-msvc\release\cuervo.exe --version
```

### ✅ Manual Installation Testing

#### Unix/Linux/macOS
```bash
# Set test environment
export CUERVO_INSTALL_DIR="/tmp/cuervo-test-$$"
export CUERVO_REPO_OWNER="cuervo-ai"
export CUERVO_REPO_NAME="cuervo-cli"

# Run installer
curl -fsSL https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.sh | sh

# Verify
$CUERVO_INSTALL_DIR/cuervo --version

# Cleanup
rm -rf $CUERVO_INSTALL_DIR
```

#### Windows (PowerShell)
```powershell
# Set test environment
$env:CUERVO_INSTALL_DIR = "$env:TEMP\cuervo-test"

# Run installer
iwr -useb https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.ps1 | iex

# Verify
& "$env:CUERVO_INSTALL_DIR\cuervo.exe" --version

# Cleanup
Remove-Item -Recurse -Force $env:CUERVO_INSTALL_DIR
```

### ✅ GitHub Actions Workflow

- [ ] Workflow syntax is valid (`.github/workflows/release.yml`)
- [ ] All matrix targets are configured correctly
- [ ] Secrets are available: `GITHUB_TOKEN` (auto), `CARGO_REGISTRY_TOKEN` (optional)
- [ ] Asset naming follows convention: `cuervo-{target}.{ext}`
- [ ] Checksums are generated for all assets
- [ ] Release notes template is correct

### ✅ Release Process

1. **Update version in Cargo.toml**
   ```bash
   # Update workspace version
   sed -i 's/version = "0.1.0"/version = "0.2.0"/' Cargo.toml
   ```

2. **Update CHANGELOG.md**
   ```markdown
   ## [0.2.0] - 2026-02-14
   ### Added
   - Feature X
   ### Fixed
   - Bug Y
   ```

3. **Commit changes**
   ```bash
   git add Cargo.toml CHANGELOG.md
   git commit -m "chore: bump version to 0.2.0"
   ```

4. **Create and push tag**
   ```bash
   git tag v0.2.0
   git push origin main
   git push origin v0.2.0
   ```

5. **Monitor GitHub Actions**
   - Go to Actions tab
   - Watch release workflow
   - Verify all builds complete successfully
   - Check release artifacts are uploaded

6. **Verify release**
   - Download binaries from release page
   - Test installation on at least 2 platforms
   - Verify checksums match

### ✅ Post-Release

- [ ] Test installer with new release
- [ ] Update documentation if API changed
- [ ] Announce release (if applicable)
- [ ] Monitor issue tracker for bug reports

## Automated Testing

Run the test suite:

```bash
./scripts/test-install.sh
```

Expected output:
- All syntax checks pass
- Security checks pass
- Dependency checks pass

## Troubleshooting

### Build fails on certain target

- Check if target requires `cross`: https://github.com/cross-rs/cross
- Verify target is in rustc: `rustc --print target-list`
- Check GitHub Actions logs for specific errors

### Installer fails to download

- Verify release exists: `https://github.com/cuervo-ai/cuervo-cli/releases/latest`
- Check asset naming matches: `cuervo-{target}.tar.gz`
- Verify checksum file exists: `cuervo-{target}.tar.gz.sha256`

### Checksum verification fails

- Regenerate checksums after any binary modification
- Ensure checksum format is: `{hash}  {filename}` (two spaces)
- Use `sha256sum` on Linux/macOS, `Get-FileHash` on Windows

## Manual Test Scenarios

### Scenario 1: Fresh install on clean system
1. Use Docker container or VM
2. Run installer
3. Verify binary works
4. Verify PATH is updated

### Scenario 2: Upgrade existing installation
1. Install v0.1.0
2. Install v0.2.0
3. Verify old binary is replaced
4. Verify version command shows v0.2.0

### Scenario 3: Fallback to cargo install
1. Mock GitHub releases as unavailable
2. Verify installer attempts cargo-binstall
3. Verify installer falls back to cargo install
4. Verify successful installation from source

### Scenario 4: Platform detection
Test on:
- Ubuntu 20.04 (Linux x86_64 glibc)
- Alpine Linux (Linux x86_64 musl)
- macOS 12 Intel (x86_64-apple-darwin)
- macOS 14 M4 (aarch64-apple-darwin)
- Windows 11 (x86_64-pc-windows-msvc)

## Performance Benchmarks

Record binary sizes for comparison:

```bash
# Check binary size
ls -lh target/release/cuervo

# Check stripped size
strip target/release/cuervo
ls -lh target/release/cuervo

# Check compressed size
tar czf cuervo.tar.gz -C target/release cuervo
ls -lh cuervo.tar.gz
```

Target sizes:
- Uncompressed: < 10 MB
- Compressed (tar.gz): < 3 MB

## Security Validation

- [ ] No credentials in code
- [ ] HTTPS enforced for all downloads
- [ ] Checksums verified before installation
- [ ] No eval or dangerous shell constructs
- [ ] Proper error handling (fail-fast)
- [ ] Minimal permissions required (no sudo)
