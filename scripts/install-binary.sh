#!/usr/bin/env bash
# Halcón CLI - Universal Binary Installer
# Usage: curl -sSfL https://cli.cuervo.cloud/install.sh | sh
# or:    wget -qO- https://cli.cuervo.cloud/install.sh | sh

set -euo pipefail

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Constants
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

readonly REPO_OWNER="${HALCON_REPO_OWNER:-cuervo-ai}"
readonly REPO_NAME="${HALCON_REPO_NAME:-halcon-cli}"
readonly BINARY_NAME="halcon"
readonly INSTALL_DIR="${HALCON_INSTALL_DIR:-$HOME/.local/bin}"
readonly GITHUB_API="https://api.github.com"
readonly GITHUB_DOWNLOAD="https://github.com/${REPO_OWNER}/${REPO_NAME}/releases/latest/download"
# Optional: set HALCON_GITHUB_TOKEN for private repo access, or HALCON_VERSION to pin a version
readonly HALCON_VERSION="${HALCON_VERSION:-}"
readonly HALCON_GITHUB_TOKEN="${HALCON_GITHUB_TOKEN:-}"

get_latest_version() {
    # Allow pinning a specific version via env var (also works for private repos without token)
    if [ -n "$HALCON_VERSION" ]; then
        echo "$HALCON_VERSION"
        return 0
    fi

    local auth_header=""
    if [ -n "$HALCON_GITHUB_TOKEN" ]; then
        auth_header="Authorization: Bearer ${HALCON_GITHUB_TOKEN}"
    fi

    local version
    if has curl; then
        if [ -n "$auth_header" ]; then
            version="$(curl --proto '=https' --tlsv1.2 -fsSL \
                -H "$auth_header" \
                "${GITHUB_API}/repos/${REPO_OWNER}/${REPO_NAME}/releases/latest" \
                | grep '"tag_name"' | sed 's/.*"tag_name": *"v\?\([^"]*\)".*/\1/')"
        else
            version="$(curl --proto '=https' --tlsv1.2 -fsSL \
                "${GITHUB_API}/repos/${REPO_OWNER}/${REPO_NAME}/releases/latest" \
                | grep '"tag_name"' | sed 's/.*"tag_name": *"v\?\([^"]*\)".*/\1/')"
        fi
    elif has wget; then
        if [ -n "$auth_header" ]; then
            version="$(wget --https-only -qO- \
                --header="$auth_header" \
                "${GITHUB_API}/repos/${REPO_OWNER}/${REPO_NAME}/releases/latest" \
                | grep '"tag_name"' | sed 's/.*"tag_name": *"v\?\([^"]*\)".*/\1/')"
        else
            version="$(wget --https-only -qO- \
                "${GITHUB_API}/repos/${REPO_OWNER}/${REPO_NAME}/releases/latest" \
                | grep '"tag_name"' | sed 's/.*"tag_name": *"v\?\([^"]*\)".*/\1/')"
        fi
    else
        error "Neither curl nor wget found."
    fi

    if [ -z "$version" ]; then
        error "Failed to determine latest release version. Try setting HALCON_VERSION=x.y.z or HALCON_GITHUB_TOKEN=<token>"
    fi
    echo "$version"
}

# Colors
readonly RED='\033[0;31m'
readonly GREEN='\033[0;32m'
readonly YELLOW='\033[1;33m'
readonly BLUE='\033[0;34m'
readonly MAGENTA='\033[0;35m'
readonly CYAN='\033[0;36m'
readonly BOLD='\033[1m'
readonly NC='\033[0m'

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Utility Functions
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

info() {
    echo -e "${BLUE}${BOLD}[INFO]${NC}  $*" >&2
}

success() {
    echo -e "${GREEN}${BOLD}[✓]${NC}     $*" >&2
}

warn() {
    echo -e "${YELLOW}${BOLD}[WARN]${NC}  $*" >&2
}

error() {
    echo -e "${RED}${BOLD}[ERROR]${NC} $*" >&2
    exit 1
}

header() {
    echo -e "\n${CYAN}${BOLD}━━━ $* ━━━${NC}\n" >&2
}

has() {
    command -v "$1" >/dev/null 2>&1
}

need() {
    if ! has "$1"; then
        error "Required command not found: $1"
    fi
}

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Platform Detection
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

detect_os() {
    local os
    os="$(uname -s)"
    case "$os" in
        Linux)  echo "linux" ;;
        Darwin) echo "darwin" ;;
        MINGW*|MSYS*|CYGWIN*) echo "windows" ;;
        *)      error "Unsupported OS: $os" ;;
    esac
}

detect_arch() {
    local arch
    arch="$(uname -m)"
    case "$arch" in
        x86_64|amd64)  echo "x86_64" ;;
        aarch64|arm64) echo "aarch64" ;;
        armv7l)        echo "armv7" ;;
        i686|i386)     echo "i686" ;;
        *)             error "Unsupported architecture: $arch" ;;
    esac
}

