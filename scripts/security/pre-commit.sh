#!/bin/bash
# Cuervo CLI - Security Pre-commit Hooks
# This script runs security checks before commits

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}╔══════════════════════════════════════════════════════════╗${NC}"
echo -e "${BLUE}║             Security Pre-commit Checks                   ║${NC}"
echo -e "${BLUE}╚══════════════════════════════════════════════════════════╝${NC}"
echo ""

# Function to check if command exists
command_exists() {
    command -v "$1" >/dev/null 2>&1
}

# Function to print section header
print_section() {
    echo -e "\n${YELLOW}[$1]${NC} $2"
}

# Function to print result
print_result() {
    if [ $1 -eq 0 ]; then
        echo -e "${GREEN}✓${NC} $2"
    else
        echo -e "${RED}✗${NC} $2"
        return 1
    fi
}

# Track overall success
OVERALL_SUCCESS=0

# 1. Check for secrets
print_section "1/7" "Checking for secrets..."
if command_exists trufflehog; then
    trufflehog filesystem . --only-verified --no-update
    print_result $? "No secrets found"
else
    echo -e "${YELLOW}⚠${NC} trufflehog not installed, skipping secret detection"
fi

# 2. Check for large files
print_section "2/7" "Checking for large files..."
LARGE_FILES=$(find . -type f -size +5M ! -path "./target/*" ! -path "./.git/*" | head -10)
if [ -z "$LARGE_FILES" ]; then
    print_result 0 "No large files found"
else
    echo -e "${RED}✗${NC} Large files found:"
    echo "$LARGE_FILES"
    OVERALL_SUCCESS=1
fi

# 3. Check for suspicious file permissions
print_section "3/7" "Checking file permissions..."
SUSPICIOUS_PERMS=$(find . -type f -perm /111 ! -path "./target/*" ! -path "./.git/*" ! -name "*.sh" ! -name "*.py" | head -10)
if [ -z "$SUSPICIOUS_PERMS" ]; then
    print_result 0 "No suspicious file permissions"
else
    echo -e "${YELLOW}⚠${NC} Executable files found (non-script):"
    echo "$SUSPICIOUS_PERMS"
fi

# 4. Rust security checks
print_section "4/7" "Running Rust security checks..."

# Check formatting
if cargo fmt -- --check >/dev/null 2>&1; then
    print_result 0 "Code is properly formatted"
else
    echo -e "${RED}✗${NC} Code formatting issues"
    cargo fmt -- --check
    OVERALL_SUCCESS=1
fi

# Clippy security checks
if cargo clippy -- -D warnings -D clippy::security >/dev/null 2>&1; then
    print_result 0 "Clippy security checks passed"
else
    echo -e "${RED}✗${NC} Clippy security checks failed"
    cargo clippy -- -D warnings -D clippy::security
    OVERALL_SUCCESS=1
fi

# 5. Check for TODO/FIXME in code
print_section "5/7" "Checking for TODO/FIXME comments..."
TODO_COUNT=$(grep -r "TODO\|FIXME" --include="*.rs" --include="*.toml" --include="*.md" . | grep -v "./target/" | grep -v "./.git/" | wc -l)
if [ "$TODO_COUNT" -eq 0 ]; then
    print_result 0 "No TODO/FIXME comments found"
else
    echo -e "${YELLOW}⚠${NC} Found $TODO_COUNT TODO/FIXME comments"
    grep -r "TODO\|FIXME" --include="*.rs" --include="*.toml" --include="*.md" . | grep -v "./target/" | grep -v "./.git/" | head -5
fi

# 6. Check for debug statements
print_section "6/7" "Checking for debug statements..."
DEBUG_COUNT=$(grep -r "println!\|dbg!\|eprintln!" --include="*.rs" . | grep -v "./target/" | grep -v "./.git/" | grep -v "//.*debug" | wc -l)
if [ "$DEBUG_COUNT" -eq 0 ]; then
    print_result 0 "No debug statements found"
else
    echo -e "${YELLOW}⚠${NC} Found $DEBUG_COUNT debug statements"
    grep -r "println!\|dbg!\|eprintln!" --include="*.rs" . | grep -v "./target/" | grep -v "./.git/" | grep -v "//.*debug" | head -5
fi

# 7. Check dependency security
print_section "7/7" "Checking dependency security..."
if command_exists cargo-audit; then
    if cargo audit >/dev/null 2>&1; then
        print_result 0 "No vulnerable dependencies"
    else
        echo -e "${RED}✗${NC} Vulnerable dependencies found"
        cargo audit
        OVERALL_SUCCESS=1
    fi
else
    echo -e "${YELLOW}⚠${NC} cargo-audit not installed, skipping dependency audit"
fi

# Summary
echo -e "\n${BLUE}══════════════════════════════════════════════════════════${NC}"
if [ $OVERALL_SUCCESS -eq 0 ]; then
    echo -e "${GREEN}✅ All security checks passed!${NC}"
    echo -e "You can proceed with the commit."
else
    echo -e "${RED}❌ Security checks failed!${NC}"
    echo -e "Please fix the issues above before committing."
    echo -e "\nTo bypass security checks (not recommended):"
    echo -e "  git commit --no-verify"
    exit 1
fi

# Additional security recommendations
echo -e "\n${BLUE}Security Recommendations:${NC}"
echo "1. Run 'cargo audit' regularly to check for vulnerabilities"
echo "2. Use 'cargo outdated' to check for dependency updates"
echo "3. Enable secret scanning in your IDE"
echo "4. Review code for security issues before committing"
echo "5. Run security tests: 'cargo test --test security'"

echo -e "\n${BLUE}For more information:${NC}"
echo "📚 Security documentation: docs/05-security-legal/"
echo "🔧 Security tools: scripts/security/"
echo "🚨 Report security issues: security@cuervo.ai"

exit $OVERALL_SUCCESS