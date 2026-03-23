// commands/update.rs — halcon self-update
//
// Downloads the latest (or a specific) release binary from releases.cli.cuervo.cloud,
// verifies its SHA-256, backs up the current binary, and replaces it atomically.
//
// Flags:
//   --check    Only check if a newer version is available; don't download
//   --force    Download and replace even if already on the latest version
//   --version  Install a specific version (e.g. "v0.2.1")

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Base URL for releases (can be overridden via env var for testing)
const RELEASES_URL: &str = "https://releases.cli.cuervo.cloud";
const SITE_URL: &str = "https://cli.cuervo.cloud";

/// Maximum number of versioned backups to keep on disk (oldest pruned first).
const MAX_BACKUPS: usize = 3;

/// Current binary's cargo version
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Platform target triple — resolved at compile time
fn artifact_target() -> &'static str {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "x86_64-unknown-linux-musl"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "aarch64-unknown-linux-gnu"
    } else if cfg!(all(target_os = "linux", target_arch = "arm")) {
        "armv7-unknown-linux-musleabihf"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "x86_64-apple-darwin"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "x86_64-pc-windows-msvc"
    } else if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
        "aarch64-pc-windows-msvc"
    } else {
        "x86_64-unknown-linux-musl" // safe fallback
    }
}

fn archive_extension() -> &'static str {
    if cfg!(target_os = "windows") {
        "zip"
    } else {
        "tar.gz"
    }
}

fn binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "halcon.exe"
    } else {
        "halcon"
    }
}

// ─── Manifest ───────────────────────────────────────────────────────────────

#[derive(serde::Deserialize, Debug)]
struct Manifest {
    version: String,
    artifacts: Vec<ManifestArtifact>,
    /// Optional release notes / changelog excerpt (markdown)
    #[serde(default)]
    release_notes: Option<String>,
    /// Channel this manifest was published on (stable / beta / nightly)
    #[serde(default)]
    channel: Option<String>,
    /// ISO-8601 publish timestamp
    #[serde(default)]
    published_at: Option<String>,
}

#[derive(serde::Deserialize, Debug)]
struct ManifestArtifact {
    name: String,
    sha256: String,
    #[allow(dead_code)]
    size: u64,
}

// ─── Main entry ─────────────────────────────────────────────────────────────

pub struct UpdateArgs {
    pub check: bool,
    pub force: bool,
    pub version: Option<String>,
    /// Release channel: "stable" (default), "beta", or "nightly"
    pub channel: Option<String>,
}

