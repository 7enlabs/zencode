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
//! - `zen telemetry off` (writes "telemetry": "off" to ~/.zen/settings.json)
//! - DO_NOT_TRACK=1
//! - ZENCODE_TELEMETRY=0
//! - "telemetry": "off" in ~/.zen/settings.json (the file the command writes)
//!
//! The design doc (OPT-OUT, §2/§3) names the same opt-out paths.
//! Before `zen telemetry` existed, opting out through settings meant
//! hand-creating the file; the subcommand writes it instead, so the
//! settings path no longer needs a hand-made file.
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
pub const FIRST_RUN_NOTICE: &str = "ZenCode counts anonymous usage (opt out: zen telemetry off, DO_NOT_TRACK=1, or ZENCODE_TELEMETRY=0).";

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

/// Where the effective telemetry choice comes from, for
/// `zen telemetry status`. Environment wins over the settings file,
/// matching `opted_out`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryState {
    /// DO_NOT_TRACK=1 is set.
    OptedOutDoNotTrack,
    /// ZENCODE_TELEMETRY=0 is set.
    OptedOutEnv,
    /// The merged settings say "telemetry": "off".
    OptedOutSetting,
    /// Counting is on (the default).
    On,
}

impl TelemetryState {
    /// Human-readable source for the status line.
    pub fn source(self) -> &'static str {
        match self {
            TelemetryState::OptedOutDoNotTrack => "DO_NOT_TRACK=1",
            TelemetryState::OptedOutEnv => "ZENCODE_TELEMETRY=0",
            TelemetryState::OptedOutSetting => "\"telemetry\": \"off\" in settings",
            TelemetryState::On => "default",
        }
    }
}

/// Human-readable one-line summary of the effective telemetry state.
pub fn status_message(state: TelemetryState) -> String {
    match state {
        TelemetryState::On => "Telemetry: on (default)".to_string(),
        _ => format!("Telemetry: off (opted out via {})", state.source()),
    }
}

/// Effective telemetry state from the merged settings value and the
/// environment. `setting` is the merged settings value of the
/// "telemetry" key; `get_env` reads environment variables (injected
/// so tests do not mutate the process env).
pub fn effective_state(
    setting: Option<&str>,
    get_env: impl Fn(&str) -> Option<String>,
) -> TelemetryState {
    if get_env("DO_NOT_TRACK").as_deref() == Some("1") {
        return TelemetryState::OptedOutDoNotTrack;
    }
    if get_env("ZENCODE_TELEMETRY").as_deref() == Some("0") {
        return TelemetryState::OptedOutEnv;
    }
    if setting.is_some_and(|s| s.trim().eq_ignore_ascii_case("off")) {
        return TelemetryState::OptedOutSetting;
    }
    TelemetryState::On
}

/// True when any opt-out path is active. `setting` is the merged
/// settings value of the "telemetry" key; `get_env` reads environment
/// variables (injected so tests do not mutate the process env).
pub fn opted_out(setting: Option<&str>, get_env: impl Fn(&str) -> Option<String>) -> bool {
    effective_state(setting, get_env) != TelemetryState::On
}

/// The user-level settings file the CLI itself manages for the
/// telemetry opt-out (`zen telemetry off` / `on`). This is the "user"
/// layer `zen_ext::load_settings_trust` reads.
pub fn user_settings_path(home: &Path) -> PathBuf {
    home.join(".zen").join("settings.json")
}

