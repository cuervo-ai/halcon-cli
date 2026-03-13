#!/usr/bin/env sh
# Halcón CLI installer
# Usage: curl -sSfL https://halcon.cuervo.cloud/install.sh | sh
# Or:    curl -sSfL https://halcon.cuervo.cloud/install.sh | sh -s -- --version v0.3.0
set -e

HALCON_VERSION="${HALCON_VERSION:-latest}"
RELEASES_URL="${HALCON_RELEASES_URL:-https://releases.cli.cuervo.cloud}"
MANIFEST_URL="${RELEASES_URL}/latest/manifest.json"
INSTALL_DIR="${HALCON_INSTALL_DIR:-$HOME/.local/bin}"
BINARY_NAME="halcon"

# ─── Colors ────────────────────────────────────────────────────────────────
if [ -t 1 ] && command -v tput >/dev/null 2>&1; then
    RED="$(tput setaf 1)"
    GREEN="$(tput setaf 2)"
    YELLOW="$(tput setaf 3)"
    CYAN="$(tput setaf 6)"
    BOLD="$(tput bold)"
    RESET="$(tput sgr0)"
else
    RED="" GREEN="" YELLOW="" CYAN="" BOLD="" RESET=""
fi

info()  { printf "${CYAN}  →${RESET} %s\n" "$*"; }
ok()    { printf "${GREEN}  ✓${RESET} %s\n" "$*"; }
warn()  { printf "${YELLOW}  !${RESET} %s\n" "$*" >&2; }
error() { printf "${RED}  ✗${RESET} %s\n" "$*" >&2; exit 1; }

# ─── Parse args ────────────────────────────────────────────────────────────
parse_args() {
    while [ $# -gt 0 ]; do
        case "$1" in
            --version) HALCON_VERSION="$2"; shift 2 ;;
            --dir)     INSTALL_DIR="$2";    shift 2 ;;
            --help|-h)
                printf "Halcon CLI Installer\n\n"
                printf "Options:\n"
                printf "  --version VERSION   Install specific version (default: latest)\n"
                printf "  --dir DIR           Install directory (default: ~/.local/bin)\n"
                printf "\nExamples:\n"
                printf "  curl -sSfL https://halcon.cuervo.cloud/install.sh | sh\n"
                printf "  curl -sSfL https://halcon.cuervo.cloud/install.sh | sh -s -- --version v0.3.0\n"
                exit 0 ;;
            *) error "Unknown argument: $1" ;;
        esac
    done
}

# ─── Platform detection ─────────────────────────────────────────────────────
detect_platform() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    case "$OS" in
        Linux)
            case "$ARCH" in
                x86_64)  TARGET="x86_64-unknown-linux-musl" ;;
                aarch64|arm64) TARGET="aarch64-unknown-linux-gnu" ;;
                armv7l)  TARGET="armv7-unknown-linux-musleabihf" ;;
                *) error "Unsupported Linux architecture: $ARCH" ;;
            esac
            EXT="tar.gz"
            ;;
        Darwin)
            case "$ARCH" in
                x86_64)  TARGET="x86_64-apple-darwin" ;;
                arm64)   TARGET="aarch64-apple-darwin" ;;
                *) error "Unsupported macOS architecture: $ARCH" ;;
            esac
            EXT="tar.gz"
            ;;
        *) error "Unsupported OS: $OS. Use install.ps1 on Windows." ;;
    esac

    info "Platform: ${OS} ${ARCH} → ${TARGET}"
}

# ─── Download helpers ───────────────────────────────────────────────────────
download() {
    local url="$1"
    local dest="$2"
    if command -v curl >/dev/null 2>&1; then
        curl -sSfL --retry 3 --retry-delay 2 -o "$dest" "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget -q --tries=3 -O "$dest" "$url"
    else
        error "Neither curl nor wget found. Install one and retry."
    fi
}

# ─── SHA-256 verification ───────────────────────────────────────────────────
verify_sha256() {
    local file="$1"
    local expected="$2"

    if command -v sha256sum >/dev/null 2>&1; then
        actual="$(sha256sum "$file" | cut -d' ' -f1)"
    elif command -v shasum >/dev/null 2>&1; then
        actual="$(shasum -a 256 "$file" | cut -d' ' -f1)"
    else
        warn "Cannot verify SHA-256: sha256sum/shasum not found"
        return 0
    fi

    if [ "$actual" != "$expected" ]; then
        error "SHA-256 mismatch!\n  Expected: $expected\n  Got:      $actual"
    fi
    ok "SHA-256 verified"
}