pub fn run(args: UpdateArgs) -> Result<()> {
    let releases_url =
        std::env::var("HALCON_RELEASES_URL").unwrap_or_else(|_| RELEASES_URL.to_string());

    let target_version = args.version.as_deref().map(|v| v.trim_start_matches('v'));

    // Resolve channel: CLI arg > env var > "stable"
    let channel = args
        .channel
        .as_deref()
        .or_else(|| std::env::var("HALCON_CHANNEL").ok().as_deref().map(|_| ""))
        .unwrap_or("stable");
    let channel = if channel.is_empty() {
        std::env::var("HALCON_CHANNEL").unwrap_or_else(|_| "stable".to_string())
    } else {
        channel.to_string()
    };

    // Pick manifest URL: versioned > channel > stable/latest
    let manifest_url = if let Some(v) = target_version {
        format!("{releases_url}/v{v}/manifest.json")
    } else {
        match channel.as_str() {
            "stable" | "" => format!("{releases_url}/latest/manifest.json"),
            ch => format!("{releases_url}/{ch}/manifest.json"),
        }
    };

    let channel_label = if channel == "stable" {
        String::new()
    } else {
        format!(" [{channel}]")
    };
    println!("  Checking for updates{channel_label}...");

    let manifest = fetch_manifest(&manifest_url).context("Failed to fetch release manifest")?;

    let remote_version = manifest.version.trim_start_matches('v').to_string();

    println!("  Current:  v{CURRENT_VERSION}");
    print!("  Latest:   v{remote_version}");
    if let Some(ref ts) = manifest.published_at {
        // Show only the date portion (first 10 chars of ISO-8601)
        let date = ts.get(..10).unwrap_or(ts.as_str());
        print!("  (released {date})");
    }
    println!();

    // Version comparison
    let needs_update = if args.force {
        println!("  --force: reinstalling v{remote_version}");
        true
    } else if remote_version == CURRENT_VERSION {
        println!("  Already up to date.");
        return Ok(());
    } else if version_gt(&remote_version, CURRENT_VERSION) {
        true
    } else {
        println!("  Current version is newer than remote (downgrade skipped; use --force).");
        return Ok(());
    };

    if !needs_update {
        return Ok(());
    }

    if args.check {
        println!("\n  New version available: v{remote_version}");
        if let Some(ref notes) = manifest.release_notes {
            if !notes.is_empty() {
                println!("\n  Release notes:");
                for line in notes.lines().take(10) {
                    println!("    {line}");
                }
            }
        }
        println!("\n  Run `halcon update` to install.");
        return Ok(());
    }

    // Build artifact name
    let target = artifact_target();
    let ext = archive_extension();
    let artifact_name = format!("halcon-{remote_version}-{target}.{ext}");
    let download_url = format!("{releases_url}/latest/{artifact_name}");

    // Find expected SHA
    let expected_sha = manifest
        .artifacts
        .iter()
        .find(|a| a.name == artifact_name)
        .map(|a| a.sha256.clone())
        .unwrap_or_default();

    println!("\n  Downloading v{remote_version} for {target}...");

    // Create temp dir
    let tmp_dir = tempfile::tempdir().context("Failed to create temp directory")?;

    // Download archive
    let archive_path = tmp_dir.path().join(&artifact_name);
    download_file(&download_url, &archive_path)
        .with_context(|| format!("Download failed: {download_url}"))?;

    println!("  Downloaded.");

    // Verify SHA-256
    if !expected_sha.is_empty() {
        print!("  Verifying SHA-256... ");
        std::io::stdout().flush().ok();
        let actual_sha = sha256_file(&archive_path)?;
        if actual_sha != expected_sha {
            bail!(
                "SHA-256 mismatch!\n  Expected: {expected_sha}\n  Got:      {actual_sha}\n\
                 Aborting — the downloaded file may be corrupted or tampered with."
            );
        }
        println!("OK");
    } else {
        println!("  WARN: SHA-256 not found in manifest, skipping verification");
    }

    // Extract binary
    print!("  Extracting... ");
    std::io::stdout().flush().ok();
    let new_binary = extract_binary(tmp_dir.path(), &archive_path, binary_name())?;
    println!("OK");

    // Locate current binary
    let current_exe =
        std::env::current_exe().context("Could not determine current executable path")?;
    let current_exe = canonicalize_best_effort(&current_exe);

    // Windows: cannot replace running binary — write alongside with instructions
    if cfg!(target_os = "windows") {
        let next_path = current_exe.with_file_name("halcon-new.exe");
        std::fs::copy(&new_binary, &next_path).context("Failed to write new binary")?;
        println!("\n  New binary written to: {}", next_path.display());
        println!("  To complete update, run:");
        println!(
            "    Move-Item -Force '{}' '{}'",
            next_path.display(),
            current_exe.display()
        );
        return Ok(());
    }

    // Unix: versioned backup → replace atomically
    // Backup name: halcon.bak.v{current_version} — keeps up to MAX_BACKUPS old binaries.
    let backup_name = format!(
        "{}.bak.v{CURRENT_VERSION}",
        current_exe
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
    );
    let backup_path = current_exe.with_file_name(&backup_name);
    print!("  Replacing binary... ");
    std::io::stdout().flush().ok();
    atomic_replace(&current_exe, &new_binary, &backup_path).context("Failed to replace binary")?;
    println!("OK");

    // Prune old backups — keep at most MAX_BACKUPS files matching *.bak.v*
    if let Some(parent) = current_exe.parent() {
        let bin_stem = current_exe
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        prune_backups(parent, &bin_stem, MAX_BACKUPS);
    }

    println!("\n  Halcon CLI updated to v{remote_version}.");
    println!("  Backup saved: {}", backup_path.display());
    println!("  To rollback: halcon update --version {CURRENT_VERSION}");

    // Verify new binary
    if let Ok(output) = std::process::Command::new(&current_exe)
        .arg("--version")
        .output()
    {
        let ver_out = String::from_utf8_lossy(&output.stdout);
        println!("  Verification: {}", ver_out.trim());
    }

    // Clean up duplicate binaries in other PATH locations to prevent version conflicts.
    // The active binary is at current_exe; remove stale copies elsewhere.
    clean_duplicate_binaries(&current_exe);

    Ok(())
}