/// Persist a telemetry choice ("off" or "on") in the user settings
/// file, creating it (and `~/.zen/`) when missing. Other keys are
/// preserved. An existing file that is not a JSON object is left
/// untouched and reported as an error instead of being clobbered.
/// A file that already holds the requested value is left untouched.
pub fn set_telemetry(home: &Path, value: &str) -> Result<PathBuf, String> {
    let path = user_settings_path(home);
    let mut map: serde_json::Map<String, serde_json::Value> = match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(serde_json::Value::Object(map)) => map,
            Ok(_) => {
                return Err(format!(
                    "settings file {} is not a JSON object; refusing to overwrite it",
                    path.display()
                ));
            }
            Err(e) => {
                return Err(format!(
                    "settings file {} is not valid JSON ({}); refusing to overwrite it",
                    path.display(),
                    e
                ));
            }
        },
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                serde_json::Map::new()
            } else {
                return Err(format!("cannot read settings file {}: {e}", path.display()));
            }
        }
    };
    if map
        .get("telemetry")
        .and_then(|v| v.as_str())
        .is_some_and(|v| v == value)
    {
        return Ok(path);
    }
    map.insert(
        "telemetry".to_string(),
        serde_json::Value::String(value.to_string()),
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create settings directory {}: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(&serde_json::Value::Object(map))
        .map_err(|e| format!("cannot encode settings file {}: {e}", path.display()))?;
    std::fs::write(&path, format!("{text}\n"))
        .map_err(|e| format!("cannot write settings file {}: {e}", path.display()))?;
    Ok(path)
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

    fn fake_home() -> PathBuf {
        std::env::temp_dir().join(format!("zen-telemetry-test-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn set_telemetry_creates_file_and_dir_when_missing() {
        let home = fake_home();
        let path = set_telemetry(&home, "off").expect("writes the setting");
        assert_eq!(path, home.join(".zen").join("settings.json"));
        let text = std::fs::read_to_string(&path).expect("file exists");
        let doc: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(doc["telemetry"], "off");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn set_telemetry_preserves_other_keys() {
        let home = fake_home();
        let dir = home.join(".zen");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{"model": "custom-model", "telemetry": "on"}"#,
        )
        .unwrap();
        set_telemetry(&home, "off").expect("writes the setting");
        let text = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(doc["telemetry"], "off");
        assert_eq!(doc["model"], "custom-model");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn set_telemetry_leaves_invalid_json_untouched() {
        let home = fake_home();
        let dir = home.join(".zen");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, "{not json").unwrap();
        assert!(set_telemetry(&home, "off").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn set_telemetry_rejects_non_object_json() {
        let home = fake_home();
        let dir = home.join(".zen");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, "[1, 2]").unwrap();
        assert!(set_telemetry(&home, "off").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[1, 2]");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn set_telemetry_noop_when_already_set() {
        let home = fake_home();
        let dir = home.join(".zen");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, "{\"telemetry\": \"off\"}\n").unwrap();
        set_telemetry(&home, "off").expect("already off is fine");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\"telemetry\": \"off\"}\n"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn effective_state_reports_each_source() {
        let env = |k: &str| (k == "DO_NOT_TRACK").then(|| "1".to_string());
        assert_eq!(
            effective_state(Some("on"), env),
            TelemetryState::OptedOutDoNotTrack
        );
        let env = |k: &str| (k == "ZENCODE_TELEMETRY").then(|| "0".to_string());
        assert_eq!(
            effective_state(Some("on"), env),
            TelemetryState::OptedOutEnv
        );
        assert_eq!(
            effective_state(Some("off"), no_env),
            TelemetryState::OptedOutSetting
        );
        assert_eq!(effective_state(Some("on"), no_env), TelemetryState::On);
        assert_eq!(effective_state(None, no_env), TelemetryState::On);
    }

    #[test]
    fn status_message_names_each_state() {
        assert_eq!(
            status_message(TelemetryState::On),
            "Telemetry: on (default)"
        );
        assert_eq!(
            status_message(TelemetryState::OptedOutDoNotTrack),
            "Telemetry: off (opted out via DO_NOT_TRACK=1)"
        );
        assert_eq!(
            status_message(TelemetryState::OptedOutEnv),
            "Telemetry: off (opted out via ZENCODE_TELEMETRY=0)"
        );
        assert_eq!(
            status_message(TelemetryState::OptedOutSetting),
            "Telemetry: off (opted out via \"telemetry\": \"off\" in settings)"
        );
    }

    #[test]
    fn set_telemetry_rejects_unreadable_existing_path() {
        let home = fake_home();
        let dir = home.join(".zen");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::create_dir(&path).unwrap();
        let result = set_telemetry(&home, "off");
        assert!(result.is_err(), "expected error, got {result:?}");
        assert!(path.is_dir(), "must not replace the directory");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn first_run_notice_names_telemetry_command() {
        assert!(
            FIRST_RUN_NOTICE.contains("zen telemetry off"),
            "{FIRST_RUN_NOTICE}"
        );
    }
}