detect_libc() {
    local os="$1"
    if [ "$os" = "linux" ]; then
        if ldd --version 2>&1 | grep -q musl; then
            echo "musl"
        else
            echo "gnu"
        fi
    else
        echo ""
    fi
}

construct_target() {
    local os="$1"
    local arch="$2"
    local libc="$3"

    case "$os-$arch" in
        linux-x86_64)
            if [ "$libc" = "musl" ]; then
                echo "x86_64-unknown-linux-musl"
            else
                echo "x86_64-unknown-linux-gnu"
            fi
            ;;
        linux-aarch64)
            if [ "$libc" = "musl" ]; then
                echo "aarch64-unknown-linux-musl"
            else
                echo "aarch64-unknown-linux-gnu"
            fi
            ;;
        linux-armv7)
            echo "armv7-unknown-linux-gnueabihf"
            ;;
        darwin-x86_64)
            echo "x86_64-apple-darwin"
            ;;
        darwin-aarch64)
            echo "aarch64-apple-darwin"
            ;;
        windows-x86_64)
            echo "x86_64-pc-windows-msvc"
            ;;
        *)
            error "Unsupported platform: $os-$arch"
            ;;
    esac
}

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Download & Verification
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

download_file() {
    local url="$1"
    local output="$2"

    if has curl; then
        curl --proto '=https' --tlsv1.2 -fsSL "$url" -o "$output"
    elif has wget; then
        wget --https-only --secure-protocol=TLSv1_2 -q -O "$output" "$url"
    else
        error "Neither curl nor wget found. Please install one of them."
    fi
}

# For private GitHub repos, the browser download URL doesn't work — must use the API.
# Finds the asset by name in the releases/latest response, downloads via asset endpoint.
download_github_asset() {
    local asset_name="$1"
    local output="$2"

    if [ -z "$HALCON_GITHUB_TOKEN" ]; then
        # Public repo: use browser download URL directly
        local url="${GITHUB_DOWNLOAD}/${asset_name}"
        download_file "$url" "$output"
        return $?
    fi

    # Private repo: resolve asset API URL, then stream with Accept: application/octet-stream
    info "Resolving asset via GitHub API..."
    local release_json asset_url

    if has curl; then
        release_json="$(curl --proto '=https' --tlsv1.2 -fsSL \
            -H "Authorization: Bearer ${HALCON_GITHUB_TOKEN}" \
            "${GITHUB_API}/repos/${REPO_OWNER}/${REPO_NAME}/releases/latest")"
    elif has wget; then
        release_json="$(wget --https-only -qO- \
            --header="Authorization: Bearer ${HALCON_GITHUB_TOKEN}" \
            "${GITHUB_API}/repos/${REPO_OWNER}/${REPO_NAME}/releases/latest")"
    fi

    # Use python3 (available on macOS/Linux) to extract the asset API URL by name
    if has python3; then
        asset_url="$(echo "$release_json" | python3 -c "
import sys, json
data = json.load(sys.stdin)
for a in data.get('assets', []):
    if a.get('name') == '${asset_name}':
        print(a.get('url', ''))
        break
")"
    else
        # Fallback: awk-based extraction (url field is in assets array before name field)
        asset_url="$(echo "$release_json" | tr ',' '\n' | grep -E '"url":"https://api.github.com.*assets' | sed 's/.*"url":"\([^"]*\)".*/\1/' | head -1)"
    fi

    if [ -z "$asset_url" ]; then
        warn "Asset '$asset_name' not found in latest release"
        return 1
    fi

    if has curl; then
        curl --proto '=https' --tlsv1.2 -fsSL \
            -H "Authorization: Bearer ${HALCON_GITHUB_TOKEN}" \
            -H "Accept: application/octet-stream" \
            "$asset_url" -o "$output"
    elif has wget; then
        wget --https-only --secure-protocol=TLSv1_2 -q \
            --header="Authorization: Bearer ${HALCON_GITHUB_TOKEN}" \
            --header="Accept: application/octet-stream" \
            -O "$output" "$asset_url"
    fi
}

