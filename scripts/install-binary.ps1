# Cuervo CLI - Windows PowerShell Installer
# Usage: iwr -useb https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.ps1 | iex
# or:    Invoke-WebRequest -Uri https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.ps1 -UseBasicParsing | Invoke-Expression

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Constants
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

$REPO_OWNER = if ($env:CUERVO_REPO_OWNER) { $env:CUERVO_REPO_OWNER } else { "cuervo-ai" }
$REPO_NAME = if ($env:CUERVO_REPO_NAME) { $env:CUERVO_REPO_NAME } else { "cuervo-cli" }
$BINARY_NAME = "cuervo"
$INSTALL_DIR = if ($env:CUERVO_INSTALL_DIR) { $env:CUERVO_INSTALL_DIR } else { "$env:USERPROFILE\.local\bin" }
$GITHUB_DOWNLOAD = "https://github.com/$REPO_OWNER/$REPO_NAME/releases/latest/download"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Utility Functions
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

function Write-Info {
    param([string]$Message)
    Write-Host "[INFO]  $Message" -ForegroundColor Blue
}

function Write-Success {
    param([string]$Message)
    Write-Host "[✓]     $Message" -ForegroundColor Green
}

function Write-Warning {
    param([string]$Message)
    Write-Host "[WARN]  $Message" -ForegroundColor Yellow
}

function Write-ErrorMsg {
    param([string]$Message)
    Write-Host "[ERROR] $Message" -ForegroundColor Red
    exit 1
}

function Write-Header {
    param([string]$Message)
    Write-Host ""
    Write-Host "━━━ $Message ━━━" -ForegroundColor Cyan
    Write-Host ""
}

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Platform Detection
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

function Get-Architecture {
    if ([Environment]::Is64BitOperatingSystem) {
        return "x86_64"
    } else {
        return "i686"
    }
}

function Get-Target {
    $arch = Get-Architecture
    switch ($arch) {
        "x86_64" { return "x86_64-pc-windows-msvc" }
        "i686"   { return "i686-pc-windows-msvc" }
        default  { Write-ErrorMsg "Unsupported architecture: $arch" }
    }
}

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Download & Verification
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

function Download-File {
    param(
        [string]$Url,
        [string]$OutFile
    )

    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -Uri $Url -OutFile $OutFile -UseBasicParsing
    } catch {
        throw "Failed to download $Url : $_"
    }
}

function Verify-Checksum {
    param(
        [string]$File,
        [string]$ChecksumFile
    )

    if (-not (Get-Command Get-FileHash -ErrorAction SilentlyContinue)) {
        Write-Warning "Get-FileHash not available, skipping checksum verification"
        return
    }

    $expectedHash = (Get-Content $ChecksumFile).Split()[0]
    $actualHash = (Get-FileHash -Path $File -Algorithm SHA256).Hash

    if ($expectedHash -ne $actualHash) {
        Write-ErrorMsg "Checksum verification failed!`nExpected: $expectedHash`nGot:      $actualHash"
    }

    Write-Success "Checksum verified"
}

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Installation
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

function Install-Binary {
    param(
        [string]$BinaryPath,
        [string]$InstallDir
    )

    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    }

    $destination = Join-Path $InstallDir "$BINARY_NAME.exe"
    Copy-Item -Path $BinaryPath -Destination $destination -Force

    Write-Success "Installed to $destination"
}

function Add-ToPath {
    param([string]$InstallDir)

    $userPath = [Environment]::GetEnvironmentVariable("PATH", "User")

    if ($userPath -split ';' | Where-Object { $_ -eq $InstallDir }) {
        Write-Success "$InstallDir is already in PATH"
        return
    }

    Write-Info "Adding $InstallDir to PATH"

    $newPath = if ($userPath) { "$userPath;$InstallDir" } else { $InstallDir }
    [Environment]::SetEnvironmentVariable("PATH", $newPath, "User")

    $env:PATH = "$env:PATH;$InstallDir"

    Write-Success "Added to PATH (restart terminal to apply)"
}

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Fallback Installation
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

