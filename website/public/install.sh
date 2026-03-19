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
FORCE="${HALCON_FORCE:-0}"
NO_MODIFY_PATH="${HALCON_NO_MODIFY_PATH:-0}"
CHANNEL="${HALCON_CHANNEL:-stable}"

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
            --version)        HALCON_VERSION="$2"; shift 2 ;;
            --dir)            INSTALL_DIR="$2";    shift 2 ;;
            --channel)        CHANNEL="$2";        shift 2 ;;
            --force|-f)       FORCE=1;             shift ;;
            --no-modify-path) NO_MODIFY_PATH=1;    shift ;;
            --uninstall)      _do_uninstall;       exit 0 ;;
            --help|-h)
                printf "Halcon CLI Installer\n\n"
                printf "Options:\n"
                printf "  --version VERSION     Install specific version (default: latest)\n"
                printf "  --dir DIR             Install directory (default: auto-detected)\n"
                printf "  --channel CHANNEL     Release channel: stable|beta|nightly (default: stable)\n"
                printf "  --force, -f           Reinstall even if already at target version\n"
                printf "  --no-modify-path      Skip shell RC file modification\n"
                printf "\nEnv vars:\n"
                printf "  HALCON_VERSION        Version to install\n"
                printf "  HALCON_INSTALL_DIR    Install directory override\n"
                printf "  HALCON_CHANNEL        Release channel\n"
                printf "  HALCON_FORCE=1        Force reinstall\n"
                printf "\nExamples:\n"
                printf "  curl -sSfL https://halcon.cuervo.cloud/install.sh | sh\n"
                printf "  curl -sSfL https://halcon.cuervo.cloud/install.sh | sh -s -- --version v0.3.0\n"
                printf "  curl -sSfL https://halcon.cuervo.cloud/install.sh | sh -s -- --dir /usr/local/bin\n"
                printf "  curl -sSfL https://halcon.cuervo.cloud/install.sh | sh -s -- --force\n"
                printf "  HALCON_CHANNEL=beta curl -sSfL https://halcon.cuervo.cloud/install.sh | sh\n"
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
            # Detect musl vs glibc — musl has no /lib/ld-linux* but has /lib/ld-musl*
            # or ldd reports "musl" in version output
            _IS_MUSL=0
            if ldd --version 2>&1 | grep -qi musl; then
                _IS_MUSL=1
            elif ls /lib/ld-musl* >/dev/null 2>&1; then
                _IS_MUSL=1
            fi
            case "$ARCH" in
                x86_64)  TARGET="x86_64-unknown-linux-musl" ;;  # always musl for x86_64 (static binary)
                aarch64|arm64)
                    if [ "$_IS_MUSL" = "1" ]; then
                        TARGET="aarch64-unknown-linux-musl"
                    else
                        # Require glibc ≥ 2.17 for the gnu target; fall back to musl
                        if _check_glibc 2 17; then
                            TARGET="aarch64-unknown-linux-gnu"
                        else
                            TARGET="aarch64-unknown-linux-musl"
                            info "glibc < 2.17 detected — using static musl binary"
                        fi
                    fi
                    ;;
                armv7l)  TARGET="armv7-unknown-linux-musleabihf" ;;
                *) error "Unsupported Linux architecture: $ARCH" ;;
            esac
            EXT="tar.gz"
            ;;
        Darwin)
            # Rosetta 2 detection: a shell running under x86_64 emulation on
            # arm64 hardware will report ARCH=x86_64 via uname -m.  Use sysctl
            # to check the real hardware capability and prefer the native binary.
            if [ "$ARCH" = "x86_64" ]; then
                if (sysctl hw.optional.arm64 2>/dev/null || true) | grep -q ': 1'; then
                    ARCH="arm64"
                    info "Rosetta 2 detected — selecting native arm64 binary"
                fi
            fi
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

# ─── Uninstall ────────────────────────────────────────────────────────────────
_do_uninstall() {
    printf "\n${BOLD}  Halcon CLI — Uninstall${RESET}\n\n"
    _removed=0
    for _loc in \
        "$HOME/.local/bin/${BINARY_NAME}" \
        "$HOME/bin/${BINARY_NAME}" \
        "/usr/local/bin/${BINARY_NAME}" \
        "$(command -v ${BINARY_NAME} 2>/dev/null)"
    do
        [ -f "$_loc" ] || continue
        # Avoid double-removing the same path via multiple matches
        case " $REMOVED_PATHS " in *" $_loc "*) continue ;; esac
        REMOVED_PATHS="${REMOVED_PATHS:-} $_loc"
        if rm -f "$_loc" 2>/dev/null || { command -v sudo >/dev/null 2>&1 && sudo rm -f "$_loc" 2>/dev/null; }; then
            ok "Removed: $_loc"
            _removed=$(( _removed + 1 ))
        else
            warn "Could not remove: $_loc (check permissions)"
        fi
    done
    # Remove versioned backups
    for _loc in "$HOME/.local/bin" "$HOME/bin" "/usr/local/bin"; do
        for _bak in "$_loc/${BINARY_NAME}.bak"*; do
            [ -f "$_bak" ] && rm -f "$_bak" 2>/dev/null && ok "Removed backup: $_bak"
        done
    done
    if [ "$_removed" -gt 0 ]; then
        ok "Halcon CLI uninstalled."
        warn "Config and data in ~/.halcon/ were NOT removed. To clean up:"
        warn "  rm -rf ~/.halcon"
    else
        warn "No Halcon binary found. Already uninstalled?"
    fi
}

# ─── Semver comparison helper ────────────────────────────────────────────────
# Returns 0 (true) if $1 is strictly greater than $2.
# Handles "X.Y.Z" format, strips leading 'v'.
_version_gt() {
    _va="$(printf '%s' "$1" | sed 's/^v//')"
    _vb="$(printf '%s' "$2" | sed 's/^v//')"
    [ "$_va" = "$_vb" ] && return 1
    # Use sort -V (GNU coreutils) when available — handles pre-release suffixes
    if sort --version 2>&1 | grep -q "GNU coreutils" 2>/dev/null; then
        _higher="$(printf '%s\n%s' "$_va" "$_vb" | sort -V | tail -1)"
        [ "$_higher" = "$_va" ] && return 0 || return 1
    fi
    # Manual split: X.Y.Z — strip pre-release suffix after '-'
    _va="$(printf '%s' "$_va" | cut -d'-' -f1)"
    _vb="$(printf '%s' "$_vb" | cut -d'-' -f1)"
    _a1="$(printf '%s' "$_va" | cut -d. -f1)"
    _a2="$(printf '%s' "$_va" | cut -d. -f2)"
    _a3="$(printf '%s' "$_va" | cut -d. -f3)"
    _b1="$(printf '%s' "$_vb" | cut -d. -f1)"
    _b2="$(printf '%s' "$_vb" | cut -d. -f2)"
    _b3="$(printf '%s' "$_vb" | cut -d. -f3)"
    # Ensure integers (default 0)
    _a1="${_a1:-0}"; _a2="${_a2:-0}"; _a3="${_a3:-0}"
    _b1="${_b1:-0}"; _b2="${_b2:-0}"; _b3="${_b3:-0}"
    [ "$_a1" -gt "$_b1" ] && return 0
    [ "$_a1" -lt "$_b1" ] && return 1
    [ "$_a2" -gt "$_b2" ] && return 0
    [ "$_a2" -lt "$_b2" ] && return 1
    [ "$_a3" -gt "$_b3" ] && return 0
    return 1
}

