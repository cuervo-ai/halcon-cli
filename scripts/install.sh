#!/usr/bin/env bash
# Halcón CLI — Installation Script
# Usage: ./scripts/install.sh
# Installs halcon to ~/.local/bin/ (no sudo required)
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
MAGENTA='\033[0;35m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

INSTALL_DIR="${HALCON_INSTALL_DIR:-$HOME/.local/bin}"
REQUIRED_MSRV="1.80.0"
REPO_URL="https://github.com/cuervo-ai/halcon-cli"

info()    { echo -e "${BLUE}[INFO]${NC}  $*"; }
ok()      { echo -e "${GREEN}[OK]${NC}    $*"; }
warn()    { echo -e "${YELLOW}[WARN]${NC}  $*"; }
fail()    { echo -e "${RED}[FAIL]${NC}  $*"; exit 1; }
section() { echo -e "\n${CYAN}${BOLD}━━━ $* ━━━${NC}"; }

# Is the script running interactively (TTY)? curl | sh sets up a non-interactive session.
is_interactive() { [ -t 0 ]; }

# Prompt helper — returns 0 (yes) or 1 (no). Non-interactive always returns the default.
# Usage: ask "Enable X?" "y"
ask() {
    local prompt="$1"
    local default="${2:-y}"
    if ! is_interactive; then
        [ "$default" = "y" ] && return 0 || return 1
    fi
    local yn_hint
    [ "$default" = "y" ] && yn_hint="[Y/n]" || yn_hint="[y/N]"
    read -r -p "  ${BOLD}${CYAN}?${NC} ${prompt} ${yn_hint} " answer
    answer="${answer:-$default}"
    case "$answer" in
        [Yy]*) return 0 ;;
        *)     return 1 ;;
    esac
}

echo -e "${BOLD}${MAGENTA}"
echo "  ╔═══════════════════════════════════════════════╗"
echo "  ║        Halcón CLI — Installation              ║"
echo "  ╚═══════════════════════════════════════════════╝"
echo -e "${NC}"

# ── Step 1: Verify Rust ──────────────────────────────────────────────────────
section "[1/7] Rust toolchain"

if ! command -v rustc &>/dev/null; then
    warn "Rust not found. Installing via rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    # shellcheck source=/dev/null
    source "$HOME/.cargo/env"
fi

RUST_VERSION=$(rustc --version | cut -d' ' -f2)
if [ "$(printf '%s\n' "$REQUIRED_MSRV" "$RUST_VERSION" | sort -V | head -n1)" = "$REQUIRED_MSRV" ]; then
    ok "Rust $RUST_VERSION (>= $REQUIRED_MSRV)"
else
    warn "Rust $RUST_VERSION < $REQUIRED_MSRV required. Updating..."
    rustup update stable
    RUST_VERSION=$(rustc --version | cut -d' ' -f2)
    ok "Rust updated to $RUST_VERSION"
fi

# ── Step 2: Get source code ──────────────────────────────────────────────────
section "[2/7] Source code"

if [ -f "Cargo.toml" ] && grep -q 'name = "halcon"' Cargo.toml 2>/dev/null; then
    ok "Already in halcon-cli repository"
    REPO_DIR="$(pwd)"
elif [ -f "../Cargo.toml" ] && grep -q 'name = "halcon"' ../Cargo.toml 2>/dev/null; then
    ok "Found halcon-cli repository in parent directory"
    REPO_DIR="$(cd .. && pwd)"
elif [ -d "halcon-cli" ]; then
    info "Updating existing clone..."
    cd halcon-cli
    git pull origin main
    REPO_DIR="$(pwd)"
else
    info "Cloning from $REPO_URL..."
    git clone "$REPO_URL"
    cd halcon-cli
    REPO_DIR="$(pwd)"
fi

# ── Step 3: Build ─────────────────────────────────────────────────────────────
section "[3/7] Build"

cd "$REPO_DIR"
cargo build --release --no-default-features 2>&1 | tail -5

BINARY="$REPO_DIR/target/release/halcon"
if [ ! -f "$BINARY" ]; then
    fail "Build failed — binary not found at $BINARY"
