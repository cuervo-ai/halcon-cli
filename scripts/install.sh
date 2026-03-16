#!/usr/bin/env bash
# Halcón CLI — Universal Installer (macOS & Linux)
# Installs: CLI binary, frontier tools config, VS Code extension,
#           desktop app, shell completions, Docker image (optional)
#
# Usage:
#   ./scripts/install.sh                    # interactive
#   HALCON_FULL=1 ./scripts/install.sh      # install everything non-interactive
#   HALCON_MINIMAL=1 ./scripts/install.sh   # CLI only
set -euo pipefail

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Constants & environment overrides
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

readonly INSTALL_DIR="${HALCON_INSTALL_DIR:-$HOME/.local/bin}"
readonly CONFIG_DIR="${HALCON_CONFIG_DIR:-$HOME/.halcon}"
readonly REQUIRED_MSRV="1.80.0"
readonly REPO_URL="https://github.com/cuervo-ai/halcon-cli"

# Opt-in flags (env or interactive)
INSTALL_FULL="${HALCON_FULL:-0}"
INSTALL_MINIMAL="${HALCON_MINIMAL:-0}"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# UI helpers
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
BLUE='\033[0;34m'; MAGENTA='\033[0;35m'; CYAN='\033[0;36m'
BOLD='\033[1m'; DIM='\033[2m'; NC='\033[0m'

info()    { echo -e "${BLUE}${BOLD}[INFO]${NC}  $*"; }
ok()      { echo -e "${GREEN}${BOLD}[ OK ]${NC}  $*"; }
warn()    { echo -e "${YELLOW}${BOLD}[WARN]${NC}  $*"; }
fail()    { echo -e "${RED}${BOLD}[FAIL]${NC}  $*"; exit 1; }
skip()    { echo -e "${DIM}[SKIP]  $*${NC}"; }
section() { echo -e "\n${CYAN}${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n  $*\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"; }

has() { command -v "$1" >/dev/null 2>&1; }
need() { has "$1" || fail "Required tool not found: $1. Please install it and retry."; }

is_interactive() { [ -t 0 ] && [ "${CI:-}" = "" ]; }

ask() {
    local prompt="$1" default="${2:-y}"
    [ "$INSTALL_FULL" = "1" ] && return 0      # full install: always accept
    [ "$INSTALL_MINIMAL" = "1" ] && return 1   # minimal install: always decline
    if ! is_interactive; then [ "$default" = "y" ] && return 0 || return 1; fi
    local hint; [ "$default" = "y" ] && hint="${GREEN}Y${NC}/n" || hint="y/${GREEN}N${NC}"
    echo -en "  ${CYAN}?${NC} ${BOLD}${prompt}${NC} [${hint}] " >&2
    read -r ans; ans="${ans:-$default}"
    case "$ans" in [Yy]*) return 0 ;; *) return 1 ;; esac
}

detect_os() {
    case "$(uname -s)" in
        Linux)  echo "linux" ;;
        Darwin) echo "darwin" ;;
        *)      fail "Unsupported OS: $(uname -s)" ;;
    esac
}

detect_arch() {
    case "$(uname -m)" in
        x86_64|amd64)  echo "x86_64" ;;
        aarch64|arm64) echo "aarch64" ;;
        *)             fail "Unsupported arch: $(uname -m)" ;;
    esac
}

OS="$(detect_os)"
ARCH="$(detect_arch)"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Banner
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

echo -e "${BOLD}${MAGENTA}"
cat << 'BANNER'

  ██╗  ██╗ █████╗ ██╗      ██████╗ ██████╗ ███╗   ██╗
  ██║  ██║██╔══██╗██║     ██╔════╝██╔═══██╗████╗  ██║
  ███████║███████║██║     ██║     ██║   ██║██╔██╗ ██║
  ██╔══██║██╔══██║██║     ██║     ██║   ██║██║╚██╗██║
  ██║  ██║██║  ██║███████╗╚██████╗╚██████╔╝██║ ╚████║
  ╚═╝  ╚═╝╚═╝  ╚═╝╚══════╝ ╚═════╝ ╚═════╝ ╚═╝  ╚═══╝
               CLI + Frontier Tools Installer

BANNER
echo -e "${NC}"
echo -e "  ${DIM}Platform: ${OS}/${ARCH}${NC}"
echo -e "  ${DIM}Install:  ${INSTALL_DIR}${NC}"
echo -e "  ${DIM}Config:   ${CONFIG_DIR}${NC}"
echo ""

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# STEP 1 — Rust toolchain
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

section "1 · Rust toolchain"

if ! has rustc; then
    warn "Rust not found — installing rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --quiet
    # shellcheck source=/dev/null
    source "$HOME/.cargo/env"
fi

RUST_VER="$(rustc --version | cut -d' ' -f2)"
NEED_VER="$REQUIRED_MSRV"
if [ "$(printf '%s\n' "$NEED_VER" "$RUST_VER" | sort -V | head -n1)" != "$NEED_VER" ]; then
    warn "Rust $RUST_VER < $NEED_VER — updating..."
    rustup update stable --quiet
    RUST_VER="$(rustc --version | cut -d' ' -f2)"
fi
ok "Rust $RUST_VER"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# STEP 2 — Source code
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

section "2 · Source code"

_is_repo() { [ -d "$1/crates/halcon-cli" ] && [ -f "$1/Cargo.toml" ]; }

SCRIPT_REPO="$(cd "$(dirname "$0")/.." 2>/dev/null && pwd || echo "")"

if _is_repo "$(pwd)"; then
    ok "Running from halcon-cli repository"
    REPO_DIR="$(pwd)"
elif _is_repo "$SCRIPT_REPO"; then
    ok "Found halcon-cli repository: $SCRIPT_REPO"
    REPO_DIR="$SCRIPT_REPO"