# ─── Existing installation probe ──────────────────────────────────────────────
# Sets EXISTING_VERSION and EXISTING_PATH if halcon is already installed.
_probe_existing() {
    EXISTING_VERSION=""
    EXISTING_PATH=""
    for _cand in \
        "${INSTALL_DIR}/${BINARY_NAME}" \
        "$(command -v "${BINARY_NAME}" 2>/dev/null)" \
        "$HOME/.local/bin/${BINARY_NAME}" \
        "$HOME/bin/${BINARY_NAME}" \
        "/usr/local/bin/${BINARY_NAME}"
    do
        [ -x "$_cand" ] || continue
        _v="$("$_cand" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)"
        if [ -n "$_v" ]; then
            EXISTING_VERSION="$_v"
            EXISTING_PATH="$_cand"
            return
        fi
    done
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
# Enforces HTTPS (rejects non-HTTPS redirects), TLS ≥ 1.2, and explicit cipher
# suites (TLS 1.3 preferred, ECDHE-based TLS 1.2 fallback) — rustup pattern.
_CURL_CIPHERS="TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256:\
ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256"

_prefer_curl() {
    # Prefer system curl over snap-installed curl (snap curl lacks file-write perms)
    for _c in /usr/bin/curl /usr/local/bin/curl "$(command -v curl 2>/dev/null)"; do
        [ -x "$_c" ] || continue
        case "$_c" in
            */snap/*) continue ;;  # skip snap curl
        esac
        echo "$_c"
        return
    done
    # Fall back to snap curl if nothing else available
    command -v curl 2>/dev/null && return
    return 1
}

download() {
    local url="$1"
    local dest="$2"
    # Use --progress-bar when connected to a TTY; silent otherwise (piped installs)
    local _progress
    [ -t 1 ] && _progress="--progress-bar" || _progress="--silent"
    local _curl
    _curl="$(_prefer_curl 2>/dev/null)" || _curl=""
    if [ -n "$_curl" ]; then
        # --proto '=https' rejects any non-HTTPS redirects
        # --tlsv1.2 enforces minimum TLS version
        # --retry-connrefused retries on ECONNREFUSED (transient CDN errors)
        # Attempt with cipher enforcement first; fall back without (older curl/macOS)
        "$_curl" $_progress \
            --proto '=https' \
            --tlsv1.2 \
            --ciphers "$_CURL_CIPHERS" \
            --retry 3 --retry-delay 2 --retry-connrefused \
            -fL -o "$dest" "$url" 2>/dev/null \
        || "$_curl" -sSfL \
            --proto '=https' \
            --tlsv1.2 \
            --retry 3 --retry-delay 2 --retry-connrefused \
            -o "$dest" "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget -q --tries=3 --https-only -O "$dest" "$url" 2>/dev/null \
            || wget -q --tries=3 -O "$dest" "$url"
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

# ─── glibc version check ─────────────────────────────────────────────────────
# Returns 0 if system glibc satisfies the minimum major.minor requirement.
# Falls back gracefully on non-glibc systems (musl, macOS, etc.).
# Handles distroless/WolfiOS where ldd may be absent — uses ld.so --version.
_check_glibc() {
    local _min_major="${1:-2}"
    local _min_minor="${2:-17}"
    # Only applicable on Linux with GNU libc
    [ "$(uname -s)" = "Linux" ] || return 0

    # Fast musl detection via ldd (when present)
    if command -v ldd >/dev/null 2>&1; then
        ldd --version 2>&1 | grep -qi musl && return 0  # musl — always OK
        _glibc_ver="$(ldd --version 2>/dev/null | awk 'NR==1{print $NF}')"
    fi

    # Fallback: probe ld.so directly (WolfiOS, distroless, minimal containers without ldd)
    if [ -z "${_glibc_ver:-}" ]; then
        _ld_so="$(ls /lib/ld-linux*.so.* /lib64/ld-linux*.so.* /lib/ld-linux-*.so.* 2>/dev/null | head -1)"
        if [ -n "$_ld_so" ] && [ -x "$_ld_so" ]; then
            _glibc_ver="$("$_ld_so" --version 2>&1 | awk 'NR==1{print $NF}')"
        fi
    fi

    [ -z "${_glibc_ver:-}" ] && return 0  # can't detect; assume OK

    _sys_major="$(printf '%s' "$_glibc_ver" | cut -d. -f1)"
    _sys_minor="$(printf '%s' "$_glibc_ver" | cut -d. -f2)"
    # Compare: major first, then minor
    [ "$_sys_major" -gt "$_min_major" ] && return 0
    [ "$_sys_major" -eq "$_min_major" ] && [ "${_sys_minor:-0}" -ge "$_min_minor" ] && return 0
    return 1  # too old
}

# ─── Cosign optional verification ────────────────────────────────────────────
# Verifies supply chain integrity when cosign is available.
# Degrades gracefully — installer proceeds without it (TLS + SHA-256 is the floor).
# See: https://docs.sigstore.dev/cosign/verifying/verify/
_verify_cosign() {
    local _archive="$1"
    local _sig_url="$2"
    local _cert_url="$3"
    local _tmpdir="$4"

    command -v cosign >/dev/null 2>&1 || {
        info "cosign not found — skipping signature verification (TLS+SHA-256 enforced)"
        info "Install cosign for supply chain security: https://docs.sigstore.dev"
        return 0
    }

    _sig_file="${_tmpdir}/archive.sig"
    _cert_file="${_tmpdir}/archive.pem"

    download "$_sig_url"  "$_sig_file"  2>/dev/null || { warn "cosign sig unavailable — skipping"; return 0; }
    download "$_cert_url" "$_cert_file" 2>/dev/null || { warn "cosign cert unavailable — skipping"; return 0; }

    info "Verifying cosign signature (Sigstore keyless)..."
    if cosign verify-blob \
        --signature "$_sig_file" \
        --certificate "$_cert_file" \
        --certificate-identity-regexp "^https://github.com/cuervo-ai/halcon-cli/\.github/workflows/release\.yml@refs/tags/" \
        --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
        "$_archive" 2>/dev/null
    then
        ok "Cosign signature verified (Sigstore Rekor)"
    else
        warn "Cosign verification failed — the binary may still be safe (SHA-256 was verified)"
        warn "Check: https://search.sigstore.dev/?hash=$(sha256sum "$_archive" 2>/dev/null | cut -d' ' -f1)"
    fi
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
    # Trap EXIT for normal cleanup; INT/TERM for Ctrl-C / kill (128+signal convention)
    trap 'rm -rf "$TMPDIR_WORK"' EXIT
    trap 'rm -rf "$TMPDIR_WORK"; exit 130' INT
    trap 'rm -rf "$TMPDIR_WORK"; exit 143' TERM

    # ─── Channel-aware manifest URL ──────────────────────────────────────────
    case "$CHANNEL" in
        beta|nightly)
            MANIFEST_URL="${RELEASES_URL}/${CHANNEL}/manifest.json" ;;
        stable|"")
            MANIFEST_URL="${RELEASES_URL}/latest/manifest.json" ;;
        *)
            warn "Unknown channel '${CHANNEL}', defaulting to stable"
            MANIFEST_URL="${RELEASES_URL}/latest/manifest.json" ;;
    esac

    # ─── Resolve version ────────────────────────────────────────────────────
    REQUESTED_VERSION="$(printf '%s' "$HALCON_VERSION" | sed 's/^v//')"

    if [ "$REQUESTED_VERSION" = "latest" ]; then
        info "Fetching release manifest (channel: ${CHANNEL})..."
        MANIFEST_FILE="$TMPDIR_WORK/manifest.json"
        download "$MANIFEST_URL" "$MANIFEST_FILE"

        if grep -q '"error"' "$MANIFEST_FILE" 2>/dev/null; then
            if command -v jq >/dev/null 2>&1; then
                ERR="$(jq -r '.error // empty' "$MANIFEST_FILE" 2>/dev/null)"
            else
                ERR="$(grep -o '"error": *"[^"]*"' "$MANIFEST_FILE" | sed 's/.*"\([^"]*\)".*/\1/')"
            fi
            error "Release API error: ${ERR}. Check https://releases.cli.cuervo.cloud/health"
        fi

        if command -v jq >/dev/null 2>&1; then
            VERSION="$(jq -r '.version // empty' "$MANIFEST_FILE" 2>/dev/null)"
        else
            VERSION="$(grep -o '"version": *"[^"]*"' "$MANIFEST_FILE" | head -1 | sed 's/.*"\([^"]*\)".*/\1/')"
        fi
        if [ -z "$VERSION" ]; then
            error "Failed to parse version from manifest"
        fi
        info "Latest version: ${VERSION}"
    else
        VERSION="$REQUESTED_VERSION"
        info "Installing version: ${VERSION}"
    fi

    # ─── Detect existing installation ────────────────────────────────────────
    resolve_install_dir
    _probe_existing

    if [ -n "$EXISTING_VERSION" ]; then
        if [ "$EXISTING_VERSION" = "$VERSION" ] && [ "$FORCE" = "0" ]; then
            ok "Already at latest (v${VERSION}) — ${EXISTING_PATH}"
            info "To reinstall: re-run with --force"
            configure_path "$INSTALL_DIR"
            exit 0
        elif _version_gt "$VERSION" "$EXISTING_VERSION"; then
            printf "\n${BOLD}  Upgrading${RESET}: v${EXISTING_VERSION} → v${VERSION}\n\n"
        elif _version_gt "$EXISTING_VERSION" "$VERSION"; then
            if [ "$FORCE" = "0" ]; then
                warn "Installed v${EXISTING_VERSION} is newer than v${VERSION}. Skipping."
                warn "Use --force to downgrade."
                exit 0
            fi
            printf "\n${YELLOW}${BOLD}  Downgrading${RESET}: v${EXISTING_VERSION} → v${VERSION} (--force)\n\n"
        fi
    else
        printf "\n${BOLD}  Fresh install${RESET}: v${VERSION}\n\n"
    fi
    info "Install directory: ${INSTALL_DIR}"

    # ─── Build artifact name & URLs ──────────────────────────────────────────
    ARTIFACT_NAME="halcon-${VERSION}-${TARGET}.${EXT}"
    if [ "$REQUESTED_VERSION" = "latest" ]; then
        DOWNLOAD_URL="${RELEASES_URL}/latest/${ARTIFACT_NAME}"
        CS_URL="${RELEASES_URL}/latest/checksums.txt"
        GITHUB_URL="https://github.com/cuervo-ai/halcon-cli/releases/latest"
    else
        DOWNLOAD_URL="${RELEASES_URL}/v${VERSION}/${ARTIFACT_NAME}"
        CS_URL="${RELEASES_URL}/v${VERSION}/checksums.txt"
        GITHUB_URL="https://github.com/cuervo-ai/halcon-cli/releases/tag/v${VERSION}"
    fi

    # ─── Check artifact is listed in manifest (fast-fail before download) ───
    # We already have the manifest if REQUESTED_VERSION=latest; fetch it for
    # specific versions too so we can give a helpful error.
    if [ -z "${MANIFEST_FILE:-}" ]; then
        MANIFEST_FILE="$TMPDIR_WORK/manifest.json"
        _MURL="${RELEASES_URL}/v${VERSION}/manifest.json"
        download "$_MURL" "$MANIFEST_FILE" 2>/dev/null || true
    fi
    if [ -f "$MANIFEST_FILE" ] && ! grep -q "\"${ARTIFACT_NAME}\"" "$MANIFEST_FILE" 2>/dev/null; then
        printf "\n${RED}  ✗${RESET} No pre-built binary for ${BOLD}${OS} ${ARCH}${RESET} (${TARGET}) in v${VERSION}.\n" >&2
        printf "\n  Available artifacts in this release:\n" >&2
        grep -o '"name": *"[^"]*"' "$MANIFEST_FILE" | sed 's/.*"\([^"]*\)".*/    • \1/' >&2 || true
        printf "\n  ${YELLOW}Install via script (recommended):${RESET}\n" >&2
        printf "    — Wait for the next release which may include your platform, or\n" >&2
        printf "    — Build from source: https://github.com/cuervo-ai/halcon-cli\n" >&2
        printf "    — Check available releases: ${GITHUB_URL}\n\n" >&2
        exit 1
    fi

    # ─── Fetch SHA-256 ──────────────────────────────────────────────────────
    EXPECTED_SHA=""
    CS_FILE="$TMPDIR_WORK/checksums.txt"
    if download "$CS_URL" "$CS_FILE" 2>/dev/null; then
        EXPECTED_SHA="$(grep "${ARTIFACT_NAME}" "$CS_FILE" | awk '{print $1}' | head -1)"
    fi

    # ─── Download artifact ──────────────────────────────────────────────────
    info "Downloading ${ARTIFACT_NAME}..."
    ARCHIVE_FILE="$TMPDIR_WORK/${ARTIFACT_NAME}"
    download "$DOWNLOAD_URL" "$ARCHIVE_FILE" || \
        error "Download failed. Check available artifacts: ${GITHUB_URL}"
    ok "Downloaded ($(du -sh "$ARCHIVE_FILE" | cut -f1))"

    # ─── Verify SHA-256 ─────────────────────────────────────────────────────
    if [ -n "$EXPECTED_SHA" ]; then
        info "Verifying SHA-256..."
        verify_sha256 "$ARCHIVE_FILE" "$EXPECTED_SHA"
    else
        warn "SHA-256 not available, skipping verification"
    fi

    # ─── Cosign supply-chain verification (optional, graceful degradation) ───
    if [ "$REQUESTED_VERSION" = "latest" ]; then
        _SIG_URL="${RELEASES_URL}/latest/${ARTIFACT_NAME}.sig"
        _CERT_URL="${RELEASES_URL}/latest/${ARTIFACT_NAME}.pem"
    else
        _SIG_URL="${RELEASES_URL}/v${VERSION}/${ARTIFACT_NAME}.sig"
        _CERT_URL="${RELEASES_URL}/v${VERSION}/${ARTIFACT_NAME}.pem"
    fi
    _verify_cosign "$ARCHIVE_FILE" "$_SIG_URL" "$_CERT_URL" "$TMPDIR_WORK"

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

    # ─── Install (atomic) ────────────────────────────────────────────────────
    DEST="${INSTALL_DIR}/${BINARY_NAME}"
    chmod +x "$BINARY_SRC"

    # Atomic install: write to a temp file in the SAME directory as the
    # destination, then rename(2) — a single syscall that is either complete
    # or a no-op.  This prevents a partially-written binary from ever being
    # visible to concurrent processes (cargo-dist / uv pattern).
    _atomic_install() {
        local _src="$1" _dst="$2" _dir
        _dir="$(dirname "$_dst")"
        _tmp="$(mktemp "${_dir}/.${BINARY_NAME}.tmp.XXXXXX" 2>/dev/null)" || return 1
        cp "$_src" "$_tmp" 2>/dev/null || { rm -f "$_tmp"; return 1; }
        chmod 755 "$_tmp"
        mv -f "$_tmp" "$_dst" 2>/dev/null || { rm -f "$_tmp"; return 1; }
        return 0
    }

    if _atomic_install "$BINARY_SRC" "$DEST"; then
        ok "Installed to ${DEST}"
    else
        # Last resort: try with sudo (only for system dirs like /usr/local/bin)
        case "$INSTALL_DIR" in
            /usr/local/bin|/usr/bin|/opt/*)
                if command -v sudo >/dev/null 2>&1; then
                    warn "Using sudo to install to ${INSTALL_DIR}"
                    _sudo_tmp="$(sudo mktemp "${INSTALL_DIR}/.${BINARY_NAME}.tmp.XXXXXX" 2>/dev/null)" || \
                        _sudo_tmp=""
                    if [ -n "$_sudo_tmp" ]; then
                        sudo cp "$BINARY_SRC" "$_sudo_tmp" && \
                        sudo chmod 755 "$_sudo_tmp" && \
                        sudo mv -f "$_sudo_tmp" "$DEST" || \
                            { sudo rm -f "$_sudo_tmp" 2>/dev/null; error "Installation failed — try: --dir \$HOME/bin"; }
                    else
                        sudo cp "$BINARY_SRC" "$DEST" && sudo chmod +x "$DEST" || \
                            error "Installation failed — try: --dir \$HOME/bin"
                    fi
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
    if [ "$NO_MODIFY_PATH" = "0" ]; then
        configure_path "$INSTALL_DIR"
    else
        info "Skipping shell config (--no-modify-path)"
        printf "  Add to PATH manually: ${BOLD}export PATH=\"\$PATH:${INSTALL_DIR}\"${RESET}\n"
    fi

    # ─── Full-capacity configuration ─────────────────────────────────────────
    configure_halcon

    printf "\n${GREEN}${BOLD}  Installation complete!${RESET}\n\n"

    # ── Cenzontle active path ─────────────────────────────────────────────────
    if [ "$_SYS_CENZONTLE_CONFIGURED" = "true" ]; then
        printf "${GREEN}${BOLD}  ✓ Cenzontle AI active — you're ready to go!${RESET}\n\n"
        printf "  ${BOLD}Start now:${RESET}\n"
        printf "    ${BOLD}halcon chat --tui --full --expert${RESET}       ${CYAN}# default: Cenzontle${RESET}\n"
        printf "    ${BOLD}halcon -p cenzontle chat --tui${RESET}          ${CYAN}# explicit${RESET}\n"
        printf "    ${BOLD}halcon auth status${RESET}                      ${CYAN}# verify token${RESET}\n"
        printf "\n  ${BOLD}Other providers (optional):${RESET}\n"
        printf "    ${BOLD}halcon -p anthropic chat${RESET}                ${CYAN}# Claude direct (needs ANTHROPIC_API_KEY)${RESET}\n"
        printf "    ${BOLD}halcon -p ollama chat${RESET}                   ${CYAN}# local, no API key${RESET}\n"
    else
        # ── No provider yet ──────────────────────────────────────────────────
        printf "  ${BOLD}Next step — connect a provider:${RESET}\n"
        printf "\n  ${CYAN}Enterprise (recommended):${RESET}\n"
        printf "    ${BOLD}halcon auth login cenzontle${RESET}     ${CYAN}# Cenzontle SSO — browser OAuth, no API key needed${RESET}\n"
        printf "\n  ${CYAN}Cloud APIs (API key):${RESET}\n"
        printf "    ${BOLD}halcon auth login anthropic${RESET}     ${CYAN}# Claude — recommended${RESET}\n"
        printf "    ${BOLD}halcon auth login openai${RESET}        ${CYAN}# GPT models${RESET}\n"
        printf "    ${BOLD}halcon auth login deepseek${RESET}      ${CYAN}# cheapest option${RESET}\n"
        printf "    ${BOLD}halcon auth login gemini${RESET}        ${CYAN}# Google Gemini${RESET}\n"
        printf "\n  ${CYAN}Cloud Infrastructure:${RESET}\n"
        printf "    ${BOLD}export CLAUDE_CODE_USE_BEDROCK=1${RESET} ${CYAN}# AWS Bedrock (+ AWS_REGION + credentials)${RESET}\n"
        printf "    ${BOLD}export CLAUDE_CODE_USE_AZURE=1${RESET}   ${CYAN}# Azure AI Foundry (+ AZURE_AI_ENDPOINT)${RESET}\n"
        printf "    ${BOLD}export CLAUDE_CODE_USE_VERTEX=1${RESET}  ${CYAN}# Google Vertex AI (+ project + gcloud ADC)${RESET}\n"
        printf "\n  ${CYAN}Local (no API key):${RESET}\n"
        printf "    ${BOLD}halcon chat -p ollama${RESET}           ${CYAN}# Ollama — fully local${RESET}\n"
        printf "\n  ${BOLD}Then start:${RESET}\n"
        printf "    ${BOLD}halcon chat --tui --full --expert${RESET}\n"
    fi

    printf "\n  ${BOLD}Useful commands:${RESET}\n"
    printf "    ${BOLD}halcon auth status${RESET}              ${CYAN}# check configured providers${RESET}\n"
    printf "    ${BOLD}halcon doctor${RESET}                   ${CYAN}# runtime diagnostics${RESET}\n"
    printf "    ${BOLD}halcon update${RESET}                   ${CYAN}# update to latest version${RESET}\n"
    printf "    ${BOLD}halcon agents list${RESET}              ${CYAN}# sub-agent registry${RESET}\n"
    printf "    ${BOLD}halcon mcp list${RESET}                 ${CYAN}# MCP servers${RESET}\n"
    printf "\n"
}

# ─── System detection ────────────────────────────────────────────────────────
# Probes hardware, OS, environment, and installed tools.
# Sets _SYS_* variables consumed by _write_config / _write_mcp_config.
detect_system() {
    _SYS_OS="$(uname -s)"        # Darwin | Linux
    _SYS_ARCH="$(uname -m)"      # x86_64 | aarch64 | arm64

    # ── CPU cores ────────────────────────────────────────────────────────────
    if command -v nproc >/dev/null 2>&1; then
        _SYS_CORES="$(nproc)"
    elif command -v sysctl >/dev/null 2>&1; then
        _SYS_CORES="$(sysctl -n hw.logicalcpu 2>/dev/null || echo 4)"
    else
        _SYS_CORES=4
    fi

    # ── RAM (MB) ─────────────────────────────────────────────────────────────
    if [ "$_SYS_OS" = "Darwin" ]; then
        _SYS_RAM_MB="$(( $(sysctl -n hw.memsize 2>/dev/null || echo 4294967296) / 1048576 ))"
    elif [ -f /proc/meminfo ]; then
        _SYS_RAM_MB="$(awk '/MemTotal/{print int($2/1024)}' /proc/meminfo)"
    else
        _SYS_RAM_MB=4096
    fi
    _SYS_RAM_GB="$(( _SYS_RAM_MB / 1024 ))"

    # ── Free disk (GB, measured at $HOME) ────────────────────────────────────
    _SYS_DISK_GB="$(df -k "$HOME" 2>/dev/null | awk 'NR==2{print int($4/1048576)}' || echo 10)"
    [ -z "$_SYS_DISK_GB" ] || [ "$_SYS_DISK_GB" = "0" ] && _SYS_DISK_GB=10

    # ── Environment ──────────────────────────────────────────────────────────
    _SYS_IS_CI=0
    [ -n "${CI:-}" ] || [ -n "${GITHUB_ACTIONS:-}" ] || [ -n "${GITLAB_CI:-}" ] \
        || [ -n "${CIRCLECI:-}" ] || [ -n "${BUILDKITE:-}" ] \
        || [ -n "${TF_BUILD:-}" ] && _SYS_IS_CI=1 || true

    _SYS_IS_CONTAINER=0
    { [ -f /.dockerenv ] \
        || grep -qE "docker|lxc|containerd" /proc/1/cgroup 2>/dev/null; } \
        && _SYS_IS_CONTAINER=1 || true

    _SYS_IS_WSL=0
    grep -qi microsoft /proc/version 2>/dev/null && _SYS_IS_WSL=1 || true

    # ── GPU ──────────────────────────────────────────────────────────────────
    _SYS_HAS_GPU=0
    if command -v nvidia-smi >/dev/null 2>&1 && nvidia-smi -L >/dev/null 2>&1; then
        _SYS_HAS_GPU=1
    elif [ "$_SYS_OS" = "Darwin" ]; then
        # Apple Silicon always has Metal GPU; Intel Macs usually have discrete GPU
        _SYS_HAS_GPU=1
    fi

    # ── Ollama ───────────────────────────────────────────────────────────────
    _SYS_OLLAMA_ENABLED="false"
    _SYS_OLLAMA_MODEL="llama3.2"
    if command -v ollama >/dev/null 2>&1; then
        _SYS_OLLAMA_ENABLED="true"
        # Try to get installed model list (non-blocking)
        _first="$(curl -sf --max-time 2 http://localhost:11434/api/tags 2>/dev/null \
            | grep -o '"name":"[^"]*"' | head -1 | sed 's/"name":"//;s/"//')"
        [ -n "$_first" ] && _SYS_OLLAMA_MODEL="$_first"
    fi

    # ── Claude CLI ───────────────────────────────────────────────────────────
    _SYS_CLAUDE_CMD=""
    for _c in \
        "$HOME/.local/bin/claude" \
        "/usr/local/bin/claude" \
        "/opt/homebrew/bin/claude" \
        "$(command -v claude 2>/dev/null)"
    do
        [ -x "$_c" ] && { _SYS_CLAUDE_CMD="$_c"; break; }
    done

    # ── Cenzontle SSO token detection ─────────────────────────────────────────
    # Priority: env var > macOS Keychain > Linux D-Bus (secret-tool) > XDG file store
    # Non-blocking: each probe has a fast timeout; failures are silently skipped.
    _SYS_CENZONTLE_CONFIGURED="false"
    _SYS_CENZONTLE_TOKEN=""

    # 1. Environment variable (CI/CD or manual export — highest priority)
    if [ -n "${CENZONTLE_ACCESS_TOKEN:-}" ]; then
        _SYS_CENZONTLE_CONFIGURED="true"
        _SYS_CENZONTLE_TOKEN="${CENZONTLE_ACCESS_TOKEN}"

    # 2. macOS Keychain (written by `halcon auth login cenzontle` on macOS)
    elif [ "$_SYS_OS" = "Darwin" ] && command -v security >/dev/null 2>&1; then
        _tok="$(security find-generic-password \
            -s "halcon-cli" -a "cenzontle:access_token" -w 2>/dev/null || true)"
        if [ -n "$_tok" ]; then
            _SYS_CENZONTLE_CONFIGURED="true"
            _SYS_CENZONTLE_TOKEN="$_tok"
        fi

    # 3. Linux D-Bus Secret Service (written by `halcon auth login cenzontle` on desktop Linux)
    elif [ "$_SYS_OS" = "Linux" ] && command -v secret-tool >/dev/null 2>&1; then
        _tok="$(secret-tool lookup \
            service halcon-cli account "cenzontle:access_token" 2>/dev/null || true)"
        if [ -n "$_tok" ]; then
            _SYS_CENZONTLE_CONFIGURED="true"
            _SYS_CENZONTLE_TOKEN="$_tok"
        fi
    fi

    # 4. XDG file store fallback (headless Linux — ~/.local/share/halcon/halcon-cli.json)
    if [ "$_SYS_CENZONTLE_CONFIGURED" = "false" ] && [ "$_SYS_OS" = "Linux" ]; then
        _cz_store="$HOME/.local/share/halcon/halcon-cli.json"
        if [ -f "$_cz_store" ] && command -v python3 >/dev/null 2>&1; then
            _tok="$(python3 - "$_cz_store" <<'PYEOF' 2>/dev/null
import json, sys
try:
    d = json.load(open(sys.argv[1]))
    print(d.get("cenzontle:access_token", ""))
except Exception:
    pass
PYEOF
)"
            if [ -n "$_tok" ]; then
                _SYS_CENZONTLE_CONFIGURED="true"
                _SYS_CENZONTLE_TOKEN="$_tok"
            fi
        fi
    fi

    # Derive: if cenzontle is configured, make it the default provider
    if [ "$_SYS_CENZONTLE_CONFIGURED" = "true" ]; then
        _SYS_DEFAULT_PROVIDER="cenzontle"
    else
        _SYS_DEFAULT_PROVIDER="anthropic"
    fi

    # ── Derive tuned values ───────────────────────────────────────────────────

    # max_parallel_tools: 2× cores, clamped [4, 20]
    _SYS_MAX_PARALLEL=$(( _SYS_CORES * 2 ))
    [ "$_SYS_MAX_PARALLEL" -lt 4 ]  && _SYS_MAX_PARALLEL=4
    [ "$_SYS_MAX_PARALLEL" -gt 20 ] && _SYS_MAX_PARALLEL=20

    # max_concurrent_agents: cores/4, clamped [1, 5]
    _SYS_MAX_AGENTS=$(( _SYS_CORES / 4 ))
    [ "$_SYS_MAX_AGENTS" -lt 1 ] && _SYS_MAX_AGENTS=1
    [ "$_SYS_MAX_AGENTS" -gt 5 ] && _SYS_MAX_AGENTS=5

    # sandbox max_memory_mb: RAM/2, clamped [512, 16384]
    _SYS_SANDBOX_MEM=$(( _SYS_RAM_MB / 2 ))
    [ "$_SYS_SANDBOX_MEM" -lt 512 ]   && _SYS_SANDBOX_MEM=512
    [ "$_SYS_SANDBOX_MEM" -gt 16384 ] && _SYS_SANDBOX_MEM=16384

    # max_context_tokens: tiered by RAM
    if   [ "$_SYS_RAM_GB" -ge 32 ]; then _SYS_MAX_CTX=180000
    elif [ "$_SYS_RAM_GB" -ge 16 ]; then _SYS_MAX_CTX=120000
    elif [ "$_SYS_RAM_GB" -ge 8  ]; then _SYS_MAX_CTX=80000
    else                                  _SYS_MAX_CTX=40000
    fi

    # search max_documents: tiered by disk
    if   [ "$_SYS_DISK_GB" -ge 50 ]; then _SYS_MAX_DOCS=100000
    elif [ "$_SYS_DISK_GB" -ge 20 ]; then _SYS_MAX_DOCS=50000
    else                                   _SYS_MAX_DOCS=20000
    fi

    # tool / provider timeouts: CI gets tighter, slow networks get looser
    if [ "$_SYS_IS_CI" = "1" ]; then
        _SYS_TOOL_TIMEOUT=60
        _SYS_PROVIDER_TIMEOUT=120
        _SYS_MAX_DURATION=900
        _SYS_CONFIRM_DESTRUCTIVE="false"
        _SYS_AUTO_APPROVE_CI="true"
    else
        _SYS_TOOL_TIMEOUT=120
        _SYS_PROVIDER_TIMEOUT=300
        _SYS_MAX_DURATION=1800
        _SYS_CONFIRM_DESTRUCTIVE="true"
        _SYS_AUTO_APPROVE_CI="false"
    fi

    # theme: fire on macOS/rich terminals; auto elsewhere
    if [ "$_SYS_OS" = "Darwin" ]; then
        _SYS_THEME="fire"
    else
        _SYS_THEME="auto"
    fi

    # animations: off in CI or containers (no TTY)
    if [ "$_SYS_IS_CI" = "1" ] || [ "$_SYS_IS_CONTAINER" = "1" ]; then
        _SYS_ANIMATIONS="false"
    else
        _SYS_ANIMATIONS="true"
    fi

    # logging: debug in CI, info otherwise
    if [ "$_SYS_IS_CI" = "1" ]; then
        _SYS_LOG_LEVEL="debug"
    else
        _SYS_LOG_LEVEL="info"
    fi

    # multimodal mode: local if GPU + ARM (Apple Silicon), api otherwise
    if [ "$_SYS_HAS_GPU" = "1" ] && [ "$_SYS_ARCH" = "arm64" ]; then
        _SYS_MULTIMODAL_MODE="hybrid"
    else
        _SYS_MULTIMODAL_MODE="api"
    fi

    # max_file_size for multimodal: 50MB on GPU, 20MB otherwise
    if [ "$_SYS_HAS_GPU" = "1" ]; then
        _SYS_MULTIMODAL_MAX_FILE=52428800
    else
        _SYS_MULTIMODAL_MAX_FILE=20971520
    fi

    # allowed_directories — OS-aware
    # Build as newline-separated quoted entries for the TOML array
    _SYS_ALLOWED_DIRS="    \".\",
    \"/tmp\""

    if [ "$_SYS_OS" = "Darwin" ]; then
        _SYS_ALLOWED_DIRS="${_SYS_ALLOWED_DIRS},
    \"/private/tmp\",
    \"$HOME\",
    \"$HOME/Documents\",
    \"$HOME/Downloads\",
    \"$HOME/Desktop\""
    else
        # Linux: only add directories that actually exist
        _SYS_ALLOWED_DIRS="${_SYS_ALLOWED_DIRS},
    \"$HOME\""
        for _d in "$HOME/Documents" "$HOME/Downloads" "$HOME/projects" "$HOME/dev" "$HOME/src"; do
            [ -d "$_d" ] && _SYS_ALLOWED_DIRS="${_SYS_ALLOWED_DIRS},
    \"$_d\""
        done
        # WSL: also expose Windows user directory if accessible
        if [ "$_SYS_IS_WSL" = "1" ]; then
            _WIN_HOME="$(cmd.exe /c "echo %USERPROFILE%" 2>/dev/null | tr -d '\r' | sed 's|\\\\|/|g; s|C:|/mnt/c|')" || true
            [ -d "$_WIN_HOME" ] && _SYS_ALLOWED_DIRS="${_SYS_ALLOWED_DIRS},
    \"$_WIN_HOME\""
        fi
    fi

    # MCP filesystem dirs — reuse for .mcp.json
    _SYS_MCP_DIRS="\"$HOME\",
        \"/tmp\""
    if [ "$_SYS_OS" = "Darwin" ]; then
        _SYS_MCP_DIRS="\"$HOME/Documents\",
        \"$HOME/Downloads\",
        \"$HOME/Desktop\",
        \"/tmp\""
    else
        for _d in "$HOME/Documents" "$HOME/Downloads" "$HOME/projects" "$HOME/dev"; do
            [ -d "$_d" ] && _SYS_MCP_DIRS="${_SYS_MCP_DIRS},
        \"$_d\""
        done
    fi

    # claude_code section string (included only when claude CLI is found)
    if [ -n "$_SYS_CLAUDE_CMD" ]; then
        _SYS_CLAUDE_SECTION="[models.providers.claude_code]
enabled              = true
command              = \"$_SYS_CLAUDE_CMD\"
default_model        = \"claude-sonnet-4-6\"
mode                 = \"chat\"
drain_timeout_secs   = 30
auto_restart         = true
request_timeout_secs = 120

[models.providers.claude_code.http]
connect_timeout_secs = 10
request_timeout_secs = 300
max_retries          = 3
retry_base_delay_ms  = 1000"
    else
        _SYS_CLAUDE_SECTION="# claude_code provider: claude CLI not found at install time.
# Install from https://claude.ai/download, then re-run: halcon config set models.providers.claude_code.enabled true
# [models.providers.claude_code]
# enabled = false"
    fi
}

# ─── Full-capacity configuration ─────────────────────────────────────────────
# Writes ~/.halcon/config.toml and ~/.halcon/.mcp.json if they don't exist.
# Skips gracefully if the user already has a config.
configure_halcon() {
    HALCON_DIR="$HOME/.halcon"
    CONFIG_FILE="$HALCON_DIR/config.toml"
    MCP_FILE="$HALCON_DIR/.mcp.json"

    printf "\n  ${BOLD}Detecting system...${RESET}\n"
    detect_system

    # Print summary of what was detected
    printf "    OS: %s %s" "$_SYS_OS" "$_SYS_ARCH"
    [ "$_SYS_IS_WSL"       = "1" ] && printf " (WSL)"
    [ "$_SYS_IS_CONTAINER" = "1" ] && printf " (container)"
    [ "$_SYS_IS_CI"        = "1" ] && printf " (CI)"
    printf "\n"
    printf "    CPU: %s cores  RAM: %s GB  Disk: %s GB free\n" \
        "$_SYS_CORES" "$_SYS_RAM_GB" "$_SYS_DISK_GB"
    [ "$_SYS_HAS_GPU"        = "1" ] && printf "    GPU: detected (%s)\n" "$_SYS_MULTIMODAL_MODE"
    [ "$_SYS_OLLAMA_ENABLED" = "true" ] && printf "    Ollama: found (%s)\n" "$_SYS_OLLAMA_MODEL"
    [ -n "$_SYS_CLAUDE_CMD" ] && printf "    Claude CLI: %s\n" "$_SYS_CLAUDE_CMD"
    if [ "$_SYS_CENZONTLE_CONFIGURED" = "true" ]; then
        printf "    ${GREEN}Cenzontle:${RESET} token found — will be set as default provider\n"
    else
        printf "    Cenzontle: not configured (run: halcon auth login cenzontle)\n"
    fi

    printf "\n  ${BOLD}Configuring Halcón...${RESET}\n"
    mkdir -p "$HALCON_DIR" 2>/dev/null || true

    # ── config.toml ──────────────────────────────────────────────────────────
    if [ -f "$CONFIG_FILE" ]; then
        ok "Config already exists — patching cenzontle state (${CONFIG_FILE})"
        _patch_cenzontle_config "$CONFIG_FILE"
    else
        info "Writing system-adapted config..."
        _write_config "$CONFIG_FILE"
        ok "Config written: ${CONFIG_FILE}"
    fi

    # ── .mcp.json ────────────────────────────────────────────────────────────
    if [ -f "$MCP_FILE" ]; then
        ok "MCP config already exists — skipping"
    else
        _write_mcp_config "$MCP_FILE"
    fi

    # ── Agent registry directories ────────────────────────────────────────────
    _setup_agent_registry "$HALCON_DIR"

    # ── Classifier rules ─────────────────────────────────────────────────────
    _write_classifier_rules "$HALCON_DIR/classifier_rules.toml"

    # ── Cenzontle SSO — interactive login if not yet configured ──────────────
    # Skipped in CI, containers, or when token already present.
    if [ "$_SYS_CENZONTLE_CONFIGURED" = "false" ] \
        && [ "$_SYS_IS_CI" = "0" ] \
        && [ "$_SYS_IS_CONTAINER" = "0" ] \
        && [ -t 1 ]; then
        printf "\n${CYAN}  Cenzontle AI${RESET} — enterprise platform (Zuclubit SSO)\n"
        printf "  ${CYAN}→${RESET} Log in once; token stored securely in OS keystore.\n"
        printf "  ${CYAN}→${RESET} Makes Cenzontle your default provider automatically.\n"
        printf "\n"
        printf "  Log in to Cenzontle now? [y/N] "
        read -r _cz_ans 2>/dev/null || _cz_ans="n"
        case "$_cz_ans" in
            [Yy]*)
                HALCON_BIN="${INSTALL_DIR:-$HOME/.local/bin}/halcon"
                if [ -x "$HALCON_BIN" ]; then
                    if "$HALCON_BIN" auth login cenzontle; then
                        # Re-detect token after successful login
                        _SYS_CENZONTLE_CONFIGURED="true"
                        _SYS_DEFAULT_PROVIDER="cenzontle"
                        _patch_cenzontle_config "$CONFIG_FILE"
                        ok "Cenzontle: authenticated and set as default provider"
                    else
                        warn "Cenzontle login failed — run: halcon auth login cenzontle"
                    fi
                else
                    warn "Binary not yet in PATH — run: halcon auth login cenzontle"
                fi
                ;;
            *)
                info "Skipping Cenzontle login — run: halcon auth login cenzontle"
                ;;
        esac
    elif [ "$_SYS_CENZONTLE_CONFIGURED" = "true" ]; then
        if [ "$_SYS_OS" = "Darwin" ]; then
            _cz_backend="macOS Keychain"
        elif command -v secret-tool >/dev/null 2>&1; then
            _cz_backend="D-Bus Secret Service"
        else
            _cz_backend="XDG file store"
        fi
        ok "Cenzontle: active (token found in ${_cz_backend})"
    fi
}

_write_config() {
    local dest="$1"
    # NOTE: unquoted heredoc — shell variables expand intentionally.
    # Dollar signs that should be literal in TOML (none here) would need \$.
    cat > "$dest" << HALCON_CONFIG
# ═══════════════════════════════════════════════════════════════════════════════
#  HALCÓN CLI — System-Adapted Configuration
#  Generated by install.sh on $(date '+%Y-%m-%d %H:%M %Z')
#
#  System profile:
#    OS       : ${_SYS_OS} ${_SYS_ARCH}
#    CPU      : ${_SYS_CORES} cores
#    RAM      : ${_SYS_RAM_GB} GB
#    Disk     : ${_SYS_DISK_GB} GB free
#    GPU      : ${_SYS_HAS_GPU}  |  Ollama: ${_SYS_OLLAMA_ENABLED}
#    Cenzontle: ${_SYS_CENZONTLE_CONFIGURED}  (default_provider → ${_SYS_DEFAULT_PROVIDER})
#
#  Usage:
#    halcon chat --tui --full --expert
#    halcon -p openai    chat --tui --full --expert
#    halcon -p deepseek  chat --tui --full --expert   # cheapest
#    halcon -p ollama    chat --tui --full --expert   # local / no API key
#
#  Add API keys:
#    halcon auth login anthropic
#    halcon auth login cenzontle   # Cenzontle SSO (Zuclubit enterprise)
# ═══════════════════════════════════════════════════════════════════════════════

# ── General ───────────────────────────────────────────────────────────────────
[general]
default_provider = "${_SYS_DEFAULT_PROVIDER}"
default_model    = "claude-sonnet-4-6"
max_tokens       = 16000
temperature      = 0.0

# ── Display ───────────────────────────────────────────────────────────────────
[display]
show_banner         = true
animations          = ${_SYS_ANIMATIONS}
theme               = "${_SYS_THEME}"
ui_mode             = "expert"
brand_color         = "#e85200"
terminal_background = "#1a1a1a"
compact_width       = 0

# ── Agent Limits ─────────────────────────────────────────────────────────────
# Tuned for: ${_SYS_CORES} cores / ${_SYS_RAM_GB} GB RAM
[agent.limits]
max_rounds              = 40
max_total_tokens        = 0
max_duration_secs       = ${_SYS_MAX_DURATION}
tool_timeout_secs       = ${_SYS_TOOL_TIMEOUT}
provider_timeout_secs   = ${_SYS_PROVIDER_TIMEOUT}
max_parallel_tools      = ${_SYS_MAX_PARALLEL}
max_tool_output_chars   = 100000
max_concurrent_agents   = ${_SYS_MAX_AGENTS}
max_cost_usd            = 0.0
clarification_threshold = 0.6

# ── Agent Routing ─────────────────────────────────────────────────────────────
[agent.routing]
strategy    = "quality"
mode        = "failover"
max_retries = 1
fallback_models = [
    "claude-haiku-4-5-20251001",
    "claude-sonnet-4-6",
    "gpt-4o-mini",
]
speculation_providers = []

# ── Compaction ────────────────────────────────────────────────────────────────
# max_context_tokens tuned for ${_SYS_RAM_GB} GB RAM
[agent.compaction]
enabled            = true
threshold_fraction = 0.55
keep_recent        = 8
max_context_tokens = ${_SYS_MAX_CTX}

# ── Model Selection ───────────────────────────────────────────────────────────
[agent.model_selection]
enabled                    = true
budget_cap_usd             = 0.0
complexity_token_threshold = 2000

# ── Planning ──────────────────────────────────────────────────────────────────
[planning]
enabled              = true
adaptive             = true
max_replans          = 3
min_confidence       = 0.65
timeout_secs         = 45
auto_learn_playbooks = false

# ── Reasoning ─────────────────────────────────────────────────────────────────
[reasoning]
enabled             = true
success_threshold   = 0.6
max_retries         = 2
exploration_factor  = 1.4
learning            = true
enable_loop_critic  = true
critic_timeout_secs = 60
critic_model        = "claude-haiku-4-5-20251001"
critic_provider     = "anthropic"

# ── Reflexion ─────────────────────────────────────────────────────────────────
[reflexion]
enabled            = true
max_reflections    = 5
reflect_on_success = false

# ── Memory ────────────────────────────────────────────────────────────────────
[memory]
enabled                = true
max_entries            = 10000
auto_summarize         = true
episodic               = true
retrieval_top_k        = 5
retrieval_token_budget = 2000
decay_half_life_days   = 14.0
rrf_k                  = 60.0

# ── Orchestrator ──────────────────────────────────────────────────────────────
[orchestrator]
enabled                   = true
max_concurrent_agents     = ${_SYS_MAX_AGENTS}
sub_agent_timeout_secs    = 270
shared_budget             = true
enable_communication      = false
min_delegation_confidence = 0.7

# ── Task Framework ────────────────────────────────────────────────────────────
[task_framework]
enabled               = true
persist_tasks         = true
default_max_retries   = 3
default_retry_base_ms = 500
resume_on_startup     = false
strict_enforcement    = false

# ── Context ───────────────────────────────────────────────────────────────────
[context]
dynamic_tool_selection = true

[context.governance]
default_max_tokens_per_source = 0
default_ttl_secs              = 0

# ── Context Servers ───────────────────────────────────────────────────────────
[context_servers]
enabled = true

[context_servers.requirements]
enabled = true
priority = 100
token_budget = 2000
cache_ttl_secs = 3600

[context_servers.architecture]
enabled = true
priority = 90
token_budget = 2000
cache_ttl_secs = 3600

[context_servers.codebase]
enabled = true
priority = 80
token_budget = 2000
cache_ttl_secs = 3600

[context_servers.workflow]
enabled = true
priority = 70
token_budget = 1500
cache_ttl_secs = 3600

[context_servers.testing]
enabled = true
priority = 60
token_budget = 1500
cache_ttl_secs = 3600

[context_servers.security]
enabled = true
priority = 40
token_budget = 1000
cache_ttl_secs = 3600

# ── Security ──────────────────────────────────────────────────────────────────
[security]
pii_detection          = false
pii_action             = "redact"
audit_enabled          = true
tbac_enabled           = true
pre_execution_critique = false
session_grant_ttl_secs = 300
scan_system_prompts    = false

[security.guardrails]
enabled  = true
builtins = true
rules    = []

[security.analysis_mode]
enabled                  = true
allow_grep_recursive     = true
allow_find_project_files = true
analysis_tool_whitelist  = [
    "grep ", "grep -", "rg ",
    "find . ", "find src", "find crates",
    "cat ", "head ", "tail ", "wc ", "ls ",
    "cargo audit", "cargo check", "cargo test --",
    "npm audit", "npm ls", "yarn audit",
    "git log ", "git diff ", "git status", "git show ",
]

# ── Tools ─────────────────────────────────────────────────────────────────────
[tools]
confirm_destructive       = ${_SYS_CONFIRM_DESTRUCTIVE}
timeout_secs              = ${_SYS_TOOL_TIMEOUT}
prompt_timeout_secs       = 45
auto_approve_in_ci        = ${_SYS_AUTO_APPROVE_CI}
allow_write_in_ci         = false
allow_destructive_in_ci   = false
dry_run                   = false
command_blacklist         = []
disable_builtin_blacklist = false
allowed_directories = [
${_SYS_ALLOWED_DIRS}
]
blocked_patterns = [
    "**/.env",
    "**/.env.*",
    "**/*.pem",
    "**/*.key",
    "**/credentials.json",
    "**/.ssh/**",
]

