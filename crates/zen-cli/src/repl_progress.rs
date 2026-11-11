//! Live progress for the interactive REPL (G4): one short stderr line per tool call, tool
//! result, API retry and compaction while a turn runs. The final answer is still printed once
//! by the REPL after the turn, so assistant text without tool calls is not echoed here.
//!
//! S9 patterns 2 and 3: tool calls print as numbered tree cards whose results
//! stay collapsed to a summary until /expand prints the stored full text,
//! and long-running tools report a friendly still-working line with the real
//! elapsed time from the agent heartbeat.

use std::collections::HashMap;
use std::io::Write;
use std::sync::Mutex;
use zen_core::{AgentEvent, EventSink};

const ARG_MAX: usize = 80;

/// Stored card results are clipped so a long session cannot grow without
/// bound; the clip marker says so instead of faking a complete text.
const CARD_RESULT_MAX: usize = 65_536;

/// One tool call kept after it prints: the header shows the real command,
/// the result stays collapsed to a summary line, and the full result text
/// lives here so /expand can print or re-collapse it on demand.
pub struct ToolCard {
    pub n: usize,
    pub name: String,
    pub arg: String,
    pub result: Option<String>,
    pub is_error: bool,
    pub expanded: bool,
}

#[derive(Default)]
pub struct ReplProgressSink {
    cards: Mutex<Vec<ToolCard>>,
    by_id: Mutex<HashMap<String, usize>>,
}

impl ReplProgressSink {
    pub fn new() -> Self {
        Self::default()
    }

    fn register_card(&self, id: &str, name: &str, arg: String) -> usize {
        let mut cards = self.cards.lock().unwrap();
        let n = cards.len() + 1;
        cards.push(ToolCard {
            n,
            name: name.to_string(),
            arg,
            result: None,
            is_error: false,
            expanded: false,
        });
        self.by_id
            .lock()
            .unwrap()
            .insert(id.to_string(), cards.len() - 1);
        n
    }

    fn attach_result(&self, id: &str, content: &str, is_error: bool) -> Option<usize> {
        let idx = *self.by_id.lock().unwrap().get(id)?;
        let mut cards = self.cards.lock().unwrap();
        let card = cards.get_mut(idx)?;
        card.result = Some(store_result(content));
        card.is_error = is_error;
        Some(card.n)
    }

    /// Toggle one card between its collapsed summary and its full result,
    /// driven by the /expand REPL command. With no number, the most recent
    /// finished card is toggled. Returns the text the REPL prints.
    pub fn toggle_expand(&self, n: Option<usize>) -> String {
        let mut cards = self.cards.lock().unwrap();
        let idx = match n {
            Some(n) => cards.iter().position(|c| c.n == n),
            None => cards.iter().rposition(|c| c.result.is_some()),
        };
        let Some(idx) = idx else {
            return match n {
                Some(n) => {
                    format!("No tool card {n} yet - cards are numbered as tool calls run.")
                }
                None => "No tool results to expand yet - run a prompt first.".to_string(),
            };
        };
        let card = &mut cards[idx];
        let Some(result) = card.result.clone() else {
            return format!("Card {} ({}) has no result yet.", card.n, card.name);
        };
        card.expanded = !card.expanded;
        if card.expanded {
            format!(
                "{}. {}({}) - full output:\n{}",
                card.n, card.name, card.arg, result
            )
        } else {
            format!(
                "{}. {} - {}",
                card.n,
                card.name,
                tool_result_summary(&result, card.is_error).trim_start()
            )
        }
    }

    /// Tree-card lines for the events that carry tool calls and results.
    fn card_lines(&self, ev: &AgentEvent) -> Vec<String> {
        match ev {
            AgentEvent::Assistant {
                message,
                parent_tool_use_id,
            } => {
                let Some(calls) = message.tool_calls.as_ref().filter(|c| !c.is_empty()) else {
                    return Vec::new();
                };
                let indent = if parent_tool_use_id.is_some() {
                    "  "
                } else {
                    ""
                };
                let mut out = Vec::new();
                if let Some(text) = message.content.as_deref()
                    && !text.trim().is_empty()
                {
                    // Same ● marker as the final answer (print_turn).
                    out.push(format!("{indent}● {}", clip(text, 160)));
                }
                for tc in calls {
                    let arg = tool_arg_summary(&tc.function.arguments);
                    let n = self.register_card(&tc.id, &tc.function.name, arg.clone());
                    out.push(format!("{indent}● {n}. {}({arg})", tc.function.name));
                }
                out
            }
            AgentEvent::ToolResult {
                tool_use_id,
                content,
                is_error,
                parent_tool_use_id,
                ..
            } => {
                let indent = if parent_tool_use_id.is_some() {
                    "  "
                } else {
                    ""
                };
                let summary = tool_result_summary(content, *is_error);
                let summary = summary.trim_start();
                match self.attach_result(tool_use_id, content, *is_error) {
                    Some(n) if is_long_result(content) => vec![format!(
                        "{indent}└ {summary} (+{} lines — /expand {n})",
                        content.lines().count()
                    )],
                    _ => vec![format!("{indent}└ {summary}")],
                }
            }
            _ => Vec::new(),
        }
    }
}

