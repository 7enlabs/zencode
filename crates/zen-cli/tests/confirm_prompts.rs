//! Confirm prompts for `/clear` (B6) and `/undo` (B7).
//!
//! Both prompts only ask when stdin is a terminal, so they are exercised
//! under `script(1)`, which gives the child a pty. Piped stdin (the usual
//! e2e harness) takes the old behaviour and never reaches the prompt
//! code, which is why these mutants survived until now.
//!
//! Linux-only: `script -qec` is the util-linux spelling; the mutants job
//! runs on ubuntu-latest, and the Windows test workflow skips this file.
#![cfg(target_os = "linux")]

mod common;

use common::*;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

fn tool_call(id: &str, name: &str, args: serde_json::Value) -> Reply {
    Reply::with_tool_calls(
        "",
        Some(serde_json::json!([{
            "id": id,
            "type": "function",
            "function": {"name": name, "arguments": args.to_string()}
        }])),
    )
}

/// Run the zen binary under `script` so its stdin is a real terminal.
/// Lines are written one at a time with a pause so the REPL consumes them
/// in order; stdin stays open until the child exits (closing it early can
/// drop the pty master before the REPL reads the last lines).
/// Returns (stdout with CRLF normalised, exit code).
fn run_pty(home: &TempDir, cwd: &TempDir, server: &MockServer, lines: &[&str]) -> (String, i32) {
    let cmdline = format!(
        "{} --setting-sources '' --permission-mode bypassPermissions --model mock-model --base-url {}",
        zen_bin().display(),
        server.base_url()
    );
    let mut child = Command::new("script")
        .args(["-qec", &cmdline, "/dev/null"])
        .current_dir(cwd.path())
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("TERM", "xterm")
        .env("ZEN_API_KEY", "dummy")
        .env_remove("ZEN_MODEL")
        .env_remove("ZEN_BASE_URL")
        .env_remove("OPENAI_API_KEY")
        .env_remove("ZEN_SETTINGS")
        .env("ZEN_TRUST_FOLDER", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn script");
    let mut stdin = child.stdin.take().expect("pty stdin");
    let done = Arc::new(AtomicBool::new(false));
    let done_w = Arc::clone(&done);
    let owned: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
    let writer = std::thread::spawn(move || {
        for line in owned {
            if stdin.write_all(line.as_bytes()).is_err() {
                break;
            }
            if stdin.write_all(b"\n").is_err() {
                break;
            }
            let _ = stdin.flush();
            std::thread::sleep(Duration::from_millis(500));
        }
        while !done_w.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(200));
        }
    });
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        match child.try_wait().expect("try_wait") {
            Some(status) => {
                done.store(true, Ordering::SeqCst);
                let _ = writer.join();
                let out = child.wait_with_output().expect("wait_with_output");
                let stdout = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
                return (stdout, status.code().unwrap_or(-1));
            }
            None => {
                if Instant::now() > deadline {
                    let _ = child.kill();
                    done.store(true, Ordering::SeqCst);
                    let _ = writer.join();
                    let out = child.wait_with_output().expect("wait after kill");
                    panic!(
                        "zen under pty exceeded {:?}\nstdout: {}",
                        TEST_TIMEOUT,
                        String::from_utf8_lossy(&out.stdout)
                    );
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clear_asks_and_keeps_on_no() {
    // Kills repl.rs:1258 (delete ! on messages().is_empty()) and :1260
    // (delete ! on prompt_yes_no): with a non-empty conversation and an
    // "n" answer the conversation must be kept, not cleared.
    let home = TempDir::new("b6_home");
    let cwd = TempDir::new("b6_cwd");
    let server = MockServer::start(vec![Reply::text("hi")]).await;
    let (stdout, code) = run_pty(&home, &cwd, &server, &["hello", "/clear", "n", "/exit"]);
    assert_eq!(code, 0, "stdout {stdout}");
    assert!(
        stdout.contains("Clear the conversation and start a new session?"),
        "{stdout}"
    );
    assert!(
        stdout.contains("Kept the current conversation."),
        "{stdout}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn undo_edit_asks_and_keeps_on_no() {
    // Restored-only preview. Kills repl.rs:2362 (delete ! on
    // restored.is_empty()) and :2381 (delete ! on prompt_yes_no).
    let home = TempDir::new("b7_edit_home");
    let cwd = TempDir::new("b7_edit_cwd");
    cwd.write("a.txt", "old line\nkeep\n");
    let server = MockServer::start(vec![
        tool_call("r1", "Read", serde_json::json!({"file_path": "a.txt"})),
        tool_call(
            "e1",
            "Edit",
            serde_json::json!({"file_path": "a.txt", "old_string": "old line", "new_string": "new line"}),
        ),
        Reply::text("edited"),
    ])
    .await;
    let (stdout, code) = run_pty(
        &home,
        &cwd,
        &server,
        &["edit please", "/undo", "n", "/exit"],
    );
    assert_eq!(code, 0, "stdout {stdout}");
    assert!(
        stdout.contains(&format!("restore {}", cwd.path().join("a.txt").display())),
        "{stdout}"
    );
    assert!(stdout.contains("Undo this turn?"), "{stdout}");
    assert!(stdout.contains("Kept the files as they are."), "{stdout}");
    assert!(
        std::fs::read_to_string(cwd.path().join("a.txt"))
            .unwrap()
            .contains("new line"),
        "undo ran despite 'n'"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn undo_create_asks_and_keeps_on_no() {
    // Removed-only preview (the turn only created a file). Kills
    // repl.rs:2363, both the delete-! and the || -> && mutant, and :2381.
    let home = TempDir::new("b7_create_home");
    let cwd = TempDir::new("b7_create_cwd");
    let server = MockServer::start(vec![
        tool_call(
            "w1",
            "Write",
            serde_json::json!({"file_path": "b.txt", "content": "created\n"}),
        ),
        Reply::text("created"),
    ])
    .await;
    let (stdout, code) = run_pty(
        &home,
        &cwd,
        &server,
        &["create please", "/undo", "n", "/exit"],
    );
    assert_eq!(code, 0, "stdout {stdout}");
    assert!(
        stdout.contains(&format!(
            "delete {} (created in that turn)",
            cwd.path().join("b.txt").display()
        )),
        "{stdout}"
    );
    assert!(stdout.contains("Undo this turn?"), "{stdout}");
    assert!(stdout.contains("Kept the files as they are."), "{stdout}");
    assert!(cwd.path().join("b.txt").exists(), "undo ran despite 'n'");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn undo_oversize_file_asks_and_keeps_on_no() {
    // Skipped-only preview: the file is over the 10 MB checkpoint limit,
    // so its entry is Skipped, not Saved. Kills repl.rs:2364, both the
    // delete-! and the || -> && mutant, and :2381.
    let home = TempDir::new("b7_big_home");
    let cwd = TempDir::new("b7_big_cwd");
    let big = format!("BIGFILE-OLD\n{}", "x".repeat(10 * 1024 * 1024));
    std::fs::write(cwd.path().join("big.txt"), big).unwrap();
    let server = MockServer::start(vec![
        tool_call("r1", "Read", serde_json::json!({"file_path": "big.txt"})),
        tool_call(
            "e1",
            "Edit",
            serde_json::json!({"file_path": "big.txt", "old_string": "BIGFILE-OLD", "new_string": "BIGFILE-NEW"}),
        ),
        Reply::text("edited"),
    ])
    .await;
    let (stdout, code) = run_pty(&home, &cwd, &server, &["edit big", "/undo", "n", "/exit"]);
    assert_eq!(code, 0, "stdout {stdout}");
    assert!(
        stdout.contains(&format!(
            "NOT restore {}",
            cwd.path().join("big.txt").display()
        )),
        "{stdout}"
    );
    assert!(stdout.contains("Undo this turn?"), "{stdout}");
    assert!(stdout.contains("Kept the files as they are."), "{stdout}");
    let after = std::fs::read_to_string(cwd.path().join("big.txt")).unwrap();
    assert!(after.starts_with("BIGFILE-NEW"), "undo ran despite 'n'");
}