[tools.sandbox]
enabled             = true
max_output_bytes    = 10485760
max_memory_mb       = ${_SYS_SANDBOX_MEM}
max_cpu_secs        = 60
max_file_size_bytes = 104857600

[tools.retry]
max_retries   = 3
base_delay_ms = 500
max_delay_ms  = 10000

# ── Cache ─────────────────────────────────────────────────────────────────────
[cache]
enabled          = true
default_ttl_secs = 3600
max_entries      = 1000
prompt_cache     = true

# ── Search ────────────────────────────────────────────────────────────────────
# max_documents tuned for ${_SYS_DISK_GB} GB free disk
[search]
enabled         = true
max_documents   = ${_SYS_MAX_DOCS}
enable_semantic = true
enable_cache    = true

[search.ranking]
bm25_weight             = 0.6
semantic_weight         = 0.3
pagerank_weight         = 0.1
use_rrf                 = true
min_semantic_similarity = 0.25

[search.query]
default_results      = 10
enable_feedback_loop = true

# ── Multimodal ────────────────────────────────────────────────────────────────
# mode = "${_SYS_MULTIMODAL_MODE}" (GPU detected: ${_SYS_HAS_GPU})
[multimodal]
enabled                 = true
mode                    = "${_SYS_MULTIMODAL_MODE}"
max_file_size_bytes     = ${_SYS_MULTIMODAL_MAX_FILE}
local_threshold_bytes   = 2097152
strip_exif              = true
privacy_strict          = false
max_audio_duration_secs = 300
max_video_duration_secs = 120
video_sample_fps        = 2
max_video_frames        = 25
max_concurrent_analyses = ${_SYS_MAX_AGENTS}
cache_enabled           = true
cache_ttl_secs          = 3600
api_timeout_ms          = 30000

