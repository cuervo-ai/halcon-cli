#!/usr/bin/env bash
set -euo pipefail

echo "🚀 Halcon CLI Installation & Validation Script"
echo "=============================================="
echo ""

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

INSTALL_DIR="$HOME/.local/bin"
BINARY_NAME="halcon"
SOURCE_BINARY="./target/release/$BINARY_NAME"
DEST_BINARY="$INSTALL_DIR/$BINARY_NAME"

# Step 1: Check if release binary exists
echo -e "${BLUE}[1/7]${NC} Checking for compiled binary..."
if [ ! -f "$SOURCE_BINARY" ]; then
    echo -e "${RED}✗ Error: Release binary not found at $SOURCE_BINARY${NC}"
    echo "Please run: cargo build --release --package halcon-cli"
    exit 1
fi
echo -e "${GREEN}✓ Binary found: $SOURCE_BINARY${NC}"
ls -lh "$SOURCE_BINARY"
echo ""

# Step 2: Backup old version if exists
echo -e "${BLUE}[2/7]${NC} Checking for existing installation..."
if [ -f "$DEST_BINARY" ]; then
    OLD_VERSION=$($DEST_BINARY --version 2>&1 || echo "unknown")
    echo -e "${YELLOW}⚠ Found existing installation:${NC}"
    echo "  Version: $OLD_VERSION"
    echo "  Location: $DEST_BINARY"

    BACKUP_FILE="$DEST_BINARY.backup.$(date +%Y%m%d_%H%M%S)"
    echo -e "${BLUE}  Creating backup: $BACKUP_FILE${NC}"
    cp "$DEST_BINARY" "$BACKUP_FILE"
    echo -e "${GREEN}✓ Backup created${NC}"
else
    echo -e "${GREEN}✓ No existing installation found${NC}"
fi
echo ""

# Step 3: Ensure install directory exists
echo -e "${BLUE}[3/7]${NC} Ensuring install directory exists..."
mkdir -p "$INSTALL_DIR"
echo -e "${GREEN}✓ Directory ready: $INSTALL_DIR${NC}"
echo ""

# Step 4: Install new binary
echo -e "${BLUE}[4/7]${NC} Installing new binary..."
cp "$SOURCE_BINARY" "$DEST_BINARY"
chmod +x "$DEST_BINARY"
echo -e "${GREEN}✓ Installed to: $DEST_BINARY${NC}"
ls -lh "$DEST_BINARY"
echo ""

# Step 5: Verify PATH
echo -e "${BLUE}[5/7]${NC} Verifying PATH configuration..."
if echo "$PATH" | grep -q "$INSTALL_DIR"; then
    echo -e "${GREEN}✓ $INSTALL_DIR is in PATH${NC}"
else
    echo -e "${YELLOW}⚠ Warning: $INSTALL_DIR is not in PATH${NC}"
    echo "  Add this to your shell profile (~/.zshrc or ~/.bashrc):"
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
fi
echo ""

# Step 6: Test binary execution
echo -e "${BLUE}[6/7]${NC} Testing binary execution..."

# Test 1: Version
echo -ne "  Testing --version... "
VERSION_OUTPUT=$($DEST_BINARY --version 2>&1)
if [ $? -eq 0 ]; then
    echo -e "${GREEN}✓${NC}"
    echo "    $VERSION_OUTPUT"
else
    echo -e "${RED}✗ Failed${NC}"
    exit 1
fi

# Test 2: Help
echo -ne "  Testing --help... "
if $DEST_BINARY --help > /dev/null 2>&1; then
    echo -e "${GREEN}✓${NC}"
else
    echo -e "${RED}✗ Failed${NC}"
    exit 1
fi

# Test 3: Commands available
echo -ne "  Checking available commands... "
HELP_OUTPUT=$($DEST_BINARY --help 2>&1)
if echo "$HELP_OUTPUT" | grep -q "Commands:"; then
    echo -e "${GREEN}✓${NC}"
    echo "$HELP_OUTPUT" | grep -A 20 "Commands:" | head -15 | sed 's/^/    /'
else
    echo -e "${RED}✗ Failed${NC}"
    exit 1
fi

echo ""

# Step 7: Final validation
echo -e "${BLUE}[7/7]${NC} Final validation..."
INSTALLED_VERSION=$($DEST_BINARY --version 2>&1)
echo "  Installed version: $INSTALLED_VERSION"

# Check binary size (should be reasonable)
BINARY_SIZE=$(stat -f%z "$DEST_BINARY" 2>/dev/null || stat -c%s "$DEST_BINARY" 2>/dev/null)
BINARY_SIZE_MB=$((BINARY_SIZE / 1024 / 1024))
echo "  Binary size: ${BINARY_SIZE_MB}MB"

if [ $BINARY_SIZE_MB -lt 5 ]; then
    echo -e "${YELLOW}⚠ Warning: Binary seems small (${BINARY_SIZE_MB}MB)${NC}"
elif [ $BINARY_SIZE_MB -gt 100 ]; then
    echo -e "${YELLOW}⚠ Warning: Binary seems large (${BINARY_SIZE_MB}MB)${NC}"
else
    echo -e "${GREEN}✓ Binary size is reasonable${NC}"
fi

echo ""
echo -e "${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${GREEN}✓ Installation completed successfully!${NC}"
echo -e "${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "To verify the installation, run:"
echo "  halcon --version"
echo "  halcon --help"
echo ""
echo "Quick start:"
echo "  halcon chat         # Start interactive chat"
echo "  halcon serve        # Start API server"
echo "  halcon --help       # See all commands"
echo ""

# Clean up old backups (keep only last 3)
echo -e "${BLUE}Cleaning up old backups...${NC}"
BACKUP_COUNT=$(ls -1 "$INSTALL_DIR/$BINARY_NAME.backup."* 2>/dev/null | wc -l)
if [ "$BACKUP_COUNT" -gt 3 ]; then
    ls -1t "$INSTALL_DIR/$BINARY_NAME.backup."* | tail -n +4 | xargs rm -f
    echo -e "${GREEN}✓ Cleaned up old backups (kept last 3)${NC}"
fi
echo ""

exit 0
