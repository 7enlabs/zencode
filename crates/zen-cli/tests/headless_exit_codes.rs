#![cfg(unix)]

//! Headless (`-p`) exit codes for provider failures (issue #165).
//!
//! The agent reports a provider failure as a `result` event with
//! `subtype: "error_during_execution"` and text `HTTP <status>: ...`
//! instead of propagating an error, so the exit code has to come
//! from the HTTP status in the result text: 401 auth (2), 429 rate
//! limit (6), 500 general (1). Before the fix every one of these
//! exited 1.

mod common;

use common::*;
use std::time::Duration;

const SESSION: &str = "00000000-0000-4000-8000-0000000000a1";

async fn run_headless(replies: Vec<Reply>, timeout: Duration) -> (String, String, i32) {
    let home = TempDir::new("exit_codes_home");
    let cwd = TempDir::new("exit_codes_cwd");
    let server = MockServer::start(replies).await;
    ZenRun::new("exit_codes", &home, &cwd)
        .args([
            "-p",
            "--output-format",
            "stream-json",
            "--session-id",
            SESSION,
            "--bare",
            "--setting-sources",
            "",
        ])
        .mock("mock-model", &server)
        .timeout(timeout)
        .arg("hello")
        .run_output()
}

fn assert_error_result(stdout: &str, status: u16) {
    let result = last_result(stdout);
    assert_eq!(result["subtype"], "error_during_execution", "{stdout}");
    assert_eq!(result["is_error"], true, "{stdout}");
    assert!(
        result["result"]
            .as_str()
            .unwrap_or_default()
            .contains(&format!("HTTP {status}")),
        "result text carries no HTTP {status}: {stdout}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_exit_code_auth_401() {
    let (stdout, _stderr, code) =
        run_headless(vec![Reply::status(401, "bad key")], TEST_TIMEOUT).await;
    assert_error_result(&stdout, 401);
    assert_eq!(code, 2, "401 must exit AUTH (2):\n{stdout}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_exit_code_rate_limit_429() {
    // 429 retries to the attempt budget (~30s); give the run room.
    let (stdout, _stderr, code) = run_headless(
        vec![Reply::status(429, "rate_limit_exceeded")],
        Duration::from_secs(120),
    )
    .await;
    assert_error_result(&stdout, 429);
    assert_eq!(code, 6, "429 must exit RATE_LIMIT (6):\n{stdout}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_exit_code_general_500() {
    // 500 retries to the attempt budget (~30s); give the run room.
    let (stdout, _stderr, code) = run_headless(
        vec![Reply::status(500, "upstream overloaded")],
        Duration::from_secs(120),
    )
    .await;
    assert_error_result(&stdout, 500);
    assert_eq!(code, 1, "500 must stay GENERAL (1):\n{stdout}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_exit_code_success_200() {
    let (stdout, _stderr, code) =
        run_headless(vec![Reply::text("hello from mock")], TEST_TIMEOUT).await;
    let result = last_result(&stdout);
    assert_eq!(result["subtype"], "success", "{stdout}");
    assert_eq!(result["is_error"], false, "{stdout}");
    assert_eq!(code, 0, "success must exit 0:\n{stdout}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_denied_edit_exits_1_with_hint() {
    // Core-perfect phase 2 (A6): in Default mode a mutating tool call
    // has no prompter to ask, so it is denied - and the run must not
    // exit 0 as if the requested work had happened.
    let home = TempDir::new("exit_codes_home");
    let cwd = TempDir::new("exit_codes_cwd");
    let write_call = Reply::with_tool_calls(
        "",
        Some(serde_json::json!([{
            "id": "call_1",
            "type": "function",
            "function": {"name": "Write", "arguments": "{\"file_path\":\"out.txt\",\"content\":\"hi\"}"}
        }])),
    );
    let server = MockServer::start(vec![write_call, Reply::text("done")]).await;
    let (_stdout, stderr, code) = ZenRun::new("exit_codes", &home, &cwd)
        .args([
            "-p",
            "--output-format",
            "stream-json",
            "--session-id",
            SESSION,
            "--bare",
            "--setting-sources",
            "",
        ])
        .mock("mock-model", &server)
        .timeout(TEST_TIMEOUT)
        .arg("write a file")
        .run_output();
    assert_eq!(
        code, 1,
        "denied edit in headless mode must not exit 0; stderr: {stderr}"
    );
    assert!(
        !cwd.path().join("out.txt").exists(),
        "denied write must not create the file"
    );
    assert!(
        stderr.contains("bypassPermissions"),
        "hint must name the flag that allows it; stderr: {stderr}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_bypass_permissions_allows_edit() {
    // Core-perfect phase 2 (A6): the same write with
    // --permission-mode bypassPermissions runs and exits 0.
    let home = TempDir::new("exit_codes_home");
    let cwd = TempDir::new("exit_codes_cwd");
    let write_call = Reply::with_tool_calls(
        "",
        Some(serde_json::json!([{
            "id": "call_1",
            "type": "function",
            "function": {"name": "Write", "arguments": "{\"file_path\":\"out.txt\",\"content\":\"hi\"}"}
        }])),
    );
    let server = MockServer::start(vec![write_call, Reply::text("done")]).await;
    let (_stdout, stderr, code) = ZenRun::new("exit_codes", &home, &cwd)
        .args([
            "-p",
            "--output-format",
            "stream-json",
            "--session-id",
            SESSION,
            "--bare",
            "--setting-sources",
            "",
            "--permission-mode",
            "bypassPermissions",
        ])
        .mock("mock-model", &server)
        .timeout(TEST_TIMEOUT)
        .arg("write a file")
        .run_output();
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(
        std::fs::read_to_string(cwd.path().join("out.txt")).unwrap_or_default(),
        "hi",
        "stderr: {stderr}"
    );
}