# ── Resilience ────────────────────────────────────────────────────────────────
[resilience]
enabled = true

[resilience.circuit_breaker]
failure_threshold  = 5
window_secs        = 60
open_duration_secs = 30
half_open_probes   = 2

[resilience.health]
window_minutes      = 60
degraded_threshold  = 50
unhealthy_threshold = 30

[resilience.backpressure]
max_concurrent_per_provider = 5
queue_timeout_secs          = 30

# ── Storage ───────────────────────────────────────────────────────────────────
[storage]
max_sessions         = 1000
max_session_age_days = 90

# ── Plugins ───────────────────────────────────────────────────────────────────
[plugins]
enabled    = true
plugin_dir = "$HOME/.halcon/plugins"

# ── Logging ───────────────────────────────────────────────────────────────────
[logging]
level  = "${_SYS_LOG_LEVEL}"
format = "pretty"

# ── MCP ───────────────────────────────────────────────────────────────────────
[mcp]
max_reconnect_attempts = 3

# ── MCP Server (Halcon as MCP server for Claude Code etc.) ────────────────────
[mcp_server]
enabled          = false
transport        = "stdio"
port             = 7777
expose_agents    = true
require_auth     = true
session_ttl_secs = 1800

# ── Providers ─────────────────────────────────────────────────────────────────
[models.providers.anthropic]
enabled       = true
api_key_env   = "ANTHROPIC_API_KEY"
api_base      = "https://api.anthropic.com"
default_model = "claude-sonnet-4-6"

