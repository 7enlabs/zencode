//! Detect when the agent is stuck in a loop and stop the run with `loop_stuck`.
//!
//! Four triggers, all driven by the same threshold `K` (configurable, default
//! [`DEFAULT_LOOP_STUCK_K`]; `K = 0` disables the guard):
//!
//! * **Repeated call** - the same tool call (same tool name and identical
//!   JSON arguments; the per-turn call id is ignored) is observed `K` times
//!   in a row, regardless of whether each call succeeded or failed. A failing
//!   loop is still a loop.
//! * **Unchanged turns** - `K` consecutive turns whose working state is
//!   identical: the same ordered list of tool calls with the same result
//!   fingerprints. This catches multi-call turns that repeat as a batch
//!   (e.g. `[Read a, Read b]` three times), which the consecutive-call
//!   counter alone would miss because the calls interleave.
//! * **Repeated rejection** - the same tool call is rejected `K` times in a
//!   row (denied, unknown tool, invalid args, deferred, plan mode, hook
//!   block). Rejected calls never run, so without this trigger a denied
//!   loop would burn turns until `max_turns`.
//! * **Repeated error** - `K` executed calls in a row fail with the same
//!   normalized error, even when the calls themselves differ (e.g. the same
//!   test failure through three different `cargo test` invocations).
//!   Transient errors (timeouts, rate limits) never count: they are
//!   infrastructure noise, not the model being stuck.
//!
//! "Working state" here means exactly what the loop can observe without
//! asking the model: the calls it made and the results it got back. Two
//! turns that call different tools, or the same tools with different results,
//! count as progress and reset the counter, so ordinary exploration (reading
//! a new file each turn) never trips the guard.
//!
//! The guard only records; the agent loop decides when to stop. Call
//! [`LoopGuard::observe_call`] for every executed tool call (in execution
//! order), [`LoopGuard::observe_rejected`] for every call that never ran,
//! and [`LoopGuard::end_turn`] once per turn, after the turn's calls
//! have been flushed. When any trigger fires, [`LoopGuard::take_stuck`]
//! returns the [`LoopStuck`] reason and the loop must stop with subtype
//! `loop_stuck` and the [`LoopStuck::message`] text, which names the reason
//! and tells the user to rephrase the prompt or narrow the scope.

use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use zen_core::ToolOutput;

/// Default value of `K`: how many repeats, or identical turns, trip the guard.
pub const DEFAULT_LOOP_STUCK_K: usize = 3;

/// User-facing suggestion attached to every `loop_stuck` stop.
pub const LOOP_STUCK_SUGGESTION: &str = "Suggestion: rephrase the prompt, narrow the scope, or break the task into a smaller step instead of repeating the same call.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopStuckKind {
    /// The same tool call was observed `K` times in a row.
    RepeatedCall,
    /// `K` consecutive turns had identical calls and results.
    UnchangedTurns,
    /// The same tool call was rejected `K` times in a row.
    RepeatedRejection,
    /// `K` executed calls in a row failed with the same normalized error.
    RepeatedError,
}

/// Reason classes for calls that never ran, as fed to
/// [`LoopGuard::observe_rejected`]. Kept coarse on purpose: the full
/// denial text varies per call, the class is what repeats.
pub mod rejection_reason {
    pub const DENIED: &str = "denied";
    pub const UNKNOWN_TOOL: &str = "unknown_tool";
    pub const INVALID_ARGS: &str = "invalid_args";
    pub const DEFERRED: &str = "deferred";
    pub const PLAN_MODE: &str = "plan_mode";
    pub const HOOK_BLOCKED: &str = "hook_blocked";
}

/// Why the loop was stopped. Render with [`LoopStuck::message`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopStuck {
    pub kind: LoopStuckKind,
    pub detail: String,
}

impl LoopStuck {
    /// Full stop text: always starts with `loop_stuck` and ends with the
    /// user-facing suggestion.
    pub fn message(&self) -> String {
        let what = match self.kind {
            LoopStuckKind::RepeatedCall => "the same tool call was repeated",
            LoopStuckKind::UnchangedTurns => "the working state did not change across turns",
            LoopStuckKind::RepeatedRejection => "the same tool call was rejected",
            LoopStuckKind::RepeatedError => "the same error came back",
        };
        format!(
            "loop_stuck: stopped because {what} ({}). {LOOP_STUCK_SUGGESTION}",
            self.detail
        )
    }
}

