# Installation Guide

Complete installation guide for Cuervo CLI across all supported platforms.

## Quick Start

**TL;DR**: One-line installation:

```bash
# Linux / macOS
curl -fsSL https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.sh | sh

# Windows (PowerShell)
iwr -useb https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.ps1 | iex
```

---

## Table of Contents

- [Supported Platforms](#supported-platforms)
- [Method 1: Binary Installation (Fastest)](#method-1-binary-installation-fastest)
- [Method 2: cargo-binstall](#method-2-cargo-binstall)
- [Method 3: cargo install](#method-3-cargo-install)
- [Method 4: From Source](#method-4-from-source)
- [Verification](#verification)
- [Configuration](#configuration)
- [Updating](#updating)
- [Uninstallation](#uninstallation)
- [Troubleshooting](#troubleshooting)

---

## Supported Platforms

| Platform | Architecture | Support Level |
|----------|-------------|---------------|
| **Linux (glibc)** | x86_64 | ✅ Tier 1 |
| **Linux (musl)** | x86_64 | ✅ Tier 1 |
| **Linux** | aarch64 (ARM64) | ✅ Tier 1 |
| **macOS** | Intel (x86_64) | ✅ Tier 1 |
| **macOS** | Apple Silicon (M1/M2/M3/M4) | ✅ Tier 1 |
| **Windows** | x86_64 | ✅ Tier 1 |

---

## Method 1: Binary Installation (Fastest)

**⏱️ Installation time: ~10 seconds**

Pre-compiled binaries are the fastest way to get started.

### Linux / macOS

```bash
curl -fsSL https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.sh | sh
```

**What it does:**
1. Detects your OS and architecture
2. Downloads the correct binary from GitHub Releases
3. Verifies SHA256 checksum
4. Installs to `~/.local/bin/cuervo`
5. Adds `~/.local/bin` to your PATH

**Custom installation directory:**

```bash
export CUERVO_INSTALL_DIR="$HOME/bin"
curl -fsSL https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.sh | sh
```

### Windows

Open PowerShell as **Administrator** (recommended) or regular user:

```powershell
iwr -useb https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.ps1 | iex
```

**What it does:**
1. Detects your architecture (x64/x86)
2. Downloads the correct `.zip` from GitHub Releases
3. Verifies SHA256 checksum
4. Installs to `%USERPROFILE%\.local\bin\cuervo.exe`
5. Adds to User PATH environment variable

**Custom installation directory:**

```powershell
$env:CUERVO_INSTALL_DIR = "C:\Tools"
iwr -useb https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.ps1 | iex
```

### Alpine Linux (musl)

Alpine uses musl instead of glibc:

```bash
# Installer auto-detects musl
curl -fsSL https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.sh | sh
```

The installer will download `cuervo-x86_64-unknown-linux-musl.tar.gz`.

### Manual Binary Download

If you prefer manual installation:

1. **Download**: Go to [Releases](https://github.com/cuervo-ai/cuervo-cli/releases/latest)
2. **Select**: Choose your platform asset:
   - `cuervo-x86_64-unknown-linux-gnu.tar.gz` (Linux x64)
   - `cuervo-aarch64-apple-darwin.tar.gz` (macOS Apple Silicon)
   - `cuervo-x86_64-pc-windows-msvc.zip` (Windows x64)
3. **Download checksum**: Also download the `.sha256` file
4. **Verify**:
   ```bash
   sha256sum -c cuervo-*.tar.gz.sha256
   ```
5. **Extract**:
   ```bash
   tar xzf cuervo-*.tar.gz
   ```
6. **Install**:
   ```bash
   mv cuervo ~/.local/bin/
   chmod +x ~/.local/bin/cuervo
   ```

---

## Method 2: cargo-binstall

**⏱️ Installation time: ~15 seconds**

[cargo-binstall](https://github.com/cargo-bins/cargo-binstall) downloads pre-compiled binaries from GitHub Releases.

### Prerequisites

```bash
# Install cargo-binstall first
cargo install cargo-binstall
```

### Install Cuervo

```bash
cargo binstall cuervo-cli
```

**Advantages:**
- Faster than `cargo install` (no compilation)
- Integrated with cargo ecosystem
- Supports private registries

---

## Method 3: cargo install

**⏱️ Installation time: ~2-5 minutes**

Compile from source using cargo.

### Prerequisites

1. **Rust 1.80+**:
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

2. **System dependencies** (Linux only):
   ```bash
   # Debian/Ubuntu
   sudo apt-get install pkg-config libssl-dev

   # Fedora/RHEL
   sudo dnf install pkg-config openssl-devel

   # Arch
   sudo pacman -S pkg-config openssl
   ```

### Install

```bash
# Latest release
cargo install --git https://github.com/cuervo-ai/cuervo-cli --tag v0.1.0 --features tui

# From main branch (development)
cargo install --git https://github.com/cuervo-ai/cuervo-cli --features tui
```

**Custom features:**

```bash
# TUI mode enabled
cargo install --git https://github.com/cuervo-ai/cuervo-cli --features tui

# All features
cargo install --git https://github.com/cuervo-ai/cuervo-cli --all-features
```

---

## Method 4: From Source

**⏱️ Build time: ~5-10 minutes**

For development or customization.

### Clone Repository

```bash
git clone https://github.com/cuervo-ai/cuervo-cli.git
cd cuervo-cli
```

### Build

```bash
# Debug build (faster compilation)
cargo build --features tui

# Release build (optimized)
cargo build --release --features tui
```

### Install

```bash
# Copy to ~/.local/bin
cp target/release/cuervo ~/.local/bin/

# Or install via cargo
cargo install --path crates/cuervo-cli --features tui
```

### Run without installing

```bash
cargo run --features tui -- --help
```

---

## Verification

After installation, verify it works:

```bash
# Check version
cuervo --version
# Output: cuervo 0.1.0 (f8f41dd0, aarch64-apple-darwin)

# Show help
cuervo --help

# Run diagnostics
cuervo doctor
```

---

## Configuration

### First-time Setup

```bash
# Initialize configuration
cuervo init

# Configure API keys
cuervo auth login anthropic
cuervo auth login openai
cuervo auth login deepseek
```

### Configuration File

Location: `~/.cuervo/config.toml`

```toml
[general]
default_provider = "deepseek"
default_model = "deepseek-chat"
max_tokens = 8192
temperature = 0.0

[models.providers.anthropic]
enabled = true
api_base = "https://api.anthropic.com"
default_model = "claude-sonnet-4-5-20250929"

[tools]
confirm_destructive = true
timeout_secs = 120
dry_run = false

[security]
pii_detection = true
audit_enabled = true
```

### Environment Variables

```bash
# API Keys
export ANTHROPIC_API_KEY="sk-ant-..."
export OPENAI_API_KEY="sk-..."
export DEEPSEEK_API_KEY="sk-..."

# Configuration
export CUERVO_CONFIG="$HOME/.cuervo/config.toml"
export CUERVO_DB="$HOME/.cuervo/cuervo.db"
```

---

## Updating

### Binary Installation

Re-run the installer:

```bash
# Linux/macOS
curl -fsSL https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.sh | sh

# Windows
iwr -useb https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.ps1 | iex
```

### cargo install

```bash
cargo install --git https://github.com/cuervo-ai/cuervo-cli --features tui --force
```

### Self-Update (Coming Soon)

```bash
cuervo update
```

---

## Uninstallation

### Binary Installation

```bash
# Linux/macOS
rm ~/.local/bin/cuervo
rm -rf ~/.cuervo  # Optional: remove config and data

# Windows
Remove-Item "$env:USERPROFILE\.local\bin\cuervo.exe"
Remove-Item -Recurse "$env:USERPROFILE\.cuervo"  # Optional
```

### cargo install

```bash
cargo uninstall cuervo-cli
```

---

## Troubleshooting

### "Command not found: cuervo"

**Problem**: PATH is not configured.

**Solution**:

```bash
# Check if binary exists
ls ~/.local/bin/cuervo

# Add to PATH manually
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc

# Or for zsh
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc
source ~/.zshrc
```

### "Permission denied" (Linux/macOS)

**Problem**: Binary is not executable.

**Solution**:

```bash
chmod +x ~/.local/bin/cuervo
```

### "curl: command not found"

**Problem**: curl is not installed.

**Solutions**:

```bash
# Ubuntu/Debian
sudo apt-get install curl

# macOS (should be pre-installed)
# If not: xcode-select --install

# Or use wget instead
wget -qO- https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.sh | sh
```

### Checksum Verification Fails

**Problem**: Downloaded file doesn't match checksum.

**Solutions**:

1. **Re-download**: Network corruption may have occurred
2. **Check release**: Ensure release was published correctly
3. **Manual verification**:
   ```bash
   sha256sum cuervo-*.tar.gz
   cat cuervo-*.tar.gz.sha256
   ```

### No Binary Available for Platform

**Problem**: Your platform doesn't have a precompiled binary.

**Solutions**:

1. **Use cargo install**:
   ```bash
   cargo install --git https://github.com/cuervo-ai/cuervo-cli --features tui
   ```

2. **Request support**: Open an issue for your platform
3. **Build from source**: See [Method 4](#method-4-from-source)

### SSL/TLS Errors

**Problem**: Certificate verification fails.

**Solutions**:

```bash
# Update CA certificates (Linux)
sudo apt-get install ca-certificates
sudo update-ca-certificates

# macOS
# Certificates should be managed by Keychain

# Temporary workaround (NOT recommended for production)
curl -k https://...  # Skip verification
```

### Windows SmartScreen Warning

**Problem**: Windows Defender flags the installer.

**Reason**: New binary without widespread usage.

**Solution**:

1. Click "More info"
2. Click "Run anyway"
3. Binary is safe (verify checksum if concerned)

### Firewall/Proxy Issues

**Problem**: Downloads fail behind corporate firewall.

**Solutions**:

```bash
# Configure proxy
export HTTP_PROXY="http://proxy.example.com:8080"
export HTTPS_PROXY="http://proxy.example.com:8080"

# Then run installer
curl -fsSL https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.sh | sh
```

---

## Getting Help

- **Documentation**: [docs/](docs/)
- **Issues**: [GitHub Issues](https://github.com/cuervo-ai/cuervo-cli/issues)
- **Discussions**: [GitHub Discussions](https://github.com/cuervo-ai/cuervo-cli/discussions)
- **Security**: See [SECURITY.md](SECURITY.md)

---

## Next Steps

After installation:

1. **Quick Start**: `cuervo --help`
2. **Configuration**: `cuervo init`
3. **Authentication**: `cuervo auth login <provider>`
4. **First Chat**: `cuervo chat "Hello, Cuervo!"`
5. **TUI Mode**: `cuervo --tui`

For more information, see the [User Guide](docs/USER_GUIDE.md).
