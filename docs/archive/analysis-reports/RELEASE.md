# Release Process Guide

Complete guide for creating and publishing new releases of Cuervo CLI.

## Table of Contents

- [Overview](#overview)
- [Prerequisites](#prerequisites)
- [Release Workflow](#release-workflow)
- [Supported Platforms](#supported-platforms)
- [Versioning](#versioning)
- [Troubleshooting](#troubleshooting)

## Overview

Cuervo CLI uses an automated release process powered by GitHub Actions. When you push a version tag, the following happens automatically:

1. **Build Matrix**: Compiles binaries for 6 platforms
2. **Create Archives**: Packages binaries as `.tar.gz` (Unix) or `.zip` (Windows)
3. **Generate Checksums**: Creates SHA256 checksums for verification
4. **Create Release**: Publishes GitHub Release with all artifacts
5. **Publish Crate** (optional): Publishes to crates.io

## Prerequisites

### Required Tools

- **Git**: Version control
- **Rust**: 1.80+ (`rustup`)
- **cross** (optional): For local cross-compilation testing
  ```bash
  cargo install cross --git https://github.com/cross-rs/cross
  ```

### Required Permissions

- **Repository**: Write access to push tags
- **GitHub Actions**: Enabled in repository settings
- **Secrets** (optional):
  - `CARGO_REGISTRY_TOKEN`: For crates.io publishing

### Local Validation

Before creating a release, ensure:

```bash
# All tests pass
cargo test --workspace --all-features

# No clippy warnings
cargo clippy --workspace --all-features -- -D warnings

# Documentation builds
cargo doc --workspace --no-deps

# Builds successfully
cargo build --release --features tui
```

## Release Workflow

### 1. Update Version

Edit `Cargo.toml` workspace version:

```toml
[workspace.package]
version = "0.2.0"  # Update this
```

### 2. Update Changelog

Add release notes to `CHANGELOG.md`:

```markdown
## [0.2.0] - 2026-02-14

### Added
- New TUI mode with improved UX
- Support for multiple AI providers

### Changed
- Improved error handling
- Better performance for large files

### Fixed
- Bug in authentication flow
- Memory leak in MCP bridge

### Security
- Updated dependencies with security patches
```

### 3. Commit Changes

```bash
git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "chore: bump version to v0.2.0"
git push origin main
```

### 4. Create and Push Tag

```bash
# Create annotated tag
git tag -a v0.2.0 -m "Release v0.2.0"

# Push tag to trigger release workflow
git push origin v0.2.0
```

### 5. Monitor Release

1. Go to **Actions** tab in GitHub
2. Watch the "Release" workflow
3. Wait for all builds to complete (~10-15 minutes)
4. Check for any failures

### 6. Verify Release

Once complete:

1. Go to **Releases** page
2. Find your new release
3. Verify all 6 platform binaries are attached
4. Download and test on at least 2 platforms:

```bash
# Linux/macOS
curl -fsSL https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.sh | sh
cuervo --version  # Should show v0.2.0

# Windows (PowerShell)
iwr -useb https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.ps1 | iex
cuervo --version  # Should show v0.2.0
```

## Supported Platforms

### Tier 1 (Fully Tested)

| Platform | Target | Runner | Notes |
|----------|--------|--------|-------|
| Linux x64 (glibc) | `x86_64-unknown-linux-gnu` | ubuntu-latest | Most common Linux |
| Linux x64 (musl) | `x86_64-unknown-linux-musl` | ubuntu-latest + cross | Static binary, Alpine |
| Linux ARM64 | `aarch64-unknown-linux-gnu` | ubuntu-latest + cross | Raspberry Pi, AWS Graviton |
| macOS Intel | `x86_64-apple-darwin` | macos-latest | macOS 10.13+ |
| macOS Apple Silicon | `aarch64-apple-darwin` | macos-latest | M1/M2/M3/M4 |
| Windows x64 | `x86_64-pc-windows-msvc` | windows-latest | Windows 10+ |

### Archive Formats

- **Unix/Linux/macOS**: `.tar.gz`
- **Windows**: `.zip`

### Checksums

All archives include SHA256 checksums:
- Format: `{hash}  {filename}`
- File: `{archive}.sha256`
- Verified automatically by installers

## Versioning

We follow [Semantic Versioning](https://semver.org/):

- **MAJOR**: Breaking changes (v1.0.0 → v2.0.0)
- **MINOR**: New features, backward compatible (v1.0.0 → v1.1.0)
- **PATCH**: Bug fixes, backward compatible (v1.0.0 → v1.0.1)

### Version Tags

- **Stable releases**: `v1.2.3`
- **Alpha releases**: `v1.2.3-alpha.1`
- **Beta releases**: `v1.2.3-beta.1`
- **Release candidates**: `v1.2.3-rc.1`

Pre-releases (alpha/beta/rc) are marked as "pre-release" on GitHub.

## Manual Release (Emergency)

If automated release fails, you can build and upload manually:

### 1. Build Binaries

```bash
# Linux x64 (glibc)
cargo build --release --target x86_64-unknown-linux-gnu --features tui

# macOS (native arch)
cargo build --release --features tui

# Windows (from Windows machine)
cargo build --release --target x86_64-pc-windows-msvc --features tui
```

### 2. Create Archives

```bash
# Unix
tar czf cuervo-x86_64-unknown-linux-gnu.tar.gz -C target/x86_64-unknown-linux-gnu/release cuervo
sha256sum cuervo-x86_64-unknown-linux-gnu.tar.gz > cuervo-x86_64-unknown-linux-gnu.tar.gz.sha256

# Windows (PowerShell)
Compress-Archive -Path target\x86_64-pc-windows-msvc\release\cuervo.exe -DestinationPath cuervo-x86_64-pc-windows-msvc.zip
(Get-FileHash -Algorithm SHA256 cuervo-x86_64-pc-windows-msvc.zip).Hash + "  cuervo-x86_64-pc-windows-msvc.zip" | Out-File -Encoding ASCII cuervo-x86_64-pc-windows-msvc.zip.sha256
```

### 3. Create Release

```bash
# Using GitHub CLI
gh release create v0.2.0 \
  --title "Cuervo CLI v0.2.0" \
  --notes "Release notes here" \
  cuervo-*.tar.gz \
  cuervo-*.tar.gz.sha256 \
  cuervo-*.zip \
  cuervo-*.zip.sha256
```

## Troubleshooting

### Build Failures

**Problem**: Build fails for specific target

**Solution**:
1. Check GitHub Actions logs for specific error
2. Test locally with `cross build --target {target}`
3. Verify target is supported: `rustc --print target-list | grep {target}`
4. Check for platform-specific dependencies

**Problem**: Clippy fails in CI

**Solution**:
```bash
# Fix locally
cargo clippy --workspace --all-features --fix --allow-dirty
git commit -am "fix: clippy warnings"
git push
```

### Release Failures

**Problem**: Release creation fails

**Solution**:
1. Check tag format matches: `v[0-9]+.[0-9]+.[0-9]+`
2. Verify tag doesn't already exist: `git tag | grep v0.2.0`
3. Check GitHub Actions permissions
4. Look for "create-release" job errors

**Problem**: Asset upload fails

**Solution**:
1. Check artifact exists in build output
2. Verify naming matches: `cuervo-{target}.{ext}`
3. Check file size limits (< 2GB per asset)
4. Retry workflow if transient network issue

### Installer Issues

**Problem**: Installer can't find binary

**Solution**:
1. Verify release is published (not draft)
2. Check asset naming exactly matches:
   - `cuervo-x86_64-unknown-linux-gnu.tar.gz`
   - `cuervo-x86_64-unknown-linux-gnu.tar.gz.sha256`
3. Test URL manually:
   ```bash
   curl -I https://github.com/cuervo-ai/cuervo-cli/releases/latest/download/cuervo-x86_64-unknown-linux-gnu.tar.gz
   ```

**Problem**: Checksum verification fails

**Solution**:
1. Verify checksum file format: `{hash}  {filename}` (two spaces)
2. Regenerate checksums if binary was modified
3. Check file wasn't corrupted during upload

## Hotfix Process

For critical bugs requiring immediate release:

1. **Create hotfix branch**:
   ```bash
   git checkout -b hotfix/v0.1.1 v0.1.0
   ```

2. **Fix the bug**:
   ```bash
   # Make fixes
   git commit -m "fix: critical security issue"
   ```

3. **Update version**:
   ```bash
   # Bump patch version
   sed -i 's/version = "0.1.0"/version = "0.1.1"/' Cargo.toml
   git commit -am "chore: bump version to v0.1.1"
   ```

4. **Merge and release**:
   ```bash
   git checkout main
   git merge hotfix/v0.1.1
   git tag v0.1.1
   git push origin main v0.1.1
   ```

## Best Practices

1. **Test thoroughly** before releasing
2. **Write clear release notes** explaining changes
3. **Use semantic versioning** consistently
4. **Tag annotated** (`git tag -a`) not lightweight
5. **Verify installers** work on multiple platforms
6. **Monitor** initial downloads for issues
7. **Keep CHANGELOG** up to date
8. **Security patches** get priority (hotfix)

## Release Checklist

- [ ] All tests pass locally
- [ ] Clippy has no warnings
- [ ] Version bumped in `Cargo.toml`
- [ ] CHANGELOG updated
- [ ] Changes committed and pushed
- [ ] Tag created and pushed
- [ ] GitHub Actions workflow completed
- [ ] All 6 binaries uploaded
- [ ] Checksums present for all assets
- [ ] Installers tested on ≥2 platforms
- [ ] Release notes are clear
- [ ] Documentation updated if needed

## Support

For release-related issues:

1. Check [GitHub Actions logs](https://github.com/cuervo-ai/cuervo-cli/actions)
2. Review [TESTING.md](scripts/TESTING.md)
3. Open issue with "release" label
4. Contact maintainers

## Additional Resources

- [GitHub Actions Documentation](https://docs.github.com/en/actions)
- [Semantic Versioning Spec](https://semver.org/)
- [Rust Cross Compilation](https://rust-lang.github.io/rustup/cross-compilation.html)
- [cargo-dist](https://opensource.axo.dev/cargo-dist/) - Alternative tool