#[derive(Debug, Default)]
pub struct LoopGuard {
    k: usize,
    last_call: Option<String>,
    repeat: usize,
    turn_calls: Vec<String>,
    prev_turn: Option<String>,
    unchanged: usize,
    stuck: Option<LoopStuck>,
    last_rejected: Option<String>,
    rejected: usize,
    last_error: Option<u64>,
    last_error_text: String,
    error_repeat: usize,
}

impl LoopGuard {
    pub fn new(k: usize) -> Self {
        Self {
            k,
            ..Default::default()
        }
    }

    fn enabled(&self) -> bool {
        self.k > 0
    }

    fn call_signature(tool: &str, args: &Value) -> String {
        format!("{tool}\u{0}{args}")
    }

    fn result_fingerprint(content: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        hasher.finish()
    }

    /// Observe one executed tool call, in execution order. Fires the
    /// repeated-call trigger when the same signature reaches `K` in a row,
    /// and the repeated-error trigger when `K` calls in a row fail with the
    /// same normalized error. Skipped or denied calls must not be fed here:
    /// only calls that ran (see [`LoopGuard::observe_rejected`]).
    pub fn observe_call(&mut self, tool: &str, args: &Value, out: &ToolOutput) {
        if !self.enabled() || self.stuck.is_some() {
            return;
        }
        // A call that ran breaks any rejection streak: the rejections are
        // no longer "in a row".
        self.last_rejected = None;
        self.rejected = 0;
        let sig = Self::call_signature(tool, args);
        if self.last_call.as_deref() == Some(sig.as_str()) {
            self.repeat += 1;
        } else {
            self.last_call = Some(sig.clone());
            self.repeat = 1;
        }
        self.turn_calls.push(format!(
            "{sig}\u{0}{:x}",
            Self::result_fingerprint(&out.content)
        ));
        if self.repeat >= self.k {
            self.stuck = Some(LoopStuck {
                kind: LoopStuckKind::RepeatedCall,
                detail: format!(
                    "`{tool}` with identical arguments {} times in a row",
                    self.repeat
                ),
            });
            return;
        }
        self.observe_error(out);
    }

    /// Observe a tool call that never ran (denied, unknown tool, invalid
    /// args, deferred, plan mode, hook block). Fires the
    /// repeated-rejection trigger when the same (tool, args, reason) is
    /// rejected `K` times in a row. Calls skipped because the run is
    /// already stopping must NOT be fed here: stopping is the consequence
    /// of a trip, not a loop.
    pub fn observe_rejected(&mut self, tool: &str, args: &Value, reason_class: &str) {
        if !self.enabled() || self.stuck.is_some() {
            return;
        }
        // A rejection breaks the executed-call streaks, and a run breaks
        // the rejection streak: the counters never accumulate across each
        // other, so an interleaved deny/run sequence cannot trip either.
        self.last_call = None;
        self.repeat = 0;
        self.error_repeat = 0;
        self.last_error = None;
        self.last_error_text.clear();
        let key = format!("{tool}\u{0}{args}\u{0}{reason_class}");
        if self.last_rejected.as_deref() == Some(key.as_str()) {
            self.rejected += 1;
        } else {
            self.last_rejected = Some(key);
            self.rejected = 1;
        }
        if self.rejected >= self.k {
            self.stuck = Some(LoopStuck {
                kind: LoopStuckKind::RepeatedRejection,
                detail: format!("`{tool}` ({reason_class}) {} times in a row", self.rejected),
            });
        }
    }

