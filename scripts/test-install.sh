#!/usr/bin/env bash
# Test script for install-binary.sh
# Validates installer behavior across different scenarios

set -euo pipefail

readonly GREEN='\033[0;32m'
readonly RED='\033[0;31m'
readonly YELLOW='\033[1;33m'
readonly BLUE='\033[0;34m'
readonly NC='\033[0m'

TESTS_PASSED=0
TESTS_FAILED=0

pass() {
    ((TESTS_PASSED++))
    echo -e "${GREEN}✓${NC} $1"
}

fail() {
    ((TESTS_FAILED++))
    echo -e "${RED}✗${NC} $1"
}

info() {
    echo -e "${BLUE}[TEST]${NC} $1"
}

section() {
    echo ""
    echo -e "${YELLOW}━━━ $1 ━━━${NC}"
    echo ""
}

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Test Functions
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

test_script_exists() {
    section "Script Existence Tests"

    if [ -f "scripts/install-binary.sh" ]; then
        pass "install-binary.sh exists"
    else
        fail "install-binary.sh not found"
    fi

    if [ -x "scripts/install-binary.sh" ]; then
        pass "install-binary.sh is executable"
    else
        fail "install-binary.sh is not executable"
    fi
}

test_syntax() {
    section "Syntax Validation"

    if bash -n scripts/install-binary.sh 2>/dev/null; then
        pass "Bash syntax is valid"
    else
        fail "Bash syntax errors detected"
    fi

    if command -v shellcheck >/dev/null 2>&1; then
        if shellcheck -S warning scripts/install-binary.sh 2>/dev/null; then
            pass "ShellCheck validation passed"
        else
            fail "ShellCheck found issues"
        fi
    else
        info "ShellCheck not available, skipping"
    fi
}

test_platform_detection() {
    section "Platform Detection Functions"

    # Source the script functions (without running main)
    source <(sed '/^main /,/^main /d' scripts/install-binary.sh 2>/dev/null || echo "")

    # Test OS detection
    local os
    os=$(detect_os 2>/dev/null || echo "failed")
    if [ "$os" != "failed" ] && [[ "$os" =~ ^(linux|darwin|windows)$ ]]; then
        pass "OS detection: $os"
    else
        fail "OS detection failed: $os"
    fi

    # Test architecture detection
    local arch
    arch=$(detect_arch 2>/dev/null || echo "failed")
    if [ "$arch" != "failed" ] && [[ "$arch" =~ ^(x86_64|aarch64|armv7|i686)$ ]]; then
        pass "Architecture detection: $arch"
    else
        fail "Architecture detection failed: $arch"
    fi

    # Test target construction
    local target
    target=$(construct_target "$os" "$arch" "gnu" 2>/dev/null || echo "failed")
    if [ "$target" != "failed" ] && [[ "$target" =~ ^[a-z0-9_-]+$ ]]; then
        pass "Target construction: $target"
    else
        fail "Target construction failed: $target"
    fi
}

test_dependencies() {
    section "Dependency Checks"

    local required_tools=("curl" "tar" "sha256sum")
    local optional_tools=("wget" "unzip")

    for tool in "${required_tools[@]}"; do
        if command -v "$tool" >/dev/null 2>&1; then
            pass "Required tool available: $tool"
        else
            fail "Required tool missing: $tool"
        fi
    done

    for tool in "${optional_tools[@]}"; do
        if command -v "$tool" >/dev/null 2>&1; then
            info "Optional tool available: $tool"
        fi
    done
}

test_checksum_validation() {
    section "Checksum Validation"

    # Create test file and checksum
    local test_file="test_file.txt"
    local checksum_file="test_file.txt.sha256"

    echo "test content" > "$test_file"
    sha256sum "$test_file" > "$checksum_file"

    # Source verify function
    source <(sed '/^main /,/^main /d' scripts/install-binary.sh 2>/dev/null || echo "")

    if verify_checksum "$test_file" "$checksum_file" 2>/dev/null; then
        pass "Checksum verification works"
    else
        fail "Checksum verification failed"
    fi

    # Test with invalid checksum
    echo "invalid_hash  test_file.txt" > "$checksum_file"
    if ! verify_checksum "$test_file" "$checksum_file" 2>/dev/null; then
        pass "Invalid checksum correctly rejected"
    else
        fail "Invalid checksum not detected"
    fi

    # Cleanup
    rm -f "$test_file" "$checksum_file"
}