[models.providers.anthropic.http]
connect_timeout_secs = 10
request_timeout_secs = ${_SYS_PROVIDER_TIMEOUT}
max_retries          = 3
retry_base_delay_ms  = 1000

[models.providers.openai]
enabled       = true
api_key_env   = "OPENAI_API_KEY"
api_base      = "https://api.openai.com/v1"
default_model = "gpt-4o"

[models.providers.openai.http]
connect_timeout_secs = 10
request_timeout_secs = ${_SYS_PROVIDER_TIMEOUT}
max_retries          = 3
retry_base_delay_ms  = 1000

[models.providers.deepseek]
enabled       = true
api_key_env   = "DEEPSEEK_API_KEY"
api_base      = "https://api.deepseek.com"
default_model = "deepseek-chat"

[models.providers.deepseek.http]
connect_timeout_secs = 15
request_timeout_secs = ${_SYS_PROVIDER_TIMEOUT}
max_retries          = 3
retry_base_delay_ms  = 1000

[models.providers.gemini]
enabled       = true
api_key_env   = "GEMINI_API_KEY"
api_base      = "https://generativelanguage.googleapis.com"
default_model = "gemini-2.5-flash"

[models.providers.gemini.http]
connect_timeout_secs = 10
request_timeout_secs = ${_SYS_PROVIDER_TIMEOUT}
max_retries          = 3
retry_base_delay_ms  = 1000