elif [ -d "halcon-cli" ] && _is_repo "$(pwd)/halcon-cli"; then
    cd halcon-cli && git pull origin main --quiet && REPO_DIR="$(pwd)"
    ok "Updated existing clone"
else
    info "Cloning $REPO_URL..."
    git clone "$REPO_URL" --quiet && cd halcon-cli
    REPO_DIR="$(pwd)"
    ok "Cloned to $REPO_DIR"
fi

cd "$REPO_DIR"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# STEP 3 — Build CLI binary
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

section "3 · Build CLI"

info "Building halcon (release)..."
cargo build --release -p halcon-cli --bin halcon --no-default-features 2>&1 | \
    grep -E "^(error|warning\[|Finished|Compiling halcon-cli)" | tail -8 || true

CLI_BIN="$REPO_DIR/target/release/halcon"
[ -f "$CLI_BIN" ] || fail "Build failed — binary not found at $CLI_BIN"
ok "CLI built ($(du -h "$CLI_BIN" | cut -f1))"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# STEP 4 — Install CLI
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

section "4 · Install CLI"

mkdir -p "$INSTALL_DIR"
cp "$CLI_BIN" "$INSTALL_DIR/halcon"
chmod +x "$INSTALL_DIR/halcon"

# macOS: re-sign so Gatekeeper doesn't kill it with exit 137
if [ "$OS" = "darwin" ] && has codesign; then
    codesign --force --sign - "$INSTALL_DIR/halcon" 2>/dev/null && ok "macOS Gatekeeper signature applied"
fi

ok "Installed → $INSTALL_DIR/halcon"

# PATH check
if ! echo "$PATH" | tr ':' '\n' | grep -qx "$INSTALL_DIR"; then
    warn "$INSTALL_DIR not in PATH — add to your shell profile:"
    echo -e "     ${BOLD}export PATH=\"$INSTALL_DIR:\$PATH\"${NC}"
fi

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# STEP 5 — Base configuration
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

section "5 · Base configuration"

mkdir -p "$CONFIG_DIR"
CONFIG_FILE="$CONFIG_DIR/config.toml"

if [ -f "$CONFIG_FILE" ]; then
    ok "Config exists — keeping $CONFIG_FILE"
else
    cat > "$CONFIG_FILE" << 'TOML'
[general]
default_provider = "anthropic"
default_model    = "claude-sonnet-4-6"
max_tokens       = 8192
temperature      = 0.0

[models.providers.anthropic]
enabled       = true
api_key_env   = "ANTHROPIC_API_KEY"
default_model = "claude-sonnet-4-6"

[models.providers.deepseek]
enabled       = true
api_base      = "https://api.deepseek.com"
api_key_env   = "DEEPSEEK_API_KEY"
default_model = "deepseek-chat"

[models.providers.openai]
enabled       = true
api_base      = "https://api.openai.com/v1"
api_key_env   = "OPENAI_API_KEY"
default_model = "gpt-4o-mini"

[models.providers.gemini]
enabled       = true
api_key_env   = "GEMINI_API_KEY"
default_model = "gemini-2.0-flash"

[models.providers.ollama]
enabled       = true
api_base      = "http://localhost:11434"
default_model = "llama3.2"

# Cenzontle — plataforma AI propia de Zuclubit (SSO OAuth 2.1 PKCE)
# Autenticación: halcon login cenzontle  o  export CENZONTLE_ACCESS_TOKEN
[models.providers.cenzontle]
enabled       = false
api_base      = "https://ca-cenzontle-backend.graypond-e35bfdd8.eastus2.azurecontainerapps.io"
api_key_env   = "CENZONTLE_ACCESS_TOKEN"
default_model = "claude-sonnet-4-6"

# ── AWS Bedrock ────────────────────────────────────────────────────────────────
# Activa con: export CLAUDE_CODE_USE_BEDROCK=1
# Requiere:   AWS_ACCESS_KEY_ID + AWS_SECRET_ACCESS_KEY + AWS_REGION
# Opcional:   AWS_SESSION_TOKEN (credenciales temporales / IAM roles)
[models.providers.bedrock]
enabled       = false
region        = "us-east-1"
default_model = "anthropic.claude-sonnet-4-6"
cross_region  = false

# ── Azure AI Foundry ───────────────────────────────────────────────────────────
# Activa con: export CLAUDE_CODE_USE_AZURE=1
# Requiere:   AZURE_AI_ENDPOINT + AZURE_API_KEY
# Alternativa Entra ID (sin API key): AZURE_CLIENT_ID + AZURE_TENANT_ID
[models.providers.azure_foundry]
enabled       = false
endpoint_env  = "AZURE_AI_ENDPOINT"
api_key_env   = "AZURE_API_KEY"
default_model = "claude-sonnet-4-6"
api_version   = "2024-05-01-preview"

# ── Google Vertex AI ──────────────────────────────────────────────────────────
# Activa con: export CLAUDE_CODE_USE_VERTEX=1
# Requiere:   ANTHROPIC_VERTEX_PROJECT_ID + GOOGLE_APPLICATION_CREDENTIALS
# Setup:      gcloud auth application-default login
[models.providers.vertex]
enabled       = false
project_env   = "ANTHROPIC_VERTEX_PROJECT_ID"
region        = "us-east5"
default_model = "claude-sonnet-4-6"

[tools]
confirm_destructive = true
timeout_secs        = 120

[security]
pii_detection  = true
audit_enabled  = true
TOML
    ok "Base config written → $CONFIG_FILE"
fi

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# STEP 6 — Frontier tools
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

section "6 · Frontier tools"