/// Remove duplicate `halcon` binaries from well-known locations that are NOT
/// the active binary. Prevents PATH shadowing after updates.
fn clean_duplicate_binaries(active: &Path) {
    let active = canonicalize_best_effort(active);
    let home = dirs::home_dir().unwrap_or_default();
    let candidates = [
        PathBuf::from("/usr/local/bin/halcon"),
        home.join(".local/bin/halcon"),
        home.join(".cargo/bin/halcon"),
    ];
    for candidate in &candidates {
        if !candidate.exists() {
            continue;
        }
        let candidate = canonicalize_best_effort(candidate);
        if candidate == active {
            continue;
        }
        match std::fs::remove_file(&candidate) {
            Ok(()) => println!("  Removed stale binary: {}", candidate.display()),
            Err(e) => {
                // Permission denied is expected for /usr/local/bin without sudo
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    println!(
                        "  Note: old binary at {} needs manual removal (sudo rm)",
                        candidate.display()
                    );
                }
                // Other errors silently ignored (file vanished, etc.)
            }
        }
    }
    // Also clean legacy "cuervo" binaries
    for legacy in ["cuervo", "cuervo-desktop"] {
        for dir in ["/usr/local/bin", &home.join(".local/bin").to_string_lossy().into_owned(), &home.join(".cargo/bin").to_string_lossy().into_owned()] {
            let p = PathBuf::from(dir).join(legacy);
            if p.exists() {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn fetch_manifest(url: &str) -> Result<Manifest> {
    // Use a simple blocking HTTP GET via reqwest (tokio runtime already exists
    // but this command runs in async context, so we use blocking client)
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(format!("halcon-cli/{CURRENT_VERSION}"))
        .build()
        .context("Failed to build HTTP client")?;

    let resp = client
        .get(url)
        .send()
        .with_context(|| format!("GET {url}"))?;

    if !resp.status().is_success() {
        bail!("HTTP {} from {url}", resp.status());
    }

    let manifest: Manifest = resp.json().context("Failed to parse manifest JSON")?;

    Ok(manifest)
}

fn download_file(url: &str, dest: &Path) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300)) // 5 min for large files
        .user_agent(format!("halcon-cli/{CURRENT_VERSION}"))
        .build()
        .context("Failed to build HTTP client")?;

    let mut resp = client
        .get(url)
        .send()
        .with_context(|| format!("GET {url}"))?;

    if !resp.status().is_success() {
        bail!("HTTP {} downloading {url}", resp.status());
    }

    let mut file =
        std::fs::File::create(dest).with_context(|| format!("Create {}", dest.display()))?;

    // Stream with progress dots
    let mut downloaded: u64 = 0;
    let total = resp.content_length().unwrap_or(0);
    let mut last_print = 0u64;

    let mut buf = [0u8; 65536];
    use std::io::Read;
    loop {
        let n = resp.read(&mut buf).context("Read response")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).context("Write to temp file")?;
        downloaded += n as u64;

        if total > 0 && downloaded - last_print > total / 20 {
            print!(".");
            std::io::stdout().flush().ok();
            last_print = downloaded;
        }
    }
    if total > 0 {
        println!();
    }

    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let data = std::fs::read(path).with_context(|| format!("Read {}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    Ok(hex::encode(hasher.finalize()))
}

fn extract_binary(dir: &Path, archive: &Path, binary_name: &str) -> Result<PathBuf> {
    let archive_str = archive.to_string_lossy();

    if archive_str.ends_with(".tar.gz") {
        // tar extraction
        let file = std::fs::File::open(archive)
            .with_context(|| format!("Open archive: {}", archive.display()))?;
        let gz = flate2::read::GzDecoder::new(file);
        let mut tar = tar::Archive::new(gz);

        for entry in tar.entries().context("Read tar entries")? {
            let mut entry = entry.context("Read tar entry")?;
            let entry_path = entry.path().context("Entry path")?;
            let filename = entry_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();

            if filename == binary_name {
                let dest = dir.join(binary_name);
                entry
                    .unpack(&dest)
                    .with_context(|| format!("Unpack {binary_name}"))?;

                // Make executable
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mut perms = std::fs::metadata(&dest)?.permissions();
                    perms.set_mode(0o755);
                    std::fs::set_permissions(&dest, perms)?;
                }

                return Ok(dest);
            }
        }
        bail!("Binary '{binary_name}' not found in tar.gz archive");
    } else if archive_str.ends_with(".zip") {
        let file = std::fs::File::open(archive)
            .with_context(|| format!("Open archive: {}", archive.display()))?;
        let mut zip = zip::ZipArchive::new(file).context("Parse zip archive")?;

        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).context("Read zip entry")?;
            let filename = entry.name().to_string();
            let basename = Path::new(&filename)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();

            if basename == binary_name {
                let dest = dir.join(binary_name);
                let mut out = std::fs::File::create(&dest)
                    .with_context(|| format!("Create {}", dest.display()))?;
                std::io::copy(&mut entry, &mut out).context("Extract binary")?;
                return Ok(dest);
            }
        }
        bail!("Binary '{binary_name}' not found in zip archive");
    } else {
        bail!("Unknown archive format: {archive_str}");
    }
}