[models.providers.ollama]
enabled       = ${_SYS_OLLAMA_ENABLED}
api_base      = "http://localhost:11434"
default_model = "${_SYS_OLLAMA_MODEL}"

[models.providers.ollama.http]
connect_timeout_secs = 10
request_timeout_secs = 300
max_retries          = 3
retry_base_delay_ms  = 1000

# ── Claude Code CLI ───────────────────────────────────────────────────────────
${_SYS_CLAUDE_SECTION}

# ── Cenzontle — plataforma AI propia de Zuclubit (SSO OAuth 2.1 PKCE) ────────
# enabled = ${_SYS_CENZONTLE_CONFIGURED}  (auto-detected at install time)
# To authenticate: halcon auth login cenzontle
# Env var override: export CENZONTLE_ACCESS_TOKEN=<token>
[models.providers.cenzontle]
enabled       = ${_SYS_CENZONTLE_CONFIGURED}
api_base      = "https://ca-cenzontle-backend.graypond-e35bfdd8.eastus2.azurecontainerapps.io"
api_key_env   = "CENZONTLE_ACCESS_TOKEN"
default_model = "claude-sonnet-4-6"

[models.providers.cenzontle.http]
connect_timeout_secs = 10
request_timeout_secs = ${_SYS_PROVIDER_TIMEOUT}
max_retries          = 3
retry_base_delay_ms  = 1000