fi

BINARY_SIZE=$(du -h "$BINARY" | cut -f1)
ok "Build complete — $BINARY_SIZE"

# ── Step 4: Install ──────────────────────────────────────────────────────────
section "[4/7] Install"

mkdir -p "$INSTALL_DIR"
cp "$BINARY" "$INSTALL_DIR/halcon"
chmod +x "$INSTALL_DIR/halcon"
ok "Installed to $INSTALL_DIR/halcon"

# macOS: re-sign to satisfy Gatekeeper (SIP kills unsigned copies, exit 137)
if [ "$(uname -s)" = "Darwin" ] && command -v codesign &>/dev/null; then
    codesign --force --sign - "$INSTALL_DIR/halcon" 2>/dev/null || true
    ok "macOS Gatekeeper signature applied"
fi

# Ensure install dir is in PATH
if ! echo "$PATH" | tr ':' '\n' | grep -qx "$INSTALL_DIR"; then
    warn "$INSTALL_DIR is not in your PATH."
    echo ""
    echo "  Add this to your shell profile (~/.zshrc or ~/.bashrc):"
    echo ""
    echo "    export PATH=\"$INSTALL_DIR:\$PATH\""
    echo ""
fi

# ── Step 5: Base Configuration ────────────────────────────────────────────────
section "[5/7] Base configuration"

CONFIG_DIR="$HOME/.halcon"
CONFIG_FILE="$CONFIG_DIR/config.toml"
mkdir -p "$CONFIG_DIR"

if [ -f "$CONFIG_FILE" ]; then
    ok "Configuration exists at $CONFIG_FILE (keeping existing)"
else
    info "Creating base configuration..."
    cat > "$CONFIG_FILE" << 'TOML'
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

[models.providers.ollama]
enabled = true
api_base = "http://localhost:11434"
default_model = "llama3.2"

[models.providers.gemini]
enabled = true
api_key_env = "GEMINI_API_KEY"
default_model = "gemini-1.5-pro"

[tools]
confirm_destructive = true
timeout_secs = 120

[security]
pii_detection = true
audit_enabled = true
TOML
    ok "Base configuration written to $CONFIG_FILE"
fi

# ── Step 6: Frontier Tools Setup ─────────────────────────────────────────────
section "[6/7] Frontier tools"

echo ""
echo -e "  Halcón includes a suite of ${BOLD}frontier capabilities${NC} beyond basic chat:"
echo -e "    • ${CYAN}Agent Registry${NC}     — declarative sub-agents (YAML frontmatter .md files)"
echo -e "    • ${CYAN}Semantic Memory${NC}    — vector search over session memory (TF-IDF, MMR)"
echo -e "    • ${CYAN}MCP Ecosystem${NC}      — Model Context Protocol: GitHub, Slack, Linear..."
echo -e "    • ${CYAN}Hooks System${NC}       — lifecycle hooks (PreToolUse, PostToolUse, Stop...)"
echo -e "    • ${CYAN}Audit & Compliance${NC} — SOC2 JSONL/CSV/PDF export, HMAC chain verification"
echo -e "    • ${CYAN}Cenzontle SSO${NC}      — Zuclubit OAuth 2.1 PKCE login"
echo -e "    • ${CYAN}VS Code Extension${NC}  — JSON-RPC bridge for IDE integration"
echo -e "    • ${CYAN}Full Tool Suite${NC}    — 50+ tools: semantic_grep, native_search, docker..."
echo ""

ENABLE_FRONTIER=true
if is_interactive; then
    if ! ask "Set up frontier tools now?" "y"; then
        ENABLE_FRONTIER=false
        warn "Skipping frontier tools. Run this script again or edit $CONFIG_FILE manually."
    fi
fi

