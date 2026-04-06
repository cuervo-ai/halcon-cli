#!/usr/bin/env bash
# Halcon CLI Installation Verification Script
# Run this anytime to verify your Halcon installation

set -euo pipefail

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

PASS=0
FAIL=0

check() {
    local name="$1"
    shift
    printf "%-50s " "$name"
    if "$@" &>/dev/null; then
        echo -e "${GREEN}✓ PASS${NC}"
        ((PASS++))
        return 0
    else
        echo -e "${RED}✗ FAIL${NC}"
        ((FAIL++))
        return 1
    fi
}

check_output() {
    local name="$1"
    local expected="$2"
    shift 2
    printf "%-50s " "$name"
    local output=$("$@" 2>&1)
    if echo "$output" | grep -q "$expected"; then
        echo -e "${GREEN}✓ PASS${NC}"
        ((PASS++))
        return 0
    else
        echo -e "${RED}✗ FAIL${NC}"
        ((FAIL++))
        return 1
    fi
}

echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BLUE}  Halcon CLI Installation Verification${NC}"
echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""

echo -e "${YELLOW}[1/5] Binary Checks${NC}"
check "Binary exists in PATH" which halcon
check "Binary is executable" test -x "$(which halcon)"
check_output "Version command works" "0.3.14" halcon --version
check "Help command works" halcon --help
echo ""

echo -e "${YELLOW}[2/5] Core Commands${NC}"
check "chat command available" halcon chat --help
check "config command available" halcon config --help
check "status command available" halcon status --help
check "doctor command available" halcon doctor --help
check "memory command available" halcon memory --help
check "tools command available" halcon tools --help
check "metrics command available" halcon metrics --help
echo ""

echo -e "${YELLOW}[3/5] Subsystems${NC}"
check "Status command executes" halcon status
check "Memory stats work" halcon memory stats
check "Config show works" halcon config show
echo ""

echo -e "${YELLOW}[4/5] Provider Configuration${NC}"
check_output "Default provider set" "cenzontle" halcon status
check_output "Provider list available" "providers" halcon status
echo ""

echo -e "${YELLOW}[5/5] Advanced Features${NC}"
check "MCP subsystem available" halcon mcp --help
check "LSP subsystem available" halcon lsp --help
check "Agents subsystem available" halcon agents --help
check "Trace subsystem available" halcon trace --help
check "Replay subsystem available" halcon replay --help
echo ""

# Summary
echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BLUE}  Test Results${NC}"
echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo -e "  ${GREEN}Passed: $PASS${NC}"
echo -e "  ${RED}Failed: $FAIL${NC}"
echo ""

TOTAL=$((PASS + FAIL))
PERCENT=$((PASS * 100 / TOTAL))

if [ $FAIL -eq 0 ]; then
    echo -e "${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${GREEN}  ✓ All checks passed! ($PERCENT%)${NC}"
    echo -e "${GREEN}  Installation is 100% functional.${NC}"
    echo -e "${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo ""
    echo "Quick start:"
    echo "  halcon chat         # Start interactive chat"
    echo "  halcon doctor       # Run full diagnostics"
    echo "  halcon --help       # See all commands"
    exit 0
else
    echo -e "${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${YELLOW}  ⚠ Some checks failed ($PERCENT% passed)${NC}"
    echo -e "${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo ""
    echo "Run 'halcon doctor' for detailed diagnostics."
    exit 1
fi
