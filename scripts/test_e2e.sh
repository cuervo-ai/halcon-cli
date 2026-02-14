#!/usr/bin/env bash
# Cuervo CLI — End-to-End Test Suite
# Usage: ./scripts/test_e2e.sh [--provider deepseek] [--binary ./target/release/cuervo]
#
# Runs 9 E2E test cases covering:
#   1. Simple prompt (text-only response)
#   2. File read (tool use)
#   3. File write (destructive tool, plan execution)
#   4. File edit (tool dedup verification)
#   5. Multi-tool (directory_tree + file_read)
#   6. Error recovery (nonexistent file)
#   7. Loop guard (consecutive tool rounds limit)
#   8. Smoke: --help, --version, tools list, tools validate
#   9. Cleanup
set -uo pipefail

# ── Load API Keys ────────────────────────────────────────────────────────────
# Source shell profile if API keys aren't already in environment.
# Keys are typically exported in ~/.zshrc or ~/.bashrc.
if [ -z "${DEEPSEEK_API_KEY:-}" ] && [ -z "${OPENAI_API_KEY:-}" ]; then
    for rcfile in "$HOME/.zshrc" "$HOME/.bashrc" "$HOME/.bash_profile"; do
        if [ -f "$rcfile" ]; then
            # Only extract export lines to avoid interactive shell features
            eval "$(grep '^export .*_API_KEY=' "$rcfile" 2>/dev/null)" || true
            break
        fi
    done
fi

# Strip ANSI escape codes from input
strip_ansi() {
    sed 's/\x1b\[[0-9;]*m//g'
}

# ── Configuration ────────────────────────────────────────────────────────────

CUERVO="${CUERVO_BINARY:-./target/release/cuervo}"
PROVIDER="${CUERVO_PROVIDER:-deepseek}"
TEST_DIR="/tmp/cuervo_e2e_$$"
TIMEOUT_SECS=120
OLLAMA_TIMEOUT_SECS=300
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

# Parse CLI args
while [[ $# -gt 0 ]]; do
    case $1 in
        --provider) PROVIDER="$2"; shift 2 ;;
        --binary)   CUERVO="$2"; shift 2 ;;
        --timeout)  TIMEOUT_SECS="$2"; shift 2 ;;
        --help)
            echo "Usage: $0 [--provider deepseek] [--binary ./target/release/cuervo] [--timeout 120]"
            exit 0
            ;;
        *) echo "Unknown arg: $1"; exit 1 ;;
    esac
done

# ── Colors & Helpers ─────────────────────────────────────────────────────────

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
BOLD='\033[1m'
DIM='\033[2m'
NC='\033[0m'

PASS=0
FAIL=0
SKIP=0
declare -a RESULTS=()

pass() {
    ((PASS++)) || true
    RESULTS+=("PASS|$1|$2")
    echo -e "  ${GREEN}PASS${NC} $1 ${DIM}($2)${NC}"
}

fail() {
    ((FAIL++)) || true
    RESULTS+=("FAIL|$1|$2")
    echo -e "  ${RED}FAIL${NC} $1 ${DIM}($2)${NC}"
}

skip() {
    ((SKIP++)) || true
    RESULTS+=("SKIP|$1|$2")
    echo -e "  ${YELLOW}SKIP${NC} $1 ${DIM}($2)${NC}"
}

# Portable timeout: prefer coreutils timeout, fall back to perl alarm
if command -v timeout &>/dev/null; then
    _timeout() { timeout "$@"; }
elif command -v gtimeout &>/dev/null; then
    _timeout() { gtimeout "$@"; }
else
    _timeout() { local secs="$1"; shift; perl -e "alarm $secs; exec @ARGV" -- "$@"; }
fi

run_cuervo() {
    local timeout_val="${1:-$TIMEOUT_SECS}"
    shift
    _timeout "$timeout_val" "$CUERVO" --no-banner -p "$PROVIDER" "$@" 2>/tmp/cuervo_e2e_stderr.txt
}

elapsed() {
    local start=$1
    local end
    end=$(python3 -c 'import time; print(int(time.time()*1000))' 2>/dev/null || date +%s%3N)
    echo "$(( end - start ))ms"
}

# ── Pre-flight ───────────────────────────────────────────────────────────────

echo -e "${BOLD}${BLUE}"
echo "  ╔═════════════════════════════════════════════╗"
echo "  ║      Cuervo CLI — E2E Test Suite            ║"
echo "  ╚═════════════════════════════════════════════╝"
echo -e "${NC}"
echo -e "  Binary:   ${BOLD}$CUERVO${NC}"
echo -e "  Provider: ${BOLD}$PROVIDER${NC}"
echo -e "  Test dir: ${BOLD}$TEST_DIR${NC}"
echo -e "  Timeout:  ${BOLD}${TIMEOUT_SECS}s${NC}"
echo -e "  Time:     ${DIM}$TIMESTAMP${NC}"
echo ""