if [ "$ENABLE_FRONTIER" = true ]; then

    # ── 6a: Agent Registry ───────────────────────────────────────────────────
    info "Setting up agent registry..."
    AGENTS_DIR="$CONFIG_DIR/agents"
    SKILLS_DIR="$CONFIG_DIR/skills"
    mkdir -p "$AGENTS_DIR" "$SKILLS_DIR"

    if [ ! -f "$AGENTS_DIR/code-reviewer.md" ]; then
        cat > "$AGENTS_DIR/code-reviewer.md" << 'MD'
---
name: code-reviewer
description: |
  Expert code reviewer. Use when asked to review, audit, or assess code quality,
  security vulnerabilities, or architectural issues.
tools: [Read, Grep, Glob]
model: claude-sonnet-4-6
max_turns: 20
---

You are a senior code reviewer. Focus on correctness, security, and readability.
Always provide specific line references and actionable suggestions.
MD
        ok "Created $AGENTS_DIR/code-reviewer.md"
    fi

    if [ ! -f "$AGENTS_DIR/test-writer.md" ]; then
        cat > "$AGENTS_DIR/test-writer.md" << 'MD'
---
name: test-writer
description: |
  Writes comprehensive test suites. Use when asked to add tests, increase coverage,
  or write unit/integration/e2e tests for any codebase.
tools: [Read, Grep, Glob, file_write, bash]
model: claude-sonnet-4-6
max_turns: 30
---

You are a test engineering specialist. Write thorough, non-flaky tests.
Prefer integration tests over mocks where possible.
MD
        ok "Created $AGENTS_DIR/test-writer.md"
    fi

    # ── 6b: Instruction persistence (HALCON.md) ──────────────────────────────
    info "Setting up instruction persistence..."
    HALCON_MD="$CONFIG_DIR/HALCON.md"
    if [ ! -f "$HALCON_MD" ]; then
        cat > "$HALCON_MD" << 'MD'
# Halcón — Personal Instructions

<!-- This file is loaded at the start of every session. Keep under 200 lines. -->

## Preferences

- Prefer concise, direct answers over lengthy explanations
- Always show file paths with line numbers when referencing code
- Use the audit trail for destructive operations

## Project Conventions

<!-- Add project-specific rules here, e.g.: -->
<!-- - Language: Rust (edition 2021) -->
<!-- - Test runner: cargo nextest -->
<!-- - Commit style: Conventional Commits -->

## Providers

<!-- Preferred provider order for different task types: -->
<!-- - Fast tasks: deepseek-chat -->
<!-- - Complex reasoning: claude-sonnet-4-6 -->
<!-- - Code generation: claude-sonnet-4-6 or deepseek-coder -->

## Memory

<!-- Semantic memory is stored in ~/.halcon/memory/ -->
<!-- Use `search_memory` tool to retrieve past context -->
MD
        ok "Created $HALCON_MD"
    fi

    # ── 6c: Hooks configuration ──────────────────────────────────────────────
    info "Setting up hooks..."
    HOOKS_DIR="$CONFIG_DIR/hooks"
    mkdir -p "$HOOKS_DIR"
    HOOKS_FILE="$CONFIG_DIR/hooks.toml"
    if [ ! -f "$HOOKS_FILE" ]; then
        cat > "$HOOKS_FILE" << 'TOML'
# Halcón Hooks — lifecycle event handlers
# Docs: https://github.com/cuervo-ai/halcon-cli#hooks

# Block rm -rf and force-push at the hook layer (belt-and-suspenders with bash.rs blacklist)
[[hooks]]
event = "PreToolUse"
tool  = "bash"
command = """
#!/usr/bin/env bash
input="$(cat)"
if echo "$input" | grep -qE 'rm\s+-rf\s+/|git push.*--force.*main'; then
  echo '{"permissionDecision":"deny","permissionDecisionReason":"Blocked by PreToolUse hook"}'
  exit 2
fi
"""