verify_checksum() {
    local file="$1"
    local checksum_file="$2"

    if ! has sha256sum; then
        warn "sha256sum not found, skipping checksum verification"
        return 0
    fi

    local expected_hash
    expected_hash="$(awk '{print $1}' "$checksum_file")"

    local actual_hash
    actual_hash="$(sha256sum "$file" | awk '{print $1}')"

    if [ "$expected_hash" != "$actual_hash" ]; then
        error "Checksum verification failed!\nExpected: $expected_hash\nGot:      $actual_hash"
    fi

    success "Checksum verified"
}

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Installation
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

extract_archive() {
    local archive="$1"
    local target_dir="$2"

    case "$archive" in
        *.tar.gz|*.tgz)
            need tar
            tar xzf "$archive" -C "$target_dir"
            ;;
        *.zip)
            need unzip
            unzip -q "$archive" -d "$target_dir"
            ;;
        *)
            error "Unsupported archive format: $archive"
            ;;
    esac
}

install_binary() {
    local binary_path="$1"
    local install_dir="$2"

    mkdir -p "$install_dir"
    cp "$binary_path" "$install_dir/${BINARY_NAME}"
    chmod +x "$install_dir/${BINARY_NAME}"

    success "Installed to $install_dir/${BINARY_NAME}"
}

detect_shell_profile() {
    if [ -n "${BASH_VERSION:-}" ]; then
        if [ -f "$HOME/.bashrc" ]; then
            echo "$HOME/.bashrc"
        elif [ -f "$HOME/.bash_profile" ]; then
            echo "$HOME/.bash_profile"
        else
            echo "$HOME/.profile"
        fi
    elif [ -n "${ZSH_VERSION:-}" ]; then
        echo "$HOME/.zshrc"
    elif [ -n "${FISH_VERSION:-}" ]; then
        echo "$HOME/.config/fish/config.fish"
    else
        case "$SHELL" in
            */bash)
                [ -f "$HOME/.bashrc" ] && echo "$HOME/.bashrc" || echo "$HOME/.profile"
                ;;
            */zsh)
                echo "$HOME/.zshrc"
                ;;
            */fish)
                echo "$HOME/.config/fish/config.fish"
                ;;
            *)
                echo "$HOME/.profile"
                ;;
        esac
    fi
}

add_to_path() {
    local install_dir="$1"

    if echo "$PATH" | tr ':' '\n' | grep -qx "$install_dir"; then
        success "$install_dir is already in PATH"
        return 0
    fi

    local shell_profile
    shell_profile="$(detect_shell_profile)"

    info "Adding $install_dir to PATH in $shell_profile"

    mkdir -p "$(dirname "$shell_profile")"
    touch "$shell_profile"

    if ! grep -q "export PATH=\"$install_dir:\$PATH\"" "$shell_profile"; then
        echo "" >> "$shell_profile"
        echo "# Added by halcon-cli installer" >> "$shell_profile"
        echo "export PATH=\"$install_dir:\$PATH\"" >> "$shell_profile"
        success "Added to PATH. Run: source $shell_profile"
    else
        success "PATH entry already exists in $shell_profile"
    fi
}

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Fallback Installation
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

try_cargo_binstall() {
    if ! has cargo-binstall; then
        return 1
    fi

    info "Attempting installation via cargo-binstall..."
    if cargo-binstall -y "${REPO_NAME}"; then
        success "Installed via cargo-binstall"
        return 0
    else
        return 1
    fi
}

try_cargo_install() {
    if ! has cargo; then
        error "cargo not found. Please install Rust from https://rustup.rs/"
    fi

    warn "No precompiled binary available for your platform."
    info "Falling back to cargo install (this will compile from source, may take 2-5 minutes)..."

    if cargo install --git "https://github.com/${REPO_OWNER}/${REPO_NAME}" --locked --no-default-features halcon-cli; then
        success "Installed via cargo install"
        return 0
    else
        error "cargo install failed"
    fi
}

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Frontier Tools Setup
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

