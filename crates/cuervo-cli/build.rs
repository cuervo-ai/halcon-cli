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
    println!("cargo:rustc-env=CUERVO_GIT_HASH={}", git_hash.trim());

    // Embed build date (UTC)
    let build_date = Command::new("date")
        .args(["-u", "+%Y-%m-%d"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok()
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=CUERVO_BUILD_DATE={}", build_date.trim());

    // Embed target triple
    if let Ok(target) = std::env::var("TARGET") {
        println!("cargo:rustc-env=CUERVO_TARGET={target}");
    }

    // Re-run if git HEAD changes or source changes
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads/");
    println!("cargo:rerun-if-changed=src/");
}
