#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────────────
# scripts/build-linux-docker.sh
#
# Build halcon for Linux targets using plain Docker (no cross/emulation issues).
# Works from Apple Silicon (arm64) macOS.
#
# Targets:
#   aarch64-unknown-linux-gnu  — Linux ARM64 (Raspberry Pi 4/5, Ampere, Graviton)
#   x86_64-unknown-linux-gnu   — Linux x86_64 (Ubuntu, Debian, RHEL, Arch…)
#
# Prerequisites:
#   Docker Desktop running (or colima)
#   Zuclubit workspace at ../Zuclubit relative to this repo
#
# Usage:
#   ./scripts/build-linux-docker.sh [TARGET] [--release|--debug]
#
# Examples:
#   ./scripts/build-linux-docker.sh                            # ARM64, release
#   ./scripts/build-linux-docker.sh aarch64-unknown-linux-gnu
#   ./scripts/build-linux-docker.sh x86_64-unknown-linux-gnu
# ──────────────────────────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

TARGET="${1:-aarch64-unknown-linux-gnu}"
PROFILE_FLAG="${2:---release}"
PROFILE_DIR="$( [[ "$PROFILE_FLAG" == "--release" ]] && echo "release" || echo "debug" )"

# ── Resolve Docker platform ───────────────────────────────────────────────────
case "$TARGET" in
    aarch64-unknown-linux-gnu | aarch64-unknown-linux-musl)
        DOCKER_PLATFORM="linux/arm64"
        ;;
    x86_64-unknown-linux-gnu | x86_64-unknown-linux-musl)
        DOCKER_PLATFORM="linux/amd64"
        ;;
    *)
        echo "ERROR: unsupported target '$TARGET'"
        exit 1
        ;;
esac

# ── Features per target ───────────────────────────────────────────────────────
# tui requires arboard (clipboard) which needs X11 headers on Linux.
# We build without tui for headless server environments.
# Users can still use the full TUI if they run halcon in a desktop terminal.
FEATURES="headless"