# Log all tool calls to the audit trail
[[hooks]]
event = "PostToolUse"
command = """
#!/usr/bin/env bash
echo "[$(date -u +%Y-%m-%dT%H:%M:%SZ)] PostToolUse: $(cat | python3 -c 'import sys,json; d=json.load(sys.stdin); print(d.get(\"tool_name\",\"unknown\"))')" \
  >> ~/.halcon/audit/hooks.log 2>/dev/null || true
"""
TOML
        mkdir -p "$CONFIG_DIR/audit"
        ok "Created $HOOKS_FILE"
    fi

    # ── 6d: MCP configuration ────────────────────────────────────────────────
    info "Setting up MCP ecosystem..."
    MCP_FILE="$CONFIG_DIR/mcp.toml"
    if [ ! -f "$MCP_FILE" ]; then
        cat > "$MCP_FILE" << 'TOML'
# Halcón MCP — Model Context Protocol servers
# Add servers here to give Halcón access to external tools and data sources.
# Run `halcon mcp list` to see all configured servers.
# Run `halcon mcp add <name> -- <command>` to add new servers interactively.

# ── Local stdio servers ──────────────────────────────────────────────────────

# Filesystem access (safe, no network)
# [servers.filesystem]
# transport = "stdio"
# command   = "npx"
# args      = ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/allowed/dir"]

# Git repository tools
# [servers.git]
# transport = "stdio"
# command   = "uvx"
# args      = ["mcp-server-git", "--repository", "."]

# ── Remote HTTP servers (OAuth 2.1 required) ─────────────────────────────────

# GitHub (requires GITHUB_TOKEN env var or SSO)
# [servers.github]
# transport = "http"
# url       = "https://api.githubcopilot.com/mcp/"
# auth      = "oauth"

# Slack
# [servers.slack]
# transport = "http"
# url       = "https://mcp.slack.com/api/mcp"
# auth      = "oauth"

[options]
tool_search_threshold = 0.10   # Fraction of context window before deferred tool search
oauth_port            = 9876   # Loopback port for OAuth PKCE callbacks
session_ttl_secs      = 3600   # MCP HTTP session TTL
TOML
        ok "Created $MCP_FILE"
    fi

    # ── 6e: Frontier tools config block ─────────────────────────────────────
    info "Appending frontier tools config..."
    if ! grep -q '\[policy\]' "$CONFIG_FILE" 2>/dev/null; then
        cat >> "$CONFIG_FILE" << 'TOML'

# ── Frontier capabilities ────────────────────────────────────────────────────

[policy]
enable_agent_registry    = true   # Load declarative agents from ~/.halcon/agents/
enable_semantic_memory   = true   # TF-IDF vector store over session memory
semantic_memory_top_k    = 5      # Top-K memories injected per round
enable_hooks             = true   # Run lifecycle hooks from ~/.halcon/hooks.toml
enable_audit_trail       = true   # Persistent HMAC-chained audit log

[tools.advanced]
enable_native_search     = true   # BM25 + PageRank + semantic search (replaces web_search)
enable_background        = true   # background_start / background_output / background_kill
enable_semantic_grep     = true   # LLM-powered grep for natural-language queries
enable_docker            = false  # Docker tool (enable if Docker daemon is running)
enable_sql_query         = true   # SQLite/DuckDB in-process queries
enable_secret_scan       = true   # Scan files for leaked credentials before commits
enable_web_fetch         = true   # Fetch and parse web pages
enable_code_metrics      = true   # LOC, complexity, test coverage analysis

[mcp_server]
transport   = "stdio"    # "stdio" (Claude Code compatible) or "http"
port        = 7777       # Only used when transport = "http"
require_auth = false     # Set to true + HALCON_MCP_SERVER_API_KEY for HTTP auth

[context]
max_tokens          = 180000   # Context window budget
compaction_threshold = 0.80    # Trigger compaction at 80% fill
enable_repo_map     = true     # Include codebase structure in context

