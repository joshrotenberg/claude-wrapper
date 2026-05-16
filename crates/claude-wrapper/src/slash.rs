//! Helpers for building Claude Code's slash-command strings.
//!
//! The interactive CLI exposes commands like `/compact`, `/clear`,
//! `/model` that the user types mid-conversation. In stream-json
//! input mode the same `/foo args` strings are passed through as
//! turn prompts -- the CLI interprets them, the model doesn't see
//! them as user prompts.
//!
//! Hosts wrapping a duplex session as a chat (MCP servers, IDE
//! backends, agent runtimes) need to construct these strings
//! safely, especially when forwarding optional user instructions.
//! These helpers keep the CLI's slash syntax in one place rather
//! than having every caller hand-roll `format!("/compact {}", ...)`.
//!
//! # Example
//!
//! ```
//! use claude_wrapper::slash;
//!
//! assert_eq!(slash::compact(None), "/compact");
//! assert_eq!(slash::compact(Some("focus on auth bug")), "/compact focus on auth bug");
//! ```

/// Build the prompt string for `/compact`.
///
/// Pass `Some(instructions)` to mirror the CLI's
/// `/compact <instructions>` form (the trailing text steers the
/// summarizer); pass `None` for a default compaction. Empty or
/// whitespace-only `instructions` collapse to the bare form.
pub fn compact(instructions: Option<&str>) -> String {
    match instructions {
        Some(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                "/compact".to_string()
            } else {
                format!("/compact {trimmed}")
            }
        }
        None => "/compact".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_without_instructions_is_bare() {
        assert_eq!(compact(None), "/compact");
    }

    #[test]
    fn compact_with_instructions_appends_them() {
        assert_eq!(
            compact(Some("focus on the auth bug")),
            "/compact focus on the auth bug"
        );
    }

    #[test]
    fn compact_trims_surrounding_whitespace() {
        assert_eq!(compact(Some("   keep tests   ")), "/compact keep tests");
    }

    #[test]
    fn compact_empty_string_collapses_to_bare() {
        assert_eq!(compact(Some("")), "/compact");
        assert_eq!(compact(Some("   ")), "/compact");
    }
}