function Try-CargoInstall {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Write-ErrorMsg "cargo not found. Please install Rust from https://rustup.rs/"
    }

    Write-Warning "No precompiled binary available."
    Write-Info "Falling back to cargo install (may take 2-5 minutes)..."

    try {
        cargo install --git "https://github.com/$REPO_OWNER/$REPO_NAME" --locked
        Write-Success "Installed via cargo install"
    } catch {
        Write-ErrorMsg "cargo install failed: $_"
    }
}

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Main Installation Flow
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

function Main {
    Write-Host ""
    Write-Host "╔═══════════════════════════════════════╗" -ForegroundColor Magenta
    Write-Host "║      Cuervo CLI - Installation        ║" -ForegroundColor Magenta
    Write-Host "╚═══════════════════════════════════════╝" -ForegroundColor Magenta
    Write-Host ""

    Write-Header "Detecting platform"

    $target = Get-Target
    Write-Info "Target: $target"

    Write-Header "Preparing download"

    $archiveName = "$BINARY_NAME-$target.zip"
    $archiveUrl = "$GITHUB_DOWNLOAD/$archiveName"
    $checksumUrl = "$GITHUB_DOWNLOAD/$archiveName.sha256"

    Write-Info "Asset: $archiveName"
    Write-Info "URL:   $archiveUrl"

    $tmpDir = Join-Path $env:TEMP "cuervo-install-$(Get-Random)"
    New-Item -ItemType Directory -Path $tmpDir -Force | Out-Null

    try {
        Push-Location $tmpDir

        Write-Header "Downloading binary"

        try {
            Download-File -Url $archiveUrl -OutFile $archiveName
            Write-Success "Downloaded $archiveName"
        } catch {
            Write-Warning "Failed to download precompiled binary: $_"
            Try-CargoInstall
            return
        }

        Write-Header "Verifying integrity"

        try {
            Download-File -Url $checksumUrl -OutFile "$archiveName.sha256"
            Verify-Checksum -File $archiveName -ChecksumFile "$archiveName.sha256"
        } catch {
            Write-Warning "Checksum verification skipped: $_"
        }

        Write-Header "Extracting archive"

        Expand-Archive -Path $archiveName -DestinationPath . -Force

        $binaryPath = Get-ChildItem -Recurse -Filter "$BINARY_NAME.exe" | Select-Object -First 1 -ExpandProperty FullName

        if (-not $binaryPath) {
            Write-ErrorMsg "Binary not found in archive"
        }

        Write-Success "Extracted binary: $binaryPath"

        Write-Header "Installing"

        Install-Binary -BinaryPath $binaryPath -InstallDir $INSTALL_DIR

        Write-Header "Configuring PATH"

        Add-ToPath -InstallDir $INSTALL_DIR

        Write-Header "Verification"

        $installedBinary = Join-Path $INSTALL_DIR "$BINARY_NAME.exe"
        if (Test-Path $installedBinary) {
            try {
                $version = & $installedBinary --version 2>&1
                Write-Success "Installation verified: $version"
            } catch {
                Write-Success "Binary installed at $installedBinary"
            }
        } else {
            Write-ErrorMsg "Binary not found at $installedBinary"
        }

        Write-Host ""
        Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor Green
        Write-Host "   Installation complete! 🎉" -ForegroundColor Green
        Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor Green
        Write-Host ""
        Write-Host "  Next steps:" -ForegroundColor White
        Write-Host ""
        Write-Host "  1. Restart your terminal" -ForegroundColor Cyan
        Write-Host ""
        Write-Host "  2. Verify installation:" -ForegroundColor Cyan
        Write-Host "     cuervo --version" -ForegroundColor White
        Write-Host ""
        Write-Host "  3. Get started:" -ForegroundColor Cyan
        Write-Host "     cuervo --help" -ForegroundColor White
        Write-Host ""
        Write-Host "  Documentation: https://github.com/$REPO_OWNER/$REPO_NAME" -ForegroundColor Blue
        Write-Host ""

    } finally {
        Pop-Location
        Remove-Item -Path $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Main