# Is the script running interactively (TTY)?
is_interactive() { [ -t 0 ]; }

# Prompt helper — 0=yes, 1=no. Non-interactive returns default.
ask() {
    local prompt="$1" default="${2:-y}"
    if ! is_interactive; then [ "$default" = "y" ] && return 0 || return 1; fi
    local yn_hint; [ "$default" = "y" ] && yn_hint="[Y/n]" || yn_hint="[y/N]"
    read -r -p "  ${BOLD}${CYAN}?${NC} ${prompt} ${yn_hint} " answer
    answer="${answer:-$default}"
    case "$answer" in [Yy]*) return 0 ;; *) return 1 ;; esac
}

setup_frontier_tools() {
    local install_dir="$1"
    local config_dir="$HOME/.halcon"
    local config_file="$config_dir/config.toml"

    header "Frontier tools"

    echo ""
    echo -e "  Halcón includes a suite of ${BOLD}frontier capabilities${NC}:"
    echo -e "    • ${CYAN}Agent Registry${NC}     — declarative sub-agents (.md frontmatter)"
    echo -e "    • ${CYAN}Semantic Memory${NC}    — TF-IDF vector search over session memory"
    echo -e "    • ${CYAN}MCP Ecosystem${NC}      — GitHub, Slack, Linear via Model Context Protocol"
    echo -e "    • ${CYAN}Hooks System${NC}       — PreToolUse/PostToolUse lifecycle hooks"
    echo -e "    • ${CYAN}Audit & Compliance${NC} — SOC2 JSONL/CSV/PDF + HMAC chain verification"
    echo -e "    • ${CYAN}Cenzontle SSO${NC}      — Zuclubit OAuth 2.1 PKCE enterprise login"
    echo -e "    • ${CYAN}VS Code Extension${NC}  — JSON-RPC bridge for IDE integration"
    echo -e "    • ${CYAN}Full Tool Suite${NC}    — 50+ tools: semantic_grep, native_search..."
    echo ""

    if is_interactive; then
        if ! ask "Set up frontier tools now?" "y"; then
            warn "Skipping. Edit $config_file manually or re-run installer."
            return 0
        fi
    fi

    mkdir -p "$config_dir"

    # ── Base config ──────────────────────────────────────────────────────────
    if [ ! -f "$config_file" ]; then
        cat > "$config_file" << 'TOML'
[general]
default_provider = "deepseek"
default_model = "deepseek-chat"
max_tokens = 8192
temperature = 0.0

[models.providers.anthropic]
enabled = true
api_key_env = "ANTHROPIC_API_KEY"
default_model = "claude-sonnet-4-6"

[models.providers.deepseek]
enabled = true
api_base = "https://api.deepseek.com"
api_key_env = "DEEPSEEK_API_KEY"
default_model = "deepseek-chat"

[models.providers.openai]
enabled = true
api_base = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
default_model = "gpt-4o-mini"

[models.providers.gemini]
enabled = true
api_key_env = "GEMINI_API_KEY"
default_model = "gemini-1.5-pro"

[models.providers.ollama]
enabled = true
api_base = "http://localhost:11434"
default_model = "llama3.2"

[tools]
confirm_destructive = true
timeout_secs = 120

[security]
pii_detection = true
audit_enabled = true
TOML
        success "Base config written to $config_file"
    fi

    # ── Policy + frontier config block ───────────────────────────────────────
    if ! grep -q '\[policy\]' "$config_file" 2>/dev/null; then
        cat >> "$config_file" << 'TOML'

[policy]
enable_agent_registry  = true
enable_semantic_memory = true
semantic_memory_top_k  = 5
enable_hooks           = true
enable_audit_trail     = true

[tools.advanced]
enable_native_search = true
enable_background    = true
enable_semantic_grep = true
enable_docker        = false
enable_sql_query     = true
enable_secret_scan   = true
enable_web_fetch     = true
enable_code_metrics  = true

[mcp_server]
transport    = "stdio"
port         = 7777
require_auth = false

[context]
max_tokens           = 180000
compaction_threshold = 0.80
enable_repo_map      = true

[agent]
enable_registry        = true
enable_lifecycle_hooks = true
enable_planner         = true
enable_convergence     = true
max_rounds             = 50
TOML
        success "Frontier policy config added"
    fi

    # ── Agent registry ───────────────────────────────────────────────────────
    local agents_dir="$config_dir/agents"
    mkdir -p "$agents_dir"
    if [ ! -f "$agents_dir/code-reviewer.md" ]; then
        cat > "$agents_dir/code-reviewer.md" << 'MD'
---
name: code-reviewer
description: |
  Expert code reviewer for correctness, security, and architecture.
  Use when asked to review, audit, or assess code quality.
tools: [Read, Grep, Glob]
model: claude-sonnet-4-6
max_turns: 20
---

Focus on correctness, security vulnerabilities, and readability.
Always provide specific line references and actionable suggestions.
MD
        success "Created $agents_dir/code-reviewer.md"
    fi
    if [ ! -f "$agents_dir/test-writer.md" ]; then
        cat > "$agents_dir/test-writer.md" << 'MD'
---
name: test-writer
description: |
  Writes comprehensive test suites (unit, integration, e2e).
  Use when asked to add tests or increase test coverage.
tools: [Read, Grep, Glob, file_write, bash]
model: claude-sonnet-4-6
max_turns: 30
---

Write thorough, non-flaky tests. Prefer integration tests over mocks where possible.
MD
        success "Created $agents_dir/test-writer.md"
    fi

    # ── HALCON.md instruction persistence ───────────────────────────────────
    local halcon_md="$config_dir/HALCON.md"
    if [ ! -f "$halcon_md" ]; then
        cat > "$halcon_md" << 'MD'
# Halcón — Personal Instructions

<!-- Loaded at the start of every session. Keep under 200 lines. -->

## Preferences

- Prefer concise, direct answers
- Show file paths with line numbers when referencing code
- Use audit trail for destructive operations

## Project Conventions

<!-- Add project-specific rules, e.g. language, test runner, commit style -->

## Memory

<!-- Semantic memory: ~/.halcon/memory/ -->
<!-- Use `search_memory` tool to retrieve past context -->
MD
        success "Created $halcon_md"
    fi

    # ── MCP config ───────────────────────────────────────────────────────────
    local mcp_file="$config_dir/mcp.toml"
    if [ ! -f "$mcp_file" ]; then
        cat > "$mcp_file" << 'TOML'
# Halcón MCP — Model Context Protocol servers
# Run `halcon mcp list` to see configured servers
# Run `halcon mcp add <name> -- <command>` to add new servers

# [servers.filesystem]
# transport = "stdio"
# command   = "npx"
# args      = ["-y", "@modelcontextprotocol/server-filesystem", "/path"]

# [servers.github]
# transport = "http"
# url       = "https://api.githubcopilot.com/mcp/"
# auth      = "oauth"

[options]
tool_search_threshold = 0.10
oauth_port            = 9876
session_ttl_secs      = 3600
TOML
        success "Created $mcp_file"
    fi

    # ── Hooks config ─────────────────────────────────────────────────────────
    local hooks_file="$config_dir/hooks.toml"
    if [ ! -f "$hooks_file" ]; then
        mkdir -p "$config_dir/audit"
        cat > "$hooks_file" << 'TOML'
[[hooks]]
event   = "PreToolUse"
tool    = "bash"
command = """
#!/usr/bin/env bash
input="$(cat)"
if echo "$input" | grep -qE 'rm\s+-rf\s+/|git push.*--force.*main'; then
  echo '{"permissionDecision":"deny","permissionDecisionReason":"Blocked by hook"}'
  exit 2
fi
"""

[[hooks]]
event   = "PostToolUse"
command = """
#!/usr/bin/env bash
echo "[$(date -u +%Y-%m-%dT%H:%M:%SZ)] $(cat | python3 -c 'import sys,json; d=json.load(sys.stdin); print(d.get("tool_name","unknown"))' 2>/dev/null)" \
  >> ~/.halcon/audit/hooks.log 2>/dev/null || true
"""
TOML
        success "Created $hooks_file"
    fi

    # ── Semantic memory directory ─────────────────────────────────────────────
    local memory_dir="$config_dir/memory"
    mkdir -p "$memory_dir"
    if [ ! -f "$memory_dir/MEMORY.md" ]; then
        cat > "$memory_dir/MEMORY.md" << 'MD'
# Halcón — Session Memory Index

> Auto-managed by the semantic memory system.
> Add manual notes here; they will be indexed and retrieved automatically.

## Notes

<!-- project notes, team preferences, recurring context -->
MD
        success "Created $memory_dir/MEMORY.md"
    fi

    # ── Docker detection ──────────────────────────────────────────────────────
    if command -v docker &>/dev/null && docker info &>/dev/null 2>&1; then
        success "Docker detected — enabling docker tool"
        sed -i.bak 's/enable_docker.*=.*false/enable_docker = true/' "$config_file" 2>/dev/null || true
        rm -f "${config_file}.bak"
    fi

    # ── VS Code / Cursor ──────────────────────────────────────────────────────
    local editor_cmd="" editor_name=""
    command -v code   &>/dev/null && { editor_cmd="code";   editor_name="VS Code"; }
    command -v cursor &>/dev/null && { editor_cmd="cursor"; editor_name="Cursor"; }

    if [ -n "$editor_cmd" ]; then
        info "$editor_name detected"
        local do_vscode=true
        is_interactive && { ask "Configure $editor_name integration?" "y" || do_vscode=false; }
        if [ "$do_vscode" = true ]; then
            local vs_dir="$HOME/.config/Code/User"
            [ "$(uname -s)" = "Darwin" ] && vs_dir="$HOME/Library/Application Support/Code/User"
            mkdir -p "$vs_dir"
            cat > "$vs_dir/halcon.settings.json" << VSJSON
{
  "halcon.binaryPath": "${install_dir}/halcon",
  "halcon.defaultProvider": "anthropic",
  "halcon.maxTurns": 50,
  "halcon.jsonRpcMode": true
}
VSJSON
            success "$editor_name settings written to $vs_dir/halcon.settings.json"
        fi
    fi

    # ── Cenzontle SSO ─────────────────────────────────────────────────────────
    if is_interactive; then
        echo ""
        echo -e "  ${BOLD}Cenzontle AI${NC} — enterprise AI platform (Zuclubit SSO)"
        if ask "Log in to Cenzontle AI? (requires Zuclubit SSO account)" "n"; then
            local halcon_bin="$install_dir/halcon"
            if [ -x "$halcon_bin" ]; then
                "$halcon_bin" login cenzontle || warn "SSO login failed — run 'halcon login cenzontle' later"
            fi
        fi
    fi

    echo ""
    success "Frontier tools ready:"
    echo -e "    ${GREEN}✓${NC} Agents   → $agents_dir/"
    echo -e "    ${GREEN}✓${NC} HALCON.md → $halcon_md"
    echo -e "    ${GREEN}✓${NC} Hooks    → $hooks_file"
    echo -e "    ${GREEN}✓${NC} MCP      → $mcp_file"
    echo -e "    ${GREEN}✓${NC} Memory   → $memory_dir/"
    [ -n "$editor_name" ] && echo -e "    ${GREEN}✓${NC} $editor_name  → $vs_dir/halcon.settings.json"
}

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Main Installation Flow
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

