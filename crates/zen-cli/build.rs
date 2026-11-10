//! Build-time git revision for `zen doctor --json`.
//!
//! Emits `ZEN_GIT_SHA` (the full 40-char HEAD sha) so the running binary
//! can report the exact revision it was built from. Falls back to
//! `"unknown"` when git is unavailable (for example a source tarball
//! without `.git`). No new dependency: plain `git rev-parse`.
//!
//! On Windows hosts it also embeds a VERSIONINFO resource into the
//! binary (FileVersion, ProductName, CompanyName, ...). Code-signing
//! flows (SignPath) and the Microsoft Store MSIX package both expect
//! the executable to carry version metadata.

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
    embed_windows_version_info();
}

/// Embed the VERSIONINFO resource when the target is Windows. The
/// official Windows build runs on a Windows host in CI, so gating on
/// the host is enough; on any other host this is a no-op and the
/// `winresource` build-dependency is not even compiled.
#[cfg(windows)]
fn embed_windows_version_info() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // MSIX and VERSIONINFO both use four numeric parts; the crate
    // version has three (0.1.0 -> 0.1.0.0).
    let version = format!("{}.0", env!("CARGO_PKG_VERSION"));
    let mut res = winresource::WindowsResource::new();
    res.set("FileVersion", &version);
    res.set("ProductVersion", &version);
    res.set("ProductName", "ZenCode");
    res.set(
        "FileDescription",
        "ZenCode CLI - a coding agent for the terminal",
    );
    res.set("CompanyName", "7EN Labs");
    res.set("OriginalFilename", "zen.exe");
    res.set("InternalName", "zen");
    if let Err(err) = res.compile() {
        // Keep the build green on machines without the Windows SDK
        // resource compiler; CI verifies the metadata is present.
        println!("cargo:warning=failed to embed version resource: {err}");
    }
}

#[cfg(not(windows))]
fn embed_windows_version_info() {}