fn atomic_replace(current: &Path, new: &Path, backup: &Path) -> Result<()> {
    // Backup current binary
    std::fs::copy(current, backup).with_context(|| format!("Backup to {}", backup.display()))?;

    // Replace (on Unix, rename is atomic if on same filesystem)
    std::fs::rename(new, current)
        .or_else(|_| {
            // Fallback: copy + remove if rename fails (cross-device)
            std::fs::copy(new, current).map(|_| ())?;
            std::fs::remove_file(new).ok();
            Ok::<(), anyhow::Error>(())
        })
        .with_context(|| format!("Replace {}", current.display()))?;

    // Ensure executable permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(current)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(current, perms)?;
    }

    Ok(())
}

fn canonicalize_best_effort(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Remove oldest versioned backups so at most `keep` remain.
/// Backups match the pattern `{bin_stem}.bak.v*` in `dir`.
fn prune_backups(dir: &Path, bin_stem: &str, keep: usize) {
    let prefix = format!("{bin_stem}.bak.v");
    let mut backups: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with(&prefix))
                    .unwrap_or(false)
            })
            .collect(),
        Err(_) => return,
    };
    // Sort by modification time ascending (oldest first)
    backups.sort_by_key(|p| {
        p.metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });
    // Remove oldest until we're within the keep limit
    let excess = backups.len().saturating_sub(keep);
    for old in backups.into_iter().take(excess) {
        let _ = std::fs::remove_file(&old);
        tracing::debug!(path = %old.display(), "Pruned old backup");
    }
}

// ─── Public update info type ─────────────────────────────────────────────────

/// Pending update information read from local notification files.
///
/// Written by the background checker thread; consumed by interactive UI code
/// so the user sees a rich prompt (version, notes, size) rather than a bare
/// one-liner.
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub current: String,
    pub remote: String,
    pub notes: Option<String>,
    pub published_at: Option<String>,
    /// Total download size in bytes (0 = unknown)
    pub size_bytes: u64,
    /// Download URL for the artifact (resolved at check time)
    pub artifact_url: String,
    /// Expected SHA-256 hex digest
    pub artifact_sha256: String,
}