# Verify binary exists
if [ ! -x "$CUERVO" ]; then
    echo -e "${RED}Binary not found or not executable: $CUERVO${NC}"
    echo "Build first: cargo build --release --no-default-features"
    exit 1
fi

# Create test directory
mkdir -p "$TEST_DIR"

# Create seed files for read/edit tests
echo "Hello from Cuervo E2E test!" > "$TEST_DIR/hello.txt"
echo "First line" > "$TEST_DIR/multiline.txt"
echo "Second line" >> "$TEST_DIR/multiline.txt"
echo "Third line" >> "$TEST_DIR/multiline.txt"
mkdir -p "$TEST_DIR/subdir"
echo "Nested file content" > "$TEST_DIR/subdir/nested.txt"

echo -e "${BOLD}═══ SMOKE TESTS ═══${NC}"
echo ""

# ── Test 0: Smoke Tests ─────────────────────────────────────────────────────

# 0a: --version
if "$CUERVO" --version >/dev/null 2>&1; then
    VERSION=$("$CUERVO" --version 2>&1)
    pass "cuervo --version" "$VERSION"
else
    fail "cuervo --version" "exit non-zero"
fi

# 0b: --help
if "$CUERVO" --help >/dev/null 2>&1; then
    pass "cuervo --help" "exit 0"
else
    fail "cuervo --help" "exit non-zero"
fi

# 0c: tools list (output goes to stderr)
TOOL_COUNT=$("$CUERVO" tools list 2>&1 | strip_ansi | grep -cE '\[RO\]|\[RW\]|\[D!\]') || true
TOOL_COUNT="${TOOL_COUNT:-0}"
if [ "$TOOL_COUNT" -ge 20 ]; then
    pass "cuervo tools list" "$TOOL_COUNT tools found"
else
    fail "cuervo tools list" "expected >= 20 tools, got $TOOL_COUNT"
fi

# 0d: tools validate (output goes to stderr)
VALIDATE_OUTPUT=$("$CUERVO" tools validate 2>&1 | strip_ansi || true)
VALIDATE_OK=$(echo "$VALIDATE_OUTPUT" | grep -c '\[OK\]') || true
VALIDATE_OK="${VALIDATE_OK:-0}"
VALIDATE_FAIL=$(echo "$VALIDATE_OUTPUT" | grep -c '\[FAIL\]') || true
VALIDATE_FAIL="${VALIDATE_FAIL:-0}"
if [ "$VALIDATE_FAIL" -eq 0 ] && [ "$VALIDATE_OK" -ge 20 ]; then
    pass "cuervo tools validate" "$VALIDATE_OK pass, $VALIDATE_FAIL fail"
else
    fail "cuervo tools validate" "$VALIDATE_OK pass, $VALIDATE_FAIL fail"
fi

echo ""
echo -e "${BOLD}═══ E2E CASES (provider: $PROVIDER) ═══${NC}"
echo ""

# ── CASO 1: Simple Prompt (text-only) ───────────────────────────────────────

echo -e "${BLUE}[CASO 1] Simple prompt — text-only response${NC}"
START=$(python3 -c 'import time; print(int(time.time()*1000))' 2>/dev/null || date +%s%3N)

RESPONSE=$(run_cuervo "$TIMEOUT_SECS" chat "Respond with ONLY the word CUERVO_OK and nothing else." 2>/dev/null || true)

if echo "$RESPONSE" | grep -qi "CUERVO_OK"; then
    pass "Simple prompt" "$(elapsed "$START")"
else
    fail "Simple prompt" "Expected CUERVO_OK, got: $(echo "$RESPONSE" | head -1 | cut -c1-80)"
fi

# ── CASO 2: File Read (tool use) ────────────────────────────────────────────

echo -e "${BLUE}[CASO 2] File read — reads $TEST_DIR/hello.txt${NC}"
START=$(python3 -c 'import time; print(int(time.time()*1000))' 2>/dev/null || date +%s%3N)

RESPONSE=$(run_cuervo "$TIMEOUT_SECS" chat "Read the file $TEST_DIR/hello.txt and tell me its EXACT content. Quote it verbatim." 2>/dev/null || true)

if echo "$RESPONSE" | grep -qi "Hello from Cuervo"; then
    pass "File read" "$(elapsed "$START")"
else
    fail "File read" "Content not found in response: $(echo "$RESPONSE" | head -3 | cut -c1-100)"
fi

# ── CASO 3: File Write (destructive tool) ───────────────────────────────────

