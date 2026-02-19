#!/usr/bin/env bash
# Halcón CLI - Universal Binary Installer
# Usage: curl -sSfL https://cli.cuervo.cloud/install.sh | sh
# or:    wget -qO- https://cli.cuervo.cloud/install.sh | sh

set -euo pipefail

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Constants
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

readonly REPO_OWNER="${HALCON_REPO_OWNER:-cuervo-ai}"
readonly REPO_NAME="${HALCON_REPO_NAME:-halcon-cli}"
readonly BINARY_NAME="halcon"
readonly INSTALL_DIR="${HALCON_INSTALL_DIR:-$HOME/.local/bin}"
readonly GITHUB_API="https://api.github.com"
readonly GITHUB_DOWNLOAD="https://github.com/${REPO_OWNER}/${REPO_NAME}/releases/latest/download"
# Optional: set HALCON_GITHUB_TOKEN for private repo access, or HALCON_VERSION to pin a version
readonly HALCON_VERSION="${HALCON_VERSION:-}"
readonly HALCON_GITHUB_TOKEN="${HALCON_GITHUB_TOKEN:-}"

get_latest_version() {
    # Allow pinning a specific version via env var (also works for private repos without token)
    if [ -n "$HALCON_VERSION" ]; then
        echo "$HALCON_VERSION"
        return 0
    fi

    local auth_header=""
    if [ -n "$HALCON_GITHUB_TOKEN" ]; then
        auth_header="Authorization: Bearer ${HALCON_GITHUB_TOKEN}"
    fi

    local version
    if has curl; then
        if [ -n "$auth_header" ]; then
            version="$(curl --proto '=https' --tlsv1.2 -fsSL \
                -H "$auth_header" \
                "${GITHUB_API}/repos/${REPO_OWNER}/${REPO_NAME}/releases/latest" \
                | grep '"tag_name"' | sed 's/.*"tag_name": *"v\?\([^"]*\)".*/\1/')"
        else
            version="$(curl --proto '=https' --tlsv1.2 -fsSL \
                "${GITHUB_API}/repos/${REPO_OWNER}/${REPO_NAME}/releases/latest" \
                | grep '"tag_name"' | sed 's/.*"tag_name": *"v\?\([^"]*\)".*/\1/')"
        fi
    elif has wget; then
        if [ -n "$auth_header" ]; then
            version="$(wget --https-only -qO- \
                --header="$auth_header" \
                "${GITHUB_API}/repos/${REPO_OWNER}/${REPO_NAME}/releases/latest" \
                | grep '"tag_name"' | sed 's/.*"tag_name": *"v\?\([^"]*\)".*/\1/')"
        else
            version="$(wget --https-only -qO- \
                "${GITHUB_API}/repos/${REPO_OWNER}/${REPO_NAME}/releases/latest" \
                | grep '"tag_name"' | sed 's/.*"tag_name": *"v\?\([^"]*\)".*/\1/')"
        fi
    else
        error "Neither curl nor wget found."
    fi

    if [ -z "$version" ]; then
        error "Failed to determine latest release version. Try setting HALCON_VERSION=x.y.z or HALCON_GITHUB_TOKEN=<token>"
    fi
    echo "$version"
}

# Colors
readonly RED='\033[0;31m'
readonly GREEN='\033[0;32m'
readonly YELLOW='\033[1;33m'
readonly BLUE='\033[0;34m'
readonly MAGENTA='\033[0;35m'
readonly CYAN='\033[0;36m'
readonly BOLD='\033[1m'
readonly NC='\033[0m'

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Utility Functions
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

info() {
    echo -e "${BLUE}${BOLD}[INFO]${NC}  $*" >&2
}

success() {
    echo -e "${GREEN}${BOLD}[✓]${NC}     $*" >&2
}

warn() {
    echo -e "${YELLOW}${BOLD}[WARN]${NC}  $*" >&2
}

error() {
    echo -e "${RED}${BOLD}[ERROR]${NC} $*" >&2
    exit 1
}

header() {
    echo -e "\n${CYAN}${BOLD}━━━ $* ━━━${NC}\n" >&2
}

has() {
    command -v "$1" >/dev/null 2>&1
}

need() {
    if ! has "$1"; then
        error "Required command not found: $1"
    fi
}

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Platform Detection
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

detect_os() {
    local os
    os="$(uname -s)"
    case "$os" in
        Linux)  echo "linux" ;;
        Darwin) echo "darwin" ;;
        MINGW*|MSYS*|CYGWIN*) echo "windows" ;;
        *)      error "Unsupported OS: $os" ;;
    esac
}

detect_arch() {
    local arch
    arch="$(uname -m)"
    case "$arch" in
        x86_64|amd64)  echo "x86_64" ;;
        aarch64|arm64) echo "aarch64" ;;
        armv7l)        echo "armv7" ;;
        i686|i386)     echo "i686" ;;
        *)             error "Unsupported architecture: $arch" ;;
    esac
}