/// Return a pending update if one has been detected by the background checker.
///
/// Reads the set of small notification files written by `notify_if_update_available`.
/// Returns `None` immediately when no update is pending or on any I/O error.
pub fn get_pending_update_info() -> Option<UpdateInfo> {
    let halcon_dir = dirs_next()?;
    let note = halcon_dir.join(".update-available");
    let remote_raw = std::fs::read_to_string(&note).ok()?;
    let remote = remote_raw.trim().to_string();
    if remote.is_empty() || !version_gt(&remote, CURRENT_VERSION) {
        return None;
    }

    let notes = std::fs::read_to_string(halcon_dir.join(".update-notes"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let published_at = std::fs::read_to_string(halcon_dir.join(".update-date"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let size_bytes: u64 = std::fs::read_to_string(halcon_dir.join(".update-size"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);

    let artifact_url = std::fs::read_to_string(halcon_dir.join(".update-url"))
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    let artifact_sha256 = std::fs::read_to_string(halcon_dir.join(".update-sha256"))
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    Some(UpdateInfo {
        current: CURRENT_VERSION.to_string(),
        remote,
        notes,
        published_at,
        size_bytes,
        artifact_url,
        artifact_sha256,
    })
}

/// Interactive classic-mode update prompt (non-TUI).
///
/// Renders a crossterm box with version info, release notes, and download size,
/// then asks the user to confirm.  Returns `true` when the user chose to install.
pub fn run_interactive_classic(info: &UpdateInfo) -> anyhow::Result<bool> {
    use std::io::{BufRead, Write};

    let mut stdout = std::io::stdout();

    // Build size label
    let size_label = if info.size_bytes > 0 {
        let mb = info.size_bytes as f64 / 1_048_576.0;
        format!("  {:.1} MB", mb)
    } else {
        String::new()
    };

    let date_label = info
        .published_at
        .as_deref()
        .and_then(|d| d.get(..10))
        .map(|d| format!("  Released {d}"))
        .unwrap_or_default();

    // ─── Header ───────────────────────────────────────────────────────────────
    writeln!(stdout)?;
    writeln!(stdout, "  \x1b[1;33m⚡ Actualización disponible\x1b[0m")?;
    writeln!(
        stdout,
        "  \x1b[90m────────────────────────────────────────────────\x1b[0m"
    )?;
    writeln!(
        stdout,
        "  Versión actual:  \x1b[36mv{}\x1b[0m",
        info.current
    )?;
    writeln!(
        stdout,
        "  Nueva versión:   \x1b[1;32mv{}\x1b[0m{}{}",
        info.remote, date_label, size_label
    )?;
    writeln!(stdout)?;

    // ─── Release notes (capped at 12 lines) ────────────────────────────────
    if let Some(ref notes) = info.notes {
        writeln!(stdout, "  \x1b[1mNotas de versión:\x1b[0m")?;
        for line in notes.lines().take(12) {
            writeln!(stdout, "    {line}")?;
        }
        writeln!(stdout)?;
    }

    // ─── Confirmation ──────────────────────────────────────────────────────
    write!(stdout, "  \x1b[1m¿Instalar ahora? [S/n]\x1b[0m  ")?;
    stdout.flush()?;

    let stdin = std::io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    let trimmed = line.trim().to_lowercase();

    let confirmed = trimmed.is_empty()
        || trimmed == "s"
        || trimmed == "si"
        || trimmed == "y"
        || trimmed == "yes";

    if !confirmed {
        writeln!(
            stdout,
            "  Actualización pospuesta. Usa \x1b[1mhalcon update\x1b[0m cuando quieras."
        )?;
    }
    writeln!(stdout)?;

    Ok(confirmed)
}

/// Download and install from an `UpdateInfo` record, streaming progress to stderr.
///
/// Writes a real progress bar for each phase: downloading, verifying, replacing.
/// On success the current binary is replaced atomically and backups are pruned.
pub fn run_update_from_info(info: &UpdateInfo) -> anyhow::Result<()> {
    use std::io::Write;

    if info.artifact_url.is_empty() {
        // Fall back to the normal run() path which re-fetches the manifest
        return run(UpdateArgs {
            check: false,
            force: false,
            version: Some(info.remote.clone()),
            channel: None,
        });
    }

    let releases_url =
        std::env::var("HALCON_RELEASES_URL").unwrap_or_else(|_| RELEASES_URL.to_string());

    let tmp_dir = tempfile::tempdir().context("Failed to create temp directory")?;
    let target = artifact_target();
    let ext = archive_extension();
    let artifact_name = format!("halcon-{}-{target}.{ext}", info.remote);
    let archive_path = tmp_dir.path().join(&artifact_name);

    // ─── Download with progress ──────────────────────────────────────────────
    eprint!(
        "\r  Descargando v{}...  [                    ]   0%",
        info.remote
    );
    std::io::stderr().flush().ok();

    let url = if !info.artifact_url.is_empty() {
        info.artifact_url.clone()
    } else {
        format!("{releases_url}/latest/{artifact_name}")
    };

    download_with_progress(&url, &archive_path, |done, total| {
        if total > 0 {
            let pct = (done * 100 / total) as usize;
            let filled = pct / 5; // 20-char bar
            let bar: String = format!("{}{}", "█".repeat(filled), "░".repeat(20 - filled));
            let mb_done = done as f64 / 1_048_576.0;
            let mb_total = total as f64 / 1_048_576.0;
            eprint!(
                "\r  Descargando v{}...  [{bar}]  {pct:3}%  ({mb_done:.1}/{mb_total:.1} MB)",
                info.remote
            );
            std::io::stderr().flush().ok();
        }
    })?;
    eprintln!(
        "\r  Descargando v{}...  [████████████████████]  100%  ✓",
        info.remote
    );

    // ─── SHA-256 verification ─────────────────────────────────────────────────
    if !info.artifact_sha256.is_empty() {
        eprint!("  Verificando SHA-256... ");
        std::io::stderr().flush().ok();
        let actual = sha256_file(&archive_path)?;
        if actual != info.artifact_sha256 {
            anyhow::bail!(
                "SHA-256 no coincide!\n  Esperado: {}\n  Obtenido: {}\n\
                 Abortando — el archivo puede estar corrupto o comprometido.",
                info.artifact_sha256,
                actual
            );
        }
        eprintln!("✓");
    } else {
        eprintln!("  AVISO: SHA-256 no disponible en manifiesto, omitiendo verificación");
    }

    // ─── Extract ──────────────────────────────────────────────────────────────
    eprint!("  Extrayendo... ");
    std::io::stderr().flush().ok();
    let new_binary = extract_binary(tmp_dir.path(), &archive_path, binary_name())?;
    eprintln!("✓");

    // ─── Replace current binary ───────────────────────────────────────────────
    let current_exe =
        std::env::current_exe().context("No se pudo determinar el ejecutable actual")?;
    let current_exe = canonicalize_best_effort(&current_exe);

    if cfg!(target_os = "windows") {
        let next_path = current_exe.with_file_name("halcon-new.exe");
        std::fs::copy(&new_binary, &next_path).context("Error al escribir nuevo binario")?;
        eprintln!("\n  Nuevo binario: {}", next_path.display());
        eprintln!(
            "  Para completar: Move-Item -Force '{}' '{}'",
            next_path.display(),
            current_exe.display()
        );
        return Ok(());
    }

    eprint!("  Reemplazando binario... ");
    std::io::stderr().flush().ok();
    let backup_name = format!(
        "{}.bak.v{}",
        current_exe
            .file_name()
            .unwrap_or_default()
            .to_string_lossy(),
        info.current
    );
    let backup_path = current_exe.with_file_name(&backup_name);
    atomic_replace(&current_exe, &new_binary, &backup_path)?;
    eprintln!("✓");

    if let Some(parent) = current_exe.parent() {
        let bin_stem = current_exe
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        prune_backups(parent, &bin_stem, MAX_BACKUPS);
    }

    // ─── Clean up notification files ──────────────────────────────────────────
    if let Some(halcon_dir) = dirs_next() {
        for name in &[
            ".update-available",
            ".update-notes",
            ".update-date",
            ".update-size",
            ".update-url",
            ".update-sha256",
        ] {
            let _ = std::fs::remove_file(halcon_dir.join(name));
        }
    }

    eprintln!("\n  ✅ Halcon CLI actualizado a v{}.", info.remote);
    eprintln!("  Respaldo guardado: {}", backup_path.display());
    eprintln!("  Para revertir: halcon update --version {}", info.current);
    Ok(())
}

/// Re-execute the current process with the same arguments.
///
/// Uses `exec()` on Unix (replaces process image) and `Command::new().spawn()` + exit on Windows.
/// This enables seamless restart after a self-update without the user noticing.
pub fn reexec_with_current_args() -> ! {
    let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("halcon"));
    let args: Vec<_> = std::env::args_os().skip(1).collect();

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(&exe).args(&args).exec();
        eprintln!("halcon: re-exec failed: {err}");
        std::process::exit(1);
    }

    #[cfg(not(unix))]
    {
        let status = std::process::Command::new(&exe)
            .args(&args)
            .status()
            .unwrap_or_else(|e| {
                eprintln!("halcon: re-exec failed: {e}");
                std::process::exit(1);
            });
        std::process::exit(status.code().unwrap_or(0));
    }
}

/// Download `url` to `dest`, calling `progress(bytes_done, total_bytes)` for each chunk.
pub fn download_with_progress<F>(
    url: &str,
    dest: &std::path::Path,
    mut progress: F,
) -> anyhow::Result<()>
where
    F: FnMut(u64, u64),
{
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .user_agent(format!("halcon-cli/{CURRENT_VERSION}"))
        .build()
        .context("Failed to build HTTP client")?;

    let mut resp = client
        .get(url)
        .send()
        .with_context(|| format!("GET {url}"))?;

    if !resp.status().is_success() {
        anyhow::bail!("HTTP {} downloading {url}", resp.status());
    }

    let total = resp.content_length().unwrap_or(0);
    let mut file =
        std::fs::File::create(dest).with_context(|| format!("Create {}", dest.display()))?;

    let mut downloaded: u64 = 0;
    let mut buf = [0u8; 65536];
    use std::io::Read;
    loop {
        let n = resp.read(&mut buf).context("Read response")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).context("Write to temp file")?;
        downloaded += n as u64;
        progress(downloaded, total);
    }
    Ok(())
}

/// Check for updates silently in the background (best-effort, non-blocking).
///
/// Spawns a short-lived thread that fetches the manifest and writes a
/// notification file (`~/.halcon/.update-available`) if a newer version
/// is available.  Main process reads this file on next invocation to
/// show a one-line hint without slowing the startup path.
pub fn notify_if_update_available() {
    // Only check once per day — gate on a stamp file
    let stamp_path = dirs_next();
    let stamp_stale = stamp_path
        .as_ref()
        .map(|p| {
            let max_age = std::time::Duration::from_secs(86_400); // 24h
            p.join(".update-check")
                .metadata()
                .and_then(|m| m.modified())
                .map(|t| t.elapsed().unwrap_or(max_age + max_age) > max_age)
                .unwrap_or(true)
        })
        .unwrap_or(false);

    if !stamp_stale {
        return;
    }

    let releases_url =
        std::env::var("HALCON_RELEASES_URL").unwrap_or_else(|_| RELEASES_URL.to_string());

    let _ = std::thread::Builder::new()
        .name("halcon-update-check".into())
        .spawn(move || {
            let url = format!("{releases_url}/latest/manifest.json");
            let Ok(client) = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .user_agent(format!("halcon-cli/{CURRENT_VERSION}"))
                .build()
            else {
                return;
            };

            let Ok(resp) = client.get(&url).send() else {
                return;
            };
            if !resp.status().is_success() {
                return;
            }
            let Ok(manifest) = resp.json::<Manifest>() else {
                return;
            };

            let remote = manifest.version.trim_start_matches('v').to_string();
            if version_gt(&remote, CURRENT_VERSION) {
                if let Some(halcon_dir) = dirs_next() {
                    let _ = std::fs::write(halcon_dir.join(".update-available"), &remote);
                    // Save extra metadata for rich interactive UI
                    if let Some(ref notes) = manifest.release_notes {
                        let _ = std::fs::write(halcon_dir.join(".update-notes"), notes);
                    }
                    if let Some(ref ts) = manifest.published_at {
                        let _ = std::fs::write(halcon_dir.join(".update-date"), ts);
                    }
                    // Resolve artifact info for this platform
                    let target = artifact_target();
                    let ext = archive_extension();
                    let artifact_name = format!("halcon-{remote}-{target}.{ext}");
                    let art_url = format!("{releases_url}/latest/{artifact_name}");
                    let _ = std::fs::write(halcon_dir.join(".update-url"), &art_url);
                    if let Some(art) = manifest.artifacts.iter().find(|a| a.name == artifact_name) {
                        let _ = std::fs::write(halcon_dir.join(".update-sha256"), &art.sha256);
                        let _ =
                            std::fs::write(halcon_dir.join(".update-size"), art.size.to_string());
                    }
                    // Touch the stamp to avoid re-checking for 24h
                    let _ = std::fs::write(halcon_dir.join(".update-check"), "");
                }
            } else {
                // Up to date — clear the notification files and touch stamp
                if let Some(halcon_dir) = dirs_next() {
                    for name in &[
                        ".update-available",
                        ".update-notes",
                        ".update-date",
                        ".update-size",
                        ".update-url",
                        ".update-sha256",
                    ] {
                        let _ = std::fs::remove_file(halcon_dir.join(name));
                    }
                    let _ = std::fs::write(halcon_dir.join(".update-check"), "");
                }
            }
        });
}

/// Read the pending update notification (if any) and print a one-line hint.
/// Called from the REPL startup path — total cost ≤ one file read.
pub fn print_update_hint() {
    let Some(halcon_dir) = dirs_next() else {
        return;
    };
    let note = halcon_dir.join(".update-available");
    if let Ok(ver) = std::fs::read_to_string(&note) {
        let ver = ver.trim();
        if !ver.is_empty() && version_gt(ver, CURRENT_VERSION) {
            eprintln!(
                "  \x1b[33m⚡ Update available:\x1b[0m v{CURRENT_VERSION} → v{ver}  \
                 Run \x1b[1mhalcon update\x1b[0m"
            );
        }
    }
}

fn dirs_next() -> Option<std::path::PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| std::path::PathBuf::from(h).join(".halcon"))
}