# ── AWS Bedrock — Claude via Amazon Bedrock ───────────────────────────────────
# Activa: export CLAUDE_CODE_USE_BEDROCK=1
# Requiere: AWS_ACCESS_KEY_ID + AWS_SECRET_ACCESS_KEY + AWS_REGION
# Opcional: AWS_SESSION_TOKEN (credenciales temporales / IAM roles)
# Modelos cross-region: set cross_region = true para prefijos us.*/eu.*/ap.*
[models.providers.bedrock]
enabled       = false
region        = "us-east-1"
default_model = "anthropic.claude-sonnet-4-6"
cross_region  = false

# ── Azure AI Foundry — Claude y modelos GPT via Azure ────────────────────────
# Activa: export CLAUDE_CODE_USE_AZURE=1
# Requiere: AZURE_AI_ENDPOINT + AZURE_API_KEY
# Alternativa Entra ID: AZURE_CLIENT_ID + AZURE_TENANT_ID (sin API key)
[models.providers.azure_foundry]
enabled       = false
endpoint_env  = "AZURE_AI_ENDPOINT"
api_key_env   = "AZURE_API_KEY"
default_model = "claude-sonnet-4-6"
api_version   = "2024-05-01-preview"

[models.providers.azure_foundry.http]
connect_timeout_secs = 10
request_timeout_secs = ${_SYS_PROVIDER_TIMEOUT}
max_retries          = 3
retry_base_delay_ms  = 1000

# ── Google Vertex AI — Claude via Google Cloud ────────────────────────────────
# Activa: export CLAUDE_CODE_USE_VERTEX=1
# Requiere: ANTHROPIC_VERTEX_PROJECT_ID + Application Default Credentials
# Setup: gcloud auth application-default login
[models.providers.vertex]
enabled       = false
project_env   = "ANTHROPIC_VERTEX_PROJECT_ID"
region        = "us-east5"
default_model = "claude-sonnet-4-6"

[models.providers.vertex.http]
connect_timeout_secs = 10
request_timeout_secs = ${_SYS_PROVIDER_TIMEOUT}
max_retries          = 3
retry_base_delay_ms  = 1000

# ── Policy ────────────────────────────────────────────────────────────────────
[policy]
use_intent_pipeline          = true
use_boundary_decision_engine = true
use_halcon_md                = true
enable_hooks                 = true
enable_auto_memory           = true
memory_importance_threshold  = 0.30
enable_agent_registry        = true
enable_semantic_memory       = false
semantic_memory_top_k        = 5
success_threshold            = 0.6
halt_confidence_threshold    = 0.8
max_round_iterations         = 12

# ── SSO / Cenzontle ───────────────────────────────────────────────────────────
# Uncomment and fill in to enable enterprise SSO via Cenzontle OAuth 2.1 PKCE.
# [sso]
# enabled       = false
# provider      = "cenzontle"
# issuer_url    = "https://auth.your-domain.com"
# client_id     = "halcon"
# scopes        = ["openid", "profile", "email", "halcon:chat"]
# redirect_port = 9876
HALCON_CONFIG
}