echo ""
echo -e "  ${BOLD}Available frontier capabilities:${NC}"
echo -e "  ${GREEN}●${NC} Agent Registry     — declarative sub-agents with YAML frontmatter"
echo -e "  ${GREEN}●${NC} Semantic Memory    — TF-IDF vector store, MMR retrieval"
echo -e "  ${GREEN}●${NC} MCP Ecosystem      — GitHub, Slack, Linear (OAuth 2.1 PKCE)"
echo -e "  ${GREEN}●${NC} Lifecycle Hooks    — PreToolUse/PostToolUse/Stop event handlers"
echo -e "  ${GREEN}●${NC} Audit & Compliance — SOC2 JSONL/CSV/PDF, HMAC chain verification"
echo -e "  ${GREEN}●${NC} 50+ Tools          — semantic_grep, native_search, docker, sql..."
echo ""

DO_FRONTIER=true
ask "Configure frontier tools?" "y" || DO_FRONTIER=false

if $DO_FRONTIER; then

    # ── Policy config block ──────────────────────────────────────────────────
    if ! grep -q '\[policy\]' "$CONFIG_FILE" 2>/dev/null; then
        cat >> "$CONFIG_FILE" << 'TOML'

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
enable_perf_analyze  = true

[mcp_server]
transport    = "stdio"
port         = 7777
require_auth = false
TOML
        ok "Policy + frontier config added"
    fi

    # ── Context overrides (guard against duplicate) ──────────────────────────
    if ! grep -q '\[context\]' "$CONFIG_FILE" 2>/dev/null; then
        cat >> "$CONFIG_FILE" << 'TOML'

[context]
max_tokens           = 180000
compaction_threshold = 0.80
enable_repo_map      = true
TOML
    else
        # Patch existing [context] with frontier values if not present
        grep -q 'max_tokens' "$CONFIG_FILE" 2>/dev/null || \
            printf '\nmax_tokens           = 180000\n' >> "$CONFIG_FILE"
    fi

    # ── Agent config (guard against duplicate) ───────────────────────────────
    if ! grep -q '^\[agent\]' "$CONFIG_FILE" 2>/dev/null; then
        cat >> "$CONFIG_FILE" << 'TOML'

[agent]
enable_registry        = true
enable_lifecycle_hooks = true
enable_planner         = true
enable_convergence     = true
max_rounds             = 50
TOML
    fi

    # ── Docker auto-detect ───────────────────────────────────────────────────
    if has docker && docker info &>/dev/null 2>&1; then
        sed -i.bak 's/enable_docker.*=.*false/enable_docker = true/' "$CONFIG_FILE" 2>/dev/null \
            && rm -f "${CONFIG_FILE}.bak"
        ok "Docker detected → enable_docker = true"
    fi

    # ── Agent registry ───────────────────────────────────────────────────────
    mkdir -p "$CONFIG_DIR/agents" "$CONFIG_DIR/skills"

    [ -f "$CONFIG_DIR/agents/code-reviewer.md" ] || cat > "$CONFIG_DIR/agents/code-reviewer.md" << 'MD'
---
name: code-reviewer
description: |
  Expert code reviewer. Triggers when asked to review, audit, or assess
  code quality, security vulnerabilities, or architectural decisions.
tools: [Read, Grep, Glob]
model: claude-sonnet-4-6
max_turns: 20
---

You are a senior code reviewer. Focus on correctness, security, and readability.
Always provide specific file:line references and actionable suggestions.
MD

    [ -f "$CONFIG_DIR/agents/test-writer.md" ] || cat > "$CONFIG_DIR/agents/test-writer.md" << 'MD'
---
name: test-writer
description: |
  Writes comprehensive test suites. Triggers when asked to add tests,
  increase coverage, or write unit/integration/e2e tests.
tools: [Read, Grep, Glob, file_write, bash]
model: claude-sonnet-4-6
max_turns: 30
---

Write thorough, non-flaky tests. Prefer integration tests over mocks.
Always verify tests pass before finishing.
MD

    [ -f "$CONFIG_DIR/agents/security-auditor.md" ] || cat > "$CONFIG_DIR/agents/security-auditor.md" << 'MD'
---
name: security-auditor
description: |
  Security specialist. Triggers when asked to audit for vulnerabilities,
  scan for secrets, check OWASP top-10, or assess supply-chain risks.
tools: [Read, Grep, Glob, secret_scan, bash]
model: claude-opus-4-6
max_turns: 25
---

You are a security engineer. Look for: injection flaws, broken auth,
exposed secrets, insecure deps, SSRF, XSS, path traversal.
Report with CVE references where applicable.
MD

    ok "Agent registry → $CONFIG_DIR/agents/ (3 agents)"

    # ── HALCON.md (instruction persistence) ──────────────────────────────────
    [ -f "$CONFIG_DIR/HALCON.md" ] || cat > "$CONFIG_DIR/HALCON.md" << 'MD'
# Halcón — Personal Instructions

<!-- Loaded at the start of every session. Keep under 200 lines for best adherence. -->
<!-- Equivalent to Claude Code's CLAUDE.md — explicit procedural memory. -->

## Preferences

- Concise, direct answers over lengthy explanations
- File paths with line numbers when referencing code
- Use audit trail for destructive operations

## Conventions

<!-- Project-specific rules. Examples:
- Language: Rust (edition 2021), test runner: cargo nextest
- Commit style: Conventional Commits (feat/fix/docs/refactor)
- No force-push to main
-->

## Memory

<!-- Semantic memory index: ~/.halcon/memory/MEMORY.md -->
<!-- Use `search_memory` tool to retrieve past session context -->
MD
    ok "HALCON.md → $CONFIG_DIR/HALCON.md"

    # ── Hooks ────────────────────────────────────────────────────────────────
    mkdir -p "$CONFIG_DIR/audit"
    [ -f "$CONFIG_DIR/hooks.toml" ] || cat > "$CONFIG_DIR/hooks.toml" << 'TOML'