main() {
    echo -e "${BOLD}${MAGENTA}"
    cat << 'EOF'
   ╔═══════════════════════════════════════╗
   ║      Halcón CLI - Installation        ║
   ╚═══════════════════════════════════════╝
EOF
    echo -e "${NC}"

    header "Detecting platform"

    local os arch libc target
    os="$(detect_os)"
    arch="$(detect_arch)"
    libc="$(detect_libc "$os")"
    target="$(construct_target "$os" "$arch" "$libc")"

    info "OS:           $os"
    info "Architecture: $arch"
    [ -n "$libc" ] && info "libc:         $libc"
    success "Target:       $target"

    header "Preparing download"

    local archive_ext
    [ "$os" = "windows" ] && archive_ext="zip" || archive_ext="tar.gz"

    info "Fetching latest release version..."
    local version
    version="$(get_latest_version)"
    success "Latest version: $version"

    local archive_name="${BINARY_NAME}-${version}-${target}.${archive_ext}"
    local checksum_name="${archive_name}.sha256"

    info "Asset:    $archive_name"

    # Use a global var so the EXIT trap can reference it (local vars are out of scope in traps)
    _HALCON_TMP="$(mktemp -d)"
    trap 'rm -rf "${_HALCON_TMP:-}"' EXIT
    local tmp_dir="$_HALCON_TMP"

    cd "$tmp_dir"

    header "Downloading binary"

    if ! download_github_asset "$archive_name" "$archive_name"; then
        warn "Failed to download precompiled binary for $target"

        if try_cargo_binstall; then
            exit 0
        elif try_cargo_install; then
            exit 0
        else
            error "All installation methods failed"
        fi
    fi

    success "Downloaded $archive_name"

    header "Verifying integrity"

    if download_github_asset "$checksum_name" "$checksum_name" 2>/dev/null; then
        verify_checksum "$archive_name" "$checksum_name"
    else
        warn "Checksum file not available, skipping verification"
    fi

    header "Extracting archive"

    extract_archive "$archive_name" .

    local binary_path
    if [ -f "${BINARY_NAME}" ]; then
        binary_path="${BINARY_NAME}"
    elif [ -f "${BINARY_NAME}.exe" ]; then
        binary_path="${BINARY_NAME}.exe"
    else
        binary_path="$(find . -name "${BINARY_NAME}" -o -name "${BINARY_NAME}.exe" | head -n1)"
        if [ -z "$binary_path" ]; then
            error "Binary not found in archive"
        fi
    fi

    success "Extracted binary: $binary_path"

    header "Installing"

    install_binary "$binary_path" "$INSTALL_DIR"

    header "Configuring PATH"

    add_to_path "$INSTALL_DIR"

    # macOS: re-sign to satisfy Gatekeeper (exit 137 without signature)
    if [ "$os" = "darwin" ] && command -v codesign &>/dev/null; then
        codesign --force --sign - "$INSTALL_DIR/${BINARY_NAME}" 2>/dev/null || true
        success "macOS Gatekeeper signature applied"
    fi

    header "Verification"

    local installed_binary="$INSTALL_DIR/${BINARY_NAME}"
    if [ -x "$installed_binary" ]; then
        local installed_version
        installed_version="$("$installed_binary" --version 2>&1 || echo "unknown")"
        success "Installation verified: $installed_version"
    else
        error "Binary not executable at $installed_binary"
    fi

    # ── Frontier tools ────────────────────────────────────────────────────────
    setup_frontier_tools "$INSTALL_DIR"

    echo ""
    echo -e "${GREEN}${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${GREEN}${BOLD}   Installation complete!${NC}"
    echo -e "${GREEN}${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo ""
    echo -e "  ${BOLD}Next steps:${NC}"
    echo ""
    echo -e "  ${CYAN}1.${NC} Reload your shell:"
    echo -e "     ${BOLD}source $(detect_shell_profile)${NC}"
    echo ""
    echo -e "  ${CYAN}2.${NC} Set an API key:"
    echo -e "     ${BOLD}export ANTHROPIC_API_KEY=sk-ant-...${NC}"
    echo -e "     ${BOLD}export DEEPSEEK_API_KEY=sk-...${NC}"
    echo ""
    echo -e "  ${CYAN}3.${NC} Start using Halcón:"
    echo -e "     ${BOLD}halcon${NC}                           — interactive REPL"
    echo -e "     ${BOLD}halcon chat \"explain this\"${NC}        — single-shot"
    echo -e "     ${BOLD}halcon agents list${NC}               — frontier sub-agents"
    echo -e "     ${BOLD}halcon mcp list${NC}                  — MCP servers"
    echo -e "     ${BOLD}halcon tools list${NC}                — all 50+ tools"
    echo -e "     ${BOLD}halcon login cenzontle${NC}           — Cenzontle SSO"
    echo ""
    echo -e "  ${BLUE}Config:${NC}   $HOME/.halcon/config.toml"
    echo -e "  ${BLUE}Agents:${NC}   $HOME/.halcon/agents/"
    echo -e "  ${BLUE}Docs:${NC}     https://github.com/${REPO_OWNER}/${REPO_NAME}"
    echo ""
}

main "$@"