[agent]
enable_registry         = true
enable_lifecycle_hooks  = true
enable_planner          = true   # LLM-based multi-step planning
enable_convergence      = true   # TerminationOracle + ConvergenceController
max_rounds              = 50
TOML
        ok "Frontier config block added to $CONFIG_FILE"
    else
        ok "[policy] block already present — skipping"
    fi

    # ── 6f: Docker detection ─────────────────────────────────────────────────
    if command -v docker &>/dev/null && docker info &>/dev/null 2>&1; then
        ok "Docker detected — enabling docker tool"
        sed -i.bak 's/enable_docker.*=.*false/enable_docker = true/' "$CONFIG_FILE" 2>/dev/null || true
        rm -f "${CONFIG_FILE}.bak"
    fi

    # ── 6g: VS Code / Cursor detection ───────────────────────────────────────
    VSCODE_DETECTED=false
    if command -v code &>/dev/null; then
        VSCODE_DETECTED=true
        EDITOR_CMD="code"
        EDITOR_NAME="VS Code"
    elif command -v cursor &>/dev/null; then
        VSCODE_DETECTED=true
        EDITOR_CMD="cursor"
        EDITOR_NAME="Cursor"
    fi

    if [ "$VSCODE_DETECTED" = true ]; then
        info "$EDITOR_NAME detected"
        CONFIGURE_VSCODE=true
        if is_interactive; then
            if ! ask "Configure $EDITOR_NAME extension (JSON-RPC bridge)?" "y"; then
                CONFIGURE_VSCODE=false
            fi
        fi
        if [ "$CONFIGURE_VSCODE" = true ]; then
            VSCODE_SETTINGS_DIR="$HOME/.config/Code/User"
            [ "$(uname -s)" = "Darwin" ] && VSCODE_SETTINGS_DIR="$HOME/Library/Application Support/Code/User"
            mkdir -p "$VSCODE_SETTINGS_DIR"
            VSCODE_SETTINGS="$VSCODE_SETTINGS_DIR/settings.json"
            HALCON_VSCODE_SETTINGS='{
  "halcon.binaryPath": "'"$INSTALL_DIR/halcon"'",
  "halcon.defaultProvider": "anthropic",
  "halcon.maxTurns": 50,
  "halcon.jsonRpcMode": true
}'
            # Merge into existing settings if present, otherwise create
            if [ -f "$VSCODE_SETTINGS" ] && command -v python3 &>/dev/null; then
                python3 << PYEOF
import json, sys
try:
    with open("$VSCODE_SETTINGS") as f:
        settings = json.load(f)
except Exception:
    settings = {}
new = json.loads('$HALCON_VSCODE_SETTINGS'.replace('\n', ''))
settings.update(new)
with open("$VSCODE_SETTINGS", "w") as f:
    json.dump(settings, f, indent=2)
print("  Merged into existing settings.json")
PYEOF
            else
                echo "$HALCON_VSCODE_SETTINGS" > "$VSCODE_SETTINGS_DIR/halcon-settings.json"
                info "Written to $VSCODE_SETTINGS_DIR/halcon-settings.json"
            fi
            ok "$EDITOR_NAME configured"
        fi
    fi

    # ── 6h: Cenzontle SSO ────────────────────────────────────────────────────
    CONFIGURE_CENZONTLE=false
    if is_interactive; then
        echo ""
        echo -e "  ${BOLD}Cenzontle AI${NC} — enterprise AI platform (Zuclubit SSO)"
        if ask "Log in to Cenzontle AI now? (requires Zuclubit SSO account)" "n"; then
            CONFIGURE_CENZONTLE=true
        fi
    fi

    if [ "$CONFIGURE_CENZONTLE" = true ]; then
        HALCON_BIN_EARLY="$INSTALL_DIR/halcon"
        if [ -x "$HALCON_BIN_EARLY" ]; then
            info "Starting Cenzontle SSO login (browser will open)..."
            "$HALCON_BIN_EARLY" login cenzontle || warn "SSO login failed — run 'halcon login cenzontle' later"
        else
            warn "Binary not ready yet — run 'halcon login cenzontle' after installation"
        fi
    fi

    # ── 6i: Memory directory ─────────────────────────────────────────────────
    MEMORY_DIR="$CONFIG_DIR/memory"
    mkdir -p "$MEMORY_DIR"
    if [ ! -f "$MEMORY_DIR/MEMORY.md" ]; then
        cat > "$MEMORY_DIR/MEMORY.md" << 'MD'
# Halcón — Session Memory Index