# Halcón lifecycle hooks — https://github.com/cuervo-ai/halcon-cli#hooks

# Block catastrophic bash commands before execution
[[hooks]]
event   = "PreToolUse"
tool    = "bash"
command = """
#!/usr/bin/env bash
input="$(cat)"
if echo "$input" | grep -qE 'rm\s+-rf\s+[/~]|git push.*--force.*(main|master)'; then
  echo '{"permissionDecision":"deny","permissionDecisionReason":"Blocked by PreToolUse hook: catastrophic pattern detected"}'
  exit 2
fi
"""

# Audit log every tool call
[[hooks]]
event   = "PostToolUse"
command = """
#!/usr/bin/env bash
ts="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
tool="$(cat | python3 -c 'import sys,json; d=json.load(sys.stdin); print(d.get("tool_name","unknown"))' 2>/dev/null || echo unknown)"
echo "[$ts] $tool" >> ~/.halcon/audit/hooks.log 2>/dev/null || true
"""
TOML
    ok "Hooks → $CONFIG_DIR/hooks.toml"

    # ── MCP config ───────────────────────────────────────────────────────────
    [ -f "$CONFIG_DIR/mcp.toml" ] || cat > "$CONFIG_DIR/mcp.toml" << 'TOML'
# Halcón MCP — Model Context Protocol server configuration
# Run: halcon mcp list        (see configured servers)
# Run: halcon mcp add <name> -- <command>  (add new server)
# Run: halcon mcp serve       (expose halcon itself as MCP server)

# ── Stdio (local process) servers ────────────────────────────────────────────

# Filesystem access (sandboxed to a path)
# [servers.filesystem]
# transport = "stdio"
# command   = "npx"
# args      = ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/workspace"]

# Git repository tools
# [servers.git]
# transport = "stdio"
# command   = "uvx"
# args      = ["mcp-server-git", "--repository", "."]

# SQLite database
# [servers.sqlite]
# transport = "stdio"
# command   = "uvx"
# args      = ["mcp-server-sqlite", "--db-path", "/path/to/db.sqlite"]

# ── HTTP (remote, OAuth 2.1) servers ─────────────────────────────────────────

# GitHub (requires GITHUB_TOKEN or SSO)
# [servers.github]
# transport = "http"
# url       = "https://api.githubcopilot.com/mcp/"
# auth      = "oauth"

# Slack
# [servers.slack]
# transport = "http"
# url       = "https://mcp.slack.com/api/mcp"
# auth      = "oauth"

# Linear
# [servers.linear]
# transport = "http"
# url       = "https://mcp.linear.app/sse"
# auth      = "oauth"

[options]
tool_search_threshold = 0.10    # context fraction before deferred tool search activates
oauth_port            = 9876    # loopback port for OAuth 2.1 PKCE callbacks
session_ttl_secs      = 3600    # HTTP session time-to-live
TOML
    ok "MCP config → $CONFIG_DIR/mcp.toml"

    # ── Semantic memory ───────────────────────────────────────────────────────
    mkdir -p "$CONFIG_DIR/memory"
    [ -f "$CONFIG_DIR/memory/MEMORY.md" ] || cat > "$CONFIG_DIR/memory/MEMORY.md" << 'MD'
# Halcón — Semantic Memory Index

> Auto-managed by the vector store. Add notes below; they are indexed automatically.
> Use `search_memory "query"` in any session to retrieve relevant context.

## Project Notes

<!-- Add project-specific facts, decisions, recurring context -->

## Team Conventions

<!-- Coding standards, PR guidelines, deployment notes -->
MD
    ok "Semantic memory → $CONFIG_DIR/memory/"

    # ── Cenzontle SSO (interactive only) ─────────────────────────────────────
    if is_interactive; then
        echo ""
        echo -e "  ${BOLD}Cenzontle AI${NC} — enterprise platform (Zuclubit OAuth 2.1 PKCE)"
        if ask "Log in to Cenzontle AI now? (browser will open)" "n"; then
            "$INSTALL_DIR/halcon" login cenzontle \
                || warn "SSO login failed — run 'halcon login cenzontle' later"
        fi
    fi

    ok "Frontier tools configured"
fi  # DO_FRONTIER

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# STEP 7 — Shell completions
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

section "7 · Shell completions"

echo ""
if ! ask "Install shell completions (bash/zsh/fish)?" "y"; then
    skip "Shell completions"
else

COMP_DIR="$CONFIG_DIR/completions"
mkdir -p "$COMP_DIR"

# ── zsh completion ───────────────────────────────────────────────────────────
cat > "$COMP_DIR/_halcon" << 'ZSH'
#compdef halcon

_halcon() {
  local state

  _arguments \
    '(-h --help)'{-h,--help}'[Show help]' \
    '(-V --version)'{-V,--version}'[Show version]' \
    '--provider[Provider to use]:provider:(anthropic deepseek openai gemini ollama cenzontle bedrock vertex azure)' \
    '--model[Model to use]:model:' \
    '--air-gap[Enable air-gap mode (no network calls)]' \
    '--no-banner[Suppress startup banner]' \
    '1: :_halcon_commands' \
    '*::args:->args'

  case $state in
    args)
      case $words[1] in
        chat)       _arguments '--provider:provider:(anthropic deepseek openai)' '*:message:' ;;
        agents)     _arguments '1:action:(list validate)' ;;
        audit)      _arguments '1:action:(list export verify)' '--session:session_id:' '--format:format:(jsonl csv pdf)' ;;
        mcp)        _arguments '1:action:(list add remove get auth serve)' ;;
        tools)      _arguments '1:action:(list describe)' ;;
        login)      _arguments '1:provider:(cenzontle)' ;;
        logout)     _arguments '1:provider:(cenzontle)' ;;
        serve)      _arguments '--port:port:' '--host:host:' ;;
        update)     ;;
        doctor)     ;;
        status)     ;;
        memory)     _arguments '1:action:(list search clear)' ;;
        schedule)   _arguments '1:action:(list add remove run)' ;;
        trace)      _arguments '1:session:' ;;
        plugin)     _arguments '1:action:(list install remove)' ;;
        users)      _arguments '1:action:(list add remove)' ;;
      esac
  esac
}

