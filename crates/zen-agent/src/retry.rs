//! Retry around tool execution (B6.1, fixed per PR #30 review).
//!
//! The agent loop retries a failed tool call at most twice, with backoff,
//! only when the tool name is in scope, the error looks transient, and
//! the error is not the agent internal timeout. Scope is Read, or an MCP
//! tool named mcp__..., plus Bash only when the tool reports is_read_only
//! (the real BashTool does not, so a real Bash call is effectively never
//! retried: a mutating shell command must not run twice). Transient
//! matching uses only the FIRST line of the error content, so a transient
//! word buried in command output does not trigger a retry. The agent
//! internal timeout (Error: <tool> timed out after N ms and was stopped)
//! must not triple the user-set wait. Deterministic errors are never
//! retried, and tools outside scope are never retried.
//!
//! MCP tools keep name scope: McpTool hardcodes is_read_only to false,
//! so gating MCP on that trait would silently drop all MCP retries from
//! the approved B6.1 scope; plumbing MCP readOnlyHint is separate work.

/// Maximum number of retries after the initial attempt (B6.1 scope: 2).
pub const MAX_TOOL_RETRIES: u32 = 2;

/// Base backoff in milliseconds; retry n sleeps base times 2 to the n.
pub const TOOL_RETRY_BACKOFF_BASE_MS: u64 = 100;

/// Transient error substrings, matched case-insensitively against the
/// FIRST line of the error content only.
const TRANSIENT_SUBSTRINGS: &[&str] = &[
    "timed out",
    "timeout",
    "connection reset",
    "connection aborted",
    "temporarily unavailable",
    "service unavailable",
    "try again",
    "overloaded",
    "rate limit",
];

/// Whether a tool name is in B6.1 retry scope: Read, or an MCP tool.
/// MCP tools are named mcp__server__tool. Bash is handled by
/// tool_retry_eligible_for, not by this name-only check.
pub fn tool_retry_eligible(tool_name: &str) -> bool {
    tool_name == "Read" || tool_name.starts_with("mcp__")
}

/// Whether a concrete tool call is retry-eligible. Bash is mutating
/// (docs/tools/bash.md), so it retries only if the tool itself reports
/// read-only; the real BashTool reports false, so it never retries.
/// MCP tools keep name scope: McpTool hardcodes is_read_only to false,
/// so gating MCP on that trait would silently drop all MCP retries
/// from the approved B6.1 scope; plumbing MCP readOnlyHint is separate
/// work, not part of this fix.
pub fn tool_retry_eligible_for(tool_name: &str, is_read_only: bool) -> bool {
    if tool_name == "Bash" {
        return is_read_only;
    }
    tool_retry_eligible(tool_name)
}

/// Whether the error content is the agent internal timeout, as
/// produced by run_tool_once: it ends with and was stopped (...).
/// Such a timeout must never be retried: the user already set the wait.
pub fn is_internal_timeout(content: &str) -> bool {
    let first = content.lines().next().unwrap_or("").to_lowercase();
    first.contains("timed out after") && first.contains("and was stopped")
}

/// Whether an error output looks transient: allowlist match,
/// case-insensitive, against the first line only, excluding the
/// agent internal timeout.
pub fn is_transient_error(content: &str) -> bool {
    if is_internal_timeout(content) {
        return false;
    }
    let first = content.lines().next().unwrap_or("").to_lowercase();
    TRANSIENT_SUBSTRINGS.iter().any(|s| first.contains(s))
}

/// Backoff in milliseconds before retry number attempt (0-based: the first
/// retry sleeps base, the second retry sleeps base times 2).
pub fn retry_backoff_ms(attempt: u32) -> u64 {
    TOOL_RETRY_BACKOFF_BASE_MS.saturating_mul(1 << attempt.min(10))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eligible_names() {
        assert!(tool_retry_eligible("Read"));
        assert!(tool_retry_eligible("mcp__github__search"));
        assert!(!tool_retry_eligible("Bash"));
        assert!(!tool_retry_eligible("Write"));
        assert!(!tool_retry_eligible("Edit"));
        assert!(!tool_retry_eligible("Task"));
        assert!(!tool_retry_eligible("mcp"));
    }

    #[test]
    fn eligible_for_bash_gate() {
        assert!(!tool_retry_eligible_for("Bash", false));
        assert!(tool_retry_eligible_for("Bash", true));
        assert!(tool_retry_eligible_for("Read", false));
        assert!(tool_retry_eligible_for("mcp__github__search", false));
        assert!(!tool_retry_eligible_for("Write", true));
    }

    #[test]
    fn transient_classification() {
        assert!(is_transient_error("Error: Bash timed out after 5000 ms"));
        assert!(is_transient_error(
            "MCP call failed: connection reset by peer"
        ));
        assert!(is_transient_error("Service Unavailable, try again later"));
        assert!(!is_transient_error("Error: file not found"));
        assert!(!is_transient_error("Error: permission denied"));
        assert!(!is_transient_error("Error: command not found"));
        assert!(!is_transient_error("ok"));
    }

    #[test]
    fn transient_matches_first_line_only() {
        assert!(!is_transient_error(
            "Error: tests failed\nlog line mentions rate limit in output"
        ));
        assert!(!is_transient_error(
            "Error: command failed\noutput says try again later"
        ));
        assert!(is_transient_error(
            "Error: connection reset by peer\nsome later detail"
        ));
    }

    #[test]
    fn internal_timeout_not_transient() {
        assert!(is_internal_timeout(
            "Error: Read timed out after 5000 ms and was stopped (settings toolTimeouts.default)"
        ));
        assert!(!is_transient_error(
            "Error: Read timed out after 5000 ms and was stopped (settings toolTimeouts.default)"
        ));
    }

    #[test]
    fn backoff_grows() {
        assert_eq!(retry_backoff_ms(0), 100);
        assert_eq!(retry_backoff_ms(1), 200);
    }
}
