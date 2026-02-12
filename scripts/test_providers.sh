#!/usr/bin/env bash
# Cuervo CLI — Provider Integration Test Script
# Tests each provider/model individually via single-shot mode
set -euo pipefail

# Load API keys from .env
export DEEPSEEK_API_KEY=$(grep '^deepseek=' .env | cut -d= -f2 | tr -d ' ')
export OPENAI_API_KEY=$(grep '^openai=' .env | cut -d= -f2 | tr -d ' ')
export GEMINI_API_KEY=$(grep '^gemini=' .env | cut -d= -f2 | tr -d ' ')
# Anthropic from keychain (auto-resolved by cuervo)

CUERVO="./target/release/cuervo"
REPORT_FILE="docs/Provider_Test_Report.md"
PROMPT="Responde en una sola línea: ¿Cuál es la capital de Francia?"
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

# Results accumulator
declare -a RESULTS=()

test_model() {
    local provider="$1"
    local model="$2"
    local test_num="$3"
    local start_ms end_ms elapsed_ms status response

    echo "─── Test #${test_num}: ${provider}/${model} ───"
    start_ms=$(python3 -c 'import time; print(int(time.time()*1000))')

    # Run with 30s timeout, capture output + stderr
    set +e
    response=$(timeout 60 "$CUERVO" --no-banner -p "$provider" -m "$model" chat "$PROMPT" 2>/tmp/cuervo_stderr_${test_num}.txt)
    exit_code=$?
    set -e

    end_ms=$(python3 -c 'import time; print(int(time.time()*1000))')
    elapsed_ms=$((end_ms - start_ms))

    if [ $exit_code -eq 0 ] && [ -n "$response" ]; then
        status="SUCCESS"
        # Trim response to first 200 chars for report
        response_trimmed=$(echo "$response" | head -5 | cut -c1-200)
    elif [ $exit_code -eq 124 ]; then
        status="TIMEOUT"
        response_trimmed="Timed out after 60s"
    else
        status="FAILURE"
        response_trimmed=$(cat /tmp/cuervo_stderr_${test_num}.txt 2>/dev/null | tail -5 | cut -c1-200)
        if [ -z "$response_trimmed" ]; then
            response_trimmed="Exit code: $exit_code"
        fi
    fi

    echo "  Status:  $status"
    echo "  Latency: ${elapsed_ms}ms"
    echo "  Response: $(echo "$response_trimmed" | head -1)"
    echo ""

    # Store result
    RESULTS+=("${test_num}|${provider}|${model}|${status}|${elapsed_ms}|${response_trimmed}")
}

echo "╔═══════════════════════════════════════════════════╗"
echo "║  Cuervo CLI — Provider Integration Tests          ║"
echo "║  ${TIMESTAMP}                              ║"
echo "╚═══════════════════════════════════════════════════╝"
echo ""

# ── Individual Provider Tests ──────────────────────────────
echo "═══ PHASE 1: Individual Model Tests ═══"
echo ""

test_model "deepseek" "deepseek-chat" 1
test_model "deepseek" "deepseek-coder" 2
test_model "openai" "gpt-4o-mini" 3
test_model "openai" "gpt-4o" 4
test_model "gemini" "gemini-2.0-flash" 5
test_model "anthropic" "claude-sonnet-4-5-20250929" 6
test_model "ollama" "deepseek-coder-v2:latest" 7

# Reasoning models (no temperature)
echo "═══ PHASE 2: Reasoning Model Tests ═══"
echo ""
test_model "openai" "o3-mini" 8
test_model "deepseek" "deepseek-reasoner" 9

# ── Generate Report ────────────────────────────────────────
echo ""
echo "═══ GENERATING REPORT ═══"

{
    echo "# Cuervo CLI — Provider Integration Test Report"
    echo ""
    echo "**Date**: ${TIMESTAMP}"
    echo "**Build**: release ($(du -h "$CUERVO" | cut -f1))"
    echo "**Prompt**: \"${PROMPT}\""
    echo ""
    echo "## Individual Model Results"
    echo ""
    echo "| # | Provider | Model | Status | Latency (ms) | Response (truncated) |"
    echo "|---|----------|-------|--------|-------------|---------------------|"

    for result in "${RESULTS[@]}"; do
        IFS='|' read -r num prov mod stat lat resp <<< "$result"
        emoji=""
        case "$stat" in
            SUCCESS) emoji="OK" ;;
            FAILURE) emoji="FAIL" ;;
            TIMEOUT) emoji="TIMEOUT" ;;
        esac
        # Escape pipes in response
        resp_safe=$(echo "$resp" | head -1 | tr '|' '/')
        echo "| ${num} | ${prov} | ${mod} | ${emoji} | ${lat} | ${resp_safe} |"
    done

    echo ""
    echo "## Summary"
    echo ""
    success_count=0
    fail_count=0
    for result in "${RESULTS[@]}"; do
        IFS='|' read -r _ _ _ stat _ _ <<< "$result"
        if [ "$stat" = "SUCCESS" ]; then
            ((success_count++)) || true
        else
            ((fail_count++)) || true
        fi
    done
    total=${#RESULTS[@]}
    echo "- **Total tests**: ${total}"
    echo "- **Passed**: ${success_count}"
    echo "- **Failed**: ${fail_count}"
    echo "- **Success rate**: $(( success_count * 100 / total ))%"
    echo ""

    # Detailed errors
    echo "## Detailed Error Logs"
    echo ""
    for result in "${RESULTS[@]}"; do
        IFS='|' read -r num prov mod stat lat resp <<< "$result"
        if [ "$stat" != "SUCCESS" ]; then
            echo "### Test #${num}: ${prov}/${mod} — ${stat}"
            echo ""
            echo '```'
            cat /tmp/cuervo_stderr_${num}.txt 2>/dev/null || echo "(no stderr captured)"
            echo '```'
            echo ""
        fi
    done

} > "$REPORT_FILE"

echo "Report written to: $REPORT_FILE"
echo ""
echo "═══ DONE ═══"