_halcon_commands() {
  local commands=(
    'chat:Start a chat session'
    'agents:Manage declarative sub-agents'
    'audit:Audit trail export and verification'
    'mcp:Model Context Protocol server management'
    'tools:Browse available tools'
    'login:Authenticate with a provider (SSO)'
    'logout:Remove stored credentials'
    'status:Show provider and configuration status'
    'doctor:Run diagnostics'
    'serve:Start Halcon API server'
    'update:Update the Halcon binary'
    'memory:Manage semantic memory'
    'schedule:Manage scheduled agent tasks'
    'trace:Inspect execution traces'
    'plugin:Manage plugins'
    'users:Manage users (requires API server)'
    'metrics:Show runtime metrics'
  )
  _describe 'halcon commands' commands
}

_halcon
ZSH

# Install zsh completion
if has zsh; then
    ZSH_COMP_DIRS=("$HOME/.zsh/completions" "$HOME/.local/share/zsh/site-functions" "/usr/local/share/zsh/site-functions")
    ZSH_INSTALLED=false
    for d in "${ZSH_COMP_DIRS[@]}"; do
        if mkdir -p "$d" 2>/dev/null && [ -w "$d" ]; then
            cp "$COMP_DIR/_halcon" "$d/_halcon"
            ok "zsh completion → $d/_halcon"
            ZSH_INSTALLED=true
            # Add fpath to zshrc if needed
            ZSHRC="${ZDOTDIR:-$HOME}/.zshrc"
            if [ -f "$ZSHRC" ] && ! grep -q "fpath.*$d" "$ZSHRC" 2>/dev/null; then
                echo "" >> "$ZSHRC"
                echo "# Halcón completions" >> "$ZSHRC"
                echo "fpath=(\"$d\" \$fpath)" >> "$ZSHRC"
                echo "autoload -Uz compinit && compinit" >> "$ZSHRC"
            fi
            break
        fi
    done
    $ZSH_INSTALLED || warn "Could not write to zsh completions dir — copy $COMP_DIR/_halcon manually"
fi

# ── bash completion ──────────────────────────────────────────────────────────
cat > "$COMP_DIR/halcon.bash" << 'BASH'
_halcon_completions() {
  local cur prev words cword
  _init_completion 2>/dev/null || { COMPREPLY=(); return; }

  local commands="chat agents audit mcp tools login logout status doctor serve update memory schedule trace plugin users metrics"
  local providers="anthropic deepseek openai gemini ollama cenzontle bedrock vertex azure"

  if [ "$cword" -eq 1 ]; then
    COMPREPLY=($(compgen -W "$commands" -- "$cur"))
    return
  fi

  case "${words[1]}" in
    chat)
      case "$prev" in
        --provider|-p) COMPREPLY=($(compgen -W "$providers" -- "$cur")) ;;
        *)             COMPREPLY=($(compgen -W "--provider --model --no-banner" -- "$cur")) ;;
      esac ;;
    agents)   COMPREPLY=($(compgen -W "list validate" -- "$cur")) ;;
    audit)    COMPREPLY=($(compgen -W "list export verify" -- "$cur")) ;;
    mcp)      COMPREPLY=($(compgen -W "list add remove get auth serve" -- "$cur")) ;;
    tools)    COMPREPLY=($(compgen -W "list describe" -- "$cur")) ;;
    login|logout) COMPREPLY=($(compgen -W "cenzontle" -- "$cur")) ;;
    memory)   COMPREPLY=($(compgen -W "list search clear" -- "$cur")) ;;
    schedule) COMPREPLY=($(compgen -W "list add remove run" -- "$cur")) ;;
    plugin)   COMPREPLY=($(compgen -W "list install remove" -- "$cur")) ;;
    users)    COMPREPLY=($(compgen -W "list add remove" -- "$cur")) ;;
  esac
}
complete -F _halcon_completions halcon
BASH

if has bash; then
    BASH_COMP_DIRS=("$HOME/.local/share/bash-completion/completions" "/usr/local/etc/bash_completion.d" "/etc/bash_completion.d")
    BASH_INSTALLED=false
    for d in "${BASH_COMP_DIRS[@]}"; do
        if mkdir -p "$d" 2>/dev/null && [ -w "$d" ]; then
            cp "$COMP_DIR/halcon.bash" "$d/halcon"
            ok "bash completion → $d/halcon"
            BASH_INSTALLED=true
            break
        fi
    done
    if ! $BASH_INSTALLED; then
        # Fallback: source from ~/.bashrc
        BASHRC="$HOME/.bashrc"
        touch "$BASHRC"
        if ! grep -q "halcon.bash" "$BASHRC" 2>/dev/null; then
            echo "" >> "$BASHRC"
            echo "# Halcón completions" >> "$BASHRC"
            echo "source \"$COMP_DIR/halcon.bash\" 2>/dev/null || true" >> "$BASHRC"
        fi
        ok "bash completion → sourced from $BASHRC"
    fi
fi

# ── fish completion ──────────────────────────────────────────────────────────
cat > "$COMP_DIR/halcon.fish" << 'FISH'
set -l commands chat agents audit mcp tools login logout status doctor serve update memory schedule trace plugin users metrics
set -l providers anthropic deepseek openai gemini ollama cenzontle bedrock vertex azure

