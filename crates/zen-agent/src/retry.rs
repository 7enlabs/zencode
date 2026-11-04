//! Retry around tool execution (B6.1).
//!
//! The agent loop retries a failed tool call at most twice, with backoff,
//! only for tools in scope (Bash, Read, and MCP tools named mcp__...), and
//! only for transient errors in the allowlist below. Deterministic errors
//! are never retried, and tools outside scope are never retried.

/// Maximum number of retries after the initial attempt (B6.1 scope: 2).
pub const MAX_TOOL_RETRIES: u32 = 2;

/// Base backoff in milliseconds; retry n sleeps base times 2 to the n.
pub const TOOL_RETRY_BACKOFF_BASE_MS: u64 = 100;

/// Transient error substrings, matched case-insensitively against the
/// error content of a failed tool output.
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

/// Whether a tool name is in B6.1 retry scope: Bash, Read, or an MCP tool.
/// MCP tools are named mcp__server__tool.
pub fn tool_retry_eligible(tool_name: &str) -> bool {
    tool_name == "Bash" || tool_name == "Read" || tool_name.starts_with("mcp__")
}

/// Whether an error output looks transient (allowlist match, case-insensitive).
pub fn is_transient_error(content: &str) -> bool {
    let lower = content.to_lowercase();
    TRANSIENT_SUBSTRINGS.iter().any(|s| lower.contains(s))
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
        assert!(tool_retry_eligible("Bash"));
        assert!(tool_retry_eligible("Read"));
        assert!(tool_retry_eligible("mcp__github__search"));
        assert!(!tool_retry_eligible("Write"));
        assert!(!tool_retry_eligible("Edit"));
        assert!(!tool_retry_eligible("Task"));
        assert!(!tool_retry_eligible("mcp"));
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
    fn backoff_grows() {
        assert_eq!(retry_backoff_ms(0), 100);
        assert_eq!(retry_backoff_ms(1), 200);
    }
}
