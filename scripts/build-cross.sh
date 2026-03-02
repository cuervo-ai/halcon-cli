#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────────────
# scripts/build-cross.sh
#
# Cross-compile halcon for Linux targets using `cross` (Docker-based).
# Supported targets:
#   x86_64-unknown-linux-musl   — static binary, broadest Linux compatibility
#   x86_64-unknown-linux-gnu    — dynamic (glibc ≥ 2.17), easiest for ONNX RT
#   aarch64-unknown-linux-musl  — static ARM64 (Alpine / Docker)
#   aarch64-unknown-linux-gnu   — dynamic ARM64 (Raspberry Pi 4, Ampere, etc.)
#
# Prerequisites:
#   cargo install cross --git https://github.com/cross-rs/cross --locked
#   Docker (or Podman with DOCKER_HOST set)
#   rustup target add <TARGET>
#
# Usage:
#   ./scripts/build-cross.sh [TARGET] [--release|--debug]
#
#   TARGET defaults to: x86_64-unknown-linux-musl
#   Profile defaults to: --release
#
# Examples:
#   ./scripts/build-cross.sh
#   ./scripts/build-cross.sh x86_64-unknown-linux-gnu
#   ./scripts/build-cross.sh aarch64-unknown-linux-gnu
#   ./scripts/build-cross.sh aarch64-unknown-linux-musl --release
# ──────────────────────────────────────────────────────────────────────────────
set -euo pipefail

# ── Resolve workspace root ────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$WORKSPACE_ROOT"

# ── Defaults ──────────────────────────────────────────────────────────────────
TARGET="${1:-x86_64-unknown-linux-musl}"
PROFILE_FLAG="${2:---release}"

# ── Feature matrix per target ─────────────────────────────────────────────────
# - color-science is always excluded (requires local momoto submodule)
# - tui enabled for x86_64 targets only (arboard needs X11/Wayland headers at
#   runtime; musl cross images bundle the necessary static libs)
# - ARM targets: no tui (clipboard libs not available in cross image)
case "$TARGET" in
    x86_64-unknown-linux-musl)
        FEATURES="tui"
        ;;
    x86_64-unknown-linux-gnu)
        FEATURES="tui"
        ;;
    aarch64-unknown-linux-musl | aarch64-unknown-linux-gnu | armv7-unknown-linux-musleabihf)
        FEATURES=""
        ;;
    *)
        echo "WARN: unknown target '$TARGET', using no features"
        FEATURES=""
        ;;
esac

# ── Build metadata ────────────────────────────────────────────────────────────
GIT_HASH=$(git rev-parse --short=8 HEAD 2>/dev/null || echo "local")
BUILD_DATE=$(date -u +%Y-%m-%d 2>/dev/null || echo "unknown")
VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')

export HALCON_GIT_HASH="$GIT_HASH"
export HALCON_BUILD_DATE="$BUILD_DATE"

# ── ONNX Runtime config for fastembed / ort ───────────────────────────────────
# The `ort` crate (used by fastembed in halcon-search) downloads or links
# ONNX Runtime. For cross-compiled Linux targets we instruct ort to use its
# bundled copy compiled by the cross image; disable GPU/CUDA providers.
export ORT_STRATEGY="download"          # let ort download the correct prebuilt
export ORT_USE_CUDA="0"
export CARGO_FEATURE_LOAD_DYNAMIC=""    # static link (default)

# For glibc targets ort downloads official ONNX Runtime binaries.
# For musl targets ort compiles from source inside the cross container
# (cmake and a C toolchain are present in the cross-rs musl images).
if [[ "$TARGET" == *"musl"* ]]; then
    export ORT_STRATEGY="compile"       # compile from source inside container
fi

# ── Dependency: libgit2 (git2 crate) ─────────────────────────────────────────
# Disable pkg-config lookup; let the cross image's bundled cmake build
# libgit2 from source (git2-sys includes vendored sources when pkg-config fails).
export LIBGIT2_SYS_USE_PKG_CONFIG="0"
export LIBGIT2_STATIC="1"

# ── Dependency: zstd ─────────────────────────────────────────────────────────
export ZSTD_SYS_USE_PKG_CONFIG="0"

# ── Verify cross is installed ─────────────────────────────────────────────────
if ! command -v cross &>/dev/null; then
    echo "ERROR: 'cross' not found."
    echo "  Install with: cargo install cross --git https://github.com/cross-rs/cross --locked"
    exit 1
fi

# ── Verify Docker is running ──────────────────────────────────────────────────
if ! docker info &>/dev/null 2>&1; then
    echo "ERROR: Docker daemon is not running (or not accessible)."
    echo "  Start Docker Desktop / colima / docker daemon and retry."
    exit 1
fi

# ── Add rustup target ─────────────────────────────────────────────────────────
rustup target add "$TARGET" 2>/dev/null || true

# ── Build ──────────────────────────────────────────────────────────────────────
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo " halcon v${VERSION} — cross-compile for ${TARGET}"
echo " Profile : ${PROFILE_FLAG}"
echo " Features: ${FEATURES:-<none>}"
echo " Git hash: ${GIT_HASH}   Build date: ${BUILD_DATE}"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

BUILD_CMD="cross build ${PROFILE_FLAG} --target ${TARGET} -p halcon-cli"

if [ -n "$FEATURES" ]; then
    BUILD_CMD="$BUILD_CMD --no-default-features --features ${FEATURES}"
else
    BUILD_CMD="$BUILD_CMD --no-default-features"
fi

echo "Running: $BUILD_CMD"
eval "$BUILD_CMD"

# ── Locate binary ─────────────────────────────────────────────────────────────
if [ "$PROFILE_FLAG" = "--release" ]; then
    PROFILE_DIR="release"
else
    PROFILE_DIR="debug"
fi

BINARY="target/${TARGET}/${PROFILE_DIR}/halcon"

if [ ! -f "$BINARY" ]; then
    echo "ERROR: expected binary not found at $BINARY"
    exit 1
fi

# ── Package ───────────────────────────────────────────────────────────────────
ARTIFACT_NAME="halcon-${VERSION}-${TARGET}"
DIST_DIR="${WORKSPACE_ROOT}/dist"
mkdir -p "$DIST_DIR"

PKG_DIR="${DIST_DIR}/${ARTIFACT_NAME}"
mkdir -p "$PKG_DIR"

cp "$BINARY" "${PKG_DIR}/halcon"
[ -f README.md ] && cp README.md "$PKG_DIR/" || true
[ -f LICENSE ]   && cp LICENSE   "$PKG_DIR/" || true

ARCHIVE="${DIST_DIR}/${ARTIFACT_NAME}.tar.gz"
tar czf "$ARCHIVE" -C "$DIST_DIR" "$ARTIFACT_NAME"

# SHA-256
SHA_FILE="${ARCHIVE}.sha256"
if command -v sha256sum &>/dev/null; then
    sha256sum "$ARCHIVE" | awk '{print $1}' > "$SHA_FILE"
else
    shasum -a 256 "$ARCHIVE" | awk '{print $1}' > "$SHA_FILE"
fi

# Cleanup staging dir
rm -rf "$PKG_DIR"

# ── Summary ───────────────────────────────────────────────────────────────────
BINARY_SIZE=$(du -sh "$BINARY" | cut -f1)
ARCHIVE_SIZE=$(du -sh "$ARCHIVE" | cut -f1)
SHA=$(cat "$SHA_FILE")

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo " Build complete"
echo "   Binary : $BINARY  ($BINARY_SIZE)"
echo "   Archive: $ARCHIVE  ($ARCHIVE_SIZE)"
echo "   SHA-256: $SHA"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