complete -c halcon -f
complete -c halcon -n "not __fish_seen_subcommand_from $commands" -a "$commands"
complete -c halcon -l provider -s p -a "$providers" -d "AI provider"
complete -c halcon -l model -s m -d "Model name"
complete -c halcon -l air-gap -d "Disable all network calls"
complete -c halcon -l no-banner -d "Suppress startup banner"
complete -c halcon -s h -l help -d "Show help"
complete -c halcon -s V -l version -d "Show version"

complete -c halcon -n "__fish_seen_subcommand_from agents"   -a "list validate"
complete -c halcon -n "__fish_seen_subcommand_from audit"    -a "list export verify"
complete -c halcon -n "__fish_seen_subcommand_from mcp"      -a "list add remove get auth serve"
complete -c halcon -n "__fish_seen_subcommand_from tools"    -a "list describe"
complete -c halcon -n "__fish_seen_subcommand_from login"    -a "cenzontle"
complete -c halcon -n "__fish_seen_subcommand_from logout"   -a "cenzontle"
complete -c halcon -n "__fish_seen_subcommand_from memory"   -a "list search clear"
complete -c halcon -n "__fish_seen_subcommand_from schedule" -a "list add remove run"
complete -c halcon -n "__fish_seen_subcommand_from plugin"   -a "list install remove"
complete -c halcon -n "__fish_seen_subcommand_from users"    -a "list add remove"
FISH

if has fish; then
    FISH_COMP="$HOME/.config/fish/completions"
    mkdir -p "$FISH_COMP"
    cp "$COMP_DIR/halcon.fish" "$FISH_COMP/halcon.fish"
    ok "fish completion → $FISH_COMP/halcon.fish"
fi

ok "Completions written → $COMP_DIR/"

fi  # install completions

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# STEP 8 — VS Code / Cursor extension
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

section "8 · VS Code extension"

EDITOR_CMD=""; EDITOR_NAME=""
has code   && { EDITOR_CMD="code";   EDITOR_NAME="VS Code"; }
has cursor && { EDITOR_CMD="cursor"; EDITOR_NAME="Cursor"; }

if [ -z "$EDITOR_CMD" ]; then
    skip "VS Code / Cursor not found"
elif ! has node || ! has npm; then
    skip "Node.js not found — cannot build extension (install Node 18+ to enable)"
else
    echo ""
    echo -e "  ${BOLD}$EDITOR_NAME extension${NC} adds:"
    echo -e "  ${GREEN}●${NC} Cmd+Shift+H  — open Halcón AI panel"
    echo -e "  ${GREEN}●${NC} Cmd+Shift+A  — ask about selected code"
    echo -e "  ${GREEN}●${NC} Inline diff proposals"
    echo -e "  ${GREEN}●${NC} JSON-RPC subprocess bridge (streaming)"
    echo ""

    DO_VSCODE=true
    ask "Build & install $EDITOR_NAME extension?" "y" || DO_VSCODE=false

    if $DO_VSCODE; then
        EXT_DIR="$REPO_DIR/halcon-vscode"
        if [ ! -d "$EXT_DIR" ]; then
            warn "halcon-vscode/ not found in $REPO_DIR"
        else
            info "Installing npm dependencies..."
            (cd "$EXT_DIR" && npm ci --silent 2>&1 | tail -3) || \
                (cd "$EXT_DIR" && npm install --silent 2>&1 | tail -3)

            info "Bundling extension..."
            (cd "$EXT_DIR" && npm run bundle 2>&1 | tail -5)

            info "Packaging .vsix..."
            # Install vsce locally if not available globally
            if ! has vsce && ! has "@vscode/vsce"; then
                (cd "$EXT_DIR" && npm install --save-dev @vscode/vsce --silent 2>&1 | tail -2)
            fi
            (cd "$EXT_DIR" && npx @vscode/vsce package --pre-release --no-dependencies 2>&1 | tail -3) || \
                (cd "$EXT_DIR" && npx vsce package --pre-release --no-dependencies 2>&1 | tail -3)

            # Find the .vsix file
            VSIX_FILE="$(find "$EXT_DIR" -name "*.vsix" -newer "$EXT_DIR/package.json" 2>/dev/null | head -1)"
            if [ -z "$VSIX_FILE" ]; then
                VSIX_FILE="$(find "$EXT_DIR" -name "*.vsix" 2>/dev/null | sort -t- -k3 -V | tail -1)"
            fi

            if [ -n "$VSIX_FILE" ] && [ -f "$VSIX_FILE" ]; then
                info "Installing $VSIX_FILE into $EDITOR_NAME..."
                "$EDITOR_CMD" --install-extension "$VSIX_FILE" \
                    --force 2>&1 | grep -v "^$" | tail -3 \
                    && ok "$EDITOR_NAME extension installed ($(basename "$VSIX_FILE"))" \
                    || warn "Extension install returned non-zero — open $EDITOR_NAME and install $VSIX_FILE manually"

                # Write halcon.binaryPath into VS Code settings
                if [ "$OS" = "darwin" ]; then
                    VS_SETTINGS_DIR="$HOME/Library/Application Support/Code/User"
                    [ "$EDITOR_CMD" = "cursor" ] && VS_SETTINGS_DIR="$HOME/Library/Application Support/Cursor/User"
                else
                    VS_SETTINGS_DIR="$HOME/.config/Code/User"
                    [ "$EDITOR_CMD" = "cursor" ] && VS_SETTINGS_DIR="$HOME/.config/Cursor/User"
                fi
                mkdir -p "$VS_SETTINGS_DIR"
                SETTINGS_FILE="$VS_SETTINGS_DIR/settings.json"

                python3 << PYEOF
import json, os, sys
path = "$SETTINGS_FILE"
try:
    s = json.load(open(path)) if os.path.exists(path) else {}
except Exception:
    s = {}
