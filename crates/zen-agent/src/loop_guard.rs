//! Detect when the agent is stuck in a loop and stop the run with `loop_stuck`.
//!
//! Two triggers, both driven by the same threshold `K` (configurable, default
//! [`DEFAULT_LOOP_STUCK_K`]; `K = 0` disables the guard):
//!
//! * **Repeated call** — the same tool call (same tool name and identical
//!   JSON arguments; the per-turn call id is ignored) is observed `K` times
//!   in a row, regardless of whether each call succeeded or failed. A failing
//!   loop is still a loop.
//! * **Unchanged turns** — `K` consecutive turns whose working state is
//!   identical: the same ordered list of tool calls with the same result
//!   fingerprints. This catches multi-call turns that repeat as a batch
//!   (e.g. `[Read a, Read b]` three times), which the consecutive-call
//!   counter alone would miss because the calls interleave.
//!
//! "Working state" here means exactly what the loop can observe without
//! asking the model: the calls it made and the results it got back. Two
//! turns that call different tools, or the same tools with different results,
//! count as progress and reset the counter, so ordinary exploration (reading
//! a new file each turn) never trips the guard.
//!
//! The guard only records; the agent loop decides when to stop. Call
//! [`LoopGuard::observe_call`] for every executed tool call (in execution
//! order) and [`LoopGuard::end_turn`] once per turn, after the turn's calls
//! have been flushed. When either trigger fires, [`LoopGuard::take_stuck`]
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
    /// repeated-call trigger when the same signature reaches `K` in a row.
    /// Skipped or denied calls must not be fed here: only calls that ran.
    pub fn observe_call(&mut self, tool: &str, args: &Value, out: &ToolOutput) {
        if !self.enabled() || self.stuck.is_some() {
            return;
        }
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
        }
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