# ── Build metadata ────────────────────────────────────────────────────────────
GIT_HASH=$(git -C "$WORKSPACE_ROOT" rev-parse --short=8 HEAD 2>/dev/null || echo "local")
BUILD_DATE=$(date -u +%Y-%m-%d)
VERSION=$(grep '^version' "$WORKSPACE_ROOT/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')
ZUCLUBIT_DIR="$(cd "$WORKSPACE_ROOT/.." && pwd)/Zuclubit"

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo " halcon v${VERSION} — Docker build for ${TARGET}"
echo " Platform   : ${DOCKER_PLATFORM}"
echo " Profile    : ${PROFILE_FLAG}"
echo " Features   : ${FEATURES}"
echo " Git hash   : ${GIT_HASH}   Build date: ${BUILD_DATE}"
echo " Zuclubit   : ${ZUCLUBIT_DIR}"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

if [ ! -d "$ZUCLUBIT_DIR" ]; then
    echo "ERROR: Zuclubit directory not found at $ZUCLUBIT_DIR"
    echo "  The workspace requires momoto-* crates from the Zuclubit monorepo."
    exit 1
fi

# ── Docker ────────────────────────────────────────────────────────────────────
DOCKER_SCRIPT=$(cat <<'DOCKERSCRIPT'
set -euo pipefail

TARGET="$1"
PROFILE_FLAG="$2"
PROFILE_DIR="$3"
FEATURES="$4"
GIT_HASH="$5"
BUILD_DATE="$6"
VERSION="$7"

echo "[docker] Installing system dependencies..."
apt-get update -qq
apt-get install -y -qq \
    build-essential \
    cmake \
    pkg-config \
    libssl-dev \
    libdbus-1-dev \
    libxcb1-dev \
    libxcb-render0-dev \
    libxcb-shape0-dev \
    libxcb-xfixes0-dev \
    rsync \
    ca-certificates \
    2>&1 | tail -5

echo "[docker] Source directory contents:"
ls /src/ | head -10

# Copy source (excluding target/ to avoid macOS build artifacts)
echo "[docker] Copying source to /build (excluding target/)..."
rsync -a --exclude=target/ --exclude=.git/ /src/ /build/

# Zuclubit path: workspace Cargo.toml references ../Zuclubit/...
# We mounted it at /Zuclubit (which is /build/../Zuclubit)
mkdir -p /Zuclubit
rsync -a /zuclubit-src/ /Zuclubit/

cd /build

echo "[docker] Rust version: $(rustc --version)"
echo "[docker] Target: $TARGET"

# Add rustup target
rustup target add "$TARGET" 2>/dev/null || true

# Set up build env
export HALCON_GIT_HASH="$GIT_HASH"
export HALCON_BUILD_DATE="$BUILD_DATE"
export ORT_STRATEGY="download"
export LIBGIT2_SYS_USE_PKG_CONFIG="0"
export LIBGIT2_STATIC="1"
export ZSTD_SYS_USE_PKG_CONFIG="0"

# Build
BUILD_CMD="cargo build $PROFILE_FLAG --target $TARGET -p halcon-cli --no-default-features --features $FEATURES"
echo "[docker] Running: $BUILD_CMD"
eval "$BUILD_CMD"

# Find and copy binary out
BINARY="/build/target/${TARGET}/${PROFILE_DIR}/halcon"
if [ -f "$BINARY" ]; then
    cp "$BINARY" /output/halcon
    echo "[docker] Binary copied to /output/halcon"
else
    echo "[docker] ERROR: binary not found at $BINARY"
    exit 1
fi
DOCKERSCRIPT
)

# ── Create output directory ───────────────────────────────────────────────────
DIST_DIR="${WORKSPACE_ROOT}/dist"
ARTIFACT_NAME="halcon-${VERSION}-${TARGET}"
PKG_DIR="${DIST_DIR}/${ARTIFACT_NAME}"
mkdir -p "$PKG_DIR"

echo "[host] Starting Docker container..."
docker run --rm \
    --platform "$DOCKER_PLATFORM" \
    --mount "type=bind,src=${WORKSPACE_ROOT},dst=/src,readonly" \
    --mount "type=bind,src=${ZUCLUBIT_DIR},dst=/zuclubit-src,readonly" \
    --mount "type=bind,src=${PKG_DIR},dst=/output" \
    rust:1.83-bookworm \
    bash -c "$DOCKER_SCRIPT" -- \
        "$TARGET" "$PROFILE_FLAG" "$PROFILE_DIR" "$FEATURES" \
        "$GIT_HASH" "$BUILD_DATE" "$VERSION"

# ── Package ───────────────────────────────────────────────────────────────────
echo "[host] Packaging..."
[ -f "${WORKSPACE_ROOT}/README.md" ] && cp "${WORKSPACE_ROOT}/README.md" "$PKG_DIR/" || true
[ -f "${WORKSPACE_ROOT}/LICENSE" ]   && cp "${WORKSPACE_ROOT}/LICENSE"   "$PKG_DIR/" || true

ARCHIVE="${DIST_DIR}/${ARTIFACT_NAME}.tar.gz"
tar czf "$ARCHIVE" -C "$DIST_DIR" "$ARTIFACT_NAME"
rm -rf "$PKG_DIR"

# SHA-256
SHA_FILE="${ARCHIVE}.sha256"
if command -v sha256sum &>/dev/null; then
    sha256sum "$ARCHIVE" > "$SHA_FILE"
else
    shasum -a 256 "$ARCHIVE" > "$SHA_FILE"
fi

# ── Summary ───────────────────────────────────────────────────────────────────
ARCHIVE_SIZE=$(du -sh "$ARCHIVE" | cut -f1)
SHA=$(awk '{print $1}' "$SHA_FILE")

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo " Build complete"
echo "   Archive : $ARCHIVE  ($ARCHIVE_SIZE)"
echo "   SHA-256 : $SHA"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