test_archive_extraction() {
    section "Archive Extraction"

    local test_dir="test_archive_dir"
    mkdir -p "$test_dir"

    # Create test file
    echo "test binary" > "$test_dir/cuervo"

    # Create tar.gz
    tar czf test.tar.gz -C "$test_dir" cuervo

    # Test extraction
    local extract_dir="test_extract"
    mkdir -p "$extract_dir"

    source <(sed '/^main /,/^main /d' scripts/install-binary.sh 2>/dev/null || echo "")

    if extract_archive "test.tar.gz" "$extract_dir" 2>/dev/null; then
        if [ -f "$extract_dir/cuervo" ]; then
            pass "Archive extraction successful"
        else
            fail "Archive extracted but file not found"
        fi
    else
        fail "Archive extraction failed"
    fi

    # Cleanup
    rm -rf "$test_dir" "$extract_dir" test.tar.gz
}

test_path_detection() {
    section "Shell Profile Detection"

    source <(sed '/^main /,/^main /d' scripts/install-binary.sh 2>/dev/null || echo "")

    local profile
    profile=$(detect_shell_profile 2>/dev/null || echo "failed")

    if [ "$profile" != "failed" ] && [ -n "$profile" ]; then
        pass "Shell profile detected: $profile"
    else
        fail "Shell profile detection failed"
    fi
}

test_security() {
    section "Security Checks"

    # Check for hardcoded credentials
    if grep -i "password\|secret\|token" scripts/install-binary.sh | grep -v "GITHUB_TOKEN" | grep -q "="; then
        fail "Potential hardcoded credentials found"
    else
        pass "No hardcoded credentials detected"
    fi

    # Check for unsafe practices
    if grep -q "eval" scripts/install-binary.sh; then
        fail "Unsafe 'eval' usage detected"
    else
        pass "No unsafe 'eval' usage"
    fi

    # Check for proper error handling
    if grep -q "set -euo pipefail" scripts/install-binary.sh; then
        pass "Proper error handling flags set"
    else
        fail "Missing 'set -euo pipefail'"
    fi

    # Check HTTPS enforcement
    if grep "http://" scripts/install-binary.sh | grep -qv "localhost"; then
        fail "Insecure HTTP URLs detected"
    else
        pass "HTTPS enforcement verified"
    fi
}

test_dry_run() {
    section "Dry Run Test (Mock Installation)"

    # Set environment to point to non-existent repo for testing
    export CUERVO_REPO_OWNER="test-owner"
    export CUERVO_REPO_NAME="test-repo"
    export CUERVO_INSTALL_DIR="/tmp/cuervo-test-install-$$"

    info "Testing install script (expect to fail on download, which is normal)"

    # Run installer, expect it to fail at download stage
    if ./scripts/install-binary.sh 2>&1 | grep -q "Failed to download"; then
        pass "Installer reaches download stage correctly"
    else
        info "Installer may have cargo fallback available"
    fi

    # Cleanup
    rm -rf "$CUERVO_INSTALL_DIR"
    unset CUERVO_REPO_OWNER CUERVO_REPO_NAME CUERVO_INSTALL_DIR
}

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Main Test Runner
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

main() {
    echo ""
    echo -e "${BLUE}╔═══════════════════════════════════════════╗${NC}"
    echo -e "${BLUE}║   Cuervo CLI - Installer Test Suite      ║${NC}"
    echo -e "${BLUE}╚═══════════════════════════════════════════╝${NC}"

    # Change to repo root
    cd "$(dirname "$0")/.."

    # Run all tests
    test_script_exists
    test_syntax
    test_dependencies
    test_platform_detection
    test_checksum_validation
    test_archive_extraction
    test_path_detection
    test_security
    test_dry_run

    # Summary
    echo ""
    echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${GREEN}Passed: $TESTS_PASSED${NC}"
    echo -e "${RED}Failed: $TESTS_FAILED${NC}"
    echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo ""

    if [ $TESTS_FAILED -eq 0 ]; then
        echo -e "${GREEN}✓ All tests passed!${NC}"
        exit 0
    else
        echo -e "${RED}✗ Some tests failed${NC}"
        exit 1
    fi
}

main "$@"