echo -e "${BLUE}[CASO 3] File write — creates $TEST_DIR/agent_created.txt${NC}"
START=$(python3 -c 'import time; print(int(time.time()*1000))' 2>/dev/null || date +%s%3N)

RESPONSE=$(run_cuervo "$TIMEOUT_SECS" chat "Create a file at $TEST_DIR/agent_created.txt with the content 'Created by Cuervo E2E'. Use the file_write tool. Confirm when done." 2>/dev/null || true)

if [ -f "$TEST_DIR/agent_created.txt" ]; then
    CONTENT=$(cat "$TEST_DIR/agent_created.txt")
    if echo "$CONTENT" | grep -qi "Cuervo"; then
        pass "File write" "$(elapsed "$START") — content verified"
    else
        fail "File write" "File exists but content unexpected: $CONTENT"
    fi
else
    fail "File write" "$(elapsed "$START") — file not created"
fi

# ── CASO 4: File Edit (dedup verification) ──────────────────────────────────

echo -e "${BLUE}[CASO 4] File edit — edits $TEST_DIR/hello.txt${NC}"
START=$(python3 -c 'import time; print(int(time.time()*1000))' 2>/dev/null || date +%s%3N)

RESPONSE=$(run_cuervo "$TIMEOUT_SECS" chat "Edit the file $TEST_DIR/hello.txt: replace 'Hello' with 'Hola'. Then confirm the change." 2>/dev/null || true)

if [ -f "$TEST_DIR/hello.txt" ]; then
    CONTENT=$(cat "$TEST_DIR/hello.txt")
    if echo "$CONTENT" | grep -qi "Hola"; then
        # Check for dedup in stderr
        STDERR=$(cat /tmp/cuervo_e2e_stderr.txt 2>/dev/null || true)
        DEDUP_NOTE=""
        if echo "$STDERR" | grep -qi "Duplicate tool call filtered"; then
            DEDUP_NOTE=" + dedup fired"
        fi
        pass "File edit" "$(elapsed "$START")${DEDUP_NOTE}"
    else
        fail "File edit" "Edit not applied: $CONTENT"
    fi
else
    fail "File edit" "File disappeared"
fi

# ── CASO 5: Multi-tool (directory listing + read) ───────────────────────────

echo -e "${BLUE}[CASO 5] Multi-tool — list and read files in $TEST_DIR${NC}"
START=$(python3 -c 'import time; print(int(time.time()*1000))' 2>/dev/null || date +%s%3N)

RESPONSE=$(run_cuervo "$TIMEOUT_SECS" chat "List all files in $TEST_DIR (including subdirectories) and read the content of the file inside the subdir/ folder. Tell me what the nested file contains." 2>/dev/null || true)

if echo "$RESPONSE" | grep -qi "nested\|Nested file content"; then
    pass "Multi-tool" "$(elapsed "$START")"
else
    fail "Multi-tool" "Nested content not found: $(echo "$RESPONSE" | head -3 | cut -c1-100)"
fi

# ── CASO 6: Error Recovery (nonexistent file) ───────────────────────────────

echo -e "${BLUE}[CASO 6] Error recovery — read nonexistent file${NC}"
START=$(python3 -c 'import time; print(int(time.time()*1000))' 2>/dev/null || date +%s%3N)

RESPONSE=$(run_cuervo "$TIMEOUT_SECS" chat "Read the file /tmp/this_file_definitely_does_not_exist_xyz123.txt and tell me what happened." 2>/dev/null || true)

STDERR=$(cat /tmp/cuervo_e2e_stderr.txt 2>/dev/null || true)

# The model should report an error, not enter a loop
if echo "$RESPONSE" "$STDERR" | grep -qi "not found\|does not exist\|no such file\|error\|cannot\|doesn't exist"; then
    pass "Error recovery" "$(elapsed "$START")"
else
    fail "Error recovery" "No error reported: $(echo "$RESPONSE" | head -2 | cut -c1-100)"
fi

# ── CASO 7: Loop Guard (consecutive tool rounds) ────────────────────────────

echo -e "${BLUE}[CASO 7] Loop guard — analyze project (many potential tool rounds)${NC}"
START=$(python3 -c 'import time; print(int(time.time()*1000))' 2>/dev/null || date +%s%3N)

RESPONSE=$(run_cuervo "$TIMEOUT_SECS" chat "Analyze the directory $TEST_DIR: list all files, read each one, and give me a summary. Be thorough." 2>/dev/null || true)

ELAPSED=$(elapsed "$START")
STDERR=$(cat /tmp/cuervo_e2e_stderr.txt 2>/dev/null || true)