> Auto-managed by the semantic memory system.
> Add manual notes here; they will be indexed and retrieved automatically.

## Notes

<!-- Add project notes, team preferences, or recurring context here -->
MD
        ok "Created semantic memory store at $MEMORY_DIR/"
    fi

    echo ""
    ok "Frontier tools configured:"
    echo -e "    ${GREEN}✓${NC} Agent registry  → $AGENTS_DIR/"
    echo -e "    ${GREEN}✓${NC} HALCON.md        → $HALCON_MD"
    echo -e "    ${GREEN}✓${NC} Hooks            → $HOOKS_FILE"
    echo -e "    ${GREEN}✓${NC} MCP config       → $MCP_FILE"
    echo -e "    ${GREEN}✓${NC} Semantic memory  → $MEMORY_DIR/"
    echo -e "    ${GREEN}✓${NC} Policy config    → $CONFIG_FILE"
    [ "$VSCODE_DETECTED" = true ] && echo -e "    ${GREEN}✓${NC} $EDITOR_NAME bridge → $INSTALL_DIR/halcon"

fi  # ENABLE_FRONTIER

# ── Step 7: Verify ───────────────────────────────────────────────────────────
section "[7/7] Verification"

HALCON_BIN="$INSTALL_DIR/halcon"
if [ -x "$HALCON_BIN" ]; then
    VERSION=$("$HALCON_BIN" --version 2>&1 || true)
    ok "halcon --version: $VERSION"
else
    fail "Binary not executable at $HALCON_BIN"
fi

if "$HALCON_BIN" --help >/dev/null 2>&1; then
    ok "halcon --help: exit 0"
else
    warn "halcon --help returned non-zero (may need API key configuration)"
fi

# Verify frontier: agents
if [ "$ENABLE_FRONTIER" = true ]; then
    AGENT_COUNT=$(find "$CONFIG_DIR/agents" -name "*.md" 2>/dev/null | wc -l | tr -d ' ')
    ok "Agent registry: $AGENT_COUNT agent(s) in $CONFIG_DIR/agents/"
fi

echo ""
echo -e "${BOLD}${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}${GREEN}  Installation complete!${NC}"
echo -e "${BOLD}${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo -e "  ${BOLD}Next steps:${NC}"
echo ""
echo -e "  ${CYAN}1.${NC} Set at least one API key:"
echo -e "     ${BOLD}export ANTHROPIC_API_KEY=sk-ant-...${NC}"
echo -e "     ${BOLD}export DEEPSEEK_API_KEY=sk-...${NC}"
echo -e "     ${BOLD}export OPENAI_API_KEY=sk-...${NC}"
echo ""
echo -e "  ${CYAN}2.${NC} Start the interactive REPL:"
echo -e "     ${BOLD}halcon${NC}"
echo ""
echo -e "  ${CYAN}3.${NC} Single-shot chat:"
echo -e "     ${BOLD}halcon chat \"Explain this codebase\"${NC}"
echo ""
if [ "$ENABLE_FRONTIER" = true ]; then
    echo -e "  ${CYAN}4.${NC} Frontier capabilities:"
    echo -e "     ${BOLD}halcon agents list${NC}          — view registered sub-agents"
    echo -e "     ${BOLD}halcon mcp list${NC}             — view MCP server connections"
    echo -e "     ${BOLD}halcon tools list${NC}           — view all 50+ available tools"
    echo -e "     ${BOLD}halcon audit list${NC}           — view audit trail"
    echo -e "     ${BOLD}halcon login cenzontle${NC}      — Cenzontle SSO login"
    echo -e "     ${BOLD}halcon mcp serve${NC}            — expose Halcón as MCP server"
    echo ""
fi
echo -e "  ${BLUE}Config:${NC}   $CONFIG_FILE"
echo -e "  ${BLUE}Agents:${NC}   $CONFIG_DIR/agents/"
echo -e "  ${BLUE}Memory:${NC}   $CONFIG_DIR/memory/"
echo -e "  ${BLUE}Docs:${NC}     $REPO_URL"
echo ""
