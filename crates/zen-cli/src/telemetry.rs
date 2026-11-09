//! Anonymous user counter client (Viec 7, Phase 1).
//!
//! Design: docs/design/user-counter.md (OPT-OUT, approved 2026-10-20).
//! At session startup the CLI may send one anonymous ping per 24 hours
//! to the counter endpoint. The payload is exactly four fields -
//! install_id, version, os, arch. No email, no account, no code
//! content, no file paths. The server stores only a SHA-256 hash of
//! install_id and never stores the client IP (see
//! workers/user-counter/).
//!
//! Opt out at any time through any of:
//! - DO_NOT_TRACK=1
//! - ZENCODE_TELEMETRY=0
//! - "telemetry": "off" in ~/.zen/settings.json
//!
//! The design doc (OPT-OUT, §2/§3) names the same three opt-out
//! paths - DO_NOT_TRACK=1, ZENCODE_TELEMETRY=0, and "telemetry": "off"
//! in ~/.zen/settings.json. This build has no `zen config` subcommand,
//! so the settings file and the env vars are the working opt-out
//! paths and the first-run notice below names them.
//!
//! The ping is fire-and-forget: a 3 second timeout, every failure
//! swallowed, and the state file is updated before the send so a
//! crash cannot make the client ping more often than the interval.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_ENDPOINT: &str = "https://count.runzen.dev";
pub const ENDPOINT_ENV: &str = "ZENCODE_COUNTER_URL";
pub const PING_INTERVAL_SECS: u64 = 24 * 60 * 60;
pub const STATE_FILE_NAME: &str = "user-counter.json";
pub const FIRST_RUN_NOTICE: &str = "ZenCode counts anonymous usage (opt out: DO_NOT_TRACK=1, ZENCODE_TELEMETRY=0, or \"telemetry\": \"off\" in ~/.zen/settings.json).";

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CounterState {
    pub install_id: String,
    #[serde(default)]
    pub last_ping_unix: u64,
    #[serde(default)]
    pub notice_shown: bool,
}

impl CounterState {
    fn fresh() -> Self {
        Self {
            install_id: uuid::Uuid::new_v4().to_string(),
            last_ping_unix: 0,
            notice_shown: false,
        }
    }
}

pub fn state_path(home: &Path) -> PathBuf {
    home.join(".zen").join(STATE_FILE_NAME)
}

/// True when any opt-out path is active. `setting` is the merged
/// settings value of the "telemetry" key; `get_env` reads environment
/// variables (injected so tests do not mutate the process env).
pub fn opted_out(setting: Option<&str>, get_env: impl Fn(&str) -> Option<String>) -> bool {
    if get_env("DO_NOT_TRACK").as_deref() == Some("1") {
        return true;
    }
    if get_env("ZENCODE_TELEMETRY").as_deref() == Some("0") {
        return true;
    }
    setting.is_some_and(|s| s.trim().eq_ignore_ascii_case("off"))
}

pub fn endpoint_from(override_url: Option<String>) -> String {
    match override_url {
        Some(u) if !u.trim().is_empty() => u.trim().trim_end_matches('/').to_string(),
        _ => DEFAULT_ENDPOINT.to_string(),
    }
}

pub fn should_ping(state: &CounterState, now_unix: u64) -> bool {
    now_unix >= state.last_ping_unix.saturating_add(PING_INTERVAL_SECS)
}

pub fn payload(state: &CounterState) -> serde_json::Value {
    serde_json::json!({
        "install_id": state.install_id,
        "version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
    })
}

fn load_state(path: &Path) -> Option<CounterState> {
    let text = std::fs::read_to_string(path).ok()?;
    let state: CounterState = serde_json::from_str(&text).ok()?;
    // A corrupt or hand-edited id is replaced, never trusted.
    uuid::Uuid::parse_str(&state.install_id).ok()?;
    Some(state)
}