s.update({
    "halcon.binaryPath": "$INSTALL_DIR/halcon",
    "halcon.maxTurns": 50,
    "halcon.provider": "anthropic",
})
json.dump(s, open(path, "w"), indent=2)
print("  Settings merged into " + path)
PYEOF
                ok "halcon.binaryPath configured in $EDITOR_NAME settings"
            else
                warn ".vsix file not found — extension may need manual install"
            fi
        fi
    fi
fi

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# STEP 9 — Desktop app (halcon-desktop / egui)
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

section "9 · Desktop app"

echo ""
echo -e "  ${BOLD}halcon-desktop${NC} — native egui control plane:"
echo -e "  ${GREEN}●${NC} Chat panel with streaming"
echo -e "  ${GREEN}●${NC} Agent dashboard (DAG viewer, activity log)"
echo -e "  ${GREEN}●${NC} Metrics & observability charts"
echo -e "  ${GREEN}●${NC} File browser, protocols, settings"
echo -e "  ${DIM}  ~5-10 min build time${NC}"
echo ""

DO_DESKTOP=false
ask "Build & install halcon-desktop? (adds ~10 min build time)" "n" && DO_DESKTOP=true

if $DO_DESKTOP; then
    info "Building halcon-desktop (release)..."
    cargo build --release -p halcon-desktop 2>&1 | \
        grep -E "^(error|Finished|Compiling halcon-desktop)" | tail -5 || true

    DESKTOP_BIN="$REPO_DIR/target/release/halcon-desktop"
    if [ -f "$DESKTOP_BIN" ]; then
        cp "$DESKTOP_BIN" "$INSTALL_DIR/halcon-desktop"
        chmod +x "$INSTALL_DIR/halcon-desktop"

        # macOS: re-sign
        [ "$OS" = "darwin" ] && has codesign && \
            codesign --force --sign - "$INSTALL_DIR/halcon-desktop" 2>/dev/null

        # Linux: create .desktop entry for app launchers
        if [ "$OS" = "linux" ]; then
            DESKTOP_ENTRY_DIR="$HOME/.local/share/applications"
            mkdir -p "$DESKTOP_ENTRY_DIR"
            cat > "$DESKTOP_ENTRY_DIR/halcon-desktop.desktop" << DESKTOP
[Desktop Entry]
Version=1.0
Type=Application
Name=Halcón Desktop
Comment=Native control plane for the Halcon AI agent runtime
Exec=$INSTALL_DIR/halcon-desktop
Icon=utilities-terminal
Terminal=false
Categories=Development;IDE;
Keywords=ai;agent;llm;halcon;
StartupNotify=true
DESKTOP
            ok "Linux .desktop entry → $DESKTOP_ENTRY_DIR/halcon-desktop.desktop"
            has update-desktop-database && update-desktop-database "$DESKTOP_ENTRY_DIR" 2>/dev/null || true
        fi

        # macOS: create .app bundle wrapper
        if [ "$OS" = "darwin" ]; then
            APP_BUNDLE="$HOME/Applications/Halcon Desktop.app"
            mkdir -p "$APP_BUNDLE/Contents/MacOS"
            cp "$INSTALL_DIR/halcon-desktop" "$APP_BUNDLE/Contents/MacOS/halcon-desktop"
            cat > "$APP_BUNDLE/Contents/Info.plist" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key><string>halcon-desktop</string>
    <key>CFBundleIdentifier</key><string>ai.cuervo.halcon-desktop</string>
    <key>CFBundleName</key><string>Halcon Desktop</string>
    <key>CFBundleDisplayName</key><string>Halcón Desktop</string>
    <key>CFBundleVersion</key><string>0.3.0</string>
    <key>CFBundleShortVersionString</key><string>0.3.0</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>LSMinimumSystemVersion</key><string>12.0</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
            codesign --force --sign - "$APP_BUNDLE" 2>/dev/null || true
            ok "macOS app bundle → $APP_BUNDLE"
        fi

        ok "halcon-desktop installed → $INSTALL_DIR/halcon-desktop"
    else
        warn "halcon-desktop build failed — check cargo output above"
    fi
else
    skip "Desktop app (run $INSTALL_DIR/halcon-desktop after manual build)"
fi

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# STEP 10 — Docker image
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

section "10 · Docker"

if ! has docker; then
    skip "Docker not found — install Docker Desktop to enable"
elif ! docker info &>/dev/null 2>&1; then
    skip "Docker daemon not running"
else
    echo ""
    echo -e "  ${BOLD}Docker image${NC} (halcon-cli:latest):"
    echo -e "  ${GREEN}●${NC} Distroless runtime (minimal attack surface)"
    echo -e "  ${GREEN}●${NC} Non-root execution"
    echo -e "  ${GREEN}●${NC} Read-only filesystem"
    echo ""

    DO_DOCKER=false
    ask "Build halcon-cli Docker image?" "n" && DO_DOCKER=true

    if $DO_DOCKER; then
        DOCKERFILE="$REPO_DIR/scripts/docker/Dockerfile"
        if [ ! -f "$DOCKERFILE" ]; then
            warn "Dockerfile not found at $DOCKERFILE"
        else
            info "Building Docker image (halcon-cli:latest)..."
            docker build \
                --build-arg VERSION="$(git -C "$REPO_DIR" describe --tags 2>/dev/null || echo dev)" \
                --build-arg VCS_REF="$(git -C "$REPO_DIR" rev-parse HEAD 2>/dev/null || echo unknown)" \
                --build-arg BUILD_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
                --tag halcon-cli:latest \
                --file "$DOCKERFILE" \
                "$REPO_DIR" 2>&1 | tail -8

            if docker image inspect halcon-cli:latest &>/dev/null 2>&1; then
                IMAGE_SIZE="$(docker image inspect halcon-cli:latest --format '{{.Size}}' | \
                    awk '{ printf "%.1f MB", $1/1024/1024 }')"
                ok "Docker image halcon-cli:latest ($IMAGE_SIZE)"
                info "Usage: docker run --rm -v \$(pwd):/workspace halcon-cli:latest"
            else
                warn "Docker build may have failed"
            fi
        fi
    else
        skip "Docker image"
    fi
