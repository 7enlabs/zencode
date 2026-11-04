//! Live progress for the interactive REPL (G4): one short stderr line per tool call, tool
//! result, API retry and compaction while a turn runs. The final answer is still printed once
//! by the REPL after the turn, so assistant text without tool calls is not echoed here.

use std::io::Write;
use zen_core::{AgentEvent, EventSink};

const ARG_MAX: usize = 80;

#[derive(Default)]
pub struct ReplProgressSink;

impl ReplProgressSink {
    pub fn new() -> Self {
        Self
    }
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

/// One line describing a tool result: ok or error, line count, and the first line.
pub fn tool_result_summary(content: &str, is_error: bool) -> String {
    let lines = content.lines().count();
    let first = content.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let tag = if is_error { "error" } else { "ok" };
    if lines <= 1 {
        format!("  -> {tag}: {}", clip(first, ARG_MAX))
    } else {
        format!("  -> {tag}, {lines} lines: {}", clip(first, ARG_MAX))
    }
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
            if let Some(text) = message.content.as_deref() {
                if !text.trim().is_empty() {
                    out.push(format!("{indent}{}", clip(text, 160)));
                }
            }
            for tc in calls {
                out.push(format!(
                    "{indent}* {}({})",
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
        AgentEvent::SystemMessage { event, text } => vec![format!("[{event} hook] {text}")],
        _ => Vec::new(),
    }
}

impl EventSink for ReplProgressSink {
    fn emit(&self, ev: AgentEvent) {
        let lines = render(&ev);
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
        assert_eq!(tool_result_summary("done", false), "  -> ok: done");
        assert_eq!(
            tool_result_summary("\nboom\nmore", true),
            "  -> error, 3 lines: boom"
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
    fn text_delta_is_not_echoed() {
        assert!(render(&AgentEvent::TextDelta { text: "x".into() }).is_empty());
    }
}