# ─── Main ────────────────────────────────────────────────────────────────────
main() {
    parse_args "$@"
    detect_platform

    printf "\n${BOLD}  Halcon CLI Installer${RESET}\n"
    printf "  ──────────────────────\n\n"

    TMPDIR_WORK="$(mktemp -d)"
    trap 'rm -rf "$TMPDIR_WORK"' EXIT

    # ─── Resolve version ────────────────────────────────────────────────────
    # Strip leading 'v' for consistency
    REQUESTED_VERSION="$(echo "$HALCON_VERSION" | sed 's/^v//')"

    if [ "$REQUESTED_VERSION" = "latest" ]; then
        info "Fetching release manifest..."
        MANIFEST_FILE="$TMPDIR_WORK/manifest.json"
        download "$MANIFEST_URL" "$MANIFEST_FILE"

        # Check for error response
        if grep -q '"error"' "$MANIFEST_FILE"; then
            ERR="$(grep -o '"error": *"[^"]*"' "$MANIFEST_FILE" | sed 's/.*"\([^"]*\)".*/\1/')"
            error "Release API error: ${ERR}. Check https://releases.cli.cuervo.cloud/health"
        fi

        VERSION="$(grep -o '"version": *"[^"]*"' "$MANIFEST_FILE" | head -1 | sed 's/.*"\([^"]*\)".*/\1/')"
        if [ -z "$VERSION" ]; then
            error "Failed to parse version from manifest. Manifest content: $(cat "$MANIFEST_FILE" | head -5)"
        fi
        info "Latest version: ${VERSION}"
    else
        VERSION="$REQUESTED_VERSION"
        info "Installing version: ${VERSION}"
    fi

    # ─── Build artifact URL ─────────────────────────────────────────────────
    ARTIFACT_NAME="halcon-${VERSION}-${TARGET}.${EXT}"
    if [ "$REQUESTED_VERSION" = "latest" ]; then
        DOWNLOAD_URL="${RELEASES_URL}/latest/${ARTIFACT_NAME}"
    else
        DOWNLOAD_URL="${RELEASES_URL}/v${VERSION}/${ARTIFACT_NAME}"
    fi

    # ─── Fetch SHA-256 from checksums.txt ───────────────────────────────────
    EXPECTED_SHA=""
    if [ "$REQUESTED_VERSION" = "latest" ]; then
        CS_URL="${RELEASES_URL}/latest/checksums.txt"
    else
        CS_URL="${RELEASES_URL}/v${VERSION}/checksums.txt"
    fi
    CS_FILE="$TMPDIR_WORK/checksums.txt"
    if download "$CS_URL" "$CS_FILE" 2>/dev/null; then
        EXPECTED_SHA="$(grep "${ARTIFACT_NAME}" "$CS_FILE" | awk '{print $1}' | head -1)"
    fi

    # ─── Download artifact ──────────────────────────────────────────────────
    info "Downloading ${ARTIFACT_NAME}..."
    ARCHIVE_FILE="$TMPDIR_WORK/${ARTIFACT_NAME}"
    download "$DOWNLOAD_URL" "$ARCHIVE_FILE" || \
        error "Download failed. Artifact '${ARTIFACT_NAME}' may not exist for version ${VERSION}.\nCheck: ${RELEASES_URL}/latest/manifest.json"
    ok "Download complete ($(du -sh "$ARCHIVE_FILE" | cut -f1))"

    # ─── Verify SHA-256 ─────────────────────────────────────────────────────
    if [ -n "$EXPECTED_SHA" ]; then
        info "Verifying SHA-256..."
        verify_sha256 "$ARCHIVE_FILE" "$EXPECTED_SHA"
    else
        warn "SHA-256 not in checksums.txt, skipping verification"
    fi

    # ─── Extract binary ─────────────────────────────────────────────────────
    info "Extracting..."
    EXTRACT_DIR="$TMPDIR_WORK/extract"
    mkdir -p "$EXTRACT_DIR"
    tar xzf "$ARCHIVE_FILE" -C "$EXTRACT_DIR"

    # Find binary: may be at top-level or inside a subdirectory
    BINARY_SRC=""
    for candidate in \
        "$EXTRACT_DIR/${BINARY_NAME}" \
        "$EXTRACT_DIR/halcon-${VERSION}-${TARGET}/${BINARY_NAME}" \
        "$EXTRACT_DIR/${BINARY_NAME}-${VERSION}-${TARGET}/${BINARY_NAME}"
    do
        if [ -f "$candidate" ]; then
            BINARY_SRC="$candidate"
            break
        fi
    done

    # Fallback: find any executable named 'cuervo' in the extracted tree
    if [ -z "$BINARY_SRC" ]; then
        BINARY_SRC="$(find "$EXTRACT_DIR" -name "${BINARY_NAME}" -type f 2>/dev/null | head -1)"
    fi

    if [ -z "$BINARY_SRC" ]; then
        error "Binary '${BINARY_NAME}' not found in archive. Contents: $(ls -la "$EXTRACT_DIR" 2>/dev/null)"
    fi

    # ─── Install ─────────────────────────────────────────────────────────────
    mkdir -p "$INSTALL_DIR"
    chmod +x "$BINARY_SRC"
    mv "$BINARY_SRC" "${INSTALL_DIR}/${BINARY_NAME}"
    ok "Installed to ${INSTALL_DIR}/${BINARY_NAME}"

    # ─── Verify ──────────────────────────────────────────────────────────────
    if "${INSTALL_DIR}/${BINARY_NAME}" --version >/dev/null 2>&1; then
        ok "Halcon CLI ${VERSION} installed successfully!"
    else
        warn "Binary installed but could not verify execution"
    fi

    # ─── PATH hint ───────────────────────────────────────────────────────────
    case ":${PATH}:" in
        *":${INSTALL_DIR}:"*) ;;
        *)
            printf "\n${YELLOW}  Add to PATH:${RESET}\n"
            printf "    export PATH=\"\$PATH:${INSTALL_DIR}\"\n"
            printf "\n  Or add to your shell config (~/.bashrc, ~/.zshrc, etc.)\n"
            ;;
    esac

    printf "\n${GREEN}${BOLD}  Get started:${RESET}\n"
    printf "    halcon --help\n"
    printf "    halcon chat \"Hello, Halcón!\"\n\n"
}

main "$@"
