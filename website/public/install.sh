#!/usr/bin/env sh
# Halcón CLI installer
# Usage: curl -sSfL https://halcon.cuervo.cloud/install.sh | sh
# Or:    curl -sSfL https://halcon.cuervo.cloud/install.sh | sh -s -- --version v0.3.0
set -e

HALCON_VERSION="${HALCON_VERSION:-latest}"
RELEASES_URL="${HALCON_RELEASES_URL:-https://releases.cli.cuervo.cloud}"
MANIFEST_URL="${RELEASES_URL}/latest/manifest.json"
INSTALL_DIR="${HALCON_INSTALL_DIR:-}"
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
                printf "  --dir DIR           Install directory (default: auto-detected)\n"
                printf "\nExamples:\n"
                printf "  curl -sSfL https://halcon.cuervo.cloud/install.sh | sh\n"
                printf "  curl -sSfL https://halcon.cuervo.cloud/install.sh | sh -s -- --version v0.3.0\n"
                printf "  curl -sSfL https://halcon.cuervo.cloud/install.sh | sh -s -- --dir /usr/local/bin\n"
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

# ─── Install directory resolution ──────────────────────────────────────────
# Finds a writable install directory, fixing ownership if needed.
# Priority: user-specified > ~/.local/bin > ~/bin > /usr/local/bin
resolve_install_dir() {
    # If user explicitly specified --dir, honor it (create + fail loudly if unwritable)
    if [ -n "$INSTALL_DIR" ]; then
        _ensure_writable_dir "$INSTALL_DIR" || \
            error "Cannot write to --dir '${INSTALL_DIR}'. Check permissions."
        return
    fi

    # Candidates in preference order
    for candidate in \
        "$HOME/.local/bin" \
        "$HOME/bin" \
        "/usr/local/bin"
    do
        if _ensure_writable_dir "$candidate" 2>/dev/null; then
            INSTALL_DIR="$candidate"
            return
        fi
    done

    error "No writable install directory found. Try: --dir \$HOME/bin"
}

# Returns 0 if dir is (or can be made) writable by the current user.
# Fixes root-owned directories under $HOME by reclaiming ownership.
_ensure_writable_dir() {
    local dir="$1"

    # Does it exist?
    if [ -d "$dir" ]; then
        # Writable already → done
        if [ -w "$dir" ]; then
            return 0
        fi

        # Under $HOME and owned by root? Reclaim it.
        case "$dir" in
            "$HOME"*)
                _dir_owner="$(ls -ld "$dir" 2>/dev/null | awk '{print $3}')"
                if [ "$_dir_owner" = "root" ]; then
                    warn "Fixing ownership of ${dir} (was owned by root from a previous sudo install)"
                    if command -v sudo >/dev/null 2>&1; then
                        sudo chown "$(id -un)" "$dir" 2>/dev/null && return 0
                    fi
                fi
                ;;
        esac
        return 1
    fi

    # Does not exist — try to create it
    if mkdir -p "$dir" 2>/dev/null; then
        return 0
    fi

    # Under $HOME it might need a sudo mkdir if a parent is root-owned
    case "$dir" in
        "$HOME"*)
            if command -v sudo >/dev/null 2>&1; then
                sudo mkdir -p "$dir" 2>/dev/null && \
                sudo chown "$(id -un)" "$dir" 2>/dev/null && \
                return 0
            fi
            ;;
    esac

    return 1
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