detect_libc() {
    local os="$1"
    if [ "$os" = "linux" ]; then
        if ldd --version 2>&1 | grep -q musl; then
            echo "musl"
        else
            echo "gnu"
        fi
    else
        echo ""
    fi
}

construct_target() {
    local os="$1"
    local arch="$2"
    local libc="$3"

    case "$os-$arch" in
        linux-x86_64)
            if [ "$libc" = "musl" ]; then
                echo "x86_64-unknown-linux-musl"
            else
                echo "x86_64-unknown-linux-gnu"
            fi
            ;;
        linux-aarch64)
            if [ "$libc" = "musl" ]; then
                echo "aarch64-unknown-linux-musl"
            else
                echo "aarch64-unknown-linux-gnu"
            fi
            ;;
        linux-armv7)
            echo "armv7-unknown-linux-gnueabihf"
            ;;
        darwin-x86_64)
            echo "x86_64-apple-darwin"
            ;;
        darwin-aarch64)
            echo "aarch64-apple-darwin"
            ;;
        windows-x86_64)
            echo "x86_64-pc-windows-msvc"
            ;;
        *)
            error "Unsupported platform: $os-$arch"
            ;;
    esac
}

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Download & Verification
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

download_file() {
    local url="$1"
    local output="$2"

    if has curl; then
        curl --proto '=https' --tlsv1.2 -fsSL "$url" -o "$output"
    elif has wget; then
        wget --https-only --secure-protocol=TLSv1_2 -q -O "$output" "$url"
    else
        error "Neither curl nor wget found. Please install one of them."
    fi
}

# For private GitHub repos, the browser download URL doesn't work — must use the API.
# Finds the asset by name in the releases/latest response, downloads via asset endpoint.
download_github_asset() {
    local asset_name="$1"
    local output="$2"

    if [ -z "$HALCON_GITHUB_TOKEN" ]; then
        # Public repo: use browser download URL directly
        local url="${GITHUB_DOWNLOAD}/${asset_name}"
        download_file "$url" "$output"
        return $?
    fi

    # Private repo: resolve asset API URL, then stream with Accept: application/octet-stream
    info "Resolving asset via GitHub API..."
    local release_json asset_url

    if has curl; then
        release_json="$(curl --proto '=https' --tlsv1.2 -fsSL \
            -H "Authorization: Bearer ${HALCON_GITHUB_TOKEN}" \
            "${GITHUB_API}/repos/${REPO_OWNER}/${REPO_NAME}/releases/latest")"
    elif has wget; then
        release_json="$(wget --https-only -qO- \
            --header="Authorization: Bearer ${HALCON_GITHUB_TOKEN}" \
            "${GITHUB_API}/repos/${REPO_OWNER}/${REPO_NAME}/releases/latest")"
    fi

    # Use python3 (available on macOS/Linux) to extract the asset API URL by name
    if has python3; then
        asset_url="$(echo "$release_json" | python3 -c "
import sys, json
data = json.load(sys.stdin)
for a in data.get('assets', []):
    if a.get('name') == '${asset_name}':
        print(a.get('url', ''))
        break
")"
    else
        # Fallback: awk-based extraction (url field is in assets array before name field)
        asset_url="$(echo "$release_json" | tr ',' '\n' | grep -E '"url":"https://api.github.com.*assets' | sed 's/.*"url":"\([^"]*\)".*/\1/' | head -1)"
    fi

    if [ -z "$asset_url" ]; then
        warn "Asset '$asset_name' not found in latest release"
        return 1
    fi

    if has curl; then
        curl --proto '=https' --tlsv1.2 -fsSL \
            -H "Authorization: Bearer ${HALCON_GITHUB_TOKEN}" \
            -H "Accept: application/octet-stream" \
            "$asset_url" -o "$output"
    elif has wget; then
        wget --https-only --secure-protocol=TLSv1_2 -q \
            --header="Authorization: Bearer ${HALCON_GITHUB_TOKEN}" \
            --header="Accept: application/octet-stream" \
            -O "$output" "$asset_url"
    fi
}