    /// Observe the error half of an executed call: count consecutive
    /// calls that failed with the same normalized error. Success, a
    /// different error, or a transient error resets the counter.
    fn observe_error(&mut self, out: &ToolOutput) {
        if !out.is_error || crate::retry::is_transient_error(&out.content) {
            // Success is progress; a transient error is infrastructure
            // noise, not the model being stuck. Neither counts.
            self.error_repeat = 0;
            self.last_error = None;
            self.last_error_text.clear();
            return;
        }
        let Some((fp, text)) = Self::error_fingerprint(&out.content) else {
            return;
        };
        if self.last_error == Some(fp) {
            self.error_repeat += 1;
        } else {
            self.last_error = Some(fp);
            self.last_error_text = text;
            self.error_repeat = 1;
        }
        if self.error_repeat >= self.k {
            self.stuck = Some(LoopStuck {
                kind: LoopStuckKind::RepeatedError,
                detail: format!(
                    "{} times in a row ({})",
                    self.error_repeat, self.last_error_text
                ),
            });
        }
    }

    /// Whether a line is only the generic BashTool failure prefix
    /// `Exit code N` (case-insensitive, surrounding whitespace ignored).
    fn is_exit_code_line(line: &str) -> bool {
        let lower = line.trim().to_ascii_lowercase();
        let Some(code) = lower.strip_prefix("exit code ") else {
            return false;
        };
        !code.is_empty() && code.chars().all(|c| c.is_ascii_digit())
    }

    /// Normalize an error's first distinguishing line into a comparable
    /// fingerprint: lowercased, absolute paths replaced with `<path>`,
    /// digit runs collapsed to `#`, whitespace collapsed. Only the first
    /// distinguishing line (capped at 500 chars) is used, so a timestamp
    /// buried at the end of a long log cannot make every error look
    /// different. A leading generic `Exit code N` line (the BashTool
    /// failure prefix) is skipped in favour of the next non-empty line,
    /// so unrelated Bash failures do not all fingerprint alike; when
    /// there is no body line, the prefix itself is the fingerprint.
    /// Returns the hash plus a short excerpt for the stop message, or
    /// `None` when nothing comparable remains.
    fn error_fingerprint(content: &str) -> Option<(u64, String)> {
        let mut lines = content.lines().filter(|l| !l.trim().is_empty());
        let first = lines.next()?;
        let line = if Self::is_exit_code_line(first) {
            lines.next().unwrap_or(first)
        } else {
            first
        };
        let capped: String = line.chars().take(500).collect();
        let mut normalized = String::with_capacity(capped.len());
        for token in capped.split_whitespace() {
            if !normalized.is_empty() {
                normalized.push(' ');
            }
            if token.starts_with('/') || token.contains(":\\") || token.contains(":/") {
                normalized.push_str("<path>");
                continue;
            }
            let mut in_digits = false;
            for ch in token.chars() {
                if ch.is_ascii_digit() {
                    if !in_digits {
                        normalized.push('#');
                        in_digits = true;
                    }
                } else {
                    normalized.push(ch.to_ascii_lowercase());
                    in_digits = false;
                }
            }
        }
        if normalized.is_empty() {
            return None;
        }
        let mut hasher = DefaultHasher::new();
        normalized.hash(&mut hasher);
        let excerpt: String = normalized.chars().take(120).collect();
        Some((hasher.finish(), excerpt))
    }

    /// Call once at the end of each turn, after the turn's calls were
    /// flushed. Returns the unchanged-turns trigger on the `K`th identical
    /// turn in a row, and `None` otherwise. Turns with no executed calls do
    /// not count either way.
    pub fn end_turn(&mut self) -> Option<LoopStuck> {
        if self.stuck.is_some() {
            return self.stuck.clone();
        }
        let turn = self.turn_calls.join("\n");
        self.turn_calls.clear();
        if !self.enabled() || turn.is_empty() {
            return None;
        }
        if self.prev_turn.as_deref() == Some(turn.as_str()) {
            self.unchanged += 1;
            if self.unchanged + 1 >= self.k {
                let stuck = LoopStuck {
                    kind: LoopStuckKind::UnchangedTurns,
                    detail: format!(
                        "{} turns in a row with identical tool calls and results",
                        self.unchanged + 1
                    ),
                };
                self.stuck = Some(stuck.clone());
                return Some(stuck);
            }
        } else {
            self.prev_turn = Some(turn);
            self.unchanged = 0;
        }
        None
    }