fi

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# STEP 11 — Homebrew tap (macOS)
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

if [ "$OS" = "darwin" ] && has brew; then
    section "11 · Homebrew tap"
    if brew tap cuervo-ai/homebrew-tap &>/dev/null 2>&1; then
        ok "Homebrew tap: cuervo-ai/homebrew-tap (users can now: brew install halcon)"
    else
        skip "Homebrew tap not available (will be ready at next release)"
    fi
fi

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# FINAL — Verify & summary
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

section "✓ Verification"

HALCON="$INSTALL_DIR/halcon"
[ -x "$HALCON" ] || fail "Binary not executable at $HALCON"

VER="$("$HALCON" --version 2>&1 || echo 'error')"
ok "halcon --version: $VER"

HELP_EXIT=0
"$HALCON" --help >/dev/null 2>&1 || HELP_EXIT=$?
[ "$HELP_EXIT" -eq 0 ] && ok "halcon --help: exit 0" || warn "halcon --help exit $HELP_EXIT (may need API key)"

AGENT_COUNT="$(find "$CONFIG_DIR/agents" -name "*.md" 2>/dev/null | wc -l | tr -d ' ')"
ok "Agent registry: $AGENT_COUNT agent(s)"
ok "Config: $CONFIG_FILE"

echo ""
echo -e "${GREEN}${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${GREEN}${BOLD}   Halcón CLI installed successfully${NC}"
echo -e "${GREEN}${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo -e "  ${BOLD}Quick start:${NC}"
echo ""
echo -e "  ${CYAN}1.${NC} Connect a provider:"
echo ""
echo -e "     ${DIM}— Cloud APIs (API key) ────────────────────────────────────────────────────${NC}"
echo -e "     ${BOLD}export ANTHROPIC_API_KEY=sk-ant-...${NC}    ${DIM}# Claude — recommended${NC}"
echo -e "     ${BOLD}export OPENAI_API_KEY=sk-...${NC}           ${DIM}# GPT models${NC}"
echo -e "     ${BOLD}export DEEPSEEK_API_KEY=sk-...${NC}         ${DIM}# cheapest option${NC}"
echo -e "     ${BOLD}export GEMINI_API_KEY=AI...${NC}            ${DIM}# Google Gemini${NC}"
echo ""
echo -e "     ${DIM}— Enterprise / Cloud Infrastructure ──────────────────────────────────────${NC}"
echo -e "     ${BOLD}halcon login cenzontle${NC}                  ${DIM}# Cenzontle SSO (Zuclubit)${NC}"
echo -e "     ${BOLD}export CLAUDE_CODE_USE_BEDROCK=1${NC}       ${DIM}# AWS Bedrock${NC}"
echo -e "       ${DIM}→ also set: AWS_ACCESS_KEY_ID  AWS_SECRET_ACCESS_KEY  AWS_REGION${NC}"
echo -e "     ${BOLD}export CLAUDE_CODE_USE_AZURE=1${NC}         ${DIM}# Azure AI Foundry${NC}"
echo -e "       ${DIM}→ also set: AZURE_AI_ENDPOINT  AZURE_API_KEY${NC}"
echo -e "     ${BOLD}export CLAUDE_CODE_USE_VERTEX=1${NC}        ${DIM}# Google Vertex AI${NC}"
echo -e "       ${DIM}→ also set: ANTHROPIC_VERTEX_PROJECT_ID  (+ gcloud ADC)${NC}"
echo ""
echo -e "     ${DIM}— Local / Air-gap ─────────────────────────────────────────────────────────${NC}"
echo -e "     ${BOLD}halcon chat -p ollama${NC}                   ${DIM}# Ollama — fully local, no API key${NC}"
echo ""
echo -e "  ${CYAN}2.${NC} Reload your shell:  ${BOLD}source ~/.zshrc${NC}  or  ${BOLD}source ~/.bashrc${NC}"
echo ""
echo -e "  ${CYAN}3.${NC} Start using Halcón:"
echo -e "     ${BOLD}halcon${NC}                     — interactive REPL"
echo -e "     ${BOLD}halcon chat \"explain this\"${NC}  — single shot"
echo -e "     ${BOLD}halcon agents list${NC}         — sub-agents"
echo -e "     ${BOLD}halcon mcp list${NC}            — MCP servers"
echo -e "     ${BOLD}halcon tools list${NC}          — all 50+ tools"
echo -e "     ${BOLD}halcon audit list${NC}          — compliance export"
echo -e "     ${BOLD}halcon login cenzontle${NC}     — enterprise SSO"
echo -e "     ${BOLD}halcon mcp serve${NC}           — expose como servidor MCP"
echo -e "     ${BOLD}halcon doctor${NC}              — diagnóstico de proveedores"
[ -f "$INSTALL_DIR/halcon-desktop" ] && \
echo -e "     ${BOLD}halcon-desktop${NC}             — native GUI"
echo ""
echo -e "  ${BLUE}Config:${NC}  $CONFIG_FILE"
echo -e "  ${BLUE}Agents:${NC}  $CONFIG_DIR/agents/"
echo -e "  ${BLUE}Memory:${NC}  $CONFIG_DIR/memory/"
echo -e "  ${BLUE}Hooks:${NC}   $CONFIG_DIR/hooks.toml"
echo -e "  ${BLUE}MCP:${NC}     $CONFIG_DIR/mcp.toml"
echo -e "  ${BLUE}Docs:${NC}    $REPO_URL"
echo ""