verify_checksum() {
    local file="$1"
    local checksum_file="$2"

    if ! has sha256sum; then
        warn "sha256sum not found, skipping checksum verification"
        return 0
    fi

    local expected_hash
    expected_hash="$(awk '{print $1}' "$checksum_file")"

    local actual_hash
    actual_hash="$(sha256sum "$file" | awk '{print $1}')"

    if [ "$expected_hash" != "$actual_hash" ]; then
        error "Checksum verification failed!\nExpected: $expected_hash\nGot:      $actual_hash"
    fi

    success "Checksum verified"
}

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Installation
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

extract_archive() {
    local archive="$1"
    local target_dir="$2"

    case "$archive" in
        *.tar.gz|*.tgz)
            need tar
            tar xzf "$archive" -C "$target_dir"
            ;;
        *.zip)
            need unzip
            unzip -q "$archive" -d "$target_dir"
            ;;
        *)
            error "Unsupported archive format: $archive"
            ;;
    esac
}

install_binary() {
    local binary_path="$1"
    local install_dir="$2"

    mkdir -p "$install_dir"
    cp "$binary_path" "$install_dir/${BINARY_NAME}"
    chmod +x "$install_dir/${BINARY_NAME}"

    success "Installed to $install_dir/${BINARY_NAME}"
}

detect_shell_profile() {
    if [ -n "${BASH_VERSION:-}" ]; then
        if [ -f "$HOME/.bashrc" ]; then
            echo "$HOME/.bashrc"
        elif [ -f "$HOME/.bash_profile" ]; then
            echo "$HOME/.bash_profile"
        else
            echo "$HOME/.profile"
        fi
    elif [ -n "${ZSH_VERSION:-}" ]; then
        echo "$HOME/.zshrc"
    elif [ -n "${FISH_VERSION:-}" ]; then
        echo "$HOME/.config/fish/config.fish"
    else
        case "$SHELL" in
            */bash)
                [ -f "$HOME/.bashrc" ] && echo "$HOME/.bashrc" || echo "$HOME/.profile"
                ;;
            */zsh)
                echo "$HOME/.zshrc"
                ;;
            */fish)
                echo "$HOME/.config/fish/config.fish"
                ;;
            *)
                echo "$HOME/.profile"
                ;;
        esac
    fi
}

add_to_path() {
    local install_dir="$1"

    if echo "$PATH" | tr ':' '\n' | grep -qx "$install_dir"; then
        success "$install_dir is already in PATH"
        return 0
    fi

    local shell_profile
    shell_profile="$(detect_shell_profile)"

    info "Adding $install_dir to PATH in $shell_profile"

    mkdir -p "$(dirname "$shell_profile")"
    touch "$shell_profile"

    if ! grep -q "export PATH=\"$install_dir:\$PATH\"" "$shell_profile"; then
        echo "" >> "$shell_profile"
        echo "# Added by halcon-cli installer" >> "$shell_profile"
        echo "export PATH=\"$install_dir:\$PATH\"" >> "$shell_profile"
        success "Added to PATH. Run: source $shell_profile"
    else
        success "PATH entry already exists in $shell_profile"
    fi
}

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Fallback Installation
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

try_cargo_binstall() {
    if ! has cargo-binstall; then
        return 1
    fi

    info "Attempting installation via cargo-binstall..."
    if cargo-binstall -y "${REPO_NAME}"; then
        success "Installed via cargo-binstall"
        return 0
    else
        return 1
    fi
}