    /// Take the latched trigger, if any. The loop must stop when this
    /// returns `Some`; call it before [`LoopGuard::end_turn`] so the more
    /// specific repeated-call reason wins when both fire on one turn.
    pub fn take_stuck(&mut self) -> Option<LoopStuck> {
        self.stuck.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ok(content: &str) -> ToolOutput {
        ToolOutput::ok(content)
    }

    fn err(content: &str) -> ToolOutput {
        ToolOutput::err(content)
    }

    #[test]
    fn rejected_repeats_trip_at_k() {
        let mut g = LoopGuard::new(3);
        let args = json!({"file_path": "a.rs"});
        g.observe_rejected("Edit", &args, rejection_reason::DENIED);
        assert_eq!(g.take_stuck(), None);
        g.observe_rejected("Edit", &args, rejection_reason::DENIED);
        assert_eq!(g.take_stuck(), None);
        g.observe_rejected("Edit", &args, rejection_reason::DENIED);
        let stuck = g.take_stuck().expect("third rejection must trip");
        assert_eq!(stuck.kind, LoopStuckKind::RepeatedRejection);
        assert!(stuck.message().contains("denied"));
        assert!(stuck.message().starts_with("loop_stuck"));
    }

    #[test]
    fn different_rejection_reason_does_not_accumulate() {
        let mut g = LoopGuard::new(3);
        let args = json!({"file_path": "a.rs"});
        g.observe_rejected("Edit", &args, rejection_reason::DENIED);
        g.observe_rejected("Edit", &args, rejection_reason::UNKNOWN_TOOL);
        g.observe_rejected("Edit", &args, rejection_reason::DENIED);
        g.observe_rejected("Edit", &args, rejection_reason::DENIED);
        assert_eq!(g.take_stuck(), None);
        g.observe_rejected("Edit", &args, rejection_reason::DENIED);
        assert_eq!(
            g.take_stuck().map(|s| s.kind),
            Some(LoopStuckKind::RepeatedRejection)
        );
    }

    #[test]
    fn run_between_rejections_resets_streak() {
        let mut g = LoopGuard::new(3);
        let args = json!({"file_path": "a.rs"});
        g.observe_rejected("Edit", &args, rejection_reason::DENIED);
        g.observe_rejected("Edit", &args, rejection_reason::DENIED);
        g.observe_call("Read", &json!({"path": "a.rs"}), &ok("r"));
        g.observe_rejected("Edit", &args, rejection_reason::DENIED);
        g.observe_rejected("Edit", &args, rejection_reason::DENIED);
        assert_eq!(g.take_stuck(), None);
    }

    #[test]
    fn same_error_different_calls_trip() {
        let mut g = LoopGuard::new(3);
        let e = err("error: test `foo` failed");
        g.observe_call("Bash", &json!({"command": "cargo test --workspace"}), &e);
        assert_eq!(g.take_stuck(), None);
        g.observe_call("Bash", &json!({"command": "cargo test -p zen-cli"}), &e);
        assert_eq!(g.take_stuck(), None);
        g.observe_call("Bash", &json!({"command": "cargo test -- --nocapture"}), &e);
        let stuck = g.take_stuck().expect("third identical error must trip");
        assert_eq!(stuck.kind, LoopStuckKind::RepeatedError);
        assert!(stuck.message().contains("same error"));
    }

    #[test]
    fn changing_error_does_not_trip() {
        let mut g = LoopGuard::new(3);
        let errors = [
            "error: borrow of moved value",
            "error: mismatched types",
            "error: cannot find value in this scope",
            "error: expected semicolon",
            "error: type annotations needed",
        ];
        for (i, e) in errors.iter().enumerate() {
            g.observe_call("Bash", &json!({"command": format!("cmd{i}")}), &err(e));
            assert_eq!(g.take_stuck(), None);
        }
    }

    #[test]
    fn transient_error_does_not_trip() {
        let mut g = LoopGuard::new(3);
        for i in 0..5 {
            g.observe_call(
                "Bash",
                &json!({"command": format!("cmd{i}")}),
                &err("rate limit exceeded, try again later"),
            );
            assert_eq!(g.take_stuck(), None);
        }
    }

    #[test]
    fn error_fingerprint_ignores_paths_numbers_and_case() {
        let mut g = LoopGuard::new(3);
        g.observe_call(
            "Bash",
            &json!({"command": "a"}),
            &err("error: assertion failed at /home/u/repo/src/lib.rs:42:10"),
        );
        g.observe_call(
            "Bash",
            &json!({"command": "b"}),
            &err("Error: assertion failed at /tmp/other/src/main.rs:7:3"),
        );
        assert_eq!(g.take_stuck(), None);
        g.observe_call(
            "Bash",
            &json!({"command": "c"}),
            &err("error: assertion failed at /home/u/repo/src/lib.rs:99:1"),
        );
        let stuck = g.take_stuck().expect("normalized errors must trip");
        assert_eq!(stuck.kind, LoopStuckKind::RepeatedError);
        assert!(
            stuck
                .message()
                .contains("error: assertion failed at <path>")
        );
    }

    #[test]
    fn success_resets_error_streak() {
        let mut g = LoopGuard::new(3);
        let e = err("error: boom");
        g.observe_call("Bash", &json!({"command": "a"}), &e);
        g.observe_call("Bash", &json!({"command": "b"}), &e);
        g.observe_call("Bash", &json!({"command": "ok"}), &ok("fine"));
        g.observe_call("Bash", &json!({"command": "c"}), &e);
        g.observe_call("Bash", &json!({"command": "d"}), &e);
        assert_eq!(g.take_stuck(), None);
    }

    #[test]
    fn default_k_is_three() {
        assert_eq!(DEFAULT_LOOP_STUCK_K, 3);
    }

    #[test]
    fn fingerprint_normalizes_paths_and_digit_runs() {
        // Windows-style paths (C:/... and C:\...) normalize like unix
        // paths, and digit runs of different lengths normalize to the
        // same '#': the same error through different paths and exit
        // codes is still the same error.
        let a = LoopGuard::error_fingerprint("cannot open C:/Users/a/file.txt: exit 1").unwrap();
        let b = LoopGuard::error_fingerprint("cannot open C:/Users/b/other.txt: exit 101").unwrap();
        let c = LoopGuard::error_fingerprint(r"cannot open D:\data\x.txt: exit 7").unwrap();
        assert_eq!(a.1, "cannot open <path> exit #");
        assert_eq!(a, b);
        assert_eq!(a, c);
    }

    #[test]
    fn different_bash_exit_code_failures_do_not_trip() {
        // B1: every failed Bash call starts with the generic `Exit code N`
        // line, so a fingerprint taken from that line alone counts
        // unrelated failures as the same error. The fingerprint must
        // look past the prefix at the body.
        let mut g = LoopGuard::new(3);
        let failures = [
            "Exit code 1",
            "Exit code 2\nls: cannot access '/nonexistent-dir': No such file or directory",
            "Exit code 101\ntest result: FAILED. 3 failed",
        ];
        for (i, e) in failures.iter().enumerate() {
            g.observe_call("Bash", &json!({"command": format!("cmd{i}")}), &err(e));
            assert_eq!(g.take_stuck(), None);
        }
    }

    #[test]
    fn same_bash_failure_body_trips() {
        // The same Bash failure, body included, three times still trips,
        // and the stop message names the body, not the generic prefix.
        let mut g = LoopGuard::new(3);
        let e = err("Exit code 1\nerror: assertion failed at src/lib.rs:42");
        for i in 0..3 {
            g.observe_call("Bash", &json!({"command": format!("cmd{i}")}), &e);
            if i < 2 {
                assert_eq!(g.take_stuck(), None);
            }
        }
        let stuck = g.take_stuck().expect("same Bash failure body must trip");
        assert_eq!(stuck.kind, LoopStuckKind::RepeatedError);
        assert!(stuck.message().contains("error: assertion failed"));
        assert!(
            !stuck.message().contains("exit code #"),
            "stop message must name the body, not the generic prefix: {}",
            stuck.message()
        );
    }

    #[test]
    fn bare_exit_code_failures_with_same_code_still_trip() {
        // No body to distinguish: a bare `Exit code N` line falls back to
        // the prefix itself, so the same bare code three times still trips.
        let mut g = LoopGuard::new(3);
        for i in 0..3 {
            g.observe_call(
                "Bash",
                &json!({"command": format!("cmd{i}")}),
                &err("Exit code 1"),
            );
            if i < 2 {
                assert_eq!(g.take_stuck(), None);
            }
        }
        let stuck = g.take_stuck().expect("same bare exit code must trip");
        assert_eq!(stuck.kind, LoopStuckKind::RepeatedError);
    }

    #[test]
    fn k_zero_disables_both_triggers() {
        let mut g = LoopGuard::new(0);
        let args = json!({"command": "ls"});
        for _ in 0..5 {
            g.observe_rejected("Edit", &args, rejection_reason::DENIED);
            g.observe_call("Bash", &args, &err("error: boom"));
        }
        assert_eq!(g.end_turn(), None);
        assert_eq!(g.take_stuck(), None);
    }

    #[test]
    fn repeated_call_triggers_at_k() {
        let mut g = LoopGuard::new(3);
        let args = json!({"command": "ls"});
        g.observe_call("Bash", &args, &ok("x"));
        assert_eq!(g.take_stuck(), None);
        g.observe_call("Bash", &args, &ok("x"));
        assert_eq!(g.take_stuck(), None);
        g.observe_call("Bash", &args, &ok("x"));
        let stuck = g.take_stuck().expect("third repeat must trip");
        assert_eq!(stuck.kind, LoopStuckKind::RepeatedCall);
        assert!(stuck.message().starts_with("loop_stuck"));
        assert!(stuck.message().contains("narrow the scope"));
    }

    #[test]
    fn different_call_resets_repeat() {
        let mut g = LoopGuard::new(3);
        let a = json!({"command": "ls"});
        let b = json!({"command": "pwd"});
        for args in [&a, &a, &b, &a, &a] {
            g.observe_call("Bash", args, &ok("x"));
        }
        assert_eq!(g.take_stuck(), None);
    }

    #[test]
    fn unchanged_turns_trigger_on_kth_identical_turn() {
        let mut g = LoopGuard::new(3);
        let turn = |g: &mut LoopGuard| {
            g.observe_call("Read", &json!({"path": "a.rs"}), &ok("r"));
            g.observe_call("Read", &json!({"path": "b.rs"}), &ok("r"));
            g.end_turn()
        };
        assert_eq!(turn(&mut g), None);
        assert_eq!(turn(&mut g), None);
        let stuck = turn(&mut g).expect("third identical turn must trip");
        assert_eq!(stuck.kind, LoopStuckKind::UnchangedTurns);
    }

    #[test]
    fn changed_turn_resets_unchanged_counter() {
        let mut g = LoopGuard::new(3);
        let run = |g: &mut LoopGuard, path: &str| {
            g.observe_call("Read", &json!({"path": path}), &ok("r"));
            g.end_turn()
        };
        assert_eq!(run(&mut g, "a.rs"), None);
        assert_eq!(run(&mut g, "a.rs"), None);
        assert_eq!(run(&mut g, "b.rs"), None);
        assert_eq!(run(&mut g, "a.rs"), None);
        assert_eq!(g.take_stuck(), None);
    }

    #[test]
    fn k_zero_disables_guard() {
        let mut g = LoopGuard::new(0);
        let args = json!({"command": "ls"});
        for _ in 0..5 {
            g.observe_call("Bash", &args, &ok("x"));
        }
        assert_eq!(g.end_turn(), None);
        assert_eq!(g.take_stuck(), None);
    }

    #[test]
    fn k_is_configurable() {
        let mut g = LoopGuard::new(5);
        let args = json!({"command": "ls"});
        for _ in 0..3 {
            g.observe_call("Bash", &args, &ok("x"));
        }
        assert_eq!(g.take_stuck(), None);
        for _ in 0..2 {
            g.observe_call("Bash", &args, &ok("x"));
        }
        assert_eq!(
            g.take_stuck().map(|s| s.kind),
            Some(LoopStuckKind::RepeatedCall)
        );
    }
}
