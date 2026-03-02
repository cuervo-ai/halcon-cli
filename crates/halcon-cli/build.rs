use std::process::Command;

fn main() {
    // Embed git commit hash (or build tag if no git repo)
    let git_hash = Command::new("git")
        .args(["rev-parse", "--short=8", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok()
            } else {
                None
            }
        })
        .unwrap_or_else(|| "ctx-v2".to_string());
    println!("cargo:rustc-env=HALCON_GIT_HASH={}", git_hash.trim());

    // Embed build date (UTC) — cross-platform: Unix date → PowerShell → fallback
    let build_date = build_date_cross_platform();
    println!("cargo:rustc-env=HALCON_BUILD_DATE={build_date}");

    // Embed target triple
    if let Ok(target) = std::env::var("TARGET") {
        println!("cargo:rustc-env=HALCON_TARGET={target}");
    }

    // Re-run if git HEAD changes or source changes
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads/");
    println!("cargo:rerun-if-changed=src/");
}

fn build_date_cross_platform() -> String {
    // 1. Unix / macOS
    if let Ok(out) = Command::new("date").args(["-u", "+%Y-%m-%d"]).output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    // 2. Windows — PowerShell
    if let Ok(out) = Command::new("powershell")
        .args(["-NoProfile", "-Command", "Get-Date -Format 'yyyy-MM-dd'"])
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    // 3. CI override via environment variable
    if let Ok(v) = std::env::var("HALCON_BUILD_DATE_OVERRIDE") {
        if !v.is_empty() {
            return v;
        }
    }
    "unknown".to_string()
}