# ─── Patch cenzontle into an existing config.toml ────────────────────────────
# Called when config already exists (upgrade path).
# Idempotent: safe to call multiple times.
_patch_cenzontle_config() {
    local cfg="$1"

    if [ "$_SYS_CENZONTLE_CONFIGURED" = "true" ]; then
        # Activate cenzontle: flip enabled = false → true
        if grep -q "^\[models.providers.cenzontle\]" "$cfg" 2>/dev/null; then
            # Use awk for portable in-place edit (sed -i differs between macOS/GNU)
            awk '
                /^\[models\.providers\.cenzontle\]/ { in_cz=1 }
                in_cz && /^enabled[[:space:]]*=/ {
                    print "enabled       = true"
                    in_cz=0
                    next
                }
                /^\[/ && !/^\[models\.providers\.cenzontle\]/ { in_cz=0 }
                { print }
            ' "$cfg" > "${cfg}.tmp" && mv "${cfg}.tmp" "$cfg"
            ok "Cenzontle: enabled = true (patched in ${cfg})"
        else
            # Section missing — append it
            cat >> "$cfg" << CZEOF

# ── Cenzontle — added by installer upgrade (token detected) ───────────────────
[models.providers.cenzontle]
enabled       = true
api_base      = "https://ca-cenzontle-backend.graypond-e35bfdd8.eastus2.azurecontainerapps.io"
api_key_env   = "CENZONTLE_ACCESS_TOKEN"
default_model = "claude-sonnet-4-6"

[models.providers.cenzontle.http]
connect_timeout_secs = 10
request_timeout_secs = 300
max_retries          = 3
retry_base_delay_ms  = 1000
CZEOF
            ok "Cenzontle: section added to ${cfg}"
        fi

        # Update default_provider if it's still anthropic
        if grep -q '^default_provider.*=.*"anthropic"' "$cfg" 2>/dev/null; then
            awk '{
                if (/^default_provider[[:space:]]*=/) {
                    print "default_provider = \"cenzontle\""
                } else {
                    print
                }
            }' "$cfg" > "${cfg}.tmp" && mv "${cfg}.tmp" "$cfg"
            ok "default_provider → cenzontle"
        fi
    else
        ok "Cenzontle: not configured — keeping existing config"
    fi
}

_setup_agent_registry() {
    local halcon_dir="$1"
    local agents_dir="${halcon_dir}/agents"
    local skills_dir="${halcon_dir}/skills"

    mkdir -p "$agents_dir" "$skills_dir" 2>/dev/null || true

    # Write a README so users know where to put agent definitions
    if [ ! -f "${agents_dir}/README.md" ]; then
        cat > "${agents_dir}/README.md" << 'AGENTS_README'
# Halcon Sub-Agent Definitions

Place agent definition files here (`*.md`) to register custom sub-agents.

## Format

```markdown
---
name: my-agent          # kebab-case, required
description: What this agent does  # shown in routing manifest
model: sonnet           # haiku | sonnet | opus | inherit (default)
max_turns: 10           # 1–100
tools: [file_read, grep]  # empty = inherit all
skills: [my-skill]        # skills from ~/.halcon/skills/
---

You are a specialized agent that...
```

## Usage

Agents are discovered automatically. Reference them in conversation:
  "Use the code-reviewer agent to check this PR"

Run `halcon agents list` to see all loaded agents.
AGENTS_README
    fi

    # Write a README for skills
    if [ ! -f "${skills_dir}/README.md" ]; then
        cat > "${skills_dir}/README.md" << 'SKILLS_README'
# Halcon Skill Definitions

Place skill files here (`*.md`) to create reusable capability bundles.
Skills are injected into agent system prompts via the `skills:` frontmatter key.

## Format

```markdown
---
name: security-rules
description: Security analysis guidelines
---

Always check for:
- SQL injection via parameterized queries
- XSS via output encoding
- SSRF via URL allowlists
```
SKILLS_README
    fi

    ok "Agent registry directories ready: ${agents_dir}"
}

_write_classifier_rules() {
    local dest="$1"
    [ -f "$dest" ] && { ok "Classifier rules already exist — skipping"; return; }

    cat > "$dest" << 'RULES_EOF'
# Halcon Classifier Rules — User Configuration
# Edit to customize intent classification without recompiling.
#
# Load order (first found wins):
#   1. $HALCON_CLASSIFIER_RULES  (env var)
#   2. .halcon/classifier_rules.toml  (project)
#   3. ~/.halcon/classifier_rules.toml  (user) ← this file
#   4. Built-in defaults
#
# Scores: 5.0=exact multi-word | 3.0=domain noun | 2.0=strong verb | 0.4=weak signal

[[rule]]
task_type  = "git_operation"
base_score = 5.0
keywords   = [
  "git commit", "git status", "git diff", "git log",
  "git add", "git push", "git pull", "git fetch",
  "git branch", "git merge", "git rebase", "git stash",
  "git checkout", "git cherry-pick",
  "commit changes", "stage files", "push changes",
  "pull request", "merge request",
]

[[rule]]
task_type  = "file_management"
base_score = 5.0
keywords   = [
  "delete file", "remove file", "rename file",
  "move file", "copy file", "create directory",
  "create folder", "list files", "show files",
  "find files", "search files", "file permissions",
]

[[rule]]
task_type  = "debugging"
base_score = 3.0
keywords   = [
  "stacktrace", "stack trace", "traceback",
  "segfault", "segmentation fault",
  "null pointer", "deadlock", "race condition",
  "memory leak", "buffer overflow",
  "panic at", "thread panicked", "core dump",
]

[[rule]]
task_type  = "debugging"
base_score = 2.0
keywords   = [
  "fix", "debug", "diagnose", "troubleshoot", "resolve",
  "not working", "broken", "crash",
  "arregla", "corrige", "depura", "soluciona", "no funciona",
]

[[rule]]
task_type  = "code_generation"
base_score = 2.0
keywords   = [
  "implement", "scaffold", "generate code", "bootstrap",
  "add function", "add method", "write a function",
  "write a class", "create a function", "build a",
]

[[rule]]
task_type  = "explanation"
base_score = 2.0
keywords   = [
  "explain", "describe", "walk me through", "how does",
  "what is", "what are", "why does", "clarify",
  "explica", "como funciona", "que es", "por que",
]

[[rule]]
task_type  = "code_modification"
base_score = 2.0
keywords   = [
  "modify", "change", "update", "edit", "refactor",
  "rename", "replace", "rewrite", "optimize",
  "modifica", "cambia", "actualiza", "refactoriza",
]

[[rule]]
task_type  = "research"
base_score = 2.0
keywords   = [
  "analyze", "investigate", "compare", "review", "assess",
  "audit", "benchmark", "profile",
  "analiza", "investiga", "revisa", "evalua",
]

[[rule]]
task_type  = "configuration"
base_score = 2.0
keywords   = [
  "configure", "setup", "set up", "install",
  "enable", "disable", "settings",
  "configura", "instala", "habilita",
]
RULES_EOF
    ok "Classifier rules installed: ${dest}"
}

_write_mcp_config() {
    local dest="$1"

    # Detect mcp-server-filesystem binary
    _MCP_BIN=""
    for _c in \
        "/opt/homebrew/bin/mcp-server-filesystem" \
        "/usr/local/bin/mcp-server-filesystem" \
        "$HOME/.local/bin/mcp-server-filesystem" \
        "$(command -v mcp-server-filesystem 2>/dev/null)"
    do
        [ -x "$_c" ] && { _MCP_BIN="$_c"; break; }
    done

    # Build JSON args array — one entry per line, OS-aware
    _mcp_args_json="        \"/tmp\""
    if [ "$_SYS_OS" = "Darwin" ]; then
        _mcp_args_json="        \"$HOME/Documents\",
        \"$HOME/Downloads\",
        \"$HOME/Desktop\",
        \"/tmp\""
    else
        _mcp_args_json="        \"$HOME\",
        \"/tmp\""
        for _d in "$HOME/Documents" "$HOME/Downloads" "$HOME/projects" "$HOME/dev" "$HOME/src"; do
            [ -d "$_d" ] && _mcp_args_json="$_mcp_args_json,
        \"$_d\""
        done
        if [ "$_SYS_IS_WSL" = "1" ] && [ -n "${_WIN_HOME:-}" ] && [ -d "$_WIN_HOME" ]; then
            _mcp_args_json="$_mcp_args_json,
        \"$_WIN_HOME\""
        fi
    fi

    if [ -n "$_MCP_BIN" ]; then
        cat > "$dest" << MCPEOF
{
  "mcpServers": {
    "filesystem": {
      "command": "$_MCP_BIN",
      "args": [
${_mcp_args_json}
      ]
    }
  }
}
MCPEOF
        ok "MCP filesystem server configured: ${_MCP_BIN}"
    else
        cat > "$dest" << 'MCPEOF'
{
  "mcpServers": {}
}
MCPEOF
        warn "mcp-server-filesystem not found — MCP left empty"
        warn "Install: npm install -g @modelcontextprotocol/server-filesystem"
    fi
    ok "MCP config written: ${dest}"
}

main "$@"
