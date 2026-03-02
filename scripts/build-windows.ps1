# ──────────────────────────────────────────────────────────────────────────────
# scripts/build-windows.ps1
#
# Build halcon for Windows x86_64 (MSVC) natively on a Windows machine.
# Requires:
#   - Rust stable via rustup  (https://rustup.rs)
#   - Visual Studio 2019/2022 Build Tools with C++ workload
#     (or "Desktop development with C++" in VS Installer)
#   - cmake  (https://cmake.org/download/ — add to PATH)
#   - git    (https://git-scm.com)
#   - Perl   (for openssl-sys if needed; Strawberry Perl recommended)
#
# Usage (from project root in PowerShell):
#   .\scripts\build-windows.ps1
#   .\scripts\build-windows.ps1 -Debug
#   .\scripts\build-windows.ps1 -SkipPackage
# ──────────────────────────────────────────────────────────────────────────────
[CmdletBinding()]
param(
    [switch]$Debug,
    [switch]$SkipPackage
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

# ── Resolve workspace root ────────────────────────────────────────────────────
$ScriptDir   = Split-Path -Parent $MyInvocation.MyCommand.Path
$WorkspaceRoot = (Resolve-Path "$ScriptDir\..").Path
Set-Location $WorkspaceRoot

$Target = "x86_64-pc-windows-msvc"

# ── Build metadata ────────────────────────────────────────────────────────────
try   { $GitHash = (git rev-parse --short=8 HEAD 2>$null).Trim() } `
catch { $GitHash = "local" }

$BuildDate = Get-Date -Format "yyyy-MM-dd"
$Version   = (Get-Content Cargo.toml | Select-String '^version\s*=' | Select-Object -First 1) `
             -replace '.*"(.*)".*', '$1'

$env:HALCON_GIT_HASH           = $GitHash
$env:HALCON_BUILD_DATE         = $BuildDate
$env:HALCON_BUILD_DATE_OVERRIDE = $BuildDate  # build.rs reads this as fallback

# ── Dependency env vars ───────────────────────────────────────────────────────
# ONNX Runtime (fastembed / ort crate) — download prebuilt Windows binary
$env:ORT_STRATEGY   = "download"
$env:ORT_USE_CUDA   = "0"

# libgit2 — disable pkg-config, use static vendored build
$env:LIBGIT2_SYS_USE_PKG_CONFIG = "0"
$env:LIBGIT2_STATIC             = "1"

# zstd — use bundled
$env:ZSTD_SYS_USE_PKG_CONFIG = "0"

# ── Check prerequisites ───────────────────────────────────────────────────────
Write-Host "Checking prerequisites..."

@("cargo", "rustup", "cmake", "git") | ForEach-Object {
    if (-not (Get-Command $_ -ErrorAction SilentlyContinue)) {
        Write-Error "$_ not found in PATH. Please install it and retry."
    }
}

# Add Windows MSVC target
rustup target add $Target 2>$null

# ── Feature selection ─────────────────────────────────────────────────────────
# Windows: no color-science (momoto submodule), no tui (skips arboard which
# works on Windows but adds complexity; enable manually if desired).
# To enable clipboard support: change Features to "tui"
$Features = ""   # add "tui" if you want clipboard support on Windows

# ── Build ──────────────────────────────────────────────────────────────────────
$ProfileFlag = if ($Debug) { "" } else { "--release" }
$ProfileDir  = if ($Debug) { "debug" } else { "release" }

$BuildArgs = @("build", "--target", $Target, "-p", "halcon-cli", "--no-default-features")
if ($ProfileFlag) { $BuildArgs += $ProfileFlag }
if ($Features)    { $BuildArgs += @("--features", $Features) }

Write-Host ""
Write-Host ("=" * 66)
Write-Host " halcon v$Version — Windows MSVC ($Target)"
Write-Host " Profile : $(if ($Debug) { 'debug' } else { 'release' })"
Write-Host " Features: $(if ($Features) { $Features } else { '<none>' })"
Write-Host " Git hash: $GitHash   Build date: $BuildDate"
Write-Host ("=" * 66)
Write-Host "Running: cargo $($BuildArgs -join ' ')"
Write-Host ""

& cargo @BuildArgs
if ($LASTEXITCODE -ne 0) {
    Write-Error "cargo build failed with exit code $LASTEXITCODE"
}

# ── Locate binary ─────────────────────────────────────────────────────────────
$Binary = "target\$Target\$ProfileDir\halcon.exe"
if (-not (Test-Path $Binary)) {
    Write-Error "Expected binary not found: $Binary"
}

# ── Package ───────────────────────────────────────────────────────────────────
if (-not $SkipPackage) {
    $ArtifactName = "halcon-$Version-$Target"
    $DistDir      = "$WorkspaceRoot\dist"
    $PkgDir       = "$DistDir\$ArtifactName"

    New-Item -ItemType Directory -Path $PkgDir -Force | Out-Null

    Copy-Item $Binary "$PkgDir\halcon.exe"
    if (Test-Path "README.md") { Copy-Item "README.md" "$PkgDir\" }
    if (Test-Path "LICENSE")   { Copy-Item "LICENSE"   "$PkgDir\" }

    $Archive   = "$DistDir\$ArtifactName.zip"
    $ShaFile   = "$Archive.sha256"

    Compress-Archive -Path "$PkgDir\*" -DestinationPath $Archive -Force

    $Hash = (Get-FileHash -Path $Archive -Algorithm SHA256).Hash.ToLower()
    $Hash | Out-File -FilePath $ShaFile -Encoding ASCII -NoNewline

    # Cleanup staging dir
    Remove-Item -Recurse -Force $PkgDir

    $BinarySize = (Get-Item $Binary).Length
    $ArchiveSize = (Get-Item $Archive).Length

    Write-Host ""
    Write-Host ("=" * 66)
    Write-Host " Build complete"
    Write-Host ("   Binary : {0}  ({1:N0} bytes)" -f $Binary, $BinarySize)
    Write-Host ("   Archive: {0}  ({1:N0} bytes)" -f $Archive, $ArchiveSize)
    Write-Host "   SHA-256: $Hash"
    Write-Host ("=" * 66)
} else {
    Write-Host ""
    Write-Host "Build complete: $Binary"
}