fn save_state(path: &Path, state: &CounterState) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string(state) {
        let _ = std::fs::write(path, text);
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Session-start hook: show the first-run notice once, then send at
/// most one anonymous ping per 24 hours. Never fails, never blocks.
pub fn maybe_ping(home: Option<&Path>, telemetry_setting: Option<&str>) {
    if opted_out(telemetry_setting, |k| std::env::var(k).ok()) {
        return;
    }
    let Some(home) = home else { return };
    let path = state_path(home);
    let mut state = load_state(&path).unwrap_or_else(CounterState::fresh);
    let mut dirty = false;
    if !state.notice_shown {
        eprintln!("{FIRST_RUN_NOTICE}");
        state.notice_shown = true;
        dirty = true;
    }
    let now = now_unix();
    if !should_ping(&state, now) {
        if dirty {
            save_state(&path, &state);
        }
        return;
    }
    // Record the attempt before sending: a crash mid-send must not
    // make the next start ping again inside the interval.
    state.last_ping_unix = now;
    save_state(&path, &state);

    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let url = format!("{}/ping", endpoint_from(std::env::var(ENDPOINT_ENV).ok()));
    let body = payload(&state);
    handle.spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(3))
            .build();
        if let Ok(client) = client {
            let _ = client.post(&url).json(&body).send().await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn state_with(last_ping_unix: u64) -> CounterState {
        CounterState {
            install_id: uuid::Uuid::new_v4().to_string(),
            last_ping_unix,
            notice_shown: true,
        }
    }

    #[test]
    fn opt_out_by_do_not_track() {
        let env = |k: &str| (k == "DO_NOT_TRACK").then(|| "1".to_string());
        assert!(opted_out(None, env));
    }

    #[test]
    fn opt_out_by_zencode_telemetry_zero() {
        let env = |k: &str| (k == "ZENCODE_TELEMETRY").then(|| "0".to_string());
        assert!(opted_out(None, env));
    }

    #[test]
    fn opt_out_by_setting_off_case_insensitive() {
        assert!(opted_out(Some("off"), no_env));
        assert!(opted_out(Some(" OFF "), no_env));
    }

    #[test]
    fn not_opted_out_by_default() {
        assert!(!opted_out(None, no_env));
        assert!(!opted_out(Some("on"), no_env));
        let env = |k: &str| (k == "ZENCODE_TELEMETRY").then(|| "1".to_string());
        assert!(!opted_out(None, env));
    }

    #[test]
    fn first_run_always_pings() {
        assert!(should_ping(&state_with(0), 1_700_000_000));
    }

    #[test]
    fn ping_within_interval_is_suppressed() {
        let t = 1_700_000_000;
        assert!(!should_ping(&state_with(t), t + PING_INTERVAL_SECS - 1));
    }

    #[test]
    fn ping_after_interval_is_allowed() {
        let t = 1_700_000_000;
        assert!(should_ping(&state_with(t), t + PING_INTERVAL_SECS));
    }

    #[test]
    fn state_roundtrip_keeps_install_id() {
        let dir = std::env::temp_dir().join(format!("zen-counter-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join(STATE_FILE_NAME);
        let state = CounterState::fresh();
        save_state(&path, &state);
        let loaded = load_state(&path).expect("state reloads");
        assert_eq!(loaded.install_id, state.install_id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_state_is_rejected() {
        let dir = std::env::temp_dir().join(format!("zen-counter-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(STATE_FILE_NAME);
        std::fs::write(&path, r#"{"install_id":"not-a-uuid"}"#).unwrap();
        assert!(load_state(&path).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn payload_has_exactly_the_four_fields() {
        let body = payload(&state_with(0));
        let obj = body.as_object().expect("payload is an object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["arch", "install_id", "os", "version"]);
        assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn endpoint_defaults_and_trims_override() {
        assert_eq!(endpoint_from(None), DEFAULT_ENDPOINT);
        assert_eq!(endpoint_from(Some("  ".into())), DEFAULT_ENDPOINT);
        assert_eq!(
            endpoint_from(Some("https://example.workers.dev/".into())),
            "https://example.workers.dev"
        );
    }
}