# ─── PATH configuration ──────────────────────────────────────────────────────
configure_path() {
    local dir="$1"

    # Already in PATH
    case ":${PATH}:" in
        *":${dir}:"*)
            ok "Already in PATH — no shell config change needed"
            return
            ;;
    esac

    local export_line="export PATH=\"\$PATH:${dir}\""
    local rc_file=""

    SHELL_NAME="$(basename "${SHELL:-sh}")"
    case "$SHELL_NAME" in
        zsh)  rc_file="$HOME/.zshrc" ;;
        bash) rc_file="$HOME/.bashrc" ;;
        fish)
            rc_file="$HOME/.config/fish/config.fish"
            export_line="fish_add_path ${dir}"
            ;;
        *)    rc_file="$HOME/.profile" ;;
    esac

    # Create rc file if it doesn't exist
    if [ ! -f "$rc_file" ]; then
        mkdir -p "$(dirname "$rc_file")" 2>/dev/null || true
        touch "$rc_file" 2>/dev/null || true
    fi

    if [ -f "$rc_file" ] && grep -qF "$dir" "$rc_file" 2>/dev/null; then
        ok "PATH already in ${rc_file}"
    elif [ -w "$rc_file" ] || [ -w "$(dirname "$rc_file")" ]; then
        printf '\n# Halcon CLI\n%s\n' "$export_line" >> "$rc_file"
        ok "PATH added to ${rc_file}"
    else
        warn "Could not update ${rc_file} — add manually:"
        warn "  ${export_line}"
    fi

    printf "\n${YELLOW}  To use halcon in this terminal session:${RESET}\n"
    printf "    ${BOLD}export PATH=\"\$PATH:${dir}\"${RESET}\n"
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
    REQUESTED_VERSION="$(printf '%s' "$HALCON_VERSION" | sed 's/^v//')"

    if [ "$REQUESTED_VERSION" = "latest" ]; then
        info "Fetching release manifest..."
        MANIFEST_FILE="$TMPDIR_WORK/manifest.json"
        download "$MANIFEST_URL" "$MANIFEST_FILE"

        if grep -q '"error"' "$MANIFEST_FILE" 2>/dev/null; then
            ERR="$(grep -o '"error": *"[^"]*"' "$MANIFEST_FILE" | sed 's/.*"\([^"]*\)".*/\1/')"
            error "Release API error: ${ERR}. Check https://releases.cli.cuervo.cloud/health"
        fi

        VERSION="$(grep -o '"version": *"[^"]*"' "$MANIFEST_FILE" | head -1 | sed 's/.*"\([^"]*\)".*/\1/')"
        if [ -z "$VERSION" ]; then
            error "Failed to parse version from manifest"
        fi
        info "Latest version: ${VERSION}"
    else
        VERSION="$REQUESTED_VERSION"
        info "Installing version: ${VERSION}"
    fi

    # ─── Resolve install directory ───────────────────────────────────────────
    resolve_install_dir
    info "Install directory: ${INSTALL_DIR}"

    # ─── Build artifact URL ─────────────────────────────────────────────────
    ARTIFACT_NAME="halcon-${VERSION}-${TARGET}.${EXT}"
    if [ "$REQUESTED_VERSION" = "latest" ]; then
        DOWNLOAD_URL="${RELEASES_URL}/latest/${ARTIFACT_NAME}"
    else
        DOWNLOAD_URL="${RELEASES_URL}/v${VERSION}/${ARTIFACT_NAME}"
    fi

    # ─── Fetch SHA-256 ──────────────────────────────────────────────────────
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
        error "Download failed. Check: ${RELEASES_URL}/latest/manifest.json"
    ok "Downloaded ($(du -sh "$ARCHIVE_FILE" | cut -f1))"

    # ─── Verify SHA-256 ─────────────────────────────────────────────────────
    if [ -n "$EXPECTED_SHA" ]; then
        info "Verifying SHA-256..."
        verify_sha256 "$ARCHIVE_FILE" "$EXPECTED_SHA"
    else
        warn "SHA-256 not available, skipping verification"
    fi

    # ─── Extract binary ─────────────────────────────────────────────────────
    info "Extracting..."
    EXTRACT_DIR="$TMPDIR_WORK/extract"
    mkdir -p "$EXTRACT_DIR"
    tar xzf "$ARCHIVE_FILE" -C "$EXTRACT_DIR"

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

    if [ -z "$BINARY_SRC" ]; then
        BINARY_SRC="$(find "$EXTRACT_DIR" -name "${BINARY_NAME}" -type f 2>/dev/null | head -1)"
    fi

    if [ -z "$BINARY_SRC" ]; then
        error "Binary '${BINARY_NAME}' not found in archive. Contents: $(ls -la "$EXTRACT_DIR" 2>/dev/null)"
    fi

    # ─── Install ─────────────────────────────────────────────────────────────
    DEST="${INSTALL_DIR}/${BINARY_NAME}"
    chmod +x "$BINARY_SRC"

    # Remove existing binary first — if it's root-owned the cp would fail,
    # but rm succeeds as long as the parent directory is user-writable.
    rm -f "$DEST" 2>/dev/null || true

    # Use cp instead of mv — avoids cross-device and permission edge cases
    if cp "$BINARY_SRC" "$DEST" 2>/dev/null; then
        ok "Installed to ${DEST}"
    else
        # Last resort: try with sudo (only for system dirs like /usr/local/bin)
        case "$INSTALL_DIR" in
            /usr/local/bin|/usr/bin|/opt/*)
                if command -v sudo >/dev/null 2>&1; then
                    warn "Using sudo to install to ${INSTALL_DIR}"
                    sudo cp "$BINARY_SRC" "$DEST" && sudo chmod +x "$DEST" || \
                        error "Installation failed — try: --dir \$HOME/bin"
                    ok "Installed to ${DEST} (via sudo)"
                else
                    error "Cannot write to ${INSTALL_DIR} and sudo not available. Try: --dir \$HOME/bin"
                fi
                ;;
            *)
                error "Cannot write to ${INSTALL_DIR}. Try: curl ... | sh -s -- --dir \$HOME/bin"
                ;;
        esac
    fi

    # ─── Verify installation ─────────────────────────────────────────────────
    if "${DEST}" --version >/dev/null 2>&1; then
        ok "Halcon CLI ${VERSION} ready"
    else
        warn "Binary installed but could not verify — may need PATH update"
    fi

    # ─── PATH configuration ──────────────────────────────────────────────────
    configure_path "$INSTALL_DIR"

    printf "\n${GREEN}${BOLD}  Installation complete!${RESET}\n"
    printf "\n  Get started:\n"
    printf "    ${BOLD}halcon --help${RESET}\n"
    printf "    ${BOLD}halcon chat \"Hello, Halcón!\"${RESET}\n\n"
}

main "$@"
