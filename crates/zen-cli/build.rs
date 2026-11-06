//! Build-time git revision for `zen doctor --json`.
//!
//! Emits `ZEN_GIT_SHA` (the full 40-char HEAD sha) so the running binary
//! can report the exact revision it was built from. Falls back to
//! `"unknown"` when git is unavailable (for example a source tarball
//! without `.git`). No new dependency: plain `git rev-parse`.

use std::process::Command;

fn main() {
    let sha = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=ZEN_GIT_SHA={sha}");
}