/// A result counts as long when collapsing it actually hides something: more
/// than one line, or a single line past the summary clip.
fn is_long_result(content: &str) -> bool {
    content.lines().count() > 1 || content.chars().count() > 160
}

/// Clip a stored card result to a bounded size, keeping line breaks (unlike
/// the summary clip, which flattens text for one-line display).
fn store_result(content: &str) -> String {
    if content.chars().count() <= CARD_RESULT_MAX {
        return content.to_string();
    }
    let mut out: String = content.chars().take(CARD_RESULT_MAX).collect();
    out.push_str("\n... (result clipped for storage)");
    out
}

/// Cut `s` to at most `max` characters on a char boundary, adding `...` when cut.
pub fn clip(s: &str, max: usize) -> String {
    let one_line: String = s
        .chars()
        .map(|c| {
            if c == '\n' || c == '\r' || c == '\t' {
                ' '
            } else {
                c
            }
        })
        .collect();
    let one_line = one_line.trim();
    if one_line.chars().count() <= max {
        one_line.to_string()
    } else {
        let mut out: String = one_line.chars().take(max).collect();
        out.push_str("...");
        out
    }
}

/// The argument a person wants to see for a tool call: the command, path, pattern or url,
/// else the raw JSON, clipped.
pub fn tool_arg_summary(arguments: &str) -> String {
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(arguments)
    {
        for key in [
            "command",
            "file_path",
            "path",
            "pattern",
            "url",
            "query",
            "description",
            "prompt",
        ] {
            if let Some(serde_json::Value::String(v)) = map.get(key) {
                return clip(v, ARG_MAX);
            }
        }
    }
    clip(arguments, ARG_MAX)
}

/// One line describing a tool result: ✓/✗, line count, and the first line.
/// v2: the `-> ok,` log prefix is gone; the closing corner `└` is added
/// by the caller (card_lines), not here.
pub fn tool_result_summary(content: &str, is_error: bool) -> String {
    let lines = content.lines().count();
    let first = content.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let tag = if is_error { "✗" } else { "✓" };
    if lines <= 1 {
        format!("{tag} {}", clip(first, ARG_MAX))
    } else {
        format!("{tag} {lines} lines · {}", clip(first, ARG_MAX))
    }
}

/// Original Zen thinking verbs (clean-room: written from scratch, not
/// copied from any other coding tool). Picked deterministically per tool
/// call so the word does not flicker between heartbeats.
const THINKING_VERBS: &[&str] = &[
    "Thinking",
    "Composing",
    "Weaving",
    "Polishing",
    "Digging",
    "Reading",
    "Writing",
    "Searching",
    "Checking",
    "Zening",
];

fn thinking_verb(tool_use_id: &str) -> &'static str {
    let hash: usize = tool_use_id.bytes().fold(0usize, |acc, b| {
        acc.wrapping_mul(31).wrapping_add(b as usize)
    });
    THINKING_VERBS[hash % THINKING_VERBS.len()]
}

/// The lines a progress event prints, or none.
pub fn render(ev: &AgentEvent) -> Vec<String> {
    match ev {
        AgentEvent::Assistant {
            message,
            parent_tool_use_id,
        } => {
            let Some(calls) = message.tool_calls.as_ref().filter(|c| !c.is_empty()) else {
                return Vec::new();
            };
            let indent = if parent_tool_use_id.is_some() {
                "  "
            } else {
                ""
            };
            let mut out = Vec::new();
            if let Some(text) = message.content.as_deref()
                && !text.trim().is_empty()
            {
                out.push(format!("{indent}● {}", clip(text, 160)));
            }
            for tc in calls {
                out.push(format!(
                    "{indent}● {}({})",
                    tc.function.name,
                    tool_arg_summary(&tc.function.arguments)
                ));
            }
            out
        }
        AgentEvent::ToolResult {
            content,
            is_error,
            parent_tool_use_id,
            ..
        } => {
            let indent = if parent_tool_use_id.is_some() {
                "  "
            } else {
                ""
            };
            vec![format!(
                "{indent}{}",
                tool_result_summary(content, *is_error)
            )]
        }
        AgentEvent::ApiRetry {
            attempt,
            max_retries,
            retry_delay_ms,
            error_status,
            ..
        } => {
            let status = error_status
                .map(|s| format!(" (HTTP {s})"))
                .unwrap_or_default();
            vec![format!(
                "retrying API call {attempt}/{max_retries} in {:.1}s{status}",
                *retry_delay_ms as f64 / 1000.0
            )]
        }
        AgentEvent::CompactBoundary {
            trigger,
            pre_tokens,
        } => {
            vec![format!(
                "context compacted ({trigger}, {pre_tokens} tokens before)"
            )]
        }
        AgentEvent::ToolProgress {
            tool_use_id,
            tool_name,
            elapsed_ms,
            ..
        } => vec![format!(
            "{}… {tool_name} · {:.0}s · Ctrl-C to stop",
            thinking_verb(tool_use_id),
            *elapsed_ms as f64 / 1000.0
        )],
        AgentEvent::SystemMessage { event, text } => vec![format!("[{event} hook] {text}")],
        _ => Vec::new(),
    }
}

