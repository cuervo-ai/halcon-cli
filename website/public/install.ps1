# Halcón CLI Windows Installer (PowerShell)
# Usage: iwr -useb https://halcon.cuervo.cloud/install.ps1 | iex
# Or:    & ([scriptblock]::Create((iwr -useb https://halcon.cuervo.cloud/install.ps1))) -Version v0.3.0
[CmdletBinding()]
param(
    [string]$Version = "latest",
    [string]$InstallDir = "$env:LOCALAPPDATA\halcon\bin"
)

$ErrorActionPreference = "Stop"

$ReleasesUrl = "https://releases.cli.cuervo.cloud"
$ManifestUrl = "$ReleasesUrl/latest/manifest.json"
$BinaryName = "halcon.exe"
$Target = "x86_64-pc-windows-msvc"

function Write-Info  { Write-Host "  → $args" -ForegroundColor Cyan }
function Write-Ok    { Write-Host "  ✓ $args" -ForegroundColor Green }
function Write-Warn  { Write-Host "  ! $args" -ForegroundColor Yellow }
function Write-Fail  { Write-Host "  ✗ $args" -ForegroundColor Red; exit 1 }

# ─── Detect architecture ────────────────────────────────────────────────────
$Arch = [System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture
if ($Arch -eq "Arm64") {
    $Target = "aarch64-pc-windows-msvc"
    Write-Info "Architecture: ARM64"
} else {
    Write-Info "Architecture: x86_64"
}

Write-Host ""
Write-Host "  Halcon CLI Installer" -ForegroundColor White -BackgroundColor DarkBlue
Write-Host "  ─────────────────────"
Write-Host ""

# ─── Fetch manifest ─────────────────────────────────────────────────────────
Write-Info "Fetching release manifest..."
try {
    $Manifest = Invoke-RestMethod -Uri $ManifestUrl -Method Get -UseBasicParsing
} catch {
    Write-Fail "Failed to fetch manifest: $_"
}

$LatestVersion = $Manifest.version
if (-not $LatestVersion) { Write-Fail "Could not parse version from manifest" }
Write-Info "Latest version: $LatestVersion"

# ─── Build download URL ─────────────────────────────────────────────────────
$ArtifactName = "halcon-$LatestVersion-$Target.zip"
$DownloadUrl = "$ReleasesUrl/latest/$ArtifactName"

# Find expected SHA-256
$ExpectedSha = $null
foreach ($artifact in $Manifest.artifacts) {
    if ($artifact.name -eq $ArtifactName) {
        $ExpectedSha = $artifact.sha256
        break
    }
}

# ─── Download ───────────────────────────────────────────────────────────────
$TmpDir = [System.IO.Path]::GetTempPath() + "halcon-install-" + [System.Guid]::NewGuid().ToString("N")
New-Item -ItemType Directory -Path $TmpDir | Out-Null

try {
    $ArchivePath = Join-Path $TmpDir $ArtifactName
    Write-Info "Downloading $ArtifactName..."

    $ProgressPreference = 'SilentlyContinue'
    Invoke-WebRequest -Uri $DownloadUrl -OutFile $ArchivePath -UseBasicParsing
    $ProgressPreference = 'Continue'

    Write-Ok "Download complete"

    # ─── Verify SHA-256 ─────────────────────────────────────────────────────
    if ($ExpectedSha) {
        Write-Info "Verifying SHA-256..."
        $ActualSha = (Get-FileHash -Path $ArchivePath -Algorithm SHA256).Hash.ToLower()
        if ($ActualSha -ne $ExpectedSha.ToLower()) {
            Write-Fail "SHA-256 mismatch!`n  Expected: $ExpectedSha`n  Got:      $ActualSha"
        }
        Write-Ok "SHA-256 verified"
    } else {
        Write-Warn "SHA-256 not in manifest, skipping verification"
    }

    # ─── Extract ────────────────────────────────────────────────────────────
    Write-Info "Extracting..."
    Expand-Archive -Path $ArchivePath -DestinationPath $TmpDir -Force

    $BinarySrc = Join-Path $TmpDir $BinaryName
    if (-not (Test-Path $BinarySrc)) {
        Write-Fail "Binary not found in archive: $BinaryName"
    }

    # ─── Install ────────────────────────────────────────────────────────────
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    $BinaryDest = Join-Path $InstallDir $BinaryName
    Copy-Item $BinarySrc $BinaryDest -Force
    Write-Ok "Installed to $BinaryDest"

    # ─── Add to PATH ────────────────────────────────────────────────────────
    $CurrentPath = [System.Environment]::GetEnvironmentVariable("Path", "User")
    if ($CurrentPath -notlike "*$InstallDir*") {
        [System.Environment]::SetEnvironmentVariable(
            "Path",
            "$CurrentPath;$InstallDir",
            "User"
        )
        Write-Ok "Added $InstallDir to PATH (restart terminal to take effect)"
    }

    # ─── Verify ─────────────────────────────────────────────────────────────
    try {
        $InstalledVersion = & $BinaryDest --version 2>&1
        Write-Ok "Halcon CLI $LatestVersion installed: $InstalledVersion"
    } catch {
        Write-Warn "Binary installed but could not verify execution"
    }

} finally {
    Remove-Item -Recurse -Force $TmpDir -ErrorAction SilentlyContinue
}

Write-Host ""
Write-Host "  Get started:" -ForegroundColor Green
Write-Host "    halcon --help"
Write-Host "    halcon chat ""Hello, Halcón!"""
Write-Host ""
