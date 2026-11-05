mod common;

use common::*;
use std::io::Read;
use std::time::{Duration, Instant};

// Behavior contract for a lone word in prompt position (design audit
// section 4.7, PR #35 follow-up): a mistyped command fails fast instead
// of being fed to the REPL as a first prompt, while real commands -
// including the clap-provided help subcommand - and the scripted-pipe
// REPL mode keep working.

#[test]
fn unknown_single_word_fails_fast_and_does_not_hang() {
    let home = TempDir::new("uc_home");
    let cwd = TempDir::new("uc_cwd");
    // Hold stdin open with no data: this is the audit hang condition.
    // If the word were swallowed as a prompt, the REPL would block here
    // waiting for input that never comes.
    let mut child = ZenRun::new("uc1", &home, &cwd)
        .arg("nosuchcmd")
        .spawn_piped();
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        match child.try_wait().expect("try_wait") {
            Some(s) => break s,
            None => {
                assert!(
                    Instant::now() < deadline,
                    "zen nosuchcmd did not exit within 15s with stdin held open: the word was treated as a prompt and the process hung"
                );
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    };
    drop(child.stdin.take());
    let mut out = Vec::new();
    let mut err = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    child.stderr.take().unwrap().read_to_end(&mut err).unwrap();
    let stderr = String::from_utf8_lossy(&err);
    assert_eq!(status.code(), Some(2), "stderr: {stderr}");
    assert!(
        out.is_empty(),
        "stdout must stay empty, got: {}",
        String::from_utf8_lossy(&out)
    );
    assert!(
        stderr.contains("unrecognized command 'nosuchcmd'"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("zen -p \"nosuchcmd\""),
        "stderr must teach the -p fix: {stderr}"
    );
}

#[test]
fn help_subcommand_is_a_real_command_not_blocked() {
    let home = TempDir::new("uc_home");
    let cwd = TempDir::new("uc_cwd");
    // help is clap built-in subcommand: it must print help with exit 0,
    // never trip the unknown-command guard.
    let (stdout, stderr, code) = ZenRun::new("uc2", &home, &cwd).arg("help").run_output();
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("Usage: zen"), "stdout: {stdout}");
    assert!(!stderr.contains("unrecognized command"), "stderr: {stderr}");
}

#[test]
fn real_subcommand_doctor_is_not_blocked() {
    let home = TempDir::new("uc_home");
    let cwd = TempDir::new("uc_cwd");
    let (stdout, stderr, code) = ZenRun::new("uc3", &home, &cwd).arg("doctor").run_output();
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("zen doctor"), "stdout: {stdout}");
}

#[test]
fn piped_script_into_repl_still_exits_zero() {
    let home = TempDir::new("uc_home");
    let cwd = TempDir::new("uc_cwd");
    // The supported scripted-pipe mode the builtins.rs tests protect:
    // piping lines into a bare REPL runs them until /exit (or EOF).
    let (stdout, _stderr, code) = ZenRun::new("uc4", &home, &cwd)
        .stdin("/exit\n")
        .run_output();
    assert_eq!(code, 0);
    assert!(!stdout.is_empty(), "REPL banner should print");
}

#[test]
fn quiet_suppresses_doctor_json_output() {
    let home = TempDir::new("uc_home");
    let cwd = TempDir::new("uc_cwd");
    let (stdout, stderr, code) = ZenRun::new("uc5", &home, &cwd)
        .args(["--quiet", "--output-format", "json", "doctor"])
        .run_output();
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(
        stdout.is_empty(),
        "--quiet must leave stdout at 0 bytes in JSON mode too, got: {stdout}"
    );
}

#[test]
fn doctor_json_flag_after_subcommand_emits_json() {
    let home = TempDir::new("uc_home");
    let cwd = TempDir::new("uc_cwd");
    let (stdout, stderr, code) = ZenRun::new("uc6", &home, &cwd)
        .args(["doctor", "--json"])
        .run_output();
    assert_eq!(code, 0, "stderr: {stderr}");
    let v: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("zen doctor --json must emit valid JSON");
    assert_eq!(v["type"], "doctor");
}