impl EventSink for ReplProgressSink {
    fn emit(&self, ev: AgentEvent) {
        let lines = match &ev {
            AgentEvent::Assistant { .. } | AgentEvent::ToolResult { .. } => self.card_lines(&ev),
            _ => render(&ev),
        };
        if lines.is_empty() {
            return;
        }
        let mut err = std::io::stderr().lock();
        for l in lines {
            let _ = writeln!(err, "{l}");
        }
        let _ = err.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_is_char_safe_and_single_line() {
        let s = "sửa tiếng Việt\nrất dài ".repeat(20);
        let c = clip(&s, 10);
        assert!(!c.contains('\n'));
        assert!(c.ends_with("..."));
        assert_eq!(c.chars().count(), 13);
        assert_eq!(clip("ngắn", 10), "ngắn");
    }

    #[test]
    fn arg_summary_prefers_command_and_path() {
        assert_eq!(
            tool_arg_summary(r#"{"command":"cargo test","timeout":5}"#),
            "cargo test"
        );
        assert_eq!(
            tool_arg_summary(r#"{"file_path":"src/a.rs","content":"x"}"#),
            "src/a.rs"
        );
        assert_eq!(tool_arg_summary("not json"), "not json");
    }

    #[test]
    fn result_summary_counts_lines() {
        assert_eq!(tool_result_summary("done", false), "✓ done");
        assert_eq!(
            tool_result_summary("\nboom\nmore", true),
            "✗ 3 lines · boom"
        );
    }

    #[test]
    fn retry_and_compaction_render() {
        let r = render(&AgentEvent::ApiRetry {
            attempt: 2,
            max_retries: 5,
            retry_delay_ms: 1500,
            error_status: Some(429),
            error: "rate".into(),
        });
        assert_eq!(
            r,
            vec!["retrying API call 2/5 in 1.5s (HTTP 429)".to_string()]
        );
        let c = render(&AgentEvent::CompactBoundary {
            trigger: "auto".into(),
            pre_tokens: 1000,
        });
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn tool_progress_renders_friendly_still_working() {
        let r = render(&AgentEvent::ToolProgress {
            tool_use_id: "t1".into(),
            tool_name: "Bash".into(),
            elapsed_ms: 12_400,
        });
        assert_eq!(r.len(), 1);
        assert!(r[0].contains("Bash"), "line: {}", r[0]);
        assert!(r[0].contains("12s"), "line: {}", r[0]);
        assert!(r[0].contains("Ctrl-C to stop"), "line: {}", r[0]);
        assert!(r[0].ends_with("Ctrl-C to stop"), "line: {}", r[0]);
        // No "Still working" sentence anymore; a verb + ellipsis instead.
        assert!(!r[0].contains("Still working"), "line: {}", r[0]);
        assert!(r[0].contains('…'), "line: {}", r[0]);
    }

    #[test]
    fn thinking_verb_is_deterministic() {
        assert_eq!(thinking_verb("t1"), thinking_verb("t1"));
        assert!(THINKING_VERBS.contains(&thinking_verb("anything")));
    }

    #[test]
    fn cards_register_collapse_and_expand() {
        use zen_core::{ChatMessage, FunctionCall, ToolCall};
        let sink = ReplProgressSink::new();
        let msg = ChatMessage::assistant(
            Some("running tests".into()),
            Some(vec![ToolCall {
                id: "c1".into(),
                call_type: "function".into(),
                function: FunctionCall {
                    name: "Bash".into(),
                    arguments: "{\"command\":\"cargo test\"}".into(),
                },
            }]),
        );
        sink.emit(AgentEvent::Assistant {
            message: msg,
            parent_tool_use_id: None,
        });
        let body = "line one\nline two is the secret middle\nline three";
        sink.emit(AgentEvent::ToolResult {
            tool_use_id: "c1".into(),
            content: body.into(),
            is_error: false,
            parent_tool_use_id: None,
            parallel: false,
        });
        let full = sink.toggle_expand(None);
        assert!(full.contains("full output"), "expand: {full}");
        assert!(full.contains("secret middle"), "expand: {full}");
        let collapsed = sink.toggle_expand(Some(1));
        assert!(
            !collapsed.contains("secret middle"),
            "collapse: {collapsed}"
        );
        assert!(collapsed.contains("✓"), "collapse: {collapsed}");
        assert!(sink.toggle_expand(Some(99)).contains("No tool card 99"));
    }

    #[test]
    fn text_delta_is_not_echoed() {
        assert!(render(&AgentEvent::TextDelta { text: "x".into() }).is_empty());
    }
}