# Check that we got a response (didn't hang) and look for loop guard activity
LOOP_GUARD_NOTE=""
if echo "$STDERR" | grep -qi "loop guard\|forcing tool withdrawal\|synthesis"; then
    LOOP_GUARD_NOTE=" + loop guard active"
fi
if echo "$STDERR" | grep -qi "Duplicate tool call filtered"; then
    LOOP_GUARD_NOTE="${LOOP_GUARD_NOTE} + dedup"
fi

if [ -n "$RESPONSE" ]; then
    pass "Loop guard" "${ELAPSED}${LOOP_GUARD_NOTE}"
else
    fail "Loop guard" "${ELAPSED} — no response (possible hang)"
fi

# ── CASO 8: Ollama Provider (if available) ───────────────────────────────────

echo ""
echo -e "${BOLD}═══ OLLAMA TEST (optional) ═══${NC}"
echo ""

# Check if ollama is running
if curl -s http://localhost:11434/api/tags >/dev/null 2>&1; then
    echo -e "${BLUE}[CASO 8] Ollama — loop guard stress test${NC}"
    START=$(python3 -c 'import time; print(int(time.time()*1000))' 2>/dev/null || date +%s%3N)

    RESPONSE=$(_timeout "$OLLAMA_TIMEOUT_SECS" "$CUERVO" --no-banner -p ollama chat \
        "Create a file at $TEST_DIR/ollama_test.txt with 'Ollama works'. Then read it back and confirm." \
        2>/tmp/cuervo_e2e_ollama_stderr.txt || true)

    ELAPSED=$(elapsed "$START")
    STDERR=$(cat /tmp/cuervo_e2e_ollama_stderr.txt 2>/dev/null || true)

    OLLAMA_NOTES=""
    if echo "$STDERR" | grep -qi "forcing tool withdrawal"; then
        OLLAMA_NOTES=" + loop guard forced withdrawal"
    fi
    if echo "$STDERR" | grep -qi "Duplicate tool call filtered"; then
        OLLAMA_NOTES="${OLLAMA_NOTES} + dedup"
    fi

    # Ollama test success criteria: loop guard activated OR file created OR got a response
    # Local models are slow and unreliable — the key check is that we don't hang forever
    if [ -f "$TEST_DIR/ollama_test.txt" ]; then
        pass "Ollama loop guard" "${ELAPSED}${OLLAMA_NOTES} — file created"
    elif [ -n "$RESPONSE" ]; then
        pass "Ollama loop guard" "${ELAPSED}${OLLAMA_NOTES} — got response"
    elif echo "$STDERR" | grep -qi "loop guard\|forcing tool withdrawal\|synthesis"; then
        pass "Ollama loop guard" "${ELAPSED}${OLLAMA_NOTES} — guard activated"
    else
        fail "Ollama loop guard" "${ELAPSED} — no response, no guard activity"
    fi
else
    skip "Ollama loop guard" "ollama not running on localhost:11434"
fi

# ── Report ───────────────────────────────────────────────────────────────────

echo ""
echo -e "${BOLD}═══ RESULTS ═══${NC}"
echo ""

TOTAL=$((PASS + FAIL + SKIP))
echo -e "  ${GREEN}PASS${NC}: $PASS"
echo -e "  ${RED}FAIL${NC}: $FAIL"
echo -e "  ${YELLOW}SKIP${NC}: $SKIP"
echo -e "  TOTAL: $TOTAL"
echo ""

# Detailed table
echo "  ┌────────┬────────────────────────────────────┬─────────────┐"
echo "  │ Status │ Test                               │ Detail      │"
echo "  ├────────┼────────────────────────────────────┼─────────────┤"
for result in "${RESULTS[@]}"; do
    IFS='|' read -r status name detail <<< "$result"
    case "$status" in
        PASS) color="$GREEN" ;;
        FAIL) color="$RED" ;;
        SKIP) color="$YELLOW" ;;
        *)    color="$NC" ;;
    esac
    printf "  │ ${color}%-6s${NC} │ %-34s │ %-11s │\n" "$status" "$name" "$(echo "$detail" | cut -c1-11)"
done
echo "  └────────┴────────────────────────────────────┴─────────────┘"

# ── Cleanup ──────────────────────────────────────────────────────────────────

echo ""
echo -e "${DIM}Cleaning up $TEST_DIR...${NC}"
rm -rf "$TEST_DIR"
rm -f /tmp/cuervo_e2e_stderr.txt /tmp/cuervo_e2e_ollama_stderr.txt

echo ""
if [ "$FAIL" -eq 0 ]; then
    echo -e "${BOLD}${GREEN}ALL TESTS PASSED${NC}"
    exit 0
else
    echo -e "${BOLD}${RED}$FAIL TEST(S) FAILED${NC}"
    exit 1
fi