try_cargo_install() {
    if ! has cargo; then
        error "cargo not found. Please install Rust from https://rustup.rs/"
    fi

    warn "No precompiled binary available for your platform."
    info "Falling back to cargo install (this will compile from source, may take 2-5 minutes)..."

    if cargo install --git "https://github.com/${REPO_OWNER}/${REPO_NAME}" --locked --no-default-features halcon-cli; then
        success "Installed via cargo install"
        return 0
    else
        error "cargo install failed"
    fi
}

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Main Installation Flow
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

main() {
    echo -e "${BOLD}${MAGENTA}"
    cat << 'EOF'
   ╔═══════════════════════════════════════╗
   ║      Halcón CLI - Installation        ║
   ╚═══════════════════════════════════════╝
EOF
    echo -e "${NC}"

    header "Detecting platform"

    local os arch libc target
    os="$(detect_os)"
    arch="$(detect_arch)"
    libc="$(detect_libc "$os")"
    target="$(construct_target "$os" "$arch" "$libc")"

    info "OS:           $os"
    info "Architecture: $arch"
    [ -n "$libc" ] && info "libc:         $libc"
    success "Target:       $target"

    header "Preparing download"

    local archive_ext
    [ "$os" = "windows" ] && archive_ext="zip" || archive_ext="tar.gz"

    info "Fetching latest release version..."
    local version
    version="$(get_latest_version)"
    success "Latest version: $version"

    local archive_name="${BINARY_NAME}-${version}-${target}.${archive_ext}"
    local checksum_name="${archive_name}.sha256"

    info "Asset:    $archive_name"

    # Use a global var so the EXIT trap can reference it (local vars are out of scope in traps)
    _HALCON_TMP="$(mktemp -d)"
    trap 'rm -rf "${_HALCON_TMP:-}"' EXIT
    local tmp_dir="$_HALCON_TMP"

    cd "$tmp_dir"

    header "Downloading binary"

    if ! download_github_asset "$archive_name" "$archive_name"; then
        warn "Failed to download precompiled binary for $target"

        if try_cargo_binstall; then
            exit 0
        elif try_cargo_install; then
            exit 0
        else
            error "All installation methods failed"
        fi
    fi

    success "Downloaded $archive_name"

    header "Verifying integrity"

    if download_github_asset "$checksum_name" "$checksum_name" 2>/dev/null; then
        verify_checksum "$archive_name" "$checksum_name"
    else
        warn "Checksum file not available, skipping verification"
    fi

    header "Extracting archive"

    extract_archive "$archive_name" .

    local binary_path
    if [ -f "${BINARY_NAME}" ]; then
        binary_path="${BINARY_NAME}"
    elif [ -f "${BINARY_NAME}.exe" ]; then
        binary_path="${BINARY_NAME}.exe"
    else
        binary_path="$(find . -name "${BINARY_NAME}" -o -name "${BINARY_NAME}.exe" | head -n1)"
        if [ -z "$binary_path" ]; then
            error "Binary not found in archive"
        fi
    fi

    success "Extracted binary: $binary_path"

    header "Installing"

    install_binary "$binary_path" "$INSTALL_DIR"

    header "Configuring PATH"

    add_to_path "$INSTALL_DIR"

    header "Verification"

    local installed_binary="$INSTALL_DIR/${BINARY_NAME}"
    if [ -x "$installed_binary" ]; then
        local version
        version="$("$installed_binary" --version 2>&1 || echo "unknown")"
        success "Installation verified: $version"
    else
        error "Binary not executable at $installed_binary"
    fi

    echo ""
    echo -e "${GREEN}${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${GREEN}${BOLD}   Installation complete! 🎉${NC}"
    echo -e "${GREEN}${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo ""
    echo -e "  ${BOLD}Next steps:${NC}"
    echo ""
    echo -e "  ${CYAN}1.${NC} Reload your shell:"
    echo -e "     ${BOLD}source $(detect_shell_profile)${NC}"
    echo ""
    echo -e "  ${CYAN}2.${NC} Verify installation:"
    echo -e "     ${BOLD}halcon --version${NC}"
    echo ""
    echo -e "  ${CYAN}3.${NC} Get started:"
    echo -e "     ${BOLD}halcon --help${NC}"
    echo ""
    echo -e "  ${BLUE}Documentation:${NC} https://github.com/${REPO_OWNER}/${REPO_NAME}"
    echo ""
}

main "$@"
