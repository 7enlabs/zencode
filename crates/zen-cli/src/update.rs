use anyhow::{Context, Result, anyhow};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const DEFAULT_REPO: &str = "https://github.com/7enlabs/zencore.git";
pub const DEFAULT_BRANCH: &str = "main";

pub fn default_src_dir(home: Option<&Path>) -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("ZENCODE_SRC_DIR")
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir));
    }
    home.map(|h| h.join(".local").join("share").join("zencode"))
}

fn repo_url() -> String {
    std::env::var("ZENCODE_REPO").unwrap_or_else(|_| DEFAULT_REPO.to_string())
}

fn find_cargo(home: Option<&Path>) -> Option<PathBuf> {
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            for name in ["cargo", "cargo.exe"] {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    if let Some(h) = home {
        for name in ["cargo", "cargo.exe"] {
            let candidate = h.join(".cargo").join("bin").join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn run(program: &Path, args: &[&str], cwd: Option<&Path>) -> Result<()> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let status = cmd
        .status()
        .with_context(|| format!("failed to run {}", program.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "command failed: {} {}",
            program.display(),
            args.join(" ")
        ))
    }
}

fn run_output(program: &str, args: &[&str], cwd: &Path) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("failed to run {program}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(anyhow!(
            "{} failed: {}",
            program,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn parse_ls_remote_head(out: &str) -> Option<String> {
    // Accept only a full 40-hex SHA as the remote HEAD: anything else
    // (error text, an empty response) is rejected rather than treated as
    // a version to update to.
    let first = out.lines().next()?;
    let sha = first.split_whitespace().next()?;
    if sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(sha.to_string())
    } else {
        None
    }
}

fn backup_path_for(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_os_string();
    name.push(".old");
    target.with_file_name(name)
}

fn built_binary_name() -> &'static str {
    if cfg!(windows) { "zen.exe" } else { "zen" }
}

pub fn run_update(
    check: bool,
    home: Option<&Path>,
    quiet: bool,
    json: bool,
    raw: bool,
) -> Result<i32> {
    let Some(src_dir) = default_src_dir(home) else {
        anyhow::bail!("cannot determine home directory; set ZENCODE_SRC_DIR");
    };
    let git = "git";
    if Command::new(git).arg("--version").output().is_err() {
        anyhow::bail!(
            "git not found; zen update works on a source checkout and needs Git - install Git first, then rerun. If you installed the prebuilt binary, re-run the installer one-command from INSTALL.md to update instead"
        );
    }

    if !src_dir.join(".git").is_dir() {
        if check {
            if json {
                let v = serde_json::json!({"status": "no_local_checkout", "path": src_dir.display().to_string()});
                if raw {
                    println!("{}", serde_json::to_string(&v)?);
                } else {
                    println!("{}", serde_json::to_string_pretty(&v)?);
                }
            } else if !quiet {
                println!("no local checkout found at {}", src_dir.display());
                println!("run zen update without --check to install from source");
            }
            return Ok(0);
        }
        if !quiet && !json {
            println!(
                "Cloning {} ({}) into {}",
                repo_url(),
                DEFAULT_BRANCH,
                src_dir.display()
            );
        }
        if let Some(parent) = src_dir.parent() {
            std::fs::create_dir_all(parent)?;
        }
        run(
            Path::new(git),
            &[
                "clone",
                "--branch",
                DEFAULT_BRANCH,
                "--depth",
                "1",
                &repo_url(),
                src_dir
                    .to_str()
                    .ok_or_else(|| anyhow!("invalid checkout path"))?,
            ],
            None,
        )?;
    } else {
        let status = run_output(git, &["status", "--porcelain"], &src_dir)?;
        // Never update over local changes: an update must not merge into,
        // or discard, work the user has not committed.
        if !status.trim().is_empty() {
            anyhow::bail!(
                "local checkout at {} has uncommitted changes; commit or stash them first",
                src_dir.display()
            );
        }
        let local = run_output(git, &["rev-parse", "HEAD"], &src_dir)?;
        let local = local.trim().to_string();
        let remote_out = run_output(git, &["ls-remote", "origin", "HEAD"], &src_dir)?;
        let remote = parse_ls_remote_head(&remote_out)
            .ok_or_else(|| anyhow!("could not read remote HEAD"))?;
        if check {
            if json {
                let v = serde_json::json!({"status": if local == remote { "up_to_date" } else { "update_available" }, "local": local, "remote": remote});
                if raw {
                    println!("{}", serde_json::to_string(&v)?);
                } else {
                    println!("{}", serde_json::to_string_pretty(&v)?);
                }
            } else if !quiet {
                if local == remote {
                    println!("up to date: {local}");
                } else {
                    println!("update available: local {local}, remote {remote}");
                }
            }
            return Ok(0);
        }
        if local == remote {
            if json {
                let v = serde_json::json!({"status": "up_to_date", "local": local});
                if raw {
                    println!("{}", serde_json::to_string(&v)?);
                } else {
                    println!("{}", serde_json::to_string_pretty(&v)?);
                }
            } else if !quiet {
                println!("already up to date: {local}");
            }
            return Ok(0);
        }
        if !quiet && !json {
            println!("Updating checkout from {local} to {remote}");
        }
        run(
            Path::new(git),
            &["fetch", "origin", DEFAULT_BRANCH],
            Some(&src_dir),
        )?;
        run(
            Path::new(git),
            &["checkout", DEFAULT_BRANCH],
            Some(&src_dir),
        )?;
        run(
            Path::new(git),
            // Fast-forward only: a diverged local checkout is never
            // auto-merged or force-updated; the user resolves it first.
            &["merge", "--ff-only", &format!("origin/{DEFAULT_BRANCH}")],
            Some(&src_dir),
        )?;
    }

    let Some(cargo) = find_cargo(home) else {
        anyhow::bail!(
            "cargo not found; zen update rebuilds from source and needs a Rust toolchain - install Rust first, for example from https://rustup.rs. If you installed the prebuilt binary, re-run the installer one-command from INSTALL.md to update instead"
        );
    };
    if !quiet && !json {
        println!("Building zen-cli in release mode (this can take a few minutes)");
    }
    run(
        &cargo,
        &[
            "build",
            "--release",
            "-p",
            "zen-cli",
            "--manifest-path",
            src_dir
                .join("Cargo.toml")
                .to_str()
                .ok_or_else(|| anyhow!("invalid manifest path"))?,
        ],
        None,
    )?;

    let built = src_dir
        .join("target")
        .join("release")
        .join(built_binary_name());
    if !built.is_file() {
        anyhow::bail!("build finished but {} is missing", built.display());
    }
    let current = std::env::current_exe()?;
    let target = current.with_file_name(built_binary_name());
    if built.canonicalize().ok() == target.canonicalize().ok() {
        if !quiet && !json {
            println!("already running the freshly built binary");
        }
        return Ok(0);
    }
    // Self-replacement: the running binary is moved aside to a `.old`
    // backup first, and a failed copy restores it, so a broken install
    // never leaves the user without a working binary.
    let backup = backup_path_for(&target);
    if backup.exists() {
        let _ = std::fs::remove_file(&backup);
    }
    if target.exists() {
        std::fs::rename(&target, &backup).with_context(
            || "could not move the running binary aside; close other zen processes and retry",
        )?;
    }
    if let Err(e) = std::fs::copy(&built, &target) {
        let _ = std::fs::rename(&backup, &target);
        return Err(e).context("could not install the new binary; the old binary was restored");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755));
    }
    let head = run_output(git, &["rev-parse", "--short", "HEAD"], &src_dir)?;
    if json {
        let v = serde_json::json!({"status": "updated", "rev": head.trim(), "path": target.display().to_string()});
        if raw {
            println!("{}", serde_json::to_string(&v)?);
        } else {
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
    } else if !quiet {
        println!("Updated zen to {} at {}", head.trim(), target.display());
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_src_dir_uses_home() {
        let dir = default_src_dir(Some(Path::new("/home/test")));
        assert_eq!(dir, Some(PathBuf::from("/home/test/.local/share/zencode")));
    }

    #[test]
    fn parse_ls_remote_head_reads_first_field() {
        assert_eq!(
            parse_ls_remote_head("0123456789abcdef0123456789abcdef01234567\tHEAD\n"),
            Some("0123456789abcdef0123456789abcdef01234567".to_string())
        );
        assert_eq!(parse_ls_remote_head(""), None);
        assert_eq!(parse_ls_remote_head("not-a-sha\tHEAD\n"), None);
    }

    #[test]
    fn run_update_check_without_checkout_returns_zero() {
        // No ZENCODE_SRC_DIR override and no checkout under home:
        // --check reports nothing to do and exits 0.
        let _env = crate::settings::env_lock();
        let old = std::env::var("ZENCODE_SRC_DIR").ok();
        unsafe { std::env::remove_var("ZENCODE_SRC_DIR") };
        let home =
            std::env::temp_dir().join(format!("zen_update_nocheckout_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let code = run_update(true, Some(&home), true, false, false).unwrap();
        assert_eq!(code, 0);
        let _ = std::fs::remove_dir_all(&home);
        match old {
            Some(v) => unsafe { std::env::set_var("ZENCODE_SRC_DIR", v) },
            None => unsafe { std::env::remove_var("ZENCODE_SRC_DIR") },
        }
    }

    #[test]
    fn run_update_bails_on_uncommitted_changes() {
        // A checkout with an untracked file must refuse to update
        // (and never reach the network) - an error, not Ok(0).
        let _env = crate::settings::env_lock();
        let old = std::env::var("ZENCODE_SRC_DIR").ok();
        unsafe { std::env::remove_var("ZENCODE_SRC_DIR") };
        let home = std::env::temp_dir().join(format!("zen_update_dirty_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let src = home.join(".local").join("share").join("zencode");
        std::fs::create_dir_all(&src).unwrap();
        let status = std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&src)
            .status()
            .expect("git init for the dirty-checkout fixture");
        assert!(status.success(), "git init must succeed, got {status}");
        std::fs::write(src.join("dirty.txt"), "uncommitted").unwrap();
        let err = run_update(false, Some(&home), true, false, false)
            .expect_err("dirty checkout must bail");
        assert!(
            err.to_string().contains("uncommitted changes"),
            "unexpected error: {err}"
        );
        let _ = std::fs::remove_dir_all(&home);
        match old {
            Some(v) => unsafe { std::env::set_var("ZENCODE_SRC_DIR", v) },
            None => unsafe { std::env::remove_var("ZENCODE_SRC_DIR") },
        }
    }

    #[test]
    fn backup_path_appends_old() {
        assert_eq!(
            backup_path_for(Path::new("/tmp/bin/zen")),
            PathBuf::from("/tmp/bin/zen.old")
        );
        assert_eq!(
            backup_path_for(Path::new("/tmp/bin/zen.exe")),
            PathBuf::from("/tmp/bin/zen.exe.old")
        );
    }
}