/// Returns true if `a` > `b` using simple semver comparison.
/// Supports "X.Y.Z" format.
fn version_gt(a: &str, b: &str) -> bool {
    parse_version(a) > parse_version(b)
}

fn parse_version(v: &str) -> (u64, u64, u64) {
    let parts: Vec<u64> = v
        .trim_start_matches('v')
        .split('-')
        .next()
        .unwrap_or(v) // strip pre-release suffix
        .splitn(3, '.')
        .map(|p| p.parse().unwrap_or(0))
        .collect();
    (
        parts.first().copied().unwrap_or(0),
        parts.get(1).copied().unwrap_or(0),
        parts.get(2).copied().unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_gt() {
        assert!(version_gt("0.3.0", "0.2.0"));
        assert!(version_gt("1.0.0", "0.9.9"));
        assert!(!version_gt("0.2.0", "0.2.0"));
        assert!(!version_gt("0.1.0", "0.2.0"));
        assert!(version_gt("0.2.1", "0.2.0"));
    }

    #[test]
    fn test_parse_version() {
        assert_eq!(parse_version("0.2.0"), (0, 2, 0));
        assert_eq!(parse_version("v1.2.3"), (1, 2, 3));
        assert_eq!(parse_version("1.0.0-alpha.1"), (1, 0, 0));
    }

    #[test]
    fn test_artifact_target_nonempty() {
        let t = artifact_target();
        assert!(!t.is_empty());
        assert!(t.contains('-'));
    }

    #[test]
    fn test_binary_name() {
        let name = binary_name();
        assert!(name.starts_with("halcon"));
    }

    #[test]
    fn test_versioned_backup_name_format() {
        // The backup name must include the current version so users can roll back.
        let backup_stem = format!("halcon.bak.v{CURRENT_VERSION}");
        assert!(backup_stem.starts_with("halcon.bak.v"));
        // Version should contain at least one dot
        let ver_part = backup_stem.trim_start_matches("halcon.bak.v");
        assert!(
            ver_part.contains('.'),
            "version in backup name must be semver"
        );
    }

    #[test]
    fn test_prune_backups_removes_oldest() {
        use std::fs;
        use std::time::Duration;
        let dir = tempfile::tempdir().expect("tempdir");
        let d = dir.path();

        // Create 5 fake backup files with predictable mtimes (write content to distinguish)
        for i in 0..5u32 {
            let p = d.join(format!("halcon.bak.v0.{i}.0"));
            fs::write(&p, i.to_string()).expect("write");
        }

        // prune to keep=3 → should delete the 2 oldest
        prune_backups(d, "halcon", 3);

        let remaining: Vec<_> = fs::read_dir(d)
            .expect("readdir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();

        assert_eq!(
            remaining.len(),
            3,
            "should keep exactly 3 backups; got {:?}",
            remaining
        );
    }

    #[test]
    fn test_manifest_channel_deserialization() {
        let json = r#"{
            "version": "0.3.0",
            "channel": "stable",
            "published_at": "2026-03-16T00:00:00Z",
            "artifacts": [],
            "release_notes": "- Fixed orchestrator budget tracking\n- Added channel support"
        }"#;
        let m: Manifest = serde_json::from_str(json).expect("parse manifest");
        assert_eq!(m.version, "0.3.0");
        assert_eq!(m.channel.as_deref(), Some("stable"));
        assert_eq!(m.published_at.as_deref(), Some("2026-03-16T00:00:00Z"));
        assert!(m.release_notes.as_deref().unwrap().contains("budget"));
    }

    #[test]
    fn test_manifest_missing_optional_fields() {
        // Old manifests without channel/release_notes/published_at must deserialize cleanly
        let json = r#"{"version": "0.2.0", "artifacts": []}"#;
        let m: Manifest = serde_json::from_str(json).expect("parse legacy manifest");
        assert_eq!(m.version, "0.2.0");
        assert!(m.channel.is_none());
        assert!(m.published_at.is_none());
        assert!(m.release_notes.is_none());
    }
}
